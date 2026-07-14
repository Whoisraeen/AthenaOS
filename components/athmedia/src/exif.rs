//! EXIF orientation parsing + the 8 EXIF transforms — *"show my photos the right
//! way up"* (LEGACY_GAMING_CONCEPT.md §creators/media). Phones and cameras store frames
//! in sensor order and record an Orientation tag (TIFF tag 0x0112) describing how
//! the viewer should rotate/flip them. A viewer that ignores it shows sideways
//! and mirror-flipped photos — the #1 "this OS feels broken" papercut.
//!
//! This module is a *hostile-input* parser (a photo is untrusted data): every
//! path is bounds-checked and **never panics** — a malformed or missing APP1
//! segment yields [`Orientation::Normal`] (identity), which is exactly what a
//! viewer wants for a stripped/odd file. Host-KAT'd (`cargo test -p athmedia`):
//! a synthetic APP1 with orientation=6 decodes to `Rotate90Cw`, a missing APP1
//! decodes to `Normal`, and a 90° transform moves a known corner correctly.

use alloc::vec;

use crate::jpeg::DecodedImage;

/// EXIF/TIFF Orientation (tag 0x0112), values 1..=8 per the TIFF 6.0 spec.
/// `Normal` (1) is identity and the safe fallback for anything we cannot parse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Orientation {
    /// 1: 0th row at top, 0th column at left (no transform).
    Normal,
    /// 2: mirrored horizontally.
    FlipHorizontal,
    /// 3: rotated 180°.
    Rotate180,
    /// 4: mirrored vertically.
    FlipVertical,
    /// 5: mirrored horizontally then rotated 90° CCW (transpose).
    Transpose,
    /// 6: rotated 90° clockwise.
    Rotate90Cw,
    /// 7: mirrored horizontally then rotated 90° CW (transverse).
    Transverse,
    /// 8: rotated 90° counter-clockwise (270° CW).
    Rotate270Cw,
}

impl Orientation {
    /// Map a raw TIFF tag value to an `Orientation`. Anything outside 1..=8
    /// (including the "undefined" 0) falls back to `Normal` — never panics.
    pub fn from_tag(v: u16) -> Self {
        match v {
            1 => Orientation::Normal,
            2 => Orientation::FlipHorizontal,
            3 => Orientation::Rotate180,
            4 => Orientation::FlipVertical,
            5 => Orientation::Transpose,
            6 => Orientation::Rotate90Cw,
            7 => Orientation::Transverse,
            8 => Orientation::Rotate270Cw,
            _ => Orientation::Normal,
        }
    }

    /// The raw TIFF tag value (1..=8) for this orientation.
    pub fn as_tag(self) -> u16 {
        match self {
            Orientation::Normal => 1,
            Orientation::FlipHorizontal => 2,
            Orientation::Rotate180 => 3,
            Orientation::FlipVertical => 4,
            Orientation::Transpose => 5,
            Orientation::Rotate90Cw => 6,
            Orientation::Transverse => 7,
            Orientation::Rotate270Cw => 8,
        }
    }

    /// True when applying this orientation swaps the image's width and height
    /// (the four 90°/270° cases). Callers that pre-size a target use this.
    pub fn swaps_axes(self) -> bool {
        matches!(
            self,
            Orientation::Transpose
                | Orientation::Rotate90Cw
                | Orientation::Transverse
                | Orientation::Rotate270Cw
        )
    }
}

// ─── APP1 / TIFF / IFD0 parser ──────────────────────────────────────────────

/// JPEG markers: a marker is `0xFF` followed by a non-zero, non-`0xFF` byte.
const MARKER_SOI: u8 = 0xD8; // Start Of Image
const MARKER_APP1: u8 = 0xE1; // EXIF lives here
const MARKER_SOS: u8 = 0xDA; // Start Of Scan — entropy data starts; stop scanning
const MARKER_EOI: u8 = 0xD9; // End Of Image

/// The TIFF Orientation tag id.
const TAG_ORIENTATION: u16 = 0x0112;
/// TIFF field type SHORT (16-bit unsigned) — the type Orientation uses.
const TYPE_SHORT: u16 = 3;

/// Parse the EXIF Orientation from a full JPEG byte stream. Walks the marker
/// segments to the first `APP1` carrying the `"Exif\0\0"` identifier, parses the
/// TIFF header + IFD0, and returns the Orientation tag. Returns
/// [`Orientation::Normal`] for any stream that lacks EXIF or is malformed —
/// **never panics, never returns Err** (a viewer always gets a usable value).
pub fn parse_orientation(data: &[u8]) -> Orientation {
    // Must start with SOI (FF D8).
    if data.len() < 2 || data[0] != 0xFF || data[1] != MARKER_SOI {
        return Orientation::Normal;
    }
    let mut i = 2usize;
    // Walk marker segments. Each marker is FF <code> <len:be16> <payload>.
    while i + 4 <= data.len() {
        // Skip any fill 0xFF bytes between segments.
        if data[i] != 0xFF {
            // Not aligned on a marker — give up cleanly.
            return Orientation::Normal;
        }
        let mut code_at = i + 1;
        while code_at < data.len() && data[code_at] == 0xFF {
            code_at += 1;
        }
        if code_at >= data.len() {
            return Orientation::Normal;
        }
        let code = data[code_at];
        // Standalone markers (no length/payload): SOI, EOI, RSTn, TEM.
        if code == MARKER_SOI || code == MARKER_EOI {
            i = code_at + 1;
            continue;
        }
        // SOS begins entropy-coded data; EXIF (if any) is already behind us.
        if code == MARKER_SOS {
            break;
        }
        // Length covers the 2 length bytes themselves but not the marker.
        let len_at = code_at + 1;
        if len_at + 2 > data.len() {
            return Orientation::Normal;
        }
        let seg_len = ((data[len_at] as usize) << 8) | (data[len_at + 1] as usize);
        if seg_len < 2 {
            return Orientation::Normal;
        }
        let payload_start = len_at + 2;
        let payload_end = len_at + seg_len; // len_at + seg_len == payload_start + (seg_len-2)
        if payload_end > data.len() {
            return Orientation::Normal;
        }
        if code == MARKER_APP1 {
            let payload = &data[payload_start..payload_end];
            if let Some(o) = parse_app1_exif(payload) {
                return o;
            }
            // APP1 without parseable EXIF (e.g. XMP) — keep scanning; a later
            // APP1 could be the real EXIF block.
        }
        i = payload_end;
    }
    Orientation::Normal
}

/// Parse one APP1 payload. Returns `Some(orientation)` only if it carries a valid
/// `"Exif\0\0"` header with a TIFF IFD0 containing the Orientation tag; `None`
/// otherwise (so the caller keeps scanning). Never panics.
fn parse_app1_exif(payload: &[u8]) -> Option<Orientation> {
    // EXIF identifier: "Exif" 00 00.
    if payload.len() < 6 || &payload[0..4] != b"Exif" || payload[4] != 0 || payload[5] != 0 {
        return None;
    }
    let tiff = &payload[6..];
    parse_tiff_orientation(tiff)
}

/// Parse a TIFF block (byte order + IFD0) and return the Orientation tag if
/// present. Bounds-checked throughout; returns `None` on any malformation.
fn parse_tiff_orientation(tiff: &[u8]) -> Option<Orientation> {
    if tiff.len() < 8 {
        return None;
    }
    // Byte-order mark: "II" little-endian, "MM" big-endian.
    let le = match &tiff[0..2] {
        b"II" => true,
        b"MM" => false,
        _ => return None,
    };
    // 42 magic.
    let magic = read_u16(tiff, 2, le)?;
    if magic != 42 {
        return None;
    }
    // Offset to IFD0 (relative to the TIFF header start).
    let ifd0_off = read_u32(tiff, 4, le)? as usize;
    if ifd0_off + 2 > tiff.len() {
        return None;
    }
    let entry_count = read_u16(tiff, ifd0_off, le)? as usize;
    // Each IFD entry is 12 bytes; the count follows at ifd0_off+2.
    let entries_start = ifd0_off + 2;
    // Cap the entry walk to what actually fits (defends against a bogus count).
    let max_entries = tiff.len().saturating_sub(entries_start) / 12;
    let entry_count = entry_count.min(max_entries);

    for e in 0..entry_count {
        let base = entries_start + e * 12;
        let tag = read_u16(tiff, base, le)?;
        if tag != TAG_ORIENTATION {
            continue;
        }
        let field_type = read_u16(tiff, base + 2, le)?;
        // Orientation is a single SHORT. If a file mis-types it, still try to
        // read a SHORT from the value field (the low 2 bytes), but only trust
        // it when the type is plausible.
        if field_type != TYPE_SHORT {
            return None;
        }
        // count at base+4 (u32) is 1 for Orientation; the value is inline in the
        // 4-byte value field at base+8 (a SHORT uses the first 2 of those bytes).
        let val = read_u16(tiff, base + 8, le)?;
        return Some(Orientation::from_tag(val));
    }
    None
}

#[inline]
fn read_u16(buf: &[u8], off: usize, le: bool) -> Option<u16> {
    let b = buf.get(off..off + 2)?;
    Some(if le {
        u16::from_le_bytes([b[0], b[1]])
    } else {
        u16::from_be_bytes([b[0], b[1]])
    })
}

#[inline]
fn read_u32(buf: &[u8], off: usize, le: bool) -> Option<u32> {
    let b = buf.get(off..off + 4)?;
    Some(if le {
        u32::from_le_bytes([b[0], b[1], b[2], b[3]])
    } else {
        u32::from_be_bytes([b[0], b[1], b[2], b[3]])
    })
}

// ─── The 8 EXIF transforms on the ARGB buffer ───────────────────────────────

/// Apply an EXIF orientation to a decoded ARGB8888 image, returning a new
/// correctly-oriented image. `Normal` clones the input unchanged. The four
/// 90°/270° cases swap width/height. Pure index arithmetic — no allocation
/// beyond the single output buffer, never panics (every destination index is
/// in range by construction).
///
/// Mapping convention: for each *source* pixel `(sx, sy)` we compute its
/// *destination* `(dx, dy)` in the output image and copy it there, so the whole
/// source is covered exactly once.
pub fn apply_orientation(img: &DecodedImage, orientation: Orientation) -> DecodedImage {
    let w = img.width as usize;
    let h = img.height as usize;
    // Defensive: a DecodedImage whose pixel count disagrees with its dims is
    // returned as-is (we never index past the buffer).
    if w == 0 || h == 0 || img.pixels.len() != w * h {
        return img.clone();
    }

    let (ow, oh) = if orientation.swaps_axes() {
        (h, w)
    } else {
        (w, h)
    };
    let mut out = vec![0u32; ow * oh];

    for sy in 0..h {
        for sx in 0..w {
            let p = img.pixels[sy * w + sx];
            // Destination coordinate for this orientation.
            let (dx, dy) = match orientation {
                Orientation::Normal => (sx, sy),
                Orientation::FlipHorizontal => (w - 1 - sx, sy),
                Orientation::Rotate180 => (w - 1 - sx, h - 1 - sy),
                Orientation::FlipVertical => (sx, h - 1 - sy),
                // Axis-swapping cases: output dims are (ow=h, oh=w).
                Orientation::Transpose => (sy, sx),
                Orientation::Rotate90Cw => (h - 1 - sy, sx),
                Orientation::Transverse => (h - 1 - sy, w - 1 - sx),
                Orientation::Rotate270Cw => (sy, w - 1 - sx),
            };
            out[dy * ow + dx] = p;
        }
    }

    DecodedImage {
        width: ow as u32,
        height: oh as u32,
        pixels: out,
    }
}

/// Decode a JPEG and auto-apply its EXIF orientation in one step — the
/// "just show it right" path a Photos viewer wants. The raw [`crate::jpeg::decode_jpeg`]
/// is left untouched for callers that need sensor-order pixels. On a successful
/// decode we always return an oriented image; a missing/garbage EXIF block is a
/// `Normal` (identity) transform, so this never fails *because of* EXIF.
pub fn decode_jpeg_oriented(data: &[u8]) -> Result<DecodedImage, crate::jpeg::JpegError> {
    let img = crate::jpeg::decode_jpeg(data)?;
    let orientation = parse_orientation(data);
    if orientation == Orientation::Normal {
        Ok(img)
    } else {
        Ok(apply_orientation(&img, orientation))
    }
}

// ─── Host KATs (cargo test -p athmedia) ─────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use alloc::vec::Vec as StdVec;

    /// Build a minimal JPEG byte stream: SOI, one APP1 EXIF segment carrying a
    /// TIFF IFD0 with a single Orientation entry, then EOI. Little-endian TIFF.
    fn jpeg_with_orientation(value: u16) -> StdVec<u8> {
        // TIFF block: header (8 bytes) + IFD0 (count + 1 entry + next-ifd ptr).
        let mut tiff: StdVec<u8> = StdVec::new();
        tiff.extend_from_slice(b"II"); // little-endian
        tiff.extend_from_slice(&42u16.to_le_bytes()); // magic
        tiff.extend_from_slice(&8u32.to_le_bytes()); // IFD0 at offset 8
        tiff.extend_from_slice(&1u16.to_le_bytes()); // 1 entry
                                                     // Entry: tag, type, count, value(4)
        tiff.extend_from_slice(&TAG_ORIENTATION.to_le_bytes());
        tiff.extend_from_slice(&TYPE_SHORT.to_le_bytes());
        tiff.extend_from_slice(&1u32.to_le_bytes());
        tiff.extend_from_slice(&value.to_le_bytes());
        tiff.extend_from_slice(&0u16.to_le_bytes()); // pad to fill the 4-byte value field
        tiff.extend_from_slice(&0u32.to_le_bytes()); // next IFD = 0

        // APP1 payload: "Exif\0\0" + TIFF block.
        let mut payload: StdVec<u8> = StdVec::new();
        payload.extend_from_slice(b"Exif\0\0");
        payload.extend_from_slice(&tiff);

        // APP1 segment length covers the 2 length bytes + payload.
        let seg_len = (payload.len() + 2) as u16;

        let mut out: StdVec<u8> = StdVec::new();
        out.extend_from_slice(&[0xFF, MARKER_SOI]);
        out.extend_from_slice(&[0xFF, MARKER_APP1]);
        out.extend_from_slice(&seg_len.to_be_bytes());
        out.extend_from_slice(&payload);
        out.extend_from_slice(&[0xFF, MARKER_EOI]);
        out
    }

    #[test]
    fn parses_orientation_6_rotate90cw() {
        let jpeg = jpeg_with_orientation(6);
        assert_eq!(parse_orientation(&jpeg), Orientation::Rotate90Cw);
    }

    #[test]
    fn parses_all_eight_values() {
        let expect = [
            (1u16, Orientation::Normal),
            (2, Orientation::FlipHorizontal),
            (3, Orientation::Rotate180),
            (4, Orientation::FlipVertical),
            (5, Orientation::Transpose),
            (6, Orientation::Rotate90Cw),
            (7, Orientation::Transverse),
            (8, Orientation::Rotate270Cw),
        ];
        for (v, o) in expect {
            assert_eq!(parse_orientation(&jpeg_with_orientation(v)), o, "value {v}");
        }
    }

    #[test]
    fn missing_app1_is_normal() {
        // SOI + EOI only — no EXIF at all.
        let jpeg = [0xFFu8, MARKER_SOI, 0xFF, MARKER_EOI];
        assert_eq!(parse_orientation(&jpeg), Orientation::Normal);
    }

    #[test]
    fn out_of_range_tag_is_normal() {
        // Tag value 99 is not 1..=8.
        assert_eq!(
            parse_orientation(&jpeg_with_orientation(99)),
            Orientation::Normal
        );
    }

    #[test]
    fn malformed_never_panics() {
        // A pile of degenerate inputs must all yield Normal, never panic.
        assert_eq!(parse_orientation(&[]), Orientation::Normal);
        assert_eq!(parse_orientation(&[0xFF]), Orientation::Normal);
        assert_eq!(parse_orientation(&[0x00, 0x00]), Orientation::Normal);
        assert_eq!(parse_orientation(&[0xFF, MARKER_SOI]), Orientation::Normal);
        // SOI then a truncated APP1 (claims a length past the buffer).
        let trunc = [0xFFu8, MARKER_SOI, 0xFF, MARKER_APP1, 0xFF, 0xFF];
        assert_eq!(parse_orientation(&trunc), Orientation::Normal);
        // SOI then APP1 with "Exif\0\0" but a TIFF that points its IFD0 off the end.
        let mut bad = StdVec::new();
        bad.extend_from_slice(&[0xFFu8, MARKER_SOI, 0xFF, MARKER_APP1]);
        let payload: &[u8] = b"Exif\0\0II\x2a\x00\xFF\xFF\xFF\xFF"; // IFD0 offset = huge
        let seg_len = (payload.len() + 2) as u16;
        bad.extend_from_slice(&seg_len.to_be_bytes());
        bad.extend_from_slice(payload);
        assert_eq!(parse_orientation(&bad), Orientation::Normal);
    }

    #[test]
    fn big_endian_tiff_parses() {
        // Build a big-endian ("MM") TIFF carrying orientation=8.
        let mut tiff: StdVec<u8> = StdVec::new();
        tiff.extend_from_slice(b"MM");
        tiff.extend_from_slice(&42u16.to_be_bytes());
        tiff.extend_from_slice(&8u32.to_be_bytes());
        tiff.extend_from_slice(&1u16.to_be_bytes());
        tiff.extend_from_slice(&TAG_ORIENTATION.to_be_bytes());
        tiff.extend_from_slice(&TYPE_SHORT.to_be_bytes());
        tiff.extend_from_slice(&1u32.to_be_bytes());
        tiff.extend_from_slice(&8u16.to_be_bytes());
        tiff.extend_from_slice(&0u16.to_be_bytes());
        tiff.extend_from_slice(&0u32.to_be_bytes());

        let mut payload: StdVec<u8> = StdVec::new();
        payload.extend_from_slice(b"Exif\0\0");
        payload.extend_from_slice(&tiff);
        let seg_len = (payload.len() + 2) as u16;
        let mut out: StdVec<u8> = StdVec::new();
        out.extend_from_slice(&[0xFF, MARKER_SOI]);
        out.extend_from_slice(&[0xFF, MARKER_APP1]);
        out.extend_from_slice(&seg_len.to_be_bytes());
        out.extend_from_slice(&payload);
        out.extend_from_slice(&[0xFF, MARKER_EOI]);
        assert_eq!(parse_orientation(&out), Orientation::Rotate270Cw);
    }

    // ── Transform exact-pixel KATs ──────────────────────────────────────────

    /// A 3x2 image (width 3, height 2) with a unique value per pixel so any
    /// index error is visible:
    ///   row0: 0 1 2
    ///   row1: 3 4 5
    fn sample_3x2() -> DecodedImage {
        DecodedImage {
            width: 3,
            height: 2,
            pixels: vec![0, 1, 2, 3, 4, 5],
        }
    }

    #[test]
    fn normal_is_identity() {
        let img = sample_3x2();
        let out = apply_orientation(&img, Orientation::Normal);
        assert_eq!(out, img);
    }

    #[test]
    fn rotate90cw_moves_top_left_to_top_right() {
        // 3x2 rotated 90° CW becomes 2x3. The source top-left (0,0)=value 0
        // must land at the destination top-right corner.
        let img = sample_3x2();
        let out = apply_orientation(&img, Orientation::Rotate90Cw);
        assert_eq!(out.width, 2);
        assert_eq!(out.height, 3);
        // Source (sx=0, sy=0) → dest (h-1-sy=1, sx=0) = (1,0): top-right.
        assert_eq!(out.pixel(1, 0).map(|p| p.3), Some(0));
        // Full expected layout (2 wide, 3 tall):
        //   (3,0) (0,?)... compute directly:
        //   dst[dy*ow+dx]; ow=2.
        // Expected pixels row-major:
        //   row0: src(0,1)=3, src(0,0)=0
        //   row1: src(1,1)=4, src(1,0)=1
        //   row2: src(2,1)=5, src(2,0)=2
        assert_eq!(out.pixels, vec![3, 0, 4, 1, 5, 2]);
    }

    #[test]
    fn flip_horizontal_mirrors_columns() {
        let img = sample_3x2();
        let out = apply_orientation(&img, Orientation::FlipHorizontal);
        assert_eq!(out.width, 3);
        assert_eq!(out.height, 2);
        // row0 reversed: 2 1 0 ; row1 reversed: 5 4 3
        assert_eq!(out.pixels, vec![2, 1, 0, 5, 4, 3]);
    }

    #[test]
    fn rotate180_reverses_everything() {
        let img = sample_3x2();
        let out = apply_orientation(&img, Orientation::Rotate180);
        assert_eq!(out.pixels, vec![5, 4, 3, 2, 1, 0]);
    }

    #[test]
    fn rotate270cw_layout() {
        let img = sample_3x2();
        let out = apply_orientation(&img, Orientation::Rotate270Cw);
        assert_eq!(out.width, 2);
        assert_eq!(out.height, 3);
        // dst (dx=sy, dy=w-1-sx), ow=2:
        //   row0: src(2,0)=2, src(2,1)=5
        //   row1: src(1,0)=1, src(1,1)=4
        //   row2: src(0,0)=0, src(0,1)=3
        assert_eq!(out.pixels, vec![2, 5, 1, 4, 0, 3]);
    }

    #[test]
    fn malformed_image_returned_as_is() {
        // pixel count disagrees with dims → returned unchanged, no panic.
        let img = DecodedImage {
            width: 4,
            height: 4,
            pixels: vec![7, 7, 7], // wrong length on purpose
        };
        let out = apply_orientation(&img, Orientation::Rotate90Cw);
        assert_eq!(out, img);
    }

    // ── Deterministic seeded-PRNG fuzz (cargo test -p athmedia) ─────────────────
    //
    // The EXIF/TIFF path is an UNTRUSTED-OFFSET surface: the IFD0 offset and each
    // entry's value-offset are attacker-controlled u32s read out of the file. The
    // classic bug class is reading at one of those offsets without bounds-checking
    // (OOB read → panic) or following a self-referential / circular IFD offset
    // without a forward-progress guard (infinite loop). `parse_orientation` and
    // `parse_tiff_orientation` promise to NEVER panic and always terminate,
    // returning `Orientation::Normal` on anything malformed.
    //
    // FAIL-ABILITY: an unchecked IFD or value offset would slice `tiff[off..off+N]`
    // and panic (index out of range), surfacing as a `cargo test` failure inside
    // these `fuzz_*` bodies. A self-referential IFD offset followed without a
    // visited/decreasing guard would hang the test (harness timeout). Both are
    // observable failures — these tests can go red.

    /// Self-contained deterministic PRNG (xorshift64*). No external fuzz crate,
    /// no `Cargo.toml` change — matches the png / jpeg fuzz pattern in this crate.
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

    /// Wrap an arbitrary TIFF body in a valid SOI + APP1("Exif\0\0") + EOI JPEG so
    /// the fuzz drives all the way into `parse_tiff_orientation`. (Random bytes
    /// alone almost always bail at the SOI / marker / "Exif" gates, leaving the
    /// offset-following logic untested.)
    fn jpeg_wrapping_tiff(tiff: &[u8]) -> StdVec<u8> {
        let mut payload: StdVec<u8> = StdVec::new();
        payload.extend_from_slice(b"Exif\0\0");
        payload.extend_from_slice(tiff);
        let seg_len = (payload.len() + 2) as u16;
        let mut out: StdVec<u8> = StdVec::new();
        out.extend_from_slice(&[0xFF, MARKER_SOI]);
        out.extend_from_slice(&[0xFF, MARKER_APP1]);
        out.extend_from_slice(&seg_len.to_be_bytes());
        out.extend_from_slice(&payload);
        out.extend_from_slice(&[0xFF, MARKER_EOI]);
        out
    }

    /// Pure random bytes (0..1024) must never panic the orientation parser.
    #[test]
    fn fuzz_random_bytes_never_panic() {
        let mut rng = XorShift::new(0xE71F_0001_1234_5678);
        for _ in 0..30_000 {
            let len = rng.range(1025);
            let mut buf: StdVec<u8> = StdVec::with_capacity(len);
            for _ in 0..len {
                buf.push(rng.next_u8());
            }
            // Always falls back to Normal; the point is it must not panic.
            let _ = parse_orientation(&buf);
        }
    }

    /// Random bytes that always start with the valid SOI marker drive the JPEG
    /// segment walker deeper (random alone usually bails at the SOI gate).
    #[test]
    fn fuzz_valid_soi_random_tail_never_panic() {
        let mut rng = XorShift::new(0xE71F_0002_FACE_F00D);
        for _ in 0..30_000 {
            let len = rng.range(512);
            let mut buf: StdVec<u8> = StdVec::with_capacity(len + 2);
            buf.extend_from_slice(&[0xFF, MARKER_SOI]);
            for _ in 0..len {
                buf.push(rng.next_u8());
            }
            let _ = parse_orientation(&buf);
        }
    }

    /// Random *TIFF bodies* wrapped in a valid Exif APP1 — exercises the byte-order
    /// mark, magic, IFD0 offset and entry walk with hostile content. Covers both
    /// "II" and "MM" by forcing the leading mark half the time.
    #[test]
    fn fuzz_random_tiff_body_never_panic() {
        let mut rng = XorShift::new(0xE71F_0003_0BAD_BEEF);
        for _ in 0..30_000 {
            let len = rng.range(256);
            let mut tiff: StdVec<u8> = StdVec::with_capacity(len + 2);
            // Force a recognized byte-order mark some of the time so the parser
            // proceeds past the BOM check into offset following.
            match rng.range(3) {
                0 => tiff.extend_from_slice(b"II"),
                1 => tiff.extend_from_slice(b"MM"),
                _ => {
                    tiff.push(rng.next_u8());
                    tiff.push(rng.next_u8());
                }
            }
            for _ in 0..len {
                tiff.push(rng.next_u8());
            }
            let jpeg = jpeg_wrapping_tiff(&tiff);
            let _ = parse_orientation(&jpeg);
        }
    }

    /// A TIFF whose IFD0 offset points PAST the end of the buffer must be rejected
    /// (→ Normal), never read OOB. FAIL-able: an unchecked `tiff[ifd0_off..]` read
    /// would panic here. Tests a wide range of past-end offsets, both endians.
    #[test]
    fn fuzz_ifd_offset_past_end_is_bounded() {
        for le in [true, false] {
            for &off in &[
                8u32,
                9,
                16,
                100,
                1000,
                0x7FFF_FFFF,
                0xFFFF_FFF0,
                0xFFFF_FFFF,
            ] {
                let mut tiff: StdVec<u8> = StdVec::new();
                tiff.extend_from_slice(if le { b"II" } else { b"MM" });
                if le {
                    tiff.extend_from_slice(&42u16.to_le_bytes());
                    tiff.extend_from_slice(&off.to_le_bytes());
                } else {
                    tiff.extend_from_slice(&42u16.to_be_bytes());
                    tiff.extend_from_slice(&off.to_be_bytes());
                }
                // Header is 8 bytes; offset points beyond it / beyond the buffer.
                let jpeg = jpeg_wrapping_tiff(&tiff);
                assert_eq!(
                    parse_orientation(&jpeg),
                    Orientation::Normal,
                    "le={le} off={off:#x} must fall back to Normal, not OOB"
                );
            }
        }
    }

    /// A TIFF declaring a huge IFD entry count (up to u16::MAX) must walk only the
    /// entries that actually fit (the `max_entries` cap), never index past the
    /// buffer. FAIL-able: trusting the declared count would slice the 12-byte entry
    /// at `entries_start + e*12` past the end and panic.
    #[test]
    fn fuzz_bogus_entry_count_is_bounded() {
        let mut rng = XorShift::new(0xE71F_0004_DEAD_C0DE);
        for _ in 0..5_000 {
            let le = rng.next_u64() & 1 == 0;
            // A handful of partial entry bytes after the count — far fewer than
            // the declared count claims.
            let trailing = rng.range(40);
            let claimed = rng.next_u64() as u16; // up to 65535 entries claimed
            let mut tiff: StdVec<u8> = StdVec::new();
            tiff.extend_from_slice(if le { b"II" } else { b"MM" });
            if le {
                tiff.extend_from_slice(&42u16.to_le_bytes());
                tiff.extend_from_slice(&8u32.to_le_bytes()); // IFD0 right after header
                tiff.extend_from_slice(&claimed.to_le_bytes());
            } else {
                tiff.extend_from_slice(&42u16.to_be_bytes());
                tiff.extend_from_slice(&8u32.to_be_bytes());
                tiff.extend_from_slice(&claimed.to_be_bytes());
            }
            for _ in 0..trailing {
                tiff.push(rng.next_u8());
            }
            let jpeg = jpeg_wrapping_tiff(&tiff);
            // Must terminate and not panic regardless of the inflated count.
            let _ = parse_orientation(&jpeg);
        }
    }

    /// An Orientation entry with a bogus field type / count must be handled, not
    /// trusted into an OOB value read. FAIL-able: reading the inline value at
    /// `base+8` without the entry being in-bounds would panic.
    #[test]
    fn fuzz_bad_tag_type_and_count_never_panic() {
        let mut rng = XorShift::new(0xE71F_0005_5EED_1111);
        for _ in 0..10_000 {
            let le = rng.next_u64() & 1 == 0;
            let field_type = rng.next_u64() as u16; // any type, mostly != SHORT
            let count = rng.next_u64() as u32; // huge counts allowed
            let value = rng.next_u64() as u32; // arbitrary value-offset
            let mut tiff: StdVec<u8> = StdVec::new();
            let push16 = |t: &mut StdVec<u8>, v: u16| {
                if le {
                    t.extend_from_slice(&v.to_le_bytes())
                } else {
                    t.extend_from_slice(&v.to_be_bytes())
                }
            };
            let push32 = |t: &mut StdVec<u8>, v: u32| {
                if le {
                    t.extend_from_slice(&v.to_le_bytes())
                } else {
                    t.extend_from_slice(&v.to_be_bytes())
                }
            };
            tiff.extend_from_slice(if le { b"II" } else { b"MM" });
            push16(&mut tiff, 42);
            push32(&mut tiff, 8); // IFD0 at offset 8
            push16(&mut tiff, 1); // one entry
            push16(&mut tiff, TAG_ORIENTATION);
            push16(&mut tiff, field_type);
            push32(&mut tiff, count);
            push32(&mut tiff, value);
            // Truncate the value field sometimes to stress partial reads.
            if rng.next_u64() & 1 == 0 && tiff.len() > 2 {
                let cut = tiff.len() - rng.range(3);
                tiff.truncate(cut);
            }
            let jpeg = jpeg_wrapping_tiff(&tiff);
            let _ = parse_orientation(&jpeg);
        }
    }

    /// A self-referential / circular IFD offset must not hang the parser. This
    /// crate only follows IFD0 (it ignores the next-IFD pointer), so following is
    /// inherently bounded; this test pins that property: a TIFF whose IFD0 offset
    /// points back into the header (offset 0, self-overlap) must terminate and
    /// return Normal, never loop. FAIL-able: a parser that chased next-IFD pointers
    /// without a visited-set would spin forever and time the harness out.
    #[test]
    fn fuzz_self_referential_ifd_offset_terminates() {
        for le in [true, false] {
            // IFD0 offset = 0 → points at the byte-order mark (self-overlap).
            let mut tiff: StdVec<u8> = StdVec::new();
            tiff.extend_from_slice(if le { b"II" } else { b"MM" });
            if le {
                tiff.extend_from_slice(&42u16.to_le_bytes());
                tiff.extend_from_slice(&0u32.to_le_bytes()); // IFD0 at offset 0
            } else {
                tiff.extend_from_slice(&42u16.to_be_bytes());
                tiff.extend_from_slice(&0u32.to_be_bytes());
            }
            // Some filler so an entry walk at offset 0 has bytes to chew on.
            tiff.extend_from_slice(&[0u8; 64]);
            let jpeg = jpeg_wrapping_tiff(&tiff);
            // It must return (terminate); the value is Normal by construction.
            let _ = parse_orientation(&jpeg);
        }
    }

    /// Mutation fuzz over a known-good orientation JPEG: flip bytes (corrupting the
    /// segment length, "Exif" id, TIFF BOM/magic, IFD offset, entry fields) and
    /// truncate. Must never panic. The base fixture decodes cleanly so mutations
    /// exercise the failure paths.
    #[test]
    fn fuzz_mutated_valid_exif_never_panic() {
        let base = jpeg_with_orientation(6);
        assert_eq!(
            parse_orientation(&base),
            Orientation::Rotate90Cw,
            "fixture must parse clean before mutation"
        );
        let mut rng = XorShift::new(0xE71F_0006_C0DE_D00D);
        for _ in 0..40_000 {
            let mut buf = base.clone();
            let nmut = 1 + rng.range(6);
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
            let _ = parse_orientation(&buf);
        }
    }

    /// Every truncation prefix of a valid orientation JPEG must parse-or-fallback,
    /// never panic — covers truncated SOI, APP1 header, "Exif" id, TIFF header,
    /// IFD count, mid-entry, and the value field.
    #[test]
    fn fuzz_all_truncations_never_panic() {
        let base = jpeg_with_orientation(6);
        for cut in 0..=base.len() {
            let _ = parse_orientation(&base[..cut]);
        }
    }
}
