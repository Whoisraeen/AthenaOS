//! # RaePNG — a never-panic, `no_std` PNG decoder + encoder (RFC 2083 / W3C PNG).
//!
//! LEGACY_GAMING_CONCEPT.md (§creators / media): a daily driver must let people "show
//! my photos." PNG is the dominant *lossless* image format — photographs exported
//! losslessly, the app icons the launcher and Files app draw, and the bulk of web
//! imagery all ship as PNG. AthenaOS already decodes JPEG/GIF/BMP; this crate is
//! the from-scratch PNG path the Photos viewer and `raeplay` thumbnailer sit on.
//!
//! Output is a flat ARGB8888 `Vec<u32>` (`0xAARRGGBB`) — the AthGFX compositor /
//! Canvas pixel format, **matching the [`rae_bmp`]/[`rae_gif`] decoders** so a
//! gallery, a tab-strip, or a Quick Look preview can blit any of the four image
//! formats through one uniform pixel model.
//!
//! ## What it decodes
//! - The 8-byte PNG signature + the chunk stream (length / type / data / CRC-32).
//!   **Every chunk's CRC-32 is verified.** Unknown *critical* chunks (those whose
//!   type's 5th-bit-of-byte-0 is uppercase) are rejected; unknown *ancillary*
//!   chunks are skipped.
//! - `IHDR`: width, height, bit depth, color type, compression / filter /
//!   interlace method.
//! - **Color types** 0 (grayscale), 2 (truecolor RGB), 3 (palette `PLTE`),
//!   4 (gray+alpha), 6 (truecolor+alpha).
//! - **Bit depths**: 8 and 16 for the non-palette types; 1/2/4/8 for palette;
//!   1/2/4/8/16 for grayscale. (See the per-type matrix in [`decode_png`] docs.)
//! - `PLTE` (palette) + `tRNS` (palette transparency, plus single-color-key
//!   transparency for grayscale and truecolor).
//! - `IDAT` concatenation → zlib inflate via [`rae_deflate::zlib_decompress`] →
//!   per-scanline unfiltering of all five filter types (None / Sub / Up / Average
//!   / Paeth — the Paeth predictor exactly per spec §6.6).
//! - **Adam7 interlace** deinterlace (the 7-pass layout) as well as the
//!   non-interlaced path.
//!
//! ## What it encodes ([`encode_png`] / [`PngImage::to_png`])
//! The "edit AND save/export my photo" path (LEGACY_GAMING_CONCEPT.md §creators /
//! media, criterion #5) — the partner of [`decode_png`], so the Photos app can
//! *write* a PNG, not just read one. Output is bit-depth-8 color type 6 (RGBA),
//! falling back to color type 2 (RGB) when every pixel is fully opaque; scanlines
//! use the adaptive libpng minimum-sum filter heuristic across all five filter
//! types; `IDAT` is zlib via [`rae_deflate::zlib_compress`]. The encode→decode
//! round-trip is **pixel-exact / lossless** and is the primary proof.
//!
//! ## Hostile-input posture (CLAUDE: decoders are the #1 RCE surface)
//! Every PNG byte is treated as attacker-controlled. There is **no
//! `unwrap`/`expect`/`panic`/raw-index-panic path** reachable from [`decode_png`]:
//! bad signatures, truncated chunks, a wrong CRC, a missing `PLTE`, an illegal
//! bit-depth/color-type pair, oversized dimensions, and a corrupt zlib/IDAT stream
//! all return `Err(PngError)`. Memory is bounded up front ([`MAX_DIMENSION`],
//! [`MAX_PIXELS`]) so a crafted `IHDR` cannot request a multi-gigabyte allocation.
//! The host KAT suite at the bottom of this file is the primary proof
//! (`cargo test -p rae_png`).

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

use alloc::vec;
use alloc::vec::Vec;

/// Bound on either image axis. PNG dimensions are 31-bit but no real image is
/// anywhere near this; the ceiling keeps a single canvas under [`MAX_PIXELS`].
pub const MAX_DIMENSION: u32 = 1 << 16; // 65_536
/// Bound on total pixel count (width * height). ~67M px = 256 MiB at 4 B/px
/// ARGB. A crafted `IHDR` claiming a huge canvas is rejected before allocation.
pub const MAX_PIXELS: u64 = 64 * 1024 * 1024;

/// PNG decode error. Every variant is a *handled* path — nothing here panics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PngError {
    /// The first 8 bytes are not the PNG signature.
    BadSignature,
    /// A chunk header/body ran past the end of the buffer.
    Truncated,
    /// A chunk's stored CRC-32 did not match the computed CRC-32.
    BadCrc,
    /// The first chunk was not `IHDR`, or `IHDR`'s length was wrong.
    BadHeader,
    /// Width/height was zero or exceeded the memory bound.
    DimensionsOutOfRange,
    /// An illegal bit-depth / color-type combination, or an unsupported value.
    BadColorType,
    /// A `PLTE`/`tRNS` chunk was malformed, missing, or an index was out of range.
    BadPalette,
    /// The compression / filter / interlace method byte was not a value the spec
    /// (or this decoder) supports.
    Unsupported,
    /// An unknown chunk whose type is *critical* (must-understand) was found.
    UnknownCriticalChunk,
    /// The concatenated IDAT zlib stream failed to inflate / was corrupt.
    InflateFailed,
    /// The inflated image data was the wrong size for the declared image.
    BadImageData,
    /// A scanline declared a filter type outside 0..=4.
    BadFilter,
    /// No `IDAT` data was present.
    NoImageData,
}

/// A decoded image: a flat ARGB8888 buffer plus dimensions.
///
/// `pixels.len() == (width * height) as usize`. Each `u32` is `0xAARRGGBB` —
/// identical to [`rae_bmp::BmpImage`] / a `rae_gif` frame so callers consume all
/// of AthenaOS's still-image decoders through one pixel model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PngImage {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u32>,
}

impl PngImage {
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

/// The 8-byte PNG signature (§5.2).
const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

// ─── CRC-32 (PNG uses the same IEEE polynomial as zlib; reuse rae_deflate) ───
//
// rae_deflate::crc32 is the exact algorithm PNG specifies (§5.5): reflected,
// init 0xFFFFFFFF, final XOR 0xFFFFFFFF, computed over the chunk's TYPE + DATA.

// ─── Color types (IHDR byte 9) ──────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ColorType {
    Grayscale,      // 0: 1/2/4/8/16-bit gray
    Truecolor,      // 2: 8/16-bit RGB
    Palette,        // 3: 1/2/4/8-bit indexed
    GrayscaleAlpha, // 4: 8/16-bit gray + alpha
    TruecolorAlpha, // 6: 8/16-bit RGBA
}

impl ColorType {
    fn from_byte(b: u8) -> Result<Self, PngError> {
        Ok(match b {
            0 => ColorType::Grayscale,
            2 => ColorType::Truecolor,
            3 => ColorType::Palette,
            4 => ColorType::GrayscaleAlpha,
            6 => ColorType::TruecolorAlpha,
            _ => return Err(PngError::BadColorType),
        })
    }

    /// Channels per pixel in the raw (post-unfilter) sample stream.
    fn channels(self) -> usize {
        match self {
            ColorType::Grayscale | ColorType::Palette => 1,
            ColorType::GrayscaleAlpha => 2,
            ColorType::Truecolor => 3,
            ColorType::TruecolorAlpha => 4,
        }
    }

    /// Validate the bit-depth/color-type pair per PNG spec Table 11.1.
    fn validate_depth(self, depth: u8) -> Result<(), PngError> {
        let ok = match self {
            ColorType::Grayscale => matches!(depth, 1 | 2 | 4 | 8 | 16),
            ColorType::Palette => matches!(depth, 1 | 2 | 4 | 8),
            ColorType::Truecolor | ColorType::GrayscaleAlpha | ColorType::TruecolorAlpha => {
                matches!(depth, 8 | 16)
            }
        };
        if ok {
            Ok(())
        } else {
            Err(PngError::BadColorType)
        }
    }
}

struct Ihdr {
    width: u32,
    height: u32,
    bit_depth: u8,
    color_type: ColorType,
    interlace: bool,
}

// ─── Chunk cursor (bounds-checked, never panics) ─────────────────────────────

fn read_u32_be(buf: &[u8], off: usize) -> Option<u32> {
    let b = buf.get(off..off + 4)?;
    Some(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}

/// Decode a PNG byte stream into an ARGB8888 [`PngImage`].
///
/// Hostile-input safe: returns `Err` (never panics) on any malformed input.
///
/// ## Supported color-type / bit-depth matrix (honest)
/// | Color type | Bit depths supported |
/// |---|---|
/// | 0 grayscale | 1, 2, 4, 8, 16 |
/// | 2 truecolor (RGB) | 8, 16 |
/// | 3 palette | 1, 2, 4, 8 |
/// | 4 gray+alpha | 8, 16 |
/// | 6 truecolor+alpha (RGBA) | 8, 16 |
///
/// 16-bit samples are down-scaled to 8 bits per channel (the high byte) for the
/// ARGB output. Interlacing: **Adam7** and non-interlaced are both supported.
/// `tRNS` is honored for palette, grayscale (single key), and truecolor (key).
pub fn decode_png(data: &[u8]) -> Result<PngImage, PngError> {
    // ── Signature ──────────────────────────────────────────────────────────
    if data.len() < 8 || data[0..8] != PNG_SIGNATURE {
        return Err(PngError::BadSignature);
    }

    let mut pos = 8usize;
    let mut ihdr: Option<Ihdr> = None;
    let mut palette: Vec<(u8, u8, u8)> = Vec::new();
    let mut trns: Vec<u8> = Vec::new(); // raw tRNS bytes; meaning depends on color type
    let mut have_trns = false;
    let mut idat: Vec<u8> = Vec::new();
    let mut seen_iend = false;

    // ── Chunk loop ───────────────────────────────────────────────────────────
    while pos + 8 <= data.len() {
        let length = read_u32_be(data, pos).ok_or(PngError::Truncated)? as usize;
        let type_bytes = data.get(pos + 4..pos + 8).ok_or(PngError::Truncated)?;
        let mut ctype = [0u8; 4];
        ctype.copy_from_slice(type_bytes);

        // length + 12 bytes (4 len + 4 type + 4 crc) must fit.
        let data_start = pos + 8;
        let data_end = data_start.checked_add(length).ok_or(PngError::Truncated)?;
        let crc_end = data_end.checked_add(4).ok_or(PngError::Truncated)?;
        if crc_end > data.len() {
            return Err(PngError::Truncated);
        }
        let chunk_data = &data[data_start..data_end];

        // CRC-32 covers the type + data (not the length field).
        let stored_crc = read_u32_be(data, data_end).ok_or(PngError::Truncated)?;
        let computed = rae_deflate::crc32(&data[pos + 4..data_end]);
        if stored_crc != computed {
            return Err(PngError::BadCrc);
        }

        match &ctype {
            b"IHDR" => {
                if ihdr.is_some() {
                    return Err(PngError::BadHeader);
                }
                ihdr = Some(parse_ihdr(chunk_data)?);
            }
            b"PLTE" => {
                if ihdr.is_none() {
                    return Err(PngError::BadHeader);
                }
                if length == 0 || length % 3 != 0 || length > 256 * 3 {
                    return Err(PngError::BadPalette);
                }
                palette = chunk_data
                    .chunks_exact(3)
                    .map(|c| (c[0], c[1], c[2]))
                    .collect();
            }
            b"tRNS" => {
                if ihdr.is_none() {
                    return Err(PngError::BadHeader);
                }
                trns = chunk_data.to_vec();
                have_trns = true;
            }
            b"IDAT" => {
                if ihdr.is_none() {
                    return Err(PngError::BadHeader);
                }
                idat.extend_from_slice(chunk_data);
            }
            b"IEND" => {
                seen_iend = true;
                break;
            }
            _ => {
                // Unknown chunk. The 5th bit of the first type byte (0x20) is the
                // ancillary bit: set (lowercase) = ancillary (safe to skip);
                // clear (uppercase) = critical (must understand → reject).
                let ancillary = ctype[0] & 0x20 != 0;
                if !ancillary {
                    return Err(PngError::UnknownCriticalChunk);
                }
                // Ancillary: ignore.
            }
        }

        pos = crc_end;
    }

    let ihdr = ihdr.ok_or(PngError::BadHeader)?;
    if !seen_iend {
        // A stream that ended without IEND is tolerated only if we still got data;
        // but a clean reject keeps the parser strict against truncation.
        return Err(PngError::Truncated);
    }
    if idat.is_empty() {
        return Err(PngError::NoImageData);
    }
    if matches!(ihdr.color_type, ColorType::Palette) && palette.is_empty() {
        return Err(PngError::BadPalette);
    }

    // ── Inflate the zlib-wrapped IDAT stream (REUSE rae_deflate) ─────────────
    let raw = rae_deflate::zlib_decompress(&idat).map_err(|_| PngError::InflateFailed)?;

    // ── Unfilter + deinterlace + convert to ARGB ─────────────────────────────
    let pixels = if ihdr.interlace {
        decode_adam7(&ihdr, &raw, &palette, &trns, have_trns)?
    } else {
        decode_progressive(&ihdr, &raw, &palette, &trns, have_trns)?
    };

    Ok(PngImage {
        width: ihdr.width,
        height: ihdr.height,
        pixels,
    })
}

/// Parse the 13-byte IHDR chunk body.
fn parse_ihdr(d: &[u8]) -> Result<Ihdr, PngError> {
    if d.len() != 13 {
        return Err(PngError::BadHeader);
    }
    let width = u32::from_be_bytes([d[0], d[1], d[2], d[3]]);
    let height = u32::from_be_bytes([d[4], d[5], d[6], d[7]]);
    let bit_depth = d[8];
    let color_type = ColorType::from_byte(d[9])?;
    let compression = d[10];
    let filter = d[11];
    let interlace = d[12];

    if width == 0 || height == 0 {
        return Err(PngError::DimensionsOutOfRange);
    }
    if width > MAX_DIMENSION || height > MAX_DIMENSION {
        return Err(PngError::DimensionsOutOfRange);
    }
    if (width as u64) * (height as u64) > MAX_PIXELS {
        return Err(PngError::DimensionsOutOfRange);
    }

    color_type.validate_depth(bit_depth)?;

    // Only compression method 0 (DEFLATE) and filter method 0 are defined.
    if compression != 0 || filter != 0 {
        return Err(PngError::Unsupported);
    }
    let interlace = match interlace {
        0 => false,
        1 => true,
        _ => return Err(PngError::Unsupported),
    };

    Ok(Ihdr {
        width,
        height,
        bit_depth,
        color_type,
        interlace,
    })
}

/// Bytes-per-pixel rounded up (the "bpp" used as the Sub/Average/Paeth offset).
/// PNG defines it as ceil(bits-per-pixel / 8), minimum 1.
fn filter_bpp(ihdr: &Ihdr) -> usize {
    let bits = ihdr.channels_bits();
    ((bits + 7) / 8).max(1)
}

impl Ihdr {
    /// Total bits per pixel = channels * bit_depth.
    fn channels_bits(&self) -> usize {
        self.color_type.channels() * self.bit_depth as usize
    }
}

/// The Paeth predictor (PNG spec §6.6). `a`=left, `b`=above, `c`=upper-left.
#[inline]
fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let p = a as i32 + b as i32 - c as i32;
    let pa = (p - a as i32).abs();
    let pb = (p - b as i32).abs();
    let pc = (p - c as i32).abs();
    if pa <= pb && pa <= pc {
        a
    } else if pb <= pc {
        b
    } else {
        c
    }
}

/// Unfilter one image (or one Adam7 pass) in place. `raw` is the filtered byte
/// stream: each scanline is `1 + stride` bytes (a filter-type byte + the row).
/// Returns the unfiltered sample bytes (no filter bytes), `height * stride` long.
fn unfilter(
    raw: &[u8],
    width: usize,
    height: usize,
    bpp: usize,
    stride: usize,
) -> Result<Vec<u8>, PngError> {
    if width == 0 || height == 0 {
        return Ok(Vec::new());
    }
    let filtered_len = (stride + 1)
        .checked_mul(height)
        .ok_or(PngError::BadImageData)?;
    if raw.len() < filtered_len {
        return Err(PngError::BadImageData);
    }

    let mut out = vec![0u8; stride * height];
    for y in 0..height {
        let in_off = y * (stride + 1);
        let filter = raw[in_off];
        let line = &raw[in_off + 1..in_off + 1 + stride];
        let cur_off = y * stride;
        let prev_off = if y > 0 { (y - 1) * stride } else { 0 };
        for i in 0..stride {
            let x = line[i];
            let a = if i >= bpp { out[cur_off + i - bpp] } else { 0 };
            let b = if y > 0 { out[prev_off + i] } else { 0 };
            let c = if y > 0 && i >= bpp {
                out[prev_off + i - bpp]
            } else {
                0
            };
            let val = match filter {
                0 => x,                                                 // None
                1 => x.wrapping_add(a),                                 // Sub
                2 => x.wrapping_add(b),                                 // Up
                3 => x.wrapping_add(((a as u16 + b as u16) / 2) as u8), // Average
                4 => x.wrapping_add(paeth(a, b, c)),                    // Paeth
                _ => return Err(PngError::BadFilter),
            };
            out[cur_off + i] = val;
        }
    }
    Ok(out)
}

/// Read sample `i` (a single channel value) from a packed scanline of samples,
/// scaled up to an 8-bit value. `bit_depth` is 1/2/4/8/16.
///
/// For palette images the *index* (not a scaled value) is needed, so callers use
/// [`read_index`] instead; this is for direct color/gray sample channels.
#[inline]
fn read_sample_scaled(row: &[u8], sample_idx: usize, bit_depth: u8) -> u8 {
    match bit_depth {
        16 => {
            // Take the high byte (down-scale 16→8).
            let off = sample_idx * 2;
            *row.get(off).unwrap_or(&0)
        }
        8 => *row.get(sample_idx).unwrap_or(&0),
        _ => {
            // 1/2/4-bit: extract the sub-byte sample then scale to 0..255.
            let bd = bit_depth as usize;
            let bit_off = sample_idx * bd;
            let byte = *row.get(bit_off / 8).unwrap_or(&0);
            let shift = 8 - bd - (bit_off % 8);
            let mask = (1u16 << bd) - 1;
            let raw = ((byte as u16 >> shift) & mask) as u32;
            // Scale to 8-bit: replicate, e.g. 1-bit 1 -> 255, 4-bit n -> n*17.
            let maxval = (1u32 << bd) - 1;
            ((raw * 255 + maxval / 2) / maxval) as u8
        }
    }
}

/// Read the raw palette index of sample `i` for a palette image (no scaling).
#[inline]
fn read_index(row: &[u8], sample_idx: usize, bit_depth: u8) -> u8 {
    match bit_depth {
        8 => *row.get(sample_idx).unwrap_or(&0),
        _ => {
            let bd = bit_depth as usize;
            let bit_off = sample_idx * bd;
            let byte = *row.get(bit_off / 8).unwrap_or(&0);
            let shift = 8 - bd - (bit_off % 8);
            let mask = (1u16 << bd) - 1;
            ((byte as u16 >> shift) & mask) as u8
        }
    }
}

/// Read a 16-bit channel value (the full two bytes) for tRNS key comparison.
#[inline]
fn read_sample_full(row: &[u8], sample_idx: usize, bit_depth: u8) -> u16 {
    if bit_depth == 16 {
        let off = sample_idx * 2;
        let hi = *row.get(off).unwrap_or(&0) as u16;
        let lo = *row.get(off + 1).unwrap_or(&0) as u16;
        (hi << 8) | lo
    } else {
        read_index(row, sample_idx, bit_depth) as u16
    }
}

/// Convert one unfiltered scanline (`stride` bytes, `width` pixels) into ARGB and
/// write it into `out` at row `dst_y` of a `canvas_w`-wide canvas, honoring an
/// optional Adam7 column mapping (`map_x`: pass column -> canvas column).
#[allow(clippy::too_many_arguments)]
fn row_to_argb(
    row: &[u8],
    width: usize,
    ihdr: &Ihdr,
    palette: &[(u8, u8, u8)],
    trns: &[u8],
    have_trns: bool,
    out: &mut [u32],
    dst_y: usize,
    canvas_w: usize,
    col_start: usize,
    col_step: usize,
) -> Result<(), PngError> {
    let bd = ihdr.bit_depth;

    for x in 0..width {
        let cx = col_start + x * col_step;
        if cx >= canvas_w {
            break;
        }
        let dst = dst_y * canvas_w + cx;
        if dst >= out.len() {
            break;
        }

        let pixel = match ihdr.color_type {
            ColorType::Grayscale => {
                let g = read_sample_scaled(row, x, bd);
                let mut a = 0xFFu8;
                if have_trns && trns.len() >= 2 {
                    let key = ((trns[0] as u16) << 8) | trns[1] as u16;
                    if read_sample_full(row, x, bd) == key {
                        a = 0;
                    }
                }
                argb(a, g, g, g)
            }
            ColorType::GrayscaleAlpha => {
                let g = read_sample_scaled(row, x * 2, bd);
                let a = read_sample_scaled(row, x * 2 + 1, bd);
                argb(a, g, g, g)
            }
            ColorType::Truecolor => {
                let r = read_sample_scaled(row, x * 3, bd);
                let g = read_sample_scaled(row, x * 3 + 1, bd);
                let b = read_sample_scaled(row, x * 3 + 2, bd);
                let mut a = 0xFFu8;
                if have_trns && trns.len() >= 6 {
                    let kr = ((trns[0] as u16) << 8) | trns[1] as u16;
                    let kg = ((trns[2] as u16) << 8) | trns[3] as u16;
                    let kb = ((trns[4] as u16) << 8) | trns[5] as u16;
                    if read_sample_full(row, x * 3, bd) == kr
                        && read_sample_full(row, x * 3 + 1, bd) == kg
                        && read_sample_full(row, x * 3 + 2, bd) == kb
                    {
                        a = 0;
                    }
                }
                argb(a, r, g, b)
            }
            ColorType::TruecolorAlpha => {
                let r = read_sample_scaled(row, x * 4, bd);
                let g = read_sample_scaled(row, x * 4 + 1, bd);
                let b = read_sample_scaled(row, x * 4 + 2, bd);
                let a = read_sample_scaled(row, x * 4 + 3, bd);
                argb(a, r, g, b)
            }
            ColorType::Palette => {
                let idx = read_index(row, x, bd) as usize;
                let (r, g, b) = *palette.get(idx).ok_or(PngError::BadPalette)?;
                // tRNS for palette is a per-index alpha table; entries past its
                // length are fully opaque.
                let a = if have_trns {
                    *trns.get(idx).unwrap_or(&0xFF)
                } else {
                    0xFF
                };
                argb(a, r, g, b)
            }
        };
        out[dst] = pixel;
    }
    Ok(())
}

/// Scanline stride (packed sample bytes per row) for a given pixel width.
fn row_stride(ihdr: &Ihdr, width: usize) -> usize {
    let bits = width * ihdr.channels_bits();
    (bits + 7) / 8
}

/// Decode a non-interlaced image.
fn decode_progressive(
    ihdr: &Ihdr,
    raw: &[u8],
    palette: &[(u8, u8, u8)],
    trns: &[u8],
    have_trns: bool,
) -> Result<Vec<u32>, PngError> {
    let w = ihdr.width as usize;
    let h = ihdr.height as usize;
    let stride = row_stride(ihdr, w);
    let bpp = filter_bpp(ihdr);

    let samples = unfilter(raw, w, h, bpp, stride)?;

    let mut out = vec![0u32; w * h];
    for y in 0..h {
        let row = &samples[y * stride..y * stride + stride];
        row_to_argb(row, w, ihdr, palette, trns, have_trns, &mut out, y, w, 0, 1)?;
    }
    Ok(out)
}

// ─── Adam7 interlace ─────────────────────────────────────────────────────────
//
// The 7 passes' (x_start, y_start, x_step, y_step) per PNG spec §A. Each pass is
// an independent sub-image with its own filtered scanlines.
const ADAM7_X_START: [usize; 7] = [0, 4, 0, 2, 0, 1, 0];
const ADAM7_Y_START: [usize; 7] = [0, 0, 4, 0, 2, 0, 1];
const ADAM7_X_STEP: [usize; 7] = [8, 8, 4, 4, 2, 2, 1];
const ADAM7_Y_STEP: [usize; 7] = [8, 8, 8, 4, 4, 2, 2];

/// Number of pixels in a pass dimension given the full size, start, and step.
fn pass_count(full: usize, start: usize, step: usize) -> usize {
    if full <= start {
        0
    } else {
        (full - start + step - 1) / step
    }
}

/// Decode an Adam7-interlaced image.
fn decode_adam7(
    ihdr: &Ihdr,
    raw: &[u8],
    palette: &[(u8, u8, u8)],
    trns: &[u8],
    have_trns: bool,
) -> Result<Vec<u32>, PngError> {
    let w = ihdr.width as usize;
    let h = ihdr.height as usize;
    let bpp = filter_bpp(ihdr);
    let mut out = vec![0u32; w * h];

    let mut offset = 0usize;
    for pass in 0..7 {
        let pw = pass_count(w, ADAM7_X_START[pass], ADAM7_X_STEP[pass]);
        let ph = pass_count(h, ADAM7_Y_START[pass], ADAM7_Y_STEP[pass]);
        if pw == 0 || ph == 0 {
            continue;
        }
        let stride = row_stride(ihdr, pw);
        let pass_filtered_len = (stride + 1).checked_mul(ph).ok_or(PngError::BadImageData)?;
        let end = offset
            .checked_add(pass_filtered_len)
            .ok_or(PngError::BadImageData)?;
        if end > raw.len() {
            return Err(PngError::BadImageData);
        }
        let pass_raw = &raw[offset..end];
        offset = end;

        let samples = unfilter(pass_raw, pw, ph, bpp, stride)?;

        for py in 0..ph {
            let row = &samples[py * stride..py * stride + stride];
            let dst_y = ADAM7_Y_START[pass] + py * ADAM7_Y_STEP[pass];
            row_to_argb(
                row,
                pw,
                ihdr,
                palette,
                trns,
                have_trns,
                &mut out,
                dst_y,
                w,
                ADAM7_X_START[pass],
                ADAM7_X_STEP[pass],
            )?;
        }
    }
    Ok(out)
}

// ════════════════════════════════════════════════════════════════════════════
// PNG ENCODER — the "edit AND save/export my photo" path (LEGACY_GAMING_CONCEPT.md
// §creators / media: criterion #5). The decoder above is the verification lever:
// every encoder output is a valid PNG the decoder reads back pixel-exact.
//
// Output is always color type 6 (RGBA truecolor+alpha) at bit depth 8 — the
// simplest universal target that round-trips the ARGB8888 model losslessly —
// UNLESS every pixel is fully opaque (alpha == 0xFF), in which case color type 2
// (RGB truecolor) is emitted as a size optimization (3 bytes/px instead of 4).
// Both forms round-trip to the same ARGB pixels (opaque RGB decodes with
// alpha = 0xFF, exactly matching the original 0xFF-alpha input).
//
// Scanline filtering uses the standard libpng adaptive heuristic: for each row
// every defined filter (None / Sub / Up / Average / Paeth) is trial-applied and
// the one with the minimum sum of absolute (signed) byte deviations is chosen,
// which the decoder's unfilter() recovers exactly regardless of choice. The IDAT
// zlib stream is produced by rae_deflate::zlib_compress (NOT reimplemented), and
// each chunk's CRC-32 uses the decoder's rae_deflate::crc32 (no second impl).
// Memory is bounded up front via the same MAX_DIMENSION / MAX_PIXELS ceilings as
// the decoder, so an absurd PngImage can never request a huge allocation.
// ════════════════════════════════════════════════════════════════════════════

/// PNG encode error. Mirrors the decoder's hostile-input posture: nothing here
/// panics; bad inputs are returned as `Err`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PngEncodeError {
    /// Width or height was zero, or exceeded [`MAX_DIMENSION`] / [`MAX_PIXELS`].
    DimensionsOutOfRange,
    /// `pixels.len()` did not equal `width * height`.
    PixelCountMismatch,
}

impl PngImage {
    /// Encode this image to a valid PNG byte stream. See [`encode_png`].
    pub fn to_png(&self) -> Result<Vec<u8>, PngEncodeError> {
        encode_png(self)
    }
}

/// One PNG filter applied to a single scanline, given the previous (already
/// unfiltered) row and the per-pixel byte offset `bpp`. Returns the filtered
/// bytes (without the leading filter-type byte). `prev` is empty for row 0.
fn filter_scanline(filter: u8, cur: &[u8], prev: &[u8], bpp: usize) -> Vec<u8> {
    let n = cur.len();
    let mut out = vec![0u8; n];
    for i in 0..n {
        let a = if i >= bpp { cur[i - bpp] } else { 0 };
        let b = if !prev.is_empty() { prev[i] } else { 0 };
        let c = if !prev.is_empty() && i >= bpp {
            prev[i - bpp]
        } else {
            0
        };
        out[i] = match filter {
            0 => cur[i],                                                 // None
            1 => cur[i].wrapping_sub(a),                                 // Sub
            2 => cur[i].wrapping_sub(b),                                 // Up
            3 => cur[i].wrapping_sub(((a as u16 + b as u16) / 2) as u8), // Average
            4 => cur[i].wrapping_sub(paeth(a, b, c)),                    // Paeth
            _ => cur[i],
        };
    }
    out
}

/// libpng's minimum-sum-of-absolute-differences heuristic: a filtered row is
/// "cheaper" when its bytes are near 0 (treated as signed: 200 counts as 56).
fn filter_cost(line: &[u8]) -> u64 {
    let mut sum = 0u64;
    for &b in line {
        let v = b as i8 as i32;
        sum += v.unsigned_abs() as u64;
    }
    sum
}

/// Pick the lowest-cost filter for one scanline and return
/// `(filter_type, filtered_bytes)`.
fn choose_filter(cur: &[u8], prev: &[u8], bpp: usize) -> (u8, Vec<u8>) {
    let mut best_filter = 0u8;
    let mut best_line = filter_scanline(0, cur, prev, bpp);
    let mut best_cost = filter_cost(&best_line);
    for filter in 1u8..=4 {
        let line = filter_scanline(filter, cur, prev, bpp);
        let cost = filter_cost(&line);
        if cost < best_cost {
            best_cost = cost;
            best_filter = filter;
            best_line = line;
        }
    }
    (best_filter, best_line)
}

/// Build an `IHDR` body, then a chunk; helper shared by [`encode_png`].
fn encode_chunk(ty: &[u8; 4], data: &[u8], out: &mut Vec<u8>) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(ty);
    out.extend_from_slice(data);
    // CRC-32 over the type + data (REUSE the decoder's rae_deflate::crc32).
    let crc = {
        let mut crc_input = Vec::with_capacity(4 + data.len());
        crc_input.extend_from_slice(ty);
        crc_input.extend_from_slice(data);
        rae_deflate::crc32(&crc_input)
    };
    out.extend_from_slice(&crc.to_be_bytes());
}

/// Encode an ARGB8888 [`PngImage`] into a valid PNG byte stream.
///
/// LEGACY_GAMING_CONCEPT.md §creators / media (criterion #5: "edit AND save/export my
/// photo"). This completes the image stack to decode + encode: the partner of
/// [`decode_png`], and verified against it (a round-trip is pixel-exact and
/// lossless).
///
/// ## Output format
/// - 8-byte signature, `IHDR`, one `IDAT`, `IEND` — each chunk length + type +
///   CRC-32 correct.
/// - **Bit depth 8.** Color type **6 (RGBA)** in general; color type **2 (RGB)**
///   when every pixel is fully opaque (`alpha == 0xFF`) as a size optimization
///   (both decode back to the identical ARGB pixels).
/// - Scanlines use the **adaptive libpng minimum-sum heuristic** across all five
///   filter types; the `IDAT` body is zlib via [`rae_deflate::zlib_compress`].
///
/// ## Errors
/// - [`PngEncodeError::DimensionsOutOfRange`] — zero or over the
///   [`MAX_DIMENSION`] / [`MAX_PIXELS`] bound (checked before allocating).
/// - [`PngEncodeError::PixelCountMismatch`] — `pixels.len() != width * height`.
pub fn encode_png(img: &PngImage) -> Result<Vec<u8>, PngEncodeError> {
    // ── Bound dimensions BEFORE allocating (hostile/garbage PngImage safe) ───
    if img.width == 0 || img.height == 0 {
        return Err(PngEncodeError::DimensionsOutOfRange);
    }
    if img.width > MAX_DIMENSION || img.height > MAX_DIMENSION {
        return Err(PngEncodeError::DimensionsOutOfRange);
    }
    let total = (img.width as u64) * (img.height as u64);
    if total > MAX_PIXELS {
        return Err(PngEncodeError::DimensionsOutOfRange);
    }
    if img.pixels.len() as u64 != total {
        return Err(PngEncodeError::PixelCountMismatch);
    }

    let w = img.width as usize;
    let h = img.height as usize;

    // Optimization: emit RGB (color type 2) when every pixel is fully opaque.
    let all_opaque = img.pixels.iter().all(|&p| (p >> 24) as u8 == 0xFF);
    let (color_type, bpp) = if all_opaque {
        (2u8, 3usize)
    } else {
        (6u8, 4usize)
    };
    let stride = w * bpp;

    // ── Build + adaptively filter the scanline stream ────────────────────────
    // Filtered stream = per row: [filter byte][filtered sample bytes].
    let mut filtered = Vec::with_capacity((stride + 1) * h);
    let mut cur = vec![0u8; stride]; // raw (unfiltered) sample bytes for this row
    let mut prev = Vec::new();
    for y in 0..h {
        let row_base = y * w;
        for x in 0..w {
            let p = img.pixels[row_base + x];
            let a = (p >> 24) as u8;
            let r = (p >> 16) as u8;
            let g = (p >> 8) as u8;
            let b = p as u8;
            let o = x * bpp;
            // ARGB8888 (0xAARRGGBB) → PNG byte order R, G, B[, A].
            cur[o] = r;
            cur[o + 1] = g;
            cur[o + 2] = b;
            if bpp == 4 {
                cur[o + 3] = a;
            }
        }
        let (ftype, line) = choose_filter(&cur, &prev, bpp);
        filtered.push(ftype);
        filtered.extend_from_slice(&line);
        // `cur` becomes next row's `prev`; swap buffers to avoid re-allocating.
        core::mem::swap(&mut prev, &mut cur);
        if cur.len() != stride {
            cur = vec![0u8; stride];
        }
    }

    // ── zlib-compress the filtered stream (REUSE rae_deflate) ────────────────
    let idat = rae_deflate::zlib_compress(&filtered);

    // ── Assemble the PNG ─────────────────────────────────────────────────────
    let mut out = Vec::with_capacity(8 + 25 + idat.len() + 12 + 12);
    out.extend_from_slice(&PNG_SIGNATURE);

    let mut ihdr = [0u8; 13];
    ihdr[0..4].copy_from_slice(&img.width.to_be_bytes());
    ihdr[4..8].copy_from_slice(&img.height.to_be_bytes());
    ihdr[8] = 8; // bit depth
    ihdr[9] = color_type; // 2 (RGB) or 6 (RGBA)
    ihdr[10] = 0; // compression method (DEFLATE)
    ihdr[11] = 0; // filter method (adaptive, the only defined method)
    ihdr[12] = 0; // interlace: none
    encode_chunk(b"IHDR", &ihdr, &mut out);
    encode_chunk(b"IDAT", &idat, &mut out);
    encode_chunk(b"IEND", &[], &mut out);

    Ok(out)
}

// ════════════════════════════════════════════════════════════════════════════
// Host KAT suite — `cargo test -p rae_png`. FAIL-able by construction.
//
// Under `#[cfg(test)]` the crate compiles as `std` (the `no_std` attribute is
// `cfg_attr(not(test), ...)`), so `Vec`/`vec!` are in scope via the default
// prelude — no `extern crate std` / `use std::` (the architecture gate bans
// those std-ism lines). PNG fixtures are constructed in-test from real chunks
// (IHDR/PLTE/tRNS/IDAT/IEND), with the IDAT body built as a zlib *stored* block
// (rae_deflate-independent on the encode side) so each pixel assert is concrete.
// ════════════════════════════════════════════════════════════════════════════
#[cfg(test)]
mod tests {
    use super::*;

    // ── PNG fixture builders ─────────────────────────────────────────────────

    /// Wrap a raw DEFLATE-able payload as a zlib *stored* stream: a 2-byte zlib
    /// header (CM=8, %31-valid), one final stored DEFLATE block, then a big-endian
    /// Adler-32 of the *uncompressed* payload. The decoder's
    /// `rae_deflate::zlib_decompress` recovers `payload` exactly.
    fn zlib_stored(payload: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        // zlib header: CMF=0x78, FLG chosen so (CMF<<8|FLG) % 31 == 0.
        let cmf: u16 = 0x78;
        let mut flg: u16 = 0;
        let rem = ((cmf << 8) | flg) % 31;
        if rem != 0 {
            flg += 31 - rem;
        }
        out.push(cmf as u8);
        out.push(flg as u8);

        // DEFLATE stored block: BFINAL=1, BTYPE=00 → first byte 0x01, byte-aligned,
        // then LEN (LE) + ~LEN (LE) + raw bytes. Payload must be <= 65535 here.
        out.push(0x01);
        let len = payload.len() as u16;
        let nlen = !len;
        out.push((len & 0xFF) as u8);
        out.push((len >> 8) as u8);
        out.push((nlen & 0xFF) as u8);
        out.push((nlen >> 8) as u8);
        out.extend_from_slice(payload);

        // Adler-32 trailer (big-endian).
        out.extend_from_slice(&rae_deflate::adler32(payload).to_be_bytes());
        out
    }

    fn chunk(ty: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        out.extend_from_slice(ty);
        out.extend_from_slice(data);
        // CRC over type + data.
        let mut crc_input = Vec::new();
        crc_input.extend_from_slice(ty);
        crc_input.extend_from_slice(data);
        out.extend_from_slice(&rae_deflate::crc32(&crc_input).to_be_bytes());
        out
    }

    /// Build a complete PNG from IHDR fields + an already-zlib'd IDAT body, with
    /// optional PLTE and tRNS chunks inserted in spec order.
    #[allow(clippy::too_many_arguments)]
    fn build_png(
        width: u32,
        height: u32,
        bit_depth: u8,
        color_type: u8,
        interlace: u8,
        plte: Option<&[(u8, u8, u8)]>,
        trns: Option<&[u8]>,
        idat_zlib: &[u8],
    ) -> Vec<u8> {
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&width.to_be_bytes());
        ihdr.extend_from_slice(&height.to_be_bytes());
        ihdr.push(bit_depth);
        ihdr.push(color_type);
        ihdr.push(0); // compression
        ihdr.push(0); // filter
        ihdr.push(interlace);

        let mut out = Vec::new();
        out.extend_from_slice(&PNG_SIGNATURE);
        out.extend_from_slice(&chunk(b"IHDR", &ihdr));
        if let Some(p) = plte {
            let mut pb = Vec::new();
            for &(r, g, b) in p {
                pb.push(r);
                pb.push(g);
                pb.push(b);
            }
            out.extend_from_slice(&chunk(b"PLTE", &pb));
        }
        if let Some(t) = trns {
            out.extend_from_slice(&chunk(b"tRNS", t));
        }
        out.extend_from_slice(&chunk(b"IDAT", idat_zlib));
        out.extend_from_slice(&chunk(b"IEND", &[]));
        out
    }

    /// Build the filtered scanline stream for a non-interlaced image: a filter
    /// byte per row followed by the row's raw sample bytes. `filter` applies to
    /// every row (the caller pre-computes filtered bytes if filter != 0).
    fn scanlines(rows: &[Vec<u8>], filter: u8) -> Vec<u8> {
        let mut out = Vec::new();
        for r in rows {
            out.push(filter);
            out.extend_from_slice(r);
        }
        out
    }

    // ── 1. Truecolor (color type 2, 8-bit) → exact ARGB ─────────────────────

    #[test]
    fn decode_truecolor_rgb() {
        // 2x2. Pixels (RGB), filter 0 (None):
        //   (0,0)=red   (1,0)=green
        //   (0,1)=blue  (1,1)=white
        let row0 = vec![255, 0, 0, 0, 255, 0]; // red, green
        let row1 = vec![0, 0, 255, 255, 255, 255]; // blue, white
        let filtered = scanlines(&[row0, row1], 0);
        let idat = zlib_stored(&filtered);
        let png = build_png(2, 2, 8, 2, 0, None, None, &idat);

        let img = decode_png(&png).expect("truecolor decode");
        assert_eq!((img.width, img.height), (2, 2));
        assert_eq!(img.pixel(0, 0), Some((0xFF, 255, 0, 0)));
        assert_eq!(img.pixel(1, 0), Some((0xFF, 0, 255, 0)));
        assert_eq!(img.pixel(0, 1), Some((0xFF, 0, 0, 255)));
        assert_eq!(img.pixel(1, 1), Some((0xFF, 255, 255, 255)));
        // FAIL-ability: an R/B swap would make (0,0) blue.
        assert_ne!(img.pixel(0, 0), Some((0xFF, 0, 0, 255)));
    }

    // ── 2. Truecolor+alpha (color type 6) → exact ARGB with alpha ───────────

    #[test]
    fn decode_truecolor_alpha() {
        // 1x2 RGBA: (255,0,0,128) then (0,255,0,255).
        let row0 = vec![255, 0, 0, 128];
        let row1 = vec![0, 255, 0, 255];
        let filtered = scanlines(&[row0, row1], 0);
        let idat = zlib_stored(&filtered);
        let png = build_png(1, 2, 8, 6, 0, None, None, &idat);
        let img = decode_png(&png).expect("rgba decode");
        assert_eq!(img.pixel(0, 0), Some((128, 255, 0, 0)));
        assert_eq!(img.pixel(0, 1), Some((255, 0, 255, 0)));
    }

    // ── 3. Grayscale (color type 0, 8-bit) ──────────────────────────────────

    #[test]
    fn decode_grayscale() {
        // 3x1: 0, 128, 255.
        let row0 = vec![0, 128, 255];
        let filtered = scanlines(&[row0], 0);
        let idat = zlib_stored(&filtered);
        let png = build_png(3, 1, 8, 0, 0, None, None, &idat);
        let img = decode_png(&png).expect("gray decode");
        assert_eq!(img.pixel(0, 0), Some((0xFF, 0, 0, 0)));
        assert_eq!(img.pixel(1, 0), Some((0xFF, 128, 128, 128)));
        assert_eq!(img.pixel(2, 0), Some((0xFF, 255, 255, 255)));
    }

    // ── 4. Grayscale+alpha (color type 4) ───────────────────────────────────

    #[test]
    fn decode_grayscale_alpha() {
        // 2x1: (gray=100, a=50), (gray=200, a=255).
        let row0 = vec![100, 50, 200, 255];
        let filtered = scanlines(&[row0], 0);
        let idat = zlib_stored(&filtered);
        let png = build_png(2, 1, 8, 4, 0, None, None, &idat);
        let img = decode_png(&png).expect("gray+alpha decode");
        assert_eq!(img.pixel(0, 0), Some((50, 100, 100, 100)));
        assert_eq!(img.pixel(1, 0), Some((255, 200, 200, 200)));
    }

    // ── 5. Palette (color type 3) + tRNS ────────────────────────────────────

    #[test]
    fn decode_palette_with_trns() {
        // palette[0]=red, [1]=green, [2]=blue. tRNS=[0x00, 0x80] → idx0 fully
        // transparent, idx1 alpha=128, idx2 (no entry) opaque.
        let palette = [(255u8, 0, 0), (0, 255, 0), (0, 0, 255)];
        let trns = [0x00u8, 0x80];
        // 3x1 indices 0,1,2. 8-bit palette → one byte per pixel.
        let row0 = vec![0u8, 1, 2];
        let filtered = scanlines(&[row0], 0);
        let idat = zlib_stored(&filtered);
        let png = build_png(3, 1, 8, 3, 0, Some(&palette), Some(&trns), &idat);
        let img = decode_png(&png).expect("palette decode");
        assert_eq!(img.pixel(0, 0), Some((0x00, 255, 0, 0))); // idx0 transparent red
        assert_eq!(img.pixel(1, 0), Some((0x80, 0, 255, 0))); // idx1 alpha 128 green
        assert_eq!(img.pixel(2, 0), Some((0xFF, 0, 0, 255))); // idx2 opaque blue
                                                              // FAIL-ability: ignoring tRNS would make idx0 opaque.
        assert_ne!(img.pixel(0, 0), Some((0xFF, 255, 0, 0)));
    }

    // ── 6. Palette with sub-byte depth (4-bit) ──────────────────────────────

    #[test]
    fn decode_palette_4bit() {
        // 4-bit palette: two pixels per byte. palette[1]=green, [2]=blue.
        let palette = [(0u8, 0, 0), (0, 255, 0), (0, 0, 255), (255, 255, 255)];
        // 3x1 indices 1, 2, 3 packed 4-bit: byte0 = (1<<4)|2 = 0x12, byte1 = 3<<4 = 0x30.
        let row0 = vec![0x12u8, 0x30];
        let filtered = scanlines(&[row0], 0);
        let idat = zlib_stored(&filtered);
        let png = build_png(3, 1, 4, 3, 0, Some(&palette), None, &idat);
        let img = decode_png(&png).expect("4-bit palette decode");
        assert_eq!(img.pixel(0, 0), Some((0xFF, 0, 255, 0))); // idx1 green
        assert_eq!(img.pixel(1, 0), Some((0xFF, 0, 0, 255))); // idx2 blue
        assert_eq!(img.pixel(2, 0), Some((0xFF, 255, 255, 255))); // idx3 white
    }

    // ── 7. Grayscale 1-bit ──────────────────────────────────────────────────

    #[test]
    fn decode_grayscale_1bit() {
        // 8x1 bits: 1,0,1,0,1,0,1,0 → 0xAA. 1-bit gray scales 1→255, 0→0.
        let row0 = vec![0xAAu8];
        let filtered = scanlines(&[row0], 0);
        let idat = zlib_stored(&filtered);
        let png = build_png(8, 1, 1, 0, 0, None, None, &idat);
        let img = decode_png(&png).expect("1-bit gray decode");
        assert_eq!(img.pixel(0, 0), Some((0xFF, 255, 255, 255)));
        assert_eq!(img.pixel(1, 0), Some((0xFF, 0, 0, 0)));
        assert_eq!(img.pixel(7, 0), Some((0xFF, 0, 0, 0)));
    }

    // ── 8. 16-bit truecolor down-scales to the high byte ────────────────────

    #[test]
    fn decode_truecolor_16bit() {
        // 1x1 RGB16: R=0x1234, G=0x5678, B=0x9ABC. High bytes: 0x12, 0x56, 0x9A.
        let row0 = vec![0x12, 0x34, 0x56, 0x78, 0x9A, 0xBC];
        let filtered = scanlines(&[row0], 0);
        let idat = zlib_stored(&filtered);
        let png = build_png(1, 1, 16, 2, 0, None, None, &idat);
        let img = decode_png(&png).expect("16-bit decode");
        assert_eq!(img.pixel(0, 0), Some((0xFF, 0x12, 0x56, 0x9A)));
    }

    // ── 9. Each of the 5 filter types round-trips ───────────────────────────
    //
    // We hand-build a 3x3 grayscale image, apply each filter to its scanlines
    // (using the SAME predictor math the spec defines, computed independently in
    // the test), and assert the decoder recovers the original pixels exactly.

    fn original_3x3() -> [[u8; 3]; 3] {
        [[10, 20, 30], [40, 50, 60], [70, 80, 90]]
    }

    /// Apply a PNG filter to produce filtered scanlines for an 8-bit, 1-channel
    /// (grayscale) image. bpp = 1.
    fn apply_filter(orig: &[[u8; 3]; 3], filter: u8) -> Vec<u8> {
        let h = orig.len();
        let w = orig[0].len();
        let bpp = 1usize;
        let mut out = Vec::new();
        for y in 0..h {
            out.push(filter);
            for x in 0..w {
                let cur = orig[y][x];
                let a = if x >= bpp { orig[y][x - bpp] } else { 0 };
                let b = if y > 0 { orig[y - 1][x] } else { 0 };
                let c = if y > 0 && x >= bpp {
                    orig[y - 1][x - bpp]
                } else {
                    0
                };
                let f = match filter {
                    0 => cur,
                    1 => cur.wrapping_sub(a),
                    2 => cur.wrapping_sub(b),
                    3 => cur.wrapping_sub(((a as u16 + b as u16) / 2) as u8),
                    4 => cur.wrapping_sub(super::paeth(a, b, c)),
                    _ => cur,
                };
                out.push(f);
            }
        }
        out
    }

    #[test]
    fn all_five_filters_roundtrip() {
        let orig = original_3x3();
        for filter in 0u8..=4 {
            let filtered = apply_filter(&orig, filter);
            let idat = zlib_stored(&filtered);
            let png = build_png(3, 3, 8, 0, 0, None, None, &idat);
            let img =
                decode_png(&png).unwrap_or_else(|e| panic!("filter {filter} decode failed: {e:?}"));
            for y in 0..3 {
                for x in 0..3 {
                    let got = img.pixel(x as u32, y as u32).expect("in-bounds");
                    let want = orig[y][x];
                    assert_eq!(
                        got,
                        (0xFF, want, want, want),
                        "filter {filter} pixel ({x},{y}) mismatch"
                    );
                }
            }
        }
    }

    // ── 10. Paeth predictor matches the spec on a hand-checked case ─────────

    #[test]
    fn paeth_predictor_known_values() {
        // Spec §6.6 example logic, hand-computed:
        // paeth(a=10, b=20, c=15): p = 10+20-15 = 15; pa=|15-10|=5, pb=|15-20|=5,
        // pc=|15-15|=0 → pc smallest? pa(5)<=pb(5) && pa<=pc(0)? no (5>0). So
        // pb<=pc? 5<=0? no → return c=15.
        assert_eq!(super::paeth(10, 20, 15), 15);
        // paeth(a=200, b=10, c=5): p=205; pa=|205-200|=5, pb=|205-10|=195,
        // pc=|205-5|=200 → pa smallest → a=200.
        assert_eq!(super::paeth(200, 10, 5), 200);
        // paeth(a=5, b=200, c=10): p=195; pa=190, pb=5, pc=185 → pb smallest → b=200.
        assert_eq!(super::paeth(5, 200, 10), 200);
        // Tie rule: pa==pb==pc favors a, then b. paeth(0,0,0)=0.
        assert_eq!(super::paeth(0, 0, 0), 0);
        // FAIL-ability: a predictor that always returned `a` would give 10 not 15.
        assert_ne!(super::paeth(10, 20, 15), 10);
    }

    // ── 11. Adam7 interlace == progressive for the same pixels ──────────────

    #[test]
    fn adam7_matches_progressive() {
        // 8x8 truecolor: pixel (x,y) = (x*32, y*32, (x+y)*16) so every pixel is
        // distinct → a wrong pass mapping is visible.
        let w = 8usize;
        let h = 8usize;
        let color = |x: usize, y: usize| -> [u8; 3] {
            [(x * 32) as u8, (y * 32) as u8, ((x + y) * 16) as u8]
        };

        // Progressive reference.
        let mut prog_rows: Vec<Vec<u8>> = Vec::new();
        for y in 0..h {
            let mut row = Vec::new();
            for x in 0..w {
                row.extend_from_slice(&color(x, y));
            }
            prog_rows.push(row);
        }
        let prog_idat = zlib_stored(&scanlines(&prog_rows, 0));
        let prog_png = build_png(w as u32, h as u32, 8, 2, 0, None, None, &prog_idat);
        let prog = decode_png(&prog_png).expect("progressive");

        // Adam7: emit each pass's sub-image scanlines in order.
        let mut filtered = Vec::new();
        for pass in 0..7 {
            let pw = pass_count(w, ADAM7_X_START[pass], ADAM7_X_STEP[pass]);
            let ph = pass_count(h, ADAM7_Y_START[pass], ADAM7_Y_STEP[pass]);
            for py in 0..ph {
                filtered.push(0u8); // filter None
                let y = ADAM7_Y_START[pass] + py * ADAM7_Y_STEP[pass];
                for px in 0..pw {
                    let x = ADAM7_X_START[pass] + px * ADAM7_X_STEP[pass];
                    filtered.extend_from_slice(&color(x, y));
                }
            }
        }
        let inter_idat = zlib_stored(&filtered);
        let inter_png = build_png(w as u32, h as u32, 8, 2, 1, None, None, &inter_idat);
        let inter = decode_png(&inter_png).expect("interlaced");

        assert_eq!(prog.pixels, inter.pixels, "Adam7 must equal progressive");
        // Spot-check a corner and a center pixel.
        assert_eq!(inter.pixel(0, 0), Some((0xFF, 0, 0, 0)));
        assert_eq!(inter.pixel(7, 7), prog.pixel(7, 7));
        // FAIL-ability: distinct pixels mean a broken pass order would differ.
        assert_ne!(inter.pixel(0, 0), inter.pixel(7, 7));
    }

    // ── 12. Hostile battery: every malformed input is Err, never a panic ────

    #[test]
    fn reject_bad_signature() {
        assert_eq!(decode_png(&[0u8; 64]), Err(PngError::BadSignature));
        assert_eq!(
            decode_png(b"not a png at all here"),
            Err(PngError::BadSignature)
        );
    }

    #[test]
    fn reject_truncated() {
        // Just the signature.
        assert_eq!(decode_png(&PNG_SIGNATURE), Err(PngError::BadHeader));
        // Signature + partial chunk header.
        let mut d = PNG_SIGNATURE.to_vec();
        d.extend_from_slice(&[0, 0, 0]);
        assert!(matches!(decode_png(&d), Err(PngError::BadHeader)));
    }

    #[test]
    fn reject_bad_crc() {
        let row0 = vec![255u8, 0, 0];
        let idat = zlib_stored(&scanlines(&[row0], 0));
        let mut png = build_png(1, 1, 8, 2, 0, None, None, &idat);
        // Corrupt the IHDR CRC: IHDR is the first chunk after the 8-byte sig;
        // its CRC is the 4 bytes at offset 8 + 4 + 4 + 13 = 29.
        let crc_off = 8 + 4 + 4 + 13;
        png[crc_off] ^= 0xFF;
        assert_eq!(decode_png(&png), Err(PngError::BadCrc));
    }

    #[test]
    fn reject_unknown_critical_chunk() {
        // Insert a chunk "BoGs" with the critical bit (uppercase first letter)
        // after IHDR. "BoGs": 'B'=0x42 (bit 0x20 clear → critical).
        let row0 = vec![255u8, 0, 0];
        let idat = zlib_stored(&scanlines(&[row0], 0));
        // Build manually to insert the bad chunk between IHDR and IDAT.
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&1u32.to_be_bytes());
        ihdr.extend_from_slice(&1u32.to_be_bytes());
        ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
        let mut png = Vec::new();
        png.extend_from_slice(&PNG_SIGNATURE);
        png.extend_from_slice(&chunk(b"IHDR", &ihdr));
        png.extend_from_slice(&chunk(b"BoGs", &[1, 2, 3]));
        png.extend_from_slice(&chunk(b"IDAT", &idat));
        png.extend_from_slice(&chunk(b"IEND", &[]));
        assert_eq!(decode_png(&png), Err(PngError::UnknownCriticalChunk));
    }

    #[test]
    fn accept_unknown_ancillary_chunk() {
        // Lowercase first letter "bKGD"-style → ancillary, must be skipped.
        let row0 = vec![255u8, 0, 0];
        let idat = zlib_stored(&scanlines(&[row0], 0));
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&1u32.to_be_bytes());
        ihdr.extend_from_slice(&1u32.to_be_bytes());
        ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
        let mut png = Vec::new();
        png.extend_from_slice(&PNG_SIGNATURE);
        png.extend_from_slice(&chunk(b"IHDR", &ihdr));
        png.extend_from_slice(&chunk(b"tEXt", b"Comment\0hello"));
        png.extend_from_slice(&chunk(b"IDAT", &idat));
        png.extend_from_slice(&chunk(b"IEND", &[]));
        let img = decode_png(&png).expect("ancillary chunk must be skipped");
        assert_eq!(img.pixel(0, 0), Some((0xFF, 255, 0, 0)));
    }

    #[test]
    fn reject_truncated_idat() {
        // A valid header but the IDAT zlib stream is chopped → InflateFailed.
        let row0 = vec![255u8, 0, 0];
        let mut idat = zlib_stored(&scanlines(&[row0], 0));
        idat.truncate(3); // keep zlib header, drop the body
        let png = build_png(1, 1, 8, 2, 0, None, None, &idat);
        assert_eq!(decode_png(&png), Err(PngError::InflateFailed));
    }

    #[test]
    fn reject_oversized_dimensions() {
        // 0x7FFFFFFF x 0x7FFFFFFF in IHDR → over MAX_PIXELS.
        let idat = zlib_stored(&[0u8]);
        let png = build_png(0x7FFF_FFFF, 0x7FFF_FFFF, 8, 2, 0, None, None, &idat);
        assert_eq!(decode_png(&png), Err(PngError::DimensionsOutOfRange));
    }

    #[test]
    fn reject_zero_dimensions() {
        let idat = zlib_stored(&[0u8]);
        let png = build_png(0, 1, 8, 2, 0, None, None, &idat);
        assert_eq!(decode_png(&png), Err(PngError::DimensionsOutOfRange));
    }

    #[test]
    fn reject_bad_color_type() {
        let idat = zlib_stored(&[0u8]);
        // color type 5 is illegal.
        let png = build_png(1, 1, 8, 5, 0, None, None, &idat);
        assert_eq!(decode_png(&png), Err(PngError::BadColorType));
        // bit depth 3 with grayscale is illegal.
        let png2 = build_png(1, 1, 3, 0, 0, None, None, &idat);
        assert_eq!(decode_png(&png2), Err(PngError::BadColorType));
    }

    #[test]
    fn reject_palette_missing() {
        // color type 3 (palette) without a PLTE chunk.
        let row0 = vec![0u8];
        let idat = zlib_stored(&scanlines(&[row0], 0));
        let png = build_png(1, 1, 8, 3, 0, None, None, &idat);
        assert_eq!(decode_png(&png), Err(PngError::BadPalette));
    }

    #[test]
    fn reject_palette_index_out_of_range() {
        // 8-bit palette index 5 but only 2 palette entries.
        let palette = [(255u8, 0, 0), (0, 255, 0)];
        let row0 = vec![5u8];
        let idat = zlib_stored(&scanlines(&[row0], 0));
        let png = build_png(1, 1, 8, 3, 0, Some(&palette), None, &idat);
        assert_eq!(decode_png(&png), Err(PngError::BadPalette));
    }

    #[test]
    fn reject_bad_image_data_size() {
        // Declare 4x4 truecolor but the IDAT only carries one byte of samples →
        // BadImageData (the filtered stream is too short).
        let idat = zlib_stored(&[0u8, 1, 2]);
        let png = build_png(4, 4, 8, 2, 0, None, None, &idat);
        assert_eq!(decode_png(&png), Err(PngError::BadImageData));
    }

    // ── 13. Grayscale tRNS color-key transparency ───────────────────────────

    #[test]
    fn grayscale_trns_color_key() {
        // 8-bit gray, tRNS key = gray 128 (2 bytes big-endian, high byte ignored
        // for 8-bit but the low byte carries the value: [0x00, 0x80]).
        let trns = [0x00u8, 0x80];
        let row0 = vec![128u8, 200];
        let idat = zlib_stored(&scanlines(&[row0], 0));
        let png = build_png(2, 1, 8, 0, 0, None, Some(&trns), &idat);
        let img = decode_png(&png).expect("gray trns decode");
        // Pixel 0 == key → transparent; pixel 1 opaque.
        assert_eq!(img.pixel(0, 0), Some((0x00, 128, 128, 128)));
        assert_eq!(img.pixel(1, 0), Some((0xFF, 200, 200, 200)));
    }

    // ════════════════════════════════════════════════════════════════════════
    // ENCODER KATs — the round-trip is the proof: encode_png() then the EXISTING
    // decode_png() must read back width/height + EVERY pixel exactly (lossless).
    // FAIL-able by construction: tweaking any expected pixel breaks the assert.
    // ════════════════════════════════════════════════════════════════════════

    fn make_image(w: u32, h: u32, f: impl Fn(u32, u32) -> u32) -> PngImage {
        let mut pixels = Vec::with_capacity((w * h) as usize);
        for y in 0..h {
            for x in 0..w {
                pixels.push(f(x, y));
            }
        }
        PngImage {
            width: w,
            height: h,
            pixels,
        }
    }

    /// Assert encode→decode is pixel-exact and the bytes are a real PNG.
    fn assert_roundtrip(img: &PngImage) -> Vec<u8> {
        let encoded = encode_png(img).expect("encode must succeed");
        // The encoded bytes start with the PNG signature.
        assert_eq!(&encoded[0..8], &PNG_SIGNATURE, "missing PNG signature");
        // The existing decoder accepts them and reads back identical pixels.
        let back = decode_png(&encoded).expect("decoder must accept our output");
        assert_eq!(
            (back.width, back.height),
            (img.width, img.height),
            "dimensions changed across round-trip"
        );
        assert_eq!(
            back.pixels, img.pixels,
            "LOSSLESS round-trip failed: a pixel differs"
        );
        encoded
    }

    // ── E1. Lossless round-trip: solid colors + gradient + varied alpha ─────

    #[test]
    fn encode_roundtrip_mixed_alpha() {
        // 8x6 with: a fully-transparent column (alpha 0), a semi-transparent
        // column (alpha 0x40), an opaque gradient, and solid red/green/blue.
        let img = make_image(8, 6, |x, y| {
            match x {
                0 => argb(0x00, 255, 0, 0),     // fully transparent red
                1 => argb(0x40, 0, 255, 0),     // semi-transparent green
                2 => argb(0xFF, 255, 0, 0),     // opaque red
                3 => argb(0xFF, 0, 255, 0),     // opaque green
                4 => argb(0xFF, 0, 0, 255),     // opaque blue
                5 => argb(0xFF, 255, 255, 255), // white
                6 => argb(0x80, 128, 64, 32),   // semi-transparent brown
                // gradient column varying by row, partial alpha
                _ => argb(
                    (y as u8) * 40,
                    (y as u8) * 30,
                    (x as u8) * 20,
                    255 - (y as u8) * 30,
                ),
            }
        });
        let encoded = assert_roundtrip(&img);
        // Mixed alpha → must be color type 6 (RGBA). IHDR color-type byte is at
        // signature(8) + len(4) + "IHDR"(4) + 9 = 25.
        assert_eq!(encoded[25], 6, "mixed alpha must encode as RGBA (type 6)");
        // FAIL-ability: a wrong expected pixel would break assert_roundtrip.
        // Confirm the transparent pixel actually survived as transparent.
        let back = decode_png(&encoded).unwrap();
        assert_eq!(back.pixel(0, 0), Some((0x00, 255, 0, 0)));
        assert_eq!(back.pixel(1, 0), Some((0x40, 0, 255, 0)));
        assert_ne!(back.pixel(0, 0), Some((0xFF, 255, 0, 0))); // alpha was NOT clobbered
    }

    // ── E2. 1x1 image (the degenerate-but-valid case) ───────────────────────

    #[test]
    fn encode_roundtrip_1x1() {
        let img = make_image(1, 1, |_, _| argb(0x7F, 12, 34, 56));
        let encoded = assert_roundtrip(&img);
        let back = decode_png(&encoded).unwrap();
        assert_eq!(back.pixel(0, 0), Some((0x7F, 12, 34, 56)));
    }

    // ── E3. Wide-and-short + tall-and-narrow (stride / scanline correctness) ─

    #[test]
    fn encode_roundtrip_wide_short() {
        // 257x1 forces a row wider than a byte boundary; gradient so a stride
        // bug shears the image.
        let img = make_image(257, 1, |x, _| {
            argb(0xFF, (x & 0xFF) as u8, ((x >> 1) & 0xFF) as u8, 7)
        });
        assert_roundtrip(&img);
    }

    #[test]
    fn encode_roundtrip_tall_narrow() {
        let img = make_image(1, 257, |_, y| {
            argb(0xC0, (y & 0xFF) as u8, 9, ((y >> 1) & 0xFF) as u8)
        });
        assert_roundtrip(&img);
    }

    // ── E4. All-opaque image takes the RGB optimization AND still round-trips ─

    #[test]
    fn encode_all_opaque_uses_rgb_and_roundtrips() {
        let img = make_image(5, 5, |x, y| {
            argb(0xFF, (x * 50) as u8, (y * 50) as u8, ((x + y) * 25) as u8)
        });
        let encoded = assert_roundtrip(&img); // round-trips to the SAME ARGB (alpha 0xFF)
                                              // IHDR color-type byte at offset 25 must be 2 (RGB), not 6.
        assert_eq!(encoded[25], 2, "all-opaque image must use RGB color type 2");
    }

    // ── E5. Every emitted filter type round-trips ───────────────────────────
    //
    // The encoder's adaptive heuristic may pick any of the 5 filters per row;
    // this drives each one explicitly through the same filter_scanline path the
    // encoder uses, wraps it as a PNG, and asserts the decoder recovers it.

    #[test]
    fn encode_each_filter_roundtrips() {
        // A 4x4 RGBA image with structure so different filters are exercised.
        let img = make_image(4, 4, |x, y| {
            argb(200, (x * 60) as u8, (y * 60) as u8, ((x ^ y) * 40) as u8)
        });
        let w = img.width as usize;
        let h = img.height as usize;
        let bpp = 4usize;
        let stride = w * bpp;

        // Build the raw (unfiltered) sample rows once.
        let mut raw_rows: Vec<Vec<u8>> = Vec::new();
        for y in 0..h {
            let mut row = vec![0u8; stride];
            for x in 0..w {
                let p = img.pixels[y * w + x];
                let o = x * bpp;
                row[o] = (p >> 16) as u8;
                row[o + 1] = (p >> 8) as u8;
                row[o + 2] = p as u8;
                row[o + 3] = (p >> 24) as u8;
            }
            raw_rows.push(row);
        }

        for filter in 0u8..=4 {
            let mut filtered = Vec::new();
            for y in 0..h {
                let prev: &[u8] = if y > 0 { &raw_rows[y - 1] } else { &[] };
                let line = super::filter_scanline(filter, &raw_rows[y], prev, bpp);
                filtered.push(filter);
                filtered.extend_from_slice(&line);
            }
            let idat = zlib_stored(&filtered);
            let png = build_png(w as u32, h as u32, 8, 6, 0, None, None, &idat);
            let back = decode_png(&png)
                .unwrap_or_else(|e| panic!("encoder filter {filter} not decodable: {e:?}"));
            assert_eq!(
                back.pixels, img.pixels,
                "encoder filter {filter} did not round-trip"
            );
        }
    }

    // ── E6. Absurd / inconsistent dimensions → Err (never corrupt output) ───

    #[test]
    fn encode_rejects_bad_dimensions() {
        // Zero dimension.
        let z = PngImage {
            width: 0,
            height: 1,
            pixels: vec![0],
        };
        assert_eq!(encode_png(&z), Err(PngEncodeError::DimensionsOutOfRange));
        // Over MAX_PIXELS (claimed dimensions absurd) — checked before alloc.
        let huge = PngImage {
            width: MAX_DIMENSION,
            height: MAX_DIMENSION,
            pixels: Vec::new(),
        };
        assert_eq!(encode_png(&huge), Err(PngEncodeError::DimensionsOutOfRange));
        // pixel-count mismatch.
        let mismatch = PngImage {
            width: 4,
            height: 4,
            pixels: vec![0u32; 3],
        };
        assert_eq!(
            encode_png(&mismatch),
            Err(PngEncodeError::PixelCountMismatch)
        );
    }

    // ── E7. IDAT actually compresses a large flat image (compressed < raw) ──

    #[test]
    fn encode_compresses_flat_image() {
        // 256x256 of one color → highly compressible. Raw filtered stream is
        // (256*3 + 1) * 256 bytes (RGB after the all-opaque optimization).
        let img = make_image(256, 256, |_, _| argb(0xFF, 17, 42, 99));
        let encoded = assert_roundtrip(&img);
        let raw_filtered = (256 * 3 + 1) * 256;
        assert!(
            encoded.len() < raw_filtered / 4,
            "flat image should compress well: encoded {} vs raw {}",
            encoded.len(),
            raw_filtered
        );
    }

    // ── E8. to_png() convenience method matches encode_png() ────────────────

    #[test]
    fn to_png_matches_encode_png() {
        let img = make_image(3, 2, |x, y| argb(0x90, (x * 80) as u8, (y * 80) as u8, 5));
        assert_eq!(img.to_png(), encode_png(&img));
        assert_roundtrip(&img);
    }
}

// ════════════════════════════════════════════════════════════════════════════
// FUZZ / PROPERTY suite — deterministic seeded PRNG, no external fuzz crate.
//
// The properties: on ANY byte sequence `decode_png` must (a) never panic, and
// (b) bound every allocation — a crafted IHDR (huge dimensions) cannot request a
// multi-GiB buffer. FAIL-ability:
//  - `#![forbid(unsafe_code)]` means an OOB index is a guaranteed panic, not
//    silent UB, so the never-panic loops genuinely prove bounds-safety: if any
//    decode path could panic on hostile bytes the loop aborts the test process.
//  - If MAX_PIXELS / MAX_DIMENSION were removed, `fuzz_huge_dimensions` would
//    request a multi-GiB `vec![0u32; w*h]` and OOM (process abort = failure)
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

    fn assert_bounded(img: &PngImage) {
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
            if let Ok(img) = decode_png(&buf) {
                assert_bounded(&img);
            }
        }
    }

    #[test]
    fn fuzz_signature_prefixed_never_panic() {
        let mut rng = Rng::new(0x5160_F00D);
        for _ in 0..40_000 {
            let len = rng.below(256);
            let mut buf = Vec::with_capacity(8 + len);
            buf.extend_from_slice(&PNG_SIGNATURE);
            for _ in 0..len {
                buf.push(rng.byte());
            }
            if let Ok(img) = decode_png(&buf) {
                assert_bounded(&img);
            }
        }
    }

    #[test]
    fn fuzz_mutated_valid_png_never_panic() {
        // A well-formed 3x3 truecolor PNG, mutated byte-by-byte.
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&3u32.to_be_bytes());
        ihdr.extend_from_slice(&3u32.to_be_bytes());
        ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);

        // filtered scanlines: 3 rows of 9 sample bytes, filter 0.
        let mut filtered = Vec::new();
        for y in 0..3u8 {
            filtered.push(0);
            for x in 0..9u8 {
                filtered.push(x.wrapping_add(y));
            }
        }
        let idat = make_zlib_stored(&filtered);

        let mut base = Vec::new();
        base.extend_from_slice(&PNG_SIGNATURE);
        base.extend_from_slice(&make_chunk(b"IHDR", &ihdr));
        base.extend_from_slice(&make_chunk(b"IDAT", &idat));
        base.extend_from_slice(&make_chunk(b"IEND", &[]));

        assert!(decode_png(&base).is_ok(), "seed PNG must decode");

        let mut rng = Rng::new(0x3333_F00D);
        for _ in 0..80_000 {
            let mut m = base.clone();
            let muts = 1 + rng.below(4);
            for _ in 0..muts {
                let i = rng.below(m.len());
                m[i] ^= rng.byte();
            }
            if let Ok(img) = decode_png(&m) {
                assert_bounded(&img);
            }
        }
    }

    #[test]
    fn fuzz_huge_dimensions_bounded() {
        // Sweep large IHDR dimensions: must Err with the cap, never allocate.
        let mut rng = Rng::new(0x5151_BAD);
        for _ in 0..5000 {
            let w = (rng.below(0x8000_0000)) as u32;
            let h = (rng.below(0x8000_0000)) as u32;
            let mut ihdr = Vec::new();
            ihdr.extend_from_slice(&w.to_be_bytes());
            ihdr.extend_from_slice(&h.to_be_bytes());
            ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
            let mut png = Vec::new();
            png.extend_from_slice(&PNG_SIGNATURE);
            png.extend_from_slice(&make_chunk(b"IHDR", &ihdr));
            png.extend_from_slice(&make_chunk(b"IDAT", &make_zlib_stored(&[0u8])));
            png.extend_from_slice(&make_chunk(b"IEND", &[]));
            match decode_png(&png) {
                Ok(img) => assert_bounded(&img),
                Err(_) => {}
            }
        }
    }

    // Local copies of the fixture helpers (the `tests` module's are private to it).
    fn make_chunk(ty: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        out.extend_from_slice(ty);
        out.extend_from_slice(data);
        let mut crc_input = Vec::new();
        crc_input.extend_from_slice(ty);
        crc_input.extend_from_slice(data);
        out.extend_from_slice(&rae_deflate::crc32(&crc_input).to_be_bytes());
        out
    }

    fn make_zlib_stored(payload: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.push(0x78);
        let cmf: u16 = 0x78;
        let mut flg: u16 = 0;
        let rem = ((cmf << 8) | flg) % 31;
        if rem != 0 {
            flg += 31 - rem;
        }
        out.pop();
        out.push(cmf as u8);
        out.push(flg as u8);
        out.push(0x01);
        let len = payload.len() as u16;
        let nlen = !len;
        out.push((len & 0xFF) as u8);
        out.push((len >> 8) as u8);
        out.push((nlen & 0xFF) as u8);
        out.push((nlen >> 8) as u8);
        out.extend_from_slice(payload);
        out.extend_from_slice(&rae_deflate::adler32(payload).to_be_bytes());
        out
    }
}
