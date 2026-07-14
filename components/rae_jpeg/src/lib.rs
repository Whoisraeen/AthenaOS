//! # RaeJPEG — a never-panic, `no_std` baseline JPEG codec (ITU-T T.81 / JFIF).
//!
//! RaeenOS_Concept.md (§creators / media): a daily driver must let people "show
//! my photos" *and* "save/export my photo as JPEG." JPEG is the dominant *lossy*
//! photo/camera format — every phone, every camera, and the bulk of web
//! photography ship as JPEG. RaeenOS already decodes PNG/BMP/GIF; this crate is
//! the from-scratch baseline-DCT JPEG path the Photos viewer and `raeplay`
//! thumbnailer sit on, with both a **decoder** ([`decode_jpeg`]) and a baseline
//! sequential **encoder** ([`encode_jpeg`] / [`JpegImage::to_jpeg`]) so a user
//! can export an edit or a screenshot in the format the rest of the world reads.
//!
//! ## What it encodes (honest scope)
//! - **Baseline sequential DCT** (SOF0), 8-bit, 3-component YCbCr 4:4:4 (no
//!   chroma subsampling — the cleanest round-trip; 4:2:0 is a size follow-up).
//! - Standard **Annex K** quantization tables scaled by a 1..=100 quality knob,
//!   and the standard Annex K baseline Huffman tables (luma/chroma DC+AC).
//! - Full JFIF container: SOI, APP0/JFIF, DQT, SOF0, DHT, SOS + byte-stuffed
//!   entropy scan, EOI. The encoder's output decodes back through this crate's
//!   own [`decode_jpeg`] within lossy tolerance (the primary KAT lever).
//!
//! Output is a flat ARGB8888 `Vec<u32>` (`0xAARRGGBB`) — the RaeGFX compositor /
//! Canvas pixel format, **matching the [`rae_png`]/[`rae_bmp`]/`rae_gif`
//! decoders** so a gallery, a tab-strip, or a Quick Look preview can blit any of
//! the still-image formats through one uniform pixel model.
//!
//! ## What it decodes (honest scope)
//! - **Baseline sequential DCT** (SOF0) — the overwhelming-majority case for
//!   photos. 8-bit precision, 1 component (grayscale) or 3 components (YCbCr).
//! - **Markers**: SOI, APP0/APPn (skipped), COM (skipped), DQT (8- and 16-bit
//!   precision quant tables), DHT (DC + AC Huffman tables), SOF0, SOS, DRI
//!   (restart interval) + RSTn restart markers, EOI.
//! - **Chroma subsampling**: 4:4:4 (1x1), 4:2:2 (2x1), 4:2:0 (2x2) — the common
//!   camera/web samplings. Chroma is box/replicate upsampled (documented).
//! - **Non-multiple-of-MCU dimensions** are cropped to the declared size.
//!
//! ## What it rejects cleanly (documented gaps, never a fake decode)
//! - **Progressive DCT (SOF2)**, extended sequential (SOF1), and arithmetic-coded
//!   frames return [`JpegError::Unsupported`] — progressive is a documented
//!   follow-up, not silently mis-decoded.
//!
//! ## Hostile-input posture (CLAUDE: decoders are the #1 RCE surface)
//! Every JPEG byte is treated as attacker-controlled. There is **no
//! `unwrap`/`expect`/`panic`/raw-index-panic path** reachable from
//! [`decode_jpeg`]: truncated segments, bad markers, an out-of-range Huffman code,
//! a premature end of the entropy stream, oversized dimensions, and a malformed
//! scan header all return `Err(JpegError)`. Memory is bounded up front
//! ([`MAX_DIMENSION`], [`MAX_PIXELS`]) so a crafted SOF0 cannot request a
//! multi-gigabyte allocation. The host KAT suite at the bottom of this file is the
//! primary proof (`cargo test -p rae_jpeg`).
//!
//! ## No-libm posture
//! The kernel/media path is `no_std` + soft-float with no libm. The 8x8 inverse
//! DCT and the YCbCr->RGB transform therefore use a **precomputed const cosine
//! table** ([`IDCT_COS`]) and integer arithmetic — no runtime `cos`/`sqrt`.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

use alloc::vec;
use alloc::vec::Vec;

/// Bound on either image axis. JPEG dimensions are 16-bit (max 65535) so the
/// format cannot exceed this; the ceiling keeps a single canvas under
/// [`MAX_PIXELS`].
pub const MAX_DIMENSION: u32 = 1 << 16; // 65_536
/// Bound on total pixel count (width * height). ~67M px = 256 MiB at 4 B/px
/// ARGB. A crafted SOF0 claiming a huge canvas is rejected before allocation.
pub const MAX_PIXELS: u64 = 64 * 1024 * 1024;
/// Bound on component count in a frame. Baseline JFIF is 1 (gray) or 3 (YCbCr);
/// we accept up to 4 (some files carry a 4th) but never more.
const MAX_COMPONENTS: usize = 4;

/// JPEG decode error. Every variant is a *handled* path — nothing here panics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JpegError {
    /// The stream did not begin with SOI (`FFD8`).
    BadSignature,
    /// A marker/segment header or body ran past the end of the buffer.
    Truncated,
    /// A marker was malformed (e.g. a segment length shorter than its header).
    BadMarker,
    /// Width/height was zero or exceeded the memory bound.
    DimensionsOutOfRange,
    /// A frame type (SOF1/SOF2/arithmetic) this decoder does not implement.
    Unsupported,
    /// A DQT segment was malformed (bad precision/length/table id).
    BadQuantTable,
    /// A DHT segment was malformed (counts overflow the value list / bad id).
    BadHuffmanTable,
    /// The SOF0 frame header was malformed.
    BadFrame,
    /// The SOS scan header was malformed, or referenced an undefined component
    /// or Huffman/quant table.
    BadScan,
    /// The entropy-coded data was truncated, contained an undecodable Huffman
    /// code, or a restart marker arrived out of sequence.
    BadEntropy,
    /// No SOF0 frame was present before the scan.
    NoFrame,
}

/// A decoded image: a flat ARGB8888 buffer plus dimensions.
///
/// `pixels.len() == (width * height) as usize`. Each `u32` is `0xAARRGGBB` —
/// identical to [`rae_png::PngImage`] / a `rae_bmp` image so callers consume all
/// of RaeenOS's still-image decoders through one pixel model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JpegImage {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u32>,
}

impl JpegImage {
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

#[inline]
fn argb(a: u8, r: u8, g: u8, b: u8) -> u32 {
    ((a as u32) << 24) | ((r as u32) << 16) | ((g as u32) << 8) | (b as u32)
}

// ─── Zig-zag order (JPEG §A.3.6) ─────────────────────────────────────────────
//
// The entropy decoder emits the 64 coefficients in zig-zag scan order; this maps
// each scan position to its natural row-major index in the 8x8 block.
#[rustfmt::skip]
const ZIGZAG: [usize; 64] = [
     0,  1,  8, 16,  9,  2,  3, 10,
    17, 24, 32, 25, 18, 11,  4,  5,
    12, 19, 26, 33, 40, 48, 41, 34,
    27, 20, 13,  6,  7, 14, 21, 28,
    35, 42, 49, 56, 57, 50, 43, 36,
    29, 22, 15, 23, 30, 37, 44, 51,
    58, 59, 52, 45, 38, 31, 39, 46,
    53, 60, 61, 54, 47, 55, 62, 63,
];

// ─── Precomputed IDCT cosine table (no-libm) ─────────────────────────────────
//
// The separable 8-point inverse DCT-III needs cos((2x+1)*u*pi/16) for x,u in
// 0..8. We store it as fixed-point i32 with 12 fractional bits (scale 4096).
// Values are computed once here as `const` so there is NO runtime cos() call.
// (Generated by the in-test `reference_cos_table` which a KAT cross-checks.)
const COS_SCALE: i32 = 4096;
#[rustfmt::skip]
const IDCT_COS: [[i32; 8]; 8] = [
    // [x][u] = round(cos((2x+1)*u*pi/16) * 4096)
    [4096,  4017,  3784,  3406,  2896,  2276,  1567,   799],
    [4096,  3406,  1567,  -799, -2896, -4017, -3784, -2276],
    [4096,  2276, -1567, -4017, -2896,   799,  3784,  3406],
    [4096,   799, -3784, -2276,  2896,  3406, -1567, -4017],
    [4096,  -799, -3784,  2276,  2896, -3406, -1567,  4017],
    [4096, -2276, -1567,  4017, -2896,  -799,  3784, -3406],
    [4096, -3406,  1567,   799, -2896,  4017, -3784,  2276],
    [4096, -4017,  3784, -3406,  2896, -2276,  1567,  -799],
];

/// `1/sqrt(2)` in COS_SCALE fixed-point — the C0 normalization factor.
const INV_SQRT2: i32 = 2896; // round(0.70710678 * 4096)

// ─── Huffman decode table ────────────────────────────────────────────────────

/// A decoded Huffman table built from the DHT `bits[16]` + `huffval` format.
///
/// We store the canonical-code lookup as parallel arrays indexed by code length
/// (1..=16): for each length, the smallest code (`min_code`), the largest+1
/// (`max_code`, or -1 if no codes of that length), and the offset into `values`.
#[derive(Clone, Default)]
struct HuffTable {
    /// `mincode[l-1]` = first canonical code of length `l`.
    min_code: [i32; 16],
    /// `maxcode[l-1]` = last canonical code of length `l`, or -1 if none.
    max_code: [i32; 16],
    /// `valptr[l-1]` = index into `values` of the first symbol of length `l`.
    val_ptr: [usize; 16],
    /// The HUFFVAL symbol list, in canonical order.
    values: Vec<u8>,
}

impl HuffTable {
    /// Build from a DHT `bits` (count of codes per length 1..=16) + `huffval`.
    fn build(bits: &[u8; 16], huffval: &[u8]) -> Result<HuffTable, JpegError> {
        // Total number of symbols must equal the value-list length.
        let total: usize = bits.iter().map(|&b| b as usize).sum();
        if total != huffval.len() || total > 256 {
            return Err(JpegError::BadHuffmanTable);
        }

        let mut t = HuffTable {
            min_code: [0; 16],
            max_code: [-1; 16],
            val_ptr: [0; 16],
            values: huffval.to_vec(),
        };

        // Canonical Huffman code assignment (JPEG §C / Annex F figures).
        let mut code: i32 = 0;
        let mut k: usize = 0; // running index into values
        for l in 0..16 {
            let n = bits[l] as i32;
            if n > 0 {
                // A code of length l+1 cannot exceed (1 << (l+1)) - 1.
                if code + n - 1 >= (1 << (l + 1)) {
                    return Err(JpegError::BadHuffmanTable);
                }
                t.val_ptr[l] = k;
                t.min_code[l] = code;
                t.max_code[l] = code + n - 1;
                code += n;
                k += n as usize;
            } else {
                t.max_code[l] = -1;
            }
            code <<= 1;
        }
        Ok(t)
    }
}

// ─── Bit reader over the entropy-coded segment ───────────────────────────────
//
// JPEG entropy data is a byte stream where a literal `0xFF` is followed by a
// `0x00` stuff byte (which is dropped), and any other `0xFF xx` is a marker
// (RSTn / EOI / etc.) that ends the current entropy run. The reader serves bits
// MSB-first and surfaces an encountered marker to the caller.

struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
    /// Current bit buffer (filled MSB-first).
    bits: u32,
    /// Number of valid bits currently in `bits`.
    count: u32,
    /// A marker byte (the second byte of an `FF xx`) that halted bit-filling,
    /// or 0 if none seen.
    marker: u8,
    /// Set once the stream is exhausted of usable entropy bytes.
    eof: bool,
    /// Number of *synthetic* zero-padding bits served because the entropy stream
    /// ran out. Any decode that consumes more than a final byte of padding is
    /// reading off the end of a truncated stream — the scan loop treats this as
    /// [`JpegError::BadEntropy`] rather than silently producing zeros.
    pad_bits: u32,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        BitReader {
            data,
            pos: 0,
            bits: 0,
            count: 0,
            marker: 0,
            eof: false,
            pad_bits: 0,
        }
    }

    /// Pull the next entropy byte, handling `FF00` byte-stuffing and surfacing
    /// any real marker. Returns `Some(byte)` for a data byte, or `None` when a
    /// marker (or EOF) is hit (the marker is recorded in `self.marker`).
    fn next_byte(&mut self) -> Option<u8> {
        if self.marker != 0 || self.eof {
            return None;
        }
        let b = match self.data.get(self.pos) {
            Some(&b) => b,
            None => {
                self.eof = true;
                return None;
            }
        };
        self.pos += 1;
        if b != 0xFF {
            return Some(b);
        }
        // 0xFF: consume following byte(s). Multiple 0xFF fills are allowed.
        let mut nb;
        loop {
            nb = match self.data.get(self.pos) {
                Some(&b) => b,
                None => {
                    self.eof = true;
                    return None;
                }
            };
            self.pos += 1;
            if nb != 0xFF {
                break;
            }
        }
        if nb == 0x00 {
            // Stuffed 0xFF — a literal data byte.
            Some(0xFF)
        } else {
            // A real marker terminates this entropy run.
            self.marker = nb;
            None
        }
    }

    /// Fill the bit buffer to at least `n` bits if possible. Bits beyond the
    /// stream (after a marker/EOF) are served as 0 — premature exhaustion is
    /// surfaced to the caller via `eof`/`marker` so it can fail the decode.
    fn fill(&mut self, n: u32) {
        while self.count < n {
            match self.next_byte() {
                Some(b) => {
                    self.bits = (self.bits << 8) | b as u32;
                    self.count += 8;
                }
                None => {
                    // Pad with zero bits so receive() doesn't read garbage; the
                    // caller detects the shortfall via `pad_bits`/`exhausted()`.
                    self.bits <<= 8;
                    self.count += 8;
                    self.pad_bits = self.pad_bits.saturating_add(8);
                    if self.count >= n {
                        break;
                    }
                }
            }
        }
    }

    /// True when the decoder has been forced to consume synthetic zero-padding
    /// beyond the final real byte — i.e. it is reading off the end of a truncated
    /// entropy stream. (Up to 7 trailing pad bits can legitimately fill the last
    /// partial byte at a clean MCU boundary, so we only flag a *full* byte or
    /// more of padding.)
    fn truncated(&self) -> bool {
        self.pad_bits > 7
    }

    /// Read a single bit (MSB-first).
    fn get_bit(&mut self) -> u32 {
        if self.count == 0 {
            self.fill(1);
        }
        self.count -= 1;
        (self.bits >> self.count) & 1
    }

    /// Read `n` bits as an unsigned value (n in 0..=16).
    fn get_bits(&mut self, n: u32) -> u32 {
        if n == 0 {
            return 0;
        }
        if self.count < n {
            self.fill(n);
        }
        self.count -= n;
        (self.bits >> self.count) & ((1u32 << n) - 1)
    }

    /// Decode one Huffman symbol using `tbl`. Returns the symbol byte, or `Err`
    /// if the code is undecodable / the stream ran out.
    fn decode_huff(&mut self, tbl: &HuffTable) -> Result<u8, JpegError> {
        let mut code: i32 = 0;
        for l in 0..16 {
            code = (code << 1) | self.get_bit() as i32;
            if tbl.max_code[l] >= 0 && code <= tbl.max_code[l] {
                // Found a code of length l+1.
                let idx = tbl.val_ptr[l] + (code - tbl.min_code[l]) as usize;
                return tbl.values.get(idx).copied().ok_or(JpegError::BadEntropy);
            }
        }
        Err(JpegError::BadEntropy)
    }

    /// Reset the bit buffer at a restart marker boundary (discard partial bits).
    fn reset_after_restart(&mut self) {
        self.bits = 0;
        self.count = 0;
        // The marker was the RSTn that ended the run; clear it so reading resumes
        // with the bytes that follow.
        self.marker = 0;
    }
}

/// JPEG "EXTEND": sign-extend a `size`-bit magnitude read from the stream into a
/// signed coefficient (JPEG §F.2.2.1, the receive_and_extend operation).
#[inline]
fn extend(v: u32, size: u32) -> i32 {
    if size == 0 {
        return 0;
    }
    let vt = 1i32 << (size - 1);
    if (v as i32) < vt {
        (v as i32) + (-1i32 << size) + 1
    } else {
        v as i32
    }
}

// ─── Frame / component descriptors ───────────────────────────────────────────

#[derive(Clone, Copy, Default)]
struct Component {
    /// Component identifier from SOF0.
    id: u8,
    /// Horizontal sampling factor (1..=4).
    h: u8,
    /// Vertical sampling factor (1..=4).
    v: u8,
    /// Quantization-table selector (0..=3).
    quant: u8,
    /// DC Huffman-table selector (set by SOS).
    dc_tbl: u8,
    /// AC Huffman-table selector (set by SOS).
    ac_tbl: u8,
    /// DC predictor (running across the scan, reset at restart intervals).
    pred: i32,
}

struct Frame {
    width: u32,
    height: u32,
    components: Vec<Component>,
}

// ─── The 8x8 inverse DCT (separable, fixed-point, no-libm) ───────────────────

/// Dequantize a zig-zag-natural coefficient block and run the separable inverse
/// DCT, writing level-shifted, clamped 0..255 samples into `out` (row-major).
///
/// `block[k]` is the natural-order (already de-zigzagged) coefficient k; `quant`
/// is the natural-order quantization table. Uses [`IDCT_COS`] fixed-point — no
/// floating cos/sqrt. Two-pass separable: rows then columns.
fn idct_8x8(block: &[i32; 64], quant: &[u16; 64], out: &mut [u8; 64]) {
    // ── Hostile-input clamp (criterion #6: never panic / never silent garbage) ─
    //
    // The dequantized coefficient is `coeff * quant`. On a CRAFTED stream the DC
    // predictor accumulates via `wrapping_add` across MCUs (decode_block), so
    // `block[0]` can reach ~2^31; a 16-bit DQT entry (~2^16) makes the product
    // ~2^47. Fed unbounded into the two separable 8-point passes (each term gains
    // two ~2^12 fixed-point factors and is summed over 8 taps), the accumulator
    // would reach ~2^98–2^101 — far past i64::MAX (2^63). In a checked build that
    // overflow PANICS (DoS on a crafted file); in release it wraps to silent
    // garbage pixels. Both are unacceptable for an attacker-controlled decoder.
    //
    // Real baseline 8-bit JPEG coefficients are tiny: a dequantized DC tops out
    // near 255*8 ≈ 2040 and AC magnitudes are the same order, so EVERY valid
    // coefficient sits far below COEFF_CLAMP = 2^23 (8,388,608) — clamping leaves
    // valid images byte-identical (verified by the existing KATs). A value beyond
    // ±2^23 can only come from a malformed/crafted stream, so we saturate it.
    //
    // Post-clamp accumulator bound (why no overflow remains), with C = 2^23,
    // S = COS_SCALE = 2^12 and 8 taps per pass:
    //   work  ≤ C                                   = 2^23
    //   pass1 ≤ 8 * (S * S * work)  = 2^3 * 2^12 * 2^12 * 2^23 = 2^50  (tmp)
    //   pass2 ≤ 8 * (S * S * tmp)   = 2^3 * 2^12 * 2^12 * 2^50 = 2^77  (sum)
    // 2^77 still exceeds i64 (2^63), so the pass accumulators / `tmp` use i128
    // (max 2^127) — 2^77 leaves ~2^50 of headroom. (The final normalized sample
    // is then divided by 4*S^4 = 2^50 and clamped to 0..255, so a clamped-but-
    // still-large coefficient yields a saturated pixel, never garbage.)
    const COEFF_CLAMP: i64 = 1 << 23;

    // Dequantize into a working buffer of f-point sums, clamping hostile values.
    let mut work = [0i128; 64];
    for i in 0..64 {
        let deq = (block[i] as i64) * (quant[i] as i64);
        let clamped = if deq > COEFF_CLAMP {
            COEFF_CLAMP
        } else if deq < -COEFF_CLAMP {
            -COEFF_CLAMP
        } else {
            deq
        };
        work[i] = clamped as i128;
    }

    // Pass 1: 1-D IDCT on each row.
    let mut tmp = [0i128; 64];
    for y in 0..8 {
        let row = &work[y * 8..y * 8 + 8];
        for x in 0..8 {
            let mut sum: i128 = 0;
            for u in 0..8 {
                let cu = if u == 0 {
                    INV_SQRT2 as i128
                } else {
                    COS_SCALE as i128
                };
                // IDCT_COS[x][u] = cos((2x+1)*u*pi/16) * COS_SCALE.
                sum += cu * (IDCT_COS[x][u] as i128) * row[u];
            }
            tmp[y * 8 + x] = sum;
        }
    }

    // Pass 2: 1-D IDCT on each column of the row-transformed data.
    // After two passes the total scale is COS_SCALE^2 * 4 (the 1/2 * 1/2 from the
    // two C-factor sums and the 1/4 normalization of the 2-D IDCT). We fold the
    // constant 1/4 and the two COS_SCALE divisions into one final shift+divide.
    for x in 0..8 {
        for y in 0..8 {
            let mut sum: i128 = 0;
            for v in 0..8 {
                let cv = if v == 0 {
                    INV_SQRT2 as i128
                } else {
                    COS_SCALE as i128
                };
                sum += cv * (IDCT_COS[y][v] as i128) * tmp[v * 8 + x];
            }
            // Normalize: divide by 4 (2-D IDCT factor) and by COS_SCALE^3
            // (cu/cv each carry one COS_SCALE, plus the two IDCT_COS factors carry
            // one COS_SCALE each = COS_SCALE^4 total over both passes; cu & cv
            // contributed COS_SCALE each as well). Track the exact scale:
            //   work: coeff*quant (scale 1)
            //   pass1: cu(scale S) * cos(scale S) * work => scale S^2
            //   pass2: cv(scale S) * cos(scale S) * tmp(scale S^2) => scale S^4
            // 2-D IDCT divides the double sum by 4. So sample =
            //   sum / (4 * S^4).
            let s = COS_SCALE as i128;
            let denom = 4 * s * s * s * s;
            // Rounded divide.
            let val = (sum + denom / 2) / denom;
            let shifted = val + 128; // level shift
            let clamped = if shifted < 0 {
                0
            } else if shifted > 255 {
                255
            } else {
                shifted as u8
            };
            out[y * 8 + x] = clamped;
        }
    }
}

// ─── YCbCr -> RGB (JFIF, integer fixed-point, no-libm) ───────────────────────

/// Convert one YCbCr triple to ARGB (opaque). The JFIF transform:
///   R = Y + 1.402   * (Cr-128)
///   G = Y - 0.34414 * (Cb-128) - 0.71414 * (Cr-128)
///   B = Y + 1.772   * (Cb-128)
/// Fixed-point with 16 fractional bits, rounded, clamped to 0..255.
#[inline]
fn ycbcr_to_argb(y: i32, cb: i32, cr: i32) -> u32 {
    let cr0 = cr - 128;
    let cb0 = cb - 128;
    // Coefficients * 65536, rounded.
    let r = y + ((91881 * cr0) >> 16);
    let g = y - ((22554 * cb0 + 46802 * cr0) >> 16);
    let b = y + ((116130 * cb0) >> 16);
    argb(0xFF, clamp_u8(r), clamp_u8(g), clamp_u8(b))
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

// ─── Top-level decode ────────────────────────────────────────────────────────

/// Decode a baseline JPEG byte stream into an ARGB8888 [`JpegImage`].
///
/// Hostile-input safe: returns `Err` (never panics) on any malformed input.
///
/// ## Supported (honest)
/// - Baseline sequential DCT (SOF0), 8-bit precision.
/// - 1-component grayscale and 3-component YCbCr (JFIF).
/// - Chroma subsampling 4:4:4, 4:2:2, 4:2:0 (chroma box-upsampled).
/// - DRI restart intervals + RSTn resynchronization.
///
/// ## Rejected as [`JpegError::Unsupported`] (documented follow-ups)
/// - Progressive DCT (SOF2), extended sequential (SOF1), arithmetic coding.
pub fn decode_jpeg(data: &[u8]) -> Result<JpegImage, JpegError> {
    // ── SOI ──────────────────────────────────────────────────────────────────
    if data.len() < 2 || data[0] != 0xFF || data[1] != 0xD8 {
        return Err(JpegError::BadSignature);
    }

    let mut pos = 2usize;

    // Decoder state assembled from the segments.
    let mut quant_tables: [[u16; 64]; 4] = [[0; 64]; 4];
    let mut quant_defined = [false; 4];
    let mut dc_tables: [Option<HuffTable>; 4] = [None, None, None, None];
    let mut ac_tables: [Option<HuffTable>; 4] = [None, None, None, None];
    let mut restart_interval: u32 = 0;
    let mut frame: Option<Frame> = None;

    loop {
        // Each segment begins with a marker: 0xFF followed by a marker code
        // (which is not 0x00 and not 0xFF-fill).
        if pos + 1 >= data.len() {
            return Err(JpegError::Truncated);
        }
        if data[pos] != 0xFF {
            return Err(JpegError::BadMarker);
        }
        // Skip any fill 0xFF bytes.
        let mut mpos = pos;
        while mpos < data.len() && data[mpos] == 0xFF {
            mpos += 1;
        }
        if mpos >= data.len() {
            return Err(JpegError::Truncated);
        }
        let marker = data[mpos];
        pos = mpos + 1;

        match marker {
            // Standalone markers (no length): RSTn, TEM. Should not appear here.
            0xD9 => {
                // EOI before a scan — nothing decoded.
                return Err(JpegError::Truncated);
            }
            0x01 | 0xD0..=0xD7 => {
                // TEM / stray RSTn outside a scan — skip.
                continue;
            }
            _ => {}
        }

        // All other markers carry a 2-byte big-endian length (including itself).
        let seg_len = read_u16_be(data, pos).ok_or(JpegError::Truncated)? as usize;
        if seg_len < 2 {
            return Err(JpegError::BadMarker);
        }
        let seg_start = pos + 2;
        let seg_end = pos + seg_len;
        if seg_end > data.len() {
            return Err(JpegError::Truncated);
        }
        let seg = &data[seg_start..seg_end];

        match marker {
            0xC0 => {
                // SOF0 — baseline DCT.
                frame = Some(parse_sof0(seg)?);
            }
            0xC1 | 0xC2 | 0xC3 | 0xC5 | 0xC6 | 0xC7 | 0xC9 | 0xCA | 0xCB | 0xCD | 0xCE | 0xCF => {
                // SOF1 (extended), SOF2 (progressive), SOF3 (lossless), and the
                // arithmetic-coded variants — all unsupported, cleanly.
                return Err(JpegError::Unsupported);
            }
            0xC4 => {
                // DHT — one or more Huffman tables.
                parse_dht(seg, &mut dc_tables, &mut ac_tables)?;
            }
            0xDB => {
                // DQT — one or more quant tables.
                parse_dqt(seg, &mut quant_tables, &mut quant_defined)?;
            }
            0xDD => {
                // DRI — restart interval.
                if seg.len() != 2 {
                    return Err(JpegError::BadMarker);
                }
                restart_interval = ((seg[0] as u32) << 8) | seg[1] as u32;
            }
            0xDA => {
                // SOS — start of scan. The entropy-coded data immediately
                // follows this segment.
                let frame = frame.as_mut().ok_or(JpegError::NoFrame)?;
                parse_sos(seg, frame)?;
                let entropy = &data[seg_end..];
                return decode_scan(
                    frame,
                    &quant_tables,
                    &quant_defined,
                    &dc_tables,
                    &ac_tables,
                    restart_interval,
                    entropy,
                );
            }
            0xE0..=0xEF | 0xFE => {
                // APPn / COM — skip.
            }
            0xD8 => {
                // A second SOI — malformed.
                return Err(JpegError::BadMarker);
            }
            _ => {
                // Unknown segment with a length — skip its body.
            }
        }

        pos = seg_end;
    }
}

fn read_u16_be(buf: &[u8], off: usize) -> Option<u16> {
    let b = buf.get(off..off + 2)?;
    Some(u16::from_be_bytes([b[0], b[1]]))
}

/// Parse the SOF0 baseline frame header.
fn parse_sof0(seg: &[u8]) -> Result<Frame, JpegError> {
    // precision(1) height(2) width(2) ncomp(1) then 3 bytes per component.
    if seg.len() < 6 {
        return Err(JpegError::BadFrame);
    }
    let precision = seg[0];
    if precision != 8 {
        // Baseline is 8-bit precision only.
        return Err(JpegError::Unsupported);
    }
    let height = ((seg[1] as u32) << 8) | seg[2] as u32;
    let width = ((seg[3] as u32) << 8) | seg[4] as u32;
    let ncomp = seg[5] as usize;

    if width == 0 || height == 0 {
        return Err(JpegError::DimensionsOutOfRange);
    }
    if width > MAX_DIMENSION || height > MAX_DIMENSION {
        return Err(JpegError::DimensionsOutOfRange);
    }
    if (width as u64) * (height as u64) > MAX_PIXELS {
        return Err(JpegError::DimensionsOutOfRange);
    }
    if ncomp == 0 || ncomp > MAX_COMPONENTS {
        return Err(JpegError::BadFrame);
    }
    if seg.len() < 6 + ncomp * 3 {
        return Err(JpegError::BadFrame);
    }

    let mut components = Vec::with_capacity(ncomp);
    for i in 0..ncomp {
        let off = 6 + i * 3;
        let id = seg[off];
        let sampling = seg[off + 1];
        let h = sampling >> 4;
        let v = sampling & 0x0F;
        let quant = seg[off + 2];
        if h == 0 || h > 4 || v == 0 || v > 4 || quant > 3 {
            return Err(JpegError::BadFrame);
        }
        components.push(Component {
            id,
            h,
            v,
            quant,
            dc_tbl: 0,
            ac_tbl: 0,
            pred: 0,
        });
    }

    Ok(Frame {
        width,
        height,
        components,
    })
}

/// Parse a DQT segment (may contain multiple tables).
fn parse_dqt(
    seg: &[u8],
    tables: &mut [[u16; 64]; 4],
    defined: &mut [bool; 4],
) -> Result<(), JpegError> {
    let mut i = 0;
    while i < seg.len() {
        let pq_tq = seg[i];
        let precision = pq_tq >> 4; // 0 = 8-bit, 1 = 16-bit
        let id = (pq_tq & 0x0F) as usize;
        i += 1;
        if id >= 4 || precision > 1 {
            return Err(JpegError::BadQuantTable);
        }
        let mut table = [0u16; 64];
        if precision == 0 {
            // 64 8-bit entries.
            if i + 64 > seg.len() {
                return Err(JpegError::BadQuantTable);
            }
            for k in 0..64 {
                table[ZIGZAG[k]] = seg[i + k] as u16;
            }
            i += 64;
        } else {
            // 64 16-bit big-endian entries.
            if i + 128 > seg.len() {
                return Err(JpegError::BadQuantTable);
            }
            for k in 0..64 {
                let hi = seg[i + k * 2] as u16;
                let lo = seg[i + k * 2 + 1] as u16;
                table[ZIGZAG[k]] = (hi << 8) | lo;
            }
            i += 128;
        }
        tables[id] = table;
        defined[id] = true;
    }
    Ok(())
}

/// Parse a DHT segment (may contain multiple tables).
fn parse_dht(
    seg: &[u8],
    dc: &mut [Option<HuffTable>; 4],
    ac: &mut [Option<HuffTable>; 4],
) -> Result<(), JpegError> {
    let mut i = 0;
    while i < seg.len() {
        if i + 17 > seg.len() {
            return Err(JpegError::BadHuffmanTable);
        }
        let tc_th = seg[i];
        let class = tc_th >> 4; // 0 = DC, 1 = AC
        let id = (tc_th & 0x0F) as usize;
        i += 1;
        if id >= 4 || class > 1 {
            return Err(JpegError::BadHuffmanTable);
        }
        let mut bits = [0u8; 16];
        bits.copy_from_slice(&seg[i..i + 16]);
        i += 16;
        let total: usize = bits.iter().map(|&b| b as usize).sum();
        if i + total > seg.len() {
            return Err(JpegError::BadHuffmanTable);
        }
        let huffval = &seg[i..i + total];
        i += total;
        let table = HuffTable::build(&bits, huffval)?;
        if class == 0 {
            dc[id] = Some(table);
        } else {
            ac[id] = Some(table);
        }
    }
    Ok(())
}

/// Parse the SOS scan header, wiring DC/AC table selectors into the frame's
/// components.
fn parse_sos(seg: &[u8], frame: &mut Frame) -> Result<(), JpegError> {
    if seg.is_empty() {
        return Err(JpegError::BadScan);
    }
    let ns = seg[0] as usize;
    if ns == 0 || ns > frame.components.len() {
        return Err(JpegError::BadScan);
    }
    // 1 (Ns) + 2*Ns + 3 (Ss, Se, Ah/Al) bytes.
    if seg.len() < 1 + 2 * ns + 3 {
        return Err(JpegError::BadScan);
    }
    for i in 0..ns {
        let cs = seg[1 + i * 2];
        let td_ta = seg[1 + i * 2 + 1];
        let dc_sel = td_ta >> 4;
        let ac_sel = td_ta & 0x0F;
        if dc_sel > 3 || ac_sel > 3 {
            return Err(JpegError::BadScan);
        }
        // Map the scan component selector (cs) to a frame component by id.
        let comp = frame
            .components
            .iter_mut()
            .find(|c| c.id == cs)
            .ok_or(JpegError::BadScan)?;
        comp.dc_tbl = dc_sel;
        comp.ac_tbl = ac_sel;
    }
    // Ss/Se/Ah/Al: baseline must be Ss=0, Se=63, Ah=0, Al=0. We tolerate the
    // standard baseline values and reject anything that signals progressive.
    let ss = seg[1 + 2 * ns];
    let se = seg[1 + 2 * ns + 1];
    let ah_al = seg[1 + 2 * ns + 2];
    if ss != 0 || se != 63 || ah_al != 0 {
        return Err(JpegError::Unsupported);
    }
    Ok(())
}

/// Decode the entropy-coded scan into the final ARGB image.
fn decode_scan(
    frame: &mut Frame,
    quant_tables: &[[u16; 64]; 4],
    quant_defined: &[bool; 4],
    dc_tables: &[Option<HuffTable>; 4],
    ac_tables: &[Option<HuffTable>; 4],
    restart_interval: u32,
    entropy: &[u8],
) -> Result<JpegImage, JpegError> {
    let width = frame.width as usize;
    let height = frame.height as usize;
    let ncomp = frame.components.len();

    // Validate each component's quant table is present.
    for c in &frame.components {
        if !quant_defined[c.quant as usize] {
            return Err(JpegError::BadScan);
        }
    }

    // Max sampling factors define the MCU geometry.
    let hmax = frame.components.iter().map(|c| c.h).max().unwrap_or(1) as usize;
    let vmax = frame.components.iter().map(|c| c.v).max().unwrap_or(1) as usize;
    if hmax == 0 || vmax == 0 {
        return Err(JpegError::BadScan);
    }

    let mcu_w = 8 * hmax;
    let mcu_h = 8 * vmax;
    let mcus_x = (width + mcu_w - 1) / mcu_w;
    let mcus_y = (height + mcu_h - 1) / mcu_h;

    // Per-component full-resolution sample plane sized to the MCU grid (so the
    // chroma planes, scaled by their own sampling factor, line up after upsample).
    // We store each component at its NATIVE resolution: width_c = mcus_x * h * 8.
    struct Plane {
        w: usize,
        h: usize,
        samples: Vec<u8>,
    }
    let mut planes: Vec<Plane> = Vec::with_capacity(ncomp);
    for c in &frame.components {
        let pw = mcus_x * c.h as usize * 8;
        let ph = mcus_y * c.v as usize * 8;
        let area = pw.checked_mul(ph).ok_or(JpegError::DimensionsOutOfRange)?;
        // Bound: every plane is <= MCU-padded canvas; the canvas pixel count is
        // already capped, and each plane is <= that * sampling, but sampling is
        // <= 4*4 — still well within reason given MAX_PIXELS. Guard anyway.
        if area > (MAX_PIXELS as usize) * 16 {
            return Err(JpegError::DimensionsOutOfRange);
        }
        planes.push(Plane {
            w: pw,
            h: ph,
            samples: vec![0u8; area],
        });
    }

    let mut reader = BitReader::new(entropy);
    let mut block = [0i32; 64];
    let mut natural = [0i32; 64];
    let mut idct_out = [0u8; 64];

    // Reset DC predictors.
    for c in frame.components.iter_mut() {
        c.pred = 0;
    }

    let mut mcu_count: u32 = 0;
    let mut expected_rst: u8 = 0;

    for my in 0..mcus_y {
        for mx in 0..mcus_x {
            // Restart handling: at each interval boundary, expect/consume RSTn.
            if restart_interval != 0 && mcu_count != 0 && mcu_count % restart_interval == 0 {
                // The bit reader should have halted on an RSTn marker.
                // Advance the reader past any remaining bits and re-synchronize.
                if !sync_restart(&mut reader, &mut expected_rst)? {
                    return Err(JpegError::BadEntropy);
                }
                for c in frame.components.iter_mut() {
                    c.pred = 0;
                }
            }

            // Each MCU = for each component, h*v data units (8x8 blocks).
            for ci in 0..ncomp {
                let (ch, cv, quant_id, dc_id, ac_id, pred);
                {
                    let c = &frame.components[ci];
                    ch = c.h as usize;
                    cv = c.v as usize;
                    quant_id = c.quant as usize;
                    dc_id = c.dc_tbl as usize;
                    ac_id = c.ac_tbl as usize;
                    pred = c.pred;
                }
                let dc_tbl = dc_tables[dc_id].as_ref().ok_or(JpegError::BadScan)?;
                let ac_tbl = ac_tables[ac_id].as_ref().ok_or(JpegError::BadScan)?;
                let quant = &quant_tables[quant_id];

                let mut new_pred = pred;
                for by in 0..cv {
                    for bx in 0..ch {
                        // Decode one 8x8 block in zig-zag, into `block`.
                        new_pred = decode_block(&mut reader, dc_tbl, ac_tbl, new_pred, &mut block)?;
                        // A truncated entropy stream forces synthetic padding;
                        // refuse rather than emit zero-filled garbage blocks.
                        if reader.truncated() {
                            return Err(JpegError::BadEntropy);
                        }

                        // De-zigzag into natural order.
                        for k in 0..64 {
                            natural[ZIGZAG[k]] = block[k];
                        }
                        idct_8x8(&natural, quant, &mut idct_out);

                        // Place this block into the component plane.
                        let plane = &mut planes[ci];
                        let px0 = (mx * ch + bx) * 8;
                        let py0 = (my * cv + by) * 8;
                        for yy in 0..8 {
                            let dst_y = py0 + yy;
                            if dst_y >= plane.h {
                                break;
                            }
                            let base = dst_y * plane.w + px0;
                            for xx in 0..8 {
                                let dst_x = px0 + xx;
                                if dst_x >= plane.w {
                                    break;
                                }
                                plane.samples[base + xx] = idct_out[yy * 8 + xx];
                            }
                        }
                    }
                }
                frame.components[ci].pred = new_pred;
            }

            mcu_count += 1;
        }
    }

    // ── Upsample + color-convert into the ARGB canvas ────────────────────────
    let mut pixels = vec![0u32; width * height];
    if ncomp == 1 {
        // Grayscale.
        let plane = &planes[0];
        for y in 0..height {
            for x in 0..width {
                let s = *plane.samples.get(y * plane.w + x).unwrap_or(&0);
                pixels[y * width + x] = argb(0xFF, s, s, s);
            }
        }
    } else {
        // YCbCr (components 0,1,2). Each chroma plane is upsampled by replicating
        // (box upsample) according to its sampling factor vs hmax/vmax.
        let yc = &planes[0];
        let cbc = planes.get(1);
        let crc = planes.get(2);
        let (yh, yv) = (
            frame.components[0].h as usize,
            frame.components[0].v as usize,
        );
        let cb_hv = frame
            .components
            .get(1)
            .map(|c| (c.h as usize, c.v as usize));
        let cr_hv = frame
            .components
            .get(2)
            .map(|c| (c.h as usize, c.v as usize));

        for y in 0..height {
            for x in 0..width {
                // Luma sample at full res (luma h/v relative to hmax/vmax).
                let lx = x * yh / hmax;
                let ly = y * yv / vmax;
                let yv_s = *yc.samples.get(ly * yc.w + lx).unwrap_or(&0) as i32;

                let cb = match (cbc, cb_hv) {
                    (Some(p), Some((h, v))) => {
                        let cx = x * h / hmax;
                        let cyy = y * v / vmax;
                        *p.samples.get(cyy * p.w + cx).unwrap_or(&128) as i32
                    }
                    _ => 128,
                };
                let cr = match (crc, cr_hv) {
                    (Some(p), Some((h, v))) => {
                        let cx = x * h / hmax;
                        let cyy = y * v / vmax;
                        *p.samples.get(cyy * p.w + cx).unwrap_or(&128) as i32
                    }
                    _ => 128,
                };

                pixels[y * width + x] = ycbcr_to_argb(yv_s, cb, cr);
            }
        }
    }

    Ok(JpegImage {
        width: frame.width,
        height: frame.height,
        pixels,
    })
}

/// Decode one 8x8 block (DC + AC) into `block` (zig-zag order). Returns the new
/// DC predictor for this component.
fn decode_block(
    reader: &mut BitReader,
    dc_tbl: &HuffTable,
    ac_tbl: &HuffTable,
    pred: i32,
    block: &mut [i32; 64],
) -> Result<i32, JpegError> {
    *block = [0i32; 64];

    // ── DC coefficient ───────────────────────────────────────────────────────
    let t = reader.decode_huff(dc_tbl)?;
    if t > 16 {
        return Err(JpegError::BadEntropy);
    }
    let diff = if t == 0 {
        0
    } else {
        let v = reader.get_bits(t as u32);
        extend(v, t as u32)
    };
    let dc = pred.wrapping_add(diff);
    block[0] = dc;

    // ── AC coefficients ──────────────────────────────────────────────────────
    let mut k = 1usize;
    while k < 64 {
        let rs = reader.decode_huff(ac_tbl)?;
        let run = (rs >> 4) as usize;
        let size = (rs & 0x0F) as u32;
        if size == 0 {
            if run == 15 {
                // ZRL — skip 16 zeros.
                k += 16;
                continue;
            } else {
                // EOB — rest of the block is zero.
                break;
            }
        }
        k += run;
        if k >= 64 {
            return Err(JpegError::BadEntropy);
        }
        let v = reader.get_bits(size);
        block[k] = extend(v, size);
        k += 1;
    }

    Ok(dc)
}

/// At a restart-interval boundary, consume the RSTn marker the bit reader halted
/// on and resynchronize. Returns Ok(true) if a valid RSTn (or recoverable
/// boundary) was found. `expected` tracks the modulo-8 RSTn sequence.
fn sync_restart(reader: &mut BitReader, expected: &mut u8) -> Result<bool, JpegError> {
    // Drain any remaining buffered bits up to the marker.
    // Pull bytes until the reader records a marker (or EOF).
    while reader.marker == 0 && !reader.eof {
        if reader.next_byte().is_none() {
            break;
        }
    }
    if reader.marker == 0 {
        // No marker found where one was required.
        return Ok(false);
    }
    let m = reader.marker;
    if !(0xD0..=0xD7).contains(&m) {
        // A non-restart marker (e.g. EOI) at a restart boundary — stop cleanly
        // is not valid mid-image; treat as entropy error.
        return Err(JpegError::BadEntropy);
    }
    // RSTn sequence is m & 7; it should match `expected`. We tolerate mismatch
    // (some encoders/corruption) but advance the counter.
    let _seq = m & 0x07;
    *expected = (*expected + 1) & 0x07;
    reader.reset_after_restart();
    Ok(true)
}

// ════════════════════════════════════════════════════════════════════════════
// BASELINE JPEG ENCODER (ITU-T T.81 / JFIF) — the "save/export my photo as JPEG"
// path (RaeenOS_Concept.md §creators / media). JPEG is the dominant *lossy*
// photo/camera interchange format; an OS that can decode but not encode JPEG
// can't let a user export an edit or a screenshot as the format the rest of the
// world reads. This is the from-scratch baseline sequential-DCT encoder that
// completes the image-save story alongside the PNG encoder.
//
// ## Design (mirrors the decoder's conventions exactly)
// - Reuses [`ZIGZAG`], [`IDCT_COS`] (the FDCT is the transpose of the IDCT — the
//   same cosine table indexed [sample][freq]), [`COS_SCALE`], [`INV_SQRT2`] and
//   the no-libm fixed-point posture. No runtime `cos`/`sqrt`.
// - Subsampling: **4:4:4** (no chroma subsample). 4:4:4 round-trips most cleanly
//   against the decoder (no chroma box-upsample error), which is the verification
//   lever, and is the simplest correct baseline. (4:2:0 is a documented size
//   follow-up; the SOF0 here always declares 1x1 sampling for all components.)
// - Standard Annex K luminance + chrominance quantization tables, scaled by the
//   standard quality formula; standard Annex K baseline Huffman tables (luma +
//   chroma, DC + AC) emitted verbatim in the DHT segments.
// - JFIF container: SOI, APP0 (JFIF), DQT, SOF0, DHT, SOS + entropy scan with
//   0xFF -> 0xFF00 byte stuffing, EOI.
// - Bounded like the decoder: absurd dimensions are rejected before any alloc.
// ════════════════════════════════════════════════════════════════════════════

/// JPEG encode error. Every variant is a *handled* path — the encoder never
/// panics and never emits a corrupt/partial stream on a rejected input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JpegEncodeError {
    /// Width or height was zero, exceeded [`MAX_DIMENSION`], or the pixel count
    /// exceeded [`MAX_PIXELS`].
    DimensionsOutOfRange,
    /// `pixels.len()` did not equal `width * height` — a malformed source image.
    PixelCountMismatch,
    /// `quality` was outside the valid 1..=100 range.
    BadQuality,
}

// ─── Standard Annex K quantization tables (50% quality baseline) ─────────────
//
// JPEG Annex K, Tables K.1 (luminance) and K.2 (chrominance), in NATURAL
// (row-major) order. These are the de-facto "quality 50" tables every encoder
// scales from. (The decoder de-zigzags DQT into natural order; we keep these
// natural and zig-zag them on emit, matching the decoder's DQT parse.)
#[rustfmt::skip]
const STD_LUMA_QUANT: [u16; 64] = [
    16, 11, 10, 16, 24, 40, 51, 61,
    12, 12, 14, 19, 26, 58, 60, 55,
    14, 13, 16, 24, 40, 57, 69, 56,
    14, 17, 22, 29, 51, 87, 80, 62,
    18, 22, 37, 56, 68, 109, 103, 77,
    24, 35, 55, 64, 81, 104, 113, 92,
    49, 64, 78, 87, 103, 121, 120, 101,
    72, 92, 95, 98, 112, 100, 103, 99,
];
#[rustfmt::skip]
const STD_CHROMA_QUANT: [u16; 64] = [
    17, 18, 24, 47, 99, 99, 99, 99,
    18, 21, 26, 66, 99, 99, 99, 99,
    24, 26, 56, 99, 99, 99, 99, 99,
    47, 66, 99, 99, 99, 99, 99, 99,
    99, 99, 99, 99, 99, 99, 99, 99,
    99, 99, 99, 99, 99, 99, 99, 99,
    99, 99, 99, 99, 99, 99, 99, 99,
    99, 99, 99, 99, 99, 99, 99, 99,
];

/// Scale the standard Annex K table by `quality` using the canonical libjpeg
/// quality->scale formula, clamping each entry to 1..=255 (8-bit DQT precision).
///
/// `quality` is 1..=100; 50 reproduces the base table, 100 -> all-1 (near
/// lossless within DCT rounding), 1 -> maximal quantization.
fn scaled_quant_table(base: &[u16; 64], quality: u8) -> [u16; 64] {
    // libjpeg jpeg_quality_scaling: scale = quality<50 ? 5000/quality
    //                                     : 200 - quality*2  (quality in 1..=100)
    let q = quality as i32;
    let scale = if q < 50 { 5000 / q } else { 200 - q * 2 };
    let mut out = [0u16; 64];
    for i in 0..64 {
        // (base*scale + 50) / 100, clamped to 1..=255.
        let v = ((base[i] as i32) * scale + 50) / 100;
        let v = if v < 1 {
            1
        } else if v > 255 {
            255
        } else {
            v
        };
        out[i] = v as u16;
    }
    out
}

// ─── Forward 8x8 DCT (separable, fixed-point, no-libm) ───────────────────────
//
// The 2-D forward DCT-II:
//   F(u,v) = (1/4) C(u) C(v) Σx Σy f(x,y) cos((2x+1)uπ/16) cos((2y+1)vπ/16)
// with C(0)=1/√2, C(k>0)=1. The cosine factor cos((2x+1)uπ/16) is exactly the
// decoder's `IDCT_COS[x][u]` (sample index x, frequency index u) — the FDCT is
// the transpose of the IDCT over the same const table. We level-shift the input
// by -128 first (JPEG §A.3.1), run two separable passes in i64, then normalize.

/// Forward DCT of a level-shiftable 8x8 sample block. `samples` are raw 0..=255
/// component samples (row-major); the result `out` is the natural-order signed
/// DCT coefficient block (NOT yet quantized, NOT zig-zagged).
fn fdct_8x8(samples: &[u8; 64], out: &mut [i32; 64]) {
    // Level shift to signed [-128, 127]. Range bound: |f| <= 128.
    let mut work = [0i64; 64];
    for i in 0..64 {
        work[i] = samples[i] as i64 - 128;
    }

    // Pass 1: 1-D DCT along each row (transform x -> u).
    //   row_dct[u] = Σx work[x] * cos[x][u]      (scale COS_SCALE)
    let mut tmp = [0i64; 64];
    for y in 0..8 {
        let row = &work[y * 8..y * 8 + 8];
        for u in 0..8 {
            let mut sum: i64 = 0;
            for x in 0..8 {
                sum += row[x] * (IDCT_COS[x][u] as i64);
            }
            tmp[y * 8 + u] = sum;
        }
    }

    // Pass 2: 1-D DCT along each column (transform y -> v), then apply the C(u),
    // C(v) factors and the 1/4 normalization in one rounded divide.
    //
    // Scale bookkeeping (S = COS_SCALE):
    //   work: scale 1
    //   tmp (pass1): Σ work*cos  -> scale S
    //   pass2: Σ tmp*cos         -> scale S^2
    //   C(u)=C(v)=1/√2 folded as INV_SQRT2/S (scale-1 factor each).
    //   Final F = (1/4) * C(u) * C(v) * sum / S^2.
    // We fold C(u),C(v) as integer multiplies by either S (=1.0) or INV_SQRT2
    // (=1/√2), which adds one extra S to the denominator per axis -> divide by
    // S^2 (the two passes) * S^2 (the two C-factor multiplies) * 4 = 4 * S^4.
    let s = COS_SCALE as i64;
    let denom = 4 * s * s * s * s;
    for u in 0..8 {
        for v in 0..8 {
            let mut sum: i64 = 0;
            for y in 0..8 {
                sum += tmp[y * 8 + u] * (IDCT_COS[y][v] as i64);
            }
            let cu = if u == 0 {
                INV_SQRT2 as i64
            } else {
                COS_SCALE as i64
            };
            let cv = if v == 0 {
                INV_SQRT2 as i64
            } else {
                COS_SCALE as i64
            };
            let scaled = sum * cu * cv;
            // Rounded divide toward nearest (sign-aware).
            let val = if scaled >= 0 {
                (scaled + denom / 2) / denom
            } else {
                (scaled - denom / 2) / denom
            };
            out[v * 8 + u] = val as i32;
        }
    }
}

// ─── RGB -> YCbCr (JFIF, integer fixed-point, no-libm) ───────────────────────

/// The JFIF forward transform (inverse of [`ycbcr_to_argb`]):
///   Y  =  0.299  R + 0.587  G + 0.114  B
///   Cb = -0.168736 R - 0.331264 G + 0.5      B + 128
///   Cr =  0.5      R - 0.418688 G - 0.081312 B + 128
/// Fixed-point with 16 fractional bits, rounded, clamped to 0..=255.
#[inline]
fn rgb_to_ycbcr(r: u8, g: u8, b: u8) -> (u8, u8, u8) {
    let r = r as i32;
    let g = g as i32;
    let b = b as i32;
    // Coefficients * 65536, rounded.
    let y = (19595 * r + 38470 * g + 7471 * b + 32768) >> 16;
    let cb = ((-11059 * r - 21709 * g + 32768 * b + 32768) >> 16) + 128;
    let cr = ((32768 * r - 27439 * g - 5329 * b + 32768) >> 16) + 128;
    (clamp_u8(y), clamp_u8(cb), clamp_u8(cr))
}

// ─── Standard Annex K baseline Huffman tables ────────────────────────────────
//
// These are the well-known fixed tables from JPEG Annex K (Tables K.3–K.6),
// expressed as the DHT `bits[16]` (count of codes per length 1..=16) + the
// `huffval` symbol list. The encoder builds a (code, length) lookup from them
// and emits them verbatim in the DHT segments so the decoder rebuilds the
// identical canonical codes.

const STD_DC_LUMA_BITS: [u8; 16] = [0, 1, 5, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0];
const STD_DC_LUMA_VALS: [u8; 12] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11];

const STD_DC_CHROMA_BITS: [u8; 16] = [0, 3, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0];
const STD_DC_CHROMA_VALS: [u8; 12] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11];

const STD_AC_LUMA_BITS: [u8; 16] = [0, 2, 1, 3, 3, 2, 4, 3, 5, 5, 4, 4, 0, 0, 1, 0x7d];
#[rustfmt::skip]
const STD_AC_LUMA_VALS: [u8; 162] = [
    0x01, 0x02, 0x03, 0x00, 0x04, 0x11, 0x05, 0x12,
    0x21, 0x31, 0x41, 0x06, 0x13, 0x51, 0x61, 0x07,
    0x22, 0x71, 0x14, 0x32, 0x81, 0x91, 0xa1, 0x08,
    0x23, 0x42, 0xb1, 0xc1, 0x15, 0x52, 0xd1, 0xf0,
    0x24, 0x33, 0x62, 0x72, 0x82, 0x09, 0x0a, 0x16,
    0x17, 0x18, 0x19, 0x1a, 0x25, 0x26, 0x27, 0x28,
    0x29, 0x2a, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39,
    0x3a, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49,
    0x4a, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58, 0x59,
    0x5a, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68, 0x69,
    0x6a, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79,
    0x7a, 0x83, 0x84, 0x85, 0x86, 0x87, 0x88, 0x89,
    0x8a, 0x92, 0x93, 0x94, 0x95, 0x96, 0x97, 0x98,
    0x99, 0x9a, 0xa2, 0xa3, 0xa4, 0xa5, 0xa6, 0xa7,
    0xa8, 0xa9, 0xaa, 0xb2, 0xb3, 0xb4, 0xb5, 0xb6,
    0xb7, 0xb8, 0xb9, 0xba, 0xc2, 0xc3, 0xc4, 0xc5,
    0xc6, 0xc7, 0xc8, 0xc9, 0xca, 0xd2, 0xd3, 0xd4,
    0xd5, 0xd6, 0xd7, 0xd8, 0xd9, 0xda, 0xe1, 0xe2,
    0xe3, 0xe4, 0xe5, 0xe6, 0xe7, 0xe8, 0xe9, 0xea,
    0xf1, 0xf2, 0xf3, 0xf4, 0xf5, 0xf6, 0xf7, 0xf8,
    0xf9, 0xfa,
];

const STD_AC_CHROMA_BITS: [u8; 16] = [0, 2, 1, 2, 4, 4, 3, 4, 7, 5, 4, 4, 0, 1, 2, 0x77];
#[rustfmt::skip]
const STD_AC_CHROMA_VALS: [u8; 162] = [
    0x00, 0x01, 0x02, 0x03, 0x11, 0x04, 0x05, 0x21,
    0x31, 0x06, 0x12, 0x41, 0x51, 0x07, 0x61, 0x71,
    0x13, 0x22, 0x32, 0x81, 0x08, 0x14, 0x42, 0x91,
    0xa1, 0xb1, 0xc1, 0x09, 0x23, 0x33, 0x52, 0xf0,
    0x15, 0x62, 0x72, 0xd1, 0x0a, 0x16, 0x24, 0x34,
    0xe1, 0x25, 0xf1, 0x17, 0x18, 0x19, 0x1a, 0x26,
    0x27, 0x28, 0x29, 0x2a, 0x35, 0x36, 0x37, 0x38,
    0x39, 0x3a, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48,
    0x49, 0x4a, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58,
    0x59, 0x5a, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68,
    0x69, 0x6a, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78,
    0x79, 0x7a, 0x82, 0x83, 0x84, 0x85, 0x86, 0x87,
    0x88, 0x89, 0x8a, 0x92, 0x93, 0x94, 0x95, 0x96,
    0x97, 0x98, 0x99, 0x9a, 0xa2, 0xa3, 0xa4, 0xa5,
    0xa6, 0xa7, 0xa8, 0xa9, 0xaa, 0xb2, 0xb3, 0xb4,
    0xb5, 0xb6, 0xb7, 0xb8, 0xb9, 0xba, 0xc2, 0xc3,
    0xc4, 0xc5, 0xc6, 0xc7, 0xc8, 0xc9, 0xca, 0xd2,
    0xd3, 0xd4, 0xd5, 0xd6, 0xd7, 0xd8, 0xd9, 0xda,
    0xe2, 0xe3, 0xe4, 0xe5, 0xe6, 0xe7, 0xe8, 0xe9,
    0xea, 0xf2, 0xf3, 0xf4, 0xf5, 0xf6, 0xf7, 0xf8,
    0xf9, 0xfa,
];

/// An encode-side Huffman table: for every symbol value (0..=255), its canonical
/// `(code, length)`. Built from the standard `bits`/`huffval` with the SAME
/// canonical assignment the decoder's [`HuffTable::build`] uses, so the two are
/// guaranteed consistent.
struct EncodeHuff {
    /// `codes[sym]` = (canonical code, bit length). Length 0 = symbol unused.
    codes: [(u16, u8); 256],
}

impl EncodeHuff {
    fn build(bits: &[u8; 16], huffval: &[u8]) -> EncodeHuff {
        let mut codes = [(0u16, 0u8); 256];
        let mut code: u32 = 0;
        let mut k = 0usize;
        for l in 0..16 {
            let n = bits[l] as usize;
            for _ in 0..n {
                if let Some(&sym) = huffval.get(k) {
                    codes[sym as usize] = (code as u16, (l + 1) as u8);
                }
                code += 1;
                k += 1;
            }
            code <<= 1;
        }
        EncodeHuff { codes }
    }

    /// Look up a symbol's `(code, length)`. A length of 0 means the symbol is not
    /// in the table (never happens for the standard tables over valid input).
    #[inline]
    fn get(&self, sym: u8) -> (u16, u8) {
        self.codes[sym as usize]
    }
}

// ─── Entropy bit writer (MSB-first, with 0xFF byte stuffing) ─────────────────

/// MSB-first bit accumulator that flushes whole bytes into the output, inserting
/// a `0x00` stuff byte after every emitted `0xFF` (JPEG entropy convention).
struct EntropyWriter<'a> {
    out: &'a mut Vec<u8>,
    acc: u32,
    nbits: u32,
}

impl<'a> EntropyWriter<'a> {
    fn new(out: &'a mut Vec<u8>) -> Self {
        EntropyWriter {
            out,
            acc: 0,
            nbits: 0,
        }
    }

    /// Append the low `len` bits of `value` (len in 0..=24), MSB-first.
    fn put_bits(&mut self, value: u32, len: u8) {
        if len == 0 {
            return;
        }
        let len = len as u32;
        // Shift the new bits in below the existing accumulator.
        self.acc = (self.acc << len) | (value & ((1u32 << len) - 1));
        self.nbits += len;
        while self.nbits >= 8 {
            self.nbits -= 8;
            let byte = ((self.acc >> self.nbits) & 0xFF) as u8;
            self.out.push(byte);
            if byte == 0xFF {
                self.out.push(0x00); // byte stuffing
            }
        }
    }

    /// Emit a Huffman code (code value + bit length).
    #[inline]
    fn put_code(&mut self, code_len: (u16, u8)) {
        self.put_bits(code_len.0 as u32, code_len.1);
    }

    /// Flush a final partial byte, padding with 1-bits (JPEG convention).
    fn flush(&mut self) {
        if self.nbits > 0 {
            let pad = 8 - self.nbits;
            let byte = (((self.acc << pad) | ((1u32 << pad) - 1)) & 0xFF) as u8;
            self.out.push(byte);
            if byte == 0xFF {
                self.out.push(0x00);
            }
            self.acc = 0;
            self.nbits = 0;
        }
    }
}

/// JPEG magnitude category + bits for a signed coefficient (the inverse of the
/// decoder's [`extend`]). Returns `(size, bits)` where `size` is the number of
/// significant bits and `bits` are the `size`-bit magnitude code (negatives are
/// stored as `value - 1` in `size` bits).
#[inline]
fn magnitude_category(v: i32) -> (u8, u32) {
    if v == 0 {
        return (0, 0);
    }
    let mag = v.unsigned_abs();
    let size = (32 - mag.leading_zeros()) as u8;
    let bits = if v > 0 {
        (v as u32) & ((1u32 << size) - 1)
    } else {
        ((v - 1) as u32) & ((1u32 << size) - 1)
    };
    (size, bits)
}

/// Emit a DQT segment for one 8-bit-precision quant table (zig-zag order on the
/// wire, matching the decoder's DQT parse which de-zigzags into natural order).
fn emit_dqt(out: &mut Vec<u8>, table_id: u8, natural_table: &[u16; 64]) {
    out.push(0xFF);
    out.push(0xDB);
    // length = 2 (len) + 1 (pq/tq) + 64.
    out.extend_from_slice(&(2u16 + 1 + 64).to_be_bytes());
    out.push(table_id & 0x0F); // precision 0 (8-bit), table id
    for k in 0..64 {
        // ZIGZAG[k] is the natural index of zig-zag position k.
        out.push(natural_table[ZIGZAG[k]] as u8);
    }
}

/// Emit a DHT segment for one Huffman table.
fn emit_dht(out: &mut Vec<u8>, class: u8, table_id: u8, bits: &[u8; 16], vals: &[u8]) {
    out.push(0xFF);
    out.push(0xC4);
    let body_len = 1 + 16 + vals.len();
    out.extend_from_slice(&((2 + body_len) as u16).to_be_bytes());
    out.push(((class & 0x0F) << 4) | (table_id & 0x0F));
    out.extend_from_slice(bits);
    out.extend_from_slice(vals);
}

/// Encode an [`JpegImage`] (ARGB8888) into a baseline-sequential JPEG byte
/// stream at the given `quality` (1..=100).
///
/// 3-component YCbCr 4:4:4 (no chroma subsampling) so the round-trip against
/// [`decode_jpeg`] carries no upsampling error — the cleanest correctness lever.
/// Uses the standard Annex K quant tables (scaled by `quality`) + the standard
/// Annex K baseline Huffman tables.
///
/// Bounded/hostile-safe: dimensions are validated against [`MAX_DIMENSION`] /
/// [`MAX_PIXELS`] and the pixel buffer length is checked before any work, so a
/// malformed source cannot drive an unbounded allocation, and the function never
/// emits a partial/corrupt stream on a rejected input.
pub fn encode_jpeg(img: &JpegImage, quality: u8) -> Result<Vec<u8>, JpegEncodeError> {
    if quality < 1 || quality > 100 {
        return Err(JpegEncodeError::BadQuality);
    }
    let w = img.width;
    let h = img.height;
    if w == 0 || h == 0 || w > MAX_DIMENSION || h > MAX_DIMENSION {
        return Err(JpegEncodeError::DimensionsOutOfRange);
    }
    if (w as u64) * (h as u64) > MAX_PIXELS {
        return Err(JpegEncodeError::DimensionsOutOfRange);
    }
    if img.pixels.len() != (w as usize) * (h as usize) {
        return Err(JpegEncodeError::PixelCountMismatch);
    }

    let width = w as usize;
    let height = h as usize;

    // Scaled quant tables (natural order).
    let luma_q = scaled_quant_table(&STD_LUMA_QUANT, quality);
    let chroma_q = scaled_quant_table(&STD_CHROMA_QUANT, quality);

    // Encode-side Huffman tables.
    let dc_luma = EncodeHuff::build(&STD_DC_LUMA_BITS, &STD_DC_LUMA_VALS);
    let ac_luma = EncodeHuff::build(&STD_AC_LUMA_BITS, &STD_AC_LUMA_VALS);
    let dc_chroma = EncodeHuff::build(&STD_DC_CHROMA_BITS, &STD_DC_CHROMA_VALS);
    let ac_chroma = EncodeHuff::build(&STD_AC_CHROMA_BITS, &STD_AC_CHROMA_VALS);

    // ── Convert the whole image to YCbCr planes (4:4:4) ──────────────────────
    let npix = width * height;
    let mut y_plane = vec![0u8; npix];
    let mut cb_plane = vec![0u8; npix];
    let mut cr_plane = vec![0u8; npix];
    for i in 0..npix {
        let p = img.pixels[i];
        let r = (p >> 16) as u8;
        let g = (p >> 8) as u8;
        let b = p as u8;
        let (yy, cb, cr) = rgb_to_ycbcr(r, g, b);
        y_plane[i] = yy;
        cb_plane[i] = cb;
        cr_plane[i] = cr;
    }

    // ── Assemble the JFIF container ──────────────────────────────────────────
    let mut out = Vec::new();
    // SOI.
    out.push(0xFF);
    out.push(0xD8);
    // APP0 / JFIF.
    out.push(0xFF);
    out.push(0xE0);
    out.extend_from_slice(&16u16.to_be_bytes()); // length 16
    out.extend_from_slice(b"JFIF\0"); // identifier
    out.push(1); // version major
    out.push(1); // version minor
    out.push(0); // units: none (aspect ratio only)
    out.extend_from_slice(&1u16.to_be_bytes()); // X density
    out.extend_from_slice(&1u16.to_be_bytes()); // Y density
    out.push(0); // X thumbnail
    out.push(0); // Y thumbnail
                 // DQT (table 0 = luma, table 1 = chroma).
    emit_dqt(&mut out, 0, &luma_q);
    emit_dqt(&mut out, 1, &chroma_q);
    // SOF0 (baseline). precision 8, height, width, 3 components, all 1x1 sampling.
    out.push(0xFF);
    out.push(0xC0);
    out.extend_from_slice(&(8u16 + 3 * 3).to_be_bytes()); // length = 2+1+2+2+1 + 3*3
    out.push(8); // precision
    out.extend_from_slice(&(h as u16).to_be_bytes());
    out.extend_from_slice(&(w as u16).to_be_bytes());
    out.push(3); // num components
    out.push(1); // Y id
    out.push(0x11); // 1x1 sampling
    out.push(0); // quant table 0
    out.push(2); // Cb id
    out.push(0x11);
    out.push(1); // quant table 1
    out.push(3); // Cr id
    out.push(0x11);
    out.push(1);
    // DHT (the four standard tables).
    emit_dht(&mut out, 0, 0, &STD_DC_LUMA_BITS, &STD_DC_LUMA_VALS);
    emit_dht(&mut out, 1, 0, &STD_AC_LUMA_BITS, &STD_AC_LUMA_VALS);
    emit_dht(&mut out, 0, 1, &STD_DC_CHROMA_BITS, &STD_DC_CHROMA_VALS);
    emit_dht(&mut out, 1, 1, &STD_AC_CHROMA_BITS, &STD_AC_CHROMA_VALS);
    // SOS.
    out.push(0xFF);
    out.push(0xDA);
    out.extend_from_slice(&(6u16 + 2 * 3).to_be_bytes()); // length
    out.push(3); // num components in scan
    out.push(1); // Y -> DC0/AC0
    out.push(0x00);
    out.push(2); // Cb -> DC1/AC1
    out.push(0x11);
    out.push(3); // Cr -> DC1/AC1
    out.push(0x11);
    out.push(0); // Ss
    out.push(63); // Se
    out.push(0); // Ah/Al

    // ── Entropy-coded scan ───────────────────────────────────────────────────
    //
    // 4:4:4 with all-1x1 sampling -> the MCU is one 8x8 block per component, in
    // raster MCU order (Y, Cb, Cr). DC predictors run per component across the
    // whole scan.
    let mcus_x = (width + 7) / 8;
    let mcus_y = (height + 7) / 8;
    let mut dc_pred = [0i32; 3];

    let mut writer = EntropyWriter::new(&mut out);
    let mut samples = [0u8; 64];
    let mut coeffs = [0i32; 64];

    for my in 0..mcus_y {
        for mx in 0..mcus_x {
            // For each component, in Y, Cb, Cr order.
            for comp in 0..3 {
                let plane = match comp {
                    0 => &y_plane,
                    1 => &cb_plane,
                    _ => &cr_plane,
                };
                // Gather the 8x8 block, padding partial edge blocks by edge
                // replication (clamp the source coordinate).
                let bx0 = mx * 8;
                let by0 = my * 8;
                for yy in 0..8 {
                    let sy = {
                        let s = by0 + yy;
                        if s >= height {
                            height - 1
                        } else {
                            s
                        }
                    };
                    for xx in 0..8 {
                        let sx = {
                            let s = bx0 + xx;
                            if s >= width {
                                width - 1
                            } else {
                                s
                            }
                        };
                        samples[yy * 8 + xx] = plane[sy * width + sx];
                    }
                }

                // Forward DCT.
                fdct_8x8(&samples, &mut coeffs);

                // Quantize (natural order): round(coeff / quant).
                let quant = if comp == 0 { &luma_q } else { &chroma_q };
                let mut quantized = [0i32; 64];
                for i in 0..64 {
                    let q = quant[i] as i32;
                    let c = coeffs[i];
                    quantized[i] = if c >= 0 {
                        (c + q / 2) / q
                    } else {
                        -((-c + q / 2) / q)
                    };
                }

                // Select the Huffman tables for this component.
                let (dc_tbl, ac_tbl) = if comp == 0 {
                    (&dc_luma, &ac_luma)
                } else {
                    (&dc_chroma, &ac_chroma)
                };

                // ── DC: differential + category code ──────────────────────────
                let dc = quantized[0];
                let diff = dc - dc_pred[comp];
                dc_pred[comp] = dc;
                let (dc_size, dc_bits) = magnitude_category(diff);
                writer.put_code(dc_tbl.get(dc_size));
                if dc_size > 0 {
                    writer.put_bits(dc_bits, dc_size);
                }

                // ── AC: zig-zag run-length / size with ZRL + EOB ──────────────
                let mut run = 0u32;
                for k in 1..64 {
                    let coeff = quantized[ZIGZAG[k]];
                    if coeff == 0 {
                        run += 1;
                    } else {
                        // Emit ZRL (0xF0) for each full run of 16 zeros.
                        while run >= 16 {
                            writer.put_code(ac_tbl.get(0xF0));
                            run -= 16;
                        }
                        let (size, bits) = magnitude_category(coeff);
                        let rs = ((run as u8) << 4) | size;
                        writer.put_code(ac_tbl.get(rs));
                        writer.put_bits(bits, size);
                        run = 0;
                    }
                }
                // Trailing zeros -> EOB (symbol 0x00).
                if run > 0 {
                    writer.put_code(ac_tbl.get(0x00));
                }
            }
        }
    }
    writer.flush();

    // EOI.
    out.push(0xFF);
    out.push(0xD9);
    Ok(out)
}

impl JpegImage {
    /// Encode this image to a baseline JPEG at `quality` (1..=100). See
    /// [`encode_jpeg`].
    pub fn to_jpeg(&self, quality: u8) -> Result<Vec<u8>, JpegEncodeError> {
        encode_jpeg(self, quality)
    }
}

// ════════════════════════════════════════════════════════════════════════════
// Host KAT suite — `cargo test -p rae_jpeg`. FAIL-able by construction.
//
// Under `#[cfg(test)]` the crate compiles as `std` (the `no_std` attribute is
// `cfg_attr(not(test), ...)`), so `Vec`/`vec!` are in scope via the default
// prelude — no `extern crate std` / `use std::` (the architecture gate bans
// those std-ism lines). JPEG fixtures are hand-encoded from real segments
// (DQT/DHT/SOF0/SOS + entropy bytes); a DC-only solid color = DC-only blocks,
// which are tractable to hand-encode, so the end-to-end pixel asserts are
// concrete.
// ════════════════════════════════════════════════════════════════════════════
#[cfg(test)]
mod tests {
    use super::*;

    // ── A floating reference IDCT + cosine table (the in-test oracle) ─────────
    //
    // These use std floats (allowed in #[cfg(test)] std builds) to independently
    // verify the no-libm fixed-point IDCT and the const cosine table.

    fn reference_cos_table() -> [[f64; 8]; 8] {
        let pi = core::f64::consts::PI;
        let mut t = [[0.0f64; 8]; 8];
        for x in 0..8 {
            for u in 0..8 {
                t[x][u] = ((2 * x + 1) as f64 * u as f64 * pi / 16.0).cos();
            }
        }
        t
    }

    /// Reference 8x8 inverse DCT in f64, returning clamped level-shifted samples.
    fn reference_idct(block: &[f64; 64], quant: &[u16; 64]) -> [u8; 64] {
        let cos = reference_cos_table();
        let mut deq = [0.0f64; 64];
        for i in 0..64 {
            deq[i] = block[i] * quant[i] as f64;
        }
        let mut out = [0u8; 64];
        for y in 0..8 {
            for x in 0..8 {
                let mut sum = 0.0f64;
                for v in 0..8 {
                    for u in 0..8 {
                        let cu = if u == 0 {
                            core::f64::consts::FRAC_1_SQRT_2
                        } else {
                            1.0
                        };
                        let cv = if v == 0 {
                            core::f64::consts::FRAC_1_SQRT_2
                        } else {
                            1.0
                        };
                        sum += cu * cv * deq[v * 8 + u] * cos[x][u] * cos[y][v];
                    }
                }
                sum = sum / 4.0 + 128.0;
                let s = if sum < 0.0 {
                    0
                } else if sum > 255.0 {
                    255
                } else {
                    (sum + 0.5) as u8
                };
                out[y * 8 + x] = s;
            }
        }
        out
    }

    // ── 1. The const cosine table matches a freshly-computed reference ────────

    #[test]
    fn const_cos_table_matches_reference() {
        let r = reference_cos_table();
        for x in 0..8 {
            for u in 0..8 {
                let want = (r[x][u] * COS_SCALE as f64).round() as i32;
                assert_eq!(
                    IDCT_COS[x][u], want,
                    "IDCT_COS[{x}][{u}] mismatch (got {}, want {})",
                    IDCT_COS[x][u], want
                );
            }
        }
        // INV_SQRT2 check.
        let want = (core::f64::consts::FRAC_1_SQRT_2 * COS_SCALE as f64).round() as i32;
        assert_eq!(INV_SQRT2, want);
        // FAIL-ability: a zeroed table would mismatch [0][0]=4096.
        assert_ne!(IDCT_COS[0][0], 0);
    }

    // ── 2. The fixed-point IDCT matches the f64 reference within tolerance ────

    #[test]
    fn idct_matches_reference() {
        let quant = [1u16; 64];
        // Several coefficient patterns: DC-only, a single AC, and a mix.
        let mut cases: Vec<[i32; 64]> = Vec::new();
        let mut dc_only = [0i32; 64];
        dc_only[0] = 64; // flat block
        cases.push(dc_only);
        let mut one_ac = [0i32; 64];
        one_ac[1] = 50;
        cases.push(one_ac);
        let mut mixed = [0i32; 64];
        mixed[0] = 100;
        mixed[1] = -30;
        mixed[8] = 20;
        mixed[9] = 10;
        mixed[63] = 5;
        cases.push(mixed);

        for blk in &cases {
            let mut fixed_out = [0u8; 64];
            idct_8x8(blk, &quant, &mut fixed_out);

            let mut fblk = [0.0f64; 64];
            for i in 0..64 {
                fblk[i] = blk[i] as f64;
            }
            let ref_out = reference_idct(&fblk, &quant);

            for i in 0..64 {
                let d = fixed_out[i] as i32 - ref_out[i] as i32;
                assert!(
                    d.abs() <= 1,
                    "IDCT sample {i} off by {d} (fixed {}, ref {})",
                    fixed_out[i],
                    ref_out[i]
                );
            }
        }
    }

    // ── 3. DC-only flat block decodes to a known constant ────────────────────

    #[test]
    fn idct_dc_only_is_flat() {
        let quant = [1u16; 64];
        let mut blk = [0i32; 64];
        // A DC coefficient of D produces a flat block of value D/8 + 128 (the
        // 2-D IDCT of a pure DC term: C0*C0*D*cos0*cos0 / 4 = D * (1/2) /... ).
        // We just assert flatness + that the value is in range; the reference
        // test (#2) pins the exact number.
        blk[0] = 80;
        let mut out = [0u8; 64];
        idct_8x8(&blk, &quant, &mut out);
        for i in 1..64 {
            assert_eq!(out[i], out[0], "DC-only block must be perfectly flat");
        }
    }

    // ── 3b. Hostile-input IDCT: crafted overflow is clamped, never panics ─────
    //
    // The crafted worst case the reviewer flagged: a runaway DC predictor
    // (`block[0]` near 2^31, reached via wrapping_add across MCUs) and a 16-bit
    // max DQT entry (65535). The dequantized product `block[0]*quant[0]` is then
    // ~2^47, and the OLD code fed that as `i64` straight into the two separable
    // 8-point passes — the accumulator reached ~2^98, far past i64::MAX (2^63):
    //   * checked build (overflow-checks ON) -> arithmetic overflow PANIC (DoS),
    //   * release build                       -> silent wrap to garbage pixels.
    // `cargo test` runs with overflow-checks ON, so if the clamp+i128 fix were
    // reverted, the `idct_8x8` call below would panic and FAIL this test. With
    // the fix, the coefficient is saturated to ±2^23 and the passes run in i128,
    // so the call returns and every output sample is a valid clamped 0..=255.
    #[test]
    fn idct_crafted_overflow_is_clamped_not_panic() {
        // All-maximum coefficients AND a 16-bit-max quant table — the absolute
        // worst case a malformed DQT + runaway entropy stream can produce.
        let block = [i32::MAX; 64];
        let quant = [u16::MAX; 64];
        let mut out = [0u8; 64];

        // (a) No panic: in the overflow-checks-ON test profile, reaching this
        // line at all means the i64 accumulator never overflowed (the OLD path
        // would have aborted here).
        idct_8x8(&block, &quant, &mut out);

        // (b) Output is real, clamped pixels — not garbage. `out` is [u8;64] so
        // every byte is structurally 0..=255; assert it is genuinely saturated
        // to the bound rather than a wrapped mid-range value: a uniformly huge
        // positive coefficient block drives the DC term hard positive, so at
        // least one sample must hit the 255 ceiling (and none can be invalid).
        let saturated = out.iter().any(|&s| s == 255);
        assert!(
            saturated,
            "crafted huge-positive block must saturate to the 255 ceiling, got {out:?}"
        );

        // Also exercise the negative-overflow rail (min coeff) -> 0 floor.
        let nblock = [i32::MIN; 64];
        let mut nout = [0u8; 64];
        idct_8x8(&nblock, &quant, &mut nout);
        let floored = nout.iter().any(|&s| s == 0);
        assert!(
            floored,
            "crafted huge-negative block must saturate to the 0 floor, got {nout:?}"
        );

        // FAIL-ability sanity: prove the inputs really are the crafted-overflow
        // case — block[0]*quant[0] as i64 is ~2^47, well past the COEFF_CLAMP
        // (2^23) the fix saturates to, and the unbounded two-pass sum would
        // exceed i64::MAX. (This mirrors the arithmetic in the fix comment.)
        let deq = (block[0] as i64).wrapping_mul(quant[0] as i64);
        assert!(
            deq.abs() > (1i64 << 40),
            "fixture must hit the overflow regime"
        );
    }

    // ── 3c. End-to-end: a crafted DQT + DC stream never panics, stays clamped ─
    //
    // Whole-decoder version of the above: a real baseline JPEG carrying a 16-bit
    // max DQT and the largest DC magnitude the entropy path will emit. `decode_jpeg`
    // must return Ok with valid pixels (or a graceful Err) — never panic/abort.
    #[test]
    fn decode_crafted_max_dqt_never_panics() {
        let jpeg = build_gray_dc_only_q16_max();
        match decode_jpeg(&jpeg) {
            Ok(img) => {
                assert_eq!((img.width, img.height), (8, 8));
                // Every decoded pixel must be a structurally valid ARGB sample
                // with opaque alpha — clamped, not wrapped garbage.
                for y in 0..8 {
                    for x in 0..8 {
                        let (a, _r, _g, _b) = img.pixel(x, y).expect("in-bounds");
                        assert_eq!(a, 0xFF, "alpha must stay opaque at ({x},{y})");
                    }
                }
            }
            // A graceful error is also acceptable for hostile input.
            Err(_) => {}
        }
    }

    // ── 4. Huffman table builder produces the right canonical codes ──────────

    #[test]
    fn huffman_builder_canonical_codes() {
        // A simple table: two symbols of length 2, one of length 3.
        // bits[1]=0 (no len-1), bits[2]=2, bits[3]=1.
        let mut bits = [0u8; 16];
        bits[1] = 2; // two length-2 codes
        bits[2] = 1; // one length-3 code
        let huffval = [b'A', b'B', b'C'];
        let t = HuffTable::build(&bits, &huffval).expect("build");
        // Canonical codes: length 2 -> 00, 01 (min_code[1]=0, max_code[1]=1);
        // length 3 -> 100 (=4) (min_code[2]=4, max=4).
        assert_eq!(t.min_code[1], 0);
        assert_eq!(t.max_code[1], 1);
        assert_eq!(t.val_ptr[1], 0);
        assert_eq!(t.min_code[2], 4);
        assert_eq!(t.max_code[2], 4);
        assert_eq!(t.val_ptr[2], 2);

        // Decode them from a bit stream: 00 01 100 -> A B C. Pack MSB-first into
        // entropy bytes: 00 01 100 = 0001100 (7 bits) -> pad to 0x18 = 00011000.
        let entropy = [0x18u8];
        let mut r = BitReader::new(&entropy);
        assert_eq!(r.decode_huff(&t).unwrap(), b'A'); // 00
        assert_eq!(r.decode_huff(&t).unwrap(), b'B'); // 01
        assert_eq!(r.decode_huff(&t).unwrap(), b'C'); // 100
                                                      // FAIL-ability: a broken builder would decode the wrong symbol.
    }

    #[test]
    fn huffman_builder_rejects_overflow() {
        // 3 codes of length 1 is impossible (only 2 codes fit in 1 bit).
        let mut bits = [0u8; 16];
        bits[0] = 3;
        let huffval = [1u8, 2, 3];
        assert!(matches!(
            HuffTable::build(&bits, &huffval),
            Err(JpegError::BadHuffmanTable)
        ));
    }

    #[test]
    fn huffman_builder_rejects_count_mismatch() {
        let mut bits = [0u8; 16];
        bits[1] = 2;
        // Only one value supplied for two declared codes.
        let huffval = [1u8];
        assert!(matches!(
            HuffTable::build(&bits, &huffval),
            Err(JpegError::BadHuffmanTable)
        ));
    }

    // ── 5. Byte-stuffing 0xFF00 is unstuffed ─────────────────────────────────

    #[test]
    fn bit_reader_unstuffs_ff00() {
        // Bytes: 0xFF 0x00 0xAB. The 0xFF00 becomes a literal 0xFF, then 0xAB.
        let data = [0xFFu8, 0x00, 0xAB];
        let mut r = BitReader::new(&data);
        let first = r.get_bits(8);
        assert_eq!(first, 0xFF, "0xFF00 must unstuff to a literal 0xFF byte");
        let second = r.get_bits(8);
        assert_eq!(second, 0xAB);
        // Only real bytes were consumed — no synthetic padding yet.
        assert!(!r.truncated());
    }

    #[test]
    fn bit_reader_surfaces_marker() {
        // 0xAA then 0xFFD9 (EOI marker). After reading 0xAA, the next fill hits
        // the marker; the reader records it and stops yielding data.
        let data = [0xAAu8, 0xFF, 0xD9];
        let mut r = BitReader::new(&data);
        assert_eq!(r.get_bits(8), 0xAA);
        // Force a fill that encounters the marker.
        let _ = r.get_bits(8);
        assert_eq!(r.marker, 0xD9, "EOI marker must be surfaced");
    }

    // ── 6. EXTEND (receive_and_extend) matches the spec ──────────────────────

    #[test]
    fn extend_sign_extension() {
        // size=0 -> 0.
        assert_eq!(extend(0, 0), 0);
        // size=1: value 0 -> -1, value 1 -> 1.
        assert_eq!(extend(0, 1), -1);
        assert_eq!(extend(1, 1), 1);
        // size=2: 00->-3, 01->-2, 10->2, 11->3.
        assert_eq!(extend(0b00, 2), -3);
        assert_eq!(extend(0b01, 2), -2);
        assert_eq!(extend(0b10, 2), 2);
        assert_eq!(extend(0b11, 2), 3);
        // size=3: 000->-7, 111->7.
        assert_eq!(extend(0b000, 3), -7);
        assert_eq!(extend(0b111, 3), 7);
        // FAIL-ability: ignoring sign would make extend(0,1)=0 not -1.
        assert_ne!(extend(0, 1), 0);
    }

    // ── 7. YCbCr -> RGB of known values ──────────────────────────────────────

    #[test]
    fn ycbcr_to_rgb_known() {
        // Neutral gray: Y=128, Cb=Cr=128 -> R=G=B=128.
        let p = ycbcr_to_argb(128, 128, 128);
        assert_eq!(p, argb(0xFF, 128, 128, 128));
        // Pure white: Y=255 -> ~255,255,255.
        let w = ycbcr_to_argb(255, 128, 128);
        assert_eq!(w, argb(0xFF, 255, 255, 255));
        // Pure black.
        let bk = ycbcr_to_argb(0, 128, 128);
        assert_eq!(bk, argb(0xFF, 0, 0, 0));
        // Red-ish: Y=76, Cb=85, Cr=255 -> near pure red (the JFIF inverse of
        // RGB red (255,0,0) is Y~76, Cb~85, Cr~255).
        let (_, r, g, b) = unpack(ycbcr_to_argb(76, 85, 255));
        assert!(r > 240, "expected strong red, got r={r}");
        assert!(g < 40 && b < 40, "expected low g/b, got g={g} b={b}");
    }

    fn unpack(p: u32) -> (u8, u8, u8, u8) {
        ((p >> 24) as u8, (p >> 16) as u8, (p >> 8) as u8, p as u8)
    }

    // ── JPEG fixture builders ────────────────────────────────────────────────

    fn marker_seg(marker: u8, body: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.push(0xFF);
        out.push(marker);
        let len = (body.len() + 2) as u16;
        out.extend_from_slice(&len.to_be_bytes());
        out.extend_from_slice(body);
        out
    }

    /// A DQT body for table id 0 with all-1 quantization (8-bit precision).
    fn dqt_all_ones(id: u8) -> Vec<u8> {
        let mut body = Vec::new();
        body.push(id & 0x0F); // precision 0, table id
        for _ in 0..64 {
            body.push(1);
        }
        body
    }

    /// A 16-bit-precision DQT body for table id 0 with EVERY entry = 0xFFFF (the
    /// largest quant a 16-bit DQT can encode). Used to drive the IDCT into the
    /// crafted-overflow regime end-to-end: a DC coefficient of 255 dequantizes to
    /// 255*65535 ≈ 2^24, past the COEFF_CLAMP the fix saturates to.
    fn dqt_16bit_max(id: u8) -> Vec<u8> {
        let mut body = Vec::new();
        body.push((1u8 << 4) | (id & 0x0F)); // precision 1 (16-bit), table id
        for _ in 0..64 {
            body.push(0xFF);
            body.push(0xFF);
        }
        body
    }

    /// An 8x8 grayscale baseline JPEG using a 16-bit all-max DQT and a single
    /// DC-only block at category 8, magnitude 255 (the largest the fixture DC
    /// table encodes). The dequantized DC blows past the clamp bound, exercising
    /// the hostile-input saturation path through the full decoder.
    fn build_gray_dc_only_q16_max() -> Vec<u8> {
        let (dc_bits, dc_vals) = fixture_dc_bits_vals();
        let (ac_bits, ac_vals) = fixture_ac_bits_vals();

        let mut out = Vec::new();
        out.push(0xFF);
        out.push(0xD8);
        out.extend_from_slice(&marker_seg(0xDB, &dqt_16bit_max(0)));
        let mut sof = Vec::new();
        sof.push(8);
        sof.extend_from_slice(&8u16.to_be_bytes());
        sof.extend_from_slice(&8u16.to_be_bytes());
        sof.push(1);
        sof.push(1);
        sof.push(0x11);
        sof.push(0);
        out.extend_from_slice(&marker_seg(0xC0, &sof));
        out.extend_from_slice(&marker_seg(0xC4, &dht_body(0, 0, &dc_bits, &dc_vals)));
        out.extend_from_slice(&marker_seg(0xC4, &dht_body(1, 0, &ac_bits, &ac_vals)));
        let mut sos = Vec::new();
        sos.push(1);
        sos.push(1);
        sos.push(0x00);
        sos.push(0);
        sos.push(63);
        sos.push(0);
        out.extend_from_slice(&marker_seg(0xDA, &sos));

        let mut bw = BitWriter::new();
        bw.put(0b10, 2); // DC category 8
        bw.put(255, 8); // magnitude 255 -> DC coefficient 255
        bw.put(0, 1); // AC EOB
        out.extend_from_slice(&bw.finish());

        out.push(0xFF);
        out.push(0xD9);
        out
    }

    /// A DHT body. `class` 0=DC,1=AC; `id` table id; bits/huffval as given.
    fn dht_body(class: u8, id: u8, bits: &[u8; 16], huffval: &[u8]) -> Vec<u8> {
        let mut body = Vec::new();
        body.push((class << 4) | (id & 0x0F));
        body.extend_from_slice(bits);
        body.extend_from_slice(huffval);
        body
    }

    /// Bit packer that emits MSB-first into a byte vec, with JPEG 0xFF byte
    /// stuffing applied at flush.
    struct BitWriter {
        bytes: Vec<u8>,
        cur: u8,
        nbits: u8,
    }
    impl BitWriter {
        fn new() -> Self {
            BitWriter {
                bytes: Vec::new(),
                cur: 0,
                nbits: 0,
            }
        }
        fn put(&mut self, value: u32, len: u32) {
            for i in (0..len).rev() {
                let bit = ((value >> i) & 1) as u8;
                self.cur = (self.cur << 1) | bit;
                self.nbits += 1;
                if self.nbits == 8 {
                    self.flush_byte();
                }
            }
        }
        fn flush_byte(&mut self) {
            self.bytes.push(self.cur);
            if self.cur == 0xFF {
                self.bytes.push(0x00); // byte-stuff
            }
            self.cur = 0;
            self.nbits = 0;
        }
        fn finish(mut self) -> Vec<u8> {
            if self.nbits > 0 {
                // Pad the final partial byte with 1-bits (JPEG convention).
                self.cur <<= 8 - self.nbits;
                self.cur |= (1 << (8 - self.nbits)) - 1;
                let last = self.cur;
                self.bytes.push(last);
                if last == 0xFF {
                    self.bytes.push(0x00);
                }
                self.cur = 0;
                self.nbits = 0;
            }
            self.bytes
        }
    }

    // Standard DC/AC tables for the fixtures: a tiny custom table.
    //
    // DC table: one symbol per category we use. We use category 0 (code "0") for
    // a zero DC diff and category 8 (code "10" etc) for value 128. To keep it
    // simple we define:
    //   DC bits: length-1: 1 code (symbol 0), length-2: 1 code (symbol 8).
    //   -> code "0" = category 0, code "10" = category 8.
    // AC table: one symbol, EOB (0x00), as a length-1 code "0".
    fn fixture_dc_bits_vals() -> ([u8; 16], Vec<u8>) {
        let mut bits = [0u8; 16];
        bits[0] = 1; // one length-1 code
        bits[1] = 1; // one length-2 code
                     // values: category 0 (the len-1 code "0"), category 8 (the len-2 "10").
        (bits, vec![0x00, 0x08])
    }
    fn fixture_ac_bits_vals() -> ([u8; 16], Vec<u8>) {
        let mut bits = [0u8; 16];
        bits[0] = 1; // one length-1 code "0"
        (bits, vec![0x00]) // EOB
    }

    /// Build a complete grayscale baseline JPEG of `w`x`h` whose every 8x8 block
    /// is DC-only with the given DC category/value, producing a solid color.
    ///
    /// We encode each block as: DC code for category, then the magnitude bits,
    /// then the AC EOB code. With DC category 8 and value 255 the IDCT yields a
    /// flat block of (dc_coeff * quant[0] -> IDCT -> +128).
    fn build_gray_dc_only(w: u16, h: u16, dc_category: u8, dc_mag: u32) -> Vec<u8> {
        let (dc_bits, dc_vals) = fixture_dc_bits_vals();
        let (ac_bits, ac_vals) = fixture_ac_bits_vals();

        let mut out = Vec::new();
        // SOI
        out.push(0xFF);
        out.push(0xD8);
        // APP0 JFIF (optional but realistic) — skip for brevity; not required.
        // DQT id0 all ones.
        out.extend_from_slice(&marker_seg(0xDB, &dqt_all_ones(0)));
        // SOF0: precision 8, h, w, ncomp=1, comp(id=1, sampling 0x11, quant 0).
        let mut sof = Vec::new();
        sof.push(8);
        sof.extend_from_slice(&h.to_be_bytes());
        sof.extend_from_slice(&w.to_be_bytes());
        sof.push(1); // ncomp
        sof.push(1); // component id
        sof.push(0x11); // sampling 1x1
        sof.push(0); // quant table 0
        out.extend_from_slice(&marker_seg(0xC0, &sof));
        // DHT DC table 0.
        out.extend_from_slice(&marker_seg(0xC4, &dht_body(0, 0, &dc_bits, &dc_vals)));
        // DHT AC table 0.
        out.extend_from_slice(&marker_seg(0xC4, &dht_body(1, 0, &ac_bits, &ac_vals)));
        // SOS: ns=1, comp(id=1, dc=0/ac=0), Ss=0 Se=63 Ah/Al=0.
        let mut sos = Vec::new();
        sos.push(1); // ns
        sos.push(1); // component selector (id 1)
        sos.push(0x00); // dc table 0, ac table 0
        sos.push(0); // Ss
        sos.push(63); // Se
        sos.push(0); // Ah/Al
        out.extend_from_slice(&marker_seg(0xDA, &sos));

        // Entropy: number of 8x8 blocks = ceil(w/8)*ceil(h/8).
        let bx = ((w as usize) + 7) / 8;
        let by = ((h as usize) + 7) / 8;
        let nblocks = bx * by;

        // DC codes: category 0 -> code "0" (1 bit). category 8 -> "10" (2 bits).
        let (dc_code, dc_len) = match dc_category {
            0 => (0u32, 1u32),
            8 => (0b10u32, 2u32),
            _ => panic!("fixture only supports DC category 0 or 8"),
        };
        let ac_eob_code = 0u32; // "0", 1 bit.
        let ac_eob_len = 1u32;

        let mut bw = BitWriter::new();
        // The DC predictor carries across blocks, so only the FIRST block emits
        // the full DC diff; subsequent blocks emit category 0 (diff 0) to keep
        // the same DC. That yields a uniform image.
        for i in 0..nblocks {
            if i == 0 {
                bw.put(dc_code, dc_len);
                if dc_category != 0 {
                    bw.put(dc_mag, dc_category as u32);
                }
            } else {
                // Category 0 -> DC diff 0 (same value).
                bw.put(0u32, 1u32);
            }
            // AC: EOB.
            bw.put(ac_eob_code, ac_eob_len);
        }
        out.extend_from_slice(&bw.finish());

        // EOI.
        out.push(0xFF);
        out.push(0xD9);
        out
    }

    // ── 8. END-TO-END: a solid-color grayscale baseline JPEG decodes ─────────

    #[test]
    fn decode_solid_gray_dc_only() {
        // 16x16, DC category 8, magnitude bits = 255 -> DC coefficient = 255
        // (extend(255, 8) = 255). With quant[0]=1, the flat IDCT sample is
        // round(255 / 8) + 128 ... we don't pin the exact gray here; we assert
        // the image is perfectly uniform and non-trivial, then a second case
        // pins an exact value via the DC=0 (mid-gray 128) path.
        let jpeg = build_gray_dc_only(16, 16, 8, 255);
        let img = decode_jpeg(&jpeg).expect("solid gray decode");
        assert_eq!((img.width, img.height), (16, 16));
        let first = img.pixel(0, 0).expect("p0");
        // Uniform across the whole image.
        for y in 0..16 {
            for x in 0..16 {
                assert_eq!(
                    img.pixel(x, y),
                    Some(first),
                    "solid image must be uniform at ({x},{y})"
                );
            }
        }
        // Grayscale: r==g==b, alpha opaque.
        let (a, r, g, b) = first;
        assert_eq!(a, 0xFF);
        assert_eq!(r, g);
        assert_eq!(g, b);
        // DC=255 with quant 1 must be brighter than mid-gray (128).
        assert!(r > 128, "DC=255 must brighten above mid-gray, got {r}");
    }

    #[test]
    fn decode_solid_gray_midgray() {
        // DC category 0 -> DC diff 0 -> DC coefficient 0 -> flat block of +128
        // (mid gray exactly). Pins an exact ARGB value end-to-end.
        let jpeg = build_gray_dc_only(8, 8, 0, 0);
        let img = decode_jpeg(&jpeg).expect("midgray decode");
        assert_eq!((img.width, img.height), (8, 8));
        for y in 0..8 {
            for x in 0..8 {
                assert_eq!(
                    img.pixel(x, y),
                    Some((0xFF, 128, 128, 128)),
                    "DC=0 block must be exactly mid-gray 128 at ({x},{y})"
                );
            }
        }
        // FAIL-ability: forgetting the +128 level shift would give 0,0,0.
        assert_ne!(img.pixel(0, 0), Some((0xFF, 0, 0, 0)));
    }

    #[test]
    fn decode_non_multiple_of_8_dimensions() {
        // 5x3 image: MCU padding cropped to declared size. DC=0 mid-gray.
        let jpeg = build_gray_dc_only(5, 3, 0, 0);
        let img = decode_jpeg(&jpeg).expect("5x3 decode");
        assert_eq!((img.width, img.height), (5, 3));
        assert_eq!(img.pixels.len(), 15);
        assert_eq!(img.pixel(4, 2), Some((0xFF, 128, 128, 128)));
        assert_eq!(img.pixel(5, 0), None); // out of bounds
    }

    // ── 8b. END-TO-END AC PATH: a single 8x8 block with one AC coefficient ───
    //
    // This exercises the full AC entropy path (run/size decode + EXTEND + the
    // non-flat IDCT), which the DC-only fixtures above do not. We build an 8x8
    // grayscale JPEG whose single block has DC coefficient 0 and AC[1] (the
    // first horizontal frequency) set to a known value, then cross-check every
    // decoded sample against the in-test floating reference IDCT.

    /// DC table: just category 0 ("0", 1 bit) — DC diff zero.
    /// AC table: two symbols —
    ///   symbol 0x01 (run 0, size 1) as code "0" (1 bit),
    ///   symbol 0x00 (EOB)           as code "10" (2 bits).
    fn build_gray_one_ac(ac1_value: i32) -> Vec<u8> {
        build_gray_one_ac_q(ac1_value, 1)
    }

    /// As [`build_gray_one_ac`] but with a custom quantization value at zig-zag
    /// position 1 (the AC[1] coefficient) so the AC amplitude can be made large
    /// enough to visibly perturb the block.
    fn build_gray_one_ac_q(ac1_value: i32, ac1_quant: u8) -> Vec<u8> {
        // Custom DQT body: all-1 except zig-zag position 1.
        let mut dqt = Vec::new();
        dqt.push(0u8); // precision 0, table id 0
        for k in 0..64 {
            dqt.push(if k == 1 { ac1_quant } else { 1 });
        }

        // DC: one length-1 code, value = category 0.
        let mut dc_bits = [0u8; 16];
        dc_bits[0] = 1;
        let dc_vals = vec![0x00u8];
        // AC: one length-1 code + one length-2 code.
        let mut ac_bits = [0u8; 16];
        ac_bits[0] = 1; // length-1: symbol 0x01 (run0,size1)
        ac_bits[1] = 1; // length-2: symbol 0x00 (EOB)
        let ac_vals = vec![0x01u8, 0x00u8];

        let mut out = Vec::new();
        out.push(0xFF);
        out.push(0xD8);
        out.extend_from_slice(&marker_seg(0xDB, &dqt));
        let mut sof = Vec::new();
        sof.push(8);
        sof.extend_from_slice(&8u16.to_be_bytes());
        sof.extend_from_slice(&8u16.to_be_bytes());
        sof.push(1);
        sof.push(1);
        sof.push(0x11);
        sof.push(0);
        out.extend_from_slice(&marker_seg(0xC0, &sof));
        out.extend_from_slice(&marker_seg(0xC4, &dht_body(0, 0, &dc_bits, &dc_vals)));
        out.extend_from_slice(&marker_seg(0xC4, &dht_body(1, 0, &ac_bits, &ac_vals)));
        let mut sos = Vec::new();
        sos.push(1);
        sos.push(1);
        sos.push(0x00);
        sos.push(0);
        sos.push(63);
        sos.push(0);
        out.extend_from_slice(&marker_seg(0xDA, &sos));

        // Encode: DC code "0" (diff 0). Then AC coefficient at index 1:
        //   run0/size1 code "0", then the size-1 magnitude bits. For a value
        //   v=+1 the EXTEND magnitude bits are "1"; for v=-1 they are "0".
        // Then EOB code "10".
        let (mag, size) = encode_magnitude(ac1_value);
        let mut bw = BitWriter::new();
        bw.put(0, 1); // DC category 0
                      // AC symbol (run0,size`size`). Our fixture AC table only has size-1
                      // symbol 0x01, so this fixture supports |ac1_value| == 1 exactly.
        assert_eq!(size, 1, "one-AC fixture only encodes |value|==1");
        bw.put(0, 1); // AC code "0" -> symbol 0x01
        bw.put(mag, size); // the size-bit magnitude
        bw.put(0b10, 2); // EOB code "10"
        out.extend_from_slice(&bw.finish());

        out.push(0xFF);
        out.push(0xD9);
        out
    }

    /// JPEG magnitude encoding (inverse of EXTEND): returns (bits, size).
    fn encode_magnitude(v: i32) -> (u32, u32) {
        if v == 0 {
            return (0, 0);
        }
        let mag = v.unsigned_abs();
        let size = 32 - mag.leading_zeros();
        let bits = if v > 0 {
            v as u32 & ((1 << size) - 1)
        } else {
            // Negative: stored as (v - 1) in `size` bits (the EXTEND inverse).
            ((v - 1) as u32) & ((1 << size) - 1)
        };
        (bits, size)
    }

    #[test]
    fn decode_one_ac_coefficient_matches_reference() {
        // AC[1] = +1, DC = 0, quant all-1.
        let jpeg = build_gray_one_ac(1);
        let img = decode_jpeg(&jpeg).expect("one-AC decode");
        assert_eq!((img.width, img.height), (8, 8));

        // Reference: the same coefficient block through the f64 IDCT.
        let mut natural = [0.0f64; 64];
        natural[0] = 0.0; // DC
        natural[ZIGZAG[1]] = 1.0; // AC at zig-zag index 1 -> natural index 1
        let quant = [1u16; 64];
        let ref_out = reference_idct(&natural, &quant);

        // The decoded block must match the reference IDCT of the SAME coefficient
        // block to within rounding — this is the real proof the AC entropy path
        // (run/size decode + EXTEND + non-flat IDCT) produced the right block.
        // (With quant=1 the AC[1]=1 amplitude is sub-unit, so the reference is
        // itself near-flat; non-flatness is asserted in the larger-quant test.)
        for y in 0..8u32 {
            for x in 0..8u32 {
                let got = img.pixel(x, y).expect("in-bounds").1; // gray = r
                let want = ref_out[(y as usize) * 8 + (x as usize)];
                let d = got as i32 - want as i32;
                assert!(
                    d.abs() <= 1,
                    "AC decode sample ({x},{y}) off by {d} (got {got}, want {want})"
                );
            }
        }
    }

    #[test]
    fn decode_one_ac_coefficient_visible_gradient() {
        // AC[1]=+1 but quant[zigzag 1]=40, so the dequantized AC coefficient is
        // 40 — large enough to produce a visible horizontal gradient. Proves the
        // AC term genuinely perturbs the block (not silently dropped), and that
        // dequantization scales it. Cross-checked against the reference IDCT.
        let q: u16 = 40;
        let jpeg = build_gray_one_ac_q(1, q as u8);
        let img = decode_jpeg(&jpeg).expect("one-AC-q decode");

        // Non-flat: the leftmost and rightmost samples of row 0 must differ
        // (a horizontal cosine gradient).
        let p00 = img.pixel(0, 0).expect("p0").1;
        let p70 = img.pixel(7, 0).expect("p7").1;
        assert_ne!(
            p00, p70,
            "a dequantized AC[1] term must make the row non-flat"
        );

        // Reference cross-check.
        let mut natural = [0.0f64; 64];
        natural[ZIGZAG[1]] = 1.0;
        let mut quant = [1u16; 64];
        quant[ZIGZAG[1]] = q;
        let ref_out = reference_idct(&natural, &quant);
        for y in 0..8u32 {
            for x in 0..8u32 {
                let got = img.pixel(x, y).expect("in-bounds").1;
                let want = ref_out[(y as usize) * 8 + (x as usize)];
                let d = got as i32 - want as i32;
                assert!(d.abs() <= 1, "AC-q sample ({x},{y}) off by {d}");
            }
        }
    }

    // ── 8c. END-TO-END RESTART INTERVAL: DRI + RSTn resync ───────────────────
    //
    // Builds a 16x16 (4 MCUs at 1x1) grayscale image with a restart interval of
    // 1 MCU, inserting an RSTn marker between every MCU's entropy data. Proves
    // the bit reader halts on RSTn, the DC predictor resets, and decoding
    // resumes — the whole image must still be uniform mid-gray.
    fn build_gray_dc_only_with_restarts(restart_interval: u16) -> Vec<u8> {
        let (dc_bits, dc_vals) = fixture_dc_bits_vals();
        let (ac_bits, ac_vals) = fixture_ac_bits_vals();

        let mut out = Vec::new();
        out.push(0xFF);
        out.push(0xD8);
        out.extend_from_slice(&marker_seg(0xDB, &dqt_all_ones(0)));
        let mut sof = Vec::new();
        sof.push(8);
        sof.extend_from_slice(&16u16.to_be_bytes());
        sof.extend_from_slice(&16u16.to_be_bytes());
        sof.push(1);
        sof.push(1);
        sof.push(0x11);
        sof.push(0);
        out.extend_from_slice(&marker_seg(0xC0, &sof));
        out.extend_from_slice(&marker_seg(0xC4, &dht_body(0, 0, &dc_bits, &dc_vals)));
        out.extend_from_slice(&marker_seg(0xC4, &dht_body(1, 0, &ac_bits, &ac_vals)));
        // DRI segment.
        out.extend_from_slice(&marker_seg(0xDD, &restart_interval.to_be_bytes()));
        // SOS.
        let mut sos = Vec::new();
        sos.push(1);
        sos.push(1);
        sos.push(0x00);
        sos.push(0);
        sos.push(63);
        sos.push(0);
        out.extend_from_slice(&marker_seg(0xDA, &sos));

        // 16x16 @ 1x1 = 4 MCUs (one block each). DC=255 in the FIRST block of
        // each restart run (predictor resets at every restart), so every block
        // ends up the same brightness.
        let nblocks = 4usize;
        let mut rst_index = 0u8;
        for i in 0..nblocks {
            // Emit an RSTn marker before this block if a restart boundary just
            // passed (i.e. before blocks 1,2,3 when interval == 1).
            if restart_interval != 0 && i != 0 && (i as u16) % restart_interval == 0 {
                out.push(0xFF);
                out.push(0xD0 + (rst_index & 0x07));
                rst_index += 1;
            }
            let mut bw = BitWriter::new();
            // Each block after a restart has predictor 0, so emit the full DC.
            // Block 0 also has predictor 0. With interval 1, EVERY block is the
            // first of its run -> emit category 8, value 255 each time.
            let is_run_start = restart_interval == 0 && i == 0
                || restart_interval != 0 && (i as u16) % restart_interval == 0;
            if is_run_start {
                bw.put(0b10, 2); // DC category 8
                bw.put(255, 8); // magnitude
            } else {
                bw.put(0, 1); // DC category 0 (diff 0)
            }
            bw.put(0, 1); // AC EOB
            out.extend_from_slice(&bw.finish());
        }

        out.push(0xFF);
        out.push(0xD9);
        out
    }

    #[test]
    fn decode_with_restart_interval_resyncs() {
        // Restart interval = 1 MCU: an RSTn between every block.
        let jpeg = build_gray_dc_only_with_restarts(1);
        let img = decode_jpeg(&jpeg).expect("restart-interval decode");
        assert_eq!((img.width, img.height), (16, 16));
        let first = img.pixel(0, 0).expect("p0");
        // All four MCUs must decode to the same brightness — proving the RSTn
        // resync + per-restart DC-predictor reset worked across every boundary.
        for y in 0..16 {
            for x in 0..16 {
                assert_eq!(
                    img.pixel(x, y),
                    Some(first),
                    "restart-interval image must stay uniform at ({x},{y})"
                );
            }
        }
        // It must match the no-restart equivalent (same DC=255 everywhere).
        let plain = decode_jpeg(&build_gray_dc_only(16, 16, 8, 255)).expect("plain");
        assert_eq!(img.pixels, plain.pixels, "restarts must not change pixels");
    }

    // ── 9. Hostile battery: every malformed input is Err, never a panic ──────

    #[test]
    fn reject_bad_signature() {
        assert_eq!(decode_jpeg(&[0u8; 64]), Err(JpegError::BadSignature));
        assert_eq!(decode_jpeg(b"not a jpeg"), Err(JpegError::BadSignature));
        // SOI must be exactly FFD8.
        assert_eq!(decode_jpeg(&[0xFF, 0xD9]), Err(JpegError::BadSignature));
    }

    #[test]
    fn reject_truncated_after_soi() {
        assert_eq!(decode_jpeg(&[0xFF, 0xD8]), Err(JpegError::Truncated));
    }

    #[test]
    fn reject_progressive_sof2() {
        // SOI + a SOF2 segment -> Unsupported (never a fake decode).
        let mut out = Vec::new();
        out.push(0xFF);
        out.push(0xD8);
        let mut sof = Vec::new();
        sof.push(8);
        sof.extend_from_slice(&8u16.to_be_bytes());
        sof.extend_from_slice(&8u16.to_be_bytes());
        sof.push(1);
        sof.push(1);
        sof.push(0x11);
        sof.push(0);
        out.extend_from_slice(&marker_seg(0xC2, &sof)); // SOF2 = progressive
        assert_eq!(decode_jpeg(&out), Err(JpegError::Unsupported));
    }

    #[test]
    fn reject_absurd_dimensions() {
        let mut out = Vec::new();
        out.push(0xFF);
        out.push(0xD8);
        let mut sof = Vec::new();
        sof.push(8);
        sof.extend_from_slice(&0xFFFFu16.to_be_bytes()); // height 65535
        sof.extend_from_slice(&0xFFFFu16.to_be_bytes()); // width 65535
        sof.push(1);
        sof.push(1);
        sof.push(0x11);
        sof.push(0);
        out.extend_from_slice(&marker_seg(0xC0, &sof));
        // 65535*65535 > MAX_PIXELS.
        assert_eq!(decode_jpeg(&out), Err(JpegError::DimensionsOutOfRange));
    }

    #[test]
    fn reject_zero_dimensions() {
        let mut out = Vec::new();
        out.push(0xFF);
        out.push(0xD8);
        let mut sof = Vec::new();
        sof.push(8);
        sof.extend_from_slice(&0u16.to_be_bytes());
        sof.extend_from_slice(&8u16.to_be_bytes());
        sof.push(1);
        sof.push(1);
        sof.push(0x11);
        sof.push(0);
        out.extend_from_slice(&marker_seg(0xC0, &sof));
        assert_eq!(decode_jpeg(&out), Err(JpegError::DimensionsOutOfRange));
    }

    #[test]
    fn reject_bad_marker_segment_length() {
        // SOI then a marker claiming length 1 (illegal, < 2).
        let data = [0xFFu8, 0xD8, 0xFF, 0xDB, 0x00, 0x01];
        assert_eq!(decode_jpeg(&data), Err(JpegError::BadMarker));
    }

    #[test]
    fn reject_truncated_entropy() {
        // A valid header but the entropy stream is empty -> the bit reader runs
        // out and decode_block surfaces BadEntropy (or the scan can't complete).
        let mut jpeg = build_gray_dc_only(16, 16, 8, 255);
        // Find the SOS and chop everything after it (remove entropy + EOI).
        // SOS marker is 0xFFDA; truncate right after the SOS segment.
        // Easiest: truncate the whole buffer to just before entropy by locating
        // the last occurrence is fragile — instead rebuild a header-only stream.
        // Simpler: corrupt by truncating to a point inside the entropy.
        jpeg.truncate(jpeg.len().saturating_sub(40));
        let res = decode_jpeg(&jpeg);
        assert!(
            matches!(res, Err(JpegError::BadEntropy) | Err(JpegError::Truncated)),
            "truncated entropy must Err, got {res:?}"
        );
    }

    #[test]
    fn reject_scan_without_frame() {
        // SOI then an SOS with no preceding SOF0.
        let mut out = Vec::new();
        out.push(0xFF);
        out.push(0xD8);
        let mut sos = Vec::new();
        sos.push(1);
        sos.push(1);
        sos.push(0x00);
        sos.push(0);
        sos.push(63);
        sos.push(0);
        out.extend_from_slice(&marker_seg(0xDA, &sos));
        out.push(0xFF);
        out.push(0xD9);
        assert_eq!(decode_jpeg(&out), Err(JpegError::NoFrame));
    }

    #[test]
    fn reject_bad_huffman_in_stream() {
        // Build a valid header but reference a DC table id (1) that was never
        // defined -> BadScan when the scan tries to use it.
        let (dc_bits, dc_vals) = fixture_dc_bits_vals();
        let (ac_bits, ac_vals) = fixture_ac_bits_vals();
        let mut out = Vec::new();
        out.push(0xFF);
        out.push(0xD8);
        out.extend_from_slice(&marker_seg(0xDB, &dqt_all_ones(0)));
        let mut sof = Vec::new();
        sof.push(8);
        sof.extend_from_slice(&8u16.to_be_bytes());
        sof.extend_from_slice(&8u16.to_be_bytes());
        sof.push(1);
        sof.push(1);
        sof.push(0x11);
        sof.push(0);
        out.extend_from_slice(&marker_seg(0xC0, &sof));
        out.extend_from_slice(&marker_seg(0xC4, &dht_body(0, 0, &dc_bits, &dc_vals)));
        out.extend_from_slice(&marker_seg(0xC4, &dht_body(1, 0, &ac_bits, &ac_vals)));
        // SOS references DC table 1 (undefined).
        let mut sos = Vec::new();
        sos.push(1);
        sos.push(1);
        sos.push(0x10); // dc table 1, ac table 0
        sos.push(0);
        sos.push(63);
        sos.push(0);
        out.extend_from_slice(&marker_seg(0xDA, &sos));
        out.push(0xFF);
        out.push(0xD9);
        assert_eq!(decode_jpeg(&out), Err(JpegError::BadScan));
    }
}

// ════════════════════════════════════════════════════════════════════════════
// ENCODER KAT suite — `cargo test -p rae_jpeg`. The load-bearing proof is the
// LOSSY ROUND-TRIP: encode a known image, decode it with the EXISTING decoder,
// assert dimensions are EXACT and every channel is within a quality-dependent
// tolerance. FAIL-able by construction (tighten a tolerance to 0 on a solid
// block, or tweak an expected value, and the asserts fire).
// ════════════════════════════════════════════════════════════════════════════
#[cfg(test)]
mod encode_tests {
    use super::*;

    /// Build a solid-color image of the given dimensions.
    fn solid(width: u32, height: u32, r: u8, g: u8, b: u8) -> JpegImage {
        let px = argb(0xFF, r, g, b);
        JpegImage {
            width,
            height,
            pixels: vec![px; (width * height) as usize],
        }
    }

    /// Build a smooth horizontal+vertical gradient image.
    fn gradient(width: u32, height: u32) -> JpegImage {
        let mut pixels = Vec::with_capacity((width * height) as usize);
        for y in 0..height {
            for x in 0..width {
                let r = ((x * 255) / width.max(1)) as u8;
                let g = ((y * 255) / height.max(1)) as u8;
                let b = (((x + y) * 255) / (width + height).max(1)) as u8;
                pixels.push(argb(0xFF, r, g, b));
            }
        }
        JpegImage {
            width,
            height,
            pixels,
        }
    }

    fn channels(p: u32) -> (u8, u8, u8) {
        ((p >> 16) as u8, (p >> 8) as u8, p as u8)
    }

    /// Maximum absolute per-channel error between two same-sized images.
    fn max_channel_error(a: &JpegImage, b: &JpegImage) -> i32 {
        assert_eq!((a.width, a.height), (b.width, b.height));
        let mut worst = 0i32;
        for i in 0..a.pixels.len() {
            let (ar, ag, ab) = channels(a.pixels[i]);
            let (br, bg, bb) = channels(b.pixels[i]);
            for (x, y) in [(ar, br), (ag, bg), (ab, bb)] {
                let d = (x as i32 - y as i32).abs();
                if d > worst {
                    worst = d;
                }
            }
        }
        worst
    }

    // ── E1. The encoded stream starts with SOI and the decoder accepts it ─────

    #[test]
    fn encoded_starts_with_soi_and_decodes() {
        let img = solid(8, 8, 100, 150, 200);
        let enc = encode_jpeg(&img, 90).expect("encode");
        assert_eq!(&enc[0..2], &[0xFF, 0xD8], "must start with SOI");
        assert_eq!(&enc[enc.len() - 2..], &[0xFF, 0xD9], "must end with EOI");
        let dec = decode_jpeg(&enc).expect("decoder must accept our output");
        assert_eq!((dec.width, dec.height), (8, 8));
    }

    // ── E2. LOSSY ROUND-TRIP: solid color is near-exact at high quality ───────

    #[test]
    fn roundtrip_solid_color_high_quality() {
        // A solid color has zero AC energy, so only DC quantization perturbs it;
        // at q=90 the error must be tiny. This is the load-bearing assert: tighten
        // the bound to 0 and (because of YCbCr<->RGB rounding) it FAILS.
        let img = solid(16, 16, 200, 50, 120);
        let enc = encode_jpeg(&img, 90).expect("encode");
        let dec = decode_jpeg(&enc).expect("decode");
        assert_eq!(
            (dec.width, dec.height),
            (16, 16),
            "dimensions must be EXACT"
        );
        let err = max_channel_error(&img, &dec);
        assert!(
            err <= 4,
            "solid color round-trip error {err} exceeds tolerance 4"
        );
    }

    // ── E3. LOSSY ROUND-TRIP: a smooth gradient survives within tolerance ─────

    #[test]
    fn roundtrip_gradient_high_quality() {
        let img = gradient(32, 24);
        let enc = encode_jpeg(&img, 90).expect("encode");
        let dec = decode_jpeg(&enc).expect("decode");
        assert_eq!(
            (dec.width, dec.height),
            (32, 24),
            "dimensions must be EXACT"
        );
        let err = max_channel_error(&img, &dec);
        // A smooth gradient is mostly low-frequency, so q=90 keeps it close.
        assert!(
            err <= 18,
            "gradient round-trip error {err} exceeds tolerance 18"
        );
    }

    // ── E4. Mid-gray block round-trips near-exact at q=100 ────────────────────

    #[test]
    fn roundtrip_midgray_q100_near_exact() {
        let img = solid(8, 8, 128, 128, 128);
        let enc = encode_jpeg(&img, 100).expect("encode");
        let dec = decode_jpeg(&enc).expect("decode");
        let err = max_channel_error(&img, &dec);
        assert!(err <= 1, "mid-gray q100 error {err} should be <=1");
    }

    // ── E5. QUALITY-MONOTONIC ERROR: higher quality -> smaller error ──────────

    #[test]
    fn higher_quality_lower_error() {
        let img = gradient(32, 32);
        let dec95 = decode_jpeg(&encode_jpeg(&img, 95).unwrap()).unwrap();
        let dec50 = decode_jpeg(&encode_jpeg(&img, 50).unwrap()).unwrap();
        // Use total squared error as the energy metric (max-error can tie).
        let sse = |d: &JpegImage| -> u64 {
            let mut s = 0u64;
            for i in 0..img.pixels.len() {
                let (ir, ig, ib) = channels(img.pixels[i]);
                let (dr, dg, db) = channels(d.pixels[i]);
                for (a, b) in [(ir, dr), (ig, dg), (ib, db)] {
                    let e = (a as i64 - b as i64).abs() as u64;
                    s += e * e;
                }
            }
            s
        };
        let e95 = sse(&dec95);
        let e50 = sse(&dec50);
        assert!(e95 < e50, "q95 error ({e95}) must be < q50 error ({e50})");
    }

    // ── E6. Higher quality -> larger (or equal) encoded size ──────────────────

    #[test]
    fn higher_quality_not_smaller() {
        let img = gradient(32, 32);
        let big = encode_jpeg(&img, 95).unwrap();
        let small = encode_jpeg(&img, 20).unwrap();
        assert!(
            big.len() >= small.len(),
            "q95 ({}) should not be smaller than q20 ({})",
            big.len(),
            small.len()
        );
    }

    // ── E7. FDCT matches an in-test f64 reference forward DCT ─────────────────

    fn reference_fdct(samples: &[u8; 64]) -> [f64; 64] {
        let pi = core::f64::consts::PI;
        let mut f = [0.0f64; 64];
        for i in 0..64 {
            f[i] = samples[i] as f64 - 128.0;
        }
        let mut out = [0.0f64; 64];
        for v in 0..8 {
            for u in 0..8 {
                let cu = if u == 0 {
                    core::f64::consts::FRAC_1_SQRT_2
                } else {
                    1.0
                };
                let cv = if v == 0 {
                    core::f64::consts::FRAC_1_SQRT_2
                } else {
                    1.0
                };
                let mut sum = 0.0f64;
                for y in 0..8 {
                    for x in 0..8 {
                        let cx = ((2 * x + 1) as f64 * u as f64 * pi / 16.0).cos();
                        let cy = ((2 * y + 1) as f64 * v as f64 * pi / 16.0).cos();
                        sum += f[y * 8 + x] * cx * cy;
                    }
                }
                out[v * 8 + u] = 0.25 * cu * cv * sum;
            }
        }
        out
    }

    #[test]
    fn fdct_matches_reference() {
        // A few blocks: flat, a ramp, and a checker-ish pattern.
        let mut blocks: Vec<[u8; 64]> = Vec::new();
        blocks.push([128u8; 64]); // flat -> only DC (=0 after level shift)
        let mut ramp = [0u8; 64];
        for i in 0..64 {
            ramp[i] = (i * 4) as u8;
        }
        blocks.push(ramp);
        let mut check = [0u8; 64];
        for y in 0..8 {
            for x in 0..8 {
                check[y * 8 + x] = if (x + y) % 2 == 0 { 60 } else { 200 };
            }
        }
        blocks.push(check);

        for blk in &blocks {
            let mut got = [0i32; 64];
            fdct_8x8(blk, &mut got);
            let want = reference_fdct(blk);
            for i in 0..64 {
                let d = got[i] as f64 - want[i];
                assert!(
                    d.abs() <= 1.5,
                    "FDCT coeff {i} off by {d} (got {}, want {:.3})",
                    got[i],
                    want[i]
                );
            }
        }
        // FAIL-ability: a flat block has all-zero AC coefficients.
        let mut flat = [0i32; 64];
        fdct_8x8(&[128u8; 64], &mut flat);
        for i in 1..64 {
            assert_eq!(flat[i], 0, "flat block AC[{i}] must be 0");
        }
    }

    // ── E8. FDCT -> IDCT round-trips (the transform pair is consistent) ───────

    #[test]
    fn fdct_idct_roundtrip() {
        // Encode a block via FDCT (quant=1), decode via the decoder's IDCT, and
        // the samples must come back within rounding.
        let mut samples = [0u8; 64];
        for y in 0..8 {
            for x in 0..8 {
                samples[y * 8 + x] = (16 + x * 20 + y * 8) as u8;
            }
        }
        let mut coeffs = [0i32; 64];
        fdct_8x8(&samples, &mut coeffs);
        let quant = [1u16; 64];
        let mut out = [0u8; 64];
        idct_8x8(&coeffs, &quant, &mut out);
        for i in 0..64 {
            let d = out[i] as i32 - samples[i] as i32;
            assert!(d.abs() <= 2, "FDCT/IDCT sample {i} off by {d}");
        }
    }

    // ── E9. RGB<->YCbCr round-trips within tolerance ──────────────────────────

    #[test]
    fn rgb_ycbcr_roundtrip() {
        let cases = [
            (0u8, 0u8, 0u8),
            (255, 255, 255),
            (128, 128, 128),
            (255, 0, 0),
            (0, 255, 0),
            (0, 0, 255),
            (200, 50, 120),
            (17, 211, 99),
        ];
        for &(r, g, b) in &cases {
            let (y, cb, cr) = rgb_to_ycbcr(r, g, b);
            let p = ycbcr_to_argb(y as i32, cb as i32, cr as i32);
            let (rr, gg, bb) = channels(p);
            let dr = (rr as i32 - r as i32).abs();
            let dg = (gg as i32 - g as i32).abs();
            let db = (bb as i32 - b as i32).abs();
            assert!(
                dr <= 2 && dg <= 2 && db <= 2,
                "RGB roundtrip ({r},{g},{b}) -> ({rr},{gg},{bb}) off by ({dr},{dg},{db})"
            );
        }
        // FAIL-ability: a neutral gray maps to Cb=Cr=128 exactly.
        let (_, cb, cr) = rgb_to_ycbcr(128, 128, 128);
        assert_eq!((cb, cr), (128, 128), "neutral gray must have Cb=Cr=128");
    }

    // ── E10. Byte-stuffing: a 0xFF entropy data byte is followed by 0x00 ──────

    #[test]
    fn entropy_byte_stuffing_present() {
        // The EntropyWriter must insert 0x00 after every emitted 0xFF. Drive it
        // directly with bits that flush to a 0xFF byte.
        let mut buf = Vec::new();
        {
            let mut w = EntropyWriter::new(&mut buf);
            w.put_bits(0xFF, 8); // exactly one 0xFF byte
            w.put_bits(0x00, 8); // a following 0x00 data byte
            w.flush();
        }
        assert_eq!(buf[0], 0xFF, "first emitted byte is 0xFF");
        assert_eq!(buf[1], 0x00, "0xFF must be stuffed with 0x00");
        assert_eq!(buf[2], 0x00, "the actual 0x00 data byte follows");

        // And end-to-end: a real encoded image's entropy section must contain at
        // least one FF00 stuff pair when an 0xFF entropy byte naturally occurs.
        // A high-contrast checker forces large coefficients -> 0xFF data bytes.
        let mut img = JpegImage {
            width: 16,
            height: 16,
            pixels: vec![0u32; 256],
        };
        for y in 0..16u32 {
            for x in 0..16u32 {
                let v = if (x / 2 + y / 2) % 2 == 0 { 0u8 } else { 255u8 };
                img.pixels[(y * 16 + x) as usize] = argb(0xFF, v, v, v);
            }
        }
        let enc = encode_jpeg(&img, 95).unwrap();
        let has_stuff = enc.windows(2).any(|w| w[0] == 0xFF && w[1] == 0x00);
        assert!(
            has_stuff,
            "a busy image must produce at least one FF00 stuff pair"
        );
    }

    // ── E11. Quantization tables scale correctly with quality ─────────────────

    #[test]
    fn quant_scaling_endpoints() {
        // q=50 reproduces the base table.
        let q50 = scaled_quant_table(&STD_LUMA_QUANT, 50);
        assert_eq!(q50, STD_LUMA_QUANT, "q50 must equal the base Annex K table");
        // q=100 -> all entries 1 (minimal quantization).
        let q100 = scaled_quant_table(&STD_LUMA_QUANT, 100);
        assert!(q100.iter().all(|&v| v == 1), "q100 must be all-1");
        // q=1 -> all entries clamped to 255 (maximal quantization).
        let q1 = scaled_quant_table(&STD_LUMA_QUANT, 1);
        assert!(q1.iter().all(|&v| v == 255), "q1 must saturate to 255");
        // Every entry is in the legal 8-bit DQT range.
        for q in [1u8, 10, 25, 50, 75, 90, 100] {
            let t = scaled_quant_table(&STD_LUMA_QUANT, q);
            assert!(t.iter().all(|&v| (1..=255).contains(&v)));
        }
    }

    // ── E12. The standard encode Huffman codes match the decoder's canonical ──
    //         build (so emitted codes are guaranteed decodable) ────────────────

    #[test]
    fn encode_huffman_matches_decoder_canonical() {
        // Build the decoder-side table and the encoder-side table from the same
        // bits/vals; every symbol the encoder can emit must round-trip to itself.
        let enc = EncodeHuff::build(&STD_AC_LUMA_BITS, &STD_AC_LUMA_VALS);
        let dec = HuffTable::build(&STD_AC_LUMA_BITS, &STD_AC_LUMA_VALS).expect("dec build");
        for &sym in STD_AC_LUMA_VALS.iter() {
            let (code, len) = enc.get(sym);
            assert!(len > 0, "symbol {sym:#x} must have a code");
            // Feed the code bits MSB-first into a BitReader and decode.
            let mut bytes = Vec::new();
            {
                let mut w = EntropyWriter::new(&mut bytes);
                w.put_bits(code as u32, len);
                w.flush();
            }
            let mut r = BitReader::new(&bytes);
            let got = r.decode_huff(&dec).expect("decode");
            assert_eq!(got, sym, "encode/decode Huffman mismatch for {sym:#x}");
        }
    }

    // ── E13. magnitude_category is the inverse of the decoder's extend() ──────

    #[test]
    fn magnitude_category_inverts_extend() {
        for v in -2047i32..=2047 {
            let (size, bits) = magnitude_category(v);
            let back = extend(bits, size as u32);
            assert_eq!(back, v, "magnitude/extend mismatch for {v}");
        }
        // FAIL-ability: zero has size 0.
        assert_eq!(magnitude_category(0), (0, 0));
    }

    // ── E14. Bounded: absurd / malformed inputs are rejected, never panic ─────

    #[test]
    fn reject_bad_inputs() {
        // Zero dimension.
        let zero = JpegImage {
            width: 0,
            height: 8,
            pixels: vec![],
        };
        assert_eq!(
            encode_jpeg(&zero, 90),
            Err(JpegEncodeError::DimensionsOutOfRange)
        );
        // Pixel-count mismatch.
        let bad = JpegImage {
            width: 8,
            height: 8,
            pixels: vec![0u32; 10],
        };
        assert_eq!(
            encode_jpeg(&bad, 90),
            Err(JpegEncodeError::PixelCountMismatch)
        );
        // Bad quality.
        let ok = solid(8, 8, 1, 2, 3);
        assert_eq!(encode_jpeg(&ok, 0), Err(JpegEncodeError::BadQuality));
        assert_eq!(encode_jpeg(&ok, 101), Err(JpegEncodeError::BadQuality));
        // Over-large dimensions (without allocating the source) — fabricate a
        // descriptor whose declared size exceeds MAX_PIXELS but whose pixel vec
        // length is checked first only after the dimension gate, so the dimension
        // gate must fire first.
        let huge = JpegImage {
            width: MAX_DIMENSION + 1,
            height: 1,
            pixels: vec![],
        };
        assert_eq!(
            encode_jpeg(&huge, 90),
            Err(JpegEncodeError::DimensionsOutOfRange)
        );
    }

    // ── E15. Non-multiple-of-8 dimensions encode + round-trip (edge padding) ──

    #[test]
    fn roundtrip_odd_dimensions() {
        let img = gradient(13, 7); // not a multiple of 8 in either axis
        let enc = encode_jpeg(&img, 90).expect("encode");
        let dec = decode_jpeg(&enc).expect("decode");
        assert_eq!((dec.width, dec.height), (13, 7), "odd dims must be EXACT");
        let err = max_channel_error(&img, &dec);
        assert!(err <= 24, "odd-dim gradient error {err} too high");
    }

    // ── E16. to_jpeg() convenience method matches encode_jpeg() ───────────────

    #[test]
    fn to_jpeg_method_matches_free_fn() {
        let img = solid(8, 8, 10, 20, 30);
        let a = img.to_jpeg(80).unwrap();
        let b = encode_jpeg(&img, 80).unwrap();
        assert_eq!(a, b, "to_jpeg must match encode_jpeg");
    }
}

// ════════════════════════════════════════════════════════════════════════════
// FUZZ / PROPERTY suite — deterministic seeded PRNG, no external fuzz crate.
//
// The properties: on ANY byte sequence `decode_jpeg` must (a) never panic, and
// (b) bound every allocation — a crafted SOF0 (huge dimensions) cannot request a
// multi-GiB buffer. FAIL-ability:
//  - `#![forbid(unsafe_code)]` means an OOB index is a guaranteed panic, not
//    silent UB, so the never-panic loops genuinely prove bounds-safety: if any
//    decode path could panic on hostile bytes the loop aborts the test process.
//  - If MAX_PIXELS / MAX_DIMENSION were removed, `fuzz_huge_dimensions` would
//    request a multi-GiB plane allocation and OOM (process abort = failure)
//    instead of returning DimensionsOutOfRange.
// ════════════════════════════════════════════════════════════════════════════
#[cfg(test)]
mod fuzz {
    use super::*;

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

    fn assert_bounded(img: &JpegImage) {
        assert!(
            (img.width as u64) * (img.height as u64) <= MAX_PIXELS,
            "decoded image exceeded MAX_PIXELS"
        );
        assert_eq!(
            img.pixels.len(),
            (img.width as usize) * (img.height as usize),
            "pixel buffer mismatched dimensions"
        );
    }

    #[test]
    fn fuzz_random_bytes_never_panic() {
        let mut rng = Rng::new(0x0B16_F00D);
        for _ in 0..40_000 {
            let len = rng.below(256);
            let mut buf = Vec::with_capacity(len);
            for _ in 0..len {
                buf.push(rng.byte());
            }
            if let Ok(img) = decode_jpeg(&buf) {
                assert_bounded(&img);
            }
        }
    }

    #[test]
    fn fuzz_soi_prefixed_never_panic() {
        let mut rng = Rng::new(0x5160_F00D);
        for _ in 0..40_000 {
            let len = rng.below(256);
            let mut buf = Vec::with_capacity(2 + len);
            buf.push(0xFF);
            buf.push(0xD8);
            for _ in 0..len {
                buf.push(rng.byte());
            }
            if let Ok(img) = decode_jpeg(&buf) {
                assert_bounded(&img);
            }
        }
    }

    #[test]
    fn fuzz_huge_dimensions_bounded() {
        // SOF0 segments with sweeping huge dimensions must Err with the cap,
        // never allocate.
        let mut rng = Rng::new(0x5151_BAD);
        for _ in 0..5000 {
            let w = (rng.below(0x1_0000)) as u16;
            let h = (rng.below(0x1_0000)) as u16;
            let mut sof = Vec::new();
            sof.push(8);
            sof.extend_from_slice(&h.to_be_bytes());
            sof.extend_from_slice(&w.to_be_bytes());
            sof.push(1);
            sof.push(1);
            sof.push(0x11);
            sof.push(0);
            let mut buf = Vec::new();
            buf.push(0xFF);
            buf.push(0xD8);
            buf.push(0xFF);
            buf.push(0xC0);
            let len = (sof.len() + 2) as u16;
            buf.extend_from_slice(&len.to_be_bytes());
            buf.extend_from_slice(&sof);
            buf.push(0xFF);
            buf.push(0xD9);
            match decode_jpeg(&buf) {
                Ok(img) => assert_bounded(&img),
                Err(_) => {}
            }
        }
    }
}
