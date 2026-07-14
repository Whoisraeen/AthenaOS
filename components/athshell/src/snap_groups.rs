//! Snap groups — remember windows snapped into a layout as a unit (Win11).
//!
//! Concept §"Windows pain points -> our answer": once you've filled a Snap
//! Layout (via [`crate::snap_assist`]), Windows 11 remembers those windows as a
//! *group* — minimize one and the whole group tucks away; restore it and the
//! whole layout snaps back into place. This module is that bookkeeping: a window
//! belongs to at most one group, a group is `(surface_id, zone_rect)` pairs, and
//! the shell drives minimize-together / restore-together off it.
//!
//! Pure state, so the membership + minimize/restore-set logic is host-KAT'd.

use crate::Rect;
use alloc::vec::Vec;

/// One snap group: the windows that were snapped together + the exact rect each
/// occupies in the layout (so a restore puts them all back).
#[derive(Debug, Clone)]
pub struct SnapGroup {
    pub members: Vec<(u64, Rect)>,
}

/// Tracks every live snap group. A window is in at most one group.
#[derive(Debug, Default)]
pub struct SnapGroups {
    groups: Vec<SnapGroup>,
}

impl SnapGroups {
    pub fn new() -> Self {
        Self { groups: Vec::new() }
    }

    /// Record a group from a completed layout. Any member already in another
    /// group is pulled out of it first (a window belongs to one group). A group
    /// of fewer than two members is meaningless and is dropped.
    pub fn form(&mut self, members: Vec<(u64, Rect)>) {
        for &(id, _) in &members {
            self.remove_window(id);
        }
        if members.len() >= 2 {
            self.groups.push(SnapGroup { members });
        }
    }

    /// Index of the group containing `id`, if any.
    fn group_index(&self, id: u64) -> Option<usize> {
        self.groups
            .iter()
            .position(|g| g.members.iter().any(|&(m, _)| m == id))
    }

    /// The `(surface_id, zone_rect)` of every window in the group that contains
    /// `id` (including `id` itself). Empty if `id` is not grouped — the caller
    /// then acts on the single window as usual.
    pub fn group_members(&self, id: u64) -> Vec<(u64, Rect)> {
        match self.group_index(id) {
            Some(i) => self.groups[i].members.clone(),
            None => Vec::new(),
        }
    }

    /// True when `id` is part of a group (so minimize/restore act on the set).
    pub fn is_grouped(&self, id: u64) -> bool {
        self.group_index(id).is_some()
    }

    /// Remove a window from its group (it was moved out, closed, or re-snapped
    /// elsewhere). Dissolves a group that drops below two members.
    pub fn remove_window(&mut self, id: u64) {
        if let Some(i) = self.group_index(id) {
            self.groups[i].members.retain(|&(m, _)| m != id);
            if self.groups[i].members.len() < 2 {
                self.groups.remove(i);
            }
        }
    }

    /// Drop a whole group (e.g. the layout was torn down).
    pub fn dissolve(&mut self, id: u64) {
        if let Some(i) = self.group_index(id) {
            self.groups.remove(i);
        }
    }

    pub fn group_count(&self) -> usize {
        self.groups.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(x: i32) -> Rect {
        Rect::new(x, 0, 100, 100)
    }

    /// A formed group is queryable from any of its members, and membership is
    /// symmetric (minimize/restore off any member acts on the whole set).
    #[test]
    fn group_is_queryable_from_any_member() {
        let mut g = SnapGroups::new();
        g.form(alloc::vec![(1, r(0)), (2, r(100)), (3, r(200))]);
        assert_eq!(g.group_count(), 1);
        assert!(g.is_grouped(2));
        let from1 = g.group_members(1);
        let from3 = g.group_members(3);
        assert_eq!(from1.len(), 3);
        assert_eq!(from1.len(), from3.len());
        // The zone rect is remembered per member (restore puts them back).
        assert!(from1.iter().any(|&(id, rect)| id == 2 && rect == r(100)));
    }

    /// A group of one is not a group (a single snapped window has no partners).
    #[test]
    fn single_member_is_not_a_group() {
        let mut g = SnapGroups::new();
        g.form(alloc::vec![(1, r(0))]);
        assert_eq!(g.group_count(), 0);
        assert!(!g.is_grouped(1));
        assert!(g.group_members(1).is_empty());
    }

    /// Re-snapping a window into a NEW layout moves it out of the old group; the
    /// old group dissolves if it drops below two.
    #[test]
    fn resnap_moves_window_and_dissolves_small_group() {
        let mut g = SnapGroups::new();
        g.form(alloc::vec![(1, r(0)), (2, r(100))]);
        // Window 2 joins a new group with 3 and 4.
        g.form(alloc::vec![(2, r(0)), (3, r(100)), (4, r(200))]);
        // The old {1,2} group had only 1 left -> dissolved.
        assert!(!g.is_grouped(1));
        // 2 is now in the new group.
        assert_eq!(g.group_members(2).len(), 3);
        assert_eq!(g.group_count(), 1);
    }

    /// Removing a member from a 3-group keeps the group (2 left); removing
    /// another dissolves it.
    #[test]
    fn remove_window_shrinks_then_dissolves() {
        let mut g = SnapGroups::new();
        g.form(alloc::vec![(1, r(0)), (2, r(100)), (3, r(200))]);
        g.remove_window(1);
        assert_eq!(g.group_count(), 1);
        assert_eq!(g.group_members(2).len(), 2);
        g.remove_window(2);
        assert_eq!(g.group_count(), 0);
    }
}
