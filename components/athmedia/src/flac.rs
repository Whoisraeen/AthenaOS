//! From-scratch native FLAC decoder (RFC 9639 / the FLAC format spec) → interleaved
//! f32-normalized PCM. Concept §creators/media: *"play my music"* — FLAC is the
//! lossless format real music libraries ship in; today only WAV produced sound, so
//! this is the second real audio codec.
//!
//! What decodes:
//! - STREAMINFO metadata block (sample rate, channels, bits-per-sample, total
//!   samples, block sizes) + skips every other metadata block.
//! - Frame header: sync code, blocking strategy, block-size + sample-rate + bit-depth
//!   codes (incl. the "read from end of header" 8/16-bit forms), channel assignment,
//!   UTF-8 coded frame/sample number, and the header CRC-8.
//! - All four subframe types: CONSTANT, VERBATIM, FIXED predictor (orders 0–4), and
//!   LPC (quantized coefficients, any order/precision/shift).
//! - Rice-coded residual: both partitioned coding methods (4-bit and 5-bit parameter)
//!   incl. the escape code (verbatim residual of an explicit bit width).
//! - Inter-channel decorrelation: independent, left-side, right-side, mid-side.
//! - Wasted bits per subframe (unary `k`, samples shifted left by `k`).
//!
//! Untrusted-input discipline: every read goes through a bounds-checked bit reader;
//! a truncated/corrupt stream returns a clean `Err` and NEVER panics or indexes out
//! of bounds. (Decoders are the #1 RCE surface — CLAUDE media quality bar.)
//!
//! Host-KAT'd in `lib.rs` tests against constructed known streams (concrete PCM
//! match). No external dependency — pure `#![no_std]` + `alloc`.

use alloc::vec;
use alloc::vec::Vec;

/// Parsed STREAMINFO metadata block (RFC 9639 §8.2).
#[derive(Debug, Clone, Copy)]
pub struct StreamInfo {
    pub min_block_size: u16,
    pub max_block_size: u16,
    pub min_frame_size: u32,
    pub max_frame_size: u32,
    pub sample_rate: u32,
    pub channels: u8,
    pub bits_per_sample: u8,
    pub total_samples: u64,
}

/// One decoded FLAC frame: interleaved PCM samples as raw signed integers (in the
/// stream's native bit depth, sign-extended into i32) plus geometry. The caller
/// normalizes to f32 using `bits_per_sample`.
pub struct DecodedFrame {
    /// Interleaved signed samples, length = `block_size * channels`.
    pub samples: Vec<i32>,
    pub block_size: u32,
    pub channels: u8,
    pub sample_rate: u32,
    pub bits_per_sample: u8,
    /// Total bytes of the input slice consumed by this frame (sync → end CRC-16).
    pub consumed: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlacError {
    /// Ran off the end of the bitstream (truncated/short input).
    UnexpectedEof,
    /// A structural field held a value the spec forbids / we don't support.
    Invalid(&'static str),
    /// Header CRC-8 mismatch (corrupt frame header).
    BadHeaderCrc,
    /// Frame CRC-16 mismatch (corrupt frame body).
    BadFrameCrc,
}

// ─── Bounds-checked MSB-first bit reader ──────────────────────────────────────

/// MSB-first bit reader over a byte slice. Every accessor returns `Err` rather than
/// panicking when the stream is exhausted — this is the untrusted-input boundary.
pub struct BitReader<'a> {
    data: &'a [u8],
    /// Bit cursor (absolute, from the start of `data`).
    bitpos: usize,
}

impl<'a> BitReader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, bitpos: 0 }
    }

    /// Current bit position (for measuring consumed bytes).
    pub fn bit_position(&self) -> usize {
        self.bitpos
    }

    /// Byte position rounded up to the next whole byte.
    pub fn byte_position_ceil(&self) -> usize {
        (self.bitpos + 7) / 8
    }

    #[inline]
    fn total_bits(&self) -> usize {
        self.data.len() * 8
    }

    /// Read a single bit.
    #[inline]
    pub fn read_bit(&mut self) -> Result<u32, FlacError> {
        if self.bitpos >= self.total_bits() {
            return Err(FlacError::UnexpectedEof);
        }
        let byte = self.data[self.bitpos >> 3];
        let shift = 7 - (self.bitpos & 7);
        self.bitpos += 1;
        Ok(((byte >> shift) & 1) as u32)
    }

    /// Read `n` bits (0..=32) MSB-first into a u32.
    pub fn read_bits(&mut self, n: u32) -> Result<u32, FlacError> {
        if n == 0 {
            return Ok(0);
        }
        if n > 32 {
            return Err(FlacError::Invalid("read_bits > 32"));
        }
        if self.bitpos + n as usize > self.total_bits() {
            return Err(FlacError::UnexpectedEof);
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

    /// Read `n` bits (0..=64) into a u64.
    pub fn read_bits_u64(&mut self, n: u32) -> Result<u64, FlacError> {
        if n > 64 {
            return Err(FlacError::Invalid("read_bits_u64 > 64"));
        }
        if n <= 32 {
            return Ok(self.read_bits(n)? as u64);
        }
        let hi = self.read_bits(n - 32)? as u64;
        let lo = self.read_bits(32)? as u64;
        Ok((hi << 32) | lo)
    }

    /// Read `n` bits and sign-extend (two's complement) into an i32.
    pub fn read_signed(&mut self, n: u32) -> Result<i32, FlacError> {
        if n == 0 {
            return Ok(0);
        }
        let raw = self.read_bits(n)?;
        // Sign-extend from bit (n-1).
        let shift = 32 - n;
        Ok(((raw << shift) as i32) >> shift)
    }

    /// Read a unary-coded value: count of zero bits before the terminating one bit.
    pub fn read_unary(&mut self) -> Result<u32, FlacError> {
        let mut count = 0u32;
        loop {
            if self.read_bit()? == 1 {
                return Ok(count);
            }
            count += 1;
            // Defensive: a corrupt all-zero tail must not spin forever; the EOF check
            // in read_bit terminates it, but cap to the remaining bits anyway.
            if count as usize > self.total_bits() {
                return Err(FlacError::UnexpectedEof);
            }
        }
    }

    /// Align the cursor to the next byte boundary (skip the pad bits).
    pub fn align_to_byte(&mut self) {
        let rem = self.bitpos & 7;
        if rem != 0 {
            self.bitpos += 8 - rem;
        }
    }
}

// ─── CRC ──────────────────────────────────────────────────────────────────────

/// FLAC frame-header CRC-8 (polynomial x^8 + x^2 + x^1 + x^0 = 0x07), over the
/// header bytes excluding the CRC-8 byte itself.
fn crc8(data: &[u8]) -> u8 {
    let mut crc: u8 = 0;
    for &b in data {
        crc ^= b;
        for _ in 0..8 {
            if crc & 0x80 != 0 {
                crc = (crc << 1) ^ 0x07;
            } else {
                crc <<= 1;
            }
        }
    }
    crc
}

/// FLAC frame CRC-16 (polynomial x^16 + x^15 + x^2 + x^0 = 0x8005), over the whole
/// frame excluding the trailing CRC-16 bytes.
fn crc16(data: &[u8]) -> u16 {
    let mut crc: u16 = 0;
    for &b in data {
        crc ^= (b as u16) << 8;
        for _ in 0..8 {
            if crc & 0x8000 != 0 {
                crc = (crc << 1) ^ 0x8005;
            } else {
                crc <<= 1;
            }
        }
    }
    crc
}

// ─── STREAMINFO + metadata ─────────────────────────────────────────────────────

/// Parse the `fLaC` marker + the metadata-block chain, returning the parsed
/// STREAMINFO and the byte offset where the audio frames begin (just past the last
/// metadata block). Skips all non-STREAMINFO metadata blocks.
pub fn parse_metadata(data: &[u8]) -> Result<(StreamInfo, usize), FlacError> {
    if data.len() < 4 || &data[0..4] != b"fLaC" {
        return Err(FlacError::Invalid("missing fLaC marker"));
    }
    let mut off = 4usize;
    let mut stream_info: Option<StreamInfo> = None;

    loop {
        if off + 4 > data.len() {
            return Err(FlacError::UnexpectedEof);
        }
        let header = data[off];
        let is_last = (header & 0x80) != 0;
        let block_type = header & 0x7F;
        let len = ((data[off + 1] as usize) << 16)
            | ((data[off + 2] as usize) << 8)
            | (data[off + 3] as usize);
        let body = off + 4;
        if body + len > data.len() {
            return Err(FlacError::UnexpectedEof);
        }

        if block_type == 0 {
            // STREAMINFO (must be 34 bytes).
            if len < 34 {
                return Err(FlacError::Invalid("STREAMINFO too short"));
            }
            stream_info = Some(parse_streaminfo(&data[body..body + 34])?);
        }
        // All other block types (PADDING/APPLICATION/SEEKTABLE/VORBIS_COMMENT/
        // CUESHEET/PICTURE/reserved) are skipped — we only need the geometry.

        off = body + len;
        if is_last {
            break;
        }
    }

    match stream_info {
        Some(si) => Ok((si, off)),
        None => Err(FlacError::Invalid("no STREAMINFO block")),
    }
}

/// Parse a 34-byte STREAMINFO body.
fn parse_streaminfo(b: &[u8]) -> Result<StreamInfo, FlacError> {
    if b.len() < 34 {
        return Err(FlacError::UnexpectedEof);
    }
    let min_block_size = ((b[0] as u16) << 8) | b[1] as u16;
    let max_block_size = ((b[2] as u16) << 8) | b[3] as u16;
    let min_frame_size = ((b[4] as u32) << 16) | ((b[5] as u32) << 8) | b[6] as u32;
    let max_frame_size = ((b[7] as u32) << 16) | ((b[8] as u32) << 8) | b[9] as u32;
    // 20 bits sample rate, 3 bits (channels-1), 5 bits (bps-1), 36 bits total samples.
    let sample_rate = ((b[10] as u32) << 12) | ((b[11] as u32) << 4) | ((b[12] as u32) >> 4);
    let channels = ((b[12] >> 1) & 0x07) + 1;
    let bits_per_sample = (((b[12] & 0x01) << 4) | (b[13] >> 4)) + 1;
    let total_samples = (((b[13] & 0x0F) as u64) << 32)
        | ((b[14] as u64) << 24)
        | ((b[15] as u64) << 16)
        | ((b[16] as u64) << 8)
        | (b[17] as u64);

    if channels == 0 || channels > 8 {
        return Err(FlacError::Invalid("STREAMINFO channels out of range"));
    }
    if bits_per_sample < 4 || bits_per_sample > 32 {
        return Err(FlacError::Invalid("STREAMINFO bit depth out of range"));
    }

    Ok(StreamInfo {
        min_block_size,
        max_block_size,
        min_frame_size,
        max_frame_size,
        sample_rate,
        channels,
        bits_per_sample,
        total_samples,
    })
}

// ─── Channel assignment ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChannelAssignment {
    /// Each channel coded independently (assignment 0..=7 → channels-1).
    Independent(u8),
    /// Left + side (channel 0 = left, channel 1 = side = left - right).
    LeftSide,
    /// Right + side (channel 0 = side = left - right, channel 1 = right).
    RightSide,
    /// Mid + side (channel 0 = mid = (l+r)>>1, channel 1 = side = l - r).
    MidSide,
}

// ─── Frame decode ───────────────────────────────────────────────────────────────

/// Decode a single FLAC frame starting at `data[0]` (which must be the frame sync).
/// `si` is the stream's STREAMINFO (used as a fallback for the "from STREAMINFO"
/// block-size / sample-rate / bit-depth codes). Returns the decoded frame and how
/// many input bytes it consumed.
pub fn decode_frame(data: &[u8], si: &StreamInfo) -> Result<DecodedFrame, FlacError> {
    let mut br = BitReader::new(data);

    // ── Frame header ──
    // Sync code: 14 bits = 0b11111111111110.
    let sync = br.read_bits(14)?;
    if sync != 0b11_1111_1111_1110 {
        return Err(FlacError::Invalid("bad frame sync"));
    }
    let _reserved = br.read_bit()?; // mandatory 0
    let blocking_strategy = br.read_bit()?; // 0 fixed-blocksize, 1 variable

    let block_size_code = br.read_bits(4)?;
    let sample_rate_code = br.read_bits(4)?;
    let channel_assignment_code = br.read_bits(4)?;
    let sample_size_code = br.read_bits(3)?;
    let _reserved2 = br.read_bit()?; // mandatory 0

    // Coded number (frame number for fixed, sample number for variable): UTF-8-ish.
    let _coded_number = read_utf8_number(&mut br)?;

    // Block size: may be encoded after the header.
    let block_size: u32 = match block_size_code {
        0 => return Err(FlacError::Invalid("reserved block size 0")),
        1 => 192,
        2..=5 => 576 << (block_size_code - 2),
        6 => (br.read_bits(8)? + 1) as u32,  // 8-bit, value-1
        7 => (br.read_bits(16)? + 1) as u32, // 16-bit, value-1
        8..=15 => 256 << (block_size_code - 8),
        _ => unreachable!(),
    };

    // Sample rate: may be encoded after the header; we don't need the encoded value
    // for decoding samples, but must consume the right number of bits.
    let sample_rate: u32 = match sample_rate_code {
        0 => si.sample_rate,
        1 => 88200,
        2 => 176400,
        3 => 192000,
        4 => 8000,
        5 => 16000,
        6 => 22050,
        7 => 24000,
        8 => 32000,
        9 => 44100,
        10 => 48000,
        11 => 96000,
        12 => br.read_bits(8)? * 1000,
        13 => br.read_bits(16)?,
        14 => br.read_bits(16)? * 10,
        15 => return Err(FlacError::Invalid("invalid sample-rate code 15")),
        _ => unreachable!(),
    };

    let channel_assignment = match channel_assignment_code {
        0..=7 => ChannelAssignment::Independent(channel_assignment_code as u8),
        8 => ChannelAssignment::LeftSide,
        9 => ChannelAssignment::RightSide,
        10 => ChannelAssignment::MidSide,
        _ => return Err(FlacError::Invalid("reserved channel assignment")),
    };

    let channels: u8 = match channel_assignment {
        ChannelAssignment::Independent(n) => n + 1,
        _ => 2,
    };

    let bits_per_sample: u8 = match sample_size_code {
        0 => si.bits_per_sample,
        1 => 8,
        2 => 12,
        3 => return Err(FlacError::Invalid("reserved sample size code 3")),
        4 => 16,
        5 => 20,
        6 => 24,
        7 => 32,
        _ => unreachable!(),
    };

    // Verify the header CRC-8 over all header bytes up to (not including) the CRC byte.
    let header_bytes_len = br.byte_position_ceil();
    // The CRC-8 byte itself follows the header. We've consumed exactly the header
    // (bit cursor is byte-aligned at this point per the spec).
    if br.bit_position() & 7 != 0 {
        return Err(FlacError::Invalid("frame header not byte-aligned"));
    }
    if header_bytes_len >= data.len() {
        return Err(FlacError::UnexpectedEof);
    }
    let expected_crc8 = crc8(&data[..header_bytes_len]);
    let crc8_byte = br.read_bits(8)? as u8;
    if crc8_byte != expected_crc8 {
        return Err(FlacError::BadHeaderCrc);
    }

    let _ = blocking_strategy; // captured for completeness; not needed for PCM output

    // ── Subframes ──
    let bs = block_size as usize;
    if bs == 0 || bs > 1 << 20 {
        return Err(FlacError::Invalid("block size out of range"));
    }
    let mut channel_data: Vec<Vec<i32>> = Vec::with_capacity(channels as usize);
    for ch in 0..channels as usize {
        // For side channels the effective bit depth is bps + 1.
        let ch_bps = match channel_assignment {
            ChannelAssignment::LeftSide | ChannelAssignment::MidSide => {
                if ch == 1 {
                    bits_per_sample + 1
                } else {
                    bits_per_sample
                }
            }
            ChannelAssignment::RightSide => {
                if ch == 0 {
                    bits_per_sample + 1
                } else {
                    bits_per_sample
                }
            }
            ChannelAssignment::Independent(_) => bits_per_sample,
        };
        let sub = decode_subframe(&mut br, bs, ch_bps)?;
        channel_data.push(sub);
    }

    // ── Frame footer: align + CRC-16 ──
    br.align_to_byte();
    let body_len = br.byte_position_ceil();
    if body_len + 2 > data.len() {
        return Err(FlacError::UnexpectedEof);
    }
    let expected_crc16 = crc16(&data[..body_len]);
    let crc16_lo_hi = [data[body_len], data[body_len + 1]];
    let actual_crc16 = ((crc16_lo_hi[0] as u16) << 8) | crc16_lo_hi[1] as u16;
    if actual_crc16 != expected_crc16 {
        return Err(FlacError::BadFrameCrc);
    }
    let consumed = body_len + 2;

    // ── Inter-channel decorrelation → interleaved PCM ──
    let interleaved = decorrelate_interleave(&channel_data, channel_assignment, bs)?;

    Ok(DecodedFrame {
        samples: interleaved,
        block_size,
        channels,
        sample_rate,
        bits_per_sample,
        consumed,
    })
}

/// Reconstruct independent channels from the coded assignment, then interleave.
fn decorrelate_interleave(
    channel_data: &[Vec<i32>],
    assignment: ChannelAssignment,
    block_size: usize,
) -> Result<Vec<i32>, FlacError> {
    let channels = channel_data.len();
    if channels == 0 {
        return Err(FlacError::Invalid("zero channels"));
    }
    for c in channel_data {
        if c.len() != block_size {
            return Err(FlacError::Invalid("subframe length mismatch"));
        }
    }

    let mut out = vec![0i32; block_size * channels];

    match assignment {
        ChannelAssignment::Independent(_) => {
            for i in 0..block_size {
                for c in 0..channels {
                    out[i * channels + c] = channel_data[c][i];
                }
            }
        }
        ChannelAssignment::LeftSide => {
            // ch0 = left, ch1 = side = left - right  →  right = left - side.
            for i in 0..block_size {
                let left = channel_data[0][i];
                let side = channel_data[1][i];
                let right = left.wrapping_sub(side);
                out[i * 2] = left;
                out[i * 2 + 1] = right;
            }
        }
        ChannelAssignment::RightSide => {
            // ch0 = side = left - right, ch1 = right  →  left = right + side.
            for i in 0..block_size {
                let side = channel_data[0][i];
                let right = channel_data[1][i];
                let left = right.wrapping_add(side);
                out[i * 2] = left;
                out[i * 2 + 1] = right;
            }
        }
        ChannelAssignment::MidSide => {
            // ch0 = mid, ch1 = side. left = mid + (side+ (side&1? ... ))
            // Per spec: mid = (left + right) >> 1, side = left - right.
            //   left  = ((mid<<1) | (side & 1) + side) / 2  ... reconstruct:
            //   m2 = (mid << 1) | (side & 1)
            //   left  = (m2 + side) >> 1
            //   right = (m2 - side) >> 1
            for i in 0..block_size {
                let mid = channel_data[0][i];
                let side = channel_data[1][i];
                let m2 = (mid << 1) | (side & 1);
                let left = (m2.wrapping_add(side)) >> 1;
                let right = (m2.wrapping_sub(side)) >> 1;
                out[i * 2] = left;
                out[i * 2 + 1] = right;
            }
        }
    }
    Ok(out)
}

// ─── Subframe decode ────────────────────────────────────────────────────────────

/// Decode one subframe of `block_size` samples at effective bit depth `bps`.
fn decode_subframe(br: &mut BitReader, block_size: usize, bps: u8) -> Result<Vec<i32>, FlacError> {
    // Subframe header: 1 pad bit (0) + 6 type bits + wasted-bits flag/unary.
    let pad = br.read_bit()?;
    if pad != 0 {
        return Err(FlacError::Invalid("subframe header pad bit nonzero"));
    }
    let type_bits = br.read_bits(6)?;
    let wasted_flag = br.read_bit()?;
    let wasted = if wasted_flag == 1 {
        // Unary: number of leading zeros + 1 = wasted bits count.
        br.read_unary()? + 1
    } else {
        0
    };

    let effective_bps = (bps as u32)
        .checked_sub(wasted)
        .ok_or(FlacError::Invalid("wasted bits exceed sample size"))?;
    if effective_bps == 0 || effective_bps > 33 {
        return Err(FlacError::Invalid("effective bit depth out of range"));
    }

    // Decode according to subframe type.
    let mut samples: Vec<i32> = if type_bits == 0 {
        // CONSTANT
        let v = br.read_signed(effective_bps)?;
        vec![v; block_size]
    } else if type_bits == 1 {
        // VERBATIM
        let mut s = Vec::with_capacity(block_size);
        for _ in 0..block_size {
            s.push(br.read_signed(effective_bps)?);
        }
        s
    } else if (type_bits & 0b111000) == 0b001000 {
        // FIXED predictor (type bits 001ooo), order = low 3 bits (0..=4).
        let order = (type_bits & 0x07) as usize;
        if order > 4 {
            return Err(FlacError::Invalid("FIXED order > 4"));
        }
        decode_fixed(br, block_size, effective_bps, order)?
    } else if (type_bits & 0b100000) == 0b100000 {
        // LPC, order = (low 5 bits) + 1 (1..=32).
        let order = ((type_bits & 0x1F) + 1) as usize;
        decode_lpc(br, block_size, effective_bps, order)?
    } else {
        return Err(FlacError::Invalid("reserved subframe type"));
    };

    // Apply wasted bits (shift left).
    if wasted > 0 {
        for s in samples.iter_mut() {
            *s = s.wrapping_shl(wasted);
        }
    }
    Ok(samples)
}

/// FIXED predictor subframe (orders 0–4). Warm-up samples are stored verbatim, the
/// remaining residual is Rice-coded; the predictor reconstructs each sample.
fn decode_fixed(
    br: &mut BitReader,
    block_size: usize,
    bps: u32,
    order: usize,
) -> Result<Vec<i32>, FlacError> {
    if order > block_size {
        return Err(FlacError::Invalid("FIXED order > block size"));
    }
    let mut samples: Vec<i32> = Vec::with_capacity(block_size);
    for _ in 0..order {
        samples.push(br.read_signed(bps)?);
    }
    let residual = decode_residual(br, block_size, order)?;
    // Apply the fixed predictor (i64 accumulation to avoid overflow).
    for (i, &r) in residual.iter().enumerate() {
        let idx = order + i;
        let pred: i64 = match order {
            0 => 0,
            1 => samples[idx - 1] as i64,
            2 => 2 * samples[idx - 1] as i64 - samples[idx - 2] as i64,
            3 => {
                3 * samples[idx - 1] as i64 - 3 * samples[idx - 2] as i64 + samples[idx - 3] as i64
            }
            4 => {
                4 * samples[idx - 1] as i64 - 6 * samples[idx - 2] as i64
                    + 4 * samples[idx - 3] as i64
                    - samples[idx - 4] as i64
            }
            _ => unreachable!(),
        };
        samples.push((pred + r as i64) as i32);
    }
    Ok(samples)
}

/// LPC subframe: order warm-up samples, then a 4-bit precision, 5-bit shift, `order`
/// signed quantized coefficients, then the Rice residual; the linear predictor
/// reconstructs each sample.
fn decode_lpc(
    br: &mut BitReader,
    block_size: usize,
    bps: u32,
    order: usize,
) -> Result<Vec<i32>, FlacError> {
    if order == 0 || order > 32 || order > block_size {
        return Err(FlacError::Invalid("LPC order out of range"));
    }
    let mut samples: Vec<i32> = Vec::with_capacity(block_size);
    for _ in 0..order {
        samples.push(br.read_signed(bps)?);
    }
    // Precision: 4 bits, value+1 (a value of 0b1111 = 15+1=16 is the max; the all-ones
    // escape is invalid per spec).
    let precision_code = br.read_bits(4)?;
    if precision_code == 0b1111 {
        return Err(FlacError::Invalid("invalid LPC precision (escape)"));
    }
    let precision = precision_code + 1;
    // Quantization shift: 5-bit signed.
    let shift = br.read_signed(5)?;
    if shift < 0 {
        return Err(FlacError::Invalid("negative LPC shift unsupported"));
    }
    let shift = shift as u32;

    let mut coeffs: Vec<i32> = Vec::with_capacity(order);
    for _ in 0..order {
        coeffs.push(br.read_signed(precision)?);
    }

    let residual = decode_residual(br, block_size, order)?;
    for (i, &r) in residual.iter().enumerate() {
        let idx = order + i;
        let mut acc: i64 = 0;
        for (j, &c) in coeffs.iter().enumerate() {
            acc += c as i64 * samples[idx - 1 - j] as i64;
        }
        let pred = acc >> shift;
        samples.push((pred + r as i64) as i32);
    }
    Ok(samples)
}

/// Decode the Rice-coded residual for a subframe. `predictor_order` warm-up samples
/// have already been read; the residual covers `block_size - predictor_order` values
/// split into `2^partition_order` partitions.
fn decode_residual(
    br: &mut BitReader,
    block_size: usize,
    predictor_order: usize,
) -> Result<Vec<i32>, FlacError> {
    // Residual coding method: 2 bits. 0 = 4-bit Rice param, 1 = 5-bit Rice param.
    let method = br.read_bits(2)?;
    let param_bits: u32 = match method {
        0 => 4,
        1 => 5,
        _ => return Err(FlacError::Invalid("reserved residual coding method")),
    };
    // Escape parameter (all ones) signals a verbatim partition.
    let escape_param: u32 = (1 << param_bits) - 1;

    let partition_order = br.read_bits(4)? as usize;
    let num_partitions = 1usize << partition_order;
    if block_size % num_partitions != 0 {
        return Err(FlacError::Invalid("block size not divisible by partitions"));
    }
    let partition_samples = block_size / num_partitions;
    if partition_samples <= predictor_order && partition_order == 0 {
        // First (and only) partition still subtracts the predictor order; valid only
        // if it leaves a non-negative count.
    }

    let total_residual = block_size - predictor_order;
    let mut residual: Vec<i32> = Vec::with_capacity(total_residual);

    for p in 0..num_partitions {
        // The first partition holds (partition_samples - predictor_order) residuals;
        // every other partition holds partition_samples.
        let count = if p == 0 {
            if partition_samples < predictor_order {
                return Err(FlacError::Invalid("partition smaller than predictor order"));
            }
            partition_samples - predictor_order
        } else {
            partition_samples
        };

        let rice_param = br.read_bits(param_bits)?;
        if rice_param == escape_param {
            // Escape: a raw bit width follows, then `count` verbatim signed samples.
            let raw_bits = br.read_bits(5)?;
            for _ in 0..count {
                residual.push(br.read_signed(raw_bits)?);
            }
        } else {
            for _ in 0..count {
                residual.push(read_rice(br, rice_param)?);
            }
        }
    }
    Ok(residual)
}

/// Read one Rice-coded value with parameter `k`: unary quotient (zeros then a one)
/// + `k`-bit remainder, then zig-zag de-interleave to a signed value.
#[inline]
fn read_rice(br: &mut BitReader, k: u32) -> Result<i32, FlacError> {
    let quotient = br.read_unary()?;
    let remainder = if k > 0 { br.read_bits(k)? } else { 0 };
    let u = (quotient << k) | remainder;
    // Zig-zag: even -> u/2, odd -> -(u+1)/2.
    Ok(((u >> 1) as i32) ^ -((u & 1) as i32))
}

/// Read the UTF-8-style coded frame/sample number from a FLAC frame header. Returns
/// the decoded value; only the byte consumption matters for PCM output, but we parse
/// it correctly to stay byte-aligned. Supports the 1–7 byte forms (FLAC extends
/// UTF-8 to 36 bits for sample numbers).
fn read_utf8_number(br: &mut BitReader) -> Result<u64, FlacError> {
    let first = br.read_bits(8)? as u8;
    // Count leading ones to determine length.
    let extra = if first & 0x80 == 0 {
        0
    } else if first & 0xE0 == 0xC0 {
        1
    } else if first & 0xF0 == 0xE0 {
        2
    } else if first & 0xF8 == 0xF0 {
        3
    } else if first & 0xFC == 0xF8 {
        4
    } else if first & 0xFE == 0xFC {
        5
    } else if first == 0xFE {
        6
    } else {
        return Err(FlacError::Invalid("invalid UTF-8 frame number lead byte"));
    };

    let mut value: u64 = if extra == 0 {
        first as u64
    } else {
        // Mask off the length-indicating high bits.
        let mask = 0x7Fu8 >> extra;
        (first & mask) as u64
    };
    for _ in 0..extra {
        let cont = br.read_bits(8)? as u8;
        if cont & 0xC0 != 0x80 {
            return Err(FlacError::Invalid("invalid UTF-8 continuation byte"));
        }
        value = (value << 6) | (cont & 0x3F) as u64;
    }
    Ok(value)
}
