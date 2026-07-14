//! Native HTTP/1.1 client — request build + streaming response parse.
//!
//! AthenaOS Concept §AthNet / "web apps that feel native": the browser pillar and
//! every app that fetches over the network rests on a correct, hostile-input-safe
//! HTTP/1.1 implementation. This module is the foundation — pure protocol logic
//! (request serialization, response parsing, chunked decoding, URL parsing) kept
//! deliberately independent of any live socket so it is host-KAT-able as the
//! cheapest real proof (CLAUDE.md §15 layer 1).
//!
//! Design rules honored here:
//! * `#![no_std]` + `alloc` — mirrors the crate.
//! * Decoder discipline: this is a NETWORK-FACING parser. Every input is treated
//!   as hostile. The parser NEVER panics — truncated frames, bad chunk sizes,
//!   missing status lines, oversized headers, and non-UTF-8 bodies all produce a
//!   clean `Err`, never an unwrap/index-out-of-bounds.
//! * Header and body sizes are bounded (`Limits`) so a malicious peer cannot
//!   exhaust memory.
//! * The transport is abstracted behind [`HttpTransport`] so the protocol logic
//!   is exercised on the host with a mock, and the real impl wraps the
//!   athnet/athkit TCP socket + DNS resolve when that wrapper lands.
//!
//! HTTPS/TLS is a DEFERRED follow-up: TLS 1.3 building blocks exist behind the
//! crate's `tls13` feature (`athnet::tls_crypto`), but a full handshake-driving
//! transport is out of scope for this slice. `fetch` here is `http://` only.

extern crate alloc;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Limits — bound everything a hostile peer can grow.
// ---------------------------------------------------------------------------

/// Size bounds applied while parsing a response. A peer that exceeds any of
/// these gets a clean `Err`, never an allocation blow-up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Max bytes of the status-line + all header bytes (before body).
    pub max_header_bytes: usize,
    /// Max number of header fields.
    pub max_header_count: usize,
    /// Max total decoded body bytes.
    pub max_body_bytes: usize,
    /// Max value of a single chunk-size (guards a `Transfer-Encoding: chunked`
    /// peer claiming an absurd chunk).
    pub max_chunk_bytes: usize,
}

impl Limits {
    pub const fn new() -> Self {
        Self {
            max_header_bytes: 64 * 1024,
            max_header_count: 256,
            max_body_bytes: 16 * 1024 * 1024,
            max_chunk_bytes: 16 * 1024 * 1024,
        }
    }
}

impl Default for Limits {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Every failure mode of build / parse / fetch. No variant ever comes from a
/// panic — malformed network input maps here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Http1Error {
    /// URL could not be parsed (`fetch` only accepts `http://host[:port]/path`).
    InvalidUrl(String),
    /// Status line was absent / malformed (missing code, non-numeric code, ...).
    BadStatusLine,
    /// A header line was malformed (no colon, control chars, ...).
    BadHeader,
    /// Header section exceeded `Limits::max_header_bytes` / `max_header_count`.
    HeadersTooLarge,
    /// Body exceeded `Limits::max_body_bytes`.
    BodyTooLarge,
    /// A `Transfer-Encoding: chunked` frame was malformed (bad hex size,
    /// missing CRLF, oversized chunk, truncated mid-chunk).
    BadChunk,
    /// A `Content-Length` header value was not a valid non-negative integer.
    BadContentLength,
    /// The peer closed / the stream ended before a complete message arrived.
    UnexpectedEof,
    /// The transport reported a connect/send/recv failure.
    Transport(String),
    /// `Content-Encoding` named a coding we don't implement (e.g. `br`, `zstd`).
    /// The raw body is intact on the `Http1Response`; the caller decides whether
    /// to fail or pass the still-encoded bytes through.
    UnsupportedContentEncoding(String),
    /// A `Content-Encoding: gzip|deflate` body failed to decompress (corrupt
    /// stream, bad checksum, or it tried to expand past the body limit).
    BadContentEncoding,
}

pub type Http1Result<T> = core::result::Result<T, Http1Error>;

// ---------------------------------------------------------------------------
// Request building
// ---------------------------------------------------------------------------

/// HTTP/1.1 request methods this client serializes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
    Head,
    Put,
    Delete,
    Patch,
    Options,
}

impl Method {
    pub fn as_str(&self) -> &'static str {
        match self {
            Method::Get => "GET",
            Method::Post => "POST",
            Method::Head => "HEAD",
            Method::Put => "PUT",
            Method::Delete => "DELETE",
            Method::Patch => "PATCH",
            Method::Options => "OPTIONS",
        }
    }

    /// Per RFC 9110: HEAD responses carry no body even with Content-Length.
    pub fn is_bodyless_response(&self) -> bool {
        matches!(self, Method::Head)
    }
}

/// A request to be serialized into the on-wire byte stream.
///
/// `host` populates the mandatory `Host:` header. `headers` are emitted in
/// order; if a body is present a `Content-Length` header is emitted
/// automatically (callers must NOT also pass one — see [`Http1Request::build`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Http1Request {
    pub method: Method,
    /// Request target, e.g. `/index.html?q=1`. Must start with `/`.
    pub path: String,
    /// Value for the `Host:` header (authority — host or host:port).
    pub host: String,
    /// Extra headers, in emission order (name, value).
    pub headers: Vec<(String, String)>,
    /// Optional request body; drives the auto `Content-Length`.
    pub body: Option<Vec<u8>>,
}

impl Http1Request {
    pub fn new(method: Method, path: impl Into<String>, host: impl Into<String>) -> Self {
        Self {
            method,
            path: path.into(),
            host: host.into(),
            headers: Vec::new(),
            body: None,
        }
    }

    pub fn get(path: impl Into<String>, host: impl Into<String>) -> Self {
        Self::new(Method::Get, path, host)
    }

    pub fn head(path: impl Into<String>, host: impl Into<String>) -> Self {
        Self::new(Method::Head, path, host)
    }

    pub fn post(path: impl Into<String>, host: impl Into<String>, body: Vec<u8>) -> Self {
        let mut r = Self::new(Method::Post, path, host);
        r.body = Some(body);
        r
    }

    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    /// Serialize to the exact on-wire byte stream.
    ///
    /// Layout:
    /// ```text
    /// METHOD path HTTP/1.1\r\n
    /// Host: <host>\r\n
    /// Connection: close\r\n        (default unless caller overrides)
    /// <each extra header>\r\n
    /// Content-Length: N\r\n        (only if body present)
    /// \r\n
    /// <body bytes>
    /// ```
    /// A missing CRLF or wrong Content-Length flips the byte-exact KAT.
    pub fn build(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(128 + self.body.as_ref().map_or(0, |b| b.len()));

        let path = if self.path.is_empty() {
            "/"
        } else {
            &self.path
        };

        buf.extend_from_slice(self.method.as_str().as_bytes());
        buf.push(b' ');
        buf.extend_from_slice(path.as_bytes());
        buf.extend_from_slice(b" HTTP/1.1\r\n");

        // Mandatory Host header.
        buf.extend_from_slice(b"Host: ");
        buf.extend_from_slice(self.host.as_bytes());
        buf.extend_from_slice(b"\r\n");

        // Default Connection: close unless the caller set one. We use a
        // single-shot connection model (no keep-alive pooling here) so the
        // response is framed by EOF when no length is given.
        let has_connection = self
            .headers
            .iter()
            .any(|(n, _)| n.eq_ignore_ascii_case("connection"));
        if !has_connection {
            buf.extend_from_slice(b"Connection: close\r\n");
        }

        for (name, value) in &self.headers {
            buf.extend_from_slice(name.as_bytes());
            buf.extend_from_slice(b": ");
            buf.extend_from_slice(value.as_bytes());
            buf.extend_from_slice(b"\r\n");
        }

        if let Some(body) = &self.body {
            let cl = format!("Content-Length: {}\r\n", body.len());
            buf.extend_from_slice(cl.as_bytes());
            buf.extend_from_slice(b"\r\n");
            buf.extend_from_slice(body);
        } else {
            buf.extend_from_slice(b"\r\n");
        }

        buf
    }
}

// ---------------------------------------------------------------------------
// Response model
// ---------------------------------------------------------------------------

/// A fully-parsed HTTP/1.1 response. `headers` preserve order and duplicates;
/// use [`Http1Response::header`] for case-insensitive lookup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Http1Response {
    pub version_minor: u8,
    pub status: u16,
    pub reason: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Http1Response {
    /// Case-insensitive lookup of the FIRST header with this name.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// All values for a header name (for `Set-Cookie` and other repeatables).
    pub fn header_all<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a str> + 'a {
        self.headers
            .iter()
            .filter(move |(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }

    pub fn is_redirect(&self) -> bool {
        (300..400).contains(&self.status)
    }

    /// The `Content-Encoding` header value, if present.
    pub fn content_encoding(&self) -> Option<&str> {
        self.header("content-encoding")
    }
}

// ---------------------------------------------------------------------------
// Content-Encoding decoding (gzip / deflate) — RFC 9110 §8.4
// ---------------------------------------------------------------------------

/// Decode a body according to its `Content-Encoding` header. Codings are applied
/// in reverse order (the last coding listed was applied last by the server, so
/// it is undone first). `gzip`/`x-gzip` and `deflate` are supported via the
/// bomb-bounded `ath_deflate` decoder; `identity` (and an absent header) pass the
/// body through unchanged. Any other coding (`br`, `zstd`, `compress`) yields
/// [`Http1Error::UnsupportedContentEncoding`] so the caller never receives bytes
/// it would mistake for plaintext.
///
/// NEVER panics: a corrupt compressed stream maps to
/// [`Http1Error::BadContentEncoding`], and the decoder itself caps output growth
/// (decompression-bomb safe).
pub fn decode_content_encoding(
    encoding: Option<&str>,
    body: Vec<u8>,
    limits: &Limits,
) -> Http1Result<Vec<u8>> {
    let Some(encoding) = encoding else {
        return Ok(body);
    };
    // Empty / whitespace-only header is treated as identity.
    if encoding.trim().is_empty() {
        return Ok(body);
    }

    // Collect the coding tokens in application order, then undo in reverse.
    let codings: Vec<&str> = encoding
        .split(',')
        .map(|c| c.trim())
        .filter(|c| !c.is_empty())
        .collect();

    let mut out = body;
    for coding in codings.iter().rev() {
        out = decode_one_coding(coding, out, limits)?;
    }
    Ok(out)
}

fn decode_one_coding(coding: &str, body: Vec<u8>, limits: &Limits) -> Http1Result<Vec<u8>> {
    let decoded = if coding.eq_ignore_ascii_case("identity") {
        return Ok(body);
    } else if coding.eq_ignore_ascii_case("gzip") || coding.eq_ignore_ascii_case("x-gzip") {
        ath_deflate::gzip_decompress(&body).map_err(|_| Http1Error::BadContentEncoding)?
    } else if coding.eq_ignore_ascii_case("deflate") {
        // "deflate" officially means zlib-wrapped (RFC 7230), but enough servers
        // send a raw DEFLATE stream that browsers accept either; try zlib first,
        // fall back to raw inflate before declaring the body corrupt.
        match ath_deflate::zlib_decompress(&body) {
            Ok(v) => v,
            Err(_) => ath_deflate::inflate(&body).map_err(|_| Http1Error::BadContentEncoding)?,
        }
    } else if coding.eq_ignore_ascii_case("br") {
        // Brotli (RFC 7932) — handled by the battle-tested brotli-decompressor
        // behind the `brotli` feature; without it, a clean Unsupported error.
        decode_brotli(&body, limits.max_body_bytes)?
    } else {
        return Err(Http1Error::UnsupportedContentEncoding(coding.to_string()));
    };

    // Defense in depth: ath_deflate already caps output, but enforce the response
    // body limit on the decoded size too.
    if decoded.len() > limits.max_body_bytes {
        return Err(Http1Error::BodyTooLarge);
    }
    Ok(decoded)
}

/// Decode a `Content-Encoding: br` (brotli) body. With the `brotli` feature on,
/// uses the no_std brotli-decompressor (RFC 7932 incl. the static dictionary +
/// transforms — impractical to hand-roll); off, it's a clean Unsupported error
/// so a `br`-advertising mistake never silently yields garbage.
#[cfg(feature = "brotli")]
fn decode_brotli(input: &[u8], max_out: usize) -> Http1Result<Vec<u8>> {
    // The no_std decoder splits one buffer into the decoded-output region and its
    // working scratch; `decoded_size` reports the real output length. Provision
    // the output bound plus a window-scratch headroom.
    let mut buf: Vec<u8> = Vec::new();
    buf.resize(max_out.saturating_add(8 * 1024 * 1024), 0u8);
    let info = brotli_decompressor::brotli_decode(input, &mut buf);
    match info.result {
        brotli_decompressor::BrotliResult::ResultSuccess => {}
        _ => return Err(Http1Error::BadContentEncoding),
    }
    if info.decoded_size > max_out {
        return Err(Http1Error::BodyTooLarge);
    }
    buf.truncate(info.decoded_size);
    Ok(buf)
}

#[cfg(not(feature = "brotli"))]
fn decode_brotli(_input: &[u8], _max_out: usize) -> Http1Result<Vec<u8>> {
    Err(Http1Error::UnsupportedContentEncoding(String::from("br")))
}

// ---------------------------------------------------------------------------
// Response parsing — never panics on hostile input.
// ---------------------------------------------------------------------------

/// Find a CRLF in `buf` starting at `from`. Returns the index of `\r`.
fn find_crlf(buf: &[u8], from: usize) -> Option<usize> {
    if from >= buf.len() {
        return None;
    }
    let mut i = from;
    while i + 1 < buf.len() {
        if buf[i] == b'\r' && buf[i + 1] == b'\n' {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// Parse a status line: `HTTP/1.1 200 OK` (reason optional/empty).
fn parse_status_line(line: &[u8]) -> Http1Result<(u8, u16, String)> {
    // Version token.
    let mut sp = 0;
    while sp < line.len() && line[sp] != b' ' {
        sp += 1;
    }
    if sp == 0 || sp >= line.len() {
        return Err(Http1Error::BadStatusLine);
    }
    let version = &line[..sp];
    if !version.starts_with(b"HTTP/1.") || version.len() != 8 {
        return Err(Http1Error::BadStatusLine);
    }
    let minor = match version[7] {
        b'0' => 0u8,
        b'1' => 1u8,
        _ => return Err(Http1Error::BadStatusLine),
    };

    // Status code: exactly 3 ASCII digits.
    let rest = &line[sp + 1..];
    let mut cp = 0;
    while cp < rest.len() && rest[cp] != b' ' {
        cp += 1;
    }
    let code_bytes = &rest[..cp];
    if code_bytes.len() != 3 || !code_bytes.iter().all(|b| b.is_ascii_digit()) {
        return Err(Http1Error::BadStatusLine);
    }
    let status = (code_bytes[0] - b'0') as u16 * 100
        + (code_bytes[1] - b'0') as u16 * 10
        + (code_bytes[2] - b'0') as u16;

    // Reason phrase (rest after the space, may be empty / absent).
    let reason = if cp < rest.len() {
        // skip the single space
        let r = &rest[cp + 1..];
        String::from_utf8_lossy(r).into_owned()
    } else {
        String::new()
    };

    Ok((minor, status, reason))
}

/// Parse a single header line `Name: value` (value already CRLF-stripped, but
/// may need leading/trailing OWS trimmed). Obsolete line folding (a continuation
/// line beginning with SP/HTAB) is appended by the caller.
fn parse_header_line(line: &[u8]) -> Http1Result<(String, String)> {
    let colon = line
        .iter()
        .position(|&b| b == b':')
        .ok_or(Http1Error::BadHeader)?;
    if colon == 0 {
        return Err(Http1Error::BadHeader);
    }
    let name_bytes = &line[..colon];
    // Field name must be a valid token (no spaces, no control chars).
    if name_bytes
        .iter()
        .any(|&b| b == b' ' || b == b'\t' || b < 0x21 || b == 0x7f)
    {
        return Err(Http1Error::BadHeader);
    }
    let name = String::from_utf8_lossy(name_bytes).into_owned();
    let value = trim_ows(&line[colon + 1..]);
    let value = String::from_utf8_lossy(value).into_owned();
    Ok((name, value))
}

fn trim_ows(mut s: &[u8]) -> &[u8] {
    while let [first, rest @ ..] = s {
        if *first == b' ' || *first == b'\t' {
            s = rest;
        } else {
            break;
        }
    }
    while let [rest @ .., last] = s {
        if *last == b' ' || *last == b'\t' {
            s = rest;
        } else {
            break;
        }
    }
    s
}

/// Result of parsing the head (status + headers); records where the body begins.
struct Head {
    version_minor: u8,
    status: u16,
    reason: String,
    headers: Vec<(String, String)>,
    body_start: usize,
}

/// Parse status line + headers from `buf`. Returns `None`-style EOF as a clean
/// `Err(UnexpectedEof)` when the head terminator (`\r\n\r\n`) is absent.
fn parse_head(buf: &[u8], limits: &Limits) -> Http1Result<Head> {
    // Locate the end of the header block.
    let mut term = None;
    let mut i = 0;
    while i + 3 < buf.len() {
        if &buf[i..i + 4] == b"\r\n\r\n" {
            term = Some(i);
            break;
        }
        i += 1;
    }
    let header_end = term.ok_or(Http1Error::UnexpectedEof)?;
    if header_end > limits.max_header_bytes {
        return Err(Http1Error::HeadersTooLarge);
    }
    let body_start = header_end + 4;

    // Status line.
    let sl_end = find_crlf(buf, 0).ok_or(Http1Error::BadStatusLine)?;
    let (version_minor, status, reason) = parse_status_line(&buf[..sl_end])?;

    // Header lines, with obsolete-fold support.
    let mut headers: Vec<(String, String)> = Vec::new();
    let mut pos = sl_end + 2;
    while pos < header_end {
        let line_end = find_crlf(buf, pos).ok_or(Http1Error::BadHeader)?;
        let line = &buf[pos..line_end];
        if line.is_empty() {
            break;
        }
        // Obsolete line folding: a line starting with SP/HTAB continues the
        // previous header value.
        if line[0] == b' ' || line[0] == b'\t' {
            let folded = trim_ows(line);
            if let Some(last) = headers.last_mut() {
                last.1.push(' ');
                last.1.push_str(&String::from_utf8_lossy(folded));
            } else {
                return Err(Http1Error::BadHeader);
            }
        } else {
            if headers.len() >= limits.max_header_count {
                return Err(Http1Error::HeadersTooLarge);
            }
            headers.push(parse_header_line(line)?);
        }
        pos = line_end + 2;
    }

    Ok(Head {
        version_minor,
        status,
        reason,
        headers,
        body_start,
    })
}

/// How the body is framed.
enum Framing {
    Length(usize),
    Chunked,
    /// No length and no chunked → body is everything until EOF.
    UntilEof,
    /// No body permitted (HEAD, 1xx/204/304).
    None,
}

fn determine_framing(head: &Head, req_method: Method, limits: &Limits) -> Http1Result<Framing> {
    // Responses that never carry a body.
    if req_method.is_bodyless_response()
        || head.status == 204
        || head.status == 304
        || (100..200).contains(&head.status)
    {
        return Ok(Framing::None);
    }

    // Transfer-Encoding takes precedence over Content-Length (RFC 9112 §6.1).
    let te = head
        .headers
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case("transfer-encoding"))
        .map(|(_, v)| v.as_str());
    if let Some(te) = te {
        // The last coding being "chunked" is the framing case we decode.
        if te
            .rsplit(',')
            .next()
            .map(|c| c.trim().eq_ignore_ascii_case("chunked"))
            .unwrap_or(false)
        {
            return Ok(Framing::Chunked);
        }
        // Any other transfer coding we don't implement; treat as EOF-framed
        // rather than guessing (and never panic).
        return Ok(Framing::UntilEof);
    }

    if let Some((_, v)) = head
        .headers
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case("content-length"))
    {
        let v = v.trim();
        if v.is_empty() || !v.bytes().all(|b| b.is_ascii_digit()) {
            return Err(Http1Error::BadContentLength);
        }
        let len: usize = v.parse().map_err(|_| Http1Error::BadContentLength)?;
        if len > limits.max_body_bytes {
            return Err(Http1Error::BodyTooLarge);
        }
        return Ok(Framing::Length(len));
    }

    Ok(Framing::UntilEof)
}

/// Decode a chunked body starting at `body` (the bytes after the head). Returns
/// the decoded payload. Trailers are consumed and discarded. NEVER panics.
fn decode_chunked(body: &[u8], limits: &Limits) -> Http1Result<Vec<u8>> {
    let mut out: Vec<u8> = Vec::new();
    let mut pos = 0usize;

    loop {
        // Read chunk-size line.
        let line_end = find_crlf(body, pos).ok_or(Http1Error::BadChunk)?;
        let size_line = &body[pos..line_end];
        // chunk-size may be followed by ';' chunk-ext — take hex prefix only.
        let hex_end = size_line
            .iter()
            .position(|&b| b == b';')
            .unwrap_or(size_line.len());
        let hex = &size_line[..hex_end];
        let chunk_size = parse_hex(hex)?;
        if chunk_size > limits.max_chunk_bytes {
            return Err(Http1Error::BadChunk);
        }
        pos = line_end + 2;

        if chunk_size == 0 {
            // Last chunk. Consume optional trailer headers up to the final CRLF.
            // Find the terminating empty line; trailers are discarded.
            loop {
                let te = find_crlf(body, pos).ok_or(Http1Error::BadChunk)?;
                if te == pos {
                    // empty line → end of trailers
                    break;
                }
                pos = te + 2;
            }
            break;
        }

        // Chunk data: exactly chunk_size bytes, then CRLF.
        let data_end = pos.checked_add(chunk_size).ok_or(Http1Error::BadChunk)?;
        if data_end + 2 > body.len() {
            return Err(Http1Error::BadChunk);
        }
        if out.len() + chunk_size > limits.max_body_bytes {
            return Err(Http1Error::BodyTooLarge);
        }
        out.extend_from_slice(&body[pos..data_end]);
        // Verify the trailing CRLF.
        if body[data_end] != b'\r' || body[data_end + 1] != b'\n' {
            return Err(Http1Error::BadChunk);
        }
        pos = data_end + 2;
    }

    Ok(out)
}

/// Parse an ASCII hex string to a `usize`, never panicking. Empty → error.
fn parse_hex(hex: &[u8]) -> Http1Result<usize> {
    if hex.is_empty() {
        return Err(Http1Error::BadChunk);
    }
    let mut val: usize = 0;
    for &b in hex {
        let d = match b {
            b'0'..=b'9' => (b - b'0') as usize,
            b'a'..=b'f' => (b - b'a' + 10) as usize,
            b'A'..=b'F' => (b - b'A' + 10) as usize,
            _ => return Err(Http1Error::BadChunk),
        };
        val = val
            .checked_mul(16)
            .and_then(|v| v.checked_add(d))
            .ok_or(Http1Error::BadChunk)?;
    }
    Ok(val)
}

/// Parse a complete response from a single in-memory buffer.
///
/// `req_method` is needed because a HEAD response carries no body even with a
/// Content-Length. This treats `buf` as the entire byte stream received (head +
/// however much body arrived); for EOF-framed bodies it takes everything after
/// the head. NEVER panics on malformed input.
pub fn parse_response(
    buf: &[u8],
    req_method: Method,
    limits: &Limits,
) -> Http1Result<Http1Response> {
    let head = parse_head(buf, limits)?;
    let body_bytes = &buf[head.body_start..];

    let body = match determine_framing(&head, req_method, limits)? {
        Framing::None => Vec::new(),
        Framing::Length(n) => {
            if body_bytes.len() < n {
                return Err(Http1Error::UnexpectedEof);
            }
            body_bytes[..n].to_vec()
        }
        Framing::Chunked => decode_chunked(body_bytes, limits)?,
        Framing::UntilEof => {
            if body_bytes.len() > limits.max_body_bytes {
                return Err(Http1Error::BodyTooLarge);
            }
            body_bytes.to_vec()
        }
    };

    Ok(Http1Response {
        version_minor: head.version_minor,
        status: head.status,
        reason: head.reason,
        headers: head.headers,
        body,
    })
}

// ---------------------------------------------------------------------------
// URL parsing (http only)
// ---------------------------------------------------------------------------

/// A parsed `http://host[:port]/path` URL. The browser/HTTPS path lives in
/// `https.rs`; this is the minimal target the plain-HTTP `fetch` needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpUrl {
    pub host: String,
    pub port: u16,
    /// Request target including any query string; always starts with `/`.
    pub path: String,
}

impl HttpUrl {
    /// Authority for the `Host:` header (omits the default port 80).
    pub fn authority(&self) -> String {
        if self.port == 80 {
            self.host.clone()
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }

    /// Parse `http://host[:port][/path][?query]`. Rejects non-http schemes,
    /// empty host, and bad ports. NEVER panics.
    pub fn parse(url: &str) -> Http1Result<HttpUrl> {
        let rest = url
            .strip_prefix("http://")
            .ok_or_else(|| Http1Error::InvalidUrl("scheme must be http://".to_string()))?;

        // Strip fragment (never sent on the wire).
        let rest = match rest.find('#') {
            Some(p) => &rest[..p],
            None => rest,
        };

        // Split authority from the request target at the first '/' or '?'. Splitting
        // only on '/' folded a query-without-path (`http://host?q=1`) into the host
        // and dropped the query.
        let (authority, target) = match rest.find(|c: char| c == '/' || c == '?') {
            Some(p) => (&rest[..p], &rest[p..]),
            None => (rest, ""),
        };

        if authority.is_empty() {
            return Err(Http1Error::InvalidUrl("empty host".to_string()));
        }

        // Reject embedded userinfo for plain HTTP (keep the surface tight).
        if authority.contains('@') {
            return Err(Http1Error::InvalidUrl("userinfo not supported".to_string()));
        }

        let (host, port) = match authority.rfind(':') {
            Some(p) => {
                let h = &authority[..p];
                let port_str = &authority[p + 1..];
                let port = port_str
                    .parse::<u16>()
                    .map_err(|_| Http1Error::InvalidUrl("bad port".to_string()))?;
                if port == 0 {
                    return Err(Http1Error::InvalidUrl("bad port".to_string()));
                }
                (h, port)
            }
            None => (authority, 80u16),
        };

        if host.is_empty() {
            return Err(Http1Error::InvalidUrl("empty host".to_string()));
        }

        // Origin-form request target: an absolute path plus any query. A bare query
        // (empty path) gets the required leading "/".
        let path = if target.is_empty() {
            "/".to_string()
        } else if target.starts_with('?') {
            format!("/{}", target)
        } else {
            target.to_string()
        };

        Ok(HttpUrl {
            host: host.to_string(),
            port,
            path,
        })
    }
}

// ---------------------------------------------------------------------------
// Transport seam
// ---------------------------------------------------------------------------

/// The byte transport the HTTP logic rides on. The protocol code is fully
/// testable on the host with a mock implementation; the real implementation
/// wraps the athnet/athkit TCP socket + DNS resolve once a userspace socket
/// wrapper is available (see NEEDS-INTERFACE in the module slice notes).
pub trait HttpTransport {
    /// Resolve + connect to `host:port`. After this returns `Ok`, `send`/`recv`
    /// operate on the established stream.
    fn connect(&mut self, host: &str, port: u16) -> Http1Result<()>;
    /// Send all of `buf`. Implementations should loop internally on partial
    /// writes; a short write that cannot complete is a `Transport` error.
    fn send(&mut self, buf: &[u8]) -> Http1Result<()>;
    /// Read up to `buf.len()` bytes; returns the count. `Ok(0)` means EOF.
    fn recv(&mut self, buf: &mut [u8]) -> Http1Result<usize>;
}

/// Drive a request to completion over `transport` and parse the response.
///
/// Reads until either the framed body is complete or the peer closes (EOF). For
/// Content-Length and chunked bodies we read until the message is structurally
/// complete; for EOF-framed bodies we read until `recv` returns 0.
pub fn send_request<T: HttpTransport>(
    transport: &mut T,
    req: &Http1Request,
    limits: &Limits,
) -> Http1Result<Http1Response> {
    let wire = req.build();
    transport.send(&wire)?;

    let mut buf: Vec<u8> = Vec::with_capacity(2048);
    let mut tmp = [0u8; 2048];

    loop {
        // Attempt a parse with what we have so far; only EOF-class errors mean
        // "need more data" — everything else is a hard, non-panicking failure.
        match parse_response(&buf, req.method, limits) {
            Ok(resp) => return finalize_response(resp, limits),
            Err(Http1Error::UnexpectedEof) | Err(Http1Error::BadChunk) if !buf.is_empty() => {
                // BadChunk here can be a transient "chunk not fully arrived"
                // state; keep reading and let a real malformation surface once
                // the stream ends.
            }
            Err(Http1Error::BadStatusLine) | Err(Http1Error::BadHeader) if buf.is_empty() => {}
            Err(e @ Http1Error::HeadersTooLarge)
            | Err(e @ Http1Error::BodyTooLarge)
            | Err(e @ Http1Error::BadContentLength) => return Err(e),
            Err(_) => {}
        }

        if buf.len() > limits.max_header_bytes + limits.max_body_bytes {
            return Err(Http1Error::BodyTooLarge);
        }

        let n = transport.recv(&mut tmp)?;
        if n == 0 {
            // Peer closed. Final parse attempt: for EOF-framed bodies this is
            // exactly the completion signal.
            return parse_response(&buf, req.method, limits)
                .and_then(|resp| finalize_response(resp, limits));
        }
        buf.extend_from_slice(&tmp[..n]);
    }
}

/// Apply `Content-Encoding` decoding to a structurally-complete response so the
/// high-level `fetch`/`send_request` callers receive decoded content. The raw,
/// transfer-decoded body is fully present at this point, so decompression is
/// safe. `parse_response` itself stays content-encoding-agnostic.
fn finalize_response(mut resp: Http1Response, limits: &Limits) -> Http1Result<Http1Response> {
    if let Some(enc) = resp.content_encoding().map(|s| s.to_string()) {
        let body = core::mem::take(&mut resp.body);
        resp.body = decode_content_encoding(Some(&enc), body, limits)?;
    }
    Ok(resp)
}

/// Parse a `http://` URL, connect via `transport`, GET it, and FOLLOW up to 10
/// `http://` redirects (real sites redirect constantly: trailing-slash, canonical
/// host, http→https). The high-level entry point for `http://` fetches. A
/// redirect to `https://` (which needs TLS, not wired here) or an exhausted hop
/// budget hands the 3xx response back to the caller.
pub fn fetch<T: HttpTransport>(url: &str, transport: &mut T) -> Http1Result<Http1Response> {
    fetch_follow(url, transport, 10)
}

/// GET `url`, following up to `max_redirects` `http://` 3xx redirects. Each hop
/// resolves the `Location` header (absolute URL or path) against the current URL
/// and re-connects (single-shot `Connection: close`). Stops and returns the last
/// response when it is not a redirect, has no/unfollowable `Location`, or the hop
/// budget is exhausted — never loops unboundedly.
pub fn fetch_follow<T: HttpTransport>(
    url: &str,
    transport: &mut T,
    max_redirects: u32,
) -> Http1Result<Http1Response> {
    let mut current = String::from(url);
    let mut hops = 0u32;
    loop {
        let resp = fetch_with(&current, Method::Get, None, &[], transport, &Limits::new())?;
        if !resp.is_redirect() || hops >= max_redirects {
            return Ok(resp);
        }
        let location = match resp.header("location") {
            Some(l) if !l.trim().is_empty() => l.trim().to_string(),
            _ => return Ok(resp), // 3xx without a usable Location: hand it back
        };
        let next = resolve_redirect(&current, &location)?;
        // Only plain http:// is followable here; https:// needs TLS (deferred).
        if !next.starts_with("http://") {
            return Ok(resp);
        }
        current = next;
        hops += 1;
    }
}

/// Resolve a redirect `Location` (absolute URL, absolute path `/...`, or a
/// relative path) against the current `base` URL (always `http://...`).
/// RFC 3986 §5.2.4 dot-segment removal on an absolute path: resolve `.` and `..`.
/// `..` past the root is a no-op (stays at root); a trailing dir-marker (`/`, `/.`,
/// `/..`) keeps a trailing slash. Empty segments (`//`) are preserved.
fn normalize_path(path: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    for seg in path.split('/') {
        match seg {
            "." => {}
            ".." => {
                if out.len() > 1 {
                    out.pop();
                }
            }
            other => out.push(other),
        }
    }
    let mut result = out.join("/");
    if result.is_empty() {
        result.push('/');
    }
    let dir_like = path.ends_with('/') || path.ends_with("/.") || path.ends_with("/..");
    if dir_like && !result.ends_with('/') {
        result.push('/');
    }
    result
}

fn resolve_redirect(base: &str, location: &str) -> Http1Result<String> {
    if location.starts_with("http://") || location.starts_with("https://") {
        return Ok(String::from(location));
    }
    let parsed = HttpUrl::parse(base)?;
    let authority = parsed.authority();

    // Protocol-relative `//host/path` inherits the scheme but switches host.
    if let Some(rest) = location.strip_prefix("//") {
        return Ok(format!("http://{rest}"));
    }

    // Keep the location's own query out of path normalization.
    let (loc_path, loc_query) = match location.split_once('?') {
        Some((p, q)) => (p, Some(q)),
        None => (location, None),
    };

    let merged = if loc_path.starts_with('/') {
        loc_path.to_string()
    } else {
        // Relative to the base path's directory.
        let base_path = parsed.path.split('?').next().unwrap_or("/");
        let dir = match base_path.rfind('/') {
            Some(i) => &base_path[..=i],
            None => "/",
        };
        format!("{dir}{loc_path}")
    };
    let path = normalize_path(&merged);
    match loc_query {
        Some(q) => Ok(format!("http://{authority}{path}?{q}")),
        None => Ok(format!("http://{authority}{path}")),
    }
}

/// Like [`fetch_follow`] but threads a [`CookieJar`](crate::cookies::CookieJar):
/// matching cookies are sent on each request and the `Set-Cookie`s from each
/// response (including a redirect's) are stored — so a login flow that sets a
/// session cookie on a 302 and redirects carries it through to the destination.
/// `now` is the current unix time (cookie expiry). http:// only; `secure` cookies
/// are withheld (this path is not TLS).
pub fn fetch_follow_jar<T: HttpTransport>(
    url: &str,
    transport: &mut T,
    max_redirects: u32,
    jar: &mut crate::cookies::CookieJar,
    now: u64,
) -> Http1Result<Http1Response> {
    let mut current = String::from(url);
    let mut hops = 0u32;
    loop {
        let parsed = HttpUrl::parse(&current)?;
        let cookie_hdr = jar.cookie_header(&parsed.host, &parsed.path, false, now);
        let mut extra: Vec<(&str, &str)> = Vec::new();
        if let Some(h) = &cookie_hdr {
            extra.push(("Cookie", h.as_str()));
        }
        let resp = fetch_with(
            &current,
            Method::Get,
            None,
            &extra,
            transport,
            &Limits::new(),
        )?;
        for sc in resp.header_all("set-cookie") {
            jar.store(sc, &parsed.host, &parsed.path, now);
        }
        if !resp.is_redirect() || hops >= max_redirects {
            return Ok(resp);
        }
        let location = match resp.header("location") {
            Some(l) if !l.trim().is_empty() => l.trim().to_string(),
            _ => return Ok(resp),
        };
        let next = resolve_redirect(&current, &location)?;
        if !next.starts_with("http://") {
            return Ok(resp);
        }
        current = next;
        hops += 1;
    }
}

/// Generalized fetch: choose method, body, extra headers, and limits.
pub fn fetch_with<T: HttpTransport>(
    url: &str,
    method: Method,
    body: Option<Vec<u8>>,
    extra_headers: &[(&str, &str)],
    transport: &mut T,
    limits: &Limits,
) -> Http1Result<Http1Response> {
    let parsed = HttpUrl::parse(url)?;
    transport.connect(&parsed.host, parsed.port)?;

    let mut req = Http1Request::new(method, parsed.path.clone(), parsed.authority());
    for (k, v) in extra_headers {
        req = req.header(*k, *v);
    }
    // Advertise the content codings we can decode so servers actually compress
    // the response (otherwise `decode_content_encoding` never has work to do).
    // The caller may override by passing their own Accept-Encoding.
    if !req
        .headers
        .iter()
        .any(|(n, _)| n.eq_ignore_ascii_case("accept-encoding"))
    {
        // `br` is advertised only when we can actually decode it (the `brotli`
        // feature) — never claim an encoding we can't handle.
        #[cfg(feature = "brotli")]
        let ae = "gzip, deflate, br";
        #[cfg(not(feature = "brotli"))]
        let ae = "gzip, deflate";
        req = req.header("Accept-Encoding", ae);
    }
    req.body = body;

    send_request(transport, &req, limits)
}

// ---------------------------------------------------------------------------
// Mock transport (host-test + reference for the real socket adapter)
// ---------------------------------------------------------------------------

/// A canned transport: returns a fixed response and records what was sent and
/// where it connected. Used by the host KATs to drive `fetch`/`send_request`
/// end-to-end without a live socket, and as the reference shape the real
/// athnet/athkit socket adapter must satisfy.
#[derive(Debug, Clone)]
pub struct MockTransport {
    /// The bytes the peer will deliver, drained across `recv` calls.
    pub response: Vec<u8>,
    /// Captured request bytes (everything passed to `send`).
    pub sent: Vec<u8>,
    /// `(host, port)` captured at `connect`.
    pub connected_to: Option<(String, u16)>,
    /// Bytes returned per `recv` (to exercise the incremental read loop).
    pub recv_chunk: usize,
    read_pos: usize,
}

impl MockTransport {
    pub fn new(response: Vec<u8>) -> Self {
        Self {
            response,
            sent: Vec::new(),
            connected_to: None,
            recv_chunk: usize::MAX,
            read_pos: 0,
        }
    }

    /// Deliver the canned response in `chunk`-sized pieces (drip-feed) to prove
    /// the incremental read loop reassembles correctly.
    pub fn with_recv_chunk(mut self, chunk: usize) -> Self {
        self.recv_chunk = chunk.max(1);
        self
    }
}

impl HttpTransport for MockTransport {
    fn connect(&mut self, host: &str, port: u16) -> Http1Result<()> {
        self.connected_to = Some((host.to_string(), port));
        Ok(())
    }

    fn send(&mut self, buf: &[u8]) -> Http1Result<()> {
        self.sent.extend_from_slice(buf);
        Ok(())
    }

    fn recv(&mut self, buf: &mut [u8]) -> Http1Result<usize> {
        let remaining = self.response.len().saturating_sub(self.read_pos);
        if remaining == 0 {
            return Ok(0);
        }
        let n = remaining.min(buf.len()).min(self.recv_chunk);
        buf[..n].copy_from_slice(&self.response[self.read_pos..self.read_pos + n]);
        self.read_pos += n;
        Ok(n)
    }
}

// ---------------------------------------------------------------------------
// Tests — host KATs (cargo test -p athnet)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    // ---- request build: exact bytes (FAIL-able) -------------------------

    #[test]
    fn build_get_exact_bytes() {
        let req = Http1Request::get("/index.html", "example.com");
        let wire = req.build();
        let expected =
            b"GET /index.html HTTP/1.1\r\nHost: example.com\r\nConnection: close\r\n\r\n";
        assert_eq!(wire, expected, "GET request bytes must match exactly");
    }

    #[test]
    fn build_post_content_length_exact() {
        let req = Http1Request::post("/submit", "api.test:8080", b"hello".to_vec())
            .header("Content-Type", "text/plain");
        let wire = req.build();
        let expected = b"POST /submit HTTP/1.1\r\n\
Host: api.test:8080\r\n\
Connection: close\r\n\
Content-Type: text/plain\r\n\
Content-Length: 5\r\n\
\r\n\
hello";
        // FAIL-ABILITY: changing the body length, dropping a CRLF, or
        // miscounting Content-Length (e.g. 4 or 6) flips this assert.
        assert_eq!(wire, expected);
    }

    #[test]
    fn build_head_no_body() {
        let req = Http1Request::head("/", "h.example");
        let wire = req.build();
        assert_eq!(
            wire,
            b"HEAD / HTTP/1.1\r\nHost: h.example\r\nConnection: close\r\n\r\n"
        );
    }

    #[test]
    fn build_respects_caller_connection_header() {
        let req = Http1Request::get("/x", "h").header("Connection", "keep-alive");
        let wire = req.build();
        // We must NOT also emit our default Connection: close.
        let s = String::from_utf8_lossy(&wire);
        assert_eq!(s.matches("Connection:").count(), 1);
        assert!(s.contains("Connection: keep-alive"));
    }

    // ---- response parse: Content-Length ---------------------------------

    #[test]
    fn parse_content_length() {
        let raw = b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nServer: rae\r\n\r\nhello";
        let resp = parse_response(raw, Method::Get, &Limits::new()).unwrap();
        assert_eq!(resp.status, 200);
        assert_eq!(resp.reason, "OK");
        assert_eq!(resp.version_minor, 1);
        assert_eq!(resp.body, b"hello");
        assert_eq!(resp.header("server"), Some("rae"));
    }

    #[test]
    fn parse_no_reason_phrase() {
        let raw = b"HTTP/1.0 204 \r\n\r\n";
        let resp = parse_response(raw, Method::Get, &Limits::new()).unwrap();
        assert_eq!(resp.status, 204);
        assert_eq!(resp.version_minor, 0);
        assert!(resp.body.is_empty());
    }

    #[test]
    fn head_response_ignores_content_length_body() {
        // HEAD: Content-Length present but body must be empty.
        let raw = b"HTTP/1.1 200 OK\r\nContent-Length: 99\r\n\r\n";
        let resp = parse_response(raw, Method::Head, &Limits::new()).unwrap();
        assert!(resp.body.is_empty());
    }

    // ---- response parse: chunked (multi-chunk + trailer) ----------------

    #[test]
    fn parse_chunked_multi_with_trailer() {
        // "Wiki" + "pedia" + " in\r\n\r\nchunks." then a trailer header.
        // NB: built by concatenation (not a `\`-continued literal) because a
        // byte-string line continuation strips leading whitespace, which would
        // silently corrupt the third chunk's leading space.
        let mut raw: Vec<u8> = Vec::new();
        raw.extend_from_slice(b"HTTP/1.1 200 OK\r\n");
        raw.extend_from_slice(b"Transfer-Encoding: chunked\r\n");
        raw.extend_from_slice(b"\r\n");
        raw.extend_from_slice(b"4\r\nWiki\r\n");
        raw.extend_from_slice(b"5\r\npedia\r\n");
        // chunk-size 0xE = 14 bytes: " in\r\n\r\nchunks."
        raw.extend_from_slice(b"E\r\n in\r\n\r\nchunks.\r\n");
        raw.extend_from_slice(b"0\r\nX-Trailer: done\r\n\r\n");
        let resp = parse_response(&raw, Method::Get, &Limits::new()).unwrap();
        // FAIL-ABILITY: if chunk-size hex decode or the per-chunk CRLF check
        // breaks, the reassembled body differs and THIS assert flips.
        assert_eq!(resp.body, b"Wikipedia in\r\n\r\nchunks.");
    }

    #[test]
    fn parse_chunked_with_extension() {
        let raw = b"HTTP/1.1 200 OK\r\n\
Transfer-Encoding: chunked\r\n\
\r\n\
5;name=value\r\n\
abcde\r\n\
0\r\n\
\r\n";
        let resp = parse_response(raw, Method::Get, &Limits::new()).unwrap();
        assert_eq!(resp.body, b"abcde");
    }

    #[test]
    fn chunked_takes_precedence_over_content_length() {
        let raw = b"HTTP/1.1 200 OK\r\n\
Content-Length: 100\r\n\
Transfer-Encoding: chunked\r\n\
\r\n\
3\r\n\
abc\r\n\
0\r\n\
\r\n";
        let resp = parse_response(raw, Method::Get, &Limits::new()).unwrap();
        assert_eq!(resp.body, b"abc");
    }

    // ---- header case-insensitivity, dups, folding -----------------------

    #[test]
    fn header_case_insensitive_and_dups() {
        let raw = b"HTTP/1.1 200 OK\r\n\
Content-Length: 0\r\n\
Set-Cookie: a=1\r\n\
set-cookie: b=2\r\n\
\r\n";
        let resp = parse_response(raw, Method::Get, &Limits::new()).unwrap();
        assert_eq!(resp.header("CONTENT-LENGTH"), Some("0"));
        let cookies: Vec<&str> = resp.header_all("Set-Cookie").collect();
        assert_eq!(cookies, vec!["a=1", "b=2"]);
    }

    #[test]
    fn header_obsolete_folding() {
        // Built by concatenation: a `\`-continued byte literal would strip the
        // leading space of the continuation line, defeating the fold test.
        let mut raw: Vec<u8> = Vec::new();
        raw.extend_from_slice(b"HTTP/1.1 200 OK\r\n");
        raw.extend_from_slice(b"Content-Length: 0\r\n");
        raw.extend_from_slice(b"X-Long: part1\r\n");
        raw.extend_from_slice(b" part2\r\n");
        raw.extend_from_slice(b"\r\n");
        let resp = parse_response(&raw, Method::Get, &Limits::new()).unwrap();
        assert_eq!(resp.header("X-Long"), Some("part1 part2"));
    }

    // ---- URL parsing ----------------------------------------------------

    #[test]
    fn url_parse_basic() {
        let u = HttpUrl::parse("http://example.com/path?x=1").unwrap();
        assert_eq!(u.host, "example.com");
        assert_eq!(u.port, 80);
        assert_eq!(u.path, "/path?x=1");
        assert_eq!(u.authority(), "example.com");
    }

    #[test]
    fn url_parse_explicit_port() {
        let u = HttpUrl::parse("http://10.0.0.1:8080/api").unwrap();
        assert_eq!(u.host, "10.0.0.1");
        assert_eq!(u.port, 8080);
        assert_eq!(u.path, "/api");
        assert_eq!(u.authority(), "10.0.0.1:8080");
    }

    #[test]
    fn url_parse_no_path_defaults_slash() {
        let u = HttpUrl::parse("http://host.local").unwrap();
        assert_eq!(u.path, "/");
    }

    #[test]
    fn url_parse_query_without_path() {
        // `http://host?query` (no path) must keep the host clean and place the query
        // in an origin-form target ("/?..."), not fold it into the host.
        let u = HttpUrl::parse("http://example.com?utm=x&a=1").unwrap();
        assert_eq!(u.host, "example.com");
        assert_eq!(u.port, 80);
        assert_eq!(u.path, "/?utm=x&a=1");
        assert_eq!(u.authority(), "example.com");
        // With an explicit port too.
        let u2 = HttpUrl::parse("http://example.com:8080?q=1").unwrap();
        assert_eq!(u2.host, "example.com");
        assert_eq!(u2.port, 8080);
        assert_eq!(u2.path, "/?q=1");
    }

    #[test]
    fn url_parse_rejects_non_http() {
        assert!(HttpUrl::parse("https://example.com/").is_err());
        assert!(HttpUrl::parse("ftp://example.com/").is_err());
        assert!(HttpUrl::parse("example.com").is_err());
    }

    #[test]
    fn url_parse_rejects_bad_port_and_empty_host() {
        assert!(HttpUrl::parse("http://host:notaport/").is_err());
        assert!(HttpUrl::parse("http://host:0/").is_err());
        assert!(HttpUrl::parse("http:///path").is_err());
    }

    // ---- full fetch round-trip over the mock transport ------------------

    #[test]
    fn fetch_round_trip_mock() {
        let canned = b"HTTP/1.1 200 OK\r\nContent-Length: 11\r\n\r\nhello world".to_vec();
        let mut t = MockTransport::new(canned);
        let resp = fetch("http://example.com:8080/greet", &mut t).unwrap();

        assert_eq!(resp.status, 200);
        assert_eq!(resp.body, b"hello world");
        // Connected to the right place.
        assert_eq!(t.connected_to, Some((String::from("example.com"), 8080)));
        // The request we sent is well-formed.
        let sent = String::from_utf8_lossy(&t.sent);
        assert!(sent.starts_with("GET /greet HTTP/1.1\r\n"));
        assert!(sent.contains("Host: example.com:8080\r\n"));
    }

    #[test]
    fn fetch_round_trip_chunked_dripfed() {
        // Drip-feed 3 bytes per recv to exercise the incremental read loop.
        let canned = b"HTTP/1.1 200 OK\r\n\
Transfer-Encoding: chunked\r\n\
\r\n\
4\r\n\
data\r\n\
0\r\n\
\r\n"
            .to_vec();
        let mut t = MockTransport::new(canned).with_recv_chunk(3);
        let resp = fetch("http://h/p", &mut t).unwrap();
        assert_eq!(resp.body, b"data");
    }

    #[test]
    fn fetch_eof_framed_body() {
        // No Content-Length, no chunked → body is until EOF.
        let canned = b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\n\r\nstreamed body".to_vec();
        let mut t = MockTransport::new(canned);
        let resp = fetch("http://h/", &mut t).unwrap();
        assert_eq!(resp.body, b"streamed body");
    }

    // ---- hostile / malformed inputs: all Err, NONE panic ----------------

    #[test]
    fn malformed_inputs_never_panic() {
        let lim = Limits::new();
        let cases: Vec<&[u8]> = vec![
            b"",
            b"\r\n\r\n",
            b"GARBAGE",
            b"HTTP/1.1\r\n\r\n",                     // no status code
            b"HTTP/1.1 \r\n\r\n",                    // empty code
            b"HTTP/1.1 20 OK\r\n\r\n",               // 2-digit code
            b"HTTP/1.1 2000 OK\r\n\r\n",             // 4-digit code
            b"HTTP/1.1 abc OK\r\n\r\n",              // non-numeric code
            b"HTTP/2.0 200 OK\r\n\r\n",              // wrong version
            b"HTTP/1.1 200 OK\r\nbadheader\r\n\r\n", // header without colon
            b"HTTP/1.1 200 OK\r\n: noname\r\n\r\n",  // empty header name
            b"HTTP/1.1 200 OK\r\nContent-Length: abc\r\n\r\nbody", // bad CL
            b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\nshort", // truncated body
            // chunked: bad hex size
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\nZZ\r\nxx\r\n0\r\n\r\n",
            // chunked: size larger than data (truncated)
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\nFF\r\nshort\r\n0\r\n\r\n",
            // chunked: missing terminating zero chunk
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n4\r\ndata\r\n",
            // head without terminator
            b"HTTP/1.1 200 OK\r\nContent-Length: 0",
        ];
        for case in cases {
            // The contract is: returns (Ok or Err) without panicking.
            let _ = parse_response(case, Method::Get, &lim);
        }
    }

    #[test]
    fn malformed_chunk_size_is_err() {
        let raw = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\nZZ\r\nxx\r\n0\r\n\r\n";
        let err = parse_response(raw, Method::Get, &Limits::new()).unwrap_err();
        assert_eq!(err, Http1Error::BadChunk);
    }

    #[test]
    fn bad_content_length_is_err() {
        let raw = b"HTTP/1.1 200 OK\r\nContent-Length: notanumber\r\n\r\nx";
        let err = parse_response(raw, Method::Get, &Limits::new()).unwrap_err();
        assert_eq!(err, Http1Error::BadContentLength);
    }

    #[test]
    fn header_count_limit_enforced() {
        let mut raw = String::from("HTTP/1.1 200 OK\r\n");
        for i in 0..10 {
            raw.push_str(&format!("X-{}: v\r\n", i));
        }
        raw.push_str("Content-Length: 0\r\n\r\n");
        let lim = Limits {
            max_header_count: 3,
            ..Limits::new()
        };
        let err = parse_response(raw.as_bytes(), Method::Get, &lim).unwrap_err();
        assert_eq!(err, Http1Error::HeadersTooLarge);
    }

    #[test]
    fn body_size_limit_enforced() {
        let raw = b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n";
        let lim = Limits {
            max_body_bytes: 10,
            ..Limits::new()
        };
        let err = parse_response(raw, Method::Get, &lim).unwrap_err();
        assert_eq!(err, Http1Error::BodyTooLarge);
    }

    // ───────────────────────── Fuzz / property hardening ─────────────────────
    //
    // `parse_response` decodes the raw bytes a remote HTTP server sends — fully
    // attacker-controlled. The dangerous surfaces are the chunked-transfer
    // decoder (a hex chunk-size that overflows or claims more than is present)
    // and the header scanner (slicing on found CRLF positions). These tests
    // assert: never panics on arbitrary / truncated / mutated input, chunk and
    // body sizes are clamped (no unbounded alloc / OOM), and no infinite loop.
    //
    // Self-contained xorshift PRNG — no external fuzz crate.

    struct Rng(u64);
    impl Rng {
        fn new(seed: u64) -> Self {
            Rng(seed ^ 0x9E37_79B9_7F4A_7C15)
        }
        fn next_u64(&mut self) -> u64 {
            let mut x = self.0;
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            self.0 = x;
            x
        }
        fn byte(&mut self) -> u8 {
            (self.next_u64() & 0xFF) as u8
        }
        fn below(&mut self, n: usize) -> usize {
            if n == 0 {
                0
            } else {
                (self.next_u64() % n as u64) as usize
            }
        }
    }

    /// Arbitrary bytes through the full response parser must never panic and
    /// must always return (Ok or Err) — never hang.
    #[test]
    fn fuzz_parse_response_random_never_panics() {
        let mut rng = Rng::new(0x48_54_54_50); // "HTTP"
        let lim = Limits::new();
        for _ in 0..30_000 {
            let len = rng.below(400);
            let mut buf = Vec::with_capacity(len);
            for _ in 0..len {
                buf.push(rng.byte());
            }
            let _ = parse_response(&buf, Method::Get, &lim);
        }
    }

    /// Random bytes biased toward CR/LF/digits/hex/';' — the chars that drive
    /// the header scanner and chunk decoder into their branches.
    #[test]
    fn fuzz_parse_response_structured_bytes_never_panics() {
        let mut rng = Rng::new(0xC0FFEE_11);
        let alphabet: &[u8] = b"\r\n0123456789abcdefABCDEF; :HTTP/.OKContent-LngthrasfeEodikuy";
        let lim = Limits::new();
        for _ in 0..30_000 {
            let len = rng.below(300);
            let mut buf = Vec::with_capacity(len);
            for _ in 0..len {
                buf.push(alphabet[rng.below(alphabet.len())]);
            }
            for m in [Method::Get, Method::Head, Method::Post] {
                let _ = parse_response(&buf, m, &lim);
            }
        }
    }

    /// Truncate a valid chunked response at every offset — each prefix parses to
    /// Ok or Err, never panic.
    #[test]
    fn fuzz_chunked_truncated_at_every_offset() {
        let raw = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n\
                    4\r\nWiki\r\n5\r\npedia\r\n0\r\n\r\n";
        let lim = Limits::new();
        for cut in 0..=raw.len() {
            let _ = parse_response(&raw[..cut], Method::Get, &lim);
        }
    }

    /// A chunk-size that is enormous (claims gigabytes) must be REJECTED against
    /// the chunk limit, never attempt to allocate it. Proves bounded alloc.
    #[test]
    fn fuzz_chunked_huge_size_is_clamped() {
        // 0xFFFFFFFF bytes claimed, only a few present.
        let raw = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n\
                    ffffffff\r\nABC";
        let lim = Limits::new(); // max_chunk_bytes = 16 MiB << 4 GiB
        let res = parse_response(raw, Method::Get, &lim);
        assert!(
            res.is_err(),
            "oversize chunk must be rejected, not allocated"
        );
    }

    /// A chunk-size hex string that would overflow usize must be rejected via
    /// checked arithmetic, not wrap to a small value.
    #[test]
    fn fuzz_chunked_overflow_hex_rejected() {
        // 17 hex 'f' digits overflows 64-bit usize.
        let raw = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n\
                    fffffffffffffffff\r\nx\r\n0\r\n\r\n";
        let lim = Limits::new();
        let res = parse_response(raw, Method::Get, &lim);
        assert!(res.is_err(), "overflowing chunk-size must be rejected");
    }

    /// A Content-Length far larger than the body present must error (UnexpectedEof
    /// or BodyTooLarge), never slice OOB and never allocate the claimed size.
    #[test]
    fn fuzz_content_length_overclaim_rejected() {
        let raw = b"HTTP/1.1 200 OK\r\nContent-Length: 1000000\r\n\r\nshort";
        let lim = Limits::new();
        let res = parse_response(raw, Method::Get, &lim);
        assert!(
            res.is_err(),
            "over-claimed Content-Length must error, not OOB/OOM"
        );
    }

    /// Mutated-valid: flip random bytes in a real chunked response. Near-valid
    /// inputs exercise the most branches; must never panic.
    #[test]
    fn fuzz_mutated_valid_response_never_panics() {
        let base: Vec<u8> = b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\n\
                              Transfer-Encoding: chunked\r\n\r\n\
                              a\r\n0123456789\r\n0\r\n\r\n"
            .to_vec();
        let mut rng = Rng::new(0xBEEF_CACE);
        let lim = Limits::new();
        for _ in 0..30_000 {
            let mut m = base.clone();
            let flips = 1 + rng.below(6);
            for _ in 0..flips {
                let idx = rng.below(m.len());
                m[idx] = rng.byte();
            }
            for meth in [Method::Get, Method::Head] {
                let _ = parse_response(&m, meth, &lim);
            }
        }
    }

    // ───────────────────────── Content-Encoding (gzip/deflate) ───────────────

    /// A `Content-Encoding: gzip` response is transparently decompressed by the
    /// high-level fetch, using ath_deflate's own gzip writer as the server.
    #[test]
    fn gzip_content_encoding_decoded_round_trip() {
        let plain = b"<!doctype html><title>AthenaOS</title><h1>hi</h1>";
        let gz = ath_deflate::gzip_compress(plain);
        let mut raw: Vec<u8> = Vec::new();
        raw.extend_from_slice(b"HTTP/1.1 200 OK\r\n");
        raw.extend_from_slice(b"Content-Encoding: gzip\r\n");
        raw.extend_from_slice(format!("Content-Length: {}\r\n\r\n", gz.len()).as_bytes());
        raw.extend_from_slice(&gz);

        let mut t = MockTransport::new(raw);
        let resp = fetch("http://example.com/", &mut t).unwrap();
        // FAIL-ABILITY: if content decoding is skipped, body == gzip bytes != plain.
        assert_eq!(resp.body, plain);
        // The header is preserved for transparency.
        assert_eq!(resp.content_encoding(), Some("gzip"));
    }

    /// `fetch` advertises `Accept-Encoding: gzip, deflate` so servers actually
    /// compress — without this header the content-decoding above never triggers
    /// on real fetches. A caller-supplied Accept-Encoding is respected (not duped).
    #[test]
    fn fetch_advertises_accept_encoding() {
        let canned = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nhi".to_vec();
        let mut t = MockTransport::new(canned);
        let _ = fetch("http://h/", &mut t).unwrap();
        let sent = String::from_utf8_lossy(&t.sent);
        // `gzip, deflate` is always advertised; `, br` is appended when the brotli
        // feature is on, so match the common prefix to pass under both.
        assert!(
            sent.contains("Accept-Encoding: gzip, deflate"),
            "fetch must advertise the codings it can decode; sent:\n{sent}"
        );

        // Caller override wins and is not duplicated.
        let canned2 = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nhi".to_vec();
        let mut t2 = MockTransport::new(canned2);
        let _ = fetch_with(
            "http://h/",
            Method::Get,
            None,
            &[("Accept-Encoding", "identity")],
            &mut t2,
            &Limits::new(),
        )
        .unwrap();
        let sent2 = String::from_utf8_lossy(&t2.sent);
        assert_eq!(
            sent2.matches("Accept-Encoding:").count(),
            1,
            "caller Accept-Encoding must not be duplicated"
        );
        assert!(sent2.contains("Accept-Encoding: identity\r\n"));
    }

    // ───────────────────────── Redirect following ────────────────────────────

    /// A transport that serves a SEQUENCE of canned responses, one per connect —
    /// to drive multi-hop redirect following.
    struct SeqTransport {
        responses: Vec<Vec<u8>>,
        current: usize,
        served_any: bool,
        read_pos: usize,
        connects: Vec<(String, u16)>,
        sent: Vec<Vec<u8>>,
    }
    impl SeqTransport {
        fn new(responses: Vec<Vec<u8>>) -> Self {
            Self {
                responses,
                current: 0,
                served_any: false,
                read_pos: 0,
                connects: Vec::new(),
                sent: Vec::new(),
            }
        }
    }
    impl HttpTransport for SeqTransport {
        fn connect(&mut self, host: &str, port: u16) -> Http1Result<()> {
            if self.served_any {
                self.current += 1;
            }
            self.served_any = true;
            self.read_pos = 0;
            self.connects.push((host.to_string(), port));
            Ok(())
        }
        fn send(&mut self, buf: &[u8]) -> Http1Result<()> {
            self.sent.push(buf.to_vec());
            Ok(())
        }
        fn recv(&mut self, buf: &mut [u8]) -> Http1Result<usize> {
            let resp = match self.responses.get(self.current) {
                Some(r) => r,
                None => return Ok(0),
            };
            let remaining = resp.len().saturating_sub(self.read_pos);
            if remaining == 0 {
                return Ok(0);
            }
            let n = remaining.min(buf.len());
            buf[..n].copy_from_slice(&resp[self.read_pos..self.read_pos + n]);
            self.read_pos += n;
            Ok(n)
        }
    }

    /// fetch follows a relative 302 to the final 200.
    #[test]
    fn fetch_follows_relative_302_to_200() {
        let r1 = b"HTTP/1.1 302 Found\r\nLocation: /final\r\nContent-Length: 0\r\n\r\n".to_vec();
        let r2 = b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello".to_vec();
        let mut t = SeqTransport::new(vec![r1, r2]);
        let resp = fetch("http://example.com/start", &mut t).unwrap();
        assert_eq!(resp.status, 200);
        assert_eq!(resp.body, b"hello");
        assert_eq!(
            t.connects.len(),
            2,
            "should connect twice (start + redirect)"
        );
        // The second connect targeted the same host (relative redirect).
        assert_eq!(t.connects[1].0, "example.com");
    }

    /// An absolute-URL redirect re-targets host + port.
    #[test]
    fn fetch_follows_absolute_url_redirect() {
        let r1 = b"HTTP/1.1 301 Moved\r\nLocation: http://other.example:8080/x\r\nContent-Length: 0\r\n\r\n".to_vec();
        let r2 = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok".to_vec();
        let mut t = SeqTransport::new(vec![r1, r2]);
        let resp = fetch("http://a/", &mut t).unwrap();
        assert_eq!(resp.body, b"ok");
        assert_eq!(t.connects[1], (String::from("other.example"), 8080));
    }

    /// A redirect loop terminates at the hop budget and returns the last 3xx —
    /// never loops forever.
    #[test]
    fn fetch_follow_bounds_hops() {
        let loop_resp =
            b"HTTP/1.1 302 Found\r\nLocation: /again\r\nContent-Length: 0\r\n\r\n".to_vec();
        let mut t = SeqTransport::new(vec![loop_resp; 10]);
        let resp = fetch_follow("http://a/", &mut t, 3).unwrap();
        assert!(
            resp.is_redirect(),
            "exhausted hop budget returns the last 3xx"
        );
        // 1 initial + 3 followed = 4 connects, then stop.
        assert_eq!(t.connects.len(), 4);
    }

    /// An https:// redirect is handed back (TLS not wired here) rather than dropped.
    #[test]
    fn fetch_follow_https_redirect_handed_back() {
        let r1 =
            b"HTTP/1.1 301 Moved\r\nLocation: https://secure.example/\r\nContent-Length: 0\r\n\r\n"
                .to_vec();
        let mut t = SeqTransport::new(vec![r1]);
        let resp = fetch("http://a/", &mut t).unwrap();
        assert_eq!(resp.status, 301);
        assert_eq!(resp.header("location"), Some("https://secure.example/"));
    }

    /// The login-flow case: a 302 sets a session cookie and redirects; the
    /// redirect hop must carry that cookie to the destination.
    #[test]
    fn fetch_follow_jar_carries_cookie_across_redirect() {
        let r1 = b"HTTP/1.1 302 Found\r\nSet-Cookie: session=abc; Path=/\r\nLocation: /home\r\nContent-Length: 0\r\n\r\n".to_vec();
        let r2 = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok".to_vec();
        let mut t = SeqTransport::new(vec![r1, r2]);
        let mut jar = crate::cookies::CookieJar::new();
        let resp = fetch_follow_jar("http://example.com/login", &mut t, 5, &mut jar, 0).unwrap();
        assert_eq!(resp.body, b"ok");
        // The jar captured the Set-Cookie.
        assert_eq!(
            jar.cookie_header("example.com", "/home", false, 0),
            Some("session=abc".into())
        );
        // First request had no cookie; the redirect hop carried it.
        let first = String::from_utf8_lossy(&t.sent[0]);
        assert!(
            !first.contains("Cookie:"),
            "first request must have no cookie yet"
        );
        let second = String::from_utf8_lossy(&t.sent[1]);
        assert!(
            second.contains("Cookie: session=abc\r\n"),
            "redirect hop must send the cookie; sent:\n{second}"
        );
    }

    /// resolve_redirect handles the three Location forms.
    #[test]
    fn resolve_redirect_forms() {
        assert_eq!(
            resolve_redirect("http://h/a/b", "http://x/y").unwrap(),
            "http://x/y"
        );
        assert_eq!(
            resolve_redirect("http://h:8080/a/b", "/c").unwrap(),
            "http://h:8080/c"
        );
        assert_eq!(
            resolve_redirect("http://h/a/b?q=1", "c").unwrap(),
            "http://h/a/c"
        );
    }

    #[test]
    fn resolve_redirect_dot_segments_and_protocol_relative() {
        // ".." climbs one directory; "." stays.
        assert_eq!(
            resolve_redirect("http://h/a/b/c", "../x").unwrap(),
            "http://h/a/x"
        );
        assert_eq!(
            resolve_redirect("http://h/a/b/c", "./x").unwrap(),
            "http://h/a/b/x"
        );
        // multiple "..", and ".." past root stays at root.
        assert_eq!(
            resolve_redirect("http://h/a/b/c", "../../x").unwrap(),
            "http://h/x"
        );
        assert_eq!(
            resolve_redirect("http://h/a", "../../x").unwrap(),
            "http://h/x"
        );
        // absolute path is normalized too; query is preserved.
        assert_eq!(
            resolve_redirect("http://h/a/b", "/c/../d").unwrap(),
            "http://h/d"
        );
        assert_eq!(
            resolve_redirect("http://h/a/b", "../c?q=1").unwrap(),
            "http://h/c?q=1"
        );
        // protocol-relative inherits scheme + switches host (was folded into base host).
        assert_eq!(
            resolve_redirect("http://h/a", "//other/x").unwrap(),
            "http://other/x"
        );
    }

    /// `Content-Encoding: br` (brotli) is decoded — against REAL `brotli -q 11`
    /// CLI output (quality 11 exercises the full RFC 7932 feature set incl. the
    /// static dictionary). Only runs with `--features brotli`.
    #[cfg(feature = "brotli")]
    #[test]
    fn brotli_content_encoding_decoded() {
        // `printf '<plaintext>' | brotli -q 11 -c` on the dev box.
        const REAL_BR: &[u8] = &[
            161, 224, 4, 0, 96, 164, 14, 107, 234, 188, 144, 229, 45, 213, 75, 193, 132, 106, 35,
            164, 55, 15, 219, 207, 27, 31, 16, 47, 58, 57, 96, 255, 119, 224, 182, 22, 165, 97, 26,
            180, 148, 2, 217, 33, 29, 111, 34, 201, 68, 164, 186, 20, 153, 103, 36, 230, 25, 146,
            19, 192, 216, 243, 86, 147, 1, 87, 4, 175, 220, 242, 214, 215, 21, 177, 168, 102, 213,
            139, 36, 216, 65, 136, 159, 208, 127, 40, 218, 48, 235, 25, 100, 120, 84, 68, 45, 217,
            190, 243, 230, 89, 24, 125,
        ];
        const PLAIN: &[u8] = b"<!doctype html><title>AthenaOS</title><h1>brotli works</h1><p>the quick brown fox jumps over the lazy dog repeatedly and at length to make it compressible</p>";
        let mut raw: Vec<u8> = Vec::new();
        raw.extend_from_slice(b"HTTP/1.1 200 OK\r\nContent-Encoding: br\r\n");
        raw.extend_from_slice(format!("Content-Length: {}\r\n\r\n", REAL_BR.len()).as_bytes());
        raw.extend_from_slice(REAL_BR);
        // The no_std brotli decoder stack-allocates its Huffman tables (~2 MiB),
        // so run on a thread with ample stack (the production caller must do the
        // same, or use the heap-allocator variant) — keeps the test self-contained.
        let body = std::thread::Builder::new()
            .stack_size(16 * 1024 * 1024)
            .spawn(move || {
                let mut t = MockTransport::new(raw);
                fetch("http://srv/", &mut t).unwrap().body
            })
            .unwrap()
            .join()
            .unwrap();
        assert_eq!(body, PLAIN, "brotli body must decode to the original");
    }

    /// We decode REAL external gzip output (produced by the system `gzip -9` CLI,
    /// i.e. exactly what a real HTTP server emits) — proving cross-implementation
    /// compatibility, independent of ath_deflate's own writer.
    #[test]
    fn gzip_real_server_output_decoded() {
        // `printf '<plaintext>' | gzip -9 -n` on the dev box.
        const REAL_GZIP: &[u8] = &[
            0x1f, 0x8b, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02, 0x03, 0xb3, 0x51, 0x4c, 0xc9,
            0x4f, 0x2e, 0xa9, 0x2c, 0x48, 0x55, 0xc8, 0x28, 0xc9, 0xcd, 0xb1, 0xb3, 0x29, 0xc9,
            0x2c, 0xc9, 0x49, 0xb5, 0x0b, 0x4a, 0x4c, 0x4d, 0xcd, 0xf3, 0x0f, 0xb6, 0xd1, 0x87,
            0x70, 0x6d, 0x32, 0x0c, 0xed, 0x32, 0x52, 0x73, 0x72, 0xf2, 0x15, 0xd2, 0x8a, 0xf2,
            0x73, 0x15, 0x12, 0x15, 0xd2, 0xab, 0x32, 0x0b, 0x0a, 0x52, 0x53, 0x14, 0x8a, 0x53,
            0x8b, 0xca, 0x52, 0x8b, 0x6c, 0xf4, 0x81, 0xf2, 0x00, 0x3b, 0x79, 0x1a, 0x8b, 0x49,
            0x00, 0x00, 0x00,
        ];
        const PLAIN: &[u8] =
            b"<!doctype html><title>AthenaOS</title><h1>hello from a gzipped server</h1>";
        let mut raw: Vec<u8> = Vec::new();
        raw.extend_from_slice(b"HTTP/1.1 200 OK\r\nContent-Encoding: gzip\r\n");
        raw.extend_from_slice(format!("Content-Length: {}\r\n\r\n", REAL_GZIP.len()).as_bytes());
        raw.extend_from_slice(REAL_GZIP);

        let mut t = MockTransport::new(raw);
        let resp = fetch("http://srv/", &mut t).unwrap();
        assert_eq!(resp.body, PLAIN);
    }

    /// `Content-Encoding: deflate` (zlib-wrapped) is decoded.
    #[test]
    fn deflate_content_encoding_decoded() {
        let plain = b"the quick brown fox jumps over the lazy dog, repeatedly and at length";
        let z = ath_deflate::zlib_compress(plain);
        let mut raw: Vec<u8> = Vec::new();
        raw.extend_from_slice(b"HTTP/1.1 200 OK\r\nContent-Encoding: deflate\r\n");
        raw.extend_from_slice(format!("Content-Length: {}\r\n\r\n", z.len()).as_bytes());
        raw.extend_from_slice(&z);
        let mut t = MockTransport::new(raw);
        let resp = fetch("http://h/", &mut t).unwrap();
        assert_eq!(resp.body, plain);
    }

    /// Case-insensitive coding token + `x-gzip` alias both work.
    #[test]
    fn content_encoding_case_insensitive_and_x_gzip() {
        let plain = b"alias works";
        let gz = ath_deflate::gzip_compress(plain);
        for enc in ["GZIP", "x-gzip", "  gzip  "] {
            let mut raw: Vec<u8> = Vec::new();
            raw.extend_from_slice(b"HTTP/1.1 200 OK\r\n");
            raw.extend_from_slice(format!("Content-Encoding: {enc}\r\n").as_bytes());
            raw.extend_from_slice(format!("Content-Length: {}\r\n\r\n", gz.len()).as_bytes());
            raw.extend_from_slice(&gz);
            let mut t = MockTransport::new(raw);
            let resp = fetch("http://h/", &mut t).unwrap();
            assert_eq!(resp.body, plain, "encoding token {enc:?} should decode");
        }
    }

    /// No `Content-Encoding` header → body passes through unchanged (identity).
    #[test]
    fn absent_content_encoding_passes_through() {
        let raw = b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello".to_vec();
        let mut t = MockTransport::new(raw);
        let resp = fetch("http://h/", &mut t).unwrap();
        assert_eq!(resp.body, b"hello");
        assert_eq!(
            decode_content_encoding(None, b"raw".to_vec(), &Limits::new()).unwrap(),
            b"raw"
        );
        assert_eq!(
            decode_content_encoding(Some("identity"), b"raw".to_vec(), &Limits::new()).unwrap(),
            b"raw"
        );
    }

    /// An unimplemented coding is a clean error, not a panic and not silently-
    /// wrong plaintext. `br` is "unsupported" only without the `brotli` feature;
    /// with it, garbage is a decode error (never Unsupported). `zstd` is always
    /// unsupported.
    #[test]
    fn unsupported_content_encoding_is_error() {
        let body = b"\x00\x01\x02 not-real-compressed".to_vec();
        let br = decode_content_encoding(Some("br"), body.clone(), &Limits::new());
        #[cfg(not(feature = "brotli"))]
        assert_eq!(
            br.unwrap_err(),
            Http1Error::UnsupportedContentEncoding("br".to_string())
        );
        #[cfg(feature = "brotli")]
        assert!(
            !matches!(br, Err(Http1Error::UnsupportedContentEncoding(_))),
            "with the brotli feature, br is handled (decoded or a decode error), not Unsupported"
        );
        let err2 = decode_content_encoding(Some("zstd"), body, &Limits::new()).unwrap_err();
        assert!(matches!(err2, Http1Error::UnsupportedContentEncoding(_)));
    }

    /// A corrupt gzip body maps to BadContentEncoding — never a panic.
    #[test]
    fn corrupt_gzip_body_is_error_not_panic() {
        let plain = b"will be corrupted";
        let mut gz = ath_deflate::gzip_compress(plain);
        // Smash a byte in the deflate payload (after the 10-byte gzip header).
        if gz.len() > 12 {
            gz[11] ^= 0xFF;
        }
        let err = decode_content_encoding(Some("gzip"), gz, &Limits::new()).unwrap_err();
        assert_eq!(err, Http1Error::BadContentEncoding);
    }

    /// Direct chunk decoder + hex parser degenerate cases.
    #[test]
    fn fuzz_chunked_decoder_degenerate() {
        let lim = Limits::new();
        let cases: &[&[u8]] = &[
            b"",
            b"\r\n",
            b"0\r\n\r\n",        // empty body, well-formed
            b"z\r\n",            // bad hex
            b";\r\n",            // empty hex prefix before ext
            b"1\r\nA",           // missing trailing CRLF
            b"1\r\nA\r\n",       // no terminating zero chunk
            b"00000000\r\n\r\n", // zero via many zeros
        ];
        for c in cases {
            let _ = decode_chunked(c, &lim);
        }
        // parse_hex edge cases.
        assert!(parse_hex(b"").is_err());
        assert!(parse_hex(b"g").is_err());
        assert!(parse_hex(b"ffffffffffffffffff").is_err()); // overflow
        assert_eq!(parse_hex(b"10").unwrap(), 16);
    }
}
