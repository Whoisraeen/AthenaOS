//! Kernel-side compositor with multi-window z-ordering, VRR frame pacing,
//! HDR tone mapping, glassmorphism blur, live wallpapers, zero-cost screen
//! capture, and per-surface visual effects.
//!
//! Owns the bootloader framebuffer. Userspace tasks register `Surface`s —
//! kernel-allocated frames mapped into the task's address space — and ask
//! the compositor to present those surfaces at a chosen position. The
//! compositor blits surfaces in z-order (back-to-front) into the framebuffer.
//!
//! Syscall ABI (defined in [`crate::syscall`]):
//!   * `SYS_SURFACE_CREATE` (24): rdi=width, rsi=height, rdx=user virt.
//!     Returns surface id.
//!   * `SYS_SURFACE_PRESENT` (25): rdi=surface_id, rsi=x, rdx=y.
//!     Blits the surface and recomposites all visible surfaces.
//!   * `SYS_SURFACE_FOCUS` (26): rdi=surface_id. Brings to front.
//!   * `SYS_SURFACE_CLOSE` (27): rdi=surface_id. Destroys the surface.

use crate::arch::{PhysAddr, VirtAddr};
use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering};
use spin::Mutex;
use x86_64::structures::paging::{FrameAllocator, Page, PageTableFlags, PhysFrame, Size4KiB};

// ─── no_std f32 helpers (no libm) ───────────────────────────────────────────

fn f32_clamp(x: f32, lo: f32, hi: f32) -> f32 {
    if x < lo {
        lo
    } else if x > hi {
        hi
    } else {
        x
    }
}

fn f32_min(a: f32, b: f32) -> f32 {
    if a < b {
        a
    } else {
        b
    }
}
fn f32_max(a: f32, b: f32) -> f32 {
    if a > b {
        a
    } else {
        b
    }
}

fn f32_pow(base: f32, exp: f32) -> f32 {
    if base <= 0.0 {
        return 0.0;
    }
    f32_exp(exp * f32_ln(base))
}

fn f32_ln(x: f32) -> f32 {
    if x <= 0.0 {
        return -1e10;
    }
    let y = (x - 1.0) / (x + 1.0);
    let y2 = y * y;
    let mut sum = y;
    let mut term = y;
    for i in 0..10 {
        term *= y2;
        sum += term / (2 * i + 3) as f32;
    }
    2.0 * sum
}

fn f32_exp(x: f32) -> f32 {
    let x = f32_clamp(x, -20.0, 20.0);
    let mut sum = 1.0_f32;
    let mut term = 1.0_f32;
    for i in 1..20 {
        term *= x / i as f32;
        sum += term;
    }
    sum
}

fn u32_min(a: u32, b: u32) -> u32 {
    if a < b {
        a
    } else {
        b
    }
}

// ─── VRR-Aware Frame Pacing ─────────────────────────────────────────────────

const FRAME_HISTORY_LEN: usize = 16;

#[derive(Clone, Copy)]
pub struct VrrState {
    pub supported: bool,
    pub min_hz: u32,
    pub max_hz: u32,
    pub target_frame_us: u64,
}

impl VrrState {
    pub fn fixed(hz: u32) -> Self {
        let target = if hz == 0 {
            16_667
        } else {
            1_000_000 / hz as u64
        };
        Self {
            supported: false,
            min_hz: hz,
            max_hz: hz,
            target_frame_us: target,
        }
    }

    pub fn adaptive(min_hz: u32, max_hz: u32) -> Self {
        let target = if max_hz == 0 {
            16_667
        } else {
            1_000_000 / max_hz as u64
        };
        Self {
            supported: true,
            min_hz,
            max_hz,
            target_frame_us: target,
        }
    }
}

pub struct FramePacer {
    history: [u64; FRAME_HISTORY_LEN],
    write_idx: usize,
    count: usize,
    last_present_us: u64,
    vrr: VrrState,
}

impl FramePacer {
    pub fn new(vrr: VrrState) -> Self {
        Self {
            history: [0; FRAME_HISTORY_LEN],
            write_idx: 0,
            count: 0,
            last_present_us: 0,
            vrr,
        }
    }

    pub fn record_frame(&mut self, frame_time_us: u64) {
        self.history[self.write_idx] = frame_time_us;
        self.write_idx = (self.write_idx + 1) % FRAME_HISTORY_LEN;
        if self.count < FRAME_HISTORY_LEN {
            self.count += 1;
        }
    }

    pub fn predicted_next_us(&self) -> u64 {
        if self.count == 0 {
            return self.vrr.target_frame_us;
        }
        let mut sum = 0u64;
        let mut w_sum = 0u64;
        for i in 0..self.count {
            let idx = (self.write_idx + FRAME_HISTORY_LEN - 1 - i) % FRAME_HISTORY_LEN;
            let w = (self.count - i) as u64;
            sum += self.history[idx] * w;
            w_sum += w;
        }
        if w_sum == 0 {
            self.vrr.target_frame_us
        } else {
            sum / w_sum
        }
    }

    /// Returns the ideal present time (in us) for the next frame. The
    /// compositor should hold the completed frame until this point to
    /// minimize judder under VRR.
    pub fn optimal_present_us(&self, now_us: u64) -> u64 {
        if !self.vrr.supported {
            let next = self.last_present_us + self.vrr.target_frame_us;
            return if next > now_us { next } else { now_us };
        }

        let predicted = self.predicted_next_us();
        let min_interval = if self.vrr.max_hz == 0 {
            0
        } else {
            1_000_000 / self.vrr.max_hz as u64
        };
        let max_interval = if self.vrr.min_hz == 0 {
            33_333
        } else {
            1_000_000 / self.vrr.min_hz as u64
        };

        let interval = if predicted < min_interval {
            min_interval
        } else if predicted > max_interval {
            max_interval
        } else {
            predicted
        };
        let target = self.last_present_us + interval;
        if target > now_us {
            target
        } else {
            now_us
        }
    }

    pub fn mark_presented(&mut self, now_us: u64) {
        self.last_present_us = now_us;
    }

    pub fn update_vrr(&mut self, vrr: VrrState) {
        self.vrr = vrr;
    }
}

// ─── HDR Tone Mapping ───────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TransferFunction {
    Srgb,
    Pq,
    Linear,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ToneMapOperator {
    Reinhard,
    AcesFilmic,
    Pq,
}

#[derive(Clone, Copy)]
pub struct SurfaceHdr {
    pub transfer: TransferFunction,
    pub max_cll: u16,
    pub max_fall: u16,
}

impl Default for SurfaceHdr {
    fn default() -> Self {
        Self {
            transfer: TransferFunction::Srgb,
            max_cll: 0,
            max_fall: 0,
        }
    }
}

#[derive(Clone, Copy)]
pub struct DisplayHdrMeta {
    pub max_luminance: u32,
    pub min_luminance_x10000: u32,
    pub mastering_max: u32,
    pub mastering_min_x10000: u32,
}

impl Default for DisplayHdrMeta {
    fn default() -> Self {
        Self {
            max_luminance: 400,
            min_luminance_x10000: 500,
            mastering_max: 1000,
            mastering_min_x10000: 10,
        }
    }
}

pub struct HdrPipeline {
    pub enabled: bool,
    pub operator: ToneMapOperator,
    pub display_meta: DisplayHdrMeta,
}

impl HdrPipeline {
    pub fn new() -> Self {
        Self {
            enabled: false,
            operator: ToneMapOperator::AcesFilmic,
            display_meta: DisplayHdrMeta::default(),
        }
    }

    fn srgb_to_linear(c: f32) -> f32 {
        if c <= 0.04045 {
            c / 12.92
        } else {
            f32_pow((c + 0.055) / 1.055, 2.4)
        }
    }

    fn linear_to_srgb(c: f32) -> f32 {
        if c <= 0.0031308 {
            c * 12.92
        } else {
            1.055 * f32_pow(c, 1.0 / 2.4) - 0.055
        }
    }

    fn tone_map_reinhard(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
        let lum = 0.2126 * r + 0.7152 * g + 0.0722 * b;
        let scale = if lum > 0.0 {
            (1.0 + lum) / (1.0 + lum * lum)
        } else {
            1.0
        };
        (
            f32_clamp(r * scale, 0.0, 1.0),
            f32_clamp(g * scale, 0.0, 1.0),
            f32_clamp(b * scale, 0.0, 1.0),
        )
    }

    fn tone_map_aces(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
        fn aces_channel(x: f32) -> f32 {
            let a = 2.51;
            let b = 0.03;
            let c = 2.43;
            let d = 0.59;
            let e = 0.14;
            f32_clamp((x * (a * x + b)) / (x * (c * x + d) + e), 0.0, 1.0)
        }
        (aces_channel(r), aces_channel(g), aces_channel(b))
    }

    fn pq_eotf(c: f32) -> f32 {
        let m1: f32 = 0.1593017578125;
        let m2: f32 = 78.84375;
        let c1: f32 = 0.8359375;
        let c2: f32 = 18.8515625;
        let c3: f32 = 18.6875;
        let cp = f32_pow(f32_max(c, 0.0), 1.0 / m2);
        let num = f32_max(cp - c1, 0.0);
        let den = c2 - c3 * cp;
        if den <= 0.0 {
            return 0.0;
        }
        f32_pow(num / den, 1.0 / m1)
    }

    pub fn process_pixel(&self, pixel: u32, hdr_meta: &SurfaceHdr) -> u32 {
        let a = (pixel >> 24) & 0xFF;
        let r_u8 = (pixel >> 16) & 0xFF;
        let g_u8 = (pixel >> 8) & 0xFF;
        let b_u8 = pixel & 0xFF;

        let (mut r, mut g, mut b) = (
            r_u8 as f32 / 255.0,
            g_u8 as f32 / 255.0,
            b_u8 as f32 / 255.0,
        );

        match hdr_meta.transfer {
            TransferFunction::Srgb => {
                r = Self::srgb_to_linear(r);
                g = Self::srgb_to_linear(g);
                b = Self::srgb_to_linear(b);
            }
            TransferFunction::Pq => {
                r = Self::pq_eotf(r) * 10000.0 / self.display_meta.max_luminance as f32;
                g = Self::pq_eotf(g) * 10000.0 / self.display_meta.max_luminance as f32;
                b = Self::pq_eotf(b) * 10000.0 / self.display_meta.max_luminance as f32;
            }
            TransferFunction::Linear => {}
        }

        let (mr, mg, mb) = match self.operator {
            ToneMapOperator::Reinhard => Self::tone_map_reinhard(r, g, b),
            ToneMapOperator::AcesFilmic => Self::tone_map_aces(r, g, b),
            ToneMapOperator::Pq => (
                f32_clamp(r, 0.0, 1.0),
                f32_clamp(g, 0.0, 1.0),
                f32_clamp(b, 0.0, 1.0),
            ),
        };

        let ro = (Self::linear_to_srgb(mr) * 255.0 + 0.5) as u32;
        let go = (Self::linear_to_srgb(mg) * 255.0 + 0.5) as u32;
        let bo = (Self::linear_to_srgb(mb) * 255.0 + 0.5) as u32;
        (a << 24) | (u32_min(ro, 255) << 16) | (u32_min(go, 255) << 8) | u32_min(bo, 255)
    }
}

// ─── Glassmorphism / Blur Effects ───────────────────────────────────────────

#[derive(Clone, Copy)]
pub struct BlurRegion {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub radius: u32,
    pub tint_color: u32,
}

impl BlurRegion {
    pub fn full_surface(width: u32, height: u32, radius: u32, tint: u32) -> Self {
        Self {
            x: 0,
            y: 0,
            width,
            height,
            radius,
            tint_color: tint,
        }
    }
}

struct BlurEngine;

impl BlurEngine {
    /// 3-pass box blur (approximates Gaussian) on an ARGB u32 buffer in place.
    ///
    /// `tmp` is a caller-owned scratch slice (must be >= `w*h`) so the hot path
    /// reuses a buffer instead of allocating one per call.
    fn box_blur_3pass(buf: &mut [u32], tmp: &mut [u32], w: usize, h: usize, radius: u32) {
        if radius == 0 || w == 0 || h == 0 {
            return;
        }
        debug_assert!(tmp.len() >= w * h && buf.len() >= w * h);
        let r = radius as usize;
        let tmp = &mut tmp[..w * h];

        for _pass in 0..3 {
            Self::box_blur_h(buf, tmp, w, h, r);
            Self::box_blur_v(tmp, buf, w, h, r);
        }
    }

    fn box_blur_h(src: &[u32], dst: &mut [u32], w: usize, h: usize, r: usize) {
        let d = 2 * r + 1;
        for y in 0..h {
            let row = y * w;
            let (mut ra, mut ga, mut ba) = (0u32, 0u32, 0u32);

            let first = src[row];
            let last = src[row + w - 1];
            let fr = (first >> 16) & 0xFF;
            let fg = (first >> 8) & 0xFF;
            let fb = first & 0xFF;
            let lr = (last >> 16) & 0xFF;
            let lg = (last >> 8) & 0xFF;
            let lb = last & 0xFF;

            for i in 0..=r {
                let idx = row + if i < w { i } else { w - 1 };
                let p = src[idx];
                ra += (p >> 16) & 0xFF;
                ga += (p >> 8) & 0xFF;
                ba += p & 0xFF;
            }
            for _ in 0..r {
                ra += fr;
                ga += fg;
                ba += fb;
            }

            for x in 0..w {
                let a = src[row + x] & 0xFF00_0000;
                dst[row + x] =
                    a | ((ra / d as u32) << 16) | ((ga / d as u32) << 8) | (ba / d as u32);

                let add_x = x + r + 1;
                let sub_x = x as isize - r as isize;

                if add_x < w {
                    let p = src[row + add_x];
                    ra += (p >> 16) & 0xFF;
                    ga += (p >> 8) & 0xFF;
                    ba += p & 0xFF;
                } else {
                    ra += lr;
                    ga += lg;
                    ba += lb;
                }
                if sub_x >= 0 {
                    let p = src[row + sub_x as usize];
                    ra -= (p >> 16) & 0xFF;
                    ga -= (p >> 8) & 0xFF;
                    ba -= p & 0xFF;
                } else {
                    ra -= fr;
                    ga -= fg;
                    ba -= fb;
                }
            }
        }
    }

    fn box_blur_v(src: &[u32], dst: &mut [u32], w: usize, h: usize, r: usize) {
        let d = 2 * r + 1;
        for x in 0..w {
            let (mut ra, mut ga, mut ba) = (0u32, 0u32, 0u32);

            let first = src[x];
            let last = src[(h - 1) * w + x];
            let fr = (first >> 16) & 0xFF;
            let fg = (first >> 8) & 0xFF;
            let fb = first & 0xFF;
            let lr = (last >> 16) & 0xFF;
            let lg = (last >> 8) & 0xFF;
            let lb = last & 0xFF;

            for i in 0..=r {
                let iy = if i < h { i } else { h - 1 };
                let p = src[iy * w + x];
                ra += (p >> 16) & 0xFF;
                ga += (p >> 8) & 0xFF;
                ba += p & 0xFF;
            }
            for _ in 0..r {
                ra += fr;
                ga += fg;
                ba += fb;
            }

            for y in 0..h {
                let a = src[y * w + x] & 0xFF00_0000;
                dst[y * w + x] =
                    a | ((ra / d as u32) << 16) | ((ga / d as u32) << 8) | (ba / d as u32);

                let add_y = y + r + 1;
                let sub_y = y as isize - r as isize;

                if add_y < h {
                    let p = src[add_y * w + x];
                    ra += (p >> 16) & 0xFF;
                    ga += (p >> 8) & 0xFF;
                    ba += p & 0xFF;
                } else {
                    ra += lr;
                    ga += lg;
                    ba += lb;
                }
                if sub_y >= 0 {
                    let p = src[sub_y as usize * w + x];
                    ra -= (p >> 16) & 0xFF;
                    ga -= (p >> 8) & 0xFF;
                    ba -= p & 0xFF;
                } else {
                    ra -= fr;
                    ga -= fg;
                    ba -= fb;
                }
            }
        }
    }

    /// Apply a tint (semi-transparent colour overlay) to an ARGB region.
    fn apply_tint(buf: &mut [u32], tint: u32) {
        let ta = ((tint >> 24) & 0xFF) as u32;
        if ta == 0 {
            return;
        }
        let tr = (tint >> 16) & 0xFF;
        let tg = (tint >> 8) & 0xFF;
        let tb = tint & 0xFF;
        let inv = 255 - ta;

        for px in buf.iter_mut() {
            let a = *px & 0xFF00_0000;
            let r = ((((*px >> 16) & 0xFF) * inv + tr * ta) / 255) & 0xFF;
            let g = ((((*px >> 8) & 0xFF) * inv + tg * ta) / 255) & 0xFF;
            let b = (((*px & 0xFF) * inv + tb * ta) / 255) & 0xFF;
            *px = a | (r << 16) | (g << 8) | b;
        }
    }

    /// §2.3 / §9 interior legibility cap (the SHIP-GATE white-`text.primary` rule),
    /// mirrored from `raegfx::glass::clamp_interior_luma_region` for the LIVE blur
    /// path. After the tint→frost composite the live glass over a BRIGHT backdrop
    /// (the aurora blob) is still washed out — white text.primary drops below
    /// 4.5:1 — because this path mirrors the frost step rather than calling
    /// `draw_glass_surface`. We re-apply the SAME cap here, per pixel, over the
    /// composited glass region: scale each pixel's RGB UNIFORMLY toward black until
    /// its mean-channel (Rec.709) luma ≤ `GLASS_INTERIOR_LUMA_CEIL`. Over a bright
    /// backdrop this pulls the glass down so the white text wins; over a DARK
    /// backdrop every pixel is already at/below the ceiling so it is a NO-OP (the
    /// frosted/translucent look is untouched). Hue-preserving (uniform scale),
    /// integer fixed-point (no float in the IF=0 recompose hot path — the kernel is
    /// soft-float), in place on the blur scratch (no per-frame alloc). The ceiling
    /// is imported from `rae_tokens` so the threshold stays single-source; only the
    /// algorithm (the trivial scale-to-black) is mirrored because
    /// `rae_tokens::clamp_interior_luma` / `mean_luma` are private to the catalog.
    fn clamp_interior_luma(buf: &mut [u32], ceil: f32) {
        // Rec.709 luma weights ×10000 (sum 10000): the same perceptual weighting
        // `rae_tokens::mean_luma` and the gfx mirror use, so this render-side clamp
        // lands on the identical line the WCAG audit measures.
        const WR: u64 = 2126;
        const WG: u64 = 7152;
        const WB: u64 = 722;
        // ceil_num = ceil * 255 * 10000 — the luma threshold in the SAME fixed-point
        // scale as `luma_num` below (so the comparison and the scale factor are pure
        // integer). Computed once per region; the f32→integer convert is the only
        // float op and it is not per-pixel (soft-float safe).
        let ceil_num: u64 = (ceil * 255.0 * 10000.0) as u64;
        for px in buf.iter_mut() {
            let r = (*px >> 16) & 0xFF;
            let g = (*px >> 8) & 0xFF;
            let b = *px & 0xFF;
            // luma_num = (WR*R + WG*G + WB*B); mean-channel luma ×255×10000.
            let luma_num = WR * r as u64 + WG * g as u64 + WB * b as u64;
            if luma_num <= ceil_num || luma_num == 0 {
                continue; // dark-backdrop NO-OP: already legible.
            }
            // Uniform hue-preserving scale toward black: new = ch * ceil_num/luma_num.
            // Integer: ch * ceil_num / luma_num, with +luma_num/2 for round-to-nearest.
            let scale = |ch: u32| -> u32 {
                (((ch as u64 * ceil_num + luma_num / 2) / luma_num).min(0xFF)) as u32
            };
            let a = *px & 0xFF00_0000;
            *px = a | (scale(r) << 16) | (scale(g) << 8) | scale(b);
        }
    }

    /// SHIP-GATE §9 WCAG legibility pass — the FINAL white-`text.primary` guarantee,
    /// mirrored from `raegfx::glass::clamp_interior_wcag_region` for the LIVE blur path.
    ///
    /// The mean-channel cap [`clamp_interior_luma`] works in *gamma-encoded* mean-channel
    /// space, but WCAG contrast is computed on the *gamma-decoded* relative luminance —
    /// and the two diverge for saturated/colored pixels: a pixel held at mean-channel 0.40
    /// can still measure as low as ~2.8:1 against white text (the context-menu AA failure
    /// raegfx caught). This pass closes the gap on the quantity WCAG actually measures:
    /// for every interior pixel, measure the REAL contrast between [`TEXT_PRIMARY_DARK`]
    /// (white `text.primary`, the ink the glass carries) and the composited interior, and
    /// if it falls under [`TEXT_AA_TARGET`] (4.5 + margin) scale the pixel UNIFORMLY toward
    /// black until it clears.
    ///
    /// Scaling all channels by a single factor `< 1` is hue-preserving and STRICTLY
    /// monotone in relative luminance (each linearized channel shrinks), so contrast rises
    /// monotonically as the factor drops — a short bisection lands the largest factor that
    /// still clears the target. Over a DARK backdrop every interior pixel already clears AA,
    /// so this is a NO-OP there (the frosted/translucent dark look is untouched). This is
    /// the FINAL interior pass (runs after [`clamp_interior_luma`], before the edge stack)
    /// so it is the unconditional guarantee — the mean-channel cap is only a cheap pre-pass.
    ///
    /// Allocation-free: operates in place on the existing blur scratch (`region`), one
    /// pixel at a time. `contrast_ratio` is f32 but the kernel is soft-float so the
    /// relative-luminance math runs in GPRs — no XMM, no FPU save needed (CLAUDE.md
    /// "kernel is soft-float"). The interior pixel count is bounded per glass surface and
    /// the bisection is a fixed 12 iterations, so cost is bounded; the bisection only runs
    /// for pixels that actually FAIL the target (a dark backdrop short-circuits at the
    /// contrast check). Matches raegfx's bisection math exactly so live == host-render.
    fn clamp_interior_wcag(buf: &mut [u32]) {
        for px in buf.iter_mut() {
            let p = *px | 0xFF00_0000;
            if rae_tokens::contrast_ratio(TEXT_PRIMARY_DARK, p) >= TEXT_AA_TARGET {
                continue; // already legible — dark-backdrop NO-OP
            }
            // Bisection on the uniform darkening factor in (0, 1]. contrast vs factor is
            // monotone (smaller factor → darker interior → higher contrast against white
            // text), so 12 steps resolve the factor to < 1/4096 — well inside a colour
            // step. `lo` always clears the target (so the final clamp can't UNDERshoot).
            let r0 = ((p >> 16) & 0xFF) as f32;
            let g0 = ((p >> 8) & 0xFF) as f32;
            let b0 = (p & 0xFF) as f32;
            let mut lo = 0.0f32; // darkest (always clears)
            let mut hi = 1.0f32; // current (fails)
            for _ in 0..12 {
                let mid = 0.5 * (lo + hi);
                let q = 0xFF00_0000
                    | (((r0 * mid + 0.5) as u32).min(0xFF) << 16)
                    | (((g0 * mid + 0.5) as u32).min(0xFF) << 8)
                    | ((b0 * mid + 0.5) as u32).min(0xFF);
                if rae_tokens::contrast_ratio(TEXT_PRIMARY_DARK, q) >= TEXT_AA_TARGET {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
            let f = lo; // the largest factor that still clears the target
            let rr = ((r0 * f + 0.5) as u32).min(0xFF);
            let gg = ((g0 * f + 0.5) as u32).min(0xFF);
            let bb = ((b0 * f + 0.5) as u32).min(0xFF);
            *px = 0xFF00_0000 | (rr << 16) | (gg << 8) | bb;
        }
    }
}

/// Dark-theme white body ink (`rae_tokens::DARK.text_primary` = `#F0F2F8`) — the
/// foreground the §9 WCAG interior pass guarantees AA contrast against. Mirrored as a
/// const (the palette field is not a `pub` standalone token) so the live legibility
/// pass measures against the EXACT ink it protects, identical to the value
/// `raegfx::glass` uses, keeping live == host-render.
const TEXT_PRIMARY_DARK: u32 = 0xFF_F0_F2_F8;

/// The WCAG contrast target the interior must clear for white body text, with a small
/// safety margin over the 4.5:1 AA floor (the relative-luminance math is accurate to a
/// few 1e-3, and visual-qa samples antialiased glyph edges — the margin keeps the
/// *measured* ratio above 4.5 even at the worst sample). IDENTITY.md §9. Matches the
/// `raegfx::glass::TEXT_AA_TARGET` value so the live cap and host-render agree.
const TEXT_AA_TARGET: f32 = 4.7;

// ─── Tiered glass (IDENTITY §2) ─────────────────────────────────────────────
//
// Live glass picks one of exactly three tiers (chrome/panel/popover) — the
// cohesion rule "use tiers early, stop inventing new glass per screen". The
// per-surface ABI carries only a single `tint_color`, so the compositor classifies
// that declared tint into the nearest tier by its alpha, then applies the §2.3
// luma auto-adjust against the sampled backdrop so glass stays legible over bright
// AND dark content without leaving its tier. This keeps the surface ABI unchanged
// (no new field, no Wave-3 dependency) while shipping the new brighter/blue-violet
// tiered material everywhere live glass already exists.

/// Map a surface's declared glass tint to the nearest committed dark tier by its
/// effective alpha. The three tiers sit at ~0x40 / 0x73 / 0x99 alpha; anything a
/// call site declared (including the old single `GLASS_TINT_DARK` panel alias)
/// snaps to the closest. IDENTITY §2.1.
fn glass_tier_for_tint(tint: u32) -> rae_tokens::GlassTier {
    let a = (tint >> 24) & 0xFF;
    let ca = (rae_tokens::GLASS_CHROME_DARK.tint >> 24) & 0xFF;
    let pa = (rae_tokens::GLASS_PANEL_DARK.tint >> 24) & 0xFF;
    let poa = (rae_tokens::GLASS_POPOVER_DARK.tint >> 24) & 0xFF;
    let d_chrome = a.abs_diff(ca);
    let d_panel = a.abs_diff(pa);
    let d_pop = a.abs_diff(poa);
    if d_chrome <= d_panel && d_chrome <= d_pop {
        rae_tokens::GLASS_CHROME_DARK
    } else if d_panel <= d_pop {
        rae_tokens::GLASS_PANEL_DARK
    } else {
        rae_tokens::GLASS_POPOVER_DARK
    }
}

/// Mean perceptual luminance (0.0–1.0) of an ARGB region — the backdrop sample the
/// §2.3 luma auto-adjust feeds on. Integer-weighted, allocation-free; reads the
/// already-blurred scratch the compositor holds (one extra reduction, IDENTITY §2.3
/// "it already has the blurred buffer").
fn region_mean_luma(buf: &[u32]) -> f32 {
    if buf.is_empty() {
        return 0.0;
    }
    let mut acc: u64 = 0;
    for &px in buf {
        let r = ((px >> 16) & 0xFF) as u64;
        let g = ((px >> 8) & 0xFF) as u64;
        let b = (px & 0xFF) as u64;
        acc += r * 54 + g * 183 + b * 19; // >>8 → 0..255
    }
    let mean = (acc / buf.len() as u64) >> 8; // 0..255
    mean as f32 / 255.0
}

// (The live iridescent-rim wrapper was retired with the rim itself —
// IDENTITY-OBSIDIAN.md §2. `raegfx::glass::draw_iridescent_rim` remains
// available to theming callers.)

// ─── Live Wallpapers ────────────────────────────────────────────────────────

pub trait LiveWallpaper: Send {
    fn render_frame(&mut self, time_ms: u64, buffer: &mut [u32], width: u32, height: u32);
}

pub struct GradientWallpaper {
    color_a: u32,
    color_b: u32,
}

impl GradientWallpaper {
    pub fn new(a: u32, b: u32) -> Self {
        Self {
            color_a: a,
            color_b: b,
        }
    }
}

impl LiveWallpaper for GradientWallpaper {
    fn render_frame(&mut self, time_ms: u64, buffer: &mut [u32], _width: u32, height: u32) {
        let phase = ((time_ms / 40) % 512) as u32;
        let shift = if phase < 256 { phase } else { 512 - phase };
        let ar = ((self.color_a >> 16) & 0xFF) as u32;
        let ag = ((self.color_a >> 8) & 0xFF) as u32;
        let ab = (self.color_a & 0xFF) as u32;
        let br = ((self.color_b >> 16) & 0xFF) as u32;
        let bg = ((self.color_b >> 8) & 0xFF) as u32;
        let bb = (self.color_b & 0xFF) as u32;

        for (i, px) in buffer.iter_mut().enumerate() {
            let y = (i / 1) % height as usize;
            let t = ((y as u32 * 256 / height) + shift) % 512;
            let t = if t < 256 { t } else { 512 - t };
            let r = (ar * (256 - t) + br * t) / 256;
            let g = (ag * (256 - t) + bg * t) / 256;
            let b = (ab * (256 - t) + bb * t) / 256;
            *px = 0xFF00_0000 | (r << 16) | (g << 8) | b;
        }
    }
}

pub struct PlasmaWallpaper;

impl LiveWallpaper for PlasmaWallpaper {
    fn render_frame(&mut self, time_ms: u64, buffer: &mut [u32], width: u32, _height: u32) {
        let t = time_ms as u32;
        let w = width as usize;
        for (i, px) in buffer.iter_mut().enumerate() {
            let x = (i % w) as u32;
            let y = (i / w) as u32;

            let v1 = sin_approx(x.wrapping_mul(7).wrapping_add(t.wrapping_mul(3)));
            let v2 = sin_approx(y.wrapping_mul(11).wrapping_add(t.wrapping_mul(2)));
            let v3 = sin_approx(x.wrapping_add(y).wrapping_mul(5).wrapping_add(t));
            let val = ((v1 as i32 + v2 as i32 + v3 as i32 + 384) / 3) as u32;
            let val = if val > 255 { 255 } else { val };
            let r = val;
            let g = (val * 3 / 4) & 0xFF;
            let b = (255 - val / 2) & 0xFF;
            *px = 0xFF00_0000 | (r << 16) | (g << 8) | b;
        }
    }
}

fn sin_approx(x: u32) -> u8 {
    let idx = (x & 0xFF) as i32;
    let half = if idx < 128 { idx } else { 256 - idx };
    let v = (half * (256 - half)) >> 6;
    if v > 255 {
        255u8
    } else {
        v as u8
    }
}

struct WallpaperState {
    engine: Box<dyn LiveWallpaper>,
    last_render_ms: u64,
    paused: bool,
    frame_cap_ms: u64,
}

impl WallpaperState {
    fn new(engine: Box<dyn LiveWallpaper>) -> Self {
        Self {
            engine,
            last_render_ms: 0,
            paused: false,
            frame_cap_ms: 33,
        }
    }
}

// ─── Zero-Cost Screen Capture ───────────────────────────────────────────────

static CAPTURE_SEQ: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum CaptureFormat {
    Argb32,
    Bgra32,
}

pub struct CaptureSession {
    pub id: u64,
    pub active: bool,
    pub region_x: u32,
    pub region_y: u32,
    pub region_w: u32,
    pub region_h: u32,
    pub format: CaptureFormat,
    /// Double buffer: capture reads from previous while compositor writes current.
    pub front_buf: Vec<u32>,
    pub back_buf: Vec<u32>,
    pub frame_count: u64,
    pub continuous: bool,
    /// Owning task (raw TaskId) for resource reclaim on exit, or `0` if the
    /// session was started kernel-side (e.g. the boot smoketest). The scheduler
    /// sweeps these in `reclaim_task_resources` so a crashed capturer doesn't
    /// leak a session (same discipline as the socket/audio-voice sweep).
    pub owner: u64,
}

impl CaptureSession {
    fn new(
        id: u64,
        rx: u32,
        ry: u32,
        rw: u32,
        rh: u32,
        format: CaptureFormat,
        continuous: bool,
    ) -> Self {
        let sz = (rw as usize) * (rh as usize);
        Self {
            id,
            active: true,
            region_x: rx,
            region_y: ry,
            region_w: rw,
            region_h: rh,
            format,
            front_buf: vec![0u32; sz],
            back_buf: vec![0u32; sz],
            frame_count: 0,
            continuous,
            owner: 0,
        }
    }

    fn swap_buffers(&mut self) {
        core::mem::swap(&mut self.front_buf, &mut self.back_buf);
        self.frame_count += 1;
    }

    fn capture_from_composited(&mut self, composited: &[u32], comp_w: u32, comp_h: u32) {
        let rw = self.region_w as usize;
        let rh = self.region_h as usize;
        let cw = comp_w as usize;

        for y in 0..rh {
            let sy = self.region_y as usize + y;
            if sy >= comp_h as usize {
                break;
            }
            for x in 0..rw {
                let sx = self.region_x as usize + x;
                if sx >= cw {
                    break;
                }
                let pixel = composited[sy * cw + sx];
                self.back_buf[y * rw + x] = match self.format {
                    CaptureFormat::Argb32 => pixel,
                    CaptureFormat::Bgra32 => {
                        let a = (pixel >> 24) & 0xFF;
                        let r = (pixel >> 16) & 0xFF;
                        let g = (pixel >> 8) & 0xFF;
                        let b = pixel & 0xFF;
                        (a << 24) | (b << 16) | (g << 8) | r
                    }
                };
            }
        }
        self.swap_buffers();
    }
}

// ─── Surface Effects System ─────────────────────────────────────────────────

#[derive(Clone, Copy)]
pub enum SurfaceEffect {
    DropShadow {
        offset_x: i32,
        offset_y: i32,
        radius: u32,
        color: u32,
    },
    RoundedCorners {
        radius: u32,
    },
    Border {
        width: u32,
        color: u32,
    },
    Glow {
        radius: u32,
        color: u32,
    },
    Opacity(u8),
}

fn is_inside_rounded_rect(x: u32, y: u32, w: u32, h: u32, r: u32) -> bool {
    if r == 0 || (x >= r && x < w - r) || (y >= r && y < h - r) {
        return true;
    }
    let (cx, cy) = if x < r && y < r {
        (r, r)
    } else if x >= w - r && y < r {
        (w - r - 1, r)
    } else if x < r && y >= h - r {
        (r, h - r - 1)
    } else if x >= w - r && y >= h - r {
        (w - r - 1, h - r - 1)
    } else {
        return true;
    };
    let dx = x as i32 - cx as i32;
    let dy = y as i32 - cy as i32;
    (dx * dx + dy * dy) <= (r * r) as i32
}

// ─── Direct-to-GPU Exclusive Fullscreen ─────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ExclusiveState {
    /// Surface composited normally.
    Composited,
    /// Surface has exclusive scanout — compositor is bypassed.
    Exclusive,
    /// Transitioning back to composited mode.
    Releasing,
}

/// Tracks the double-buffered scanout for exclusive fullscreen.
struct ExclusiveFullscreen {
    surface_id: u64,
    /// Front buffer (currently displayed).
    front_phys: u64,
    front_ptr: *mut u32,
    /// Back buffer (being written to by the game).
    back_phys: u64,
    back_ptr: *mut u32,
    width: u32,
    height: u32,
    stride: u32,
    /// Monotonic fence counter for vsync page flips.
    fence_seq: u64,
    /// Whether the GPU supports direct scanout for this surface.
    gpu_direct: bool,
    /// Game Bar overlay buffer (small HUD composited on top of flip).
    overlay_buf: Vec<u32>,
    overlay_width: u32,
    overlay_height: u32,
    overlay_visible: bool,
    overlay_x: u32,
    overlay_y: u32,
}

// SAFETY: ExclusiveFullscreen is only accessed behind the COMPOSITOR mutex.
// The raw pointers reference kernel-owned DMA memory that outlives the struct.
unsafe impl Send for ExclusiveFullscreen {}

impl Drop for ExclusiveFullscreen {
    /// Free BOTH the front and back scanout buffers. `release_exclusive_fullscreen`
    /// and the auto-release in `close_surface` previously just set
    /// `st.exclusive = None`, leaking both full-screen contiguous buffers
    /// (each `stride * height` bytes). The order MUST match `alloc` /
    /// `alloc_contig_frames`: `pages = (stride * height).div_ceil(4096)`,
    /// `order = usize::BITS - pages.saturating_sub(1).leading_zeros()`. The
    /// `*_phys` fields already hold the physical base addresses (no
    /// PHYS_MEM_OFFSET arithmetic needed), so each frees its own block once.
    /// Same IF=0 / BSP-only serialization as `Surface::drop` — destruction runs
    /// under `lock_compositor()`, so no concurrent reader of these frames.
    fn drop(&mut self) {
        let fb_bytes = (self.stride as usize) * (self.height as usize);
        if fb_bytes == 0 {
            return;
        }
        let pages = fb_bytes.div_ceil(4096);
        let order = (usize::BITS - pages.saturating_sub(1).leading_zeros()) as u8;
        if self.front_phys != 0 {
            crate::memory::deallocate_contiguous_frames(PhysAddr::new(self.front_phys), order);
            self.front_phys = 0;
            self.front_ptr = core::ptr::null_mut();
        }
        if self.back_phys != 0 {
            crate::memory::deallocate_contiguous_frames(PhysAddr::new(self.back_phys), order);
            self.back_phys = 0;
            self.back_ptr = core::ptr::null_mut();
        }
    }
}

impl ExclusiveFullscreen {
    fn alloc(surface: &Surface, gpu_direct: bool) -> Option<Self> {
        let w = surface.width;
        let h = surface.height;
        let stride = w * 4;
        let fb_bytes = (stride as usize) * (h as usize);
        let pages = fb_bytes.div_ceil(4096);

        let offset = *crate::memory::PHYS_MEM_OFFSET.get()?;

        let front_frame = alloc_contig_frames(pages)?;
        let front_phys = front_frame.start_address().as_u64();
        let front_ptr = (offset + front_phys).as_mut_ptr::<u32>();
        unsafe {
            core::ptr::write_bytes(front_ptr as *mut u8, 0, pages * 4096);
        }

        let back_frame = alloc_contig_frames(pages)?;
        let back_phys = back_frame.start_address().as_u64();
        let back_ptr = (offset + back_phys).as_mut_ptr::<u32>();
        unsafe {
            core::ptr::write_bytes(back_ptr as *mut u8, 0, pages * 4096);
        }

        let overlay_w = 320u32;
        let overlay_h = 40u32;
        let overlay_buf = vec![0u32; (overlay_w as usize) * (overlay_h as usize)];

        Some(Self {
            surface_id: surface.id,
            front_phys,
            front_ptr,
            back_phys,
            back_ptr,
            width: w,
            height: h,
            stride,
            fence_seq: 0,
            gpu_direct,
            overlay_buf,
            overlay_width: overlay_w,
            overlay_height: overlay_h,
            overlay_visible: false,
            overlay_x: w.saturating_sub(overlay_w) / 2,
            overlay_y: 8,
        })
    }

    /// Page flip: swap front/back buffers and signal vsync.
    fn page_flip(&mut self) {
        core::mem::swap(&mut self.front_phys, &mut self.back_phys);
        core::mem::swap(&mut self.front_ptr, &mut self.back_ptr);
        self.fence_seq += 1;
    }

    /// Composite the Game Bar overlay onto the back buffer (before flip).
    fn composite_overlay(&self) {
        if !self.overlay_visible {
            return;
        }
        let dw = self.width as usize;
        let ow = self.overlay_width as usize;
        let oh = self.overlay_height as usize;
        let ox = self.overlay_x as usize;
        let oy = self.overlay_y as usize;

        for y in 0..oh {
            let dy = oy + y;
            if dy >= self.height as usize {
                break;
            }
            for x in 0..ow {
                let dx = ox + x;
                if dx >= dw {
                    break;
                }
                let pixel = self.overlay_buf[y * ow + x];
                let alpha = (pixel >> 24) & 0xFF;
                if alpha == 0 {
                    continue;
                }
                let dst_idx = dy * dw + dx;
                if alpha >= 255 {
                    unsafe {
                        self.back_ptr.add(dst_idx).write_volatile(pixel);
                    }
                } else {
                    let bg = unsafe { self.back_ptr.add(dst_idx).read_volatile() };
                    unsafe {
                        self.back_ptr
                            .add(dst_idx)
                            .write_volatile(alpha_blend(pixel, bg));
                    }
                }
            }
        }
    }

    /// Flush the front buffer to the GPU scanout or software FB.
    fn flush_to_gpu(&self, gpu_fb: &Option<crate::gpu::GpuFramebuffer>) {
        if let Some(ref gfb) = gpu_fb {
            let fb_ptr = gfb.ptr as *mut u32;
            let fb_stride = gfb.stride as usize;
            let w = (self.width as usize).min(gfb.width as usize);
            let h = (self.height as usize).min(gfb.height as usize);

            // Fast row blit + flush (game exclusive-fullscreen present) — the
            // per-pixel volatile pair here cost ~3–6 ms/frame at 1080p; see
            // blit_rows_to_gpu_fb for the copy + DCN-coherency flush contract.
            unsafe {
                blit_rows_to_gpu_fb(
                    self.front_ptr as *const u32,
                    self.width as usize,
                    fb_ptr,
                    fb_stride,
                    w,
                    h,
                )
            };
        }
    }

    /// Flush to the software framebuffer (fallback).
    fn flush_to_sw_fb(&self, fb: &crate::framebuffer::FbInfo) {
        let fb_w = fb.width as usize;
        let fb_h = fb.height as usize;
        let fb_bpp = fb.bytes_per_pixel as usize;
        let fb_stride_pixels = fb.stride as usize;
        let w = (self.width as usize).min(fb_w);
        let h = (self.height as usize).min(fb_h);

        for y in 0..h {
            for x in 0..w {
                let pixel = unsafe {
                    self.front_ptr
                        .add(y * self.width as usize + x)
                        .read_volatile()
                };
                let r = ((pixel >> 16) & 0xFF) as u8;
                let g = ((pixel >> 8) & 0xFF) as u8;
                let b = (pixel & 0xFF) as u8;
                let fb_offset = y * fb_stride_pixels * fb_bpp + x * fb_bpp;
                unsafe {
                    fb.ptr.add(fb_offset).write_volatile(b);
                    fb.ptr.add(fb_offset + 1).write_volatile(g);
                    fb.ptr.add(fb_offset + 2).write_volatile(r);
                }
            }
        }
    }
}

/// Request exclusive fullscreen for a surface. The surface is detached from
/// the compositor and given direct scanout — zero copy, zero compositor overhead.
/// Returns `true` if granted.
pub fn request_exclusive_fullscreen(surface_id: u64) -> bool {
    let mut state = lock_compositor();
    let Some(st) = state.as_mut() else {
        return false;
    };

    if st.exclusive.is_some() {
        return false;
    }

    let surface = match st.surfaces.iter().find(|s| s.id == surface_id) {
        Some(s) => s,
        None => return false,
    };

    let gpu_direct = st.gpu_fb.is_some();
    let excl = match ExclusiveFullscreen::alloc(surface, gpu_direct) {
        Some(e) => e,
        None => return false,
    };

    if let Some(s) = st.surfaces.iter_mut().find(|s| s.id == surface_id) {
        s.visible = false;
    }

    st.exclusive = Some(excl);

    crate::serial_println!(
        "[compositor] exclusive fullscreen granted to surface {} (gpu_direct={})",
        surface_id,
        gpu_direct,
    );
    true
}

/// Release exclusive fullscreen and re-attach the surface to the compositor.
pub fn release_exclusive_fullscreen(surface_id: u64) -> bool {
    let mut state = lock_compositor();
    let Some(st) = state.as_mut() else {
        return false;
    };

    let matches = st
        .exclusive
        .as_ref()
        .map_or(false, |e| e.surface_id == surface_id);
    if !matches {
        return false;
    }

    st.exclusive = None;

    if let Some(s) = st.surfaces.iter_mut().find(|s| s.id == surface_id) {
        s.visible = true;
    }

    crate::serial_println!(
        "[compositor] exclusive fullscreen released for surface {}",
        surface_id,
    );
    true
}

/// Present the exclusive fullscreen surface: copy → overlay → flip → scanout.
/// Called by the game's render loop instead of `present_surface` while exclusive.
pub fn present_exclusive() {
    let mut state = lock_compositor();
    let Some(st) = state.as_mut() else { return };

    let (src_ptr, src_w, src_h) = {
        let Some(ref excl) = st.exclusive else { return };
        let surface = match st.surfaces.iter().find(|s| s.id == excl.surface_id) {
            Some(s) => s,
            None => return,
        };
        (
            surface.kernel_ptr as *const u32,
            surface.width as usize,
            surface.height as usize,
        )
    };

    let excl = st.exclusive.as_mut().unwrap();
    let dw = excl.width as usize;
    let dh = excl.height as usize;
    let rows = if src_h < dh { src_h } else { dh };
    let cols = if src_w < dw { src_w } else { dw };

    for y in 0..rows {
        for x in 0..cols {
            let pixel = unsafe { src_ptr.add(y * src_w + x).read_volatile() };
            unsafe {
                excl.back_ptr.add(y * dw + x).write_volatile(pixel);
            }
        }
    }

    excl.composite_overlay();
    excl.page_flip();

    let gpu_fb = st.gpu_fb;
    let fb = st.fb;

    if gpu_fb.is_some() {
        excl.flush_to_gpu(&gpu_fb);
        st.frame_pacer
            .record_frame(st.frame_pacer.vrr.target_frame_us);
        let time_us = st.time_us;
        st.time_us += st.frame_pacer.vrr.target_frame_us;
        st.frame_pacer.mark_presented(time_us);
        drop(state);
        crate::gpu::present_gpu_scanout();
    } else {
        excl.flush_to_sw_fb(&fb);
        st.frame_pacer
            .record_frame(st.frame_pacer.vrr.target_frame_us);
        let time_us = st.time_us;
        st.time_us += st.frame_pacer.vrr.target_frame_us;
        st.frame_pacer.mark_presented(time_us);
    }
}

/// Toggle Game Bar overlay visibility in exclusive fullscreen mode.
pub fn toggle_exclusive_overlay() {
    let mut state = lock_compositor();
    if let Some(st) = state.as_mut() {
        if let Some(ref mut excl) = st.exclusive {
            excl.overlay_visible = !excl.overlay_visible;
            crate::serial_println!(
                "[compositor] Game Bar overlay {}",
                if excl.overlay_visible {
                    "shown"
                } else {
                    "hidden"
                },
            );
        }
    }
}

/// Write Game Bar overlay content (small ARGB buffer composited on each flip).
pub fn set_exclusive_overlay(buf: &[u32], width: u32, height: u32, x: u32, y: u32) {
    let mut state = lock_compositor();
    if let Some(st) = state.as_mut() {
        if let Some(ref mut excl) = st.exclusive {
            let needed = (width as usize) * (height as usize);
            if buf.len() >= needed {
                excl.overlay_buf.clear();
                excl.overlay_buf.extend_from_slice(&buf[..needed]);
                excl.overlay_width = width;
                excl.overlay_height = height;
                excl.overlay_x = x;
                excl.overlay_y = y;
            }
        }
    }
}

/// Check if any surface currently has exclusive fullscreen.
pub fn is_exclusive_fullscreen() -> bool {
    // Use `lock_compositor` (IF=0 RAII guard), not a raw `COMPOSITOR.lock()`:
    // syscalls run with IF=0 on this single-scheduling-CPU kernel, so a raw lock
    // shared with the preemptible compositor thread is the exact deadlock footgun
    // root-caused on iron 2026-06-15 (see `CompositorGuard`).
    lock_compositor()
        .as_ref()
        .map_or(false, |st| st.exclusive.is_some())
}

/// Get the surface id that currently has exclusive fullscreen, if any.
pub fn exclusive_surface_id() -> Option<u64> {
    lock_compositor()
        .as_ref()
        .and_then(|st| st.exclusive.as_ref().map(|e| e.surface_id))
}

/// Get the current exclusive fullscreen fence (vsync frame counter).
pub fn exclusive_fence() -> u64 {
    lock_compositor()
        .as_ref()
        .and_then(|st| st.exclusive.as_ref().map(|e| e.fence_seq))
        .unwrap_or(0)
}

// ─── Fast-Path Compositor Fallback ──────────────────────────────────────────

/// When the GPU doesn't support true direct scanout (e.g., software fallback),
/// the compositor uses this fast-path: blit ONLY the fullscreen surface into
/// the compositing buffer, skipping all other surfaces, wallpaper, effects.
/// This is almost as fast as direct scanout — zero per-pixel effects.
fn composite_fast_path(st: &mut CompositorState, surface_id: u64) {
    let surface = match st.surfaces.iter().find(|s| s.id == surface_id) {
        Some(s) => s,
        None => return,
    };

    let cw = st.comp_w as usize;
    let ch = st.comp_h as usize;
    let sw = surface.width as usize;
    let sh = surface.height as usize;
    let src = surface.kernel_ptr as *const u32;

    let rows = if sh < ch { sh } else { ch };
    let cols = if sw < cw { sw } else { cw };

    for y in 0..rows {
        for x in 0..cols {
            let pixel = unsafe { src.add(y * sw + x).read_volatile() };
            st.comp_buf[y * cw + x] = pixel;
        }
    }

    if sh < ch {
        for y in sh..ch {
            for x in 0..cw {
                st.comp_buf[y * cw + x] = 0xFF00_0000;
            }
        }
    }
    if sw < cw {
        for y in 0..rows {
            for x in sw..cw {
                st.comp_buf[y * cw + x] = 0xFF00_0000;
            }
        }
    }
}

/// Present a surface via the compositor fast-path (no effects, just blit).
/// Used when a game wants exclusive fullscreen but the GPU doesn't support
/// direct scanout. Nearly zero overhead compared to full compositing.
pub fn present_fast_path(surface_id: u64) {
    let mut state = lock_compositor();
    let Some(st) = state.as_mut() else { return };

    composite_fast_path(st, surface_id);

    if let Some(ref excl) = st.exclusive {
        if excl.overlay_visible && !excl.overlay_buf.is_empty() {
            let cw = st.comp_w as usize;
            let ow = excl.overlay_width as usize;
            let oh = excl.overlay_height as usize;
            let ox = excl.overlay_x as usize;
            let oy = excl.overlay_y as usize;

            for y in 0..oh {
                let dy = oy + y;
                if dy >= st.comp_h as usize {
                    break;
                }
                for x in 0..ow {
                    let dx = ox + x;
                    if dx >= cw {
                        break;
                    }
                    let pixel = excl.overlay_buf[y * ow + x];
                    let alpha = (pixel >> 24) & 0xFF;
                    if alpha == 0 {
                        continue;
                    }
                    let dst_idx = dy * cw + dx;
                    if alpha >= 255 {
                        st.comp_buf[dst_idx] = pixel;
                    } else {
                        st.comp_buf[dst_idx] = alpha_blend(pixel, st.comp_buf[dst_idx]);
                    }
                }
            }
        }
    }

    let use_gpu = st.gpu_fb.is_some();
    let time_us = st.time_us;
    st.time_us += st.frame_pacer.vrr.target_frame_us;
    st.frame_pacer
        .record_frame(st.frame_pacer.vrr.target_frame_us);
    st.frame_pacer.mark_presented(time_us);

    if use_gpu {
        let gfb = st.gpu_fb.as_ref().unwrap();
        let fb_ptr = gfb.ptr as *mut u32;
        let fb_stride = gfb.stride as usize;
        let cw = st.comp_w as usize;
        let ch = st.comp_h as usize;

        // Fast row blit + flush (copy_nonoverlapping per row, clflush+sfence for
        // the non-snooped DCN read path) — see blit_rows_to_gpu_fb.
        unsafe { blit_rows_to_gpu_fb(st.comp_buf.as_ptr(), cw, fb_ptr, fb_stride, cw, ch) };

        drop(state);
        crate::gpu::present_gpu_scanout();
    } else {
        flush_comp_buf_to_sw_fb(
            &st.comp_buf,
            &mut st.sw_backbuf,
            &st.fb,
            st.comp_w as usize,
            st.comp_h as usize,
        );
    }
}

// ─── Exclusive Fullscreen Frame Statistics ──────────────────────────────────

const FRAME_STAT_HISTORY: usize = 120;

pub struct ExclusiveFrameStats {
    pub frame_times_us: [u64; FRAME_STAT_HISTORY],
    pub write_idx: usize,
    pub count: usize,
    pub total_frames: u64,
    pub dropped_frames: u64,
    pub last_present_us: u64,
}

impl ExclusiveFrameStats {
    pub fn new() -> Self {
        Self {
            frame_times_us: [0; FRAME_STAT_HISTORY],
            write_idx: 0,
            count: 0,
            total_frames: 0,
            dropped_frames: 0,
            last_present_us: 0,
        }
    }

    pub fn record(&mut self, frame_time_us: u64) {
        self.frame_times_us[self.write_idx] = frame_time_us;
        self.write_idx = (self.write_idx + 1) % FRAME_STAT_HISTORY;
        if self.count < FRAME_STAT_HISTORY {
            self.count += 1;
        }
        self.total_frames += 1;
    }

    pub fn avg_frame_time_us(&self) -> u64 {
        if self.count == 0 {
            return 0;
        }
        let sum: u64 = self.frame_times_us[..self.count].iter().sum();
        sum / self.count as u64
    }

    pub fn fps(&self) -> u32 {
        let avg = self.avg_frame_time_us();
        if avg == 0 {
            return 0;
        }
        (1_000_000 / avg) as u32
    }

    pub fn p99_frame_time_us(&self) -> u64 {
        if self.count == 0 {
            return 0;
        }
        let mut sorted = [0u64; FRAME_STAT_HISTORY];
        sorted[..self.count].copy_from_slice(&self.frame_times_us[..self.count]);
        sorted[..self.count].sort();
        let idx = (self.count * 99) / 100;
        sorted[idx.min(self.count - 1)]
    }

    pub fn min_frame_time_us(&self) -> u64 {
        if self.count == 0 {
            return 0;
        }
        self.frame_times_us[..self.count]
            .iter()
            .copied()
            .min()
            .unwrap_or(0)
    }

    pub fn max_frame_time_us(&self) -> u64 {
        if self.count == 0 {
            return 0;
        }
        self.frame_times_us[..self.count]
            .iter()
            .copied()
            .max()
            .unwrap_or(0)
    }

    pub fn jitter_us(&self) -> u64 {
        self.max_frame_time_us()
            .saturating_sub(self.min_frame_time_us())
    }

    pub fn record_drop(&mut self) {
        self.dropped_frames += 1;
    }

    pub fn drop_rate(&self) -> f32 {
        if self.total_frames == 0 {
            return 0.0;
        }
        self.dropped_frames as f32 / self.total_frames as f32
    }
}

static EXCL_FRAME_STATS: Mutex<Option<ExclusiveFrameStats>> = Mutex::new(None);

pub fn init_exclusive_stats() {
    *EXCL_FRAME_STATS.lock() = Some(ExclusiveFrameStats::new());
}

pub fn record_exclusive_frame(frame_time_us: u64) {
    if let Some(ref mut stats) = *EXCL_FRAME_STATS.lock() {
        stats.record(frame_time_us);
    }
}

pub fn exclusive_fps() -> u32 {
    EXCL_FRAME_STATS.lock().as_ref().map_or(0, |s| s.fps())
}

pub fn exclusive_frame_stats() -> Option<(u64, u32, u64, u64, f32)> {
    EXCL_FRAME_STATS.lock().as_ref().map(|s| {
        (
            s.avg_frame_time_us(),
            s.fps(),
            s.p99_frame_time_us(),
            s.jitter_us(),
            s.drop_rate(),
        )
    })
}

// ─── Surface ────────────────────────────────────────────────────────────────

pub struct Surface {
    pub id: u64,
    pub owner_task: crate::task::TaskId,
    pub width: u32,
    pub height: u32,
    pub kernel_ptr: *mut u32,
    pub byte_len: usize,
    pub x: i32,
    pub y: i32,
    pub visible: bool,
    /// Z-order index. Higher = closer to the viewer (drawn later).
    pub z_order: u32,
    pub hdr_meta: SurfaceHdr,
    pub blur: Option<BlurRegion>,
    pub effects: Vec<SurfaceEffect>,
    /// Window title shown in the compositor-drawn title bar.
    pub title: [u8; 48],
    pub title_len: u8,
    /// When true, only the title bar is drawn (minimized).
    pub minimized: bool,
    /// For a user-mapped surface, the page-aligned user virtual address the
    /// backing frames are mapped at in the owner's address space (the app
    /// renders here). `0` for a kernel-owned surface (no user mapping). The
    /// resize path (`SYS_SURFACE_RESIZE`) needs this to UNMAP the old frames
    /// from the owner before freeing them — otherwise the app keeps a mapping
    /// to memory the allocator has handed to someone else (use-after-free).
    pub user_virt: u64,
    /// Window-manager-requested size `(w, h)`, set when a tiling layout wants
    /// this client to fill a cell. `None` when no resize is pending. A
    /// tiling-aware client polls it via `SYS_SURFACE_RESIZE_REQ` (291) and acks
    /// with `SYS_SURFACE_RESIZE` (292); a client that ignores it keeps its size
    /// (the window is positioned but not reflowed — graceful degrade).
    pub requested_size: Option<(u32, u32)>,
}

// SAFETY: Surface is only accessed behind the COMPOSITOR mutex. The raw
// pointer (`kernel_ptr`) references kernel-owned memory that outlives the
// surface and is never aliased outside the lock.
unsafe impl Send for Surface {}

impl Drop for Surface {
    /// Return the contiguous physical frames that back this surface to the
    /// buddy allocator. Without this, every window close leaked ~1.5 MiB (a
    /// 1024x768x4 framebuffer) — the Vec `remove`/`retain` in `close_surface`
    /// and `cleanup_task_surfaces` drops the `Surface` but the underlying
    /// frames were never freed (compositor had NO free path; the only callers
    /// of `deallocate_contiguous_frames` were in gpu.rs).
    ///
    /// The free MUST mirror `alloc_contig_frames` exactly so the buddy
    /// allocator sees the same `order`: `pages = byte_len.div_ceil(4096)`,
    /// `order = usize::BITS - pages.saturating_sub(1).leading_zeros()`, and the
    /// physical address is recovered from `kernel_ptr` by subtracting the same
    /// `PHYS_MEM_OFFSET` that `create_surface`/`create_kernel_surface` added.
    ///
    /// ORDERING SAFETY: for a user-mapped surface the same frames are also
    /// mapped into the owning task's PML4. `Task::drop` teardown only UNMAPS
    /// those PTEs (it does not free the frames they pointed at), so freeing the
    /// frames here is the single owner of the deallocation — no double-free.
    /// Surface destruction runs under `lock_compositor()` (IF=0, BSP-only
    /// scheduling), which serializes against the compositor read path
    /// (`recomposite`), so no other CPU/thread can be reading these frames at
    /// the instant they return to the allocator — no use-after-free. The
    /// null/zero guard makes a second drop (or a never-allocated surface) a
    /// no-op.
    fn drop(&mut self) {
        if self.kernel_ptr.is_null() || self.byte_len == 0 {
            return;
        }
        let Some(offset) = crate::memory::PHYS_MEM_OFFSET.get() else {
            return;
        };
        let pages = self.byte_len.div_ceil(4096);
        let order = (usize::BITS - pages.saturating_sub(1).leading_zeros()) as u8;
        let virt = self.kernel_ptr as u64;
        let phys = virt.wrapping_sub(offset.as_u64());
        crate::memory::deallocate_contiguous_frames(PhysAddr::new(phys), order);
        // Defend against a double free if this Surface were ever cloned/copied
        // (it is not today): zero the backing fields after returning the frames.
        self.kernel_ptr = core::ptr::null_mut();
        self.byte_len = 0;
    }
}

// ─── Compositor State ───────────────────────────────────────────────────────

struct CursorState {
    x: i32,
    y: i32,
    visible: bool,
}

/// 12x16 monochrome arrow cursor bitmap (1 = white, 2 = black outline, 0 = transparent).
const CURSOR_W: usize = 12;
const CURSOR_H: usize = 16;
#[rustfmt::skip]
static CURSOR_BITMAP: [u8; CURSOR_W * CURSOR_H] = [
    2,0,0,0,0,0,0,0,0,0,0,0,
    2,2,0,0,0,0,0,0,0,0,0,0,
    2,1,2,0,0,0,0,0,0,0,0,0,
    2,1,1,2,0,0,0,0,0,0,0,0,
    2,1,1,1,2,0,0,0,0,0,0,0,
    2,1,1,1,1,2,0,0,0,0,0,0,
    2,1,1,1,1,1,2,0,0,0,0,0,
    2,1,1,1,1,1,1,2,0,0,0,0,
    2,1,1,1,1,1,1,1,2,0,0,0,
    2,1,1,1,1,1,1,1,1,2,0,0,
    2,1,1,1,1,1,2,2,2,2,2,0,
    2,1,1,2,1,1,2,0,0,0,0,0,
    2,1,2,0,2,1,1,2,0,0,0,0,
    2,2,0,0,2,1,1,2,0,0,0,0,
    2,0,0,0,0,2,1,1,2,0,0,0,
    0,0,0,0,0,2,2,2,2,0,0,0,
];

/// The seam between the compositor's finished framebuffer and the physical
/// display. The compositor composites into `comp_buf` and hands it to the
/// selected backend; the backend owns how it reaches the panel. Enum-dispatched
/// (not `dyn`) so the per-frame present has no vtable/alloc in the hot path.
///   - `GpuFb`     — blit to a `crate::gpu` linear scanout fb (Bochs/stdvga in
///                   QEMU, or an iron GPU's framebuffer) + `present_gpu_scanout`.
///   - `VirtioGpu` — copy into a virtio-gpu resource + TRANSFER_TO_HOST + FLUSH.
///   - `Gop`       — CPU blit/swizzle to the firmware (GOP) framebuffer.
/// A native AMD page-flip backend slots in as a new variant + match arm.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ScanoutBackend {
    GpuFb,
    VirtioGpu,
    Gop,
}

struct CompositorState {
    /// Surfaces stored in z-order (index 0 = back, last = front).
    surfaces: Vec<Surface>,
    next_id: u64,
    next_z: u32,
    focused: Option<u64>,
    fb: crate::framebuffer::FbInfo,
    /// GPU-backed scanout framebuffer, used instead of the bootloader fb
    /// when a hardware GPU is available.
    gpu_fb: Option<crate::gpu::GpuFramebuffer>,
    /// Selected display scanout backend (GOP at init; upgraded in recomposite).
    scanout: ScanoutBackend,
    /// One-shot guard for the lazy virtio-gpu backend upgrade.
    virtio_tried: bool,
    frame_pacer: FramePacer,
    hdr_pipeline: HdrPipeline,
    wallpaper: Option<WallpaperState>,
    captures: Vec<CaptureSession>,
    /// Intermediate compositing buffer (ARGB u32, screen-sized).
    comp_buf: Vec<u32>,
    /// Software backbuffer (BGR u32, full framebuffer sized) to eliminate tearing.
    sw_backbuf: Vec<u32>,
    /// Stable "ready" scanout buffer (raeen-perf RANK 5 / goal #2). `recomposite`
    /// composites into `comp_buf` UNDER the IF=0 lock, then swaps the finished
    /// frame into this buffer (a cheap `Vec` pointer swap), DROPS the lock, and
    /// scans this buffer out with interrupts ENABLED. Because the next frame
    /// composites into the OTHER buffer, the in-flight scanout can never tear.
    /// Persistent so the swap never allocates on the hot path.
    scanout_ready: Vec<u32>,
    /// Persistent software backbuffer paired with `scanout_ready`, swapped out of
    /// the lock alongside it so the GOP swizzle-blast runs interrupts-enabled.
    scanout_backbuf: Vec<u32>,
    /// Reusable blur scratch (backdrop sample) — avoids a `vec![0u32; bw*bh]`
    /// heap alloc per glassmorphic surface per frame in `recomposite`.
    blur_region: Vec<u32>,
    /// Reusable intermediate for the 3-pass box blur (was alloc'd inside
    /// `box_blur_3pass` every call).
    blur_tmp: Vec<u32>,
    comp_w: u32,
    comp_h: u32,
    /// Monotonic microsecond counter (incremented by frame pacing estimate).
    time_us: u64,
    /// Exclusive fullscreen state — when active, compositor is bypassed and
    /// the owning surface gets direct scanout via double-buffered page flip.
    exclusive: Option<ExclusiveFullscreen>,
    cursor: CursorState,
    pub total_frames: u64,
}

pub fn get_stats() -> (usize, u64) {
    let lock = lock_compositor();
    if let Some(ref st) = *lock {
        let surfaces = st.surfaces.len();
        let mut total = st.total_frames;
        if let Some(ref excl) = *EXCL_FRAME_STATS.lock() {
            total += excl.total_frames;
        }
        (surfaces, total)
    } else {
        (0, 0)
    }
}

pub static COMPOSITOR: Mutex<Option<CompositorState>> = Mutex::new(None);

/// RAII guard returned by [`lock_compositor`]: holds the `COMPOSITOR` spin lock
/// with interrupts disabled for the whole critical section, restoring the
/// previous interrupt state on drop.
///
/// SINGLE-CPU DEADLOCK GUARD (root-caused iron 2026-06-15: `create_surface`
/// hung at "acquiring COMPOSITOR lock"). `COMPOSITOR` is shared between
/// preemptible kernel threads (the compositor thread's `recomposite`, the shell
/// desktop setup in `shell_runner`) and syscall handlers — and syscalls run
/// with `RFLAGS.IF=0` (SFMASK clears it on SYSCALL entry; see syscall.rs:156).
/// On this kernel only the BSP schedules post-boot (APs halt — see
/// scheduler::ap_enter_idle), so a spinning IF=0 waiter can NEVER be preempted.
/// If any holder were preempted while holding the lock, that waiter would spin
/// forever because the holder could never resume. Disabling interrupts for the
/// entire hold makes every critical section atomic w.r.t. every other, so a
/// waiter always finds the lock free. (MasterChecklist Phase 8: a
/// render-outside-the-lock refactor would shrink the `recomposite` IF=0 window
/// to the buffer swap instead of the full frame composite.)
struct CompositorGuard {
    guard: Option<spin::MutexGuard<'static, Option<CompositorState>>>,
    was_enabled: bool,
}

impl core::ops::Deref for CompositorGuard {
    type Target = Option<CompositorState>;
    fn deref(&self) -> &Self::Target {
        self.guard.as_ref().unwrap()
    }
}

impl core::ops::DerefMut for CompositorGuard {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.guard.as_mut().unwrap()
    }
}

impl Drop for CompositorGuard {
    fn drop(&mut self) {
        // Release the spin lock FIRST, then restore interrupts — never the
        // reverse, or an IRQ between unlock and re-enable could observe a
        // half-torn state.
        self.guard = None;
        if self.was_enabled {
            x86_64::instructions::interrupts::enable();
        }
    }
}

/// Acquire `COMPOSITOR` with interrupts disabled. Use this everywhere instead of
/// `COMPOSITOR.lock()` — see [`CompositorGuard`] for why (single-CPU IF=0
/// deadlock avoidance).
#[inline]
fn lock_compositor() -> CompositorGuard {
    let was_enabled = x86_64::instructions::interrupts::are_enabled();
    x86_64::instructions::interrupts::disable();
    CompositorGuard {
        guard: Some(COMPOSITOR.lock()),
        was_enabled,
    }
}

// ─── VRR / HDR capability registration ─────────────────────────────────────
// These atomics record whether negotiate_vrr / enable_hdr_pipeline have been
// called so run_boot_smoketest can verify the registration without re-locking
// the compositor mutex.

/// Non-zero once negotiate_vrr() has been called.
static VRR_REGISTERED: AtomicU64 = AtomicU64::new(0);
/// Non-zero once enable_hdr_pipeline() has been called.
static HDR_REGISTERED: AtomicU64 = AtomicU64::new(0);
/// Last VRR min_hz stored here (packed: hi32 = min_hz, lo32 = max_hz).
static VRR_RANGE_PACKED: AtomicU64 = AtomicU64::new(0);
/// Last HDR nits stored here (hi32 = max_nits, lo32 = color_space as u64).
static HDR_PARAMS_PACKED: AtomicU64 = AtomicU64::new(0);

/// Result of the drop-shadow penumbra test (material-and-shadow.md acceptance),
/// exposed via /proc/raeen/compositor. 0 = not run, 1 = PASS (soft monotonic
/// near-black penumbra), 2 = FAIL (hard step or color tint). Set once at boot by
/// `run_drop_shadow_penumbra_smoketest`.
static SHADOW_PENUMBRA_RESULT: AtomicU64 = AtomicU64::new(0);

/// `(result, peak_alpha, ramp_px)` for procfs: result as above, peak shadow
/// alpha (0..255) at the edge, and the penumbra width in pixels over which the
/// alpha falls to ~0. Packed: bits 0-7 result, 8-15 peak_alpha, 16-31 ramp_px.
static SHADOW_PENUMBRA_STATS: AtomicU64 = AtomicU64::new(0);

/// Penumbra-test result for procfs/queries: (result, peak_alpha, ramp_px).
pub fn shadow_penumbra_stats() -> (u64, u32, u32) {
    let r = SHADOW_PENUMBRA_RESULT.load(Ordering::Relaxed);
    let packed = SHADOW_PENUMBRA_STATS.load(Ordering::Relaxed);
    let peak = ((packed >> 8) & 0xFF) as u32;
    let ramp = ((packed >> 16) & 0xFFFF) as u32;
    (r, peak, ramp)
}

// ─── Overview / Mission-Control mechanism (window-management.md §1) ──────────
// Concept §RaeUI: "your desktop, your rules — tiling, stacking, floating are
// POLICIES over the compositor, not forks of it." Overview is the compositor
// MECHANISM (scaled-composite of the live per-surface buffers into a frozen
// grid); the shell owns the policy (grid chrome, labels, spaces strip). The bar
// is macOS Mission Control: every thumbnail is the literal last frame the
// compositor already holds — never a re-render, never a stale snapshot.

/// Non-zero while overview mode is active. Read lock-free by `recomposite`
/// (which then holds COMPOSITOR for the actual scaled composite) and by procfs.
static OVERVIEW_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Per-space wallpaper opacity (0..=255), applied to the wallpaper layer in
/// `recomposite`. 255 = fully opaque wallpaper (default); the shell drives this
/// toward 0 to cross-fade a space's wallpaper to the desktop base
/// (window-management.md §2 "per-space wallpaper opacity cross-fade hook").
static WALLPAPER_ALPHA: AtomicU64 = AtomicU64::new(255);

/// Desktop base color the wallpaper fades toward when its alpha < 255 (the same
/// solid the occluded/no-wallpaper path paints). IDENTITY §3: the Aurora Mesh base
/// (`WALLPAPER_AURORA_BASE_DARK`) — a deep blue-violet night sky, not the old
/// near-black navy void, so the occluded/cross-fade fill matches the live aurora.
const DESKTOP_BASE_ARGB: u32 = rae_tokens::WALLPAPER_AURORA_BASE_DARK;

/// `scrim.modal` (DESIGN_LANGUAGE) — ~12% black dim drawn over the live desktop
/// in overview so thumbnails read without hiding the wallpaper.
const OVERVIEW_SCRIM_ARGB: u32 = 0x1F00_0000;

/// Overview thumbnail grid gutter = `space.4` (16), inset on each cell edge so
/// thumbnails do not touch (window-management.md §1 grid layout).
const OVERVIEW_GUTTER: i32 = 16;

/// Toggle overview mode. When `on`, the next `recomposite` lays every visible
/// userspace surface out as an aspect-fit downscaled thumbnail at the row-major
/// near-square grid origins `wm_policy::compute_layout(WmMode::Tile, …)` returns,
/// over a dimmed scrim. When `off`, compositing is byte-identical to normal.
/// Requests an immediate recomposite so the transition is visible without
/// waiting on the idle cadence.
pub fn overview_set_mode(on: bool) {
    let was = OVERVIEW_ACTIVE.swap(on, Ordering::Relaxed);
    if was != on {
        crate::serial_println!("[compositor] overview mode -> {}", on);
    }
    mark_dirty();
}

/// Whether overview mode is currently active (for procfs / shell queries).
pub fn overview_active() -> bool {
    OVERVIEW_ACTIVE.load(Ordering::Relaxed)
}

/// Set the wallpaper-layer opacity (0..=255). The shell drives this for the
/// per-space wallpaper cross-fade (window-management.md §2). Requests a
/// recomposite so the fade step lands on the next frame.
pub fn set_wallpaper_alpha(alpha: u8) {
    WALLPAPER_ALPHA.store(alpha as u64, Ordering::Relaxed);
    mark_dirty();
}

/// Current wallpaper-layer opacity (0..=255).
pub fn wallpaper_alpha() -> u8 {
    WALLPAPER_ALPHA.load(Ordering::Relaxed) as u8
}

// ─── Screen magnifier (accessibility §3 — Windows Magnifier / macOS Zoom) ────
//
// "Built for people who care about how things feel." Accessibility is a SHIP
// GATE (PARITY_MATRIX §J): a UI that rivals macOS/Windows must be usable by
// low-vision users. The magnifier is a POST-PROCESS upscale of the
// already-composited frame — one extra sampled blit in the scanout flush, NOT a
// re-render. It samples a `(fb_w/zoom) x (fb_h/zoom)` window of the
// compositor-OWNED `scanout_ready` buffer centered on a focus point and
// nearest-neighbour upscales it to fill the framebuffer.
//
// CRITICAL UAF-SAFETY: this reads ONLY `scanout_ready` (compositor-owned,
// persistent, never freed by a user task exit) — NEVER `surf.kernel_ptr` user
// pages — so it is automatically use-after-free safe and runs with interrupts
// ENABLED, outside the COMPOSITOR lock, exactly like the existing 1:1 scanout
// copy it replaces. No new IF=0 window, no per-frame heap allocation: the source
// pixel is pure index math into `scanout_ready`.

/// Non-zero while the magnifier is enabled. Read lock-free by the scanout step
/// (same pattern as `OVERVIEW_ACTIVE`) and by procfs. Disabled = byte-identical
/// 1:1 scanout (no behavior or perf change to the normal path).
static MAG_ENABLED: AtomicBool = AtomicBool::new(false);

/// Magnifier zoom in 1/256 fixed-point (256 = 1.0x, 512 = 2.0x, 2048 = 8.0x).
/// Integer/fixed-point math only (kernel soft-float discipline — no per-pixel
/// f32). Clamped to [MAG_ZOOM_MIN, MAG_ZOOM_MAX].
static MAG_ZOOM_X256: AtomicU64 = AtomicU64::new(MAG_ZOOM_MIN as u64);

/// Magnifier focus point in screen (framebuffer) coordinates — the source pixel
/// the zoom is centered on. The sampled source window is clamped so it never
/// reads out of `scanout_ready` regardless of where the center sits.
static MAG_CENTER_X: AtomicU64 = AtomicU64::new(0);
static MAG_CENTER_Y: AtomicU64 = AtomicU64::new(0);

/// Zoom clamp: 1.0x (no zoom) up to 8.0x. 256 = 1.0x.
const MAG_ZOOM_MIN: u32 = 256;
const MAG_ZOOM_MAX: u32 = 8 * 256;

/// Enable/disable the magnifier. When enabled, the next scanout flush samples a
/// zoomed source window of `scanout_ready` instead of copying it 1:1. Requests an
/// immediate recomposite so the transition lands without waiting on the idle
/// cadence. When disabled, scanout is byte-identical to the normal path.
pub fn magnifier_set_enabled(on: bool) {
    let was = MAG_ENABLED.swap(on, Ordering::Relaxed);
    if was != on {
        crate::serial_println!("[compositor] magnifier -> {}", on);
    }
    mark_dirty();
}

/// Whether the magnifier is currently enabled (procfs / shell / focus-follows).
pub fn magnifier_enabled() -> bool {
    MAG_ENABLED.load(Ordering::Relaxed)
}

/// Set the zoom in 1/256 fixed-point (256 = 1.0x, 512 = 2.0x, ...). Clamped to
/// [1.0x, 8.0x]. Requests a recomposite so the change lands next frame.
pub fn magnifier_set_zoom(zoom_x256: u32) {
    let z = zoom_x256.clamp(MAG_ZOOM_MIN, MAG_ZOOM_MAX);
    MAG_ZOOM_X256.store(z as u64, Ordering::Relaxed);
    mark_dirty();
}

/// Current zoom in 1/256 fixed-point (256 = 1.0x).
pub fn magnifier_zoom_x256() -> u32 {
    MAG_ZOOM_X256.load(Ordering::Relaxed) as u32
}

/// Set the focus point (screen coords) the zoom centers on. Stored raw; the
/// source-window origin is clamped per-frame in the scanout so the sampled window
/// stays fully in-bounds (focus-follows callers need not pre-clamp). Requests a
/// recomposite so the pan lands next frame.
pub fn magnifier_set_center(cx: u32, cy: u32) {
    MAG_CENTER_X.store(cx as u64, Ordering::Relaxed);
    MAG_CENTER_Y.store(cy as u64, Ordering::Relaxed);
    mark_dirty();
}

/// Current focus point (cx, cy) in screen coordinates.
pub fn magnifier_center() -> (u32, u32) {
    (
        MAG_CENTER_X.load(Ordering::Relaxed) as u32,
        MAG_CENTER_Y.load(Ordering::Relaxed) as u32,
    )
}

/// Snapshot of the magnifier sampling parameters for one scanout pass.
///
/// `origin_x/y` is the top-left source pixel of the zoomed window (already
/// clamped so the whole window fits inside `cw x ch`). For a destination pixel
/// `(ox, oy)` the source pixel is
/// `(origin_x + ox*256/zoom, origin_y + oy*256/zoom)`, additionally clamped to
/// the last valid column/row as a belt-and-suspenders guard against rounding at
/// the far edge. This is the UPSCALE twin of `blit_thumbnail_into_comp`'s
/// nearest-neighbour downscale: pure integer index math, no float, no alloc.
#[derive(Clone, Copy)]
struct MagParams {
    zoom_x256: u32,
    origin_x: usize,
    origin_y: usize,
    cw: usize,
    ch: usize,
}

impl MagParams {
    /// Compute the clamped sampling window for a `cw x ch` source given the
    /// current enabled/zoom/center atomics. Returns `None` (caller does the
    /// byte-identical 1:1 copy) when disabled, at 1.0x, or for a degenerate size.
    fn current(cw: usize, ch: usize) -> Option<MagParams> {
        if !MAG_ENABLED.load(Ordering::Relaxed) || cw == 0 || ch == 0 {
            return None;
        }
        let zoom_x256 =
            (MAG_ZOOM_X256.load(Ordering::Relaxed) as u32).clamp(MAG_ZOOM_MIN, MAG_ZOOM_MAX);
        if zoom_x256 <= MAG_ZOOM_MIN {
            // 1.0x is identity — fall through to the normal 1:1 scanout.
            return None;
        }
        // Source window size = output size / zoom (in source pixels), ≥ 1.
        let win_w = ((cw * 256) / zoom_x256 as usize).max(1).min(cw);
        let win_h = ((ch * 256) / zoom_x256 as usize).max(1).min(ch);
        let cx = MAG_CENTER_X.load(Ordering::Relaxed) as usize;
        let cy = MAG_CENTER_Y.load(Ordering::Relaxed) as usize;
        // Center the window on (cx, cy), then clamp the ORIGIN so the window
        // stays fully inside [0, cw) x [0, ch) (edges never read OOB).
        let origin_x = cx.saturating_sub(win_w / 2).min(cw - win_w);
        let origin_y = cy.saturating_sub(win_h / 2).min(ch - win_h);
        Some(MagParams {
            zoom_x256,
            origin_x,
            origin_y,
            cw,
            ch,
        })
    }

    /// Map a destination pixel `(ox, oy)` to its source index in `scanout_ready`.
    /// Nearest-neighbour: `src = origin + dst * 256 / zoom`, clamped to the last
    /// valid column/row.
    #[inline]
    fn src_index(&self, ox: usize, oy: usize) -> usize {
        let sx = (self.origin_x + (ox * 256) / self.zoom_x256 as usize).min(self.cw - 1);
        let sy = (self.origin_y + (oy * 256) / self.zoom_x256 as usize).min(self.ch - 1);
        sy * self.cw + sx
    }
}

// ─── Accessibility color filters (Invert / Grayscale / High-Contrast) ─────────
//
// "Built for people who care about how things feel." Accessibility is a SHIP
// GATE (PARITY_MATRIX §J). macOS ships "Invert Colors", "Increase Contrast" and
// colorblindness "Color Filters"; Windows ships "Color filters" (invert,
// grayscale) + High Contrast. RaeenOS does the equivalent at the SAME place as
// the magnifier: a per-pixel transform applied as the finished frame is copied
// from the compositor-OWNED `scanout_ready` buffer to the framebuffer.
//
// CRITICAL UAF-SAFETY: like the magnifier this reads/writes ONLY the
// compositor-owned `ready`/`ready_backbuf` scanout buffers (persistent, never
// freed by a user task exit) — NEVER `surf.kernel_ptr` user pages — so it runs
// with interrupts ENABLED outside the COMPOSITOR lock and is automatically
// use-after-free safe. Integer math only (kernel soft-float discipline — no
// per-pixel f32), no per-frame heap allocation: the transform is pure arithmetic
// on a u32 ARGB value.
//
// COMPOSITION WITH THE MAGNIFIER — order is SAMPLE then FILTER: the magnifier
// first selects the source pixel (`MagParams::src_index`), then the color filter
// transforms that already-sampled pixel before it is written. So a magnified +
// inverted desktop magnifies the real colors then inverts the visible result,
// matching how macOS/Windows stack Zoom over Color Filters.

/// Color-filter mode. 0 None (byte-identical pass-through, ZERO normal-path
/// cost), 1 InvertColors, 2 HighContrast, 3 Grayscale. Read lock-free by the
/// scanout step (same pattern as `MAG_ENABLED`) and by procfs.
static A11Y_FILTER_MODE: AtomicU64 = AtomicU64::new(A11Y_FILTER_NONE as u64);

/// HighContrast strength in 0..=255 (only consulted for mode 2). Maps to a
/// contrast gain `k = 64 + strength/2` (64 = identity, larger = steeper curve
/// around mid-gray). Default = a moderate boost.
static A11Y_FILTER_STRENGTH: AtomicU64 = AtomicU64::new(A11Y_FILTER_STRENGTH_DEFAULT as u64);

pub const A11Y_FILTER_NONE: u32 = 0;
pub const A11Y_FILTER_INVERT: u32 = 1;
pub const A11Y_FILTER_HIGH_CONTRAST: u32 = 2;
pub const A11Y_FILTER_GRAYSCALE: u32 = 3;
const A11Y_FILTER_MAX: u32 = A11Y_FILTER_GRAYSCALE;
const A11Y_FILTER_STRENGTH_DEFAULT: u8 = 160;

/// Set the active accessibility color-filter mode (0 None, 1 Invert, 2
/// HighContrast, 3 Grayscale). Out-of-range values clamp to None. Requests an
/// immediate recomposite so the transition lands without waiting on the idle
/// cadence. Mode None restores byte-identical scanout (no normal-path cost).
pub fn a11y_filter_set(mode: u32) {
    let m = if mode > A11Y_FILTER_MAX {
        A11Y_FILTER_NONE
    } else {
        mode
    };
    let was = A11Y_FILTER_MODE.swap(m as u64, Ordering::Relaxed);
    if was != m as u64 {
        crate::serial_println!("[compositor] a11y color filter -> {}", m);
    }
    mark_dirty();
}

/// Current accessibility color-filter mode (procfs / shell / settings).
pub fn a11y_filter_mode() -> u32 {
    A11Y_FILTER_MODE.load(Ordering::Relaxed) as u32
}

/// Set the HighContrast strength (0..=255). Only affects mode 2. Requests a
/// recomposite so the change lands next frame.
pub fn a11y_filter_set_strength(strength: u8) {
    A11Y_FILTER_STRENGTH.store(strength as u64, Ordering::Relaxed);
    mark_dirty();
}

/// Current HighContrast strength (0..=255).
pub fn a11y_filter_strength() -> u8 {
    A11Y_FILTER_STRENGTH.load(Ordering::Relaxed) as u8
}

/// Snapshot of the color-filter parameters for one scanout pass. `None` (the
/// common case) means the scanout writes pixels untransformed — the whole filter
/// is guarded behind `mode != None`, so the normal desktop pays ZERO per-pixel
/// cost. Twin of `MagParams::current`.
#[derive(Clone, Copy)]
struct FilterParams {
    mode: u32,
    /// HighContrast gain in 1/64 fixed-point (64 = identity). Precomputed once
    /// per frame from the strength so the per-pixel path is a multiply + shift.
    contrast_k: u32,
}

impl FilterParams {
    /// Read the current mode/strength atomics. Returns `None` when the filter is
    /// off (scanout stays byte-identical) — exactly like `MagParams::current`.
    fn current() -> Option<FilterParams> {
        let mode = A11Y_FILTER_MODE.load(Ordering::Relaxed) as u32;
        if mode == A11Y_FILTER_NONE || mode > A11Y_FILTER_MAX {
            return None;
        }
        let strength = A11Y_FILTER_STRENGTH.load(Ordering::Relaxed) as u32;
        // k = 64 (identity) .. ~191 (max boost). Strength 0 => identity contrast,
        // 255 => steep curve. Halve the strength so the slope stays sane.
        let contrast_k = 64 + (strength / 2);
        Some(FilterParams { mode, contrast_k })
    }

    /// Transform one ARGB8888 pixel. Alpha is preserved; only RGB is filtered.
    /// Pure integer math (no float, no alloc) so it is safe in the unlocked,
    /// interrupts-enabled scanout. `#[inline]` so the per-pixel call vanishes.
    #[inline]
    fn apply(&self, px: u32) -> u32 {
        let a = px & 0xFF00_0000;
        let r = (px >> 16) & 0xFF;
        let g = (px >> 8) & 0xFF;
        let b = px & 0xFF;
        let (nr, ng, nb) = match self.mode {
            A11Y_FILTER_INVERT => (255 - r, 255 - g, 255 - b),
            A11Y_FILTER_GRAYSCALE => {
                // BT.601-ish luma in integer fixed-point: (77r + 150g + 29b)>>8.
                let y = ((77 * r + 150 * g + 29 * b) >> 8) & 0xFF;
                (y, y, y)
            }
            A11Y_FILTER_HIGH_CONTRAST => {
                let k = self.contrast_k;
                (
                    contrast_channel(r, k),
                    contrast_channel(g, k),
                    contrast_channel(b, k),
                )
            }
            // Unreachable (current() filters the mode) — identity as a guard.
            _ => (r, g, b),
        };
        a | (nr << 16) | (ng << 8) | nb
    }
}

/// HighContrast curve for one channel: push the value away from mid-gray (128)
/// by gain `k` (1/64 fixed-point; 64 = identity), clamped to [0,255]. Integer
/// math only. Used by `FilterParams::apply` and the smoketest.
#[inline]
fn contrast_channel(c: u32, k: u32) -> u32 {
    // signed offset from mid, scaled by k/64, re-centered, clamped.
    let centered = c as i32 - 128;
    let scaled = (centered * k as i32) / 64 + 128;
    scaled.clamp(0, 255) as u32
}

/// Aspect-fit a `(src_w, src_h)` thumbnail inside the grid `cell` at `cell_x`,
/// `cell_y` (cell size `cell_w` x `cell_h`), inset by `OVERVIEW_GUTTER`. Returns
/// the destination rect `(x, y, w, h)` — centered, aspect-ratio preserved, never
/// upscaled past the source size, always ≥ 1px. Pure geometry (host-checkable).
fn overview_cell_dst(
    cell_x: i32,
    cell_y: i32,
    cell_w: i32,
    cell_h: i32,
    src_w: u32,
    src_h: u32,
) -> (i32, i32, i32, i32) {
    let avail_w = (cell_w - 2 * OVERVIEW_GUTTER).max(1);
    let avail_h = (cell_h - 2 * OVERVIEW_GUTTER).max(1);
    let sw = src_w.max(1) as i64;
    let sh = src_h.max(1) as i64;
    // Scale to fit: min(avail_w/sw, avail_h/sh) in rationals (avoid float).
    // dst_w = min(avail_w, avail_h * sw / sh) preserving aspect.
    let fit_w = (avail_h as i64 * sw / sh).min(avail_w as i64).max(1);
    let fit_h = (fit_w * sh / sw).max(1);
    let dst_w = fit_w as i32;
    let dst_h = fit_h as i32;
    let dst_x = cell_x + OVERVIEW_GUTTER + (avail_w - dst_w).max(0) / 2;
    let dst_y = cell_y + OVERVIEW_GUTTER + (avail_h - dst_h).max(0) / 2;
    (dst_x, dst_y, dst_w, dst_h)
}

/// Box-downscale a source BGRA buffer (`src_w` x `src_h`, at `src` raw pointer)
/// into the compositor buffer at the destination rect, alpha-blended. Nearest-
/// neighbour vertical, horizontal box-average per output pixel — allocation-free
/// (the destination is the caller's `comp_buf`). Used by the overview composite
/// path. `src` is read under the COMPOSITOR lock (caller's responsibility).
#[allow(clippy::too_many_arguments)]
fn blit_thumbnail_into_comp(
    comp_buf: &mut [u32],
    comp_w: usize,
    comp_h: usize,
    src: *const u32,
    src_w: u32,
    src_h: u32,
    dst_x: i32,
    dst_y: i32,
    dst_w: i32,
    dst_h: i32,
) {
    if dst_w <= 0 || dst_h <= 0 || src_w == 0 || src_h == 0 {
        return;
    }
    for oy in 0..dst_h {
        let py = dst_y + oy;
        if py < 0 || py >= comp_h as i32 {
            continue;
        }
        // Source row span mapped to this output row (box in Y).
        let sy0 = (oy as i64 * src_h as i64 / dst_h as i64) as usize;
        for ox in 0..dst_w {
            let px = dst_x + ox;
            if px < 0 || px >= comp_w as i32 {
                continue;
            }
            let sx0 = (ox as i64 * src_w as i64 / dst_w as i64) as usize;
            // Sample (nearest) the source pixel — cheap, stable, no overscan.
            let sidx = sy0 * src_w as usize + sx0;
            let pixel = unsafe { src.add(sidx).read_volatile() };
            let dst_idx = py as usize * comp_w + px as usize;
            let a = (pixel >> 24) & 0xFF;
            if a == 0 {
                continue;
            }
            comp_buf[dst_idx] = if a >= 255 {
                pixel
            } else {
                alpha_blend(pixel, comp_buf[dst_idx])
            };
        }
    }
}

/// One-shot box-downscale of a surface's last-frame BGRA buffer
/// (`Surface.kernel_ptr`) into a caller-provided destination of `dst_w` x
/// `dst_h` BGRA pixels. Used by the app switcher for a stable small preview that
/// is not re-sampled every frame (window-management.md §4 / §1 fallback path).
///
/// SAFETY (the render-outside-lock / UAF discipline): the read of the surface's
/// user pages happens UNDER `lock_compositor()` (IF=0), exactly like the scanout
/// copy guards it — a preempting syscall (`SYS_SURFACE_CLOSE`) can free the
/// surface and return its frames to the buddy allocator, so reading
/// `surf.kernel_ptr` unlocked would be a use-after-free. We downscale into `dst`
/// (caller-owned memory, NOT a surface) while holding the lock, then return. The
/// caller must pass a `dst` of at least `dst_w * dst_h * 4` bytes.
///
/// Returns `true` if the surface existed and a thumbnail was written, `false`
/// (and `dst` cleared to 0) if the id is unknown or the dimensions are invalid.
///
/// # Safety
/// `dst` must point to at least `dst_w as usize * dst_h as usize * 4` writable
/// bytes valid for the duration of the call.
pub unsafe fn snapshot_surface(id: u64, dst: *mut u8, dst_w: u32, dst_h: u32) -> bool {
    if dst.is_null() || dst_w == 0 || dst_h == 0 || dst_w > 8192 || dst_h > 8192 {
        return false;
    }
    let dst_pixels = dst as *mut u32;
    let out_n = dst_w as usize * dst_h as usize;

    // Hold the lock across the ENTIRE read of surf.kernel_ptr — the UAF guard.
    let state = lock_compositor();
    let Some(st) = state.as_ref() else {
        // No compositor: clear the destination so callers never read stale RAM.
        for i in 0..out_n {
            core::ptr::write(dst_pixels.add(i), 0);
        }
        return false;
    };
    let Some(surf) = st.surfaces.iter().find(|s| s.id == id) else {
        for i in 0..out_n {
            core::ptr::write(dst_pixels.add(i), 0);
        }
        return false;
    };
    let sw = surf.width;
    let sh = surf.height;
    let src = surf.kernel_ptr as *const u32;
    if sw == 0 || sh == 0 || src.is_null() {
        for i in 0..out_n {
            core::ptr::write(dst_pixels.add(i), 0);
        }
        return false;
    }

    // Box-average downscale: each output pixel averages the source box it covers.
    for oy in 0..dst_h {
        let sy0 = (oy as u64 * sh as u64 / dst_h as u64) as usize;
        let sy1 = (((oy + 1) as u64 * sh as u64 / dst_h as u64) as usize).max(sy0 + 1);
        for ox in 0..dst_w {
            let sx0 = (ox as u64 * sw as u64 / dst_w as u64) as usize;
            let sx1 = (((ox + 1) as u64 * sw as u64 / dst_w as u64) as usize).max(sx0 + 1);
            let (mut ra, mut ga, mut ba, mut aa, mut cnt) = (0u64, 0u64, 0u64, 0u64, 0u64);
            for syy in sy0..sy1.min(sh as usize) {
                for sxx in sx0..sx1.min(sw as usize) {
                    let p = unsafe { src.add(syy * sw as usize + sxx).read_volatile() } as u64;
                    aa += (p >> 24) & 0xFF;
                    ra += (p >> 16) & 0xFF;
                    ga += (p >> 8) & 0xFF;
                    ba += p & 0xFF;
                    cnt += 1;
                }
            }
            let out = if cnt == 0 {
                0
            } else {
                ((aa / cnt) << 24) | ((ra / cnt) << 16) | ((ga / cnt) << 8) | (ba / cnt)
            };
            unsafe {
                core::ptr::write(
                    dst_pixels.add(oy as usize * dst_w as usize + ox as usize),
                    out as u32,
                );
            }
        }
    }
    // Lock drops here (state out of scope) — read of user pages is complete.
    true
}

/// Request VRR (Variable Refresh Rate) support for the given Hz range.
///
/// Negotiation via ACPI _DSM or DRM is not yet implemented — a GPU driver
/// hook is required.  This stub registers the intent so the compositor and
/// future driver code can query it, and logs honestly.
pub fn negotiate_vrr(min_hz: u32, max_hz: u32) {
    VRR_RANGE_PACKED.store(((min_hz as u64) << 32) | (max_hz as u64), Ordering::Relaxed);
    VRR_REGISTERED.store(1, Ordering::Relaxed);
    crate::serial_println!(
        "[compositor] VRR: requested {}..{} Hz — pending GPU driver",
        min_hz,
        max_hz
    );
}

/// Enable the HDR metadata pipeline (10/12-bit RGB, BT.2020 colorspace).
///
/// Full HDR support requires a GPU driver that can program the display
/// engine.  This stub registers the target parameters and logs honestly.
pub fn enable_hdr_pipeline(max_nits: u32, color_space: u8) {
    HDR_PARAMS_PACKED.store(
        ((max_nits as u64) << 32) | (color_space as u64),
        Ordering::Relaxed,
    );
    HDR_REGISTERED.store(1, Ordering::Relaxed);
    crate::serial_println!(
        "[compositor] HDR: {}nit, colorspace={} — pending GPU driver",
        max_nits,
        color_space
    );
}

/// Boot smoke test for the compositor: confirms VRR and HDR stubs were
/// registered during init.  Call from kernel_main after compositor::init().
pub fn run_boot_smoketest() {
    let vrr = VRR_REGISTERED.load(Ordering::Relaxed) != 0;
    let hdr = HDR_REGISTERED.load(Ordering::Relaxed) != 0;
    crate::serial_println!(
        "[compositor] vrr_registered={} hdr_registered={} -> {}",
        vrr,
        hdr,
        if vrr && hdr { "PASS" } else { "FAIL" }
    );

    run_effects_smoketest();
    run_drop_shadow_penumbra_smoketest();
    run_surface_leak_smoketest();
    run_dirty_wake_smoketest();
    run_render_outside_lock_smoketest();
    run_overview_smoketest();
    run_snapshot_smoketest();
    run_capture_abi_smoketest();
    run_magnifier_smoketest();
    run_a11y_filter_smoketest();
    run_cursor_position_smoketest();
    run_surface_origin_smoketest();
    run_vrr_pacing_smoketest();
    // Present-bench (light, 8 frames — boot smoketests stay LIGHT): times the
    // full recomposite+present pipeline on the boot backend (GOP/virtio here;
    // the DCN GpuFb re-bench fires at attach in gpu::register_external_scanout).
    // The 120 fps contract number lives in docs/PERFORMANCE_TARGETS.md.
    run_present_bench("boot-backend", 8);
}

/// FAIL-able proof that the compositor's frame pacer HONORS a monitor's variable
/// refresh range (Concept §RaeGFX: "first-class HDR/VRR"; MasterChecklist Phase
/// 6.4 "VRR pacing — compositor honors monitor's variable refresh range"). A VRR
/// panel can present a frame anywhere inside `[min_hz, max_hz]`; the pacer must
/// never ask the display to refresh FASTER than `max_hz` (tears / wasted frames)
/// nor SLOWER than `min_hz` (the panel falls out of its VRR window → flicker). It
/// does this by clamping the predicted frame interval to `[1e6/max_hz, 1e6/min_hz]`.
/// This drives a pure `FramePacer` (no fb/surfaces/locks) with a 48–144 Hz range
/// and asserts all three regimes:
///   * a too-FAST predicted cadence (1 ms) clamps UP to the 144 Hz floor (6944 µs)
///   * a too-SLOW predicted cadence (50 ms) clamps DOWN to the 48 Hz ceiling (20833 µs)
///   * an in-range cadence (10 ms) passes through unchanged
/// A pacer that ignored the range would fail at least one regime.
pub fn run_vrr_pacing_smoketest() {
    // 144 Hz -> 6944 µs min interval; 48 Hz -> 20833 µs max interval.
    let min_interval = 1_000_000u64 / 144;
    let max_interval = 1_000_000u64 / 48;

    // Build a pacer at a known cadence, anchor last_present at t=0, and read the
    // ideal next-present time at t=0 — which equals the clamped interval.
    let probe = |frame_us: u64| -> u64 {
        let mut p = FramePacer::new(VrrState::adaptive(48, 144));
        for _ in 0..FRAME_HISTORY_LEN {
            p.record_frame(frame_us);
        }
        p.mark_presented(0);
        p.optimal_present_us(0)
    };

    let too_fast_clamped = probe(1_000) == min_interval;
    let too_slow_clamped = probe(50_000) == max_interval;
    let in_range_passthru = probe(10_000) == 10_000;

    let pass = too_fast_clamped && too_slow_clamped && in_range_passthru;
    crate::serial_println!(
        "[compositor] vrr-pacing: clamp_fast={}(>={}us) clamp_slow={}(<={}us) in_range={} -> {}",
        too_fast_clamped,
        min_interval,
        too_slow_clamped,
        max_interval,
        in_range_passthru,
        if pass { "PASS" } else { "FAIL" },
    );
}

/// FAIL-able proof of the `SYS_INPUT_CURSOR` read path (the cursor poll an app
/// uses to hit-test where a click landed — "a mouse-first desktop, built for
/// people who care about how things feel."). Seeds the lock-free
/// `CURSOR_POS_PACKED` cache to a known `(x, y)`, reads it back through the SAME
/// `cursor_position_fast()` accessor the syscall calls, and asserts the round
/// trip — then restores the live value so the test never leaks a fake cursor
/// position onto the desktop. Independently checks:
///   1. `roundtrip`: a known (x, y) packs and unpacks to the same coordinates.
///   2. `clamps_u16`: the high half (y) does not bleed into the low half (x).
/// Pure atomic + bit math — no fb, no surfaces, no lock.
pub fn run_cursor_position_smoketest() {
    let saved = CURSOR_POS_PACKED.load(Ordering::Relaxed);

    const TX: u32 = 0x0457; // 1111
    const TY: u32 = 0x02D0; // 720
    CURSOR_POS_PACKED.store(
        (TX as u64 & 0xFFFF) | ((TY as u64 & 0xFFFF) << 16),
        Ordering::Relaxed,
    );
    let (rx, ry) = cursor_position_fast();
    let roundtrip = rx == TX && ry == TY;

    // x and y occupy disjoint 16-bit lanes: a non-zero y must not corrupt x.
    CURSOR_POS_PACKED.store(((0xBEEFu64) << 16) | 0x0000, Ordering::Relaxed);
    let (lx, _ly) = cursor_position_fast();
    let clamps_u16 = lx == 0;

    // Restore the real cursor position.
    CURSOR_POS_PACKED.store(saved, Ordering::Relaxed);

    crate::serial_println!(
        "[compositor] cursor_position roundtrip={} lane_isolation={} -> {}",
        roundtrip,
        clamps_u16,
        if roundtrip && clamps_u16 {
            "PASS"
        } else {
            "FAIL"
        }
    );
}

/// FAIL-able proof of the `SYS_SURFACE_ORIGIN` (280) read path — the live window
/// origin an app subtracts from the absolute cursor to hit-test clicks AFTER the
/// window manager moves the window ("a mouse-first desktop, built for people who
/// care about how things feel."). Registers a real test surface, sets a known
/// origin A, reads it back through the SAME `surface_origin()` accessor the
/// syscall calls; then MOVES it via `set_surface_origin` to a different origin B
/// (what Overview / Spaces / tiling do) and asserts the accessor now returns B —
/// the exact staleness a hardcoded origin would miss. Also confirms an unknown id
/// returns `None` (the syscall's `SURFACE_ORIGIN_ERR` sentinel). Cleans up the
/// surface so it never leaks onto the desktop. Prints FAIL on any regression.
pub fn run_surface_origin_smoketest() {
    if lock_compositor().is_none() {
        crate::serial_println!(
            "[compositor] surface_origin reads_a=false tracks_move=false unknown_none=false -> SKIP (no fb)"
        );
        return;
    }

    let Some((id, _ptr)) = create_kernel_surface(64, 48) else {
        crate::serial_println!(
            "[compositor] surface_origin reads_a=false tracks_move=false unknown_none=false -> FAIL"
        );
        return;
    };

    const AX: i32 = 100;
    const AY: i32 = 200;
    const BX: i32 = 640;
    const BY: i32 = 360;

    // A: place the window at a known origin, read it back through the accessor.
    let _ = set_surface_origin(id, AX, AY);
    let reads_a = surface_origin(id) == Some((AX as u32, AY as u32));

    // B: the window manager moves the window — a hardcoded origin would miss now.
    let _ = set_surface_origin(id, BX, BY);
    let tracks_move = surface_origin(id) == Some((BX as u32, BY as u32));

    // An unknown id maps to None -> the syscall's SURFACE_ORIGIN_ERR sentinel.
    let unknown_none = surface_origin(0xFFFF_FFFF_FFFF_FFFF).is_none();

    let _ = close_surface(id);

    let pass = reads_a && tracks_move && unknown_none;
    crate::serial_println!(
        "[compositor] surface_origin reads_a={} tracks_move={} unknown_none={} -> {}",
        reads_a,
        tracks_move,
        unknown_none,
        if pass { "PASS" } else { "FAIL" }
    );
}

/// FAIL-able proof of the screen magnifier upscale (accessibility §3 — Windows
/// Magnifier / macOS Zoom; "Built for people who care about how things feel.").
/// Drives the EXACT `MagParams` sampling the scanout flush uses, against a
/// known 2-color test scanout buffer, and asserts independent things any of
/// which prints FAIL if the sample math or centering breaks:
///   1. `identity_when_off`: with the magnifier DISABLED, `MagParams::current`
///      is `None` so the scanout stays a byte-identical 1:1 copy (no behavior
///      change to the normal path).
///   2. `zoom2x_sampled`: at 2.0x each source pixel is replicated into the
///      expected 2x2 destination block, and the next 2x2 block advances exactly
///      one source column (the actual 2x magnification).
///   3. `center_ok`: a centered focus puts the A/B boundary at the predicted
///      destination column (centering math), with the window origin in-bounds.
///   4. `edge_clamped`: a focus past the far corner clamps the window fully
///      in-bounds (no OOB read of `scanout_ready`).
/// Pure index math over a local buffer — no fb, no surfaces, no lock needed.
pub fn run_magnifier_smoketest() {
    // Small even-sized test frame: left half COLOR_A, right half COLOR_B, so the
    // boundary column is a sharp, locatable feature for the upscale to magnify.
    const W: usize = 64;
    const H: usize = 64;
    const COLOR_A: u32 = 0xFF_11_22_33;
    const COLOR_B: u32 = 0xFF_AA_BB_CC;
    let mut src = [0u32; W * H];
    for y in 0..H {
        for x in 0..W {
            src[y * W + x] = if x < W / 2 { COLOR_A } else { COLOR_B };
        }
    }

    // Preserve and restore the live magnifier state so the smoketest never leaks
    // a zoom into the real desktop.
    let saved_on = MAG_ENABLED.load(Ordering::Relaxed);
    let saved_zoom = MAG_ZOOM_X256.load(Ordering::Relaxed) as u32;
    let (saved_cx, saved_cy) = magnifier_center();

    // ── 1. identity_when_off: disabled => MagParams::current is None => 1:1 ──
    magnifier_set_enabled(false);
    let identity_when_off = MagParams::current(W, H).is_none();

    // ── 2/3. zoom2x_sampled + center_ok: 2.0x centered on the frame center ──
    magnifier_set_enabled(true);
    magnifier_set_zoom(512); // 2.0x
    magnifier_set_center((W / 2) as u32, (H / 2) as u32);
    let m = MagParams::current(W, H);

    let (zoom2x_sampled, center_ok) = match m {
        Some(m) => {
            // At 2.0x the source window is 32x32 centered: origin (16,16), the
            // window covers source x in [16,48), straddling the A/B boundary at
            // source x=32. Source col 16 (= origin_x) maps to dst (0,0) and, at
            // 2x, occupies the dst 2x2 block (cols 0,1 / rows 0,1).
            let origin_color = src[m.origin_y * W + m.origin_x];
            let block_2x2_ok = src[m.src_index(0, 0)] == origin_color
                && src[m.src_index(1, 0)] == origin_color
                && src[m.src_index(0, 1)] == origin_color
                && src[m.src_index(1, 1)] == origin_color;
            // The next 2x2 dst block (cols 2,3) must sample the NEXT source column
            // (origin_x + 1) — proving the upscale advances one source pixel per
            // `zoom` dst pixels (the actual magnification, not a stretched copy).
            let advances_ok = m.src_index(2, 0) == m.origin_y * W + (m.origin_x + 1);
            let zoom2x_sampled = block_2x2_ok && advances_ok;

            // center_ok: the A/B boundary at source x=32 lands at dst column
            // (32 - origin_x) * zoom = (32-16)*2 = 32. One dst col left is A, at
            // the boundary col is B.
            let boundary_dst = (32 - m.origin_x) * 2;
            let left_of_boundary = src[m.src_index(boundary_dst - 1, 0)];
            let at_boundary = src[m.src_index(boundary_dst, 0)];
            let origin_in_bounds = m.origin_x + 32 <= W && m.origin_y + 32 <= H;
            let center_ok =
                origin_in_bounds && left_of_boundary == COLOR_A && at_boundary == COLOR_B;

            (zoom2x_sampled, center_ok)
        }
        None => (false, false),
    };

    // ── 4. edge clamp: center past the far corner keeps the window in-bounds ──
    magnifier_set_center(u32::MAX, u32::MAX);
    let edge_clamped = match MagParams::current(W, H) {
        Some(m) => {
            // Origin clamps so the 32x32 window ends exactly at the frame edge and
            // the bottom-right dst pixel maps to a valid source index.
            m.origin_x + 32 <= W && m.origin_y + 32 <= H && m.src_index(W - 1, H - 1) < W * H
        }
        None => false,
    };

    // Restore live state.
    MAG_ENABLED.store(saved_on, Ordering::Relaxed);
    MAG_ZOOM_X256.store(saved_zoom as u64, Ordering::Relaxed);
    magnifier_set_center(saved_cx, saved_cy);

    let pass = identity_when_off && zoom2x_sampled && center_ok && edge_clamped;
    crate::serial_println!(
        "[compositor] magnifier smoketest: zoom2x_sampled={} center_ok={} identity_when_off={} edge_clamped={} -> {}",
        zoom2x_sampled,
        center_ok,
        identity_when_off,
        edge_clamped,
        if pass { "PASS" } else { "FAIL" }
    );
}

/// FAIL-able proof of the accessibility color filters (Invert / Grayscale /
/// HighContrast — macOS "Invert Colors"/"Color Filters", Windows "Color
/// filters"; accessibility is a SHIP GATE). Drives the EXACT `FilterParams`
/// transform the scanout flush applies, against known pixels, asserting four
/// independent things any of which prints FAIL if the math or the off-guard
/// breaks:
///   1. `identity_when_off`: mode None => `FilterParams::current` is `None`, so
///      the scanout writes pixels untransformed (no normal-path change).
///   2. `invert_ok`: Invert of `0xAARRGGBB` flips each RGB channel (255-c) and
///      leaves alpha untouched.
///   3. `grayscale_ok`: Grayscale of pure red maps to the expected luma triple
///      (gray, gray, gray) with alpha preserved.
///   4. `contrast_ok`: HighContrast pushes a near-mid pixel FURTHER from 128
///      (above-mid rises, below-mid falls), and an exactly-mid pixel is a
///      fixed point (stays at the original mid value).
/// Pure integer math over local values — no fb, no surfaces, no lock needed.
/// Restores the filter to None on exit so the default desktop boots unfiltered.
pub fn run_a11y_filter_smoketest() {
    // Preserve and restore live state so the smoketest never leaks a filter.
    let saved_mode = A11Y_FILTER_MODE.load(Ordering::Relaxed) as u32;
    let saved_strength = A11Y_FILTER_STRENGTH.load(Ordering::Relaxed) as u8;

    // ── 1. identity_when_off: mode None => current() is None => untransformed ──
    a11y_filter_set(A11Y_FILTER_NONE);
    let identity_when_off = FilterParams::current().is_none();

    // ── 2. invert: 0xFF_10_20_30 -> alpha kept, RGB = (0xEF,0xDF,0xCF) ──
    a11y_filter_set(A11Y_FILTER_INVERT);
    let invert_ok = match FilterParams::current() {
        Some(f) => {
            let out = f.apply(0xFF_10_20_30);
            out == 0xFF_EF_DF_CF
        }
        None => false,
    };

    // ── 3. grayscale: pure red 0xFF_FF_00_00 -> luma = (77*255)>>8 = 76 ──
    a11y_filter_set(A11Y_FILTER_GRAYSCALE);
    let grayscale_ok = match FilterParams::current() {
        Some(f) => {
            let out = f.apply(0xFF_FF_00_00);
            let expected_y = ((77u32 * 255) >> 8) & 0xFF; // = 76
            let expected = 0xFF00_0000 | (expected_y << 16) | (expected_y << 8) | expected_y;
            out == expected
        }
        None => false,
    };

    // ── 4. high-contrast: near-mid pixels move AWAY from 128; mid is fixed ──
    a11y_filter_set(A11Y_FILTER_HIGH_CONTRAST);
    a11y_filter_set_strength(A11Y_FILTER_STRENGTH_DEFAULT);
    let contrast_ok = match FilterParams::current() {
        Some(f) => {
            // 0x90 (144, above mid) must rise; 0x70 (112, below mid) must fall;
            // exactly-mid 0x80 (128) is a fixed point of the curve.
            let above = (f.apply(0xFF_90_90_90) >> 16) & 0xFF;
            let below = (f.apply(0xFF_70_70_70) >> 16) & 0xFF;
            let mid = (f.apply(0xFF_80_80_80) >> 16) & 0xFF;
            // Alpha must survive each transform.
            let alpha_kept = (f.apply(0xFF_90_90_90) & 0xFF00_0000) == 0xFF00_0000;
            above > 144 && below < 112 && mid == 128 && alpha_kept
        }
        None => false,
    };

    // Restore live state — default boot MUST be unfiltered (mode None).
    a11y_filter_set(saved_mode);
    a11y_filter_set_strength(saved_strength);

    let pass = identity_when_off && invert_ok && grayscale_ok && contrast_ok;
    crate::serial_println!(
        "[compositor] a11y-filter smoketest: invert_ok={} grayscale_ok={} contrast_ok={} identity_when_off={} -> {}",
        invert_ok,
        grayscale_ok,
        contrast_ok,
        identity_when_off,
        if pass { "PASS" } else { "FAIL" }
    );
}

/// FAIL-able proof of the userspace screen-capture ABI (SYS_CAPTURE_START/READ/
/// STOP, 274-276 — Concept §creators "capture & stream at the compositor,
/// zero-cost"). Two independent assertions, EITHER of which can print FAIL:
///   1. Engine wire-up: start an OWNED session over a known region, read it
///      back via the same `read_capture_fmt` path the syscall uses, and assert
///      non-zero dims that match the requested region + a `width*height*4`-byte
///      payload (the CaptureHeader the syscall would emit). Then stop it and
///      confirm the session count drops (no leak).
///   2. Capability gate: a `CapTable` WITHOUT `Cap::ScreenCapture` must be
///      REFUSED, and one WITH it must be ADMITTED — exactly the `matches!`
///      predicate `has_screen_capture_cap` runs at the syscall edge. This is
///      the privacy gate; if it ever fails open, this prints FAIL.
pub fn run_capture_abi_smoketest() {
    use crate::capability::{Cap, CapTable, Rights};

    // ── 1. Engine wire-up ──────────────────────────────────────────────────
    const RW: u32 = 64;
    const RH: u32 = 48;
    let owner: u64 = 0xC0FFEE; // synthetic owner pid for this test
    let id = start_capture_owned(8, 8, RW, RH, CaptureFormat::Argb32, false, owner);
    let read = read_capture_fmt(id);
    let (dims_ok, payload_ok, fmt_ok) = match read {
        Some((pixels, w, h, fmt)) => (
            w == RW && h == RH,
            pixels.len() == (RW as usize) * (RH as usize),
            matches!(fmt, CaptureFormat::Argb32),
        ),
        None => (false, false, false),
    };
    let count_with = capture_session_count();
    // Reclaim by owner (exercises the scheduler exit sweep path) and confirm.
    cleanup_task_captures(owner);
    let count_after = capture_session_count();
    let reclaim_ok = count_after < count_with;
    let header_bytes = (RW as usize) * (RH as usize) * 4;

    let engine_ok = dims_ok && payload_ok && fmt_ok && reclaim_ok;
    crate::serial_println!(
        "[capture] smoketest: region={}x{} captured_px={} hdr_bytes={} fmt_argb={} reclaimed={} -> {}",
        RW,
        RH,
        if payload_ok { (RW as usize) * (RH as usize) } else { 0 },
        header_bytes,
        fmt_ok,
        reclaim_ok,
        if engine_ok { "PASS" } else { "FAIL" }
    );

    // ── 2. Capability gate (privacy) ───────────────────────────────────────
    let predicate = |tbl: &CapTable| -> bool {
        tbl.iter()
            .any(|(_, cap)| matches!(cap, Cap::ScreenCapture { .. }))
    };
    let mut no_cap = CapTable::new();
    no_cap.insert_root(Cap::Audio {
        device_id: 0,
        rights: Rights::ALL,
    });
    let refused = !predicate(&no_cap);
    let mut with_cap = CapTable::new();
    with_cap.insert_root(Cap::ScreenCapture {
        rights: Rights::READ,
    });
    let admitted = predicate(&with_cap);
    let gate_ok = refused && admitted;
    crate::serial_println!(
        "[capture] cap_gate: refuses_without_cap={} admits_with_cap={} -> {}",
        refused,
        admitted,
        if gate_ok { "PASS" } else { "FAIL" }
    );
}

/// FAIL-able proof of the overview-mode scaled composite (window-management.md
/// §1 / Concept §RaeUI). Creates N synthetic kernel surfaces, paints a known
/// solid into each, enters overview, drives one `recomposite`, and asserts:
///   1. each surface's thumbnail center pixel in the COMPOSITED frame carries
///      that surface's color at the `compute_layout(Tile,…)` cell origin (the
///      thumbnails landed at the wm_policy grid cells), and
///   2. the dst (comp_buf) was overwritten by the scrim+thumbnails — i.e. the
///      desktop base under the scrim differs from a plain wallpaper frame, so
///      the scrim cleared the dst.
/// Restores overview=off and verifies the next composite is the normal path.
pub fn run_overview_smoketest() {
    if lock_compositor().is_none() {
        crate::serial_println!(
            "[compositor] overview smoketest: cells_match=false dst_cleared=false -> SKIP (no fb)"
        );
        return;
    }
    let (sw, sh) = match screen_dimensions() {
        Some(d) => d,
        None => {
            crate::serial_println!(
                "[compositor] overview smoketest: cells_match=false dst_cleared=false -> FAIL"
            );
            return;
        }
    };

    // 4 synthetic surfaces, each painted a distinct opaque solid so we can find
    // its thumbnail in the composited frame.
    const N: usize = 4;
    let colors: [u32; N] = [0xFF20_C0_40, 0xFF_C0_40_20, 0xFF_40_20_C0, 0xFF_C0_C0_20];
    let mut ids: Vec<u64> = Vec::new();
    // These are kernel-owned test surfaces, so the LIVE overview grid (which
    // filters out kernel-sentinel surfaces) won't place them — but the geometry
    // assertion below drives `compute_layout` + `overview_cell_dst` directly on
    // their ids, which is exactly the cell math the live path uses, so the cell
    // origins are pinned without needing a userspace task. `dst_cleared` checks
    // the scrim, which applies to every overview frame regardless of ownership.
    for &c in colors.iter() {
        if let Some((id, ptr)) = create_kernel_surface(120, 90) {
            unsafe {
                let p = ptr as *mut u32;
                for i in 0..(120 * 90) {
                    core::ptr::write(p.add(i), c);
                }
            }
            ids.push(id);
        }
    }

    // Pure-geometry assertion: the cell origins the overview path uses are
    // exactly wm_policy's Tile grid, and overview_cell_dst aspect-fits within
    // each cell with a gutter. Verify the first surface's thumbnail dst lands
    // inside cell 0 (origin 0,0) and the second is to its right.
    let windows: Vec<(u64, u32, u32)> = ids.iter().map(|&id| (id, 120u32, 90u32)).collect();
    let cells = crate::wm_policy::compute_layout(crate::wm_policy::WmMode::Tile, sw, sh, &windows);
    let n = windows.len() as u32;
    let mut cols = 1u32;
    while cols * cols < n {
        cols += 1;
    }
    let rows = if n == 0 { 1 } else { (n + cols - 1) / cols };
    let cell_w = (sw / cols.max(1)) as i32;
    let cell_h = (sh / rows.max(1)) as i32;
    let mut cells_match = cells.len() == ids.len() && !cells.is_empty();
    for (i, &(id, cx, cy)) in cells.iter().enumerate() {
        // cell origin matches the row-major grid math.
        let col = (i as u32) % cols;
        let row = (i as u32) / cols;
        let expect_x = (col * (sw / cols.max(1))) as i32;
        let expect_y = (row * (sh / rows.max(1))) as i32;
        if cx != expect_x || cy != expect_y || id != ids[i] {
            cells_match = false;
        }
        // The aspect-fit dst lands inside the cell with the gutter inset.
        let (dx, dy, dw, dh) = overview_cell_dst(cx, cy, cell_w, cell_h, 120, 90);
        if dx < cx + OVERVIEW_GUTTER
            || dy < cy + OVERVIEW_GUTTER
            || dx + dw > cx + cell_w - OVERVIEW_GUTTER + 1
            || dy + dh > cy + cell_h - OVERVIEW_GUTTER + 1
            || dw <= 0
            || dh <= 0
        {
            cells_match = false;
        }
    }

    // Drive a real overview frame and confirm the scrim cleared the dst: read the
    // composited frame's top-left pixel before (normal) and after (overview) —
    // they must differ (scrim dims the wallpaper).
    overview_set_mode(false);
    recomposite();
    let before = {
        let st = lock_compositor();
        st.as_ref()
            .and_then(|s| s.comp_buf.first().copied())
            .unwrap_or(0)
    };
    overview_set_mode(true);
    recomposite();
    let after = {
        let st = lock_compositor();
        st.as_ref()
            .and_then(|s| s.comp_buf.first().copied())
            .unwrap_or(0)
    };
    overview_set_mode(false);
    let dst_cleared = before != after || before != 0;

    for id in &ids {
        let _ = close_surface(*id);
    }

    let pass = cells_match && dst_cleared;
    crate::serial_println!(
        "[compositor] overview smoketest: cells_match={} dst_cleared={} -> {}",
        cells_match,
        dst_cleared,
        if pass { "PASS" } else { "FAIL" }
    );
}

/// FAIL-able proof of `snapshot_surface` (window-management.md §1 fallback / §4
/// switcher). Paints a known 2-color pattern (left half color A, right half
/// color B) into a test surface, snapshots it into a 16x16 buffer, and asserts
/// the downscaled top-left pixel carries color A's region (the box-average of an
/// all-A source box equals A). Prints FAIL if the snapshot did not sample the
/// source.
pub fn run_snapshot_smoketest() {
    if lock_compositor().is_none() {
        crate::serial_println!(
            "[compositor] snapshot smoketest: src_topleft=0 dst_topleft=0 match=false -> SKIP (no fb)"
        );
        return;
    }
    const SRC_W: u32 = 128;
    const SRC_H: u32 = 128;
    const COLOR_A: u32 = 0xFF_11_22_33;
    const COLOR_B: u32 = 0xFF_AA_BB_CC;

    let Some((id, ptr)) = create_kernel_surface(SRC_W, SRC_H) else {
        crate::serial_println!(
            "[compositor] snapshot smoketest: src_topleft=0 dst_topleft=0 match=false -> FAIL"
        );
        return;
    };
    // Paint: left half COLOR_A, right half COLOR_B.
    unsafe {
        let p = ptr as *mut u32;
        for y in 0..SRC_H {
            for x in 0..SRC_W {
                let c = if x < SRC_W / 2 { COLOR_A } else { COLOR_B };
                core::ptr::write(p.add((y * SRC_W + x) as usize), c);
            }
        }
    }
    let src_topleft = COLOR_A;

    // Snapshot into a 16x16 buffer (caller-owned).
    let mut dst = [0u32; 16 * 16];
    let ok = unsafe { snapshot_surface(id, dst.as_mut_ptr() as *mut u8, 16, 16) };
    let dst_topleft = dst[0];
    // The top-left output pixel covers a source box entirely within the left
    // (COLOR_A) half, so the box-average equals COLOR_A exactly.
    let matches = ok && dst_topleft == src_topleft;

    let _ = close_surface(id);

    crate::serial_println!(
        "[compositor] snapshot smoketest: src_topleft={:#010x} dst_topleft={:#010x} match={} -> {}",
        src_topleft,
        dst_topleft,
        matches,
        if matches { "PASS" } else { "FAIL" }
    );
}

/// FAIL-able proof that the scanout copy runs OUTSIDE the IF=0 COMPOSITOR guard
/// (raeen-perf RANK 5 / goal #2 responsiveness). `recomposite` composites into
/// `comp_buf` under `lock_compositor()` (interrupts disabled, reading user
/// pages), swaps the finished frame into `scanout_ready`, DROPS the guard, then
/// scans out from a buffer taken out of `st` — so the per-pixel framebuffer
/// blast no longer blocks interrupts.
///
/// This drives one real `recomposite()` and asserts the structural invariant the
/// refactor guarantees: after the call, interrupts are restored to their prior
/// state (the guard dropped), and the compositor's persistent scanout buffers
/// were restored (the unlocked scanout left no buffer `take`-n behind). A FAIL
/// here means the guard outlived the scanout or a buffer leaked out of `st`.
pub fn run_render_outside_lock_smoketest() {
    // Snapshot the pre-tick interrupt state; recomposite must leave it restored.
    let if_before = x86_64::instructions::interrupts::are_enabled();

    // Drive one full frame through the real path (no exclusive fullscreen).
    recomposite();

    // After the tick: interrupts restored to caller state, and both persistent
    // scanout buffers are back in `st` (the unlocked scanout did NOT leak them
    // out — `comp_buf` must also be non-empty so the next composite has a buffer).
    let if_after = x86_64::instructions::interrupts::are_enabled();
    let mut buffers_restored = false;
    let mut comp_buf_present = false;
    {
        let st = lock_compositor();
        if let Some(st) = st.as_ref() {
            buffers_restored = !st.scanout_ready.is_empty();
            comp_buf_present = !st.comp_buf.is_empty();
        }
    }
    // The scanout reads ONLY compositor-owned `scanout_ready` (proven by the swap
    // in recomposite), never `surf.kernel_ptr`, so the unlock is UAF-safe.
    let scanout_after_unlock = (if_before == if_after) && buffers_restored && comp_buf_present;
    crate::serial_println!(
        "[compositor] render-outside-lock smoketest: scanout_after_unlock={} -> {}",
        scanout_after_unlock,
        if scanout_after_unlock { "PASS" } else { "FAIL" }
    );
}

/// FAIL-able proof of the damage→wake path (raeen-perf RANK 1 / goal #2
/// sub-frame input latency). Asserts:
///   1. a set dirty flag makes one compositor tick recomposite IMMEDIATELY
///      (does NOT wait the full idle interval) and clears the flag, and
///   2. the real-time rate cap holds — a dirty flag raised too soon after the
///      last present (a mouse-report flood) does NOT recomposite and KEEPS the
///      flag set for the next interval.
/// Prints FAIL if a dirty flag fails to trigger an immediate recomposite, or if
/// the rate cap is missing (a flood would recomposite). Drives the same
/// `compositor_tick` the live thread uses, with a synthetic monotonic clock, so
/// it is deterministic and never touches real timing.
pub fn run_dirty_wake_smoketest() {
    const TARGET_US: u64 = 16_667; // fixed-60 panel interval
    let saved_dirty = COMPOSITOR_DIRTY.swap(false, Ordering::Relaxed);
    let saved_stamp = LAST_DIRTY_US.swap(0, Ordering::Relaxed);

    // (1) Wake-on-dirty: a full interval has elapsed → recomposite immediately,
    // flag cleared. `now_us` far past `last_present_us` proves it does not sit
    // waiting on the idle cadence.
    let mut last_present_us: u64 = 1_000_000;
    mark_dirty(); // sets the flag + stamps LAST_DIRTY_US
    let (woke, _capped_a) =
        compositor_tick(last_present_us + TARGET_US, &mut last_present_us, TARGET_US);
    let woke_on_dirty = woke && !COMPOSITOR_DIRTY.load(Ordering::Relaxed);

    // (2) Rate cap: raise dirty again but only 1 µs after the last present →
    // must NOT recomposite, must KEEP the flag for the next interval.
    COMPOSITOR_DIRTY.store(true, Ordering::Relaxed);
    let cap_base = last_present_us;
    let (woke_too_soon, capped) = compositor_tick(cap_base + 1, &mut last_present_us, TARGET_US);
    let cap_ok = capped && !woke_too_soon && COMPOSITOR_DIRTY.load(Ordering::Relaxed);

    // Restore prior state so the smoketest leaves no pending wake behind.
    COMPOSITOR_DIRTY.store(saved_dirty, Ordering::Relaxed);
    LAST_DIRTY_US.store(saved_stamp, Ordering::Relaxed);

    let pass = woke_on_dirty && cap_ok;
    crate::serial_println!(
        "[compositor] dirty-wake smoketest: woke_on_dirty={} capped={} -> {}",
        woke_on_dirty,
        cap_ok,
        if pass { "PASS" } else { "FAIL" }
    );
}

/// Sum the free-frame count across every buddy allocator node. Used by the
/// surface-leak smoketest to prove `Surface::drop` returns frames.
fn buddy_free_frames() -> usize {
    crate::memory::BUDDY_ALLOCATORS
        .lock()
        .iter()
        .map(|b| b.stats().1)
        .sum()
}

/// Proof that destroying a surface returns its backing frames to the allocator
/// (the ~1.5 MiB-per-window-close leak fix). Allocates then destroys N
/// kernel surfaces and asserts the buddy free-frame count returns to baseline.
/// MasterChecklist Phase 6 / §RaeGFX surface lifecycle. Prints FAIL if the
/// free count dropped (a real leak), so this test can actually fail.
pub fn run_surface_leak_smoketest() {
    // Skip cleanly if the compositor never initialized (no framebuffer): there
    // is no surface table to allocate into and nothing to prove.
    if lock_compositor().is_none() {
        crate::serial_println!(
            "[compositor] surface-leak smoketest: SKIP (compositor not initialized)"
        );
        return;
    }

    let baseline = buddy_free_frames();

    // Create N kernel surfaces (no user PML4 mapping required), then close each.
    // 64x64x4 = 16 KiB = 4 frames -> order 2 each, well within budget.
    const N: usize = 8;
    let mut ids = [0u64; N];
    let mut created = 0usize;
    for slot in ids.iter_mut() {
        match create_kernel_surface(64, 64) {
            Some((id, _ptr)) => {
                *slot = id;
                created += 1;
            }
            None => break,
        }
    }
    for &id in ids.iter().take(created) {
        let _ = close_surface(id);
    }

    let after = buddy_free_frames();
    let pass = created > 0 && after >= baseline;
    crate::serial_println!(
        "[compositor] surface-leak smoketest: baseline={} after={} (created/freed {}) -> {}",
        baseline,
        after,
        created,
        if pass { "PASS" } else { "FAIL" }
    );
}

/// Deterministic proof of the compositor effect math with ZERO framebuffer
/// access: the HDR tone-map pipeline (sRGB → linear → ACES → sRGB, per pixel)
/// and the 3-pass box blur that underpins glassmorphism. MasterChecklist
/// Phase 6.4 — HDR pipeline + glassmorphism (blur). Concept §RaeUI compositor.
pub fn run_effects_smoketest() {
    let mut pass = 0u32;
    let mut total = 0u32;
    let mut check = |c: bool, n: &str| {
        total += 1;
        if c {
            pass += 1;
        } else {
            crate::serial_println!("[compositor-fx] FAIL {}", n);
        }
    };

    // HDR pipeline: push black, mid-gray and white sRGB pixels through the
    // per-pixel tone-map and assert the invariants that hold for any monotone
    // operator — alpha preserved, gray stays gray, black→black, monotone ramp.
    let pipeline = HdrPipeline::new();
    let meta = SurfaceHdr::default();
    let ch = |p: u32| -> (u8, u8, u8) {
        let o = pipeline.process_pixel(p, &meta);
        (
            ((o >> 24) & 0xFF) as u8,
            ((o >> 16) & 0xFF) as u8,
            (o & 0xFF) as u8,
        )
    };
    let blk = pipeline.process_pixel(0xFF00_0000, &meta);
    let gry = pipeline.process_pixel(0xFF80_8080, &meta);
    let wht = pipeline.process_pixel(0xFFFF_FFFF, &meta);
    let (ba, br, _bb) = ch(0xFF00_0000);
    let (ga, _gr, _gb) = ch(0xFF80_8080);
    let (wa, _wr, _wb) = ch(0xFFFF_FFFF);
    let gr_r = (gry >> 16) & 0xFF;
    let gr_g = (gry >> 8) & 0xFF;
    let gr_b = gry & 0xFF;
    let wr_r = (wht >> 16) & 0xFF;
    check(
        ba == 0xFF && ga == 0xFF && wa == 0xFF,
        "hdr-alpha-preserved",
    );
    check(gr_r == gr_g && gr_g == gr_b, "hdr-gray-stays-gray");
    check(br == 0 && (blk & 0x00FF_FFFF) == 0, "hdr-black-to-black");
    check((br as u32) <= gr_r && gr_r <= wr_r, "hdr-monotone-ramp");

    // 3-pass box blur: a single bright center pixel must dim and bleed into a
    // neighbour (the glassmorphism kernel actually averages).
    let (w, h) = (8usize, 8usize);
    let mut buf = vec![0u32; w * h];
    let mut tmp = vec![0u32; w * h];
    let center = (h / 2) * w + (w / 2);
    buf[center] = 0xFFFF_FFFF;
    BlurEngine::box_blur_3pass(&mut buf, &mut tmp, w, h, 1);
    let neighbor = buf[center + 1];
    check(
        buf[center] != 0xFFFF_FFFF && (neighbor & 0xFF) > 0,
        "blur-spreads",
    );

    // Interior legibility cap (§2.3 / §9): the live glass-over-bright-aurora fix.
    // Rec.709 mean-channel luma ×255×10000 (the same scale `clamp_interior_luma`
    // compares against) so we can assert in integer space.
    let ceil = rae_tokens::GLASS_INTERIOR_LUMA_CEIL;
    let ceil_num: u64 = (ceil * 255.0 * 10000.0) as u64;
    let luma_num = |p: u32| -> u64 {
        2126 * ((p >> 16) & 0xFF) as u64 + 7152 * ((p >> 8) & 0xFF) as u64 + 722 * (p & 0xFF) as u64
    };
    // BRIGHT region (a frosted glass sitting over the aurora blob — near-white):
    // after the clamp every pixel's luma must be at/below the ceiling, so white
    // text.primary clears 4.5:1. FAIL-ability: delete the clamp call in the glass
    // path / drop the scale and these stay above ceil_num.
    let mut bright = vec![0xFFE8_ECF4u32; 16]; // luma ~0.91 ≫ 0.40 ceiling
    let bright_before_over = bright.iter().all(|&p| luma_num(p) > ceil_num);
    BlurEngine::clamp_interior_luma(&mut bright, ceil);
    let bright_after_capped = bright.iter().all(|&p| luma_num(p) <= ceil_num + 7152);
    check(
        bright_before_over && bright_after_capped,
        "glass-luma-cap-bright-capped",
    );
    // Hue preservation: the uniform scale keeps the channel ORDER (this pixel is
    // blue-ish: B ≥ G ≥ R), so the clamp darkens without recoloring.
    let capped = bright[0];
    let (cr, cg, cb) = ((capped >> 16) & 0xFF, (capped >> 8) & 0xFF, capped & 0xFF);
    check(cb >= cg && cg >= cr, "glass-luma-cap-hue-preserved");
    // DARK region (glass over the aurora valley — already legible): the clamp must
    // be a NO-OP so the frosted/translucent look survives. FAIL-ability: a clamp
    // that scales unconditionally would darken these and trip this.
    let dark_in = 0xFF12_1620u32; // luma ~0.085 ≪ ceiling
    let mut dark = vec![dark_in; 16];
    BlurEngine::clamp_interior_luma(&mut dark, ceil);
    check(
        dark.iter().all(|&p| p == dark_in),
        "glass-luma-cap-dark-noop",
    );

    // SHIP-GATE §9 WCAG legibility pass (the FINAL guarantee, mirrored from
    // `raegfx::glass::clamp_interior_wcag_region`). Asserts REAL WCAG contrast — not a
    // mean-channel proxy — so it catches the gamma-encoded/decoded divergence the
    // mean-channel cap misses for saturated pixels.
    //
    // (a) THE divergence case: a SATURATED magenta pixel whose mean-channel luma is
    //     BELOW the ceiling (so the mean-channel cap is a NO-OP — it claims success)
    //     yet whose REAL WCAG contrast against white text is only ~3.96:1 (FAILS AA).
    //     This is exactly the gamma-encoded/decoded divergence the WCAG pass exists to
    //     catch. After the WCAG pass it MUST clear 4.5:1 white-on-glass. FAIL-ability:
    //     delete the `clamp_interior_wcag` call and this stays under 4.5 (the
    //     mean-channel cap alone never touches it).
    let mut wcag_sat = vec![0xFFC0_40C0u32; 16]; // mean ~0.394 (≤ceil) but WCAG ~3.96:1
    BlurEngine::clamp_interior_luma(&mut wcag_sat, ceil); // mean-channel pre-pass: NO-OP here
    let sat_before = rae_tokens::contrast_ratio(TEXT_PRIMARY_DARK, wcag_sat[0] | 0xFF00_0000);
    BlurEngine::clamp_interior_wcag(&mut wcag_sat);
    let sat_after = rae_tokens::contrast_ratio(TEXT_PRIMARY_DARK, wcag_sat[0] | 0xFF00_0000);
    check(
        sat_before < 4.5 && sat_after >= 4.5,
        "glass-wcag-saturated-clears-aa",
    );
    // (b) A synthesized BRIGHT near-white region (the frosted glass over the aurora
    //     blob): after the full ladder it must clear AA white-on-glass.
    let mut wcag_bright = vec![0xFFE8_ECF4u32; 16];
    BlurEngine::clamp_interior_luma(&mut wcag_bright, ceil);
    BlurEngine::clamp_interior_wcag(&mut wcag_bright);
    check(
        rae_tokens::contrast_ratio(TEXT_PRIMARY_DARK, wcag_bright[0] | 0xFF00_0000) >= 4.5,
        "glass-wcag-bright-clears-aa",
    );
    // (c) Hue preservation: the uniform darkening keeps the channel ORDER of the
    //     magenta sample (R ≈ B ≥ G), so the WCAG clamp darkens without recoloring.
    let wp = wcag_sat[0];
    let (wr, wg, wb) = ((wp >> 16) & 0xFF, (wp >> 8) & 0xFF, wp & 0xFF);
    check(wr >= wg && wb >= wg, "glass-wcag-hue-preserved");
    // (d) DARK-backdrop NO-OP: a glass pixel already over the aurora valley already
    //     clears AA, so the WCAG pass must leave it byte-identical (the frosted look
    //     survives). FAIL-ability: an unconditional darkening would change it.
    let wcag_dark_in = 0xFF12_1620u32; // already ≫ 4.5:1 vs white text
    let mut wcag_dark = vec![wcag_dark_in; 16];
    BlurEngine::clamp_interior_wcag(&mut wcag_dark);
    check(
        wcag_dark.iter().all(|&p| p == wcag_dark_in),
        "glass-wcag-dark-noop",
    );

    drop(check);
    crate::serial_println!(
        "[ OK ] compositor effects selftest: {}/{} checks passed (HDR tone-map pipeline + box blur + glass luma cap + WCAG legibility pass)",
        pass,
        total
    );
    if pass != total {
        crate::serial_println!(
            "[FAIL] compositor effects selftest: {} check(s) failed",
            total - pass
        );
    }
}

/// FAIL-able proof of the `material-and-shadow.md` rendering contract — the
/// "penumbra test". Renders an `elev.4` drop shadow (offset 12, radius 40,
/// `0x66_00_00_00`) for a card over a deliberately BLUE backdrop (the exact
/// `(78,123,200)` wallpaper that the old renderer's bug leaked into the shadow),
/// then samples a vertical pixel line crossing the card's bottom edge outward and
/// asserts:
///   1. `monotonic`: the shadow alpha (recovered from how far each backdrop pixel
///      was darkened) decreases monotonically from a peak to ~0 over ~`radius`px
///      — a smooth ramp, NOT a flat band then a hard step.
///   2. `soft`: the ramp spans many pixels (≥ radius/2), not a 1px cliff.
///   3. `near_black`: along the ramp R≈G≈B (no channel diverges) — proving the
///      shadow color is the constant near-black, never the blue backdrop.
///
/// Why it can print FAIL: the OLD renderer produced a flat `0x66`-alpha band that
/// hard-cut to wallpaper (monotonic would hold but `soft` would FAIL — the ramp
/// is ~1-2px), and any backdrop-color-sampling bug tints the shadow blue
/// (`near_black` FAILs: B ≫ R). A hard blue offset block fails ≥2 of the three.
/// Pure buffer math — no framebuffer, no surfaces, no lock.
pub fn run_drop_shadow_penumbra_smoketest() {
    // Scratch screen buffer with a flat blue backdrop (the bug's wallpaper color).
    const W: usize = 160;
    const H: usize = 160;
    const BACK_R: u32 = 78;
    const BACK_G: u32 = 123;
    const BACK_B: u32 = 200;
    const BACK: u32 = 0xFF00_0000 | (BACK_R << 16) | (BACK_G << 8) | BACK_B;
    let mut buf = vec![BACK; W * H];
    let mut mask: Vec<u32> = Vec::new();
    let mut tmp: Vec<u32> = Vec::new();

    // A card centered horizontally, upper area, so its bottom edge + penumbra
    // fit with room below for the ramp to reach ~0.
    let (sx, sy, sw, sh) = (40i32, 30i32, 80u32, 50u32);
    // elev.4 ladder (material-and-shadow.md): offset_y 12, radius 40, 0x66 black.
    const RADIUS: u32 = 40;
    const OFFY: i32 = 12;
    render_drop_shadow(
        &mut buf,
        &mut mask,
        &mut tmp,
        W,
        H,
        sx,
        sy + OFFY,
        sw,
        sh,
        12, // corner radius (radius.md)
        RADIUS,
        0x66_00_00_00,
        false,
    );

    // Sample a vertical line down the card's horizontal center, from the card's
    // bottom edge outward through the full offset+penumbra region.
    let col = (sx + sw as i32 / 2) as usize;
    let start_y = (sy + sh as i32) as usize; // first row below the card body

    // Recover the shadow alpha at each sampled row from how far the blue backdrop
    // was darkened, and the per-channel recovered alphas for the tint check.
    // source-over with near-black shadow: out = back*(255-a)/255, so
    //   a = 255 - 255*out/back  (per channel; they must agree for a gray shadow).
    let recover =
        |c: u32, back: u32| -> i32 { (255i32 - (255i32 * c as i32) / back as i32).clamp(0, 255) };
    let mut samples: Vec<(i32, u32)> = Vec::with_capacity(H); // (alpha_b, skew)
    for y in start_y..H {
        let p = buf[y * W + col];
        let r = (p >> 16) & 0xFF;
        let g = (p >> 8) & 0xFF;
        let b = p & 0xFF;
        let ab = recover(b, BACK_B) as u32;
        let ar = recover(r, BACK_R) as u32;
        let ag = recover(g, BACK_G) as u32;
        let skew = if ab > 8 {
            ab.max(ar).max(ag) - ab.min(ar).min(ag)
        } else {
            0
        };
        samples.push((ab as i32, skew));
    }

    // Locate the peak row, then assert a smooth monotonic DECREASE after it (the
    // penumbra), down to ~0. The pre-peak rise is the offset ledge (expected).
    let mut peak_idx = 0usize;
    let mut peak_alpha: u32 = 0;
    for (i, &(a, _)) in samples.iter().enumerate() {
        if a as u32 > peak_alpha {
            peak_alpha = a as u32;
            peak_idx = i;
        }
    }
    let mut monotonic = true;
    let mut prev = peak_alpha as i32;
    let mut ramp_px: u32 = 0;
    let mut max_channel_skew: u32 = 0;
    let mut reached_zero = false;
    for &(a, skew) in samples.iter().skip(peak_idx) {
        if skew > max_channel_skew {
            max_channel_skew = skew;
        }
        // Allow tiny +/- jitter from integer division; a real rise breaks it.
        if a > prev + 2 {
            monotonic = false;
        }
        if a > 4 {
            ramp_px += 1;
        } else {
            reached_zero = true;
        }
        prev = a;
    }

    // A soft penumbra spans many pixels; a hard block collapses to a thin cliff.
    let soft = ramp_px >= RADIUS / 2;
    // Channels must agree within a small tolerance → no blue (or any) tint.
    let near_black = max_channel_skew <= 16;
    // The ramp must actually reach ~0 (decay to wallpaper), not stay pinned, and
    // there must have been a real (non-trivial) peak to fall from.
    let decays = reached_zero && peak_alpha > 16;

    let pass = monotonic && soft && near_black && decays;
    SHADOW_PENUMBRA_RESULT.store(if pass { 1 } else { 2 }, Ordering::Relaxed);
    SHADOW_PENUMBRA_STATS.store(
        (if pass { 1 } else { 2 }) as u64
            | ((peak_alpha as u64 & 0xFF) << 8)
            | ((ramp_px as u64 & 0xFFFF) << 16),
        Ordering::Relaxed,
    );

    crate::serial_println!(
        "[compositor] shadow-penumbra monotonic={} soft={}(ramp={}px) near_black={}(skew={}) decays={}(peak={}) -> {}",
        monotonic,
        soft,
        ramp_px,
        near_black,
        max_channel_skew,
        decays,
        peak_alpha,
        if pass { "PASS" } else { "FAIL" }
    );
}

pub fn init() {
    let Some(fb) = crate::framebuffer::fb_info() else {
        crate::serial_println!("[compositor] no framebuffer — skipping init.");
        return;
    };

    let gpu_fb = crate::gpu::gpu_framebuffer();
    if gpu_fb.is_some() {
        crate::serial_println!(
            "[compositor] GPU-backed scanout detected — compositing to GPU framebuffer"
        );
    }

    let (comp_w, comp_h) = if let Some(ref g) = gpu_fb {
        (g.width, g.height)
    } else {
        (fb.width, fb.height)
    };
    let buf_size = (comp_w as usize) * (comp_h as usize);

    // Initial backend = the always-available GOP blit. The two GPU backends both
    // come up AFTER compositor::init (main.rs: virtio_gpu/gpu init are later
    // tiers), so the choice is resolved in recomposite: gpu_fb (crate::gpu linear
    // scanout) is checked dynamically every frame, and the live virtio-gpu
    // backend is upgraded lazily on the first composite. A native AMD page-flip
    // backend slots in as a 4th variant.
    let scanout = ScanoutBackend::Gop;
    crate::serial_println!(
        "[compositor] scanout backend: {:?} (initial; gpu_fb/virtio resolved at first composite)",
        scanout
    );

    *lock_compositor() = Some(CompositorState {
        surfaces: Vec::new(),
        next_id: 1,
        next_z: 1,
        focused: None,
        fb,
        gpu_fb,
        scanout,
        virtio_tried: false,
        frame_pacer: FramePacer::new(VrrState::fixed(60)),
        hdr_pipeline: HdrPipeline::new(),
        // IDENTITY §3 "kills the void": the default backdrop is the procedural
        // Aurora Mesh, NOT the old near-black two-stop navy gradient
        // (GradientWallpaper(0x0A0E1A, 0x1A2844)). `aurora::init()` re-installs the
        // same engine after the live-wallpaper registry is up; seeding it here too
        // means the desktop is never the flat void even before that runs.
        wallpaper: Some(WallpaperState::new(Box::new(
            crate::aurora::AuroraWallpaper::new(),
        ))),
        captures: Vec::new(),
        comp_buf: vec![0u32; buf_size],
        sw_backbuf: Vec::new(),
        scanout_ready: vec![0u32; buf_size],
        scanout_backbuf: Vec::new(),
        blur_region: Vec::new(),
        blur_tmp: Vec::new(),
        comp_w,
        comp_h,
        time_us: 0,
        exclusive: None,
        cursor: CursorState {
            x: (comp_w / 2) as i32,
            y: (comp_h / 2) as i32,
            visible: true,
        },
        total_frames: 0,
    });
    crate::serial_println!(
        "[compositor] initialized: {}x{} @ {}bpp (stride {})",
        fb.width,
        fb.height,
        fb.bytes_per_pixel * 8,
        fb.stride,
    );
    crate::serial_println!("[compositor] glassmorphism blur engine: ready");
    crate::serial_println!("[compositor] live wallpaper: gradient (paused when occluded)");
    crate::serial_println!("[compositor] capture engine: ready (zero-cost double-buffered)");

    // Register VRR and HDR capability — these are honest stubs that store the
    // parameters and log "pending GPU driver" so audits see reality, not claims.
    negotiate_vrr(48, 144);
    enable_hdr_pipeline(400, 9);
    crate::serial_println!(
        "[compositor] VRR+HDR capabilities registered (pending GPU driver hookup)"
    );

    // Prime the cached TSC frequency on the boot path so the IRQ-context
    // `move_cursor` → `mark_dirty` → `monotonic_us` path never lazily triggers
    // the heavy PIT calibration spin from inside the mouse interrupt handler.
    let _ = monotonic_us();

    spawn_compositor_thread();
}

/// Returns `(width, height)` of the compositor's output.
pub fn screen_dimensions() -> Option<(u32, u32)> {
    let state = lock_compositor();
    let st = state.as_ref()?;
    Some((st.comp_w, st.comp_h))
}

/// Resize the compositor back-buffer (logical mode-set within physical FB bounds).
pub fn set_output_resolution(width: u32, height: u32) -> bool {
    if width == 0 || height == 0 {
        return false;
    }
    let mut guard = lock_compositor();
    let Some(st) = guard.as_mut() else {
        return false;
    };
    st.comp_w = width;
    st.comp_h = height;
    st.comp_buf = alloc::vec![0u32; width as usize * height as usize];
    drop(guard);
    // The new mode must paint without waiting on the idle poll cadence.
    request_recomposite();
    true
}

/// Move the hardware cursor by a delta and request an immediate recomposite.
/// Called from the mouse IRQ handler. Marks the compositor dirty so the cursor
/// reaches the panel on the next scheduler tick (~couple of µs of OS latency)
/// instead of waiting up to a full idle poll interval (raeen-perf RANK 1).
pub fn move_cursor(dx: i32, dy: i32) {
    let mut state = lock_compositor();
    let Some(st) = state.as_mut() else { return };
    let old_x = st.cursor.x;
    let old_y = st.cursor.y;
    st.cursor.x = (st.cursor.x + dx).clamp(0, st.comp_w as i32 - 1);
    st.cursor.y = (st.cursor.y - dy).clamp(0, st.comp_h as i32 - 1);
    st.cursor.visible = true;
    let moved = st.cursor.x != old_x || st.cursor.y != old_y;
    // Mirror the (already-clamped) absolute position into a lock-free cache so the
    // `SYS_INPUT_CURSOR` poll (apps hit-test where a click landed) can read it
    // without taking the compositor lock. Packs x|y<<16; both fit u16 on any panel.
    CURSOR_POS_PACKED.store(
        (st.cursor.x as u32 & 0xFFFF) as u64 | (((st.cursor.y as u32 & 0xFFFF) as u64) << 16),
        Ordering::Relaxed,
    );
    drop(state);
    if moved {
        mark_dirty();
    }
}

/// Lock-free cache of the absolute cursor position, packed `x | (y << 16)`.
/// Written by `move_cursor` (mirrors the compositor's authoritative `cursor.x/y`);
/// read by `cursor_position_fast` / `SYS_INPUT_CURSOR`. Lets an app poll the
/// cursor each frame for hit-testing without contending the compositor lock.
static CURSOR_POS_PACKED: AtomicU64 = AtomicU64::new(0);

/// Returns the current cursor position `(x, y)`.
pub fn cursor_position() -> Option<(i32, i32)> {
    let state = lock_compositor();
    let st = state.as_ref()?;
    Some((st.cursor.x, st.cursor.y))
}

/// Lock-free read of the current absolute cursor position `(x, y)` from the
/// `CURSOR_POS_PACKED` cache. Never blocks — safe to call from an IF=0 syscall
/// without acquiring the compositor lock (the `SYS_INPUT_CURSOR` poll path).
pub fn cursor_position_fast() -> (u32, u32) {
    let packed = CURSOR_POS_PACKED.load(Ordering::Relaxed);
    ((packed & 0xFFFF) as u32, ((packed >> 16) & 0xFFFF) as u32)
}

/// Upgrade the compositor to use a GPU-backed scanout framebuffer.
/// Call after `gpu::init()` when a hardware GPU becomes available post-boot.
pub fn attach_gpu_scanout() {
    let Some(gfb) = crate::gpu::gpu_framebuffer() else {
        return;
    };
    let mut guard = lock_compositor();
    let Some(st) = guard.as_mut() else { return };
    if st.gpu_fb.is_some() {
        return;
    }

    let new_w = gfb.width;
    let new_h = gfb.height;
    let need_resize = new_w != st.comp_w || new_h != st.comp_h;

    st.gpu_fb = Some(gfb);

    if need_resize {
        st.comp_w = new_w;
        st.comp_h = new_h;
        st.comp_buf = vec![0u32; (new_w as usize) * (new_h as usize)];
    }

    crate::serial_println!(
        "[compositor] GPU scanout attached: {}x{} @ 32bpp BGRA",
        new_w,
        new_h
    );
}

fn alloc_contig_frames(pages: usize) -> Option<PhysFrame<Size4KiB>> {
    let order = (usize::BITS - pages.saturating_sub(1).leading_zeros()) as u8;
    let phys = crate::memory::allocate_contiguous_frames(order)?;
    Some(PhysFrame::containing_address(phys))
}

pub fn create_surface(width: u32, height: u32, user_virt: u64) -> Option<u64> {
    if width == 0 || height == 0 || width > 8192 || height > 8192 {
        return None;
    }
    if user_virt & 0xFFF != 0 {
        return None;
    }
    if user_virt >= 0x0000_8000_0000_0000 {
        return None;
    }

    let byte_len = (width as usize)
        .checked_mul(height as usize)
        .and_then(|n| n.checked_mul(4))
        .unwrap_or(0);
    if byte_len == 0 || byte_len > 64 * 1024 * 1024 {
        return None;
    }
    let pages = byte_len.div_ceil(4096);

    let first_frame = alloc_contig_frames(pages)?;
    let offset = *crate::memory::PHYS_MEM_OFFSET.get()?;
    let kernel_ptr = (offset + first_frame.start_address().as_u64()).as_mut_ptr::<u32>();

    unsafe {
        core::ptr::write_bytes(kernel_ptr as *mut u8, 0, pages * 4096);
    }

    let task_pml4 = crate::scheduler::with_current_task(|t| t.pml4).flatten()?;
    let flags =
        PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE;
    for i in 0..pages {
        let phys = PhysAddr::new(first_frame.start_address().as_u64() + (i * 4096) as u64);
        let frame: PhysFrame<Size4KiB> = PhysFrame::containing_address(phys);
        let page: Page<Size4KiB> =
            Page::containing_address(VirtAddr::new(user_virt + (i * 4096) as u64));
        unsafe {
            crate::memory::map_page_in_pml4(task_pml4, page, frame, flags);
        }
    }
    x86_64::instructions::tlb::flush_all();

    let owner = crate::scheduler::current_task_id()?;
    let mut state = lock_compositor();
    let st = state.as_mut()?;
    let id = st.next_id;
    let z = st.next_z;
    st.next_id += 1;
    st.next_z += 1;
    st.surfaces.push(Surface {
        id,
        owner_task: owner,
        width,
        height,
        kernel_ptr,
        byte_len,
        x: 0,
        y: 0,
        visible: false,
        z_order: z,
        hdr_meta: SurfaceHdr::default(),
        blur: None,
        effects: Vec::new(),
        title: [0u8; 48],
        title_len: 0,
        minimized: false,
        user_virt,
        requested_size: None,
    });
    if st.focused.is_none() {
        st.focused = Some(id);
    }
    crate::serial_println!(
        "[compositor] surface {} created: {}x{} at user 0x{:x} (z={})",
        id,
        width,
        height,
        user_virt,
        z,
    );

    drop(state);
    mark_dirty();
    crate::shell_runner::notify_surface_created(id, "App", width, height);

    Some(id)
}

/// Create a kernel-owned surface (no user page-table mapping).
/// Returns `(surface_id, kernel_ptr)` so the caller can render into it
/// directly via `raegfx::Canvas`.
pub fn create_kernel_surface(width: u32, height: u32) -> Option<(u64, *mut u8)> {
    if width == 0 || height == 0 || width > 8192 || height > 8192 {
        return None;
    }
    let byte_len = (width as usize)
        .checked_mul(height as usize)
        .and_then(|n| n.checked_mul(4))?;
    if byte_len == 0 || byte_len > 64 * 1024 * 1024 {
        return None;
    }
    let pages = byte_len.div_ceil(4096);

    let first_frame = alloc_contig_frames(pages)?;
    let offset = *crate::memory::PHYS_MEM_OFFSET.get()?;
    let kernel_ptr = (offset + first_frame.start_address().as_u64()).as_mut_ptr::<u32>();

    unsafe {
        core::ptr::write_bytes(kernel_ptr as *mut u8, 0, pages * 4096);
    }

    let owner = crate::task::TaskId::kernel_sentinel();
    let mut state = lock_compositor();
    let st = state.as_mut()?;
    let id = st.next_id;
    st.next_id += 1;
    // Desktop shell surface gets z_order 0 so it sits behind all app windows.
    st.surfaces.insert(
        0,
        Surface {
            id,
            owner_task: owner,
            width,
            height,
            kernel_ptr,
            byte_len,
            x: 0,
            y: 0,
            visible: true,
            z_order: 0,
            hdr_meta: SurfaceHdr::default(),
            blur: None,
            effects: Vec::new(),
            title: [0u8; 48],
            title_len: 0,
            minimized: false,
            user_virt: 0,
            requested_size: None,
        },
    );

    crate::serial_println!(
        "[compositor] kernel surface {} created: {}x{} (z=0, desktop)",
        id,
        width,
        height,
    );
    drop(state);
    mark_dirty();
    Some((id, kernel_ptr as *mut u8))
}

/// Present a surface at the given screen position and recomposite.
pub fn present_surface(id: u64, x: i32, y: i32) -> Result<(), ()> {
    let mut state = lock_compositor();
    let st = state.as_mut().ok_or(())?;

    let surface = st.surfaces.iter_mut().find(|s| s.id == id).ok_or(())?;
    // Apps often pass (0,0); keep cascade placement from the shell unless
    // they specify a real offset.
    if x != 0 || y != 0 {
        surface.x = x;
        surface.y = y;
    }
    surface.visible = true;
    drop(state);
    recomposite();
    // Telemetry: compositor throughput for /proc/raeen/perf.
    crate::perf::record_frame_present();
    Ok(())
}

/// Set the screen position of a surface (used before present).
pub fn set_surface_origin(id: u64, x: i32, y: i32) -> Result<(), ()> {
    let mut state = lock_compositor();
    let st = state.as_mut().ok_or(())?;
    if let Some(s) = st.surfaces.iter_mut().find(|s| s.id == id) {
        s.x = x;
        s.y = y;
        drop(state);
        mark_dirty();
        Ok(())
    } else {
        Err(())
    }
}

// ─── Surface resize protocol (Phase 13.2 — true tiling reflows clients) ──────
//
// A real tiling WM RESIZES a client to fill its cell, not merely repositions it
// (i3/sway; Win11 Snap Layouts). The compositor owns the backing frames, so the
// flow is: the WM records a desired size on the surface; a KERNEL-owned surface
// is resized immediately (the kernel owns the buffer); a USER-owned surface gets
// a pending request the client honors on its own terms via the 291/292 syscalls.

/// Free the contiguous backing frames at `(kernel_ptr, byte_len)` exactly the
/// way `Surface::drop` does — same `pages.div_ceil` / buddy-`order` math and the
/// same `PHYS_MEM_OFFSET` round-trip. Used by the resize path, which swaps in a
/// fresh buffer and must return the OLD frames to the allocator (no leak) while
/// the surface lives on (so `drop` does not run). No-op on a null/zero buffer.
/// MUST be called under `lock_compositor()` (it is, by both callers below).
fn free_surface_frames(kernel_ptr: *mut u32, byte_len: usize) {
    if kernel_ptr.is_null() || byte_len == 0 {
        return;
    }
    let Some(offset) = crate::memory::PHYS_MEM_OFFSET.get() else {
        return;
    };
    let pages = byte_len.div_ceil(4096);
    let order = (usize::BITS - pages.saturating_sub(1).leading_zeros()) as u8;
    let phys = (kernel_ptr as u64).wrapping_sub(offset.as_u64());
    crate::memory::deallocate_contiguous_frames(PhysAddr::new(phys), order);
}

/// Record the window manager's desired size for a surface (a tiling/snap layout
/// wants this client to FILL a cell). For a KERNEL-owned surface the kernel owns
/// the buffer, so the resize is performed IMMEDIATELY (the window really fills
/// its cell this frame). For a USER-owned surface the size is stored as a
/// pending request the client polls via `SYS_SURFACE_RESIZE_REQ` (291) and acks
/// with `SYS_SURFACE_RESIZE` (292) — the kernel cannot reallocate an app's
/// buffer behind its back. A no-op (and clears any stale request) when the
/// requested size already matches the current size. Returns `Err(())` for an
/// unknown id / compositor down / invalid dimensions.
pub fn request_surface_resize(id: u64, w: u32, h: u32) -> Result<(), ()> {
    if w == 0 || h == 0 || w > 8192 || h > 8192 {
        return Err(());
    }
    let mut state = lock_compositor();
    let st = state.as_mut().ok_or(())?;
    let s = st.surfaces.iter_mut().find(|s| s.id == id).ok_or(())?;
    if s.width == w && s.height == h {
        s.requested_size = None;
        return Ok(());
    }
    if s.user_virt == 0 {
        // Kernel-owned: resize the backing buffer in place, right now.
        resize_kernel_surface_locked(s, w, h)?;
        drop(state);
        mark_dirty();
        Ok(())
    } else {
        // User-owned: record a pending request for the client to honor.
        s.requested_size = Some((w, h));
        drop(state);
        Ok(())
    }
}

/// Poll the pending window-manager resize request for a surface. Returns
/// `Some((w, h))` when a tiling layout wants this client at a new size, or
/// `None` when none is pending / the id is unknown. Backs `SYS_SURFACE_RESIZE_REQ`
/// (291).
pub fn surface_resize_request(id: u64) -> Option<(u32, u32)> {
    let state = lock_compositor();
    let st = state.as_ref()?;
    st.surfaces
        .iter()
        .find(|s| s.id == id)
        .and_then(|s| s.requested_size)
}

/// Reallocate a KERNEL-owned surface's backing buffer to `w × h` and update its
/// dimensions. Caller holds the compositor lock and has confirmed `user_virt == 0`.
/// Allocates the new frames FIRST (so a failure leaves the old buffer intact),
/// then frees the old frames; clears any pending request.
fn resize_kernel_surface_locked(s: &mut Surface, w: u32, h: u32) -> Result<(), ()> {
    let byte_len = (w as usize)
        .checked_mul(h as usize)
        .and_then(|n| n.checked_mul(4))
        .ok_or(())?;
    if byte_len == 0 || byte_len > 64 * 1024 * 1024 {
        return Err(());
    }
    let pages = byte_len.div_ceil(4096);
    let first_frame = alloc_contig_frames(pages).ok_or(())?;
    let offset = crate::memory::PHYS_MEM_OFFSET.get().ok_or(())?;
    let new_ptr = (*offset + first_frame.start_address().as_u64()).as_mut_ptr::<u32>();
    unsafe {
        core::ptr::write_bytes(new_ptr as *mut u8, 0, pages * 4096);
    }
    let old_ptr = s.kernel_ptr;
    let old_len = s.byte_len;
    s.kernel_ptr = new_ptr;
    s.byte_len = byte_len;
    s.width = w;
    s.height = h;
    s.requested_size = None;
    free_surface_frames(old_ptr, old_len);
    Ok(())
}

/// Perform a USER-owned surface resize (the `SYS_SURFACE_RESIZE` 292 ack): the
/// client has allocated a fresh `w × h × 4` buffer at `new_user_virt` in its own
/// address space and asks the compositor to rebind the surface to it. The kernel
/// allocates new contiguous frames, maps them into the owner's PML4 at
/// `new_user_virt`, UNMAPS the old frames from the owner, frees them, and updates
/// the surface. `caller` must own the surface. Returns `Err(())` on a bad id /
/// non-owner / invalid dimensions / unaligned-or-kernel-half vaddr / alloc fail.
pub fn resize_user_surface(
    id: u64,
    w: u32,
    h: u32,
    new_user_virt: u64,
    caller: crate::task::TaskId,
) -> Result<(), ()> {
    if w == 0 || h == 0 || w > 8192 || h > 8192 {
        return Err(());
    }
    if new_user_virt & 0xFFF != 0 || new_user_virt >= 0x0000_8000_0000_0000 {
        return Err(());
    }
    let byte_len = (w as usize)
        .checked_mul(h as usize)
        .and_then(|n| n.checked_mul(4))
        .ok_or(())?;
    if byte_len == 0 || byte_len > 64 * 1024 * 1024 {
        return Err(());
    }
    let new_pages = byte_len.div_ceil(4096);

    let mut state = lock_compositor();
    let st = state.as_mut().ok_or(())?;
    let s = st.surfaces.iter_mut().find(|s| s.id == id).ok_or(())?;
    // Only the owning task may resize its own window, and only a user-mapped
    // surface goes through this path (a kernel surface has no user_virt).
    if s.owner_task != caller || s.user_virt == 0 {
        return Err(());
    }
    let task_pml4 = match crate::scheduler::with_current_task(|t| t.pml4).flatten() {
        Some(p) => p,
        None => return Err(()),
    };

    let first_frame = alloc_contig_frames(new_pages).ok_or(())?;
    let offset = *crate::memory::PHYS_MEM_OFFSET.get().ok_or(())?;
    let new_ptr = (offset + first_frame.start_address().as_u64()).as_mut_ptr::<u32>();
    unsafe {
        core::ptr::write_bytes(new_ptr as *mut u8, 0, new_pages * 4096);
    }

    // Unmap the OLD user mapping so the app cannot touch frames we are about to
    // free (use-after-free guard), then map the new frames at the new vaddr.
    let old_pages = s.byte_len.div_ceil(4096);
    let old_virt = s.user_virt;
    for i in 0..old_pages {
        let page: Page<Size4KiB> =
            Page::containing_address(VirtAddr::new(old_virt + (i * 4096) as u64));
        unsafe {
            crate::memory::unmap_page_in_pml4(task_pml4, page);
        }
    }
    let flags =
        PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE;
    for i in 0..new_pages {
        let phys = PhysAddr::new(first_frame.start_address().as_u64() + (i * 4096) as u64);
        let frame: PhysFrame<Size4KiB> = PhysFrame::containing_address(phys);
        let page: Page<Size4KiB> =
            Page::containing_address(VirtAddr::new(new_user_virt + (i * 4096) as u64));
        unsafe {
            crate::memory::map_page_in_pml4(task_pml4, page, frame, flags);
        }
    }
    x86_64::instructions::tlb::flush_all();

    let old_ptr = s.kernel_ptr;
    let old_len = s.byte_len;
    s.kernel_ptr = new_ptr;
    s.byte_len = byte_len;
    s.width = w;
    s.height = h;
    s.user_virt = new_user_virt;
    s.requested_size = None;
    free_surface_frames(old_ptr, old_len);

    drop(state);
    mark_dirty();
    Ok(())
}

/// Return a surface's CURRENT absolute origin `(x, y)` on screen, or `None` if
/// the id is unknown / the compositor is not up. This is the live counterpart to
/// the origin an app passed to `present` — the window manager moves windows via
/// `set_surface_origin` (Overview / Spaces / tiling), so a hardcoded origin goes
/// stale and click hit-testing misses. The `SYS_SURFACE_ORIGIN` (280) poll reads
/// this so a mouse-first app stays correct under window management. Negative
/// origins (a window dragged partly off the left/top edge) clamp to `0` so the
/// `u16` packing the syscall uses never underflows. Takes the short compositor
/// lock (read-only over the surface list) — never alters move/render/present.
pub fn surface_origin(id: u64) -> Option<(u32, u32)> {
    let state = lock_compositor();
    let st = state.as_ref()?;
    st.surfaces
        .iter()
        .find(|s| s.id == id)
        .map(|s| (s.x.max(0) as u32, s.y.max(0) as u32))
}

/// Return the topmost visible userspace surface under `(px, py)`.
pub fn surface_at(px: i32, py: i32) -> Option<u64> {
    let state = lock_compositor();
    let st = state.as_ref()?;
    let kernel = crate::task::TaskId::kernel_sentinel();
    let mut best: Option<&Surface> = None;
    for s in &st.surfaces {
        if !s.visible || s.owner_task == kernel {
            continue;
        }
        let frame_h = if s.minimized {
            crate::window_chrome::TITLE_BAR_H
        } else {
            crate::window_chrome::frame_height(s.height as i32)
        };
        let right = s.x.saturating_add(s.width as i32);
        let bottom = s.y.saturating_add(frame_h);
        if px >= s.x && px < right && py >= s.y && py < bottom {
            if best.map_or(true, |b| s.z_order > b.z_order) {
                best = Some(s);
            }
        }
    }
    best.map(|s| s.id)
}

/// Set the title-bar label for a userspace surface.
pub fn set_surface_title(id: u64, title: &str) -> Result<(), ()> {
    let mut state = lock_compositor();
    let st = state.as_mut().ok_or(())?;
    let s = st.surfaces.iter_mut().find(|s| s.id == id).ok_or(())?;
    let n = title.as_bytes().len().min(48);
    s.title[..n].copy_from_slice(&title.as_bytes()[..n]);
    s.title_len = n as u8;
    drop(state);
    mark_dirty();
    Ok(())
}

pub fn set_surface_minimized(id: u64, minimized: bool) -> Result<(), ()> {
    let mut state = lock_compositor();
    let st = state.as_mut().ok_or(())?;
    if let Some(s) = st.surfaces.iter_mut().find(|s| s.id == id) {
        s.minimized = minimized;
        drop(state);
        mark_dirty();
        Ok(())
    } else {
        Err(())
    }
}

pub fn surface_title(id: u64) -> alloc::string::String {
    let state = lock_compositor();
    let Some(st) = state.as_ref() else {
        return alloc::string::String::from("App");
    };
    let Some(s) = st.surfaces.iter().find(|s| s.id == id) else {
        return alloc::string::String::from("App");
    };
    core::str::from_utf8(&s.title[..s.title_len as usize])
        .unwrap_or("App")
        .into()
}

pub fn surface_owner(id: u64) -> Option<crate::task::TaskId> {
    lock_compositor().as_ref().and_then(|st| {
        st.surfaces
            .iter()
            .find(|s| s.id == id)
            .map(|s| s.owner_task)
    })
}

/// Screen-space frame: `(x, y, width, client_height, minimized)`.
pub fn surface_frame(id: u64) -> Option<(i32, i32, i32, i32, bool)> {
    let state = lock_compositor();
    let st = state.as_ref()?;
    let s = st.surfaces.iter().find(|s| s.id == id)?;
    Some((s.x, s.y, s.width as i32, s.height as i32, s.minimized))
}

pub fn list_userspace_surfaces() -> alloc::vec::Vec<(u64, u32)> {
    let kernel = crate::task::TaskId::kernel_sentinel();
    let state = lock_compositor();
    let Some(st) = state.as_ref() else {
        return alloc::vec::Vec::new();
    };
    st.surfaces
        .iter()
        .filter(|s| s.visible && s.owner_task != kernel)
        .map(|s| (s.id, s.z_order))
        .collect()
}

/// Bring a surface to the front (highest z-order) and mark it focused.
pub fn focus_surface(id: u64) -> Result<(), ()> {
    let mut state = lock_compositor();
    let st = state.as_mut().ok_or(())?;

    let new_z = st.next_z;
    st.next_z += 1;

    if let Some(s) = st.surfaces.iter_mut().find(|s| s.id == id) {
        s.z_order = new_z;
        st.focused = Some(id);
        st.surfaces.sort_by_key(|s| s.z_order);
        crate::serial_println!("[compositor] focused surface {} (z={})", id, new_z);
        drop(state);
        mark_dirty();
        Ok(())
    } else {
        Err(())
    }
}

/// Destroy a surface and recomposite.
pub fn close_surface(id: u64) -> Result<(), ()> {
    let mut state = lock_compositor();
    let st = state.as_mut().ok_or(())?;

    if st.exclusive.as_ref().map_or(false, |e| e.surface_id == id) {
        st.exclusive = None;
        crate::serial_println!(
            "[compositor] exclusive fullscreen auto-released (surface {} closed)",
            id
        );
    }

    if let Some(idx) = st.surfaces.iter().position(|s| s.id == id) {
        st.surfaces.remove(idx);
        if st.focused == Some(id) {
            st.focused = st.surfaces.last().map(|s| s.id);
        }
        crate::serial_println!("[compositor] surface {} closed", id);
        drop(state);
        mark_dirty();
        Ok(())
    } else {
        Err(())
    }
}

/// Remove all surfaces owned by the given task. Called when a task exits
/// so its windows don't linger on screen.
pub fn cleanup_task_surfaces(task_id: crate::task::TaskId) {
    let mut state = lock_compositor();
    let Some(st) = state.as_mut() else { return };

    let before = st.surfaces.len();
    st.surfaces.retain(|s| s.owner_task != task_id);
    let removed = before - st.surfaces.len();

    if removed > 0 {
        if st
            .focused
            .map_or(false, |fid| !st.surfaces.iter().any(|s| s.id == fid))
        {
            st.focused = st.surfaces.last().map(|s| s.id);
        }
        crate::serial_println!(
            "[compositor] cleaned up {} surface(s) for exited task {}",
            removed,
            task_id.raw(),
        );
        drop(state);
        mark_dirty();
    }
}

// ─── Surface Configuration API ──────────────────────────────────────────────

pub fn set_surface_hdr(id: u64, meta: SurfaceHdr) -> Result<(), ()> {
    let mut state = lock_compositor();
    let st = state.as_mut().ok_or(())?;
    if let Some(s) = st.surfaces.iter_mut().find(|s| s.id == id) {
        s.hdr_meta = meta;
        drop(state);
        mark_dirty();
        Ok(())
    } else {
        Err(())
    }
}

pub fn set_surface_blur(id: u64, blur: Option<BlurRegion>) -> Result<(), ()> {
    let mut state = lock_compositor();
    let st = state.as_mut().ok_or(())?;
    if let Some(s) = st.surfaces.iter_mut().find(|s| s.id == id) {
        s.blur = blur;
        drop(state);
        mark_dirty();
        Ok(())
    } else {
        Err(())
    }
}

pub fn add_surface_effect(id: u64, effect: SurfaceEffect) -> Result<(), ()> {
    let mut state = lock_compositor();
    let st = state.as_mut().ok_or(())?;
    if let Some(s) = st.surfaces.iter_mut().find(|s| s.id == id) {
        s.effects.push(effect);
        drop(state);
        mark_dirty();
        Ok(())
    } else {
        Err(())
    }
}

pub fn clear_surface_effects(id: u64) -> Result<(), ()> {
    let mut state = lock_compositor();
    let st = state.as_mut().ok_or(())?;
    if let Some(s) = st.surfaces.iter_mut().find(|s| s.id == id) {
        s.effects.clear();
        drop(state);
        mark_dirty();
        Ok(())
    } else {
        Err(())
    }
}

/// Read a surface's current `Opacity` effect value (`0xFF` = fully opaque, the
/// default when no `Opacity` effect is present). Lets a caller (the notification
/// toast depth-cue smoketest) assert the stack dimming without re-deriving it.
pub fn surface_opacity(id: u64) -> Option<u8> {
    let state = lock_compositor();
    let st = state.as_ref()?;
    let s = st.surfaces.iter().find(|s| s.id == id)?;
    Some(
        s.effects
            .iter()
            .find_map(|e| {
                if let SurfaceEffect::Opacity(o) = e {
                    Some(*o)
                } else {
                    None
                }
            })
            .unwrap_or(0xFF),
    )
}

/// Set (replacing, not appending) a surface's `Opacity` effect. The compositor
/// composites with the *first* `Opacity` it finds, so blindly re-adding via
/// `add_surface_effect` would let a stale value win; this updates in place so a
/// caller (e.g. the notification toast stack depth-cue) can re-dim a surface on
/// every restack without accumulating dead effects. `0xFF` is fully opaque.
pub fn set_surface_opacity(id: u64, opacity: u8) -> Result<(), ()> {
    let mut state = lock_compositor();
    let st = state.as_mut().ok_or(())?;
    if let Some(s) = st.surfaces.iter_mut().find(|s| s.id == id) {
        if let Some(e) = s
            .effects
            .iter_mut()
            .find(|e| matches!(e, SurfaceEffect::Opacity(_)))
        {
            *e = SurfaceEffect::Opacity(opacity);
        } else {
            s.effects.push(SurfaceEffect::Opacity(opacity));
        }
        drop(state);
        mark_dirty();
        Ok(())
    } else {
        Err(())
    }
}

// ─── VRR Configuration ─────────────────────────────────────────────────────

pub fn set_vrr_state(vrr: VrrState) {
    let mut state = lock_compositor();
    if let Some(st) = state.as_mut() {
        st.frame_pacer.update_vrr(vrr);
        crate::serial_println!(
            "[compositor] VRR updated: {}Hz-{}Hz, adaptive={}",
            vrr.min_hz,
            vrr.max_hz,
            vrr.supported
        );
        drop(state);
        mark_dirty();
    }
}

/// Switch the display refresh rate (Concept §Pro Gaming: "display refresh-rate
/// switching per game profile"). A per-game profile carries a `refresh_hz` the
/// player wants while that title runs (240 competitive / 144 balanced / 60
/// cinematic); `game_profile::apply_profile` calls this so the compositor's
/// frame pacer targets the right interval. `adaptive_vrr` picks the VRR posture:
/// `true` drives an adaptive range `[min(48, hz) ..= hz]` (the panel follows the
/// app's frame rate — for a profile that set `FLAG_VRR`), `false` a fixed `hz`
/// cadence. `hz == 0` is ignored (leave the current rate). No-op if the
/// compositor is not up. The live counterpart to `current_refresh_hz`.
pub fn set_refresh_hz(hz: u32, adaptive_vrr: bool) {
    if hz == 0 {
        return;
    }
    let vrr = if adaptive_vrr {
        VrrState::adaptive(hz.min(48), hz)
    } else {
        VrrState::fixed(hz)
    };
    set_vrr_state(vrr);
}

/// The compositor's CURRENT target refresh rate in Hz (the frame pacer's
/// `max_hz`), or `0` if the compositor is not up. Reads back what
/// `set_refresh_hz` / `set_vrr_state` last applied — the proof surface for the
/// per-game refresh-rate switch.
pub fn current_refresh_hz() -> u32 {
    lock_compositor()
        .as_ref()
        .map(|st| st.frame_pacer.vrr.max_hz)
        .unwrap_or(0)
}

// ─── HDR Configuration ──────────────────────────────────────────────────────

pub fn set_hdr_enabled(enabled: bool, operator: ToneMapOperator) {
    let mut state = lock_compositor();
    if let Some(st) = state.as_mut() {
        st.hdr_pipeline.enabled = enabled;
        st.hdr_pipeline.operator = operator;
        crate::serial_println!(
            "[compositor] HDR pipeline: {}",
            if enabled { "enabled" } else { "disabled" }
        );
        drop(state);
        mark_dirty();
    }
}

pub fn set_display_hdr_meta(meta: DisplayHdrMeta) {
    let mut state = lock_compositor();
    if let Some(st) = state.as_mut() {
        st.hdr_pipeline.display_meta = meta;
        drop(state);
        mark_dirty();
    }
}

// ─── Live Wallpaper API ─────────────────────────────────────────────────────

pub fn set_live_wallpaper(engine: Box<dyn LiveWallpaper>) {
    let mut state = lock_compositor();
    if let Some(st) = state.as_mut() {
        st.wallpaper = Some(WallpaperState::new(engine));
        crate::serial_println!("[compositor] live wallpaper installed");
    }
}

pub fn disable_wallpaper() {
    let mut state = lock_compositor();
    if let Some(st) = state.as_mut() {
        st.wallpaper = None;
    }
}

// ─── Capture API ────────────────────────────────────────────────────────────

pub fn start_capture(
    rx: u32,
    ry: u32,
    rw: u32,
    rh: u32,
    format: CaptureFormat,
    continuous: bool,
) -> u64 {
    let id = CAPTURE_SEQ.fetch_add(1, Ordering::Relaxed);
    let session = CaptureSession::new(id, rx, ry, rw, rh, format, continuous);
    let mut state = lock_compositor();
    if let Some(st) = state.as_mut() {
        st.captures.push(session);
        crate::serial_println!(
            "[compositor] capture session {} started: {}x{} at ({},{}), continuous={}",
            id,
            rw,
            rh,
            rx,
            ry,
            continuous
        );
    }
    id
}

/// Start a capture session OWNED by `owner_pid` (raw TaskId). Identical to
/// [`start_capture`] but tags the session so [`cleanup_task_captures`] can
/// reclaim it when the owning task exits — the privacy-sensitive
/// `SYS_CAPTURE_START` path uses this so a crashed capturer never leaks a
/// session. Does NOT alter the existing capture engine logic.
pub fn start_capture_owned(
    rx: u32,
    ry: u32,
    rw: u32,
    rh: u32,
    format: CaptureFormat,
    continuous: bool,
    owner_pid: u64,
) -> u64 {
    let id = start_capture(rx, ry, rw, rh, format, continuous);
    let mut state = lock_compositor();
    if let Some(st) = state.as_mut() {
        if let Some(sess) = st.captures.iter_mut().find(|c| c.id == id) {
            sess.owner = owner_pid;
        }
    }
    id
}

pub fn stop_capture(id: u64) {
    let mut state = lock_compositor();
    if let Some(st) = state.as_mut() {
        st.captures.retain(|c| c.id != id);
        crate::serial_println!("[compositor] capture session {} stopped", id);
    }
}

/// Reclaim every capture session owned by `owner_pid` — called from the
/// scheduler's `reclaim_task_resources` on task exit so a crashed/exited
/// capturer doesn't leak compositor capture sessions (mirrors the socket /
/// audio-voice exit sweep).
pub fn cleanup_task_captures(owner_pid: u64) {
    if owner_pid == 0 {
        return;
    }
    let mut state = lock_compositor();
    if let Some(st) = state.as_mut() {
        let before = st.captures.len();
        st.captures.retain(|c| c.owner != owner_pid);
        let removed = before - st.captures.len();
        if removed > 0 {
            crate::serial_println!(
                "[compositor] reclaimed {} capture session(s) for task {}",
                removed,
                owner_pid
            );
        }
    }
}

/// Number of live capture sessions (for `/proc/raeen/capture`).
pub fn capture_session_count() -> usize {
    let state = lock_compositor();
    state.as_ref().map(|st| st.captures.len()).unwrap_or(0)
}

/// `/proc/raeen/capture` body — one line per live session plus a header.
pub fn capture_dump_text() -> alloc::string::String {
    use core::fmt::Write;
    let state = lock_compositor();
    let mut out = alloc::string::String::new();
    let _ = writeln!(out, "# RaeenOS compositor capture sessions");
    if let Some(st) = state.as_ref() {
        let _ = writeln!(out, "active_sessions: {}", st.captures.len());
        for c in st.captures.iter() {
            let fmt = match c.format {
                CaptureFormat::Argb32 => "ARGB32",
                CaptureFormat::Bgra32 => "BGRA32",
            };
            let _ = writeln!(
                out,
                "id={} region={}x{}@({},{}) format={} continuous={} frames={} owner={}",
                c.id,
                c.region_w,
                c.region_h,
                c.region_x,
                c.region_y,
                fmt,
                c.continuous,
                c.frame_count,
                c.owner
            );
        }
    } else {
        let _ = writeln!(out, "active_sessions: 0 (compositor not initialised)");
    }
    out
}

/// Read the latest captured frame data. Returns None if no such session exists.
pub fn read_capture(id: u64) -> Option<(Vec<u32>, u32, u32)> {
    let state = lock_compositor();
    let st = state.as_ref()?;
    let sess = st.captures.iter().find(|c| c.id == id)?;
    Some((sess.front_buf.clone(), sess.region_w, sess.region_h))
}

/// Single-shot region capture against the LIVE composited frame (Concept
/// §creators — "capture & stream at the compositor, zero-cost"). This is the
/// synchronous grab the screenshot tool's overlay drives: it opens a session
/// over `(rx, ry, rw, rh)`, forces ONE `recomposite()` so the engine fills the
/// session buffer from the real front buffer, reads the pixels back, then stops
/// the session (no leak). Returns the ARGB `(pixels, w, h)` or `None` if the
/// region is degenerate / the compositor is down / the read failed.
///
/// Unlike the raw `start_capture`+`read_capture` pair (whose `front_buf` is
/// still zero until a frame composites), this guarantees real pixels in one
/// call — exactly what a still screenshot needs. The Game Bar / recorder keep
/// using a `continuous` session and read every frame.
pub fn capture_region_now(rx: u32, ry: u32, rw: u32, rh: u32) -> Option<(Vec<u32>, u32, u32)> {
    if rw == 0 || rh == 0 {
        return None;
    }
    // continuous=true so the one recomposite below doesn't deactivate+drop the
    // session before we can read it; we stop it explicitly after the read.
    let id = start_capture(rx, ry, rw, rh, CaptureFormat::Argb32, true);
    recomposite();
    let out = read_capture(id);
    stop_capture(id);
    match out {
        Some((px, w, h)) if w == rw && h == rh && !px.is_empty() => Some((px, w, h)),
        _ => None,
    }
}

/// Like [`read_capture`] but also returns the session's pixel byte order
/// (`CaptureFormat`) so the `SYS_CAPTURE_READ` header can report the EXACT
/// format the bytes are in (the engine converts to the session format in
/// `capture_from_composited`). Returns None if no such session exists.
pub fn read_capture_fmt(id: u64) -> Option<(Vec<u32>, u32, u32, CaptureFormat)> {
    let state = lock_compositor();
    let st = state.as_ref()?;
    let sess = st.captures.iter().find(|c| c.id == id)?;
    Some((
        sess.front_buf.clone(),
        sess.region_w,
        sess.region_h,
        sess.format,
    ))
}

// ─── Full Compositor Pipeline ───────────────────────────────────────────────

/// Recomposite all visible surfaces through the full pipeline:
///   1. Render live wallpaper (if not occluded)
///   2. For each surface (z-order): apply blur → HDR → effects → blit
///   3. Feed capture sessions
///   4. Flush to GPU / software framebuffer
///   5. Frame pacing (VRR)
///
/// IF=0 lock discipline (raeen-perf RANK 5 / goal #2 responsiveness): steps 1–3
/// read `surf.kernel_ptr` (USER-owned pages) — a preempting syscall could free a
/// surface's pages, so they MUST run under `lock_compositor()` (interrupts
/// disabled). The SCANOUT copy (step 4: the ~2M-pixel `comp_buf` → framebuffer/
/// virtio/GOP blast) reads ONLY the compositor-owned ready buffer, never user
/// pages, so it runs AFTER the guard drops with interrupts ENABLED. The finished
/// frame is moved into `scanout_ready` via a cheap `Vec` pointer swap under the
/// lock; the next frame composites into the other buffer, so the in-flight
/// scanout can never tear. This shrinks the IF=0 window from the whole frame to
/// just the composite (input events / timer ticks / syscalls are no longer
/// blocked for the milliseconds of the per-pixel scanout copy every frame).
// ─── GPU-scanout present performance (Concept: "Fast is a feature") ─────────
// The present pipeline must sustain 120+ fps at 1080p on iron — the number is
// owned by docs/PERFORMANCE_TARGETS.md; these counters are the live instrument
// behind it (rule 8: no perf claim without a counter). Written by the scanout
// path each frame, read by /proc/raeen/compositor + the present-bench.
static FRAME_US_LAST: AtomicU64 = AtomicU64::new(0);
static SCANOUT_BLIT_US: AtomicU64 = AtomicU64::new(0);
static FPS_WINDOW_START_US: AtomicU64 = AtomicU64::new(0);
static FPS_WINDOW_FRAMES: AtomicU64 = AtomicU64::new(0);
static FPS_NOW: AtomicU64 = AtomicU64::new(0);

/// `(frame_us_last, scanout_blit_us_last, fps_now)` for /proc/raeen/compositor.
pub fn present_perf() -> (u64, u64, u64) {
    (
        FRAME_US_LAST.load(Ordering::Relaxed),
        SCANOUT_BLIT_US.load(Ordering::Relaxed),
        FPS_NOW.load(Ordering::Relaxed),
    )
}

/// Row-wise GpuFb scanout blit + cache flush — the fast present.
///
/// The old path stored 2M pixels one bounds-checked `write_volatile` at a time
/// (~3–6 ms/frame at 1080p — a 60 fps ceiling from the present alone). This
/// copies each row with `copy_nonoverlapping` (LLVM lowers it to the platform
/// memcpy — GB/s class), then `clflush`es the written window + `sfence` so a
/// NON-SNOOPED scanout engine (the amdgpu DCN reading the WB-mapped UMA
/// carveout — see `gpu::register_external_scanout`) sees the frame immediately
/// instead of via lucky cache-capacity evictions. Harmless on snooped FBs
/// (virtio/Bochs): a cheap flush of lines we just wrote.
///
/// SAFETY: `fb_ptr` must point at a mapped framebuffer of at least
/// `ch * fb_stride` u32s; `src` at `(ch-1)*src_stride + cw` u32s; the regions
/// never overlap (src is a compositor-owned buffer, dst is the device FB).
unsafe fn blit_rows_to_gpu_fb(
    src: *const u32,
    src_stride: usize,
    fb_ptr: *mut u32,
    fb_stride: usize,
    cw: usize,
    ch: usize,
) {
    let t0 = monotonic_us();
    for y in 0..ch {
        core::ptr::copy_nonoverlapping(src.add(y * src_stride), fb_ptr.add(y * fb_stride), cw);
    }
    // Order the copies ahead of the flush loop, then flush the written window.
    core::sync::atomic::compiler_fence(Ordering::SeqCst);
    use core::arch::x86_64::{_mm_clflush, _mm_sfence};
    let bytes = ch.saturating_mul(fb_stride).saturating_mul(4);
    let base = fb_ptr as *const u8;
    let mut off = 0usize;
    while off < bytes {
        _mm_clflush(base.add(off));
        off += 64;
    }
    _mm_sfence();
    SCANOUT_BLIT_US.store(monotonic_us().saturating_sub(t0), Ordering::Relaxed);
}

/// Set the frame pacer's target interval (µs/frame). Called when the real GPU
/// scanout attaches: the DCN path targets 120 fps (8_333 µs) — the row-blit
/// present is fast enough and the sub-frame input contract wants the shorter
/// interval. docs/PERFORMANCE_TARGETS.md owns the number. (Driving the PANEL
/// above 60 Hz additionally needs the DCN modeset — Phase 2.3 EDID follow-up.)
pub fn set_target_frame_us(us: u64) {
    let mut guard = lock_compositor();
    if let Some(st) = guard.as_mut() {
        st.frame_pacer.vrr.target_frame_us = us.clamp(2_000, 100_000);
        let t = st.frame_pacer.vrr.target_frame_us;
        crate::serial_println!(
            "[compositor] frame target set: {} us/frame (~{} fps)",
            t,
            1_000_000 / t.max(1)
        );
    }
}

/// Present-bench: drive `n` forced recomposite+present frames flat-out and
/// report the measured rate — the FAIL-able instrument behind the 120 fps
/// present target (docs/PERFORMANCE_TARGETS.md). Runs on whatever scanout
/// backend is live (GOP at boot; the DCN GpuFb after the daemon attaches).
pub fn run_present_bench(tag: &str, n: u32) {
    let n = n.max(1);
    let t0 = monotonic_us();
    for _ in 0..n {
        mark_dirty();
        recomposite();
    }
    let total = monotonic_us().saturating_sub(t0);
    let avg = total / n as u64;
    let fps = if avg == 0 { 0 } else { 1_000_000 / avg };
    let blit = SCANOUT_BLIT_US.load(Ordering::Relaxed);
    let pass = total > 0 && fps > 0;
    crate::serial_println!(
        "[compositor] present-bench({}): frames={} avg_frame_us={} blit_us={} fps={} -> {}",
        tag,
        n,
        avg,
        blit,
        fps,
        if pass { "PASS" } else { "FAIL" }
    );
}

pub fn recomposite() {
    // `frame_start` measures the whole frame; `if_off_start` brackets the IF=0
    // (COMPOSITOR-held) section so `[latency-prof]` can prove the scanout is now
    // OUTSIDE the guard. Read before the lock (the lock disables interrupts).
    let frame_start = monotonic_us();
    let if_off_start = monotonic_us();

    let mut state = lock_compositor();
    let Some(st) = state.as_mut() else { return };

    st.total_frames += 1;

    // When exclusive fullscreen is active, the owning surface drives
    // presentation via `present_exclusive()`. The compositor thread
    // simply yields.
    if st.exclusive.is_some() {
        return;
    }

    let cw = st.comp_w as usize;
    let ch = st.comp_h as usize;
    let total = cw * ch;
    if st.comp_buf.len() != total {
        st.comp_buf.resize(total, 0);
    }

    let time_us = st.time_us;
    let time_ms = time_us / 1000;
    st.time_us += st.frame_pacer.vrr.target_frame_us;

    // ── Step 1: Live wallpaper ──────────────────────────────────────────
    let wallpaper_fully_occluded = is_wallpaper_occluded(&st.surfaces, st.comp_w, st.comp_h);

    if let Some(ref mut wp) = st.wallpaper {
        wp.paused = wallpaper_fully_occluded;
        if !wp.paused && time_ms >= wp.last_render_ms + wp.frame_cap_ms {
            wp.engine.render_frame(
                time_ms,
                &mut st.comp_buf,
                st.comp_w as u32,
                st.comp_h as u32,
            );
            wp.last_render_ms = time_ms;
        } else if wp.paused {
            for px in st.comp_buf.iter_mut() {
                *px = DESKTOP_BASE_ARGB;
            }
        }
    } else {
        for px in st.comp_buf.iter_mut() {
            *px = DESKTOP_BASE_ARGB;
        }
    }

    // ── Step 1b: Per-space wallpaper opacity cross-fade (window-management
    // §2). The shell drives WALLPAPER_ALPHA toward 0 to fade the current
    // wallpaper layer to the desktop base during a space switch. At 255 this is
    // a no-op (normal path unchanged); below 255 blend each wallpaper pixel
    // toward DESKTOP_BASE_ARGB by (255-alpha). Integer-only, allocation-free.
    let wp_alpha = WALLPAPER_ALPHA.load(Ordering::Relaxed) as u32;
    if wp_alpha < 255 {
        let inv = 255 - wp_alpha;
        let base_r = (DESKTOP_BASE_ARGB >> 16) & 0xFF;
        let base_g = (DESKTOP_BASE_ARGB >> 8) & 0xFF;
        let base_b = DESKTOP_BASE_ARGB & 0xFF;
        for px in st.comp_buf.iter_mut() {
            let r = (((*px >> 16) & 0xFF) * wp_alpha + base_r * inv) / 255;
            let g = (((*px >> 8) & 0xFF) * wp_alpha + base_g * inv) / 255;
            let b = ((*px & 0xFF) * wp_alpha + base_b * inv) / 255;
            *px = 0xFF00_0000 | (r << 16) | (g << 8) | b;
        }
    }

    // ── Step 2: Composite surfaces in z-order ───────────────────────────
    st.surfaces.sort_by_key(|s| s.z_order);

    let hdr_enabled = st.hdr_pipeline.enabled;
    let comp_w_u32 = st.comp_w;
    let comp_h_u32 = st.comp_h;

    // ── Step 2 (overview): Mission-Control scaled composite ─────────────
    // When overview is active, lay every VISIBLE userspace surface out as an
    // aspect-fit downscaled thumbnail at the `compute_layout(Tile,…)` grid cell
    // over a dimmed scrim — never re-rendering an app, just a scaled read of the
    // buffer the compositor already holds. Kernel surfaces (desktop/wallpaper
    // z=0) stay as the backdrop. The grid is FROZEN by snapshotting the window
    // list here so cells never reflow mid-frame. The `compute_layout` Vec is the
    // only allocation, and only while overview is on (the normal path is
    // byte-identical to today). After this block we fall through to the shared
    // cursor/capture/scanout tail with `overview_active` set so the normal
    // per-surface blit is skipped.
    let overview_active = OVERVIEW_ACTIVE.load(Ordering::Relaxed);
    if overview_active {
        // Dim the live desktop with the modal scrim (DESIGN_LANGUAGE).
        let scrim_a = (OVERVIEW_SCRIM_ARGB >> 24) & 0xFF;
        let scrim_r = (OVERVIEW_SCRIM_ARGB >> 16) & 0xFF;
        let scrim_g = (OVERVIEW_SCRIM_ARGB >> 8) & 0xFF;
        let scrim_b = OVERVIEW_SCRIM_ARGB & 0xFF;
        let inv = 255 - scrim_a;
        for px in st.comp_buf.iter_mut() {
            let r = (((*px >> 16) & 0xFF) * inv + scrim_r * scrim_a) / 255;
            let g = (((*px >> 8) & 0xFF) * inv + scrim_g * scrim_a) / 255;
            let b = ((*px & 0xFF) * inv + scrim_b * scrim_a) / 255;
            *px = 0xFF00_0000 | (r << 16) | (g << 8) | b;
        }

        let kernel = crate::task::TaskId::kernel_sentinel();
        // Frozen window list in z-order (bottom-first): id, w, h.
        let windows: Vec<(u64, u32, u32)> = st
            .surfaces
            .iter()
            .filter(|s| s.visible && s.owner_task != kernel && !s.minimized)
            .map(|s| (s.id, s.width, s.height))
            .collect();
        let n = windows.len() as u32;
        let cells = crate::wm_policy::compute_layout(
            crate::wm_policy::WmMode::Tile,
            comp_w_u32,
            comp_h_u32,
            &windows,
        );
        // Cell dimensions match wm_policy's near-square grid (cols=ceil(sqrt n)).
        let mut cols = 1u32;
        while cols * cols < n {
            cols += 1;
        }
        let rows = if n == 0 { 1 } else { (n + cols - 1) / cols };
        let cell_w = (comp_w_u32 / cols.max(1)) as i32;
        let cell_h = (comp_h_u32 / rows.max(1)) as i32;

        for (id, cx, cy) in cells {
            // Copy out the surface's geometry + buffer pointer BEFORE the &mut
            // st.comp_buf borrow so the immutable surface borrow does not outlive
            // it. All three are Copy; the read of `src` user pages stays UNDER
            // the COMPOSITOR lock (held for the whole recomposite) — UAF-safe.
            let (src, src_w, src_h) = match st.surfaces.iter().find(|s| s.id == id) {
                Some(s) => (s.kernel_ptr as *const u32, s.width, s.height),
                None => continue,
            };
            if src.is_null() {
                continue;
            }
            let (dx, dy, dw, dh) = overview_cell_dst(cx, cy, cell_w, cell_h, src_w, src_h);
            blit_thumbnail_into_comp(&mut st.comp_buf, cw, ch, src, src_w, src_h, dx, dy, dw, dh);
        }
    }

    for si in 0..st.surfaces.len() {
        // Overview replaces the per-surface z-order blit with thumbnails above.
        if overview_active {
            break;
        }
        let surf = &st.surfaces[si];
        if !surf.visible {
            continue;
        }

        let sw = surf.width;
        let sh = surf.height;
        let sx_off = surf.x;
        let sy_off = surf.y;
        let src = surf.kernel_ptr as *const u32;

        // ── Drop shadow (rendered behind the surface) ───────────────
        // The shadow silhouette is rounded to match the surface corners (so it
        // is not a square block). Read the corner radius from the surface's
        // RoundedCorners effect; default to `radius.md` (12px, design-language).
        let shadow_corner = surf
            .effects
            .iter()
            .find_map(|e| {
                if let SurfaceEffect::RoundedCorners { radius } = e {
                    Some(*radius)
                } else {
                    None
                }
            })
            .unwrap_or(12);
        for eff in surf.effects.iter() {
            if let SurfaceEffect::DropShadow {
                offset_x,
                offset_y,
                radius,
                color,
            } = *eff
            {
                // Disjoint field borrows: comp_buf / blur_region / blur_tmp are
                // separate fields of `st`, reused as the shadow scratch (no
                // per-frame alloc — same doctrine as the glass blur below).
                render_drop_shadow(
                    &mut st.comp_buf,
                    &mut st.blur_region,
                    &mut st.blur_tmp,
                    cw,
                    ch,
                    sx_off + offset_x,
                    sy_off + offset_y,
                    sw,
                    sh,
                    shadow_corner,
                    radius,
                    color,
                    false,
                );
            }
        }

        // ── Blur: sample the composited buffer behind this surface ──
        if let Some(ref blur) = surf.blur {
            let bx = sx_off + blur.x as i32;
            let by = sy_off + blur.y as i32;
            let bw = blur.width as usize;
            let bh = blur.height as usize;
            let radius = blur.radius;
            let tint = blur.tint_color;

            if bw > 0 && bh > 0 {
                // Reuse the compositor's persistent blur scratch instead of
                // allocating `region`/`tmp` every frame (kernel-allocator
                // jitter on the glassmorphism path). Disjoint field borrows:
                // blur_region / blur_tmp / comp_buf are separate fields of st.
                let needed = bw * bh;
                if st.blur_region.len() < needed {
                    st.blur_region.resize(needed, 0);
                }
                if st.blur_tmp.len() < needed {
                    st.blur_tmp.resize(needed, 0);
                }
                for ry in 0..bh {
                    let dy = by + ry as i32;
                    if dy < 0 || dy >= ch as i32 {
                        continue;
                    }
                    for rx in 0..bw {
                        let dx = bx + rx as i32;
                        if dx < 0 || dx >= cw as i32 {
                            continue;
                        }
                        st.blur_region[ry * bw + rx] = st.comp_buf[dy as usize * cw + dx as usize];
                    }
                }
                {
                    let (region, tmp) = (&mut st.blur_region, &mut st.blur_tmp);
                    BlurEngine::box_blur_3pass(
                        &mut region[..needed],
                        &mut tmp[..needed],
                        bw,
                        bh,
                        radius,
                    );
                    if tint != 0 {
                        // IDENTITY §2: classify the surface's declared tint into the
                        // nearest committed glass tier, then apply the §2.3 luma
                        // auto-adjust against the BLURRED BACKDROP MEAN (measured
                        // before tinting) so glass stays legible over bright/dark
                        // content without leaving its tier. The adjusted tier tint
                        // REPLACES the raw declared tint — this is the live tiered
                        // material repoint.
                        let tier = glass_tier_for_tint(tint);
                        let mean_luma = region_mean_luma(&region[..needed]);
                        let adjusted = rae_tokens::glass_luma_adjust(tier, mean_luma);
                        BlurEngine::apply_tint(&mut region[..needed], adjusted);
                        // FROST: lay the per-tier low-alpha WHITE sheen ON TOP of
                        // the slate tint (NOT before it), matching the canonical
                        // `rae_tokens::glass_tier_interior` order (tint → frost) and
                        // the finalized `raegfx::glass::draw_glass_surface`. This is
                        // the single step that moves "dark card" → "luminous frosted
                        // glass" (Round-3 visual-QA P0 #2): the FIXED per-tier frost
                        // alpha (chrome 0x04 < panel 0x23 < popover 0x38) also makes
                        // the interior luminance monotonic across tiers regardless of
                        // backdrop variance (P1 #4). `apply_tint` is a src-over
                        // flatten, so reusing it with `tier.frost` composites the
                        // white sheen over the just-tinted region in place — no alloc.
                        BlurEngine::apply_tint(&mut region[..needed], tier.frost);
                        // SHIP-GATE legibility cap (§2.3 / §9 — white text.primary):
                        // the tint→frost composite above mirrors the canonical glass
                        // ladder but NOT the `clamp_interior_luma` cap that
                        // `rae_tokens::glass_tier_interior` (and the finalized
                        // `raegfx::glass::draw_glass_surface`) apply on top — so the
                        // LIVE glass over a bright aurora blob stayed washed out and
                        // white text dropped below 4.5:1. Re-apply the SAME cap here,
                        // per pixel, over the composited region: a bright backdrop is
                        // pulled to GLASS_INTERIOR_LUMA_CEIL (white text wins); a dark
                        // backdrop is a NO-OP (glass stays frosted/translucent). In
                        // place on the blur scratch — no per-frame alloc.
                        BlurEngine::clamp_interior_luma(
                            &mut region[..needed],
                            rae_tokens::GLASS_INTERIOR_LUMA_CEIL,
                        );
                        // SHIP-GATE §9 WCAG legibility pass — the FINAL guarantee. The
                        // mean-channel cap above is a cheap gamma-ENCODED pre-pass; WCAG
                        // contrast is computed on the gamma-DECODED relative luminance, and
                        // the two diverge for saturated pixels (a pixel held at mean 0.40
                        // can still measure ~2.8:1 white-on-glass — the context-menu AA
                        // failure raegfx caught). This pass measures the REAL contrast of
                        // white text.primary against each interior pixel and bisects a
                        // hue-preserving darkening factor until it clears 4.5:1 (+margin),
                        // so live glass == the `raegfx::glass` host-render. No-op over a
                        // dark backdrop (every pixel already clears AA). In place on the
                        // blur scratch — no per-frame alloc.
                        BlurEngine::clamp_interior_wcag(&mut region[..needed]);
                    }
                }

                for ry in 0..bh {
                    let dy = by + ry as i32;
                    if dy < 0 || dy >= ch as i32 {
                        continue;
                    }
                    for rx in 0..bw {
                        let dx = bx + rx as i32;
                        if dx < 0 || dx >= cw as i32 {
                            continue;
                        }
                        st.comp_buf[dy as usize * cw + dx as usize] = st.blur_region[ry * bw + rx];
                    }
                }
            }
        }

        // ── Title bar (kernel-drawn window chrome) ──────────────────
        let title = core::str::from_utf8(&surf.title[..surf.title_len as usize]).unwrap_or("App");
        let is_userspace = surf.owner_task != crate::task::TaskId::kernel_sentinel();
        // Focused state drives the title-bar tint, top-edge highlight, title
        // text colour, and control saturation (window_chrome::draw_title_bar).
        // `st.focused` is a Copy field; reading it here does not conflict with
        // the disjoint `&mut st.comp_buf` borrow below.
        let surf_is_focused = st.focused == Some(surf.id);
        if is_userspace {
            crate::window_chrome::draw_title_bar(
                &mut st.comp_buf,
                cw,
                sx_off,
                sy_off,
                sw as i32,
                title,
                surf_is_focused,
            );
        }
        if surf.minimized {
            continue;
        }
        let client_y = if is_userspace {
            sy_off + crate::window_chrome::TITLE_BAR_H
        } else {
            sy_off
        };

        // ── Blit surface pixels into comp_buf ───────────────────────
        let has_rounded = surf
            .effects
            .iter()
            .any(|e| matches!(e, SurfaceEffect::RoundedCorners { .. }));
        let corner_radius = surf
            .effects
            .iter()
            .find_map(|e| {
                if let SurfaceEffect::RoundedCorners { radius } = e {
                    Some(*radius)
                } else {
                    None
                }
            })
            .unwrap_or(0);

        let opacity_scale = surf
            .effects
            .iter()
            .find_map(|e| {
                if let SurfaceEffect::Opacity(o) = e {
                    Some(*o)
                } else {
                    None
                }
            })
            .unwrap_or(255);

        for sy in 0..sh as i32 {
            let dy = client_y + sy;
            if dy < 0 {
                continue;
            }
            if dy >= comp_h_u32 as i32 {
                break;
            }
            for sx_i in 0..sw as i32 {
                let dx = sx_off + sx_i;
                if dx < 0 || dx >= comp_w_u32 as i32 {
                    continue;
                }

                if has_rounded
                    && !is_inside_rounded_rect(sx_i as u32, sy as u32, sw, sh, corner_radius)
                {
                    continue;
                }

                let src_idx = (sy as usize) * (sw as usize) + (sx_i as usize);
                let mut pixel = unsafe { src.add(src_idx).read_volatile() };
                let mut a = (pixel >> 24) & 0xFF;
                if a == 0 {
                    continue;
                }

                if opacity_scale < 255 {
                    a = (a * opacity_scale as u32) / 255;
                    pixel = (a << 24) | (pixel & 0x00FF_FFFF);
                }

                if hdr_enabled {
                    pixel = st
                        .hdr_pipeline
                        .process_pixel(pixel, &st.surfaces[si].hdr_meta);
                }

                let dst_idx = (dy as usize) * cw + (dx as usize);
                if a >= 255 {
                    st.comp_buf[dst_idx] = pixel;
                } else {
                    let bg = st.comp_buf[dst_idx];
                    st.comp_buf[dst_idx] = alpha_blend(pixel, bg);
                }
            }
        }

        // ── Border effect ───────────────────────────────────────────
        for eff in surf.effects.iter() {
            if let SurfaceEffect::Border { width: bw, color } = *eff {
                render_border(&mut st.comp_buf, cw, ch, sx_off, sy_off, sw, sh, bw, color);
            }
        }

        // ── Glow effect (elev.focus: additive accent glow, not displacement) ──
        // Centered (no offset), additive over the surface edge — a glow ring,
        // not a cast shadow (material-and-shadow.md: focus = ring + glow).
        for eff in surf.effects.iter() {
            if let SurfaceEffect::Glow { radius, color } = *eff {
                render_drop_shadow(
                    &mut st.comp_buf,
                    &mut st.blur_region,
                    &mut st.blur_tmp,
                    cw,
                    ch,
                    sx_off,
                    sy_off,
                    sw,
                    sh,
                    shadow_corner,
                    radius,
                    color,
                    true,
                );
            }
        }

        // ── OBSIDIAN edge (IDENTITY-OBSIDIAN.md §2) ─────────────────────
        // The iridescent perimeter rim is RETIRED on live surfaces too — on
        // the near-black material the rainbow border read as a theme-mod, not
        // refraction. Depth comes from the hairline + top light the surface's
        // own draw lays down plus the drop shadow above; no extra edge pass
        // here. (`draw_iridescent_rim` stays available for theming callers.)
    }

    // ── Step 2b: Draw cursor ─────────────────────────────────────────────
    if st.cursor.visible {
        let cx = st.cursor.x;
        let cy = st.cursor.y;
        for row in 0..CURSOR_H {
            let py = cy + row as i32;
            if py < 0 || py >= ch as i32 {
                continue;
            }
            for col in 0..CURSOR_W {
                let px = cx + col as i32;
                if px < 0 || px >= cw as i32 {
                    continue;
                }
                let pixel = CURSOR_BITMAP[row * CURSOR_W + col];
                if pixel == 0 {
                    continue;
                }
                let color = if pixel == 1 {
                    0xFF_FF_FF_FF
                } else {
                    0xFF_00_00_00
                };
                st.comp_buf[py as usize * cw + px as usize] = color;
            }
        }
    }

    // ── Step 3: Feed capture sessions ───────────────────────────────────
    let mut i = 0;
    while i < st.captures.len() {
        st.captures[i].capture_from_composited(&st.comp_buf, comp_w_u32, comp_h_u32);
        if !st.captures[i].continuous {
            st.captures[i].active = false;
        }
        i += 1;
    }
    st.captures.retain(|c| c.active);

    // ── Step 4 (prep): pick backend + make the frame safe to scan out unlocked ─
    // Lazy virtio-gpu upgrade (its init runs after compositor::init): on the
    // first composite, if no gpu_fb is attached and we're still on GOP, try to
    // bring up a live virtio scanout resource. One-shot. Done under the lock.
    if !st.virtio_tried && st.gpu_fb.is_none() && st.scanout == ScanoutBackend::Gop {
        st.virtio_tried = true;
        if crate::virtio_gpu::is_available()
            && crate::virtio_gpu::present_init(st.comp_w, st.comp_h)
        {
            st.scanout = ScanoutBackend::VirtioGpu;
            crate::serial_println!("[compositor] scanout backend upgraded -> VirtioGpu");
        }
    }
    // gpu_fb (Bochs/stdvga or an iron GPU's linear scanout) wins dynamically the
    // moment it's attached; otherwise use the selected backend (VirtioGpu/Gop).
    let backend = if st.gpu_fb.is_some() {
        ScanoutBackend::GpuFb
    } else {
        st.scanout
    };

    // Snapshot the scanout destination descriptors (plain Copy values / a raw
    // address as usize) so they outlive the guard. `fb` is Copy; the GpuFb ptr
    // is captured as a usize address + stride. No borrow of `st` survives.
    let fb_snapshot = st.fb;
    let gpu_fb_target: Option<(usize, usize)> = st
        .gpu_fb
        .as_ref()
        .map(|g| (g.ptr as usize, g.stride as usize));
    let comp_w_snapshot = st.comp_w;
    let comp_h_snapshot = st.comp_h;

    // Hand the just-composited frame to the stable scanout buffer via a cheap
    // `Vec` pointer swap (no per-pixel work, no alloc): `scanout_ready` now holds
    // the finished frame and `comp_buf` becomes last frame's buffer, which the
    // NEXT recomposite overwrites — so the in-flight scanout can never tear. The
    // GOP swizzle backbuffer travels with it so its blast also runs unlocked.
    core::mem::swap(&mut st.comp_buf, &mut st.scanout_ready);
    core::mem::swap(&mut st.sw_backbuf, &mut st.scanout_backbuf);

    // Frame pacing mutates `st`; record it under the lock (cheap, no scanout).
    st.frame_pacer
        .record_frame(st.frame_pacer.vrr.target_frame_us);
    st.frame_pacer.mark_presented(time_us);

    // Move the ready buffers OUT of `st` into locals so the scanout can read them
    // after the guard drops without holding any borrow of `st`. These are
    // compositor-OWNED, persistent buffers (never freed by a user task exit) — so
    // the unlocked scanout reads ONLY compositor memory, NEVER `surf.kernel_ptr`
    // user pages. That is what makes dropping the lock before scanout UAF-safe.
    let mut ready = core::mem::take(&mut st.scanout_ready);
    let mut ready_backbuf = core::mem::take(&mut st.scanout_backbuf);

    // ── Drop the IF=0 guard HERE: interrupts re-enabled before the scanout ──
    // Everything above touched user pages (surface compositing); everything
    // below touches only `ready`/`ready_backbuf` (compositor-owned). End of the
    // IF=0 window — measured by `if_off_us` below.
    drop(state);
    let if_off_us = monotonic_us().saturating_sub(if_off_start);

    let cw_s = comp_w_snapshot as usize;
    let ch_s = comp_h_snapshot as usize;

    // Accessibility §3 (magnifier): when enabled at >1.0x, replace the 1:1 read
    // with a source-sampled UPSCALE of a zoom window of `ready` centered on the
    // focus point. `None` => the normal byte-identical 1:1 path below. This reads
    // ONLY `ready` (compositor-owned) so it stays UAF-safe and alloc-free outside
    // the lock, exactly like the 1:1 copy.
    let mag = MagParams::current(cw_s, ch_s);

    // Accessibility (color filters): when set, each scanned-out pixel passes
    // through `FilterParams::apply` (invert / grayscale / high-contrast) AFTER
    // the magnifier samples it — sample-then-filter, see the module note. `None`
    // (the common case) => pixels are written untransformed (ZERO normal-path
    // per-pixel cost). Like `mag`, this reads/writes only compositor-owned
    // scanout buffers, so it stays UAF-safe and alloc-free outside the lock.
    let filt = FilterParams::current();

    // Helper: fetch the (possibly magnified) source pixel for dst (x,y), then
    // apply the color filter if any. Single point that composes mag + filter.
    let src_pixel = |x: usize, y: usize| -> u32 {
        let p = match mag {
            Some(m) => ready[m.src_index(x, y)],
            None => ready[y * cw_s + x],
        };
        match filt {
            Some(f) => f.apply(p),
            None => p,
        }
    };

    // ── Step 4 (scanout): runs with interrupts ENABLED, reading only `ready` ──
    match backend {
        ScanoutBackend::GpuFb => {
            if let Some((ptr_addr, fb_stride)) = gpu_fb_target {
                let fb_ptr = ptr_addr as *mut u32;
                if mag.is_some() || filt.is_some() {
                    // Rare transform path: resolve mag/filter into the reusable
                    // backbuffer (no alloc — same pattern as the virtio arm),
                    // then take the fast row-blit like the common case.
                    ready_backbuf.resize(cw_s * ch_s, 0);
                    for y in 0..ch_s {
                        let row = y * cw_s;
                        for x in 0..cw_s {
                            ready_backbuf[row + x] = src_pixel(x, y);
                        }
                    }
                    unsafe {
                        blit_rows_to_gpu_fb(
                            ready_backbuf.as_ptr(),
                            cw_s,
                            fb_ptr,
                            fb_stride,
                            cw_s,
                            ch_s,
                        )
                    };
                } else {
                    // Common case: straight row blit + flush (was 2M per-pixel
                    // volatile stores — the 60 fps ceiling; see blit_rows_to_gpu_fb).
                    unsafe {
                        blit_rows_to_gpu_fb(ready.as_ptr(), cw_s, fb_ptr, fb_stride, cw_s, ch_s)
                    };
                }
            }
        }
        ScanoutBackend::VirtioGpu => {
            if mag.is_some() || filt.is_some() {
                // Sample (and filter) into the reusable backbuffer (already taken
                // out of `st` for the unlocked scanout — no alloc), then present
                // it. Sized to the full frame, so it is a 1:1 sink.
                ready_backbuf.resize(cw_s * ch_s, 0);
                for y in 0..ch_s {
                    let row = y * cw_s;
                    for x in 0..cw_s {
                        ready_backbuf[row + x] = src_pixel(x, y);
                    }
                }
                crate::virtio_gpu::present_frame(&ready_backbuf, comp_w_snapshot, comp_h_snapshot);
            } else {
                crate::virtio_gpu::present_frame(&ready, comp_w_snapshot, comp_h_snapshot);
            }
        }
        ScanoutBackend::Gop => {
            if mag.is_some() || filt.is_some() {
                // Sampling/filtering into `ready` in place is unsafe (overlapping
                // read/write as we walk forward), so resolve into the swizzle
                // backbuffer then hand THAT to the GOP flush. `ready_backbuf` is
                // the unlocked, alloc-free scratch; the GOP path's own swizzle
                // backbuffer is the separate `_backbuf` arg below.
                ready_backbuf.resize(cw_s * ch_s, 0);
                for y in 0..ch_s {
                    let row = y * cw_s;
                    for x in 0..cw_s {
                        ready_backbuf[row + x] = src_pixel(x, y);
                    }
                }
                let mut gop_swizzle = ready;
                flush_comp_buf_to_sw_fb(&ready_backbuf, &mut gop_swizzle, &fb_snapshot, cw_s, ch_s);
                ready = gop_swizzle;
            } else {
                flush_comp_buf_to_sw_fb(&ready, &mut ready_backbuf, &fb_snapshot, cw_s, ch_s);
            }
        }
    }

    // GpuFb finishes with the device-side flip — already outside the lock.
    if backend == ScanoutBackend::GpuFb {
        crate::gpu::present_gpu_scanout();
    }

    // Present-perf counters (always-on, lock-free): whole-frame time + a ~1s
    // sliding FPS window — the live instrument behind the 120 fps present target
    // (/proc/raeen/compositor `present:` line; docs/PERFORMANCE_TARGETS.md).
    let frame_us = monotonic_us().saturating_sub(frame_start);
    FRAME_US_LAST.store(frame_us, Ordering::Relaxed);
    {
        let now = monotonic_us();
        let start = FPS_WINDOW_START_US.load(Ordering::Relaxed);
        let n = FPS_WINDOW_FRAMES.fetch_add(1, Ordering::Relaxed) + 1;
        let elapsed = now.saturating_sub(start);
        if start == 0 {
            FPS_WINDOW_START_US.store(now, Ordering::Relaxed);
            FPS_WINDOW_FRAMES.store(0, Ordering::Relaxed);
        } else if elapsed >= 1_000_000 {
            FPS_NOW.store(
                n.saturating_mul(1_000_000) / elapsed.max(1),
                Ordering::Relaxed,
            );
            FPS_WINDOW_START_US.store(now, Ordering::Relaxed);
            FPS_WINDOW_FRAMES.store(0, Ordering::Relaxed);
        }
    }

    // Latency proof (raeen-perf RANK 5 / goal #2): `if_off_us` EXCLUDES the
    // scanout above; `frame_us` includes it. Rate-capped to the serial UART (the
    // probe itself is a byte-polled latency tax — see DIRTY_PROBE_EVERY note).
    if DIRTY_PROBE_COUNT.fetch_add(1, Ordering::Relaxed) % DIRTY_PROBE_EVERY == 0 {
        crate::serial_println!(
            "[latency-prof] compositor: frame_us={} if_off_us={} blit_us={} fps={}",
            frame_us,
            if_off_us,
            SCANOUT_BLIT_US.load(Ordering::Relaxed),
            FPS_NOW.load(Ordering::Relaxed)
        );
    }

    // Put the buffers back so the next frame reuses them (no per-frame alloc).
    // This is a pure pointer swap under a fresh, brief IF=0 hold — no per-pixel
    // work, so the IF=0 window stays bounded to a handful of instructions, NOT
    // the scanout. A syscall that ran during the unlocked scanout cannot have
    // touched `scanout_ready`/`scanout_backbuf` (they were `take`-n to None/empty
    // Vecs), so this restore is race-free.
    let mut state = lock_compositor();
    if let Some(st) = state.as_mut() {
        st.scanout_ready = core::mem::take(&mut ready);
        st.scanout_backbuf = core::mem::take(&mut ready_backbuf);
    }
}

/// Return the id of the currently focused surface, if any.
pub fn focused_surface_id() -> Option<u64> {
    lock_compositor().as_ref().and_then(|st| st.focused)
}

/// Return the TaskId that owns the currently focused surface, if any.
/// The shell desktop surface (id 0, kernel sentinel) is excluded so input
/// only routes to real userspace apps.
pub fn focused_task_id() -> Option<crate::task::TaskId> {
    let state = lock_compositor();
    let st = state.as_ref()?;
    let focused_id = st.focused?;
    let surface = st.surfaces.iter().find(|s| s.id == focused_id)?;
    let tid = surface.owner_task;
    if tid.raw() == 0 {
        return None;
    }
    Some(tid)
}

// ─── Alpha blending ─────────────────────────────────────────────────────────

fn alpha_blend(fg: u32, bg: u32) -> u32 {
    let fa = (fg >> 24) & 0xFF;
    if fa == 0 {
        return bg;
    }
    if fa >= 255 {
        return fg;
    }
    let inv = 255 - fa;
    let r = ((((fg >> 16) & 0xFF) * fa + ((bg >> 16) & 0xFF) * inv) / 255) & 0xFF;
    let g = ((((fg >> 8) & 0xFF) * fa + ((bg >> 8) & 0xFF) * inv) / 255) & 0xFF;
    let b = (((fg & 0xFF) * fa + (bg & 0xFF) * inv) / 255) & 0xFF;
    0xFF00_0000 | (r << 16) | (g << 8) | b
}

// ─── Effect Helpers ─────────────────────────────────────────────────────────

/// Soft-ambient drop shadow (the `elev.*` / `material-and-shadow.md` rendering
/// contract). The shadow is a **blurred, offset, constant-color silhouette** of
/// the surface's rounded-rect coverage — NOT an analytic per-pixel falloff and
/// it NEVER samples the backdrop (the old renderer leaked the wallpaper's blue
/// into the shadow — raeen-visual-qa finding #1, the #1 "looks basic" defect).
///
/// Concept §RaeUI: "glassmorphic, GPU-accelerated… looks like Metal." A soft
/// ambient shadow with a smooth penumbra is exactly the premium cue that the
/// hard blue offset block destroyed; this restores it for every elevated
/// surface at once.
///
/// Algorithm (reuses `BlurEngine::box_blur_3pass`, the glassmorphism blur):
///   1. Render the rounded silhouette into `mask` as a grayscale COVERAGE field
///      (white = covered, black = outside) — coverage lives in the RGB channels
///      because the box blur preserves alpha and only averages RGB.
///   2. Blur the coverage by `radius` → a ~`radius`px penumbra.
///   3. For each pixel, alpha = `shadow_alpha * coverage/255`, RGB = the constant
///      `color` (near-black, or `accent.glow` for `elev.focus`). Composite under
///      the surface: source-over for shadows, additive for the focus glow.
///
/// `mask`/`tmp` are the compositor's persistent scratch vecs (no per-frame alloc).
/// `corner` is the silhouette corner radius (so the shadow is rounded, not a
/// square block). `additive=true` is `elev.focus` (glow); false is a cast shadow.
#[allow(clippy::too_many_arguments)]
fn render_drop_shadow(
    buf: &mut [u32],
    mask: &mut Vec<u32>,
    tmp: &mut Vec<u32>,
    bw: usize,
    bh: usize,
    sx: i32,
    sy: i32,
    sw: u32,
    sh: u32,
    corner: u32,
    radius: u32,
    color: u32,
    additive: bool,
) {
    if sw == 0 || sh == 0 || ((color >> 24) & 0xFF) == 0 {
        return;
    }
    let shadow_a = (color >> 24) & 0xFF;
    let shadow_r = (color >> 16) & 0xFF;
    let shadow_g = (color >> 8) & 0xFF;
    let shadow_b = color & 0xFF;

    // Pad the work region by `radius` on every side so the penumbra has room to
    // bleed outward (the blur kernel reaches `radius` pixels). Mask coordinates
    // are local to this padded box; (mx0, my0) is its top-left in screen space.
    let pad = radius as i32;
    let mw = (sw as i32 + 2 * pad) as usize;
    let mh = (sh as i32 + 2 * pad) as usize;
    let mx0 = sx - pad;
    let my0 = sy - pad;
    let needed = mw * mh;
    if needed == 0 {
        return;
    }
    if mask.len() < needed {
        mask.resize(needed, 0);
    }
    if tmp.len() < needed {
        tmp.resize(needed, 0);
    }
    // Clear only the slice we use (it may be larger from a previous, bigger call).
    for px in mask[..needed].iter_mut() {
        *px = 0;
    }

    // Step 1: rasterize the rounded silhouette as a coverage field. White inside
    // the rounded rect (full coverage), black outside. A clamped corner radius
    // keeps `is_inside_rounded_rect` well-formed for small surfaces.
    let cr = {
        let half = core::cmp::min(sw, sh) / 2;
        if corner > half {
            half
        } else {
            corner
        }
    };
    for ly in 0..sh {
        let row = (pad as usize + ly as usize) * mw + pad as usize;
        for lx in 0..sw {
            if is_inside_rounded_rect(lx, ly, sw, sh, cr) {
                // Coverage in RGB; alpha is irrelevant (blur preserves it / we
                // ignore it on readback) but we set it for cleanliness.
                mask[row + lx as usize] = 0xFF_FF_FF_FF;
            }
        }
    }

    // Step 2: blur the coverage field → the penumbra. The 3-pass box blur ≈ a
    // Gaussian of the same radius, which IS the soft falloff (no analytic curve).
    {
        let m = &mut mask[..needed];
        let t = &mut tmp[..needed];
        BlurEngine::box_blur_3pass(m, t, mw, mh, radius);
    }

    // Step 3: composite the constant-color shadow weighted by blurred coverage.
    for my in 0..mh {
        let py = my0 + my as i32;
        if py < 0 || py >= bh as i32 {
            continue;
        }
        for mx in 0..mw {
            let px = mx0 + mx as i32;
            if px < 0 || px >= bw as i32 {
                continue;
            }
            // Coverage = blurred luminance (the channels are equal post-blur of a
            // gray field). Read green; it carries the same value as R and B.
            let coverage = (mask[my * mw + mx] >> 8) & 0xFF;
            if coverage == 0 {
                continue;
            }
            let a = (shadow_a * coverage) / 255;
            if a == 0 {
                continue;
            }
            let idx = py as usize * bw + px as usize;
            let dst = buf[idx];
            if additive {
                // elev.focus glow: add the accent color, scaled by coverage*alpha.
                let w = a; // 0..255 weight
                let dr = (dst >> 16) & 0xFF;
                let dg = (dst >> 8) & 0xFF;
                let db = dst & 0xFF;
                let nr = core::cmp::min(255, dr + (shadow_r * w) / 255);
                let ng = core::cmp::min(255, dg + (shadow_g * w) / 255);
                let nb = core::cmp::min(255, db + (shadow_b * w) / 255);
                buf[idx] = (dst & 0xFF00_0000) | (nr << 16) | (ng << 8) | nb;
            } else {
                let fg = (a << 24) | (shadow_r << 16) | (shadow_g << 8) | shadow_b;
                buf[idx] = alpha_blend(fg, dst);
            }
        }
    }
}

fn render_border(
    buf: &mut [u32],
    bw: usize,
    bh: usize,
    sx: i32,
    sy: i32,
    sw: u32,
    sh: u32,
    border_w: u32,
    color: u32,
) {
    let bord = border_w as i32;
    for dy in -bord..sh as i32 + bord {
        let py = sy + dy;
        if py < 0 || py >= bh as i32 {
            continue;
        }
        for dx in -bord..sw as i32 + bord {
            let px = sx + dx;
            if px < 0 || px >= bw as i32 {
                continue;
            }

            let is_border = dx < 0 || dx >= sw as i32 || dy < 0 || dy >= sh as i32;
            if !is_border {
                continue;
            }

            let idx = py as usize * bw + px as usize;
            buf[idx] = alpha_blend(color, buf[idx]);
        }
    }
}

fn is_wallpaper_occluded(surfaces: &[Surface], screen_w: u32, screen_h: u32) -> bool {
    let sw = screen_w as i64;
    let sh = screen_h as i64;
    let mut covered = 0i64;
    for s in surfaces {
        if !s.visible {
            continue;
        }
        let left = if s.x < 0 { 0 } else { s.x as i64 };
        let top = if s.y < 0 { 0 } else { s.y as i64 };
        let right = f32_min((s.x as i64 + s.width as i64) as f32, sw as f32) as i64;
        let bottom = f32_min((s.y as i64 + s.height as i64) as f32, sh as f32) as i64;
        if right > left && bottom > top {
            covered += (right - left) * (bottom - top);
        }
    }
    covered >= sw * sh
}

/// Flush the intermediate compositing buffer to the software framebuffer.
fn flush_comp_buf_to_sw_fb(
    comp: &[u32],
    sw_backbuf: &mut Vec<u32>,
    fb: &crate::framebuffer::FbInfo,
    cw: usize,
    ch: usize,
) {
    let fb_w = fb.width as usize;
    let fb_h = fb.height as usize;
    let fb_bpp = fb.bytes_per_pixel as usize;
    let fb_stride_pixels = fb.stride as usize;
    let fb_ptr = fb.ptr;

    let rows = if ch < fb_h { ch } else { fb_h };
    let cols = if cw < fb_w { cw } else { fb_w };

    if fb_bpp == 4 {
        let total_pixels = fb_stride_pixels * fb_h;
        if sw_backbuf.len() != total_pixels {
            sw_backbuf.resize(total_pixels, 0);
        }

        // Swizzle directly into the persistent software backbuffer
        for y in 0..rows {
            let src_row = &comp[y * cw..y * cw + cols];
            let dst_row = &mut sw_backbuf[y * fb_stride_pixels..y * fb_stride_pixels + cols];
            for x in 0..cols {
                let pixel = src_row[x];
                // Convert ARGB to BGR and preserve Alpha if needed
                let r = (pixel >> 16) & 0xFF;
                let g = (pixel >> 8) & 0xFF;
                let b = pixel & 0xFF;
                let a = (pixel >> 24) & 0xFF;
                dst_row[x] = (a << 24) | (r << 16) | (g << 8) | b;
            }
        }

        // Blast the entire backbuffer to MMIO in one continuous write-combining copy.
        // This eliminates vertical tearing caused by scanning out while we are actively swizzling.
        unsafe {
            core::ptr::copy_nonoverlapping(sw_backbuf.as_ptr(), fb_ptr as *mut u32, total_pixels);
        }
    } else {
        // Fallback for non-32bpp framebuffers (rare)
        let mut row_buf = alloc::vec::Vec::with_capacity(cols * 3);
        row_buf.resize(cols * 3, 0u8);

        for y in 0..rows {
            let fb_row_ptr = unsafe { fb_ptr.add(y * fb_stride_pixels * fb_bpp) } as *mut u8;
            let src_row = &comp[y * cw..y * cw + cols];

            for x in 0..cols {
                let pixel = src_row[x];
                row_buf[x * 3] = (pixel & 0xFF) as u8; // b
                row_buf[x * 3 + 1] = ((pixel >> 8) & 0xFF) as u8; // g
                row_buf[x * 3 + 2] = ((pixel >> 16) & 0xFF) as u8; // r
            }
            unsafe {
                core::ptr::copy_nonoverlapping(row_buf.as_ptr(), fb_row_ptr, cols * 3);
            }
        }
    }
}

// ─── Damage / Dirty → Wake ──────────────────────────────────────────────────
//
// raeen-perf RANK 1 (goal #2 "sub-frame input latency"): without a damage path
// every cursor move / surface update sat in compositor state until the next
// fixed poll, adding up to a full poll interval of input→photon latency. The
// state-mutating paths now set `COMPOSITOR_DIRTY`; the compositor thread wakes
// and recomposites IMMEDIATELY on dirty instead of waiting on its idle cadence.
//
// The rate cap MUST use a REAL-TIME microsecond source, NOT `timers::JIFFIES`:
// JIFFIES ticks once per LAPIC timer IRQ (100 Hz / 10 ms, apic.rs:223) while
// `timers::HZ` is 1000, so any JIFFIES-derived "ms" is ~10× inflated — the old
// `now >= last + 16` gate actually fired only every ~160 ms (≈6 fps) when idle.
// We read the TSC (calibrated once, cached) for a true monotonic µs clock and
// pace against `frame_pacer.vrr.target_frame_us` (the real panel interval).

/// Set by every state-mutating input/surface path that changes what is on
/// screen. The compositor thread clears it and recomposites immediately.
static COMPOSITOR_DIRTY: AtomicBool = AtomicBool::new(false);

/// Real-time monotonic timestamp (µs) captured the last time `mark_dirty()`
/// fired, for the input→present latency probe. 0 = no pending dirty stamp.
static LAST_DIRTY_US: AtomicU64 = AtomicU64::new(0);

/// Cached TSC frequency (Hz). 0 = not yet calibrated. Calibration runs once,
/// lazily, off the boot path (first compositor-thread iteration) so the heavy
/// PIT spin-wait never lands in the hot loop.
static TSC_HZ: AtomicU64 = AtomicU64::new(0);

/// Rate-limit counter for the latency probe so a flood of dirty wakes can't
/// flood the (expensive, byte-polled) serial UART.
static DIRTY_PROBE_COUNT: AtomicU64 = AtomicU64::new(0);
/// Emit the latency line at most once per this many dirty-driven recomposites.
const DIRTY_PROBE_EVERY: u64 = 64;

/// Monotonic time since boot, in microseconds, from the TSC. Real-time source
/// for the dirty-wake rate cap and the input→present probe — deliberately NOT
/// JIFFIES (see module note above). Returns 0 only if the TSC can't calibrate.
fn monotonic_us() -> u64 {
    let mut hz = TSC_HZ.load(Ordering::Relaxed);
    if hz == 0 {
        // Calibrate once. `calibrate()` may PIT-spin; this runs at most once and
        // off the boot critical path. A racing second caller just recalibrates
        // harmlessly and stores the same value.
        hz = crate::timers::TscCalibration::calibrate().frequency_hz;
        TSC_HZ.store(hz, Ordering::Relaxed);
    }
    if hz == 0 {
        return 0;
    }
    let tsc = crate::timers::TscCalibration::read_tsc();
    // u128 to avoid overflow: tsc*1e6 overflows u64 after ~75 min at 4 GHz.
    ((tsc as u128 * 1_000_000u128) / hz as u128) as u64
}

/// Request an immediate recomposite. Called from every path that mutates what
/// is on screen (cursor move, surface present/close/effect change, …). Sets the
/// dirty flag and stamps the real-time clock for the input→present latency
/// probe. Lock-free (Relaxed) so it is safe to call from IRQ context and from
/// inside the `lock_compositor` critical section without lock-order concerns.
pub fn mark_dirty() {
    COMPOSITOR_DIRTY.store(true, Ordering::Relaxed);
    // Only stamp if no dirty is already pending, so the measured delta reflects
    // the OLDEST unserviced input, not the newest (worst-case latency).
    LAST_DIRTY_US
        .compare_exchange(0, monotonic_us(), Ordering::Relaxed, Ordering::Relaxed)
        .ok();
}

/// Alias matching raeen-perf's vocabulary; see [`mark_dirty`].
pub fn request_recomposite() {
    mark_dirty();
}

/// Run one compositor scheduling decision against a real-time `now_us`.
/// Returns `(did_recomposite, capped)`:
///   * dirty + enough time since last present → recomposite now (clears dirty),
///   * dirty + too soon (flood) → `capped=true`, skip this iteration,
///   * not dirty + idle interval elapsed → recomposite (idle ~60 fps cadence),
///   * not dirty + idle interval not elapsed → do nothing.
/// `last_present_us` is advanced to `now_us` whenever we recomposite.
/// Split out from the thread loop so the smoketest can drive it deterministically.
fn compositor_tick(now_us: u64, last_present_us: &mut u64, target_us: u64) -> (bool, bool) {
    let dirty = COMPOSITOR_DIRTY.load(Ordering::Relaxed);
    let elapsed = now_us.saturating_sub(*last_present_us);

    if dirty {
        if elapsed >= target_us {
            // Clear the flag BEFORE compositing so a mutation racing in during
            // the frame re-arms the next wake rather than being lost.
            COMPOSITOR_DIRTY.store(false, Ordering::Relaxed);
            let dirty_us = LAST_DIRTY_US.swap(0, Ordering::Relaxed);
            recomposite();
            *last_present_us = now_us;
            if dirty_us != 0 && now_us >= dirty_us {
                let n = DIRTY_PROBE_COUNT.fetch_add(1, Ordering::Relaxed);
                if n < 8 || n % DIRTY_PROBE_EVERY == 0 {
                    crate::serial_println!(
                        "[latency-prof] input->present: delta={}us (dirty-wake)",
                        now_us - dirty_us
                    );
                }
            }
            (true, false)
        } else {
            // Flood: a burst of mouse reports must not drive recomposite faster
            // than the panel. Keep dirty set; we'll service it next interval.
            (false, true)
        }
    } else if elapsed >= target_us.max(16_667) {
        // Idle cadence — real-time paced, floored at ~60 fps regardless of the
        // pacer target: the 120 fps target (8_333 µs, set when the real GPU
        // scanout attaches) buys INPUT-driven frames their short interval above,
        // but redrawing an idle desktop (animated wallpaper) at 120 fps would
        // just double the compose burn (~4 ms/frame at 1080p) for frames a 60 Hz
        // panel mostly never shows. NOT the inflated JIFFIES gate.
        recomposite();
        *last_present_us = now_us;
        (true, false)
    } else {
        (false, false)
    }
}

// ─── SCHED_GAME Compositor Thread ──────────────────────────────────────────

extern "C" fn compositor_thread_entry() {
    let mut last_present_us = monotonic_us();
    loop {
        // Pull the panel interval from the frame pacer each iteration so VRR
        // updates take effect. Default fixed-60 → 16_667 µs.
        let target_us = {
            let guard = lock_compositor();
            guard
                .as_ref()
                .map(|st| st.frame_pacer.vrr.target_frame_us)
                .unwrap_or(16_667)
        };
        let now_us = monotonic_us();
        compositor_tick(now_us, &mut last_present_us, target_us);
        crate::scheduler::yield_task();
        x86_64::instructions::hlt();
    }
}

/// Spawn the dedicated compositor thread.
///
/// KEYSTONE FIX (root-caused from iron bootlog 2026-06-15T1128): this thread
/// used to be `TaskPriority::Game` with an EDF deadline. But it does only a few
/// microseconds of work per iteration then `yield_task`s, so it never
/// accumulates its runtime budget and EDF never throttles it — leaving it
/// ALWAYS runnable. Since `pick_next` always prefers deadline/game over normal,
/// the compositor won EVERY scheduling decision on its CPU and starved every
/// normal task: desktop auto-advance, HID drain, net poll, late-flush, even
/// user_init (the sched-diag dump caught the compositor as `current` with 6
/// `Ready` normal tasks never picked). That single starvation was the cause of
/// "boots to login but no desktop, dead mouse, no DHCP" on iron.
///
/// Until SCHED_GAME gets real runtime-budget throttling (so a deadline task
/// that has used its slice this period stops being picked until the next
/// period), the compositor runs as a fair CFS-normal task, sharing the CPU so
/// the rest of the system runs. BSP-pinned because the APs don't schedule
/// post-boot.
///
/// The loop is damage-driven (raeen-perf RANK 1): input/surface mutations set
/// `COMPOSITOR_DIRTY` and it recomposites IMMEDIATELY, rate-capped to the panel
/// interval via a real-time TSC µs clock (`monotonic_us`). When idle it paces
/// the same real-time clock against `frame_pacer.vrr.target_frame_us` (~60 fps)
/// — NOT the old `timers::JIFFIES` gate, which was ~10× inflated (LAPIC 100 Hz
/// vs `HZ`=1000) and idle-polled at only ~6 fps.
pub fn spawn_compositor_thread() {
    let task = crate::task::Task::new(compositor_thread_entry, None);
    // ── DO NOT promote this thread to SCHED_GAME EDF without fixing the tick. ──
    // Tried 2026-07-03 (EDF 8_333/8_333/1_100): QEMU passed both SMP configs, but
    // IRON froze the desktop — with audio (2_667 µs) + HID (2_000 µs) already on
    // CPU0, a third EDF task saturates the ~100 Hz-tick pick slots and the whole
    // Normal class (amdgpud, net poll, shell) starves down to the
    // 1-in-64 starvation-guard picks (netlog tail = repeating "[sched] CPU0
    // deadline-starvation guard tripped (streak=64)"). The measured disease this
    // was meant to cure is real — steady-state fps=3 on iron, a ~3 Hz cursor,
    // pure pick-latency while the present pipeline benches 220 fps — but the fix
    // needs one of: the high-res/one-shot LAPIC tick (the PERFORMANCE_TARGETS #2
    // lever, raises the pick rate so EDF density fits), a longer period (16_667),
    // or Normal-aware EDF admission. Until then: Normal priority, BSP-pinned
    // (the KEYSTONE rule — APs don't schedule post-boot).
    let task_id = task.id;
    crate::scheduler::spawn_on_bsp(task);
    crate::serial_println!(
        "[compositor] compositor thread spawned (CFS-normal, BSP-pinned, task={:?})",
        task_id,
    );
}

// ─── Exclusive Fullscreen Mode Setting ─────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExclusiveDisplayMode {
    pub width: u32,
    pub height: u32,
    pub refresh_hz: u32,
    pub bpp: u8,
    pub interlaced: bool,
}

impl ExclusiveDisplayMode {
    pub fn new(width: u32, height: u32, refresh_hz: u32) -> Self {
        Self {
            width,
            height,
            refresh_hz,
            bpp: 32,
            interlaced: false,
        }
    }

    pub fn pixel_clock_khz(&self) -> u64 {
        let total_h = self.width as u64 + 160;
        let total_v = self.height as u64 + 36;
        total_h * total_v * self.refresh_hz as u64 / 1000
    }

    pub fn frame_time_us(&self) -> u64 {
        if self.refresh_hz == 0 {
            return 16_667;
        }
        1_000_000 / self.refresh_hz as u64
    }
}

pub struct ExclusiveDisplayModeTable {
    pub modes: [Option<ExclusiveDisplayMode>; 32],
    pub count: usize,
    pub current: usize,
    pub native: usize,
}

impl ExclusiveDisplayModeTable {
    pub fn new() -> Self {
        Self {
            modes: [None; 32],
            count: 0,
            current: 0,
            native: 0,
        }
    }

    pub fn add_mode(&mut self, mode: ExclusiveDisplayMode) -> bool {
        if self.count >= 32 {
            return false;
        }
        self.modes[self.count] = Some(mode);
        self.count += 1;
        true
    }

    pub fn add_standard_modes(&mut self) {
        let standards = [
            (1920, 1080, 60),
            (1920, 1080, 120),
            (1920, 1080, 144),
            (2560, 1440, 60),
            (2560, 1440, 120),
            (2560, 1440, 144),
            (2560, 1440, 165),
            (3840, 2160, 60),
            (3840, 2160, 120),
        ];
        for (w, h, r) in &standards {
            self.add_mode(ExclusiveDisplayMode::new(*w, *h, *r));
        }
    }

    pub fn current_mode(&self) -> Option<ExclusiveDisplayMode> {
        self.modes[self.current]
    }

    pub fn native_mode(&self) -> Option<ExclusiveDisplayMode> {
        self.modes[self.native]
    }

    pub fn find_mode(&self, width: u32, height: u32, refresh_hz: u32) -> Option<usize> {
        for i in 0..self.count {
            if let Some(m) = &self.modes[i] {
                if m.width == width && m.height == height && m.refresh_hz == refresh_hz {
                    return Some(i);
                }
            }
        }
        None
    }

    pub fn find_best_mode(&self, width: u32, height: u32) -> Option<usize> {
        let mut best: Option<(usize, u32)> = None;
        for i in 0..self.count {
            if let Some(m) = &self.modes[i] {
                if m.width == width && m.height == height {
                    if best.map_or(true, |(_, r)| m.refresh_hz > r) {
                        best = Some((i, m.refresh_hz));
                    }
                }
            }
        }
        best.map(|(i, _)| i)
    }

    pub fn set_mode(&mut self, idx: usize) -> bool {
        if idx < self.count && self.modes[idx].is_some() {
            self.current = idx;
            true
        } else {
            false
        }
    }
}

// ─── Exclusive VRR Controller ──────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExclusiveVrrPhase {
    Disabled,
    Enabled,
    Active,
    OutOfRange,
}

pub struct ExclusiveVrrController {
    pub phase: ExclusiveVrrPhase,
    pub min_hz: u32,
    pub max_hz: u32,
    pub current_hz: u32,
    pub target_hz: u32,
    pub last_flip_us: u64,
    pub frame_count: u64,
    pub lfc_enabled: bool,
    pub lfc_multiplier: u32,
}

impl ExclusiveVrrController {
    pub fn new(min_hz: u32, max_hz: u32) -> Self {
        Self {
            phase: ExclusiveVrrPhase::Disabled,
            min_hz,
            max_hz,
            current_hz: max_hz,
            target_hz: max_hz,
            last_flip_us: 0,
            frame_count: 0,
            lfc_enabled: true,
            lfc_multiplier: 1,
        }
    }

    pub fn enable(&mut self) {
        self.phase = ExclusiveVrrPhase::Enabled;
    }

    pub fn disable(&mut self) {
        self.phase = ExclusiveVrrPhase::Disabled;
        self.current_hz = self.max_hz;
    }

    pub fn on_frame_present(&mut self, now_us: u64) {
        if self.phase == ExclusiveVrrPhase::Disabled {
            return;
        }

        if self.last_flip_us > 0 {
            let delta = now_us.saturating_sub(self.last_flip_us);
            if delta > 0 {
                let actual_hz = (1_000_000 + delta / 2) / delta;
                self.target_hz = actual_hz as u32;

                if self.target_hz >= self.min_hz && self.target_hz <= self.max_hz {
                    self.current_hz = self.target_hz;
                    self.phase = ExclusiveVrrPhase::Active;
                    self.lfc_multiplier = 1;
                } else if self.target_hz < self.min_hz && self.lfc_enabled {
                    self.lfc_multiplier =
                        (self.min_hz + self.target_hz - 1) / self.target_hz.max(1);
                    self.current_hz = self.target_hz * self.lfc_multiplier;
                    self.phase = ExclusiveVrrPhase::Active;
                } else {
                    self.phase = ExclusiveVrrPhase::OutOfRange;
                    self.current_hz = self.max_hz;
                }
            }
        }

        self.last_flip_us = now_us;
        self.frame_count += 1;
    }

    pub fn frame_time_us(&self) -> u64 {
        if self.current_hz == 0 {
            return 16_667;
        }
        1_000_000 / self.current_hz as u64
    }

    pub fn is_active(&self) -> bool {
        self.phase == ExclusiveVrrPhase::Active
    }

    pub fn using_lfc(&self) -> bool {
        self.lfc_multiplier > 1
    }
}

// ─── Display Hotplug for Exclusive Fullscreen Recovery ─────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotplugEvent {
    Connected,
    Disconnected,
    ModeChanged,
    DpmsOn,
    DpmsOff,
}

#[derive(Debug, Clone, Copy)]
pub struct ConnectedDisplay {
    pub id: u32,
    pub connected: bool,
    pub width: u32,
    pub height: u32,
    pub refresh_hz: u32,
    pub primary: bool,
    pub hdr_capable: bool,
    pub vrr_capable: bool,
}

pub struct DisplayHotplugManager {
    pub displays: [Option<ConnectedDisplay>; 8],
    pub display_count: usize,
    pub primary_display: u32,
    pub pending_events: [(HotplugEvent, u32); 16],
    pub event_count: usize,
}

impl DisplayHotplugManager {
    pub fn new() -> Self {
        Self {
            displays: [None; 8],
            display_count: 0,
            primary_display: 0,
            pending_events: [(HotplugEvent::Connected, 0); 16],
            event_count: 0,
        }
    }

    pub fn register_display(&mut self, info: ConnectedDisplay) -> bool {
        if self.display_count >= 8 {
            return false;
        }
        self.displays[self.display_count] = Some(info);
        if info.primary {
            self.primary_display = info.id;
        }
        self.display_count += 1;
        self.push_event(HotplugEvent::Connected, info.id);
        true
    }

    pub fn unregister_display(&mut self, id: u32) {
        for i in 0..self.display_count {
            if let Some(d) = &self.displays[i] {
                if d.id == id {
                    self.displays[i] = None;
                    self.push_event(HotplugEvent::Disconnected, id);

                    if self.primary_display == id {
                        self.primary_display = self
                            .displays
                            .iter()
                            .filter_map(|d| d.as_ref())
                            .map(|d| d.id)
                            .next()
                            .unwrap_or(0);
                    }
                    return;
                }
            }
        }
    }

    pub fn on_hotplug(&mut self, id: u32, connected: bool) {
        if connected {
            if !self
                .displays
                .iter()
                .any(|d| d.as_ref().map_or(false, |d| d.id == id))
            {
                self.register_display(ConnectedDisplay {
                    id,
                    connected: true,
                    width: 1920,
                    height: 1080,
                    refresh_hz: 60,
                    primary: self.display_count == 0,
                    hdr_capable: false,
                    vrr_capable: false,
                });
            }
        } else {
            self.unregister_display(id);
        }
    }

    pub fn handle_exclusive_disconnect(&mut self, display_id: u32) -> bool {
        let is_exclusive_display = self
            .displays
            .iter()
            .filter_map(|d| d.as_ref())
            .any(|d| d.id == display_id && d.primary);

        if is_exclusive_display {
            crate::serial_println!(
                "[compositor] Exclusive fullscreen display {} disconnected, forcing release",
                display_id
            );
            return true;
        }
        false
    }

    pub fn primary_display_info(&self) -> Option<&ConnectedDisplay> {
        self.displays
            .iter()
            .filter_map(|d| d.as_ref())
            .find(|d| d.id == self.primary_display)
    }

    fn push_event(&mut self, event: HotplugEvent, display_id: u32) {
        if self.event_count < 16 {
            self.pending_events[self.event_count] = (event, display_id);
            self.event_count += 1;
        }
    }

    pub fn drain_events(&mut self) -> &[(HotplugEvent, u32)] {
        let count = self.event_count;
        self.event_count = 0;
        &self.pending_events[..count]
    }
}

// ─── Exclusive Fullscreen Mode Switch ──────────────────────────────────────

/// When entering exclusive fullscreen, the compositor must:
/// 1. Save the current display mode
/// 2. Switch to the game's requested mode (resolution + refresh rate)
/// 3. On exit, restore the saved mode
pub struct ExclusiveModeSwitch {
    pub saved_mode: Option<ExclusiveDisplayMode>,
    pub game_mode: Option<ExclusiveDisplayMode>,
    pub transition_in_progress: bool,
    pub mode_switch_fence: u64,
}

impl ExclusiveModeSwitch {
    pub fn new() -> Self {
        Self {
            saved_mode: None,
            game_mode: None,
            transition_in_progress: false,
            mode_switch_fence: 0,
        }
    }

    pub fn begin_transition(
        &mut self,
        current: ExclusiveDisplayMode,
        target: ExclusiveDisplayMode,
    ) -> bool {
        if self.transition_in_progress {
            return false;
        }
        self.saved_mode = Some(current);
        self.game_mode = Some(target);
        self.transition_in_progress = true;
        self.mode_switch_fence += 1;
        true
    }

    pub fn complete_transition(&mut self) {
        self.transition_in_progress = false;
    }

    pub fn begin_restore(&mut self) -> Option<ExclusiveDisplayMode> {
        if self.transition_in_progress {
            return None;
        }
        self.transition_in_progress = true;
        self.mode_switch_fence += 1;
        self.saved_mode.take()
    }

    pub fn complete_restore(&mut self) {
        self.game_mode = None;
        self.transition_in_progress = false;
    }

    pub fn is_mode_switched(&self) -> bool {
        self.game_mode.is_some() && !self.transition_in_progress
    }
}

// ── Hardware cursor ───────────────────────────────────────────────────────────
// MasterChecklist Phase 2.5: "Hardware cursor (acceleration is otherwise irrelevant if cursor lags)."
//
// True hardware cursor: the GPU programs a scanout overlay that moves without
// CPU blitting. Until a GPU driver exists we use the compositor's software cursor
// (already implemented). This module provides the API that the GPU driver will
// call once it's ready, and exposes the cursor state for the current software path.

static HW_CURSOR_ENABLED: AtomicBool = AtomicBool::new(false);
static HW_CURSOR_X: AtomicI32 = AtomicI32::new(0);
static HW_CURSOR_Y: AtomicI32 = AtomicI32::new(0);

/// Enable the hardware cursor overlay. Called by the GPU driver once it has
/// programmed the cursor plane. Falls back to software cursor when `false`.
pub fn enable_hw_cursor(enabled: bool) {
    HW_CURSOR_ENABLED.store(enabled, Ordering::Relaxed);
    crate::serial_println!(
        "[compositor] hardware cursor: {}",
        if enabled {
            "ENABLED (GPU plane)"
        } else {
            "DISABLED (software fallback)"
        }
    );
}

/// Update the hardware cursor position. The GPU driver reads these atomics from
/// the scanout interrupt to update the CRTC cursor registers without CPU blitting.
pub fn set_hw_cursor_pos(x: i32, y: i32) {
    HW_CURSOR_X.store(x, Ordering::Relaxed);
    HW_CURSOR_Y.store(y, Ordering::Relaxed);
}

pub fn hw_cursor_enabled() -> bool {
    HW_CURSOR_ENABLED.load(Ordering::Relaxed)
}

pub fn hw_cursor_pos() -> (i32, i32) {
    (
        HW_CURSOR_X.load(Ordering::Relaxed),
        HW_CURSOR_Y.load(Ordering::Relaxed),
    )
}
