//! Snap Assist — the "fill the rest of the layout" flow (Win11 parity).
//!
//! Concept §"Windows pain points -> our answer": after you snap one window into
//! a [`crate::snap_layouts`] zone (or a Rae+Arrow half), Windows 11 offers the
//! remaining zones filled with pickable thumbnails of your OTHER windows, so a
//! whole layout comes together in a couple of clicks. This module is that flow:
//! it tracks per-zone occupancy + the candidate windows and, for the current
//! empty zone, presents the candidates; picking one snaps it there and advances
//! to the next empty zone until the layout is full (or Esc dismisses).
//!
//! Pure state + a self-contained overlay renderer, so the occupancy/advance
//! logic is host-KAT'd and the look is host-rendered — no live desktop needed.

use crate::tiling_wm::Rect;
use alloc::string::String;
use alloc::vec::Vec;

/// A window that can fill a zone: its surface id + a title for the tile label.
#[derive(Debug, Clone)]
pub struct Candidate {
    pub id: u64,
    pub title: String,
}

/// The Snap Assist flow over one layout. Zones come from the Snap Layouts
/// template the first window was snapped into; `occupancy[i]` is the window in
/// zone `i` (or `None`). The overlay fills the FIRST empty zone with candidate
/// tiles; each pick advances to the next empty zone.
pub struct SnapAssist {
    pub visible: bool,
    screen_w: usize,
    screen_h: usize,
    zones: Vec<Rect>,
    occupancy: Vec<Option<u64>>,
    candidates: Vec<Candidate>,
    /// Hovered candidate tile index within the current target zone.
    hover: Option<usize>,
}

impl SnapAssist {
    /// Begin Snap Assist after `occupant` was snapped into `occupied_zone` of a
    /// layout whose zones are `zones`. `candidates` are the other open windows.
    /// Inactive (never shows) if there is no empty zone or no candidate.
    pub fn new(
        zones: Vec<Rect>,
        occupied_zone: usize,
        occupant: u64,
        candidates: Vec<Candidate>,
        screen_w: usize,
        screen_h: usize,
    ) -> Self {
        let mut occupancy = alloc::vec![None; zones.len()];
        if occupied_zone < occupancy.len() {
            occupancy[occupied_zone] = Some(occupant);
        }
        let mut a = SnapAssist {
            visible: false,
            screen_w,
            screen_h,
            zones,
            occupancy,
            candidates,
            hover: None,
        };
        a.visible = a.current_target().is_some() && !a.candidates.is_empty();
        a
    }

    pub fn close(&mut self) {
        self.visible = false;
        self.hover = None;
    }

    /// True while the flow owns input (has an empty zone AND candidates left).
    pub fn is_active(&self) -> bool {
        self.visible && self.current_target().is_some() && !self.candidates.is_empty()
    }

    /// The first empty zone — the one currently offering candidates.
    pub fn current_target(&self) -> Option<usize> {
        self.occupancy.iter().position(|o| o.is_none())
    }

    pub fn candidates(&self) -> &[Candidate] {
        &self.candidates
    }

    /// Every occupied zone as `(window_id, zone_rect)` — used to form a snap
    /// group once the layout is complete.
    pub fn placements(&self) -> Vec<(u64, Rect)> {
        self.zones
            .iter()
            .enumerate()
            .filter_map(|(i, z)| self.occupancy[i].map(|id| (id, *z)))
            .collect()
    }

    /// Assign candidate `cand_idx` to the current target zone. Returns the
    /// `(window_id, zone_rect)` to snap, and advances: the candidate is consumed
    /// and, if that was the last empty zone or last candidate, the flow closes.
    pub fn pick(&mut self, cand_idx: usize) -> Option<(u64, Rect)> {
        let target = self.current_target()?;
        if cand_idx >= self.candidates.len() {
            return None;
        }
        let cand = self.candidates.remove(cand_idx);
        self.occupancy[target] = Some(cand.id);
        let rect = self.zones[target];
        self.hover = None;
        if self.current_target().is_none() || self.candidates.is_empty() {
            self.visible = false;
        }
        Some((cand.id, rect))
    }

    /// Layout of the candidate tiles inside the current target zone (screen
    /// space). A simple grid that keeps tiles a readable size. Shared by render
    /// + hit-test so they never drift.
    fn tile_rects(&self) -> Vec<Rect> {
        let Some(target) = self.current_target() else {
            return Vec::new();
        };
        let zone = self.zones[target];
        let n = self.candidates.len();
        if n == 0 {
            return Vec::new();
        }
        let pad = rae_tokens::SPACE_4 as i32;
        let gap = rae_tokens::SPACE_3 as i32;
        // Prefer a near-square grid, capped so tiles stay finger/mouse sized.
        // Integer ceil(sqrt(n)) — no std `f32::sqrt` in this no_std crate.
        let mut cols = 1i32;
        while cols * cols < n as i32 {
            cols += 1;
        }
        let cols = cols.clamp(1, 4);
        let rows = ((n as i32) + cols - 1) / cols;
        let avail_w = (zone.w as i32 - 2 * pad - (cols - 1) * gap).max(1);
        let avail_h = (zone.h as i32 - 2 * pad - (rows - 1) * gap).max(1);
        let tw = (avail_w / cols).clamp(1, 320);
        let th = (avail_h / rows).clamp(1, 220);
        // Center the grid within the zone.
        let grid_w = cols * tw + (cols - 1) * gap;
        let grid_h = rows * th + (rows - 1) * gap;
        let ox = zone.x + (zone.w as i32 - grid_w) / 2;
        let oy = zone.y + (zone.h as i32 - grid_h) / 2;
        let mut v = Vec::with_capacity(n);
        for i in 0..n as i32 {
            let c = i % cols;
            let r = i / cols;
            v.push(Rect::new(
                ox + c * (tw + gap),
                oy + r * (th + gap),
                tw as u32,
                th as u32,
            ));
        }
        v
    }

    /// Directly set the highlighted candidate tile — keyboard navigation of the
    /// picker (arrows) and the host-render preview. Clamped to a valid tile.
    pub fn set_hover(&mut self, idx: Option<usize>) {
        self.hover = idx.filter(|&i| i < self.candidates.len());
    }

    /// Update the hovered candidate tile from a cursor position. Returns true if
    /// the highlight changed (repaint hint).
    pub fn hover_at(&mut self, px: i32, py: i32) -> bool {
        if !self.is_active() {
            return false;
        }
        let new = self.tile_rects().iter().position(|t| t.contains(px, py));
        let changed = new != self.hover;
        self.hover = new;
        changed
    }

    /// A click at `(px, py)`: if it hits a candidate tile, pick it (returns the
    /// snap target); if it misses every tile, dismiss the flow (returns `None`).
    pub fn click(&mut self, px: i32, py: i32) -> Option<(u64, Rect)> {
        if !self.is_active() {
            return None;
        }
        let hit = self.tile_rects().iter().position(|t| t.contains(px, py));
        match hit {
            Some(i) => self.pick(i),
            None => {
                self.close();
                None
            }
        }
    }

    /// Paint the overlay: a scrim, the already-filled zones outlined, and the
    /// current target zone filled with candidate tiles (accent on hover).
    pub fn render(&self, canvas: &mut raegfx::Canvas) {
        if !self.is_active() {
            return;
        }
        let accent = crate::accent();
        let p = crate::active_palette();

        // Scrim across the whole screen so the picker reads over any wallpaper.
        for y in 0..self.screen_h {
            for x in 0..self.screen_w {
                canvas.blend_pixel(x, y, 0x30_0A_0C_14);
            }
        }

        // Already-occupied zones: a quiet outline so the user sees the layout
        // taking shape (the real windows show through the light scrim).
        for (i, z) in self.zones.iter().enumerate() {
            if self.occupancy[i].is_some() {
                canvas.draw_rect_outline(
                    z.x as usize,
                    z.y as usize,
                    z.w as usize,
                    z.h as usize,
                    accent.subtle,
                );
            }
        }

        // Current target zone: a glass field + a header + candidate tiles.
        if let Some(target) = self.current_target() {
            let z = self.zones[target];
            raegfx::glass::draw_glass_surface(
                canvas,
                z.x as usize,
                z.y as usize,
                z.w as usize,
                z.h as usize,
                rae_tokens::RADIUS_LG as usize,
                rae_tokens::GLASS_POPOVER_DARK,
            );
            canvas.draw_text_aa(
                z.x + rae_tokens::SPACE_4 as i32,
                z.y + rae_tokens::SPACE_3 as i32,
                "Pick a window",
                rae_tokens::TYPE_LABEL,
                p.text_secondary,
                raegfx::text::FontFamily::Sans,
            );

            let tiles = self.tile_rects();
            for (i, t) in tiles.iter().enumerate() {
                let hovered = self.hover == Some(i);
                // Tile card: accent-tinted on hover, else a raised glass swatch.
                let fill = if hovered {
                    accent.subtle
                } else {
                    0x40_00_00_00
                };
                canvas.fill_rounded_rect(
                    t.x as usize,
                    t.y as usize,
                    t.w as usize,
                    t.h as usize,
                    rae_tokens::RADIUS_MD as usize,
                    fill,
                );
                if hovered {
                    canvas.draw_rect_outline(
                        t.x as usize,
                        t.y as usize,
                        t.w as usize,
                        t.h as usize,
                        accent.base,
                    );
                }
                // Title label (clipped to the tile width by the text renderer).
                if let Some(c) = self.candidates.get(i) {
                    canvas.draw_text_aa(
                        t.x + rae_tokens::SPACE_2 as i32,
                        t.y + t.h as i32 - rae_tokens::SPACE_5 as i32,
                        &c.title,
                        rae_tokens::TYPE_CAPTION,
                        p.text_primary,
                        raegfx::text::FontFamily::Sans,
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snap_layouts::SnapTemplate;

    const WORK: Rect = Rect::new(0, 0, 1920, 1040);

    fn cands(ids: &[u64]) -> Vec<Candidate> {
        ids.iter()
            .map(|&id| Candidate {
                id,
                title: alloc::format!("Win{id}"),
            })
            .collect()
    }

    /// After snapping one window into a 2-zone layout, the OTHER zone is the
    /// target and the assist is active with the remaining candidates.
    #[test]
    fn activates_with_one_empty_zone() {
        let zones = SnapTemplate::TwoEven.zones(WORK);
        let a = SnapAssist::new(zones, 0, 100, cands(&[200, 300]), 1920, 1080);
        assert!(a.is_active());
        assert_eq!(a.current_target(), Some(1));
        assert_eq!(a.candidates().len(), 2);
    }

    /// Picking a candidate snaps it to the target zone and, in a 2-zone layout,
    /// completes the flow (no empty zones left).
    #[test]
    fn pick_fills_zone_and_completes() {
        let zones = SnapTemplate::TwoEven.zones(WORK);
        let right = zones[1];
        let mut a = SnapAssist::new(zones, 0, 100, cands(&[200, 300]), 1920, 1080);
        let (id, rect) = a.pick(0).expect("pick returns the snap target");
        assert_eq!(id, 200);
        assert_eq!(rect, right, "candidate fills the empty (right) zone");
        assert!(!a.is_active(), "2-zone layout is full after one pick");
    }

    /// A 4-zone layout advances zone-by-zone as candidates are picked.
    #[test]
    fn quadrants_advance_until_full() {
        let zones = SnapTemplate::Quadrants.zones(WORK);
        let mut a = SnapAssist::new(zones.clone(), 0, 1, cands(&[2, 3, 4]), 1920, 1080);
        assert_eq!(a.current_target(), Some(1));
        assert_eq!(a.pick(0).unwrap().1, zones[1]);
        assert_eq!(a.current_target(), Some(2));
        assert_eq!(a.pick(0).unwrap().1, zones[2]);
        assert_eq!(a.current_target(), Some(3));
        assert_eq!(a.pick(0).unwrap().1, zones[3]);
        assert!(!a.is_active(), "all four quadrants filled");
    }

    /// No candidates => never activates (nothing to offer).
    #[test]
    fn inactive_without_candidates() {
        let zones = SnapTemplate::TwoEven.zones(WORK);
        let a = SnapAssist::new(zones, 0, 100, cands(&[]), 1920, 1080);
        assert!(!a.is_active());
        assert!(!a.visible);
    }

    /// Clicking the center of a rendered candidate tile picks that candidate —
    /// the render->hit-test round-trip the shell relies on.
    #[test]
    fn click_tile_center_picks_it() {
        let zones = SnapTemplate::TwoEven.zones(WORK);
        let mut a = SnapAssist::new(zones, 0, 100, cands(&[200, 300]), 1920, 1080);
        let tiles = a.tile_rects();
        let t1 = tiles[1];
        let (cx, cy) = (t1.x + t1.w as i32 / 2, t1.y + t1.h as i32 / 2);
        let (id, _) = a.click(cx, cy).expect("click on a tile picks it");
        assert_eq!(id, 300, "second tile is the second candidate");
    }
}
