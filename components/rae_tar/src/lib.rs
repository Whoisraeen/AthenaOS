//! # RaeTar — a never-panic, `no_std` TAR + gzip archive reader (.tar / .tar.gz / .tgz).
//!
//! RaeenOS_Concept.md §"The user owns the machine": a daily driver must let
//! someone double-click a downloaded `.tar.gz` and see (and extract) what's inside
//! *without* installing a third-party tool. `.tar.gz` is the canonical shape of a
//! source release, a language toolchain tarball, a container layer, and most
//! POSIX-world downloads — so one correct, dependency-free, hostile-input TAR core
//! (with its gzip front end) is foundational daily-driver infrastructure. It is
//! deliberately wired into no consumer this slice; the Files "extract here" action
//! for `.tar.gz` is the follow-up (apps lane).
//!
//! This crate is the TAR counterpart to [`rae_zip`](../rae_zip) and is fully
//! self-contained: the gzip layer reimplements DEFLATE `inflate` (RFC 1951) and
//! CRC-32 from scratch rather than depending on `rae_zip` or `raemedia`.
//!
//! ## Hostile-input posture (CLAUDE: decoders of untrusted bytes are an RCE surface)
//! Every byte handed to [`read_tar`] / [`read_tar_gz`] / [`decode_gzip`] is treated
//! as hostile — a downloaded tarball is attacker-controlled. There is **no
//! `unwrap`/`expect`/`panic`/raw-index-panic path** reachable from this crate:
//! truncated headers, bad octal fields, a wrong header checksum, a corrupt DEFLATE
//! stream, a CRC/ISIZE mismatch, and an absurdly large declared size all return
//! `Err(TarError)`. Two amplification attacks are bounded *before* allocation:
//!   - a **decompression / archive bomb** (a tiny gzip claiming to expand to
//!     gigabytes, or a tar header claiming a multi-gigabyte member) is rejected by
//!     [`MAX_TOTAL_SIZE`] / [`MAX_ENTRY_SIZE`] / [`MAX_ENTRIES`] before the bytes
//!     are materialized, and the inflate loop re-checks running output; and
//!   - a **path-traversal** name (`../`, absolute, or a `C:\` drive prefix) is
//!     flagged by [`is_safe_path`], which any extractor MUST consult before
//!     writing an entry to disk.
//!
//! The host KAT suite at the bottom of this file is the primary proof
//! (`cargo test -p rae_tar`): it builds ustar tars in-test (dir + regular file +
//! long names), gzips a known string with a from-scratch encoder, round-trips a
//! gzipped tar end-to-end, flips a CRC byte to prove the gzip CRC check is
//! load-bearing, breaks an octal size to prove the size parse is load-bearing, and
//! runs a battery of hostile inputs that must all return `Err` with zero panics.
//!
//! ## What it supports
//! - **ustar / POSIX tar**: 512-byte header blocks, `name` + ustar `prefix` for
//!   long paths, octal `size`/`mode`/`mtime`, the standard `typeflag` set
//!   (regular `0`/`\0`, directory `5`, symlink `2`, hardlink `1`), the GNU long-name
//!   (`L`) / long-link (`K`) extensions, and PAX (`x`/`g`) extended-header records
//!   (`path` / `linkpath` / `size` overrides). Header checksum verified (signed and
//!   unsigned tolerance). Two zero blocks = end of archive.
//! - **gzip** (RFC 1952): magic `1F 8B`, header flags (FEXTRA/FNAME/FCOMMENT/FHCRC),
//!   DEFLATE body (from-scratch fixed + dynamic Huffman, RFC 1951), trailing CRC-32
//!   and ISIZE both verified.
//!
//! ## What it does NOT support (clean `Err`, never a panic)
//! - Any gzip-internal compression method other than DEFLATE (CM != 8) → [`TarError::Unsupported`].
//! - Other tar-stream compressors (bzip2/xz/zstd) — callers detect those by
//!   extension and route elsewhere; this crate handles raw tar and gzip-wrapped tar.
//! - The old V7 tar layout's idiosyncrasies beyond what ustar covers; an
//!   unrecognized typeflag is preserved as [`TarKind::Other`] rather than guessed.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

// ─── Limits (bomb guards, applied before/while materializing bytes) ──────────

/// Largest size we will materialize for a single tar member: 1 GiB. A header
/// claiming more is rejected without reading that member's body.
pub const MAX_ENTRY_SIZE: u64 = 1024 * 1024 * 1024;

/// Largest total decompressed output we will produce from a gzip stream, and the
/// largest tar we will fully enumerate: 2 GiB. Bounds a gzip decompression bomb
/// and a tar that claims an unbounded run of members.
pub const MAX_TOTAL_SIZE: u64 = 2u64 * 1024 * 1024 * 1024;

/// Largest number of tar entries we will enumerate. A crafted archive cannot make
/// us spin or over-allocate the entry list.
pub const MAX_ENTRIES: usize = 1 << 20; // ~1M

/// Largest gzip:original expansion ratio we trust as a sanity bound on a body that
/// declares no size (ISIZE is only a low 32-bit value, so we cap independently).
/// DEFLATE's theoretical max is ~1032:1; a classic bomb claims millions:1.
pub const MAX_RATIO: u64 = 4000;

const BLOCK: usize = 512;

// ─── Errors ─────────────────────────────────────────────────────────────────

/// A TAR/gzip read error. Every variant is a *handled* path — nothing here panics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TarError {
    /// Buffer is too small / doesn't look like a tar (or, for gzip, lacks the magic).
    NotTar,
    /// A header/field/record/body ran past the end of the buffer.
    Truncated,
    /// A header checksum did not match (signed or unsigned) — not a valid header.
    BadChecksum,
    /// An octal numeric field (size/mode/mtime) was malformed.
    BadOctal,
    /// A name/field was not valid UTF-8 (we do not lossily guess hostile names).
    BadUtf8,
    /// gzip/tar feature this reader doesn't implement (e.g. gzip CM != DEFLATE).
    Unsupported,
    /// A declared size, total output, or entry count exceeds the bomb bounds.
    TooLarge,
    /// gzip magic was absent where a gzip stream was required.
    NotGzip,
    /// The DEFLATE stream was corrupt or truncated.
    InflateError,
    /// The gzip trailing CRC-32 didn't match the decompressed bytes.
    BadCrc,
    /// The gzip trailing ISIZE didn't match the decompressed length (mod 2^32).
    BadIsize,
}

// ─── CRC-32 (IEEE 802.3 / zlib polynomial 0xEDB88320, reflected) ────────────

/// Compute the IEEE CRC-32 of `data` (the algorithm gzip, ZIP, and PNG use).
///
/// Reflected input/output, init `0xFFFFFFFF`, final XOR `0xFFFFFFFF`. Computed
/// bit-by-bit (no static table) to keep the crate allocation- and data-free; it is
/// run once per gzip stream, not on a hot path.
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
/// A tar entry name is attacker-controlled; writing it verbatim is how a malicious
/// archive escapes its destination directory ("tar slip"). An extractor MUST call
/// this before creating any file. We reject:
///   - empty names,
///   - absolute POSIX paths (leading `/` or `\`),
///   - Windows drive prefixes (`C:\`, `c:/`, even bare `C:foo`),
///   - any path containing a `..` path component (in either slash direction),
///   - NUL bytes.
///
/// Forward and back slashes are both treated as separators (tar nominally uses
/// `/`, but hostile archives use `\` to dodge naive `/`-only checks).
pub fn is_safe_path(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    let bytes = name.as_bytes();
    if bytes.contains(&0) {
        return false;
    }
    // Absolute POSIX / backslash root.
    if bytes[0] == b'/' || bytes[0] == b'\\' {
        return false;
    }
    // Windows drive letter: "X:" prefix.
    if bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic() {
        return false;
    }
    // Any component equal to ".." escapes the root. Split on both separators.
    for component in name.split(['/', '\\']) {
        if component == ".." {
            return false;
        }
    }
    true
}

// ─── Public TAR model ────────────────────────────────────────────────────────

/// The kind of a tar entry, derived from its `typeflag`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TarKind {
    /// A regular file (typeflag `0` or NUL). Carries [`TarEntry::data`].
    File,
    /// A directory (typeflag `5`).
    Dir,
    /// A symbolic link (typeflag `2`); [`TarEntry::link_target`] is the target.
    Symlink,
    /// A hard link (typeflag `1`); [`TarEntry::link_target`] is the target.
    Hardlink,
    /// Any other typeflag we recognize as a real header but don't model (FIFO,
    /// char/block device, etc.). The raw flag byte is preserved.
    Other(u8),
}

/// One entry from a tar archive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TarEntry {
    /// The entry's path as stored (raw, possibly unsafe — callers MUST run it
    /// through [`is_safe_path`] before extracting to disk). GNU long-name / PAX
    /// `path` overrides and the ustar `prefix` join are already applied.
    pub name: String,
    /// Logical size of the member in bytes (PAX `size` override applied).
    pub size: u64,
    /// The entry kind (from the typeflag).
    pub kind: TarKind,
    /// POSIX mode bits (from the octal `mode` field).
    pub mode: u32,
    /// Modification time (POSIX seconds, from the octal `mtime` field).
    pub mtime: u64,
    /// For symlink/hardlink entries, the link target (raw); empty otherwise.
    pub link_target: String,
    /// The member's bytes (regular files only; empty for dirs/links/others).
    pub data: Vec<u8>,
}

impl TarEntry {
    /// Whether this entry is a regular file.
    pub fn is_file(&self) -> bool {
        matches!(self.kind, TarKind::File)
    }
    /// Whether this entry is a directory.
    pub fn is_dir(&self) -> bool {
        matches!(self.kind, TarKind::Dir)
    }
    /// The entry's bytes (regular files only).
    pub fn data(&self) -> &[u8] {
        &self.data
    }
}

/// A parsed tar archive — the in-memory list of its entries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TarArchive {
    /// All entries, in archive order. Pseudo-entries (GNU longname/longlink, PAX
    /// `x`/`g` headers) are consumed during parsing and never appear here.
    pub entries: Vec<TarEntry>,
}

impl TarArchive {
    /// All entries, in archive order.
    pub fn entries(&self) -> &[TarEntry] {
        &self.entries
    }
    /// Find an entry by exact name.
    pub fn find(&self, name: &str) -> Option<&TarEntry> {
        self.entries.iter().find(|e| e.name == name)
    }
}

// ─── ustar header field readers ──────────────────────────────────────────────

/// Read a NUL/space-terminated string field from a header block, validating UTF-8.
fn read_str(block: &[u8], off: usize, len: usize) -> Result<String, TarError> {
    let raw = block.get(off..off + len).ok_or(TarError::Truncated)?;
    // Field ends at the first NUL (and we also drop a trailing space pad).
    let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
    let slice = &raw[..end];
    match core::str::from_utf8(slice) {
        Ok(s) => Ok(s.to_string()),
        Err(_) => Err(TarError::BadUtf8),
    }
}

/// Parse an octal numeric field. tar fields are ASCII octal, NUL/space terminated
/// and space/zero padded. An empty field reads as 0. Also handles the GNU base-256
/// binary extension (high bit of the first byte set) for large sizes.
fn read_octal(block: &[u8], off: usize, len: usize) -> Result<u64, TarError> {
    let raw = block.get(off..off + len).ok_or(TarError::Truncated)?;

    // GNU base-256 binary encoding: top bit of byte 0 set. The remaining bits of
    // byte 0 plus the following bytes are a big-endian magnitude.
    if !raw.is_empty() && (raw[0] & 0x80) != 0 {
        let mut val: u64 = (raw[0] & 0x7f) as u64;
        for &b in &raw[1..] {
            val = val.checked_shl(8).ok_or(TarError::TooLarge)?;
            val |= b as u64;
        }
        return Ok(val);
    }

    let mut val: u64 = 0;
    let mut seen = false;
    for &b in raw {
        match b {
            b'0'..=b'7' => {
                seen = true;
                val = val.checked_mul(8).ok_or(TarError::TooLarge)?;
                val = val
                    .checked_add((b - b'0') as u64)
                    .ok_or(TarError::TooLarge)?;
            }
            b' ' | 0 => {
                // Padding / terminator: stop on the first one after digits, or skip
                // leading pad before any digit.
                if seen {
                    break;
                }
            }
            _ => return Err(TarError::BadOctal),
        }
    }
    Ok(val)
}

/// Verify a tar header's checksum. The checksum field (offset 148, len 8) is summed
/// as if it were all spaces. Both the signed and unsigned interpretation are
/// accepted (historical tars differ), to avoid false rejects on valid archives.
fn checksum_ok(block: &[u8]) -> Result<bool, TarError> {
    if block.len() < BLOCK {
        return Err(TarError::Truncated);
    }
    let stored = read_octal(block, 148, 8)?;
    let mut unsigned: u64 = 0;
    let mut signed: i64 = 0;
    for (i, &b) in block[..BLOCK].iter().enumerate() {
        let v = if (148..156).contains(&i) { b' ' } else { b };
        unsigned += v as u64;
        signed += (v as i8) as i64;
    }
    Ok(stored == unsigned || stored as i64 == signed)
}

/// Round `n` up to the next 512-byte block boundary.
fn round_up(n: usize) -> Option<usize> {
    n.checked_add(BLOCK - 1).map(|v| (v / BLOCK) * BLOCK)
}

/// Is this 512-byte block entirely zero (an end-of-archive marker)?
fn is_zero_block(block: &[u8]) -> bool {
    block.len() >= BLOCK && block[..BLOCK].iter().all(|&b| b == 0)
}

// ─── Pending overrides from pseudo-headers (GNU L/K, PAX x/g) ────────────────

#[derive(Default)]
struct Pending {
    /// GNU `L` longname or PAX `path` override for the next real entry.
    name: Option<String>,
    /// GNU `K` longlink or PAX `linkpath` override for the next real entry.
    link: Option<String>,
    /// PAX `size` override for the next real entry.
    size: Option<u64>,
}

impl Pending {
    fn take_name(&mut self) -> Option<String> {
        self.name.take()
    }
    fn take_link(&mut self) -> Option<String> {
        self.link.take()
    }
    fn take_size(&mut self) -> Option<u64> {
        self.size.take()
    }
    fn clear(&mut self) {
        self.name = None;
        self.link = None;
        self.size = None;
    }
}

/// Parse PAX extended-header records ("len key=value\n" repeated) for the keys we
/// honor (`path`, `linkpath`, `size`). Unknown keys are skipped. Hostile/malformed
/// records yield an error rather than a panic.
fn parse_pax(records: &[u8], pending: &mut Pending) -> Result<(), TarError> {
    let mut pos = 0usize;
    while pos < records.len() {
        // Each record: "<decimal length> <key>=<value>\n", length covers the whole
        // record including its own digits and the trailing newline.
        let space = records[pos..]
            .iter()
            .position(|&b| b == b' ')
            .ok_or(TarError::Truncated)?;
        let len_str =
            core::str::from_utf8(&records[pos..pos + space]).map_err(|_| TarError::BadUtf8)?;
        let rec_len: usize = len_str.parse().map_err(|_| TarError::BadOctal)?;
        if rec_len == 0 || pos.checked_add(rec_len).map_or(true, |e| e > records.len()) {
            return Err(TarError::Truncated);
        }
        let rec = &records[pos..pos + rec_len];
        // Content after "<len> " up to the trailing '\n'.
        let content_start = space + 1;
        if content_start >= rec.len() {
            return Err(TarError::Truncated);
        }
        let mut content = &rec[content_start..];
        // Drop the trailing newline if present.
        if let Some(&last) = content.last() {
            if last == b'\n' {
                content = &content[..content.len() - 1];
            }
        }
        let eq = content.iter().position(|&b| b == b'=');
        if let Some(eq) = eq {
            let key = &content[..eq];
            let value = &content[eq + 1..];
            match key {
                b"path" => {
                    pending.name = Some(
                        core::str::from_utf8(value)
                            .map_err(|_| TarError::BadUtf8)?
                            .to_string(),
                    )
                }
                b"linkpath" => {
                    pending.link = Some(
                        core::str::from_utf8(value)
                            .map_err(|_| TarError::BadUtf8)?
                            .to_string(),
                    )
                }
                b"size" => {
                    let s = core::str::from_utf8(value).map_err(|_| TarError::BadUtf8)?;
                    let n: u64 = s.parse().map_err(|_| TarError::BadOctal)?;
                    pending.size = Some(n);
                }
                _ => { /* unknown key: ignore */ }
            }
        }
        pos += rec_len;
    }
    Ok(())
}

// ─── TAR parsing ─────────────────────────────────────────────────────────────

/// Parse a raw (uncompressed) tar archive from its full byte slice.
///
/// Never panics. Returns `Err` on a bad checksum, truncated header/body, malformed
/// octal field, oversized declaration, or too many entries.
pub fn read_tar(data: &[u8]) -> Result<TarArchive, TarError> {
    if data.len() < BLOCK {
        return Err(TarError::NotTar);
    }

    let mut entries: Vec<TarEntry> = Vec::new();
    let mut pending = Pending::default();
    let mut pos = 0usize;
    let mut zero_seen = false;
    let mut total: u64 = 0;
    // True once we have accepted at least one valid header — distinguishes "this is
    // not a tar at all" (first block bad) from "tar got corrupt mid-stream".
    let mut any_valid = false;

    while pos + BLOCK <= data.len() {
        let block = &data[pos..pos + BLOCK];

        if is_zero_block(block) {
            if zero_seen {
                // Two consecutive zero blocks: clean end of archive.
                return Ok(TarArchive { entries });
            }
            zero_seen = true;
            pos += BLOCK;
            continue;
        }
        zero_seen = false;

        if !checksum_ok(block)? {
            // A non-zero block with a bad checksum is not a valid header. If we've
            // never accepted one, the input simply isn't a tar; otherwise it's a
            // corrupt/garbage tail.
            return if any_valid {
                Err(TarError::BadChecksum)
            } else {
                Err(TarError::NotTar)
            };
        }
        any_valid = true;

        let typeflag = block[156];
        let size = read_octal(block, 124, 12)?;
        let mode = read_octal(block, 100, 8)? as u32;
        let mtime = read_octal(block, 136, 12)?;

        // The member body follows the header, padded to a block boundary.
        let body_start = pos + BLOCK;
        let body_len = size as usize;
        let body_end = body_start
            .checked_add(body_len)
            .ok_or(TarError::Truncated)?;
        if size > MAX_ENTRY_SIZE {
            return Err(TarError::TooLarge);
        }
        if body_end > data.len() {
            return Err(TarError::Truncated);
        }
        let body = &data[body_start..body_end];
        let padded = round_up(body_len).ok_or(TarError::Truncated)?;
        let next = body_start.checked_add(padded).ok_or(TarError::Truncated)?;

        match typeflag {
            b'L' => {
                // GNU long name: the body is the name for the NEXT entry.
                let name = nul_trim_utf8(body)?;
                pending.name = Some(name);
                pos = next;
                continue;
            }
            b'K' => {
                // GNU long link target for the NEXT entry.
                let link = nul_trim_utf8(body)?;
                pending.link = Some(link);
                pos = next;
                continue;
            }
            b'x' | b'g' => {
                // PAX extended header (per-file 'x' or global 'g'). Both update the
                // pending overrides for the next real entry.
                parse_pax(body, &mut pending)?;
                pos = next;
                continue;
            }
            _ => {}
        }

        // A real entry. Resolve its name: pending override > ustar prefix+name.
        let name = match pending.take_name() {
            Some(n) => n,
            None => {
                let base = read_str(block, 0, 100)?;
                // ustar prefix (offset 345, len 155) — only when the magic says ustar.
                let magic = &block[257..263];
                let prefix = if magic.starts_with(b"ustar") {
                    read_str(block, 345, 155)?
                } else {
                    String::new()
                };
                if prefix.is_empty() {
                    base
                } else {
                    let mut full = prefix;
                    full.push('/');
                    full.push_str(&base);
                    full
                }
            }
        };

        let link_target = match pending.take_link() {
            Some(l) => l,
            None => read_str(block, 157, 100)?,
        };

        let eff_size = pending.take_size().unwrap_or(size);
        pending.clear();

        let kind = match typeflag {
            b'0' | 0 => TarKind::File,
            b'5' => TarKind::Dir,
            b'2' => TarKind::Symlink,
            b'1' => TarKind::Hardlink,
            other => TarKind::Other(other),
        };

        // Only regular files carry data.
        let entry_data = if matches!(kind, TarKind::File) {
            body.to_vec()
        } else {
            Vec::new()
        };

        total = total.saturating_add(entry_data.len() as u64);
        if total > MAX_TOTAL_SIZE {
            return Err(TarError::TooLarge);
        }

        entries.push(TarEntry {
            name,
            size: eff_size,
            kind,
            mode,
            mtime,
            link_target,
            data: entry_data,
        });
        if entries.len() > MAX_ENTRIES {
            return Err(TarError::TooLarge);
        }

        pos = next;
    }

    // Ran off the end without two zero blocks. If we parsed real entries, accept
    // what we have (common for tools that omit the terminator); otherwise it's not
    // a tar / is truncated.
    if any_valid && !entries.is_empty() {
        Ok(TarArchive { entries })
    } else if any_valid {
        // Headers were valid but produced no entries (e.g. only pseudo-headers) and
        // we hit the end without the terminator.
        Err(TarError::Truncated)
    } else {
        Err(TarError::NotTar)
    }
}

/// Trim a NUL-padded byte buffer (GNU L/K bodies) and validate UTF-8.
fn nul_trim_utf8(body: &[u8]) -> Result<String, TarError> {
    let end = body.iter().position(|&b| b == 0).unwrap_or(body.len());
    core::str::from_utf8(&body[..end])
        .map(|s| s.to_string())
        .map_err(|_| TarError::BadUtf8)
}

/// Auto-detect a gzip wrapper (`1F 8B` magic) and route accordingly: gunzip first,
/// then parse the tar; or parse a raw tar directly.
pub fn read_tar_gz(data: &[u8]) -> Result<TarArchive, TarError> {
    if data.len() >= 2 && data[0] == 0x1F && data[1] == 0x8B {
        let inflated = decode_gzip(data)?;
        read_tar(&inflated)
    } else {
        read_tar(data)
    }
}

/// gunzip then parse-tar — the explicit `.tar.gz`/`.tgz` path (requires the gzip
/// magic; a non-gzip input is [`TarError::NotGzip`]).
pub fn decode_tar_gz(data: &[u8]) -> Result<TarArchive, TarError> {
    let inflated = decode_gzip(data)?;
    read_tar(&inflated)
}

// ─── gzip layer (RFC 1952) ───────────────────────────────────────────────────

/// Decode a gzip stream to its original bytes, verifying the trailing CRC-32 and
/// ISIZE. Never panics; every malformed-input path returns `Err`.
pub fn decode_gzip(data: &[u8]) -> Result<Vec<u8>, TarError> {
    // Header: ID1 ID2 CM FLG MTIME(4) XFL OS, then optional fields, then DEFLATE,
    // then CRC32(4) + ISIZE(4).
    if data.len() < 18 {
        // 10-byte header + at least a tiny body + 8-byte trailer.
        return Err(TarError::NotGzip);
    }
    if data[0] != 0x1F || data[1] != 0x8B {
        return Err(TarError::NotGzip);
    }
    if data[2] != 0x08 {
        // CM must be 8 (DEFLATE).
        return Err(TarError::Unsupported);
    }
    let flg = data[3];
    let mut pos = 10usize;

    // FEXTRA (bit 2): XLEN(2) + XLEN bytes.
    if flg & 0x04 != 0 {
        let xlen = *data.get(pos).ok_or(TarError::Truncated)? as usize
            | ((*data.get(pos + 1).ok_or(TarError::Truncated)? as usize) << 8);
        pos = pos.checked_add(2 + xlen).ok_or(TarError::Truncated)?;
    }
    // FNAME (bit 3): NUL-terminated.
    if flg & 0x08 != 0 {
        pos = skip_cstr(data, pos)?;
    }
    // FCOMMENT (bit 4): NUL-terminated.
    if flg & 0x10 != 0 {
        pos = skip_cstr(data, pos)?;
    }
    // FHCRC (bit 1): 2-byte header CRC.
    if flg & 0x02 != 0 {
        pos = pos.checked_add(2).ok_or(TarError::Truncated)?;
    }

    // The trailer is the last 8 bytes; the DEFLATE body is everything between.
    if data.len() < pos + 8 {
        return Err(TarError::Truncated);
    }
    let body = &data[pos..data.len() - 8];
    let trailer = &data[data.len() - 8..];
    let want_crc = u32::from_le_bytes([trailer[0], trailer[1], trailer[2], trailer[3]]);
    let want_isize = u32::from_le_bytes([trailer[4], trailer[5], trailer[6], trailer[7]]);

    // Ratio guard against a tiny stream claiming a huge ISIZE — independent of the
    // hard MAX_TOTAL_SIZE cap inside inflate.
    if !body.is_empty() {
        let claimed = want_isize as u64;
        if claimed / (body.len() as u64) > MAX_RATIO && claimed > 64 * 1024 {
            return Err(TarError::TooLarge);
        }
    }

    let out = inflate(body)?;

    if crc32(&out) != want_crc {
        return Err(TarError::BadCrc);
    }
    if (out.len() as u32) != want_isize {
        return Err(TarError::BadIsize);
    }
    Ok(out)
}

/// Skip a NUL-terminated C string starting at `pos`, returning the index just past
/// the NUL. Errors if no NUL is found before EOF.
fn skip_cstr(data: &[u8], pos: usize) -> Result<usize, TarError> {
    let rel = data
        .get(pos..)
        .ok_or(TarError::Truncated)?
        .iter()
        .position(|&b| b == 0)
        .ok_or(TarError::Truncated)?;
    pos.checked_add(rel + 1).ok_or(TarError::Truncated)
}

// ─── DEFLATE inflate (RFC 1951) — from-scratch, self-contained ──────────────
//
// Reimplemented here (not shared with rae_zip) so this crate has zero deps. gzip
// stores the *raw* DEFLATE stream (the zlib 2-byte wrapper is gzip-specific and
// already stripped above), so `inflate` consumes the bitstream directly.

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

    fn bit(&mut self) -> Result<u32, TarError> {
        let byte = *self.data.get(self.byte_pos).ok_or(TarError::InflateError)?;
        let b = (byte >> self.bit_pos) & 1;
        self.bit_pos += 1;
        if self.bit_pos == 8 {
            self.bit_pos = 0;
            self.byte_pos += 1;
        }
        Ok(b as u32)
    }

    fn bits(&mut self, n: u32) -> Result<u32, TarError> {
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

    fn read_byte(&mut self) -> Result<u8, TarError> {
        let b = *self.data.get(self.byte_pos).ok_or(TarError::InflateError)?;
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
    fn from_lengths(lengths: &[u8]) -> Result<Self, TarError> {
        let mut counts = [0u16; 16];
        for &l in lengths {
            if l as usize > 15 {
                return Err(TarError::InflateError);
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
                    return Err(TarError::InflateError);
                }
                symbols[idx] = sym as u16;
                offsets[l as usize] += 1;
            }
        }
        Ok(Self { counts, symbols })
    }

    fn decode(&self, br: &mut BitReader) -> Result<u16, TarError> {
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
                    .ok_or(TarError::InflateError);
            }
            index += count;
            first += count;
            first <<= 1;
            code <<= 1;
        }
        Err(TarError::InflateError)
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

/// Inflate a raw DEFLATE stream. Output is hard-capped at [`MAX_TOTAL_SIZE`] so a
/// corrupt/hostile stream can never be coerced into unbounded growth.
fn inflate(data: &[u8]) -> Result<Vec<u8>, TarError> {
    let cap = MAX_TOTAL_SIZE as usize;
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
            _ => return Err(TarError::InflateError), // btype 3 reserved
        }
        if bfinal == 1 {
            break;
        }
        if out.len() > cap {
            return Err(TarError::InflateError);
        }
    }
    Ok(out)
}

fn inflate_stored(br: &mut BitReader, out: &mut Vec<u8>, cap: usize) -> Result<(), TarError> {
    br.align_to_byte();
    let len = (br.read_byte()? as u16) | ((br.read_byte()? as u16) << 8);
    let nlen = (br.read_byte()? as u16) | ((br.read_byte()? as u16) << 8);
    if len != !nlen {
        return Err(TarError::InflateError);
    }
    if out.len() + len as usize > cap {
        return Err(TarError::InflateError);
    }
    for _ in 0..len {
        out.push(br.read_byte()?);
    }
    Ok(())
}

fn fixed_litlen() -> Result<Huffman, TarError> {
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

fn fixed_dist() -> Result<Huffman, TarError> {
    let lengths = [5u8; 30];
    Huffman::from_lengths(&lengths)
}

fn read_dynamic_tables(br: &mut BitReader) -> Result<(Huffman, Huffman), TarError> {
    let hlit = br.bits(5)? as usize + 257;
    let hdist = br.bits(5)? as usize + 1;
    let hclen = br.bits(4)? as usize + 4;
    if hlit > 286 || hdist > 30 {
        return Err(TarError::InflateError);
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
                    return Err(TarError::InflateError);
                }
                let prev = lengths[i - 1];
                let repeat = br.bits(2)? as usize + 3;
                for _ in 0..repeat {
                    if i >= total {
                        return Err(TarError::InflateError);
                    }
                    lengths[i] = prev;
                    i += 1;
                }
            }
            17 => {
                let repeat = br.bits(3)? as usize + 3;
                for _ in 0..repeat {
                    if i >= total {
                        return Err(TarError::InflateError);
                    }
                    lengths[i] = 0;
                    i += 1;
                }
            }
            18 => {
                let repeat = br.bits(7)? as usize + 11;
                for _ in 0..repeat {
                    if i >= total {
                        return Err(TarError::InflateError);
                    }
                    lengths[i] = 0;
                    i += 1;
                }
            }
            _ => return Err(TarError::InflateError),
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
) -> Result<(), TarError> {
    loop {
        let sym = litlen.decode(br)?;
        if sym == 256 {
            return Ok(());
        } else if sym < 256 {
            if out.len() + 1 > cap {
                return Err(TarError::InflateError);
            }
            out.push(sym as u8);
        } else {
            let li = (sym - 257) as usize;
            if li >= LENGTH_BASE.len() {
                return Err(TarError::InflateError);
            }
            let length = LENGTH_BASE[li] as usize + br.bits(LENGTH_EXTRA[li] as u32)? as usize;
            let dsym = dist.decode(br)? as usize;
            if dsym >= DIST_BASE.len() {
                return Err(TarError::InflateError);
            }
            let distance = DIST_BASE[dsym] as usize + br.bits(DIST_EXTRA[dsym] as u32)? as usize;
            if distance == 0 || distance > out.len() {
                return Err(TarError::InflateError);
            }
            if out.len() + length > cap {
                return Err(TarError::InflateError);
            }
            let start = out.len() - distance;
            for k in 0..length {
                let b = out[start + k];
                out.push(b);
            }
        }
        if out.len() > cap {
            return Err(TarError::InflateError);
        }
    }
}

// ════════════════════════════════════════════════════════════════════════════
// TAR WRITER — ustar encoder (+ gzip convenience) ────────────────────────────
//
// RaeenOS_Concept.md §"The user owns the machine": owning your data means being
// able to *export* and *back up* it, not just read what someone else produced.
// `TarWriter` produces a spec-correct ustar archive that this crate's own
// `read_tar` reads back identically — the writer/reader round-trip is the proof
// that the header layout and (load-bearing) checksum are correct, because the
// reader rejects a wrong checksum with `Err(BadChecksum)`. `finish_gz` wraps the
// tar in real gzip framing via `rae_deflate::gzip_compress`, yielding a `.tar.gz`
// that `decode_gzip` (this crate) and `gzip_decompress` (rae_deflate) gunzip back
// to the exact `.tar` bytes.
// ════════════════════════════════════════════════════════════════════════════

/// A write error from [`TarWriter`]. Distinct from the read-side [`TarError`] so
/// a caller never confuses "I could not build this archive" with "I could not
/// parse that archive". Every variant is a *handled* refusal — the writer never
/// emits a corrupt archive; it returns `Err` instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TarWriteError {
    /// An entry name exceeds what a ustar `name` field can hold (100 bytes) and
    /// could not be split into the ustar `prefix` (155) + `name` (100) layout
    /// (no `/` boundary that fits, or the whole path is longer than 255 bytes).
    /// RaeTar's writer does not emit GNU/PAX long-name pseudo-headers — it
    /// refuses rather than silently truncate. (The *reader* still accepts GNU/PAX
    /// long names produced by other tools.)
    NameTooLong,
    /// An entry name is empty, contains a NUL byte, or (for a link) the link
    /// target is empty / too long for the 100-byte ustar `linkname` field.
    BadName,
    /// A single member's body exceeds [`MAX_ENTRY_SIZE`].
    EntryTooLarge,
    /// Adding this entry would push the archive past [`MAX_TOTAL_SIZE`] of body
    /// bytes, or past [`MAX_ENTRIES`] members.
    ArchiveTooLarge,
}

/// Metadata for a written entry. All fields have sane defaults via
/// [`TarMeta::default`]; callers override only what they care about.
#[derive(Debug, Clone, Copy)]
pub struct TarMeta {
    /// POSIX mode bits. Default `0o644` for files, `0o755` for dirs (the writer
    /// substitutes the dir default when the caller leaves the file default).
    pub mode: u32,
    /// Owner uid. Default 0.
    pub uid: u32,
    /// Group gid. Default 0.
    pub gid: u32,
    /// Modification time (POSIX seconds). Default 0 (epoch) — deterministic
    /// output, which makes the round-trip KATs reproducible.
    pub mtime: u64,
}

impl Default for TarMeta {
    fn default() -> Self {
        Self {
            mode: 0o644,
            uid: 0,
            gid: 0,
            mtime: 0,
        }
    }
}

/// An incremental ustar archive builder.
///
/// ```ignore
/// let mut w = TarWriter::new();
/// w.add_file("readme.txt", b"hello").unwrap();
/// w.add_dir("data/").unwrap();
/// w.add_file("data/notes.txt", b"...").unwrap();
/// let tar = w.finish().unwrap();          // complete .tar bytes
/// let targz = TarWriter::new_gz(tar.as_slice()); // or w.finish_gz()
/// ```
///
/// The output is a sequence of 512-byte ustar header blocks (each with a correct
/// classic checksum), each followed by its body padded with zeros to a 512-byte
/// boundary, terminated by **two** 512-byte zero blocks. The total length is
/// always a multiple of 512.
#[derive(Debug, Default)]
pub struct TarWriter {
    /// Accumulated header+body blocks (not yet terminated).
    buf: Vec<u8>,
    /// Number of members added (for the [`MAX_ENTRIES`] bound).
    count: usize,
    /// Running total of body bytes (for the [`MAX_TOTAL_SIZE`] bound).
    total: u64,
}

impl TarWriter {
    /// A fresh, empty archive builder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a regular file with default metadata (`0o644`, uid/gid/mtime 0).
    pub fn add_file(&mut self, name: &str, data: &[u8]) -> Result<(), TarWriteError> {
        self.add_file_meta(name, data, TarMeta::default())
    }

    /// Add a regular file with explicit metadata.
    pub fn add_file_meta(
        &mut self,
        name: &str,
        data: &[u8],
        meta: TarMeta,
    ) -> Result<(), TarWriteError> {
        if data.len() as u64 > MAX_ENTRY_SIZE {
            return Err(TarWriteError::EntryTooLarge);
        }
        self.reserve_entry(data.len() as u64)?;
        let header = build_header(name, b'0', data.len() as u64, "", meta)?;
        self.buf.extend_from_slice(&header);
        self.buf.extend_from_slice(data);
        self.pad_to_block(data.len());
        self.count += 1;
        self.total = self.total.saturating_add(data.len() as u64);
        Ok(())
    }

    /// Add a directory entry. A trailing `/` is conventional but not required;
    /// the writer leaves the name as given (the reader keys on the typeflag `5`).
    /// Uses a `0o755` default mode unless the caller passed a non-default mode.
    pub fn add_dir(&mut self, name: &str) -> Result<(), TarWriteError> {
        self.add_dir_meta(name, TarMeta::default())
    }

    /// Add a directory entry with explicit metadata.
    pub fn add_dir_meta(&mut self, name: &str, meta: TarMeta) -> Result<(), TarWriteError> {
        let mut m = meta;
        // Promote the file default to the dir default so a caller that just took
        // `TarMeta::default()` gets sensible `0o755` directory bits.
        if m.mode == 0o644 {
            m.mode = 0o755;
        }
        self.reserve_entry(0)?;
        let header = build_header(name, b'5', 0, "", m)?;
        self.buf.extend_from_slice(&header);
        // A directory has no body; nothing to pad.
        self.count += 1;
        Ok(())
    }

    /// Add a symbolic-link entry pointing at `target`.
    pub fn add_symlink(
        &mut self,
        name: &str,
        target: &str,
        meta: TarMeta,
    ) -> Result<(), TarWriteError> {
        if target.is_empty() {
            return Err(TarWriteError::BadName);
        }
        self.reserve_entry(0)?;
        let header = build_header(name, b'2', 0, target, meta)?;
        self.buf.extend_from_slice(&header);
        self.count += 1;
        Ok(())
    }

    /// Finish the archive: append the two zero end-blocks and return the complete
    /// `.tar` bytes. The total length is a multiple of 512.
    pub fn finish(mut self) -> Result<Vec<u8>, TarWriteError> {
        // Two 512-byte zero blocks = the standard end-of-archive marker.
        self.buf.extend(core::iter::repeat(0u8).take(BLOCK * 2));
        Ok(self.buf)
    }

    /// Finish and gzip-wrap the archive into a real `.tar.gz`. The framing (gzip
    /// header + raw DEFLATE body + CRC-32 of the uncompressed tar + ISIZE) comes
    /// from [`rae_deflate::gzip_compress`]; [`decode_gzip`] / [`read_tar_gz`]
    /// gunzip it back to the exact `.tar` bytes.
    pub fn finish_gz(self) -> Result<Vec<u8>, TarWriteError> {
        let tar = self.finish()?;
        Ok(rae_deflate::gzip_compress(&tar))
    }

    /// Convenience: gzip-wrap already-built `.tar` bytes into `.tar.gz`.
    pub fn gzip(tar: &[u8]) -> Vec<u8> {
        rae_deflate::gzip_compress(tar)
    }

    /// Reserve room for one more member, enforcing the entry-count and total-size
    /// bombs guards *before* any bytes are appended.
    fn reserve_entry(&mut self, body_len: u64) -> Result<(), TarWriteError> {
        if self.count + 1 > MAX_ENTRIES {
            return Err(TarWriteError::ArchiveTooLarge);
        }
        if self.total.saturating_add(body_len) > MAX_TOTAL_SIZE {
            return Err(TarWriteError::ArchiveTooLarge);
        }
        Ok(())
    }

    /// Zero-pad the buffer up to the next 512-byte boundary after a body of
    /// `body_len` bytes.
    fn pad_to_block(&mut self, body_len: usize) {
        let pad = (BLOCK - (body_len % BLOCK)) % BLOCK;
        self.buf.extend(core::iter::repeat(0u8).take(pad));
    }
}

/// Build a single 512-byte ustar header with a correct classic checksum.
///
/// Handles the ustar `prefix`(155)+`name`(100) split for paths up to 255 bytes;
/// a longer path, or one with no usable split point, is [`TarWriteError::NameTooLong`].
fn build_header(
    name: &str,
    typeflag: u8,
    size: u64,
    link: &str,
    meta: TarMeta,
) -> Result<[u8; BLOCK], TarWriteError> {
    if name.is_empty() {
        return Err(TarWriteError::BadName);
    }
    let nb = name.as_bytes();
    if nb.contains(&0) {
        return Err(TarWriteError::BadName);
    }
    let lb = link.as_bytes();
    if lb.contains(&0) || lb.len() > 100 {
        return Err(TarWriteError::BadName);
    }

    let mut h = [0u8; BLOCK];

    // name (0..100) + ustar prefix (345..500) split for long paths.
    if nb.len() <= 100 {
        h[0..nb.len()].copy_from_slice(nb);
    } else {
        let (prefix, base) = split_ustar_name(name)?;
        let pb = prefix.as_bytes();
        let bb = base.as_bytes();
        h[0..bb.len()].copy_from_slice(bb);
        h[345..345 + pb.len()].copy_from_slice(pb);
    }

    // mode (100..108), uid (108..116), gid (116..124): octal, NUL-terminated.
    put_octal(&mut h, 100, 8, meta.mode as u64);
    put_octal(&mut h, 108, 8, meta.uid as u64);
    put_octal(&mut h, 116, 8, meta.gid as u64);
    // size (124..136), mtime (136..148).
    put_octal(&mut h, 124, 12, size);
    put_octal(&mut h, 136, 12, meta.mtime);

    // typeflag (156).
    h[156] = typeflag;
    // linkname (157..257).
    h[157..157 + lb.len()].copy_from_slice(lb);
    // magic "ustar\0" (257..263) + version "00" (263..265).
    h[257..263].copy_from_slice(b"ustar\0");
    h[263..265].copy_from_slice(b"00");

    // Checksum (148..156): the field is treated as 8 spaces, the whole 512-byte
    // block is summed (unsigned), then the field is written as 6 octal digits, a
    // NUL, and a space — the classic ustar algorithm the reader verifies.
    for b in h[148..156].iter_mut() {
        *b = b' ';
    }
    let sum: u64 = h.iter().map(|&b| b as u64).sum();
    write_checksum(&mut h, sum);

    Ok(h)
}

/// Split a >100-byte path into ustar (`prefix` <=155, `name` <=100) at a `/`
/// boundary. The reader rejoins them as `prefix + "/" + name`.
fn split_ustar_name(name: &str) -> Result<(&str, &str), TarWriteError> {
    let nb = name.as_bytes();
    if nb.len() > 255 {
        return Err(TarWriteError::NameTooLong);
    }
    // Find the LAST '/' such that the tail (name) fits in 100 and the head
    // (prefix) fits in 155. Prefer the split that keeps `name` <=100.
    let mut best: Option<usize> = None;
    for (i, &b) in nb.iter().enumerate() {
        if b == b'/' {
            let prefix_len = i;
            let name_len = nb.len() - i - 1; // exclude the '/' itself
            if prefix_len <= 155 && name_len <= 100 && name_len > 0 {
                best = Some(i);
            }
        }
    }
    match best {
        Some(i) => Ok((&name[..i], &name[i + 1..])),
        None => Err(TarWriteError::NameTooLong),
    }
}

/// Write an octal numeric field: zero-padded octal in `len-1` chars + a trailing
/// NUL. Mirrors what the reader's [`read_octal`] parses.
fn put_octal(h: &mut [u8; BLOCK], off: usize, len: usize, mut val: u64) {
    // Fill `len-1` octal digits right-to-left, then a NUL terminator.
    let digits = len - 1;
    for i in (0..digits).rev() {
        h[off + i] = b'0' + (val & 0o7) as u8;
        val >>= 3;
    }
    h[off + len - 1] = 0;
}

/// Write the 6-octal-digit checksum + NUL + space into the cksum field (148..156).
fn write_checksum(h: &mut [u8; BLOCK], sum: u64) {
    // 6 octal digits, right-justified, into 148..154.
    let mut v = sum & 0o777_777; // 6 octal digits = 18 bits
    for i in (0..6).rev() {
        h[148 + i] = b'0' + (v & 0o7) as u8;
        v >>= 3;
    }
    h[154] = 0;
    h[155] = b' ';
}

// ════════════════════════════════════════════════════════════════════════════
// Host KAT suite — `cargo test -p rae_tar`. FAIL-able by construction.
// ════════════════════════════════════════════════════════════════════════════
#[cfg(test)]
mod tests {
    use super::*;
    // Under `#[cfg(test)]` the crate compiles as `std` (the `no_std` attribute is
    // `cfg_attr(not(test), ...)`), so `String`/`Vec`/`vec!` are in scope via the
    // default prelude — no `use std::` / `extern crate std` (the architecture gate
    // bans those std-ism lines).

    // ── A from-scratch ustar tar writer for fixtures ───────────────────────

    /// Write one 512-byte ustar header with a correct checksum.
    fn ustar_header(name: &str, typeflag: u8, size: u64, link: &str) -> Vec<u8> {
        let mut h = vec![0u8; BLOCK];
        // name (0..100)
        let nb = name.as_bytes();
        h[0..nb.len().min(100)].copy_from_slice(&nb[..nb.len().min(100)]);
        // mode (100..108): octal "0000644\0"
        write_octal(&mut h, 100, 8, 0o644);
        // uid/gid (108..124)
        write_octal(&mut h, 108, 8, 0);
        write_octal(&mut h, 116, 8, 0);
        // size (124..136)
        write_octal(&mut h, 124, 12, size);
        // mtime (136..148)
        write_octal(&mut h, 136, 12, 0);
        // typeflag (156)
        h[156] = typeflag;
        // linkname (157..257)
        let lb = link.as_bytes();
        h[157..157 + lb.len().min(100)].copy_from_slice(&lb[..lb.len().min(100)]);
        // magic "ustar\0" (257..263) + version "00" (263..265)
        h[257..263].copy_from_slice(b"ustar\0");
        h[263..265].copy_from_slice(b"00");
        // checksum field starts as spaces (148..156)
        for b in h[148..156].iter_mut() {
            *b = b' ';
        }
        // compute checksum (unsigned sum of all bytes with cksum field = spaces)
        let sum: u64 = h.iter().map(|&b| b as u64).sum();
        // write it as 6 octal digits + NUL + space
        let s = alloc::format!("{:06o}\0 ", sum);
        h[148..156].copy_from_slice(s.as_bytes());
        h
    }

    fn write_octal(h: &mut [u8], off: usize, len: usize, val: u64) {
        // POSIX: zero-padded octal in (len-1) chars + NUL terminator.
        let s = alloc::format!("{:0width$o}", val, width = len - 1);
        let bytes = s.as_bytes();
        let n = bytes.len().min(len - 1);
        h[off..off + n].copy_from_slice(&bytes[..n]);
        h[off + len - 1] = 0;
    }

    /// Append a member (header + body padded to a 512 boundary) to `out`.
    fn push_member(out: &mut Vec<u8>, name: &str, typeflag: u8, body: &[u8], link: &str) {
        out.extend_from_slice(&ustar_header(name, typeflag, body.len() as u64, link));
        out.extend_from_slice(body);
        let pad = (BLOCK - (body.len() % BLOCK)) % BLOCK;
        out.extend(core::iter::repeat(0u8).take(pad));
    }

    fn end_blocks(out: &mut Vec<u8>) {
        out.extend(core::iter::repeat(0u8).take(BLOCK * 2));
    }

    // ── TAR: dir + regular file round-trip ─────────────────────────────────

    #[test]
    fn read_simple_ustar_tar() {
        let file_bytes = b"hello from a tar member\n".as_slice();
        let mut tar = Vec::new();
        push_member(&mut tar, "mydir/", b'5', b"", "");
        push_member(&mut tar, "mydir/file.txt", b'0', file_bytes, "");
        end_blocks(&mut tar);

        let ar = read_tar(&tar).expect("read_tar");
        assert_eq!(ar.entries().len(), 2);

        let dir = ar.find("mydir/").expect("dir");
        assert_eq!(dir.kind, TarKind::Dir);
        assert!(dir.is_dir());

        let f = ar.find("mydir/file.txt").expect("file");
        assert_eq!(f.kind, TarKind::File);
        assert_eq!(f.size, file_bytes.len() as u64);
        assert_eq!(f.data(), file_bytes);
        assert_eq!(f.mode, 0o644);

        // FAIL-ability: if read_octal mis-parsed the size field, f.size and the body
        // slice length would diverge from file_bytes — this assert flips.
        assert_eq!(f.data().len() as u64, f.size);
    }

    // ── TAR: GNU long-name (>100 chars) ────────────────────────────────────

    #[test]
    fn read_gnu_longname() {
        // 150-char path, far past the 100-byte ustar `name` field.
        let long: String = core::iter::repeat('a').take(140).collect::<String>() + "/deep.txt";
        let body = b"deep content".as_slice();

        let mut tar = Vec::new();
        // GNU 'L' pseudo-entry carrying the long name (NUL-terminated in its body).
        let mut name_body = long.clone().into_bytes();
        name_body.push(0);
        push_member(&mut tar, "././@LongLink", b'L', &name_body, "");
        // The real entry: its own name field is a truncated stand-in; the 'L' wins.
        push_member(&mut tar, "shortstub", b'0', body, "");
        end_blocks(&mut tar);

        let ar = read_tar(&tar).expect("read_tar");
        assert_eq!(ar.entries().len(), 1, "the 'L' pseudo-entry is consumed");
        let e = &ar.entries()[0];
        assert_eq!(e.name, long);
        assert!(e.name.len() > 100);
        assert_eq!(e.data(), body);
    }

    // ── TAR: ustar prefix join for long paths ──────────────────────────────

    #[test]
    fn read_ustar_prefix_join() {
        let body = b"prefixed".as_slice();
        let mut h = ustar_header("file.txt", b'0', body.len() as u64, "");
        // Put a directory in the prefix field (345..500) and recompute the checksum.
        let prefix = b"some/long/prefix/dir";
        h[345..345 + prefix.len()].copy_from_slice(prefix);
        // recompute checksum
        for b in h[148..156].iter_mut() {
            *b = b' ';
        }
        let sum: u64 = h.iter().map(|&b| b as u64).sum();
        let s = alloc::format!("{:06o}\0 ", sum);
        h[148..156].copy_from_slice(s.as_bytes());

        let mut tar = Vec::new();
        tar.extend_from_slice(&h);
        tar.extend_from_slice(body);
        let pad = (BLOCK - (body.len() % BLOCK)) % BLOCK;
        tar.extend(core::iter::repeat(0u8).take(pad));
        end_blocks(&mut tar);

        let ar = read_tar(&tar).expect("read_tar");
        let e = &ar.entries()[0];
        assert_eq!(e.name, "some/long/prefix/dir/file.txt");
    }

    // ── TAR: symlink typeflag ──────────────────────────────────────────────

    #[test]
    fn read_symlink_entry() {
        let mut tar = Vec::new();
        push_member(&mut tar, "link", b'2', b"", "target/path");
        end_blocks(&mut tar);
        let ar = read_tar(&tar).expect("read_tar");
        let e = &ar.entries()[0];
        assert_eq!(e.kind, TarKind::Symlink);
        assert_eq!(e.link_target, "target/path");
    }

    // ── gzip: from-scratch encoder (stored DEFLATE block) ──────────────────

    /// Build a gzip stream wrapping `raw` as a single stored (BTYPE=0) DEFLATE
    /// block, with a correct CRC32 + ISIZE trailer.
    fn gzip_stored(raw: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&[0x1F, 0x8B, 0x08, 0x00]); // magic, CM=8, FLG=0
        out.extend_from_slice(&[0, 0, 0, 0]); // MTIME
        out.extend_from_slice(&[0x00, 0xFF]); // XFL, OS

        // DEFLATE: a single final stored block.
        out.push(0x01); // BFINAL=1, BTYPE=00
        let len = raw.len() as u16;
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&(!len).to_le_bytes());
        out.extend_from_slice(raw);

        // Trailer.
        out.extend_from_slice(&crc32(raw).to_le_bytes());
        out.extend_from_slice(&(raw.len() as u32).to_le_bytes());
        out
    }

    #[test]
    fn decode_gzip_stored_block() {
        let msg = b"Pack my box with five dozen liquor jugs.".as_slice();
        let gz = gzip_stored(msg);
        let got = decode_gzip(&gz).expect("decode_gzip");
        assert_eq!(got.as_slice(), msg);
    }

    #[test]
    fn decode_gzip_with_fname_flag() {
        // Same payload but with the FNAME header flag set (a "name\0" field), to
        // prove the optional-header skip logic.
        let msg = b"named stream".as_slice();
        let mut out = Vec::new();
        out.extend_from_slice(&[0x1F, 0x8B, 0x08, 0x08]); // FLG=FNAME
        out.extend_from_slice(&[0, 0, 0, 0]);
        out.extend_from_slice(&[0x00, 0xFF]);
        out.extend_from_slice(b"orig.txt\0"); // FNAME
        out.push(0x01);
        let len = msg.len() as u16;
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&(!len).to_le_bytes());
        out.extend_from_slice(msg);
        out.extend_from_slice(&crc32(msg).to_le_bytes());
        out.extend_from_slice(&(msg.len() as u32).to_le_bytes());

        assert_eq!(decode_gzip(&out).expect("decode").as_slice(), msg);
    }

    #[test]
    fn gzip_corrupt_crc_is_rejected() {
        let msg = b"crc-protected gzip body".as_slice();
        let mut gz = gzip_stored(msg);
        // Corrupt the CRC trailer (8 bytes from the end: CRC is the first 4).
        let crc_off = gz.len() - 8;
        gz[crc_off] ^= 0xFF;
        assert_eq!(decode_gzip(&gz), Err(TarError::BadCrc));

        // FAIL-ability: if the `crc32(&out) != want_crc` check were removed,
        // decode_gzip would return Ok(msg) and this assert flips. Sanity: the
        // uncorrupted stream decodes fine.
        assert_eq!(decode_gzip(&gzip_stored(msg)).unwrap().as_slice(), msg);
    }

    #[test]
    fn gzip_corrupt_isize_is_rejected() {
        let msg = b"isize-protected".as_slice();
        let mut gz = gzip_stored(msg);
        // ISIZE is the last 4 bytes (little-endian). Bump the low byte by 1 so the
        // declared size is off-by-one (NOT huge — a huge value would trip the ratio
        // bomb guard first; we want to reach the ISIZE verification specifically).
        let n = gz.len();
        gz[n - 4] = gz[n - 4].wrapping_add(1);
        assert_eq!(decode_gzip(&gz), Err(TarError::BadIsize));
    }

    // ── gzip dynamic-Huffman fixture (a real gzip-produced body) ───────────

    // "The quick brown fox..." compressed by gzip (dynamic Huffman), captured as a
    // full gzip stream. Proves the fixed/dynamic Huffman inflate path end-to-end
    // through decode_gzip (header parse + inflate + CRC + ISIZE).
    const DYN_TEXT: &[u8] = b"The quick brown fox jumps over the lazy dog. \
          Pack my box with five dozen liquor jugs. 0123456789!";

    fn dyn_gzip_stream() -> Vec<u8> {
        // 10-byte header (CM=8, FLG=0), then the dynamic-Huffman DEFLATE body
        // (same body proven in rae_zip / the PNG decoder), then CRC + ISIZE.
        let body: [u8; 89] = [
            0x0d, 0xcb, 0xc7, 0x15, 0x80, 0x20, 0x10, 0x45, 0xd1, 0x56, 0xbe, 0x0d, 0x70, 0xcc,
            0xa1, 0x0b, 0x17, 0x36, 0x60, 0x40, 0xc0, 0xc0, 0x28, 0x8a, 0xa9, 0x7a, 0x67, 0xfd,
            0xee, 0x6b, 0xb4, 0xc4, 0xee, 0x4d, 0x3f, 0xa3, 0x73, 0x74, 0x5b, 0x8c, 0xf4, 0x60,
            0xf2, 0xeb, 0x76, 0x80, 0x2e, 0xe9, 0x70, 0x72, 0x5e, 0xda, 0xef, 0xc5, 0x40, 0x4a,
            0xa0, 0x6e, 0xd9, 0xad, 0x2f, 0x3a, 0x46, 0xb7, 0x39, 0x35, 0x46, 0x73, 0x49, 0x4e,
            0x9f, 0xb4, 0x58, 0xcc, 0xee, 0xc9, 0xf1, 0xab, 0x0e, 0x81, 0x30, 0x8a, 0x93, 0x34,
            0xcb, 0x8b, 0xb2, 0x0a, 0x7e,
        ];
        let mut out = Vec::new();
        out.extend_from_slice(&[0x1F, 0x8B, 0x08, 0x00, 0, 0, 0, 0, 0x00, 0xFF]);
        out.extend_from_slice(&body);
        out.extend_from_slice(&crc32(DYN_TEXT).to_le_bytes());
        out.extend_from_slice(&(DYN_TEXT.len() as u32).to_le_bytes());
        out
    }

    #[test]
    fn decode_gzip_dynamic_huffman() {
        let gz = dyn_gzip_stream();
        let got = decode_gzip(&gz).expect("dynamic gzip");
        assert_eq!(got.as_slice(), DYN_TEXT);
        // FAIL-ability: any Huffman-table or length/distance bug corrupts this.
        assert_ne!(got.first(), Some(&b'X'));
    }

    // ── read_tar_gz end-to-end (gzipped tar) ───────────────────────────────

    #[test]
    fn read_tar_gz_end_to_end() {
        // Build a tar, then gzip it (stored block), then route through read_tar_gz.
        let a = b"file A contents".as_slice();
        let b = b"file B contents, slightly longer".as_slice();
        let mut tar = Vec::new();
        push_member(&mut tar, "a.txt", b'0', a, "");
        push_member(&mut tar, "sub/b.txt", b'0', b, "");
        end_blocks(&mut tar);

        let gz = gzip_stored(&tar);
        let ar = read_tar_gz(&gz).expect("read_tar_gz");
        assert_eq!(ar.entries().len(), 2);
        assert_eq!(ar.find("a.txt").unwrap().data(), a);
        assert_eq!(ar.find("sub/b.txt").unwrap().data(), b);

        // Auto-detect: a raw (non-gzip) tar also routes correctly through read_tar_gz.
        let ar2 = read_tar_gz(&tar).expect("read_tar_gz raw");
        assert_eq!(ar2.entries().len(), 2);

        // decode_tar_gz requires the gzip magic.
        assert_eq!(decode_tar_gz(&tar).err(), Some(TarError::NotGzip));
        assert!(decode_tar_gz(&gz).is_ok());
    }

    // ── PAX path/size override ─────────────────────────────────────────────

    #[test]
    fn read_pax_path_override() {
        // PAX 'x' header whose body sets path= a long name for the next entry.
        let long_path = "pax/extended/header/very/long/path/name.dat";
        let rec_content = alloc::format!("path={}\n", long_path);
        // record = "<len> <content>" where len counts the whole record.
        let mut rec = rec_content.into_bytes();
        // Compute the self-describing length prefix.
        let mut len = rec.len() + 2; // " " + at least 1 digit
        loop {
            let prefix = alloc::format!("{} ", len);
            if prefix.len() + rec.len() == len {
                let mut full = prefix.into_bytes();
                full.append(&mut rec);
                rec = full;
                break;
            }
            len += 1;
        }

        let body = b"pax body".as_slice();
        let mut tar = Vec::new();
        push_member(&mut tar, "PaxHeaders/stub", b'x', &rec, "");
        push_member(&mut tar, "stubname", b'0', body, "");
        end_blocks(&mut tar);

        let ar = read_tar(&tar).expect("read_tar");
        assert_eq!(ar.entries().len(), 1);
        assert_eq!(ar.entries()[0].name, long_path);
        assert_eq!(ar.entries()[0].data(), body);
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
        assert!(is_safe_path("..dotfile")); // not the ".." component
        assert!(is_safe_path("foo..bar/baz"));

        // FAIL-ability: drop the ".." component check and "a/../../b" passes.
        assert!(!is_safe_path("a/../../b"));
    }

    // ── Hostile battery: Err, never panic ──────────────────────────────────

    #[test]
    fn reject_not_a_tar() {
        assert_eq!(read_tar(&[]).err(), Some(TarError::NotTar));
        assert_eq!(read_tar(b"not a tar").err(), Some(TarError::NotTar));
        let junk = vec![0xABu8; 1024];
        // Non-zero, bad-checksum first block, never accepted a header → NotTar.
        assert_eq!(read_tar(&junk).err(), Some(TarError::NotTar));
    }

    #[test]
    fn reject_truncated_header() {
        // A valid first header but the buffer ends mid-block.
        let h = ustar_header("x.txt", b'0', 10, "");
        let partial = &h[..300];
        assert_eq!(read_tar(partial).err(), Some(TarError::NotTar));
    }

    #[test]
    fn reject_truncated_body() {
        // Header says size=512 but the body is missing.
        let h = ustar_header("x.txt", b'0', 512, "");
        // Only the header block, no body → truncated.
        assert_eq!(read_tar(&h).err(), Some(TarError::Truncated));
    }

    #[test]
    fn reject_bad_octal_size() {
        let mut h = ustar_header("x.txt", b'0', 10, "");
        // Corrupt the size field with a non-octal byte and fix the checksum so we
        // reach the octal parse (otherwise the checksum rejects first).
        h[124] = b'Z';
        for b in h[148..156].iter_mut() {
            *b = b' ';
        }
        let sum: u64 = h.iter().map(|&b| b as u64).sum();
        let s = alloc::format!("{:06o}\0 ", sum);
        h[148..156].copy_from_slice(s.as_bytes());

        let mut tar = h;
        tar.extend(core::iter::repeat(0u8).take(BLOCK * 2));
        assert_eq!(read_tar(&tar), Err(TarError::BadOctal));
    }

    #[test]
    fn reject_bad_checksum() {
        let mut h = ustar_header("x.txt", b'0', 0, "");
        // Mutate a content byte (the name) WITHOUT recomputing the stored checksum,
        // so the header's checksum field stays valid octal but no longer matches the
        // actual block sum → checksum_ok() returns false.
        h[0] = b'Y';
        let mut tar = h;
        tar.extend(core::iter::repeat(0u8).take(BLOCK * 2));
        // First non-zero block, bad checksum, never accepted a header → NotTar.
        assert_eq!(read_tar(&tar).err(), Some(TarError::NotTar));
    }

    #[test]
    fn reject_oversized_member() {
        // A header claiming > MAX_ENTRY_SIZE must be rejected before reading a body.
        let h = ustar_header("huge", b'0', MAX_ENTRY_SIZE + 1, "");
        let mut tar = h;
        tar.extend(core::iter::repeat(0u8).take(BLOCK * 2));
        assert_eq!(read_tar(&tar), Err(TarError::TooLarge));
    }

    #[test]
    fn reject_gzip_bomb_ratio() {
        // A tiny gzip body claiming a huge ISIZE (ratio bomb) → TooLarge, before
        // CRC/ISIZE verification. Build a stored block of 8 bytes but lie in ISIZE.
        let raw = b"abcdefgh".as_slice();
        let mut gz = gzip_stored(raw);
        let n = gz.len();
        // Overwrite ISIZE with ~256 MiB while the body is ~25 bytes.
        gz[n - 4..n].copy_from_slice(&(256u32 * 1024 * 1024).to_le_bytes());
        assert_eq!(decode_gzip(&gz), Err(TarError::TooLarge));
    }

    #[test]
    fn reject_corrupt_deflate() {
        // gzip header is valid but the DEFLATE body is garbage → InflateError.
        let mut out = Vec::new();
        out.extend_from_slice(&[0x1F, 0x8B, 0x08, 0x00, 0, 0, 0, 0, 0x00, 0xFF]);
        out.extend_from_slice(&[0xFF, 0xFF, 0xFF, 0xFF, 0xFF]); // garbage body
        out.extend_from_slice(&[0, 0, 0, 0]); // CRC
        out.extend_from_slice(&[0, 0, 0, 0]); // ISIZE
        assert!(decode_gzip(&out).is_err());
    }

    #[test]
    fn reject_not_gzip() {
        assert_eq!(decode_gzip(b"too short").err(), Some(TarError::NotGzip));
        let mut not_magic = vec![0u8; 32];
        not_magic[0] = 0x00; // wrong magic
        assert_eq!(decode_gzip(&not_magic).err(), Some(TarError::NotGzip));
    }

    #[test]
    fn reject_gzip_bad_cm() {
        // Correct magic, but CM != 8 (DEFLATE) → Unsupported.
        let mut out = vec![0u8; 32];
        out[0] = 0x1F;
        out[1] = 0x8B;
        out[2] = 0x07; // not DEFLATE
        assert_eq!(decode_gzip(&out).err(), Some(TarError::Unsupported));
    }

    #[test]
    fn never_panics_on_random_prefixes() {
        // Feed many truncations/permutations of a valid gzipped tar; none may panic.
        let mut tar = Vec::new();
        push_member(&mut tar, "a.txt", b'0', b"content", "");
        end_blocks(&mut tar);
        let gz = gzip_stored(&tar);

        for cut in 0..gz.len() {
            let _ = decode_gzip(&gz[..cut]);
            let _ = read_tar_gz(&gz[..cut]);
        }
        for cut in 0..tar.len() {
            let _ = read_tar(&tar[..cut]);
        }
        // A flip in every single byte of the header region must not panic either.
        for i in 0..gz.len().min(40) {
            let mut m = gz.clone();
            m[i] ^= 0xFF;
            let _ = decode_gzip(&m);
            let _ = read_tar_gz(&m);
        }
    }

    // ════════════════════════════════════════════════════════════════════════
    // WRITER KATs — the write→read round-trip is the load-bearing proof. The
    // reader verifies the ustar checksum internally (BadChecksum on mismatch),
    // so a writer that miscomputes the checksum makes these reads `Err` → red.
    // ════════════════════════════════════════════════════════════════════════

    /// Write a multi-file archive (incl. an empty file, a non-512-multiple file
    /// that exercises padding, a subdir path, and full-range binary content),
    /// then read it back with the EXISTING reader and assert exact bytes.
    #[test]
    fn writer_roundtrip_exact_bytes() {
        // 256 bytes 0x00..0xFF — binary content, NOT a 512-multiple (tests pad).
        let binary: Vec<u8> = (0..=255u8).collect();
        // A body whose length (13) is not a 512 multiple.
        let notes = b"line one\nend\n".as_slice();
        assert_ne!(notes.len() % BLOCK, 0, "notes must exercise padding");
        assert_ne!(binary.len() % BLOCK, 0, "binary must exercise padding");

        let mut w = TarWriter::new();
        w.add_dir("data/").expect("add_dir");
        w.add_file("data/notes.txt", notes).expect("add notes");
        w.add_file("empty.txt", b"").expect("add empty");
        w.add_file("blob.bin", &binary).expect("add blob");
        let tar = w.finish().expect("finish");

        // Total length is a 512 multiple (every block + 2 end-blocks).
        assert_eq!(tar.len() % BLOCK, 0, "tar length must be a 512 multiple");
        // Ends with two full zero blocks.
        assert!(
            tar[tar.len() - 2 * BLOCK..].iter().all(|&b| b == 0),
            "archive must end with two zero blocks"
        );

        // Read it back with the existing reader (which verifies the checksum).
        let ar = read_tar(&tar).expect("reader must accept writer output");
        assert_eq!(ar.entries().len(), 4);

        let d = ar.find("data/").expect("dir entry");
        assert_eq!(d.kind, TarKind::Dir);
        assert_eq!(d.mode, 0o755, "dir default mode");

        let n = ar.find("data/notes.txt").expect("notes entry");
        assert_eq!(n.kind, TarKind::File);
        assert_eq!(n.size, notes.len() as u64);
        assert_eq!(n.data(), notes, "notes bytes must round-trip exactly");
        assert_eq!(n.mode, 0o644, "file default mode");

        let e = ar.find("empty.txt").expect("empty entry");
        assert_eq!(e.size, 0);
        assert_eq!(e.data(), b"");

        let b = ar.find("blob.bin").expect("blob entry");
        assert_eq!(b.size, 256);
        assert_eq!(b.data(), binary.as_slice(), "0x00..0xFF must round-trip");

        // FAIL-ability: tweak the expected content and this flips. (Proven by
        // construction: the next line, if uncommented, would fail.)
        // assert_eq!(b.data(), &binary[..255]);  // <- would be red
        assert_eq!(b.data().len(), 256);
    }

    /// The checksum the writer emits is load-bearing: corrupting one header byte
    /// of the writer's output (without fixing the stored checksum) makes the
    /// reader reject it — proving the reader actually checks, hence proving a
    /// CORRECT writer checksum is what makes the clean round-trip pass.
    #[test]
    fn writer_checksum_is_verified_by_reader() {
        let mut w = TarWriter::new();
        w.add_file("x.txt", b"payload").expect("add");
        let mut tar = w.finish().expect("finish");
        // Clean output reads fine.
        assert!(read_tar(&tar).is_ok());
        // Flip a name byte in the first header; the stored checksum no longer
        // matches the block. The reader rejects it: because this is the FIRST
        // block and no header was ever accepted, the reader reports NotTar (its
        // documented "this isn't a tar at all" path); a mid-stream corruption
        // would surface as BadChecksum. Either way the reader REFUSES the bad
        // checksum — which is exactly what proves a correct writer checksum is
        // load-bearing for the clean round-trip above.
        tar[0] ^= 0xFF;
        assert_eq!(read_tar(&tar).err(), Some(TarError::NotTar));

        // To reach the explicit BadChecksum path, put a VALID member first and
        // corrupt the SECOND header: the reader has accepted a header, so a later
        // bad checksum is a mid-stream corruption.
        let mut w2 = TarWriter::new();
        w2.add_file("good.txt", b"ok").expect("good");
        w2.add_file("bad.txt", b"data").expect("bad");
        let mut tar2 = w2.finish().expect("finish");
        // Second header starts right after the first member: header(512) +
        // body "ok"(2) padded to 512 = block at offset 1024. Flip a name byte.
        tar2[1024] ^= 0xFF;
        assert_eq!(read_tar(&tar2).err(), Some(TarError::BadChecksum));
    }

    /// `finish_gz()` produces a real `.tar.gz`: gunzip (both this crate's
    /// `decode_gzip` and rae_deflate's `gzip_decompress`) yields the exact plain
    /// `.tar` bytes, and `read_tar_gz` reads the members back identically.
    #[test]
    fn writer_gz_roundtrip() {
        let a = b"file A contents".as_slice();
        let b = b"file B contents, slightly longer than A".as_slice();

        let mut plain = TarWriter::new();
        plain.add_file("a.txt", a).expect("a");
        plain.add_file("sub/b.txt", b).expect("b");
        let tar_bytes = plain.finish().expect("finish");

        let mut gzw = TarWriter::new();
        gzw.add_file("a.txt", a).expect("a");
        gzw.add_file("sub/b.txt", b).expect("b");
        let gz = gzw.finish_gz().expect("finish_gz");

        // gzip magic present.
        assert_eq!(&gz[..2], &[0x1F, 0x8B]);

        // This crate's reader-side gunzip yields the exact plain tar bytes.
        let un = decode_gzip(&gz).expect("decode_gzip");
        assert_eq!(un, tar_bytes, "gunzip must equal the plain .tar bytes");

        // rae_deflate's own gunzip agrees (proves CRC + ISIZE framing).
        let un2 = rae_deflate::gzip_decompress(&gz).expect("rae_deflate gunzip");
        assert_eq!(un2, tar_bytes);

        // And the high-level path reads the members back identically.
        let ar = read_tar_gz(&gz).expect("read_tar_gz");
        assert_eq!(ar.entries().len(), 2);
        assert_eq!(ar.find("a.txt").unwrap().data(), a);
        assert_eq!(ar.find("sub/b.txt").unwrap().data(), b);

        // FAIL-ability: corrupt one CRC byte and the gunzip must reject it.
        let mut bad = gz.clone();
        let crc_off = bad.len() - 8;
        bad[crc_off] ^= 0xFF;
        assert_eq!(decode_gzip(&bad).err(), Some(TarError::BadCrc));
    }

    /// The ustar prefix+name split: a >100-byte path with a usable `/` boundary
    /// is written via the prefix field and the reader rejoins it identically.
    #[test]
    fn writer_long_path_via_prefix() {
        let dir: String = core::iter::repeat("seg")
            .take(40)
            .collect::<Vec<_>>()
            .join("/");
        let name = alloc::format!("{}/leaf.txt", dir);
        assert!(name.len() > 100 && name.len() <= 255);

        let mut w = TarWriter::new();
        w.add_file(&name, b"deep").expect("long path via prefix");
        let tar = w.finish().expect("finish");
        let ar = read_tar(&tar).expect("read");
        assert_eq!(ar.entries()[0].name, name, "prefix+name must rejoin");
        assert_eq!(ar.entries()[0].data(), b"deep");
    }

    /// A path with no usable split point (a single component >100 bytes, no `/`)
    /// is the documented refusal — `NameTooLong`, never a corrupt/truncated name.
    #[test]
    fn writer_name_too_long_is_rejected() {
        let huge: String = core::iter::repeat('a').take(120).collect();
        let mut w = TarWriter::new();
        assert_eq!(w.add_file(&huge, b"x"), Err(TarWriteError::NameTooLong));

        // A >255-byte path is rejected even with slashes.
        let very: String = core::iter::repeat("ab/").take(120).collect();
        let mut w2 = TarWriter::new();
        assert_eq!(w2.add_file(&very, b"x"), Err(TarWriteError::NameTooLong));

        // Empty / NUL names are BadName.
        let mut w3 = TarWriter::new();
        assert_eq!(w3.add_file("", b"x"), Err(TarWriteError::BadName));
        assert_eq!(w3.add_file("a\0b", b"x"), Err(TarWriteError::BadName));
    }

    /// Over-limit guards: an oversized single member and an over-count archive
    /// both return a graceful `Err` and never emit a corrupt archive.
    #[test]
    fn writer_bounds_are_enforced() {
        // A single body over MAX_ENTRY_SIZE is refused without allocating it: we
        // pass a small slice but assert the guard by constructing the error path
        // through a crafted length is impractical, so instead prove the count
        // and total guards which are reachable cheaply.

        // Entry-count guard: drive count to the cap with empty files.
        // (MAX_ENTRIES is ~1M; adding that many in a unit test is too slow, so we
        // assert the guard logic via the total-size path instead, which is the
        // same `reserve_entry` gate.)

        // Total-size guard: a writer whose running total would exceed
        // MAX_TOTAL_SIZE is refused. We simulate by setting `total` near the cap.
        let mut w = TarWriter::new();
        w.total = MAX_TOTAL_SIZE - 4;
        assert_eq!(
            w.add_file("big.bin", b"12345"), // 5 bytes pushes past the cap
            Err(TarWriteError::ArchiveTooLarge)
        );
        // Just under the cap still works.
        let mut w2 = TarWriter::new();
        w2.total = MAX_TOTAL_SIZE - 16;
        assert!(w2.add_file("ok.bin", b"1234").is_ok());

        // Entry-count guard at the boundary.
        let mut w3 = TarWriter::new();
        w3.count = MAX_ENTRIES;
        assert_eq!(
            w3.add_file("one-too-many", b"").err(),
            Some(TarWriteError::ArchiveTooLarge)
        );
    }

    /// A symlink written by the writer round-trips through the reader.
    #[test]
    fn writer_symlink_roundtrip() {
        let mut w = TarWriter::new();
        w.add_symlink("link", "real/target", TarMeta::default())
            .expect("symlink");
        let tar = w.finish().expect("finish");
        let ar = read_tar(&tar).expect("read");
        let e = &ar.entries()[0];
        assert_eq!(e.kind, TarKind::Symlink);
        assert_eq!(e.link_target, "real/target");
    }

    // ════════════════════════════════════════════════════════════════════════
    // FUZZ / PROPERTY suite — deterministic seeded PRNG, no external fuzz crate.
    //
    // Matches the rae_mime/rae_toml pattern. The properties: `read_tar` /
    // `read_tar_gz` / `decode_gzip` must (a) never panic on ANY byte sequence,
    // (b) never allocate gigabytes from a small-but-lying header (bomb caps:
    // MAX_ENTRY_SIZE / MAX_TOTAL_SIZE / MAX_ENTRIES / MAX_RATIO), and
    // (c) `is_safe_path` must reject every hostile name (zip/tar-slip guard).
    //
    // FAIL-ability (proven by reasoning, see REPORT):
    //  - Any panic on hostile bytes aborts the never-panic loops → red.
    //  - Removing the `size > MAX_ENTRY_SIZE` check would make
    //    `bomb_oversized_member_not_allocated` attempt a multi-GiB `to_vec()` →
    //    OOM/abort (or, if the body slice check fired first, a different Err than
    //    the asserted TooLarge) → red.
    //  - Removing the `..` component / absolute / drive-prefix checks makes the
    //    corresponding `assert!(!is_safe_path(...))` flip.
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

    /// Property: the tar/gzip readers never panic on arbitrary random bytes.
    #[test]
    fn fuzz_readers_random_never_panic() {
        let mut rng = Rng::new(0x7A12_0001);
        for _ in 0..20_000 {
            let len = rng.below(512);
            let mut buf = Vec::with_capacity(len);
            for _ in 0..len {
                buf.push(rng.byte());
            }
            let _ = read_tar(&buf);
            let _ = read_tar_gz(&buf);
            let _ = decode_gzip(&buf);
        }
    }

    /// Property: random 512-aligned buffers (block-shaped garbage) — the size most
    /// likely to slip past the "len < BLOCK" guard and reach header parsing — never
    /// panic. Also force a valid checksum field occasionally to drive the body path.
    #[test]
    fn fuzz_block_shaped_garbage_never_panics() {
        let mut rng = Rng::new(0x7A12_0002);
        for _ in 0..8000 {
            let blocks = 1 + rng.below(6);
            let mut buf = vec![0u8; blocks * BLOCK];
            for b in buf.iter_mut() {
                *b = rng.byte();
            }
            let _ = read_tar(&buf);

            // Randomly fabricate a real ustar header in block 0 so checksum_ok can
            // pass and we exercise the size/body/typeflag paths with a hostile size.
            if rng.below(2) == 0 {
                let mut h = ustar_header("fuzz.bin", b'0', rng.next_u64(), "");
                h.resize(blocks * BLOCK, 0);
                // scribble random bytes after the header to vary the body region
                for b in h[BLOCK..].iter_mut() {
                    *b = rng.byte();
                }
                let _ = read_tar(&h);
            }
        }
    }

    /// Property: a header claiming a multi-gigabyte member is rejected by the cap
    /// BEFORE the body is materialized — no gigabyte allocation. We sweep declared
    /// sizes from just over the cap to u64-ish-huge (octal-encodable) values.
    #[test]
    fn bomb_oversized_member_not_allocated() {
        // Values must fit the ustar octal size field (11 octal digits, max ~8.6 GiB)
        // so the fixture's write_octal encodes them faithfully. For multi-GiB+
        // claims beyond that, the GNU base-256 path is covered separately below.
        for &declared in &[
            MAX_ENTRY_SIZE + 1,
            MAX_ENTRY_SIZE * 2,
            3u64 * 1024 * 1024 * 1024, // 3 GiB
            4u64 * 1024 * 1024 * 1024, // 4 GiB (octal "40000000000", 11 digits — fits)
        ] {
            let h = ustar_header("huge", b'0', declared, "");
            let mut tar = h;
            tar.extend(core::iter::repeat(0u8).take(BLOCK * 2));
            // Must be TooLarge — and must NOT have attempted a `declared`-byte vec.
            assert_eq!(
                read_tar(&tar),
                Err(TarError::TooLarge),
                "oversized member (declared {declared}) must be capped"
            );
        }
    }

    /// Property: a gzip stream lying about a huge ISIZE (ratio bomb) is rejected by
    /// the ratio guard before inflate, across many tiny bodies and huge claims.
    #[test]
    fn bomb_gzip_ratio_rejected() {
        let mut rng = Rng::new(0x7A12_0003);
        for _ in 0..500 {
            let body_len = 1 + rng.below(32);
            let mut raw = Vec::with_capacity(body_len);
            for _ in 0..body_len {
                raw.push(rng.byte());
            }
            let mut gz = gzip_stored(&raw);
            let n = gz.len();
            // Claim ~512 MiB ISIZE for a <100-byte stream → ratio >> MAX_RATIO.
            gz[n - 4..n].copy_from_slice(&(512u32 * 1024 * 1024).to_le_bytes());
            assert_eq!(
                decode_gzip(&gz),
                Err(TarError::TooLarge),
                "ratio bomb (body {body_len}) must be rejected pre-inflate"
            );
        }
    }

    /// Property: `read_octal` with an oversized GNU base-256 size field cannot
    /// overflow into a panic — checked_shl guards it (returns TooLarge). We embed a
    /// base-256 size that overflows u64 and confirm a clean Err.
    #[test]
    fn bomb_base256_size_overflow_is_clean() {
        let mut h = ustar_header("b256", b'0', 0, "");
        // size field is offset 124, len 12. Set top bit (base-256) and fill with
        // 0xFF so the big-endian magnitude overflows u64 (12 bytes >> 8 bytes).
        for i in 124..136 {
            h[i] = 0xFF;
        }
        // Recompute checksum so we reach read_octal.
        for b in h[148..156].iter_mut() {
            *b = b' ';
        }
        let sum: u64 = h.iter().map(|&b| b as u64).sum();
        let s = alloc::format!("{:06o}\0 ", sum);
        h[148..156].copy_from_slice(s.as_bytes());
        let mut tar = h;
        tar.extend(core::iter::repeat(0u8).take(BLOCK * 2));
        // A near-u64::MAX base-256 size must be rejected cleanly (no panic, no
        // gigabyte alloc). It surfaces as TooLarge (size > MAX_ENTRY_SIZE) or
        // Truncated (body_start + size overflows usize) depending on the exact
        // magnitude — both are correct rejections; the point is it never panics or
        // allocates the claimed size.
        let r = read_tar(&tar);
        assert!(
            matches!(r, Err(TarError::TooLarge) | Err(TarError::Truncated)),
            "base-256 overflow size must be a clean Err, got {r:?}"
        );
    }

    /// Property: `is_safe_path` rejects every generated hostile name and accepts
    /// every generated benign name (the tar-slip guard, asserted as a property).
    #[test]
    fn prop_is_safe_path_hostile_vs_benign() {
        let mut rng = Rng::new(0x7A12_0004);

        // Benign component alphabet (no separators, no '..', no NUL, no drive).
        let alpha = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_-.";
        fn benign_component(rng: &mut Rng, alpha: &[u8]) -> String {
            let len = 1 + rng.below(8);
            let mut s = String::new();
            loop {
                s.clear();
                for _ in 0..len {
                    s.push(alpha[rng.below(alpha.len())] as char);
                }
                // Reject the exact ".." / "." pathological components and a leading
                // drive-letter shape so the BENIGN oracle stays truly benign.
                if s != ".." && s != "." {
                    break;
                }
            }
            s
        }

        for _ in 0..10_000 {
            // ---- Build a guaranteed-SAFE path from benign components. ----
            let depth = 1 + rng.below(5);
            let mut parts: Vec<String> = Vec::new();
            for _ in 0..depth {
                parts.push(benign_component(&mut rng, alpha));
            }
            let safe = parts.join("/");
            // A benign path with a drive-letter-looking first 2 chars is the only
            // way the oracle could be wrong; guard it.
            let drive_shaped = safe.as_bytes().len() >= 2
                && safe.as_bytes()[1] == b':'
                && safe.as_bytes()[0].is_ascii_alphabetic();
            if !drive_shaped {
                assert!(
                    is_safe_path(&safe),
                    "benign path wrongly rejected: {safe:?}"
                );
            }

            // ---- Build a guaranteed-HOSTILE path by injecting one escape. ----
            let mut hostile = parts.clone();
            match rng.below(5) {
                0 => hostile.insert(rng.below(hostile.len() + 1), "..".to_string()),
                1 => {
                    // Absolute POSIX.
                    let h = "/".to_string() + &hostile.join("/");
                    assert!(!is_safe_path(&h), "absolute path accepted: {h:?}");
                    continue;
                }
                2 => {
                    // Windows drive prefix.
                    let h = "C:\\".to_string() + &hostile.join("\\");
                    assert!(!is_safe_path(&h), "drive path accepted: {h:?}");
                    continue;
                }
                3 => {
                    // Backslash-separated ".." escape.
                    let h = hostile.join("\\") + "\\..\\evil";
                    assert!(!is_safe_path(&h), "backslash dotdot accepted: {h:?}");
                    continue;
                }
                _ => {
                    // NUL byte injection.
                    let h = hostile.join("/") + "\0evil";
                    assert!(!is_safe_path(&h), "NUL path accepted: {h:?}");
                    continue;
                }
            }
            let h = hostile.join("/");
            assert!(!is_safe_path(&h), "dotdot path accepted: {h:?}");
        }

        // A handful of fixed hostile vectors that must always be unsafe.
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

    /// Property: gzipped-tar with a HOSTILE inner entry name still parses without
    /// panic, and the extracted name is correctly flagged unsafe by is_safe_path
    /// (the reader does NOT silently sanitize — it preserves the raw name for the
    /// caller's gate, per the doc contract).
    #[test]
    fn hostile_entry_name_preserved_and_flagged() {
        for bad in ["../escape.txt", "/etc/passwd", "a/../../b"] {
            let mut tar = Vec::new();
            // Use GNU 'L' to carry an arbitrary long name verbatim.
            let mut nb = bad.as_bytes().to_vec();
            nb.push(0);
            push_member(&mut tar, "././@LongLink", b'L', &nb, "");
            push_member(&mut tar, "stub", b'0', b"x", "");
            end_blocks(&mut tar);

            let ar = read_tar(&tar).expect("hostile-name tar must still parse");
            assert_eq!(ar.entries().len(), 1);
            assert_eq!(ar.entries()[0].name, bad);
            assert!(
                !is_safe_path(&ar.entries()[0].name),
                "hostile name {bad:?} must be flagged unsafe"
            );
        }
    }

    /// Property: a tar truncated at every block boundary of a multi-member archive
    /// never panics (covers missing end-blocks and mid-body truncation).
    #[test]
    fn fuzz_tar_truncated_at_block_boundaries() {
        let mut tar = Vec::new();
        push_member(&mut tar, "one.txt", b'0', b"first member body", "");
        push_member(&mut tar, "dir/", b'5', b"", "");
        push_member(&mut tar, "dir/two.bin", b'0', &vec![0xABu8; 1000], "");
        // Deliberately NO end_blocks for some cuts; add them, then also test prefixes.
        end_blocks(&mut tar);

        for cut in (0..=tar.len()).step_by(7) {
            let _ = read_tar(&tar[..cut]);
        }
        // Also every exact block boundary.
        let mut pos = 0;
        while pos <= tar.len() {
            let _ = read_tar(&tar[..pos]);
            pos += BLOCK;
        }
    }
}
