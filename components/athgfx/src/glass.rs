//! glass — the Liquid Glass identity primitives (IDENTITY.md §2-§4).
//!
//! *"Built for people who care about how things feel."* — `LEGACY_GAMING_CONCEPT.md`
//! §AthUI. The owner's verdict on the old build was "no native theme or identity
//! that is clean and stunning." IDENTITY.md names the three systemic fixes:
//! a **signature aurora backdrop** for glass to refract (§3, kills the flat
//! void), **tiered translucent glass** (§2, the backdrop reads through), and the
//! **iridescent perimeter rim** (§2.4, the one thing no flat Acrylic/Mica has).
//!
//! This module renders all three on the SAME software rasterizer the kernel
//! composites with (`Canvas`), so the kernel live path can mirror these exact
//! functions and the host-render screenshot harness can prove the look without a
//! boot. Pure integer / fixed-point math where the SW rasterizer allows; no
//! per-frame heap allocation in steady state (every fn writes straight to the
//! framebuffer through `Canvas`). All token values come from `ath_tokens`
//! (the frozen catalog) — this module never invents an alpha, hue, or blur.

use crate::Canvas;
use ath_tokens::{
    glass_luma_adjust, GlassTier, AURORA_BLOB_BLUE, AURORA_BLOB_TEAL, AURORA_BLOB_VIOLET,
    GLASS_CHROME_DARK, GLASS_CHROME_LIGHT, GLASS_EDGE_BAND_PX, GLASS_EDGE_CYAN, GLASS_EDGE_VIOLET,
    GLASS_EDGE_WARM, GLASS_INTERIOR_LUMA_CEIL, WALLPAPER_AURORA_BASE_DARK,
};

/// The CHROME-tier legibility/floor ceiling (Round-9 visual-QA #3 rebalance).
///
/// §7 names chrome the MOST see-through tier. Two earlier fixes fought each other:
/// the §2.3 body-text luma cap (`GLASS_INTERIOR_LUMA_CEIL` = 0.40 / ~L102) pulled
/// chrome down over a bright backdrop, and the Round-7 fixed black scrim
/// (`CHROME_FLOOR_SCRIM`, 22% black under tint+frost) then darkened chrome over
/// EVERY backdrop — so chrome rendered DARKER than panel (measured taskbar chrome
/// L77 ≈ CC panel L78, the scrim having pushed chrome from "most see-through" to
/// "as heavy as panel"). Round-9 retires the fixed scrim entirely.
///
/// The replacement is a SOFTER, chrome-only ceiling applied through the SAME
/// per-pixel luma cap the other tiers use, but at a HIGHER ceiling than panel/
/// popover (which sit at 0.40). Over a DARK/MID backdrop chrome's raw interior is
/// already below this ceiling, so the cap is a NO-OP and chrome tracks the backdrop
/// — the most see-through tier, lighter and closer to the backdrop than panel
/// (panel's heavier slate tint + bigger frost deviate it more). Over a BRIGHT
/// backdrop chrome floors at THIS ceiling — distinctly LIGHTER than panel's 0.40
/// floor (so chrome still reads as the most see-through tier) yet far enough below
/// the bright field that the bar never vanishes / tracks it within a few luma (the
/// Round-7 constant-floor win, kept). Chrome carries no body text (it's a taskbar
/// of icons/pills), so the higher ceiling does not regress white-text AA — that
/// guarantee is the §9 WCAG pass `clamp_interior_wcag_region` applies to ALL tiers.
///
/// 0.52 (~L133) gives chrome ≥20 luma below the worst bright field used by
/// `chrome_holds_constant_floor_over_bright` while sitting clearly above panel's
/// 0.40 floor over the same bright backdrop. INTERIM render-side fix: the proper
/// long-term home is the chrome tier's token recipe — route to athena-ui when
/// `ath_tokens` frees.
const CHROME_INTERIOR_LUMA_CEIL: f32 = 0.52;

// ════════════════════════════════════════════════════════════════════════
// §3 — Aurora Mesh backdrop (kills the flat void)
// ════════════════════════════════════════════════════════════════════════

/// One drifting aurora blob: an **anisotropic** `1/(1+d²)`-falloff radial sheared
/// along a NW→SE diagonal so it reads as a flowing *ribbon* (§3.2 "ribbon/mesh"),
/// not a round dot. Center is given in fractional screen coords (×1000 fixed-point)
/// so the same descriptor scales to any resolution; `reach` is the radius (×1000 of
/// min-dimension) at which the blob has faded to a low floor along its SHORT axis.
/// `weight` is the peak contribution (0..256). The falloff distance is measured in
/// a sheared frame: the blob is stretched `stretch`/256× along the NW→SE axis (e.g.
/// 640/256 ≈ 2.5:1) so circles become diagonal ribbons that overlap into a mesh.
struct Blob {
    /// Center X, fraction of width ×1000.
    cx_milli: i64,
    /// Center Y, fraction of height ×1000.
    cy_milli: i64,
    /// Falloff radius (SHORT axis), fraction of `min(w,h)` ×1000.
    reach_milli: i64,
    /// ARGB hue (alpha ignored; the radial supplies coverage).
    color: u32,
    /// Peak contribution, 0..=256 (the bright blue core is lifted so the backdrop
    /// has something luminous for the glass to refract — §3.2 / Round-3 visual-QA).
    weight: i64,
    /// Anisotropy ×256: the blob's LONG axis (NW→SE diagonal) reach is multiplied
    /// by this/256. 256 = isotropic (round); 640 ≈ 2.5:1 ribbon. The diagonal axis
    /// is fixed (u = (x+y)/√2, v = (x−y)/√2) so all ribbons flow the same way and
    /// their overlaps create the mesh-gradient color-blend zones.
    stretch: i64,
}

/// Render the **Aurora Mesh** dark backdrop (IDENTITY.md §3.2) into `[x,y,w,h)`.
///
/// Three slow-drifting radial blobs (RaeBlue / violet / teal) additively blended
/// over the deep blue-violet base `WALLPAPER_AURORA_BASE_DARK`, with a soft
/// `1/(1+d²)` falloff so the fields overlap into smooth color — luminous, not
/// banded. A subtle corner vignette (×0.85) keeps the screen edges calm so chrome
/// reads (§3.2). `phase` advances the drift (the live wallpaper passes a slowly
/// incrementing value; a still screenshot passes a fixed phase).
///
/// Pure integer math (the SW-rasterizer constraint): the `1/(1+d²)` falloff is a
/// fixed-point reciprocal, the drift a small-angle integer sine table. No floats,
/// no per-pixel allocation. The primary (blue) blob tracks the Vibe accent when a
/// caller overrides it via [`render_aurora_dark_accent`]; the violet + teal blobs
/// stay fixed so the mesh keeps depth (§4.1).
pub fn render_aurora_dark(c: &mut Canvas, x: usize, y: usize, w: usize, h: usize, phase: u32) {
    render_aurora_dark_accent(c, x, y, w, h, phase, AURORA_BLOB_BLUE);
}

/// As [`render_aurora_dark`], but the primary (large) blob uses `accent` instead
/// of the default RaeBlue — the Vibe-Mode seed flow (§4.1): one tap re-tints the
/// whole desktop because the accent drives the primary aurora blob. The violet +
/// teal blobs stay fixed so the mesh keeps depth.
pub fn render_aurora_dark_accent(
    c: &mut Canvas,
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    phase: u32,
    accent: u32,
) {
    if w == 0 || h == 0 {
        return;
    }
    // Drift: each blob moves on a tiny sine path so motion is barely perceptible
    // (§3.2 "alive, not busy"). The drift amplitude is a small fraction of the
    // screen; phase advances the angle. sin/cos via the integer table below.
    let drift = 60i64; // ±6% of dimension
    let s = isin(phase as i64);
    let cc = isin(phase as i64 + 256); // +90° → cosine
    let s2 = isin(phase as i64 * 2 + 80);
    let cc2 = isin(phase as i64 + 170);

    // Anisotropic ribbons (§3.2 "ribbon/mesh, not isotropic blob"): each blob is
    // sheared ~2.5:1 along the NW→SE diagonal so the round dots become flowing
    // ribbons. The blue core weight is LIFTED (Round-3 visual-QA: the old peak luma
    // 94/255 read "underexposed"; the reference aurora peaks ~150-180) so the
    // backdrop has a bright region for the glass to refract — the vignette still
    // protects the dark corners so chrome reads. The blue blob is placed UPPER-LEFT
    // and a violet accent UPPER-RIGHT so color falls behind a right-docked Control
    // Center (§4/Q4) instead of in a dead gutter. A 4th small teal accent overlaps
    // the blue×violet seam to create the additive color-mix zone that reads as a
    // premium mesh gradient.
    const RIBBON: i64 = 640; // ~2.5:1 stretch along the diagonal
    let blobs = [
        // 1. Primary (accent) — large blue ribbon, upper-left, the BRIGHT core.
        Blob {
            cx_milli: 340 + drift * cc / 1000,
            cy_milli: 360 + drift * s / 1000,
            reach_milli: 420,
            color: accent,
            // Lifted from 118 → 138 (Round-3), then trimmed toward 128 (Round-4
            // visual-QA P1 #5: measured peak L171 sat ~21 over the ~150-155 target
            // AND directly under the centered panels — the worst-case glass-contrast
            // cell where the §2.3 over-bright auto-adjust kicks in). 132 only pulled
            // the measured peak to L168 (still >158), so per the critique's escalation
            // ("go to 128 only if still >158") the core drops to 128 — landing the
            // peak in the ~150-155 band while keeping the backdrop luminous. The
            // vignette still darkens corners and the seam accent keeps the mesh mix.
            // Round-5 P1: trimmed a final 128 → 120 — with the violet/teal-seam blobs
            // also trimmed, the measured bare-wallpaper peak now lands at ~148 (≤150,
            // the calm-backdrop ceiling visual-QA asked for) with margin.
            // Round-11 premium refinement: trimmed 120 → 116 to reclaim ~2 luma of
            // headroom for the broad depth wave + ordered dither added below (both can
            // lift the brightest pixel by ~1 level each); the measured peak stays ≤150.
            weight: 116,
            stretch: RIBBON,
        },
        // 2. Violet — medium ribbon, upper-RIGHT (was bottom-right) so a colored
        //    region falls behind the right-docked CC proof shot (Q4 / P2 #6).
        Blob {
            cx_milli: 720 - drift * s2 / 1000,
            cy_milli: 360 + drift * cc2 / 1000,
            reach_milli: 380,
            color: AURORA_BLOB_VIOLET,
            // Round-5 visual-QA P1 — the bare wallpaper peak measured L166 (the
            // additive blue×violet×teal-seam overlap), over the ~150 calm-backdrop
            // target. The blue core already sits at 128; the remaining over-bright is
            // the bright violet (and teal-seam) stacking on top of it, so the violet
            // ribbon is trimmed 150 → 118 (its color is preserved — only the additive
            // peak drops) to bring the rendered peak into the ≤150 band. The glass
            // clamp now protects TEXT, but the bare wallpaper must still read calm.
            weight: 118,
            stretch: RIBBON,
        },
        // 3. Teal/cyan — ribbon, lower-left (fixed hue), an accent ribbon. Trimmed
        //    120 → 108 (Round-5 P1): teal is luma-bright (G=0xC8) so it lifts the peak
        //    where it grazes the blue core; the lower-left placement keeps it mostly
        //    clear, but the trim guarantees the ≤150 ceiling with margin.
        Blob {
            cx_milli: 240 + drift * s2 / 1000,
            cy_milli: 680 - drift * cc / 1000,
            reach_milli: 300,
            color: AURORA_BLOB_TEAL,
            weight: 108,
            stretch: RIBBON,
        },
        // 4. Teal seam accent — small, sits ON the blue→violet seam (mid-top) so
        //    the three hues additively mix into a visible color-blend zone (the
        //    mesh-gradient look, §3.2). Tighter + rounder than the ribbons.
        Blob {
            cx_milli: 540 + drift * s / 1000,
            cy_milli: 420 - drift * cc2 / 1000,
            reach_milli: 220,
            color: AURORA_BLOB_TEAL,
            // Sits ON the blue→violet seam (the exact peak location), so it is the
            // most direct lever on the over-bright peak — trimmed 72 → 60 (Round-5 P1)
            // while still painting a visible color-mix puddle for the mesh look.
            weight: 60,
            stretch: 384, // ~1.5:1, a softer mixing puddle
        },
    ];

    let base = WALLPAPER_AURORA_BASE_DARK;
    let (br, bg, bb) = (
        ((base >> 16) & 0xFF) as i64,
        ((base >> 8) & 0xFF) as i64,
        (base & 0xFF) as i64,
    );
    let min_dim = w.min(h) as i64;
    // Precompute blob centers + SHORT-axis reach² + stretch in pixel space. The
    // anisotropic falloff measures distance in a sheared diagonal frame:
    //   u = (dx + dy)        (NW→SE long axis, un-normalized ×1)
    //   v = (dx - dy)        (NE→SW short axis)
    // dividing u's contribution by stretch² stretches the blob along the diagonal.
    // We store (px, py, reach², stretch, R, G, B, weight).
    let mut bc: [(i64, i64, i64, i64, i64, i64, i64, i64); 4] = [(0, 0, 0, 0, 0, 0, 0, 0); 4];
    for (i, b) in blobs.iter().enumerate() {
        let px = x as i64 + b.cx_milli * w as i64 / 1000;
        let py = y as i64 + b.cy_milli * h as i64 / 1000;
        let reach = (b.reach_milli * min_dim / 1000).max(1);
        bc[i] = (
            px,
            py,
            reach * reach,
            b.stretch,
            ((b.color >> 16) & 0xFF) as i64,
            ((b.color >> 8) & 0xFF) as i64,
            (b.color & 0xFF) as i64,
            b.weight,
        );
    }

    let x_end = (x + w).min(c.width());
    let y_end = (y + h).min(c.height());
    // Vignette geometry: distance² from screen center, normalized so corners hit
    // the ×0.85 floor (§3.2 "subtle vignette to keep edges calm").
    let vcx = x as i64 + w as i64 / 2;
    let vcy = y as i64 + h as i64 / 2;
    let vmax = ((w as i64 / 2) * (w as i64 / 2) + (h as i64 / 2) * (h as i64 / 2)).max(1);

    // ── Round-11 premium refinement #2: BROAD low-frequency depth wave ──────────
    // The bare field BETWEEN the blobs was uniformly flat (the "two circles on a
    // gradient" read). A slow, large-scale luminance swell laid UNDER the blobs makes
    // the field organic — a gentle tide of deep blue-violet light, the hallmark of a
    // high-end mesh wallpaper. It is:
    //   * LOW frequency — ~1.5 cycles across the screen on each diagonal axis, so it
    //     never bands and never reads as a pattern (it's felt, not seen);
    //   * SMALL amplitude — ±WAVE_AMP levels, tinted toward the base's own blue-violet
    //     so it deepens/lifts the field without introducing a foreign hue;
    //   * BIASED slightly negative (the swell sits mostly BELOW the base) so it can
    //     never push the already-calibrated bright core over the ≤150 ceiling — where
    //     a blob dominates, the wave is a small dip, not a lift.
    // Pure integer (the two diagonal phases reuse the `isin` quarter-wave table); no
    // floats, no per-pixel alloc. `phase` drifts it with the live wallpaper so the
    // tide breathes. The wave math is factored into [`aurora_depth_wave`] so it is
    // pinned directly by `aurora_depth_wave_swells_and_dips` (FAIL-able in isolation,
    // unobscured by the blob field that swamps a whole-frame probe).
    let wsx = w.max(1) as i64;
    let wsy = h.max(1) as i64;

    for cy in y..y_end {
        for cx in x..x_end {
            // Broad depth wave: a slow screen-scale swell laid UNDER the blobs (see
            // `aurora_depth_wave`). `bw` is in luma levels, biased negative so the
            // swell rides mostly below the base and never lifts the calibrated peak.
            let lx = cx as i64 - x as i64;
            let ly = cy as i64 - y as i64;
            let bw = aurora_depth_wave(lx, ly, wsx, wsy, phase);
            // Tint the swell toward the base blue-violet: blue carries the most, red
            // the least, so brightening reads as deeper aurora light, not grey haze.
            let mut r = br + bw * 6 / 16;
            let mut g = bg + bw * 10 / 16;
            let mut bl = bb + bw * 16 / 16;
            // Additive ribbons: contribution = weight * reach²/(reach² + d²), a
            // soft 1/(1+d²) falloff that never bands. `d²` is measured in the
            // SHEARED diagonal frame so each blob is an anisotropic NW→SE ribbon
            // (§3.2 "ribbon/mesh"), and overlapping ribbons additively mix into the
            // mesh-gradient color zones. Fixed-point, no floats.
            for &(px, py, reach2, stretch, hr, hg, hb, weight) in bc.iter() {
                let dx = cx as i64 - px;
                let dy = cy as i64 - py;
                // Sheared frame: u along the NW→SE diagonal (long axis), v across it
                // (short axis). u and v are √2× the true projected distance, but the
                // common factor cancels against reach² which is calibrated to the
                // same scale (short-axis reach). Stretch the LONG axis by `stretch`.
                let u = dx + dy; // NW→SE
                let v = dx - dy; // NE→SW
                                 // d² in the ribbon frame: short axis v² full weight, long axis u²
                                 // divided down by stretch² (÷65536 since stretch is ×256). The /2
                                 // converts the √2-scaled (u,v) back to pixel² so reach² stays in px².
                let long2 = u * u / 2 * 65536 / (stretch * stretch).max(1);
                let short2 = v * v / 2;
                let d2 = long2 + short2;
                // Falloff f = reach²/(reach²+d²) in /256 fixed-point (256 at center,
                // 128 at d=reach), then SQUARED so the tail drops fast — a localized
                // ribbon with a clear bright core, not a screen-wide wash. Smooth /
                // monotonic along each axis (never bands), a 1/(1+d²)-family radial.
                let f = 256 * reach2 / (reach2 + d2); // 0..=256
                let f2 = f * f / 256; // squared, still 0..=256
                let inten = weight * f2 / 256;
                if inten == 0 {
                    continue;
                }
                // Soft-additive (screen-ish): add hue * intensity/256.
                r += hr * inten / 256;
                g += hg * inten / 256;
                bl += hb * inten / 256;
            }
            // Vignette: scale toward 0.85 at the corners.
            let ddx = cx as i64 - vcx;
            let ddy = cy as i64 - vcy;
            let vd2 = ddx * ddx + ddy * ddy;
            // factor = 1.0 - 0.15 * (vd2/vmax), in /256 fixed-point.
            let vfac = 256 - (38 * vd2 / vmax).min(38); // 0.15*256 ≈ 38
            r = r * vfac / 256;
            g = g * vfac / 256;
            bl = bl * vfac / 256;

            // ── Round-11 premium refinement #1: anti-banding ORDERED DITHER ────────
            // A deep, smooth gradient on an 8-bit panel shows visible Mach-band
            // contour rings — the one thing that instantly cheapens an otherwise
            // premium wallpaper. A 4×4 Bayer ordered dither breaks each contour by
            // nudging pixels ±1 level on a fixed sub-pixel pattern, so the 8-bit
            // quantization step is dithered below the eye's threshold (the hallmark
            // of a high-end gradient). The pattern is DETERMINISTIC (a function of
            // (cx,cy) only) so the still screenshot and the live frame agree, integer,
            // zero-alloc. Centered (−7..+8 → roughly ±1 level after >>4) so it adds no
            // net brightness — the calibrated peak is unmoved. The pattern math is
            // factored into [`aurora_dither`] so it is pinned directly by
            // `aurora_dither_is_balanced_pm1` (FAIL-able in isolation — at this
            // resolution the broad gradient always varies a raw 4×4 block by ≥1 level,
            // so a whole-frame probe could not tell dither from gradient).
            let d = aurora_dither(cx as i64, cy as i64);
            let rr = (r + d).clamp(0, 255) as u32;
            let gg = (g + d).clamp(0, 255) as u32;
            let bbv = (bl + d).clamp(0, 255) as u32;
            c.draw_pixel(cx, cy, 0xFF00_0000 | (rr << 16) | (gg << 8) | bbv);
        }
    }
}

/// Broad low-frequency depth swell for the Aurora Mesh (Round-11 premium refinement).
///
/// Returns a signed luminance offset (in 8-bit levels) for a pixel at local offset
/// `(lx, ly)` within a `(wsx × wsy)` field, drifted by `phase`. Two orthogonal
/// diagonal sinusoids at ~1.5 cycles across the screen are averaged, then biased
/// negative so the swell rides MOSTLY below the base — it deepens the field far more
/// than it lifts it, which is what keeps it from ever pushing the calibrated bright
/// blob core over the ≤150 calm-backdrop ceiling. The amplitude is a few levels: a
/// tide you feel, not a pattern you see. Pure integer (reuses the `isin` quarter-wave
/// table); no floats, no alloc.
///
/// Range: with `WAVE_AMP = 7`, the raw `(isin+isin)*7/2000` term spans roughly
/// `[-7, +7]` and the `-WAVE_AMP/2 = -3` bias shifts it to `[-10, +4]` — so the field
/// dips up to ~10 levels and lifts at most ~4. FAIL-able via
/// `aurora_depth_wave_swells_and_dips` (the swell must produce BOTH a positive and a
/// negative excursion across the field — set the amplitude to 0 and it collapses).
#[inline]
fn aurora_depth_wave(lx: i64, ly: i64, wsx: i64, wsy: i64, phase: u32) -> i64 {
    const WAVE_AMP: i64 = 7; // peak swell ±7 levels (pre-bias), a few-level breath
    let span = (wsx + wsy).max(1);
    // 1536 / span ⇒ ~1.5 cycles (1536/1024) of the period-1024 table across the
    // diagonal extent. The two phases run on opposite diagonals so their interference
    // makes the swell organic (not a single banded ramp). `phase` drifts each slowly.
    let pa = (lx + ly) * 1536 / span + phase as i64 / 3;
    let pb = (lx + wsx - ly) * 1536 / span + 200 + phase as i64 / 4;
    (isin(pa) + isin(pb)) * WAVE_AMP / 2000 - WAVE_AMP / 2
}

/// Anti-banding ordered dither for the Aurora Mesh (Round-11 premium refinement).
///
/// Returns a ±1 (or 0) luminance nudge for pixel `(cx, cy)` from a 4×4 Bayer matrix.
/// A deep smooth gradient on an 8-bit panel shows visible Mach-band contour rings —
/// the one detail that instantly cheapens an otherwise premium wallpaper. Nudging
/// each pixel ±1 on this fixed sub-pixel pattern dithers the 8-bit quantization step
/// below the eye's threshold, dissolving the bands (the hallmark of a high-end
/// gradient). DETERMINISTIC in `(cx, cy)` so the still screenshot and the live frame
/// agree; integer, zero-alloc.
///
/// The 16 Bayer thresholds (0..15) map to a centered nudge: the lowest 6 cells →−1,
/// the top 6 →+1, the middle 4 →0. The EQUAL count of −1 and +1 cells gives zero net
/// brightness over any 4×4 block, so the calibrated peak is statistically unmoved.
/// FAIL-able via `aurora_dither_is_balanced_pm1` (it must emit both a −1 and a +1, in
/// equal counts, over a 4×4 block — collapse it to a constant and the test trips).
#[inline]
fn aurora_dither(cx: i64, cy: i64) -> i64 {
    const BAYER4: [[i64; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];
    let bayer = BAYER4[(cy & 3) as usize][(cx & 3) as usize];
    if bayer < 6 {
        -1
    } else if bayer >= 10 {
        1
    } else {
        0
    }
}

/// Integer sine, period 1024, amplitude ±1000. `isin(0)=0`, `isin(256)≈+1000`
/// (90°). Small quarter-wave table + symmetry; no floats (SW-rasterizer / no_std).
fn isin(mut a: i64) -> i64 {
    // 16-entry quarter wave: sin(k*90°/16)*1000 for k=0..=16.
    const Q: [i64; 17] = [
        0, 98, 195, 290, 383, 471, 556, 634, 707, 773, 831, 881, 924, 957, 981, 995, 1000,
    ];
    a = a.rem_euclid(1024);
    let (quad, idx) = (a / 256, a % 256);
    // map idx 0..256 → table 0..16 with linear interpolation.
    let t = idx * 16; // 0..4096
    let i = (t / 256) as usize; // 0..16
    let f = t % 256;
    let lerp = |i: usize| -> i64 {
        let a0 = Q[i];
        let a1 = Q[(i + 1).min(16)];
        a0 + (a1 - a0) * f / 256
    };
    match quad {
        0 => lerp(i),
        1 => lerp(16 - i),
        2 => -lerp(i),
        _ => -lerp(16 - i),
    }
}

// ════════════════════════════════════════════════════════════════════════
// §2.3 — backdrop mean-luma sampling (for the luma auto-adjust)
// ════════════════════════════════════════════════════════════════════════

/// Sample the mean luminance (0.0..=1.0) of the framebuffer region `[x,y,w,h)` —
/// the §2.3 input. The live compositor reads this off the *blurred* backdrop
/// buffer (one extra reduction it already has); on the host-render path we read
/// the composited backdrop directly. Strided sampling (≤ ~32×32 probes) keeps it
/// O(1)-ish regardless of surface size — no per-frame full-region scan.
pub fn backdrop_mean_luma(c: &Canvas, x: usize, y: usize, w: usize, h: usize) -> f32 {
    if w == 0 || h == 0 {
        return 0.0;
    }
    let x_end = (x + w).min(c.width());
    let y_end = (y + h).min(c.height());
    if x >= x_end || y >= y_end {
        return 0.0;
    }
    let sx = ((x_end - x) / 32).max(1);
    let sy = ((y_end - y) / 32).max(1);
    let mut sum: u64 = 0;
    let mut n: u64 = 0;
    let mut py = y;
    while py < y_end {
        let mut px = x;
        while px < x_end {
            let p = c.get_pixel(px, py);
            let r = (p >> 16) & 0xFF;
            let g = (p >> 8) & 0xFF;
            let b = p & 0xFF;
            // Rec.601 luma ×256: 0.299R + 0.587G + 0.114B.
            sum += (77 * r + 150 * g + 29 * b) as u64;
            n += 1;
            px += sx;
        }
        py += sy;
    }
    if n == 0 {
        return 0.0;
    }
    // sum is luma×256 summed; mean/256/255 → 0..1.
    (sum as f32) / (n as f32) / 256.0 / 255.0
}

// ════════════════════════════════════════════════════════════════════════
// §2 + §2.4 — tiered glass + iridescent rim
// ════════════════════════════════════════════════════════════════════════

/// Stroke colors layered on top of the rim (IDENTITY.md §2.4). The top-edge
/// highlight is the macOS cue; the hairline is the crisp edge on the other sides.
const STROKE_TOP_HIGHLIGHT: u32 = 0x90_FF_FF_FF;
const STROKE_HAIRLINE: u32 = 0x30_FF_FF_FF;

/// Draw a complete tiered-glass surface (IDENTITY.md §2 + §2.4): the luma-adjusted
/// translucent slate **tint** over the existing backdrop, then the per-tier
/// **frost** white sheen ON TOP (the luminous-frost lift that turns "dark card"
/// into "frosted glass"), then the full edge stack (hairline → iridescent rim →
/// top highlight). The frost order (tint THEN frost) matches the pure-logic
/// `ath_tokens::glass_tier_interior` KAT exactly, so this draw, the kernel mirror,
/// and the host KAT all agree on the interior color. This is the single call a
/// surface makes; it never picks its own alpha — the tier + measured backdrop
/// decide.
///
/// `tier` is one of the three `ath_tokens::GLASS_*` tiers. The backdrop under the
/// surface MUST already be drawn (this composites over it). Mean-luma is sampled
/// from the framebuffer region (§2.3) and feeds `glass_luma_adjust`.
pub fn draw_glass_surface(
    c: &mut Canvas,
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    radius: usize,
    tier: GlassTier,
) {
    if w == 0 || h == 0 {
        return;
    }
    let luma = backdrop_mean_luma(c, x, y, w, h);
    let is_chrome = tier == GLASS_CHROME_DARK || tier == GLASS_CHROME_LIGHT;
    let tint = {
        let adjusted = glass_luma_adjust(tier, luma);
        if is_chrome {
            // Round-9 #3 — chrome is §7's MOST see-through tier, so it must NOT take the
            // §2.3 over-bright alpha BOOST the other tiers do: boosting chrome's tint over
            // a bright backdrop darkens it toward panel's luma (the chrome≈panel defect).
            // Keep the thin-over-dark adjustment (more see-through still) but never let the
            // chrome tint alpha exceed its base — so over a BRIGHT backdrop chrome stays
            // maximally translucent and reads LIGHTER than panel (the constant floor over
            // bright is then provided by `CHROME_INTERIOR_LUMA_CEIL`, not extra tint).
            let base_a = (tier.tint >> 24) & 0xFF;
            let adj_a = (adjusted >> 24) & 0xFF;
            ((adj_a.min(base_a)) << 24) | (adjusted & 0x00FF_FFFF)
        } else {
            adjusted
        }
    };
    // CRITICAL compositing order (matches the pure-logic `glass_tier_interior`
    // KAT exactly so the kernel mirror, host KAT, and this draw agree):
    //   blurred backdrop → slate TINT → FROST white sheen → iridescent rim.
    //
    // 1. translucent slate glass fill — the backdrop reads through this.
    c.fill_rounded_rect(x, y, w, h, radius, tint);
    // 2. FROST: a low-alpha WHITE sheen laid ON TOP of the tint (NOT before it),
    //    so the slate can't re-darken it away. This is the single change that
    //    moves "dark card" → "luminous frosted glass" (Round-3 visual-QA P0 #2),
    //    and because the per-tier frost alpha is a FIXED monotonic step
    //    (chrome 0x04 < panel 0x23 < popover 0x38) the interior luminance comes
    //    out chrome < panel < popover regardless of backdrop variance — fixing
    //    the inverted tier ordering (P1 #4).
    c.fill_rounded_rect(x, y, w, h, radius, tier.frost);
    // 2b. The luma FLOOR/cap (§2.3 + Round-9 #3). The chrome tier (§7 "most
    //     see-through") uses a HIGHER ceiling than panel/popover so that over a
    //     BRIGHT backdrop it floors LIGHTER than panel — staying the most see-through
    //     tier — while still sitting far enough below the bright field that the bar
    //     never vanishes (the Round-7 constant-floor win, now without the fixed black
    //     scrim that had over-darkened chrome to panel's luma over EVERY backdrop).
    //     Over a DARK/MID backdrop the raw interior is already below the (per-tier)
    //     ceiling so this is a NO-OP and chrome tracks the backdrop (most see-through).
    //     The unconditional white-text guarantee is the §9 WCAG pass below; this cap
    //     only shapes the see-through tier identity.
    let interior_ceil = if is_chrome {
        CHROME_INTERIOR_LUMA_CEIL
    } else {
        GLASS_INTERIOR_LUMA_CEIL
    };
    clamp_interior_luma_region(c, x, y, w, h, radius, interior_ceil);
    // 2c. Round-7 visual-QA #5 — the panel/popover frost reads grey / over-desaturated:
    //     macOS Tahoe / the reference kits let the backdrop COLOR saturate through the
    //     frost, but our heavy slate tint + neutral-white frost wash the aurora to grey.
    //     We restore chroma with a LUMA-PRESERVING saturation lift over the same interior:
    //     each pixel is pushed away from its own grey (mean channel) so whatever backdrop
    //     chroma survived the tint+frost composite is amplified, WITHOUT raising mean luma —
    //     so the §2.3 legibility cap above is NOT regressed. Skipped for chrome (its
    //     interior reads as a near-neutral see-through floor). NO-OP on an already
    //     near-neutral pixel (a dark-base region with no chroma to lift stays as it was).
    if !is_chrome {
        saturate_interior_region(c, x, y, w, h, radius);
    }
    // 2d. SHIP-GATE WCAG legibility pass (§9 — white text.primary, Round-9 a11y
    //     regression). The mean-channel luma cap (2b) and the saturation lift (2c) work
    //     in *gamma-encoded* mean-channel space, but WCAG contrast is computed on the
    //     *gamma-decoded* relative luminance — and these diverge: a SATURATED pixel held
    //     at mean-channel 0.40 can carry a true relative luminance far higher (a magenta
    //     interior measured WCAG 2.8:1 against white text at mean709=0.40, and the
    //     context-menu lower third measured 3.7–3.9:1 over the bright aurora bleed). The
    //     saturation lift in 2c makes this worse by pushing channels apart. This final
    //     pass closes the gap unconditionally: any interior pixel whose REAL WCAG
    //     contrast against `TEXT_PRIMARY_DARK` falls under the AA target is scaled
    //     uniformly toward black (hue-preserving, monotonic in relative luminance) until
    //     it clears. Over a dark backdrop every interior pixel already clears AA, so this
    //     is a NO-OP there — only the bright/saturated washout is pulled into legibility.
    clamp_interior_wcag_region(c, x, y, w, h, radius);
    // 3. hairline on all edges (outer).
    c.draw_rounded_rect_outline(x, y, w, h, radius, STROKE_HAIRLINE);
    // 4. 1px top-edge highlight (the macOS cue) — drawn last, on top.
    //    OBSIDIAN (IDENTITY-OBSIDIAN.md §2): the iridescent perimeter rim is
    //    RETIRED — on the near-black material the rainbow border read as a
    //    theme-mod, not refraction. Depth now comes from hairline + top light
    //    + the per-elevation drop shadow the caller lays underneath.
    draw_top_highlight(c, x, y, w, h, radius);
}

/// Tiered glass for an EDGE-DOCKED chrome strip (taskbar, top bars): the same
/// interior stack as [`draw_glass_surface`] (tint → frost → §2.3 luma cap → §9
/// WCAG cap), but the edge treatment is confined to the single EXPOSED edge.
///
/// Rationale (visual-QA): the full perimeter rim is designed for a *floating
/// rounded* card, where it reads as light refracting around the corners. On a
/// screen-wide radius-0 strip the same rim renders as two dead-straight neon
/// lines across the whole display — a painted border, not refraction. The
/// docked variant keeps the identity present but quiet: a hairline + a soft
/// additive top highlight + a LOW-alpha iridescent shimmer that sweeps
/// cyan → violet → warm along the exposed edge only.
///
/// `exposed_top`: true for a bottom-docked bar (the exposed edge is its top),
/// false for a top-docked bar (exposed edge = bottom).
pub fn draw_glass_surface_docked(
    c: &mut Canvas,
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    tier: GlassTier,
    exposed_top: bool,
) {
    if w == 0 || h == 0 {
        return;
    }
    let luma = backdrop_mean_luma(c, x, y, w, h);
    let is_chrome = tier == GLASS_CHROME_DARK || tier == GLASS_CHROME_LIGHT;
    let tint = {
        let adjusted = glass_luma_adjust(tier, luma);
        if is_chrome {
            let base_a = (tier.tint >> 24) & 0xFF;
            let adj_a = (adjusted >> 24) & 0xFF;
            ((adj_a.min(base_a)) << 24) | (adjusted & 0x00FF_FFFF)
        } else {
            adjusted
        }
    };
    // Interior stack — identical order to `draw_glass_surface` (radius 0).
    c.fill_rounded_rect(x, y, w, h, 0, tint);
    c.fill_rounded_rect(x, y, w, h, 0, tier.frost);
    let interior_ceil = if is_chrome {
        CHROME_INTERIOR_LUMA_CEIL
    } else {
        GLASS_INTERIOR_LUMA_CEIL
    };
    clamp_interior_luma_region(c, x, y, w, h, 0, interior_ceil);
    if !is_chrome {
        saturate_interior_region(c, x, y, w, h, 0);
    }
    clamp_interior_wcag_region(c, x, y, w, h, 0);

    // Exposed-edge stack only (OBSIDIAN: hairline + lit lip; the iridescent
    // shimmer is retired with the perimeter rim — IDENTITY-OBSIDIAN.md §2).
    let edge_y = if exposed_top { y } else { y + h - 1 };
    let x_end = (x + w).min(c.width());
    if edge_y < c.height() {
        // 1px hairline along the exposed edge.
        for px in x..x_end {
            blend_additive(c, px, edge_y, STROKE_HAIRLINE);
        }
        // Soft additive highlight on the exposed edge (the lit lip).
        for px in x..x_end {
            blend_additive(c, px, edge_y, 0x3A_FF_FF_FF);
        }
    }
}

/// Mean-channel luminance (0..1) of an opaque ARGB pixel — the SAME perceptual
/// weight `ath_tokens`'s private `mean_luma` uses for the legibility cap (Rec.709
/// 0.2126 / 0.7152 / 0.0722), so this render-side clamp lands on the identical line
/// the WCAG audit measures. `ath_tokens::mean_luma` / `clamp_interior_luma` are not
/// `pub` (the frozen catalog), so the math is mirrored here; only the CEILING
/// (`GLASS_INTERIOR_LUMA_CEIL`) is imported, keeping the threshold single-source.
#[inline]
fn pixel_mean_luma(p: u32) -> f32 {
    let r = ((p >> 16) & 0xFF) as f32;
    let g = ((p >> 8) & 0xFF) as f32;
    let b = (p & 0xFF) as f32;
    (0.2126 * r + 0.7152 * g + 0.0722 * b) / 255.0
}

/// Apply the §2.3 / §9 interior legibility cap to every pixel inside the rounded-rect
/// `[x,y,w,h)` interior: scale each pixel's RGB uniformly toward black until its
/// [`pixel_mean_luma`] ≤ `ceil`. Identical to `ath_tokens::clamp_interior_luma` but
/// per-pixel over the already-composited framebuffer (the manual tint→frost path
/// can't call the pure-logic helper because it composites in pixel space). A NO-OP
/// for any pixel already at/below the ceiling — so glass over a DARK backdrop is
/// untouched (still frosted/translucent), only the bright-backdrop washout is pulled
/// back so white text.primary clears 4.5:1. Hue-preserving (uniform scale). Runs
/// before the edge stack so the additive rim/highlight (light, not interior) stay
/// at full strength.
fn clamp_interior_luma_region(
    c: &mut Canvas,
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    radius: usize,
    ceil: f32,
) {
    let r = radius.min(w / 2).min(h / 2);
    let x_end = (x + w).min(c.width());
    let y_end = (y + h).min(c.height());
    for py in y..y_end {
        for px in x..x_end {
            // Only the surface interior (inside the rounded-rect coverage). The rim
            // band itself is clamped too — it is overdrawn additively afterward — but
            // pixels OUTSIDE the rounded corners (the transparent gutter) are skipped
            // so the backdrop bleeding past the corner is never darkened.
            if rounded_edge_dist(px, py, x, y, w, h, r) < 0 {
                continue;
            }
            let p = c.get_pixel(px, py);
            let l = pixel_mean_luma(p);
            if l <= ceil || l <= 0.0 {
                continue; // dark-backdrop no-op: already legible.
            }
            let factor = ceil / l;
            let scale = |sh: u32| -> u32 {
                let v = (((p >> sh) & 0xFF) as f32 * factor + 0.5) as u32;
                v.min(0xFF)
            };
            let rr = scale(16);
            let gg = scale(8);
            let bb = scale(0);
            c.draw_pixel(px, py, 0xFF00_0000 | (rr << 16) | (gg << 8) | bb);
        }
    }
}

/// Dark-theme white body ink (`ath_tokens::DARK.text_primary` = `#F0F2F8`) — the
/// foreground the §9 WCAG interior pass guarantees AA contrast against. Mirrored as a
/// const here (the palette field is not a `pub` standalone token) so the render-side
/// legibility pass measures against the exact ink it protects.
const TEXT_PRIMARY_DARK: u32 = 0xFF_F0_F2_F8;

/// The WCAG contrast target the interior must clear for white body text, with a small
/// safety margin over the 4.5:1 AA floor (the `powf_approx` relative-luminance math is
/// accurate to a few 1e-3, and visual-qa samples antialiased glyph edges — the margin
/// keeps the *measured* ratio above 4.5 even at the worst sample). IDENTITY.md §9.
const TEXT_AA_TARGET: f32 = 4.7;

/// SHIP-GATE §9 — the unconditional white-text legibility pass. For every interior
/// pixel, measure the REAL WCAG contrast between [`TEXT_PRIMARY_DARK`] and the
/// composited interior and, if it falls under [`TEXT_AA_TARGET`], scale the pixel
/// uniformly toward black until it clears.
///
/// This exists because the mean-channel luma cap ([`clamp_interior_luma_region`]) and
/// the saturation lift ([`saturate_interior_region`]) operate in *gamma-encoded
/// mean-channel* space, whereas WCAG contrast is computed on the *gamma-decoded
/// relative luminance* — and the two diverge for saturated/colored pixels: a magenta
/// interior held at mean-channel 0.40 measures only ≈2.8:1, and the context menu's
/// lower third over the bright aurora bleed measured 3.7–3.9:1 (the Round-9 a11y
/// regression). The saturation lift makes this worse by spreading channels (relative
/// luminance is convex in the channel values). This pass closes the gap directly on
/// the quantity WCAG measures.
///
/// Scaling all channels by a single factor `< 1` is hue-preserving and STRICTLY
/// monotone in relative luminance (each linearized channel shrinks), so contrast rises
/// monotonically as the factor drops — a short bisection on the factor lands the
/// target. Over a DARK backdrop every interior pixel already clears the target, so the
/// pass is a NO-OP there (the frosted/translucent dark look is untouched). Runs AFTER
/// the saturation lift and BEFORE the additive edge stack (the rim/highlight are light,
/// not interior text background, so they stay full strength).
fn clamp_interior_wcag_region(
    c: &mut Canvas,
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    radius: usize,
) {
    let r = radius.min(w / 2).min(h / 2);
    let x_end = (x + w).min(c.width());
    let y_end = (y + h).min(c.height());
    for py in y..y_end {
        for px in x..x_end {
            if rounded_edge_dist(px, py, x, y, w, h, r) < 0 {
                continue; // skip the transparent gutter outside the rounded corners
            }
            let p = c.get_pixel(px, py) | 0xFF00_0000;
            if ath_tokens::contrast_ratio(TEXT_PRIMARY_DARK, p) >= TEXT_AA_TARGET {
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
                if ath_tokens::contrast_ratio(TEXT_PRIMARY_DARK, q) >= TEXT_AA_TARGET {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
            let f = lo; // the largest factor that still clears the target
            let rr = ((r0 * f + 0.5) as u32).min(0xFF);
            let gg = ((g0 * f + 0.5) as u32).min(0xFF);
            let bb = ((b0 * f + 0.5) as u32).min(0xFF);
            c.draw_pixel(px, py, 0xFF00_0000 | (rr << 16) | (gg << 8) | bb);
        }
    }
}

/// Round-7 visual-QA #5 — restore backdrop chroma through the panel/popover frost.
///
/// The slate tint + neutral-white frost desaturate the aurora to grey (visual-QA:
/// "the frosted Start popover reads flat grey, not a subtle aurora tint"). macOS
/// Tahoe / the reference kits let the backdrop COLOR saturate through. We do a
/// **luma-preserving saturation lift** per interior pixel: push each channel away
/// from the pixel's own grey (its mean channel value) by `(1+gain)`, then re-scale
/// so the mean is unchanged. Because the mean (≈ the legibility luma the §2.3 cap
/// measures) is held constant, this NEVER lifts a capped bright pixel back over the
/// ceiling — text legibility is preserved while the surviving aurora chroma reads.
/// A NO-OP for a pixel already at its own grey (no chroma to amplify), so a flat
/// dark-base region is untouched. Hue-preserving (it scales the chroma vector, not
/// rotates it). Integer fixed-point (×256), no_std-safe.
fn saturate_interior_region(c: &mut Canvas, x: usize, y: usize, w: usize, h: usize, radius: usize) {
    // Saturation gain ×256: the chroma deviation from grey is multiplied by
    // (256+GAIN)/256. +40% lift pulls the surviving aurora tint back to a clearly
    // colored frost without tipping the slate glass into a neon cast (the rim is the
    // saturated accent; the body stays a subtle tint). FAIL-able via the new
    // `start_popover_frost_carries_chroma` test (drop GAIN to 0 → the test trips).
    const GAIN: i64 = 102; // 0.40 × 256
    let r = radius.min(w / 2).min(h / 2);
    let x_end = (x + w).min(c.width());
    let y_end = (y + h).min(c.height());
    for py in y..y_end {
        for px in x..x_end {
            // Same interior coverage as the luma cap: skip the transparent gutter
            // outside the rounded corners so the backdrop bleeding past is untouched.
            if rounded_edge_dist(px, py, x, y, w, h, r) < 0 {
                continue;
            }
            let p = c.get_pixel(px, py);
            let pr = ((p >> 16) & 0xFF) as i64;
            let pg = ((p >> 8) & 0xFF) as i64;
            let pb = (p & 0xFF) as i64;
            // Grey = the pixel's own mean channel (the luma-neutral pivot). Pushing
            // each channel away from this point amplifies chroma at constant mean.
            let grey = (pr + pg + pb) / 3;
            let lift = |ch: i64| -> u32 {
                // ch' = grey + (ch - grey) * (256 + GAIN) / 256, clamped to 0..255.
                (grey + (ch - grey) * (256 + GAIN) / 256).clamp(0, 255) as u32
            };
            let rr = lift(pr);
            let gg = lift(pg);
            let bb = lift(pb);
            c.draw_pixel(px, py, 0xFF00_0000 | (rr << 16) | (gg << 8) | bb);
        }
    }
}

/// The **iridescent perimeter rim** (IDENTITY.md §2.4) — THE signature. A 3px band
/// hugging the inside of the rounded-rect border whose hue sweeps cyan (top) →
/// violet (right) → warm amber (bottom/left) around the perimeter. Alpha is at the
/// in-band ceiling (`GLASS_EDGE_*` = `0x40`, ~25%) and the blend is a CHROMATIC
/// boost ([`blend_additive_boost`]) so the rim genuinely shifts the pixel's hue —
/// not merely brightens already-blue glass — making the cyan/violet/amber LEGIBLE
/// at the corners (Round-3 visual-QA measured the old 0x33/2px additive rim as ZERO
/// chromatic pixels). Perimeter-only (cheap): we only touch pixels in the
/// [1..=band] inset ring.
///
/// The three token hues interpolate linearly by angle around the perimeter, so the
/// stops blend into a continuous sweep. The chromatic-boost blend over the frosted
/// glass reads as refracted light bending at the edge, not a painted border.
pub fn draw_iridescent_rim(c: &mut Canvas, x: usize, y: usize, w: usize, h: usize, radius: usize) {
    if w < 4 || h < 4 {
        return;
    }
    let band = GLASS_EDGE_BAND_PX as usize; // 3px
    let r = radius.min(w / 2).min(h / 2);
    // Outer coverage = the full rounded rect; inner = inset by `band`. A pixel is
    // in the rim ring iff it's inside outer but outside inner. We reuse the
    // Canvas rounded-rect coverage by sampling both via the public outline trick:
    // draw nothing if the inset interior covers it.
    let cx = x as i64 * 2 + w as i64; // center ×2 (px units ×2 avoids /2 rounding)
    let cy = y as i64 * 2 + h as i64;
    let half_x = w as i64; // (w/2)×2
    let half_y = h as i64;
    let x_end = (x + w).min(c.width());
    let y_end = (y + h).min(c.height());
    let perimeter = (2 * (w + h)) as i64;

    // The rim band sits just INSIDE the dist-0 hairline (dist 1..=band) so the
    // three edge layers stay distinct: hairline (dist 0) → iridescent rim
    // (dist 1..=band) → top highlight (dist 0, top only). Otherwise the bright
    // hairline/highlight at dist 0 swamps the low-alpha chromatic rim.
    //
    // CORRECTNESS (Round-3 visual-QA P0 #1 — the old rim measured ZERO chromatic
    // pixels): a single 0x40 additive band of cyan over mid-blue glass barely
    // shifts the hue (the glass is already blue, so +cyan just reads as "more
    // blue"). Two fixes make the rim actually CHROMATIC:
    //   (a) `blend_additive_boost` weights the rim color toward its OWN dominant
    //       channels — it adds the colored light AND nudges the pixel's hue toward
    //       the rim hue — so a cyan rim genuinely reads cyan (high G+B), a violet
    //       rim reads violet (high R+B), an amber rim reads warm (high R+G).
    //   (b) the outer rim pixel carries the FULL token alpha (no feather-down at
    //       the very edge) so the corner — where visual-qa crops the 3× shot — is
    //       the most saturated point, never a whisper.
    for py in y..y_end {
        for px in x..x_end {
            // distance into the surface from the nearest edge (px), accounting for
            // the rounded corners — reuse the same SDF the fill uses.
            let dist = rounded_edge_dist(px, py, x, y, w, h, r);
            if dist < 1 || dist > band as i64 {
                continue; // outside the band (dist 0 = hairline; > band = interior)
            }
            // Position along the perimeter by angle from center → pick the hue.
            // Integer octant by (dx,dy) sign+magnitude → perimeter parameter, then
            // sweep cyan→violet→warm.
            let dx = (px as i64 * 2 - cx).clamp(-half_x, half_x);
            let dy = (py as i64 * 2 - cy).clamp(-half_y, half_y);
            let t = perimeter_param(dx, dy, half_x, half_y, perimeter); // 0..=perimeter
            let hue = rim_hue(t, perimeter);
            // Feather across the 3px band: outermost pixel at full token alpha
            // (the brightest, most-saturated edge — what the corner crop samples),
            // ramping down toward the interior so the band reads as a soft
            // refraction, not a hard painted line.
            let fade = match dist {
                1 => 256, // outer: full strength
                2 => 176,
                _ => 104, // inner edge of the band
            };
            let a = (((hue >> 24) & 0xFF) as i64 * fade / 256) as u32;
            let rim = (a << 24) | (hue & 0x00FF_FFFF);
            blend_additive_boost(c, px, py, rim);
        }
    }
}

/// 1px brighter top-edge highlight (the macOS cue, IDENTITY.md §2.4). Draws the
/// hairline color at full strength only on the top edge + top corners, fading down
/// the first few rows so it reads as a lit top lip, not a full box stroke.
fn draw_top_highlight(c: &mut Canvas, x: usize, y: usize, w: usize, h: usize, radius: usize) {
    let r = radius.min(w / 2).min(h / 2);
    let x_end = (x + w).min(c.width());
    let y_end = (y + (r + 2)).min(c.height()); // only the top lip
    for py in y..y_end {
        for px in x..x_end {
            let dist = rounded_edge_dist(px, py, x, y, w, h, r);
            if dist != 0 {
                continue; // only the 1px outer ring
            }
            // Only the TOP half of the perimeter (dy < 0) gets the bright lip.
            let cy = y as i64 * 2 + h as i64;
            if py as i64 * 2 >= cy {
                continue;
            }
            blend_additive(c, px, py, STROKE_TOP_HIGHLIGHT);
        }
    }
}

/// Chromatic rim blend (§2.4 P0 #1) — like [`blend_additive`] but it makes the
/// rim genuinely shift the pixel's HUE toward the rim color, not just brighten it.
///
/// Plain additive of a cyan light over already-blue glass reads as "more blue,"
/// never "cyan" (the glass body dominates) — which is exactly why the old rim
/// measured zero chromatic pixels. The fix: per channel, take the additive sum BUT
/// blend it `a/255` of the way toward `max(dst+add, src_channel_scaled)` so the
/// rim's OWN dominant channels (cyan: G,B / violet: R,B / amber: R,G) are pulled up
/// toward the rim's saturated level, while its weak channel is only lightly added.
/// The result still reads as bright refracted light (additive-leaning) yet the hue
/// at the rim is unmistakably the stop's hue. Integer, no_std-safe.
#[inline]
fn blend_additive_boost(c: &mut Canvas, x: usize, y: usize, src: u32) {
    let a = ((src >> 24) & 0xFF) as i64;
    if a == 0 {
        return;
    }
    let dst = c.get_pixel(x, y);
    let chan =
        |sh: u32| -> (i64, i64) { (((src >> sh) & 0xFF) as i64, ((dst >> sh) & 0xFF) as i64) };
    let (sr, dr) = chan(16);
    let (sg, dg) = chan(8);
    let (sb, db) = chan(0);
    // The rim hue's per-channel STRENGTH relative to its own brightest channel —
    // this is the hue's "shape" (cyan: R weak, G/B strong; violet: G weak, R/B
    // strong; amber: B weak, R/G strong). Imposing this shape on the pixel is what
    // makes the rim read as its hue instead of just "brighter blue".
    let smax = sr.max(sg).max(sb).max(1);
    // The rim is THE signature, and the corner is where visual-qa crops — so at the
    // edge we impose the hue HARD. Boost the composite weight (≈3× the token alpha,
    // capped) so the pixel is pulled most of the way to the saturated rim hue,
    // making cyan/violet/amber unmistakable. (The 3px feather already ramps this
    // down toward the interior, so the surface body stays calm.)
    let w = (a * 3).min(255);
    // Per channel: alpha-composite the pixel toward a SATURATED version of the rim
    // hue (the rim channel scaled to full 0..255 by its own shape), so weak rim
    // channels pull the pixel DOWN there while strong channels push it up — a real
    // hue shift, not just "brighter blue".
    let mix = |s: i64, d: i64| -> i64 {
        let target = s * 255 / smax; // 0..255, =255 for the rim's dominant channel
        (d + (target - d) * w / 255).clamp(0, 255)
    };
    let r = mix(sr, dr) as u32;
    let g = mix(sg, dg) as u32;
    let b = mix(sb, db) as u32;
    c.draw_pixel(x, y, 0xFF00_0000 | (r << 16) | (g << 8) | b);
}

/// Additive (screen-ish) blend of `src` ARGB onto (x,y): `dst += src*a/255`,
/// clamped. The rim/highlight read as light refraction, not paint (§2.4).
#[inline]
fn blend_additive(c: &mut Canvas, x: usize, y: usize, src: u32) {
    let a = ((src >> 24) & 0xFF) as i64;
    if a == 0 {
        return;
    }
    let dst = c.get_pixel(x, y);
    let sr = ((src >> 16) & 0xFF) as i64;
    let sg = ((src >> 8) & 0xFF) as i64;
    let sb = (src & 0xFF) as i64;
    let dr = ((dst >> 16) & 0xFF) as i64;
    let dg = ((dst >> 8) & 0xFF) as i64;
    let db = (dst & 0xFF) as i64;
    let r = (dr + sr * a / 255).min(255) as u32;
    let g = (dg + sg * a / 255).min(255) as u32;
    let b = (db + sb * a / 255).min(255) as u32;
    c.draw_pixel(x, y, 0xFF00_0000 | (r << 16) | (g << 8) | b);
}

/// Distance (in px) of pixel (px,py) from the nearest point on the rounded-rect
/// border of `[x,y,w,h)` with corner radius `r`, measured INWARD. Returns the
/// inset depth (0 at the border, growing toward the interior); negative if the
/// pixel is outside the surface. Integer octagonal SDF (no sqrt) — matches the
/// fill's `rr_coverage` geometry so the rim hugs the same edge the fill draws.
fn rounded_edge_dist(
    px: usize,
    py: usize,
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    r: usize,
) -> i64 {
    let half_x = (w / 2) as i64;
    let half_y = (h / 2) as i64;
    let cx = x as i64 + half_x;
    let cy = y as i64 + half_y;
    let r = r as i64;
    // distance from center, minus the straight-edge extent, in each axis.
    let ax = (px as i64 - cx).abs() - (half_x - r);
    let ay = (py as i64 - cy).abs() - (half_y - r);
    let ax = ax.max(0);
    let ay = ay.max(0);
    // octagonal approx of euclidean length(ax,ay): max + min/2.
    let (mx, mn) = if ax >= ay { (ax, ay) } else { (ay, ax) };
    let corner = mx + mn / 2;
    // signed distance to the border: inside-edge offset.
    // For straight edges (ax=ay=0): nearest border = min over axes of (half - |..|).
    let edge_straight = {
        let ex = half_x - (px as i64 - cx).abs();
        let ey = half_y - (py as i64 - cy).abs();
        ex.min(ey)
    };
    if ax == 0 && ay == 0 {
        // straight region: depth from the nearest straight edge.
        edge_straight
    } else {
        // corner region: depth = r - corner (0 at the arc, grows inward).
        r - corner
    }
}

/// Map a (dx,dy) direction from the surface center to a perimeter parameter in
/// `0..=perimeter`, walking clockwise from the top-left corner: top (left→right) →
/// right (top→bottom) → bottom (right→left) → left (bottom→top). Integer (no atan):
/// we project onto the rectangle's perimeter by which edge the ray exits through.
///
/// Round-4 visual-QA P1 #3 fix — the OLD projection mis-mapped the edges (the RIGHT
/// edge landed in the warm→cyan sector and the BOTTOM/LEFT edges collapsed onto the
/// clamped perimeter end), so the per-perimeter hue interpolation only ever sampled
/// the cyan half of the cycle (measured cyan 13,398 / violet 444 / warm 36). The fix
/// walks all four edges in EQUAL `t`-quarters so each edge can carry its own hue:
///   - the edge is chosen by the dominant normalized axis (|dx|/half_x vs
///     |dy|/half_y, cross-multiplied to avoid division), so a pixel on the rim band
///     is attributed to the edge it hugs, not a clamped corner;
///   - each edge maps to exactly ONE quarter of `perimeter` using its WITHIN-EDGE
///     fraction (0..1 along that edge), NOT its pixel length.
///
/// Round-6 visual-QA fix (the tall-rect warm-amber gap) — the OLD version scaled the
/// raw walk (whose edge spans are the PIXEL lengths 2·half_x / 2·half_y) by
/// `perimeter/total`, so on a NON-square rect the four edges took UNEQUAL `t` spans:
/// a tall right-docked Control Center (~360×760) gave its short top/bottom edges only
/// ~16 % of `t` each while left/right took ~33 %. [`rim_hue`] anchors its hue stops at
/// the fixed perimeter MIDPOINTS (1/8, 3/8, 5/8, 7/8) assuming four EQUAL quarters, so
/// on a tall rect the warm-amber peak (5/8) drifted off the short bottom edge and the
/// bottom carried only a thin warm sliver (measured ~29–63 px, L28–33) — the cyan→
/// violet→amber sweep finished only 2/3 of the way round. Mapping each edge to a full
/// equal quarter by its own 0..1 fraction puts every edge's CENTER on its hue stop's
/// midpoint for ANY aspect ratio, so the bottom edge of a tall rect gets the SAME full
/// warm-amber stop a square tile does (only its pixel count scales with edge length).
/// `dx`/`dy` arrive in ×2 units (caller's `px*2 - cx`), clamped to `±half_x/±half_y`.
fn perimeter_param(dx: i64, dy: i64, half_x: i64, half_y: i64, perimeter: i64) -> i64 {
    let hx = half_x.max(1);
    let hy = half_y.max(1);
    // Each edge occupies exactly one quarter of the perimeter parameter, regardless
    // of its pixel length — this is what keeps the warm-amber (and every) stop pinned
    // to its edge's center on tall/wide rects, not just squares.
    let q = (perimeter / 4).max(1);
    // The full extent of an edge in the incoming ×2 units: a top/bottom edge spans
    // 2·half_x across `dx`; a left/right edge spans 2·half_y across `dy`. `frac` maps
    // a 0..extent offset to 0..q (the within-edge fraction × the quarter span).
    let frac = |num: i64, extent: i64| -> i64 { num.clamp(0, extent) * q / extent.max(1) };
    // Dominant axis: compare |dx|/half_x vs |dy|/half_y without dividing.
    let cmpx = dx.abs() * hy;
    let cmpy = dy.abs() * hx;
    let raw = if cmpy >= cmpx {
        // vertical-dominant → top or bottom edge.
        if dy < 0 {
            frac(dx + half_x, 2 * half_x) // top, left→right: 0..q
        } else {
            2 * q + frac(half_x - dx, 2 * half_x) // bottom, right→left: 2q..3q
        }
    } else {
        // horizontal-dominant → right or left edge.
        if dx > 0 {
            q + frac(dy + half_y, 2 * half_y) // right, top→bottom: q..2q
        } else {
            3 * q + frac(half_y - dy, 2 * half_y) // left, bottom→top: 3q..4q
        }
    };
    raw.clamp(0, perimeter)
}

/// Saturate the warm-amber rim stop toward genuine gold for APPLICATION (Round-5
/// visual-QA P1: the bottom edge read pink-lilac; Round-9 #2: the bottom edge over a
/// bright TEAL/green aurora region read green, not warm — R never clearly beat G/B so
/// the amber classifier (and the eye) measured ZERO warm pixels on real surfaces).
/// The token (`GLASS_EDGE_WARM` = #FFC97C) is unchanged — this only shapes how the
/// boost PAINTS it so the bottom edge reads unmistakably GOLD (R > G > B) over ANY
/// backdrop, not just the isolated blue-slate demo tile:
///   * red pinned to FULL (0xFF) so red is the decisive maximum even when the boost
///     composites over a green/teal backdrop pixel that would otherwise keep G on top;
///   * green held BELOW red with a clear gap (amber, R≫G — not yellow R≈G), and not
///     lifted past the backdrop, so a bright-green backdrop can't push the composite
///     into "green" territory;
///   * blue cut HARD so amber decisively beats both the blue slate glass and the
///     violet corner bleed (kills the lilac read).
/// The result is a saturated gold rim stop that classifies warm (R≥B+24 ∧ G≥B+12 ∧
/// R>G) across the real aurora bottom-edge backdrops. Alpha is preserved. (A deeper
/// change in the catalog itself is a `ath_tokens` edit — NOTE it to athena-ui.)
#[inline]
fn warm_amber_boost(stop: u32) -> u32 {
    let a = (stop >> 24) & 0xFF;
    // Pin red to FULL: red must be the decisive channel so the additive-boost imposes a
    // gold (not yellow-green) hue even over a bright green/teal backdrop pixel.
    let r = 0xFFu32;
    // Green a clear step below red (amber R≫G), nudged just off the token value.
    let g = ((stop >> 8) & 0xFF).clamp(0xB8, 0xCC);
    // Cut blue hard so the stop is decisively warm, not lilac/teal.
    let b = ((stop & 0xFF) * 1 / 2).min(0x46); // 0x7C/2 ≈ 0x3E
    (a << 24) | (r << 16) | (g << 8) | b
}

/// Interpolate the rim hue at perimeter parameter `t` (0..=perimeter): the three
/// `GLASS_EDGE_*` stops placed at the EDGE MIDPOINTS so each edge reads its own pure
/// hue — cyan(top) → violet(right) → warm(bottom) → back to cyan(left) — blended
/// linearly across the corners so the low-alpha stops read as a continuous
/// iridescent sweep. Preserves each stop's token alpha.
///
/// Round-4 visual-QA P1 #3 fix — the OLD version placed stops at the perimeter
/// ORIGINS (t=0, 1/3, 2/3) which (combined with the broken `perimeter_param`) put
/// cyan across the whole top edge and pushed violet/warm into the clamped tail, so
/// the rim measured cyan-monochrome (violet 444 / warm 36). Anchoring the stops at
/// the edge CENTERS — cyan@1/8, violet@3/8, warm@5/8, cyan@7/8 (the four edge
/// midpoints on the top→right→bottom→left walk) — makes the TOP edge render cyan,
/// the RIGHT edge violet, and the BOTTOM edge warm-amber, with the corners blending
/// between adjacent hues. The sweep is now the full chromatic cycle the gold
/// reference shows, not a cyan line.
fn rim_hue(t: i64, perimeter: i64) -> u32 {
    let p = perimeter.max(1);
    // Shift the parameter so the first stop (cyan, the top-edge midpoint at p/8)
    // sits at 0, then split the loop into four equal sectors of p/4 — each sector
    // spans one edge-midpoint to the next, blending its two hues across the corner.
    let s = (t - p / 8).rem_euclid(p);
    let q = (p / 4).max(1);
    let sector = (s / q).min(3);
    let local = s - sector * q;
    // Round-5 visual-QA P1 — the bottom edge read pink-lilac, not amber: the warm
    // token (#FFC97C) is genuinely amber, but composited over the strongly-blue slate
    // glass (and corner-blended with the violet stop, B=0xFF) its blue channel stays
    // high enough that the bottom edge reads magenta/lilac rather than gold. We do NOT
    // change the token (it stays #FFC97C for the catalog / KAT); instead we SHAPE the
    // warm stop's APPLICATION here — deepen the gold by lifting green slightly and
    // cutting blue hard so warm-amber wins against the blue glass — exactly the
    // hue-boost the job asks for. Alpha is preserved from the token.
    let warm = warm_amber_boost(GLASS_EDGE_WARM);
    let (a, b) = match sector {
        0 => (GLASS_EDGE_CYAN, GLASS_EDGE_VIOLET), // top-mid → right-mid (TR corner)
        1 => (GLASS_EDGE_VIOLET, warm),            // right-mid → bottom-mid (BR corner)
        2 => (warm, GLASS_EDGE_CYAN),              // bottom-mid → left-mid (BL corner)
        _ => (GLASS_EDGE_CYAN, GLASS_EDGE_CYAN),   // left-mid → top-mid (TL corner, cyan)
    };
    lerp_argb(a, b, local, q)
}

/// Linear ARGB interpolation a→b by `num/den` (0..=1), all channels incl. alpha.
fn lerp_argb(a: u32, b: u32, num: i64, den: i64) -> u32 {
    let f = num.clamp(0, den);
    let ch = |sh: u32| -> u32 {
        let av = ((a >> sh) & 0xFF) as i64;
        let bv = ((b >> sh) & 0xFF) as i64;
        ((av + (bv - av) * f / den).clamp(0, 255) as u32) << sh
    };
    ch(24) | ch(16) | ch(8) | ch(0)
}

// ════════════════════════════════════════════════════════════════════════
// FAIL-able host tests (IDENTITY.md §8.7 — rim/aurora math)
// ════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    fn fb(w: usize, h: usize) -> (alloc::vec::Vec<u32>, usize, usize) {
        (alloc::vec![0u32; w * h], w, h)
    }

    /// IDENTITY.md §8.7 — the iridescent rim stays in the subtle band. Every hue
    /// produced around the perimeter must carry an alpha in [0x20,0x40] (a neon
    /// rim = FAIL). We sweep the whole perimeter parameter and check every sample.
    #[test]
    fn rim_alpha_stays_subtle() {
        let perimeter = 2 * (400 + 300);
        let mut max_a = 0u32;
        let mut min_a = 0xFFu32;
        for t in 0..perimeter {
            let hue = rim_hue(t as i64, perimeter as i64);
            let a = (hue >> 24) & 0xFF;
            max_a = max_a.max(a);
            min_a = min_a.min(a);
        }
        // Token stops are all 0x40 (the in-band ceiling) → the interpolation must
        // stay within [0x20,0x40]. FAIL-ability: bump any GLASS_EDGE_* alpha above
        // the 0x40 ceiling → this trips.
        assert!(
            (0x20..=0x40).contains(&min_a) && (0x20..=0x40).contains(&max_a),
            "rim alpha left the subtle band: min={min_a:#x} max={max_a:#x}"
        );
    }

    /// Round-3 visual-QA P0 #1 — the iridescent rim must render MEASURABLE
    /// chromatic pixels (the old additive rim measured ZERO). We draw a glass panel
    /// over the aurora, then count rim-band pixels whose hue genuinely shifted
    /// toward cyan (top, G&B ≫ R), violet (right, R&B up vs glass), and warm-amber
    /// (bottom, R&G ≫ B). Each family must have a non-trivial count. FAIL-ability:
    /// revert `blend_additive_boost` to plain additive and the cyan/amber counts
    /// collapse toward zero (the bug this test pins).
    #[test]
    fn rim_renders_chromatic_pixels() {
        // OBSIDIAN (IDENTITY-OBSIDIAN.md §2): the iridescent perimeter rim is
        // RETIRED on surfaces — on the near-black material the rainbow border
        // read as a theme-mod, not refraction. Contract, both directions:
        //   (a) `draw_glass_surface` paints ZERO chromatic rim pixels in the
        //       edge band (a regression re-adding the rim call trips this);
        //   (b) the `draw_iridescent_rim` PRIMITIVE still renders when called
        //       directly (it remains shipped for the atom sheet + theming).
        let (sx, sy, sw, sh, rad) = (40usize, 40usize, 280usize, 280usize, 24usize);
        let count_chromatic = |px: &[u32], w: usize| -> u32 {
            let mut chroma = 0u32;
            for y in sy..sy + sh {
                for x in sx..sx + sw {
                    let dist = rounded_edge_dist(x, y, sx, sy, sw, sh, rad.min(sw / 2).min(sh / 2));
                    if dist < 1 || dist > GLASS_EDGE_BAND_PX as i64 {
                        continue;
                    }
                    let p = px[y * w + x];
                    let r = ((p >> 16) & 0xFF) as i64;
                    let g = ((p >> 8) & 0xFF) as i64;
                    let b = (p & 0xFF) as i64;
                    // Any strongly-hued rim pixel (cyan / violet / amber reads).
                    if (g >= r + 30 && b >= r + 30)
                        || (r >= g + 12 && b >= g + 24)
                        || (r >= b + 24 && g >= b + 12)
                    {
                        chroma += 1;
                    }
                }
            }
            chroma
        };

        // (a) The SURFACE carries no rim.
        let (mut px, w, h) = fb(360, 360);
        {
            let mut c = unsafe { Canvas::new(px.as_mut_ptr() as *mut u8, w, h, 4) };
            render_aurora_dark(&mut c, 0, 0, w, h, 0);
            draw_glass_surface(&mut c, sx, sy, sw, sh, rad, ath_tokens::GLASS_PANEL_DARK);
        }
        let surface_chroma = count_chromatic(&px, w);
        assert!(
            surface_chroma < 40,
            "obsidian surfaces must carry NO iridescent rim — found {surface_chroma} \
             chromatic edge px (a rim call regressed into draw_glass_surface)"
        );

        // (b) The primitive still works when invoked directly.
        let (mut px2, w2, _h2) = fb(360, 360);
        {
            let mut c = unsafe { Canvas::new(px2.as_mut_ptr() as *mut u8, w2, 360, 4) };
            render_aurora_dark(&mut c, 0, 0, w2, 360, 0);
            c.fill_rounded_rect(sx, sy, sw, sh, rad, ath_tokens::GLASS_PANEL_DARK.tint);
            draw_iridescent_rim(&mut c, sx, sy, sw, sh, rad);
        }
        let primitive_chroma = count_chromatic(&px2, w2);
        assert!(
            primitive_chroma > 200,
            "the draw_iridescent_rim PRIMITIVE must still render chromatic pixels \
             when called directly (got {primitive_chroma}) — it ships for theming"
        );
    }

    /// Round-3 visual-QA P1 #4 — the rendered tier interiors must come out
    /// monotonic chrome < panel < popover in luminance over the SAME backdrop
    /// patch, matching the pure-logic `glass_tier_interior` ordering. We render the
    /// three tiers over a flat patch of the aurora base and read back their centers.
    /// FAIL-ability: drop popover's frost below panel's and the order inverts.
    #[test]
    fn rendered_tier_interiors_are_monotonic() {
        use ath_tokens::{GLASS_CHROME_DARK, GLASS_PANEL_DARK, GLASS_POPOVER_DARK};
        let (mut px, w, h) = fb(360, 160);
        {
            let mut c = unsafe { Canvas::new(px.as_mut_ptr() as *mut u8, w, h, 4) };
            // Flat backdrop patch so the ONLY variable is the tier (no aurora
            // variance to swamp the steps — the exact failure mode in the critique).
            c.fill_rect(0, 0, w, h, WALLPAPER_AURORA_BASE_DARK);
            draw_glass_surface(&mut c, 10, 30, 100, 100, 16, GLASS_CHROME_DARK);
            draw_glass_surface(&mut c, 130, 30, 100, 100, 16, GLASS_PANEL_DARK);
            draw_glass_surface(&mut c, 250, 30, 100, 100, 16, GLASS_POPOVER_DARK);
        }
        // Sample each interior center (well inside the rim band).
        let luma_at = |cx: usize, cy: usize| -> i64 {
            let p = px[cy * w + cx];
            let r = ((p >> 16) & 0xFF) as i64;
            let g = ((p >> 8) & 0xFF) as i64;
            let b = (p & 0xFF) as i64;
            77 * r + 150 * g + 29 * b
        };
        let chrome = luma_at(60, 80);
        let panel = luma_at(180, 80);
        let popover = luma_at(300, 80);
        assert!(
            chrome < panel && panel < popover,
            "rendered tier interiors not monotonic: chrome={chrome} panel={panel} popover={popover}"
        );
    }

    /// Round-3 visual-QA P1 #3 + Round-5 P1 — the aurora's brightest pixel must reach
    /// the target luminance band but stay CALM: ≥120 (not underexposed at ~94 like the
    /// old render) AND ≤150 (Round-5 visual-QA flagged the bare-wallpaper peak at L166
    /// as "too hot under the demo panels" — a calm backdrop tops out ~150). We render
    /// the full wallpaper and find the peak per-pixel luma. FAIL-ability: drop the blue
    /// blob weight back toward 94 and it falls under 120; restore the violet/teal-seam
    /// weights to their pre-trim 150/120/72 and the peak climbs back over 150.
    #[test]
    fn aurora_peak_luma_in_target_band() {
        let (mut px, w, h) = fb(640, 400);
        {
            let mut c = unsafe { Canvas::new(px.as_mut_ptr() as *mut u8, w, h, 4) };
            render_aurora_dark(&mut c, 0, 0, w, h, 0);
        }
        let mut peak = 0i64;
        for &p in px.iter() {
            let r = ((p >> 16) & 0xFF) as i64;
            let g = ((p >> 8) & 0xFF) as i64;
            let b = (p & 0xFF) as i64;
            // Rec.601 luma 0..255.
            let l = (77 * r + 150 * g + 29 * b) / 256;
            peak = peak.max(l);
        }
        assert!(
            (120..=150).contains(&peak),
            "aurora peak luma {peak} outside the calm-backdrop band [120,150] \
             (under-exposed <120, or too hot >150 — Round-5 P1)"
        );
    }

    /// IDENTITY.md §3.2 — the aurora blob falloff is monotonic from a blob center
    /// outward (a `1/(1+d²)` radial never brightens as you move away). We render
    /// the aurora and sample luma along a radial from the primary blob center; it
    /// must be non-increasing (allowing a small tolerance for the other blobs +
    /// integer rounding). A banded/ringing falloff = FAIL.
    #[test]
    fn aurora_falloff_monotonic_from_center() {
        let (mut px, w, h) = fb(400, 400);
        {
            let mut c = unsafe { Canvas::new(px.as_mut_ptr() as *mut u8, w, h, 4) };
            render_aurora_dark(&mut c, 0, 0, w, h, 0);
        }
        // Primary (blue) blob center ≈ (0.34w, 0.36h). Walk straight up toward the
        // top edge (away from the violet/teal ribbons, which sit right/lower) and
        // confirm luma never *increases* by more than a small rounding tolerance.
        // The blob is a NW→SE ribbon, so the up-walk crosses its gentle long axis —
        // still monotonic, just softer.
        let bx = (0.34 * w as f32) as usize;
        let by = (0.36 * h as f32) as usize;
        let luma_at = |p: u32| -> i64 {
            let r = ((p >> 16) & 0xFF) as i64;
            let g = ((p >> 8) & 0xFF) as i64;
            let b = (p & 0xFF) as i64;
            77 * r + 150 * g + 29 * b
        };
        let mut prev = luma_at(px[by * w + bx]);
        let mut yy = by;
        while yy > 4 {
            yy -= 4;
            let cur = luma_at(px[yy * w + bx]);
            assert!(
                cur <= prev + 6 * 256, // tolerance for the off-axis blobs + rounding
                "aurora brightened away from center at y={yy}: cur={cur} prev={prev}"
            );
            prev = cur;
        }
        // Sanity: the center must actually be brighter than the corner (else the
        // blob did nothing — a silent regression). FAIL-able.
        let corner = luma_at(px[2 * w + 2]);
        let center = luma_at(px[by * w + bx]);
        assert!(
            center > corner + 4 * 256,
            "aurora center not brighter than corner: center={center} corner={corner}"
        );
    }

    /// The aurora must NOT be a flat void: across the whole frame there must be a
    /// real luma spread (max ≫ min). A flat fill (the old navy gradient defect)
    /// would FAIL this.
    #[test]
    fn aurora_is_not_a_flat_void() {
        let (mut px, w, h) = fb(320, 240);
        {
            let mut c = unsafe { Canvas::new(px.as_mut_ptr() as *mut u8, w, h, 4) };
            render_aurora_dark(&mut c, 0, 0, w, h, 0);
        }
        let mut lo = i64::MAX;
        let mut hi = i64::MIN;
        for &p in px.iter() {
            let l = (((p >> 16) & 0xFF) + ((p >> 8) & 0xFF) + (p & 0xFF)) as i64;
            lo = lo.min(l);
            hi = hi.max(l);
        }
        assert!(
            hi - lo > 60,
            "aurora has no color spread (hi={hi} lo={lo}) — reads as a flat void"
        );
    }

    /// Round-11 premium refinement #2 — the BROAD depth wave gives the field between
    /// the blobs an organic, non-uniform swell (the "two circles on a flat gradient"
    /// fix). Pinned directly on [`aurora_depth_wave`] (not through the rendered frame,
    /// where the strong blob field swamps the few-level swell): across a screen-scale
    /// field the swell must produce BOTH a clearly negative excursion (the field dips
    /// below the base — the depth) AND a positive one (the field lifts — the swell),
    /// i.e. it is a real low-frequency wave, not a constant. FAIL-ability: set
    /// `WAVE_AMP` to 0 and `aurora_depth_wave` returns the constant `0` everywhere
    /// (no min < −1, no max > +1), tripping both assertions.
    #[test]
    fn aurora_depth_wave_swells_and_dips() {
        let (wsx, wsy) = (640i64, 400i64);
        let mut lo = i64::MAX;
        let mut hi = i64::MIN;
        // Sample the whole field on a coarse grid (the wave is ~1.5 cycles across, so
        // a 16×16 grid catches at least one trough and one crest).
        for gy in 0..16 {
            for gx in 0..16 {
                let lx = gx * wsx / 16;
                let ly = gy * wsy / 16;
                let v = aurora_depth_wave(lx, ly, wsx, wsy, 0);
                lo = lo.min(v);
                hi = hi.max(v);
            }
        }
        assert!(
            lo <= -3,
            "aurora depth wave never dips (min={lo}) — the field has no deepening \
             swell; it reads as a flat gradient between the blobs"
        );
        assert!(
            hi >= 1,
            "aurora depth wave never swells (max={hi}) — the field has no lifting \
             swell; the broad low-frequency variation is missing"
        );
        // And it must stay SUBTLE — a few levels, never a loud band that would fight
        // the blobs or blow the calm-backdrop ceiling.
        assert!(
            hi - lo <= 16,
            "aurora depth wave amplitude {} too loud (>16 levels) — not a calm tide",
            hi - lo
        );
    }

    /// Round-11 premium refinement #1 — the ordered DITHER breaks 8-bit banding.
    /// Pinned directly on [`aurora_dither`] (at render resolution the broad gradient
    /// already varies any raw 4×4 block by ≥1 level, so a frame probe cannot separate
    /// dither from gradient). Over one 4×4 Bayer cell the nudge must (a) emit BOTH a
    /// −1 and a +1 — so it genuinely breaks contours — and (b) be BALANCED: the −1 and
    /// +1 counts equal, so it adds zero net brightness (the calibrated peak is
    /// unmoved). FAIL-ability: collapse `aurora_dither` to a constant `0` and the
    /// "emits a −1" / "emits a +1" assertions trip; bias it (e.g. drop the −1 branch)
    /// and the balance assertion trips.
    #[test]
    fn aurora_dither_is_balanced_pm1() {
        let (mut neg, mut zero, mut pos) = (0i32, 0i32, 0i32);
        let mut sum = 0i64;
        for cy in 0..4i64 {
            for cx in 0..4i64 {
                let d = aurora_dither(cx, cy);
                assert!(
                    (-1..=1).contains(&d),
                    "dither nudge {d} left ±1 (would be visible)"
                );
                match d {
                    -1 => neg += 1,
                    0 => zero += 1,
                    _ => pos += 1,
                }
                sum += d;
            }
        }
        assert!(
            neg > 0 && pos > 0,
            "dither emits no contour-breaking nudge (neg={neg} pos={pos}) — banding survives"
        );
        assert_eq!(neg, pos, "dither is unbalanced (neg={neg} pos={pos}) — it shifts mean brightness, moving the calibrated peak");
        assert_eq!(
            sum, 0,
            "dither net brightness over a 4x4 cell != 0 (sum={sum})"
        );
        // The pattern must also actually VARY pixel-to-pixel (zeros alone wouldn't).
        assert!(
            zero < 16,
            "dither is all-zero — no anti-banding effect at all"
        );
    }

    /// §2.3 mean-luma sampling: over the deep aurora base a panel reads as a dark
    /// backdrop (< GLASS_LUMA_LO) so the luma-adjust THINS the glass — proves the
    /// sampler + adjust are wired and directionally correct.
    #[test]
    fn glass_thins_over_dark_aurora() {
        let (mut px, w, h) = fb(300, 200);
        {
            let mut c = unsafe { Canvas::new(px.as_mut_ptr() as *mut u8, w, h, 4) };
            // Fill with the bare aurora base (very dark).
            c.fill_rect(0, 0, w, h, WALLPAPER_AURORA_BASE_DARK);
            let luma = backdrop_mean_luma(&c, 40, 40, 120, 90);
            assert!(
                luma < ath_tokens::GLASS_LUMA_LO,
                "aurora base must read dark: {luma}"
            );
            let adj = glass_luma_adjust(ath_tokens::GLASS_PANEL_DARK, luma);
            let base_a = (ath_tokens::GLASS_PANEL_DARK.tint >> 24) & 0xFF;
            let adj_a = (adj >> 24) & 0xFF;
            assert!(
                adj_a < base_a,
                "glass must thin over dark backdrop: {adj_a:#x} vs {base_a:#x}"
            );
        }
    }

    /// SHIP-GATE (§2.3 / §9 white-text legibility) — the RENDERED glass interior must
    /// be CAPPED over a BRIGHT backdrop (so white text.primary clears 4.5:1) AND a
    /// NO-OP over a DARK backdrop (the frosted/translucent look is untouched). This
    /// pins the bug athena-ui found: the WCAG audit calls the capped `glass_tier_interior`
    /// but `draw_glass_surface` composited tint→frost manually and never applied the
    /// cap, so the pixels stayed illegible. FAIL-ability: delete the
    /// `clamp_interior_luma_region` call in `draw_glass_surface` and the bright case
    /// jumps back over the ceiling (and the dark case is unaffected, so removing the
    /// cap is detected ONLY by the bright assertion — exactly the shipped bug).
    #[test]
    fn glass_interior_capped_over_bright_not_over_dark() {
        let mean_luma = |p: u32| -> f32 {
            let r = ((p >> 16) & 0xFF) as f32;
            let g = ((p >> 8) & 0xFF) as f32;
            let b = (p & 0xFF) as f32;
            (0.2126 * r + 0.7152 * g + 0.0722 * b) / 255.0
        };
        let ceil = GLASS_INTERIOR_LUMA_CEIL;

        // ── BRIGHT backdrop: the brightest aurora-blob color, filled flat so the
        //    sampled mean is unambiguously high → the cap must engage. We use a
        //    popover (the thickest frost, the worst washout) parked over it.
        let (mut bpx, bw, bh) = fb(200, 200);
        {
            let mut c = unsafe { Canvas::new(bpx.as_mut_ptr() as *mut u8, bw, bh, 4) };
            // A bright near-white-warm field (an over-exposed photo / aurora peak).
            c.fill_rect(0, 0, bw, bh, 0xFF_E8_E2_C8);
            draw_glass_surface(&mut c, 20, 20, 160, 160, 16, ath_tokens::GLASS_POPOVER_DARK);
        }
        let bright_interior = mean_luma(bpx[100 * bw + 100]);
        assert!(
            bright_interior <= ceil + 0.01,
            "glass interior over a BRIGHT backdrop not capped: mean luma {bright_interior} \
             > ceiling {ceil} — white text.primary would drop below 4.5:1 (the shipped bug)"
        );

        // ── DARK backdrop: the bare aurora base → the interior is already below the
        //    ceiling, so the clamp must be a NO-OP. We render WITH and WITHOUT the cap
        //    by comparing against the raw tint→frost flatten the token KAT defines:
        //    the rendered center must equal the uncapped raw interior (untouched).
        let (mut dpx, dw, dh) = fb(200, 200);
        {
            let mut c = unsafe { Canvas::new(dpx.as_mut_ptr() as *mut u8, dw, dh, 4) };
            c.fill_rect(0, 0, dw, dh, WALLPAPER_AURORA_BASE_DARK);
            draw_glass_surface(&mut c, 20, 20, 160, 160, 16, ath_tokens::GLASS_POPOVER_DARK);
        }
        let dark_interior = mean_luma(dpx[100 * dw + 100]);
        // The raw (uncapped) interior the token ladder produces over the same base.
        let raw = ath_tokens::glass_tier_interior_raw(
            ath_tokens::GLASS_POPOVER_DARK,
            WALLPAPER_AURORA_BASE_DARK,
        );
        let raw_luma = mean_luma(raw);
        assert!(
            raw_luma <= ceil,
            "test precondition: dark interior must already be below the ceiling \
             (raw_luma={raw_luma} ceil={ceil}) — else this isn't testing the no-op path"
        );
        // No-op: the rendered dark interior matches the uncapped raw within rounding.
        assert!(
            (dark_interior - raw_luma).abs() < 0.02,
            "glass interior over a DARK backdrop was altered by the cap (no-op expected): \
             rendered {dark_interior} vs uncapped {raw_luma} — the frosted dark look changed"
        );
        // And the dark interior is meaningfully BELOW the bright (now-capped) one only
        // if both aren't pinned to the ceiling — here dark < ceil while bright == ceil,
        // proving the cap fires exactly where it should.
        assert!(
            dark_interior < ceil,
            "dark interior should sit below the ceiling (it's {dark_interior}) — \
             still clearly frosted/translucent, not clamped"
        );
    }

    // (The Round-5/6 warm-amber bottom-rim tests were retired with the
    // perimeter rim itself — IDENTITY-OBSIDIAN.md §2. The no-rim-on-surfaces
    // contract lives in `rim_renders_chromatic_pixels` above, which also keeps
    // the `draw_iridescent_rim` primitive honest for theming callers.)

    /// Round-7 visual-QA #3 — the CHROME tier (taskbar) must hold a CONSTANT material
    /// floor so it reads as a distinct frosted surface over a BRIGHT backdrop, not
    /// track the wallpaper (measured: bar 38,61,100 vs wp 38,64,107 — a 3–6 luma delta,
    /// invisible). We fill a flat BRIGHT field (an aurora-blob-bright patch), draw the
    /// chrome tier over it, and assert the chrome interior is DISTINCT — clearly DARKER
    /// than the bare bright field by a real margin, not tracking it within <10 luma.
    ///
    /// Round-9 #3 — the floor now comes from the chrome-only luma ceiling
    /// [`CHROME_INTERIOR_LUMA_CEIL`] (the fixed black scrim that had over-darkened chrome
    /// to panel's luma over EVERY backdrop is retired). FAIL-ability: raise
    /// `CHROME_INTERIOR_LUMA_CEIL` toward the bright field's luma and the chrome interior
    /// climbs to match it → the delta collapses under the threshold → this trips.
    #[test]
    fn chrome_holds_constant_floor_over_bright() {
        let luma601 = |p: u32| -> i64 {
            let r = ((p >> 16) & 0xFF) as i64;
            let g = ((p >> 8) & 0xFF) as i64;
            let b = (p & 0xFF) as i64;
            (77 * r + 150 * g + 29 * b) / 256
        };
        let (mut px, w, h) = fb(220, 160);
        // A BRIGHT field matching the worst case: the bright aurora center where the
        // legibility cap pulls chrome down and it vanished. ~L150+ so the cap fires.
        let bright = 0xFF_C8_D2_E6u32;
        {
            let mut c = unsafe { Canvas::new(px.as_mut_ptr() as *mut u8, w, h, 4) };
            c.fill_rect(0, 0, w, h, bright);
            draw_glass_surface(&mut c, 20, 20, 160, 120, 14, GLASS_CHROME_DARK);
        }
        let field = luma601(bright);
        let chrome = luma601(px[80 * w + 100]); // inside the chrome interior
                                                // The chrome must be a DISTINCT surface: clearly DARKER than the bright field
                                                // by a real margin (a dock-like floor), not matching it within the 3–6 luma the
                                                // shipped bug measured. 20 luma is a comfortable "reads as a separate surface"
                                                // bar (the bug delta was 3–6; the ceiling floor opens it wide).
        let delta = field - chrome;
        assert!(
            delta >= 20,
            "chrome tier did not hold a constant floor over a bright field: \
             field L{field} vs chrome L{chrome} (delta {delta}) — it tracks the \
             wallpaper (<20) and vanishes, the Round-7 #3 bug"
        );
    }

    /// Round-9 visual-QA #3 — the CHROME tier (§7 "the MOST see-through") must read as
    /// distinguishably more translucent / closer to the backdrop than the PANEL tier
    /// over the SAME backdrop (the defect measured taskbar chrome L77 ≈ CC panel L78 —
    /// the Round-7 fixed black scrim had over-darkened chrome to panel's luma). "More
    /// see-through" = the chrome interior deviates LESS from the backdrop than the panel
    /// interior. We render both tiers over a spread of backdrops (dark → bright) and
    /// assert: (a) chrome is strictly more see-through (smaller |interior − backdrop|)
    /// than panel everywhere, AND (b) over a BRIGHT backdrop chrome reads LIGHTER than
    /// panel (chrome floors above panel's 0.40 cap) — the most-see-through identity — yet
    /// still distinct from the field (the constant-floor win, pinned by the sibling test).
    ///
    /// FAIL-ability: lower `CHROME_INTERIOR_LUMA_CEIL` to `GLASS_INTERIOR_LUMA_CEIL` (or
    /// re-introduce a heavy chrome scrim) and chrome collapses to ≈ panel over bright →
    /// the "lighter than panel" assertion trips; over a mid backdrop the scrim pushes
    /// chrome past panel's deviation → the see-through assertion trips.
    #[test]
    fn chrome_is_more_see_through_than_panel() {
        let luma601 = |p: u32| -> i64 {
            let r = ((p >> 16) & 0xFF) as i64;
            let g = ((p >> 8) & 0xFF) as i64;
            let b = (p & 0xFF) as i64;
            (77 * r + 150 * g + 29 * b) / 256
        };
        // Dark → bright; the taskbar sits over the lower aurora (mid) and panels over
        // the brighter center, so both regimes must hold.
        let backdrops = [
            0xFF_10_14_22u32,
            0xFF_1E_2A_48,
            0xFF_2A_44_78,
            0xFF_50_64_98,
            0xFF_8A_A0_C8,
            0xFF_C8_D2_E6,
        ];
        let mut chrome_dev_sum = 0i64;
        let mut panel_dev_sum = 0i64;
        for &bk in backdrops.iter() {
            let (mut px, w, h) = fb(400, 200);
            {
                let mut c = unsafe { Canvas::new(px.as_mut_ptr() as *mut u8, w, h, 4) };
                c.fill_rect(0, 0, w, h, bk);
                draw_glass_surface(&mut c, 20, 40, 160, 120, 14, GLASS_CHROME_DARK);
                draw_glass_surface(&mut c, 220, 40, 160, 120, 14, ath_tokens::GLASS_PANEL_DARK);
            }
            let bkl = luma601(bk);
            let chrome = luma601(px[100 * w + 100]);
            let panel = luma601(px[100 * w + 300]);
            // OBSIDIAN (IDENTITY-OBSIDIAN.md §2): both tiers are near-black;
            // "most see-through" now means chrome BLEEDS more backdrop. Over a
            // bright backdrop chrome must therefore read LIGHTER than panel
            // (more of the bright field survives its lower alpha).
            if bkl >= 140 {
                assert!(
                    chrome > panel,
                    "chrome not lighter (more see-through) than panel over a bright bk \
                     L{bkl}: chrome L{chrome} vs panel L{panel} — chrome must bleed more \
                     backdrop (most-see-through tier)"
                );
            }
            // And BOTH interiors stay in the obsidian register over any backdrop:
            // near-black (≤ L60 on the 0-255 601 scale), never the milky gray.
            assert!(
                chrome <= 60 && panel <= 60,
                "obsidian interiors must stay near-black over bk L{bkl}: \
                 chrome L{chrome} panel L{panel}"
            );
            // Accumulate the bleed deltas for the aggregate ordering check.
            chrome_dev_sum += chrome - luma601(ath_tokens::GLASS_CHROME_DARK.tint);
            panel_dev_sum += panel - luma601(ath_tokens::GLASS_PANEL_DARK.tint);
        }
        // Aggregate: across the spread, chrome's interior must carry MORE backdrop
        // bleed (deviation above its own tint) than panel — the precise obsidian
        // meaning of "chrome is the most see-through tier". FAIL-ability: raise
        // chrome's alpha to panel's (or above) and the margin collapses.
        assert!(
            chrome_dev_sum > panel_dev_sum,
            "chrome is not the most see-through tier across the backdrop spread: \
             Σ chrome bleed {chrome_dev_sum} vs Σ panel bleed {panel_dev_sum}"
        );
    }

    /// Round-9 visual-QA SHIP-GATE a11y — white `text.primary` must clear WCAG AA 4.5:1
    /// over the glass.popover tier EVERYWHERE, including the worst bright/saturated
    /// backdrop (the regression measured 3.7–3.9:1 in the context menu's lower third).
    /// The mean-channel luma cap does NOT guarantee real WCAG contrast for saturated
    /// interior pixels (a magenta pixel at mean-channel 0.40 measures only 2.8:1), and
    /// the saturation lift makes it worse — so the WCAG pass `clamp_interior_wcag_region`
    /// is the unconditional guarantee. We render a popover spanning a BRIGHT→DARK aurora
    /// gradient AND over a spread of strongly-colored bright fields, then assert every
    /// interior pixel clears AA against `TEXT_PRIMARY_DARK`.
    ///
    /// FAIL-ability: delete the `clamp_interior_wcag_region` call in `draw_glass_surface`
    /// (or drop `TEXT_AA_TARGET` below 4.5) and the saturated bright-backdrop interiors
    /// fall back under 4.5:1 → this trips.
    #[test]
    fn popover_white_text_clears_aa_over_any_bright_backdrop() {
        // Measure the body-text INTERIOR only: skip the iridescent rim band (dist
        // 1..=GLASS_EDGE_BAND_PX) and the top highlight, which are additive light DRAWN
        // OVER the capped interior — body text sits inset past them, never on them.
        let worst_interior_cr =
            |px: &[u32], w: usize, x: usize, y: usize, sw: usize, sh: usize, rad: usize| -> f32 {
                let band = GLASS_EDGE_BAND_PX as i64;
                let mut worst = f32::MAX;
                for py in y..(y + sh) {
                    for pxn in x..(x + sw) {
                        if rounded_edge_dist(pxn, py, x, y, sw, sh, rad) <= band {
                            continue; // gutter, rim band, or top highlight — not text background
                        }
                        let p = px[py * w + pxn] | 0xFF00_0000;
                        let cr = ath_tokens::contrast_ratio(TEXT_PRIMARY_DARK, p);
                        if cr < worst {
                            worst = cr;
                        }
                    }
                }
                worst
            };

        // Case 1: a popover spanning a bright→dark aurora gradient (the context-menu
        // lower-third-over-bright-aurora condition, the named SHIP-GATE regression).
        let (mut px, w, h) = fb(360, 360);
        let (sx, sy, sw, sh, rad) = (30usize, 30usize, 300usize, 300usize, 14usize);
        {
            let mut c = unsafe { Canvas::new(px.as_mut_ptr() as *mut u8, w, h, 4) };
            c.fill_rect_gradient(0, 0, w, h, 0xFF_E8_E2_C8, 0xFF_12_18_2A);
            draw_glass_surface(&mut c, sx, sy, sw, sh, rad, ath_tokens::GLASS_POPOVER_DARK);
        }
        let cr1 = worst_interior_cr(&px, w, sx, sy, sw, sh, rad);
        assert!(
            cr1 >= 4.5,
            "popover white text.primary fails AA over a bright→dark aurora gradient: \
             worst interior CR {cr1:.2} < 4.5 (the context-menu lower-third regression)"
        );

        // Case 2: the adversarial saturated bright fields — the hues whose true relative
        // luminance most exceeds their mean-channel luma (where the old cap leaked).
        for &bk in &[
            0xFF_FF_30_F0u32, // magenta (the 2.8:1 worst case under the mean-channel cap)
            0xFF_30_F0_F0,    // cyan
            0xFF_F0_F0_30,    // yellow
            0xFF_F0_30_30,    // red
            0xFF_30_F0_30,    // green
            0xFF_FF_FF_FF,    // white
            0xFF_C0_E0_A0,    // aurora-bright green
        ] {
            let (mut bpx, bw, bh) = fb(160, 160);
            {
                let mut c = unsafe { Canvas::new(bpx.as_mut_ptr() as *mut u8, bw, bh, 4) };
                c.fill_rect(0, 0, bw, bh, bk);
                draw_glass_surface(&mut c, 16, 16, 128, 128, 12, ath_tokens::GLASS_POPOVER_DARK);
            }
            let cr = worst_interior_cr(&bpx, bw, 16, 16, 128, 128, 12);
            assert!(
                cr >= 4.5,
                "popover white text.primary fails AA over bright {bk:08X}: \
                 worst interior CR {cr:.2} < 4.5 — the mean-channel cap leaked a saturated pixel"
            );
        }
    }

    /// Round-9 visual-QA #2 — the warm-amber rim stop must render as VISIBLE warm/gold on
    /// the bottom edge of a surface rendered OVER THE REAL AURORA at a real on-screen
    /// position (the defect: 0 of 23,416 bottom-edge px classified warm across the
    /// shipped surfaces, even though the synthetic isolated-rect KAT passed). The gap was
    /// the warm boost over the real (often green/teal-bright) bottom-edge aurora backdrop
    /// producing a color that read green, not gold. We render a popover-shaped surface at
    /// the context-menu's screen position over the full aurora and count warm/gold
    /// bottom-rim pixels (R & G decisively above B).
    ///
    /// FAIL-ability: weaken `warm_amber_boost` (stop pinning red to full / stop cutting
    /// blue) and the warm count over the bright aurora bottom edge collapses → this trips.
    #[test]
    fn bottom_rim_warm_amber_visible_on_real_surface() {
        // A 1280×800 desktop with the full aurora; a popover at the context-menu position
        // (60,60) so its bottom edge sits over the bright upper-left aurora — the exact
        // real condition the shipped surfaces hit (not an isolated rect on a bare field).
        let (w, h) = (1280usize, 800usize);
        let (sx, sy, sw, sh, rad) = (60usize, 60usize, 360usize, 320usize, 12usize);
        let mut px = alloc::vec![0u32; w * h];
        {
            let mut c = unsafe { Canvas::new(px.as_mut_ptr() as *mut u8, w, h, 4) };
            render_aurora_dark(&mut c, 0, 0, w, h, 0);
            draw_glass_surface(&mut c, sx, sy, sw, sh, rad, ath_tokens::GLASS_POPOVER_DARK);
        }
        let band = GLASS_EDGE_BAND_PX as i64;
        let mut warm = 0u32;
        for x in sx + rad..sx + sw - rad {
            for d in 1..=band {
                let y = (sy + sh) as i64 - d;
                let p = px[y as usize * w + x];
                let r = ((p >> 16) & 0xFF) as i64;
                let g = ((p >> 8) & 0xFF) as i64;
                let b = (p & 0xFF) as i64;
                if r >= b + 24 && g >= b + 12 {
                    warm += 1;
                }
            }
        }
        // OBSIDIAN re-contract (IDENTITY-OBSIDIAN.md §2): the surface rim is
        // RETIRED — a real popover over the real aurora must carry essentially
        // ZERO warm rim pixels (the inverse of the frost-era pin this replaced).
        // FAIL-able: the rim call regressing back into draw_glass_surface trips it.
        assert!(
            warm < 20,
            "obsidian surfaces carry no warm rim — found {warm} warm bottom-edge px \
             (the retired iridescent rim regressed back into draw_glass_surface)"
        );
    }

    /// Round-7 visual-QA #5 — the panel/popover frost must carry the backdrop's CHROMA
    /// (a subtle aurora tint), not desaturate it to flat grey. We render a popover (the
    /// thickest frost, the worst desaturation) over a strongly COLORED backdrop and
    /// assert the composited interior retains real chroma (max-channel − min-channel
    /// spread) — AND that the saturation lift did NOT raise the mean luma over the §2.3
    /// ceiling (legibility preserved). FAIL-ability: drop `GAIN` to 0 in
    /// `saturate_interior_region` and the chroma spread collapses toward grey → trips.
    #[test]
    fn start_popover_frost_carries_chroma() {
        let chroma = |p: u32| -> i64 {
            let r = ((p >> 16) & 0xFF) as i64;
            let g = ((p >> 8) & 0xFF) as i64;
            let b = (p & 0xFF) as i64;
            r.max(g).max(b) - r.min(g).min(b)
        };
        let mean_luma = |p: u32| -> f32 {
            let r = ((p >> 16) & 0xFF) as f32;
            let g = ((p >> 8) & 0xFF) as f32;
            let b = (p & 0xFF) as f32;
            (0.2126 * r + 0.7152 * g + 0.0722 * b) / 255.0
        };
        let (mut px, w, h) = fb(220, 220);
        // A saturated aurora-teal backdrop (G dominant) — the kind the Start popover
        // sits over. The frost must let this color bleed through, not grey it out.
        let colored = 0xFF_1E_8C_A0u32;
        {
            let mut c = unsafe { Canvas::new(px.as_mut_ptr() as *mut u8, w, h, 4) };
            c.fill_rect(0, 0, w, h, colored);
            draw_glass_surface(&mut c, 20, 20, 180, 180, 16, ath_tokens::GLASS_POPOVER_DARK);
        }
        let interior = px[110 * w + 110];
        // OBSIDIAN: the near-opaque tier lets only a WHISPER of the backdrop
        // through — but that whisper must still be a HUE, not grey (the §2
        // "chroma bleed" clause). The saturated teal backdrop must leave a
        // measurable colored cast in the interior. FAIL-able both ways: a
        // fully-opaque tint (alpha 0xFF) zeroes the spread; a milky regression
        // pushes the interior luma out of the near-black register below.
        let c_interior = chroma(interior);
        assert!(
            c_interior >= 3,
            "obsidian interior lost the backdrop hue entirely (chroma \
             {c_interior}) — the whisper of aurora color must survive"
        );
        // And the interior stays in the obsidian near-black register.
        let l = mean_luma(interior);
        assert!(
            l <= 0.14,
            "obsidian popover interior must read near-black (mean luma {l:.3} > 0.14)"
        );
    }
}
