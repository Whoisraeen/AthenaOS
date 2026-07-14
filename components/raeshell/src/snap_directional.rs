//! Keyboard directional window snapping — the Rae key + Arrow chords.
//!
//! Concept §"Windows pain points -> our answer": the single most-used window
//! shortcut on Windows is Win+←/→/↑/↓ (tile left/right, maximize, restore/
//! minimize). AthenaOS had the Snap Layouts flyout ([`crate::snap_layouts`]) and
//! edge-drag Aero snap, but not this keyboard path. This module is the pure
//! state machine + geometry behind it: each window carries a [`SnapState`], and
//! an arrow press transitions it (Win11-accurate: a side-snapped window snaps to
//! a quarter on ↑/↓, restores toward Normal on the opposite arrow, maximizes on
//! ↑ from Normal, minimizes on ↓ from the bottom). Being pure logic, the whole
//! transition table + tiling geometry is proven by a host KAT — no live desktop.

use crate::Rect;

/// A window's current directional-snap state. `Normal` = a free-floating window
/// at its user-chosen `restore` rect; the rest are managed regions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SnapState {
    /// Free-floating at the restore rect.
    #[default]
    Normal,
    Left,
    Right,
    Max,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
    /// Minimized to the taskbar.
    Min,
}

/// The four arrow directions of a Rae+Arrow chord.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapDir {
    Left,
    Right,
    Up,
    Down,
}

impl SnapState {
    /// The Win11 transition table: given the current state and an arrow, return
    /// the next state. Returning the SAME state means "no change" (the caller
    /// treats it as a no-op — e.g. Rae+← on an already-left-snapped window).
    pub fn apply(self, dir: SnapDir) -> SnapState {
        use SnapDir as D;
        use SnapState as S;
        match (self, dir) {
            // From a free window: sides snap, ↑ maximizes, ↓ minimizes.
            (S::Normal, D::Left) => S::Left,
            (S::Normal, D::Right) => S::Right,
            (S::Normal, D::Up) => S::Max,
            (S::Normal, D::Down) => S::Min,

            // Left half: → restores, ↑/↓ go to the left quarters.
            (S::Left, D::Right) => S::Normal,
            (S::Left, D::Up) => S::TopLeft,
            (S::Left, D::Down) => S::BottomLeft,

            // Right half: ← restores, ↑/↓ go to the right quarters.
            (S::Right, D::Left) => S::Normal,
            (S::Right, D::Up) => S::TopRight,
            (S::Right, D::Down) => S::BottomRight,

            // Maximized: ↓ restores, sides snap to the corresponding half.
            (S::Max, D::Down) => S::Normal,
            (S::Max, D::Left) => S::Left,
            (S::Max, D::Right) => S::Right,

            // Top quarters: ↓ grows back to the half, ← / → swap sides.
            (S::TopLeft, D::Down) => S::Left,
            (S::TopLeft, D::Right) => S::TopRight,
            (S::TopRight, D::Down) => S::Right,
            (S::TopRight, D::Left) => S::TopLeft,

            // Bottom quarters: ↑ grows back to the half, ↓ minimizes, sides swap.
            (S::BottomLeft, D::Up) => S::Left,
            (S::BottomLeft, D::Right) => S::BottomRight,
            (S::BottomLeft, D::Down) => S::Min,
            (S::BottomRight, D::Up) => S::Right,
            (S::BottomRight, D::Left) => S::BottomLeft,
            (S::BottomRight, D::Down) => S::Min,

            // Minimized: ↑ restores; nothing else (the window isn't focusable
            // while minimized, so this rarely fires — it's the safety exit).
            (S::Min, D::Up) => S::Normal,

            // Everything else is a no-op (same state).
            (s, _) => s,
        }
    }

    /// The rect this state occupies in `work`, or `None` for `Normal`
    /// (restore to the window's own saved rect) and `Min` (minimize — no rect).
    /// The side/quarter splits absorb odd-pixel remainder on the far edge so a
    /// left+right (or 2x2) pair tiles `work` exactly, matching Snap Layouts.
    pub fn geometry(self, work: Rect) -> Option<Rect> {
        let hw = work.w / 2;
        let hh = work.h / 2;
        let rw = work.w - hw; // right column absorbs the remainder
        let bh = work.h - hh; // bottom row absorbs the remainder
        let x = work.x;
        let y = work.y;
        let mx = work.x + hw as i32;
        let my = work.y + hh as i32;
        match self {
            SnapState::Left => Some(Rect::new(x, y, hw, work.h)),
            SnapState::Right => Some(Rect::new(mx, y, rw, work.h)),
            SnapState::Max => Some(work),
            SnapState::TopLeft => Some(Rect::new(x, y, hw, hh)),
            SnapState::TopRight => Some(Rect::new(mx, y, rw, hh)),
            SnapState::BottomLeft => Some(Rect::new(x, my, hw, bh)),
            SnapState::BottomRight => Some(Rect::new(mx, my, rw, bh)),
            SnapState::Normal | SnapState::Min => None,
        }
    }

    pub fn is_minimized(self) -> bool {
        matches!(self, SnapState::Min)
    }

    pub fn is_normal(self) -> bool {
        matches!(self, SnapState::Normal)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORK: Rect = Rect::new(0, 0, 1920, 1080);

    /// The canonical Win11 chords land where a user expects.
    #[test]
    fn common_chords_reach_expected_states() {
        use SnapDir as D;
        use SnapState as S;
        // From Normal.
        assert_eq!(S::Normal.apply(D::Left), S::Left);
        assert_eq!(S::Normal.apply(D::Right), S::Right);
        assert_eq!(S::Normal.apply(D::Up), S::Max);
        assert_eq!(S::Normal.apply(D::Down), S::Min);
        // Left half -> top-left quarter -> back to left half.
        assert_eq!(S::Left.apply(D::Up), S::TopLeft);
        assert_eq!(S::TopLeft.apply(D::Down), S::Left);
        // Left half -> restore with the opposite arrow.
        assert_eq!(S::Left.apply(D::Right), S::Normal);
        // Maximize then restore.
        assert_eq!(S::Max.apply(D::Down), S::Normal);
        // Corner swaps.
        assert_eq!(S::TopLeft.apply(D::Right), S::TopRight);
        assert_eq!(S::BottomRight.apply(D::Left), S::BottomLeft);
        // Bottom quarter -> minimize.
        assert_eq!(S::BottomLeft.apply(D::Down), S::Min);
    }

    /// Re-snapping the same direction is a no-op (returns the same state), so the
    /// caller doesn't thrash geometry on a held key.
    #[test]
    fn same_direction_is_a_noop() {
        assert_eq!(SnapState::Left.apply(SnapDir::Left), SnapState::Left);
        assert_eq!(SnapState::Right.apply(SnapDir::Right), SnapState::Right);
        assert_eq!(SnapState::Max.apply(SnapDir::Up), SnapState::Max);
    }

    /// Left + Right halves tile the work area exactly (no gap, no overlap) — the
    /// same guarantee Snap Layouts makes, so keyboard + flyout snapping agree.
    #[test]
    fn halves_and_quarters_tile_exactly() {
        let area = WORK.w as u64 * WORK.h as u64;
        let l = SnapState::Left.geometry(WORK).unwrap();
        let r = SnapState::Right.geometry(WORK).unwrap();
        assert_eq!(l.w as u64 * l.h as u64 + r.w as u64 * r.h as u64, area);
        assert_eq!(l.x + l.w as i32, r.x, "left/right must be adjacent, no gap");

        let quarters = [
            SnapState::TopLeft,
            SnapState::TopRight,
            SnapState::BottomLeft,
            SnapState::BottomRight,
        ];
        let sum: u64 = quarters
            .iter()
            .map(|s| {
                let g = s.geometry(WORK).unwrap();
                g.w as u64 * g.h as u64
            })
            .sum();
        assert_eq!(
            sum, area,
            "the four quarters must cover the work area exactly"
        );
    }

    /// Normal and Min carry no fixed rect (they're restore / minimize).
    #[test]
    fn normal_and_min_have_no_geometry() {
        assert!(SnapState::Normal.geometry(WORK).is_none());
        assert!(SnapState::Min.geometry(WORK).is_none());
        assert!(SnapState::Max.geometry(WORK).is_some());
    }
}
