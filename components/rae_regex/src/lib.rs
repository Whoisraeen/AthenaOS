//! # RaeRegex — a never-panic, `no_std`, ReDoS-safe regular-expression engine.
//!
//! LEGACY_GAMING_CONCEPT.md §"The user owns the machine": the machine answers to its
//! owner, not the other way around — which includes never letting an untrusted
//! pattern or an untrusted input string wedge the system. Regular expressions are
//! everywhere a text tool lives (find/replace in editors, input validation,
//! search), and a naive backtracking engine turns a one-line pattern like
//! `(a+)+$` into an exponential-time denial-of-service. RaeRegex is therefore
//! built on a **Thompson NFA** simulated by a **Pike VM**: there is no
//! backtracking, matching is linear in `text.len() * program.len()`, and the
//! compiled program is size-bounded so a pathological pattern is rejected at
//! compile time rather than at run time. One correct, dependency-free,
//! hostile-input regex core serves every consumer — so this crate is foundational
//! infrastructure, deliberately wired into none this slice.
//!
//! ## Hostile-input posture (CLAUDE: parsers of untrusted bytes are an RCE surface)
//! Every pattern handed to [`Regex::new`] and every text handed to a match method
//! is treated as hostile. There is **no `unwrap`/`expect`/`panic`/raw-index-panic
//! path** reachable from compile or match: unbalanced parens/brackets, bad
//! `{n,m}` ranges, dangling escapes, and absurdly large repetitions all return
//! `Err(RegexError)`; matching operates on decoded `char`s so a multibyte UTF-8
//! sequence is never split and byte offsets always land on char boundaries.
//!
//! ## Supported syntax (a practical subset)
//! - Literals and escaped metacharacters: `\. \* \+ \? \( \) \[ \] \{ \} \| \^ \$ \\`.
//! - `.` — any character except newline.
//! - Character classes: `[abc]`, ranges `[a-z]`, negation `[^...]`, escapes inside
//!   classes (`[\d\s_]`), and a literal `]` as the first member (`[]a]`).
//! - Perl shorthands: `\d \w \s` and their negations `\D \W \S` (also usable inside
//!   classes).
//! - Quantifiers: `*` `+` `?` and bounded `{n}` `{n,}` `{n,m}` (greedy). Lazy
//!   variants (`*?` etc.) are **not** supported — see "Omissions" below.
//! - Alternation `|`, grouping `(...)` (capturing) and `(?:...)` (non-capturing).
//! - Anchors `^` and `$` (start/end of text; `$` also matches before a trailing
//!   `\n` is **not** implemented — `$` is end-of-text only, multiline mode omitted).
//!
//! ## Documented omissions (intentional, not bugs)
//! - Lazy/non-greedy quantifiers (`*?`, `+?`, `??`, `{n,m}?`). All quantifiers are
//!   greedy; the leftmost-longest-by-greedy semantics are deterministic.
//! - Backreferences (`\1`) — these are *what makes a regex non-regular* and force
//!   backtracking; they are deliberately unsupported because they reintroduce the
//!   ReDoS class this engine exists to prevent.
//! - Multiline (`^`/`$` per line), word boundaries `\b`, Unicode property classes
//!   `\p{…}`, and inline flags `(?i)`.
//! - Captures across [`Regex::find_all`] are not collected (only group spans of a
//!   single [`Regex::captures`] call). `replace_all` supports `$1` group refs.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

/// Maximum number of compiled instructions a pattern may produce. A pattern that
/// would compile to more than this (e.g. `a{100000}{100000}`) is rejected with
/// [`RegexError::ProgramTooLarge`] *before* the allocation happens — this is the
/// memory-safety bound for the hostile-input posture, not a regex-spec limit.
pub const MAX_PROGRAM_LEN: usize = 100_000;

/// Maximum nesting depth of groups the parser will accept. A pattern nested
/// deeper than this is rejected with [`RegexError::NestingTooDeep`] before the
/// recursion can exhaust the stack.
pub const MAX_DEPTH: usize = 256;

/// Maximum repetition count accepted in a `{n}` / `{n,m}` bound. Counts above
/// this are rejected with [`RegexError::RepetitionTooLarge`]; combined with
/// [`MAX_PROGRAM_LEN`] this caps the blow-up of bounded repetition.
pub const MAX_REPEAT: u32 = 10_000;

// ─────────────────────────────────────────────────────────────────────────────
// Errors
// ─────────────────────────────────────────────────────────────────────────────

/// Why a pattern failed to compile. Every malformed pattern maps to one of these;
/// [`Regex::new`] never panics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegexError {
    /// A `(` with no matching `)`, or `)` with no matching `(`.
    UnbalancedParen,
    /// A `[` with no matching `]`.
    UnbalancedClass,
    /// A `{` quantifier that is not a well-formed `{n}` / `{n,}` / `{n,m}`.
    BadRepetition,
    /// A `{n,m}` with `m < n`.
    RepetitionOutOfOrder,
    /// A repetition count exceeding [`MAX_REPEAT`].
    RepetitionTooLarge,
    /// A quantifier (`*`, `+`, `?`, `{…}`) with nothing to repeat.
    NothingToRepeat,
    /// A `\` at end of pattern, or `\x` where `x` is not a recognized escape.
    BadEscape,
    /// A character class with a reversed range like `[z-a]`.
    BadClassRange,
    /// Group nesting exceeded [`MAX_DEPTH`].
    NestingTooDeep,
    /// The compiled program would exceed [`MAX_PROGRAM_LEN`] instructions.
    ProgramTooLarge,
}

// ─────────────────────────────────────────────────────────────────────────────
// AST
// ─────────────────────────────────────────────────────────────────────────────

/// One member of a character class: either a single char or an inclusive range.
#[derive(Debug, Clone, PartialEq)]
enum ClassItem {
    Char(char),
    Range(char, char),
    /// A shorthand (`\d \w \s` etc.) embedded in a class; `bool` is "negated".
    Shorthand(Shorthand),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shorthand {
    Digit,    // \d
    NotDigit, // \D
    Word,     // \w
    NotWord,  // \W
    Space,    // \s
    NotSpace, // \S
}

impl Shorthand {
    fn matches(self, c: char) -> bool {
        let is_digit = c.is_ascii_digit();
        let is_word = c.is_ascii_alphanumeric() || c == '_';
        let is_space =
            c == ' ' || c == '\t' || c == '\n' || c == '\r' || c == '\u{0B}' || c == '\u{0C}';
        match self {
            Shorthand::Digit => is_digit,
            Shorthand::NotDigit => !is_digit,
            Shorthand::Word => is_word,
            Shorthand::NotWord => !is_word,
            Shorthand::Space => is_space,
            Shorthand::NotSpace => !is_space,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Ast {
    /// Match exactly this character.
    Literal(char),
    /// `.` — any char except `\n`.
    AnyChar,
    /// A character class; `negated` flips the membership test.
    Class {
        negated: bool,
        items: Vec<ClassItem>,
    },
    /// A Perl shorthand outside a class.
    Shorthand(Shorthand),
    /// `^` start anchor.
    StartAnchor,
    /// `$` end anchor.
    EndAnchor,
    /// Concatenation of sub-expressions.
    Concat(Vec<Ast>),
    /// `a|b|c`.
    Alternate(Vec<Ast>),
    /// A quantified sub-expression. `min`/`max` (`max == None` => unbounded).
    Repeat {
        node: alloc::boxed::Box<Ast>,
        min: u32,
        max: Option<u32>,
    },
    /// A group. `cap` is `Some(index)` for capturing groups, `None` for `(?:…)`.
    Group {
        cap: Option<usize>,
        node: alloc::boxed::Box<Ast>,
    },
    /// The empty expression (matches the empty string).
    Empty,
}

// ─────────────────────────────────────────────────────────────────────────────
// Parser  (pattern &str  ->  Ast)
// ─────────────────────────────────────────────────────────────────────────────

struct Parser {
    chars: Vec<char>,
    pos: usize,
    /// Next capture-group index to hand out (group 0 is the implicit whole match).
    next_group: usize,
}

impl Parser {
    fn new(pattern: &str) -> Self {
        Parser {
            chars: pattern.chars().collect(),
            pos: 0,
            next_group: 1,
        }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.chars.get(self.pos).copied();
        if c.is_some() {
            self.pos += 1;
        }
        c
    }

    fn eat(&mut self, c: char) -> bool {
        if self.peek() == Some(c) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    /// Parse a full alternation (the top of the grammar at the current depth).
    fn parse_alternation(&mut self, depth: usize) -> Result<Ast, RegexError> {
        if depth > MAX_DEPTH {
            return Err(RegexError::NestingTooDeep);
        }
        let mut branches = Vec::new();
        branches.push(self.parse_concat(depth)?);
        while self.eat('|') {
            branches.push(self.parse_concat(depth)?);
        }
        if branches.len() == 1 {
            Ok(branches.pop().unwrap_or(Ast::Empty))
        } else {
            Ok(Ast::Alternate(branches))
        }
    }

    /// Parse a concatenation of quantified atoms up to `|`, `)`, or end.
    fn parse_concat(&mut self, depth: usize) -> Result<Ast, RegexError> {
        let mut parts = Vec::new();
        loop {
            match self.peek() {
                None | Some('|') | Some(')') => break,
                _ => {}
            }
            let atom = self.parse_quantified(depth)?;
            parts.push(atom);
        }
        if parts.is_empty() {
            Ok(Ast::Empty)
        } else if parts.len() == 1 {
            Ok(parts.pop().unwrap_or(Ast::Empty))
        } else {
            Ok(Ast::Concat(parts))
        }
    }

    /// Parse a single atom plus any trailing quantifier(s).
    fn parse_quantified(&mut self, depth: usize) -> Result<Ast, RegexError> {
        let atom = self.parse_atom(depth)?;
        // A quantifier must follow a real atom, never an anchor or nothing.
        match self.peek() {
            Some('*') => {
                self.bump();
                self.check_repeatable(&atom)?;
                Ok(Ast::Repeat {
                    node: alloc::boxed::Box::new(atom),
                    min: 0,
                    max: None,
                })
            }
            Some('+') => {
                self.bump();
                self.check_repeatable(&atom)?;
                Ok(Ast::Repeat {
                    node: alloc::boxed::Box::new(atom),
                    min: 1,
                    max: None,
                })
            }
            Some('?') => {
                self.bump();
                self.check_repeatable(&atom)?;
                Ok(Ast::Repeat {
                    node: alloc::boxed::Box::new(atom),
                    min: 0,
                    max: Some(1),
                })
            }
            Some('{') => {
                // Could be a bounded repetition or a literal `{` — only treat as a
                // repetition if it parses as one. We attempt, and on a malformed
                // brace return BadRepetition (strict; a literal `{` must be `\{`).
                // NOTE: lazy quantifiers (`*?`, `{n,m}?`) are a documented omission;
                // a following `?` is parsed as its own atom and yields
                // NothingToRepeat, keeping the greedy-only semantics explicit.
                let (min, max) = self.parse_brace()?;
                if max.is_some() && max.unwrap_or(0) < min {
                    return Err(RegexError::RepetitionOutOfOrder);
                }
                self.check_repeatable(&atom)?;
                Ok(Ast::Repeat {
                    node: alloc::boxed::Box::new(atom),
                    min,
                    max,
                })
            }
            _ => Ok(atom),
        }
    }

    fn check_repeatable(&self, atom: &Ast) -> Result<(), RegexError> {
        match atom {
            Ast::Empty | Ast::StartAnchor | Ast::EndAnchor => Err(RegexError::NothingToRepeat),
            // Disallow stacking quantifiers like `a**` (ambiguous / pathological).
            Ast::Repeat { .. } => Err(RegexError::NothingToRepeat),
            _ => Ok(()),
        }
    }

    /// Parse `{n}` / `{n,}` / `{n,m}`. Assumes the next char is `{`.
    fn parse_brace(&mut self) -> Result<(u32, Option<u32>), RegexError> {
        // consume '{'
        self.bump();
        let min = self.parse_number()?;
        let max;
        if self.eat(',') {
            if self.peek() == Some('}') {
                max = None; // {n,}
            } else {
                max = Some(self.parse_number()?);
            }
        } else {
            max = Some(min); // {n}
        }
        if !self.eat('}') {
            return Err(RegexError::BadRepetition);
        }
        Ok((min, max))
    }

    fn parse_number(&mut self) -> Result<u32, RegexError> {
        let mut digits = 0usize;
        let mut value: u64 = 0;
        while let Some(c) = self.peek() {
            if let Some(d) = c.to_digit(10) {
                self.bump();
                digits += 1;
                value = value * 10 + d as u64;
                if value > MAX_REPEAT as u64 {
                    return Err(RegexError::RepetitionTooLarge);
                }
            } else {
                break;
            }
        }
        if digits == 0 {
            return Err(RegexError::BadRepetition);
        }
        Ok(value as u32)
    }

    /// Parse a single atom: literal, `.`, class, group, anchor, shorthand, escape.
    fn parse_atom(&mut self, depth: usize) -> Result<Ast, RegexError> {
        match self.peek() {
            None => Ok(Ast::Empty),
            Some('(') => self.parse_group(depth),
            Some('[') => self.parse_class(),
            Some('.') => {
                self.bump();
                Ok(Ast::AnyChar)
            }
            Some('^') => {
                self.bump();
                Ok(Ast::StartAnchor)
            }
            Some('$') => {
                self.bump();
                Ok(Ast::EndAnchor)
            }
            Some('\\') => self.parse_escape(),
            // A quantifier with nothing before it is an error.
            Some('*') | Some('+') | Some('?') => Err(RegexError::NothingToRepeat),
            Some(')') => Err(RegexError::UnbalancedParen),
            Some(']') => {
                // A bare `]` is treated as a literal (matches common engines).
                self.bump();
                Ok(Ast::Literal(']'))
            }
            Some('{') => {
                // A `{` that is not a valid quantifier here (no preceding atom) is
                // a literal brace.
                self.bump();
                Ok(Ast::Literal('{'))
            }
            Some('}') => {
                self.bump();
                Ok(Ast::Literal('}'))
            }
            Some(c) => {
                self.bump();
                Ok(Ast::Literal(c))
            }
        }
    }

    fn parse_group(&mut self, depth: usize) -> Result<Ast, RegexError> {
        // consume '('
        self.bump();
        let cap = if self.peek() == Some('?') && self.chars.get(self.pos + 1).copied() == Some(':')
        {
            self.bump(); // ?
            self.bump(); // :
            None
        } else {
            let idx = self.next_group;
            self.next_group += 1;
            Some(idx)
        };
        let inner = self.parse_alternation(depth + 1)?;
        if !self.eat(')') {
            return Err(RegexError::UnbalancedParen);
        }
        Ok(Ast::Group {
            cap,
            node: alloc::boxed::Box::new(inner),
        })
    }

    fn parse_escape(&mut self) -> Result<Ast, RegexError> {
        // consume '\'
        self.bump();
        match self.bump() {
            None => Err(RegexError::BadEscape),
            Some('d') => Ok(Ast::Shorthand(Shorthand::Digit)),
            Some('D') => Ok(Ast::Shorthand(Shorthand::NotDigit)),
            Some('w') => Ok(Ast::Shorthand(Shorthand::Word)),
            Some('W') => Ok(Ast::Shorthand(Shorthand::NotWord)),
            Some('s') => Ok(Ast::Shorthand(Shorthand::Space)),
            Some('S') => Ok(Ast::Shorthand(Shorthand::NotSpace)),
            Some('n') => Ok(Ast::Literal('\n')),
            Some('r') => Ok(Ast::Literal('\r')),
            Some('t') => Ok(Ast::Literal('\t')),
            Some('f') => Ok(Ast::Literal('\u{0C}')),
            Some('v') => Ok(Ast::Literal('\u{0B}')),
            Some('0') => Ok(Ast::Literal('\0')),
            // Escaped metacharacters and a literal backslash.
            Some(
                c @ ('.' | '*' | '+' | '?' | '(' | ')' | '[' | ']' | '{' | '}' | '|' | '^' | '$'
                | '\\' | '/' | '-'),
            ) => Ok(Ast::Literal(c)),
            // Any other escape is rejected rather than silently treated literal,
            // so a dangling/garbage escape is a compile error (hostile-input).
            Some(_) => Err(RegexError::BadEscape),
        }
    }

    /// Parse one class member's leading char, honoring escapes. Returns either a
    /// concrete char or a shorthand (shorthands cannot form a range endpoint).
    fn parse_class_atom(&mut self) -> Result<ClassAtom, RegexError> {
        match self.bump() {
            None => Err(RegexError::UnbalancedClass),
            Some('\\') => match self.bump() {
                None => Err(RegexError::UnbalancedClass),
                Some('d') => Ok(ClassAtom::Shorthand(Shorthand::Digit)),
                Some('D') => Ok(ClassAtom::Shorthand(Shorthand::NotDigit)),
                Some('w') => Ok(ClassAtom::Shorthand(Shorthand::Word)),
                Some('W') => Ok(ClassAtom::Shorthand(Shorthand::NotWord)),
                Some('s') => Ok(ClassAtom::Shorthand(Shorthand::Space)),
                Some('S') => Ok(ClassAtom::Shorthand(Shorthand::NotSpace)),
                Some('n') => Ok(ClassAtom::Char('\n')),
                Some('r') => Ok(ClassAtom::Char('\r')),
                Some('t') => Ok(ClassAtom::Char('\t')),
                Some('f') => Ok(ClassAtom::Char('\u{0C}')),
                Some('v') => Ok(ClassAtom::Char('\u{0B}')),
                Some('0') => Ok(ClassAtom::Char('\0')),
                Some(
                    c @ ('.' | '*' | '+' | '?' | '(' | ')' | '[' | ']' | '{' | '}' | '|' | '^'
                    | '$' | '\\' | '/' | '-'),
                ) => Ok(ClassAtom::Char(c)),
                Some(_) => Err(RegexError::BadEscape),
            },
            Some(c) => Ok(ClassAtom::Char(c)),
        }
    }

    fn parse_class(&mut self) -> Result<Ast, RegexError> {
        // consume '['
        self.bump();
        let negated = self.eat('^');
        let mut items: Vec<ClassItem> = Vec::new();

        // A `]` immediately after `[` or `[^` is a literal `]`.
        if self.peek() == Some(']') {
            self.bump();
            items.push(ClassItem::Char(']'));
        }

        loop {
            match self.peek() {
                None => return Err(RegexError::UnbalancedClass),
                Some(']') => {
                    self.bump();
                    break;
                }
                _ => {}
            }
            let first = self.parse_class_atom()?;
            // Possible range: <char> '-' <char>, but only when both ends are chars
            // and the '-' is not the closing-bracket-adjacent dash.
            if let ClassAtom::Char(lo) = first {
                if self.peek() == Some('-')
                    && self.chars.get(self.pos + 1).copied() != Some(']')
                    && self.chars.get(self.pos + 1).is_some()
                {
                    self.bump(); // consume '-'
                    let second = self.parse_class_atom()?;
                    match second {
                        ClassAtom::Char(hi) => {
                            if (hi as u32) < (lo as u32) {
                                return Err(RegexError::BadClassRange);
                            }
                            items.push(ClassItem::Range(lo, hi));
                        }
                        // `a-\d` is nonsense; treat dash literally instead.
                        ClassAtom::Shorthand(sh) => {
                            items.push(ClassItem::Char(lo));
                            items.push(ClassItem::Char('-'));
                            items.push(ClassItem::Shorthand(sh));
                        }
                    }
                    continue;
                }
            }
            match first {
                ClassAtom::Char(c) => items.push(ClassItem::Char(c)),
                ClassAtom::Shorthand(sh) => items.push(ClassItem::Shorthand(sh)),
            }
        }

        if items.is_empty() {
            // `[]` (no members, `]` already consumed as literal above means this
            // only triggers on a truly empty class) — treat as never-match class.
            // We keep it valid but matching nothing.
        }
        Ok(Ast::Class { negated, items })
    }
}

enum ClassAtom {
    Char(char),
    Shorthand(Shorthand),
}

fn class_matches(negated: bool, items: &[ClassItem], c: char) -> bool {
    let mut hit = false;
    for item in items {
        let m = match item {
            ClassItem::Char(ch) => *ch == c,
            ClassItem::Range(lo, hi) => (*lo as u32) <= (c as u32) && (c as u32) <= (*hi as u32),
            ClassItem::Shorthand(sh) => sh.matches(c),
        };
        if m {
            hit = true;
            break;
        }
    }
    hit ^ negated
}

// ─────────────────────────────────────────────────────────────────────────────
// Instruction set  (Ast  ->  Program)  — Thompson NFA as a flat instruction list
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
enum Inst {
    /// Consume one char if it equals this literal, then advance to pc+1.
    Char(char),
    /// Consume one char if it is not `\n`.
    AnyChar,
    /// Consume one char if it satisfies the class.
    Class {
        negated: bool,
        items: Vec<ClassItem>,
    },
    /// Consume one char if it satisfies the shorthand.
    Shorthand(Shorthand),
    /// Fork: try `a` then `b` (priority order = greedy/leftmost).
    Split(usize, usize),
    /// Unconditional jump.
    Jmp(usize),
    /// Save the current input position into capture slot `n`.
    Save(usize),
    /// Assert start of text.
    AssertStart,
    /// Assert end of text.
    AssertEnd,
    /// Accept.
    Match,
}

struct Compiler {
    prog: Vec<Inst>,
    n_slots: usize,
}

impl Compiler {
    fn new(n_groups: usize) -> Self {
        // Two slots per group (start,end); group 0 is the whole match.
        Compiler {
            prog: Vec::new(),
            n_slots: n_groups * 2,
        }
    }

    fn push(&mut self, inst: Inst) -> Result<usize, RegexError> {
        if self.prog.len() >= MAX_PROGRAM_LEN {
            return Err(RegexError::ProgramTooLarge);
        }
        let pc = self.prog.len();
        self.prog.push(inst);
        Ok(pc)
    }

    fn emit(&mut self, ast: &Ast) -> Result<(), RegexError> {
        match ast {
            Ast::Empty => Ok(()),
            Ast::Literal(c) => {
                self.push(Inst::Char(*c))?;
                Ok(())
            }
            Ast::AnyChar => {
                self.push(Inst::AnyChar)?;
                Ok(())
            }
            Ast::Class { negated, items } => {
                self.push(Inst::Class {
                    negated: *negated,
                    items: items.clone(),
                })?;
                Ok(())
            }
            Ast::Shorthand(sh) => {
                self.push(Inst::Shorthand(*sh))?;
                Ok(())
            }
            Ast::StartAnchor => {
                self.push(Inst::AssertStart)?;
                Ok(())
            }
            Ast::EndAnchor => {
                self.push(Inst::AssertEnd)?;
                Ok(())
            }
            Ast::Concat(parts) => {
                for p in parts {
                    self.emit(p)?;
                }
                Ok(())
            }
            Ast::Alternate(branches) => self.emit_alternate(branches),
            Ast::Group { cap, node } => {
                if let Some(idx) = cap {
                    self.push(Inst::Save(idx * 2))?;
                    self.emit(node)?;
                    self.push(Inst::Save(idx * 2 + 1))?;
                } else {
                    self.emit(node)?;
                }
                Ok(())
            }
            Ast::Repeat { node, min, max } => self.emit_repeat(node, *min, *max),
        }
    }

    fn emit_alternate(&mut self, branches: &[Ast]) -> Result<(), RegexError> {
        // Emits the same code shape as the former right-nested recursion —
        //   split L_body, L_next ; L_body: branch ; jmp END ; L_next: <rest>
        // for each branch but the last (which falls through to END) — only
        // ITERATIVELY, so a merely WIDE alternation (`a|a|…` thousands of times,
        // trivially pasted into an editor find box) cannot overflow the stack at
        // compile time. `Regex::new` is documented to never panic on any pattern.
        if branches.is_empty() {
            return Ok(());
        }
        if branches.len() == 1 {
            return self.emit(&branches[0]);
        }
        let last = branches.len() - 1;
        let mut jmp_pcs: Vec<usize> = Vec::with_capacity(last);
        for (idx, branch) in branches.iter().enumerate() {
            if idx == last {
                // Final branch: no split, no trailing jmp — it falls into END.
                self.emit(branch)?;
                break;
            }
            // split L_body, L_next ; L_body: branch ; jmp END
            let split_pc = self.push(Inst::Split(0, 0))?;
            let l_body = self.prog.len();
            self.emit(branch)?;
            let jmp_pc = self.push(Inst::Jmp(0))?;
            jmp_pcs.push(jmp_pc);
            let l_next = self.prog.len();
            self.prog[split_pc] = Inst::Split(l_body, l_next);
        }
        let end = self.prog.len();
        for jmp_pc in jmp_pcs {
            self.prog[jmp_pc] = Inst::Jmp(end);
        }
        Ok(())
    }

    fn emit_repeat(&mut self, node: &Ast, min: u32, max: Option<u32>) -> Result<(), RegexError> {
        // Bounded lower part: emit `node` `min` times.
        for _ in 0..min {
            self.emit(node)?;
            self.guard_size()?;
        }
        match max {
            None => {
                // Unbounded tail: a Kleene-star-style loop.
                // L1: split L2, L3 ; L2: node ; jmp L1 ; L3:
                let l1 = self.push(Inst::Split(0, 0))?;
                let l2 = self.prog.len();
                self.emit(node)?;
                self.push(Inst::Jmp(l1))?;
                let l3 = self.prog.len();
                self.prog[l1] = Inst::Split(l2, l3);
            }
            Some(max) => {
                // Optional copies: (max - min) of `split L_body, L_end ; node`.
                let extra = max.saturating_sub(min);
                let mut split_pcs: Vec<usize> = Vec::new();
                for _ in 0..extra {
                    let sp = self.push(Inst::Split(0, 0))?;
                    split_pcs.push(sp);
                    let body = self.prog.len();
                    self.emit(node)?;
                    self.prog[sp] = Inst::Split(body, 0); // patch second arm later
                    self.guard_size()?;
                }
                let end = self.prog.len();
                for sp in split_pcs {
                    if let Inst::Split(a, _) = self.prog[sp] {
                        self.prog[sp] = Inst::Split(a, end);
                    }
                }
            }
        }
        Ok(())
    }

    fn guard_size(&self) -> Result<(), RegexError> {
        if self.prog.len() >= MAX_PROGRAM_LEN {
            Err(RegexError::ProgramTooLarge)
        } else {
            Ok(())
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Pike VM  (parallel NFA simulation with capture tracking — no backtracking)
// ─────────────────────────────────────────────────────────────────────────────

/// A compiled regular expression. Construct with [`Regex::new`]; it never panics
/// at match time.
#[derive(Debug, Clone)]
pub struct Regex {
    prog: Vec<Inst>,
    n_slots: usize,
    n_groups: usize,
}

/// A successful match: a half-open byte range `[start, end)` into the text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Match {
    pub start: usize,
    pub end: usize,
}

/// Capture spans of a single match. Index 0 is the whole match; index `n` is the
/// `n`-th capturing group (`None` if that group did not participate).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Captures {
    spans: Vec<Option<Match>>,
}

impl Captures {
    /// Span of capture group `i` (group 0 = whole match). `None` if the group did
    /// not participate or `i` is out of range.
    pub fn get(&self, i: usize) -> Option<Match> {
        self.spans.get(i).copied().flatten()
    }

    /// Number of capture slots (group 0 included), i.e. groups + 1.
    pub fn len(&self) -> usize {
        self.spans.len()
    }

    /// True if there are no capture slots (never the case for a valid match).
    pub fn is_empty(&self) -> bool {
        self.spans.is_empty()
    }
}

/// A decoded character with its byte offset in the original text.
#[derive(Clone, Copy)]
struct Step {
    ch: char,
    byte: usize,
}

impl Regex {
    /// Compile `pattern`. Returns `Err(RegexError)` for any malformed pattern;
    /// never panics.
    pub fn new(pattern: &str) -> Result<Regex, RegexError> {
        let mut parser = Parser::new(pattern);
        let ast = parser.parse_alternation(0)?;
        // Any leftover `)` means an unbalanced close.
        if parser.pos != parser.chars.len() {
            return match parser.peek() {
                Some(')') => Err(RegexError::UnbalancedParen),
                _ => Err(RegexError::UnbalancedParen),
            };
        }
        let n_groups = parser.next_group; // includes group 0
        let mut comp = Compiler::new(n_groups);
        // Wrap whole program in Save(0) … Save(1) … Match.
        comp.push(Inst::Save(0))?;
        comp.emit(&ast)?;
        comp.push(Inst::Save(1))?;
        comp.push(Inst::Match)?;
        Ok(Regex {
            prog: comp.prog,
            n_slots: comp.n_slots.max(2),
            n_groups,
        })
    }

    /// True if the pattern matches anywhere in `text`.
    pub fn is_match(&self, text: &str) -> bool {
        self.find(text).is_some()
    }

    /// Leftmost match in `text`, as a byte range. `None` if no match.
    pub fn find(&self, text: &str) -> Option<Match> {
        self.captures_from(text, 0).and_then(|c| c.spans[0])
    }

    /// Leftmost match with capture-group spans. `None` if no match.
    pub fn captures(&self, text: &str) -> Option<Captures> {
        self.captures_from(text, 0)
    }

    /// All non-overlapping matches, left to right. An empty match advances one
    /// char to guarantee termination.
    pub fn find_all(&self, text: &str) -> Vec<Match> {
        let mut out = Vec::new();
        let mut start = 0usize;
        let bytes_len = text.len();
        loop {
            if start > bytes_len {
                break;
            }
            match self.captures_from(text, start) {
                Some(caps) => {
                    let m = match caps.spans[0] {
                        Some(m) => m,
                        None => break,
                    };
                    out.push(m);
                    if m.end > start {
                        start = m.end;
                    } else {
                        // Zero-width match: step forward one char to avoid looping.
                        start = next_char_boundary(text, m.end);
                        if start <= m.end && m.end >= bytes_len {
                            break;
                        }
                    }
                }
                None => break,
            }
        }
        out
    }

    /// Replace all non-overlapping matches with `repl`. `repl` supports `$0`..`$9`
    /// group references (an unknown `$n` expands to empty); a literal `$` is
    /// written as `$$`.
    pub fn replace_all(&self, text: &str, repl: &str) -> String {
        let mut out = String::new();
        let mut last = 0usize;
        let bytes_len = text.len();
        let mut start = 0usize;
        loop {
            if start > bytes_len {
                break;
            }
            let caps = match self.captures_from(text, start) {
                Some(c) => c,
                None => break,
            };
            let m = match caps.spans[0] {
                Some(m) => m,
                None => break,
            };
            // Append the gap before this match.
            out.push_str(&text[last..m.start]);
            expand_repl(repl, &caps, text, &mut out);
            last = m.end;
            if m.end > start {
                start = m.end;
            } else {
                start = next_char_boundary(text, m.end);
                if m.end >= bytes_len {
                    break;
                }
            }
        }
        out.push_str(&text[last..]);
        out
    }

    /// Core: run the Pike VM searching for the leftmost match at or after
    /// `start_byte`. The outer search loop tries each starting position in turn;
    /// the inner simulation runs all threads in lock-step over the input.
    fn captures_from(&self, text: &str, start_byte: usize) -> Option<Captures> {
        // Decode the (sub)text from start_byte into (char, byte-offset) steps.
        let steps: Vec<Step> = text[start_byte..]
            .char_indices()
            .map(|(i, ch)| Step {
                ch,
                byte: start_byte + i,
            })
            .collect();

        let prog_len = self.prog.len();
        // `clist`/`nlist` hold the active threads for the current/next input pos.
        // Each thread carries its own capture-slot vector (Pike VM).
        let mut clist: Vec<Thread> = Vec::new();
        let mut nlist: Vec<Thread> = Vec::new();
        // `seen` dedupes pcs per step so the simulation stays O(steps * prog).
        let mut seen_gen: Vec<u32> = vec![0; prog_len];
        let mut gen: u32 = 0;

        let mut matched: Option<Vec<usize>> = None;

        // Iterate over input positions 0..=steps.len() (one past end for $/Match).
        let n = steps.len();
        let mut sp = 0usize;
        loop {
            // Current input position: byte offset and whether we're at text start.
            let at_start_of_text = if sp == 0 { start_byte == 0 } else { false };
            // Byte offset at this position.
            let pos_byte = if sp < n { steps[sp].byte } else { text.len() };

            // Seed a new thread at the program entry for this start position,
            // but only while we have not yet found a match (leftmost semantics).
            // We seed at every position so `find` locates the leftmost start.
            gen = gen.wrapping_add(1);
            // Re-key the seen vector per step.
            // (Using a generation counter avoids reallocating each iteration.)

            // Add the initial thread (entry pc 0) and close its epsilon-closure.
            if matched.is_none() {
                let init_slots = vec![usize::MAX; self.n_slots];
                self.add_thread(
                    &mut clist,
                    &mut seen_gen,
                    gen,
                    0,
                    &init_slots,
                    pos_byte,
                    at_start_of_text,
                    sp >= n,
                );
            } else {
                // Still need to close epsilon for already-present threads; they
                // were added in the previous step's nlist swap.
                // (Handled below by processing clist as-is.)
            }

            // Process all threads in clist against the current char.
            nlist.clear();
            let next_gen = gen.wrapping_add(1);
            let mut i = 0;
            while i < clist.len() {
                let pc = clist[i].pc;
                let inst = &self.prog[pc];
                match inst {
                    Inst::Char(expected) => {
                        if sp < n && steps[sp].ch == *expected {
                            let next_byte = if sp + 1 < n {
                                steps[sp + 1].byte
                            } else {
                                text.len()
                            };
                            let slots = clist[i].slots.clone();
                            self.add_thread(
                                &mut nlist,
                                &mut seen_gen,
                                next_gen,
                                pc + 1,
                                &slots,
                                next_byte,
                                false,
                                sp + 1 >= n,
                            );
                        }
                    }
                    Inst::AnyChar => {
                        if sp < n && steps[sp].ch != '\n' {
                            let next_byte = if sp + 1 < n {
                                steps[sp + 1].byte
                            } else {
                                text.len()
                            };
                            let slots = clist[i].slots.clone();
                            self.add_thread(
                                &mut nlist,
                                &mut seen_gen,
                                next_gen,
                                pc + 1,
                                &slots,
                                next_byte,
                                false,
                                sp + 1 >= n,
                            );
                        }
                    }
                    Inst::Class { negated, items } => {
                        if sp < n && class_matches(*negated, items, steps[sp].ch) {
                            let next_byte = if sp + 1 < n {
                                steps[sp + 1].byte
                            } else {
                                text.len()
                            };
                            let slots = clist[i].slots.clone();
                            self.add_thread(
                                &mut nlist,
                                &mut seen_gen,
                                next_gen,
                                pc + 1,
                                &slots,
                                next_byte,
                                false,
                                sp + 1 >= n,
                            );
                        }
                    }
                    Inst::Shorthand(sh) => {
                        if sp < n && sh.matches(steps[sp].ch) {
                            let next_byte = if sp + 1 < n {
                                steps[sp + 1].byte
                            } else {
                                text.len()
                            };
                            let slots = clist[i].slots.clone();
                            self.add_thread(
                                &mut nlist,
                                &mut seen_gen,
                                next_gen,
                                pc + 1,
                                &slots,
                                next_byte,
                                false,
                                sp + 1 >= n,
                            );
                        }
                    }
                    Inst::Match => {
                        // Highest-priority thread to reach Match wins (greedy /
                        // leftmost). Record and stop processing lower-priority
                        // threads in this step.
                        matched = Some(clist[i].slots.clone());
                        break;
                    }
                    // Split/Jmp/Save/Assert are handled during epsilon-closure in
                    // add_thread, so they never appear here.
                    _ => {}
                }
                i += 1;
            }

            // Move to next position.
            core::mem::swap(&mut clist, &mut nlist);
            gen = next_gen;

            if sp >= n {
                break;
            }
            sp += 1;
        }

        matched.map(|slots| self.slots_to_captures(&slots))
    }

    /// Epsilon-closure: follow Split/Jmp/Save/Assert without consuming input,
    /// adding reachable consuming/Match pcs to `list`. Recursion is bounded by the
    /// per-step `seen` dedupe (each pc visited at most once per generation), so it
    /// cannot loop forever even on `(a*)*` — the ReDoS guarantee.
    #[allow(clippy::too_many_arguments)]
    fn add_thread(
        &self,
        list: &mut Vec<Thread>,
        seen: &mut [u32],
        gen: u32,
        pc: usize,
        slots: &[usize],
        pos_byte: usize,
        at_text_start: bool,
        at_text_end: bool,
    ) {
        // Iterative worklist to avoid deep recursion on long epsilon chains; each
        // entry carries its own capture-slot vector so forks don't alias.
        let mut stack: Vec<(usize, Vec<usize>)> = Vec::new();
        stack.push((pc, slots.to_vec()));

        while let Some((pc, mut cur)) = stack.pop() {
            if pc >= self.prog.len() {
                continue;
            }
            if seen[pc] == gen {
                continue;
            }
            seen[pc] = gen;
            match &self.prog[pc] {
                Inst::Jmp(t) => {
                    stack.push((*t, cur));
                }
                Inst::Split(a, b) => {
                    // Priority order: `a` before `b`. Because this is a stack
                    // (LIFO), push `b` first so `a` is processed first.
                    stack.push((*b, cur.clone()));
                    stack.push((*a, cur));
                }
                Inst::Save(slot) => {
                    if *slot < cur.len() {
                        cur[*slot] = pos_byte;
                    }
                    stack.push((pc + 1, cur));
                }
                Inst::AssertStart => {
                    if at_text_start {
                        stack.push((pc + 1, cur));
                    }
                }
                Inst::AssertEnd => {
                    if at_text_end {
                        stack.push((pc + 1, cur));
                    }
                }
                // Consuming instructions and Match are added to the active list.
                Inst::Char(_)
                | Inst::AnyChar
                | Inst::Class { .. }
                | Inst::Shorthand(_)
                | Inst::Match => {
                    list.push(Thread { pc, slots: cur });
                }
            }
        }
    }

    fn slots_to_captures(&self, slots: &[usize]) -> Captures {
        let mut spans = Vec::with_capacity(self.n_groups);
        for g in 0..self.n_groups {
            let s = slots.get(g * 2).copied().unwrap_or(usize::MAX);
            let e = slots.get(g * 2 + 1).copied().unwrap_or(usize::MAX);
            if s != usize::MAX && e != usize::MAX && s <= e {
                spans.push(Some(Match { start: s, end: e }));
            } else {
                spans.push(None);
            }
        }
        Captures { spans }
    }
}

/// A live NFA thread: a program counter plus its capture slots.
#[derive(Clone)]
struct Thread {
    pc: usize,
    slots: Vec<usize>,
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Next char boundary at or after `byte` (used to advance over zero-width
/// matches without ever splitting a codepoint).
fn next_char_boundary(text: &str, byte: usize) -> usize {
    if byte >= text.len() {
        return text.len();
    }
    let mut b = byte + 1;
    while b < text.len() && !text.is_char_boundary(b) {
        b += 1;
    }
    b
}

/// Expand a replacement template into `out`, honoring `$0`..`$9` group refs and
/// `$$` for a literal dollar.
fn expand_repl(repl: &str, caps: &Captures, text: &str, out: &mut String) {
    let chars: Vec<char> = repl.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '$' {
            if i + 1 < chars.len() {
                let next = chars[i + 1];
                if next == '$' {
                    out.push('$');
                    i += 2;
                    continue;
                }
                if let Some(d) = next.to_digit(10) {
                    if let Some(m) = caps.get(d as usize) {
                        out.push_str(&text[m.start..m.end]);
                    }
                    i += 2;
                    continue;
                }
            }
            // Lone `$` at end or before a non-digit: emit literally.
            out.push('$');
            i += 1;
        } else {
            out.push(c);
            i += 1;
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Host KAT suite  (cargo test -p rae_regex)  — the primary proof.
// FAIL-ability: each test asserts a concrete value; the notes below name which
// assert flips if a specific feature is broken.
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;

    fn re(p: &str) -> Regex {
        Regex::new(p).expect("pattern should compile")
    }

    // ── Literals & `.` ───────────────────────────────────────────────────────

    #[test]
    fn literal_match() {
        let r = re("abc");
        assert!(r.is_match("xxabcyy"));
        assert!(!r.is_match("abx"));
        let m = r.find("xxabcyy").expect("should find");
        assert_eq!(m, Match { start: 2, end: 5 });
        // FAIL-ability: break the Char instruction and this offset flips.
        assert_ne!(m.start, m.end);
    }

    #[test]
    fn any_char() {
        let r = re("a.c");
        assert!(r.is_match("abc"));
        assert!(r.is_match("axc"));
        assert!(!r.is_match("a\nc")); // `.` excludes newline
        assert!(!r.is_match("ac"));
    }

    // ── Anchors ──────────────────────────────────────────────────────────────

    #[test]
    fn anchors() {
        let r = re("^abc$");
        assert!(r.is_match("abc"));
        assert!(!r.is_match("xabc"));
        assert!(!r.is_match("abcx"));
        // ^ only
        let s = re("^foo");
        assert!(s.is_match("foobar"));
        assert!(!s.is_match("afoobar"));
        // $ only
        let e = re("bar$");
        assert!(e.is_match("foobar"));
        assert!(!e.is_match("barfoo"));
    }

    // ── Character classes ────────────────────────────────────────────────────

    #[test]
    fn classes() {
        let r = re("[a-z]+");
        let m = r.find("123abc456").expect("find");
        assert_eq!(m, Match { start: 3, end: 6 });

        let neg = re("[^0-9]");
        assert!(neg.is_match("a"));
        assert!(!neg.is_match("5"));

        let set = re("[abc]");
        assert!(set.is_match("b"));
        assert!(!set.is_match("d"));

        // FAIL-ability: break negation handling and `[^0-9]` matches '5'.
        assert!(!re("[^0-9]").is_match("7"));
    }

    #[test]
    fn class_literal_bracket_and_dash() {
        // `]` as first member is literal; trailing `-` is literal.
        let r = re("[]a-]");
        assert!(r.is_match("]"));
        assert!(r.is_match("a"));
        assert!(r.is_match("-"));
        assert!(!r.is_match("b"));
    }

    // ── Shorthands ───────────────────────────────────────────────────────────

    #[test]
    fn shorthands() {
        let phone = re(r"\d{3}-\d{4}");
        let m = phone.find("call 555-1234 now").expect("find");
        assert_eq!(m, Match { start: 5, end: 13 });
        assert!(!phone.is_match("55-1234"));

        assert!(re(r"\w+").is_match("hello_9"));
        assert!(re(r"\s").is_match(" "));
        assert!(!re(r"\S").is_match(" "));
        assert!(re(r"\D").is_match("a"));
        assert!(!re(r"\D").is_match("5"));
    }

    // ── Quantifiers ──────────────────────────────────────────────────────────

    #[test]
    fn quantifiers() {
        assert_eq!(re("a*").find("aaa"), Some(Match { start: 0, end: 3 }));
        assert_eq!(re("a*").find("b"), Some(Match { start: 0, end: 0 })); // zero-width OK
        assert!(re("a+").is_match("a"));
        assert!(!re("a+").is_match("b"));
        assert_eq!(re("a+").find("baaa"), Some(Match { start: 1, end: 4 }));
        assert!(re("a?b").is_match("b"));
        assert!(re("a?b").is_match("ab"));

        // bounded {n}, {n,}, {n,m}
        assert!(re("a{2,3}").is_match("aa"));
        assert!(re("a{2,3}").is_match("aaa"));
        assert!(!re("a{2,3}").is_match("a"));
        assert_eq!(re("a{2,3}").find("aaaa"), Some(Match { start: 0, end: 3 })); // greedy, capped at 3
        assert!(re("a{2}").is_match("aa"));
        assert!(!re("a{2}").is_match("a"));
        assert!(re("a{2,}").is_match("aaaaa"));

        // FAIL-ability: if {n,m} bound is broken, this caps wrong.
        let m = re("a{2,3}").find("aaaaa").expect("find");
        assert_eq!(m.end - m.start, 3);
    }

    // ── Alternation ──────────────────────────────────────────────────────────

    #[test]
    fn alternation() {
        let r = re("cat|dog");
        assert!(r.is_match("i have a cat"));
        assert!(r.is_match("i have a dog"));
        assert!(!r.is_match("i have a fish"));
        assert_eq!(r.find("a dog"), Some(Match { start: 2, end: 5 }));

        // grouped alternation
        let g = re("(cat|dog)s");
        assert!(g.is_match("cats"));
        assert!(g.is_match("dogs"));
        assert!(!g.is_match("fishs"));

        // FAIL-ability: break Split priority/wiring and one branch stops matching.
        assert!(re("a|b|c").is_match("c"));
        assert!(!re("a|b|c").is_match("d"));
    }

    // ── Groups & captures ────────────────────────────────────────────────────

    #[test]
    fn captures_basic() {
        let r = re(r"(\d{4})-(\d{2})-(\d{2})");
        let caps = r.captures("date: 2026-06-18.").expect("captures");
        assert_eq!(caps.get(0), Some(Match { start: 6, end: 16 }));
        assert_eq!(caps.get(1), Some(Match { start: 6, end: 10 }));
        assert_eq!(caps.get(2), Some(Match { start: 11, end: 13 }));
        assert_eq!(caps.get(3), Some(Match { start: 14, end: 16 }));
        assert_eq!(caps.len(), 4); // group0 + 3 groups
        assert_eq!(caps.get(9), None); // out of range
    }

    #[test]
    fn non_capturing_group() {
        let r = re(r"(?:ab)+(c)");
        let caps = r.captures("ababc").expect("captures");
        assert_eq!(caps.get(0), Some(Match { start: 0, end: 5 }));
        // Only one capturing group (the `(c)`), so index 1 is `c`.
        assert_eq!(caps.get(1), Some(Match { start: 4, end: 5 }));
        assert_eq!(caps.len(), 2);
    }

    // ── find_all / replace_all ───────────────────────────────────────────────

    #[test]
    fn find_all_non_overlapping() {
        let r = re("ab");
        let all = r.find_all("ababab");
        assert_eq!(all.len(), 3);
        assert_eq!(all[0], Match { start: 0, end: 2 });
        assert_eq!(all[1], Match { start: 2, end: 4 });
        assert_eq!(all[2], Match { start: 4, end: 6 });

        // overlapping-looking pattern stays non-overlapping
        let aa = re("aa");
        assert_eq!(aa.find_all("aaaa").len(), 2);
    }

    #[test]
    fn replace_all_literal_and_refs() {
        let r = re(r"\d+");
        assert_eq!(r.replace_all("a1b22c333", "#"), "a#b#c#");
        // group ref
        let date = re(r"(\d{4})-(\d{2})-(\d{2})");
        assert_eq!(date.replace_all("2026-06-18", "$3/$2/$1"), "18/06/2026");
        // literal $$
        let w = re(r"\d+");
        assert_eq!(w.replace_all("cost 5", "$$"), "cost $");
    }

    // ── ReDoS safety (the load-bearing guard) ────────────────────────────────

    #[test]
    fn redos_catastrophic_pattern_completes() {
        // A classic catastrophic-backtracking pattern. On a backtracking engine,
        // matching this against "aaaa...aaab" is exponential and hangs. On this
        // Thompson NFA / Pike VM it is linear and returns immediately.
        let r = re("(a+)+$");
        let mut s = String::new();
        for _ in 0..50 {
            s.push('a');
        }
        s.push('b'); // forces overall failure
                     // The point is: this returns (does not hang). The bool is also correct:
                     // "$"-anchored, the trailing 'b' means no full match ending in a's.
        assert!(!r.is_match(&s));

        // And a string it *should* match returns true, also fast.
        let mut ok = String::new();
        for _ in 0..50 {
            ok.push('a');
        }
        assert!(r.is_match(&ok));

        // Another classic: nested alternation star.
        let r2 = re("(a|aa)+$");
        let mut t = String::new();
        for _ in 0..40 {
            t.push('a');
        }
        t.push('b');
        assert!(!r2.is_match(&t)); // completes quickly, no exponential blowup
    }

    // ── Bad patterns -> Err, never panic ─────────────────────────────────────

    #[test]
    fn bad_patterns_are_errors() {
        assert_eq!(Regex::new("a(").unwrap_err(), RegexError::UnbalancedParen);
        assert_eq!(Regex::new("a)").unwrap_err(), RegexError::UnbalancedParen);
        assert_eq!(Regex::new("[a").unwrap_err(), RegexError::UnbalancedClass);
        assert_eq!(
            Regex::new("a{2,1}").unwrap_err(),
            RegexError::RepetitionOutOfOrder
        );
        assert_eq!(Regex::new("*").unwrap_err(), RegexError::NothingToRepeat);
        assert_eq!(Regex::new("+abc").unwrap_err(), RegexError::NothingToRepeat);
        assert!(matches!(Regex::new("a\\"), Err(RegexError::BadEscape)));
        assert_eq!(Regex::new("[z-a]").unwrap_err(), RegexError::BadClassRange);
        // Absurd repetition is rejected (ProgramTooLarge or RepetitionTooLarge),
        // never an OOM/panic.
        assert!(Regex::new("a{1000000}").is_err());
        // A single in-range bound just below MAX_REPEAT compiles but stays bounded.
        assert!(Regex::new("a{9999}").is_ok());
        // A nested bounded repetition that would blow the program past
        // MAX_PROGRAM_LEN is rejected with ProgramTooLarge — not an OOM/panic.
        assert_eq!(
            Regex::new("(a{1000}){1000}").unwrap_err(),
            RegexError::ProgramTooLarge
        );
    }

    #[test]
    fn empty_pattern_and_text() {
        let r = re("");
        // Empty pattern matches the empty string at position 0.
        assert_eq!(r.find(""), Some(Match { start: 0, end: 0 }));
        assert_eq!(r.find("abc"), Some(Match { start: 0, end: 0 }));
        assert!(r.is_match(""));

        // Non-empty pattern against empty text.
        assert_eq!(re("a").find(""), None);
        assert!(!re("a").is_match(""));
        assert_eq!(re("a*").find(""), Some(Match { start: 0, end: 0 }));
    }

    // ── UTF-8 correctness ────────────────────────────────────────────────────

    #[test]
    fn utf8_multibyte() {
        // "héllo wörld" — é and ö are 2-byte UTF-8 sequences.
        let text = "héllo wörld";
        let r = re("wörld");
        let m = r.find(text).expect("find");
        // Verify offsets land on char boundaries and slice correctly.
        assert!(text.is_char_boundary(m.start));
        assert!(text.is_char_boundary(m.end));
        assert_eq!(&text[m.start..m.end], "wörld");

        // `.` matches a multibyte char as a single unit.
        let dot = re("h.llo");
        assert!(dot.is_match("héllo"));
        let dm = dot.find("héllo").expect("find");
        assert_eq!(&text[..0], ""); // no-op boundary sanity
        assert_eq!("héllo".get(dm.start..dm.end), Some("héllo"));

        // A class with a multibyte member.
        let cls = re("[áé]");
        assert!(cls.is_match("café"));
        let cm = cls.find("café").expect("find");
        assert_eq!("café".get(cm.start..cm.end), Some("é"));

        // Quantified emoji (4-byte) does not split.
        let emo = re("😀+");
        let etext = "x😀😀y";
        let em = emo.find(etext).expect("find");
        assert_eq!(etext.get(em.start..em.end), Some("😀😀"));
    }

    #[test]
    fn escaped_metacharacters() {
        let r = re(r"a\.b");
        assert!(r.is_match("a.b"));
        assert!(!r.is_match("axb")); // `\.` is a literal dot, not any-char
        let p = re(r"\(\)");
        assert!(p.is_match("()"));
        let star = re(r"a\*");
        assert!(star.is_match("a*"));
        assert!(!star.is_match("aaa"));
    }

    #[test]
    fn leftmost_semantics() {
        // find returns the leftmost match, not the longest-overall.
        let r = re("a|ab");
        // At position 0, alternation tries `a` first (higher priority) → "a".
        assert_eq!(r.find("ab"), Some(Match { start: 0, end: 1 }));
        // But a real prefix search still anchors leftmost.
        assert_eq!(re("b").find("aab"), Some(Match { start: 2, end: 3 }));
    }

    #[test]
    fn to_string_error_is_debuggable() {
        // RegexError derives Debug — used in tooling. Sanity check it formats.
        let e = Regex::new("(").unwrap_err();
        let s = alloc::format!("{:?}", e);
        assert_eq!(s, "UnbalancedParen".to_string());
    }

    // =======================================================================
    // FUZZ / PROPERTY suite — deterministic seeded PRNG, no external fuzz crate.
    //
    // Matches the rae_json/rae_toml/rae_gif pattern that landed this session. The
    // properties under test are the hostile-input invariants of this engine — the
    // pattern AND the text are both untrusted (editor find/replace, search, input
    // validation; CLAUDE: parsers of untrusted bytes are an RCE surface):
    //   A. [`Regex::new`] never panics: any pattern compiles to `Ok` or `Err`.
    //   B. Every match method (`is_match`/`find`/`captures`/`find_all`/
    //      `replace_all`) never panics, never produces an out-of-bounds / non-char-
    //      boundary span, and always terminates.
    //   C. ReDoS-resistance — the load-bearing security property: a pathological
    //      pattern (`(a+)+$`, `(a|aa)*c`, deep nesting, huge bounded reps) run
    //      against a LONG input terminates. Because this is a Thompson NFA driven
    //      by a Pike VM (no backtracking; `add_thread` dedupes pcs per input step),
    //      matching is O(text.len() * program.len()) — linear, not exponential.
    //
    // FAIL-ability (proven by reasoning in the REPORT): a backtracking engine would
    // make the ReDoS cases run effectively forever, timing the test out (= FAIL).
    // A spurious panic anywhere flips the relevant `is_ok`/no-panic property to a
    // hard FAIL. The span-validity property asserts concrete `is_char_boundary` /
    // `start <= end <= len` invariants, which a slicing bug would violate.
    // =======================================================================

    /// Deterministic xorshift64* PRNG — pure, no_std-safe, reproducible.
    struct Rng(u64);
    impl Rng {
        fn new(seed: u64) -> Self {
            Rng(seed ^ 0x9E37_79B9_7F4A_7C15)
        }
        fn next_u64(&mut self) -> u64 {
            let mut x = self.0;
            x ^= x >> 12;
            x ^= x << 25;
            x ^= x >> 27;
            self.0 = x;
            x.wrapping_mul(0x2545_F491_4F6C_DD1D)
        }
        fn below(&mut self, n: usize) -> usize {
            (self.next_u64() % (n as u64)) as usize
        }
    }

    /// Run the full match API against `text` and assert every returned span is
    /// valid (in-bounds, on char boundaries, start <= end) and that none of the
    /// methods panic. Used by the fuzz tests to enforce property B uniformly.
    fn exercise(re: &Regex, text: &str) {
        let _ = re.is_match(text);
        if let Some(m) = re.find(text) {
            assert!(m.start <= m.end, "span start>end: {:?}", m);
            assert!(m.end <= text.len(), "span past end: {:?}", m);
            assert!(
                text.is_char_boundary(m.start),
                "start off boundary: {:?}",
                m
            );
            assert!(text.is_char_boundary(m.end), "end off boundary: {:?}", m);
            // The slice must not panic.
            let _ = &text[m.start..m.end];
        }
        if let Some(caps) = re.captures(text) {
            for i in 0..caps.len() {
                if let Some(m) = caps.get(i) {
                    assert!(m.start <= m.end && m.end <= text.len());
                    assert!(text.is_char_boundary(m.start) && text.is_char_boundary(m.end));
                }
            }
        }
        let all = re.find_all(text);
        // Non-overlapping & monotonic: each match starts at or after the previous
        // match's end (the engine guarantees this; a regression would break it).
        let mut prev_end = 0usize;
        for m in &all {
            assert!(m.start >= prev_end || m.start == m.end, "overlap: {:?}", m);
            assert!(m.end <= text.len());
            assert!(text.is_char_boundary(m.start) && text.is_char_boundary(m.end));
            prev_end = m.end;
        }
        let _ = re.replace_all(text, "<$1>");
        let _ = re.replace_all(text, "$$x$0");
    }

    /// A1. Random pattern soup over the regex-significant alphabet: `Regex::new`
    /// must return Ok or Err, NEVER panic. Whatever compiles is then matched
    /// against a battery of texts (property B).
    #[test]
    fn fuzz_regex_pattern_soup_never_panics() {
        const ALPHABET: &[u8] = b"()[]{}|*+?.^$\\dDwWsS-,0123abcz \t\n:/^()()[a-z][^0-9]";
        let texts = [
            "",
            "a",
            "abc123",
            "aaaaa",
            "the quick brown fox 42!",
            "  \n\t  ",
        ];
        let mut rng = Rng::new(0x5_0117);
        let mut compiled = 0u32;
        for _ in 0..120_000 {
            let len = rng.below(40);
            let mut p = String::with_capacity(len);
            for _ in 0..len {
                p.push(ALPHABET[rng.below(ALPHABET.len())] as char);
            }
            match Regex::new(&p) {
                Ok(re) => {
                    compiled += 1;
                    for t in &texts {
                        exercise(&re, t);
                    }
                }
                Err(_) => {} // malformed → Err, never panic. Acceptable.
            }
        }
        // Sanity: the soup must have compiled at least *some* patterns, else the
        // exercise() path (property B) was never taken — a false-green guard.
        assert!(
            compiled > 0,
            "no random pattern compiled — exercise() unreached"
        );
    }

    /// A2. Random UTF-8 patterns AND random UTF-8 texts (multibyte / control /
    /// emoji): compile never panics; match never splits a codepoint.
    #[test]
    fn fuzz_regex_utf8_pattern_and_text() {
        let palette: &[char] = &[
            '(', ')', '[', ']', '|', '*', '+', '?', '.', '^', '$', '\\', 'd', 'w', 's', '-', 'a',
            '0', ' ', '\n', '\u{0}', '\u{7f}', 'é', 'ö', '中', '😀', '\u{feff}',
        ];
        let mut rng = Rng::new(0x5_0317);
        for _ in 0..80_000 {
            let plen = rng.below(24);
            let mut p = String::new();
            for _ in 0..plen {
                p.push(palette[rng.below(palette.len())]);
            }
            let tlen = rng.below(24);
            let mut t = String::new();
            for _ in 0..tlen {
                t.push(palette[rng.below(palette.len())]);
            }
            if let Ok(re) = Regex::new(&p) {
                exercise(&re, &t);
            }
        }
    }

    /// A3. Mutate a battery of VALID patterns char-wise: deletions / swaps /
    /// insertions of metacharacters. Each mutant compiles to Ok or Err, never
    /// panics; the Ok ones match a fixed text without panicking.
    #[test]
    fn fuzz_regex_mutated_valid_patterns() {
        let seeds = [
            r"(\d{4})-(\d{2})-(\d{2})",
            r"[a-z]+@[a-z]+\.[a-z]+",
            r"(cat|dog|fish)s?",
            r"^\s*#.*$",
            r"(?:ab)+(c|d)*",
            r"a{2,5}b{1,3}",
        ];
        let inject: &[char] = &[
            '(', ')', '[', ']', '{', '}', '|', '*', '+', '?', '\\', '.', ',', '-',
        ];
        let mut rng = Rng::new(0x5_0517);
        for _ in 0..80_000 {
            let seed = seeds[rng.below(seeds.len())];
            let mut c: Vec<char> = seed.chars().collect();
            let mutations = 1 + rng.below(4);
            for _ in 0..mutations {
                if c.is_empty() {
                    break;
                }
                match rng.below(3) {
                    0 => {
                        let i = rng.below(c.len());
                        c.remove(i);
                    }
                    1 => {
                        let i = rng.below(c.len());
                        c[i] = inject[rng.below(inject.len())];
                    }
                    _ => {
                        let i = rng.below(c.len());
                        c.insert(i, inject[rng.below(inject.len())]);
                    }
                }
            }
            let p: String = c.iter().collect();
            if let Ok(re) = Regex::new(&p) {
                exercise(&re, "2026-06-21 cat dog #note a@b.io aabbb");
            }
        }
    }

    /// A4. Explicit pathological-pattern battery: nested quantifiers, huge `{n,m}`,
    /// unbalanced parens/brackets, huge alternations, backref-looking syntax,
    /// dangling escapes. Each must compile to Ok or Err — never panic, never hang —
    /// and the program-size / repeat / depth caps must reject the truly enormous
    /// ones (rather than OOM).
    #[test]
    fn fuzz_regex_pathological_patterns() {
        let mut cases: Vec<String> = vec![
            "(a+)+".to_string(),
            "(a+)+$".to_string(),
            "(a*)*".to_string(),
            "(a|a)*".to_string(),
            "(a|aa)*$".to_string(),
            "(.*)*".to_string(),
            "((((((((((a))))))))))".to_string(),
            "a{10000}".to_string(),
            "a{10001}".to_string(), // over MAX_REPEAT → Err
            "a{0,10000}".to_string(),
            "(a{1000}){1000}".to_string(), // over MAX_PROGRAM_LEN → Err
            "\\1\\2\\3".to_string(),       // backref-looking → BadEscape
            "(a)\\1".to_string(),          // backref → BadEscape
            "[".to_string(),
            "]".to_string(),
            "(".to_string(),
            ")".to_string(),
            "[a-".to_string(),
            "[z-a]".to_string(),
            "a**".to_string(),
            "*".to_string(),
            "+".to_string(),
            "?".to_string(),
            "{2,3}".to_string(),
            "a{,3}".to_string(),
            "a{2,1}".to_string(),
            "\\".to_string(),
            "\\q".to_string(),
            "(?:".to_string(),
            "(?".to_string(),
        ];
        // Deeply nested groups, far past MAX_DEPTH → NestingTooDeep, never overflow.
        let deep = {
            let mut s = String::new();
            for _ in 0..5000 {
                s.push('(');
            }
            s.push('a');
            for _ in 0..5000 {
                s.push(')');
            }
            s
        };
        cases.push(deep);
        // Flat alternation at a stack-SAFE width. A much wider one overflows at
        // compile time — see DEFECT below + `wide_alternation_overflows_regression`.
        let wide_alt = {
            let mut s = String::new();
            for i in 0..400 {
                if i > 0 {
                    s.push('|');
                }
                s.push('a');
            }
            s
        };
        cases.push(wide_alt);
        // Long run of nested quantifier groups (parser+compiler iterate here; safe).
        cases.push("(a+)+".repeat(2000));

        for p in &cases {
            // The contract: Ok or Err, no panic, terminates.
            if let Ok(re) = Regex::new(p) {
                exercise(&re, "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaab");
            }
        }

        // Specific caps must trigger (FAIL-ability for the bound logic):
        assert!(Regex::new("a{10001}").is_err(), "MAX_REPEAT cap missing");
        assert!(
            Regex::new("(a{1000}){1000}").is_err(),
            "MAX_PROGRAM_LEN cap missing"
        );
        let deep2 = {
            let mut s = String::new();
            for _ in 0..(MAX_DEPTH + 50) {
                s.push('(');
            }
            s.push('a');
            for _ in 0..(MAX_DEPTH + 50) {
                s.push(')');
            }
            s
        };
        assert_eq!(
            Regex::new(&deep2).unwrap_err(),
            RegexError::NestingTooDeep,
            "MAX_DEPTH cap missing"
        );
        // Backref syntax MUST be rejected (it is the ReDoS-reintroducing feature).
        assert!(matches!(Regex::new(r"(a)\1"), Err(RegexError::BadEscape)));
    }

    /// REGRESSION GUARD (no_std-clean): proves the COMPILER does not overflow the
    /// stack on a wide flat alternation.
    ///
    /// DEFECT (now fixed): `Compiler::emit_alternate` compiled `a|b|c|…` as
    /// right-nested `a | (b | (c | …))`, recursing once per branch with NO width
    /// cap. The parser flattens alternation iteratively and `MAX_DEPTH` only bounds
    /// GROUP nesting, so a merely-WIDE pattern (e.g. `a|a|a|…` thousands of times,
    /// trivially pasted into an editor find box) overflowed the stack at compile
    /// time. `Regex::new` is documented to "never panic"; the iterative
    /// `emit_alternate` rewrite restores that.
    ///
    /// FALSIFIABLE: 20_000 branches recursed once each is far beyond the default
    /// ~8 MiB test stack, so removing the iterative rewrite makes this `Regex::new`
    /// abort the process during compile — i.e. the test stops passing without the
    /// fix (no small-stack thread needed: the width alone exhausts any sane stack).
    #[test]
    fn wide_alternation_overflows_regression() {
        // 20_000 branches: uncapped right-recursion is one frame per branch, which
        // exceeds the default test stack, so without the iterative emit this aborts.
        let pattern = "a|".repeat(20_000) + "a";
        // The load-bearing assertion: compiling RETURNS (never overflows / panics).
        assert!(
            Regex::new(&pattern).is_ok(),
            "wide flat alternation must compile without overflowing the stack: \
             Compiler::emit_alternate must stay iterative (no per-branch recursion)."
        );
    }

    /// A5. THE ReDoS GUARANTEE. Classic catastrophic-backtracking patterns matched
    /// against LONG inputs MUST terminate (linear time). On a backtracking engine
    /// `(a+)+$` vs "a"*N + "b" is O(2^N) and never returns; here it is linear and
    /// returns instantly. We scale N up to 100k to make a non-linear engine hang
    /// the test (the falsifiable proof of linearity). Correctness of the boolean
    /// is asserted too, so a "return early / wrong answer" cheat can't pass.
    #[test]
    fn fuzz_regex_redos_linear_time() {
        // `(bool, pattern)`: bool = "the trailing 'b' provably blocks ANY match"
        // (true only when the body cannot match the empty string at end-of-text
        // and 'b' is not in its alphabet). For the empty-matchable ones (`(a*)*$`,
        // `(a|a)*$`) a zero-width match before `$` is the CORRECT answer, so we only
        // require termination — the ReDoS property — not non-match.
        let evil_patterns: &[(bool, &str)] = &[
            (true, "(a+)+$"),
            (false, "(a*)*$"),
            (false, "(a|a)*$"),
            (true, "(a|aa)+$"),
            (false, "(.*a){20}"),
            (false, "(a?){50}a{50}"),
            (false, "([a-z]+)*$"), // 'b' IS [a-z] → can match; only require terminate
        ];
        // N=30k is far beyond where a backtracking engine would already hang for
        // minutes; keeping it modest holds the whole linear suite well under a
        // sane unit-test budget while still proving termination.
        for &(blocks, pat) in evil_patterns {
            let re = Regex::new(pat).expect("evil pattern should still compile");
            for &n in &[1_000usize, 5_000, 30_000] {
                // Worst case for a backtracker: all 'a' then a 'b'.
                let mut bad = String::with_capacity(n + 1);
                for _ in 0..n {
                    bad.push('a');
                }
                bad.push('b');
                // The load-bearing assertion is that this RETURNS (no ReDoS hang).
                let got = re.is_match(&bad);
                if blocks {
                    // Proves it didn't "return early with a stale true".
                    assert!(!got, "{pat:?} wrongly matched {n} a's + b");
                }
                // A matching input also returns fast.
                let good = "a".repeat(n);
                let _ = re.is_match(&good);
            }
        }
    }

    /// A6. ReDoS via `find_all` / `replace_all` over a long hostile input: the
    /// outer search loop tries every start position, so an O(n^2) blow-up would
    /// hang here if the inner sim were not linear. Must terminate.
    #[test]
    fn fuzz_regex_redos_find_all_terminates() {
        let re = Regex::new("(a+)+b").expect("compile");
        let mut s = String::with_capacity(50_001);
        for _ in 0..50_000 {
            s.push('a');
        }
        s.push('b');
        // One match at the end; the engine must reach it without blowing up.
        let all = re.find_all(&s);
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].end, s.len());

        // replace_all over a long input with many zero-width opportunities.
        let star = Regex::new("a*").expect("compile");
        let long_a = "a".repeat(20_000);
        let replaced = star.replace_all(&long_a, "X");
        // a* matches the whole run once then a zero-width at end; must terminate
        // and not loop forever on the zero-width match.
        assert!(replaced.contains('X'));
    }

    /// A7. Match against pathological TEXT (not pattern): huge inputs, all-same
    /// char, control/UTF-8 soup, against a benign pattern — never panic, terminate.
    #[test]
    fn fuzz_regex_pathological_text() {
        // Huge single-pass searches (find / is_match are O(n)): these stay fast
        // even at 200k chars. We use single-pass methods here rather than the full
        // `exercise` helper because `find_all`/`replace_all` re-decode the tail at
        // every match start (an O(n^2) constant — it TERMINATES, no defect, but it
        // would make a 200k * many-matches case needlessly slow; that quadratic
        // re-decode is noted in the REPORT as a perf observation, not a bug).
        let re = Regex::new(r"\w+").expect("compile");
        for big in [
            "a".repeat(200_000),
            "\u{0}".repeat(50_000),
            "😀".repeat(20_000),
            " \t\n".repeat(50_000),
        ] {
            let _ = re.is_match(&big);
            if let Some(m) = re.find(&big) {
                assert!(big.is_char_boundary(m.start) && big.is_char_boundary(m.end));
                assert!(m.end <= big.len());
                let _ = &big[m.start..m.end];
            }
        }

        // A capturing pattern with groups against a multi-match input — full
        // `exercise` (find_all + replace_all) at a moderate size to keep it quick.
        let g = Regex::new(r"(\d+)-(\d+)").expect("compile");
        let mut s = String::new();
        for _ in 0..1_500 {
            s.push_str("12-34 ");
        }
        exercise(&g, &s);

        // Empty / zero-width patterns against long input must terminate find_all
        // (the zero-width-advance guard prevents an infinite loop — a real risk).
        let empty = Regex::new("").expect("compile");
        let m = empty.find_all(&"x".repeat(5_000));
        assert_eq!(m.len(), 5_001); // a zero-width match at every boundary + end
        let opt = Regex::new("a?").expect("compile");
        let _ = opt.find_all(&"b".repeat(5_000));
    }
}
