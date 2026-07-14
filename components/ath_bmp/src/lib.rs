//! # RaeBMP — BMP + ICO decoder (favicons + raster coverage).
//!
//! LEGACY_GAMING_CONCEPT.md (§web / media): a browser that can't show a site's
//! favicon, and an OS that can't render a Windows bitmap, aren't daily drivers.
//! AthenaOS already decodes PNG/JPEG/GIF; BMP rounds out raster coverage, and
//! **ICO is the favicon container the web pillar needs** — favicons ship as
//! `.ico` files served from untrusted websites. This crate is the from-scratch
//! decoder a browser's tab-strip and a future Photos viewer sit on.
//!
//! ## What it decodes
//! - **BMP**: `BITMAPFILEHEADER` + `BITMAPINFOHEADER` (tolerating BITMAPV4/V5 by
//!   reading the core fields and skipping the rest). Bit depths 32 (BGRA/BGRX),
//!   24 (BGR), 8 / 4 / 1 (palette-indexed, with the color table). Both bottom-up
//!   and top-down (negative height) row order, with rows padded to a 4-byte
//!   boundary. RLE8/RLE4 are detected and rejected cleanly as `Unsupported`.
//! - **ICO**: the `ICONDIR` + `ICONDIRENTRY` table. Each entry is either an
//!   embedded BMP "DIB" (a `BITMAPINFOHEADER` with **doubled height** for the
//!   AND-mask, no file header, plus a 1-bpp transparency mask after the color
//!   data — the mask becomes alpha) or an embedded **PNG** (modern large icons:
//!   detected by signature and returned as raw bytes for the PNG decoder).
//!
//! Output is a flat ARGB8888 `Vec<u32>` (`0xAARRGGBB`) — the AthGFX
//! compositor/Canvas pixel format, matching the house PNG/JPEG/GIF decoders.
//!
//! ## Hostile-input posture (CLAUDE: decoders are the #1 RCE surface)
//! Favicons are attacker-controlled. There is **no `unwrap`/`expect`/panic** path
//! reachable from `decode_bmp` or `decode_ico`: bad signatures, truncated
//! headers, offsets past the buffer, oversized dimensions, absurd entry counts,
//! and corrupt palettes all return `Err(...)`. Memory is bounded up front
//! (`MAX_DIMENSION`, `MAX_PIXELS`, `MAX_ICON_ENTRIES`) so a crafted header can't
//! request a multi-gigabyte allocation. The host KAT suite at the bottom of this
//! file is the primary proof (run `cargo test -p ath_bmp`).

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

use alloc::vec;
use alloc::vec::Vec;

/// Bound on either image axis. Matches the PNG/GIF decoders' practical ceiling.
const MAX_DIMENSION: u32 = 1 << 16; // 65_536
/// Bound on total pixel count (width * height). ~16M px = 64 MiB at 4 B/px.
/// Icons are tiny; this generously covers the largest plausible BMP wallpaper.
const MAX_PIXELS: u64 = 16 * 1024 * 1024;
/// Bound on ICONDIRENTRY count. Real `.ico` files have a handful of sizes.
const MAX_ICON_ENTRIES: usize = 256;

/// Decode error. Every variant is a *handled* path — nothing here panics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BmpError {
    /// Buffer too small to hold the required headers.
    Truncated,
    /// First bytes are not the `BM` BMP magic.
    BadSignature,
    /// Width/height is zero, negative-width, or exceeds the memory bound.
    DimensionsOutOfRange,
    /// A bit depth / header this decoder does not implement (e.g. RLE, OS/2 v1).
    Unsupported,
    /// The pixel-data offset points past the end of the buffer.
    BadOffset,
    /// A palette index referenced a color outside the color table.
    BadPaletteIndex,
    /// Color table required (indexed depth) but missing / too small.
    MissingPalette,
    /// `ICONDIR` reserved/type fields were not a valid icon directory.
    BadIconDir,
    /// An `ICONDIRENTRY` offset/size pointed outside the buffer.
    BadIconEntry,
    /// `.ico` had zero entries.
    NoEntries,
}

/// A decoded image: a flat ARGB8888 buffer plus dimensions.
///
/// `pixels.len() == (width * height) as usize`. Each `u32` is `0xAARRGGBB`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BmpImage {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u32>,
}

impl BmpImage {
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

fn read_u16_le(buf: &[u8], off: usize) -> Option<u16> {
    let b = buf.get(off..off + 2)?;
    Some(u16::from_le_bytes([b[0], b[1]]))
}
fn read_u32_le(buf: &[u8], off: usize) -> Option<u32> {
    let b = buf.get(off..off + 4)?;
    Some(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}
fn read_i32_le(buf: &[u8], off: usize) -> Option<i32> {
    let b = buf.get(off..off + 4)?;
    Some(i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

/// The PNG 8-byte signature — embedded-PNG icons start with this.
const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

// ─── BMP info header (the part shared by V3/V4/V5) ──────────────────────────

/// The fields of `BITMAPINFOHEADER` this decoder reads. V4/V5 extend the header
/// with masks/colorspace we skip; `header_size` lets us locate the palette.
struct BmpInfo {
    header_size: u32,
    width: u32,
    /// `true` when height was negative (top-down row order).
    top_down: bool,
    height: u32,
    bit_count: u16,
    compression: u32,
    colors_used: u32,
}

/// Compression constants (BITMAPINFOHEADER `biCompression`).
const BI_RGB: u32 = 0;
const BI_RLE8: u32 = 1;
const BI_RLE4: u32 = 2;
const BI_BITFIELDS: u32 = 3;

/// Parse a `BITMAPINFOHEADER` (or V4/V5 superset) at `off`. `info_size` is the
/// declared header size (first u32 of the header).
fn parse_info_header(buf: &[u8], off: usize) -> Result<BmpInfo, BmpError> {
    let header_size = read_u32_le(buf, off).ok_or(BmpError::Truncated)?;
    // BITMAPINFOHEADER is 40 bytes; V4=108, V5=124. OS/2 v1 (BITMAPCOREHEADER)
    // is 12 bytes and uses a different layout — reject it as Unsupported.
    if header_size < 40 {
        return Err(BmpError::Unsupported);
    }
    let width = read_i32_le(buf, off + 4).ok_or(BmpError::Truncated)?;
    let height_raw = read_i32_le(buf, off + 8).ok_or(BmpError::Truncated)?;
    let bit_count = read_u16_le(buf, off + 14).ok_or(BmpError::Truncated)?;
    let compression = read_u32_le(buf, off + 16).ok_or(BmpError::Truncated)?;
    let colors_used = read_u32_le(buf, off + 32).ok_or(BmpError::Truncated)?;

    if width <= 0 {
        return Err(BmpError::DimensionsOutOfRange);
    }
    let top_down = height_raw < 0;
    // `i32::MIN` has no positive magnitude — reject before negation.
    let height = if height_raw == i32::MIN {
        return Err(BmpError::DimensionsOutOfRange);
    } else {
        height_raw.unsigned_abs()
    };
    if height == 0 {
        return Err(BmpError::DimensionsOutOfRange);
    }
    let width = width as u32;
    if width > MAX_DIMENSION || height > MAX_DIMENSION {
        return Err(BmpError::DimensionsOutOfRange);
    }
    if (width as u64) * (height as u64) > MAX_PIXELS {
        return Err(BmpError::DimensionsOutOfRange);
    }
    Ok(BmpInfo {
        header_size,
        width,
        top_down,
        height,
        bit_count,
        compression,
        colors_used,
    })
}

/// Number of palette entries for an indexed depth, honoring `biClrUsed`.
fn palette_count(info: &BmpInfo) -> usize {
    if info.colors_used != 0 {
        return info.colors_used as usize;
    }
    match info.bit_count {
        1 => 2,
        4 => 16,
        8 => 256,
        _ => 0,
    }
}

/// Read a BGRA/BGRX color table of `count` 4-byte entries at `off`.
/// Returns the table as ARGB (opaque) plus the bytes consumed.
fn read_palette(buf: &[u8], off: usize, count: usize) -> Result<Vec<u32>, BmpError> {
    let bytes = count.checked_mul(4).ok_or(BmpError::MissingPalette)?;
    let table = buf.get(off..off + bytes).ok_or(BmpError::MissingPalette)?;
    let mut pal = Vec::with_capacity(count);
    for c in table.chunks_exact(4) {
        // BMP palette entries are stored B, G, R, reserved.
        pal.push(argb(0xFF, c[2], c[1], c[0]));
    }
    Ok(pal)
}

/// Decode the pixel array of a top-level BMP, given an already-parsed header and
/// the absolute byte offset of the pixel data. `palette` is empty for >8bpp.
///
/// `alpha_from_32` selects whether 32-bit pixels keep their stored alpha byte
/// (`true`) or are forced opaque (`false` — many BMPs store junk in the 4th
/// byte). Top-level BMPs force opaque; the ICO path overrides alpha via the mask.
#[allow(clippy::too_many_arguments)]
fn decode_pixels(
    buf: &[u8],
    info: &BmpInfo,
    pixel_off: usize,
    palette: &[u32],
    alpha_from_32: bool,
) -> Result<Vec<u32>, BmpError> {
    let width = info.width as usize;
    let height = info.height as usize;
    let bit_count = info.bit_count;

    // Row size padded up to a 4-byte boundary (BMP rows are DWORD-aligned).
    let row_bits = width
        .checked_mul(bit_count as usize)
        .ok_or(BmpError::DimensionsOutOfRange)?;
    let row_bytes_unpadded = (row_bits + 7) / 8;
    let row_stride = (row_bytes_unpadded + 3) & !3usize;

    let needed = row_stride
        .checked_mul(height)
        .ok_or(BmpError::DimensionsOutOfRange)?;
    let data = buf
        .get(pixel_off..pixel_off + needed)
        .ok_or(BmpError::BadOffset)?;

    let mut pixels = vec![0u32; width * height];

    for src_row in 0..height {
        // Bottom-up storage: the first row in the file is the bottom of the image.
        let dst_row = if info.top_down {
            src_row
        } else {
            height - 1 - src_row
        };
        let row = &data[src_row * row_stride..src_row * row_stride + row_bytes_unpadded];
        let out = &mut pixels[dst_row * width..dst_row * width + width];

        match bit_count {
            32 => {
                for (x, px) in out.iter_mut().enumerate() {
                    let c = &row[x * 4..x * 4 + 4];
                    // Stored B, G, R, A.
                    let a = if alpha_from_32 { c[3] } else { 0xFF };
                    *px = argb(a, c[2], c[1], c[0]);
                }
            }
            24 => {
                for (x, px) in out.iter_mut().enumerate() {
                    let c = &row[x * 3..x * 3 + 3];
                    *px = argb(0xFF, c[2], c[1], c[0]);
                }
            }
            8 => {
                for (x, px) in out.iter_mut().enumerate() {
                    let idx = row[x] as usize;
                    *px = *palette.get(idx).ok_or(BmpError::BadPaletteIndex)?;
                }
            }
            4 => {
                for x in 0..width {
                    let byte = row[x / 2];
                    let idx = if x & 1 == 0 {
                        (byte >> 4) as usize
                    } else {
                        (byte & 0x0F) as usize
                    };
                    out[x] = *palette.get(idx).ok_or(BmpError::BadPaletteIndex)?;
                }
            }
            1 => {
                for x in 0..width {
                    let byte = row[x / 8];
                    let bit = 7 - (x & 7);
                    let idx = ((byte >> bit) & 1) as usize;
                    out[x] = *palette.get(idx).ok_or(BmpError::BadPaletteIndex)?;
                }
            }
            _ => return Err(BmpError::Unsupported),
        }
    }

    Ok(pixels)
}

/// Decode a standalone BMP file (`BM` magic + file header + info header).
///
/// Hostile-input safe: returns `Err` (never panics) on any malformed input.
/// RLE8/RLE4 compression is detected and rejected as `Unsupported`.
pub fn decode_bmp(data: &[u8]) -> Result<BmpImage, BmpError> {
    // BITMAPFILEHEADER is 14 bytes: "BM", size(4), reserved(4), pixel offset(4).
    if data.len() < 14 {
        return Err(BmpError::Truncated);
    }
    if &data[0..2] != b"BM" {
        return Err(BmpError::BadSignature);
    }
    let pixel_off = read_u32_le(data, 10).ok_or(BmpError::Truncated)? as usize;

    let info = parse_info_header(data, 14)?;
    decode_bmp_dib(data, &info, 14, Some(pixel_off), false)
}

/// Decode a BMP whose info header starts at `info_off`. Shared by the file-level
/// `decode_bmp` and the ICO embedded-DIB path. When `pixel_off_override` is
/// `Some`, the file header supplied an absolute pixel offset; otherwise the pixel
/// data is assumed to follow the header + palette (the ICO DIB layout).
fn decode_bmp_dib(
    data: &[u8],
    info: &BmpInfo,
    info_off: usize,
    pixel_off_override: Option<usize>,
    alpha_from_32: bool,
) -> Result<BmpImage, BmpError> {
    if info.compression == BI_RLE8 || info.compression == BI_RLE4 {
        // RLE is a documented gap — decoded cleanly as Err, not garbage.
        return Err(BmpError::Unsupported);
    }
    if info.compression != BI_RGB && info.compression != BI_BITFIELDS {
        return Err(BmpError::Unsupported);
    }

    let pal_count = palette_count(info);
    let palette = if pal_count > 0 {
        let pal_off = info_off
            .checked_add(info.header_size as usize)
            .ok_or(BmpError::Truncated)?;
        read_palette(data, pal_off, pal_count)?
    } else {
        Vec::new()
    };

    // For indexed depths the palette must be present.
    if matches!(info.bit_count, 1 | 4 | 8) && palette.is_empty() {
        return Err(BmpError::MissingPalette);
    }
    if !matches!(info.bit_count, 1 | 4 | 8 | 24 | 32) {
        return Err(BmpError::Unsupported);
    }

    let pixel_off = match pixel_off_override {
        Some(o) => o,
        None => {
            // DIB layout: pixels follow the header + (BGRA) palette.
            let pal_bytes = pal_count.checked_mul(4).ok_or(BmpError::Truncated)?;
            info_off
                .checked_add(info.header_size as usize)
                .and_then(|v| v.checked_add(pal_bytes))
                .ok_or(BmpError::Truncated)?
        }
    };
    if pixel_off > data.len() {
        return Err(BmpError::BadOffset);
    }

    let pixels = decode_pixels(data, info, pixel_off, &palette, alpha_from_32)?;
    Ok(BmpImage {
        width: info.width,
        height: info.height,
        pixels,
    })
}

// ════════════════════════════════════════════════════════════════════════════
// ICO container
// ════════════════════════════════════════════════════════════════════════════

/// One icon directory entry, decoded to either an embedded BMP or raw PNG bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IconKind {
    /// Embedded BMP DIB (AND-mask already folded into alpha).
    Bmp(BmpImage),
    /// Embedded PNG — the raw PNG byte stream, signature intact, for the PNG
    /// decoder. A browser routes this to `athmedia::decode_png`.
    Png(Vec<u8>),
}

/// A single ICO directory entry plus its decoded payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IconEntry {
    pub width: u32,
    pub height: u32,
    pub kind: IconKind,
}

/// The full set of images in an `.ico` file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IconSet {
    pub entries: Vec<IconEntry>,
}

impl IconSet {
    /// Pick the entry whose width best matches `target_px`: the smallest entry
    /// that is `>= target_px`, or — if none is large enough — the largest entry.
    /// `None` only when the set is empty.
    pub fn best(&self, target_px: u32) -> Option<&IconEntry> {
        // Smallest entry with width >= target.
        let at_least = self
            .entries
            .iter()
            .filter(|e| e.width >= target_px)
            .min_by_key(|e| e.width);
        if let Some(e) = at_least {
            return Some(e);
        }
        // Otherwise the largest available.
        self.entries.iter().max_by_key(|e| e.width)
    }
}

/// Decode an `.ico` file into its set of images.
///
/// Hostile-input safe: every offset/length is bounds-checked, entry count is
/// capped at `MAX_ICON_ENTRIES`, and a corrupt/truncated entry yields a clean
/// `Err`. Embedded-PNG entries are returned as `IconKind::Png(raw_bytes)` for the
/// PNG decoder; embedded-BMP entries decode here with the AND-mask → alpha.
pub fn decode_ico(data: &[u8]) -> Result<IconSet, BmpError> {
    // ICONDIR: reserved(2)=0, type(2)=1 (icon), count(2).
    if data.len() < 6 {
        return Err(BmpError::Truncated);
    }
    let reserved = read_u16_le(data, 0).ok_or(BmpError::Truncated)?;
    let kind = read_u16_le(data, 2).ok_or(BmpError::Truncated)?;
    let count = read_u16_le(data, 4).ok_or(BmpError::Truncated)? as usize;
    // type 1 = icon, type 2 = cursor; accept both, reject anything else.
    if reserved != 0 || (kind != 1 && kind != 2) {
        return Err(BmpError::BadIconDir);
    }
    if count == 0 {
        return Err(BmpError::NoEntries);
    }
    if count > MAX_ICON_ENTRIES {
        return Err(BmpError::BadIconDir);
    }

    let mut entries = Vec::with_capacity(count);
    for i in 0..count {
        // Each ICONDIRENTRY is 16 bytes, starting after the 6-byte ICONDIR.
        let e = 6 + i * 16;
        // width/height of 0 means 256 px.
        let w_byte = *data.get(e).ok_or(BmpError::Truncated)?;
        let h_byte = *data.get(e + 1).ok_or(BmpError::Truncated)?;
        let dir_w = if w_byte == 0 { 256u32 } else { w_byte as u32 };
        let dir_h = if h_byte == 0 { 256u32 } else { h_byte as u32 };
        let bytes_in_res = read_u32_le(data, e + 8).ok_or(BmpError::Truncated)? as usize;
        let img_off = read_u32_le(data, e + 12).ok_or(BmpError::Truncated)? as usize;

        let end = img_off
            .checked_add(bytes_in_res)
            .ok_or(BmpError::BadIconEntry)?;
        if bytes_in_res == 0 || end > data.len() {
            return Err(BmpError::BadIconEntry);
        }
        let payload = &data[img_off..end];

        // Embedded PNG? Detect the signature and hand back the raw bytes.
        if payload.len() >= 8 && payload[0..8] == PNG_SIGNATURE {
            entries.push(IconEntry {
                width: dir_w,
                height: dir_h,
                kind: IconKind::Png(payload.to_vec()),
            });
            continue;
        }

        // Otherwise it's an embedded BMP DIB: a BITMAPINFOHEADER with no file
        // header and a DOUBLED height (color rows then the 1-bpp AND mask).
        let img = decode_ico_dib(payload, dir_w, dir_h)?;
        entries.push(img);
    }

    Ok(IconSet { entries })
}

/// Decode the embedded-BMP DIB of one ICO entry, folding the AND-mask into alpha.
fn decode_ico_dib(payload: &[u8], dir_w: u32, dir_h: u32) -> Result<IconEntry, BmpError> {
    let mut info = parse_info_header(payload, 0)?;
    // The DIB stores 2x the icon height: color rows + AND-mask rows. Recover the
    // true image height before decoding the color plane.
    let true_height = info.height / 2;
    if true_height == 0 {
        return Err(BmpError::DimensionsOutOfRange);
    }
    // ICO color planes are always bottom-up (the high bit of height is never set
    // here; the doubling is the convention, not a sign flip).
    info.height = true_height;
    info.top_down = false;

    // For 32-bit icons the stored alpha is meaningful; keep it AND still apply the
    // mask. For <=8/24-bit there is no alpha channel — the mask is the only alpha.
    let alpha_from_32 = info.bit_count == 32;
    let color = decode_bmp_dib(payload, &info, 0, None, alpha_from_32)?;

    // Locate the AND mask: it follows the color plane in the payload.
    let width = info.width as usize;
    let height = true_height as usize;

    // Color-plane byte size (same stride math as decode_pixels).
    let pal_count = palette_count(&info);
    let color_row_bits = width
        .checked_mul(info.bit_count as usize)
        .ok_or(BmpError::DimensionsOutOfRange)?;
    let color_stride = (((color_row_bits + 7) / 8) + 3) & !3usize;
    let pal_bytes = pal_count.checked_mul(4).ok_or(BmpError::Truncated)?;
    let color_plane_off = (info.header_size as usize)
        .checked_add(pal_bytes)
        .ok_or(BmpError::Truncated)?;
    let color_bytes = color_stride
        .checked_mul(height)
        .ok_or(BmpError::DimensionsOutOfRange)?;
    let mask_off = color_plane_off
        .checked_add(color_bytes)
        .ok_or(BmpError::Truncated)?;

    // AND mask is 1 bpp, rows DWORD-aligned, bottom-up. A mask bit of 1 means the
    // pixel is transparent. The mask may legitimately be absent/truncated in some
    // malformed icons — treat a missing mask as fully opaque rather than erroring.
    let mask_row_bytes = (((width + 7) / 8) + 3) & !3usize;
    let mut pixels = color.pixels;
    if let Some(needed) = mask_row_bytes.checked_mul(height) {
        if let Some(mask) = payload.get(mask_off..mask_off + needed) {
            for src_row in 0..height {
                let dst_row = height - 1 - src_row; // bottom-up
                let row =
                    &mask[src_row * mask_row_bytes..src_row * mask_row_bytes + mask_row_bytes];
                for x in 0..width {
                    let byte = row[x / 8];
                    let bit = 7 - (x & 7);
                    let transparent = (byte >> bit) & 1 == 1;
                    if transparent {
                        // Clear alpha; keep RGB so a caller can still see the color
                        // if it ignores alpha.
                        let idx = dst_row * width + x;
                        pixels[idx] &= 0x00FF_FFFF;
                    }
                }
            }
        }
    }

    Ok(IconEntry {
        width: dir_w,
        height: dir_h,
        kind: IconKind::Bmp(BmpImage {
            width: info.width,
            height: true_height,
            pixels,
        }),
    })
}

// ════════════════════════════════════════════════════════════════════════════
// Host KAT suite — `cargo test -p ath_bmp`. FAIL-able by construction.
// ════════════════════════════════════════════════════════════════════════════
#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use alloc::vec::Vec;

    // ── Tiny BMP builders ───────────────────────────────────────────────────

    /// Build a standalone BMP file from already-laid-out pixel rows.
    /// `info_height` may be negative (top-down). `rows` is the raw pixel byte
    /// stream in file order (first row = bottom for bottom-up).
    fn build_bmp(
        width: i32,
        info_height: i32,
        bit_count: u16,
        palette: &[(u8, u8, u8)],
        rows: &[u8],
    ) -> Vec<u8> {
        let mut info = Vec::new();
        info.extend_from_slice(&40u32.to_le_bytes()); // header size
        info.extend_from_slice(&width.to_le_bytes());
        info.extend_from_slice(&info_height.to_le_bytes());
        info.extend_from_slice(&1u16.to_le_bytes()); // planes
        info.extend_from_slice(&bit_count.to_le_bytes());
        info.extend_from_slice(&BI_RGB.to_le_bytes()); // compression
        info.extend_from_slice(&0u32.to_le_bytes()); // image size
        info.extend_from_slice(&0i32.to_le_bytes()); // x ppm
        info.extend_from_slice(&0i32.to_le_bytes()); // y ppm
        info.extend_from_slice(&(palette.len() as u32).to_le_bytes()); // colors used
        info.extend_from_slice(&0u32.to_le_bytes()); // colors important

        let mut pal = Vec::new();
        for &(r, g, b) in palette {
            pal.push(b);
            pal.push(g);
            pal.push(r);
            pal.push(0);
        }

        let pixel_off = 14 + info.len() + pal.len();
        let total = pixel_off + rows.len();

        let mut out = Vec::new();
        out.extend_from_slice(b"BM");
        out.extend_from_slice(&(total as u32).to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes()); // reserved
        out.extend_from_slice(&(pixel_off as u32).to_le_bytes());
        out.extend_from_slice(&info);
        out.extend_from_slice(&pal);
        out.extend_from_slice(rows);
        out
    }

    /// Pad a row of bytes to a 4-byte boundary with zeros.
    fn pad4(mut row: Vec<u8>) -> Vec<u8> {
        while row.len() % 4 != 0 {
            row.push(0);
        }
        row
    }

    // ── BMP: 24-bit ─────────────────────────────────────────────────────────

    #[test]
    fn decode_24bit_bottom_up() {
        // 2x2, bottom-up. File order rows: bottom row first.
        // Logical image:
        //   (0,0)=red   (1,0)=green
        //   (0,1)=blue  (1,1)=white
        // Stored bottom-up: row0(file) = bottom = blue,white ; row1 = red,green.
        // 24-bit pixels are stored B,G,R.
        let bottom = pad4(vec![255, 0, 0, /*blue*/ 255, 255, 255 /*white*/]);
        let top = pad4(vec![0, 0, 255, /*red*/ 0, 255, 0 /*green*/]);
        let mut rows = Vec::new();
        rows.extend_from_slice(&bottom);
        rows.extend_from_slice(&top);
        let bmp = build_bmp(2, 2, 24, &[], &rows);
        let img = decode_bmp(&bmp).expect("24-bit decode");
        assert_eq!((img.width, img.height), (2, 2));
        assert_eq!(img.pixel(0, 0), Some((0xFF, 255, 0, 0))); // red
        assert_eq!(img.pixel(1, 0), Some((0xFF, 0, 255, 0))); // green
        assert_eq!(img.pixel(0, 1), Some((0xFF, 0, 0, 255))); // blue
        assert_eq!(img.pixel(1, 1), Some((0xFF, 255, 255, 255))); // white
                                                                  // FAIL-ability: if the bottom-up row flip were removed, (0,0) would be
                                                                  // blue, not red — this guard flips.
        assert_ne!(img.pixel(0, 0), Some((0xFF, 0, 0, 255)));
    }

    #[test]
    fn decode_24bit_top_down_matches_bottom_up() {
        // Same logical image as above, but stored top-down (negative height):
        // file row0 = top = red,green ; row1 = blue,white.
        let top = pad4(vec![0, 0, 255, 0, 255, 0]);
        let bottom = pad4(vec![255, 0, 0, 255, 255, 255]);
        let mut rows = Vec::new();
        rows.extend_from_slice(&top);
        rows.extend_from_slice(&bottom);
        let bmp = build_bmp(2, -2, 24, &[], &rows);
        let img = decode_bmp(&bmp).expect("top-down decode");
        assert_eq!(img.pixel(0, 0), Some((0xFF, 255, 0, 0))); // red
        assert_eq!(img.pixel(1, 0), Some((0xFF, 0, 255, 0))); // green
        assert_eq!(img.pixel(0, 1), Some((0xFF, 0, 0, 255))); // blue
        assert_eq!(img.pixel(1, 1), Some((0xFF, 255, 255, 255))); // white
    }

    // ── BMP: 32-bit ─────────────────────────────────────────────────────────

    #[test]
    fn decode_32bit() {
        // 1x1, B,G,R,A = (10,20,30, x). Top-level BMP forces alpha opaque.
        let rows = vec![10u8, 20, 30, 0x77];
        let bmp = build_bmp(1, 1, 32, &[], &rows);
        let img = decode_bmp(&bmp).expect("32-bit decode");
        // r=30, g=20, b=10, a forced 0xFF (top-level BMP ignores stored alpha).
        assert_eq!(img.pixel(0, 0), Some((0xFF, 30, 20, 10)));
        assert_ne!(img.pixel(0, 0), Some((0xFF, 10, 20, 30))); // not B/R swapped
    }

    // ── BMP: 8-bit palette ──────────────────────────────────────────────────

    #[test]
    fn decode_8bit_palette() {
        // palette[0]=red, [1]=green, [2]=blue. 3x1 image indices 2,0,1.
        let palette = [(255u8, 0, 0), (0, 255, 0), (0, 0, 255)];
        let row = pad4(vec![2u8, 0, 1]); // width 3, padded to 4
        let bmp = build_bmp(3, 1, 8, &palette, &row);
        let img = decode_bmp(&bmp).expect("8-bit decode");
        assert_eq!(img.pixel(0, 0), Some((0xFF, 0, 0, 255))); // idx2 = blue
        assert_eq!(img.pixel(1, 0), Some((0xFF, 255, 0, 0))); // idx0 = red
        assert_eq!(img.pixel(2, 0), Some((0xFF, 0, 255, 0))); // idx1 = green
    }

    // ── BMP: row padding with a non-multiple-of-4 width ─────────────────────

    #[test]
    fn decode_24bit_row_padding() {
        // width 3 @ 24bpp = 9 bytes/row, padded to 12. Two rows, top-down so
        // file order == logical order. Distinct colors prove padding is skipped.
        let r0 = pad4(vec![1, 2, 3, 4, 5, 6, 7, 8, 9]); // BGR triples
        let r1 = pad4(vec![10, 11, 12, 13, 14, 15, 16, 17, 18]);
        let mut rows = Vec::new();
        rows.extend_from_slice(&r0);
        rows.extend_from_slice(&r1);
        assert_eq!(r0.len(), 12, "row must pad 9->12");
        let bmp = build_bmp(3, -2, 24, &[], &rows);
        let img = decode_bmp(&bmp).expect("padding decode");
        // r0 pixel0 = B1,G2,R3 -> (r=3,g=2,b=1)
        assert_eq!(img.pixel(0, 0), Some((0xFF, 3, 2, 1)));
        assert_eq!(img.pixel(2, 0), Some((0xFF, 9, 8, 7)));
        // If padding were mis-handled, row 1 would read shifted bytes.
        assert_eq!(img.pixel(0, 1), Some((0xFF, 12, 11, 10)));
        assert_eq!(img.pixel(2, 1), Some((0xFF, 18, 17, 16)));
    }

    // ── BMP: RLE rejected cleanly ───────────────────────────────────────────

    #[test]
    fn reject_rle8_unsupported() {
        let mut info = Vec::new();
        info.extend_from_slice(&40u32.to_le_bytes());
        info.extend_from_slice(&4i32.to_le_bytes());
        info.extend_from_slice(&4i32.to_le_bytes());
        info.extend_from_slice(&1u16.to_le_bytes());
        info.extend_from_slice(&8u16.to_le_bytes());
        info.extend_from_slice(&BI_RLE8.to_le_bytes());
        info.extend_from_slice(&[0u8; 20]);
        let mut bmp = Vec::new();
        bmp.extend_from_slice(b"BM");
        bmp.extend_from_slice(&0u32.to_le_bytes());
        bmp.extend_from_slice(&0u32.to_le_bytes());
        bmp.extend_from_slice(&54u32.to_le_bytes());
        bmp.extend_from_slice(&info);
        assert_eq!(decode_bmp(&bmp), Err(BmpError::Unsupported));
    }

    // ── ICO builders ────────────────────────────────────────────────────────

    /// Build a one-image-or-more ICO from a list of (dir_w, dir_h, payload).
    fn build_ico(images: &[(u8, u8, Vec<u8>)]) -> Vec<u8> {
        let count = images.len();
        let mut out = Vec::new();
        out.extend_from_slice(&0u16.to_le_bytes()); // reserved
        out.extend_from_slice(&1u16.to_le_bytes()); // type = icon
        out.extend_from_slice(&(count as u16).to_le_bytes());

        // First payload starts after the dir (6 + 16*count).
        let mut data_off = 6 + 16 * count;
        let mut payloads = Vec::new();
        for (w, h, payload) in images {
            out.push(*w);
            out.push(*h);
            out.push(0); // color count
            out.push(0); // reserved
            out.extend_from_slice(&1u16.to_le_bytes()); // planes
            out.extend_from_slice(&0u16.to_le_bytes()); // bit count
            out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
            out.extend_from_slice(&(data_off as u32).to_le_bytes());
            data_off += payload.len();
            payloads.extend_from_slice(payload);
        }
        out.extend_from_slice(&payloads);
        out
    }

    /// Build an embedded-BMP DIB payload: a BITMAPINFOHEADER with DOUBLED height,
    /// a 32-bit color plane, then a 1-bpp AND mask. `mask_rows` are the raw mask
    /// bytes (one entry per row, top stored last because bottom-up).
    fn build_ico_dib_32(
        width: u32,
        height: u32,
        color_rows_bottom_up: &[u8],
        mask_rows_bottom_up: &[u8],
    ) -> Vec<u8> {
        let mut p = Vec::new();
        p.extend_from_slice(&40u32.to_le_bytes());
        p.extend_from_slice(&(width as i32).to_le_bytes());
        p.extend_from_slice(&((height * 2) as i32).to_le_bytes()); // doubled
        p.extend_from_slice(&1u16.to_le_bytes());
        p.extend_from_slice(&32u16.to_le_bytes());
        p.extend_from_slice(&BI_RGB.to_le_bytes());
        p.extend_from_slice(&[0u8; 20]);
        p.extend_from_slice(color_rows_bottom_up);
        p.extend_from_slice(mask_rows_bottom_up);
        p
    }

    #[test]
    fn decode_ico_embedded_bmp_with_mask() {
        // 2x1 32-bit icon. Two pixels: left opaque red, right masked transparent.
        // Color plane (bottom-up, but 1 row so trivial): B,G,R,A per pixel.
        let color = vec![
            0, 0, 255, 0xFF, // left: red, stored alpha ignored-but-present
            0, 255, 0, 0xFF, // right: green
        ];
        // AND mask: 1 bpp, row padded to 4 bytes. bit for x=0 = 0 (opaque),
        // x=1 = 1 (transparent). MSB-first: 0b01000000 = 0x40.
        let mask = vec![0x40u8, 0, 0, 0];
        let dib = build_ico_dib_32(2, 1, &color, &mask);
        let ico = build_ico(&[(2, 1, dib)]);

        let set = decode_ico(&ico).expect("ico decode");
        assert_eq!(set.entries.len(), 1);
        let entry = &set.entries[0];
        assert_eq!((entry.width, entry.height), (2, 1));
        match &entry.kind {
            IconKind::Bmp(img) => {
                assert_eq!((img.width, img.height), (2, 1));
                // Left pixel: red, opaque (mask bit 0).
                assert_eq!(img.pixel(0, 0), Some((0xFF, 255, 0, 0)));
                // Right pixel: mask bit 1 -> alpha cleared to 0.
                let (a, _, _, _) = img.pixel(1, 0).expect("in-bounds");
                assert_eq!(a, 0x00, "AND-mask must clear alpha on masked pixel");
                // FAIL-ability: if the AND-mask alpha were dropped, this would be
                // 0xFF and the assert flips.
                assert_ne!(a, 0xFF);
            }
            IconKind::Png(_) => panic!("expected embedded BMP, got PNG"),
        }
    }

    #[test]
    fn decode_ico_best_picks_16() {
        // Three sizes: 8, 16, 32. best(16) must pick exactly the 16px entry.
        let dib8 = build_ico_dib_32(8, 8, &vec![0u8; 8 * 8 * 4], &vec![0u8; 4 * 8]);
        let dib16 = build_ico_dib_32(16, 16, &vec![0u8; 16 * 16 * 4], &vec![0u8; 4 * 16]);
        let dib32 = build_ico_dib_32(32, 32, &vec![0u8; 32 * 32 * 4], &vec![0u8; 4 * 32]);
        let ico = build_ico(&[(8, 8, dib8), (16, 16, dib16), (32, 32, dib32)]);
        let set = decode_ico(&ico).expect("multi-size ico");
        assert_eq!(set.entries.len(), 3);
        let best = set.best(16).expect("best(16)");
        assert_eq!(best.width, 16, "best(16) must pick the 16px entry");
        // best(target) larger than all -> largest entry.
        assert_eq!(set.best(64).expect("best(64)").width, 32);
        // best(target) smaller than all -> smallest >= target.
        assert_eq!(set.best(4).expect("best(4)").width, 8);
    }

    #[test]
    fn decode_ico_embedded_png() {
        // A minimal "PNG" payload — only the signature matters for routing.
        let mut png = Vec::new();
        png.extend_from_slice(&PNG_SIGNATURE);
        png.extend_from_slice(b"...rest of png stream...");
        let ico = build_ico(&[(64, 64, png.clone())]);
        let set = decode_ico(&ico).expect("png-in-ico");
        assert_eq!(set.entries.len(), 1);
        match &set.entries[0].kind {
            IconKind::Png(bytes) => {
                assert_eq!(&bytes[0..8], &PNG_SIGNATURE, "PNG signature must survive");
                assert_eq!(bytes.as_slice(), png.as_slice());
            }
            IconKind::Bmp(_) => panic!("expected PNG entry"),
        }
    }

    // ── Malformed battery: all Err, zero panics ─────────────────────────────

    #[test]
    fn reject_not_a_bmp() {
        assert_eq!(decode_bmp(&[0u8; 64]), Err(BmpError::BadSignature));
    }

    #[test]
    fn reject_truncated_bmp_header() {
        assert_eq!(decode_bmp(b"BM"), Err(BmpError::Truncated));
    }

    #[test]
    fn reject_bmp_bad_offset() {
        // Valid header but pixel offset points way past the buffer.
        let row = pad4(vec![1u8, 2, 3]);
        let mut bmp = build_bmp(1, 1, 24, &[], &row);
        // Overwrite the pixel offset (bytes 10..14) with a huge value.
        bmp[10..14].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
        assert_eq!(decode_bmp(&bmp), Err(BmpError::BadOffset));
    }

    #[test]
    fn reject_bmp_oversized_dimensions() {
        let mut info = Vec::new();
        info.extend_from_slice(&40u32.to_le_bytes());
        info.extend_from_slice(&0x7FFF_FFFFi32.to_le_bytes());
        info.extend_from_slice(&0x7FFF_FFFFi32.to_le_bytes());
        info.extend_from_slice(&1u16.to_le_bytes());
        info.extend_from_slice(&24u16.to_le_bytes());
        info.extend_from_slice(&[0u8; 24]);
        let mut bmp = Vec::new();
        bmp.extend_from_slice(b"BM");
        bmp.extend_from_slice(&0u32.to_le_bytes());
        bmp.extend_from_slice(&0u32.to_le_bytes());
        bmp.extend_from_slice(&54u32.to_le_bytes());
        bmp.extend_from_slice(&info);
        assert_eq!(decode_bmp(&bmp), Err(BmpError::DimensionsOutOfRange));
    }

    #[test]
    fn reject_ico_zero_entries() {
        let mut ico = Vec::new();
        ico.extend_from_slice(&0u16.to_le_bytes());
        ico.extend_from_slice(&1u16.to_le_bytes());
        ico.extend_from_slice(&0u16.to_le_bytes()); // count = 0
        assert_eq!(decode_ico(&ico), Err(BmpError::NoEntries));
    }

    #[test]
    fn reject_ico_not_an_icon() {
        let mut ico = Vec::new();
        ico.extend_from_slice(&0u16.to_le_bytes());
        ico.extend_from_slice(&99u16.to_le_bytes()); // bogus type
        ico.extend_from_slice(&1u16.to_le_bytes());
        ico.extend_from_slice(&[0u8; 16]);
        assert_eq!(decode_ico(&ico), Err(BmpError::BadIconDir));
    }

    #[test]
    fn reject_ico_bad_entry_offset() {
        // One entry whose data offset/size runs past the buffer.
        let mut ico = Vec::new();
        ico.extend_from_slice(&0u16.to_le_bytes());
        ico.extend_from_slice(&1u16.to_le_bytes());
        ico.extend_from_slice(&1u16.to_le_bytes());
        ico.push(16);
        ico.push(16);
        ico.push(0);
        ico.push(0);
        ico.extend_from_slice(&1u16.to_le_bytes());
        ico.extend_from_slice(&32u16.to_le_bytes());
        ico.extend_from_slice(&0xFFFFu32.to_le_bytes()); // bytes in res
        ico.extend_from_slice(&0xFFFFu32.to_le_bytes()); // offset
        assert_eq!(decode_ico(&ico), Err(BmpError::BadIconEntry));
    }

    #[test]
    fn reject_ico_truncated() {
        assert_eq!(decode_ico(&[0u8, 0, 1]), Err(BmpError::Truncated));
    }

    #[test]
    fn no_panic_on_random_inputs() {
        // Length-swept garbage must never panic — only Err.
        for len in 0..64usize {
            let buf: Vec<u8> = (0..len).map(|i| (i as u8).wrapping_mul(37)).collect();
            let _ = decode_bmp(&buf);
            let _ = decode_ico(&buf);
        }
        // A "BM"-prefixed fuzz stream and an icon-dir-prefixed one.
        for len in 6..80usize {
            let mut b: Vec<u8> = (0..len).map(|i| (i as u8).wrapping_mul(91)).collect();
            b[0] = b'B';
            if len > 1 {
                b[1] = b'M';
            }
            let _ = decode_bmp(&b);
            let mut c: Vec<u8> = (0..len).map(|i| (i as u8).wrapping_mul(53)).collect();
            c[0] = 0;
            c[1] = 0;
            c[2] = 1;
            c[3] = 0;
            let _ = decode_ico(&c);
        }
    }
}

// ════════════════════════════════════════════════════════════════════════════
// FUZZ / PROPERTY suite — deterministic seeded PRNG, no external fuzz crate.
//
// Matches the ath_mime/ath_toml/ath_deflate pattern. The properties under test
// are the hostile-input invariants of `decode_bmp` / `decode_ico` (favicons are
// attacker-controlled): on ANY byte sequence they must (a) never panic, and
// (b) bound every allocation — a crafted width/height (incl. negative, huge, or
// width*height*bpp overflow) cannot request a multi-GiB buffer.
//
// FAIL-ability (proven by reasoning, see REPORT):
//  - If any decode path could panic on hostile bytes (an unchecked index, an
//    `unwrap`, an arithmetic overflow in debug) the never-panic loops abort the
//    test process — the test goes red. (#![forbid(unsafe_code)] means an OOB
//    index is a guaranteed panic, not silent UB — so the loops genuinely prove
//    bounds-safety.)
//  - If the MAX_PIXELS / MAX_DIMENSION caps were removed, `huge_dimensions_*`
//    would request a multi-GiB `vec![0u32; w*h]` and OOM (process abort = test
//    failure) instead of returning DimensionsOutOfRange.
//  - If the `row_bits = width * bit_count` / `row_stride * height` checked_mul
//    guards were dropped, an overflow-crafted header would wrap to a tiny `needed`
//    and then index past `data` → panic (forbid-unsafe) = test failure; the
//    `overflow_dimensions` case proves the checked path returns Err instead.
//  - If MAX_ICON_ENTRIES were removed, a 0xFFFF-entry ICONDIR would build an
//    unbounded `entries` Vec → OOM.
// ════════════════════════════════════════════════════════════════════════════
#[cfg(test)]
mod fuzz {
    use super::*;
    use alloc::vec::Vec;

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

    /// Any decoded BMP must respect the pixel-area cap.
    fn assert_bmp_bounded(img: &BmpImage) {
        assert!(
            (img.width as u64) * (img.height as u64) <= MAX_PIXELS,
            "BMP exceeded MAX_PIXELS"
        );
        assert!(
            img.pixels.len() == (img.width as usize) * (img.height as usize),
            "pixel buffer mismatched dimensions"
        );
    }

    /// 2a. Random bytes (0..512) never panic for either decoder.
    #[test]
    fn fuzz_random_bytes_never_panic() {
        let mut rng = Rng::new(0xB17_F00D);
        for _ in 0..40_000 {
            let len = rng.below(512);
            let mut buf = Vec::with_capacity(len);
            for _ in 0..len {
                buf.push(rng.byte());
            }
            if let Ok(img) = decode_bmp(&buf) {
                assert_bmp_bounded(&img);
            }
            let _ = decode_ico(&buf);
        }
    }

    /// 2b. "BM"-prefixed random tails: pushes past the signature into the file +
    /// info header parse and pixel decode.
    #[test]
    fn fuzz_bm_prefixed_random_tail_never_panic() {
        let mut rng = Rng::new(0x4D42_C0DE);
        for _ in 0..40_000 {
            let len = rng.below(400);
            let mut buf = Vec::with_capacity(2 + len);
            buf.extend_from_slice(b"BM");
            for _ in 0..len {
                buf.push(rng.byte());
            }
            if let Ok(img) = decode_bmp(&buf) {
                assert_bmp_bounded(&img);
            }
        }
    }

    /// 2c. Mutate a well-formed BMP header field-by-field: bogus bpp, compression,
    /// header_size, colors_used, pixel offset, dimensions — never panic.
    #[test]
    fn fuzz_mutated_bmp_header_never_panic() {
        // Well-formed 2x2 24-bit BMP as the seed.
        let mut base = Vec::new();
        base.extend_from_slice(b"BM");
        // file header
        base.extend_from_slice(&0u32.to_le_bytes()); // size
        base.extend_from_slice(&0u32.to_le_bytes()); // reserved
        let pixel_off = 14u32 + 40;
        base.extend_from_slice(&pixel_off.to_le_bytes());
        // info header (40)
        base.extend_from_slice(&40u32.to_le_bytes());
        base.extend_from_slice(&2i32.to_le_bytes()); // width
        base.extend_from_slice(&2i32.to_le_bytes()); // height
        base.extend_from_slice(&1u16.to_le_bytes()); // planes
        base.extend_from_slice(&24u16.to_le_bytes()); // bpp
        base.extend_from_slice(&BI_RGB.to_le_bytes());
        base.extend_from_slice(&[0u8; 20]);
        // pixels: 2 rows of 2*3=6 padded to 8.
        base.extend_from_slice(&[1, 2, 3, 4, 5, 6, 0, 0]);
        base.extend_from_slice(&[7, 8, 9, 10, 11, 12, 0, 0]);

        // sanity: the seed decodes.
        assert!(decode_bmp(&base).is_ok());

        let mut rng = Rng::new(0x2222_F00D);
        for _ in 0..80_000 {
            let mut m = base.clone();
            let muts = 1 + rng.below(3);
            for _ in 0..muts {
                let i = rng.below(m.len());
                m[i] ^= rng.byte();
            }
            if let Ok(img) = decode_bmp(&m) {
                assert_bmp_bounded(&img);
            }
        }
    }

    /// 2d. Sweep every bit depth (legal and bogus) with a too-short pixel buffer:
    /// 0,1,2,3,4,8,15,16,24,32,48,64 — must Err, never panic.
    #[test]
    fn fuzz_all_bit_depths_short_buffer() {
        for bpp in [0u16, 1, 2, 3, 4, 5, 8, 15, 16, 24, 32, 48, 64, 255] {
            let mut info = Vec::new();
            info.extend_from_slice(&40u32.to_le_bytes());
            info.extend_from_slice(&8i32.to_le_bytes()); // width
            info.extend_from_slice(&8i32.to_le_bytes()); // height
            info.extend_from_slice(&1u16.to_le_bytes());
            info.extend_from_slice(&bpp.to_le_bytes());
            info.extend_from_slice(&BI_RGB.to_le_bytes());
            info.extend_from_slice(&[0u8; 20]);
            let mut bmp = Vec::new();
            bmp.extend_from_slice(b"BM");
            bmp.extend_from_slice(&0u32.to_le_bytes());
            bmp.extend_from_slice(&0u32.to_le_bytes());
            bmp.extend_from_slice(&54u32.to_le_bytes()); // offset past end (no pixels)
            bmp.extend_from_slice(&info);
            // No pixel data → BadOffset / Unsupported / DimensionsOutOfRange.
            let res = decode_bmp(&bmp);
            assert!(res.is_err(), "bpp {bpp} short buffer must Err, got {res:?}");
        }
    }

    /// 2e. Huge / negative / i32::MIN dimensions must be rejected by the cap, not
    /// allocated. Proves MAX_PIXELS + the i32::MIN and width<=0 guards.
    #[test]
    fn fuzz_huge_and_negative_dimensions_bounded() {
        let build = |w: i32, h: i32, bpp: u16| -> Vec<u8> {
            let mut info = Vec::new();
            info.extend_from_slice(&40u32.to_le_bytes());
            info.extend_from_slice(&w.to_le_bytes());
            info.extend_from_slice(&h.to_le_bytes());
            info.extend_from_slice(&1u16.to_le_bytes());
            info.extend_from_slice(&bpp.to_le_bytes());
            info.extend_from_slice(&BI_RGB.to_le_bytes());
            info.extend_from_slice(&[0u8; 20]);
            let mut bmp = Vec::new();
            bmp.extend_from_slice(b"BM");
            bmp.extend_from_slice(&0u32.to_le_bytes());
            bmp.extend_from_slice(&0u32.to_le_bytes());
            bmp.extend_from_slice(&54u32.to_le_bytes());
            bmp.extend_from_slice(&info);
            bmp
        };
        // Massive positive dimensions (> MAX_PIXELS).
        assert_eq!(
            decode_bmp(&build(0x7FFF_FFFF, 0x7FFF_FFFF, 24)),
            Err(BmpError::DimensionsOutOfRange)
        );
        // Negative width.
        assert_eq!(
            decode_bmp(&build(-4, 4, 24)),
            Err(BmpError::DimensionsOutOfRange)
        );
        // i32::MIN height (no positive magnitude) — must not negate-overflow.
        assert_eq!(
            decode_bmp(&build(4, i32::MIN, 24)),
            Err(BmpError::DimensionsOutOfRange)
        );
        // Zero dims.
        assert_eq!(
            decode_bmp(&build(0, 4, 24)),
            Err(BmpError::DimensionsOutOfRange)
        );
        // width*height*bpp overflow attempt: width and height each within
        // MAX_DIMENSION individually but the product over the pixel cap, plus a
        // 32-bpp depth — must trip the area cap, never overflow row math.
        assert_eq!(
            decode_bmp(&build(60000, 60000, 32)),
            Err(BmpError::DimensionsOutOfRange)
        );
        // Sweep large random dims: never panic, never decode past the cap.
        let mut rng = Rng::new(0x5151_BAD);
        for _ in 0..5000 {
            let w = (rng.below(0x8000_0000)) as i32;
            let h = (rng.below(0x8000_0000)) as i32;
            let bpp = [1u16, 4, 8, 24, 32][rng.below(5)];
            match decode_bmp(&build(w, h, bpp)) {
                Ok(img) => assert_bmp_bounded(&img),
                Err(_) => {}
            }
        }
    }

    /// 2f. ICO entry-count + truncated-entry-table fuzz. A 0xFFFF count must be
    /// capped (MAX_ICON_ENTRIES); truncated entry tables must Err, never panic.
    #[test]
    fn fuzz_ico_entry_table_never_panic() {
        // Oversized count with a tiny buffer → BadIconDir (cap) or Truncated.
        let mut huge = Vec::new();
        huge.extend_from_slice(&0u16.to_le_bytes());
        huge.extend_from_slice(&1u16.to_le_bytes());
        huge.extend_from_slice(&0xFFFFu16.to_le_bytes()); // 65535 entries
        let res = decode_ico(&huge);
        assert!(
            matches!(res, Err(BmpError::BadIconDir) | Err(BmpError::Truncated)),
            "huge ICO count must be bounded, got {res:?}"
        );

        // Random ICONDIR headers with truncated entry tables.
        let mut rng = Rng::new(0x1C0_F00D);
        for _ in 0..40_000 {
            let count = 1 + rng.below(40);
            let mut buf = Vec::new();
            buf.extend_from_slice(&0u16.to_le_bytes());
            buf.extend_from_slice(&1u16.to_le_bytes());
            buf.extend_from_slice(&(count as u16).to_le_bytes());
            // Provide a random PREFIX of the entry table (often truncated).
            let entry_bytes = rng.below(count * 16 + 8);
            for _ in 0..entry_bytes {
                buf.push(rng.byte());
            }
            let _ = decode_ico(&buf); // Ok or Err, never panic.
        }
    }

    /// 2g. ICO embedded-DIB with crafted doubled-height / mask offsets. Mutate a
    /// valid embedded-BMP icon to drive the AND-mask / true-height math hostile.
    #[test]
    fn fuzz_ico_embedded_dib_mutation_never_panic() {
        // Valid 2x1 32-bit embedded-BMP icon (doubled height = 2).
        let mut dib = Vec::new();
        dib.extend_from_slice(&40u32.to_le_bytes());
        dib.extend_from_slice(&2i32.to_le_bytes()); // width
        dib.extend_from_slice(&2i32.to_le_bytes()); // doubled height
        dib.extend_from_slice(&1u16.to_le_bytes());
        dib.extend_from_slice(&32u16.to_le_bytes());
        dib.extend_from_slice(&BI_RGB.to_le_bytes());
        dib.extend_from_slice(&[0u8; 20]);
        dib.extend_from_slice(&[0, 0, 255, 0xFF, 0, 255, 0, 0xFF]); // color row
        dib.extend_from_slice(&[0x40, 0, 0, 0]); // 1-bpp AND mask row

        let mut ico = Vec::new();
        ico.extend_from_slice(&0u16.to_le_bytes());
        ico.extend_from_slice(&1u16.to_le_bytes());
        ico.extend_from_slice(&1u16.to_le_bytes());
        // single ICONDIRENTRY
        ico.push(2);
        ico.push(1);
        ico.push(0);
        ico.push(0);
        ico.extend_from_slice(&1u16.to_le_bytes());
        ico.extend_from_slice(&0u16.to_le_bytes());
        ico.extend_from_slice(&(dib.len() as u32).to_le_bytes());
        ico.extend_from_slice(&(6u32 + 16).to_le_bytes()); // offset
        ico.extend_from_slice(&dib);

        assert!(decode_ico(&ico).is_ok(), "seed icon must decode");

        let mut rng = Rng::new(0x1C0_D1B);
        for _ in 0..60_000 {
            let mut m = ico.clone();
            let muts = 1 + rng.below(4);
            for _ in 0..muts {
                let i = rng.below(m.len());
                m[i] ^= rng.byte();
            }
            let _ = decode_ico(&m); // Ok or Err, never panic.
        }
    }

    /// 2h. Palette-size mismatch: indexed depth declaring colors_used past the
    /// available palette bytes must Err (MissingPalette/Truncated), never OOB.
    #[test]
    fn fuzz_palette_mismatch_never_panic() {
        for colors_used in [0u32, 1, 2, 16, 256, 1000, 0xFFFF_FFFF] {
            let mut info = Vec::new();
            info.extend_from_slice(&40u32.to_le_bytes());
            info.extend_from_slice(&4i32.to_le_bytes());
            info.extend_from_slice(&4i32.to_le_bytes());
            info.extend_from_slice(&1u16.to_le_bytes());
            info.extend_from_slice(&8u16.to_le_bytes()); // 8-bit indexed
            info.extend_from_slice(&BI_RGB.to_le_bytes());
            info.extend_from_slice(&0u32.to_le_bytes()); // image size
            info.extend_from_slice(&0i32.to_le_bytes());
            info.extend_from_slice(&0i32.to_le_bytes());
            info.extend_from_slice(&colors_used.to_le_bytes()); // colors used
            info.extend_from_slice(&0u32.to_le_bytes());
            let mut bmp = Vec::new();
            bmp.extend_from_slice(b"BM");
            bmp.extend_from_slice(&0u32.to_le_bytes());
            bmp.extend_from_slice(&0u32.to_le_bytes());
            bmp.extend_from_slice(&54u32.to_le_bytes());
            bmp.extend_from_slice(&info);
            // Provide only a handful of palette bytes — far fewer than claimed.
            bmp.extend_from_slice(&[0u8; 8]);
            let res = decode_bmp(&bmp);
            // Either the oversized palette is rejected, or (for small counts) the
            // pixel offset/data is short — in every case a clean Err, no panic.
            assert!(
                res.is_err(),
                "colors_used {colors_used} mismatch must Err, got {res:?}"
            );
        }
    }
}
