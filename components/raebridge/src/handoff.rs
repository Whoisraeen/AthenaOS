//! RaeBridge guest-process launch handoff (option (b) — no-ABI VFS rendezvous).
//!
//! Concept §Compatibility Strategy: "RaeBridge runs Windows apps on day one.
//! Wine + Proton heritage, tightly integrated. Not a 'subsystem' — apps run
//! naturally." Apps running *naturally* means each `.exe` is its OWN RaeenOS
//! process (Wine's one-loader-process-per-exe model) — so a guest
//! `ExitProcess` kills only that child and a parent reaps the code, instead of
//! every fixture sharing one host process where the first `ExitProcess`
//! terminates them all.
//!
//! This module is the **interim, no-new-syscall** target-passing channel from
//! `docs/components/raebridge-process-model.md` §2 option (b): the parent
//! writes the per-spawn launch target to a well-known VFS path *before*
//! spawning `raebridge_run`, and `raebridge_run` reads it at startup to learn
//! WHICH PE to load. It is buildable now without touching the hot kernel spawn
//! path (option (a) `SYS_SPAWN_ARGS=284` is the durable replacement, GATED on
//! `scheduler.rs`/`task.rs` cooling).
//!
//! The wire format is deliberately tiny and **pure logic** (no syscalls), so the
//! encode/decode round-trip and the launcher's target resolution are
//! host-KAT'd and FAIL-able off-target before any guest code runs.

use alloc::vec::Vec;

/// The well-known VFS path the parent writes the launch target to *before*
/// spawning `raebridge_run`, and the launcher reads at startup.
///
/// **The write-once gotcha + the fix.** The kernel's RAM-backed home-file open
/// (`kernel/src/vfs.rs::open_path_exact`) returns a *read-only* snapshot for an
/// already-existing file (step 2, the virtual hierarchy) and only *creates a
/// writable* inode on the FIRST open of a not-yet-existing path (step 3,
/// `open_or_create_home_file`) — so a second write to the same home path is a
/// silent no-op. The parent therefore **`SYS_UNLINK`s this path before each
/// write** (97), which removes the File node from `ROOT` so the next open
/// re-creates a fresh writable inode. With that, one well-known path is reused
/// safely across launches with zero kernel change. This matches the spec's
/// option (b) ("a well-known per-child path"); when `SYS_SPAWN_ARGS` (284)
/// lands, the target moves to `argv[1]` and this path retires.
///
/// It lives under the session home (`/home/raeen/`) deliberately: that tree is
/// RAM-writable regardless of disk/safe-mode state (the boot self-test
/// round-trips a file there), so the handoff has zero RaeFS/disk dependency.
/// The kernel-global VFS `ROOT` is shared parent↔child, so the child reads what
/// the parent wrote. The parent serialises (unlink → write → spawn → reap)
/// so the single path is never raced.
pub const HANDOFF_PATH: &[u8] = b"/home/raeen/.rae-launch-target";

/// Maximum handoff blob size (a bound the launcher reads into a fixed buffer).
/// A real PE path is short; the bundled-fixture tokens below are <16 bytes.
pub const HANDOFF_MAX_BYTES: usize = 512;

/// Fixed on-disk record width the parent writes (token + trailing-newline pad).
///
/// The kernel's RAM-backed home-file write (`VfsMemoryFileInode::write_at`)
/// overwrites in place and does NOT truncate, so a shorter second write would
/// leave the first target's tail behind. Writing every record at this constant
/// width (newline-padded) means each overwrite fully covers the previous one;
/// [`decode`] trims the trailing newlines back off. 64 ≥ the longest token and
/// any short fixture path; production PE paths exceeding this use option (a).
pub const HANDOFF_RECORD_WIDTH: usize = 64;

/// Encode a [`Target`] as the fixed-width, newline-padded record the parent
/// writes to [`HANDOFF_PATH`] (see [`HANDOFF_RECORD_WIDTH`]). Returns `None` if
/// the token does not fit the record width (a too-long PE path → use option (a)).
pub fn encode_record(target: &Target) -> Option<Vec<u8>> {
    let token = encode(target);
    if token.len() > HANDOFF_RECORD_WIDTH {
        return None;
    }
    let mut rec = token;
    rec.resize(HANDOFF_RECORD_WIDTH, b'\n');
    Some(rec)
}

/// The launch target `raebridge_run` resolves at startup.
///
/// For the isolation proof we use two **bundled** fixtures with DIFFERENT exit
/// codes (`42 != 0` proves the reaped codes are real, not a fixed sentinel):
/// the hand-built `ExitProcess(42)` image and the genuine MSVC `/MT` C++ exe
/// that exits 0 after its static-ctor + main. The `Pe { path }` variant is the
/// production shape (a real `.exe` on disk) — wired the same way once a guest
/// app is bundled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// Bundled `testpe::build_exit_process_exe()` — `ExitProcess(42)`.
    BundledExit42,
    /// Bundled `testpe::REAL_MSVC_MT_CPP_EXE` — real C++ CRT, exits 0.
    BundledCpp,
    /// A real PE on the VFS at `path` (production double-click path).
    Pe { path: Vec<u8> },
}

/// Token prefix for the on-disk PE form (`pe:/path/to/app.exe`).
const PE_PREFIX: &[u8] = b"pe:";
/// Token for the bundled `ExitProcess(42)` fixture.
const TOKEN_EXIT42: &[u8] = b"bundled:exit42";
/// Token for the bundled C++ fixture.
const TOKEN_CPP: &[u8] = b"bundled:cpp";

/// Encode a [`Target`] into the handoff blob the parent writes to
/// [`HANDOFF_PATH`]. Stable, ASCII, length-bounded — pure logic (KAT'd).
pub fn encode(target: &Target) -> Vec<u8> {
    match target {
        Target::BundledExit42 => TOKEN_EXIT42.to_vec(),
        Target::BundledCpp => TOKEN_CPP.to_vec(),
        Target::Pe { path } => {
            let mut out = Vec::with_capacity(PE_PREFIX.len() + path.len());
            out.extend_from_slice(PE_PREFIX);
            out.extend_from_slice(path);
            out
        }
    }
}

/// Decode the handoff blob the launcher read back from [`HANDOFF_PATH`].
///
/// Returns `None` on an unrecognised, empty, or oversized blob — the launcher
/// then fails loud (exits with a named code) rather than guessing a target.
/// FAIL-demonstrated in the host KATs against a truncated/garbage blob.
pub fn decode(blob: &[u8]) -> Option<Target> {
    // Trim a single trailing newline/NUL the writer or VFS may have appended,
    // so a blob written with a terminator still decodes.
    let mut end = blob.len();
    while end > 0 && (blob[end - 1] == b'\n' || blob[end - 1] == 0 || blob[end - 1] == b'\r') {
        end -= 1;
    }
    let blob = &blob[..end];

    if blob.is_empty() || blob.len() > HANDOFF_MAX_BYTES {
        return None;
    }
    if blob == TOKEN_EXIT42 {
        return Some(Target::BundledExit42);
    }
    if blob == TOKEN_CPP {
        return Some(Target::BundledCpp);
    }
    if blob.len() > PE_PREFIX.len() && &blob[..PE_PREFIX.len()] == PE_PREFIX {
        let path = &blob[PE_PREFIX.len()..];
        if path.is_empty() {
            return None;
        }
        return Some(Target::Pe {
            path: path.to_vec(),
        });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn round_trip_bundled_exit42() {
        let t = Target::BundledExit42;
        assert_eq!(decode(&encode(&t)), Some(t));
    }

    #[test]
    fn round_trip_bundled_cpp() {
        let t = Target::BundledCpp;
        assert_eq!(decode(&encode(&t)), Some(t));
    }

    #[test]
    fn round_trip_pe_path() {
        let t = Target::Pe {
            path: b"/apps/win/notepad.exe".to_vec(),
        };
        assert_eq!(decode(&encode(&t)), Some(t));
    }

    #[test]
    fn distinct_tokens_decode_distinct_targets() {
        // The whole point of the isolation proof: the two launches must resolve
        // to DIFFERENT fixtures so their exit codes differ (42 vs 0).
        let a = decode(&encode(&Target::BundledExit42)).unwrap();
        let b = decode(&encode(&Target::BundledCpp)).unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn tolerates_trailing_newline_and_nul() {
        let mut blob = encode(&Target::BundledCpp);
        blob.push(b'\n');
        blob.push(0);
        assert_eq!(decode(&blob), Some(Target::BundledCpp));
    }

    #[test]
    fn fail_empty_blob() {
        assert_eq!(decode(b""), None);
        assert_eq!(decode(b"\n\n"), None);
    }

    #[test]
    fn fail_garbage_blob() {
        assert_eq!(decode(b"not-a-target"), None);
        assert_eq!(decode(b"bundled:"), None);
        assert_eq!(decode(b"pe:"), None); // prefix with empty path
    }

    #[test]
    fn fail_truncated_token() {
        // A truncated bundled token must NOT silently match the full one.
        assert_eq!(decode(b"bundled:exit4"), None);
        assert_eq!(decode(b"bundled:cp"), None);
    }

    #[test]
    fn fail_oversized_blob() {
        let big = vec![b'A'; HANDOFF_MAX_BYTES + 1];
        assert_eq!(decode(&big), None);
    }

    #[test]
    fn record_round_trips_at_fixed_width() {
        for t in [Target::BundledExit42, Target::BundledCpp] {
            let rec = encode_record(&t).unwrap();
            assert_eq!(rec.len(), HANDOFF_RECORD_WIDTH);
            assert_eq!(decode(&rec), Some(t));
        }
    }

    #[test]
    fn record_overwrite_leaves_no_stale_tail() {
        // The bug this guards: a shorter second write over a longer first one.
        // exit42 token (14 bytes) is LONGER than cpp (11 bytes); a non-truncating
        // VFS write of the cpp record over the exit42 record must still decode as
        // cpp, never as exit42 (no leftover "42" tail). Fixed-width padding makes
        // the second record fully cover the first.
        let first = encode_record(&Target::BundledExit42).unwrap();
        let second = encode_record(&Target::BundledCpp).unwrap();
        assert_eq!(first.len(), second.len());
        // Simulate the in-place overwrite: copy `second` over `first`'s bytes.
        let mut storage = first.clone();
        storage[..second.len()].copy_from_slice(&second);
        assert_eq!(decode(&storage), Some(Target::BundledCpp));
    }
}
