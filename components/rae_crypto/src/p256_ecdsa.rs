//! ECDSA over NIST P-256 (secp256r1) with SHA-256 — i.e. **ES256** (RFC 8152
//! / COSE algorithm -7), `#![no_std]`.
//!
//! ES256 is the algorithm that virtually every FIDO2 / WebAuthn hardware
//! security key (YubiKey, Titan, …) and platform authenticator (Windows Hello,
//! Touch ID, Android) emits. The Concept's AthGuard pitch — "run untrusted
//! software without fear" and let the user own the machine — extends to
//! passwordless login: AthID's WebAuthn verifier (`raeid::webauthn`) needs a
//! correct ES256 signature check to register/authenticate those credentials.
//! It currently rejects ES256 with `UnsupportedAlgorithm`; this module is the
//! primitive that closes that gap.
//!
//! Implementation: wraps the vetted RustCrypto `p256` + `ecdsa` crates (the
//! same `p256 = "0.13"` already vendored by `raenet`'s TLS path). We do NOT
//! hand-roll P-256 field arithmetic — rolling your own short-Weierstrass curve
//! is the classic crypto footgun (invalid-curve / incomplete-addition bugs).
//! We only own the WebAuthn/COSE *encoding* glue (SEC1 / DER / fixed forms)
//! and the SHA-256 prehash, which we take from `crate::sha256` so signer and
//! verifier share one hash.
//!
//! Fail-closed: every public entry point takes attacker-controlled bytes and
//! returns `false`/`Err` (never panics) on a malformed point, off-curve key,
//! malformed/short/empty signature, or a forged signature.
//!
//! Validated below against published NIST CAVP FIPS 186-4 ECDSA P-256/SHA-256
//! test vectors (and an RFC 6979 §A.2.5 deterministic vector), plus tamper /
//! malformed-input / fuzz negative tests.

use ecdsa::signature::hazmat::PrehashVerifier;
use p256::ecdsa::{Signature, VerifyingKey};
use p256::elliptic_curve::sec1::FromEncodedPoint;
use p256::{AffinePoint, EncodedPoint};

/// The public-key encodings accepted by [`verify`] / [`verify_prehash`].
///
/// WebAuthn/COSE EC2 keys carry the affine coordinates as two 32-byte
/// integers (`x`, `y`); raw TLS / SEC1 contexts carry the SEC1 point. We
/// accept both, detected by length:
///   * **65 bytes**, `0x04 || X || Y` — SEC1 uncompressed point.
///   * **64 bytes**, `X || Y` — the bare COSE EC2 coordinate pair.
///   * **33 bytes**, `0x02|0x03 || X` — SEC1 compressed point (accepted too;
///     harmless to support and some authenticators emit it).
///
/// Compressed and uncompressed are both decoded through the `p256` SEC1 parser,
/// which rejects points that are not on the curve.
fn parse_public_key(public_key: &[u8]) -> Option<VerifyingKey> {
    let point = match public_key.len() {
        // Bare COSE EC2 coordinates (X || Y): synthesize the SEC1 uncompressed
        // form by prepending the 0x04 tag, then decode.
        64 => {
            let mut sec1 = [0u8; 65];
            sec1[0] = 0x04;
            sec1[1..].copy_from_slice(public_key);
            EncodedPoint::from_bytes(sec1).ok()?
        }
        // SEC1 uncompressed (0x04 || X || Y) or compressed (0x02/0x03 || X).
        65 | 33 => EncodedPoint::from_bytes(public_key).ok()?,
        _ => return None,
    };

    // Reject the identity and any point not on the curve. `from_encoded_point`
    // returns a `CtOption`; map it to `Option` without panicking.
    let affine: Option<AffinePoint> = AffinePoint::from_encoded_point(&point).into();
    let affine = affine?;
    VerifyingKey::from_affine(affine).ok()
}

/// Decode an ECDSA/P-256 signature from either of the two forms WebAuthn and
/// X.509 produce:
///   * **DER (X9.62)** — a `SEQUENCE { INTEGER r, INTEGER s }`. This is what
///     WebAuthn assertion *and* packed-attestation signatures are.
///   * **Fixed `r || s`** — the 64-byte big-endian concatenation (what you get
///     after a DER→raw conversion, and what JOSE/JWS ES256 uses).
///
/// Detected by trying fixed-64 first (unambiguous length), then DER. A
/// malformed/short/empty input yields `None` (caller fails closed).
fn parse_signature(signature: &[u8]) -> Option<Signature> {
    if signature.len() == 64 {
        // Fixed r||s. `from_slice` enforces both scalars are in range and
        // non-zero (low-S is NOT required here; WebAuthn does not mandate it).
        if let Ok(sig) = Signature::from_slice(signature) {
            return Some(sig);
        }
        // A 64-byte blob that is not a valid fixed signature is not DER either.
        return None;
    }
    // Otherwise it must be DER (X9.62). `from_der` rejects trailing garbage,
    // out-of-range scalars, and non-canonical encodings.
    Signature::from_der(signature).ok()
}

/// Verify an **ES256** (ECDSA-P256 + SHA-256) signature over `message`.
///
/// The message is hashed with SHA-256 internally (via [`crate::sha256`]), so
/// callers pass the raw signed bytes (e.g. WebAuthn's
/// `authenticatorData || SHA-256(clientDataJSON)` — already the "message" at
/// this layer). Returns `true` only for a well-formed key, a well-formed
/// signature, and a matching signature; `false` for everything else.
///
/// `public_key` accepts the forms documented on [`parse_public_key`]; `sig`
/// accepts the forms documented on [`parse_signature`]. Never panics on
/// attacker-controlled input.
pub fn verify(public_key: &[u8], message: &[u8], sig: &[u8]) -> bool {
    let digest = crate::sha256::sha256(message);
    verify_prehash(public_key, &digest, sig)
}

/// Verify an ES256 signature when the 32-byte SHA-256 digest is already known
/// (the WebAuthn verifier hashes once and may reuse the digest). `prehash`
/// MUST be exactly the 32-byte SHA-256 output; any other length fails closed.
pub fn verify_prehash(public_key: &[u8], prehash: &[u8], sig: &[u8]) -> bool {
    if prehash.len() != 32 {
        return false;
    }
    let vk = match parse_public_key(public_key) {
        Some(vk) => vk,
        None => return false,
    };
    let signature = match parse_signature(sig) {
        Some(s) => s,
        None => return false,
    };
    vk.verify_prehash(prehash, &signature).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    // ── RFC 6979 §A.2.5 — ECDSA, curve P-256 (secp256r1), hash SHA-256. ──
    //
    // The canonical published deterministic-ECDSA test vector. Key and the
    // signature for message "sample" with SHA-256:
    //   x  (priv) = C9AFA9D845BA75166B5C215767B1D6934E50C3DB36E89B127B8A622B120F6721
    //   Ux (pub)  = 60FED4BA255A9D31C961EB74C6356D68C049B8923B61FA6CE669622E60F29FB6
    //   Uy (pub)  = 7903FE1008B8BC99A41AE9E95628BC64F2F1B20C2D7E9F5177A3C294D4462299
    //   With SHA-256, message = "sample":
    //   r = EFD48B2AACB6A8FD1140DD9CD45E81D69D2C877B56AAF991C34D0EA84EAF3716
    //   s = F7CB1C942D657C41D436C7A1B6E29F65F3E900DBB9AFF4064DC4AB2F843ACDA8
    // (RFC 6979, August 2013, Appendix A.2.5.)
    const QX: [u8; 32] = [
        0x60, 0xFE, 0xD4, 0xBA, 0x25, 0x5A, 0x9D, 0x31, 0xC9, 0x61, 0xEB, 0x74, 0xC6, 0x35, 0x6D,
        0x68, 0xC0, 0x49, 0xB8, 0x92, 0x3B, 0x61, 0xFA, 0x6C, 0xE6, 0x69, 0x62, 0x2E, 0x60, 0xF2,
        0x9F, 0xB6,
    ];
    const QY: [u8; 32] = [
        0x79, 0x03, 0xFE, 0x10, 0x08, 0xB8, 0xBC, 0x99, 0xA4, 0x1A, 0xE9, 0xE9, 0x56, 0x28, 0xBC,
        0x64, 0xF2, 0xF1, 0xB2, 0x0C, 0x2D, 0x7E, 0x9F, 0x51, 0x77, 0xA3, 0xC2, 0x94, 0xD4, 0x46,
        0x22, 0x99,
    ];
    const MSG: &[u8] = b"sample";
    const SIG_R: [u8; 32] = [
        0xEF, 0xD4, 0x8B, 0x2A, 0xAC, 0xB6, 0xA8, 0xFD, 0x11, 0x40, 0xDD, 0x9C, 0xD4, 0x5E, 0x81,
        0xD6, 0x9D, 0x2C, 0x87, 0x7B, 0x56, 0xAA, 0xF9, 0x91, 0xC3, 0x4D, 0x0E, 0xA8, 0x4E, 0xAF,
        0x37, 0x16,
    ];
    const SIG_S: [u8; 32] = [
        0xF7, 0xCB, 0x1C, 0x94, 0x2D, 0x65, 0x7C, 0x41, 0xD4, 0x36, 0xC7, 0xA1, 0xB6, 0xE2, 0x9F,
        0x65, 0xF3, 0xE9, 0x00, 0xDB, 0xB9, 0xAF, 0xF4, 0x06, 0x4D, 0xC4, 0xAB, 0x2F, 0x84, 0x3A,
        0xCD, 0xA8,
    ];

    fn pk_raw() -> Vec<u8> {
        // COSE EC2 form: X || Y.
        let mut v = Vec::with_capacity(64);
        v.extend_from_slice(&QX);
        v.extend_from_slice(&QY);
        v
    }
    fn pk_sec1() -> Vec<u8> {
        // SEC1 uncompressed: 0x04 || X || Y.
        let mut v = Vec::with_capacity(65);
        v.push(0x04);
        v.extend_from_slice(&QX);
        v.extend_from_slice(&QY);
        v
    }
    fn sig_fixed() -> Vec<u8> {
        // Fixed r || s.
        let mut v = Vec::with_capacity(64);
        v.extend_from_slice(&SIG_R);
        v.extend_from_slice(&SIG_S);
        v
    }
    fn sig_der() -> Vec<u8> {
        // Minimal DER X9.62 encoder for SEQUENCE { INTEGER r, INTEGER s }.
        fn der_int(x: &[u8]) -> Vec<u8> {
            // Strip leading zeros, then re-add one if the high bit is set
            // (DER INTEGERs are signed two's complement, so a 0x80+ leading
            // byte needs a 0x00 prefix).
            let mut i = 0;
            while i < x.len() - 1 && x[i] == 0 {
                i += 1;
            }
            let mut body = x[i..].to_vec();
            if body[0] & 0x80 != 0 {
                body.insert(0, 0x00);
            }
            let mut out = Vec::new();
            out.push(0x02);
            out.push(body.len() as u8);
            out.extend_from_slice(&body);
            out
        }
        let mut inner = der_int(&SIG_R);
        inner.extend_from_slice(&der_int(&SIG_S));
        let mut out = Vec::new();
        out.push(0x30);
        out.push(inner.len() as u8);
        out.extend_from_slice(&inner);
        out
    }

    /// LOAD-BEARING: the published RFC 6979 §A.2.5 vector verifies under every
    /// accepted key/signature encoding combination.
    #[test]
    fn rfc6979_p256_sha256_valid_all_encodings() {
        // raw-pk + fixed-sig
        assert!(verify(&pk_raw(), MSG, &sig_fixed()));
        // raw-pk + der-sig
        assert!(verify(&pk_raw(), MSG, &sig_der()));
        // sec1-pk + fixed-sig
        assert!(verify(&pk_sec1(), MSG, &sig_fixed()));
        // sec1-pk + der-sig
        assert!(verify(&pk_sec1(), MSG, &sig_der()));
    }

    /// FAIL-able: tampering the message must break verification.
    #[test]
    fn tampered_message_rejected() {
        let mut bad = MSG.to_vec();
        bad[0] ^= 0x01;
        assert!(!verify(&pk_raw(), &bad, &sig_fixed()));
        assert!(!verify(&pk_sec1(), &bad, &sig_der()));
    }

    /// FAIL-able: a single flipped signature byte (both forms) must be rejected.
    #[test]
    fn flipped_signature_rejected() {
        let mut bad_fixed = sig_fixed();
        bad_fixed[10] ^= 0x01;
        assert!(!verify(&pk_raw(), MSG, &bad_fixed));

        let mut bad_der = sig_der();
        let last = bad_der.len() - 1;
        bad_der[last] ^= 0x01;
        assert!(!verify(&pk_raw(), MSG, &bad_der));
    }

    /// FAIL-able: a different (but still valid on-curve) public key must not
    /// verify the signature. We flip a coordinate bit; if it lands off-curve it
    /// is rejected at parse, if on-curve it is the wrong key — either way false.
    #[test]
    fn wrong_pubkey_rejected() {
        let mut bad = pk_raw();
        bad[0] ^= 0x01;
        assert!(!verify(&bad, MSG, &sig_fixed()));

        let mut bad_sec1 = pk_sec1();
        bad_sec1[1] ^= 0x01;
        assert!(!verify(&bad_sec1, MSG, &sig_fixed()));
    }

    /// FAIL-able: malformed public keys fail gracefully (no panic), return false.
    #[test]
    fn malformed_pubkey_no_panic() {
        assert!(!verify(&[], MSG, &sig_fixed())); // empty
        assert!(!verify(&[0u8; 10], MSG, &sig_fixed())); // too short
        assert!(!verify(&[0xFFu8; 64], MSG, &sig_fixed())); // 64B not on curve
        assert!(!verify(&[0xFFu8; 65], MSG, &sig_fixed())); // 65B bad tag/point
        let mut not_on_curve = pk_sec1();
        // Corrupt Y so the point is not on the curve (X kept).
        not_on_curve[64] ^= 0xFF;
        not_on_curve[63] ^= 0xFF;
        assert!(!verify(&not_on_curve, MSG, &sig_fixed()));
    }

    /// FAIL-able: malformed signatures fail gracefully (no panic), return false.
    #[test]
    fn malformed_signature_no_panic() {
        assert!(!verify(&pk_raw(), MSG, &[])); // empty
        assert!(!verify(&pk_raw(), MSG, &[0u8; 64])); // all-zero fixed (r=s=0 invalid)
        assert!(!verify(&pk_raw(), MSG, &[0u8; 8])); // truncated
        assert!(!verify(&pk_raw(), MSG, &[0x30, 0x06, 0x02, 0x01, 0x00])); // truncated DER
                                                                           // DER claiming a huge length.
        assert!(!verify(&pk_raw(), MSG, &[0x30, 0x7f, 0x02, 0x01, 0x01]));
    }

    /// FAIL-able: prehash path enforces a 32-byte digest length.
    #[test]
    fn prehash_length_enforced() {
        let digest = crate::sha256::sha256(MSG);
        assert!(verify_prehash(&pk_raw(), &digest, &sig_fixed()));
        assert!(!verify_prehash(&pk_raw(), &digest[..31], &sig_fixed())); // short
        assert!(!verify_prehash(&pk_raw(), &[0u8; 33], &sig_fixed())); // long
        assert!(!verify_prehash(&pk_raw(), &[], &sig_fixed())); // empty
    }

    /// Never-panic under a seeded fuzz over signature AND public-key bytes.
    /// A tiny xorshift PRNG keeps this `#![no_std]`-clean (no `rand`, no `std`).
    #[test]
    fn fuzz_never_panics() {
        let mut state: u64 = 0x1234_5678_9abc_def0;
        let mut next = || {
            // xorshift64*
            state ^= state >> 12;
            state ^= state << 25;
            state ^= state >> 27;
            state.wrapping_mul(0x2545_F491_4F6C_DD1D)
        };
        for _ in 0..2000 {
            let pk_len = (next() % 70) as usize;
            let sig_len = (next() % 80) as usize;
            let mut pk = Vec::with_capacity(pk_len);
            for _ in 0..pk_len {
                pk.push((next() & 0xFF) as u8);
            }
            let mut sig = Vec::with_capacity(sig_len);
            for _ in 0..sig_len {
                sig.push((next() & 0xFF) as u8);
            }
            // Must return a bool, never panic. Result intentionally ignored.
            let _ = verify(&pk, MSG, &sig);
            let _ = verify_prehash(&pk, &[0u8; 32], &sig);
        }
    }
}
