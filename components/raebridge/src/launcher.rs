//! RaeBridge per-process launcher — load ONE PE and run it to its own exit.
//!
//! Concept §Compatibility Strategy: "RaeBridge runs Windows apps on day one.
//! Wine + Proton heritage, tightly integrated. Not a 'subsystem' — apps run
//! naturally." This is the shared load path the `raebridge_run` binary calls so
//! that each Windows `.exe` runs as its OWN RaeenOS process. Unlike the in-host
//! harness (where the first guest `ExitProcess` terminates every fixture in one
//! process), here the launcher IS the process: the guest's `ExitProcess` shim
//! calls `sys_exit`, killing only THIS child, and the parent reaps the code via
//! `SYS_WAIT` — the structural "double-click → own process" model.
//!
//! It is buildable now off the hot kernel spawn path: it reuses the existing,
//! verifier-proven `exec::load_pe_executable` (mmap RW → reloc → IAT → mprotect
//! RX → TEB/PEB → set_gs_base) unchanged — that path only touches THIS task's
//! address space, so it is process-agnostic by construction.

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::exec::load_pe_executable;
use crate::handoff::Target;
use crate::syscalls::{sys_debug_print, sys_exit};
use crate::testpe;
use crate::winapi_shims::{arm_real_cpp_milestone, host_context_installed, install_host_context};
use crate::{FullCompatSession, SessionId};

/// Exit code the launcher uses when its own machinery (not the guest) fails.
pub const LAUNCH_ERR_SESSION: u64 = 0xE1;
pub const LAUNCH_ERR_DUP_CTX: u64 = 0xE2;
pub const LAUNCH_ERR_LOAD: u64 = 0xE3;
pub const LAUNCH_ERR_IMPORTS: u64 = 0xE4;

fn log(msg: &str) {
    // SAFETY: SYS_DEBUG_PRINT only reads `len` bytes of the buffer.
    unsafe {
        sys_debug_print(msg.as_bytes());
    }
}

/// Diverging launcher failure: print the reason, then exit ONLY this process so
/// the parent reaps a named non-zero code (the desktop survives — "driver crash
/// ≠ system crash" generalised to guest apps).
fn die(msg: &str, code: u64) -> ! {
    log(msg);
    // SAFETY: sys_exit never returns; terminates only this child process.
    unsafe { sys_exit(code) }
}

/// Resolve a [`Target`] to the PE image bytes and a human label. For the
/// bundled fixtures this hands back the embedded image; the production `Pe`
/// variant reads the bytes from the VFS (the caller passes them in via
/// `pe_bytes` since `launcher` is `no_std` and does not own the syscall read
/// loop — `raebridge_run::_start` does the `SYS_OPEN`/`SYS_READ`).
pub fn target_image(target: &Target, pe_bytes: Option<Vec<u8>>) -> (Vec<u8>, &'static str) {
    match target {
        Target::BundledExit42 => (testpe::build_exit_process_exe(), "bundled:exit42"),
        Target::BundledCpp => (testpe::REAL_MSVC_MT_CPP_EXE.to_vec(), "bundled:cpp"),
        Target::Pe { .. } => (pe_bytes.unwrap_or_default(), "pe"),
    }
}

/// Load + run the PE for `target` to its own `ExitProcess`. This function does
/// NOT return on success — the guest's `ExitProcess` shim calls `sys_exit`
/// terminating this process. It returns only conceptually on the diverging
/// `die` path (also a `sys_exit`). `image` is the PE bytes (from
/// [`target_image`]); `command_line` seeds the session.
///
/// SAFETY: jumps to guest machine code under the Microsoft x64 ABI. The image
/// must be a PE32+ whose entry eventually calls `ExitProcess` (every CRT exe and
/// the bundled fixtures do).
pub unsafe fn run_target(target: &Target, image: Vec<u8>, command_line: &str) -> ! {
    // ── Install the process-global Win32 session (Box'd: a stack session has
    //    double-faulted before — MasterChecklist Phase 11 known issue). ──────
    let session = match FullCompatSession::new(
        SessionId(1),
        String::from(command_line),
        image.clone(),
        String::from(command_line),
    ) {
        Ok(s) => Box::new(s),
        Err(_) => die(
            "[raebridge_run] FATAL: session construction failed\n",
            LAUNCH_ERR_SESSION,
        ),
    };
    if install_host_context(session).is_some() {
        die(
            "[raebridge_run] FATAL: duplicate host context\n",
            LAUNCH_ERR_DUP_CTX,
        );
    }
    if !host_context_installed() {
        die(
            "[raebridge_run] FATAL: host context not installed\n",
            LAUNCH_ERR_DUP_CTX,
        );
    }

    // ── Map + relocate + IAT-patch + W^X flip + TEB/PEB + set_gs_base ───────
    let loaded = match load_pe_executable(&image) {
        Ok(l) => l,
        Err(e) => die(
            &format!("[raebridge_run] FATAL: load: {e:?}\n"),
            LAUNCH_ERR_LOAD,
        ),
    };
    if loaded.stubbed_imports != 0 {
        die(
            &format!(
                "[raebridge_run] FATAL: {} imports UNRESOLVED (would fail-loud before entry)\n",
                loaded.stubbed_imports
            ),
            LAUNCH_ERR_IMPORTS,
        );
    }

    // ── Arm the milestone matching the target so the ExitProcess shim emits
    //    the correct verdict line, then jump the entry. ──────────────────────
    match target {
        Target::BundledCpp => {
            // The real C++ CRT: arm kind=3 so shim_exit_process asserts the
            // static-ctor + main lines and exits 0.
            crate::winapi_shims::enable_stdout_capture();
            arm_real_cpp_milestone();
        }
        Target::BundledExit42 => {
            // The hand-built ExitProcess(42) image: arm the exit-42 SENTINEL so
            // the shim asserts the arriving code IS 42 (the propagation proof) →
            // "[raebridge] guest ExitProcess(42) -> exit 42 PASS". A real `.exe`
            // (Pe) stays unarmed → a neutral "reaped by parent" report instead of
            // a spurious FAIL on its own (correct) exit code.
            crate::winapi_shims::arm_exit42_milestone();
        }
        Target::Pe { .. } => {
            // Production app: run its CRT unarmed; ExitProcess carries the
            // guest's real code straight to sys_exit and the parent reaps it.
        }
    }

    // SAFETY: entry_point is AddressOfEntryPoint of the image we just mapped
    // into executable user pages with a patched IAT, GS base at a live TEB, and
    // W^X-flipped .text. The entry never returns (it ExitProcess'es); `loaded`
    // holds the process_env the GS base points at, so it is forgotten (leaked
    // for this process's lifetime) across the divergent call.
    let entry: unsafe extern "win64" fn() -> ! = core::mem::transmute(loaded.entry_point as usize);
    core::mem::forget(loaded);
    entry()
}
