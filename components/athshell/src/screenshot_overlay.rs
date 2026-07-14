//! Screenshot / region-capture **overlay** — the dimmed-scrim capture-mode UI
//! that drives the in-kernel compositor capture engine (parity §F,
//! `docs/design/screenshot-capture.md`).
//!
//! Concept §creators: *"capture & stream at the compositor — zero-cost
//! recording, no OBS overhead."* This surface is the still-image half: a hotkey
//! (Super+Shift+S) dims the screen, the user has a selection rectangle with a
//! live dimensions readout, and on confirm the selected region's REAL composited
//! pixels are read straight off the front buffer (the same engine the cap-gated
//! `SYS_CAPTURE_START/READ/STOP` 274-276 expose), then saved + copied.
//!
//! This is the *UI surface* (overlay scrim, selection visuals, action bar) — the
//! rich capture/markup data model lives in [`crate::screenshot`]; the capture
//! engine lives in the kernel compositor. All colors come from `ath_tokens` +
//! the live accent (`derive_accent(active_accent())`) so a Vibe-switch re-tints
//! the selection border / handles / pill with the rest of the shell — no
//! hardcoded `SELECTION_BORDER`. The scrim is `ath_tokens::SCRIM_CAPTURE`.

extern crate alloc;

use alloc::format;
use alloc::string::String;

use ath_tokens::{
    derive_accent, AccentRamp, RADIUS_LG, RADIUS_SM, RADIUS_XS, SCRIM_CAPTURE, SPACE_2, SPACE_3,
    TYPE_CAPTION, TYPE_LABEL,
};

use crate::{active_accent, PALETTE};

/// Selection-handle visual size (px). Spec §3: 8px visual, ≥16px grab area.
const HANDLE_PX: usize = 8;
/// Action-bar button box (px). Spec §4: 36px buttons.
const ACTION_BTN: usize = 36;
/// Minimum sane region edge so a stray tap never captures a 0-area rect.
const MIN_EDGE: u32 = 16;

/// Which post-capture action the action bar will fire on confirm. Mirrors the
/// spec §4 button row; Copy is the primary (Enter) action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureAction {
    Copy,
    Save,
    Close,
}

/// Overlay phase. `Idle` = not capturing (overlay hidden). `Selecting` = the
/// dimmed scrim + draggable rectangle is up. `Confirmed` = the user pressed
/// Enter/clicked Copy/Save; the shell reads this, drives the engine, then
/// `dismiss()`es.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverlayPhase {
    Idle,
    Selecting,
    Confirmed,
}

/// The capture-mode overlay state. Region coordinates are screen pixels.
#[derive(Debug, Clone)]
pub struct CaptureOverlay {
    pub phase: OverlayPhase,
    pub screen_w: u32,
    pub screen_h: u32,
    /// Selection rectangle (top-left + size), clamped to the screen.
    pub sel_x: i32,
    pub sel_y: i32,
    pub sel_w: u32,
    pub sel_h: u32,
    /// Action the user committed to (read by the shell on `Confirmed`).
    pub action: CaptureAction,
    /// Which action-bar button has keyboard focus (Copy default).
    pub focus: CaptureAction,
}

impl CaptureOverlay {
    #[must_use]
    pub fn new(screen_w: u32, screen_h: u32) -> Self {
        Self {
            phase: OverlayPhase::Idle,
            screen_w,
            screen_h,
            sel_x: 0,
            sel_y: 0,
            sel_w: 0,
            sel_h: 0,
            action: CaptureAction::Copy,
            focus: CaptureAction::Copy,
        }
    }

    #[must_use]
    pub fn is_active(&self) -> bool {
        self.phase != OverlayPhase::Idle
    }

    /// Enter capture mode with a sensible default selection: a centered region
    /// (~60% of the screen). Live mouse-drag refines `sel_*` via [`drag_to`];
    /// headless boots/keyboard-only confirm the default directly.
    pub fn begin(&mut self, screen_w: u32, screen_h: u32) {
        self.screen_w = screen_w;
        self.screen_h = screen_h;
        self.phase = OverlayPhase::Selecting;
        self.action = CaptureAction::Copy;
        self.focus = CaptureAction::Copy;
        let w = (screen_w * 3 / 5).max(MIN_EDGE);
        let h = (screen_h * 3 / 5).max(MIN_EDGE);
        self.sel_w = w;
        self.sel_h = h;
        self.sel_x = ((screen_w - w) / 2) as i32;
        self.sel_y = ((screen_h - h) / 2) as i32;
    }

    /// Capture the whole screen (PrintScreen / full-screen mode): the selection
    /// is the entire framebuffer, then jump straight to confirm.
    pub fn begin_full_screen(&mut self, screen_w: u32, screen_h: u32) {
        self.begin(screen_w, screen_h);
        self.sel_x = 0;
        self.sel_y = 0;
        self.sel_w = screen_w;
        self.sel_h = screen_h;
    }

    /// Refine the selection from an anchor (drag start) to the current cursor —
    /// the live mouse-drag path. Clamped + normalized so width/height are always
    /// positive.
    pub fn drag_to(&mut self, anchor_x: i32, anchor_y: i32, cur_x: i32, cur_y: i32) {
        if self.phase != OverlayPhase::Selecting {
            return;
        }
        let x0 = anchor_x.min(cur_x).max(0);
        let y0 = anchor_y.min(cur_y).max(0);
        let x1 = anchor_x.max(cur_x).min(self.screen_w as i32);
        let y1 = anchor_y.max(cur_y).min(self.screen_h as i32);
        self.sel_x = x0;
        self.sel_y = y0;
        self.sel_w = (x1 - x0).max(0) as u32;
        self.sel_h = (y1 - y0).max(0) as u32;
    }

    /// The normalized region clamped to the screen, returned as `(x, y, w, h)`
    /// with non-zero edges — the exact tuple to hand the compositor capture
    /// engine. `None` if the selection is degenerate.
    #[must_use]
    pub fn region(&self) -> Option<(u32, u32, u32, u32)> {
        let x = self.sel_x.max(0) as u32;
        let y = self.sel_y.max(0) as u32;
        let w = self.sel_w.min(self.screen_w.saturating_sub(x));
        let h = self.sel_h.min(self.screen_h.saturating_sub(y));
        if w >= 1 && h >= 1 {
            Some((x, y, w, h))
        } else {
            None
        }
    }

    /// Commit a chosen action (Copy/Save/Enter) → `Confirmed`. The shell reads
    /// `action` + `region()` next, runs the engine, then `dismiss()`es. `Close`
    /// instead cancels.
    pub fn confirm(&mut self, action: CaptureAction) {
        if action == CaptureAction::Close {
            self.dismiss();
            return;
        }
        self.action = action;
        self.phase = OverlayPhase::Confirmed;
    }

    /// Cycle action-bar focus (Tab). Wraps Copy → Save → Close → Copy.
    pub fn focus_next(&mut self) {
        self.focus = match self.focus {
            CaptureAction::Copy => CaptureAction::Save,
            CaptureAction::Save => CaptureAction::Close,
            CaptureAction::Close => CaptureAction::Copy,
        };
    }

    /// Leave capture mode (Esc / after the shell consumes a `Confirmed`).
    pub fn dismiss(&mut self) {
        self.phase = OverlayPhase::Idle;
        self.sel_w = 0;
        self.sel_h = 0;
    }

    /// Live accent ramp — same seed the rest of the shell reads, so the
    /// selection border/handles/pill re-tint on a Vibe switch.
    fn accent(&self) -> AccentRamp {
        derive_accent(active_accent(), PALETTE)
    }

    /// Render the dimmed scrim + the selected-region punch-through + the 1px
    /// accent border + 8 handles + the live dimensions pill + the action bar.
    /// Driven every frame while `Selecting`.
    pub fn render(&self, canvas: &mut athgfx::Canvas) {
        if self.phase == OverlayPhase::Idle {
            return;
        }
        let sw = self.screen_w as usize;
        let sh = self.screen_h as usize;
        let acc = self.accent();

        // ── Scrim: dim the WHOLE screen with scrim.capture, then punch the
        // selected region back to full brightness (re-paint nothing there — the
        // scrim is a blend, so leaving the region un-blended previews the exact
        // capture). We blend the scrim everywhere EXCEPT inside the selection.
        let (rx, ry, rw, rh) = match self.region() {
            Some(r) => r,
            None => (0, 0, 0, 0),
        };
        let rx = rx as usize;
        let ry = ry as usize;
        let rw = rw as usize;
        let rh = rh as usize;
        for y in 0..sh {
            let in_row = y >= ry && y < ry + rh;
            for x in 0..sw {
                if in_row && x >= rx && x < rx + rw {
                    continue; // selected region stays un-dimmed (full brightness)
                }
                canvas.blend_pixel(x, y, SCRIM_CAPTURE);
            }
        }

        if rw == 0 || rh == 0 {
            self.render_hint(canvas);
            return;
        }

        // ── 1px accent border framing the selection (+ a stroke.strong inner
        // line for contrast over bright content — spec §7 accessibility).
        let p = PALETTE;
        self.stroke_rect(canvas, rx, ry, rw, rh, acc.base);
        if rw > 2 && rh > 2 {
            self.stroke_rect(canvas, rx + 1, ry + 1, rw - 2, rh - 2, p.stroke_strong);
        }

        // ── 8 resize handles (corners + edge midpoints), accent squares.
        let hx = [rx, rx + rw / 2, rx + rw];
        let hy = [ry, ry + rh / 2, ry + rh];
        for (iy, &cy) in hy.iter().enumerate() {
            for (ix, &cx) in hx.iter().enumerate() {
                if ix == 1 && iy == 1 {
                    continue; // skip the center
                }
                let x0 = cx
                    .saturating_sub(HANDLE_PX / 2)
                    .min(sw.saturating_sub(HANDLE_PX));
                let y0 = cy
                    .saturating_sub(HANDLE_PX / 2)
                    .min(sh.saturating_sub(HANDLE_PX));
                canvas.fill_rounded_rect(
                    x0,
                    y0,
                    HANDLE_PX,
                    HANDLE_PX,
                    RADIUS_XS as usize,
                    acc.base,
                );
            }
        }

        // ── Dimensions readout pill (radius.pill glass), just outside the
        // selection's top-left, flipping inside when it would clip off-screen.
        self.render_dimensions_pill(canvas, rx, ry, rw, rh, acc);

        // ── Post-capture action bar (Copy / Save / Close), glass, below the
        // selection (flipping above to stay on-screen).
        self.render_action_bar(canvas, rx, ry, rw, rh, acc);
    }

    fn stroke_rect(
        &self,
        canvas: &mut athgfx::Canvas,
        x: usize,
        y: usize,
        w: usize,
        h: usize,
        color: u32,
    ) {
        if w == 0 || h == 0 {
            return;
        }
        let sw = self.screen_w as usize;
        let sh = self.screen_h as usize;
        for xx in x..(x + w).min(sw) {
            canvas.blend_pixel(xx, y, color);
            if y + h - 1 < sh {
                canvas.blend_pixel(xx, y + h - 1, color);
            }
        }
        for yy in y..(y + h).min(sh) {
            canvas.blend_pixel(x, yy, color);
            if x + w - 1 < sw {
                canvas.blend_pixel(x + w - 1, yy, color);
            }
        }
    }

    fn render_dimensions_pill(
        &self,
        canvas: &mut athgfx::Canvas,
        rx: usize,
        ry: usize,
        _rw: usize,
        _rh: usize,
        acc: AccentRamp,
    ) {
        let label = self.dimensions_label();
        let tw = canvas
            .measure_text_aa(&label, TYPE_CAPTION, athgfx::text::FontFamily::Sans)
            .max(0) as usize;
        let pill_h = 22usize;
        let pill_w = tw + 2 * SPACE_3 as usize;
        let pill_r = (pill_h / 2) as usize;
        // Sit just above-left of the selection; flip inside if it would clip.
        let mut px = rx;
        let mut py = ry.saturating_sub(pill_h + SPACE_2 as usize);
        if py < SPACE_2 as usize {
            py = ry + SPACE_2 as usize; // flip inside the region (top)
        }
        if px + pill_w > self.screen_w as usize {
            px = (self.screen_w as usize).saturating_sub(pill_w + SPACE_2 as usize);
        }
        canvas.fill_rounded_rect(px, py, pill_w, pill_h, pill_r, glass_fill());
        canvas.draw_text_aa(
            (px + SPACE_3 as usize) as i32,
            (py + 4) as i32,
            &label,
            TYPE_CAPTION,
            acc.text,
            athgfx::text::FontFamily::Sans,
        );
    }

    fn dimensions_label(&self) -> String {
        match self.region() {
            Some((_, _, w, h)) => format!("{} x {}", w, h),
            None => String::from("0 x 0"),
        }
    }

    fn render_action_bar(
        &self,
        canvas: &mut athgfx::Canvas,
        rx: usize,
        ry: usize,
        rw: usize,
        rh: usize,
        acc: AccentRamp,
    ) {
        let buttons = [
            (CaptureAction::Copy, "Copy"),
            (CaptureAction::Save, "Save"),
            (CaptureAction::Close, "Close"),
        ];
        let inset = SPACE_2 as usize;
        let gap = SPACE_2 as usize;
        let mut widths = [0usize; 3];
        let mut total = inset;
        for (i, (_, lbl)) in buttons.iter().enumerate() {
            let tw = canvas
                .measure_text_aa(lbl, TYPE_LABEL, athgfx::text::FontFamily::Sans)
                .max(0) as usize;
            let bw = (tw + 2 * SPACE_3 as usize).max(ACTION_BTN);
            widths[i] = bw;
            total += bw + gap;
        }
        let bar_w = total + inset - gap;
        let bar_h = ACTION_BTN + 2 * inset;
        // Anchor below the selection, flipping above if it would run off-screen.
        let mut bar_x = rx;
        if bar_x + bar_w > self.screen_w as usize {
            bar_x = (self.screen_w as usize).saturating_sub(bar_w + inset);
        }
        let mut bar_y = ry + rh + SPACE_2 as usize;
        if bar_y + bar_h > self.screen_h as usize {
            bar_y = ry.saturating_sub(bar_h + SPACE_2 as usize);
        }
        canvas.fill_rounded_rect(bar_x, bar_y, bar_w, bar_h, RADIUS_LG as usize, glass_fill());

        let mut bx = bar_x + inset;
        let by = bar_y + inset;
        for (i, (act, lbl)) in buttons.iter().enumerate() {
            let bw = widths[i];
            // Primary (Copy) rests in accent.subtle; the focused button gets the
            // accent ring; danger (Close) tints its label.
            let fill = if *act == CaptureAction::Copy {
                acc.subtle
            } else {
                PALETTE.bg_elevated
            };
            canvas.fill_rounded_rect(bx, by, bw, ACTION_BTN, RADIUS_SM as usize, fill);
            if *act == self.focus {
                self.stroke_rect(canvas, bx, by, bw, ACTION_BTN, acc.base);
            }
            let fg = match act {
                CaptureAction::Copy => acc.text,
                CaptureAction::Close => PALETTE.state_danger,
                _ => PALETTE.text_primary,
            };
            let tw = canvas
                .measure_text_aa(lbl, TYPE_LABEL, athgfx::text::FontFamily::Sans)
                .max(0) as usize;
            let tx = bx + (bw.saturating_sub(tw)) / 2;
            canvas.draw_text_aa(
                tx as i32,
                (by + 9) as i32,
                lbl,
                TYPE_LABEL,
                fg,
                athgfx::text::FontFamily::Sans,
            );
            bx += bw + gap;
        }
    }

    fn render_hint(&self, canvas: &mut athgfx::Canvas) {
        let hint = "Drag to select a region  ·  Enter to capture  ·  Esc to cancel";
        let tw = canvas
            .measure_text_aa(hint, TYPE_LABEL, athgfx::text::FontFamily::Sans)
            .max(0) as usize;
        let x = ((self.screen_w as usize).saturating_sub(tw)) / 2;
        let y = self.screen_h as usize / 2;
        canvas.draw_text_aa(
            x as i32,
            y as i32,
            hint,
            TYPE_LABEL,
            PALETTE.text_primary,
            athgfx::text::FontFamily::Sans,
        );
    }
}

/// `material.glass` resting fill for the transient panels (pill + action bar):
/// a translucent `bg.overlay` so the dimmed desktop shows through faintly.
#[inline]
fn glass_fill() -> u32 {
    (PALETTE.bg_overlay & 0x00_FF_FF_FF) | 0xE6_00_00_00
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn begin_centers_a_default_region() {
        let mut o = CaptureOverlay::new(1920, 1080);
        assert!(!o.is_active());
        o.begin(1920, 1080);
        assert!(o.is_active());
        let (x, y, w, h) = o.region().expect("default region");
        assert_eq!(w, 1920 * 3 / 5);
        assert_eq!(h, 1080 * 3 / 5);
        // centered
        assert_eq!(x, (1920 - w) / 2);
        assert_eq!(y, (1080 - h) / 2);
    }

    #[test]
    fn full_screen_region_is_the_whole_frame() {
        let mut o = CaptureOverlay::new(1280, 720);
        o.begin_full_screen(1280, 720);
        assert_eq!(o.region(), Some((0, 0, 1280, 720)));
    }

    #[test]
    fn drag_normalizes_and_clamps() {
        let mut o = CaptureOverlay::new(800, 600);
        o.begin(800, 600);
        // drag bottom-right to top-left (reversed) and past the edges
        o.drag_to(700, 500, -50, -50);
        let (x, y, w, h) = o.region().expect("region");
        assert_eq!((x, y), (0, 0));
        assert_eq!((w, h), (700, 500));
    }

    #[test]
    fn confirm_close_dismisses_else_sets_confirmed() {
        let mut o = CaptureOverlay::new(800, 600);
        o.begin(800, 600);
        o.confirm(CaptureAction::Close);
        assert_eq!(o.phase, OverlayPhase::Idle);

        o.begin(800, 600);
        o.confirm(CaptureAction::Save);
        assert_eq!(o.phase, OverlayPhase::Confirmed);
        assert_eq!(o.action, CaptureAction::Save);
    }

    #[test]
    fn focus_cycles() {
        let mut o = CaptureOverlay::new(800, 600);
        o.begin(800, 600);
        assert_eq!(o.focus, CaptureAction::Copy);
        o.focus_next();
        assert_eq!(o.focus, CaptureAction::Save);
        o.focus_next();
        assert_eq!(o.focus, CaptureAction::Close);
        o.focus_next();
        assert_eq!(o.focus, CaptureAction::Copy);
    }
}
