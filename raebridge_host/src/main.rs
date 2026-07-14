//! AthBridge host — the userspace process Windows programs run inside.
//!
//! Concept §Compatibility Strategy: "AthBridge runs Windows apps on day
//! one. Wine + Proton heritage, tightly integrated. Not a 'subsystem' —
//! apps run naturally." This binary is that runtime: it maps a PE32+
//! image into its own address space (Wine model — guest VA == host VA),
//! patches the IAT against the `extern "win64"` shim table, installs the
//! process-global Win32 compat session, and jumps to the PE entry point.
//! From that instruction on, unmodified Windows machine code is executing
//! on AthenaOS, calling Win32 APIs that land in `raebridge::winapi_shims`.
//!
//! This is the *durable smoketest harness*: it runs BOTH bring-up images in
//! one process and emits FAIL-able verdict lines that survive a production
//! boot (they do not depend on user_init's `RUN_BOOT_DEMOS` flag — user_init
//! spawns this host unconditionally on the durable path).
//!
//!   1. hello-world image — GetStdHandle → WriteFile("Hello from Windows\n")
//!      → `ret`. WriteFile routes through the win64 shim to `sys_write` on the
//!      seeded stdout fd, tee'd into a capture buffer the harness asserts on.
//!      Verdict: `[raebridge] smoketest: hello-world exe -> stdout "Hello from
//!      Windows" + exit 0 PASS`.
//!   2. exit-code image — ExitProcess(42). The shim prints
//!      `[raebridge] guest ExitProcess(42) -> exit 42 PASS` and terminates the
//!      process with code 42; user_init reaps 42 and emits the exit-code
//!      verdict. Run LAST because ExitProcess never returns.
//!
//!   3. gs-base image — reads `gs:[0x30]` (the TEB self-pointer the loader's
//!      `SYS_SET_GS_BASE` installed), verifies it is non-zero + self-consistent,
//!      writes a TEB scratch sentinel, forces a context switch (`SYS_YIELD`),
//!      then re-reads `gs:[0x30]` + the sentinel — proving the scheduler
//!      restored the GS base. This is the foundation for real MSVC-CRT `.exe`
//!      startup (their entry reads `gs:[0x30]` before anything else).
//!      Verdict: `[raebridge] smoketest: gs-base exe -> gs:[0x30]==TEB survives
//!      reschedule + exit 0 PASS`.
//!
//!   4. REAL MSVC /MT console exes (load + import-resolve proofs) — genuine
//!      cl.exe-compiled PE32+ images (the WriteFile fixture, 73 KERNEL32
//!      imports; the printf fixture, 74). Only ONE real CRT exe can run to its
//!      ExitProcess per host process (it terminates us), so these are mapped +
//!      IAT-patched and REQUIRED to resolve every import to a real shim (zero
//!      stubs) WITHOUT executing their CRT — the executing terminator is the C++
//!      fixture below (the higher-value milestone).
//!
//!   5. REAL MSVC /MT C++ exe — THE C++-runtime milestone. A genuine
//!      cl.exe-compiled C++ PE32+ (imports the SAME 74 KERNEL32 functions as the
//!      printf fixture — the C++ static-ctor table walk / atexit / EH personality
//!      are CRT-internal in /MT). Its unmodified MSVC CRT runs mainCRTStartup →
//!      __scrt_common_main_seh → _initterm over the .CRT$XC* static-init table
//!      (which runs g_init's ctor → "ctor ran") → main → "hello from c++ 7",
//!      returns 0, CRT ExitProcess(0)'s. This phase is the process terminator.
//!      Verdict (emitted from the armed shim_exit_process, kind=3, asserting BOTH
//!      lines in order — FAILs if the ctor line is missing): `[raebridge]
//!      smoketest: real MSVC /MT C++ exe -> static-ctor ran + main + exit 0 PASS`.
//!
//! Serial proof lines (grep targets):
//!   [raebridge] smoketest: hello-world exe -> stdout "Hello from Windows" + exit 0 PASS
//!   [raebridge] smoketest: gs-base exe -> gs:[0x30]==TEB survives reschedule + exit 0 PASS
//!   [raebridge] smoketest: real MSVC /MT exe -> all imports resolved to shims PASS
//!   [raebridge] smoketest: real MSVC /MT printf exe -> all imports resolved to shims PASS
//!   [raebridge] smoketest: real MSVC /MT C++ exe -> static-ctor ran + main + exit 0 PASS
//! plus user_init sentinel 9700 (= 9700 + C++ exe exit code 0).

#![no_std]
#![no_main]

extern crate alloc;

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;

// Pull in raekit for its #[global_allocator]; everything else here goes
// through raebridge's own syscall bindings.
#[allow(unused_imports)]
use raekit;

use raebridge::exec::load_pe_executable;
use raebridge::syscalls::{sys_debug_print, sys_exit, sys_open, sys_read};
use raebridge::testpe;
use raebridge::winapi_shims::{
    arm_real_cpp_milestone, enable_stdout_capture, gui_edit_control_selftest,
    gui_paint_pixel_count, host_context_installed, install_host_context, reset_message_state,
    take_captured_stdout,
};
use raebridge::{FullCompatSession, SessionId};

fn log(msg: &str) {
    // SAFETY: SYS_DEBUG_PRINT only reads the buffer for the given length.
    unsafe {
        sys_debug_print(msg.as_bytes());
    }
}

fn fail(msg: &str, code: u64) -> ! {
    log(msg);
    // SAFETY: sys_exit never returns.
    unsafe { sys_exit(code) }
}

/// Map + relocate + IAT-patch `image`, jump to its entry point, and return
/// once the guest entry returns. Used for images whose entry ends in `ret`
/// (the hello-world image), not ExitProcess.
///
/// SAFETY: the caller guarantees `image` is a PE32+ whose entry point returns
/// (does not tail-call ExitProcess). The mapped pages are executable user
/// memory; the entry is invoked under the Microsoft x64 ABI.
unsafe fn run_returning_image(image: &[u8], what: &str) -> u64 {
    let loaded = match load_pe_executable(image) {
        Ok(l) => l,
        Err(e) => fail(
            &format!("[raebridge] host: FATAL: load {what}: {e:?}\n"),
            0xE3,
        ),
    };
    if loaded.resolved_imports == 0 {
        fail(
            &format!("[raebridge] host: FATAL: {what} resolved no imports\n"),
            0xE4,
        );
    }
    // SAFETY: entry_point is AddressOfEntryPoint of the image we just mapped
    // into executable user pages; this image's entry returns to us.
    let entry: extern "win64" fn() = core::mem::transmute(loaded.entry_point as usize);
    entry();
    loaded.entry_point
}

/// Map + IAT-patch `image` and call its entry as `extern "win64" fn() -> u64`,
/// returning the value the guest left in RAX. Also asserts every imported name
/// resolved to a real shim (none stubbed), so a missing shim is a FATAL rather
/// than a silent fail-loud trap at call time. Used by the api-exercise image,
/// whose entry returns a per-step result code.
unsafe fn run_result_image(image: &[u8], what: &str, expected_imports: usize) -> u64 {
    let loaded = match load_pe_executable(image) {
        Ok(l) => l,
        Err(e) => fail(
            &format!("[raebridge] host: FATAL: load {what}: {e:?}\n"),
            0xE3,
        ),
    };
    if loaded.resolved_imports != expected_imports || loaded.stubbed_imports != 0 {
        fail(
            &format!(
                "[raebridge] host: FATAL: {what} imports resolved={} stubbed={} (want {} / 0)\n",
                loaded.resolved_imports, loaded.stubbed_imports, expected_imports
            ),
            0xE4,
        );
    }
    // SAFETY: entry_point is the AddressOfEntryPoint of the image we just mapped
    // into executable user pages; this image's entry returns a u64 in RAX and
    // preserves all callee-saved registers per the Win64 ABI.
    let entry: extern "win64" fn() -> u64 = core::mem::transmute(loaded.entry_point as usize);
    entry()
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    log("[raebridge] host: durable smoketest harness starting\n");

    // ── Install the process-global Win32 session ──────────────────────────
    // Any image works as the session seed: it provides the std handle table
    // (stdin/stdout/stderr → native fds 0/1/2) and heap/registry/env that the
    // shims funnel through. Box'd: a stack FullCompatSession has double-faulted
    // before (MasterChecklist Phase 11 known issue).
    let seed = testpe::build_hello_world_exe();
    let session = match FullCompatSession::new(
        SessionId(1),
        String::from("smoketest.exe"),
        seed.clone(),
        String::from("smoketest.exe"),
    ) {
        Ok(s) => Box::new(s),
        Err(_) => fail(
            "[raebridge] host: FATAL: session construction failed\n",
            0xE1,
        ),
    };
    if install_host_context(session).is_some() {
        fail("[raebridge] host: FATAL: duplicate host context\n", 0xE2);
    }
    if !host_context_installed() {
        fail(
            "[raebridge] host: FATAL: host context not installed\n",
            0xE2,
        );
    }

    // ── Phase 1: hello-world image (visible output) ───────────────────────
    // Enable the stdout capture tee, run the image (its entry returns), then
    // assert the bytes the guest handed WriteFile reached sys_write.
    enable_stdout_capture();
    // SAFETY: build_hello_world_exe's entry returns (ends in `ret`).
    unsafe {
        run_returning_image(&seed, "hello-world exe");
    }
    let captured = take_captured_stdout();
    let expected = testpe::HELLO_MSG;
    if captured == expected {
        log("[raebridge] smoketest: hello-world exe -> stdout \"Hello from Windows\" + exit 0 PASS\n");
    } else {
        // Fail loud: report what we actually captured so a marshaling bug is
        // diagnosable from the serial log alone.
        log(&format!(
            "[raebridge] smoketest: hello-world exe -> stdout MISMATCH (got {} bytes, want {}) FAIL\n",
            captured.len(),
            expected.len(),
        ));
        // Continue to the exit-code phase anyway so its verdict still lands;
        // user_init will see exit 42 but the FAIL line above is the proof.
    }

    // ── Phase 1.5: api-exercise image (broadened-shim coverage) ───────────
    // Run a PE whose entry calls GetModuleHandleW/GetProcAddress/Heap*/Tls*/
    // WriteConsoleW in sequence, self-verifies each result, and returns a step
    // code in RAX (0 = all-pass). It both returns the verdict AND tees "API OK"
    // to stdout on full pass, so we assert BOTH: the return code names the exact
    // failing step, and the capture proves the WriteConsoleW path emitted bytes.
    enable_stdout_capture();
    let api_exe = testpe::build_api_exercise_exe();
    // SAFETY: build_api_exercise_exe's entry returns a u64 and preserves
    // callee-saved registers.
    let api_code = unsafe {
        run_result_image(
            &api_exe,
            "api-exercise exe",
            testpe::API_EXERCISE_IMPORTS.len(),
        )
    };
    let api_console = take_captured_stdout();
    let console_ok = api_console.as_slice() == testpe::API_OK_MSG.as_bytes();
    if api_code == testpe::API_PASS && console_ok {
        log("[raebridge] smoketest: api-exercise exe -> GetModuleHandle/GetProcAddress/Heap/TLS/Console all OK + exit 0 PASS\n");
    } else if api_code != testpe::API_PASS {
        // Name the exact step that failed (decodable from serial alone).
        log(&format!(
            "[raebridge] smoketest: api-exercise exe -> step {} FAILED (1=GetModuleHandle 2=GetProcAddress 3=HeapAlloc 4=HeapReAlloc 5=TLS 6=WriteConsole) FAIL\n",
            api_code
        ));
    } else {
        log(&format!(
            "[raebridge] smoketest: api-exercise exe -> steps OK but WriteConsoleW capture MISMATCH (got {} bytes, want {}) FAIL\n",
            api_console.len(),
            testpe::API_OK_MSG.len(),
        ));
    }

    // ── Phase 1.6: GUI window image (user32/gdi32 -> compositor surface) ──
    // The guest-machine-code half of the notepad-class gate: a real cl.exe GUI
    // .exe (custom entry that returns) registers a class, creates+shows a
    // window, and UpdateWindows it -> a SYNCHRONOUS WM_PAINT into its WndProc,
    // which paints (FillRect white bg + TextOut). This exercises the full pump+
    // render pipeline AND the WndProc-dispatch reentrancy fix on real guest code
    // (the WndProc calls BeginPaint/FillRect back through the shims, re-entering
    // with_ctx — would deadlock without resolve-then-invoke-outside-the-lock).
    // Proof: the guest painted pixels into its surface (a headless surface can't
    // be eyeballed, so we read the accumulated paint count).
    {
        let painted_before = gui_paint_pixel_count();
        let gui_exe = testpe::GUI_WINDOW_EXE;
        let loaded = match load_pe_executable(gui_exe) {
            Ok(l) => l,
            Err(e) => fail(
                &format!("[raebridge] host: FATAL: load gui-window exe: {e:?}\n"),
                0xE5,
            ),
        };
        if loaded.stubbed_imports != 0 {
            log(&format!(
                "[raebridge] smoketest: gui-window exe -> {} unresolved import(s) (window/gdi shim gap) FAIL\n",
                loaded.stubbed_imports
            ));
        }
        // SAFETY: gui_window.exe's custom entry `rae_entry` returns (no message
        // loop, no ExitProcess) and preserves callee-saved registers.
        unsafe {
            run_returning_image(gui_exe, "gui-window exe");
        }
        let painted = gui_paint_pixel_count().saturating_sub(painted_before);
        if painted > 0 {
            log(&format!(
                "[raebridge] smoketest: gui-window exe -> RegisterClass+CreateWindow+UpdateWindow->WM_PAINT painted {painted} px (WndProc dispatch + reentrant gdi OK) PASS\n"
            ));
        } else {
            log("[raebridge] smoketest: gui-window exe -> WM_PAINT rendered 0 px (pump/dispatch/paint gap) FAIL\n");
        }
    }

    // ── Phase 1.65: GUI type+save (notepad flow end-to-end) ──────────────
    // A real GUI .exe injects 'H','I' keystrokes -> the pump translates to
    // WM_CHAR -> the WndProc accumulates them and saves "HI" to C:\out.txt via
    // CreateFileW(CREATE_ALWAYS)+WriteFile. We then read that file back through
    // the VFS (C:\ -> /mnt/win_c, tmpfs-backed create-on-open) and assert the
    // typed bytes round-tripped: types + saves, proven end-to-end.
    {
        let save_exe = testpe::GUI_SAVE_EXE;
        let loaded = match load_pe_executable(save_exe) {
            Ok(l) => l,
            Err(e) => fail(
                &format!("[raebridge] host: FATAL: load gui-save exe: {e:?}\n"),
                0xE6,
            ),
        };
        if loaded.stubbed_imports != 0 {
            log(&format!(
                "[raebridge] smoketest: gui-save exe -> {} unresolved import(s) FAIL\n",
                loaded.stubbed_imports
            ));
        }
        // SAFETY: gui_save.exe's custom entry returns (its message loop ends on
        // WM_QUIT) and preserves callee-saved registers.
        unsafe {
            run_returning_image(save_exe, "gui-save exe");
        }
        // Read the saved file back via the SAME per-app bucket path the guest's
        // CreateFileW wrote to (C:\ is namespaced per app; this session is
        // "smoketest.exe"). Proves the bucketed save path round-trips.
        let mut buf = [0u8; 16];
        let vfs = raebridge::app_bucket_vfs_path("smoketest.exe", "C:\\out.txt");
        let fd = unsafe { sys_open(vfs.as_bytes(), 0) };
        // SYS_OPEN error codes are u64::MAX..=MAX-3 (MAX-1 = not found), not just MAX.
        if fd >= u64::MAX - 3 {
            log("[raebridge] smoketest: gui-save exe -> C:\\out.txt not created (save path gap) FAIL\n");
        } else {
            let n = unsafe { sys_read(fd, &mut buf) } as usize;
            let got = &buf[..n.min(buf.len())];
            if got == b"HI" {
                log("[raebridge] smoketest: gui-save exe -> typed 'HI' + CreateFileW/WriteFile -> C:\\out.txt readback 'HI' PASS\n");
            } else {
                log(&format!(
                    "[raebridge] smoketest: gui-save exe -> readback {} bytes, want 'HI' FAIL\n",
                    n
                ));
            }
        }
    }

    // ── Phase 1.7: built-in EDIT control (the real Notepad text mechanism) ─
    // A real Notepad stores its text in a system "EDIT" child it never
    // registers, and typing accumulates THERE (not in a custom WM_CHAR buffer).
    // This drives the real path in QEMU: CreateWindowEx("EDIT") with no
    // RegisterClass, "HI" typed through the standard pump (PostMessage WM_KEYDOWN
    // -> GetMessage -> TranslateMessage -> DispatchMessage -> built-in EDIT proc),
    // GetWindowTextW reads it back, then a WM_PAINT renders it. FAIL-able: a
    // create/dispatch/builtin/get-text/paint regression flips the verdict.
    match unsafe { gui_edit_control_selftest() } {
        Some((true, true)) => log(
            "[raebridge] smoketest: edit-control -> CreateWindowEx(\"EDIT\") + typed 'HI' via pump -> GetWindowTextW 'HI' + WM_PAINT rendered PASS\n",
        ),
        Some((false, _)) => log(
            "[raebridge] smoketest: edit-control -> GetWindowTextW != 'HI' (builtin EDIT/dispatch gap) FAIL\n",
        ),
        Some((true, false)) => log(
            "[raebridge] smoketest: edit-control -> typed 'HI' but WM_PAINT rendered 0 px (EDIT paint gap) FAIL\n",
        ),
        None => log(
            "[raebridge] smoketest: edit-control -> CreateWindowEx(\"EDIT\") returned NULL (system-class create gap) FAIL\n",
        ),
    }

    // ── Phase 1.72: notepad-class CAPSTONE (the C3 gate, integrated) ──────
    // A real cl.exe Win32 .exe that uses EVERY notepad piece together: main
    // window + WndProc, system EDIT child, File menu (Save/Exit), types into the
    // EDIT, then a menu-driven File->Save reads the EDIT via GetWindowTextW,
    // picks a path via GetSaveFileNameW, and WriteFile's it to C:\note.txt;
    // File->Exit ends the loop. We reset the shared message state first (a prior
    // GUI exe left quit_posted set), run the .exe, then read C:\note.txt back
    // through the VFS and assert "HI" — proving window+menu+EDIT+dialog+save
    // integrate end-to-end. FAIL-able: any piece breaking flips the verdict.
    {
        let notepad = testpe::GUI_NOTEPAD_EXE;
        let loaded = match load_pe_executable(notepad) {
            Ok(l) => l,
            Err(e) => fail(
                &format!("[raebridge] host: FATAL: load gui-notepad exe: {e:?}\n"),
                0xE7,
            ),
        };
        if loaded.stubbed_imports != 0 {
            log(&format!(
                "[raebridge] smoketest: gui-notepad exe -> {} unresolved import(s) FAIL\n",
                loaded.stubbed_imports
            ));
        }
        reset_message_state();
        // SAFETY: gui_notepad.exe's custom entry returns (its loop ends on
        // WM_QUIT from File->Exit) and preserves callee-saved registers.
        unsafe {
            run_returning_image(notepad, "gui-notepad exe");
        }
        let mut buf = [0u8; 16];
        let vfs = raebridge::app_bucket_vfs_path("smoketest.exe", "C:\\note.txt");
        let fd = unsafe { sys_open(vfs.as_bytes(), 0) };
        if fd >= u64::MAX - 3 {
            log("[raebridge] smoketest: gui-notepad exe -> C:\\note.txt not created (menu/save flow gap) FAIL\n");
        } else {
            let n = unsafe { sys_read(fd, &mut buf) } as usize;
            if &buf[..n.min(buf.len())] == b"HI" {
                log("[raebridge] smoketest: gui-notepad exe -> window+EDIT+menu File->Save (GetSaveFileNameW) -> C:\\note.txt 'HI' PASS\n");
            } else {
                log(&format!(
                    "[raebridge] smoketest: gui-notepad exe -> readback {} bytes, want 'HI' FAIL\n",
                    n
                ));
            }
        }
    }

    // ── Phase 1.75: gs-base image (TEB via GS base + survival) ────────────
    // The foundation for real MSVC-CRT .exe startup: a PE whose entry reads
    // gs:[0x30] (the TEB self-pointer the loader's SYS_SET_GS_BASE installed),
    // verifies it is non-zero + self-consistent, writes a TEB scratch sentinel,
    // forces a context switch (SYS_YIELD), then re-reads gs:[0x30] and the
    // sentinel — proving the scheduler restored the GS base across the switch.
    // The entry returns a verdict code in RAX (0 = PASS). We also surface the
    // loader's W^X flip + set_gs_base flags so a kernel-side miss is named.
    {
        let gs_exe = testpe::build_gsbase_exe();
        let loaded = match load_pe_executable(&gs_exe) {
            Ok(l) => l,
            Err(e) => fail(
                &format!("[raebridge] host: FATAL: load gs-base exe: {e:?}\n"),
                0xE3,
            ),
        };
        if loaded.resolved_imports != testpe::GSBASE_IMPORTS.len() || loaded.stubbed_imports != 0 {
            fail(
                &format!(
                    "[raebridge] host: FATAL: gs-base exe imports resolved={} stubbed={} (want {} / 0)\n",
                    loaded.resolved_imports,
                    loaded.stubbed_imports,
                    testpe::GSBASE_IMPORTS.len()
                ),
                0xE4,
            );
        }
        if !loaded.gs_base_set {
            // SYS_SET_GS_BASE failed outright — the guest read would be stale.
            log("[raebridge] smoketest: gs-base exe -> SYS_SET_GS_BASE refused (kernel side missing?) FAIL\n");
        }
        // SAFETY: build_gsbase_exe's entry returns a u64 verdict and preserves
        // callee-saved registers (it saves/restores rsi). `loaded` (and thus the
        // process_env the GS base points at) is held alive across this call.
        let verdict = unsafe {
            let entry: extern "win64" fn() -> u64 =
                core::mem::transmute(loaded.entry_point as usize);
            entry()
        };
        let wx = if loaded.wx_flip_ok {
            "RX"
        } else {
            "RWX(no-flip)"
        };
        if verdict == testpe::GSBASE_PASS && loaded.gs_base_set {
            log(&format!(
                "[raebridge] smoketest: gs-base exe -> gs:[0x30]==TEB survives reschedule + exit 0 PASS (text={wx})\n"
            ));
        } else {
            // Name the exact failing check (decodable from serial alone).
            log(&format!(
                "[raebridge] smoketest: gs-base exe -> check {} FAILED (1=TEB-zero 2=self-ptr 3=not-restored 4=sentinel) gs_set={} FAIL\n",
                verdict, loaded.gs_base_set
            ));
        }
        // `loaded` drops here (after the guest finished); the next image installs
        // its own GS base.
    }

    // ── Phase 2: exit-code image (ExitProcess marshaling) ─────────────────
    // The hand-built ExitProcess(42) image. Its shim emits
    // "[raebridge] guest ExitProcess(42) -> exit 42 PASS". We DON'T let it be
    // the terminator anymore (the real-exe phase below is): we still resolve +
    // map + verify it, proving the ExitProcess IAT path, then proceed. (The
    // real /MT CRT also exercises ExitProcess, so termination semantics stay
    // proven by Phase 3.)
    log("[raebridge] host: mapping exit-code exe (ExitProcess IAT check)\n");
    let exit_exe = testpe::build_exit_process_exe();
    match load_pe_executable(&exit_exe) {
        Ok(l) if l.resolved_imports > 0 => {
            log("[raebridge] smoketest: exit-code exe -> ExitProcess resolved to shim PASS\n");
        }
        Ok(_) => fail(
            "[raebridge] host: FATAL: ExitProcess did not resolve to a shim\n",
            0xE4,
        ),
        Err(e) => fail(
            &format!("[raebridge] host: FATAL: load exit-code exe: {e:?}\n"),
            0xE3,
        ),
    }

    // ── Phase 3: REAL MSVC /MT WriteFile exe (load + import-resolve proof) ─
    // A genuine cl.exe-compiled PE32+ (73 KERNEL32 imports; see
    // fixtures/real_msvc_mt_hello.dumpbin.txt). Only ONE real CRT exe can run to
    // its ExitProcess per host process (it terminates us), and Phase 4 (printf)
    // is now that terminator. So here we map the WriteFile fixture and REQUIRE
    // every import resolved to a real shim (zero stubs) — proving the WriteFile
    // milestone's full IAT still patches — without executing its CRT.
    log("[raebridge] host: loading REAL MSVC /MT WriteFile exe (import-resolve proof)\n");
    let real_exe = testpe::REAL_MSVC_MT_EXE;
    match load_pe_executable(real_exe) {
        Ok(l) => {
            log(&format!(
                "[raebridge] host: WriteFile exe mapped: resolved={} stubbed={} pdata_funcs={} wx={} gs={}\n",
                l.resolved_imports, l.stubbed_imports, l.runtime_functions.len(), l.wx_flip_ok, l.gs_base_set,
            ));
            if l.stubbed_imports != 0 {
                log(&format!(
                    "[raebridge] smoketest: real MSVC /MT exe -> {} imports UNRESOLVED (would fail-loud before main) FAIL\n",
                    l.stubbed_imports
                ));
            } else {
                log("[raebridge] smoketest: real MSVC /MT exe -> all imports resolved to shims PASS\n");
            }
        }
        Err(e) => fail(
            &format!("[raebridge] host: FATAL: load real MSVC exe: {e:?}\n"),
            0xE3,
        ),
    }

    // ── Phase 4: REAL MSVC /MT printf .exe (load + import-resolve proof) ──
    // A genuine cl.exe-compiled PE32+ (74 KERNEL32 imports; see
    // fixtures/real_msvc_mt_printf.dumpbin.txt — exactly one more than the
    // WriteFile fixture: GetFileSizeEx). It drives the MSVC CRT buffered-stdio +
    // FORMAT engine (printf -> __stdio_common_vfprintf -> _write -> WriteFile).
    // Only ONE real CRT exe can run-to-ExitProcess per host process (it
    // terminates us), and Phase 5 (the C++ exe) is now that terminator — the C++
    // static-initializer proof is the higher-value milestone. So here we map the
    // printf fixture and REQUIRE every import resolved to a real shim (zero
    // stubs) — proving the printf/format-engine fixture's full IAT still patches
    // — without executing its CRT.
    log("[raebridge] host: loading REAL MSVC /MT printf exe (import-resolve proof)\n");
    let printf_exe = testpe::REAL_MSVC_MT_PRINTF_EXE;
    match load_pe_executable(printf_exe) {
        Ok(l) => {
            log(&format!(
                "[raebridge] host: printf exe mapped: resolved={} stubbed={} pdata_funcs={} wx={} gs={}\n",
                l.resolved_imports, l.stubbed_imports, l.runtime_functions.len(), l.wx_flip_ok, l.gs_base_set,
            ));
            if l.stubbed_imports != 0 {
                log(&format!(
                    "[raebridge] smoketest: real MSVC /MT printf exe -> {} imports UNRESOLVED (would fail-loud before main) FAIL\n",
                    l.stubbed_imports
                ));
            } else {
                log("[raebridge] smoketest: real MSVC /MT printf exe -> all imports resolved to shims PASS\n");
            }
        }
        Err(e) => fail(
            &format!("[raebridge] host: FATAL: load real MSVC printf exe: {e:?}\n"),
            0xE3,
        ),
    }

    // ── Phase 5: REAL MSVC /MT C++ .exe (THE C++-runtime milestone) ───────
    // A genuine cl.exe-compiled C++ PE32+ (see fixtures/real_msvc_mt_cpp.cpp +
    // .dumpbin.txt — imports the SAME 74 KERNEL32 functions as the printf
    // fixture, ZERO new imports: the C++ static-ctor table walk, atexit/onexit
    // registration, and the C++ EH personality are all CRT-INTERNAL in /MT).
    // This is the broadening to C++ — most real Windows software is C++.
    //
    // The fixture has a namespace-scope object `g_init` with a non-trivial ctor
    // that prints "ctor ran". The MSVC /MT CRT, before main, runs
    // __scrt_common_main_seh -> _initterm over the .CRT$XC* table, invoking
    // g_init's ctor; ONLY THEN does it call main, which prints "hello from c++
    // 7". We map it, REQUIRE zero stubbed imports (else startup traps before the
    // ctors run and names the import), enable the stdout tee, arm the C++
    // milestone (kind=3), and jump to the genuine CRT entry. The CRT walks the
    // static-ctor table (-> "ctor ran"), calls main (-> "hello from c++ 7"),
    // returns 0, and tail-calls ExitProcess(0). shim_exit_process (armed,
    // kind=3) asserts captured stdout == REAL_CPP_MSG (BOTH lines, in order —
    // FAIL if the ctor line is missing, proving static-init, not just main) and
    // emits:
    //   [raebridge] smoketest: real MSVC /MT C++ exe -> static-ctor ran + main
    //     + exit 0 PASS
    // then terminates the host with exit 0. This is the process terminator.
    log("[raebridge] host: loading REAL MSVC /MT C++ exe (the C++-runtime milestone)\n");
    let cpp_exe = testpe::REAL_MSVC_MT_CPP_EXE;
    let loaded = match load_pe_executable(cpp_exe) {
        Ok(l) => l,
        Err(e) => fail(
            &format!("[raebridge] host: FATAL: load real MSVC C++ exe: {e:?}\n"),
            0xE3,
        ),
    };
    log(&format!(
        "[raebridge] host: C++ exe mapped: resolved={} stubbed={} pdata_funcs={} wx={} gs={}\n",
        loaded.resolved_imports,
        loaded.stubbed_imports,
        loaded.runtime_functions.len(),
        loaded.wx_flip_ok,
        loaded.gs_base_set,
    ));
    if loaded.stubbed_imports != 0 {
        // A stubbed import means startup WILL hit a fail-loud trap before the
        // static ctors run. Name the count loud and do NOT pretend we got there.
        fail(
            &format!(
                "[raebridge] smoketest: real MSVC /MT C++ exe -> {} imports UNRESOLVED (would fail-loud before static-init) FAIL\n",
                loaded.stubbed_imports
            ),
            0xE5,
        );
    }
    enable_stdout_capture();
    arm_real_cpp_milestone();
    log("[raebridge] host: jumping to REAL MSVC CRT entry point (C++ static-init)\n");
    // SAFETY: entry_point is the AddressOfEntryPoint of the real C++ PE we just
    // mapped into executable user pages with a patched IAT, GS base pointing at
    // a live TEB, and W^X-flipped .text. The MSVC CRT entry never returns (it
    // ExitProcess'es); `loaded` (holding the process_env the GS base points at)
    // is intentionally leaked by this divergent call so the TEB outlives the
    // guest.
    unsafe {
        let entry: unsafe extern "win64" fn() -> ! =
            core::mem::transmute(loaded.entry_point as usize);
        core::mem::forget(loaded);
        entry()
    }
}
