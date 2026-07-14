//! ECDSA over NIST P-384 (secp384r1) with SHA-384 — i.e. **ES384** (RFC 8152
//! / COSE algorithm -35), `#![no_std]`.
//!
//! P-384/SHA-384 is the "high-assurance" ECDSA tier: the curve NSA Suite B /
//! CNSA reserves for TOP SECRET, and — the reason it lands here now — DNSSEC
//! algorithm **14** (`ECDSAP384SHA384`, RFC 6605). The Concept's AthNet pitch
//! is a resolver that can *prove* an answer is authentic instead of trusting
//! the wire; DNSSEC is how it does that, and a validator that silently skips an
//! algorithm is a downgrade hole. The kernel's `dns::verify_rrsig` already
//! covers algs 8/10/13/15; alg 14 was the last one it could not check and had
//! to fail to `Indeterminate`. This module is the primitive that closes DNSSEC
//! to algorithm-complete. It is equally the ES384 verifier for higher-assurance
//! COSE/WebAuthn credentials and P-384 code-signing chains (AthGuard).
//!
//! Implementation: wraps the vetted RustCrypto `p384` + `ecdsa` crates — the
//! exact sibling of the `p256` stack `p256_ecdsa` uses. We do NOT hand-roll
//! P-384 field arithmetic (rolling your own short-Weierstrass curve is the
//! classic crypto footgun: invalid-curve / incomplete-addition bugs). We own
//! only the DNSSEC/COSE *encoding* glue (SEC1 / DER / raw fixed forms) and the
//! SHA-384 prehash, taken from `sha2::Sha384` so signer and verifier share one
//! hash.
//!
//! Fail-closed: every public entry point takes attacker-controlled bytes
//! (hostile network RRSIGs) and returns `false` (never panics) on a malformed
//! point, off-curve key, malformed/short/empty signature, or a forged
//! signature. Length-mismatched keys/signatures are rejected before any
//! curve math.
//!
//! Validated below against the published NIST CAVP FIPS 186-4 ECDSA
//! P-384/SHA-384 SigVer vector (a genuine accept plus a genuine CAVP reject),
//! the RFC 6979 §A.2.6 deterministic P-384/SHA-384 vector, and tamper /
//! malformed-input / fuzz negative tests. The DNSSEC alg-14 end-to-end path is
//! additionally proven in `kernel::dns::run_boot_smoketest` against the real
//! RFC 6605 §6.2 `www.example.net` P-384 RRSIG.

use ecdsa::signature::hazmat::PrehashVerifier;
use p384::ecdsa::{Signature, VerifyingKey};
use p384::elliptic_curve::sec1::FromEncodedPoint;
use p384::{AffinePoint, EncodedPoint};
use sha2::{Digest, Sha384};

/// The public-key encodings accepted by [`verify`] / [`verify_prehash`].
///
/// DNSSEC alg-14 DNSKEYs (RFC 6605 §4) and COSE EC2 keys carry the affine
/// coordinates as two 48-byte integers (`x`, `y`); raw TLS / SEC1 contexts
/// carry the SEC1 point. We accept both, detected by length:
///   * **97 bytes**, `0x04 || X || Y` — SEC1 uncompressed point.
///   * **96 bytes**, `X || Y` — the bare DNSSEC / COSE EC2 coordinate pair.
///   * **49 bytes**, `0x02|0x03 || X` — SEC1 compressed point (accepted too;
///     harmless to support and some producers emit it).
///
/// Compressed and uncompressed are both decoded through the `p384` SEC1 parser,
/// which rejects points that are not on the curve.
fn parse_public_key(public_key: &[u8]) -> Option<VerifyingKey> {
    let point = match public_key.len() {
        // Bare DNSSEC/COSE coordinates (X || Y): synthesize the SEC1
        // uncompressed form by prepending the 0x04 tag, then decode.
        96 => {
            let mut sec1 = [0u8; 97];
            sec1[0] = 0x04;
            sec1[1..].copy_from_slice(public_key);
            EncodedPoint::from_bytes(sec1).ok()?
        }
        // SEC1 uncompressed (0x04 || X || Y) or compressed (0x02/0x03 || X).
        97 | 49 => EncodedPoint::from_bytes(public_key).ok()?,
        _ => return None,
    };

    // Reject the identity and any point not on the curve. `from_encoded_point`
    // returns a `CtOption`; map it to `Option` without panicking.
    let affine: Option<AffinePoint> = AffinePoint::from_encoded_point(&point).into();
    let affine = affine?;
    VerifyingKey::from_affine(affine).ok()
}

/// Decode an ECDSA/P-384 signature from either of the two forms DNSSEC, COSE
/// and X.509 produce:
///   * **Fixed `r || s`** — the 96-byte big-endian concatenation. This is the
///     DNSSEC RRSIG form (RFC 6605 §4) and the JOSE/JWS ES384 form.
///   * **DER (X9.62)** — a `SEQUENCE { INTEGER r, INTEGER s }` (X.509 / packed
///     attestation).
///
/// Detected by trying fixed-96 first (unambiguous length), then DER. A
/// malformed/short/empty input yields `None` (caller fails closed).
fn parse_signature(signature: &[u8]) -> Option<Signature> {
    if signature.len() == 96 {
        // Fixed r||s. `from_slice` enforces both scalars are in range and
        // non-zero (low-S is NOT required here; DNSSEC does not mandate it).
        if let Ok(sig) = Signature::from_slice(signature) {
            return Some(sig);
        }
        // A 96-byte blob that is not a valid fixed signature is not DER either.
        return None;
    }
    // Otherwise it must be DER (X9.62). `from_der` rejects trailing garbage,
    // out-of-range scalars, and non-canonical encodings.
    Signature::from_der(signature).ok()
}

/// Verify an **ES384** (ECDSA-P384 + SHA-384) signature over `message`.
///
/// The message is hashed with SHA-384 internally, so callers pass the raw
/// signed bytes (e.g. the RFC 4034 DNSSEC signed data — already the "message"
/// at this layer). Returns `true` only for a well-formed key, a well-formed
/// signature, and a matching signature; `false` for everything else.
///
/// `pubkey` accepts the forms documented on [`parse_public_key`]; `sig` accepts
/// the forms documented on [`parse_signature`]. Never panics on
/// attacker-controlled input.
pub fn verify(pubkey: &[u8], message: &[u8], sig: &[u8]) -> bool {
    let digest = Sha384::digest(message);
    verify_prehash(pubkey, &digest, sig)
}

/// Verify an ES384 signature when the 48-byte SHA-384 digest is already known.
/// `prehash` MUST be exactly the 48-byte SHA-384 output; any other length fails
/// closed.
pub fn verify_prehash(pubkey: &[u8], prehash: &[u8], sig: &[u8]) -> bool {
    if prehash.len() != 48 {
        return false;
    }
    let vk = match parse_public_key(pubkey) {
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

    // ── NIST CAVP FIPS 186-4 ECDSA SigVer.rsp, [P-384,SHA-384], a genuine
    //    "Result = P" (accept) vector. Cross-checked with the Python
    //    `cryptography` oracle (`ec.ECDSA(hashes.SHA384())`) before embedding. ──
    const CAVP_QX: [u8; 48] = [
        0xcb, 0x90, 0x8b, 0x1f, 0xd5, 0x16, 0xa5, 0x7b, 0x8e, 0xe1, 0xe1, 0x43, 0x83, 0x57, 0x9b,
        0x33, 0xcb, 0x15, 0x4f, 0xec, 0xe2, 0x0c, 0x50, 0x35, 0xe2, 0xb3, 0x76, 0x51, 0x95, 0xd1,
        0x95, 0x1d, 0x75, 0xbd, 0x78, 0xfb, 0x23, 0xe0, 0x0f, 0xef, 0x37, 0xd7, 0xd0, 0x64, 0xfd,
        0x9a, 0xf1, 0x44,
    ];
    const CAVP_QY: [u8; 48] = [
        0xcd, 0x99, 0xc4, 0x6b, 0x58, 0x57, 0x40, 0x1d, 0xdc, 0xff, 0x2c, 0xf7, 0xcf, 0x82, 0x21,
        0x21, 0xfa, 0xf1, 0xcb, 0xad, 0x9a, 0x01, 0x1b, 0xed, 0x8c, 0x55, 0x1f, 0x6f, 0x59, 0xb2,
        0xc3, 0x60, 0xf7, 0x9b, 0xfb, 0xe3, 0x2a, 0xdb, 0xca, 0xa0, 0x95, 0x83, 0xbd, 0xfd, 0xf7,
        0xc3, 0x74, 0xbb,
    ];
    const CAVP_MSG: [u8; 128] = [
        0x9d, 0xd7, 0x89, 0xea, 0x25, 0xc0, 0x47, 0x45, 0xd5, 0x7a, 0x38, 0x1f, 0x22, 0xde, 0x01,
        0xfb, 0x0a, 0xbd, 0x3c, 0x72, 0xdb, 0xde, 0xfd, 0x44, 0xe4, 0x32, 0x13, 0xc1, 0x89, 0x58,
        0x3e, 0xef, 0x85, 0xba, 0x66, 0x20, 0x44, 0xda, 0x3d, 0xe2, 0xdd, 0x86, 0x70, 0xe6, 0x32,
        0x51, 0x54, 0x48, 0x01, 0x55, 0xbb, 0xee, 0xbb, 0x70, 0x2c, 0x75, 0x78, 0x1a, 0xc3, 0x2e,
        0x13, 0x94, 0x18, 0x60, 0xcb, 0x57, 0x6f, 0xe3, 0x7a, 0x05, 0xb7, 0x57, 0xda, 0x5b, 0x5b,
        0x41, 0x8f, 0x6d, 0xd7, 0xc3, 0x0b, 0x04, 0x2e, 0x40, 0xf4, 0x39, 0x5a, 0x34, 0x2a, 0xe4,
        0xdc, 0xe0, 0x56, 0x34, 0xc3, 0x36, 0x25, 0xe2, 0xbc, 0x52, 0x43, 0x45, 0x48, 0x1f, 0x7e,
        0x25, 0x3d, 0x95, 0x51, 0x26, 0x68, 0x23, 0x77, 0x1b, 0x25, 0x17, 0x05, 0xb4, 0xa8, 0x51,
        0x66, 0x02, 0x2a, 0x37, 0xac, 0x28, 0xf1, 0xbd,
    ];
    const CAVP_R: [u8; 48] = [
        0x33, 0xf6, 0x4f, 0xb6, 0x5c, 0xd6, 0xa8, 0x91, 0x85, 0x23, 0xf2, 0x3a, 0xea, 0x0b, 0xbc,
        0xf5, 0x6b, 0xba, 0x1d, 0xac, 0xa7, 0xaf, 0xf8, 0x17, 0xc8, 0x79, 0x1d, 0xc9, 0x24, 0x28,
        0xd6, 0x05, 0xac, 0x62, 0x9d, 0xe2, 0xe8, 0x47, 0xd4, 0x3c, 0xee, 0x55, 0xba, 0x9e, 0x4a,
        0x0e, 0x83, 0xba,
    ];
    const CAVP_S: [u8; 48] = [
        0x44, 0x28, 0xbb, 0x47, 0x8a, 0x43, 0xac, 0x73, 0xec, 0xd6, 0xde, 0x51, 0xdd, 0xf7, 0xc2,
        0x8f, 0xf3, 0xc2, 0x44, 0x16, 0x25, 0xa0, 0x81, 0x71, 0x43, 0x37, 0xdd, 0x44, 0xfe, 0xa8,
        0x01, 0x1b, 0xae, 0x71, 0x95, 0x9a, 0x10, 0x94, 0x7b, 0x6e, 0xa3, 0x3f, 0x77, 0xe1, 0x28,
        0xd3, 0xc6, 0xae,
    ];

    // ── RFC 6979 §A.2.6 — ECDSA, curve P-384 (secp384r1), hash SHA-384,
    //    message "sample". The canonical published deterministic-ECDSA vector
    //    (RFC 6979, August 2013). Also cross-checked with the `cryptography`
    //    oracle before embedding. ──
    const RFC6979_QX: [u8; 48] = [
        0xec, 0x3a, 0x4e, 0x41, 0x5b, 0x4e, 0x19, 0xa4, 0x56, 0x86, 0x18, 0x02, 0x9f, 0x42, 0x7f,
        0xa5, 0xda, 0x9a, 0x8b, 0xc4, 0xae, 0x92, 0xe0, 0x2e, 0x06, 0xaa, 0xe5, 0x28, 0x6b, 0x30,
        0x0c, 0x64, 0xde, 0xf8, 0xf0, 0xea, 0x90, 0x55, 0x86, 0x60, 0x64, 0xa2, 0x54, 0x51, 0x54,
        0x80, 0xbc, 0x13,
    ];
    const RFC6979_QY: [u8; 48] = [
        0x80, 0x15, 0xd9, 0xb7, 0x2d, 0x7d, 0x57, 0x24, 0x4e, 0xa8, 0xef, 0x9a, 0xc0, 0xc6, 0x21,
        0x89, 0x67, 0x08, 0xa5, 0x93, 0x67, 0xf9, 0xdf, 0xb9, 0xf5, 0x4c, 0xa8, 0x4b, 0x3f, 0x1c,
        0x9d, 0xb1, 0x28, 0x8b, 0x23, 0x1c, 0x3a, 0xe0, 0xd4, 0xfe, 0x73, 0x44, 0xfd, 0x25, 0x33,
        0x26, 0x47, 0x20,
    ];
    const RFC6979_MSG: &[u8] = b"sample";
    const RFC6979_R: [u8; 48] = [
        0x94, 0xed, 0xbb, 0x92, 0xa5, 0xec, 0xb8, 0xaa, 0xd4, 0x73, 0x6e, 0x56, 0xc6, 0x91, 0x91,
        0x6b, 0x3f, 0x88, 0x14, 0x06, 0x66, 0xce, 0x9f, 0xa7, 0x3d, 0x64, 0xc4, 0xea, 0x95, 0xad,
        0x13, 0x3c, 0x81, 0xa6, 0x48, 0x15, 0x2e, 0x44, 0xac, 0xf9, 0x6e, 0x36, 0xdd, 0x1e, 0x80,
        0xfa, 0xbe, 0x46,
    ];
    const RFC6979_S: [u8; 48] = [
        0x99, 0xef, 0x4a, 0xeb, 0x15, 0xf1, 0x78, 0xce, 0xa1, 0xfe, 0x40, 0xdb, 0x26, 0x03, 0x13,
        0x8f, 0x13, 0x0e, 0x74, 0x0a, 0x19, 0x62, 0x45, 0x26, 0x20, 0x3b, 0x63, 0x51, 0xd0, 0xa3,
        0xa9, 0x4f, 0xa3, 0x29, 0xc1, 0x45, 0x78, 0x6e, 0x67, 0x9e, 0x7b, 0x82, 0xc7, 0x1a, 0x38,
        0x62, 0x8a, 0xc8,
    ];

    fn pk_raw(qx: &[u8; 48], qy: &[u8; 48]) -> Vec<u8> {
        // DNSSEC/COSE form: X || Y (96 bytes).
        let mut v = Vec::with_capacity(96);
        v.extend_from_slice(qx);
        v.extend_from_slice(qy);
        v
    }
    fn pk_sec1(qx: &[u8; 48], qy: &[u8; 48]) -> Vec<u8> {
        // SEC1 uncompressed: 0x04 || X || Y (97 bytes).
        let mut v = Vec::with_capacity(97);
        v.push(0x04);
        v.extend_from_slice(qx);
        v.extend_from_slice(qy);
        v
    }
    fn sig_fixed(r: &[u8; 48], s: &[u8; 48]) -> Vec<u8> {
        // Fixed r || s (96 bytes).
        let mut v = Vec::with_capacity(96);
        v.extend_from_slice(r);
        v.extend_from_slice(s);
        v
    }
    fn sig_der(r: &[u8; 48], s: &[u8; 48]) -> Vec<u8> {
        // Minimal DER X9.62 encoder for SEQUENCE { INTEGER r, INTEGER s }.
        fn der_int(x: &[u8]) -> Vec<u8> {
            // Strip leading zeros, then re-add one if the high bit is set (DER
            // INTEGERs are signed two's complement).
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
        let mut inner = der_int(r);
        inner.extend_from_slice(&der_int(s));
        let mut out = Vec::new();
        out.push(0x30);
        out.push(inner.len() as u8);
        out.extend_from_slice(&inner);
        out
    }

    /// LOAD-BEARING: the published NIST CAVP FIPS 186-4 P-384/SHA-384 accept
    /// vector verifies under every accepted key/signature encoding combination.
    #[test]
    fn cavp_p384_sha384_valid_all_encodings() {
        let raw = pk_raw(&CAVP_QX, &CAVP_QY);
        let sec1 = pk_sec1(&CAVP_QX, &CAVP_QY);
        let fixed = sig_fixed(&CAVP_R, &CAVP_S);
        let der = sig_der(&CAVP_R, &CAVP_S);
        assert!(verify(&raw, &CAVP_MSG, &fixed));
        assert!(verify(&raw, &CAVP_MSG, &der));
        assert!(verify(&sec1, &CAVP_MSG, &fixed));
        assert!(verify(&sec1, &CAVP_MSG, &der));
    }

    /// LOAD-BEARING: the RFC 6979 §A.2.6 deterministic P-384/SHA-384 vector
    /// (message "sample") verifies too — a second independent published source.
    #[test]
    fn rfc6979_p384_sha384_valid() {
        let raw = pk_raw(&RFC6979_QX, &RFC6979_QY);
        let sec1 = pk_sec1(&RFC6979_QX, &RFC6979_QY);
        assert!(verify(
            &raw,
            RFC6979_MSG,
            &sig_fixed(&RFC6979_R, &RFC6979_S)
        ));
        assert!(verify(&raw, RFC6979_MSG, &sig_der(&RFC6979_R, &RFC6979_S)));
        assert!(verify(
            &sec1,
            RFC6979_MSG,
            &sig_fixed(&RFC6979_R, &RFC6979_S)
        ));
    }

    /// FAIL-able: tampering the message must break verification.
    #[test]
    fn tampered_message_rejected() {
        let raw = pk_raw(&CAVP_QX, &CAVP_QY);
        let mut bad = CAVP_MSG.to_vec();
        bad[0] ^= 0x01;
        assert!(!verify(&raw, &bad, &sig_fixed(&CAVP_R, &CAVP_S)));
        assert!(!verify(&raw, &bad, &sig_der(&CAVP_R, &CAVP_S)));
    }

    /// FAIL-able: a single flipped signature byte (both forms) must be rejected.
    #[test]
    fn flipped_signature_rejected() {
        let raw = pk_raw(&CAVP_QX, &CAVP_QY);
        let mut bad_fixed = sig_fixed(&CAVP_R, &CAVP_S);
        bad_fixed[10] ^= 0x01;
        assert!(!verify(&raw, &CAVP_MSG, &bad_fixed));

        let mut bad_der = sig_der(&CAVP_R, &CAVP_S);
        let last = bad_der.len() - 1;
        bad_der[last] ^= 0x01;
        assert!(!verify(&raw, &CAVP_MSG, &bad_der));
    }

    /// FAIL-able: a genuine NIST CAVP "Result = F (2 - R changed)" reject
    /// vector must NOT verify — proves we reject a forged R against a real
    /// on-curve key/message from the CAVP corpus.
    #[test]
    fn cavp_p384_sha384_negative_vector_rejected() {
        const QX: [u8; 48] = [
            0x1f, 0x94, 0xeb, 0x6f, 0x43, 0x9a, 0x38, 0x06, 0xf8, 0x05, 0x4d, 0xd7, 0x91, 0x24,
            0x84, 0x7d, 0x13, 0x8d, 0x14, 0xd4, 0xf5, 0x2b, 0xac, 0x93, 0xb0, 0x42, 0xf2, 0xee,
            0x3c, 0xdb, 0x7d, 0xc9, 0xe0, 0x99, 0x25, 0xc2, 0xa5, 0xfe, 0xe7, 0x0d, 0x4c, 0xe0,
            0x8c, 0x61, 0xe3, 0xb1, 0x91, 0x60,
        ];
        const QY: [u8; 48] = [
            0x1c, 0x4f, 0xd1, 0x11, 0xf6, 0xe3, 0x33, 0x03, 0x06, 0x94, 0x21, 0xde, 0xb3, 0x1e,
            0x87, 0x31, 0x26, 0xbe, 0x35, 0xee, 0xb4, 0x36, 0xfe, 0x20, 0x34, 0x85, 0x6a, 0x3e,
            0xd1, 0xe8, 0x97, 0xf2, 0x6c, 0x84, 0x6e, 0xe3, 0x23, 0x3c, 0xd1, 0x62, 0x40, 0x98,
            0x9a, 0x79, 0x90, 0xc1, 0x9d, 0x8c,
        ];
        const MSG: [u8; 128] = [
            0x41, 0x32, 0x83, 0x3a, 0x52, 0x5a, 0xec, 0xc8, 0xa1, 0xa6, 0xde, 0xa9, 0xf4, 0x07,
            0x5f, 0x44, 0xfe, 0xef, 0xce, 0x81, 0x0c, 0x46, 0x68, 0x42, 0x3b, 0x38, 0x58, 0x04,
            0x17, 0xf7, 0xbd, 0xca, 0x5b, 0x21, 0x06, 0x1a, 0x45, 0xea, 0xa3, 0xcb, 0xe2, 0xa7,
            0x03, 0x5e, 0xd1, 0x89, 0x52, 0x3a, 0xf8, 0x00, 0x2d, 0x65, 0xc2, 0x89, 0x9e, 0x65,
            0x73, 0x5e, 0x4d, 0x93, 0xa1, 0x65, 0x03, 0xc1, 0x45, 0x05, 0x9f, 0x36, 0x5c, 0x32,
            0xb3, 0xac, 0xc6, 0x27, 0x0e, 0x29, 0xa0, 0x91, 0x31, 0x29, 0x91, 0x81, 0xc9, 0x8b,
            0x3c, 0x76, 0x76, 0x9a, 0x18, 0xfa, 0xf2, 0x1f, 0x6b, 0x4a, 0x8f, 0x27, 0x1e, 0x6b,
            0xf9, 0x08, 0xe2, 0x38, 0xaf, 0xe8, 0x00, 0x2e, 0x27, 0xc6, 0x34, 0x17, 0xbd, 0xa7,
            0x58, 0xf8, 0x46, 0xe1, 0xe3, 0xb8, 0xe6, 0x2d, 0x7f, 0x05, 0xeb, 0xd9, 0x8f, 0x1f,
            0x91, 0x54,
        ];
        const R: [u8; 48] = [
            0x3c, 0x15, 0xc3, 0xce, 0xdf, 0x2a, 0x6f, 0xbf, 0xf2, 0xf9, 0x06, 0xe6, 0x61, 0xf5,
            0x93, 0x2f, 0x25, 0x42, 0xf0, 0xce, 0x68, 0xe2, 0xa8, 0x18, 0x2e, 0x5e, 0xd3, 0x85,
            0x8f, 0x33, 0xbd, 0x3c, 0x56, 0x66, 0xf1, 0x7a, 0xc3, 0x9e, 0x52, 0xcb, 0x00, 0x4b,
            0x80, 0xa0, 0xd4, 0xba, 0x73, 0xcd,
        ];
        const S: [u8; 48] = [
            0x9d, 0xe8, 0x79, 0x08, 0x3c, 0xbb, 0x0a, 0x97, 0x97, 0x3c, 0x94, 0xf1, 0x96, 0x3d,
            0x84, 0xf5, 0x81, 0xe4, 0xc6, 0x54, 0x1b, 0x7d, 0x00, 0x0f, 0x98, 0x50, 0xde, 0xb2,
            0x51, 0x54, 0xb2, 0x3a, 0x37, 0xdd, 0x72, 0x26, 0x7b, 0xdd, 0x72, 0x66, 0x5c, 0xc7,
            0x02, 0x7f, 0x88, 0x16, 0x4f, 0xab,
        ];
        assert!(!verify(&pk_raw(&QX, &QY), &MSG, &sig_fixed(&R, &S)));
    }

    /// FAIL-able: a different (but still valid on-curve) public key must not
    /// verify the signature. We flip a coordinate bit; if it lands off-curve it
    /// is rejected at parse, if on-curve it is the wrong key — either way false.
    #[test]
    fn wrong_pubkey_rejected() {
        let mut bad = pk_raw(&CAVP_QX, &CAVP_QY);
        bad[0] ^= 0x01;
        assert!(!verify(&bad, &CAVP_MSG, &sig_fixed(&CAVP_R, &CAVP_S)));

        let mut bad_sec1 = pk_sec1(&CAVP_QX, &CAVP_QY);
        bad_sec1[1] ^= 0x01;
        assert!(!verify(&bad_sec1, &CAVP_MSG, &sig_fixed(&CAVP_R, &CAVP_S)));
    }

    /// FAIL-able: malformed public keys fail gracefully (no panic), return false.
    #[test]
    fn malformed_pubkey_no_panic() {
        let sig = sig_fixed(&CAVP_R, &CAVP_S);
        assert!(!verify(&[], &CAVP_MSG, &sig)); // empty
        assert!(!verify(&[0u8; 10], &CAVP_MSG, &sig)); // too short
        assert!(!verify(&[0xFFu8; 96], &CAVP_MSG, &sig)); // 96B not on curve
        assert!(!verify(&[0xFFu8; 97], &CAVP_MSG, &sig)); // 97B bad tag/point
        assert!(!verify(&[0u8; 64], &CAVP_MSG, &sig)); // P-256 length, wrong curve
        let mut not_on_curve = pk_sec1(&CAVP_QX, &CAVP_QY);
        // Corrupt Y so the point is not on the curve (X kept).
        let n = not_on_curve.len();
        not_on_curve[n - 1] ^= 0xFF;
        not_on_curve[n - 2] ^= 0xFF;
        assert!(!verify(&not_on_curve, &CAVP_MSG, &sig));
    }

    /// FAIL-able: malformed signatures fail gracefully (no panic), return false.
    #[test]
    fn malformed_signature_no_panic() {
        let raw = pk_raw(&CAVP_QX, &CAVP_QY);
        assert!(!verify(&raw, &CAVP_MSG, &[])); // empty
        assert!(!verify(&raw, &CAVP_MSG, &[0u8; 96])); // all-zero fixed (r=s=0 invalid)
        assert!(!verify(&raw, &CAVP_MSG, &[0u8; 8])); // truncated
        assert!(!verify(&raw, &CAVP_MSG, &[0u8; 64])); // P-256 sig length, wrong curve
        assert!(!verify(&raw, &CAVP_MSG, &[0x30, 0x06, 0x02, 0x01, 0x00])); // truncated DER
        assert!(!verify(&raw, &CAVP_MSG, &[0x30, 0x7f, 0x02, 0x01, 0x01])); // DER lying about length
    }

    /// FAIL-able: prehash path enforces a 48-byte digest length.
    #[test]
    fn prehash_length_enforced() {
        let raw = pk_raw(&CAVP_QX, &CAVP_QY);
        let sig = sig_fixed(&CAVP_R, &CAVP_S);
        let digest = Sha384::digest(CAVP_MSG);
        assert!(verify_prehash(&raw, &digest, &sig));
        assert!(!verify_prehash(&raw, &digest[..47], &sig)); // short
        assert!(!verify_prehash(&raw, &[0u8; 49], &sig)); // long
        assert!(!verify_prehash(&raw, &[], &sig)); // empty
        assert!(!verify_prehash(&raw, &[0u8; 32], &sig)); // SHA-256 length, wrong hash
    }

    /// Never-panic under a seeded fuzz over signature AND public-key bytes.
    /// A tiny xorshift PRNG keeps this `#![no_std]`-clean (no `rand`, no `std`).
    #[test]
    fn fuzz_never_panics() {
        let mut state: u64 = 0x0f38_1c2b_5a7d_9e01;
        let mut next = || {
            // xorshift64*
            state ^= state >> 12;
            state ^= state << 25;
            state ^= state >> 27;
            state.wrapping_mul(0x2545_F491_4F6C_DD1D)
        };
        for _ in 0..2000 {
            let pk_len = (next() % 104) as usize;
            let sig_len = (next() % 112) as usize;
            let mut pk = Vec::with_capacity(pk_len);
            for _ in 0..pk_len {
                pk.push((next() & 0xFF) as u8);
            }
            let mut sig = Vec::with_capacity(sig_len);
            for _ in 0..sig_len {
                sig.push((next() & 0xFF) as u8);
            }
            // Must return a bool, never panic. Result intentionally ignored.
            let _ = verify(&pk, &CAVP_MSG, &sig);
            let _ = verify_prehash(&pk, &[0u8; 48], &sig);
        }
    }
}
