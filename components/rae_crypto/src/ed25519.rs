//! Ed25519 (RFC 8032) signatures + SHA-512 (FIPS 180-4), `#![no_std]`.
//!
//! Twisted-Edwards core ported faithfully from the TweetNaCl reference — the
//! same proven implementation the kernel's `crypto::Ed25519Context` already
//! ships (RFC 8032 §7.1 KAT-verified). Carrying it here, in the shared crate,
//! means the host `raesign` tool, future `raeupdate` signature checks, and
//! RaeShield code signing all derive from ONE implementation, so signer and
//! verifier are guaranteed interoperable. Validated by the `#[cfg(test)]` RFC
//! 8032 §7.1 vectors below and the `tools/raesign` host harness.
//!
//! `sign`/`verify` are constant-time in the secret-dependent paths (TweetNaCl
//! `cswap` ladder); `pack25519`'s conditional subtractions are constant-time.

// ─── SHA-512 (FIPS 180-4) ───────────────────────────────────────────────────

const SHA512_IV: [u64; 8] = [
    0x6a09e667f3bcc908,
    0xbb67ae8584caa73b,
    0x3c6ef372fe94f82b,
    0xa54ff53a5f1d36f1,
    0x510e527fade682d1,
    0x9b05688c2b3e6c1f,
    0x1f83d9abfb41bd6b,
    0x5be0cd19137e2179,
];

const SHA512_K: [u64; 80] = [
    0x428a2f98d728ae22,
    0x7137449123ef65cd,
    0xb5c0fbcfec4d3b2f,
    0xe9b5dba58189dbbc,
    0x3956c25bf348b538,
    0x59f111f1b605d019,
    0x923f82a4af194f9b,
    0xab1c5ed5da6d8118,
    0xd807aa98a3030242,
    0x12835b0145706fbe,
    0x243185be4ee4b28c,
    0x550c7dc3d5ffb4e2,
    0x72be5d74f27b896f,
    0x80deb1fe3b1696b1,
    0x9bdc06a725c71235,
    0xc19bf174cf692694,
    0xe49b69c19ef14ad2,
    0xefbe4786384f25e3,
    0x0fc19dc68b8cd5b5,
    0x240ca1cc77ac9c65,
    0x2de92c6f592b0275,
    0x4a7484aa6ea6e483,
    0x5cb0a9dcbd41fbd4,
    0x76f988da831153b5,
    0x983e5152ee66dfab,
    0xa831c66d2db43210,
    0xb00327c898fb213f,
    0xbf597fc7beef0ee4,
    0xc6e00bf33da88fc2,
    0xd5a79147930aa725,
    0x06ca6351e003826f,
    0x142929670a0e6e70,
    0x27b70a8546d22ffc,
    0x2e1b21385c26c926,
    0x4d2c6dfc5ac42aed,
    0x53380d139d95b3df,
    0x650a73548baf63de,
    0x766a0abb3c77b2a8,
    0x81c2c92e47edaee6,
    0x92722c851482353b,
    0xa2bfe8a14cf10364,
    0xa81a664bbc423001,
    0xc24b8b70d0f89791,
    0xc76c51a30654be30,
    0xd192e819d6ef5218,
    0xd69906245565a910,
    0xf40e35855771202a,
    0x106aa07032bbd1b8,
    0x19a4c116b8d2d0c8,
    0x1e376c085141ab53,
    0x2748774cdf8eeb99,
    0x34b0bcb5e19b48a8,
    0x391c0cb3c5c95a63,
    0x4ed8aa4ae3418acb,
    0x5b9cca4f7763e373,
    0x682e6ff3d6b2b8a3,
    0x748f82ee5defb2fc,
    0x78a5636f43172f60,
    0x84c87814a1f0ab72,
    0x8cc702081a6439ec,
    0x90befffa23631e28,
    0xa4506cebde82bde9,
    0xbef9a3f7b2c67915,
    0xc67178f2e372532b,
    0xca273eceea26619c,
    0xd186b8c721c0c207,
    0xeada7dd6cde0eb1e,
    0xf57d4f7fee6ed178,
    0x06f067aa72176fba,
    0x0a637dc5a2c898a6,
    0x113f9804bef90dae,
    0x1b710b35131c471b,
    0x28db77f523047d84,
    0x32caab7b40c72493,
    0x3c9ebe0a15c9bebc,
    0x431d67c49c100d4c,
    0x4cc5d4becb3e42b6,
    0x597f299cfc657e2a,
    0x5fcb6fab3ad6faec,
    0x6c44198c4a475817,
];

struct Sha512 {
    state: [u64; 8],
    buffer: [u8; 128],
    buffer_len: usize,
    total_len: u128,
}

impl Sha512 {
    fn new() -> Self {
        Self {
            state: SHA512_IV,
            buffer: [0u8; 128],
            buffer_len: 0,
            total_len: 0,
        }
    }

    fn compress(&mut self, block: &[u8]) {
        let mut w = [0u64; 80];
        for i in 0..16 {
            w[i] = u64::from_be_bytes(block[8 * i..8 * i + 8].try_into().unwrap());
        }
        for i in 16..80 {
            let s0 = w[i - 15].rotate_right(1) ^ w[i - 15].rotate_right(8) ^ (w[i - 15] >> 7);
            let s1 = w[i - 2].rotate_right(19) ^ w[i - 2].rotate_right(61) ^ (w[i - 2] >> 6);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = self.state;
        for i in 0..80 {
            let s1 = e.rotate_right(14) ^ e.rotate_right(18) ^ e.rotate_right(41);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = h
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(SHA512_K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(28) ^ a.rotate_right(34) ^ a.rotate_right(39);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }
        self.state[0] = self.state[0].wrapping_add(a);
        self.state[1] = self.state[1].wrapping_add(b);
        self.state[2] = self.state[2].wrapping_add(c);
        self.state[3] = self.state[3].wrapping_add(d);
        self.state[4] = self.state[4].wrapping_add(e);
        self.state[5] = self.state[5].wrapping_add(f);
        self.state[6] = self.state[6].wrapping_add(g);
        self.state[7] = self.state[7].wrapping_add(h);
    }

    fn update(&mut self, data: &[u8]) {
        self.total_len += data.len() as u128;
        let mut offset = 0;
        if self.buffer_len > 0 {
            let fill = 128 - self.buffer_len;
            let copy = core::cmp::min(fill, data.len());
            self.buffer[self.buffer_len..self.buffer_len + copy].copy_from_slice(&data[..copy]);
            self.buffer_len += copy;
            offset = copy;
            if self.buffer_len == 128 {
                let block = self.buffer;
                self.compress(&block);
                self.buffer_len = 0;
            }
        }
        while offset + 128 <= data.len() {
            self.compress(&data[offset..offset + 128]);
            offset += 128;
        }
        if offset < data.len() {
            let remaining = data.len() - offset;
            self.buffer[..remaining].copy_from_slice(&data[offset..]);
            self.buffer_len = remaining;
        }
    }

    fn finalize(mut self) -> [u8; 64] {
        let bit_len = self.total_len * 8;
        self.buffer[self.buffer_len] = 0x80;
        self.buffer_len += 1;
        if self.buffer_len > 112 {
            for i in self.buffer_len..128 {
                self.buffer[i] = 0;
            }
            let block = self.buffer;
            self.compress(&block);
            self.buffer_len = 0;
        }
        for i in self.buffer_len..112 {
            self.buffer[i] = 0;
        }
        self.buffer[112..128].copy_from_slice(&(bit_len).to_be_bytes()[..16]);
        let block = self.buffer;
        self.compress(&block);
        let mut out = [0u8; 64];
        for (i, &word) in self.state.iter().enumerate() {
            out[i * 8..i * 8 + 8].copy_from_slice(&word.to_be_bytes());
        }
        out
    }
}

// ─── Ed25519 twisted-Edwards core (TweetNaCl) ───────────────────────────────
// GF(2^255-19) field arithmetic is shared with x25519 in `crate::field25519`.

use crate::field25519::{
    fe_add, fe_inv, fe_mul, fe_sq, fe_sub, fe_zero, pack25519, sel25519, unpack25519, Gf,
};

const ED_D2: Gf = [
    0xf159, 0x26b2, 0x9b94, 0xebd6, 0xb156, 0x8283, 0x149a, 0x00e0, 0xd130, 0xeef3, 0x80f2, 0x198e,
    0xfce7, 0x56df, 0xd9dc, 0x2406,
];
const ED_BX: Gf = [
    0xd51a, 0x8f25, 0x2d60, 0xc956, 0xa7b2, 0x9525, 0xc760, 0x692c, 0xdc5c, 0xfdd6, 0xe231, 0xc0a4,
    0x53fe, 0xcd6e, 0x36d3, 0x2169,
];
const ED_BY: Gf = [
    0x6658, 0x6666, 0x6666, 0x6666, 0x6666, 0x6666, 0x6666, 0x6666, 0x6666, 0x6666, 0x6666, 0x6666,
    0x6666, 0x6666, 0x6666, 0x6666,
];
const ED_DC: Gf = [
    0x78a3, 0x1359, 0x4dca, 0x75eb, 0xd8ab, 0x4141, 0x0a4d, 0x0070, 0xe898, 0x7779, 0x4079, 0x8cc7,
    0xfe73, 0x2b6f, 0x6cee, 0x5203,
];
const ED_SQRTM1: Gf = [
    0xa0b0, 0x4a0e, 0x1b27, 0xc4ee, 0xe478, 0xad2f, 0x1806, 0x2f43, 0xd7a7, 0x3dfb, 0x0099, 0x2b4d,
    0xdf0b, 0x4fc1, 0x2480, 0x2b83,
];
const ED_L: [i64; 32] = [
    0xed, 0xd3, 0xf5, 0x5c, 0x1a, 0x63, 0x12, 0x58, 0xd6, 0x9c, 0xf7, 0xa2, 0xde, 0xf9, 0xde, 0x14,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x10,
];

#[inline]
fn ed_gf1() -> Gf {
    let mut g = fe_zero();
    g[0] = 1;
    g
}

/// SHA-512-expand a 32-byte seed into (clamped scalar a, prefix) (RFC 8032).
fn expand_seed(seed: &[u8; 32]) -> ([u8; 32], [u8; 32]) {
    let mut hasher = Sha512::new();
    hasher.update(seed);
    let h = hasher.finalize();
    let mut a = [0u8; 32];
    a.copy_from_slice(&h[0..32]);
    a[0] &= 248;
    a[31] &= 127;
    a[31] |= 64;
    let mut prefix = [0u8; 32];
    prefix.copy_from_slice(&h[32..64]);
    (a, prefix)
}

fn fe_pow2523(i: &Gf) -> Gf {
    let mut c = *i;
    for a in (0..=250).rev() {
        c = fe_sq(&c);
        if a != 1 {
            c = fe_mul(&c, i);
        }
    }
    c
}

#[inline]
fn ed_par(a: &Gf) -> u8 {
    pack25519(a)[0] & 1
}
#[inline]
fn ed_neq(a: &Gf, b: &Gf) -> bool {
    pack25519(a) != pack25519(b)
}

fn ed_add(p: &mut [Gf; 4], q: &[Gf; 4]) {
    let a = fe_mul(&fe_sub(&p[1], &p[0]), &fe_sub(&q[1], &q[0]));
    let b = fe_mul(&fe_add(&p[0], &p[1]), &fe_add(&q[0], &q[1]));
    let c = fe_mul(&fe_mul(&p[3], &q[3]), &ED_D2);
    let d0 = fe_mul(&p[2], &q[2]);
    let d = fe_add(&d0, &d0);
    let e = fe_sub(&b, &a);
    let f = fe_sub(&d, &c);
    let g = fe_add(&d, &c);
    let h = fe_add(&b, &a);
    p[0] = fe_mul(&e, &f);
    p[1] = fe_mul(&h, &g);
    p[2] = fe_mul(&g, &f);
    p[3] = fe_mul(&e, &h);
}

fn ed_cswap(p: &mut [Gf; 4], q: &mut [Gf; 4], b: u8) {
    for i in 0..4 {
        sel25519(&mut p[i], &mut q[i], b as i64);
    }
}

fn ed_pack(p: &[Gf; 4]) -> [u8; 32] {
    let zi = fe_inv(&p[2]);
    let tx = fe_mul(&p[0], &zi);
    let ty = fe_mul(&p[1], &zi);
    let mut r = pack25519(&ty);
    r[31] ^= ed_par(&tx) << 7;
    r
}

fn ed_scalarmult(q: &mut [Gf; 4], s: &[u8; 32]) -> [Gf; 4] {
    let mut p: [Gf; 4] = [fe_zero(), ed_gf1(), ed_gf1(), fe_zero()];
    for i in (0..=255).rev() {
        let b = (s[i >> 3] >> (i & 7)) & 1;
        ed_cswap(&mut p, q, b);
        let pc = p;
        ed_add(q, &pc);
        let pc2 = p;
        ed_add(&mut p, &pc2);
        ed_cswap(&mut p, q, b);
    }
    p
}

fn ed_scalarbase(s: &[u8; 32]) -> [Gf; 4] {
    let mut q: [Gf; 4] = [ED_BX, ED_BY, ed_gf1(), fe_mul(&ED_BX, &ED_BY)];
    ed_scalarmult(&mut q, s)
}

fn ed_modl(r: &mut [u8; 32], x: &mut [i64; 64]) {
    for i in (32..=63).rev() {
        let mut carry = 0i64;
        let mut j = i - 32;
        while j < i - 12 {
            x[j] += carry - 16 * x[i] * ED_L[j - (i - 32)];
            carry = (x[j] + 128) >> 8;
            x[j] -= carry << 8;
            j += 1;
        }
        x[j] += carry;
        x[i] = 0;
    }
    let mut carry = 0i64;
    for j in 0..32 {
        x[j] += carry - (x[31] >> 4) * ED_L[j];
        carry = x[j] >> 8;
        x[j] &= 255;
    }
    for j in 0..32 {
        x[j] -= carry * ED_L[j];
    }
    for i in 0..32 {
        x[i + 1] += x[i] >> 8;
        r[i] = (x[i] & 255) as u8;
    }
}

fn ed_reduce64(r: &[u8; 64]) -> [u8; 32] {
    let mut x = [0i64; 64];
    for i in 0..64 {
        x[i] = r[i] as i64;
    }
    let mut out = [0u8; 32];
    ed_modl(&mut out, &mut x);
    out
}

fn ed_unpackneg(p: &[u8; 32]) -> Option<[Gf; 4]> {
    let mut r: [Gf; 4] = [fe_zero(), fe_zero(), ed_gf1(), fe_zero()];
    r[1] = unpack25519(p);
    let num0 = fe_sq(&r[1]);
    let den0 = fe_mul(&num0, &ED_DC);
    let num = fe_sub(&num0, &r[2]);
    let den = fe_add(&r[2], &den0);

    let den2 = fe_sq(&den);
    let den4 = fe_sq(&den2);
    let den6 = fe_mul(&den4, &den2);
    let mut t = fe_mul(&den6, &num);
    t = fe_mul(&t, &den);

    t = fe_pow2523(&t);
    t = fe_mul(&t, &num);
    t = fe_mul(&t, &den);
    t = fe_mul(&t, &den);
    r[0] = fe_mul(&t, &den);

    let chk = fe_mul(&fe_sq(&r[0]), &den);
    if ed_neq(&chk, &num) {
        r[0] = fe_mul(&r[0], &ED_SQRTM1);
    }
    let chk = fe_mul(&fe_sq(&r[0]), &den);
    if ed_neq(&chk, &num) {
        return None;
    }

    if ed_par(&r[0]) == (p[31] >> 7) {
        r[0] = fe_sub(&fe_zero(), &r[0]);
    }
    let r0 = r[0];
    let r1 = r[1];
    r[3] = fe_mul(&r0, &r1);
    Some(r)
}

// ─── Public API ─────────────────────────────────────────────────────────────

/// Derive the 32-byte public key A = [a]B from a 32-byte seed (RFC 8032 §5.1.5).
pub fn derive_public_key(seed: &[u8; 32]) -> [u8; 32] {
    let (a, _prefix) = expand_seed(seed);
    ed_pack(&ed_scalarbase(&a))
}

/// Produce a 64-byte detached Ed25519 signature over `msg` (RFC 8032 §5.1.6).
pub fn sign(seed: &[u8; 32], msg: &[u8]) -> [u8; 64] {
    let (a, prefix) = expand_seed(seed);
    let public_key = ed_pack(&ed_scalarbase(&a));

    // r = H(prefix || msg) mod L; R = [r]B
    let mut hr = Sha512::new();
    hr.update(&prefix);
    hr.update(msg);
    let r = ed_reduce64(&hr.finalize());
    let r_comp = ed_pack(&ed_scalarbase(&r));

    // k = H(R || A || msg) mod L
    let mut hk = Sha512::new();
    hk.update(&r_comp);
    hk.update(&public_key);
    hk.update(msg);
    let k = ed_reduce64(&hk.finalize());

    // S = (r + k*a) mod L
    let mut x = [0i64; 64];
    for i in 0..32 {
        x[i] = r[i] as i64;
    }
    for i in 0..32 {
        for j in 0..32 {
            x[i + j] += (k[i] as i64) * (a[j] as i64);
        }
    }
    let mut s = [0u8; 32];
    ed_modl(&mut s, &mut x);

    let mut sig = [0u8; 64];
    sig[0..32].copy_from_slice(&r_comp);
    sig[32..64].copy_from_slice(&s);
    sig
}

/// Verify a 64-byte detached Ed25519 signature (RFC 8032 §5.1.7). Fail-closed:
/// returns `false` for any malformed key/point or forged signature.
pub fn verify(public_key: &[u8; 32], msg: &[u8], sig: &[u8; 64]) -> bool {
    let mut neg_a = match ed_unpackneg(public_key) {
        Some(q) => q,
        None => return false,
    };

    let mut r_comp = [0u8; 32];
    r_comp.copy_from_slice(&sig[0..32]);
    let mut s = [0u8; 32];
    s.copy_from_slice(&sig[32..64]);

    // k = H(R || A || msg) mod L
    let mut hk = Sha512::new();
    hk.update(&r_comp);
    hk.update(public_key);
    hk.update(msg);
    let k = ed_reduce64(&hk.finalize());

    // p = [k](-A) + [S]B  ==  [S]B - [k]A ; accept iff it equals R.
    let mut p = ed_scalarmult(&mut neg_a, &k);
    let sb = ed_scalarbase(&s);
    ed_add(&mut p, &sb);
    let t = ed_pack(&p);

    let mut diff = 0u8;
    for i in 0..32 {
        diff |= t[i] ^ r_comp[i];
    }
    diff == 0
}

/// One-shot SHA-512 of `input` (exposed because Ed25519 ships it anyway and
/// the signing tool wants to hash payloads).
pub fn sha512(input: &[u8]) -> [u8; 64] {
    let mut h = Sha512::new();
    h.update(input);
    h.finalize()
}

#[cfg(test)]
mod tests {
    use super::*;

    // RFC 8032 §7.1 Test 1 (empty message).
    const SEED1: [u8; 32] = [
        0x9d, 0x61, 0xb1, 0x9d, 0xef, 0xfd, 0x5a, 0x60, 0xba, 0x84, 0x4a, 0xf4, 0x92, 0xec, 0x2c,
        0xc4, 0x44, 0x49, 0xc5, 0x69, 0x7b, 0x32, 0x69, 0x19, 0x70, 0x3b, 0xac, 0x03, 0x1c, 0xae,
        0x7f, 0x60,
    ];
    const PK1: [u8; 32] = [
        0xd7, 0x5a, 0x98, 0x01, 0x82, 0xb1, 0x0a, 0xb7, 0xd5, 0x4b, 0xfe, 0xd3, 0xc9, 0x64, 0x07,
        0x3a, 0x0e, 0xe1, 0x72, 0xf3, 0xda, 0xa6, 0x23, 0x25, 0xaf, 0x02, 0x1a, 0x68, 0xf7, 0x07,
        0x51, 0x1a,
    ];
    const SIG1: [u8; 64] = [
        0xe5, 0x56, 0x43, 0x00, 0xc3, 0x60, 0xac, 0x72, 0x90, 0x86, 0xe2, 0xcc, 0x80, 0x6e, 0x82,
        0x8a, 0x84, 0x87, 0x7f, 0x1e, 0xb8, 0xe5, 0xd9, 0x74, 0xd8, 0x73, 0xe0, 0x65, 0x22, 0x49,
        0x01, 0x55, 0x5f, 0xb8, 0x82, 0x15, 0x90, 0xa3, 0x3b, 0xac, 0xc6, 0x1e, 0x39, 0x70, 0x1c,
        0xf9, 0xb4, 0x6b, 0xd2, 0x5b, 0xf5, 0xf0, 0x59, 0x5b, 0xbe, 0x24, 0x65, 0x51, 0x41, 0x43,
        0x8e, 0x7a, 0x10, 0x0b,
    ];

    // RFC 8032 §7.1 Test 2 (1-byte message 0x72).
    const SEED2: [u8; 32] = [
        0x4c, 0xcd, 0x08, 0x9b, 0x28, 0xff, 0x96, 0xda, 0x9d, 0xb6, 0xc3, 0x46, 0xec, 0x11, 0x4e,
        0x0f, 0x5b, 0x8a, 0x31, 0x9f, 0x35, 0xab, 0xa6, 0x24, 0xda, 0x8c, 0xf6, 0xed, 0x4f, 0xb8,
        0xa6, 0xfb,
    ];
    const PK2: [u8; 32] = [
        0x3d, 0x40, 0x17, 0xc3, 0xe8, 0x43, 0x89, 0x5a, 0x92, 0xb7, 0x0a, 0xa7, 0x4d, 0x1b, 0x7e,
        0xbc, 0x9c, 0x98, 0x2c, 0xcf, 0x2e, 0xc4, 0x96, 0x8c, 0xc0, 0xcd, 0x55, 0xf1, 0x2a, 0xf4,
        0x66, 0x0c,
    ];
    const MSG2: [u8; 1] = [0x72];
    const SIG2: [u8; 64] = [
        0x92, 0xa0, 0x09, 0xa9, 0xf0, 0xd4, 0xca, 0xb8, 0x72, 0x0e, 0x82, 0x0b, 0x5f, 0x64, 0x25,
        0x40, 0xa2, 0xb2, 0x7b, 0x54, 0x16, 0x50, 0x3f, 0x8f, 0xb3, 0x76, 0x22, 0x23, 0xeb, 0xdb,
        0x69, 0xda, 0x08, 0x5a, 0xc1, 0xe4, 0x3e, 0x15, 0x99, 0x6e, 0x45, 0x8f, 0x36, 0x13, 0xd0,
        0xf1, 0x1d, 0x8c, 0x38, 0x7b, 0x2e, 0xae, 0xb4, 0x30, 0x2a, 0xee, 0xb0, 0x0d, 0x29, 0x16,
        0x12, 0xbb, 0x0c, 0x00,
    ];

    #[test]
    fn sha512_abc() {
        // FIPS 180-4 SHA-512("abc").
        let got = sha512(b"abc");
        let want: [u8; 64] = [
            0xdd, 0xaf, 0x35, 0xa1, 0x93, 0x61, 0x7a, 0xba, 0xcc, 0x41, 0x73, 0x49, 0xae, 0x20,
            0x41, 0x31, 0x12, 0xe6, 0xfa, 0x4e, 0x89, 0xa9, 0x7e, 0xa2, 0x0a, 0x9e, 0xee, 0xe6,
            0x4b, 0x55, 0xd3, 0x9a, 0x21, 0x92, 0x99, 0x2a, 0x27, 0x4f, 0xc1, 0xa8, 0x36, 0xba,
            0x3c, 0x23, 0xa3, 0xfe, 0xeb, 0xbd, 0x45, 0x4d, 0x44, 0x23, 0x64, 0x3c, 0xe8, 0x0e,
            0x2a, 0x9a, 0xc9, 0x4f, 0xa5, 0x4c, 0xa4, 0x9f,
        ];
        assert_eq!(got, want);
    }

    #[test]
    fn rfc8032_test1_empty_message() {
        assert_eq!(derive_public_key(&SEED1), PK1);
        assert_eq!(sign(&SEED1, &[]), SIG1);
        assert!(verify(&PK1, &[], &SIG1));
    }

    #[test]
    fn rfc8032_test2_one_byte() {
        assert_eq!(derive_public_key(&SEED2), PK2);
        assert_eq!(sign(&SEED2, &MSG2), SIG2);
        assert!(verify(&PK2, &MSG2, &SIG2));
    }

    #[test]
    fn forgery_and_tamper_rejected() {
        // Wrong message must not verify under a valid signature.
        assert!(!verify(&PK2, b"not the message", &SIG2));
        // A single flipped signature bit must be rejected.
        let mut bad = SIG2;
        bad[10] ^= 0x01;
        assert!(!verify(&PK2, &MSG2, &bad));
        // A flipped public-key bit must be rejected.
        let mut badpk = PK2;
        badpk[0] ^= 0x01;
        assert!(!verify(&badpk, &MSG2, &SIG2));
    }
}
