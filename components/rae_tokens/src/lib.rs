//! AthenaOS shared design tokens — *"Built for people who care about how things
//! feel."* (LEGACY_GAMING_CONCEPT.md).
//!
//! This crate is the single source of truth for the design language defined in
//! `docs/design/design-language.md`: spacing, corner radius, the color palettes,
//! the accent ramp + [`derive_accent`], elevation/shadow, the type ramp, the
//! motion system, and the glass material. Every value here is the verbatim
//! token from that document.
//!
//! Why it exists (Concept Vibe Mode): "one tap re-skins the whole desktop" is
//! only real if a single seed accent flows to every surface from one home.
//! Today ~30 files each redefine `const ACCENT: u32 = 0xFF_4E_9C_FF` plus a
//! private palette; this crate ends that duplication. Per ADR 0003 it is a
//! zero-dependency `#![no_std]` crate so the bare-metal kernel AND userspace
//! (raeui re-exports it as `raeui::tokens`) share ONE token home. It is NOT an
//! ABI surface — `rae_abi` stays frozen; visual tokens churn here freely.
//!
//! Pure logic, host-KAT'd (`cargo test -p rae_tokens`): the crate is `no_std`
//! in normal builds and toggles to `std` only under `cfg(test)` so the KAT
//! harness can run on the dev box without QEMU.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

// ── §2 Spacing & grid ──────────────────────────────────────────────────────
// Base unit = 4px. All spacing/padding/gaps are multiples. (px as u32.)

/// Flush (no gap).
pub const SPACE_0: u32 = 0;
/// Icon-to-label, tight inset.
pub const SPACE_1: u32 = 4;
/// Default control padding, intra-group gap.
pub const SPACE_2: u32 = 8;
/// Control vertical padding, list-row inset.
pub const SPACE_3: u32 = 12;
/// Panel padding, inter-group gap.
pub const SPACE_4: u32 = 16;
/// Section gap.
pub const SPACE_5: u32 = 24;
/// Window content margin.
pub const SPACE_6: u32 = 32;
/// Large couch-mode gap.
pub const SPACE_8: u32 = 48;

/// Minimum interactive element size in pointer/desktop mode (px).
pub const HIT_TARGET_POINTER: u32 = 32;
/// Minimum interactive element size in touch/couch/controller mode (px).
pub const HIT_TARGET_COUCH: u32 = 48;

// ── §3 Corner-radius scale ─────────────────────────────────────────────────

/// Buttons, chips, tray icons, menu rows.
pub const RADIUS_XS: u32 = 4;
/// Controls, search field, toasts.
pub const RADIUS_SM: u32 = 8;
/// Window corners, flyouts, cards.
pub const RADIUS_MD: u32 = 12;
/// Start menu, quick-settings panel, large cards.
pub const RADIUS_LG: u32 = 16;
/// OOBE / full-screen modal cards.
pub const RADIUS_XL: u32 = 24;

/// Pill radius for a control of the given height (`h/2`).
#[must_use]
pub const fn radius_pill(height: u32) -> u32 {
    height / 2
}

/// Concentric child radius: a child inside a padded parent uses
/// `parent_radius - parent_padding`, clamped to `>= RADIUS_XS` so nested glass
/// never shows mismatched corners (the macOS Liquid-Glass lesson, §3).
#[must_use]
pub const fn concentric(parent_radius: u32, parent_padding: u32) -> u32 {
    let inner = parent_radius.saturating_sub(parent_padding);
    if inner < RADIUS_XS {
        RADIUS_XS
    } else {
        inner
    }
}

// ── §4 Color system ────────────────────────────────────────────────────────
// All colors are ARGB 0xAARRGGBB (compositor-native).

/// A base color palette (§4.1 dark, §4.2 light). All fields are ARGB `u32`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Palette {
    /// Desktop void / deepest layer.
    pub bg_base: u32,
    /// Window client, panels.
    pub bg_raised: u32,
    /// Menus, flyouts (pre-glass solid fallback).
    pub bg_overlay: u32,
    /// Hovered rows, selected list items.
    pub bg_elevated: u32,
    /// Hairline dividers.
    pub stroke_subtle: u32,
    /// Glass top-edge highlight.
    pub stroke_strong: u32,
    /// Headings, active labels.
    pub text_primary: u32,
    /// Body, inactive labels.
    pub text_secondary: u32,
    /// Hints, disabled, timestamps.
    pub text_tertiary: u32,
    /// Close hover, destructive.
    pub state_danger: u32,
    /// Warnings.
    pub state_warn: u32,
    /// Success, link-up.
    pub state_ok: u32,
}

/// §4.1 Dark palette (default).
pub const DARK: Palette = Palette {
    bg_base: 0xFF_0A_0E_1A,
    bg_raised: 0xFF_12_16_24,
    bg_overlay: 0xFF_1A_1E_2E,
    bg_elevated: 0xFF_22_27_38,
    stroke_subtle: 0x33_FF_FF_FF,
    stroke_strong: 0x55_FF_FF_FF,
    text_primary: 0xFF_F0_F2_F8,
    text_secondary: 0xFF_AE_B4_C6,
    text_tertiary: 0xFF_6E_76_8C,
    state_danger: 0xFF_E5_4B_4B,
    state_warn: 0xFF_E8_B5_4B,
    // §4.1 prints the typo'd `0xFF_3FBF_7F`; the corrected value is `0xFF_3F_BF_7F`.
    state_ok: 0xFF_3F_BF_7F,
};

/// §4.2 Light palette. (`state_*` are shared with dark — §4.2 omits them, so
/// the dark state colors are reused; they already meet contrast on light bg.)
pub const LIGHT: Palette = Palette {
    bg_base: 0xFF_EC_EF_F5,
    bg_raised: 0xFF_F7_F9_FC,
    bg_overlay: 0xFF_FF_FF_FF,
    bg_elevated: 0xFF_E2_E7_F0,
    stroke_subtle: 0x1A_00_00_00,
    stroke_strong: 0x33_FF_FF_FF,
    text_primary: 0xFF_14_18_22,
    text_secondary: 0xFF_45_4C_5E,
    // Darkened on the same blue-grey hue ramp so hints/disabled/timestamps clear
    // WCAG 1.4.11 non-text/large contrast (3.0:1) on `light.bg_base`: the old
    // `0xFF8A90A0` measured 2.73:1 (a real defect the audit caught); this lands
    // ~4.0:1 with margin while staying visually a muted tertiary grey.
    text_tertiary: 0xFF_6E_74_86,
    state_danger: 0xFF_E5_4B_4B,
    state_warn: 0xFF_E8_B5_4B,
    state_ok: 0xFF_3F_BF_7F,
};

/// The default seed accent — "RaeBlue" (`ThemeAbi.accent_argb` default, §4.3).
pub const RAEBLUE: u32 = 0xFF_4E_9C_FF;

/// §4 color · `scrim.capture` — the screenshot/region-capture overlay backdrop
/// (`docs/design/screenshot-capture.md` §3). ~60% near-black dim, tinted toward
/// `bg.base` rather than pure black for material consistency, applied over the
/// whole screen during region selection; the *selected* region is punched back
/// to full brightness so the user previews the exact capture. The one new token
/// the screenshot spec introduces. Palette-neutral (used over both dark/light).
pub const SCRIM_CAPTURE: u32 = 0x99_06_08_10;

/// Light-mode focus-ring accent. RaeBlue itself (`0xFF4E9CFF`) is only 2.40:1 on
/// `LIGHT.bg_base` — below WCAG 1.4.11's 3.0:1 floor for focus indicators — so on
/// light surfaces the ring uses the deeper pressed-accent blue (`derive_accent`'s
/// `active` darken of RaeBlue, `0xFF3A80DB`), which clears 3.0:1 with margin while
/// still reading as an accent blue. The DARK ring stays RaeBlue (it already passes
/// and carries the accent/mica cohesion).
pub const LIGHT_FOCUS_RING: u32 = 0xFF_3A_80_DB;

/// §4.3 Accent ramp: six tokens derived deterministically from one seed so a
/// re-skin is a single value change (the Vibe-Mode cohesion engine).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AccentRamp {
    /// The seed itself.
    pub base: u32,
    /// Lighten seed toward white (hover state).
    pub hover: u32,
    /// Darken seed (active/pressed state).
    pub active: u32,
    /// Seed at ~24% alpha over bg (subtle fills, selection wash).
    pub subtle: u32,
    /// Seed if it meets WCAG 4.5:1 on `bg.base`, else `text.primary`.
    pub text: u32,
    /// Seed at ~40% alpha — focus-ring / glow shadow color.
    pub glow: u32,
}

// ── ARGB channel helpers ───────────────────────────────────────────────────

#[inline]
const fn argb(a: u32, r: u32, g: u32, b: u32) -> u32 {
    (a << 24) | (r << 16) | (g << 8) | b
}
#[inline]
const fn chan_a(c: u32) -> u32 {
    (c >> 24) & 0xFF
}
#[inline]
const fn chan_r(c: u32) -> u32 {
    (c >> 16) & 0xFF
}
#[inline]
const fn chan_g(c: u32) -> u32 {
    (c >> 8) & 0xFF
}
#[inline]
const fn chan_b(c: u32) -> u32 {
    c & 0xFF
}

/// Per-channel lighten toward white: `c + (255 - c) * num / den`, alpha kept.
#[inline]
fn lighten(color: u32, num: u32, den: u32) -> u32 {
    let l = |c: u32| -> u32 { c + (255 - c) * num / den };
    argb(
        chan_a(color),
        l(chan_r(color)),
        l(chan_g(color)),
        l(chan_b(color)),
    )
}

/// Per-channel "active/pressed" darken, alpha kept. A pure per-channel multiply
/// cannot reproduce the §4.3 table's `accent.active` (0xFF3A80DB) from RaeBlue —
/// the documented value reduces the blue channel more than a hue-preserving
/// scale would (it is a value-darken + slight saturation lift from the design
/// tool). The closest clean integer fit that lands every channel within ±2 of
/// the table across the accent range is the affine `round(c * 0.897) - 12`
/// (saturating at 0): a ~10% multiplicative darken plus a small fixed
/// subtractive bias that deepens the brighter channels. See the report note.
#[inline]
fn darken_active(color: u32) -> u32 {
    let d = |c: u32| -> u32 {
        let scaled = (c * 897 + 500) / 1000;
        scaled.saturating_sub(12)
    };
    argb(
        chan_a(color),
        d(chan_r(color)),
        d(chan_g(color)),
        d(chan_b(color)),
    )
}

/// Replace the alpha channel, keeping RGB.
#[inline]
const fn with_alpha(color: u32, alpha: u32) -> u32 {
    (color & 0x00_FF_FF_FF) | ((alpha & 0xFF) << 24)
}

/// Derive the full six-token accent ramp from a single seed over a palette
/// (§4.3). Deterministic — the same seed always yields the same ramp, which is
/// what makes Vibe Mode's one-tap re-skin coherent across every surface.
///
/// Rules: `base` = seed; `hover` = lighten ~18% toward white; `active` =
/// darken ~14%; `subtle` = seed @ alpha `0x3D` (~24%); `text` = seed if it
/// meets WCAG 4.5:1 on `palette.bg_base` else `palette.text_primary`; `glow` =
/// seed @ alpha `0x66` (~40%).
#[must_use]
pub fn derive_accent(seed: u32, palette: &Palette) -> AccentRamp {
    let text = if contrast_ratio(seed, palette.bg_base) >= 4.5 {
        seed
    } else {
        palette.text_primary
    };
    AccentRamp {
        base: seed,
        // 18/100: per-channel toward-white lands RaeBlue exactly on the §4.3
        // table's 0xFF6EAEFF (the prose "~12%" undershoots; the table wins).
        hover: lighten(seed, 18, 100),
        // ~14% value-darken; matches the §4.3 table's 0xFF3A80DB within ±2.
        active: darken_active(seed),
        subtle: with_alpha(seed, 0x3D),
        text,
        glow: with_alpha(seed, 0x66),
    }
}

// ── §4.4 File-type semantics (fixed — NOT accent-derived) ───────────────────
// A small fixed palette so a file *type* reads consistently across every Vibe
// preset (a directory must look like a directory in any theme). Reusable by any
// app that shows a file chip; lives here alongside the palettes per
// `design-language.md` §4.4. Two exceptions track the accent on purpose
// (`dir`/`code` read as "primary") — they are NOT consts because they depend on
// the live seed, so they are resolved through [`ftype_dir`] / [`ftype_code`].

/// `ftype.exec` — executables (= `state.ok` green). §4.4.
pub const FTYPE_EXEC: u32 = 0xFF_3F_BF_7F;
/// `ftype.media` — image / video / audio (collapsed from the old 4-hue rainbow,
/// premium restraint). §4.4.
pub const FTYPE_MEDIA: u32 = 0xFF_C0_7C_FF;
/// `ftype.doc` — documents / pdf. §4.4.
pub const FTYPE_DOC: u32 = 0xFF_F0_C8_5C;
/// `ftype.archive` — archives. §4.4.
pub const FTYPE_ARCHIVE: u32 = 0xFF_F0_A0_3C;

/// `ftype.dir` — directories (the one type that tracks accent). §4.4: directories
/// read as "primary", so this returns `derive_accent(seed, palette).base`.
#[must_use]
pub fn ftype_dir(seed: u32, palette: &Palette) -> u32 {
    derive_accent(seed, palette).base
}

/// `ftype.code` — source code (also tracks accent, like `dir`). §4.4.
#[must_use]
pub fn ftype_code(seed: u32, palette: &Palette) -> u32 {
    derive_accent(seed, palette).base
}

/// `ftype.neutral` — plain / unknown / device / socket / pipe (= `text.secondary`).
/// §4.4. Palette-dependent so it flips with dark/light.
#[must_use]
pub const fn ftype_neutral(palette: &Palette) -> u32 {
    palette.text_secondary
}

// ── WCAG relative luminance + contrast (§4.2 / §8 accessibility) ───────────

/// sRGB channel (0..=255) → linear light (0.0..=1.0), WCAG sRGB transfer
/// function: `c/12.92` below the 0.03928 threshold, else `((c+0.055)/1.055)`
/// raised to 2.4. The exponent is evaluated with a `no_std`-safe power
/// approximation (no `libm`); accuracy is well inside the margin needed to
/// gate the AA contrast thresholds (7:1 / 4.5:1 / 3:1).
fn srgb_to_linear(c8: u32) -> f32 {
    let c = (c8 & 0xFF) as f32 / 255.0;
    if c <= 0.03928 {
        c / 12.92
    } else {
        let base = (c + 0.055) / 1.055;
        powf_approx(base, 2.4)
    }
}

/// `x^y` for `x` in (0, 1], `y > 0`, without `libm`. Uses `y^x = exp2(x*log2(y))`
/// with polynomial `log2`/`exp2` over the reduced range. Monotonic and accurate
/// to a few 1e-3 on [0,1] — far inside the contrast-gate margins.
fn powf_approx(x: f32, y: f32) -> f32 {
    if x <= 0.0 {
        return 0.0;
    }
    exp2_approx(y * log2_approx(x))
}

/// log2(x) for x > 0 via mantissa/exponent decomposition + a 3rd-order
/// polynomial on the mantissa in [1, 2).
fn log2_approx(x: f32) -> f32 {
    // Decompose x = m * 2^e with m in [1, 2).
    let bits = x.to_bits();
    let exp = ((bits >> 23) & 0xFF) as i32 - 127;
    let mantissa_bits = (bits & 0x007F_FFFF) | 0x3F80_0000; // force exponent 0 -> [1,2)
    let m = f32::from_bits(mantissa_bits);
    // Minimax-ish cubic for log2(m), m in [1,2): exact at m=1 (0) and m=2 (1).
    let t = m - 1.0;
    let poly = t * (1.441_793_8 + t * (-0.708_144_4 + t * 0.266_350_6));
    poly + exp as f32
}

/// 2^x for any real x via integer split: `2^x = 2^i * 2^f`, f in [0,1) by a
/// cubic; the integer part is an exact exponent shift.
fn exp2_approx(x: f32) -> f32 {
    let i = floor_f32(x);
    let f = x - i;
    // Cubic for 2^f on [0,1): exact at f=0 (1) and f=1 (2).
    let frac = 1.0 + f * (0.656_366_3 + f * (0.227_411_3 + f * 0.116_222_4));
    // Scale by 2^i via the float exponent field.
    let ii = i as i32;
    let scale = if ii >= 0 {
        f32::from_bits(((127 + ii) as u32) << 23)
    } else {
        1.0 / f32::from_bits(((127 - ii) as u32) << 23)
    };
    frac * scale
}

#[inline]
fn floor_f32(x: f32) -> f32 {
    let i = x as i32;
    let f = i as f32;
    if f > x {
        f - 1.0
    } else {
        f
    }
}

/// Relative luminance of an ARGB color (alpha ignored — opaque surface).
fn relative_luminance(color: u32) -> f32 {
    let r = srgb_to_linear(chan_r(color));
    let g = srgb_to_linear(chan_g(color));
    let b = srgb_to_linear(chan_b(color));
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

/// WCAG contrast ratio between two opaque colors: `(L_light + 0.05) /
/// (L_dark + 0.05)`, always `>= 1.0`. Use to gate the §4.2 / §8 AA targets.
#[must_use]
pub fn contrast_ratio(fg_argb: u32, bg_argb: u32) -> f32 {
    let l1 = relative_luminance(fg_argb);
    let l2 = relative_luminance(bg_argb);
    let (hi, lo) = if l1 >= l2 { (l1, l2) } else { (l2, l1) };
    (hi + 0.05) / (lo + 0.05)
}

// ── §8 Accessibility: WCAG contrast AUDIT over the shipped palette ──────────
//
// *"Built for people who care about how things feel."* — accessibility is a
// SHIP GATE (Phase 19 / PARITY_MATRIX §J): a UI that rivals macOS and Windows
// has to be legible for low-vision users. Neither Windows nor macOS ships a
// machine-checked WCAG audit of its OWN default palette — AthenaOS does, here,
// as a FAIL-able host KAT + boot smoketest. The contrast bar is WCAG 2.1 AA:
//   • 4.5:1 for normal body text (the [`AA_BODY`] threshold)
//   • 3.0:1 for large/UI text and non-text affordances incl. focus indicators
//     (the [`AA_LARGE`] threshold; WCAG 1.4.11 non-text contrast)
// Any shipped token pair below its bar is a real contrast defect, reported by
// name + measured ratio so it is actionable, not just red.

/// WCAG 2.1 AA contrast threshold for normal body text (1.4.3).
pub const AA_BODY: f32 = 4.5;
/// WCAG 2.1 AA contrast threshold for large text, UI affordances and non-text
/// elements incl. focus indicators (1.4.11 / large-text 1.4.3).
pub const AA_LARGE: f32 = 3.0;
/// WCAG 2.1 AAA contrast threshold (1.4.6) — held against the high-contrast
/// mode's own pairs. The HC palette is the a11y mode for low-vision users, so it
/// must be *exemplary*, not merely AA: pure-black/white clears this by ~3x.
pub const AAA: f32 = 7.0;

/// Flatten a (possibly translucent) ARGB color over an opaque ARGB backdrop —
/// straight alpha compositing per channel: `out = src*a + dst*(1-a)`. Used so
/// the audit measures the *composited* glass/tint surface a user actually sees,
/// not the raw token's pre-blend RGB (the macOS-glass legibility lesson: a tint
/// at 62% alpha is a different color once it lands on the desktop void).
#[must_use]
pub fn flatten_over(src_argb: u32, backdrop_argb: u32) -> u32 {
    let a = chan_a(src_argb);
    let blend = |s: u32, d: u32| -> u32 { (s * a + d * (255 - a) + 127) / 255 };
    argb(
        0xFF,
        blend(chan_r(src_argb), chan_r(backdrop_argb)),
        blend(chan_g(src_argb), chan_g(backdrop_argb)),
        blend(chan_b(src_argb), chan_b(backdrop_argb)),
    )
}

/// What legibility class a token pair belongs to, picking its WCAG threshold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContrastKind {
    /// Normal body text — must clear [`AA_BODY`] (4.5:1).
    BodyText,
    /// Large/UI/label text or a non-text affordance (incl. focus ring) — must
    /// clear [`AA_LARGE`] (3.0:1).
    LargeOrUi,
    /// A high-contrast-mode pair — held to WCAG AAA ([`AAA`], 7.0:1). The HC
    /// palette exists FOR low-vision users, so the audit holds it to the AAA bar
    /// (the task acceptance: "high-contrast pairs >= 7.0:1"), not merely AA.
    HighContrastAaa,
}

impl ContrastKind {
    /// The WCAG threshold this kind must meet.
    #[must_use]
    pub const fn threshold(self) -> f32 {
        match self {
            ContrastKind::BodyText => AA_BODY,
            ContrastKind::LargeOrUi => AA_LARGE,
            ContrastKind::HighContrastAaa => AAA,
        }
    }
}

/// One audited foreground/background token pair: its human name, the two
/// already-composited opaque colors, and the legibility class that sets its bar.
#[derive(Clone, Copy, Debug)]
pub struct ContrastPair {
    /// Stable name (`"fg/bg"` form) printed in the audit + failure message.
    pub name: &'static str,
    /// Foreground color (opaque; flatten translucent tokens before constructing).
    pub fg: u32,
    /// Background color (opaque; flatten translucent surfaces first).
    pub bg: u32,
    /// Which WCAG threshold this pair must clear.
    pub kind: ContrastKind,
}

impl ContrastPair {
    /// Measured WCAG contrast ratio for this pair.
    #[must_use]
    pub fn ratio(&self) -> f32 {
        contrast_ratio(self.fg, self.bg)
    }
    /// Does this pair clear its WCAG threshold?
    #[must_use]
    pub fn passes(&self) -> bool {
        self.ratio() >= self.kind.threshold()
    }
}

/// The result of [`audit_contrast`]: how many pairs were checked, how many
/// failed, and the single worst (lowest-ratio relative to its own bar) pair.
#[derive(Clone, Copy, Debug)]
pub struct ContrastReport {
    /// Total token pairs audited.
    pub pairs: usize,
    /// How many cleared their WCAG threshold.
    pub passed: usize,
    /// How many fell below their threshold (a real defect each).
    pub failed: usize,
    /// Name of the worst-margin pair (smallest `ratio - threshold`).
    pub worst_name: &'static str,
    /// Measured ratio of the worst-margin pair.
    pub worst_ratio: f32,
    /// Threshold the worst pair was held to.
    pub worst_threshold: f32,
}

impl ContrastReport {
    /// True iff every audited pair cleared its WCAG bar.
    #[must_use]
    pub const fn all_pass(self) -> bool {
        self.failed == 0
    }
}

/// The REAL shipped, legibility-bearing token pairs the OS actually paints.
/// Translucent surfaces (glass, accent washes) are flattened over their backdrop
/// first so the audit measures the composited pixel a user sees. This is the
/// authoritative list `audit_contrast` walks; extend it as new surfaces ship.
#[must_use]
pub fn shipped_contrast_pairs() -> [ContrastPair; 24] {
    // Composited glass surfaces (tint @ alpha over the base void).
    let glass_dark = flatten_over(GLASS_TINT_DARK, DARK.bg_base);
    let glass_light = flatten_over(GLASS_TINT_LIGHT, LIGHT.bg_base);
    // IDENTITY.md §8.7: the REAL backdrop is now the aurora mesh, not the flat
    // `bg_base` void. The audit must flatten the three dark tiers over the aurora
    // base and confirm `text.primary` still clears AA — the brighter/thinner glass
    // is the legibility risk, so this is where a too-translucent tier would fail.
    let chrome_over_aurora = flatten_over(GLASS_CHROME_DARK.tint, WALLPAPER_AURORA_BASE_DARK);
    let panel_over_aurora = flatten_over(GLASS_PANEL_DARK.tint, WALLPAPER_AURORA_BASE_DARK);
    let popover_over_aurora = flatten_over(GLASS_POPOVER_DARK.tint, WALLPAPER_AURORA_BASE_DARK);
    // SHIP-GATE a11y (raeen-accessibility WCAG audit): the aurora BASE above is the
    // DARK valley between blobs — the easy case. The legibility risk is the bright
    // aurora PEAK: where the additive blobs pile up the backdrop reaches ~0.41+ mean
    // luma, and the frost-lifted glass interior would wash white text below 4.5:1.
    // Model the peak as the base with the blue + teal blobs added (saturating) — the
    // brightest realistic overlap — and audit text.primary over the COMPOSITED
    // panel/popover interior there (via `glass_tier_interior`, which now applies the
    // §2.3/§9 luma cap). These pairs FAIL without the cap and PASS with it, so this
    // whole bright-region failure class is locked behind the ship-gate KAT forever.
    let aurora_peak = {
        let add = |a: u32, b: u32| -> u32 {
            argb(
                0xFF,
                (chan_r(a) + chan_r(b)).min(0xFF),
                (chan_g(a) + chan_g(b)).min(0xFF),
                (chan_b(a) + chan_b(b)).min(0xFF),
            )
        };
        add(
            add(WALLPAPER_AURORA_BASE_DARK, AURORA_BLOB_BLUE),
            AURORA_BLOB_TEAL,
        )
    };
    // Single-blob peak (just the blue blob) — the common "panel parked over one
    // aurora blob" case the audit flagged at 3.87:1 / 3.36:1 before the cap.
    let aurora_blob = AURORA_BLOB_BLUE;
    let panel_over_peak = glass_tier_interior(GLASS_PANEL_DARK, aurora_peak);
    let popover_over_peak = glass_tier_interior(GLASS_POPOVER_DARK, aurora_peak);
    let panel_over_blob = glass_tier_interior(GLASS_PANEL_DARK, aurora_blob);
    let popover_over_blob = glass_tier_interior(GLASS_POPOVER_DARK, aurora_blob);
    // Default accent focus ring vs the two backdrops (opaque seed; the ring is
    // drawn as a solid stroke even though the GLOW shadow uses alpha).
    let accent = RAEBLUE;
    // High-contrast palette (PARITY_MATRIX §J target; mirrors raeui's HC values).
    let hc_bg = HIGH_CONTRAST.bg_base;
    let hc_text = HIGH_CONTRAST.text_primary;
    let hc_text_tertiary = HIGH_CONTRAST.text_tertiary;
    let hc_focus = HIGH_CONTRAST_FOCUS_RING;

    [
        // ── Body text: 4.5:1 ──
        ContrastPair {
            name: "dark.text_primary/bg_base",
            fg: DARK.text_primary,
            bg: DARK.bg_base,
            kind: ContrastKind::BodyText,
        },
        ContrastPair {
            name: "dark.text_secondary/bg_base",
            fg: DARK.text_secondary,
            bg: DARK.bg_base,
            kind: ContrastKind::BodyText,
        },
        ContrastPair {
            name: "dark.text_primary/bg_raised",
            fg: DARK.text_primary,
            bg: DARK.bg_raised,
            kind: ContrastKind::BodyText,
        },
        ContrastPair {
            name: "dark.text_primary/glass",
            fg: DARK.text_primary,
            bg: glass_dark,
            kind: ContrastKind::BodyText,
        },
        ContrastPair {
            name: "dark.text_secondary/glass",
            fg: DARK.text_secondary,
            bg: glass_dark,
            kind: ContrastKind::BodyText,
        },
        ContrastPair {
            name: "light.text_primary/bg_base",
            fg: LIGHT.text_primary,
            bg: LIGHT.bg_base,
            kind: ContrastKind::BodyText,
        },
        ContrastPair {
            name: "light.text_secondary/bg_base",
            fg: LIGHT.text_secondary,
            bg: LIGHT.bg_base,
            kind: ContrastKind::BodyText,
        },
        ContrastPair {
            name: "light.text_primary/glass",
            fg: LIGHT.text_primary,
            bg: glass_light,
            kind: ContrastKind::BodyText,
        },
        // ── Glass tiers over the AURORA backdrop (IDENTITY.md §8.7) — the real
        //    composited surface now that the wallpaper is the aurora mesh, not the
        //    flat void. text.primary must clear AA over each dark tier. ──
        ContrastPair {
            name: "dark.text_primary/glass.chrome@aurora",
            fg: DARK.text_primary,
            bg: chrome_over_aurora,
            kind: ContrastKind::BodyText,
        },
        ContrastPair {
            name: "dark.text_primary/glass.panel@aurora",
            fg: DARK.text_primary,
            bg: panel_over_aurora,
            kind: ContrastKind::BodyText,
        },
        ContrastPair {
            name: "dark.text_primary/glass.popover@aurora",
            fg: DARK.text_primary,
            bg: popover_over_aurora,
            kind: ContrastKind::BodyText,
        },
        // ── Glass over the bright aurora PEAK/blob (SHIP-GATE a11y) — the §2.3/§9
        //    legibility luma cap (applied inside `glass_tier_interior`) must hold
        //    text.primary at AA over the brightest backdrop, not just the dark
        //    valley. WITHOUT the cap these measure 2.3–3.9:1 (the audited defect);
        //    WITH it they clear 4.5:1. This pins the bright-region class to the KAT. ──
        ContrastPair {
            name: "dark.text_primary/glass.panel@aurora_blob",
            fg: DARK.text_primary,
            bg: panel_over_blob,
            kind: ContrastKind::BodyText,
        },
        ContrastPair {
            name: "dark.text_primary/glass.popover@aurora_blob",
            fg: DARK.text_primary,
            bg: popover_over_blob,
            kind: ContrastKind::BodyText,
        },
        ContrastPair {
            name: "dark.text_primary/glass.panel@aurora_peak",
            fg: DARK.text_primary,
            bg: panel_over_peak,
            kind: ContrastKind::BodyText,
        },
        ContrastPair {
            name: "dark.text_primary/glass.popover@aurora_peak",
            fg: DARK.text_primary,
            bg: popover_over_peak,
            kind: ContrastKind::BodyText,
        },
        // ── Tertiary text is hints/disabled/timestamps → UI/large bar 3.0:1 ──
        ContrastPair {
            name: "dark.text_tertiary/bg_base",
            fg: DARK.text_tertiary,
            bg: DARK.bg_base,
            kind: ContrastKind::LargeOrUi,
        },
        ContrastPair {
            name: "light.text_tertiary/bg_base",
            fg: LIGHT.text_tertiary,
            bg: LIGHT.bg_base,
            kind: ContrastKind::LargeOrUi,
        },
        // ── State colors used as text/icons → body bar on their surfaces ──
        ContrastPair {
            name: "dark.state_danger/bg_base",
            fg: DARK.state_danger,
            bg: DARK.bg_base,
            kind: ContrastKind::LargeOrUi,
        },
        ContrastPair {
            name: "dark.state_ok/bg_base",
            fg: DARK.state_ok,
            bg: DARK.bg_base,
            kind: ContrastKind::LargeOrUi,
        },
        // ── Focus indicators (non-text contrast 1.4.11) → 3.0:1 ──
        ContrastPair {
            name: "accent_focus_ring/dark.bg_base",
            fg: accent,
            bg: DARK.bg_base,
            kind: ContrastKind::LargeOrUi,
        },
        ContrastPair {
            name: "accent_focus_ring/light.bg_base",
            fg: LIGHT_FOCUS_RING,
            bg: LIGHT.bg_base,
            kind: ContrastKind::LargeOrUi,
        },
        // ── High-contrast palette (the a11y mode itself must be exemplary:
        //    held to WCAG AAA 7:1, not merely AA) ──
        ContrastPair {
            name: "hc.text_primary/bg_base",
            fg: hc_text,
            bg: hc_bg,
            kind: ContrastKind::HighContrastAaa,
        },
        ContrastPair {
            name: "hc.text_tertiary/bg_base",
            fg: hc_text_tertiary,
            bg: hc_bg,
            kind: ContrastKind::HighContrastAaa,
        },
        ContrastPair {
            name: "hc.focus_ring/bg_base",
            fg: hc_focus,
            bg: hc_bg,
            kind: ContrastKind::HighContrastAaa,
        },
    ]
}

/// Audit the shipped legibility-bearing token pairs against their WCAG AA
/// thresholds. Pure logic over the static palette — runs as a host KAT AND a
/// boot smoketest. Returns a [`ContrastReport`] naming the worst-margin pair so
/// a failure is actionable. A pair is "worst" by smallest `ratio - threshold`
/// (a 4.6:1 body pair is closer to failing than a 4.0:1 UI pair).
#[must_use]
pub fn audit_contrast() -> ContrastReport {
    let pairs = shipped_contrast_pairs();
    let mut passed = 0usize;
    let mut failed = 0usize;
    let mut worst_name = pairs[0].name;
    let mut worst_ratio = pairs[0].ratio();
    let mut worst_threshold = pairs[0].kind.threshold();
    let mut worst_margin = worst_ratio - worst_threshold;
    for p in &pairs {
        let r = p.ratio();
        let margin = r - p.kind.threshold();
        if p.passes() {
            passed += 1;
        } else {
            failed += 1;
        }
        if margin < worst_margin {
            worst_margin = margin;
            worst_name = p.name;
            worst_ratio = r;
            worst_threshold = p.kind.threshold();
        }
    }
    ContrastReport {
        pairs: pairs.len(),
        passed,
        failed,
        worst_name,
        worst_ratio,
        worst_threshold,
    }
}

// ── High-contrast palette (PARITY_MATRIX §J: "palette swap via tokens") ──────
//
// Mirrors raeui's `HighContrastPalette` (background black / foreground white /
// cyan focus ring) but promoted into `rae_tokens` so the whole stack reads ONE
// HC home (foundation §4 / plan §4: "promote it to a rae_tokens variant so the
// swap is one token lookup"). Pure-black/white maximizes legibility — the HC
// pairs clear WCAG AAA (7:1) by a wide margin, which the audit asserts.

/// High-contrast `Palette` variant (black ground, white text, vivid states).
/// State colors are pure-saturated so they stay distinguishable at HC.
pub const HIGH_CONTRAST: Palette = Palette {
    bg_base: 0xFF_00_00_00,
    bg_raised: 0xFF_00_00_00,
    bg_overlay: 0xFF_00_00_00,
    bg_elevated: 0xFF_1A_1A_1A,
    stroke_subtle: 0xFF_FF_FF_FF,
    stroke_strong: 0xFF_FF_FF_FF,
    text_primary: 0xFF_FF_FF_FF,
    text_secondary: 0xFF_FF_FF_FF,
    text_tertiary: 0xFF_E0_E0_E0,
    state_danger: 0xFF_FF_00_00,
    state_warn: 0xFF_FF_FF_00,
    state_ok: 0xFF_00_FF_00,
};

/// High-contrast focus ring — cyan (mirrors raeui `HighContrastPalette.focus_ring`).
pub const HIGH_CONTRAST_FOCUS_RING: u32 = 0xFF_00_FF_FF;

// ── Live high-contrast mode flag (the forced-colors swap, plan §4 / audit P0 #3)
//
// The PARITY_MATRIX §J "no a11y palette swap" gap: the HC palette above is
// defined but nothing makes the running UI render in it. This is the single
// global flag every surface consults via [`active_palette`] so flipping it
// repaints the chrome in HC with ONE token lookup — Windows High Contrast /
// macOS Increase Contrast parity. `no_std`-safe (`AtomicBool`); the kernel
// a11y on-switch (`crate::a11y::set_high_contrast`) drives it.

/// Whether forced high-contrast mode is active. Default off (normal palette).
static HIGH_CONTRAST_ON: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// Enable/disable the live high-contrast mode. The a11y on-switch (hotkey or
/// Control Center) calls this; every surface that reads [`active_palette`] then
/// repaints in [`HIGH_CONTRAST`] on its next frame.
pub fn set_high_contrast(on: bool) {
    HIGH_CONTRAST_ON.store(on, core::sync::atomic::Ordering::Release);
}

/// Whether high-contrast mode is currently active.
#[must_use]
pub fn high_contrast() -> bool {
    HIGH_CONTRAST_ON.load(core::sync::atomic::Ordering::Acquire)
}

/// The palette every surface should paint with RIGHT NOW: [`HIGH_CONTRAST`] when
/// forced-colors mode is on, else the default [`DARK`] palette. This is the one
/// lookup that makes the HC swap live — a surface reads `active_palette()`
/// instead of a fixed `&DARK` and is HC-aware for free. (Light/Vibe palette
/// selection is a separate, additive future branch on the same accessor.)
#[must_use]
pub fn active_palette() -> &'static Palette {
    if high_contrast() {
        &HIGH_CONTRAST
    } else {
        &DARK
    }
}

/// The focus-ring color for the active palette: cyan under high contrast (the
/// `HighContrastPalette` ring), else the caller's normal accent ring. Surfaces
/// that draw a focus ring pass their normal-mode ring and get the HC override
/// automatically when forced-colors is on.
#[must_use]
pub fn active_focus_ring(normal_ring: u32) -> u32 {
    if high_contrast() {
        HIGH_CONTRAST_FOCUS_RING
    } else {
        normal_ring
    }
}

// ── §5.3 Elevation / shadow ladder ─────────────────────────────────────────

/// A drop-shadow recipe → `compositor::SurfaceEffect::DropShadow`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Elevation {
    /// Vertical shadow offset (px; positive = downward).
    pub offset_y: i32,
    /// Shadow blur radius (px).
    pub radius: u32,
    /// Shadow color (ARGB; alpha = strength). `0` = no shadow.
    pub color: u32,
}

/// Flush to desktop (no shadow).
pub const ELEV_0: Elevation = Elevation {
    offset_y: 0,
    radius: 0,
    color: 0,
};
/// Taskbar, resting cards.
pub const ELEV_1: Elevation = Elevation {
    offset_y: 1,
    radius: 6,
    color: 0x30_00_00_00,
};
/// Flyouts, toasts, menus.
pub const ELEV_2: Elevation = Elevation {
    offset_y: 3,
    radius: 14,
    color: 0x40_00_00_00,
};
/// Start menu, quick-settings, modals.
pub const ELEV_3: Elevation = Elevation {
    offset_y: 8,
    radius: 28,
    color: 0x55_00_00_00,
};
/// Dragged window, OOBE card.
pub const ELEV_4: Elevation = Elevation {
    offset_y: 12,
    radius: 40,
    color: 0x66_00_00_00,
};
/// Floating top-level window (Settings, app windows): the macOS-26/Win11 window
/// drop shadow — a long, soft, low-opacity cast that lifts the whole frame off the
/// wallpaper. §5.3. (offset 24 / blur 48 / 30% black.)
pub const ELEV_5: Elevation = Elevation {
    offset_y: 24,
    radius: 48,
    color: 0x4D_00_00_00,
};

/// Focus glow (`elev.focus`): an additive accent-tinted glow, not displacement.
/// Pass `AccentRamp.glow` as the color.
#[must_use]
pub const fn elev_focus(accent_glow: u32) -> Elevation {
    Elevation {
        offset_y: 0,
        radius: 10,
        color: accent_glow,
    }
}

/// Scale a shadow color's alpha by `num/den`, keeping RGB. Light mode reads
/// shadows heavier, so §5.3 multiplies shadow alpha by ~0.6 there:
/// `scale_shadow_alpha(c, 6, 10)`.
#[must_use]
pub const fn scale_shadow_alpha(color: u32, factor_num: u32, factor_den: u32) -> u32 {
    let a = chan_a(color) * factor_num / factor_den;
    with_alpha(color, a)
}

// ── §6 Typography ──────────────────────────────────────────────────────────

/// A type-ramp entry (px / weight / line-height, all px or unitless weight).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TypeStyle {
    /// Font size in px.
    pub px: u32,
    /// Font weight (400 / 500 / 600 shipped).
    pub weight: u32,
    /// Line height in px.
    pub line_height: u32,
}

/// OOBE, lock-screen clock.
pub const TYPE_DISPLAY: TypeStyle = TypeStyle {
    px: 32,
    weight: 600,
    line_height: 40,
};
/// Window/section titles, Start header.
pub const TYPE_TITLE: TypeStyle = TypeStyle {
    px: 22,
    weight: 600,
    line_height: 28,
};
/// Flyout headers, settings group.
pub const TYPE_SUBTITLE: TypeStyle = TypeStyle {
    px: 17,
    weight: 500,
    line_height: 24,
};
/// Default UI text.
pub const TYPE_BODY: TypeStyle = TypeStyle {
    px: 14,
    weight: 400,
    line_height: 20,
};
/// Buttons, tabs, taskbar labels.
pub const TYPE_LABEL: TypeStyle = TypeStyle {
    px: 13,
    weight: 500,
    line_height: 16,
};
/// Timestamps, hints, tray.
pub const TYPE_CAPTION: TypeStyle = TypeStyle {
    px: 11,
    weight: 400,
    line_height: 14,
};

// ── §7 Motion system ───────────────────────────────────────────────────────

/// A motion token: duration + cubic-bezier easing control points (x1,y1,x2,y2).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Motion {
    /// Duration in milliseconds.
    pub duration_ms: u32,
    /// Cubic-bezier easing (x1, y1, x2, y2).
    pub ease: (f32, f32, f32, f32),
}

/// Reduced-motion fallback (0ms).
pub const MOTION_INSTANT: Motion = Motion {
    duration_ms: 0,
    ease: (0.0, 0.0, 1.0, 1.0),
};
/// Hover/press state, focus ring appear (standard-out).
pub const MOTION_MICRO: Motion = Motion {
    duration_ms: 90,
    ease: (0.4, 0.0, 0.2, 1.0),
};
/// Toast in, tray flyout, button press.
pub const MOTION_FAST: Motion = Motion {
    duration_ms: 140,
    ease: (0.3, 0.0, 0.1, 1.0),
};
/// Start/quick-settings open, window open (decelerate).
pub const MOTION_STANDARD: Motion = Motion {
    duration_ms: 220,
    ease: (0.2, 0.0, 0.0, 1.0),
};
/// Maximize/restore, Vibe Mode transition.
pub const MOTION_EMPHASIZED: Motion = Motion {
    duration_ms: 320,
    ease: (0.2, 0.0, 0.0, 1.0),
};
/// Dismiss/close (accelerate-in; faster than entry).
pub const MOTION_EXIT: Motion = Motion {
    duration_ms: 120,
    ease: (0.4, 0.0, 1.0, 1.0),
};

/// Reduced-motion collapses all durations to 0ms (§7 / §8).
pub const REDUCED_MOTION_DURATION_MS: u32 = 0;

// ── §5.1 material.glass — the Liquid Glass tiered material (IDENTITY.md §2) ──
//
// THE cohesion mechanism: AthenaOS ships EXACTLY three glass tiers and never a
// fourth (IDENTITY.md §1 "Use tiers early. Stop inventing new glass per screen.").
// Every translucent surface picks one of chrome / panel / popover; the §7
// per-surface table in IDENTITY.md assigns the tier so there is no judgment call
// at the call site. These tiers REPLACE the old single `GLASS_TINT_*` pair, which
// was too dark (0x9E ≈ 62% over a near-black void → visually opaque) and untiered.
//
// The defining identity change (IDENTITY.md §2.1): glass is brighter and more
// translucent so the backdrop reads THROUGH it, and the tint hue moved off the
// dead navy (`0x1A1E2E`) onto a *blue-violet slate* (`0x1A223A`–`0x1E2642`) that
// picks up color from the aurora backdrop and reads as glass, not a gray card.

/// One glass tier: an ARGB tint composited over the live-blurred backdrop, plus
/// the blur radius (px) and the per-tier frost (white luminance-add) for that
/// tier. IDENTITY.md §2 — the only three materials.
///
/// Compositing order inside `draw_glass_surface` (raegfx): blurred backdrop →
/// slate **tint** ([`GlassTier::tint`]) → **frost** (low-alpha WHITE add,
/// [`GlassTier::frost`]) → iridescent rim. The frost is what turns a "dark card"
/// into "frosted glass": it lifts the interior luminance as a sheen ON TOP of the
/// tinted glass (so the slate cannot re-darken it away), and being a FIXED per-tier
/// white-add (not derived from alpha) it makes the interior luminance monotonic
/// across tiers regardless of the backdrop variance that was inverting the tier
/// ordering (Round-3 visual-QA). The pure-logic [`glass_tier_interior`] applies
/// exactly this order. IDENTITY.md §2.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GlassTier {
    /// ARGB `0xAARRGGBB` tint laid over the blurred backdrop. The alpha is the
    /// tier's "effective" opacity — lower = more backdrop reads through.
    pub tint: u32,
    /// Live-blur radius in px for this tier (chrome blurs most, popover least).
    pub blur_radius: u32,
    /// ARGB `0xAARRGGBB` white luminance-add laid ON TOP of the slate `tint` (the
    /// frost sheen) — the "frosted" lift. A FIXED per-tier step (NOT derived from
    /// alpha) so the interior luminance is monotonic chrome < panel < popover even
    /// when the backdrop variance swamps the alpha steps. White RGB (`0xFFFFFF`);
    /// only the alpha differs per tier (dark 0x04/0x23/0x48, light 0x06/0x18/0x2E).
    /// IDENTITY.md §2.
    pub frost: u32,
}

/// Live-blur radius for transient glass surfaces (`ThemeAbi.blur_radius` default).
/// Kept as the popover/default fallback (IDENTITY.md §8.1) for any path that has
/// not yet declared a tier; new code should pick a [`GlassTier`] instead.
pub const GLASS_BLUR_RADIUS: u32 = 16;

/// Accent halo behind the FOCUSED/ACTIVE element (IDENTITY-OBSIDIAN.md §3):
/// "lit on black". Drawn as an accent-colored soft shadow (additive halo)
/// under the active pill — sparse by rule: at most one glowing element per
/// surface. Alpha of the halo's peak; the renderer feathers it out.
pub const GLOW_ACCENT_ALPHA: u32 = 0x2E;
/// Halo feather radius (px) for [`GLOW_ACCENT_ALPHA`].
pub const GLOW_ACCENT_BLUR: u32 = 14;

/// `glass.frost` — the §2 luminance-add term the tier model was missing. A
/// low-alpha WHITE (`0xFFFFFF`) add composited inside `draw_glass_surface`
/// (raegfx) as a sheen over the blurred-and-tinted backdrop. This is the
/// ingredient that turns a "dark card" into "frosted glass": a ≈8% white lift so
/// the interior reads as luminous frost, not subtractive shade. It is the
/// reference/default frost magnitude; each [`GlassTier`] carries its own per-tier
/// [`GlassTier::frost`] (dark 0x04/0x23/0x48, light 0x06/0x18/0x2E) stepped around
/// this value so the interior luminance is monotonic across tiers (Round-3
/// visual-QA). IDENTITY.md §2.
pub const GLASS_FROST_LIGHTEN: u32 = 0x14_FF_FF_FF;

// ── §2.1 Dark tiers (default theme) ──
//
// chrome 25% / panel 45% / popover 60% effective alpha; blur 24 / 20 / 16 px.
// Chrome is the most see-through (always-on system chrome floats on the aurora);
// popover is the most opaque (transient surfaces need instant legibility over a
// busy backdrop).

/// `glass.chrome` (dark) — taskbar, title bars, always-on system chrome.
///
/// **OBSIDIAN re-bake (IDENTITY-OBSIDIAN.md §2, owner directive 2026-07-01):**
/// the frost recipe landed every tier at interior L77–112 — MID-GRAY, the
/// "toy OS / bad Linux clone" register. The dark references (ShadowMist Win11,
/// macOS dark) are NEAR-BLACK (L≤25) with a whisper of wallpaper color. Tints
/// go near-black at HIGH alpha; frost drops to a breath. Chrome stays the
/// most see-through tier (most wallpaper bleeds through the taskbar).
pub const GLASS_CHROME_DARK: GlassTier = GlassTier {
    tint: 0xE4_0C_0E_12,
    blur_radius: 24,
    // A breath, not milk — the top-light/hairline carry the "lit surface" cue.
    frost: 0x03_FF_FF_FF,
};
/// `glass.panel` (dark) — the workhorse: Control Center, Start, Settings panes,
/// Files sidebar, large cards. Obsidian near-black (IDENTITY-OBSIDIAN.md §2);
/// hierarchy inside a panel comes from `bg.raised`/`bg.elevated` solid steps,
/// never a white wash.
pub const GLASS_PANEL_DARK: GlassTier = GlassTier {
    tint: 0xF0_10_13_18,
    blur_radius: 20,
    frost: 0x04_FF_FF_FF,
};
/// `glass.popover` (dark) — transient surfaces over arbitrary content: menus,
/// flyouts, toasts, tooltips, command palette. The most opaque tier — floats
/// highest, instantly legible over any backdrop (IDENTITY-OBSIDIAN.md §2).
pub const GLASS_POPOVER_DARK: GlassTier = GlassTier {
    tint: 0xF6_14_17_1D,
    blur_radius: 16,
    frost: 0x06_FF_FF_FF,
};

// ── §2.2 Light tiers ("Lumen") ──
//
// The luminous *frosted* look — near-white milky glass with a faint cool blue
// tint (`F4F7FF` not pure white; pure-white glass reads as plastic, the cool tint
// reads as glass). chrome 35% / panel 55% / popover 70% effective alpha.

/// `glass.chrome` (light / "Lumen") — see [`GLASS_CHROME_DARK`]. 35% / 24px.
/// IDENTITY.md §2.2.
pub const GLASS_CHROME_LIGHT: GlassTier = GlassTier {
    tint: 0x59_F4_F7_FF,
    blur_radius: 24,
    // Light tiers already lift toward white via the milky tint, so the frost
    // steps are smaller — but kept monotonic chrome < panel < popover. IDENTITY.md §2.
    frost: 0x06_FF_FF_FF,
};
/// `glass.panel` (light / "Lumen") — see [`GLASS_PANEL_DARK`]. 55% / 20px.
/// IDENTITY.md §2.2.
pub const GLASS_PANEL_LIGHT: GlassTier = GlassTier {
    tint: 0x8C_FB_FC_FF,
    blur_radius: 20,
    frost: 0x18_FF_FF_FF,
    // (light frost steps 0x06/0x18/0x2E keep chrome < panel < popover luminance.)
};
/// `glass.popover` (light / "Lumen") — see [`GLASS_POPOVER_DARK`]. 70% / 16px.
/// IDENTITY.md §2.2.
pub const GLASS_POPOVER_LIGHT: GlassTier = GlassTier {
    tint: 0xB3_FF_FF_FF,
    blur_radius: 16,
    frost: 0x2E_FF_FF_FF,
};

/// **Deprecated alias** — `GLASS_TINT_DARK` now points at the new `glass.panel`
/// (dark) tint (IDENTITY.md §8.1: "leave them aliased to the panel tier for one
/// cycle so no call site breaks, then remove"). Existing consumers keep compiling
/// but render the new brighter/blue-violet panel glass instead of the old muddy
/// navy. New code MUST select a [`GlassTier`] (`GLASS_PANEL_DARK` etc.) directly.
pub const GLASS_TINT_DARK: u32 = GLASS_PANEL_DARK.tint;
/// **Deprecated alias** — `GLASS_TINT_LIGHT` now points at the new `glass.panel`
/// (light) tint (IDENTITY.md §8.1). See [`GLASS_TINT_DARK`].
pub const GLASS_TINT_LIGHT: u32 = GLASS_PANEL_LIGHT.tint;

// ── §2.3 Luminance auto-adjust bounds (over-light / over-dark adaptation) ────
//
// Glass over an arbitrary backdrop must stay legible without going opaque. The
// compositor samples the mean luminance of the *blurred backdrop under the
// surface* and nudges the tier's tint alpha: brighten over a bright backdrop
// (so text doesn't wash out on a white photo), thin over a dark backdrop (so the
// glass doesn't read as a solid black slab over a black video). Bounded so the
// surface never strays far from its tier — this is an automatic micro-adjust, NOT
// a new tier (the "stop inventing new glass" rule still holds). IDENTITY.md §2.3.

/// Mean-luma threshold above which a backdrop counts as "over-bright" → add
/// [`GLASS_ALPHA_BOOST`] to the tier alpha. IDENTITY.md §2.3.
///
/// Lowered 0.6 → 0.38 (SHIP-GATE a11y, raeen-accessibility WCAG audit): the
/// default aurora's brightest blob measures ~0.41 mean luma, so the old 0.6 gate
/// sat ABOVE the blob and the protective over-bright branch NEVER fired over the
/// one backdrop that actually threatens white-text legibility. 0.38 is just below
/// the blob luma so the over-bright adaptation engages where it is needed (and
/// still well above a normal dark-aurora region so the adjust stays a micro-tweak,
/// not a constant-on opacity bump). The real legibility guarantee is the hard
/// effective-luminance cap in [`glass_tier_interior`] — this threshold only steers
/// the *alpha micro-adjust*; the cap is what makes the AA bound unconditional.
pub const GLASS_LUMA_HI: f32 = 0.38;
/// Mean-luma threshold below which a backdrop counts as "over-dark" → subtract
/// [`GLASS_ALPHA_DROP`] from the tier alpha. IDENTITY.md §2.3.
pub const GLASS_LUMA_LO: f32 = 0.2;
/// Alpha added to a tier over a bright backdrop (mean luma > [`GLASS_LUMA_HI`]).
/// IDENTITY.md §2.3.
pub const GLASS_ALPHA_BOOST: u32 = 0x18;
/// Alpha subtracted from a tier over a dark backdrop (mean luma < [`GLASS_LUMA_LO`]).
/// IDENTITY.md §2.3.
pub const GLASS_ALPHA_DROP: u32 = 0x14;

/// **Legibility luma ceiling** — the maximum effective (mean-channel, 0..1)
/// luminance the *composited glass interior* may reach where body text sits, so
/// white [`Palette::text_primary`] (`F0F2F8`, rel-lum ≈ 0.888) always clears WCAG
/// AA 4.5:1. IDENTITY.md §2.3 / §9.
///
/// The dark-theme insight (SHIP-GATE a11y, raeen-accessibility): on a DARK theme
/// the body text is WHITE, so a glass surface cannot rise as bright as a
/// light-theme reference without the text washing out. Over a dark region the
/// tier interior is already far below this ceiling (panel ≈ 0.22, popover ≈ 0.37
/// over the aurora base) so the cap NEVER bites there — the frosted, translucent
/// dark-region look is preserved exactly. The cap engages ONLY over a bright
/// backdrop (an aurora blob, a light photo) where the frost-lifted interior would
/// otherwise climb past the legibility line. At 0.40 the worst bright region
/// (panel/popover over the aurora blue+teal peak) lands at ≈4.9–5.1:1 — a clean
/// AA pass with headroom — while the WCAG-exact crossing for `text.primary` is at
/// ≈0.435; 0.40 keeps a deliberate margin. (≤ the "~L48" raeen-accessibility
/// recommended ceiling; the WCAG-correct value is the binding constraint.)
pub const GLASS_INTERIOR_LUMA_CEIL: f32 = 0.40;

/// Apply the §2.3 luma auto-adjust to a tier's tint alpha for a measured backdrop
/// mean luminance, clamped to `[tier_alpha-GLASS_ALPHA_DROP, tier_alpha+0x20]` so
/// the surface never leaves its tier identity. Returns the adjusted ARGB tint.
/// Pure logic so the compositor and the host KAT share one implementation.
/// IDENTITY.md §2.3.
#[must_use]
pub fn glass_luma_adjust(tier: GlassTier, backdrop_mean_luma: f32) -> u32 {
    let base = chan_a(tier.tint);
    // The clamp window is the tier's identity envelope: at most -DROP below and
    // at most +0x20 above the tier's own alpha (IDENTITY.md §2.3 "Clamp to
    // [tier_alpha-0x18, tier_alpha+0x20]"; the -0x18 lower edge is the largest a
    // single thin-step can reach, GLASS_ALPHA_DROP=0x14 ≤ 0x18 so it stays inside).
    let lo = base.saturating_sub(0x18);
    let hi = (base + 0x20).min(0xFF);
    let adjusted = if backdrop_mean_luma > GLASS_LUMA_HI {
        (base + GLASS_ALPHA_BOOST).min(0xFF)
    } else if backdrop_mean_luma < GLASS_LUMA_LO {
        base.saturating_sub(GLASS_ALPHA_DROP)
    } else {
        base
    };
    let clamped = if adjusted < lo {
        lo
    } else if adjusted > hi {
        hi
    } else {
        adjusted
    };
    with_alpha(tier.tint, clamped)
}

/// Flatten a glass tier's interior over an opaque backdrop in the SAME order
/// `draw_glass_surface` (raegfx) composites: backdrop → slate **tint** → **frost**
/// (the per-tier white luminance-add sheen). Returns the opaque interior color a
/// user sees (pre-rim). The frost is laid on TOP of the tinted glass so the slate
/// can't re-darken it away — that is what makes the per-tier luminance a reliable
/// monotonic ordering term (chrome < panel < popover) instead of being swamped by
/// the tint alpha (Round-3 visual-QA found the tier ordering inverting). Pure
/// logic so the compositor, raegfx and the `tier_luminance_is_monotonic` KAT share
/// ONE definition of the interior. IDENTITY.md §2.
#[must_use]
pub fn glass_tier_interior(tier: GlassTier, backdrop_argb: u32) -> u32 {
    // §2.3 / §9 legibility luma cap (DARK theme — white text.primary): the frost
    // must never lift an already-bright surface (one sitting over an aurora blob /
    // a light photo) past the point where WHITE text.primary drops below AA. We
    // scale the interior RGB uniformly toward black until its mean-channel
    // luminance is ≤ GLASS_INTERIOR_LUMA_CEIL. Uniform scaling preserves hue (the
    // glass keeps its tint cast, just dimmer) and is a no-op over dark regions
    // (interior already below the ceiling) — so the frosted dark-region look is
    // UNTOUCHED and only the bright-region washout is pulled back.
    //
    // This is the DARK-theme interior. The LIGHT ("Lumen") theme paints DARK text on
    // deliberately bright milky glass, so capping its luminance would WRECK its
    // legibility instead of protecting it — light-theme callers and the tier-ordering
    // invariant use [`glass_tier_interior_raw`] (the uncapped frost ladder). The cap
    // is a white-text guarantee, not a tier property. IDENTITY.md §2.3 / §9.
    clamp_interior_luma(
        glass_tier_interior_raw(tier, backdrop_argb),
        GLASS_INTERIOR_LUMA_CEIL,
    )
}

/// The UNCAPPED glass interior — the raw frost ladder (backdrop → tint → frost) the
/// dark cap in [`glass_tier_interior`] wraps. This is the tier-ordering source of
/// truth (chrome < panel < popover in luminance) AND the interior the LIGHT
/// ("Lumen") theme uses directly: light glass is intentionally bright because the
/// light theme paints DARK text on it, so the white-text luminance cap must NOT
/// apply there. Pure logic shared by the compositor, raegfx and the
/// `tier_luminance_is_monotonic` KAT. IDENTITY.md §2.
#[must_use]
pub fn glass_tier_interior_raw(tier: GlassTier, backdrop_argb: u32) -> u32 {
    let tinted = flatten_over(tier.tint, backdrop_argb);
    flatten_over(tier.frost, tinted)
}

/// Mean-channel luminance (0..1) of an opaque ARGB color — the simple perceptual
/// weight the §2.3 backdrop sampler and the [`GLASS_INTERIOR_LUMA_CEIL`] cap use
/// (NOT the gamma-correct [`relative_luminance`]; this is the linear mean the
/// compositor measures cheaply per-region). IDENTITY.md §2.3.
#[must_use]
fn mean_luma(color: u32) -> f32 {
    (0.2126 * chan_r(color) as f32 + 0.7152 * chan_g(color) as f32 + 0.0722 * chan_b(color) as f32)
        / 255.0
}

/// Scale an opaque ARGB color's RGB uniformly toward black until its [`mean_luma`]
/// is ≤ `ceil`. A no-op when already at/below the ceiling. Uniform (hue-preserving)
/// so a clamped bright-backdrop glass still reads as the same tinted material, just
/// dimmed to the legibility line. IDENTITY.md §2.3 / §9.
#[must_use]
fn clamp_interior_luma(color: u32, ceil: f32) -> u32 {
    let l = mean_luma(color);
    if l <= ceil || l <= 0.0 {
        return color;
    }
    let factor = ceil / l;
    let scale = |c: u32| -> u32 {
        let v = (c as f32 * factor + 0.5) as u32;
        v.min(0xFF)
    };
    argb(
        chan_a(color),
        scale(chan_r(color)),
        scale(chan_g(color)),
        scale(chan_b(color)),
    )
}

// ── §2.4 The iridescent / chromatic edge — THE signature ────────────────────
//
// Real liquid glass refracts light into a thin rainbow at its border; flat
// Acrylic/Mica has none. We fake it cheaply: a 3px additive band hugging the
// inside of the rounded-rect border, hue cycling cyan → violet → warm-amber
// around the perimeter. It is felt at rest, but the signature must be LEGIBLE at
// the corners — so the alpha is at the in-band ceiling 0x40 (~25%) and the band
// is 3px (Round-3 visual-QA measured the old 0x33/2px rim as ZERO chromatic
// pixels; the rim must actually render). Drawn additively so it reads as a light
// refraction, not a painted border. Every glass surface gets it — that is the
// visual fingerprint that makes the system cohere. IDENTITY.md §2.4.

/// `glass.edge.iridescent` cyan stop (top / top-left of the perimeter sweep).
/// Alpha at the in-band ceiling `0x40` (~25%), drawn additively, so the rim is
/// legible at the corners rather than dropping to zero chromatic pixels (Round-3
/// visual-QA). IDENTITY.md §2.4.
pub const GLASS_EDGE_CYAN: u32 = 0x40_7C_E7_FF;
/// `glass.edge.iridescent` violet stop (right edge of the perimeter sweep).
/// Alpha at the `0x40` ceiling — see [`GLASS_EDGE_CYAN`]. IDENTITY.md §2.4.
pub const GLASS_EDGE_VIOLET: u32 = 0x40_B4_7C_FF;
/// `glass.edge.iridescent` warm-amber stop (bottom / bottom-right). Tracks the
/// Vibe accent warm-stop so Vibe Mode re-tints the rim. Alpha at the `0x40`
/// ceiling — see [`GLASS_EDGE_CYAN`]. IDENTITY.md §2.4.
pub const GLASS_EDGE_WARM: u32 = 0x40_FF_C9_7C;
/// Width in px of the iridescent rim band hugging the inside of the border.
/// Widened 2px → 3px so the chromatic sweep is actually visible at the corners
/// (Round-3 visual-QA). IDENTITY.md §2.4.
pub const GLASS_EDGE_BAND_PX: u32 = 3;

// ── §3 The signature backdrop — "Aurora Mesh" wallpaper palette ─────────────
//
// The flat navy void is half the identity problem: glass has nothing to refract.
// The default wallpaper is a procedural aurora mesh — soft radial color blobs
// drifting on a deep base, blended additively into smooth color fields. These are
// the palette tokens; raeen-gfx implements the `AuroraWallpaper: LiveWallpaper`
// drift/blend engine over them (FOLLOW-UP, not this crate). IDENTITY.md §3.

/// Aurora mesh base (dark) — a deep blue-violet night sky (`0x0B0F1E`), slightly
/// warmer/bluer than the old `0x0A0E1A` void so it reads as sky, not pure black.
/// IDENTITY.md §3.2.
pub const WALLPAPER_AURORA_BASE_DARK: u32 = 0xFF_0B_0F_1E;
/// Aurora mesh base (light / "Lumen Dawn") — a cool off-white. IDENTITY.md §3.3.
pub const WALLPAPER_AURORA_BASE_LIGHT: u32 = 0xFF_E8_EE_F8;
/// Aurora primary blob — RaeBlue. = [`RAEBLUE`]; this blob TRACKS the Vibe seed so
/// switching Vibe re-tints the wallpaper (IDENTITY.md §4.1). IDENTITY.md §3.2.
pub const AURORA_BLOB_BLUE: u32 = 0xFF_4E_9C_FF;
/// Aurora secondary blob — violet. FIXED (not Vibe-tracked) so the mesh keeps
/// depth across Vibe presets. IDENTITY.md §3.2.
pub const AURORA_BLOB_VIOLET: u32 = 0xFF_9B_5C_FF;
/// Aurora tertiary blob — teal/cyan. FIXED (not Vibe-tracked). IDENTITY.md §3.2.
pub const AURORA_BLOB_TEAL: u32 = 0xFF_3F_C8_E0;

// ── §4.1 Vibe Mode accent presets (named seeds) ─────────────────────────────
//
// Vibe Mode re-seeds ONE value — the accent — and `derive_accent` flows it to the
// full six-token ramp, the aurora primary blob, the iridescent rim warm-stop, the
// selection wash and the focus glow. Five shipped presets, each a single seed, so
// one tap re-skins the whole desktop coherently. IDENTITY.md §4.1.

/// Vibe "RaeBlue" (default) — the signature electric azure. = [`RAEBLUE`].
/// IDENTITY.md §4.1.
pub const VIBE_RAEBLUE: u32 = 0xFF_4E_9C_FF;
/// Vibe "Sunset" — warm coral. IDENTITY.md §4.1.
pub const VIBE_SUNSET: u32 = 0xFF_FF_6B_5C;
/// Vibe "Aurora" — teal-green. IDENTITY.md §4.1.
pub const VIBE_AURORA: u32 = 0xFF_3F_D0_A8;
/// Vibe "Orchid" — violet. IDENTITY.md §4.1.
pub const VIBE_ORCHID: u32 = 0xFF_C0_7C_FF;
/// Vibe "Gold" — warm amber. IDENTITY.md §4.1.
pub const VIBE_GOLD: u32 = 0xFF_F0_B8_4C;

/// The five shipped Vibe seeds in preset order (default first). Iterated by the
/// KAT and by any Vibe-Mode UI that lists presets. IDENTITY.md §4.1.
pub const VIBE_SEEDS: [u32; 5] = [
    VIBE_RAEBLUE,
    VIBE_SUNSET,
    VIBE_AURORA,
    VIBE_ORCHID,
    VIBE_GOLD,
];

// ── Host KATs (R10: a smoketest must be able to print FAIL) ─────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// |a - b| for channel diff.
    fn diff(a: u32, b: u32) -> u32 {
        if a >= b {
            a - b
        } else {
            b - a
        }
    }

    fn within2(got: u32, want: u32) -> bool {
        diff(chan_a(got), chan_a(want)) <= 2
            && diff(chan_r(got), chan_r(want)) <= 2
            && diff(chan_g(got), chan_g(want)) <= 2
            && diff(chan_b(got), chan_b(want)) <= 2
    }

    #[test]
    fn raeblue_ramp_matches_design_table() {
        let r = derive_accent(RAEBLUE, &DARK);
        // Exact-match tokens (alpha/passthrough math — no rounding slack).
        assert_eq!(r.base, 0xFF_4E_9C_FF, "accent.base must be the seed");
        assert_eq!(r.subtle, 0x3D_4E_9C_FF, "accent.subtle = seed @ 0x3D alpha");
        assert_eq!(r.glow, 0x66_4E_9C_FF, "accent.glow = seed @ 0x66 alpha");
        // Within-±2 tokens (per the §4.3 table).
        assert!(
            within2(r.hover, 0xFF_6E_AE_FF),
            "accent.hover {:#010X} not within 2 of 0xFF6EAEFF",
            r.hover
        );
        assert!(
            within2(r.active, 0xFF_3A_80_DB),
            "accent.active {:#010X} not within 2 of 0xFF3A80DB",
            r.active
        );
    }

    #[test]
    fn accent_text_picks_seed_when_contrast_passes() {
        // RaeBlue on the dark bg meets 4.5:1, so accent.text == seed (per the
        // TASK rule; the §4.3 table's 0xFF6EAEFF is inconsistent with its own
        // stated rule — see the crate-level note / report).
        let r = derive_accent(RAEBLUE, &DARK);
        assert!(contrast_ratio(RAEBLUE, DARK.bg_base) >= 4.5);
        assert_eq!(r.text, RAEBLUE);
    }

    #[test]
    fn ftype_palette_matches_design_table() {
        // §4.4 fixed semantic colors — exact.
        assert_eq!(FTYPE_EXEC, 0xFF_3F_BF_7F, "ftype.exec = state.ok green");
        assert_eq!(FTYPE_EXEC, DARK.state_ok, "ftype.exec aliases state.ok");
        assert_eq!(FTYPE_MEDIA, 0xFF_C0_7C_FF, "ftype.media violet");
        assert_eq!(FTYPE_DOC, 0xFF_F0_C8_5C, "ftype.doc gold");
        assert_eq!(FTYPE_ARCHIVE, 0xFF_F0_A0_3C, "ftype.archive amber");
        assert_eq!(
            ftype_neutral(&DARK),
            DARK.text_secondary,
            "ftype.neutral = text.secondary"
        );
        // The two accent-tracking exceptions: dir/code follow the seed so they
        // re-skin with Vibe Mode (the cohesion contract).
        assert_eq!(
            ftype_dir(RAEBLUE, &DARK),
            RAEBLUE,
            "ftype.dir tracks accent"
        );
        assert_eq!(
            ftype_code(RAEBLUE, &DARK),
            RAEBLUE,
            "ftype.code tracks accent"
        );
        let alt = 0xFF_FF_50_80; // a different seed (Vibe switch)
        assert_eq!(ftype_dir(alt, &DARK), alt, "ftype.dir follows a re-skin");
        assert_ne!(
            ftype_dir(alt, &DARK),
            ftype_dir(RAEBLUE, &DARK),
            "a Vibe switch must move ftype.dir"
        );
        // …while the fixed hues do NOT move on a re-skin.
        assert_eq!(
            FTYPE_MEDIA, 0xFF_C0_7C_FF,
            "ftype.media is seed-independent"
        );
    }

    #[test]
    fn concentric_clamps_to_xs() {
        // Normal case: 16 - 8 = 8.
        assert_eq!(concentric(RADIUS_LG, SPACE_2), 8);
        // Clamp: 4 - 16 would underflow → RADIUS_XS.
        assert_eq!(concentric(RADIUS_XS, SPACE_4), RADIUS_XS);
        // Just-below-xs result clamps up.
        assert_eq!(concentric(8, 6), RADIUS_XS);
    }

    #[test]
    fn primary_text_contrast_is_aa_compliant() {
        // text.primary on bg.base must clear the AA 7:1 target (§4.2).
        let c = contrast_ratio(DARK.text_primary, DARK.bg_base);
        assert!(c >= 7.0, "text.primary contrast {:.2} < 7:1", c);
        // White on a near-black bg must clear 7:1 with room to spare.
        let w = contrast_ratio(0xFF_FF_FF_FF, DARK.bg_base);
        assert!(w >= 7.0, "white-on-bg.base contrast {:.2} < 7:1", w);
    }

    #[test]
    fn radius_pill_and_elevation() {
        assert_eq!(radius_pill(40), 20);
        assert_eq!(ELEV_3.radius, 28);
        // elev.focus carries the accent glow color verbatim.
        let r = derive_accent(RAEBLUE, &DARK);
        assert_eq!(elev_focus(r.glow).color, 0x66_4E_9C_FF);
        // Light-mode shadow alpha scaled by 0.6: 0x40 (64) * 6 / 10 = 38 = 0x26.
        assert_eq!(chan_a(scale_shadow_alpha(ELEV_2.color, 6, 10)), 0x26);
    }

    /// Token pairs known to fail WCAG AA and deliberately deferred (genuine debt,
    /// not yet fixable). EMPTY today: the two light-mode defects this audit
    /// originally caught — `accent_focus_ring/light.bg_base` (was 2.40, RaeBlue
    /// ring on light bg) and `light.text_tertiary/bg_base` (was 2.73) — were FIXED
    /// at the token level (deeper `LIGHT_FOCUS_RING`, darkened `LIGHT.text_tertiary`)
    /// rather than tracked as debt, so the audit below now enforces ALL 16 pairs
    /// strictly. Anything added here must be real, justified, deferred debt.
    const KNOWN_CONTRAST_DEFECTS: [&str; 0] = [];

    #[test]
    fn contrast_audit_passes_wcag_aa_strictly() {
        // SHIP-GATE KAT (Phase 19 / PARITY_MATRIX §J): EVERY shipped
        // legibility-bearing token pair must clear its WCAG AA bar (4.5:1 body,
        // 3.0:1 large/UI/focus). With KNOWN_CONTRAST_DEFECTS now empty the audit
        // is STRICT — no pair gets a pass. This is genuinely FAIL-able: it walks
        // the real composited colors via `passes()`, so if ANY token color drops
        // below its threshold this flips to FAIL naming the offending pair + its
        // ratio (the `contrast_audit_is_failable_not_tautological` test proves
        // `passes()` itself is not a tautology).
        let pairs = shipped_contrast_pairs();
        let mut worst_fail: Option<(&str, f32, f32)> = None;
        for p in &pairs {
            let tracked = KNOWN_CONTRAST_DEFECTS.contains(&p.name);
            // No pair should be tracked-deferred today; if one is, fail loudly so
            // the list cannot quietly hide a regression.
            assert!(
                !tracked,
                "{} is in KNOWN_CONTRAST_DEFECTS but the list must be empty \
                 (token-level fixes preferred over tracked debt)",
                p.name,
            );
            if !p.passes()
                && worst_fail.map_or(true, |(_, r, t)| (p.ratio() - p.kind.threshold()) < (r - t))
            {
                worst_fail = Some((p.name, p.ratio(), p.kind.threshold()));
            }
        }
        assert!(
            worst_fail.is_none(),
            "WCAG AA contrast FAILURE: {} = {:.2} (needs {:.1})",
            worst_fail.unwrap().0,
            worst_fail.unwrap().1,
            worst_fail.unwrap().2,
        );
        // The audit must have actually run over the real, full pair list.
        assert_eq!(pairs.len(), 24, "audit must cover all shipped pairs");
    }

    #[test]
    fn audit_report_names_the_worst_pair() {
        // The report's worst-pair fields must point at the genuinely lowest-
        // margin pair so any future failure message is actionable. After the
        // token fixes the whole palette passes, and the tightest-margin pair is
        // the light-mode focus ring we just deepened (it clears 3.0:1 but with
        // the smallest headroom of the 16). This test is NOT a tautology: it
        // independently recomputes the lowest-margin pair from the raw pair list
        // and asserts the report agrees, so a wrong worst-pair calculation fails.
        let report = audit_contrast();
        assert_eq!(report.pairs, 24);
        assert_eq!(
            report.failed, 0,
            "all 24 shipped pairs must clear their bar"
        );
        assert_eq!(report.passed, 24);

        // Independently find the genuine lowest-margin pair.
        let pairs = shipped_contrast_pairs();
        let mut min_name = pairs[0].name;
        let mut min_margin = pairs[0].ratio() - pairs[0].kind.threshold();
        for p in &pairs {
            let m = p.ratio() - p.kind.threshold();
            if m < min_margin {
                min_margin = m;
                min_name = p.name;
            }
        }
        assert_eq!(
            report.worst_name, min_name,
            "report.worst_name must be the genuine lowest-margin pair"
        );
        assert_eq!(
            report.worst_name, "accent_focus_ring/light.bg_base",
            "the deepened light focus ring is the tightest-margin pair"
        );
        // The worst pair now PASSES (margin >= 0) — the palette is strict-clean.
        assert!(
            report.worst_ratio >= report.worst_threshold,
            "even the worst pair clears its bar ({:.2} >= {:.1})",
            report.worst_ratio,
            report.worst_threshold
        );
    }

    #[test]
    fn contrast_audit_is_failable_not_tautological() {
        // Prove the audit can print FAIL: a deliberately illegible pair (grey on
        // grey, ~1.0:1) MUST be reported as failing. If `passes()` were a
        // tautology, this would not catch it.
        let bad = ContrastPair {
            name: "grey/grey",
            fg: 0xFF_80_80_80,
            bg: 0xFF_88_88_88,
            kind: ContrastKind::BodyText,
        };
        assert!(bad.ratio() < AA_BODY, "grey-on-grey must be below 4.5:1");
        assert!(!bad.passes(), "an illegible pair MUST fail passes()");
        // And a clearly-legible pair must pass — both directions exercised.
        let good = ContrastPair {
            name: "white/black",
            fg: 0xFF_FF_FF_FF,
            bg: 0xFF_00_00_00,
            kind: ContrastKind::BodyText,
        };
        assert!(good.passes(), "white-on-black must clear 4.5:1");
    }

    #[test]
    fn legibility_luma_cap_rescues_text_over_bright_aurora() {
        // SHIP-GATE a11y — OBSIDIAN re-contract (IDENTITY-OBSIDIAN.md §2/§5).
        // The frost-era version of this KAT proved the cap RESCUED white text
        // (raw interiors failed AA over the bright aurora). Obsidian designs
        // the bug class OUT: near-black high-alpha tiers keep the RAW interior
        // ≥ AAA (7:1) for white text over even the brightest aurora peak — the
        // cap becomes a safety net, not a load-bearing fix. FAIL-able: a
        // regression back toward milky tiers (tint alpha down / RGB up / a fat
        // frost) drops the raw ratio under AAA and these assertions flip.
        let add = |a: u32, b: u32| -> u32 {
            argb(
                0xFF,
                (chan_r(a) + chan_r(b)).min(0xFF),
                (chan_g(a) + chan_g(b)).min(0xFF),
                (chan_b(a) + chan_b(b)).min(0xFF),
            )
        };
        let peak = add(
            add(WALLPAPER_AURORA_BASE_DARK, AURORA_BLOB_BLUE),
            AURORA_BLOB_TEAL,
        );
        let text = DARK.text_primary;
        const AAA_BODY: f32 = 7.0;
        for (name, tier) in [("panel", GLASS_PANEL_DARK), ("popover", GLASS_POPOVER_DARK)] {
            // Each bright single blob AND the multi-blob peak.
            for (bg_name, bg) in [
                ("blue_blob", AURORA_BLOB_BLUE),
                ("teal_blob", AURORA_BLOB_TEAL),
                ("peak", peak),
            ] {
                let raw = glass_tier_interior_raw(tier, bg);
                let capped = glass_tier_interior(tier, bg);
                let raw_ratio = contrast_ratio(text, raw);
                let capped_ratio = contrast_ratio(text, capped);
                // Obsidian: the RAW interior clears AAA — legible by material,
                // not by clamp.
                assert!(
                    raw_ratio >= AAA_BODY,
                    "{name}@{bg_name}: obsidian RAW interior must clear AAA \
                     (got {raw_ratio:.2}, needs >= {AAA_BODY})"
                );
                assert!(
                    capped_ratio >= AAA_BODY,
                    "{name}@{bg_name}: capped interior must clear AAA \
                     (got {capped_ratio:.2}, needs >= {AAA_BODY})"
                );
                // The cap (still armed as a safety net) only dims, never brightens.
                assert!(
                    mean_luma(capped) <= mean_luma(raw) + f32::EPSILON,
                    "{name}@{bg_name}: cap must not brighten"
                );
                // §5 acceptance: interiors sit DEEP — L ≤ 30/255 over the peak.
                assert!(
                    mean_luma(raw) <= 0.14,
                    "{name}@{bg_name}: obsidian interior must read near-black \
                     (mean_luma {:.3} > 0.14)",
                    mean_luma(raw)
                );
            }
        }
    }

    #[test]
    fn legibility_luma_cap_is_noop_in_dark_regions() {
        // The cap must NOT bite over the dark aurora valley — the frosted, see-through
        // dark-region glass look the design depends on is preserved EXACTLY. Over the
        // aurora base the panel/popover interiors are already well below the ceiling,
        // so capped == raw (the cap is a no-op) and the dark-region math that already
        // PASSES is untouched.
        for tier in [GLASS_CHROME_DARK, GLASS_PANEL_DARK, GLASS_POPOVER_DARK] {
            let raw = glass_tier_interior_raw(tier, WALLPAPER_AURORA_BASE_DARK);
            let capped = glass_tier_interior(tier, WALLPAPER_AURORA_BASE_DARK);
            assert!(
                mean_luma(raw) <= GLASS_INTERIOR_LUMA_CEIL,
                "dark-region interior must already be under the ceiling (luma {:.3})",
                mean_luma(raw)
            );
            assert_eq!(
                raw, capped,
                "the cap must be a NO-OP over the dark aurora valley \
                 (frosted dark-region look preserved)"
            );
        }
        // And light "Lumen" glass is never capped (dark text on bright glass): its
        // public interior is the raw bright interior, unchanged.
        let lp_raw = glass_tier_interior_raw(GLASS_PANEL_LIGHT, WALLPAPER_AURORA_BASE_LIGHT);
        assert!(
            mean_luma(lp_raw) > GLASS_INTERIOR_LUMA_CEIL,
            "light panel glass is intentionally bright (dark text rides on it)"
        );
    }

    #[test]
    fn high_contrast_aaa_bar_is_failable() {
        // Prove the AAA (7:1) class is a real, distinct, FAIL-able bar: a pair
        // that clears AA (4.5:1) but NOT AAA must be reported as failing under the
        // HighContrastAaa kind. `LIGHT.text_secondary/bg_base` clears AA on light
        // but does not reach 7:1, so it is the perfect counterexample.
        assert_eq!(ContrastKind::HighContrastAaa.threshold(), 7.0);
        let aa_only = ContrastPair {
            name: "dark.state_danger/bg_base (AA-only)",
            fg: DARK.state_danger,
            bg: DARK.bg_base,
            kind: ContrastKind::HighContrastAaa,
        };
        let r = aa_only.ratio();
        assert!(r >= AA_BODY, "this pair should clear AA ({:.2} >= 4.5)", r);
        assert!(r < AAA, "this pair should NOT clear AAA ({:.2} < 7.0)", r);
        assert!(
            !aa_only.passes(),
            "a sub-7:1 pair MUST fail under the AAA class"
        );
    }

    #[test]
    fn glass_flatten_changes_the_measured_surface() {
        // The glass tint is 62% alpha; flattening it over the void must yield a
        // surface distinct from the raw tint RGB and from the void — otherwise
        // the audit would be measuring the wrong (pre-blend) color.
        let glass = flatten_over(GLASS_TINT_DARK, DARK.bg_base);
        assert_eq!(chan_a(glass), 0xFF, "flattened surface is opaque");
        // White-on-glass must still be legible (the surface stays dark enough).
        let c = contrast_ratio(DARK.text_primary, glass);
        assert!(
            c >= 4.5,
            "text.primary on composited glass {:.2} < 4.5:1",
            c
        );
    }

    #[test]
    fn high_contrast_palette_is_exemplary() {
        // The HC mode itself must be the BEST contrast we ship: clear AAA (7:1).
        let c = contrast_ratio(HIGH_CONTRAST.text_primary, HIGH_CONTRAST.bg_base);
        assert!(c >= AAA, "HC text/bg {:.2} < 7:1 (AAA)", c);
        // The HC focus ring and tertiary text are also held to AAA in the audit —
        // assert it here so a token regression that drops them below 7:1 is caught.
        let f = contrast_ratio(HIGH_CONTRAST_FOCUS_RING, HIGH_CONTRAST.bg_base);
        assert!(f >= AAA, "HC focus ring/bg {:.2} < 7:1 (AAA)", f);
        let t = contrast_ratio(HIGH_CONTRAST.text_tertiary, HIGH_CONTRAST.bg_base);
        assert!(t >= AAA, "HC text_tertiary/bg {:.2} < 7:1 (AAA)", t);
    }

    #[test]
    fn active_palette_swaps_under_high_contrast() {
        // The live forced-colors swap (audit P0 #3): active_palette() must be the
        // normal palette when off and the HC palette when on. FAIL-able — if the
        // flag were ignored, active_palette() would never return HIGH_CONTRAST.
        set_high_contrast(false);
        assert!(!high_contrast());
        assert_eq!(*active_palette(), DARK, "off -> normal palette");
        assert_eq!(
            active_focus_ring(0xDEAD_BEEF),
            0xDEAD_BEEF,
            "off -> normal ring"
        );
        set_high_contrast(true);
        assert!(high_contrast());
        assert_eq!(*active_palette(), HIGH_CONTRAST, "on -> HC palette");
        assert_eq!(
            active_focus_ring(0xDEAD_BEEF),
            HIGH_CONTRAST_FOCUS_RING,
            "on -> cyan HC ring"
        );
        // Leave the flag clean for any sibling test.
        set_high_contrast(false);
    }

    // ── IDENTITY.md §8.7: the four new FAIL-able Liquid Glass KATs ──────────

    #[test]
    fn glass_tiers_are_ordered() {
        // IDENTITY.md §8.7 #1: chrome < panel < popover in effective alpha — the
        // tier discipline ("most see-through chrome → most opaque popover"). A
        // tier inversion (e.g. someone bumping chrome's alpha past panel) = FAIL.
        // FAIL-ability: swap GLASS_CHROME_DARK.tint's alpha to >= panel's and this
        // strict-ordering chain breaks.
        let chrome = chan_a(GLASS_CHROME_DARK.tint);
        let panel = chan_a(GLASS_PANEL_DARK.tint);
        let popover = chan_a(GLASS_POPOVER_DARK.tint);
        assert!(
            chrome < panel && panel < popover,
            "dark tiers must be ordered chrome({chrome:#X}) < panel({panel:#X}) < popover({popover:#X})"
        );
        // Same discipline holds for the light "Lumen" tiers.
        let cl = chan_a(GLASS_CHROME_LIGHT.tint);
        let pl = chan_a(GLASS_PANEL_LIGHT.tint);
        let ol = chan_a(GLASS_POPOVER_LIGHT.tint);
        assert!(
            cl < pl && pl < ol,
            "light tiers must be ordered chrome({cl:#X}) < panel({pl:#X}) < popover({ol:#X})"
        );
        // Blur radius decreases as opacity increases (chrome blurs most): 24/20/16.
        assert!(
            GLASS_CHROME_DARK.blur_radius > GLASS_PANEL_DARK.blur_radius
                && GLASS_PANEL_DARK.blur_radius > GLASS_POPOVER_DARK.blur_radius,
            "blur radius must decrease chrome > panel > popover"
        );
    }

    #[test]
    fn glass_over_aurora_stays_legible() {
        // IDENTITY.md §8.7 #2: text.primary over each dark tier FLATTENED OVER THE
        // AURORA BASE (the real backdrop now, not bg_base) must clear AA 4.5:1.
        // This is the brighter/thinner-glass legibility risk made FAIL-able: drop
        // a tier's alpha far enough and its flattened surface goes too light for
        // dark text → ratio < 4.5 → FAIL. We assert it both directly here AND via
        // the shipped-pairs audit (which now carries the three @aurora pairs).
        for (name, tier) in [
            ("chrome", GLASS_CHROME_DARK),
            ("panel", GLASS_PANEL_DARK),
            ("popover", GLASS_POPOVER_DARK),
        ] {
            let surface = flatten_over(tier.tint, WALLPAPER_AURORA_BASE_DARK);
            let c = contrast_ratio(DARK.text_primary, surface);
            assert!(
                c >= AA_BODY,
                "text.primary over glass.{name} @aurora = {c:.2} < 4.5:1"
            );
        }
        // And the audit must actually carry these aurora pairs (not just bg_base),
        // so a regression there cannot hide behind the old void-only flatten.
        let pairs = shipped_contrast_pairs();
        assert!(
            pairs
                .iter()
                .any(|p| p.name == "dark.text_primary/glass.panel@aurora"),
            "the audit must flatten the panel tier over the aurora base"
        );
    }

    #[test]
    fn iridescent_rim_alpha_is_subtle() {
        // IDENTITY.md §8.7 #3: each GLASS_EDGE_* alpha ∈ [0x20, 0x40] — the rim is
        // *felt*, not a neon outline. FAIL-ability: bump any GLASS_EDGE_* alpha to,
        // say, 0xCC (a bright painted border) and this trips. The lower bound
        // catches a rim so faint it's invisible (defeating the signature).
        for (name, c) in [
            ("cyan", GLASS_EDGE_CYAN),
            ("violet", GLASS_EDGE_VIOLET),
            ("warm", GLASS_EDGE_WARM),
        ] {
            let a = chan_a(c);
            assert!(
                (0x20..=0x40).contains(&a),
                "iridescent rim {name} alpha {a:#X} outside subtle band [0x20,0x40] (neon = FAIL)"
            );
        }
        // The band is a thin 3px refraction (Round-3 widened 2px→3px so it
        // actually renders), not a thick frame.
        assert_eq!(GLASS_EDGE_BAND_PX, 3, "iridescent rim band must be 3px");
    }

    #[test]
    fn tier_luminance_is_monotonic() {
        // Round-3 visual-QA: the tier ordering was inverting because backdrop
        // variance swamped the alpha steps. The FIXED per-tier `frost` white-add
        // makes the INTERIOR LUMINANCE monotonic chrome < panel < popover
        // regardless of the backdrop. Flatten each dark tier (frost THEN tint) over
        // a fixed mid backdrop and assert strictly increasing luminance.
        // FAIL-ability: drop popover's frost to chrome's level (or below) and the
        // higher (darker) popover tint alpha pulls its luminance under panel's →
        // the strict chain breaks → FAIL.
        // Monotonicity is a property of the RAW frost ladder (tint → frost), not of
        // the capped output: over a bright backdrop the §2.3/§9 legibility cap
        // deliberately pulls every dark tier down to the same luminance ceiling (text
        // wins over tier distinction there), so the ordering invariant is checked on
        // `glass_tier_interior_raw`. The cap's own behaviour is checked separately by
        // `legibility_luma_cap_*`.
        let mid = 0xFF_80_80_80u32;
        let chrome = relative_luminance(glass_tier_interior_raw(GLASS_CHROME_DARK, mid));
        let panel = relative_luminance(glass_tier_interior_raw(GLASS_PANEL_DARK, mid));
        let popover = relative_luminance(glass_tier_interior_raw(GLASS_POPOVER_DARK, mid));
        assert!(
            chrome < panel && panel < popover,
            "dark tier interior luminance must be monotonic: \
             chrome={chrome:.4} < panel={panel:.4} < popover={popover:.4}"
        );
        // The light "Lumen" tiers must be monotonic too. (Light glass is uncapped —
        // dark text on bright glass — so it always uses the raw interior.)
        let lc = relative_luminance(glass_tier_interior_raw(GLASS_CHROME_LIGHT, mid));
        let lp = relative_luminance(glass_tier_interior_raw(GLASS_PANEL_LIGHT, mid));
        let lo = relative_luminance(glass_tier_interior_raw(GLASS_POPOVER_LIGHT, mid));
        assert!(
            lc < lp && lp < lo,
            "light tier interior luminance must be monotonic: \
             chrome={lc:.4} < panel={lp:.4} < popover={lo:.4}"
        );
        // The frost is a WHITE add (RGB 0xFFFFFF), only alpha differs per tier, and
        // the per-tier frost alpha is itself monotonic (the cause of the lum order).
        for t in [GLASS_CHROME_DARK, GLASS_PANEL_DARK, GLASS_POPOVER_DARK] {
            assert_eq!(t.frost & 0x00FF_FFFF, 0x00FF_FFFF, "frost must be white");
        }
        assert!(
            chan_a(GLASS_CHROME_DARK.frost) < chan_a(GLASS_PANEL_DARK.frost)
                && chan_a(GLASS_PANEL_DARK.frost) < chan_a(GLASS_POPOVER_DARK.frost),
            "per-tier frost alpha must increase chrome < panel < popover"
        );
        // GLASS_FROST_LIGHTEN is the documented §2 base white-add (≈8%).
        assert_eq!(
            GLASS_FROST_LIGHTEN & 0x00FF_FFFF,
            0x00FF_FFFF,
            "base frost is white"
        );
        assert_eq!(
            chan_a(GLASS_FROST_LIGHTEN),
            0x14,
            "base frost ≈8% white add"
        );
    }

    #[test]
    fn iridescent_rim_alpha_ceiling_is_legal() {
        // Round-3 raised every rim alpha to the in-band CEILING 0x40 and the band
        // to 3px. Confirm the existing `iridescent_rim_alpha_is_subtle` band still
        // admits 0x40 (it asserts [0x20, 0x40] inclusive) and the values landed.
        assert_eq!(
            chan_a(GLASS_EDGE_CYAN),
            0x40,
            "cyan rim at the 0x40 ceiling"
        );
        assert_eq!(
            chan_a(GLASS_EDGE_VIOLET),
            0x40,
            "violet rim at the 0x40 ceiling"
        );
        assert_eq!(
            chan_a(GLASS_EDGE_WARM),
            0x40,
            "warm rim at the 0x40 ceiling"
        );
        assert_eq!(GLASS_EDGE_BAND_PX, 3, "rim band widened to 3px");
        for c in [GLASS_EDGE_CYAN, GLASS_EDGE_VIOLET, GLASS_EDGE_WARM] {
            assert!(
                (0x20..=0x40).contains(&chan_a(c)),
                "0x40 is the legal ceiling"
            );
        }
    }

    #[test]
    fn vibe_seeds_are_distinct_and_valid() {
        // IDENTITY.md §8.7 #4: the 5 Vibe seeds are pairwise distinct AND each
        // produces a valid derive_accent ramp. FAIL-ability: set two seeds equal
        // (Vibe presets that look identical) → distinctness trips; or break
        // derive_accent so hover/active stop bracketing the seed → validity trips.
        assert_eq!(VIBE_SEEDS.len(), 5, "exactly five shipped Vibe presets");
        assert_eq!(VIBE_RAEBLUE, RAEBLUE, "default Vibe is the RaeBlue seed");
        // Pairwise-distinct.
        for i in 0..VIBE_SEEDS.len() {
            for j in (i + 1)..VIBE_SEEDS.len() {
                assert_ne!(
                    VIBE_SEEDS[i], VIBE_SEEDS[j],
                    "Vibe seeds {i} and {j} must be distinct"
                );
            }
        }
        // Each seed yields a coherent ramp: base == seed, hover lighter than the
        // seed, active darker than the seed, and the alpha-derived tokens carry
        // their documented alphas. A no-op or inverted ramp fails here.
        for &seed in VIBE_SEEDS.iter() {
            let r = derive_accent(seed, &DARK);
            assert_eq!(r.base, seed, "ramp.base must be the seed {seed:#010X}");
            assert!(
                relative_luminance(r.hover) > relative_luminance(seed),
                "ramp.hover must be lighter than seed {seed:#010X}"
            );
            assert!(
                relative_luminance(r.active) < relative_luminance(seed),
                "ramp.active must be darker than seed {seed:#010X}"
            );
            assert_eq!(chan_a(r.subtle), 0x3D, "ramp.subtle alpha for {seed:#010X}");
            assert_eq!(chan_a(r.glow), 0x66, "ramp.glow alpha for {seed:#010X}");
        }
    }

    #[test]
    fn glass_luma_adjust_stays_within_tier() {
        // The §2.3 auto-adjust must brighten over bright, thin over dark, and NEVER
        // leave the tier envelope [alpha-0x18, alpha+0x20]. FAIL-able: an unbounded
        // adjust over a fully-bright/fully-dark backdrop would escape the envelope.
        let tier = GLASS_PANEL_DARK;
        let base = chan_a(tier.tint);
        // Over a bright backdrop → boost, but capped at +0x20 AND at 0xFF
        // (obsidian tiers sit near the top of the alpha range, so the boost
        // saturates at fully-opaque instead of overflowing).
        let bright = chan_a(glass_luma_adjust(tier, 0.95));
        assert_eq!(
            bright,
            (base + GLASS_ALPHA_BOOST).min(0xFF),
            "bright backdrop boosts by 0x18 (saturating at 0xFF)"
        );
        assert!(
            bright <= base + 0x20,
            "boost must stay within +0x20 of the tier"
        );
        // Over a dark backdrop → thin by 0x14, never below the lower envelope.
        let dark = chan_a(glass_luma_adjust(tier, 0.05));
        assert_eq!(dark, base - GLASS_ALPHA_DROP, "dark backdrop thins by 0x14");
        assert!(
            dark >= base.saturating_sub(0x18),
            "thin must stay within -0x18"
        );
        // Mid-luma backdrop (strictly between GLASS_LUMA_LO=0.2 and the lowered
        // GLASS_LUMA_HI=0.38) → unchanged (no new glass invented).
        assert!(
            0.30 > GLASS_LUMA_LO && 0.30 < GLASS_LUMA_HI,
            "0.30 must sit inside the neutral [LO,HI] band"
        );
        assert_eq!(
            chan_a(glass_luma_adjust(tier, 0.30)),
            base,
            "mid-luma backdrop leaves the tier alpha untouched"
        );
        // RGB never changes — only the alpha is nudged.
        assert_eq!(
            glass_luma_adjust(tier, 0.95) & 0x00FF_FFFF,
            tier.tint & 0x00FF_FFFF,
            "luma-adjust touches alpha only, never the tint hue"
        );
    }

    #[test]
    fn deliberately_checkable_invariant() {
        // A real, fail-able assertion (NOT assert!(true)): darken must reduce
        // luminance and lighten must raise it relative to the seed. If the
        // ramp math were inverted or a no-op, this prints FAIL.
        let r = derive_accent(RAEBLUE, &DARK);
        let seed_l = relative_luminance(RAEBLUE);
        assert!(
            relative_luminance(r.hover) > seed_l,
            "hover must be lighter than the seed"
        );
        assert!(
            relative_luminance(r.active) < seed_l,
            "active must be darker than the seed"
        );
    }
}
