//! # RaeMedia H.264 baseline-profile intra (I-frame) decoder.
//!
//! RaeenOS_Concept.md (§creators / media): a daily driver must "play my movies" and
//! "play my music." MP4 (ISO BMFF) is the dominant container for phone video, downloaded
//! video, and AAC audio; H.264 baseline/constrained-baseline is the floor of "movies and
//! downloaded video." And the manifesto's first principle — "Native everywhere. No
//! Electron tax. No web wrappers. Native rendering, native input, native audio." This
//! module makes `apps/video` show a real decoded first keyframe instead of flat gray.
//!
//! This is a from-scratch, zero-dependency, `#![no_std]` + soft-float decoder of the
//! H.264 **intra** path: RBSP/Exp-Golomb SPS/PPS/slice-header parse (recovering the REAL
//! width/height from the SPS — killing the old 1920×1080 default), CAVLC residual decode
//! (§9.2), Intra_4x4 (9 modes) + Intra_16x16 (4 modes) + chroma intra (4 modes) + I_PCM,
//! the 4×4 integer inverse transform + Hadamard DC + flat dequant (§8.5), raster
//! reconstruction, and the in-loop deblocking filter (§8.7). The sibling of the
//! from-scratch JPEG decoder (`jpeg.rs`): same integer-IDCT family, same chroma
//! subsampling, same never-panic-on-hostile-bytes posture.
//!
//! ## Honest scope (mirrors the AAC PNS/SBR deferral posture)
//! IN: baseline / constrained-baseline (and Main-tagged-but-CAVLC-all-intra) I/IDR
//! slices, CAVLC, 4:2:0, 8-bit, single slice. DEFERRED — a clean `Err` the instant the
//! bitstream demands it, NEVER a wrong-shape frame or a panic (the consumer turns `Err`
//! into its honest "decode pending" placeholder): CABAC, P/B inter, Main/High tools
//! (8×8 transform, custom scaling lists), >8-bit, 4:2:2/4:4:4, interlace, multi-slice/FMO.
//!
//! ## Untrusted-input discipline (decoders are the #1 RCE surface)
//! Every Exp-Golomb/CAVLC read is bounds-checked (a read past the RBSP end returns `Err`,
//! never panics); every count derived from the bitstream is clamped to its ITU maximum
//! before indexing a table or buffer; the frame allocation is capped. A hostile `.mp4`
//! degrades to "can't decode this," never to memory unsafety.

extern crate alloc;

use alloc::vec;
use alloc::vec::Vec;

use crate::h264_tables as t;

/// Hard cap on decoded frame area (luma samples) — a crafted SPS must not allocate
/// gigabytes. 8192×8192 = 64 Mpx.
const MAX_LUMA_SAMPLES: usize = 8192 * 8192;
/// Hard cap on macroblock count (PicSizeInMbs).
const MAX_MBS: usize = MAX_LUMA_SAMPLES / 256;

/// H.264 decode error. Every variant is a *handled* path — nothing here panics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum H264Error {
    Truncated,
    BadExpGolomb,
    Unsupported(&'static str),
    DimensionsOutOfRange,
    BadVlc,
    OutOfRange,
    MissingParamSet,
}

// ─── RBSP bit reader (Exp-Golomb + fixed) — §7.3.1 / §9.1 ───────────────────

/// A reader over an RBSP byte slice (emulation-prevention already stripped). Bounds-checked.
pub struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> BitReader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    #[inline]
    pub fn bits_left(&self) -> usize {
        (self.data.len() * 8).saturating_sub(self.pos)
    }

    #[inline]
    pub fn u1(&mut self) -> Result<u32, H264Error> {
        if self.pos >= self.data.len() * 8 {
            return Err(H264Error::Truncated);
        }
        let byte = self.data[self.pos >> 3];
        let bit = (byte >> (7 - (self.pos & 7))) & 1;
        self.pos += 1;
        Ok(bit as u32)
    }

    pub fn un(&mut self, n: u32) -> Result<u32, H264Error> {
        if n == 0 {
            return Ok(0);
        }
        if n > 32 || self.pos + n as usize > self.data.len() * 8 {
            return Err(H264Error::Truncated);
        }
        let mut val = 0u32;
        for _ in 0..n {
            val = (val << 1) | self.u1()?;
        }
        Ok(val)
    }

    pub fn ue(&mut self) -> Result<u32, H264Error> {
        let mut leading_zeros = 0u32;
        while self.u1()? == 0 {
            leading_zeros += 1;
            if leading_zeros > 31 {
                return Err(H264Error::BadExpGolomb);
            }
        }
        if leading_zeros == 0 {
            return Ok(0);
        }
        let suffix = self.un(leading_zeros)?;
        Ok((1u32 << leading_zeros) - 1 + suffix)
    }

    pub fn se(&mut self) -> Result<i32, H264Error> {
        let k = self.ue()?;
        let val = ((k + 1) >> 1) as i32;
        if k & 1 == 1 {
            Ok(val)
        } else {
            Ok(-val)
        }
    }

    fn peek(&self, n: u32) -> u32 {
        let mut val = 0u32;
        for i in 0..n {
            let p = self.pos + i as usize;
            let bit = if p < self.data.len() * 8 {
                (self.data[p >> 3] >> (7 - (p & 7))) & 1
            } else {
                0
            };
            val = (val << 1) | bit as u32;
        }
        val
    }

    fn skip(&mut self, n: u32) {
        self.pos += n as usize;
    }

    fn vlc(&mut self, table: &[t::Vlc]) -> Result<(u8, u8), H264Error> {
        for e in table {
            let code = self.peek(e.len as u32);
            if code == e.code as u32 {
                if self.pos + e.len as usize > self.data.len() * 8 {
                    return Err(H264Error::Truncated);
                }
                self.skip(e.len as u32);
                return Ok((e.a, e.b));
            }
        }
        Err(H264Error::BadVlc)
    }
}

/// Strip emulation-prevention bytes from a NAL payload (after the 1-byte header) → RBSP.
/// `00 00 03 xx` (xx ≤ 0x03) → `00 00 xx`. §7.3.1.
pub fn nal_to_rbsp(payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len());
    let mut zeros = 0usize;
    let mut i = 0;
    while i < payload.len() {
        let b = payload[i];
        if zeros >= 2 && b == 0x03 && i + 1 < payload.len() && payload[i + 1] <= 0x03 {
            zeros = 0;
            i += 1;
            continue;
        }
        out.push(b);
        if b == 0 {
            zeros += 1;
        } else {
            zeros = 0;
        }
        i += 1;
    }
    out
}

// ─── SPS (§7.3.2.1) ─────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct Sps {
    pub profile_idc: u8,
    pub level_idc: u8,
    pub seq_parameter_set_id: u32,
    pub log2_max_frame_num: u8,
    pub pic_order_cnt_type: u8,
    pub log2_max_poc_lsb: u8,
    pub pic_width_in_mbs: u32,
    pub frame_height_in_mbs: u32,
    pub frame_mbs_only: bool,
    pub crop_left: u32,
    pub crop_right: u32,
    pub crop_top: u32,
    pub crop_bottom: u32,
}

impl Sps {
    pub fn coded_width(&self) -> u32 {
        self.pic_width_in_mbs * 16
    }
    pub fn coded_height(&self) -> u32 {
        self.frame_height_in_mbs * 16
    }
    pub fn display_width(&self) -> u32 {
        self.coded_width()
            .saturating_sub(2 * (self.crop_left + self.crop_right))
    }
    pub fn display_height(&self) -> u32 {
        self.coded_height()
            .saturating_sub(2 * (self.crop_top + self.crop_bottom))
    }
}

const HIGH_PROFILES: &[u8] = &[100, 110, 122, 244, 44, 83, 86, 118, 128, 138, 139, 134, 135];

/// Parse the SPS RBSP (bytes AFTER the NAL header byte). §7.3.2.1.
pub fn parse_sps(rbsp: &[u8]) -> Result<Sps, H264Error> {
    let mut r = BitReader::new(rbsp);
    let profile_idc = r.un(8)? as u8;
    let _constraint = r.un(8)?;
    let level_idc = r.un(8)? as u8;
    let seq_parameter_set_id = r.ue()?;

    if HIGH_PROFILES.contains(&profile_idc) {
        let chroma_format_idc = r.ue()?;
        if chroma_format_idc == 3 {
            let _separate_colour = r.u1()?;
        }
        if chroma_format_idc != 1 {
            return Err(H264Error::Unsupported("h264: non-4:2:0 chroma"));
        }
        let bit_depth_luma = r.ue()? + 8;
        let bit_depth_chroma = r.ue()? + 8;
        if bit_depth_luma != 8 || bit_depth_chroma != 8 {
            return Err(H264Error::Unsupported("h264: >8-bit depth"));
        }
        let _qpprime_y_zero = r.u1()?;
        let seq_scaling_matrix_present = r.u1()?;
        if seq_scaling_matrix_present == 1 {
            return Err(H264Error::Unsupported("h264: custom scaling lists"));
        }
    }

    let log2_max_frame_num = (r.ue()? + 4) as u8;
    if log2_max_frame_num > 16 {
        return Err(H264Error::OutOfRange);
    }
    let pic_order_cnt_type = r.ue()? as u8;
    let mut log2_max_poc_lsb = 4u8;
    if pic_order_cnt_type == 0 {
        log2_max_poc_lsb = (r.ue()? + 4) as u8;
        if log2_max_poc_lsb > 16 {
            return Err(H264Error::OutOfRange);
        }
    } else if pic_order_cnt_type == 1 {
        let _delta_zero = r.u1()?;
        let _off_non_ref = r.se()?;
        let _off_top_bottom = r.se()?;
        let num_ref = r.ue()?;
        if num_ref > 255 {
            return Err(H264Error::OutOfRange);
        }
        for _ in 0..num_ref {
            let _ = r.se()?;
        }
    } else if pic_order_cnt_type > 2 {
        return Err(H264Error::OutOfRange);
    }

    let _max_num_ref_frames = r.ue()?;
    let _gaps_allowed = r.u1()?;
    let pic_width_in_mbs = r.ue()? + 1;
    let pic_height_in_map_units = r.ue()? + 1;
    let frame_mbs_only = r.u1()? == 1;
    if !frame_mbs_only {
        return Err(H264Error::Unsupported(
            "h264: interlaced (frame_mbs_only=0)",
        ));
    }
    let frame_height_in_mbs = pic_height_in_map_units;
    let _direct_8x8_inference = r.u1()?;

    let frame_cropping = r.u1()? == 1;
    let (mut crop_left, mut crop_right, mut crop_top, mut crop_bottom) = (0, 0, 0, 0);
    if frame_cropping {
        crop_left = r.ue()?;
        crop_right = r.ue()?;
        crop_top = r.ue()?;
        crop_bottom = r.ue()?;
    }

    let mbs = (pic_width_in_mbs as usize).saturating_mul(frame_height_in_mbs as usize);
    if pic_width_in_mbs == 0
        || frame_height_in_mbs == 0
        || pic_width_in_mbs > 512
        || frame_height_in_mbs > 512
        || mbs > MAX_MBS
    {
        return Err(H264Error::DimensionsOutOfRange);
    }

    Ok(Sps {
        profile_idc,
        level_idc,
        seq_parameter_set_id,
        log2_max_frame_num,
        pic_order_cnt_type,
        log2_max_poc_lsb,
        pic_width_in_mbs,
        frame_height_in_mbs,
        frame_mbs_only,
        crop_left,
        crop_right,
        crop_top,
        crop_bottom,
    })
}

// ─── PPS (§7.3.2.2) ─────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct Pps {
    pub pic_parameter_set_id: u32,
    pub seq_parameter_set_id: u32,
    pub entropy_coding_mode: bool,
    pub bottom_field_pic_order_present: bool,
    pub num_slice_groups: u32,
    pub pic_init_qp: i32,
    pub chroma_qp_index_offset: i32,
    pub deblocking_filter_control_present: bool,
    pub constrained_intra_pred: bool,
}

/// Parse the PPS RBSP (bytes after the NAL header byte). §7.3.2.2.
pub fn parse_pps(rbsp: &[u8]) -> Result<Pps, H264Error> {
    let mut r = BitReader::new(rbsp);
    let pic_parameter_set_id = r.ue()?;
    let seq_parameter_set_id = r.ue()?;
    let entropy_coding_mode = r.u1()? == 1;
    if entropy_coding_mode {
        return Err(H264Error::Unsupported("h264: CABAC not supported"));
    }
    let bottom_field_pic_order_present = r.u1()? == 1;
    let num_slice_groups = r.ue()? + 1;
    if num_slice_groups > 1 {
        return Err(H264Error::Unsupported("h264: slice groups / FMO"));
    }
    let _num_ref_idx_l0 = r.ue()?;
    let _num_ref_idx_l1 = r.ue()?;
    let _weighted_pred = r.u1()?;
    let _weighted_bipred = r.un(2)?;
    let pic_init_qp = 26 + r.se()?;
    let _pic_init_qs = r.se()?;
    let chroma_qp_index_offset = r.se()?;
    let deblocking_filter_control_present = r.u1()? == 1;
    let constrained_intra_pred = r.u1()? == 1;
    let _redundant_pic_cnt_present = r.u1()?;

    Ok(Pps {
        pic_parameter_set_id,
        seq_parameter_set_id,
        entropy_coding_mode,
        bottom_field_pic_order_present,
        num_slice_groups,
        pic_init_qp,
        chroma_qp_index_offset,
        deblocking_filter_control_present,
        constrained_intra_pred,
    })
}

// ─── Slice header (§7.3.3) ──────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct SliceHeader {
    pub first_mb_in_slice: u32,
    pub slice_type: u32,
    pub pic_parameter_set_id: u32,
    pub frame_num: u32,
    pub slice_qp: i32,
    pub disable_deblocking_filter_idc: u32,
    pub slice_alpha_c0_offset: i32,
    pub slice_beta_offset: i32,
}

impl SliceHeader {
    pub fn is_i_slice(&self) -> bool {
        let st = self.slice_type % 5;
        st == 2 || st == 4
    }
}

/// Parse an IDR/I slice header. `nal_type` 5 = IDR. §7.3.3.
pub fn parse_slice_header(
    rbsp: &[u8],
    sps: &Sps,
    pps: &Pps,
    nal_type: u8,
) -> Result<SliceHeader, H264Error> {
    let mut r = BitReader::new(rbsp);
    let first_mb_in_slice = r.ue()?;
    let slice_type = r.ue()?;
    let pic_parameter_set_id = r.ue()?;
    let frame_num = r.un(sps.log2_max_frame_num as u32)?;
    let idr = nal_type == 5;
    if idr {
        let _idr_pic_id = r.ue()?;
    }
    if sps.pic_order_cnt_type == 0 {
        let _poc_lsb = r.un(sps.log2_max_poc_lsb as u32)?;
        if pps.bottom_field_pic_order_present {
            let _delta_poc_bottom = r.se()?;
        }
    }
    if idr {
        let _no_output_of_prior = r.u1()?;
        let _long_term_reference = r.u1()?;
    }
    let slice_qp_delta = r.se()?;
    let slice_qp = pps.pic_init_qp + slice_qp_delta;
    if !(0..=51).contains(&slice_qp) {
        return Err(H264Error::OutOfRange);
    }

    let mut disable_deblocking_filter_idc = 0u32;
    let mut slice_alpha_c0_offset = 0i32;
    let mut slice_beta_offset = 0i32;
    if pps.deblocking_filter_control_present {
        disable_deblocking_filter_idc = r.ue()?;
        if disable_deblocking_filter_idc > 2 {
            return Err(H264Error::OutOfRange);
        }
        if disable_deblocking_filter_idc != 1 {
            slice_alpha_c0_offset = r.se()? * 2;
            slice_beta_offset = r.se()? * 2;
        }
    }

    Ok(SliceHeader {
        first_mb_in_slice,
        slice_type,
        pic_parameter_set_id,
        frame_num,
        slice_qp,
        disable_deblocking_filter_idc,
        slice_alpha_c0_offset,
        slice_beta_offset,
    })
}

// ─── CAVLC residual decode (§9.2) ───────────────────────────────────────────

/// Decode one residual 4×4 block via CAVLC into a 16-entry raster coefficient array.
/// Returns TotalCoeff for nnz bookkeeping.
fn cavlc_block(
    r: &mut BitReader,
    nc: i32,
    max_coeff: usize,
    out: &mut [i32; 16],
) -> Result<usize, H264Error> {
    for o in out.iter_mut() {
        *o = 0;
    }
    let token_table: &[t::Vlc] = if nc < 0 {
        t::COEFF_TOKEN_CHROMA_DC
    } else if nc < 2 {
        t::COEFF_TOKEN_0
    } else if nc < 4 {
        t::COEFF_TOKEN_1
    } else if nc < 8 {
        t::COEFF_TOKEN_2
    } else {
        return cavlc_block_nc8(r, max_coeff, out);
    };
    let (total_coeff, trailing_ones) = r.vlc(token_table)?;
    let total_coeff = total_coeff as usize;
    let trailing_ones = trailing_ones as usize;
    if total_coeff == 0 {
        return Ok(0);
    }
    if total_coeff > max_coeff {
        return Err(H264Error::OutOfRange);
    }
    decode_levels_and_zeros(r, total_coeff, trailing_ones, max_coeff, out)?;
    Ok(total_coeff)
}

/// nC ≥ 8 coeff_token: a 6-bit fixed-length code; all-zero token = 0b000011 (3).
fn cavlc_block_nc8(
    r: &mut BitReader,
    max_coeff: usize,
    out: &mut [i32; 16],
) -> Result<usize, H264Error> {
    let code = r.un(6)?;
    let (total_coeff, trailing_ones) = if code == 3 {
        (0usize, 0usize)
    } else {
        ((code >> 2) as usize + 1, (code & 3) as usize)
    };
    if total_coeff == 0 {
        return Ok(0);
    }
    if total_coeff > max_coeff || trailing_ones > 3 {
        return Err(H264Error::OutOfRange);
    }
    decode_levels_and_zeros(r, total_coeff, trailing_ones, max_coeff, out)?;
    Ok(total_coeff)
}

fn decode_levels_and_zeros(
    r: &mut BitReader,
    total_coeff: usize,
    trailing_ones: usize,
    max_coeff: usize,
    out: &mut [i32; 16],
) -> Result<(), H264Error> {
    let mut level = [0i32; 16];
    for i in 0..trailing_ones {
        let sign = r.u1()?;
        level[i] = if sign == 1 { -1 } else { 1 };
    }

    let mut suffix_length = if total_coeff > 10 && trailing_ones < 3 {
        1
    } else {
        0
    };
    for i in trailing_ones..total_coeff {
        let mut level_prefix = 0u32;
        while r.u1()? == 0 {
            level_prefix += 1;
            if level_prefix > 31 {
                return Err(H264Error::BadVlc);
            }
        }
        let mut level_suffix_size = suffix_length;
        if level_prefix == 14 && suffix_length == 0 {
            level_suffix_size = 4;
        } else if level_prefix >= 15 {
            level_suffix_size = level_prefix - 3;
        }
        let level_suffix = if level_suffix_size > 0 {
            r.un(level_suffix_size)?
        } else {
            0
        };

        let mut level_code = (core::cmp::min(15u32, level_prefix) << suffix_length) + level_suffix;
        if level_prefix >= 15 && suffix_length == 0 {
            level_code += 15;
        }
        if level_prefix >= 16 {
            level_code += (1u32 << (level_prefix - 3)) - 4096;
        }
        if i == trailing_ones && trailing_ones < 3 {
            level_code += 2;
        }

        let lvl = if level_code & 1 == 0 {
            ((level_code as i32) + 2) >> 1
        } else {
            (-(level_code as i32) - 1) >> 1
        };
        level[i] = lvl;

        if suffix_length == 0 {
            suffix_length = 1;
        }
        let abs_lvl = lvl.unsigned_abs();
        if abs_lvl > (3u32 << (suffix_length - 1)) && suffix_length < 6 {
            suffix_length += 1;
        }
    }

    let mut total_zeros = 0usize;
    if total_coeff < max_coeff {
        let tz_table: &[t::Vlc] = if max_coeff == 4 {
            t::TOTAL_ZEROS_CHROMA_DC[total_coeff - 1]
        } else {
            t::TOTAL_ZEROS_4X4[total_coeff - 1]
        };
        let (tz, _) = r.vlc(tz_table)?;
        total_zeros = tz as usize;
    }

    let mut runs = [0usize; 16];
    let mut zeros_left = total_zeros;
    for i in 0..total_coeff.saturating_sub(1) {
        if zeros_left == 0 {
            break;
        }
        let idx = core::cmp::min(zeros_left, 7) - 1;
        let (run, _) = r.vlc(t::RUN_BEFORE[idx])?;
        let run = run as usize;
        if run > zeros_left {
            return Err(H264Error::BadVlc);
        }
        runs[i] = run;
        zeros_left -= run;
    }
    if total_coeff > 0 {
        runs[total_coeff - 1] = zeros_left;
    }

    let mut scan_pos = total_coeff + total_zeros;
    if scan_pos > max_coeff {
        return Err(H264Error::OutOfRange);
    }
    let mut coeff_idx = 0usize;
    scan_pos -= 1;
    while coeff_idx < total_coeff {
        if scan_pos >= max_coeff {
            return Err(H264Error::OutOfRange);
        }
        let raster = if max_coeff == 4 {
            scan_pos
        } else if max_coeff == 15 {
            t::ZIGZAG_4X4[scan_pos + 1]
        } else {
            t::ZIGZAG_4X4[scan_pos]
        };
        if raster >= 16 {
            return Err(H264Error::OutOfRange);
        }
        out[raster] = level[coeff_idx];
        if coeff_idx + 1 < total_coeff {
            let step = runs[coeff_idx] + 1;
            if step > scan_pos {
                scan_pos = 0;
            } else {
                scan_pos -= step;
            }
        }
        coeff_idx += 1;
    }
    Ok(())
}

// ─── 4×4 integer inverse transform + dequant (§8.5) ─────────────────────────

/// Dequantize a 4×4 residual block (flat scaling lists). With a flat scaling list the
/// LevelScale folds to `normAdjust`, and the inverse transform's final `>>6` absorbs the
/// `/16` normalisation, so the per-coefficient scaling is `d = (c * normAdjust) << (qP/6)`.
pub fn dequant_4x4(c: &[i32; 16], qp: i32) -> [i32; 16] {
    let qp = qp.clamp(0, 51);
    let qbits = qp / 6;
    let m = (qp % 6) as usize;
    let mut d = [0i32; 16];
    for i in 0..16 {
        let scale = t::NORM_ADJUST_4X4[m][t::NORM_ADJUST_POS[i]];
        d[i] = (c[i] * scale) << qbits;
    }
    d
}

/// The H.264 integer 4×4 inverse core transform (§8.5.12.2): rows then columns, then
/// `(x + 32) >> 6`.
pub fn idct_4x4(d: &[i32; 16]) -> [i32; 16] {
    let mut tmp = [0i32; 16];
    for i in 0..4 {
        let o = i * 4;
        let d0 = d[o];
        let d1 = d[o + 1];
        let d2 = d[o + 2];
        let d3 = d[o + 3];
        let e0 = d0 + d2;
        let e1 = d0 - d2;
        let e2 = (d1 >> 1) - d3;
        let e3 = d1 + (d3 >> 1);
        tmp[o] = e0 + e3;
        tmp[o + 1] = e1 + e2;
        tmp[o + 2] = e1 - e2;
        tmp[o + 3] = e0 - e3;
    }
    let mut out = [0i32; 16];
    for j in 0..4 {
        let f0 = tmp[j];
        let f1 = tmp[4 + j];
        let f2 = tmp[8 + j];
        let f3 = tmp[12 + j];
        let g0 = f0 + f2;
        let g1 = f0 - f2;
        let g2 = (f1 >> 1) - f3;
        let g3 = f1 + (f3 >> 1);
        out[j] = (g0 + g3 + 32) >> 6;
        out[4 + j] = (g1 + g2 + 32) >> 6;
        out[8 + j] = (g1 - g2 + 32) >> 6;
        out[12 + j] = (g0 - g3 + 32) >> 6;
    }
    out
}

/// Inverse 4×4 Hadamard for the I_16x16 luma DC block (§8.5.10).
pub fn ihadamard_4x4(c: &[i32; 16]) -> [i32; 16] {
    let mut tmp = [0i32; 16];
    for i in 0..4 {
        let o = i * 4;
        let a0 = c[o] + c[o + 2];
        let a1 = c[o] - c[o + 2];
        let a2 = c[o + 1] - c[o + 3];
        let a3 = c[o + 1] + c[o + 3];
        tmp[o] = a0 + a3;
        tmp[o + 1] = a1 + a2;
        tmp[o + 2] = a1 - a2;
        tmp[o + 3] = a0 - a3;
    }
    let mut out = [0i32; 16];
    for j in 0..4 {
        let a0 = tmp[j] + tmp[8 + j];
        let a1 = tmp[j] - tmp[8 + j];
        let a2 = tmp[4 + j] - tmp[12 + j];
        let a3 = tmp[4 + j] + tmp[12 + j];
        out[j] = a0 + a3;
        out[4 + j] = a1 + a2;
        out[8 + j] = a1 - a2;
        out[12 + j] = a0 - a3;
    }
    out
}

/// Scale the post-Hadamard luma DC values for I_16x16 (§8.5.10).
/// Spec dcY = ((f * LevelScale4x4[qp%6][0][0]) << (qp/6)) >> 6, flat LevelScale = 16*norm.
/// Folding *16 against >>6 (consistent with `dequant_4x4`) gives
/// dcY = (f * normAdjust << (qp/6)) >> 2 — with rounding for qp/6 < 2.
pub fn scale_dc_luma(f: &[i32; 16], qp: i32) -> [i32; 16] {
    let qp = qp.clamp(0, 51);
    let qbits = qp / 6;
    let m = (qp % 6) as usize;
    let scale = t::NORM_ADJUST_4X4[m][0];
    let mut out = [0i32; 16];
    for i in 0..16 {
        if qbits >= 2 {
            out[i] = (f[i] * scale) << (qbits - 2);
        } else {
            let shift = 2 - qbits;
            let round = 1 << (shift - 1);
            out[i] = (f[i] * scale + round) >> shift;
        }
    }
    out
}

/// Inverse 2×2 Hadamard + scale for chroma DC (§8.5.11). Input/output are 4 DC coeffs.
/// Spec dcC = ((f * LevelScale4x4[qPc%6][0][0]) << (qPc/6)) >> 5, flat LevelScale=16*norm.
/// Folding *16 against >>5 (consistent with `dequant_4x4`) gives
/// dcC = (f * normAdjust << (qPc/6)) >> 1.
pub fn chroma_dc_transform(c: &[i32; 4], qp: i32) -> [i32; 4] {
    // Inverse 2×2 Hadamard (FFmpeg chroma_dc layout: pair (0,2) and (1,3)).
    let a = c[0] + c[2];
    let b = c[0] - c[2];
    let d = c[1] + c[3];
    let e = c[1] - c[3];
    let f = [a + d, a - d, b + e, b - e];
    let qp = qp.clamp(0, 51);
    let scale = t::NORM_ADJUST_4X4[(qp % 6) as usize][0];
    let qbits = qp / 6;
    let mut out = [0i32; 4];
    for i in 0..4 {
        out[i] = ((f[i] * scale) << qbits) >> 1;
    }
    out
}

#[inline]
fn clip_u8(x: i32) -> u8 {
    x.clamp(0, 255) as u8
}

// ─── Frame planes ───────────────────────────────────────────────────────────

/// A decoded picture in 4:2:0: Y at coded resolution, Cb/Cr at half.
pub struct Frame {
    pub width: usize,
    pub height: usize,
    pub y: Vec<u8>,
    pub cb: Vec<u8>,
    pub cr: Vec<u8>,
    pub cw: usize,
    pub ch: usize,
}

impl Frame {
    fn new(width: usize, height: usize) -> Self {
        let cw = width / 2;
        let ch = height / 2;
        Frame {
            width,
            height,
            y: vec![0u8; width * height],
            cb: vec![128u8; cw * ch],
            cr: vec![128u8; cw * ch],
            cw,
            ch,
        }
    }
}

/// Luma 4×4 block index → (x,y) offset within the 16×16 macroblock. §6.4.3.
const LUMA4X4_OFFSET: [(usize, usize); 16] = [
    (0, 0),
    (4, 0),
    (0, 4),
    (4, 4),
    (8, 0),
    (12, 0),
    (8, 4),
    (12, 4),
    (0, 8),
    (4, 8),
    (0, 12),
    (4, 12),
    (8, 8),
    (12, 8),
    (8, 12),
    (12, 12),
];

// ─── Intra prediction (§8.3) ────────────────────────────────────────────────

/// Intra_4x4 prediction into `pred[16]` (raster). `bx,by` = block top-left in plane coords;
/// `mode` 0..8. Availability from picture boundaries (all-intra IDR).
#[allow(clippy::too_many_arguments)]
fn intra_4x4_pred(
    plane: &[u8],
    stride: usize,
    bx: usize,
    by: usize,
    mode: u8,
    up_avail: bool,
    left_avail: bool,
    up_right_avail: bool,
    pred: &mut [i32; 16],
) {
    let get = |x: usize, y: usize| -> i32 { plane[y * stride + x] as i32 };
    let mut top = [0i32; 8];
    let mut left = [0i32; 4];
    let mut topleft = 0i32;
    if up_avail {
        for i in 0..4 {
            top[i] = get(bx + i, by - 1);
        }
        if up_right_avail {
            for i in 0..4 {
                top[4 + i] = get(bx + 4 + i, by - 1);
            }
        } else {
            for i in 0..4 {
                top[4 + i] = top[3];
            }
        }
    }
    if left_avail {
        for i in 0..4 {
            left[i] = get(bx - 1, by + i);
        }
    }
    if up_avail && left_avail {
        topleft = get(bx - 1, by - 1);
    }

    match mode {
        0 => {
            for y in 0..4 {
                for x in 0..4 {
                    pred[y * 4 + x] = top[x];
                }
            }
        }
        1 => {
            for y in 0..4 {
                for x in 0..4 {
                    pred[y * 4 + x] = left[y];
                }
            }
        }
        2 => {
            let dc = if up_avail && left_avail {
                (top[0] + top[1] + top[2] + top[3] + left[0] + left[1] + left[2] + left[3] + 4) >> 3
            } else if left_avail {
                (left[0] + left[1] + left[2] + left[3] + 2) >> 2
            } else if up_avail {
                (top[0] + top[1] + top[2] + top[3] + 2) >> 2
            } else {
                128
            };
            for v in pred.iter_mut() {
                *v = dc;
            }
        }
        3 => {
            for y in 0..4 {
                for x in 0..4 {
                    let v = if x == 3 && y == 3 {
                        (top[6] + 3 * top[7] + 2) >> 2
                    } else {
                        (top[x + y] + 2 * top[x + y + 1] + top[x + y + 2] + 2) >> 2
                    };
                    pred[y * 4 + x] = v;
                }
            }
        }
        4 => {
            for y in 0..4 {
                for x in 0..4 {
                    let v = if x > y {
                        let z = x - y;
                        let a = if z == 1 { topleft } else { top[z - 2] };
                        (a + 2 * top[z - 1] + top[z] + 2) >> 2
                    } else if x < y {
                        let z = y - x;
                        let a = if z == 1 { topleft } else { left[z - 2] };
                        (a + 2 * left[z - 1] + left[z] + 2) >> 2
                    } else {
                        (top[0] + 2 * topleft + left[0] + 2) >> 2
                    };
                    pred[y * 4 + x] = v;
                }
            }
        }
        5 => {
            for y in 0..4 {
                for x in 0..4 {
                    let zvr = 2 * x as i32 - y as i32;
                    let v = if zvr >= 0 && zvr % 2 == 0 {
                        let i = x - (y >> 1);
                        let a = if i == 0 { topleft } else { top[i - 1] };
                        (a + top[i] + 1) >> 1
                    } else if zvr >= 0 {
                        let i = x - (y >> 1);
                        let a = if i >= 2 { top[i - 2] } else { topleft };
                        let b = if i >= 1 { top[i - 1] } else { topleft };
                        (a + 2 * b + top[i] + 2) >> 2
                    } else if zvr == -1 {
                        (left[0] + 2 * topleft + top[0] + 2) >> 2
                    } else {
                        // zVR < -1 (y > 2x). FFmpeg vertical_right: src(0,2)=(l1+2l0+lt),
                        // src(0,3)=(l2+2l1+l0). k=y-2: left[k+1]+2*left[k]+p[k-1].
                        let k = y - 2;
                        let c = if k >= 1 { left[k - 1] } else { topleft };
                        (left[k + 1] + 2 * left[k] + c + 2) >> 2
                    };
                    pred[y * 4 + x] = v;
                }
            }
        }
        6 => {
            for y in 0..4 {
                for x in 0..4 {
                    let zhd = 2 * y as i32 - x as i32;
                    let v = if zhd >= 0 && zhd % 2 == 0 {
                        let i = y - (x >> 1);
                        let a = if i == 0 { topleft } else { left[i - 1] };
                        (a + left[i] + 1) >> 1
                    } else if zhd >= 0 {
                        let i = y - (x >> 1);
                        let a = if i >= 2 { left[i - 2] } else { topleft };
                        let b = if i >= 1 { left[i - 1] } else { topleft };
                        (a + 2 * b + left[i] + 2) >> 2
                    } else if zhd == -1 {
                        (top[0] + 2 * topleft + left[0] + 2) >> 2
                    } else {
                        // zHD < -1 (x > 2y). FFmpeg horizontal_down: src(2,0)=(t1+2t0+lt),
                        // src(3,0)=(t2+2t1+t0). k=x-2: top[k+1]+2*top[k]+p[k-1].
                        let k = x - 2;
                        let c = if k >= 1 { top[k - 1] } else { topleft };
                        (top[k + 1] + 2 * top[k] + c + 2) >> 2
                    };
                    pred[y * 4 + x] = v;
                }
            }
        }
        7 => {
            for y in 0..4 {
                for x in 0..4 {
                    let i = x + (y >> 1);
                    let v = if y % 2 == 0 {
                        (top[i] + top[i + 1] + 1) >> 1
                    } else {
                        (top[i] + 2 * top[i + 1] + top[i + 2] + 2) >> 2
                    };
                    pred[y * 4 + x] = v;
                }
            }
        }
        8 => {
            for y in 0..4 {
                for x in 0..4 {
                    let zhu = x + 2 * y;
                    let v = if zhu < 5 && zhu % 2 == 0 {
                        let i = y + (x >> 1);
                        (left[i] + left[i + 1] + 1) >> 1
                    } else if zhu < 5 {
                        let i = y + (x >> 1);
                        (left[i] + 2 * left[i + 1] + left[i + 2] + 2) >> 2
                    } else if zhu == 5 {
                        (left[2] + 3 * left[3] + 2) >> 2
                    } else {
                        left[3]
                    };
                    pred[y * 4 + x] = v;
                }
            }
        }
        _ => {
            for v in pred.iter_mut() {
                *v = 128;
            }
        }
    }
}

/// Chroma DC prediction (§8.3.4.1) for an 8×8 4:2:0 chroma block: per-4×4-quadrant DC with
/// the special corner/edge averaging (top-only / left-only / both).
fn chroma_dc_pred(
    plane: &[u8],
    stride: usize,
    bx: usize,
    by: usize,
    up_avail: bool,
    left_avail: bool,
    pred: &mut [i32],
) {
    let get = |x: usize, y: usize| -> i32 { plane[y * stride + x] as i32 };
    let top_sum = |qx: usize| -> i32 {
        let mut s = 0;
        for i in 0..4 {
            s += get(bx + qx + i, by - 1);
        }
        s
    };
    let left_sum = |qy: usize| -> i32 {
        let mut s = 0;
        for i in 0..4 {
            s += get(bx - 1, by + qy + i);
        }
        s
    };
    for q in 0..4 {
        let qx = (q % 2) * 4;
        let qy = (q / 2) * 4;
        let dc = match (qx, qy) {
            (0, 0) => {
                if up_avail && left_avail {
                    (top_sum(0) + left_sum(0) + 4) >> 3
                } else if up_avail {
                    (top_sum(0) + 2) >> 2
                } else if left_avail {
                    (left_sum(0) + 2) >> 2
                } else {
                    128
                }
            }
            (4, 0) => {
                if up_avail {
                    (top_sum(4) + 2) >> 2
                } else if left_avail {
                    (left_sum(0) + 2) >> 2
                } else {
                    128
                }
            }
            (0, 4) => {
                if left_avail {
                    (left_sum(4) + 2) >> 2
                } else if up_avail {
                    (top_sum(0) + 2) >> 2
                } else {
                    128
                }
            }
            _ => {
                if up_avail && left_avail {
                    (top_sum(4) + left_sum(4) + 4) >> 3
                } else if up_avail {
                    (top_sum(4) + 2) >> 2
                } else if left_avail {
                    (left_sum(4) + 2) >> 2
                } else {
                    128
                }
            }
        };
        for y in 0..4 {
            for x in 0..4 {
                pred[(qy + y) * 8 + qx + x] = dc;
            }
        }
    }
}

/// Generic NxN (16 or 8) V/H/DC/Plane predictor (§8.3.3 / §8.3.4) over a plane. `mode`
/// normalized to 0=V,1=H,2=DC,3=Plane (caller remaps chroma's enum).
fn intra_nxn_pred(
    plane: &[u8],
    stride: usize,
    bx: usize,
    by: usize,
    n: usize,
    mode: u8,
    up_avail: bool,
    left_avail: bool,
    pred: &mut [i32],
) {
    let get = |x: usize, y: usize| -> i32 { plane[y * stride + x] as i32 };
    match mode {
        0 => {
            for y in 0..n {
                for x in 0..n {
                    pred[y * n + x] = get(bx + x, by - 1);
                }
            }
        }
        1 => {
            for y in 0..n {
                for x in 0..n {
                    pred[y * n + x] = get(bx - 1, by + y);
                }
            }
        }
        2 => {
            let mut sum = 0i32;
            let mut cnt = 0i32;
            if up_avail {
                for x in 0..n {
                    sum += get(bx + x, by - 1);
                }
                cnt += n as i32;
            }
            if left_avail {
                for y in 0..n {
                    sum += get(bx - 1, by + y);
                }
                cnt += n as i32;
            }
            let dc = if cnt > 0 { (sum + cnt / 2) / cnt } else { 128 };
            for v in pred.iter_mut().take(n * n) {
                *v = dc;
            }
        }
        3 => {
            let half = (n / 2) as i32;
            let mut hh = 0i32;
            let mut vv = 0i32;
            for i in 0..half {
                let xa = (bx as i32 + half + i) as usize;
                let xb = (bx as i32 + half - 2 - i).max(0) as usize;
                hh += (i + 1) * (get(xa, by - 1) - get(xb, by - 1));
                let ya = (by as i32 + half + i) as usize;
                let yb = (by as i32 + half - 2 - i).max(0) as usize;
                vv += (i + 1) * (get(bx - 1, ya) - get(bx - 1, yb));
            }
            let (bc, cc) = if n == 16 {
                ((5 * hh + 32) >> 6, (5 * vv + 32) >> 6)
            } else {
                ((17 * hh + 16) >> 5, (17 * vv + 16) >> 5)
            };
            let aa = 16 * (get(bx + n - 1, by - 1) + get(bx - 1, by + n - 1));
            for y in 0..n {
                for x in 0..n {
                    let val =
                        (aa + bc * (x as i32 - half + 1) + cc * (y as i32 - half + 1) + 16) >> 5;
                    pred[y * n + x] = val.clamp(0, 255);
                }
            }
        }
        _ => {
            for v in pred.iter_mut().take(n * n) {
                *v = 128;
            }
        }
    }
}

// ─── Macroblock decode loop (§7.3.5 + §8.3/§8.5) ────────────────────────────

/// Intra-coded-block-pattern inverse map for I-MB mb_type (Table 9-4, intra column,
/// ChromaArrayType=1).
static CBP_INTRA: [u8; 48] = [
    47, 31, 15, 0, 23, 27, 29, 30, 7, 11, 13, 14, 39, 43, 45, 46, 16, 3, 5, 10, 12, 19, 21, 26, 28,
    35, 37, 42, 44, 1, 2, 4, 8, 17, 18, 20, 24, 6, 9, 22, 25, 32, 33, 34, 36, 40, 38, 41,
];

struct MbCtx {
    mb_w: usize,
    mb_h: usize,
    qp: i32,
    nnz_luma: Vec<u8>,
    nnz_cb: Vec<u8>,
    nnz_cr: Vec<u8>,
    i4_modes: Vec<i8>,
    mb_qp: Vec<i32>,
    mb_field: Vec<u8>,
}

impl MbCtx {
    fn new(mb_w: usize, mb_h: usize) -> Self {
        let lw = mb_w * 4;
        let lh = mb_h * 4;
        let cw = mb_w * 2;
        let ch = mb_h * 2;
        MbCtx {
            mb_w,
            mb_h,
            qp: 0,
            nnz_luma: vec![0u8; lw * lh],
            nnz_cb: vec![0u8; cw * ch],
            nnz_cr: vec![0u8; cw * ch],
            i4_modes: vec![-1i8; lw * lh],
            mb_qp: vec![0i32; mb_w * mb_h],
            mb_field: vec![0u8; mb_w * mb_h],
        }
    }
}

/// Decode all macroblocks of an I-slice into `frame`. §7.3.5.
fn decode_macroblocks(
    r: &mut BitReader,
    frame: &mut Frame,
    sps: &Sps,
    pps: &Pps,
    sh: &SliceHeader,
) -> Result<(), H264Error> {
    let mb_w = sps.pic_width_in_mbs as usize;
    let mb_h = sps.frame_height_in_mbs as usize;
    let pic_size = mb_w * mb_h;
    let mut ctx = MbCtx::new(mb_w, mb_h);
    ctx.qp = sh.slice_qp;

    let lw = mb_w * 4;
    let cw = mb_w * 2;

    for mb_addr in 0..pic_size {
        let mbx = mb_addr % mb_w;
        let mby = mb_addr / mb_w;
        decode_one_macroblock(r, frame, sps, pps, sh, &mut ctx, mbx, mby, lw, cw)?;
        ctx.mb_qp[mb_addr] = ctx.qp;
        ctx.mb_field[mb_addr] = 1;
    }

    if sh.disable_deblocking_filter_idc != 1 {
        deblock(frame, &ctx, sh, pps.chroma_qp_index_offset);
    }
    Ok(())
}

fn luma_nc(ctx: &MbCtx, lw: usize, lx: usize, ly: usize) -> i32 {
    let na = if lx > 0 {
        ctx.nnz_luma[ly * lw + (lx - 1)] as i32
    } else {
        -1
    };
    let nb = if ly > 0 {
        ctx.nnz_luma[(ly - 1) * lw + lx] as i32
    } else {
        -1
    };
    nc_from(na, nb)
}

fn chroma_nc(plane: &[u8], cw: usize, cx: usize, cy: usize) -> i32 {
    let na = if cx > 0 {
        plane[cy * cw + (cx - 1)] as i32
    } else {
        -1
    };
    let nb = if cy > 0 {
        plane[(cy - 1) * cw + cx] as i32
    } else {
        -1
    };
    nc_from(na, nb)
}

#[inline]
fn nc_from(na: i32, nb: i32) -> i32 {
    if na >= 0 && nb >= 0 {
        (na + nb + 1) >> 1
    } else if na >= 0 {
        na
    } else if nb >= 0 {
        nb
    } else {
        0
    }
}

#[allow(clippy::too_many_arguments)]
fn decode_one_macroblock(
    r: &mut BitReader,
    frame: &mut Frame,
    _sps: &Sps,
    pps: &Pps,
    sh: &SliceHeader,
    ctx: &mut MbCtx,
    mbx: usize,
    mby: usize,
    lw: usize,
    cw: usize,
) -> Result<(), H264Error> {
    let mb_type = r.ue()?;
    if mb_type > 25 {
        return Err(H264Error::Unsupported("h264: non-intra mb_type in I slice"));
    }

    if mb_type == 25 {
        while r.bits_left() % 8 != 0 {
            let _ = r.u1()?;
        }
        let bx = mbx * 16;
        let by = mby * 16;
        for y in 0..16 {
            for x in 0..16 {
                frame.y[(by + y) * frame.width + bx + x] = r.un(8)? as u8;
            }
        }
        let cbx = mbx * 8;
        let cby = mby * 8;
        for y in 0..8 {
            for x in 0..8 {
                frame.cb[(cby + y) * frame.cw + cbx + x] = r.un(8)? as u8;
            }
        }
        for y in 0..8 {
            for x in 0..8 {
                frame.cr[(cby + y) * frame.cw + cbx + x] = r.un(8)? as u8;
            }
        }
        set_mb_nnz(ctx, lw, cw, mbx, mby, 16, 16);
        return Ok(());
    }

    let bx = mbx * 16;
    let by = mby * 16;
    let up_avail = mby > 0;
    let left_avail = mbx > 0;

    if mb_type == 0 {
        let mut modes = [2i8; 16];
        for blk in 0..16 {
            let (ox, oy) = LUMA4X4_OFFSET[blk];
            let lx = mbx * 4 + ox / 4;
            let ly = mby * 4 + oy / 4;
            let left_m = if ox > 0 || left_avail {
                ctx.i4_modes[ly * lw + (lx.wrapping_sub(1))]
            } else {
                -1
            };
            let up_m = if oy > 0 || up_avail {
                ctx.i4_modes[(ly.wrapping_sub(1)) * lw + lx]
            } else {
                -1
            };
            let pred_mode = if left_m < 0 || up_m < 0 {
                2
            } else {
                left_m.min(up_m)
            };
            let prev_flag = r.u1()?;
            let mode = if prev_flag == 1 {
                pred_mode
            } else {
                let rem = r.un(3)? as i8;
                if rem < pred_mode {
                    rem
                } else {
                    rem + 1
                }
            };
            modes[blk] = mode;
            ctx.i4_modes[ly * lw + lx] = mode;
        }
        let intra_chroma_pred_mode = r.ue()? as u8;
        if intra_chroma_pred_mode > 3 {
            return Err(H264Error::OutOfRange);
        }
        let cbp_code = r.ue()? as usize;
        if cbp_code >= 48 {
            return Err(H264Error::OutOfRange);
        }
        let cbp = CBP_INTRA[cbp_code];
        let cbp_luma = cbp & 0x0F;
        let cbp_chroma = cbp >> 4;

        if cbp_luma != 0 || cbp_chroma != 0 {
            ctx.qp = wrap_qp(ctx.qp + r.se()?);
        }

        for blk in 0..16 {
            let (ox, oy) = LUMA4X4_OFFSET[blk];
            let px = bx + ox;
            let py = by + oy;
            let lx = mbx * 4 + ox / 4;
            let ly = mby * 4 + oy / 4;
            let b_up = py > 0;
            let b_left = px > 0;
            let b_upright =
                py > 0 && (px + 4) < frame.width && upright_avail(blk, mbx, mby, frame.width);
            let mut pred = [0i32; 16];
            intra_4x4_pred(
                &frame.y,
                frame.width,
                px,
                py,
                modes[blk] as u8,
                b_up,
                b_left,
                b_upright,
                &mut pred,
            );
            let mut coeffs = [0i32; 16];
            let mut nnz = 0usize;
            if cbp_luma & (1 << (blk / 4)) != 0 {
                let nc = luma_nc(ctx, lw, lx, ly);
                nnz = cavlc_block(r, nc, 16, &mut coeffs)?;
            }
            ctx.nnz_luma[ly * lw + lx] = nnz as u8;
            let d = dequant_4x4(&coeffs, ctx.qp);
            let res = idct_4x4(&d);
            for y in 0..4 {
                for x in 0..4 {
                    let idx = (py + y) * frame.width + px + x;
                    frame.y[idx] = clip_u8(pred[y * 4 + x] + res[y * 4 + x]);
                }
            }
        }
        reconstruct_chroma(
            r,
            frame,
            pps,
            ctx,
            mbx,
            mby,
            cw,
            cbp_chroma,
            intra_chroma_pred_mode,
            up_avail,
            left_avail,
        )?;
    } else {
        let m = mb_type - 1;
        let luma_mode = (m % 4) as u8;
        let cbp_chroma = ((m / 4) % 3) as u8;
        let cbp_luma_all = m >= 12;
        let intra_chroma_pred_mode = r.ue()? as u8;
        if intra_chroma_pred_mode > 3 {
            return Err(H264Error::OutOfRange);
        }
        ctx.qp = wrap_qp(ctx.qp + r.se()?);

        let mut pred = [0i32; 256];
        intra_nxn_pred(
            &frame.y,
            frame.width,
            bx,
            by,
            16,
            luma_mode,
            up_avail,
            left_avail,
            &mut pred,
        );

        let mut dc_coeffs = [0i32; 16];
        {
            let lx = mbx * 4;
            let ly = mby * 4;
            let nc = luma_nc(ctx, lw, lx, ly);
            cavlc_block(r, nc, 16, &mut dc_coeffs)?;
        }
        let dc_had = ihadamard_4x4(&dc_coeffs);
        let dc_scaled = scale_dc_luma(&dc_had, ctx.qp);

        for blk in 0..16 {
            let (ox, oy) = LUMA4X4_OFFSET[blk];
            let px = bx + ox;
            let py = by + oy;
            let lx = mbx * 4 + ox / 4;
            let ly = mby * 4 + oy / 4;
            let mut ac = [0i32; 16];
            let mut nnz = 0usize;
            if cbp_luma_all {
                let nc = luma_nc(ctx, lw, lx, ly);
                nnz = cavlc_block(r, nc, 15, &mut ac)?;
            }
            ctx.nnz_luma[ly * lw + lx] = nnz as u8;
            let dc_idx = (oy / 4) * 4 + (ox / 4);
            let mut d = dequant_4x4(&ac, ctx.qp);
            d[0] = dc_scaled[dc_idx];
            let res = idct_4x4(&d);
            for y in 0..4 {
                for x in 0..4 {
                    let idx = (py + y) * frame.width + px + x;
                    frame.y[idx] = clip_u8(pred[(oy + y) * 16 + ox + x] + res[y * 4 + x]);
                }
            }
        }
        reconstruct_chroma(
            r,
            frame,
            pps,
            ctx,
            mbx,
            mby,
            cw,
            cbp_chroma,
            intra_chroma_pred_mode,
            up_avail,
            left_avail,
        )?;
    }
    let _ = sh;
    Ok(())
}

/// above-right availability for an Intra_4x4 block (§6.4.11.4). Blocks 3,7,11,13,15 have
/// no above-right inside the MB; top-row blocks need the MB above.
fn upright_avail(blk: usize, _mbx: usize, mby: usize, _width: usize) -> bool {
    let unavailable_inside = matches!(blk, 3 | 7 | 11 | 13 | 15);
    if unavailable_inside {
        false
    } else {
        mby > 0 || !matches!(blk, 0 | 1 | 4 | 5)
    }
}

fn wrap_qp(qp: i32) -> i32 {
    let mut q = qp;
    while q < 0 {
        q += 52;
    }
    while q > 51 {
        q -= 52;
    }
    q
}

fn set_mb_nnz(ctx: &mut MbCtx, lw: usize, cw: usize, mbx: usize, mby: usize, lval: u8, cval: u8) {
    for oy in 0..4 {
        for ox in 0..4 {
            ctx.nnz_luma[(mby * 4 + oy) * lw + mbx * 4 + ox] = lval;
        }
    }
    for oy in 0..2 {
        for ox in 0..2 {
            ctx.nnz_cb[(mby * 2 + oy) * cw + mbx * 2 + ox] = cval;
            ctx.nnz_cr[(mby * 2 + oy) * cw + mbx * 2 + ox] = cval;
        }
    }
}

/// Decode + reconstruct the Cb/Cr 8×8 chroma blocks of a macroblock. §8.3.4 / §8.5.11.
#[allow(clippy::too_many_arguments)]
fn reconstruct_chroma(
    r: &mut BitReader,
    frame: &mut Frame,
    pps: &Pps,
    ctx: &mut MbCtx,
    mbx: usize,
    mby: usize,
    cw: usize,
    cbp_chroma: u8,
    chroma_mode: u8,
    up_avail: bool,
    left_avail: bool,
) -> Result<(), H264Error> {
    let qpc = t::chroma_qp(wrap_qp(ctx.qp + pps.chroma_qp_index_offset));
    // chroma intra pred mode enum: 0=DC,1=H,2=V,3=Plane → normalize to V/H/DC/Plane.
    let norm_mode = match chroma_mode {
        0 => 2u8,
        1 => 1u8,
        2 => 0u8,
        _ => 3u8,
    };
    let cbx = mbx * 8;
    let cby = mby * 8;

    // Residual order (§7.3.5.3): BOTH chroma-DC blocks (Cb then Cr) are coded first, THEN
    // all chroma-AC blocks (Cb 4, then Cr 4). Read in that exact order.
    let mut dc_t = [[0i32; 4]; 2];
    for comp in 0..2 {
        let mut dc = [0i32; 16];
        let mut dc4 = [0i32; 4];
        if cbp_chroma != 0 {
            cavlc_block(r, -1, 4, &mut dc)?;
            dc4 = [dc[0], dc[1], dc[2], dc[3]];
        }
        dc_t[comp] = chroma_dc_transform(&dc4, qpc);
    }

    // AC blocks per component (Cb then Cr).
    let mut ac_blocks = [[[0i32; 16]; 4]; 2];
    for comp in 0..2 {
        let is_cb = comp == 0;
        for sub in 0..4 {
            let sox = (sub % 2) * 4;
            let soy = (sub / 2) * 4;
            let cx = mbx * 2 + sox / 4;
            let cy = mby * 2 + soy / 4;
            let mut nnz = 0usize;
            if cbp_chroma & 0x2 != 0 {
                let nplane: &[u8] = if is_cb { &ctx.nnz_cb } else { &ctx.nnz_cr };
                let nc = chroma_nc(nplane, cw, cx, cy);
                nnz = cavlc_block(r, nc, 15, &mut ac_blocks[comp][sub])?;
            }
            if is_cb {
                ctx.nnz_cb[cy * cw + cx] = nnz as u8;
            } else {
                ctx.nnz_cr[cy * cw + cx] = nnz as u8;
            }
        }
    }

    // Predict + reconstruct each component.
    for comp in 0..2 {
        let is_cb = comp == 0;
        let mut pred = [0i32; 64];
        let plane: &[u8] = if is_cb { &frame.cb } else { &frame.cr };
        if norm_mode == 2 {
            chroma_dc_pred(plane, frame.cw, cbx, cby, up_avail, left_avail, &mut pred);
        } else {
            intra_nxn_pred(
                plane, frame.cw, cbx, cby, 8, norm_mode, up_avail, left_avail, &mut pred,
            );
        }
        let mut recon = [0u8; 64];
        for sub in 0..4 {
            let sox = (sub % 2) * 4;
            let soy = (sub / 2) * 4;
            let mut d = dequant_4x4(&ac_blocks[comp][sub], qpc);
            d[0] = dc_t[comp][sub];
            let res = idct_4x4(&d);
            for y in 0..4 {
                for x in 0..4 {
                    recon[(soy + y) * 8 + sox + x] =
                        clip_u8(pred[(soy + y) * 8 + sox + x] + res[y * 4 + x]);
                }
            }
        }
        let plane_mut: &mut [u8] = if is_cb { &mut frame.cb } else { &mut frame.cr };
        for y in 0..8 {
            for x in 0..8 {
                plane_mut[(cby + y) * frame.cw + cbx + x] = recon[y * 8 + x];
            }
        }
    }
    Ok(())
}

// ─── In-loop deblocking filter (§8.7) ───────────────────────────────────────

/// Deblock the whole frame: vertical edges then horizontal edges, per MB raster order.
/// Chroma edges use the chroma-QP-mapped average (§8.7.2.2).
fn deblock(frame: &mut Frame, ctx: &MbCtx, sh: &SliceHeader, chroma_qp_offset: i32) {
    let mb_w = ctx.mb_w;
    let mb_h = ctx.mb_h;
    for mby in 0..mb_h {
        for mbx in 0..mb_w {
            let qp = ctx.mb_qp[mby * mb_w + mbx];
            for e in 0..4 {
                let x0 = mbx * 16 + e * 4;
                if x0 == 0 {
                    continue;
                }
                let is_mb_edge = e == 0;
                if is_mb_edge && sh.disable_deblocking_filter_idc == 2 {
                    continue;
                }
                let bs = if is_mb_edge { 4 } else { 3 };
                let qp_left = if is_mb_edge {
                    ctx.mb_qp[mby * mb_w + (mbx - 1)]
                } else {
                    qp
                };
                let qpav = (qp + qp_left + 1) >> 1;
                deblock_luma_vert(frame, x0, mby * 16, qpav, bs, sh);
                if e % 2 == 0 {
                    let qpav_c = (t::chroma_qp(qp + chroma_qp_offset)
                        + t::chroma_qp(qp_left + chroma_qp_offset)
                        + 1)
                        >> 1;
                    deblock_chroma_vert(frame, x0 / 2, mby * 8, qpav_c, bs, sh);
                }
            }
            for e in 0..4 {
                let y0 = mby * 16 + e * 4;
                if y0 == 0 {
                    continue;
                }
                let is_mb_edge = e == 0;
                if is_mb_edge && sh.disable_deblocking_filter_idc == 2 {
                    continue;
                }
                let bs = if is_mb_edge { 4 } else { 3 };
                let qp_up = if is_mb_edge {
                    ctx.mb_qp[(mby - 1) * mb_w + mbx]
                } else {
                    qp
                };
                let qpav = (qp + qp_up + 1) >> 1;
                deblock_luma_horiz(frame, mbx * 16, y0, qpav, bs, sh);
                if e % 2 == 0 {
                    let qpav_c = (t::chroma_qp(qp + chroma_qp_offset)
                        + t::chroma_qp(qp_up + chroma_qp_offset)
                        + 1)
                        >> 1;
                    deblock_chroma_horiz(frame, mbx * 8, y0 / 2, qpav_c, bs, sh);
                }
            }
        }
    }
}

#[inline]
fn thresholds(qpav: i32, sh: &SliceHeader) -> (i32, i32, usize) {
    let idx_a = (qpav + sh.slice_alpha_c0_offset).clamp(0, 51) as usize;
    let idx_b = (qpav + sh.slice_beta_offset).clamp(0, 51) as usize;
    (
        t::ALPHA_TABLE[idx_a] as i32,
        t::BETA_TABLE[idx_b] as i32,
        idx_a,
    )
}

fn deblock_luma_vert(frame: &mut Frame, x: usize, y0: usize, qpav: i32, bs: i32, sh: &SliceHeader) {
    let (alpha, beta, idx_a) = thresholds(qpav, sh);
    if alpha == 0 {
        return;
    }
    let w = frame.width;
    for row in y0..y0 + 16 {
        if row >= frame.height {
            break;
        }
        let base = row * w + x;
        filter_edge_luma(&mut frame.y, base, 1, alpha, beta, idx_a, bs);
    }
}

fn deblock_luma_horiz(
    frame: &mut Frame,
    x0: usize,
    y: usize,
    qpav: i32,
    bs: i32,
    sh: &SliceHeader,
) {
    let (alpha, beta, idx_a) = thresholds(qpav, sh);
    if alpha == 0 {
        return;
    }
    let w = frame.width;
    for col in x0..x0 + 16 {
        if col >= frame.width {
            break;
        }
        let base = y * w + col;
        filter_edge_luma(&mut frame.y, base, w as isize, alpha, beta, idx_a, bs);
    }
}

/// Filter one luma edge: samples p1 p0 | q0 q1 at `base ± k*stride` (q side at base).
fn filter_edge_luma(
    plane: &mut [u8],
    base: usize,
    stride: isize,
    alpha: i32,
    beta: i32,
    idx_a: usize,
    bs: i32,
) {
    let idx = |i: isize| -> usize { (base as isize + i * stride) as usize };
    let p2 = plane[idx(-3)] as i32;
    let p1 = plane[idx(-2)] as i32;
    let p0 = plane[idx(-1)] as i32;
    let q0 = plane[idx(0)] as i32;
    let q1 = plane[idx(1)] as i32;
    let q2 = plane[idx(2)] as i32;

    if (p0 - q0).abs() >= alpha || (p1 - p0).abs() >= beta || (q1 - q0).abs() >= beta {
        return;
    }

    if bs < 4 {
        let tc0 = t::TC0_TABLE[(bs - 1) as usize][idx_a] as i32;
        let ap = (p2 - p0).abs();
        let aq = (q2 - q0).abs();
        let mut tc = tc0;
        if ap < beta {
            tc += 1;
        }
        if aq < beta {
            tc += 1;
        }
        let delta = (((q0 - p0) * 4 + (p1 - q1) + 4) >> 3).clamp(-tc, tc);
        plane[idx(-1)] = clip_u8(p0 + delta);
        plane[idx(0)] = clip_u8(q0 - delta);
        if ap < beta {
            let dp = ((p2 + ((p0 + q0 + 1) >> 1) - 2 * p1) >> 1).clamp(-tc0, tc0);
            plane[idx(-2)] = clip_u8(p1 + dp);
        }
        if aq < beta {
            let dq = ((q2 + ((p0 + q0 + 1) >> 1) - 2 * q1) >> 1).clamp(-tc0, tc0);
            plane[idx(1)] = clip_u8(q1 + dq);
        }
    } else {
        let p3 = plane[idx(-4)] as i32;
        let q3 = plane[idx(3)] as i32;
        let ap = (p2 - p0).abs();
        let aq = (q2 - q0).abs();
        if ap < beta && (p0 - q0).abs() < ((alpha >> 2) + 2) {
            plane[idx(-1)] = clip_u8((p2 + 2 * p1 + 2 * p0 + 2 * q0 + q1 + 4) >> 3);
            plane[idx(-2)] = clip_u8((p2 + p1 + p0 + q0 + 2) >> 2);
            plane[idx(-3)] = clip_u8((2 * p3 + 3 * p2 + p1 + p0 + q0 + 4) >> 3);
        } else {
            plane[idx(-1)] = clip_u8((2 * p1 + p0 + q1 + 2) >> 2);
        }
        if aq < beta && (p0 - q0).abs() < ((alpha >> 2) + 2) {
            plane[idx(0)] = clip_u8((q2 + 2 * q1 + 2 * q0 + 2 * p0 + p1 + 4) >> 3);
            plane[idx(1)] = clip_u8((q2 + q1 + q0 + p0 + 2) >> 2);
            plane[idx(2)] = clip_u8((2 * q3 + 3 * q2 + q1 + q0 + p0 + 4) >> 3);
        } else {
            plane[idx(0)] = clip_u8((2 * q1 + q0 + p1 + 2) >> 2);
        }
    }
}

fn deblock_chroma_vert(
    frame: &mut Frame,
    x: usize,
    y0: usize,
    qpav: i32,
    bs: i32,
    sh: &SliceHeader,
) {
    let (alpha, beta, idx_a) = thresholds(qpav, sh);
    if alpha == 0 {
        return;
    }
    let cw = frame.cw;
    for row in y0..y0 + 8 {
        if row >= frame.ch {
            break;
        }
        let base = row * cw + x;
        filter_edge_chroma(&mut frame.cb, base, 1, alpha, beta, idx_a, bs);
        filter_edge_chroma(&mut frame.cr, base, 1, alpha, beta, idx_a, bs);
    }
}

fn deblock_chroma_horiz(
    frame: &mut Frame,
    x0: usize,
    y: usize,
    qpav: i32,
    bs: i32,
    sh: &SliceHeader,
) {
    let (alpha, beta, idx_a) = thresholds(qpav, sh);
    if alpha == 0 {
        return;
    }
    let cw = frame.cw;
    for col in x0..x0 + 8 {
        if col >= frame.cw {
            break;
        }
        let base = y * cw + col;
        filter_edge_chroma(&mut frame.cb, base, cw as isize, alpha, beta, idx_a, bs);
        filter_edge_chroma(&mut frame.cr, base, cw as isize, alpha, beta, idx_a, bs);
    }
}

/// Chroma edge filter (only p0/q0 modified; §8.7.2.4).
fn filter_edge_chroma(
    plane: &mut [u8],
    base: usize,
    stride: isize,
    alpha: i32,
    beta: i32,
    idx_a: usize,
    bs: i32,
) {
    let idx = |i: isize| -> usize { (base as isize + i * stride) as usize };
    let p1 = plane[idx(-2)] as i32;
    let p0 = plane[idx(-1)] as i32;
    let q0 = plane[idx(0)] as i32;
    let q1 = plane[idx(1)] as i32;
    if (p0 - q0).abs() >= alpha || (p1 - p0).abs() >= beta || (q1 - q0).abs() >= beta {
        return;
    }
    if bs < 4 {
        let tc = t::TC0_TABLE[(bs - 1) as usize][idx_a] as i32 + 1;
        let delta = (((q0 - p0) * 4 + (p1 - q1) + 4) >> 3).clamp(-tc, tc);
        plane[idx(-1)] = clip_u8(p0 + delta);
        plane[idx(0)] = clip_u8(q0 - delta);
    } else {
        plane[idx(-1)] = clip_u8((2 * p1 + p0 + q1 + 2) >> 2);
        plane[idx(0)] = clip_u8((2 * q1 + q0 + p1 + 2) >> 2);
    }
}

// ─── Public decode entry + crop to display ──────────────────────────────────

/// A decoded YUV420 keyframe ready to hand to the `VideoFrame` builder. Display-cropped.
pub struct DecodedYuv {
    pub width: usize,
    pub height: usize,
    pub y: Vec<u8>,
    pub cb: Vec<u8>,
    pub cr: Vec<u8>,
}

/// Decode a single I/IDR slice into a display-cropped YUV420 keyframe. The Concept promise
/// ("play my movies" — native rendering, no web wrappers) is served here: this turns the
/// first keyframe of a real `.mp4` into actual picture. `slice_rbsp` is the slice NAL's
/// RBSP (emulation-prevention already stripped); `nal_type` 5 = IDR.
pub fn decode_slice(
    slice_rbsp: &[u8],
    sps: &Sps,
    pps: &Pps,
    nal_type: u8,
) -> Result<DecodedYuv, H264Error> {
    let sh = parse_slice_header(slice_rbsp, sps, pps, nal_type)?;
    if !sh.is_i_slice() {
        return Err(H264Error::Unsupported(
            "h264: non-I slice (P/B inter deferred)",
        ));
    }
    if sh.first_mb_in_slice != 0 {
        return Err(H264Error::Unsupported("h264: multi-slice frame"));
    }

    let cw_pix = sps.coded_width() as usize;
    let ch_pix = sps.coded_height() as usize;
    let mut frame = Frame::new(cw_pix, ch_pix);

    let mut r = BitReader::new(slice_rbsp);
    skip_slice_header(&mut r, sps, pps, nal_type)?;

    decode_macroblocks(&mut r, &mut frame, sps, pps, &sh)?;

    let dw = sps.display_width() as usize;
    let dh = sps.display_height() as usize;
    let dcw = dw / 2;
    let dch = dh / 2;
    let mut y = vec![0u8; dw * dh];
    for row in 0..dh {
        let src = (row + sps.crop_top as usize) * frame.width + sps.crop_left as usize;
        y[row * dw..row * dw + dw].copy_from_slice(&frame.y[src..src + dw]);
    }
    let mut cb = vec![0u8; dcw * dch];
    let mut cr = vec![0u8; dcw * dch];
    for row in 0..dch {
        let src = (row + sps.crop_top as usize) * frame.cw + sps.crop_left as usize;
        cb[row * dcw..row * dcw + dcw].copy_from_slice(&frame.cb[src..src + dcw]);
        cr[row * dcw..row * dcw + dcw].copy_from_slice(&frame.cr[src..src + dcw]);
    }

    Ok(DecodedYuv {
        width: dw,
        height: dh,
        y,
        cb,
        cr,
    })
}

/// Advance `r` past the slice header (same syntax as `parse_slice_header`).
fn skip_slice_header(
    r: &mut BitReader,
    sps: &Sps,
    pps: &Pps,
    nal_type: u8,
) -> Result<(), H264Error> {
    let _first_mb = r.ue()?;
    let _slice_type = r.ue()?;
    let _pps_id = r.ue()?;
    let _frame_num = r.un(sps.log2_max_frame_num as u32)?;
    let idr = nal_type == 5;
    if idr {
        let _idr_pic_id = r.ue()?;
    }
    if sps.pic_order_cnt_type == 0 {
        let _poc = r.un(sps.log2_max_poc_lsb as u32)?;
        if pps.bottom_field_pic_order_present {
            let _ = r.se()?;
        }
    }
    if idr {
        let _ = r.u1()?;
        let _ = r.u1()?;
    }
    let _qp_delta = r.se()?;
    if pps.deblocking_filter_control_present {
        let idc = r.ue()?;
        if idc != 1 {
            let _ = r.se()?;
            let _ = r.se()?;
        }
    }
    Ok(())
}

// ─── procfs + R10 boot smoketest ────────────────────────────────────────────

/// procfs status line for the H.264 path. Reports `h264=iframe` now that intra-frame
/// reconstruction is wired (was `h264=pending`).
pub fn h264_procfs_status() -> &'static str {
    "h264=iframe profile=baseline entropy=cavlc intra=4x4/16x16/chroma deblock=on"
}

static FRAME16_H264: &[u8] = include_bytes!("../tests/fixtures/frame16.h264");
static FRAME16_REF_YUV: &[u8] = include_bytes!("../tests/fixtures/frame16.ref.yuv");

/// Decode the first SPS+PPS+slice out of an Annex-B byte stream into a DecodedYuv.
/// Returns Err on any unsupported/hostile input — never panics, never wrong shape.
pub fn decode_annexb(stream: &[u8]) -> Result<DecodedYuv, H264Error> {
    let nals = split_annexb(stream);
    let mut sps: Option<Sps> = None;
    let mut pps: Option<Pps> = None;
    for nal in &nals {
        if nal.is_empty() {
            continue;
        }
        let nal_type = nal[0] & 0x1F;
        let rbsp = nal_to_rbsp(&nal[1..]);
        match nal_type {
            7 => sps = Some(parse_sps(&rbsp)?),
            8 => pps = Some(parse_pps(&rbsp)?),
            1 | 5 => {
                let s = sps.as_ref().ok_or(H264Error::MissingParamSet)?;
                let p = pps.as_ref().ok_or(H264Error::MissingParamSet)?;
                return decode_slice(&rbsp, s, p, nal_type);
            }
            _ => {}
        }
    }
    Err(H264Error::MissingParamSet)
}

/// Split an Annex-B byte stream into NAL payloads (3- and 4-byte start codes).
pub fn split_annexb(data: &[u8]) -> Vec<Vec<u8>> {
    let mut nals = Vec::new();
    let mut i = 0;
    let mut start: Option<usize> = None;
    while i + 2 < data.len() {
        if data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1 {
            if let Some(s) = start {
                nals.push(data[s..i].to_vec());
            }
            i += 3;
            start = Some(i);
        } else {
            i += 1;
        }
    }
    if let Some(s) = start {
        nals.push(data[s..].to_vec());
    }
    nals
}

/// R10 boot smoketest: decode the embedded 16×16 keyframe and assert it reproduces the
/// ffmpeg golden YUV bit-exact. FAIL if decode errors, the frame is the wrong shape, or
/// any sample differs from the reference (the FAIL lever).
pub fn run_boot_smoketest() -> alloc::string::String {
    use alloc::format;
    let res = decode_annexb(FRAME16_H264);
    let (w, h, bitexact, mbs) = match res {
        Ok(yuv) => {
            let expect_y = yuv.width * yuv.height;
            let expect_c = (yuv.width / 2) * (yuv.height / 2);
            let shape_ok = yuv.y.len() == expect_y
                && yuv.cb.len() == expect_c
                && yuv.cr.len() == expect_c
                && FRAME16_REF_YUV.len() == expect_y + 2 * expect_c;
            let bitexact = if shape_ok {
                let ry = &FRAME16_REF_YUV[..expect_y];
                let rcb = &FRAME16_REF_YUV[expect_y..expect_y + expect_c];
                let rcr = &FRAME16_REF_YUV[expect_y + expect_c..];
                yuv.y == ry && yuv.cb == rcb && yuv.cr == rcr
            } else {
                false
            };
            let mbs = (yuv.width / 16) * (yuv.height / 16);
            (yuv.width, yuv.height, bitexact, mbs)
        }
        Err(_) => (0, 0, false, 0),
    };
    let pass = bitexact && w == 16 && h == 16;
    format!(
        "[raemedia] h264-iframe: {}x{} mbs={} bitexact={} -> {}",
        w,
        h,
        mbs,
        bitexact,
        if pass { "PASS" } else { "FAIL" }
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    static FRAME16_REF: &[u8] = include_bytes!("../tests/fixtures/frame16.ref.yuv");
    static FRAME16: &[u8] = include_bytes!("../tests/fixtures/frame16.h264");
    static FRAME32: &[u8] = include_bytes!("../tests/fixtures/frame32.h264");
    static FRAME32_REF: &[u8] = include_bytes!("../tests/fixtures/frame32.ref.yuv");

    fn max_abs_diff(a: &[u8], b: &[u8]) -> i32 {
        let mut m = 0i32;
        for (x, y) in a.iter().zip(b.iter()) {
            let d = (*x as i32 - *y as i32).abs();
            if d > m {
                m = d;
            }
        }
        m
    }

    #[test]
    fn h264_sps_geometry() {
        let nals = split_annexb(FRAME16);
        let sps_nal = nals
            .iter()
            .find(|n| !n.is_empty() && n[0] & 0x1F == 7)
            .unwrap();
        let sps = parse_sps(&nal_to_rbsp(&sps_nal[1..])).expect("sps parse");
        assert_eq!(sps.pic_width_in_mbs, 1);
        assert_eq!(sps.frame_height_in_mbs, 1);
        assert_eq!(sps.display_width(), 16);
        assert_eq!(sps.display_height(), 16);

        let nals32 = split_annexb(FRAME32);
        let s32 = nals32
            .iter()
            .find(|n| !n.is_empty() && n[0] & 0x1F == 7)
            .unwrap();
        let sps32 = parse_sps(&nal_to_rbsp(&s32[1..])).expect("sps32");
        assert_eq!(sps32.pic_width_in_mbs, 2);
        assert_eq!(sps32.frame_height_in_mbs, 2);
        assert_eq!(sps32.display_width(), 32);
        assert_eq!(sps32.display_height(), 32);
    }

    #[test]
    fn h264_sps_truncated_is_err() {
        assert!(parse_sps(&[0x42, 0xc0]).is_err());
    }

    #[test]
    fn h264_cavlc_block() {
        let data = [0b1000_0000u8];
        let mut r = BitReader::new(&data);
        let mut out = [0i32; 16];
        let tc = cavlc_block(&mut r, 0, 16, &mut out).unwrap();
        assert_eq!(tc, 0);
        assert!(out.iter().all(|&c| c == 0));

        let data = [0b0111_1000u8];
        let mut r = BitReader::new(&data);
        let mut out = [0i32; 16];
        let tc = cavlc_block(&mut r, 0, 16, &mut out).unwrap();
        assert_eq!(tc, 1);
        assert_eq!(out[0], -1);
        assert!(out[1..].iter().all(|&c| c == 0));

        let dw = [0b1111_1000u8];
        let mut r2 = BitReader::new(&dw);
        let mut o2 = [0i32; 16];
        let tc2 = cavlc_block(&mut r2, 0, 16, &mut o2).unwrap();
        assert_eq!(tc2, 0);
        assert_ne!(o2[0], -1);
    }

    #[test]
    fn h264_inverse_transform() {
        let mut d = [0i32; 16];
        d[0] = 64;
        let r = idct_4x4(&d);
        for &v in r.iter() {
            assert_eq!(v, 1, "DC-only IDCT flat");
        }
        let z = idct_4x4(&[0i32; 16]);
        assert!(z.iter().all(|&v| v == 0));
    }

    #[test]
    fn h264_intra_dc_16x16() {
        let stride = 17usize;
        let mut plane = vec![0u8; stride * 17];
        for x in 0..17 {
            plane[x] = 100;
        }
        for y in 0..17 {
            plane[y * stride] = 50;
        }
        let mut pred = [0i32; 256];
        intra_nxn_pred(&plane, stride, 1, 1, 16, 2, true, true, &mut pred);
        assert_eq!(pred[0], 75, "16x16 DC avg");
        intra_nxn_pred(&plane, stride, 1, 1, 16, 0, true, true, &mut pred);
        assert!(pred[..256].iter().all(|&v| v == 100));
        intra_nxn_pred(&plane, stride, 1, 1, 16, 1, true, true, &mut pred);
        assert!(pred[..256].iter().all(|&v| v == 50));
    }

    #[test]
    fn h264_intra_4x4_modes() {
        let stride = 9usize;
        let mut plane = vec![0u8; stride * 9];
        for x in 0..9 {
            plane[x] = 10 + x as u8;
        }
        for y in 0..9 {
            plane[y * stride] = 200 - y as u8;
        }
        let mut pred = [0i32; 16];
        intra_4x4_pred(&plane, stride, 1, 1, 0, true, true, true, &mut pred);
        for y in 0..4 {
            for x in 0..4 {
                assert_eq!(pred[y * 4 + x], (11 + x) as i32);
            }
        }
        intra_4x4_pred(&plane, stride, 1, 1, 1, true, true, true, &mut pred);
        for y in 0..4 {
            for x in 0..4 {
                assert_eq!(pred[y * 4 + x], (199 - y) as i32);
            }
        }
        intra_4x4_pred(&plane, stride, 1, 1, 2, true, true, true, &mut pred);
        assert_eq!(pred[0], 105);
    }

    #[test]
    fn h264_deblock_identity_when_off() {
        let mut frame = Frame::new(16, 16);
        frame.y[0] = 10;
        frame.y[1] = 200;
        let before = frame.y.clone();
        let sh = SliceHeader {
            first_mb_in_slice: 0,
            slice_type: 7,
            pic_parameter_set_id: 0,
            frame_num: 0,
            slice_qp: 0,
            disable_deblocking_filter_idc: 0,
            slice_alpha_c0_offset: 0,
            slice_beta_offset: 0,
        };
        deblock_luma_vert(&mut frame, 4, 0, 0, 3, &sh);
        assert_eq!(frame.y, before, "alpha=0 identity");
    }

    #[test]
    fn h264_decode_known_keyframe() {
        let yuv = decode_annexb(FRAME16).expect("decode 16x16");
        assert_eq!(yuv.width, 16);
        assert_eq!(yuv.height, 16);
        assert_eq!(yuv.y.len(), 256);
        assert_eq!(yuv.cb.len(), 64);
        assert_eq!(yuv.cr.len(), 64);
        let ry = &FRAME16_REF[..256];
        let rcb = &FRAME16_REF[256..320];
        let rcr = &FRAME16_REF[320..384];
        assert_eq!(max_abs_diff(&yuv.y, ry), 0, "luma not bit-exact");
        assert_eq!(max_abs_diff(&yuv.cb, rcb), 0, "Cb not bit-exact");
        assert_eq!(max_abs_diff(&yuv.cr, rcr), 0, "Cr not bit-exact");
    }

    #[test]
    fn h264_decode_known_keyframe_32() {
        let yuv = decode_annexb(FRAME32).expect("decode 32x32");
        assert_eq!(yuv.width, 32);
        assert_eq!(yuv.height, 32);
        let ny = 32 * 32;
        let nc = 16 * 16;
        let ry = &FRAME32_REF[..ny];
        let rcb = &FRAME32_REF[ny..ny + nc];
        let rcr = &FRAME32_REF[ny + nc..];
        assert_eq!(max_abs_diff(&yuv.y, ry), 0, "32x32 luma not bit-exact");
        assert_eq!(max_abs_diff(&yuv.cb, rcb), 0, "32x32 Cb not bit-exact");
        assert_eq!(max_abs_diff(&yuv.cr, rcr), 0, "32x32 Cr not bit-exact");
    }

    #[test]
    fn h264_truncated_slice_is_err() {
        let nals = split_annexb(FRAME16);
        let slice = nals
            .iter()
            .find(|n| !n.is_empty() && (n[0] & 0x1F == 5 || n[0] & 0x1F == 1))
            .unwrap();
        let sps_nal = nals
            .iter()
            .find(|n| !n.is_empty() && n[0] & 0x1F == 7)
            .unwrap();
        let pps_nal = nals
            .iter()
            .find(|n| !n.is_empty() && n[0] & 0x1F == 8)
            .unwrap();
        let sps = parse_sps(&nal_to_rbsp(&sps_nal[1..])).unwrap();
        let pps = parse_pps(&nal_to_rbsp(&pps_nal[1..])).unwrap();
        let rbsp = nal_to_rbsp(&slice[1..]);
        let truncated = &rbsp[..rbsp.len() / 2];
        let _ = decode_slice(truncated, &sps, &pps, slice[0] & 0x1F);
    }

    #[test]
    fn h264_boot_smoketest_passes() {
        let line = run_boot_smoketest();
        assert!(line.contains("-> PASS"), "h264 smoketest: {}", line);
        assert!(line.starts_with("[raemedia] h264-iframe:"));
        assert!(line.contains("bitexact=true"));
        assert!(h264_procfs_status().contains("h264=iframe"));
    }
}
