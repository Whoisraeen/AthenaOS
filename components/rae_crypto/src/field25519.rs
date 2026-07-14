//! GF(2^255-19) field arithmetic (TweetNaCl), shared by `ed25519` and
//! `x25519`. The 16-limb radix-2^16 representation + carry/reduce. Correctness
//! is regression-checked by the Ed25519 and X25519 RFC KATs in those modules.

pub(crate) type Gf = [i64; 16];

pub(crate) const GF_121665: Gf = [0xDB41, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];

#[inline]
pub(crate) fn fe_zero() -> Gf {
    [0i64; 16]
}

/// Carry-propagate + reduce mod 2^255-19 (TweetNaCl `car25519`).
pub(crate) fn car25519(o: &mut Gf) {
    for i in 0..16 {
        o[i] += 1i64 << 16;
        let c = o[i] >> 16;
        if i < 15 {
            o[i + 1] += c - 1;
        } else {
            o[0] += 38 * (c - 1);
        }
        o[i] -= c << 16;
    }
}

/// Constant-time conditional swap of `p` and `q` when `b == 1`.
pub(crate) fn sel25519(p: &mut Gf, q: &mut Gf, b: i64) {
    let c = !(b - 1);
    for i in 0..16 {
        let t = c & (p[i] ^ q[i]);
        p[i] ^= t;
        q[i] ^= t;
    }
}

pub(crate) fn unpack25519(n: &[u8; 32]) -> Gf {
    let mut o = fe_zero();
    for i in 0..16 {
        o[i] = n[2 * i] as i64 + ((n[2 * i + 1] as i64) << 8);
    }
    o[15] &= 0x7fff;
    o
}

pub(crate) fn pack25519(n: &Gf) -> [u8; 32] {
    let mut t = *n;
    car25519(&mut t);
    car25519(&mut t);
    car25519(&mut t);
    for _ in 0..2 {
        let mut m = fe_zero();
        m[0] = t[0] - 0xffed;
        for i in 1..15 {
            m[i] = t[i] - 0xffff - ((m[i - 1] >> 16) & 1);
            m[i - 1] &= 0xffff;
        }
        m[15] = t[15] - 0x7fff - ((m[14] >> 16) & 1);
        let b = (m[15] >> 16) & 1;
        m[14] &= 0xffff;
        sel25519(&mut t, &mut m, 1 - b);
    }
    let mut o = [0u8; 32];
    for i in 0..16 {
        o[2 * i] = (t[i] & 0xff) as u8;
        o[2 * i + 1] = (t[i] >> 8) as u8;
    }
    o
}

pub(crate) fn fe_add(a: &Gf, b: &Gf) -> Gf {
    let mut o = fe_zero();
    for i in 0..16 {
        o[i] = a[i] + b[i];
    }
    o
}

pub(crate) fn fe_sub(a: &Gf, b: &Gf) -> Gf {
    let mut o = fe_zero();
    for i in 0..16 {
        o[i] = a[i] - b[i];
    }
    o
}

pub(crate) fn fe_mul(a: &Gf, b: &Gf) -> Gf {
    let mut t = [0i64; 31];
    for i in 0..16 {
        for j in 0..16 {
            t[i + j] += a[i] * b[j];
        }
    }
    for i in 0..15 {
        t[i] += 38 * t[i + 16];
    }
    let mut o = fe_zero();
    o[..16].copy_from_slice(&t[..16]);
    car25519(&mut o);
    car25519(&mut o);
    o
}

pub(crate) fn fe_sq(a: &Gf) -> Gf {
    fe_mul(a, a)
}

/// Field inversion via Fermat: a^(p-2) = a^(2^255-21) (TweetNaCl `inv25519`).
pub(crate) fn fe_inv(i: &Gf) -> Gf {
    let mut c = *i;
    for a in (0..=253).rev() {
        c = fe_sq(&c);
        if a != 2 && a != 4 {
            c = fe_mul(&c, i);
        }
    }
    c
}
