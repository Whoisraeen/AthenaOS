//! # RaeJSON — a never-panic, `no_std` JSON parser + serializer (RFC 8259).
//!
//! RaeenOS_Concept.md §"Web apps via PWA support that actually feels native":
//! the web is the universal app runtime, and JSON is the lingua franca of every
//! web API a PWA talks to. It is *also* the on-disk format for RaeStore package
//! manifests and for settings/config. One correct, dependency-free, hostile-input
//! JSON core serves all three — so this crate is foundational infrastructure, not
//! tied to any one consumer (it is deliberately wired into none this slice).
//!
//! ## Hostile-input posture (CLAUDE: parsers of untrusted bytes are an RCE surface)
//! Every byte handed to [`parse`] is treated as hostile. There is **no
//! `unwrap`/`expect`/`panic`/raw-index-panic path** reachable from the parser:
//! malformed UTF-8 escapes, lone surrogates, unclosed braces/brackets/strings,
//! trailing commas, bare words, leading-zero numbers, control characters in
//! strings, and pathological nesting all return `Err(JsonError)`. Recursion is
//! bounded by [`MAX_DEPTH`] so a crafted deeply-nested document cannot blow the
//! stack. The host KAT suite at the bottom of this file is the primary proof
//! (`cargo test -p rae_json`).
//!
//! ## What it is
//! - A [`Json`] value enum mirroring the RFC 8259 grammar, **preserving object
//!   key order** (objects are `Vec<(String, Json)>`, not a hash map).
//! - [`parse`]: a recursive-descent parser over UTF-8 `&str` with full string
//!   escape handling (`\" \\ \/ \b \f \n \r \t` and `\uXXXX` including UTF-16
//!   surrogate pairs decoded to UTF-8), and a pure-`f64` number builder (no
//!   `libm`, no `f64::from_str`).
//! - [`to_string`]: a round-trippable serializer with proper string escaping, so
//!   the crate writes config/manifests as well as reads them.
//! - Convenience accessors ([`Json::as_str`], [`Json::get`], [`Json::at`], …),
//!   all `Option`-returning and panic-free.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

/// Maximum nesting depth of arrays/objects the parser will accept. A document
/// nested deeper than this is rejected with [`JsonError::DepthExceeded`] *before*
/// the recursion can exhaust the stack — this is the stack-safety bound for the
/// hostile-input posture, not a JSON-spec limit.
pub const MAX_DEPTH: usize = 256;

/// An RFC 8259 JSON value.
///
/// `Number` is stored as `f64` (the JSON spec's number is a decimal real; we use
/// the IEEE-754 double JavaScript itself uses). `Object` is an order-preserving
/// `Vec<(String, Json)>` so manifests/config round-trip with keys in source
/// order rather than a hash-map's arbitrary order.
#[derive(Debug, Clone, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Array(Vec<Json>),
    Object(Vec<(String, Json)>),
}

/// Why parsing failed. Every malformed input maps to one of these — the parser
/// never panics, so a web/manifest/config consumer can surface a calm error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JsonError {
    /// Input was empty or only whitespace.
    Empty,
    /// A required token/character was missing or unexpected.
    Unexpected,
    /// A string, array, or object was not closed before end-of-input.
    Unterminated,
    /// A `\`-escape in a string was invalid (unknown escape or short `\u`).
    BadEscape,
    /// A `\u` escape produced a lone/mismatched UTF-16 surrogate.
    LoneSurrogate,
    /// A number did not match the RFC 8259 grammar (e.g. leading zero, lone
    /// minus, missing exponent digits, lone decimal point).
    BadNumber,
    /// A raw control character (U+0000..U+001F) appeared unescaped in a string.
    ControlChar,
    /// Nesting exceeded [`MAX_DEPTH`].
    DepthExceeded,
    /// Valid value parsed, but non-whitespace junk followed it.
    TrailingData,
}

// ── Convenience accessors (all Option, never panic) ──────────────────────────

impl Json {
    /// `&str` if this is a `String`, else `None`.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::String(s) => Some(s.as_str()),
            _ => None,
        }
    }

    /// `f64` if this is a `Number`, else `None`.
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Json::Number(n) => Some(*n),
            _ => None,
        }
    }

    /// `bool` if this is a `Bool`, else `None`.
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Json::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// `true` if this is `Null`.
    pub fn is_null(&self) -> bool {
        matches!(self, Json::Null)
    }

    /// The backing slice if this is an `Array`, else `None`.
    pub fn as_array(&self) -> Option<&[Json]> {
        match self {
            Json::Array(v) => Some(v.as_slice()),
            _ => None,
        }
    }

    /// The backing `(key, value)` slice if this is an `Object`, else `None`.
    pub fn as_object(&self) -> Option<&[(String, Json)]> {
        match self {
            Json::Object(v) => Some(v.as_slice()),
            _ => None,
        }
    }

    /// Look up `key` in an `Object` (first match, source order). `None` if this
    /// is not an object or the key is absent.
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Object(v) => v.iter().find(|(k, _)| k == key).map(|(_, val)| val),
            _ => None,
        }
    }

    /// Index `i` of an `Array`. `None` if this is not an array or out of bounds.
    pub fn at(&self, i: usize) -> Option<&Json> {
        match self {
            Json::Array(v) => v.get(i),
            _ => None,
        }
    }
}

// ── Parser ───────────────────────────────────────────────────────────────────

/// Parse a UTF-8 JSON document into a [`Json`] value.
///
/// Accepts the full RFC 8259 grammar; rejects everything else with a
/// [`JsonError`] (never panics). Object key order is preserved.
///
/// ```
/// use rae_json::{parse, Json};
/// let v = parse(r#"{"name":"Rae","n":42,"ok":true}"#).unwrap();
/// assert_eq!(v.get("name").and_then(Json::as_str), Some("Rae"));
/// assert_eq!(v.get("n").and_then(Json::as_f64), Some(42.0));
/// assert_eq!(v.get("ok").and_then(Json::as_bool), Some(true));
/// ```
pub fn parse(input: &str) -> Result<Json, JsonError> {
    let mut p = ParserState {
        bytes: input.as_bytes(),
        pos: 0,
    };
    p.skip_ws();
    if p.pos >= p.bytes.len() {
        return Err(JsonError::Empty);
    }
    let value = p.parse_value(0)?;
    p.skip_ws();
    if p.pos != p.bytes.len() {
        return Err(JsonError::TrailingData);
    }
    Ok(value)
}

struct ParserState<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> ParserState<'a> {
    #[inline]
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    #[inline]
    fn bump(&mut self) -> Option<u8> {
        let b = self.bytes.get(self.pos).copied();
        if b.is_some() {
            self.pos += 1;
        }
        b
    }

    fn skip_ws(&mut self) {
        while let Some(b) = self.peek() {
            match b {
                b' ' | b'\t' | b'\n' | b'\r' => self.pos += 1,
                _ => break,
            }
        }
    }

    /// Consume an exact ASCII literal (`true`/`false`/`null` tails).
    fn expect_literal(&mut self, lit: &[u8]) -> Result<(), JsonError> {
        if self.pos + lit.len() > self.bytes.len() {
            return Err(JsonError::Unexpected);
        }
        if &self.bytes[self.pos..self.pos + lit.len()] != lit {
            return Err(JsonError::Unexpected);
        }
        self.pos += lit.len();
        Ok(())
    }

    fn parse_value(&mut self, depth: usize) -> Result<Json, JsonError> {
        if depth > MAX_DEPTH {
            return Err(JsonError::DepthExceeded);
        }
        self.skip_ws();
        match self.peek() {
            None => Err(JsonError::Unexpected),
            Some(b'{') => self.parse_object(depth),
            Some(b'[') => self.parse_array(depth),
            Some(b'"') => Ok(Json::String(self.parse_string()?)),
            Some(b't') => {
                self.expect_literal(b"true")?;
                Ok(Json::Bool(true))
            }
            Some(b'f') => {
                self.expect_literal(b"false")?;
                Ok(Json::Bool(false))
            }
            Some(b'n') => {
                self.expect_literal(b"null")?;
                Ok(Json::Null)
            }
            Some(b'-') | Some(b'0'..=b'9') => self.parse_number(),
            Some(_) => Err(JsonError::Unexpected),
        }
    }

    fn parse_object(&mut self, depth: usize) -> Result<Json, JsonError> {
        if depth >= MAX_DEPTH {
            return Err(JsonError::DepthExceeded);
        }
        // consume '{'
        self.pos += 1;
        let mut members: Vec<(String, Json)> = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            return Ok(Json::Object(members));
        }
        loop {
            self.skip_ws();
            // key must be a string
            if self.peek() != Some(b'"') {
                return match self.peek() {
                    None => Err(JsonError::Unterminated),
                    _ => Err(JsonError::Unexpected),
                };
            }
            let key = self.parse_string()?;
            self.skip_ws();
            if self.bump() != Some(b':') {
                return Err(JsonError::Unexpected);
            }
            let val = self.parse_value(depth + 1)?;
            members.push((key, val));
            self.skip_ws();
            match self.bump() {
                Some(b',') => continue,
                Some(b'}') => return Ok(Json::Object(members)),
                None => return Err(JsonError::Unterminated),
                Some(_) => return Err(JsonError::Unexpected),
            }
        }
    }

    fn parse_array(&mut self, depth: usize) -> Result<Json, JsonError> {
        if depth >= MAX_DEPTH {
            return Err(JsonError::DepthExceeded);
        }
        // consume '['
        self.pos += 1;
        let mut items: Vec<Json> = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b']') {
            self.pos += 1;
            return Ok(Json::Array(items));
        }
        loop {
            let val = self.parse_value(depth + 1)?;
            items.push(val);
            self.skip_ws();
            match self.bump() {
                Some(b',') => continue,
                Some(b']') => return Ok(Json::Array(items)),
                None => return Err(JsonError::Unterminated),
                Some(_) => return Err(JsonError::Unexpected),
            }
        }
    }

    /// Parse a string literal starting at the opening `"`. Returns the decoded
    /// (escape-resolved, UTF-8) contents.
    fn parse_string(&mut self) -> Result<String, JsonError> {
        // consume opening quote
        if self.bump() != Some(b'"') {
            return Err(JsonError::Unexpected);
        }
        let mut out = String::new();
        loop {
            match self.bump() {
                None => return Err(JsonError::Unterminated),
                Some(b'"') => return Ok(out),
                Some(b'\\') => self.parse_escape(&mut out)?,
                Some(b) if b < 0x20 => return Err(JsonError::ControlChar),
                Some(b) if b < 0x80 => out.push(b as char),
                Some(b) => {
                    // A UTF-8 lead byte: copy the whole sequence verbatim. The
                    // input is a valid `&str`, so the continuation bytes exist
                    // and form a valid scalar — but we still verify defensively.
                    let len = utf8_len(b);
                    if len == 0 || self.pos + (len - 1) > self.bytes.len() {
                        return Err(JsonError::Unexpected);
                    }
                    let start = self.pos - 1;
                    let end = start + len;
                    match core::str::from_utf8(&self.bytes[start..end]) {
                        Ok(s) => {
                            out.push_str(s);
                            self.pos = end;
                        }
                        Err(_) => return Err(JsonError::Unexpected),
                    }
                }
            }
        }
    }

    /// Handle the character after a `\` (the backslash is already consumed).
    fn parse_escape(&mut self, out: &mut String) -> Result<(), JsonError> {
        match self.bump() {
            None => Err(JsonError::Unterminated),
            Some(b'"') => {
                out.push('"');
                Ok(())
            }
            Some(b'\\') => {
                out.push('\\');
                Ok(())
            }
            Some(b'/') => {
                out.push('/');
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
            Some(b'u') => self.parse_unicode_escape(out),
            Some(_) => Err(JsonError::BadEscape),
        }
    }

    /// Parse the 4 hex digits after `\u`, decoding UTF-16 surrogate pairs into a
    /// single Unicode scalar, then push the UTF-8 of that scalar.
    fn parse_unicode_escape(&mut self, out: &mut String) -> Result<(), JsonError> {
        let hi = self.read_hex4()?;
        if (0xD800..=0xDBFF).contains(&hi) {
            // high surrogate — must be followed by `\uXXXX` low surrogate
            if self.bump() != Some(b'\\') {
                return Err(JsonError::LoneSurrogate);
            }
            if self.bump() != Some(b'u') {
                return Err(JsonError::LoneSurrogate);
            }
            let lo = self.read_hex4()?;
            if !(0xDC00..=0xDFFF).contains(&lo) {
                return Err(JsonError::LoneSurrogate);
            }
            let scalar = 0x1_0000u32 + (((hi - 0xD800) as u32) << 10) + ((lo - 0xDC00) as u32);
            match char::from_u32(scalar) {
                Some(c) => {
                    out.push(c);
                    Ok(())
                }
                None => Err(JsonError::LoneSurrogate),
            }
        } else if (0xDC00..=0xDFFF).contains(&hi) {
            // unaccompanied low surrogate
            Err(JsonError::LoneSurrogate)
        } else {
            match char::from_u32(hi as u32) {
                Some(c) => {
                    out.push(c);
                    Ok(())
                }
                None => Err(JsonError::BadEscape),
            }
        }
    }

    /// Read exactly four hex digits into a `u16`.
    fn read_hex4(&mut self) -> Result<u16, JsonError> {
        let mut v: u16 = 0;
        for _ in 0..4 {
            let d = match self.bump() {
                None => return Err(JsonError::BadEscape),
                Some(b) => hex_val(b).ok_or(JsonError::BadEscape)?,
            };
            v = (v << 4) | d as u16;
        }
        Ok(v)
    }

    /// Parse a number per the RFC 8259 grammar:
    /// `-? (0 | [1-9][0-9]*) ('.' [0-9]+)? ([eE][+-]?[0-9]+)?`.
    /// Builds the `f64` manually (no `f64::from_str`, no `libm`).
    fn parse_number(&mut self) -> Result<Json, JsonError> {
        let start = self.pos;

        let mut neg = false;
        if self.peek() == Some(b'-') {
            neg = true;
            self.pos += 1;
        }

        // integer part
        match self.peek() {
            Some(b'0') => {
                self.pos += 1;
                // leading zero may NOT be followed by another digit
                if let Some(b'0'..=b'9') = self.peek() {
                    return Err(JsonError::BadNumber);
                }
            }
            Some(b'1'..=b'9') => {
                self.pos += 1;
                while let Some(b'0'..=b'9') = self.peek() {
                    self.pos += 1;
                }
            }
            _ => return Err(JsonError::BadNumber),
        }

        // we accumulate the digits ourselves rather than re-scan
        let mut mantissa: f64 = 0.0;
        // integer digits (skip the leading '-')
        let int_begin = if neg { start + 1 } else { start };
        for &b in &self.bytes[int_begin..self.pos] {
            mantissa = mantissa * 10.0 + (b - b'0') as f64;
        }

        // fraction
        let mut frac_exp: i32 = 0;
        if self.peek() == Some(b'.') {
            self.pos += 1;
            // must be at least one digit
            if !matches!(self.peek(), Some(b'0'..=b'9')) {
                return Err(JsonError::BadNumber);
            }
            while let Some(b @ b'0'..=b'9') = self.peek() {
                mantissa = mantissa * 10.0 + (b - b'0') as f64;
                frac_exp -= 1;
                self.pos += 1;
            }
        }

        // exponent
        let mut exp: i32 = 0;
        if matches!(self.peek(), Some(b'e') | Some(b'E')) {
            self.pos += 1;
            let mut exp_neg = false;
            match self.peek() {
                Some(b'+') => self.pos += 1,
                Some(b'-') => {
                    exp_neg = true;
                    self.pos += 1;
                }
                _ => {}
            }
            // must be at least one digit
            if !matches!(self.peek(), Some(b'0'..=b'9')) {
                return Err(JsonError::BadNumber);
            }
            let mut e: i32 = 0;
            while let Some(b @ b'0'..=b'9') = self.peek() {
                // saturate to keep the scaling bounded (huge exps -> ±inf/0)
                e = e.saturating_mul(10).saturating_add((b - b'0') as i32);
                self.pos += 1;
            }
            exp = if exp_neg { -e } else { e };
        }

        let total_exp = exp.saturating_add(frac_exp);
        let mut value = scale_pow10(mantissa, total_exp);
        if neg {
            value = -value;
        }
        Ok(Json::Number(value))
    }
}

/// Powers of ten `10^k` for `k ∈ 0..=22`, each *exactly* representable as `f64`
/// (10^22 has ≤ 53 significant bits). Used for the correctly-rounded fast path in
/// [`scale_pow10`].
const POW10_EXACT: [f64; 23] = [
    1e0, 1e1, 1e2, 1e3, 1e4, 1e5, 1e6, 1e7, 1e8, 1e9, 1e10, 1e11, 1e12, 1e13, 1e14, 1e15, 1e16,
    1e17, 1e18, 1e19, 1e20, 1e21, 1e22,
];

/// Powers of ten `10^k` for `k ∈ 0..=308`, each the *correctly-rounded nearest*
/// `f64` to the true decimal power (Rust evaluates the literals at compile time
/// with round-to-nearest). For `k > 22` these are no longer exact integers, but a
/// single `mantissa * POW10_BIG[k]` (or `mantissa / POW10_BIG[k]`) introduces at
/// most one extra rounding — far better than the old chained `*= 10.0` loop, and
/// the serializer's parse-back oracle absorbs any residual last-ULP slack by
/// trying more significant digits. `10^309` overflows `f64`, so `k` is capped.
const POW10_BIG: [f64; 309] = [
    1e0, 1e1, 1e2, 1e3, 1e4, 1e5, 1e6, 1e7, 1e8, 1e9, 1e10, 1e11, 1e12, 1e13, 1e14, 1e15, 1e16,
    1e17, 1e18, 1e19, 1e20, 1e21, 1e22, 1e23, 1e24, 1e25, 1e26, 1e27, 1e28, 1e29, 1e30, 1e31, 1e32,
    1e33, 1e34, 1e35, 1e36, 1e37, 1e38, 1e39, 1e40, 1e41, 1e42, 1e43, 1e44, 1e45, 1e46, 1e47, 1e48,
    1e49, 1e50, 1e51, 1e52, 1e53, 1e54, 1e55, 1e56, 1e57, 1e58, 1e59, 1e60, 1e61, 1e62, 1e63, 1e64,
    1e65, 1e66, 1e67, 1e68, 1e69, 1e70, 1e71, 1e72, 1e73, 1e74, 1e75, 1e76, 1e77, 1e78, 1e79, 1e80,
    1e81, 1e82, 1e83, 1e84, 1e85, 1e86, 1e87, 1e88, 1e89, 1e90, 1e91, 1e92, 1e93, 1e94, 1e95, 1e96,
    1e97, 1e98, 1e99, 1e100, 1e101, 1e102, 1e103, 1e104, 1e105, 1e106, 1e107, 1e108, 1e109, 1e110,
    1e111, 1e112, 1e113, 1e114, 1e115, 1e116, 1e117, 1e118, 1e119, 1e120, 1e121, 1e122, 1e123,
    1e124, 1e125, 1e126, 1e127, 1e128, 1e129, 1e130, 1e131, 1e132, 1e133, 1e134, 1e135, 1e136,
    1e137, 1e138, 1e139, 1e140, 1e141, 1e142, 1e143, 1e144, 1e145, 1e146, 1e147, 1e148, 1e149,
    1e150, 1e151, 1e152, 1e153, 1e154, 1e155, 1e156, 1e157, 1e158, 1e159, 1e160, 1e161, 1e162,
    1e163, 1e164, 1e165, 1e166, 1e167, 1e168, 1e169, 1e170, 1e171, 1e172, 1e173, 1e174, 1e175,
    1e176, 1e177, 1e178, 1e179, 1e180, 1e181, 1e182, 1e183, 1e184, 1e185, 1e186, 1e187, 1e188,
    1e189, 1e190, 1e191, 1e192, 1e193, 1e194, 1e195, 1e196, 1e197, 1e198, 1e199, 1e200, 1e201,
    1e202, 1e203, 1e204, 1e205, 1e206, 1e207, 1e208, 1e209, 1e210, 1e211, 1e212, 1e213, 1e214,
    1e215, 1e216, 1e217, 1e218, 1e219, 1e220, 1e221, 1e222, 1e223, 1e224, 1e225, 1e226, 1e227,
    1e228, 1e229, 1e230, 1e231, 1e232, 1e233, 1e234, 1e235, 1e236, 1e237, 1e238, 1e239, 1e240,
    1e241, 1e242, 1e243, 1e244, 1e245, 1e246, 1e247, 1e248, 1e249, 1e250, 1e251, 1e252, 1e253,
    1e254, 1e255, 1e256, 1e257, 1e258, 1e259, 1e260, 1e261, 1e262, 1e263, 1e264, 1e265, 1e266,
    1e267, 1e268, 1e269, 1e270, 1e271, 1e272, 1e273, 1e274, 1e275, 1e276, 1e277, 1e278, 1e279,
    1e280, 1e281, 1e282, 1e283, 1e284, 1e285, 1e286, 1e287, 1e288, 1e289, 1e290, 1e291, 1e292,
    1e293, 1e294, 1e295, 1e296, 1e297, 1e298, 1e299, 1e300, 1e301, 1e302, 1e303, 1e304, 1e305,
    1e306, 1e307, 1e308,
];

/// Largest integer mantissa with no precision loss in `f64` (2^53).
const MAX_EXACT_MANTISSA: f64 = 9_007_199_254_740_992.0;

/// Compute `mantissa * 10^exp` without `libm`, **correctly rounded** in the common
/// regime (Clinger's fast path). Saturates to `0.0`/`±inf` for out-of-range
/// exponents in a bounded number of operations.
///
/// The previous implementation multiplied by `10.0`/`0.1` repeatedly, which
/// compounds a rounding error per step — `8434 * 0.1 * 0.1` lands one ULP above
/// the true `84.34`. That made the parser lossy, which in turn made the
/// shortest-round-trip serializer unable to find any string mapping back to a
/// literal like `84.34`. When both operands are exact, a *single* IEEE multiply or
/// divide is correctly rounded, so the fast path below restores exactness.
fn scale_pow10(mantissa: f64, exp: i32) -> f64 {
    if mantissa == 0.0 {
        return 0.0;
    }

    // Fast path: mantissa is an exact integer (≤ 2^53) and one exact power of ten
    // gets us there in a single correctly-rounded IEEE operation.
    if mantissa <= MAX_EXACT_MANTISSA {
        if (0..=22).contains(&exp) {
            // mantissa * 10^exp, both operands exact -> one correctly-rounded mul.
            return mantissa * POW10_EXACT[exp as usize];
        }
        if (-22..0).contains(&exp) {
            // mantissa / 10^(-exp), both operands exact -> one correctly-rounded div.
            return mantissa / POW10_EXACT[(-exp) as usize];
        }
        // Extended fast path: pull the mantissa up by 10^k (still exact, result
        // ≤ 2^53) so the residual exponent fits 0..=22, then one exact multiply.
        if (23..=44).contains(&exp) {
            let k = exp - 22;
            let lifted = mantissa * POW10_EXACT[k as usize];
            if lifted <= MAX_EXACT_MANTISSA {
                return lifted * POW10_EXACT[22];
            }
        }
    }

    // Single correctly-rounded power-of-ten multiply/divide for the rest of the
    // f64 range. One extra rounding at most (vs. the chained loop's per-step
    // error); the serializer oracle handles residual last-ULP cases.
    if (0..=308).contains(&exp) {
        let r = mantissa * POW10_BIG[exp as usize];
        if r.is_finite() {
            return r;
        }
    } else if (-308..0).contains(&exp) {
        return mantissa / POW10_BIG[(-exp) as usize];
    }

    // Extreme / overflow path: bounded repeated multiply. Saturates a crafted
    // 1e9999 to inf and 1e-9999 to 0 in a bounded number of steps.
    let clamped = if exp > 400 {
        400
    } else if exp < -400 {
        -400
    } else {
        exp
    };
    let mut value = mantissa;
    let mut n = clamped;
    if n > 0 {
        while n > 0 {
            value *= 10.0;
            n -= 1;
        }
    } else {
        while n < 0 {
            value *= 0.1;
            n += 1;
        }
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

/// Hex digit value (0..=15), or `None` if `b` is not `[0-9a-fA-F]`.
#[inline]
fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

// ── Serializer ────────────────────────────────────────────────────────────────

/// Serialize a [`Json`] value to a compact, round-trippable JSON string.
///
/// Strings are escaped per RFC 8259 (control chars, `"` and `\`); numbers use a
/// pure-`f64` formatter (no `libm`). Object keys are emitted in stored order.
///
/// ```
/// use rae_json::{parse, to_string};
/// let v = parse(r#"{"a":[1,2,3],"b":"hi\n"}"#).unwrap();
/// let s = to_string(&v);
/// assert_eq!(parse(&s).unwrap(), v); // round-trips
/// ```
pub fn to_string(value: &Json) -> String {
    let mut out = String::new();
    write_value(&mut out, value);
    out
}

fn write_value(out: &mut String, value: &Json) {
    match value {
        Json::Null => out.push_str("null"),
        Json::Bool(true) => out.push_str("true"),
        Json::Bool(false) => out.push_str("false"),
        Json::Number(n) => write_number(out, *n),
        Json::String(s) => write_escaped_string(out, s),
        Json::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i != 0 {
                    out.push(',');
                }
                write_value(out, item);
            }
            out.push(']');
        }
        Json::Object(members) => {
            out.push('{');
            for (i, (k, v)) in members.iter().enumerate() {
                if i != 0 {
                    out.push(',');
                }
                write_escaped_string(out, k);
                out.push(':');
                write_value(out, v);
            }
            out.push('}');
        }
    }
}

/// Write a JSON string literal, escaping per RFC 8259.
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
                // other control chars -> \u00XX
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

/// Format an `f64` into `out` without `libm`, as the **shortest decimal string
/// that round-trips back to the same `f64`** through this crate's own [`parse`].
///
/// Non-finite values print as `null` (JSON has no NaN/Inf). Exact integers in the
/// faithful range print without a decimal point (`42`, not `42.0`). Everything
/// else uses the shortest-digits search below, in fixed or scientific notation.
///
/// ## Correctness contracts (the two write-side bugs this replaces)
/// 1. **Large finite doubles** (`|n| >= ~1e18`) used to saturate through
///    `(x as u128)` and emit garbage; they now print in scientific notation
///    (e.g. `1.1e147`) that re-parses to the identical bits.
/// 2. **Fractional values** used to emit non-shortest digit noise
///    (`84.340000000000102`), so `serialize→parse→serialize` drifted; the
///    shortest-round-trip search makes serialization idempotent by construction.
///
/// ## Approach (correct-enough, no Ryū/Grisū needed in `no_std`)
/// Try `p = 1..=17` significant digits. For each `p`, emit the `p`-significant
/// rounded decimal and re-`parse` it with the crate's own parser; the FIRST `p`
/// whose re-parse equals `n` bit-for-bit is the answer. Because the oracle is the
/// crate's own parser, this guarantees `parse(write_number(n)) == n` (round-trip
/// exact through the string) and hence serializer idempotency for every finite
/// value, regardless of magnitude. 17 significant digits always round-trips an
/// f64, so the loop always terminates with a faithful result.
fn write_number(out: &mut String, n: f64) {
    if !n.is_finite() {
        // JSON cannot represent NaN/Inf; emit null so output stays valid JSON.
        out.push_str("null");
        return;
    }
    if n == 0.0 {
        // Preserves -0.0 as "0" (re-parses to +0.0; JSON has no signed zero, and
        // 0.0 == -0.0 so the round-trip contract still holds).
        out.push('0');
        return;
    }

    let mut value = n;
    if value < 0.0 {
        out.push('-');
        value = -value;
    }

    // Exact integer fast path (covers manifest version ints, counts, etc.): a
    // clean integer with no decimal point. Bounded to the range where `value as
    // u64` is exact (1e18 < u64::MAX, and every integer < 2^53 is representable;
    // above 2^53 only multiples of a power of two are integral, all < 1e18 still
    // fit u64 exactly). This branch is idempotent on its own.
    if value < 1e18 && value == (value as u64) as f64 {
        write_u64(out, value as u64);
        return;
    }

    // General path: shortest decimal that re-parses to `value`.
    write_shortest(out, value);
}

/// Append the shortest `p`-significant-digit decimal (1..=17) for `value > 0`
/// (already finite, non-integer-fast-path) that re-parses to `value` exactly.
fn write_shortest(out: &mut String, value: f64) {
    // Normalize `value` into the mantissa range [1, 10) and record the base
    // decimal exponent e10 such that value ≈ m * 10^e10. Done with bounded
    // exact-power multiplies so no intermediate overflows (mirrors `scale_pow10`).
    let (e10, _m) = decimal_exponent(value);

    // Try increasing significant-digit counts until one round-trips. Each attempt
    // recomputes from the stable base `e10`; `significant_digits` returns its own
    // (possibly carry-bumped) exponent in `exp` for this precision.
    for p in 1..=17usize {
        let mut digits = [0u8; 17];
        let mut exp = e10;
        let ndig = significant_digits(value, e10, p, &mut digits, &mut exp);

        let mut cand = String::new();
        format_decimal(&mut cand, &digits[..ndig], exp);

        // Oracle: re-parse with the crate's own parser and compare bits.
        if let Ok(Json::Number(back)) = parse(&cand) {
            if back.to_bits() == value.to_bits() {
                out.push_str(&cand);
                return;
            }
        }
    }

    // Fallback (should be unreachable: 17 sig digits always round-trips an f64).
    // Emit the full 17-digit form so output is at least valid + close.
    let mut digits = [0u8; 17];
    let mut exp = e10;
    let ndig = significant_digits(value, e10, 17, &mut digits, &mut exp);
    format_decimal(out, &digits[..ndig], exp);
}

/// Return `(e10, m)` where `value = m * 10^e10` and `m ∈ [1, 10)`, for
/// `value > 0` finite. Bounded loop using exact small powers of ten; no overflow.
fn decimal_exponent(value: f64) -> (i32, f64) {
    let mut m = value;
    let mut e10: i32 = 0;
    // Bring large values down. Each /10 is one decimal place; bounded by f64 range
    // (~±308) plus slack, so this can never spin.
    while m >= 10.0 {
        m *= 0.1;
        e10 += 1;
        if e10 > 400 {
            break;
        }
    }
    // Bring small values up.
    while m < 1.0 {
        m *= 10.0;
        e10 -= 1;
        if e10 < -400 {
            break;
        }
    }
    (e10, m)
}

/// Extract `p` significant decimal digits (rounded to nearest, ties away) of
/// `value > 0`, given its decimal exponent `e10` (value ≈ m·10^e10, m∈[1,10)).
/// Writes the digits into `digits[..p]` (each 0..=9). If rounding carries past the
/// leading digit (e.g. 9.995 → 10.00 at p=3), the digits become `1 0 0…` and the
/// returned exponent (`*out_exp`) is bumped by one. Trailing zeros are trimmed and
/// the trimmed digit count is returned.
fn significant_digits(
    value: f64,
    e10: i32,
    p: usize,
    digits: &mut [u8; 17],
    out_exp: &mut i32,
) -> usize {
    // scaled = value * 10^(p-1-e10), an integer-valued magnitude in
    // [10^(p-1), 10^p). Use the correctly-rounded `scale_pow10` so the digits are
    // accurate even at the extremes of the f64 range (the old chained `*0.1`/`*10`
    // normalization drifted near f64::MAX and produced wrong leading digits).
    let q = (p as i32) - 1 - e10;
    let scaled = scale_pow10(value, q); // ∈ ~[10^(p-1), 10^p)

    // Round to nearest integer (ties away from zero); scaled < 1e17 fits u128.
    let mut int = (scaled + 0.5) as u128;

    let pow_p = pow10_u128(p);
    let mut exp = e10;
    if int >= pow_p {
        // Rounding carried (…99→100): drop the last digit, bump the exponent.
        int /= 10;
        exp += 1;
    }
    *out_exp = exp;

    // Render the (now exactly p-digit) integer into digits[0..p], MSD first.
    let mut tmp = [0u8; 17];
    let mut count = 0usize;
    let mut v = int;
    if v == 0 {
        tmp[0] = 0;
        count = 1;
    } else {
        while v > 0 {
            tmp[count] = (v % 10) as u8;
            v /= 10;
            count += 1;
        }
    }
    // Left-pad to exactly p digits (a value like 1·10^(p-1) already has p digits).
    let lead_zeros = p - count;
    let mut idx = 0usize;
    for _ in 0..lead_zeros {
        digits[idx] = 0;
        idx += 1;
    }
    for i in (0..count).rev() {
        digits[idx] = tmp[i];
        idx += 1;
    }
    let mut ndig = p;
    // Trim trailing zeros (shortest form); keep at least one digit.
    while ndig > 1 && digits[ndig - 1] == 0 {
        ndig -= 1;
    }
    ndig
}

/// `10^p` as `u128` for `p ∈ 0..=17`.
#[inline]
fn pow10_u128(p: usize) -> u128 {
    let mut v = 1u128;
    for _ in 0..p {
        v *= 10;
    }
    v
}

/// Format significant `digits` (MSD first, each 0..=9) with decimal exponent
/// `e10` (so the value is `d0.d1d2… × 10^e10`) into `out`. Chooses fixed or
/// scientific notation the way a typical JSON/JS serializer does, always emitting
/// a form this crate's parser accepts (single leading zero, valid `e` exponent).
fn format_decimal(out: &mut String, digits: &[u8], e10: i32) {
    debug_assert!(!digits.is_empty());
    let n = digits.len();

    // Scientific notation for very large or very small magnitudes (mirrors the
    // JS Number→string thresholds: exponent < -6 or >= 21). Keeps fixed output
    // compact and avoids absurd zero-runs.
    if e10 < -6 || e10 >= 21 {
        // d0[.d1d2…]e±E
        out.push((b'0' + digits[0]) as char);
        if n > 1 {
            out.push('.');
            for &d in &digits[1..] {
                out.push((b'0' + d) as char);
            }
        }
        out.push('e');
        let mut e = e10;
        if e < 0 {
            out.push('-');
            e = -e;
        }
        write_u64(out, e as u64);
        return;
    }

    if e10 >= 0 {
        let int_len = (e10 as usize) + 1;
        if n <= int_len {
            // All significant digits are in the integer part; pad with zeros.
            for &d in digits {
                out.push((b'0' + d) as char);
            }
            for _ in 0..(int_len - n) {
                out.push('0');
            }
        } else {
            // Split: int_len digits before the point, the rest after.
            for &d in &digits[..int_len] {
                out.push((b'0' + d) as char);
            }
            out.push('.');
            for &d in &digits[int_len..] {
                out.push((b'0' + d) as char);
            }
        }
    } else {
        // 0.00…d0d1…  — leading "0." then (-e10 - 1) zeros then the digits.
        out.push('0');
        out.push('.');
        for _ in 0..(-e10 - 1) {
            out.push('0');
        }
        for &d in digits {
            out.push((b'0' + d) as char);
        }
    }
}

/// Write an unsigned 64-bit integer in decimal.
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

// ── Host KATs (the FAIL-able proof: `cargo test -p rae_json`) ────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;
    use alloc::vec;

    fn approx(a: f64, b: f64) -> bool {
        let d = a - b;
        let d = if d < 0.0 { -d } else { d };
        let scale = if b.abs() > 1.0 { b.abs() } else { 1.0 };
        d <= 1e-9 * scale
    }

    // ── Scalar types parse to exact values ───────────────────────────────────

    #[test]
    fn parse_null_bool() {
        assert_eq!(parse("null"), Ok(Json::Null));
        assert_eq!(parse("true"), Ok(Json::Bool(true)));
        assert_eq!(parse("false"), Ok(Json::Bool(false)));
        // FAIL-guard: true must NOT parse as false.
        assert_ne!(parse("true"), Ok(Json::Bool(false)));
    }

    #[test]
    fn parse_string_value() {
        assert_eq!(parse(r#""hello""#), Ok(Json::String("hello".to_string())));
        assert_eq!(parse(r#""""#), Ok(Json::String(String::new())));
        assert_ne!(parse(r#""hello""#), Ok(Json::String("Hello".to_string())));
    }

    #[test]
    fn parse_numbers_exact() {
        assert_eq!(parse("0").unwrap().as_f64(), Some(0.0));
        assert_eq!(parse("42").unwrap().as_f64(), Some(42.0));
        assert_eq!(parse("-7").unwrap().as_f64(), Some(-7.0));
        assert!(approx(parse("3.14").unwrap().as_f64().unwrap(), 3.14));
        assert!(approx(parse("-0.5").unwrap().as_f64().unwrap(), -0.5));
        // FAIL-guard: -7 must not read as +7.
        assert_ne!(parse("-7").unwrap().as_f64(), Some(7.0));
    }

    #[test]
    fn parse_number_exponents() {
        // If you break exponent handling, these flip.
        assert!(approx(parse("1e3").unwrap().as_f64().unwrap(), 1000.0));
        assert!(approx(parse("1E3").unwrap().as_f64().unwrap(), 1000.0));
        assert!(approx(parse("1.5e2").unwrap().as_f64().unwrap(), 150.0));
        assert!(approx(parse("2e-3").unwrap().as_f64().unwrap(), 0.002));
        assert!(approx(parse("6.022e2").unwrap().as_f64().unwrap(), 602.2));
        assert!(approx(parse("1e+2").unwrap().as_f64().unwrap(), 100.0));
        // FAIL-guard: 1e3 is 1000, not 1 (proves the exponent is applied).
        assert_ne!(parse("1e3").unwrap().as_f64(), Some(1.0));
    }

    #[test]
    fn parse_number_large_small() {
        assert!(parse("1e400").unwrap().as_f64().unwrap().is_infinite());
        assert_eq!(parse("1e-400").unwrap().as_f64(), Some(0.0));
        assert!(approx(
            parse("123456789").unwrap().as_f64().unwrap(),
            123456789.0
        ));
    }

    // ── Arrays / objects, order, accessors ───────────────────────────────────

    #[test]
    fn parse_array() {
        let v = parse("[1,2,3]").unwrap();
        assert_eq!(v.as_array().map(|a| a.len()), Some(3));
        assert_eq!(v.at(0).and_then(Json::as_f64), Some(1.0));
        assert_eq!(v.at(2).and_then(Json::as_f64), Some(3.0));
        assert!(v.at(3).is_none());
        assert_eq!(parse("[]"), Ok(Json::Array(vec![])));
    }

    #[test]
    fn parse_object_preserves_key_order() {
        let v = parse(r#"{"z":1,"a":2,"m":3}"#).unwrap();
        let obj = v.as_object().unwrap();
        let keys: vec::Vec<&str> = obj.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, vec!["z", "a", "m"]); // NOT sorted
        assert_eq!(v.get("a").and_then(Json::as_f64), Some(2.0));
        assert!(v.get("missing").is_none());
        assert_eq!(parse("{}"), Ok(Json::Object(vec![])));
    }

    #[test]
    fn accessors_return_none_on_type_mismatch() {
        let v = parse("42").unwrap();
        assert!(v.as_str().is_none());
        assert!(v.as_bool().is_none());
        assert!(v.as_array().is_none());
        assert!(v.as_object().is_none());
        assert!(v.get("x").is_none());
        assert!(v.at(0).is_none());
        assert!(parse("null").unwrap().is_null());
    }

    // ── Escapes ──────────────────────────────────────────────────────────────

    #[test]
    fn parse_simple_escapes() {
        let v = parse(r#""a\"b\\c\/d\b\f\n\r\te""#).unwrap();
        assert_eq!(v.as_str(), Some("a\"b\\c/d\u{0008}\u{000C}\n\r\te"));
    }

    #[test]
    fn parse_unicode_bmp_escape() {
        // é = é, € = €.
        assert_eq!(parse(r#""é""#).unwrap().as_str(), Some("é"));
        assert_eq!(parse(r#""€""#).unwrap().as_str(), Some("€"));
        // FAIL-guard: if the \u decoder is broken this won't equal é.
        assert_ne!(parse(r#""é""#).unwrap().as_str(), Some("e"));
    }

    #[test]
    fn parse_surrogate_pair_emoji() {
        // U+1F600 GRINNING FACE = surrogate pair D83D DE00.
        let v = parse(r#""😀""#).unwrap();
        assert_eq!(v.as_str(), Some("\u{1F600}"));
        // FAIL-guard: a broken surrogate combiner would not fold the pair into a
        // single U+1F600 scalar. The pair must become ONE char (4 UTF-8 bytes),
        // not two replacement/surrogate chars.
        assert_eq!(v.as_str().unwrap().chars().count(), 1);
        assert_eq!(v.as_str().unwrap().len(), 4); // UTF-8 byte length of U+1F600
    }

    // ── Round-trips ──────────────────────────────────────────────────────────

    #[test]
    fn nested_round_trip() {
        let src = r#"{"name":"Rae","tags":["os","gaming"],"meta":{"v":2,"beta":false,"score":3.5},"empty":[],"nil":null}"#;
        let v = parse(src).unwrap();
        let s = to_string(&v);
        let v2 = parse(&s).unwrap();
        assert_eq!(v, v2);
        // And key order survives the round-trip.
        let keys: vec::Vec<&str> = v2
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, _)| k.as_str())
            .collect();
        assert_eq!(keys, vec!["name", "tags", "meta", "empty", "nil"]);
    }

    #[test]
    fn serialize_escapes_round_trip() {
        let v = Json::String("tab\there\nnewline\"quote\\slash".to_string());
        let s = to_string(&v);
        assert_eq!(parse(&s).unwrap(), v);
    }

    #[test]
    fn serialize_number_formats() {
        assert_eq!(to_string(&Json::Number(42.0)), "42");
        assert_eq!(to_string(&Json::Number(-7.0)), "-7");
        assert_eq!(to_string(&Json::Number(0.0)), "0");
        assert_eq!(to_string(&Json::Number(3.5)), "3.5");
        // non-finite -> null (still valid JSON)
        assert_eq!(to_string(&Json::Number(f64::NAN)), "null");
        assert_eq!(to_string(&Json::Number(f64::INFINITY)), "null");
    }

    /// BUG #1 (fuzz-found): large finite doubles must serialize to a string that
    /// re-parses to the SAME f64 — they used to saturate `(x as u128)` and emit
    /// garbage off by ~130 orders of magnitude (`"374607431768211712"` for
    /// `1.1e147`). RED on the old `write_number`, GREEN on the shortest formatter.
    #[test]
    fn serialize_large_doubles_round_trip() {
        let cases = [
            parse("11E146").unwrap().as_f64().unwrap(), // 1.1e147 — the fuzz repro
            1e300,
            -1e300,
            1e-300,
            1.7976931348623157e308, // ~f64::MAX
            5e-324,                 // f64::MIN_POSITIVE subnormal
            9.007199254740992e18,   // > 1e18 integer (left the int fast path)
            1234567890123456789.0,
            -1e18,
            1e21, // first magnitude that switches to scientific in JS-style output
            1e-7, // first magnitude that switches to scientific (small side)
        ];
        for &x in &cases {
            let s = to_string(&Json::Number(x));
            let back = parse(&s).unwrap().as_f64().unwrap();
            assert_eq!(
                back.to_bits(),
                x.to_bits(),
                "large-double round-trip failed: {x:?} -> {s:?} -> {back:?}"
            );
        }
        // FAIL-guard: the old garbage output for 1.1e147 must NOT reappear.
        let s = to_string(&Json::Number(parse("11E146").unwrap().as_f64().unwrap()));
        assert_ne!(s, "374607431768211712");
    }

    /// BUG #2 (fuzz-found): fractional values must be serialize-idempotent. The old
    /// formatter printed non-shortest digit noise (`84.34` -> `"84.340000000000102"`)
    /// that drifted under repeat serialize→parse. RED before, GREEN after.
    #[test]
    fn serialize_fractional_idempotent() {
        let cases = [
            84.34,
            0.1,
            0.2,
            0.3,
            3.14159,
            2.5,
            0.125,
            -7.5,
            123.456,
            1.0 / 3.0,
        ];
        for &x in &cases {
            let s1 = to_string(&Json::Number(x));
            // (a) round-trips exactly through the string …
            let back = parse(&s1).unwrap().as_f64().unwrap();
            assert_eq!(
                back.to_bits(),
                x.to_bits(),
                "fractional round-trip failed: {x:?} -> {s1:?} -> {back:?}"
            );
            // (b) … and is idempotent: serialize→parse→serialize is a fixed point.
            let s2 = to_string(&parse(&s1).unwrap());
            assert_eq!(
                s1, s2,
                "serializer not idempotent for {x:?}: {s1:?} != {s2:?}"
            );
        }
        // FAIL-guard: the specific old-noise outputs must NOT reappear.
        let s = to_string(&Json::Number(84.34));
        assert_ne!(s, "84.340000000000102");
        assert_ne!(s, "84.340000000000017");
        assert_eq!(s, "84.34"); // shortest faithful form
    }

    /// Shortest-form sanity: simple values produce the minimal canonical string.
    #[test]
    fn serialize_shortest_canonical() {
        assert_eq!(to_string(&Json::Number(0.1)), "0.1");
        assert_eq!(to_string(&Json::Number(3.5)), "3.5");
        assert_eq!(to_string(&Json::Number(100.0)), "100");
        assert_eq!(to_string(&Json::Number(0.5)), "0.5");
    }

    #[test]
    fn float_round_trips_through_string() {
        for &x in &[0.0, 1.0, -1.0, 3.14159, -2.5, 1000.0, 0.125, 123.456] {
            let s = to_string(&Json::Number(x));
            let back = parse(&s).unwrap().as_f64().unwrap();
            assert!(approx(back, x), "round-trip {} -> {} -> {}", x, s, back);
        }
    }

    // ── Whitespace tolerance ─────────────────────────────────────────────────

    #[test]
    fn whitespace_tolerated() {
        let v = parse("  {  \"a\" : [ 1 , 2 ] , \"b\" : true }  \n").unwrap();
        assert_eq!(
            v.get("a").and_then(|j| j.as_array().map(|a| a.len())),
            Some(2)
        );
        assert_eq!(v.get("b").and_then(Json::as_bool), Some(true));
    }

    // ── Depth limit (stack safety) ───────────────────────────────────────────

    #[test]
    fn deep_nesting_within_limit_ok() {
        let mut s = String::new();
        for _ in 0..100 {
            s.push('[');
        }
        s.push('1');
        for _ in 0..100 {
            s.push(']');
        }
        assert!(parse(&s).is_ok());
    }

    #[test]
    fn pathological_nesting_rejected_not_panic() {
        let mut s = String::new();
        for _ in 0..(MAX_DEPTH + 50) {
            s.push('[');
        }
        // never closed; must Err (DepthExceeded), never panic / never overflow stack
        assert_eq!(parse(&s), Err(JsonError::DepthExceeded));
    }

    // ── Malformed battery: ALL must Err, ZERO panics ─────────────────────────

    #[test]
    fn malformed_battery_is_err_not_panic() {
        let bad = [
            "",                 // empty
            "   ",              // whitespace only
            "{",                // unclosed brace
            "[",                // unclosed bracket
            "[1,2",             // unterminated array
            "{\"a\":1",         // unterminated object
            "{\"a\":1,}",       // trailing comma (object)
            "[1,2,]",           // trailing comma (array)
            "[1,,2]",           // double comma
            "\"unterminated",   // unclosed string
            "\"bad\\xescape\"", // bad escape
            "\"\\u00\"",        // short \u escape
            "\"\\uZZZZ\"",      // non-hex \u
            "\"\\uD83D\"",      // lone high surrogate
            "\"\\uDE00\"",      // lone low surrogate
            "\"\\uD83Dx\"",     // high surrogate not followed by \u
            "tru",              // bare/partial word
            "True",             // wrong case
            "nul",              // partial null
            "01",               // leading zero
            "-",                // lone minus
            "1.",               // trailing decimal point
            ".5",               // leading decimal point (not valid JSON)
            "1e",               // exponent without digits
            "1e+",              // exponent sign without digits
            "+1",               // leading plus not allowed
            "1 2",              // trailing data
            "{1:2}",            // non-string key
            "}",                // stray close
            "[}",               // mismatched close
            "\"\u{0001}\"",     // raw control char in string
        ];
        for case in bad.iter() {
            let r = parse(case);
            assert!(
                r.is_err(),
                "expected Err for malformed input {:?}, got {:?}",
                case,
                r
            );
        }
    }

    #[test]
    fn leading_zero_specifically_rejected() {
        assert_eq!(parse("01"), Err(JsonError::BadNumber));
        assert_eq!(parse("00"), Err(JsonError::BadNumber));
        // but a lone 0 and 0.x are fine
        assert_eq!(parse("0").unwrap().as_f64(), Some(0.0));
        assert!(approx(parse("0.5").unwrap().as_f64().unwrap(), 0.5));
    }

    #[test]
    fn manifest_shaped_document() {
        // Shaped like a RaeStore manifest — the real-world use this serves.
        let src = r#"{
            "name": "com.raeen.example",
            "version": "1.2.3",
            "permissions": ["net", "fs.read"],
            "window": {"width": 1280, "height": 720, "resizable": true},
            "icon": null
        }"#;
        let v = parse(src).unwrap();
        assert_eq!(
            v.get("name").and_then(Json::as_str),
            Some("com.raeen.example")
        );
        assert_eq!(
            v.get("permissions")
                .and_then(Json::as_array)
                .map(|a| a.len()),
            Some(2)
        );
        assert_eq!(
            v.get("window")
                .and_then(|w| w.get("width"))
                .and_then(Json::as_f64),
            Some(1280.0)
        );
        assert!(v.get("icon").map(Json::is_null).unwrap_or(false));
        // round-trips
        assert_eq!(parse(&to_string(&v)).unwrap(), v);
    }
}

// ════════════════════════════════════════════════════════════════════════════
// FUZZ / PROPERTY suite — deterministic seeded PRNG, no external fuzz crate.
//
// Matches the rae_mime/rae_toml/rae_deflate pattern. The properties under test
// are the hostile-input invariants of [`parse`] (untrusted API/config JSON): on
// ANY input it must (a) never panic, and (b) bound recursion at MAX_DEPTH so a
// crafted deeply-nested document cannot blow the stack. Plus the round-trip
// property `parse(to_string(parse(x))) == parse(x)` over the parsed corpus.
//
// `parse` takes `&str`, so bad-UTF-8 bytes are exercised the only way the public
// API can receive them: via `core::str::from_utf8` (lone surrogates / overlong /
// truncated sequences are rejected at the &str boundary before parse sees them;
// in-string lone-surrogate ESCAPES `\uD800` are the parser's job and are fuzzed
// directly).
//
// FAIL-ability (proven by reasoning, see REPORT):
//  - If any parse path could panic on hostile input (an unchecked index, an
//    `unwrap`, a slice past the end, an arithmetic overflow in debug) the
//    never-panic loops abort the test process — the test goes red.
//    (#![forbid(unsafe_code)] makes any OOB index a guaranteed panic, not silent
//    UB, so these loops genuinely prove bounds-safety.)
//  - If the MAX_DEPTH recursion bound were removed, `deep_nesting_no_stack_overflow`
//    would recurse thousands of levels and overflow the stack (SIGSEGV / abort =
//    test failure) instead of returning DepthExceeded.
//  - If the exponent saturation in `scale_pow10` were removed, the huge-number
//    fuzz would loop 1e9999 times (effective hang / timeout = test failure).
//  - If the parser accepted trailing garbage, the `trailing_garbage_rejected`
//    cases would return Ok and the `is_err()` asserts flip.
//  - If `parse`/`to_string` disagreed on any value, the round-trip property
//    `assert_eq!(reparse, v)` flips for the offending generated input.
// ════════════════════════════════════════════════════════════════════════════
#[cfg(test)]
mod fuzz {
    use super::*;
    use alloc::string::String;
    use alloc::string::ToString;
    use alloc::vec::Vec;

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

    /// 3a. Token soup: random sequences of JSON-significant ASCII characters
    /// never panic.
    #[test]
    fn fuzz_token_soup_never_panic() {
        // The alphabet of structurally-meaningful JSON bytes plus a little noise.
        const ALPHABET: &[u8] = b"{}[]\":,0123456789-+.eEtrufalsn \t\n\\u/x ";
        let mut rng = Rng::new(0x5_0117);
        for _ in 0..100_000 {
            let len = rng.below(64);
            let mut s = String::with_capacity(len);
            for _ in 0..len {
                let c = ALPHABET[rng.below(ALPHABET.len())] as char;
                s.push(c);
            }
            let _ = parse(&s); // Ok or Err, never panic.
        }
    }

    /// 3b. Random valid-UTF-8 strings (full ASCII printable + some multibyte)
    /// never panic — covers control chars, quotes, backslashes in arbitrary spots.
    #[test]
    fn fuzz_random_utf8_never_panic() {
        let mut rng = Rng::new(0x5_0317);
        let palette: &[char] = &[
            '"', '\\', '/', '{', '}', '[', ']', ':', ',', 'a', '1', '.', 'e', '-', '+', '\n', '\t',
            '\u{0001}', 'é', '€', '😀', 'u', 'D', '8', 'F',
        ];
        for _ in 0..100_000 {
            let len = rng.below(48);
            let mut s = String::new();
            for _ in 0..len {
                s.push(palette[rng.below(palette.len())]);
            }
            let _ = parse(&s);
        }
    }

    /// 3c. Mutate a well-formed document byte-wise (UTF-8-safe via char swaps):
    /// truncations, swapped delimiters, broken escapes — never panic.
    #[test]
    fn fuzz_mutated_valid_document_never_panic() {
        let seed = r#"{"name":"Rae","tags":["os","gaming"],"n":42,"f":3.14,"e":1e5,"b":true,"z":null,"nested":{"a":[1,{"b":[2,3]}]},"esc":"a\"b\\cé😀"}"#;
        // Sanity: the seed parses.
        assert!(parse(seed).is_ok());
        let chars: Vec<char> = seed.chars().collect();
        let mut rng = Rng::new(0x5_0517);
        for _ in 0..100_000 {
            let mut c = chars.clone();
            let muts = 1 + rng.below(3);
            for _ in 0..muts {
                let op = rng.below(3);
                let i = rng.below(c.len());
                match op {
                    0 => {
                        // truncate at i
                        c.truncate(i);
                    }
                    1 => {
                        // replace with a random structural char
                        const REPL: &[char] = &[
                            '{', '}', '[', ']', '"', ':', ',', '\\', 'u', '0', 'e', '-', ' ',
                        ];
                        c[i] = REPL[rng.below(REPL.len())];
                    }
                    _ => {
                        // duplicate
                        let ch = c[i];
                        c.insert(i, ch);
                    }
                }
                if c.is_empty() {
                    break;
                }
            }
            let s: String = c.into_iter().collect();
            let _ = parse(&s);
        }
    }

    /// 3d. Truncate the valid seed at EVERY char boundary: never panic.
    #[test]
    fn fuzz_truncate_at_every_boundary() {
        let seed = r#"{"a":[1,2,{"b":"cé😀","d":[true,false,null,-1.5e3]}],"e":{}}"#;
        let chars: Vec<char> = seed.chars().collect();
        for cut in 0..=chars.len() {
            let s: String = chars[..cut].iter().collect();
            let _ = parse(&s);
        }
    }

    /// 3e. Deep nesting must hit the depth cap, never overflow the stack. Tests
    /// both arrays and objects, far past MAX_DEPTH.
    #[test]
    fn fuzz_deep_nesting_no_stack_overflow() {
        // Arrays.
        let mut s = String::new();
        for _ in 0..(MAX_DEPTH * 4) {
            s.push('[');
        }
        assert_eq!(parse(&s), Err(JsonError::DepthExceeded));
        // Objects.
        let mut o = String::new();
        for _ in 0..(MAX_DEPTH * 4) {
            o.push_str("{\"a\":");
        }
        assert_eq!(parse(&o), Err(JsonError::DepthExceeded));
        // Mixed, even deeper.
        let mut m = String::new();
        for i in 0..(MAX_DEPTH * 4) {
            m.push(if i % 2 == 0 { '[' } else { '{' });
            if i % 2 == 1 {
                m.push_str("\"k\":");
            }
        }
        assert_eq!(parse(&m), Err(JsonError::DepthExceeded));
        // Just within the limit must succeed (proves the cap isn't over-tight).
        let mut ok = String::new();
        for _ in 0..(MAX_DEPTH - 2) {
            ok.push('[');
        }
        ok.push('1');
        for _ in 0..(MAX_DEPTH - 2) {
            ok.push(']');
        }
        assert!(parse(&ok).is_ok());
    }

    /// 3f. Huge numbers / huge exponents must terminate (saturated), never hang.
    #[test]
    fn fuzz_huge_numbers_terminate() {
        // A 5000-digit integer, a 5000-digit fraction, and absurd exponents.
        let mut big = String::new();
        for _ in 0..5000 {
            big.push('9');
        }
        assert!(parse(&big).unwrap().as_f64().unwrap().is_infinite());

        let mut frac = String::from("0.");
        for _ in 0..5000 {
            frac.push('1');
        }
        assert!(parse(&frac).is_ok());

        assert!(parse("1e999999999")
            .unwrap()
            .as_f64()
            .unwrap()
            .is_infinite());
        assert_eq!(parse("1e-999999999").unwrap().as_f64(), Some(0.0));
        // saturating exponent accumulation must not overflow i32 in debug.
        let mut e = String::from("1e");
        for _ in 0..5000 {
            e.push('9');
        }
        assert!(parse(&e).unwrap().as_f64().unwrap().is_infinite());
    }

    /// 3g. Lone surrogate escapes / bad escapes are rejected, never panic.
    #[test]
    fn fuzz_surrogate_and_escape_battery() {
        let bad = [
            r#""\uD800""#,       // lone high surrogate
            r#""\uDC00""#,       // lone low surrogate
            r#""\uD800A""#,      // high not followed by low
            r#""\uD800\uD800""#, // two highs
            r#""\uDFFF\uDFFF""#, // two lows
            r#""\uD83D""#,       // truncated pair
            r#""\uD83D\u""#,     // truncated low
            r#""\uZZZZ""#,       // non-hex
            r#""\x""#,           // unknown escape
            r#""\""#,            // dangling escape (then EOF)
            r#""\u123""#,        // short \u
        ];
        for case in bad {
            let r = parse(case);
            assert!(r.is_err(), "expected Err for {case:?}, got {r:?}");
        }
        // A VALID surrogate pair must still succeed (proves the rejection above is
        // surrogate-specific, not a blanket \u failure).
        assert_eq!(parse(r#""😀""#).unwrap().as_str(), Some("😀"));
    }

    /// 3h. Trailing garbage after a complete value is rejected.
    #[test]
    fn fuzz_trailing_garbage_rejected() {
        for case in [
            "42 garbage",
            "true false",
            "{}{}",
            "[1,2,3]extra",
            "\"str\"\"str\"",
            "null null",
            "1.0 2.0",
        ] {
            assert!(parse(case).is_err(), "trailing garbage accepted: {case:?}");
        }
    }

    /// 3i. Duplicate keys are accepted (order-preserving Vec) and round-trip; the
    /// `get` accessor returns the FIRST match. Proves the duplicate-key policy is
    /// stable and panic-free.
    #[test]
    fn fuzz_duplicate_keys_stable() {
        let v = parse(r#"{"a":1,"a":2,"a":3}"#).unwrap();
        let obj = v.as_object().unwrap();
        assert_eq!(obj.len(), 3, "all duplicate keys preserved");
        assert_eq!(v.get("a").and_then(Json::as_f64), Some(1.0), "get = first");
        // round-trips (all three entries survive).
        let s = to_string(&v);
        assert_eq!(parse(&s).unwrap(), v);
    }

    /// 3j. Round-trip property over the fuzz corpus. Every input that parses must:
    ///   (a) re-parse from its serialization without error (the never-panic /
    ///       valid-output guarantee — holds for ALL values, every magnitude); and
    ///   (b) for EVERY finite value (no magnitude restriction), satisfy serializer
    ///       idempotency `to_string(parse(to_string(v))) == to_string(v)`.
    ///
    /// Property (b) was previously restricted to exact-integer Numbers because the
    /// old `write_number` was NOT a faithful (shortest) formatter — large finite
    /// doubles serialized to garbage and fractional values printed digit noise.
    /// The shortest-round-trip formatter (`write_shortest`) fixed both, so (b) now
    /// guards the WHOLE finite number surface: it is RED on the old serializer and
    /// GREEN on the fixed one.
    #[test]
    fn fuzz_roundtrip_property() {
        const ALPHABET: &[u8] = b"{}[]\":,0123456789-.eEtrufalsn \t";
        let mut rng = Rng::new(0x5_0717);
        let mut reparses = 0u32;
        let mut idempotent_checks = 0u32;
        for _ in 0..200_000 {
            let len = rng.below(48);
            let mut s = String::with_capacity(len);
            for _ in 0..len {
                s.push(ALPHABET[rng.below(ALPHABET.len())] as char);
            }
            if let Ok(v) = parse(&s) {
                // (a) to_string is total; the re-parse must SUCCEED for every value.
                let serialized = to_string(&v);
                let reparsed = parse(&serialized);
                assert!(
                    reparsed.is_ok(),
                    "to_string output failed to re-parse: input={:?} out={:?} err={:?}",
                    s,
                    serialized,
                    reparsed
                );
                let reparsed = reparsed.unwrap();
                reparses += 1;
                // (b) Serializer idempotency over ALL finite numbers (no range cap).
                if all_numbers_finite(&v) {
                    assert_eq!(
                        to_string(&reparsed),
                        serialized,
                        "serializer not idempotent for input {:?} (serialized {:?})",
                        s,
                        serialized
                    );
                    idempotent_checks += 1;
                }
            }
        }
        // Both arms must have real coverage, else the property is vacuous (a false
        // green): the alphabet yields plenty of bare scalars and numbers.
        assert!(
            reparses > 1000,
            "re-parse corpus was nearly empty ({reparses}); property (a) vacuous"
        );
        assert!(
            idempotent_checks > 200,
            "idempotency corpus was nearly empty ({idempotent_checks}); property (b) vacuous"
        );
    }

    /// True if every Number in `v` is finite (the shortest formatter's full domain;
    /// non-finite Numbers serialize to `null` and are excluded from idempotency by
    /// design — `null` re-parses to Null, a deliberate lossy mapping).
    fn all_numbers_finite(v: &Json) -> bool {
        match v {
            Json::Number(n) => n.is_finite(),
            Json::Array(items) => items.iter().all(all_numbers_finite),
            Json::Object(m) => m.iter().all(|(_, val)| all_numbers_finite(val)),
            _ => true,
        }
    }

    /// 3k. Explicit construct → serialize → parse → equal, across all value kinds
    /// including nested + escapes + edge numbers.
    #[test]
    fn fuzz_constructed_value_roundtrip() {
        use alloc::vec;
        let values = [
            Json::Null,
            Json::Bool(true),
            Json::Bool(false),
            Json::Number(0.0),
            Json::Number(-1.0),
            Json::Number(42.0),
            Json::Number(3.5),
            Json::Number(123456.0),
            Json::String(String::new()),
            Json::String("hello\nworld\t\"q\"\\s/é😀".to_string()),
            Json::Array(vec![Json::Number(1.0), Json::Bool(false), Json::Null]),
            Json::Object(vec![
                ("k".to_string(), Json::Number(1.0)),
                ("nested".to_string(), Json::Array(vec![Json::Null])),
            ]),
        ];
        for v in values {
            let s = to_string(&v);
            let back = parse(&s).unwrap_or_else(|e| panic!("reparse {s:?}: {e:?}"));
            assert_eq!(back, v, "round-trip mismatch for {s:?}");
        }
    }
}
