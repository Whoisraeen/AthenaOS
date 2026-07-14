//! The ECMAScript lexer: source text → a flat [`Token`] stream.
//!
//! Three subtleties make a JS lexer harder than the spreadsheet/CSS tokenizers:
//!   - **Regex vs. division** (`/`): `a / b` is division but `/ab+/g` is a regex
//!     literal. The classic disambiguation: a `/` begins a regex iff a value/operand
//!     could *not* legally precede it — decided from the previous significant token
//!     ([`regex_allowed`]).
//!   - **Template literals** (`` `…${expr}…` ``): the `${` … `}` holes contain
//!     arbitrary expressions (which may themselves contain `}`, strings, nested
//!     templates), so a template is lexed as a small bracket-balanced sub-scan rather
//!     than a single regex-like span.
//!   - **Automatic Semicolon Insertion (ASI)**: newlines are significant. The lexer
//!     records, on each token, whether a line terminator preceded it
//!     ([`Token::newline_before`]); the parser uses that to insert the semicolons the
//!     source omits and to forbid a newline after `return`/`break`/`continue`/`++`/`--`.
//!
//! Every failure is a positioned [`JsError`]; the scan is strictly advancing (each
//! branch consumes ≥1 byte or terminates), so it cannot loop, and the [`MAX_TOKENS`]
//! bound caps total work.

use alloc::string::String;
use alloc::vec::Vec;

use crate::{JsError, MAX_SOURCE_LEN, MAX_TOKENS};

/// A lexed token: its kind, the byte span it covers, and whether a newline (line
/// terminator or block comment containing one) appeared before it (for ASI).
#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub start: usize,
    pub end: usize,
    pub line: usize,
    pub col: usize,
    pub newline_before: bool,
}

/// The lexical category of a [`Token`].
#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    /// An identifier (also covers `undefined`, which is technically an identifier).
    Ident(String),
    /// A reserved word.
    Keyword(Keyword),
    /// A numeric literal already parsed to its `f64` value.
    Number(f64),
    /// A BigInt literal `…n` — the digit text (without the `n`).
    BigInt(String),
    /// A string literal with escapes decoded.
    String(String),
    /// A regex literal: `(pattern, flags)`.
    Regex(String, String),
    /// A complete template literal, pre-split: `quasis` are the cooked chunks and
    /// `expr_sources` are the raw `${…}` source slices (re-lexed/parsed by the parser).
    /// `quasis.len() == expr_sources.len() + 1`.
    Template {
        quasis: Vec<String>,
        expr_sources: Vec<String>,
    },
    /// A punctuator / operator.
    Punct(Punct),
    /// End of input.
    Eof,
}

/// ECMAScript reserved words this lexer recognizes (others stay [`TokenKind::Ident`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Keyword {
    Var,
    Let,
    Const,
    Function,
    Return,
    If,
    Else,
    For,
    While,
    Do,
    Break,
    Continue,
    New,
    Typeof,
    Instanceof,
    In,
    Of,
    This,
    Null,
    True,
    False,
    Delete,
    Void,
    Switch,
    Case,
    Default,
    Throw,
    Try,
    Catch,
    Finally,
    Class,
    Extends,
    Super,
    Yield,
    Async,
    Await,
    Static,
    Get,
    Set,
}

/// Punctuators / operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Punct {
    LBrace,
    RBrace,
    LParen,
    RParen,
    LBracket,
    RBracket,
    Dot,
    DotDotDot,
    Semicolon,
    Comma,
    Arrow,         // =>
    OptionalChain, // ?.
    Question,
    Colon,
    // assignment & compound
    Assign,
    PlusEq,
    MinusEq,
    StarEq,
    SlashEq,
    PercentEq,
    StarStarEq,
    ShlEq,
    ShrEq,
    UShrEq,
    AmpEq,
    PipeEq,
    CaretEq,
    AmpAmpEq,
    PipePipeEq,
    QuestionQuestionEq,
    // arithmetic
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    StarStar,
    PlusPlus,
    MinusMinus,
    // comparison
    EqEq,
    NotEq,
    EqEqEq,
    NotEqEq,
    Lt,
    Gt,
    LtEq,
    GtEq,
    // logical / nullish
    AmpAmp,
    PipePipe,
    QuestionQuestion,
    Not,
    // bitwise
    Amp,
    Pipe,
    Caret,
    Tilde,
    Shl,
    Shr,
    UShr,
}

struct Lexer<'a> {
    src: &'a [u8],
    s: &'a str,
    pos: usize,
    line: usize,
    line_start: usize,
    tokens: Vec<Token>,
}

/// Lex source text into a token stream terminated by [`TokenKind::Eof`].
pub fn lex(src: &str) -> Result<Vec<Token>, JsError> {
    if src.len() > MAX_SOURCE_LEN {
        return Err(JsError::new("source exceeds maximum length", 0, 1, 1));
    }
    let mut lx = Lexer {
        src: src.as_bytes(),
        s: src,
        pos: 0,
        line: 1,
        line_start: 0,
        tokens: Vec::new(),
    };
    lx.run()?;
    Ok(lx.tokens)
}

impl<'a> Lexer<'a> {
    #[inline]
    fn at(&self, i: usize) -> u8 {
        if i < self.src.len() {
            self.src[i]
        } else {
            0
        }
    }

    #[inline]
    fn cur(&self) -> u8 {
        self.at(self.pos)
    }

    fn col(&self, pos: usize) -> usize {
        pos.saturating_sub(self.line_start) + 1
    }

    fn err(&self, msg: &str, pos: usize) -> JsError {
        JsError::new(msg, pos, self.line, self.col(pos))
    }

    /// Whether a `/` at the current position begins a regex literal, based on the
    /// previous significant token. A regex is allowed where an *operand* is expected
    /// (start of input, after most operators/punctuators, after keywords like `return`),
    /// and division where a *value* just ended (ident, literal, `)`, `]`, `}`).
    fn regex_allowed(&self) -> bool {
        match self.tokens.last() {
            None => true,
            Some(t) => match &t.kind {
                TokenKind::Ident(_)
                | TokenKind::Number(_)
                | TokenKind::BigInt(_)
                | TokenKind::String(_)
                | TokenKind::Regex(_, _)
                | TokenKind::Template { .. } => false,
                TokenKind::Keyword(k) => !matches!(
                    k,
                    // After `this`/`super`/`true`/`false`/`null` a `/` is division;
                    // after any other keyword (return, typeof, in, …) it's a regex.
                    Keyword::This | Keyword::Super | Keyword::True | Keyword::False | Keyword::Null
                ),
                // Closers and postfix `++`/`--` end a value → division follows; every
                // other punctuator expects an operand → a regex literal follows. (A
                // `}` is ambiguous — it can end a block, after which a regex is fine —
                // but treating it as a value-end is the common, safe heuristic for our
                // current parser, which never feeds a bare regex right after a `}`.)
                TokenKind::Punct(p) => !matches!(
                    p,
                    Punct::RParen
                        | Punct::RBracket
                        | Punct::RBrace
                        | Punct::PlusPlus
                        | Punct::MinusMinus
                ),
                TokenKind::Eof => true,
            },
        }
    }

    fn run(&mut self) -> Result<(), JsError> {
        loop {
            let newline_before = self.skip_trivia()?;
            if self.tokens.len() > MAX_TOKENS {
                return Err(self.err("too many tokens", self.pos));
            }
            if self.pos >= self.src.len() {
                let start = self.pos;
                self.tokens.push(Token {
                    kind: TokenKind::Eof,
                    start,
                    end: start,
                    line: self.line,
                    col: self.col(start),
                    newline_before,
                });
                return Ok(());
            }
            let start = self.pos;
            let line = self.line;
            let col = self.col(start);
            let kind = self.next_token()?;
            self.tokens.push(Token {
                kind,
                start,
                end: self.pos,
                line,
                col,
                newline_before,
            });
        }
    }

    /// Skip whitespace and comments; return whether any line terminator was crossed.
    fn skip_trivia(&mut self) -> Result<bool, JsError> {
        let mut newline = false;
        loop {
            let c = self.cur();
            match c {
                b' ' | b'\t' | 0x0b | 0x0c | b'\r' => {
                    self.pos += 1;
                }
                b'\n' => {
                    newline = true;
                    self.pos += 1;
                    self.line += 1;
                    self.line_start = self.pos;
                }
                b'/' if self.at(self.pos + 1) == b'/' => {
                    self.pos += 2;
                    while self.pos < self.src.len() && self.cur() != b'\n' {
                        self.pos += 1;
                    }
                }
                b'/' if self.at(self.pos + 1) == b'*' => {
                    let start = self.pos;
                    self.pos += 2;
                    loop {
                        if self.pos >= self.src.len() {
                            return Err(self.err("unterminated block comment", start));
                        }
                        if self.cur() == b'*' && self.at(self.pos + 1) == b'/' {
                            self.pos += 2;
                            break;
                        }
                        if self.cur() == b'\n' {
                            newline = true;
                            self.line += 1;
                            self.line_start = self.pos + 1;
                        }
                        self.pos += 1;
                    }
                }
                // UTF-8 whitespace (NBSP / line/paragraph separators) — treat leading
                // bytes conservatively: only ASCII trivia above is skipped; multibyte
                // chars fall through to be scanned as identifier/other.
                _ => break,
            }
        }
        Ok(newline)
    }

    fn next_token(&mut self) -> Result<TokenKind, JsError> {
        let c = self.cur();
        match c {
            b'"' | b'\'' => self.lex_string(c),
            b'`' => self.lex_template(),
            b'0'..=b'9' => self.lex_number(),
            b'.' if self.at(self.pos + 1).is_ascii_digit() => self.lex_number(),
            b'/' => {
                if self.regex_allowed() {
                    self.lex_regex()
                } else {
                    self.lex_punct()
                }
            }
            _ => {
                if is_ident_start(c) {
                    Ok(self.lex_ident())
                } else if c >= 0x80 {
                    // A non-ASCII byte: accept as part of an identifier (covers Unicode
                    // identifiers conservatively) rather than failing.
                    Ok(self.lex_ident())
                } else {
                    self.lex_punct()
                }
            }
        }
    }

    fn lex_ident(&mut self) -> TokenKind {
        let start = self.pos;
        // Advance over identifier bytes (ASCII ident chars + any non-ASCII byte).
        while self.pos < self.src.len() {
            let c = self.cur();
            if is_ident_continue(c) || c >= 0x80 {
                self.pos += 1;
            } else {
                break;
            }
        }
        let word = &self.s[start..self.pos];
        match keyword_of(word) {
            Some(k) => TokenKind::Keyword(k),
            None => TokenKind::Ident(String::from(word)),
        }
    }

    fn lex_number(&mut self) -> Result<TokenKind, JsError> {
        let start = self.pos;
        // Radix prefixes.
        if self.cur() == b'0' {
            let n = self.at(self.pos + 1);
            match n {
                b'x' | b'X' => return self.lex_radix(start, 16),
                b'o' | b'O' => return self.lex_radix(start, 8),
                b'b' | b'B' => return self.lex_radix(start, 2),
                _ => {}
            }
        }
        // Decimal / float / exponent.
        let mut seen_dot = false;
        let mut seen_exp = false;
        while self.pos < self.src.len() {
            let c = self.cur();
            match c {
                b'0'..=b'9' | b'_' => self.pos += 1,
                b'.' if !seen_dot && !seen_exp => {
                    seen_dot = true;
                    self.pos += 1;
                }
                b'e' | b'E' if !seen_exp => {
                    seen_exp = true;
                    self.pos += 1;
                    if self.cur() == b'+' || self.cur() == b'-' {
                        self.pos += 1;
                    }
                }
                _ => break,
            }
        }
        // BigInt suffix (only on integers).
        if self.cur() == b'n' && !seen_dot && !seen_exp {
            let digits = self.s[start..self.pos].replace('_', "");
            self.pos += 1;
            return Ok(TokenKind::BigInt(digits));
        }
        let raw = self.s[start..self.pos].replace('_', "");
        match parse_decimal(&raw) {
            Some(v) => Ok(TokenKind::Number(v)),
            None => Err(self.err("invalid numeric literal", start)),
        }
    }

    fn lex_radix(&mut self, start: usize, radix: u32) -> Result<TokenKind, JsError> {
        self.pos += 2; // skip 0x / 0o / 0b
        let digits_start = self.pos;
        while self.pos < self.src.len() {
            let c = self.cur();
            if c == b'_' || (c as char).is_digit(radix) {
                self.pos += 1;
            } else {
                break;
            }
        }
        if self.pos == digits_start {
            return Err(self.err("missing digits after radix prefix", start));
        }
        let is_bigint = self.cur() == b'n';
        let digits = self.s[digits_start..self.pos].replace('_', "");
        if is_bigint {
            self.pos += 1;
            return Ok(TokenKind::BigInt(digits));
        }
        match u128::from_str_radix(&digits, radix) {
            Ok(v) => Ok(TokenKind::Number(v as f64)),
            Err(_) => Err(self.err("numeric literal out of range", start)),
        }
    }

    fn lex_string(&mut self, quote: u8) -> Result<TokenKind, JsError> {
        let start = self.pos;
        self.pos += 1; // opening quote
        let mut out = String::new();
        loop {
            if self.pos >= self.src.len() {
                return Err(self.err("unterminated string literal", start));
            }
            let c = self.cur();
            if c == quote {
                self.pos += 1;
                return Ok(TokenKind::String(out));
            }
            if c == b'\n' {
                return Err(self.err("unterminated string literal (newline)", start));
            }
            if c == b'\\' {
                self.pos += 1;
                self.lex_escape(&mut out)?;
            } else {
                // Copy one UTF-8 scalar verbatim.
                let cstart = self.pos;
                self.pos += 1;
                while self.pos < self.src.len() && (self.cur() & 0xC0) == 0x80 {
                    self.pos += 1;
                }
                out.push_str(&self.s[cstart..self.pos]);
            }
        }
    }

    /// Decode the escape following a `\` (cursor is just past the backslash).
    fn lex_escape(&mut self, out: &mut String) -> Result<(), JsError> {
        if self.pos >= self.src.len() {
            return Err(self.err("unterminated escape sequence", self.pos));
        }
        let e = self.cur();
        self.pos += 1;
        match e {
            b'n' => out.push('\n'),
            b't' => out.push('\t'),
            b'r' => out.push('\r'),
            b'b' => out.push('\u{0008}'),
            b'f' => out.push('\u{000C}'),
            b'v' => out.push('\u{000B}'),
            b'0' if !self.cur().is_ascii_digit() => out.push('\0'),
            b'\\' => out.push('\\'),
            b'\'' => out.push('\''),
            b'"' => out.push('"'),
            b'`' => out.push('`'),
            b'\n' => {
                // Line continuation — escaped newline produces nothing.
                self.line += 1;
                self.line_start = self.pos;
            }
            b'\r' => {
                if self.cur() == b'\n' {
                    self.pos += 1;
                }
                self.line += 1;
                self.line_start = self.pos;
            }
            b'x' => {
                let v = self.read_hex(2)?;
                push_code_point(out, v, self)?;
            }
            b'u' => {
                if self.cur() == b'{' {
                    self.pos += 1;
                    let hstart = self.pos;
                    let mut v: u32 = 0;
                    let mut count = 0;
                    while self.pos < self.src.len() && self.cur() != b'}' {
                        let d = hex_val(self.cur())
                            .ok_or_else(|| self.err("invalid \\u{} escape", hstart))?;
                        v = v.saturating_mul(16).saturating_add(d);
                        count += 1;
                        if count > 6 {
                            return Err(self.err("\\u{} escape too long", hstart));
                        }
                        self.pos += 1;
                    }
                    if self.cur() != b'}' || count == 0 {
                        return Err(self.err("unterminated \\u{} escape", hstart));
                    }
                    self.pos += 1; // }
                    push_code_point(out, v, self)?;
                } else {
                    let v = self.read_hex(4)?;
                    push_code_point(out, v, self)?;
                }
            }
            // Any other escaped char is itself (the spec's "non-escape character").
            _ => {
                // Re-read the byte as a UTF-8 scalar.
                let cstart = self.pos - 1;
                let mut end = self.pos;
                while end < self.src.len() && (self.at(end) & 0xC0) == 0x80 {
                    end += 1;
                }
                self.pos = end;
                out.push_str(&self.s[cstart..end]);
            }
        }
        Ok(())
    }

    fn read_hex(&mut self, n: usize) -> Result<u32, JsError> {
        let start = self.pos;
        let mut v: u32 = 0;
        for _ in 0..n {
            let d = hex_val(self.cur()).ok_or_else(|| self.err("invalid hex escape", start))?;
            v = v * 16 + d;
            self.pos += 1;
        }
        Ok(v)
    }

    fn lex_template(&mut self) -> Result<TokenKind, JsError> {
        let start = self.pos;
        self.pos += 1; // opening backtick
        let mut quasis: Vec<String> = Vec::new();
        let mut expr_sources: Vec<String> = Vec::new();
        let mut cur = String::new();
        loop {
            if self.pos >= self.src.len() {
                return Err(self.err("unterminated template literal", start));
            }
            let c = self.cur();
            if c == b'`' {
                self.pos += 1;
                quasis.push(cur);
                return Ok(TokenKind::Template {
                    quasis,
                    expr_sources,
                });
            }
            if c == b'\\' {
                self.pos += 1;
                self.lex_escape(&mut cur)?;
                continue;
            }
            if c == b'$' && self.at(self.pos + 1) == b'{' {
                quasis.push(core::mem::take(&mut cur));
                self.pos += 2; // ${
                let expr_start = self.pos;
                self.scan_template_hole(start)?;
                let expr_end = self.pos - 1; // exclude closing }
                expr_sources.push(String::from(&self.s[expr_start..expr_end]));
                continue;
            }
            if c == b'\n' {
                self.line += 1;
                self.line_start = self.pos + 1;
            }
            let cstart = self.pos;
            self.pos += 1;
            while self.pos < self.src.len() && (self.cur() & 0xC0) == 0x80 {
                self.pos += 1;
            }
            cur.push_str(&self.s[cstart..self.pos]);
        }
    }

    /// Advance the cursor past a balanced `${ … }` hole, leaving it just after the `}`.
    /// Tracks nested braces, strings, template literals and comments so a `}` inside a
    /// string/nested template does not close the hole prematurely.
    fn scan_template_hole(&mut self, tmpl_start: usize) -> Result<(), JsError> {
        let mut depth: usize = 1;
        let mut guard: usize = 0;
        while self.pos < self.src.len() {
            guard += 1;
            if guard > MAX_TOKENS {
                return Err(self.err("template expression too large", tmpl_start));
            }
            let c = self.cur();
            match c {
                b'{' => {
                    depth += 1;
                    self.pos += 1;
                }
                b'}' => {
                    depth -= 1;
                    self.pos += 1;
                    if depth == 0 {
                        return Ok(());
                    }
                }
                b'"' | b'\'' => {
                    self.skip_string_in_hole(c, tmpl_start)?;
                }
                b'`' => {
                    self.skip_nested_template_in_hole(tmpl_start)?;
                }
                b'/' if self.at(self.pos + 1) == b'/' => {
                    self.pos += 2;
                    while self.pos < self.src.len() && self.cur() != b'\n' {
                        self.pos += 1;
                    }
                }
                b'/' if self.at(self.pos + 1) == b'*' => {
                    self.pos += 2;
                    while self.pos < self.src.len()
                        && !(self.cur() == b'*' && self.at(self.pos + 1) == b'/')
                    {
                        if self.cur() == b'\n' {
                            self.line += 1;
                            self.line_start = self.pos + 1;
                        }
                        self.pos += 1;
                    }
                    if self.pos < self.src.len() {
                        self.pos += 2;
                    }
                }
                b'\n' => {
                    self.line += 1;
                    self.line_start = self.pos + 1;
                    self.pos += 1;
                }
                _ => self.pos += 1,
            }
        }
        Err(self.err("unterminated template expression", tmpl_start))
    }

    fn skip_string_in_hole(&mut self, quote: u8, tmpl_start: usize) -> Result<(), JsError> {
        self.pos += 1;
        while self.pos < self.src.len() {
            let c = self.cur();
            if c == b'\\' {
                self.pos += 2;
                continue;
            }
            if c == quote {
                self.pos += 1;
                return Ok(());
            }
            if c == b'\n' {
                return Err(self.err("unterminated string in template", tmpl_start));
            }
            self.pos += 1;
        }
        Err(self.err("unterminated string in template", tmpl_start))
    }

    fn skip_nested_template_in_hole(&mut self, tmpl_start: usize) -> Result<(), JsError> {
        self.pos += 1; // opening backtick
        while self.pos < self.src.len() {
            let c = self.cur();
            if c == b'\\' {
                self.pos += 2;
                continue;
            }
            if c == b'`' {
                self.pos += 1;
                return Ok(());
            }
            if c == b'$' && self.at(self.pos + 1) == b'{' {
                self.pos += 2;
                self.scan_template_hole(tmpl_start)?;
                continue;
            }
            if c == b'\n' {
                self.line += 1;
                self.line_start = self.pos + 1;
            }
            self.pos += 1;
        }
        Err(self.err("unterminated nested template", tmpl_start))
    }

    fn lex_regex(&mut self) -> Result<TokenKind, JsError> {
        let start = self.pos;
        self.pos += 1; // opening /
        let body_start = self.pos;
        let mut in_class = false; // inside [ ... ]
        loop {
            if self.pos >= self.src.len() {
                return Err(self.err("unterminated regex literal", start));
            }
            let c = self.cur();
            match c {
                b'\\' => {
                    self.pos += 2; // escape — skip next char
                    continue;
                }
                b'[' => {
                    in_class = true;
                    self.pos += 1;
                }
                b']' => {
                    in_class = false;
                    self.pos += 1;
                }
                b'/' if !in_class => {
                    break;
                }
                b'\n' => {
                    return Err(self.err("unterminated regex literal (newline)", start));
                }
                _ => {
                    self.pos += 1;
                    while self.pos < self.src.len() && (self.cur() & 0xC0) == 0x80 {
                        self.pos += 1;
                    }
                }
            }
        }
        let pattern = String::from(&self.s[body_start..self.pos]);
        self.pos += 1; // closing /
        let flags_start = self.pos;
        while self.pos < self.src.len() {
            let c = self.cur();
            if is_ident_continue(c) {
                self.pos += 1;
            } else {
                break;
            }
        }
        let flags = String::from(&self.s[flags_start..self.pos]);
        Ok(TokenKind::Regex(pattern, flags))
    }

    fn lex_punct(&mut self) -> Result<TokenKind, JsError> {
        let start = self.pos;
        let c = self.cur();
        let c1 = self.at(self.pos + 1);
        let c2 = self.at(self.pos + 2);
        let c3 = self.at(self.pos + 3);

        // Helper closures would borrow self; just advance inline.
        macro_rules! emit {
            ($n:expr, $p:expr) => {{
                self.pos += $n;
                return Ok(TokenKind::Punct($p));
            }};
        }

        match c {
            b'{' => emit!(1, Punct::LBrace),
            b'}' => emit!(1, Punct::RBrace),
            b'(' => emit!(1, Punct::LParen),
            b')' => emit!(1, Punct::RParen),
            b'[' => emit!(1, Punct::LBracket),
            b']' => emit!(1, Punct::RBracket),
            b';' => emit!(1, Punct::Semicolon),
            b',' => emit!(1, Punct::Comma),
            b'~' => emit!(1, Punct::Tilde),
            b'.' => {
                if c1 == b'.' && c2 == b'.' {
                    emit!(3, Punct::DotDotDot);
                }
                emit!(1, Punct::Dot);
            }
            b'?' => {
                if c1 == b'.' && !c2.is_ascii_digit() {
                    emit!(2, Punct::OptionalChain);
                }
                if c1 == b'?' && c2 == b'=' {
                    emit!(3, Punct::QuestionQuestionEq);
                }
                if c1 == b'?' {
                    emit!(2, Punct::QuestionQuestion);
                }
                emit!(1, Punct::Question);
            }
            b':' => emit!(1, Punct::Colon),
            b'+' => {
                if c1 == b'+' {
                    emit!(2, Punct::PlusPlus);
                }
                if c1 == b'=' {
                    emit!(2, Punct::PlusEq);
                }
                emit!(1, Punct::Plus);
            }
            b'-' => {
                if c1 == b'-' {
                    emit!(2, Punct::MinusMinus);
                }
                if c1 == b'=' {
                    emit!(2, Punct::MinusEq);
                }
                emit!(1, Punct::Minus);
            }
            b'*' => {
                if c1 == b'*' && c2 == b'=' {
                    emit!(3, Punct::StarStarEq);
                }
                if c1 == b'*' {
                    emit!(2, Punct::StarStar);
                }
                if c1 == b'=' {
                    emit!(2, Punct::StarEq);
                }
                emit!(1, Punct::Star);
            }
            b'/' => {
                if c1 == b'=' {
                    emit!(2, Punct::SlashEq);
                }
                emit!(1, Punct::Slash);
            }
            b'%' => {
                if c1 == b'=' {
                    emit!(2, Punct::PercentEq);
                }
                emit!(1, Punct::Percent);
            }
            b'=' => {
                if c1 == b'=' && c2 == b'=' {
                    emit!(3, Punct::EqEqEq);
                }
                if c1 == b'=' {
                    emit!(2, Punct::EqEq);
                }
                if c1 == b'>' {
                    emit!(2, Punct::Arrow);
                }
                emit!(1, Punct::Assign);
            }
            b'!' => {
                if c1 == b'=' && c2 == b'=' {
                    emit!(3, Punct::NotEqEq);
                }
                if c1 == b'=' {
                    emit!(2, Punct::NotEq);
                }
                emit!(1, Punct::Not);
            }
            b'<' => {
                if c1 == b'<' && c2 == b'=' {
                    emit!(3, Punct::ShlEq);
                }
                if c1 == b'<' {
                    emit!(2, Punct::Shl);
                }
                if c1 == b'=' {
                    emit!(2, Punct::LtEq);
                }
                emit!(1, Punct::Lt);
            }
            b'>' => {
                if c1 == b'>' && c2 == b'>' && c3 == b'=' {
                    emit!(4, Punct::UShrEq);
                }
                if c1 == b'>' && c2 == b'>' {
                    emit!(3, Punct::UShr);
                }
                if c1 == b'>' && c2 == b'=' {
                    emit!(3, Punct::ShrEq);
                }
                if c1 == b'>' {
                    emit!(2, Punct::Shr);
                }
                if c1 == b'=' {
                    emit!(2, Punct::GtEq);
                }
                emit!(1, Punct::Gt);
            }
            b'&' => {
                if c1 == b'&' && c2 == b'=' {
                    emit!(3, Punct::AmpAmpEq);
                }
                if c1 == b'&' {
                    emit!(2, Punct::AmpAmp);
                }
                if c1 == b'=' {
                    emit!(2, Punct::AmpEq);
                }
                emit!(1, Punct::Amp);
            }
            b'|' => {
                if c1 == b'|' && c2 == b'=' {
                    emit!(3, Punct::PipePipeEq);
                }
                if c1 == b'|' {
                    emit!(2, Punct::PipePipe);
                }
                if c1 == b'=' {
                    emit!(2, Punct::PipeEq);
                }
                emit!(1, Punct::Pipe);
            }
            b'^' => {
                if c1 == b'=' {
                    emit!(2, Punct::CaretEq);
                }
                emit!(1, Punct::Caret);
            }
            _ => Err(self.err("unexpected character", start)),
        }
    }
}

// ─── character classes & literal decoding ────────────────────────────────────

fn is_ident_start(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'_' || c == b'$'
}

fn is_ident_continue(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_' || c == b'$'
}

fn hex_val(c: u8) -> Option<u32> {
    match c {
        b'0'..=b'9' => Some((c - b'0') as u32),
        b'a'..=b'f' => Some((c - b'a' + 10) as u32),
        b'A'..=b'F' => Some((c - b'A' + 10) as u32),
        _ => None,
    }
}

/// Push a Unicode scalar (or a lone surrogate rendered as U+FFFD) onto `out`.
fn push_code_point(out: &mut String, v: u32, lx: &Lexer) -> Result<(), JsError> {
    match char::from_u32(v) {
        Some(ch) => {
            out.push(ch);
            Ok(())
        }
        None => {
            // Lone surrogates (\uD800..\uDFFF) and out-of-range: substitute U+FFFD
            // rather than failing the whole parse (matches lenient engines).
            let _ = lx;
            out.push('\u{FFFD}');
            Ok(())
        }
    }
}

/// Parse a decimal/float/exponent literal (already `_`-stripped) into `f64`.
fn parse_decimal(s: &str) -> Option<f64> {
    if s.is_empty() {
        return None;
    }
    // `core::str::parse::<f64>` is available in no_std core and rejects junk.
    s.parse::<f64>().ok()
}

fn keyword_of(word: &str) -> Option<Keyword> {
    Some(match word {
        "var" => Keyword::Var,
        "let" => Keyword::Let,
        "const" => Keyword::Const,
        "function" => Keyword::Function,
        "return" => Keyword::Return,
        "if" => Keyword::If,
        "else" => Keyword::Else,
        "for" => Keyword::For,
        "while" => Keyword::While,
        "do" => Keyword::Do,
        "break" => Keyword::Break,
        "continue" => Keyword::Continue,
        "new" => Keyword::New,
        "typeof" => Keyword::Typeof,
        "instanceof" => Keyword::Instanceof,
        "in" => Keyword::In,
        "of" => Keyword::Of,
        "this" => Keyword::This,
        "null" => Keyword::Null,
        "true" => Keyword::True,
        "false" => Keyword::False,
        "delete" => Keyword::Delete,
        "void" => Keyword::Void,
        "switch" => Keyword::Switch,
        "case" => Keyword::Case,
        "default" => Keyword::Default,
        "throw" => Keyword::Throw,
        "try" => Keyword::Try,
        "catch" => Keyword::Catch,
        "finally" => Keyword::Finally,
        "class" => Keyword::Class,
        "extends" => Keyword::Extends,
        "super" => Keyword::Super,
        "yield" => Keyword::Yield,
        "async" => Keyword::Async,
        "await" => Keyword::Await,
        "static" => Keyword::Static,
        "get" => Keyword::Get,
        "set" => Keyword::Set,
        _ => return None,
    })
}
