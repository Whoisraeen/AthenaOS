//! # RaeHash — never-panic, `no_std` file-integrity checksums.
//!
//! RaeenOS_Concept.md §"security by default, not by friction" / "the user owns
//! the machine": when you download a kernel image, an installer, a `.raepkg`, or
//! any artifact a publisher has posted a checksum for, you should be able to
//! verify its integrity *locally*, on your own hardware, before trusting a single
//! byte of it — no network round-trip, no third party, no friction. This crate is
//! that capability: SHA-256 (the primary, modern checksum), SHA-1 and MD5 (still
//! ubiquitous in published checksums — kept *integrity-only*, see below), and
//! CRC32 (the zip/gzip frame checksum). One correct, dependency-free, hostile-byte
//! core serves the RaeStore download path, a future Files "verify checksum"
//! action, and `.raepkg` sideload verification — so it is deliberately wired into
//! none this slice.
//!
//! ## Security posture (read before you reach for SHA-1 / MD5)
//! - **SHA-256** is collision-resistant and is the algorithm a download-integrity
//!   check should use.
//! - **SHA-1** and **MD5** are **cryptographically broken** (practical collisions
//!   exist). They are provided here *only* to verify a legacy checksum a publisher
//!   already posted — i.e. to detect *accidental* corruption (a truncated download,
//!   a flipped bit). They MUST NOT be used as a security primitive (signatures,
//!   password hashing, dedup-by-hash trust). For security, use SHA-256 here or the
//!   kernel's `rae_crypto`.
//! - **CRC32** is a non-cryptographic error-detection checksum (the IEEE 802.3 /
//!   zip / gzip polynomial). Integrity-against-accident only.
//!
//! ## Never-panic, hostile-byte posture
//! There is no `unwrap`/`expect`/`panic`/raw-index-panic path reachable from any
//! public function: every digest is a fixed-size loop over `&[u8]`, padding is
//! computed arithmetically, and the streaming buffers are bounded. Streaming
//! `update()` accepts arbitrary chunk boundaries — feeding the same bytes one at a
//! time and all at once yields an identical digest (proven in the host KATs). The
//! KAT suite at the bottom of this file is the primary proof
//! (`cargo test -p rae_hash`) and uses the NIST FIPS 180-4 / RFC 1321 / IEEE
//! known-answer vectors so it can actually FAIL if the math is wrong.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

use alloc::string::String;

// ===========================================================================
// Hex helpers
// ===========================================================================

/// Encode bytes as a lowercase hexadecimal string (the familiar
/// `e3b0c44298fc1c14...` form publishers post). Never panics; allocates a
/// `String` of exactly `2 * bytes.len()` ASCII chars.
pub fn to_hex(bytes: &[u8]) -> String {
    const LUT: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(LUT[(b >> 4) as usize] as char);
        out.push(LUT[(b & 0x0f) as usize] as char);
    }
    out
}

/// The hash algorithm to use in [`verify`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Algo {
    /// SHA-256 (FIPS 180-4) — the recommended download-integrity checksum.
    Sha256,
    /// SHA-1 (FIPS 180-4) — legacy, integrity-only (broken for security).
    Sha1,
    /// MD5 (RFC 1321) — legacy, integrity-only (broken for security).
    Md5,
    /// CRC32 (IEEE / zip / gzip) — non-cryptographic error detection.
    Crc32,
}

/// Compute the lowercase-hex digest of `data` under `algo`.
pub fn hex_digest(data: &[u8], algo: Algo) -> String {
    match algo {
        Algo::Sha256 => to_hex(&sha256(data)),
        Algo::Sha1 => to_hex(&sha1(data)),
        Algo::Md5 => to_hex(&md5(data)),
        Algo::Crc32 => to_hex(&crc32(data).to_be_bytes()),
    }
}

/// Convenience: does `data`'s `algo` digest equal `expected_hex`?
///
/// The comparison is case-insensitive on the expected string (publishers post
/// either case) and tolerant of surrounding ASCII whitespace. Returns `false`
/// (never panics) for any malformed/mismatched input. This is the building block
/// a Files "verify checksum" action calls: hash the file's bytes, then compare to
/// the string the user pasted from the download page.
pub fn verify(data: &[u8], expected_hex: &str, algo: Algo) -> bool {
    let got = hex_digest(data, algo);
    let want = expected_hex.trim();
    if got.len() != want.len() {
        return false;
    }
    // ASCII case-insensitive compare; no allocation of a lowercased copy needed.
    got.bytes()
        .zip(want.bytes())
        .all(|(a, b)| a == b.to_ascii_lowercase())
}

// ===========================================================================
// SHA-256 (FIPS 180-4)
// ===========================================================================

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

const SHA256_H0: [u32; 8] = [
    0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
];

/// Streaming SHA-256 (FIPS 180-4). Feed bytes with [`Sha256::update`] across as
/// many calls as you like, then [`Sha256::finalize`] for the 32-byte digest.
#[derive(Clone)]
pub struct Sha256 {
    h: [u32; 8],
    /// 64-byte block buffer; `buf_len` bytes are valid.
    buf: [u8; 64],
    buf_len: usize,
    /// Total message length in bytes (for the length-in-bits suffix).
    total_len: u64,
}

impl Default for Sha256 {
    fn default() -> Self {
        Self::new()
    }
}

impl Sha256 {
    /// A fresh SHA-256 state primed with the FIPS 180-4 initial hash values.
    pub fn new() -> Self {
        Sha256 {
            h: SHA256_H0,
            buf: [0u8; 64],
            buf_len: 0,
            total_len: 0,
        }
    }

    /// Absorb `data`. Handles arbitrary chunk boundaries: any split of the same
    /// byte stream produces the same digest.
    pub fn update(&mut self, mut data: &[u8]) {
        self.total_len = self.total_len.wrapping_add(data.len() as u64);

        // Top up a partially-filled buffer first.
        if self.buf_len > 0 {
            let need = 64 - self.buf_len;
            let take = if data.len() < need { data.len() } else { need };
            self.buf[self.buf_len..self.buf_len + take].copy_from_slice(&data[..take]);
            self.buf_len += take;
            data = &data[take..];
            if self.buf_len == 64 {
                let block = self.buf;
                self.process(&block);
                self.buf_len = 0;
            }
        }

        // Process full 64-byte blocks straight from `data`.
        while data.len() >= 64 {
            let mut block = [0u8; 64];
            block.copy_from_slice(&data[..64]);
            self.process(&block);
            data = &data[64..];
        }

        // Stash the remainder.
        if !data.is_empty() {
            self.buf[..data.len()].copy_from_slice(data);
            self.buf_len = data.len();
        }
    }

    /// Apply FIPS 180-4 padding and return the 32-byte digest. Takes `self` by
    /// value so the one-shot path is zero-copy.
    pub fn finalize(mut self) -> [u8; 32] {
        // total_len counts only real message bytes — the padding tail below goes
        // through feed_no_count(), not update() — so the bit length is correct.
        let bit_len = self.total_len.wrapping_mul(8);

        // Append 0x80 then zeros until 56 mod 64, then the 64-bit big-endian length.
        let mut pad = [0u8; 64 + 8];
        pad[0] = 0x80;
        // How many total padding bytes (0x80 .. length) we need.
        let rem = (self.total_len % 64) as usize;
        let pad_len = if rem < 56 { 56 - rem } else { 120 - rem };
        // Write the 8-byte big-endian bit length right after the zero padding.
        pad[pad_len..pad_len + 8].copy_from_slice(&bit_len.to_be_bytes());
        self.feed_no_count(&pad[..pad_len + 8]);

        let mut out = [0u8; 32];
        for (i, word) in self.h.iter().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
        }
        out
    }

    /// Feed bytes WITHOUT updating total_len (used for the padding tail).
    fn feed_no_count(&mut self, mut data: &[u8]) {
        if self.buf_len > 0 {
            let need = 64 - self.buf_len;
            let take = if data.len() < need { data.len() } else { need };
            self.buf[self.buf_len..self.buf_len + take].copy_from_slice(&data[..take]);
            self.buf_len += take;
            data = &data[take..];
            if self.buf_len == 64 {
                let block = self.buf;
                self.process(&block);
                self.buf_len = 0;
            }
        }
        while data.len() >= 64 {
            let mut block = [0u8; 64];
            block.copy_from_slice(&data[..64]);
            self.process(&block);
            data = &data[64..];
        }
        if !data.is_empty() {
            self.buf[..data.len()].copy_from_slice(data);
            self.buf_len = data.len();
        }
    }

    fn process(&mut self, block: &[u8; 64]) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([
                block[i * 4],
                block[i * 4 + 1],
                block[i * 4 + 2],
                block[i * 4 + 3],
            ]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }

        let mut a = self.h[0];
        let mut b = self.h[1];
        let mut c = self.h[2];
        let mut d = self.h[3];
        let mut e = self.h[4];
        let mut f = self.h[5];
        let mut g = self.h[6];
        let mut hh = self.h[7];

        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let t1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(SHA256_K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);

            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }

        self.h[0] = self.h[0].wrapping_add(a);
        self.h[1] = self.h[1].wrapping_add(b);
        self.h[2] = self.h[2].wrapping_add(c);
        self.h[3] = self.h[3].wrapping_add(d);
        self.h[4] = self.h[4].wrapping_add(e);
        self.h[5] = self.h[5].wrapping_add(f);
        self.h[6] = self.h[6].wrapping_add(g);
        self.h[7] = self.h[7].wrapping_add(hh);
    }
}

/// One-shot SHA-256 of `data` → 32-byte digest.
pub fn sha256(data: &[u8]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(data);
    h.finalize()
}

// ===========================================================================
// SHA-1 (FIPS 180-4) — legacy, integrity-only.
// ===========================================================================

/// Streaming SHA-1. Legacy / integrity-only (see crate docs).
#[derive(Clone)]
pub struct Sha1 {
    h: [u32; 5],
    buf: [u8; 64],
    buf_len: usize,
    total_len: u64,
}

impl Default for Sha1 {
    fn default() -> Self {
        Self::new()
    }
}

impl Sha1 {
    pub fn new() -> Self {
        Sha1 {
            h: [0x67452301, 0xEFCDAB89, 0x98BADCFE, 0x10325476, 0xC3D2E1F0],
            buf: [0u8; 64],
            buf_len: 0,
            total_len: 0,
        }
    }

    pub fn update(&mut self, data: &[u8]) {
        self.total_len = self.total_len.wrapping_add(data.len() as u64);
        self.feed_no_count(data);
    }

    fn feed_no_count(&mut self, mut data: &[u8]) {
        if self.buf_len > 0 {
            let need = 64 - self.buf_len;
            let take = if data.len() < need { data.len() } else { need };
            self.buf[self.buf_len..self.buf_len + take].copy_from_slice(&data[..take]);
            self.buf_len += take;
            data = &data[take..];
            if self.buf_len == 64 {
                let block = self.buf;
                self.process(&block);
                self.buf_len = 0;
            }
        }
        while data.len() >= 64 {
            let mut block = [0u8; 64];
            block.copy_from_slice(&data[..64]);
            self.process(&block);
            data = &data[64..];
        }
        if !data.is_empty() {
            self.buf[..data.len()].copy_from_slice(data);
            self.buf_len = data.len();
        }
    }

    pub fn finalize(mut self) -> [u8; 20] {
        let bit_len = self.total_len.wrapping_mul(8);
        let mut pad = [0u8; 64 + 8];
        pad[0] = 0x80;
        let rem = (self.total_len % 64) as usize;
        let pad_len = if rem < 56 { 56 - rem } else { 120 - rem };
        pad[pad_len..pad_len + 8].copy_from_slice(&bit_len.to_be_bytes());
        self.feed_no_count(&pad[..pad_len + 8]);

        let mut out = [0u8; 20];
        for (i, word) in self.h.iter().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
        }
        out
    }

    fn process(&mut self, block: &[u8; 64]) {
        let mut w = [0u32; 80];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([
                block[i * 4],
                block[i * 4 + 1],
                block[i * 4 + 2],
                block[i * 4 + 3],
            ]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }

        let mut a = self.h[0];
        let mut b = self.h[1];
        let mut c = self.h[2];
        let mut d = self.h[3];
        let mut e = self.h[4];

        for (i, &wi) in w.iter().enumerate() {
            let (f, k) = if i < 20 {
                ((b & c) | ((!b) & d), 0x5A827999u32)
            } else if i < 40 {
                (b ^ c ^ d, 0x6ED9EBA1)
            } else if i < 60 {
                ((b & c) | (b & d) | (c & d), 0x8F1BBCDC)
            } else {
                (b ^ c ^ d, 0xCA62C1D6)
            };
            let tmp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(wi);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = tmp;
        }

        self.h[0] = self.h[0].wrapping_add(a);
        self.h[1] = self.h[1].wrapping_add(b);
        self.h[2] = self.h[2].wrapping_add(c);
        self.h[3] = self.h[3].wrapping_add(d);
        self.h[4] = self.h[4].wrapping_add(e);
    }
}

/// One-shot SHA-1 of `data` → 20-byte digest. Legacy / integrity-only.
pub fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h = Sha1::new();
    h.update(data);
    h.finalize()
}

// ===========================================================================
// MD5 (RFC 1321) — legacy, integrity-only.
// ===========================================================================

const MD5_S: [u32; 64] = [
    7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9,
    14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10, 15,
    21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
];

const MD5_K: [u32; 64] = [
    0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee, 0xf57c0faf, 0x4787c62a, 0xa8304613, 0xfd469501,
    0x698098d8, 0x8b44f7af, 0xffff5bb1, 0x895cd7be, 0x6b901122, 0xfd987193, 0xa679438e, 0x49b40821,
    0xf61e2562, 0xc040b340, 0x265e5a51, 0xe9b6c7aa, 0xd62f105d, 0x02441453, 0xd8a1e681, 0xe7d3fbc8,
    0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed, 0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a,
    0xfffa3942, 0x8771f681, 0x6d9d6122, 0xfde5380c, 0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70,
    0x289b7ec6, 0xeaa127fa, 0xd4ef3085, 0x04881d05, 0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665,
    0xf4292244, 0x432aff97, 0xab9423a7, 0xfc93a039, 0x655b59c3, 0x8f0ccc92, 0xffeff47d, 0x85845dd1,
    0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1, 0xf7537e82, 0xbd3af235, 0x2ad7d2bb, 0xeb86d391,
];

/// Streaming MD5. Legacy / integrity-only (see crate docs).
#[derive(Clone)]
pub struct Md5 {
    h: [u32; 4],
    buf: [u8; 64],
    buf_len: usize,
    total_len: u64,
}

impl Default for Md5 {
    fn default() -> Self {
        Self::new()
    }
}

impl Md5 {
    pub fn new() -> Self {
        Md5 {
            h: [0x67452301, 0xefcdab89, 0x98badcfe, 0x10325476],
            buf: [0u8; 64],
            buf_len: 0,
            total_len: 0,
        }
    }

    pub fn update(&mut self, data: &[u8]) {
        self.total_len = self.total_len.wrapping_add(data.len() as u64);
        self.feed_no_count(data);
    }

    fn feed_no_count(&mut self, mut data: &[u8]) {
        if self.buf_len > 0 {
            let need = 64 - self.buf_len;
            let take = if data.len() < need { data.len() } else { need };
            self.buf[self.buf_len..self.buf_len + take].copy_from_slice(&data[..take]);
            self.buf_len += take;
            data = &data[take..];
            if self.buf_len == 64 {
                let block = self.buf;
                self.process(&block);
                self.buf_len = 0;
            }
        }
        while data.len() >= 64 {
            let mut block = [0u8; 64];
            block.copy_from_slice(&data[..64]);
            self.process(&block);
            data = &data[64..];
        }
        if !data.is_empty() {
            self.buf[..data.len()].copy_from_slice(data);
            self.buf_len = data.len();
        }
    }

    pub fn finalize(mut self) -> [u8; 16] {
        // MD5 appends the length as a LITTLE-endian 64-bit bit count.
        let bit_len = self.total_len.wrapping_mul(8);
        let mut pad = [0u8; 64 + 8];
        pad[0] = 0x80;
        let rem = (self.total_len % 64) as usize;
        let pad_len = if rem < 56 { 56 - rem } else { 120 - rem };
        pad[pad_len..pad_len + 8].copy_from_slice(&bit_len.to_le_bytes());
        self.feed_no_count(&pad[..pad_len + 8]);

        let mut out = [0u8; 16];
        for (i, word) in self.h.iter().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&word.to_le_bytes());
        }
        out
    }

    fn process(&mut self, block: &[u8; 64]) {
        let mut m = [0u32; 16];
        for i in 0..16 {
            m[i] = u32::from_le_bytes([
                block[i * 4],
                block[i * 4 + 1],
                block[i * 4 + 2],
                block[i * 4 + 3],
            ]);
        }

        let mut a = self.h[0];
        let mut b = self.h[1];
        let mut c = self.h[2];
        let mut d = self.h[3];

        for i in 0..64 {
            let (f, g) = if i < 16 {
                ((b & c) | ((!b) & d), i)
            } else if i < 32 {
                ((d & b) | ((!d) & c), (5 * i + 1) % 16)
            } else if i < 48 {
                (b ^ c ^ d, (3 * i + 5) % 16)
            } else {
                (c ^ (b | (!d)), (7 * i) % 16)
            };
            let tmp = d;
            d = c;
            c = b;
            let sum = a.wrapping_add(f).wrapping_add(MD5_K[i]).wrapping_add(m[g]);
            b = b.wrapping_add(sum.rotate_left(MD5_S[i]));
            a = tmp;
        }

        self.h[0] = self.h[0].wrapping_add(a);
        self.h[1] = self.h[1].wrapping_add(b);
        self.h[2] = self.h[2].wrapping_add(c);
        self.h[3] = self.h[3].wrapping_add(d);
    }
}

/// One-shot MD5 of `data` → 16-byte digest. Legacy / integrity-only.
pub fn md5(data: &[u8]) -> [u8; 16] {
    let mut h = Md5::new();
    h.update(data);
    h.finalize()
}

// ===========================================================================
// CRC32 (IEEE 802.3 / zip / gzip)
// ===========================================================================

/// Streaming CRC-32 (IEEE polynomial 0xEDB88320, reflected; the zip/gzip one).
///
/// Computed on the fly (no 256-entry table) to stay tiny and dependency-free;
/// for file-integrity volumes this is still trivial cost.
#[derive(Clone)]
pub struct Crc32 {
    state: u32,
}

impl Default for Crc32 {
    fn default() -> Self {
        Self::new()
    }
}

impl Crc32 {
    pub fn new() -> Self {
        Crc32 { state: 0xFFFF_FFFF }
    }

    pub fn update(&mut self, data: &[u8]) {
        let mut crc = self.state;
        for &byte in data {
            crc ^= byte as u32;
            for _ in 0..8 {
                let mask = (crc & 1).wrapping_neg();
                crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
            }
        }
        self.state = crc;
    }

    pub fn finalize(self) -> u32 {
        self.state ^ 0xFFFF_FFFF
    }
}

/// One-shot CRC-32 (IEEE) of `data`.
pub fn crc32(data: &[u8]) -> u32 {
    let mut c = Crc32::new();
    c.update(data);
    c.finalize()
}

// ===========================================================================
// Host KAT suite — `cargo test -p rae_hash`. FAIL-able: each assert is a
// published NIST FIPS 180-4 / RFC 1321 / IEEE known-answer vector.
// ===========================================================================
#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    // --- SHA-256 (FIPS 180-4) ---------------------------------------------

    #[test]
    fn sha256_empty() {
        // FIPS 180-4 / NIST: SHA-256 of "".
        let got = to_hex(&sha256(b""));
        assert_eq!(
            got,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        // FAIL-ability guard: any padding/constant break moves this away from
        // the known empty-string digest.
        assert_ne!(
            got,
            "0000000000000000000000000000000000000000000000000000000000000000"
        );
    }

    #[test]
    fn sha256_abc() {
        // FIPS 180-4 Appendix B.1.
        let got = to_hex(&sha256(b"abc"));
        assert_eq!(
            got,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_ne!(got, to_hex(&sha256(b"abd")));
    }

    #[test]
    fn sha256_448_bit() {
        // FIPS 180-4 Appendix B.2: the 56-byte (448-bit) two-block message.
        let msg = b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq";
        let got = to_hex(&sha256(msg));
        assert_eq!(
            got,
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
    }

    #[test]
    fn sha256_multiblock() {
        // 1,000,000 'a' is the classic FIPS long vector; use a >1-block (>64-byte)
        // input here with a precomputed reference: 64 'a' bytes exactly = 1 block,
        // 65 'a' bytes forces the second block + padding logic.
        // Reference digests verified against an independent SHA-256.
        let got_64 = to_hex(&sha256(&[b'a'; 64]));
        assert_eq!(
            got_64,
            "ffe054fe7ae0cb6dc65c3af9b61d5209f439851db43d0ba5997337df154668eb"
        );
        let got_65 = to_hex(&sha256(&[b'a'; 65]));
        assert_eq!(
            got_65,
            "635361c48bb9eab14198e76ea8ab7f1a41685d6ad62aa9146d301d4f17eb0ae0"
        );
        assert_ne!(got_64, got_65);
    }

    #[test]
    fn sha256_streaming_equals_oneshot() {
        let data: vec::Vec<u8> = (0..200u32).map(|i| (i % 251) as u8).collect();
        let oneshot = sha256(&data);

        let mut h = Sha256::new();
        for b in &data {
            h.update(core::slice::from_ref(b)); // 1-byte chunks
        }
        let streamed = h.finalize();
        assert_eq!(oneshot, streamed);

        // Odd split sizes too.
        let mut h2 = Sha256::new();
        for chunk in data.chunks(7) {
            h2.update(chunk);
        }
        assert_eq!(oneshot, h2.finalize());
    }

    // --- SHA-1 (FIPS 180-4) -----------------------------------------------

    #[test]
    fn sha1_vectors() {
        assert_eq!(
            to_hex(&sha1(b"abc")),
            "a9993e364706816aba3e25717850c26c9cd0d89d"
        );
        assert_eq!(
            to_hex(&sha1(b"")),
            "da39a3ee5e6b4b0d3255bfef95601890afd80709"
        );
        // FIPS 180-4 second SHA-1 vector (multi-block).
        assert_eq!(
            to_hex(&sha1(
                b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"
            )),
            "84983e441c3bd26ebaae4aa1f95129e5e54670f1"
        );
    }

    #[test]
    fn sha1_streaming_equals_oneshot() {
        let data: vec::Vec<u8> = (0..130u32).map(|i| (i * 3) as u8).collect();
        let oneshot = sha1(&data);
        let mut h = Sha1::new();
        for b in &data {
            h.update(core::slice::from_ref(b));
        }
        assert_eq!(oneshot, h.finalize());
    }

    // --- MD5 (RFC 1321) ----------------------------------------------------

    #[test]
    fn md5_vectors() {
        // RFC 1321 Appendix A.5 test suite.
        assert_eq!(to_hex(&md5(b"")), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(to_hex(&md5(b"abc")), "900150983cd24fb0d6963f7d28e17f72");
        assert_eq!(
            to_hex(&md5(b"message digest")),
            "f96b697d7cb7938d525a2f31aaf161d0"
        );
        assert_eq!(
            to_hex(&md5(b"abcdefghijklmnopqrstuvwxyz")),
            "c3fcd3d76192e4007dfb496cca67e13b"
        );
    }

    #[test]
    fn md5_streaming_equals_oneshot() {
        let data: vec::Vec<u8> = (0..150u32).map(|i| (i ^ 0x5a) as u8).collect();
        let oneshot = md5(&data);
        let mut h = Md5::new();
        for chunk in data.chunks(3) {
            h.update(chunk);
        }
        assert_eq!(oneshot, h.finalize());
    }

    // --- CRC32 (IEEE) ------------------------------------------------------

    #[test]
    fn crc32_check_vector() {
        // The canonical CRC-32/ISO-HDLC "check" value over "123456789".
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0x0000_0000);
        assert_ne!(crc32(b"123456789"), crc32(b"123456780"));
    }

    #[test]
    fn crc32_streaming_equals_oneshot() {
        let data: vec::Vec<u8> = (0..300u32).map(|i| i as u8).collect();
        let oneshot = crc32(&data);
        let mut c = Crc32::new();
        for chunk in data.chunks(5) {
            c.update(chunk);
        }
        assert_eq!(oneshot, c.finalize());
    }

    // --- helpers + never-panic -------------------------------------------

    #[test]
    fn hex_lowercase_and_len() {
        assert_eq!(to_hex(&[0x00, 0x0f, 0xa0, 0xff]), "000fa0ff");
        assert_eq!(to_hex(&[]), "");
    }

    #[test]
    fn verify_convenience() {
        let data = b"abc";
        assert!(verify(
            data,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            Algo::Sha256
        ));
        // Uppercase + surrounding whitespace tolerated.
        assert!(verify(
            data,
            "  BA7816BF8F01CFEA414140DE5DAE2223B00361A396177A9CB410FF61F20015AD  ",
            Algo::Sha256
        ));
        // Wrong digest, wrong length, wrong algo all return false (never panic).
        assert!(!verify(data, "deadbeef", Algo::Sha256));
        assert!(!verify(
            data,
            "900150983cd24fb0d6963f7d28e17f72",
            Algo::Sha256
        ));
        assert!(verify(data, "900150983cd24fb0d6963f7d28e17f72", Algo::Md5));
    }

    #[test]
    fn hex_digest_dispatch() {
        assert_eq!(
            hex_digest(b"", Algo::Sha256),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            hex_digest(b"", Algo::Sha1),
            "da39a3ee5e6b4b0d3255bfef95601890afd80709"
        );
        assert_eq!(
            hex_digest(b"", Algo::Md5),
            "d41d8cd98f00b204e9800998ecf8427e"
        );
        // CRC32 of "123456789" big-endian hex.
        assert_eq!(hex_digest(b"123456789", Algo::Crc32), "cbf43926");
    }

    #[test]
    fn large_input_no_panic() {
        let big = vec![0xABu8; 1024 * 1024]; // 1 MiB
        let _ = sha256(&big);
        let _ = sha1(&big);
        let _ = md5(&big);
        let _ = crc32(&big);
        // Empty input across all four.
        let _ = sha256(b"");
        let _ = sha1(b"");
        let _ = md5(b"");
        let _ = crc32(b"");
    }
}
