//! ChaCha20-Poly1305 AEAD (RFC 8439), `#![no_std]`.
//!
//! ChaCha20 stream cipher + Poly1305 one-time MAC, ported faithfully from the
//! kernel's proven implementation (Poly1305 from the public-domain
//! poly1305-donna 32-bit reference). The shared, host-buildable copy so the
//! sync channel (`raesync`), TLS, and WireGuard all use one RFC 8439 §2.8.2
//! KAT-verified AEAD. `seal`/`open` are the ergonomic API; `open` verifies the
//! tag in constant time BEFORE releasing any plaintext and returns `None` on
//! any forgery.

use alloc::vec;
use alloc::vec::Vec;

// ─── ChaCha20 (RFC 8439 §2.4) ───────────────────────────────────────────────

struct ChaCha20 {
    state: [u32; 16],
}

impl ChaCha20 {
    fn new(key: &[u8; 32], nonce: &[u8; 12], counter: u32) -> Self {
        let mut state = [0u32; 16];
        state[0] = 0x61707865;
        state[1] = 0x3320646e;
        state[2] = 0x79622d32;
        state[3] = 0x6b206574;
        for i in 0..8 {
            state[4 + i] = u32::from_le_bytes(key[4 * i..4 * i + 4].try_into().unwrap());
        }
        state[12] = counter;
        for i in 0..3 {
            state[13 + i] = u32::from_le_bytes(nonce[4 * i..4 * i + 4].try_into().unwrap());
        }
        Self { state }
    }

    #[inline]
    fn quarter_round(state: &mut [u32; 16], a: usize, b: usize, c: usize, d: usize) {
        state[a] = state[a].wrapping_add(state[b]);
        state[d] ^= state[a];
        state[d] = state[d].rotate_left(16);
        state[c] = state[c].wrapping_add(state[d]);
        state[b] ^= state[c];
        state[b] = state[b].rotate_left(12);
        state[a] = state[a].wrapping_add(state[b]);
        state[d] ^= state[a];
        state[d] = state[d].rotate_left(8);
        state[c] = state[c].wrapping_add(state[d]);
        state[b] ^= state[c];
        state[b] = state[b].rotate_left(7);
    }

    fn block(&self) -> [u8; 64] {
        let mut working = self.state;
        for _ in 0..10 {
            Self::quarter_round(&mut working, 0, 4, 8, 12);
            Self::quarter_round(&mut working, 1, 5, 9, 13);
            Self::quarter_round(&mut working, 2, 6, 10, 14);
            Self::quarter_round(&mut working, 3, 7, 11, 15);
            Self::quarter_round(&mut working, 0, 5, 10, 15);
            Self::quarter_round(&mut working, 1, 6, 11, 12);
            Self::quarter_round(&mut working, 2, 7, 8, 13);
            Self::quarter_round(&mut working, 3, 4, 9, 14);
        }
        for i in 0..16 {
            working[i] = working[i].wrapping_add(self.state[i]);
        }
        let mut out = [0u8; 64];
        for i in 0..16 {
            out[4 * i..4 * i + 4].copy_from_slice(&working[i].to_le_bytes());
        }
        out
    }

    fn crypt(&mut self, data: &[u8], out: &mut [u8]) {
        let mut offset = 0;
        while offset < data.len() {
            let keystream = self.block();
            self.state[12] = self.state[12].wrapping_add(1);
            let chunk = core::cmp::min(64, data.len() - offset);
            for i in 0..chunk {
                out[offset + i] = data[offset + i] ^ keystream[i];
            }
            offset += chunk;
        }
    }
}

// ─── Poly1305 (RFC 8439 §2.5), radix-2^26 5-limb (poly1305-donna) ───────────

struct Poly1305 {
    r: [u32; 5],
    h: [u32; 5],
    pad: [u32; 4],
    leftover: usize,
    buffer: [u8; 16],
    finished: bool,
}

#[inline]
fn poly_u8to32(p: &[u8]) -> u32 {
    u32::from_le_bytes([p[0], p[1], p[2], p[3]])
}

impl Poly1305 {
    fn new(key: &[u8; 32]) -> Self {
        Self {
            r: [
                poly_u8to32(&key[0..]) & 0x3ff_ffff,
                (poly_u8to32(&key[3..]) >> 2) & 0x3ff_ff03,
                (poly_u8to32(&key[6..]) >> 4) & 0x3ff_c0ff,
                (poly_u8to32(&key[9..]) >> 6) & 0x3f0_3fff,
                (poly_u8to32(&key[12..]) >> 8) & 0x00f_ffff,
            ],
            h: [0; 5],
            pad: [
                poly_u8to32(&key[16..]),
                poly_u8to32(&key[20..]),
                poly_u8to32(&key[24..]),
                poly_u8to32(&key[28..]),
            ],
            leftover: 0,
            buffer: [0; 16],
            finished: false,
        }
    }

    fn blocks(&mut self, mut m: &[u8]) {
        let hibit: u32 = if self.finished { 0 } else { 1 << 24 };
        let (r0, r1, r2, r3, r4) = (self.r[0], self.r[1], self.r[2], self.r[3], self.r[4]);
        let (s1, s2, s3, s4) = (r1 * 5, r2 * 5, r3 * 5, r4 * 5);
        let (mut h0, mut h1, mut h2, mut h3, mut h4) =
            (self.h[0], self.h[1], self.h[2], self.h[3], self.h[4]);
        while m.len() >= 16 {
            h0 += poly_u8to32(&m[0..]) & 0x3ff_ffff;
            h1 += (poly_u8to32(&m[3..]) >> 2) & 0x3ff_ffff;
            h2 += (poly_u8to32(&m[6..]) >> 4) & 0x3ff_ffff;
            h3 += (poly_u8to32(&m[9..]) >> 6) & 0x3ff_ffff;
            h4 += (poly_u8to32(&m[12..]) >> 8) | hibit;
            let d0 = h0 as u64 * r0 as u64
                + h1 as u64 * s4 as u64
                + h2 as u64 * s3 as u64
                + h3 as u64 * s2 as u64
                + h4 as u64 * s1 as u64;
            let mut d1 = h0 as u64 * r1 as u64
                + h1 as u64 * r0 as u64
                + h2 as u64 * s4 as u64
                + h3 as u64 * s3 as u64
                + h4 as u64 * s2 as u64;
            let mut d2 = h0 as u64 * r2 as u64
                + h1 as u64 * r1 as u64
                + h2 as u64 * r0 as u64
                + h3 as u64 * s4 as u64
                + h4 as u64 * s3 as u64;
            let mut d3 = h0 as u64 * r3 as u64
                + h1 as u64 * r2 as u64
                + h2 as u64 * r1 as u64
                + h3 as u64 * r0 as u64
                + h4 as u64 * s4 as u64;
            let mut d4 = h0 as u64 * r4 as u64
                + h1 as u64 * r3 as u64
                + h2 as u64 * r2 as u64
                + h3 as u64 * r1 as u64
                + h4 as u64 * r0 as u64;
            let mut c = (d0 >> 26) as u32;
            h0 = d0 as u32 & 0x3ff_ffff;
            d1 += c as u64;
            c = (d1 >> 26) as u32;
            h1 = d1 as u32 & 0x3ff_ffff;
            d2 += c as u64;
            c = (d2 >> 26) as u32;
            h2 = d2 as u32 & 0x3ff_ffff;
            d3 += c as u64;
            c = (d3 >> 26) as u32;
            h3 = d3 as u32 & 0x3ff_ffff;
            d4 += c as u64;
            c = (d4 >> 26) as u32;
            h4 = d4 as u32 & 0x3ff_ffff;
            h0 += c * 5;
            c = h0 >> 26;
            h0 &= 0x3ff_ffff;
            h1 += c;
            m = &m[16..];
        }
        self.h = [h0, h1, h2, h3, h4];
    }

    fn update(&mut self, mut data: &[u8]) {
        if self.leftover > 0 {
            let want = core::cmp::min(16 - self.leftover, data.len());
            self.buffer[self.leftover..self.leftover + want].copy_from_slice(&data[..want]);
            self.leftover += want;
            data = &data[want..];
            if self.leftover < 16 {
                return;
            }
            let buf = self.buffer;
            self.blocks(&buf);
            self.leftover = 0;
        }
        if data.len() >= 16 {
            let n = data.len() & !15;
            let (full, rest) = data.split_at(n);
            self.blocks(full);
            data = rest;
        }
        if !data.is_empty() {
            self.buffer[..data.len()].copy_from_slice(data);
            self.leftover = data.len();
        }
    }

    fn finalize(mut self, tag: &mut [u8; 16]) {
        if self.leftover > 0 {
            let i = self.leftover;
            self.buffer[i] = 1;
            for b in self.buffer.iter_mut().take(16).skip(i + 1) {
                *b = 0;
            }
            self.finished = true;
            let buf = self.buffer;
            self.blocks(&buf);
        }
        let (mut h0, mut h1, mut h2, mut h3, mut h4) =
            (self.h[0], self.h[1], self.h[2], self.h[3], self.h[4]);
        let mut c = h1 >> 26;
        h1 &= 0x3ff_ffff;
        h2 += c;
        c = h2 >> 26;
        h2 &= 0x3ff_ffff;
        h3 += c;
        c = h3 >> 26;
        h3 &= 0x3ff_ffff;
        h4 += c;
        c = h4 >> 26;
        h4 &= 0x3ff_ffff;
        h0 += c * 5;
        c = h0 >> 26;
        h0 &= 0x3ff_ffff;
        h1 += c;
        let mut g0 = h0 + 5;
        c = g0 >> 26;
        g0 &= 0x3ff_ffff;
        let mut g1 = h1 + c;
        c = g1 >> 26;
        g1 &= 0x3ff_ffff;
        let mut g2 = h2 + c;
        c = g2 >> 26;
        g2 &= 0x3ff_ffff;
        let mut g3 = h3 + c;
        c = g3 >> 26;
        g3 &= 0x3ff_ffff;
        let g4 = (h4 + c).wrapping_sub(1 << 26);
        let mut mask = (g4 >> 31).wrapping_sub(1);
        g0 &= mask;
        g1 &= mask;
        g2 &= mask;
        g3 &= mask;
        let g4m = g4 & mask;
        mask = !mask;
        h0 = (h0 & mask) | g0;
        h1 = (h1 & mask) | g1;
        h2 = (h2 & mask) | g2;
        h3 = (h3 & mask) | g3;
        h4 = (h4 & mask) | g4m;
        let f0 = (h0 | (h1 << 26)) as u64;
        let f1 = ((h1 >> 6) | (h2 << 20)) as u64;
        let f2 = ((h2 >> 12) | (h3 << 14)) as u64;
        let f3 = ((h3 >> 18) | (h4 << 8)) as u64;
        let mut f = f0 + self.pad[0] as u64;
        let o0 = f as u32;
        f = f1 + self.pad[1] as u64 + (f >> 32);
        let o1 = f as u32;
        f = f2 + self.pad[2] as u64 + (f >> 32);
        let o2 = f as u32;
        f = f3 + self.pad[3] as u64 + (f >> 32);
        let o3 = f as u32;
        tag[0..4].copy_from_slice(&o0.to_le_bytes());
        tag[4..8].copy_from_slice(&o1.to_le_bytes());
        tag[8..12].copy_from_slice(&o2.to_le_bytes());
        tag[12..16].copy_from_slice(&o3.to_le_bytes());
    }
}

// ─── AEAD (RFC 8439 §2.8) ───────────────────────────────────────────────────

/// RFC 8439 §2.8: one-time Poly1305 key from ChaCha20 block 0, MAC over
/// `aad || pad16 || ct || pad16 || le64(aad_len) || le64(ct_len)`.
fn compute_tag(key: &[u8; 32], nonce: &[u8; 12], aad: &[u8], ct: &[u8], tag: &mut [u8; 16]) {
    let mut poly_key = [0u8; 64];
    let mut chacha = ChaCha20::new(key, nonce, 0);
    let zeros = [0u8; 64];
    chacha.crypt(&zeros, &mut poly_key);
    let mut poly = Poly1305::new(poly_key[..32].try_into().unwrap());
    let pad = [0u8; 16];
    poly.update(aad);
    if aad.len() % 16 != 0 {
        poly.update(&pad[..16 - (aad.len() % 16)]);
    }
    poly.update(ct);
    if ct.len() % 16 != 0 {
        poly.update(&pad[..16 - (ct.len() % 16)]);
    }
    let mut lens = [0u8; 16];
    lens[0..8].copy_from_slice(&(aad.len() as u64).to_le_bytes());
    lens[8..16].copy_from_slice(&(ct.len() as u64).to_le_bytes());
    poly.update(&lens);
    poly.finalize(tag);
}

/// Encrypt + authenticate: returns `ciphertext || 16-byte tag`.
pub fn seal(key: &[u8; 32], nonce: &[u8; 12], aad: &[u8], plaintext: &[u8]) -> Vec<u8> {
    let mut out = vec![0u8; plaintext.len() + 16];
    let pt_len = plaintext.len();
    let mut chacha = ChaCha20::new(key, nonce, 1);
    chacha.crypt(plaintext, &mut out[..pt_len]);
    let mut tag = [0u8; 16];
    compute_tag(key, nonce, aad, &out[..pt_len], &mut tag);
    out[pt_len..].copy_from_slice(&tag);
    out
}

/// Verify + decrypt `ciphertext || 16-byte tag`. Returns the plaintext, or
/// `None` if the tag is wrong/missing (constant-time tag check; no plaintext is
/// released on failure).
pub fn open(key: &[u8; 32], nonce: &[u8; 12], aad: &[u8], ct_and_tag: &[u8]) -> Option<Vec<u8>> {
    if ct_and_tag.len() < 16 {
        return None;
    }
    let ct_len = ct_and_tag.len() - 16;
    let ct = &ct_and_tag[..ct_len];
    let recv_tag = &ct_and_tag[ct_len..];
    let mut expected = [0u8; 16];
    compute_tag(key, nonce, aad, ct, &mut expected);
    let mut diff = 0u8;
    for i in 0..16 {
        diff |= expected[i] ^ recv_tag[i];
    }
    if diff != 0 {
        return None;
    }
    let mut out = vec![0u8; ct_len];
    let mut chacha = ChaCha20::new(key, nonce, 1);
    chacha.crypt(ct, &mut out);
    Some(out)
}

// ─── Raw primitives (for constructions beyond the RFC 8439 AEAD) ─────────────

/// Raw ChaCha20 keystream XOR (RFC 8439 §2.4 layout: 96-bit `nonce`, 32-bit
/// initial `counter`). Returns `data XOR keystream(key, nonce, counter..)`.
///
/// Exposed so callers that need the bare stream cipher — notably the SSH
/// `chacha20-poly1305@openssh.com` packet cipher, which drives ChaCha20 with an
/// explicit counter (0 for the length field + Poly1305 key, 1 for the payload)
/// and a sequence-number nonce — can build on the same KAT-verified core the
/// AEAD uses, instead of re-implementing ChaCha20.
pub fn chacha20_xor(key: &[u8; 32], nonce: &[u8; 12], counter: u32, data: &[u8]) -> Vec<u8> {
    let mut out = vec![0u8; data.len()];
    let mut chacha = ChaCha20::new(key, nonce, counter);
    chacha.crypt(data, &mut out);
    out
}

/// Raw one-shot Poly1305 MAC (RFC 8439 §2.5) over `msg` under the one-time
/// `key`. The key MUST be used for exactly one message (it is a one-time
/// authenticator); callers derive it per-message (e.g. from a ChaCha20 block).
pub fn poly1305_mac(key: &[u8; 32], msg: &[u8]) -> [u8; 16] {
    let mut poly = Poly1305::new(key);
    poly.update(msg);
    let mut tag = [0u8; 16];
    poly.finalize(&mut tag);
    tag
}

#[cfg(test)]
mod tests {
    use super::*;

    // RFC 8439 §2.8.2 AEAD known-answer test.
    const KEY: [u8; 32] = [
        0x80, 0x81, 0x82, 0x83, 0x84, 0x85, 0x86, 0x87, 0x88, 0x89, 0x8a, 0x8b, 0x8c, 0x8d, 0x8e,
        0x8f, 0x90, 0x91, 0x92, 0x93, 0x94, 0x95, 0x96, 0x97, 0x98, 0x99, 0x9a, 0x9b, 0x9c, 0x9d,
        0x9e, 0x9f,
    ];
    const NONCE: [u8; 12] = [
        0x07, 0x00, 0x00, 0x00, 0x40, 0x41, 0x42, 0x43, 0x44, 0x45, 0x46, 0x47,
    ];
    const AAD: [u8; 12] = [
        0x50, 0x51, 0x52, 0x53, 0xc0, 0xc1, 0xc2, 0xc3, 0xc4, 0xc5, 0xc6, 0xc7,
    ];
    const PLAINTEXT: &[u8] = b"Ladies and Gentlemen of the class of '99: If I could offer you only one tip for the future, sunscreen would be it.";
    const TAG: [u8; 16] = [
        0x1a, 0xe1, 0x0b, 0x59, 0x4f, 0x09, 0xe2, 0x6a, 0x7e, 0x90, 0x2e, 0xcb, 0xd0, 0x60, 0x06,
        0x91,
    ];

    #[test]
    fn rfc8439_aead_kat() {
        let sealed = seal(&KEY, &NONCE, &AAD, PLAINTEXT);
        // Tag is the last 16 bytes; must match the RFC vector.
        assert_eq!(&sealed[sealed.len() - 16..], &TAG);
        // First ciphertext bytes per RFC 8439 §2.8.2.
        assert_eq!(&sealed[..4], &[0xd3, 0x1a, 0x8d, 0x34]);
        // Round-trip.
        assert_eq!(
            open(&KEY, &NONCE, &AAD, &sealed).as_deref(),
            Some(PLAINTEXT)
        );
    }

    #[test]
    fn forgery_rejected() {
        let mut sealed = seal(&KEY, &NONCE, &AAD, PLAINTEXT);
        // Flip a ciphertext byte -> tag mismatch -> None.
        sealed[0] ^= 0x01;
        assert!(open(&KEY, &NONCE, &AAD, &sealed).is_none());
        // Flip a tag byte -> None.
        let mut s2 = seal(&KEY, &NONCE, &AAD, PLAINTEXT);
        let n = s2.len();
        s2[n - 1] ^= 0x01;
        assert!(open(&KEY, &NONCE, &AAD, &s2).is_none());
        // Wrong AAD -> None.
        let s3 = seal(&KEY, &NONCE, &AAD, PLAINTEXT);
        assert!(open(&KEY, &NONCE, b"wrong-aad", &s3).is_none());
        // Empty/too-short input -> None.
        assert!(open(&KEY, &NONCE, &AAD, &[0u8; 8]).is_none());
    }

    #[test]
    fn empty_plaintext_roundtrip() {
        let sealed = seal(&KEY, &NONCE, &AAD, b"");
        assert_eq!(sealed.len(), 16); // just the tag
        assert_eq!(open(&KEY, &NONCE, &AAD, &sealed).as_deref(), Some(&b""[..]));
    }

    // RFC 8439 §2.4.2 ChaCha20 keystream KAT (counter=1). XORing the keystream
    // against zeros yields the raw keystream block.
    #[test]
    fn rfc8439_chacha20_keystream_kat() {
        let mut key = [0u8; 32];
        for (i, b) in key.iter_mut().enumerate() {
            *b = i as u8; // 00,01,..,1f
        }
        let nonce: [u8; 12] = [
            0x00, 0x00, 0x00, 0x09, 0x00, 0x00, 0x00, 0x4a, 0x00, 0x00, 0x00, 0x00,
        ];
        let keystream = chacha20_xor(&key, &nonce, 1, &[0u8; 64]);
        let expected: [u8; 64] = [
            0x10, 0xf1, 0xe7, 0xe4, 0xd1, 0x3b, 0x59, 0x15, 0x50, 0x0f, 0xdd, 0x1f, 0xa3, 0x20,
            0x71, 0xc4, 0xc7, 0xd1, 0xf4, 0xc7, 0x33, 0xc0, 0x68, 0x03, 0x04, 0x22, 0xaa, 0x9a,
            0xc3, 0xd4, 0x6c, 0x4e, 0xd2, 0x82, 0x64, 0x46, 0x07, 0x9f, 0xaa, 0x09, 0x14, 0xc2,
            0xd7, 0x05, 0xd9, 0x8b, 0x02, 0xa2, 0xb5, 0x12, 0x9c, 0xd1, 0xde, 0x16, 0x4e, 0xb9,
            0xcb, 0xd0, 0x83, 0xe8, 0xa2, 0x50, 0x3c, 0x4e,
        ];
        assert_eq!(keystream, expected);
        // XOR is an involution: encrypting then re-XORing the same keystream
        // returns the plaintext (self-inverse property the cipher relies on).
        let pt = b"any 21-byte plaintext";
        let ct = chacha20_xor(&key, &nonce, 1, pt);
        assert_eq!(chacha20_xor(&key, &nonce, 1, &ct), pt);
    }

    // RFC 8439 §2.5.2 Poly1305 one-shot MAC KAT.
    #[test]
    fn rfc8439_poly1305_mac_kat() {
        let key: [u8; 32] = [
            0x85, 0xd6, 0xbe, 0x78, 0x57, 0x55, 0x6d, 0x33, 0x7f, 0x44, 0x52, 0xfe, 0x42, 0xd5,
            0x06, 0xa8, 0x01, 0x03, 0x80, 0x8a, 0xfb, 0x0d, 0xb2, 0xfd, 0x4a, 0xbf, 0xf6, 0xaf,
            0x41, 0x49, 0xf5, 0x1b,
        ];
        let msg = b"Cryptographic Forum Research Group";
        let tag = poly1305_mac(&key, msg);
        let expected: [u8; 16] = [
            0xa8, 0x06, 0x1d, 0xc1, 0x30, 0x51, 0x36, 0xc6, 0xc2, 0x2b, 0x8b, 0xaf, 0x0c, 0x01,
            0x27, 0xa9,
        ];
        assert_eq!(tag, expected);
    }
}
