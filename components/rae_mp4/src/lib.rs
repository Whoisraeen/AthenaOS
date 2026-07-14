//! # RaeMP4 — a never-panic, `no_std` MP4 / ISO-BMFF container demuxer.
//!
//! RaeenOS_Concept.md (§creators / media): a daily driver must "play my movies"
//! and "play my music." MP4 (ISO/IEC 14496-12, the ISO Base Media File Format)
//! is the dominant container for both — phone video, downloaded video, and AAC
//! audio (`.m4a`/`.mp4`) all ship as BMFF. This crate is the from-scratch
//! **demuxer** the `raeplay` video player and the `apps/music` `.m4a` path sit on.
//!
//! ## This is a DEMUXER, not a decoder
//! It parses the box tree → tracks → sample tables, and resolves, for every
//! sample, its **absolute file offset, byte size, decode/composition timestamps,
//! and keyframe flag**. It then hands a caller the raw elementary-stream bytes for
//! any sample ([`Track::sample_data`]). It does **not** decode AAC/H.264/HEVC —
//! it surfaces the codec fourcc plus the raw codec-private config bytes
//! (`esds`/AudioSpecificConfig for AAC, `avcC` for H.264, `hvcC` for HEVC) so a
//! future decoder gets exactly what it needs. The output shape — opaque
//! elementary-stream `&[u8]` slices keyed by track + sample index — is what a
//! `raemedia`-style decoder consumes to produce `AudioFrame`s / video frames.
//!
//! ## What is modeled
//! - The box header: 32-bit size + 4-byte type, the 64-bit `largesize` form, the
//!   `size == 0` (extends-to-EOF) form, and `uuid` extended-type boxes.
//! - `ftyp` (major brand + compatible brands).
//! - `moov` → `mvhd` (movie timescale/duration), `trak`* → `tkhd` (track id +
//!   video dimensions), `mdia` → `mdhd` (media timescale/duration/language) +
//!   `hdlr` (handler → [`TrackKind`]), `minf` → `stbl` (the sample table):
//!   `stsd` (codec fourcc + audio params / video dims + codec-private config),
//!   `stts`, `ctts`, `stsc`, `stsz`/`stz2`, `stco`/`co64`, `stss`.
//! - Both versions (0/1) of the full-box time/duration fields.
//!
//! ## What is deferred (honest)
//! - **Fragmented MP4** (`moof`/`traf`/`trun`, the `fMP4`/streaming case) is
//!   parsed for *presence* (a movie with no `moov` sample tables is reported as
//!   [`Mp4Error::Fragmented`]) but the per-fragment sample run is **not** resolved
//!   yet — the common-case non-fragmented (`moov`-based progressive download) file
//!   is fully supported. Marked `[~]` until fragment-run extraction lands.
//! - Codec *internals* are never parsed (this is a demuxer).
//!
//! ## Hostile-input posture (CLAUDE: parsers are the #1 RCE surface)
//! Every byte is attacker-controlled. There is **no `unwrap`/`expect`/panic/
//! raw-index path** reachable from [`Mp4::parse`]: box count ([`MAX_BOXES`]),
//! sample count ([`MAX_SAMPLES`]), and chunk/entry counts are all bounded *before*
//! allocation; nesting is structurally fixed ([`MAX_BOX_DEPTH`] — recursion only
//! descends known container types, never an attacker-named box); a `size`-too-small
//! / `size == 0` nested / truncated / overflowing box can neither read past the
//! buffer nor loop forever. [`Track::sample_data`] bounds-checks the resolved offset+size against
//! the original buffer length on every call. The host KAT + fuzz suite at the
//! bottom of this file is the primary proof (`cargo test -p rae_mp4`).

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

// ─── Bounds (a crafted file cannot exhaust memory or loop) ───────────────────

/// The structural box-nesting depth this demuxer descends. Recursion only ever
/// enters *known container types* (moov → trak → mdia → minf → stbl → stsd-child,
/// ~6 levels), never an attacker-named/`uuid`/unknown box — so nesting depth is a
/// fixed structural constant, not attacker-controlled, and the recursion cannot be
/// driven into a stack-overflow bomb. This ceiling documents that structural max.
pub const MAX_BOX_DEPTH: usize = 32;
/// Maximum total number of boxes parsed across the whole file. A flood of empty
/// 8-byte boxes cannot make the parser allocate/iterate unboundedly.
pub const MAX_BOXES: usize = 1 << 20; // 1,048,576
/// Maximum samples in any one track's sample table. `stsz` carries a 32-bit
/// `sample_count`; a crafted huge count is rejected before the table is built.
/// ~16M samples ≈ days of 24fps video — well beyond any real file.
pub const MAX_SAMPLES: u32 = 16 * 1024 * 1024;
/// Maximum entries in any single table chunk (`stts`/`stsc`/`stco`/…). Bounds the
/// per-table allocation independent of `MAX_SAMPLES`.
pub const MAX_TABLE_ENTRIES: u32 = 16 * 1024 * 1024;
/// Maximum number of tracks. Real files have a handful; the ceiling caps `trak`
/// flooding.
pub const MAX_TRACKS: usize = 1024;
/// Maximum codec-private blob size kept per track (esds/avcC/hvcC). Real configs
/// are tens of bytes; the cap stops a giant `stsd` entry ballooning memory.
pub const MAX_CODEC_PRIVATE: usize = 1 << 20; // 1 MiB

// ─── Errors (every variant is a handled path — nothing here panics) ──────────

/// MP4 demux error. Every variant is reachable from hostile input and returned
/// cleanly; none of these is a panic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mp4Error {
    /// The buffer is too short to contain even a box header / required field.
    Truncated,
    /// A box's `size` field was smaller than its own header (would not advance →
    /// infinite loop), or a `largesize`/offset computation overflowed.
    BadBoxSize,
    /// The file exceeded [`MAX_BOXES`] total boxes (a box-flood). (Nesting depth
    /// is structurally fixed — see [`MAX_BOX_DEPTH`] — so it cannot be exceeded.)
    TooComplex,
    /// No `ftyp` box was found, or its body was too short for a major brand.
    NoFtyp,
    /// No `moov` box was found.
    NoMoov,
    /// A `moov` exists but carries no usable sample tables — the file is
    /// fragmented (`moof`-based). Fragment sample-run extraction is deferred.
    Fragmented,
    /// A declared sample/chunk/entry count exceeded its bound ([`MAX_SAMPLES`] /
    /// [`MAX_TABLE_ENTRIES`]).
    TooManyEntries,
    /// A full-box version byte was a value this demuxer does not implement.
    UnsupportedVersion,
    /// A sample table was internally inconsistent (e.g. `stsc` referenced a chunk
    /// past `stco`, or sample math overflowed) — the file cannot be demuxed.
    MalformedSampleTable,
    /// More tracks than [`MAX_TRACKS`].
    TooManyTracks,
}

// ─── Public model ────────────────────────────────────────────────────────────

/// What an elementary stream carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackKind {
    /// `hdlr` handler `'soun'`.
    Audio,
    /// `hdlr` handler `'vide'`.
    Video,
    /// `'text'`/`'sbtl'`/`'subt'` (subtitles/timed text) or anything else.
    Other,
}

/// A friendly view of the `stsd` sample-entry fourcc. The raw 4 bytes are always
/// available in [`Track::codec_fourcc`]; this enum is the ergonomic switch a
/// player uses to pick a decoder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Codec {
    /// `mp4a` — MPEG-4 audio, almost always AAC (config in `esds`).
    Aac,
    /// `alac` — Apple Lossless.
    Alac,
    /// `Opus` — Opus in MP4.
    Opus,
    /// `avc1`/`avc3` — H.264/AVC (config in `avcC`).
    H264,
    /// `hev1`/`hvc1` — H.265/HEVC (config in `hvcC`).
    Hevc,
    /// `mp4v` — MPEG-4 Visual.
    Mpeg4Visual,
    /// `vp09` — VP9.
    Vp9,
    /// `av01` — AV1.
    Av1,
    /// Any fourcc not in the friendly set (raw bytes still in `codec_fourcc`).
    Other,
}

impl Codec {
    fn from_fourcc(f: &[u8; 4]) -> Codec {
        match f {
            b"mp4a" => Codec::Aac,
            b"alac" => Codec::Alac,
            b"Opus" => Codec::Opus,
            b"avc1" | b"avc3" => Codec::H264,
            b"hev1" | b"hvc1" => Codec::Hevc,
            b"mp4v" => Codec::Mpeg4Visual,
            b"vp09" => Codec::Vp9,
            b"av01" => Codec::Av1,
            _ => Codec::Other,
        }
    }
}

/// Audio sample-entry parameters (from the `stsd` audio sample entry).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AudioParams {
    pub channels: u16,
    pub sample_size_bits: u16,
    /// Sample rate in Hz (the 16.16 fixed-point `samplerate` field's integer part).
    pub sample_rate: u32,
}

/// Video sample-entry dimensions (from the `stsd` visual sample entry).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct VideoParams {
    pub width: u16,
    pub height: u16,
}

/// One resolved sample: where its elementary-stream bytes live in the file and
/// when it plays.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sample {
    /// Absolute offset of this sample's bytes in the original file buffer.
    pub offset: u64,
    /// Byte length of the sample.
    pub size: u32,
    /// Decode timestamp, in this track's media timescale (sum of `stts` deltas).
    pub dts: u64,
    /// Composition (presentation) timestamp = `dts + ctts` offset.
    pub cts: u64,
    /// True if `stss` lists this sample as a sync sample (keyframe). If no `stss`
    /// box is present every sample is a sync sample (per spec) → `true`.
    pub is_sync: bool,
}

/// A demuxed track.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Track {
    pub id: u32,
    pub kind: TrackKind,
    pub codec: Codec,
    /// The raw 4-byte sample-entry fourcc (e.g. `*b"mp4a"`).
    pub codec_fourcc: [u8; 4],
    /// Media timescale (ticks per second) from `mdhd`.
    pub timescale: u32,
    /// Track media duration in `timescale` ticks from `mdhd`.
    pub duration: u64,
    /// 3-char ISO-639-2/T language (packed in `mdhd`), e.g. `"und"`.
    pub language: [u8; 3],
    /// Audio params if [`TrackKind::Audio`].
    pub audio: Option<AudioParams>,
    /// Video params if [`TrackKind::Video`].
    pub video: Option<VideoParams>,
    /// Raw codec-private config bytes (esds/avcC/hvcC payload) for the decoder.
    /// Empty if the sample entry carried none.
    pub codec_private: Vec<u8>,
    /// The fully resolved per-sample table.
    samples: Vec<Sample>,
}

impl Track {
    /// Number of samples in this track.
    pub fn sample_count(&self) -> usize {
        self.samples.len()
    }

    /// The `i`-th sample's location/timing, or `None` if `i` is out of range.
    pub fn sample(&self, i: usize) -> Option<Sample> {
        self.samples.get(i).copied()
    }

    /// All samples (in decode order).
    pub fn samples(&self) -> &[Sample] {
        &self.samples
    }

    /// Slice the elementary-stream bytes of sample `i` out of the *original file
    /// buffer* `data`. Bounds-checked on every call: returns `None` if `i` is out
    /// of range or the resolved `[offset, offset+size)` falls outside `data`.
    ///
    /// This is the load-bearing demuxer payoff: a player feeds the returned slice
    /// straight to an AAC/H.264 decoder.
    pub fn sample_data<'a>(&self, data: &'a [u8], i: usize) -> Option<&'a [u8]> {
        let s = self.samples.get(i)?;
        let start = usize::try_from(s.offset).ok()?;
        let len = s.size as usize;
        let end = start.checked_add(len)?;
        data.get(start..end)
    }

    /// True if this is an audio track (the `.m4a` playback path).
    pub fn is_audio(&self) -> bool {
        matches!(self.kind, TrackKind::Audio)
    }
}

/// A demuxed MP4 file: brand info + tracks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mp4 {
    pub major_brand: [u8; 4],
    pub compatible_brands: Vec<[u8; 4]>,
    /// Movie timescale from `mvhd` (ticks per second for movie-level durations).
    pub movie_timescale: u32,
    /// Movie duration in `movie_timescale` ticks.
    pub movie_duration: u64,
    tracks: Vec<Track>,
}

impl Mp4 {
    /// All tracks.
    pub fn tracks(&self) -> &[Track] {
        &self.tracks
    }

    /// The first audio track, if any (the `.m4a` / movie-soundtrack entry point).
    pub fn first_audio_track(&self) -> Option<&Track> {
        self.tracks.iter().find(|t| t.is_audio())
    }

    /// The first video track, if any.
    pub fn first_video_track(&self) -> Option<&Track> {
        self.tracks
            .iter()
            .find(|t| matches!(t.kind, TrackKind::Video))
    }

    /// Parse an MP4/ISO-BMFF byte buffer into an [`Mp4`].
    ///
    /// Hostile-input safe: returns `Err(Mp4Error)` on any malformed input; never
    /// panics, never loops, never over-allocates.
    pub fn parse(data: &[u8]) -> Result<Mp4, Mp4Error> {
        parse_mp4(data)
    }
}

/// Iterate an audio track's samples as `(Sample, &[u8])` pairs against the file
/// buffer. This is the path that — with a future AAC decoder — plays `.m4a`:
/// `for (s, bytes) in audio_samples(&mp4, &file) { aac.decode(bytes); }`.
///
/// Returns `None` if there is no audio track. Out-of-range/garbled sample slices
/// are skipped (yielded as `None` payload) rather than aborting iteration.
pub fn audio_samples<'a>(
    mp4: &'a Mp4,
    data: &'a [u8],
) -> Option<impl Iterator<Item = (Sample, Option<&'a [u8]>)> + 'a> {
    let track = mp4.first_audio_track()?;
    Some((0..track.sample_count()).filter_map(move |i| {
        let s = track.sample(i)?;
        Some((s, track.sample_data(data, i)))
    }))
}

// ─── Bounds-checked big-endian cursor over a byte slice ──────────────────────

#[inline]
fn be_u16(b: &[u8], off: usize) -> Option<u16> {
    let s = b.get(off..off + 2)?;
    Some(u16::from_be_bytes([s[0], s[1]]))
}

#[inline]
fn be_u32(b: &[u8], off: usize) -> Option<u32> {
    let s = b.get(off..off + 4)?;
    Some(u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
}

#[inline]
fn be_u64(b: &[u8], off: usize) -> Option<u64> {
    let s = b.get(off..off + 8)?;
    Some(u64::from_be_bytes([
        s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7],
    ]))
}

#[inline]
fn fourcc(b: &[u8], off: usize) -> Option<[u8; 4]> {
    let s = b.get(off..off + 4)?;
    Some([s[0], s[1], s[2], s[3]])
}

// ─── Box header parsing ──────────────────────────────────────────────────────

/// A parsed box header. `body` is the slice of the box payload (after the header,
/// including the `uuid` if present is *excluded* — `body` starts at the user
/// type's payload). `total_len` is how far to advance the parent cursor.
struct BoxHeader<'a> {
    btype: [u8; 4],
    body: &'a [u8],
    total_len: usize,
}

/// Parse one box header at `&data[pos..]`. Returns the header + the position just
/// past the whole box. Rejects sizes that would not advance the cursor (loop
/// guard) and sizes that run past the buffer.
fn parse_box<'a>(data: &'a [u8], pos: usize) -> Result<BoxHeader<'a>, Mp4Error> {
    // Need at least the 8-byte size+type.
    let hdr = data.get(pos..).ok_or(Mp4Error::Truncated)?;
    if hdr.len() < 8 {
        return Err(Mp4Error::Truncated);
    }
    let size32 = be_u32(hdr, 0).ok_or(Mp4Error::Truncated)? as u64;
    let btype = fourcc(hdr, 4).ok_or(Mp4Error::Truncated)?;

    // Header byte count (size+type, plus largesize, plus uuid).
    let mut header_len = 8usize;

    let box_size: u64 = match size32 {
        0 => {
            // size == 0 → box extends to end of file.
            (data.len() - pos) as u64
        }
        1 => {
            // 64-bit largesize follows the type.
            let large = be_u64(hdr, 8).ok_or(Mp4Error::Truncated)?;
            header_len += 8;
            large
        }
        n => n,
    };

    // A `uuid` box has a 16-byte extended type after the (large)size+type.
    if &btype == b"uuid" {
        header_len = header_len.checked_add(16).ok_or(Mp4Error::BadBoxSize)?;
    }

    // The box must be at least as large as its own header (else the cursor would
    // not advance → infinite loop).
    if box_size < header_len as u64 {
        return Err(Mp4Error::BadBoxSize);
    }
    let box_size = usize::try_from(box_size).map_err(|_| Mp4Error::BadBoxSize)?;
    let end = pos.checked_add(box_size).ok_or(Mp4Error::BadBoxSize)?;
    if end > data.len() {
        return Err(Mp4Error::Truncated);
    }

    let body = data.get(pos + header_len..end).ok_or(Mp4Error::Truncated)?;
    Ok(BoxHeader {
        btype,
        body,
        total_len: box_size,
    })
}

/// Collect the box headers directly inside `body`. Enforces the global box-count
/// and per-box advancement guard. `counter` is the shared running box count
/// (recursion-wide). Returning a `Vec` (rather than a callback) keeps the
/// recursive descent free of nested-closure mutable-borrow conflicts on
/// `counter`/`acc`.
fn child_boxes<'a>(body: &'a [u8], counter: &mut usize) -> Result<Vec<BoxHeader<'a>>, Mp4Error> {
    let mut out = Vec::new();
    let mut pos = 0usize;
    while pos + 8 <= body.len() {
        *counter += 1;
        if *counter > MAX_BOXES {
            return Err(Mp4Error::TooComplex);
        }
        let bx = parse_box(body, pos)?;
        // total_len >= 8 is guaranteed by parse_box (header_len >= 8), so pos
        // strictly advances — no infinite loop possible.
        let next = pos.checked_add(bx.total_len).ok_or(Mp4Error::BadBoxSize)?;
        out.push(bx);
        pos = next;
    }
    Ok(out)
}

// ─── Intermediate sample-table accumulation ──────────────────────────────────

#[derive(Default)]
struct StblTables {
    // stts: (sample_count, sample_delta)
    stts: Vec<(u32, u32)>,
    // ctts: (sample_count, sample_offset) — signed in v1, stored as i64
    ctts: Vec<(u32, i64)>,
    // stsc: (first_chunk, samples_per_chunk, sample_description_index)
    stsc: Vec<(u32, u32, u32)>,
    // stsz: explicit per-sample sizes (empty if constant)
    stsz_sizes: Vec<u32>,
    stsz_constant: u32,
    stsz_count: u32,
    // chunk offsets (from stco or co64)
    chunk_offsets: Vec<u64>,
    // stss sync sample numbers (1-based)
    stss: Vec<u32>,
    have_stss: bool,
}

#[derive(Default)]
struct TrakAccum {
    track_id: u32,
    kind: Option<TrackKind>,
    timescale: u32,
    duration: u64,
    language: [u8; 3],
    video_dims: Option<VideoParams>,
    // From stsd:
    codec_fourcc: Option<[u8; 4]>,
    audio: Option<AudioParams>,
    codec_private: Vec<u8>,
    stbl: StblTables,
}

// ─── Top-level parse ─────────────────────────────────────────────────────────

fn parse_mp4(data: &[u8]) -> Result<Mp4, Mp4Error> {
    let mut counter = 0usize;
    let mut major_brand: Option<[u8; 4]> = None;
    let mut compatible_brands: Vec<[u8; 4]> = Vec::new();
    let mut movie_timescale = 0u32;
    let mut movie_duration = 0u64;
    let mut tracks: Vec<Track> = Vec::new();
    let mut saw_moov = false;
    let mut saw_moof = false;

    let top = child_boxes(data, &mut counter)?;
    for bx in &top {
        match &bx.btype {
            b"ftyp" => {
                if bx.body.len() >= 4 {
                    major_brand = Some(fourcc(bx.body, 0).ok_or(Mp4Error::NoFtyp)?);
                    // major(4) + minor_version(4) then a list of 4-byte brands.
                    let mut off = 8usize;
                    while off + 4 <= bx.body.len() {
                        if let Some(b) = fourcc(bx.body, off) {
                            compatible_brands.push(b);
                        }
                        off += 4;
                    }
                }
            }
            b"moov" => {
                saw_moov = true;
                parse_moov(
                    bx.body,
                    &mut counter,
                    &mut movie_timescale,
                    &mut movie_duration,
                    &mut tracks,
                )?;
            }
            b"moof" => {
                saw_moof = true;
            }
            _ => {}
        }
    }

    if major_brand.is_none() {
        return Err(Mp4Error::NoFtyp);
    }
    if !saw_moov {
        return Err(Mp4Error::NoMoov);
    }
    // A moov with zero resolvable samples in any track, alongside a moof, is a
    // fragmented file whose runs we do not yet extract.
    if tracks.iter().all(|t| t.samples.is_empty()) && saw_moof {
        return Err(Mp4Error::Fragmented);
    }

    Ok(Mp4 {
        major_brand: major_brand.ok_or(Mp4Error::NoFtyp)?,
        compatible_brands,
        movie_timescale,
        movie_duration,
        tracks,
    })
}

fn parse_moov(
    body: &[u8],
    counter: &mut usize,
    movie_timescale: &mut u32,
    movie_duration: &mut u64,
    tracks: &mut Vec<Track>,
) -> Result<(), Mp4Error> {
    let children = child_boxes(body, counter)?;
    for bx in &children {
        match &bx.btype {
            b"mvhd" => {
                let (ts, dur) = parse_mvhd(bx.body)?;
                *movie_timescale = ts;
                *movie_duration = dur;
            }
            b"trak" => {
                if tracks.len() >= MAX_TRACKS {
                    return Err(Mp4Error::TooManyTracks);
                }
                let mut acc = TrakAccum::default();
                acc.language = *b"und";
                parse_trak(bx.body, counter, &mut acc)?;
                if let Some(track) = finalize_track(acc)? {
                    tracks.push(track);
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// `mvhd`: version-dependent creation/modification/timescale/duration.
fn parse_mvhd(b: &[u8]) -> Result<(u32, u64), Mp4Error> {
    let version = *b.get(0).ok_or(Mp4Error::Truncated)?;
    match version {
        0 => {
            // version(1)+flags(3)+ctime(4)+mtime(4)+timescale(4)+duration(4)
            let ts = be_u32(b, 12).ok_or(Mp4Error::Truncated)?;
            let dur = be_u32(b, 16).ok_or(Mp4Error::Truncated)? as u64;
            Ok((ts, dur))
        }
        1 => {
            // version(1)+flags(3)+ctime(8)+mtime(8)+timescale(4)+duration(8)
            let ts = be_u32(b, 20).ok_or(Mp4Error::Truncated)?;
            let dur = be_u64(b, 24).ok_or(Mp4Error::Truncated)?;
            Ok((ts, dur))
        }
        _ => Err(Mp4Error::UnsupportedVersion),
    }
}

fn parse_trak(body: &[u8], counter: &mut usize, acc: &mut TrakAccum) -> Result<(), Mp4Error> {
    let children = child_boxes(body, counter)?;
    for bx in &children {
        match &bx.btype {
            b"tkhd" => parse_tkhd(bx.body, acc)?,
            b"mdia" => parse_mdia(bx.body, counter, acc)?,
            _ => {}
        }
    }
    Ok(())
}

/// `tkhd`: track id + (for video) the 16.16 fixed-point width/height.
fn parse_tkhd(b: &[u8], acc: &mut TrakAccum) -> Result<(), Mp4Error> {
    let version = *b.get(0).ok_or(Mp4Error::Truncated)?;
    // The track_id offset and the width/height offset differ by version.
    let (id_off, dims_off) = match version {
        0 => (12usize, 76usize), // v0: ...+ width@76, height@80
        1 => (20usize, 88usize), // v1: 8-byte times shift everything by 8
        _ => return Err(Mp4Error::UnsupportedVersion),
    };
    acc.track_id = be_u32(b, id_off).ok_or(Mp4Error::Truncated)?;
    // width/height are 16.16 fixed point; the integer part is the high 16 bits.
    if let (Some(w), Some(h)) = (be_u32(b, dims_off), be_u32(b, dims_off + 4)) {
        let wi = (w >> 16) as u16;
        let hi = (h >> 16) as u16;
        if wi != 0 || hi != 0 {
            acc.video_dims = Some(VideoParams {
                width: wi,
                height: hi,
            });
        }
    }
    Ok(())
}

fn parse_mdia(body: &[u8], counter: &mut usize, acc: &mut TrakAccum) -> Result<(), Mp4Error> {
    let children = child_boxes(body, counter)?;
    for bx in &children {
        match &bx.btype {
            b"mdhd" => parse_mdhd(bx.body, acc)?,
            b"hdlr" => parse_hdlr(bx.body, acc),
            b"minf" => parse_minf(bx.body, counter, acc)?,
            _ => {}
        }
    }
    Ok(())
}

/// `mdhd`: media timescale + duration + packed language.
fn parse_mdhd(b: &[u8], acc: &mut TrakAccum) -> Result<(), Mp4Error> {
    let version = *b.get(0).ok_or(Mp4Error::Truncated)?;
    let lang_off;
    match version {
        0 => {
            acc.timescale = be_u32(b, 12).ok_or(Mp4Error::Truncated)?;
            acc.duration = be_u32(b, 16).ok_or(Mp4Error::Truncated)? as u64;
            lang_off = 20;
        }
        1 => {
            acc.timescale = be_u32(b, 20).ok_or(Mp4Error::Truncated)?;
            acc.duration = be_u64(b, 24).ok_or(Mp4Error::Truncated)?;
            lang_off = 32;
        }
        _ => return Err(Mp4Error::UnsupportedVersion),
    }
    // Language: 1 pad bit + three 5-bit chars (each + 0x60 → ASCII).
    if let Some(packed) = be_u16(b, lang_off) {
        let c0 = ((packed >> 10) & 0x1F) as u8 + 0x60;
        let c1 = ((packed >> 5) & 0x1F) as u8 + 0x60;
        let c2 = (packed & 0x1F) as u8 + 0x60;
        // Only accept printable a–z; otherwise keep "und".
        if c0.is_ascii_lowercase() && c1.is_ascii_lowercase() && c2.is_ascii_lowercase() {
            acc.language = [c0, c1, c2];
        }
    }
    Ok(())
}

/// `hdlr`: the handler type at body offset 8 selects the track kind.
fn parse_hdlr(b: &[u8], acc: &mut TrakAccum) {
    // version(1)+flags(3)+pre_defined(4)+handler_type(4)
    if let Some(h) = fourcc(b, 8) {
        acc.kind = Some(match &h {
            b"vide" => TrackKind::Video,
            b"soun" => TrackKind::Audio,
            _ => TrackKind::Other,
        });
    }
}

fn parse_minf(body: &[u8], counter: &mut usize, acc: &mut TrakAccum) -> Result<(), Mp4Error> {
    let children = child_boxes(body, counter)?;
    for bx in &children {
        if &bx.btype == b"stbl" {
            parse_stbl(bx.body, counter, acc)?;
        }
    }
    Ok(())
}

fn parse_stbl(body: &[u8], counter: &mut usize, acc: &mut TrakAccum) -> Result<(), Mp4Error> {
    let children = child_boxes(body, counter)?;
    for bx in &children {
        match &bx.btype {
            b"stsd" => parse_stsd(bx.body, acc)?,
            b"stts" => acc.stbl.stts = parse_stts(bx.body)?,
            b"ctts" => acc.stbl.ctts = parse_ctts(bx.body)?,
            b"stsc" => acc.stbl.stsc = parse_stsc(bx.body)?,
            b"stsz" => parse_stsz(bx.body, acc)?,
            b"stz2" => parse_stz2(bx.body, acc)?,
            b"stco" => acc.stbl.chunk_offsets = parse_stco(bx.body)?,
            b"co64" => acc.stbl.chunk_offsets = parse_co64(bx.body)?,
            b"stss" => {
                acc.stbl.stss = parse_stss(bx.body)?;
                acc.stbl.have_stss = true;
            }
            _ => {}
        }
    }
    Ok(())
}

// ─── stsd (sample description: codec fourcc + params + codec-private) ─────────

/// Check a declared entry/sample count against a bound *before* trusting it.
fn bound_count(n: u32) -> Result<u32, Mp4Error> {
    if n > MAX_TABLE_ENTRIES {
        Err(Mp4Error::TooManyEntries)
    } else {
        Ok(n)
    }
}

fn parse_stsd(b: &[u8], acc: &mut TrakAccum) -> Result<(), Mp4Error> {
    // version(1)+flags(3)+entry_count(4) then the sample entries.
    let _version = *b.get(0).ok_or(Mp4Error::Truncated)?;
    let entry_count = be_u32(b, 4).ok_or(Mp4Error::Truncated)?;
    if entry_count == 0 {
        return Ok(());
    }
    // Parse only the first entry (the common case; multiplexed descriptions are
    // rare and a single codec per track is what a player consumes).
    let entry = b.get(8..).ok_or(Mp4Error::Truncated)?;
    // Sample entry: size(4)+format(4)+reserved(6)+data_reference_index(2)=16.
    let entry_size = be_u32(entry, 0).ok_or(Mp4Error::Truncated)? as usize;
    let format = fourcc(entry, 4).ok_or(Mp4Error::Truncated)?;
    acc.codec_fourcc = Some(format);

    // The entry body (after the 16-byte SampleEntry base) — bounded to this entry.
    let entry_end = entry_size.min(entry.len());
    let ebody = entry.get(16..entry_end).unwrap_or(&[]);

    let is_audio = matches!(acc.kind, Some(TrackKind::Audio));
    let is_video = matches!(acc.kind, Some(TrackKind::Video));

    if is_audio {
        parse_audio_sample_entry(ebody, acc);
    } else if is_video {
        parse_video_sample_entry(ebody, acc);
    } else {
        // Unknown kind: still scan for any child codec-config box.
        scan_codec_private(ebody, 0, acc);
    }
    Ok(())
}

/// AudioSampleEntry: (v0) reserved(8)+channelcount(2)+samplesize(2)+pre(2)+
/// reserved(2)+samplerate(4, 16.16). Child boxes (esds) follow at offset 20.
fn parse_audio_sample_entry(ebody: &[u8], acc: &mut TrakAccum) {
    let channels = be_u16(ebody, 8).unwrap_or(2);
    let sample_size_bits = be_u16(ebody, 10).unwrap_or(16);
    let sample_rate = be_u32(ebody, 16).map(|v| v >> 16).unwrap_or(0);
    acc.audio = Some(AudioParams {
        channels,
        sample_size_bits,
        sample_rate,
    });
    // QuickTime sound v1/v2 entries put extra fields here, but the child boxes
    // (esds/dOps/alac) follow the base v0 layout at offset 20 in the common case.
    scan_codec_private(ebody, 20, acc);
}

/// VisualSampleEntry: pre_defined/reserved(16)+width(2)+height(2)+... child
/// boxes (avcC/hvcC) start at offset 70.
fn parse_video_sample_entry(ebody: &[u8], acc: &mut TrakAccum) {
    let width = be_u16(ebody, 16).unwrap_or(0);
    let height = be_u16(ebody, 18).unwrap_or(0);
    if (width != 0 || height != 0) && acc.video_dims.is_none() {
        acc.video_dims = Some(VideoParams { width, height });
    } else if width != 0 || height != 0 {
        // Prefer the sample-entry dimensions over tkhd if present.
        acc.video_dims = Some(VideoParams { width, height });
    }
    scan_codec_private(ebody, 70, acc);
}

/// Scan child boxes inside a sample entry starting at `start`, extracting the
/// codec-private config blob. We do NOT recurse into codec internals — we keep
/// the raw payload of the relevant config box.
fn scan_codec_private(ebody: &[u8], start: usize, acc: &mut TrakAccum) {
    let mut pos = start;
    let mut guard = 0usize;
    while pos + 8 <= ebody.len() {
        guard += 1;
        if guard > 64 {
            break; // bounded scan; sample entries hold a handful of child boxes
        }
        let size = match be_u32(ebody, pos) {
            Some(s) => s as usize,
            None => break,
        };
        let ty = match fourcc(ebody, pos + 4) {
            Some(t) => t,
            None => break,
        };
        if size < 8 {
            break; // would not advance
        }
        let end = match pos.checked_add(size) {
            Some(e) if e <= ebody.len() => e,
            _ => break,
        };
        let payload = ebody.get(pos + 8..end).unwrap_or(&[]);
        match &ty {
            b"esds" | b"avcC" | b"hvcC" | b"dOps" | b"alac" | b"av1C" | b"vpcC" => {
                if acc.codec_private.is_empty() && payload.len() <= MAX_CODEC_PRIVATE {
                    acc.codec_private = payload.to_vec();
                }
            }
            _ => {}
        }
        pos = end;
    }
}

// ─── stbl table parsers (all bounded before allocation) ──────────────────────

fn parse_stts(b: &[u8]) -> Result<Vec<(u32, u32)>, Mp4Error> {
    let count = bound_count(be_u32(b, 4).ok_or(Mp4Error::Truncated)?)?;
    let mut v = Vec::new();
    let mut off = 8usize;
    for _ in 0..count {
        let sc = be_u32(b, off).ok_or(Mp4Error::Truncated)?;
        let delta = be_u32(b, off + 4).ok_or(Mp4Error::Truncated)?;
        v.push((sc, delta));
        off += 8;
    }
    Ok(v)
}

fn parse_ctts(b: &[u8]) -> Result<Vec<(u32, i64)>, Mp4Error> {
    let version = *b.get(0).ok_or(Mp4Error::Truncated)?;
    let count = bound_count(be_u32(b, 4).ok_or(Mp4Error::Truncated)?)?;
    let mut v = Vec::new();
    let mut off = 8usize;
    for _ in 0..count {
        let sc = be_u32(b, off).ok_or(Mp4Error::Truncated)?;
        let raw = be_u32(b, off + 4).ok_or(Mp4Error::Truncated)?;
        // v0: offset is u32; v1: offset is i32 (signed). Anything else → reject.
        let offset = match version {
            0 => raw as i64,
            1 => (raw as i32) as i64,
            _ => return Err(Mp4Error::UnsupportedVersion),
        };
        v.push((sc, offset));
        off += 8;
    }
    Ok(v)
}

fn parse_stsc(b: &[u8]) -> Result<Vec<(u32, u32, u32)>, Mp4Error> {
    let count = bound_count(be_u32(b, 4).ok_or(Mp4Error::Truncated)?)?;
    let mut v = Vec::new();
    let mut off = 8usize;
    for _ in 0..count {
        let first_chunk = be_u32(b, off).ok_or(Mp4Error::Truncated)?;
        let spc = be_u32(b, off + 4).ok_or(Mp4Error::Truncated)?;
        let sdi = be_u32(b, off + 8).ok_or(Mp4Error::Truncated)?;
        v.push((first_chunk, spc, sdi));
        off += 12;
    }
    Ok(v)
}

fn parse_stsz(b: &[u8], acc: &mut TrakAccum) -> Result<(), Mp4Error> {
    // version(1)+flags(3)+sample_size(4)+sample_count(4)+[sizes...]
    let constant = be_u32(b, 4).ok_or(Mp4Error::Truncated)?;
    let sample_count = be_u32(b, 8).ok_or(Mp4Error::Truncated)?;
    if sample_count > MAX_SAMPLES {
        return Err(Mp4Error::TooManyEntries);
    }
    acc.stbl.stsz_constant = constant;
    acc.stbl.stsz_count = sample_count;
    if constant == 0 {
        let mut v = Vec::new();
        let mut off = 12usize;
        for _ in 0..sample_count {
            v.push(be_u32(b, off).ok_or(Mp4Error::Truncated)?);
            off += 4;
        }
        acc.stbl.stsz_sizes = v;
    }
    Ok(())
}

/// `stz2`: a compact sample-size table with 4/8/16-bit field widths.
fn parse_stz2(b: &[u8], acc: &mut TrakAccum) -> Result<(), Mp4Error> {
    // version(1)+flags(3)+reserved(3)+field_size(1)+sample_count(4)+[sizes...]
    let field_size = *b.get(7).ok_or(Mp4Error::Truncated)?;
    let sample_count = be_u32(b, 8).ok_or(Mp4Error::Truncated)?;
    if sample_count > MAX_SAMPLES {
        return Err(Mp4Error::TooManyEntries);
    }
    acc.stbl.stsz_constant = 0;
    acc.stbl.stsz_count = sample_count;
    let mut v = Vec::with_capacity(0);
    match field_size {
        16 => {
            let mut off = 12usize;
            for _ in 0..sample_count {
                v.push(be_u16(b, off).ok_or(Mp4Error::Truncated)? as u32);
                off += 2;
            }
        }
        8 => {
            let mut off = 12usize;
            for _ in 0..sample_count {
                v.push(*b.get(off).ok_or(Mp4Error::Truncated)? as u32);
                off += 1;
            }
        }
        4 => {
            // two 4-bit entries per byte, high nibble first.
            let mut off = 12usize;
            let mut i = 0u32;
            while i < sample_count {
                let byte = *b.get(off).ok_or(Mp4Error::Truncated)?;
                v.push((byte >> 4) as u32);
                i += 1;
                if i < sample_count {
                    v.push((byte & 0x0F) as u32);
                    i += 1;
                }
                off += 1;
            }
        }
        _ => return Err(Mp4Error::MalformedSampleTable),
    }
    acc.stbl.stsz_sizes = v;
    Ok(())
}

fn parse_stco(b: &[u8]) -> Result<Vec<u64>, Mp4Error> {
    let count = bound_count(be_u32(b, 4).ok_or(Mp4Error::Truncated)?)?;
    let mut v = Vec::new();
    let mut off = 8usize;
    for _ in 0..count {
        v.push(be_u32(b, off).ok_or(Mp4Error::Truncated)? as u64);
        off += 4;
    }
    Ok(v)
}

fn parse_co64(b: &[u8]) -> Result<Vec<u64>, Mp4Error> {
    let count = bound_count(be_u32(b, 4).ok_or(Mp4Error::Truncated)?)?;
    let mut v = Vec::new();
    let mut off = 8usize;
    for _ in 0..count {
        v.push(be_u64(b, off).ok_or(Mp4Error::Truncated)?);
        off += 8;
    }
    Ok(v)
}

fn parse_stss(b: &[u8]) -> Result<Vec<u32>, Mp4Error> {
    let count = bound_count(be_u32(b, 4).ok_or(Mp4Error::Truncated)?)?;
    let mut v = Vec::new();
    let mut off = 8usize;
    for _ in 0..count {
        v.push(be_u32(b, off).ok_or(Mp4Error::Truncated)?);
        off += 4;
    }
    Ok(v)
}

// ─── Sample-table resolution (the heart: → absolute offset/size/dts/cts/sync) ─

fn finalize_track(acc: TrakAccum) -> Result<Option<Track>, Mp4Error> {
    let kind = acc.kind.unwrap_or(TrackKind::Other);
    let fourcc = acc.codec_fourcc.unwrap_or([0, 0, 0, 0]);
    let codec = Codec::from_fourcc(&fourcc);

    let samples = resolve_samples(&acc.stbl)?;

    let audio = if matches!(kind, TrackKind::Audio) {
        acc.audio.or(Some(AudioParams::default()))
    } else {
        None
    };
    let video = if matches!(kind, TrackKind::Video) {
        acc.video_dims.or(Some(VideoParams::default()))
    } else {
        None
    };

    Ok(Some(Track {
        id: acc.track_id,
        kind,
        codec,
        codec_fourcc: fourcc,
        timescale: acc.timescale,
        duration: acc.duration,
        language: acc.language,
        audio,
        video,
        codec_private: acc.codec_private,
        samples,
    }))
}

/// Combine stts/ctts/stsc/stsz/stco/stss into the flat per-sample table. All
/// arithmetic is checked; an inconsistent table yields an `Err`, not a panic.
fn resolve_samples(t: &StblTables) -> Result<Vec<Sample>, Mp4Error> {
    // The authoritative sample count: stsz_count if set, else derive from stts.
    let sample_count = if t.stsz_count != 0 {
        t.stsz_count
    } else {
        // Sum of stts sample_counts (bounded).
        let mut total: u64 = 0;
        for &(sc, _) in &t.stts {
            total = total.saturating_add(sc as u64);
        }
        if total > MAX_SAMPLES as u64 {
            return Err(Mp4Error::TooManyEntries);
        }
        total as u32
    };
    if sample_count == 0 {
        return Ok(Vec::new());
    }
    if sample_count > MAX_SAMPLES {
        return Err(Mp4Error::TooManyEntries);
    }

    // ── Per-sample size ──────────────────────────────────────────────────────
    let size_of = |i: usize| -> Result<u32, Mp4Error> {
        if t.stsz_constant != 0 {
            Ok(t.stsz_constant)
        } else {
            t.stsz_sizes
                .get(i)
                .copied()
                .ok_or(Mp4Error::MalformedSampleTable)
        }
    };

    // ── Per-sample chunk membership + offset ─────────────────────────────────
    // Expand stsc into a per-chunk samples-per-chunk view, then walk chunks and
    // accumulate the running offset within each chunk.
    if t.stsc.is_empty() || t.chunk_offsets.is_empty() {
        return Err(Mp4Error::MalformedSampleTable);
    }
    let num_chunks = t.chunk_offsets.len();

    // Build, per chunk index (0-based), the samples_per_chunk by interpreting the
    // run-length stsc entries.
    let mut samples = Vec::with_capacity(sample_count as usize);

    // Precompute decode timestamps from stts (a run-length list of deltas).
    // We'll advance a dts cursor as we emit samples in order.
    let mut stts_iter = SttsCursor::new(&t.stts);
    // Composition offsets from ctts (run-length).
    let mut ctts_iter = CttsCursor::new(&t.ctts);

    let mut sample_index_global: u64 = 0;
    let mut dts: u64 = 0;

    // Walk stsc runs. Each run covers chunks [first_chunk, next_first_chunk).
    for (entry_i, &(first_chunk, spc, _sdi)) in t.stsc.iter().enumerate() {
        if first_chunk == 0 {
            return Err(Mp4Error::MalformedSampleTable); // chunks are 1-based
        }
        let next_first_chunk = if entry_i + 1 < t.stsc.len() {
            t.stsc[entry_i + 1].0
        } else {
            // Last run extends to the final chunk.
            (num_chunks as u64 + 1) as u32
        };
        if next_first_chunk < first_chunk {
            return Err(Mp4Error::MalformedSampleTable);
        }
        // Iterate the chunks this run covers (1-based chunk numbers).
        let mut chunk = first_chunk;
        while (chunk as usize) <= num_chunks && chunk < next_first_chunk {
            let chunk_off = t.chunk_offsets[(chunk - 1) as usize];
            let mut running = chunk_off;
            for _ in 0..spc {
                if sample_index_global >= sample_count as u64 {
                    // stsc claims more samples than stsz declared — stop cleanly.
                    break;
                }
                let i = sample_index_global as usize;
                let size = size_of(i)?;
                let delta = stts_iter.next_delta();
                let cts_off = ctts_iter.next_offset();
                let cts = if cts_off >= 0 {
                    dts.saturating_add(cts_off as u64)
                } else {
                    dts.saturating_sub((-cts_off) as u64)
                };
                samples.push(Sample {
                    offset: running,
                    size,
                    dts,
                    cts,
                    is_sync: false, // filled below from stss
                });
                running = running
                    .checked_add(size as u64)
                    .ok_or(Mp4Error::MalformedSampleTable)?;
                dts = dts.saturating_add(delta as u64);
                sample_index_global += 1;
            }
            chunk += 1;
        }
    }

    if samples.is_empty() {
        return Ok(Vec::new());
    }

    // ── Sync samples (stss). No stss → every sample is sync. ─────────────────
    if t.have_stss {
        for s in samples.iter_mut() {
            s.is_sync = false;
        }
        for &num in &t.stss {
            if num == 0 {
                continue;
            }
            let idx = (num - 1) as usize;
            if let Some(s) = samples.get_mut(idx) {
                s.is_sync = true;
            }
        }
    } else {
        for s in samples.iter_mut() {
            s.is_sync = true;
        }
    }

    Ok(samples)
}

/// Walks a run-length `stts` table, yielding one decode delta per sample. Past
/// the end it yields 0 (graceful — a short stts just stops advancing time).
struct SttsCursor<'a> {
    entries: &'a [(u32, u32)],
    entry: usize,
    remaining: u32,
}
impl<'a> SttsCursor<'a> {
    fn new(entries: &'a [(u32, u32)]) -> Self {
        let remaining = entries.first().map(|e| e.0).unwrap_or(0);
        SttsCursor {
            entries,
            entry: 0,
            remaining,
        }
    }
    fn next_delta(&mut self) -> u32 {
        // Skip exhausted/zero-count entries.
        while self.entry < self.entries.len() && self.remaining == 0 {
            self.entry += 1;
            if self.entry < self.entries.len() {
                self.remaining = self.entries[self.entry].0;
            }
        }
        if self.entry >= self.entries.len() {
            return 0;
        }
        self.remaining -= 1;
        self.entries[self.entry].1
    }
}

/// Walks a run-length `ctts` table, yielding one composition offset per sample.
/// Past the end (or with no ctts) it yields 0.
struct CttsCursor<'a> {
    entries: &'a [(u32, i64)],
    entry: usize,
    remaining: u32,
}
impl<'a> CttsCursor<'a> {
    fn new(entries: &'a [(u32, i64)]) -> Self {
        let remaining = entries.first().map(|e| e.0).unwrap_or(0);
        CttsCursor {
            entries,
            entry: 0,
            remaining,
        }
    }
    fn next_offset(&mut self) -> i64 {
        if self.entries.is_empty() {
            return 0;
        }
        while self.entry < self.entries.len() && self.remaining == 0 {
            self.entry += 1;
            if self.entry < self.entries.len() {
                self.remaining = self.entries[self.entry].0;
            }
        }
        if self.entry >= self.entries.len() {
            return 0;
        }
        self.remaining -= 1;
        self.entries[self.entry].1
    }
}

/// Convenience: render a track's 3-byte language as an owned `String`.
impl Track {
    pub fn language_str(&self) -> String {
        String::from_utf8_lossy(&self.language).into_owned()
    }
}

#[cfg(test)]
mod tests;
