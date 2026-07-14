//! Trash as a **CoW move** — LEGACY_GAMING_CONCEPT.md §AthFS *"instant rollback /
//! per-app data buckets"*. Deleting a file moves it into a `.Trash` bucket under
//! the session home (a real AthFS directory, satisfying the kernel's
//! `is_session_home_path` write gate); the move is a metadata-only CoW rename,
//! so it is instant and **undoable** — Restore is just the reverse rename, and
//! Empty Trash is the only step that actually unlinks bytes.
//!
//! Pure path arithmetic, no syscalls: the app feeds these targets to
//! `SYS_RENAME` / `SYS_UNLINK`.

use crate::Path;

/// The trash bucket name under the session home (e.g. `/home/athena/.Trash`).
/// Dot-prefixed so it does not clutter the home listing (macOS `.Trash`).
pub const TRASH_DIR_NAME: &str = ".Trash";

/// Why a trash/restore computation could not produce a target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrashError {
    /// The resulting path would exceed [`crate::PATH_CAP`].
    PathTooLong,
    /// The source has no file name (e.g. it is the root `/`), so it cannot be
    /// trashed.
    NoFileName,
    /// The item is already inside the trash bucket (refuse to nest).
    AlreadyTrashed,
    /// A restore was requested for a path that is not inside the trash bucket.
    NotInTrash,
}

/// The absolute trash directory for a given session home, e.g.
/// `home = "/home/athena"` → `"/home/athena/.Trash"`. The app `mkdir`s this once
/// (idempotently) before the first delete.
pub fn trash_dir_for_home(home: &str) -> Option<Path> {
    let mut p = Path::from_str(home)?;
    if !p.push(TRASH_DIR_NAME) {
        return None;
    }
    Some(p)
}

/// True iff `path` lives inside the trash bucket for `home`.
pub fn is_in_trash(path: &str, home: &str) -> bool {
    match trash_dir_for_home(home) {
        Some(t) => {
            let td = t.as_str();
            // Must be the dir itself or a child (`<trash>/...`).
            path == td
                || (path.len() > td.len()
                    && path.starts_with(td)
                    && path.as_bytes()[td.len()] == b'/')
        }
        None => false,
    }
}

/// Compute where `src` moves to when trashed: `<home>/.Trash/<file_name>`.
/// Refuses to trash the root, a path with no name, or something already in the
/// trash. Collision handling (a same-named file already trashed) is the app's
/// job — it gets `E_VFS_EXISTS` from `SYS_RENAME` and can disambiguate; this
/// keeps the function deterministic and testable.
pub fn trash_target(src: &str, home: &str) -> Result<Path, TrashError> {
    if is_in_trash(src, home) {
        return Err(TrashError::AlreadyTrashed);
    }
    let src_path = Path::from_str(src).ok_or(TrashError::PathTooLong)?;
    let name = src_path.file_name();
    if name.is_empty() {
        return Err(TrashError::NoFileName);
    }
    let mut dst = trash_dir_for_home(home).ok_or(TrashError::PathTooLong)?;
    if !dst.push(name) {
        return Err(TrashError::PathTooLong);
    }
    Ok(dst)
}

/// Compute where a trashed item restores to: back into `restore_parent` under
/// its own name. `trashed` must be inside the trash bucket. The app remembers
/// the original parent per trashed item (the undo record); when it does not, the
/// home root is a safe default the caller can pass.
pub fn restore_target(trashed: &str, home: &str, restore_parent: &str) -> Result<Path, TrashError> {
    if !is_in_trash(trashed, home) {
        return Err(TrashError::NotInTrash);
    }
    let tp = Path::from_str(trashed).ok_or(TrashError::PathTooLong)?;
    let name = tp.file_name();
    if name.is_empty() {
        return Err(TrashError::NoFileName);
    }
    let mut dst = Path::from_str(restore_parent).ok_or(TrashError::PathTooLong)?;
    if !dst.push(name) {
        return Err(TrashError::PathTooLong);
    }
    Ok(dst)
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOME: &str = "/home/athena";

    #[test]
    fn trash_dir_is_dotted_under_home() {
        let d = trash_dir_for_home(HOME).unwrap();
        assert_eq!(d.as_str(), "/home/athena/.Trash");
    }

    #[test]
    fn trash_target_keeps_file_name() {
        let t = trash_target("/home/athena/Documents/notes.txt", HOME).unwrap();
        assert_eq!(t.as_str(), "/home/athena/.Trash/notes.txt");
    }

    #[test]
    fn trash_then_restore_round_trips() {
        let trashed = trash_target("/home/athena/Pictures/photo.png", HOME).unwrap();
        assert_eq!(trashed.as_str(), "/home/athena/.Trash/photo.png");
        // Restore back to the original parent.
        let restored = restore_target(trashed.as_str(), HOME, "/home/athena/Pictures").unwrap();
        assert_eq!(restored.as_str(), "/home/athena/Pictures/photo.png");
    }

    #[test]
    fn refuses_to_trash_something_already_trashed() {
        assert_eq!(
            trash_target("/home/athena/.Trash/old.txt", HOME),
            Err(TrashError::AlreadyTrashed)
        );
    }

    #[test]
    fn refuses_to_restore_a_non_trash_path() {
        assert_eq!(
            restore_target("/home/athena/Documents/x.txt", HOME, "/home/athena"),
            Err(TrashError::NotInTrash)
        );
    }

    #[test]
    fn is_in_trash_does_not_match_sibling_prefixes() {
        // ".TrashCan" shares the prefix but is NOT the trash bucket — guards the
        // boundary check from a false positive.
        assert!(!is_in_trash("/home/athena/.TrashCan/x", HOME));
        assert!(is_in_trash("/home/athena/.Trash/x", HOME));
    }

    #[test]
    fn fail_able_wrong_target_is_rejected() {
        // FAIL-able guard: if trash_target ever dropped the extension or used the
        // wrong bucket, this would (wrongly) pass — the assert_ne pins it.
        let t = trash_target("/home/athena/a.txt", HOME).unwrap();
        assert_ne!(t.as_str(), "/home/athena/.Trash/a"); // extension MUST be kept
        assert_ne!(t.as_str(), "/home/athena/Trash/a.txt"); // bucket MUST be dotted
    }
}
