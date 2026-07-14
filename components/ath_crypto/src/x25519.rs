//! X25519 (RFC 7748) — Diffie-Hellman over Curve25519, `#![no_std]`.
//!
//! Montgomery-ladder scalar multiplication ported from the kernel's proven
//! TweetNaCl `crypto_scalarmult`, over the GF(2^255-19) field arithmetic shared
//! with `ed25519` (`crate::field25519`). The shared key-agreement primitive for
//! secure channels (athsync, TLS 1.3, WireGuard). Validated against the RFC 7748
//! §5.2 and §6.1 known-answer vectors below.

use crate::field25519::{
    fe_add, fe_inv, fe_mul, fe_sq, fe_sub, fe_zero, pack25519, sel25519, unpack25519, GF_121665,
};

/// `scalar * point` on Curve25519 (RFC 7748). `scalar` is clamped per the RFC.
fn scalarmult(scalar: &[u8; 32], point: &[u8; 32]) -> [u8; 32] {
    let mut z = *scalar;
    z[31] = (z[31] & 127) | 64;
    z[0] &= 248;

    let x = unpack25519(point);
    let mut a = fe_zero();
    let mut b = x;
    let mut c = fe_zero();
    let mut d = fe_zero();
    a[0] = 1;
    d[0] = 1;

    for i in (0..=254).rev() {
        let r = ((z[i >> 3] >> (i & 7)) & 1) as i64;
        sel25519(&mut a, &mut b, r);
        sel25519(&mut c, &mut d, r);
        let e = fe_add(&a, &c);
        a = fe_sub(&a, &c);
        c = fe_add(&b, &d);
        b = fe_sub(&b, &d);
        d = fe_sq(&e);
        let f = fe_sq(&a);
        a = fe_mul(&c, &a);
        c = fe_mul(&b, &e);
        let e2 = fe_add(&a, &c);
        a = fe_sub(&a, &c);
        b = fe_sq(&a);
        c = fe_sub(&d, &f);
        a = fe_mul(&c, &GF_121665);
        a = fe_add(&a, &d);
        c = fe_mul(&c, &a);
        a = fe_mul(&d, &f);
        d = fe_mul(&b, &x);
        b = fe_sq(&e2);
        sel25519(&mut a, &mut b, r);
        sel25519(&mut c, &mut d, r);
    }

    let z_inv = fe_inv(&c);
    let res = fe_mul(&a, &z_inv);
    pack25519(&res)
}

/// X25519(scalar, u) — the raw RFC 7748 function.
pub fn x25519(scalar: &[u8; 32], u: &[u8; 32]) -> [u8; 32] {
    scalarmult(scalar, u)
}

/// Public key for a secret scalar: X25519(secret, 9) (basepoint u = 9).
pub fn public_key(secret: &[u8; 32]) -> [u8; 32] {
    let mut base = [0u8; 32];
    base[0] = 9;
    scalarmult(secret, &base)
}

/// Diffie-Hellman shared secret = X25519(my_secret, their_public). Both parties
/// computing this with swapped roles agree on the same 32 bytes.
pub fn diffie_hellman(my_secret: &[u8; 32], their_public: &[u8; 32]) -> [u8; 32] {
    scalarmult(my_secret, their_public)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(s: &str) -> [u8; 32] {
        let mut out = [0u8; 32];
        let b = s.as_bytes();
        for i in 0..32 {
            let hi = (b[2 * i] as char).to_digit(16).unwrap() as u8;
            let lo = (b[2 * i + 1] as char).to_digit(16).unwrap() as u8;
            out[i] = (hi << 4) | lo;
        }
        out
    }

    #[test]
    fn rfc7748_section5_2_vector1() {
        let k = h("a546e36bf0527c9d3b16154b82465edd62144c0ac1fc5a18506a2244ba449ac4");
        let u = h("e6db6867583030db3594c1a424b15f7c726624ec26b3353b10a903a6d0ab1c4c");
        let want = h("c3da55379de9c6908e94ea4df28d084f32eccf03491c71f754b4075577a28552");
        assert_eq!(x25519(&k, &u), want);
    }

    #[test]
    fn rfc7748_section6_1_diffie_hellman() {
        let alice_sk = h("77076d0a7318a57d3c16c17251b26645df4c2f87ebc0992ab177fba51db92c2a");
        let alice_pk_want = h("8520f0098930a754748b7ddcb43ef75a0dbf3a0d26381af4eba4a98eaa9b4e6a");
        let bob_sk = h("5dab087e624a8a4b79e17f8b83800ee66f3bb1292618b6fd1c2f8b27ff88e0eb");
        let bob_pk_want = h("de9edb7d7b7dc1b4d35b61c2ece435373f8343c85b78674dadfc7e146f882b4f");
        let shared_want = h("4a5d9d5ba4ce2de1728e3bf480350f25e07e21c947d19e3376f09b3c1e161742");

        let alice_pk = public_key(&alice_sk);
        let bob_pk = public_key(&bob_sk);
        assert_eq!(alice_pk, alice_pk_want);
        assert_eq!(bob_pk, bob_pk_want);

        // Both sides derive the same shared secret.
        let k_ab = diffie_hellman(&alice_sk, &bob_pk);
        let k_ba = diffie_hellman(&bob_sk, &alice_pk);
        assert_eq!(k_ab, shared_want);
        assert_eq!(k_ba, shared_want);
    }
}
