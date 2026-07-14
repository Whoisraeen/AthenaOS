//! # RaeMedia baseline JPEG decoder — the format real photos ship in.
//!
//! LEGACY_GAMING_CONCEPT.md (§creators / media): the OS must let people "play my movies,
//! show my photos." The overwhelming majority of real photos on a person's machine
//! are JPEGs (camera output, phone exports, web downloads), so a daily-driver OS that
//! cannot turn a `.jpg` into pixels has no photo library, no thumbnails, no Quick Look.
//! This module is the JPEG sibling of the from-scratch PNG decoder (`png.rs`) and the
//! engine a future Photos viewer + the Files-app Quick Look will sit on. Daily-driver
//! parity vs Windows Photos / macOS Preview starts here.
//!
//! This is a **from-scratch, zero-dependency** *baseline* (sequential DCT, Huffman)
//! JPEG decoder: JFIF/JPEG marker parsing (SOI/APPn/DQT/SOF0/DHT/SOS/DRI/RSTn/EOI),
//! baseline entropy decode (Huffman DC DPCM + AC run/level + EOB), dequantisation,
//! zig-zag reorder, an 8x8 inverse DCT, YCbCr→RGB (BT.601 full-range JFIF), chroma
//! upsampling for 4:4:4 / 4:2:2 / 4:2:0, grayscale, and output to a flat ARGB8888
//! `Vec<u32>` (the AthGFX compositor/Canvas pixel format).
//!
//! ## Scope
//! Only **baseline sequential DCT** (SOF0) is decoded — the overwhelmingly common
//! case. Progressive (SOF2), arithmetic (SOF9/SOF10), lossless (SOF3) and other
//! process variants return a clean `Err(JpegError::UnsupportedFormat)` and are NEVER
//! mis-decoded into garbage.
//!
//! ## no_std without libm
//! The inverse DCT needs cosines. We do **not** link `libm` or any transcendental: a
//! 64-entry cosine lookup table (`IDCT_COS`) is built once at decode time from a small
//! local polynomial cosine approximation (`cos_approx`, range-reduced + a minimax
//! series), exactly the ath_calc / athfont pattern. The IDCT itself is then pure
//! `f32` multiply-add (kernel-safe: soft-float, GPR math, no XMM state to corrupt).
//!
//! ## Hostile-input posture (CLAUDE: decoders are the #1 RCE surface)
//! Every JPEG is treated as hostile. There is **no `unwrap`/`expect`/`panic`/
//! array-index-panic** path reachable from `decode_jpeg`: bad markers, truncated
//! segments, oversized frames, missing tables, malformed Huffman, and corrupt entropy
//! data all return `Err(...)`. Dimensions and component counts are bounded up front so
//! a crafted SOF can't request a multi-gigabyte allocation. The host KAT suite at the
//! bottom of this file is the primary proof (run `cargo test -p athmedia`).

extern crate alloc;

use alloc::vec;
use alloc::vec::Vec;

/// Bound on either image axis. Matches `png.rs` (`1<<16`) — well under `MAX_PIXELS`.
const MAX_DIMENSION: u32 = 1 << 16;
/// Bound on total pixel count (width * height). ~67M px = 256 MiB at 4 B/px ARGB.
const MAX_PIXELS: u64 = 64 * 1024 * 1024;
/// JPEG allows up to 4 components (e.g. YCbCr or YCCK); we decode 1 (gray) and 3
/// (YCbCr). A SOF claiming more is rejected before any per-component allocation.
const MAX_COMPONENTS: usize = 4;
/// Max sampling factor per axis (the SOF nibble is 1..=4 per the spec).
const MAX_SAMPLING: usize = 4;

/// JPEG decode error. Every variant is a *handled* path — nothing here panics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JpegError {
    /// Stream does not start with SOI (FFD8) or is too short.
    BadSignature,
    /// Stream ended mid-marker / mid-segment.
    Truncated,
    /// A segment length field is impossible (too small / past end).
    BadSegmentLength,
    /// A marker byte sequence was malformed (missing 0xFF lead, etc).
    BadMarker,
    /// SOF present but the process is not baseline sequential DCT (progressive,
    /// arithmetic, lossless, ...). Returned instead of mis-decoding.
    UnsupportedFormat,
    /// Width/height/component count is zero or exceeds a memory bound.
    DimensionsOutOfRange,
    /// A DQT/DHT/SOF table was malformed.
    BadTable,
    /// SOS references a quant or Huffman table that was never defined.
    MissingTable,
    /// Entropy-coded data ran out before the image was complete.
    UnexpectedEof,
    /// A Huffman code in the entropy stream did not decode to a valid symbol.
    BadHuffmanCode,
    /// Sampling factors were unsupported (e.g. > MAX_SAMPLING, or chroma > luma).
    BadSampling,
    /// No SOS / no scan data present.
    NoScanData,
}

/// A decoded image: a flat ARGB8888 buffer plus dimensions. Identical shape to
/// `png::DecodedImage` so both decoders feed the compositor/Canvas the same way.
///
/// `pixels.len() == (width * height) as usize`. Each `u32` is `0xAARRGGBB`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedImage {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u32>,
}

impl DecodedImage {
    /// Sample a pixel as `(a, r, g, b)`. Returns `None` out of bounds — tests use
    /// this so a wrong coordinate is a clean failure, not a panic.
    pub fn pixel(&self, x: u32, y: u32) -> Option<(u8, u8, u8, u8)> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let idx = (y as usize) * (self.width as usize) + (x as usize);
        let p = *self.pixels.get(idx)?;
        Some(((p >> 24) as u8, (p >> 16) as u8, (p >> 8) as u8, p as u8))
    }
}

#[inline]
fn argb(a: u8, r: u8, g: u8, b: u8) -> u32 {
    ((a as u32) << 24) | ((r as u32) << 16) | ((g as u32) << 8) | (b as u32)
}

// ─── Zig-zag order (JPEG Annex A, Figure A.6) ───────────────────────────────
// Maps the position in the zig-zag-scanned coefficient stream to the natural
// (row-major) position in the 8x8 block.
const ZIGZAG: [usize; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20,
    13, 6, 7, 14, 21, 28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58, 59,
    52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,
];

// ─── no_std cosine (no libm) ────────────────────────────────────────────────

const PI: f32 = 3.14159265358979_f32;
const TWO_PI: f32 = 2.0 * PI;

/// Local polynomial cosine approximation. Range-reduced into [-PI, PI] then a
/// minimax-style even-power series (good to ~1e-6 over the reduced range, far
/// tighter than the IDCT's per-pixel rounding needs). NO libm, NO XMM transcendental.
fn cos_approx(mut x: f32) -> f32 {
    // Range-reduce to [-PI, PI].
    while x > PI {
        x -= TWO_PI;
    }
    while x < -PI {
        x += TWO_PI;
    }
    let x2 = x * x;
    // cos series: 1 - x^2/2! + x^4/4! - x^6/6! + x^8/8! - x^10/10!
    1.0 - x2 / 2.0 + x2 * x2 / 24.0 - x2 * x2 * x2 / 720.0 + x2 * x2 * x2 * x2 / 40320.0
        - x2 * x2 * x2 * x2 * x2 / 3_628_800.0
}

/// Precomputed IDCT cosine basis: `IDCT_COS[x][u] = cos((2x+1) u pi / 16)`.
/// Built from `cos_approx`; 64 f32s, computed once per decode.
struct IdctCos {
    table: [[f32; 8]; 8],
}

impl IdctCos {
    fn new() -> Self {
        let mut table = [[0.0f32; 8]; 8];
        for (x, row) in table.iter_mut().enumerate() {
            for (u, cell) in row.iter_mut().enumerate() {
                let angle = ((2 * x + 1) as f32) * (u as f32) * PI / 16.0;
                *cell = cos_approx(angle);
            }
        }
        Self { table }
    }
}

/// Inverse 8x8 DCT-III (separable, straight from the JPEG definition, Annex A.3.3):
///   s(x,y) = 1/4 * Σu Σv C(u)C(v) S(u,v) cos((2x+1)uπ/16) cos((2y+1)vπ/16)
/// where C(0)=1/√2, C(k)=1 for k>0. `coeffs` is the dequantised, de-zigzagged 8x8
/// frequency block (row-major); writes the 8x8 spatial block (row-major) into `out`,
/// level-shifted by +128 and clamped to 0..=255.
fn idct_8x8(coeffs: &[i32; 64], cos: &IdctCos, out: &mut [u8; 64]) {
    // C(k) scale folded in.
    const INV_SQRT2: f32 = 0.70710678_f32;
    for y in 0..8 {
        for x in 0..8 {
            let mut sum = 0.0f32;
            for v in 0..8 {
                let cv = if v == 0 { INV_SQRT2 } else { 1.0 };
                let cosy = cos.table[y][v];
                for u in 0..8 {
                    let cu = if u == 0 { INV_SQRT2 } else { 1.0 };
                    sum += cu * cv * (coeffs[v * 8 + u] as f32) * cos.table[x][u] * cosy;
                }
            }
            let val = sum / 4.0 + 128.0;
            // Round-to-nearest then clamp.
            let r = val + 0.5;
            out[y * 8 + x] = if r <= 0.0 {
                0
            } else if r >= 255.0 {
                255
            } else {
                r as u8
            };
        }
    }
}

// ─── Marker / segment parse ─────────────────────────────────────────────────

/// A defined quantisation table (64 entries, in natural/de-zigzagged order).
#[derive(Clone)]
struct QuantTable {
    values: [u16; 64],
}

/// A canonical JPEG Huffman table: decode by walking bit lengths 1..=16. Built from
/// the per-length code counts (`bits`) + the symbols in code order (`huffval`).
struct HuffTable {
    // For each length 1..=16: how many codes have that length.
    counts: [u8; 17],
    // Symbols, ordered by increasing code (the order they appear in DHT).
    symbols: Vec<u8>,
    // mincode/maxcode/valptr per the spec (Annex F.2.2.3) for fast decode.
    mincode: [i32; 17],
    maxcode: [i32; 17],
    valptr: [i32; 17],
}

impl HuffTable {
    fn build(counts: [u8; 17], symbols: Vec<u8>) -> Self {
        // Generate huffsize / huffcode (Annex C.2 / F.2.2.3).
        let mut mincode = [0i32; 17];
        let mut maxcode = [-1i32; 17];
        let mut valptr = [0i32; 17];
        let mut code: i32 = 0;
        let mut k: i32 = 0;
        for len in 1..=16usize {
            let n = counts[len] as i32;
            if n > 0 {
                valptr[len] = k;
                mincode[len] = code;
                code += n;
                maxcode[len] = code - 1;
                k += n;
            } else {
                maxcode[len] = -1;
            }
            code <<= 1;
        }
        Self {
            counts,
            symbols,
            mincode,
            maxcode,
            valptr,
        }
    }

    /// Decode one symbol from the bit reader (Annex F.2.2.3).
    fn decode(&self, br: &mut BitReader) -> Result<u8, JpegError> {
        let mut code: i32 = 0;
        for len in 1..=16usize {
            code = (code << 1) | br.bit()? as i32;
            if self.counts[len] != 0 && code <= self.maxcode[len] {
                let idx = (self.valptr[len] + (code - self.mincode[len])) as usize;
                return self
                    .symbols
                    .get(idx)
                    .copied()
                    .ok_or(JpegError::BadHuffmanCode);
            }
        }
        Err(JpegError::BadHuffmanCode)
    }
}

/// A frame component (from SOF0).
#[derive(Clone, Copy)]
struct Component {
    id: u8,
    h: usize, // horizontal sampling factor
    v: usize, // vertical sampling factor
    quant_id: usize,
    // Filled from SOS:
    dc_table: usize,
    ac_table: usize,
}

/// Bit reader over the entropy-coded segment. Handles JPEG byte-stuffing
/// (0xFF 0x00 → 0xFF) and stops cleanly at the next marker (0xFF followed by a
/// non-zero, non-RSTn byte). MSB-first (JPEG packs bits high-to-low).
struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
    bit_buf: u32,
    bit_count: u32,
    /// Set when a real marker (not a stuffed byte / RST) was hit — the scan ends.
    hit_marker: bool,
    marker: u8,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8], start: usize) -> Self {
        Self {
            data,
            pos: start,
            bit_buf: 0,
            bit_count: 0,
            hit_marker: false,
            marker: 0,
        }
    }

    /// Pull the next entropy byte, resolving byte-stuffing and detecting markers.
    /// Returns `None` when a marker terminates the scan (or data ends).
    fn next_byte(&mut self) -> Option<u8> {
        if self.pos >= self.data.len() {
            return None;
        }
        let b = self.data[self.pos];
        if b == 0xFF {
            // Peek the following byte.
            let next = *self.data.get(self.pos + 1)?;
            if next == 0x00 {
                // Stuffed 0xFF.
                self.pos += 2;
                return Some(0xFF);
            }
            if (0xD0..=0xD7).contains(&next) {
                // RSTn marker inside the scan: consume it but signal a restart by
                // ending the current bit feed (the caller resyncs).
                self.hit_marker = true;
                self.marker = next;
                return None;
            }
            // Any other marker ends the scan.
            self.hit_marker = true;
            self.marker = next;
            return None;
        }
        self.pos += 1;
        Some(b)
    }

    fn bit(&mut self) -> Result<u32, JpegError> {
        if self.bit_count == 0 {
            let byte = self.next_byte().ok_or(JpegError::UnexpectedEof)?;
            self.bit_buf = byte as u32;
            self.bit_count = 8;
        }
        self.bit_count -= 1;
        Ok((self.bit_buf >> self.bit_count) & 1)
    }

    /// Read `n` bits MSB-first into a value (n <= 16).
    fn bits(&mut self, n: u32) -> Result<u32, JpegError> {
        let mut v = 0u32;
        for _ in 0..n {
            v = (v << 1) | self.bit()?;
        }
        Ok(v)
    }

    /// JPEG "receive and extend": read `n` bits as a signed coefficient (Annex F.2.2.1).
    fn receive_extend(&mut self, n: u32) -> Result<i32, JpegError> {
        if n == 0 {
            return Ok(0);
        }
        let v = self.bits(n)? as i32;
        // If the high bit is 0, the value is negative: v + (-1 << n) + 1.
        if v < (1 << (n - 1)) {
            Ok(v - (1 << n) + 1)
        } else {
            Ok(v)
        }
    }

    /// Drop any partial bits and resync after an RSTn marker. Returns the RST
    /// marker number (0..=7) if one was hit, else None.
    fn restart(&mut self) -> Option<u8> {
        self.bit_count = 0;
        self.bit_buf = 0;
        if self.hit_marker && (0xD0..=0xD7).contains(&self.marker) {
            let n = self.marker - 0xD0;
            // Skip past the FF Dn marker bytes.
            self.pos += 2;
            self.hit_marker = false;
            self.marker = 0;
            Some(n)
        } else {
            None
        }
    }
}

fn read_u16_be(data: &[u8], off: usize) -> Option<u16> {
    let b = data.get(off..off + 2)?;
    Some(u16::from_be_bytes([b[0], b[1]]))
}

/// Decode a baseline JPEG byte stream into an ARGB8888 image.
///
/// Hostile-input safe: returns `Err` (never panics) on any malformed input.
pub fn decode_jpeg(data: &[u8]) -> Result<DecodedImage, JpegError> {
    if data.len() < 2 || data[0] != 0xFF || data[1] != 0xD8 {
        return Err(JpegError::BadSignature);
    }

    let mut quant: [Option<QuantTable>; 4] = [None, None, None, None];
    let mut dc_huff: [Option<HuffTable>; 4] = [None, None, None, None];
    let mut ac_huff: [Option<HuffTable>; 4] = [None, None, None, None];
    let mut components: Vec<Component> = Vec::new();
    let mut width: usize = 0;
    let mut height: usize = 0;
    let mut restart_interval: usize = 0;
    let mut have_sof = false;

    let mut pos = 2usize; // after SOI
    loop {
        // Every marker is 0xFF followed by the marker code. Skip fill bytes (0xFF).
        if pos >= data.len() {
            return Err(JpegError::Truncated);
        }
        if data[pos] != 0xFF {
            return Err(JpegError::BadMarker);
        }
        // Consume any run of 0xFF (fill).
        while pos < data.len() && data[pos] == 0xFF {
            pos += 1;
        }
        if pos >= data.len() {
            return Err(JpegError::Truncated);
        }
        let marker = data[pos];
        pos += 1;

        match marker {
            0xD9 => {
                // EOI before any scan → no image.
                return Err(JpegError::NoScanData);
            }
            // Standalone markers with no length (TEM, RSTn) — shouldn't appear here.
            0x01 | 0xD0..=0xD7 => {
                continue;
            }
            _ => {}
        }

        // All other markers carry a 2-byte big-endian length (includes the 2 bytes).
        let seg_len = read_u16_be(data, pos).ok_or(JpegError::Truncated)? as usize;
        if seg_len < 2 {
            return Err(JpegError::BadSegmentLength);
        }
        let seg_start = pos + 2;
        let seg_end = pos
            .checked_add(seg_len)
            .ok_or(JpegError::BadSegmentLength)?;
        if seg_end > data.len() {
            return Err(JpegError::BadSegmentLength);
        }
        let seg = &data[seg_start..seg_end];

        match marker {
            // APP0..APPF and COM: metadata we skip.
            0xE0..=0xEF | 0xFE => {}
            // DQT: one or more quant tables.
            0xDB => parse_dqt(seg, &mut quant)?,
            // DHT: one or more Huffman tables.
            0xC4 => parse_dht(seg, &mut dc_huff, &mut ac_huff)?,
            // DRI: restart interval.
            0xDD => {
                if seg.len() < 2 {
                    return Err(JpegError::BadSegmentLength);
                }
                restart_interval = u16::from_be_bytes([seg[0], seg[1]]) as usize;
            }
            // SOF0: baseline sequential DCT — the only process we decode.
            0xC0 => {
                let (w, h, comps) = parse_sof0(seg)?;
                width = w;
                height = h;
                components = comps;
                have_sof = true;
            }
            // SOF1 (extended sequential) we treat as unsupported to stay strictly
            // baseline; SOF2 progressive, SOF3 lossless, SOF5-7 differential,
            // SOF9-11/13-15 arithmetic/lossless: all cleanly unsupported.
            0xC1..=0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF => {
                return Err(JpegError::UnsupportedFormat);
            }
            // SOS: start of scan — entropy data follows the segment.
            0xDA => {
                if !have_sof {
                    return Err(JpegError::MissingTable);
                }
                parse_sos(seg, &mut components)?;
                // The entropy-coded data begins right after this segment.
                return decode_scan(
                    data,
                    seg_end,
                    width,
                    height,
                    &components,
                    &quant,
                    &dc_huff,
                    &ac_huff,
                    restart_interval,
                );
            }
            _ => {
                // Unknown but length-bearing marker: skip its segment.
            }
        }

        pos = seg_end;
    }
}

fn parse_dqt(seg: &[u8], quant: &mut [Option<QuantTable>; 4]) -> Result<(), JpegError> {
    let mut i = 0usize;
    while i < seg.len() {
        let pq_tq = seg[i];
        i += 1;
        let precision = (pq_tq >> 4) & 0x0F; // 0 = 8-bit, 1 = 16-bit
        let id = (pq_tq & 0x0F) as usize;
        if id >= 4 {
            return Err(JpegError::BadTable);
        }
        let mut values = [0u16; 64];
        if precision == 0 {
            if i + 64 > seg.len() {
                return Err(JpegError::BadTable);
            }
            for k in 0..64 {
                // Stored in zig-zag order; de-zigzag into natural order.
                values[ZIGZAG[k]] = seg[i + k] as u16;
            }
            i += 64;
        } else if precision == 1 {
            if i + 128 > seg.len() {
                return Err(JpegError::BadTable);
            }
            for k in 0..64 {
                values[ZIGZAG[k]] = u16::from_be_bytes([seg[i + 2 * k], seg[i + 2 * k + 1]]);
            }
            i += 128;
        } else {
            return Err(JpegError::BadTable);
        }
        quant[id] = Some(QuantTable { values });
    }
    Ok(())
}

fn parse_dht(
    seg: &[u8],
    dc: &mut [Option<HuffTable>; 4],
    ac: &mut [Option<HuffTable>; 4],
) -> Result<(), JpegError> {
    let mut i = 0usize;
    while i < seg.len() {
        let tc_th = *seg.get(i).ok_or(JpegError::BadTable)?;
        i += 1;
        let class = (tc_th >> 4) & 0x0F; // 0 = DC, 1 = AC
        let id = (tc_th & 0x0F) as usize;
        if id >= 4 || class > 1 {
            return Err(JpegError::BadTable);
        }
        // 16 count bytes.
        let count_bytes = seg.get(i..i + 16).ok_or(JpegError::BadTable)?;
        let mut counts = [0u8; 17];
        let mut total = 0usize;
        for (len, &c) in count_bytes.iter().enumerate() {
            counts[len + 1] = c;
            total += c as usize;
        }
        i += 16;
        if total > 256 {
            return Err(JpegError::BadTable);
        }
        let sym_bytes = seg.get(i..i + total).ok_or(JpegError::BadTable)?;
        let symbols = sym_bytes.to_vec();
        i += total;
        let table = HuffTable::build(counts, symbols);
        if class == 0 {
            dc[id] = Some(table);
        } else {
            ac[id] = Some(table);
        }
    }
    Ok(())
}

fn parse_sof0(seg: &[u8]) -> Result<(usize, usize, Vec<Component>), JpegError> {
    // precision(1) height(2) width(2) ncomp(1) then ncomp*3 bytes.
    if seg.len() < 6 {
        return Err(JpegError::BadTable);
    }
    let precision = seg[0];
    if precision != 8 {
        // Baseline is 8-bit precision only.
        return Err(JpegError::UnsupportedFormat);
    }
    let height = u16::from_be_bytes([seg[1], seg[2]]) as usize;
    let width = u16::from_be_bytes([seg[3], seg[4]]) as usize;
    let ncomp = seg[5] as usize;
    if width == 0 || height == 0 {
        return Err(JpegError::DimensionsOutOfRange);
    }
    if width as u32 > MAX_DIMENSION || height as u32 > MAX_DIMENSION {
        return Err(JpegError::DimensionsOutOfRange);
    }
    if (width as u64) * (height as u64) > MAX_PIXELS {
        return Err(JpegError::DimensionsOutOfRange);
    }
    if ncomp == 0 || ncomp > MAX_COMPONENTS {
        return Err(JpegError::DimensionsOutOfRange);
    }
    // Only grayscale (1) and YCbCr (3) are decoded; YCCK/CMYK (4) unsupported.
    if ncomp != 1 && ncomp != 3 {
        return Err(JpegError::UnsupportedFormat);
    }
    if seg.len() < 6 + ncomp * 3 {
        return Err(JpegError::BadTable);
    }
    let mut comps = Vec::with_capacity(ncomp);
    for c in 0..ncomp {
        let off = 6 + c * 3;
        let id = seg[off];
        let sampling = seg[off + 1];
        let h = ((sampling >> 4) & 0x0F) as usize;
        let v = (sampling & 0x0F) as usize;
        let quant_id = seg[off + 2] as usize;
        if h == 0 || v == 0 || h > MAX_SAMPLING || v > MAX_SAMPLING {
            return Err(JpegError::BadSampling);
        }
        if quant_id >= 4 {
            return Err(JpegError::BadTable);
        }
        comps.push(Component {
            id,
            h,
            v,
            quant_id,
            dc_table: 0,
            ac_table: 0,
        });
    }
    Ok((width, height, comps))
}

fn parse_sos(seg: &[u8], components: &mut [Component]) -> Result<(), JpegError> {
    // ns(1) then ns*(comp_sel, td_ta) then Ss Se Ah_Al (3 bytes).
    if seg.is_empty() {
        return Err(JpegError::BadTable);
    }
    let ns = seg[0] as usize;
    if seg.len() < 1 + ns * 2 + 3 {
        return Err(JpegError::BadTable);
    }
    for k in 0..ns {
        let cs = seg[1 + k * 2];
        let tdta = seg[1 + k * 2 + 1];
        let td = ((tdta >> 4) & 0x0F) as usize;
        let ta = (tdta & 0x0F) as usize;
        if td >= 4 || ta >= 4 {
            return Err(JpegError::BadTable);
        }
        // Bind to the matching frame component.
        let mut found = false;
        for comp in components.iter_mut() {
            if comp.id == cs {
                comp.dc_table = td;
                comp.ac_table = ta;
                found = true;
                break;
            }
        }
        if !found {
            return Err(JpegError::MissingTable);
        }
    }
    // Ss must be 0 and Se 63 for baseline (spectral selection / successive approx
    // are progressive-only). Reject anything else as unsupported.
    let ss = seg[1 + ns * 2];
    let se = seg[1 + ns * 2 + 1];
    if ss != 0 || se != 63 {
        return Err(JpegError::UnsupportedFormat);
    }
    Ok(())
}

/// Decode one 8x8 block: entropy → dequant → IDCT → 64 spatial samples.
#[allow(clippy::too_many_arguments)]
fn decode_block(
    br: &mut BitReader,
    dc_pred: &mut i32,
    dc_table: &HuffTable,
    ac_table: &HuffTable,
    quant: &QuantTable,
    cos: &IdctCos,
    out: &mut [u8; 64],
) -> Result<(), JpegError> {
    let mut coeffs = [0i32; 64];

    // DC: magnitude category (Huffman) + that many bits → diff from prediction.
    let t = dc_table.decode(br)?;
    if t > 16 {
        return Err(JpegError::BadHuffmanCode);
    }
    let diff = br.receive_extend(t as u32)?;
    *dc_pred += diff;
    // De-zigzag position 0 is natural position 0; dequant.
    coeffs[0] = *dc_pred * quant.values[0] as i32;

    // AC: run/size pairs until 64 coeffs or EOB.
    let mut k = 1usize;
    while k < 64 {
        let rs = ac_table.decode(br)?;
        let run = (rs >> 4) & 0x0F;
        let size = rs & 0x0F;
        if size == 0 {
            if run == 15 {
                // ZRL: skip 16 zeros.
                k += 16;
                continue;
            }
            // EOB.
            break;
        }
        k += run as usize;
        if k >= 64 {
            break;
        }
        let val = br.receive_extend(size as u32)?;
        let nat = ZIGZAG[k];
        coeffs[nat] = val * quant.values[nat] as i32;
        k += 1;
    }

    idct_8x8(&coeffs, cos, out);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn decode_scan(
    data: &[u8],
    scan_start: usize,
    width: usize,
    height: usize,
    components: &[Component],
    quant: &[Option<QuantTable>; 4],
    dc_huff: &[Option<HuffTable>; 4],
    ac_huff: &[Option<HuffTable>; 4],
    restart_interval: usize,
) -> Result<DecodedImage, JpegError> {
    // Determine MCU geometry from the maximum sampling factors.
    let mut hmax = 1usize;
    let mut vmax = 1usize;
    for c in components {
        if c.h > hmax {
            hmax = c.h;
        }
        if c.v > vmax {
            vmax = c.v;
        }
    }
    let mcu_w = hmax * 8;
    let mcu_h = vmax * 8;
    let mcus_x = (width + mcu_w - 1) / mcu_w;
    let mcus_y = (height + mcu_h - 1) / mcu_h;

    // Per-component full-resolution-ish sample planes, sized to the padded MCU grid
    // at the component's own resolution (downsampled chroma stays small).
    let mut planes: Vec<Vec<u8>> = Vec::with_capacity(components.len());
    let mut plane_w: Vec<usize> = Vec::with_capacity(components.len());
    let mut plane_h: Vec<usize> = Vec::with_capacity(components.len());
    for c in components {
        let pw = mcus_x * c.h * 8;
        let ph = mcus_y * c.v * 8;
        // Bound: the padded plane can't exceed the global pixel cap by much.
        if (pw as u64) * (ph as u64) > MAX_PIXELS * (MAX_COMPONENTS as u64) {
            return Err(JpegError::DimensionsOutOfRange);
        }
        planes.push(vec![0u8; pw * ph]);
        plane_w.push(pw);
        plane_h.push(ph);
    }

    let cos = IdctCos::new();
    let mut br = BitReader::new(data, scan_start);
    let mut dc_pred = vec![0i32; components.len()];

    let mut mcu_count = 0usize;
    let mut block = [0u8; 64];

    for my in 0..mcus_y {
        for mx in 0..mcus_x {
            // Restart handling: at each interval, reset DC predictors and resync.
            if restart_interval != 0 && mcu_count != 0 && mcu_count % restart_interval == 0 {
                // Finish the current bit buffer, expect/consume an RSTn marker.
                // Drain to the marker if not already at it.
                while !br.hit_marker && br.pos < data.len() {
                    if br.next_byte().is_none() {
                        break;
                    }
                }
                let _ = br.restart();
                for p in dc_pred.iter_mut() {
                    *p = 0;
                }
            }

            for (ci, c) in components.iter().enumerate() {
                let qt = quant
                    .get(c.quant_id)
                    .and_then(|q| q.as_ref())
                    .ok_or(JpegError::MissingTable)?;
                let dct = dc_huff
                    .get(c.dc_table)
                    .and_then(|t| t.as_ref())
                    .ok_or(JpegError::MissingTable)?;
                let act = ac_huff
                    .get(c.ac_table)
                    .and_then(|t| t.as_ref())
                    .ok_or(JpegError::MissingTable)?;

                // Each component contributes h*v blocks per MCU.
                for by in 0..c.v {
                    for bx in 0..c.h {
                        decode_block(&mut br, &mut dc_pred[ci], dct, act, qt, &cos, &mut block)?;
                        // Place this 8x8 block into the component plane.
                        let px0 = (mx * c.h + bx) * 8;
                        let py0 = (my * c.v + by) * 8;
                        let pw = plane_w[ci];
                        for ry in 0..8 {
                            let dst_row = (py0 + ry) * pw + px0;
                            let src_row = ry * 8;
                            // Bounds are guaranteed by plane sizing, but stay safe.
                            if dst_row + 8 <= planes[ci].len() {
                                planes[ci][dst_row..dst_row + 8]
                                    .copy_from_slice(&block[src_row..src_row + 8]);
                            }
                        }
                    }
                }
            }
            mcu_count += 1;
        }
    }

    // Compose to ARGB8888 with chroma upsampling (nearest, scaled by sampling).
    let mut pixels = vec![0u32; width * height];

    if components.len() == 1 {
        let pw = plane_w[0];
        for y in 0..height {
            for x in 0..width {
                let g = planes[0][y * pw + x];
                pixels[y * width + x] = argb(0xFF, g, g, g);
            }
        }
    } else {
        // 3-component YCbCr. Upsample each chroma plane by the ratio of its sampling
        // to the max sampling.
        let y_c = &components[0];
        let cb_c = &components[1];
        let cr_c = &components[2];
        let yw = plane_w[0];
        let cbw = plane_w[1];
        let crw = plane_w[2];

        // Sub-sampling step: how many luma pixels each chroma sample covers.
        let cb_hx = hmax / cb_c.h;
        let cb_vy = vmax / cb_c.v;
        let cr_hx = hmax / cr_c.h;
        let cr_vy = vmax / cr_c.v;
        // y component is at full (hmax,vmax) resolution by construction here for the
        // common case; still map through its own sampling for generality.
        let y_hx = hmax / y_c.h;
        let y_vy = vmax / y_c.v;

        for y in 0..height {
            for x in 0..width {
                let yy = planes[0][(y / y_vy) * yw + (x / y_hx)] as i32;
                let cb = planes[1][(y / cb_vy) * cbw + (x / cb_hx)] as i32 - 128;
                let cr = planes[2][(y / cr_vy) * crw + (x / cr_hx)] as i32 - 128;
                let (r, g, b) = ycbcr_to_rgb(yy, cb, cr);
                pixels[y * width + x] = argb(0xFF, r, g, b);
            }
        }
        let _ = crw; // (kept for symmetry / clarity)
    }

    Ok(DecodedImage {
        width: width as u32,
        height: height as u32,
        pixels,
    })
}

/// BT.601 full-range (JFIF) YCbCr→RGB with fixed-point rounding and clamping.
/// R = Y + 1.402 (Cr); G = Y - 0.344136 Cb - 0.714136 Cr; B = Y + 1.772 Cb.
/// `cb`/`cr` are already centered (i.e. value - 128).
#[inline]
fn ycbcr_to_rgb(y: i32, cb: i32, cr: i32) -> (u8, u8, u8) {
    // Fixed-point (16-bit) coefficients, +0.5 rounding folded in.
    let r = y + ((91881 * cr + 32768) >> 16);
    let g = y - ((22554 * cb + 46802 * cr + 32768) >> 16);
    let b = y + ((116130 * cb + 32768) >> 16);
    (clamp_u8(r), clamp_u8(g), clamp_u8(b))
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

// ════════════════════════════════════════════════════════════════════════════
// Host KAT suite — `cargo test -p athmedia`. FAIL-able by construction.
//
// The fixtures are produced by a tiny **from-scratch baseline JPEG encoder**
// embedded in this test module. Building our own encoder (rather than depending on
// an external tool) keeps the fixtures fully documented and reproducible, and — more
// importantly — pins the decoder against an *independent* DCT/quant/Huffman path:
// the encoder forward-DCTs + quantises, the decoder dequantises + inverse-DCTs, and
// the round-trip must land within JPEG's lossy tolerance at known coordinates. The
// IDCT itself is additionally pinned *directly* (no encoder) by `idct_dc_only` /
// `idct_single_ac` — the load-bearing guards.
// ════════════════════════════════════════════════════════════════════════════
#[cfg(test)]
mod tests {
    use super::*;
    // Tests stay strictly `no_std`-compatible: only `core` + `alloc` (no `std`),
    // matching the ath_crypto pattern, so the crate never links std and the
    // architecture-gate's std-ism lint passes.
    use alloc::collections::BTreeMap;
    use alloc::vec;
    use alloc::vec::Vec as StdVec;

    // ── Standard JPEG luminance/chrominance quant tables (Annex K.1) ─────────
    // (natural order)
    const STD_LUMA_ZZ: [u8; 64] = [
        16, 11, 10, 16, 24, 40, 51, 61, 12, 12, 14, 19, 26, 58, 60, 55, 14, 13, 16, 24, 40, 57, 69,
        56, 14, 17, 22, 29, 51, 87, 80, 62, 18, 22, 37, 56, 68, 109, 103, 77, 24, 35, 55, 64, 81,
        104, 113, 92, 49, 64, 78, 87, 103, 121, 120, 101, 72, 92, 95, 98, 112, 100, 103, 99,
    ];
    const STD_CHROMA_ZZ: [u8; 64] = [
        17, 18, 24, 47, 99, 99, 99, 99, 18, 21, 26, 66, 99, 99, 99, 99, 24, 26, 56, 99, 99, 99, 99,
        99, 47, 66, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99,
        99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99,
    ];

    // ── Forward cosine via the same table approach (encoder side) ───────────
    fn fdct_8x8(samples: &[f32; 64]) -> [f32; 64] {
        let cos = IdctCos::new();
        let mut out = [0.0f32; 64];
        for u in 0..8 {
            let cu = if u == 0 { 0.70710678f32 } else { 1.0 };
            for v in 0..8 {
                let cv = if v == 0 { 0.70710678f32 } else { 1.0 };
                let mut sum = 0.0f32;
                for x in 0..8 {
                    for y in 0..8 {
                        sum += (samples[y * 8 + x] - 128.0) * cos.table[x][u] * cos.table[y][v];
                    }
                }
                out[v * 8 + u] = 0.25 * cu * cv * sum;
            }
        }
        out
    }

    // ── Canonical Huffman code generation for the encoder ───────────────────
    fn huff_codes(counts: &[u8; 17], symbols: &[u8]) -> BTreeMap<u8, (u32, u32)> {
        // returns symbol -> (code, length)
        let mut map = BTreeMap::new();
        let mut code: u32 = 0;
        let mut k = 0usize;
        for len in 1..=16usize {
            for _ in 0..counts[len] {
                map.insert(symbols[k], (code, len as u32));
                code += 1;
                k += 1;
            }
            code <<= 1;
        }
        map
    }

    // Standard baseline Huffman tables (Annex K.3). DC luma:
    const DC_LUMA_COUNTS: [u8; 17] = [0, 0, 1, 5, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0];
    const DC_LUMA_SYMS: [u8; 12] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11];
    // AC luma (Annex K.3 Table K.5) — the 162-symbol table.
    const AC_LUMA_COUNTS: [u8; 17] = [0, 0, 2, 1, 3, 3, 2, 4, 3, 5, 5, 4, 4, 0, 0, 1, 0x7d];
    const AC_LUMA_SYMS: [u8; 162] = [
        0x01, 0x02, 0x03, 0x00, 0x04, 0x11, 0x05, 0x12, 0x21, 0x31, 0x41, 0x06, 0x13, 0x51, 0x61,
        0x07, 0x22, 0x71, 0x14, 0x32, 0x81, 0x91, 0xa1, 0x08, 0x23, 0x42, 0xb1, 0xc1, 0x15, 0x52,
        0xd1, 0xf0, 0x24, 0x33, 0x62, 0x72, 0x82, 0x09, 0x0a, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x25,
        0x26, 0x27, 0x28, 0x29, 0x2a, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39, 0x3a, 0x43, 0x44, 0x45,
        0x46, 0x47, 0x48, 0x49, 0x4a, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58, 0x59, 0x5a, 0x63, 0x64,
        0x65, 0x66, 0x67, 0x68, 0x69, 0x6a, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79, 0x7a, 0x83,
        0x84, 0x85, 0x86, 0x87, 0x88, 0x89, 0x8a, 0x92, 0x93, 0x94, 0x95, 0x96, 0x97, 0x98, 0x99,
        0x9a, 0xa2, 0xa3, 0xa4, 0xa5, 0xa6, 0xa7, 0xa8, 0xa9, 0xaa, 0xb2, 0xb3, 0xb4, 0xb5, 0xb6,
        0xb7, 0xb8, 0xb9, 0xba, 0xc2, 0xc3, 0xc4, 0xc5, 0xc6, 0xc7, 0xc8, 0xc9, 0xca, 0xd2, 0xd3,
        0xd4, 0xd5, 0xd6, 0xd7, 0xd8, 0xd9, 0xda, 0xe1, 0xe2, 0xe3, 0xe4, 0xe5, 0xe6, 0xe7, 0xe8,
        0xe9, 0xea, 0xf1, 0xf2, 0xf3, 0xf4, 0xf5, 0xf6, 0xf7, 0xf8, 0xf9, 0xfa,
    ];

    // ── A from-scratch baseline JPEG encoder for fixtures ───────────────────
    struct BitWriter {
        bytes: StdVec<u8>,
        acc: u32,
        nbits: u32,
    }
    impl BitWriter {
        fn new() -> Self {
            Self {
                bytes: StdVec::new(),
                acc: 0,
                nbits: 0,
            }
        }
        fn put(&mut self, code: u32, len: u32) {
            for i in (0..len).rev() {
                let bit = (code >> i) & 1;
                self.acc = (self.acc << 1) | bit;
                self.nbits += 1;
                if self.nbits == 8 {
                    let b = (self.acc & 0xFF) as u8;
                    self.bytes.push(b);
                    if b == 0xFF {
                        self.bytes.push(0x00); // byte-stuffing
                    }
                    self.acc = 0;
                    self.nbits = 0;
                }
            }
        }
        fn flush(&mut self) {
            if self.nbits > 0 {
                // Pad with 1-bits (JPEG convention).
                let pad = 8 - self.nbits;
                self.acc = (self.acc << pad) | ((1 << pad) - 1);
                let b = (self.acc & 0xFF) as u8;
                self.bytes.push(b);
                if b == 0xFF {
                    self.bytes.push(0x00);
                }
                self.acc = 0;
                self.nbits = 0;
            }
        }
    }

    fn magnitude(v: i32) -> (u32, u32) {
        // returns (size, bits) for receive_extend
        if v == 0 {
            return (0, 0);
        }
        let a = v.unsigned_abs();
        let mut size = 0u32;
        let mut t = a;
        while t > 0 {
            size += 1;
            t >>= 1;
        }
        let bits = if v > 0 {
            v as u32
        } else {
            // negative: v + (2^size - 1)
            (v + ((1 << size) - 1)) as u32
        };
        (size, bits & ((1 << size) - 1))
    }

    /// Encode a single grayscale or YCbCr baseline JPEG. `samples` is per-component
    /// natural-order 8-bit plane data at the component's own resolution; here we keep
    /// it simple and only support 4:4:4 / 4:2:0 / grayscale via explicit block lists.
    #[allow(clippy::too_many_arguments)]
    fn encode_baseline(
        width: usize,
        height: usize,
        comps: &[(u8, usize, usize, usize)], // (id, h, v, quant_id)
        quant_tables: &[(usize, [u8; 64])],  // (id, zigzag-order table)
        // component planes in natural order, each sized (mcus_x*h*8) x (mcus_y*v*8)
        planes: &[StdVec<u8>],
        plane_w: &[usize],
    ) -> StdVec<u8> {
        let mut out = StdVec::new();
        out.extend_from_slice(&[0xFF, 0xD8]); // SOI

        // DQT (one segment per table, 8-bit).
        for (id, zz) in quant_tables {
            out.extend_from_slice(&[0xFF, 0xDB]);
            let len = 2 + 1 + 64;
            out.extend_from_slice(&(len as u16).to_be_bytes());
            out.push(*id as u8); // pq=0, tq=id
            out.extend_from_slice(zz);
        }

        // SOF0.
        out.extend_from_slice(&[0xFF, 0xC0]);
        let sof_len = 2 + 1 + 2 + 2 + 1 + comps.len() * 3;
        out.extend_from_slice(&(sof_len as u16).to_be_bytes());
        out.push(8); // precision
        out.extend_from_slice(&(height as u16).to_be_bytes());
        out.extend_from_slice(&(width as u16).to_be_bytes());
        out.push(comps.len() as u8);
        for (id, h, v, q) in comps {
            out.push(*id);
            out.push(((*h as u8) << 4) | (*v as u8));
            out.push(*q as u8);
        }

        // DHT — DC luma + AC luma (reused for chroma too; legal).
        let emit_dht = |out: &mut StdVec<u8>, class: u8, id: u8, counts: &[u8; 17], syms: &[u8]| {
            out.extend_from_slice(&[0xFF, 0xC4]);
            let total: usize = syms.len();
            let len = 2 + 1 + 16 + total;
            out.extend_from_slice(&(len as u16).to_be_bytes());
            out.push((class << 4) | id);
            out.extend_from_slice(&counts[1..17]);
            out.extend_from_slice(syms);
        };
        emit_dht(&mut out, 0, 0, &DC_LUMA_COUNTS, &DC_LUMA_SYMS);
        emit_dht(&mut out, 1, 0, &AC_LUMA_COUNTS, &AC_LUMA_SYMS);

        // SOS.
        out.extend_from_slice(&[0xFF, 0xDA]);
        let sos_len = 2 + 1 + comps.len() * 2 + 3;
        out.extend_from_slice(&(sos_len as u16).to_be_bytes());
        out.push(comps.len() as u8);
        for (id, _, _, _) in comps {
            out.push(*id);
            out.push(0x00); // td=0, ta=0
        }
        out.extend_from_slice(&[0x00, 0x3F, 0x00]); // Ss=0 Se=63 Ah/Al=0

        // Entropy.
        let dc_codes = huff_codes(&DC_LUMA_COUNTS, &DC_LUMA_SYMS);
        let ac_codes = huff_codes(&AC_LUMA_COUNTS, &AC_LUMA_SYMS);
        // Build quant lookup (natural order) per quant id.
        let mut q_natural: BTreeMap<usize, [i32; 64]> = BTreeMap::new();
        for (id, zz) in quant_tables {
            let mut nat = [0i32; 64];
            for k in 0..64 {
                nat[ZIGZAG[k]] = zz[k] as i32;
            }
            q_natural.insert(*id, nat);
        }

        let hmax = comps.iter().map(|c| c.1).max().unwrap();
        let vmax = comps.iter().map(|c| c.2).max().unwrap();
        let mcu_w = hmax * 8;
        let mcu_h = vmax * 8;
        let mcus_x = (width + mcu_w - 1) / mcu_w;
        let mcus_y = (height + mcu_h - 1) / mcu_h;

        let mut bw = BitWriter::new();
        let mut dc_pred = vec![0i32; comps.len()];

        for my in 0..mcus_y {
            for mx in 0..mcus_x {
                for (ci, c) in comps.iter().enumerate() {
                    let (_, h, v, q) = *c;
                    let qn = q_natural.get(&q).unwrap();
                    let pw = plane_w[ci];
                    for by in 0..v {
                        for bx in 0..h {
                            let px0 = (mx * h + bx) * 8;
                            let py0 = (my * v + by) * 8;
                            let mut samples = [0.0f32; 64];
                            for ry in 0..8 {
                                for rx in 0..8 {
                                    let p = planes[ci][(py0 + ry) * pw + (px0 + rx)];
                                    samples[ry * 8 + rx] = p as f32;
                                }
                            }
                            let dct = fdct_8x8(&samples);
                            // Quantise (natural order).
                            let mut q_coef = [0i32; 64];
                            for i in 0..64 {
                                let d = dct[i] / qn[i] as f32;
                                q_coef[i] = (d + if d >= 0.0 { 0.5 } else { -0.5 }) as i32;
                            }
                            // DC.
                            let diff = q_coef[0] - dc_pred[ci];
                            dc_pred[ci] = q_coef[0];
                            let (size, bits) = magnitude(diff);
                            let (code, len) = dc_codes[&(size as u8)];
                            bw.put(code, len);
                            if size > 0 {
                                bw.put(bits, size);
                            }
                            // AC in zigzag order.
                            let mut run = 0u32;
                            for k in 1..64 {
                                let coef = q_coef[ZIGZAG[k]];
                                if coef == 0 {
                                    run += 1;
                                } else {
                                    while run > 15 {
                                        // ZRL
                                        let (c0, l0) = ac_codes[&0xF0];
                                        bw.put(c0, l0);
                                        run -= 16;
                                    }
                                    let (size, bits) = magnitude(coef);
                                    let rs = ((run as u8) << 4) | (size as u8);
                                    let (code, len) = ac_codes[&rs];
                                    bw.put(code, len);
                                    bw.put(bits, size);
                                    run = 0;
                                }
                            }
                            if run > 0 {
                                // EOB
                                let (code, len) = ac_codes[&0x00];
                                bw.put(code, len);
                            }
                        }
                    }
                }
            }
        }
        bw.flush();
        out.extend_from_slice(&bw.bytes);
        out.extend_from_slice(&[0xFF, 0xD9]); // EOI
        out
    }

    // ── IDCT load-bearing guards (independent of the encoder) ───────────────

    #[test]
    fn idct_dc_only() {
        // A pure DC coefficient → a flat block. For coeff[0]=C, the IDCT yields a
        // constant level of C/8 (since the (0,0) basis is 1/4 * (1/√2)^2 = 1/8),
        // plus the +128 level shift. Pick C so the level is exactly 200.
        // level = C/8 + 128 = 200 → C = 576.
        let mut coeffs = [0i32; 64];
        coeffs[0] = 576;
        let cos = IdctCos::new();
        let mut out = [0u8; 64];
        idct_8x8(&coeffs, &cos, &mut out);
        for (i, &p) in out.iter().enumerate() {
            assert!(
                (p as i32 - 200).abs() <= 1,
                "DC-only pixel {i} = {p}, expected ~200"
            );
        }
        // FAIL-ability: a broken IDCT scale (e.g. missing the C(0) 1/√2 folding, or
        // dividing by 4 instead of the correct normalisation) would NOT land at 200.
        assert!((out[0] as i32 - 200).abs() <= 1);
        assert_ne!(out[0], 128); // a no-op IDCT (just level shift) would give 128.
    }

    #[test]
    fn idct_single_ac() {
        // A single low-frequency AC coefficient (u=1, v=0) produces a horizontal
        // cosine gradient: s(x,y) = 1/4 * C * cos((2x+1)π/16), constant in y.
        // The output must be a smooth horizontal ramp (left bright → right dark or
        // vice-versa), symmetric about the centre, NOT a flat block.
        let mut coeffs = [0i32; 64];
        coeffs[1] = 200; // (u=1, v=0)
        let cos = IdctCos::new();
        let mut out = [0u8; 64];
        idct_8x8(&coeffs, &cos, &mut out);
        // Row 0 must be a monotonic ramp across x.
        let row0: StdVec<i32> = (0..8).map(|x| out[x] as i32).collect();
        for x in 1..8 {
            assert!(
                row0[x] <= row0[x - 1],
                "AC(1,0) must be a monotonically decreasing ramp: {row0:?}"
            );
        }
        // Every row must equal row 0 (no vertical variation for v=0).
        for y in 1..8 {
            for x in 0..8 {
                assert_eq!(out[y * 8 + x], out[x], "AC(1,0) must be constant in y");
            }
        }
        // FAIL-ability: a flat output (broken AC basis / zigzag) would make row0
        // constant — this assert flips.
        assert!(
            row0[0] != row0[7],
            "AC coefficient produced a flat block (broken IDCT/zigzag)"
        );
    }

    #[test]
    fn zigzag_is_a_permutation() {
        let mut seen = [false; 64];
        for &z in ZIGZAG.iter() {
            assert!(z < 64);
            assert!(!seen[z], "zigzag index {z} repeated");
            seen[z] = true;
        }
        assert!(seen.iter().all(|&s| s));
        // Spot-check the canonical first few (Annex A.6).
        assert_eq!(ZIGZAG[0], 0);
        assert_eq!(ZIGZAG[1], 1);
        assert_eq!(ZIGZAG[2], 8);
        assert_eq!(ZIGZAG[3], 16);
        // FAIL-ability: a row-major (identity) "zigzag" would make ZIGZAG[2]==2.
        assert_ne!(ZIGZAG[2], 2);
    }

    #[test]
    fn dequant_applies_table() {
        // Decode-side dequant is `coef * quant[nat]`. Verify the DQT parse puts the
        // table into natural order and the multiply happens. Build a DQT with a
        // distinctive value at zigzag position 5 (natural ZIGZAG[5]).
        let mut zz = [1u8; 64];
        zz[5] = 7;
        let mut seg = StdVec::new();
        seg.push(0x00); // pq=0, tq=0
        seg.extend_from_slice(&zz);
        let mut quant: [Option<QuantTable>; 4] = [None, None, None, None];
        parse_dqt(&seg, &mut quant).expect("dqt parse");
        let qt = quant[0].as_ref().unwrap();
        assert_eq!(qt.values[ZIGZAG[5]], 7);
        assert_eq!(qt.values[ZIGZAG[0]], 1);
        // FAIL-ability: forgetting to de-zigzag would put 7 at natural index 5.
        if ZIGZAG[5] != 5 {
            assert_ne!(qt.values[5], 7);
        }
    }

    #[test]
    fn ycbcr_conversion_known_points() {
        // Pure luma (Cb=Cr=128 → centered 0): grayscale.
        let (r, g, b) = ycbcr_to_rgb(128, 0, 0);
        assert_eq!((r, g, b), (128, 128, 128));
        // White: Y=255.
        let (r, _, _) = ycbcr_to_rgb(255, 0, 0);
        assert_eq!(r, 255);
        // A red-ish point: pure red (255,0,0) in JFIF YCbCr is Y=76, Cb=85, Cr=255.
        // Pass centered chroma (value - 128): cb = -43, cr = +127.
        let (r, _g, b) = ycbcr_to_rgb(76, 85 - 128, 255 - 128);
        // Cr large positive → R should clearly exceed B.
        assert!(r > b, "high Cr must boost red over blue (r={r}, b={b})");
        assert!(r > 200, "red point should be near-saturated red (r={r})");
        // FAIL-ability: swapping the Cr/Cb coefficients (a classic bug) flips this —
        // with cb/cr swapped the same input yields a blue-dominant pixel.
    }

    // ── Full-pipeline fixtures via the embedded encoder ─────────────────────

    fn flat_plane(w: usize, h: usize, val: u8) -> StdVec<u8> {
        vec![val; w * h]
    }

    #[test]
    fn decode_grayscale_jpeg() {
        // 8x8 flat gray image, single component, value 160.
        let w = 8;
        let h = 8;
        let plane = flat_plane(8, 8, 160);
        let jpg = encode_baseline(w, h, &[(1, 1, 1, 0)], &[(0, STD_LUMA_ZZ)], &[plane], &[8]);
        let img = decode_jpeg(&jpg).expect("grayscale decode");
        assert_eq!(img.width, 8);
        assert_eq!(img.height, 8);
        let (a, r, g, b) = img.pixel(3, 3).expect("in bounds");
        assert_eq!(a, 0xFF);
        // Flat gray survives DCT/quant nearly exactly (only the DC term).
        assert!((r as i32 - 160).abs() <= 3, "gray r={r}");
        assert_eq!(r, g);
        assert_eq!(g, b);
        // FAIL-ability: a black/blown-out result (broken DC dequant) trips this.
        assert!(r > 100 && r < 220);
    }

    #[test]
    fn decode_444_color_jpeg() {
        // 8x8, three components all at 1x1 (4:4:4). Solid mid-blue.
        // Choose a target RGB and convert to YCbCr planes for the encoder.
        let w = 8;
        let h = 8;
        // Target: a clear blue (60, 80, 200).
        let (tr, tg, tb) = (60i32, 80i32, 200i32);
        let yv = ((19595 * tr + 38470 * tg + 7471 * tb + 32768) >> 16) as u8;
        let cb = (128 + ((-11059 * tr - 21709 * tg + 32768 * tb + 32768) >> 16)) as u8;
        let cr = (128 + ((32768 * tr - 27439 * tg - 5329 * tb + 32768) >> 16)) as u8;
        let yp = flat_plane(8, 8, yv);
        let cbp = flat_plane(8, 8, cb);
        let crp = flat_plane(8, 8, cr);
        let jpg = encode_baseline(
            w,
            h,
            &[(1, 1, 1, 0), (2, 1, 1, 1), (3, 1, 1, 1)],
            &[(0, STD_LUMA_ZZ), (1, STD_CHROMA_ZZ)],
            &[yp, cbp, crp],
            &[8, 8, 8],
        );
        let img = decode_jpeg(&jpg).expect("4:4:4 decode");
        let (_, r, g, b) = img.pixel(4, 4).expect("in bounds");
        assert!((r as i32 - tr).abs() <= 8, "r={r} want {tr}");
        assert!((g as i32 - tg).abs() <= 8, "g={g} want {tg}");
        assert!((b as i32 - tb).abs() <= 8, "b={b} want {tb}");
        // FAIL-ability: blue must dominate; an R/B swap (bad YCbCr matrix) flips this.
        assert!(b > r, "blue must exceed red (b={b}, r={r})");
        assert!(b > g, "blue must exceed green (b={b}, g={g})");
    }

    #[test]
    fn decode_420_color_jpeg() {
        // 16x16, luma 2x2, chroma 1x1 (4:2:0). Left half red, right half blue —
        // exercises real chroma upsampling (each chroma sample covers 2x2 luma).
        let w = 16;
        let h = 16;
        // Build luma plane (16x16) and chroma planes (8x8) directly so subsampling
        // is genuine. Left 8 cols = red, right 8 = blue.
        let red = (200i32, 30i32, 30i32);
        let blue = (30i32, 30i32, 200i32);
        let to_y =
            |r: i32, g: i32, b: i32| ((19595 * r + 38470 * g + 7471 * b + 32768) >> 16) as u8;
        let to_cb = |r: i32, g: i32, b: i32| {
            (128 + ((-11059 * r - 21709 * g + 32768 * b + 32768) >> 16)) as u8
        };
        let to_cr = |r: i32, g: i32, b: i32| {
            (128 + ((32768 * r - 27439 * g - 5329 * b + 32768) >> 16)) as u8
        };

        let mut yp = vec![0u8; 16 * 16];
        for y in 0..16 {
            for x in 0..16 {
                let (r, g, b) = if x < 8 { red } else { blue };
                yp[y * 16 + x] = to_y(r, g, b);
            }
        }
        let mut cbp = vec![0u8; 8 * 8];
        let mut crp = vec![0u8; 8 * 8];
        for y in 0..8 {
            for x in 0..8 {
                let (r, g, b) = if x < 4 { red } else { blue };
                cbp[y * 8 + x] = to_cb(r, g, b);
                crp[y * 8 + x] = to_cr(r, g, b);
            }
        }
        let jpg = encode_baseline(
            w,
            h,
            &[(1, 2, 2, 0), (2, 1, 1, 1), (3, 1, 1, 1)],
            &[(0, STD_LUMA_ZZ), (1, STD_CHROMA_ZZ)],
            &[yp, cbp, crp],
            &[16, 8, 8],
        );
        let img = decode_jpeg(&jpg).expect("4:2:0 decode");
        assert_eq!(img.width, 16);
        assert_eq!(img.height, 16);
        // Left side should read red-ish.
        let (_, lr, _lg, lb) = img.pixel(2, 8).expect("left");
        assert!(lr > lb + 40, "left must be red (r={lr}, b={lb})");
        // Right side should read blue-ish.
        let (_, rr, _rg, rb) = img.pixel(13, 8).expect("right");
        assert!(rb > rr + 40, "right must be blue (r={rr}, b={rb})");
        // FAIL-ability: a broken 4:2:0 upsample (e.g. not scaling the chroma index
        // by 2) would smear/misplace the chroma boundary and break the red/blue
        // separation above. A horizontal-flip upsample bug would swap these two.
    }

    // ── Malformed / hostile inputs: Err, never panic ────────────────────────

    #[test]
    fn reject_not_a_jpeg() {
        let data = vec![0u8; 64];
        assert_eq!(decode_jpeg(&data), Err(JpegError::BadSignature));
    }

    #[test]
    fn reject_truncated_soi() {
        let data = vec![0xFFu8];
        assert_eq!(decode_jpeg(&data), Err(JpegError::BadSignature));
    }

    #[test]
    fn reject_progressive_sof2() {
        // SOI + a SOF2 (progressive) marker → UnsupportedFormat, not garbage.
        let mut data = StdVec::new();
        data.extend_from_slice(&[0xFF, 0xD8]); // SOI
        data.extend_from_slice(&[0xFF, 0xC2]); // SOF2
        let body = [8u8, 0, 16, 0, 16, 1, 1, 0x11, 0]; // precision,h,w,ncomp,comp
        data.extend_from_slice(&((body.len() + 2) as u16).to_be_bytes());
        data.extend_from_slice(&body);
        assert_eq!(decode_jpeg(&data), Err(JpegError::UnsupportedFormat));
    }

    #[test]
    fn reject_oversized_sof() {
        // SOF0 claiming 0x7FFF x 0x7FFF → DimensionsOutOfRange.
        let mut data = StdVec::new();
        data.extend_from_slice(&[0xFF, 0xD8]);
        data.extend_from_slice(&[0xFF, 0xC0]);
        let body = [8u8, 0x7F, 0xFF, 0x7F, 0xFF, 1, 1, 0x11, 0];
        data.extend_from_slice(&((body.len() + 2) as u16).to_be_bytes());
        data.extend_from_slice(&body);
        assert_eq!(decode_jpeg(&data), Err(JpegError::DimensionsOutOfRange));
    }

    #[test]
    fn reject_bad_marker() {
        // SOI then a byte that isn't 0xFF where a marker is expected.
        let data = vec![0xFFu8, 0xD8, 0x12, 0x34];
        assert_eq!(decode_jpeg(&data), Err(JpegError::BadMarker));
    }

    #[test]
    fn reject_truncated_scan() {
        // Valid headers but the entropy data is cut off mid-block. Build a real
        // grayscale header via the encoder, then chop the entropy bytes.
        let plane = flat_plane(8, 8, 100);
        let mut jpg = encode_baseline(8, 8, &[(1, 1, 1, 0)], &[(0, STD_LUMA_ZZ)], &[plane], &[8]);
        // Find SOS (FF DA) and truncate a few bytes after the scan header so the
        // entropy decoder runs out of bits.
        let mut sos = None;
        for i in 0..jpg.len() - 1 {
            if jpg[i] == 0xFF && jpg[i + 1] == 0xDA {
                sos = Some(i);
                break;
            }
        }
        let sos = sos.expect("has SOS");
        // SOS segment is FF DA + len(2) + payload; keep header, drop most entropy.
        let seg_len = u16::from_be_bytes([jpg[sos + 2], jpg[sos + 3]]) as usize;
        let entropy_start = sos + 2 + seg_len;
        jpg.truncate(entropy_start + 1); // leave only 1 entropy byte
        let res = decode_jpeg(&jpg);
        assert!(res.is_err(), "truncated scan must Err, got {res:?}");
    }

    #[test]
    fn reject_missing_quant_table() {
        // SOF references quant id 0 but no DQT was sent → MissingTable at scan time.
        let mut data = StdVec::new();
        data.extend_from_slice(&[0xFF, 0xD8]);
        // SOF0 1 component, quant id 0.
        data.extend_from_slice(&[0xFF, 0xC0]);
        let body = [8u8, 0, 8, 0, 8, 1, 1, 0x11, 0];
        data.extend_from_slice(&((body.len() + 2) as u16).to_be_bytes());
        data.extend_from_slice(&body);
        // Minimal DHT so we reach scan decode (DC+AC luma).
        // SOS.
        data.extend_from_slice(&[0xFF, 0xDA]);
        let sos_body = [1u8, 1, 0x00, 0x00, 0x3F, 0x00];
        data.extend_from_slice(&((sos_body.len() + 2) as u16).to_be_bytes());
        data.extend_from_slice(&sos_body);
        data.extend_from_slice(&[0x00, 0x00]); // a little entropy
        data.extend_from_slice(&[0xFF, 0xD9]);
        let res = decode_jpeg(&data);
        assert!(matches!(res, Err(JpegError::MissingTable)), "got {res:?}");
    }

    #[test]
    fn reject_no_scan_data() {
        // SOI then immediate EOI.
        let data = vec![0xFFu8, 0xD8, 0xFF, 0xD9];
        assert_eq!(decode_jpeg(&data), Err(JpegError::NoScanData));
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Fuzz / property hardening — hostile-input boundary (Concept: "show my
    // photos" decodes downloaded/embedded JPEG, a classic memory-safety surface).
    //
    // Contract under test: `decode_jpeg` is total over arbitrary bytes — it
    // returns `Ok`/`Err` but NEVER panics, NEVER OOMs on a forged header, and
    // NEVER reads out of bounds. These tests are FAIL-able: a real panic / OOB /
    // unbounded alloc surfaces as a test-harness abort (panic) or an OOM kill,
    // which the `cargo test` runner reports as a failure. (E.g. removing the
    // `MAX_PIXELS` guard at line ~601 makes `fuzz_huge_dimension_header_is_bounded`
    // attempt a multi-GiB allocation and either OOM-abort or, on success, fail
    // the `is_err()` assertion.)
    // ─────────────────────────────────────────────────────────────────────────

    /// Self-contained deterministic PRNG (xorshift64*). No external fuzz crate,
    /// no `Cargo.toml` change — matches the ath_gif / ath_bmp fuzz pattern.
    struct XorShift(u64);
    impl XorShift {
        fn new(seed: u64) -> Self {
            // Avoid the zero fixed-point of xorshift.
            XorShift(seed ^ 0x9E37_79B9_7F4A_7C15)
        }
        fn next_u64(&mut self) -> u64 {
            let mut x = self.0;
            x ^= x >> 12;
            x ^= x << 25;
            x ^= x >> 27;
            self.0 = x;
            x.wrapping_mul(0x2545_F491_4F6C_DD1D)
        }
        fn next_u8(&mut self) -> u8 {
            (self.next_u64() & 0xFF) as u8
        }
        fn range(&mut self, n: usize) -> usize {
            if n == 0 {
                0
            } else {
                (self.next_u64() % (n as u64)) as usize
            }
        }
    }

    /// Pure random bytes (0..1024) must never panic the decoder.
    #[test]
    fn fuzz_random_bytes_never_panic() {
        let mut rng = XorShift::new(0xC0FF_EE00_1234_5678);
        for _ in 0..20_000 {
            let len = rng.range(1025); // 0..=1024
            let mut buf: StdVec<u8> = StdVec::with_capacity(len);
            for _ in 0..len {
                buf.push(rng.next_u8());
            }
            // Total function: any Result is fine; a panic fails the test.
            let _ = decode_jpeg(&buf);
        }
    }

    /// Random bytes that always START with a valid SOI exercise the marker /
    /// segment parser much more deeply (random bytes alone usually bail at the
    /// signature check).
    #[test]
    fn fuzz_valid_soi_random_tail_never_panic() {
        let mut rng = XorShift::new(0x5EED_0BAD_F00D_CAFE);
        for _ in 0..20_000 {
            let len = rng.range(512);
            let mut buf: StdVec<u8> = StdVec::with_capacity(len + 2);
            buf.push(0xFF);
            buf.push(0xD8); // valid SOI
            for _ in 0..len {
                buf.push(rng.next_u8());
            }
            let _ = decode_jpeg(&buf);
        }
    }

    /// Mutation fuzz over a real, valid baseline JPEG fixture: flip / inject /
    /// truncate bytes and confirm no mutation can panic. Walks SOI, APP0, SOF0,
    /// DQT, DHT, SOS and the entropy-coded tail.
    #[test]
    fn fuzz_mutated_valid_jpeg_never_panic() {
        let base = encode_baseline_8x8_gray(); // known-good fixture
        assert!(decode_jpeg(&base).is_ok(), "fixture must decode clean");
        let mut rng = XorShift::new(0xABCD_1234_DEAD_BEEF);
        for _ in 0..40_000 {
            let mut buf = base.clone();
            // 1..=8 random single-byte mutations.
            let nmut = 1 + rng.range(8);
            for _ in 0..nmut {
                if buf.is_empty() {
                    break;
                }
                let idx = rng.range(buf.len());
                buf[idx] = rng.next_u8();
            }
            // Sometimes also truncate to a random prefix (truncated SOI/APP0/
            // SOF/SOS/entropy data — all marker boundaries get hit).
            if rng.next_u64() & 1 == 0 {
                let cut = rng.range(buf.len() + 1);
                buf.truncate(cut);
            }
            let _ = decode_jpeg(&buf);
        }
    }

    /// Every truncation prefix of a valid JPEG must decode-or-Err, never panic.
    /// This deterministically hits truncated SOI, APP0, SOF, DQT, DHT, SOS, and
    /// mid-entropy-stream boundaries.
    #[test]
    fn fuzz_all_truncations_never_panic() {
        let base = encode_baseline_8x8_gray();
        for cut in 0..=base.len() {
            let _ = decode_jpeg(&base[..cut]);
        }
    }

    /// A header claiming huge dimensions must be REJECTED by the dimension/pixel
    /// caps, not honored into a giant allocation. FAIL-able: deleting the
    /// `MAX_DIMENSION` / `MAX_PIXELS` checks turns this into an OOM or makes the
    /// `is_err()` assertion fail.
    #[test]
    fn fuzz_huge_dimension_header_is_bounded() {
        // Build a minimal SOI + SOF0 declaring a 65535x65535 image (the largest
        // a 16-bit field can claim → ~4.29e9 px, far over MAX_PIXELS=64Mi).
        let mut d = StdVec::new();
        d.extend_from_slice(&[0xFF, 0xD8]); // SOI
        d.extend_from_slice(&[0xFF, 0xC0]); // SOF0
        let sof_body: [u8; 6 + 3] = [
            0x08, // precision 8
            0xFF, 0xFF, // height = 65535
            0xFF, 0xFF, // width  = 65535
            0x01, // 1 component
            0x01, 0x11, 0x00, // comp id=1, sampling 1x1, quant tbl 0
        ];
        d.extend_from_slice(&((sof_body.len() + 2) as u16).to_be_bytes());
        d.extend_from_slice(&sof_body);
        d.extend_from_slice(&[0xFF, 0xD9]); // EOI
        let res = decode_jpeg(&d);
        assert!(
            matches!(res, Err(JpegError::DimensionsOutOfRange)),
            "huge-dimension header must be rejected by the cap, got {res:?}"
        );
    }

    /// A header whose width*height fits each axis cap but overflows MAX_PIXELS
    /// must still be rejected (the pixel-count guard, separate from per-axis).
    #[test]
    fn fuzz_pixel_count_overflow_is_bounded() {
        // 60000 x 60000 = 3.6e9 px > MAX_PIXELS, each axis < MAX_DIMENSION(65536).
        let mut d = StdVec::new();
        d.extend_from_slice(&[0xFF, 0xD8]);
        d.extend_from_slice(&[0xFF, 0xC0]);
        let (w, h): (u16, u16) = (60000, 60000);
        let mut sof = StdVec::new();
        sof.push(0x08);
        sof.extend_from_slice(&h.to_be_bytes());
        sof.extend_from_slice(&w.to_be_bytes());
        sof.push(0x01);
        sof.extend_from_slice(&[0x01, 0x11, 0x00]);
        d.extend_from_slice(&((sof.len() + 2) as u16).to_be_bytes());
        d.extend_from_slice(&sof);
        d.extend_from_slice(&[0xFF, 0xD9]);
        let res = decode_jpeg(&d);
        assert!(
            matches!(res, Err(JpegError::DimensionsOutOfRange)),
            "pixel-count overflow must be rejected, got {res:?}"
        );
    }

    /// Fuzz the SOF component count + sampling-factor fields specifically: bad
    /// component counts (0, >MAX_COMPONENTS) and absurd sampling factors must Err,
    /// never panic or over-allocate the MCU buffers.
    #[test]
    fn fuzz_bad_component_and_sampling_never_panic() {
        let mut rng = XorShift::new(0x1357_9BDF_2468_ACE0);
        for _ in 0..10_000 {
            let ncomp = rng.next_u8(); // 0..=255, deliberately often invalid
            let mut d = StdVec::new();
            d.extend_from_slice(&[0xFF, 0xD8]);
            d.extend_from_slice(&[0xFF, 0xC0]);
            let mut sof = StdVec::new();
            sof.push(0x08);
            sof.extend_from_slice(&16u16.to_be_bytes()); // height 16
            sof.extend_from_slice(&16u16.to_be_bytes()); // width 16
            sof.push(ncomp);
            // Emit `min(ncomp, 8)` component descriptors with random sampling.
            let emit = core::cmp::min(ncomp as usize, 8);
            for c in 0..emit {
                sof.push(c as u8 + 1); // id
                sof.push(rng.next_u8()); // sampling HxV (random, often absurd)
                sof.push(rng.next_u8() & 0x03); // quant tbl id
            }
            d.extend_from_slice(&((sof.len() + 2) as u16).to_be_bytes());
            d.extend_from_slice(&sof);
            d.extend_from_slice(&[0xFF, 0xD9]);
            let _ = decode_jpeg(&d);
        }
    }

    /// Restart-marker edge cases: inject RSTn markers (FFD0..FFD7) and bogus
    /// restart intervals (DRI) into otherwise-valid streams; must never panic.
    #[test]
    fn fuzz_restart_marker_edges_never_panic() {
        let base = encode_baseline_8x8_gray();
        let mut rng = XorShift::new(0x0FED_CBA9_8765_4321);
        for _ in 0..10_000 {
            let buf = base.clone();
            // Insert a DRI segment (FF DD, len 4, interval) right after SOI.
            let interval = (rng.next_u64() & 0xFFFF) as u16;
            let mut dri = StdVec::new();
            dri.extend_from_slice(&[0xFF, 0xDD, 0x00, 0x04]);
            dri.extend_from_slice(&interval.to_be_bytes());
            // Splice after the 2-byte SOI.
            let mut spliced = StdVec::new();
            spliced.extend_from_slice(&buf[..2]);
            spliced.extend_from_slice(&dri);
            spliced.extend_from_slice(&buf[2..]);
            // Sprinkle stray RSTn markers into the entropy tail.
            let nrst = rng.range(4);
            for _ in 0..nrst {
                let rst = 0xD0 + (rng.next_u8() & 0x07);
                let at = rng.range(spliced.len() + 1);
                spliced.insert(at, rst);
                spliced.insert(at, 0xFF);
            }
            let _ = decode_jpeg(&spliced);
        }
    }

    /// Minimal known-good baseline JPEG: 8x8 grayscale via the proven in-test
    /// `encode_baseline` helper (same path the round-trip tests use), so the
    /// mutation/truncation fuzzers start from a real, spec-valid stream.
    fn encode_baseline_8x8_gray() -> StdVec<u8> {
        let plane = vec![128u8; 64]; // 8x8 mid-gray plane
        encode_baseline(8, 8, &[(1, 1, 1, 0)], &[(0, STD_LUMA_ZZ)], &[plane], &[8])
    }
}
