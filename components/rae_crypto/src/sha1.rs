//! SHA-1 (RFC 3174 / FIPS 180-4) — `sha1(data) -> [u8;20]`.
//!
//! ## Why a cryptographically retired hash lives in this crate
//! SHA-1 is **broken for collision resistance** (SHAttered, 2017) and MUST NOT be
//! used for digital signatures, certificate fingerprints, content-addressing, or
//! anywhere an attacker who can choose *both* inputs would profit from a collision.
//! It is present here for **exactly one reason: HOTP / TOTP authenticator-app
//! compatibility** (RFC 4226 / RFC 6238). Every Google Authenticator / Authy /
//! 1Password-style code in the world is `HMAC-SHA-1` by default, and the format is
//! frozen by interoperability — to read a user's existing 2FA secrets RaeenOS must
//! speak HMAC-SHA-1.
//!
//! Crucially, the collision weakness does **not** apply to this use: HMAC's
//! security rests on SHA-1's *pseudo-random-function* property under a secret key,
//! not on collision resistance. HMAC-SHA-1 has no practical break and remains a
//! secure MAC (see RFC 6151). The OTP truncation further discards all but ~31 bits.
//!
//! Rule for callers: use this **only** for OTP / legacy MAC compatibility. For
//! hashing or signing, use [`crate::sha256`] / [`crate::ed25519`]. Validated by the
//! RFC 3174 known-answer vectors in the `#[cfg(test)]` block below.

/// SHA-1 initial state (FIPS 180-4 §5.3.1).
const SHA1_IV: [u32; 5] = [0x67452301, 0xEFCDAB89, 0x98BADCFE, 0x10325476, 0xC3D2E1F0];

/// Streaming SHA-1 context. Mirrors the [`crate::sha256::Sha256`] shape so the two
/// hashes are interchangeable behind the generic HMAC in [`crate::hmac`].
#[derive(Clone)]
pub struct Sha1 {
    state: [u32; 5],
    buffer: [u8; 64],
    buffer_len: usize,
    total_len: u64,
}

impl Default for Sha1 {
    fn default() -> Self {
        Self::new()
    }
}

impl Sha1 {
    /// A fresh context at the SHA-1 IV.
    pub fn new() -> Self {
        Self {
            state: SHA1_IV,
            buffer: [0u8; 64],
            buffer_len: 0,
            total_len: 0,
        }
    }

    fn compress(&mut self, block: &[u8]) {
        let mut w = [0u32; 80];
        for i in 0..16 {
            w[i] = u32::from_be_bytes(block[4 * i..4 * i + 4].try_into().unwrap());
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let [mut a, mut b, mut c, mut d, mut e] = self.state;
        for (i, &wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | ((!b) & d), 0x5A827999u32),
                20..=39 => (b ^ c ^ d, 0x6ED9EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1BBCDC),
                _ => (b ^ c ^ d, 0xCA62C1D6),
            };
            let temp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(wi);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temp;
        }
        self.state[0] = self.state[0].wrapping_add(a);
        self.state[1] = self.state[1].wrapping_add(b);
        self.state[2] = self.state[2].wrapping_add(c);
        self.state[3] = self.state[3].wrapping_add(d);
        self.state[4] = self.state[4].wrapping_add(e);
    }

    /// Absorb more input. Buffers a partial block across calls.
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

    /// Pad (the `0x80` + zero + 64-bit big-endian bit length of FIPS 180-4 §5.1.1)
    /// and emit the 20-byte digest.
    pub fn finalize(mut self) -> [u8; 20] {
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
        let mut out = [0u8; 20];
        for (i, &word) in self.state.iter().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
        }
        out
    }
}

/// One-shot SHA-1. Returns the 20-byte digest. See the module note on why this
/// hash exists (OTP / legacy MAC compatibility only — never for signatures).
pub fn sha1(input: &[u8]) -> [u8; 20] {
    let mut h = Sha1::new();
    h.update(input);
    h.finalize()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(b: &[u8]) -> alloc::string::String {
        use core::fmt::Write;
        let mut s = alloc::string::String::new();
        for x in b {
            let _ = write!(s, "{:02x}", x);
        }
        s
    }

    #[test]
    fn sha1_rfc3174_vectors() {
        // RFC 3174 §7.3 / FIPS 180-4 examples. FAIL-able: tweak any expected hex.
        assert_eq!(
            hex(&sha1(b"abc")),
            "a9993e364706816aba3e25717850c26c9cd0d89d"
        );
        // Empty input.
        assert_eq!(hex(&sha1(b"")), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
        // 448-bit message (crosses into a second padded block).
        assert_eq!(
            hex(&sha1(
                b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"
            )),
            "84983e441c3bd26ebaae4aa1f95129e5e54670f1"
        );
    }

    #[test]
    fn sha1_one_million_a() {
        // FIPS 180-4 / RFC 3174 long vector: 1,000,000 'a' characters, fed in
        // chunks to exercise the streaming buffer path.
        let mut h = Sha1::new();
        let chunk = [b'a'; 1000];
        for _ in 0..1000 {
            h.update(&chunk);
        }
        assert_eq!(
            hex(&h.finalize()),
            "34aa973cd4c4daa4f61eeb2bdbad27316534016f"
        );
    }

    #[test]
    fn streaming_matches_oneshot() {
        // Splitting input across update() calls must not change the digest.
        let data = b"The quick brown fox jumps over the lazy dog";
        let one = sha1(data);
        let mut h = Sha1::new();
        h.update(&data[..10]);
        h.update(&data[10..11]);
        h.update(&data[11..]);
        assert_eq!(one, h.finalize());
        // Known value for this classic input.
        assert_eq!(hex(&one), "2fd4e1c67a2d28fced849ee1bb76e7391b93eb12");
    }
}
