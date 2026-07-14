//! # RaeEncode — never-panic, `no_std` web codecs: base64, URL percent, hex.
//!
//! LEGACY_GAMING_CONCEPT.md §"Web apps via PWA support that actually feels native":
//! the web is the universal app runtime, and base64 / URL percent-encoding / hex
//! are the lingua franca of every layer of that stack — `data:` URIs in HTML and
//! CSS, HTTP `Basic` auth headers (`base64(user:pass)`), URL query-string and
//! form (`application/x-www-form-urlencoded`) values, and the hex digests that
//! show up in ETags, content hashes, and TLS fingerprints. One correct,
//! dependency-free, hostile-input codec core serves the browser, the PWA runtime,
//! the HTTP client, and anything else that touches the wire — so this crate is
//! foundational infrastructure, deliberately wired into no consumer this slice.
//!
//! ## Hostile-input posture (CLAUDE: decoders of untrusted bytes are an attack surface)
//! Every byte handed to a decoder is treated as hostile. There is **no
//! `unwrap`/`expect`/`panic`/raw-index-panic path** reachable from any decode
//! function: invalid alphabet characters, wrong-length input, bad padding,
//! truncated `%XX` escapes, non-hex digits, odd-length hex, and percent-decoded
//! bytes that are not valid UTF-8 all return [`DecodeError`]. Encoders are total
//! (every `&[u8]` / `&str` has an encoding). The host KAT suite at the bottom of
//! this file is the primary proof (`cargo test -p rae_encode`).
//!
//! ## What it is
//! - **Base64** (RFC 4648 §4): [`base64_encode`] / [`base64_decode`] over the
//!   standard alphabet `A-Za-z0-9+/` with `=` padding, plus the URL-safe
//!   alphabet (`-_`, §5) [`base64url_encode`] / [`base64url_decode`] where input
//!   padding is optional. Decoders skip ASCII whitespace (so wrapped MIME base64
//!   round-trips) and reject everything else.
//! - **URL percent-encoding** (RFC 3986): [`url_encode`] (path/segment-safe,
//!   space → `%20`) and [`url_encode_component`] (query/form-value-safe), both
//!   percent-encoding every byte outside the unreserved set `A-Za-z0-9-._~`.
//!   [`url_decode`] decodes `%XX`; [`form_url_decode`] additionally maps `+` →
//!   space (`application/x-www-form-urlencoded`). Decoders validate UTF-8.
//! - **Hex**: [`hex_encode`] (lowercase) / [`hex_decode`] (accepts upper or
//!   lower, rejects odd length and non-hex).

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

/// Why a decode failed. Returned by every `*_decode` function; encoders never
/// fail. Carries enough context to be actionable without leaking the input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError {
    /// A byte was not in the expected alphabet (base64 char, hex digit) or was an
    /// otherwise-illegal character (e.g. a stray `%` in a base64 stream).
    InvalidCharacter,
    /// The input length is impossible for the encoding: base64 not a multiple of
    /// 4 after stripping whitespace (standard, padded), or hex of odd length.
    InvalidLength,
    /// Base64 padding (`=`) is malformed: padding in the wrong place, too much
    /// padding, or required padding missing for the standard alphabet.
    InvalidPadding,
    /// A percent-escape was truncated (`%`, `%A`) — fewer than two hex digits
    /// follow the `%`.
    TruncatedEscape,
    /// The decoded bytes are not valid UTF-8, so they cannot form a `String`
    /// (URL decoders only; the byte-returning base64/hex decoders never raise it).
    InvalidUtf8,
}

// ---------------------------------------------------------------------------
// Base64 (RFC 4648)
// ---------------------------------------------------------------------------

const STD_ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
const URL_ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// Encode bytes as standard base64 (`A-Za-z0-9+/`, `=` padded) — RFC 4648 §4.
pub fn base64_encode(input: &[u8]) -> String {
    base64_encode_with(input, STD_ALPHABET)
}

/// Encode bytes as URL- and filename-safe base64 (`-_`, `=` padded) — RFC 4648 §5.
pub fn base64url_encode(input: &[u8]) -> String {
    base64_encode_with(input, URL_ALPHABET)
}

fn base64_encode_with(input: &[u8], alphabet: &[u8; 64]) -> String {
    // 4 output chars per 3 input bytes, rounded up.
    let mut out = String::with_capacity((input.len() + 2) / 3 * 4);
    let mut chunks = input.chunks_exact(3);
    for chunk in &mut chunks {
        let b = (chunk[0] as u32) << 16 | (chunk[1] as u32) << 8 | chunk[2] as u32;
        out.push(alphabet[(b >> 18 & 0x3f) as usize] as char);
        out.push(alphabet[(b >> 12 & 0x3f) as usize] as char);
        out.push(alphabet[(b >> 6 & 0x3f) as usize] as char);
        out.push(alphabet[(b & 0x3f) as usize] as char);
    }
    let rem = chunks.remainder();
    match rem.len() {
        1 => {
            let b = (rem[0] as u32) << 16;
            out.push(alphabet[(b >> 18 & 0x3f) as usize] as char);
            out.push(alphabet[(b >> 12 & 0x3f) as usize] as char);
            out.push('=');
            out.push('=');
        }
        2 => {
            let b = (rem[0] as u32) << 16 | (rem[1] as u32) << 8;
            out.push(alphabet[(b >> 18 & 0x3f) as usize] as char);
            out.push(alphabet[(b >> 12 & 0x3f) as usize] as char);
            out.push(alphabet[(b >> 6 & 0x3f) as usize] as char);
            out.push('=');
        }
        _ => {}
    }
    out
}

/// Map a base64 alphabet byte to its 6-bit value, or `None` if not in `alphabet`.
fn base64_value(c: u8, alphabet: &[u8; 64]) -> Option<u8> {
    let mut i = 0;
    while i < 64 {
        if alphabet[i] == c {
            return Some(i as u8);
        }
        i += 1;
    }
    None
}

/// Decode standard base64 (`A-Za-z0-9+/`, `=` padded) — RFC 4648 §4.
///
/// ASCII whitespace (space, tab, CR, LF) is skipped so wrapped MIME base64
/// round-trips. Length (after stripping whitespace) must be a multiple of 4,
/// padding must be well-formed, and every other byte must be in the alphabet —
/// otherwise [`DecodeError`].
pub fn base64_decode(input: &str) -> Result<Vec<u8>, DecodeError> {
    base64_decode_with(input, STD_ALPHABET, true)
}

/// Decode URL-safe base64 (`-_`) — RFC 4648 §5. Padding is optional: a stream
/// whose length is not a multiple of 4 is accepted and treated as if the missing
/// `=` were present. ASCII whitespace is skipped.
pub fn base64url_decode(input: &str) -> Result<Vec<u8>, DecodeError> {
    base64_decode_with(input, URL_ALPHABET, false)
}

fn base64_decode_with(
    input: &str,
    alphabet: &[u8; 64],
    require_padding: bool,
) -> Result<Vec<u8>, DecodeError> {
    // Collect significant (non-whitespace) characters, tracking padding.
    let mut symbols: Vec<u8> = Vec::with_capacity(input.len());
    let mut pad = 0usize;
    for &c in input.as_bytes() {
        match c {
            b' ' | b'\t' | b'\r' | b'\n' => continue,
            b'=' => {
                pad += 1;
            }
            _ => {
                // A data char after padding has begun is malformed.
                if pad != 0 {
                    return Err(DecodeError::InvalidPadding);
                }
                match base64_value(c, alphabet) {
                    Some(v) => symbols.push(v),
                    None => return Err(DecodeError::InvalidCharacter),
                }
            }
        }
    }

    // Padding may be at most 2 chars and only legal when it completes a group.
    if pad > 2 {
        return Err(DecodeError::InvalidPadding);
    }

    let data_len = symbols.len();
    let total = data_len + pad;

    if require_padding {
        // Standard alphabet: the padded stream must be a whole number of groups.
        if total % 4 != 0 {
            return Err(DecodeError::InvalidLength);
        }
    } else {
        // URL-safe: padding optional, but if present it must still land on a
        // group boundary, and an inner data length of 1 mod 4 is impossible.
        if pad != 0 && total % 4 != 0 {
            return Err(DecodeError::InvalidLength);
        }
    }

    // The number of leftover data symbols determines how many bytes the final
    // partial group yields. A leftover of exactly 1 symbol is never valid (it
    // encodes < 8 bits). When padding is required the leftover must match it.
    let leftover = data_len % 4;
    if leftover == 1 {
        return Err(DecodeError::InvalidPadding);
    }
    if require_padding && pad != 0 {
        let expected_pad = (4 - leftover) % 4;
        if pad != expected_pad {
            return Err(DecodeError::InvalidPadding);
        }
    }

    let mut out: Vec<u8> = Vec::with_capacity(data_len / 4 * 3 + 2);
    let mut iter = symbols.chunks_exact(4);
    for group in &mut iter {
        let n = (group[0] as u32) << 18
            | (group[1] as u32) << 12
            | (group[2] as u32) << 6
            | group[3] as u32;
        out.push((n >> 16) as u8);
        out.push((n >> 8) as u8);
        out.push(n as u8);
    }
    let tail = iter.remainder();
    match tail.len() {
        2 => {
            let n = (tail[0] as u32) << 18 | (tail[1] as u32) << 12;
            out.push((n >> 16) as u8);
        }
        3 => {
            let n = (tail[0] as u32) << 18 | (tail[1] as u32) << 12 | (tail[2] as u32) << 6;
            out.push((n >> 16) as u8);
            out.push((n >> 8) as u8);
        }
        _ => {}
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// URL percent-encoding (RFC 3986)
// ---------------------------------------------------------------------------

/// `true` if `b` is an RFC 3986 *unreserved* character (`A-Za-z0-9-._~`) — the
/// set that is never percent-encoded.
fn is_unreserved(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~')
}

fn percent_encode_with(input: &str, keep: fn(u8) -> bool) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(input.len());
    for &b in input.as_bytes() {
        if keep(b) {
            out.push(b as char);
        } else {
            out.push('%');
            out.push(HEX[(b >> 4) as usize] as char);
            out.push(HEX[(b & 0x0f) as usize] as char);
        }
    }
    out
}

/// Percent-encode a string for use as a URL path segment: everything outside the
/// unreserved set `A-Za-z0-9-._~` becomes `%XX` (so a space becomes `%20`).
///
/// Use this where the value may legitimately contain reserved delimiters you want
/// preserved per segment; for a query-string *value* prefer
/// [`url_encode_component`] (identical encoding here, but named for intent and
/// the natural pair to [`form_url_decode`]).
pub fn url_encode(input: &str) -> String {
    percent_encode_with(input, is_unreserved)
}

/// Percent-encode a string for use as a single query-string or form value: every
/// byte outside the unreserved set `A-Za-z0-9-._~` is `%XX`-escaped, so reserved
/// characters (`&`, `=`, `/`, `?`, …) are all encoded and cannot be misread as
/// delimiters. A space becomes `%20` (callers wanting `+` should post-process).
pub fn url_encode_component(input: &str) -> String {
    percent_encode_with(input, is_unreserved)
}

/// One hex ASCII digit → its 4-bit value, or `None` if not `[0-9A-Fa-f]`.
fn hex_nibble(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

fn percent_decode_with(input: &str, plus_is_space: bool) -> Result<String, DecodeError> {
    let bytes = input.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        match b {
            b'%' => {
                // Need exactly two hex digits after the '%'.
                if i + 2 >= bytes.len() {
                    return Err(DecodeError::TruncatedEscape);
                }
                let hi = hex_nibble(bytes[i + 1]).ok_or(DecodeError::InvalidCharacter)?;
                let lo = hex_nibble(bytes[i + 2]).ok_or(DecodeError::InvalidCharacter)?;
                out.push(hi << 4 | lo);
                i += 3;
            }
            b'+' if plus_is_space => {
                out.push(b' ');
                i += 1;
            }
            _ => {
                out.push(b);
                i += 1;
            }
        }
    }
    match String::from_utf8(out) {
        Ok(s) => Ok(s),
        Err(_) => Err(DecodeError::InvalidUtf8),
    }
}

/// Decode a percent-encoded URL string (`%XX` → byte). A literal `+` is left as
/// `+` (use [`form_url_decode`] for `application/x-www-form-urlencoded` where `+`
/// means space). A truncated escape (`%`, `%A`), a non-hex digit after `%`, or a
/// percent-decoded byte sequence that is not valid UTF-8 returns [`DecodeError`].
pub fn url_decode(input: &str) -> Result<String, DecodeError> {
    percent_decode_with(input, false)
}

/// Decode an `application/x-www-form-urlencoded` value: like [`url_decode`] but
/// `+` maps to a space. Same hostile-input guarantees.
pub fn form_url_decode(input: &str) -> Result<String, DecodeError> {
    percent_decode_with(input, true)
}

// ---------------------------------------------------------------------------
// Hex
// ---------------------------------------------------------------------------

/// Encode bytes as lowercase hex (`[0xde,0xad]` → `"dead"`).
pub fn hex_encode(input: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(input.len() * 2);
    for &b in input {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

/// Decode a hex string (upper or lower case) to bytes. An odd-length string
/// returns [`DecodeError::InvalidLength`]; a non-hex character returns
/// [`DecodeError::InvalidCharacter`]. Never panics.
pub fn hex_decode(input: &str) -> Result<Vec<u8>, DecodeError> {
    let bytes = input.as_bytes();
    if bytes.len() % 2 != 0 {
        return Err(DecodeError::InvalidLength);
    }
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len() / 2);
    let mut i = 0;
    while i < bytes.len() {
        let hi = hex_nibble(bytes[i]).ok_or(DecodeError::InvalidCharacter)?;
        let lo = hex_nibble(bytes[i + 1]).ok_or(DecodeError::InvalidCharacter)?;
        out.push(hi << 4 | lo);
        i += 2;
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Host KATs — the FAIL-able proof (`cargo test -p rae_encode`).
// Under `cfg(test)` the crate builds as `std`, so the prelude (Vec, String,
// assert!) is in scope; we still pull `alloc::vec`/`ToString` explicitly to
// mirror the no_std lib and keep `use std::` out of the test module (R7 gate).
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;
    use alloc::vec;

    // -- Base64 RFC 4648 §10 test vectors --------------------------------------

    /// The seven canonical RFC 4648 vectors, exact, round-tripped both ways.
    /// FAIL-ability: if the encoder's 1-byte/2-byte padding logic were broken
    /// (e.g. dropping the trailing `=`), the `"f" -> "Zg=="` and `"fo" -> "Zm8="`
    /// assert_eq lines below flip to failure.
    #[test]
    fn base64_rfc4648_vectors() {
        let cases: &[(&str, &str)] = &[
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ];
        for (plain, encoded) in cases {
            let got = base64_encode(plain.as_bytes());
            assert_eq!(&got, encoded, "encode {plain:?}");
            // Guard against a degenerate encoder that returns the input/empty.
            if !plain.is_empty() {
                assert_ne!(got, plain.to_string(), "encoder must transform {plain:?}");
            }
            let back = base64_decode(encoded).expect("decode must succeed");
            assert_eq!(back, plain.as_bytes(), "round-trip {encoded:?}");
        }
    }

    /// URL-safe alphabet maps the `+` and `/` producing bytes to `-` and `_`.
    /// FAIL-ability: swap URL_ALPHABET back to `+/` and these assert_eq lines
    /// fail (the standard encoding of the same bytes contains `+` and `/`).
    #[test]
    fn base64url_alphabet_uses_dash_underscore() {
        // 0xFB,0xFF -> standard "+/8=" style high bits; pick bytes that exercise
        // both the 62nd ('+'/'-') and 63rd ('/'/'_') symbols.
        let data = [0xfbu8, 0xff, 0xbf];
        let std = base64_encode(&data);
        let url = base64url_encode(&data);
        assert!(
            std.contains('+') || std.contains('/'),
            "std should use +//: {std}"
        );
        assert!(
            !url.contains('+') && !url.contains('/'),
            "url-safe must not: {url}"
        );
        // -_ are the only chars that differ from the standard alphabet.
        assert_eq!(url, std.replace('+', "-").replace('/', "_"));
        // Round-trip both alphabets.
        assert_eq!(base64_decode(&std).unwrap(), data);
        assert_eq!(base64url_decode(&url).unwrap(), data);
    }

    /// URL-safe decode tolerates missing padding.
    #[test]
    fn base64url_optional_padding() {
        // base64url_encode("foob") = "Zm9vYg==" -> strip padding -> "Zm9vYg"
        let padded = base64url_encode(b"foob");
        let stripped = padded.trim_end_matches('=').to_string();
        assert_ne!(padded, stripped);
        assert_eq!(base64url_decode(&stripped).unwrap(), b"foob");
        assert_eq!(base64url_decode(&padded).unwrap(), b"foob");
    }

    /// MIME-style wrapped base64 (embedded whitespace) decodes.
    #[test]
    fn base64_skips_whitespace() {
        assert_eq!(base64_decode("Zm9v\r\nYmFy").unwrap(), b"foobar");
        assert_eq!(base64_decode("Zm 9v Ym Fy").unwrap(), b"foobar");
    }

    // -- URL percent-encoding --------------------------------------------------

    /// `url_encode("a b&c=d")` percent-encodes the space and the reserved
    /// delimiters. FAIL-ability: if `is_unreserved` wrongly admitted `&`/`=`,
    /// the expected string below (with `%26`/`%3D`) would not match.
    #[test]
    fn url_encode_reserved_and_space() {
        assert_eq!(url_encode("a b&c=d"), "a%20b%26c%3Dd");
        assert_eq!(url_encode_component("a b&c=d"), "a%20b%26c%3Dd");
        // Unreserved set passes through untouched.
        assert_eq!(url_encode("Aa0-._~"), "Aa0-._~");
    }

    #[test]
    fn url_decode_round_trip_and_form_plus() {
        let s = "name=Rae & Co/100% ünïcødé";
        let enc = url_encode_component(s);
        assert_eq!(url_decode(&enc).unwrap(), s);
        // Form variant: '+' is a space; plain url_decode keeps it literal.
        assert_eq!(form_url_decode("a+b%20c").unwrap(), "a b c");
        assert_eq!(url_decode("a+b%20c").unwrap(), "a+b c");
    }

    /// Truncated / malformed percent escapes are errors, never panics.
    /// FAIL-ability: if the `i + 2 >= len` bound check were removed, `"%A"`
    /// would index out of bounds and panic instead of returning Err.
    #[test]
    fn url_decode_bad_escapes_are_err() {
        assert_eq!(url_decode("%"), Err(DecodeError::TruncatedEscape));
        assert_eq!(url_decode("%A"), Err(DecodeError::TruncatedEscape));
        assert_eq!(url_decode("%G0"), Err(DecodeError::InvalidCharacter));
        assert_eq!(url_decode("%0G"), Err(DecodeError::InvalidCharacter));
        // %FF alone is not valid UTF-8.
        assert_eq!(url_decode("%FF"), Err(DecodeError::InvalidUtf8));
    }

    // -- Hex -------------------------------------------------------------------

    /// FAIL-ability: if `hex_encode` emitted uppercase, this lowercase-literal
    /// assert_eq fails; if the nibble order were swapped, "deadbeef" would read
    /// "edabebfe".
    #[test]
    fn hex_round_trip() {
        assert_eq!(hex_encode(&[0xde, 0xad, 0xbe, 0xef]), "deadbeef");
        assert_eq!(
            hex_decode("deadbeef").unwrap(),
            vec![0xde, 0xad, 0xbe, 0xef]
        );
        // Decode tolerates uppercase.
        assert_eq!(
            hex_decode("DEADBEEF").unwrap(),
            vec![0xde, 0xad, 0xbe, 0xef]
        );
        assert_eq!(hex_encode(&[]), "");
        assert_eq!(hex_decode("").unwrap(), Vec::<u8>::new());
    }

    #[test]
    fn hex_bad_input_is_err() {
        assert_eq!(hex_decode("abc"), Err(DecodeError::InvalidLength));
        assert_eq!(hex_decode("xy"), Err(DecodeError::InvalidCharacter));
        assert_eq!(hex_decode("0g"), Err(DecodeError::InvalidCharacter));
    }

    // -- Malformed battery: every case Err, ZERO panics ------------------------

    /// A spread of hostile inputs across all three codecs. Each must return Err
    /// (not panic, not silently accept). FAIL-ability: if base64's padding/length
    /// validation were removed, several of these would decode to garbage Ok(...)
    /// and the asserts below would fail.
    #[test]
    fn malformed_battery_all_err_no_panic() {
        // Bad base64 alphabet char.
        assert!(base64_decode("Zm9v!Fy").is_err());
        // Bad base64 padding (data after '=').
        assert_eq!(
            base64_decode("Zm=v").err(),
            Some(DecodeError::InvalidPadding)
        );
        // Too much padding.
        assert_eq!(
            base64_decode("Zg===").err(),
            Some(DecodeError::InvalidPadding)
        );
        // Wrong length (3 mod 4 with no padding, standard requires padding).
        assert_eq!(base64_decode("Zm9").err(), Some(DecodeError::InvalidLength));
        // Single leftover symbol is never valid.
        assert_eq!(base64_decode("Z").err(), Some(DecodeError::InvalidLength));
        // Truncated percent escape.
        assert_eq!(url_decode("abc%").err(), Some(DecodeError::TruncatedEscape));
        // Odd-length hex.
        assert_eq!(hex_decode("abcde").err(), Some(DecodeError::InvalidLength));
        // Embedded NUL: passes through encoders fine, but hex_decode of a NUL
        // char is non-hex.
        assert!(hex_decode("\0\0").is_err());
        // NUL round-trips through base64 and url encoders without panic.
        assert_eq!(base64_decode(&base64_encode(b"\0\0\0")).unwrap(), b"\0\0\0");
        assert_eq!(url_decode(&url_encode("\0a\0")).unwrap(), "\0a\0");
        assert_eq!(hex_decode(&hex_encode(b"\0\0")).unwrap(), b"\0\0");
    }

    /// Exhaustive single-byte round-trip across all three byte codecs — proves
    /// no input byte value (0..=255) trips a panic or a lossy path.
    #[test]
    fn all_byte_values_round_trip() {
        let all: Vec<u8> = (0u16..=255).map(|b| b as u8).collect();
        assert_eq!(base64_decode(&base64_encode(&all)).unwrap(), all);
        assert_eq!(base64url_decode(&base64url_encode(&all)).unwrap(), all);
        assert_eq!(hex_decode(&hex_encode(&all)).unwrap(), all);
    }
}
