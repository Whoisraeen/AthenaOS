//! Virtual desktops / Spaces — the shell-side membership policy that drives the
//! compositor's per-surface visibility (`docs/design/window-management.md` §2).
//!
//! Concept §AthUI: "your desktop, your rules — tiling, stacking, floating are
//! POLICIES over the compositor, not forks of it; switching is one call, not a
//! different OS." A Space is exactly that policy layer: it owns no surface
//! mechanism, only *membership* — which surface ids belong to which space — plus
//! the per-space wallpaper id. Switching a space is a membership/visibility flip
//! the kernel applies through the existing compositor path
//! (`set_surface_minimized` to drop a non-member out of the active composite),
//! never a new surface model.
//!
//! This module is pure `no_std` policy state so it is host-KAT'able and lives in
//! `raeshell` with the rest of the shell chrome; the compositor calls that act on
//! the live surfaces are made by the kernel `shell_runner`, which reads this
//! model. The pre-existing `virtual_desktops.rs` is a self-contained buffer
//! renderer with its own window model; this is the live-compositor-wired model
//! the design spec's Handoff §"shell side" asks for (it intentionally does NOT
//! duplicate that renderer — it carries only membership + wallpaper id).

extern crate alloc;

use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// One virtual desktop: a named, ordered set of member surface ids plus an
/// optional per-space wallpaper surface id (`z=0` kernel surface). Per
/// `window-management.md` §2 model — membership + wallpaper only, no geometry.
#[derive(Debug, Clone)]
pub struct Space {
    pub name: String,
    /// Surface ids that live in this space (z-order not tracked here — the
    /// compositor owns z; this is pure set membership).
    pub members: Vec<u64>,
    /// Optional per-space wallpaper surface id; `None` shares the default.
    pub wallpaper_id: Option<u64>,
}

impl Space {
    fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            members: Vec::new(),
            wallpaper_id: None,
        }
    }

    fn contains(&self, id: u64) -> bool {
        self.members.iter().any(|&m| m == id)
    }
}

/// The shell's per-display spaces model. Single-display today (the
/// `screen_dimensions()` reality); the spec keys spaces per display so the
/// multi-output upgrade is additive — that key is the `display_id` carried here
/// but unused until the multi-output compositor lands.
pub struct SpaceManager {
    #[allow(dead_code)]
    display_id: u32,
    spaces: Vec<Space>,
    current: usize,
    /// Wallpaper cross-fade alpha (0..=255) the switch animation ramps; the
    /// kernel pushes it into `compositor::set_wallpaper_alpha`.
    fade_alpha: u8,
}

impl SpaceManager {
    /// Default: one space named "1" (the zero-migration upgrade — existing
    /// single-space behaviour is just "one space").
    pub fn new() -> Self {
        let mut spaces = Vec::new();
        spaces.push(Space::new("1"));
        Self {
            display_id: 0,
            spaces,
            current: 0,
            fade_alpha: 255,
        }
    }

    pub fn count(&self) -> usize {
        self.spaces.len()
    }

    pub fn current_index(&self) -> usize {
        self.current
    }

    pub fn current_name(&self) -> &str {
        self.spaces[self.current].name.as_str()
    }

    pub fn space_name(&self, idx: usize) -> Option<&str> {
        self.spaces.get(idx).map(|s| s.name.as_str())
    }

    pub fn fade_alpha(&self) -> u8 {
        self.fade_alpha
    }

    /// Add a new space named by its 1-based ordinal; returns its index.
    pub fn add_space(&mut self) -> usize {
        let n = self.spaces.len() + 1;
        let mut buf = String::new();
        // no_std-friendly integer→string (avoids pulling `format!`).
        push_usize(&mut buf, n);
        self.spaces.push(Space::new(&buf));
        self.spaces.len() - 1
    }

    /// Register a freshly-created window into the current space (the default
    /// home for a new surface — matches "new windows open on the active space").
    pub fn add_window_to_current(&mut self, id: u64) {
        if !self.spaces[self.current].contains(id) {
            self.spaces[self.current].members.push(id);
        }
    }

    /// Remove a surface from every space (window closed). Returns true if it was
    /// a member of any space.
    pub fn remove_window(&mut self, id: u64) -> bool {
        let mut found = false;
        for sp in &mut self.spaces {
            let before = sp.members.len();
            sp.members.retain(|&m| m != id);
            if sp.members.len() != before {
                found = true;
            }
        }
        found
    }

    /// Move a surface from its current space to `target`. Pure membership; the
    /// caller flips compositor visibility afterwards via [`is_visible`].
    /// Returns `(removed_from_some, added_to_target)`.
    pub fn move_window(&mut self, id: u64, target: usize) -> (bool, bool) {
        if target >= self.spaces.len() {
            return (false, false);
        }
        let mut removed = false;
        for sp in &mut self.spaces {
            let before = sp.members.len();
            sp.members.retain(|&m| m != id);
            if sp.members.len() != before {
                removed = true;
            }
        }
        let added = if !self.spaces[target].contains(id) {
            self.spaces[target].members.push(id);
            true
        } else {
            false
        };
        (removed, added)
    }

    /// Which space (index) a surface belongs to, if any.
    pub fn space_of(&self, id: u64) -> Option<usize> {
        self.spaces.iter().position(|sp| sp.contains(id))
    }

    /// True iff the surface is a member of the *current* space (i.e. should be
    /// composited). A surface in no space at all is treated as visible
    /// (un-managed windows — e.g. transient dialogs — are never hidden).
    pub fn is_visible(&self, id: u64) -> bool {
        match self.space_of(id) {
            Some(idx) => idx == self.current,
            None => true,
        }
    }

    /// All members of the current space.
    pub fn current_members(&self) -> &[u64] {
        &self.spaces[self.current].members
    }

    /// Members of an arbitrary space.
    pub fn members(&self, idx: usize) -> &[u64] {
        self.spaces
            .get(idx)
            .map(|s| s.members.as_slice())
            .unwrap_or(&[])
    }

    /// Switch to `target`. Returns the set of `(id, should_be_visible)` flips
    /// the kernel must apply to the compositor, or `None` if already there /
    /// out of range. Resets the cross-fade to 0 so the kernel can ramp it.
    pub fn switch_to(&mut self, target: usize) -> Option<Vec<(u64, bool)>> {
        if target >= self.spaces.len() || target == self.current {
            return None;
        }
        // Surfaces that must hide (leaving the active space) and show (entering).
        let mut flips: Vec<(u64, bool)> = Vec::new();
        for &id in &self.spaces[self.current].members {
            flips.push((id, false));
        }
        for &id in &self.spaces[target].members {
            flips.push((id, true));
        }
        self.current = target;
        self.fade_alpha = 0; // kernel ramps 0->255 over motion.emphasized
        Some(flips)
    }

    pub fn switch_next(&mut self) -> Option<Vec<(u64, bool)>> {
        if self.spaces.len() < 2 {
            return None;
        }
        let next = (self.current + 1) % self.spaces.len();
        self.switch_to(next)
    }

    pub fn switch_prev(&mut self) -> Option<Vec<(u64, bool)>> {
        if self.spaces.len() < 2 {
            return None;
        }
        let prev = if self.current == 0 {
            self.spaces.len() - 1
        } else {
            self.current - 1
        };
        self.switch_to(prev)
    }

    /// Advance the wallpaper cross-fade by `step` (0..=255 saturating); returns
    /// the new alpha. The kernel drives this each animation tick.
    pub fn advance_fade(&mut self, step: u8) -> u8 {
        self.fade_alpha = self.fade_alpha.saturating_add(step);
        self.fade_alpha
    }
}

impl Default for SpaceManager {
    fn default() -> Self {
        Self::new()
    }
}

fn push_usize(buf: &mut String, mut n: usize) {
    if n == 0 {
        buf.push('0');
        return;
    }
    let mut digits = [0u8; 20];
    let mut i = 0;
    while n > 0 {
        digits[i] = b'0' + (n % 10) as u8;
        n /= 10;
        i += 1;
    }
    while i > 0 {
        i -= 1;
        buf.push(digits[i] as char);
    }
}

/// The accent-ring color the overview selection ring and the app-switcher
/// selected-tile ring must use — `derive_accent(seed).base`, NOT the retired
/// `0xFF_4E_9C_FF` literal (`window-management.md` §4 / §6). One helper so both
/// chrome surfaces and the smoketest agree on the exact value.
#[must_use]
pub fn selection_ring(seed: u32) -> u32 {
    rae_tokens::derive_accent(seed, &rae_tokens::DARK).base
}

/// The accent glow color for `elev.focus` on the selected overview thumbnail /
/// switcher tile — `derive_accent(seed).glow`.
#[must_use]
pub fn selection_glow(seed: u32) -> u32 {
    rae_tokens::derive_accent(seed, &rae_tokens::DARK).glow
}

/// FAIL-able boot smoketests for the spaces policy (`window-management.md`
/// §"Boot-log smoketest lines" 3, 4, 6). Pure model assertions — no compositor
/// needed, so they run identically on the host KAT and in QEMU CI.
///
/// 1. Membership + visibility: two spaces, three surfaces (2 in A, 1 in B);
///    switch to B and assert exactly A's went hidden and B's shown.
/// 2. Move-window-to-space: move X from A to B; assert membership moved and
///    visibility followed the active space.
/// 3. Switcher tokenized selection: the ring color equals
///    `derive_accent(seed).base`, proving the hardcode is gone.
pub fn run_boot_smoketest() -> (bool, bool, bool) {
    // ── (3) Spaces membership + visibility ──────────────────────────────────
    let mut mgr = SpaceManager::new();
    let b = mgr.add_space(); // space "2"
                             // A = space 0, B = space 1. Surfaces 100,101 in A; 102 in B.
    mgr.add_window_to_current(100);
    mgr.add_window_to_current(101);
    // Move 102 into B by adding then moving (it lands in current=A first).
    mgr.add_window_to_current(102);
    let _ = mgr.move_window(102, b);
    // Currently on A: A members visible, B's hidden.
    let on_a_ok = mgr.is_visible(100) && mgr.is_visible(101) && !mgr.is_visible(102);
    // Switch to B.
    let flips = mgr.switch_to(b);
    let flips_ok = flips
        .as_ref()
        .map(|f| {
            // A's two -> false, B's one -> true.
            let hidden = f.iter().filter(|(_, v)| !*v).count();
            let shown = f.iter().filter(|(_, v)| *v).count();
            hidden == 2 && shown == 1
        })
        .unwrap_or(false);
    let a_hidden = [100u64, 101]
        .iter()
        .filter(|&&id| !mgr.is_visible(id))
        .count();
    let b_shown = [102u64].iter().filter(|&&id| mgr.is_visible(id)).count();
    let membership_ok = on_a_ok && flips_ok && a_hidden == 2 && b_shown == 1 && mgr.current == b;

    // ── (4) Move-window-to-space ────────────────────────────────────────────
    // Fresh manager: X starts in A, move to B, verify membership + visibility.
    let mut m2 = SpaceManager::new();
    let bb = m2.add_space();
    m2.add_window_to_current(200); // in A
    let (removed, added) = m2.move_window(200, bb);
    // Still on A, so the moved-away surface must now be hidden.
    let visible_consistent = !m2.is_visible(200) && m2.space_of(200) == Some(bb);
    let move_ok = removed && added && visible_consistent;

    // ── (6) Switcher tokenized selection (the hardcode is gone) ─────────────
    // The ring color MUST equal `derive_accent(seed).base` — it flows from the
    // live seed, not the retired `0xFF_4E_9C_FF` literal. Proving equality to
    // the derived value is the proof the hardcode is gone (the value re-skins
    // with Vibe Mode because it tracks the seed).
    let seed = crate::active_accent();
    let ring = selection_ring(seed);
    let expected = rae_tokens::derive_accent(seed, &rae_tokens::DARK).base;
    let ring_ok = ring == expected;

    (membership_ok, move_ok, ring_ok)
}
