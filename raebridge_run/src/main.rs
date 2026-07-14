//! `raebridge_run` — the per-process AthBridge launcher (one `.exe`, one process).
//!
//! Concept §Compatibility Strategy: "AthBridge runs Windows apps on day one.
//! Wine + Proton heritage, tightly integrated. Not a 'subsystem' — apps run
//! naturally." Apps run *naturally* only when each Windows `.exe` is its OWN
//! AthenaOS process — Wine's one-loader-process-per-exe model. This binary is
//! that loader process (our `wineloader`): the parent writes a launch target to
//! a well-known VFS path, spawns `raebridge_run`, and reaps its exit code; the
//! launcher reads the target, loads the PE, and runs it to its own
//! `ExitProcess`. A guest `ExitProcess` therefore kills ONLY this child — not
//! the parent, not its siblings — which is the structural leap from the in-host
//! harness (where the first `ExitProcess` terminates every fixture at once).
//!
//! Interim handoff (option (b), `docs/components/raebridge-process-model.md`
//! §2): the target is read from `raebridge::handoff::HANDOFF_PATH` — a no-ABI,
//! no-new-syscall rendezvous, buildable now off the hot kernel spawn path. When
//! `SYS_SPAWN_ARGS=284` lands, the target moves to `argv[1]` and this file
//! reads `argc@[rsp]`/`argv@[rsp+8]` instead.
//!
//! Serial proof line (per launch):
//!   [raebridge_run] launched pid=<self> target=<token> -> running
//! then the guest's own `[raebridge] guest ExitProcess(<c>) -> exit <c>` /
//! `[raebridge] smoketest: ... C++ exe ... PASS`, then `sys_exit(<c>)`.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::format;
use alloc::vec;
use alloc::vec::Vec;

// Pull in raekit for its #[global_allocator]; everything else goes through
// raebridge's own native syscall bindings.
#[allow(unused_imports)]
use raekit;

use raebridge::handoff::{self, Target, HANDOFF_MAX_BYTES, HANDOFF_PATH};
use raebridge::launcher::{run_target, target_image};
use raebridge::syscalls::{sys_close, sys_debug_print, sys_exit, sys_getpid, sys_open, sys_read};

/// Exit code when the launcher cannot read/decode its target (vs a guest exit).
const RUN_ERR_NO_TARGET: u64 = 0xE0;

fn log(msg: &str) {
    // SAFETY: SYS_DEBUG_PRINT only reads `len` bytes of the buffer.
    unsafe {
        sys_debug_print(msg.as_bytes());
    }
}

fn die(msg: &str, code: u64) -> ! {
    log(msg);
    // SAFETY: sys_exit never returns; terminates only this process.
    unsafe { sys_exit(code) }
}

/// Read the per-spawn handoff blob from [`HANDOFF_PATH`]. The parent unlinked +
/// re-wrote this path immediately before spawning us (see
/// `raebridge::handoff::HANDOFF_PATH`), so it holds OUR target. Returns the raw
/// blob, or `None` if the path could not be opened or read empty.
fn read_handoff() -> Option<Vec<u8>> {
    // SAFETY: sys_open reads `path` for its length; flags=0 opens existing.
    let fd = unsafe { sys_open(HANDOFF_PATH, 0) };
    // SYS_OPEN errors are u64::MAX / MAX-N (large sentinels); valid fds small.
    if fd >= 0xFF00_0000_0000_0000 {
        return None;
    }
    let mut buf = vec![0u8; HANDOFF_MAX_BYTES];
    // SAFETY: sys_read writes at most buf.len() bytes into buf.
    let n = unsafe { sys_read(fd, &mut buf) };
    // SAFETY: fd is a valid open fd from sys_open above.
    unsafe {
        sys_close(fd);
    }
    if n == 0 || n >= 0xFF00_0000_0000_0000 {
        return None;
    }
    buf.truncate(n as usize);
    Some(buf)
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    // SAFETY: sys_getpid takes no args and never faults.
    let pid = unsafe { sys_getpid() };

    // ── Learn WHICH PE to run (option (b) no-ABI handoff) ──────────────────
    let blob = match read_handoff() {
        Some(b) => b,
        None => die(
            "[raebridge_run] FATAL: handoff target unreadable (no /run/raebridge/self.target)\n",
            RUN_ERR_NO_TARGET,
        ),
    };
    let target: Target = match handoff::decode(&blob) {
        Some(t) => t,
        None => die(
            "[raebridge_run] FATAL: handoff target malformed\n",
            RUN_ERR_NO_TARGET,
        ),
    };

    // For the production Pe variant, read the PE bytes from the VFS; the bundled
    // fixtures carry their image in-binary.
    let pe_bytes: Option<Vec<u8>> = match &target {
        Target::Pe { path } => match read_pe_file(path) {
            Some(b) => Some(b),
            None => die(
                "[raebridge_run] FATAL: PE target unreadable\n",
                RUN_ERR_NO_TARGET,
            ),
        },
        _ => None,
    };

    let (image, label) = target_image(&target, pe_bytes);
    if image.is_empty() {
        die("[raebridge_run] FATAL: empty PE image\n", RUN_ERR_NO_TARGET);
    }

    log(&format!(
        "[raebridge_run] launched pid={pid} target={label} -> running\n"
    ));

    // SAFETY: `image` is a PE32+ whose entry runs to ExitProcess (the bundled
    // fixtures and every CRT exe do). run_target does not return on success —
    // the guest's ExitProcess shim calls sys_exit, terminating only this child.
    unsafe { run_target(&target, image, label) }
}

/// Read a PE file from the VFS into a heap buffer (production `Pe` target).
fn read_pe_file(path: &[u8]) -> Option<Vec<u8>> {
    // SAFETY: sys_open reads `path` for its length.
    let fd = unsafe { sys_open(path, 0) };
    if fd >= 0xFF00_0000_0000_0000 {
        return None;
    }
    let mut out: Vec<u8> = Vec::new();
    let mut chunk = vec![0u8; 4096];
    loop {
        // SAFETY: sys_read writes at most chunk.len() bytes.
        let n = unsafe { sys_read(fd, &mut chunk) };
        if n >= 0xFF00_0000_0000_0000 {
            // SAFETY: fd valid.
            unsafe {
                sys_close(fd);
            }
            return None;
        }
        if n == 0 {
            break;
        }
        out.extend_from_slice(&chunk[..n as usize]);
        if (n as usize) < chunk.len() {
            break;
        }
    }
    // SAFETY: fd valid.
    unsafe {
        sys_close(fd);
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}
