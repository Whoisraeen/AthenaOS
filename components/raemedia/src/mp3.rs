//! From-scratch native MPEG-1/2/2.5 Audio Layer III (MP3) frame parser + entropy
//! decoder. Concept §creators/media: *"play my music"* — MP3 is the dominant music
//! format; before this, raemedia only produced sound from WAV + FLAC and the MP3
//! path was header-parse + silence.
//!
//! SCOPE (honest — a full Layer III decoder is large; see the crate REPORT):
//! IMPLEMENTED + host-KAT'd here:
//! - Frame sync + header: MPEG version (1/2/2.5), Layer III gate, bitrate +
//!   sample-rate tables, padding, channel mode, CRC-present flag, computed
//!   frame_size, samples-per-frame, granule count, side-info size.
//! - Side information: main_data_begin (the bit-reservoir back-pointer), scfsi,
//!   and per-granule per-channel part2_3_length, big_values, global_gain,
//!   scalefac_compress, window switching + block_type + mixed flag, table selects,
//!   subblock_gain, region counts, preflag, scalefac_scale, count1table_select —
//!   for both MPEG-1 (2 granules) and MPEG-2/2.5 LSF (1 granule).
//! - Bit reservoir assembly: main_data spanning earlier frames via main_data_begin.
//! - Bounds-checked MSB-first bit reader (the untrusted-input boundary).
//! - Huffman big-values decode for the generator-verified tables {1,2,3,5,6,7,8,10}
//!   (`mp3_tables.rs`) + the linbits/sign machinery, the count1 region table B, and
//!   `decode_huffman_region` → the full `is[576]` spectrum.
//!
//! DSP BACK-END (landed this slice — see `mp3_dsp.rs`, host-KAT'd): scalefactor decode
//! (MPEG-1 long/short/mixed), requantization (2^(global_gain/4)·2^(-scalefac)·|is|^(4/3),
//! no-libm), short-block reordering, MS-stereo, alias reduction, and the IMDCT (36-pt
//! long / 12-pt short) + windowing + cross-granule overlap-add (the hybrid filterbank).
//! `Mp3Decoder::decode_hybrid_frame` runs the whole entropy+DSP path end-to-end and
//! maintains the overlap/scfsi state.
//!
//! DEFERRED (documented, NOT silently wrong — clean error / clean silence, never wrong
//! PCM):
//! - The polyphase synthesis filterbank (32 subbands → 32 PCM via the ISO Table B.3
//!   512-tap D[] prototype window). That window is a fixed, non-closed-form table not
//!   transcribed here; until it lands the public `AudioFrame` stays correctly-silent
//!   rather than emitting un-synthesized samples. This is the one stage between the
//!   working hybrid filterbank and audible PCM.
//! - Big-values tables {4,9,11,12,13–31} + count1 table A (region decodes to silence,
//!   not wrong coefficients); LSF scalefactor decode + intensity-stereo position.
//!
//! Untrusted-input discipline: every bitstream read is bounds-checked; a truncated
//! or corrupt frame returns a clean `Err`, never a panic or OOB index. Decoders are
//! the #1 RCE surface (CLAUDE media quality bar) — this is the parser boundary.
//!
//! Host-KAT'd in `lib.rs` against constructed known frames (concrete header fields,
//! concrete Huffman coefficients, hostile-input never-panics). No external
//! dependency — pure `#![no_std]` + `alloc`.

use alloc::vec::Vec;

// ─── Error type ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mp3Error {
    /// Ran off the end of the bitstream (truncated input).
    UnexpectedEof,
    /// No valid frame sync at the expected position.
    NoSync,
    /// A structural field held a forbidden / unsupported value.
    Invalid(&'static str),
    /// Not enough main-data buffered yet (reservoir back-reference unsatisfied).
    NeedMoreData,
}

// ─── MSB-first bounds-checked bit reader ────────────────────────────────────

/// MSB-first bit reader. Every accessor returns `Err(UnexpectedEof)` rather than
/// panicking when the stream is exhausted — the untrusted-input boundary.
pub struct BitReader<'a> {
    data: &'a [u8],
    bitpos: usize,
}

impl<'a> BitReader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, bitpos: 0 }
    }

    pub fn with_pos(data: &'a [u8], bitpos: usize) -> Self {
        Self { data, bitpos }
    }

    #[inline]
    pub fn bit_position(&self) -> usize {
        self.bitpos
    }

    #[inline]
    pub fn set_position(&mut self, bitpos: usize) {
        self.bitpos = bitpos;
    }

    #[inline]
    fn total_bits(&self) -> usize {
        self.data.len() * 8
    }

    #[inline]
    pub fn bits_left(&self) -> usize {
        self.total_bits().saturating_sub(self.bitpos)
    }

    #[inline]
    pub fn read_bit(&mut self) -> Result<u32, Mp3Error> {
        if self.bitpos >= self.total_bits() {
            return Err(Mp3Error::UnexpectedEof);
        }
        let byte = self.data[self.bitpos >> 3];
        let shift = 7 - (self.bitpos & 7);
        self.bitpos += 1;
        Ok(((byte >> shift) & 1) as u32)
    }

    /// Read `n` bits (0..=32) MSB-first into a u32.
    pub fn read_bits(&mut self, n: u32) -> Result<u32, Mp3Error> {
        if n == 0 {
            return Ok(0);
        }
        if n > 32 {
            return Err(Mp3Error::Invalid("read_bits > 32"));
        }
        if self.bitpos + n as usize > self.total_bits() {
            return Err(Mp3Error::UnexpectedEof);
        }
        let mut v: u32 = 0;
        for _ in 0..n {
            let byte = self.data[self.bitpos >> 3];
            let shift = 7 - (self.bitpos & 7);
            v = (v << 1) | (((byte >> shift) & 1) as u32);
            self.bitpos += 1;
        }
        Ok(v)
    }
}

// ─── Header tables ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MpegVersion {
    V1,
    V2,
    V25,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelMode {
    Stereo,
    JointStereo,
    DualChannel,
    Mono,
}

/// Parsed Layer III frame header (the 4 sync bytes).
#[derive(Debug, Clone, Copy)]
pub struct FrameHeader {
    pub version: MpegVersion,
    pub layer: u8,
    pub protection: bool, // true = CRC present
    pub bitrate: u32,     // bits/sec
    pub sample_rate: u32, // Hz
    pub padding: bool,
    pub mode: ChannelMode,
    pub mode_ext: u8, // joint-stereo: bit0 = intensity, bit1 = MS
    /// Total bytes in the frame (incl. the 4-byte header + optional CRC).
    pub frame_size: usize,
    /// Channels (1 for mono, else 2).
    pub channels: usize,
}

// Layer III bitrate tables (kbps). Index by bitrate_index 0..15.
const BITRATE_V1_L3: [u32; 16] = [
    0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 0,
];
const BITRATE_V2_L3: [u32; 16] = [
    0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160, 0,
];

const SAMPLE_RATE_V1: [u32; 4] = [44100, 48000, 32000, 0];
const SAMPLE_RATE_V2: [u32; 4] = [22050, 24000, 16000, 0];
const SAMPLE_RATE_V25: [u32; 4] = [11025, 12000, 8000, 0];

impl FrameHeader {
    /// Parse a 4-byte Layer III header. Returns `NoSync` if the sync word / layer is
    /// not a valid Layer III frame; `Invalid` for reserved/free-format fields.
    pub fn parse(b: &[u8]) -> Result<FrameHeader, Mp3Error> {
        if b.len() < 4 {
            return Err(Mp3Error::UnexpectedEof);
        }
        // Frame sync: 11 bits set.
        if b[0] != 0xFF || (b[1] & 0xE0) != 0xE0 {
            return Err(Mp3Error::NoSync);
        }
        let version = match (b[1] >> 3) & 0x03 {
            0 => MpegVersion::V25,
            2 => MpegVersion::V2,
            3 => MpegVersion::V1,
            _ => return Err(Mp3Error::Invalid("reserved MPEG version")),
        };
        let layer = match (b[1] >> 1) & 0x03 {
            1 => 3u8,
            2 => 2,
            3 => 1,
            _ => return Err(Mp3Error::Invalid("reserved layer")),
        };
        if layer != 3 {
            return Err(Mp3Error::Invalid("not Layer III"));
        }
        let protection = (b[1] & 0x01) == 0; // 0 = protected (CRC present)
        let br_idx = (b[2] >> 4) & 0x0F;
        if br_idx == 0x0F {
            return Err(Mp3Error::Invalid("bad bitrate index"));
        }
        let bitrate_kbps = match version {
            MpegVersion::V1 => BITRATE_V1_L3[br_idx as usize],
            _ => BITRATE_V2_L3[br_idx as usize],
        };
        if bitrate_kbps == 0 {
            return Err(Mp3Error::Invalid("free-format / zero bitrate unsupported"));
        }
        let bitrate = bitrate_kbps * 1000;
        let sr_idx = (b[2] >> 2) & 0x03;
        if sr_idx == 3 {
            return Err(Mp3Error::Invalid("bad sample rate index"));
        }
        let sample_rate = match version {
            MpegVersion::V1 => SAMPLE_RATE_V1[sr_idx as usize],
            MpegVersion::V2 => SAMPLE_RATE_V2[sr_idx as usize],
            MpegVersion::V25 => SAMPLE_RATE_V25[sr_idx as usize],
        };
        let padding = (b[2] & 0x02) != 0;
        let mode = match (b[3] >> 6) & 0x03 {
            0 => ChannelMode::Stereo,
            1 => ChannelMode::JointStereo,
            2 => ChannelMode::DualChannel,
            _ => ChannelMode::Mono,
        };
        let mode_ext = (b[3] >> 4) & 0x03;
        let channels = if mode == ChannelMode::Mono { 1 } else { 2 };

        // Frame size in bytes. V1: 144*br/sr + pad. V2/2.5: 72*br/sr + pad.
        let coeff = match version {
            MpegVersion::V1 => 144u32,
            _ => 72,
        };
        let pad = if padding { 1usize } else { 0 };
        let frame_size = (coeff * bitrate / sample_rate) as usize + pad;
        if frame_size < 4 {
            return Err(Mp3Error::Invalid("degenerate frame size"));
        }

        Ok(FrameHeader {
            version,
            layer,
            protection,
            bitrate,
            sample_rate,
            padding,
            mode,
            mode_ext,
            frame_size,
            channels,
        })
    }

    /// Samples produced per channel per frame.
    pub fn samples_per_frame(&self) -> usize {
        match self.version {
            MpegVersion::V1 => 1152,
            _ => 576,
        }
    }

    /// Number of granules (2 for MPEG-1, 1 for MPEG-2/2.5).
    pub fn granules(&self) -> usize {
        match self.version {
            MpegVersion::V1 => 2,
            _ => 1,
        }
    }

    /// Bytes of side-information after the header (+CRC).
    pub fn side_info_size(&self) -> usize {
        match (self.version, self.channels) {
            (MpegVersion::V1, 1) => 17,
            (MpegVersion::V1, _) => 32,
            (_, 1) => 9,
            (_, _) => 17,
        }
    }
}

// ─── Side information ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, Default)]
pub struct GranuleChannel {
    pub part2_3_length: u32,
    pub big_values: u32,
    pub global_gain: u32,
    pub scalefac_compress: u32,
    pub window_switching: bool,
    pub block_type: u8,
    pub mixed_block: bool,
    pub table_select: [u32; 3],
    pub subblock_gain: [u32; 3],
    pub region0_count: u32,
    pub region1_count: u32,
    pub preflag: bool,
    pub scalefac_scale: u32,
    pub count1table_select: u32,
}

#[derive(Debug, Clone)]
pub struct SideInfo {
    pub main_data_begin: u32,
    pub scfsi: [[bool; 4]; 2], // [channel][scfsi_band]
    /// [granule][channel]
    pub gr: [[GranuleChannel; 2]; 2],
}

/// Parse side information. `data` begins at the side-info byte (after header+CRC).
pub fn parse_side_info(
    data: &[u8],
    version: MpegVersion,
    channels: usize,
) -> Result<SideInfo, Mp3Error> {
    let mut r = BitReader::new(data);
    let ngr = if version == MpegVersion::V1 { 2 } else { 1 };
    let mut si = SideInfo {
        main_data_begin: 0,
        scfsi: [[false; 4]; 2],
        gr: [[GranuleChannel::default(); 2]; 2],
    };

    if version == MpegVersion::V1 {
        si.main_data_begin = r.read_bits(9)?;
        // private_bits: 5 (mono) or 3 (stereo)
        let priv_bits = if channels == 1 { 5 } else { 3 };
        let _ = r.read_bits(priv_bits)?;
        for ch in 0..channels {
            for band in 0..4 {
                si.scfsi[ch][band] = r.read_bit()? == 1;
            }
        }
    } else {
        si.main_data_begin = r.read_bits(8)?;
        let priv_bits = if channels == 1 { 1 } else { 2 };
        let _ = r.read_bits(priv_bits)?;
    }

    for gr in 0..ngr {
        for ch in 0..channels {
            let g = &mut si.gr[gr][ch];
            g.part2_3_length = r.read_bits(12)?;
            g.big_values = r.read_bits(9)?;
            if g.big_values > 288 {
                return Err(Mp3Error::Invalid("big_values out of range"));
            }
            g.global_gain = r.read_bits(8)?;
            let sfc_bits = if version == MpegVersion::V1 { 4 } else { 9 };
            g.scalefac_compress = r.read_bits(sfc_bits)?;
            g.window_switching = r.read_bit()? == 1;
            if g.window_switching {
                g.block_type = r.read_bits(2)? as u8;
                if g.block_type == 0 {
                    return Err(Mp3Error::Invalid("window switching with block_type 0"));
                }
                g.mixed_block = r.read_bit()? == 1;
                for i in 0..2 {
                    g.table_select[i] = r.read_bits(5)?;
                }
                for i in 0..3 {
                    g.subblock_gain[i] = r.read_bits(3)?;
                }
                // region0_count is implied for window-switched granules.
                if g.block_type == 2 && !g.mixed_block {
                    g.region0_count = 8;
                } else {
                    g.region0_count = 7;
                }
                g.region1_count = 36 - g.region0_count;
            } else {
                g.block_type = 0;
                for i in 0..3 {
                    g.table_select[i] = r.read_bits(5)?;
                }
                g.region0_count = r.read_bits(4)?;
                g.region1_count = r.read_bits(3)?;
            }
            g.preflag = if version == MpegVersion::V1 {
                r.read_bit()? == 1
            } else {
                // LSF preflag is derived from scalefac_compress; no bit on the wire.
                false
            };
            g.scalefac_scale = r.read_bit()?;
            g.count1table_select = r.read_bit()?;
        }
    }
    Ok(si)
}

// ─── Huffman big-values tables (verified subset) ────────────────────────────
//
// ISO/IEC 11172-3 Table B.7. Each entry is (code, len, x, y); we decode by reading
// bits MSB-first and matching the (len, code) prefix, then applying linbits escapes
// and sign bits. Only the small, exactly-transcribed tables (0–3) are provided here;
// `huff_big` returns `Invalid` for any untranscribed table rather than guessing —
// emitting wrong coefficients would be worse than a clean error (CLAUDE quality bar).

pub struct HuffEntry {
    pub code: u32,
    pub len: u8,
    pub x: u8,
    pub y: u8,
}

pub struct HuffTable {
    pub entries: &'static [HuffEntry],
    pub linbits: u32,
}

// Table 0 = all-zero region (no bits consumed). The transcribed + generator-verified
// big-value tables (1,2,3,5,6,7,8,10) live in `mp3_tables.rs`.
static T0: [HuffEntry; 0] = [];

/// Return the Huffman codebook + linbits for a `table_select` value (ISO Table B.7).
/// Every conformant select 0..=31 maps onto one of the 15 generator-verified codebooks
/// ({1,2,3,5,6,7,8,9,10,11,12,13,15,16,24}); selects 16..=23 share T16 and 24..=31
/// share T24, each with its own linbits escape width. The only `Invalid` cases are the
/// ISO "not used" selects 4 and 14 (a non-conformant stream — its big-values region
/// then decodes to clean silence, never wrong coefficients).
pub fn huff_table(index: u32) -> Result<HuffTable, Mp3Error> {
    use crate::mp3_tables as t;
    let (entries, linbits): (&'static [HuffEntry], u32) = match index {
        0 => (&T0, 0),
        1 => (&t::T1, 0),
        2 => (&t::T2, 0),
        3 => (&t::T3, 0),
        5 => (&t::T5, 0),
        6 => (&t::T6, 0),
        7 => (&t::T7, 0),
        8 => (&t::T8, 0),
        9 => (&t::T9, 0),
        10 => (&t::T10, 0),
        11 => (&t::T11, 0),
        12 => (&t::T12, 0),
        13 => (&t::T13, 0),
        15 => (&t::T15, 0),
        // T16 family: codebook T16, linbits per select (ISO Table B.7).
        16 => (&t::T16, 1),
        17 => (&t::T16, 2),
        18 => (&t::T16, 3),
        19 => (&t::T16, 4),
        20 => (&t::T16, 6),
        21 => (&t::T16, 8),
        22 => (&t::T16, 10),
        23 => (&t::T16, 13),
        // T24 family: codebook T24, linbits per select.
        24 => (&t::T24, 4),
        25 => (&t::T24, 5),
        26 => (&t::T24, 6),
        27 => (&t::T24, 7),
        28 => (&t::T24, 8),
        29 => (&t::T24, 9),
        30 => (&t::T24, 11),
        31 => (&t::T24, 13),
        // 4 and 14 are ISO "not used"; any other index is out of range.
        _ => return Err(Mp3Error::Invalid("huffman table not used / out of range")),
    };
    Ok(HuffTable { entries, linbits })
}

/// Decode one (x, y) pair from a big-values Huffman table: prefix match, then the
/// linbits escape (when |value| == 15) and sign bits. Returns the signed pair.
pub fn decode_huff_pair(r: &mut BitReader, table: &HuffTable) -> Result<(i32, i32), Mp3Error> {
    if table.entries.is_empty() {
        return Ok((0, 0));
    }
    let mut code: u32 = 0;
    let mut len: u8 = 0;
    loop {
        code = (code << 1) | r.read_bit()?;
        len += 1;
        if len > 19 {
            return Err(Mp3Error::Invalid("huffman code too long"));
        }
        for e in table.entries {
            if e.len == len && e.code == code {
                let mut x = e.x as i32;
                let mut y = e.y as i32;
                if x == 15 && table.linbits > 0 {
                    x += r.read_bits(table.linbits)? as i32;
                }
                if x != 0 && r.read_bit()? == 1 {
                    x = -x;
                }
                if y == 15 && table.linbits > 0 {
                    y += r.read_bits(table.linbits)? as i32;
                }
                if y != 0 && r.read_bit()? == 1 {
                    y = -y;
                }
                return Ok((x, y));
            }
        }
    }
}

/// count1 table A (`count1table_select == 0`, ISO t32): a variable-length Huffman tree
/// over the 4-bit quad value `vwxy` (v = bit3 … y = bit0). `(value, code, len)`,
/// prefix-free-verified (Kraft sum == 1; FFmpeg `mpa_quad_codes[0]`/`mpa_quad_bits[0]`).
/// NOTE: the research spec's table listed value `1010` as len 4 — that is a
/// transcription typo (it collides with `1001`=`0011` and makes the Kraft sum 1.0156,
/// i.e. not prefix-free). The correct length for `1010` (code 6) is **6** (→ `000110`);
/// with that fix the tree is exactly complete. Verified by `mp3_count1_table_a_*` KATs.
static COUNT1_TABLE_A: [(u8, u32, u8); 16] = [
    (0b0000, 0b1, 1),
    (0b0001, 0b0101, 4),
    (0b0010, 0b0100, 4),
    (0b0011, 0b00101, 5),
    (0b0100, 0b0110, 4),
    (0b0101, 0b000101, 6),
    (0b0110, 0b00100, 5),
    (0b0111, 0b000100, 6),
    (0b1000, 0b0111, 4),
    (0b1001, 0b0011, 4),
    (0b1010, 0b000110, 6),
    (0b1011, 0b000000, 6),
    (0b1100, 0b000111, 6),
    (0b1101, 0b000010, 6),
    (0b1110, 0b000011, 6),
    (0b1111, 0b000001, 6),
];

/// Decode one count1 quadruple (v, w, x, y). Two ISO tables:
/// - table B (`count1table_select == 1`, t33): a fixed 4-bit code whose bits are the
///   ones-complement of the four magnitudes, each followed by a sign bit when nonzero.
/// - table A (`count1table_select == 0`, t32): the variable-length Huffman tree above;
///   magnitudes are the four bits of the decoded `value`, each followed by a sign bit.
/// Any other select is non-conformant → clean `Invalid` (never wrong data).
pub fn decode_count1_quad(
    r: &mut BitReader,
    table_select: u32,
) -> Result<(i32, i32, i32, i32), Mp3Error> {
    if table_select == 1 {
        let bits = r.read_bits(4)?;
        let v = apply_sign(r, (((bits >> 3) & 1) ^ 1) as i32)?;
        let w = apply_sign(r, (((bits >> 2) & 1) ^ 1) as i32)?;
        let x = apply_sign(r, (((bits >> 1) & 1) ^ 1) as i32)?;
        let y = apply_sign(r, ((bits & 1) ^ 1) as i32)?;
        return Ok((v, w, x, y));
    }
    if table_select == 0 {
        // Prefix-match the variable-length code MSB-first.
        let mut code: u32 = 0;
        let mut len: u8 = 0;
        loop {
            code = (code << 1) | r.read_bit()?;
            len += 1;
            if len > 6 {
                return Err(Mp3Error::Invalid("count1 table A code too long"));
            }
            for &(value, c, l) in COUNT1_TABLE_A.iter() {
                if l == len && c == code {
                    let v = apply_sign(r, ((value >> 3) & 1) as i32)?;
                    let w = apply_sign(r, ((value >> 2) & 1) as i32)?;
                    let x = apply_sign(r, ((value >> 1) & 1) as i32)?;
                    let y = apply_sign(r, (value & 1) as i32)?;
                    return Ok((v, w, x, y));
                }
            }
        }
    }
    Err(Mp3Error::Invalid("count1 table out of range"))
}

#[inline]
fn apply_sign(r: &mut BitReader, mag: i32) -> Result<i32, Mp3Error> {
    if mag != 0 && r.read_bit()? == 1 {
        Ok(-mag)
    } else {
        Ok(mag)
    }
}

/// Decode the full Huffman-coded spectrum for one granule/channel into `is[0..576]`:
/// the three big-value regions (each with its own table + region boundary) followed by
/// the count1 region (quadruples) until `part2_3_length` bits are consumed or 576 lines
/// are filled. Bounded by `bits_budget` (= part2_3_length minus the scalefactor bits
/// already read), so a corrupt granule can never read past its own data. Any region
/// using an untranscribed table stops cleanly (the rest of `is` stays zero = silence
/// for that region), never wrong coefficients.
///
/// `region_bounds` are the line indices that end region0 and region1 (region2 runs to
/// `2*big_values`). For short blocks the caller passes the short-block region split.
pub fn decode_huffman_region(
    r: &mut BitReader,
    big_values: u32,
    table_select: &[u32; 3],
    region_bounds: (usize, usize),
    count1table_select: u32,
    bits_budget: usize,
    start_bit: usize,
    is: &mut [i32; 576],
) -> Result<(), Mp3Error> {
    let end_bit = start_bit + bits_budget;
    let big = (big_values as usize * 2).min(576);
    let (r1, r2) = region_bounds;
    let r1 = r1.min(big);
    let r2 = r2.min(big);

    let mut idx = 0usize;
    // Big-value regions.
    while idx < big {
        // Select the table for the current region by line index.
        let tbl_idx = if idx < r1 {
            table_select[0]
        } else if idx < r2 {
            table_select[1]
        } else {
            table_select[2]
        };
        let table = match huff_table(tbl_idx) {
            Ok(t) => t,
            // Untranscribed table: stop here (rest stays zero = clean silence).
            Err(_) => return Ok(()),
        };
        if r.bit_position() >= end_bit {
            break;
        }
        let (x, y) = decode_huff_pair(r, &table)?;
        if idx + 1 < 576 {
            is[idx] = x;
            is[idx + 1] = y;
        }
        idx += 2;
    }
    // Count1 region: quadruples until budget exhausted or 576 lines filled.
    while idx + 4 <= 576 && r.bit_position() + 1 <= end_bit {
        match decode_count1_quad(r, count1table_select) {
            Ok((v, w, x, y)) => {
                is[idx] = v;
                is[idx + 1] = w;
                is[idx + 2] = x;
                is[idx + 3] = y;
                idx += 4;
            }
            // Untranscribed count1 table (A): stop cleanly.
            Err(Mp3Error::Invalid(_)) => break,
            Err(e) => return Err(e),
        }
        if r.bit_position() >= end_bit {
            break;
        }
    }
    Ok(())
}

// ─── Bit reservoir / main_data assembly ─────────────────────────────────────

/// Reservoir buffer: accumulates main_data bytes across frames so a frame can read
/// `main_data_begin` bytes that physically lived in earlier frames.
#[derive(Default)]
pub struct Reservoir {
    buf: Vec<u8>,
}

impl Reservoir {
    pub fn new() -> Self {
        Self { buf: Vec::new() }
    }

    pub fn clear(&mut self) {
        self.buf.clear();
    }

    pub fn len(&self) -> usize {
        self.buf.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    /// Append the main_data portion of a frame, bounding total growth (the reservoir
    /// back-pointer is at most 511 bytes, so 8 KiB of history is always enough).
    pub fn push(&mut self, data: &[u8]) {
        self.buf.extend_from_slice(data);
        if self.buf.len() > 8192 {
            let excess = self.buf.len() - 8192;
            self.buf.drain(0..excess);
        }
    }

    /// Resolve main_data for the current frame. `main_data_begin` counts bytes
    /// backwards from the end of the previously-accumulated reservoir to the start of
    /// this frame's main data. Returns the assembled main-data bytes (history prefix
    /// + this frame's own main data), or `NeedMoreData` if the back-pointer reaches
    /// before what we have buffered (the normal case at stream start).
    pub fn assemble(
        &self,
        main_data_begin: usize,
        this_frame_main: &[u8],
    ) -> Result<Vec<u8>, Mp3Error> {
        if main_data_begin > self.buf.len() {
            return Err(Mp3Error::NeedMoreData);
        }
        let mut out = Vec::with_capacity(main_data_begin + this_frame_main.len());
        let start = self.buf.len() - main_data_begin;
        out.extend_from_slice(&self.buf[start..]);
        out.extend_from_slice(this_frame_main);
        Ok(out)
    }
}

#[cfg(test)]
mod selftests {
    // These run only under `cargo test`; the public KATs live in lib.rs.
    use super::*;

    #[test]
    fn bitreader_reads_msb_first() {
        let data = [0b1010_0000u8];
        let mut r = BitReader::new(&data);
        assert_eq!(r.read_bit().unwrap(), 1);
        assert_eq!(r.read_bit().unwrap(), 0);
        assert_eq!(r.read_bit().unwrap(), 1);
        assert_eq!(r.read_bit().unwrap(), 0);
        assert!(r.read_bits(8).is_err()); // only 4 bits left
    }
}
