//! # RaePdf — a never-panic, `no_std` PDF reader (PDF 1.4–1.7 subset).
//!
//! LEGACY_GAMING_CONCEPT.md §Compatibility Strategy ("how to actually win" — let people
//! switch without conscious effort) and criterion #5, "open my PDFs": a Windows/
//! macOS switcher arrives with a folder of PDFs, and "does my OS open my files" is
//! table stakes for a daily driver. This crate is the from-scratch **reader** the
//! Files Quick Look preview, a future viewer, and the "just show me the text" path
//! sit on. It is the inverse of athprint's `pdf.rs` **generator**: that module
//! *writes* the COS object graph this module *reads*, so a round-trip
//! `read(write(doc))` recovers the same pages and text.
//!
//! ## What a PDF is
//! A PDF is a **COS object graph** — booleans, numbers (int/real), strings (literal
//! `(…)` with escapes / hex `<…>`), names (`/Name` with `#XX`), arrays `[…]`,
//! dictionaries `<<…>>`, streams (`stream … endstream`), `null`, and indirect
//! objects `N G obj … endobj` referenced by `N G R`. The file ends with a `trailer`
//! / `startxref` pointing at a **cross-reference table** (classic `xref`) or a
//! **cross-reference stream** (PDF 1.5+, type 0/1/2 entries). `/Root → /Pages`
//! is a page tree (`/Kids`, with `/Resources` / `/MediaBox` inherited down).
//! Page `/Contents` streams are usually **FlateDecode**'d.
//!
//! ## What it models vs defers
//! - **Object parser:** the full COS object model above (booleans, int/real,
//!   literal+hex strings, names with `#XX`, arrays, dicts, streams, null, indirect
//!   refs and objects).
//! - **Cross-reference:** classic `xref` subsections AND xref **streams** (PDF 1.5+),
//!   type 0/1/2 entries. `/Prev` incremental-update chains are followed up to a
//!   bounded depth (newest wins); deeper chains are truncated (documented deferral),
//!   never an infinite loop.
//! - **Streams:** `/Filter /FlateDecode` (the overwhelmingly common case) via
//!   [`ath_deflate::zlib_decompress`], `/Length` direct or indirect. Other filters
//!   (LZW, DCT/JPEG, ASCII85, ASCIIHex, RunLength, CCITT) are **deferred**:
//!   [`Document::decoded_stream`] returns [`PdfError::UnsupportedFilter`] for them —
//!   honest, never faked.
//! - **Text extraction:** content-stream operators `BT`/`ET`, `Tj`/`TJ`/`'`/`"`,
//!   positioning `Td`/`TD`/`Tm`/`T*` (used heuristically to insert spaces/newlines),
//!   and `Tf` (font select). Single-byte WinAnsi/Standard encoding and ASCII pass
//!   through correctly. **Deferred:** CID/Type0/ToUnicode-CMap multi-byte fonts —
//!   we extract what the single-byte path can and never crash.
//!
//! ## Hostile-input posture (CLAUDE: PDF is a notorious parser-exploitation surface)
//! Every byte handed to [`Document::open`] is attacker-controlled. There is **no
//! `unwrap`/`expect`/`panic`/raw-index-panic** path reachable from this crate, and
//! every amplification vector is bounded *before* allocation:
//!   - total parsed-object count ([`MAX_OBJECTS`]),
//!   - xref entry count ([`MAX_XREF_ENTRIES`]),
//!   - decompressed stream size (delegated to ath_deflate's [`ath_deflate::MAX_OUTPUT`]),
//!   - nested dict/array recursion depth ([`MAX_DEPTH`]),
//!   - indirect-reference resolution depth ([`MAX_REF_DEPTH`]) — defeats reference
//!     cycles,
//!   - `/Prev` xref-chain depth ([`MAX_XREF_CHAIN`]),
//!   - page-tree node count + depth ([`MAX_PAGES`] / [`MAX_DEPTH`]).
//! A malformed/truncated/hostile file returns `Err(PdfError)` — never a panic, never
//! an infinite loop.
//!
//! The host KAT suite at the bottom (`cargo test -p ath_pdf`) is the proof: it
//! hand-assembles minimal valid PDFs in-test (classic `xref` AND an xref **stream**,
//! uncompressed AND FlateDecode'd content via ath_deflate's compressor) and asserts
//! exact page counts and extracted text (`"Hello, AthenaOS!"`), TJ-array kerning
//! reassembly, literal-escape + hex-string decoding, and a hostile battery
//! (non-PDF, truncated, bad startxref, reference cycle, huge object count, seeded
//! fuzz) that all return `Err` with zero panics.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

// ─── Limits (resource-exhaustion guards) ─────────────────────────────────────

/// Largest number of indirect objects we will track for one document.
pub const MAX_OBJECTS: usize = 1 << 20; // 1,048,576

/// Largest number of cross-reference entries (classic + stream) we will accept.
pub const MAX_XREF_ENTRIES: usize = 1 << 21;

/// Largest dict/array nesting depth the object parser will descend.
pub const MAX_DEPTH: usize = 200;

/// Largest indirect-reference resolution chain — defeats `R`-cycles.
pub const MAX_REF_DEPTH: usize = 64;

/// Largest `/Prev` incremental-update xref chain we will follow.
pub const MAX_XREF_CHAIN: usize = 64;

/// Largest number of pages we will enumerate from the page tree.
pub const MAX_PAGES: usize = 1 << 16;

/// Largest total content-stream bytes we will scan for text across a document
/// (work cap, independent of the per-stream decompression bound).
pub const MAX_TEXT_WORK: usize = 256 * 1024 * 1024;

// ─── Errors ──────────────────────────────────────────────────────────────────

/// A PDF read error. Every variant is a *handled* path — nothing here panics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PdfError {
    /// The bytes don't start with a `%PDF-` header.
    NotPdf,
    /// No `startxref` / `%%EOF` trailer was found at the end of the file.
    NoStartxref,
    /// The cross-reference table/stream was malformed or pointed out of range.
    BadXref,
    /// The trailer dictionary was missing or lacked a usable `/Root`.
    BadTrailer,
    /// The document `/Root` → `/Pages` page tree was missing or malformed.
    BadPageTree,
    /// A structural value was malformed (an object, number, string, name, dict…).
    Malformed,
    /// A resource bound ([`MAX_OBJECTS`] / [`MAX_XREF_ENTRIES`] / [`MAX_PAGES`] …)
    /// was exceeded.
    TooLarge,
    /// Nesting / reference / xref-chain recursion exceeded a depth bound.
    TooDeep,
    /// A stream used a `/Filter` this reader does not implement (LZW, DCT, ASCII85,
    /// …). Honest deferral — the raw stream is still reachable via [`Document::raw_stream`].
    UnsupportedFilter,
    /// A FlateDecode stream failed to decompress.
    BadStream,
}

// ════════════════════════════════════════════════════════════════════════════
// COS object model
// ════════════════════════════════════════════════════════════════════════════

/// A reference to an indirect object: `(object number, generation number)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ObjRef {
    pub num: u32,
    pub gen: u16,
}

/// A parsed COS object.
#[derive(Debug, Clone, PartialEq)]
pub enum Object {
    /// `null`.
    Null,
    /// `true` / `false`.
    Boolean(bool),
    /// An integer number.
    Integer(i64),
    /// A real number. PDF reals are decimal; we keep f64 for arithmetic, but the
    /// reader never relies on exact float equality for control flow.
    Real(f64),
    /// A string — already de-escaped (literal) or hex-decoded into raw bytes.
    StringLit(Vec<u8>),
    /// A name (`/Foo`), with `#XX` escapes decoded, without the leading slash.
    Name(String),
    /// An array.
    Array(Vec<Object>),
    /// A dictionary.
    Dictionary(Dict),
    /// A stream: its dictionary plus the raw (still-encoded) stream bytes.
    Stream(Stream),
    /// An indirect reference `N G R`.
    Reference(ObjRef),
}

/// A COS dictionary: name → object. Insertion-independent lookup by name.
pub type Dict = BTreeMap<String, Object>;

/// A COS stream object: its dictionary and the raw (encoded) body bytes.
#[derive(Debug, Clone, PartialEq)]
pub struct Stream {
    pub dict: Dict,
    /// Raw stream bytes exactly as they appeared between `stream`/`endstream`
    /// (pre-filter). Decode via [`Document::decoded_stream`].
    pub raw: Vec<u8>,
}

impl Object {
    /// Borrow as an integer (accepting an integral real too).
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Object::Integer(i) => Some(*i),
            // Integral real (no `f64::fract`, unavailable in core/no_std): a real
            // is integral iff casting to i64 and back is lossless.
            Object::Real(r) => {
                let t = *r as i64;
                if t as f64 == *r {
                    Some(t)
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// Borrow as a name string.
    pub fn as_name(&self) -> Option<&str> {
        match self {
            Object::Name(n) => Some(n.as_str()),
            _ => None,
        }
    }

    /// Borrow as a dictionary (also yields a stream's dict).
    pub fn as_dict(&self) -> Option<&Dict> {
        match self {
            Object::Dictionary(d) => Some(d),
            Object::Stream(s) => Some(&s.dict),
            _ => None,
        }
    }

    /// Borrow as an array.
    pub fn as_array(&self) -> Option<&[Object]> {
        match self {
            Object::Array(a) => Some(a),
            _ => None,
        }
    }
}

// ════════════════════════════════════════════════════════════════════════════
// Lexer / object parser (the COS tokenizer)
// ════════════════════════════════════════════════════════════════════════════

#[inline]
fn is_ws(b: u8) -> bool {
    matches!(b, b'\0' | b'\t' | b'\n' | 0x0C | b'\r' | b' ')
}

#[inline]
fn is_delim(b: u8) -> bool {
    matches!(
        b,
        b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%'
    )
}

#[inline]
fn is_regular(b: u8) -> bool {
    !is_ws(b) && !is_delim(b)
}

/// A bounded byte cursor over the PDF, used by the object parser. Never indexes
/// out of range: every read goes through `get`.
struct Lexer<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Lexer<'a> {
    fn new(data: &'a [u8], pos: usize) -> Self {
        Lexer { data, pos }
    }

    #[inline]
    fn peek(&self) -> Option<u8> {
        self.data.get(self.pos).copied()
    }

    #[inline]
    fn peek_at(&self, off: usize) -> Option<u8> {
        self.data.get(self.pos + off).copied()
    }

    #[inline]
    fn bump(&mut self) -> Option<u8> {
        let b = self.data.get(self.pos).copied();
        if b.is_some() {
            self.pos += 1;
        }
        b
    }

    /// Skip whitespace and `%`-comments (to end of line).
    fn skip_ws(&mut self) {
        loop {
            match self.peek() {
                Some(b) if is_ws(b) => {
                    self.pos += 1;
                }
                Some(b'%') => {
                    // Comment to end of line.
                    while let Some(b) = self.peek() {
                        self.pos += 1;
                        if b == b'\n' || b == b'\r' {
                            break;
                        }
                    }
                }
                _ => break,
            }
        }
    }

    /// Does the upcoming text match `kw` exactly (followed by a non-regular char
    /// or EOF)? If so, consume it and return true.
    fn eat_keyword(&mut self, kw: &[u8]) -> bool {
        let end = self.pos + kw.len();
        if end > self.data.len() {
            return false;
        }
        if &self.data[self.pos..end] != kw {
            return false;
        }
        // Keyword boundary: next byte must not be a regular char.
        if let Some(&next) = self.data.get(end) {
            if is_regular(next) {
                return false;
            }
        }
        self.pos = end;
        true
    }

    /// Parse one object at the current position with a depth bound.
    fn parse_object(&mut self, depth: usize) -> Result<Object, PdfError> {
        if depth > MAX_DEPTH {
            return Err(PdfError::TooDeep);
        }
        self.skip_ws();
        let b = self.peek().ok_or(PdfError::Malformed)?;
        match b {
            b'/' => self.parse_name(),
            b'(' => self.parse_literal_string(),
            b'[' => self.parse_array(depth),
            b'<' => {
                if self.peek_at(1) == Some(b'<') {
                    self.parse_dict_or_stream(depth)
                } else {
                    self.parse_hex_string()
                }
            }
            b'+' | b'-' | b'.' | b'0'..=b'9' => self.parse_number_or_ref(),
            b't' | b'f' | b'n' => self.parse_keyword_literal(),
            _ => Err(PdfError::Malformed),
        }
    }

    fn parse_keyword_literal(&mut self) -> Result<Object, PdfError> {
        if self.eat_keyword(b"true") {
            Ok(Object::Boolean(true))
        } else if self.eat_keyword(b"false") {
            Ok(Object::Boolean(false))
        } else if self.eat_keyword(b"null") {
            Ok(Object::Null)
        } else {
            Err(PdfError::Malformed)
        }
    }

    fn parse_name(&mut self) -> Result<Object, PdfError> {
        // Consume leading '/'.
        if self.bump() != Some(b'/') {
            return Err(PdfError::Malformed);
        }
        let mut out = String::new();
        while let Some(b) = self.peek() {
            if !is_regular(b) {
                break;
            }
            self.pos += 1;
            if b == b'#' {
                // Two hex digits follow.
                let h = self.bump().ok_or(PdfError::Malformed)?;
                let l = self.bump().ok_or(PdfError::Malformed)?;
                let hv = hex_val(h).ok_or(PdfError::Malformed)?;
                let lv = hex_val(l).ok_or(PdfError::Malformed)?;
                out.push((hv << 4 | lv) as char);
            } else {
                out.push(b as char);
            }
        }
        Ok(Object::Name(out))
    }

    fn parse_literal_string(&mut self) -> Result<Object, PdfError> {
        // Consume '('.
        if self.bump() != Some(b'(') {
            return Err(PdfError::Malformed);
        }
        let mut out: Vec<u8> = Vec::new();
        let mut nest: u32 = 1;
        loop {
            let b = self.bump().ok_or(PdfError::Malformed)?;
            match b {
                b'\\' => {
                    let e = self.bump().ok_or(PdfError::Malformed)?;
                    match e {
                        b'n' => out.push(b'\n'),
                        b'r' => out.push(b'\r'),
                        b't' => out.push(b'\t'),
                        b'b' => out.push(0x08),
                        b'f' => out.push(0x0C),
                        b'(' => out.push(b'('),
                        b')' => out.push(b')'),
                        b'\\' => out.push(b'\\'),
                        b'\r' => {
                            // Line continuation: backslash-CRLF or backslash-CR.
                            if self.peek() == Some(b'\n') {
                                self.pos += 1;
                            }
                        }
                        b'\n' => { /* line continuation */ }
                        b'0'..=b'7' => {
                            // Up to 3 octal digits (first already consumed).
                            let mut val = (e - b'0') as u32;
                            for _ in 0..2 {
                                match self.peek() {
                                    Some(d @ b'0'..=b'7') => {
                                        val = val * 8 + (d - b'0') as u32;
                                        self.pos += 1;
                                    }
                                    _ => break,
                                }
                            }
                            out.push((val & 0xFF) as u8);
                        }
                        // Any other escaped char: the char itself.
                        other => out.push(other),
                    }
                }
                b'(' => {
                    nest += 1;
                    out.push(b'(');
                }
                b')' => {
                    nest -= 1;
                    if nest == 0 {
                        break;
                    }
                    out.push(b')');
                }
                other => out.push(other),
            }
            if out.len() > ath_deflate::MAX_OUTPUT {
                return Err(PdfError::TooLarge);
            }
        }
        Ok(Object::StringLit(out))
    }

    fn parse_hex_string(&mut self) -> Result<Object, PdfError> {
        // Consume '<'.
        if self.bump() != Some(b'<') {
            return Err(PdfError::Malformed);
        }
        let mut nibbles: Vec<u8> = Vec::new();
        loop {
            let b = self.bump().ok_or(PdfError::Malformed)?;
            if b == b'>' {
                break;
            }
            if is_ws(b) {
                continue;
            }
            let v = hex_val(b).ok_or(PdfError::Malformed)?;
            nibbles.push(v);
            if nibbles.len() > ath_deflate::MAX_OUTPUT {
                return Err(PdfError::TooLarge);
            }
        }
        // Pair nibbles into bytes; a trailing odd nibble is padded with 0.
        let mut out = Vec::with_capacity(nibbles.len().div_ceil(2));
        let mut i = 0;
        while i < nibbles.len() {
            let hi = nibbles[i];
            let lo = if i + 1 < nibbles.len() {
                nibbles[i + 1]
            } else {
                0
            };
            out.push(hi << 4 | lo);
            i += 2;
        }
        Ok(Object::StringLit(out))
    }

    fn parse_array(&mut self, depth: usize) -> Result<Object, PdfError> {
        if self.bump() != Some(b'[') {
            return Err(PdfError::Malformed);
        }
        let mut out: Vec<Object> = Vec::new();
        loop {
            self.skip_ws();
            match self.peek() {
                None => return Err(PdfError::Malformed),
                Some(b']') => {
                    self.pos += 1;
                    break;
                }
                _ => {
                    let obj = self.parse_object(depth + 1)?;
                    out.push(obj);
                    if out.len() > MAX_XREF_ENTRIES {
                        return Err(PdfError::TooLarge);
                    }
                }
            }
        }
        Ok(Object::Array(out))
    }

    /// Parse a number, or — if it's `N G R` / `N G obj` shaped — an indirect
    /// reference. (Indirect *object* bodies are parsed separately; here we only
    /// recognise the `R` reference form that appears inside other objects.)
    fn parse_number_or_ref(&mut self) -> Result<Object, PdfError> {
        let start = self.pos;
        let first = self.parse_raw_number()?;
        // Try the `<int> <int> R` reference form.
        if let Number::Int(n) = first {
            if n >= 0 {
                let save = self.pos;
                self.skip_ws();
                if let Some(b'0'..=b'9') = self.peek() {
                    if let Ok(Number::Int(g)) = self.parse_raw_number() {
                        if (0..=65535).contains(&g) {
                            self.skip_ws();
                            if self.eat_keyword(b"R") {
                                return Ok(Object::Reference(ObjRef {
                                    num: n as u32,
                                    gen: g as u16,
                                }));
                            }
                        }
                    }
                }
                // Not a reference: rewind to just after the first number.
                self.pos = save;
            }
        }
        // Plain number.
        let _ = start;
        Ok(match first {
            Number::Int(i) => Object::Integer(i),
            Number::Real(r) => Object::Real(r),
        })
    }

    fn parse_raw_number(&mut self) -> Result<Number, PdfError> {
        let start = self.pos;
        let mut seen_digit = false;
        let mut is_real = false;
        if matches!(self.peek(), Some(b'+') | Some(b'-')) {
            self.pos += 1;
        }
        while let Some(b) = self.peek() {
            match b {
                b'0'..=b'9' => {
                    seen_digit = true;
                    self.pos += 1;
                }
                b'.' => {
                    is_real = true;
                    self.pos += 1;
                }
                _ => break,
            }
        }
        if !seen_digit {
            return Err(PdfError::Malformed);
        }
        let slice = &self.data[start..self.pos];
        let s = core::str::from_utf8(slice).map_err(|_| PdfError::Malformed)?;
        if is_real {
            parse_real(s).map(Number::Real).ok_or(PdfError::Malformed)
        } else {
            // Large integers that overflow i64 are clamped (never panic).
            match s.parse::<i64>() {
                Ok(v) => Ok(Number::Int(v)),
                Err(_) => parse_real(s).map(Number::Real).ok_or(PdfError::Malformed),
            }
        }
    }

    fn parse_dict_or_stream(&mut self, depth: usize) -> Result<Object, PdfError> {
        // Consume '<<'.
        if self.bump() != Some(b'<') || self.bump() != Some(b'<') {
            return Err(PdfError::Malformed);
        }
        let mut dict: Dict = BTreeMap::new();
        loop {
            self.skip_ws();
            match self.peek() {
                None => return Err(PdfError::Malformed),
                Some(b'>') => {
                    if self.peek_at(1) == Some(b'>') {
                        self.pos += 2;
                        break;
                    }
                    return Err(PdfError::Malformed);
                }
                Some(b'/') => {
                    let key = match self.parse_name()? {
                        Object::Name(n) => n,
                        _ => return Err(PdfError::Malformed),
                    };
                    let val = self.parse_object(depth + 1)?;
                    dict.insert(key, val);
                    if dict.len() > MAX_XREF_ENTRIES {
                        return Err(PdfError::TooLarge);
                    }
                }
                _ => return Err(PdfError::Malformed),
            }
        }
        // Is this dict followed by a `stream` keyword?
        let save = self.pos;
        self.skip_ws();
        if self.eat_keyword(b"stream") {
            // After `stream`, the spec says CRLF or a single LF (not a lone CR).
            match self.peek() {
                Some(b'\r') => {
                    self.pos += 1;
                    if self.peek() == Some(b'\n') {
                        self.pos += 1;
                    }
                }
                Some(b'\n') => {
                    self.pos += 1;
                }
                _ => {}
            }
            let body_start = self.pos;
            // Determine length: prefer a direct /Length, else scan for endstream.
            let raw = self.read_stream_body(&dict, body_start)?;
            return Ok(Object::Stream(Stream { dict, raw }));
        }
        self.pos = save;
        Ok(Object::Dictionary(dict))
    }

    /// Read a stream body. Uses a direct integer `/Length` when present and
    /// consistent; otherwise (or for an indirect `/Length` we can't resolve here)
    /// scans for the `endstream` keyword. Always bounded.
    fn read_stream_body(&mut self, dict: &Dict, body_start: usize) -> Result<Vec<u8>, PdfError> {
        let direct_len = dict.get("Length").and_then(|o| o.as_i64());
        if let Some(len) = direct_len {
            if len >= 0 {
                let len = len as usize;
                if let Some(end) = body_start.checked_add(len) {
                    if end <= self.data.len() {
                        // Validate that `endstream` plausibly follows (allow ws).
                        let mut probe = Lexer::new(self.data, end);
                        probe.skip_ws();
                        if probe.eat_keyword(b"endstream") {
                            self.pos = probe.pos;
                            return Ok(self.data[body_start..end].to_vec());
                        }
                    }
                }
            }
        }
        // Fallback: scan for the `endstream` keyword.
        self.scan_for_endstream(body_start)
    }

    fn scan_for_endstream(&mut self, body_start: usize) -> Result<Vec<u8>, PdfError> {
        let needle = b"endstream";
        let mut i = body_start;
        let data = self.data;
        while i + needle.len() <= data.len() {
            if &data[i..i + needle.len()] == needle {
                // Trim a single trailing EOL before `endstream`.
                let mut end = i;
                if end > body_start && data[end - 1] == b'\n' {
                    end -= 1;
                    if end > body_start && data[end - 1] == b'\r' {
                        end -= 1;
                    }
                } else if end > body_start && data[end - 1] == b'\r' {
                    end -= 1;
                }
                self.pos = i + needle.len();
                return Ok(data[body_start..end].to_vec());
            }
            i += 1;
        }
        Err(PdfError::Malformed)
    }
}

enum Number {
    Int(i64),
    Real(f64),
}

#[inline]
fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Parse a PDF real number (decimal, no exponent) without `f64::from_str`'s full
/// generality — bounded and never-panic. Accepts `[+-]?ddd.ddd`, `.ddd`, `ddd.`.
fn parse_real(s: &str) -> Option<f64> {
    let bytes = s.as_bytes();
    if bytes.is_empty() {
        return None;
    }
    let mut i = 0;
    let mut neg = false;
    match bytes[0] {
        b'+' => i = 1,
        b'-' => {
            neg = true;
            i = 1;
        }
        _ => {}
    }
    let mut int_part: f64 = 0.0;
    let mut frac_part: f64 = 0.0;
    let mut frac_div: f64 = 1.0;
    let mut seen_dot = false;
    let mut seen_digit = false;
    while i < bytes.len() {
        match bytes[i] {
            d @ b'0'..=b'9' => {
                seen_digit = true;
                let dv = (d - b'0') as f64;
                if seen_dot {
                    frac_div *= 10.0;
                    frac_part += dv / frac_div;
                } else {
                    int_part = int_part * 10.0 + dv;
                }
            }
            b'.' if !seen_dot => seen_dot = true,
            _ => return None,
        }
        i += 1;
    }
    if !seen_digit {
        return None;
    }
    let v = int_part + frac_part;
    Some(if neg { -v } else { v })
}

// ════════════════════════════════════════════════════════════════════════════
// File structure: trailer, startxref, xref table + xref streams
// ════════════════════════════════════════════════════════════════════════════

/// Where an object lives, resolved from the cross-reference data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum XrefEntry {
    /// In-use object at a byte offset in the file (classic type 1).
    Offset(usize),
    /// Compressed object inside an object stream `stream_num`, at `index` within it
    /// (xref-stream type 2).
    InStream { stream_num: u32, index: u32 },
}

/// Find the last `startxref` in the file and return the byte offset it points at.
fn find_startxref(data: &[u8]) -> Result<usize, PdfError> {
    // Search backward from EOF for "startxref".
    let needle = b"startxref";
    if data.len() < needle.len() {
        return Err(PdfError::NoStartxref);
    }
    let mut i = data.len() - needle.len();
    loop {
        if &data[i..i + needle.len()] == needle {
            let mut lx = Lexer::new(data, i + needle.len());
            lx.skip_ws();
            let n = lx.parse_raw_number().map_err(|_| PdfError::NoStartxref)?;
            return match n {
                Number::Int(v) if v >= 0 && (v as usize) <= data.len() => Ok(v as usize),
                _ => Err(PdfError::NoStartxref),
            };
        }
        if i == 0 {
            return Err(PdfError::NoStartxref);
        }
        i -= 1;
    }
}

// ════════════════════════════════════════════════════════════════════════════
// Document
// ════════════════════════════════════════════════════════════════════════════

/// A parsed PDF document: version, the resolved object table, the trailer, and the
/// enumerated pages.
pub struct Document {
    /// The `%PDF-X.Y` version string (e.g. `"1.7"`), best-effort.
    pub version: String,
    /// The trailer dictionary (carries `/Root`, `/Size`, …).
    pub trailer: Dict,
    /// The enumerated pages, in document order.
    pub pages: Vec<Page>,
    /// Resolved indirect objects, keyed by object number (latest generation wins).
    objects: BTreeMap<u32, Object>,
    /// The raw file bytes (kept so object streams can be lazily materialized).
    data: Vec<u8>,
}

/// One page of the document.
pub struct Page {
    /// The page's `/MediaBox` `[x0 y0 x1 y1]` if present/inherited (points).
    pub media_box: Option<[f64; 4]>,
    /// The object reference of the page node (for future structure work).
    pub node: ObjRef,
    /// The (possibly multiple) `/Contents` stream object references.
    contents: Vec<ObjRef>,
    /// The eagerly extracted plain text for this page.
    text: String,
}

impl Page {
    /// This page's extracted plain text.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Page width in points (`x1 - x0`), if a MediaBox is known.
    pub fn width(&self) -> Option<f64> {
        self.media_box.map(|b| (b[2] - b[0]).abs())
    }

    /// Page height in points (`y1 - y0`), if a MediaBox is known.
    pub fn height(&self) -> Option<f64> {
        self.media_box.map(|b| (b[3] - b[1]).abs())
    }
}

impl Document {
    /// Parse a PDF from its bytes. Returns `Err` on any malformed/hostile input —
    /// never panics, never infinite-loops.
    pub fn open(data: &[u8]) -> Result<Document, PdfError> {
        // 1. Header: %PDF-X.Y near the start.
        let version = parse_version(data)?;

        // 2. Build the object table from the xref chain.
        let mut doc = Document {
            version,
            trailer: BTreeMap::new(),
            pages: Vec::new(),
            objects: BTreeMap::new(),
            data: data.to_vec(),
        };

        let start = find_startxref(data)?;
        doc.load_xref_chain(start)?;

        // 3. Materialize compressed (object-stream) objects referenced by the xref.
        // (Done lazily within resolve; here we ensure the trailer/Root are present.)
        if doc.trailer.get("Root").is_none() {
            // Some files put Root only in an xref-stream dict we already merged.
            return Err(PdfError::BadTrailer);
        }

        // 4. Walk the page tree.
        doc.enumerate_pages()?;

        // 5. Eagerly extract text per page (bounded total work).
        doc.extract_all_text()?;

        Ok(doc)
    }

    /// Number of pages.
    pub fn page_count(&self) -> usize {
        self.pages.len()
    }

    /// Concatenated plain text of every page, in order, separated by form-feeds
    /// (`\u{0C}`) so callers can split on page boundaries if they wish.
    pub fn extract_text(&self) -> String {
        let mut out = String::new();
        for (i, p) in self.pages.iter().enumerate() {
            if i > 0 {
                out.push('\u{0C}');
            }
            out.push_str(&p.text);
        }
        out
    }

    /// The `/Root` catalog dictionary, if resolvable (for future structure use).
    pub fn root(&self) -> Option<&Dict> {
        let r = self.trailer.get("Root")?;
        self.resolve(r, 0).ok().and_then(|o| match o {
            ResolvedRef::Borrowed(b) => b.as_dict(),
            ResolvedRef::Direct(_) => None,
        })
    }

    // ── xref loading ────────────────────────────────────────────────────────

    /// Load the xref data starting at `offset`, following `/Prev` (and xref-stream
    /// `/XRefStm`) chains up to [`MAX_XREF_CHAIN`]. The *first* (newest) definition
    /// of each object number wins (so later/incremental updates take precedence).
    fn load_xref_chain(&mut self, offset: usize) -> Result<(), PdfError> {
        let mut xref: BTreeMap<u32, XrefEntry> = BTreeMap::new();
        let mut next: Option<usize> = Some(offset);
        let mut visited: Vec<usize> = Vec::new();
        let mut chain = 0usize;

        while let Some(off) = next {
            if chain >= MAX_XREF_CHAIN {
                break; // documented deferral — stop following deeper /Prev links.
            }
            if visited.contains(&off) {
                break; // cycle in /Prev — stop.
            }
            visited.push(off);
            chain += 1;

            if off >= self.data.len() {
                return Err(PdfError::BadXref);
            }
            let (trailer, prev) = self.parse_one_xref_section(off, &mut xref)?;
            // The first section parsed (newest) seeds the document trailer.
            if self.trailer.is_empty() {
                self.trailer = trailer;
            } else {
                // Merge missing keys from older trailers (e.g. /Root only old).
                for (k, v) in trailer {
                    self.trailer.entry(k).or_insert(v);
                }
            }
            next = prev;
        }

        if xref.is_empty() {
            return Err(PdfError::BadXref);
        }
        if xref.len() > MAX_XREF_ENTRIES {
            return Err(PdfError::TooLarge);
        }

        // Now materialize every referenced object.
        self.materialize_objects(&xref)?;
        Ok(())
    }

    /// Parse one xref section at `off` — either a classic `xref` table (followed by
    /// a `trailer` dict) or an xref stream (an indirect object whose dict has
    /// `/Type /XRef`). Adds entries to `xref` (only if not already present — newest
    /// wins). Returns `(trailer_dict, prev_offset)`.
    fn parse_one_xref_section(
        &self,
        off: usize,
        xref: &mut BTreeMap<u32, XrefEntry>,
    ) -> Result<(Dict, Option<usize>), PdfError> {
        let mut lx = Lexer::new(&self.data, off);
        lx.skip_ws();
        if lx.eat_keyword(b"xref") {
            self.parse_classic_xref(&mut lx, xref)
        } else {
            // Expect an indirect object: `N G obj << /Type /XRef … >> stream …`.
            self.parse_xref_stream(off, xref)
        }
    }

    /// Classic `xref` table: one or more `start count` subsections of fixed-width
    /// 20-byte entries, then `trailer << … >>`.
    fn parse_classic_xref(
        &self,
        lx: &mut Lexer,
        xref: &mut BTreeMap<u32, XrefEntry>,
    ) -> Result<(Dict, Option<usize>), PdfError> {
        loop {
            lx.skip_ws();
            if lx.eat_keyword(b"trailer") {
                break;
            }
            // Subsection header: `start count`.
            let start = match lx.parse_raw_number() {
                Ok(Number::Int(v)) if v >= 0 => v as u32,
                _ => return Err(PdfError::BadXref),
            };
            lx.skip_ws();
            let count = match lx.parse_raw_number() {
                Ok(Number::Int(v)) if v >= 0 => v as usize,
                _ => return Err(PdfError::BadXref),
            };
            if count > MAX_XREF_ENTRIES {
                return Err(PdfError::TooLarge);
            }
            lx.skip_ws();
            for k in 0..count {
                // Each entry: 10-digit offset, 5-digit gen, 1 type char ('n'/'f').
                let offset = read_fixed_int(lx, 10)?;
                lx.skip_ws();
                let _gen = read_fixed_int(lx, 5)?;
                lx.skip_ws();
                let ty = lx.bump().ok_or(PdfError::BadXref)?;
                let num = start + k as u32;
                if ty == b'n' {
                    xref.entry(num)
                        .or_insert(XrefEntry::Offset(offset as usize));
                }
                // 'f' = free; skip.
                lx.skip_ws();
                if xref.len() > MAX_XREF_ENTRIES {
                    return Err(PdfError::TooLarge);
                }
            }
        }
        // Parse the trailer dictionary.
        let trailer = match lx.parse_object(0)? {
            Object::Dictionary(d) => d,
            _ => return Err(PdfError::BadTrailer),
        };
        // A hybrid file may have an /XRefStm pointing at a supplemental xref stream.
        if let Some(off) = trailer.get("XRefStm").and_then(|o| o.as_i64()) {
            if off >= 0 && (off as usize) < self.data.len() {
                let _ = self.parse_xref_stream(off as usize, xref);
            }
        }
        let prev = trailer
            .get("Prev")
            .and_then(|o| o.as_i64())
            .filter(|v| *v >= 0)
            .map(|v| v as usize);
        Ok((trailer, prev))
    }

    /// Cross-reference STREAM (PDF 1.5+): an indirect object `<< /Type /XRef
    /// /W [a b c] /Index [...] /Size n /Prev … >>` whose FlateDecode'd body is a
    /// packed table of (type, field2, field3) records.
    fn parse_xref_stream(
        &self,
        off: usize,
        xref: &mut BTreeMap<u32, XrefEntry>,
    ) -> Result<(Dict, Option<usize>), PdfError> {
        let mut lx = Lexer::new(&self.data, off);
        // `N G obj`
        lx.skip_ws();
        let _num = lx.parse_raw_number().map_err(|_| PdfError::BadXref)?;
        lx.skip_ws();
        let _gen = lx.parse_raw_number().map_err(|_| PdfError::BadXref)?;
        lx.skip_ws();
        if !lx.eat_keyword(b"obj") {
            return Err(PdfError::BadXref);
        }
        let obj = lx.parse_object(0)?;
        let stream = match obj {
            Object::Stream(s) => s,
            _ => return Err(PdfError::BadXref),
        };
        let dict = stream.dict.clone();

        // W = field widths.
        let w = dict
            .get("W")
            .and_then(|o| o.as_array())
            .ok_or(PdfError::BadXref)?;
        if w.len() < 3 {
            return Err(PdfError::BadXref);
        }
        let w0 = w[0]
            .as_i64()
            .filter(|v| *v >= 0 && *v <= 8)
            .ok_or(PdfError::BadXref)? as usize;
        let w1 = w[1]
            .as_i64()
            .filter(|v| *v >= 0 && *v <= 8)
            .ok_or(PdfError::BadXref)? as usize;
        let w2 = w[2]
            .as_i64()
            .filter(|v| *v >= 0 && *v <= 8)
            .ok_or(PdfError::BadXref)? as usize;
        let rec_len = w0 + w1 + w2;
        if rec_len == 0 {
            return Err(PdfError::BadXref);
        }

        // Index = [start count start count …]; default [0 Size].
        let size = dict.get("Size").and_then(|o| o.as_i64()).unwrap_or(0);
        let index: Vec<(u32, usize)> = match dict.get("Index").and_then(|o| o.as_array()) {
            Some(arr) => {
                let mut v = Vec::new();
                let mut i = 0;
                while i + 1 < arr.len() {
                    let s = arr[i]
                        .as_i64()
                        .filter(|x| *x >= 0)
                        .ok_or(PdfError::BadXref)? as u32;
                    let c = arr[i + 1]
                        .as_i64()
                        .filter(|x| *x >= 0)
                        .ok_or(PdfError::BadXref)? as usize;
                    v.push((s, c));
                    i += 2;
                }
                v
            }
            None => {
                if size < 0 {
                    return Err(PdfError::BadXref);
                }
                vec![(0u32, size as usize)]
            }
        };

        // Decode the stream body (FlateDecode expected).
        let decoded = decode_stream(&stream, None)?;
        let mut p = 0usize;
        for (start, count) in index {
            if count > MAX_XREF_ENTRIES {
                return Err(PdfError::TooLarge);
            }
            for k in 0..count {
                if p + rec_len > decoded.len() {
                    // Truncated table — stop gracefully.
                    break;
                }
                let f0 = read_be(&decoded[p..p + w0]);
                let f1 = read_be(&decoded[p + w0..p + w0 + w1]);
                let f2 = read_be(&decoded[p + w0 + w1..p + rec_len]);
                p += rec_len;
                // Default type when w0 == 0 is 1.
                let ty = if w0 == 0 { 1 } else { f0 };
                let num = start.wrapping_add(k as u32);
                match ty {
                    1 => {
                        xref.entry(num).or_insert(XrefEntry::Offset(f1 as usize));
                    }
                    2 => {
                        xref.entry(num).or_insert(XrefEntry::InStream {
                            stream_num: f1 as u32,
                            index: f2 as u32,
                        });
                    }
                    _ => { /* type 0 = free; skip */ }
                }
                if xref.len() > MAX_XREF_ENTRIES {
                    return Err(PdfError::TooLarge);
                }
            }
        }

        let prev = dict
            .get("Prev")
            .and_then(|o| o.as_i64())
            .filter(|v| *v >= 0)
            .map(|v| v as usize);
        Ok((dict, prev))
    }

    /// Resolve every xref entry into `self.objects`. Direct offsets are parsed in
    /// place; object-stream (type 2) members are materialized by decoding their
    /// container object stream once.
    fn materialize_objects(&mut self, xref: &BTreeMap<u32, XrefEntry>) -> Result<(), PdfError> {
        if xref.len() > MAX_OBJECTS {
            return Err(PdfError::TooLarge);
        }
        // Cache decoded object streams: stream_num → (decoded bytes, offsets).
        let mut obj_streams: BTreeMap<u32, ObjStm> = BTreeMap::new();

        for (&num, &entry) in xref {
            if self.objects.len() > MAX_OBJECTS {
                return Err(PdfError::TooLarge);
            }
            match entry {
                XrefEntry::Offset(off) => {
                    if let Ok(obj) = self.parse_indirect_at(off, num) {
                        self.objects.entry(num).or_insert(obj);
                    }
                }
                XrefEntry::InStream { stream_num, index } => {
                    // Lazily decode the container object stream.
                    if !obj_streams.contains_key(&stream_num) {
                        if let Some(off) = match xref.get(&stream_num) {
                            Some(XrefEntry::Offset(o)) => Some(*o),
                            _ => None,
                        } {
                            if let Ok(Object::Stream(s)) = self.parse_indirect_at(off, stream_num) {
                                if let Ok(ostm) = ObjStm::decode(&s) {
                                    obj_streams.insert(stream_num, ostm);
                                }
                            }
                        }
                    }
                    if let Some(ostm) = obj_streams.get(&stream_num) {
                        if let Some(obj) = ostm.object_at(index as usize) {
                            self.objects.entry(num).or_insert(obj);
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Parse an indirect object body `N G obj <object> endobj` located at byte
    /// `off`, validating the leading object number matches `expect_num`.
    fn parse_indirect_at(&self, off: usize, expect_num: u32) -> Result<Object, PdfError> {
        if off >= self.data.len() {
            return Err(PdfError::Malformed);
        }
        let mut lx = Lexer::new(&self.data, off);
        lx.skip_ws();
        let num = match lx.parse_raw_number() {
            Ok(Number::Int(v)) if v >= 0 => v as u32,
            _ => return Err(PdfError::Malformed),
        };
        if num != expect_num {
            // Tolerate mismatch but don't trust it.
            return Err(PdfError::Malformed);
        }
        lx.skip_ws();
        let _gen = lx.parse_raw_number().map_err(|_| PdfError::Malformed)?;
        lx.skip_ws();
        if !lx.eat_keyword(b"obj") {
            return Err(PdfError::Malformed);
        }
        lx.parse_object(0)
    }

    // ── reference resolution ──────────────────────────────────────────────────

    /// Resolve an object, following indirect references up to [`MAX_REF_DEPTH`].
    fn resolve<'a>(&'a self, obj: &'a Object, depth: usize) -> Result<ResolvedRef<'a>, PdfError> {
        if depth > MAX_REF_DEPTH {
            return Err(PdfError::TooDeep);
        }
        match obj {
            Object::Reference(r) => match self.objects.get(&r.num) {
                Some(target) => self.resolve(target, depth + 1),
                None => Ok(ResolvedRef::Direct(Object::Null)),
            },
            other => Ok(ResolvedRef::Borrowed(other)),
        }
    }

    /// Resolve to an owned object (used where a borrow can't span the recursion).
    fn resolve_owned(&self, obj: &Object, depth: usize) -> Result<Object, PdfError> {
        match self.resolve(obj, depth)? {
            ResolvedRef::Borrowed(b) => Ok(b.clone()),
            ResolvedRef::Direct(d) => Ok(d),
        }
    }

    // ── page-tree enumeration ─────────────────────────────────────────────────

    fn enumerate_pages(&mut self) -> Result<(), PdfError> {
        let root_ref = self
            .trailer
            .get("Root")
            .cloned()
            .ok_or(PdfError::BadTrailer)?;
        let root = self.resolve_owned(&root_ref, 0)?;
        let root_dict = root.as_dict().ok_or(PdfError::BadPageTree)?;
        let pages_ref = root_dict
            .get("Pages")
            .cloned()
            .ok_or(PdfError::BadPageTree)?;

        let mut pages: Vec<Page> = Vec::new();
        let mut visited: Vec<u32> = Vec::new();
        self.walk_page_node(
            &pages_ref,
            &Inherited::default(),
            0,
            &mut pages,
            &mut visited,
        )?;
        if pages.is_empty() {
            return Err(PdfError::BadPageTree);
        }
        self.pages = pages;
        Ok(())
    }

    fn walk_page_node(
        &self,
        node_ref: &Object,
        inherited: &Inherited,
        depth: usize,
        out: &mut Vec<Page>,
        visited: &mut Vec<u32>,
    ) -> Result<(), PdfError> {
        if depth > MAX_DEPTH {
            return Err(PdfError::TooDeep);
        }
        if out.len() > MAX_PAGES {
            return Err(PdfError::TooLarge);
        }
        // Track the object number for cycle detection.
        let this_num = match node_ref {
            Object::Reference(r) => Some(r.num),
            _ => None,
        };
        if let Some(n) = this_num {
            if visited.contains(&n) {
                return Ok(()); // cycle — stop this branch.
            }
            visited.push(n);
        }

        let node = self.resolve_owned(node_ref, 0)?;
        let dict = match node.as_dict() {
            Some(d) => d,
            None => return Ok(()), // skip non-dict node gracefully
        };

        // Merge inheritable attributes.
        let mut inh = inherited.clone();
        if let Some(mb) = dict.get("MediaBox").and_then(|o| self.as_rect(o)) {
            inh.media_box = Some(mb);
        }

        let ty = dict.get("Type").and_then(|o| o.as_name());
        match ty {
            Some("Pages") => {
                let kids = match dict.get("Kids") {
                    Some(k) => self.resolve_owned(k, 0)?,
                    None => return Ok(()),
                };
                if let Some(arr) = kids.as_array() {
                    if arr.len() > MAX_PAGES {
                        return Err(PdfError::TooLarge);
                    }
                    for kid in arr {
                        self.walk_page_node(kid, &inh, depth + 1, out, visited)?;
                        if out.len() > MAX_PAGES {
                            return Err(PdfError::TooLarge);
                        }
                    }
                }
            }
            Some("Page") | None => {
                // A Page leaf (some malformed files omit /Type on leaves; if it has
                // /Kids treat as a tree node, else as a leaf).
                if dict.contains_key("Kids") && ty.is_none() {
                    if let Some(arr) = dict.get("Kids").and_then(|o| o.as_array()) {
                        for kid in arr.to_vec() {
                            self.walk_page_node(&kid, &inh, depth + 1, out, visited)?;
                        }
                    }
                    return Ok(());
                }
                let node = match node_ref {
                    Object::Reference(r) => *r,
                    _ => ObjRef { num: 0, gen: 0 },
                };
                let contents = self.collect_contents(dict)?;
                out.push(Page {
                    media_box: inh.media_box,
                    node,
                    contents,
                    text: String::new(),
                });
            }
            _ => { /* unknown node type — skip */ }
        }
        Ok(())
    }

    /// A page's `/Contents` may be a single stream ref or an array of stream refs.
    fn collect_contents(&self, page: &Dict) -> Result<Vec<ObjRef>, PdfError> {
        let mut refs = Vec::new();
        if let Some(c) = page.get("Contents") {
            match c {
                Object::Reference(r) => refs.push(*r),
                Object::Array(arr) => {
                    for it in arr {
                        if let Object::Reference(r) = it {
                            refs.push(*r);
                        }
                    }
                }
                _ => {}
            }
        }
        Ok(refs)
    }

    fn as_rect(&self, obj: &Object) -> Option<[f64; 4]> {
        let r = self.resolve(obj, 0).ok()?;
        let arr = match &r {
            ResolvedRef::Borrowed(b) => b.as_array()?,
            ResolvedRef::Direct(_) => return None,
        };
        if arr.len() != 4 {
            return None;
        }
        let mut v = [0.0f64; 4];
        for (i, e) in arr.iter().enumerate() {
            v[i] = match e {
                Object::Integer(n) => *n as f64,
                Object::Real(r) => *r,
                _ => return None,
            };
        }
        Some(v)
    }

    // ── stream decoding ───────────────────────────────────────────────────────

    /// The raw (still-encoded) bytes of a stream object, by reference.
    pub fn raw_stream(&self, r: ObjRef) -> Option<&[u8]> {
        match self.objects.get(&r.num) {
            Some(Object::Stream(s)) => Some(&s.raw),
            _ => None,
        }
    }

    /// Decode a stream object's bytes through its `/Filter` chain. Returns
    /// [`PdfError::UnsupportedFilter`] for filters this reader doesn't implement
    /// (honest deferral — the raw bytes remain available via [`raw_stream`]).
    pub fn decoded_stream(&self, r: ObjRef) -> Result<Vec<u8>, PdfError> {
        match self.objects.get(&r.num) {
            Some(Object::Stream(s)) => decode_stream(s, Some(self)),
            _ => Err(PdfError::Malformed),
        }
    }

    // ── text extraction ───────────────────────────────────────────────────────

    fn extract_all_text(&mut self) -> Result<(), PdfError> {
        let mut total_work = 0usize;
        // Take pages out to avoid borrowing self mutably + immutably at once.
        let mut pages = core::mem::take(&mut self.pages);
        for page in &mut pages {
            let mut combined: Vec<u8> = Vec::new();
            for cref in &page.contents {
                if let Some(Object::Stream(s)) = self.objects.get(&cref.num) {
                    if let Ok(decoded) = decode_stream(s, Some(self)) {
                        total_work = total_work.saturating_add(decoded.len());
                        if total_work > MAX_TEXT_WORK {
                            break;
                        }
                        combined.extend_from_slice(&decoded);
                        combined.push(b'\n');
                    }
                }
            }
            page.text = extract_text_from_content(&combined);
        }
        self.pages = pages;
        Ok(())
    }
}

/// Inheritable page attributes propagated down the page tree.
#[derive(Clone, Default)]
struct Inherited {
    media_box: Option<[f64; 4]>,
}

/// A resolved reference: either a borrow of an existing object or a freshly
/// produced direct value (e.g. `Null` for a dangling reference).
enum ResolvedRef<'a> {
    Borrowed(&'a Object),
    Direct(Object),
}

/// Parse the `%PDF-X.Y` header version. Tolerates a few junk bytes before it.
fn parse_version(data: &[u8]) -> Result<String, PdfError> {
    let needle = b"%PDF-";
    let scan_limit = data.len().min(1024);
    for i in 0..scan_limit {
        if i + needle.len() <= data.len() && &data[i..i + needle.len()] == needle {
            let mut v = String::new();
            let mut j = i + needle.len();
            while j < data.len() && v.len() < 8 {
                let b = data[j];
                if b.is_ascii_digit() || b == b'.' {
                    v.push(b as char);
                    j += 1;
                } else {
                    break;
                }
            }
            if v.is_empty() {
                return Err(PdfError::NotPdf);
            }
            return Ok(v);
        }
    }
    Err(PdfError::NotPdf)
}

/// Decode a stream's body through its `/Filter`. `doc` (when present) is used to
/// resolve an indirect `/Filter` / `/Length`; xref-stream decoding passes `None`.
fn decode_stream(s: &Stream, doc: Option<&Document>) -> Result<Vec<u8>, PdfError> {
    // Resolve the filter (may be a name, an array of names, or indirect).
    let filter_obj = s.dict.get("Filter");
    let resolved = match (filter_obj, doc) {
        (Some(f), Some(d)) => d.resolve_owned(f, 0)?,
        (Some(f), None) => f.clone(),
        (None, _) => return Ok(s.raw.clone()), // no filter — raw is the data
    };

    let filters: Vec<String> = match resolved {
        Object::Name(n) => vec![n],
        Object::Array(arr) => {
            let mut v = Vec::new();
            for it in arr {
                match it {
                    Object::Name(n) => v.push(n),
                    _ => return Err(PdfError::UnsupportedFilter),
                }
            }
            v
        }
        Object::Null => return Ok(s.raw.clone()),
        _ => return Err(PdfError::UnsupportedFilter),
    };

    let mut data = s.raw.clone();
    for f in &filters {
        match f.as_str() {
            "FlateDecode" | "Fl" => {
                data = ath_deflate::zlib_decompress(&data).map_err(|_| PdfError::BadStream)?;
                if data.len() > ath_deflate::MAX_OUTPUT {
                    return Err(PdfError::TooLarge);
                }
            }
            // Honest deferral: every other filter is unsupported (not faked).
            _ => return Err(PdfError::UnsupportedFilter),
        }
    }
    Ok(data)
}

/// Read a big-endian unsigned integer from up to 8 bytes (xref-stream fields).
fn read_be(bytes: &[u8]) -> u64 {
    let mut v = 0u64;
    for &b in bytes.iter().take(8) {
        v = (v << 8) | b as u64;
    }
    v
}

/// Read a fixed-width ASCII integer of exactly `width` digits from the lexer
/// (classic-xref entries are zero-padded fixed fields).
fn read_fixed_int(lx: &mut Lexer, width: usize) -> Result<u64, PdfError> {
    lx.skip_ws();
    let mut v: u64 = 0;
    let mut read = 0;
    while read < width {
        match lx.peek() {
            Some(d @ b'0'..=b'9') => {
                v = v.saturating_mul(10).saturating_add((d - b'0') as u64);
                lx.pos += 1;
                read += 1;
            }
            _ => break,
        }
    }
    if read == 0 {
        return Err(PdfError::BadXref);
    }
    Ok(v)
}

// ════════════════════════════════════════════════════════════════════════════
// Object streams (PDF 1.5+, /Type /ObjStm) — containers for compressed objects.
// ════════════════════════════════════════════════════════════════════════════

/// A decoded object stream: the concatenated object bodies plus their offsets.
struct ObjStm {
    /// Decompressed body (the part after the header offset table).
    body: Vec<u8>,
    /// `(object number, byte offset within body)` for each contained object.
    entries: Vec<(u32, usize)>,
}

impl ObjStm {
    fn decode(s: &Stream) -> Result<ObjStm, PdfError> {
        let n = s.dict.get("N").and_then(|o| o.as_i64()).unwrap_or(0);
        let first = s.dict.get("First").and_then(|o| o.as_i64()).unwrap_or(0);
        if n < 0 || first < 0 {
            return Err(PdfError::Malformed);
        }
        let n = n as usize;
        if n > MAX_OBJECTS {
            return Err(PdfError::TooLarge);
        }
        let decoded = decode_stream(s, None)?;
        let first = first as usize;
        if first > decoded.len() {
            return Err(PdfError::Malformed);
        }
        // Header: N pairs of `objnum offset` integers (in the first `first` bytes).
        let mut lx = Lexer::new(&decoded[..first], 0);
        let mut entries = Vec::with_capacity(n);
        for _ in 0..n {
            lx.skip_ws();
            let onum = match lx.parse_raw_number() {
                Ok(Number::Int(v)) if v >= 0 => v as u32,
                _ => return Err(PdfError::Malformed),
            };
            lx.skip_ws();
            let ooff = match lx.parse_raw_number() {
                Ok(Number::Int(v)) if v >= 0 => v as usize,
                _ => return Err(PdfError::Malformed),
            };
            entries.push((onum, ooff));
        }
        let body = decoded[first..].to_vec();
        Ok(ObjStm { body, entries })
    }

    /// Parse the object at `index` within the stream.
    fn object_at(&self, index: usize) -> Option<Object> {
        let (_num, off) = *self.entries.get(index)?;
        if off > self.body.len() {
            return None;
        }
        let mut lx = Lexer::new(&self.body, off);
        lx.parse_object(0).ok()
    }
}

// ════════════════════════════════════════════════════════════════════════════
// Content-stream text extraction
// ════════════════════════════════════════════════════════════════════════════

/// Extract plain text from a (decoded) content stream by interpreting the text-
/// showing and positioning operators. This is a heuristic recovery, not a full
/// layout engine: it emits run text in stream order, inserts a space on `Td`/`Tm`
/// horizontal jumps between runs, and a newline on `T*`/vertical moves.
///
/// Single-byte WinAnsi/Standard/ASCII text passes through directly. CID/Type0
/// multi-byte show strings are emitted byte-for-byte (best-effort — documented
/// deferral); they never crash the extractor.
fn extract_text_from_content(data: &[u8]) -> String {
    let mut out = String::new();
    let mut lx = Lexer::new(data, 0);
    // Operand stack of recently-parsed objects (numbers, strings, arrays, names).
    let mut stack: Vec<Object> = Vec::new();
    let mut in_text = false;
    // Track whether the previous show produced output, to space/break sensibly.
    let mut prev_was_show = false;

    let mut guard = 0usize;
    let limit = data.len().saturating_mul(2) + 16;

    loop {
        guard += 1;
        if guard > limit {
            break; // hard work cap — never spin on adversarial input.
        }
        lx.skip_ws();
        let b = match lx.peek() {
            Some(b) => b,
            None => break,
        };
        // Operand?
        if b == b'/'
            || b == b'('
            || b == b'<'
            || b == b'['
            || b == b'+'
            || b == b'-'
            || b == b'.'
            || b.is_ascii_digit()
        {
            match lx.parse_object(0) {
                Ok(obj) => {
                    stack.push(obj);
                    if stack.len() > 4096 {
                        // Bound the operand stack — drop the oldest half.
                        stack.drain(0..2048);
                    }
                }
                Err(_) => {
                    // Skip one byte and continue — never abort on bad operand.
                    lx.pos += 1;
                }
            }
            continue;
        }
        // Otherwise: an operator token (a run of regular chars), or a delimiter we
        // don't start an object with (e.g. ')' / ']' stray) — consume it.
        if !is_regular(b) {
            lx.pos += 1;
            continue;
        }
        let op_start = lx.pos;
        while let Some(c) = lx.peek() {
            if is_regular(c) {
                lx.pos += 1;
            } else {
                break;
            }
        }
        let op = &data[op_start..lx.pos];

        match op {
            b"BT" => {
                in_text = true;
                prev_was_show = false;
                if !out.is_empty() && !out.ends_with('\n') {
                    out.push('\n');
                }
            }
            b"ET" => {
                in_text = false;
                stack.clear();
            }
            b"Tj" | b"'" | b"\"" => {
                // Show a single string (top of stack). `'` and `"` also move to a
                // new line first.
                if op == b"'" || op == b"\"" {
                    if prev_was_show {
                        out.push('\n');
                    }
                }
                if let Some(Object::StringLit(bytes)) = stack.last() {
                    if in_text {
                        push_show_bytes(&mut out, bytes);
                        prev_was_show = true;
                    }
                }
                stack.clear();
            }
            b"TJ" => {
                // Show an array of strings and kerning numbers. Large negative
                // numbers imply a space; we approximate by inserting a space for a
                // sufficiently large gap.
                if let Some(Object::Array(arr)) = stack.last() {
                    if in_text {
                        for el in arr {
                            match el {
                                Object::StringLit(bytes) => push_show_bytes(&mut out, bytes),
                                Object::Integer(n) => {
                                    if *n <= -120 {
                                        out.push(' ');
                                    }
                                }
                                Object::Real(r) => {
                                    if *r <= -120.0 {
                                        out.push(' ');
                                    }
                                }
                                _ => {}
                            }
                        }
                        prev_was_show = true;
                    }
                }
                stack.clear();
            }
            b"Td" | b"TD" => {
                // Move text position: [tx ty]. A non-zero ty (line move) → newline;
                // a positive tx after a show → a space.
                let ty = stack
                    .get(stack.len().wrapping_sub(1))
                    .and_then(|o| o.as_i64_loose());
                let tx = stack
                    .get(stack.len().wrapping_sub(2))
                    .and_then(|o| o.as_i64_loose());
                if prev_was_show {
                    if ty.map(|v| v != 0).unwrap_or(false) {
                        out.push('\n');
                    } else if tx.map(|v| v != 0).unwrap_or(false) {
                        out.push(' ');
                    }
                }
                prev_was_show = false;
                stack.clear();
            }
            b"T*" => {
                if prev_was_show {
                    out.push('\n');
                }
                prev_was_show = false;
                stack.clear();
            }
            b"Tm" => {
                if prev_was_show {
                    out.push('\n');
                }
                prev_was_show = false;
                stack.clear();
            }
            // Any other operator: discard its operands and continue.
            _ => {
                stack.clear();
            }
        }
        // Bound output size defensively.
        if out.len() > ath_deflate::MAX_OUTPUT {
            break;
        }
    }
    out
}

/// Append the bytes of a show-string to the output, mapping the common single-byte
/// encodings: ASCII passes through; high bytes use a small WinAnsi-ish mapping for
/// the printable Latin-1 range; control bytes are dropped.
fn push_show_bytes(out: &mut String, bytes: &[u8]) {
    for &b in bytes {
        match b {
            0x20..=0x7E => out.push(b as char),
            0xA0..=0xFF => out.push(b as char), // Latin-1 maps 1:1 to U+00A0..U+00FF
            b'\n' | b'\r' | b'\t' => out.push(' '),
            _ => { /* drop control/undefined bytes */ }
        }
    }
}

impl Object {
    /// Loose integer view used by the content interpreter: integers and reals both
    /// yield their truncated integer value (positioning operands are often reals).
    fn as_i64_loose(&self) -> Option<i64> {
        match self {
            Object::Integer(i) => Some(*i),
            Object::Real(r) => Some(*r as i64),
            _ => None,
        }
    }
}

// ════════════════════════════════════════════════════════════════════════════
// Host KAT suite — `cargo test -p ath_pdf`. FAIL-able by construction.
// ════════════════════════════════════════════════════════════════════════════
#[cfg(test)]
mod tests;
