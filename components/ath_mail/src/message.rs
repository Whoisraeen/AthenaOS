//! Message / envelope model + the RFC 822 / 2045 message parse.
//!
//! `ath_mime` is a file-association resolver, not a message parser, so the parse
//! of an RFC 2822 message (header block, `multipart/*` split,
//! `Content-Transfer-Encoding` base64 / quoted-printable decode, charset) lives
//! here. We reuse `ath_mime` to *classify* a part by its declared `Content-Type`
//! / filename, and `ath_encode` for the base64 codec.
//!
//! Hostile-byte posture: a message body arrives from an untrusted server, so the
//! parser bounds header count, header line length, multipart nesting depth, and
//! part count; a malformed message yields a best-effort model or a typed
//! [`MessageError`], never a panic or an infinite loop.

use crate::{lossy_str, strip_crlf};
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// Caps for the message parser (server-controlled input).
const MAX_HEADERS: usize = 4096;
const MAX_HEADER_LEN: usize = 64 * 1024;
const MAX_PARTS: usize = 4096;
const MAX_DEPTH: usize = 32;

/// Why a message parse failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MessageError {
    /// More than [`MAX_HEADERS`] header lines, or a header line over
    /// [`MAX_HEADER_LEN`] — a hostile or malformed header block.
    HeaderLimit,
    /// More than [`MAX_PARTS`] MIME parts, or nesting deeper than [`MAX_DEPTH`].
    StructureLimit,
    /// A declared transfer-encoding decode failed (bad base64). The part text is
    /// still available raw; this is returned only when a caller explicitly
    /// requested a decode that could not be honored.
    DecodeFailed,
}

/// A parsed email address (`Display Name <local@domain>`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Address {
    /// Optional display name (`"Ada Lovelace"`), empty if none.
    pub name: String,
    /// The addr-spec (`ada@example.com`), best-effort; may be empty if unparsable.
    pub addr: String,
}

impl Address {
    /// Parse a single address from a header fragment like `Ada <ada@x.com>` or a
    /// bare `ada@x.com`. Never panics. Quotes around the name are stripped.
    pub fn parse(s: &str) -> Address {
        let s = s.trim();
        if let Some(lt) = s.find('<') {
            if let Some(gt) = s[lt..].find('>') {
                let addr = s[lt + 1..lt + gt].trim().to_string();
                let mut name = s[..lt].trim().to_string();
                // strip surrounding quotes on a display name
                if name.len() >= 2 && name.starts_with('"') && name.ends_with('"') {
                    name = name[1..name.len() - 1].to_string();
                }
                return Address { name, addr };
            }
        }
        Address {
            name: String::new(),
            addr: s.to_string(),
        }
    }

    /// Parse a comma-separated address list (`To:` / `Cc:` value). Never panics.
    /// Commas inside quoted display names are respected.
    pub fn parse_list(s: &str) -> Vec<Address> {
        let mut out = Vec::new();
        let mut start = 0;
        let mut in_quotes = false;
        let bytes = s.as_bytes();
        for (i, &b) in bytes.iter().enumerate() {
            match b {
                b'"' => in_quotes = !in_quotes,
                b',' if !in_quotes => {
                    let frag = s[start..i].trim();
                    if !frag.is_empty() {
                        out.push(Address::parse(frag));
                    }
                    start = i + 1;
                }
                _ => {}
            }
        }
        let frag = s[start..].trim();
        if !frag.is_empty() {
            out.push(Address::parse(frag));
        }
        out
    }
}

/// The IMAP-style envelope summary used to list a mailbox without fetching bodies.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Envelope {
    pub date: String,
    pub subject: String,
    pub from: Vec<Address>,
    pub to: Vec<Address>,
    pub cc: Vec<Address>,
    pub message_id: String,
}

/// A mailbox / folder as returned by IMAP `LIST` and selected by `SELECT`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Mailbox {
    /// Hierarchy name, e.g. `INBOX`, `Work/2026`.
    pub name: String,
    /// Hierarchy delimiter character reported by the server (often `/` or `.`).
    pub delimiter: char,
    /// `\Noselect`, `\HasChildren`, etc. flag attributes from LIST.
    pub attributes: Vec<String>,
    /// Message count from `SELECT`/`EXAMINE` (`EXISTS`), if known.
    pub exists: Option<u32>,
    /// Recent count (`RECENT`), if known.
    pub recent: Option<u32>,
    /// `UIDVALIDITY`, if reported.
    pub uid_validity: Option<u32>,
    /// Permanent / session flags advertised by `FLAGS`.
    pub flags: Vec<String>,
}

/// One MIME part of a (possibly multipart) message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Part {
    /// The declared `Content-Type` (e.g. `text/plain`, `application/pdf`),
    /// lowercased; empty defaults to `text/plain` per RFC 2045.
    pub content_type: String,
    /// `charset` parameter if present (lowercased), e.g. `utf-8`.
    pub charset: String,
    /// `filename` / `name` parameter if this is an attachment, else empty.
    pub filename: String,
    /// The `Content-Transfer-Encoding`, lowercased (`7bit` default).
    pub encoding: String,
    /// The DECODED part body bytes (base64 / quoted-printable already applied).
    pub body: Vec<u8>,
}

impl Part {
    /// The decoded body as a lossy UTF-8 string (for `text/*` display). Never
    /// panics on invalid bytes.
    pub fn text(&self) -> String {
        lossy_str(&self.body)
    }

    /// Reuse `ath_mime` to classify this part: returns the MIME type `ath_mime`
    /// would assign to its filename / leading bytes (handy for an attachment
    /// "Open With" decision). Falls back to the declared `content_type` when
    /// `ath_mime` has nothing better.
    pub fn classify(&self) -> &str {
        // Prefer filename+magic resolution from ath_mime; fall back to declared.
        let magic = if self.body.is_empty() {
            None
        } else {
            Some(self.body.as_slice())
        };
        let resolved = ath_mime::resolve_mime(&self.filename, magic);
        if resolved == ath_mime::OCTET_STREAM && !self.content_type.is_empty() {
            &self.content_type
        } else {
            resolved.as_str()
        }
    }
}

/// A fully fetched message: its headers, its envelope summary, and its decoded
/// parts.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FetchedMessage {
    /// Raw header (name, value) pairs in order, values unfolded.
    pub headers: Vec<(String, String)>,
    /// The summarized envelope derived from the headers.
    pub envelope: Envelope,
    /// The decoded parts. A non-multipart message has exactly one part.
    pub parts: Vec<Part>,
}

impl FetchedMessage {
    /// Case-insensitive header lookup; returns the first match's value.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// The first `text/plain` part's decoded text, else the first part's text,
    /// else empty. The "show me the message body" convenience for a reader UI.
    pub fn body_text(&self) -> String {
        for p in &self.parts {
            if p.content_type == "text/plain" {
                return p.text();
            }
        }
        self.parts.first().map(|p| p.text()).unwrap_or_default()
    }

    /// Parse a full RFC 822 / 2045 message from raw bytes (e.g. an IMAP `BODY[]`
    /// or POP3 `RETR` payload). Never panics, never loops on hostile input; over-
    /// limit structure yields [`MessageError`].
    pub fn parse(raw: &[u8]) -> Result<FetchedMessage, MessageError> {
        let (headers, body_start) = parse_headers(raw)?;
        let body = &raw[body_start..];

        let envelope = envelope_from_headers(&headers);

        // Determine top-level content type / boundary.
        let ctype = header_value(&headers, "content-type").unwrap_or_default();
        let parts = parse_body(&ctype, &headers, body, 0)?;

        Ok(FetchedMessage {
            headers,
            envelope,
            parts,
        })
    }
}

/// Parse the header block; returns `(headers, index_of_body_start)`.
/// Unfolds continuation lines (a line starting with SP/TAB continues the prior
/// header). Bounded by [`MAX_HEADERS`] / [`MAX_HEADER_LEN`].
fn parse_headers(raw: &[u8]) -> Result<(Vec<(String, String)>, usize), MessageError> {
    let mut headers: Vec<(String, String)> = Vec::new();
    let mut i = 0;
    let mut current: Option<(String, String)> = None;

    loop {
        // find end of this line
        let line_start = i;
        let mut j = i;
        while j < raw.len() && raw[j] != b'\n' {
            j += 1;
        }
        let next = if j < raw.len() { j + 1 } else { raw.len() };
        // Slice THROUGH the '\n' (when present) so strip_crlf can remove the
        // trailing CRLF; slicing up to `j` would leave a lone '\r'.
        let line = strip_crlf(&raw[line_start..next]);

        // A blank line ends the header block; body starts after it.
        if line.is_empty() {
            if let Some(h) = current.take() {
                push_header(&mut headers, h)?;
            }
            return Ok((headers, next));
        }

        if line.len() > MAX_HEADER_LEN {
            return Err(MessageError::HeaderLimit);
        }

        // Continuation (folded) line?
        if line[0] == b' ' || line[0] == b'\t' {
            if let Some((_, ref mut val)) = current {
                val.push(' ');
                val.push_str(lossy_str(line).trim());
            }
            // (a leading-WS line with no current header is ignored)
        } else {
            if let Some(h) = current.take() {
                push_header(&mut headers, h)?;
            }
            // split on first ':'
            if let Some(colon) = line.iter().position(|&b| b == b':') {
                let name = lossy_str(&line[..colon]).trim().to_string();
                let value = lossy_str(&line[colon + 1..]).trim().to_string();
                current = Some((name, value));
            }
            // a header line with no colon is dropped (defensive, never panics)
        }

        if next >= raw.len() {
            // EOF without a blank line: flush and treat the rest as no-body.
            if let Some(h) = current.take() {
                push_header(&mut headers, h)?;
            }
            return Ok((headers, raw.len()));
        }
        i = next;
    }
}

fn push_header(
    headers: &mut Vec<(String, String)>,
    h: (String, String),
) -> Result<(), MessageError> {
    if headers.len() >= MAX_HEADERS {
        return Err(MessageError::HeaderLimit);
    }
    headers.push(h);
    Ok(())
}

fn header_value(headers: &[(String, String)], name: &str) -> Option<String> {
    headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.clone())
}

fn envelope_from_headers(headers: &[(String, String)]) -> Envelope {
    Envelope {
        date: header_value(headers, "date").unwrap_or_default(),
        subject: header_value(headers, "subject").unwrap_or_default(),
        from: Address::parse_list(&header_value(headers, "from").unwrap_or_default()),
        to: Address::parse_list(&header_value(headers, "to").unwrap_or_default()),
        cc: Address::parse_list(&header_value(headers, "cc").unwrap_or_default()),
        message_id: header_value(headers, "message-id").unwrap_or_default(),
    }
}

/// Parse a body given its content-type, recursing for `multipart/*`. `depth`
/// guards against runaway nesting.
fn parse_body(
    ctype: &str,
    part_headers: &[(String, String)],
    body: &[u8],
    depth: usize,
) -> Result<Vec<Part>, MessageError> {
    if depth > MAX_DEPTH {
        return Err(MessageError::StructureLimit);
    }

    let (mime, params) = split_content_type(ctype);
    let mime_l = ascii_lower(&mime);

    if mime_l.starts_with("multipart/") {
        let boundary = match param(&params, "boundary") {
            Some(b) if !b.is_empty() => b,
            _ => {
                // multipart without a boundary: treat the whole body as one part.
                return Ok(vec_one_part(part_headers, body, &mime_l, &params));
            }
        };
        let chunks = split_multipart(body, &boundary)?;
        let mut parts = Vec::new();
        for chunk in chunks {
            if parts.len() >= MAX_PARTS {
                return Err(MessageError::StructureLimit);
            }
            // Each chunk is itself a mini-message (headers + body).
            let (sub_headers, bstart) = parse_headers(chunk)?;
            let sub_ctype = header_value(&sub_headers, "content-type").unwrap_or_default();
            let sub_body = &chunk[bstart..];
            let mut sub = parse_body(&sub_ctype, &sub_headers, sub_body, depth + 1)?;
            parts.append(&mut sub);
            if parts.len() > MAX_PARTS {
                return Err(MessageError::StructureLimit);
            }
        }
        Ok(parts)
    } else {
        Ok(vec_one_part(part_headers, body, &mime_l, &params))
    }
}

/// Build a single decoded [`Part`] from a leaf body.
fn vec_one_part(
    headers: &[(String, String)],
    body: &[u8],
    mime_l: &str,
    params: &[(String, String)],
) -> Vec<Part> {
    let encoding =
        ascii_lower(&header_value(headers, "content-transfer-encoding").unwrap_or_default());
    let charset = ascii_lower(&param(params, "charset").unwrap_or_default());
    let filename = param(params, "name")
        .or_else(|| {
            header_value(headers, "content-disposition")
                .and_then(|cd| param(&split_content_type(&cd).1, "filename"))
        })
        .unwrap_or_default();

    let content_type = if mime_l.is_empty() {
        "text/plain".to_string()
    } else {
        mime_l.to_string()
    };

    let decoded = decode_transfer(&encoding, body);

    alloc::vec![Part {
        content_type,
        charset,
        filename,
        encoding,
        body: decoded,
    }]
}

/// Decode a leaf body per its transfer-encoding. Unknown / `7bit` / `8bit` /
/// `binary` pass through unchanged. Bad base64 falls back to raw bytes (never
/// errors out the whole message — a reader prefers garbled text to no message).
fn decode_transfer(encoding: &str, body: &[u8]) -> Vec<u8> {
    match encoding {
        "base64" => {
            let s = lossy_str(body);
            match ath_encode::base64_decode(&s) {
                Ok(bytes) => bytes,
                Err(_) => body.to_vec(),
            }
        }
        "quoted-printable" => decode_quoted_printable(body),
        _ => body.to_vec(),
    }
}

/// RFC 2045 §6.7 quoted-printable decoder. Never panics; a malformed `=` escape
/// is passed through literally (lenient, like most MUAs). Soft line breaks
/// (`=` at end of line) are removed.
fn decode_quoted_printable(body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(body.len());
    let mut i = 0;
    while i < body.len() {
        let b = body[i];
        if b == b'=' {
            // soft break: =\r\n or =\n
            if i + 1 < body.len() && body[i + 1] == b'\n' {
                i += 2;
                continue;
            }
            if i + 2 < body.len() && body[i + 1] == b'\r' && body[i + 2] == b'\n' {
                i += 3;
                continue;
            }
            // =XX hex
            if i + 2 < body.len() {
                if let (Some(h), Some(l)) = (hex_val(body[i + 1]), hex_val(body[i + 2])) {
                    out.push((h << 4) | l);
                    i += 3;
                    continue;
                }
            }
            // malformed: keep the '=' literally
            out.push(b'=');
            i += 1;
        } else {
            out.push(b);
            i += 1;
        }
    }
    out
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Split a multipart body on `--boundary` delimiters, returning each part's raw
/// bytes (between delimiters). The preamble (before the first boundary) and the
/// epilogue (after `--boundary--`) are discarded. Bounded by [`MAX_PARTS`].
fn split_multipart<'a>(body: &'a [u8], boundary: &str) -> Result<Vec<&'a [u8]>, MessageError> {
    let delim = {
        let mut d = Vec::with_capacity(boundary.len() + 2);
        d.extend_from_slice(b"--");
        d.extend_from_slice(boundary.as_bytes());
        d
    };
    let mut parts = Vec::new();
    let mut search_from = 0;
    let mut part_start: Option<usize> = None;

    while search_from <= body.len() {
        match find_at_line_start(body, &delim, search_from) {
            Some(pos) => {
                if let Some(ps) = part_start.take() {
                    // body of the previous part is [ps .. pos], minus the CRLF
                    // immediately preceding the boundary.
                    let mut end = pos;
                    if end >= 2 && &body[end - 2..end] == b"\r\n" {
                        end -= 2;
                    } else if end >= 1 && body[end - 1] == b'\n' {
                        end -= 1;
                    }
                    if parts.len() >= MAX_PARTS {
                        return Err(MessageError::StructureLimit);
                    }
                    parts.push(&body[ps..end.min(body.len())]);
                }
                // Position right after the delimiter to inspect the terminator.
                let after = pos + delim.len();
                // closing delimiter "--boundary--" ends the multipart.
                if after + 1 < body.len() && body[after] == b'-' && body[after + 1] == b'-' {
                    break;
                }
                // advance past the boundary line's CRLF to start the next part
                let mut np = after;
                while np < body.len() && body[np] != b'\n' {
                    np += 1;
                }
                if np < body.len() {
                    np += 1;
                }
                part_start = Some(np);
                search_from = np;
            }
            None => break,
        }
    }
    Ok(parts)
}

/// Find `needle` in `hay` at or after `from`, but only when it begins at the
/// start of the buffer or immediately after a `\n` (boundaries are line-anchored).
fn find_at_line_start(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if needle.is_empty() || needle.len() > hay.len() {
        return None;
    }
    let mut i = from;
    while i + needle.len() <= hay.len() {
        let at_line_start = i == 0 || hay[i - 1] == b'\n';
        if at_line_start && &hay[i..i + needle.len()] == needle {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// Split a `Content-Type` header value into `(mime, params)` where params is a
/// list of `(key_lower, value)` pairs. Quoted values are unquoted. Never panics.
fn split_content_type(value: &str) -> (String, Vec<(String, String)>) {
    let mut iter = value.split(';');
    let mime = iter.next().unwrap_or("").trim().to_string();
    let mut params = Vec::new();
    for seg in iter {
        let seg = seg.trim();
        if let Some(eq) = seg.find('=') {
            let k = ascii_lower(seg[..eq].trim());
            let mut v = seg[eq + 1..].trim().to_string();
            if v.len() >= 2 && v.starts_with('"') && v.ends_with('"') {
                v = v[1..v.len() - 1].to_string();
            }
            params.push((k, v));
        }
    }
    (mime, params)
}

fn param(params: &[(String, String)], key: &str) -> Option<String> {
    params
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(key))
        .map(|(_, v)| v.clone())
}

fn ascii_lower(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        out.push(b.to_ascii_lowercase() as char);
    }
    out
}

/// Parse an IMAP ENVELOPE structure: `(date subject from sender reply-to to cc
/// bcc in-reply-to message-id)` where each address list is
/// `((name adl mailbox host) ...)` or NIL. Best-effort, bounded, never panics.
pub(crate) fn parse_imap_envelope(tokens: &str) -> Envelope {
    // The ENVELOPE is a parenthesized S-expression. We do a minimal, bounded
    // token walk rather than a full IMAP grammar — enough for date/subject/from.
    let parsed = ImapSexp::parse(tokens, 0);
    let items = match parsed {
        Some((ImapSexp::List(items), _)) => items,
        _ => return Envelope::default(),
    };
    let get_str = |idx: usize| -> String {
        match items.get(idx) {
            Some(ImapSexp::Atom(s)) => s.clone(),
            Some(ImapSexp::Str(s)) => s.clone(),
            _ => String::new(),
        }
    };
    let get_addrs = |idx: usize| -> Vec<Address> {
        match items.get(idx) {
            Some(ImapSexp::List(addr_list)) => addr_list
                .iter()
                .filter_map(|a| {
                    if let ImapSexp::List(fields) = a {
                        let f = |n: usize| -> String {
                            match fields.get(n) {
                                Some(ImapSexp::Str(s)) | Some(ImapSexp::Atom(s)) => s.clone(),
                                _ => String::new(),
                            }
                        };
                        let name = f(0);
                        let mbox = f(2);
                        let host = f(3);
                        let addr = if host.is_empty() {
                            mbox.clone()
                        } else {
                            alloc::format!("{}@{}", mbox, host)
                        };
                        Some(Address { name, addr })
                    } else {
                        None
                    }
                })
                .collect(),
            _ => Vec::new(),
        }
    };
    Envelope {
        date: get_str(0),
        subject: get_str(1),
        from: get_addrs(2),
        to: get_addrs(5),
        cc: get_addrs(6),
        message_id: get_str(9),
    }
}

/// A tiny, bounded IMAP S-expression model used only for ENVELOPE parsing.
enum ImapSexp {
    Atom(String), // NIL or a bare number/keyword
    Str(String),  // a quoted "string"
    List(Vec<ImapSexp>),
}

impl ImapSexp {
    /// Parse one S-expression starting at byte index `pos`, returning the value
    /// and the index just past it. Depth- and length-bounded; never recurses
    /// past [`MAX_DEPTH`] (returns `None`), never panics.
    fn parse(s: &str, pos: usize) -> Option<(ImapSexp, usize)> {
        Self::parse_depth(s.as_bytes(), pos, 0)
    }

    fn parse_depth(b: &[u8], mut pos: usize, depth: usize) -> Option<(ImapSexp, usize)> {
        if depth > MAX_DEPTH {
            return None;
        }
        // skip whitespace
        while pos < b.len() && (b[pos] == b' ' || b[pos] == b'\t') {
            pos += 1;
        }
        if pos >= b.len() {
            return None;
        }
        match b[pos] {
            b'(' => {
                pos += 1;
                let mut items = Vec::new();
                loop {
                    while pos < b.len() && (b[pos] == b' ' || b[pos] == b'\t') {
                        pos += 1;
                    }
                    if pos >= b.len() {
                        return None; // unterminated list → bounded failure
                    }
                    if b[pos] == b')' {
                        pos += 1;
                        return Some((ImapSexp::List(items), pos));
                    }
                    if items.len() >= MAX_PARTS {
                        return None;
                    }
                    let (item, np) = Self::parse_depth(b, pos, depth + 1)?;
                    items.push(item);
                    pos = np;
                }
            }
            b'"' => {
                pos += 1;
                let mut out = String::new();
                while pos < b.len() {
                    let c = b[pos];
                    if c == b'\\' && pos + 1 < b.len() {
                        out.push(b[pos + 1] as char);
                        pos += 2;
                        continue;
                    }
                    if c == b'"' {
                        pos += 1;
                        return Some((ImapSexp::Str(out), pos));
                    }
                    if out.len() >= MAX_HEADER_LEN {
                        return None;
                    }
                    out.push(c as char);
                    pos += 1;
                }
                None // unterminated string
            }
            _ => {
                // bare atom (NIL, number) up to whitespace or ')'
                let start = pos;
                while pos < b.len() && b[pos] != b' ' && b[pos] != b')' && b[pos] != b'(' {
                    pos += 1;
                }
                let atom = lossy_str(&b[start..pos]);
                Some((ImapSexp::Atom(atom), pos))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn address_parse_name_and_bare() {
        let a = Address::parse("Ada Lovelace <ada@example.com>");
        assert_eq!(a.name, "Ada Lovelace");
        assert_eq!(a.addr, "ada@example.com");
        let b = Address::parse("bob@host.test");
        assert_eq!(b.name, "");
        assert_eq!(b.addr, "bob@host.test");
        let q = Address::parse("\"Doe, John\" <john@x.com>");
        assert_eq!(q.name, "Doe, John");
        assert_eq!(q.addr, "john@x.com");
    }

    #[test]
    fn address_list_respects_quoted_commas() {
        let list = Address::parse_list("\"Doe, John\" <j@x.com>, ada@y.com");
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].addr, "j@x.com");
        assert_eq!(list[1].addr, "ada@y.com");
    }

    #[test]
    fn parse_simple_text_message() {
        let raw = b"From: Ada <ada@x.com>\r\nTo: bob@y.com\r\nSubject: Hi\r\n\r\nHello, world!\r\n";
        let m = FetchedMessage::parse(raw).unwrap();
        assert_eq!(m.envelope.subject, "Hi");
        assert_eq!(m.envelope.from[0].addr, "ada@x.com");
        assert_eq!(m.parts.len(), 1);
        assert_eq!(m.parts[0].content_type, "text/plain");
        assert!(m.body_text().contains("Hello, world!"));
    }

    #[test]
    fn parse_folded_headers() {
        let raw = b"Subject: a very\r\n long subject\r\nFrom: x@y.com\r\n\r\nbody";
        let m = FetchedMessage::parse(raw).unwrap();
        assert_eq!(m.envelope.subject, "a very long subject");
    }

    #[test]
    fn decode_base64_part() {
        // "Hello" base64 = SGVsbG8=
        let raw =
            b"Content-Type: text/plain\r\nContent-Transfer-Encoding: base64\r\n\r\nSGVsbG8=\r\n";
        let m = FetchedMessage::parse(raw).unwrap();
        assert_eq!(m.parts[0].body, b"Hello");
        assert_eq!(m.body_text(), "Hello");
    }

    #[test]
    fn decode_quoted_printable_part() {
        let raw = b"Content-Transfer-Encoding: quoted-printable\r\n\r\nCaf=C3=A9 =\r\ntime";
        let m = FetchedMessage::parse(raw).unwrap();
        // =C3=A9 = "é" in UTF-8; the soft break joins "Caf é" -> "Café time"
        assert_eq!(m.parts[0].text(), "Café time");
    }

    #[test]
    fn parse_multipart_two_parts() {
        let raw = b"Content-Type: multipart/mixed; boundary=\"BB\"\r\n\r\n\
preamble\r\n\
--BB\r\n\
Content-Type: text/plain\r\n\r\n\
the text body\r\n\
--BB\r\n\
Content-Type: application/octet-stream; name=\"a.bin\"\r\n\
Content-Transfer-Encoding: base64\r\n\r\n\
SGk=\r\n\
--BB--\r\n";
        let m = FetchedMessage::parse(raw).unwrap();
        assert_eq!(m.parts.len(), 2);
        assert_eq!(m.parts[0].content_type, "text/plain");
        assert!(m.parts[0].text().contains("the text body"));
        assert_eq!(m.parts[1].filename, "a.bin");
        assert_eq!(m.parts[1].body, b"Hi");
    }

    #[test]
    fn classify_uses_ath_mime() {
        let p = Part {
            content_type: "application/octet-stream".to_string(),
            charset: String::new(),
            filename: "photo.png".to_string(),
            encoding: "base64".to_string(),
            // PNG magic
            body: alloc::vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A],
        };
        assert_eq!(p.classify(), "image/png");
    }

    #[test]
    fn imap_envelope_parse_extracts_from_and_subject() {
        // (date subject from sender reply-to to cc bcc in-reply-to message-id)
        let env = "(\"Mon, 1 Jan 2026 10:00:00 +0000\" \"Test Subject\" \
((\"Ada\" NIL \"ada\" \"x.com\")) \
((\"Ada\" NIL \"ada\" \"x.com\")) \
NIL \
((NIL NIL \"bob\" \"y.com\")) \
NIL NIL NIL \"<id@x.com>\")";
        let e = parse_imap_envelope(env);
        assert_eq!(e.subject, "Test Subject");
        assert_eq!(e.from.len(), 1);
        assert_eq!(e.from[0].name, "Ada");
        assert_eq!(e.from[0].addr, "ada@x.com");
        assert_eq!(e.to[0].addr, "bob@y.com");
        assert_eq!(e.message_id, "<id@x.com>");
    }

    // ---- hostile / never-panic ----

    #[test]
    fn hostile_messages_never_panic() {
        // empty
        let _ = FetchedMessage::parse(b"");
        // headers only, no blank line
        let _ = FetchedMessage::parse(b"Subject: x");
        // colon-less garbage header
        let _ = FetchedMessage::parse(b"not a header\r\n\r\nbody");
        // multipart with no closing boundary (unterminated)
        let raw = b"Content-Type: multipart/mixed; boundary=Z\r\n\r\n--Z\r\nContent-Type: text/plain\r\n\r\nhi";
        let m = FetchedMessage::parse(raw).unwrap();
        // best-effort: it found the one part before EOF (no closing --Z--)
        assert!(m.parts.iter().any(|p| p.text().contains("hi")) || m.parts.is_empty());
        // multipart declaring a boundary that never appears
        let raw2 = b"Content-Type: multipart/mixed; boundary=NOPE\r\n\r\njust text";
        let _ = FetchedMessage::parse(raw2).unwrap();
    }

    #[test]
    fn header_limit_is_enforced() {
        // build a header block far over MAX_HEADERS
        let mut raw = Vec::new();
        for _ in 0..(MAX_HEADERS + 10) {
            raw.extend_from_slice(b"X: y\r\n");
        }
        raw.extend_from_slice(b"\r\nbody");
        assert_eq!(FetchedMessage::parse(&raw), Err(MessageError::HeaderLimit));
    }

    #[test]
    fn imap_envelope_unterminated_is_bounded() {
        // unterminated list → default envelope, no panic/loop
        let e = parse_imap_envelope("(\"date\" \"subj\" ((\"n\" NIL \"m\"");
        assert_eq!(e, Envelope::default());
        // deeply nested parens (over MAX_DEPTH) → bounded None → default
        let deep: String = core::iter::repeat('(').take(MAX_DEPTH + 50).collect();
        let _ = parse_imap_envelope(&deep);
    }

    #[test]
    fn qp_malformed_escape_is_lenient() {
        // a stray '=' not followed by hex is kept literally; never panics
        let out = decode_quoted_printable(b"a=ZZb=");
        assert_eq!(out, b"a=ZZb=");
    }
}
