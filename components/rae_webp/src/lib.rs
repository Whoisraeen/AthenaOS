//! # RaeWebP — a never-panic, `no_std` WebP image decoder (VP8L lossless).
//!
//! RaeenOS_Concept.md §creators / media (criterion #5: "show my photos" / web
//! images): a daily driver must display the images people actually encounter.
//! WebP is the **dominant modern web image format** — Google serves it across the
//! web, and a browser/Photos viewer that can't decode WebP looks broken on a huge
//! fraction of real pages. RaeenOS already decodes BMP/GIF/PNG/JPEG; this crate is
//! the from-scratch WebP path that completes the still-image stack.
//!
//! Output is a flat ARGB8888 `Vec<u32>` (`0xAARRGGBB`) — the RaeGFX compositor /
//! Canvas pixel format, **matching [`rae_png`]/`rae_bmp`/`rae_gif`** so a gallery,
//! a tab-strip, or a Quick Look preview can blit any RaeenOS image format through
//! one uniform pixel model.
//!
//! ## What it decodes
//! - The **RIFF/WebP container**: the `RIFF`/`WEBP` FourCC envelope, the chunk
//!   stream (`VP8L`, `VP8 `, `VP8X`, `ALPH`, `ANIM`, `ANMF`), dispatched by chunk.
//!   A non-WebP / truncated / oversized file is rejected.
//! - **VP8L lossless** (the primary deliverable): the 1-byte signature (`0x2F`),
//!   the 14-bit width/height (minus-one encoded) + the alpha-used / version bits,
//!   the **transform chain** (predictor / color / subtract-green / color-indexing,
//!   applied in reverse on decode), the **meta-Huffman / entropy-image** structure,
//!   the **canonical Huffman code groups** (5 per meta-group: green+length, red,
//!   blue, alpha, distance), the **LZ77 backward references** with the WebP
//!   distance-mapping, and the **color cache** — decoded to ARGB8888.
//!
//! ## What is deferred (honest)
//! - **VP8 lossy** (`VP8 ` chunk) → [`WebpError::UnsupportedLossy`]. VP8 is a full
//!   intra-frame video keyframe decoder (boolean entropy + intra prediction + DCT/
//!   WHT + loop filter) — out of scope for this slice; returned as an honest error
//!   rather than a garbage decode.
//! - **Animation** (`ANIM`/`ANMF`): the first frame is decoded when it is VP8L;
//!   otherwise [`WebpError::UnsupportedAnimation`] / `UnsupportedLossy`.
//! - **`ALPH`** (lossy alpha) is only meaningful with a VP8 frame, hence deferred.
//!
//! ## Hostile-input posture (CLAUDE: decoders are the #1 RCE surface)
//! Every WebP byte is attacker-controlled. There is **no `unwrap`/`expect`/`panic`/
//! raw-index-panic path** reachable from [`decode_webp`]: a bad signature, a
//! truncated chunk, an absurd dimension, an over-long Huffman code, a back-reference
//! pointing before the start of output, a too-large color cache, and a malformed
//! transform all return `Err(WebpError)`. Memory is bounded up front
//! ([`MAX_DIMENSION`], [`MAX_PIXELS`]) so a crafted header cannot request a
//! multi-gigabyte allocation, and every decode loop has an explicit progress/size
//! cap so a crafted stream cannot hang or OOM. The host KAT suite at the bottom of
//! this file is the primary proof (`cargo test -p rae_webp`).

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

use alloc::vec;
use alloc::vec::Vec;

/// Bound on either image axis. WebP/VP8L dimensions are 14-bit (max 16384), so
/// this ceiling is generous; it keeps a single canvas under [`MAX_PIXELS`].
pub const MAX_DIMENSION: u32 = 16384;
/// Bound on total pixel count (width * height). ~67M px = 256 MiB at 4 B/px ARGB.
/// A crafted header claiming a huge canvas is rejected before allocation.
pub const MAX_PIXELS: u64 = 64 * 1024 * 1024;

/// WebP decode error. Every variant is a *handled* path — nothing here panics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebpError {
    /// The buffer is too short, or a chunk ran past the end of the buffer.
    Truncated,
    /// The `RIFF` / `WEBP` FourCC envelope was missing or malformed.
    NotWebp,
    /// No image (`VP8L`/`VP8 `/`VP8X`+payload) chunk was present.
    NoImageData,
    /// Width/height was zero or exceeded the memory bound.
    DimensionsOutOfRange,
    /// The VP8L signature byte or version field was wrong.
    BadVp8l,
    /// A canonical Huffman table was malformed (bad code lengths / not prefix-free).
    BadHuffman,
    /// A back-reference distance/length was out of range, or pointed before start.
    BadBackref,
    /// A transform was malformed (bad type, bad color-index size, bad tile bits).
    BadTransform,
    /// The decoded stream ended early or produced the wrong number of pixels.
    BadImageData,
    /// VP8 lossy is not supported by this decoder (honest deferral).
    UnsupportedLossy,
    /// Animation is not supported / the first frame is not decodable here.
    UnsupportedAnimation,
}

/// A decoded image: a flat ARGB8888 buffer plus dimensions.
///
/// `pixels.len() == (width * height) as usize`. Each `u32` is `0xAARRGGBB` —
/// identical to [`rae_png`]/`rae_bmp`/`rae_gif` so callers consume every RaeenOS
/// still-image decoder through one pixel model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebpImage {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u32>,
}

impl WebpImage {
    /// Sample a pixel as `(a, r, g, b)`. `None` out of bounds — tests use this so
    /// a wrong coordinate is a clean failure, not a panic.
    pub fn pixel(&self, x: u32, y: u32) -> Option<(u8, u8, u8, u8)> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let idx = (y as usize) * (self.width as usize) + (x as usize);
        let p = *self.pixels.get(idx)?;
        Some(((p >> 24) as u8, (p >> 16) as u8, (p >> 8) as u8, p as u8))
    }
}

// VP8L stores pixels internally as ARGB packed as 0xAARRGGBB (the same packing
// our public format uses), but its bit-stream order per channel is G, R, B, A in
// the Huffman groups. We keep a single u32 with these accessors.
#[inline]
fn make_argb(a: u8, r: u8, g: u8, b: u8) -> u32 {
    ((a as u32) << 24) | ((r as u32) << 16) | ((g as u32) << 8) | (b as u32)
}
#[inline]
fn px_a(p: u32) -> u8 {
    (p >> 24) as u8
}
#[inline]
fn px_r(p: u32) -> u8 {
    (p >> 16) as u8
}
#[inline]
fn px_g(p: u32) -> u8 {
    (p >> 8) as u8
}
#[inline]
fn px_b(p: u32) -> u8 {
    p as u8
}

// ════════════════════════════════════════════════════════════════════════════
// RIFF / WebP container
// ════════════════════════════════════════════════════════════════════════════

/// Decode a WebP byte stream into an ARGB8888 [`WebpImage`].
///
/// Hostile-input safe: returns `Err` (never panics/OOMs/hangs) on any malformed
/// input. VP8L lossless is fully decoded; VP8 lossy → [`WebpError::UnsupportedLossy`];
/// animation decodes the first VP8L frame or returns [`WebpError::UnsupportedAnimation`].
pub fn decode_webp(data: &[u8]) -> Result<WebpImage, WebpError> {
    // ── RIFF envelope: "RIFF" <u32 le size> "WEBP" ──────────────────────────
    if data.len() < 12 {
        return Err(WebpError::Truncated);
    }
    if &data[0..4] != b"RIFF" || &data[8..12] != b"WEBP" {
        return Err(WebpError::NotWebp);
    }
    let riff_size = u32::from_le_bytes([data[4], data[5], data[6], data[7]]) as usize;
    // The RIFF size counts everything after the 8-byte "RIFF"+size prefix.
    // Clamp the working buffer to what RIFF declares (but never past the slice).
    let declared_end = 8usize.checked_add(riff_size).ok_or(WebpError::Truncated)?;
    let end = declared_end.min(data.len());
    if end < 12 {
        return Err(WebpError::Truncated);
    }

    // ── Chunk loop ───────────────────────────────────────────────────────────
    let mut pos = 12usize;
    // Extended (VP8X) state, if present.
    let mut saw_vp8x = false;
    let mut anim = false;
    while pos + 8 <= end {
        let fourcc = &data[pos..pos + 4];
        let csize = u32::from_le_bytes([data[pos + 4], data[pos + 5], data[pos + 6], data[pos + 7]])
            as usize;
        let cstart = pos + 8;
        let cend = cstart.checked_add(csize).ok_or(WebpError::Truncated)?;
        if cend > end {
            return Err(WebpError::Truncated);
        }
        let chunk = &data[cstart..cend];

        match fourcc {
            b"VP8L" => {
                return decode_vp8l(chunk);
            }
            b"VP8 " => {
                return Err(WebpError::UnsupportedLossy);
            }
            b"VP8X" => {
                // Extended header: 1 flag byte + 3 reserved + 3-byte canvas w-1 +
                // 3-byte canvas h-1. We note the flags; the actual image comes in
                // a following VP8L/VP8/ANMF chunk.
                if chunk.len() >= 1 {
                    let flags = chunk[0];
                    anim = (flags & 0x02) != 0; // ANIMATION bit
                }
                saw_vp8x = true;
            }
            b"ANIM" => {
                anim = true;
            }
            b"ANMF" => {
                // Animation frame: 16-byte frame header then a sub-chunk
                // (VP8L / VP8 / ALPH+VP8). Decode the first VP8L sub-frame.
                return decode_anmf(chunk);
            }
            b"ALPH" => { /* lossy alpha — only meaningful with a VP8 frame; skip */ }
            _ => { /* unknown ancillary chunk — skip */ }
        }

        // Chunks are padded to an even size.
        let padded = csize + (csize & 1);
        pos = cstart.checked_add(padded).ok_or(WebpError::Truncated)?;
    }

    if anim {
        return Err(WebpError::UnsupportedAnimation);
    }
    let _ = saw_vp8x;
    Err(WebpError::NoImageData)
}

/// Decode an `ANMF` (animation frame) chunk: a 16-byte frame header followed by
/// the frame's image sub-chunk. We honor only a VP8L sub-frame (first frame).
fn decode_anmf(chunk: &[u8]) -> Result<WebpImage, WebpError> {
    // ANMF header = 24 bytes? Per spec: Frame X(3) Y(3) W-1(3) H-1(3) Duration(3)
    // flags(1) = 16 bytes, then frame data. Be defensive about the length.
    if chunk.len() < 16 + 8 {
        return Err(WebpError::Truncated);
    }
    let mut pos = 16usize;
    while pos + 8 <= chunk.len() {
        let fourcc = &chunk[pos..pos + 4];
        let csize = u32::from_le_bytes([
            chunk[pos + 4],
            chunk[pos + 5],
            chunk[pos + 6],
            chunk[pos + 7],
        ]) as usize;
        let cstart = pos + 8;
        let cend = cstart.checked_add(csize).ok_or(WebpError::Truncated)?;
        if cend > chunk.len() {
            return Err(WebpError::Truncated);
        }
        match fourcc {
            b"VP8L" => return decode_vp8l(&chunk[cstart..cend]),
            b"VP8 " => return Err(WebpError::UnsupportedLossy),
            _ => {}
        }
        let padded = csize + (csize & 1);
        pos = cstart.checked_add(padded).ok_or(WebpError::Truncated)?;
    }
    Err(WebpError::UnsupportedAnimation)
}

// ════════════════════════════════════════════════════════════════════════════
// VP8L bit reader (LSB-first within a byte, the WebP lossless convention).
// ════════════════════════════════════════════════════════════════════════════

struct Vp8lReader<'a> {
    data: &'a [u8],
    bit_pos: usize, // absolute bit offset
}

impl<'a> Vp8lReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, bit_pos: 0 }
    }

    /// Read `n` (0..=24) bits, LSB-first. Past-end reads are an error.
    fn read_bits(&mut self, n: u32) -> Result<u32, WebpError> {
        let mut v = 0u32;
        for i in 0..n {
            let byte_idx = self.bit_pos >> 3;
            let bit_idx = (self.bit_pos & 7) as u32;
            let byte = *self.data.get(byte_idx).ok_or(WebpError::Truncated)?;
            let bit = ((byte >> bit_idx) & 1) as u32;
            v |= bit << i;
            self.bit_pos += 1;
        }
        Ok(v)
    }

    #[inline]
    fn read_bit(&mut self) -> Result<u32, WebpError> {
        self.read_bits(1)
    }
}

// ════════════════════════════════════════════════════════════════════════════
// Canonical Huffman (VP8L variant): build from code lengths, decode MSB-first via
// a length-walk. Max code length in VP8L is 15.
// ════════════════════════════════════════════════════════════════════════════

const MAX_HUFFMAN_BITS: usize = 15;

/// A canonical-Huffman decoder built from a list of code lengths, decoded the same
/// way [`rae_png`] / [`rae_deflate`] do: walk bit-by-bit accumulating the code,
/// comparing against the running count per length. A single-symbol table (one
/// non-zero length, or the VP8L "simple" 1-symbol code) decodes with zero bits.
struct Huffman {
    counts: [u16; MAX_HUFFMAN_BITS + 1],
    symbols: Vec<u16>,
    /// For the degenerate single-symbol case, decode consumes no bits.
    single: Option<u16>,
}

impl Huffman {
    /// Build a canonical table from code lengths (0 = symbol absent).
    fn from_lengths(lengths: &[u16]) -> Result<Self, WebpError> {
        let mut counts = [0u16; MAX_HUFFMAN_BITS + 1];
        let mut nonzero = 0usize;
        let mut last_sym = 0u16;
        for (sym, &l) in lengths.iter().enumerate() {
            let l = l as usize;
            if l > MAX_HUFFMAN_BITS {
                return Err(WebpError::BadHuffman);
            }
            if l != 0 {
                counts[l] += 1;
                nonzero += 1;
                last_sym = sym as u16;
            }
        }
        counts[0] = 0;

        if nonzero == 0 {
            return Err(WebpError::BadHuffman);
        }
        if nonzero == 1 {
            // One symbol: VP8L decodes it with no bits consumed.
            return Ok(Self {
                counts,
                symbols: Vec::new(),
                single: Some(last_sym),
            });
        }

        // Verify the code set is complete/prefix-free (kraft check) and build the
        // canonical symbol ordering.
        let mut offsets = [0u16; MAX_HUFFMAN_BITS + 1];
        let mut sum = 0u32;
        let mut left = 1i64; // available codes at length 0 doubled per level
        for len in 1..=MAX_HUFFMAN_BITS {
            left <<= 1;
            left -= counts[len] as i64;
            if left < 0 {
                return Err(WebpError::BadHuffman); // over-subscribed
            }
            offsets[len] = sum as u16;
            sum += counts[len] as u32;
        }
        // `left > 0` means an incomplete (non-full) code. VP8L permits this in
        // practice only for the 1-symbol case (handled above); reject otherwise to
        // stay strict against malformed tables, EXCEPT we still allow it because
        // some valid streams use incomplete codes — but an incomplete code that is
        // actually consulted will fail to decode (returns Err), which is safe.

        let mut symbols = vec![0u16; sum as usize];
        for (sym, &l) in lengths.iter().enumerate() {
            let l = l as usize;
            if l != 0 {
                let idx = offsets[l] as usize;
                if idx >= symbols.len() {
                    return Err(WebpError::BadHuffman);
                }
                symbols[idx] = sym as u16;
                offsets[l] += 1;
            }
        }

        Ok(Self {
            counts,
            symbols,
            single: None,
        })
    }

    fn decode(&self, br: &mut Vp8lReader) -> Result<u16, WebpError> {
        if let Some(s) = self.single {
            return Ok(s);
        }
        let mut code: i32 = 0;
        let mut first: i32 = 0;
        let mut index: i32 = 0;
        for len in 1..=MAX_HUFFMAN_BITS {
            code |= br.read_bit()? as i32;
            let count = self.counts[len] as i32;
            if code - first < count {
                let sym_idx = (index + (code - first)) as usize;
                return self
                    .symbols
                    .get(sym_idx)
                    .copied()
                    .ok_or(WebpError::BadHuffman);
            }
            index += count;
            first += count;
            first <<= 1;
            code <<= 1;
        }
        Err(WebpError::BadHuffman)
    }
}

// ── Reading a Huffman code (VP8L §"Decoding the code lengths") ───────────────
//
// A code is either "simple" (1 or 2 symbols, lengths implied) or "normal"
// (code lengths are themselves Huffman-coded by a meta code-length code).

/// The order code-length-code lengths are transmitted (VP8L spec).
const CODE_LENGTH_CODE_ORDER: [usize; 19] = [
    17, 18, 0, 1, 2, 3, 4, 5, 16, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15,
];

/// Read one Huffman code of `alphabet_size` symbols from the bitstream.
fn read_huffman_code(br: &mut Vp8lReader, alphabet_size: usize) -> Result<Huffman, WebpError> {
    if alphabet_size == 0 || alphabet_size > 2328 {
        // 256 + 24 color-cache + len codes; 2328 is the green alphabet ceiling.
        return Err(WebpError::BadHuffman);
    }
    let simple = br.read_bit()?;
    if simple == 1 {
        // Simple code: 1 or 2 symbols.
        let num_symbols = br.read_bits(1)? as usize + 1;
        let first_bits = br.read_bit()?; // 0 => 1-bit symbol, 1 => 8-bit symbol
        let sym0 = if first_bits == 0 {
            br.read_bits(1)? as u16
        } else {
            br.read_bits(8)? as u16
        };
        let mut lengths = vec![0u16; alphabet_size];
        if (sym0 as usize) >= alphabet_size {
            return Err(WebpError::BadHuffman);
        }
        if num_symbols == 1 {
            lengths[sym0 as usize] = 1;
            // single-symbol table
            return Huffman::from_lengths(&lengths);
        }
        let sym1 = br.read_bits(8)? as u16;
        if (sym1 as usize) >= alphabet_size {
            return Err(WebpError::BadHuffman);
        }
        lengths[sym0 as usize] = 1;
        lengths[sym1 as usize] = 1;
        return Huffman::from_lengths(&lengths);
    }

    // Normal code: read the code-length code, then use it to read the lengths.
    let num_code_lengths = br.read_bits(4)? as usize + 4;
    if num_code_lengths > CODE_LENGTH_CODE_ORDER.len() {
        return Err(WebpError::BadHuffman);
    }
    let mut cl_lengths = [0u16; 19];
    for i in 0..num_code_lengths {
        cl_lengths[CODE_LENGTH_CODE_ORDER[i]] = br.read_bits(3)? as u16;
    }
    let cl_huff = Huffman::from_lengths(&cl_lengths)?;

    // Optional length-limit (max_symbol) prefix.
    let mut max_symbol = alphabet_size;
    if br.read_bit()? == 1 {
        let length_nbits = 2 + 2 * br.read_bits(3)? as u32;
        max_symbol = 2 + br.read_bits(length_nbits)? as usize;
    }

    let mut code_lengths = vec![0u16; alphabet_size];
    let mut prev_len = 8u16; // default repeat value
    let mut sym = 0usize;
    let mut remaining = max_symbol;
    // Bound the total iterations: at most alphabet_size symbols can be written.
    let mut guard = alphabet_size + 8;
    while sym < alphabet_size && remaining > 0 {
        if guard == 0 {
            return Err(WebpError::BadHuffman);
        }
        guard -= 1;
        remaining -= 1;
        let code_len = cl_huff.decode(br)?;
        if code_len < 16 {
            code_lengths[sym] = code_len;
            if code_len != 0 {
                prev_len = code_len;
            }
            sym += 1;
        } else {
            // Repeat codes.
            let (repeat_base, extra_bits, repeat_value): (usize, u32, u16) = match code_len {
                16 => (3, 2, prev_len), // repeat previous, 3..6
                17 => (3, 3, 0),        // repeat zero, 3..10
                18 => (11, 7, 0),       // repeat zero, 11..138
                _ => return Err(WebpError::BadHuffman),
            };
            let repeat = repeat_base + br.read_bits(extra_bits)? as usize;
            for _ in 0..repeat {
                if sym >= alphabet_size {
                    return Err(WebpError::BadHuffman);
                }
                code_lengths[sym] = repeat_value;
                sym += 1;
            }
        }
    }

    Huffman::from_lengths(&code_lengths)
}

// ════════════════════════════════════════════════════════════════════════════
// VP8L LZ77 length/distance tables & distance mapping.
// ════════════════════════════════════════════════════════════════════════════

/// VP8L code → (length/distance) base + extra-bit count. For symbols 0..3 the
/// value is `sym + 1`; for higher symbols it is `2^(e+1) + ... ` per the spec.
fn lz77_value(sym: u16, br: &mut Vp8lReader) -> Result<u32, WebpError> {
    let s = sym as u32;
    if s < 4 {
        return Ok(s + 1);
    }
    let extra_bits = (s - 2) >> 1;
    if extra_bits > 24 {
        return Err(WebpError::BadBackref);
    }
    let offset = (2 + (s & 1)) << extra_bits;
    let extra = br.read_bits(extra_bits)?;
    Ok(offset + extra + 1)
}

/// VP8L 2-D distance mapping: small distance codes map to nearby pixels via a
/// fixed (x,y) offset table; codes ≥ table length are plain linear distances.
const DISTANCE_MAP: [(i32, i32); 120] = [
    (0, 1),
    (1, 0),
    (1, 1),
    (-1, 1),
    (0, 2),
    (2, 0),
    (1, 2),
    (-1, 2),
    (2, 1),
    (-2, 1),
    (2, 2),
    (-2, 2),
    (0, 3),
    (3, 0),
    (1, 3),
    (-1, 3),
    (3, 1),
    (-3, 1),
    (2, 3),
    (-2, 3),
    (3, 2),
    (-3, 2),
    (0, 4),
    (4, 0),
    (1, 4),
    (-1, 4),
    (4, 1),
    (-4, 1),
    (3, 3),
    (-3, 3),
    (2, 4),
    (-2, 4),
    (4, 2),
    (-4, 2),
    (0, 5),
    (3, 4),
    (-3, 4),
    (4, 3),
    (-4, 3),
    (5, 0),
    (1, 5),
    (-1, 5),
    (5, 1),
    (-5, 1),
    (2, 5),
    (-2, 5),
    (5, 2),
    (-5, 2),
    (4, 4),
    (-4, 4),
    (3, 5),
    (-3, 5),
    (5, 3),
    (-5, 3),
    (0, 6),
    (6, 0),
    (1, 6),
    (-1, 6),
    (6, 1),
    (-6, 1),
    (2, 6),
    (-2, 6),
    (6, 2),
    (-6, 2),
    (4, 5),
    (-4, 5),
    (5, 4),
    (-5, 4),
    (3, 6),
    (-3, 6),
    (6, 3),
    (-6, 3),
    (0, 7),
    (7, 0),
    (1, 7),
    (-1, 7),
    (5, 5),
    (-5, 5),
    (7, 1),
    (-7, 1),
    (4, 6),
    (-4, 6),
    (6, 4),
    (-6, 4),
    (2, 7),
    (-2, 7),
    (7, 2),
    (-7, 2),
    (3, 7),
    (-3, 7),
    (7, 3),
    (-7, 3),
    (5, 6),
    (-5, 6),
    (6, 5),
    (-6, 5),
    (8, 0),
    (4, 7),
    (-4, 7),
    (7, 4),
    (-7, 4),
    (8, 1),
    (8, 2),
    (6, 6),
    (-6, 6),
    (8, 3),
    (5, 7),
    (-5, 7),
    (7, 5),
    (-7, 5),
    (8, 4),
    (6, 7),
    (-6, 7),
    (7, 6),
    (-7, 6),
    (8, 5),
    (7, 7),
    (-7, 7),
    (8, 6),
    (8, 7),
];

/// Translate a VP8L distance code (1-based) into a linear pixel distance.
fn map_distance(dist_code: u32, xsize: u32) -> u32 {
    if dist_code > DISTANCE_MAP.len() as u32 {
        return dist_code - DISTANCE_MAP.len() as u32;
    }
    let (dx, dy) = DISTANCE_MAP[(dist_code - 1) as usize];
    let d = dy * xsize as i32 + dx;
    if d < 1 {
        1
    } else {
        d as u32
    }
}

// ════════════════════════════════════════════════════════════════════════════
// VP8L decode: header, transforms, entropy image, and the main pixel loop.
// ════════════════════════════════════════════════════════════════════════════

const NUM_LITERAL_CODES: usize = 256;
const NUM_LENGTH_CODES: usize = 24;
const NUM_DISTANCE_CODES: usize = 40;

/// A transform that must be reversed on decode, in reverse order of application.
enum Transform {
    /// Predictor transform: per-tile prediction mode image, `bits` = tile size log2.
    Predictor { bits: u32, data: Vec<u32>, tw: u32 },
    /// Color transform: per-tile color-mixing image.
    Color { bits: u32, data: Vec<u32>, tw: u32 },
    /// Subtract-green: add green back to red and blue.
    SubtractGreen,
    /// Color-indexing (palette): map indices back to colors.
    ColorIndexing { table: Vec<u32>, table_size: u32 },
}

/// Decode a VP8L chunk body into ARGB8888.
fn decode_vp8l(data: &[u8]) -> Result<WebpImage, WebpError> {
    let mut br = Vp8lReader::new(data);

    // Signature byte 0x2F.
    let sig = br.read_bits(8)?;
    if sig != 0x2F {
        return Err(WebpError::BadVp8l);
    }
    // 14-bit width-1, 14-bit height-1, 1-bit alpha-used, 3-bit version (must be 0).
    let width = br.read_bits(14)? + 1;
    let height = br.read_bits(14)? + 1;
    let _alpha_used = br.read_bit()?;
    let version = br.read_bits(3)?;
    if version != 0 {
        return Err(WebpError::BadVp8l);
    }
    if width == 0 || height == 0 || width > MAX_DIMENSION || height > MAX_DIMENSION {
        return Err(WebpError::DimensionsOutOfRange);
    }
    if (width as u64) * (height as u64) > MAX_PIXELS {
        return Err(WebpError::DimensionsOutOfRange);
    }

    // ── Read the transform chain. `xsize` shrinks if a color-index transform
    // packs multiple pixels per byte. We track the working width separately. ──
    let mut transforms: Vec<Transform> = Vec::new();
    let mut xsize = width;
    let mut guard = 4usize; // at most 4 transforms (each type once)
    while br.read_bit()? == 1 {
        if guard == 0 {
            return Err(WebpError::BadTransform);
        }
        guard -= 1;
        let ttype = br.read_bits(2)?;
        match ttype {
            0 => {
                // PREDICTOR_TRANSFORM
                let bits = br.read_bits(3)? + 2;
                let (data, tw) = read_transform_image(&mut br, xsize, height, bits)?;
                transforms.push(Transform::Predictor { bits, data, tw });
            }
            1 => {
                // COLOR_TRANSFORM
                let bits = br.read_bits(3)? + 2;
                let (data, tw) = read_transform_image(&mut br, xsize, height, bits)?;
                transforms.push(Transform::Color { bits, data, tw });
            }
            2 => {
                // SUBTRACT_GREEN
                transforms.push(Transform::SubtractGreen);
            }
            3 => {
                // COLOR_INDEXING_TRANSFORM
                let table_size = br.read_bits(8)? + 1;
                // The palette is itself an image of `table_size` x 1 pixels, then
                // it is delta-coded (each entry adds the previous).
                let table = read_color_index_table(&mut br, table_size)?;
                // Pixels are packed: how many indices share a byte/pixel.
                let new_xsize = packed_xsize(xsize, table_size);
                transforms.push(Transform::ColorIndexing { table, table_size });
                xsize = new_xsize;
            }
            _ => return Err(WebpError::BadTransform),
        }
    }

    // ── Decode the entropy-coded image at the (possibly reduced) xsize. ──
    let pixels = decode_image_data(&mut br, xsize, height, true)?;

    // ── Reverse the transforms (in reverse order of application). ──
    let mut argb = pixels;
    let mut cur_xsize = xsize;
    while let Some(t) = transforms.pop() {
        match t {
            Transform::ColorIndexing { table, table_size } => {
                argb = inverse_color_indexing(&argb, cur_xsize, width, height, &table, table_size)?;
                cur_xsize = width;
            }
            Transform::SubtractGreen => {
                inverse_subtract_green(&mut argb);
            }
            Transform::Color { bits, data, tw } => {
                inverse_color_transform(&mut argb, cur_xsize, height, bits, &data, tw)?;
            }
            Transform::Predictor { bits, data, tw } => {
                inverse_predictor(&mut argb, cur_xsize, height, bits, &data, tw)?;
            }
        }
    }

    if argb.len() != (width as usize) * (height as usize) {
        return Err(WebpError::BadImageData);
    }

    Ok(WebpImage {
        width,
        height,
        pixels: argb,
    })
}

/// How many entropy-image / transform-image tiles wide for a given pixel width.
#[inline]
fn subsample_size(size: u32, bits: u32) -> u32 {
    (size + (1 << bits) - 1) >> bits
}

/// Read a transform's per-tile sub-image (predictor / color). Returns the tile
/// data and the tile-image width.
fn read_transform_image(
    br: &mut Vp8lReader,
    xsize: u32,
    ysize: u32,
    bits: u32,
) -> Result<(Vec<u32>, u32), WebpError> {
    let tw = subsample_size(xsize, bits);
    let th = subsample_size(ysize, bits);
    let data = decode_image_data(br, tw, th, false)?;
    Ok((data, tw))
}

/// Read the color-index (palette) table: `table_size` ARGB entries, decoded as a
/// `table_size` x 1 image, then delta-decoded (each entry += previous).
fn read_color_index_table(br: &mut Vp8lReader, table_size: u32) -> Result<Vec<u32>, WebpError> {
    let raw = decode_image_data(br, table_size, 1, false)?;
    let mut table = raw;
    for i in 1..table.len() {
        let prev = table[i - 1];
        let cur = table[i];
        table[i] = add_argb(cur, prev);
    }
    Ok(table)
}

/// Component-wise (mod 256) add of two packed ARGB values (palette delta-coding).
#[inline]
fn add_argb(a: u32, b: u32) -> u32 {
    let aa = px_a(a).wrapping_add(px_a(b));
    let ar = px_r(a).wrapping_add(px_r(b));
    let ag = px_g(a).wrapping_add(px_g(b));
    let ab = px_b(a).wrapping_add(px_b(b));
    make_argb(aa, ar, ag, ab)
}

/// Bits-per-pixel packing for the color-index transform, and the resulting width.
#[inline]
fn packed_xsize(xsize: u32, table_size: u32) -> u32 {
    let width_bits = if table_size <= 2 {
        3 // 1 bit per pixel → 8 per byte (but width packs by powers of two)
    } else if table_size <= 4 {
        2
    } else if table_size <= 16 {
        1
    } else {
        0
    };
    if width_bits == 0 {
        xsize
    } else {
        let pixels_per = 1u32 << width_bits;
        (xsize + pixels_per - 1) >> width_bits
    }
}

// ════════════════════════════════════════════════════════════════════════════
// Meta-Huffman group table + entropy image.
// ════════════════════════════════════════════════════════════════════════════

/// One meta-Huffman group: 5 canonical Huffman codes.
struct HuffmanGroup {
    green: Huffman, // green + length codes + color-cache codes
    red: Huffman,
    blue: Huffman,
    alpha: Huffman,
    distance: Huffman,
}

/// Decode the entropy-coded image data of `xsize` x `ysize` ARGB pixels.
///
/// `allow_meta`: only the top-level image may carry a meta-Huffman / color-cache
/// header; transform sub-images do not.
fn decode_image_data(
    br: &mut Vp8lReader,
    xsize: u32,
    ysize: u32,
    allow_meta: bool,
) -> Result<Vec<u32>, WebpError> {
    let num_pixels = (xsize as u64) * (ysize as u64);
    if num_pixels == 0 {
        return Ok(Vec::new());
    }
    if num_pixels > MAX_PIXELS {
        return Err(WebpError::DimensionsOutOfRange);
    }

    // ── Color cache ──────────────────────────────────────────────────────────
    let mut color_cache_bits = 0u32;
    if br.read_bit()? == 1 {
        color_cache_bits = br.read_bits(4)?;
        if color_cache_bits < 1 || color_cache_bits > 11 {
            return Err(WebpError::BadImageData);
        }
    }
    let cache_size = if color_cache_bits > 0 {
        1usize << color_cache_bits
    } else {
        0
    };
    let mut color_cache: Vec<u32> = vec![0u32; cache_size];

    // ── Meta-Huffman (entropy image) ──────────────────────────────────────────
    let mut huffman_bits = 0u32;
    let mut huffman_xsize = 1u32;
    let mut entropy_image: Vec<u32> = Vec::new();
    let mut num_groups = 1usize;
    if allow_meta && br.read_bit()? == 1 {
        huffman_bits = br.read_bits(3)? + 2;
        huffman_xsize = subsample_size(xsize, huffman_bits);
        let huffman_ysize = subsample_size(ysize, huffman_bits);
        entropy_image = decode_image_data(br, huffman_xsize, huffman_ysize, false)?;
        // The group index is packed into the red+green bytes of each entropy pixel.
        let mut max_group = 0usize;
        for &p in &entropy_image {
            let g = (((px_r(p) as usize) << 8) | px_g(p) as usize) + 1;
            if g > max_group {
                max_group = g;
            }
        }
        num_groups = max_group.max(1);
    }
    if num_groups > 1 << 16 {
        return Err(WebpError::BadImageData);
    }

    // ── Read all Huffman groups ──
    // The green alphabet includes color-cache codes when a cache is present.
    let green_alphabet = NUM_LITERAL_CODES + NUM_LENGTH_CODES + cache_size;
    let mut groups: Vec<HuffmanGroup> = Vec::with_capacity(num_groups);
    for _ in 0..num_groups {
        let green = read_huffman_code(br, green_alphabet)?;
        let red = read_huffman_code(br, NUM_LITERAL_CODES)?;
        let blue = read_huffman_code(br, NUM_LITERAL_CODES)?;
        let alpha = read_huffman_code(br, NUM_LITERAL_CODES)?;
        let distance = read_huffman_code(br, NUM_DISTANCE_CODES)?;
        groups.push(HuffmanGroup {
            green,
            red,
            blue,
            alpha,
            distance,
        });
    }

    // ── Main decode loop ──
    let total = num_pixels as usize;
    let mut out: Vec<u32> = vec![0u32; total];
    let xsize_us = xsize as usize;
    let mut idx = 0usize;
    // Bound the total number of decode operations to defeat a crafted stream that
    // never advances. Each op writes ≥1 pixel, so total ops ≤ total pixels.
    let mut op_guard = total
        .checked_add(total)
        .and_then(|v| v.checked_add(16))
        .ok_or(WebpError::BadImageData)?;

    while idx < total {
        if op_guard == 0 {
            return Err(WebpError::BadImageData);
        }
        op_guard -= 1;

        // Select the Huffman group for this pixel position.
        let group = if num_groups == 1 {
            &groups[0]
        } else {
            let x = (idx % xsize_us) as u32;
            let y = (idx / xsize_us) as u32;
            let gx = x >> huffman_bits;
            let gy = y >> huffman_bits;
            let meta_idx = (gy * huffman_xsize + gx) as usize;
            let entropy = *entropy_image.get(meta_idx).ok_or(WebpError::BadImageData)?;
            let gi = ((px_r(entropy) as usize) << 8) | px_g(entropy) as usize;
            groups.get(gi).ok_or(WebpError::BadImageData)?
        };

        let green_sym = group.green.decode(br)?;
        if (green_sym as usize) < NUM_LITERAL_CODES {
            // Literal ARGB pixel: green is decoded, then R, B, A from their codes.
            let g = green_sym as u8;
            let r = group.red.decode(br)? as u8;
            let b = group.blue.decode(br)? as u8;
            let a = group.alpha.decode(br)? as u8;
            let pixel = make_argb(a, r, g, b);
            out[idx] = pixel;
            if cache_size > 0 {
                let key = color_cache_key(pixel, color_cache_bits);
                color_cache[key] = pixel;
            }
            idx += 1;
        } else if (green_sym as usize) < NUM_LITERAL_CODES + NUM_LENGTH_CODES {
            // Backward reference (LZ77).
            let length_sym = green_sym - NUM_LITERAL_CODES as u16;
            let length = lz77_value(length_sym, br)? as usize;
            let dist_sym = group.distance.decode(br)?;
            let dist_code = lz77_value(dist_sym, br)?;
            let distance = map_distance(dist_code, xsize) as usize;
            if distance == 0 || distance > idx {
                return Err(WebpError::BadBackref);
            }
            if length == 0 || idx + length > total {
                return Err(WebpError::BadBackref);
            }
            let start = idx - distance;
            for k in 0..length {
                let v = out[start + k];
                out[idx] = v;
                if cache_size > 0 {
                    let key = color_cache_key(v, color_cache_bits);
                    color_cache[key] = v;
                }
                idx += 1;
            }
        } else {
            // Color-cache reference.
            if cache_size == 0 {
                return Err(WebpError::BadImageData);
            }
            let key = (green_sym as usize) - (NUM_LITERAL_CODES + NUM_LENGTH_CODES);
            let v = *color_cache.get(key).ok_or(WebpError::BadImageData)?;
            out[idx] = v;
            idx += 1;
        }
    }

    Ok(out)
}

/// VP8L color-cache hash: `(0x1e35a7bd * argb) >> (32 - cache_bits)`.
#[inline]
fn color_cache_key(argb: u32, cache_bits: u32) -> usize {
    ((0x1e35a7bdu32.wrapping_mul(argb)) >> (32 - cache_bits)) as usize
}

// ════════════════════════════════════════════════════════════════════════════
// Inverse transforms.
// ════════════════════════════════════════════════════════════════════════════

/// Inverse subtract-green: add the green channel to red and blue (mod 256).
fn inverse_subtract_green(argb: &mut [u32]) {
    for p in argb.iter_mut() {
        let g = px_g(*p);
        let r = px_r(*p).wrapping_add(g);
        let b = px_b(*p).wrapping_add(g);
        *p = make_argb(px_a(*p), r, g, b);
    }
}

/// Inverse color transform: per-tile, un-mix red/blue using green (and red).
fn inverse_color_transform(
    argb: &mut [u32],
    xsize: u32,
    ysize: u32,
    bits: u32,
    tiles: &[u32],
    tw: u32,
) -> Result<(), WebpError> {
    let xs = xsize as usize;
    for y in 0..ysize {
        for x in 0..xsize {
            let tx = x >> bits;
            let ty = y >> bits;
            let tidx = (ty * tw + tx) as usize;
            let t = *tiles.get(tidx).ok_or(WebpError::BadTransform)?;
            // The color-transform element packs three signed 8-bit multipliers:
            //   green_to_red = blue byte, green_to_blue = green byte,
            //   red_to_blue = red byte (per the VP8L spec layout).
            let green_to_red = px_b(t) as i8 as i32;
            let green_to_blue = px_g(t) as i8 as i32;
            let red_to_blue = px_r(t) as i8 as i32;

            let idx = (y as usize) * xs + (x as usize);
            let p = argb[idx];
            let mut red = px_r(p) as i32;
            let green = px_g(p) as i32;
            let mut blue = px_b(p) as i32;

            red += color_transform_delta(green_to_red, green as i8);
            red &= 0xff;
            blue += color_transform_delta(green_to_blue, green as i8);
            blue += color_transform_delta(red_to_blue, red as i8);
            blue &= 0xff;

            argb[idx] = make_argb(px_a(p), red as u8, green as u8, blue as u8);
        }
    }
    Ok(())
}

#[inline]
fn color_transform_delta(t: i32, c: i8) -> i32 {
    (t * c as i32) >> 5
}

/// Inverse predictor transform: per-pixel, add the predicted value back.
fn inverse_predictor(
    argb: &mut [u32],
    xsize: u32,
    ysize: u32,
    bits: u32,
    tiles: &[u32],
    tw: u32,
) -> Result<(), WebpError> {
    let xs = xsize as usize;
    let ys = ysize as usize;
    if xs == 0 || ys == 0 {
        return Ok(());
    }

    // Top-left pixel: predictor 0 (opaque black added), i.e. add 0xff000000.
    argb[0] = add_argb(argb[0], 0xff000000);

    // First row: predict from the left (predictor 1).
    for x in 1..xs {
        argb[x] = add_argb(argb[x], argb[x - 1]);
    }
    // First column: predict from above (predictor 2).
    for y in 1..ys {
        let idx = y * xs;
        argb[idx] = add_argb(argb[idx], argb[idx - xs]);
    }

    for y in 1..ys {
        for x in 1..xs {
            let tx = (x as u32) >> bits;
            let ty = (y as u32) >> bits;
            let tidx = (ty * tw + tx) as usize;
            let mode = px_g(*tiles.get(tidx).ok_or(WebpError::BadTransform)?);

            let idx = y * xs + x;
            let left = argb[idx - 1];
            let top = argb[idx - xs];
            let top_left = argb[idx - xs - 1];
            let top_right = if x + 1 < xs { argb[idx - xs + 1] } else { top };

            let pred = predict(mode, left, top, top_left, top_right);
            argb[idx] = add_argb(argb[idx], pred);
        }
    }
    Ok(())
}

/// The 14 VP8L predictor functions (mode 0..=13).
fn predict(mode: u8, left: u32, top: u32, tl: u32, tr: u32) -> u32 {
    match mode {
        0 => 0xff000000,
        1 => left,
        2 => top,
        3 => tr,
        4 => tl,
        5 => average2(average2(left, tr), top),
        6 => average2(left, tl),
        7 => average2(left, top),
        8 => average2(tl, top),
        9 => average2(top, tr),
        10 => average2(average2(left, tl), average2(top, tr)),
        11 => select(left, top, tl),
        12 => clamp_add_subtract_full(left, top, tl),
        13 => clamp_add_subtract_half(average2(left, top), tl),
        _ => 0xff000000,
    }
}

#[inline]
fn average2(a: u32, b: u32) -> u32 {
    let aa = ((px_a(a) as u32 + px_a(b) as u32) / 2) as u8;
    let ar = ((px_r(a) as u32 + px_r(b) as u32) / 2) as u8;
    let ag = ((px_g(a) as u32 + px_g(b) as u32) / 2) as u8;
    let ab = ((px_b(a) as u32 + px_b(b) as u32) / 2) as u8;
    make_argb(aa, ar, ag, ab)
}

/// Predictor 11: choose left or top by which is closer to the gradient L+T-TL.
fn select(left: u32, top: u32, tl: u32) -> u32 {
    let pa = abs_diff_sum(top, tl);
    let pb = abs_diff_sum(left, tl);
    if pa < pb {
        left
    } else {
        top
    }
}

#[inline]
fn abs_diff_sum(a: u32, b: u32) -> i32 {
    // Sum of |pred - channel| where pred = the OTHER's channel; used by `select`.
    (px_a(a) as i32 - px_a(b) as i32).abs()
        + (px_r(a) as i32 - px_r(b) as i32).abs()
        + (px_g(a) as i32 - px_g(b) as i32).abs()
        + (px_b(a) as i32 - px_b(b) as i32).abs()
}

#[inline]
fn clamp_u8(v: i32) -> u8 {
    if v < 0 {
        0
    } else if v > 255 {
        255
    } else {
        v as u8
    }
}

/// Predictor 12: ClampAddSubtractFull(L, T, TL) per channel = clamp(L + T - TL).
fn clamp_add_subtract_full(left: u32, top: u32, tl: u32) -> u32 {
    let f = |l: u8, t: u8, c: u8| clamp_u8(l as i32 + t as i32 - c as i32);
    make_argb(
        f(px_a(left), px_a(top), px_a(tl)),
        f(px_r(left), px_r(top), px_r(tl)),
        f(px_g(left), px_g(top), px_g(tl)),
        f(px_b(left), px_b(top), px_b(tl)),
    )
}

/// Predictor 13: ClampAddSubtractHalf(avg(L,T), TL) per channel.
fn clamp_add_subtract_half(avg: u32, tl: u32) -> u32 {
    let f = |a: u8, c: u8| {
        let av = a as i32;
        clamp_u8(av + (av - c as i32) / 2)
    };
    make_argb(
        f(px_a(avg), px_a(tl)),
        f(px_r(avg), px_r(tl)),
        f(px_g(avg), px_g(tl)),
        f(px_b(avg), px_b(tl)),
    )
}

/// Inverse color-indexing (palette): expand each (packed) index into its color.
fn inverse_color_indexing(
    indexed: &[u32],
    src_xsize: u32,
    dst_width: u32,
    height: u32,
    table: &[u32],
    table_size: u32,
) -> Result<Vec<u32>, WebpError> {
    let dst_w = dst_width as usize;
    let h = height as usize;
    let mut out = vec![0u32; dst_w * h];

    let width_bits = if table_size <= 2 {
        3
    } else if table_size <= 4 {
        2
    } else if table_size <= 16 {
        1
    } else {
        0
    };

    let lookup = |i: usize| -> u32 {
        // Out-of-range index → transparent black (spec: undefined; be safe).
        *table.get(i).unwrap_or(&0)
    };

    if width_bits == 0 {
        // One index per pixel; index lives in the green channel.
        if indexed.len() < dst_w * h {
            return Err(WebpError::BadImageData);
        }
        for y in 0..h {
            for x in 0..dst_w {
                let g = px_g(indexed[y * (src_xsize as usize) + x]) as usize;
                out[y * dst_w + x] = lookup(g);
            }
        }
    } else {
        let pixels_per = 1usize << width_bits;
        let bits_per = 8usize >> width_bits;
        let mask = (1usize << bits_per) - 1;
        let src_w = src_xsize as usize;
        for y in 0..h {
            for sx in 0..src_w {
                let packed = px_g(indexed[y * src_w + sx]) as usize;
                for sub in 0..pixels_per {
                    let dx = sx * pixels_per + sub;
                    if dx >= dst_w {
                        break;
                    }
                    let idx = (packed >> (bits_per * sub)) & mask;
                    out[y * dst_w + dx] = lookup(idx);
                }
            }
        }
    }
    Ok(out)
}

// ════════════════════════════════════════════════════════════════════════════
// Host KAT suite — `cargo test -p rae_webp`. FAIL-able by construction.
//
// Under `#[cfg(test)]` the crate compiles as `std` (the `no_std` attribute is
// `cfg_attr(not(test), ...)`), so `Vec`/`vec!` are in scope via the default
// prelude — no `extern crate std` / `use std::` (the architecture gate bans
// those std-ism lines).
//
// VP8L's bitstream is hand-buildable for small images: a `Vp8lWriter` mirrors the
// reader, and the tests encode a tiny lossless image (signature + 14-bit dims +
// a 1-symbol-per-channel "simple" Huffman code per group, one literal pixel each)
// to a concrete byte stream, wrap it in RIFF/WEBP, and assert the EXACT decoded
// ARGB pixels. This proves the container, the simple-Huffman path, the literal
// pixel loop, and each transform reversal end-to-end with concrete pixel asserts.
// ════════════════════════════════════════════════════════════════════════════
#[cfg(test)]
mod tests {
    use super::*;

    // ── LSB-first bit writer (mirror of Vp8lReader) ──────────────────────────
    struct Vp8lWriter {
        out: Vec<u8>,
        cur: u8,
        nbits: u32,
    }
    impl Vp8lWriter {
        fn new() -> Self {
            Self {
                out: Vec::new(),
                cur: 0,
                nbits: 0,
            }
        }
        fn put(&mut self, value: u32, n: u32) {
            for i in 0..n {
                let bit = ((value >> i) & 1) as u8;
                self.cur |= bit << self.nbits;
                self.nbits += 1;
                if self.nbits == 8 {
                    self.out.push(self.cur);
                    self.cur = 0;
                    self.nbits = 0;
                }
            }
        }
        fn finish(mut self) -> Vec<u8> {
            if self.nbits > 0 {
                self.out.push(self.cur);
            }
            self.out
        }
    }

    /// Write a "simple" 1-symbol Huffman code for `sym` (8-bit symbol form).
    /// Mirrors read_huffman_code's simple branch: bit 1 (is-simple), num_symbols-1
    /// = 0, first-bits = 1 (8-bit), then the 8-bit symbol.
    fn write_simple_code(w: &mut Vp8lWriter, sym: u8) {
        w.put(1, 1); // simple
        w.put(0, 1); // num_symbols - 1 = 0 → one symbol
        w.put(1, 1); // first symbol is 8-bit
        w.put(sym as u32, 8);
    }

    /// Build a VP8L stream for a single-Huffman-group image where EVERY pixel is
    /// the same literal color (g,r,b,a), no transforms, no color cache, no meta.
    /// `npix` literal pixels are emitted (each picks the single symbol, 0 bits).
    fn build_solid_vp8l(width: u32, height: u32, a: u8, r: u8, g: u8, b: u8) -> Vec<u8> {
        let mut w = Vp8lWriter::new();
        w.put(0x2F, 8); // signature
        w.put(width - 1, 14);
        w.put(height - 1, 14);
        w.put(0, 1); // alpha_used
        w.put(0, 3); // version
        w.put(0, 1); // no transform
                     // image data header:
        w.put(0, 1); // no color cache
        w.put(0, 1); // no meta-huffman (allow_meta true at top level)
                     // Huffman group: green, red, blue, alpha, distance.
        write_simple_code(&mut w, g); // green = literal green value
        write_simple_code(&mut w, r);
        write_simple_code(&mut w, b);
        write_simple_code(&mut w, a);
        write_simple_code(&mut w, 0); // distance (unused), 1 symbol
                                      // Now every pixel decodes as: green sym (0 bits) < 256 → literal, then
                                      // r,b,a each 0 bits. So no payload bits are needed for the pixels.
        let _ = (width, height);
        w.finish()
    }

    fn wrap_riff(fourcc: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut body = Vec::new();
        body.extend_from_slice(fourcc);
        body.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        body.extend_from_slice(payload);
        if payload.len() & 1 == 1 {
            body.push(0); // pad to even
        }
        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&((4 + body.len()) as u32).to_le_bytes());
        out.extend_from_slice(b"WEBP");
        out.extend_from_slice(&body);
        out
    }

    // ── 1. Container rejects non-WebP ────────────────────────────────────────
    #[test]
    fn rejects_non_webp() {
        assert_eq!(
            decode_webp(b"not a webp at all").unwrap_err(),
            WebpError::NotWebp
        );
        assert_eq!(decode_webp(&[]).unwrap_err(), WebpError::Truncated);
        // RIFF but not WEBP.
        let mut v = Vec::new();
        v.extend_from_slice(b"RIFF");
        v.extend_from_slice(&100u32.to_le_bytes());
        v.extend_from_slice(b"AVI ");
        v.extend_from_slice(&[0u8; 8]);
        assert_eq!(decode_webp(&v).unwrap_err(), WebpError::NotWebp);
    }

    // ── 2. VP8 lossy is honestly deferred ────────────────────────────────────
    #[test]
    fn vp8_lossy_unsupported() {
        let webp = wrap_riff(b"VP8 ", &[0u8; 16]);
        assert_eq!(decode_webp(&webp).unwrap_err(), WebpError::UnsupportedLossy);
    }

    // ── 3. THE load-bearing assert: solid-color VP8L → exact ARGB pixels ─────
    #[test]
    fn decode_solid_vp8l() {
        // 2x2 solid color: R=10, G=20, B=30, A=255.
        let payload = build_solid_vp8l(2, 2, 255, 10, 20, 30);
        let webp = wrap_riff(b"VP8L", &payload);
        let img = decode_webp(&webp).expect("solid vp8l decode");
        assert_eq!((img.width, img.height), (2, 2));
        for y in 0..2 {
            for x in 0..2 {
                assert_eq!(
                    img.pixel(x, y),
                    Some((255, 10, 20, 30)),
                    "pixel ({x},{y}) wrong"
                );
            }
        }
        // FAIL-ability: an R/B swap would make this (255, 30, 20, 10).
        assert_ne!(img.pixel(0, 0), Some((255, 30, 20, 10)));
    }

    // ── 4. Different solid color (proves channels aren't hard-coded) ─────────
    #[test]
    fn decode_solid_vp8l_other_color() {
        let payload = build_solid_vp8l(3, 1, 128, 200, 50, 7);
        let webp = wrap_riff(b"VP8L", &payload);
        let img = decode_webp(&webp).expect("decode");
        assert_eq!((img.width, img.height), (3, 1));
        assert_eq!(img.pixel(0, 0), Some((128, 200, 50, 7)));
        assert_eq!(img.pixel(2, 0), Some((128, 200, 50, 7)));
    }

    // ── 5. Canonical Huffman: a known multi-symbol code decodes correctly ────
    //
    // Build a 2-symbol "simple" code {symA=1bit, symB=1bit} and decode a known
    // bit pattern, proving the canonical decode path (not just the single-symbol
    // shortcut). With 2 symbols of length 1, canonical assignment is sym0→0, sym1→1.
    #[test]
    fn huffman_two_symbol_simple() {
        let mut w = Vp8lWriter::new();
        // simple, num_symbols-1=1, first-bits=1 (8-bit), sym0=5, sym1=9.
        w.put(1, 1);
        w.put(1, 1);
        w.put(1, 1);
        w.put(5, 8);
        w.put(9, 8);
        let bytes = w.finish();
        let mut br = Vp8lReader::new(&bytes);
        let h = read_huffman_code(&mut br, 256).expect("two-symbol code");
        // The two symbols were given in order (5, 9). Canonical lengths are both 1;
        // from_lengths assigns by ascending symbol index: symbol 5 → code 0,
        // symbol 9 → code 1. Decode a 0 then a 1.
        let mut br2 = Vp8lReader::new(&[0b10]); // bit0=0 → sym 5, bit1=1 → sym 9
        assert_eq!(h.decode(&mut br2).unwrap(), 5);
        assert_eq!(h.decode(&mut br2).unwrap(), 9);
    }

    // ── 6. Subtract-green transform reverses correctly ───────────────────────
    #[test]
    fn subtract_green_reverses() {
        // After subtract-green encoding, stored R' = R-G, B' = B-G (mod 256).
        // The inverse adds G back. Construct stored values and check recovery.
        // Original: R=100, G=40, B=200. Stored: R'=60, B'=160, G=40.
        let mut argb = vec![make_argb(255, 60, 40, 160)];
        inverse_subtract_green(&mut argb);
        let p = argb[0];
        assert_eq!((px_a(p), px_r(p), px_g(p), px_b(p)), (255, 100, 40, 200));
        // FAIL-ability: if inverse subtracted instead of added, R would be 20.
        assert_ne!(px_r(p), 20);
    }

    // ── 7. Predictor transform reverses (predictor 1 = left) end-to-end ──────
    //
    // Build a 2x1 image with a predictor transform whose single tile selects
    // mode 1 (predict-from-left). Pixel 0 stores (color - 0xff000000-pred);
    // verify the inverse reconstructs an intended gradient.
    #[test]
    fn predictor_left_reverses() {
        // Manually exercise inverse_predictor: residuals + predictor-1 tile.
        // Row of 3 pixels. After decode, residuals are stored; inverse adds preds.
        // We want output: p0, p1, p2 where p0 = 0xff000000 + r0,
        // p1 = p0 + r1, p2 = p1 + r2 (predictor 1 = left for x>=1).
        // Tile data = one tile, mode in green channel = 1.
        let tile = vec![make_argb(0, 0, 1, 0)]; // mode 1 in green
        let r0 = make_argb(10, 5, 6, 7); // residual for p0 (added to 0xff000000)
        let r1 = make_argb(1, 1, 1, 1);
        let r2 = make_argb(2, 2, 2, 2);
        let mut argb = vec![r0, r1, r2];
        // bits large enough that all 3 pixels fall in tile 0.
        inverse_predictor(&mut argb, 3, 1, 4, &tile, 1).unwrap();
        // p0 = 0xff000000 + r0 = (a=255+10 wrap=9? a: 0xff+10 = 0x109 → 0x09)
        let p0 = add_argb(r0, 0xff000000);
        let p1 = add_argb(r1, p0);
        let p2 = add_argb(r2, p1);
        assert_eq!(argb[0], p0);
        assert_eq!(argb[1], p1);
        assert_eq!(argb[2], p2);
    }

    /// Write a NORMAL Huffman code from explicit per-symbol code lengths (0 = no
    /// code). Mirrors read_huffman_code's normal branch. We transmit a code-length
    /// code (lengths-of-lengths) over symbols {0,1} (each length-symbol gets a
    /// 1-bit code-length code), then emit each symbol's length directly (no
    /// repeats). This lets a test carry symbols > 255 (e.g. color-cache codes)
    /// that the "simple" 8-bit form cannot express.
    fn write_normal_code(w: &mut Vp8lWriter, lengths: &[u16]) {
        w.put(0, 1); // not simple

        // We use code-length symbols 0 (=length 0) and 1 (=length 1) only, so the
        // code-length-code has lengths: cl[0]=1, cl[1]=1, rest 0 → a 1-bit prefix
        // code where cl-symbol 0 → bit 0, cl-symbol 1 → bit 1 (canonical).
        // num_code_lengths must cover indices 0 and 1 in CODE_LENGTH_CODE_ORDER.
        // ORDER = [17,18,0,1,...]; index 2 = symbol 0, index 3 = symbol 1.
        let num_code_lengths = 4; // covers ORDER[0..4] = {17,18,0,1}
        w.put((num_code_lengths - 4) as u32, 4);
        // ORDER[0]=17 → 0, ORDER[1]=18 → 0, ORDER[2]=0 → 1, ORDER[3]=1 → 1.
        w.put(0, 3); // cl for symbol 17
        w.put(0, 3); // cl for symbol 18
        w.put(1, 3); // cl for symbol 0  (length-value 0)
        w.put(1, 3); // cl for symbol 1  (length-value 1)

        w.put(0, 1); // no max_symbol limit (use full alphabet)

        // Emit each symbol's length via the 1-bit code-length code: value 0 → bit 0,
        // value 1 → bit 1. All our lengths are 0 or 1.
        for &l in lengths {
            match l {
                0 => w.put(0, 1),
                1 => w.put(1, 1),
                _ => panic!("write_normal_code only supports lengths 0/1"),
            }
        }
    }

    // ── 8. Color cache resolves (a back-reference + cache lookup) ────────────
    //
    // Build a VP8L stream with a color cache where the second pixel is emitted as
    // a color-cache reference to the first. This proves the cache insert+lookup.
    // The green code is a NORMAL Huffman code because the cache symbol (256+24+key)
    // exceeds 255 and cannot be carried by the 8-bit "simple" form.
    #[test]
    fn color_cache_resolves() {
        // 2x1 image. Color cache bits = 1 (size 2). First pixel literal color C;
        // its cache key = (0x1e35a7bd * C) >> 31. Second pixel = cache code.
        let color = make_argb(255, 11, 22, 33);
        let key = color_cache_key(color, 1);
        let cache_size = 2usize;
        let green_alphabet = NUM_LITERAL_CODES + NUM_LENGTH_CODES + cache_size; // 282
        let cache_sym = NUM_LITERAL_CODES + NUM_LENGTH_CODES + key; // 280 + key

        // Green code: two symbols of length 1 → {22, cache_sym}. Canonical: the
        // lower symbol index (22) → code 0, the higher (cache_sym) → code 1.
        let mut green_lengths = vec![0u16; green_alphabet];
        green_lengths[22] = 1;
        green_lengths[cache_sym] = 1;

        let mut w = Vp8lWriter::new();
        w.put(0x2F, 8);
        w.put(2 - 1, 14); // width 2
        w.put(1 - 1, 14); // height 1
        w.put(0, 1); // alpha used
        w.put(0, 3); // version
        w.put(0, 1); // no transform
                     // image data:
        w.put(1, 1); // color cache present
        w.put(1, 4); // color_cache_bits = 1 → size 2
        w.put(0, 1); // no meta-huffman
        write_normal_code(&mut w, &green_lengths); // green (carries cache symbol)
        write_simple_code(&mut w, 11); // red
        write_simple_code(&mut w, 33); // blue
        write_simple_code(&mut w, 255); // alpha
        write_simple_code(&mut w, 0); // distance
                                      // Pixel 0: green code 0 (bit 0) → literal green 22 → color C, cached.
                                      // Pixel 1: green code 1 (bit 1) → cache_sym → cache[key] = C.
        w.put(0, 1); // pixel 0 green = sym 22
        w.put(1, 1); // pixel 1 green = cache symbol
        let payload = w.finish();
        let webp = wrap_riff(b"VP8L", &payload);
        let img = decode_webp(&webp).expect("cache decode");
        assert_eq!((img.width, img.height), (2, 1));
        assert_eq!(img.pixel(0, 0), Some((255, 11, 22, 33)));
        assert_eq!(img.pixel(1, 0), Some((255, 11, 22, 33))); // resolved from cache
                                                              // FAIL-ability: if the cache weren't written on the literal, pixel 1 would
                                                              // read a zero/garbage cache slot, not the exact (255,11,22,33).
        assert_ne!(img.pixel(1, 0), Some((0, 0, 0, 0)));
    }

    // ── 9. Back-reference (LZ77) copies prior pixels (end-to-end decode) ─────
    #[test]
    fn backref_copies() {
        // 4x1 image. Pixel 0 is a literal color C (green 77). Then a single
        // back-reference of length 3, distance 1 fills pixels 1..4 with C.
        //
        // Green alphabet symbols used: literal green 77, and the length symbol
        // 256 + length_code. For length 3, lz77_value(code) = 3 needs code = 2
        // (sym<4 ⇒ value = sym+1), so the green length symbol is 256 + 2 = 258.
        // Distance: map_distance(dist_code, xsize) must equal 1. dist_code = 2 maps
        // via DISTANCE_MAP[1] = (1,0) ⇒ d = 0*xsize + 1 = 1; lz77_value(1) = 2, so
        // the distance symbol is 1 (no extra bits).
        let c_green = 77u8;
        let length_sym = NUM_LITERAL_CODES + 2; // 258
        let green_alphabet = NUM_LITERAL_CODES + NUM_LENGTH_CODES; // 280 (no cache)

        let mut green_lengths = vec![0u16; green_alphabet];
        green_lengths[c_green as usize] = 1; // literal green
        green_lengths[length_sym] = 1; // length symbol
                                       // Canonical: lower index (77) → code 0, higher (258) → code 1.

        let mut w = Vp8lWriter::new();
        w.put(0x2F, 8);
        w.put(4 - 1, 14);
        w.put(1 - 1, 14);
        w.put(0, 1);
        w.put(0, 3);
        w.put(0, 1); // no transform
        w.put(0, 1); // no cache
        w.put(0, 1); // no meta
        write_normal_code(&mut w, &green_lengths); // green (literal + length sym)
        write_simple_code(&mut w, 11); // red
        write_simple_code(&mut w, 33); // blue
        write_simple_code(&mut w, 255); // alpha
                                        // distance: single-symbol code carrying symbol 1.
        let mut dist_lengths = vec![0u16; NUM_DISTANCE_CODES];
        dist_lengths[1] = 1;
        write_normal_code(&mut w, &dist_lengths);

        // Pixel 0: green code 0 (bit 0) → literal 77 → C, then red/blue/alpha.
        w.put(0, 1); // green = literal symbol 77
                     // Back-reference: green code 1 (bit 1) → length symbol 258.
        w.put(1, 1); // green = length symbol
                     // lz77_value(258-256=2) returns 3 with 0 extra bits (no bits to read).
                     // distance symbol = 1 (single-symbol code, 0 bits), lz77_value(1)=2 →
                     // dist_code 2 → map_distance → 1.
        let payload = w.finish();
        let webp = wrap_riff(b"VP8L", &payload);
        let img = decode_webp(&webp).expect("backref decode");
        assert_eq!((img.width, img.height), (4, 1));
        let c = (255, 11, 77, 33);
        assert_eq!(img.pixel(0, 0), Some(c));
        assert_eq!(img.pixel(1, 0), Some(c)); // copied via back-reference
        assert_eq!(img.pixel(2, 0), Some(c));
        assert_eq!(img.pixel(3, 0), Some(c));
        // FAIL-ability: if the back-reference distance were wrong, the copy would
        // pull garbage (0,0,0,0) rather than the prior pixel.
        assert_ne!(img.pixel(3, 0), Some((0, 0, 0, 0)));
    }

    // ── 9b. Subtract-green transform reverses through the FULL decode path ────
    #[test]
    fn subtract_green_end_to_end() {
        // 2x1 image with a SUBTRACT_GREEN transform. The stored literal pixels hold
        // (R - G, G, B - G); the transform inverse adds G back. Original target:
        // R=100, G=40, B=200, A=255 → stored R'=60, B'=160.
        let mut w = Vp8lWriter::new();
        w.put(0x2F, 8);
        w.put(2 - 1, 14);
        w.put(1 - 1, 14);
        w.put(0, 1); // alpha used
        w.put(0, 3); // version
                     // ONE transform: present(1), type 2 = SUBTRACT_GREEN, then loop ends(0).
        w.put(1, 1); // transform present
        w.put(2, 2); // type 2 = subtract-green
        w.put(0, 1); // no more transforms
                     // image data: no cache, no meta, single Huffman group of literals.
        w.put(0, 1); // no cache
        w.put(0, 1); // no meta
        write_simple_code(&mut w, 40); // green = 40 (unchanged by subtract-green)
        write_simple_code(&mut w, 60); // red'  = 60
        write_simple_code(&mut w, 160); // blue' = 160
        write_simple_code(&mut w, 255); // alpha
        write_simple_code(&mut w, 0); // distance
        let payload = w.finish();
        let webp = wrap_riff(b"VP8L", &payload);
        let img = decode_webp(&webp).expect("subtract-green decode");
        assert_eq!((img.width, img.height), (2, 1));
        // After inverse subtract-green: R = 60+40 = 100, B = 160+40 = 200.
        assert_eq!(img.pixel(0, 0), Some((255, 100, 40, 200)));
        assert_eq!(img.pixel(1, 0), Some((255, 100, 40, 200)));
        // FAIL-ability: without the inverse transform the stored R'=60 would remain.
        assert_ne!(img.pixel(0, 0), Some((255, 60, 40, 160)));
    }

    // ── 10. Hostile-input battery: each returns Err, never panics/OOMs/hangs ──
    #[test]
    fn hostile_inputs_err_not_panic() {
        // Truncated RIFF.
        assert!(decode_webp(b"RIFF").is_err());
        // RIFF/WEBP but truncated chunk.
        let mut v = Vec::new();
        v.extend_from_slice(b"RIFF");
        v.extend_from_slice(&1000u32.to_le_bytes());
        v.extend_from_slice(b"WEBP");
        v.extend_from_slice(b"VP8L");
        v.extend_from_slice(&500u32.to_le_bytes()); // claims 500 bytes, has none
        assert!(decode_webp(&v).is_err());

        // VP8L with bad signature.
        let webp = wrap_riff(b"VP8L", &[0x00, 0, 0, 0, 0]);
        assert_eq!(decode_webp(&webp).unwrap_err(), WebpError::BadVp8l);

        // VP8L claiming absurd dimensions: width-1 = 0x3FFF (16384), height-1 =
        // 0x3FFF → 16384*16384 = 268M px > MAX_PIXELS.
        let mut w = Vp8lWriter::new();
        w.put(0x2F, 8);
        w.put(0x3FFF, 14);
        w.put(0x3FFF, 14);
        w.put(0, 1);
        w.put(0, 3);
        let big = wrap_riff(b"VP8L", &w.finish());
        assert_eq!(
            decode_webp(&big).unwrap_err(),
            WebpError::DimensionsOutOfRange
        );

        // VP8L header OK but the entropy data is empty/garbage → Err, no hang.
        let mut w2 = Vp8lWriter::new();
        w2.put(0x2F, 8);
        w2.put(7, 14); // width 8
        w2.put(7, 14); // height 8
        w2.put(0, 1);
        w2.put(0, 3);
        // truncate immediately — no transform/cache/huffman bytes follow.
        let truncated = wrap_riff(b"VP8L", &w2.finish());
        assert!(decode_webp(&truncated).is_err());
    }

    // ── 11. Seeded fuzz: random + RIFF-prefixed + mutated-valid stay bounded ──
    struct Rng(u64);
    impl Rng {
        fn new(seed: u64) -> Self {
            Rng(seed ^ 0x9E37_79B9_7F4A_7C15)
        }
        fn next(&mut self) -> u64 {
            let mut x = self.0;
            x ^= x >> 12;
            x ^= x << 25;
            x ^= x >> 27;
            self.0 = x;
            x.wrapping_mul(0x2545_F491_4F6C_DD1D)
        }
        fn byte(&mut self) -> u8 {
            (self.next() & 0xFF) as u8
        }
        fn below(&mut self, n: usize) -> usize {
            (self.next() % n as u64) as usize
        }
    }

    #[test]
    fn fuzz_random_never_panics() {
        let mut rng = Rng::new(0xC0FFEE);
        for _ in 0..20_000 {
            let len = rng.below(256);
            let mut buf = Vec::with_capacity(len);
            for _ in 0..len {
                buf.push(rng.byte());
            }
            let _ = decode_webp(&buf); // Ok or Err — never a panic.
        }
    }

    #[test]
    fn fuzz_riff_prefixed_never_panics() {
        let mut rng = Rng::new(0xBADF00D);
        for _ in 0..20_000 {
            let len = rng.below(200);
            let mut buf = Vec::new();
            buf.extend_from_slice(b"RIFF");
            buf.extend_from_slice(&(len as u32).to_le_bytes());
            buf.extend_from_slice(b"WEBP");
            let fourcc = match rng.below(4) {
                0 => b"VP8L".as_slice(),
                1 => b"VP8 ".as_slice(),
                2 => b"VP8X".as_slice(),
                _ => b"ANMF".as_slice(),
            };
            buf.extend_from_slice(fourcc);
            buf.extend_from_slice(&((len.saturating_sub(8)) as u32).to_le_bytes());
            for _ in 0..len {
                buf.push(rng.byte());
            }
            let _ = decode_webp(&buf);
        }
    }

    #[test]
    fn fuzz_mutated_valid_never_panics() {
        let valid = wrap_riff(b"VP8L", &build_solid_vp8l(4, 4, 255, 1, 2, 3));
        let mut rng = Rng::new(0x5EED);
        for _ in 0..20_000 {
            let mut buf = valid.clone();
            // Flip 1..4 random bytes.
            let flips = 1 + rng.below(4);
            for _ in 0..flips {
                if buf.is_empty() {
                    break;
                }
                let i = rng.below(buf.len());
                buf[i] ^= rng.byte();
            }
            let _ = decode_webp(&buf); // must never panic / hang / OOM
        }
    }
}
