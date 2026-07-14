//! HMAC (RFC 2104) over the crate's hashes — `hmac_sha1` + `hmac_sha256`.
//!
//! HMAC is the keyed-MAC construction `H((K ⊕ opad) || H((K ⊕ ipad) || msg))`
//! with a 64-byte block for both SHA-1 and SHA-256. Keys longer than the block
//! are hashed first; shorter keys are zero-padded (RFC 2104 §2).
//!
//! [`hmac_sha256`] already lived in [`crate::sha256`]; it is re-exported here so
//! callers have one MAC namespace. [`hmac_sha1`] is the primitive HOTP/TOTP need
//! (see [`crate::sha1`] for why SHA-1 is present at all). These are **not**
//! constant-time in the message and are **not** intended for comparing MACs —
//! `ath_otp` does its own length-checked digit compare and rate-limits brute
//! force at a higher layer. Validated by RFC 2202 (HMAC-SHA-1) and RFC 4231
//! (HMAC-SHA-256) known-answer vectors below.

use crate::sha1::Sha1;

/// SHA-1 and SHA-256 share a 64-byte HMAC block size.
const HMAC_BLOCK: usize = 64;

/// HMAC-SHA-1 (RFC 2104). Returns a 20-byte MAC. OTP / legacy compatibility only
/// — see [`crate::sha1`]. Never panics on any key/message length.
pub fn hmac_sha1(key: &[u8], msg: &[u8]) -> [u8; 20] {
    let mut k = [0u8; HMAC_BLOCK];
    if key.len() > HMAC_BLOCK {
        // A long key is replaced by H(key) (20 bytes), then zero-padded.
        let digest = crate::sha1::sha1(key);
        k[..20].copy_from_slice(&digest);
    } else {
        k[..key.len()].copy_from_slice(key);
    }

    let mut ipad = [0x36u8; HMAC_BLOCK];
    let mut opad = [0x5cu8; HMAC_BLOCK];
    for i in 0..HMAC_BLOCK {
        ipad[i] ^= k[i];
        opad[i] ^= k[i];
    }

    let mut inner = Sha1::new();
    inner.update(&ipad);
    inner.update(msg);
    let inner_hash = inner.finalize();

    let mut outer = Sha1::new();
    outer.update(&opad);
    outer.update(&inner_hash);
    outer.finalize()
}

/// HMAC-SHA-256 (RFC 2104). Returns a 32-byte MAC. Re-exported from
/// [`crate::sha256::hmac_sha256`] so both MACs share this module.
pub use crate::sha256::hmac_sha256;

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn hex(b: &[u8]) -> alloc::string::String {
        use core::fmt::Write;
        let mut s = alloc::string::String::new();
        for x in b {
            let _ = write!(s, "{:02x}", x);
        }
        s
    }

    // ---- HMAC-SHA-1: RFC 2202 §3 test vectors ------------------------------
    // FAIL-able: tweak any expected hex below and the test turns red.

    #[test]
    fn hmac_sha1_rfc2202_case1() {
        // key = 20 bytes of 0x0b, data = "Hi There".
        assert_eq!(
            hex(&hmac_sha1(&[0x0b; 20], b"Hi There")),
            "b617318655057264e28bc0b6fb378c8ef146be00"
        );
    }

    #[test]
    fn hmac_sha1_rfc2202_case2() {
        // key = "Jefe", data = "what do ya want for nothing?".
        assert_eq!(
            hex(&hmac_sha1(b"Jefe", b"what do ya want for nothing?")),
            "effcdf6ae5eb2fa2d27416d5f184df9c259a7c79"
        );
    }

    #[test]
    fn hmac_sha1_rfc2202_case3() {
        // key = 20 bytes of 0xaa, data = 50 bytes of 0xdd.
        assert_eq!(
            hex(&hmac_sha1(&[0xaa; 20], &[0xdd; 50])),
            "125d7342b9ac11cd91a39af48aa17b4f63f175d3"
        );
    }

    #[test]
    fn hmac_sha1_rfc2202_case5() {
        // key = 25 bytes 0x01..0x19, data = 50 bytes of 0xcd.
        let key: vec::Vec<u8> = (1u8..=25).collect();
        assert_eq!(
            hex(&hmac_sha1(&key, &[0xcd; 50])),
            "4c9007f4026250c6bc8414f9bf50c86c2d7235da"
        );
    }

    #[test]
    fn hmac_sha1_rfc2202_case6_long_key() {
        // key = 80 bytes of 0xaa (> block size, so it is hashed first),
        // data = "Test Using Larger Than Block-Size Key - Hash Key First".
        assert_eq!(
            hex(&hmac_sha1(
                &[0xaa; 80],
                b"Test Using Larger Than Block-Size Key - Hash Key First"
            )),
            "aa4ae5e15272d00e95705637ce8a3b55ed402112"
        );
    }

    // ---- HMAC-SHA-256: RFC 4231 §4 test vectors ----------------------------

    #[test]
    fn hmac_sha256_rfc4231_case1() {
        assert_eq!(
            hex(&hmac_sha256(&[0x0b; 20], b"Hi There")),
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
        );
    }

    #[test]
    fn hmac_sha256_rfc4231_case2() {
        assert_eq!(
            hex(&hmac_sha256(b"Jefe", b"what do ya want for nothing?")),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
    }

    #[test]
    fn hmac_sha256_rfc4231_case6_long_key() {
        // 131-byte key (> block size).
        assert_eq!(
            hex(&hmac_sha256(
                &[0xaa; 131],
                b"Test Using Larger Than Block-Size Key - Hash Key First"
            )),
            "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"
        );
    }
}
