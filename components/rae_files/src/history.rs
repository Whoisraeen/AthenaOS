//! Tabs + per-tab back/forward history — the Explorer-tabs / Finder-tabs model
//! (RaeenOS_Concept.md §Windows Pain Points: "the modern file manager").
//!
//! Each [`Tab`] is its own current directory plus a bounded history stack with a
//! cursor: `navigate` truncates the forward history (a fresh branch, like a
//! browser), `back`/`forward` move the cursor without losing entries. The
//! [`TabSet`] owns a fixed array of tabs and the active index. Alloc-free and
//! panic-free: every bound is checked and surfaced as a `TabError`.

use crate::Path;

/// Max open tabs.
pub const MAX_TABS: usize = 8;
/// Max history depth per tab (back/forward).
pub const MAX_HISTORY: usize = 32;

/// Why a tab/history operation failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabError {
    /// All [`MAX_TABS`] slots are in use.
    Full,
    /// Refused to close the last remaining tab (a window keeps ≥1 tab).
    LastTab,
    /// The requested tab index does not exist.
    NoSuchTab,
    /// The path did not fit [`crate::PATH_CAP`].
    PathTooLong,
}

/// One tab: a directory + a back/forward history with a cursor.
#[derive(Clone, Copy)]
pub struct Tab {
    hist: [Path; MAX_HISTORY],
    len: usize,    // number of valid entries
    cursor: usize, // index of the current entry within [0, len)
}

impl Tab {
    /// A new tab rooted at `path`.
    pub fn new(path: &str) -> Result<Self, TabError> {
        let p = Path::from_str(path).ok_or(TabError::PathTooLong)?;
        let mut hist = [Path::new(); MAX_HISTORY];
        hist[0] = p;
        Ok(Self {
            hist,
            len: 1,
            cursor: 0,
        })
    }

    /// The current directory.
    pub fn cwd(&self) -> &str {
        self.hist[self.cursor].as_str()
    }

    /// Navigate to `path`: drops any forward history, pushes the new entry, and
    /// moves the cursor onto it. If history is full, the oldest entry is evicted
    /// (the cursor stays on the newest). A navigate to the current dir is a
    /// no-op (does not pollute history).
    pub fn navigate(&mut self, path: &str) -> Result<(), TabError> {
        let p = Path::from_str(path).ok_or(TabError::PathTooLong)?;
        if p.as_str() == self.cwd() {
            return Ok(());
        }
        // Truncate forward history.
        self.len = self.cursor + 1;
        if self.len < MAX_HISTORY {
            self.hist[self.len] = p;
            self.len += 1;
            self.cursor = self.len - 1;
        } else {
            // Full: shift everything down by one, drop the oldest.
            for i in 1..MAX_HISTORY {
                self.hist[i - 1] = self.hist[i];
            }
            self.hist[MAX_HISTORY - 1] = p;
            self.cursor = MAX_HISTORY - 1;
            self.len = MAX_HISTORY;
        }
        Ok(())
    }

    /// True iff `back` would move.
    pub fn can_back(&self) -> bool {
        self.cursor > 0
    }
    /// True iff `forward` would move.
    pub fn can_forward(&self) -> bool {
        self.cursor + 1 < self.len
    }

    /// Move back one entry; returns the new cwd or `None` if at the start.
    pub fn back(&mut self) -> Option<&str> {
        if self.cursor == 0 {
            return None;
        }
        self.cursor -= 1;
        Some(self.hist[self.cursor].as_str())
    }

    /// Move forward one entry; returns the new cwd or `None` if at the end.
    pub fn forward(&mut self) -> Option<&str> {
        if self.cursor + 1 >= self.len {
            return None;
        }
        self.cursor += 1;
        Some(self.hist[self.cursor].as_str())
    }
}

/// The set of open tabs + the active index. Always holds ≥1 tab.
pub struct TabSet {
    tabs: [Tab; MAX_TABS],
    count: usize,
    active: usize,
}

impl TabSet {
    /// A tab set with a single tab at `path`.
    pub fn new(path: &str) -> Result<Self, TabError> {
        let first = Tab::new(path)?;
        Ok(Self {
            tabs: [first; MAX_TABS],
            count: 1,
            active: 0,
        })
    }

    pub fn count(&self) -> usize {
        self.count
    }
    pub fn active_index(&self) -> usize {
        self.active
    }

    /// The active tab (immutable).
    pub fn active(&self) -> &Tab {
        &self.tabs[self.active]
    }
    /// The active tab (mutable) — the app drives navigation through this.
    pub fn active_mut(&mut self) -> &mut Tab {
        &mut self.tabs[self.active]
    }

    /// Tab `i`, if it exists.
    pub fn get(&self, i: usize) -> Option<&Tab> {
        if i < self.count {
            Some(&self.tabs[i])
        } else {
            None
        }
    }

    /// Open a new tab at `path` and make it active. Returns its index.
    pub fn open(&mut self, path: &str) -> Result<usize, TabError> {
        if self.count >= MAX_TABS {
            return Err(TabError::Full);
        }
        let t = Tab::new(path)?;
        self.tabs[self.count] = t;
        self.active = self.count;
        self.count += 1;
        Ok(self.active)
    }

    /// Close tab `i`. Refuses the last tab. Adjusts the active index so it stays
    /// valid (Chrome/Finder behavior: focus the neighbor).
    pub fn close(&mut self, i: usize) -> Result<(), TabError> {
        if i >= self.count {
            return Err(TabError::NoSuchTab);
        }
        if self.count == 1 {
            return Err(TabError::LastTab);
        }
        // Shift left over the removed slot.
        for j in i..self.count - 1 {
            self.tabs[j] = self.tabs[j + 1];
        }
        self.count -= 1;
        if self.active >= self.count {
            self.active = self.count - 1;
        } else if self.active > i {
            self.active -= 1;
        }
        Ok(())
    }

    /// Activate tab `i`.
    pub fn select(&mut self, i: usize) -> Result<(), TabError> {
        if i >= self.count {
            return Err(TabError::NoSuchTab);
        }
        self.active = i;
        Ok(())
    }

    /// Cycle to the next tab (wraps) — Ctrl+Tab.
    pub fn next(&mut self) {
        self.active = (self.active + 1) % self.count;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn navigate_builds_back_forward() {
        let mut t = Tab::new("/home/raeen").unwrap();
        t.navigate("/home/raeen/Documents").unwrap();
        t.navigate("/home/raeen/Documents/sub").unwrap();
        assert_eq!(t.cwd(), "/home/raeen/Documents/sub");
        assert!(t.can_back());
        assert!(!t.can_forward());
        assert_eq!(t.back(), Some("/home/raeen/Documents"));
        assert_eq!(t.back(), Some("/home/raeen"));
        assert_eq!(t.back(), None); // at start
        assert!(t.can_forward());
        assert_eq!(t.forward(), Some("/home/raeen/Documents"));
    }

    #[test]
    fn navigate_after_back_truncates_forward() {
        let mut t = Tab::new("/a").unwrap();
        t.navigate("/b").unwrap();
        t.navigate("/c").unwrap();
        t.back(); // now at /b, forward = /c
        t.navigate("/d").unwrap(); // branch — /c is dropped
        assert_eq!(t.cwd(), "/d");
        assert!(!t.can_forward());
        assert_eq!(t.back(), Some("/b"));
    }

    #[test]
    fn navigate_to_same_dir_is_noop() {
        let mut t = Tab::new("/a").unwrap();
        t.navigate("/a").unwrap();
        assert!(!t.can_back()); // history did not grow
    }

    #[test]
    fn open_and_close_tabs() {
        let mut ts = TabSet::new("/home").unwrap();
        assert_eq!(ts.count(), 1);
        let i = ts.open("/etc").unwrap();
        assert_eq!(i, 1);
        assert_eq!(ts.active_index(), 1);
        assert_eq!(ts.active().cwd(), "/etc");
        ts.close(1).unwrap();
        assert_eq!(ts.count(), 1);
        assert_eq!(ts.active_index(), 0);
    }

    #[test]
    fn cannot_close_last_tab() {
        let mut ts = TabSet::new("/home").unwrap();
        assert_eq!(ts.close(0), Err(TabError::LastTab));
    }

    #[test]
    fn full_tabset_rejects_more() {
        let mut ts = TabSet::new("/0").unwrap();
        for n in 1..MAX_TABS {
            ts.open("/x").unwrap();
            let _ = n;
        }
        assert_eq!(ts.open("/over"), Err(TabError::Full));
    }

    #[test]
    fn close_active_focuses_neighbor() {
        let mut ts = TabSet::new("/a").unwrap();
        ts.open("/b").unwrap();
        ts.open("/c").unwrap(); // active = 2
        ts.select(1).unwrap();
        ts.close(1).unwrap(); // closing active
        assert!(ts.active_index() < ts.count());
    }

    #[test]
    fn fail_able_history_cap_holds() {
        // Drive past MAX_HISTORY; cursor must stay in-bounds and cwd correct.
        let mut t = Tab::new("/h0").unwrap();
        for n in 1..(MAX_HISTORY + 10) {
            // Each path distinct so navigate is never a no-op.
            let p = match n % 4 {
                0 => "/h_a",
                1 => "/h_b",
                2 => "/h_c",
                _ => "/h_d",
            };
            // Ensure distinct-from-current by alternating; navigate handles dup.
            let _ = t.navigate(p);
        }
        // Must never panic; cwd must be a valid string.
        assert!(!t.cwd().is_empty());
        // FAIL-able: if eviction math regressed, can_back could be wrong.
        assert!(t.can_back());
    }
}
