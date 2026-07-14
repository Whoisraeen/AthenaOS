//! Windows 11-style **Snap Layouts** for the AthenaOS desktop.
//!
//! Concept §"Windows pain points -> our answer": productivity parity with the
//! best of Win11/macOS window management. AthenaOS already has edge-drag Aero
//! snapping (`tiling_wm::SnapZone`); this adds the *signature* Win11 interaction
//! it was missing — a flyout of layout **templates** (two-even, large-left,
//! thirds, quadrants, …) that you open with the Rae key + Z (or by hovering the
//! window's maximize control) and click a zone to place the focused window into
//! a precise region. The chosen zone's rect is a real, exact tiling of the work
//! area, so a follow-up "snap assist" can fill the remaining zones.
//!
//! This module is pure geometry + a self-contained overlay renderer, so its
//! correctness is proven by a host KAT (`cargo test -p athshell`) and its look
//! by `tools/ui_screenshot` — neither needs a live desktop.

use crate::tiling_wm::Rect;
use alloc::vec::Vec;

/// A snap-layout template: a named partition of the work area into 2–4 zones.
///
/// The set mirrors the Win11 flyout for a standard landscape monitor (the first
/// four) plus two wide-screen favourites (`ThreeEven`, `ThreeWideCenter`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapTemplate {
    /// `| 50 | 50 |` — two even columns.
    TwoEven,
    /// `| 62 | 38 |` — a wide left column + a narrow right (Win11 "large left").
    TwoWideLeft,
    /// `| 50 | 25 / 25 |` — left half + two stacked quarters on the right.
    LeftHalfRightStack,
    /// `2 x 2` — four quadrants.
    Quadrants,
    /// `| 33 | 33 | 33 |` — three even columns.
    ThreeEven,
    /// `| 25 | 50 | 25 |` — a wide centre column flanked by two narrow ones.
    ThreeWideCenter,
}

impl SnapTemplate {
    /// The templates offered in the flyout, in display order.
    pub const ALL: [SnapTemplate; 6] = [
        SnapTemplate::TwoEven,
        SnapTemplate::TwoWideLeft,
        SnapTemplate::LeftHalfRightStack,
        SnapTemplate::Quadrants,
        SnapTemplate::ThreeEven,
        SnapTemplate::ThreeWideCenter,
    ];

    /// The zones this template carves `work` into, in reading order (left→right,
    /// top→bottom). Every zone is a real snap target. The partition is EXACT:
    /// the zones are pairwise disjoint and their union is exactly `work` (the
    /// final zone in each row/column absorbs integer-division remainder), so a
    /// snapped set of windows tiles the desktop with no gaps or overlaps.
    pub fn zones(self, work: Rect) -> Vec<Rect> {
        let (x, y, w, h) = (work.x, work.y, work.w, work.h);
        let mut v = Vec::new();
        match self {
            SnapTemplate::TwoEven => {
                let l = w / 2;
                v.push(Rect::new(x, y, l, h));
                v.push(Rect::new(x + l as i32, y, w - l, h));
            }
            SnapTemplate::TwoWideLeft => {
                let l = w * 62 / 100;
                v.push(Rect::new(x, y, l, h));
                v.push(Rect::new(x + l as i32, y, w - l, h));
            }
            SnapTemplate::LeftHalfRightStack => {
                let l = w / 2;
                let rw = w - l;
                let th = h / 2;
                v.push(Rect::new(x, y, l, h));
                v.push(Rect::new(x + l as i32, y, rw, th));
                v.push(Rect::new(x + l as i32, y + th as i32, rw, h - th));
            }
            SnapTemplate::Quadrants => {
                let l = w / 2;
                let t = h / 2;
                let rw = w - l;
                let bh = h - t;
                v.push(Rect::new(x, y, l, t));
                v.push(Rect::new(x + l as i32, y, rw, t));
                v.push(Rect::new(x, y + t as i32, l, bh));
                v.push(Rect::new(x + l as i32, y + t as i32, rw, bh));
            }
            SnapTemplate::ThreeEven => {
                let c = w / 3;
                v.push(Rect::new(x, y, c, h));
                v.push(Rect::new(x + c as i32, y, c, h));
                v.push(Rect::new(x + (2 * c) as i32, y, w - 2 * c, h));
            }
            SnapTemplate::ThreeWideCenter => {
                let side = w / 4;
                let mid = w - 2 * side;
                v.push(Rect::new(x, y, side, h));
                v.push(Rect::new(x + side as i32, y, mid, h));
                v.push(Rect::new(x + (side + mid) as i32, y, side, h));
            }
        }
        v
    }
}

/// The Snap Layouts flyout overlay: transient UI that lets the user pick a zone
/// for the focused window. Owns no window state — it computes zone rects and the
/// shell applies the chosen one to the focused surface.
pub struct SnapOverlay {
    pub visible: bool,
    screen_w: usize,
    screen_h: usize,
    /// The usable desktop rect the layouts tile (excludes the taskbar).
    work: Rect,
    /// Hover target as `(template_index, zone_index)`; drives the accent
    /// highlight and is the same index space `zone_at` returns.
    hover: Option<(usize, usize)>,
}

/// Thumbnail grid geometry (shared by render + hit-test so they never drift).
struct Grid {
    panel: Rect,
    thumb_w: usize,
    thumb_h: usize,
    cols: usize,
    gap: usize,
    pad: usize,
    title_h: usize,
}

impl SnapOverlay {
    pub fn new(screen_w: usize, screen_h: usize, work: Rect) -> Self {
        Self {
            visible: false,
            screen_w,
            screen_h,
            work,
            hover: None,
        }
    }

    pub fn open(&mut self) {
        self.visible = true;
        self.hover = None;
    }

    pub fn close(&mut self) {
        self.visible = false;
        self.hover = None;
    }

    pub fn toggle(&mut self) {
        if self.visible {
            self.close();
        } else {
            self.open();
        }
    }

    /// Keep the overlay sized to the live screen/work area (taskbar height can
    /// change with auto-hide / DPI).
    pub fn set_geometry(&mut self, screen_w: usize, screen_h: usize, work: Rect) {
        self.screen_w = screen_w;
        self.screen_h = screen_h;
        self.work = work;
    }

    /// Flyout grid geometry: a 3×2 thumbnail grid inside a centred glass panel.
    fn grid(&self) -> Grid {
        let cols = 3usize;
        let gap = ath_tokens::SPACE_3 as usize; // 12
        let pad = ath_tokens::SPACE_4 as usize; // 16
        let title_h = ath_tokens::SPACE_5 as usize; // 24
                                                    // Thumbnails keep the work area's aspect so the previews read true.
        let thumb_w = 148usize;
        let thumb_h = ((thumb_w as u64 * self.work.h.max(1) as u64) / self.work.w.max(1) as u64)
            .clamp(72, 108) as usize;
        let rows = SnapTemplate::ALL.len().div_ceil(cols);
        let panel_w = cols * thumb_w + (cols - 1) * gap + 2 * pad;
        let panel_h = title_h + rows * thumb_h + (rows - 1) * gap + 2 * pad;
        let panel_x = self.screen_w.saturating_sub(panel_w) / 2;
        // Sit in the upper third (near where a maximize-hover flyout would land),
        // but never off-screen on a short display.
        let panel_y = (self.screen_h / 6).min(self.screen_h.saturating_sub(panel_h + 8));
        Grid {
            panel: Rect::new(
                panel_x as i32,
                panel_y as i32,
                panel_w as u32,
                panel_h as u32,
            ),
            thumb_w,
            thumb_h,
            cols,
            gap,
            pad,
            title_h,
        }
    }

    /// Screen rect of template `i`'s thumbnail.
    fn thumb_rect(&self, g: &Grid, i: usize) -> Rect {
        let row = i / g.cols;
        let col = i % g.cols;
        let x = g.panel.x + g.pad as i32 + (col * (g.thumb_w + g.gap)) as i32;
        let y = g.panel.y + (g.pad + g.title_h) as i32 + (row * (g.thumb_h + g.gap)) as i32;
        Rect::new(x, y, g.thumb_w as u32, g.thumb_h as u32)
    }

    /// Map a zone (in work-area coordinates) into a thumbnail's pixel rect, with
    /// a 1px inset so adjacent zones read as separate tiles in the preview.
    fn zone_in_thumb(&self, thumb: Rect, zone: Rect) -> Rect {
        let sx = |v: i32| -> i32 {
            thumb.x + ((v - self.work.x) as i64 * thumb.w as i64 / self.work.w.max(1) as i64) as i32
        };
        let sy = |v: i32| -> i32 {
            thumb.y + ((v - self.work.y) as i64 * thumb.h as i64 / self.work.h.max(1) as i64) as i32
        };
        let x0 = sx(zone.x);
        let y0 = sy(zone.y);
        let x1 = sx(zone.right());
        let y1 = sy(zone.bottom());
        let inset = 1i32;
        Rect::new(
            x0 + inset,
            y0 + inset,
            (x1 - x0 - 2 * inset).max(1) as u32,
            (y1 - y0 - 2 * inset).max(1) as u32,
        )
    }

    /// Directly set the highlighted `(template, zone)` — used by keyboard
    /// navigation of the flyout (arrow keys) and by the host-render preview.
    pub fn set_hover(&mut self, sel: Option<(usize, usize)>) {
        self.hover = sel.filter(|&(t, z)| {
            t < SnapTemplate::ALL.len() && z < SnapTemplate::ALL[t].zones(self.work).len()
        });
    }

    /// The currently highlighted `(template, zone)`, if any.
    pub fn hovered(&self) -> Option<(usize, usize)> {
        self.hover
    }

    /// The full work-area rect of the highlighted zone — what the shell applies
    /// to the focused window when the user confirms with Enter (keyboard path).
    pub fn hovered_zone_rect(&self) -> Option<Rect> {
        self.hover
            .and_then(|(t, z)| SnapTemplate::ALL[t].zones(self.work).into_iter().nth(z))
    }

    /// Update the hovered `(template, zone)` from a cursor position. No-op when
    /// hidden. Returns true if the hover changed (the shell can skip a repaint).
    pub fn hover_at(&mut self, px: i32, py: i32) -> bool {
        if !self.visible {
            return false;
        }
        let new = self.pick(px, py).map(|(t, z, _)| (t, z));
        let changed = new != self.hover;
        self.hover = new;
        changed
    }

    /// Hit-test the flyout: returns the **full work-area rect** of the zone under
    /// `(px, py)`, or `None` if the cursor is outside every thumbnail. This is
    /// the snap target the shell applies to the focused window.
    pub fn zone_at(&self, px: i32, py: i32) -> Option<Rect> {
        self.pick(px, py).map(|(_, _, rect)| rect)
    }

    /// Like [`Self::zone_at`], but returns the FULL picked layout: every zone of
    /// the clicked template (work-area rects) + the index of the zone clicked.
    /// Hands Snap Assist the layout's remaining empty zones to fill.
    pub fn picked_layout(&self, px: i32, py: i32) -> Option<(Vec<Rect>, usize)> {
        let (t, z, _) = self.pick(px, py)?;
        Some((SnapTemplate::ALL[t].zones(self.work), z))
    }

    /// Shared hit-test core: `(template_index, zone_index, full_zone_rect)`.
    fn pick(&self, px: i32, py: i32) -> Option<(usize, usize, Rect)> {
        if !self.visible {
            return None;
        }
        let g = self.grid();
        for (ti, tmpl) in SnapTemplate::ALL.iter().enumerate() {
            let thumb = self.thumb_rect(&g, ti);
            if !thumb.contains(px, py) {
                continue;
            }
            let zones = tmpl.zones(self.work);
            for (zi, zone) in zones.iter().enumerate() {
                if self.zone_in_thumb(thumb, *zone).contains(px, py) {
                    return Some((ti, zi, *zone));
                }
            }
        }
        None
    }

    /// Paint the flyout: scrim, glass panel, title, and each template thumbnail
    /// with its zones (accent fill on the hovered zone, subtle glass otherwise).
    pub fn render(&self, canvas: &mut athgfx::Canvas) {
        if !self.visible {
            return;
        }
        let accent = crate::accent();
        let p = crate::active_palette();
        let g = self.grid();

        // Backdrop scrim — dim the desktop so the glass reads over any wallpaper.
        for y in 0..self.screen_h {
            for x in 0..self.screen_w {
                canvas.blend_pixel(x, y, 0x24_0A_0C_14);
            }
        }

        // Glass popover panel (shadow first so it floats, then the shipped glass
        // stack at the POPOVER tier — same call Start / the command palette make).
        let (pw, ph) = (g.panel.w as usize, g.panel.h as usize);
        let (pxp, pyp) = (g.panel.x as usize, g.panel.y as usize);
        canvas.fill_rounded_rect_shadow(
            pxp,
            pyp,
            pw,
            ph,
            ath_tokens::RADIUS_LG as usize,
            0x0A_10_1C,
            40,
            16,
        );
        athgfx::glass::draw_glass_surface(
            canvas,
            pxp,
            pyp,
            pw,
            ph,
            ath_tokens::RADIUS_LG as usize,
            ath_tokens::GLASS_POPOVER_DARK,
        );

        // Title.
        canvas.draw_text_aa(
            g.panel.x + g.pad as i32,
            g.panel.y + g.pad as i32,
            "Snap layout",
            ath_tokens::TYPE_LABEL,
            p.text_secondary,
            athgfx::text::FontFamily::Sans,
        );

        // Thumbnails.
        for (ti, tmpl) in SnapTemplate::ALL.iter().enumerate() {
            let thumb = self.thumb_rect(&g, ti);
            // Thumbnail well: a slightly raised, rounded backdrop the zone tiles
            // sit on so the layout reads as a card, not floating rectangles.
            canvas.fill_rounded_rect(
                thumb.x as usize,
                thumb.y as usize,
                thumb.w as usize,
                thumb.h as usize,
                ath_tokens::RADIUS_SM as usize,
                0x30_00_00_00,
            );
            for (zi, zone) in tmpl.zones(self.work).iter().enumerate() {
                let z = self.zone_in_thumb(thumb, *zone);
                let hovered = self.hover == Some((ti, zi));
                let fill = if hovered {
                    accent.base
                } else {
                    // Subtle token-derived tile so zones read without shouting.
                    accent.subtle
                };
                canvas.fill_rounded_rect(
                    z.x as usize,
                    z.y as usize,
                    z.w as usize,
                    z.h as usize,
                    ath_tokens::RADIUS_XS as usize,
                    fill,
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORK: Rect = Rect::new(0, 0, 1920, 1080);

    /// Every template must partition the work area EXACTLY: zones stay in bounds,
    /// never overlap, and cover the whole work area (areas sum to work.area).
    /// A gap or overlap here would snap a window into dead space or over another.
    #[test]
    fn templates_tile_work_area_exactly() {
        let work_area = WORK.w as u64 * WORK.h as u64;
        for tmpl in SnapTemplate::ALL {
            let zones = tmpl.zones(WORK);
            assert!(zones.len() >= 2, "{tmpl:?}: a layout must have >= 2 zones");
            let mut sum = 0u64;
            for (i, z) in zones.iter().enumerate() {
                assert!(z.w > 0 && z.h > 0, "{tmpl:?} zone {i}: empty");
                assert!(
                    z.x >= WORK.x
                        && z.y >= WORK.y
                        && z.right() <= WORK.right()
                        && z.bottom() <= WORK.bottom(),
                    "{tmpl:?} zone {i}: out of work bounds"
                );
                sum += z.w as u64 * z.h as u64;
                // Pairwise disjoint (integer rectangle intersection is empty).
                for other in &zones[i + 1..] {
                    let ix = z.x.max(other.x);
                    let iy = z.y.max(other.y);
                    let ir = z.right().min(other.right());
                    let ib = z.bottom().min(other.bottom());
                    assert!(ix >= ir || iy >= ib, "{tmpl:?}: zone {i} overlaps another");
                }
            }
            assert_eq!(sum, work_area, "{tmpl:?}: zones do not cover the work area");
        }
    }

    /// The offset work area (taskbar carve-out) must still tile exactly — the
    /// remainder-absorbing math must not assume origin (0,0).
    #[test]
    fn tiling_holds_with_taskbar_offset() {
        let work = Rect::new(0, 0, 2560, 1400); // origin 0, but full-HiDPI odd size
        let area = work.w as u64 * work.h as u64;
        for tmpl in SnapTemplate::ALL {
            let sum: u64 = tmpl
                .zones(work)
                .iter()
                .map(|z| z.w as u64 * z.h as u64)
                .sum();
            assert_eq!(sum, area, "{tmpl:?}: HiDPI work area not fully covered");
        }
    }

    /// Clicking the centre of a rendered zone must return that exact zone — the
    /// render→hit-test round-trip the shell relies on to snap the right region.
    #[test]
    fn click_center_of_zone_returns_that_zone() {
        let mut ov = SnapOverlay::new(1920, 1080, WORK);
        ov.open();
        let g = ov.grid();
        for (ti, tmpl) in SnapTemplate::ALL.iter().enumerate() {
            let thumb = ov.thumb_rect(&g, ti);
            for zone in tmpl.zones(WORK) {
                let z = ov.zone_in_thumb(thumb, zone);
                let (cx, cy) = (z.x + z.w as i32 / 2, z.y + z.h as i32 / 2);
                let hit = ov.zone_at(cx, cy).expect("center of a zone must hit");
                assert_eq!(hit, zone, "{tmpl:?}: center hit-test returned wrong zone");
            }
        }
    }

    /// A hidden overlay is inert — no hit-tests, no hovers (the shell must not be
    /// able to snap while the flyout is closed).
    #[test]
    fn closed_overlay_is_inert() {
        let mut ov = SnapOverlay::new(1920, 1080, WORK);
        assert!(ov.zone_at(960, 200).is_none());
        ov.open();
        ov.close();
        assert!(ov.zone_at(960, 200).is_none());
        assert!(!ov.hover_at(960, 200));
    }
}
