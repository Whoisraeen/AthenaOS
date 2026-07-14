//! HDR Output Pipeline — tone mapping operators, color space conversions,
//! and Perceptual Quantizer (PQ) transfer functions for HDR10/scRGB output.
//!
//! All math is `no_std` safe: no libm, uses inline float approximations.

#![allow(dead_code)]

// ═══════════════════════════════════════════════════════════════════════════
// HDR Configuration
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HdrMode {
    /// Standard dynamic range (SDR) — sRGB gamma, 0–1 output.
    Sdr,
    /// HDR10 — PQ transfer function, BT.2020 color space, 10-bit.
    Hdr10,
    /// scRGB — linear HDR with values > 1.0, 16-bit float per channel.
    ScRgb,
    /// Dolby Vision — dynamic metadata, 12-bit internal.
    DolbyVision,
}

#[derive(Debug, Clone, Copy)]
pub struct HdrConfig {
    pub mode: HdrMode,
    /// Peak luminance of the display in nits (cd/m²).
    pub max_luminance: f32,
    /// Minimum luminance the display can produce.
    pub min_luminance: f32,
    /// MaxCLL: maximum content light level (nits).
    pub max_content_light: u16,
    /// MaxFALL: maximum frame-average light level (nits).
    pub max_frame_average_light: u16,
    /// Reference white level in nits (typically 80 for SDR, 203 for HDR).
    pub reference_white: f32,
}

impl Default for HdrConfig {
    fn default() -> Self {
        Self {
            mode: HdrMode::Sdr,
            max_luminance: 100.0,
            min_luminance: 0.1,
            max_content_light: 100,
            max_frame_average_light: 100,
            reference_white: 80.0,
        }
    }
}

impl HdrConfig {
    pub fn hdr10_display(peak_nits: f32) -> Self {
        Self {
            mode: HdrMode::Hdr10,
            max_luminance: peak_nits,
            min_luminance: 0.005,
            max_content_light: peak_nits as u16,
            max_frame_average_light: (peak_nits * 0.5) as u16,
            reference_white: 203.0,
        }
    }

    pub fn scrgb_display(peak_nits: f32) -> Self {
        Self {
            mode: HdrMode::ScRgb,
            max_luminance: peak_nits,
            min_luminance: 0.0,
            max_content_light: peak_nits as u16,
            max_frame_average_light: (peak_nits * 0.5) as u16,
            reference_white: 80.0,
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// no_std float helpers
// ═══════════════════════════════════════════════════════════════════════════

#[inline]
fn f_abs(x: f32) -> f32 {
    if x < 0.0 {
        -x
    } else {
        x
    }
}

#[inline]
fn f_clamp(x: f32, lo: f32, hi: f32) -> f32 {
    if x < lo {
        lo
    } else if x > hi {
        hi
    } else {
        x
    }
}

#[inline]
fn f_max(a: f32, b: f32) -> f32 {
    if a > b {
        a
    } else {
        b
    }
}

#[inline]
fn f_min(a: f32, b: f32) -> f32 {
    if a < b {
        a
    } else {
        b
    }
}

fn f_pow(base: f32, exp: f32) -> f32 {
    if base <= 0.0 {
        return 0.0;
    }
    f_exp(exp * f_ln(base))
}

fn f_ln(x: f32) -> f32 {
    if x <= 0.0 {
        return -100.0;
    }
    let bits = x.to_bits();
    let exponent = ((bits >> 23) & 0xFF) as i32 - 127;
    let mantissa_bits = (bits & 0x007F_FFFF) | 0x3F80_0000;
    let m = f32::from_bits(mantissa_bits);
    const LN2: f32 = 0.6931472;
    let t = m - 1.0;
    let ln_m = t * (2.0 - t * (0.5 - t * (0.3333 - t * 0.25)));
    exponent as f32 * LN2 + ln_m
}

fn f_exp(x: f32) -> f32 {
    if x < -80.0 {
        return 0.0;
    }
    if x > 80.0 {
        return f32::MAX;
    }
    const LOG2E: f32 = 1.442695;
    let t = x * LOG2E;
    let i = f_floor(t) as i32;
    let f = t - i as f32;
    let pow2_f = 1.0 + f * (0.6931472 + f * (0.2402265 + f * 0.0555041));
    if i < -126 {
        return 0.0;
    }
    if i > 127 {
        return f32::MAX;
    }
    let pow2_i = f32::from_bits(((i + 127) as u32) << 23);
    pow2_i * pow2_f
}

#[inline]
fn f_floor(x: f32) -> f32 {
    let i = x as i32;
    if x < 0.0 && x != i as f32 {
        (i - 1) as f32
    } else {
        i as f32
    }
}

fn f_sqrt(x: f32) -> f32 {
    if x <= 0.0 {
        return 0.0;
    }
    let mut guess = x;
    for _ in 0..12 {
        guess = 0.5 * (guess + x / guess);
    }
    guess
}

// ═══════════════════════════════════════════════════════════════════════════
// Tone Mapping Operators
// ═══════════════════════════════════════════════════════════════════════════

/// ACES filmic tone mapping (approximation by Krzysztof Narkowicz).
/// Maps linear HDR color [0, ∞) to displayable [0, 1) range.
pub fn tone_map_aces(color: [f32; 3]) -> [f32; 3] {
    const A: f32 = 2.51;
    const B: f32 = 0.03;
    const C: f32 = 2.43;
    const D: f32 = 0.59;
    const E: f32 = 0.14;

    let map = |x: f32| -> f32 {
        let x = f_max(x, 0.0);
        let num = x * (A * x + B);
        let den = x * (C * x + D) + E;
        f_clamp(num / den, 0.0, 1.0)
    };

    [map(color[0]), map(color[1]), map(color[2])]
}

/// Reinhard tone mapping with configurable white point.
/// `max_white` is the luminance value that maps to display white.
pub fn tone_map_reinhard(color: [f32; 3], max_white: f32) -> [f32; 3] {
    let white_sq = max_white * max_white;

    let map = |x: f32| -> f32 {
        let x = f_max(x, 0.0);
        (x * (1.0 + x / white_sq)) / (1.0 + x)
    };

    [map(color[0]), map(color[1]), map(color[2])]
}

/// Extended Reinhard with luminance-based mapping (preserves color ratios).
pub fn tone_map_reinhard_luminance(color: [f32; 3], max_white: f32) -> [f32; 3] {
    let l_in = luminance_bt709(color);
    if l_in <= 0.0 {
        return [0.0; 3];
    }
    let white_sq = max_white * max_white;
    let l_out = (l_in * (1.0 + l_in / white_sq)) / (1.0 + l_in);
    let scale = l_out / l_in;
    [
        f_clamp(color[0] * scale, 0.0, 1.0),
        f_clamp(color[1] * scale, 0.0, 1.0),
        f_clamp(color[2] * scale, 0.0, 1.0),
    ]
}

/// Uncharted 2 tone mapping (John Hable's filmic curve).
pub fn tone_map_uncharted2(color: [f32; 3]) -> [f32; 3] {
    fn hable(x: f32) -> f32 {
        const A: f32 = 0.15;
        const B: f32 = 0.50;
        const C: f32 = 0.10;
        const D: f32 = 0.20;
        const E: f32 = 0.02;
        const F: f32 = 0.30;
        ((x * (A * x + C * B) + D * E) / (x * (A * x + B) + D * F)) - E / F
    }

    const EXPOSURE_BIAS: f32 = 2.0;
    const WHITE_POINT: f32 = 11.2;
    let white_scale = 1.0 / hable(WHITE_POINT);

    let map =
        |x: f32| -> f32 { f_clamp(hable(f_max(x, 0.0) * EXPOSURE_BIAS) * white_scale, 0.0, 1.0) };

    [map(color[0]), map(color[1]), map(color[2])]
}

// ═══════════════════════════════════════════════════════════════════════════
// Transfer Functions
// ═══════════════════════════════════════════════════════════════════════════

/// Perceptual Quantizer (PQ / SMPTE ST 2084) — encode linear to PQ.
/// Input: linear luminance normalized to 10000 nits (i.e., 1.0 = 10000 cd/m²).
pub fn linear_to_pq(linear: f32) -> f32 {
    const M1: f32 = 0.1593017578125;
    const M2: f32 = 78.84375;
    const C1: f32 = 0.8359375;
    const C2: f32 = 18.8515625;
    const C3: f32 = 18.6875;

    let l = f_max(linear, 0.0);
    let lm1 = f_pow(l, M1);
    let num = C1 + C2 * lm1;
    let den = 1.0 + C3 * lm1;
    f_pow(num / den, M2)
}

/// PQ to linear (SMPTE ST 2084 inverse EOTF).
/// Output: linear luminance normalized to 10000 nits.
pub fn pq_to_linear(pq: f32) -> f32 {
    const M1: f32 = 0.1593017578125;
    const M2: f32 = 78.84375;
    const C1: f32 = 0.8359375;
    const C2: f32 = 18.8515625;
    const C3: f32 = 18.6875;

    let p = f_max(pq, 0.0);
    let pm2_inv = f_pow(p, 1.0 / M2);
    let num = f_max(pm2_inv - C1, 0.0);
    let den = C2 - C3 * pm2_inv;
    if den <= 0.0 {
        return 0.0;
    }
    f_pow(num / den, 1.0 / M1)
}

/// sRGB gamma encode: linear → sRGB.
pub fn linear_to_srgb(linear: f32) -> f32 {
    let c = f_clamp(linear, 0.0, 1.0);
    if c <= 0.0031308 {
        c * 12.92
    } else {
        1.055 * f_pow(c, 1.0 / 2.4) - 0.055
    }
}

/// sRGB gamma decode: sRGB → linear.
pub fn srgb_to_linear(srgb: f32) -> f32 {
    let c = f_clamp(srgb, 0.0, 1.0);
    if c <= 0.04045 {
        c / 12.92
    } else {
        f_pow((c + 0.055) / 1.055, 2.4)
    }
}

/// Hybrid Log-Gamma (HLG) OETF — linear to HLG.
pub fn linear_to_hlg(linear: f32) -> f32 {
    const A: f32 = 0.17883277;
    const B: f32 = 0.28466892;
    const C: f32 = 0.55991073;

    let l = f_max(linear, 0.0);
    if l <= 1.0 / 12.0 {
        f_sqrt(3.0 * l)
    } else {
        A * f_ln(12.0 * l - B) + C
    }
}

/// HLG inverse OETF — HLG signal to linear.
pub fn hlg_to_linear(hlg: f32) -> f32 {
    const A: f32 = 0.17883277;
    const B: f32 = 0.28466892;
    const C: f32 = 0.55991073;

    let h = f_clamp(hlg, 0.0, 1.0);
    if h <= 0.5 {
        (h * h) / 3.0
    } else {
        (f_exp((h - C) / A) + B) / 12.0
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Color Space Utilities
// ═══════════════════════════════════════════════════════════════════════════

/// BT.709 luminance coefficients (same as sRGB).
pub fn luminance_bt709(color: [f32; 3]) -> f32 {
    0.2126 * color[0] + 0.7152 * color[1] + 0.0722 * color[2]
}

/// BT.2020 luminance coefficients (for HDR10).
pub fn luminance_bt2020(color: [f32; 3]) -> f32 {
    0.2627 * color[0] + 0.6780 * color[1] + 0.0593 * color[2]
}

/// Convert BT.709/sRGB gamut to BT.2020 (3x3 matrix multiply).
pub fn bt709_to_bt2020(color: [f32; 3]) -> [f32; 3] {
    [
        0.6274 * color[0] + 0.3293 * color[1] + 0.0433 * color[2],
        0.0691 * color[0] + 0.9195 * color[1] + 0.0114 * color[2],
        0.0164 * color[0] + 0.0880 * color[1] + 0.8956 * color[2],
    ]
}

/// Convert BT.2020 to BT.709/sRGB gamut.
pub fn bt2020_to_bt709(color: [f32; 3]) -> [f32; 3] {
    [
        1.6605 * color[0] - 0.5877 * color[1] - 0.0728 * color[2],
        -0.1246 * color[0] + 1.1330 * color[1] - 0.0084 * color[2],
        -0.0182 * color[0] - 0.1006 * color[1] + 1.1187 * color[2],
    ]
}

/// Apply exposure adjustment (EV stops).
pub fn apply_exposure(color: [f32; 3], ev: f32) -> [f32; 3] {
    let scale = f_pow(2.0, ev);
    [color[0] * scale, color[1] * scale, color[2] * scale]
}

// ═══════════════════════════════════════════════════════════════════════════
// HDR Pipeline — full output transform
// ═══════════════════════════════════════════════════════════════════════════

/// Full HDR output pipeline: takes linear-light input and produces the
/// final signal suitable for the display's transfer function.
pub fn hdr_output_transform(color: [f32; 3], config: &HdrConfig) -> [f32; 3] {
    match config.mode {
        HdrMode::Sdr => {
            let mapped = tone_map_aces(color);
            [
                linear_to_srgb(mapped[0]),
                linear_to_srgb(mapped[1]),
                linear_to_srgb(mapped[2]),
            ]
        }
        HdrMode::Hdr10 => {
            let adapted = adapt_to_display(color, config);
            let bt2020 = bt709_to_bt2020(adapted);
            let normalized = [
                bt2020[0] / 10000.0,
                bt2020[1] / 10000.0,
                bt2020[2] / 10000.0,
            ];
            [
                linear_to_pq(normalized[0]),
                linear_to_pq(normalized[1]),
                linear_to_pq(normalized[2]),
            ]
        }
        HdrMode::ScRgb => {
            let scale = config.reference_white / 80.0;
            [color[0] * scale, color[1] * scale, color[2] * scale]
        }
        HdrMode::DolbyVision => {
            let adapted = adapt_to_display(color, config);
            let bt2020 = bt709_to_bt2020(adapted);
            [
                linear_to_pq(bt2020[0] / 10000.0),
                linear_to_pq(bt2020[1] / 10000.0),
                linear_to_pq(bt2020[2] / 10000.0),
            ]
        }
    }
}

/// Adapt scene-referred linear values to display luminance range.
fn adapt_to_display(color: [f32; 3], config: &HdrConfig) -> [f32; 3] {
    let peak = config.max_luminance;
    let mapped = tone_map_reinhard(color, peak / config.reference_white);
    [mapped[0] * peak, mapped[1] * peak, mapped[2] * peak]
}

/// Convert SDR content for HDR display (inverse tone map / boost).
pub fn sdr_to_hdr(srgb_color: [f32; 3], config: &HdrConfig) -> [f32; 3] {
    let linear = [
        srgb_to_linear(srgb_color[0]),
        srgb_to_linear(srgb_color[1]),
        srgb_to_linear(srgb_color[2]),
    ];
    let boosted = [
        linear[0] * config.reference_white,
        linear[1] * config.reference_white,
        linear[2] * config.reference_white,
    ];
    boosted
}

/// Clamp a color to the valid range for a given HDR mode.
pub fn clamp_for_mode(color: [f32; 3], mode: HdrMode) -> [f32; 3] {
    match mode {
        HdrMode::Sdr => [
            f_clamp(color[0], 0.0, 1.0),
            f_clamp(color[1], 0.0, 1.0),
            f_clamp(color[2], 0.0, 1.0),
        ],
        HdrMode::Hdr10 | HdrMode::DolbyVision => [
            f_clamp(color[0], 0.0, 1.0),
            f_clamp(color[1], 0.0, 1.0),
            f_clamp(color[2], 0.0, 1.0),
        ],
        HdrMode::ScRgb => [
            f_clamp(color[0], -0.5, 7.5),
            f_clamp(color[1], -0.5, 7.5),
            f_clamp(color[2], -0.5, 7.5),
        ],
    }
}

/// Compute the maximum luminance ratio (display headroom) for HDR content.
pub fn display_headroom(config: &HdrConfig) -> f32 {
    if config.reference_white <= 0.0 {
        return 1.0;
    }
    config.max_luminance / config.reference_white
}

/// Check whether a pixel exceeds the max content light level.
pub fn exceeds_max_cll(color: [f32; 3], config: &HdrConfig) -> bool {
    let lum = luminance_bt709(color);
    lum > config.max_content_light as f32
}

// ═══════════════════════════════════════════════════════════════════════════
// HDR Metadata for display EDID / InfoFrame
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy)]
pub struct HdrStaticMetadata {
    pub max_display_mastering_luminance: u32,
    pub min_display_mastering_luminance: u32,
    pub max_content_light_level: u16,
    pub max_frame_average_light_level: u16,
    pub color_primaries: ColorPrimaries,
    pub transfer_function: TransferFunction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorPrimaries {
    Bt709,
    Bt2020,
    DciP3,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferFunction {
    Srgb,
    Pq,
    Hlg,
    Linear,
}

impl HdrStaticMetadata {
    pub fn from_config(config: &HdrConfig) -> Self {
        let (primaries, tf) = match config.mode {
            HdrMode::Sdr => (ColorPrimaries::Bt709, TransferFunction::Srgb),
            HdrMode::Hdr10 => (ColorPrimaries::Bt2020, TransferFunction::Pq),
            HdrMode::ScRgb => (ColorPrimaries::Bt709, TransferFunction::Linear),
            HdrMode::DolbyVision => (ColorPrimaries::Bt2020, TransferFunction::Pq),
        };

        Self {
            max_display_mastering_luminance: config.max_luminance as u32,
            min_display_mastering_luminance: (config.min_luminance * 10000.0) as u32,
            max_content_light_level: config.max_content_light,
            max_frame_average_light_level: config.max_frame_average_light,
            color_primaries: primaries,
            transfer_function: tf,
        }
    }
}
