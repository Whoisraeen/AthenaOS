//! Aurora Mesh wallpaper — the AthenaOS signature backdrop (IDENTITY.md §3):
//!
//! > "The flat void is half the problem: glass has nothing to refract. The
//! >  default AthenaOS wallpaper is a **procedural aurora mesh** — no asset file
//! >  … three to four soft radial color blobs on a deep base, drifting on
//! >  independent low-frequency sine paths, blended additively into smooth
//! >  color fields … alive, not busy."
//!   — docs/design/IDENTITY.md §3 "The signature backdrop — Aurora Mesh"
//!
//! This kills the "flat void desktop" defect: the old default was a near-black
//! two-stop navy gradient that gave the glass nothing to refract. The aurora is a
//! deep blue-violet night sky (`WALLPAPER_AURORA_BASE_DARK`) with slow-drifting
//! anisotropic radial *ribbons* — RaeBlue, violet, teal (from the committed
//! `AURORA_BLOB_*` tokens) — sheared ~2.5:1 along the NW→SE diagonal, additively
//! blended into a mesh-gradient and corner-vignetted so chrome and text stay
//! legible over the brightest region.
//!
//! ## Single source of truth (kill the double-maintenance)
//! The ribbon-mesh math lives in `raegfx::glass::render_aurora_dark` — the
//! finalized canonical render (raegfx commit `dcb4ee3`, host-KAT'd: anisotropic
//! NW→SE shear ~2.5:1, blue core weight ~138 / peak luma ~169, violet upper-right,
//! teal seam accent over the blue×violet seam). `raegfx` is `#![no_std]` and the
//! kernel already depends on it, and its `Canvas` is a `*mut u8` framebuffer
//! wrapper — so this engine constructs a `Canvas` straight over the compositor's
//! existing `comp_buf` (bpp=4, ARGB) with **no allocation** and CALLS the canonical
//! function. There is no mirrored copy of the aurora math here anymore; any future
//! polish to the ribbon mesh in raegfx flows to the live desktop automatically.
//!
//! ## Cost / hot-path discipline
//! This is the existing `compositor::LiveWallpaper` per-frame path: frame-capped at
//! 33 ms and auto-paused when a fullscreen surface occludes the desktop (the
//! compositor already drives that gating). The canonical renderer is integer /
//! fixed-point only (the SW-rasterizer constraint) and writes every pixel straight
//! into the caller's buffer through `Canvas` — **no per-frame heap allocation**.
//! The drift `phase` advances on a slow clock so motion is barely perceptible.

#![allow(dead_code)]

extern crate alloc;

use alloc::boxed::Box;
use rae_tokens::{
    AURORA_BLOB_BLUE, AURORA_BLOB_TEAL, AURORA_BLOB_VIOLET, WALLPAPER_AURORA_BASE_DARK,
};
use raegfx::Canvas;

/// Display name the live-wallpaper registry binds this engine under. The default
/// selection (and the `/proc/raeen/wallpaper` reflection) keys off this name.
pub const AURORA_NAME: &str = "Aurora Mesh";

/// Map a frame time (ms) to the canonical renderer's drift `phase` (period 1024,
/// the angle units of `raegfx::glass`'s integer sine). A full drift cycle takes
/// ~70 s (≈1024 phase units / (1000/64) per second), so motion is "alive, not
/// busy". Wrapped to the 1024-unit period so the value never overflows the table.
#[inline]
fn phase_for_time(time_ms: u64) -> u32 {
    ((time_ms / 64) % 1024) as u32
}

/// Render the aurora mesh into `buffer` (ARGB `0xFFRRGGBB`, row-major `w×h`) for a
/// frame at `time_ms`. Allocation-free: wraps the caller's buffer in a `raegfx`
/// `Canvas` (zero-copy: the `Vec<u32>`'s bytes ARE the framebuffer) and calls the
/// canonical `raegfx::glass::render_aurora_dark`. This is the shared core used by
/// both [`AuroraWallpaper::render_frame`] (the compositor's live default) and the
/// shell desktop fallback fill, so the live look is byte-identical to the
/// host-render screenshot harness.
pub fn render_aurora(buffer: &mut [u32], w: u32, h: u32, time_ms: u64) {
    if w == 0 || h == 0 {
        return;
    }
    let wu = w as usize;
    let hu = h as usize;
    if buffer.len() < wu * hu {
        return;
    }
    // SAFETY: `buffer` is `[u32]` of at least `w*h` elements; a `Canvas` over its
    // bytes at bpp=4 addresses exactly the same `w*h` ARGB pixels and never reads
    // or writes past `buffer.len()`. The borrow lasts only for this call.
    let mut canvas = unsafe { Canvas::new(buffer.as_mut_ptr() as *mut u8, wu, hu, 4) };
    raegfx::glass::render_aurora_dark(&mut canvas, 0, 0, wu, hu, phase_for_time(time_ms));
}

// ── LiveWallpaper engine ─────────────────────────────────────────────────────

/// The compositor's default live wallpaper: the procedural Aurora Mesh.
pub struct AuroraWallpaper;

impl AuroraWallpaper {
    pub fn new() -> Self {
        AuroraWallpaper
    }
}

impl Default for AuroraWallpaper {
    fn default() -> Self {
        Self::new()
    }
}

impl crate::compositor::LiveWallpaper for AuroraWallpaper {
    fn render_frame(&mut self, time_ms: u64, buffer: &mut [u32], width: u32, height: u32) {
        render_aurora(buffer, width, height, time_ms);
    }
}

// ── A monotonic-ish ms clock for the shell fallback fill ─────────────────────

/// Best-effort milliseconds since boot, for the shell's static desktop fallback
/// (the compositor's live path uses its own `time_us`). Reuses the TSC the
/// compositor calibrates; returns 0 before calibration (a still aurora, never a
/// panic).
pub fn aurora_now_ms() -> u64 {
    let cal = crate::timers::TscCalibration::calibrate();
    if cal.frequency_hz == 0 {
        return 0;
    }
    let tsc = crate::timers::TscCalibration::read_tsc();
    cal.tsc_to_ns(tsc) / 1_000_000
}

// ── Boot init / registration ─────────────────────────────────────────────────

/// Register the Aurora Mesh as the compositor's default live wallpaper. Called
/// from `kernel_main` after the compositor and the live-wallpaper registry are up.
pub fn init() {
    crate::compositor::set_live_wallpaper(Box::new(AuroraWallpaper::new()));
    DEFAULT_WALLPAPER_NAME.store(true, core::sync::atomic::Ordering::Relaxed);
    crate::serial_println!(
        "[ OK ] Aurora Mesh wallpaper: default backdrop set (base=#{:06x}, anisotropic ribbon mesh via raegfx::glass, vignette, no per-frame alloc)",
        WALLPAPER_AURORA_BASE_DARK & 0x00FF_FFFF,
    );
}

/// True once `init` has made the aurora the default (drives the procfs line).
static DEFAULT_WALLPAPER_NAME: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// Whether the aurora is the active default backdrop (for `/proc/raeen/wallpaper`).
pub fn is_default() -> bool {
    DEFAULT_WALLPAPER_NAME.load(core::sync::atomic::Ordering::Relaxed)
}

/// `/proc/raeen/wallpaper` identity prefix — the Aurora Mesh default-backdrop
/// state (IDENTITY §3). Allocation is fine here (procfs read path, not hot).
pub fn dump_text() -> alloc::string::String {
    alloc::format!(
        "# AthenaOS default backdrop = \"{}\" (Aurora Mesh, IDENTITY \u{a7}3)\n\
         # default_active={} base=#{:06x} blobs=[blue #{:06x}, violet #{:06x}, teal #{:06x}] ribbon_mesh=raegfx::glass vignette=0.85 per_frame_alloc=none\n",
        AURORA_NAME,
        is_default(),
        WALLPAPER_AURORA_BASE_DARK & 0x00FF_FFFF,
        AURORA_BLOB_BLUE & 0x00FF_FFFF,
        AURORA_BLOB_VIOLET & 0x00FF_FFFF,
        AURORA_BLOB_TEAL & 0x00FF_FFFF,
    )
}

// ── Boot smoketest (FAIL-able) ───────────────────────────────────────────────

/// Renders one aurora frame (via the canonical `raegfx::glass` ribbon-mesh path)
/// into a small off-screen buffer and asserts the IDENTITY contract on the LIVE
/// render: (1) the frame is not a flat void — there is a real luma spread across
/// it (the ribbon mesh, not a banded gradient); (2) the brightest pixel reaches
/// the target band (~120-190/255 — the lifted blue core weight ~138, NOT the old
/// underexposed ~94); (3) the rendered center region is brighter than the dark
/// corners (the vignette + blob placement); and (4) the aurora is installed as
/// the default backdrop. Any violation prints `FAIL`.
///
/// FAIL-ability: revert the ribbon math (drop the blue blob weight back toward
/// 118, or flatten the mesh to the old gradient) and the peak-luma / luma-spread
/// assertions trip. This asserts the NEW ribbon math, so it tracks the canonical
/// render rather than the retired isotropic-blob falloff.
pub fn run_boot_smoketest() {
    const TW: u32 = 160;
    const TH: u32 = 100;
    let mut buf = alloc::vec![0u32; (TW * TH) as usize];
    render_aurora(&mut buf, TW, TH, 0);

    let luma = |argb: u32| -> u32 {
        let r = (argb >> 16) & 0xFF;
        let g = (argb >> 8) & 0xFF;
        let b = argb & 0xFF;
        // Rec.601-ish weighted luma ×256 → 0..255.
        (r * 77 + g * 150 + b * 29) >> 8
    };

    // (1) Not a flat void: the frame must carry a real luma spread (max ≫ min).
    // The old two-stop navy gradient (and any flat fill) would FAIL this.
    let mut lo = u32::MAX;
    let mut hi = 0u32;
    let mut peak = 0u32;
    for &p in buf.iter() {
        let l = luma(p);
        lo = lo.min(l);
        hi = hi.max(l);
        peak = peak.max(l);
    }
    let has_spread = hi.saturating_sub(lo) > 24;

    // (2) The brightest pixel reaches the target luminance band (the lifted blue
    // core, ~120-190). Out of band = under/over-exposed = FAIL.
    let peak_in_band = (120..=190).contains(&peak);

    // (3) The interior (upper-left, where the bright blue ribbon sits at
    // ~0.34w,0.36h) is brighter than the dark bottom-right corner the vignette
    // dims most. A blob that did nothing → center == corner → FAIL.
    let interior = {
        let bx = (0.34 * TW as f32) as usize;
        let by = (0.36 * TH as f32) as usize;
        luma(buf[by * TW as usize + bx])
    };
    let corner_br = luma(buf[((TH - 1) * TW + (TW - 1)) as usize]);
    let center_brighter = interior > corner_br + 8;

    // (4) Aurora is the registered default.
    let is_def = is_default();

    if has_spread && peak_in_band && center_brighter && is_def {
        crate::serial_println!(
            "[aurora] smoketest: PASS (ribbon mesh: luma spread {}, peak {} in [120,190], interior {} > corner {}, default=on)",
            hi.saturating_sub(lo),
            peak,
            interior,
            corner_br,
        );
    } else {
        crate::serial_println!(
            "[aurora] smoketest: FAIL (spread={} (hi={} lo={}) peak={} in_band={} interior={} corner={} center_brighter={} default={})",
            hi.saturating_sub(lo),
            hi,
            lo,
            peak,
            peak_in_band,
            interior,
            corner_br,
            center_brighter,
            is_def,
        );
    }
}
