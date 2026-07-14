//! # RaeMedia PNG decoder — pixels, not hex.
//!
//! RaeenOS_Concept.md (§creators / media): the OS must let people "play my movies,
//! show my photos." A computer that can't render a photo library isn't a daily
//! driver. Today the Files-app Quick Look only reports a PNG's dimensions and a hex
//! dump — this module is the foundation that turns that into actual pixels, and the
//! engine a future Photos viewer will sit on.
//!
//! This is a **from-scratch** PNG decoder: PNG signature + IHDR, IDAT
//! concatenation, a self-contained zlib/DEFLATE `inflate` (fixed + dynamic Huffman,
//! no external crate), all five scanline filters (None/Sub/Up/Average/Paeth), and
//! output to a flat ARGB8888 `Vec<u32>` buffer (the compositor/Canvas pixel format).
//!
//! ## Hostile-input posture (CLAUDE: decoders are the #1 RCE surface)
//! Every media file is treated as hostile. There is **no `unwrap`/`expect`/`panic`
//! path** reachable from `decode_png`: malformed signatures, truncated chunks, bad
//! lengths, oversized dimensions, and corrupt DEFLATE streams all return `Err(...)`.
//! Memory is bounded up front (`MAX_DIMENSION`, `MAX_PIXELS`) so a crafted IHDR can't
//! request a multi-gigabyte allocation. The host KAT suite at the bottom of this file
//! is the primary proof (run `cargo test -p raemedia`).
//!
//! Supported color types (8-bit depth):
//! - Type 0  — Grayscale
//! - Type 2  — Truecolor (RGB)
//! - Type 3  — Indexed (palette, with optional `tRNS` alpha)
//! - Type 4  — Grayscale + alpha
//! - Type 6  — Truecolor + alpha (RGBA)

extern crate alloc;

use alloc::vec;
use alloc::vec::Vec;

/// Bound on either image axis. 1<<16 keeps a single allocation well under the
/// `MAX_PIXELS` cap and matches the practical ceiling of real photo formats.
const MAX_DIMENSION: u32 = 1 << 16; // 65_536
/// Bound on total pixel count (width * height). ~67M px = 256 MiB at 4 B/px ARGB.
/// A crafted IHDR claiming billions of pixels is rejected before any allocation.
const MAX_PIXELS: u64 = 64 * 1024 * 1024;

/// PNG decode error. Every variant is a *handled* error path — nothing here panics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PngError {
    /// First 8 bytes are not the PNG signature.
    BadSignature,
    /// Stream ended mid-chunk / mid-field.
    Truncated,
    /// A chunk length field points past the end of the buffer.
    BadChunkLength,
    /// First chunk was not IHDR, or IHDR was malformed.
    BadIhdr,
    /// Width/height is zero or exceeds the memory bound.
    DimensionsOutOfRange,
    /// Bit depth / color type combination this decoder does not implement.
    UnsupportedFormat,
    /// No IDAT data present.
    NoImageData,
    /// zlib header was malformed (bad CMF/FLG, unsupported method).
    BadZlibHeader,
    /// DEFLATE stream was corrupt or truncated.
    InflateError,
    /// Decompressed data length did not match the expected raw image size.
    SizeMismatch,
    /// A palette index referenced a color outside the PLTE table.
    BadPaletteIndex,
    /// PLTE chunk required (color type 3) but missing.
    MissingPalette,
}

/// A decoded image: a flat ARGB8888 buffer plus dimensions.
///
/// `pixels.len() == (width * height) as usize`. Each `u32` is `0xAARRGGBB`,
/// the format the RaeGFX compositor/Canvas consumes directly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedImage {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u32>,
}

impl DecodedImage {
    /// Sample a pixel as `(a, r, g, b)`. Returns `None` out of bounds — callers in
    /// tests use this so a wrong coordinate is a clean failure, not a panic.
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

// ─── PNG container parse ────────────────────────────────────────────────────

const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

struct Ihdr {
    width: u32,
    height: u32,
    bit_depth: u8,
    color_type: u8,
    interlace: u8,
}

fn read_u32_be(buf: &[u8], off: usize) -> Option<u32> {
    let b = buf.get(off..off + 4)?;
    Some(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}

fn parse_ihdr(data: &[u8]) -> Result<Ihdr, PngError> {
    // data is the 13-byte IHDR payload.
    if data.len() != 13 {
        return Err(PngError::BadIhdr);
    }
    let width = u32::from_be_bytes([data[0], data[1], data[2], data[3]]);
    let height = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);
    let bit_depth = data[8];
    let color_type = data[9];
    let compression = data[10];
    let filter = data[11];
    let interlace = data[12];

    if width == 0 || height == 0 {
        return Err(PngError::DimensionsOutOfRange);
    }
    if width > MAX_DIMENSION || height > MAX_DIMENSION {
        return Err(PngError::DimensionsOutOfRange);
    }
    if (width as u64) * (height as u64) > MAX_PIXELS {
        return Err(PngError::DimensionsOutOfRange);
    }
    // Only DEFLATE compression (0) and adaptive filtering (0) are valid PNG.
    if compression != 0 || filter != 0 {
        return Err(PngError::BadIhdr);
    }
    Ok(Ihdr {
        width,
        height,
        bit_depth,
        color_type,
        interlace,
    })
}

/// Channels per pixel for a given color type (before palette expansion).
fn channels_for(color_type: u8) -> Option<usize> {
    match color_type {
        0 => Some(1), // grayscale
        2 => Some(3), // truecolor RGB
        3 => Some(1), // indexed (one index byte per pixel)
        4 => Some(2), // grayscale + alpha
        6 => Some(4), // truecolor + alpha
        _ => None,
    }
}

/// Decode a PNG byte stream into an ARGB8888 image.
///
/// Hostile-input safe: returns `Err` (never panics) on any malformed input.
pub fn decode_png(data: &[u8]) -> Result<DecodedImage, PngError> {
    if data.len() < 8 || data[0..8] != PNG_SIGNATURE {
        return Err(PngError::BadSignature);
    }

    let mut off = 8usize;
    let mut ihdr: Option<Ihdr> = None;
    let mut idat: Vec<u8> = Vec::new();
    let mut palette: Vec<(u8, u8, u8)> = Vec::new();
    let mut trns: Vec<u8> = Vec::new();
    let mut seen_iend = false;

    while off + 8 <= data.len() {
        let len = read_u32_be(data, off).ok_or(PngError::Truncated)? as usize;
        let ctype = data.get(off + 4..off + 8).ok_or(PngError::Truncated)?;
        let chunk_type = [ctype[0], ctype[1], ctype[2], ctype[3]];
        let data_start = off + 8;
        // length + 4-byte CRC must fit.
        let data_end = data_start
            .checked_add(len)
            .ok_or(PngError::BadChunkLength)?;
        let crc_end = data_end.checked_add(4).ok_or(PngError::BadChunkLength)?;
        if crc_end > data.len() {
            return Err(PngError::BadChunkLength);
        }
        let payload = &data[data_start..data_end];

        match &chunk_type {
            b"IHDR" => {
                if ihdr.is_some() {
                    return Err(PngError::BadIhdr);
                }
                ihdr = Some(parse_ihdr(payload)?);
            }
            b"PLTE" => {
                if payload.len() % 3 != 0 {
                    return Err(PngError::BadIhdr);
                }
                palette = payload
                    .chunks_exact(3)
                    .map(|c| (c[0], c[1], c[2]))
                    .collect();
            }
            b"tRNS" => {
                trns = payload.to_vec();
            }
            b"IDAT" => {
                idat.extend_from_slice(payload);
            }
            b"IEND" => {
                seen_iend = true;
                break;
            }
            _ => {
                // Ancillary chunk we don't need (tEXt, pHYs, gAMA, ...): skip.
            }
        }

        off = crc_end;
    }

    let ihdr = ihdr.ok_or(PngError::BadIhdr)?;
    if !seen_iend && idat.is_empty() {
        return Err(PngError::NoImageData);
    }
    if idat.is_empty() {
        return Err(PngError::NoImageData);
    }
    if ihdr.interlace != 0 {
        // Adam7 interlacing is a rare follow-up; reject cleanly rather than
        // silently producing garbage.
        return Err(PngError::UnsupportedFormat);
    }
    if ihdr.bit_depth != 8 {
        // This slice scopes to 8-bit depth (the common case). 1/2/4/16-bit later.
        return Err(PngError::UnsupportedFormat);
    }
    let channels = channels_for(ihdr.color_type).ok_or(PngError::UnsupportedFormat)?;
    if ihdr.color_type == 3 && palette.is_empty() {
        return Err(PngError::MissingPalette);
    }

    // zlib stream: 2-byte header, DEFLATE body, 4-byte Adler32 (we don't verify it).
    let raw = zlib_inflate(&idat)?;

    let width = ihdr.width as usize;
    let height = ihdr.height as usize;
    let bytes_per_pixel = channels; // 8-bit depth → 1 byte/channel
    let stride = width * bytes_per_pixel;
    // Each scanline is prefixed with a 1-byte filter type.
    let expected = (stride + 1) * height;
    if raw.len() < expected {
        return Err(PngError::SizeMismatch);
    }

    let unfiltered = unfilter(&raw, width, height, bytes_per_pixel)?;

    // Map to ARGB8888.
    let mut pixels = vec![0u32; width * height];
    match ihdr.color_type {
        0 => {
            // grayscale
            for (i, px) in pixels.iter_mut().enumerate() {
                let g = unfiltered[i];
                *px = argb(0xFF, g, g, g);
            }
        }
        2 => {
            // RGB
            for i in 0..width * height {
                let r = unfiltered[i * 3];
                let g = unfiltered[i * 3 + 1];
                let b = unfiltered[i * 3 + 2];
                pixels[i] = argb(0xFF, r, g, b);
            }
        }
        3 => {
            // indexed
            for i in 0..width * height {
                let idx = unfiltered[i] as usize;
                let (r, g, b) = *palette.get(idx).ok_or(PngError::BadPaletteIndex)?;
                let a = trns.get(idx).copied().unwrap_or(0xFF);
                pixels[i] = argb(a, r, g, b);
            }
        }
        4 => {
            // grayscale + alpha
            for i in 0..width * height {
                let g = unfiltered[i * 2];
                let a = unfiltered[i * 2 + 1];
                pixels[i] = argb(a, g, g, g);
            }
        }
        6 => {
            // RGBA
            for i in 0..width * height {
                let r = unfiltered[i * 4];
                let g = unfiltered[i * 4 + 1];
                let b = unfiltered[i * 4 + 2];
                let a = unfiltered[i * 4 + 3];
                pixels[i] = argb(a, r, g, b);
            }
        }
        _ => return Err(PngError::UnsupportedFormat),
    }

    Ok(DecodedImage {
        width: ihdr.width,
        height: ihdr.height,
        pixels,
    })
}

// ─── Scanline un-filtering (PNG spec §9) ────────────────────────────────────

#[inline]
fn paeth_predictor(a: i32, b: i32, c: i32) -> i32 {
    // a = left, b = above, c = upper-left
    let p = a + b - c;
    let pa = (p - a).abs();
    let pb = (p - b).abs();
    let pc = (p - c).abs();
    if pa <= pb && pa <= pc {
        a
    } else if pb <= pc {
        b
    } else {
        c
    }
}

/// Reverse the 5 PNG filters, producing `width*height*bpp` raw pixel bytes.
fn unfilter(raw: &[u8], width: usize, height: usize, bpp: usize) -> Result<Vec<u8>, PngError> {
    let stride = width * bpp;
    let mut out = vec![0u8; stride * height];
    // We process line by line; `prev` is the already-unfiltered previous line.
    let mut prev = vec![0u8; stride];

    let mut src = 0usize;
    for y in 0..height {
        let filter = *raw.get(src).ok_or(PngError::SizeMismatch)?;
        src += 1;
        let line = raw.get(src..src + stride).ok_or(PngError::SizeMismatch)?;
        src += stride;

        let cur = &mut out[y * stride..(y + 1) * stride];
        match filter {
            0 => {
                // None
                cur.copy_from_slice(line);
            }
            1 => {
                // Sub: cur[x] = line[x] + cur[x-bpp]
                for x in 0..stride {
                    let left = if x >= bpp { cur[x - bpp] } else { 0 };
                    cur[x] = line[x].wrapping_add(left);
                }
            }
            2 => {
                // Up: cur[x] = line[x] + prev[x]
                for x in 0..stride {
                    cur[x] = line[x].wrapping_add(prev[x]);
                }
            }
            3 => {
                // Average: cur[x] = line[x] + floor((left + up)/2)
                for x in 0..stride {
                    let left = if x >= bpp { cur[x - bpp] as u32 } else { 0 };
                    let up = prev[x] as u32;
                    cur[x] = line[x].wrapping_add(((left + up) / 2) as u8);
                }
            }
            4 => {
                // Paeth
                for x in 0..stride {
                    let left = if x >= bpp { cur[x - bpp] as i32 } else { 0 };
                    let up = prev[x] as i32;
                    let up_left = if x >= bpp { prev[x - bpp] as i32 } else { 0 };
                    let pred = paeth_predictor(left, up, up_left) as u8;
                    cur[x] = line[x].wrapping_add(pred);
                }
            }
            _ => return Err(PngError::InflateError),
        }
        prev.copy_from_slice(cur);
    }
    Ok(out)
}

// ─── zlib / DEFLATE inflate (RFC 1950 / 1951) ───────────────────────────────

/// Strip the zlib wrapper and inflate the DEFLATE body.
fn zlib_inflate(data: &[u8]) -> Result<Vec<u8>, PngError> {
    if data.len() < 2 {
        return Err(PngError::BadZlibHeader);
    }
    let cmf = data[0];
    let flg = data[1];
    let cm = cmf & 0x0F;
    if cm != 8 {
        return Err(PngError::BadZlibHeader); // only DEFLATE
    }
    // (cmf << 8 | flg) must be a multiple of 31.
    if ((cmf as u16) << 8 | flg as u16) % 31 != 0 {
        return Err(PngError::BadZlibHeader);
    }
    // FDICT bit set means a preset dictionary follows — not used by PNG.
    let fdict = (flg & 0x20) != 0;
    let body_start = if fdict { 6 } else { 2 };
    if data.len() < body_start {
        return Err(PngError::BadZlibHeader);
    }
    inflate(&data[body_start..])
}

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

    /// Read a single bit (DEFLATE packs LSB-first).
    fn bit(&mut self) -> Result<u32, PngError> {
        let byte = *self.data.get(self.byte_pos).ok_or(PngError::InflateError)?;
        let b = (byte >> self.bit_pos) & 1;
        self.bit_pos += 1;
        if self.bit_pos == 8 {
            self.bit_pos = 0;
            self.byte_pos += 1;
        }
        Ok(b as u32)
    }

    /// Read `n` bits LSB-first into a value.
    fn bits(&mut self, n: u32) -> Result<u32, PngError> {
        let mut v = 0u32;
        for i in 0..n {
            v |= self.bit()? << i;
        }
        Ok(v)
    }

    /// Discard bits to the next byte boundary (for stored/uncompressed blocks).
    fn align_to_byte(&mut self) {
        if self.bit_pos != 0 {
            self.bit_pos = 0;
            self.byte_pos += 1;
        }
    }

    fn read_byte(&mut self) -> Result<u8, PngError> {
        let b = *self.data.get(self.byte_pos).ok_or(PngError::InflateError)?;
        self.byte_pos += 1;
        Ok(b)
    }
}

/// Canonical Huffman decoder built from a list of code lengths.
struct Huffman {
    // For each symbol-bit-length, the first canonical code and the symbol offset.
    counts: [u16; 16],
    symbols: Vec<u16>,
}

impl Huffman {
    fn from_lengths(lengths: &[u8]) -> Result<Self, PngError> {
        let mut counts = [0u16; 16];
        for &l in lengths {
            if l as usize > 15 {
                return Err(PngError::InflateError);
            }
            counts[l as usize] += 1;
        }
        counts[0] = 0; // length-0 symbols don't participate
                       // Build the sorted symbol table (offsets per length).
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
                symbols[idx] = sym as u16;
                offsets[l as usize] += 1;
            }
        }
        Ok(Self { counts, symbols })
    }

    /// Decode one symbol from the bit stream.
    fn decode(&self, br: &mut BitReader) -> Result<u16, PngError> {
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
                    .ok_or(PngError::InflateError);
            }
            index += count;
            first += count;
            first <<= 1;
            code <<= 1;
        }
        Err(PngError::InflateError)
    }
}

// Length / distance base tables (RFC 1951 §3.2.5).
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

fn inflate(data: &[u8]) -> Result<Vec<u8>, PngError> {
    let mut br = BitReader::new(data);
    let mut out: Vec<u8> = Vec::new();

    loop {
        let bfinal = br.bit()?;
        let btype = br.bits(2)?;
        match btype {
            0 => inflate_stored(&mut br, &mut out)?,
            1 => inflate_block(&mut br, &mut out, &fixed_litlen()?, &fixed_dist()?)?,
            2 => {
                let (litlen, dist) = read_dynamic_tables(&mut br)?;
                inflate_block(&mut br, &mut out, &litlen, &dist)?;
            }
            _ => return Err(PngError::InflateError), // btype 3 is reserved
        }
        if bfinal == 1 {
            break;
        }
        // Bound output to the memory cap regardless of stream claims.
        if out.len() as u64 > MAX_PIXELS * 5 {
            return Err(PngError::InflateError);
        }
    }
    Ok(out)
}

fn inflate_stored(br: &mut BitReader, out: &mut Vec<u8>) -> Result<(), PngError> {
    br.align_to_byte();
    let len = (br.read_byte()? as u16) | ((br.read_byte()? as u16) << 8);
    let nlen = (br.read_byte()? as u16) | ((br.read_byte()? as u16) << 8);
    if len != !nlen {
        return Err(PngError::InflateError);
    }
    for _ in 0..len {
        out.push(br.read_byte()?);
    }
    Ok(())
}

fn fixed_litlen() -> Result<Huffman, PngError> {
    // RFC 1951 §3.2.6 fixed literal/length code lengths.
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

fn fixed_dist() -> Result<Huffman, PngError> {
    let lengths = [5u8; 30];
    Huffman::from_lengths(&lengths)
}

fn read_dynamic_tables(br: &mut BitReader) -> Result<(Huffman, Huffman), PngError> {
    let hlit = br.bits(5)? as usize + 257;
    let hdist = br.bits(5)? as usize + 1;
    let hclen = br.bits(4)? as usize + 4;
    if hlit > 286 || hdist > 30 {
        return Err(PngError::InflateError);
    }

    // Code-length code lengths come in this permuted order.
    const ORDER: [usize; 19] = [
        16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
    ];
    let mut cl_lengths = [0u8; 19];
    for i in 0..hclen {
        cl_lengths[ORDER[i]] = br.bits(3)? as u8;
    }
    let cl_huff = Huffman::from_lengths(&cl_lengths)?;

    // Decode the literal/length + distance code lengths (run-length encoded).
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
                // copy previous length 3..6 times
                if i == 0 {
                    return Err(PngError::InflateError);
                }
                let prev = lengths[i - 1];
                let repeat = br.bits(2)? as usize + 3;
                for _ in 0..repeat {
                    if i >= total {
                        return Err(PngError::InflateError);
                    }
                    lengths[i] = prev;
                    i += 1;
                }
            }
            17 => {
                let repeat = br.bits(3)? as usize + 3;
                for _ in 0..repeat {
                    if i >= total {
                        return Err(PngError::InflateError);
                    }
                    lengths[i] = 0;
                    i += 1;
                }
            }
            18 => {
                let repeat = br.bits(7)? as usize + 11;
                for _ in 0..repeat {
                    if i >= total {
                        return Err(PngError::InflateError);
                    }
                    lengths[i] = 0;
                    i += 1;
                }
            }
            _ => return Err(PngError::InflateError),
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
) -> Result<(), PngError> {
    loop {
        let sym = litlen.decode(br)?;
        if sym == 256 {
            // end of block
            return Ok(());
        } else if sym < 256 {
            out.push(sym as u8);
        } else {
            let li = (sym - 257) as usize;
            if li >= LENGTH_BASE.len() {
                return Err(PngError::InflateError);
            }
            let length = LENGTH_BASE[li] as usize + br.bits(LENGTH_EXTRA[li] as u32)? as usize;
            let dsym = dist.decode(br)? as usize;
            if dsym >= DIST_BASE.len() {
                return Err(PngError::InflateError);
            }
            let distance = DIST_BASE[dsym] as usize + br.bits(DIST_EXTRA[dsym] as u32)? as usize;
            if distance == 0 || distance > out.len() {
                return Err(PngError::InflateError);
            }
            let start = out.len() - distance;
            // Copy byte-by-byte; overlapping copies are legal in DEFLATE.
            for k in 0..length {
                let b = out[start + k];
                out.push(b);
            }
        }
        if out.len() as u64 > MAX_PIXELS * 5 {
            return Err(PngError::InflateError);
        }
    }
}

// ════════════════════════════════════════════════════════════════════════════
// Host KAT suite — `cargo test -p raemedia`. FAIL-able by construction.
// ════════════════════════════════════════════════════════════════════════════
#[cfg(test)]
mod tests {
    use super::*;
    extern crate std;
    use std::vec::Vec as StdVec;

    // ── Tiny zlib/PNG encoder for fixtures (stored DEFLATE blocks) ──────────
    // This lets us hand-build PNGs with known pixels. It uses ONLY stored
    // (uncompressed) blocks, so the dynamic-Huffman test uses a separate,
    // real compressed fixture (see `dynamic_huffman_fixture`).

    fn adler32(data: &[u8]) -> u32 {
        let mut a: u32 = 1;
        let mut b: u32 = 0;
        for &byte in data {
            a = (a + byte as u32) % 65521;
            b = (b + a) % 65521;
        }
        (b << 16) | a
    }

    fn zlib_store(raw: &[u8]) -> StdVec<u8> {
        let mut out = StdVec::new();
        out.push(0x78); // CMF: CM=8, CINFO=7
        out.push(0x01); // FLG: makes (0x78<<8|0x01)=0x7801, 0x7801 % 31 == 0
                        // Single stored final block.
        out.push(0x01); // BFINAL=1, BTYPE=00, then byte-aligned
        let len = raw.len() as u16;
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&(!len).to_le_bytes());
        out.extend_from_slice(raw);
        out.extend_from_slice(&adler32(raw).to_be_bytes());
        out
    }

    const CRC_INIT: u32 = 0xFFFF_FFFF;
    fn crc32(bytes: &[u8]) -> u32 {
        let mut crc = CRC_INIT;
        for &b in bytes {
            crc ^= b as u32;
            for _ in 0..8 {
                if crc & 1 != 0 {
                    crc = (crc >> 1) ^ 0xEDB8_8320;
                } else {
                    crc >>= 1;
                }
            }
        }
        crc ^ 0xFFFF_FFFF
    }

    fn chunk(out: &mut StdVec<u8>, ctype: &[u8; 4], payload: &[u8]) {
        out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        out.extend_from_slice(ctype);
        out.extend_from_slice(payload);
        let mut crc_input = StdVec::new();
        crc_input.extend_from_slice(ctype);
        crc_input.extend_from_slice(payload);
        out.extend_from_slice(&crc32(&crc_input).to_be_bytes());
    }

    /// Build a PNG with a custom (already filter-prefixed) raw scanline buffer.
    fn build_png_raw(
        width: u32,
        height: u32,
        color_type: u8,
        palette: Option<&[(u8, u8, u8)]>,
        trns: Option<&[u8]>,
        raw_scanlines: &[u8],
    ) -> StdVec<u8> {
        let mut out = StdVec::new();
        out.extend_from_slice(&PNG_SIGNATURE);
        let mut ihdr = StdVec::new();
        ihdr.extend_from_slice(&width.to_be_bytes());
        ihdr.extend_from_slice(&height.to_be_bytes());
        ihdr.push(8); // bit depth
        ihdr.push(color_type);
        ihdr.push(0); // compression
        ihdr.push(0); // filter
        ihdr.push(0); // interlace
        chunk(&mut out, b"IHDR", &ihdr);
        if let Some(pal) = palette {
            let mut p = StdVec::new();
            for &(r, g, b) in pal {
                p.push(r);
                p.push(g);
                p.push(b);
            }
            chunk(&mut out, b"PLTE", &p);
        }
        if let Some(t) = trns {
            chunk(&mut out, b"tRNS", t);
        }
        let idat = zlib_store(raw_scanlines);
        chunk(&mut out, b"IDAT", &idat);
        chunk(&mut out, b"IEND", &[]);
        out
    }

    // Helper: prefix each scanline with a filter byte 0 (None).
    fn raw_none(pixels: &[u8], stride: usize) -> StdVec<u8> {
        let mut out = StdVec::new();
        for line in pixels.chunks(stride) {
            out.push(0);
            out.extend_from_slice(line);
        }
        out
    }

    // ── Color-type round-trip tests ─────────────────────────────────────────

    #[test]
    fn decode_rgb_truecolor() {
        // 2x1 image: pixel0 = red (255,0,0), pixel1 = green (0,255,0)
        let pixels = [255u8, 0, 0, 0, 255, 0];
        let raw = raw_none(&pixels, 6);
        let png = build_png_raw(2, 1, 2, None, None, &raw);
        let img = decode_png(&png).expect("rgb decode");
        assert_eq!(img.width, 2);
        assert_eq!(img.height, 1);
        assert_eq!(img.pixel(0, 0), Some((0xFF, 255, 0, 0)));
        assert_eq!(img.pixel(1, 0), Some((0xFF, 0, 255, 0)));
        // FAIL-ability: a wrong decode that swapped R/G would trip this.
        assert_ne!(img.pixel(0, 0), Some((0xFF, 0, 255, 0)));
    }

    #[test]
    fn decode_rgba_truecolor_alpha() {
        // 1x1: semi-transparent blue (0,0,255, a=128)
        let pixels = [0u8, 0, 255, 128];
        let raw = raw_none(&pixels, 4);
        let png = build_png_raw(1, 1, 6, None, None, &raw);
        let img = decode_png(&png).expect("rgba decode");
        assert_eq!(img.pixel(0, 0), Some((128, 0, 0, 255)));
        assert_ne!(img.pixel(0, 0), Some((0xFF, 0, 0, 255)));
    }

    #[test]
    fn decode_grayscale() {
        // 3x1: black, mid, white
        let pixels = [0u8, 128, 255];
        let raw = raw_none(&pixels, 3);
        let png = build_png_raw(3, 1, 0, None, None, &raw);
        let img = decode_png(&png).expect("gray decode");
        assert_eq!(img.pixel(0, 0), Some((0xFF, 0, 0, 0)));
        assert_eq!(img.pixel(1, 0), Some((0xFF, 128, 128, 128)));
        assert_eq!(img.pixel(2, 0), Some((0xFF, 255, 255, 255)));
    }

    #[test]
    fn decode_grayscale_alpha() {
        // 1x1: gray 64, alpha 200
        let pixels = [64u8, 200];
        let raw = raw_none(&pixels, 2);
        let png = build_png_raw(1, 1, 4, None, None, &raw);
        let img = decode_png(&png).expect("gray+alpha decode");
        assert_eq!(img.pixel(0, 0), Some((200, 64, 64, 64)));
    }

    #[test]
    fn decode_palette_indexed() {
        // palette: [red, green, blue]; indices 2,0,1
        let palette = [(255u8, 0, 0), (0, 255, 0), (0, 0, 255)];
        let pixels = [2u8, 0, 1];
        let raw = raw_none(&pixels, 3);
        let png = build_png_raw(3, 1, 3, Some(&palette), None, &raw);
        let img = decode_png(&png).expect("palette decode");
        assert_eq!(img.pixel(0, 0), Some((0xFF, 0, 0, 255))); // index 2 = blue
        assert_eq!(img.pixel(1, 0), Some((0xFF, 255, 0, 0))); // index 0 = red
        assert_eq!(img.pixel(2, 0), Some((0xFF, 0, 255, 0))); // index 1 = green
    }

    #[test]
    fn decode_palette_with_trns_alpha() {
        let palette = [(10u8, 20, 30), (40, 50, 60)];
        let trns = [0u8, 255]; // index 0 fully transparent
        let pixels = [0u8, 1];
        let raw = raw_none(&pixels, 2);
        let png = build_png_raw(2, 1, 3, Some(&palette), Some(&trns), &raw);
        let img = decode_png(&png).expect("palette+trns decode");
        assert_eq!(img.pixel(0, 0), Some((0, 10, 20, 30)));
        assert_eq!(img.pixel(1, 0), Some((255, 40, 50, 60)));
    }

    // ── The 5 filter types each round-trip ─────────────────────────────────

    /// Apply a PNG filter forward (encode), so we can verify our reverse.
    /// Uses 3-byte (RGB) pixels, 2 rows so Up/Average/Paeth have a real prev line.
    fn build_filtered_png(filter: u8) -> (StdVec<u8>, [[u8; 6]; 2]) {
        // Two rows, each 2 RGB pixels. Values chosen so the Paeth predictor picks
        // `up` and `upper-left` (not always `left`) for some columns — a broken
        // predictor that returns `a`/`left` would corrupt the recovered pixels and
        // flip `filter_paeth_roundtrip`, not just the dedicated unit assert.
        let rows: [[u8; 6]; 2] = [[200, 10, 90, 5, 250, 40], [12, 220, 6, 240, 30, 200]];
        let bpp = 3usize;
        let stride = 6usize;
        let mut raw = StdVec::new();
        let mut prev = [0u8; 6];
        for row in rows.iter() {
            raw.push(filter);
            let mut filtered = [0u8; 6];
            for x in 0..stride {
                let left = if x >= bpp { row[x - bpp] as i32 } else { 0 };
                let up = prev[x] as i32;
                let up_left = if x >= bpp { prev[x - bpp] as i32 } else { 0 };
                let cur = row[x] as i32;
                let f = match filter {
                    0 => cur,
                    1 => cur - left,
                    2 => cur - up,
                    3 => cur - (left + up) / 2,
                    4 => cur - paeth_predictor(left, up, up_left),
                    _ => cur,
                };
                filtered[x] = (f & 0xFF) as u8;
            }
            raw.extend_from_slice(&filtered);
            prev = *row;
        }
        let png = build_png_raw(2, 2, 2, None, None, &raw);
        (png, rows)
    }

    fn check_filter(filter: u8) {
        let (png, rows) = build_filtered_png(filter);
        let img = decode_png(&png).unwrap_or_else(|e| panic!("filter {filter} decode: {e:?}"));
        for (y, row) in rows.iter().enumerate() {
            for px in 0..2 {
                let (_, r, g, b) = img.pixel(px as u32, y as u32).expect("in-bounds");
                assert_eq!(r, row[px * 3], "filter {filter} R at ({px},{y})");
                assert_eq!(g, row[px * 3 + 1], "filter {filter} G at ({px},{y})");
                assert_eq!(b, row[px * 3 + 2], "filter {filter} B at ({px},{y})");
            }
        }
    }

    #[test]
    fn filter_none_roundtrip() {
        check_filter(0);
    }
    #[test]
    fn filter_sub_roundtrip() {
        check_filter(1);
    }
    #[test]
    fn filter_up_roundtrip() {
        check_filter(2);
    }
    #[test]
    fn filter_average_roundtrip() {
        check_filter(3);
    }
    #[test]
    fn filter_paeth_roundtrip() {
        // Exercises the full Paeth encode→decode container path end-to-end.
        // (The decode-side predictor correctness is pinned independently by
        // `paeth_predictor_is_load_bearing` below — that is the FAIL-able guard,
        // since a round-trip alone is self-consistent if both sides agree.)
        check_filter(4);
    }

    /// Named FAIL-ability proof for the Paeth filter. A deliberately broken
    /// predictor (e.g. one that always returns `a`/left) makes this assert flip:
    /// for (a=10, b=20, c=5) the correct selection is `b`=20, not 10. Break the
    /// predictor in `paeth_predictor` and `cargo test -p raemedia` reports this
    /// test FAILED (verified during development).
    #[test]
    fn paeth_predictor_is_load_bearing() {
        // Correct predictor for (a=10, b=20, c=5): p=25, pa=15, pb=5, pc=20 → b=20.
        assert_eq!(paeth_predictor(10, 20, 5), 20);
        // A broken "always return left" predictor would give 10 here.
        assert_ne!(paeth_predictor(10, 20, 5), 10);
        // A second discriminating case: (a=180, b=12, c=200): p=-8, pa=188,
        // pb=20, pc=208 → b=12 (not a, not c).
        assert_eq!(paeth_predictor(180, 12, 200), 12);
    }

    // ── Dynamic-Huffman inflate (most real PNGs use BTYPE=2) ────────────────

    /// A real **dynamic-Huffman** (BTYPE=2) DEFLATE stream — the encoding nearly
    /// every real PNG uses. This is the raw DEFLATE body (RFC 1951, no zlib/gzip
    /// wrapper) produced offline by `gzip -9` for the 97-byte ASCII string below,
    /// with the 10-byte gzip header and 8-byte trailer stripped. First byte 0x0d =
    /// BFINAL=1, BTYPE=10b → dynamic Huffman. Inflating it must reproduce the text.
    const DYNAMIC_HUFFMAN_TEXT: &[u8] = b"The quick brown fox jumps over the lazy dog. \
          Pack my box with five dozen liquor jugs. 0123456789!";

    fn dynamic_huffman_deflate_body() -> StdVec<u8> {
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
    fn inflate_dynamic_huffman() {
        // Confirm the fixture really is a dynamic block: BFINAL=1 (bit0), BTYPE=2.
        let body = dynamic_huffman_deflate_body();
        assert_eq!(
            body[0] & 0x07,
            0b101,
            "fixture must be a final dynamic block"
        );
        let out = inflate(&body).expect("dynamic huffman inflate");
        assert_eq!(out.as_slice(), DYNAMIC_HUFFMAN_TEXT);
        // FAIL-ability: a Huffman-table or length/distance bug would corrupt this.
        assert_ne!(out.first(), Some(&b'X'));
    }

    #[test]
    fn inflate_fixed_huffman() {
        // Real fixed-Huffman (BTYPE=1) DEFLATE body for "ABABAB", produced by
        // `gzip -1` with the 10-byte header / 8-byte trailer stripped. First byte
        // 0x73 → BFINAL=1, BTYPE=01b (fixed). Decoding must reproduce the text;
        // the back-reference ("AB" then a length/distance copy) exercises the
        // fixed length/distance tables.
        let body = std::vec![0x73u8, 0x74, 0x72, 0x04, 0x42, 0x00];
        assert_eq!(body[0] & 0x07, 0b011, "fixture must be a final fixed block");
        let out = inflate(&body).expect("fixed huffman inflate");
        assert_eq!(out.as_slice(), b"ABABAB");
        // The fixed tables must also construct cleanly.
        let _ = fixed_litlen().expect("fixed litlen table");
        let _ = fixed_dist().expect("fixed dist table");
    }

    // ── Malformed / hostile inputs: Err, never panic ────────────────────────

    #[test]
    fn reject_not_a_png() {
        let data = std::vec![0u8; 64];
        assert_eq!(decode_png(&data), Err(PngError::BadSignature));
    }

    #[test]
    fn reject_truncated_signature() {
        let data = std::vec![0x89u8, b'P', b'N'];
        assert_eq!(decode_png(&data), Err(PngError::BadSignature));
    }

    #[test]
    fn reject_truncated_idat() {
        // Valid signature + IHDR, but a chunk length that runs past the buffer.
        let mut png = StdVec::new();
        png.extend_from_slice(&PNG_SIGNATURE);
        let mut ihdr = StdVec::new();
        ihdr.extend_from_slice(&2u32.to_be_bytes());
        ihdr.extend_from_slice(&2u32.to_be_bytes());
        ihdr.push(8);
        ihdr.push(2);
        ihdr.push(0);
        ihdr.push(0);
        ihdr.push(0);
        chunk(&mut png, b"IHDR", &ihdr);
        // An IDAT header claiming 0xFFFF bytes but with no payload.
        png.extend_from_slice(&0xFFFFu32.to_be_bytes());
        png.extend_from_slice(b"IDAT");
        // (no payload follows)
        let res = decode_png(&png);
        assert!(matches!(res, Err(PngError::BadChunkLength)), "got {res:?}");
    }

    #[test]
    fn reject_oversized_dimensions() {
        let mut png = StdVec::new();
        png.extend_from_slice(&PNG_SIGNATURE);
        let mut ihdr = StdVec::new();
        ihdr.extend_from_slice(&0x7FFF_FFFFu32.to_be_bytes());
        ihdr.extend_from_slice(&0x7FFF_FFFFu32.to_be_bytes());
        ihdr.push(8);
        ihdr.push(2);
        ihdr.push(0);
        ihdr.push(0);
        ihdr.push(0);
        chunk(&mut png, b"IHDR", &ihdr);
        chunk(&mut png, b"IEND", &[]);
        assert_eq!(decode_png(&png), Err(PngError::DimensionsOutOfRange));
    }

    #[test]
    fn reject_zero_dimensions() {
        let mut png = StdVec::new();
        png.extend_from_slice(&PNG_SIGNATURE);
        let mut ihdr = StdVec::new();
        ihdr.extend_from_slice(&0u32.to_be_bytes());
        ihdr.extend_from_slice(&1u32.to_be_bytes());
        ihdr.push(8);
        ihdr.push(2);
        ihdr.push(0);
        ihdr.push(0);
        ihdr.push(0);
        chunk(&mut png, b"IHDR", &ihdr);
        chunk(&mut png, b"IEND", &[]);
        assert_eq!(decode_png(&png), Err(PngError::DimensionsOutOfRange));
    }

    #[test]
    fn reject_corrupt_deflate() {
        // Valid signature/IHDR but IDAT contains a garbage zlib body.
        let raw_garbage = std::vec![0x78u8, 0x9c, 0xFF, 0xFF, 0xFF, 0xFF];
        let mut png = StdVec::new();
        png.extend_from_slice(&PNG_SIGNATURE);
        let mut ihdr = StdVec::new();
        ihdr.extend_from_slice(&2u32.to_be_bytes());
        ihdr.extend_from_slice(&2u32.to_be_bytes());
        ihdr.push(8);
        ihdr.push(2);
        ihdr.push(0);
        ihdr.push(0);
        ihdr.push(0);
        chunk(&mut png, b"IHDR", &ihdr);
        chunk(&mut png, b"IDAT", &raw_garbage);
        chunk(&mut png, b"IEND", &[]);
        // Must return an Err (InflateError or SizeMismatch), never panic.
        assert!(decode_png(&png).is_err());
    }

    #[test]
    fn reject_bad_zlib_header() {
        let bad = std::vec![0x00u8, 0x00]; // CM != 8
        assert_eq!(zlib_inflate(&bad), Err(PngError::BadZlibHeader));
    }

    #[test]
    fn reject_palette_missing() {
        // color type 3 with no PLTE chunk.
        let pixels = [0u8];
        let raw = raw_none(&pixels, 1);
        let png = build_png_raw(1, 1, 3, None, None, &raw);
        assert_eq!(decode_png(&png), Err(PngError::MissingPalette));
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Fuzz / property hardening — hostile-input boundary (Concept: "show my
    // photos" decodes downloaded/embedded PNG, a classic memory-safety surface).
    //
    // Contract under test: `decode_png` is total over arbitrary bytes — it
    // returns `Ok`/`Err` but NEVER panics, NEVER OOMs on a forged IHDR, and
    // NEVER reads/writes out of bounds. These tests are FAIL-able: a real panic /
    // OOB / unbounded alloc surfaces as a harness abort or OOM kill, reported by
    // `cargo test` as a failure. (E.g. removing the IHDR `MAX_PIXELS` guard makes
    // `fuzz_huge_dimension_header_is_bounded` attempt a multi-GiB allocation and
    // either OOM-abort or fail its `is_err()` assertion; an unchecked chunk
    // length would index past the buffer and panic in `fuzz_*`.)
    // ─────────────────────────────────────────────────────────────────────────

    /// Self-contained deterministic PRNG (xorshift64*). No external fuzz crate,
    /// no `Cargo.toml` change — matches the rae_gif / rae_bmp fuzz pattern.
    struct XorShift(u64);
    impl XorShift {
        fn new(seed: u64) -> Self {
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
        let mut rng = XorShift::new(0xDEAD_BEEF_1234_5678);
        for _ in 0..20_000 {
            let len = rng.range(1025);
            let mut buf: StdVec<u8> = StdVec::with_capacity(len);
            for _ in 0..len {
                buf.push(rng.next_u8());
            }
            let _ = decode_png(&buf);
        }
    }

    /// Random bytes that always start with the valid PNG signature drive the
    /// chunk parser much deeper (random alone bails at the signature check).
    #[test]
    fn fuzz_valid_signature_random_tail_never_panic() {
        let mut rng = XorShift::new(0xFACE_F00D_0BAD_BEEF);
        for _ in 0..20_000 {
            let len = rng.range(512);
            let mut buf: StdVec<u8> = StdVec::with_capacity(len + 8);
            buf.extend_from_slice(&PNG_SIGNATURE);
            for _ in 0..len {
                buf.push(rng.next_u8());
            }
            let _ = decode_png(&buf);
        }
    }

    /// Mutation fuzz over a real, valid PNG: flip bytes (corrupting IHDR fields,
    /// chunk lengths, CRCs, the zlib/IDAT stream) and truncate. Never panics.
    #[test]
    fn fuzz_mutated_valid_png_never_panic() {
        // Known-good 4x3 truecolor fixture.
        let pixels: StdVec<u8> = (0..(4 * 3 * 3)).map(|i| (i * 7) as u8).collect();
        let raw = raw_none(&pixels, 4 * 3);
        let base = build_png_raw(4, 3, 2, None, None, &raw);
        assert!(decode_png(&base).is_ok(), "fixture must decode clean");

        let mut rng = XorShift::new(0xC0DE_D00D_5EED_1111);
        for _ in 0..40_000 {
            let mut buf = base.clone();
            let nmut = 1 + rng.range(8);
            for _ in 0..nmut {
                if buf.is_empty() {
                    break;
                }
                let idx = rng.range(buf.len());
                buf[idx] = rng.next_u8();
            }
            if rng.next_u64() & 1 == 0 {
                let cut = rng.range(buf.len() + 1);
                buf.truncate(cut);
            }
            let _ = decode_png(&buf);
        }
    }

    /// Every truncation prefix of a valid PNG (truncated signature, IHDR, IDAT,
    /// IEND, mid-zlib-stream) must decode-or-Err, never panic.
    #[test]
    fn fuzz_all_truncations_never_panic() {
        let pixels: StdVec<u8> = (0..(4 * 3 * 3)).map(|i| (i * 7) as u8).collect();
        let raw = raw_none(&pixels, 4 * 3);
        let base = build_png_raw(4, 3, 2, None, None, &raw);
        for cut in 0..=base.len() {
            let _ = decode_png(&base[..cut]);
        }
    }

    /// A forged chunk length must be REJECTED, not trusted into an OOB read.
    /// Builds a valid signature + IHDR, then an IDAT whose declared length is
    /// enormous (0x7FFF_FFFF) but whose payload is tiny. The decoder must Err
    /// (BadChunkLength / Truncated), never index past the buffer. FAIL-able: an
    /// unchecked length would slice `data[off..off+len]` and panic.
    #[test]
    fn fuzz_chunk_length_overflow_is_bounded() {
        let mut buf = StdVec::new();
        buf.extend_from_slice(&PNG_SIGNATURE);
        // Valid IHDR (1x1 truecolor).
        let mut ihdr = StdVec::new();
        ihdr.extend_from_slice(&1u32.to_be_bytes());
        ihdr.extend_from_slice(&1u32.to_be_bytes());
        ihdr.push(8); // bit depth
        ihdr.push(2); // color type RGB
        ihdr.push(0);
        ihdr.push(0);
        ihdr.push(0);
        chunk(&mut buf, b"IHDR", &ihdr);
        // Forged IDAT: claim a huge length, provide ~no payload.
        buf.extend_from_slice(&0x7FFF_FFFFu32.to_be_bytes());
        buf.extend_from_slice(b"IDAT");
        buf.extend_from_slice(&[0x01, 0x02, 0x03]); // far less than claimed
        let res = decode_png(&buf);
        assert!(
            res.is_err(),
            "forged huge chunk length must be rejected, got {res:?}"
        );
    }

    /// A chunk length of exactly u32::MAX (length-field overflow when added to the
    /// read offset) must not wrap/overflow into an in-bounds slice. FAIL-able:
    /// `off + len` without checked arithmetic would wrap and slice wrongly.
    #[test]
    fn fuzz_chunk_length_u32_max_is_bounded() {
        let mut buf = StdVec::new();
        buf.extend_from_slice(&PNG_SIGNATURE);
        buf.extend_from_slice(&u32::MAX.to_be_bytes());
        buf.extend_from_slice(b"IDAT");
        buf.extend_from_slice(&[0xFF; 4]);
        let res = decode_png(&buf);
        assert!(res.is_err(), "u32::MAX chunk length must Err, got {res:?}");
    }

    /// An IHDR claiming huge dimensions must be rejected by the dimension/pixel
    /// caps, not honored into a giant allocation. FAIL-able: removing the
    /// MAX_DIMENSION / MAX_PIXELS guard makes this OOM or fail `is_err()`.
    #[test]
    fn fuzz_huge_dimension_header_is_bounded() {
        let mut buf = StdVec::new();
        buf.extend_from_slice(&PNG_SIGNATURE);
        let mut ihdr = StdVec::new();
        ihdr.extend_from_slice(&0x7FFF_FFFFu32.to_be_bytes()); // width ~2.1e9
        ihdr.extend_from_slice(&0x7FFF_FFFFu32.to_be_bytes()); // height ~2.1e9
        ihdr.push(8);
        ihdr.push(6); // RGBA → would be ~4 channels * 4.6e18 px
        ihdr.push(0);
        ihdr.push(0);
        ihdr.push(0);
        chunk(&mut buf, b"IHDR", &ihdr);
        let res = decode_png(&buf);
        assert!(
            matches!(res, Err(PngError::DimensionsOutOfRange)),
            "huge-dimension IHDR must be rejected by the cap, got {res:?}"
        );
    }

    /// width*height*channels overflow guard: dimensions under MAX_DIMENSION per
    /// axis but whose product over MAX_PIXELS must still be rejected.
    #[test]
    fn fuzz_pixel_count_overflow_is_bounded() {
        let mut buf = StdVec::new();
        buf.extend_from_slice(&PNG_SIGNATURE);
        let mut ihdr = StdVec::new();
        ihdr.extend_from_slice(&60000u32.to_be_bytes()); // < MAX_DIMENSION
        ihdr.extend_from_slice(&60000u32.to_be_bytes()); // < MAX_DIMENSION; product > MAX_PIXELS
        ihdr.push(8);
        ihdr.push(6); // RGBA
        ihdr.push(0);
        ihdr.push(0);
        ihdr.push(0);
        chunk(&mut buf, b"IHDR", &ihdr);
        let res = decode_png(&buf);
        assert!(
            matches!(res, Err(PngError::DimensionsOutOfRange)),
            "pixel-count overflow must be rejected, got {res:?}"
        );
    }

    /// Fuzz IHDR bit-depth / color-type combinations (most are invalid per the
    /// PNG spec). Unsupported combos must Err, never panic.
    #[test]
    fn fuzz_bad_bitdepth_colortype_never_panic() {
        let mut rng = XorShift::new(0x1122_3344_5566_7788);
        for _ in 0..10_000 {
            let mut buf = StdVec::new();
            buf.extend_from_slice(&PNG_SIGNATURE);
            let mut ihdr = StdVec::new();
            ihdr.extend_from_slice(&2u32.to_be_bytes());
            ihdr.extend_from_slice(&2u32.to_be_bytes());
            ihdr.push(rng.next_u8()); // arbitrary bit depth (incl. illegal)
            ihdr.push(rng.next_u8()); // arbitrary color type (incl. illegal)
            ihdr.push(0);
            ihdr.push(0);
            ihdr.push(rng.next_u8() & 0x01); // interlace 0/1 (or garbage low bit)
            chunk(&mut buf, b"IHDR", &ihdr);
            // A minimal (garbage) IDAT so parsing proceeds past IHDR sometimes.
            chunk(&mut buf, b"IDAT", &[0x78, 0x01, 0x00]);
            chunk(&mut buf, b"IEND", &[]);
            let _ = decode_png(&buf);
        }
    }

    /// Fuzz a palette (color type 3) image with a malformed / random PLTE and
    /// random index bytes — bad palette indices must Err, never OOB the table.
    #[test]
    fn fuzz_malformed_palette_never_panic() {
        let mut rng = XorShift::new(0x99AA_BBCC_DDEE_FF00);
        for _ in 0..10_000 {
            let w = 1 + rng.range(4);
            let h = 1 + rng.range(4);
            // Random small palette (0..4 entries → indices easily out of range).
            let npal = rng.range(5);
            let mut pal: StdVec<(u8, u8, u8)> = StdVec::new();
            for _ in 0..npal {
                pal.push((rng.next_u8(), rng.next_u8(), rng.next_u8()));
            }
            // Random index bytes (often >= palette size).
            let mut pixels: StdVec<u8> = StdVec::new();
            for _ in 0..(w * h) {
                pixels.push(rng.next_u8());
            }
            let raw = raw_none(&pixels, w);
            let pal_ref: Option<&[(u8, u8, u8)]> = if npal == 0 { None } else { Some(&pal) };
            let png = build_png_raw(w as u32, h as u32, 3, pal_ref, None, &raw);
            let _ = decode_png(&png);
        }
    }

    /// Fuzz the zlib/IDAT payload: valid container, garbage compressed data.
    /// Corrupt/truncated DEFLATE must yield Err, never panic.
    #[test]
    fn fuzz_corrupt_zlib_stream_never_panic() {
        let mut rng = XorShift::new(0x0BAD_F00D_FEED_FACE);
        for _ in 0..10_000 {
            let mut buf = StdVec::new();
            buf.extend_from_slice(&PNG_SIGNATURE);
            let mut ihdr = StdVec::new();
            ihdr.extend_from_slice(&4u32.to_be_bytes());
            ihdr.extend_from_slice(&4u32.to_be_bytes());
            ihdr.push(8);
            ihdr.push(2); // RGB
            ihdr.push(0);
            ihdr.push(0);
            ihdr.push(0);
            chunk(&mut buf, b"IHDR", &ihdr);
            // Plausible zlib header then random bytes.
            let mut idat = StdVec::new();
            idat.push(0x78);
            idat.push(0x01);
            let n = rng.range(64);
            for _ in 0..n {
                idat.push(rng.next_u8());
            }
            chunk(&mut buf, b"IDAT", &idat);
            chunk(&mut buf, b"IEND", &[]);
            let _ = decode_png(&buf);
        }
    }
}
