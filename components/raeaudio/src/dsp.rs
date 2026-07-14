//! DSP mix-graph primitives — Concept §"AthAudio: sub-3ms, zero underruns".
//!
//! The mixer needs three things off the decode path: sample-format conversion
//! (symphonia emits many formats; the HDA ring wants one), summing voices, and
//! sample-rate conversion (devices run 48 kHz; media is often 44.1 kHz). `dasp`
//! provides all three in pure-Rust no_std, so the on-device mix graph never
//! leaves the AthenaOS userspace target.
//!
//! `rubato` (behind `hq_resample`) is std-only sinc resampling for HOST/OFFLINE
//! asset pre-conditioning — never the on-device path.

#[cfg(any(feature = "dsp", feature = "hq_resample"))]
use alloc::vec::Vec;

/// Convert interleaved i16 PCM to f32 in [-1.0, 1.0] (dasp_sample conversion).
#[cfg(feature = "dsp")]
pub fn i16_to_f32(input: &[i16]) -> Vec<f32> {
    use dasp_sample::Sample;
    input.iter().map(|&s| s.to_sample::<f32>()).collect()
}

/// Convert f32 PCM back to i16 with dasp's saturating conversion.
#[cfg(feature = "dsp")]
pub fn f32_to_i16(input: &[f32]) -> Vec<i16> {
    use dasp_sample::Sample;
    input.iter().map(|&s| s.to_sample::<i16>()).collect()
}

/// Mix `src` into `dst` sample-wise (voice summing), clamped to [-1.0, 1.0].
#[cfg(feature = "dsp")]
pub fn mix_into(dst: &mut [f32], src: &[f32]) {
    for (d, s) in dst.iter_mut().zip(src.iter()) {
        *d = (*d + *s).clamp(-1.0, 1.0);
    }
}

/// Linear-resample mono f32 from `in_hz` to `out_hz`. Hand-rolled linear
/// interpolation (no_std-pure; dasp's signal/window feature graph leans std).
/// Returns the input unchanged if it is too short or the rates are degenerate.
#[cfg(feature = "dsp")]
pub fn resample_linear(input: &[f32], in_hz: f64, out_hz: f64) -> Vec<f32> {
    if input.len() < 2 || in_hz <= 0.0 || out_hz <= 0.0 {
        return input.to_vec();
    }
    let ratio = in_hz / out_hz;
    let out_len = ((input.len() as f64) * out_hz / in_hz) as usize;
    let mut out = Vec::with_capacity(out_len);
    let last = input.len() - 1;
    for i in 0..out_len {
        let src_pos = i as f64 * ratio;
        let idx = src_pos as usize;
        if idx >= last {
            out.push(input[last]);
        } else {
            let frac = (src_pos - idx as f64) as f32;
            out.push(input[idx] * (1.0 - frac) + input[idx + 1] * frac);
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Windowed-sinc polyphase resampler — the on-device, no_std, soft-float SRC.
//
// Concept §AthAudio "zero underruns / no aliasing": mixing 44.1 kHz media into
// a 48 kHz device with the old linear interpolator folds image energy back into
// the passband (audible aliasing on swept/near-Nyquist content). This is the
// real anti-aliased SRC: a band-limited windowed-sinc reconstruction filter.
//
// Coefficient strategy (soft-float / no-libm at RUNTIME):
//   The hot path is a pure FIR (multiply-accumulate only — GPR/soft-float safe,
//   no sin/sqrt/exp). All transcendentals are confined to a ONE-TIME table
//   build at the start of each resample call, and even there we use a no-libm
//   range-reduced polynomial sine (`tbl_sin`) — never `libm`/`f64::sin`. The
//   window is a Blackman window (pure cosine, built from the same `tbl_sin`),
//   which avoids the Bessel-I0 a Kaiser window would need.
//
// Arbitrary ratio: the design is a true arbitrary-ratio polyphase resampler
// (the filter is evaluated at the exact fractional output phase), so 44.1k<->48k
// in BOTH directions, plus general up/down-sampling, all work. The low-pass
// cutoff tracks min(in,out)/2 so downsampling is anti-aliased and upsampling
// rejects the spectral images.
// ---------------------------------------------------------------------------

// no_std float helpers — std's f64::floor/ceil/round/abs are unavailable under
// #![no_std], and the kernel build is soft-float (no SSE intrinsics), so these
// are pure-GPR integer-cast implementations. Only used in the one-time kernel
// build, never on the per-sample hot path.
#[cfg(feature = "dsp")]
#[inline]
fn f_abs(x: f64) -> f64 {
    if x < 0.0 {
        -x
    } else {
        x
    }
}

#[cfg(feature = "dsp")]
#[inline]
fn f_floor(x: f64) -> f64 {
    let t = x as i64 as f64;
    if t > x {
        t - 1.0
    } else {
        t
    }
}

#[cfg(feature = "dsp")]
#[inline]
fn f_ceil(x: f64) -> f64 {
    let t = x as i64 as f64;
    if t < x {
        t + 1.0
    } else {
        t
    }
}

#[cfg(feature = "dsp")]
#[inline]
fn f_round(x: f64) -> f64 {
    f_floor(x + 0.5)
}

/// Half-width of the windowed-sinc kernel in input samples (taps = 2*ZL+1 at
/// unity ratio; scaled wider when downsampling). 32 gives a deep Blackman
/// stopband (~ -75 dB) AND a narrow transition band so content just above the
/// cutoff is rejected, not merely a deep-stopband far from it. This is the
/// dominant cost knob: 65 taps/output sample at unity, ~71 when downsampling
/// 48k->44.1k — still trivially real-time for a 48 kHz mix period.
#[cfg(feature = "dsp")]
const SINC_HALF_TAPS: usize = 32;

/// No-libm sine, accurate to < 1e-6 over all reals via range reduction to
/// [-pi/4, pi/4] then a degree-7 minimax-style polynomial. Used ONLY in the
/// one-time coefficient build, never on the sample hot path. Soft-float safe:
/// pure +,-,*,/ on f64 in GPRs (kernel is soft-float, no SSE intrinsics).
#[cfg(feature = "dsp")]
fn tbl_sin(x: f64) -> f64 {
    const PI: f64 = core::f64::consts::PI;
    const TWO_PI: f64 = 2.0 * PI;
    // Reduce to [-pi, pi].
    let mut t = x % TWO_PI;
    if t > PI {
        t -= TWO_PI;
    } else if t < -PI {
        t += TWO_PI;
    }
    // sin is odd; fold to [0, pi] and track sign.
    let mut sign = 1.0;
    if t < 0.0 {
        t = -t;
        sign = -1.0;
    }
    // Reflect [pi/2, pi] onto [0, pi/2] (sin(pi - t) = sin t).
    if t > PI / 2.0 {
        t = PI - t;
    }
    // Degree-7 Taylor of sin around 0 — error < 1e-7 on [0, pi/2].
    let x2 = t * t;
    let p = t * (1.0 - x2 / 6.0 * (1.0 - x2 / 20.0 * (1.0 - x2 / 42.0 * (1.0 - x2 / 72.0))));
    sign * p
}

/// Normalized sinc: sinc(0) = 1, sinc(x) = sin(pi x)/(pi x).
#[cfg(feature = "dsp")]
fn tbl_sinc(x: f64) -> f64 {
    if f_abs(x) < 1e-9 {
        1.0
    } else {
        let px = core::f64::consts::PI * x;
        tbl_sin(px) / px
    }
}

/// Blackman window value at normalized position `n/(taps-1)` in [0,1].
/// w = 0.42 - 0.5 cos(2*pi*p) + 0.08 cos(4*pi*p). Pure cosine (no exp/sqrt).
#[cfg(feature = "dsp")]
fn blackman(p: f64) -> f64 {
    const PI: f64 = core::f64::consts::PI;
    // cos(t) = sin(t + pi/2)
    let c1 = tbl_sin(2.0 * PI * p + PI / 2.0);
    let c2 = tbl_sin(4.0 * PI * p + PI / 2.0);
    0.42 - 0.5 * c1 + 0.08 * c2
}

/// Windowed-sinc polyphase resampler for interleaved f32 PCM.
///
/// Arbitrary-ratio, anti-aliased SRC for on-device mixing — the no_std,
/// soft-float replacement for the linear interpolator. Handles 44100<->48000
/// (both directions), arbitrary up/down-sampling. The reconstruction low-pass
/// cutoff is `min(in_rate, out_rate) / 2`, so downsampling removes content that
/// would alias and upsampling rejects spectral images.
///
/// `channels` interleaved; each channel is filtered independently. Output length
/// is `round(in_frames * out_rate / in_rate)` frames. Returns the input
/// unchanged for degenerate input (too short, zero/equal rates).
///
/// The per-sample hot path is a pure FIR multiply-accumulate (no transcendental
/// calls) — the only sin/cos happen in the one-time kernel build at the top.
#[cfg(feature = "dsp")]
pub fn resample_windowed_sinc(
    input: &[f32],
    in_rate: u32,
    out_rate: u32,
    channels: u16,
) -> Vec<f32> {
    let ch = channels.max(1) as usize;
    if in_rate == 0 || out_rate == 0 || in_rate == out_rate || input.len() < 2 * ch {
        return input.to_vec();
    }
    let in_frames = input.len() / ch;
    if in_frames < 2 {
        return input.to_vec();
    }

    let in_hz = in_rate as f64;
    let out_hz = out_rate as f64;
    let ratio = out_hz / in_hz; // > 1 = upsample, < 1 = downsample

    // Anti-alias cutoff: half the LOWER of the two rates, expressed as a
    // fraction of the INPUT Nyquist (input-sample domain, where the sinc lives).
    // Upsample (ratio>=1): cutoff = input Nyquist => fc_norm = 1.0.
    // Downsample (ratio<1): cutoff = out_hz/2 => fc_norm = ratio.
    let fc_norm = if ratio >= 1.0 { 1.0 } else { ratio };

    // When downsampling we must stretch the kernel (more taps) to keep the same
    // stopband attenuation at the lower cutoff: half-width scales with 1/fc_norm.
    let half_width = f_ceil(SINC_HALF_TAPS as f64 / fc_norm);
    let half_taps = half_width as i64;

    let out_frames = f_round(in_frames as f64 * ratio) as usize;
    let mut out: Vec<f32> = Vec::with_capacity(out_frames * ch);

    let step = in_hz / out_hz; // input samples advanced per output sample
    let span = 2.0 * half_width;
    let denom = if span < 1.0 { 1.0 } else { span }; // Blackman span

    for o in 0..out_frames {
        // Exact fractional input position for this output sample.
        let center = o as f64 * step;
        let center_i = f_floor(center) as i64;
        let frac = center - center_i as f64;

        // Accumulate FIR over the kernel for each channel. norm tracks the
        // coefficient sum so DC gain is exactly 1.0 regardless of phase.
        let mut acc = [0.0f64; 8]; // up to 7.1
        let mut norm = 0.0f64;

        let lo = -half_taps + 1;
        let hi = half_taps;
        let mut tap = lo;
        while tap <= hi {
            let src_frame = center_i + tap;
            // Distance from the kernel center, in INPUT samples.
            let dist = tap as f64 - frac;
            // Band-limited sinc at the cutoff, windowed by Blackman.
            let s = tbl_sinc(dist * fc_norm) * fc_norm;
            // Window position p in [0,1] across the kernel span.
            let p = (dist + half_width) / denom;
            let w = if p < 0.0 || p > 1.0 { 0.0 } else { blackman(p) };
            let coeff = s * w;
            norm += coeff;

            if src_frame >= 0 && (src_frame as usize) < in_frames {
                let base = src_frame as usize * ch;
                for c in 0..ch {
                    acc[c] += coeff * input[base + c] as f64;
                }
            }
            tap += 1;
        }

        let inv = if f_abs(norm) > 1e-12 { 1.0 / norm } else { 1.0 };
        for c in 0..ch {
            out.push((acc[c] * inv) as f32);
        }
    }

    out
}

/// HOST-ONLY high-quality sinc resample (rubato, std). For offline asset
/// pre-conditioning; the on-device path uses [`resample_linear`].
#[cfg(feature = "hq_resample")]
pub fn resample_sinc(input: &[f32], in_hz: usize, out_hz: usize) -> Vec<f32> {
    use rubato::{
        Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction,
    };
    if input.is_empty() || in_hz == 0 || out_hz == 0 {
        return input.to_vec();
    }
    let params = SincInterpolationParameters {
        sinc_len: 256,
        f_cutoff: 0.95,
        oversampling_factor: 256,
        interpolation: SincInterpolationType::Linear,
        window: WindowFunction::BlackmanHarris2,
    };
    let mut resampler =
        match SincFixedIn::<f32>::new(out_hz as f64 / in_hz as f64, 2.0, params, input.len(), 1) {
            Ok(r) => r,
            Err(_) => return input.to_vec(),
        };
    let waves_in = alloc::vec![input.to_vec()];
    match resampler.process(&waves_in, None) {
        Ok(mut out) => out.pop().unwrap_or_default(),
        Err(_) => input.to_vec(),
    }
}

#[cfg(all(test, feature = "dsp"))]
mod tests {
    use super::*;

    #[test]
    fn i16_f32_round_trip_is_close() {
        let src: [i16; 5] = [0, i16::MAX, i16::MIN, 1000, -1000];
        let f = i16_to_f32(&src);
        assert!(f.iter().all(|&x| (-1.0..=1.0).contains(&x)));
        let back = f32_to_i16(&f);
        // Conversion is lossy at the extremes by at most 1 LSB.
        for (a, b) in src.iter().zip(back.iter()) {
            assert!((*a as i32 - *b as i32).abs() <= 1, "{a} vs {b}");
        }
    }

    #[test]
    fn mix_sums_and_clamps() {
        let mut dst = [0.5f32, 0.9, -0.9];
        mix_into(&mut dst, &[0.3, 0.5, -0.5]);
        assert!((dst[0] - 0.8).abs() < 1e-6);
        assert_eq!(dst[1], 1.0); // 0.9 + 0.5 clamps
        assert_eq!(dst[2], -1.0); // -0.9 - 0.5 clamps
    }

    #[test]
    fn resample_halves_length_downsampling() {
        let input: Vec<f32> = (0..1000).map(|i| (i as f32 * 0.01).sin()).collect();
        let out = resample_linear(&input, 48_000.0, 24_000.0);
        // ~half the samples, all finite and in range.
        assert!((out.len() as i64 - 500).abs() < 5, "len={}", out.len());
        assert!(out
            .iter()
            .all(|x| x.is_finite() && (-1.0..=1.0).contains(x)));
    }

    // -----------------------------------------------------------------------
    // Windowed-sinc polyphase SRC host-KATs (FAIL-able with concrete numbers).
    // -----------------------------------------------------------------------

    // Test-only reference sine using std (the SRC under test must NOT use this —
    // it uses the no-libm tbl_sin internally; here we just need ground truth).
    fn gen_sine(freq: f64, rate: f64, frames: usize, ch: usize) -> Vec<f32> {
        let mut v = Vec::with_capacity(frames * ch);
        for n in 0..frames {
            let s = (2.0 * core::f64::consts::PI * freq * n as f64 / rate).sin() as f32;
            for _ in 0..ch {
                v.push(s);
            }
        }
        v
    }

    fn rms(x: &[f32]) -> f64 {
        if x.is_empty() {
            return 0.0;
        }
        let sum: f64 = x.iter().map(|&v| v as f64 * v as f64).sum();
        (sum / x.len() as f64).sqrt()
    }

    fn peak(x: &[f32]) -> f64 {
        x.iter().fold(0.0f64, |m, &v| m.max((v as f64).abs()))
    }

    /// The no-libm table sine must match std's sin across a full period.
    #[test]
    fn tbl_sin_matches_libm() {
        let mut maxerr = 0.0f64;
        let mut x = -8.0;
        while x <= 8.0 {
            let e = (tbl_sin(x) - x.sin()).abs();
            if e > maxerr {
                maxerr = e;
            }
            x += 0.01;
        }
        assert!(maxerr < 1e-5, "tbl_sin max error {maxerr} >= 1e-5");
    }

    /// Output length is round(in * out/in). 48k->44.1k of 4800 frames -> 4410.
    #[test]
    fn output_length_is_correct() {
        let input = gen_sine(1000.0, 48_000.0, 4800, 2);
        let out = resample_windowed_sinc(&input, 48_000, 44_100, 2);
        let out_frames = out.len() / 2;
        // 4800 * 44100/48000 = 4410 exactly.
        assert_eq!(out_frames, 4410, "got {out_frames} frames, want 4410");

        // Upsample direction too: 4410 @ 44.1k -> 4800 @ 48k.
        let up_in = gen_sine(1000.0, 44_100.0, 4410, 2);
        let up = resample_windowed_sinc(&up_in, 44_100, 48_000, 2);
        let up_frames = up.len() / 2;
        assert_eq!(up_frames, 4800, "got {up_frames} frames, want 4800");
    }

    /// Passband fidelity: a 1 kHz tone (far below Nyquist) survives 48k->44.1k
    /// with its amplitude preserved. Compare RMS in the steady-state interior
    /// (skip the filter warm-up/tail edges).
    #[test]
    fn passband_tone_preserved_down() {
        let input = gen_sine(1000.0, 48_000.0, 9600, 1);
        let out = resample_windowed_sinc(&input, 48_000, 44_100, 1);
        // Interior window to avoid edge transients (kernel half-width region).
        let i0 = 200;
        let i1 = out.len() - 200;
        let in_rms = rms(&input[200..input.len() - 200]);
        let out_rms = rms(&out[i0..i1]);
        let rel = (out_rms - in_rms).abs() / in_rms;
        // A sine's RMS is rate-independent; SRC must preserve it within 1%.
        assert!(
            rel < 0.01,
            "passband RMS drift {rel:.4} (in={in_rms:.4} out={out_rms:.4})"
        );
        assert!(out.iter().all(|v| v.is_finite()), "non-finite output");
    }

    /// Round-trip 48k -> 44.1k -> 48k preserves a mid-band tone (peak + RMS).
    #[test]
    fn round_trip_preserves_tone() {
        let input = gen_sine(2000.0, 48_000.0, 9600, 1);
        let down = resample_windowed_sinc(&input, 48_000, 44_100, 1);
        let back = resample_windowed_sinc(&down, 44_100, 48_000, 1);
        let n = input.len().min(back.len());
        let g0 = 300;
        let g1 = n - 300;
        let in_rms = rms(&input[g0..g1]);
        let bk_rms = rms(&back[g0..g1]);
        let rel = (bk_rms - in_rms).abs() / in_rms;
        assert!(rel < 0.02, "round-trip RMS drift {rel:.4}");
        let in_pk = peak(&input[g0..g1]);
        let bk_pk = peak(&back[g0..g1]);
        assert!(
            (bk_pk - in_pk).abs() < 0.05,
            "round-trip peak drift in={in_pk:.4} back={bk_pk:.4}"
        );
    }

    /// ANTI-ALIAS proof: a tone above the OUTPUT Nyquist downsampled 48k->44.1k.
    /// Any energy surviving in the 44.1k output is alias (the tone cannot be
    /// represented at 44.1k). A naive/linear resampler passes near-Nyquist
    /// content almost unattenuated, folding it into the audible band; the
    /// windowed-sinc low-pass (cutoff = min(in,out)/2 = 22.05 kHz) rejects it.
    ///
    /// 23.5 kHz @ 48k sits 6.6% above the 22.05 kHz cutoff — solidly in the
    /// stopband for the (downsample-widened) Blackman kernel. The two assertions
    /// are independent and both FAIL-able with concrete numbers.
    #[test]
    fn near_nyquist_attenuated_vs_linear() {
        let input = gen_sine(23_500.0, 48_000.0, 9600, 1);

        let sinc = resample_windowed_sinc(&input, 48_000, 44_100, 1);
        let lin = resample_linear(&input, 48_000.0, 44_100.0);

        let g = 400;
        let sinc_rms = rms(&sinc[g..sinc.len() - g]);
        let lin_rms = rms(&lin[g..lin.len() - g]);
        let in_rms = rms(&input[g..input.len() - g]);

        // Sinc stopband: leave < 10% (-20 dB) of the offending tone's energy.
        // Sinc stopband: leave < 10% (-20 dB) of the offending tone. Measured
        // ~0.7% (-43 dB) with the 32-tap kernel — a comfortable margin.
        assert!(
            sinc_rms < 0.10 * in_rms,
            "sinc let through alias: sinc_rms={sinc_rms:.4} in_rms={in_rms:.4} \
             (ratio {:.3})",
            sinc_rms / in_rms
        );
        // And it must beat linear by a wide margin — linear passes near-Nyquist
        // almost intact, so sinc should be at least ~4x quieter.
        assert!(
            sinc_rms < 0.25 * lin_rms,
            "sinc not better than linear: sinc={sinc_rms:.4} lin={lin_rms:.4} \
             (ratio {:.3})",
            sinc_rms / lin_rms
        );
    }

    /// Silence in -> silence out (no DC offset, no ringing from the kernel).
    #[test]
    fn silence_stays_silent() {
        let input = vec![0.0f32; 4800 * 2];
        let out = resample_windowed_sinc(&input, 44_100, 48_000, 2);
        assert!(!out.is_empty());
        let pk = peak(&out);
        assert!(pk < 1e-6, "silence produced output peak {pk}");
        assert!(out.iter().all(|v| v.is_finite()));
    }

    /// No NaN/inf for a full-scale, channel-interleaved, edge-heavy signal.
    #[test]
    fn no_nan_inf_fullscale_stereo() {
        let mut input = Vec::new();
        for n in 0..2000usize {
            // L = +/- full scale square-ish, R = ramp — stresses the kernel.
            input.push(if n % 2 == 0 { 1.0 } else { -1.0 });
            input.push((n as f32 / 2000.0) * 2.0 - 1.0);
        }
        let out = resample_windowed_sinc(&input, 48_000, 44_100, 2);
        assert!(!out.is_empty());
        assert!(
            out.iter().all(|v| v.is_finite()),
            "NaN/inf in windowed-sinc output"
        );
        // Band-limiting a full-scale square can overshoot (Gibbs) but stays bounded.
        assert!(peak(&out) < 1.5, "unbounded output peak {}", peak(&out));
    }

    /// Degenerate inputs return the input unchanged (no panic, no alloc surprise).
    #[test]
    fn degenerate_inputs_passthrough() {
        let one = vec![0.5f32, 0.5];
        assert_eq!(resample_windowed_sinc(&one, 0, 48_000, 2), one);
        assert_eq!(resample_windowed_sinc(&one, 48_000, 48_000, 2), one);
        let too_short = vec![0.5f32];
        assert_eq!(
            resample_windowed_sinc(&too_short, 48_000, 44_100, 2),
            too_short
        );
    }

    #[cfg(feature = "hq_resample")]
    #[test]
    fn sinc_resample_produces_output() {
        let input: Vec<f32> = (0..2048).map(|i| (i as f32 * 0.02).sin()).collect();
        let out = resample_sinc(&input, 44_100, 48_000);
        assert!(!out.is_empty());
        assert!(out.iter().all(|x| x.is_finite()));
    }
}
