//! # RaeImage — the unified "open any image" dispatcher (the cohesion seam).
//!
//! LEGACY_GAMING_CONCEPT.md criterion #5 (cohesion — every part of the OS agrees on
//! what a file *is* and consumes it through one model): AthenaOS already ships five
//! from-scratch still-image decoders ([`rae_png`], [`rae_jpeg`], [`rae_bmp`],
//! [`rae_gif`], [`rae_webp`]), each producing the **identical** ARGB8888
//! `Vec<u32>` pixel model. What was missing was the single front door. This crate
//! is that door: one [`decode`] call that sniffs the format from its **magic
//! bytes** (via [`rae_formats::detect`], NOT the file extension — criterion #6
//! security: never trust a name an attacker controls), dispatches to the matching
//! decoder, and re-homes its output into one unified [`Image`]. Photos, `raeplay`,
//! and `raegfx` call `rae_image::decode(bytes)` and never branch on format.
//!
//! ## What this crate is — and is not
//! It is a **thin dispatcher**. It owns **no decode logic**: each underlying
//! decoder already emits ARGB8888 (`0xAARRGGBB`), so dispatch is a content sniff,
//! a field move of the pixel `Vec`, and error mapping. The value is the single
//! entry point + the detection wiring + the one unified type — reimplementing a
//! decoder here would be exactly the duplication this seam exists to remove.
//!
//! ## What decodes ([`decode`])
//! | Format | Detected as | Decoder | Notes |
//! |---|---|---|---|
//! | PNG  | [`FileKind::Png`]  | [`rae_png::decode_png`]   | full color-type matrix |
//! | JPEG | [`FileKind::Jpeg`] | [`rae_jpeg::decode_jpeg`] | baseline DCT |
//! | BMP  | [`FileKind::Bmp`]  | [`rae_bmp::decode_bmp`]   | + ICO via rae_bmp directly |
//! | GIF  | [`FileKind::Gif`]  | [`rae_gif::decode_gif`]   | **first frame only** (see below) |
//! | WebP | [`FileKind::Webp`] | [`rae_webp::decode_webp`] | VP8L lossless |
//!
//! Any other (or undetected) kind → [`ImageError::UnsupportedFormat`]. A decoder's
//! own failure is mapped to [`ImageError::Decode`] carrying which format failed
//! plus a short reason string.
//!
//! **GIF animation:** [`decode`] returns the **first frame** as a still [`Image`]
//! — the right behavior for a thumbnail / Quick Look / a gallery cell. Callers
//! that need the full animation (all frames + per-frame delays + disposal) use
//! [`rae_gif::decode_gif`] directly; that multi-frame surface intentionally stays
//! on `rae_gif` rather than being flattened into this still-image type.
//!
//! ## What encodes ([`encode`])
//! Only the two formats that have in-tree encoders: [`ImageFormat::Png`] (via
//! [`rae_png::encode_png`]) and [`ImageFormat::Jpeg`] (via [`rae_jpeg::encode_jpeg`]
//! at the requested quality). This is the Photos "Save As PNG / JPEG" one-call
//! path. Every other format → [`ImageError::UnsupportedEncode`] (honest — we do
//! not pretend to encode formats whose encoder does not exist).
//!
//! ## Never-panic posture (CLAUDE: decoders are the #1 RCE surface)
//! Every byte is attacker-controlled. This crate adds **no** `unwrap`/`expect`/
//! `panic`/raw-index path: the underlying decoders are already never-panic, and
//! the dispatch layer only sniffs, moves a `Vec`, and maps `Err`s. Empty,
//! truncated, garbage, and mis-named input all return an [`ImageError`], proven by
//! the seeded fuzz loop in the host KAT suite (`cargo test -p rae_image`).

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

use alloc::vec::Vec;

pub use rae_formats::FileKind;

/// A decoded image in the one shared AthenaOS pixel model: a flat ARGB8888 buffer
/// plus dimensions. Each `u32` is `0xAARRGGBB` — byte-identical to every
/// underlying decoder's output, so re-homing one is a field move, never a convert.
///
/// `pixels.len() == (width * height) as usize`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u32>,
}

impl Image {
    /// Sample a pixel as `(a, r, g, b)`. `None` out of bounds — callers/tests use
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

/// A format this dispatcher can **encode** to. (Decode auto-detects every
/// supported format from bytes; only encode needs the caller to name a target.)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageFormat {
    /// PNG, lossless. Encoded via [`rae_png::encode_png`].
    Png,
    /// JPEG, baseline lossy, with a 1..=100 quality. Encoded via
    /// [`rae_jpeg::encode_jpeg`].
    Jpeg { quality: u8 },
}

/// A unified decode/encode error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageError {
    /// The bytes sniffed to a [`FileKind`] this dispatcher has no decoder for
    /// (including [`FileKind::Unknown`] for undetected / garbage input).
    UnsupportedFormat(FileKind),
    /// The format was recognized and dispatched, but its decoder returned an
    /// error. Carries which format failed plus a short reason string.
    Decode {
        format: &'static str,
        reason: &'static str,
    },
    /// [`encode`] was asked for a format with no in-tree encoder. Honest: we only
    /// encode PNG and JPEG.
    UnsupportedEncode(&'static str),
    /// An encoder returned an error (e.g. dimensions out of range, bad quality).
    Encode {
        format: &'static str,
        reason: &'static str,
    },
}

/// Report the detected format's canonical name (e.g. `"png"`) so a caller can show
/// the type, or `None` if the bytes did not sniff to any known kind. This is a
/// pure detection peek — it does not decode.
pub fn format_of(bytes: &[u8]) -> Option<&'static str> {
    match rae_formats::detect(bytes) {
        FileKind::Unknown => None,
        k => Some(k.extension()),
    }
}

/// Detect the file's [`FileKind`] from its bytes (re-exported sniffer), so a caller
/// can branch on the exact kind (e.g. to route ICO or GIF-animation to a
/// format-specific path). Content only — see [`rae_formats::detect_with_hint`].
pub fn detect(bytes: &[u8]) -> FileKind {
    rae_formats::detect(bytes)
}

/// Decode any supported still image from its bytes into a unified [`Image`].
///
/// The format is chosen by **magic bytes** ([`rae_formats::detect`]), never the
/// extension — a PNG body named `photo.jpg` still decodes as PNG. See the crate
/// docs for the supported-format table and the GIF first-frame note.
///
/// Never panics: an unsupported/undetected kind → [`ImageError::UnsupportedFormat`];
/// a decoder failure → [`ImageError::Decode`].
pub fn decode(bytes: &[u8]) -> Result<Image, ImageError> {
    dispatch(rae_formats::detect(bytes), bytes)
}

/// Like [`decode`], but an optional filename/extension hint disambiguates a true
/// tie that the content alone cannot resolve (delegated to
/// [`rae_formats::detect_with_hint`]). **Content always wins** — the hint never
/// overrides a clear magic match. For images this rarely matters (every image
/// format here has a strong magic signature); it exists for symmetry with the
/// sniffer and to honor a hint when bytes are genuinely ambiguous.
pub fn decode_with_hint(bytes: &[u8], filename_or_ext: Option<&str>) -> Result<Image, ImageError> {
    dispatch(rae_formats::detect_with_hint(bytes, filename_or_ext), bytes)
}

/// The shared dispatch core: route an already-detected [`FileKind`] to its decoder
/// and re-home the result into [`Image`]. Keeping this in one place means
/// [`decode`] and [`decode_with_hint`] cannot drift.
fn dispatch(kind: FileKind, bytes: &[u8]) -> Result<Image, ImageError> {
    match kind {
        FileKind::Png => rae_png::decode_png(bytes)
            .map(|img| Image {
                width: img.width,
                height: img.height,
                pixels: img.pixels,
            })
            .map_err(|e| ImageError::Decode {
                format: "png",
                reason: png_reason(e),
            }),
        FileKind::Jpeg => rae_jpeg::decode_jpeg(bytes)
            .map(|img| Image {
                width: img.width,
                height: img.height,
                pixels: img.pixels,
            })
            .map_err(|e| ImageError::Decode {
                format: "jpeg",
                reason: jpeg_reason(e),
            }),
        FileKind::Bmp => rae_bmp::decode_bmp(bytes)
            .map(|img| Image {
                width: img.width,
                height: img.height,
                pixels: img.pixels,
            })
            .map_err(|e| ImageError::Decode {
                format: "bmp",
                reason: bmp_reason(e),
            }),
        FileKind::Gif => match rae_gif::decode_gif(bytes) {
            Ok(g) => {
                // First frame only — the still-image view of a GIF (thumbnail /
                // Quick Look). Full animation stays on rae_gif::decode_gif.
                let frame = g.frames.into_iter().next().ok_or(ImageError::Decode {
                    format: "gif",
                    reason: "no frames",
                })?;
                Ok(Image {
                    width: g.width,
                    height: g.height,
                    pixels: frame.pixels,
                })
            }
            Err(e) => Err(ImageError::Decode {
                format: "gif",
                reason: gif_reason(e),
            }),
        },
        FileKind::Webp => rae_webp::decode_webp(bytes)
            .map(|img| Image {
                width: img.width,
                height: img.height,
                pixels: img.pixels,
            })
            .map_err(|e| ImageError::Decode {
                format: "webp",
                reason: webp_reason(e),
            }),
        other => Err(ImageError::UnsupportedFormat(other)),
    }
}

/// Encode a unified [`Image`] to the requested [`ImageFormat`].
///
/// Only PNG and JPEG have in-tree encoders; any other target →
/// [`ImageError::UnsupportedEncode`] (honest). This is the Photos "Save As" path.
/// Never panics: an encoder error → [`ImageError::Encode`].
pub fn encode(img: &Image, format: ImageFormat) -> Result<Vec<u8>, ImageError> {
    match format {
        ImageFormat::Png => {
            let png = rae_png::PngImage {
                width: img.width,
                height: img.height,
                pixels: img.pixels.clone(),
            };
            rae_png::encode_png(&png).map_err(|e| ImageError::Encode {
                format: "png",
                reason: png_encode_reason(e),
            })
        }
        ImageFormat::Jpeg { quality } => {
            let jpeg = rae_jpeg::JpegImage {
                width: img.width,
                height: img.height,
                pixels: img.pixels.clone(),
            };
            rae_jpeg::encode_jpeg(&jpeg, quality).map_err(|e| ImageError::Encode {
                format: "jpeg",
                reason: jpeg_encode_reason(e),
            })
        }
    }
}

// ─── Error → reason-string mappings (thin, exhaustive, no panic) ─────────────
//
// Each underlying decoder has its own error enum; we collapse it to a short
// static reason so callers get a uniform `ImageError` without this crate having
// to re-export five foreign enums. Matches are exhaustive so a new variant
// upstream is a compile error here, not a silent mis-label.

fn png_reason(e: rae_png::PngError) -> &'static str {
    use rae_png::PngError::*;
    match e {
        BadSignature => "bad signature",
        Truncated => "truncated",
        BadCrc => "bad crc",
        BadHeader => "bad header",
        DimensionsOutOfRange => "dimensions out of range",
        BadColorType => "bad color type",
        BadPalette => "bad palette",
        Unsupported => "unsupported feature",
        UnknownCriticalChunk => "unknown critical chunk",
        InflateFailed => "inflate failed",
        BadImageData => "bad image data",
        BadFilter => "bad filter",
        NoImageData => "no image data",
    }
}

fn png_encode_reason(e: rae_png::PngEncodeError) -> &'static str {
    use rae_png::PngEncodeError::*;
    match e {
        DimensionsOutOfRange => "dimensions out of range",
        PixelCountMismatch => "pixel count mismatch",
    }
}

fn jpeg_reason(e: rae_jpeg::JpegError) -> &'static str {
    use rae_jpeg::JpegError::*;
    match e {
        BadSignature => "bad signature",
        Truncated => "truncated",
        BadMarker => "bad marker",
        DimensionsOutOfRange => "dimensions out of range",
        Unsupported => "unsupported feature",
        BadQuantTable => "bad quant table",
        BadHuffmanTable => "bad huffman table",
        BadFrame => "bad frame",
        BadScan => "bad scan",
        BadEntropy => "bad entropy",
        NoFrame => "no frame",
    }
}

fn jpeg_encode_reason(e: rae_jpeg::JpegEncodeError) -> &'static str {
    use rae_jpeg::JpegEncodeError::*;
    match e {
        DimensionsOutOfRange => "dimensions out of range",
        PixelCountMismatch => "pixel count mismatch",
        BadQuality => "bad quality",
    }
}

fn bmp_reason(e: rae_bmp::BmpError) -> &'static str {
    use rae_bmp::BmpError::*;
    match e {
        Truncated => "truncated",
        BadSignature => "bad signature",
        DimensionsOutOfRange => "dimensions out of range",
        Unsupported => "unsupported feature",
        BadOffset => "bad offset",
        BadPaletteIndex => "bad palette index",
        MissingPalette => "missing palette",
        BadIconDir => "bad icon dir",
        BadIconEntry => "bad icon entry",
        NoEntries => "no entries",
    }
}

fn gif_reason(e: rae_gif::GifError) -> &'static str {
    use rae_gif::GifError::*;
    match e {
        BadSignature => "bad signature",
        Truncated => "truncated",
        DimensionsOutOfRange => "dimensions out of range",
        BadColorIndex => "bad color index",
        BadLzwCodeSize => "bad lzw code size",
        LzwError => "lzw error",
        LzwOutputTooLarge => "lzw output too large",
        TooManyFrames => "too many frames",
        NoColorTable => "no color table",
        NoImageData => "no image data",
    }
}

fn webp_reason(e: rae_webp::WebpError) -> &'static str {
    use rae_webp::WebpError::*;
    match e {
        Truncated => "truncated",
        NotWebp => "not webp",
        NoImageData => "no image data",
        DimensionsOutOfRange => "dimensions out of range",
        BadVp8l => "bad vp8l",
        BadHuffman => "bad huffman",
        BadBackref => "bad backref",
        BadTransform => "bad transform",
        BadImageData => "bad image data",
        UnsupportedLossy => "unsupported lossy",
        UnsupportedAnimation => "unsupported animation",
    }
}

// ════════════════════════════════════════════════════════════════════════════
// Host KAT suite — `cargo test -p rae_image`. FAIL-able by construction.
//
// The dispatcher owns no decode logic, so the proof target is the DISPATCH:
//   1. each format is routed to the right decoder and re-homed pixel-exact,
//   2. the decoder is chosen by MAGIC BYTES even with a wrong/missing extension
//      (the detect-not-extension security property — criterion #6),
//   3. encode dispatch round-trips (PNG exact, JPEG within tolerance) and an
//      unencodable format is an honest UnsupportedEncode,
//   4. garbage / empty / truncated / mis-detected input never panics.
//
// Fixtures are produced by ENCODING through the in-tree encoders (PNG/JPEG) or by
// hand-building minimal real headers (BMP/WebP-VP8L/GIF) so each pixel assert is
// concrete and a one-pixel tweak fails the test.
//
// Under #[cfg(test)] the crate compiles as `std` (the no_std attr is
// cfg_attr(not(test), ...)), so Vec/vec! are in the default prelude — no
// `extern crate std` / `use std::` lines (the architecture gate bans those).
// ════════════════════════════════════════════════════════════════════════════
#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use alloc::vec::Vec;

    // ── fixture builders ─────────────────────────────────────────────────────

    /// A known 2x2 ARGB image used across the round-trip tests.
    ///   (0,0)=opaque red   (1,0)=opaque green
    ///   (0,1)=opaque blue  (1,1)=opaque white
    fn sample_2x2() -> Image {
        Image {
            width: 2,
            height: 2,
            pixels: vec![
                0xFFFF0000, 0xFF00FF00, // red, green
                0xFF0000FF, 0xFFFFFFFF, // blue, white
            ],
        }
    }

    /// Encode the sample as a real PNG via the in-tree encoder.
    fn png_bytes() -> Vec<u8> {
        let img = sample_2x2();
        let png = rae_png::PngImage {
            width: img.width,
            height: img.height,
            pixels: img.pixels.clone(),
        };
        rae_png::encode_png(&png).expect("encode png fixture")
    }

    /// Encode the sample as a real JPEG (q90) via the in-tree encoder.
    fn jpeg_bytes() -> Vec<u8> {
        let img = sample_2x2();
        let jpeg = rae_jpeg::JpegImage {
            width: img.width,
            height: img.height,
            pixels: img.pixels.clone(),
        };
        rae_jpeg::encode_jpeg(&jpeg, 90).expect("encode jpeg fixture")
    }

    /// Hand-build a minimal 24-bpp bottom-up BMP of the 2x2 sample. BMP rows are
    /// stored bottom-up and BGR, padded to a 4-byte boundary.
    fn bmp_bytes() -> Vec<u8> {
        // 2 px * 3 bytes = 6 bytes/row, padded to 8 (one pad byte * 2).
        let row_stride = 8usize;
        let pixel_data_size = row_stride * 2;
        let file_header = 14usize;
        let info_header = 40usize;
        let offset = file_header + info_header;
        let total = offset + pixel_data_size;

        let mut v = Vec::new();
        // BITMAPFILEHEADER
        v.extend_from_slice(b"BM");
        v.extend_from_slice(&(total as u32).to_le_bytes()); // file size
        v.extend_from_slice(&[0, 0, 0, 0]); // reserved
        v.extend_from_slice(&(offset as u32).to_le_bytes()); // pixel data offset
                                                             // BITMAPINFOHEADER
        v.extend_from_slice(&40u32.to_le_bytes()); // header size
        v.extend_from_slice(&2i32.to_le_bytes()); // width
        v.extend_from_slice(&2i32.to_le_bytes()); // height (positive = bottom-up)
        v.extend_from_slice(&1u16.to_le_bytes()); // planes
        v.extend_from_slice(&24u16.to_le_bytes()); // bpp
        v.extend_from_slice(&0u32.to_le_bytes()); // compression BI_RGB
        v.extend_from_slice(&(pixel_data_size as u32).to_le_bytes()); // image size
        v.extend_from_slice(&0i32.to_le_bytes()); // xppm
        v.extend_from_slice(&0i32.to_le_bytes()); // yppm
        v.extend_from_slice(&0u32.to_le_bytes()); // colors used
        v.extend_from_slice(&0u32.to_le_bytes()); // important colors

        // Pixel data, BOTTOM-UP: row y=1 of the image first, then y=0. BGR order.
        // image row y=1: blue(0,1), white(1,1)
        v.extend_from_slice(&[0xFF, 0x00, 0x00]); // blue  -> B=255,G=0,R=0
        v.extend_from_slice(&[0xFF, 0xFF, 0xFF]); // white
        v.extend_from_slice(&[0x00, 0x00]); // pad to 8
                                            // image row y=0: red(0,0), green(1,0)
        v.extend_from_slice(&[0x00, 0x00, 0xFF]); // red -> B=0,G=0,R=255
        v.extend_from_slice(&[0x00, 0xFF, 0x00]); // green
        v.extend_from_slice(&[0x00, 0x00]); // pad

        v
    }

    /// Hand-build a minimal lossless VP8L WebP carrying a solid-color image, using
    /// the EXACT bit layout rae_webp's own KAT corpus uses (a 1x1 image, no
    /// transforms, no color cache, no meta-Huffman, five single-symbol "simple"
    /// Huffman code groups). rae_webp has no encoder, so this hand-built fixture is
    /// the proven way to feed the dispatcher a real VP8L pixel.
    ///
    /// VP8L bit layout (LSB-first within each byte): signature 0x2F (8 bits in the
    /// bitstream), 14-bit (w-1), 14-bit (h-1), 1 alpha-used, 3 version, the
    /// transform-present flag (0), then in `decode_image_data`: color-cache flag
    /// (0), meta-Huffman flag (0), then the 5 simple code groups (green, red, blue,
    /// alpha, distance). A 1-symbol simple code decodes with zero consumed bits, so
    /// the single pixel needs no payload bits.
    fn webp_solid(width: u32, height: u32, a: u8, r: u8, g: u8, b: u8) -> Vec<u8> {
        let mut w = BitWriter::new();
        w.put(0x2F, 8); // signature
        w.put(width - 1, 14);
        w.put(height - 1, 14);
        w.put(0, 1); // alpha_used
        w.put(0, 3); // version
        w.put(0, 1); // no transform
        w.put(0, 1); // no color cache
        w.put(0, 1); // no meta-huffman
                     // simple 1-symbol code: bit1(simple), bit0(num-1=0), bit1(8-bit), sym.
        let simple = |w: &mut BitWriter, sym: u32| {
            w.put(1, 1);
            w.put(0, 1);
            w.put(1, 1);
            w.put(sym, 8);
        };
        simple(&mut w, g as u32); // green = literal green value
        simple(&mut w, r as u32);
        simple(&mut w, b as u32);
        simple(&mut w, a as u32);
        simple(&mut w, 0); // distance (unused), 1 symbol
        let payload = w.into_bytes();

        // RIFF/WEBP envelope wrapping a single VP8L chunk.
        let mut body = Vec::new();
        body.extend_from_slice(b"VP8L");
        body.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        body.extend_from_slice(&payload);
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

    /// A tiny LSB-first bit writer for hand-building the VP8L bitstream.
    struct BitWriter {
        bytes: Vec<u8>,
        cur: u32,
        nbits: u32,
    }
    impl BitWriter {
        fn new() -> Self {
            BitWriter {
                bytes: Vec::new(),
                cur: 0,
                nbits: 0,
            }
        }
        fn put(&mut self, val: u32, bits: u32) {
            for i in 0..bits {
                let b = (val >> i) & 1;
                self.cur |= b << self.nbits;
                self.nbits += 1;
                if self.nbits == 8 {
                    self.bytes.push(self.cur as u8);
                    self.cur = 0;
                    self.nbits = 0;
                }
            }
        }
        fn into_bytes(mut self) -> Vec<u8> {
            if self.nbits > 0 {
                self.bytes.push(self.cur as u8);
            }
            self.bytes
        }
    }

    /// Hand-build a minimal single-frame GIF89a of a 2x1 image: pixel 0 = red,
    /// pixel 1 = green, from a 2-color global table. LZW for 2-px images is small.
    fn gif_bytes() -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(b"GIF89a");
        // Logical Screen Descriptor: width=2, height=1, packed (GCT present,
        // 2 colors -> size code 0 => 2^(0+1)=2 entries), bg=0, aspect=0.
        v.extend_from_slice(&2u16.to_le_bytes());
        v.extend_from_slice(&1u16.to_le_bytes());
        v.push(0b1000_0000); // GCT flag=1, color res=0, sort=0, GCT size=0 (2 colors)
        v.push(0); // background color index
        v.push(0); // pixel aspect ratio
                   // Global Color Table: 2 entries (red, green)
        v.extend_from_slice(&[255, 0, 0]); // index 0 red
        v.extend_from_slice(&[0, 255, 0]); // index 1 green
                                           // Image Descriptor
        v.push(0x2C);
        v.extend_from_slice(&0u16.to_le_bytes()); // left
        v.extend_from_slice(&0u16.to_le_bytes()); // top
        v.extend_from_slice(&2u16.to_le_bytes()); // width
        v.extend_from_slice(&1u16.to_le_bytes()); // height
        v.push(0); // no LCT, not interlaced
                   // Image data: LZW min code size = 2.
        v.push(2);
        // Build the LZW code stream for indices [0, 1] with min code size 2:
        //   clear code = 4 (2^2), EOI = 5. Initial code size = 3 bits.
        //   emit: CLEAR(4), 0, 1, EOI(5).
        // Pack LSB-first into bytes, all codes 3 bits here.
        let codes: &[u32] = &[4, 0, 1, 5];
        let mut bw = BitWriter::new();
        for &c in codes {
            bw.put(c, 3);
        }
        let data = bw.into_bytes();
        v.push(data.len() as u8); // sub-block length
        v.extend_from_slice(&data);
        v.push(0x00); // block terminator
        v.push(0x3B); // trailer
        v
    }

    // ── 1. PNG dispatch: exact pixels (the load-bearing assert) ──────────────

    #[test]
    fn decode_png_exact() {
        let bytes = png_bytes();
        let img = decode(&bytes).expect("png decode via dispatcher");
        assert_eq!((img.width, img.height), (2, 2));
        assert_eq!(img.pixel(0, 0), Some((0xFF, 255, 0, 0))); // red
        assert_eq!(img.pixel(1, 0), Some((0xFF, 0, 255, 0))); // green
        assert_eq!(img.pixel(0, 1), Some((0xFF, 0, 0, 255))); // blue
        assert_eq!(img.pixel(1, 1), Some((0xFF, 255, 255, 255))); // white
                                                                  // FAIL-ability: dispatching to the wrong decoder, or an R/B swap, would
                                                                  // make (0,0) not-red.
        assert_ne!(img.pixel(0, 0), Some((0xFF, 0, 0, 255)));
    }

    // ── 2. JPEG dispatch: within lossy tolerance ─────────────────────────────

    #[test]
    fn decode_jpeg_tolerance() {
        let bytes = jpeg_bytes();
        let img = decode(&bytes).expect("jpeg decode via dispatcher");
        assert_eq!((img.width, img.height), (2, 2));
        // JPEG is lossy; assert each corner is near its true color.
        let (_, r, g, b) = img.pixel(0, 0).expect("in bounds");
        assert!(
            r > 150 && g < 120 && b < 120,
            "(0,0) should be reddish: {r},{g},{b}"
        );
        let (_, r, g, b) = img.pixel(1, 1).expect("in bounds");
        assert!(
            r > 180 && g > 180 && b > 180,
            "(1,1) should be whitish: {r},{g},{b}"
        );
    }

    // ── 3. BMP dispatch: exact pixels ────────────────────────────────────────

    #[test]
    fn decode_bmp_exact() {
        let bytes = bmp_bytes();
        // sanity: it must sniff as BMP first.
        assert_eq!(rae_formats::detect(&bytes), FileKind::Bmp);
        let img = decode(&bytes).expect("bmp decode via dispatcher");
        assert_eq!((img.width, img.height), (2, 2));
        assert_eq!(img.pixel(0, 0), Some((0xFF, 255, 0, 0))); // red
        assert_eq!(img.pixel(1, 0), Some((0xFF, 0, 255, 0))); // green
        assert_eq!(img.pixel(0, 1), Some((0xFF, 0, 0, 255))); // blue
        assert_eq!(img.pixel(1, 1), Some((0xFF, 255, 255, 255))); // white
    }

    // ── 4. WebP (VP8L) dispatch: exact single pixel ──────────────────────────

    #[test]
    fn decode_webp_vp8l() {
        // A 2x1 solid green image (A=255, R=0, G=255, B=0).
        let bytes = webp_solid(2, 1, 255, 0, 255, 0);
        assert_eq!(rae_formats::detect(&bytes), FileKind::Webp);
        let img = decode(&bytes).expect("webp decode via dispatcher");
        assert_eq!((img.width, img.height), (2, 1));
        assert_eq!(img.pixel(0, 0), Some((0xFF, 0, 255, 0))); // green, opaque
        assert_eq!(img.pixel(1, 0), Some((0xFF, 0, 255, 0)));
        // FAIL-ability: a channel mix-up would not be pure green.
        assert_ne!(img.pixel(0, 0), Some((0xFF, 255, 0, 0)));
    }

    // ── 5. GIF dispatch: first frame, exact pixels ───────────────────────────

    #[test]
    fn decode_gif_first_frame() {
        let bytes = gif_bytes();
        assert_eq!(rae_formats::detect(&bytes), FileKind::Gif);
        let img = decode(&bytes).expect("gif decode via dispatcher");
        assert_eq!((img.width, img.height), (2, 1));
        assert_eq!(img.pixel(0, 0), Some((0xFF, 255, 0, 0))); // red
        assert_eq!(img.pixel(1, 0), Some((0xFF, 0, 255, 0))); // green
    }

    // ── 6. THE magic-not-extension proof (criterion #6 security) ─────────────

    #[test]
    fn magic_bytes_beat_wrong_extension() {
        let png = png_bytes();
        // PNG bytes named "x.jpg" must STILL decode as PNG — content wins.
        let img = decode_with_hint(&png, Some("x.jpg")).expect("png-as-jpg decode");
        assert_eq!(img.pixel(0, 0), Some((0xFF, 255, 0, 0)));
        assert_eq!(format_of(&png), Some("png"));

        // And the reverse: JPEG bytes named "photo.png" decode as JPEG (no crash,
        // correct routing). We assert it does NOT error as a PNG decode.
        let jpeg = jpeg_bytes();
        let img = decode_with_hint(&jpeg, Some("photo.png")).expect("jpeg-as-png decode");
        assert_eq!((img.width, img.height), (2, 2));
        assert_eq!(format_of(&jpeg), Some("jpg"));

        // A bare extension with no body content still resolves by content.
        let plain_png = decode(&png).expect("plain decode");
        assert_eq!(plain_png.pixel(1, 0), Some((0xFF, 0, 255, 0)));
    }

    // ── 7. Encode dispatch: PNG round-trips exactly ──────────────────────────

    #[test]
    fn encode_png_roundtrip() {
        let img = sample_2x2();
        let bytes = encode(&img, ImageFormat::Png).expect("encode png");
        assert_eq!(rae_formats::detect(&bytes), FileKind::Png);
        let back = decode(&bytes).expect("decode encoded png");
        assert_eq!(back.width, img.width);
        assert_eq!(back.height, img.height);
        assert_eq!(back.pixels, img.pixels); // lossless: byte-exact
    }

    // ── 8. Encode dispatch: JPEG round-trips within tolerance ────────────────

    #[test]
    fn encode_jpeg_roundtrip() {
        let img = sample_2x2();
        let bytes = encode(&img, ImageFormat::Jpeg { quality: 90 }).expect("encode jpeg");
        assert_eq!(rae_formats::detect(&bytes), FileKind::Jpeg);
        let back = decode(&bytes).expect("decode encoded jpeg");
        assert_eq!((back.width, back.height), (2, 2));
        let (_, r, g, b) = back.pixel(0, 0).expect("in bounds");
        assert!(r > 150 && g < 120 && b < 120, "reddish: {r},{g},{b}");
    }

    // ── 9. Encode of an unencodable format → honest UnsupportedEncode ────────
    //
    // GIF has no in-tree encoder. The ImageFormat enum only exposes Png/Jpeg, so
    // "encode as GIF" is unrepresentable by construction — which IS the honest
    // outcome. We instead prove the encoder rejects a malformed Image (a pixel
    // count that doesn't match dimensions) with an Encode error, and that a bad
    // JPEG quality is surfaced — i.e. the error mapping is wired, not swallowed.

    #[test]
    fn encode_rejects_bad_image_and_quality() {
        // Dimensions say 4 pixels, but only 1 supplied → encoder error (mapped).
        let bad = Image {
            width: 2,
            height: 2,
            pixels: vec![0xFFFFFFFF],
        };
        let err = encode(&bad, ImageFormat::Png).unwrap_err();
        assert!(
            matches!(err, ImageError::Encode { format: "png", .. }),
            "{err:?}"
        );

        // Bad JPEG quality (0 is out of 1..=100) → mapped Encode error.
        let good = sample_2x2();
        let err = encode(&good, ImageFormat::Jpeg { quality: 0 }).unwrap_err();
        assert!(
            matches!(err, ImageError::Encode { format: "jpeg", .. }),
            "{err:?}"
        );
    }

    // ── 10. Unsupported / undetected kinds → UnsupportedFormat, no panic ─────

    #[test]
    fn unsupported_and_undetected() {
        // A real, detectable, but non-image kind: a PDF. Sniffs as Pdf, which the
        // image dispatcher does not handle → UnsupportedFormat(Pdf).
        let pdf = b"%PDF-1.7\n%abc\n";
        match decode(pdf) {
            Err(ImageError::UnsupportedFormat(k)) => assert_eq!(k, FileKind::Pdf),
            other => panic!("expected UnsupportedFormat(Pdf), got {other:?}"),
        }

        // Undetected garbage → Unknown → UnsupportedFormat(Unknown).
        let junk = b"\x01\x02\x03 not a known magic \xff\xfe";
        match decode(junk) {
            Err(ImageError::UnsupportedFormat(FileKind::Unknown)) => {}
            other => panic!("expected UnsupportedFormat(Unknown), got {other:?}"),
        }
        assert_eq!(format_of(junk), None);
    }

    // ── 11. Corrupt-but-detected → Decode error (not UnsupportedFormat) ──────

    #[test]
    fn corrupt_png_is_decode_error() {
        // Valid PNG signature so it SNIFFS as PNG, but the chunk stream is junk.
        let mut bad = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
        bad.extend_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF, 0x00, 0x00, 0x00, 0x00]);
        match decode(&bad) {
            Err(ImageError::Decode { format: "png", .. }) => {}
            other => panic!("expected png Decode error, got {other:?}"),
        }
    }

    // ── 12. Empty / truncated input never panics ─────────────────────────────

    #[test]
    fn empty_and_truncated_no_panic() {
        assert!(decode(&[]).is_err());
        assert!(decode(b"P").is_err());
        // Truncated PNG signature (7 of 8 bytes).
        assert!(decode(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A]).is_err());
        // Truncated JPEG (just SOI).
        assert!(decode(&[0xFF, 0xD8]).is_err());
        // decode_with_hint on empty, no panic.
        assert!(decode_with_hint(&[], Some("x.png")).is_err());
    }

    // ── 13. Seeded fuzz over decode(): never panics on any input ─────────────

    #[test]
    fn seeded_fuzz_never_panics() {
        // A deterministic LCG generates varied byte buffers, including ones that
        // start with each real magic prefix (so the corrupt-after-magic paths in
        // every decoder are exercised). The only contract: decode never panics.
        let magics: &[&[u8]] = &[
            &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A], // PNG
            &[0xFF, 0xD8, 0xFF],                               // JPEG
            b"BM",                                             // BMP
            b"GIF89a",                                         // GIF
            b"RIFF\x00\x00\x00\x00WEBP",                       // WebP
            &[],                                               // none
        ];
        let mut state: u64 = 0x1234_5678_9abc_def0;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        for iter in 0..4000u32 {
            let m = magics[(iter as usize) % magics.len()];
            let extra = (next() % 64) as usize;
            let mut buf: Vec<u8> = Vec::with_capacity(m.len() + extra);
            buf.extend_from_slice(m);
            for _ in 0..extra {
                buf.push((next() & 0xFF) as u8);
            }
            // Must not panic; the verdict is irrelevant.
            let _ = decode(&buf);
            let _ = decode_with_hint(&buf, Some("f.png"));
            let _ = format_of(&buf);
        }
    }
}
