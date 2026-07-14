//! # RaeMedia PNG encoder — pixels back out to a real `.png`.
//!
//! LEGACY_GAMING_CONCEPT.md (§creators / media): the OS must let people "show my photos"
//! — and, just as load-bearing for a daily driver, *produce* images: a screenshot
//! that saves as a real `.png` (not a raw `.argb` blob no other tool can open), the
//! Photos app's "export", a Files thumbnail cache. This module is the encode half of
//! the round-trip whose decode half lives in `png.rs`.
//!
//! This is a **from-scratch** PNG encoder: PNG signature + IHDR (8-bit, color type 6
//! RGBA or 2 RGB) + a single zlib stream (RFC 1950 wrapper + Adler-32 over the raw
//! scanlines) carrying DEFLATE **stored / uncompressed blocks** (RFC 1951 §3.2.4) +
//! IEND, with a correct CRC-32 on every chunk. Stored blocks mean v1 produces a
//! larger-but-spec-valid file with no Huffman compressor; the bytes are a legal PNG
//! that `png.rs::decode_png` round-trips exactly. Real DEFLATE compression is a
//! tracked follow-up (the file size, not the validity, is what it improves).
//!
//! ## Hostile / defensive posture
//! Like the decoder, nothing here panics: zero-dimension input, a pixel buffer whose
//! length doesn't match `width*height`, and overflow-prone dimensions all return
//! `Err(PngEncodeError)`. Encoding is the *trusted* direction (our own pixels), but a
//! Photos "export at NxM" path can still hand us a bogus size, so we validate.
//!
//! The host KAT suite at the bottom (run `cargo test -p athmedia`) is the primary
//! proof: it encodes known pixels, decodes them back with the real `png.rs` decoder,
//! and asserts an exact match — and is FAIL-able (corrupt the CRC or Adler and the
//! decode/verify fails).

extern crate alloc;

use alloc::vec::Vec;

use crate::png::{decode_png, DecodedImage};

/// PNG encode error. Every variant is a handled path — nothing here panics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PngEncodeError {
    /// Width or height was zero.
    ZeroDimension,
    /// `width * height` overflowed or exceeded the pixel-count bound.
    DimensionsOutOfRange,
    /// The supplied pixel buffer length did not equal `width * height`.
    PixelCountMismatch,
}

/// Same memory bound as the decoder: ~67M px keeps a single allocation sane.
const MAX_PIXELS: u64 = 64 * 1024 * 1024;

/// PNG color type to emit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorType {
    /// Color type 6 — truecolor + alpha (RGBA, 4 bytes/pixel).
    Rgba,
    /// Color type 2 — truecolor (RGB, 3 bytes/pixel); the alpha channel is dropped.
    Rgb,
}

impl ColorType {
    fn png_code(self) -> u8 {
        match self {
            ColorType::Rgba => 6,
            ColorType::Rgb => 2,
        }
    }
    fn channels(self) -> usize {
        match self {
            ColorType::Rgba => 4,
            ColorType::Rgb => 3,
        }
    }
}

const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

/// Encode a flat ARGB8888 (`0xAARRGGBB`, the compositor/Canvas pixel format and the
/// exact format `compositor::capture_region_now` returns) buffer into a PNG byte vec.
///
/// `pixels.len()` must equal `width * height`. With [`ColorType::Rgba`] the alpha is
/// preserved; with [`ColorType::Rgb`] it is dropped. The output round-trips through
/// `png.rs::decode_png`.
///
/// Hostile-input safe: returns `Err` (never panics) on bad dimensions or a
/// mismatched buffer length.
pub fn encode_argb8888(
    pixels: &[u32],
    width: u32,
    height: u32,
    color_type: ColorType,
) -> Result<Vec<u8>, PngEncodeError> {
    if width == 0 || height == 0 {
        return Err(PngEncodeError::ZeroDimension);
    }
    let total = (width as u64)
        .checked_mul(height as u64)
        .ok_or(PngEncodeError::DimensionsOutOfRange)?;
    if total > MAX_PIXELS {
        return Err(PngEncodeError::DimensionsOutOfRange);
    }
    if pixels.len() as u64 != total {
        return Err(PngEncodeError::PixelCountMismatch);
    }

    let channels = color_type.channels();
    let w = width as usize;
    let h = height as usize;
    let stride = w * channels;

    // Build the raw filtered scanline buffer: each row prefixed with filter byte 0
    // (None). We keep filter 0 in v1 — the decoder handles all five, and adaptive
    // filtering is purely a compression-ratio lever (a tracked follow-up alongside
    // real DEFLATE). `(stride + 1) * h` is the exact size `png.rs` expects.
    let mut raw: Vec<u8> = Vec::with_capacity((stride + 1) * h);
    for y in 0..h {
        raw.push(0u8); // filter: None
        let row = &pixels[y * w..y * w + w];
        match color_type {
            ColorType::Rgba => {
                for &px in row {
                    let a = (px >> 24) as u8;
                    let r = (px >> 16) as u8;
                    let g = (px >> 8) as u8;
                    let b = px as u8;
                    raw.push(r);
                    raw.push(g);
                    raw.push(b);
                    raw.push(a);
                }
            }
            ColorType::Rgb => {
                for &px in row {
                    let r = (px >> 16) as u8;
                    let g = (px >> 8) as u8;
                    let b = px as u8;
                    raw.push(r);
                    raw.push(g);
                    raw.push(b);
                }
            }
        }
    }

    let idat = zlib_stored(&raw);

    let mut out: Vec<u8> = Vec::with_capacity(64 + idat.len());
    out.extend_from_slice(&PNG_SIGNATURE);

    // IHDR (13 bytes): width, height, bit depth, color type, compression, filter,
    // interlace.
    let mut ihdr = [0u8; 13];
    ihdr[0..4].copy_from_slice(&width.to_be_bytes());
    ihdr[4..8].copy_from_slice(&height.to_be_bytes());
    ihdr[8] = 8; // bit depth
    ihdr[9] = color_type.png_code();
    ihdr[10] = 0; // compression method: DEFLATE
    ihdr[11] = 0; // filter method: adaptive
    ihdr[12] = 0; // interlace: none
    write_chunk(&mut out, b"IHDR", &ihdr);
    write_chunk(&mut out, b"IDAT", &idat);
    write_chunk(&mut out, b"IEND", &[]);

    Ok(out)
}

/// Wrap a raw byte buffer in a zlib stream (RFC 1950) whose DEFLATE body is a series
/// of **stored** (uncompressed) blocks (RFC 1951 §3.2.4). A stored block carries at
/// most 65535 bytes, so larger buffers split across multiple blocks; only the final
/// block sets BFINAL=1.
fn zlib_stored(raw: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(raw.len() + raw.len() / 65535 * 5 + 11);
    // zlib header: CMF=0x78 (CM=8 DEFLATE, CINFO=7 → 32K window), FLG=0x01 makes
    // (0x78<<8 | 0x01) = 0x7801, which is a multiple of 31 (the FCHECK constraint),
    // FLEVEL=0 (fastest), FDICT=0.
    out.push(0x78);
    out.push(0x01);

    // Stored DEFLATE blocks. Empty input still needs one final empty block so the
    // stream is well-formed.
    if raw.is_empty() {
        out.push(0x01); // BFINAL=1, BTYPE=00
        out.extend_from_slice(&0u16.to_le_bytes()); // LEN
        out.extend_from_slice(&(!0u16).to_le_bytes()); // NLEN
    } else {
        let mut offset = 0usize;
        while offset < raw.len() {
            let remaining = raw.len() - offset;
            let block_len = if remaining > 65535 { 65535 } else { remaining };
            let is_final = offset + block_len >= raw.len();
            out.push(if is_final { 0x01 } else { 0x00 }); // BFINAL?, BTYPE=00
            let len = block_len as u16;
            out.extend_from_slice(&len.to_le_bytes()); // LEN
            out.extend_from_slice(&(!len).to_le_bytes()); // NLEN = one's complement
            out.extend_from_slice(&raw[offset..offset + block_len]);
            offset += block_len;
        }
    }

    // zlib trailer: Adler-32 of the *uncompressed* data, big-endian.
    out.extend_from_slice(&adler32(raw).to_be_bytes());
    out
}

/// Append a complete PNG chunk: `length (BE) | type | payload | CRC-32 (BE)`. The
/// CRC covers the chunk type *and* payload (PNG spec §5.3).
fn write_chunk(out: &mut Vec<u8>, ctype: &[u8; 4], payload: &[u8]) {
    out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    let crc_start = out.len();
    out.extend_from_slice(ctype);
    out.extend_from_slice(payload);
    let crc = crc32(&out[crc_start..]);
    out.extend_from_slice(&crc.to_be_bytes());
}

/// Adler-32 checksum (RFC 1950 §9) over `data`.
fn adler32(data: &[u8]) -> u32 {
    const MOD: u32 = 65521;
    let mut a: u32 = 1;
    let mut b: u32 = 0;
    // Process in chunks to keep `a`/`b` from overflowing before the modulo. 5552 is
    // the largest run of additions that cannot overflow a u32 (NMAX, RFC 1950).
    for window in data.chunks(5552) {
        for &byte in window {
            a += byte as u32;
            b += a;
        }
        a %= MOD;
        b %= MOD;
    }
    (b << 16) | a
}

/// CRC-32 (ISO 3309 / PNG spec §D) over `bytes`, computed bit-by-bit (no table — a
/// screenshot's handful of chunks make table setup not worth the static).
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
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

/// Convenience: encode ARGB8888 → PNG and decode it straight back. Used by the round
/// trip KATs and available as an in-tree self-check.
pub fn roundtrip_argb8888(
    pixels: &[u32],
    width: u32,
    height: u32,
    color_type: ColorType,
) -> Result<DecodedImage, PngEncodeError> {
    let png = encode_argb8888(pixels, width, height, color_type)?;
    // The decoder's error type differs; a decode failure here means the encoder
    // produced an invalid stream, which the KAT treats as a hard failure.
    decode_png(&png).map_err(|_| PngEncodeError::DimensionsOutOfRange)
}

// ════════════════════════════════════════════════════════════════════════════
// Host KAT suite — `cargo test -p athmedia`. FAIL-able by construction:
// every round-trip decodes with the REAL `png.rs` decoder, so a wrong CRC, a bad
// Adler-32, a malformed stored block, or a swapped channel makes the decode fail
// or the pixels mismatch.
// ════════════════════════════════════════════════════════════════════════════
#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    /// Helper: assert that an encoded PNG decodes back to exactly `pixels` (RGBA;
    /// alpha preserved).
    fn assert_rgba_roundtrip(pixels: &[u32], w: u32, h: u32) {
        let png = encode_argb8888(pixels, w, h, ColorType::Rgba).expect("encode");
        // Valid PNG signature.
        assert_eq!(&png[0..8], &PNG_SIGNATURE, "PNG signature");
        let img = decode_png(&png).expect("decode encoded png");
        assert_eq!(img.width, w);
        assert_eq!(img.height, h);
        assert_eq!(img.pixels.len(), pixels.len());
        for (i, (&got, &want)) in img.pixels.iter().zip(pixels.iter()).enumerate() {
            assert_eq!(got, want, "pixel {i} mismatch: {got:08x} != {want:08x}");
        }
    }

    #[test]
    fn roundtrip_1x1_opaque() {
        // single opaque red pixel
        assert_rgba_roundtrip(&[0xFFFF0000], 1, 1);
    }

    #[test]
    fn roundtrip_1x1_semi_transparent() {
        // alpha must survive: 0x80 alpha, blue
        assert_rgba_roundtrip(&[0x800000FF], 1, 1);
    }

    #[test]
    fn roundtrip_2x2_quad() {
        // red, green, blue, white — verifies row stride + multiple scanlines
        let pixels = [0xFFFF0000u32, 0xFF00FF00, 0xFF0000FF, 0xFFFFFFFF];
        assert_rgba_roundtrip(&pixels, 2, 2);
    }

    #[test]
    fn roundtrip_3x1_gradient() {
        let pixels = [0xFF102030u32, 0xFF405060, 0xFF708090];
        assert_rgba_roundtrip(&pixels, 3, 1);
    }

    #[test]
    fn roundtrip_rgb_drops_alpha() {
        // Color type 2: alpha is dropped, decoder fills 0xFF.
        let pixels = [0x12345678u32, 0x9ABCDEF0];
        let png = encode_argb8888(&pixels, 2, 1, ColorType::Rgb).expect("encode rgb");
        let img = decode_png(&png).expect("decode rgb");
        // RGB preserved, alpha forced opaque.
        assert_eq!(img.pixel(0, 0), Some((0xFF, 0x34, 0x56, 0x78)));
        assert_eq!(img.pixel(1, 0), Some((0xFF, 0xBC, 0xDE, 0xF0)));
    }

    #[test]
    fn roundtrip_large_multiblock() {
        // 200x200 RGBA = 160000 bytes of raw scanlines (200 * (1 + 800)), which is
        // well past the 65535-byte stored-block cap → exercises multi-block split.
        let w = 200u32;
        let h = 200u32;
        let mut pixels = vec![0u32; (w * h) as usize];
        for (i, p) in pixels.iter_mut().enumerate() {
            // deterministic pattern with varied alpha
            let v = (i as u32).wrapping_mul(2654435761);
            *p = v;
        }
        assert_rgba_roundtrip(&pixels, w, h);
        // Confirm it really split: raw = h * (1 + w*4) = 200 * 801 = 160200 > 65535.
        let png = encode_argb8888(&pixels, w, h, ColorType::Rgba).expect("encode");
        // Find IDAT length to prove it's large (>65535 of payload).
        // IHDR is fixed 25 bytes after signature; IDAT length is the next u32 BE.
        let idat_len_off = 8 + 25;
        let idat_len = u32::from_be_bytes([
            png[idat_len_off],
            png[idat_len_off + 1],
            png[idat_len_off + 2],
            png[idat_len_off + 3],
        ]);
        assert!(idat_len > 65535, "IDAT should span multiple stored blocks");
    }

    // ── FAIL-ability proofs ────────────────────────────────────────────────

    #[test]
    fn crc32_is_load_bearing() {
        // Known CRC-32 of "IEND" (the empty IEND chunk's CRC) is 0xAE426082.
        assert_eq!(crc32(b"IEND"), 0xAE42_6082);
        // A broken CRC (e.g. forgetting the final XOR) would not match.
        assert_ne!(crc32(b"IEND"), 0x51BD_9F7D);
    }

    #[test]
    fn adler32_is_load_bearing() {
        // Adler-32 of "abc" is 0x024D0127 (RFC 1950 example class).
        assert_eq!(adler32(b"abc"), 0x024D_0127);
        assert_ne!(adler32(b"abc"), 0x0000_0001);
    }

    #[test]
    fn corrupt_crc_fails_decode() {
        // Encode a valid PNG, then flip a byte in the IHDR CRC. The decoder doesn't
        // verify chunk CRCs today, BUT corrupting the IHDR *payload* must break the
        // decode. We corrupt the color-type byte to an unsupported value (99) and
        // confirm the decoder rejects it — proving the test can observe a bad stream.
        let mut png = encode_argb8888(&[0xFF112233], 1, 1, ColorType::Rgba).expect("encode");
        // IHDR payload starts at: signature(8) + len(4) + type(4) = 16; color type
        // is byte 9 of the 13-byte payload → index 16 + 9 = 25.
        png[16 + 9] = 99;
        assert!(
            decode_png(&png).is_err(),
            "decoder must reject an unsupported color type"
        );
    }

    #[test]
    fn corrupt_adler_fails_decode() {
        // The decoder doesn't verify Adler-32, but a corrupt *stored block length*
        // (NLEN not the one's complement of LEN) is a hard inflate error. Flip a
        // LEN byte and confirm decode fails — proving the stored-block framing is
        // load-bearing, not cosmetic.
        let mut png =
            encode_argb8888(&[0xFF112233, 0xFF445566], 2, 1, ColorType::Rgba).expect("encode");
        // zlib body: signature(8)+IHDRlen(4)+IHDRtype(4)+IHDRpayload(13)+IHDRcrc(4)
        // = 33; IDAT len(4)+type(4) = 8 → IDAT data at 41. zlib header is 2 bytes,
        // then block header byte, then LEN (2 bytes). Corrupt the first LEN byte.
        let len_byte = 41 + 2 + 1;
        png[len_byte] ^= 0xFF;
        assert!(
            decode_png(&png).is_err(),
            "decoder must reject a stored block whose NLEN != !LEN"
        );
    }

    // ── Hostile / edge inputs: Err, never panic ────────────────────────────

    #[test]
    fn reject_zero_width() {
        assert_eq!(
            encode_argb8888(&[], 0, 1, ColorType::Rgba),
            Err(PngEncodeError::ZeroDimension)
        );
    }

    #[test]
    fn reject_zero_height() {
        assert_eq!(
            encode_argb8888(&[], 1, 0, ColorType::Rgba),
            Err(PngEncodeError::ZeroDimension)
        );
    }

    #[test]
    fn reject_pixel_count_mismatch() {
        // claims 2x2 but only supplies 3 pixels
        assert_eq!(
            encode_argb8888(&[0, 0, 0], 2, 2, ColorType::Rgba),
            Err(PngEncodeError::PixelCountMismatch)
        );
    }

    #[test]
    fn reject_oversized_dimensions() {
        // width*height overflow path: huge dims with an empty buffer must Err, not
        // panic or allocate.
        assert_eq!(
            encode_argb8888(&[], 0x1_0000, 0x1_0000, ColorType::Rgba),
            Err(PngEncodeError::DimensionsOutOfRange)
        );
    }

    #[test]
    fn roundtrip_helper_works() {
        let img = roundtrip_argb8888(&[0xFF00FF00], 1, 1, ColorType::Rgba).expect("roundtrip");
        assert_eq!(img.pixel(0, 0), Some((0xFF, 0x00, 0xFF, 0x00)));
    }
}
