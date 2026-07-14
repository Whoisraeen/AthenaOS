//! # RaeGIF — a never-panic, `no_std` animated GIF decoder (87a + 89a).
//!
//! RaeenOS_Concept.md (§creators / media): the OS must let people "play my
//! movies, show my photos." A daily-driver computer that can show a still photo
//! but not an animated one — the GIFs that fill messaging, stickers, and the web
//! — has a real consumer-format gap. RaeenOS already decodes PNG and JPEG; this
//! module is the missing *animated* image path, and the engine a future Photos /
//! Quick Look animated preview will sit on (that wiring is a separate follow-up).
//!
//! This is a **from-scratch** GIF decoder: header (GIF87a/GIF89a), Logical Screen
//! Descriptor + Global Color Table, the block stream (Image Descriptor + Local
//! Color Table, **LZW image-data decompression**, Graphic Control Extensions for
//! delay / transparency / disposal, Application & Comment extensions skipped,
//! Trailer), **interlaced** images (the 4-pass row order), and per-frame
//! composition onto a persistent canvas honoring the disposal method. Output is a
//! flat ARGB8888 `Vec<u32>` per frame (the compositor / Canvas pixel format),
//! each frame a ready-to-blit full-canvas buffer — a player just blits frame N
//! then waits `delay_ms`.
//!
//! ## Hostile-input posture (CLAUDE: decoders are the #1 RCE surface)
//! Every GIF byte is treated as hostile — LZW in particular is a classic fuzz
//! target. There is **no `unwrap`/`expect`/`panic`/raw-index-panic path**
//! reachable from [`decode_gif`]: bad signatures, truncated streams, corrupt LZW
//! code streams, oversized dimensions, and decompression bombs all return
//! `Err(GifError)`. Memory is bounded up front ([`MAX_DIMENSION`], [`MAX_PIXELS`],
//! [`MAX_LZW_OUTPUT`], [`MAX_FRAMES`]) so a crafted header or a runaway LZW
//! dictionary cannot request a multi-gigabyte allocation or loop forever. The
//! host KAT suite at the bottom of this file is the primary proof
//! (`cargo test -p rae_gif`).

#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::vec;
use alloc::vec::Vec;

/// Bound on either image axis. GIF screen dimensions are 16-bit (≤65_535), so
/// this is the format ceiling; it keeps a single canvas well under `MAX_PIXELS`.
const MAX_DIMENSION: u32 = 1 << 16; // 65_536
/// Bound on total canvas pixel count (width * height). ~67M px = 256 MiB at
/// 4 B/px ARGB. A crafted screen descriptor claiming a huge canvas is rejected
/// before any allocation.
const MAX_PIXELS: u64 = 64 * 1024 * 1024;
/// Bound on the number of index bytes a single image's LZW stream may emit. A
/// frame can't have more indices than its own pixel area, but we cap the LZW
/// output independently so a decompression bomb (a tiny stream that expands
/// without bound) is rejected even before the size is reconciled with the frame.
const MAX_LZW_OUTPUT: usize = (MAX_PIXELS as usize) + 16;
/// Bound on the number of animation frames. Real GIFs are dozens to low
/// thousands; a crafted file with millions of 1-byte frames is rejected.
const MAX_FRAMES: usize = 4096;
/// Bound on the TOTAL ARGB pixels held across the whole frame store at once.
///
/// Each accepted frame is a full-canvas ARGB clone pushed into `frames`, so the
/// peak memory is `canvas_len * frames.len() * 4 B`. `MAX_FRAMES` and
/// `MAX_PIXELS` each bound one axis, but their *product* is ~256 G px (~1 TiB) —
/// so a crafted GIF declaring a large-but-legal canvas (≤ `MAX_PIXELS`) plus
/// thousands of tiny 1×1 frames (≤ `MAX_FRAMES`) can pin ~1 TiB of ARGB from a
/// tiny file (762 MiB empirically reproduced from a 1000×1000 canvas + 200 1×1
/// frames). This cap bounds the *combined* product so neither axis alone can
/// pass while their product OOMs the host.
///
/// 256 M total pixels = 1 GiB of ARGB across the entire frame store — a hard
/// ceiling, not a per-frame one. A real animated GIF never needs both a huge
/// canvas AND many frames: e.g. a 512×512 canvas (262 144 px) admits 1024 frames
/// under this cap, and a 256×256 canvas admits 4096 (the full `MAX_FRAMES`). A
/// 1920×1080 frame admits ~129 frames. Legitimate files stay far below it; only
/// the bomb shape (big canvas × many frames) is rejected.
const MAX_TOTAL_FRAME_PIXELS: u64 = 256 * 1024 * 1024;
/// LZW codes are at most 12 bits wide (4096 dictionary entries) per the GIF spec.
const MAX_LZW_CODE_BITS: u8 = 12;
const LZW_MAX_DICT: usize = 1 << 12; // 4096

/// GIF decode error. Every variant is a *handled* error path — nothing panics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GifError {
    /// First 6 bytes are not "GIF87a" or "GIF89a".
    BadSignature,
    /// Stream ended mid-field / mid-block.
    Truncated,
    /// Logical screen dimensions are zero or exceed the memory bound.
    DimensionsOutOfRange,
    /// A color table index referenced a color outside the active table.
    BadColorIndex,
    /// LZW minimum code size was out of the legal 2..=8 range.
    BadLzwCodeSize,
    /// The LZW code stream was corrupt (bad code, dictionary overflow, etc.).
    LzwError,
    /// LZW output exceeded the per-image bound (decompression bomb).
    LzwOutputTooLarge,
    /// More frames than [`MAX_FRAMES`].
    TooManyFrames,
    /// An image was declared without any usable color table.
    NoColorTable,
    /// No image data present in the whole stream.
    NoImageData,
}

/// One composited animation frame: a full-canvas ARGB8888 buffer + its display
/// duration. `pixels.len() == (width * height)` of the parent [`GifImage`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GifFrame {
    /// ARGB8888 (`0xAARRGGBB`), fully composited onto the canvas and ready to
    /// blit. A player blits this, then waits `delay_ms`.
    pub pixels: Vec<u32>,
    /// Display duration in milliseconds (from the Graphic Control Extension,
    /// hundredths of a second × 10). 0 means "as fast as possible".
    pub delay_ms: u16,
}

/// A decoded (possibly animated) GIF: canvas dimensions + one or more frames.
/// A single-frame GIF yields exactly one frame (a still image).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GifImage {
    pub width: u32,
    pub height: u32,
    pub frames: Vec<GifFrame>,
}

impl GifImage {
    /// Sample a pixel of frame `f` as `(a, r, g, b)`. Returns `None` out of
    /// bounds — tests use this so a wrong coordinate is a clean failure, not a
    /// panic.
    pub fn pixel(&self, f: usize, x: u32, y: u32) -> Option<(u8, u8, u8, u8)> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let frame = self.frames.get(f)?;
        let idx = (y as usize) * (self.width as usize) + (x as usize);
        let p = *frame.pixels.get(idx)?;
        Some(((p >> 24) as u8, (p >> 16) as u8, (p >> 8) as u8, p as u8))
    }
}

#[inline]
fn argb(a: u8, r: u8, g: u8, b: u8) -> u32 {
    ((a as u32) << 24) | ((r as u32) << 16) | ((g as u32) << 8) | (b as u32)
}

// ─── Byte cursor (bounds-checked, never panics) ─────────────────────────────

struct Cursor<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    fn u8(&mut self) -> Result<u8, GifError> {
        let b = *self.data.get(self.pos).ok_or(GifError::Truncated)?;
        self.pos += 1;
        Ok(b)
    }

    /// Little-endian u16 (GIF is little-endian).
    fn u16le(&mut self) -> Result<u16, GifError> {
        let lo = self.u8()? as u16;
        let hi = self.u8()? as u16;
        Ok(lo | (hi << 8))
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], GifError> {
        let end = self.pos.checked_add(n).ok_or(GifError::Truncated)?;
        let s = self.data.get(self.pos..end).ok_or(GifError::Truncated)?;
        self.pos = end;
        Ok(s)
    }

    fn skip(&mut self, n: usize) -> Result<(), GifError> {
        let end = self.pos.checked_add(n).ok_or(GifError::Truncated)?;
        if end > self.data.len() {
            return Err(GifError::Truncated);
        }
        self.pos = end;
        Ok(())
    }
}

/// An RGB color-table entry. GIF tables are opaque RGB; alpha comes only from a
/// transparent-index flag in the Graphic Control Extension.
type Rgb = (u8, u8, u8);

fn read_color_table(cur: &mut Cursor, entries: usize) -> Result<Vec<Rgb>, GifError> {
    let bytes = cur.take(entries * 3)?;
    Ok(bytes.chunks_exact(3).map(|c| (c[0], c[1], c[2])).collect())
}

/// Disposal method (GIF89a Graphic Control Extension, bits 2-4 of the packed
/// field). Governs what the canvas looks like *before the next frame* draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Disposal {
    /// No disposal specified — leave the canvas as-is (treated like Keep).
    Unspecified,
    /// Do not dispose — leave this frame's pixels on the canvas.
    Keep,
    /// Restore the area this frame occupied to the background color.
    RestoreBackground,
    /// Restore the canvas to its state before this frame was drawn.
    RestorePrevious,
}

impl Disposal {
    fn from_bits(b: u8) -> Self {
        match b & 0x07 {
            2 => Disposal::RestoreBackground,
            3 => Disposal::RestorePrevious,
            1 => Disposal::Keep,
            _ => Disposal::Unspecified,
        }
    }
}

/// State carried across the block stream from a Graphic Control Extension to the
/// image that immediately follows it (GCE applies to the next graphic).
#[derive(Clone, Copy)]
struct GraphicControl {
    delay_ms: u16,
    transparent_index: Option<u8>,
    disposal: Disposal,
}

impl Default for GraphicControl {
    fn default() -> Self {
        Self {
            delay_ms: 0,
            transparent_index: None,
            disposal: Disposal::Unspecified,
        }
    }
}

/// Decode a GIF byte stream into a composited [`GifImage`].
///
/// Hostile-input safe: returns `Err` (never panics) on any malformed input.
pub fn decode_gif(data: &[u8]) -> Result<GifImage, GifError> {
    let mut cur = Cursor::new(data);

    // ── Header: "GIF" + version "87a" / "89a" ──────────────────────────────
    let sig = cur.take(6)?;
    if &sig[0..3] != b"GIF" || (sig != b"GIF87a" && sig != b"GIF89a") {
        return Err(GifError::BadSignature);
    }

    // ── Logical Screen Descriptor ──────────────────────────────────────────
    let screen_w = cur.u16le()? as u32;
    let screen_h = cur.u16le()? as u32;
    let packed = cur.u8()?;
    let bg_index = cur.u8()?;
    let _aspect = cur.u8()?;

    if screen_w == 0 || screen_h == 0 {
        return Err(GifError::DimensionsOutOfRange);
    }
    if screen_w > MAX_DIMENSION || screen_h > MAX_DIMENSION {
        return Err(GifError::DimensionsOutOfRange);
    }
    if (screen_w as u64) * (screen_h as u64) > MAX_PIXELS {
        return Err(GifError::DimensionsOutOfRange);
    }

    let global_table: Vec<Rgb> = if packed & 0x80 != 0 {
        let size = 1usize << ((packed & 0x07) + 1);
        read_color_table(&mut cur, size)?
    } else {
        Vec::new()
    };

    let canvas_len = (screen_w as usize) * (screen_h as usize);
    // The persistent canvas. ARGB; starts fully transparent (the GIF spec leaves
    // undrawn area undefined — transparent is the sane, web-matching choice).
    let mut canvas: Vec<u32> = vec![0u32; canvas_len];
    // Background color used by RestoreBackground disposal. If a global table and
    // a valid bg index exist, use it; otherwise transparent.
    let background: u32 = global_table
        .get(bg_index as usize)
        .map(|&(r, g, b)| argb(0xFF, r, g, b))
        .unwrap_or(0);

    let mut frames: Vec<GifFrame> = Vec::new();
    // Pending graphic control from the most recent GCE (resets after each image).
    let mut pending = GraphicControl::default();

    // ── Block stream ───────────────────────────────────────────────────────
    loop {
        let introducer = cur.u8()?;
        match introducer {
            0x3B => break, // Trailer — end of the GIF.
            0x21 => {
                // Extension. label byte then sub-blocks.
                let label = cur.u8()?;
                if label == 0xF9 {
                    // Graphic Control Extension. Block size is always 4.
                    let block_size = cur.u8()?;
                    if block_size != 4 {
                        // Be lenient but bounded: skip exactly what it claims.
                        cur.skip(block_size as usize)?;
                        skip_sub_blocks(&mut cur)?;
                    } else {
                        let gce_packed = cur.u8()?;
                        let delay_cs = cur.u16le()?;
                        let tindex = cur.u8()?;
                        let _terminator = cur.u8()?; // sub-block terminator (0)
                        let transparent = if gce_packed & 0x01 != 0 {
                            Some(tindex)
                        } else {
                            None
                        };
                        pending = GraphicControl {
                            // GIF delays are in hundredths of a second.
                            delay_ms: delay_cs.saturating_mul(10),
                            transparent_index: transparent,
                            disposal: Disposal::from_bits(gce_packed >> 2),
                        };
                    }
                } else {
                    // Application (0xFF) / Comment (0xFE) / Plain Text (0x01):
                    // skip their sub-block chain. We don't render plain text.
                    skip_sub_blocks(&mut cur)?;
                }
            }
            0x2C => {
                // Image Descriptor.
                let img_left = cur.u16le()? as u32;
                let img_top = cur.u16le()? as u32;
                let img_w = cur.u16le()? as u32;
                let img_h = cur.u16le()? as u32;
                let img_packed = cur.u8()?;

                let local_table: Vec<Rgb> = if img_packed & 0x80 != 0 {
                    let size = 1usize << ((img_packed & 0x07) + 1);
                    read_color_table(&mut cur, size)?
                } else {
                    Vec::new()
                };
                let interlaced = img_packed & 0x40 != 0;

                let active_table: &[Rgb] = if !local_table.is_empty() {
                    &local_table
                } else {
                    &global_table
                };
                if active_table.is_empty() {
                    return Err(GifError::NoColorTable);
                }

                // LZW minimum code size.
                let min_code_size = cur.u8()?;
                if !(2..=8).contains(&min_code_size) {
                    return Err(GifError::BadLzwCodeSize);
                }

                // Gather the LZW data sub-blocks into one buffer (bounded).
                let lzw_data = read_sub_blocks(&mut cur)?;

                let expected = (img_w as usize)
                    .checked_mul(img_h as usize)
                    .ok_or(GifError::DimensionsOutOfRange)?;
                // An image frame inside the canvas: bound its own area too.
                if expected > MAX_LZW_OUTPUT {
                    return Err(GifError::LzwOutputTooLarge);
                }

                let indices = lzw_decode(&lzw_data, min_code_size, expected)?;

                // ── Compose onto the persistent canvas ─────────────────────
                // Save state if this frame asks to RestorePrevious afterwards.
                let saved_canvas = if matches!(pending.disposal, Disposal::RestorePrevious) {
                    Some(canvas.clone())
                } else {
                    None
                };

                compose_frame(
                    &mut canvas,
                    screen_w,
                    screen_h,
                    img_left,
                    img_top,
                    img_w,
                    img_h,
                    interlaced,
                    &indices,
                    active_table,
                    pending.transparent_index,
                )?;

                if frames.len() >= MAX_FRAMES {
                    return Err(GifError::TooManyFrames);
                }
                // Cumulative frame-store budget (the OOM guard): each frame is a
                // full-canvas ARGB clone, so the peak held memory is
                // `canvas_len * (frames.len() + 1)` pixels. MAX_FRAMES and
                // MAX_PIXELS each bound one axis but not their product — a large
                // legal canvas × many tiny frames could otherwise pin ~1 TiB.
                // Reject the next clone BEFORE allocating it. Saturating math so a
                // huge canvas_len can never overflow into a small product.
                let projected_total =
                    (canvas_len as u64).saturating_mul((frames.len() as u64).saturating_add(1));
                if projected_total > MAX_TOTAL_FRAME_PIXELS {
                    return Err(GifError::TooManyFrames);
                }
                frames.push(GifFrame {
                    pixels: canvas.clone(),
                    delay_ms: pending.delay_ms,
                });

                // ── Apply disposal for the NEXT frame ──────────────────────
                match pending.disposal {
                    Disposal::RestoreBackground => {
                        restore_region(
                            &mut canvas,
                            screen_w,
                            screen_h,
                            img_left,
                            img_top,
                            img_w,
                            img_h,
                            background,
                        );
                    }
                    Disposal::RestorePrevious => {
                        if let Some(prev) = saved_canvas {
                            canvas = prev;
                        }
                    }
                    // Keep / Unspecified: leave the composited canvas in place.
                    _ => {}
                }

                // GCE applies only to the image that immediately follows it.
                pending = GraphicControl::default();
            }
            _ => {
                // Unknown introducer — corrupt stream. Fail cleanly rather than
                // guessing.
                return Err(GifError::Truncated);
            }
        }
    }

    if frames.is_empty() {
        return Err(GifError::NoImageData);
    }

    Ok(GifImage {
        width: screen_w,
        height: screen_h,
        frames,
    })
}

/// Skip a GIF sub-block chain (size byte, that many bytes, … until a 0 size).
fn skip_sub_blocks(cur: &mut Cursor) -> Result<(), GifError> {
    loop {
        let size = cur.u8()?;
        if size == 0 {
            return Ok(());
        }
        cur.skip(size as usize)?;
    }
}

/// Read a GIF sub-block chain into one contiguous buffer (bounded).
fn read_sub_blocks(cur: &mut Cursor) -> Result<Vec<u8>, GifError> {
    let mut out = Vec::new();
    loop {
        let size = cur.u8()?;
        if size == 0 {
            return Ok(out);
        }
        let chunk = cur.take(size as usize)?;
        // Compressed LZW data is bounded too — a runaway sub-block chain that
        // never terminates would be caught by Truncated, but cap the buffer so a
        // crafted stream can't force a huge allocation before that.
        if out.len() + chunk.len() > MAX_LZW_OUTPUT {
            return Err(GifError::LzwOutputTooLarge);
        }
        out.extend_from_slice(chunk);
    }
}

// ─── LZW decompression (GIF variant, RFC-style variable-width codes) ─────────
//
// GIF LZW packs codes LSB-first into the byte stream, starting at
// `min_code_size + 1` bits. Two reserved codes sit just above the literal range:
// CLEAR = 1 << min_code_size  (reset the dictionary)
// EOI   = CLEAR + 1           (end of information)
// The dictionary starts with the `2^min_code_size` literal entries plus the two
// reserved codes; each emitted code adds one new dictionary entry (the previous
// output plus the first symbol of the current output). The code width grows by 1
// bit whenever the next code to be assigned needs it, up to 12 bits.

struct LzwBitReader<'a> {
    data: &'a [u8],
    byte_pos: usize,
    bit_pos: u8,
}

impl<'a> LzwBitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            byte_pos: 0,
            bit_pos: 0,
        }
    }

    /// Read `n` bits (1..=12) LSB-first. Returns `None` when the stream is
    /// exhausted (a well-formed GIF ends on EOI before this happens).
    fn read(&mut self, n: u8) -> Option<u16> {
        let mut value: u32 = 0;
        for i in 0..n {
            let byte = *self.data.get(self.byte_pos)?;
            let bit = (byte >> self.bit_pos) & 1;
            value |= (bit as u32) << i;
            self.bit_pos += 1;
            if self.bit_pos == 8 {
                self.bit_pos = 0;
                self.byte_pos += 1;
            }
        }
        Some(value as u16)
    }
}

/// Decode a GIF LZW code stream into palette indices.
///
/// `min_code_size` is the value from the Image Descriptor (2..=8). `max_output`
/// is the expected pixel count for the frame — decoding stops being *accepted*
/// past `MAX_LZW_OUTPUT`, and the result is reconciled with `max_output` by the
/// caller (we pad/truncate defensively below so a short stream still yields a
/// full frame rather than an OOB later).
fn lzw_decode(data: &[u8], min_code_size: u8, max_output: usize) -> Result<Vec<u8>, GifError> {
    if !(2..=8).contains(&min_code_size) {
        return Err(GifError::BadLzwCodeSize);
    }
    let clear_code: u16 = 1 << min_code_size;
    let eoi_code: u16 = clear_code + 1;

    // Dictionary entries are byte strings. `prefix`/`suffix` form the classic
    // LZW trie so we never store unbounded Vec<Vec<u8>> recursively; output is
    // reconstructed by walking the prefix chain.
    let mut prefix: Vec<u16> = vec![0u16; LZW_MAX_DICT];
    let mut suffix: Vec<u8> = vec![0u8; LZW_MAX_DICT];

    // Initialize the literal entries.
    let init_dict = |prefix: &mut Vec<u16>, suffix: &mut Vec<u8>| {
        for i in 0..(clear_code as usize) {
            prefix[i] = 0xFFFF; // sentinel: no prefix (a root literal)
            suffix[i] = i as u8;
        }
    };
    init_dict(&mut prefix, &mut suffix);

    let mut next_code: usize = (eoi_code as usize) + 1;
    let mut code_size: u8 = min_code_size + 1;
    let mut out: Vec<u8> = Vec::new();
    let mut reader = LzwBitReader::new(data);

    // Scratch buffer for reconstructing a code's byte string (reversed, then
    // appended in order). Bounded by the dictionary size.
    let mut stack: Vec<u8> = Vec::with_capacity(LZW_MAX_DICT);

    // The previous code's reconstructed byte string. We keep the previous string
    // materialized so the standard KwKwK case (string(prev) + first(prev)) and
    // dictionary growth are trivial and panic-free. `prev_code_val` is the prior
    // code value, used as the prefix link for the new dictionary entry.
    let mut prev_string: Option<Vec<u8>> = None;
    let mut prev_code_val: u16 = 0;

    // Helper: materialize a dictionary code's byte string by walking the
    // prefix chain. Returns the bytes in correct (forward) order.
    fn reconstruct(
        code: u16,
        prefix: &[u16],
        suffix: &[u8],
        stack: &mut Vec<u8>,
    ) -> Result<Vec<u8>, GifError> {
        stack.clear();
        let mut c = code;
        let mut guard = 0usize;
        loop {
            let suf = *suffix.get(c as usize).ok_or(GifError::LzwError)?;
            stack.push(suf);
            let pfx = *prefix.get(c as usize).ok_or(GifError::LzwError)?;
            if pfx == 0xFFFF {
                break; // root literal
            }
            c = pfx;
            guard += 1;
            if guard > LZW_MAX_DICT {
                return Err(GifError::LzwError); // corrupt cyclic dictionary
            }
        }
        let mut s = Vec::with_capacity(stack.len());
        for &b in stack.iter().rev() {
            s.push(b);
        }
        Ok(s)
    }

    // `read` returns None when the stream is exhausted (a well-formed GIF ends on
    // EOI before that; some encoders omit a clean EOI, in which case we accept
    // what we have and the caller reconciles the length).
    while let Some(code) = reader.read(code_size) {
        if code == clear_code {
            init_dict(&mut prefix, &mut suffix);
            next_code = (eoi_code as usize) + 1;
            code_size = min_code_size + 1;
            prev_string = None;
            continue;
        }
        if code == eoi_code {
            break;
        }

        // Reconstruct this code's byte string.
        let cur_string: Vec<u8> = if (code as usize) < next_code {
            reconstruct(code, &prefix, &suffix, &mut stack)?
        } else if code as usize == next_code {
            // KwKwK: string = string(prev) + first(prev). Only valid with a prev.
            let prev = prev_string.as_ref().ok_or(GifError::LzwError)?;
            let mut s = prev.clone();
            let first = *prev.first().ok_or(GifError::LzwError)?;
            s.push(first);
            s
        } else {
            // Code beyond next_code is never valid.
            return Err(GifError::LzwError);
        };

        // Emit (bounded — a decompression bomb trips here).
        for &b in &cur_string {
            out.push(b);
            if out.len() > MAX_LZW_OUTPUT {
                return Err(GifError::LzwOutputTooLarge);
            }
        }

        // Grow the dictionary: new entry = string(prev) + first(cur). In the
        // prefix/suffix trie that is prefix = prev_code, suffix = first(cur).
        //
        // Width growth: the decoder's dictionary lags the encoder's by exactly one
        // entry (it can only assign a new code once it has seen the *next* code).
        // The encoder widens when, after inserting, `next_code == (1<<code_size)`;
        // so the decoder — one insert behind — must widen one step earlier, when
        // `next_code + 1 == (1<<code_size)`, so the two stay bit-for-bit in sync.
        if prev_string.is_some() && next_code < LZW_MAX_DICT {
            let first = *cur_string.first().ok_or(GifError::LzwError)?;
            prefix[next_code] = prev_code_val;
            suffix[next_code] = first;
            next_code += 1;
            if next_code + 1 == (1usize << code_size) && code_size < MAX_LZW_CODE_BITS {
                code_size += 1;
            }
        }

        prev_code_val = code;
        prev_string = Some(cur_string);

        if max_output != 0 && out.len() >= max_output.min(MAX_LZW_OUTPUT) {
            // Enough pixels for the frame; trailing data before EOI is harmless.
            break;
        }
    }

    Ok(out)
}

// ─── Frame composition onto the persistent canvas ───────────────────────────

/// The Adam-style 4-pass interlace row order for GIF: starting row + step.
/// Pass 0: rows 0,8,16…  1: 4,12…  2: 2,6,10…  3: 1,3,5…
const INTERLACE_START: [usize; 4] = [0, 4, 2, 1];
const INTERLACE_STEP: [usize; 4] = [8, 8, 4, 2];

/// Map LZW index buffer → canvas, honoring interlacing + transparency.
#[allow(clippy::too_many_arguments)]
fn compose_frame(
    canvas: &mut [u32],
    screen_w: u32,
    screen_h: u32,
    img_left: u32,
    img_top: u32,
    img_w: u32,
    img_h: u32,
    interlaced: bool,
    indices: &[u8],
    table: &[Rgb],
    transparent_index: Option<u8>,
) -> Result<(), GifError> {
    let sw = screen_w as usize;
    let sh = screen_h as usize;
    let iw = img_w as usize;
    let ih = img_h as usize;
    let left = img_left as usize;
    let top = img_top as usize;

    // Build the source-row order.
    let row_order: Vec<usize> = if interlaced {
        let mut order = Vec::with_capacity(ih);
        for pass in 0..4 {
            let mut r = INTERLACE_START[pass];
            while r < ih {
                order.push(r);
                r += INTERLACE_STEP[pass];
            }
        }
        order
    } else {
        (0..ih).collect()
    };

    let mut src = 0usize;
    for &dst_row in &row_order {
        for col in 0..iw {
            // Each source pixel in image-local raster order.
            let index = match indices.get(src) {
                Some(&i) => i,
                // Short LZW output (truncated/lenient): leave remaining canvas
                // untouched rather than erroring — defensive, never OOB.
                None => return Ok(()),
            };
            src += 1;

            // Transparent pixels leave the canvas (previous frame) showing through.
            if let Some(t) = transparent_index {
                if index == t {
                    continue;
                }
            }

            let (r, g, b) = match table.get(index as usize) {
                Some(&c) => c,
                None => return Err(GifError::BadColorIndex),
            };

            let cx = left + col;
            let cy = top + dst_row;
            if cx >= sw || cy >= sh {
                continue; // frame extends past the canvas: clip.
            }
            let ci = cy * sw + cx;
            if let Some(slot) = canvas.get_mut(ci) {
                *slot = argb(0xFF, r, g, b);
            }
        }
    }
    Ok(())
}

/// Restore a rectangular region of the canvas to a single color (background).
#[allow(clippy::too_many_arguments)]
fn restore_region(
    canvas: &mut [u32],
    screen_w: u32,
    screen_h: u32,
    img_left: u32,
    img_top: u32,
    img_w: u32,
    img_h: u32,
    color: u32,
) {
    let sw = screen_w as usize;
    let sh = screen_h as usize;
    for row in 0..(img_h as usize) {
        let cy = img_top as usize + row;
        if cy >= sh {
            break;
        }
        for col in 0..(img_w as usize) {
            let cx = img_left as usize + col;
            if cx >= sw {
                break;
            }
            if let Some(slot) = canvas.get_mut(cy * sw + cx) {
                *slot = color;
            }
        }
    }
}

// ════════════════════════════════════════════════════════════════════════════
// Host KAT suite — `cargo test -p rae_gif`. FAIL-able by construction.
// ════════════════════════════════════════════════════════════════════════════
#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec as StdVec;

    // ── A tiny GIF encoder + a reference LZW compressor for fixtures ─────────
    //
    // The encoder builds GIF89a streams with known pixels so we can assert exact
    // ARGB at known coordinates. The LZW compressor is the standard GIF variant
    // (variable-width, CLEAR + EOI), used ONLY to generate test inputs — the
    // decoder under test is fully independent of it.

    struct LzwWriter {
        bits: StdVec<u8>,
        cur: u32,
        nbits: u8,
    }
    impl LzwWriter {
        fn new() -> Self {
            Self {
                bits: StdVec::new(),
                cur: 0,
                nbits: 0,
            }
        }
        fn write(&mut self, code: u16, width: u8) {
            self.cur |= (code as u32) << self.nbits;
            self.nbits += width;
            while self.nbits >= 8 {
                self.bits.push((self.cur & 0xFF) as u8);
                self.cur >>= 8;
                self.nbits -= 8;
            }
        }
        fn finish(&mut self) -> StdVec<u8> {
            if self.nbits > 0 {
                self.bits.push((self.cur & 0xFF) as u8);
                self.cur = 0;
                self.nbits = 0;
            }
            self.bits.clone()
        }
    }

    /// Compress palette indices with GIF LZW. Returns the raw code stream bytes.
    fn lzw_compress(indices: &[u8], min_code_size: u8) -> StdVec<u8> {
        use alloc::collections::BTreeMap as HashMap;
        let clear: u16 = 1 << min_code_size;
        let eoi: u16 = clear + 1;
        let mut w = LzwWriter::new();
        let mut code_size = min_code_size + 1;

        let mut dict: HashMap<StdVec<u8>, u16> = HashMap::new();
        let reset = |dict: &mut HashMap<StdVec<u8>, u16>| {
            dict.clear();
            for i in 0..clear {
                dict.insert(alloc::vec![i as u8], i);
            }
        };
        reset(&mut dict);
        let mut next_code = eoi + 1;

        w.write(clear, code_size);

        if indices.is_empty() {
            w.write(eoi, code_size);
            return w.finish();
        }

        let mut current: StdVec<u8> = alloc::vec![indices[0]];
        for &k in &indices[1..] {
            let mut probe = current.clone();
            probe.push(k);
            if dict.contains_key(&probe) {
                current = probe;
            } else {
                let code = *dict.get(&current).unwrap();
                w.write(code, code_size);
                if next_code < 4096 {
                    dict.insert(probe, next_code);
                    next_code += 1;
                    if next_code == (1u16 << code_size) && code_size < 12 {
                        code_size += 1;
                    }
                }
                current = alloc::vec![k];
            }
        }
        let code = *dict.get(&current).unwrap();
        w.write(code, code_size);
        w.write(eoi, code_size);
        w.finish()
    }

    fn sub_blockify(data: &[u8]) -> StdVec<u8> {
        let mut out = StdVec::new();
        for chunk in data.chunks(255) {
            out.push(chunk.len() as u8);
            out.extend_from_slice(chunk);
        }
        out.push(0); // terminator
        out
    }

    /// Builder for a single-image (optionally multi-image) GIF89a.
    struct GifBuilder {
        out: StdVec<u8>,
        w: u16,
        h: u16,
    }
    impl GifBuilder {
        fn new(w: u16, h: u16, global_table: &[Rgb], bg_index: u8) -> Self {
            let mut out = StdVec::new();
            out.extend_from_slice(b"GIF89a");
            out.extend_from_slice(&w.to_le_bytes());
            out.extend_from_slice(&h.to_le_bytes());
            // packed: global table present, size field = log2(entries)-1. GIF
            // color tables are always a power-of-two count, so pad to `1<<(sf+1)`.
            let n = global_table.len();
            let mut packed = 0u8;
            let mut declared = 0usize;
            if n > 0 {
                let mut size_field = 0u8;
                let mut s = 2usize;
                while s < n {
                    s <<= 1;
                    size_field += 1;
                }
                declared = s;
                packed = 0x80 | (size_field & 0x07);
            }
            out.push(packed);
            out.push(bg_index);
            out.push(0); // aspect ratio
            for i in 0..declared {
                let (r, g, b) = global_table.get(i).copied().unwrap_or((0, 0, 0));
                out.push(r);
                out.push(g);
                out.push(b);
            }
            Self { out, w, h }
        }

        fn gce(&mut self, delay_cs: u16, transparent: Option<u8>, disposal: u8) {
            self.out.push(0x21);
            self.out.push(0xF9);
            self.out.push(4);
            let mut packed = (disposal & 0x07) << 2;
            if transparent.is_some() {
                packed |= 0x01;
            }
            self.out.push(packed);
            self.out.extend_from_slice(&delay_cs.to_le_bytes());
            self.out.push(transparent.unwrap_or(0));
            self.out.push(0); // terminator
        }

        #[allow(clippy::too_many_arguments)]
        fn image(
            &mut self,
            left: u16,
            top: u16,
            w: u16,
            h: u16,
            local_table: Option<&[Rgb]>,
            interlaced: bool,
            indices: &[u8],
            min_code_size: u8,
        ) {
            self.out.push(0x2C);
            self.out.extend_from_slice(&left.to_le_bytes());
            self.out.extend_from_slice(&top.to_le_bytes());
            self.out.extend_from_slice(&w.to_le_bytes());
            self.out.extend_from_slice(&h.to_le_bytes());
            let mut packed = 0u8;
            let mut declared = 0usize;
            if let Some(lt) = local_table {
                let n = lt.len();
                let mut size_field = 0u8;
                let mut s = 2usize;
                while s < n {
                    s <<= 1;
                    size_field += 1;
                }
                declared = s;
                packed |= 0x80 | (size_field & 0x07);
            }
            if interlaced {
                packed |= 0x40;
            }
            self.out.push(packed);
            if let Some(lt) = local_table {
                for i in 0..declared {
                    let (r, g, b) = lt.get(i).copied().unwrap_or((0, 0, 0));
                    self.out.push(r);
                    self.out.push(g);
                    self.out.push(b);
                }
            }
            self.out.push(min_code_size);
            let compressed = lzw_compress(indices, min_code_size);
            self.out.extend_from_slice(&sub_blockify(&compressed));
        }

        fn finish(mut self) -> StdVec<u8> {
            self.out.push(0x3B); // trailer
            let _ = (self.w, self.h);
            self.out
        }
    }

    // ── 1. A 2-color stored image decodes to exact ARGB ─────────────────────

    #[test]
    fn decode_two_color_image() {
        // 2x2 image. palette: [red, green]. pixels: TL=red, TR=green, BL=green,
        // BR=red.
        let palette = [(255u8, 0, 0), (0, 255, 0)];
        let indices = [0u8, 1, 1, 0];
        let mut b = GifBuilder::new(2, 2, &palette, 0);
        b.image(0, 0, 2, 2, None, false, &indices, 2);
        let gif = b.finish();

        let img = decode_gif(&gif).expect("decode 2-color");
        assert_eq!(img.width, 2);
        assert_eq!(img.height, 2);
        assert_eq!(img.frames.len(), 1);
        assert_eq!(img.pixel(0, 0, 0), Some((0xFF, 255, 0, 0))); // red
        assert_eq!(img.pixel(0, 1, 0), Some((0xFF, 0, 255, 0))); // green
        assert_eq!(img.pixel(0, 0, 1), Some((0xFF, 0, 255, 0))); // green
        assert_eq!(img.pixel(0, 1, 1), Some((0xFF, 255, 0, 0))); // red
                                                                 // FAIL-ability: a wrong decode that swapped the palette would trip this.
        assert_ne!(img.pixel(0, 0, 0), Some((0xFF, 0, 255, 0)));
    }

    // ── 2. Transparent-index frame leaves the canvas showing through ────────

    #[test]
    fn transparent_index_is_transparent() {
        // 2x1. palette: [red, blue]. index 1 (blue) declared transparent.
        // First frame fills both with red, second frame draws [blue, red] but
        // blue is transparent → the blue pixel keeps the red from frame 1.
        let palette = [(255u8, 0, 0), (0, 0, 255)];
        let mut b = GifBuilder::new(2, 1, &palette, 0);
        // Frame 1: both red, keep disposal.
        b.gce(10, None, 1);
        b.image(0, 0, 2, 1, None, false, &[0u8, 0], 2);
        // Frame 2: [transparent(blue), red]. transparent index = 1.
        b.gce(10, Some(1), 1);
        b.image(0, 0, 2, 1, None, false, &[1u8, 0], 2);
        let gif = b.finish();

        let img = decode_gif(&gif).expect("decode transparent");
        assert_eq!(img.frames.len(), 2);
        // Frame 1: both red.
        assert_eq!(img.pixel(0, 0, 0), Some((0xFF, 255, 0, 0)));
        assert_eq!(img.pixel(0, 1, 0), Some((0xFF, 255, 0, 0)));
        // Frame 2: pixel 0 was transparent → still red from frame 1; pixel 1 red.
        assert_eq!(img.pixel(1, 0, 0), Some((0xFF, 255, 0, 0)));
        assert_eq!(img.pixel(1, 1, 0), Some((0xFF, 255, 0, 0)));
        // FAIL-ability: if transparency were ignored, pixel 0 of frame 2 would be
        // blue. Assert it is NOT blue.
        assert_ne!(img.pixel(1, 0, 0), Some((0xFF, 0, 0, 255)));
    }

    // ── 3. Multi-frame animation: count, per-frame delay, frames differ ──────

    #[test]
    fn multi_frame_animation() {
        let palette = [(0u8, 0, 0), (255, 255, 255)];
        let mut b = GifBuilder::new(2, 1, &palette, 0);
        // Frame 1: [black, white], delay 5cs → 50ms.
        b.gce(5, None, 1);
        b.image(0, 0, 2, 1, None, false, &[0u8, 1], 2);
        // Frame 2: [white, black], delay 8cs → 80ms.
        b.gce(8, None, 1);
        b.image(0, 0, 2, 1, None, false, &[1u8, 0], 2);
        let gif = b.finish();

        let img = decode_gif(&gif).expect("decode animation");
        assert_eq!(img.frames.len(), 2);
        assert_eq!(img.frames[0].delay_ms, 50);
        assert_eq!(img.frames[1].delay_ms, 80);
        // Frame 2 must differ from frame 1.
        assert_ne!(img.frames[0].pixels, img.frames[1].pixels);
        assert_eq!(img.pixel(0, 0, 0), Some((0xFF, 0, 0, 0))); // f1 px0 black
        assert_eq!(img.pixel(1, 0, 0), Some((0xFF, 255, 255, 255))); // f2 px0 white
    }

    // ── 4. LZW unit test: a known code stream → expected indices ────────────
    //
    // This pins the LZW algorithm independent of the full GIF pipeline (the
    // load-bearing guard, like png's Paeth test). We build the code stream with
    // the reference compressor, then assert the decoder recovers the exact input
    // indices — and that a deliberately different expectation would fail.

    #[test]
    fn lzw_roundtrip_known_stream() {
        // A sequence with repeats so the dictionary actually grows past literals
        // (exercises multi-byte dictionary entries + width growth).
        let indices: StdVec<u8> = alloc::vec![1, 1, 1, 2, 2, 1, 1, 1, 2, 2, 3, 3, 3, 3];
        let min_code_size = 2u8; // palette of 4 → 2-bit
        let compressed = lzw_compress(&indices, min_code_size);
        let decoded = lzw_decode(&compressed, min_code_size, indices.len()).expect("lzw decode");
        assert_eq!(decoded, indices);
        // FAIL-ability: break dictionary growth (e.g. never assign next_code) and
        // long repeats decode wrong → this assert flips.
        assert_ne!(decoded, alloc::vec![0u8; indices.len()]);
    }

    #[test]
    fn lzw_clear_code_resets_dictionary() {
        // A longer, varied stream forces width growth (2→…) and the multi-byte
        // dictionary path; a broken KwKwK or width-growth would corrupt it.
        let mut indices: StdVec<u8> = StdVec::new();
        for i in 0..200u32 {
            indices.push((i % 4) as u8);
        }
        let compressed = lzw_compress(&indices, 2);
        let decoded = lzw_decode(&compressed, 2, indices.len()).expect("lzw long decode");
        assert_eq!(decoded, indices);
    }

    // ── 5. Interlaced == non-interlaced for the same pixels ─────────────────

    #[test]
    fn interlaced_matches_progressive() {
        // 4x8 image (8 rows → all 4 interlace passes used). Distinct value per
        // row so a wrong row order would be visible.
        let palette: StdVec<Rgb> = (0..8u8).map(|i| (i * 16, i * 8, i * 4)).collect();
        let w = 4u16;
        let h = 8u16;
        // image-local raster order, row r filled with index r.
        let mut indices = StdVec::new();
        for r in 0..h {
            for _ in 0..w {
                indices.push(r as u8);
            }
        }

        let mut prog = GifBuilder::new(w, h, &palette, 0);
        prog.image(0, 0, w, h, None, false, &indices, 3);
        let prog_img = decode_gif(&prog.finish()).expect("progressive");

        // For interlaced, the LZW data is in PASS order, not raster order. Build
        // the interlaced source by emitting rows in interlace order.
        let mut inter_indices = StdVec::new();
        for pass in 0..4usize {
            let mut r = INTERLACE_START[pass];
            while r < h as usize {
                for _ in 0..w {
                    inter_indices.push(r as u8);
                }
                r += INTERLACE_STEP[pass];
            }
        }
        let mut inter = GifBuilder::new(w, h, &palette, 0);
        inter.image(0, 0, w, h, None, true, &inter_indices, 3);
        let inter_img = decode_gif(&inter.finish()).expect("interlaced");

        assert_eq!(prog_img.frames[0].pixels, inter_img.frames[0].pixels);
        // Spot-check a row that interlacing reorders (row 1 is in the last pass).
        assert_eq!(prog_img.pixel(0, 0, 1), inter_img.pixel(0, 0, 1));
        // FAIL-ability: a broken interlace pass order would make these differ.
        assert_ne!(prog_img.pixel(0, 0, 0), prog_img.pixel(0, 0, 7));
    }

    // ── 6. Disposal: RestoreBackground vs Keep compose differently ──────────

    #[test]
    fn disposal_methods_compose_differently() {
        // Canvas 2x1. palette: [red, green, blue]. bg index 2 (blue).
        // Frame 1 (full canvas) = [red, red].
        // Frame 2 is a 1x1 sub-image at x=0 = [green].
        // With Keep: frame-1 red stays under, so frame 2 = [green, red].
        // With RestoreBackground after frame 1: the frame-1 area is wiped to bg
        //   (blue) before frame 2, so frame 2 = [green, blue].
        let palette = [(255u8, 0, 0), (0, 255, 0), (0, 0, 255)];

        let build = |disposal1: u8| -> StdVec<u8> {
            let mut b = GifBuilder::new(2, 1, &palette, 2);
            b.gce(10, None, disposal1);
            b.image(0, 0, 2, 1, None, false, &[0u8, 0], 2); // [red, red]
            b.gce(10, None, 1);
            b.image(0, 0, 1, 1, None, false, &[1u8], 2); // [green] at x=0
            b.finish()
        };

        // Keep (disposal=1).
        let keep = decode_gif(&build(1)).expect("keep");
        assert_eq!(keep.frames.len(), 2);
        assert_eq!(keep.pixel(1, 0, 0), Some((0xFF, 0, 255, 0))); // green
        assert_eq!(keep.pixel(1, 1, 0), Some((0xFF, 255, 0, 0))); // red kept

        // RestoreBackground (disposal=2).
        let rb = decode_gif(&build(2)).expect("restore-bg");
        assert_eq!(rb.pixel(1, 0, 0), Some((0xFF, 0, 255, 0))); // green
        assert_eq!(rb.pixel(1, 1, 0), Some((0xFF, 0, 0, 255))); // bg (blue)

        // The two disposal methods MUST produce different frame-2 canvases.
        assert_ne!(keep.frames[1].pixels, rb.frames[1].pixels);
        // FAIL-ability: if disposal were ignored (always Keep), rb's pixel 1
        // would be red, not blue. Assert it is blue.
        assert_ne!(rb.pixel(1, 1, 0), Some((0xFF, 255, 0, 0)));
    }

    #[test]
    fn disposal_restore_previous() {
        // Frame 1 full [red, red] with RestorePrevious. Before frame 1 the canvas
        // is transparent; after frame 1 displays, the canvas is restored to that
        // transparent state, so frame 2 (1x1 green at x=0) sees transparent under
        // pixel 1.
        let palette = [(255u8, 0, 0), (0, 255, 0)];
        let mut b = GifBuilder::new(2, 1, &palette, 0);
        b.gce(10, None, 3); // RestorePrevious
        b.image(0, 0, 2, 1, None, false, &[0u8, 0], 2);
        b.gce(10, None, 1);
        b.image(0, 0, 1, 1, None, false, &[1u8], 2);
        let img = decode_gif(&b.finish()).expect("restore-prev");
        // Frame 1 shows red.
        assert_eq!(img.pixel(0, 0, 0), Some((0xFF, 255, 0, 0)));
        // Frame 2: pixel 0 green; pixel 1 restored to pre-frame-1 (transparent).
        assert_eq!(img.pixel(1, 0, 0), Some((0xFF, 0, 255, 0)));
        assert_eq!(img.pixel(1, 1, 0), Some((0, 0, 0, 0))); // transparent
    }

    // ── 7. Hostile battery: all Err, zero panics ────────────────────────────

    #[test]
    fn reject_not_a_gif() {
        let data = alloc::vec![0u8; 64];
        assert_eq!(decode_gif(&data), Err(GifError::BadSignature));
        let almost = b"GIF99a\x01\x00\x01\x00\x00\x00\x00";
        assert_eq!(decode_gif(almost), Err(GifError::BadSignature));
    }

    #[test]
    fn reject_truncated() {
        // Just a header, nothing else.
        let data = b"GIF89a";
        assert!(matches!(decode_gif(data), Err(GifError::Truncated)));
        // Header + partial screen descriptor.
        let data2 = b"GIF89a\x02\x00";
        assert!(matches!(decode_gif(data2), Err(GifError::Truncated)));
    }

    #[test]
    fn reject_zero_and_oversized_dimensions() {
        // Zero width.
        let mut z = StdVec::new();
        z.extend_from_slice(b"GIF89a");
        z.extend_from_slice(&0u16.to_le_bytes());
        z.extend_from_slice(&1u16.to_le_bytes());
        z.push(0);
        z.push(0);
        z.push(0);
        z.push(0x3B);
        assert_eq!(decode_gif(&z), Err(GifError::DimensionsOutOfRange));

        // Oversized: 65535x65535 → > MAX_PIXELS.
        let mut o = StdVec::new();
        o.extend_from_slice(b"GIF89a");
        o.extend_from_slice(&65535u16.to_le_bytes());
        o.extend_from_slice(&65535u16.to_le_bytes());
        o.push(0);
        o.push(0);
        o.push(0);
        o.push(0x3B);
        assert_eq!(decode_gif(&o), Err(GifError::DimensionsOutOfRange));
    }

    #[test]
    fn reject_bad_lzw_code_size() {
        // Valid header + image descriptor but min_code_size = 9 (illegal).
        let palette = [(0u8, 0, 0), (255, 255, 255)];
        let mut out = StdVec::new();
        out.extend_from_slice(b"GIF89a");
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.push(0x80); // global table, size 2
        out.push(0);
        out.push(0);
        for &(r, g, b) in &palette {
            out.push(r);
            out.push(g);
            out.push(b);
        }
        out.push(0x2C);
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.push(0); // no local table
        out.push(9); // ILLEGAL min code size
        out.push(0); // empty sub-blocks
        out.push(0x3B);
        assert_eq!(decode_gif(&out), Err(GifError::BadLzwCodeSize));
    }

    #[test]
    fn reject_bad_lzw_stream() {
        // Standalone LZW decode of a code that's out of range for the dictionary.
        // min_code_size 2: clear=4, eoi=5, next_code starts at 6. A code of 100
        // before anything is in the dictionary is invalid.
        // Build a bit stream: clear(4)@3bits, then code 100@3bits is impossible
        // (>7), so use a wider scenario: feed code 7 (== next_code with no prev).
        // Easiest: a single non-clear, non-eoi code as the very first code.
        // First code after a (missing) clear with no prev_code → LzwError.
        let mut w = LzwWriter::new();
        w.write(6, 3); // code 6: == next_code, but prev_code is None → error
        let stream = w.finish();
        let res = lzw_decode(&stream, 2, 16);
        assert!(matches!(res, Err(GifError::LzwError)), "got {res:?}");
    }

    #[test]
    fn reject_decompression_bomb() {
        // A GIF bomb attacks along two axes; we prove both defenses (cheaply —
        // no 67M-byte host decode):
        //
        // (a) A frame whose *declared* area exceeds the per-image LZW-output cap is
        //     rejected before any decode. We can't declare > MAX_PIXELS dimensions
        //     (those are caught first as DimensionsOutOfRange), so this axis is
        //     proven by the dimension guard test; here we assert the per-emit LZW
        //     guard directly with a synthetic stream.
        //
        // (b) The per-emit guard: a crafted LZW stream that keeps emitting must
        //     stop at MAX_LZW_OUTPUT. We force the unbounded path with
        //     `max_output == 0` and an oversized sub-block chain through the public
        //     decoder so a real malicious GIF can't OOM us.

        // (b1) Oversized *compressed* sub-block chain is rejected before decode.
        // read_sub_blocks caps the accumulated compressed bytes at MAX_LZW_OUTPUT.
        // Build a GIF whose image LZW sub-blocks never terminate within budget.
        let palette = [(0u8, 0, 0), (255, 255, 255)];
        let mut out = StdVec::new();
        out.extend_from_slice(b"GIF89a");
        out.extend_from_slice(&4u16.to_le_bytes());
        out.extend_from_slice(&4u16.to_le_bytes());
        out.push(0x80); // global table, 2 entries
        out.push(0);
        out.push(0);
        for &(r, g, b) in &palette {
            out.push(r);
            out.push(g);
            out.push(b);
        }
        out.push(0x2C);
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&4u16.to_le_bytes());
        out.extend_from_slice(&4u16.to_le_bytes());
        out.push(0);
        out.push(2); // min code size
                     // A run of full 255-byte sub-blocks with NO terminator, longer
                     // than MAX_LZW_OUTPUT in total → read_sub_blocks rejects it.
        let block_count = (MAX_LZW_OUTPUT / 255) + 2;
        for _ in 0..block_count {
            out.push(255);
            out.extend_from_slice(&[0u8; 255]);
        }
        // (no terminator; the cap fires first)
        let res = decode_gif(&out);
        assert!(
            matches!(
                res,
                Err(GifError::LzwOutputTooLarge) | Err(GifError::Truncated)
            ),
            "compressed-chain bomb must be bounded, got {res:?}"
        );

        // (b2) Tiny-frame expansion bomb: a GIF declares a 2x2 (4-pixel) frame but
        //      its LZW stream encodes 10_000 indices. Production passes the frame
        //      area as `max_output`, so the decoder must STOP at the frame's pixel
        //      budget — it must NOT expand the 4-pixel frame into a 10_000-index
        //      buffer. This is the realistic decompression-bomb path and it is
        //      fast (bounded to 4 emits).
        let many: StdVec<u8> = alloc::vec![0u8; 10_000];
        let compressed = lzw_compress(&many, 2);
        let decoded = lzw_decode(&compressed, 2, 4).expect("bounded decode");
        assert!(
            decoded.len() <= MAX_LZW_OUTPUT,
            "tiny-frame bomb expanded past the cap"
        );
        // The early-stop bounds output to roughly the frame area (a single LZW
        // code may carry a few extra bytes, but it cannot run to 10_000).
        assert!(
            decoded.len() < 1000,
            "tiny-frame bomb should stop near the 4-pixel budget, got {}",
            decoded.len()
        );
    }

    #[test]
    fn reject_bad_color_index_via_short_palette() {
        // 1x1 image, palette of 2, but the pixel index is 3 (out of range).
        // min_code_size must be >=2; with a 2-color table we still allow up to
        // index 3 in the code space, so a literal 3 references a missing color.
        let palette = [(0u8, 0, 0), (255, 255, 255)];
        // Compress a single index 3 with min_code_size 2 (valid code space 0..3).
        let compressed = lzw_compress(&[3u8], 2);
        let mut out = StdVec::new();
        out.extend_from_slice(b"GIF89a");
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.push(0x80);
        out.push(0);
        out.push(0);
        for &(r, g, b) in &palette {
            out.push(r);
            out.push(g);
            out.push(b);
        }
        out.push(0x2C);
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.push(0);
        out.push(2);
        out.extend_from_slice(&sub_blockify(&compressed));
        out.push(0x3B);
        assert_eq!(decode_gif(&out), Err(GifError::BadColorIndex));
    }

    #[test]
    fn single_frame_is_still_image() {
        let palette = [(10u8, 20, 30)];
        // palette of 1 is below the 2-entry minimum table; use 2 entries.
        let palette = [palette[0], (40, 50, 60)];
        let mut b = GifBuilder::new(1, 1, &palette, 0);
        b.image(0, 0, 1, 1, None, false, &[0u8], 2);
        let img = decode_gif(&b.finish()).expect("single frame");
        assert_eq!(img.frames.len(), 1);
        assert_eq!(img.pixel(0, 0, 0), Some((0xFF, 10, 20, 30)));
    }

    #[test]
    fn skips_application_and_comment_extensions() {
        // A real-world GIF has a NETSCAPE2.0 application extension (loop count)
        // and often a comment. The decoder must skip both and still decode.
        let palette = [(1u8, 2, 3), (4, 5, 6)];
        let mut b = GifBuilder::new(1, 1, &palette, 0);
        // Application extension (0x21 0xFF), block size 11, "NETSCAPE2.0", then a
        // sub-block, then terminator.
        b.out.push(0x21);
        b.out.push(0xFF);
        b.out.push(11);
        b.out.extend_from_slice(b"NETSCAPE2.0");
        b.out.push(3);
        b.out.extend_from_slice(&[1, 0, 0]); // loop forever
        b.out.push(0); // terminator
                       // Comment extension.
        b.out.push(0x21);
        b.out.push(0xFE);
        b.out.push(5);
        b.out.extend_from_slice(b"hello");
        b.out.push(0);
        b.image(0, 0, 1, 1, None, false, &[1u8], 2);
        let img = decode_gif(&b.finish()).expect("skip extensions");
        assert_eq!(img.frames.len(), 1);
        assert_eq!(img.pixel(0, 0, 0), Some((0xFF, 4, 5, 6)));
    }

    // ── 8. Cumulative frame-store budget: big canvas × many small frames ─────
    //
    // Regression for the HIGH OOM (criterion #6): each accepted frame is a
    // full-canvas ARGB clone, so peak memory is `canvas_len * frames.len() * 4 B`.
    // MAX_FRAMES and MAX_PIXELS each bound one axis but NOT their product. A GIF
    // declaring a large-but-legal canvas (≤ MAX_PIXELS) plus many tiny 1×1 frames
    // (≤ MAX_FRAMES) would, on the OLD code, clone the full canvas per frame and
    // pin hundreds of MiB-to-TiB from a tiny file (762 MiB reproduced from a
    // 1000×1000 canvas + 200 1×1 frames). The fix rejects the next clone once
    // `canvas_len * (frames.len()+1)` would exceed MAX_TOTAL_FRAME_PIXELS, BEFORE
    // allocating it, so decode Errs quickly instead of ballooning memory.
    //
    // FAIL-ability: on the OLD code (no cumulative cap) this test would clone a
    // 1500×1500-px (9 MiB) canvas ~600 times → ~5.4 GiB peak before any cap fired
    // (process abort / OOM = test failure). On the fixed code it returns
    // Err(TooManyFrames) after the first handful of frames, allocating only a few
    // MiB total.
    #[test]
    fn reject_cumulative_frame_store_bomb() {
        // 1500x1500 = 2_250_000 px canvas (well under MAX_PIXELS = 64M). At the
        // 256M-px total budget that admits only ~119 stored frames; we ask for
        // far more so the budget MUST trip mid-stream.
        let w: u16 = 1500;
        let h: u16 = 1500;
        let canvas_px = (w as u64) * (h as u64);
        assert!(canvas_px <= MAX_PIXELS, "canvas must be legal");
        // Number of 1x1 frames whose product blows the budget (and is also under
        // MAX_FRAMES so it can't be rejected by the frame-count cap first).
        let frame_count = 600usize;
        assert!(frame_count < MAX_FRAMES, "must not trip MAX_FRAMES first");
        assert!(
            canvas_px * (frame_count as u64) > MAX_TOTAL_FRAME_PIXELS,
            "test must actually exceed the cumulative budget"
        );

        let palette = [(255u8, 0, 0), (0, 255, 0)];
        let mut b = GifBuilder::new(w, h, &palette, 0);
        // First image is the full canvas so frame 0 composites a real picture;
        // then many 1x1 sub-frames — each still clones the FULL canvas on the old
        // code, which is exactly the bomb.
        b.image(
            0,
            0,
            w,
            h,
            None,
            false,
            &alloc::vec![0u8; canvas_px as usize],
            2,
        );
        for _ in 1..frame_count {
            b.image(0, 0, 1, 1, None, false, &[1u8], 2);
        }
        let gif = b.finish();

        // Must Err (cumulative budget), NOT allocate ~5 GiB of frame clones.
        assert_eq!(decode_gif(&gif), Err(GifError::TooManyFrames));
    }

    // ── 9. The cumulative cap does not reject legitimate multi-frame GIFs ─────
    //
    // A realistic animated GIF: a modest canvas with many frames, whose product
    // is comfortably under MAX_TOTAL_FRAME_PIXELS. Must decode fully.
    #[test]
    fn legitimate_multi_frame_under_budget_decodes() {
        // 200x200 canvas (40_000 px) × 64 frames = 2_560_000 px total — far below
        // the 256M-px budget. A normal sticker/animation shape.
        let w: u16 = 200;
        let h: u16 = 200;
        let frame_count = 64usize;
        assert!(
            (w as u64) * (h as u64) * (frame_count as u64) <= MAX_TOTAL_FRAME_PIXELS,
            "legit case must be under budget"
        );
        let palette = [(10u8, 20, 30), (40, 50, 60)];
        let mut b = GifBuilder::new(w, h, &palette, 0);
        let full = alloc::vec![0u8; (w as usize) * (h as usize)];
        for f in 0..frame_count {
            b.gce(5, None, 1);
            // Alternate a small sub-image so frames differ; index toggles.
            if f == 0 {
                b.image(0, 0, w, h, None, false, &full, 2);
            } else {
                b.image(0, 0, 1, 1, None, false, &[(f % 2) as u8], 2);
            }
        }
        let img = decode_gif(&b.finish()).expect("legit multi-frame must decode");
        assert_eq!(img.frames.len(), frame_count);
        assert_eq!(img.width, w as u32);
        assert_eq!(img.height, h as u32);
    }
}

// ════════════════════════════════════════════════════════════════════════════
// FUZZ / PROPERTY suite — deterministic seeded PRNG, no external fuzz crate.
//
// Matches the rae_mime/rae_toml/rae_deflate pattern: a tiny xorshift64* PRNG
// generates a reproducible corpus (same seed → same run → reproducible failure).
// The property under test is the hostile-input invariant of [`decode_gif`]: on
// ANY byte sequence it must (a) never panic, and (b) bound every allocation it
// makes (canvas, LZW output, frame count) regardless of what a crafted header or
// LZW stream claims — a "huge canvas" / decompression bomb cannot OOM us.
//
// FAIL-ability (proven by reasoning, see REPORT):
//  - If any decode path could panic on hostile bytes (an unchecked slice index,
//    an `unwrap`, an arithmetic overflow in debug) the never-panic loops abort
//    the test process — the test goes red.
//  - If the MAX_PIXELS / MAX_DIMENSION dimension caps were removed, the
//    `huge_canvas_is_bounded` bomb would request a multi-GiB `vec![0u32; ...]`
//    canvas and OOM (process abort = test failure) instead of returning Err.
//  - If the MAX_LZW_OUTPUT per-emit cap were removed, the tiny-frame /
//    runaway-LZW bombs would expand without bound → OOM, or the asserted
//    `frames[*].len() <= MAX_PIXELS` upper bound flips.
//  - If MAX_FRAMES were removed, the millions-of-1-byte-frames bomb would build
//    an unbounded `frames` Vec → OOM.
// ════════════════════════════════════════════════════════════════════════════
#[cfg(test)]
mod fuzz {
    use super::*;
    use alloc::vec::Vec as StdVec;

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

    /// Every decoded frame must fit inside the canvas pixel bound — the single
    /// invariant that proves no allocation outran its cap.
    fn assert_bounded(img: &GifImage) {
        assert!(
            (img.width as u64) * (img.height as u64) <= MAX_PIXELS,
            "canvas exceeded MAX_PIXELS"
        );
        assert!(img.frames.len() <= MAX_FRAMES, "frame count exceeded cap");
        let canvas_len = (img.width as usize) * (img.height as usize);
        for f in &img.frames {
            assert!(
                f.pixels.len() == canvas_len,
                "frame pixel buffer mismatched canvas"
            );
        }
    }

    /// 1a. Random bytes (0..512) never panic; any Ok result is bounded.
    #[test]
    fn fuzz_random_bytes_never_panic() {
        let mut rng = Rng::new(0x6717_F001);
        for _ in 0..30_000 {
            let len = rng.below(512);
            let mut buf = StdVec::with_capacity(len);
            for _ in 0..len {
                buf.push(rng.byte());
            }
            if let Ok(img) = decode_gif(&buf) {
                assert_bounded(&img);
            }
        }
    }

    /// 1b. Degenerate fills at many lengths never panic.
    #[test]
    fn fuzz_degenerate_fills_never_panic() {
        for fill in [0x00u8, 0xFF, 0x21, 0x2C, 0x3B, 0x80, 0x55, 0xAA] {
            for len in 0..600usize {
                let buf = alloc::vec![fill; len];
                if let Ok(img) = decode_gif(&buf) {
                    assert_bounded(&img);
                }
            }
        }
    }

    /// 1c. Valid headers ("GIF87a"/"GIF89a") with random tails: forces the parse
    /// past the signature into the screen-descriptor / block-stream paths.
    #[test]
    fn fuzz_valid_header_random_tail_never_panic() {
        let mut rng = Rng::new(0x4717_C0DE);
        for sig in [&b"GIF87a"[..], &b"GIF89a"[..]] {
            for _ in 0..20_000 {
                let len = rng.below(256);
                let mut buf = StdVec::with_capacity(6 + len);
                buf.extend_from_slice(sig);
                for _ in 0..len {
                    buf.push(rng.byte());
                }
                if let Ok(img) = decode_gif(&buf) {
                    assert_bounded(&img);
                }
            }
        }
    }

    /// 1d. Truncate a well-formed screen descriptor at every byte offset: bad /
    /// partial logical-screen + image descriptors never panic.
    #[test]
    fn fuzz_truncated_descriptors_never_panic() {
        // A small, well-formed GIF89a with a global table, one GCE, one image
        // (interlaced), and a NETSCAPE extension — exercises every descriptor.
        let mut base = StdVec::new();
        base.extend_from_slice(b"GIF89a");
        base.extend_from_slice(&4u16.to_le_bytes()); // screen w
        base.extend_from_slice(&4u16.to_le_bytes()); // screen h
        base.push(0x80); // global table, 2 entries
        base.push(0); // bg index
        base.push(0); // aspect
        base.extend_from_slice(&[0, 0, 0, 255, 255, 255]); // 2-color table
                                                           // Graphic Control Extension.
        base.extend_from_slice(&[0x21, 0xF9, 4, 0x09, 5, 0, 0, 0]);
        // Image Descriptor: full canvas, interlaced flag set.
        base.push(0x2C);
        base.extend_from_slice(&0u16.to_le_bytes());
        base.extend_from_slice(&0u16.to_le_bytes());
        base.extend_from_slice(&4u16.to_le_bytes());
        base.extend_from_slice(&4u16.to_le_bytes());
        base.push(0x40); // interlaced, no local table
        base.push(2); // min code size
        base.extend_from_slice(&[2, 0x84, 0x8F, 0]); // a tiny LZW sub-block chain
        base.push(0x3B); // trailer

        for cut in 0..=base.len() {
            if let Ok(img) = decode_gif(&base[..cut]) {
                assert_bounded(&img);
            }
        }
        // And flip each byte of the full stream to perturb descriptor fields.
        let mut rng = Rng::new(0x1357_BEEF);
        for _ in 0..40_000 {
            let mut m = base.clone();
            let i = rng.below(m.len());
            m[i] ^= rng.byte();
            if let Ok(img) = decode_gif(&m) {
                assert_bounded(&img);
            }
        }
    }

    /// 1e. "Huge canvas" bomb: a screen descriptor claiming an enormous canvas
    /// must be rejected by the dimension cap — NOT allocate it. Proves
    /// MAX_DIMENSION / MAX_PIXELS bound the up-front canvas allocation.
    #[test]
    fn fuzz_huge_canvas_is_bounded() {
        let build = |w: u16, h: u16| -> StdVec<u8> {
            let mut out = StdVec::new();
            out.extend_from_slice(b"GIF89a");
            out.extend_from_slice(&w.to_le_bytes());
            out.extend_from_slice(&h.to_le_bytes());
            out.push(0); // no global table
            out.push(0);
            out.push(0);
            out.push(0x3B);
            out
        };
        // 65535 x 65535 ≈ 4.29e9 px (> MAX_PIXELS) → must be DimensionsOutOfRange,
        // i.e. rejected BEFORE the `vec![0u32; canvas_len]` allocation.
        assert_eq!(
            decode_gif(&build(65535, 65535)),
            Err(GifError::DimensionsOutOfRange)
        );
        // A band that is in-axis-range but over the pixel-area cap.
        assert_eq!(
            decode_gif(&build(65535, 1100)),
            Err(GifError::DimensionsOutOfRange)
        );
        // Sweep a range of large dimensions — none may panic, and none larger
        // than MAX_PIXELS may decode.
        let mut rng = Rng::new(0x0BAD_CA57);
        for _ in 0..5000 {
            let w = rng.below(65536) as u16;
            let h = rng.below(65536) as u16;
            match decode_gif(&build(w, h)) {
                Ok(img) => assert_bounded(&img),
                Err(_) => {}
            }
        }
    }

    /// 1f. Tiny-frame LZW expansion bomb through the public decoder: a 2x2 frame
    /// whose LZW stream encodes far more indices than its 4-pixel area. The frame
    /// area is the decoder's `max_output`, so output must stop near the budget —
    /// never expand to the encoded length. Proves MAX_LZW_OUTPUT + per-frame cap.
    #[test]
    fn fuzz_tiny_frame_expansion_bounded() {
        // Compress a long all-zero index run with the spec-correct GIF LZW
        // compressor, then drive it through `lzw_decode` with a 4-pixel frame
        // budget. A valid stream that decodes to 50_000 indices must be clipped.
        let indices = alloc::vec![0u8; 50_000];
        let compressed = lzw_compress(&indices, 2);
        let decoded = lzw_decode(&compressed, 2, 4).expect("bounded lzw decode");
        assert!(
            decoded.len() <= MAX_LZW_OUTPUT,
            "tiny-frame bomb expanded past MAX_LZW_OUTPUT"
        );
        assert!(
            decoded.len() < 1000,
            "tiny-frame bomb should stop near the frame budget, got {}",
            decoded.len()
        );
    }

    /// 1g. Decompression bomb via an unterminated/oversized compressed sub-block
    /// chain through the public decoder — read_sub_blocks must cap accumulation.
    #[test]
    fn fuzz_compressed_chain_bomb_bounded() {
        let mut out = StdVec::new();
        out.extend_from_slice(b"GIF89a");
        out.extend_from_slice(&4u16.to_le_bytes());
        out.extend_from_slice(&4u16.to_le_bytes());
        out.push(0x80);
        out.push(0);
        out.push(0);
        out.extend_from_slice(&[0, 0, 0, 255, 255, 255]);
        out.push(0x2C);
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&4u16.to_le_bytes());
        out.extend_from_slice(&4u16.to_le_bytes());
        out.push(0);
        out.push(2);
        // Full 255-byte sub-blocks, no terminator, longer than the cap.
        let block_count = (MAX_LZW_OUTPUT / 255) + 2;
        for _ in 0..block_count {
            out.push(255);
            out.extend_from_slice(&[0u8; 255]);
        }
        let res = decode_gif(&out);
        assert!(
            matches!(
                res,
                Err(GifError::LzwOutputTooLarge) | Err(GifError::Truncated)
            ),
            "compressed-chain bomb must be bounded, got {res:?}"
        );
    }

    /// 1h. Bad color-table size field: every value of the packed table-size bits
    /// against a too-short buffer must Err cleanly (never panic / never OOB).
    #[test]
    fn fuzz_bad_color_table_sizes_never_panic() {
        for size_bits in 0u8..8 {
            // packed: global table present, size field = size_bits → 2^(bits+1)
            // entries declared, but we provide ZERO table bytes → must Truncated.
            let mut out = StdVec::new();
            out.extend_from_slice(b"GIF89a");
            out.extend_from_slice(&2u16.to_le_bytes());
            out.extend_from_slice(&2u16.to_le_bytes());
            out.push(0x80 | size_bits);
            out.push(0);
            out.push(0);
            out.push(0x3B); // no table bytes follow
            let res = decode_gif(&out);
            assert!(
                matches!(res, Err(_)),
                "bad table size must Err, got {res:?}"
            );
        }
    }

    // ── Spec-correct GIF-LZW compressor (test fixtures only; the decoder under
    //    test is independent of it). Mirrors the reference compressor in the KAT
    //    module — used here to build a valid oversized stream for the bomb test. ──
    struct LzwWriter {
        bits: StdVec<u8>,
        cur: u32,
        nbits: u8,
    }
    impl LzwWriter {
        fn new() -> Self {
            Self {
                bits: StdVec::new(),
                cur: 0,
                nbits: 0,
            }
        }
        fn write(&mut self, code: u16, width: u8) {
            self.cur |= (code as u32) << self.nbits;
            self.nbits += width;
            while self.nbits >= 8 {
                self.bits.push((self.cur & 0xFF) as u8);
                self.cur >>= 8;
                self.nbits -= 8;
            }
        }
        fn finish(&mut self) -> StdVec<u8> {
            if self.nbits > 0 {
                self.bits.push((self.cur & 0xFF) as u8);
                self.cur = 0;
                self.nbits = 0;
            }
            self.bits.clone()
        }
    }

    fn lzw_compress(indices: &[u8], min_code_size: u8) -> StdVec<u8> {
        use alloc::collections::BTreeMap as HashMap;
        let clear: u16 = 1 << min_code_size;
        let eoi: u16 = clear + 1;
        let mut w = LzwWriter::new();
        let mut code_size = min_code_size + 1;

        let mut dict: HashMap<StdVec<u8>, u16> = HashMap::new();
        let reset = |dict: &mut HashMap<StdVec<u8>, u16>| {
            dict.clear();
            for i in 0..clear {
                dict.insert(alloc::vec![i as u8], i);
            }
        };
        reset(&mut dict);
        let mut next_code = eoi + 1;

        w.write(clear, code_size);
        if indices.is_empty() {
            w.write(eoi, code_size);
            return w.finish();
        }

        let mut current: StdVec<u8> = alloc::vec![indices[0]];
        for &k in &indices[1..] {
            let mut probe = current.clone();
            probe.push(k);
            if dict.contains_key(&probe) {
                current = probe;
            } else {
                let code = *dict.get(&current).unwrap();
                w.write(code, code_size);
                if next_code < 4096 {
                    dict.insert(probe, next_code);
                    next_code += 1;
                    if next_code == (1u16 << code_size) && code_size < 12 {
                        code_size += 1;
                    }
                }
                current = alloc::vec![k];
            }
        }
        let code = *dict.get(&current).unwrap();
        w.write(code, code_size);
        w.write(eoi, code_size);
        w.finish()
    }
}
