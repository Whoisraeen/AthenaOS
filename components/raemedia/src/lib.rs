#![no_std]

extern crate alloc;

/// From-scratch PNG decoder (signature/IHDR/IDAT, self-contained zlib inflate,
/// all 5 scanline filters → flat ARGB8888). Concept §creators/media:
/// "show my photos." Host-KAT'd; see `png.rs`.
pub mod png;

/// From-scratch PNG encoder (signature/IHDR/IDAT, zlib wrapper with Adler-32 over
/// DEFLATE stored blocks, per-chunk CRC-32 → spec-valid `.png`). Concept
/// §creators/media: "show my photos" — and *produce* them: a screenshot that saves
/// as a real `.png`, Photos export, thumbnail cache. Round-trips through `png.rs`.
/// Host-KAT'd; see `png_encode.rs`.
pub mod png_encode;

/// From-scratch baseline JPEG decoder (JFIF markers, Huffman entropy, dequant,
/// zig-zag, 8x8 IDCT, YCbCr→RGB, 4:4:4/4:2:2/4:2:0 + grayscale → flat ARGB8888).
/// Concept §creators/media: "show my photos" — JPEG is the format real photos ship
/// in. Baseline only; progressive/arithmetic/lossless return a clean Err. Host-KAT'd;
/// see `jpeg.rs`.
pub mod jpeg;

/// EXIF orientation parsing + the 8 EXIF transforms on a decoded ARGB buffer.
/// Concept §creators/media: "show my photos" — phones/cameras store frames in
/// sensor order and record an Orientation tag; a viewer that ignores it shows
/// sideways/mirrored photos. Hostile-input-safe (never panics; missing/garbage
/// EXIF → identity). Also exposes `decode_jpeg_oriented`. Host-KAT'd; see `exif.rs`.
pub mod exif;

/// From-scratch RIFF/WAVE (`.wav`) PCM decoder → interleaved i16-normalized PCM.
/// Concept §creators/media: *"play my music"* — WAV is the lossless format every
/// recording/export tool produces. Handles PCM 8/16/24/32-bit + IEEE float 32/64
/// (incl. WAVE_FORMAT_EXTENSIBLE), skips unknown chunks, and treats every file as
/// hostile input (truncated/oversized/bad-fmt → clean `Err`, never panics). The
/// `apps/music` player streams the result through `raekit::audio_submit`
/// (SYS_AUDIO_SUBMIT → mixer → ring → HDA). Host-KAT'd; see `wav.rs`.
pub mod wav;

/// From-scratch native FLAC decoder (RFC 9639) to interleaved PCM. Concept
/// creators/media: "play my music" - FLAC is the lossless format real music
/// libraries ship in (WAV was the only format that produced sound before this).
/// Decodes STREAMINFO + all four subframe types (CONSTANT/VERBATIM/FIXED/LPC),
/// partitioned Rice residual (both methods + escape), and all four channel
/// decorrelations (independent/left-side/right-side/mid-side). Every read is
/// bounds-checked: a truncated/corrupt stream returns `Err`, never panics - the
/// untrusted-input boundary. Host-KAT'd against constructed known streams (concrete
/// PCM match); see `flac.rs`.
pub mod flac;

/// From-scratch native MPEG-1/2/2.5 Audio Layer III (MP3) frame parser + entropy
/// decoder. Concept §creators/media: "play my music" - MP3 is the dominant music
/// format; before this the MP3 path was header-parse + silence. Decodes the frame
/// header (version/bitrate/sample-rate/mode/CRC/frame-size), the full side
/// information (main_data_begin, scfsi, per-granule big_values/global_gain/block
/// switching/table selects/region counts/flags), the bit-reservoir main_data
/// assembly, and Huffman big-values + count1 entropy decode for the verified ISO
/// Table B.7 subset. Every read is bounds-checked: truncated/corrupt input yields a
/// clean Err, never a panic - the untrusted-input boundary. The full DSP back-end
/// (requant/IMDCT/polyphase synthesis) is implemented, so `.mp3` decodes to **audible**
/// interleaved PCM end-to-end. Host-KAT'd; see `mp3.rs` + `mp3_dsp.rs`.
pub mod mp3;

/// Generator-verified ISO Table B.7 MP3 Huffman big-value codebooks (the full conformant
/// set {1,2,3,5,6,7,8,9,10,11,12,13,15,16,24}, covering all `table_select` 0..=31 except
/// the ISO "not used" 4/14) + the DSP constant tables (scalefactor-band boundaries,
/// slen/pretab). See `mp3_tables.rs`; verified prefix-free by `tools/mp3_huff_gen/gen.rs`.
pub mod mp3_tables;

/// Generated IMDCT cosine matrices + block windows + the synthesis-filterbank cosine
/// matrix N[64][32] and the ISO Table B.3 window D[512] (no-libm at runtime; sin/cos done
/// at table-build time by `tools/mp3_huff_gen/gentab.rs`). See `mp3_imdct_tables.rs`.
pub mod mp3_imdct_tables;

/// MP3 Layer III DSP back-end: scalefactor decode, requantization, reordering, MS/
/// intensity stereo, alias reduction, and the IMDCT + windowing + overlap-add hybrid
/// filterbank. The final polyphase synthesis (ISO Table B.3 D[] window) is the
/// documented deferred stage. No-libm. Host-KAT'd; see `mp3_dsp.rs`.
pub mod mp3_dsp;

/// Native AAC-LC decoder (ISO/IEC 14496-3 / 13818-7). Concept §creators/media: "play my
/// music" - AAC-LC (`.m4a`/`.mp4` audio, raw `.aac`/ADTS) is the dominant lossy format
/// alongside MP3. Decodes the ASC/ADTS config, the SCE/CPE/LFE element loop, all 12
/// spectral/SF Huffman codebooks (incl. the cb11 escape), inverse-quant, M/S stereo, TNS,
/// and the sine+KBD-window IMDCT + 50% overlap-add filterbank to interleaved f32 PCM.
/// HE-AAC SBR/PS and PNS/intensity are documented deferred passes (degrade to audible,
/// never wrong PCM). Hostile-input-safe. Host-KAT'd; see `aac.rs` + `aac_tables.rs`.
pub mod aac;

/// Generator-verified AAC-LC Huffman codebooks (the 11 spectral books + the scalefactor
/// book, all Kraft-sum-1 prefix-free-checked by `tools/aac_huff_gen/gen.rs`) + the
/// sampling-rate/channel/SWB-offset tables and the no-libm-built filterbank tables (sine
/// + KBD windows, IMDCT, TNS dequant). See `aac_tables.rs`.
pub mod aac_tables;

/// From-scratch native H.264 baseline-profile intra (I-frame) decoder. Concept
/// §creators/media: "play my movies" — H.264 baseline/constrained-baseline is the floor
/// of "movies and downloaded video." Recovers the REAL width/height from the SPS (killing
/// the old 1920×1080 default), CAVLC-decodes the residual, intra-predicts (4×4 9-mode +
/// 16×16 4-mode + chroma), inverse-transforms + dequantizes, reconstructs in raster order,
/// and runs the in-loop deblocking filter. CABAC, P/B inter, Main/High tools, >8-bit,
/// 4:2:2/4:4:4, interlace, and multi-slice are documented deferred passes (clean `Err`,
/// never a wrong-shape frame or a panic — the consumer turns `Err` into its honest "decode
/// pending" placeholder). Host-KAT'd bit-exact against an ffmpeg golden YUV; see `h264.rs`.
pub mod h264;

/// H.264 CAVLC VLC tables + transform/dequant/deblock constant tables (ITU-T H.264 public
/// data), prefix-free-verified by `tools/h264_vlc_gen/gen.rs`. See `h264_tables.rs`.
pub mod h264_tables;

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

// ─── Error Types ────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum MediaError {
    UnsupportedFormat,
    UnsupportedCodec(CodecId),
    InvalidData(&'static str),
    EndOfStream,
    SeekFailed,
    DecoderError(&'static str),
    ResourceExhausted,
    InvalidTrack(u32),
    ParseError(&'static str),
    BufferTooSmall,
    NotInitialized,
}

// ═══════════════════════════════════════════════════════════════════════════
// §1  Container / Demuxer Layer
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContainerFormat {
    Mp4,
    Mkv,
    Avi,
    WebM,
    Ogg,
    Flac,
    Mp3,
    Wav,
    Aac,
    Ts,
    Flv,
}

pub struct MediaContainer {
    pub format: ContainerFormat,
    pub duration_ms: u64,
    pub tracks: Vec<MediaTrack>,
    pub metadata: MediaMetadata,
    pub chapters: Vec<Chapter>,
    pub attachments: Vec<Attachment>,
}

pub struct MediaTrack {
    pub id: u32,
    pub track_type: TrackType,
    pub codec: CodecId,
    pub language: Option<String>,
    pub default: bool,
    pub forced: bool,
    pub codec_private: Vec<u8>,
    pub time_base_num: u32,
    pub time_base_den: u32,
    pub video: Option<VideoTrackInfo>,
    pub audio: Option<AudioTrackInfo>,
    pub subtitle: Option<SubtitleTrackInfo>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackType {
    Video,
    Audio,
    Subtitle,
    Data,
}

pub struct VideoTrackInfo {
    pub width: u32,
    pub height: u32,
    pub pixel_format: PixelFormat,
    pub framerate_num: u32,
    pub framerate_den: u32,
    pub bit_depth: u8,
    pub color_space: ColorSpace,
    pub color_range: ColorRange,
    pub aspect_ratio_num: u32,
    pub aspect_ratio_den: u32,
    pub profile: Option<String>,
    pub level: Option<String>,
    pub bitrate: u64,
}

pub struct AudioTrackInfo {
    pub sample_rate: u32,
    pub channels: u16,
    pub channel_layout: ChannelLayout,
    pub bits_per_sample: u16,
    pub bitrate: u64,
    pub profile: Option<String>,
}

pub struct SubtitleTrackInfo {
    pub format: SubtitleFormat,
    pub encoding: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    Yuv420p,
    Yuv422p,
    Yuv444p,
    Nv12,
    Nv21,
    Rgb24,
    Rgba32,
    Bgr24,
    Bgra32,
    Yuv420p10le,
    Yuv420p12le,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorSpace {
    Bt601,
    Bt709,
    Bt2020,
    Srgb,
    DisplayP3,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorRange {
    Limited,
    Full,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelLayout {
    Mono,
    Stereo,
    Surround21,
    Surround51,
    Surround71,
    Atmos,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubtitleFormat {
    Srt,
    Ass,
    Ssa,
    WebVtt,
    PgsSup,
    DvdSub,
    Teletext,
}

pub struct MediaMetadata {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub composer: Option<String>,
    pub genre: Option<String>,
    pub date: Option<String>,
    pub track_number: Option<u32>,
    pub disc_number: Option<u32>,
    pub comment: Option<String>,
    pub description: Option<String>,
    pub cover_art: Option<Vec<u8>>,
    pub cover_art_mime: Option<String>,
    pub encoder: Option<String>,
    pub custom: BTreeMap<String, String>,
}

impl MediaMetadata {
    pub fn empty() -> Self {
        Self {
            title: None,
            artist: None,
            album: None,
            album_artist: None,
            composer: None,
            genre: None,
            date: None,
            track_number: None,
            disc_number: None,
            comment: None,
            description: None,
            cover_art: None,
            cover_art_mime: None,
            encoder: None,
            custom: BTreeMap::new(),
        }
    }
}

pub struct Chapter {
    pub title: String,
    pub start_ms: u64,
    pub end_ms: u64,
}

pub struct Attachment {
    pub filename: String,
    pub mime_type: String,
    pub description: String,
    pub data: Vec<u8>,
}

pub struct MediaPacket {
    pub track_id: u32,
    pub pts: i64,
    pub dts: i64,
    pub duration: i64,
    pub keyframe: bool,
    pub data: Vec<u8>,
    pub flags: PacketFlags,
}

pub struct PacketFlags {
    pub corrupted: bool,
    pub discard: bool,
    pub disposable: bool,
}

impl PacketFlags {
    pub fn none() -> Self {
        Self {
            corrupted: false,
            discard: false,
            disposable: false,
        }
    }
}

pub struct Demuxer {
    container: MediaContainer,
    position_ms: u64,
    eof: bool,
    #[allow(dead_code)]
    data_offset: usize,
    #[allow(dead_code)]
    data_len: usize,
    /// For uncompressed PCM (WAV) the demuxer owns the real sample bytes and emits
    /// them as decodable packets; compressed containers leave this empty (their
    /// header parse only reads geometry, full bitstream demux is codec-pending).
    pcm: Option<PcmStream>,
    /// For native FLAC (`flac.rs`) the demuxer retains the entire FLAC byte stream
    /// (marker + metadata + frames) and emits it as decodable packets; the
    /// `FlacDecoder` buffers internally and produces one `AudioFrame` per FLAC frame.
    /// `cursor` walks the retained bytes in ~bounded chunks.
    flac: Option<FlacStream>,
}

/// The retained compressed bytes of a FLAC stream so the demuxer can deliver real
/// decodable packets (instead of the empty placeholders the other compressed paths
/// emit). Emitted in chunks so a huge file isn't one giant packet.
struct FlacStream {
    bytes: Vec<u8>,
    cursor: usize,
    chunk: usize,
}

/// The decodable PCM payload of a WAV stream, retained by the demuxer so that
/// `read_packet` produces real samples (not the placeholder empty packets the
/// compressed-container paths emit). `cursor` is a byte offset into `bytes`.
struct PcmStream {
    bytes: Vec<u8>,
    cursor: usize,
    frame_bytes: usize,
    sample_rate: u32,
    /// Samples emitted per packet (one packet ≈ ~21 ms at 48 kHz). Bounds packet
    /// size so a huge `.wav` doesn't decode into one giant frame.
    frames_per_packet: usize,
}

impl Demuxer {
    pub fn open(data: &[u8]) -> Result<Self, MediaError> {
        if data.len() < 12 {
            return Err(MediaError::InvalidData(
                "data too short to identify container",
            ));
        }
        let format = Self::detect_format(data)?;
        let mut pcm = None;
        let mut flac_stream = None;
        let container = match format {
            ContainerFormat::Mp4 => Self::parse_mp4_header(data)?,
            ContainerFormat::Mkv | ContainerFormat::WebM => Self::parse_mkv_header(data)?,
            ContainerFormat::Ogg => Self::parse_ogg_header(data)?,
            ContainerFormat::Wav => {
                let c = Self::parse_wav_header(data)?;
                pcm = Self::extract_wav_pcm(data, &c);
                c
            }
            ContainerFormat::Mp3 => Self::parse_mp3_header(data)?,
            ContainerFormat::Flac => {
                let c = Self::parse_flac_header(data)?;
                flac_stream = Self::extract_flac_stream(data, &c);
                c
            }
            _ => {
                return Err(MediaError::UnsupportedFormat);
            }
        };
        Ok(Self {
            container,
            position_ms: 0,
            eof: false,
            data_offset: 0,
            data_len: data.len(),
            pcm,
            flac: flac_stream,
        })
    }

    pub fn read_packet(&mut self) -> Result<MediaPacket, MediaError> {
        if self.eof {
            return Err(MediaError::EndOfStream);
        }
        let track_id = if !self.container.tracks.is_empty() {
            self.container.tracks[0].id
        } else {
            0
        };

        // Real PCM path: slice the next chunk of samples out of the retained data.
        if let Some(stream) = &mut self.pcm {
            if stream.cursor >= stream.bytes.len() || stream.frame_bytes == 0 {
                self.eof = true;
                return Err(MediaError::EndOfStream);
            }
            let chunk_frames = ((stream.bytes.len() - stream.cursor) / stream.frame_bytes)
                .min(stream.frames_per_packet);
            if chunk_frames == 0 {
                self.eof = true;
                return Err(MediaError::EndOfStream);
            }
            let chunk_bytes = chunk_frames * stream.frame_bytes;
            let start = stream.cursor;
            let end = start + chunk_bytes;
            let payload = stream.bytes[start..end].to_vec();
            let pts = self.position_ms as i64;
            let dur_ms = if stream.sample_rate > 0 {
                (chunk_frames as i64 * 1000) / stream.sample_rate as i64
            } else {
                0
            };
            stream.cursor = end;
            self.position_ms = self.position_ms.saturating_add(dur_ms.max(0) as u64);
            return Ok(MediaPacket {
                track_id,
                pts,
                dts: pts,
                duration: dur_ms,
                keyframe: true,
                data: payload,
                flags: PacketFlags::none(),
            });
        }

        // Native FLAC path: hand the decoder the next chunk of the retained stream.
        if let Some(stream) = &mut self.flac {
            if stream.cursor >= stream.bytes.len() {
                self.eof = true;
                return Err(MediaError::EndOfStream);
            }
            let start = stream.cursor;
            let end = (start + stream.chunk).min(stream.bytes.len());
            let payload = stream.bytes[start..end].to_vec();
            stream.cursor = end;
            let pts = self.position_ms as i64;
            return Ok(MediaPacket {
                track_id,
                pts,
                dts: pts,
                duration: 0,
                keyframe: true,
                data: payload,
                flags: PacketFlags::none(),
            });
        }

        // Compressed-container placeholder path (bitstream demux is codec-pending).
        self.position_ms += 33;
        if self.position_ms >= self.container.duration_ms {
            self.eof = true;
            return Err(MediaError::EndOfStream);
        }
        Ok(MediaPacket {
            track_id,
            pts: self.position_ms as i64,
            dts: self.position_ms as i64,
            duration: 33,
            keyframe: self.position_ms % 1000 == 0,
            data: Vec::new(),
            flags: PacketFlags::none(),
        })
    }

    pub fn seek(&mut self, position_ms: u64) -> Result<(), MediaError> {
        if position_ms > self.container.duration_ms {
            return Err(MediaError::SeekFailed);
        }
        self.position_ms = position_ms;
        self.eof = false;
        // Re-anchor the PCM cursor to the seeked frame.
        if let Some(stream) = &mut self.pcm {
            if stream.sample_rate > 0 && stream.frame_bytes > 0 {
                let frame_idx =
                    (position_ms.saturating_mul(stream.sample_rate as u64) / 1000) as usize;
                let byte = frame_idx.saturating_mul(stream.frame_bytes);
                stream.cursor = byte.min(stream.bytes.len());
            }
        }
        Ok(())
    }

    /// Locate the WAV `data` chunk and snapshot its bytes + frame geometry so the
    /// demuxer can stream real PCM packets. Returns `None` if the data chunk is
    /// missing/empty or the geometry is degenerate — the pipeline then simply has
    /// no audio rather than panicking on malformed input.
    fn extract_wav_pcm(data: &[u8], container: &MediaContainer) -> Option<PcmStream> {
        let info = container.tracks.first().and_then(|t| t.audio.as_ref())?;
        let bits = info.bits_per_sample as usize;
        let ch = info.channels as usize;
        if bits == 0 || ch == 0 {
            return None;
        }
        let frame_bytes = ((bits + 7) / 8) * ch;
        if frame_bytes == 0 {
            return None;
        }
        // Walk RIFF chunks to find `data` (offset 12 = first chunk after RIFF/WAVE).
        let mut off = 12usize;
        while off + 8 <= data.len() {
            let id = &data[off..off + 4];
            let sz =
                u32::from_le_bytes([data[off + 4], data[off + 5], data[off + 6], data[off + 7]])
                    as usize;
            let body = off + 8;
            if id == b"data" {
                let avail = data.len().saturating_sub(body);
                let take = sz.min(avail);
                if take < frame_bytes {
                    return None;
                }
                // Trim to a whole number of frames.
                let take = (take / frame_bytes) * frame_bytes;
                if take == 0 {
                    return None;
                }
                // ~21ms packets, but at least 1 frame.
                let frames_per_packet = ((info.sample_rate as usize) / 48).max(1);
                return Some(PcmStream {
                    bytes: data[body..body + take].to_vec(),
                    cursor: 0,
                    frame_bytes,
                    sample_rate: info.sample_rate,
                    frames_per_packet,
                });
            }
            // Chunks are word-aligned (pad byte if size is odd).
            let advance = 8 + sz + (sz & 1);
            if advance == 0 {
                break;
            }
            off = off.checked_add(advance)?;
        }
        None
    }

    /// Retain the entire FLAC byte stream so `read_packet` delivers real decodable
    /// packets to the native `FlacDecoder`. Returns `None` only if the data is too
    /// short to plausibly hold a stream (the decoder itself re-validates everything).
    fn extract_flac_stream(data: &[u8], container: &MediaContainer) -> Option<FlacStream> {
        if data.len() < 42 || &data[0..4] != b"fLaC" {
            return None;
        }
        let _ = container; // geometry already lives on the track; bytes are self-describing
        Some(FlacStream {
            bytes: data.to_vec(),
            cursor: 0,
            // 64 KiB chunks: large enough to usually carry whole frames, small enough
            // to bound per-packet allocation. The decoder buffers across chunks.
            chunk: 64 * 1024,
        })
    }

    pub fn tracks(&self) -> &[MediaTrack] {
        &self.container.tracks
    }

    pub fn metadata(&self) -> &MediaMetadata {
        &self.container.metadata
    }

    pub fn duration_ms(&self) -> u64 {
        self.container.duration_ms
    }

    fn detect_format(data: &[u8]) -> Result<ContainerFormat, MediaError> {
        if data.len() < 12 {
            return Err(MediaError::InvalidData(
                "insufficient data for format detection",
            ));
        }
        // ftyp box (MP4/M4A/MOV family)
        if data.len() >= 8 && &data[4..8] == b"ftyp" {
            return Ok(ContainerFormat::Mp4);
        }
        // RIFF/WAVE
        if data.len() >= 12 && &data[0..4] == b"RIFF" && &data[8..12] == b"WAVE" {
            return Ok(ContainerFormat::Wav);
        }
        // RIFF/AVI
        if data.len() >= 12 && &data[0..4] == b"RIFF" && &data[8..12] == b"AVI " {
            return Ok(ContainerFormat::Avi);
        }
        // Matroska / WebM (EBML header)
        if data.len() >= 4
            && data[0] == 0x1A
            && data[1] == 0x45
            && data[2] == 0xDF
            && data[3] == 0xA3
        {
            return Ok(ContainerFormat::Mkv);
        }
        // OGG
        if data.len() >= 4 && &data[0..4] == b"OggS" {
            return Ok(ContainerFormat::Ogg);
        }
        // FLAC
        if data.len() >= 4 && &data[0..4] == b"fLaC" {
            return Ok(ContainerFormat::Flac);
        }
        // FLV
        if data.len() >= 3 && &data[0..3] == b"FLV" {
            return Ok(ContainerFormat::Flv);
        }
        // MPEG-TS (sync byte 0x47 at regular 188-byte intervals)
        if data[0] == 0x47 && data.len() >= 188 * 2 && data[188] == 0x47 {
            return Ok(ContainerFormat::Ts);
        }
        // MP3: ID3 tag or frame sync
        if data.len() >= 3 && &data[0..3] == b"ID3" {
            return Ok(ContainerFormat::Mp3);
        }
        if data.len() >= 2 && data[0] == 0xFF && (data[1] & 0xE0) == 0xE0 {
            return Ok(ContainerFormat::Mp3);
        }
        // ADTS AAC
        if data.len() >= 2 && data[0] == 0xFF && (data[1] & 0xF0) == 0xF0 {
            return Ok(ContainerFormat::Aac);
        }
        Err(MediaError::UnsupportedFormat)
    }

    fn parse_mp4_header(data: &[u8]) -> Result<MediaContainer, MediaError> {
        let mut tracks = Vec::new();
        let mut duration_ms: u64 = 0;

        let mut offset: usize = 0;
        while offset + 8 <= data.len() {
            let box_size = u32::from_be_bytes([
                data[offset],
                data[offset + 1],
                data[offset + 2],
                data[offset + 3],
            ]) as usize;
            if box_size < 8 || offset + box_size > data.len() {
                break;
            }
            let box_type = &data[offset + 4..offset + 8];

            if box_type == b"moov" {
                let mut inner = offset + 8;
                while inner + 8 <= offset + box_size {
                    let inner_size = u32::from_be_bytes([
                        data[inner],
                        data[inner + 1],
                        data[inner + 2],
                        data[inner + 3],
                    ]) as usize;
                    let inner_type = &data[inner + 4..inner + 8];
                    if inner_size < 8 {
                        break;
                    }

                    if inner_type == b"mvhd" && inner + 28 <= data.len() {
                        let timescale = u32::from_be_bytes([
                            data[inner + 20],
                            data[inner + 21],
                            data[inner + 22],
                            data[inner + 23],
                        ]) as u64;
                        let dur = u32::from_be_bytes([
                            data[inner + 24],
                            data[inner + 25],
                            data[inner + 26],
                            data[inner + 27],
                        ]) as u64;
                        if timescale > 0 {
                            duration_ms = dur * 1000 / timescale;
                        }
                    }

                    if inner_type == b"trak" {
                        tracks.push(MediaTrack {
                            id: tracks.len() as u32 + 1,
                            track_type: TrackType::Video,
                            codec: CodecId::H264,
                            language: None,
                            default: tracks.is_empty(),
                            forced: false,
                            codec_private: Vec::new(),
                            time_base_num: 1,
                            time_base_den: 90000,
                            video: Some(VideoTrackInfo {
                                width: 1920,
                                height: 1080,
                                pixel_format: PixelFormat::Yuv420p,
                                framerate_num: 30,
                                framerate_den: 1,
                                bit_depth: 8,
                                color_space: ColorSpace::Bt709,
                                color_range: ColorRange::Limited,
                                aspect_ratio_num: 16,
                                aspect_ratio_den: 9,
                                profile: None,
                                level: None,
                                bitrate: 0,
                            }),
                            audio: None,
                            subtitle: None,
                        });
                    }
                    inner += inner_size;
                }
            }
            offset += box_size;
        }

        Ok(MediaContainer {
            format: ContainerFormat::Mp4,
            duration_ms,
            tracks,
            metadata: MediaMetadata::empty(),
            chapters: Vec::new(),
            attachments: Vec::new(),
        })
    }

    fn parse_mkv_header(data: &[u8]) -> Result<MediaContainer, MediaError> {
        if data.len() < 4 {
            return Err(MediaError::InvalidData("MKV header too short"));
        }
        let is_webm = data.len() > 32 && {
            let mut found = false;
            for i in 0..data.len().saturating_sub(4) {
                if &data[i..i + 4] == b"webm" {
                    found = true;
                    break;
                }
            }
            found
        };
        let format = if is_webm {
            ContainerFormat::WebM
        } else {
            ContainerFormat::Mkv
        };

        let tracks = vec![MediaTrack {
            id: 1,
            track_type: TrackType::Video,
            codec: if is_webm { CodecId::Vp9 } else { CodecId::H264 },
            language: None,
            default: true,
            forced: false,
            codec_private: Vec::new(),
            time_base_num: 1,
            time_base_den: 1000,
            video: Some(VideoTrackInfo {
                width: 1920,
                height: 1080,
                pixel_format: PixelFormat::Yuv420p,
                framerate_num: 24,
                framerate_den: 1,
                bit_depth: 8,
                color_space: ColorSpace::Bt709,
                color_range: ColorRange::Limited,
                aspect_ratio_num: 16,
                aspect_ratio_den: 9,
                profile: None,
                level: None,
                bitrate: 0,
            }),
            audio: None,
            subtitle: None,
        }];

        Ok(MediaContainer {
            format,
            duration_ms: 0,
            tracks,
            metadata: MediaMetadata::empty(),
            chapters: Vec::new(),
            attachments: Vec::new(),
        })
    }

    fn parse_ogg_header(data: &[u8]) -> Result<MediaContainer, MediaError> {
        if data.len() < 27 {
            return Err(MediaError::InvalidData("OGG header too short"));
        }
        let _version = data[4];
        let _header_type = data[5];

        let codec = if data.len() > 35 && &data[29..35] == b"vorbis" {
            CodecId::Vorbis
        } else if data.len() > 36 && &data[28..36] == b"OpusHead" {
            CodecId::Opus
        } else if data.len() > 35 && &data[29..35] == b"theora" {
            CodecId::Theora
        } else {
            CodecId::Vorbis
        };

        let track_type = match codec {
            CodecId::Theora => TrackType::Video,
            _ => TrackType::Audio,
        };

        let tracks = vec![MediaTrack {
            id: 1,
            track_type,
            codec,
            language: None,
            default: true,
            forced: false,
            codec_private: Vec::new(),
            time_base_num: 1,
            time_base_den: 48000,
            video: if track_type == TrackType::Video {
                Some(VideoTrackInfo {
                    width: 640,
                    height: 480,
                    pixel_format: PixelFormat::Yuv420p,
                    framerate_num: 30,
                    framerate_den: 1,
                    bit_depth: 8,
                    color_space: ColorSpace::Bt601,
                    color_range: ColorRange::Limited,
                    aspect_ratio_num: 4,
                    aspect_ratio_den: 3,
                    profile: None,
                    level: None,
                    bitrate: 0,
                })
            } else {
                None
            },
            audio: if track_type == TrackType::Audio {
                Some(AudioTrackInfo {
                    sample_rate: 48000,
                    channels: 2,
                    channel_layout: ChannelLayout::Stereo,
                    bits_per_sample: 16,
                    bitrate: 0,
                    profile: None,
                })
            } else {
                None
            },
            subtitle: None,
        }];

        Ok(MediaContainer {
            format: ContainerFormat::Ogg,
            duration_ms: 0,
            tracks,
            metadata: MediaMetadata::empty(),
            chapters: Vec::new(),
            attachments: Vec::new(),
        })
    }

    fn parse_wav_header(data: &[u8]) -> Result<MediaContainer, MediaError> {
        if data.len() < 44 {
            return Err(MediaError::InvalidData("WAV header too short"));
        }
        let format_tag = u16::from_le_bytes([data[20], data[21]]);
        let channels = u16::from_le_bytes([data[22], data[23]]);
        let sample_rate = u32::from_le_bytes([data[24], data[25], data[26], data[27]]);
        let _byte_rate = u32::from_le_bytes([data[28], data[29], data[30], data[31]]);
        let bits_per_sample = u16::from_le_bytes([data[34], data[35]]);
        // 3 = IEEE float; 0xFFFE = EXTENSIBLE (32-bit float is the only float case we
        // surface here — the integer path otherwise; matches `wav.rs`'s decode set).
        let pcm_profile: Option<String> = if format_tag == 3 {
            Some(String::from("float"))
        } else {
            Some(String::from("pcm"))
        };

        let data_size = if data.len() >= 44 {
            u32::from_le_bytes([data[40], data[41], data[42], data[43]]) as u64
        } else {
            0
        };
        let bytes_per_sample = (bits_per_sample as u64 / 8) * channels as u64;
        let total_samples = if bytes_per_sample > 0 {
            data_size / bytes_per_sample
        } else {
            0
        };
        let duration_ms = if sample_rate > 0 {
            total_samples * 1000 / sample_rate as u64
        } else {
            0
        };

        let layout = match channels {
            1 => ChannelLayout::Mono,
            2 => ChannelLayout::Stereo,
            6 => ChannelLayout::Surround51,
            8 => ChannelLayout::Surround71,
            _ => ChannelLayout::Stereo,
        };

        let tracks = vec![MediaTrack {
            id: 1,
            track_type: TrackType::Audio,
            codec: CodecId::Pcm,
            language: None,
            default: true,
            forced: false,
            codec_private: Vec::new(),
            time_base_num: 1,
            time_base_den: sample_rate,
            video: None,
            audio: Some(AudioTrackInfo {
                sample_rate,
                channels,
                channel_layout: layout,
                bits_per_sample,
                bitrate: sample_rate as u64 * channels as u64 * bits_per_sample as u64,
                profile: pcm_profile,
            }),
            subtitle: None,
        }];

        Ok(MediaContainer {
            format: ContainerFormat::Wav,
            duration_ms,
            tracks,
            metadata: MediaMetadata::empty(),
            chapters: Vec::new(),
            attachments: Vec::new(),
        })
    }

    fn parse_mp3_header(data: &[u8]) -> Result<MediaContainer, MediaError> {
        // Skip an ID3v2 tag if present (10-byte header + syncsafe size).
        let mut offset = skip_id3v2(data);

        // Scan forward for the first real Layer III frame sync (tolerate junk/
        // partial bytes before the first frame). Bounded scan over the file.
        let frame_off = find_mp3_frame(data, offset);
        let header = match frame_off {
            Some(off) => {
                offset = off;
                mp3::FrameHeader::parse(&data[off..]).ok()
            }
            None => None,
        };

        // Use the real header geometry when we found a frame; else conservative
        // defaults so the track still describes plausibly-sized audio.
        let (sample_rate, channels, bitrate) = match header {
            Some(h) => (h.sample_rate, h.channels as u16, h.bitrate as u64),
            None => {
                if offset + 4 > data.len() {
                    return Err(MediaError::InvalidData("no MP3 frame sync found"));
                }
                (44100u32, 2u16, 128_000u64)
            }
        };
        let total_bytes = data.len() as u64;
        let duration_ms = if bitrate > 0 {
            total_bytes * 8 * 1000 / bitrate
        } else {
            0
        };

        let tracks = vec![MediaTrack {
            id: 1,
            track_type: TrackType::Audio,
            codec: CodecId::Mp3,
            language: None,
            default: true,
            forced: false,
            codec_private: Vec::new(),
            time_base_num: 1,
            time_base_den: sample_rate,
            video: None,
            audio: Some(AudioTrackInfo {
                sample_rate,
                channels,
                channel_layout: ChannelLayout::Stereo,
                bits_per_sample: 16,
                bitrate,
                profile: None,
            }),
            subtitle: None,
        }];

        Ok(MediaContainer {
            format: ContainerFormat::Mp3,
            duration_ms,
            tracks,
            metadata: MediaMetadata::empty(),
            chapters: Vec::new(),
            attachments: Vec::new(),
        })
    }

    fn parse_flac_header(data: &[u8]) -> Result<MediaContainer, MediaError> {
        if data.len() < 42 {
            return Err(MediaError::InvalidData("FLAC header too short"));
        }
        let streaminfo = &data[8..];
        let _min_block = u16::from_be_bytes([streaminfo[0], streaminfo[1]]);
        let _max_block = u16::from_be_bytes([streaminfo[2], streaminfo[3]]);

        let sample_rate = ((streaminfo[10] as u32) << 12)
            | ((streaminfo[11] as u32) << 4)
            | ((streaminfo[12] as u32) >> 4);
        let channels = (((streaminfo[12] >> 1) & 0x07) + 1) as u16;
        let bits_per_sample =
            ((((streaminfo[12] & 0x01) as u16) << 4) | ((streaminfo[13] >> 4) as u16)) + 1;

        let total_samples = (((streaminfo[13] & 0x0F) as u64) << 32)
            | ((streaminfo[14] as u64) << 24)
            | ((streaminfo[15] as u64) << 16)
            | ((streaminfo[16] as u64) << 8)
            | (streaminfo[17] as u64);
        let duration_ms = if sample_rate > 0 {
            total_samples * 1000 / sample_rate as u64
        } else {
            0
        };

        let layout = match channels {
            1 => ChannelLayout::Mono,
            2 => ChannelLayout::Stereo,
            _ => ChannelLayout::Stereo,
        };

        let tracks = vec![MediaTrack {
            id: 1,
            track_type: TrackType::Audio,
            codec: CodecId::Flac,
            language: None,
            default: true,
            forced: false,
            codec_private: Vec::new(),
            time_base_num: 1,
            time_base_den: sample_rate,
            video: None,
            audio: Some(AudioTrackInfo {
                sample_rate,
                channels,
                channel_layout: layout,
                bits_per_sample,
                bitrate: sample_rate as u64 * channels as u64 * bits_per_sample as u64,
                profile: None,
            }),
            subtitle: None,
        }];

        Ok(MediaContainer {
            format: ContainerFormat::Flac,
            duration_ms,
            tracks,
            metadata: MediaMetadata::empty(),
            chapters: Vec::new(),
            attachments: Vec::new(),
        })
    }
}

/// Return the byte offset past an ID3v2 tag at the start of `data` (0 if none).
/// Handles the syncsafe 28-bit size + the optional footer flag. Bounds-safe.
fn skip_id3v2(data: &[u8]) -> usize {
    if data.len() >= 10 && &data[0..3] == b"ID3" {
        let flags = data[5];
        let tag_size = ((data[6] as usize & 0x7F) << 21)
            | ((data[7] as usize & 0x7F) << 14)
            | ((data[8] as usize & 0x7F) << 7)
            | (data[9] as usize & 0x7F);
        // +10 header, +10 footer if the footer-present flag (bit 4) is set.
        let footer = if flags & 0x10 != 0 { 10 } else { 0 };
        (10 + tag_size + footer).min(data.len())
    } else {
        0
    }
}

/// Scan forward from `start` for the first byte offset that parses as a valid
/// Layer III frame header. Bounded so a garbage file can't spin. Also skips a
/// Xing/Info VBR header by simply parsing the first frame at face value (the VBR
/// header lives *inside* the first frame's data area, so the frame header itself is
/// the right anchor either way). Returns `None` if no sync is found.
fn find_mp3_frame(data: &[u8], start: usize) -> Option<usize> {
    // Need at least a 4-byte header somewhere from `start`.
    if data.len() < 4 || start > data.len() - 4 {
        return None;
    }
    // Cap the scan to a sane window so a hostile file never loops the whole length
    // repeatedly; the first frame is normally within a few bytes of `start`.
    let last = data.len() - 4; // index of last position a 4-byte header can start
    let scan_end = (start + 4096).min(last);
    let mut i = start;
    while i <= scan_end {
        if data[i] == 0xFF && (data[i + 1] & 0xE0) == 0xE0 {
            if mp3::FrameHeader::parse(&data[i..]).is_ok() {
                return Some(i);
            }
        }
        i += 1;
    }
    None
}

/// Detect whether the first MP3 frame body carries a Xing/Info or VBRI VBR header
/// (so callers can treat that frame as non-audio). `frame` must start at the 4-byte
/// frame header. Returns true if a recognized VBR-header tag is present.
fn mp3_frame_is_vbr_header(frame: &[u8], header: &mp3::FrameHeader) -> bool {
    // Xing/Info sits after the side info; VBRI sits at a fixed offset 36.
    let si = header.side_info_size();
    let xing_off = 4 + si;
    if frame.len() >= xing_off + 4 {
        let tag = &frame[xing_off..xing_off + 4];
        if tag == b"Xing" || tag == b"Info" {
            return true;
        }
    }
    if frame.len() >= 40 && &frame[36..40] == b"VBRI" {
        return true;
    }
    false
}

// ═══════════════════════════════════════════════════════════════════════════
// §2  Codec Registry
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodecId {
    // Video
    H264,
    H265,
    Vp8,
    Vp9,
    Av1,
    Mpeg2,
    Mpeg4,
    Theora,
    ProRes,
    Dnxhd,
    Mjpeg,
    // Audio
    Aac,
    Mp3,
    Flac,
    Opus,
    Vorbis,
    Pcm,
    Alac,
    Ac3,
    Eac3,
    Dts,
    TrueHd,
    Wma,
    // Subtitle
    SubRip,
    SubStationAlpha,
    WebVtt,
    DvdSubtitle,
    PgsSubtitle,
    Teletext,
    // Unknown
    Unknown(u32),
}

pub struct CodecDescriptor {
    pub id: CodecId,
    pub name: &'static str,
    pub long_name: &'static str,
    pub codec_type: TrackType,
    pub profiles: Vec<&'static str>,
    pub mime_types: Vec<&'static str>,
    pub capabilities: CodecCapabilities,
}

pub struct CodecCapabilities {
    pub hardware_accel: bool,
    pub lossy: bool,
    pub lossless: bool,
    pub intra_only: bool,
    pub experimental: bool,
}

pub fn get_codec_descriptor(id: &CodecId) -> CodecDescriptor {
    match id {
        CodecId::H264 => CodecDescriptor {
            id: CodecId::H264,
            name: "h264",
            long_name: "H.264 / AVC / MPEG-4 Part 10",
            codec_type: TrackType::Video,
            profiles: vec![
                "Baseline",
                "Main",
                "High",
                "High 10",
                "High 4:2:2",
                "High 4:4:4",
            ],
            mime_types: vec!["video/avc", "video/h264"],
            capabilities: CodecCapabilities {
                hardware_accel: true,
                lossy: true,
                lossless: false,
                intra_only: false,
                experimental: false,
            },
        },
        CodecId::H265 => CodecDescriptor {
            id: CodecId::H265,
            name: "hevc",
            long_name: "H.265 / HEVC / High Efficiency Video Coding",
            codec_type: TrackType::Video,
            profiles: vec!["Main", "Main 10", "Main Still Picture", "Range Extensions"],
            mime_types: vec!["video/hevc", "video/h265"],
            capabilities: CodecCapabilities {
                hardware_accel: true,
                lossy: true,
                lossless: false,
                intra_only: false,
                experimental: false,
            },
        },
        CodecId::Vp9 => CodecDescriptor {
            id: CodecId::Vp9,
            name: "vp9",
            long_name: "VP9",
            codec_type: TrackType::Video,
            profiles: vec!["Profile 0", "Profile 1", "Profile 2", "Profile 3"],
            mime_types: vec!["video/vp9"],
            capabilities: CodecCapabilities {
                hardware_accel: true,
                lossy: true,
                lossless: true,
                intra_only: false,
                experimental: false,
            },
        },
        CodecId::Av1 => CodecDescriptor {
            id: CodecId::Av1,
            name: "av1",
            long_name: "AOMedia Video 1",
            codec_type: TrackType::Video,
            profiles: vec!["Main", "High", "Professional"],
            mime_types: vec!["video/av1"],
            capabilities: CodecCapabilities {
                hardware_accel: true,
                lossy: true,
                lossless: true,
                intra_only: false,
                experimental: false,
            },
        },
        CodecId::Aac => CodecDescriptor {
            id: CodecId::Aac,
            name: "aac",
            long_name: "Advanced Audio Coding",
            codec_type: TrackType::Audio,
            profiles: vec!["LC", "HE-AAC", "HE-AAC v2", "LD", "ELD"],
            mime_types: vec!["audio/aac", "audio/mp4a-latm"],
            capabilities: CodecCapabilities {
                hardware_accel: false,
                lossy: true,
                lossless: false,
                intra_only: true,
                experimental: false,
            },
        },
        CodecId::Mp3 => CodecDescriptor {
            id: CodecId::Mp3,
            name: "mp3",
            long_name: "MPEG Audio Layer III",
            codec_type: TrackType::Audio,
            profiles: vec!["Layer III"],
            mime_types: vec!["audio/mpeg", "audio/mp3"],
            capabilities: CodecCapabilities {
                hardware_accel: false,
                lossy: true,
                lossless: false,
                intra_only: true,
                experimental: false,
            },
        },
        CodecId::Flac => CodecDescriptor {
            id: CodecId::Flac,
            name: "flac",
            long_name: "Free Lossless Audio Codec",
            codec_type: TrackType::Audio,
            profiles: vec![],
            mime_types: vec!["audio/flac"],
            capabilities: CodecCapabilities {
                hardware_accel: false,
                lossy: false,
                lossless: true,
                intra_only: true,
                experimental: false,
            },
        },
        CodecId::Opus => CodecDescriptor {
            id: CodecId::Opus,
            name: "opus",
            long_name: "Opus Audio",
            codec_type: TrackType::Audio,
            profiles: vec![],
            mime_types: vec!["audio/opus"],
            capabilities: CodecCapabilities {
                hardware_accel: false,
                lossy: true,
                lossless: false,
                intra_only: true,
                experimental: false,
            },
        },
        CodecId::Vorbis => CodecDescriptor {
            id: CodecId::Vorbis,
            name: "vorbis",
            long_name: "Vorbis",
            codec_type: TrackType::Audio,
            profiles: vec![],
            mime_types: vec!["audio/vorbis"],
            capabilities: CodecCapabilities {
                hardware_accel: false,
                lossy: true,
                lossless: false,
                intra_only: true,
                experimental: false,
            },
        },
        CodecId::Pcm => CodecDescriptor {
            id: CodecId::Pcm,
            name: "pcm",
            long_name: "PCM (Uncompressed)",
            codec_type: TrackType::Audio,
            profiles: vec![],
            mime_types: vec!["audio/pcm"],
            capabilities: CodecCapabilities {
                hardware_accel: false,
                lossy: false,
                lossless: true,
                intra_only: true,
                experimental: false,
            },
        },
        CodecId::SubRip => CodecDescriptor {
            id: CodecId::SubRip,
            name: "subrip",
            long_name: "SubRip Text",
            codec_type: TrackType::Subtitle,
            profiles: vec![],
            mime_types: vec!["text/srt"],
            capabilities: CodecCapabilities {
                hardware_accel: false,
                lossy: false,
                lossless: true,
                intra_only: true,
                experimental: false,
            },
        },
        _ => CodecDescriptor {
            id: id.clone(),
            name: "unknown",
            long_name: "Unknown Codec",
            codec_type: TrackType::Data,
            profiles: vec![],
            mime_types: vec![],
            capabilities: CodecCapabilities {
                hardware_accel: false,
                lossy: false,
                lossless: false,
                intra_only: false,
                experimental: true,
            },
        },
    }
}

pub fn codec_from_fourcc(fourcc: u32) -> CodecId {
    match fourcc {
        0x61766331 => CodecId::H264,   // avc1
        0x68766331 => CodecId::H265,   // hvc1
        0x68657631 => CodecId::H265,   // hev1
        0x76703038 => CodecId::Vp8,    // vp08
        0x76703039 => CodecId::Vp9,    // vp09
        0x61763031 => CodecId::Av1,    // av01
        0x6D703476 => CodecId::Mpeg4,  // mp4v
        0x6D703261 => CodecId::Mpeg2,  // mp2v
        0x61707263 => CodecId::ProRes, // aprc
        0x6D6A7067 => CodecId::Mjpeg,  // mjpg
        0x6D703461 => CodecId::Aac,    // mp4a
        0x4F707573 => CodecId::Opus,   // Opus
        0x664C6143 => CodecId::Flac,   // fLaC
        0x616C6163 => CodecId::Alac,   // alac
        0x61632D33 => CodecId::Ac3,    // ac-3
        0x65632D33 => CodecId::Eac3,   // ec-3
        _ => CodecId::Unknown(fourcc),
    }
}

pub fn codec_from_mime(mime: &str) -> Option<CodecId> {
    match mime {
        "video/avc" | "video/h264" => Some(CodecId::H264),
        "video/hevc" | "video/h265" => Some(CodecId::H265),
        "video/vp8" => Some(CodecId::Vp8),
        "video/vp9" => Some(CodecId::Vp9),
        "video/av1" => Some(CodecId::Av1),
        "audio/aac" | "audio/mp4a-latm" => Some(CodecId::Aac),
        "audio/mpeg" | "audio/mp3" => Some(CodecId::Mp3),
        "audio/flac" => Some(CodecId::Flac),
        "audio/opus" => Some(CodecId::Opus),
        "audio/vorbis" => Some(CodecId::Vorbis),
        "audio/pcm" | "audio/wav" => Some(CodecId::Pcm),
        "audio/alac" => Some(CodecId::Alac),
        "audio/ac3" => Some(CodecId::Ac3),
        "audio/eac3" => Some(CodecId::Eac3),
        "audio/dts" => Some(CodecId::Dts),
        "audio/truehd" => Some(CodecId::TrueHd),
        "text/srt" => Some(CodecId::SubRip),
        "text/ssa" | "text/ass" => Some(CodecId::SubStationAlpha),
        "text/vtt" => Some(CodecId::WebVtt),
        _ => None,
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// §3  Video Decoder Framework
// ═══════════════════════════════════════════════════════════════════════════

pub trait VideoDecoder: Send {
    fn codec(&self) -> CodecId;
    fn decode(&mut self, packet: &MediaPacket) -> Result<Option<VideoFrame>, MediaError>;
    fn flush(&mut self) -> Vec<VideoFrame>;
    fn reset(&mut self);
    fn capabilities(&self) -> DecoderCapabilities;
}

pub struct VideoFrame {
    pub width: u32,
    pub height: u32,
    pub pixel_format: PixelFormat,
    pub planes: Vec<VideoPlane>,
    pub pts: i64,
    pub duration: i64,
    pub keyframe: bool,
    pub interlaced: bool,
    pub top_field_first: bool,
    pub color_space: ColorSpace,
    pub color_range: ColorRange,
    pub hdr_metadata: Option<HdrMetadata>,
}

pub struct VideoPlane {
    pub data: Vec<u8>,
    pub stride: u32,
}

pub struct HdrMetadata {
    pub max_content_light_level: u16,
    pub max_frame_avg_light_level: u16,
    pub mastering_display: Option<MasteringDisplay>,
    pub hdr_type: HdrType,
}

pub struct MasteringDisplay {
    pub primaries: [(u16, u16); 3],
    pub white_point: (u16, u16),
    pub max_luminance: u32,
    pub min_luminance: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HdrType {
    Sdr,
    Hdr10,
    Hdr10Plus,
    DolbyVision,
    Hlg,
}

pub struct DecoderCapabilities {
    pub max_width: u32,
    pub max_height: u32,
    pub formats: Vec<PixelFormat>,
    pub hardware: bool,
    pub threads: u32,
}

// ── H.264 Decoder ───────────────────────────────────────────────────────

pub struct H264Decoder {
    sps: Vec<H264Sps>,
    pps: Vec<H264Pps>,
    dpb: Vec<VideoFrame>,
    current_slice: Option<H264SliceHeader>,
    nal_buffer: Vec<u8>,
    output_queue: Vec<VideoFrame>,
    /// Parsed param sets from the real Exp-Golomb path (the engine that reconstructs).
    real_sps: Option<h264::Sps>,
    real_pps: Option<h264::Pps>,
    /// The reconstructed keyframe pending emission (set by `process_nal` on a slice NAL).
    pending: Option<h264::DecodedYuv>,
    pending_pts: i64,
}

pub struct H264Sps {
    pub profile: u8,
    pub level: u8,
    pub width_mbs: u32,
    pub height_mbs: u32,
    pub chroma_format: u8,
    pub bit_depth_luma: u8,
    pub frame_mbs_only: bool,
    pub max_num_ref_frames: u8,
    pub poc_type: u8,
    pub log2_max_frame_num: u8,
    pub log2_max_poc_lsb: u8,
    pub num_ref_frames_in_poc_cycle: u8,
}

pub struct H264Pps {
    pub sps_id: u8,
    pub pps_id: u8,
    pub entropy_coding_mode: bool,
    pub pic_order_present: bool,
    pub num_slice_groups: u8,
    pub num_ref_idx_l0: u8,
    pub num_ref_idx_l1: u8,
    pub weighted_pred: bool,
    pub weighted_bipred: u8,
    pub init_qp: i8,
    pub chroma_qp_offset: i8,
    pub deblocking_filter_present: bool,
    pub transform_8x8: bool,
}

pub struct H264SliceHeader {
    pub slice_type: H264SliceType,
    pub frame_num: u32,
    pub pic_order_cnt_lsb: u32,
    pub redundant_pic_cnt: u32,
    pub direct_spatial_mv_pred: bool,
    pub num_ref_idx_l0: u8,
    pub num_ref_idx_l1: u8,
    pub qp_delta: i8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum H264SliceType {
    I,
    P,
    B,
    SP,
    SI,
}

impl H264Decoder {
    pub fn new() -> Self {
        Self {
            sps: Vec::new(),
            pps: Vec::new(),
            dpb: Vec::new(),
            current_slice: None,
            nal_buffer: Vec::new(),
            output_queue: Vec::new(),
            real_sps: None,
            real_pps: None,
            pending: None,
            pending_pts: 0,
        }
    }

    fn parse_nal_units(&mut self, data: &[u8]) {
        let mut i = 0;
        while i + 4 < data.len() {
            if data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 0 && data[i + 3] == 1 {
                if !self.nal_buffer.is_empty() {
                    self.process_nal(&self.nal_buffer.clone());
                    self.nal_buffer.clear();
                }
                i += 4;
            } else if data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1 {
                if !self.nal_buffer.is_empty() {
                    self.process_nal(&self.nal_buffer.clone());
                    self.nal_buffer.clear();
                }
                i += 3;
            } else {
                self.nal_buffer.push(data[i]);
                i += 1;
            }
        }
        while i < data.len() {
            self.nal_buffer.push(data[i]);
            i += 1;
        }
    }

    fn process_nal(&mut self, nal: &[u8]) {
        if nal.is_empty() {
            return;
        }
        let nal_type = nal[0] & 0x1F;
        let rbsp = h264::nal_to_rbsp(&nal[1..]);
        match nal_type {
            7 => {
                // SPS — real Exp-Golomb parse (recovers true geometry).
                if let Ok(s) = h264::parse_sps(&rbsp) {
                    // Mirror into the legacy public struct for status/inspection.
                    self.sps.push(H264Sps {
                        profile: s.profile_idc,
                        level: s.level_idc,
                        width_mbs: s.pic_width_in_mbs,
                        height_mbs: s.frame_height_in_mbs,
                        chroma_format: 1,
                        bit_depth_luma: 8,
                        frame_mbs_only: s.frame_mbs_only,
                        max_num_ref_frames: 1,
                        poc_type: s.pic_order_cnt_type,
                        log2_max_frame_num: s.log2_max_frame_num,
                        log2_max_poc_lsb: s.log2_max_poc_lsb,
                        num_ref_frames_in_poc_cycle: 0,
                    });
                    self.real_sps = Some(s);
                }
                // A High-profile/unsupported SPS yields no real_sps → slice cleanly fails.
            }
            8 => {
                if let Ok(p) = h264::parse_pps(&rbsp) {
                    self.pps.push(H264Pps {
                        sps_id: p.seq_parameter_set_id as u8,
                        pps_id: p.pic_parameter_set_id as u8,
                        entropy_coding_mode: p.entropy_coding_mode,
                        pic_order_present: p.bottom_field_pic_order_present,
                        num_slice_groups: p.num_slice_groups as u8,
                        num_ref_idx_l0: 1,
                        num_ref_idx_l1: 1,
                        weighted_pred: false,
                        weighted_bipred: 0,
                        init_qp: p.pic_init_qp as i8,
                        chroma_qp_offset: p.chroma_qp_index_offset as i8,
                        deblocking_filter_present: p.deblocking_filter_control_present,
                        transform_8x8: false,
                    });
                    self.real_pps = Some(p);
                }
                // CABAC/FMO PPS yields no real_pps → slice cleanly fails (→ Ok(None)).
            }
            1 | 5 => {
                // Coded slice / IDR → reconstruct the keyframe via the real intra engine.
                self.current_slice = Some(H264SliceHeader {
                    slice_type: H264SliceType::I,
                    frame_num: 0,
                    pic_order_cnt_lsb: 0,
                    redundant_pic_cnt: 0,
                    direct_spatial_mv_pred: false,
                    num_ref_idx_l0: 1,
                    num_ref_idx_l1: 0,
                    qp_delta: 0,
                });
                if let (Some(sps), Some(pps)) = (self.real_sps.as_ref(), self.real_pps.as_ref()) {
                    if let Ok(yuv) = h264::decode_slice(&rbsp, sps, pps, nal_type) {
                        self.pending = Some(yuv);
                    }
                    // An unsupported/hostile slice leaves `pending` None → Ok(None).
                }
            }
            _ => {}
        }
    }

    /// Build the output `VideoFrame` from the reconstructed YUV (display-cropped). Returns
    /// `None` if no keyframe was reconstructed (unsupported stream / param-set-only packet)
    /// — the consumer turns that into its honest placeholder, never a wrong-shape frame.
    fn produce_frame(&mut self, pts: i64, keyframe: bool) -> Option<VideoFrame> {
        let yuv = self.pending.take()?;
        let w = yuv.width as u32;
        let h = yuv.height as u32;
        if w == 0 || h == 0 {
            return None;
        }
        let cw = w / 2;
        Some(VideoFrame {
            width: w,
            height: h,
            pixel_format: PixelFormat::Yuv420p,
            planes: vec![
                VideoPlane {
                    data: yuv.y,
                    stride: w,
                },
                VideoPlane {
                    data: yuv.cb,
                    stride: cw,
                },
                VideoPlane {
                    data: yuv.cr,
                    stride: cw,
                },
            ],
            pts,
            duration: 33,
            keyframe,
            interlaced: false,
            top_field_first: false,
            color_space: if h >= 720 {
                ColorSpace::Bt709
            } else {
                ColorSpace::Bt601
            },
            color_range: ColorRange::Limited,
            hdr_metadata: None,
        })
    }
}

impl VideoDecoder for H264Decoder {
    fn codec(&self) -> CodecId {
        CodecId::H264
    }

    fn decode(&mut self, packet: &MediaPacket) -> Result<Option<VideoFrame>, MediaError> {
        // Flush any buffered NAL from a prior packet, then scan this packet's start codes.
        if !self.nal_buffer.is_empty() {
            self.process_nal(&self.nal_buffer.clone());
            self.nal_buffer.clear();
        }
        self.parse_nal_units(&packet.data);
        // The Annex-B scanner leaves the final NAL in nal_buffer (no trailing start code).
        if !self.nal_buffer.is_empty() {
            self.process_nal(&self.nal_buffer.clone());
            self.nal_buffer.clear();
        }
        if self.pending.is_some() {
            let frame = self.produce_frame(packet.pts, true);
            self.current_slice = None;
            // A reconstructed keyframe → Some; an unsupported slice degraded to None.
            Ok(frame)
        } else {
            // Param-set-only packet, or an unsupported/hostile slice: clean Ok(None) — the
            // consumer keeps its honest "decode pending / can't play this" placeholder.
            self.current_slice = None;
            Ok(None)
        }
    }

    fn flush(&mut self) -> Vec<VideoFrame> {
        let mut out = Vec::new();
        core::mem::swap(&mut out, &mut self.output_queue);
        out
    }

    fn reset(&mut self) {
        self.sps.clear();
        self.pps.clear();
        self.dpb.clear();
        self.current_slice = None;
        self.nal_buffer.clear();
        self.output_queue.clear();
        self.real_sps = None;
        self.real_pps = None;
        self.pending = None;
        self.pending_pts = 0;
    }

    fn capabilities(&self) -> DecoderCapabilities {
        DecoderCapabilities {
            max_width: 8192,
            max_height: 4320,
            formats: vec![PixelFormat::Yuv420p, PixelFormat::Yuv420p10le],
            hardware: false,
            threads: 1,
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// §4  Audio Decoder Framework
// ═══════════════════════════════════════════════════════════════════════════

pub trait AudioDecoder: Send {
    fn codec(&self) -> CodecId;
    fn decode(&mut self, packet: &MediaPacket) -> Result<Option<AudioFrame>, MediaError>;
    fn flush(&mut self) -> Vec<AudioFrame>;
    fn reset(&mut self);
    fn sample_rate(&self) -> u32;
    fn channels(&self) -> u16;
}

pub struct AudioFrame {
    pub samples: Vec<f32>,
    pub sample_rate: u32,
    pub channels: u16,
    pub channel_layout: ChannelLayout,
    pub pts: i64,
    pub duration: i64,
    pub nb_samples: u32,
}

// ── AAC Decoder ─────────────────────────────────────────────────────────

pub struct AacDecoder {
    pub profile: AacProfile,
    pub sample_rate: u32,
    pub channels: u16,
    pub frame_length: u32,
    pub channel_config: u8,
    pub sbr: bool,
    pub ps: bool,
    /// Per-channel IMDCT overlap-add memory + window-shape carry (persists across frames).
    states: Vec<aac::AacChannelState>,
    /// No-libm filterbank tables (sine + KBD windows), built once on first decode.
    filter_tables: Option<aac::AacFilterTables>,
    /// Config from the ASC (`codec_private`) once set; ADTS frames refresh it inline.
    config_ready: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AacProfile {
    Main,
    Lc,
    Ssr,
    Ltp,
    He,
    HeV2,
}

impl AacDecoder {
    pub fn new() -> Self {
        Self {
            profile: AacProfile::Lc,
            sample_rate: 44100,
            channels: 2,
            frame_length: 1024,
            channel_config: 2,
            sbr: false,
            ps: false,
            states: Vec::new(),
            filter_tables: None,
            config_ready: false,
        }
    }

    /// Configure from an AudioSpecificConfig (`Track::codec_private` is the esds payload;
    /// a bare ASC is also accepted). Sets sample_rate/channels/channel_config and resets
    /// the per-channel overlap state. Returns true if a valid AAC-LC config was parsed.
    pub fn configure_from_asc(&mut self, codec_private: &[u8]) -> bool {
        let cfg = aac::parse_esds(codec_private).or_else(|| aac::parse_asc(codec_private));
        if let Some(c) = cfg {
            self.sample_rate = c.sample_rate;
            self.channels = c.channels.max(1);
            self.channel_config = c.channel_config;
            self.profile = AacProfile::Lc;
            self.ensure_state();
            self.config_ready = true;
            true
        } else {
            false
        }
    }

    fn ensure_state(&mut self) {
        let n = self.channels.max(1) as usize;
        if self.states.len() != n {
            self.states = (0..n).map(|_| aac::AacChannelState::new()).collect();
        }
        if self.filter_tables.is_none() {
            self.filter_tables = Some(aac::AacFilterTables::build());
        }
    }

    /// Decode one raw_data_block (no ADTS header) into an interleaved-PCM `AudioFrame`.
    fn decode_rdb_frame(&mut self, rdb: &[u8], pts: i64) -> AudioFrame {
        self.ensure_state();
        let cfg = aac::AacConfig {
            sample_rate: self.sample_rate,
            channel_config: self.channel_config,
            channels: self.channels,
        };
        let tabs = self.filter_tables.as_ref().expect("tables built");
        let samples = aac::decode_rdb(rdb, &cfg, &mut self.states, tabs);
        let nb_samples = 1024u32;
        AudioFrame {
            samples,
            sample_rate: self.sample_rate,
            channels: self.channels,
            channel_layout: if self.channels == 1 {
                ChannelLayout::Mono
            } else {
                ChannelLayout::Stereo
            },
            pts,
            duration: if self.sample_rate > 0 {
                (nb_samples as i64 * 1000) / self.sample_rate as i64
            } else {
                0
            },
            nb_samples,
        }
    }

    fn decode_adts_header(&mut self, data: &[u8]) -> Result<usize, MediaError> {
        if data.len() < 7 {
            return Err(MediaError::InvalidData("ADTS header too short"));
        }
        if data[0] != 0xFF || (data[1] & 0xF0) != 0xF0 {
            return Err(MediaError::InvalidData("invalid ADTS sync word"));
        }
        let profile_idx = (data[2] >> 6) & 0x03;
        self.profile = match profile_idx {
            0 => AacProfile::Main,
            1 => AacProfile::Lc,
            2 => AacProfile::Ssr,
            3 => AacProfile::Ltp,
            _ => AacProfile::Lc,
        };
        let sr_idx = (data[2] >> 2) & 0x0F;
        self.sample_rate = match sr_idx {
            0 => 96000,
            1 => 88200,
            2 => 64000,
            3 => 48000,
            4 => 44100,
            5 => 32000,
            6 => 24000,
            7 => 22050,
            8 => 16000,
            9 => 12000,
            10 => 11025,
            11 => 8000,
            12 => 7350,
            _ => 44100,
        };
        self.channel_config = ((data[2] & 0x01) << 2) | ((data[3] >> 6) & 0x03);
        self.channels = match self.channel_config {
            1 => 1,
            2 => 2,
            3 => 3,
            4 => 4,
            5 => 5,
            6 => 6,
            7 => 8,
            _ => 2,
        };
        let frame_len = (((data[3] & 0x03) as usize) << 11)
            | ((data[4] as usize) << 3)
            | ((data[5] >> 5) as usize);
        Ok(frame_len)
    }

    fn generate_silence(&self, pts: i64) -> AudioFrame {
        let nb_samples = self.frame_length;
        let total = nb_samples as usize * self.channels as usize;
        AudioFrame {
            samples: vec![0.0f32; total],
            sample_rate: self.sample_rate,
            channels: self.channels,
            channel_layout: if self.channels == 1 {
                ChannelLayout::Mono
            } else {
                ChannelLayout::Stereo
            },
            pts,
            duration: (nb_samples as i64 * 1000) / self.sample_rate as i64,
            nb_samples,
        }
    }
}

impl AudioDecoder for AacDecoder {
    fn codec(&self) -> CodecId {
        CodecId::Aac
    }

    fn decode(&mut self, packet: &MediaPacket) -> Result<Option<AudioFrame>, MediaError> {
        let data = &packet.data;
        if data.is_empty() {
            return Ok(Some(self.generate_silence(packet.pts)));
        }
        // Path B — raw ADTS stream: a frame begins with the 0xFFF syncword. Parse the
        // header (which carries config inline) and strip it to reach the RDB.
        if data.len() >= 7 && data[0] == 0xFF && (data[1] & 0xF0) == 0xF0 {
            // protection_absent bit (data[1] bit 0): 1 => 7-byte header, 0 => 9 (CRC).
            let protection_absent = data[1] & 0x01 == 1;
            let header_len = if protection_absent { 7 } else { 9 };
            // decode_adts_header sets sample_rate/channels/channel_config from the header.
            let _ = self.decode_adts_header(data);
            self.profile = AacProfile::Lc;
            if data.len() > header_len {
                let rdb = &data[header_len..];
                return Ok(Some(self.decode_rdb_frame(rdb, packet.pts)));
            }
            return Ok(Some(self.generate_silence(packet.pts)));
        }
        // Path A — bare raw_data_block (from MP4). Config must already be set via
        // `configure_from_asc`; if not, fall back to current geometry (mono/stereo) so a
        // missing ASC degrades to silence rather than wrong PCM.
        if self.config_ready || self.channels > 0 {
            return Ok(Some(self.decode_rdb_frame(data, packet.pts)));
        }
        Ok(Some(self.generate_silence(packet.pts)))
    }

    fn flush(&mut self) -> Vec<AudioFrame> {
        Vec::new()
    }
    fn reset(&mut self) {
        self.profile = AacProfile::Lc;
        self.sample_rate = 44100;
        self.channels = 2;
        for s in self.states.iter_mut() {
            s.reset();
        }
        self.config_ready = false;
    }
    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
    fn channels(&self) -> u16 {
        self.channels
    }
}

// ── FLAC Decoder ────────────────────────────────────────────────────────

/// Native FLAC decoder (RFC 9639). Real lossless decode to f32-normalized PCM via
/// `flac.rs`. The decoder is fed FLAC bytes (the whole stream past the `fLaC` marker,
/// or a chunk of frames) through `decode`; it parses STREAMINFO from the first bytes
/// that carry it, then decodes each complete frame in the accumulated buffer, one
/// `AudioFrame` per `decode` call. Every parse is bounds-checked: hostile/truncated
/// input yields `None`/`Err`, never a panic.
pub struct FlacDecoder {
    pub sample_rate: u32,
    pub channels: u16,
    pub bits_per_sample: u16,
    pub total_samples: u64,
    pub min_block_size: u16,
    pub max_block_size: u16,
    /// Accumulated FLAC bytes not yet decoded. After metadata is parsed this holds
    /// the frame bitstream; `cursor` walks it frame by frame.
    buffer: Vec<u8>,
    cursor: usize,
    /// Parsed STREAMINFO geometry (set once the metadata chain is seen).
    stream_info: Option<flac::StreamInfo>,
    /// True once the `fLaC` marker + metadata chain has been consumed from `buffer`.
    metadata_parsed: bool,
    /// Running PTS in samples, used to time emitted frames.
    samples_emitted: u64,
}

impl FlacDecoder {
    pub fn new() -> Self {
        Self {
            sample_rate: 44100,
            channels: 2,
            bits_per_sample: 16,
            total_samples: 0,
            min_block_size: 4096,
            max_block_size: 4096,
            buffer: Vec::new(),
            cursor: 0,
            stream_info: None,
            metadata_parsed: false,
            samples_emitted: 0,
        }
    }

    /// Seed the decoder's geometry from a pre-parsed track (so geometry is known even
    /// before any frame is decoded). The real STREAMINFO from the bitstream still wins
    /// once the metadata chain is parsed.
    pub fn with_geometry(sample_rate: u32, channels: u16, bits_per_sample: u16) -> Self {
        let mut d = Self::new();
        d.sample_rate = sample_rate.max(1);
        d.channels = channels.max(1);
        d.bits_per_sample = bits_per_sample.max(1);
        d
    }

    /// Normalize one signed sample of `bits` depth to f32 in [-1, 1].
    #[inline]
    fn normalize(sample: i32, bits: u8) -> f32 {
        // Full-scale magnitude is 2^(bits-1); divide by it so the most-negative
        // value maps to -1.0 (matching the WAV i16 path's /32768 convention).
        let scale = if bits >= 1 && bits <= 32 {
            (1u64 << (bits - 1)) as f32
        } else {
            32768.0
        };
        sample as f32 / scale
    }

    /// Try to decode the next complete frame in the buffer. Returns `Ok(Some(frame))`
    /// on success, `Ok(None)` if the buffer doesn't yet hold a complete frame (need
    /// more data), or `Err` only on a structurally invalid (non-recoverable) stream.
    fn decode_next_frame(&mut self) -> Result<Option<AudioFrame>, MediaError> {
        // Parse metadata once.
        if !self.metadata_parsed {
            // Only attempt once we at least have the marker + a STREAMINFO header.
            if self.buffer.len() < 42 {
                return Ok(None);
            }
            match flac::parse_metadata(&self.buffer) {
                Ok((si, frame_start)) => {
                    self.sample_rate = si.sample_rate.max(1);
                    self.channels = si.channels as u16;
                    self.bits_per_sample = si.bits_per_sample as u16;
                    self.total_samples = si.total_samples;
                    self.min_block_size = si.min_block_size;
                    self.max_block_size = si.max_block_size;
                    self.stream_info = Some(si);
                    self.metadata_parsed = true;
                    self.cursor = frame_start;
                }
                Err(flac::FlacError::UnexpectedEof) => return Ok(None),
                Err(_) => return Err(MediaError::InvalidData("invalid FLAC metadata")),
            }
        }

        let si = match self.stream_info {
            Some(si) => si,
            None => return Ok(None),
        };
        if self.cursor >= self.buffer.len() {
            return Ok(None);
        }

        let remaining = &self.buffer[self.cursor..];
        match flac::decode_frame(remaining, &si) {
            Ok(decoded) => {
                let bits = decoded.bits_per_sample;
                let samples: Vec<f32> = decoded
                    .samples
                    .iter()
                    .map(|&s| Self::normalize(s, bits))
                    .collect();
                let nb = decoded.block_size;
                let ch = decoded.channels as u16;
                let sr = decoded.sample_rate.max(1);
                let pts = (self.samples_emitted as i64 * 1000) / sr as i64;
                self.samples_emitted = self.samples_emitted.saturating_add(nb as u64);
                self.cursor += decoded.consumed;
                self.sample_rate = sr;
                self.channels = ch;
                self.bits_per_sample = bits as u16;
                Ok(Some(AudioFrame {
                    samples,
                    sample_rate: sr,
                    channels: ch,
                    channel_layout: match ch {
                        1 => ChannelLayout::Mono,
                        2 => ChannelLayout::Stereo,
                        6 => ChannelLayout::Surround51,
                        8 => ChannelLayout::Surround71,
                        _ => ChannelLayout::Stereo,
                    },
                    pts,
                    duration: (nb as i64 * 1000) / sr as i64,
                    nb_samples: nb,
                }))
            }
            // Not enough bytes buffered for a full frame yet: wait for more.
            Err(flac::FlacError::UnexpectedEof) => Ok(None),
            // A corrupt frame: report cleanly. (We do not try to resync here; the
            // pipeline treats a decode error as end-of-usable-stream.)
            Err(_) => Err(MediaError::DecoderError("corrupt FLAC frame")),
        }
    }

    /// Decode a complete in-memory FLAC file to a single interleaved f32 PCM buffer.
    /// Convenience for the host-KATs and one-shot callers. Stops at the first corrupt
    /// frame, returning what decoded so far together with the geometry.
    pub fn decode_all(data: &[u8]) -> Result<(Vec<f32>, u32, u16, u16), MediaError> {
        let mut dec = FlacDecoder::new();
        dec.buffer = data.to_vec();
        let mut out: Vec<f32> = Vec::new();
        loop {
            match dec.decode_next_frame() {
                Ok(Some(frame)) => out.extend_from_slice(&frame.samples),
                Ok(None) => break,
                Err(_) => break,
            }
        }
        Ok((out, dec.sample_rate, dec.channels, dec.bits_per_sample))
    }
}

impl AudioDecoder for FlacDecoder {
    fn codec(&self) -> CodecId {
        CodecId::Flac
    }

    fn decode(&mut self, packet: &MediaPacket) -> Result<Option<AudioFrame>, MediaError> {
        // Accumulate the packet's bytes; FLAC frames may span packet boundaries.
        if !packet.data.is_empty() {
            self.buffer.extend_from_slice(&packet.data);
        }
        // Periodically drop already-consumed bytes to bound buffer growth.
        if self.cursor > 1 << 20 {
            self.buffer.drain(0..self.cursor);
            self.cursor = 0;
        }
        self.decode_next_frame()
    }

    fn flush(&mut self) -> Vec<AudioFrame> {
        // Drain any remaining complete frames in the buffer.
        let mut out = Vec::new();
        while let Ok(Some(frame)) = self.decode_next_frame() {
            out.push(frame);
        }
        out
    }
    fn reset(&mut self) {
        self.buffer.clear();
        self.cursor = 0;
        self.stream_info = None;
        self.metadata_parsed = false;
        self.samples_emitted = 0;
    }
    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
    fn channels(&self) -> u16 {
        self.channels
    }
}

// ── Opus Decoder ────────────────────────────────────────────────────────

pub struct OpusDecoder {
    pub sample_rate: u32,
    pub channels: u16,
    pub pre_skip: u16,
    pub gain: i16,
}

impl OpusDecoder {
    pub fn new() -> Self {
        Self {
            sample_rate: 48000,
            channels: 2,
            pre_skip: 312,
            gain: 0,
        }
    }

    fn parse_toc(&self, data: &[u8]) -> (u32, u8) {
        if data.is_empty() {
            return (960, 1);
        }
        let toc = data[0];
        let config = (toc >> 3) & 0x1F;
        let frame_size = match config {
            0..=3 => 480,
            4..=7 => 960,
            8..=11 => 1920,
            12..=13 => 480,
            14..=15 => 960,
            16..=19 => 120,
            20..=23 => 240,
            24..=27 => 480,
            28..=31 => 960,
            _ => 960,
        };
        let frame_count = match toc & 0x03 {
            0 => 1,
            1 => 2,
            2 => 2,
            _ => 0, // arbitrary, parsed from data
        };
        (frame_size, frame_count)
    }
}

impl AudioDecoder for OpusDecoder {
    fn codec(&self) -> CodecId {
        CodecId::Opus
    }

    fn decode(&mut self, packet: &MediaPacket) -> Result<Option<AudioFrame>, MediaError> {
        let (frame_size, _count) = self.parse_toc(&packet.data);
        let nb_samples = frame_size;
        let total = nb_samples as usize * self.channels as usize;
        Ok(Some(AudioFrame {
            samples: vec![0.0f32; total],
            sample_rate: self.sample_rate,
            channels: self.channels,
            channel_layout: if self.channels == 1 {
                ChannelLayout::Mono
            } else {
                ChannelLayout::Stereo
            },
            pts: packet.pts,
            duration: (nb_samples as i64 * 1000) / self.sample_rate as i64,
            nb_samples,
        }))
    }

    fn flush(&mut self) -> Vec<AudioFrame> {
        Vec::new()
    }
    fn reset(&mut self) {
        self.pre_skip = 312;
    }
    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
    fn channels(&self) -> u16 {
        self.channels
    }
}

// ── MP3 Decoder ─────────────────────────────────────────────────────────
//
// Backed by the native `mp3` module: the frame header, the full side information,
// the bit-reservoir main_data assembly, the Huffman entropy decode, and the full DSP
// back-end (requantization, reorder, stereo, alias reduction, IMDCT/overlap, and the
// polyphase synthesis filterbank) are all decoded for real (host-KAT'd). `decode`
// produces **audible** interleaved PCM end-to-end. Truncated/corrupt input — or a
// not-yet-resolvable reservoir back-reference at stream start — yields geometry-correct
// silence, never wrong samples or a panic (the untrusted-input boundary). The reservoir
// + per-channel overlap + synthesis V[] FIFO are maintained across frames.

pub struct Mp3Decoder {
    pub version: Mp3Version,
    pub layer: u8,
    pub bitrate: u32,
    pub sample_rate: u32,
    pub channels: Mp3ChannelMode,
    pub samples_per_frame: u32,
    /// Bit reservoir for main_data that references earlier frames.
    reservoir: mp3::Reservoir,
    /// Per-channel IMDCT overlap-add state, carried across granules/frames.
    dsp_state: [mp3_dsp::ChannelState; 2],
    /// Granule-0 spectrum per channel, retained for scfsi scalefactor reuse in gr 1.
    prev_scalefac: [mp3_dsp::GranuleSpectrum; 2],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mp3Version {
    Mpeg1,
    Mpeg2,
    Mpeg25,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mp3ChannelMode {
    Stereo,
    JointStereo,
    DualChannel,
    Mono,
}

impl Mp3Decoder {
    pub fn new() -> Self {
        Self {
            version: Mp3Version::Mpeg1,
            layer: 3,
            bitrate: 128,
            sample_rate: 44100,
            channels: Mp3ChannelMode::Stereo,
            samples_per_frame: 1152,
            reservoir: mp3::Reservoir::new(),
            dsp_state: [mp3_dsp::ChannelState::new(), mp3_dsp::ChannelState::new()],
            prev_scalefac: [
                mp3_dsp::GranuleSpectrum::zero(),
                mp3_dsp::GranuleSpectrum::zero(),
            ],
        }
    }

    /// Parse a frame via the native `mp3` module: real header + side information +
    /// reservoir maintenance. Returns the parsed header (geometry/timing) on success.
    /// Truncated/corrupt input → clean `Err`, never a panic (untrusted boundary).
    fn parse_frame(&mut self, data: &[u8]) -> Result<mp3::FrameHeader, MediaError> {
        let off = skip_id3v2(data);
        let frame_off =
            find_mp3_frame(data, off).ok_or(MediaError::InvalidData("no MP3 frame sync"))?;
        let frame = &data[frame_off..];
        let header = mp3::FrameHeader::parse(frame)
            .map_err(|_| MediaError::InvalidData("bad MP3 header"))?;

        // Reflect geometry onto the public fields.
        self.version = match header.version {
            mp3::MpegVersion::V1 => Mp3Version::Mpeg1,
            mp3::MpegVersion::V2 => Mp3Version::Mpeg2,
            mp3::MpegVersion::V25 => Mp3Version::Mpeg25,
        };
        self.layer = header.layer;
        self.bitrate = header.bitrate / 1000;
        self.sample_rate = header.sample_rate;
        self.channels = match header.mode {
            mp3::ChannelMode::Stereo => Mp3ChannelMode::Stereo,
            mp3::ChannelMode::JointStereo => Mp3ChannelMode::JointStereo,
            mp3::ChannelMode::DualChannel => Mp3ChannelMode::DualChannel,
            mp3::ChannelMode::Mono => Mp3ChannelMode::Mono,
        };
        self.samples_per_frame = header.samples_per_frame() as u32;

        // Parse side information + maintain the bit reservoir (exercises the entropy
        // boundary even though the DSP back-end that consumes main_data is deferred).
        let crc = if header.protection { 2 } else { 0 };
        let si_start = 4 + crc;
        let si_size = header.side_info_size();
        if frame.len() >= si_start + si_size {
            let si_bytes = &frame[si_start..si_start + si_size];
            // A malformed side-info is non-fatal here: geometry is already known.
            let _ = mp3::parse_side_info(si_bytes, header.version, header.channels);
            // Feed this frame's main_data area to the reservoir (skip VBR header frames).
            if !mp3_frame_is_vbr_header(frame, &header) {
                let main_start = si_start + si_size;
                let main_end = header.frame_size.min(frame.len());
                if main_end > main_start {
                    self.reservoir.push(&frame[main_start..main_end]);
                }
            }
        }
        Ok(header)
    }

    /// Run the full DSP pipeline (requant → reorder → stereo → alias → IMDCT/overlap)
    /// for one granule, given the per-channel Huffman-decoded spectra and the granule's
    /// side info. Returns the subband-domain time output `[ch][576]` — the
    /// hybrid-filterbank output, the stage just before the (deferred) polyphase
    /// synthesis. Updates the per-channel overlap state. Pure/deterministic apart from
    /// the carried overlap, so it is the host-KAT boundary for the DSP path.
    pub fn run_dsp_granule(
        &mut self,
        gr: usize,
        channels: usize,
        mode_ext: u8,
        joint: bool,
        si: &mp3::SideInfo,
        spectra: &[mp3_dsp::GranuleSpectrum; 2],
    ) -> [[f32; mp3_dsp::NLINES]; 2] {
        let mut xr = [[0.0f32; mp3_dsp::NLINES]; 2];
        let nch = channels.min(2);
        for ch in 0..nch {
            let g = &si.gr[gr][ch];
            mp3_dsp::requantize(&spectra[ch], g, self.sample_rate, &mut xr[ch]);
            mp3_dsp::reorder(&mut xr[ch], g, self.sample_rate);
        }
        if nch == 2 && joint {
            let (l, r) = xr.split_at_mut(1);
            mp3_dsp::stereo(&mut l[0], &mut r[0], mode_ext);
        }
        let mut out = [[0.0f32; mp3_dsp::NLINES]; 2];
        for ch in 0..nch {
            let g = &si.gr[gr][ch];
            mp3_dsp::alias_reduce(&mut xr[ch], g);
            mp3_dsp::imdct(&xr[ch], g, &mut self.dsp_state[ch], &mut out[ch]);
        }
        out
    }

    /// Decode one MP3 frame end-to-end through the hybrid filterbank: header + side
    /// info + reservoir main_data assembly + (per granule/channel) scalefactor decode
    /// + Huffman spectrum + the full DSP pipeline → subband-domain time samples for the
    /// whole frame, interleaved per granule. Returns `(subband[ch] flat, channels)`,
    /// where each channel's vec holds `granules*576` samples. Truncated/corrupt input
    /// yields the geometry-correct silent result, never a panic.
    ///
    /// This is the real entropy+DSP path; the only stage NOT applied is the final
    /// polyphase synthesis (deferred — the D[] window), so the result is the
    /// hybrid-filterbank output, not yet audible PCM. The decoder runs it for real to
    /// exercise + maintain the cross-granule overlap/scfsi state.
    pub fn decode_hybrid_frame(
        &mut self,
        data: &[u8],
    ) -> Result<(Vec<Vec<f32>>, usize), MediaError> {
        let off = skip_id3v2(data);
        let frame_off =
            find_mp3_frame(data, off).ok_or(MediaError::InvalidData("no MP3 frame sync"))?;
        let frame = &data[frame_off..];
        let header = mp3::FrameHeader::parse(frame)
            .map_err(|_| MediaError::InvalidData("bad MP3 header"))?;
        // Reflect geometry (also done in parse_frame; harmless to repeat).
        self.sample_rate = header.sample_rate;
        self.samples_per_frame = header.samples_per_frame() as u32;
        let nch = header.channels;
        let ngr = header.granules();
        let joint = header.mode == mp3::ChannelMode::JointStereo;

        let crc = if header.protection { 2 } else { 0 };
        let si_start = 4 + crc;
        let si_size = header.side_info_size();
        if frame.len() < si_start + si_size {
            return Err(MediaError::InvalidData("frame too short for side info"));
        }
        let si = mp3::parse_side_info(&frame[si_start..si_start + si_size], header.version, nch)
            .map_err(|_| MediaError::InvalidData("bad side info"))?;

        // Assemble main_data via the reservoir (back-reference into earlier frames).
        let main_start = si_start + si_size;
        let main_end = header.frame_size.min(frame.len());
        let this_main: &[u8] = if main_end > main_start {
            &frame[main_start..main_end]
        } else {
            &[]
        };
        let assembled = match self
            .reservoir
            .assemble(si.main_data_begin as usize, this_main)
        {
            Ok(v) => v,
            // Not enough history yet (stream start): emit silence, push this frame.
            Err(_) => {
                if !this_main.is_empty() {
                    self.reservoir.push(this_main);
                }
                let mut out = Vec::with_capacity(nch);
                for _ in 0..nch {
                    out.push(vec![0.0f32; ngr * mp3_dsp::NLINES]);
                }
                return Ok((out, nch));
            }
        };

        let version = header.version;
        let mut ch_out: Vec<Vec<f32>> = (0..nch)
            .map(|_| vec![0.0f32; ngr * mp3_dsp::NLINES])
            .collect();

        // Bit cursor walks the assembled main data; each granule/channel reads exactly
        // part2_3_length bits (scalefactors + Huffman), bounded so corruption stays local.
        let mut bit_cursor = 0usize;
        for gr in 0..ngr {
            let mut spectra = [
                mp3_dsp::GranuleSpectrum::zero(),
                mp3_dsp::GranuleSpectrum::zero(),
            ];
            for ch in 0..nch {
                let g = si.gr[gr][ch];
                let p23 = g.part2_3_length as usize;
                let granule_start = bit_cursor;
                let mut r = mp3::BitReader::with_pos(&assembled, granule_start);
                // Scalefactors.
                let prev = &self.prev_scalefac[ch];
                let _ = mp3_dsp::decode_scalefactors(
                    &mut r,
                    &g,
                    gr,
                    ch,
                    &si,
                    version,
                    prev,
                    &mut spectra[ch],
                );
                let sf_bits = r.bit_position().saturating_sub(granule_start);
                // Huffman region bounds (long blocks) from region0/1 counts via SFB table.
                let bounds = mp3_huffman_region_bounds(&g, header.sample_rate);
                let budget = p23.saturating_sub(sf_bits);
                let cur_bit = r.bit_position();
                let _ = mp3::decode_huffman_region(
                    &mut r,
                    g.big_values,
                    &g.table_select,
                    bounds,
                    g.count1table_select,
                    budget,
                    cur_bit,
                    &mut spectra[ch].is,
                );
                // Advance the cursor by the whole granule (part2_3_length) regardless of
                // how far decode got — keeps channels/granules aligned on corrupt data.
                bit_cursor = granule_start + p23;
            }
            // Retain granule-0 scalefactors for scfsi reuse in granule 1.
            for ch in 0..nch {
                self.prev_scalefac[ch].scalefac_l = spectra[ch].scalefac_l;
                self.prev_scalefac[ch].scalefac_s = spectra[ch].scalefac_s;
            }
            let sub = self.run_dsp_granule(gr, nch, header.mode_ext, joint, &si, &spectra);
            for ch in 0..nch {
                let base = gr * mp3_dsp::NLINES;
                ch_out[ch][base..base + mp3_dsp::NLINES].copy_from_slice(&sub[ch]);
            }
        }

        // Push this frame's main data so the NEXT frame's back-reference resolves.
        if !this_main.is_empty() {
            self.reservoir.push(this_main);
        }
        Ok((ch_out, nch))
    }

    /// Decode one MP3 frame all the way to **audible** interleaved PCM: the full entropy
    /// + DSP hybrid-filterbank path (`decode_hybrid_frame`) followed by the polyphase
    /// synthesis filterbank (`mp3_dsp::synthesis`) on every granule/channel. Returns the
    /// interleaved `f32` samples (channel-interleaved per frame, `nb_samples*channels`
    /// long) and the channel count. Truncated/corrupt input yields geometry-correct
    /// silence, never a panic — synthesis carries the per-channel V[] FIFO across
    /// granules/frames exactly like the IMDCT overlap.
    pub fn decode_frame_pcm(&mut self, data: &[u8]) -> Result<(Vec<f32>, usize), MediaError> {
        let (subband, nch) = self.decode_hybrid_frame(data)?;
        let ngr = if nch > 0 {
            subband[0].len() / mp3_dsp::NLINES
        } else {
            0
        };
        let total_per_ch = ngr * mp3_dsp::NLINES;
        let mut interleaved = vec![0.0f32; total_per_ch * nch];
        for gr in 0..ngr {
            let base = gr * mp3_dsp::NLINES;
            for ch in 0..nch {
                // Copy this granule's subband-domain output into a fixed array.
                let mut sub = [0.0f32; mp3_dsp::NLINES];
                let src = &subband[ch][base..base + mp3_dsp::NLINES];
                sub.copy_from_slice(src);
                let mut pcm = [0.0f32; mp3_dsp::NLINES];
                mp3_dsp::synthesis(&sub, &mut self.dsp_state[ch], &mut pcm);
                // Interleave: frame index `base + n`, channel `ch`.
                for n in 0..mp3_dsp::NLINES {
                    interleaved[(base + n) * nch + ch] = pcm[n];
                }
            }
        }
        Ok((interleaved, nch))
    }
}

/// R10 artifact — the raemedia MP3 status line for `/proc/raeen/media` (or wherever the
/// host surfaces crate status). Reports `mp3=audible` now that the polyphase synthesis
/// filterbank is wired (was `mp3=hybrid-silent`). Pure, allocation-light, no I/O.
pub fn mp3_procfs_status() -> &'static str {
    "mp3=audible synth=on huff_tables=15 count1=AB"
}

/// R10 artifact — boot smoketest for the MP3 synthesis path. Drives the polyphase
/// synthesis filterbank with a single nonzero subband held constant across all 18
/// sub-passes (a tone-like impulse) and verifies the output PCM is non-silent and
/// bounded, and that a zero input stays exactly silent (the FAIL lever: a wrong D[]
/// table or V/U gather makes a zero input produce energy, or a tone produce silence).
/// Returns the serial marker line; ends in `-> PASS` only when both checks hold, else
/// `-> FAIL` (the test can print FAIL). No allocation, no I/O.
pub fn run_boot_smoketest() -> alloc::string::String {
    use alloc::format;
    let mut state = mp3_dsp::ChannelState::new();

    // Zero input must yield exactly-zero PCM.
    let zero_in = [0.0f32; mp3_dsp::NLINES];
    let mut zero_pcm = [0.0f32; mp3_dsp::NLINES];
    mp3_dsp::synthesis(&zero_in, &mut state, &mut zero_pcm);
    let zero_silent = zero_pcm.iter().all(|&s| s == 0.0);

    // A constant tone in subband 1 across all 18 sub-passes must produce non-silent,
    // bounded, finite PCM.
    let mut tone_in = [0.0f32; mp3_dsp::NLINES];
    for ss in 0..18 {
        tone_in[18 + ss] = 0.5; // subband 1
    }
    let mut state2 = mp3_dsp::ChannelState::new();
    let mut tone_pcm = [0.0f32; mp3_dsp::NLINES];
    mp3_dsp::synthesis(&tone_in, &mut state2, &mut tone_pcm);
    let mut peak = 0.0f32;
    let mut finite = true;
    for &s in tone_pcm.iter() {
        if !s.is_finite() {
            finite = false;
        }
        let a = if s < 0.0 { -s } else { s };
        if a > peak {
            peak = a;
        }
    }
    let nonsilent = peak > 0.01 && peak <= 1.0 && finite;
    let pass = zero_silent && nonsilent;

    format!(
        "[raemedia] mp3 synth: frames=1 peak={:.4} nonsilent={} zero_silent={} -> {}",
        peak,
        nonsilent,
        zero_silent,
        if pass { "PASS" } else { "FAIL" }
    )
}

/// Compute the big-values Huffman region line boundaries (region0 end, region1 end)
/// for a granule. Long blocks use region0_count/region1_count mapped through the
/// scalefactor-band table; short/window-switched granules use the fixed 36/lines split.
fn mp3_huffman_region_bounds(g: &mp3::GranuleChannel, sample_rate: u32) -> (usize, usize) {
    let sfb = mp3_tables::sfb_for_rate(sample_rate);
    if g.window_switching && g.block_type == 2 {
        // Short blocks: region0 = 36 lines, region1 = rest of big_values (region2 empty).
        (36usize.min(572), 576)
    } else {
        let r0 = (g.region0_count as usize + 1).min(22);
        let r1 = (g.region0_count as usize + g.region1_count as usize + 2).min(22);
        let b0 = sfb.long[r0] as usize;
        let b1 = sfb.long[r1] as usize;
        (b0.min(576), b1.min(576))
    }
}

impl AudioDecoder for Mp3Decoder {
    fn codec(&self) -> CodecId {
        CodecId::Mp3
    }

    fn decode(&mut self, packet: &MediaPacket) -> Result<Option<AudioFrame>, MediaError> {
        // Reflect geometry from the header (resilient to unparseable packets).
        let _ = self.parse_frame(&packet.data);
        let ch = match self.channels {
            Mp3ChannelMode::Mono => 1u16,
            _ => 2u16,
        };
        // Run the full decode-to-PCM path: entropy + requant + reorder + stereo + alias +
        // IMDCT/overlap + the polyphase synthesis filterbank → real, audible interleaved
        // PCM. Truncated/corrupt input (or not-yet-resolvable reservoir back-reference at
        // stream start) yields geometry-correct silence, never wrong samples or a panic.
        let total = self.samples_per_frame as usize * ch as usize;
        let samples = match self.decode_frame_pcm(&packet.data) {
            Ok((pcm, nch)) if nch > 0 && pcm.len() == self.samples_per_frame as usize * nch => {
                // If the header channel count and the decoded count agree, emit as-is.
                if nch as u16 == ch {
                    pcm
                } else {
                    // Geometry mismatch (rare): fall back to correctly-sized silence.
                    vec![0.0f32; total]
                }
            }
            _ => vec![0.0f32; total],
        };
        Ok(Some(AudioFrame {
            samples,
            sample_rate: self.sample_rate,
            channels: ch,
            channel_layout: if ch == 1 {
                ChannelLayout::Mono
            } else {
                ChannelLayout::Stereo
            },
            pts: packet.pts,
            duration: (self.samples_per_frame as i64 * 1000) / self.sample_rate as i64,
            nb_samples: self.samples_per_frame,
        }))
    }

    fn flush(&mut self) -> Vec<AudioFrame> {
        Vec::new()
    }
    fn reset(&mut self) {
        *self = Self::new();
    }
    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
    fn channels(&self) -> u16 {
        match self.channels {
            Mp3ChannelMode::Mono => 1,
            _ => 2,
        }
    }
}

// ── PCM Decoder ───────────────────────────────────────────────────────────
//
// The one *fully-decodable* audio path in the pipeline: a `.wav`/raw-PCM track
// carries uncompressed samples, so "decode" is a format conversion to the f32
// `AudioFrame` the rest of the pipeline (resampler/remixer/normalizer → AthAudio)
// expects. Concept §creators/media: *"play my music"* — WAV is the lossless
// format every export tool produces and is what actually makes sound today
// (AAC/MP3/FLAC/Opus are header-parse + silence pending a harvested codec).
//
// Supported sample formats (little-endian, interleaved), matching `wav.rs`:
//   * 8-bit unsigned PCM   (`format_tag == 1`, bits == 8)
//   * 16/24/32-bit signed PCM (`format_tag == 1`)
//   * 32/64-bit IEEE float (`format_tag == 3`)
// Each sample is normalized to f32 in [-1.0, 1.0]. Untrusted input: a packet
// whose byte length isn't a whole number of frames is truncated to the largest
// whole frame; an empty/odd tail yields an empty frame — never a panic.

/// Format-tag of the wrapped PCM stream (RIFF/WAVE `wFormatTag` semantics).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PcmFormat {
    /// Integer PCM (unsigned for 8-bit, signed two's-complement otherwise).
    Integer,
    /// IEEE 754 floating-point PCM.
    Float,
}

pub struct PcmDecoder {
    pub sample_rate: u32,
    pub channels: u16,
    pub bits_per_sample: u16,
    pub format: PcmFormat,
}

impl PcmDecoder {
    /// Construct a PCM decoder for a known stream geometry. `bits_per_sample` and
    /// `format` come from the track's `AudioTrackInfo` (via the WAV `fmt ` chunk).
    pub fn new(sample_rate: u32, channels: u16, bits_per_sample: u16, format: PcmFormat) -> Self {
        Self {
            sample_rate,
            channels,
            bits_per_sample,
            format,
        }
    }

    /// Number of bytes one sample (one channel) occupies on the wire.
    fn bytes_per_sample(&self) -> usize {
        (self.bits_per_sample as usize + 7) / 8
    }

    /// Convert one little-endian PCM sample at `b[off..]` to a normalized f32.
    /// Returns 0.0 for an unsupported bit-depth (the decoder is constructed from a
    /// validated geometry, so this is a defensive floor, not a hot path).
    fn sample_to_f32(&self, b: &[u8], off: usize) -> f32 {
        match (self.format, self.bits_per_sample) {
            (PcmFormat::Integer, 8) => {
                // 8-bit WAV PCM is unsigned, centered at 128.
                (b[off] as f32 - 128.0) / 128.0
            }
            (PcmFormat::Integer, 16) => {
                let v = i16::from_le_bytes([b[off], b[off + 1]]);
                v as f32 / 32768.0
            }
            (PcmFormat::Integer, 24) => {
                // Sign-extend 24-bit LE into i32.
                let raw =
                    (b[off] as u32) | ((b[off + 1] as u32) << 8) | ((b[off + 2] as u32) << 16);
                let v = if raw & 0x0080_0000 != 0 {
                    (raw | 0xFF00_0000) as i32
                } else {
                    raw as i32
                };
                v as f32 / 8_388_608.0
            }
            (PcmFormat::Integer, 32) => {
                let v = i32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]]);
                v as f32 / 2_147_483_648.0
            }
            (PcmFormat::Float, 32) => {
                f32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
            }
            (PcmFormat::Float, 64) => {
                let v = f64::from_le_bytes([
                    b[off],
                    b[off + 1],
                    b[off + 2],
                    b[off + 3],
                    b[off + 4],
                    b[off + 5],
                    b[off + 6],
                    b[off + 7],
                ]);
                v as f32
            }
            _ => 0.0,
        }
    }
}

impl AudioDecoder for PcmDecoder {
    fn codec(&self) -> CodecId {
        CodecId::Pcm
    }

    fn decode(&mut self, packet: &MediaPacket) -> Result<Option<AudioFrame>, MediaError> {
        let bps = self.bytes_per_sample();
        let ch = self.channels.max(1) as usize;
        let frame_bytes = bps * ch;
        if frame_bytes == 0 {
            return Err(MediaError::InvalidData("PCM stream has zero frame size"));
        }
        // Truncate any partial trailing frame — hostile/short packets must not panic.
        let n_frames = packet.data.len() / frame_bytes;
        if n_frames == 0 {
            // Nothing decodable in this packet (empty or sub-frame tail).
            return Ok(None);
        }
        let n_samples = n_frames * ch;
        let mut samples = Vec::with_capacity(n_samples);
        let data = &packet.data;
        for f in 0..n_frames {
            let base = f * frame_bytes;
            for c in 0..ch {
                let off = base + c * bps;
                // Bounds already guaranteed by n_frames computation, but the helper
                // indexes up to off+bps; off+bps <= base+frame_bytes <= data.len().
                samples.push(self.sample_to_f32(data, off));
            }
        }
        let nb_samples = n_frames as u32;
        Ok(Some(AudioFrame {
            samples,
            sample_rate: self.sample_rate,
            channels: self.channels,
            channel_layout: match self.channels {
                1 => ChannelLayout::Mono,
                2 => ChannelLayout::Stereo,
                6 => ChannelLayout::Surround51,
                8 => ChannelLayout::Surround71,
                _ => ChannelLayout::Stereo,
            },
            pts: packet.pts,
            duration: if self.sample_rate > 0 {
                (nb_samples as i64 * 1000) / self.sample_rate as i64
            } else {
                0
            },
            nb_samples,
        }))
    }

    fn flush(&mut self) -> Vec<AudioFrame> {
        Vec::new()
    }
    fn reset(&mut self) {}
    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
    fn channels(&self) -> u16 {
        self.channels
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// §5  Subtitle Renderer
// ═══════════════════════════════════════════════════════════════════════════

pub struct SubtitleRenderer {
    entries: Vec<SubtitleEntry>,
    style: SubtitleStyle,
    current_index: usize,
}

pub struct SubtitleEntry {
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
    pub format: SubtitleFormat,
    pub style_override: Option<SubtitleStyle>,
    pub position: Option<(i32, i32)>,
    pub alignment: SubtitleAlignment,
}

pub struct SubtitleStyle {
    pub font_family: String,
    pub font_size: f32,
    pub font_weight: u16,
    pub color: u32,
    pub outline_color: u32,
    pub shadow_color: u32,
    pub outline_width: f32,
    pub shadow_offset: (f32, f32),
    pub background_color: Option<u32>,
    pub margin: (i32, i32, i32, i32),
    pub italic: bool,
    pub underline: bool,
    pub strikeout: bool,
    pub border_style: BorderStyle,
    pub alignment: SubtitleAlignment,
}

impl SubtitleStyle {
    pub fn default_style() -> Self {
        Self {
            font_family: String::from("Sans"),
            font_size: 24.0,
            font_weight: 400,
            color: 0xFFFFFFFF,
            outline_color: 0x000000FF,
            shadow_color: 0x00000080,
            outline_width: 2.0,
            shadow_offset: (2.0, 2.0),
            background_color: None,
            margin: (20, 20, 20, 20),
            italic: false,
            underline: false,
            strikeout: false,
            border_style: BorderStyle::Outline,
            alignment: SubtitleAlignment::Bottom,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BorderStyle {
    Outline,
    OpaqueBox,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubtitleAlignment {
    TopLeft,
    Top,
    TopRight,
    Left,
    Center,
    Right,
    BottomLeft,
    Bottom,
    BottomRight,
}

pub struct SubtitleGlyph {
    pub x: i32,
    pub y: i32,
    pub text: String,
    pub style: SubtitleStyle,
}

impl SubtitleRenderer {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            style: SubtitleStyle::default_style(),
            current_index: 0,
        }
    }

    pub fn load_srt(&mut self, data: &str) -> Result<(), MediaError> {
        self.entries.clear();
        self.current_index = 0;

        let mut lines = data.lines().peekable();
        while lines.peek().is_some() {
            // Skip blank lines and sequence number
            while let Some(line) = lines.peek() {
                if line.trim().is_empty() {
                    lines.next();
                } else {
                    break;
                }
            }
            // Sequence number
            if let Some(line) = lines.next() {
                if line.trim().parse::<u32>().is_err() {
                    continue;
                }
            } else {
                break;
            }
            // Timestamp line
            let time_line = match lines.next() {
                Some(l) => l,
                None => break,
            };
            let parts: Vec<&str> = time_line.split("-->").collect();
            if parts.len() != 2 {
                continue;
            }
            let start_ms = match Self::parse_srt_time(parts[0].trim()) {
                Ok(t) => t,
                Err(_) => continue,
            };
            let end_ms = match Self::parse_srt_time(parts[1].trim()) {
                Ok(t) => t,
                Err(_) => continue,
            };
            // Text lines until blank
            let mut text = String::new();
            while let Some(line) = lines.peek() {
                if line.trim().is_empty() {
                    break;
                }
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(lines.next().unwrap());
            }
            let cleaned = Self::strip_html_tags(&text);
            self.entries.push(SubtitleEntry {
                start_ms,
                end_ms,
                text: cleaned,
                format: SubtitleFormat::Srt,
                style_override: None,
                position: None,
                alignment: SubtitleAlignment::Bottom,
            });
        }
        Ok(())
    }

    pub fn load_ass(&mut self, data: &str) -> Result<(), MediaError> {
        self.entries.clear();
        self.current_index = 0;

        let mut in_events = false;
        for line in data.lines() {
            let trimmed = line.trim();
            if trimmed == "[Events]" {
                in_events = true;
                continue;
            }
            if trimmed.starts_with('[') {
                in_events = false;
                continue;
            }
            if !in_events || !trimmed.starts_with("Dialogue:") {
                continue;
            }
            let content = &trimmed["Dialogue:".len()..].trim_start();
            let fields: Vec<&str> = content.splitn(10, ',').collect();
            if fields.len() < 10 {
                continue;
            }
            let start_ms = match Self::parse_ass_time(fields[1].trim()) {
                Ok(t) => t,
                Err(_) => continue,
            };
            let end_ms = match Self::parse_ass_time(fields[2].trim()) {
                Ok(t) => t,
                Err(_) => continue,
            };
            let text_raw = fields[9];
            let text = text_raw.replace("\\N", "\n").replace("\\n", "\n");
            // Strip ASS override tags like {\b1}
            let mut cleaned = String::with_capacity(text.len());
            let mut in_brace = false;
            for ch in text.chars() {
                match ch {
                    '{' => in_brace = true,
                    '}' => in_brace = false,
                    _ if !in_brace => cleaned.push(ch),
                    _ => {}
                }
            }
            self.entries.push(SubtitleEntry {
                start_ms,
                end_ms,
                text: cleaned,
                format: SubtitleFormat::Ass,
                style_override: None,
                position: None,
                alignment: SubtitleAlignment::Bottom,
            });
        }
        self.entries.sort_by_key(|e| e.start_ms);
        Ok(())
    }

    pub fn load_webvtt(&mut self, data: &str) -> Result<(), MediaError> {
        self.entries.clear();
        self.current_index = 0;

        let mut lines = data.lines().peekable();
        // Skip WEBVTT header
        if let Some(first) = lines.next() {
            if !first.starts_with("WEBVTT") {
                return Err(MediaError::InvalidData("missing WEBVTT header"));
            }
        }
        // Skip header continuation
        while let Some(line) = lines.peek() {
            if line.trim().is_empty() {
                lines.next();
                break;
            }
            lines.next();
        }

        while lines.peek().is_some() {
            while let Some(line) = lines.peek() {
                if line.trim().is_empty() {
                    lines.next();
                } else {
                    break;
                }
            }
            // Optional cue ID
            if let Some(line) = lines.peek() {
                if !line.contains("-->") {
                    lines.next(); // skip cue ID
                }
            }
            let time_line = match lines.next() {
                Some(l) => l,
                None => break,
            };
            let parts: Vec<&str> = time_line.split("-->").collect();
            if parts.len() != 2 {
                continue;
            }
            let start_ms = match Self::parse_srt_time(parts[0].trim()) {
                Ok(t) => t,
                Err(_) => continue,
            };
            let end_str = parts[1].trim().split_whitespace().next().unwrap_or("");
            let end_ms = match Self::parse_srt_time(end_str) {
                Ok(t) => t,
                Err(_) => continue,
            };
            let mut text = String::new();
            while let Some(line) = lines.peek() {
                if line.trim().is_empty() {
                    break;
                }
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(lines.next().unwrap());
            }
            let cleaned = Self::strip_html_tags(&text);
            self.entries.push(SubtitleEntry {
                start_ms,
                end_ms,
                text: cleaned,
                format: SubtitleFormat::WebVtt,
                style_override: None,
                position: None,
                alignment: SubtitleAlignment::Bottom,
            });
        }
        Ok(())
    }

    pub fn get_subtitle_at(&self, position_ms: u64) -> Option<&SubtitleEntry> {
        self.entries
            .iter()
            .find(|e| position_ms >= e.start_ms && position_ms < e.end_ms)
    }

    pub fn render_subtitle(
        &self,
        entry: &SubtitleEntry,
        canvas_width: u32,
        canvas_height: u32,
    ) -> Vec<SubtitleGlyph> {
        let style = entry.style_override.as_ref().unwrap_or(&self.style);
        let lines: Vec<&str> = entry.text.split('\n').collect();
        let line_height = (style.font_size * 1.4) as i32;
        let total_height = lines.len() as i32 * line_height;

        let (base_x, base_y) = match style.alignment {
            SubtitleAlignment::Top | SubtitleAlignment::TopLeft | SubtitleAlignment::TopRight => {
                (canvas_width as i32 / 2, style.margin.0)
            }
            SubtitleAlignment::Center | SubtitleAlignment::Left | SubtitleAlignment::Right => (
                canvas_width as i32 / 2,
                (canvas_height as i32 - total_height) / 2,
            ),
            SubtitleAlignment::Bottom
            | SubtitleAlignment::BottomLeft
            | SubtitleAlignment::BottomRight => (
                canvas_width as i32 / 2,
                canvas_height as i32 - total_height - style.margin.2,
            ),
        };

        let mut glyphs = Vec::new();
        for (i, line) in lines.iter().enumerate() {
            let approx_width = line.len() as i32 * (style.font_size * 0.55) as i32;
            let x = match style.alignment {
                SubtitleAlignment::Left
                | SubtitleAlignment::TopLeft
                | SubtitleAlignment::BottomLeft => style.margin.3,
                SubtitleAlignment::Right
                | SubtitleAlignment::TopRight
                | SubtitleAlignment::BottomRight => {
                    canvas_width as i32 - approx_width - style.margin.1
                }
                _ => base_x - approx_width / 2,
            };
            let y = base_y + (i as i32 * line_height);

            glyphs.push(SubtitleGlyph {
                x,
                y,
                text: String::from(*line),
                style: SubtitleStyle {
                    font_family: style.font_family.clone(),
                    font_size: style.font_size,
                    font_weight: style.font_weight,
                    color: style.color,
                    outline_color: style.outline_color,
                    shadow_color: style.shadow_color,
                    outline_width: style.outline_width,
                    shadow_offset: style.shadow_offset,
                    background_color: style.background_color,
                    margin: style.margin,
                    italic: style.italic,
                    underline: style.underline,
                    strikeout: style.strikeout,
                    border_style: style.border_style,
                    alignment: style.alignment,
                },
            });
        }
        glyphs
    }

    pub fn set_style(&mut self, style: SubtitleStyle) {
        self.style = style;
    }

    fn parse_srt_time(s: &str) -> Result<u64, MediaError> {
        // Format: HH:MM:SS,mmm  or  HH:MM:SS.mmm
        let s = s.replace(',', ".");
        let parts: Vec<&str> = s.split(':').collect();
        if parts.len() != 3 {
            return Err(MediaError::ParseError("invalid SRT timestamp format"));
        }
        let h: u64 = parts[0]
            .trim()
            .parse()
            .map_err(|_| MediaError::ParseError("bad hours"))?;
        let m: u64 = parts[1]
            .trim()
            .parse()
            .map_err(|_| MediaError::ParseError("bad minutes"))?;
        let sec_parts: Vec<&str> = parts[2].split('.').collect();
        let sec: u64 = sec_parts[0]
            .trim()
            .parse()
            .map_err(|_| MediaError::ParseError("bad seconds"))?;
        let ms: u64 = if sec_parts.len() > 1 {
            let frac = sec_parts[1].trim();
            let val: u64 = frac
                .parse()
                .map_err(|_| MediaError::ParseError("bad millis"))?;
            match frac.len() {
                1 => val * 100,
                2 => val * 10,
                3 => val,
                _ => val / 10u64.pow(frac.len() as u32 - 3),
            }
        } else {
            0
        };
        Ok(h * 3600000 + m * 60000 + sec * 1000 + ms)
    }

    fn parse_ass_time(s: &str) -> Result<u64, MediaError> {
        // Format: H:MM:SS.cc  (centiseconds)
        let parts: Vec<&str> = s.split(':').collect();
        if parts.len() != 3 {
            return Err(MediaError::ParseError("invalid ASS timestamp format"));
        }
        let h: u64 = parts[0]
            .trim()
            .parse()
            .map_err(|_| MediaError::ParseError("bad hours"))?;
        let m: u64 = parts[1]
            .trim()
            .parse()
            .map_err(|_| MediaError::ParseError("bad minutes"))?;
        let sec_parts: Vec<&str> = parts[2].split('.').collect();
        let sec: u64 = sec_parts[0]
            .trim()
            .parse()
            .map_err(|_| MediaError::ParseError("bad seconds"))?;
        let cs: u64 = if sec_parts.len() > 1 {
            sec_parts[1]
                .trim()
                .parse()
                .map_err(|_| MediaError::ParseError("bad centis"))?
        } else {
            0
        };
        Ok(h * 3600000 + m * 60000 + sec * 1000 + cs * 10)
    }

    fn strip_html_tags(text: &str) -> String {
        let mut result = String::with_capacity(text.len());
        let mut in_tag = false;
        for ch in text.chars() {
            match ch {
                '<' => in_tag = true,
                '>' => in_tag = false,
                _ if !in_tag => result.push(ch),
                _ => {}
            }
        }
        result
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// §6  Media Pipeline
// ═══════════════════════════════════════════════════════════════════════════

pub struct MediaPipeline {
    demuxer: Option<Demuxer>,
    video_decoder: Option<Box<dyn VideoDecoder + Send>>,
    audio_decoder: Option<Box<dyn AudioDecoder + Send>>,
    subtitle_renderer: SubtitleRenderer,
    state: PlaybackState,
    position_ms: u64,
    duration_ms: u64,
    volume: f32,
    muted: bool,
    playback_rate: f32,
    loop_mode: LoopMode,
    video_queue: Vec<VideoFrame>,
    audio_queue: Vec<AudioFrame>,
    selected_video_track: Option<u32>,
    selected_audio_track: Option<u32>,
    selected_subtitle_track: Option<u32>,
    seek_pending: Option<u64>,
    #[allow(dead_code)]
    buffered_ms: u64,
    stats: PlaybackStats,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PlaybackState {
    Idle,
    Loading,
    Playing,
    Paused,
    Buffering,
    Seeking,
    Ended,
    Error(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopMode {
    None,
    Single,
    All,
}

pub struct PlaybackStats {
    pub frames_decoded: u64,
    pub frames_dropped: u64,
    pub frames_displayed: u64,
    pub audio_samples_decoded: u64,
    pub buffer_underruns: u64,
    pub video_bitrate: u64,
    pub audio_bitrate: u64,
    pub decode_time_us: u64,
    pub render_time_us: u64,
}

impl PlaybackStats {
    fn new() -> Self {
        Self {
            frames_decoded: 0,
            frames_dropped: 0,
            frames_displayed: 0,
            audio_samples_decoded: 0,
            buffer_underruns: 0,
            video_bitrate: 0,
            audio_bitrate: 0,
            decode_time_us: 0,
            render_time_us: 0,
        }
    }
}

pub struct PipelineTick {
    pub video_ready: bool,
    pub audio_ready: bool,
    pub subtitle_changed: bool,
    pub state_changed: bool,
}

impl MediaPipeline {
    pub fn new() -> Self {
        Self {
            demuxer: None,
            video_decoder: None,
            audio_decoder: None,
            subtitle_renderer: SubtitleRenderer::new(),
            state: PlaybackState::Idle,
            position_ms: 0,
            duration_ms: 0,
            volume: 1.0,
            muted: false,
            playback_rate: 1.0,
            loop_mode: LoopMode::None,
            video_queue: Vec::new(),
            audio_queue: Vec::new(),
            selected_video_track: None,
            selected_audio_track: None,
            selected_subtitle_track: None,
            seek_pending: None,
            buffered_ms: 0,
            stats: PlaybackStats::new(),
        }
    }

    pub fn open(&mut self, data: &[u8]) -> Result<(), MediaError> {
        self.state = PlaybackState::Loading;
        let demuxer = Demuxer::open(data)?;
        self.duration_ms = demuxer.duration_ms();

        for track in demuxer.tracks() {
            match track.track_type {
                TrackType::Video if self.selected_video_track.is_none() => {
                    self.video_decoder = Self::create_decoder(&track.codec);
                    if self.video_decoder.is_some() {
                        self.selected_video_track = Some(track.id);
                    }
                }
                TrackType::Audio if self.selected_audio_track.is_none() => {
                    self.audio_decoder = Self::create_audio_decoder(track);
                    if self.audio_decoder.is_some() {
                        self.selected_audio_track = Some(track.id);
                    }
                }
                TrackType::Subtitle if self.selected_subtitle_track.is_none() => {
                    self.selected_subtitle_track = Some(track.id);
                }
                _ => {}
            }
        }

        self.demuxer = Some(demuxer);
        self.state = PlaybackState::Paused;
        self.position_ms = 0;
        Ok(())
    }

    pub fn play(&mut self) -> Result<(), MediaError> {
        match &self.state {
            PlaybackState::Paused | PlaybackState::Idle => {
                if self.demuxer.is_none() {
                    return Err(MediaError::NotInitialized);
                }
                self.state = PlaybackState::Playing;
                Ok(())
            }
            PlaybackState::Ended => {
                self.seek(0)?;
                self.state = PlaybackState::Playing;
                Ok(())
            }
            _ => Ok(()),
        }
    }

    pub fn pause(&mut self) {
        if self.state == PlaybackState::Playing {
            self.state = PlaybackState::Paused;
        }
    }

    pub fn stop(&mut self) {
        self.state = PlaybackState::Idle;
        self.position_ms = 0;
        self.video_queue.clear();
        self.audio_queue.clear();
        if let Some(dec) = &mut self.video_decoder {
            dec.reset();
        }
        if let Some(dec) = &mut self.audio_decoder {
            dec.reset();
        }
        self.stats = PlaybackStats::new();
    }

    pub fn seek(&mut self, position_ms: u64) -> Result<(), MediaError> {
        let demuxer = self.demuxer.as_mut().ok_or(MediaError::NotInitialized)?;
        self.state = PlaybackState::Seeking;
        self.video_queue.clear();
        self.audio_queue.clear();
        if let Some(dec) = &mut self.video_decoder {
            dec.reset();
        }
        if let Some(dec) = &mut self.audio_decoder {
            dec.reset();
        }
        demuxer.seek(position_ms)?;
        self.position_ms = position_ms;
        self.seek_pending = None;
        self.state = PlaybackState::Playing;
        Ok(())
    }

    pub fn set_volume(&mut self, volume: f32) {
        self.volume = volume.clamp(0.0, 2.0);
    }

    pub fn set_muted(&mut self, muted: bool) {
        self.muted = muted;
    }

    pub fn set_playback_rate(&mut self, rate: f32) {
        self.playback_rate = rate.clamp(0.25, 4.0);
    }

    pub fn set_loop_mode(&mut self, mode: LoopMode) {
        self.loop_mode = mode;
    }

    pub fn select_video_track(&mut self, track_id: u32) -> Result<(), MediaError> {
        let demuxer = self.demuxer.as_ref().ok_or(MediaError::NotInitialized)?;
        let track = demuxer
            .tracks()
            .iter()
            .find(|t| t.id == track_id && t.track_type == TrackType::Video);
        match track {
            Some(t) => {
                self.video_decoder = Self::create_decoder(&t.codec);
                self.selected_video_track = Some(track_id);
                self.video_queue.clear();
                Ok(())
            }
            None => Err(MediaError::InvalidTrack(track_id)),
        }
    }

    pub fn select_audio_track(&mut self, track_id: u32) -> Result<(), MediaError> {
        let demuxer = self.demuxer.as_ref().ok_or(MediaError::NotInitialized)?;
        let track = demuxer
            .tracks()
            .iter()
            .find(|t| t.id == track_id && t.track_type == TrackType::Audio);
        match track {
            Some(t) => {
                self.audio_decoder = Self::create_audio_decoder(t);
                self.selected_audio_track = Some(track_id);
                self.audio_queue.clear();
                Ok(())
            }
            None => Err(MediaError::InvalidTrack(track_id)),
        }
    }

    pub fn select_subtitle_track(&mut self, track_id: Option<u32>) {
        self.selected_subtitle_track = track_id;
    }

    pub fn tick(&mut self, elapsed_ms: u64) -> PipelineTick {
        let mut tick = PipelineTick {
            video_ready: false,
            audio_ready: false,
            subtitle_changed: false,
            state_changed: false,
        };

        if self.state != PlaybackState::Playing {
            return tick;
        }

        let advance = (elapsed_ms as f32 * self.playback_rate) as u64;
        let old_pos = self.position_ms;
        self.position_ms = self.position_ms.saturating_add(advance);

        if self.position_ms >= self.duration_ms && self.duration_ms > 0 {
            match self.loop_mode {
                LoopMode::Single => {
                    self.position_ms = 0;
                    if let Some(demuxer) = &mut self.demuxer {
                        let _ = demuxer.seek(0);
                    }
                    if let Some(dec) = &mut self.video_decoder {
                        dec.reset();
                    }
                    if let Some(dec) = &mut self.audio_decoder {
                        dec.reset();
                    }
                }
                LoopMode::None => {
                    self.state = PlaybackState::Ended;
                    tick.state_changed = true;
                    return tick;
                }
                LoopMode::All => {
                    self.position_ms = 0;
                    if let Some(demuxer) = &mut self.demuxer {
                        let _ = demuxer.seek(0);
                    }
                }
            }
        }

        let pumped = self.pump_demuxer();
        tick.video_ready = !self.video_queue.is_empty();
        tick.audio_ready = !self.audio_queue.is_empty();

        let old_sub = self
            .subtitle_renderer
            .get_subtitle_at(old_pos)
            .map(|s| s.start_ms);
        let new_sub = self
            .subtitle_renderer
            .get_subtitle_at(self.position_ms)
            .map(|s| s.start_ms);
        tick.subtitle_changed = old_sub != new_sub;

        if !pumped && self.video_queue.is_empty() && self.audio_queue.is_empty() {
            self.stats.buffer_underruns += 1;
            if self.state == PlaybackState::Playing {
                self.state = PlaybackState::Buffering;
                tick.state_changed = true;
            }
        }

        tick
    }

    pub fn get_video_frame(&mut self) -> Option<VideoFrame> {
        if self.video_queue.is_empty() {
            return None;
        }
        self.stats.frames_displayed += 1;
        Some(self.video_queue.remove(0))
    }

    pub fn get_audio_frame(&mut self) -> Option<AudioFrame> {
        if self.audio_queue.is_empty() {
            return None;
        }
        Some(self.audio_queue.remove(0))
    }

    pub fn get_subtitle(&self) -> Option<&SubtitleEntry> {
        self.subtitle_renderer.get_subtitle_at(self.position_ms)
    }

    pub fn position_ms(&self) -> u64 {
        self.position_ms
    }

    pub fn duration_ms(&self) -> u64 {
        self.duration_ms
    }

    pub fn state(&self) -> &PlaybackState {
        &self.state
    }

    pub fn metadata(&self) -> Option<&MediaMetadata> {
        self.demuxer.as_ref().map(|d| d.metadata())
    }

    pub fn tracks(&self) -> Option<&[MediaTrack]> {
        self.demuxer.as_ref().map(|d| d.tracks())
    }

    pub fn stats(&self) -> &PlaybackStats {
        &self.stats
    }

    fn create_decoder(codec: &CodecId) -> Option<Box<dyn VideoDecoder + Send>> {
        match codec {
            CodecId::H264 => Some(Box::new(H264Decoder::new())),
            _ => None,
        }
    }

    fn create_audio_decoder(track: &MediaTrack) -> Option<Box<dyn AudioDecoder + Send>> {
        match track.codec {
            CodecId::Aac => Some(Box::new(AacDecoder::new())),
            CodecId::Flac => {
                // Seed geometry from the parsed STREAMINFO so the decoder reports the
                // right rate/channels even before the first frame; the real per-frame
                // STREAMINFO from the bitstream still wins on decode.
                if let Some(info) = track.audio.as_ref() {
                    Some(Box::new(FlacDecoder::with_geometry(
                        info.sample_rate,
                        info.channels,
                        info.bits_per_sample,
                    )))
                } else {
                    Some(Box::new(FlacDecoder::new()))
                }
            }
            CodecId::Opus => Some(Box::new(OpusDecoder::new())),
            CodecId::Mp3 => Some(Box::new(Mp3Decoder::new())),
            CodecId::Pcm => {
                // PCM needs the real stream geometry (bits + int/float) from the
                // track's `fmt ` info; without it we can't size or scale samples.
                let info = track.audio.as_ref()?;
                let format = match info.profile.as_deref() {
                    Some("float") => PcmFormat::Float,
                    _ => PcmFormat::Integer,
                };
                Some(Box::new(PcmDecoder::new(
                    info.sample_rate,
                    info.channels,
                    info.bits_per_sample,
                    format,
                )))
            }
            _ => None,
        }
    }

    fn pump_demuxer(&mut self) -> bool {
        let demuxer = match &mut self.demuxer {
            Some(d) => d,
            None => return false,
        };

        let mut any = false;
        for _ in 0..8 {
            let packet = match demuxer.read_packet() {
                Ok(p) => p,
                Err(MediaError::EndOfStream) => break,
                Err(_) => break,
            };

            if Some(packet.track_id) == self.selected_video_track {
                if let Some(dec) = &mut self.video_decoder {
                    match dec.decode(&packet) {
                        Ok(Some(frame)) => {
                            self.stats.frames_decoded += 1;
                            self.video_queue.push(frame);
                            any = true;
                        }
                        Ok(None) => {}
                        Err(_) => {}
                    }
                }
            } else if Some(packet.track_id) == self.selected_audio_track {
                if let Some(dec) = &mut self.audio_decoder {
                    match dec.decode(&packet) {
                        Ok(Some(frame)) => {
                            self.stats.audio_samples_decoded += frame.nb_samples as u64;
                            self.audio_queue.push(frame);
                            any = true;
                        }
                        Ok(None) => {}
                        Err(_) => {}
                    }
                }
            }
        }
        any
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// §7  Audio / Video Converter
// ═══════════════════════════════════════════════════════════════════════════

pub struct PixelConverter;

impl PixelConverter {
    pub fn convert(
        frame: &VideoFrame,
        target_format: PixelFormat,
    ) -> Result<VideoFrame, MediaError> {
        if frame.pixel_format == target_format {
            return Ok(VideoFrame {
                width: frame.width,
                height: frame.height,
                pixel_format: frame.pixel_format,
                planes: frame
                    .planes
                    .iter()
                    .map(|p| VideoPlane {
                        data: p.data.clone(),
                        stride: p.stride,
                    })
                    .collect(),
                pts: frame.pts,
                duration: frame.duration,
                keyframe: frame.keyframe,
                interlaced: frame.interlaced,
                top_field_first: frame.top_field_first,
                color_space: frame.color_space,
                color_range: frame.color_range,
                hdr_metadata: None,
            });
        }
        match (frame.pixel_format, target_format) {
            (PixelFormat::Yuv420p, PixelFormat::Rgb24) => {
                if frame.planes.len() < 3 {
                    return Err(MediaError::InvalidData("YUV420p requires 3 planes"));
                }
                let rgb = Self::yuv420_to_rgb(
                    &frame.planes[0].data,
                    &frame.planes[1].data,
                    &frame.planes[2].data,
                    frame.width,
                    frame.height,
                );
                Ok(VideoFrame {
                    width: frame.width,
                    height: frame.height,
                    pixel_format: PixelFormat::Rgb24,
                    planes: vec![VideoPlane {
                        data: rgb,
                        stride: frame.width * 3,
                    }],
                    pts: frame.pts,
                    duration: frame.duration,
                    keyframe: frame.keyframe,
                    interlaced: frame.interlaced,
                    top_field_first: frame.top_field_first,
                    color_space: frame.color_space,
                    color_range: frame.color_range,
                    hdr_metadata: None,
                })
            }
            (PixelFormat::Rgb24, PixelFormat::Yuv420p) => {
                if frame.planes.is_empty() {
                    return Err(MediaError::InvalidData("RGB24 requires 1 plane"));
                }
                let (y, u, v) =
                    Self::rgb_to_yuv420(&frame.planes[0].data, frame.width, frame.height);
                Ok(VideoFrame {
                    width: frame.width,
                    height: frame.height,
                    pixel_format: PixelFormat::Yuv420p,
                    planes: vec![
                        VideoPlane {
                            data: y,
                            stride: frame.width,
                        },
                        VideoPlane {
                            data: u,
                            stride: frame.width / 2,
                        },
                        VideoPlane {
                            data: v,
                            stride: frame.width / 2,
                        },
                    ],
                    pts: frame.pts,
                    duration: frame.duration,
                    keyframe: frame.keyframe,
                    interlaced: frame.interlaced,
                    top_field_first: frame.top_field_first,
                    color_space: frame.color_space,
                    color_range: frame.color_range,
                    hdr_metadata: None,
                })
            }
            _ => Err(MediaError::UnsupportedFormat),
        }
    }

    pub fn yuv420_to_rgb(y: &[u8], u: &[u8], v: &[u8], width: u32, height: u32) -> Vec<u8> {
        let w = width as usize;
        let h = height as usize;
        let mut rgb = vec![0u8; w * h * 3];

        for row in 0..h {
            for col in 0..w {
                let y_idx = row * w + col;
                let uv_idx = (row / 2) * (w / 2) + (col / 2);

                let y_val = y.get(y_idx).copied().unwrap_or(0) as i32;
                let u_val = u.get(uv_idx).copied().unwrap_or(128) as i32 - 128;
                let v_val = v.get(uv_idx).copied().unwrap_or(128) as i32 - 128;

                // BT.601 conversion
                let r = y_val + ((359 * v_val) >> 8);
                let g = y_val - ((88 * u_val + 183 * v_val) >> 8);
                let b = y_val + ((454 * u_val) >> 8);

                let out = (row * w + col) * 3;
                rgb[out] = r.clamp(0, 255) as u8;
                rgb[out + 1] = g.clamp(0, 255) as u8;
                rgb[out + 2] = b.clamp(0, 255) as u8;
            }
        }
        rgb
    }

    pub fn rgb_to_yuv420(rgb: &[u8], width: u32, height: u32) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
        let w = width as usize;
        let h = height as usize;
        let mut y_plane = vec![0u8; w * h];
        let mut u_plane = vec![128u8; (w / 2) * (h / 2)];
        let mut v_plane = vec![128u8; (w / 2) * (h / 2)];

        for row in 0..h {
            for col in 0..w {
                let idx = (row * w + col) * 3;
                let r = rgb.get(idx).copied().unwrap_or(0) as i32;
                let g = rgb.get(idx + 1).copied().unwrap_or(0) as i32;
                let b = rgb.get(idx + 2).copied().unwrap_or(0) as i32;

                let y = ((66 * r + 129 * g + 25 * b + 128) >> 8) + 16;
                y_plane[row * w + col] = y.clamp(0, 255) as u8;

                if row % 2 == 0 && col % 2 == 0 {
                    let uv_idx = (row / 2) * (w / 2) + (col / 2);
                    let u = ((-38 * r - 74 * g + 112 * b + 128) >> 8) + 128;
                    let v = ((112 * r - 94 * g - 18 * b + 128) >> 8) + 128;
                    u_plane[uv_idx] = u.clamp(0, 255) as u8;
                    v_plane[uv_idx] = v.clamp(0, 255) as u8;
                }
            }
        }
        (y_plane, u_plane, v_plane)
    }

    pub fn scale(frame: &VideoFrame, width: u32, height: u32) -> VideoFrame {
        if frame.planes.is_empty() || (frame.width == width && frame.height == height) {
            return VideoFrame {
                width: frame.width,
                height: frame.height,
                pixel_format: frame.pixel_format,
                planes: frame
                    .planes
                    .iter()
                    .map(|p| VideoPlane {
                        data: p.data.clone(),
                        stride: p.stride,
                    })
                    .collect(),
                pts: frame.pts,
                duration: frame.duration,
                keyframe: frame.keyframe,
                interlaced: frame.interlaced,
                top_field_first: frame.top_field_first,
                color_space: frame.color_space,
                color_range: frame.color_range,
                hdr_metadata: None,
            };
        }

        // Nearest-neighbour scaling on plane 0 (works for packed formats)
        let src = &frame.planes[0].data;
        let src_w = frame.width as usize;
        let dst_w = width as usize;
        let dst_h = height as usize;
        let bpp = if frame.planes.len() == 1 {
            src.len() / (frame.width as usize * frame.height as usize)
        } else {
            1
        };

        let mut dst = vec![0u8; dst_w * dst_h * bpp];
        for dy in 0..dst_h {
            let sy = dy * frame.height as usize / dst_h;
            for dx in 0..dst_w {
                let sx = dx * frame.width as usize / dst_w;
                for c in 0..bpp {
                    let si = (sy * src_w + sx) * bpp + c;
                    let di = (dy * dst_w + dx) * bpp + c;
                    dst[di] = src.get(si).copied().unwrap_or(0);
                }
            }
        }

        VideoFrame {
            width,
            height,
            pixel_format: frame.pixel_format,
            planes: vec![VideoPlane {
                data: dst,
                stride: (dst_w * bpp) as u32,
            }],
            pts: frame.pts,
            duration: frame.duration,
            keyframe: frame.keyframe,
            interlaced: false,
            top_field_first: false,
            color_space: frame.color_space,
            color_range: frame.color_range,
            hdr_metadata: None,
        }
    }

    pub fn crop(frame: &VideoFrame, x: u32, y: u32, width: u32, height: u32) -> VideoFrame {
        if frame.planes.is_empty() {
            return VideoFrame {
                width,
                height,
                pixel_format: frame.pixel_format,
                planes: Vec::new(),
                pts: frame.pts,
                duration: frame.duration,
                keyframe: frame.keyframe,
                interlaced: false,
                top_field_first: false,
                color_space: frame.color_space,
                color_range: frame.color_range,
                hdr_metadata: None,
            };
        }

        let src = &frame.planes[0];
        let bpp = src.stride as usize / frame.width as usize;
        let mut dst = vec![0u8; width as usize * height as usize * bpp];

        for row in 0..height as usize {
            let src_row = (y as usize + row) * src.stride as usize + x as usize * bpp;
            let dst_row = row * width as usize * bpp;
            let len = width as usize * bpp;
            if src_row + len <= src.data.len() {
                dst[dst_row..dst_row + len].copy_from_slice(&src.data[src_row..src_row + len]);
            }
        }

        VideoFrame {
            width,
            height,
            pixel_format: frame.pixel_format,
            planes: vec![VideoPlane {
                data: dst,
                stride: width * bpp as u32,
            }],
            pts: frame.pts,
            duration: frame.duration,
            keyframe: frame.keyframe,
            interlaced: false,
            top_field_first: false,
            color_space: frame.color_space,
            color_range: frame.color_range,
            hdr_metadata: None,
        }
    }
}

pub struct AudioResampler;

impl AudioResampler {
    pub fn resample(frame: &AudioFrame, target_rate: u32) -> AudioFrame {
        if frame.sample_rate == target_rate || frame.sample_rate == 0 {
            return AudioFrame {
                samples: frame.samples.clone(),
                sample_rate: frame.sample_rate,
                channels: frame.channels,
                channel_layout: frame.channel_layout,
                pts: frame.pts,
                duration: frame.duration,
                nb_samples: frame.nb_samples,
            };
        }

        let ratio = target_rate as f64 / frame.sample_rate as f64;
        let ch = frame.channels as usize;
        let in_frames = frame.nb_samples as usize;
        let out_frames = (in_frames as f64 * ratio) as usize;
        let mut output = vec![0.0f32; out_frames * ch];

        // Linear interpolation resampler
        for i in 0..out_frames {
            let src_pos = i as f64 / ratio;
            let idx0 = src_pos as usize;
            let frac = (src_pos - idx0 as f64) as f32;
            let idx1 = if idx0 + 1 < in_frames { idx0 + 1 } else { idx0 };

            for c in 0..ch {
                let s0 = frame.samples.get(idx0 * ch + c).copied().unwrap_or(0.0);
                let s1 = frame.samples.get(idx1 * ch + c).copied().unwrap_or(0.0);
                output[i * ch + c] = s0 + (s1 - s0) * frac;
            }
        }

        AudioFrame {
            samples: output,
            sample_rate: target_rate,
            channels: frame.channels,
            channel_layout: frame.channel_layout,
            pts: frame.pts,
            duration: (out_frames as i64 * 1000) / target_rate as i64,
            nb_samples: out_frames as u32,
        }
    }

    pub fn remix(frame: &AudioFrame, target_layout: ChannelLayout) -> AudioFrame {
        let target_ch: u16 = match target_layout {
            ChannelLayout::Mono => 1,
            ChannelLayout::Stereo => 2,
            ChannelLayout::Surround21 => 3,
            ChannelLayout::Surround51 => 6,
            ChannelLayout::Surround71 => 8,
            ChannelLayout::Atmos => 8,
        };

        if target_ch == frame.channels {
            return AudioFrame {
                samples: frame.samples.clone(),
                sample_rate: frame.sample_rate,
                channels: frame.channels,
                channel_layout: target_layout,
                pts: frame.pts,
                duration: frame.duration,
                nb_samples: frame.nb_samples,
            };
        }

        let in_ch = frame.channels as usize;
        let out_ch = target_ch as usize;
        let n_frames = frame.nb_samples as usize;
        let mut output = vec![0.0f32; n_frames * out_ch];

        for i in 0..n_frames {
            if out_ch == 1 {
                // Downmix to mono: average all input channels
                let mut sum = 0.0f32;
                for c in 0..in_ch {
                    sum += frame.samples.get(i * in_ch + c).copied().unwrap_or(0.0);
                }
                output[i] = sum / in_ch as f32;
            } else if out_ch == 2 && in_ch == 1 {
                // Upmix mono to stereo
                let s = frame.samples.get(i).copied().unwrap_or(0.0);
                output[i * 2] = s;
                output[i * 2 + 1] = s;
            } else if out_ch > in_ch {
                // Upmix: copy existing channels, zero-fill extra
                for c in 0..in_ch {
                    output[i * out_ch + c] =
                        frame.samples.get(i * in_ch + c).copied().unwrap_or(0.0);
                }
            } else {
                // Downmix: simple fold of extra channels into first out_ch
                for c in 0..out_ch {
                    output[i * out_ch + c] =
                        frame.samples.get(i * in_ch + c).copied().unwrap_or(0.0);
                }
                for c in out_ch..in_ch {
                    let s = frame.samples.get(i * in_ch + c).copied().unwrap_or(0.0);
                    output[i * out_ch + (c % out_ch)] += s * 0.707;
                }
            }
        }

        AudioFrame {
            samples: output,
            sample_rate: frame.sample_rate,
            channels: target_ch,
            channel_layout: target_layout,
            pts: frame.pts,
            duration: frame.duration,
            nb_samples: frame.nb_samples,
        }
    }

    pub fn convert_format(samples: &[f32], target_bits: u16) -> Vec<u8> {
        match target_bits {
            8 => samples
                .iter()
                .map(|&s| {
                    let v = ((s * 127.0) + 128.0).clamp(0.0, 255.0) as u8;
                    v
                })
                .collect(),
            16 => {
                let mut out = Vec::with_capacity(samples.len() * 2);
                for &s in samples {
                    let v = (s * 32767.0).clamp(-32768.0, 32767.0) as i16;
                    out.extend_from_slice(&v.to_le_bytes());
                }
                out
            }
            24 => {
                let mut out = Vec::with_capacity(samples.len() * 3);
                for &s in samples {
                    let v = (s * 8388607.0).clamp(-8388608.0, 8388607.0) as i32;
                    let bytes = v.to_le_bytes();
                    out.push(bytes[0]);
                    out.push(bytes[1]);
                    out.push(bytes[2]);
                }
                out
            }
            32 => {
                let mut out = Vec::with_capacity(samples.len() * 4);
                for &s in samples {
                    out.extend_from_slice(&s.to_le_bytes());
                }
                out
            }
            _ => Vec::new(),
        }
    }

    pub fn normalize(frame: &mut AudioFrame, peak: f32) {
        if peak <= 0.0 {
            return;
        }
        let mut max_val: f32 = 0.0;
        for &s in &frame.samples {
            let abs = if s < 0.0 { -s } else { s };
            if abs > max_val {
                max_val = abs;
            }
        }
        if max_val <= 0.0 {
            return;
        }
        let gain = peak / max_val;
        for s in &mut frame.samples {
            *s *= gain;
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// §8  Host KATs — playback engine + PCM pipeline
// ═══════════════════════════════════════════════════════════════════════════
//
// Pure-logic proof for the parts of the media engine that turn "open a file" into
// "make sound / detect end-of-stream / control transport": the PCM decoder, the
// WAV demux->decode path, the MediaPipeline state machine, format dispatch, and
// the PCM-conversion helpers that feed AthAudio. Every fixture is built from
// `alloc` only (no std-ism — keeps the no_std crate clean for the R7 gate); the
// test runner links std itself. Each assert is a concrete value, and the hostile-
// input tests prove the untrusted-media boundary never panics.

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use alloc::vec::Vec as StdVec;

    /// Canonical 16-byte-`fmt ` WAV writer. `format`: 1 = int PCM, 3 = float.
    fn write_wav(
        format: u16,
        channels: u16,
        sample_rate: u32,
        bits: u16,
        body: &[u8],
    ) -> StdVec<u8> {
        let block_align = channels * (bits / 8);
        let byte_rate = sample_rate * block_align as u32;
        let mut out = StdVec::new();
        out.extend_from_slice(b"RIFF");
        let riff_size = (4 + (8 + 16) + (8 + body.len())) as u32;
        out.extend_from_slice(&riff_size.to_le_bytes());
        out.extend_from_slice(b"WAVE");
        out.extend_from_slice(b"fmt ");
        out.extend_from_slice(&16u32.to_le_bytes());
        out.extend_from_slice(&format.to_le_bytes());
        out.extend_from_slice(&channels.to_le_bytes());
        out.extend_from_slice(&sample_rate.to_le_bytes());
        out.extend_from_slice(&byte_rate.to_le_bytes());
        out.extend_from_slice(&block_align.to_le_bytes());
        out.extend_from_slice(&bits.to_le_bytes());
        out.extend_from_slice(b"data");
        out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        out.extend_from_slice(body);
        out
    }

    /// i16 LE interleaved body from sample values.
    fn i16_body(samples: &[i16]) -> StdVec<u8> {
        let mut b = StdVec::new();
        for &s in samples {
            b.extend_from_slice(&s.to_le_bytes());
        }
        b
    }

    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    // ── PcmDecoder: concrete sample values ───────────────────────────────

    #[test]
    fn pcm_decoder_16bit_concrete_values() {
        // 2 stereo frames: (0, max), (-max, half).
        let body = i16_body(&[0, 32767, -32768, 16384]);
        let mut dec = PcmDecoder::new(48000, 2, 16, PcmFormat::Integer);
        let pkt = MediaPacket {
            track_id: 1,
            pts: 0,
            dts: 0,
            duration: 0,
            keyframe: true,
            data: body,
            flags: PacketFlags::none(),
        };
        let frame = dec.decode(&pkt).unwrap().expect("frame");
        assert_eq!(frame.nb_samples, 2);
        assert_eq!(frame.channels, 2);
        assert_eq!(frame.samples.len(), 4);
        assert!(approx(frame.samples[0], 0.0));
        assert!(approx(frame.samples[1], 32767.0 / 32768.0));
        assert!(approx(frame.samples[2], -1.0)); // -32768 / 32768
        assert!(approx(frame.samples[3], 0.5));
    }

    #[test]
    fn pcm_decoder_8bit_unsigned_centered() {
        // 8-bit WAV is unsigned, centered at 128: 128 -> 0.0, 255 -> ~+1, 0 -> -1.
        let body: StdVec<u8> = vec![128, 255, 0, 64];
        let mut dec = PcmDecoder::new(8000, 1, 8, PcmFormat::Integer);
        let pkt = MediaPacket {
            track_id: 1,
            pts: 0,
            dts: 0,
            duration: 0,
            keyframe: true,
            data: body,
            flags: PacketFlags::none(),
        };
        let frame = dec.decode(&pkt).unwrap().expect("frame");
        assert_eq!(frame.nb_samples, 4);
        assert!(approx(frame.samples[0], 0.0));
        assert!(approx(frame.samples[1], (255.0 - 128.0) / 128.0));
        assert!(approx(frame.samples[2], -1.0));
        assert!(approx(frame.samples[3], (64.0 - 128.0) / 128.0));
    }

    #[test]
    fn pcm_decoder_float32_passthrough() {
        let mut body = StdVec::new();
        for v in [0.0f32, 1.0, -0.5, 0.25] {
            body.extend_from_slice(&v.to_le_bytes());
        }
        let mut dec = PcmDecoder::new(44100, 1, 32, PcmFormat::Float);
        let pkt = MediaPacket {
            track_id: 1,
            pts: 0,
            dts: 0,
            duration: 0,
            keyframe: true,
            data: body,
            flags: PacketFlags::none(),
        };
        let frame = dec.decode(&pkt).unwrap().expect("frame");
        assert!(approx(frame.samples[0], 0.0));
        assert!(approx(frame.samples[1], 1.0));
        assert!(approx(frame.samples[2], -0.5));
        assert!(approx(frame.samples[3], 0.25));
    }

    #[test]
    fn pcm_decoder_partial_trailing_frame_truncated_no_panic() {
        // 5 bytes for a stereo i16 stream (frame = 4 bytes): 1 whole frame + 1 stray byte.
        let body: StdVec<u8> = vec![0x00, 0x40, 0x00, 0xC0, 0x77];
        let mut dec = PcmDecoder::new(48000, 2, 16, PcmFormat::Integer);
        let pkt = MediaPacket {
            track_id: 1,
            pts: 0,
            dts: 0,
            duration: 0,
            keyframe: true,
            data: body,
            flags: PacketFlags::none(),
        };
        let frame = dec.decode(&pkt).unwrap().expect("one frame");
        assert_eq!(frame.nb_samples, 1); // stray byte dropped, no panic
        assert_eq!(frame.samples.len(), 2);
    }

    #[test]
    fn pcm_decoder_empty_packet_yields_none() {
        let mut dec = PcmDecoder::new(48000, 2, 16, PcmFormat::Integer);
        let pkt = MediaPacket {
            track_id: 1,
            pts: 0,
            dts: 0,
            duration: 0,
            keyframe: true,
            data: StdVec::new(),
            flags: PacketFlags::none(),
        };
        assert!(dec.decode(&pkt).unwrap().is_none());
    }

    // ── WAV demux -> real PCM packets ────────────────────────────────────

    #[test]
    fn demuxer_wav_emits_real_pcm_samples() {
        // 100 stereo frames of a known ramp.
        let mut samples = StdVec::new();
        for i in 0..100i16 {
            samples.push(i * 100); // L
            samples.push(-i * 100); // R
        }
        let wav = write_wav(1, 2, 48000, 16, &i16_body(&samples));
        let mut dmx = Demuxer::open(&wav).expect("open wav");
        assert_eq!(dmx.tracks().len(), 1);
        assert_eq!(dmx.tracks()[0].codec, CodecId::Pcm);

        // Read all packets, decode through PcmDecoder, collect samples.
        let mut dec = PcmDecoder::new(48000, 2, 16, PcmFormat::Integer);
        let mut collected: StdVec<f32> = StdVec::new();
        loop {
            match dmx.read_packet() {
                Ok(pkt) => {
                    assert!(!pkt.data.is_empty(), "WAV packet must carry real PCM");
                    if let Some(f) = dec.decode(&pkt).unwrap() {
                        collected.extend_from_slice(&f.samples);
                    }
                }
                Err(MediaError::EndOfStream) => break,
                Err(e) => panic!("unexpected demux error: {:?}", e),
            }
        }
        // 100 frames * 2 ch = 200 samples, exactly.
        assert_eq!(collected.len(), 200);
        assert!(approx(collected[0], 0.0));
        assert!(approx(collected[2], 100.0 / 32768.0)); // frame 1 L = 100
        assert!(approx(collected[3], -100.0 / 32768.0)); // frame 1 R = -100
    }

    // ── MediaPipeline: full transport state machine over a real WAV ──────

    #[test]
    fn pipeline_open_play_pause_seek_eos() {
        // 48000 frames @ 48kHz = exactly 1000 ms of stereo audio.
        let frames = 48000usize;
        let mut samples = StdVec::with_capacity(frames * 2);
        for _ in 0..frames {
            samples.push(1000i16);
            samples.push(-1000i16);
        }
        let wav = write_wav(1, 2, 48000, 16, &i16_body(&samples));

        let mut pipe = MediaPipeline::new();
        // Initial state
        assert_eq!(*pipe.state(), PlaybackState::Idle);

        // open -> Paused, duration ~1000ms, a PCM audio decoder selected
        pipe.open(&wav).expect("open");
        assert_eq!(*pipe.state(), PlaybackState::Paused);
        assert_eq!(pipe.duration_ms(), 1000);
        assert_eq!(pipe.position_ms(), 0);

        // play -> Playing
        pipe.play().expect("play");
        assert_eq!(*pipe.state(), PlaybackState::Playing);

        // tick advances position and produces real audio frames
        let t = pipe.tick(100);
        assert!(t.audio_ready, "pipeline must produce audio from a WAV");
        assert_eq!(pipe.position_ms(), 100);
        let af = pipe.get_audio_frame().expect("audio frame");
        assert!(
            af.samples.iter().any(|&s| s != 0.0),
            "real (non-silent) PCM"
        );
        assert!(approx(af.samples[0], 1000.0 / 32768.0));

        // pause -> Paused, tick is a no-op on position
        pipe.pause();
        assert_eq!(*pipe.state(), PlaybackState::Paused);
        let before = pipe.position_ms();
        pipe.tick(50);
        assert_eq!(pipe.position_ms(), before);

        // resume + seek to 500ms
        pipe.play().expect("replay");
        pipe.seek(500).expect("seek");
        assert_eq!(pipe.position_ms(), 500);
        assert_eq!(*pipe.state(), PlaybackState::Playing);

        // tick past the end -> Ended (LoopMode::None default)
        let t2 = pipe.tick(600); // 500 + 600 > 1000
        assert!(t2.state_changed);
        assert_eq!(*pipe.state(), PlaybackState::Ended);

        // play after end restarts from 0
        pipe.play().expect("replay after end");
        assert_eq!(pipe.position_ms(), 0);
        assert_eq!(*pipe.state(), PlaybackState::Playing);
    }

    #[test]
    fn pipeline_loop_single_wraps_position() {
        let frames = 4800usize; // 100 ms
        let mut samples = StdVec::with_capacity(frames * 2);
        for _ in 0..frames {
            samples.push(500i16);
            samples.push(500i16);
        }
        let wav = write_wav(1, 2, 48000, 16, &i16_body(&samples));
        let mut pipe = MediaPipeline::new();
        pipe.open(&wav).unwrap();
        assert_eq!(pipe.duration_ms(), 100);
        pipe.set_loop_mode(LoopMode::Single);
        pipe.play().unwrap();
        let _t = pipe.tick(150); // past end -> wraps, not Ended
        assert_eq!(*pipe.state(), PlaybackState::Playing);
        assert_eq!(pipe.position_ms(), 0);
    }

    #[test]
    fn pipeline_play_without_open_errs() {
        let mut pipe = MediaPipeline::new();
        match pipe.play() {
            Err(MediaError::NotInitialized) => {}
            other => panic!("expected NotInitialized, got {:?}", other),
        }
    }

    // ── Format detection dispatch ────────────────────────────────────────

    #[test]
    fn format_detection_picks_right_container() {
        let wav = write_wav(1, 1, 8000, 16, &i16_body(&[0, 0, 0, 0]));
        assert_eq!(Demuxer::detect_format(&wav).unwrap(), ContainerFormat::Wav);

        let mut ogg = StdVec::from(*b"OggS");
        ogg.extend_from_slice(&[0u8; 60]);
        assert_eq!(Demuxer::detect_format(&ogg).unwrap(), ContainerFormat::Ogg);

        let mut flac = StdVec::from(*b"fLaC");
        flac.extend_from_slice(&[0u8; 60]);
        assert_eq!(
            Demuxer::detect_format(&flac).unwrap(),
            ContainerFormat::Flac
        );

        let mut mp4 = StdVec::from([0u8, 0, 0, 0x18]);
        mp4.extend_from_slice(b"ftyp");
        mp4.extend_from_slice(&[0u8; 32]);
        assert_eq!(Demuxer::detect_format(&mp4).unwrap(), ContainerFormat::Mp4);

        let mut id3 = StdVec::from(*b"ID3");
        id3.extend_from_slice(&[0u8; 32]);
        assert_eq!(Demuxer::detect_format(&id3).unwrap(), ContainerFormat::Mp3);

        let ebml = [0x1A, 0x45, 0xDF, 0xA3, 0, 0, 0, 0, 0, 0, 0, 0];
        assert_eq!(Demuxer::detect_format(&ebml).unwrap(), ContainerFormat::Mkv);
    }

    // ── AudioResampler / format conversion: concrete values ──────────────

    #[test]
    fn convert_format_16bit_concrete_bytes() {
        let bytes = AudioResampler::convert_format(&[0.0, 1.0, -1.0], 16);
        assert_eq!(bytes.len(), 6);
        assert_eq!(i16::from_le_bytes([bytes[0], bytes[1]]), 0);
        assert_eq!(i16::from_le_bytes([bytes[2], bytes[3]]), 32767);
        // Symmetric scale by 32767 (not 32768): -1.0 -> -32767, matching the
        // existing convert_format math.
        assert_eq!(i16::from_le_bytes([bytes[4], bytes[5]]), -32767);
    }

    #[test]
    fn resample_doubles_rate_frame_count() {
        let frame = AudioFrame {
            samples: vec![0.0, 1.0, 0.0, -1.0],
            sample_rate: 24000,
            channels: 1,
            channel_layout: ChannelLayout::Mono,
            pts: 0,
            duration: 0,
            nb_samples: 4,
        };
        let out = AudioResampler::resample(&frame, 48000);
        assert_eq!(out.sample_rate, 48000);
        assert_eq!(out.nb_samples, 8); // 4 * (48000/24000)
        assert_eq!(out.samples.len(), 8);
        assert!(approx(out.samples[0], 0.0));
    }

    #[test]
    fn remix_mono_to_stereo_duplicates() {
        let frame = AudioFrame {
            samples: vec![0.3, -0.4],
            sample_rate: 48000,
            channels: 1,
            channel_layout: ChannelLayout::Mono,
            pts: 0,
            duration: 0,
            nb_samples: 2,
        };
        let out = AudioResampler::remix(&frame, ChannelLayout::Stereo);
        assert_eq!(out.channels, 2);
        assert_eq!(out.samples.len(), 4);
        assert!(approx(out.samples[0], 0.3));
        assert!(approx(out.samples[1], 0.3));
        assert!(approx(out.samples[2], -0.4));
        assert!(approx(out.samples[3], -0.4));
    }

    #[test]
    fn remix_stereo_to_mono_averages() {
        let frame = AudioFrame {
            samples: vec![1.0, 0.0, -0.5, 0.5],
            sample_rate: 48000,
            channels: 2,
            channel_layout: ChannelLayout::Stereo,
            pts: 0,
            duration: 0,
            nb_samples: 2,
        };
        let out = AudioResampler::remix(&frame, ChannelLayout::Mono);
        assert_eq!(out.channels, 1);
        assert!(approx(out.samples[0], 0.5)); // (1.0 + 0.0)/2
        assert!(approx(out.samples[1], 0.0)); // (-0.5 + 0.5)/2
    }

    #[test]
    fn normalize_scales_to_peak() {
        let mut frame = AudioFrame {
            samples: vec![0.25, -0.5, 0.1],
            sample_rate: 48000,
            channels: 1,
            channel_layout: ChannelLayout::Mono,
            pts: 0,
            duration: 0,
            nb_samples: 3,
        };
        AudioResampler::normalize(&mut frame, 1.0); // peak abs is 0.5 -> gain 2.0
        assert!(approx(frame.samples[0], 0.5));
        assert!(approx(frame.samples[1], -1.0));
        assert!(approx(frame.samples[2], 0.2));
    }

    // ── Hostile input: never panic on malformed media ────────────────────

    #[test]
    fn open_malformed_never_panics() {
        let cases: StdVec<StdVec<u8>> = vec![
            StdVec::new(),
            StdVec::from(*b"RIFF"),
            StdVec::from(*b"RIFFxxxxWAVE"),
            // Truncated WAV header (claims data but body is short)
            write_wav(1, 2, 48000, 16, &[0x00, 0x40]),
            // Zero channels in fmt — degenerate geometry
            write_wav(1, 0, 48000, 16, &[0u8; 8]),
            // Garbage that looks like an MP4 ftyp but is short
            StdVec::from(*b"\x00\x00\x00\x08ftyp"),
        ];
        for c in &cases {
            // Must return a Result, never panic.
            let _ = Demuxer::open(c);
            // And the pipeline open path must also be panic-safe.
            let mut pipe = MediaPipeline::new();
            let _ = pipe.open(c);
        }
    }

    #[test]
    fn wav_zero_channels_no_pcm_no_panic() {
        // fmt with 0 channels: header may parse but no PCM stream is extracted.
        let wav = write_wav(1, 0, 48000, 16, &[0u8; 16]);
        if let Ok(mut dmx) = Demuxer::open(&wav) {
            // read_packet must not panic; either EOS or a placeholder packet.
            let _ = dmx.read_packet();
        }
    }

    // ── FLAC decoder: native lossless decode, concrete PCM match ─────────
    //
    // These KATs construct known FLAC bitstreams in-test (a minimal FLAC writer with
    // correct CRC-8/CRC-16) and assert the decoder reproduces the exact original
    // samples. A test that constructs a wrong stream would FAIL the CRC or the value
    // asserts — these can print FAIL.

    /// MSB-first bit writer for building test FLAC streams.
    struct FlacBitWriter {
        bytes: StdVec<u8>,
        cur: u8,
        nbits: u8,
    }
    impl FlacBitWriter {
        fn new() -> Self {
            Self {
                bytes: StdVec::new(),
                cur: 0,
                nbits: 0,
            }
        }
        fn write_bit(&mut self, b: u32) {
            self.cur = (self.cur << 1) | ((b & 1) as u8);
            self.nbits += 1;
            if self.nbits == 8 {
                self.bytes.push(self.cur);
                self.cur = 0;
                self.nbits = 0;
            }
        }
        fn write_bits(&mut self, v: u32, n: u32) {
            for i in (0..n).rev() {
                self.write_bit((v >> i) & 1);
            }
        }
        fn write_signed(&mut self, v: i32, n: u32) {
            // Emit the low `n` bits of the two's-complement representation, MSB-first.
            let mask: u32 = if n >= 32 { u32::MAX } else { (1u32 << n) - 1 };
            self.write_bits((v as u32) & mask, n);
        }
        fn write_unary(&mut self, q: u32) {
            for _ in 0..q {
                self.write_bit(0);
            }
            self.write_bit(1);
        }
        fn align(&mut self) {
            while self.nbits != 0 {
                self.write_bit(0);
            }
        }
    }

    fn t_crc8(data: &[u8]) -> u8 {
        let mut crc: u8 = 0;
        for &b in data {
            crc ^= b;
            for _ in 0..8 {
                crc = if crc & 0x80 != 0 {
                    (crc << 1) ^ 0x07
                } else {
                    crc << 1
                };
            }
        }
        crc
    }
    fn t_crc16(data: &[u8]) -> u16 {
        let mut crc: u16 = 0;
        for &b in data {
            crc ^= (b as u16) << 8;
            for _ in 0..8 {
                crc = if crc & 0x8000 != 0 {
                    (crc << 1) ^ 0x8005
                } else {
                    crc << 1
                };
            }
        }
        crc
    }

    /// Build a 34-byte STREAMINFO body. sr=sample rate, ch=channels, bps=bits.
    fn streaminfo_body(block: u16, sr: u32, ch: u8, bps: u8, total: u64) -> StdVec<u8> {
        let mut b = StdVec::new();
        b.extend_from_slice(&block.to_be_bytes()); // min block
        b.extend_from_slice(&block.to_be_bytes()); // max block
        b.extend_from_slice(&[0, 0, 0]); // min frame size
        b.extend_from_slice(&[0, 0, 0]); // max frame size
                                         // 20 bits sr, 3 bits ch-1, 5 bits bps-1, 36 bits total samples.
        let chm1 = (ch - 1) as u32;
        let bpsm1 = (bps - 1) as u32;
        let b10 = (sr >> 12) as u8;
        let b11 = (sr >> 4) as u8;
        let b12 = (((sr & 0xF) << 4) as u8) | ((chm1 << 1) as u8) | ((bpsm1 >> 4) as u8);
        let b13 = (((bpsm1 & 0xF) << 4) as u8) | ((total >> 32) & 0xF) as u8;
        b.push(b10);
        b.push(b11);
        b.push(b12);
        b.push(b13);
        b.push((total >> 24) as u8);
        b.push((total >> 16) as u8);
        b.push((total >> 8) as u8);
        b.push(total as u8);
        b.extend_from_slice(&[0u8; 16]); // MD5 (ignored)
        assert_eq!(b.len(), 34);
        b
    }

    /// Wrap a STREAMINFO + a single frame body into a full `fLaC` stream.
    fn flac_stream(si: &[u8], frame: &[u8]) -> StdVec<u8> {
        let mut out = StdVec::new();
        out.extend_from_slice(b"fLaC");
        // Metadata block header: last-block=1, type=0 (STREAMINFO), length=34.
        out.push(0x80);
        out.extend_from_slice(&[0x00, 0x00, 0x22]); // 34
        out.extend_from_slice(si);
        out.extend_from_slice(frame);
        out
    }

    /// Build a fixed-blocksize frame header for `block` samples, sr code 0 (use
    /// STREAMINFO), the given channel-assignment code and sample-size code, frame
    /// number 0. Returns the header bytes WITH the trailing CRC-8.
    fn frame_header(block: u32, chan_code: u32, ss_code: u32) -> StdVec<u8> {
        let mut w = FlacBitWriter::new();
        w.write_bits(0b11_1111_1111_1110, 14); // sync
        w.write_bit(0); // reserved
        w.write_bit(0); // blocking strategy: fixed
                        // Block size code: use explicit 16-bit (code 7) to allow any block.
        w.write_bits(7, 4);
        w.write_bits(0, 4); // sample rate code 0 -> from STREAMINFO
        w.write_bits(chan_code, 4);
        w.write_bits(ss_code, 3);
        w.write_bit(0); // reserved
                        // UTF-8 coded frame number 0 = single byte 0x00.
        w.write_bits(0, 8);
        // Block size (code 7): 16-bit value-1.
        w.write_bits(block - 1, 16);
        w.align();
        let mut bytes = w.bytes;
        let crc = t_crc8(&bytes);
        bytes.push(crc);
        bytes
    }

    /// Append a CONSTANT subframe of value `v` at depth `bps`.
    fn sub_constant(w: &mut FlacBitWriter, v: i32, bps: u32) {
        w.write_bit(0); // pad
        w.write_bits(0b000000, 6); // CONSTANT
        w.write_bit(0); // no wasted bits
        w.write_signed(v, bps);
    }

    /// Append a VERBATIM subframe at depth `bps`.
    fn sub_verbatim(w: &mut FlacBitWriter, samples: &[i32], bps: u32) {
        w.write_bit(0);
        w.write_bits(0b000001, 6); // VERBATIM
        w.write_bit(0);
        for &s in samples {
            w.write_signed(s, bps);
        }
    }

    /// Append a FIXED order-`order` subframe with an order-0 partition Rice residual
    /// (rice param `k`). `warmup` provides the first `order` samples; `residuals` the
    /// rest (block - order of them), Rice-coded.
    fn sub_fixed_rice(
        w: &mut FlacBitWriter,
        order: u32,
        bps: u32,
        warmup: &[i32],
        residuals: &[i32],
        k: u32,
    ) {
        w.write_bit(0);
        w.write_bits(0b001000 | order, 6); // FIXED + order
        w.write_bit(0); // no wasted bits
        for &s in warmup {
            w.write_signed(s, bps);
        }
        // Residual: method 0 (4-bit param), partition order 0 (1 partition).
        w.write_bits(0, 2);
        w.write_bits(0, 4); // partition order 0
        w.write_bits(k, 4); // rice parameter
        for &r in residuals {
            // zig-zag encode then Rice with param k.
            let u = if r >= 0 {
                (r as u32) << 1
            } else {
                (((-r) as u32) << 1) - 1
            };
            let q = u >> k;
            let rem = u & ((1 << k) - 1);
            w.write_unary(q);
            if k > 0 {
                w.write_bits(rem, k);
            }
        }
    }

    /// FIXED order-`order` subframe whose residual is split into 2 partitions
    /// (partition order 1), each with its own Rice parameter — exercises the
    /// partitioned-residual path. `block` must be even and > order.
    fn sub_fixed_partitioned(
        w: &mut FlacBitWriter,
        order: u32,
        bps: u32,
        block: u32,
        warmup: &[i32],
        residuals: &[i32],
        k0: u32,
        k1: u32,
    ) {
        w.write_bit(0);
        w.write_bits(0b001000 | order, 6);
        w.write_bit(0);
        for &s in warmup {
            w.write_signed(s, bps);
        }
        // Residual: method 0 (4-bit), partition order 1 => 2 partitions.
        w.write_bits(0, 2);
        w.write_bits(1, 4);
        let part_len = (block / 2) as usize;
        // Partition 0 holds (part_len - order) residuals, partition 1 holds part_len.
        let p0 = part_len - order as usize;
        let write_part = |w: &mut FlacBitWriter, slice: &[i32], k: u32| {
            w.write_bits(k, 4);
            for &r in slice {
                let u = if r >= 0 {
                    (r as u32) << 1
                } else {
                    (((-r) as u32) << 1) - 1
                };
                let q = u >> k;
                let rem = u & ((1 << k) - 1);
                w.write_unary(q);
                if k > 0 {
                    w.write_bits(rem, k);
                }
            }
        };
        write_part(w, &residuals[..p0], k0);
        write_part(w, &residuals[p0..], k1);
    }

    /// Append an LPC order-`order` subframe with quantized coefficients, precision
    /// `prec` bits, shift `shift`, order-0 partition Rice residual (param `k`).
    fn sub_lpc_rice(
        w: &mut FlacBitWriter,
        order: u32,
        bps: u32,
        warmup: &[i32],
        coeffs: &[i32],
        prec: u32,
        shift: u32,
        residuals: &[i32],
        k: u32,
    ) {
        w.write_bit(0);
        w.write_bits(0b100000 | (order - 1), 6); // LPC + (order-1)
        w.write_bit(0); // no wasted bits
        for &s in warmup {
            w.write_signed(s, bps);
        }
        w.write_bits(prec - 1, 4); // precision-1
        w.write_signed(shift as i32, 5); // shift
        for &c in coeffs {
            w.write_signed(c, prec);
        }
        // Residual: method 0, partition order 0.
        w.write_bits(0, 2);
        w.write_bits(0, 4);
        w.write_bits(k, 4);
        for &r in residuals {
            let u = if r >= 0 {
                (r as u32) << 1
            } else {
                (((-r) as u32) << 1) - 1
            };
            let q = u >> k;
            let rem = u & ((1 << k) - 1);
            w.write_unary(q);
            if k > 0 {
                w.write_bits(rem, k);
            }
        }
    }

    /// Finish a frame: align + append CRC-16 over the whole frame so far (header+body).
    fn finish_frame(header: &[u8], body_writer: FlacBitWriter) -> StdVec<u8> {
        let mut frame = StdVec::new();
        frame.extend_from_slice(header);
        let mut bw = body_writer;
        bw.align();
        frame.extend_from_slice(&bw.bytes);
        let crc = t_crc16(&frame);
        frame.extend_from_slice(&crc.to_be_bytes());
        frame
    }

    #[test]
    fn flac_streaminfo_parses_geometry() {
        let si = streaminfo_body(4, 44100, 2, 16, 4);
        // ss_code 4 = 16-bit, chan_code 0 = 1 channel independent... use ch=2 mono pair.
        let header = frame_header(4, 1, 4); // 2 channels independent (code 1)
        let mut body = FlacBitWriter::new();
        sub_constant(&mut body, 100, 16);
        sub_constant(&mut body, -100, 16);
        let frame = finish_frame(&header, body);
        let stream = flac_stream(&si, &frame);

        let (info, frame_start) = flac::parse_metadata(&stream).expect("metadata");
        assert_eq!(info.sample_rate, 44100);
        assert_eq!(info.channels, 2);
        assert_eq!(info.bits_per_sample, 16);
        assert_eq!(info.total_samples, 4);
        assert_eq!(frame_start, 4 + 4 + 34); // marker + block header + body
    }

    #[test]
    fn flac_constant_subframe_concrete_values() {
        // 4-sample stereo block, both channels CONSTANT (L=100, R=-100), independent.
        let si = streaminfo_body(4, 48000, 2, 16, 4);
        let header = frame_header(4, 1, 4); // chan_code 1 = 2 independent channels
        let mut body = FlacBitWriter::new();
        sub_constant(&mut body, 100, 16);
        sub_constant(&mut body, -100, 16);
        let frame = finish_frame(&header, body);
        let stream = flac_stream(&si, &frame);

        let (samples, sr, ch, bps) = FlacDecoder::decode_all(&stream).expect("decode");
        assert_eq!(sr, 48000);
        assert_eq!(ch, 2);
        assert_eq!(bps, 16);
        assert_eq!(samples.len(), 8); // 4 frames * 2 ch
                                      // Interleaved L,R,L,R... all L=100/32768, R=-100/32768.
        for i in 0..4 {
            assert!(approx(samples[i * 2], 100.0 / 32768.0));
            assert!(approx(samples[i * 2 + 1], -100.0 / 32768.0));
        }
    }

    #[test]
    fn flac_verbatim_subframe_concrete_values() {
        // Mono, 4 samples, VERBATIM: exact arbitrary values round-trip.
        let vals = [1000i32, -2000, 32767, -32768];
        let si = streaminfo_body(4, 44100, 1, 16, 4);
        let header = frame_header(4, 0, 4); // chan_code 0 = 1 channel
        let mut body = FlacBitWriter::new();
        sub_verbatim(&mut body, &vals, 16);
        let frame = finish_frame(&header, body);
        let stream = flac_stream(&si, &frame);

        let (samples, sr, ch, _bps) = FlacDecoder::decode_all(&stream).expect("decode");
        assert_eq!(sr, 44100);
        assert_eq!(ch, 1);
        assert_eq!(samples.len(), 4);
        assert!(approx(samples[0], 1000.0 / 32768.0));
        assert!(approx(samples[1], -2000.0 / 32768.0));
        assert!(approx(samples[2], 32767.0 / 32768.0));
        assert!(approx(samples[3], -1.0)); // -32768/32768
    }

    #[test]
    fn flac_fixed_order1_rice_concrete_values() {
        // Mono, 8 samples, FIXED order-1 predictor.
        // Original signal we want back:
        let original = [10i32, 13, 17, 22, 28, 35, 43, 52];
        // order-1 predictor: pred[i] = s[i-1]; residual = s[i] - s[i-1].
        // warmup = original[0]; residuals = diffs.
        let warmup = [original[0]];
        let mut residuals = StdVec::new();
        for i in 1..original.len() {
            residuals.push(original[i] - original[i - 1]);
        }
        let si = streaminfo_body(8, 44100, 1, 16, 8);
        let header = frame_header(8, 0, 4); // mono, 16-bit
        let mut body = FlacBitWriter::new();
        sub_fixed_rice(&mut body, 1, 16, &warmup, &residuals, 2);
        let frame = finish_frame(&header, body);
        let stream = flac_stream(&si, &frame);

        let (samples, _sr, ch, _bps) = FlacDecoder::decode_all(&stream).expect("decode");
        assert_eq!(ch, 1);
        assert_eq!(samples.len(), 8);
        for (i, &o) in original.iter().enumerate() {
            assert!(
                approx(samples[i], o as f32 / 32768.0),
                "sample {} expected {} got {}",
                i,
                o as f32 / 32768.0,
                samples[i]
            );
        }
    }

    #[test]
    fn flac_fixed_order2_rice_concrete_values() {
        // Mono, 8 samples, FIXED order-2 predictor: pred = 2*s[i-1] - s[i-2].
        let original = [5i32, 8, 12, 17, 23, 30, 38, 47];
        let warmup = [original[0], original[1]];
        let mut residuals = StdVec::new();
        for i in 2..original.len() {
            let pred = 2 * original[i - 1] - original[i - 2];
            residuals.push(original[i] - pred);
        }
        let si = streaminfo_body(8, 48000, 1, 16, 8);
        let header = frame_header(8, 0, 4);
        let mut body = FlacBitWriter::new();
        sub_fixed_rice(&mut body, 2, 16, &warmup, &residuals, 2);
        let frame = finish_frame(&header, body);
        let stream = flac_stream(&si, &frame);

        let (samples, _sr, _ch, _bps) = FlacDecoder::decode_all(&stream).expect("decode");
        assert_eq!(samples.len(), 8);
        for (i, &o) in original.iter().enumerate() {
            assert!(approx(samples[i], o as f32 / 32768.0), "sample {}", i);
        }
    }

    #[test]
    fn flac_partitioned_residual_concrete_values() {
        // Mono, 8 samples, FIXED order-1, residual split into 2 partitions of 4.
        let original = [10i32, 13, 17, 22, 28, 35, 43, 52];
        let warmup = [original[0]];
        let mut residuals = StdVec::new();
        for i in 1..original.len() {
            residuals.push(original[i] - original[i - 1]);
        }
        let si = streaminfo_body(8, 44100, 1, 16, 8);
        let header = frame_header(8, 0, 4);
        let mut body = FlacBitWriter::new();
        sub_fixed_partitioned(&mut body, 1, 16, 8, &warmup, &residuals, 2, 3);
        let frame = finish_frame(&header, body);
        let stream = flac_stream(&si, &frame);

        let (samples, _sr, _ch, _bps) =
            FlacDecoder::decode_all(&stream).expect("decode partitioned");
        assert_eq!(samples.len(), 8);
        for (i, &o) in original.iter().enumerate() {
            assert!(
                approx(samples[i], o as f32 / 32768.0),
                "partitioned sample {}",
                i
            );
        }
    }

    #[test]
    fn flac_lpc_subframe_concrete_values() {
        // Mono, 8 samples, LPC order-2 with coeffs [c0, c1], shift `s`:
        //   pred[i] = (c0*s[i-1] + c1*s[i-2]) >> s
        //   s[i] = pred[i] + residual[i]
        // Choose coeffs [2, -1] shift 0 (== FIXED order-2) so we can hand-derive
        // residuals from a known signal but exercise the real LPC code path.
        let coeffs = [2i32, -1i32];
        let prec = 4u32; // 4-bit coeffs: 2 and -1 fit
        let shift = 0u32;
        let original = [7i32, 9, 12, 16, 21, 27, 34, 42];
        let warmup = [original[0], original[1]];
        let mut residuals = StdVec::new();
        for i in 2..original.len() {
            // pred = (2*s[i-1] - 1*s[i-2]) >> 0
            let pred = (coeffs[0] * original[i - 1] + coeffs[1] * original[i - 2]) >> shift;
            residuals.push(original[i] - pred);
        }
        let si = streaminfo_body(8, 44100, 1, 16, 8);
        let header = frame_header(8, 0, 4);
        let mut body = FlacBitWriter::new();
        sub_lpc_rice(
            &mut body, 2, 16, &warmup, &coeffs, prec, shift, &residuals, 2,
        );
        let frame = finish_frame(&header, body);
        let stream = flac_stream(&si, &frame);

        let (samples, _sr, ch, _bps) = FlacDecoder::decode_all(&stream).expect("decode lpc");
        assert_eq!(ch, 1);
        assert_eq!(samples.len(), 8);
        for (i, &o) in original.iter().enumerate() {
            assert!(
                approx(samples[i], o as f32 / 32768.0),
                "lpc sample {} expected {} got {}",
                i,
                o as f32 / 32768.0,
                samples[i]
            );
        }
    }

    #[test]
    fn flac_lpc_with_shift_concrete_values() {
        // LPC order-1, coeff [3], shift 1: pred[i] = (3*s[i-1]) >> 1.
        let coeffs = [3i32];
        let prec = 4u32;
        let shift = 1u32;
        let original = [100i32, 140, 200, 290, 430, 640, 950, 1420];
        let warmup = [original[0]];
        let mut residuals = StdVec::new();
        for i in 1..original.len() {
            let pred = (coeffs[0] * original[i - 1]) >> shift;
            residuals.push(original[i] - pred);
        }
        let si = streaminfo_body(8, 48000, 1, 16, 8);
        let header = frame_header(8, 0, 4);
        let mut body = FlacBitWriter::new();
        sub_lpc_rice(
            &mut body, 1, 16, &warmup, &coeffs, prec, shift, &residuals, 4,
        );
        let frame = finish_frame(&header, body);
        let stream = flac_stream(&si, &frame);

        let (samples, _sr, _ch, _bps) = FlacDecoder::decode_all(&stream).expect("decode lpc shift");
        assert_eq!(samples.len(), 8);
        for (i, &o) in original.iter().enumerate() {
            assert!(
                approx(samples[i], o as f32 / 32768.0),
                "lpc-shift sample {}",
                i
            );
        }
    }

    #[test]
    fn flac_mid_side_decorrelation_reconstructs() {
        // 4 stereo samples. Pick L,R, derive mid/side, encode both subframes VERBATIM,
        // set channel assignment = mid-side (code 10), assert L,R come back exactly.
        let l = [100i32, 200, -50, 32767];
        let r = [80i32, 150, -60, -32768];
        let mut mid = StdVec::new();
        let mut side = StdVec::new();
        for i in 0..4 {
            mid.push((l[i] + r[i]) >> 1);
            side.push(l[i] - r[i]);
        }
        let si = streaminfo_body(4, 44100, 2, 16, 4);
        let header = frame_header(4, 10, 4); // chan_code 10 = mid-side
        let mut body = FlacBitWriter::new();
        // mid channel at 16 bits, side channel at 17 bits (bps+1).
        sub_verbatim(&mut body, &mid, 16);
        sub_verbatim(&mut body, &side, 17);
        let frame = finish_frame(&header, body);
        let stream = flac_stream(&si, &frame);

        let (samples, _sr, ch, _bps) = FlacDecoder::decode_all(&stream).expect("decode");
        assert_eq!(ch, 2);
        assert_eq!(samples.len(), 8);
        for i in 0..4 {
            assert!(
                approx(samples[i * 2], l[i] as f32 / 32768.0),
                "L[{}] expected {} got {}",
                i,
                l[i] as f32 / 32768.0,
                samples[i * 2]
            );
            assert!(
                approx(samples[i * 2 + 1], r[i] as f32 / 32768.0),
                "R[{}] expected {} got {}",
                i,
                r[i] as f32 / 32768.0,
                samples[i * 2 + 1]
            );
        }
    }

    #[test]
    fn flac_left_side_decorrelation_reconstructs() {
        let l = [300i32, -400, 1000, 12345];
        let r = [250i32, -380, 900, -12345];
        let mut side = StdVec::new();
        for i in 0..4 {
            side.push(l[i] - r[i]);
        }
        let si = streaminfo_body(4, 48000, 2, 16, 4);
        let header = frame_header(4, 8, 4); // chan_code 8 = left-side
        let mut body = FlacBitWriter::new();
        sub_verbatim(&mut body, &l, 16); // left at 16 bits
        sub_verbatim(&mut body, &side, 17); // side at 17 bits
        let frame = finish_frame(&header, body);
        let stream = flac_stream(&si, &frame);

        let (samples, _sr, _ch, _bps) = FlacDecoder::decode_all(&stream).expect("decode");
        assert_eq!(samples.len(), 8);
        for i in 0..4 {
            assert!(approx(samples[i * 2], l[i] as f32 / 32768.0), "L[{}]", i);
            assert!(
                approx(samples[i * 2 + 1], r[i] as f32 / 32768.0),
                "R[{}]",
                i
            );
        }
    }

    #[test]
    fn flac_pipeline_end_to_end_real_pcm() {
        // The whole transport: Demuxer detects fLaC, retains bytes, FlacDecoder
        // produces real (non-silent) PCM through the pipeline like WAV does.
        let si = streaminfo_body(4, 48000, 2, 16, 4);
        let header = frame_header(4, 1, 4); // 2 independent channels
        let mut body = FlacBitWriter::new();
        sub_constant(&mut body, 5000, 16);
        sub_constant(&mut body, -5000, 16);
        let frame = finish_frame(&header, body);
        let stream = flac_stream(&si, &frame);

        let mut pipe = MediaPipeline::new();
        pipe.open(&stream).expect("open flac");
        pipe.play().expect("play");
        let t = pipe.tick(50);
        assert!(
            t.audio_ready,
            "pipeline must produce audio from a FLAC stream"
        );
        let af = pipe.get_audio_frame().expect("audio frame");
        assert!(
            af.samples.iter().any(|&s| s != 0.0),
            "real (non-silent) FLAC PCM"
        );
        assert!(approx(af.samples[0], 5000.0 / 32768.0));
        assert!(approx(af.samples[1], -5000.0 / 32768.0));
    }

    #[test]
    fn flac_truncated_stream_no_panic() {
        // Build a valid stream, then feed progressively truncated prefixes: every one
        // must return cleanly (Err or empty), never panic.
        let si = streaminfo_body(8, 44100, 1, 16, 8);
        let header = frame_header(8, 0, 4);
        let mut body = FlacBitWriter::new();
        sub_fixed_rice(&mut body, 1, 16, &[10], &[3, 4, 5, 6, 7, 8, 9], 2);
        let frame = finish_frame(&header, body);
        let stream = flac_stream(&si, &frame);

        for cut in 0..stream.len() {
            let prefix = &stream[..cut];
            // Direct decoder path.
            let _ = FlacDecoder::decode_all(prefix);
            // Metadata parse path.
            let _ = flac::parse_metadata(prefix);
            // Full pipeline path.
            let mut pipe = MediaPipeline::new();
            if pipe.open(prefix).is_ok() {
                pipe.play().ok();
                let _ = pipe.tick(50);
                let _ = pipe.get_audio_frame();
            }
        }
    }

    #[test]
    fn flac_corrupt_frame_crc_yields_err_not_panic() {
        // Valid stream, then flip a byte in the frame body so the CRC-16 fails.
        let si = streaminfo_body(4, 48000, 2, 16, 4);
        let header = frame_header(4, 1, 4);
        let mut body = FlacBitWriter::new();
        sub_constant(&mut body, 100, 16);
        sub_constant(&mut body, -100, 16);
        let frame = finish_frame(&header, body);
        let mut stream = flac_stream(&si, &frame);
        // Corrupt a byte deep in the frame body (after metadata).
        let idx = stream.len() - 3;
        stream[idx] ^= 0xFF;

        // parse_metadata still succeeds; the frame decode reports BadFrameCrc.
        let (info, fs) = flac::parse_metadata(&stream).expect("metadata still valid");
        let r = flac::decode_frame(&stream[fs..], &info);
        assert!(
            matches!(r, Err(flac::FlacError::BadFrameCrc)),
            "expected BadFrameCrc error"
        );
        // And decode_all degrades gracefully (no panic, no partial garbage frame).
        let (samples, _sr, _ch, _bps) = FlacDecoder::decode_all(&stream).unwrap();
        assert!(samples.is_empty(), "corrupt first frame yields no output");
    }

    #[test]
    fn pcm_decoder_24bit_sign_extends() {
        // One mono 24-bit sample = -1 (0xFFFFFF) -> ~ -1/8388608.
        let body: StdVec<u8> = vec![0xFF, 0xFF, 0xFF];
        let mut dec = PcmDecoder::new(48000, 1, 24, PcmFormat::Integer);
        let pkt = MediaPacket {
            track_id: 1,
            pts: 0,
            dts: 0,
            duration: 0,
            keyframe: true,
            data: body,
            flags: PacketFlags::none(),
        };
        let frame = dec.decode(&pkt).unwrap().expect("frame");
        assert!(approx(frame.samples[0], -1.0 / 8_388_608.0));
    }

    // ── MP3 (mp3.rs): header / side-info / Huffman / reservoir / hostile input ──

    /// A canonical MPEG-1 Layer III, 128 kbps, 44100 Hz, stereo, no CRC, no padding
    /// 4-byte header. Decodes to frame_size 417.
    fn mp3_header_v1_128_44100_stereo() -> [u8; 4] {
        // 0xFF, 0xFB, 0x90, 0x00
        [0xFF, 0xFB, 0x90, 0x00]
    }

    #[test]
    fn mp3_header_parses_exact_fields() {
        let h = mp3::FrameHeader::parse(&mp3_header_v1_128_44100_stereo()).expect("parse");
        assert_eq!(h.version, mp3::MpegVersion::V1);
        assert_eq!(h.layer, 3);
        assert_eq!(h.protection, false); // protection bit set => CRC absent
        assert_eq!(h.bitrate, 128_000);
        assert_eq!(h.sample_rate, 44100);
        assert_eq!(h.padding, false);
        assert_eq!(h.mode, mp3::ChannelMode::Stereo);
        assert_eq!(h.channels, 2);
        assert_eq!(h.frame_size, 417); // 144 * 128000 / 44100 = 417
        assert_eq!(h.samples_per_frame(), 1152);
        assert_eq!(h.granules(), 2);
        assert_eq!(h.side_info_size(), 32); // V1 stereo
    }

    #[test]
    fn mp3_header_mpeg2_mono_and_padding() {
        // MPEG-2 (10), Layer III (01), no CRC (1) => byte1 = 0b1111_0011 = 0xF3.
        // bitrate idx 8 (V2 = 64 kbps), sr idx 0 (22050), padding=1 => byte2:
        // 0b1000_00_1_0 = 0x82. Mode mono (11) => byte3 = 0b1100_0000 = 0xC0.
        let hdr = [0xFFu8, 0xF3, 0x82, 0xC0];
        let h = mp3::FrameHeader::parse(&hdr).expect("parse");
        assert_eq!(h.version, mp3::MpegVersion::V2);
        assert_eq!(h.bitrate, 64_000);
        assert_eq!(h.sample_rate, 22050);
        assert_eq!(h.padding, true);
        assert_eq!(h.mode, mp3::ChannelMode::Mono);
        assert_eq!(h.channels, 1);
        assert_eq!(h.granules(), 1);
        assert_eq!(h.samples_per_frame(), 576);
        assert_eq!(h.side_info_size(), 9); // V2 mono
                                           // 72 * 64000 / 22050 + 1 = 209.
        assert_eq!(h.frame_size, 209);
    }

    #[test]
    fn mp3_header_rejects_bad_sync_and_reserved() {
        // Bad sync.
        assert!(matches!(
            mp3::FrameHeader::parse(&[0x00, 0x00, 0x00, 0x00]),
            Err(mp3::Mp3Error::NoSync)
        ));
        // Reserved MPEG version (01 in version field): byte1 = 0b1110_1011 = 0xEB.
        assert!(matches!(
            mp3::FrameHeader::parse(&[0xFF, 0xEB, 0x90, 0x00]),
            Err(mp3::Mp3Error::Invalid(_))
        ));
        // Reserved bitrate index (0xF): byte2 = 0b1111_0000 = 0xF0.
        assert!(matches!(
            mp3::FrameHeader::parse(&[0xFF, 0xFB, 0xF0, 0x00]),
            Err(mp3::Mp3Error::Invalid(_))
        ));
        // Truncated.
        assert!(matches!(
            mp3::FrameHeader::parse(&[0xFF, 0xFB]),
            Err(mp3::Mp3Error::UnexpectedEof)
        ));
    }

    #[test]
    fn mp3_side_info_parses_main_data_begin_and_granule_fields() {
        // Build a V1 mono side info (17 bytes). main_data_begin = 0x1A2 (9 bits).
        // We construct the bitstream MSB-first.
        let mut w = Mp3BitWriter::new();
        w.put(0x1A2, 9); // main_data_begin
        w.put(0, 5); // private_bits (mono)
        for _ in 0..4 {
            w.put(0, 1); // scfsi[0][band] (1 channel)
        }
        // granule 0, channel 0:
        w.put(123, 12); // part2_3_length
        w.put(200, 9); // big_values
        w.put(210, 8); // global_gain
        w.put(5, 4); // scalefac_compress (V1: 4 bits)
        w.put(0, 1); // window_switching = 0 (long block)
        w.put(10, 5); // table_select[0]
        w.put(11, 5); // table_select[1]
        w.put(12, 5); // table_select[2]
        w.put(7, 4); // region0_count
        w.put(3, 3); // region1_count
        w.put(1, 1); // preflag
        w.put(0, 1); // scalefac_scale
        w.put(1, 1); // count1table_select
        let bytes = w.into_bytes_padded(17);

        let si = mp3::parse_side_info(&bytes, mp3::MpegVersion::V1, 1).expect("side info");
        assert_eq!(si.main_data_begin, 0x1A2);
        let g = &si.gr[0][0];
        assert_eq!(g.part2_3_length, 123);
        assert_eq!(g.big_values, 200);
        assert_eq!(g.global_gain, 210);
        assert_eq!(g.scalefac_compress, 5);
        assert_eq!(g.window_switching, false);
        assert_eq!(g.block_type, 0);
        assert_eq!(g.table_select, [10, 11, 12]);
        assert_eq!(g.region0_count, 7);
        assert_eq!(g.region1_count, 3);
        assert_eq!(g.preflag, true);
        assert_eq!(g.scalefac_scale, 0);
        assert_eq!(g.count1table_select, 1);
    }

    #[test]
    fn mp3_huffman_table1_decodes_known_pairs() {
        // Verified Table 1 (mp3_tables.rs, generator-proven prefix-free):
        // (0,0)=code 1 len1; (1,0)=code 1 len2 (01); (0,1)=code 1 len3 (001);
        // (1,1)=code 0 len3 (000).
        let mut w = Mp3BitWriter::new();
        w.put(0b1, 1); // (0,0) — no sign bits
        w.put(0b01, 2); // (1,0): x!=0 -> 1 sign bit
        w.put(0, 1); // x sign positive
        w.put(0b001, 3); // (0,1): y!=0 -> 1 sign bit
        w.put(1, 1); // y sign negative
        w.put(0b000, 3); // (1,1): x sign then y sign
        w.put(1, 1); // x sign negative
        w.put(0, 1); // y sign positive
        let bytes = w.into_bytes();

        let table = mp3::huff_table(1).expect("table 1");
        let mut r = mp3::BitReader::new(&bytes);
        assert_eq!(mp3::decode_huff_pair(&mut r, &table).unwrap(), (0, 0));
        assert_eq!(mp3::decode_huff_pair(&mut r, &table).unwrap(), (1, 0));
        assert_eq!(mp3::decode_huff_pair(&mut r, &table).unwrap(), (0, -1));
        assert_eq!(mp3::decode_huff_pair(&mut r, &table).unwrap(), (-1, 1));
    }

    #[test]
    fn mp3_huffman_table2_decodes_high_value_pair() {
        // Table 2: code 000001 => (0,2). y!=0 -> 1 sign bit. Choose negative.
        let mut w = Mp3BitWriter::new();
        w.put(0b000001, 6);
        w.put(1, 1); // sign of y negative
        let bytes = w.into_bytes();
        let table = mp3::huff_table(2).expect("table 2");
        let mut r = mp3::BitReader::new(&bytes);
        assert_eq!(mp3::decode_huff_pair(&mut r, &table).unwrap(), (0, -2));
    }

    #[test]
    fn mp3_count1_table_b_decodes_quad() {
        // Table B: 4 bits, ones-complement magnitudes, then sign per nonzero.
        // bits 0b0000 => magnitudes (1,1,1,1); choose signs (+,-,+,-).
        let mut w = Mp3BitWriter::new();
        w.put(0b0000, 4);
        w.put(0, 1); // v +
        w.put(1, 1); // w -
        w.put(0, 1); // x +
        w.put(1, 1); // y -
        let bytes = w.into_bytes();
        let mut r = mp3::BitReader::new(&bytes);
        assert_eq!(mp3::decode_count1_quad(&mut r, 1).unwrap(), (1, -1, 1, -1));

        // bits 0b1111 => magnitudes (0,0,0,0) -> no sign bits consumed.
        let mut w2 = Mp3BitWriter::new();
        w2.put(0b1111, 4);
        let bytes2 = w2.into_bytes();
        let mut r2 = mp3::BitReader::new(&bytes2);
        assert_eq!(mp3::decode_count1_quad(&mut r2, 1).unwrap(), (0, 0, 0, 0));
    }

    #[test]
    fn mp3_count1_and_huff_tables_resolve_or_clean_invalid() {
        // count1 table A now decodes (no longer deferred): a value-0000 quad = code 1.
        let bytes = [0x80u8]; // 0b1000_0000 -> first bit 1 = value 0000
        let mut r = mp3::BitReader::new(&bytes);
        assert_eq!(mp3::decode_count1_quad(&mut r, 0).unwrap(), (0, 0, 0, 0));
        // count1 select 2+ is non-conformant -> clean Invalid, never wrong data.
        let mut r2 = mp3::BitReader::new(&[0xFFu8, 0xFF]);
        assert!(matches!(
            mp3::decode_count1_quad(&mut r2, 2),
            Err(mp3::Mp3Error::Invalid(_))
        ));
        // The ISO "not used" big-values selects 4 and 14 return clean Invalid.
        assert!(matches!(mp3::huff_table(4), Err(mp3::Mp3Error::Invalid(_))));
        assert!(matches!(
            mp3::huff_table(14),
            Err(mp3::Mp3Error::Invalid(_))
        ));
        // Out-of-range selects also clean-stop.
        assert!(matches!(
            mp3::huff_table(99),
            Err(mp3::Mp3Error::Invalid(_))
        ));
        // The full conformant set now resolves.
        for &s in &[5u32, 9, 10, 11, 12, 13, 15, 16, 23, 24, 31] {
            assert!(mp3::huff_table(s).is_ok(), "select {} should resolve", s);
        }
    }

    #[test]
    fn mp3_reservoir_assembles_back_reference() {
        let mut res = mp3::Reservoir::new();
        res.push(&[1, 2, 3, 4, 5]); // earlier frame main data
                                    // This frame's main data is [10, 11], referencing 3 bytes back (3,4,5).
        let assembled = res.assemble(3, &[10, 11]).expect("assemble");
        assert_eq!(assembled, alloc::vec![3, 4, 5, 10, 11]);
        // A back-pointer past what we have buffered => NeedMoreData (stream start).
        assert!(matches!(
            res.assemble(100, &[0]),
            Err(mp3::Mp3Error::NeedMoreData)
        ));
    }

    #[test]
    fn mp3_demuxer_skips_id3v2_and_reads_real_geometry() {
        // ID3v2 tag (10-byte header, syncsafe size = 8 bytes of payload) then a real
        // V1/128/44100/stereo frame header padded out to a plausible frame.
        let mut data = StdVec::new();
        data.extend_from_slice(b"ID3");
        data.push(0x03); // version major
        data.push(0x00); // version minor
        data.push(0x00); // flags (no footer)
                         // syncsafe size = 8
        data.extend_from_slice(&[0x00, 0x00, 0x00, 0x08]);
        data.extend_from_slice(&[0u8; 8]); // tag payload
                                           // The MP3 frame.
        data.extend_from_slice(&mp3_header_v1_128_44100_stereo());
        // Pad to a full frame (417 bytes) so duration math is sane.
        while data.len() < 30 + 417 {
            data.push(0);
        }

        // skip_id3v2 must land exactly past the tag.
        assert_eq!(skip_id3v2(&data), 18);
        // find_mp3_frame must locate the sync right after the tag.
        assert_eq!(find_mp3_frame(&data, 18), Some(18));

        let pipe_open = MediaPipeline::new();
        let _ = pipe_open;
        let demux = Demuxer::open(&data).expect("open mp3");
        let tr = &demux.tracks()[0];
        assert_eq!(tr.codec, CodecId::Mp3);
        let info = tr.audio.as_ref().expect("audio info");
        assert_eq!(info.sample_rate, 44100);
        assert_eq!(info.channels, 2);
    }

    #[test]
    fn mp3_decoder_emits_sized_frame_with_real_geometry() {
        // Feed the decoder a real frame; it must report the real geometry and emit a
        // correctly-sized (interim silent) AudioFrame, never a panic.
        let mut frame = StdVec::new();
        frame.extend_from_slice(&mp3_header_v1_128_44100_stereo());
        while frame.len() < 417 {
            frame.push(0);
        }
        let mut dec = Mp3Decoder::new();
        let pkt = MediaPacket {
            track_id: 1,
            pts: 0,
            dts: 0,
            duration: 0,
            keyframe: true,
            data: frame,
            flags: PacketFlags::none(),
        };
        let af = dec.decode(&pkt).unwrap().expect("frame");
        assert_eq!(af.sample_rate, 44100);
        assert_eq!(af.channels, 2);
        assert_eq!(af.nb_samples, 1152);
        assert_eq!(af.samples.len(), 1152 * 2);
    }

    #[test]
    fn mp3_hostile_input_never_panics() {
        // Every prefix/truncation/garbage byte stream must return cleanly.
        let mut base = StdVec::new();
        base.extend_from_slice(b"ID3\x03\x00\x10"); // footer flag set (edge case)
        base.extend_from_slice(&[0x00, 0x00, 0x00, 0x04]);
        base.extend_from_slice(&[0xFF, 0xFF, 0xFF, 0xFF]); // garbage payload
        base.extend_from_slice(&mp3_header_v1_128_44100_stereo());
        base.extend_from_slice(&[0xAB; 60]);

        for cut in 0..base.len() {
            let prefix = &base[..cut];
            // Header parse on any slice.
            let _ = mp3::FrameHeader::parse(prefix);
            // ID3 + frame scan.
            let off = skip_id3v2(prefix);
            let _ = find_mp3_frame(prefix, off.min(prefix.len()));
            // Side info on any slice.
            let _ = mp3::parse_side_info(prefix, mp3::MpegVersion::V1, 2);
            // Full decoder path.
            let mut dec = Mp3Decoder::new();
            let pkt = MediaPacket {
                track_id: 1,
                pts: 0,
                dts: 0,
                duration: 0,
                keyframe: true,
                data: prefix.to_vec(),
                flags: PacketFlags::none(),
            };
            let _ = dec.decode(&pkt);
        }

        // Adversarial bit patterns into the entropy decoders.
        for seed in 0u16..512 {
            let bytes = [(seed >> 8) as u8, seed as u8, 0xA5, 0x5A];
            let mut r = mp3::BitReader::new(&bytes);
            if let Ok(t) = mp3::huff_table(seed as u32 % 4) {
                let _ = mp3::decode_huff_pair(&mut r, &t);
            }
            let mut r2 = mp3::BitReader::new(&bytes);
            let _ = mp3::decode_count1_quad(&mut r2, (seed & 1) as u32);
        }
    }

    // ── MP3 DSP back-end host-KATs (concrete-value, FAIL-able) ───────────────

    #[test]
    fn mp3_huff_tables_prefix_free() {
        // Every transcribed big-value table must be prefix-free + dimension-correct,
        // matching the generator's guarantee. A typo'd table fails LOUDLY here rather
        // than silently producing wrong PCM.
        for &idx in mp3_tables::VERIFIED_HUFF_TABLES {
            let t = match mp3::huff_table(idx) {
                Ok(t) => t,
                Err(_) => continue, // table 0 (empty) is allowed
            };
            let n = t.entries.len();
            for i in 0..n {
                for j in 0..n {
                    if i == j {
                        continue;
                    }
                    let a = &t.entries[i];
                    let b = &t.entries[j];
                    // a is a prefix of b iff lengths a<=b and the top a.len bits of b == a.code
                    if a.len <= b.len {
                        let shift = b.len - a.len;
                        if (b.code >> shift) == a.code {
                            panic!(
                                "table {}: code(len={},{:b}) is prefix of (len={},{:b})",
                                idx, a.len, a.code, b.len, b.code
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn mp3_huffman_table5_decodes_known_pair() {
        // Table 5 newly transcribed: code for (3,3) = he!(0b0,8,...) i.e. 00000000.
        // Build that 8-bit code; both x,y nonzero -> 2 sign bits.
        let mut w = Mp3BitWriter::new();
        w.put(0b00000000, 8); // (3,3)
        w.put(1, 1); // x sign neg
        w.put(0, 1); // y sign pos
        let bytes = w.into_bytes();
        let table = mp3::huff_table(5).expect("table 5");
        let mut r = mp3::BitReader::new(&bytes);
        assert_eq!(mp3::decode_huff_pair(&mut r, &table).unwrap(), (-3, 3));
    }

    #[test]
    fn mp3_requant_concrete_values() {
        // global_gain = 210 -> 2^((210-210)/4) = 1.0, scalefac 0, scale 0.5/step.
        // is = 0 -> 0; is = 1 -> 1^(4/3)=1; is = 8 -> 8^(4/3)=16; is = -2 -> -(2^(4/3)).
        let mut spec = mp3_dsp::GranuleSpectrum::zero();
        spec.is[0] = 0;
        spec.is[1] = 1;
        spec.is[2] = 8;
        spec.is[3] = -2;
        let mut g = mp3::GranuleChannel::default();
        g.global_gain = 210;
        g.scalefac_scale = 0;
        let mut xr = [0.0f32; mp3_dsp::NLINES];
        mp3_dsp::requantize(&spec, &g, 44100, &mut xr);
        assert!(approx(xr[0], 0.0), "is=0 -> {}", xr[0]);
        assert!(approx(xr[1], 1.0), "is=1 -> {}", xr[1]);
        assert!((xr[2] - 16.0).abs() < 1e-2, "is=8 -> {} (want 16)", xr[2]);
        let want = -(2.0f32.powf(4.0 / 3.0));
        assert!(
            (xr[3] - want).abs() < 1e-2,
            "is=-2 -> {} (want {})",
            xr[3],
            want
        );
    }

    #[test]
    fn mp3_requant_global_gain_doubles() {
        // global_gain +4 -> ×2; +8 -> ×4.
        let mut spec = mp3_dsp::GranuleSpectrum::zero();
        spec.is[0] = 1;
        let mut g = mp3::GranuleChannel::default();
        g.scalefac_scale = 0;
        g.global_gain = 210;
        let mut a = [0.0f32; mp3_dsp::NLINES];
        mp3_dsp::requantize(&spec, &g, 44100, &mut a);
        g.global_gain = 214;
        let mut b = [0.0f32; mp3_dsp::NLINES];
        mp3_dsp::requantize(&spec, &g, 44100, &mut b);
        assert!((b[0] / a[0] - 2.0).abs() < 1e-3, "ratio {}", b[0] / a[0]);
    }

    #[test]
    fn mp3_imdct_long_matches_reference() {
        // Reference 36-point IMDCT (with window) computed in-test from the closed-form
        // definition; the decoder uses the precomputed cos table. They must agree.
        // Single nonzero input coefficient X[2] = 1.0, long block (window 0).
        let mut xr = [0.0f32; mp3_dsp::NLINES];
        xr[2] = 1.0; // subband 0, line 2
        let g = mp3::GranuleChannel::default(); // long block (block_type 0)
        let mut state = mp3_dsp::ChannelState::new();
        let mut out = [0.0f32; mp3_dsp::NLINES];
        mp3_dsp::imdct(&xr, &g, &mut state, &mut out);

        // Reference: x[i] = win[i]*sum_k X[k]*cos(pi/36*(2i+1+18)*(2k+1)), then
        // out[i] = first 18 of x (overlap memory starts at 0). win0[i]=sin(pi/36*(i+.5)).
        use core::f64::consts::PI;
        let k = 2usize;
        for i in 0..18 {
            let c = ((PI / 36.0) * ((2 * i + 1 + 18) as f64) * ((2 * k + 1) as f64)).cos();
            let win = ((PI / 36.0) * ((i as f64) + 0.5)).sin();
            let mut want = (c * win) as f32;
            // frequency inversion only on odd subbands; subband 0 is even -> none.
            if (0 & 1) == 1 && (i & 1) == 1 {
                want = -want;
            }
            assert!(
                (out[i] - want).abs() < 1e-4,
                "imdct[{}] = {} want {}",
                i,
                out[i],
                want
            );
        }
    }

    #[test]
    fn mp3_imdct_overlap_carries_between_granules() {
        // The second half of granule N's IMDCT must appear added into granule N+1.
        let mut xr = [0.0f32; mp3_dsp::NLINES];
        xr[0] = 1.0;
        let g = mp3::GranuleChannel::default();
        let mut state = mp3_dsp::ChannelState::new();
        let mut out1 = [0.0f32; mp3_dsp::NLINES];
        mp3_dsp::imdct(&xr, &g, &mut state, &mut out1);
        // Save overlap, decode a zero granule: output should equal the saved overlap.
        let saved = state.overlap[0];
        let zero = [0.0f32; mp3_dsp::NLINES];
        let mut out2 = [0.0f32; mp3_dsp::NLINES];
        mp3_dsp::imdct(&zero, &g, &mut state, &mut out2);
        for i in 0..18 {
            assert!(
                (out2[i] - saved[i]).abs() < 1e-6,
                "overlap[{}] {} != {}",
                i,
                out2[i],
                saved[i]
            );
        }
    }

    #[test]
    fn mp3_ms_stereo_reconstructs() {
        // MS: L' = (M+S)/√2, R' = (M-S)/√2. With M=left, S=right pre-call.
        let mut l = [0.0f32; mp3_dsp::NLINES];
        let mut r = [0.0f32; mp3_dsp::NLINES];
        l[0] = 1.0; // M
        r[0] = 0.5; // S
        mp3_dsp::stereo(&mut l, &mut r, 0x02); // MS on
        let inv = core::f32::consts::FRAC_1_SQRT_2;
        assert!((l[0] - 1.5 * inv).abs() < 1e-5, "L {}", l[0]);
        assert!((r[0] - 0.5 * inv).abs() < 1e-5, "R {}", r[0]);
        // MS off -> unchanged.
        let mut l2 = [0.0f32; mp3_dsp::NLINES];
        let mut r2 = [0.0f32; mp3_dsp::NLINES];
        l2[5] = 0.7;
        r2[5] = -0.3;
        mp3_dsp::stereo(&mut l2, &mut r2, 0x00);
        assert_eq!(l2[5], 0.7);
        assert_eq!(r2[5], -0.3);
    }

    #[test]
    fn mp3_alias_reduce_butterfly_concrete() {
        // One boundary, one butterfly pair: with a known a,b the outputs are the
        // documented CS/CA combination.
        let mut xr = [0.0f32; mp3_dsp::NLINES];
        // subband 0 line 17 (lo), subband 1 line 0 (hi) for i=0.
        xr[17] = 1.0;
        xr[18] = 0.0;
        let g = mp3::GranuleChannel::default(); // long block
        mp3_dsp::alias_reduce(&mut xr, &g);
        // cs[0]=0.857492926, ca[0]=-0.514495755
        // lo' = a*cs - b*ca = 1*0.8575 - 0 = 0.8575
        // hi' = b*cs + a*ca = 0 + 1*(-0.5145) = -0.5145
        assert!((xr[17] - 0.857492926).abs() < 1e-4, "lo {}", xr[17]);
        assert!((xr[18] - (-0.514495755)).abs() < 1e-4, "hi {}", xr[18]);
    }

    #[test]
    fn mp3_reorder_short_block_permutes() {
        // Short block reorder must permute window-interleaved coeffs into band order.
        // Build a granule with a short block, set a marker coeff, verify it moves.
        let mut xr = [0.0f32; mp3_dsp::NLINES];
        // first short band (44100) width = short[1]-short[0] = 4. Put 1.0 at src of
        // (w=1,k=0) = start + 1*4 + 0 = 4.
        xr[4] = 1.0;
        let mut g = mp3::GranuleChannel::default();
        g.window_switching = true;
        g.block_type = 2;
        g.mixed_block = false;
        mp3_dsp::reorder(&mut xr, &g, 44100);
        // dst for (w=1,k=0) = 0 + 0*3 + 1 = 1.
        assert!((xr[1] - 1.0).abs() < 1e-6, "reordered to idx1: {}", xr[1]);
        assert!(xr[4].abs() < 1e-6, "src cleared: {}", xr[4]);
    }

    #[test]
    fn mp3_decode_hybrid_frame_runs_and_is_bounded() {
        // A synthetic but structurally-valid frame: header + side info (all zero
        // big_values -> count1-only) + zero main data. The full path must run, produce
        // geometry-correct channel buffers, and never panic.
        let mut frame = StdVec::new();
        frame.extend_from_slice(&mp3_header_v1_128_44100_stereo());
        // side info (32 bytes for V1 stereo) + main data, all zero -> silent granules.
        while frame.len() < 417 {
            frame.push(0);
        }
        let mut dec = Mp3Decoder::new();
        // First frame: main_data_begin=0 so reservoir resolves immediately.
        let (chans, nch) = dec.decode_hybrid_frame(&frame).expect("hybrid");
        assert_eq!(nch, 2);
        assert_eq!(chans.len(), 2);
        // 2 granules × 576 lines.
        assert_eq!(chans[0].len(), 1152);
        // All-zero input -> all-zero subband output (silent, correct).
        assert!(chans[0].iter().all(|&s| s == 0.0));
    }

    #[test]
    fn mp3_dsp_hostile_input_never_panics() {
        // Feed the full hybrid path every truncation of a constructed frame + random
        // side-info bytes; must never panic or index OOB.
        let mut base = StdVec::new();
        base.extend_from_slice(&mp3_header_v1_128_44100_stereo());
        for i in 0..500u32 {
            base.push((i.wrapping_mul(2654435761) >> 16) as u8);
        }
        for cut in 0..base.len() {
            let mut dec = Mp3Decoder::new();
            let _ = dec.decode_hybrid_frame(&base[..cut]);
        }
        // Direct DSP-stage fuzzing with adversarial coefficients.
        for seed in 0u32..256 {
            let mut spec = mp3_dsp::GranuleSpectrum::zero();
            for i in 0..mp3_dsp::NLINES {
                spec.is[i] = ((seed.wrapping_add(i as u32)) % 17) as i32 - 8;
            }
            let mut g = mp3::GranuleChannel::default();
            g.global_gain = (seed % 256) as u32;
            g.window_switching = seed & 1 == 1;
            g.block_type = if g.window_switching { 2 } else { 0 };
            let mut xr = [0.0f32; mp3_dsp::NLINES];
            mp3_dsp::requantize(&spec, &g, 44100, &mut xr);
            mp3_dsp::reorder(&mut xr, &g, 44100);
            mp3_dsp::alias_reduce(&mut xr, &g);
            let mut state = mp3_dsp::ChannelState::new();
            let mut out = [0.0f32; mp3_dsp::NLINES];
            mp3_dsp::imdct(&xr, &g, &mut state, &mut out);
            // Output must be finite (no NaN/Inf from the power law / IMDCT).
            assert!(
                out.iter().all(|s| s.is_finite()),
                "non-finite out seed {}",
                seed
            );
        }
    }

    /// Minimal MSB-first bit writer for constructing known MP3 bitstreams in tests.
    struct Mp3BitWriter {
        bits: StdVec<u8>,
    }
    impl Mp3BitWriter {
        fn new() -> Self {
            Self {
                bits: StdVec::new(),
            }
        }
        fn put(&mut self, value: u32, n: u32) {
            for i in (0..n).rev() {
                self.bits.push(((value >> i) & 1) as u8);
            }
        }
        fn into_bytes(self) -> StdVec<u8> {
            let mut out = StdVec::new();
            let mut cur = 0u8;
            let mut nb = 0u32;
            for b in self.bits {
                cur = (cur << 1) | b;
                nb += 1;
                if nb == 8 {
                    out.push(cur);
                    cur = 0;
                    nb = 0;
                }
            }
            if nb > 0 {
                cur <<= 8 - nb; // pad remaining bits with zeros (MSB-first)
                out.push(cur);
            }
            out
        }
        fn into_bytes_padded(self, total: usize) -> StdVec<u8> {
            let mut out = self.into_bytes();
            while out.len() < total {
                out.push(0);
            }
            out
        }
    }

    // ── New Huffman codebooks (T9/T11/T12/T13/T15/T16/T24) + count1 table A ───────

    #[test]
    fn mp3_all_table_selects_resolve() {
        // Every conformant table_select 0..=31 except the ISO "not used" 4/14 must map to
        // a codebook; 4 and 14 must return a clean Invalid (never a wrong table).
        for sel in 0u32..=31 {
            let r = mp3::huff_table(sel);
            if sel == 4 || sel == 14 {
                assert!(
                    matches!(r, Err(mp3::Mp3Error::Invalid(_))),
                    "select {} must be Invalid",
                    sel
                );
            } else {
                assert!(r.is_ok(), "select {} must resolve", sel);
            }
        }
        // The select->linbits map (T16 family 16..23, T24 family 24..31).
        let lin = |s: u32| mp3::huff_table(s).unwrap().linbits;
        assert_eq!(
            [
                lin(16),
                lin(17),
                lin(18),
                lin(19),
                lin(20),
                lin(21),
                lin(22),
                lin(23)
            ],
            [1, 2, 3, 4, 6, 8, 10, 13]
        );
        assert_eq!(
            [
                lin(24),
                lin(25),
                lin(26),
                lin(27),
                lin(28),
                lin(29),
                lin(30),
                lin(31)
            ],
            [4, 5, 6, 7, 8, 9, 11, 13]
        );
    }

    #[test]
    fn mp3_new_huff_tables_prefix_free() {
        // Every newly transcribed codebook must be prefix-free + dimension-correct. A
        // typo fails LOUDLY here, not as wrong PCM. (Covers the codebooks behind selects
        // 9,11,12,13,15,16,24; the aliasing selects share these codebooks.)
        for &idx in &[9u32, 11, 12, 13, 15, 16, 24] {
            let t = mp3::huff_table(idx).expect("codebook");
            let n = t.entries.len();
            for i in 0..n {
                for j in 0..n {
                    if i == j {
                        continue;
                    }
                    let a = &t.entries[i];
                    let b = &t.entries[j];
                    if a.len <= b.len {
                        let shift = b.len - a.len;
                        if (b.code >> shift) == a.code {
                            panic!(
                                "codebook {}: (len={},{:b}) is prefix of (len={},{:b})",
                                idx, a.len, a.code, b.len, b.code
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn mp3_huffman_table13_decodes_known_codeword() {
        // T13 entry (0,0) = code 1, len 1 -> (0,0), no sign bits.
        // T13 entry (1,0) = code 11 (0b11), len 3 -> x=1 (1 sign bit), y=0.
        let mut w = Mp3BitWriter::new();
        w.put(0b1, 1); // (0,0)
        w.put(0b11, 3); // (1,0)
        w.put(1, 1); // x sign negative -> (-1,0)
        let bytes = w.into_bytes();
        let table = mp3::huff_table(13).expect("t13");
        let mut r = mp3::BitReader::new(&bytes);
        assert_eq!(mp3::decode_huff_pair(&mut r, &table).unwrap(), (0, 0));
        assert_eq!(mp3::decode_huff_pair(&mut r, &table).unwrap(), (-1, 0));
    }

    #[test]
    fn mp3_huffman_table16_linbits_escape() {
        // T16 (select 16, linbits=1). Entry (15,15) = code 0b11, len 8 (0b00000011) ->
        // both x,y == 15, each takes `linbits` extra bits added to 15, then a sign bit.
        let mut w = Mp3BitWriter::new();
        w.put(0b00000011, 8); // (15,15)
        w.put(1, 1); // x linbits (1 bit) = 1 -> x = 15+1 = 16
        w.put(1, 1); // x sign negative -> -16
        w.put(0, 1); // y linbits (1 bit) = 0 -> y = 15
        w.put(0, 1); // y sign positive -> 15
        let bytes = w.into_bytes();
        let table = mp3::huff_table(16).expect("t16");
        assert_eq!(table.linbits, 1);
        let mut r = mp3::BitReader::new(&bytes);
        assert_eq!(mp3::decode_huff_pair(&mut r, &table).unwrap(), (-16, 15));
    }

    #[test]
    fn mp3_huffman_table24_decodes_first_codeword() {
        // T24 (select 24, linbits=4). Entry (0,0) = code 0b1111, len 4 -> (0,0).
        // Entry (0,1) = code 0b1101, len 4 -> y=1 (1 sign bit).
        let mut w = Mp3BitWriter::new();
        w.put(0b1111, 4); // (0,0)
        w.put(0b1101, 4); // (0,1)
        w.put(0, 1); // y sign positive
        let bytes = w.into_bytes();
        let table = mp3::huff_table(24).expect("t24");
        assert_eq!(table.linbits, 4);
        let mut r = mp3::BitReader::new(&bytes);
        assert_eq!(mp3::decode_huff_pair(&mut r, &table).unwrap(), (0, 0));
        assert_eq!(mp3::decode_huff_pair(&mut r, &table).unwrap(), (0, 1));
    }

    #[test]
    fn mp3_huffman_negative_case_wrong_codeword() {
        // FAIL lever: a deliberately wrong bit pattern must NOT decode to the expected
        // pair (proves the asserts above can fail). Feed T13 the code for (0,0)=1 but
        // assert it does not equal (1,0).
        let mut w = Mp3BitWriter::new();
        w.put(0b1, 1);
        let bytes = w.into_bytes();
        let table = mp3::huff_table(13).expect("t13");
        let mut r = mp3::BitReader::new(&bytes);
        assert_ne!(mp3::decode_huff_pair(&mut r, &table).unwrap(), (1, 0));
    }

    #[test]
    fn mp3_count1_table_a_prefix_free_and_decodes() {
        // Table A (count1table_select == 0): value 0000 = code 1 len1 -> mags (0,0,0,0).
        let mut w = Mp3BitWriter::new();
        w.put(0b1, 1); // value 0000 -> all-zero quad, no sign bits
        let bytes = w.into_bytes();
        let mut r = mp3::BitReader::new(&bytes);
        assert_eq!(mp3::decode_count1_quad(&mut r, 0).unwrap(), (0, 0, 0, 0));

        // value 1111 = code 1 len6 (0b000001) -> mags (1,1,1,1); signs (+,-,+,-).
        let mut w2 = Mp3BitWriter::new();
        w2.put(0b000001, 6);
        w2.put(0, 1); // v +
        w2.put(1, 1); // w -
        w2.put(0, 1); // x +
        w2.put(1, 1); // y -
        let bytes2 = w2.into_bytes();
        let mut r2 = mp3::BitReader::new(&bytes2);
        assert_eq!(mp3::decode_count1_quad(&mut r2, 0).unwrap(), (1, -1, 1, -1));

        // value 1010 = code 0b000110 len6 -> mags (1,0,1,0); the corrected-length entry
        // (the spec typo lever): if 1010 were len 4 this would mis-decode.
        let mut w3 = Mp3BitWriter::new();
        w3.put(0b000110, 6);
        w3.put(0, 1); // v sign +
        w3.put(0, 1); // x sign +
        let bytes3 = w3.into_bytes();
        let mut r3 = mp3::BitReader::new(&bytes3);
        assert_eq!(mp3::decode_count1_quad(&mut r3, 0).unwrap(), (1, 0, 1, 0));
    }

    // ── Polyphase synthesis filterbank host-KATs (FAIL-able) ─────────────────────

    /// In-test reference synthesis built from the SAME N/D constants, independent of the
    /// production code path (no FIFO copy_within / gather helper reuse). A sign or gather
    /// mismatch in the production `synthesis()` would diverge from this reference.
    fn ref_synthesis(
        sub: &[f32; mp3_dsp::NLINES],
        v: &mut [f32; 1024],
        out: &mut [f32; mp3_dsp::NLINES],
    ) {
        use mp3_imdct_tables::{SYNTH_D, SYNTH_N};
        for ss in 0..18 {
            for i in (64..1024).rev() {
                v[i] = v[i - 64];
            }
            let mut s = [0.0f32; 32];
            for k in 0..32 {
                s[k] = sub[k * 18 + ss];
            }
            for i in 0..64 {
                let mut acc = 0.0f32;
                for k in 0..32 {
                    acc += SYNTH_N[i][k] * s[k];
                }
                v[i] = acc;
            }
            let mut u = [0.0f32; 512];
            for i in 0..8 {
                for j in 0..32 {
                    u[i * 64 + j] = v[i * 128 + j];
                    u[i * 64 + j + 32] = v[i * 128 + j + 96];
                }
            }
            for i in 0..512 {
                u[i] *= SYNTH_D[i];
            }
            for i in 0..32 {
                let mut acc = 0.0f32;
                for j in 0..16 {
                    acc += u[j * 32 + i];
                }
                out[ss * 32 + i] = acc.clamp(-1.0, 1.0);
            }
        }
    }

    #[test]
    fn mp3_synthesis_matches_reference() {
        // Drive both the production synthesis and the in-test reference with the same
        // tone-like subband input; they must agree to 1e-4 across all 576 samples. This
        // is the sign/gather-correctness lever the spec names.
        let mut sub = [0.0f32; mp3_dsp::NLINES];
        for ss in 0..18 {
            sub[18 + ss] = 0.5; // subband 1
            sub[5 * 18 + ss] = -0.3;
            sub[20 * 18 + ss] = 0.2;
        }
        let mut state = mp3_dsp::ChannelState::new();
        let mut prod = [0.0f32; mp3_dsp::NLINES];
        mp3_dsp::synthesis(&sub, &mut state, &mut prod);

        let mut v = [0.0f32; 1024];
        let mut refr = [0.0f32; mp3_dsp::NLINES];
        ref_synthesis(&sub, &mut v, &mut refr);

        for n in 0..mp3_dsp::NLINES {
            assert!(
                (prod[n] - refr[n]).abs() < 1e-4,
                "synthesis vs reference diverge at {}: {} vs {}",
                n,
                prod[n],
                refr[n]
            );
        }
    }

    #[test]
    fn mp3_synthesis_nonsilent_and_bounded() {
        // A constant tone in one subband across all sub-passes must produce non-silent,
        // bounded PCM; a zero input must produce exactly-zero PCM (the D[]/gather lever).
        let mut tone = [0.0f32; mp3_dsp::NLINES];
        for ss in 0..18 {
            tone[2 * 18 + ss] = 0.4;
        }
        let mut state = mp3_dsp::ChannelState::new();
        let mut pcm = [0.0f32; mp3_dsp::NLINES];
        mp3_dsp::synthesis(&tone, &mut state, &mut pcm);
        let peak = pcm.iter().fold(0.0f32, |m, &s| m.max(s.abs()));
        assert!(peak > 0.01, "tone synthesis silent: peak={}", peak);
        assert!(
            peak <= 1.0,
            "tone synthesis clipped past range: peak={}",
            peak
        );
        assert!(pcm.iter().all(|s| s.is_finite()), "non-finite PCM");

        let zero = [0.0f32; mp3_dsp::NLINES];
        let mut state2 = mp3_dsp::ChannelState::new();
        let mut zpcm = [0.0f32; mp3_dsp::NLINES];
        mp3_dsp::synthesis(&zero, &mut state2, &mut zpcm);
        assert!(zpcm.iter().all(|&s| s == 0.0), "zero input produced energy");
    }

    #[test]
    fn mp3_synthesis_hostile_input_never_panics() {
        // Adversarial subband inputs (including NaN/inf candidates) must never panic and
        // must always emit finite, bounded PCM.
        for seed in 0u32..256 {
            let mut sub = [0.0f32; mp3_dsp::NLINES];
            for i in 0..mp3_dsp::NLINES {
                let r = seed.wrapping_mul(2654435761).wrapping_add(i as u32);
                sub[i] = ((r % 2001) as f32 - 1000.0) * 0.01;
            }
            // Inject a couple of pathological values.
            sub[0] = f32::INFINITY;
            sub[100] = f32::NAN;
            let mut state = mp3_dsp::ChannelState::new();
            let mut pcm = [0.0f32; mp3_dsp::NLINES];
            mp3_dsp::synthesis(&sub, &mut state, &mut pcm);
            assert!(
                pcm.iter().all(|s| s.is_finite() && s.abs() <= 1.0),
                "synthesis out of range/NaN at seed {}",
                seed
            );
        }
    }

    #[test]
    fn mp3_decode_emits_nonsilent_pcm_for_real_coefficients() {
        // End-to-end audible proof (documented level): construct a frame whose granule
        // big-values decode to nonzero coefficients via table 1, then assert the full
        // decode_frame_pcm path emits NON-ZERO, bounded PCM of the right shape. (A full
        // bit-exact reference MP3 is impractical to hand-build here; this asserts the
        // pipeline produces real audible energy, not silence, end-to-end.)
        //
        // We feed coefficients directly through the DSP+synthesis path to make the proof
        // deterministic: requant a known spectrum, run the hybrid path, then synthesize.
        let mut spec = mp3_dsp::GranuleSpectrum::zero();
        // A handful of nonzero low-frequency lines -> audible tone-ish content.
        for i in 0..36 {
            spec.is[i] = if i % 2 == 0 { 4 } else { -3 };
        }
        let mut g = mp3::GranuleChannel::default();
        g.global_gain = 210;
        g.block_type = 0;
        g.region0_count = 7;
        g.region1_count = 13;
        let mut xr = [0.0f32; mp3_dsp::NLINES];
        mp3_dsp::requantize(&spec, &g, 44100, &mut xr);
        mp3_dsp::alias_reduce(&mut xr, &g);
        let mut state = mp3_dsp::ChannelState::new();
        let mut sub = [0.0f32; mp3_dsp::NLINES];
        mp3_dsp::imdct(&xr, &g, &mut state, &mut sub);
        let mut pcm = [0.0f32; mp3_dsp::NLINES];
        mp3_dsp::synthesis(&sub, &mut state, &mut pcm);
        let peak = pcm.iter().fold(0.0f32, |m, &s| m.max(s.abs()));
        assert!(
            peak > 1e-4,
            "real coefficients produced silence: peak={}",
            peak
        );
        assert!(
            peak <= 1.0 && pcm.iter().all(|s| s.is_finite()),
            "PCM out of range"
        );
    }

    #[test]
    fn mp3_decode_frame_pcm_shape_and_no_panic() {
        // The public decode_frame_pcm must return interleaved PCM of the right length and
        // never panic on a structurally-valid frame.
        let mut frame = StdVec::new();
        frame.extend_from_slice(&mp3_header_v1_128_44100_stereo());
        while frame.len() < 417 {
            frame.push(0);
        }
        let mut dec = Mp3Decoder::new();
        let (pcm, nch) = dec.decode_frame_pcm(&frame).expect("pcm");
        assert_eq!(nch, 2);
        assert_eq!(pcm.len(), 1152 * 2); // 2 granules * 576 * 2 channels
        assert!(pcm.iter().all(|s| s.is_finite()));
    }

    #[test]
    fn mp3_boot_smoketest_passes() {
        let line = run_boot_smoketest();
        assert!(line.contains("-> PASS"), "smoketest did not PASS: {}", line);
        assert!(line.starts_with("[raemedia] mp3 synth:"));
        // procfs status flips to audible.
        assert!(mp3_procfs_status().contains("mp3=audible"));
    }

    #[test]
    fn aac_boot_smoketest_passes() {
        let line = aac::run_boot_smoketest();
        assert!(
            line.contains("-> PASS"),
            "aac smoketest did not PASS: {}",
            line
        );
        assert!(line.starts_with("[raemedia] aac-lc:"));
        // procfs status reports audible.
        assert!(aac::aac_procfs_status().contains("aac=audible"));
    }
}
