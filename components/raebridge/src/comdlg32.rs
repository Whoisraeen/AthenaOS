//! comdlg32.dll — the Windows common dialogs (File Open/Save, …).
//!
//! Concept §Compatibility Strategy ("apps run naturally"): real Windows apps
//! reach File→Open / File→Save through `GetOpenFileNameW` / `GetSaveFileNameW`,
//! which normally pop an interactive file picker. RaeBridge's compat path has no
//! interactive picker, so these deterministically *confirm* the path the app
//! pre-filled into `OPENFILENAMEW.lpstrFile` (apps seed it with a default file
//! name) — the same observable behavior as a scripted / automated Windows
//! session — returning TRUE with a usable absolute path under the app's virtual
//! `C:\` (which maps to the per-app bucket). This is real path resolution, not a
//! no-op: the struct is marshaled and the result buffer is written by the shim.
//!
//! The path-decision logic here is pure (no guest pointers) and host-KAT'able;
//! the guest-pointer marshaling of `OPENFILENAMEW` lives in `winapi_shims`.

use alloc::string::String;
use alloc::string::ToString;

/// Decide the path a headless Open/Save dialog confirms, given the caller's
/// pre-set `lpstrFile` contents `current`:
/// - a full/anchored path (contains `\\`, `/`, or a drive `:`) is honored as-is;
/// - a bare file name is anchored under the app's `C:\` root;
/// - an empty default falls back to a fixed `C:\untitled.txt` so the flow always
///   completes with a writable path.
///
/// Pure + FAIL-able: the anchoring and the empty-default fallback are exactly
/// the cases a save flow depends on, and each is asserted without any pointers.
pub fn resolve_dialog_path(current: &str) -> String {
    let trimmed = current.trim();
    if trimmed.is_empty() {
        return String::from("C:\\untitled.txt");
    }
    if trimmed.contains('\\') || trimmed.contains('/') || trimmed.contains(':') {
        // Already a path (absolute, relative, or drive-qualified): honor it.
        trimmed.to_string()
    } else {
        // A bare file name → anchor it under the app's C:\ root.
        let mut p = String::from("C:\\");
        p.push_str(trimmed);
        p
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_dialog_path_anchors_bare_name() {
        assert_eq!(resolve_dialog_path("notes.txt"), "C:\\notes.txt");
    }

    #[test]
    fn resolve_dialog_path_honors_full_path() {
        assert_eq!(
            resolve_dialog_path("C:\\Users\\me\\doc.txt"),
            "C:\\Users\\me\\doc.txt"
        );
        // A drive-qualified or forward-slash path is left intact too.
        assert_eq!(resolve_dialog_path("D:\\x.bin"), "D:\\x.bin");
        assert_eq!(resolve_dialog_path("sub/dir/f.txt"), "sub/dir/f.txt");
    }

    #[test]
    fn resolve_dialog_path_empty_falls_back() {
        assert_eq!(resolve_dialog_path(""), "C:\\untitled.txt");
        assert_eq!(resolve_dialog_path("   "), "C:\\untitled.txt");
    }

    #[test]
    fn resolve_dialog_path_trims_surrounding_space() {
        assert_eq!(resolve_dialog_path("  report.doc  "), "C:\\report.doc");
    }
}
