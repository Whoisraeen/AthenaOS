//! RSASSA-PKCS1-v1_5 signature **verification** (RFC 8017 §8.2.2), `#![no_std]`.
//!
//! The one true home for RSA verify in the tree. This is lifted verbatim (same
//! schoolbook big-integer modexp, same fail-closed EM encode-and-compare) from
//! the kernel's `crypto.rs`, so the kernel's DNSSEC / X.509 path and userspace
//! WebAuthn (`athid::webauthn`, COSE **RS256** = alg `-257`, the algorithm
//! Windows Hello's TPM platform authenticator emits) share ONE implementation
//! instead of two copies drifting apart.
//!
//! ## Verify-only, and deliberately not constant-time
//!
//! RSA *verify* is the PUBLIC-key operation `sig^e mod n`: it touches only
//! public values (the signature, the modulus, the small public exponent), so a
//! straightforward square-and-multiply modexp is both correct and safe. There is
//! **no** sign / decrypt / keygen here on purpose — those touch the private
//! exponent and require a constant-time bignum this module does not provide. If
//! you need a private-key op, it does not belong in this module.
//!
//! ## Fail-closed
//!
//! Every entry point takes attacker-controlled bytes and returns `false` (never
//! panics, never a false accept) on a length mismatch, malformed PKCS#1 padding,
//! an out-of-range signature representative, or a forged signature. Validated
//! against the RFC 8017 / NIST vectors by the kernel `crypto::run_boot_smoketest`
//! and by `athid::webauthn`'s genuine RS256 WebAuthn assertion KAT.

use alloc::vec::Vec;

use crate::sha256::sha256;

/// Fixed DER `DigestInfo` prefix for SHA-256 (RFC 8017 §9.2, Note 1).
const SHA256_DIGESTINFO: [u8; 19] = [
    0x30, 0x31, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x01, 0x05,
    0x00, 0x04, 0x20,
];

/// Minimal, fixed-purpose big unsigned integer for RSA signature VERIFICATION
/// — i.e. the PUBLIC-key operation `sig^e mod n`. Little-endian `u32` limbs.
///
/// Deliberately NOT constant-time (public values only); every routine is
/// panic-free on attacker-sized inputs.
mod bignum {
    use alloc::vec::Vec;
    use core::cmp::Ordering;

    #[derive(Clone)]
    pub struct BigUint {
        /// Little-endian base-2^32 limbs, normalized (no trailing zero limbs).
        /// The empty vector represents zero.
        limbs: Vec<u32>,
    }

    impl BigUint {
        pub fn zero() -> Self {
            BigUint { limbs: Vec::new() }
        }
        pub fn one() -> Self {
            BigUint {
                limbs: alloc::vec![1u32],
            }
        }
        fn normalize(&mut self) {
            while matches!(self.limbs.last(), Some(&0)) {
                self.limbs.pop();
            }
        }
        pub fn is_zero(&self) -> bool {
            self.limbs.is_empty()
        }

        /// Parse big-endian bytes into little-endian limbs.
        pub fn from_bytes_be(bytes: &[u8]) -> Self {
            let mut limbs = Vec::with_capacity(bytes.len() / 4 + 1);
            let mut i = bytes.len();
            while i > 0 {
                let start = i.saturating_sub(4);
                let mut limb: u32 = 0;
                for &b in &bytes[start..i] {
                    limb = (limb << 8) | b as u32;
                }
                limbs.push(limb);
                i = start;
            }
            let mut v = BigUint { limbs };
            v.normalize();
            v
        }

        /// Serialize to EXACTLY `out_len` big-endian bytes; `None` if the value
        /// does not fit (any significant byte would fall outside the field).
        pub fn to_bytes_be(&self, out_len: usize) -> Option<Vec<u8>> {
            let mut out = alloc::vec![0u8; out_len];
            for (li, &limb) in self.limbs.iter().enumerate() {
                let lb = limb.to_le_bytes(); // [lsb, .., msb]
                for k in 0..4 {
                    let byteval = lb[k];
                    let pos_from_end = li * 4 + k; // 0 == least-significant byte
                    if pos_from_end >= out_len {
                        if byteval != 0 {
                            return None;
                        }
                    } else {
                        out[out_len - 1 - pos_from_end] = byteval;
                    }
                }
            }
            Some(out)
        }

        pub fn cmp(&self, other: &BigUint) -> Ordering {
            if self.limbs.len() != other.limbs.len() {
                return self.limbs.len().cmp(&other.limbs.len());
            }
            for i in (0..self.limbs.len()).rev() {
                if self.limbs[i] != other.limbs[i] {
                    return self.limbs[i].cmp(&other.limbs[i]);
                }
            }
            Ordering::Equal
        }

        fn bit_len(&self) -> usize {
            match self.limbs.last() {
                None => 0,
                Some(&top) => (self.limbs.len() - 1) * 32 + (32 - top.leading_zeros() as usize),
            }
        }

        fn test_bit(&self, i: usize) -> bool {
            let limb = i / 32;
            if limb >= self.limbs.len() {
                return false;
            }
            (self.limbs[limb] >> (i % 32)) & 1 == 1
        }

        /// self = self * 2 + (bit & 1).
        fn shl1_or(&mut self, bit: u32) {
            let mut carry = bit & 1;
            for limb in self.limbs.iter_mut() {
                let new_carry = *limb >> 31;
                *limb = (*limb << 1) | carry;
                carry = new_carry;
            }
            if carry != 0 {
                self.limbs.push(carry);
            }
        }

        /// self -= other. Requires self >= other (caller guarantees).
        fn sub_assign(&mut self, other: &BigUint) {
            let mut borrow: u64 = 0;
            for i in 0..self.limbs.len() {
                let o = if i < other.limbs.len() {
                    other.limbs[i] as u64
                } else {
                    0
                };
                let sub = o + borrow;
                let cur = self.limbs[i] as u64;
                if cur >= sub {
                    self.limbs[i] = (cur - sub) as u32;
                    borrow = 0;
                } else {
                    self.limbs[i] = (cur + (1u64 << 32) - sub) as u32;
                    borrow = 1;
                }
            }
            self.normalize();
        }

        /// Schoolbook multiply.
        pub fn mul(&self, other: &BigUint) -> BigUint {
            if self.is_zero() || other.is_zero() {
                return BigUint::zero();
            }
            let mut out = alloc::vec![0u32; self.limbs.len() + other.limbs.len()];
            for (i, &a) in self.limbs.iter().enumerate() {
                let a64 = a as u64;
                let mut carry: u64 = 0;
                for (j, &b) in other.limbs.iter().enumerate() {
                    let idx = i + j;
                    let cur = out[idx] as u64 + a64 * b as u64 + carry;
                    out[idx] = cur as u32;
                    carry = cur >> 32;
                }
                let mut k = i + other.limbs.len();
                while carry != 0 && k < out.len() {
                    let cur = out[k] as u64 + carry;
                    out[k] = cur as u32;
                    carry = cur >> 32;
                    k += 1;
                }
            }
            let mut v = BigUint { limbs: out };
            v.normalize();
            v
        }

        /// self mod m, via binary long division (m must be non-zero). The
        /// remainder never exceeds m, so there is no unbounded growth.
        pub fn rem(&self, m: &BigUint) -> BigUint {
            if m.is_zero() {
                return BigUint::zero();
            }
            if self.cmp(m) == Ordering::Less {
                return self.clone();
            }
            let mut r = BigUint::zero();
            for i in (0..self.bit_len()).rev() {
                r.shl1_or(if self.test_bit(i) { 1 } else { 0 });
                if r.cmp(m) != Ordering::Less {
                    r.sub_assign(m);
                }
            }
            r
        }

        /// self^exp mod m (square-and-multiply, MSB→LSB). Public values only.
        pub fn modpow(&self, exp: &BigUint, m: &BigUint) -> BigUint {
            if m.is_zero() {
                return BigUint::zero();
            }
            let base = self.rem(m);
            let mut result = BigUint::one().rem(m); // 1 mod m (handles m == 1)
            for i in (0..exp.bit_len()).rev() {
                result = result.mul(&result).rem(m);
                if exp.test_bit(i) {
                    result = result.mul(&base).rem(m);
                }
            }
            result
        }
    }
}

/// RFC 8017 §8.2.2 `RSASSA-PKCS1-v1_5-VERIFY` EM encode-and-compare core,
/// parameterized on the DER `DigestInfo` prefix and the caller's precomputed
/// message hash. Fully fail-closed: any length mismatch, malformed padding,
/// out-of-range signature, or modexp edge returns `false` — never a panic,
/// never a false accept. `n`/`e` are the public modulus / exponent as big-endian
/// bytes; `sig` must be exactly `n.len()` bytes; `hash` must be the digest whose
/// length matches `digest_info` (32 for SHA-256, 64 for SHA-512).
pub fn verify_pkcs1_digest(
    n: &[u8],
    e: &[u8],
    sig: &[u8],
    digest_info: &[u8],
    hash: &[u8],
) -> bool {
    use bignum::BigUint;
    let k = n.len();
    // 1. Length check: reject unless the signature is exactly one modulus wide.
    if k == 0 || sig.len() != k {
        return false;
    }
    // 2. The modulus must be large enough to hold a well-formed EM:
    //    0x00 || 0x01 || PS(>= 8 * 0xFF) || 0x00 || DigestInfo || H.
    let t_len = digest_info.len() + hash.len();
    if k < t_len + 11 {
        return false;
    }
    let n_big = BigUint::from_bytes_be(n);
    if n_big.is_zero() {
        return false;
    }
    let s_big = BigUint::from_bytes_be(sig);
    // RFC 8017 §5.2.2 step 1: the signature representative must be in [0, n-1].
    if s_big.cmp(&n_big) != core::cmp::Ordering::Less {
        return false;
    }
    let e_big = BigUint::from_bytes_be(e);
    // m = s^e mod n, rendered back to a k-byte octet string.
    let m = s_big.modpow(&e_big, &n_big);
    let em = match m.to_bytes_be(k) {
        Some(v) => v,
        None => return false,
    };
    // Recompute the expected EM and compare byte-for-byte (length is already k).
    let ps_len = k - t_len - 3;
    let mut expected = Vec::with_capacity(k);
    expected.push(0x00);
    expected.push(0x01);
    expected.extend(core::iter::repeat(0xFFu8).take(ps_len));
    expected.push(0x00);
    expected.extend_from_slice(digest_info);
    expected.extend_from_slice(hash);
    em.len() == expected.len() && em == expected
}

/// RFC 8017 §8.2.2 `RSASSA-PKCS1-v1_5-VERIFY` with SHA-256 (= COSE **RS256**,
/// alg `-257`). Hashes `msg` with SHA-256 internally, then runs the fail-closed
/// EM compare. `n`/`e` are the public modulus / exponent as big-endian bytes
/// (exactly as they appear in a COSE_Key RSA key, a DNSKEY per RFC 3110, or an
/// X.509 SubjectPublicKeyInfo); `sig` must be exactly `n.len()` bytes. Returns
/// `true` only for a genuine signature; `false` (never a panic) otherwise.
pub fn verify_pkcs1_sha256(n: &[u8], e: &[u8], msg: &[u8], sig: &[u8]) -> bool {
    let h = sha256(msg);
    verify_pkcs1_digest(n, e, sig, &SHA256_DIGESTINFO, &h)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    // RFC 8017-style RSA-2048 PKCS#1 v1.5 / SHA-256 vector, generated with the
    // Python `cryptography` oracle and cross-checked by its own `.verify()`
    // before embedding (the same key material `athid::webauthn`'s RS256 KAT
    // uses, so a regression in either surfaces here first).
    const N: [u8; 256] = [
        0xb5, 0x59, 0x11, 0xb4, 0xe4, 0x51, 0x79, 0xb8, 0x57, 0x20, 0xd5, 0x6a, 0xe9, 0x12, 0x51,
        0x51, 0xf5, 0x8c, 0x7e, 0x0d, 0xa3, 0x16, 0x05, 0x84, 0xa3, 0x2d, 0x2b, 0x00, 0x05, 0xa0,
        0x0e, 0x61, 0x94, 0x6f, 0xa4, 0x67, 0x3e, 0xc7, 0x67, 0x9b, 0x51, 0x5e, 0xfb, 0xc6, 0xb1,
        0x4a, 0x6e, 0xf8, 0x2c, 0x31, 0xd0, 0x1a, 0x13, 0xe7, 0x0c, 0x09, 0xec, 0x6e, 0x5b, 0xbe,
        0x02, 0x1c, 0x11, 0x3b, 0xd2, 0x88, 0x1e, 0x49, 0x8d, 0x42, 0x50, 0xa8, 0x88, 0x86, 0x41,
        0x5c, 0x26, 0x23, 0x9d, 0x97, 0xaf, 0x10, 0x8f, 0xd1, 0x25, 0x26, 0xfa, 0x6f, 0xae, 0x1c,
        0xa7, 0x0b, 0x03, 0x2c, 0x0e, 0xc6, 0xe6, 0x6a, 0x86, 0xd6, 0x83, 0x9d, 0x89, 0x4a, 0x70,
        0x30, 0x28, 0x86, 0xee, 0xb7, 0x68, 0x67, 0xee, 0x50, 0x36, 0xc5, 0xfa, 0xab, 0x84, 0x46,
        0x57, 0x77, 0x2b, 0x54, 0x94, 0x97, 0x9d, 0x20, 0x65, 0x46, 0xe4, 0xe5, 0x1b, 0x17, 0xc0,
        0x3f, 0xf5, 0xb5, 0xed, 0x1d, 0xca, 0x1e, 0x92, 0xf0, 0x4f, 0x51, 0x53, 0x26, 0xa1, 0x72,
        0x13, 0x3a, 0xcb, 0xba, 0x0e, 0x38, 0x2a, 0xa0, 0x63, 0xa3, 0xf3, 0x85, 0xfe, 0xa9, 0x39,
        0x45, 0x0d, 0xe7, 0xed, 0x8c, 0xad, 0x4d, 0xae, 0xd5, 0xfa, 0xd3, 0xa2, 0x95, 0x06, 0x51,
        0x04, 0x61, 0x1e, 0x51, 0xd0, 0x39, 0xa7, 0x28, 0xff, 0x53, 0x16, 0xf3, 0x43, 0x2f, 0xc3,
        0x3e, 0xb6, 0x1d, 0x76, 0x9b, 0xee, 0x31, 0x6d, 0x06, 0x6a, 0xd7, 0xba, 0x5b, 0x82, 0xaf,
        0xa8, 0x9b, 0x60, 0xf6, 0x24, 0x40, 0xcc, 0x0b, 0x34, 0x9a, 0x60, 0xe2, 0x15, 0x99, 0xd2,
        0xbd, 0xf1, 0x77, 0x8a, 0x11, 0xf8, 0xb2, 0xe7, 0x6d, 0x46, 0xbe, 0x8a, 0x5a, 0xa8, 0xda,
        0x18, 0x42, 0xbd, 0x7c, 0x08, 0x9d, 0xd5, 0x92, 0xed, 0x06, 0x48, 0x61, 0x60, 0xea, 0x7f,
        0xed,
    ];
    const E: [u8; 3] = [0x01, 0x00, 0x01];
    // Message here is the genuine WebAuthn signed data (authData ||
    // SHA-256(clientDataJSON)) — proving this verifier accepts exactly what
    // `athid::webauthn` feeds it.
    const MSG: [u8; 69] = [
        // authData: SHA-256("athenaos.local") || flags=0x05 || signCount=5(BE)
        // followed by SHA-256(clientDataJSON) — the genuine WebAuthn signed data.
        0x69, 0xc7, 0x7f, 0x7a, 0x20, 0x11, 0xe1, 0x4f, 0x27, 0xfc, 0xc9, 0xa6, 0x40, 0x1c, 0x16,
        0xc4, 0xf4, 0x9e, 0x3f, 0x4a, 0xb1, 0xe7, 0x53, 0x1e, 0xd7, 0xd7, 0x2f, 0xc2, 0x87, 0x52,
        0x92, 0x36, 0x05, 0x00, 0x00, 0x00, 0x05, 0x78, 0xa1, 0xe1, 0xec, 0x31, 0x02, 0x69, 0x57,
        0x28, 0xa5, 0xfe, 0x82, 0x0c, 0xf3, 0x1a, 0x60, 0x6b, 0x9e, 0x4d, 0xc0, 0x4d, 0x01, 0x87,
        0x22, 0xa9, 0xea, 0xc5, 0x20, 0x7d, 0x80, 0xd2, 0xfa,
    ];
    const SIG: [u8; 256] = [
        0x1d, 0x7b, 0xaf, 0x78, 0xe7, 0x2c, 0x98, 0x3d, 0x88, 0x0f, 0x82, 0x45, 0x06, 0xb4, 0xf6,
        0x02, 0xaf, 0x99, 0xf1, 0x35, 0x2a, 0xbc, 0x48, 0x85, 0xb3, 0xf9, 0x63, 0x8d, 0x42, 0x9d,
        0x22, 0xb0, 0x8d, 0x3e, 0x73, 0xb1, 0xf3, 0xd1, 0xe9, 0xe6, 0x3e, 0x0a, 0x7d, 0x7b, 0x21,
        0x7a, 0xd4, 0x4f, 0xdd, 0xdb, 0x9d, 0xc3, 0x42, 0xf5, 0x83, 0xe0, 0xde, 0x53, 0x39, 0x2a,
        0xaf, 0xfb, 0xad, 0xa4, 0xed, 0x74, 0x2e, 0x64, 0x40, 0x6d, 0xdd, 0x7d, 0x48, 0x80, 0xfa,
        0xb1, 0x29, 0x68, 0x26, 0x93, 0x0a, 0x0d, 0x03, 0xfb, 0xa3, 0xcb, 0xea, 0x34, 0x5e, 0x19,
        0x71, 0xbf, 0x6b, 0xae, 0x22, 0x84, 0x95, 0x31, 0x31, 0x60, 0x64, 0x0a, 0x84, 0xa8, 0xd0,
        0xf5, 0x04, 0xc5, 0x6e, 0x4a, 0x40, 0xc6, 0x65, 0x8e, 0x59, 0x8c, 0xe6, 0x21, 0xf2, 0xea,
        0x59, 0x29, 0x50, 0x69, 0xfb, 0x74, 0x89, 0x57, 0x43, 0xd6, 0x2e, 0x8c, 0xda, 0xeb, 0x1b,
        0x36, 0x02, 0x05, 0x02, 0xc9, 0xda, 0xb0, 0xfb, 0x01, 0x89, 0x95, 0xb5, 0x02, 0x33, 0x56,
        0x39, 0x5e, 0xb8, 0x5c, 0xf1, 0xad, 0xf4, 0x1e, 0x37, 0x0e, 0x3b, 0x56, 0xfa, 0x5f, 0x1e,
        0xbc, 0xc2, 0x15, 0x7b, 0xd8, 0x82, 0x60, 0xf5, 0x48, 0xbb, 0xc7, 0x42, 0x19, 0x79, 0x80,
        0xd3, 0x34, 0x94, 0x68, 0x2d, 0xe4, 0x89, 0x15, 0x95, 0x4a, 0xa5, 0x80, 0x36, 0x37, 0xe5,
        0x0e, 0x35, 0x24, 0xe8, 0xeb, 0x0f, 0xb5, 0x75, 0x5b, 0x41, 0xbb, 0x37, 0x66, 0x8c, 0xfc,
        0x9f, 0xfd, 0x48, 0x16, 0xd3, 0x99, 0xe1, 0x7d, 0x79, 0xc8, 0x5f, 0xeb, 0xc7, 0x31, 0x27,
        0x9c, 0xa8, 0x39, 0x50, 0x41, 0xe4, 0x66, 0x4e, 0xe2, 0xad, 0xf9, 0x48, 0x98, 0x9a, 0x72,
        0x63, 0xee, 0xdd, 0xc3, 0x20, 0x00, 0x9f, 0xf8, 0x35, 0xa8, 0x10, 0x2a, 0x28, 0xed, 0xdc,
        0x56,
    ];

    #[test]
    fn rsa_pkcs1_sha256_valid() {
        assert!(verify_pkcs1_sha256(&N, &E, &MSG, &SIG));
    }

    #[test]
    fn rsa_pkcs1_sha256_tampered_signature_rejected() {
        let mut bad = SIG.to_vec();
        bad[100] ^= 0x01;
        assert!(!verify_pkcs1_sha256(&N, &E, &MSG, &bad));
    }

    #[test]
    fn rsa_pkcs1_sha256_tampered_message_rejected() {
        let mut bad = MSG.to_vec();
        bad[0] ^= 0x01;
        assert!(!verify_pkcs1_sha256(&N, &E, &bad, &SIG));
    }

    #[test]
    fn rsa_pkcs1_sha256_wrong_key_rejected() {
        let mut bad_n = N.to_vec();
        bad_n[0] ^= 0x01;
        assert!(!verify_pkcs1_sha256(&bad_n, &E, &MSG, &SIG));
    }

    #[test]
    fn rsa_pkcs1_sha256_malformed_never_panics() {
        assert!(!verify_pkcs1_sha256(&[], &E, &MSG, &SIG)); // empty modulus
        assert!(!verify_pkcs1_sha256(&N, &E, &MSG, &[])); // empty signature
        assert!(!verify_pkcs1_sha256(&N, &E, &MSG, &SIG[..255])); // short signature
        assert!(!verify_pkcs1_sha256(&N, &E, &MSG, &vec![0u8; 256])); // all-zero sig
        assert!(!verify_pkcs1_sha256(&N, &[], &MSG, &SIG)); // empty exponent
                                                            // A too-small modulus (can't hold a well-formed EM) fails closed.
        assert!(!verify_pkcs1_sha256(&[0xFFu8; 8], &E, &MSG, &[0u8; 8]));
    }
}
