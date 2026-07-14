//! AthenaOS File Manager logic engine — *"the modern file manager"*
//! (LEGACY_GAMING_CONCEPT.md §Windows Pain Points). The pure, host-testable core
//! behind the bundled Files app: tabs with back/forward history, an **undoable
//! Trash** (delete = a CoW move into a session-home bucket, restorable instantly
//! — AthFS *"instant rollback / per-app data buckets"*), and **batch rename**
//! (a `Name_###` pattern with a counter that preserves each file's extension).
//!
//! ## Why this is its own crate
//! `apps/files` is a `#![no_std] #![no_main]` bin that links athkit's
//! `#[panic_handler]`, so `cargo test` inside it trips the duplicate `panic_impl`
//! lang-item (project memory: the no-std-workspace host-test hazard). All the
//! decision logic lives here as a zero-dep `no_std` lib that toggles to `std`
//! only under `cfg(test)` for the harness (the `ath_tokens` / `ath_calc`
//! pattern), giving FAIL-able proofs: `cargo test -p ath_files`.
//!
//! ## Design contract — never panic
//! Every operation that can fail on bad input (a pattern with no counter, a name
//! collision, an over-long path, an out-of-range tab) returns an explicit
//! `Result`/`Option` so the app surfaces a calm message instead of crashing. No
//! `unwrap`/`expect`/indexing-without-bounds in any path reachable from input.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

mod history;
mod rename;
mod trash;

pub use history::{Tab, TabError, TabSet, MAX_HISTORY, MAX_TABS};
pub use rename::{batch_rename_target, expand_pattern, split_name_ext, RenameError, MAX_NAME};
pub use trash::{
    is_in_trash, restore_target, trash_dir_for_home, trash_target, TrashError, TRASH_DIR_NAME,
};

/// Cap on any single VFS path the app builds (matches the app's `PathBuf`).
pub const PATH_CAP: usize = 256;

/// A fixed-capacity, alloc-free path buffer shared by the trash/history logic so
/// this crate stays `no_std` + zero-dep (no `alloc`). Holds ASCII VFS paths.
#[derive(Clone, Copy)]
pub struct Path {
    buf: [u8; PATH_CAP],
    len: usize,
}

impl Path {
    /// Empty path.
    pub const fn new() -> Self {
        Self {
            buf: [0; PATH_CAP],
            len: 0,
        }
    }

    /// Build from a `&str`, returning `None` if it does not fit (never truncates
    /// silently — a truncated path could point somewhere dangerous).
    pub fn from_str(s: &str) -> Option<Self> {
        let b = s.as_bytes();
        if b.len() > PATH_CAP {
            return None;
        }
        let mut p = Self::new();
        p.buf[..b.len()].copy_from_slice(b);
        p.len = b.len();
        Some(p)
    }

    /// The path as a `&str` (always valid: only ASCII is ever written).
    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.buf[..self.len]).unwrap_or("")
    }

    /// Append `/name`, returning `false` if it would overflow (caller decides
    /// what to do; the buffer is left unchanged on overflow).
    pub fn push(&mut self, name: &str) -> bool {
        let nb = name.as_bytes();
        let sep = if self.len > 0 && self.buf[self.len - 1] != b'/' {
            1
        } else {
            0
        };
        if self.len + sep + nb.len() > PATH_CAP {
            return false;
        }
        if sep == 1 {
            self.buf[self.len] = b'/';
            self.len += 1;
        }
        self.buf[self.len..self.len + nb.len()].copy_from_slice(nb);
        self.len += nb.len();
        true
    }

    /// The final path component (the file/dir name), or `""` for `/` or empty.
    pub fn file_name(&self) -> &str {
        let s = self.as_str();
        match s.rfind('/') {
            Some(i) => &s[i + 1..],
            None => s,
        }
    }

    /// The parent path (everything before the last `/`), or `"/"` at the root.
    pub fn parent(&self) -> &str {
        let s = self.as_str();
        match s.rfind('/') {
            Some(0) => "/",
            Some(i) => &s[..i],
            None => "",
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl Default for Path {
    fn default() -> Self {
        Self::new()
    }
}

impl PartialEq for Path {
    fn eq(&self, other: &Self) -> bool {
        self.as_str() == other.as_str()
    }
}
impl Eq for Path {}

impl core::fmt::Debug for Path {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "Path({:?})", self.as_str())
    }
}
