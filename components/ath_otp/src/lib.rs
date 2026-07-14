//! # RaeOTP — HOTP (RFC 4226) + TOTP (RFC 6238) one-time passwords.
//!
//! LEGACY_GAMING_CONCEPT.md §Compatibility Strategy ("how to actually win", criterion
//! #5 — *import & keep my stuff*) + the daily-driver table stakes: a person
//! switching to AthenaOS arrives with a phone full of authenticator-app secrets
//! (Google Authenticator, Authy, 1Password, Microsoft Authenticator). Those are
//! all the **same** open standard — counter- and time-based one-time passwords —
//! and the format is frozen by interoperability. To be a credible daily driver
//! AthenaOS must produce the *exact same six digits* their old phone did. This
//! crate is that engine; it pairs with `athid`'s passkeys to cover both halves of
//! modern 2FA (something-you-have hardware/platform keys + software OTP tokens).
//!
//! ## What this crate is (and is NOT)
//! This is the **algorithm** layer only: it turns a shared secret + a counter (or
//! the wall clock) into a code, decodes the base32 secret format apps use, and
//! parses `otpauth://` provisioning URIs. It does **not** store secrets, drive a
//! clock, or rate-limit verification — those belong to the caller (`athid` / a
//! vault UI). All cryptography is delegated to [`ath_crypto`] (HMAC-SHA-1 /
//! HMAC-SHA-256); no hash is reimplemented here.
//!
//! ## The construction
//! - **HOTP** ([`hotp`], RFC 4226 §5.3): `HMAC-SHA-1(secret, counter_be_8)`, then
//!   *dynamic truncation* — the low nibble of the last MAC byte is an offset; the
//!   4 bytes at that offset, masked to 31 bits, taken `mod 10^digits`.
//! - **TOTP** ([`totp`], RFC 6238 §4): HOTP with `counter = (unix_time - t0) /
//!   step`, optionally over HMAC-SHA-256 (the RFC permits SHA-1/256/512; we
//!   implement SHA-1 (default) and SHA-256 — see [`Algorithm`]).
//! - **Verify** ([`totp_verify`]): checks the current step ±`window` steps for
//!   clock-skew tolerance, with a length-checked digit compare.
//!
//! ## Safety posture
//! `#![forbid(unsafe_code)]`, `no_std` + `alloc`, **never panics on any input**:
//! a bad base32 string returns `None`, an out-of-range `digits` is clamped to the
//! valid 1..=10 range, and an enormous counter or time is arithmetic-safe. The
//! FAIL-able proof is the published RFC vectors in the `#[cfg(test)]` block:
//! HOTP RFC 4226 Appendix D (all ten codes), TOTP RFC 6238 Appendix B (SHA-1 and
//! SHA-256), plus base32 / verify / never-panic cases. Run `cargo test -p ath_otp`.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

use ath_crypto::hmac::{hmac_sha1, hmac_sha256};

/// Default number of digits in a generated code (RFC 4226 / RFC 6238 default).
pub const DEFAULT_DIGITS: u8 = 6;
/// Default TOTP time step in seconds (RFC 6238 §4 recommended `X`).
pub const DEFAULT_STEP_SECS: u64 = 30;
/// Default TOTP epoch `T0` in seconds (RFC 6238 default — the Unix epoch).
pub const DEFAULT_T0: u64 = 0;
/// Smallest valid digit count.
pub const MIN_DIGITS: u8 = 1;
/// Largest digit count that fits a 31-bit truncated value (10^10 > 2^31, but the
/// RFC caps practical use here; 10 digits is the documented maximum).
pub const MAX_DIGITS: u8 = 10;

/// The HMAC hash underlying a TOTP. RFC 6238 permits SHA-1, SHA-256, SHA-512;
/// we implement the two that authenticator apps actually emit. SHA-1 is the
/// universal default; SHA-256 appears in `otpauth://...&algorithm=SHA256` URIs.
///
/// SHA-512 is **deferred** (documented): no mainstream authenticator app uses it
/// and [`ath_crypto`] does not yet expose HMAC-SHA-512. Adding it later is a
/// one-arm extension here plus the primitive in `ath_crypto`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Algorithm {
    /// HMAC-SHA-1 — the universal authenticator-app default.
    Sha1,
    /// HMAC-SHA-256 — `otpauth://...?algorithm=SHA256`.
    Sha256,
}

impl Default for Algorithm {
    fn default() -> Self {
        Algorithm::Sha1
    }
}

/// Clamp a caller-supplied digit count into the valid `MIN_DIGITS..=MAX_DIGITS`
/// range so a bad value can never panic or overflow `10^digits`.
fn clamp_digits(digits: u8) -> u8 {
    digits.clamp(MIN_DIGITS, MAX_DIGITS)
}

/// `10^n` for `n` in `0..=10`, computed with `u64` (10^10 < 2^34, fits). Never
/// overflows because `n` is pre-clamped by [`clamp_digits`].
fn pow10(n: u8) -> u64 {
    let mut v = 1u64;
    for _ in 0..n {
        v = v.wrapping_mul(10);
    }
    v
}

/// RFC 4226 §5.3 dynamic truncation of a finalized HMAC into a `digits`-wide code.
/// Shared by HOTP and TOTP (TOTP is "HOTP with a time-derived counter").
fn truncate(mac: &[u8], digits: u8) -> u32 {
    // The offset is the low nibble of the LAST MAC byte. SHA-1 MAC is 20 bytes,
    // SHA-256 is 32 — both leave room for the 4-byte read at offset 0..=15.
    let last = mac.len() - 1;
    let offset = (mac[last] & 0x0f) as usize;
    let bin = ((mac[offset] as u32 & 0x7f) << 24)
        | ((mac[offset + 1] as u32) << 16)
        | ((mac[offset + 2] as u32) << 8)
        | (mac[offset + 3] as u32);
    (bin as u64 % pow10(digits)) as u32
}

/// HOTP (RFC 4226 §5.3): counter-based one-time password over HMAC-SHA-1.
/// Returns the numeric code in `0..10^digits` (`digits` clamped to 1..=10).
///
/// The caller is responsible for the moving counter (HOTP increments per use).
pub fn hotp(secret: &[u8], counter: u64, digits: u8) -> u32 {
    let digits = clamp_digits(digits);
    let mac = hmac_sha1(secret, &counter.to_be_bytes());
    truncate(&mac, digits)
}

/// HOTP rendered as a fixed-width, zero-padded decimal string (the form shown to
/// a user — e.g. `"006789"`). `digits` clamped to 1..=10.
pub fn hotp_string(secret: &[u8], counter: u64, digits: u8) -> String {
    let digits = clamp_digits(digits);
    format_code(hotp(secret, counter, digits), digits)
}

/// TOTP (RFC 6238 §4): time-based one-time password.
///
/// `counter = (unix_time - t0) / step_secs` (saturating: a `unix_time` before
/// `t0` yields counter 0 rather than underflowing), then HOTP over the chosen
/// [`Algorithm`]. `step_secs == 0` is treated as the [`DEFAULT_STEP_SECS`] to
/// avoid a divide-by-zero. Returns the numeric code.
pub fn totp(
    secret: &[u8],
    unix_time: u64,
    step_secs: u64,
    t0: u64,
    digits: u8,
    algo: Algorithm,
) -> u32 {
    let digits = clamp_digits(digits);
    let step = if step_secs == 0 {
        DEFAULT_STEP_SECS
    } else {
        step_secs
    };
    let counter = unix_time.saturating_sub(t0) / step;
    let msg = counter.to_be_bytes();
    let mac: Vec<u8> = match algo {
        Algorithm::Sha1 => hmac_sha1(secret, &msg).to_vec(),
        Algorithm::Sha256 => hmac_sha256(secret, &msg).to_vec(),
    };
    truncate(&mac, digits)
}

/// TOTP rendered as a zero-padded decimal string.
#[allow(clippy::too_many_arguments)]
pub fn totp_string(
    secret: &[u8],
    unix_time: u64,
    step_secs: u64,
    t0: u64,
    digits: u8,
    algo: Algorithm,
) -> String {
    let digits = clamp_digits(digits);
    format_code(totp(secret, unix_time, step_secs, t0, digits, algo), digits)
}

/// Convenience: a 6-digit, 30-second, SHA-1, `T0 = 0` TOTP — the configuration
/// every default authenticator-app entry uses. Returns the zero-padded string a
/// UI would display.
pub fn totp_now(secret: &[u8], unix_time: u64) -> String {
    totp_string(
        secret,
        unix_time,
        DEFAULT_STEP_SECS,
        DEFAULT_T0,
        DEFAULT_DIGITS,
        Algorithm::Sha1,
    )
}

/// Verify a user-entered TOTP code against the current time, tolerating up to
/// `window` steps of clock skew on either side (RFC 6238 §5.2 — a typical value
/// is 1, allowing the previous/next 30-second step). Uses the default 6-digit,
/// 30-second, SHA-1 parameters.
///
/// The comparison is length-then-content equal on the rendered digit string. It
/// is **not** a hardened constant-time MAC compare — OTP codes are short and
/// public-format, and brute-force resistance must come from rate-limiting at the
/// calling layer (a vault should lock after a few wrong attempts). Documented so
/// no caller mistakes this for the whole defense.
pub fn totp_verify(secret: &[u8], unix_time: u64, code: &str, window: u64) -> bool {
    totp_verify_full(
        secret,
        unix_time,
        code,
        window,
        DEFAULT_STEP_SECS,
        DEFAULT_T0,
        DEFAULT_DIGITS,
        Algorithm::Sha1,
    )
}

/// Verify with full control over step / epoch / digits / algorithm (the form
/// used when a stored entry came from an `otpauth://` URI with non-default
/// parameters). See [`totp_verify`] for the skew-window and compare semantics.
#[allow(clippy::too_many_arguments)]
pub fn totp_verify_full(
    secret: &[u8],
    unix_time: u64,
    code: &str,
    window: u64,
    step_secs: u64,
    t0: u64,
    digits: u8,
    algo: Algorithm,
) -> bool {
    let digits = clamp_digits(digits);
    let code = code.trim();
    // A code of the wrong length can never match — reject early (also bounds the
    // per-candidate compare below).
    if code.len() != digits as usize {
        return false;
    }
    let step = if step_secs == 0 {
        DEFAULT_STEP_SECS
    } else {
        step_secs
    };
    let base = unix_time.saturating_sub(t0) / step;
    // Scan base-window ..= base+window. We compare every candidate (no early
    // return on first mismatch) so the work does not depend on which step
    // matched — a small skew-tolerant equivalent of a constant-time check.
    let mut matched = false;
    let lo = base.saturating_sub(window);
    let hi = base.saturating_add(window);
    let mut c = lo;
    loop {
        let candidate = format_code(hotp_with_algo(secret, c, digits, algo), digits);
        matched |= ct_eq_str(&candidate, code);
        if c == hi {
            break;
        }
        c += 1;
    }
    matched
}

/// HOTP over a chosen algorithm at an explicit counter — the shared core that
/// [`totp`] and [`totp_verify_full`] both step through.
fn hotp_with_algo(secret: &[u8], counter: u64, digits: u8, algo: Algorithm) -> u32 {
    let msg = counter.to_be_bytes();
    let mac: Vec<u8> = match algo {
        Algorithm::Sha1 => hmac_sha1(secret, &msg).to_vec(),
        Algorithm::Sha256 => hmac_sha256(secret, &msg).to_vec(),
    };
    truncate(&mac, digits)
}

/// Zero-pad a numeric code to `digits` columns (`format_code(42, 6) == "000042"`).
/// Builds the fixed-width decimal directly (no `ToString`, which is not in scope
/// under `no_std`): emit the least-significant `digits` digits, MSB first.
fn format_code(code: u32, digits: u8) -> String {
    let width = digits as usize;
    let mut buf = [0u8; MAX_DIGITS as usize];
    let mut v = code as u64;
    for i in (0..width).rev() {
        buf[i] = b'0' + (v % 10) as u8;
        v /= 10;
    }
    // SAFETY-FREE: buf[..width] is ASCII digits by construction.
    let mut s = String::with_capacity(width);
    for &b in &buf[..width] {
        s.push(b as char);
    }
    s
}

/// Length-checked, fixed-time-ish string equality over equal-length ASCII digit
/// strings. Compares every byte (no short-circuit) to avoid leaking the matched
/// prefix length through timing. Unequal lengths return `false` immediately
/// (length is not secret — it is the public digit count).
fn ct_eq_str(a: &str, b: &str) -> bool {
    let ab = a.as_bytes();
    let bb = b.as_bytes();
    if ab.len() != bb.len() {
        return false;
    }
    let mut diff = 0u8;
    for i in 0..ab.len() {
        diff |= ab[i] ^ bb[i];
    }
    diff == 0
}

// ---------------------------------------------------------------------------
// Base32 (RFC 4648) — the format authenticator apps use for the shared secret.
// ---------------------------------------------------------------------------

/// Decode an RFC 4648 base32 string into the raw secret bytes.
///
/// Tolerant of the way secrets appear in the wild: case-insensitive, ASCII spaces
/// and `-` separators are skipped (apps often group the secret into chunks), and
/// trailing `=` padding is optional. Returns `None` (never panics) for any
/// character outside the base32 alphabet or a bit-length that cannot form whole
/// bytes (a malformed secret).
pub fn decode_base32(input: &str) -> Option<Vec<u8>> {
    let mut bits: u32 = 0;
    let mut bit_count: u32 = 0;
    let mut out = Vec::new();
    let mut seen_pad = false;

    for ch in input.chars() {
        match ch {
            // Separators commonly used to group a displayed secret.
            ' ' | '-' | '\t' | '\n' | '\r' => continue,
            '=' => {
                seen_pad = true;
                continue;
            }
            _ => {}
        }
        // Any real data character after padding has begun is malformed.
        if seen_pad {
            return None;
        }
        let val = base32_value(ch)?;
        bits = (bits << 5) | val as u32;
        bit_count += 5;
        if bit_count >= 8 {
            bit_count -= 8;
            out.push((bits >> bit_count) as u8);
        }
    }

    // Leftover bits must be only padding zeros; any set bit means a truncated /
    // malformed secret. (Whole-byte boundaries leave 0 leftover bits.)
    if bit_count > 0 {
        let mask = (1u32 << bit_count) - 1;
        if bits & mask != 0 {
            return None;
        }
    }
    Some(out)
}

/// Map one RFC 4648 base32 character (A-Z, 2-7, case-insensitive) to its 5-bit
/// value, or `None` if it is not in the alphabet.
fn base32_value(ch: char) -> Option<u8> {
    match ch {
        'A'..='Z' => Some(ch as u8 - b'A'),
        'a'..='z' => Some(ch as u8 - b'a'),
        '2'..='7' => Some(ch as u8 - b'2' + 26),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// otpauth:// provisioning URI (Key Uri Format) — real authenticator import.
// ---------------------------------------------------------------------------

/// A parsed `otpauth://totp/...` provisioning URI — the QR-code payload every
/// authenticator app emits. Lets AthenaOS import an existing 2FA entry verbatim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OtpAuth {
    /// `"totp"` or `"hotp"` (the URI authority).
    pub kind: OtpKind,
    /// Human label, typically `Issuer:account` (URI path, percent-decoded).
    pub label: String,
    /// The decoded raw secret bytes (`secret=` base32 parameter).
    pub secret: Vec<u8>,
    /// `issuer=` parameter, if present.
    pub issuer: Option<String>,
    /// `algorithm=` parameter (defaults to SHA-1).
    pub algorithm: Algorithm,
    /// `digits=` parameter (defaults to 6, clamped to 1..=10).
    pub digits: u8,
    /// TOTP `period=` in seconds (defaults to 30). Ignored for HOTP.
    pub period: u64,
    /// HOTP `counter=` initial value (defaults to 0). Ignored for TOTP.
    pub counter: u64,
}

/// Which OTP family an [`OtpAuth`] URI describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OtpKind {
    /// Time-based (`otpauth://totp/...`).
    Totp,
    /// Counter-based (`otpauth://hotp/...`).
    Hotp,
}

impl OtpAuth {
    /// Parse an `otpauth://{totp,hotp}/LABEL?secret=...&...` URI.
    ///
    /// Returns `None` (never panics) if the scheme/authority is wrong, the
    /// mandatory `secret=` is missing/undecodable, or a numeric parameter is not
    /// a number. Unknown parameters are ignored. Percent-decoding is applied to
    /// the label and string parameter values.
    pub fn parse(uri: &str) -> Option<OtpAuth> {
        let rest = uri.strip_prefix("otpauth://")?;
        let (authority, after) = match rest.split_once('/') {
            Some(parts) => parts,
            // No path separator — allow "otpauth://totp?..." with empty label.
            None => (rest, ""),
        };
        let kind = match authority.to_ascii_lowercase().as_str() {
            "totp" => OtpKind::Totp,
            "hotp" => OtpKind::Hotp,
            _ => return None,
        };
        let (label_raw, query) = match after.split_once('?') {
            Some((l, q)) => (l, q),
            None => (after, ""),
        };
        let label = percent_decode(label_raw);

        let mut secret: Option<Vec<u8>> = None;
        let mut issuer: Option<String> = None;
        let mut algorithm = Algorithm::Sha1;
        let mut digits = DEFAULT_DIGITS;
        let mut period = DEFAULT_STEP_SECS;
        let mut counter = 0u64;

        for pair in query.split('&') {
            if pair.is_empty() {
                continue;
            }
            let (key, value) = match pair.split_once('=') {
                Some(kv) => kv,
                None => continue,
            };
            let value = percent_decode(value);
            match key.to_ascii_lowercase().as_str() {
                "secret" => secret = decode_base32(&value),
                "issuer" => issuer = Some(value),
                "algorithm" => {
                    algorithm = match value.to_ascii_uppercase().as_str() {
                        "SHA1" => Algorithm::Sha1,
                        "SHA256" => Algorithm::Sha256,
                        // SHA512 (deferred) or unknown -> fall back to the SHA-1
                        // default rather than fail the whole import.
                        _ => Algorithm::Sha1,
                    };
                }
                "digits" => {
                    digits = clamp_digits(value.parse::<u8>().ok()?);
                }
                "period" => {
                    period = value.parse::<u64>().ok()?;
                    if period == 0 {
                        period = DEFAULT_STEP_SECS;
                    }
                }
                "counter" => {
                    counter = value.parse::<u64>().ok()?;
                }
                _ => {}
            }
        }

        Some(OtpAuth {
            kind,
            label,
            secret: secret?,
            issuer,
            algorithm,
            digits,
            period,
            counter,
        })
    }

    /// Produce the current code for this imported entry: a TOTP at `unix_time`
    /// for a TOTP URI, or HOTP at the URI's stored counter for an HOTP URI.
    pub fn code_at(&self, unix_time: u64) -> String {
        match self.kind {
            OtpKind::Totp => totp_string(
                &self.secret,
                unix_time,
                self.period,
                DEFAULT_T0,
                self.digits,
                self.algorithm,
            ),
            OtpKind::Hotp => format_code(
                hotp_with_algo(&self.secret, self.counter, self.digits, self.algorithm),
                self.digits,
            ),
        }
    }
}

/// Minimal percent-decoding for URI labels / values. Invalid `%XY` escapes are
/// left verbatim (never panics); `+` is left as-is (otpauth labels are path/query
/// segments, not form-encoded). Sufficient for issuer/label display + parameter
/// values, which in practice are plain ASCII.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(h), Some(l)) = (hex_nibble(bytes[i + 1]), hex_nibble(bytes[i + 2])) {
                out.push((h << 4) | l);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

// ===========================================================================
// Host KAT suite — the FAIL-able proof (cargo test -p ath_otp)
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// The shared secret used by both RFC 4226 Appendix D and RFC 6238 Appendix B
    /// for SHA-1: the ASCII string "12345678901234567890" (20 bytes).
    const RFC_SECRET_SHA1: &[u8] = b"12345678901234567890";
    /// RFC 6238 Appendix B SHA-256 secret: 32 ASCII bytes "1234...12" repeated.
    const RFC_SECRET_SHA256: &[u8] = b"12345678901234567890123456789012";

    // ---- HOTP: RFC 4226 Appendix D — the load-bearing 10-code assert --------

    #[test]
    fn hotp_rfc4226_appendix_d_all_ten() {
        // FAIL-able: change any expected code and this turns red.
        let expected = [
            755224u32, 287082, 359152, 969429, 338314, 254676, 287922, 162583, 399871, 520489,
        ];
        for (counter, &want) in expected.iter().enumerate() {
            let got = hotp(RFC_SECRET_SHA1, counter as u64, 6);
            assert_eq!(got, want, "HOTP counter {counter} mismatch");
        }
    }

    #[test]
    fn hotp_string_is_zero_padded_six() {
        // Counter 0 -> 755224 (6 digits, no padding needed); verify width + value.
        assert_eq!(hotp_string(RFC_SECRET_SHA1, 0, 6), "755224");
        // Width is always exactly `digits`, zero-padded if the value is shorter.
        // HOTP counter 1 at 8 digits is 94287082 (RFC 4226 App D's full 31-bit
        // truncated value mod 10^8). FAIL-able: tweak either side.
        let s = hotp_string(RFC_SECRET_SHA1, 1, 8);
        assert_eq!(s.len(), 8);
        assert_eq!(s, "94287082");
        // A small value at a wide digit count must be left-padded with zeros.
        assert_eq!(format_code(42, 6), "000042");
    }

    // ---- TOTP: RFC 6238 Appendix B — SHA-1 and SHA-256 ----------------------

    #[test]
    fn totp_rfc6238_appendix_b_sha1() {
        // (time, 8-digit code) pairs from RFC 6238 Appendix B, SHA-1, step 30.
        let cases = [
            (59u64, "94287082"),
            (1111111109, "07081804"),
            (1111111111, "14050471"),
            (1234567890, "89005924"),
            (2000000000, "69279037"),
            (20000000000, "65353130"),
        ];
        for (t, want) in cases {
            let got = totp_string(RFC_SECRET_SHA1, t, 30, 0, 8, Algorithm::Sha1);
            assert_eq!(got, want, "TOTP SHA-1 at t={t}");
        }
    }

    #[test]
    fn totp_rfc6238_appendix_b_sha256() {
        // RFC 6238 Appendix B SHA-256 codes (same times, the 32-byte secret).
        let cases = [
            (59u64, "46119246"),
            (1111111109, "68084774"),
            (1111111111, "67062674"),
            (1234567890, "91819424"),
            (2000000000, "90698825"),
            (20000000000, "77737706"),
        ];
        for (t, want) in cases {
            let got = totp_string(RFC_SECRET_SHA256, t, 30, 0, 8, Algorithm::Sha256);
            assert_eq!(got, want, "TOTP SHA-256 at t={t}");
        }
    }

    #[test]
    fn totp_now_defaults_six_digits() {
        // totp_now == 6-digit SHA-1 step-30 t0-0. At t=59 the 8-digit code is
        // 94287082, so the 6-digit code is its last 6 digits: 287082.
        assert_eq!(totp_now(RFC_SECRET_SHA1, 59), "287082");
        assert_eq!(totp_now(RFC_SECRET_SHA1, 59).len(), 6);
    }

    // ---- base32 (RFC 4648) --------------------------------------------------

    #[test]
    fn base32_known_secret_roundtrips() {
        // "Hello!" base32-encodes to "JBSWY3DPEHPK3PXP" is the classic
        // example for "Hello!\xDE\xAD"; use the canonical RFC 4648 vector instead:
        // "foobar" -> "MZXW6YTBOI======".
        assert_eq!(decode_base32("MZXW6YTBOI======"), Some(b"foobar".to_vec()));
        // Padding is optional.
        assert_eq!(decode_base32("MZXW6YTBOI"), Some(b"foobar".to_vec()));
        // Case-insensitive + separators tolerated (how apps display secrets).
        assert_eq!(decode_base32("mzxw 6ytb-oi"), Some(b"foobar".to_vec()));
        // The RFC test vectors.
        assert_eq!(decode_base32("MY======"), Some(b"f".to_vec()));
        assert_eq!(decode_base32("MZXQ===="), Some(b"fo".to_vec()));
        assert_eq!(decode_base32(""), Some(Vec::new()));
    }

    #[test]
    fn base32_decode_then_hotp_matches_raw_secret() {
        // The RFC HOTP secret "12345678901234567890" base32-encodes to
        // "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ". Decoding it and running HOTP must
        // reproduce the Appendix D code — the real import path end to end.
        let decoded = decode_base32("GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ").expect("valid base32");
        assert_eq!(decoded, RFC_SECRET_SHA1);
        assert_eq!(hotp(&decoded, 0, 6), 755224);
    }

    #[test]
    fn base32_bad_chars_are_none() {
        assert_eq!(decode_base32("MZXW6YT!"), None); // '!' not in alphabet
        assert_eq!(decode_base32("01890"), None); // 0,1,8,9 not in base32
                                                  // Data after padding is malformed.
        assert_eq!(decode_base32("MY=A"), None);
    }

    // ---- totp_verify: skew window ------------------------------------------

    #[test]
    fn totp_verify_current_window() {
        // The exact current code passes; a wrong code fails.
        let code = totp_now(RFC_SECRET_SHA1, 1111111111);
        assert!(totp_verify(RFC_SECRET_SHA1, 1111111111, &code, 0));
        assert!(!totp_verify(RFC_SECRET_SHA1, 1111111111, "000000", 0));
    }

    #[test]
    fn totp_verify_skew_plus_minus_one_step() {
        // A code generated for the PREVIOUS step (30s earlier) must verify with
        // window=1 but FAIL with window=0 — proving the window actually widens.
        let now = 1111111111u64;
        let prev_step_time = now - 30;
        let prev_code = totp_now(RFC_SECRET_SHA1, prev_step_time);
        let next_step_time = now + 30;
        let next_code = totp_now(RFC_SECRET_SHA1, next_step_time);

        assert!(totp_verify(RFC_SECRET_SHA1, now, &prev_code, 1));
        assert!(totp_verify(RFC_SECRET_SHA1, now, &next_code, 1));
        assert!(!totp_verify(RFC_SECRET_SHA1, now, &prev_code, 0));
        // Two steps away is outside a window of 1.
        let far_code = totp_now(RFC_SECRET_SHA1, now + 90);
        assert!(!totp_verify(RFC_SECRET_SHA1, now, &far_code, 1));
    }

    #[test]
    fn totp_verify_wrong_length_rejected() {
        // A code of the wrong digit width can never match (and must not panic).
        assert!(!totp_verify(RFC_SECRET_SHA1, 59, "12345", 1)); // 5 digits
        assert!(!totp_verify(RFC_SECRET_SHA1, 59, "1234567", 1)); // 7 digits
        assert!(!totp_verify(RFC_SECRET_SHA1, 59, "", 1));
    }

    // ---- otpauth:// import --------------------------------------------------

    #[test]
    fn otpauth_totp_parse_and_code() {
        let uri = "otpauth://totp/ACME%20Co:alice@acme.com?secret=GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ&issuer=ACME%20Co&algorithm=SHA1&digits=6&period=30";
        let a = OtpAuth::parse(uri).expect("parse");
        assert_eq!(a.kind, OtpKind::Totp);
        assert_eq!(a.label, "ACME Co:alice@acme.com");
        assert_eq!(a.issuer.as_deref(), Some("ACME Co"));
        assert_eq!(a.algorithm, Algorithm::Sha1);
        assert_eq!(a.digits, 6);
        assert_eq!(a.period, 30);
        assert_eq!(a.secret, RFC_SECRET_SHA1);
        // Code at t=59 matches the RFC 6-digit value.
        assert_eq!(a.code_at(59), "287082");
    }

    #[test]
    fn otpauth_sha256_and_defaults() {
        // Minimal URI: only the secret. Algorithm/digits/period default.
        let a = OtpAuth::parse("otpauth://totp/Label?secret=MZXW6YTBOI").expect("parse");
        assert_eq!(a.algorithm, Algorithm::Sha1);
        assert_eq!(a.digits, 6);
        assert_eq!(a.period, 30);
        assert_eq!(a.secret, b"foobar");

        // SHA256 algorithm parameter is honored.
        let a2 =
            OtpAuth::parse("otpauth://totp/L?secret=MZXW6YTBOI&algorithm=SHA256").expect("parse");
        assert_eq!(a2.algorithm, Algorithm::Sha256);
    }

    #[test]
    fn otpauth_malformed_is_none_not_panic() {
        assert_eq!(OtpAuth::parse("https://example.com"), None); // wrong scheme
        assert_eq!(OtpAuth::parse("otpauth://xxx/L?secret=MY"), None); // bad kind
        assert_eq!(OtpAuth::parse("otpauth://totp/L?issuer=x"), None); // no secret
        assert_eq!(OtpAuth::parse("otpauth://totp/L?secret=!!!"), None); // bad b32
    }

    // ---- never-panic on hostile / edge input -------------------------------

    #[test]
    fn never_panic_edge_inputs() {
        // Empty secret: still produces a (useless but valid) code, no panic.
        let _ = hotp(b"", 0, 6);
        let _ = totp(b"", 0, 30, 0, 6, Algorithm::Sha1);
        // digits 0 clamps up to 1; digits 99 clamps down to 10. No overflow.
        assert_eq!(hotp_string(RFC_SECRET_SHA1, 0, 0).len(), 1);
        assert_eq!(hotp_string(RFC_SECRET_SHA1, 0, 99).len(), 10);
        // Huge counter / time are arithmetic-safe.
        let _ = hotp(RFC_SECRET_SHA1, u64::MAX, 6);
        let _ = totp(RFC_SECRET_SHA1, u64::MAX, 30, 0, 6, Algorithm::Sha1);
        // step 0 is treated as the default (no divide-by-zero).
        let _ = totp(RFC_SECRET_SHA1, 100, 0, 0, 6, Algorithm::Sha1);
        // time before t0 saturates to counter 0.
        let _ = totp(RFC_SECRET_SHA1, 5, 30, 1000, 6, Algorithm::Sha1);
        // Bad base32 variants never panic.
        assert_eq!(decode_base32("========"), Some(Vec::new()));
        let _ = decode_base32("ZZZZZZZZ\u{1F600}");
    }
}
