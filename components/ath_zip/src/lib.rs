//! # RaeZip — a never-panic, `no_std` ZIP archive reader (PKZIP / .zip).
//!
//! LEGACY_GAMING_CONCEPT.md §"The user owns the machine": a daily driver must let
//! someone double-click a downloaded `.zip` and see what's inside *without*
//! installing third-party tools. ZIP is also the container under `.docx`/`.xlsx`/
//! `.pptx`, `.epub`, PWA bundles, and `.athpkg` app packages — so one correct,
//! dependency-free, hostile-input ZIP core is foundational infrastructure, not
//! tied to any one consumer (it is deliberately wired into none this slice; the
//! Files "extract here" action is the follow-up).
//!
//! ## Hostile-input posture (CLAUDE: decoders of untrusted bytes are an RCE surface)
//! Every byte handed to [`Archive::open`] / [`Archive::read_entry`] is treated as
//! hostile — a downloaded ZIP is attacker-controlled. There is **no
//! `unwrap`/`expect`/`panic`/raw-index-panic path** reachable from this crate:
//! truncated archives, bogus offsets/sizes, a missing or lying End-Of-Central-
//! Directory record, overlapping or out-of-range local headers, corrupt DEFLATE
//! streams, and a CRC that doesn't match the decompressed bytes all return
//! `Err(ZipError)`. Two amplification attacks are bounded *before* allocation:
//!   - a **zip bomb** (huge `uncompressed_size`, or a tiny compressed entry that
//!     claims to expand by an absurd ratio) is rejected by [`MAX_ENTRY_SIZE`] and
//!     [`MAX_RATIO`] before a single byte is allocated, and the inflate loop
//!     re-checks its running output against the declared size; and
//!   - a **path-traversal** name (`../`, absolute, or a `C:\` drive prefix) is
//!     flagged by [`is_safe_path`], which any extractor MUST consult before
//!     writing an entry to disk.
//!
//! The host KAT suite at the bottom of this file is the primary proof
//! (`cargo test -p ath_zip`): it builds ZIPs in-test (stored + deflate), round-
//! trips them, flips a CRC byte to prove the CRC check is load-bearing, inflates
//! a real dynamic-Huffman fixture, and runs a battery of hostile inputs that must
//! all return `Err` with zero panics.
//!
//! ## What it supports
//! - The classic 32-bit ZIP layout: End Of Central Directory (EOCD) scan from the
//!   tail, the Central Directory file headers (the authoritative entry list), and
//!   per-entry Local File Headers.
//! - Compression **method 0 (Stored / uncompressed)** and **method 8 (DEFLATE)**,
//!   with a from-scratch `inflate` (stored, fixed-Huffman, and dynamic-Huffman
//!   blocks — RFC 1951).
//! - CRC-32 (IEEE 802.3 polynomial) verification of every decompressed entry.
//!
//! ## What it does NOT support (clean per-entry `Err`, never a panic)
//! - Any other compression method (bzip2 / LZMA / PPMd / …) → [`ZipError::Unsupported`].
//! - Encrypted entries (the general-purpose-flag bit 0) → [`ZipError::Unsupported`].
//! - ZIP64 (the 0xFFFFFFFF "see the ZIP64 record" sentinels) → [`ZipError::Unsupported`].
//!   Archives that merely *contain* a ZIP64 EOCD locator but whose 32-bit EOCD is
//!   still valid are read via the 32-bit path; an entry that actually needs ZIP64
//!   sizing reports `Unsupported` rather than guessing.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

// ─── Limits (zip-bomb guards, applied before any allocation) ────────────────

/// Largest decompressed size we will ever allocate for a single entry: 512 MiB.
/// A Central Directory claiming more is rejected without touching memory.
pub const MAX_ENTRY_SIZE: u64 = 512 * 1024 * 1024;

/// Largest decompressed:compressed expansion ratio we trust for a DEFLATE entry.
/// DEFLATE's theoretical max is ~1032:1; a classic zip bomb claims millions:1.
/// 2000:1 is comfortably above any real document yet rejects bomb headers up
/// front (the inflate loop also re-checks the running output independently).
pub const MAX_RATIO: u64 = 2000;

/// Largest number of central-directory entries we will enumerate. A crafted EOCD
/// claiming billions of entries cannot make us spin or over-allocate the list.
pub const MAX_ENTRIES: usize = 1 << 20; // ~1M

// ─── Errors ─────────────────────────────────────────────────────────────────

/// A ZIP read error. Every variant is a *handled* path — nothing here panics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZipError {
    /// Buffer is too small to be a ZIP, or the EOCD signature was never found.
    NotZip,
    /// A header/field/record ran past the end of the buffer.
    Truncated,
    /// A structural field (offset, size, count) points outside the archive or is
    /// internally inconsistent (e.g. a local-header offset past the buffer).
    BadOffset,
    /// A local file header's signature/contents didn't match its central record.
    BadLocalHeader,
    /// Compression method / encryption / ZIP64 this reader doesn't implement.
    Unsupported,
    /// The declared uncompressed size, or the compressed→uncompressed ratio,
    /// exceeds the zip-bomb bounds ([`MAX_ENTRY_SIZE`] / [`MAX_RATIO`]).
    TooLarge,
    /// The DEFLATE stream was corrupt or truncated.
    InflateError,
    /// Decompressed length didn't match the entry's declared uncompressed size.
    SizeMismatch,
    /// The CRC-32 of the decompressed bytes didn't match the stored CRC.
    BadCrc,
}

// ─── CRC-32 (IEEE 802.3 / zlib polynomial 0xEDB88320, reflected) ────────────

/// Compute the IEEE CRC-32 of `data` (the algorithm ZIP, gzip, and PNG use).
///
/// Reflected input/output, init `0xFFFFFFFF`, final XOR `0xFFFFFFFF`. Computed
/// bit-by-bit (no static table) to keep the crate allocation- and data-free;
/// it is only run once per extracted entry, not on a hot path.
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

// ─── Path safety (the extractor's gate against traversal) ───────────────────

/// Returns `true` only if `name` is safe to join onto an extraction root.
///
/// A ZIP entry name is attacker-controlled; writing it verbatim is how a malicious
/// archive escapes its destination directory ("zip slip"). An extractor MUST call
/// this before creating any file. We reject:
///   - empty names,
///   - absolute POSIX paths (leading `/`),
///   - Windows drive prefixes (`C:\`, `c:/`) and UNC/backslash roots,
///   - any path containing a `..` path component (in either slash direction),
///   - NUL bytes.
///
/// Forward and back slashes are both treated as separators (ZIP nominally uses
/// `/`, but hostile archives use `\` to dodge naive `/`-only checks).
pub fn is_safe_path(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    let bytes = name.as_bytes();
    if bytes.contains(&0) {
        return false;
    }
    // Absolute POSIX path.
    if bytes[0] == b'/' || bytes[0] == b'\\' {
        return false;
    }
    // Windows drive letter: "X:" prefix (e.g. C:\ or C:/ or even bare C:foo).
    if bytes.len() >= 2 && bytes[1] == b':' {
        let c = bytes[0];
        if c.is_ascii_alphabetic() {
            return false;
        }
    }
    // Any component equal to ".." escapes the root. Split on both separators.
    for component in name.split(['/', '\\']) {
        if component == ".." {
            return false;
        }
    }
    true
}

// ─── Public ZIP model ───────────────────────────────────────────────────────

/// One file (or directory) entry from the Central Directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZipEntry {
    /// The entry's name as stored in the archive (raw, possibly unsafe — callers
    /// MUST run it through [`is_safe_path`] before extracting to disk).
    pub name: String,
    /// Uncompressed size in bytes.
    pub size: u64,
    /// Compressed (on-disk) size in bytes.
    pub compressed_size: u64,
    /// Compression method (0 = Stored, 8 = DEFLATE; others are [`ZipError::Unsupported`]).
    pub method: u16,
    /// Stored CRC-32 of the uncompressed data.
    pub crc32: u32,
    /// True if this entry denotes a directory (trailing `/` or zero-size marker).
    pub is_dir: bool,
    /// General-purpose bit flag from the central record (bit 0 = encrypted).
    flags: u16,
    /// Byte offset of this entry's Local File Header within the archive.
    local_header_offset: u64,
}

impl ZipEntry {
    /// Whether this entry is encrypted (general-purpose flag bit 0).
    pub fn is_encrypted(&self) -> bool {
        self.flags & 0x0001 != 0
    }
}

/// A parsed ZIP archive bound to its backing byte slice.
///
/// `open` parses only the directory structure (cheap); the compressed bytes are
/// decompressed lazily and per-entry by [`Archive::read_entry`].
pub struct Archive<'a> {
    data: &'a [u8],
    entries: Vec<ZipEntry>,
}

// Signatures (little-endian on disk).
const SIG_EOCD: u32 = 0x0605_4b50; // "PK\x05\x06"
const SIG_CENTRAL: u32 = 0x0201_4b50; // "PK\x01\x02"
const SIG_LOCAL: u32 = 0x0403_4b50; // "PK\x03\x04"

const EOCD_MIN_LEN: usize = 22;
const CENTRAL_FIXED_LEN: usize = 46;
const LOCAL_FIXED_LEN: usize = 30;

#[inline]
fn rd_u16(buf: &[u8], off: usize) -> Option<u16> {
    let b = buf.get(off..off + 2)?;
    Some(u16::from_le_bytes([b[0], b[1]]))
}

#[inline]
fn rd_u32(buf: &[u8], off: usize) -> Option<u32> {
    let b = buf.get(off..off + 4)?;
    Some(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

impl<'a> Archive<'a> {
    /// Parse a ZIP archive from its full byte slice.
    ///
    /// Reads the End Of Central Directory record (scanned from the tail to allow a
    /// trailing comment) and then the Central Directory. Returns `Err` on any
    /// malformed input; never panics.
    pub fn open(data: &'a [u8]) -> Result<Archive<'a>, ZipError> {
        if data.len() < EOCD_MIN_LEN {
            return Err(ZipError::NotZip);
        }

        // --- Locate the EOCD by scanning backward for its signature. The EOCD is
        // 22 bytes + up to a 65535-byte comment, so we search at most that window.
        let eocd_off = find_eocd(data).ok_or(ZipError::NotZip)?;
        let e = &data[eocd_off..];
        // e[0..4] signature already matched.
        let total_entries = rd_u16(e, 10).ok_or(ZipError::Truncated)? as usize;
        let cd_size = rd_u32(e, 12).ok_or(ZipError::Truncated)? as usize;
        let cd_offset = rd_u32(e, 16).ok_or(ZipError::Truncated)? as usize;

        // ZIP64 sentinel: if any of these are maxed out the real values live in a
        // ZIP64 record we don't parse. Refuse rather than misread.
        if total_entries == 0xFFFF || cd_size == 0xFFFF_FFFF || cd_offset == 0xFFFF_FFFF {
            return Err(ZipError::Unsupported);
        }
        if total_entries > MAX_ENTRIES {
            return Err(ZipError::TooLarge);
        }

        // Central directory must lie within the file (before the EOCD).
        let cd_end = cd_offset.checked_add(cd_size).ok_or(ZipError::BadOffset)?;
        if cd_end > data.len() || cd_offset > eocd_off {
            return Err(ZipError::BadOffset);
        }

        let mut entries = Vec::new();
        let mut pos = cd_offset;
        while entries.len() < total_entries {
            // Need at least the fixed-size central record header.
            if pos + CENTRAL_FIXED_LEN > cd_end {
                return Err(ZipError::Truncated);
            }
            let sig = rd_u32(data, pos).ok_or(ZipError::Truncated)?;
            if sig != SIG_CENTRAL {
                return Err(ZipError::BadOffset);
            }
            let flags = rd_u16(data, pos + 8).ok_or(ZipError::Truncated)?;
            let method = rd_u16(data, pos + 10).ok_or(ZipError::Truncated)?;
            let crc = rd_u32(data, pos + 16).ok_or(ZipError::Truncated)?;
            let comp_size = rd_u32(data, pos + 20).ok_or(ZipError::Truncated)? as u64;
            let uncomp_size = rd_u32(data, pos + 24).ok_or(ZipError::Truncated)? as u64;
            let name_len = rd_u16(data, pos + 28).ok_or(ZipError::Truncated)? as usize;
            let extra_len = rd_u16(data, pos + 30).ok_or(ZipError::Truncated)? as usize;
            let comment_len = rd_u16(data, pos + 32).ok_or(ZipError::Truncated)? as usize;
            let lho = rd_u32(data, pos + 42).ok_or(ZipError::Truncated)? as u64;

            let name_start = pos + CENTRAL_FIXED_LEN;
            let name_end = name_start
                .checked_add(name_len)
                .ok_or(ZipError::BadOffset)?;
            if name_end > cd_end {
                return Err(ZipError::Truncated);
            }
            let raw_name = &data[name_start..name_end];
            // Names are nominally UTF-8 (flag bit 11) or CP437; we accept valid
            // UTF-8 and reject the rest cleanly (no lossy guessing of an
            // attacker-controlled name).
            let name = match core::str::from_utf8(raw_name) {
                Ok(s) => String::from(s),
                Err(_) => return Err(ZipError::BadOffset),
            };

            let is_dir = name.ends_with('/') || name.ends_with('\\');

            entries.push(ZipEntry {
                name,
                size: uncomp_size,
                compressed_size: comp_size,
                method,
                crc32: crc,
                is_dir,
                flags,
                local_header_offset: lho,
            });

            // Advance past this record (fixed + name + extra + comment).
            let advance = CENTRAL_FIXED_LEN
                .checked_add(name_len)
                .and_then(|v| v.checked_add(extra_len))
                .and_then(|v| v.checked_add(comment_len))
                .ok_or(ZipError::BadOffset)?;
            pos = pos.checked_add(advance).ok_or(ZipError::BadOffset)?;
            if pos > cd_end {
                return Err(ZipError::Truncated);
            }
        }

        Ok(Archive { data, entries })
    }

    /// The full entry list, in central-directory order.
    pub fn entries(&self) -> &[ZipEntry] {
        &self.entries
    }

    /// Find an entry by exact name.
    pub fn find(&self, name: &str) -> Option<&ZipEntry> {
        self.entries.iter().find(|e| e.name == name)
    }

    /// Decompress one entry, verifying its CRC-32.
    ///
    /// Returns the exact original bytes on success. Errors (never panics) on an
    /// unsupported/encrypted method, a zip-bomb-sized declaration, a corrupt
    /// DEFLATE stream, a size mismatch, or a CRC mismatch ([`ZipError::BadCrc`]).
    pub fn read_entry(&self, entry: &ZipEntry) -> Result<Vec<u8>, ZipError> {
        if entry.is_encrypted() {
            return Err(ZipError::Unsupported);
        }
        if entry.is_dir {
            // A directory marker carries no data.
            return Ok(Vec::new());
        }

        // --- Zip-bomb guards, BEFORE any allocation.
        if entry.size > MAX_ENTRY_SIZE {
            return Err(ZipError::TooLarge);
        }
        if entry.method == 8 && entry.compressed_size > 0 {
            // Reject an absurd declared expansion ratio up front.
            if entry.size / entry.compressed_size.max(1) > MAX_RATIO {
                return Err(ZipError::TooLarge);
            }
        }

        // --- Resolve the compressed bytes via the Local File Header (the
        // authoritative on-disk location; the central record's offset points here).
        let lho = entry.local_header_offset as usize;
        if lho + LOCAL_FIXED_LEN > self.data.len() {
            return Err(ZipError::BadOffset);
        }
        let sig = rd_u32(self.data, lho).ok_or(ZipError::Truncated)?;
        if sig != SIG_LOCAL {
            return Err(ZipError::BadLocalHeader);
        }
        let lh_method = rd_u16(self.data, lho + 8).ok_or(ZipError::Truncated)?;
        let name_len = rd_u16(self.data, lho + 26).ok_or(ZipError::Truncated)? as usize;
        let extra_len = rd_u16(self.data, lho + 28).ok_or(ZipError::Truncated)? as usize;

        // The local header repeats the method; a mismatch means a malformed/
        // inconsistent archive.
        if lh_method != entry.method {
            return Err(ZipError::BadLocalHeader);
        }

        let data_start = lho
            .checked_add(LOCAL_FIXED_LEN)
            .and_then(|v| v.checked_add(name_len))
            .and_then(|v| v.checked_add(extra_len))
            .ok_or(ZipError::BadOffset)?;
        let data_end = data_start
            .checked_add(entry.compressed_size as usize)
            .ok_or(ZipError::BadOffset)?;
        if data_end > self.data.len() {
            return Err(ZipError::Truncated);
        }
        let compressed = &self.data[data_start..data_end];

        let out = match entry.method {
            0 => {
                // Stored: compressed == uncompressed.
                if entry.compressed_size != entry.size {
                    return Err(ZipError::SizeMismatch);
                }
                compressed.to_vec()
            }
            8 => inflate(compressed, entry.size as usize)?,
            _ => return Err(ZipError::Unsupported),
        };

        if out.len() as u64 != entry.size {
            return Err(ZipError::SizeMismatch);
        }
        if crc32(&out) != entry.crc32 {
            return Err(ZipError::BadCrc);
        }
        Ok(out)
    }
}

/// Scan backward from the tail for the EOCD signature, allowing a trailing
/// comment of up to 65535 bytes. Returns the offset of the signature byte.
fn find_eocd(data: &[u8]) -> Option<usize> {
    let len = data.len();
    if len < EOCD_MIN_LEN {
        return None;
    }
    // The signature can start no earlier than len - 22 - 65535.
    let max_back = 22usize + 0xFFFF;
    let start = len.saturating_sub(max_back);
    // Iterate candidate positions from the latest possible backward — the latest
    // valid EOCD (closest to EOF) is the real one.
    let mut i = len - EOCD_MIN_LEN;
    loop {
        if let Some(sig) = rd_u32(data, i) {
            if sig == SIG_EOCD {
                // Validate the comment length: it must reach exactly to (or within)
                // EOF, which weeds out a stray signature inside earlier data.
                if let Some(comment_len) = rd_u16(data, i + 20) {
                    if i + EOCD_MIN_LEN + comment_len as usize <= len {
                        return Some(i);
                    }
                }
            }
        }
        if i == start {
            return None;
        }
        i -= 1;
    }
}

// ─── DEFLATE inflate (RFC 1951) — from-scratch, self-contained ──────────────
//
// Same algorithm as athmedia's PNG IDAT path, reimplemented here so the crate
// has zero dependencies. ZIP stores the *raw* DEFLATE stream (no zlib 2-byte
// wrapper, unlike PNG), so `inflate` consumes the bitstream directly.

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

    fn bit(&mut self) -> Result<u32, ZipError> {
        let byte = *self.data.get(self.byte_pos).ok_or(ZipError::InflateError)?;
        let b = (byte >> self.bit_pos) & 1;
        self.bit_pos += 1;
        if self.bit_pos == 8 {
            self.bit_pos = 0;
            self.byte_pos += 1;
        }
        Ok(b as u32)
    }

    fn bits(&mut self, n: u32) -> Result<u32, ZipError> {
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

    fn read_byte(&mut self) -> Result<u8, ZipError> {
        let b = *self.data.get(self.byte_pos).ok_or(ZipError::InflateError)?;
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
    fn from_lengths(lengths: &[u8]) -> Result<Self, ZipError> {
        let mut counts = [0u16; 16];
        for &l in lengths {
            if l as usize > 15 {
                return Err(ZipError::InflateError);
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
                    return Err(ZipError::InflateError);
                }
                symbols[idx] = sym as u16;
                offsets[l as usize] += 1;
            }
        }
        Ok(Self { counts, symbols })
    }

    fn decode(&self, br: &mut BitReader) -> Result<u16, ZipError> {
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
                    .ok_or(ZipError::InflateError);
            }
            index += count;
            first += count;
            first <<= 1;
            code <<= 1;
        }
        Err(ZipError::InflateError)
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

/// Inflate a raw DEFLATE stream. `expected` is the entry's declared uncompressed
/// size, used as a hard upper bound on output so a corrupt/hostile stream can't
/// be coerced into unbounded growth (defense-in-depth alongside `MAX_ENTRY_SIZE`).
fn inflate(data: &[u8], expected: usize) -> Result<Vec<u8>, ZipError> {
    let cap = expected.min(MAX_ENTRY_SIZE as usize);
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
            _ => return Err(ZipError::InflateError), // btype 3 reserved
        }
        if bfinal == 1 {
            break;
        }
        if out.len() > cap {
            return Err(ZipError::InflateError);
        }
    }
    Ok(out)
}

fn inflate_stored(br: &mut BitReader, out: &mut Vec<u8>, cap: usize) -> Result<(), ZipError> {
    br.align_to_byte();
    let len = (br.read_byte()? as u16) | ((br.read_byte()? as u16) << 8);
    let nlen = (br.read_byte()? as u16) | ((br.read_byte()? as u16) << 8);
    if len != !nlen {
        return Err(ZipError::InflateError);
    }
    if out.len() + len as usize > cap {
        return Err(ZipError::InflateError);
    }
    for _ in 0..len {
        out.push(br.read_byte()?);
    }
    Ok(())
}

fn fixed_litlen() -> Result<Huffman, ZipError> {
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

fn fixed_dist() -> Result<Huffman, ZipError> {
    let lengths = [5u8; 30];
    Huffman::from_lengths(&lengths)
}

fn read_dynamic_tables(br: &mut BitReader) -> Result<(Huffman, Huffman), ZipError> {
    let hlit = br.bits(5)? as usize + 257;
    let hdist = br.bits(5)? as usize + 1;
    let hclen = br.bits(4)? as usize + 4;
    if hlit > 286 || hdist > 30 {
        return Err(ZipError::InflateError);
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
                    return Err(ZipError::InflateError);
                }
                let prev = lengths[i - 1];
                let repeat = br.bits(2)? as usize + 3;
                for _ in 0..repeat {
                    if i >= total {
                        return Err(ZipError::InflateError);
                    }
                    lengths[i] = prev;
                    i += 1;
                }
            }
            17 => {
                let repeat = br.bits(3)? as usize + 3;
                for _ in 0..repeat {
                    if i >= total {
                        return Err(ZipError::InflateError);
                    }
                    lengths[i] = 0;
                    i += 1;
                }
            }
            18 => {
                let repeat = br.bits(7)? as usize + 11;
                for _ in 0..repeat {
                    if i >= total {
                        return Err(ZipError::InflateError);
                    }
                    lengths[i] = 0;
                    i += 1;
                }
            }
            _ => return Err(ZipError::InflateError),
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
) -> Result<(), ZipError> {
    loop {
        let sym = litlen.decode(br)?;
        if sym == 256 {
            return Ok(());
        } else if sym < 256 {
            if out.len() + 1 > cap {
                return Err(ZipError::InflateError);
            }
            out.push(sym as u8);
        } else {
            let li = (sym - 257) as usize;
            if li >= LENGTH_BASE.len() {
                return Err(ZipError::InflateError);
            }
            let length = LENGTH_BASE[li] as usize + br.bits(LENGTH_EXTRA[li] as u32)? as usize;
            let dsym = dist.decode(br)? as usize;
            if dsym >= DIST_BASE.len() {
                return Err(ZipError::InflateError);
            }
            let distance = DIST_BASE[dsym] as usize + br.bits(DIST_EXTRA[dsym] as u32)? as usize;
            if distance == 0 || distance > out.len() {
                return Err(ZipError::InflateError);
            }
            if out.len() + length > cap {
                return Err(ZipError::InflateError);
            }
            let start = out.len() - distance;
            for k in 0..length {
                let b = out[start + k];
                out.push(b);
            }
        }
        if out.len() > cap {
            return Err(ZipError::InflateError);
        }
    }
}

// ════════════════════════════════════════════════════════════════════════════
// ZIP WRITER — build a standard 32-bit (non-ZIP64) archive (APPNOTE.TXT).
//
// LEGACY_GAMING_CONCEPT.md §"The user owns the machine": a daily driver must let
// someone *save* a `.docx`/`.xlsx` (both are ZIP containers) and create/export
// packages — not just read archives. This writer is the inverse of the reader
// above: every archive it produces is read back byte-correctly by `Archive::open`
// (the round-trip is the proof). Method 8 entries are compressed with
// `ath_deflate::deflate`, whose output is a *raw* DEFLATE stream (no zlib/gzip
// wrapper) — exactly the bytes a ZIP local-file body stores, which the reader's
// method-8 `inflate` consumes directly. If DEFLATE wouldn't actually shrink an
// entry (incompressible data) the writer falls back to Stored so output is never
// inflated, mirroring how real zip tools behave.
// ════════════════════════════════════════════════════════════════════════════

/// Largest single entry the writer will accept (uncompressed). The 32-bit ZIP
/// format stores sizes/offsets in `u32`; an entry at or above 4 GiB needs ZIP64,
/// which this writer does not emit, so it is rejected with [`ZipError::TooLarge`]
/// rather than silently truncated into a corrupt record.
pub const MAX_WRITE_ENTRY_SIZE: u64 = 0xFFFF_FFFF;

/// Largest number of entries a 32-bit EOCD can count (`u16`). More than this
/// needs ZIP64; the writer rejects it ([`ZipError::TooLarge`]) instead of
/// wrapping the count into a corrupt EOCD.
pub const MAX_WRITE_ENTRIES: usize = 0xFFFF;

/// Largest total archive size the writer will produce. Local-header and
/// central-directory offsets are `u32`; if appending an entry would push the
/// running offset past 4 GiB the writer refuses ([`ZipError::TooLarge`]) so it
/// never writes an offset field that doesn't address the real bytes.
pub const MAX_ARCHIVE_SIZE: u64 = 0xFFFF_FFFF;

/// The compression method for a written entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    /// Method 0 — stored uncompressed (the body is the raw bytes verbatim).
    Stored,
    /// Method 8 — DEFLATE. If the compressed body would not be smaller than the
    /// raw bytes the writer silently stores the entry instead (never inflates).
    Deflate,
}

/// One staged entry, captured at `add_*` time (name + raw size + CRC + the
/// final on-disk method/body after the deflate-vs-store decision).
struct PendingEntry {
    name: String,
    method: u16, // 0 or 8 — the FINAL method actually written
    crc: u32,
    uncompressed_size: u32,
    body: Vec<u8>, // the exact bytes written after the local header (raw or DEFLATE)
}

/// A streaming ZIP archive builder.
///
/// Add entries with [`ZipWriter::add_file`] (or the [`ZipWriter::add_file_stored`]
/// / [`ZipWriter::add_file_deflate`] conveniences), then call
/// [`ZipWriter::finish`] for the complete archive bytes. The output is a standard
/// 32-bit ZIP (local file headers + central directory + EOCD) that the reader in
/// this crate — and any conformant ZIP tool — reads back exactly.
///
/// Mod-time/date is written as a fixed zero DOS datetime (`0x0000` time,
/// `0x0021` date = 1980-01-01, the earliest representable DOS date) so output is
/// deterministic and reproducible — AthenaOS has no wall clock dependency here and
/// reproducible archives matter for `.athpkg`/document diffing.
pub struct ZipWriter {
    entries: Vec<PendingEntry>,
    /// Running total of the local-header + body bytes (the future CD offset).
    /// Tracked as `u64` so we can detect the 4 GiB overflow before committing.
    local_bytes: u64,
    /// Set once any bound is exceeded; every subsequent `add_*` and `finish`
    /// returns this error so a partially-built archive can never be emitted.
    poisoned: Option<ZipError>,
}

impl Default for ZipWriter {
    fn default() -> Self {
        Self::new()
    }
}

// The DOS date for 1980-01-01: (year-1980)<<9 | month<<5 | day = 0<<9 | 1<<5 | 1.
const DOS_DATE_1980: u16 = (1 << 5) | 1;
const DOS_TIME_ZERO: u16 = 0;
/// Version needed to extract: 2.0 (20) — the baseline for DEFLATE + folders.
const VERSION_NEEDED: u16 = 20;

impl ZipWriter {
    /// Create an empty archive builder.
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            local_bytes: 0,
            poisoned: None,
        }
    }

    /// Add a file. `method` selects Stored or Deflate; for `Deflate` the writer
    /// compresses with `ath_deflate::deflate` and, if that does not shrink the
    /// data, transparently falls back to Stored (so an entry is never inflated).
    ///
    /// Returns `Err` (and poisons the writer) if adding this entry would exceed a
    /// 32-bit ZIP bound (entry ≥ 4 GiB, > 65535 entries, or archive ≥ 4 GiB) —
    /// the writer never produces a corrupt over-limit archive.
    pub fn add_file(&mut self, name: &str, data: &[u8], method: Method) -> Result<(), ZipError> {
        if let Some(e) = self.poisoned {
            return Err(e);
        }

        // --- Bound checks BEFORE staging anything (32-bit ZIP limits).
        if self.entries.len() >= MAX_WRITE_ENTRIES {
            return Err(self.poison(ZipError::TooLarge));
        }
        if data.len() as u64 > MAX_WRITE_ENTRY_SIZE {
            return Err(self.poison(ZipError::TooLarge));
        }

        let crc = crc32(data);
        let uncompressed_size = data.len() as u32;

        // --- Resolve the FINAL method + body (deflate-vs-store decision).
        let (final_method, body): (u16, Vec<u8>) = match method {
            Method::Stored => (0, data.to_vec()),
            Method::Deflate => {
                let compressed = ath_deflate::deflate(data);
                // Real zip tools store, not deflate, when DEFLATE doesn't help.
                // `<` (strictly smaller) so we never trade a stored body for an
                // equal-or-larger deflate body.
                if compressed.len() < data.len() {
                    (8, compressed)
                } else {
                    (0, data.to_vec())
                }
            }
        };

        // --- Per-entry on-disk footprint = local header (30 + name) + body.
        let name_bytes = name.as_bytes();
        if name_bytes.len() > 0xFFFF {
            return Err(self.poison(ZipError::TooLarge));
        }
        let local_footprint = LOCAL_FIXED_LEN as u64 + name_bytes.len() as u64 + body.len() as u64;

        // --- Will the running local-section size, plus this entry, plus the
        // central directory + EOCD we'll append later, fit in a 32-bit archive?
        // Conservatively check the local section alone against 4 GiB here; the CD
        // is bounded again in `finish`.
        let new_local = self
            .local_bytes
            .checked_add(local_footprint)
            .ok_or_else(|| self.poison(ZipError::TooLarge))?;
        if new_local > MAX_ARCHIVE_SIZE {
            return Err(self.poison(ZipError::TooLarge));
        }

        self.local_bytes = new_local;
        self.entries.push(PendingEntry {
            name: String::from(name),
            method: final_method,
            crc,
            uncompressed_size,
            body,
        });
        Ok(())
    }

    /// Convenience: add a Stored (uncompressed) file.
    pub fn add_file_stored(&mut self, name: &str, data: &[u8]) -> Result<(), ZipError> {
        self.add_file(name, data, Method::Stored)
    }

    /// Convenience: add a DEFLATE file (with the automatic store fallback).
    pub fn add_file_deflate(&mut self, name: &str, data: &[u8]) -> Result<(), ZipError> {
        self.add_file(name, data, Method::Deflate)
    }

    /// Number of entries staged so far.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether no entries have been staged.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Finish the archive and return its complete bytes.
    ///
    /// Writes, in order: every entry's Local File Header + body, then the Central
    /// Directory (one record per entry, carrying the local-header offset), then
    /// the End Of Central Directory record. Returns `Err` if the writer was
    /// poisoned by an over-limit `add_*`, or if the assembled central directory /
    /// archive would exceed the 32-bit offset limits.
    pub fn finish(self) -> Result<Vec<u8>, ZipError> {
        if let Some(e) = self.poisoned {
            return Err(e);
        }
        if self.entries.len() > MAX_WRITE_ENTRIES {
            return Err(ZipError::TooLarge);
        }

        let mut out: Vec<u8> = Vec::new();
        // Local-header offsets, captured as we lay down each local section.
        let mut local_offsets: Vec<u32> = Vec::with_capacity(self.entries.len());

        // --- Local file headers + bodies.
        for e in &self.entries {
            // The offset must fit u32 (32-bit ZIP). `local_bytes` was bounded in
            // add_file, but re-check defensively per record.
            if out.len() as u64 > MAX_ARCHIVE_SIZE {
                return Err(ZipError::TooLarge);
            }
            local_offsets.push(out.len() as u32);
            write_local_header(&mut out, e);
            out.extend_from_slice(&e.body);
        }

        // --- Central directory.
        let cd_offset = out.len() as u64;
        if cd_offset > MAX_ARCHIVE_SIZE {
            return Err(ZipError::TooLarge);
        }
        for (i, e) in self.entries.iter().enumerate() {
            write_central_record(&mut out, e, local_offsets[i]);
        }
        let cd_end = out.len() as u64;
        let cd_size = cd_end - cd_offset;
        if cd_end > MAX_ARCHIVE_SIZE || cd_size > MAX_ARCHIVE_SIZE {
            return Err(ZipError::TooLarge);
        }

        // --- End Of Central Directory.
        let count = self.entries.len() as u16;
        out.extend_from_slice(&SIG_EOCD.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // disk number
        out.extend_from_slice(&0u16.to_le_bytes()); // disk with the CD
        out.extend_from_slice(&count.to_le_bytes()); // entries on this disk
        out.extend_from_slice(&count.to_le_bytes()); // total entries
        out.extend_from_slice(&(cd_size as u32).to_le_bytes());
        out.extend_from_slice(&(cd_offset as u32).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // comment length

        Ok(out)
    }

    /// Latch an error and return it (so the first failure is sticky).
    fn poison(&mut self, e: ZipError) -> ZipError {
        if self.poisoned.is_none() {
            self.poisoned = Some(e);
        }
        e
    }
}

/// Write one Local File Header (signature `PK\x03\x04`) + nothing else.
fn write_local_header(out: &mut Vec<u8>, e: &PendingEntry) {
    out.extend_from_slice(&SIG_LOCAL.to_le_bytes());
    out.extend_from_slice(&VERSION_NEEDED.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // general-purpose flags (none)
    out.extend_from_slice(&e.method.to_le_bytes());
    out.extend_from_slice(&DOS_TIME_ZERO.to_le_bytes());
    out.extend_from_slice(&DOS_DATE_1980.to_le_bytes());
    out.extend_from_slice(&e.crc.to_le_bytes());
    out.extend_from_slice(&(e.body.len() as u32).to_le_bytes()); // compressed size
    out.extend_from_slice(&e.uncompressed_size.to_le_bytes());
    out.extend_from_slice(&(e.name.as_bytes().len() as u16).to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // extra field length
    out.extend_from_slice(e.name.as_bytes());
}

/// Write one Central Directory file header (signature `PK\x01\x02`) carrying the
/// entry's metadata and its local-header offset.
fn write_central_record(out: &mut Vec<u8>, e: &PendingEntry, local_offset: u32) {
    out.extend_from_slice(&SIG_CENTRAL.to_le_bytes());
    out.extend_from_slice(&VERSION_NEEDED.to_le_bytes()); // version made by
    out.extend_from_slice(&VERSION_NEEDED.to_le_bytes()); // version needed
    out.extend_from_slice(&0u16.to_le_bytes()); // general-purpose flags
    out.extend_from_slice(&e.method.to_le_bytes());
    out.extend_from_slice(&DOS_TIME_ZERO.to_le_bytes());
    out.extend_from_slice(&DOS_DATE_1980.to_le_bytes());
    out.extend_from_slice(&e.crc.to_le_bytes());
    out.extend_from_slice(&(e.body.len() as u32).to_le_bytes()); // compressed size
    out.extend_from_slice(&e.uncompressed_size.to_le_bytes());
    out.extend_from_slice(&(e.name.as_bytes().len() as u16).to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // extra field length
    out.extend_from_slice(&0u16.to_le_bytes()); // comment length
    out.extend_from_slice(&0u16.to_le_bytes()); // disk number start
    out.extend_from_slice(&0u16.to_le_bytes()); // internal attributes
    out.extend_from_slice(&0u32.to_le_bytes()); // external attributes
    out.extend_from_slice(&local_offset.to_le_bytes());
    out.extend_from_slice(e.name.as_bytes());
}

// ════════════════════════════════════════════════════════════════════════════
// Host KAT suite — `cargo test -p ath_zip`. FAIL-able by construction.
// ════════════════════════════════════════════════════════════════════════════
#[cfg(test)]
mod tests {
    use super::*;
    // Under `#[cfg(test)]` the crate compiles as `std` (the `no_std` attribute is
    // `cfg_attr(not(test), ...)`), so `std`/`String`/`Vec`/`std::vec!` are in scope
    // via the default prelude — no `extern crate std` (which the architecture gate
    // bans as a std-ism line). `StdVec` is just `Vec` re-aliased for the fixture
    // builders below.
    type StdVec<T> = Vec<T>;

    // ── A from-scratch ZIP writer for fixtures ─────────────────────────────
    //
    // Produces a valid 32-bit ZIP with a configurable list of (name, method,
    // raw-bytes, compressed-bytes) entries. For method 0 the raw == compressed;
    // for method 8 the caller supplies a real DEFLATE body.

    struct WEntry {
        name: std::string::String,
        method: u16,
        crc: u32,
        raw: StdVec<u8>,
        comp: StdVec<u8>,
    }

    fn local_header(e: &WEntry) -> StdVec<u8> {
        let mut h = StdVec::new();
        h.extend_from_slice(&SIG_LOCAL.to_le_bytes());
        h.extend_from_slice(&20u16.to_le_bytes()); // version needed
        h.extend_from_slice(&0u16.to_le_bytes()); // flags
        h.extend_from_slice(&e.method.to_le_bytes());
        h.extend_from_slice(&0u16.to_le_bytes()); // mod time
        h.extend_from_slice(&0u16.to_le_bytes()); // mod date
        h.extend_from_slice(&e.crc.to_le_bytes());
        h.extend_from_slice(&(e.comp.len() as u32).to_le_bytes());
        h.extend_from_slice(&(e.raw.len() as u32).to_le_bytes());
        h.extend_from_slice(&(e.name.len() as u16).to_le_bytes());
        h.extend_from_slice(&0u16.to_le_bytes()); // extra len
        h.extend_from_slice(e.name.as_bytes());
        h
    }

    /// Build a complete ZIP. Returns the bytes.
    fn build_zip(entries: &[WEntry]) -> StdVec<u8> {
        let mut out = StdVec::new();
        let mut local_offsets = StdVec::new();

        for e in entries {
            local_offsets.push(out.len() as u32);
            out.extend_from_slice(&local_header(e));
            out.extend_from_slice(&e.comp);
        }

        let cd_offset = out.len() as u32;
        for (i, e) in entries.iter().enumerate() {
            out.extend_from_slice(&SIG_CENTRAL.to_le_bytes());
            out.extend_from_slice(&20u16.to_le_bytes()); // version made by
            out.extend_from_slice(&20u16.to_le_bytes()); // version needed
            out.extend_from_slice(&0u16.to_le_bytes()); // flags
            out.extend_from_slice(&e.method.to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes()); // time
            out.extend_from_slice(&0u16.to_le_bytes()); // date
            out.extend_from_slice(&e.crc.to_le_bytes());
            out.extend_from_slice(&(e.comp.len() as u32).to_le_bytes());
            out.extend_from_slice(&(e.raw.len() as u32).to_le_bytes());
            out.extend_from_slice(&(e.name.len() as u16).to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes()); // extra len
            out.extend_from_slice(&0u16.to_le_bytes()); // comment len
            out.extend_from_slice(&0u16.to_le_bytes()); // disk number
            out.extend_from_slice(&0u16.to_le_bytes()); // internal attrs
            out.extend_from_slice(&0u32.to_le_bytes()); // external attrs
            out.extend_from_slice(&local_offsets[i].to_le_bytes());
            out.extend_from_slice(e.name.as_bytes());
        }
        let cd_size = out.len() as u32 - cd_offset;

        // EOCD.
        out.extend_from_slice(&SIG_EOCD.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // disk number
        out.extend_from_slice(&0u16.to_le_bytes()); // disk with CD
        out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        out.extend_from_slice(&cd_size.to_le_bytes());
        out.extend_from_slice(&cd_offset.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // comment len
        out
    }

    fn stored(name: &str, raw: &[u8]) -> WEntry {
        WEntry {
            name: name.into(),
            method: 0,
            crc: crc32(raw),
            raw: raw.to_vec(),
            comp: raw.to_vec(),
        }
    }

    /// Wrap raw bytes in a single stored (BTYPE=0) DEFLATE block — a valid
    /// method-8 stream we can emit without an encoder. (The dynamic-Huffman path
    /// is exercised separately by a real gzip-produced fixture.)
    fn deflate_stored_block(raw: &[u8]) -> StdVec<u8> {
        let mut out = StdVec::new();
        out.push(0x01); // BFINAL=1, BTYPE=00, then byte-aligned
        let len = raw.len() as u16;
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&(!len).to_le_bytes());
        out.extend_from_slice(raw);
        out
    }

    fn deflate_entry(name: &str, raw: &[u8]) -> WEntry {
        WEntry {
            name: name.into(),
            method: 8,
            crc: crc32(raw),
            raw: raw.to_vec(),
            comp: deflate_stored_block(raw),
        }
    }

    // ── CRC-32 known vectors ───────────────────────────────────────────────

    #[test]
    fn crc32_known_vectors() {
        // The canonical IEEE CRC-32 check value.
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        // Empty input is the initial-XOR-final identity: 0.
        assert_eq!(crc32(b""), 0x0000_0000);
        // "The quick brown fox jumps over the lazy dog".
        assert_eq!(
            crc32(b"The quick brown fox jumps over the lazy dog"),
            0x414F_A339
        );
        // FAIL-ability: a wrong polynomial / missing final XOR changes these. If
        // the final `^ 0xFFFFFFFF` were dropped, crc32(b"123456789") would be
        // 0x340BC6D9, not 0xCBF43926 — this assert flips.
        assert_ne!(crc32(b"123456789"), 0x340B_C6D9);
    }

    // ── Stored + DEFLATE round-trip through the real reader ────────────────

    #[test]
    fn open_and_read_stored_and_deflate() {
        let stored_data = b"Hello, AthenaOS! This is a stored entry.".as_slice();
        let deflate_data =
            b"This entry is wrapped in a DEFLATE stored block (method 8).".as_slice();

        let zip = build_zip(&[
            stored("readme.txt", stored_data),
            deflate_entry("notes/data.bin", deflate_data),
            // A directory marker.
            stored("notes/", b""),
        ]);

        let ar = Archive::open(&zip).expect("open");
        assert_eq!(ar.entries().len(), 3);

        let e0 = ar.find("readme.txt").expect("find readme");
        assert_eq!(e0.method, 0);
        assert_eq!(e0.size, stored_data.len() as u64);
        assert!(!e0.is_dir);
        assert_eq!(ar.read_entry(e0).expect("read stored"), stored_data);

        let e1 = ar.find("notes/data.bin").expect("find data");
        assert_eq!(e1.method, 8);
        assert_eq!(ar.read_entry(e1).expect("read deflate"), deflate_data);

        let dir = ar.find("notes/").expect("find dir");
        assert!(dir.is_dir);
        assert_eq!(ar.read_entry(dir).expect("read dir"), StdVec::<u8>::new());

        // FAIL-ability: if read_entry returned the compressed bytes instead of the
        // decompressed payload, the method-8 entry's bytes would NOT equal
        // deflate_data (they'd carry the 5-byte DEFLATE stored-block header).
        assert_ne!(ar.read_entry(e1).unwrap(), e1_comp_bytes(&zip));
    }

    // Helper for the FAIL-ability assert above: extract the raw compressed bytes
    // of the second entry by re-deflating the known payload.
    fn e1_comp_bytes(_zip: &[u8]) -> StdVec<u8> {
        deflate_stored_block(b"This entry is wrapped in a DEFLATE stored block (method 8).")
    }

    #[test]
    fn corrupted_crc_is_rejected() {
        let data = b"crc-protected payload".as_slice();
        // Build a stored entry, then corrupt its stored CRC in BOTH the central
        // and local records so the read reaches the CRC check (not a header check).
        let mut e = stored("file.txt", data);
        e.crc ^= 0x0000_00FF; // flip low byte of the CRC
        let zip = build_zip(&[e]);

        let ar = Archive::open(&zip).expect("open");
        let entry = ar.find("file.txt").expect("find");
        // Decompression succeeds, but the computed CRC won't match the (lying)
        // stored CRC → BadCrc.
        assert_eq!(ar.read_entry(entry), Err(ZipError::BadCrc));

        // FAIL-ability: if the CRC verification in read_entry were removed, this
        // would return Ok(data) and the assert flips. Sanity: an UNcorrupted copy
        // of the same data reads fine.
        let good = build_zip(&[stored("file.txt", data)]);
        let ar2 = Archive::open(&good).unwrap();
        assert_eq!(ar2.read_entry(ar2.find("file.txt").unwrap()).unwrap(), data);
    }

    // ── Real dynamic-Huffman DEFLATE entry ─────────────────────────────────

    // The same gzip-produced dynamic-Huffman body the PNG decoder is proven with,
    // packaged as a method-8 ZIP entry. First body byte 0x0d = BFINAL=1, BTYPE=10b.
    const DYN_TEXT: &[u8] = b"The quick brown fox jumps over the lazy dog. \
          Pack my box with five dozen liquor jugs. 0123456789!";

    fn dyn_deflate_body() -> StdVec<u8> {
        std::vec![
            0x0d, 0xcb, 0xc7, 0x15, 0x80, 0x20, 0x10, 0x45, 0xd1, 0x56, 0xbe, 0x0d, 0x70, 0xcc,
            0xa1, 0x0b, 0x17, 0x36, 0x60, 0x40, 0xc0, 0xc0, 0x28, 0x8a, 0xa9, 0x7a, 0x67, 0xfd,
            0xee, 0x6b, 0xb4, 0xc4, 0xee, 0x4d, 0x3f, 0xa3, 0x73, 0x74, 0x5b, 0x8c, 0xf4, 0x60,
            0xf2, 0xeb, 0x76, 0x80, 0x2e, 0xe9, 0x70, 0x72, 0x5e, 0xda, 0xef, 0xc5, 0x40, 0x4a,
            0xa0, 0x6e, 0xd9, 0xad, 0x2f, 0x3a, 0x46, 0xb7, 0x39, 0x35, 0x46, 0x73, 0x49, 0x4e,
            0x9f, 0xb4, 0x58, 0xcc, 0xee, 0xc9, 0xf1, 0xab, 0x0e, 0x81, 0x30, 0x8a, 0x93, 0x34,
            0xcb, 0x8b, 0xb2, 0x0a, 0x7e
        ]
    }

    #[test]
    fn inflate_dynamic_huffman_entry() {
        let body = dyn_deflate_body();
        assert_eq!(
            body[0] & 0x07,
            0b101,
            "fixture must be a final dynamic block"
        );

        // Raw inflate.
        let direct = inflate(&body, DYN_TEXT.len()).expect("dynamic inflate");
        assert_eq!(direct.as_slice(), DYN_TEXT);

        // Through the full ZIP path (method 8 + CRC verify).
        let e = WEntry {
            name: "fox.txt".into(),
            method: 8,
            crc: crc32(DYN_TEXT),
            raw: DYN_TEXT.to_vec(),
            comp: body,
        };
        let zip = build_zip(&[e]);
        let ar = Archive::open(&zip).expect("open");
        let entry = ar.find("fox.txt").expect("find");
        assert_eq!(ar.read_entry(entry).expect("read dyn"), DYN_TEXT);

        // FAIL-ability: any Huffman-table / length-distance bug corrupts this.
        assert_ne!(direct.first(), Some(&b'X'));
    }

    // ── Path traversal ─────────────────────────────────────────────────────

    #[test]
    fn path_safety() {
        // Unsafe.
        assert!(!is_safe_path("../etc/passwd"));
        assert!(!is_safe_path("/etc/passwd"));
        assert!(!is_safe_path("a/../../b"));
        assert!(!is_safe_path("C:\\Windows\\system32"));
        assert!(!is_safe_path("c:/Windows"));
        assert!(!is_safe_path("\\\\server\\share"));
        assert!(!is_safe_path("foo\\..\\bar"));
        assert!(!is_safe_path(".."));
        assert!(!is_safe_path(""));
        assert!(!is_safe_path("has\0nul"));

        // Safe.
        assert!(is_safe_path("readme.txt"));
        assert!(is_safe_path("dir/sub/file.bin"));
        assert!(is_safe_path("a.b.c/d_e-f/g.txt"));
        assert!(is_safe_path("..dotfile")); // "..dotfile" is not the ".." component
        assert!(is_safe_path("foo..bar/baz"));

        // FAIL-ability: if the ".." component check were dropped, "a/../../b"
        // would pass and this assert flips.
        assert!(!is_safe_path("a/../../b"));
    }

    // ── Hostile battery: Err, never panic ──────────────────────────────────

    #[test]
    fn reject_not_a_zip() {
        assert_eq!(Archive::open(&[]).err(), Some(ZipError::NotZip));
        assert_eq!(
            Archive::open(b"not a zip file at all").err(),
            Some(ZipError::NotZip)
        );
        let junk = std::vec![0xABu8; 1024];
        assert_eq!(Archive::open(&junk).err(), Some(ZipError::NotZip));
    }

    #[test]
    fn reject_truncated_eocd() {
        // A valid-looking partial EOCD signature with not enough trailing bytes.
        let mut data = std::vec![0u8; 10];
        data.extend_from_slice(&SIG_EOCD.to_le_bytes()); // signature but < 22 total
        assert!(Archive::open(&data).is_err());
    }

    #[test]
    fn reject_offset_past_buffer() {
        // Build a valid zip, then corrupt the EOCD's CD offset to point past EOF.
        let mut zip = build_zip(&[stored("a.txt", b"hello")]);
        let eocd = find_eocd(&zip).expect("eocd");
        // CD offset is at eocd+16.
        let bad = 0x7FFF_FFFFu32.to_le_bytes();
        zip[eocd + 16..eocd + 20].copy_from_slice(&bad);
        assert!(matches!(
            Archive::open(&zip),
            Err(ZipError::BadOffset) | Err(ZipError::Truncated)
        ));
    }

    #[test]
    fn reject_oversized_uncompressed_zip_bomb() {
        // A stored entry whose central record LIES about a ~4 GiB uncompressed
        // size (a zip-bomb-shaped header) must be rejected before any allocation.
        let payload = b"tiny".as_slice();
        let e = stored("bomb", payload);
        // Compressed = 4 bytes on disk, but the headers claim ~4 GiB uncompressed.
        let zip = build_zip_with_sizes(&e, payload.len() as u32, 0xFFFF_FFF0);
        let ar = Archive::open(&zip).expect("open");
        let entry = ar.find("bomb").expect("find");
        assert_eq!(entry.size, 0xFFFF_FFF0);
        assert_eq!(ar.read_entry(entry), Err(ZipError::TooLarge));
    }

    #[test]
    fn reject_deflate_ratio_bomb() {
        // A method-8 entry: 4-byte compressed, claims 1 GiB uncompressed → ratio
        // far over MAX_RATIO → TooLarge, before inflate runs.
        let e = WEntry {
            name: "ratio".into(),
            method: 8,
            crc: 0,
            raw: std::vec![0u8; 0], // size taken from raw.len() in writer; override below
            comp: std::vec![0x00, 0x00, 0x00, 0x00],
        };
        // Build manually so we can set a lying uncompressed size.
        let zip = build_zip_with_sizes(&e, 4, 1024 * 1024 * 1024);
        let ar = Archive::open(&zip).expect("open");
        let entry = ar.find("ratio").expect("find");
        assert_eq!(ar.read_entry(entry), Err(ZipError::TooLarge));
    }

    #[test]
    fn reject_corrupt_deflate() {
        // A method-8 entry whose body is garbage bits → InflateError (never panic).
        let garbage = std::vec![0xFFu8, 0xFF, 0xFF, 0xFF, 0xFF];
        let e = WEntry {
            name: "broken".into(),
            method: 8,
            crc: 0,
            raw: std::vec![0u8; 16],
            comp: garbage,
        };
        let zip = build_zip_with_sizes(&e, e_comp_len(&e), 16);
        let ar = Archive::open(&zip).expect("open");
        let entry = ar.find("broken").expect("find");
        assert!(ar.read_entry(entry).is_err());
    }

    #[test]
    fn reject_unsupported_method() {
        // Method 12 (bzip2) → Unsupported.
        let e = WEntry {
            name: "x.bz2".into(),
            method: 12,
            crc: 0,
            raw: std::vec![0u8; 4],
            comp: std::vec![0u8; 4],
        };
        let zip = build_zip_with_sizes(&e, 4, 4);
        let ar = Archive::open(&zip).expect("open");
        let entry = ar.find("x.bz2").expect("find");
        assert_eq!(ar.read_entry(entry), Err(ZipError::Unsupported));
    }

    #[test]
    fn reject_zip64_sentinel() {
        // An EOCD claiming 0xFFFF entries (ZIP64 sentinel) → Unsupported.
        let mut zip = build_zip(&[stored("a", b"x")]);
        let eocd = find_eocd(&zip).expect("eocd");
        zip[eocd + 10..eocd + 12].copy_from_slice(&0xFFFFu16.to_le_bytes());
        assert_eq!(Archive::open(&zip).err(), Some(ZipError::Unsupported));
    }

    // ── writer helpers that allow lying sizes (for bomb/corrupt tests) ──────

    fn e_comp_len(e: &WEntry) -> u32 {
        e.comp.len() as u32
    }

    /// Like `build_zip` for a single entry, but writes caller-chosen compressed
    /// and uncompressed sizes into BOTH headers (so we can construct lying/bomb
    /// records the honest writer wouldn't produce).
    fn build_zip_with_sizes(e: &WEntry, comp_size: u32, uncomp_size: u32) -> StdVec<u8> {
        let mut out = StdVec::new();
        let local_off = 0u32;

        // Local header.
        out.extend_from_slice(&SIG_LOCAL.to_le_bytes());
        out.extend_from_slice(&20u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&e.method.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&e.crc.to_le_bytes());
        out.extend_from_slice(&comp_size.to_le_bytes());
        out.extend_from_slice(&uncomp_size.to_le_bytes());
        out.extend_from_slice(&(e.name.len() as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(e.name.as_bytes());
        out.extend_from_slice(&e.comp);

        let cd_offset = out.len() as u32;
        out.extend_from_slice(&SIG_CENTRAL.to_le_bytes());
        out.extend_from_slice(&20u16.to_le_bytes());
        out.extend_from_slice(&20u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&e.method.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&e.crc.to_le_bytes());
        out.extend_from_slice(&comp_size.to_le_bytes());
        out.extend_from_slice(&uncomp_size.to_le_bytes());
        out.extend_from_slice(&(e.name.len() as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&local_off.to_le_bytes());
        out.extend_from_slice(e.name.as_bytes());
        let cd_size = out.len() as u32 - cd_offset;

        out.extend_from_slice(&SIG_EOCD.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&cd_size.to_le_bytes());
        out.extend_from_slice(&cd_offset.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out
    }

    // ════════════════════════════════════════════════════════════════════════
    // FUZZ / PROPERTY suite — deterministic seeded PRNG, no external fuzz crate.
    //
    // Matches the ath_mime/ath_toml pattern. Properties on the public surface:
    // `Archive::open` / `read_entry` must (a) never panic on ANY byte sequence,
    // (b) bound a malicious archive via the entry/size/ratio caps (MAX_ENTRY_SIZE
    // / MAX_RATIO / MAX_ENTRIES) before allocating, and (c) `is_safe_path` must
    // reject every hostile name (zip-slip guard), as a property.
    //
    // FAIL-ability (proven by reasoning, see REPORT):
    //  - Any panic on hostile bytes (an unchecked slice index, an `unwrap`, an
    //    arithmetic overflow in debug) aborts the never-panic loops → red.
    //  - Removing `entry.size > MAX_ENTRY_SIZE` makes `bomb_*` attempt a multi-GiB
    //    Vec → OOM/abort (or a wrong Err) → red.
    //  - Removing the `..`/absolute/drive checks flips `prop_is_safe_path_*`.
    //  - Removing the ratio guard makes the ratio-bomb assert (Err(TooLarge)) flip.
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

    /// Property: `Archive::open` never panics on arbitrary random bytes.
    #[test]
    fn fuzz_open_random_never_panics() {
        let mut rng = Rng::new(0x21B0_0001);
        for _ in 0..20_000 {
            let len = rng.below(512);
            let mut buf = Vec::with_capacity(len);
            for _ in 0..len {
                buf.push(rng.byte());
            }
            // If it somehow parses, reading every entry must also not panic.
            if let Ok(ar) = Archive::open(&buf) {
                for e in ar.entries() {
                    let _ = ar.read_entry(e);
                }
            }
        }
    }

    /// Property: random buffers that END with a valid EOCD signature (so the tail
    /// scan latches and the central-directory parser is actually entered) never
    /// panic — this drives far deeper into the parser than pure noise.
    #[test]
    fn fuzz_planted_eocd_never_panics() {
        let mut rng = Rng::new(0x21B0_0002);
        for _ in 0..15_000 {
            let body_len = rng.below(256);
            let mut buf = Vec::with_capacity(body_len + 22);
            for _ in 0..body_len {
                buf.push(rng.byte());
            }
            // Append a 22-byte EOCD with random count/size/offset fields.
            buf.extend_from_slice(&SIG_EOCD.to_le_bytes());
            buf.extend_from_slice(&(rng.next_u64() as u16).to_le_bytes()); // disk
            buf.extend_from_slice(&(rng.next_u64() as u16).to_le_bytes()); // disk w/ cd
            buf.extend_from_slice(&(rng.next_u64() as u16).to_le_bytes()); // entries (disk)
            buf.extend_from_slice(&(rng.next_u64() as u16).to_le_bytes()); // total entries
            buf.extend_from_slice(&(rng.next_u64() as u32).to_le_bytes()); // cd size
            buf.extend_from_slice(&(rng.next_u64() as u32).to_le_bytes()); // cd offset
            buf.extend_from_slice(&0u16.to_le_bytes()); // comment len 0
            if let Ok(ar) = Archive::open(&buf) {
                for e in ar.entries() {
                    let _ = ar.read_entry(e);
                }
            }
        }
    }

    /// Property: a valid ZIP truncated at EVERY byte offset never panics.
    #[test]
    fn fuzz_truncated_at_every_offset() {
        let zip = build_zip(&[
            stored("a.txt", b"first stored entry payload"),
            deflate_entry("b/c.bin", b"second entry via a deflate stored block"),
            stored("b/", b""),
        ]);
        for cut in 0..=zip.len() {
            if let Ok(ar) = Archive::open(&zip[..cut]) {
                for e in ar.entries() {
                    let _ = ar.read_entry(e);
                }
            }
        }
    }

    /// Property: flipping any single byte of a valid ZIP never panics (it may
    /// produce a different parse or an Err, but never a crash / out-of-bound).
    #[test]
    fn fuzz_single_byte_flips_never_panic() {
        let zip = build_zip(&[
            stored("readme", b"some bytes"),
            deflate_entry("data", b"deflate me"),
        ]);
        let mut rng = Rng::new(0x21B0_0003);
        for _ in 0..6000 {
            let mut m = zip.clone();
            let idx = rng.below(m.len());
            m[idx] ^= 1u8 << rng.below(8);
            if let Ok(ar) = Archive::open(&m) {
                for e in ar.entries() {
                    let _ = ar.read_entry(e);
                }
            }
        }
    }

    /// Property: a stored entry that LIES about a multi-GiB uncompressed size is
    /// rejected by MAX_ENTRY_SIZE before any allocation, across many huge claims.
    #[test]
    fn bomb_oversized_uncompressed_capped() {
        let payload = b"tiny".as_slice();
        let e = stored("bomb", payload);
        for &claimed in &[
            (MAX_ENTRY_SIZE as u32) + 1,
            0x8000_0000, // 2 GiB
            0xC000_0000, // 3 GiB
            0xFFFF_FFF0, // ~4 GiB
        ] {
            let zip = build_zip_with_sizes(&e, payload.len() as u32, claimed);
            let ar = Archive::open(&zip).expect("open");
            let entry = ar.find("bomb").expect("find");
            assert_eq!(
                ar.read_entry(entry),
                Err(ZipError::TooLarge),
                "oversized uncompressed claim {claimed:#x} must be capped"
            );
        }
    }

    /// Property: a method-8 entry with an absurd compressed→uncompressed ratio is
    /// rejected by MAX_RATIO before inflate runs, over many tiny compressed sizes.
    #[test]
    fn bomb_ratio_capped() {
        let mut rng = Rng::new(0x21B0_0004);
        for _ in 0..300 {
            let comp_len = 1 + rng.below(64);
            let body: Vec<u8> = (0..comp_len).map(|_| rng.byte()).collect();
            let e = WEntry {
                name: "ratio".into(),
                method: 8,
                crc: 0,
                raw: std::vec![0u8; 0],
                comp: body,
            };
            // Claim ratio far over MAX_RATIO but uncompressed under MAX_ENTRY_SIZE
            // (so the ratio guard, not the size guard, is the one under test).
            let claimed = ((comp_len as u64) * (MAX_RATIO + 50)).min(MAX_ENTRY_SIZE) as u32;
            let zip = build_zip_with_sizes(&e, comp_len as u32, claimed);
            let ar = Archive::open(&zip).expect("open");
            let entry = ar.find("ratio").expect("find");
            assert_eq!(
                ar.read_entry(entry),
                Err(ZipError::TooLarge),
                "ratio bomb (comp {comp_len} -> {claimed}) must be rejected pre-inflate"
            );
        }
    }

    /// Property: an EOCD claiming a huge total_entries cannot make `open` spin or
    /// over-allocate — MAX_ENTRIES bounds it (or a Truncated CD bound trips first).
    #[test]
    fn bomb_entry_count_capped() {
        let mut zip = build_zip(&[stored("a", b"x")]);
        let eocd = find_eocd(&zip).expect("eocd");
        // total_entries field is at eocd+10 (u16). Max it just below the ZIP64
        // sentinel (0xFFFF would route to Unsupported instead).
        zip[eocd + 10..eocd + 12].copy_from_slice(&0xFFFEu16.to_le_bytes());
        // 0xFFFE (65534) is under MAX_ENTRIES (1<<20), so open proceeds to parse the
        // central directory and hits Truncated when it runs out of CD records — a
        // clean Err, no spin, no over-allocation of a 65534-slot list of garbage.
        let err = Archive::open(&zip).err();
        assert!(
            matches!(err, Some(ZipError::Truncated) | Some(ZipError::BadOffset)),
            "huge entry count must terminate cleanly, got {err:?}"
        );
    }

    /// Property: claimed-size-vs-actual mismatch on a stored entry is caught
    /// (SizeMismatch), never silently accepted, over random size deltas.
    #[test]
    fn prop_stored_size_mismatch_rejected() {
        let mut rng = Rng::new(0x21B0_0005);
        for _ in 0..300 {
            let n = 1 + rng.below(64);
            let payload: Vec<u8> = (0..n).map(|_| rng.byte()).collect();
            let e = stored("f", &payload);
            // Lie: declared uncompressed != compressed for a stored (method 0)
            // entry. read_entry must catch via the method-0 size==comp check.
            let wrong = (n as u32).wrapping_add(1 + rng.below(16) as u32);
            let zip = build_zip_with_sizes(&e, n as u32, wrong);
            let ar = Archive::open(&zip).expect("open");
            let entry = ar.find("f").expect("find");
            let r = ar.read_entry(entry);
            assert!(
                matches!(r, Err(ZipError::SizeMismatch) | Err(ZipError::TooLarge)),
                "stored size mismatch (comp {n} vs claimed {wrong}) must reject, got {r:?}"
            );
        }
    }

    /// Property: `is_safe_path` rejects every generated hostile name and accepts
    /// every generated benign name (the zip-slip guard, asserted as a property).
    #[test]
    fn prop_is_safe_path_hostile_vs_benign() {
        let mut rng = Rng::new(0x21B0_0006);
        let alpha = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_-.";

        fn benign_component(rng: &mut Rng, alpha: &[u8]) -> std::string::String {
            let len = 1 + rng.below(8);
            let mut s = std::string::String::new();
            loop {
                s.clear();
                for _ in 0..len {
                    s.push(alpha[rng.below(alpha.len())] as char);
                }
                if s != ".." && s != "." {
                    break;
                }
            }
            s
        }

        for _ in 0..10_000 {
            let depth = 1 + rng.below(5);
            let mut parts: Vec<std::string::String> = Vec::new();
            for _ in 0..depth {
                parts.push(benign_component(&mut rng, alpha));
            }
            let safe = parts.join("/");
            let drive_shaped = safe.as_bytes().len() >= 2
                && safe.as_bytes()[1] == b':'
                && safe.as_bytes()[0].is_ascii_alphabetic();
            if !drive_shaped {
                assert!(
                    is_safe_path(&safe),
                    "benign path wrongly rejected: {safe:?}"
                );
            }

            match rng.below(5) {
                0 => {
                    let mut hostile = parts.clone();
                    hostile.insert(rng.below(hostile.len() + 1), "..".to_string());
                    let h = hostile.join("/");
                    assert!(!is_safe_path(&h), "dotdot path accepted: {h:?}");
                }
                1 => {
                    let h = "/".to_string() + &parts.join("/");
                    assert!(!is_safe_path(&h), "absolute path accepted: {h:?}");
                }
                2 => {
                    let h = "C:\\".to_string() + &parts.join("\\");
                    assert!(!is_safe_path(&h), "drive path accepted: {h:?}");
                }
                3 => {
                    let h = parts.join("\\") + "\\..\\evil";
                    assert!(!is_safe_path(&h), "backslash dotdot accepted: {h:?}");
                }
                _ => {
                    let h = parts.join("/") + "\0evil";
                    assert!(!is_safe_path(&h), "NUL path accepted: {h:?}");
                }
            }
        }

        for bad in [
            "../x",
            "a/../../b",
            "/abs",
            "\\unc",
            "C:/win",
            "c:foo",
            "..",
            "",
            "x\0y",
        ] {
            assert!(!is_safe_path(bad), "fixed hostile name accepted: {bad:?}");
        }
    }

    /// Property: a real ZIP whose entry NAME is a zip-slip path still parses without
    /// panic, the raw name is preserved (not silently sanitized), and is_safe_path
    /// flags it — the reader hands the gate to the caller per the doc contract.
    #[test]
    fn hostile_entry_name_preserved_and_flagged() {
        for bad in ["../escape.txt", "a/../../b", "nested/../../../etc"] {
            let zip = build_zip(&[stored(bad, b"payload")]);
            let ar = Archive::open(&zip).expect("hostile-name zip must still parse");
            let e = ar.find(bad).expect("entry by raw name");
            assert_eq!(e.name, bad);
            // The data still reads correctly (parsing is name-agnostic)...
            assert_eq!(ar.read_entry(e).expect("read"), b"payload");
            // ...but the name is correctly flagged unsafe for extraction.
            assert!(!is_safe_path(&e.name), "hostile name {bad:?} not flagged");
        }
    }

    // ════════════════════════════════════════════════════════════════════════
    // ZIP WRITER KATs — the load-bearing proof is the round-trip: every archive
    // the writer produces is read back BYTE-CORRECTLY by the reader above (which
    // verifies CRC-32 internally, so a wrong CRC would make read_entry Err). The
    // writer's output is intentionally NOT compared against the in-test fixture
    // builder — the real reader is the oracle.
    //
    // FAIL-ability (each proven by construction):
    //  - tweak any asserted byte/name/size and the round-trip assert flips;
    //  - a wrong CRC in write_*_header → reader returns Err(BadCrc) → .expect panics;
    //  - wrong method-8 framing (e.g. zlib-wrapped body) → reader's inflate Errs;
    //  - dropping the store-fallback → the incompressible test's method assert flips;
    //  - removing a bound check → the over-limit tests return Ok and their asserts flip.
    // ════════════════════════════════════════════════════════════════════════

    /// 2-3 files (Stored + Deflate mix) → reader reads each name, size, and the
    /// EXACT original bytes back. The CRC check lives inside read_entry, so this
    /// single round-trip also proves the writer's CRC-32 is correct.
    #[test]
    fn writer_roundtrip_stored_and_deflate() {
        let stored_data = b"Hello, AthenaOS! A stored entry written by ZipWriter.".as_slice();
        let deflate_data = "Compress me with method 8. ".repeat(40); // repetitive → shrinks
        let third = b"a third tiny stored file".as_slice();

        let mut w = ZipWriter::new();
        w.add_file("readme.txt", stored_data, Method::Stored)
            .unwrap();
        w.add_file("data/notes.txt", deflate_data.as_bytes(), Method::Deflate)
            .unwrap();
        w.add_file_stored("third.bin", third).unwrap();
        let bytes = w.finish().expect("finish");

        let ar = Archive::open(&bytes).expect("reader opens writer output");
        assert_eq!(ar.entries().len(), 3);

        let e0 = ar.find("readme.txt").expect("find readme");
        assert_eq!(e0.method, 0);
        assert_eq!(e0.size, stored_data.len() as u64);
        assert_eq!(ar.read_entry(e0).expect("read stored"), stored_data);

        let e1 = ar.find("data/notes.txt").expect("find notes");
        assert_eq!(
            e1.method, 8,
            "repetitive data must end up DEFLATE-compressed"
        );
        assert_eq!(e1.size, deflate_data.len() as u64);
        assert_eq!(
            ar.read_entry(e1).expect("read deflate"),
            deflate_data.as_bytes()
        );

        let e2 = ar.find("third.bin").expect("find third");
        assert_eq!(ar.read_entry(e2).expect("read third"), third);

        // FAIL-ability: if read_entry returned the (raw DEFLATE) on-disk body
        // instead of the inflated payload, e1's bytes would not equal deflate_data.
        assert_ne!(ar.read_entry(e1).unwrap().len(), 0);
    }

    /// A Deflate entry the reader inflates back to the original — proves the
    /// method-8 *raw* DEFLATE framing (no zlib header/adler trailer) is correct.
    #[test]
    fn writer_deflate_raw_framing_inflates() {
        // Strongly repetitive: guaranteed to take the DEFLATE branch (method 8).
        let data = vec![b'Q'; 5000];
        let mut w = ZipWriter::new();
        w.add_file_deflate("q.dat", &data).unwrap();
        let bytes = w.finish().unwrap();

        let ar = Archive::open(&bytes).unwrap();
        let e = ar.find("q.dat").unwrap();
        assert_eq!(e.method, 8, "5000 identical bytes must compress (method 8)");
        assert!(
            e.compressed_size < e.size,
            "compressed {} must be < uncompressed {}",
            e.compressed_size,
            e.size
        );
        // The reader's method-8 path runs raw DEFLATE inflate; success here is the
        // proof that the writer stored a raw (not zlib-wrapped) stream.
        assert_eq!(ar.read_entry(e).expect("inflate raw deflate"), data);
    }

    /// An empty file, an all-bytes (0x00..=0xFF) binary file, and a 100 KB
    /// compressible file that DEFLATE actually shrinks — all round-trip.
    #[test]
    fn writer_empty_binary_and_large_compressible() {
        // 0x00..=0xFF, repeated, so DEFLATE finds matches.
        let mut all_bytes = Vec::new();
        for _ in 0..4 {
            for b in 0u16..256 {
                all_bytes.push(b as u8);
            }
        }
        // ~100 KB of compressible data (repeated text).
        let large = "AthenaOS reproducible package payload. ".repeat(2700);
        assert!(large.len() > 100_000);

        let mut w = ZipWriter::new();
        w.add_file("empty", b"", Method::Deflate).unwrap();
        w.add_file("bin/all.bytes", &all_bytes, Method::Deflate)
            .unwrap();
        w.add_file("big.txt", large.as_bytes(), Method::Deflate)
            .unwrap();
        let bytes = w.finish().unwrap();

        let ar = Archive::open(&bytes).unwrap();

        let ee = ar.find("empty").unwrap();
        assert_eq!(ee.size, 0);
        assert_eq!(ar.read_entry(ee).unwrap(), Vec::<u8>::new());

        let eb = ar.find("bin/all.bytes").unwrap();
        assert_eq!(ar.read_entry(eb).unwrap(), all_bytes);

        let el = ar.find("big.txt").unwrap();
        assert_eq!(el.method, 8, "100 KB of repeated text must compress");
        assert!(
            el.compressed_size < el.size,
            "large compressible body must shrink ({} -> {})",
            el.size,
            el.compressed_size
        );
        assert_eq!(ar.read_entry(el).unwrap(), large.as_bytes());
    }

    /// An incompressible file requested as Deflate falls back to Stored (method 0)
    /// so the entry is never inflated — and still round-trips.
    #[test]
    fn writer_incompressible_falls_back_to_stored() {
        // Deterministic pseudo-random bytes (LCG) — statistically incompressible.
        let mut state: u64 = 0xC0FF_EE12_3456_789A;
        let mut data = Vec::with_capacity(2048);
        for _ in 0..2048 {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            data.push((state >> 33) as u8);
        }

        let mut w = ZipWriter::new();
        w.add_file("random.bin", &data, Method::Deflate).unwrap();
        let bytes = w.finish().unwrap();

        let ar = Archive::open(&bytes).unwrap();
        let e = ar.find("random.bin").unwrap();
        // The deflate-vs-store decision must have chosen Stored (DEFLATE would not
        // shrink random data below its length).
        assert_eq!(
            e.method, 0,
            "incompressible data requested as Deflate must store, not inflate"
        );
        assert_eq!(e.compressed_size, e.size);
        assert_eq!(ar.read_entry(e).unwrap(), data);
    }

    /// A subdirectory-path name ("word/document.xml") round-trips intact — the
    /// shape an OOXML (.docx/.xlsx) writer relies on.
    #[test]
    fn writer_subdir_path_roundtrips() {
        let ct = br#"<?xml version="1.0"?><Types/>"#.as_slice();
        let doc = "<w:document><w:body/></w:document>".repeat(10);

        let mut w = ZipWriter::new();
        w.add_file("[Content_Types].xml", ct, Method::Deflate)
            .unwrap();
        w.add_file("word/document.xml", doc.as_bytes(), Method::Deflate)
            .unwrap();
        let bytes = w.finish().unwrap();

        let ar = Archive::open(&bytes).unwrap();
        let e = ar.find("word/document.xml").expect("subdir name preserved");
        assert_eq!(e.name, "word/document.xml");
        assert_eq!(ar.read_entry(e).unwrap(), doc.as_bytes());
        // The OOXML-mandatory part is present and readable.
        let c = ar
            .find("[Content_Types].xml")
            .expect("content types present");
        assert_eq!(ar.read_entry(c).unwrap(), ct);
    }

    /// Over-limit: more than 65535 entries → graceful Err, never a corrupt EOCD.
    #[test]
    fn writer_too_many_entries_errs() {
        let mut w = ZipWriter::new();
        // Fill exactly to the limit with tiny stored entries.
        for i in 0..MAX_WRITE_ENTRIES {
            // Distinct short names; content empty to keep the test fast/small.
            let name = alloc_name(i);
            w.add_file_stored(&name, b"")
                .expect("entry under the limit");
        }
        assert_eq!(w.len(), MAX_WRITE_ENTRIES);
        // The next entry must be rejected.
        let over = w.add_file_stored("one-too-many", b"");
        assert_eq!(over, Err(ZipError::TooLarge));
        // And the writer stays poisoned: finish also returns the error.
        assert_eq!(w.finish(), Err(ZipError::TooLarge));
    }

    fn alloc_name(i: usize) -> std::string::String {
        let mut s = std::string::String::from("f");
        // Base-36-ish unique suffix; plain decimal is fine and unique.
        s.push_str(&i.to_string());
        s
    }

    /// Over-limit: a single entry whose declared size ≥ 4 GiB needs ZIP64 → Err.
    /// We cannot allocate 4 GiB in a test, so we exercise the bound directly by
    /// checking the constant and the rejection path with a crafted oversized len
    /// claim is unreachable via the public API without the data — instead assert
    /// the writer accepts the largest *feasible* small entry and that the constant
    /// is the documented 32-bit ceiling. (The count-limit test above proves the
    /// poison/Err mechanism that the size check shares.)
    #[test]
    fn writer_size_limit_is_32bit() {
        assert_eq!(MAX_WRITE_ENTRY_SIZE, 0xFFFF_FFFF);
        assert_eq!(MAX_WRITE_ENTRIES, 0xFFFF);
        assert_eq!(MAX_ARCHIVE_SIZE, 0xFFFF_FFFF);
        // A normal entry well under the ceiling is accepted.
        let mut w = ZipWriter::new();
        assert!(w.add_file_stored("ok", &vec![0u8; 1024]).is_ok());
        assert!(w.finish().is_ok());
    }

    /// Cross-check: the EOCD's CD offset/size are self-consistent with where the
    /// central directory actually sits in the writer's output, and the reader's
    /// own structural validation (which compares these) accepts it.
    #[test]
    fn writer_eocd_offsets_self_consistent() {
        let mut w = ZipWriter::new();
        w.add_file_stored("a.txt", b"alpha").unwrap();
        w.add_file_deflate("b.txt", &vec![b'b'; 1000]).unwrap();
        let bytes = w.finish().unwrap();

        // Locate the EOCD the same way the reader does.
        let eocd = find_eocd(&bytes).expect("eocd present");
        let cd_size = rd_u32(&bytes, eocd + 12).unwrap() as usize;
        let cd_offset = rd_u32(&bytes, eocd + 16).unwrap() as usize;
        // The central directory must start with a central signature...
        assert_eq!(rd_u32(&bytes, cd_offset).unwrap(), SIG_CENTRAL);
        // ...and end exactly where the EOCD begins.
        assert_eq!(cd_offset + cd_size, eocd);
        // And the count field matches.
        assert_eq!(rd_u16(&bytes, eocd + 10).unwrap(), 2);

        // The reader's structural checks accept it (it independently validates
        // cd_end <= len and cd_offset <= eocd_off).
        let ar = Archive::open(&bytes).expect("structurally valid");
        assert_eq!(ar.entries().len(), 2);
    }

    /// An empty archive (no entries) is valid and the reader sees zero entries.
    #[test]
    fn writer_empty_archive() {
        let w = ZipWriter::new();
        assert!(w.is_empty());
        let bytes = w.finish().unwrap();
        let ar = Archive::open(&bytes).expect("empty archive opens");
        assert_eq!(ar.entries().len(), 0);
    }
}
