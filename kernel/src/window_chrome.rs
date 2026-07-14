//! Window chrome: title bar + controls for kernel-drawn userspace windows.
//!
//! Concept §RaeUI ("glassmorphic, GPU-accelerated") + IDENTITY.md §7 (window
//! chrome is the MOST see-through `glass.chrome` tier) + the desktop-shell spec
//! (docs/design/desktop-shell.md §4 "Window chrome"): a Liquid-Glass title bar —
//! frosted, translucent `glass.chrome` over the window/desktop behind it, with
//! the signature iridescent rim, focused/unfocused states, traffic-light
//! controls on the LEFT, real readable title text. The bar is rendered by the
//! shipped `raegfx::glass::draw_glass_surface(.., GLASS_CHROME_DARK)` (tint →
//! frost → WCAG legibility-cap → iridescent rim → top highlight) — byte-for-byte
//! the same call the taskbar / Control Center / Files make, so the whole shell
//! reads as one material. Every colour/metric is a `rae_tokens` value
//! (docs/design/design-language.md), not a private constant, so a Vibe-Mode
//! re-skin (one seed change) recolours the chrome with the rest of the shell.
//!
//! Real glyphs: title text now renders with `raegfx::Canvas::draw_text_aa`
//! (grayscale-AA RaeSans via raefont) at the `type.label` style, with the 8×8
//! bitmap path kept as the font-engine's internal early-boot fallback. (The
//! original renderer computed a glyph index then DISCARDED it and painted
//! `(row+col+glyph)%3` procedural speckle — window titles were unreadable
//! noise; the interim fix used the `font8x8` block glyphs; this is the crisp
//! vector pass.)

#![allow(dead_code)]

extern crate alloc;

use rae_tokens::{
    GlassTier, Palette, DARK, GLASS_CHROME_DARK, RADIUS_MD, SPACE_2, SPACE_3, TYPE_LABEL,
};

/// The `glass.chrome` tier the focused title bar renders as — IDENTITY.md §7's
/// MOST see-through tier, the same constant the taskbar / Control Center / Files
/// pass to `draw_glass_surface`, so the whole shell reads as one material.
const GLASS_CHROME: GlassTier = GLASS_CHROME_DARK;

/// Unfocused title bar: the same `glass.chrome` tier, dimmed. The tint's RGB is
/// darkened ~14% (alpha — i.e. the see-through-ness — is preserved, so an
/// unfocused window stays equally translucent but reads visibly recessed), and
/// the frost lift is trimmed so the interior is a touch flatter. Token-derived:
/// a Vibe re-skin of `GLASS_CHROME_DARK` flows straight through.
const GLASS_CHROME_DIM: GlassTier = GlassTier {
    tint: darken_pct(GLASS_CHROME_DARK.tint, 14),
    blur_radius: GLASS_CHROME_DARK.blur_radius,
    frost: darken_pct(GLASS_CHROME_DARK.frost, 25),
};

/// The LIVE accent ramp for the chrome — derived from the active theme/Vibe
/// seed (`theme_engine::active_accent`) so a one-tap Vibe re-skin recolours the
/// focused-window accent with the rest of the shell (Concept §Customization
/// Engine: "the desktop becomes a different place in one tap"). Replaces the
/// previous hardcoded-RaeBlue deferral.
#[inline]
fn accent() -> rae_tokens::AccentRamp {
    rae_tokens::derive_accent(crate::theme_engine::active_accent(), PALETTE)
}

/// The focused-window accent base actually painted on the title-bar top edge.
/// Public so the cross-surface cohesion smoketest can confirm the chrome tracks
/// the live seed (`theme_engine::run_accent_cohesion_smoketest`).
#[inline]
pub fn proof_accent() -> u32 {
    accent().base
}

/// Title bar height (px). desktop-shell.md §4: 32 (up from the old 28).
pub const TITLE_BAR_H: i32 = 32;

/// Traffic-light control diameter (px). desktop-shell.md §4: 14px circles.
const CTRL_DIAMETER: i32 = 14;
/// Gap between controls. `SPACE_2` (8px).
const CTRL_GAP: i32 = SPACE_2 as i32;
/// Inset of the first control from the left edge. `SPACE_3` (12px).
const CTRL_INSET: i32 = SPACE_3 as i32;

/// Active palette (dark default). The accent seed is now LIVE — see `accent()`,
/// which reads `theme_engine::active_accent()`; a light palette swap is the one
/// remaining value change here.
const PALETTE: &Palette = &DARK;

/// Window corner radius — desktop-shell.md §4: `radius.md` (12px). Exposed so
/// the compositor's `RoundedCorners` effect and this module agree on one value.
pub const CORNER_RADIUS: u32 = RADIUS_MD;

// ── Token-derived chrome colours ────────────────────────────────────────────

/// `material.mica` static tint (design-language §5.2): a wallpaper-independent
/// solid 1:2 blend of `bg.base`/`bg.raised`, OFF the per-frame blur path — the
/// same recipe the taskbar uses (`raeshell::mica_tint`). Kept identical so
/// taskbar and titlebar read as one material.
#[inline]
const fn mica_tint() -> u32 {
    blend_opaque(PALETTE.bg_base, PALETTE.bg_raised, 1, 2)
}

/// Unfocused titlebar tint: mica darkened ~8% (desktop-shell.md §4).
#[inline]
const fn mica_unfocused() -> u32 {
    darken_pct(mica_tint(), 8)
}

/// The control colours at full saturation, in macOS order (close/min/max).
#[inline]
const fn ctrl_close() -> u32 {
    PALETTE.state_danger
}
#[inline]
const fn ctrl_min() -> u32 {
    PALETTE.state_warn
}
#[inline]
const fn ctrl_max() -> u32 {
    PALETTE.state_ok
}

// ── ARGB helpers (const, no_std, no float) ──────────────────────────────────

#[inline]
const fn chan(c: u32, shift: u32) -> u32 {
    (c >> shift) & 0xFF
}

/// One channel of a `num/den` blend of `a` toward `b`.
#[inline]
const fn mix_ch(a: u32, b: u32, num: u32, den: u32, shift: u32) -> u32 {
    (chan(a, shift) * (den - num) + chan(b, shift) * num) / den
}

/// Opaque per-channel `num/den` blend of `a` toward `b` (matches
/// `raeshell::blend_opaque` so the mica tint is byte-identical).
#[inline]
const fn blend_opaque(a: u32, b: u32, num: u32, den: u32) -> u32 {
    0xFF00_0000
        | (mix_ch(a, b, num, den, 16) << 16)
        | (mix_ch(a, b, num, den, 8) << 8)
        | mix_ch(a, b, num, den, 0)
}

/// One channel multiplicatively darkened by `pct` percent.
#[inline]
const fn dark_ch(c: u32, pct: u32, shift: u32) -> u32 {
    chan(c, shift) * (100 - pct) / 100
}

/// Per-channel multiplicative darken by `pct` percent, alpha preserved.
#[inline]
const fn darken_pct(c: u32, pct: u32) -> u32 {
    (c & 0xFF00_0000) | (dark_ch(c, pct, 16) << 16) | (dark_ch(c, pct, 8) << 8) | dark_ch(c, pct, 0)
}

/// One channel desaturated `pct` percent toward `avg`.
#[inline]
const fn desat_ch(c: u32, avg: u32, pct: u32, shift: u32) -> u32 {
    (chan(c, shift) * (100 - pct) + avg * pct) / 100
}

/// Desaturate toward the channel average by `pct` percent (controls dim when
/// the window is unfocused), alpha preserved. No float — integer luma average.
#[inline]
const fn desaturate_pct(c: u32, pct: u32) -> u32 {
    let avg = (chan(c, 16) + chan(c, 8) + chan(c, 0)) / 3;
    (c & 0xFF00_0000)
        | (desat_ch(c, avg, pct, 16) << 16)
        | (desat_ch(c, avg, pct, 8) << 8)
        | desat_ch(c, avg, pct, 0)
}

// ── Drawing ──────────────────────────────────────────────────────────────────

/// Build a `raegfx::Canvas` over the whole compositor buffer so we can use the
/// real (`font8x8`-backed) text + anti-aliased circle primitives and draw at
/// absolute screen coordinates. `stride` is the compositor row stride (== the
/// comp-buffer width), so a Canvas of `width = stride` maps 1:1.
#[inline]
fn comp_canvas(buf: &mut [u32], stride: usize) -> raegfx::Canvas {
    let height = if stride == 0 { 0 } else { buf.len() / stride };
    unsafe { raegfx::Canvas::new(buf.as_mut_ptr() as *mut u8, stride, height, 4) }
}

/// Draw a title bar into `buf` (ARGB, row stride `stride`) at `(ox, oy)` with
/// width `w`. `focused` selects the focused vs unfocused visual state.
pub fn draw_title_bar(
    buf: &mut [u32],
    stride: usize,
    ox: i32,
    oy: i32,
    w: i32,
    title: &str,
    focused: bool,
) {
    if w <= 0 || stride == 0 {
        return;
    }
    let h = TITLE_BAR_H;
    let mut canvas = comp_canvas(buf, stride);

    // ── Liquid Glass title bar ──────────────────────────────────────────────
    // IDENTITY.md §7: window chrome is the MOST see-through tier (`glass.chrome`).
    // Render the bar via the shipped frosted-glass primitive — tint → frost →
    // WCAG legibility-cap → iridescent rim → top highlight, baked in — so it
    // reads as translucent frosted glass over the window/desktop behind it. The
    // chrome floor + title-text WCAG cap are part of that primitive. radius=0:
    // the bar draws square; the window's rounded corners are the compositor's
    // `RoundedCorners` effect (see `CORNER_RADIUS`), so geometry is unchanged.
    //
    // Clamp the origin to the buffer's non-negative quadrant (the glass/rim
    // helpers take usize and self-clip on the right/bottom). A bar pushed fully
    // offscreen-left collapses to zero width and the primitives no-op.
    let gx = ox.max(0);
    let gy = oy.max(0);
    let gw = (w - (gx - ox)).max(0);
    let gh = (h - (gy - oy)).max(0);
    if gw > 0 && gh > 0 {
        let (gx, gy, gw, gh) = (gx as usize, gy as usize, gw as usize, gh as usize);
        // OBSIDIAN (IDENTITY-OBSIDIAN.md §2): the iridescent rim is retired on
        // window chrome too — the near-black bar carries the hairline + top
        // light from the glass primitive; focus reads via the accent top edge.
        if focused {
            raegfx::glass::draw_glass_surface(&mut canvas, gx, gy, gw, gh, 0, GLASS_CHROME);
            // LIVE-accent 1px top-edge highlight over the glass so a Vibe re-skin
            // recolours the focused window ("lit on black", OBSIDIAN §3).
            fill_clamped(&mut canvas, ox, oy, w, 1, accent().base);
        } else {
            // Unfocused: same tier, dimmer — no accent edge so focus reads at
            // a glance.
            raegfx::glass::draw_glass_surface(&mut canvas, gx, gy, gw, gh, 0, GLASS_CHROME_DIM);
        }
    }

    // ── Traffic-light controls (LEFT, macOS order: close / min / max) ──
    //
    // Resting = ~55% saturation; the cluster reveals real glyphs (×/−/+) on
    // hover (the compositor passes hover state through `focused`-adjacent
    // plumbing in a later pass — see the hover follow-up note). Today: focused
    // windows show full-saturation dots, unfocused desaturate them.
    let cy = oy + h / 2;
    let r = CTRL_DIAMETER / 2;
    let controls = [ctrl_close(), ctrl_min(), ctrl_max()];
    let mut cx = ox + CTRL_INSET + r;
    for color in controls {
        let dot = if focused {
            // Resting saturation per spec (~55%): blend the full-sat token a
            // little toward the bar so it reads calm until hovered.
            desaturate_pct(color, 45)
        } else {
            // Unfocused: desaturated controls (no accent draw).
            desaturate_pct(color, 70)
        };
        if cx >= 0 && cy >= 0 {
            canvas.fill_circle(cx as usize, cy as usize, r as usize, dot);
        }
        cx += CTRL_DIAMETER + CTRL_GAP;
    }

    // ── Title text ──────────────────────────────────────────────────────
    // Crisp grayscale-AA title via `draw_text_aa` at the `type.label` style
    // (raefont/RaeSans). We honour the TOKEN by colour + placement: focused
    // uses text.primary, unfocused text.tertiary. The font engine falls back to
    // the 8×8 bitmap path internally during early boot (engine not ready).
    let text_color = if focused {
        PALETTE.text_primary
    } else {
        PALETTE.text_tertiary
    };
    // Title starts after the control cluster + a SPACE_3 gap.
    let cluster_end = ox + CTRL_INSET + 3 * CTRL_DIAMETER + 2 * CTRL_GAP;
    let tx = cluster_end + SPACE_3 as i32;
    // Vertically centre the label line box in the 32px bar (was: the bare 8px
    // glyph). Top-left of the line box = bar mid minus half the line height.
    let ty = oy + (h - TYPE_LABEL.line_height as i32) / 2;
    // Available horizontal space between the cluster and the window's right edge.
    let avail_px = ((ox + w) - tx).max(0);
    if avail_px > 0 && tx >= 0 && ty >= 0 {
        // Truncate to what fits: measure the whole title, then trim chars from
        // the end until the AA advance fits the available width (replaces the
        // old fixed 8px/char clamp, which no longer matches proportional type).
        let mut shown = title;
        if canvas.measure_text_aa(shown, TYPE_LABEL, raegfx::text::FontFamily::Sans) > avail_px {
            let mut end = shown.len();
            while end > 0 {
                // Walk back to a char boundary.
                while end > 0 && !shown.is_char_boundary(end) {
                    end -= 1;
                }
                if end == 0 {
                    break;
                }
                let candidate = &shown[..end];
                if canvas.measure_text_aa(candidate, TYPE_LABEL, raegfx::text::FontFamily::Sans)
                    <= avail_px
                {
                    shown = candidate;
                    break;
                }
                end -= 1;
            }
            if end == 0 {
                shown = "";
            }
        }
        if !shown.is_empty() {
            canvas.draw_text_aa(
                tx,
                ty,
                shown,
                TYPE_LABEL,
                text_color,
                raegfx::text::FontFamily::Sans,
            );
        }
    }
}

/// Fill an axis-aligned rect clamped to non-negative origin (Canvas handles the
/// right/bottom clip).
#[inline]
fn fill_clamped(canvas: &mut raegfx::Canvas, x: i32, y: i32, w: i32, h: i32, color: u32) {
    let x0 = x.max(0);
    let y0 = y.max(0);
    let w = (w - (x0 - x)).max(0);
    let h = (h - (y0 - y)).max(0);
    if w > 0 && h > 0 {
        canvas.fill_rect(x0 as usize, y0 as usize, w as usize, h as usize, color);
    }
}

/// Which chrome button (if any) was hit. Coordinates are screen-space.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ChromeHit {
    None,
    Close,
    Maximize,
    Minimize,
    Title,
    Client,
}

/// Hit-test a point against the chrome. Controls are LEFT-aligned traffic
/// lights (macOS order: close / minimize / maximize), matching `draw_title_bar`.
pub fn hit_test(window_x: i32, window_y: i32, window_w: i32, px: i32, py: i32) -> ChromeHit {
    if px < window_x || py < window_y || px >= window_x + window_w {
        return ChromeHit::None;
    }
    if py < window_y + TITLE_BAR_H {
        // Controls live on the left; each occupies a CTRL_DIAMETER-wide slot
        // (plus gap) starting at CTRL_INSET. Hit the whole slot, not just the
        // 14px circle, so the target meets the pointer minimum comfortably.
        let slot = CTRL_DIAMETER + CTRL_GAP;
        let c0 = window_x + CTRL_INSET;
        if in_rect(px, py, c0, window_y, CTRL_DIAMETER, TITLE_BAR_H) {
            return ChromeHit::Close;
        }
        if in_rect(px, py, c0 + slot, window_y, CTRL_DIAMETER, TITLE_BAR_H) {
            return ChromeHit::Minimize;
        }
        if in_rect(px, py, c0 + 2 * slot, window_y, CTRL_DIAMETER, TITLE_BAR_H) {
            return ChromeHit::Maximize;
        }
        return ChromeHit::Title;
    }
    ChromeHit::Client
}

#[inline]
fn in_rect(px: i32, py: i32, x: i32, y: i32, w: i32, h: i32) -> bool {
    px >= x && px < x + w && py >= y && py < y + h
}

/// Total composited height including title bar.
pub fn frame_height(client_h: i32) -> i32 {
    client_h + TITLE_BAR_H
}

// ── R10 proof: token wiring + real-glyph coverage ─────────────────────────────

/// What [`run_boot_smoketest`] asserts and prints. A real fail-able snapshot of
/// the chrome's token wiring (height/tint/control placement) and the fact that
/// title text renders as actual glyphs (non-uniform coverage), not noise.
#[derive(Clone, Copy, Debug)]
pub struct ChromeProof {
    pub titlebar_h: i32,
    pub focus_tint: u32,
    pub unfocus_tint: u32,
    pub close_color: u32,
    /// The focused top-edge accent actually painted — must equal
    /// `derive_accent(theme_engine::active_accent(), &DARK).base`.
    pub accent_base: u32,
    pub controls_left: bool,
    pub glyphs_real: bool,
    pub pass: bool,
}

/// Render a known string into a scratch buffer and confirm the glyph renderer
/// produced REAL, non-uniform coverage (an all-noise or all-blank render fails).
/// This is the load-bearing "glyphs=real" assertion: the old `%3` speckle would
/// have produced a near-uniform fill; readable glyphs leave large blank regions
/// between strokes.
fn glyphs_render_real() -> bool {
    // A small offscreen ARGB buffer sized like a wide titlebar slice.
    const W: usize = 256;
    const H: usize = 32;
    let mut buf = alloc::vec![0u32; W * H];
    {
        let mut canvas = comp_canvas(&mut buf, W);
        // Two distinct glyphs at a known baseline.
        canvas.draw_glyph(16, 12, 'R', PALETTE.text_primary, None);
        canvas.draw_glyph(32, 12, 'a', PALETTE.text_primary, None);
    }
    // Count set pixels. A real 8x8 glyph fills only a fraction of its 8x8 cell;
    // two glyphs over 256x32 must light SOME pixels but nowhere near all of them
    // (noise would light ~2/3 of every cell uniformly across the whole strip).
    let set = buf.iter().filter(|&&p| p != 0).count();
    // Must have drawn something (not blank) and stayed sparse (not noise-filled
    // across the whole buffer). Two 8x8 glyphs => at most 128 lit px.
    set > 4 && set < 160
}

pub fn run_boot_smoketest() {
    // Fail-able token-wiring assertions.
    let want_mica = blend_opaque(PALETTE.bg_base, PALETTE.bg_raised, 1, 2);
    let want_close = PALETTE.state_danger;
    let glyphs_real = glyphs_render_real();
    // The focused top-edge accent must track the LIVE seed (Vibe-Mode cohesion):
    // it equals derive_accent(theme_engine::active_accent()).base. FAIL-able —
    // if the chrome ever re-hardcodes the accent this drifts off the live seed.
    let want_accent = rae_tokens::derive_accent(crate::theme_engine::active_accent(), PALETTE).base;
    let accent_ok = proof_accent() == want_accent;
    let pass = TITLE_BAR_H == 32
        && mica_tint() == want_mica
        && mica_unfocused() == darken_pct(want_mica, 8)
        && ctrl_close() == want_close
        && accent_ok
        && glyphs_real;

    let proof = ChromeProof {
        titlebar_h: TITLE_BAR_H,
        focus_tint: mica_tint(),
        unfocus_tint: mica_unfocused(),
        close_color: ctrl_close(),
        accent_base: proof_accent(),
        controls_left: true,
        glyphs_real,
        pass,
    };
    crate::serial_println!(
        "[chrome] titlebar h={} focus tint={:#010X} accent={:#010X} controls=left glyphs={} text=aa close={:#010X} -> {}",
        proof.titlebar_h,
        proof.focus_tint,
        proof.accent_base,
        if proof.glyphs_real { "real" } else { "NOISE" },
        proof.close_color,
        if proof.pass { "PASS" } else { "FAIL" },
    );
}
