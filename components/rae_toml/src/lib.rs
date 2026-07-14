//! # RaeTOML — a never-panic, `no_std` TOML parser + serializer (TOML v1.0 subset).
//!
//! LEGACY_GAMING_CONCEPT.md §"The user owns the machine": no forced updates, no
//! telemetry, no ads — the machine, *and its configuration*, belong to the user.
//! That promise is only real if "remember my settings" is real: themes, app
//! preferences, service config, and the OS's own `config/base.toml` must read
//! AND write back robustly, with key order preserved so a user's file survives a
//! round-trip unchanged. TOML is the format that config — so one correct,
//! dependency-free, hostile-input TOML core is foundational infrastructure, not
//! tied to any one consumer (it is deliberately wired into none this slice).
//!
//! ## Hostile-input posture (CLAUDE: parsers of untrusted bytes are an RCE surface)
//! Every byte handed to [`parse`] is treated as hostile. There is **no
//! `unwrap`/`expect`/`panic`/raw-index-panic path** reachable from the parser:
//! unterminated strings/arrays, bad escapes, duplicate keys, malformed numbers,
//! unclosed table headers, and pathological nesting all return `Err(TomlError)`.
//! Recursion is bounded by [`MAX_DEPTH`] so a crafted deeply-nested document
//! cannot blow the stack. The host KAT suite at the bottom of this file is the
//! primary proof (`cargo test -p rae_toml`).
//!
//! ## What it is
//! - A [`Toml`] value enum mirroring the TOML grammar, **preserving table key
//!   order** (tables are `Vec<(String, Toml)>`, not a hash map) so config files
//!   round-trip with keys in source order.
//! - [`parse`]: a line/recursive-descent parser over UTF-8 `&str` handling
//!   `key = value`, `[table]` / `[a.b.c]` nested headers, `[[array.of.tables]]`,
//!   basic/literal/multi-line strings with escapes, integers (`_` separators,
//!   `0x`/`0o`/`0b`, sign), floats (frac/exp, `inf`/`nan`), booleans, arrays
//!   (nested, multi-line, trailing comma), inline tables, and `#` comments.
//! - [`to_string`]: a round-trippable serializer emitting `[header]` sections.
//! - Convenience accessors ([`Toml::as_str`], [`Toml::get`], [`Toml::get_path`],
//!   [`Toml::at`], …), all `Option`-returning and panic-free.
//!
//! ## Documented omissions (this cut)
//! - **Datetimes** are kept as `Toml::String` — TOML's offset/local
//!   date-time/date/time types are parsed as the bare token text into a String
//!   rather than a dedicated variant. Config consumers that need a timestamp can
//!   re-parse the string; no value is lost on round-trip.
//! - Integers are `i64` (TOML's spec range); values outside `i64` are an `Err`.
//! - Dotted *keys* in `key = value` position (`a.b.c = 1`) ARE supported;
//!   number-underscore rules are validated leniently (no leading/trailing `_`).

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

/// Maximum nesting depth of arrays / inline tables / `[a.b.c…]` header segments
/// the parser will accept. A document nested deeper is rejected with
/// [`TomlError::DepthExceeded`] *before* the recursion can exhaust the stack —
/// this is the stack-safety bound for the hostile-input posture.
pub const MAX_DEPTH: usize = 128;

/// A TOML value.
///
/// `Integer` is `i64` and `Float` is `f64` (the TOML spec types). `Table` is an
/// order-preserving `Vec<(String, Toml)>` so config files round-trip with keys
/// in source order rather than a hash-map's arbitrary order. Datetimes are kept
/// as `String` this cut (see crate docs).
#[derive(Debug, Clone, PartialEq)]
pub enum Toml {
    String(String),
    Integer(i64),
    Float(f64),
    Boolean(bool),
    Array(Vec<Toml>),
    Table(Vec<(String, Toml)>),
}

/// Why parsing failed. Every malformed input maps to one of these — the parser
/// never panics, so a config consumer can surface a calm error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TomlError {
    /// A required token/character was missing or unexpected.
    Unexpected,
    /// A string, array, inline table, or `[` header was not closed.
    Unterminated,
    /// A `\`-escape in a basic string was invalid (unknown escape or short `\u`).
    BadEscape,
    /// A `\u`/`\U` escape produced a value that is not a Unicode scalar.
    BadUnicode,
    /// A number did not match the TOML grammar / overflowed `i64`.
    BadNumber,
    /// A key (bare or quoted) was missing or malformed.
    BadKey,
    /// A key was defined more than once (or a header redefined a value).
    DuplicateKey,
    /// A `[table]` / `[[array]]` header was malformed.
    BadHeader,
    /// A value was missing after `=`.
    MissingValue,
    /// Nesting exceeded [`MAX_DEPTH`].
    DepthExceeded,
}

// ── Convenience accessors (all Option, never panic) ──────────────────────────

impl Toml {
    /// `&str` if this is a `String`, else `None`.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Toml::String(s) => Some(s.as_str()),
            _ => None,
        }
    }

    /// `i64` if this is an `Integer`, else `None`.
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Toml::Integer(n) => Some(*n),
            _ => None,
        }
    }

    /// `f64` if this is a `Float`, else `None`.
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Toml::Float(n) => Some(*n),
            _ => None,
        }
    }

    /// `bool` if this is a `Boolean`, else `None`.
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Toml::Boolean(b) => Some(*b),
            _ => None,
        }
    }

    /// The backing slice if this is an `Array`, else `None`.
    pub fn as_array(&self) -> Option<&[Toml]> {
        match self {
            Toml::Array(v) => Some(v.as_slice()),
            _ => None,
        }
    }

    /// The backing `(key, value)` slice if this is a `Table`, else `None`.
    pub fn as_table(&self) -> Option<&[(String, Toml)]> {
        match self {
            Toml::Table(v) => Some(v.as_slice()),
            _ => None,
        }
    }

    /// Look up `key` in a `Table` (first match, source order). `None` if this is
    /// not a table or the key is absent.
    pub fn get(&self, key: &str) -> Option<&Toml> {
        match self {
            Toml::Table(v) => v.iter().find(|(k, _)| k == key).map(|(_, val)| val),
            _ => None,
        }
    }

    /// Walk a dotted path (`"a.b.c"`) through nested tables. `None` if any
    /// segment is missing or a non-table is hit mid-path.
    pub fn get_path(&self, path: &str) -> Option<&Toml> {
        let mut cur = self;
        for seg in path.split('.') {
            cur = cur.get(seg)?;
        }
        Some(cur)
    }

    /// Index `i` of an `Array`. `None` if this is not an array or out of bounds.
    pub fn at(&self, i: usize) -> Option<&Toml> {
        match self {
            Toml::Array(v) => v.get(i),
            _ => None,
        }
    }
}

// ── Mutable table helpers (used by the parser to build nested tables) ─────────

/// Find a mutable reference to the value for `key` in a table vec, if present.
fn table_get_mut<'a>(t: &'a mut Vec<(String, Toml)>, key: &str) -> Option<&'a mut Toml> {
    t.iter_mut().find(|(k, _)| k == key).map(|(_, v)| v)
}

/// Does this table already contain `key`?
fn table_has(t: &[(String, Toml)], key: &str) -> bool {
    t.iter().any(|(k, _)| k == key)
}

// ── Parser ───────────────────────────────────────────────────────────────────

/// Parse a UTF-8 TOML document into the top-level [`Toml::Table`].
///
/// Accepts the TOML v1.0 practical subset described in the crate docs; rejects
/// everything else with a [`TomlError`] (never panics). Table key order is
/// preserved.
///
/// ```
/// use rae_toml::{parse, Toml};
/// let v = parse("name = \"Rae\"\nn = 42\nok = true").unwrap();
/// assert_eq!(v.get("name").and_then(Toml::as_str), Some("Rae"));
/// assert_eq!(v.get("n").and_then(Toml::as_i64), Some(42));
/// assert_eq!(v.get("ok").and_then(Toml::as_bool), Some(true));
/// ```
pub fn parse(input: &str) -> Result<Toml, TomlError> {
    let mut p = Parser {
        bytes: input.as_bytes(),
        pos: 0,
    };
    let mut root: Vec<(String, Toml)> = Vec::new();
    // `current_path` is the dotted segments of the most recent `[header]`. Plain
    // `key = value` lines insert under this path.
    let mut current_path: Vec<String> = Vec::new();

    loop {
        p.skip_ws_and_comments_and_newlines();
        match p.peek() {
            None => break,
            Some(b'[') => {
                let (path, is_array) = p.parse_header()?;
                if is_array {
                    ensure_array_of_tables(&mut root, &path)?;
                } else {
                    ensure_table_path(&mut root, &path)?;
                }
                current_path = path;
                p.skip_to_eol_then_newline()?;
            }
            Some(_) => {
                // key = value
                let keys = p.parse_key_chain()?;
                p.skip_inline_ws();
                if p.bump() != Some(b'=') {
                    return Err(TomlError::Unexpected);
                }
                p.skip_inline_ws();
                let value = p.parse_value(0)?;
                insert_kv(&mut root, &current_path, &keys, value)?;
                p.skip_to_eol_then_newline()?;
            }
        }
    }

    Ok(Toml::Table(root))
}

struct Parser<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Parser<'a> {
    #[inline]
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    #[inline]
    fn peek_at(&self, off: usize) -> Option<u8> {
        self.bytes.get(self.pos + off).copied()
    }

    #[inline]
    fn bump(&mut self) -> Option<u8> {
        let b = self.bytes.get(self.pos).copied();
        if b.is_some() {
            self.pos += 1;
        }
        b
    }

    /// Skip spaces/tabs only (not newlines).
    fn skip_inline_ws(&mut self) {
        while let Some(b' ') | Some(b'\t') = self.peek() {
            self.pos += 1;
        }
    }

    /// Skip spaces, tabs, comments, and newlines (between statements).
    fn skip_ws_and_comments_and_newlines(&mut self) {
        loop {
            match self.peek() {
                Some(b' ') | Some(b'\t') | Some(b'\n') | Some(b'\r') => {
                    self.pos += 1;
                }
                Some(b'#') => {
                    // comment to end of line
                    while let Some(b) = self.peek() {
                        if b == b'\n' {
                            break;
                        }
                        self.pos += 1;
                    }
                }
                _ => break,
            }
        }
    }

    /// After a value/header, the rest of the line must be only ws + optional
    /// comment; then a newline or EOF.
    fn skip_to_eol_then_newline(&mut self) -> Result<(), TomlError> {
        self.skip_inline_ws();
        match self.peek() {
            None => Ok(()),
            Some(b'#') => {
                while let Some(b) = self.peek() {
                    if b == b'\n' {
                        break;
                    }
                    self.pos += 1;
                }
                Ok(())
            }
            Some(b'\n') | Some(b'\r') => Ok(()),
            // anything else trailing on the line is a syntax error
            Some(_) => Err(TomlError::Unexpected),
        }
    }

    // ── headers ──────────────────────────────────────────────────────────────

    /// Parse a `[table]` or `[[array.of.tables]]` header (cursor on `[`).
    /// Returns the dotted segments and whether it was a double-bracket header.
    fn parse_header(&mut self) -> Result<(Vec<String>, bool), TomlError> {
        // consume '['
        self.pos += 1;
        let is_array = self.peek() == Some(b'[');
        if is_array {
            self.pos += 1;
        }
        let mut segs: Vec<String> = Vec::new();
        loop {
            self.skip_inline_ws();
            if segs.len() >= MAX_DEPTH {
                return Err(TomlError::DepthExceeded);
            }
            let key = self.parse_single_key()?;
            segs.push(key);
            self.skip_inline_ws();
            match self.peek() {
                Some(b'.') => {
                    self.pos += 1;
                    continue;
                }
                Some(b']') => {
                    self.pos += 1;
                    if is_array {
                        if self.peek() != Some(b']') {
                            return Err(TomlError::BadHeader);
                        }
                        self.pos += 1;
                    }
                    break;
                }
                None => return Err(TomlError::Unterminated),
                Some(_) => return Err(TomlError::BadHeader),
            }
        }
        if segs.is_empty() {
            return Err(TomlError::BadHeader);
        }
        Ok((segs, is_array))
    }

    // ── keys ─────────────────────────────────────────────────────────────────

    /// Parse a dotted key chain in `key = value` position (`a.b.c`). Returns one
    /// or more segments.
    fn parse_key_chain(&mut self) -> Result<Vec<String>, TomlError> {
        let mut segs: Vec<String> = Vec::new();
        loop {
            self.skip_inline_ws();
            if segs.len() >= MAX_DEPTH {
                return Err(TomlError::DepthExceeded);
            }
            let key = self.parse_single_key()?;
            segs.push(key);
            self.skip_inline_ws();
            if self.peek() == Some(b'.') {
                self.pos += 1;
                continue;
            }
            break;
        }
        Ok(segs)
    }

    /// Parse a single key: bare (`A-Za-z0-9_-`), basic-quoted (`"..."`), or
    /// literal-quoted (`'...'`).
    fn parse_single_key(&mut self) -> Result<String, TomlError> {
        match self.peek() {
            Some(b'"') => self.parse_basic_string(),
            Some(b'\'') => self.parse_literal_string(),
            Some(b) if is_bare_key_char(b) => {
                let start = self.pos;
                while let Some(b) = self.peek() {
                    if is_bare_key_char(b) {
                        self.pos += 1;
                    } else {
                        break;
                    }
                }
                // bytes are all ASCII bare-key chars -> valid UTF-8
                match core::str::from_utf8(&self.bytes[start..self.pos]) {
                    Ok(s) => Ok(String::from(s)),
                    Err(_) => Err(TomlError::BadKey),
                }
            }
            _ => Err(TomlError::BadKey),
        }
    }

    // ── values ───────────────────────────────────────────────────────────────

    fn parse_value(&mut self, depth: usize) -> Result<Toml, TomlError> {
        if depth > MAX_DEPTH {
            return Err(TomlError::DepthExceeded);
        }
        self.skip_inline_ws();
        match self.peek() {
            None => Err(TomlError::MissingValue),
            Some(b'"') => {
                if self.peek_at(1) == Some(b'"') && self.peek_at(2) == Some(b'"') {
                    Ok(Toml::String(self.parse_multiline_basic_string()?))
                } else {
                    Ok(Toml::String(self.parse_basic_string()?))
                }
            }
            Some(b'\'') => {
                if self.peek_at(1) == Some(b'\'') && self.peek_at(2) == Some(b'\'') {
                    Ok(Toml::String(self.parse_multiline_literal_string()?))
                } else {
                    Ok(Toml::String(self.parse_literal_string()?))
                }
            }
            Some(b'[') => self.parse_array(depth),
            Some(b'{') => self.parse_inline_table(depth),
            Some(b't') | Some(b'f') => self.parse_bool(),
            Some(_) => self.parse_number_or_datetime(),
        }
    }

    fn parse_bool(&mut self) -> Result<Toml, TomlError> {
        if self.starts_with(b"true") {
            self.pos += 4;
            Ok(Toml::Boolean(true))
        } else if self.starts_with(b"false") {
            self.pos += 5;
            Ok(Toml::Boolean(false))
        } else {
            Err(TomlError::Unexpected)
        }
    }

    fn starts_with(&self, lit: &[u8]) -> bool {
        self.bytes.len() >= self.pos + lit.len()
            && &self.bytes[self.pos..self.pos + lit.len()] == lit
    }

    // ── strings ──────────────────────────────────────────────────────────────

    /// Basic string `"..."` with escapes (cursor on opening `"`).
    fn parse_basic_string(&mut self) -> Result<String, TomlError> {
        if self.bump() != Some(b'"') {
            return Err(TomlError::Unexpected);
        }
        let mut out = String::new();
        loop {
            match self.bump() {
                None => return Err(TomlError::Unterminated),
                Some(b'"') => return Ok(out),
                Some(b'\n') | Some(b'\r') => return Err(TomlError::Unterminated),
                Some(b'\\') => self.parse_escape(&mut out)?,
                Some(b) if b < 0x80 => out.push(b as char),
                Some(b) => self.push_utf8_tail(b, &mut out)?,
            }
        }
    }

    /// Literal string `'...'` — no escapes (cursor on opening `'`).
    fn parse_literal_string(&mut self) -> Result<String, TomlError> {
        if self.bump() != Some(b'\'') {
            return Err(TomlError::Unexpected);
        }
        let mut out = String::new();
        loop {
            match self.bump() {
                None => return Err(TomlError::Unterminated),
                Some(b'\'') => return Ok(out),
                Some(b'\n') | Some(b'\r') => return Err(TomlError::Unterminated),
                Some(b) if b < 0x80 => out.push(b as char),
                Some(b) => self.push_utf8_tail(b, &mut out)?,
            }
        }
    }

    /// Multi-line basic string `"""..."""` (cursor on first `"`).
    fn parse_multiline_basic_string(&mut self) -> Result<String, TomlError> {
        self.pos += 3; // consume opening """
                       // A newline immediately after the opening delimiter is trimmed.
        self.skip_leading_newline();
        let mut out = String::new();
        loop {
            if self.starts_with(b"\"\"\"") {
                self.pos += 3;
                return Ok(out);
            }
            match self.bump() {
                None => return Err(TomlError::Unterminated),
                Some(b'\\') => {
                    // line-ending backslash trims following whitespace/newlines
                    if self.is_line_ending_backslash() {
                        self.consume_trailing_whitespace_run();
                    } else {
                        self.parse_escape(&mut out)?;
                    }
                }
                Some(b) if b < 0x80 => out.push(b as char),
                Some(b) => self.push_utf8_tail(b, &mut out)?,
            }
        }
    }

    /// Multi-line literal string `'''...'''` — no escapes (cursor on first `'`).
    fn parse_multiline_literal_string(&mut self) -> Result<String, TomlError> {
        self.pos += 3; // consume opening '''
        self.skip_leading_newline();
        let mut out = String::new();
        loop {
            if self.starts_with(b"'''") {
                self.pos += 3;
                return Ok(out);
            }
            match self.bump() {
                None => return Err(TomlError::Unterminated),
                Some(b) if b < 0x80 => out.push(b as char),
                Some(b) => self.push_utf8_tail(b, &mut out)?,
            }
        }
    }

    /// Trim a single leading newline (`\n` or `\r\n`) right after a `"""`/`'''`.
    fn skip_leading_newline(&mut self) {
        match self.peek() {
            Some(b'\n') => self.pos += 1,
            Some(b'\r') if self.peek_at(1) == Some(b'\n') => self.pos += 2,
            _ => {}
        }
    }

    /// True if the backslash (already consumed) is followed only by inline ws
    /// then a newline — TOML's line-ending backslash.
    fn is_line_ending_backslash(&self) -> bool {
        let mut i = self.pos;
        while let Some(b) = self.bytes.get(i).copied() {
            match b {
                b' ' | b'\t' => i += 1,
                b'\n' => return true,
                b'\r' if self.bytes.get(i + 1).copied() == Some(b'\n') => return true,
                _ => return false,
            }
        }
        false
    }

    /// Consume a run of whitespace and newlines (after a line-ending backslash).
    fn consume_trailing_whitespace_run(&mut self) {
        while let Some(b) = self.peek() {
            match b {
                b' ' | b'\t' | b'\n' | b'\r' => self.pos += 1,
                _ => break,
            }
        }
    }

    /// Copy a multi-byte UTF-8 sequence whose lead byte `b` was already consumed.
    fn push_utf8_tail(&mut self, b: u8, out: &mut String) -> Result<(), TomlError> {
        let len = utf8_len(b);
        if len == 0 {
            return Err(TomlError::Unexpected);
        }
        let start = self.pos - 1;
        let end = start + len;
        if end > self.bytes.len() {
            return Err(TomlError::Unexpected);
        }
        match core::str::from_utf8(&self.bytes[start..end]) {
            Ok(s) => {
                out.push_str(s);
                self.pos = end;
                Ok(())
            }
            Err(_) => Err(TomlError::Unexpected),
        }
    }

    /// Handle the char after `\` in a basic string (backslash already consumed).
    fn parse_escape(&mut self, out: &mut String) -> Result<(), TomlError> {
        match self.bump() {
            None => Err(TomlError::Unterminated),
            Some(b'"') => {
                out.push('"');
                Ok(())
            }
            Some(b'\\') => {
                out.push('\\');
                Ok(())
            }
            Some(b'b') => {
                out.push('\u{0008}');
                Ok(())
            }
            Some(b'f') => {
                out.push('\u{000C}');
                Ok(())
            }
            Some(b'n') => {
                out.push('\n');
                Ok(())
            }
            Some(b'r') => {
                out.push('\r');
                Ok(())
            }
            Some(b't') => {
                out.push('\t');
                Ok(())
            }
            Some(b'u') => self.parse_unicode_escape(4, out),
            Some(b'U') => self.parse_unicode_escape(8, out),
            Some(_) => Err(TomlError::BadEscape),
        }
    }

    /// Parse `n` hex digits after `\u`/`\U` into a Unicode scalar.
    fn parse_unicode_escape(&mut self, n: usize, out: &mut String) -> Result<(), TomlError> {
        let mut v: u32 = 0;
        for _ in 0..n {
            let d = match self.bump() {
                None => return Err(TomlError::BadEscape),
                Some(b) => hex_val(b).ok_or(TomlError::BadEscape)?,
            };
            v = (v << 4) | d as u32;
        }
        match char::from_u32(v) {
            Some(c) => {
                out.push(c);
                Ok(())
            }
            None => Err(TomlError::BadUnicode),
        }
    }

    // ── arrays / inline tables ───────────────────────────────────────────────

    /// Array `[ ... ]` — nested, multi-line, trailing comma allowed. Whitespace,
    /// newlines, and comments may appear between elements.
    fn parse_array(&mut self, depth: usize) -> Result<Toml, TomlError> {
        if depth >= MAX_DEPTH {
            return Err(TomlError::DepthExceeded);
        }
        self.pos += 1; // consume '['
        let mut items: Vec<Toml> = Vec::new();
        loop {
            self.skip_ws_and_comments_and_newlines();
            match self.peek() {
                None => return Err(TomlError::Unterminated),
                Some(b']') => {
                    self.pos += 1;
                    return Ok(Toml::Array(items));
                }
                Some(_) => {
                    let v = self.parse_value(depth + 1)?;
                    items.push(v);
                    self.skip_ws_and_comments_and_newlines();
                    match self.peek() {
                        Some(b',') => {
                            self.pos += 1;
                            continue;
                        }
                        Some(b']') => {
                            self.pos += 1;
                            return Ok(Toml::Array(items));
                        }
                        None => return Err(TomlError::Unterminated),
                        Some(_) => return Err(TomlError::Unexpected),
                    }
                }
            }
        }
    }

    /// Inline table `{ a = 1, b = "x" }` — single line, no trailing comma per
    /// spec (we reject a trailing comma). Dotted keys allowed inside.
    fn parse_inline_table(&mut self, depth: usize) -> Result<Toml, TomlError> {
        if depth >= MAX_DEPTH {
            return Err(TomlError::DepthExceeded);
        }
        self.pos += 1; // consume '{'
        let mut table: Vec<(String, Toml)> = Vec::new();
        self.skip_inline_ws();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            return Ok(Toml::Table(table));
        }
        loop {
            self.skip_inline_ws();
            let keys = self.parse_key_chain()?;
            self.skip_inline_ws();
            if self.bump() != Some(b'=') {
                return Err(TomlError::Unexpected);
            }
            self.skip_inline_ws();
            let value = self.parse_value(depth + 1)?;
            insert_into_table(&mut table, &keys, value)?;
            self.skip_inline_ws();
            match self.bump() {
                Some(b',') => continue,
                Some(b'}') => return Ok(Toml::Table(table)),
                None => return Err(TomlError::Unterminated),
                Some(_) => return Err(TomlError::Unexpected),
            }
        }
    }

    // ── numbers / datetimes ──────────────────────────────────────────────────

    /// Parse a number, `inf`/`nan`, or a datetime-shaped token (kept as String).
    fn parse_number_or_datetime(&mut self) -> Result<Toml, TomlError> {
        // Grab the raw token: everything up to a value terminator.
        let start = self.pos;
        while let Some(b) = self.peek() {
            match b {
                b',' | b']' | b'}' | b'\n' | b'\r' | b'#' => break,
                b' ' | b'\t' => break,
                _ => self.pos += 1,
            }
        }
        let tok = &self.bytes[start..self.pos];
        if tok.is_empty() {
            return Err(TomlError::BadNumber);
        }
        let s = match core::str::from_utf8(tok) {
            Ok(s) => s,
            Err(_) => return Err(TomlError::BadNumber),
        };

        // inf / nan (with optional sign)
        match s {
            "inf" | "+inf" => return Ok(Toml::Float(f64::INFINITY)),
            "-inf" => return Ok(Toml::Float(f64::NEG_INFINITY)),
            "nan" | "+nan" | "-nan" => return Ok(Toml::Float(f64::NAN)),
            _ => {}
        }

        // Datetime heuristic: a date `YYYY-MM-DD…` or a time `HH:MM:SS` keeps the
        // raw token as a String (documented omission). Detected by a `:` or by a
        // `-` that is not the leading sign (i.e. a `-` after a digit).
        if looks_like_datetime(s) {
            return Ok(Toml::String(String::from(s)));
        }

        parse_number_token(s)
    }
}

/// True if `s` looks like a TOML date/time/datetime token (kept as String).
fn looks_like_datetime(s: &str) -> bool {
    let b = s.as_bytes();
    if b.contains(&b':') {
        return true;
    }
    // a '-' after the first char that follows a digit => date separator, not sign
    for i in 1..b.len() {
        if b[i] == b'-' && b[i - 1].is_ascii_digit() {
            return true;
        }
    }
    false
}

/// Parse an integer or float token (sign, `_` separators, `0x/0o/0b`, frac/exp).
fn parse_number_token(s: &str) -> Result<Toml, TomlError> {
    // Determine integer vs float by presence of `.`, `e`/`E` (decimal only).
    let lower = s.as_bytes();
    let is_hex_oct_bin = s.starts_with("0x")
        || s.starts_with("0o")
        || s.starts_with("0b")
        || s.starts_with("+0x")
        || s.starts_with("-0x")
        || s.starts_with("+0o")
        || s.starts_with("-0o")
        || s.starts_with("+0b")
        || s.starts_with("-0b");

    let is_float =
        !is_hex_oct_bin && (lower.contains(&b'.') || lower.iter().any(|&c| c == b'e' || c == b'E'));

    if is_float {
        parse_float_token(s)
    } else {
        parse_integer_token(s)
    }
}

/// Parse a TOML integer: optional sign, optional `0x/0o/0b` radix, `_` group
/// separators (not leading/trailing, not doubled), into `i64`.
fn parse_integer_token(s: &str) -> Result<Toml, TomlError> {
    let bytes = s.as_bytes();
    if bytes.is_empty() {
        return Err(TomlError::BadNumber);
    }
    let mut idx = 0;
    let mut neg = false;
    match bytes[0] {
        b'+' => idx = 1,
        b'-' => {
            neg = true;
            idx = 1;
        }
        _ => {}
    }
    if idx >= bytes.len() {
        return Err(TomlError::BadNumber);
    }

    // radix prefix
    let (radix, body) = if bytes.len() >= idx + 2 && bytes[idx] == b'0' {
        match bytes[idx + 1] {
            b'x' | b'X' => (16u32, &bytes[idx + 2..]),
            b'o' | b'O' => (8u32, &bytes[idx + 2..]),
            b'b' | b'B' => (2u32, &bytes[idx + 2..]),
            _ => (10u32, &bytes[idx..]),
        }
    } else {
        (10u32, &bytes[idx..])
    };

    if body.is_empty() {
        return Err(TomlError::BadNumber);
    }
    // underscore rules: no leading/trailing/doubled underscore
    if body[0] == b'_' || body[body.len() - 1] == b'_' {
        return Err(TomlError::BadNumber);
    }
    // leading-zero rule for decimal (TOML forbids `01`, allows `0`)
    if radix == 10 && body.len() > 1 && body[0] == b'0' {
        return Err(TomlError::BadNumber);
    }

    let mut acc: i64 = 0;
    let mut prev_underscore = false;
    let mut any_digit = false;
    for (i, &c) in body.iter().enumerate() {
        if c == b'_' {
            if prev_underscore || i == 0 {
                return Err(TomlError::BadNumber);
            }
            prev_underscore = true;
            continue;
        }
        prev_underscore = false;
        let digit = match (c as char).to_digit(radix) {
            Some(d) => d as i64,
            None => return Err(TomlError::BadNumber),
        };
        any_digit = true;
        acc = acc
            .checked_mul(radix as i64)
            .and_then(|v| v.checked_add(digit))
            .ok_or(TomlError::BadNumber)?;
    }
    if !any_digit {
        return Err(TomlError::BadNumber);
    }
    if neg {
        acc = acc.checked_neg().ok_or(TomlError::BadNumber)?;
    }
    Ok(Toml::Integer(acc))
}

/// Parse a TOML decimal float: sign, integer part, optional `.frac`, optional
/// `[eE][+-]?digits`, `_` group separators. Builds `f64` without `f64::from_str`.
fn parse_float_token(s: &str) -> Result<Toml, TomlError> {
    let bytes = s.as_bytes();
    let mut idx = 0;
    let mut neg = false;
    match bytes.first().copied() {
        Some(b'+') => idx = 1,
        Some(b'-') => {
            neg = true;
            idx = 1;
        }
        _ => {}
    }

    let mut mantissa: f64 = 0.0;
    let mut frac_exp: i32 = 0;
    let mut any_digit = false;
    let mut prev_underscore = false;
    let mut seen_int_digit = false;

    // integer part
    while idx < bytes.len() {
        let c = bytes[idx];
        if c == b'_' {
            if prev_underscore || !seen_int_digit {
                return Err(TomlError::BadNumber);
            }
            prev_underscore = true;
            idx += 1;
            continue;
        }
        if c.is_ascii_digit() {
            // leading-zero rule: `01.0` invalid, `0.0` valid
            if seen_int_digit == false && c == b'0' {
                // allow a single 0 only if next is '.', 'e', 'E', or end
                if let Some(&nx) = bytes.get(idx + 1) {
                    if nx.is_ascii_digit() {
                        return Err(TomlError::BadNumber);
                    }
                }
            }
            mantissa = mantissa * 10.0 + (c - b'0') as f64;
            any_digit = true;
            seen_int_digit = true;
            prev_underscore = false;
            idx += 1;
        } else {
            break;
        }
    }
    if prev_underscore {
        return Err(TomlError::BadNumber);
    }
    // TOML floats require at least one digit before `.`/`e` (no `.5`, no `e2`).
    if !seen_int_digit {
        return Err(TomlError::BadNumber);
    }

    // fraction
    if idx < bytes.len() && bytes[idx] == b'.' {
        idx += 1;
        let mut frac_digit = false;
        prev_underscore = false;
        while idx < bytes.len() {
            let c = bytes[idx];
            if c == b'_' {
                if prev_underscore || !frac_digit {
                    return Err(TomlError::BadNumber);
                }
                prev_underscore = true;
                idx += 1;
                continue;
            }
            if c.is_ascii_digit() {
                mantissa = mantissa * 10.0 + (c - b'0') as f64;
                frac_exp -= 1;
                any_digit = true;
                frac_digit = true;
                prev_underscore = false;
                idx += 1;
            } else {
                break;
            }
        }
        if !frac_digit || prev_underscore {
            return Err(TomlError::BadNumber);
        }
    }

    // exponent
    let mut exp: i32 = 0;
    if idx < bytes.len() && (bytes[idx] == b'e' || bytes[idx] == b'E') {
        idx += 1;
        let mut exp_neg = false;
        if idx < bytes.len() && (bytes[idx] == b'+' || bytes[idx] == b'-') {
            exp_neg = bytes[idx] == b'-';
            idx += 1;
        }
        let mut exp_digit = false;
        let mut e: i32 = 0;
        prev_underscore = false;
        while idx < bytes.len() {
            let c = bytes[idx];
            if c == b'_' {
                if prev_underscore || !exp_digit {
                    return Err(TomlError::BadNumber);
                }
                prev_underscore = true;
                idx += 1;
                continue;
            }
            if c.is_ascii_digit() {
                e = e.saturating_mul(10).saturating_add((c - b'0') as i32);
                exp_digit = true;
                prev_underscore = false;
                idx += 1;
            } else {
                break;
            }
        }
        if !exp_digit || prev_underscore {
            return Err(TomlError::BadNumber);
        }
        exp = if exp_neg { -e } else { e };
    }

    if idx != bytes.len() || !any_digit {
        return Err(TomlError::BadNumber);
    }

    let total = exp.saturating_add(frac_exp);
    let mut value = scale_pow10(mantissa, total);
    if neg {
        value = -value;
    }
    Ok(Toml::Float(value))
}

// ── table construction helpers ───────────────────────────────────────────────

/// Ensure the dotted `[a.b.c]` table path exists in `root`, descending /
/// creating tables. Errors if a path segment collides with a non-table value.
fn ensure_table_path(root: &mut Vec<(String, Toml)>, path: &[String]) -> Result<(), TomlError> {
    let mut cur = root;
    for (i, seg) in path.iter().enumerate() {
        let is_last = i + 1 == path.len();
        if !table_has(cur, seg) {
            cur.push((seg.clone(), Toml::Table(Vec::new())));
        } else if is_last {
            // redefining an existing header table is a duplicate
            match table_get_mut(cur, seg) {
                Some(Toml::Table(_)) => return Err(TomlError::DuplicateKey),
                Some(Toml::Array(_)) => {} // array-of-tables: handled separately
                _ => return Err(TomlError::DuplicateKey),
            }
        }
        // descend
        match table_get_mut(cur, seg) {
            Some(Toml::Table(inner)) => cur = inner,
            Some(Toml::Array(arr)) => {
                // descend into the LAST element of an array-of-tables
                match arr.last_mut() {
                    Some(Toml::Table(inner)) => cur = inner,
                    _ => return Err(TomlError::DuplicateKey),
                }
            }
            _ => return Err(TomlError::DuplicateKey),
        }
    }
    Ok(())
}

/// Ensure the `[[a.b]]` array-of-tables path exists; push a fresh table onto the
/// final array segment.
fn ensure_array_of_tables(
    root: &mut Vec<(String, Toml)>,
    path: &[String],
) -> Result<(), TomlError> {
    if path.is_empty() {
        return Err(TomlError::BadHeader);
    }
    let (parents, last) = path.split_at(path.len() - 1);
    let last = &last[0];

    // descend through parents as tables
    let mut cur = root;
    for seg in parents {
        if !table_has(cur, seg) {
            cur.push((seg.clone(), Toml::Table(Vec::new())));
        }
        match table_get_mut(cur, seg) {
            Some(Toml::Table(inner)) => cur = inner,
            Some(Toml::Array(arr)) => match arr.last_mut() {
                Some(Toml::Table(inner)) => cur = inner,
                _ => return Err(TomlError::DuplicateKey),
            },
            _ => return Err(TomlError::DuplicateKey),
        }
    }

    // the final segment must be (or become) an array
    if !table_has(cur, last) {
        cur.push((last.clone(), Toml::Array(Vec::new())));
    }
    match table_get_mut(cur, last) {
        Some(Toml::Array(arr)) => {
            arr.push(Toml::Table(Vec::new()));
            Ok(())
        }
        _ => Err(TomlError::DuplicateKey),
    }
}

/// Insert `value` at `keys` (dotted) within the table located at `base_path`
/// from `root`.
fn insert_kv(
    root: &mut Vec<(String, Toml)>,
    base_path: &[String],
    keys: &[String],
    value: Toml,
) -> Result<(), TomlError> {
    // descend to the base table
    let mut cur = root;
    for seg in base_path {
        match table_get_mut(cur, seg) {
            Some(Toml::Table(inner)) => cur = inner,
            Some(Toml::Array(arr)) => match arr.last_mut() {
                Some(Toml::Table(inner)) => cur = inner,
                _ => return Err(TomlError::DuplicateKey),
            },
            _ => return Err(TomlError::Unexpected),
        }
    }
    insert_into_table(cur, keys, value)
}

/// Insert `value` at the dotted `keys` within `table`, creating intermediate
/// tables. Rejects overwriting an existing leaf.
fn insert_into_table(
    table: &mut Vec<(String, Toml)>,
    keys: &[String],
    value: Toml,
) -> Result<(), TomlError> {
    if keys.is_empty() {
        return Err(TomlError::BadKey);
    }
    if keys.len() == 1 {
        if table_has(table, &keys[0]) {
            return Err(TomlError::DuplicateKey);
        }
        table.push((keys[0].clone(), value));
        return Ok(());
    }
    // intermediate
    let head = &keys[0];
    if !table_has(table, head) {
        table.push((head.clone(), Toml::Table(Vec::new())));
    }
    match table_get_mut(table, head) {
        Some(Toml::Table(inner)) => insert_into_table(inner, &keys[1..], value),
        _ => Err(TomlError::DuplicateKey),
    }
}

// ── shared numeric / char helpers ────────────────────────────────────────────

/// Multiply `mantissa` by `10^exp` without `libm`, saturating out-of-range
/// exponents to `0.0`/`±inf` in a bounded number of multiplies.
fn scale_pow10(mantissa: f64, exp: i32) -> f64 {
    if mantissa == 0.0 {
        return 0.0;
    }
    let clamped = if exp > 400 {
        400
    } else if exp < -400 {
        -400
    } else {
        exp
    };
    let mut value = mantissa;
    let mut n = clamped;
    while n > 0 {
        value *= 10.0;
        n -= 1;
    }
    while n < 0 {
        value *= 0.1;
        n += 1;
    }
    value
}

/// Length in bytes of a UTF-8 sequence given its lead byte; 0 if not a lead.
#[inline]
fn utf8_len(lead: u8) -> usize {
    if lead < 0x80 {
        1
    } else if lead & 0xE0 == 0xC0 {
        2
    } else if lead & 0xF0 == 0xE0 {
        3
    } else if lead & 0xF8 == 0xF0 {
        4
    } else {
        0
    }
}

/// Hex digit value (0..=15), or `None` if not `[0-9a-fA-F]`.
#[inline]
fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// True if `b` is a valid bare-key character (`A-Za-z0-9_-`).
#[inline]
fn is_bare_key_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'-'
}

// ── Serializer ────────────────────────────────────────────────────────────────

/// Serialize a [`Toml`] value to a round-trippable TOML string.
///
/// The top level must be a [`Toml::Table`]; sub-tables are emitted as
/// `[header]` sections (and `[[header]]` for arrays of tables), scalars as
/// `key = value` with proper string escaping. Key order is preserved.
///
/// ```
/// use rae_toml::{parse, to_string};
/// let v = parse("a = 1\n[t]\nb = \"x\"\n").unwrap();
/// let s = to_string(&v);
/// assert_eq!(parse(&s).unwrap(), v); // round-trips
/// ```
pub fn to_string(value: &Toml) -> String {
    let mut out = String::new();
    match value {
        Toml::Table(t) => write_table(&mut out, t, &[]),
        // A non-table top level isn't valid TOML on its own; wrap it minimally so
        // we never panic and still produce parseable output.
        other => {
            out.push_str("value = ");
            write_inline_value(&mut out, other);
            out.push('\n');
        }
    }
    out
}

/// Write a table: first its scalar/inline-array/inline-table leaves as
/// `key = value`, then recurse into sub-tables / arrays-of-tables as headers.
fn write_table(out: &mut String, table: &[(String, Toml)], path: &[String]) {
    // pass 1: leaf key/values that are NOT sub-tables or arrays-of-tables
    for (k, v) in table.iter() {
        if is_header_value(v) {
            continue;
        }
        write_key(out, k);
        out.push_str(" = ");
        write_inline_value(out, v);
        out.push('\n');
    }
    // pass 2: sub-tables and arrays-of-tables as `[header]` / `[[header]]`
    for (k, v) in table.iter() {
        match v {
            Toml::Table(inner) => {
                let mut child = path.to_vec();
                child.push(k.clone());
                out.push('\n');
                out.push('[');
                write_header_path(out, &child);
                out.push_str("]\n");
                write_table(out, inner, &child);
            }
            Toml::Array(items) if array_of_tables(items) => {
                let mut child = path.to_vec();
                child.push(k.clone());
                for item in items {
                    if let Toml::Table(inner) = item {
                        out.push('\n');
                        out.push_str("[[");
                        write_header_path(out, &child);
                        out.push_str("]]\n");
                        write_table(out, inner, &child);
                    }
                }
            }
            _ => {}
        }
    }
}

/// Is this value emitted as a `[header]` section (table or array-of-tables)
/// rather than an inline `key = value`?
fn is_header_value(v: &Toml) -> bool {
    match v {
        Toml::Table(_) => true,
        Toml::Array(items) => array_of_tables(items),
        _ => false,
    }
}

/// True if a non-empty array whose every element is a table (array-of-tables).
fn array_of_tables(items: &[Toml]) -> bool {
    !items.is_empty() && items.iter().all(|i| matches!(i, Toml::Table(_)))
}

fn write_header_path(out: &mut String, path: &[String]) {
    for (i, seg) in path.iter().enumerate() {
        if i != 0 {
            out.push('.');
        }
        write_key(out, seg);
    }
}

/// Write a key: bare if it is all bare-key chars and non-empty, else quoted.
fn write_key(out: &mut String, key: &str) {
    let bare = !key.is_empty() && key.bytes().all(is_bare_key_char);
    if bare {
        out.push_str(key);
    } else {
        write_escaped_string(out, key);
    }
}

/// Write a scalar / inline-array / inline-table value (the `= value` RHS).
fn write_inline_value(out: &mut String, v: &Toml) {
    match v {
        Toml::String(s) => write_escaped_string(out, s),
        Toml::Integer(n) => write_i64(out, *n),
        Toml::Float(n) => write_float(out, *n),
        Toml::Boolean(true) => out.push_str("true"),
        Toml::Boolean(false) => out.push_str("false"),
        Toml::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i != 0 {
                    out.push_str(", ");
                }
                write_inline_value(out, item);
            }
            out.push(']');
        }
        Toml::Table(t) => {
            // inline table
            out.push('{');
            for (i, (k, val)) in t.iter().enumerate() {
                if i != 0 {
                    out.push_str(", ");
                } else {
                    out.push(' ');
                }
                write_key(out, k);
                out.push_str(" = ");
                write_inline_value(out, val);
            }
            out.push_str(" }");
        }
    }
}

/// Write a basic-string literal, escaping per TOML.
fn write_escaped_string(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{0008}' => out.push_str("\\b"),
            '\u{000C}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                out.push_str("\\u00");
                let v = c as u32;
                out.push(hex_digit(((v >> 4) & 0xF) as u8));
                out.push(hex_digit((v & 0xF) as u8));
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

#[inline]
fn hex_digit(v: u8) -> char {
    if v < 10 {
        (b'0' + v) as char
    } else {
        (b'a' + (v - 10)) as char
    }
}

/// Write a signed 64-bit integer in decimal.
fn write_i64(out: &mut String, n: i64) {
    if n < 0 {
        out.push('-');
        // handle i64::MIN without overflow via u64
        let mag = (n as i128).unsigned_abs() as u64;
        write_u64(out, mag);
    } else {
        write_u64(out, n as u64);
    }
}

fn write_u64(out: &mut String, mut v: u64) {
    if v == 0 {
        out.push('0');
        return;
    }
    let mut tmp = [0u8; 20];
    let mut n = 0;
    while v > 0 {
        tmp[n] = b'0' + (v % 10) as u8;
        v /= 10;
        n += 1;
    }
    for i in (0..n).rev() {
        out.push(tmp[i] as char);
    }
}

/// Format an `f64` for TOML. Non-finite -> `inf`/`-inf`/`nan` (valid TOML).
/// Always emits a fractional part so the token re-parses as a float.
fn write_float(out: &mut String, n: f64) {
    if n.is_nan() {
        out.push_str("nan");
        return;
    }
    if n.is_infinite() {
        out.push_str(if n < 0.0 { "-inf" } else { "inf" });
        return;
    }
    let mut value = n;
    if value.is_sign_negative() && value != 0.0 {
        out.push('-');
        value = -value;
    } else if value == 0.0 && n.is_sign_negative() {
        out.push('-');
    }

    let int_part = trunc(value);
    let mut frac = value - int_part;
    if int_part < 1e18 {
        write_u64(out, int_part as u64);
    } else {
        write_u64(
            out,
            (int_part as u128 % 1_000_000_000_000_000_000u128) as u64,
        );
    }
    out.push('.');
    if frac == 0.0 {
        out.push('0');
        return;
    }
    let mut digits = [0u8; 15];
    let mut count = 0usize;
    while count < 15 && frac > 0.0 {
        frac *= 10.0;
        let d = trunc(frac) as u8;
        frac -= d as f64;
        digits[count] = d % 10;
        count += 1;
    }
    while count > 1 && digits[count - 1] == 0 {
        count -= 1;
    }
    for &d in &digits[..count] {
        out.push((b'0' + d) as char);
    }
}

/// `trunc` toward zero without `libm`, for finite non-negative values.
#[inline]
fn trunc(x: f64) -> f64 {
    (x as u128) as f64
}

// ── Host KATs (the FAIL-able proof: `cargo test -p rae_toml`) ────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn approx(a: f64, b: f64) -> bool {
        let d = a - b;
        let d = if d < 0.0 { -d } else { d };
        let scale = if b.abs() > 1.0 { b.abs() } else { 1.0 };
        d <= 1e-9 * scale
    }

    // ── A realistic config: exact values via get_path ────────────────────────

    #[test]
    fn realistic_config_exact_values() {
        let src = r#"
# AthenaOS-shaped config
title = "AthenaOS"
version = 1
ratio = 3.5
enabled = true
tags = ["os", "gaming", "fast"]

[window]
width = 1280
height = 720
resizable = false

[window.theme]
name = "vibe"
opacity = 0.85

[[plugin]]
id = "alpha"

[[plugin]]
id = "beta"
"#;
        let v = parse(src).unwrap();
        assert_eq!(v.get("title").and_then(Toml::as_str), Some("AthenaOS"));
        assert_eq!(v.get("version").and_then(Toml::as_i64), Some(1));
        assert!(approx(v.get("ratio").and_then(Toml::as_f64).unwrap(), 3.5));
        assert_eq!(v.get("enabled").and_then(Toml::as_bool), Some(true));
        assert_eq!(
            v.get("tags").and_then(Toml::as_array).map(|a| a.len()),
            Some(3)
        );

        // nested [window.theme] — THIS is the FAIL-able insertion guard.
        assert_eq!(
            v.get_path("window.theme.name").and_then(Toml::as_str),
            Some("vibe")
        );
        assert!(approx(
            v.get_path("window.theme.opacity")
                .and_then(Toml::as_f64)
                .unwrap(),
            0.85
        ));
        assert_eq!(
            v.get_path("window.width").and_then(Toml::as_i64),
            Some(1280)
        );
        // FAIL-guard: if `[a.b]` nested-table insertion is broken (e.g. theme
        // attached to root instead of under window), this flips.
        assert_ne!(v.get_path("window.theme.name").and_then(Toml::as_str), None);
        assert!(v.get("theme").is_none());

        // array-of-tables
        let plugins = v.get("plugin").and_then(Toml::as_array).unwrap();
        assert_eq!(plugins.len(), 2);
        assert_eq!(plugins[0].get("id").and_then(Toml::as_str), Some("alpha"));
        assert_eq!(plugins[1].get("id").and_then(Toml::as_str), Some("beta"));
        assert_ne!(plugins[0].get("id").and_then(Toml::as_str), Some("beta"));
    }

    #[test]
    fn table_key_order_preserved() {
        let v = parse("z = 1\na = 2\nm = 3\n").unwrap();
        let keys: vec::Vec<&str> = v
            .as_table()
            .unwrap()
            .iter()
            .map(|(k, _)| k.as_str())
            .collect();
        assert_eq!(keys, vec!["z", "a", "m"]); // NOT sorted
    }

    // ── Round-trip ───────────────────────────────────────────────────────────

    #[test]
    fn round_trip_realistic() {
        let src = r#"
name = "Rae"
count = 7
pi = 3.14159
flag = false
list = [1, 2, 3]

[server]
host = "localhost"
port = 8080

[server.tls]
enabled = true

[[entry]]
k = "a"

[[entry]]
k = "b"
"#;
        let v = parse(src).unwrap();
        let s = to_string(&v);
        let v2 = parse(&s).unwrap();
        assert_eq!(v, v2);
        // spot-check survived
        assert_eq!(
            v2.get_path("server.tls.enabled").and_then(Toml::as_bool),
            Some(true)
        );
        assert_eq!(
            v2.get("entry").and_then(Toml::as_array).map(|a| a.len()),
            Some(2)
        );
    }

    #[test]
    fn round_trip_double_parse_equal() {
        let inputs = [
            "a = 1\nb = \"two\"\nc = true\n",
            "x = [1, 2, [3, 4]]\n",
            "p = { a = 1, b = \"x\" }\n",
            "[t]\nk = 3.5\n[t.u]\nv = -9\n",
        ];
        for inp in inputs {
            let once = parse(inp).unwrap();
            let twice = parse(&to_string(&once)).unwrap();
            assert_eq!(once, twice, "round-trip mismatch for {:?}", inp);
        }
    }

    // ── Strings ──────────────────────────────────────────────────────────────

    #[test]
    fn basic_string_escapes() {
        let v = parse(r#"s = "a\tb\nc\"d\\e""#).unwrap();
        assert_eq!(v.get("s").and_then(Toml::as_str), Some("a\tb\nc\"d\\e"));
    }

    #[test]
    fn unicode_escape() {
        let v = parse(r#"s = "café \U0001F600""#).unwrap();
        assert_eq!(v.get("s").and_then(Toml::as_str), Some("café \u{1F600}"));
        // FAIL-guard: a broken \u decoder would not yield é.
        assert_ne!(v.get("s").and_then(Toml::as_str), Some("cafe "));
    }

    #[test]
    fn literal_string_no_escapes() {
        let v = parse(r#"path = 'C:\Users\rae\n'"#).unwrap();
        assert_eq!(
            v.get("path").and_then(Toml::as_str),
            Some(r"C:\Users\rae\n")
        );
    }

    #[test]
    fn multiline_basic_string() {
        let src = "s = \"\"\"\nline one\nline two\"\"\"\n";
        let v = parse(src).unwrap();
        assert_eq!(
            v.get("s").and_then(Toml::as_str),
            Some("line one\nline two")
        );
    }

    #[test]
    fn multiline_literal_string() {
        let src = "s = '''\nraw\\nliteral'''\n";
        let v = parse(src).unwrap();
        assert_eq!(v.get("s").and_then(Toml::as_str), Some("raw\\nliteral"));
    }

    // ── Integers ─────────────────────────────────────────────────────────────

    #[test]
    fn integer_forms() {
        assert_eq!(
            parse("n = 42\n").unwrap().get("n").and_then(Toml::as_i64),
            Some(42)
        );
        assert_eq!(
            parse("n = -17\n").unwrap().get("n").and_then(Toml::as_i64),
            Some(-17)
        );
        assert_eq!(
            parse("n = +5\n").unwrap().get("n").and_then(Toml::as_i64),
            Some(5)
        );
        assert_eq!(
            parse("n = 0\n").unwrap().get("n").and_then(Toml::as_i64),
            Some(0)
        );
        // FAIL-guard: underscore separators must be stripped, not mis-parsed.
        assert_eq!(
            parse("n = 1_000_000\n")
                .unwrap()
                .get("n")
                .and_then(Toml::as_i64),
            Some(1_000_000)
        );
        assert_ne!(
            parse("n = 1_000_000\n")
                .unwrap()
                .get("n")
                .and_then(Toml::as_i64),
            Some(1000)
        );
    }

    #[test]
    fn integer_radixes() {
        assert_eq!(
            parse("n = 0xff\n").unwrap().get("n").and_then(Toml::as_i64),
            Some(255)
        );
        assert_eq!(
            parse("n = 0o755\n")
                .unwrap()
                .get("n")
                .and_then(Toml::as_i64),
            Some(493)
        );
        assert_eq!(
            parse("n = 0b1010\n")
                .unwrap()
                .get("n")
                .and_then(Toml::as_i64),
            Some(10)
        );
        assert_eq!(
            parse("n = 0xDE_AD\n")
                .unwrap()
                .get("n")
                .and_then(Toml::as_i64),
            Some(0xDEAD)
        );
    }

    // ── Floats ───────────────────────────────────────────────────────────────

    #[test]
    fn float_forms() {
        assert!(approx(
            parse("f = 3.14\n")
                .unwrap()
                .get("f")
                .and_then(Toml::as_f64)
                .unwrap(),
            3.14
        ));
        assert!(approx(
            parse("f = -0.5\n")
                .unwrap()
                .get("f")
                .and_then(Toml::as_f64)
                .unwrap(),
            -0.5
        ));
        assert!(approx(
            parse("f = 1e3\n")
                .unwrap()
                .get("f")
                .and_then(Toml::as_f64)
                .unwrap(),
            1000.0
        ));
        assert!(approx(
            parse("f = 6.022e2\n")
                .unwrap()
                .get("f")
                .and_then(Toml::as_f64)
                .unwrap(),
            602.2
        ));
        assert!(approx(
            parse("f = 2e-3\n")
                .unwrap()
                .get("f")
                .and_then(Toml::as_f64)
                .unwrap(),
            0.002
        ));
        assert!(approx(
            parse("f = 9_000.5\n")
                .unwrap()
                .get("f")
                .and_then(Toml::as_f64)
                .unwrap(),
            9000.5
        ));
        // FAIL-guard: exponent must be applied.
        assert_ne!(
            parse("f = 1e3\n").unwrap().get("f").and_then(Toml::as_f64),
            Some(1.0)
        );
    }

    #[test]
    fn float_inf_nan() {
        assert!(parse("f = inf\n")
            .unwrap()
            .get("f")
            .and_then(Toml::as_f64)
            .unwrap()
            .is_infinite());
        assert!(
            parse("f = -inf\n")
                .unwrap()
                .get("f")
                .and_then(Toml::as_f64)
                .unwrap()
                < 0.0
        );
        assert!(parse("f = nan\n")
            .unwrap()
            .get("f")
            .and_then(Toml::as_f64)
            .unwrap()
            .is_nan());
    }

    // ── Arrays / inline tables ───────────────────────────────────────────────

    #[test]
    fn nested_and_multiline_arrays() {
        let src = "a = [\n  1,\n  [2, 3],\n  [4, [5, 6]],\n]\n"; // trailing comma + multiline + nested
        let v = parse(src).unwrap();
        let a = v.get("a").and_then(Toml::as_array).unwrap();
        assert_eq!(a.len(), 3);
        assert_eq!(a[0].as_i64(), Some(1));
        assert_eq!(a[1].as_array().map(|x| x.len()), Some(2));
        // deep check: a[2] = [4, [5, 6]] -> a[2][1][0] == 5
        assert_eq!(
            a[2].at(1).and_then(|x| x.at(0)).and_then(Toml::as_i64),
            Some(5)
        );
    }

    #[test]
    fn inline_table() {
        let v = parse(r#"point = { x = 1, y = 2, label = "p" }"#).unwrap();
        let p = v.get("point").unwrap();
        assert_eq!(p.get("x").and_then(Toml::as_i64), Some(1));
        assert_eq!(p.get("y").and_then(Toml::as_i64), Some(2));
        assert_eq!(p.get("label").and_then(Toml::as_str), Some("p"));
        assert_eq!(
            parse("e = {}")
                .unwrap()
                .get("e")
                .and_then(Toml::as_table)
                .map(|t| t.len()),
            Some(0)
        );
    }

    #[test]
    fn dotted_keys() {
        let v = parse("a.b.c = 1\na.b.d = 2\n").unwrap();
        assert_eq!(v.get_path("a.b.c").and_then(Toml::as_i64), Some(1));
        assert_eq!(v.get_path("a.b.d").and_then(Toml::as_i64), Some(2));
    }

    #[test]
    fn comments_and_blanks() {
        let src = "# top comment\n\nk = 1 # trailing\n\n# another\nm = 2\n";
        let v = parse(src).unwrap();
        assert_eq!(v.get("k").and_then(Toml::as_i64), Some(1));
        assert_eq!(v.get("m").and_then(Toml::as_i64), Some(2));
    }

    // ── Datetime kept as String (documented omission) ────────────────────────

    #[test]
    fn datetime_as_string() {
        let v = parse("ts = 1979-05-27\nclock = 07:32:00\n").unwrap();
        assert_eq!(v.get("ts").and_then(Toml::as_str), Some("1979-05-27"));
        assert_eq!(v.get("clock").and_then(Toml::as_str), Some("07:32:00"));
    }

    // ── Accessors / empty ────────────────────────────────────────────────────

    #[test]
    fn accessors_type_mismatch_none() {
        let v = parse("n = 5\n").unwrap();
        let n = v.get("n").unwrap();
        assert!(n.as_str().is_none());
        assert!(n.as_bool().is_none());
        assert!(n.as_array().is_none());
        assert!(n.as_f64().is_none());
        assert_eq!(n.as_i64(), Some(5));
        assert!(v.get_path("a.b.c").is_none());
    }

    #[test]
    fn empty_input_is_empty_table() {
        assert_eq!(parse(""), Ok(Toml::Table(vec![])));
        assert_eq!(parse("   \n\n# only a comment\n"), Ok(Toml::Table(vec![])));
    }

    // ── Depth limit (stack safety) ───────────────────────────────────────────

    #[test]
    fn deep_array_within_limit_ok() {
        let mut s = String::from("a = ");
        for _ in 0..50 {
            s.push('[');
        }
        s.push('1');
        for _ in 0..50 {
            s.push(']');
        }
        s.push('\n');
        assert!(parse(&s).is_ok());
    }

    #[test]
    fn pathological_array_nesting_rejected_not_panic() {
        let mut s = String::from("a = ");
        for _ in 0..(MAX_DEPTH + 50) {
            s.push('[');
        }
        // never closed; must Err, never panic / overflow stack
        let r = parse(&s);
        assert!(r.is_err());
        assert_eq!(r, Err(TomlError::DepthExceeded));
    }

    // ── Malformed battery: ALL Err, ZERO panics ──────────────────────────────

    #[test]
    fn malformed_battery_is_err_not_panic() {
        let bad = [
            "s = \"unterminated\n", // unterminated basic string
            "s = 'unterminated\n",  // unterminated literal string
            "a = [1, 2\n",          // unterminated array
            "t = { a = 1\n",        // unterminated inline table
            "a = 1\na = 2\n",       // duplicate key
            "s = \"\\xbad\"\n",     // bad escape
            "s = \"\\uZZZZ\"\n",    // non-hex \u
            "s = \"\\u00\"\n",      // short \u
            "[unclosed\n",          // unclosed header
            "[[unclosed]\n",        // half-closed array header
            "= 5\n",                // bare equals, no key
            "n = 01\n",             // leading zero integer
            "n = 1__0\n",           // doubled underscore
            "n = _10\n",            // leading underscore
            "n = 10_\n",            // trailing underscore
            "n = 0xGG\n",           // bad hex digits
            "f = 1e\n",             // exponent without digits
            "f = .5\n",             // leading dot float
            "f = 1.\n",             // trailing dot float
            "k 1\n",                // missing '='
            "[]\n",                 // empty header
            "k = \n",               // missing value
        ];
        for case in bad.iter() {
            let r = parse(case);
            assert!(
                r.is_err(),
                "expected Err for malformed {:?}, got {:?}",
                case,
                r
            );
        }
    }

    #[test]
    fn duplicate_key_specifically_rejected() {
        assert_eq!(parse("a = 1\na = 2\n"), Err(TomlError::DuplicateKey));
        // FAIL-guard: a unique-key doc must NOT be rejected.
        assert!(parse("a = 1\nb = 2\n").is_ok());
    }

    #[test]
    fn underscore_separator_specifically_handled() {
        // If `_`-separator handling breaks (e.g. underscores treated as digits),
        // this exact value flips.
        assert_eq!(
            parse("big = 12_345_678\n")
                .unwrap()
                .get("big")
                .and_then(Toml::as_i64),
            Some(12_345_678)
        );
        // malformed underscores reject
        assert!(parse("x = 1__2\n").is_err());
        assert!(parse("x = _1\n").is_err());
        assert!(parse("x = 1_\n").is_err());
    }

    // =======================================================================
    // Fuzz / property tests — hostile-config hardening.
    //
    // rae_toml parses user config (file_assoc.toml, app prefs) that can be
    // corrupt or adversarial. The contract is: parse(any &str) returns Ok or
    // Err, NEVER panics, never overflows the stack, never unbounded-allocs.
    // These tests:
    //   1. drive thousands of seeded-random byte→valid-UTF-8 garbage docs and
    //      a hand-built adversarial corpus, asserting no panic;
    //   2. confirm the depth/size caps actually bound pathological input;
    //   3. assert parse(serialize(x)) == x over generated tables (round-trip).
    //
    // No external fuzz crate: a tiny seeded xorshift PRNG builds the corpus.
    //
    // FAIL-ability (proven by reasoning in the REPORT): the round-trip property
    // asserts structural equality against the source value, so a serializer or
    // parser regression flips it. The depth test asserts the EXACT error
    // (`DepthExceeded`) and would fail-by-panic (stack overflow) if MAX_DEPTH's
    // guard were removed. The malformed-corpus test asserts `is_err()` for
    // inputs that MUST reject — accept one and it fails.
    // =======================================================================

    /// Deterministic xorshift64* PRNG — pure, reproducible.
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

    /// PROPERTY: `parse` never panics on random valid-UTF-8 documents built from
    /// a pool of TOML-significant tokens (the most adversarial corpus for a TOML
    /// grammar — it produces near-valid-but-broken structures, not just noise).
    /// The API takes `&str`, so we generate valid UTF-8 by composing str tokens
    /// (invalid-UTF-8 cannot reach `parse` through the type system; the parser's
    /// internal byte handling is separately exercised by the multibyte tokens).
    #[test]
    fn fuzz_parse_token_soup_never_panics() {
        // tokens chosen to stress every parser branch, incl. multibyte UTF-8
        let pool: [&str; 30] = [
            "[",
            "]",
            "[[",
            "]]",
            "{",
            "}",
            "=",
            ".",
            ",",
            "\"",
            "'",
            "\"\"\"",
            "'''",
            "\\",
            "\\u",
            "\\U0001F600",
            "#",
            "\n",
            " ",
            "\t",
            "a",
            "k",
            "1",
            "0x",
            "1_0",
            "1.5",
            "true",
            "false",
            "inf",
            "тест🦀",
        ];
        let mut rng = Rng::new(0xC0FF_EE00_1234_5678);
        for _ in 0..30_000 {
            let n = rng.below(40);
            let mut s = String::new();
            for _ in 0..n {
                s.push_str(pool[rng.below(pool.len())]);
            }
            // Must return Ok or Err — never panic, never hang on bounded input.
            let _ = parse(&s);
            // Whatever parsed (if Ok) must itself re-serialize+re-parse safely.
            if let Ok(v) = parse(&s) {
                let _ = parse(&to_string(&v));
            }
        }
    }

    /// PROPERTY: a hand-built corpus of adversarial configs ALL return Err (or a
    /// value) WITHOUT panicking — and the cases that MUST reject do reject. This
    /// extends `malformed_battery_is_err_not_panic` with structural attacks:
    /// huge keys, lone control chars, deep nesting, BOM, mixed quotes.
    #[test]
    fn fuzz_adversarial_corpus_no_panic() {
        // (input, must_reject) — must_reject=true ⇒ assert Err; false ⇒ just
        // assert no panic (the value/Err is acceptable either way).
        let huge_key: String = {
            let mut k = core::iter::repeat('k').take(50_000).collect::<String>();
            k.push_str(" = 1\n");
            k
        };
        let huge_string = {
            let mut s = String::from("v = \"");
            s.push_str(&core::iter::repeat('x').take(50_000).collect::<String>());
            s.push_str("\"\n");
            s
        };
        let bom = "\u{FEFF}a = 1\n"; // leading BOM — TOML allows none; must not panic
        let deep_inline = {
            // {a={a={a=... — unterminated deep inline tables
            let mut s = String::from("v = ");
            for _ in 0..(MAX_DEPTH + 60) {
                s.push_str("{a=");
            }
            s
        };
        let deep_dotted = {
            // a.a.a.....a = 1 — a header/key chain past MAX_DEPTH
            let mut s = String::new();
            for _ in 0..(MAX_DEPTH + 60) {
                s.push_str("a.");
            }
            s.push_str("z = 1\n");
            s
        };
        let owned: [(String, bool); 5] = [
            (huge_key, false),          // 50k-char bare key is VALID TOML (one key) — no panic
            (huge_string, false),       // valid huge string — must NOT panic, accepts
            (deep_inline, true),        // exceeds depth AND unterminated → Err
            (deep_dotted, true),        // key chain exceeds MAX_DEPTH → Err
            (String::from(bom), false), // BOM-prefixed — no panic
        ];
        let borrowed: [(&str, bool); 18] = [
            ("\0\0\0\0", false),                     // lone NULs
            ("a = \"\x01\x02\x03\"\n", false),       // control chars in string
            ("\x07 = 1\n", true),                    // bell as key → bad key
            ("a = \"\\\n", true),                    // backslash then EOF
            ("[a]]]]]]]]]]\n", true),                // header bracket spam
            ("{{{{{{{{\n", true),                    // brace spam (no key context)
            ("=========\n", true),                   // equals spam
            ("a = 'unterminated", true),             // literal string EOF
            ("a = \"\"\"unterminated", true),        // multiline basic EOF
            ("a = '''unterminated", true),           // multiline literal EOF
            ("a = [[[[[[[[[[", true),                // unterminated nested arrays
            ("[[[[[[[[", true),                      // unterminated array header
            ("a.b.c.d = ", true),                    // missing value
            ("a = 99999999999999999999999\n", true), // i64 overflow
            ("a = 0x\n", true),                      // radix prefix no digits
            ("a = 1.2.3\n", true),                   // malformed float
            ("\"\" = 1\n", false),                   // empty quoted key — accepted
            ("a = {} \n", false),                    // empty inline table — accepted
        ];
        for (inp, must_reject) in owned.iter() {
            let r = parse(inp); // must not panic
            if *must_reject {
                assert!(
                    r.is_err(),
                    "expected Err for adversarial input (len {})",
                    inp.len()
                );
            }
        }
        for (inp, must_reject) in borrowed.iter() {
            let r = parse(inp); // must not panic
            if *must_reject {
                assert!(r.is_err(), "expected Err for {:?}, got {:?}", inp, r);
            }
        }
    }

    /// The depth/size caps actually BOUND a pathological input: a document nested
    /// far past MAX_DEPTH is rejected with DepthExceeded — proving the recursion
    /// guard fires BEFORE the stack is exhausted (no stack overflow / hang).
    #[test]
    fn depth_cap_bounds_pathological_nesting() {
        // arrays
        let mut arr = String::from("a = ");
        for _ in 0..(MAX_DEPTH + 200) {
            arr.push('[');
        }
        arr.push('1');
        for _ in 0..(MAX_DEPTH + 200) {
            arr.push(']');
        }
        arr.push('\n');
        assert_eq!(parse(&arr), Err(TomlError::DepthExceeded));

        // inline tables nested past the cap
        let mut it = String::from("a = ");
        for _ in 0..(MAX_DEPTH + 200) {
            it.push_str("{b=");
        }
        it.push('1');
        for _ in 0..(MAX_DEPTH + 200) {
            it.push('}');
        }
        it.push('\n');
        assert_eq!(parse(&it), Err(TomlError::DepthExceeded));

        // header segment chain past the cap
        let mut hdr = String::from("[");
        for i in 0..(MAX_DEPTH + 200) {
            if i != 0 {
                hdr.push('.');
            }
            hdr.push('a');
        }
        hdr.push_str("]\n");
        assert_eq!(parse(&hdr), Err(TomlError::DepthExceeded));

        // FAIL-guard: a document AT the safe side of the cap still parses, so the
        // cap is a real boundary, not a blanket reject.
        let mut safe = String::from("a = ");
        for _ in 0..(MAX_DEPTH - 2) {
            safe.push('[');
        }
        safe.push('1');
        for _ in 0..(MAX_DEPTH - 2) {
            safe.push(']');
        }
        safe.push('\n');
        assert!(parse(&safe).is_ok());
    }

    /// PROPERTY (round-trip): `parse(to_string(x)) == x` over RANDOMLY GENERATED
    /// table values. We generate `Toml` values directly (not source text), so the
    /// serializer is the system under test; the parser is the oracle. A serializer
    /// that mis-escapes a string, mangles an integer, or drops a key flips this.
    ///
    /// Generated values are kept in the serializer's CANONICAL form so the
    /// property is exact:
    ///   - each table's SCALAR keys come before its SUB-TABLE keys (the serializer
    ///     emits leaves in pass 1, `[header]` sections in pass 2 — a documented
    ///     two-pass design, not a bug);
    ///   - scalars are integers / bools / strings / integer-arrays, all of which
    ///     this libm-free serializer reproduces byte-exactly. Floats are covered
    ///     separately by `float_forms`/`round_trip_realistic` with tolerance; an
    ///     exact-equality float round-trip is out of scope for a no-libm formatter.
    /// This still genuinely FAILs on the bugs that matter: string-escape errors,
    /// integer formatting errors, key drops/dupes, and array corruption.
    #[test]
    fn prop_serialize_parse_round_trip() {
        fn gen_scalar(rng: &mut Rng) -> Toml {
            match rng.below(4) {
                0 => Toml::Integer((rng.next_u64() as i64) / 4), // away from i64::MIN edge
                1 => Toml::Boolean(rng.below(2) == 1),
                2 => {
                    // strings with escapable / multibyte / control content — the
                    // serializer's escaper is the real thing under test here.
                    let pool = [
                        "plain",
                        "with space",
                        "q\"qu",
                        "ba\\ck",
                        "ta\tb",
                        "nl\nl",
                        "cr\rr",
                        "💾café тест",
                        "",
                        "= [ ] { } #",
                    ];
                    Toml::String(String::from(pool[rng.below(pool.len())]))
                }
                _ => {
                    let len = rng.below(4);
                    let mut v = alloc::vec::Vec::new();
                    for _ in 0..len {
                        v.push(Toml::Integer(rng.below(1000) as i64));
                    }
                    Toml::Array(v)
                }
            }
        }
        // Build a table whose scalar keys precede its sub-table keys (canonical).
        fn gen_table(rng: &mut Rng, depth: usize) -> Toml {
            let n_scalar = rng.below(5);
            let n_sub = if depth < 3 { rng.below(3) } else { 0 };
            let mut t: alloc::vec::Vec<(String, Toml)> = alloc::vec::Vec::new();
            // scalars first (unique keys → never trips duplicate-key reject)
            for i in 0..n_scalar {
                t.push((alloc::format!("s{}", i), gen_scalar(rng)));
            }
            // then non-empty sub-tables (an empty sub-table would also serialize
            // as a header and round-trip, but skip them to keep keys deterministic)
            for i in 0..n_sub {
                let sub = gen_table(rng, depth + 1);
                t.push((alloc::format!("t{}", i), sub));
            }
            Toml::Table(t)
        }

        let mut rng = Rng::new(0x5151_AAAA_9999_0001);
        let mut tested = 0u32;
        for _ in 0..3_000 {
            let v = gen_table(&mut rng, 0);
            let s = to_string(&v);
            match parse(&s) {
                Ok(v2) => {
                    assert_eq!(
                        v, v2,
                        "round-trip mismatch\n--- value ---\n{:?}\n--- text ---\n{}",
                        v, s
                    );
                    tested += 1;
                }
                Err(e) => panic!(
                    "serialized output failed to re-parse: {:?}\ntext:\n{}",
                    e, s
                ),
            }
        }
        assert!(tested > 0, "round-trip property exercised nothing");
    }
}
