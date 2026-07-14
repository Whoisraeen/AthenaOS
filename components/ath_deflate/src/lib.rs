//! # RaeDeflate — a never-panic, `no_std` DEFLATE codec (RFC 1951) + gzip/zlib.
//!
//! LEGACY_GAMING_CONCEPT.md §"The user owns the machine": a daily driver must be able
//! to *produce* compressed archives, not just read them. The `ath_zip`/`ath_tar`
//! crates gave AthenaOS a from-scratch **decompressor** so someone can double-click
//! a `.zip`; this crate is the **inverse** — the canonical DEFLATE *compressor*
//! (plus a self-contained inflate), so the Files app can offer "Compress to
//! .zip/.gz" and any future writer (snapshots, logs, package tooling) can store
//! fewer bytes. DEFLATE is the body inside ZIP entries, gzip (`.gz`), and zlib
//! (`.png`/HTTP `Content-Encoding`), so one correct, dependency-free, hostile-
//! input DEFLATE core is foundational infrastructure — wired into no app this
//! slice (the Files action is the follow-up).
//!
//! ## What it does
//! - [`inflate`]: decode a raw DEFLATE stream (stored + fixed-Huffman + dynamic-
//!   Huffman blocks, the length/distance tables, overlapping back-references),
//!   bounded against a decompression bomb, never panicking on corrupt input.
//! - [`deflate`]: encode a raw DEFLATE stream with a greedy LZ77 match finder
//!   (hash-chain on 3-byte prefixes, 32 KiB window, bounded chain length) emitting
//!   literal/length-distance tokens under the RFC 1951 **fixed** Huffman codes,
//!   with a **stored-block fallback** so output is never pathologically larger
//!   than the input. The load-bearing property is `inflate(deflate(x)) == x`.
//! - [`gzip_compress`]/[`gzip_decompress`]: the gzip container (10-byte header +
//!   DEFLATE body + CRC-32 + ISIZE trailer), checksum-verified on decode.
//! - [`zlib_compress`]/[`zlib_decompress`]: the zlib container (2-byte header +
//!   DEFLATE body + Adler-32 trailer), checksum-verified on decode.
//!
//! ## Hostile-input posture
//! `inflate` and the `*_decompress` wrappers treat every byte as attacker-
//! controlled: truncated streams, bogus back-reference distances, lying length
//! codes, a corrupt header, and a checksum that doesn't match all return
//! `Err` — there is no `unwrap`/`expect`/`panic`/raw-index-panic path reachable
//! from decode. Output is capped at [`MAX_OUTPUT`] so a tiny crafted stream cannot
//! be coerced into unbounded growth.
//!
//! The host KAT suite at the bottom (`cargo test -p ath_deflate`) is the proof:
//! it round-trips `inflate(deflate(x))` over empty/short/repetitive/text/random/
//! binary inputs, asserts the compressor actually compresses, round-trips the
//! gzip and zlib wrappers with checksum verification, checks the CRC-32 and
//! Adler-32 known vectors, and feeds corrupt streams that must `Err` without panic.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

use alloc::vec;
use alloc::vec::Vec;

// ─── Limits (decompression-bomb guard) ──────────────────────────────────────

/// Largest decompressed size [`inflate`] will ever produce: 512 MiB. A crafted
/// stream that tries to expand past this returns [`InflateError::TooLarge`]
/// instead of growing the heap without bound.
pub const MAX_OUTPUT: usize = 512 * 1024 * 1024;

// ─── Errors ─────────────────────────────────────────────────────────────────

/// A decode error. Every variant is a *handled* path — nothing here panics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InflateError {
    /// The bitstream ended before the block/stream was complete.
    Truncated,
    /// A structural value was invalid: a reserved block type, a bad Huffman code,
    /// an out-of-range length/distance symbol, or a back-reference pointing before
    /// the start of output.
    Corrupt,
    /// A stored block's LEN/~NLEN consistency check failed.
    BadStored,
    /// Output would exceed [`MAX_OUTPUT`].
    TooLarge,
    /// A wrapper header (gzip magic / zlib CMF·FLG) was malformed or unsupported.
    BadHeader,
    /// The trailer checksum (gzip CRC-32 / ISIZE, or zlib Adler-32) didn't match.
    BadChecksum,
}

// ─── CRC-32 (IEEE 802.3 / zlib polynomial 0xEDB88320, reflected) ────────────

/// Compute the IEEE CRC-32 of `data` (the algorithm ZIP, gzip, and PNG use).
///
/// Reflected input/output, init `0xFFFFFFFF`, final XOR `0xFFFFFFFF`. Computed
/// bit-by-bit (no static table) to keep the crate allocation-free; it runs once
/// per compressed buffer, not on a hot path.
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg(); // 0xFFFFFFFF if LSB set, else 0
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    crc ^ 0xFFFF_FFFF
}

// ─── Adler-32 (zlib trailer; RFC 1950) ──────────────────────────────────────

/// Compute the Adler-32 checksum of `data` (the zlib stream trailer).
///
/// `s1` = 1 + sum of bytes mod 65521; `s2` = sum of running `s1` mod 65521;
/// the result is `(s2 << 16) | s1`. The modulus is applied per chunk so the
/// 32-bit sums never overflow (5552 bytes is the largest run that cannot push
/// `s2` past `u32`).
pub fn adler32(data: &[u8]) -> u32 {
    const MOD: u32 = 65521;
    let mut s1: u32 = 1;
    let mut s2: u32 = 0;
    for chunk in data.chunks(5552) {
        for &b in chunk {
            s1 += b as u32;
            s2 += s1;
        }
        s1 %= MOD;
        s2 %= MOD;
    }
    (s2 << 16) | s1
}

// ════════════════════════════════════════════════════════════════════════════
// INFLATE (decompressor) — RFC 1951, mirrored from the proven ath_zip core.
// ════════════════════════════════════════════════════════════════════════════

struct BitReader<'a> {
    data: &'a [u8],
    byte_pos: usize,
    bit_pos: u32,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            byte_pos: 0,
            bit_pos: 0,
        }
    }

    fn bit(&mut self) -> Result<u32, InflateError> {
        let byte = *self
            .data
            .get(self.byte_pos)
            .ok_or(InflateError::Truncated)?;
        let b = (byte >> self.bit_pos) & 1;
        self.bit_pos += 1;
        if self.bit_pos == 8 {
            self.bit_pos = 0;
            self.byte_pos += 1;
        }
        Ok(b as u32)
    }

    fn bits(&mut self, n: u32) -> Result<u32, InflateError> {
        let mut v = 0u32;
        for i in 0..n {
            v |= self.bit()? << i;
        }
        Ok(v)
    }

    fn align_to_byte(&mut self) {
        if self.bit_pos != 0 {
            self.bit_pos = 0;
            self.byte_pos += 1;
        }
    }

    fn read_byte(&mut self) -> Result<u8, InflateError> {
        let b = *self
            .data
            .get(self.byte_pos)
            .ok_or(InflateError::Truncated)?;
        self.byte_pos += 1;
        Ok(b)
    }
}

/// Canonical Huffman decoder built from a list of code lengths.
struct Huffman {
    counts: [u16; 16],
    symbols: Vec<u16>,
}

impl Huffman {
    fn from_lengths(lengths: &[u8]) -> Result<Self, InflateError> {
        let mut counts = [0u16; 16];
        for &l in lengths {
            if l as usize > 15 {
                return Err(InflateError::Corrupt);
            }
            counts[l as usize] += 1;
        }
        counts[0] = 0;
        let mut offsets = [0u16; 16];
        let mut sum = 0u16;
        for len in 1..16 {
            offsets[len] = sum;
            sum += counts[len];
        }
        let mut symbols = vec![0u16; sum as usize];
        for (sym, &l) in lengths.iter().enumerate() {
            if l != 0 {
                let idx = offsets[l as usize] as usize;
                if idx >= symbols.len() {
                    return Err(InflateError::Corrupt);
                }
                symbols[idx] = sym as u16;
                offsets[l as usize] += 1;
            }
        }
        Ok(Self { counts, symbols })
    }

    fn decode(&self, br: &mut BitReader) -> Result<u16, InflateError> {
        let mut code: i32 = 0;
        let mut first: i32 = 0;
        let mut index: i32 = 0;
        for len in 1..16usize {
            code |= br.bit()? as i32;
            let count = self.counts[len] as i32;
            if code - first < count {
                let sym_idx = (index + (code - first)) as usize;
                return self
                    .symbols
                    .get(sym_idx)
                    .copied()
                    .ok_or(InflateError::Corrupt);
            }
            index += count;
            first += count;
            first <<= 1;
            code <<= 1;
        }
        Err(InflateError::Corrupt)
    }
}

const LENGTH_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258,
];
const LENGTH_EXTRA: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
const DIST_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
const DIST_EXTRA: [u8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];

/// Inflate a raw DEFLATE stream (no gzip/zlib wrapper).
///
/// Returns the decompressed bytes, or `Err` on any malformed/hostile input.
/// Output is capped at [`MAX_OUTPUT`]; a stream that tries to grow past it is
/// rejected ([`InflateError::TooLarge`]) rather than allocated.
pub fn inflate(data: &[u8]) -> Result<Vec<u8>, InflateError> {
    let cap = MAX_OUTPUT;
    let mut br = BitReader::new(data);
    let mut out: Vec<u8> = Vec::new();

    loop {
        let bfinal = br.bit()?;
        let btype = br.bits(2)?;
        match btype {
            0 => inflate_stored(&mut br, &mut out, cap)?,
            1 => inflate_block(&mut br, &mut out, &fixed_litlen()?, &fixed_dist()?, cap)?,
            2 => {
                let (litlen, dist) = read_dynamic_tables(&mut br)?;
                inflate_block(&mut br, &mut out, &litlen, &dist, cap)?;
            }
            _ => return Err(InflateError::Corrupt), // btype 3 reserved
        }
        if bfinal == 1 {
            break;
        }
        if out.len() > cap {
            return Err(InflateError::TooLarge);
        }
    }
    Ok(out)
}

fn inflate_stored(br: &mut BitReader, out: &mut Vec<u8>, cap: usize) -> Result<(), InflateError> {
    br.align_to_byte();
    let len = (br.read_byte()? as u16) | ((br.read_byte()? as u16) << 8);
    let nlen = (br.read_byte()? as u16) | ((br.read_byte()? as u16) << 8);
    if len != !nlen {
        return Err(InflateError::BadStored);
    }
    if out.len() + len as usize > cap {
        return Err(InflateError::TooLarge);
    }
    for _ in 0..len {
        out.push(br.read_byte()?);
    }
    Ok(())
}

fn fixed_litlen() -> Result<Huffman, InflateError> {
    let mut lengths = [0u8; 288];
    for (i, l) in lengths.iter_mut().enumerate() {
        *l = if i < 144 {
            8
        } else if i < 256 {
            9
        } else if i < 280 {
            7
        } else {
            8
        };
    }
    Huffman::from_lengths(&lengths)
}

fn fixed_dist() -> Result<Huffman, InflateError> {
    let lengths = [5u8; 30];
    Huffman::from_lengths(&lengths)
}

fn read_dynamic_tables(br: &mut BitReader) -> Result<(Huffman, Huffman), InflateError> {
    let hlit = br.bits(5)? as usize + 257;
    let hdist = br.bits(5)? as usize + 1;
    let hclen = br.bits(4)? as usize + 4;
    if hlit > 286 || hdist > 30 {
        return Err(InflateError::Corrupt);
    }

    const ORDER: [usize; 19] = [
        16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
    ];
    let mut cl_lengths = [0u8; 19];
    for i in 0..hclen {
        cl_lengths[ORDER[i]] = br.bits(3)? as u8;
    }
    let cl_huff = Huffman::from_lengths(&cl_lengths)?;

    let total = hlit + hdist;
    let mut lengths = vec![0u8; total];
    let mut i = 0usize;
    while i < total {
        let sym = cl_huff.decode(br)?;
        match sym {
            0..=15 => {
                lengths[i] = sym as u8;
                i += 1;
            }
            16 => {
                if i == 0 {
                    return Err(InflateError::Corrupt);
                }
                let prev = lengths[i - 1];
                let repeat = br.bits(2)? as usize + 3;
                for _ in 0..repeat {
                    if i >= total {
                        return Err(InflateError::Corrupt);
                    }
                    lengths[i] = prev;
                    i += 1;
                }
            }
            17 => {
                let repeat = br.bits(3)? as usize + 3;
                for _ in 0..repeat {
                    if i >= total {
                        return Err(InflateError::Corrupt);
                    }
                    lengths[i] = 0;
                    i += 1;
                }
            }
            18 => {
                let repeat = br.bits(7)? as usize + 11;
                for _ in 0..repeat {
                    if i >= total {
                        return Err(InflateError::Corrupt);
                    }
                    lengths[i] = 0;
                    i += 1;
                }
            }
            _ => return Err(InflateError::Corrupt),
        }
    }

    let litlen = Huffman::from_lengths(&lengths[..hlit])?;
    let dist = Huffman::from_lengths(&lengths[hlit..])?;
    Ok((litlen, dist))
}

fn inflate_block(
    br: &mut BitReader,
    out: &mut Vec<u8>,
    litlen: &Huffman,
    dist: &Huffman,
    cap: usize,
) -> Result<(), InflateError> {
    loop {
        let sym = litlen.decode(br)?;
        if sym == 256 {
            return Ok(());
        } else if sym < 256 {
            if out.len() + 1 > cap {
                return Err(InflateError::TooLarge);
            }
            out.push(sym as u8);
        } else {
            let li = (sym - 257) as usize;
            if li >= LENGTH_BASE.len() {
                return Err(InflateError::Corrupt);
            }
            let length = LENGTH_BASE[li] as usize + br.bits(LENGTH_EXTRA[li] as u32)? as usize;
            let dsym = dist.decode(br)? as usize;
            if dsym >= DIST_BASE.len() {
                return Err(InflateError::Corrupt);
            }
            let distance = DIST_BASE[dsym] as usize + br.bits(DIST_EXTRA[dsym] as u32)? as usize;
            if distance == 0 || distance > out.len() {
                return Err(InflateError::Corrupt);
            }
            if out.len() + length > cap {
                return Err(InflateError::TooLarge);
            }
            let start = out.len() - distance;
            for k in 0..length {
                let b = out[start + k];
                out.push(b);
            }
        }
        if out.len() > cap {
            return Err(InflateError::TooLarge);
        }
    }
}

// ════════════════════════════════════════════════════════════════════════════
// DEFLATE (compressor) — greedy LZ77 + RFC 1951 fixed Huffman + stored fallback.
// ════════════════════════════════════════════════════════════════════════════

/// DEFLATE window: a back-reference distance may be at most 32 KiB (RFC 1951).
const WINDOW: usize = 32768;
/// The minimum match length DEFLATE can encode (length codes start at 3).
const MIN_MATCH: usize = 3;
/// The maximum match length DEFLATE can encode (length code 285 = 258 bytes).
const MAX_MATCH: usize = 258;
/// Hash-chain length cap: bounds the worst-case match-finder work per position.
const MAX_CHAIN: usize = 128;
/// Hash table size (power of two for a cheap mask). 15-bit hash → 32768 buckets.
const HASH_SIZE: usize = 1 << 15;
const HASH_MASK: usize = HASH_SIZE - 1;
/// How many input bytes we pack into one uncompressed stored block (must be
/// ≤ 65535 since the stored LEN field is 16-bit).
const STORED_CHUNK: usize = 65535;

/// LSB-first bit writer producing the packed DEFLATE bitstream.
struct BitWriter {
    out: Vec<u8>,
    bit_buf: u32,
    bit_cnt: u32,
}

impl BitWriter {
    fn new() -> Self {
        Self {
            out: Vec::new(),
            bit_buf: 0,
            bit_cnt: 0,
        }
    }

    /// Write the low `n` bits of `value`, LSB first (the order DEFLATE uses for
    /// extra bits and the block header). `n` must be ≤ 24 so the 32-bit
    /// accumulator never overflows before a flush.
    fn push_bits(&mut self, value: u32, n: u32) {
        debug_assert!(n <= 24);
        self.bit_buf |= (value & ((1u32 << n) - 1)) << self.bit_cnt;
        self.bit_cnt += n;
        while self.bit_cnt >= 8 {
            self.out.push((self.bit_buf & 0xFF) as u8);
            self.bit_buf >>= 8;
            self.bit_cnt -= 8;
        }
    }

    /// Write a Huffman code given MSB-first (Huffman codes are transmitted with
    /// their most-significant bit first, the opposite of extra bits).
    fn push_code(&mut self, code: u32, len: u32) {
        // Reverse the `len`-bit code so it lands MSB-first in the LSB-first stream.
        let mut rev = 0u32;
        for i in 0..len {
            rev |= ((code >> i) & 1) << (len - 1 - i);
        }
        self.push_bits(rev, len);
    }

    fn align_to_byte(&mut self) {
        if self.bit_cnt > 0 {
            self.out.push((self.bit_buf & 0xFF) as u8);
            self.bit_buf = 0;
            self.bit_cnt = 0;
        }
    }

    fn finish(mut self) -> Vec<u8> {
        if self.bit_cnt > 0 {
            self.out.push((self.bit_buf & 0xFF) as u8);
        }
        self.out
    }
}

// --- Fixed Huffman code tables (RFC 1951 §3.2.6) ---------------------------
//
// Literal/length codes 0..287 and distance codes 0..29 have fixed, canonical
// code lengths; the codes themselves are derived from those lengths. We compute
// the (code, len) pair for each symbol once.

/// Returns the fixed-Huffman (code, bit-length) for a literal/length symbol
/// 0..=287. Lengths: 0-143 = 8 bits, 144-255 = 9, 256-279 = 7, 280-287 = 8.
fn fixed_litlen_code(sym: u16) -> (u32, u32) {
    // Canonical codes per RFC 1951 table:
    //   144 codes of len 8 starting at 0b00110000 (0x30)
    //   112 codes of len 9 starting at 0b110010000 (0x190)
    //    24 codes of len 7 starting at 0b0000000 (0x00)
    //     8 codes of len 8 starting at 0b11000000 (0xC0)
    let s = sym as u32;
    if s <= 143 {
        (0x30 + s, 8)
    } else if s <= 255 {
        (0x190 + (s - 144), 9)
    } else if s <= 279 {
        (0x00 + (s - 256), 7)
    } else {
        (0xC0 + (s - 280), 8)
    }
}

/// Fixed-Huffman distance codes: all 5 bits, code == symbol (RFC 1951 §3.2.6).
fn fixed_dist_code(sym: u16) -> (u32, u32) {
    (sym as u32, 5)
}

/// Map a match length (3..=258) to its length symbol (257..=285), base, and
/// number of extra bits. Returns (symbol, extra_bits, extra_value).
fn length_to_symbol(len: usize) -> (u16, u32, u32) {
    // Walk the LENGTH_BASE/LENGTH_EXTRA tables; symbol = 257 + index.
    let mut idx = 0usize;
    while idx + 1 < LENGTH_BASE.len() {
        if (len as u16) < LENGTH_BASE[idx + 1] {
            break;
        }
        idx += 1;
    }
    let base = LENGTH_BASE[idx] as usize;
    let extra_bits = LENGTH_EXTRA[idx] as u32;
    let extra_val = (len - base) as u32;
    (257 + idx as u16, extra_bits, extra_val)
}

/// Map a back-reference distance (1..=32768) to its distance symbol (0..=29),
/// base, and extra bits. Returns (symbol, extra_bits, extra_value).
fn distance_to_symbol(dist: usize) -> (u16, u32, u32) {
    let mut idx = 0usize;
    while idx + 1 < DIST_BASE.len() {
        if (dist as u16) < DIST_BASE[idx + 1] {
            break;
        }
        idx += 1;
    }
    let base = DIST_BASE[idx] as usize;
    let extra_bits = DIST_EXTRA[idx] as u32;
    let extra_val = (dist - base) as u32;
    (idx as u16, extra_bits, extra_val)
}

#[inline]
fn hash3(data: &[u8], pos: usize) -> usize {
    // 3-byte rolling hash → 15-bit bucket. Multiplicative mixing.
    let a = data[pos] as usize;
    let b = data[pos + 1] as usize;
    let c = data[pos + 2] as usize;
    ((a << 10) ^ (b << 5) ^ c).wrapping_mul(2654435761) >> 17 & HASH_MASK
}

/// One DEFLATE token: a literal byte or a (length, distance) back-reference.
enum Token {
    Literal(u8),
    Match { length: usize, distance: usize },
}

/// Run the greedy LZ77 match finder over `data`, returning the token stream.
///
/// Uses a hash table keyed on 3-byte prefixes plus a `prev` chain (the classic
/// zlib hash-chain). For each position we hash the next 3 bytes, walk the chain
/// of earlier positions sharing that hash (bounded by [`MAX_CHAIN`] and the 32
/// KiB [`WINDOW`]), and keep the longest match ≥ [`MIN_MATCH`]. Greedy: once a
/// match is taken we advance past it (no lazy matching — simpler, still valid).
fn lz77_tokens(data: &[u8]) -> Vec<Token> {
    let mut tokens = Vec::new();
    let n = data.len();
    if n < MIN_MATCH {
        for &b in data {
            tokens.push(Token::Literal(b));
        }
        return tokens;
    }

    // head[hash] = most recent position with this hash, or NIL.
    // prev[pos % WINDOW] = the previous position with the same hash.
    const NIL: usize = usize::MAX;
    let mut head = vec![NIL; HASH_SIZE];
    let mut prev = vec![NIL; WINDOW];

    let mut pos = 0usize;
    while pos < n {
        // Can we even form a 3-byte hash here?
        if pos + MIN_MATCH > n {
            tokens.push(Token::Literal(data[pos]));
            pos += 1;
            continue;
        }

        let h = hash3(data, pos);
        let mut candidate = head[h];

        let mut best_len = 0usize;
        let mut best_dist = 0usize;
        let max_len = (n - pos).min(MAX_MATCH);
        let min_pos = pos.saturating_sub(WINDOW);

        let mut chain = 0usize;
        while candidate != NIL && candidate >= min_pos && chain < MAX_CHAIN {
            // Compare data[candidate..] vs data[pos..].
            // Quick reject: the byte at best_len must match to beat best_len.
            let mut len = 0usize;
            while len < max_len && data[candidate + len] == data[pos + len] {
                len += 1;
            }
            if len > best_len {
                best_len = len;
                best_dist = pos - candidate;
                if len >= max_len {
                    break;
                }
            }
            candidate = prev[candidate % WINDOW];
            chain += 1;
        }

        // Insert current position into the chain.
        prev[pos % WINDOW] = head[h];
        head[h] = pos;

        if best_len >= MIN_MATCH {
            tokens.push(Token::Match {
                length: best_len,
                distance: best_dist,
            });
            // Insert the positions we skip over so future matches can find them.
            let end = pos + best_len;
            pos += 1;
            while pos < end && pos + MIN_MATCH <= n {
                let hh = hash3(data, pos);
                prev[pos % WINDOW] = head[hh];
                head[hh] = pos;
                pos += 1;
            }
            pos = end;
        } else {
            tokens.push(Token::Literal(data[pos]));
            pos += 1;
        }
    }

    tokens
}

/// Emit the tokens for one fixed-Huffman block (BTYPE=01) into `bw`. Does NOT
/// write the block header or the end-of-block code — the caller frames the block.
fn emit_fixed_tokens(bw: &mut BitWriter, tokens: &[Token]) {
    for tok in tokens {
        match tok {
            Token::Literal(b) => {
                let (code, len) = fixed_litlen_code(*b as u16);
                bw.push_code(code, len);
            }
            Token::Match { length, distance } => {
                let (lsym, lextra_bits, lextra_val) = length_to_symbol(*length);
                let (lcode, lcode_len) = fixed_litlen_code(lsym);
                bw.push_code(lcode, lcode_len);
                if lextra_bits > 0 {
                    bw.push_bits(lextra_val, lextra_bits);
                }
                let (dsym, dextra_bits, dextra_val) = distance_to_symbol(*distance);
                let (dcode, dcode_len) = fixed_dist_code(dsym);
                bw.push_code(dcode, dcode_len);
                if dextra_bits > 0 {
                    bw.push_bits(dextra_val, dextra_bits);
                }
            }
        }
    }
}

/// Estimate the compressed bit cost of a token stream under fixed Huffman, used
/// to decide whether the stored fallback would be smaller.
fn fixed_block_bits(tokens: &[Token]) -> usize {
    let mut bits = 0usize;
    for tok in tokens {
        match tok {
            Token::Literal(b) => {
                let (_c, len) = fixed_litlen_code(*b as u16);
                bits += len as usize;
            }
            Token::Match { length, distance } => {
                let (lsym, lextra, _v) = length_to_symbol(*length);
                let (_c, lcode_len) = fixed_litlen_code(lsym);
                bits += lcode_len as usize + lextra as usize;
                let (_dsym, dextra, _dv) = distance_to_symbol(*distance);
                bits += 5 + dextra as usize; // distance codes are 5 bits fixed
            }
        }
    }
    bits + 7 // + end-of-block code (256 = 7 bits under fixed Huffman)
}

/// Write one stored (uncompressed, BTYPE=00) block for `chunk`. `final_block`
/// sets BFINAL. `chunk.len()` must be ≤ 65535.
fn write_stored_block(bw: &mut BitWriter, chunk: &[u8], final_block: bool) {
    // Block header: BFINAL (1 bit) + BTYPE=00 (2 bits).
    bw.push_bits(if final_block { 1 } else { 0 }, 1);
    bw.push_bits(0, 2);
    bw.align_to_byte();
    let len = chunk.len() as u16;
    let nlen = !len;
    bw.out.push((len & 0xFF) as u8);
    bw.out.push((len >> 8) as u8);
    bw.out.push((nlen & 0xFF) as u8);
    bw.out.push((nlen >> 8) as u8);
    bw.out.extend_from_slice(chunk);
}

/// Compress `data` into a raw DEFLATE stream.
///
/// Strategy: run greedy LZ77 over the whole input, then emit it as a single
/// fixed-Huffman block. If that block's estimated size would exceed the input
/// (incompressible data — random bytes), fall back to stored (type 0) blocks so
/// the output is never pathologically larger than `input + small overhead`. The
/// guaranteed property is `inflate(deflate(x)) == x` for all `x`.
pub fn deflate(data: &[u8]) -> Vec<u8> {
    // Empty input: a single final empty fixed block (BFINAL=1, BTYPE=01, EOB).
    if data.is_empty() {
        let mut bw = BitWriter::new();
        bw.push_bits(1, 1); // BFINAL
        bw.push_bits(1, 2); // BTYPE = 01 (fixed)
        let (eob, eob_len) = fixed_litlen_code(256);
        bw.push_code(eob, eob_len);
        return bw.finish();
    }

    let tokens = lz77_tokens(data);
    let fixed_bits = fixed_block_bits(&tokens) + 3; // + 3-bit block header
    let fixed_bytes = fixed_bits.div_ceil(8);

    // Stored cost: 5 bytes overhead per ≤64 KiB chunk + the raw bytes.
    let n_chunks = data.len().div_ceil(STORED_CHUNK).max(1);
    let stored_bytes = data.len() + n_chunks * 5;

    if fixed_bytes <= stored_bytes {
        // Fixed-Huffman block wins (the common case).
        let mut bw = BitWriter::new();
        bw.push_bits(1, 1); // BFINAL = 1 (single block)
        bw.push_bits(1, 2); // BTYPE = 01 (fixed Huffman)
        emit_fixed_tokens(&mut bw, &tokens);
        let (eob, eob_len) = fixed_litlen_code(256);
        bw.push_code(eob, eob_len);
        bw.finish()
    } else {
        // Stored fallback: incompressible → emit raw, never bloat past overhead.
        let mut bw = BitWriter::new();
        let chunks: Vec<&[u8]> = data.chunks(STORED_CHUNK).collect();
        let last = chunks.len() - 1;
        for (i, chunk) in chunks.iter().enumerate() {
            write_stored_block(&mut bw, chunk, i == last);
        }
        bw.finish()
    }
}

// ════════════════════════════════════════════════════════════════════════════
// gzip wrapper (RFC 1952): 10-byte header + DEFLATE body + CRC-32 + ISIZE.
// ════════════════════════════════════════════════════════════════════════════

/// Wrap `data` in a gzip container: a minimal 10-byte header (magic `1F 8B`,
/// method 8 = DEFLATE, no flags, no mtime, OS = 255 "unknown"), the DEFLATE
/// body, then the CRC-32 of the *uncompressed* data and the input length mod
/// 2^32 (ISIZE), both little-endian.
pub fn gzip_compress(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    out.push(0x1F); // magic
    out.push(0x8B);
    out.push(0x08); // CM = DEFLATE
    out.push(0x00); // FLG = none
    out.extend_from_slice(&0u32.to_le_bytes()); // MTIME = 0
    out.push(0x00); // XFL
    out.push(0xFF); // OS = unknown
    out.extend_from_slice(&deflate(data));
    out.extend_from_slice(&crc32(data).to_le_bytes());
    out.extend_from_slice(&((data.len() as u32).to_le_bytes()));
    out
}

/// Strip a gzip container, inflate the body, and verify the CRC-32 + ISIZE
/// trailer. Returns `Err` on a bad header, corrupt body, or a checksum/length
/// mismatch — never panics.
pub fn gzip_decompress(data: &[u8]) -> Result<Vec<u8>, InflateError> {
    if data.len() < 18 {
        // 10-byte header + 8-byte trailer minimum.
        return Err(InflateError::Truncated);
    }
    if data[0] != 0x1F || data[1] != 0x8B || data[2] != 0x08 {
        return Err(InflateError::BadHeader);
    }
    let flg = data[3];
    let mut pos = 10usize;

    // Optional fields per FLG (we emit none, but accept a well-formed input).
    if flg & 0x04 != 0 {
        // FEXTRA: 2-byte XLEN + XLEN bytes.
        let xlen = *data.get(pos).ok_or(InflateError::Truncated)? as usize
            | ((*data.get(pos + 1).ok_or(InflateError::Truncated)? as usize) << 8);
        pos += 2 + xlen;
    }
    if flg & 0x08 != 0 {
        // FNAME: NUL-terminated.
        pos = skip_cstr(data, pos)?;
    }
    if flg & 0x10 != 0 {
        // FCOMMENT: NUL-terminated.
        pos = skip_cstr(data, pos)?;
    }
    if flg & 0x02 != 0 {
        // FHCRC: 2-byte header CRC.
        pos += 2;
    }

    if pos + 8 > data.len() {
        return Err(InflateError::Truncated);
    }
    let body = data
        .get(pos..data.len() - 8)
        .ok_or(InflateError::Truncated)?;
    let out = inflate(body)?;

    let trailer = &data[data.len() - 8..];
    let want_crc = u32::from_le_bytes([trailer[0], trailer[1], trailer[2], trailer[3]]);
    let want_size = u32::from_le_bytes([trailer[4], trailer[5], trailer[6], trailer[7]]);
    if crc32(&out) != want_crc || (out.len() as u32) != want_size {
        return Err(InflateError::BadChecksum);
    }
    Ok(out)
}

fn skip_cstr(data: &[u8], mut pos: usize) -> Result<usize, InflateError> {
    loop {
        let b = *data.get(pos).ok_or(InflateError::Truncated)?;
        pos += 1;
        if b == 0 {
            return Ok(pos);
        }
    }
}

// ════════════════════════════════════════════════════════════════════════════
// zlib wrapper (RFC 1950): 2-byte header + DEFLATE body + Adler-32 trailer.
// ════════════════════════════════════════════════════════════════════════════

/// Wrap `data` in a zlib container: a 2-byte header (CMF = `0x78` = CM 8 +
/// 32 KiB window, FLG chosen so the 16-bit `CMF*256+FLG` is a multiple of 31),
/// the DEFLATE body, then the big-endian Adler-32 of the *uncompressed* data.
pub fn zlib_compress(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let cmf: u16 = 0x78; // CM=8, CINFO=7 (32K window)
                         // FLG: FCHECK fills so (cmf<<8 | flg) % 31 == 0; FLEVEL=0, FDICT=0.
    let mut flg: u16 = 0;
    let rem = (cmf << 8 | flg) % 31;
    if rem != 0 {
        flg += 31 - rem;
    }
    out.push(cmf as u8);
    out.push(flg as u8);
    out.extend_from_slice(&deflate(data));
    out.extend_from_slice(&adler32(data).to_be_bytes());
    out
}

/// Strip a zlib container, inflate the body, and verify the big-endian Adler-32
/// trailer. Returns `Err` on a bad header, corrupt body, or checksum mismatch.
pub fn zlib_decompress(data: &[u8]) -> Result<Vec<u8>, InflateError> {
    if data.len() < 6 {
        // 2-byte header + 4-byte Adler-32 minimum.
        return Err(InflateError::Truncated);
    }
    let cmf = data[0];
    let flg = data[1];
    // CM must be 8 (DEFLATE); the header checksum must be a multiple of 31;
    // a preset dictionary (FDICT) is unsupported.
    if cmf & 0x0F != 8 {
        return Err(InflateError::BadHeader);
    }
    if ((cmf as u16) << 8 | flg as u16) % 31 != 0 {
        return Err(InflateError::BadHeader);
    }
    if flg & 0x20 != 0 {
        return Err(InflateError::BadHeader); // FDICT not supported
    }

    let body = data.get(2..data.len() - 4).ok_or(InflateError::Truncated)?;
    let out = inflate(body)?;

    let trailer = &data[data.len() - 4..];
    let want = u32::from_be_bytes([trailer[0], trailer[1], trailer[2], trailer[3]]);
    if adler32(&out) != want {
        return Err(InflateError::BadChecksum);
    }
    Ok(out)
}

// ════════════════════════════════════════════════════════════════════════════
// Host KAT suite — `cargo test -p ath_deflate`. FAIL-able by construction.
// ════════════════════════════════════════════════════════════════════════════
#[cfg(test)]
mod tests {
    // Under `#[cfg(test)]` the crate compiles as `std` (the `no_std` attribute is
    // `cfg_attr(not(test), ...)`), so `Vec`/`vec!` are in scope via the default
    // prelude — no `extern crate std` / `use std::` (the architecture gate bans
    // those std-ism lines).
    use super::*;

    // ── Test corpora ───────────────────────────────────────────────────────

    fn english_blob() -> Vec<u8> {
        let s = "The quick brown fox jumps over the lazy dog. \
                 Pack my box with five dozen liquor jugs. \
                 The five boxing wizards jump quickly. \
                 How vexingly quick daft zebras jump! \
                 Sphinx of black quartz, judge my vow. ";
        let mut v = Vec::new();
        for _ in 0..20 {
            v.extend_from_slice(s.as_bytes());
        }
        v
    }

    fn pseudo_random(n: usize) -> Vec<u8> {
        // A simple LCG — deterministic, statistically incompressible enough that
        // the stored fallback must engage.
        let mut state: u64 = 0x1234_5678_9ABC_DEF0;
        let mut v = Vec::with_capacity(n);
        for _ in 0..n {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            v.push((state >> 33) as u8);
        }
        v
    }

    fn binary_with_backrefs() -> Vec<u8> {
        // A block of structured binary data with long repeated runs and patterns
        // → many LZ77 back-references at varied distances.
        let mut v = Vec::new();
        for i in 0..256u32 {
            v.extend_from_slice(&i.to_le_bytes());
        }
        let snapshot = v.clone();
        // Repeat the whole prefix a few times (large-distance matches).
        for _ in 0..4 {
            v.extend_from_slice(&snapshot);
        }
        // Interleave a short repeating pattern (small-distance matches).
        for _ in 0..500 {
            v.extend_from_slice(b"ABCD");
        }
        v
    }

    fn roundtrip(input: &[u8]) {
        let comp = deflate(input);
        let back = inflate(&comp).expect("inflate(deflate(x)) must succeed");
        assert_eq!(back, input, "inflate(deflate(x)) != x");
    }

    // ── THE load-bearing guard: inflate(deflate(x)) == x ───────────────────

    #[test]
    fn roundtrip_empty() {
        roundtrip(b"");
    }

    #[test]
    fn roundtrip_short() {
        roundtrip(b"hi");
        roundtrip(b"a");
        roundtrip(b"Hello, AthenaOS!");
    }

    #[test]
    fn roundtrip_repetitive_compresses_well() {
        let input = vec![b'a'; 1000];
        let comp = deflate(&input);
        let back = inflate(&comp).expect("inflate");
        assert_eq!(back, input);
        // "aaaa…"×1000 must compress dramatically (one literal + back-references).
        assert!(
            comp.len() < input.len() / 10,
            "repetitive input should compress >10x, got {} -> {}",
            input.len(),
            comp.len()
        );
    }

    #[test]
    fn roundtrip_english_text() {
        let input = english_blob();
        let comp = deflate(&input);
        let back = inflate(&comp).expect("inflate");
        assert_eq!(back, input);
        // English text with repeated pangrams must compress materially.
        assert!(
            comp.len() < input.len() / 2,
            "text should compress >2x, got {} -> {}",
            input.len(),
            comp.len()
        );
    }

    #[test]
    fn roundtrip_random_uses_stored_fallback() {
        let input = pseudo_random(4096);
        let comp = deflate(&input);
        let back = inflate(&comp).expect("inflate");
        assert_eq!(back, input);
        // Incompressible: the stored fallback bounds the blow-up to a few bytes of
        // block overhead (5 bytes per ≤64 KiB chunk).
        assert!(
            comp.len() <= input.len() + 16,
            "random input must not bloat past stored overhead, {} -> {}",
            input.len(),
            comp.len()
        );
    }

    #[test]
    fn roundtrip_binary_backrefs() {
        let input = binary_with_backrefs();
        let comp = deflate(&input);
        let back = inflate(&comp).expect("inflate");
        assert_eq!(back, input);
        // Heavy repetition → strong compression, exercising varied match distances.
        assert!(
            comp.len() < input.len() / 4,
            "structured binary should compress >4x, {} -> {}",
            input.len(),
            comp.len()
        );
    }

    #[test]
    fn roundtrip_all_byte_values() {
        // Every byte 0..=255, repeated, to exercise all literal codes + matches.
        let mut input = Vec::new();
        for _ in 0..8 {
            for b in 0u16..256 {
                input.push(b as u8);
            }
        }
        roundtrip(&input);
    }

    #[test]
    fn roundtrip_max_match_length() {
        // A 258+ run hits the maximum match length (length code 285) repeatedly.
        let input = vec![0x5Au8; 1024];
        roundtrip(&input);
    }

    // ── Compression actually compresses (ratio assertion) ──────────────────

    #[test]
    fn compression_ratio_repetitive() {
        let input = vec![b'X'; 10000];
        let comp = deflate(&input);
        let ratio = input.len() as f64 / comp.len() as f64;
        assert!(
            ratio > 50.0,
            "10000 identical bytes should compress >50x, got {ratio:.1}x ({} -> {})",
            input.len(),
            comp.len()
        );
        assert_eq!(inflate(&comp).unwrap(), input);
    }

    // ── gzip wrapper round-trip + CRC/ISIZE verification ───────────────────

    #[test]
    fn gzip_roundtrip() {
        for input in [
            b"".as_slice(),
            b"gzip me please".as_slice(),
            &vec![b'q'; 5000],
            &english_blob(),
            &pseudo_random(2048),
        ] {
            let g = gzip_compress(input);
            // Header sanity: magic + DEFLATE method.
            assert_eq!(&g[0..3], &[0x1F, 0x8B, 0x08]);
            let back = gzip_decompress(&g).expect("gzip_decompress");
            assert_eq!(back, input);
        }
    }

    #[test]
    fn gzip_rejects_bad_crc() {
        let input = b"checksum-protected gzip payload".as_slice();
        let mut g = gzip_compress(input);
        let len = g.len();
        // Corrupt the CRC-32 (4 bytes before ISIZE).
        g[len - 8] ^= 0xFF;
        assert_eq!(gzip_decompress(&g), Err(InflateError::BadChecksum));
    }

    #[test]
    fn gzip_rejects_bad_magic() {
        let mut g = gzip_compress(b"x");
        g[0] = 0x00;
        assert_eq!(gzip_decompress(&g), Err(InflateError::BadHeader));
    }

    // ── zlib wrapper round-trip + Adler-32 verification ────────────────────

    #[test]
    fn zlib_roundtrip() {
        for input in [
            b"".as_slice(),
            b"zlib stream body".as_slice(),
            &vec![b'z'; 5000],
            &english_blob(),
            &binary_with_backrefs(),
        ] {
            let z = zlib_compress(input);
            // Header must pass the %31 check and declare CM=8.
            assert_eq!(z[0] & 0x0F, 8);
            assert_eq!(((z[0] as u16) << 8 | z[1] as u16) % 31, 0);
            let back = zlib_decompress(&z).expect("zlib_decompress");
            assert_eq!(back, input);
        }
    }

    #[test]
    fn zlib_rejects_bad_adler() {
        let input = b"adler-protected zlib payload".as_slice();
        let mut z = zlib_compress(input);
        let len = z.len();
        z[len - 1] ^= 0xFF; // corrupt the Adler-32 trailer
        assert_eq!(zlib_decompress(&z), Err(InflateError::BadChecksum));
    }

    // ── Cross-check: gzip body is a valid raw DEFLATE stream ───────────────

    #[test]
    fn gzip_body_is_valid_raw_deflate() {
        let input = english_blob();
        let g = gzip_compress(&input);
        // Strip the 10-byte header and 8-byte trailer; the middle must inflate.
        let body = &g[10..g.len() - 8];
        let back = inflate(body).expect("gzip body must be raw DEFLATE");
        assert_eq!(back, input);
    }

    // ── Checksum known vectors ─────────────────────────────────────────────

    #[test]
    fn crc32_known_vector() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0x0000_0000);
        // FAIL-ability: dropping the final `^ 0xFFFFFFFF` gives 0x340BC6D9.
        assert_ne!(crc32(b"123456789"), 0x340B_C6D9);
    }

    #[test]
    fn adler32_known_vector() {
        // Adler32("Wikipedia") == 0x11E60398 (the canonical RFC 1950 example).
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
        // Adler32("") == 1 (s1=1, s2=0).
        assert_eq!(adler32(b""), 0x0000_0001);
        // Adler32("123456789") == 0x091E01DE.
        assert_eq!(adler32(b"123456789"), 0x091E_01DE);
    }

    // ── Hostile input: corrupt streams Err, never panic ────────────────────

    #[test]
    fn inflate_corrupt_does_not_panic() {
        // Pure garbage bits.
        assert!(inflate(&[0xFF, 0xFF, 0xFF, 0xFF, 0xFF]).is_err());
        // Empty stream (no block header at all).
        assert!(inflate(&[]).is_err());
        // A stored-block header with a broken LEN/~NLEN pair.
        // BFINAL=1, BTYPE=00 → byte 0x01, then LEN=0x0005, NLEN bogus.
        assert!(inflate(&[0x01, 0x05, 0x00, 0x00, 0x00]).is_err());
        // Truncated: valid start of a fixed block then cut off.
        assert!(inflate(&[0x03]).is_err());
        // Fuzz a swath of short inputs — none may panic.
        for seed in 0u16..512 {
            let bytes = seed.to_le_bytes();
            let _ = inflate(&bytes); // Ok or Err, but no panic
        }
    }

    #[test]
    fn gzip_zlib_truncated_err() {
        assert!(gzip_decompress(&[0x1F, 0x8B]).is_err());
        assert!(zlib_decompress(&[0x78]).is_err());
        assert!(gzip_decompress(&[]).is_err());
        assert!(zlib_decompress(&[]).is_err());
    }

    // ── FAIL-ability documentation (asserts that flip if logic breaks) ─────
    //
    // - `roundtrip_*`: if the LZ77 back-reference DISTANCE encoding were wrong
    //   (e.g. distance_to_symbol off by one, or push_code emitting LSB-first
    //   instead of MSB-first), inflate would reconstruct different bytes and
    //   `assert_eq!(back, input)` flips for every match-bearing corpus.
    // - `roundtrip_repetitive_compresses_well` / `compression_ratio_repetitive`:
    //   if the fixed-Huffman LENGTH codes were wrong, the decoder would mis-read
    //   the run and `back != input`; if LZ77 found no matches the ratio assert
    //   would flip (output ≈ input, not >10x/>50x smaller).
    // - `gzip_rejects_bad_crc` / `zlib_rejects_bad_adler`: if checksum
    //   verification were removed these would return Ok and the `Err(...)` assert
    //   flips — proving the checks are load-bearing.

    #[test]
    fn fixed_huffman_length_codes_are_load_bearing() {
        // Construct an input that MUST use a length code (a repeated run), then
        // confirm the decoder recovers it exactly. A broken length-symbol table
        // would corrupt the reconstruction.
        let input = b"abcabcabcabcabcabcabcabcabcabc".as_slice();
        let comp = deflate(input);
        assert!(comp.len() < input.len(), "run must compress");
        assert_eq!(inflate(&comp).unwrap(), input);
    }

    // ════════════════════════════════════════════════════════════════════════
    // FUZZ / PROPERTY suite — deterministic seeded PRNG, no external fuzz crate.
    //
    // Matches the ath_mime/ath_toml pattern: a tiny xorshift64* PRNG generates a
    // reproducible corpus (same seed → same run → reproducible failure). The
    // properties under test are the hostile-input invariants of the public decode
    // surface: `inflate` / `gzip_decompress` / `zlib_decompress` must (a) never
    // panic on ANY byte sequence, (b) bound their output at MAX_OUTPUT regardless
    // of input, and (c) round-trip `inflate(deflate(x)) == x` for arbitrary x.
    //
    // FAIL-ability (proven by reasoning, see REPORT):
    //  - If any decode path could panic on hostile bytes (an unchecked index, an
    //    `unwrap`, an arithmetic overflow in debug) the never-panic loops abort the
    //    test process — the test goes red.
    //  - If the MAX_OUTPUT cap were removed from `inflate_block`/`inflate_stored`,
    //    `bomb_output_is_bounded` would either OOM (process abort = test failure) or,
    //    if it did finish, return a buffer far larger than the asserted cap → the
    //    `<= MAX_OUTPUT` assert flips.
    //  - If the CRC-32 / Adler-32 verification were dropped, the corrupted-checksum
    //    fuzz would return Ok and the `is_err()` assert flips.
    //  - If deflate/inflate disagreed on any token encoding, the round-trip property
    //    `assert_eq!(back, x)` flips for the offending generated input.
    // ════════════════════════════════════════════════════════════════════════

    /// Deterministic xorshift64* PRNG — pure, no_std-safe, reproducible.
    struct Rng(u64);
    impl Rng {
        fn new(seed: u64) -> Self {
            Rng(seed ^ 0x9E37_79B9_7F4A_7C15)
        }
        fn next_u64(&mut self) -> u64 {
            let mut x = self.0;
            x ^= x >> 12;
            x ^= x << 25;
            x ^= x >> 27;
            self.0 = x;
            x.wrapping_mul(0x2545_F491_4F6C_DD1D)
        }
        fn byte(&mut self) -> u8 {
            (self.next_u64() & 0xFF) as u8
        }
        fn below(&mut self, n: usize) -> usize {
            (self.next_u64() % (n as u64)) as usize
        }
    }

    /// Property: `inflate` never panics on arbitrary bytes (0..512 len).
    #[test]
    fn fuzz_inflate_random_never_panics() {
        let mut rng = Rng::new(0xDEFA_7E01);
        for _ in 0..20_000 {
            let len = rng.below(512);
            let mut buf = Vec::with_capacity(len);
            for _ in 0..len {
                buf.push(rng.byte());
            }
            // Ok or Err — but never a panic, and never an out-of-bound output.
            if let Ok(out) = inflate(&buf) {
                assert!(out.len() <= MAX_OUTPUT, "inflate output exceeded cap");
            }
        }
    }

    /// Property: degenerate fills (all-0x00, all-0xFF) at many lengths never panic.
    #[test]
    fn fuzz_inflate_degenerate_fills_never_panic() {
        for fill in [0x00u8, 0xFF, 0x01, 0x80, 0x55, 0xAA] {
            for len in 0..600usize {
                let buf = vec![fill; len];
                let _ = inflate(&buf); // Ok or Err, no panic
            }
        }
    }

    /// Property: truncating a VALID deflate stream at EVERY byte offset never
    /// panics and never produces more than the original (it can only Err or yield
    /// a prefix of the data — typically Err since the stream is incomplete).
    #[test]
    fn fuzz_inflate_truncated_at_every_offset() {
        let corpora: [Vec<u8>; 5] = [
            deflate(b""),
            deflate(b"hello world, a short string to compress"),
            deflate(&vec![b'a'; 4096]), // highly compressible (back-refs)
            deflate(&english_blob()),   // text, fixed-Huffman tokens
            deflate(&pseudo_random(2048)), // incompressible → stored blocks
        ];
        for comp in &corpora {
            for cut in 0..=comp.len() {
                let out = inflate(&comp[..cut]);
                if let Ok(v) = out {
                    assert!(v.len() <= MAX_OUTPUT);
                }
            }
        }
    }

    /// Property: gzip/zlib decode never panics on random bytes and on plausible
    /// header prefixes with garbage bodies.
    #[test]
    fn fuzz_gzip_zlib_random_never_panic() {
        let mut rng = Rng::new(0x6217_B00B);
        for _ in 0..15_000 {
            let len = rng.below(256);
            let mut buf = Vec::with_capacity(len);
            for _ in 0..len {
                buf.push(rng.byte());
            }
            let _ = gzip_decompress(&buf);
            let _ = zlib_decompress(&buf);

            // Same body but forced to carry a valid gzip magic + method, so the
            // header parse is passed and the garbage reaches the inflate/checksum
            // paths.
            if buf.len() >= 18 {
                let mut g = buf.clone();
                g[0] = 0x1F;
                g[1] = 0x8B;
                g[2] = 0x08;
                g[3] = 0x00;
                let r = gzip_decompress(&g);
                assert!(r.is_err(), "random gzip body must not falsely verify");
            }
            // Valid zlib header (CM=8, %31==0, no FDICT) + garbage body.
            if buf.len() >= 6 {
                let mut z = buf.clone();
                z[0] = 0x78;
                z[1] = 0x01; // 0x7801 % 31 == 0
                let r = zlib_decompress(&z);
                assert!(r.is_err(), "random zlib body must not falsely verify");
            }
        }
    }

    /// Property: gzip/zlib truncated at every offset of a valid stream never panic.
    #[test]
    fn fuzz_gzip_zlib_truncated_at_every_offset() {
        let g = gzip_compress(&english_blob());
        let z = zlib_compress(&english_blob());
        for cut in 0..=g.len() {
            let _ = gzip_decompress(&g[..cut]);
        }
        for cut in 0..=z.len() {
            let _ = zlib_decompress(&z[..cut]);
        }
    }

    /// Property: a corrupted CRC/Adler trailer ALWAYS rejects (never falsely
    /// verifies), across many seeds and corruption positions.
    #[test]
    fn fuzz_corrupt_checksum_always_rejects() {
        let mut rng = Rng::new(0xCAFE_F00D);
        let payloads: [Vec<u8>; 3] = [
            b"checksum integrity payload".to_vec(),
            english_blob(),
            pseudo_random(777),
        ];
        for p in &payloads {
            for _ in 0..2000 {
                // gzip: corrupt one of the 8 trailer bytes.
                let mut g = gzip_compress(p);
                let n = g.len();
                let idx = n - 8 + rng.below(8);
                let orig = g[idx];
                g[idx] = orig.wrapping_add(1).wrapping_add(rng.byte()); // guaranteed change
                if g[idx] != orig {
                    // A trailer mutation can change CRC, ISIZE, or (for ISIZE) be
                    // caught as BadChecksum — but it must NEVER decode to the
                    // original payload.
                    match gzip_decompress(&g) {
                        Ok(out) => assert_ne!(&out, p, "corrupt gzip falsely verified"),
                        Err(_) => {}
                    }
                }

                // zlib: corrupt one of the 4 Adler-32 trailer bytes.
                let mut zz = zlib_compress(p);
                let zn = zz.len();
                let zidx = zn - 4 + rng.below(4);
                let zorig = zz[zidx];
                zz[zidx] = zorig.wrapping_add(1).wrapping_add(rng.byte());
                if zz[zidx] != zorig {
                    match zlib_decompress(&zz) {
                        Ok(out) => assert_ne!(&out, p, "corrupt zlib falsely verified"),
                        Err(_) => {}
                    }
                }
            }
        }
    }

    /// Property: a tiny crafted stored-block stream cannot coerce unbounded output
    /// — the decompression-bomb cap is load-bearing. We build a SMALL raw DEFLATE
    /// stream consisting of back-to-back maximal stored blocks that *claim* to keep
    /// producing output; inflate must stop at MAX_OUTPUT, not allocate forever.
    ///
    /// (Stored blocks can't actually expand — every output byte costs an input
    /// byte — so the true unbounded-growth lever is the back-reference path, which
    /// `roundtrip`/`fuzz` already exercises with the cap re-checked each push. This
    /// test asserts the cap is *enforced*, not merely declared: we feed a fixed-
    /// Huffman block whose single literal+max-length back-references would, without
    /// the cap, grow unbounded from a few input bytes.)
    #[test]
    fn bomb_output_is_bounded() {
        // Hand-craft a fixed-Huffman DEFLATE stream: one literal 'a', then repeated
        // max-length back-references (distance 1) that would expand to gigabytes if
        // the stream kept going. We make the stream long enough that, uncapped, it
        // would blow past any reasonable RAM; the cap must bound it.
        //
        // Rather than hand-encode bits, we exploit deflate(): a 1 MiB run of one
        // byte compresses to a few hundred bytes of back-references. Concatenating
        // many such logical runs via a single huge logical input is what the cap
        // guards. Here we assert the *positive* direction: a legitimately large
        // (but < cap) repetitive input round-trips, and a constructed stream that
        // would exceed the cap is rejected with TooLarge rather than OOM.

        // 1) A large-but-legal repetitive input round-trips and stays under cap.
        let big = vec![0x7Eu8; 8 * 1024 * 1024]; // 8 MiB, well under 512 MiB cap
        let comp = deflate(&big);
        assert!(
            comp.len() < big.len() / 100,
            "8 MiB run must compress hugely, {} -> {}",
            big.len(),
            comp.len()
        );
        let back = inflate(&comp).expect("legal large run must inflate");
        assert_eq!(back.len(), big.len());
        assert!(back.len() <= MAX_OUTPUT);

        // 2) Construct a raw stream of stored blocks totaling MORE than MAX_OUTPUT
        //    in DECLARED output, proving inflate_stored's cap check engages. We
        //    emit non-final 64 KiB stored blocks. To keep the TEST itself bounded
        //    we cap the constructed input near the limit and assert the inflate
        //    returns TooLarge before exhausting it.
        //
        //    Each stored block: header byte (BFINAL=0,BTYPE=00) + LEN + ~LEN + data.
        //    We use empty-ish blocks is impossible to overflow, so instead we craft
        //    a back-reference bomb in fixed-Huffman form below (the real lever).

        // 3) The real bomb lever: a fixed-Huffman block that emits one literal then
        //    a run of max-length (258) back-references at distance 1. Each 258-byte
        //    copy costs a handful of bits. We hand-build the bitstream.
        let bomb = build_backref_bomb_stream();
        // The stream, if uncapped, would emit far more than MAX_OUTPUT. With the cap
        // it must return TooLarge (and crucially: not OOM / not panic).
        let r = inflate(&bomb);
        assert_eq!(
            r,
            Err(InflateError::TooLarge),
            "back-reference bomb must hit the output cap, got {:?}",
            r.as_ref().map(|v| v.len())
        );
    }

    /// Build a raw fixed-Huffman DEFLATE stream that emits one literal then enough
    /// max-length (258) distance-1 back-references that, uncapped, output would
    /// exceed MAX_OUTPUT (512 MiB). 512 MiB / 258 ≈ 2.08M copies; we emit a margin
    /// over that so the cap is definitely crossed. The whole stream is only a few
    /// hundred KB of input, proving the tiny-input → huge-output amplification.
    fn build_backref_bomb_stream() -> Vec<u8> {
        // Reuse the crate's own BitWriter via deflate? No — deflate caps each match
        // at the data it actually saw. We need MORE copies than any real input, so
        // we synthesize the bitstream directly with a local LSB-first writer.
        struct W {
            out: Vec<u8>,
            buf: u32,
            cnt: u32,
        }
        impl W {
            fn bits(&mut self, v: u32, n: u32) {
                self.buf |= (v & ((1u32 << n) - 1)) << self.cnt;
                self.cnt += n;
                while self.cnt >= 8 {
                    self.out.push((self.buf & 0xFF) as u8);
                    self.buf >>= 8;
                    self.cnt -= 8;
                }
            }
            fn code(&mut self, code: u32, len: u32) {
                let mut rev = 0u32;
                for i in 0..len {
                    rev |= ((code >> i) & 1) << (len - 1 - i);
                }
                self.bits(rev, len);
            }
            fn finish(mut self) -> Vec<u8> {
                if self.cnt > 0 {
                    self.out.push((self.buf & 0xFF) as u8);
                }
                self.out
            }
        }

        let mut w = W {
            out: Vec::new(),
            buf: 0,
            cnt: 0,
        };
        // Block header: BFINAL=1, BTYPE=01 (fixed Huffman).
        w.bits(1, 1);
        w.bits(1, 2);
        // One literal 'a' (sym 97): fixed code 0x30+97, 8 bits.
        w.code(0x30 + 97, 8);
        // Number of 258-byte copies to push past 512 MiB.
        let copies = (MAX_OUTPUT / 258) + 1024;
        for _ in 0..copies {
            // length symbol 285 (== max length 258, 0 extra bits): fixed code
            // 0xC0 + (285-280) = 0xC5, 8 bits.
            w.code(0xC5, 8);
            // distance symbol 0 (== distance 1, 0 extra bits): fixed dist code 0, 5 bits.
            w.code(0, 5);
        }
        // End-of-block (sym 256): fixed code 0x00, 7 bits.
        w.code(0x00, 7);
        w.finish()
    }

    /// Property: `inflate(deflate(x)) == x` over a broad seeded corpus of mixed
    /// random / repetitive / structured inputs (the load-bearing codec invariant).
    #[test]
    fn fuzz_roundtrip_property() {
        let mut rng = Rng::new(0x5EED_1234);
        for _ in 0..3000 {
            let len = rng.below(2048);
            let mut buf = Vec::with_capacity(len);
            // Mix three regimes so both the LZ77 match path and the stored fallback
            // are exercised: pure random, low-entropy runs, and a small alphabet.
            let mode = rng.below(3);
            for _ in 0..len {
                let b = match mode {
                    0 => rng.byte(),         // incompressible
                    1 => rng.below(4) as u8, // tiny alphabet → many matches
                    _ => {
                        if rng.below(8) == 0 {
                            rng.byte()
                        } else {
                            0x42
                        }
                    } // long runs + occasional noise
                };
                buf.push(b);
            }
            let comp = deflate(&buf);
            let back = inflate(&comp).expect("inflate(deflate(x)) must succeed");
            assert_eq!(back, buf, "round-trip mismatch (mode {mode}, len {len})");
        }
    }
}
