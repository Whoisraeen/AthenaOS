//! SHA-256 (FIPS 180-4) + HMAC-SHA256 (RFC 2104) + HKDF-SHA256 (RFC 5869).
//!
//! The most-used primitives across RaeenOS: protocol key derivation (TLS 1.3,
//! WireGuard/Noise), signed-manifest content hashing, and integrity checks.
//! Shared here so userspace components (`raenet`, `raeupdate`, `raestore`,
//! `raeid`) get one implementation instead of each rolling its own. Validated
//! by the `#[cfg(test)]` RFC 6234 / 4231 / 5869 known-answer vectors below.

use alloc::vec::Vec;

const SHA256_IV: [u32; 8] = [
    0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
];

const SHA256_K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

/// Streaming SHA-256 context.
#[derive(Clone)]
pub struct Sha256 {
    state: [u32; 8],
    buffer: [u8; 64],
    buffer_len: usize,
    total_len: u64,
}

impl Default for Sha256 {
    fn default() -> Self {
        Self::new()
    }
}

impl Sha256 {
    pub fn new() -> Self {
        Self {
            state: SHA256_IV,
            buffer: [0u8; 64],
            buffer_len: 0,
            total_len: 0,
        }
    }

    fn compress(&mut self, block: &[u8]) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes(block[4 * i..4 * i + 4].try_into().unwrap());
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = self.state;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = h
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(SHA256_K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
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

    pub fn update(&mut self, data: &[u8]) {
        self.total_len = self.total_len.wrapping_add(data.len() as u64);
        let mut offset = 0;
        if self.buffer_len > 0 {
            let fill = 64 - self.buffer_len;
            let copy = core::cmp::min(fill, data.len());
            self.buffer[self.buffer_len..self.buffer_len + copy].copy_from_slice(&data[..copy]);
            self.buffer_len += copy;
            offset = copy;
            if self.buffer_len == 64 {
                let block = self.buffer;
                self.compress(&block);
                self.buffer_len = 0;
            }
        }
        while offset + 64 <= data.len() {
            self.compress(&data[offset..offset + 64]);
            offset += 64;
        }
        if offset < data.len() {
            let remaining = data.len() - offset;
            self.buffer[..remaining].copy_from_slice(&data[offset..]);
            self.buffer_len = remaining;
        }
    }

    pub fn finalize(mut self) -> [u8; 32] {
        let bit_len = self.total_len.wrapping_mul(8);
        self.buffer[self.buffer_len] = 0x80;
        self.buffer_len += 1;
        if self.buffer_len > 56 {
            for i in self.buffer_len..64 {
                self.buffer[i] = 0;
            }
            let block = self.buffer;
            self.compress(&block);
            self.buffer_len = 0;
        }
        for i in self.buffer_len..56 {
            self.buffer[i] = 0;
        }
        self.buffer[56..64].copy_from_slice(&bit_len.to_be_bytes());
        let block = self.buffer;
        self.compress(&block);
        let mut out = [0u8; 32];
        for (i, &word) in self.state.iter().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
        }
        out
    }
}

/// One-shot SHA-256.
pub fn sha256(input: &[u8]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(input);
    h.finalize()
}

const SHA256_BLOCK: usize = 64;

/// HMAC-SHA256 (RFC 2104). Returns a 32-byte MAC.
pub fn hmac_sha256(key: &[u8], msg: &[u8]) -> [u8; 32] {
    // Keys longer than the block size are hashed first (RFC 2104).
    let mut k = [0u8; SHA256_BLOCK];
    if key.len() > SHA256_BLOCK {
        k[..32].copy_from_slice(&sha256(key));
    } else {
        k[..key.len()].copy_from_slice(key);
    }

    let mut ipad = [0x36u8; SHA256_BLOCK];
    let mut opad = [0x5cu8; SHA256_BLOCK];
    for i in 0..SHA256_BLOCK {
        ipad[i] ^= k[i];
        opad[i] ^= k[i];
    }

    let mut inner = Sha256::new();
    inner.update(&ipad);
    inner.update(msg);
    let inner_hash = inner.finalize();

    let mut outer = Sha256::new();
    outer.update(&opad);
    outer.update(&inner_hash);
    outer.finalize()
}

/// HKDF-Extract (RFC 5869 §2.2): PRK = HMAC(salt, ikm). An empty `salt` is
/// treated as a block of zeros per the RFC.
pub fn hkdf_extract(salt: &[u8], ikm: &[u8]) -> [u8; 32] {
    if salt.is_empty() {
        hmac_sha256(&[0u8; 32], ikm)
    } else {
        hmac_sha256(salt, ikm)
    }
}

/// HKDF-Expand (RFC 5869 §2.3): fill `okm` with output keying material derived
/// from `prk` and `info`. `okm.len()` must be ≤ 255*32 = 8160 bytes.
pub fn hkdf_expand(prk: &[u8; 32], info: &[u8], okm: &mut [u8]) {
    debug_assert!(
        okm.len() <= 255 * 32,
        "HKDF-Expand length exceeds 255*HashLen"
    );
    let mut t: Vec<u8> = Vec::new();
    let mut pos = 0usize;
    let mut counter: u8 = 1;
    while pos < okm.len() {
        let mut input = Vec::with_capacity(t.len() + info.len() + 1);
        input.extend_from_slice(&t);
        input.extend_from_slice(info);
        input.push(counter);
        let block = hmac_sha256(prk, &input);
        let n = core::cmp::min(32, okm.len() - pos);
        okm[pos..pos + n].copy_from_slice(&block[..n]);
        pos += n;
        t.clear();
        t.extend_from_slice(&block);
        counter = counter.wrapping_add(1);
    }
}

/// HKDF-SHA256 (RFC 5869): Extract-then-Expand convenience wrapper.
pub fn hkdf(salt: &[u8], ikm: &[u8], info: &[u8], okm: &mut [u8]) {
    let prk = hkdf_extract(salt, ikm);
    hkdf_expand(&prk, info, okm);
}

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

    #[test]
    fn sha256_fips_vectors() {
        // FIPS 180-4 / RFC 6234.
        assert_eq!(
            hex(&sha256(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            hex(&sha256(b"")),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            hex(&sha256(
                b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"
            )),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
    }

    #[test]
    fn hmac_sha256_rfc4231() {
        // RFC 4231 Test Case 2: key="Jefe", data="what do ya want for nothing?".
        assert_eq!(
            hex(&hmac_sha256(b"Jefe", b"what do ya want for nothing?")),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
        // RFC 4231 Test Case 1: 20-byte 0x0b key, data="Hi There".
        assert_eq!(
            hex(&hmac_sha256(&[0x0b; 20], b"Hi There")),
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
        );
    }

    #[test]
    fn hkdf_sha256_rfc5869_case1() {
        // RFC 5869 Appendix A.1.
        let ikm = [0x0bu8; 22];
        let salt: [u8; 13] = [
            0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c,
        ];
        let info: [u8; 10] = [0xf0, 0xf1, 0xf2, 0xf3, 0xf4, 0xf5, 0xf6, 0xf7, 0xf8, 0xf9];

        let prk = hkdf_extract(&salt, &ikm);
        assert_eq!(
            hex(&prk),
            "077709362c2e32df0ddc3f0dc47bba6390b6c73bb50f9c3122ec844ad7c2b3e5"
        );
        let mut okm = vec![0u8; 42];
        hkdf_expand(&prk, &info, &mut okm);
        assert_eq!(
            hex(&okm),
            "3cb25f25faacd57a90434f64d0362f2a2d2d0a90cf1a5a4c5db02d56ecc4c5bf34007208d5b887185865"
        );
    }
}
