//! Linux ELF execution entry point for RaeenOS.
//!
//! This is intentionally minimal: it supports launching static x86_64 Linux
//! binaries (busybox-style) via a dedicated kernel entry point, and routes
//! their syscalls through `linux_syscall::linux_syscall_dispatch`.
#![allow(dead_code)]

extern crate alloc;

use crate::posix::Errno;
use crate::task::{CpuAffinity, Task, TaskId};

/// Launch a Linux ELF from the VFS with the given argv.
pub fn linux_exec(path: &str, argv: &[&str]) -> Result<TaskId, Errno> {
    crate::serial_println!("[linux_exec] exec '{}' argv_len={}", path, argv.len());
    let data = crate::vfs::read_file(path).ok_or(Errno::Enoent)?;
    linux_exec_bytes(&data, argv)
}

/// Launch a Linux ELF that is already in memory (e.g. embedded via
/// `include_bytes!`) with the given argv. Shared by `linux_exec` (VFS path) and
/// the Linux-ABI boot smoketest (embedded probe), so the spawn sequence — ELF
/// origin check, task build, Linux syscall-routing mark, POSIX state, console
/// fds — lives in exactly one place.
pub fn linux_exec_bytes(data: &[u8], argv: &[&str]) -> Result<TaskId, Errno> {
    let origin = crate::elf_loader::detect_elf_origin(data).map_err(|_| Errno::Enoexec)?;
    if origin != crate::elf_loader::ElfOrigin::Linux {
        return Err(Errno::Enoexec);
    }

    // Minimal envp; musl/glibc static binaries are fine with empty environment.
    let envp: [&str; 0] = [];

    let parent = crate::scheduler::current_task_id();
    let pid = parent.map(|t| t.raw()).unwrap_or(0);

    let mut task = Task::new_linux_elf(data, parent, argv, &envp).map_err(|_| Errno::Enoexec)?;

    // Pin to the BSP (CPU0). APs only `loop { hlt }` post-boot (the AP-cores-
    // don't-schedule limitation), so a Linux task left on the default all-cores
    // affinity can be placed on a halted AP and never run a single syscall — the
    // exact symptom the embedded probe hit on real 12-core Athena (0 syscalls
    // dispatched) while running fine on QEMU where it happened to land on CPU0.
    // user_init is pinned to CPU0 for the same reason.
    task.affinity = CpuAffinity::from_mask(1);

    // Mark for syscall routing.
    crate::linux_syscall::mark_task_as_linux(task.id.raw());

    // Ensure basic POSIX state exists for sys_getcwd/sys_getuid/etc.
    {
        let mut table = crate::posix::POSIX_STATE.lock();
        table.insert(
            task.id.raw(),
            crate::posix::PosixProcessState::new(task.id.raw(), pid),
        );
    }

    // Best-effort: attach a console to fd 1/2 so the binary's output is visible.
    crate::posix::install_console_fds(&mut task);

    let tid = task.id;
    crate::scheduler::spawn(task);
    Ok(tid)
}

// ── Linux-ABI boot smoketest ────────────────────────────────────────────────
//
// Concept (RaeenOS_Concept.md — RaeBridge / "Steam day one"): running real
// Windows games through Proton means running real *Linux* binaries, so the
// Linux syscall-translation layer (`linux_syscall.rs`) must actually work, not
// merely build. The ~31 translated syscalls (memory: linux-syscall-oracle-gap-
// filling) were previously only build-verified.
//
// This embeds a tiny static x86_64 Linux probe (`tools/linux_abi_probe`, built
// + validated on the Athena Arch box → it prints PASS there) and spawns it
// through the very same `linux_exec` path RaeBridge/Proton binaries use. The
// probe self-checks getuid/getrandom/sysinfo/uname/statx and prints, to the
// RaeenOS console:
//     [linux-abi-probe] PASS ...        (every checked syscall returned sanely)
//   or
//     [linux-abi-probe] FAIL: <what>    (a translated syscall misbehaved)
// — a genuinely FAIL-able proof that the translation layer is correct on a real
// Linux ELF. The kernel-side marker below reports whether the *spawn* itself
// succeeded; the probe's own line is the runtime verdict.

/// The Athena-built static Linux probe (osabi=GNU, routes to the Linux ABI).
static LINUX_ABI_PROBE: &[u8] = include_bytes!("../../tools/linux_abi_probe/linux_abi_probe.elf");

/// Spawn the embedded Linux-ABI probe. The probe prints its own PASS/FAIL line
/// once the scheduler runs it; here we only report the spawn outcome (a spawn
/// error is itself a FAIL — the Linux exec path is broken).
pub fn run_boot_smoketest() {
    // The probe is BSP-pinned (in linux_exec_bytes) so it actually runs — on a
    // non-BSP core it would starve (APs `loop { hlt }` post-boot). It executes
    // via normal preemption during the post-marker daemon drain.
    match linux_exec_bytes(LINUX_ABI_PROBE, &["linux_abi_probe"]) {
        Ok(tid) => crate::serial_println!(
            "[linux-abi] probe spawned (task {:?}) -> PASS (expect [linux-abi-probe] PASS below)",
            tid,
        ),
        Err(e) => {
            crate::serial_println!("[linux-abi] FAIL: could not spawn Linux-ABI probe: {:?}", e,)
        }
    }

    // File-read smoketest: stock GNU `cat /etc/motd` — proves a real Linux
    // utility can read a bundled file through ld.so + the VFS (openat(O_RDONLY)
    // on an initramfs path → read() → write(stdout)). Simpler than `ls` (no
    // directory/ioctl), so it isolates the file-read path from the directory
    // path. Expected output: `raeen-cat-ok`. (Subsumes the old toy /bin/dh as
    // the ld.so canary; dh stays bundled for manual use.)
    match linux_exec("/bin/cat", &["/bin/cat", "/etc/motd"]) {
        Ok(tid) => crate::serial_println!(
            "[ld.so] stock /bin/cat /etc/motd spawned (task {:?}) -> expect `raeen-cat-ok` below",
            tid,
        ),
        Err(e) => crate::serial_println!("[ld.so] stock /bin/cat spawn FAILED: {:?}", e),
    }

    // Real-app smoketest: a STOCK coreutils binary (GNU `seq`, dynamically
    // linked against the same glibc) — proves the ld.so path generalizes past
    // our own hello-world to an unmodified Linux utility, exercising multi-arg
    // argv, libc stdio + number formatting, and the IFUNC-resolved string ops.
    // `seq 5 8` prints `5 6 7 8` (one per line). Expected output below.
    match linux_exec("/bin/seq", &["/bin/seq", "5", "8"]) {
        Ok(tid) => crate::serial_println!(
            "[ld.so] stock /bin/seq 5 8 spawned (task {:?}) -> expect 5,6,7,8 below",
            tid,
        ),
        Err(e) => crate::serial_println!("[ld.so] stock /bin/seq spawn FAILED: {:?}", e),
    }

    // THREADING smoketest: a real glibc `pthread` binary — proves the full
    // multithreaded-Linux stack (ld.so + libc + clone(CLONE_THREAD) +
    // pthread_mutex/futex + pthread_join via CLONE_CHILD_CLEARTID) end to end on
    // a genuine glibc threading runtime, not just our raw-syscall probe. It
    // spawns 4 threads that each mutex-increment a shared counter 1000×, joins
    // all four, and prints `raeen-pthread-ok` iff the total is exactly 4000.
    // This is the payoff of the threading work: stock multithreaded Linux
    // software (the Proton/Wine class) runs on RaeenOS.
    match linux_exec("/bin/pthreadtest", &["/bin/pthreadtest"]) {
        Ok(tid) => crate::serial_println!(
            "[ld.so] glibc pthread test spawned (task {:?}) -> expect `raeen-pthread-ok` below",
            tid,
        ),
        Err(e) => crate::serial_println!("[ld.so] glibc pthread test spawn FAILED: {:?}", e),
    }
}

/// Convenience helper for a "busybox applet" invocation.
/// Example: `linux_exec_busybox("/system/linux/busybox", "uname")`
pub fn linux_exec_busybox(busybox_path: &str, applet: &str) -> Result<TaskId, Errno> {
    let argv = [busybox_path, applet];
    linux_exec(busybox_path, &argv)
}
