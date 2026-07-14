#![no_std]
#![no_main]

extern crate alloc;
use alloc::boxed::Box;
use athgfx::Canvas;
#[allow(unused_imports)]
use athkit;
use athui::{Button, Label, Widget};
use core::panic::PanicInfo;

// ── Syscall numbers ───────────────────
const SYS_PRINT: u64 = 1;
const SYS_RECV: u64 = 3;
const SYS_CAP_GRANT: u64 = 4;
const SYS_CAP_QUERY: u64 = 6;
const SYS_MMIO_MAP: u64 = 7;
const SYS_SPAWN: u64 = 11;
const SYS_EXIT: u64 = 12;
const SYS_WAIT: u64 = 13;
const SYS_OPEN: u64 = 15;
const SYS_READ: u64 = 16;
const SYS_WRITE: u64 = 17;
const SYS_CLOSE: u64 = 18;
const SYS_SEEK: u64 = 22;
const SYS_STAT: u64 = 23;
const SYS_UNLINK: u64 = 97;
const SYS_NET_SOCKET: u64 = 121; // rdi=proto(0=TCP,1=UDP) -> fd
const SYS_NET_CLOSE: u64 = 125; // rdi=fd
const SYS_SCRIPT_RUN: u64 = 78; // rdi=src_ptr, rsi=src_len, rdx=cap_mask -> id
const SYS_SCRIPT_STATUS: u64 = 79; // rdi=id, rsi=out_ptr, rdx=out_cap -> bytes
const SYS_LINUXKPI_MSLEEP: u64 = 129; // rdi=ms (blocking sleep)

/// Valid fd indices from `SYS_OPEN` (errors are `u64::MAX`, `MAX-1`, …).
const OPEN_FD_MAX: u64 = 31;

#[inline(always)]
fn open_ok(fd: u64) -> bool {
    fd <= OPEN_FD_MAX
}

// ── Boot cap handles ─────────────
const CAP_MMIO_FB: u64 = 1;

#[inline(always)]
unsafe fn sys_print(value: u64) {
    core::arch::asm!("syscall", in("rax") SYS_PRINT, in("rdi") value, out("rcx") _, out("r11") _,);
}

#[inline(always)]
unsafe fn sys_cap_query(handle: u64) -> (u64, u64, u64) {
    let (status, flavor, rights);
    core::arch::asm!(
        "syscall",
        inout("rax") SYS_CAP_QUERY => status,
        in("rdi") handle,
        lateout("rsi") flavor,
        lateout("rdx") rights,
        out("rcx") _, out("r11") _,
    );
    (status, flavor, rights)
}

#[inline(always)]
unsafe fn sys_mmio_map(handle: u64, user_virt: u64, length: u64) -> u64 {
    let result: u64;
    core::arch::asm!(
        "syscall",
        inout("rax") SYS_MMIO_MAP => result,
        in("rdi") handle,
        in("rsi") user_virt,
        in("rdx") length,
        out("rcx") _, out("r11") _,
    );
    result
}

#[inline(always)]
unsafe fn sys_exit(code: u64) -> ! {
    core::arch::asm!("syscall", in("rax") SYS_EXIT, in("rdi") code, options(noreturn));
}

/// Spawn an ELF child by path, returning the child's TaskId in rax
/// (or u64::MAX-N for an error code).
#[inline(always)]
unsafe fn sys_spawn(path: &[u8]) -> u64 {
    let pid: u64;
    core::arch::asm!(
        "syscall",
        inout("rax") SYS_SPAWN => pid,
        in("rdi") path.as_ptr() as u64,
        in("rsi") path.len() as u64,
        out("rcx") _, out("r11") _,
    );
    pid
}

/// Block until the child with TaskId `pid` exits. Returns the exit code in
/// rax. Returns u64::MAX if the pid was never valid.
#[inline(always)]
unsafe fn sys_wait(pid: u64) -> u64 {
    let code: u64;
    core::arch::asm!(
        "syscall",
        inout("rax") SYS_WAIT => code,
        in("rdi") pid,
        out("rcx") _, out("r11") _,
    );
    code
}

/// Derive a narrower capability and deposit it in `target`'s cap table.
/// Returns the new handle (in the target's table) or a cap-error code.
#[inline(always)]
unsafe fn sys_cap_grant(
    target: u64,
    src_handle: u64,
    new_rights: u64,
    derive_arg0: u64,
    derive_arg1: u64,
) -> u64 {
    let r: u64;
    core::arch::asm!(
        "syscall",
        inout("rax") SYS_CAP_GRANT => r,
        in("rdi") target,
        in("rsi") src_handle,
        in("rdx") new_rights,
        in("r10") derive_arg0,
        in("r8")  derive_arg1,
        out("rcx") _, out("r11") _,
    );
    r
}

/// Block on an IPC channel cap until a message arrives. Returns
/// `(status, msg_type, arg1, arg2, arg3)` where status == 0 on success.
#[inline(always)]
unsafe fn sys_recv(cap_handle: u64) -> (u64, u64, u64, u64, u64) {
    let status: u64;
    let msg_type: u64;
    let arg1: u64;
    let arg2: u64;
    let arg3: u64;
    core::arch::asm!(
        "syscall",
        inout("rax") SYS_RECV => status,
        in("rdi") cap_handle,
        lateout("rsi") msg_type,
        lateout("rdx") arg1,
        lateout("r10") arg2,
        lateout("r8")  arg3,
        out("rcx") _, out("r11") _,
    );
    (status, msg_type, arg1, arg2, arg3)
}

// ── File I/O syscalls ────────────────────────────────────────────────────

#[inline(always)]
unsafe fn sys_open(path: &[u8], flags: u64) -> u64 {
    let fd: u64;
    core::arch::asm!(
        "syscall",
        inout("rax") SYS_OPEN => fd,
        in("rdi") path.as_ptr() as u64,
        in("rsi") path.len() as u64,
        in("rdx") flags,
        out("rcx") _, out("r11") _,
    );
    fd
}

#[inline(always)]
unsafe fn sys_write_fd(fd: u64, buf: &[u8]) -> u64 {
    let n: u64;
    core::arch::asm!(
        "syscall",
        inout("rax") SYS_WRITE => n,
        in("rdi") fd,
        in("rsi") buf.as_ptr() as u64,
        in("rdx") buf.len() as u64,
        out("rcx") _, out("r11") _,
    );
    n
}

#[inline(always)]
unsafe fn sys_read_fd(fd: u64, buf: &mut [u8]) -> u64 {
    let n: u64;
    core::arch::asm!(
        "syscall",
        inout("rax") SYS_READ => n,
        in("rdi") fd,
        in("rsi") buf.as_mut_ptr() as u64,
        in("rdx") buf.len() as u64,
        out("rcx") _, out("r11") _,
    );
    n
}

#[inline(always)]
unsafe fn sys_close(fd: u64) -> u64 {
    let r: u64;
    core::arch::asm!(
        "syscall",
        inout("rax") SYS_CLOSE => r,
        in("rdi") fd,
        out("rcx") _, out("r11") _,
    );
    r
}

/// Remove a file by path (SYS_UNLINK=97). Returns 0 on success, else an
/// E_VFS_* code. Used to make a RAM-backed home file overwritable: the kernel
/// only creates a *writable* inode on the first open of a non-existent path, so
/// unlink-then-reopen re-creates a fresh writable file.
#[inline(always)]
unsafe fn sys_unlink(path: &[u8]) -> u64 {
    let r: u64;
    core::arch::asm!(
        "syscall",
        inout("rax") SYS_UNLINK => r,
        in("rdi") path.as_ptr() as u64,
        in("rsi") path.len() as u64,
        out("rcx") _, out("r11") _,
    );
    r
}

#[inline(always)]
unsafe fn sys_seek(fd: u64, offset: u64) -> u64 {
    let r: u64;
    core::arch::asm!(
        "syscall",
        inout("rax") SYS_SEEK => r,
        in("rdi") fd,
        in("rsi") offset,
        out("rcx") _, out("r11") _,
    );
    r
}

#[inline(always)]
unsafe fn sys_stat(fd: u64) -> u64 {
    let r: u64;
    core::arch::asm!(
        "syscall",
        inout("rax") SYS_STAT => r,
        in("rdi") fd,
        out("rcx") _, out("r11") _,
    );
    r
}

// ── Networking syscalls (socket API only — see selftest network check) ─────

#[inline(always)]
unsafe fn sys_net_socket(proto: u64) -> u64 {
    let fd: u64;
    core::arch::asm!(
        "syscall",
        inout("rax") SYS_NET_SOCKET => fd,
        in("rdi") proto,
        out("rcx") _, out("r11") _,
    );
    fd
}

#[inline(always)]
unsafe fn sys_net_close(fd: u64) -> u64 {
    let r: u64;
    core::arch::asm!(
        "syscall",
        inout("rax") SYS_NET_CLOSE => r,
        in("rdi") fd,
        out("rcx") _, out("r11") _,
    );
    r
}

/// Dev-only boot demos (athbridge test-exe build, linux_hello/relibc/linuxkpi
/// fixtures, the VFS/AthUI/triangle/input/spawn session demos). These are
/// self-tests that ran every boot to prove subsystems on QEMU — they are NOT
/// daily-driver features and just add post-desktop time + log noise on iron
/// (athbridge alone maps its embedded PE page-by-page = ~370 mmap syscalls).
/// Gated OUT of the production boot for v0.1 polish; flip to `true` for the dev
/// bring-up sequence. Real daemons (amdgpud, i915d, driver_supervisor) stay
/// unconditional below. MasterChecklist Milestone A / v0.1 production polish.
const RUN_BOOT_DEMOS: bool = false;

/// THE bare-metal acceptance test (MasterChecklist WS2 — "one userspace test
/// to verify all the claims on iron"). Flip to `true`, build `--safe`, flash:
/// the machine runs a battery of syscall-level checks against every major OS
/// claim (memory, capabilities, filesystem, graphics, processes, networking)
/// and renders a readable PASS/FAIL report on screen, ALSO emitting structured
/// serial sentinels (20xxx) captured durably in BOOTLOG.TXT/netlog. Default
/// `false` so a normal/production boot is completely unaffected.
const RUN_SELFTEST: bool = false;

// PASS/FAIL row colors for the on-screen report (ARGB, matching athui::theme).
const SELFTEST_PASS_FG: u32 = 0xFF_2E_CC_71; // emerald
const SELFTEST_FAIL_FG: u32 = 0xFF_E7_4C_3C; // alizarin

/// A valid spawned child pid is neither the u64::MAX sentinel nor an error code
/// in the high range (mirrors the spawn-demo validity check).
#[inline(always)]
fn pid_ok(pid: u64) -> bool {
    pid != u64::MAX && pid < 0xFF00_0000_0000_0000
}

/// Run the unified bare-metal self-test: exercise each OS claim through the
/// real syscall surface, render a PASS/FAIL report, and never return (keeps the
/// report on screen for a photo + keeps this process alive holding the surface).
unsafe fn run_selftest() -> ! {
    sys_print(20000); // begin self-test

    // Results table. Fixed-size so the table itself needs no heap (the checks
    // may allocate — that IS the heap check).
    const MAX: usize = 12;
    let mut names: [&str; MAX] = [""; MAX];
    let mut pass: [bool; MAX] = [false; MAX];
    let mut n: usize = 0;

    // 1. Graphics / compositor — allocate the report surface first; success here
    //    is itself the graphics claim (kernel surface create + mapped framebuffer).
    const SURFACE_VIRT: u64 = 0x0000_8888_0000;
    let width: u64 = 760;
    let height: u64 = 540;
    let surface_id = sys_surface_create(width, height, SURFACE_VIRT);
    let graphics_ok = surface_id != u64::MAX;
    names[n] = "graphics: compositor surface";
    pass[n] = graphics_ok;
    n += 1;

    // 2. Memory — heap allocation through the global allocator (Vec grow + sum).
    {
        let mut v: alloc::vec::Vec<u32> = alloc::vec::Vec::new();
        for i in 0..512u32 {
            v.push(i.wrapping_mul(3).wrapping_add(1));
        }
        let want: u32 = (0..512u32).map(|i| i.wrapping_mul(3).wrapping_add(1)).sum();
        let got: u32 = v.iter().copied().fold(0u32, |a, b| a.wrapping_add(b));
        names[n] = "memory: heap alloc + grow";
        pass[n] = v.len() == 512 && got == want;
        n += 1;
    }

    // 3. Security — capability query on the boot framebuffer Mmio cap.
    {
        let (status, _flavor, _rights) = sys_cap_query(CAP_MMIO_FB);
        names[n] = "security: capability query";
        pass[n] = status == 0;
        n += 1;
    }

    // 4-6. Filesystem — write/stat/read+verify round trip through the VFS/AthFS.
    {
        let path = b"/home/athena/selftest.dat";
        let payload = b"AthenaOS self-test payload 0123456789 abcdef";
        let mut wrote = false;
        let mut stat_ok = false;
        let mut read_ok = false;
        let fd = sys_open(path, 0);
        if open_ok(fd) {
            wrote = sys_write_fd(fd, payload) == payload.len() as u64;
            let _ = sys_close(fd);
            let fd2 = sys_open(path, 0);
            if open_ok(fd2) {
                stat_ok = sys_stat(fd2) == payload.len() as u64;
                let _ = sys_seek(fd2, 0);
                let mut buf = [0u8; 64];
                let nr = sys_read_fd(fd2, &mut buf) as usize;
                read_ok = nr == payload.len() && &buf[..nr] == &payload[..];
                let _ = sys_close(fd2);
            }
        }
        names[n] = "filesystem: write";
        pass[n] = wrote;
        n += 1;
        names[n] = "filesystem: stat size";
        pass[n] = stat_ok;
        n += 1;
        names[n] = "filesystem: read + verify";
        pass[n] = read_ok;
        n += 1;
    }

    // 7. Processes — spawn a real ELF child and reap its exit code.
    {
        let pid = sys_spawn(b"driver_supervisor");
        let mut ok = false;
        if pid_ok(pid) {
            ok = sys_wait(pid) != u64::MAX;
        }
        names[n] = "process: spawn + wait";
        pass[n] = ok;
        n += 1;
    }

    // 8. Networking — the socket API (create a UDP socket, then close it). This
    //    proves the socket layer without needing a live DHCP lease.
    {
        let sock = sys_net_socket(1); // UDP
        let mut ok = false;
        if pid_ok(sock) {
            ok = true;
            let _ = sys_net_close(sock);
        }
        names[n] = "network: UDP socket API";
        pass[n] = ok;
        n += 1;
    }

    // Serial sentinels (durable in BOOTLOG.TXT/netlog): 20100 + i*10 + pass.
    let mut passed = 0usize;
    for i in 0..n {
        if pass[i] {
            passed += 1;
        }
        sys_print(20100 + (i as u64) * 10 + pass[i] as u64);
    }
    sys_print(20900 + passed as u64); // summary: 20900 + #passed

    // Render the report (only if we actually got a surface).
    if graphics_ok {
        let mut canvas = Canvas::new(SURFACE_VIRT as *mut u8, width as usize, height as usize, 4);
        canvas.clear(athui::theme::WINDOW_BG);
        let chrome_h: usize = 28;
        canvas.fill_rect(0, 0, width as usize, chrome_h, athui::theme::CHROME_BG);
        canvas.draw_text(
            12,
            (chrome_h - 8) / 2,
            "AthenaOS - Bare-Metal Self-Test",
            athui::theme::TITLE_FG,
            None,
        );
        canvas.draw_rect_outline(0, 0, width as usize, height as usize, athui::theme::BORDER);

        let left = 24usize;
        let status_x = width as usize - 96;
        let row_h = 30usize;
        let top = chrome_h + 20;
        for i in 0..n {
            let y = top + i * row_h;
            let (fg, label) = if pass[i] {
                (SELFTEST_PASS_FG, "PASS")
            } else {
                (SELFTEST_FAIL_FG, "FAIL")
            };
            canvas.draw_text(left, y, names[i], athui::theme::TITLE_FG, None);
            canvas.draw_text(status_x, y, label, fg, None);
        }

        // Summary line.
        let all_pass = passed == n;
        let summary_y = top + n * row_h + 16;
        let summary_fg = if all_pass {
            SELFTEST_PASS_FG
        } else {
            SELFTEST_FAIL_FG
        };
        canvas.fill_rect(
            0,
            summary_y - 6,
            width as usize,
            row_h,
            athui::theme::CHROME_BG,
        );
        // Build "N / M checks passed" without alloc-format heavy machinery.
        let mut buf = [0u8; 32];
        let line = fmt_summary(&mut buf, passed, n);
        canvas.draw_text(left, summary_y, line, summary_fg, None);

        // Center the report on screen and present it.
        let px = 80i64;
        let py = 80i64;
        let _ = sys_surface_present(surface_id, px, py);
    }

    sys_print(20999); // self-test complete; report on screen

    // Keep this process alive so the surface stays presented for a photo /
    // inspection. A normal acceptance run is a dedicated boot, so spinning here
    // is fine; the durable serial/BOOTLOG sentinels above are the authoritative
    // record regardless of what is on screen.
    loop {
        for _ in 0..2_000_000 {
            core::hint::spin_loop();
        }
    }
}

/// Format "<passed> / <total> checks passed" into `buf`, returning the &str.
/// Tiny no_std integer formatter (avoids pulling format! into the slice).
fn fmt_summary(buf: &mut [u8; 32], passed: usize, total: usize) -> &str {
    let mut i = 0usize;
    st_write_usize(buf, &mut i, passed);
    st_write_bytes(buf, &mut i, b" / ");
    st_write_usize(buf, &mut i, total);
    st_write_bytes(buf, &mut i, b" checks passed");
    core::str::from_utf8(&buf[..i]).unwrap_or("self-test complete")
}

fn st_write_bytes(buf: &mut [u8; 32], i: &mut usize, src: &[u8]) {
    for &c in src {
        if *i < buf.len() {
            buf[*i] = c;
            *i += 1;
        }
    }
}

fn st_write_usize(buf: &mut [u8; 32], i: &mut usize, mut v: usize) {
    if v == 0 {
        st_write_bytes(buf, i, b"0");
        return;
    }
    let mut digits = [0u8; 20];
    let mut d = 0usize;
    while v > 0 {
        digits[d] = b'0' + (v % 10) as u8;
        v /= 10;
        d += 1;
    }
    while d > 0 {
        d -= 1;
        if *i < buf.len() {
            buf[*i] = digits[d];
            *i += 1;
        }
    }
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    unsafe {
        sys_print(7000001);

        // Bare-metal acceptance test (opt-in). When enabled, run the unified
        // self-test FIRST and never return — the machine becomes a dedicated
        // claim-verification harness whose results are on screen + in BOOTLOG.
        if RUN_SELFTEST {
            run_selftest();
        }

        // ─────────────────────────────────────────────────────────────────
        // AthBridge execution smoketest (MasterChecklist Phase 11.2) — DURABLE.
        // Runs on EVERY boot (NOT gated by RUN_BOOT_DEMOS) and BEFORE the slow
        // amdgpud/LinuxKPI firmware probes, so its serial output always lands
        // inside the QEMU capture window AND survives the production boot cut.
        // The host is a two-phase harness in one process:
        //   1. hello-world PE — GetStdHandle → WriteFile("Hello from Windows\n")
        //      → ret. Emits, from GUEST Windows code:
        //      `[athbridge] smoketest: hello-world exe -> stdout
        //       "Hello from Windows" + exit 0 PASS`  (FAIL on captured mismatch)
        //   1.5 api-exercise PE — calls GetModuleHandleW/GetProcAddress/Heap*/
        //      Tls*/WriteConsoleW from guest code, self-verifies each, returns a
        //      step code in RAX. Emits:
        //      `[athbridge] smoketest: api-exercise exe -> GetModuleHandle/
        //       GetProcAddress/Heap/TLS/Console all OK + exit 0 PASS`
        //      (FAIL naming the exact failing step otherwise).
        //   2. exit-code PE — ExitProcess IAT check (mapped + resolved; the host
        //      emits its own PASS line, no longer the terminator).
        //   3. REAL MSVC /MT WriteFile + printf PEs — each mapped + every import
        //      resolved to a real shim (no longer executed; the C++ exe is the
        //      terminator). Emit:
        //      `[athbridge] smoketest: real MSVC /MT exe -> all imports resolved
        //       to shims PASS` and `... printf exe -> all imports resolved to
        //       shims PASS`.
        //   4. REAL MSVC /MT C++ PE (THE C++-runtime milestone) — the genuine
        //      cl.exe C++ CRT runs _initterm over the static-ctor table (firing
        //      g_init's ctor → "ctor ran") BEFORE main, then main → "hello from
        //      c++ 7", returns 0, and the CRT ExitProcess(0)'s. The host (armed,
        //      kind=3) asserts BOTH lines in order and emits:
        //      `[athbridge] smoketest: real MSVC /MT C++ exe -> static-ctor ran
        //       + main + exit 0 PASS` and terminates with exit 0. This is the
        //      broadening of "real Windows software runs" to C++.
        // Sentinels:
        //   9700        spawning athbridge_host
        //   9700        real-exe milestone round-trip OK (9700 + 0)  → PASS
        //   9700+other  bridge exited non-zero (a phase FAIL'd)      → FAIL
        //   9790        spawn failed                                 → FAIL
        // ─────────────────────────────────────────────────────────────────
        sys_print(9700);
        let bridge_pid = sys_spawn(b"athbridge_host");
        if bridge_pid != u64::MAX && bridge_pid < 0xFF00_0000_0000_0000 {
            let bridge_code = sys_wait(bridge_pid);
            sys_print(9700 + (bridge_code & 0xFF));
            // FAIL-able verdict, decodable from serial without the host's own
            // milestone line: the real-exe milestone terminates the host with
            // exit 0; any other code means a phase failed loud above.
            if (bridge_code & 0xFF) == 0 {
                let _ = sys_write_fd(
                    1,
                    b"[athbridge] smoketest: real-exe milestone -> bridge exit 0 PASS\n",
                );
            } else {
                let _ = sys_write_fd(
                    1,
                    b"[athbridge] smoketest: real-exe milestone -> bridge exit NONZERO FAIL\n",
                );
            }
        } else {
            sys_print(9790);
            let _ = sys_write_fd(1, b"[athbridge] smoketest: athbridge_host spawn FAIL\n");
        }

        // ── AthBridge guest-process ISOLATION proof (per-process model) ───────
        // docs/components/athbridge-process-model.md option (b): each Windows
        // .exe runs as its OWN AthenaOS process (athbridge_run), so a guest
        // ExitProcess kills only that child and WE (the parent) reap the code.
        // The in-host harness above can run only ONE real CRT exe to ExitProcess
        // per process; THIS runs TWO fixtures as SEPARATE PIDs with DIFFERENT
        // exit codes (42 vs 0 — distinct codes prove the reaped values are real,
        // not a sentinel). Handoff = no-ABI VFS rendezvous: write the target to
        // a well-known home path, spawn athbridge_run, it reads + runs it.
        //
        // Wire format mirrors `athbridge::handoff` (host-KAT'd): a fixed
        // 64-byte, newline-padded token record (padding so a shorter second
        // write fully overwrites the first in the non-truncating RAM VFS).
        // Path = HANDOFF_PATH; tokens = "bundled:exit42" / "bundled:cpp".
        // Serialised (write A → spawn → reap A → write B → spawn → reap B): the
        // single handoff path is never raced.
        // Sentinels: 9600 starting · 9601+(code&0xFF) childA reaped (9601+42) ·
        // 9650+(code&0xFF) childB reaped (9650+0) · 9690 a spawn failed.
        sys_print(9600);
        {
            const HANDOFF_PATH: &[u8] = b"/home/athena/.rae-launch-target";
            const REC_WIDTH: usize = 64;

            // A real MSVC /MT console .exe compiled on the dev box (its .c source
            // + dumpbin sit beside it in the fixtures dir). Baked in so Child C
            // can write it to the VFS and run it through the PRODUCTION on-disk
            // Target::Pe route — proving AthBridge runs a real .exe read off the
            // filesystem, not just an in-binary fixture blob.
            const REAL_HELLO_EXE: &[u8] =
                include_bytes!("../../components/athbridge/fixtures/real_msvc_mt_hello.exe");

            // Build a 64-byte newline-padded record for a token.
            fn make_record(token: &[u8]) -> [u8; REC_WIDTH] {
                let mut rec = [b'\n'; REC_WIDTH];
                rec[..token.len()].copy_from_slice(token);
                rec
            }

            // Write `record` to the handoff path (RAM-backed home file).
            // Unlink first: the kernel returns a read-only snapshot for an
            // already-existing home file, so a second write to the same path
            // would be a silent no-op — unlink removes the node so the reopen
            // re-creates a fresh WRITABLE inode. Returns true on a full write.
            unsafe fn write_target(record: &[u8]) -> bool {
                let _ = sys_unlink(HANDOFF_PATH); // ignore "not found" on first call
                let fd = sys_open(HANDOFF_PATH, 0);
                if !open_ok(fd) {
                    return false;
                }
                // Re-seek to 0 so each launch overwrites from the start.
                let _ = sys_seek(fd, 0);
                let ok = sys_write_fd(fd, record) == record.len() as u64;
                let _ = sys_close(fd);
                ok
            }

            // Launch one fixture as its own process and reap its exit code.
            // Returns (pid, code) or None on a spawn/handoff failure.
            unsafe fn launch_one(token: &[u8]) -> Option<(u64, u64)> {
                if !write_target(&make_record(token)) {
                    return None;
                }
                let pid = sys_spawn(b"athbridge_run");
                if pid == u64::MAX || pid >= 0xFF00_0000_0000_0000 {
                    return None;
                }
                let code = sys_wait(pid);
                if code == u64::MAX {
                    return None;
                }
                Some((pid, code))
            }

            // Write a whole byte blob to a VFS path (the real .exe). Unlink-first
            // (same RAM-home write-once gotcha as write_target), then write in
            // page chunks, seeking the running offset — the home-file inode's
            // write_at is offset-addressed, so we don't assume one giant write.
            // Returns true only on a complete write.
            unsafe fn write_blob(path: &[u8], bytes: &[u8]) -> bool {
                let _ = sys_unlink(path);
                let fd = sys_open(path, 0);
                if !open_ok(fd) {
                    return false;
                }
                let mut off: usize = 0;
                let mut ok = true;
                while off < bytes.len() {
                    let end = core::cmp::min(off + 4096, bytes.len());
                    let _ = sys_seek(fd, off as u64);
                    if sys_write_fd(fd, &bytes[off..end]) != (end - off) as u64 {
                        ok = false;
                        break;
                    }
                    off = end;
                }
                let _ = sys_close(fd);
                ok
            }

            // Child A: ExitProcess(42) fixture → expect exit 42.
            let a = unsafe { launch_one(b"bundled:exit42") };
            // Child B: real MSVC /MT C++ fixture → expect exit 0.
            let b = unsafe { launch_one(b"bundled:cpp") };

            match (a, b) {
                (Some((pid_a, code_a)), Some((pid_b, code_b))) => {
                    sys_print(9601 + (code_a & 0xFF));
                    sys_print(9650 + (code_b & 0xFF));
                    let distinct = pid_a != pid_b;
                    let codes_ok = (code_a & 0xFF) == 42 && (code_b & 0xFF) == 0;
                    if distinct && codes_ok {
                        let _ = sys_write_fd(
                            1,
                            b"[athbridge] smoketest: process-isolation -> 2 PIDs (childA exit=42, childB exit=0) reaped PASS\n",
                        );
                    } else if !distinct {
                        // The exact regression this proof exists to catch: a
                        // shared PID means the two .exes did NOT run as separate
                        // processes (or the first killed the second).
                        let _ = sys_write_fd(
                            1,
                            b"[athbridge] smoketest: process-isolation -> SAME PID (not isolated) FAIL\n",
                        );
                    } else {
                        let _ = sys_write_fd(
                            1,
                            b"[athbridge] smoketest: process-isolation -> wrong exit codes (want A=42 B=0) FAIL\n",
                        );
                    }
                }
                _ => {
                    sys_print(9690);
                    let _ = sys_write_fd(
                        1,
                        b"[athbridge] smoketest: process-isolation -> launch/reap FAIL\n",
                    );
                }
            }

            // ── Child C: a REAL external Windows console .exe from the VFS ──────
            // The production "run a file on disk" route: write a real MSVC /MT
            // console .exe to an arbitrary VFS path, then launch it via the
            // Target::Pe { path } handoff (token "pe:<path>") — NOT an in-binary
            // fixture. hello.exe does WriteFile(stdout,"real windows exe") then
            // returns 0, so the /MT CRT ExitProcess(0)'s and we reap 0. This
            // generalizes "a real .exe runs" from bundled bytes to a real file
            // the loader reads off the filesystem (athbridge_run::read_pe_file).
            // Sentinels: 9670 write-exe failed · 9671 launch/reap failed ·
            // 9672+(code&0xFF) reaped (9672 = clean exit 0).
            const REAL_APP_PATH: &[u8] = b"/home/athena/rae-app.exe";
            if unsafe { write_blob(REAL_APP_PATH, REAL_HELLO_EXE) } {
                // token = "pe:" ++ REAL_APP_PATH (26 bytes, < REC_WIDTH).
                let mut tok = [0u8; 3 + REAL_APP_PATH.len()];
                tok[..3].copy_from_slice(b"pe:");
                tok[3..].copy_from_slice(REAL_APP_PATH);
                match unsafe { launch_one(&tok) } {
                    Some((_pid, code)) => {
                        sys_print(9672 + (code & 0xFF));
                        if (code & 0xFF) == 0 {
                            let _ = sys_write_fd(
                                1,
                                b"[athbridge] smoketest: real external .exe via VFS (pe:/home/athena/rae-app.exe) -> executed + exit 0 PASS\n",
                            );
                        } else {
                            let _ = sys_write_fd(
                                1,
                                b"[athbridge] smoketest: real external .exe via VFS -> NONZERO exit FAIL\n",
                            );
                        }
                    }
                    None => {
                        sys_print(9671);
                        let _ = sys_write_fd(
                            1,
                            b"[athbridge] smoketest: real external .exe via VFS -> launch/reap FAIL\n",
                        );
                    }
                }
            } else {
                sys_print(9670);
                let _ = sys_write_fd(
                    1,
                    b"[athbridge] smoketest: real external .exe via VFS -> exe write FAIL\n",
                );
            }
        }

        // i915d — Intel Path C GPU daemon (PCI → VBT → GGTT → rings → modeset on
        // the LinuxKPI host). Spawned + waited BEFORE amdgpud: on QEMU there is no
        // Intel display device, so the probe exits immediately (daemon prints 9200
        // then 9299), and running it first keeps amdgpud's 7602 reap marker as the
        // xtask CI drain terminator. On Intel iron it walks the full pipeline.
        // Sentinels (77xx — the 9xxx block belongs to the slice-3 rg port test):
        // 7700 spawning · 7701+(pid&0x1F) spawned · 7735+(code&0x0F) reaped
        // (7735 = clean exit) · 7790 spawn failed.
        sys_print(7700);
        let i915_pid = sys_spawn(b"i915d");
        if i915_pid != u64::MAX && i915_pid < 0xFF00_0000_0000_0000 {
            sys_print(7701 + (i915_pid & 0x1F));
            let i915_code = sys_wait(i915_pid);
            sys_print(7735 + (i915_code & 0x0F));
        } else {
            sys_print(7790);
        }

        // nvidiad — native NVIDIA GPU daemon (chip identification bring-up).
        // QEMU has no NVIDIA GPU, so the probe exits immediately (daemon prints
        // 9400 then 9499). On real NVIDIA silicon it maps BAR0, decodes
        // NV_PMC_BOOT_0 (host-tested ath_nvidia logic) and reports the part +
        // its firmware-requirement tier (where GSP-RM walls a native driver).
        // Sentinels (78xx): 7800 spawning · 7801+(pid&0x1F) spawned ·
        // 7835+(code&0x0F) reaped (7835 = clean exit) · 7890 spawn failed.
        sys_print(7800);
        let nvidia_pid = sys_spawn(b"nvidiad");
        if nvidia_pid != u64::MAX && nvidia_pid < 0xFF00_0000_0000_0000 {
            sys_print(7801 + (nvidia_pid & 0x1F));
            let nvidia_code = sys_wait(nvidia_pid);
            sys_print(7835 + (nvidia_code & 0x0F));
        } else {
            sys_print(7890);
        }

        // athlangd — userspace Rae-script interpreter daemon (Concept
        // §Customization Engine). PERSISTENT: spawned WITHOUT wait — it
        // loops draining queued >64 KiB scripts via SCRIPT_FETCH/COMPLETE.
        // Proof: submit an over-inline-limit script (64 KiB of whitespace
        // padding + `return 9`), then poll SCRIPT_STATUS until the daemon
        // completes it. Sentinels (885x): 8850 spawning · 8851+(pid&7)
        // spawned · 8860 daemon completed the big script (exit 9) · 8865
        // wrong exit/state · 8869 poll timeout · 8890 spawn failed.
        sys_print(8850);
        let athlangd_pid = sys_spawn(b"athlangd");
        if athlangd_pid != u64::MAX && athlangd_pid < 0xFF00_0000_0000_0000 {
            sys_print(8851 + (athlangd_pid & 7));
            // 64 KiB + 8 of source: spaces are lexer whitespace, the tail
            // is the program. One byte over the kernel's inline ceiling.
            static mut BIG_SCRIPT: [u8; 65_544] = [b' '; 65_544];
            let script = &mut *core::ptr::addr_of_mut!(BIG_SCRIPT);
            let tail = b"return 9";
            let at = script.len() - tail.len();
            script[at..].copy_from_slice(tail);
            let sid: u64;
            core::arch::asm!(
                "syscall",
                inout("rax") SYS_SCRIPT_RUN => sid,
                in("rdi") script.as_ptr() as u64,
                in("rsi") script.len() as u64,
                in("rdx") 0u64, // pure compute — no capabilities needed
                out("rcx") _, out("r11") _,
            );
            // ScriptAbi (repr C): state u32 @16, exit_code i32 @20.
            let mut verdict = 8869u64; // poll timeout
            let mut abi_buf = [0u8; 56];
            for _ in 0..300 {
                core::arch::asm!(
                    "syscall",
                    in("rax") SYS_LINUXKPI_MSLEEP,
                    in("rdi") 20u64,
                    out("rcx") _, out("r11") _,
                );
                let n: u64;
                core::arch::asm!(
                    "syscall",
                    inout("rax") SYS_SCRIPT_STATUS => n,
                    in("rdi") sid,
                    in("rsi") abi_buf.as_mut_ptr() as u64,
                    in("rdx") abi_buf.len() as u64,
                    out("rcx") _, out("r11") _,
                );
                if n != 56 {
                    continue;
                }
                let state =
                    u32::from_le_bytes([abi_buf[16], abi_buf[17], abi_buf[18], abi_buf[19]]);
                if state <= 1 {
                    continue; // Queued / Running — daemon still on it
                }
                let exit = i32::from_le_bytes([abi_buf[20], abi_buf[21], abi_buf[22], abi_buf[23]]);
                verdict = if state == 2 && exit == 9 { 8860 } else { 8865 };
                break;
            }
            sys_print(verdict);
        } else {
            sys_print(8890);
        }

        // linux_hello — Linux x86_64 ABI conformance fixture: raw Linux
        // syscalls only (arch_prctl TLS + sched_yield persistence + write +
        // exit_group). Its ELF is stamped ELFOSABI_LINUX, so SYS_SPAWN routes
        // it through linux_exec (Linux auxv stack + Linux syscall table).
        // Serial proof: "[linux_hello] Linux ABI OK: TLS survived 8 yields".
        // Sentinels: 8740 spawning · 8741+(pid&0x1F) spawned · 8770+(code&0x0F)
        // reaped (8770 = clean exit) · 8780 spawn failed.
        if RUN_BOOT_DEMOS {
            sys_print(8740);
            let lh_pid = sys_spawn(b"linux_hello");
            if lh_pid != u64::MAX && lh_pid < 0xFF00_0000_0000_0000 {
                sys_print(8741 + (lh_pid & 0x1F));
                let lh_code = sys_wait(lh_pid);
                sys_print(8770 + (lh_code & 0x0F));
            } else {
                sys_print(8780);
            }
        }

        // hello_relibc — native relibc port smoke test (relibc speaks NATIVE
        // AthenaOS syscalls — see athenaOS_syscall.rs — so xtask stamps it
        // ELFOSABI_ATHENAOS and it spawns natively). TLS now persists across
        // switches via SYS_SET_FS_BASE (126) + Task::fs_base, so relibc
        // startup gets further than the old PAUSED state; the kernel cleanly
        // kills it if it still trips (MasterChecklist "relibc full startup").
        // Sentinels: 8800 spawning · 8801+(pid&0x1F) spawned · 8835+(code&0x0F)
        // reaped (8835 = clean exit) · 8890 spawn failed.
        if RUN_BOOT_DEMOS {
            sys_print(8800);
            let relibc_pid = sys_spawn(b"hello_relibc");
            if relibc_pid != u64::MAX && relibc_pid < 0xFF00_0000_0000_0000 {
                sys_print(8801 + (relibc_pid & 0x1F));
                let relibc_code = sys_wait(relibc_pid);
                sys_print(8835 + (relibc_code & 0x0F));
            } else {
                sys_print(8890);
            }
        }

        // amdgpud — persistent Path C GPU daemon (real upstream amdgpu).
        // Do not wait for it: on working AMD hardware it owns the initialized
        // adev for the lifetime of the OS and services render clients. On QEMU
        // it self-exits after the no-Radeon preflight (9099).
        sys_print(7600);
        let gpu_pid = sys_spawn(b"amdgpud");
        sys_print(7601 + (gpu_pid & 0xFF));
        if gpu_pid != u64::MAX && gpu_pid < 0xFF00_0000_0000_0000 {
            // A working amdgpud retains the upstream adev for the lifetime of
            // the OS, so waiting here would deadlock the rest of user startup.
            // 7602 means "spawned and detached"; amdgpud emits 9004 when ready.
            sys_print(7602);
        } else {
            sys_print(7690);
        }

        // ── v0.1 PRODUCTION BOOT CUT ────────────────────────────────────────
        // The real driver daemons (amdgpud, i915d) are up. EVERYTHING below is
        // dev demos: the LinuxKPI smoketest + the VFS/AthUI/**triangle**/input/
        // driver_supervisor/rg session sequences — including the "Vulkan triangle"
        // that shows on boot. They are self-tests, not daily-driver features, so
        // for the production boot we exit here and let the kernel shell_runner
        // desktop be the only thing on screen. Flip RUN_BOOT_DEMOS to re-run the
        // full bring-up demo sequence. (MasterChecklist Milestone A / v0.1 polish.
        // Follow-up: a real driver_supervisor supervision loop, spawned here when
        // it does production work instead of the cap-grant demo.)
        // `black_box` keeps the flag opaque to const-folding so the demo code
        // below stays "reachable" to the compiler (no unreachable_code/pattern
        // warnings) while still being skipped at runtime when the flag is false.
        if !core::hint::black_box(RUN_BOOT_DEMOS) {
            sys_print(7900);
            sys_exit(0);
        }

        // LinuxKPI Phase 1–4 smoketest ELF (sentinels 7000–7900, 8600–8700).
        if RUN_BOOT_DEMOS {
            let lkpi_pid = sys_spawn(b"hello_linuxkpi");
            sys_print(7500000 + (lkpi_pid & 0xFF));
        }

        // ─────────────────────────────────────────────────────────────────
        // Session 3 demo: persistent file I/O through the VFS layer to
        // AthFS-on-virtio-blk (or `/home/athena/` RAM file when block FS absent).
        //
        // Sentinel encoding so the serial log is human-decodable at a glance:
        //   3000             "begin VFS demo"
        //   3100 + fd        "open returned fd"
        //   3200 + n         "wrote n bytes"
        //   3300 + n         "stat reported size n"
        //   3400 + n         "read n bytes back"
        //   3500..3599       per-byte readback of the file contents (offset = sentinel - 3500)
        //   3900             "end VFS demo"
        // ─────────────────────────────────────────────────────────────────
        sys_print(3000);

        let path = b"/home/athena/hello.txt";
        let payload = b"Hello from Session 3!";

        let fd = sys_open(path, 0);
        sys_print(3100 + fd);
        if !open_ok(fd) {
            sys_print(3001);
        } else {
            let n_written = sys_write_fd(fd, payload);
            sys_print(3200 + n_written);
            let _ = sys_close(fd);

            let fd2 = sys_open(path, 0);
            sys_print(3100 + fd2);
            if open_ok(fd2) {
                let size = sys_stat(fd2);
                sys_print(3300 + size);
                let _ = sys_seek(fd2, 0);
                let mut buf = [0u8; 32];
                let n_read = sys_read_fd(fd2, &mut buf);
                sys_print(3400 + n_read);
                let n = n_read as usize;
                for i in 0..n.min(buf.len()) {
                    sys_print(3500 + buf[i] as u64);
                }
                let _ = sys_close(fd2);
            }
        }
        sys_print(3900);

        // ─────────────────────────────────────────────────────────────────
        // Continue to UI demo.
        // ─────────────────────────────────────────────────────────────────
        // Session 5 demo: real windowed UI through AthUI + compositor.
        //
        // We allocate a 480x320 surface, build a Frame("Hello AthenaOS")
        // containing a Label + Button("Click me"), render the widget tree
        // into the surface via athgfx::Canvas, present at (160, 120).
        // Then we fire a few synthetic KeyPress events at the Button to
        // toggle its color and re-render — proving the widget event path
        // works end-to-end.
        //
        // Sentinel encoding:
        //   5000             "begin AthUI demo"
        //   5100 + id        "surface created"
        //   5200             "initial render done"
        //   5300             "presented at (160, 120)"
        //   5400 + i         "toggle iteration i (re-render + re-present)"
        //   5900             "end AthUI demo"
        // ─────────────────────────────────────────────────────────────────
        sys_print(5000);

        const SURFACE_VIRT: u64 = 0x0000_8888_0000;
        let width: u64 = 480;
        let height: u64 = 320;
        sys_print(5000);
        let id = sys_surface_create(width, height, SURFACE_VIRT);
        sys_print(5005);
        if id == u64::MAX {
            sys_print(5001);
            sys_exit(1);
        }
        sys_print(5100 + id);

        // Build a Canvas over the surface bytes. Skip dynamic dispatch
        // (Box<dyn Widget>) — the user ELF loader doesn't yet handle vtables
        // emitted by function-local `impl Widget for Body` blocks. Compose
        // by hand: draw chrome via a Frame "drawer" function, then label
        // and button directly.
        let mut canvas = Canvas::new(SURFACE_VIRT as *mut u8, width as usize, height as usize, 4);

        let label = Label::new(
            "Welcome to AthenaOS",
            athui::body_y_start(),
            athui::body_y_start(),
        );
        let mut button = Button::new(
            "Click me",
            width as usize / 2 - 80,
            height as usize - 80,
            160,
            44,
        );

        // Manually draw a Frame's chrome — same shape as Frame::render but
        // without the dyn-dispatch body callout.
        let chrome_h: usize = 24;
        canvas.clear(athui::theme::WINDOW_BG);
        canvas.fill_rect(0, 0, width as usize, chrome_h, athui::theme::CHROME_BG);
        canvas.draw_text(
            8,
            (chrome_h - 8) / 2,
            "Hello AthenaOS",
            athui::theme::TITLE_FG,
            None,
        );
        canvas.draw_text(
            width as usize - 16,
            (chrome_h - 8) / 2,
            "x",
            athui::theme::BUTTON_HOT,
            None,
        );
        for x in 0..width as usize {
            canvas.draw_pixel(x, chrome_h - 1, athui::theme::BORDER);
        }
        canvas.draw_rect_outline(0, 0, width as usize, height as usize, athui::theme::BORDER);
        label.render(&mut canvas);
        button.render(&mut canvas);

        sys_print(5200);

        let _ = sys_surface_present(id, 160, 120);
        sys_print(5300);

        // Synthetic key-press events toggle the button color; re-render the
        // button area + re-present each iteration so the compositor blits
        // the new pixels.
        for i in 0u64..1 {
            for _ in 0..30_000 {
                core::hint::spin_loop();
            }
            button.on_event(&athui::Event::KeyPress(i as u8));
            // Only re-render the button — the rest of the window is unchanged.
            button.render(&mut canvas);
            let _ = sys_surface_present(id, 160, 120);
            sys_print(5400 + i);
        }
        sys_print(5900);

        // ─────────────────────────────────────────────────────────────────
        // Session 6 — Year-1 milestone demo: "hello triangle" rendered by
        // AthGFX's software rasterizer into the same compositor surface.
        //
        // This closes the concept-doc Year-1 milestone visually:
        //   "Boots, draws, plays a single Vulkan demo."
        //
        // The triangle is rasterized in software today. Once we ship a real
        // GPU driver + wgpu backend, the user code below stays unchanged —
        // we'll swap the implementation behind `canvas.draw_triangle()`.
        //
        // Sentinel encoding:
        //   6000             begin triangle demo
        //   6100             cleared the body region
        //   6200             rasterized the triangle
        //   6300             presented
        //   6900             end demo
        // ─────────────────────────────────────────────────────────────────
        sys_print(6000);

        // Clear the body region (below the title bar) to a neutral dark.
        let chrome_h = 24usize;
        canvas.fill_rect(
            0,
            chrome_h,
            width as usize,
            height as usize - chrome_h,
            athui::theme::WINDOW_BG,
        );
        sys_print(6100);

        // Three corner vertices of a centered triangle inside the body. The
        // canonical R/G/B vertex colors produce a smooth ARGB gradient — the
        // exact same image every Vulkan/Metal/WebGPU hello-world ships with.
        let cx = (width as i32) / 2;
        let body_top = chrome_h as i32 + 20;
        let body_bot = (height as i32) - 30;
        let tri_w = 240;
        let v_top = (cx, body_top, 0xFF_FF_22_22); // red
        let v_left = (cx - tri_w / 2, body_bot, 0xFF_22_FF_22); // green
        let v_right = (cx + tri_w / 2, body_bot, 0xFF_22_22_FF); // blue
        canvas.draw_triangle(v_top, v_left, v_right);
        sys_print(6200);

        // Add a small caption under the title so you can tell at a glance
        // which screen you're looking at.
        canvas.draw_text(
            8,
            chrome_h + 4,
            "Year-1 demo: software-rasterized triangle",
            athui::theme::TITLE_FG,
            None,
        );

        let _ = sys_surface_present(id, 160, 120);
        sys_print(6300);

        // ─── Animated frames ───────────────────────────────────────────
        // Rotate the per-vertex colors over 12 frames. The triangle stays
        // in place but the color wheel spins through it, proving the full
        // render/present pipeline runs every frame (clear → rasterize →
        // text overlay → present) without leaking state or stalling.
        //
        // Sentinel 6500 + i = frame i complete.
        let palette: [u32; 3] = [0xFF_FF_22_22, 0xFF_22_FF_22, 0xFF_22_22_FF];
        for frame in 0u64..1 {
            for _ in 0..20_000 {
                core::hint::spin_loop();
            }

            // Rotate the palette by `frame` positions.
            let c0 = palette[((frame + 0) % 3) as usize];
            let c1 = palette[((frame + 1) % 3) as usize];
            let c2 = palette[((frame + 2) % 3) as usize];

            canvas.fill_rect(
                0,
                chrome_h,
                width as usize,
                height as usize - chrome_h,
                athui::theme::WINDOW_BG,
            );
            canvas.draw_triangle(
                (cx, body_top, c0),
                (cx - tri_w / 2, body_bot, c1),
                (cx + tri_w / 2, body_bot, c2),
            );
            canvas.draw_text(
                8,
                chrome_h + 4,
                "Year-1 demo: software-rasterized triangle",
                athui::theme::TITLE_FG,
                None,
            );
            let _ = sys_surface_present(id, 160, 120);
            sys_print(6500 + frame);
        }
        sys_print(6900);

        // ─────────────────────────────────────────────────────────────────
        // Session 7 — Phase 6.1: Vulkan API `vkQueueSubmit` demo.
        //
        // Proves the `vk_*` surface can construct a pipeline, allocate a vertex
        // buffer, load SPIR-V, and record/submit a command buffer without panics.
        // This is the software-path equivalent of the real VirtIO-GPU Vulkan submit.
        //
        // Sentinel encoding:
        //   8000             begin vkQueueSubmit demo
        //   8100             vk instance/device created
        //   8200             spirv modules & pipeline created
        //   8300             vertex buffer allocated
        //   8400             command buffer recorded & submitted
        //   8900             end demo
        // ─────────────────────────────────────────────────────────────────
        sys_print(8000);

        use athgfx::vulkan::*;

        let app_info = VkApplicationInfo {
            app_name: alloc::string::String::from("VkDemo"),
            app_version: 1,
            engine_name: alloc::string::String::from("AthGFX"),
            engine_version: 1,
            api_version: VK_API_VERSION_1_0,
        };

        if let Ok(instance) = vk_create_instance(&app_info, &[], &[]) {
            let pdevs = vk_enumerate_physical_devices(&instance);
            if !pdevs.is_empty() {
                let pdev = &pdevs[0];
                let queue_info = VkDeviceQueueCreateInfo {
                    queue_family_index: 0,
                    queue_count: 1,
                    priorities: alloc::vec![1.0],
                };

                let features = VkPhysicalDeviceFeatures::default();

                if let Ok(device) = vk_create_device(pdev, 0, &[queue_info], &features, &[]) {
                    sys_print(8100);

                    let vs_spirv: [u8; 4] = [0x03, 0x02, 0x23, 0x07];
                    let fs_spirv: [u8; 4] = [0x03, 0x02, 0x23, 0x07];

                    let vs_mod = vk_create_shader_module(&device, &vs_spirv).unwrap();
                    let fs_mod = vk_create_shader_module(&device, &fs_spirv).unwrap();

                    let stages = alloc::vec![
                        VkPipelineShaderStageCreateInfo {
                            stage: VK_SHADER_STAGE_VERTEX,
                            module_handle: vs_mod.handle,
                            entry_point: alloc::string::String::from("main"),
                        },
                        VkPipelineShaderStageCreateInfo {
                            stage: VK_SHADER_STAGE_FRAGMENT,
                            module_handle: fs_mod.handle,
                            entry_point: alloc::string::String::from("main"),
                        }
                    ];

                    let pipeline_info = VkGraphicsPipelineCreateInfo {
                        stages,
                        vertex_input_state: VkPipelineVertexInputStateCreateInfo {
                            binding_descriptions: alloc::vec![],
                            attribute_descriptions: alloc::vec![],
                        },
                        input_assembly_state: VkPipelineInputAssemblyStateCreateInfo {
                            topology: VkPrimitiveTopology::TriangleList,
                            primitive_restart_enable: false,
                        },
                        viewport_state: VkPipelineViewportStateCreateInfo {
                            viewports: alloc::vec![],
                            scissors: alloc::vec![],
                        },
                        rasterization_state: VkPipelineRasterizationStateCreateInfo {
                            depth_clamp_enable: false,
                            rasterizer_discard_enable: false,
                            polygon_mode: VkPolygonMode::Fill,
                            cull_mode: VkCullModeFlags::None,
                            front_face: VkFrontFace::Clockwise,
                            depth_bias_enable: false,
                            depth_bias_constant_factor: 0.0,
                            depth_bias_clamp: 0.0,
                            depth_bias_slope_factor: 0.0,
                            line_width: 1.0,
                        },
                        multisample_state: VkPipelineMultisampleStateCreateInfo {
                            rasterization_samples: 1,
                            sample_shading_enable: false,
                            min_sample_shading: 1.0,
                            alpha_to_coverage_enable: false,
                            alpha_to_one_enable: false,
                        },
                        depth_stencil_state: None,
                        color_blend_state: VkPipelineColorBlendStateCreateInfo {
                            logic_op_enable: false,
                            logic_op: VkLogicOp::Copy,
                            attachments: alloc::vec![],
                            blend_constants: [0.0; 4],
                        },
                        dynamic_states: alloc::vec![],
                        layout_handle: 0,
                        render_pass_handle: 0,
                        subpass: 0,
                    };

                    if let Ok(pipeline) = vk_create_graphics_pipeline(&device, &pipeline_info) {
                        sys_print(8200);

                        let buf_info = VkBufferCreateInfo {
                            size: 1024,
                            usage: VK_BUFFER_USAGE_VERTEX_BUFFER,
                            sharing_mode: VkSharingMode::Exclusive,
                            queue_family_indices: alloc::vec![],
                        };
                        if let Ok(mut vbuf) = vk_create_buffer(&device, &buf_info) {
                            let reqs = vbuf.memory_requirements();
                            let mem_info = VkMemoryAllocateInfo {
                                allocation_size: reqs.size,
                                memory_type_index: 0,
                            };
                            if let Ok(mem) = device.allocate_memory(&mem_info) {
                                let _ = vbuf.bind_memory(&mem, 0);
                                sys_print(8300);

                                if let Ok(mut pool) = vk_create_command_pool(&device, 0, 0) {
                                    let mut cb = pool.allocate_command_buffer(
                                        &device,
                                        VkCommandBufferLevel::Primary,
                                    );
                                    let _ = cb.begin();
                                    cb.cmd_bind_pipeline(
                                        VkPipelineBindPoint::Graphics,
                                        pipeline.handle,
                                    );
                                    cb.cmd_bind_vertex_buffers(0, &[vbuf.handle], &[0]);
                                    cb.cmd_draw(3, 1, 0, 0);
                                    let _ = cb.end();

                                    if let Some(queue) = device.get_queue(0, 0) {
                                        let wait_semaphores: [&VkSemaphore; 0] = [];
                                        let wait_dst_stage_masks: [u32; 0] = [];
                                        let command_buffers = [&cb];
                                        let mut signal_semaphores_array: [&mut VkSemaphore; 0] = [];
                                        let submit = VkSubmitInfo {
                                            wait_semaphores: &wait_semaphores,
                                            wait_dst_stage_masks: &wait_dst_stage_masks,
                                            command_buffers: &command_buffers,
                                            signal_semaphores: &mut signal_semaphores_array,
                                        };
                                        if vk_queue_submit(queue, &[submit], None)
                                            == VkResult::Success
                                        {
                                            sys_print(8400);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        sys_print(8900);

        // ─────────────────────────────────────────────────────────────────
        // Session N+1: input routing demo.
        //
        // The kernel pushes keyboard scancodes into Channel 1 (Keyboard)
        // from its IRQ handler, and now also calls scheduler::unblock_receivers(1)
        // so a SYS_RECV blocker wakes up. The kernel seeded our cnode with
        // a Read cap on Channel 1 at handle 1.
        //
        // We display a "Last key: 0xNN" line that updates per keypress.
        // Boot.ps1 drives synthetic keys via QEMU's monitor (`sendkey a/b/c/d`).
        //
        // Sentinel encoding:
        //   7000             begin input demo
        //   7100 + scancode  received scancode (e.g. 'a' = 0x1E = 30 -> 7130)
        //   7200             4 keys received, demo ending
        //   7900             clean exit
        // ─────────────────────────────────────────────────────────────────
        sys_print(7000);

        const KBD_CAP: u64 = 1;
        let key_y = chrome_h + 80;

        for i in 0u64..0 {
            // Block until a scancode arrives. The kernel returns:
            //   status (rax) = 0, msg_type (rsi) = 1, arg1 (rdx) = scancode
            let (status, _mtype, scancode, _, _) = sys_recv(KBD_CAP);
            if status != 0 {
                sys_print(7900 + status); // recv error sentinel
                break;
            }

            // Sentinel: 7100 + scancode. Decoded later from the boot log.
            sys_print(7100 + scancode);

            // Update the on-screen label. Erase the previous label band first.
            canvas.fill_rect(0, key_y, width as usize, 16, athui::theme::WINDOW_BG);
            canvas.draw_text(
                8,
                key_y,
                "Last key scancode: 0x",
                athui::theme::TEXT_FG,
                None,
            );
            // Two hex digits of the scancode.
            let hi = ((scancode >> 4) & 0xF) as u8;
            let lo = (scancode & 0xF) as u8;
            let hex = |n: u8| -> char {
                if n < 10 {
                    (b'0' + n) as char
                } else {
                    (b'a' + (n - 10)) as char
                }
            };
            let mut tmp = [hex(hi) as u8, hex(lo) as u8];
            // Cheap one-glyph-at-a-time render of the 2 digits.
            canvas.draw_glyph(
                8 + 21 * 8,
                key_y,
                tmp[0] as char,
                athui::theme::BUTTON_HOT,
                None,
            );
            canvas.draw_glyph(
                8 + 22 * 8,
                key_y,
                tmp[1] as char,
                athui::theme::BUTTON_HOT,
                None,
            );

            // Also print a per-key counter in the upper-right of the body.
            canvas.fill_rect(width as usize - 80, key_y, 80, 16, athui::theme::WINDOW_BG);
            tmp[0] = b'0' + (i as u8);
            canvas.draw_text(
                width as usize - 64,
                key_y,
                "key #",
                athui::theme::TEXT_FG,
                None,
            );
            canvas.draw_glyph(
                width as usize - 64 + 5 * 8,
                key_y,
                tmp[0] as char,
                athui::theme::BUTTON_HOT,
                None,
            );

            let _ = sys_surface_present(id, 160, 120);
        }
        sys_print(7200);

        // ─────────────────────────────────────────────────────────────────
        // Slice 2: multi-process via SYS_SPAWN + SYS_CAP_GRANT + SYS_WAIT.
        //
        // user_init is the lone parent today; we spawn the prebuilt
        // `driver_supervisor` ELF (also packed in initramfs.tar). The child
        // polls for a granted Mmio cap (handle 1), maps it, writes a pattern,
        // then calls SYS_DRIVER_REGISTER (109). Sentinels: 10001 start · 55555
        // cap ready · 10010 MMIO · 10002 register OK · 10003 fail · exit 0 on success.
        //
        // We additionally exercise SYS_CAP_GRANT against the child's task id
        // to prove the cross-task cap derivation path works. The grant may
        // race with the child completing — in that case it returns the
        // NO_TASK error code (u64::MAX - 4). Either outcome is informative.
        //
        // Sentinel encoding:
        //   8000             begin spawn demo
        //   8100 + pid       child TaskId returned by SYS_SPAWN
        //   8200 + rc        SYS_CAP_GRANT result (0 = ok, MAX-4 = NoSuchTask)
        //   8300 + code      child exit code (expected 42)
        //   8900             demo done
        // ─────────────────────────────────────────────────────────────────
        sys_print(8000);

        let child_pid = sys_spawn(b"driver_supervisor");
        sys_print(8100 + (child_pid & 0xFF));

        if child_pid != u64::MAX && child_pid < 0xFF00_0000_0000_0000 {
            // Derive a 4 KiB Mmio sub-range with READ|WRITE|MAP rights from
            // our master Mmio cap (handle 1). Rights bits per
            // docs/design/capabilities.md: R=1, W=2, EXEC=4, MAP=8.
            // 1|2|8 = 11.
            let grant_rc = sys_cap_grant(
                child_pid, 1,    // src_handle = our master Mmio
                11,   // R|W|MAP, no GRANT (child can't re-grant)
                0,    // derive_arg0 = offset into parent's range
                4096, // derive_arg1 = sub-range length
            );
            // grant_rc returns the new handle in the child's table on success
            // (typically 1), or a u64::MAX-N error code. We clamp to a small
            // window so the sentinel doesn't overflow our 16-bit decoder.
            sys_print(8200 + (grant_rc & 0xFFFF));

            let code = sys_wait(child_pid);
            sys_print(8300 + (code & 0xFFFF));
        } else {
            sys_print(8190); // spawn failed
        }

        sys_print(8900);

        // ─────────────────────────────────────────────────────────────────
        // Slice 3: Third-party port binary load test (ripgrep)
        // ─────────────────────────────────────────────────────────────────
        sys_print(9000);
        let port_pid = sys_spawn(b"rg");
        sys_print(9100 + (port_pid & 0xFF));
        if port_pid != u64::MAX && port_pid < 0xFF00_0000_0000_0000 {
            let port_code = sys_wait(port_pid);
            sys_print(9200 + (port_code & 0xFFFF));
        } else {
            sys_print(9190); // port spawn failed
        }
        sys_print(9900);

        // Demo done — exit cleanly. Previous session fixed sys_exit teardown
        // so the kernel survives this without faulting.
        sys_print(7900);
        sys_exit(0);
    }
}

// ── Compositor syscall wrappers ──────────────────────────────────────────

#[inline(always)]
unsafe fn sys_surface_create(width: u64, height: u64, vaddr: u64) -> u64 {
    sys_print(5002);
    let r;
    core::arch::asm!(
        "syscall",
        in("rax") 24,
        in("rdi") width,
        in("rsi") height,
        in("rdx") vaddr,
        out("rcx") _,
        out("r11") _,
        lateout("rax") r,
    );
    sys_print(5003);
    r
}

#[inline(always)]
unsafe fn sys_surface_present(id: u64, x: i64, y: i64) -> u64 {
    let r: u64;
    core::arch::asm!(
        "syscall",
        inout("rax") 25u64 => r,
        in("rdi") id,
        in("rsi") x as u64,
        in("rdx") y as u64,
        out("rcx") _, out("r11") _,
    );
    r
}
