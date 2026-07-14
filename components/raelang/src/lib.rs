//! Rae script — Concept §Customization Engine: "Scripting layer — Swift
//! scripts for automation, no PowerShell archaeology required."
//!
//! A small, Swift-flavored language. v0.2 surface:
//!
//! - `let`/`var` bindings (immutability checked, including through paths:
//!   `let a = [1]` rejects `a[0] = 2` just like Swift)
//! - `Int`, `Float`, `Bool`, `String` (with interpolation `"\(expr)"`),
//!   `Array` (`[1, 2, 3]`), `Dictionary` (`["k": v]`, empty `[:]`),
//!   ranges (`0..<10`, `1...10`), structs, and first-class functions
//! - `if`/`else`, `while`, `for x in …` (ranges, arrays, dictionary keys,
//!   string characters), `break`/`continue`, `func`/`return`
//! - closures `{ x in x * 2 }` (capture-by-value snapshot of the enclosing
//!   scope), passable to `.map`/`.filter`/`.reduce` and user functions
//! - methods/properties on values: `.count`, `.isEmpty`, `.append(v)`,
//!   `.contains(x)`, `.uppercased()`, `.split(sep)`, `.keys`, `.map(f)`, …
//! - builtins: `print(...)` (output captured for the caller), `String(x)`,
//!   `Int(x)`, `Float(x)`, `abs(x)`, `min(a,b)`, `max(a,b)`
//!
//! **Host bindings**: the embedder passes a [`Host`] to [`run_with_host`];
//! any call the script didn't define and isn't a builtin is offered to the
//! host (`notify(...)`, `setAccent(...)`, …). The host decides per call —
//! including *denying* it, which surfaces in-script as a
//! [`RaeError::CapabilityDenied`]. This is how the kernel enforces the
//! user-authorized `cap_mask` on automation scripts (RaeShield: every
//! privileged op is capability-gated, scripts included).
//!
//! The interpreter is a fuel-limited tree-walker: a runaway loop runs out
//! of fuel deterministically instead of hanging its host — the property
//! the kernel's script lifecycle (`SCRIPT_RUN`/`SCRIPT_KILL`) needs to
//! keep automation safe.
//!
//! `no_std` + `alloc` so the SAME interpreter runs in the kernel scripting
//! layer (inline/automation scripts) and the userspace `raelangd` daemon —
//! one language, one implementation, host-testable with
//! `cargo test -p raelang`.

#![no_std]

extern crate alloc;

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

// ───────────────────────────── Errors ──────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RaeError {
    Lex(String),
    Parse(String),
    UndefinedVariable(String),
    UndefinedFunction(String),
    AssignToLet(String),
    TypeMismatch(String),
    DivisionByZero,
    ArityMismatch(String),
    OutOfFuel,
    RecursionLimit,
    IndexOutOfBounds(String),
    NoSuchField(String),
    /// `break`/`continue` escaped every loop (top level or function body).
    StrayControlFlow,
    /// The host refused a system binding the script's cap_mask doesn't grant.
    CapabilityDenied(String),
    /// The host accepted the call but the underlying operation failed.
    HostFailed(String),
}

// ───────────────────────────── Values ──────────────────────────────────

/// A function value: a `func` referenced by name or a closure literal.
/// Closures snapshot the visible bindings at creation (capture BY VALUE —
/// mutations inside the closure don't leak back out; deterministic and
/// allocation-bounded, which is what kernel-side automation wants).
#[derive(Debug)]
pub struct FuncVal {
    params: Vec<String>,
    body: Vec<Stmt>,
    captured: BTreeMap<String, (Value, bool)>,
}

#[derive(Debug, Clone)]
pub enum Value {
    Int(i64),
    Float(f64),
    Bool(bool),
    Str(String),
    Array(Vec<Value>),
    Dict(BTreeMap<String, Value>),
    Range {
        start: i64,
        end: i64,
        inclusive: bool,
    },
    Func(Rc<FuncVal>),
    Struct {
        name: String,
        fields: BTreeMap<String, Value>,
    },
    Unit,
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Value::Int(a), Value::Int(b)) => a == b,
            (Value::Float(a), Value::Float(b)) => a == b,
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::Str(a), Value::Str(b)) => a == b,
            (Value::Array(a), Value::Array(b)) => a == b,
            (Value::Dict(a), Value::Dict(b)) => a == b,
            (
                Value::Range {
                    start: a,
                    end: b,
                    inclusive: i,
                },
                Value::Range {
                    start: c,
                    end: d,
                    inclusive: j,
                },
            ) => a == c && b == d && i == j,
            (Value::Func(a), Value::Func(b)) => Rc::ptr_eq(a, b),
            (Value::Struct { name: a, fields: f }, Value::Struct { name: b, fields: g }) => {
                a == b && f == g
            }
            (Value::Unit, Value::Unit) => true,
            _ => false,
        }
    }
}

impl Value {
    pub fn display(&self) -> String {
        match self {
            Value::Int(n) => format!("{}", n),
            Value::Float(f) => format!("{}", f),
            Value::Bool(b) => format!("{}", b),
            Value::Str(s) => s.clone(),
            Value::Array(items) => {
                let mut s = String::from("[");
                for (i, v) in items.iter().enumerate() {
                    if i > 0 {
                        s.push_str(", ");
                    }
                    s.push_str(&v.display());
                }
                s.push(']');
                s
            }
            Value::Dict(map) => {
                if map.is_empty() {
                    return String::from("[:]");
                }
                let mut s = String::from("[");
                for (i, (k, v)) in map.iter().enumerate() {
                    if i > 0 {
                        s.push_str(", ");
                    }
                    s.push_str(k);
                    s.push_str(": ");
                    s.push_str(&v.display());
                }
                s.push(']');
                s
            }
            Value::Range {
                start,
                end,
                inclusive,
            } => format!("{}{}{}", start, if *inclusive { "..." } else { "..<" }, end),
            Value::Func(_) => String::from("<func>"),
            Value::Struct { name, fields } => {
                let mut s = String::new();
                s.push_str(name);
                s.push('(');
                for (i, (k, v)) in fields.iter().enumerate() {
                    if i > 0 {
                        s.push_str(", ");
                    }
                    s.push_str(k);
                    s.push_str(": ");
                    s.push_str(&v.display());
                }
                s.push(')');
                s
            }
            Value::Unit => String::from("()"),
        }
    }

    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Int(_) => "Int",
            Value::Float(_) => "Float",
            Value::Bool(_) => "Bool",
            Value::Str(_) => "String",
            Value::Array(_) => "Array",
            Value::Dict(_) => "Dictionary",
            Value::Range { .. } => "Range",
            Value::Func(_) => "Function",
            Value::Struct { .. } => "Struct",
            Value::Unit => "Unit",
        }
    }
}

// ───────────────────────────── Host bindings ───────────────────────────

/// Why a host binding call didn't produce a value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostError {
    /// The host doesn't export a function by this name — the script gets
    /// a plain `UndefinedFunction`, indistinguishable from a typo.
    Unknown,
    /// The host exports it but the caller's capability mask doesn't grant
    /// it. Fails the script closed (`RaeError::CapabilityDenied`).
    Denied(String),
    /// Granted, attempted, and the underlying system op failed.
    Failed(String),
}

/// System-binding surface an embedder exposes to scripts. Every call a
/// script makes that isn't a script-defined function, struct, or builtin
/// is offered here. The kernel's implementation gates each name on the
/// submitting user's `cap_mask` (RaeShield model: deny by default).
pub trait Host {
    fn call(&mut self, name: &str, args: &[Value]) -> Result<Value, HostError>;
}

/// The no-bindings host: pure computation only ([`run`] uses this).
pub struct NoHost;

impl Host for NoHost {
    fn call(&mut self, _name: &str, _args: &[Value]) -> Result<Value, HostError> {
        Err(HostError::Unknown)
    }
}

// ───────────────────────────── Lexer ───────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Int(i64),
    Float(f64),
    Str(Vec<StrTok>),
    Ident(String),
    // keywords
    Let,
    Var,
    If,
    Else,
    While,
    For,
    In,
    Break,
    Continue,
    Func,
    Return,
    Struct,
    True,
    False,
    // symbols
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Assign,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    AndAnd,
    OrOr,
    Bang,
    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Comma,
    Colon,
    Dot,
    RangeExcl, // ..<
    RangeIncl, // ...
}

/// String literals carry literal runs and raw `\(...)` interpolation
/// sources (sub-parsed at parse time).
#[derive(Debug, Clone, PartialEq, Eq)]
enum StrTok {
    Lit(String),
    Interp(String),
}

fn lex(src: &str) -> Result<Vec<Tok>, RaeError> {
    let b: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < b.len() {
        let c = b[i];
        match c {
            ' ' | '\t' | '\r' | '\n' => i += 1,
            '/' if i + 1 < b.len() && b[i + 1] == '/' => {
                while i < b.len() && b[i] != '\n' {
                    i += 1;
                }
            }
            '0'..='9' => {
                let start = i;
                while i < b.len() && b[i].is_ascii_digit() {
                    i += 1;
                }
                // Fraction only when '.' is followed by a digit, so `0..<9`
                // stays Int(0) RangeExcl Int(9).
                let mut is_float = false;
                if i + 1 < b.len() && b[i] == '.' && b[i + 1].is_ascii_digit() {
                    is_float = true;
                    i += 1;
                    while i < b.len() && b[i].is_ascii_digit() {
                        i += 1;
                    }
                }
                let text: String = b[start..i].iter().collect();
                if is_float {
                    let f = text
                        .parse::<f64>()
                        .map_err(|_| RaeError::Lex(format!("bad float '{}'", text)))?;
                    out.push(Tok::Float(f));
                } else {
                    let n = text
                        .parse::<i64>()
                        .map_err(|_| RaeError::Lex(format!("bad integer '{}'", text)))?;
                    out.push(Tok::Int(n));
                }
            }
            '"' => {
                i += 1;
                let mut parts: Vec<StrTok> = Vec::new();
                let mut lit = String::new();
                loop {
                    if i >= b.len() {
                        return Err(RaeError::Lex("unterminated string".to_string()));
                    }
                    match b[i] {
                        '"' => {
                            i += 1;
                            break;
                        }
                        '\\' if i + 1 < b.len() && b[i + 1] == '(' => {
                            // Swift interpolation: \( expr )
                            if !lit.is_empty() {
                                parts.push(StrTok::Lit(core::mem::take(&mut lit)));
                            }
                            i += 2;
                            let start = i;
                            let mut depth = 1i32;
                            while i < b.len() && depth > 0 {
                                match b[i] {
                                    '(' => depth += 1,
                                    ')' => depth -= 1,
                                    _ => {}
                                }
                                i += 1;
                            }
                            if depth != 0 {
                                return Err(RaeError::Lex(
                                    "unterminated \\( interpolation".to_string(),
                                ));
                            }
                            let expr_src: String = b[start..i - 1].iter().collect();
                            parts.push(StrTok::Interp(expr_src));
                        }
                        '\\' if i + 1 < b.len() => {
                            let esc = b[i + 1];
                            lit.push(match esc {
                                'n' => '\n',
                                't' => '\t',
                                '\\' => '\\',
                                '"' => '"',
                                other => other,
                            });
                            i += 2;
                        }
                        ch => {
                            lit.push(ch);
                            i += 1;
                        }
                    }
                }
                if !lit.is_empty() || parts.is_empty() {
                    parts.push(StrTok::Lit(lit));
                }
                out.push(Tok::Str(parts));
            }
            'a'..='z' | 'A'..='Z' | '_' => {
                let start = i;
                while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == '_') {
                    i += 1;
                }
                let word: String = b[start..i].iter().collect();
                out.push(match word.as_str() {
                    "let" => Tok::Let,
                    "var" => Tok::Var,
                    "if" => Tok::If,
                    "else" => Tok::Else,
                    "while" => Tok::While,
                    "for" => Tok::For,
                    "in" => Tok::In,
                    "break" => Tok::Break,
                    "continue" => Tok::Continue,
                    "func" => Tok::Func,
                    "return" => Tok::Return,
                    "struct" => Tok::Struct,
                    "true" => Tok::True,
                    "false" => Tok::False,
                    _ => Tok::Ident(word),
                });
            }
            '+' => {
                out.push(Tok::Plus);
                i += 1;
            }
            '-' => {
                out.push(Tok::Minus);
                i += 1;
            }
            '*' => {
                out.push(Tok::Star);
                i += 1;
            }
            '/' => {
                out.push(Tok::Slash);
                i += 1;
            }
            '%' => {
                out.push(Tok::Percent);
                i += 1;
            }
            '=' => {
                if i + 1 < b.len() && b[i + 1] == '=' {
                    out.push(Tok::Eq);
                    i += 2;
                } else {
                    out.push(Tok::Assign);
                    i += 1;
                }
            }
            '!' => {
                if i + 1 < b.len() && b[i + 1] == '=' {
                    out.push(Tok::Ne);
                    i += 2;
                } else {
                    out.push(Tok::Bang);
                    i += 1;
                }
            }
            '<' => {
                if i + 1 < b.len() && b[i + 1] == '=' {
                    out.push(Tok::Le);
                    i += 2;
                } else {
                    out.push(Tok::Lt);
                    i += 1;
                }
            }
            '>' => {
                if i + 1 < b.len() && b[i + 1] == '=' {
                    out.push(Tok::Ge);
                    i += 2;
                } else {
                    out.push(Tok::Gt);
                    i += 1;
                }
            }
            '&' => {
                if i + 1 < b.len() && b[i + 1] == '&' {
                    out.push(Tok::AndAnd);
                    i += 2;
                } else {
                    return Err(RaeError::Lex("single '&'".to_string()));
                }
            }
            '|' => {
                if i + 1 < b.len() && b[i + 1] == '|' {
                    out.push(Tok::OrOr);
                    i += 2;
                } else {
                    return Err(RaeError::Lex("single '|'".to_string()));
                }
            }
            '(' => {
                out.push(Tok::LParen);
                i += 1;
            }
            ')' => {
                out.push(Tok::RParen);
                i += 1;
            }
            '{' => {
                out.push(Tok::LBrace);
                i += 1;
            }
            '}' => {
                out.push(Tok::RBrace);
                i += 1;
            }
            '[' => {
                out.push(Tok::LBracket);
                i += 1;
            }
            ']' => {
                out.push(Tok::RBracket);
                i += 1;
            }
            ',' => {
                out.push(Tok::Comma);
                i += 1;
            }
            ':' => {
                out.push(Tok::Colon);
                i += 1;
            }
            '.' => {
                if i + 1 < b.len() && b[i + 1] == '.' {
                    if i + 2 < b.len() && b[i + 2] == '<' {
                        out.push(Tok::RangeExcl);
                        i += 3;
                    } else if i + 2 < b.len() && b[i + 2] == '.' {
                        out.push(Tok::RangeIncl);
                        i += 3;
                    } else {
                        return Err(RaeError::Lex(
                            "'..' is not an operator (use ..< or ...)".to_string(),
                        ));
                    }
                } else {
                    out.push(Tok::Dot);
                    i += 1;
                }
            }
            other => return Err(RaeError::Lex(format!("unexpected character '{}'", other))),
        }
    }
    Ok(out)
}

// ───────────────────────────── AST ─────────────────────────────────────

#[derive(Debug, Clone)]
enum Expr {
    Int(i64),
    Float(f64),
    Bool(bool),
    Str(Vec<StrPart>),
    Var(String),
    ArrayLit(Vec<Expr>),
    DictLit(Vec<(Expr, Expr)>),
    Range(Box<Expr>, Box<Expr>, bool /* inclusive */),
    Unary(UnOp, Box<Expr>),
    Bin(BinOp, Box<Expr>, Box<Expr>),
    Call(Box<Expr>, Vec<Expr>),
    Index(Box<Expr>, Box<Expr>),
    Field(Box<Expr>, String),
    Method(Box<Expr>, String, Vec<Expr>),
    Closure(Vec<String>, Vec<Stmt>),
}

#[derive(Debug, Clone)]
enum StrPart {
    Lit(String),
    Interp(Box<Expr>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UnOp {
    Neg,
    Not,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
}

/// Left side of an assignment: a base variable plus an optional path of
/// index/field steps (`a[0].x = v`). The base binding must be `var`.
#[derive(Debug, Clone)]
struct AssignTarget {
    base: String,
    path: Vec<PathSeg>,
}

#[derive(Debug, Clone)]
enum PathSeg {
    Index(Expr),
    Field(String),
}

#[derive(Debug, Clone)]
enum Stmt {
    Bind {
        name: String,
        mutable: bool,
        value: Expr,
    },
    Assign(AssignTarget, Expr),
    Expr(Expr),
    If(Expr, Vec<Stmt>, Vec<Stmt>),
    While(Expr, Vec<Stmt>),
    For(String, Expr, Vec<Stmt>),
    Func(String, Vec<String>, Vec<Stmt>),
    StructDecl(String, Vec<String>),
    Return(Option<Expr>),
    Break,
    Continue,
}

// ───────────────────────────── Parser ──────────────────────────────────

struct Parser {
    toks: Vec<Tok>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }
    fn peek_at(&self, off: usize) -> Option<&Tok> {
        self.toks.get(self.pos + off)
    }
    fn next(&mut self) -> Option<Tok> {
        let t = self.toks.get(self.pos).cloned();
        if t.is_some() {
            self.pos += 1;
        }
        t
    }
    fn expect(&mut self, want: &Tok, ctx: &str) -> Result<(), RaeError> {
        match self.next() {
            Some(ref t) if t == want => Ok(()),
            other => Err(RaeError::Parse(format!(
                "expected {:?} {} (got {:?})",
                want, ctx, other
            ))),
        }
    }

    fn block(&mut self) -> Result<Vec<Stmt>, RaeError> {
        self.expect(&Tok::LBrace, "to open block")?;
        let body = self.stmts_until_rbrace()?;
        self.expect(&Tok::RBrace, "to close block")?;
        Ok(body)
    }

    fn stmts_until_rbrace(&mut self) -> Result<Vec<Stmt>, RaeError> {
        let mut body = Vec::new();
        while !matches!(self.peek(), Some(Tok::RBrace)) {
            if self.peek().is_none() {
                return Err(RaeError::Parse("unterminated block".to_string()));
            }
            body.push(self.statement()?);
        }
        Ok(body)
    }

    fn statement(&mut self) -> Result<Stmt, RaeError> {
        match self.peek() {
            Some(Tok::Let) | Some(Tok::Var) => {
                let mutable = matches!(self.next(), Some(Tok::Var));
                let name = match self.next() {
                    Some(Tok::Ident(n)) => n,
                    other => {
                        return Err(RaeError::Parse(format!(
                            "expected name after let/var (got {:?})",
                            other
                        )))
                    }
                };
                self.expect(&Tok::Assign, "after binding name")?;
                let value = self.expression()?;
                Ok(Stmt::Bind {
                    name,
                    mutable,
                    value,
                })
            }
            Some(Tok::If) => {
                self.next();
                let cond = self.expression()?;
                let then = self.block()?;
                let alt = if matches!(self.peek(), Some(Tok::Else)) {
                    self.next();
                    if matches!(self.peek(), Some(Tok::If)) {
                        alloc::vec![self.statement()?]
                    } else {
                        self.block()?
                    }
                } else {
                    Vec::new()
                };
                Ok(Stmt::If(cond, then, alt))
            }
            Some(Tok::While) => {
                self.next();
                let cond = self.expression()?;
                let body = self.block()?;
                Ok(Stmt::While(cond, body))
            }
            Some(Tok::For) => {
                self.next();
                let name = match self.next() {
                    Some(Tok::Ident(n)) => n,
                    other => {
                        return Err(RaeError::Parse(format!(
                            "expected loop variable after 'for' (got {:?})",
                            other
                        )))
                    }
                };
                self.expect(&Tok::In, "after for-loop variable")?;
                let iterable = self.expression()?;
                let body = self.block()?;
                Ok(Stmt::For(name, iterable, body))
            }
            Some(Tok::Break) => {
                self.next();
                Ok(Stmt::Break)
            }
            Some(Tok::Continue) => {
                self.next();
                Ok(Stmt::Continue)
            }
            Some(Tok::Func) => {
                self.next();
                let name = match self.next() {
                    Some(Tok::Ident(n)) => n,
                    other => {
                        return Err(RaeError::Parse(format!(
                            "expected function name (got {:?})",
                            other
                        )))
                    }
                };
                self.expect(&Tok::LParen, "after function name")?;
                let mut params = Vec::new();
                while !matches!(self.peek(), Some(Tok::RParen)) {
                    match self.next() {
                        Some(Tok::Ident(p)) => params.push(p),
                        other => {
                            return Err(RaeError::Parse(format!(
                                "expected parameter name (got {:?})",
                                other
                            )))
                        }
                    }
                    if matches!(self.peek(), Some(Tok::Comma)) {
                        self.next();
                    }
                }
                self.expect(&Tok::RParen, "after parameters")?;
                let body = self.block()?;
                Ok(Stmt::Func(name, params, body))
            }
            Some(Tok::Struct) => {
                self.next();
                let name = match self.next() {
                    Some(Tok::Ident(n)) => n,
                    other => {
                        return Err(RaeError::Parse(format!(
                            "expected struct name (got {:?})",
                            other
                        )))
                    }
                };
                self.expect(&Tok::LBrace, "to open struct body")?;
                let mut fields = Vec::new();
                while !matches!(self.peek(), Some(Tok::RBrace)) {
                    match self.next() {
                        Some(Tok::Ident(f)) => fields.push(f),
                        other => {
                            return Err(RaeError::Parse(format!(
                                "expected field name in struct (got {:?})",
                                other
                            )))
                        }
                    }
                    if matches!(self.peek(), Some(Tok::Comma)) {
                        self.next();
                    }
                }
                self.expect(&Tok::RBrace, "to close struct body")?;
                if fields.is_empty() {
                    return Err(RaeError::Parse(format!("struct {} has no fields", name)));
                }
                Ok(Stmt::StructDecl(name, fields))
            }
            Some(Tok::Return) => {
                self.next();
                // `return` at end of block / before '}' carries no value.
                let value = if matches!(self.peek(), Some(Tok::RBrace)) | self.peek().is_none() {
                    None
                } else {
                    Some(self.expression()?)
                };
                Ok(Stmt::Return(value))
            }
            Some(Tok::Ident(_)) => {
                // Assignment (`name = e`, `name[i] = e`, `name.f = e`, or any
                // chain of those) or an expression statement. Try the
                // assignment-path scan; on any mismatch rewind and re-parse
                // as an expression.
                let start = self.pos;
                if let Some(stmt) = self.try_assignment()? {
                    return Ok(stmt);
                }
                self.pos = start;
                Ok(Stmt::Expr(self.expression()?))
            }
            _ => Ok(Stmt::Expr(self.expression()?)),
        }
    }

    /// Attempt `ident (.field | [expr])* = expr`. Returns Ok(None) when the
    /// tokens don't form an assignment (caller rewinds).
    fn try_assignment(&mut self) -> Result<Option<Stmt>, RaeError> {
        let base = match self.next() {
            Some(Tok::Ident(n)) => n,
            _ => return Ok(None),
        };
        let mut path = Vec::new();
        loop {
            match self.peek() {
                Some(Tok::Dot) => {
                    // `.name(` is a method call, not an assignable path.
                    let field = match self.peek_at(1) {
                        Some(Tok::Ident(f)) => f.clone(),
                        _ => return Ok(None),
                    };
                    if matches!(self.peek_at(2), Some(Tok::LParen)) {
                        return Ok(None);
                    }
                    self.next();
                    self.next();
                    path.push(PathSeg::Field(field));
                }
                Some(Tok::LBracket) => {
                    self.next();
                    let idx = self.expression()?;
                    self.expect(&Tok::RBracket, "to close index")?;
                    path.push(PathSeg::Index(idx));
                }
                _ => break,
            }
        }
        if !matches!(self.peek(), Some(Tok::Assign)) {
            return Ok(None);
        }
        self.next();
        let value = self.expression()?;
        Ok(Some(Stmt::Assign(AssignTarget { base, path }, value)))
    }

    fn expression(&mut self) -> Result<Expr, RaeError> {
        self.or_expr()
    }

    fn or_expr(&mut self) -> Result<Expr, RaeError> {
        let mut left = self.and_expr()?;
        while matches!(self.peek(), Some(Tok::OrOr)) {
            self.next();
            let right = self.and_expr()?;
            left = Expr::Bin(BinOp::Or, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn and_expr(&mut self) -> Result<Expr, RaeError> {
        let mut left = self.equality()?;
        while matches!(self.peek(), Some(Tok::AndAnd)) {
            self.next();
            let right = self.equality()?;
            left = Expr::Bin(BinOp::And, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn equality(&mut self) -> Result<Expr, RaeError> {
        let mut left = self.comparison()?;
        loop {
            let op = match self.peek() {
                Some(Tok::Eq) => BinOp::Eq,
                Some(Tok::Ne) => BinOp::Ne,
                _ => break,
            };
            self.next();
            let right = self.comparison()?;
            left = Expr::Bin(op, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn comparison(&mut self) -> Result<Expr, RaeError> {
        let mut left = self.range_expr()?;
        loop {
            let op = match self.peek() {
                Some(Tok::Lt) => BinOp::Lt,
                Some(Tok::Le) => BinOp::Le,
                Some(Tok::Gt) => BinOp::Gt,
                Some(Tok::Ge) => BinOp::Ge,
                _ => break,
            };
            self.next();
            let right = self.range_expr()?;
            left = Expr::Bin(op, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    /// Swift precedence: range formation sits between comparison and
    /// addition (`0..<n+1` parses the whole arithmetic on each side).
    fn range_expr(&mut self) -> Result<Expr, RaeError> {
        let left = self.additive()?;
        let inclusive = match self.peek() {
            Some(Tok::RangeExcl) => false,
            Some(Tok::RangeIncl) => true,
            _ => return Ok(left),
        };
        self.next();
        let right = self.additive()?;
        Ok(Expr::Range(Box::new(left), Box::new(right), inclusive))
    }

    fn additive(&mut self) -> Result<Expr, RaeError> {
        let mut left = self.multiplicative()?;
        loop {
            let op = match self.peek() {
                Some(Tok::Plus) => BinOp::Add,
                Some(Tok::Minus) => BinOp::Sub,
                _ => break,
            };
            self.next();
            let right = self.multiplicative()?;
            left = Expr::Bin(op, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn multiplicative(&mut self) -> Result<Expr, RaeError> {
        let mut left = self.unary()?;
        loop {
            let op = match self.peek() {
                Some(Tok::Star) => BinOp::Mul,
                Some(Tok::Slash) => BinOp::Div,
                Some(Tok::Percent) => BinOp::Mod,
                _ => break,
            };
            self.next();
            let right = self.unary()?;
            left = Expr::Bin(op, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn unary(&mut self) -> Result<Expr, RaeError> {
        match self.peek() {
            Some(Tok::Minus) => {
                self.next();
                Ok(Expr::Unary(UnOp::Neg, Box::new(self.unary()?)))
            }
            Some(Tok::Bang) => {
                self.next();
                Ok(Expr::Unary(UnOp::Not, Box::new(self.unary()?)))
            }
            _ => self.postfix(),
        }
    }

    /// primary followed by any chain of calls `(…)`, indexes `[…]`,
    /// fields `.name`, and method calls `.name(…)`.
    fn postfix(&mut self) -> Result<Expr, RaeError> {
        let mut e = self.primary()?;
        loop {
            match self.peek() {
                Some(Tok::LParen) => {
                    self.next();
                    let args = self.call_args()?;
                    e = Expr::Call(Box::new(e), args);
                }
                Some(Tok::LBracket) => {
                    self.next();
                    let idx = self.expression()?;
                    self.expect(&Tok::RBracket, "to close index")?;
                    e = Expr::Index(Box::new(e), Box::new(idx));
                }
                Some(Tok::Dot) => {
                    self.next();
                    let name = match self.next() {
                        Some(Tok::Ident(n)) => n,
                        other => {
                            return Err(RaeError::Parse(format!(
                                "expected member name after '.' (got {:?})",
                                other
                            )))
                        }
                    };
                    if matches!(self.peek(), Some(Tok::LParen)) {
                        self.next();
                        let args = self.call_args()?;
                        e = Expr::Method(Box::new(e), name, args);
                    } else {
                        e = Expr::Field(Box::new(e), name);
                    }
                }
                _ => break,
            }
        }
        Ok(e)
    }

    fn call_args(&mut self) -> Result<Vec<Expr>, RaeError> {
        let mut args = Vec::new();
        while !matches!(self.peek(), Some(Tok::RParen)) {
            args.push(self.expression()?);
            if matches!(self.peek(), Some(Tok::Comma)) {
                self.next();
            }
        }
        self.expect(&Tok::RParen, "after call arguments")?;
        Ok(args)
    }

    fn primary(&mut self) -> Result<Expr, RaeError> {
        match self.next() {
            Some(Tok::Int(n)) => Ok(Expr::Int(n)),
            Some(Tok::Float(f)) => Ok(Expr::Float(f)),
            Some(Tok::True) => Ok(Expr::Bool(true)),
            Some(Tok::False) => Ok(Expr::Bool(false)),
            Some(Tok::Str(parts)) => {
                let mut out = Vec::new();
                for p in parts {
                    match p {
                        StrTok::Lit(s) => out.push(StrPart::Lit(s)),
                        StrTok::Interp(src) => {
                            let toks = lex(&src)?;
                            let mut sub = Parser { toks, pos: 0 };
                            let e = sub.expression()?;
                            if sub.pos != sub.toks.len() {
                                return Err(RaeError::Parse(
                                    "trailing tokens in interpolation".to_string(),
                                ));
                            }
                            out.push(StrPart::Interp(Box::new(e)));
                        }
                    }
                }
                Ok(Expr::Str(out))
            }
            Some(Tok::Ident(name)) => Ok(Expr::Var(name)),
            Some(Tok::LParen) => {
                let e = self.expression()?;
                self.expect(&Tok::RParen, "after parenthesized expression")?;
                Ok(e)
            }
            Some(Tok::LBracket) => self.collection_literal(),
            Some(Tok::LBrace) => self.closure_literal(),
            other => Err(RaeError::Parse(format!(
                "unexpected token {:?} in expression",
                other
            ))),
        }
    }

    /// After `[`: `[]` empty array, `[:]` empty dictionary, `[a, b]` array,
    /// `["k": v]` dictionary.
    fn collection_literal(&mut self) -> Result<Expr, RaeError> {
        if matches!(self.peek(), Some(Tok::RBracket)) {
            self.next();
            return Ok(Expr::ArrayLit(Vec::new()));
        }
        if matches!(self.peek(), Some(Tok::Colon)) {
            self.next();
            self.expect(&Tok::RBracket, "after [: (empty dictionary)")?;
            return Ok(Expr::DictLit(Vec::new()));
        }
        let first = self.expression()?;
        if matches!(self.peek(), Some(Tok::Colon)) {
            // Dictionary literal.
            self.next();
            let first_val = self.expression()?;
            let mut pairs = alloc::vec![(first, first_val)];
            while matches!(self.peek(), Some(Tok::Comma)) {
                self.next();
                if matches!(self.peek(), Some(Tok::RBracket)) {
                    break;
                }
                let k = self.expression()?;
                self.expect(&Tok::Colon, "between dictionary key and value")?;
                let v = self.expression()?;
                pairs.push((k, v));
            }
            self.expect(&Tok::RBracket, "to close dictionary literal")?;
            return Ok(Expr::DictLit(pairs));
        }
        // Array literal.
        let mut items = alloc::vec![first];
        while matches!(self.peek(), Some(Tok::Comma)) {
            self.next();
            if matches!(self.peek(), Some(Tok::RBracket)) {
                break;
            }
            items.push(self.expression()?);
        }
        self.expect(&Tok::RBracket, "to close array literal")?;
        Ok(Expr::ArrayLit(items))
    }

    /// After `{` in expression position: `{ a, b in … }` (params) or
    /// `{ … }` (zero params). Param form requires the `in` keyword —
    /// exactly Swift's closure syntax.
    fn closure_literal(&mut self) -> Result<Expr, RaeError> {
        let scan_start = self.pos;
        let mut params = Vec::new();
        // Try `ident (, ident)* in`.
        loop {
            match self.peek() {
                Some(Tok::Ident(p)) => {
                    params.push(p.clone());
                    self.next();
                    match self.peek() {
                        Some(Tok::Comma) => {
                            self.next();
                        }
                        Some(Tok::In) => {
                            self.next();
                            break;
                        }
                        _ => {
                            // Not a parameter list after all.
                            self.pos = scan_start;
                            params.clear();
                            break;
                        }
                    }
                }
                Some(Tok::In) if params.is_empty() => {
                    // `{ in … }` — explicit zero params.
                    self.next();
                    break;
                }
                _ => {
                    self.pos = scan_start;
                    params.clear();
                    break;
                }
            }
        }
        let body = self.stmts_until_rbrace()?;
        self.expect(&Tok::RBrace, "to close closure")?;
        Ok(Expr::Closure(params, body))
    }
}

fn parse(src: &str) -> Result<Vec<Stmt>, RaeError> {
    let toks = lex(src)?;
    let mut p = Parser { toks, pos: 0 };
    let mut prog = Vec::new();
    while p.peek().is_some() {
        prog.push(p.statement()?);
    }
    Ok(prog)
}

// ─────────────────────────── Interpreter ───────────────────────────────

/// Result of running a script to completion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    /// Value of a top-level `return`, else 0.
    pub exit_code: i64,
    /// Everything `print(...)` produced, newline-separated.
    pub output: String,
    /// Interpreter steps consumed (each statement/loop-iteration is one).
    pub steps: u64,
}

const MAX_CALL_DEPTH: usize = 64;

/// A pre-evaluated assignment-path step (index expressions are evaluated
/// BEFORE the destination is mutably borrowed).
enum PathKey {
    Index(Value),
    Field(String),
}

struct Interp<'h> {
    funcs: BTreeMap<String, (Vec<String>, Vec<Stmt>)>,
    structs: BTreeMap<String, Vec<String>>,
    scopes: Vec<BTreeMap<String, (Value, bool)>>,
    output: String,
    fuel: u64,
    steps: u64,
    depth: usize,
    host: &'h mut dyn Host,
}

enum Flow {
    Normal,
    Return(Value),
    Break,
    Continue,
}

/// f64 truncation without `std` (`f64::trunc` is std-only on bare metal).
/// Valid for |f| < 2^63, far beyond script arithmetic needs.
fn trunc_f64(f: f64) -> i64 {
    f as i64
}

fn as_f64(v: &Value) -> Option<f64> {
    match v {
        Value::Int(n) => Some(*n as f64),
        Value::Float(f) => Some(*f),
        _ => None,
    }
}

impl<'h> Interp<'h> {
    fn burn(&mut self) -> Result<(), RaeError> {
        if self.fuel == 0 {
            return Err(RaeError::OutOfFuel);
        }
        self.fuel -= 1;
        self.steps += 1;
        Ok(())
    }

    fn lookup(&self, name: &str) -> Option<&(Value, bool)> {
        self.scopes.iter().rev().find_map(|s| s.get(name))
    }

    /// Snapshot every visible binding (outer→inner so inner shadows) for
    /// closure capture-by-value.
    fn capture_env(&self) -> BTreeMap<String, (Value, bool)> {
        let mut cap = BTreeMap::new();
        for scope in &self.scopes {
            for (k, v) in scope {
                cap.insert(k.clone(), v.clone());
            }
        }
        cap
    }

    fn eval(&mut self, e: &Expr) -> Result<Value, RaeError> {
        Ok(match e {
            Expr::Int(n) => Value::Int(*n),
            Expr::Float(f) => Value::Float(*f),
            Expr::Bool(b) => Value::Bool(*b),
            Expr::Str(parts) => {
                let mut s = String::new();
                for p in parts {
                    match p {
                        StrPart::Lit(l) => s.push_str(l),
                        StrPart::Interp(e) => {
                            let v = self.eval(e)?;
                            s.push_str(&v.display());
                        }
                    }
                }
                Value::Str(s)
            }
            Expr::Var(name) => {
                if let Some((v, _)) = self.lookup(name) {
                    v.clone()
                } else if let Some((params, body)) = self.funcs.get(name) {
                    // Named functions are first-class: `arr.map(inc)`.
                    Value::Func(Rc::new(FuncVal {
                        params: params.clone(),
                        body: body.clone(),
                        captured: BTreeMap::new(),
                    }))
                } else {
                    return Err(RaeError::UndefinedVariable(name.clone()));
                }
            }
            Expr::ArrayLit(items) => {
                let mut out = Vec::with_capacity(items.len());
                for it in items {
                    out.push(self.eval(it)?);
                }
                Value::Array(out)
            }
            Expr::DictLit(pairs) => {
                let mut map = BTreeMap::new();
                for (k, v) in pairs {
                    let key = match self.eval(k)? {
                        Value::Str(s) => s,
                        other => {
                            return Err(RaeError::TypeMismatch(format!(
                                "dictionary key must be String, got {}",
                                other.type_name()
                            )))
                        }
                    };
                    map.insert(key, self.eval(v)?);
                }
                Value::Dict(map)
            }
            Expr::Range(l, r, inclusive) => {
                let (lv, rv) = (self.eval(l)?, self.eval(r)?);
                match (lv, rv) {
                    (Value::Int(a), Value::Int(b)) => Value::Range {
                        start: a,
                        end: b,
                        inclusive: *inclusive,
                    },
                    (a, b) => {
                        return Err(RaeError::TypeMismatch(format!(
                            "range bounds must be Int, got {} and {}",
                            a.type_name(),
                            b.type_name()
                        )))
                    }
                }
            }
            Expr::Closure(params, body) => Value::Func(Rc::new(FuncVal {
                params: params.clone(),
                body: body.clone(),
                captured: self.capture_env(),
            })),
            Expr::Unary(op, inner) => {
                let v = self.eval(inner)?;
                match (op, v) {
                    (UnOp::Neg, Value::Int(n)) => Value::Int(-n),
                    (UnOp::Neg, Value::Float(f)) => Value::Float(-f),
                    (UnOp::Not, Value::Bool(b)) => Value::Bool(!b),
                    (_, v) => {
                        return Err(RaeError::TypeMismatch(format!(
                            "unary op on {}",
                            v.type_name()
                        )))
                    }
                }
            }
            Expr::Bin(op, l, r) => {
                // Short-circuit logical ops.
                if *op == BinOp::And || *op == BinOp::Or {
                    let lv = match self.eval(l)? {
                        Value::Bool(b) => b,
                        v => {
                            return Err(RaeError::TypeMismatch(format!(
                                "logical op on {}",
                                v.type_name()
                            )))
                        }
                    };
                    let needs_rhs = (*op == BinOp::And && lv) || (*op == BinOp::Or && !lv);
                    if !needs_rhs {
                        return Ok(Value::Bool(lv));
                    }
                    return match self.eval(r)? {
                        Value::Bool(b) => Ok(Value::Bool(b)),
                        v => Err(RaeError::TypeMismatch(format!(
                            "logical op on {}",
                            v.type_name()
                        ))),
                    };
                }
                let lv = self.eval(l)?;
                let rv = self.eval(r)?;
                self.eval_binop(*op, lv, rv)?
            }
            Expr::Index(recv, idx) => {
                let r = self.eval(recv)?;
                let i = self.eval(idx)?;
                self.index_value(&r, &i)?
            }
            Expr::Field(recv, name) => {
                let r = self.eval(recv)?;
                self.field_value(&r, name)?
            }
            Expr::Method(recv, name, args) => self.eval_method(recv, name, args)?,
            Expr::Call(callee, args) => self.eval_call(callee, args)?,
        })
    }

    fn eval_binop(&mut self, op: BinOp, lv: Value, rv: Value) -> Result<Value, RaeError> {
        Ok(match (op, lv, rv) {
            (BinOp::Add, Value::Int(a), Value::Int(b)) => Value::Int(a.wrapping_add(b)),
            (BinOp::Add, Value::Str(a), b) => Value::Str(a + &b.display()),
            (BinOp::Add, a, Value::Str(b)) => Value::Str(a.display() + &b),
            (BinOp::Add, Value::Array(mut a), Value::Array(b)) => {
                a.extend(b);
                Value::Array(a)
            }
            (BinOp::Sub, Value::Int(a), Value::Int(b)) => Value::Int(a.wrapping_sub(b)),
            (BinOp::Mul, Value::Int(a), Value::Int(b)) => Value::Int(a.wrapping_mul(b)),
            (BinOp::Div, Value::Int(_), Value::Int(0)) => return Err(RaeError::DivisionByZero),
            (BinOp::Div, Value::Int(a), Value::Int(b)) => Value::Int(a.wrapping_div(b)),
            (BinOp::Mod, Value::Int(_), Value::Int(0)) => return Err(RaeError::DivisionByZero),
            (BinOp::Mod, Value::Int(a), Value::Int(b)) => Value::Int(a.wrapping_rem(b)),
            (BinOp::Eq, a, b) => Value::Bool(a == b),
            (BinOp::Ne, a, b) => Value::Bool(a != b),
            (BinOp::Lt, Value::Int(a), Value::Int(b)) => Value::Bool(a < b),
            (BinOp::Le, Value::Int(a), Value::Int(b)) => Value::Bool(a <= b),
            (BinOp::Gt, Value::Int(a), Value::Int(b)) => Value::Bool(a > b),
            (BinOp::Ge, Value::Int(a), Value::Int(b)) => Value::Bool(a >= b),
            (BinOp::Lt, Value::Str(a), Value::Str(b)) => Value::Bool(a < b),
            (BinOp::Le, Value::Str(a), Value::Str(b)) => Value::Bool(a <= b),
            (BinOp::Gt, Value::Str(a), Value::Str(b)) => Value::Bool(a > b),
            (BinOp::Ge, Value::Str(a), Value::Str(b)) => Value::Bool(a >= b),
            // Mixed / float numerics promote to Float (Swift would demand an
            // explicit conversion; a scripting language shouldn't).
            (op, a, b) => {
                let (x, y) = match (as_f64(&a), as_f64(&b)) {
                    (Some(x), Some(y)) => (x, y),
                    _ => {
                        return Err(RaeError::TypeMismatch(format!(
                            "{:?} on {} and {}",
                            op,
                            a.type_name(),
                            b.type_name()
                        )))
                    }
                };
                match op {
                    BinOp::Add => Value::Float(x + y),
                    BinOp::Sub => Value::Float(x - y),
                    BinOp::Mul => Value::Float(x * y),
                    BinOp::Div => Value::Float(x / y),
                    BinOp::Mod => {
                        if y == 0.0 {
                            return Err(RaeError::DivisionByZero);
                        }
                        // fmod without libm: a - b*trunc(a/b).
                        Value::Float(x - y * (trunc_f64(x / y) as f64))
                    }
                    BinOp::Lt => Value::Bool(x < y),
                    BinOp::Le => Value::Bool(x <= y),
                    BinOp::Gt => Value::Bool(x > y),
                    BinOp::Ge => Value::Bool(x >= y),
                    _ => unreachable!("logical/equality handled above"),
                }
            }
        })
    }

    fn index_value(&self, recv: &Value, idx: &Value) -> Result<Value, RaeError> {
        match (recv, idx) {
            (Value::Array(items), Value::Int(i)) => {
                let i = *i;
                if i < 0 || i as usize >= items.len() {
                    return Err(RaeError::IndexOutOfBounds(format!(
                        "index {} on array of {}",
                        i,
                        items.len()
                    )));
                }
                Ok(items[i as usize].clone())
            }
            (Value::Dict(map), Value::Str(k)) => map.get(k).cloned().ok_or_else(|| {
                RaeError::IndexOutOfBounds(format!("no key \"{}\" in dictionary", k))
            }),
            (Value::Str(s), Value::Int(i)) => {
                let i = *i;
                let ch = if i >= 0 {
                    s.chars().nth(i as usize)
                } else {
                    None
                };
                match ch {
                    Some(c) => Ok(Value::Str(c.to_string())),
                    None => Err(RaeError::IndexOutOfBounds(format!(
                        "index {} on string of {} characters",
                        i,
                        s.chars().count()
                    ))),
                }
            }
            (r, i) => Err(RaeError::TypeMismatch(format!(
                "cannot index {} with {}",
                r.type_name(),
                i.type_name()
            ))),
        }
    }

    /// Read-only properties: `.count`, `.isEmpty`, `.first`, `.last`,
    /// `.keys`, `.values` — plus struct field access.
    fn field_value(&self, recv: &Value, name: &str) -> Result<Value, RaeError> {
        match (recv, name) {
            (Value::Struct { name: sn, fields }, f) => fields
                .get(f)
                .cloned()
                .ok_or_else(|| RaeError::NoSuchField(format!("{} has no field '{}'", sn, f))),
            (Value::Str(s), "count") => Ok(Value::Int(s.chars().count() as i64)),
            (Value::Str(s), "isEmpty") => Ok(Value::Bool(s.is_empty())),
            (Value::Array(a), "count") => Ok(Value::Int(a.len() as i64)),
            (Value::Array(a), "isEmpty") => Ok(Value::Bool(a.is_empty())),
            (Value::Array(a), "first") => Ok(a.first().cloned().unwrap_or(Value::Unit)),
            (Value::Array(a), "last") => Ok(a.last().cloned().unwrap_or(Value::Unit)),
            (Value::Dict(m), "count") => Ok(Value::Int(m.len() as i64)),
            (Value::Dict(m), "isEmpty") => Ok(Value::Bool(m.is_empty())),
            (Value::Dict(m), "keys") => Ok(Value::Array(
                m.keys().map(|k| Value::Str(k.clone())).collect(),
            )),
            (Value::Dict(m), "values") => Ok(Value::Array(m.values().cloned().collect())),
            (
                Value::Range {
                    start,
                    end,
                    inclusive,
                },
                "count",
            ) => {
                let n = end - start + if *inclusive { 1 } else { 0 };
                Ok(Value::Int(if n > 0 { n } else { 0 }))
            }
            (r, f) => Err(RaeError::NoSuchField(format!(
                "{} has no property '{}'",
                r.type_name(),
                f
            ))),
        }
    }

    // ── Calls ───────────────────────────────────────────────────────────

    fn eval_call(&mut self, callee: &Expr, args: &[Expr]) -> Result<Value, RaeError> {
        // Name-based resolution first: script funcs → structs → builtins →
        // host bindings. A local variable holding a Function shadows all.
        if let Expr::Var(name) = callee {
            if name == "print" {
                let mut line = String::new();
                for (i, a) in args.iter().enumerate() {
                    if i > 0 {
                        line.push(' ');
                    }
                    line.push_str(&self.eval(a)?.display());
                }
                self.output.push_str(&line);
                self.output.push('\n');
                return Ok(Value::Unit);
            }
            if let Some((v, _)) = self.lookup(name) {
                let v = v.clone();
                return match v {
                    Value::Func(f) => {
                        let argv = self.eval_args(args)?;
                        self.call_func(&f, argv)
                    }
                    other => Err(RaeError::TypeMismatch(format!(
                        "'{}' is a {} — not callable",
                        name,
                        other.type_name()
                    ))),
                };
            }
            if let Some((params, body)) = self.funcs.get(name).cloned() {
                let argv = self.eval_args(args)?;
                if params.len() != argv.len() {
                    return Err(RaeError::ArityMismatch(format!(
                        "{} takes {} argument(s), got {}",
                        name,
                        params.len(),
                        argv.len()
                    )));
                }
                let f = FuncVal {
                    params,
                    body,
                    captured: BTreeMap::new(),
                };
                return self.call_func(&f, argv);
            }
            if let Some(fields) = self.structs.get(name).cloned() {
                let argv = self.eval_args(args)?;
                if fields.len() != argv.len() {
                    return Err(RaeError::ArityMismatch(format!(
                        "{} takes {} field value(s), got {}",
                        name,
                        fields.len(),
                        argv.len()
                    )));
                }
                let mut map = BTreeMap::new();
                for (f, v) in fields.into_iter().zip(argv) {
                    map.insert(f, v);
                }
                return Ok(Value::Struct {
                    name: name.clone(),
                    fields: map,
                });
            }
            if let Some(v) = self.try_builtin(name, args)? {
                return Ok(v);
            }
            // Host bindings last: the embedder's capability-gated surface.
            let argv = self.eval_args(args)?;
            self.burn()?;
            return match self.host.call(name, &argv) {
                Ok(v) => Ok(v),
                Err(HostError::Unknown) => Err(RaeError::UndefinedFunction(name.clone())),
                Err(HostError::Denied(msg)) => Err(RaeError::CapabilityDenied(msg)),
                Err(HostError::Failed(msg)) => Err(RaeError::HostFailed(msg)),
            };
        }
        // Arbitrary callee expression — must evaluate to a Function.
        let f = match self.eval(callee)? {
            Value::Func(f) => f,
            other => {
                return Err(RaeError::TypeMismatch(format!(
                    "{} is not callable",
                    other.type_name()
                )))
            }
        };
        let argv = self.eval_args(args)?;
        self.call_func(&f, argv)
    }

    fn eval_args(&mut self, args: &[Expr]) -> Result<Vec<Value>, RaeError> {
        let mut out = Vec::with_capacity(args.len());
        for a in args {
            out.push(self.eval(a)?);
        }
        Ok(out)
    }

    fn call_func(&mut self, f: &FuncVal, argv: Vec<Value>) -> Result<Value, RaeError> {
        if f.params.len() != argv.len() {
            return Err(RaeError::ArityMismatch(format!(
                "function takes {} argument(s), got {}",
                f.params.len(),
                argv.len()
            )));
        }
        if self.depth >= MAX_CALL_DEPTH {
            return Err(RaeError::RecursionLimit);
        }
        self.burn()?;
        let mut frame = f.captured.clone();
        for (p, v) in f.params.iter().zip(argv) {
            frame.insert(p.clone(), (v, true));
        }
        self.depth += 1;
        self.scopes.push(frame);
        let flow = self.exec_func_body(&f.body);
        self.scopes.pop();
        self.depth -= 1;
        match flow? {
            Flow::Return(v) => Ok(v),
            Flow::Normal => Ok(Value::Unit),
            Flow::Break | Flow::Continue => Err(RaeError::StrayControlFlow),
        }
    }

    /// A function body returns its trailing expression implicitly (Swift:
    /// `{ x in x * 2 }` needs no `return`).
    fn exec_func_body(&mut self, body: &[Stmt]) -> Result<Flow, RaeError> {
        let Some((last, init)) = body.split_last() else {
            return Ok(Flow::Normal);
        };
        for stmt in init {
            match self.exec(stmt)? {
                Flow::Normal => {}
                other => return Ok(other),
            }
        }
        if let Stmt::Expr(e) = last {
            self.burn()?;
            let v = self.eval(e)?;
            return Ok(Flow::Return(v));
        }
        self.exec(last)
    }

    /// Global builtins. Returns Ok(None) when `name` isn't a builtin.
    fn try_builtin(&mut self, name: &str, args: &[Expr]) -> Result<Option<Value>, RaeError> {
        let arity = |want: usize, got: usize| -> Result<(), RaeError> {
            if want != got {
                Err(RaeError::ArityMismatch(format!(
                    "{} takes {} argument(s), got {}",
                    name, want, got
                )))
            } else {
                Ok(())
            }
        };
        Ok(match name {
            "String" => {
                arity(1, args.len())?;
                let v = self.eval(&args[0])?;
                Some(Value::Str(v.display()))
            }
            "Int" => {
                arity(1, args.len())?;
                let v = self.eval(&args[0])?;
                Some(match v {
                    Value::Int(n) => Value::Int(n),
                    Value::Float(f) => Value::Int(trunc_f64(f)),
                    Value::Bool(b) => Value::Int(if b { 1 } else { 0 }),
                    Value::Str(s) => match s.trim().parse::<i64>() {
                        Ok(n) => Value::Int(n),
                        Err(_) => {
                            return Err(RaeError::TypeMismatch(format!(
                                "Int(\"{}\") is not a number",
                                s
                            )))
                        }
                    },
                    other => {
                        return Err(RaeError::TypeMismatch(format!(
                            "Int() on {}",
                            other.type_name()
                        )))
                    }
                })
            }
            "Float" => {
                arity(1, args.len())?;
                let v = self.eval(&args[0])?;
                Some(match v {
                    Value::Int(n) => Value::Float(n as f64),
                    Value::Float(f) => Value::Float(f),
                    Value::Str(s) => match s.trim().parse::<f64>() {
                        Ok(f) => Value::Float(f),
                        Err(_) => {
                            return Err(RaeError::TypeMismatch(format!(
                                "Float(\"{}\") is not a number",
                                s
                            )))
                        }
                    },
                    other => {
                        return Err(RaeError::TypeMismatch(format!(
                            "Float() on {}",
                            other.type_name()
                        )))
                    }
                })
            }
            "abs" => {
                arity(1, args.len())?;
                let v = self.eval(&args[0])?;
                Some(match v {
                    Value::Int(n) => Value::Int(n.wrapping_abs()),
                    Value::Float(f) => Value::Float(if f < 0.0 { -f } else { f }),
                    other => {
                        return Err(RaeError::TypeMismatch(format!(
                            "abs() on {}",
                            other.type_name()
                        )))
                    }
                })
            }
            "min" | "max" => {
                arity(2, args.len())?;
                let a = self.eval(&args[0])?;
                let b = self.eval(&args[1])?;
                let take_a = match (&a, &b) {
                    (Value::Int(x), Value::Int(y)) => {
                        if name == "min" {
                            x <= y
                        } else {
                            x >= y
                        }
                    }
                    _ => match (as_f64(&a), as_f64(&b)) {
                        (Some(x), Some(y)) => {
                            if name == "min" {
                                x <= y
                            } else {
                                x >= y
                            }
                        }
                        _ => {
                            return Err(RaeError::TypeMismatch(format!(
                                "{}() on {} and {}",
                                name,
                                a.type_name(),
                                b.type_name()
                            )))
                        }
                    },
                };
                Some(if take_a { a } else { b })
            }
            _ => None,
        })
    }

    // ── Methods ─────────────────────────────────────────────────────────

    fn eval_method(&mut self, recv: &Expr, name: &str, args: &[Expr]) -> Result<Value, RaeError> {
        self.burn()?;
        let is_mutating = matches!(name, "append" | "removeLast" | "remove");
        if is_mutating {
            return self.eval_mutating_method(recv, name, args);
        }
        let argv = self.eval_args(args)?;
        let r = self.eval(recv)?;
        match (&r, name) {
            // ── String ──
            (Value::Str(s), "uppercased") => {
                self.no_args(name, &argv)?;
                Ok(Value::Str(s.to_uppercase()))
            }
            (Value::Str(s), "lowercased") => {
                self.no_args(name, &argv)?;
                Ok(Value::Str(s.to_lowercase()))
            }
            (Value::Str(s), "trimmed") => {
                self.no_args(name, &argv)?;
                Ok(Value::Str(s.trim().to_string()))
            }
            (Value::Str(s), "contains") => {
                let needle = self.one_str(name, &argv)?;
                Ok(Value::Bool(s.contains(&needle)))
            }
            (Value::Str(s), "hasPrefix") => {
                let p = self.one_str(name, &argv)?;
                Ok(Value::Bool(s.starts_with(&p)))
            }
            (Value::Str(s), "hasSuffix") => {
                let p = self.one_str(name, &argv)?;
                Ok(Value::Bool(s.ends_with(&p)))
            }
            (Value::Str(s), "split") => {
                let sep = self.one_str(name, &argv)?;
                if sep.is_empty() {
                    return Err(RaeError::TypeMismatch(
                        "split() separator must not be empty".to_string(),
                    ));
                }
                Ok(Value::Array(
                    s.split(sep.as_str())
                        .map(|p| Value::Str(p.to_string()))
                        .collect(),
                ))
            }
            // ── Array ──
            (Value::Array(items), "contains") => {
                if argv.len() != 1 {
                    return Err(RaeError::ArityMismatch(format!(
                        "contains takes 1 argument, got {}",
                        argv.len()
                    )));
                }
                Ok(Value::Bool(items.contains(&argv[0])))
            }
            (Value::Array(items), "reversed") => {
                self.no_args(name, &argv)?;
                let mut r = items.clone();
                r.reverse();
                Ok(Value::Array(r))
            }
            (Value::Array(items), "joined") => {
                let sep = self.one_str(name, &argv)?;
                let parts: Vec<String> = items.iter().map(|v| v.display()).collect();
                Ok(Value::Str(parts.join(&sep)))
            }
            (Value::Array(items), "map") => {
                let f = self.one_func(name, &argv)?;
                let mut out = Vec::with_capacity(items.len());
                for it in items.clone() {
                    self.burn()?;
                    out.push(self.call_func(&f, alloc::vec![it])?);
                }
                Ok(Value::Array(out))
            }
            (Value::Array(items), "filter") => {
                let f = self.one_func(name, &argv)?;
                let mut out = Vec::new();
                for it in items.clone() {
                    self.burn()?;
                    match self.call_func(&f, alloc::vec![it.clone()])? {
                        Value::Bool(true) => out.push(it),
                        Value::Bool(false) => {}
                        other => {
                            return Err(RaeError::TypeMismatch(format!(
                                "filter closure must return Bool, got {}",
                                other.type_name()
                            )))
                        }
                    }
                }
                Ok(Value::Array(out))
            }
            (Value::Array(items), "reduce") => {
                if argv.len() != 2 {
                    return Err(RaeError::ArityMismatch(format!(
                        "reduce takes 2 arguments (initial, closure), got {}",
                        argv.len()
                    )));
                }
                let f = match &argv[1] {
                    Value::Func(f) => f.clone(),
                    other => {
                        return Err(RaeError::TypeMismatch(format!(
                            "reduce's second argument must be a Function, got {}",
                            other.type_name()
                        )))
                    }
                };
                let mut acc = argv[0].clone();
                for it in items.clone() {
                    self.burn()?;
                    acc = self.call_func(&f, alloc::vec![acc, it])?;
                }
                Ok(acc)
            }
            // ── Dictionary ──
            (Value::Dict(map), "hasKey") => {
                let k = self.one_str(name, &argv)?;
                Ok(Value::Bool(map.contains_key(&k)))
            }
            (r, m) => Err(RaeError::NoSuchField(format!(
                "{} has no method '{}'",
                r.type_name(),
                m
            ))),
        }
    }

    fn no_args(&self, name: &str, argv: &[Value]) -> Result<(), RaeError> {
        if argv.is_empty() {
            Ok(())
        } else {
            Err(RaeError::ArityMismatch(format!(
                "{} takes no arguments, got {}",
                name,
                argv.len()
            )))
        }
    }

    fn one_str(&self, name: &str, argv: &[Value]) -> Result<String, RaeError> {
        match argv {
            [Value::Str(s)] => Ok(s.clone()),
            [other] => Err(RaeError::TypeMismatch(format!(
                "{} takes a String argument, got {}",
                name,
                other.type_name()
            ))),
            _ => Err(RaeError::ArityMismatch(format!(
                "{} takes 1 argument, got {}",
                name,
                argv.len()
            ))),
        }
    }

    fn one_func(&self, name: &str, argv: &[Value]) -> Result<Rc<FuncVal>, RaeError> {
        match argv {
            [Value::Func(f)] => Ok(f.clone()),
            [other] => Err(RaeError::TypeMismatch(format!(
                "{} takes a Function argument, got {}",
                name,
                other.type_name()
            ))),
            _ => Err(RaeError::ArityMismatch(format!(
                "{} takes 1 argument, got {}",
                name,
                argv.len()
            ))),
        }
    }

    /// `append`/`removeLast`/`remove` mutate their receiver in place, so the
    /// receiver must be a path rooted at a `var` binding (Swift semantics).
    fn eval_mutating_method(
        &mut self,
        recv: &Expr,
        name: &str,
        args: &[Expr],
    ) -> Result<Value, RaeError> {
        let argv = self.eval_args(args)?;
        let (base, segs) = match expr_as_path(recv) {
            Some(p) => p,
            None => {
                return Err(RaeError::TypeMismatch(format!(
                    "{}() mutates its receiver — call it on a variable",
                    name
                )))
            }
        };
        let keys = self.eval_path_keys(&segs)?;
        let slot = self.resolve_mut(&base)?;
        let target = walk_mut(slot, &keys, false)?;
        match (target, name) {
            (Value::Array(items), "append") => {
                if argv.len() != 1 {
                    return Err(RaeError::ArityMismatch(format!(
                        "append takes 1 argument, got {}",
                        argv.len()
                    )));
                }
                items.push(argv.into_iter().next().expect("len checked"));
                Ok(Value::Unit)
            }
            (Value::Array(items), "removeLast") => {
                if !argv.is_empty() {
                    return Err(RaeError::ArityMismatch(format!(
                        "removeLast takes no arguments, got {}",
                        argv.len()
                    )));
                }
                items.pop().ok_or_else(|| {
                    RaeError::IndexOutOfBounds("removeLast on empty array".to_string())
                })
            }
            (Value::Dict(map), "remove") => match argv.as_slice() {
                [Value::Str(k)] => Ok(map.remove(k).unwrap_or(Value::Unit)),
                [other] => Err(RaeError::TypeMismatch(format!(
                    "remove takes a String key, got {}",
                    other.type_name()
                ))),
                _ => Err(RaeError::ArityMismatch(format!(
                    "remove takes 1 argument, got {}",
                    argv.len()
                ))),
            },
            (t, m) => Err(RaeError::NoSuchField(format!(
                "{} has no method '{}'",
                t.type_name(),
                m
            ))),
        }
    }

    /// Evaluate a syntactic path's index expressions into concrete keys
    /// BEFORE mutably borrowing the destination.
    fn eval_path_keys(&mut self, segs: &[PathSegRef<'_>]) -> Result<Vec<PathKey>, RaeError> {
        let mut keys = Vec::with_capacity(segs.len());
        for seg in segs {
            keys.push(match seg {
                PathSegRef::Index(e) => PathKey::Index(self.eval(e)?),
                PathSegRef::Field(f) => PathKey::Field((*f).to_string()),
            });
        }
        Ok(keys)
    }

    /// Find the mutable slot for `name`, enforcing `var` (Swift: mutating
    /// through a `let` binding is rejected, including via paths).
    fn resolve_mut(&mut self, name: &str) -> Result<&mut Value, RaeError> {
        for scope in self.scopes.iter_mut().rev() {
            if let Some(slot) = scope.get_mut(name) {
                if !slot.1 {
                    return Err(RaeError::AssignToLet(name.to_string()));
                }
                return Ok(&mut slot.0);
            }
        }
        Err(RaeError::UndefinedVariable(name.to_string()))
    }

    // ── Statements ──────────────────────────────────────────────────────

    fn exec_block(&mut self, body: &[Stmt]) -> Result<Flow, RaeError> {
        for stmt in body {
            match self.exec(stmt)? {
                Flow::Normal => {}
                other => return Ok(other),
            }
        }
        Ok(Flow::Normal)
    }

    fn exec(&mut self, stmt: &Stmt) -> Result<Flow, RaeError> {
        self.burn()?;
        match stmt {
            Stmt::Bind {
                name,
                mutable,
                value,
            } => {
                let v = self.eval(value)?;
                self.scopes
                    .last_mut()
                    .expect("scope stack never empty")
                    .insert(name.clone(), (v, *mutable));
                Ok(Flow::Normal)
            }
            Stmt::Assign(target, value) => {
                let v = self.eval(value)?;
                let segs: Vec<PathSegRef<'_>> = target
                    .path
                    .iter()
                    .map(|s| match s {
                        PathSeg::Index(e) => PathSegRef::Index(e),
                        PathSeg::Field(f) => PathSegRef::Field(f),
                    })
                    .collect();
                let keys = self.eval_path_keys(&segs)?;
                let slot = self.resolve_mut(&target.base)?;
                if keys.is_empty() {
                    *slot = v;
                } else {
                    let dst = walk_mut(slot, &keys, true)?;
                    *dst = v;
                }
                Ok(Flow::Normal)
            }
            Stmt::Expr(e) => {
                self.eval(e)?;
                Ok(Flow::Normal)
            }
            Stmt::If(cond, then, alt) => {
                let c = match self.eval(cond)? {
                    Value::Bool(b) => b,
                    v => {
                        return Err(RaeError::TypeMismatch(format!(
                            "if condition is {}",
                            v.type_name()
                        )))
                    }
                };
                self.scopes.push(BTreeMap::new());
                let flow = self.exec_block(if c { then } else { alt });
                self.scopes.pop();
                flow
            }
            Stmt::While(cond, body) => {
                loop {
                    self.burn()?;
                    let c = match self.eval(cond)? {
                        Value::Bool(b) => b,
                        v => {
                            return Err(RaeError::TypeMismatch(format!(
                                "while condition is {}",
                                v.type_name()
                            )))
                        }
                    };
                    if !c {
                        break;
                    }
                    self.scopes.push(BTreeMap::new());
                    let flow = self.exec_block(body);
                    self.scopes.pop();
                    match flow? {
                        Flow::Return(v) => return Ok(Flow::Return(v)),
                        Flow::Break => break,
                        Flow::Continue | Flow::Normal => {}
                    }
                }
                Ok(Flow::Normal)
            }
            Stmt::For(name, iterable, body) => {
                let iter_v = self.eval(iterable)?;
                match iter_v {
                    Value::Range {
                        start,
                        end,
                        inclusive,
                    } => {
                        let last = if inclusive { end } else { end - 1 };
                        let mut i = start;
                        while i <= last {
                            match self.run_for_body(name, Value::Int(i), body)? {
                                Flow::Return(v) => return Ok(Flow::Return(v)),
                                Flow::Break => break,
                                _ => {}
                            }
                            i += 1;
                        }
                    }
                    Value::Array(items) => {
                        for it in items {
                            match self.run_for_body(name, it, body)? {
                                Flow::Return(v) => return Ok(Flow::Return(v)),
                                Flow::Break => break,
                                _ => {}
                            }
                        }
                    }
                    Value::Dict(map) => {
                        // Iterates KEYS (sorted) — index back in for values.
                        for k in map.keys().cloned().collect::<Vec<_>>() {
                            match self.run_for_body(name, Value::Str(k), body)? {
                                Flow::Return(v) => return Ok(Flow::Return(v)),
                                Flow::Break => break,
                                _ => {}
                            }
                        }
                    }
                    Value::Str(s) => {
                        for c in s.chars().collect::<Vec<_>>() {
                            match self.run_for_body(name, Value::Str(c.to_string()), body)? {
                                Flow::Return(v) => return Ok(Flow::Return(v)),
                                Flow::Break => break,
                                _ => {}
                            }
                        }
                    }
                    other => {
                        return Err(RaeError::TypeMismatch(format!(
                            "cannot iterate over {}",
                            other.type_name()
                        )))
                    }
                }
                Ok(Flow::Normal)
            }
            Stmt::Func(name, params, body) => {
                self.funcs
                    .insert(name.clone(), (params.clone(), body.clone()));
                Ok(Flow::Normal)
            }
            Stmt::StructDecl(name, fields) => {
                self.structs.insert(name.clone(), fields.clone());
                Ok(Flow::Normal)
            }
            Stmt::Return(value) => {
                let v = match value {
                    Some(e) => self.eval(e)?,
                    None => Value::Unit,
                };
                Ok(Flow::Return(v))
            }
            Stmt::Break => Ok(Flow::Break),
            Stmt::Continue => Ok(Flow::Continue),
        }
    }

    /// One for-loop iteration: fuel, fresh scope with the loop variable
    /// (immutable, like Swift), body, scope pop.
    fn run_for_body(&mut self, name: &str, v: Value, body: &[Stmt]) -> Result<Flow, RaeError> {
        self.burn()?;
        let mut scope = BTreeMap::new();
        scope.insert(name.to_string(), (v, false));
        self.scopes.push(scope);
        let flow = self.exec_block(body);
        self.scopes.pop();
        flow
    }
}

/// A borrowed path segment (assignment targets and mutating-method
/// receivers share the walker).
enum PathSegRef<'e> {
    Index(&'e Expr),
    Field(&'e str),
}

/// Decompose `a[0].x` into (`a`, [Index(0), Field(x)]). Returns None for
/// receivers that aren't variable-rooted paths (r-values can't be mutated).
fn expr_as_path(e: &Expr) -> Option<(String, Vec<PathSegRef<'_>>)> {
    fn walk<'e>(e: &'e Expr, segs: &mut Vec<PathSegRef<'e>>) -> Option<String> {
        match e {
            Expr::Var(n) => Some(n.clone()),
            Expr::Index(inner, idx) => {
                let base = walk(inner, segs)?;
                segs.push(PathSegRef::Index(idx));
                Some(base)
            }
            Expr::Field(inner, f) => {
                let base = walk(inner, segs)?;
                segs.push(PathSegRef::Field(f));
                Some(base)
            }
            _ => None,
        }
    }
    let mut segs = Vec::new();
    let base = walk(e, &mut segs)?;
    Some((base, segs))
}

/// Walk a value along pre-evaluated keys, returning the target slot.
/// `insert_final` lets a dictionary assignment create the key (Swift's
/// `d["new"] = v`); every other step must already exist.
fn walk_mut<'a>(
    v: &'a mut Value,
    keys: &[PathKey],
    insert_final: bool,
) -> Result<&'a mut Value, RaeError> {
    let Some((key, rest)) = keys.split_first() else {
        return Ok(v);
    };
    let is_final = rest.is_empty();
    match (v, key) {
        (Value::Array(items), PathKey::Index(Value::Int(i))) => {
            let i = *i;
            if i < 0 || i as usize >= items.len() {
                return Err(RaeError::IndexOutOfBounds(format!(
                    "index {} on array of {}",
                    i,
                    items.len()
                )));
            }
            walk_mut(&mut items[i as usize], rest, insert_final)
        }
        (Value::Dict(map), PathKey::Index(Value::Str(k))) => {
            if is_final && insert_final {
                return walk_mut(
                    map.entry(k.clone()).or_insert(Value::Unit),
                    rest,
                    insert_final,
                );
            }
            match map.get_mut(k) {
                Some(slot) => walk_mut(slot, rest, insert_final),
                None => Err(RaeError::IndexOutOfBounds(format!(
                    "no key \"{}\" in dictionary",
                    k
                ))),
            }
        }
        (Value::Struct { name, fields }, PathKey::Field(f)) => match fields.get_mut(f) {
            Some(slot) => walk_mut(slot, rest, insert_final),
            None => Err(RaeError::NoSuchField(format!(
                "{} has no field '{}'",
                name, f
            ))),
        },
        (other, PathKey::Index(i)) => Err(RaeError::TypeMismatch(format!(
            "cannot index {} with {}",
            other.type_name(),
            i.type_name()
        ))),
        (other, PathKey::Field(f)) => Err(RaeError::NoSuchField(format!(
            "{} has no field '{}'",
            other.type_name(),
            f
        ))),
    }
}

/// Run a script to completion under a fuel budget with no host bindings
/// (pure computation).
pub fn run(source: &str, fuel: u64) -> Result<Outcome, RaeError> {
    run_with_host(source, fuel, &mut NoHost)
}

/// Run a script under a fuel budget with an embedder-provided [`Host`] for
/// system bindings. Each statement, loop iteration, call, and host call
/// costs one fuel; exhaustion returns `Err(OutOfFuel)` — a runaway script
/// terminates deterministically.
pub fn run_with_host(source: &str, fuel: u64, host: &mut dyn Host) -> Result<Outcome, RaeError> {
    let prog = parse(source)?;
    let mut interp = Interp {
        funcs: BTreeMap::new(),
        structs: BTreeMap::new(),
        scopes: alloc::vec![BTreeMap::new()],
        output: String::new(),
        fuel,
        steps: 0,
        depth: 0,
        host,
    };
    let flow = interp.exec_block(&prog)?;
    let exit_code = match flow {
        Flow::Return(Value::Int(n)) => n,
        Flow::Break | Flow::Continue => return Err(RaeError::StrayControlFlow),
        _ => 0,
    };
    Ok(Outcome {
        exit_code,
        output: interp.output,
        steps: interp.steps,
    })
}

// ───────────────────────────── Tests ───────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── v0.1 surface (regression) ──

    #[test]
    fn arithmetic_and_print() {
        let out = run("let x = 6 * 7\nprint(\"answer: \\(x)\")", 1000).unwrap();
        assert_eq!(out.output, "answer: 42\n");
        assert_eq!(out.exit_code, 0);
    }

    #[test]
    fn while_loop_and_mutation() {
        let src = "var total = 0\nvar i = 1\nwhile i <= 10 { total = total + i\n i = i + 1 }\nreturn total";
        let out = run(src, 1000).unwrap();
        assert_eq!(out.exit_code, 55);
    }

    #[test]
    fn functions_and_recursion() {
        let src = "func fib(n) { if n < 2 { return n }\n return fib(n - 1) + fib(n - 2) }\nreturn fib(10)";
        let out = run(src, 100_000).unwrap();
        assert_eq!(out.exit_code, 55);
    }

    #[test]
    fn let_is_immutable() {
        assert_eq!(
            run("let x = 1\nx = 2", 100),
            Err(RaeError::AssignToLet("x".into()))
        );
    }

    #[test]
    fn runaway_loop_runs_out_of_fuel() {
        assert_eq!(run("while true { }", 1000), Err(RaeError::OutOfFuel));
    }

    #[test]
    fn division_by_zero() {
        assert_eq!(run("let x = 1 / 0", 100), Err(RaeError::DivisionByZero));
    }

    #[test]
    fn string_interpolation_nested() {
        let out = run(
            "let name = \"raeen\"\nprint(\"hi \\(name), \\(2 + 3) things\")",
            100,
        )
        .unwrap();
        assert_eq!(out.output, "hi raeen, 5 things\n");
    }

    #[test]
    fn if_else_chain() {
        let src = "let n = 7\nif n > 10 { return 1 } else if n > 5 { return 2 } else { return 3 }";
        assert_eq!(run(src, 100).unwrap().exit_code, 2);
    }

    // ── Floats ──

    #[test]
    fn float_arithmetic() {
        let out = run("print(\"\\(1.5 + 2.25)\")\nprint(\"\\(7.0 / 2.0)\")", 100).unwrap();
        assert_eq!(out.output, "3.75\n3.5\n");
    }

    #[test]
    fn mixed_int_float_promotes() {
        let out = run("print(\"\\(1 + 0.5)\")\nprint(\"\\(3 * 1.5)\")", 100).unwrap();
        assert_eq!(out.output, "1.5\n4.5\n");
    }

    #[test]
    fn float_compare_and_conversions() {
        let src = "if 1.5 > 1 { print(\"gt\") }\nprint(\"\\(Int(3.9))\")\nprint(\"\\(Float(2))\")\nprint(\"\\(Int(\"42\"))\")";
        let out = run(src, 100).unwrap();
        assert_eq!(out.output, "gt\n3\n2\n42\n");
    }

    #[test]
    fn float_mod_and_abs() {
        let out = run(
            "print(\"\\(7.5 % 2.0)\")\nprint(\"\\(abs(-3.5))\")\nprint(\"\\(abs(-4))\")",
            100,
        )
        .unwrap();
        assert_eq!(out.output, "1.5\n3.5\n4\n");
    }

    // ── Ranges + for + break/continue ──

    #[test]
    fn for_over_exclusive_range() {
        let src = "var sum = 0\nfor i in 0..<10 { sum = sum + i }\nreturn sum";
        assert_eq!(run(src, 1000).unwrap().exit_code, 45);
    }

    #[test]
    fn for_over_inclusive_range() {
        let src = "var sum = 0\nfor i in 1...10 { sum = sum + i }\nreturn sum";
        assert_eq!(run(src, 1000).unwrap().exit_code, 55);
    }

    #[test]
    fn break_and_continue() {
        let src = r#"
            var sum = 0
            for i in 1...100 {
                if i % 2 == 1 { continue }
                if i > 10 { break }
                sum = sum + i
            }
            return sum
        "#;
        // 2+4+6+8+10
        assert_eq!(run(src, 10_000).unwrap().exit_code, 30);
    }

    #[test]
    fn stray_break_is_an_error() {
        assert_eq!(run("break", 100), Err(RaeError::StrayControlFlow));
    }

    #[test]
    fn range_count_property() {
        let out = run("print(\"\\((0..<10).count) \\((1...10).count)\")", 100).unwrap();
        assert_eq!(out.output, "10 10\n");
    }

    // ── Arrays ──

    #[test]
    fn array_literal_index_and_count() {
        let src = "let a = [10, 20, 30]\nprint(\"\\(a[1]) \\(a.count) \\(a.first) \\(a.last)\")";
        let out = run(src, 100).unwrap();
        assert_eq!(out.output, "20 3 10 30\n");
    }

    #[test]
    fn array_mutation_append_remove() {
        let src = r#"
            var a = [1, 2]
            a.append(3)
            a[0] = 100
            let popped = a.removeLast()
            print("\(a) \(popped)")
        "#;
        let out = run(src, 100).unwrap();
        assert_eq!(out.output, "[100, 2] 3\n");
    }

    #[test]
    fn array_out_of_bounds() {
        assert!(matches!(
            run("let a = [1]\nlet x = a[5]", 100),
            Err(RaeError::IndexOutOfBounds(_))
        ));
    }

    #[test]
    fn mutating_immutable_array_fails() {
        assert_eq!(
            run("let a = [1]\na.append(2)", 100),
            Err(RaeError::AssignToLet("a".into()))
        );
        assert_eq!(
            run("let a = [1]\na[0] = 2", 100),
            Err(RaeError::AssignToLet("a".into()))
        );
    }

    #[test]
    fn for_over_array_and_concat() {
        let src = "var sum = 0\nfor x in [1, 2] + [3, 4] { sum = sum + x }\nreturn sum";
        assert_eq!(run(src, 1000).unwrap().exit_code, 10);
    }

    // ── Dictionaries ──

    #[test]
    fn dict_literal_read_insert() {
        let src = r#"
            var d = ["a": 1, "b": 2]
            d["c"] = 3
            d["a"] = 10
            print("\(d["a"]) \(d.count) \(d.hasKey("c")) \(d.hasKey("z"))")
        "#;
        let out = run(src, 100).unwrap();
        assert_eq!(out.output, "10 3 true false\n");
    }

    #[test]
    fn dict_keys_values_remove() {
        let src = r#"
            var d = ["x": 1, "y": 2]
            let gone = d.remove("x")
            print("\(d.keys) \(d.values) \(gone)")
        "#;
        let out = run(src, 100).unwrap();
        assert_eq!(out.output, "[y] [2] 1\n");
    }

    #[test]
    fn dict_missing_key_errors() {
        assert!(matches!(
            run("let d = [\"a\": 1]\nlet x = d[\"nope\"]", 100),
            Err(RaeError::IndexOutOfBounds(_))
        ));
    }

    #[test]
    fn empty_dict_literal() {
        let src = "var d = [:]\nd[\"k\"] = 1\nprint(\"\\(d.count) \\(d.isEmpty)\")";
        let out = run(src, 100).unwrap();
        assert_eq!(out.output, "1 false\n");
    }

    #[test]
    fn for_over_dict_keys() {
        let src = r#"
            let d = ["b": 2, "a": 1]
            var order = ""
            for k in d { order = order + k }
            print(order)
        "#;
        // BTreeMap iterates sorted.
        assert_eq!(run(src, 100).unwrap().output, "ab\n");
    }

    // ── Closures + first-class functions ──

    #[test]
    fn closure_basics_and_capture() {
        let src = r#"
            let double = { x in x * 2 }
            let n = 10
            let addn = { x in x + n }
            print("\(double(21)) \(addn(5))")
        "#;
        let out = run(src, 100).unwrap();
        assert_eq!(out.output, "42 15\n");
    }

    #[test]
    fn closure_capture_is_by_value() {
        let src = r#"
            var n = 1
            let f = { in n }
            n = 99
            return f()
        "#;
        // Snapshot at creation: still sees 1.
        assert_eq!(run(src, 100).unwrap().exit_code, 1);
    }

    #[test]
    fn map_filter_reduce() {
        let src = r#"
            let r = [1, 2, 3, 4].filter({ x in x % 2 == 0 }).map({ x in x * 10 })
            let sum = r.reduce(0, { a, b in a + b })
            print("\(r) \(sum)")
        "#;
        let out = run(src, 1000).unwrap();
        assert_eq!(out.output, "[20, 40] 60\n");
    }

    #[test]
    fn named_function_is_first_class() {
        let src = r#"
            func inc(x) { return x + 1 }
            let r = [1, 2].map(inc)
            print("\(r)")
        "#;
        assert_eq!(run(src, 1000).unwrap().output, "[2, 3]\n");
    }

    #[test]
    fn function_as_argument_and_return() {
        let src = r#"
            func apply(f, x) { return f(x) }
            let sq = { x in x * x }
            return apply(sq, 9)
        "#;
        assert_eq!(run(src, 1000).unwrap().exit_code, 81);
    }

    // ── Structs ──

    #[test]
    fn struct_declare_construct_access_mutate() {
        let src = r#"
            struct Point { x, y }
            var p = Point(1, 2)
            p.x = 5
            print("\(p.x + p.y) \(p)")
        "#;
        let out = run(src, 100).unwrap();
        assert_eq!(out.output, "7 Point(x: 5, y: 2)\n");
    }

    #[test]
    fn struct_field_errors() {
        assert!(matches!(
            run("struct P { x }\nlet p = P(1)\nlet z = p.nope", 100),
            Err(RaeError::NoSuchField(_))
        ));
        assert!(matches!(
            run("struct P { x, y }\nlet p = P(1)", 100),
            Err(RaeError::ArityMismatch(_))
        ));
    }

    #[test]
    fn nested_path_assignment() {
        let src = r#"
            struct P { x }
            var list = [P(1), P(2)]
            list[1].x = 42
            return list[1].x
        "#;
        assert_eq!(run(src, 100).unwrap().exit_code, 42);
    }

    // ── String methods ──

    #[test]
    fn string_methods() {
        let src = r#"
            let s = "Rae Script"
            print("\(s.uppercased()) \(s.count) \(s.contains("Scr")) \(s.hasPrefix("Rae"))")
            print("\("a,b,c".split(","))")
            print("\("  pad  ".trimmed())")
        "#;
        let out = run(src, 100).unwrap();
        assert_eq!(out.output, "RAE SCRIPT 10 true true\n[a, b, c]\npad\n");
    }

    #[test]
    fn for_over_string_chars() {
        let src = "var r = \"\"\nfor c in \"abc\" { r = c + r }\nprint(r)";
        assert_eq!(run(src, 100).unwrap().output, "cba\n");
    }

    // ── Host bindings ──

    struct TestHost {
        allow_notify: bool,
        notified: usize,
    }

    impl Host for TestHost {
        fn call(&mut self, name: &str, args: &[Value]) -> Result<Value, HostError> {
            match name {
                "uptimeMs" => Ok(Value::Int(12_345)),
                "notify" => {
                    if !self.allow_notify {
                        return Err(HostError::Denied("notify: cap not granted".into()));
                    }
                    if args.len() != 1 {
                        return Err(HostError::Failed("notify takes 1 argument".into()));
                    }
                    self.notified += 1;
                    Ok(Value::Bool(true))
                }
                _ => Err(HostError::Unknown),
            }
        }
    }

    #[test]
    fn host_binding_call_works() {
        let mut host = TestHost {
            allow_notify: true,
            notified: 0,
        };
        let src = "notify(\"hello from script\")\nreturn Int(uptimeMs() / 1000)";
        let out = run_with_host(src, 100, &mut host).unwrap();
        assert_eq!(out.exit_code, 12);
        assert_eq!(host.notified, 1);
    }

    #[test]
    fn host_binding_denied_fails_closed() {
        let mut host = TestHost {
            allow_notify: false,
            notified: 0,
        };
        assert_eq!(
            run_with_host("notify(\"x\")", 100, &mut host),
            Err(RaeError::CapabilityDenied("notify: cap not granted".into()))
        );
        assert_eq!(host.notified, 0);
    }

    #[test]
    fn unknown_host_call_is_undefined_function() {
        let mut host = TestHost {
            allow_notify: true,
            notified: 0,
        };
        assert_eq!(
            run_with_host("teleport()", 100, &mut host),
            Err(RaeError::UndefinedFunction("teleport".into()))
        );
    }

    #[test]
    fn no_host_means_pure_compute() {
        assert_eq!(
            run("notify(\"x\")", 100),
            Err(RaeError::UndefinedFunction("notify".into()))
        );
    }
}
