//! `extern "win64"` entry points for PE import resolution.
//!
//! Concept §Compatibility Strategy: "AthBridge runs Windows apps on day
//! one." This module is the execution half of that promise: each function
//! here is ABI-compatible with the real Win32 export of the same name, so
//! the PE loader can write its address directly into a loaded image's IAT
//! slot. When the Windows code executes `call qword [IAT]`, control arrives
//! here with arguments already in RCX/RDX/R8/R9 per the Microsoft x64
//! calling convention — rustc's `extern "win64"` does the marshaling that
//! MasterChecklist Phase 11.2 calls the "calling convention marshaling
//! layer".
//!
//! Design notes:
//!   • The Windows process runs *inside* the AthBridge host process (Wine
//!     model), so guest pointers are host pointers and can be dereferenced
//!     directly. No VA translation layer.
//!   • All shims funnel through one process-global [`FullCompatSession`],
//!     installed once by the host via [`install_host_context`]. The context
//!     lives in a `Box` — constructing a `FullCompatSession` on the stack
//!     has double-faulted before (MasterChecklist Phase 11 known issue).
//!   • A shim called before installation fails loud: it prints a marker via
//!     `SYS_DEBUG_PRINT` and terminates with exit code 0xDEAD. Silently
//!     returning garbage to Windows code is how unkillable heisenbugs are
//!     born.

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use crate::comdlg32 as c;
use crate::gdi32 as g;
use crate::kernel32 as k;
use crate::user32 as u;
use crate::{syscalls, DWord, FullCompatSession, WinHandle};

// ---------------------------------------------------------------------------
// Process-global host context
// ---------------------------------------------------------------------------

struct HostCtxCell {
    lock: AtomicBool,
    ctx: UnsafeCell<Option<Box<FullCompatSession>>>,
}

// SAFETY: every access to `ctx` goes through `with_ctx`/`install_host_context`,
// which serialize on the `lock` spinlock before touching the UnsafeCell.
unsafe impl Sync for HostCtxCell {}

static HOST_CTX: HostCtxCell = HostCtxCell {
    lock: AtomicBool::new(false),
    ctx: UnsafeCell::new(None),
};

/// Install the process-global compat session. Called once by the AthBridge
/// host before jumping to a PE entry point. Returns the previous session if
/// one was installed (callers should treat `Some` as a logic error).
pub fn install_host_context(ctx: Box<FullCompatSession>) -> Option<Box<FullCompatSession>> {
    while HOST_CTX
        .lock
        .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        core::hint::spin_loop();
    }
    // SAFETY: spinlock held — exclusive access to the cell.
    let prev = unsafe { (*HOST_CTX.ctx.get()).replace(ctx) };
    HOST_CTX.lock.store(false, Ordering::Release);
    prev
}

/// True once a host context is installed (used by smoketests).
pub fn host_context_installed() -> bool {
    while HOST_CTX
        .lock
        .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        core::hint::spin_loop();
    }
    // SAFETY: spinlock held — exclusive access to the cell.
    let installed = unsafe { (*HOST_CTX.ctx.get()).is_some() };
    HOST_CTX.lock.store(false, Ordering::Release);
    installed
}

/// Pixels a guest has rastered into window surfaces so far (FillRect + TextOut).
/// The harness reads this after a GUI fixture runs to prove it actually painted
/// (a headless QEMU surface can't be eyeballed). Returns 0 if no ctx installed.
pub fn gui_paint_pixel_count() -> u64 {
    while HOST_CTX
        .lock
        .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        core::hint::spin_loop();
    }
    // SAFETY: spinlock held — exclusive read of the cell.
    let n = unsafe {
        (*HOST_CTX.ctx.get())
            .as_ref()
            .map(|c| c.gui_paint_pixels)
            .unwrap_or(0)
    };
    HOST_CTX.lock.store(false, Ordering::Release);
    n
}

/// In-process EDIT-control self-test. Run by the host harness IN QEMU — where
/// the surface syscalls are real — NOT from a host `cargo test` (it executes a
/// real `syscall` via `CreateWindowExW`). Exercises the full real path a true
/// Notepad uses: create an unregistered system `"EDIT"` child, type "HI" through
/// the standard pump (`PostMessage` WM_KEYDOWN → `GetMessage` → `TranslateMessage`
/// → `DispatchMessage` → built-in EDIT proc), then read the text back with
/// `GetWindowTextW`, then drive a WM_PAINT so the EDIT renders its text. Returns
/// `Some((text_ok, painted))`: `text_ok` = the "HI" round-trip succeeded;
/// `painted` = the WM_PAINT rendered pixels into the EDIT surface. `None` = the
/// EDIT window could not be created.
///
/// # Safety
/// Must run inside the AthBridge host process (a real syscall ABI) with a host
/// context installed; never from a host unit test.
pub unsafe fn gui_edit_control_selftest() -> Option<(bool, bool)> {
    let cls = crate::string_to_wide("EDIT");
    let name = crate::string_to_wide("");
    let hwnd =
        shim_create_window_ex_w(0, cls.as_ptr(), name.as_ptr(), 0, 0, 0, 120, 24, 0, 0, 0, 0);
    if hwnd == 0 {
        return None;
    }
    // The host harness shares ONE process-global ctx across phases; an earlier
    // GUI exe ended its loop with PostQuitMessage, leaving `quit_posted` set and
    // possibly stale messages queued. A real app starts its own message loop with
    // a fresh quit state, so reset it here — otherwise the first GetMessage below
    // returns WM_QUIT immediately and nothing gets typed.
    with_ctx(|ctx| {
        ctx.quit_posted = false;
        ctx.message_queue.clear();
    });
    // Type "HI" via the real input pump (VK letter codes ARE their ASCII value).
    shim_post_message_w(hwnd, crate::WM_KEYDOWN, b'H' as u64, 0);
    shim_post_message_w(hwnd, crate::WM_KEYDOWN, b'I' as u64, 0);
    for _ in 0..32 {
        let mut buf = [0u8; 48];
        let got = shim_get_message_w(buf.as_mut_ptr(), 0, 0, 0);
        let m = read_guest_msg(buf.as_ptr());
        if got == 0 || m.message == 0 {
            break;
        }
        shim_translate_message(buf.as_ptr());
        shim_dispatch_message_w(buf.as_ptr());
    }
    let mut out = [0u16; 16];
    let n = shim_get_window_text_w(hwnd, out.as_mut_ptr(), out.len() as i32);
    let text = crate::wide_to_string(&out[..(n as usize).min(out.len())]);
    // Render the EDIT's text into its surface via the built-in WM_PAINT, and
    // confirm pixels were actually painted (the SW-render half). WM_PAINT routes
    // to the built-in EDIT proc through `DispatchMessage`'s classifier.
    let before = gui_paint_pixel_count();
    let paint = crate::Msg {
        hwnd: WinHandle(hwnd),
        message: crate::WM_PAINT,
        wparam: 0,
        lparam: 0,
        time: 0,
        pt: crate::Point { x: 0, y: 0 },
    };
    let mut pbuf = [0u8; 48];
    write_guest_msg(pbuf.as_mut_ptr(), &paint);
    shim_dispatch_message_w(pbuf.as_ptr());
    let painted = gui_paint_pixel_count() > before;
    Some((text == "HI", painted))
}

/// Reset the message-loop state (clear `quit_posted` + drain the queue) so a
/// freshly-launched guest starts its message loop clean. The host harness shares
/// ONE process-global ctx across phases, and a prior GUI exe ends with
/// PostQuitMessage — without this, the next guest's first GetMessage returns
/// WM_QUIT immediately. A real per-process launcher gets a fresh ctx, so this is
/// a harness-only concern.
pub fn reset_message_state() {
    with_ctx(|ctx| {
        ctx.quit_posted = false;
        ctx.message_queue.clear();
    });
}

// ---------------------------------------------------------------------------
// Stdout capture tee (smoketest proof)
// ---------------------------------------------------------------------------
//
// To *prove* that guest Windows code emitted bytes via WriteFile — rather than
// trusting the serial port we can't read back from inside the process — the
// AthBridge smoketest enables a capture tee. While enabled, every byte
// `shim_write_file` successfully writes to the seeded stdout fd is also
// appended to this buffer. The harness then asserts the captured bytes equal
// the image's expected output. Off in production (the tee allocates), so it
// adds nothing to the daily-driver WriteFile path.

struct StdoutCapture {
    lock: AtomicBool,
    enabled: AtomicBool,
    buf: UnsafeCell<Vec<u8>>,
}

// SAFETY: `buf` is only touched while `lock` is held.
unsafe impl Sync for StdoutCapture {}

static STDOUT_CAPTURE: StdoutCapture = StdoutCapture {
    lock: AtomicBool::new(false),
    enabled: AtomicBool::new(false),
    buf: UnsafeCell::new(Vec::new()),
};

fn capture_lock() {
    while STDOUT_CAPTURE
        .lock
        .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        core::hint::spin_loop();
    }
}

/// Begin tee-ing successful stdout writes into the capture buffer (and clear
/// any prior contents). Called by the smoketest before running an image.
pub fn enable_stdout_capture() {
    capture_lock();
    // SAFETY: lock held — exclusive access to the buffer.
    unsafe {
        (*STDOUT_CAPTURE.buf.get()).clear();
    }
    STDOUT_CAPTURE.enabled.store(true, Ordering::Release);
    STDOUT_CAPTURE.lock.store(false, Ordering::Release);
}

/// Stop tee-ing and return a copy of the captured bytes.
pub fn take_captured_stdout() -> Vec<u8> {
    capture_lock();
    STDOUT_CAPTURE.enabled.store(false, Ordering::Release);
    // SAFETY: lock held — exclusive access to the buffer.
    let out = unsafe { (*STDOUT_CAPTURE.buf.get()).clone() };
    STDOUT_CAPTURE.lock.store(false, Ordering::Release);
    out
}

/// Peek the captured stdout bytes without disabling the tee or clearing them.
/// Used by the real-MSVC-exe milestone, whose verdict is emitted from inside
/// `shim_exit_process` (the real CRT terminates via `ExitProcess`, so there is
/// no post-return point to assert from).
pub fn peek_captured_stdout() -> Vec<u8> {
    capture_lock();
    // SAFETY: lock held — exclusive access to the buffer.
    let out = unsafe { (*STDOUT_CAPTURE.buf.get()).clone() };
    STDOUT_CAPTURE.lock.store(false, Ordering::Release);
    out
}

// ---------------------------------------------------------------------------
// Real-MSVC-exe milestone arm
// ---------------------------------------------------------------------------
//
// The real /MT CRT ends `mainCRTStartup` by tail-calling `ExitProcess(main())`,
// so control never returns to the host after the entry jump — the verdict must
// be emitted from inside `shim_exit_process`. The runner arms this flag right
// before jumping to the real exe's entry; when ExitProcess then fires, the
// handler checks the captured stdout against `testpe::REAL_EXE_MSG` and prints
// the FAIL-able milestone line. Reaching ExitProcess AT ALL proves the CRT ran
// to (and through) `main` — a binary that faulted before main never gets here.

static REAL_EXE_MILESTONE_ARMED: AtomicBool = AtomicBool::new(false);

/// Which real-MSVC-exe milestone is armed for the next ExitProcess. The two real
/// CRT fixtures differ only in the expected captured stdout and the verdict line;
/// only one can run-to-ExitProcess per host process, so the runner picks one.
/// 0 = none, 1 = WriteFile fixture (`REAL_EXE_MSG`), 2 = printf fixture
/// (`REAL_PRINTF_MSG`, drives the CRT format engine), 3 = C++ fixture
/// (`REAL_CPP_MSG`, drives the C++ static-initializer table walk before main).
static REAL_EXE_MILESTONE_KIND: AtomicU32 = AtomicU32::new(0);

/// Arm the real-MSVC-exe (WriteFile fixture) milestone verdict, emitted on the
/// next ExitProcess. Expected stdout = `testpe::REAL_EXE_MSG`.
pub fn arm_real_exe_milestone() {
    REAL_EXE_MILESTONE_KIND.store(1, Ordering::SeqCst);
    REAL_EXE_MILESTONE_ARMED.store(true, Ordering::SeqCst);
}

/// Arm the real-MSVC-printf-exe milestone verdict, emitted on the next
/// ExitProcess. Expected stdout = `testpe::REAL_PRINTF_MSG` — the bytes the CRT
/// buffered-stdio + format engine produces from `printf("...%d %s\n", 42, ...)`.
pub fn arm_real_printf_milestone() {
    REAL_EXE_MILESTONE_KIND.store(2, Ordering::SeqCst);
    REAL_EXE_MILESTONE_ARMED.store(true, Ordering::SeqCst);
}

/// Arm the real-MSVC-**C++**-exe milestone verdict, emitted on the next
/// ExitProcess. Expected stdout = `testpe::REAL_CPP_MSG` — the two lines the C++
/// CRT produces: the static initializer's `"ctor ran"` (printed by `_initterm`
/// over the `.CRT$XC*` table BEFORE main) followed by `main`'s
/// `"hello from c++ 7"`. Reaching ExitProcess with BOTH lines, in order, proves
/// the C++ static-init machinery ran — the C++-runtime broadening over plain C.
pub fn arm_real_cpp_milestone() {
    REAL_EXE_MILESTONE_KIND.store(3, Ordering::SeqCst);
    REAL_EXE_MILESTONE_ARMED.store(true, Ordering::SeqCst);
}

/// Armed ONLY for the hand-built `ExitProcess(42)` sentinel fixture
/// (`testpe::build_exit_process_exe`). When set, the next `shim_exit_process`
/// asserts the arriving code IS 42 — proving the guest moved the sentinel into
/// ECX and it propagated through the IAT call. A REAL `.exe` exits with its OWN
/// (correct) code, so it must NOT be checked against 42 — that conflation made a
/// real exe's clean `exit 0` print a spurious `FAIL` in the boot log. Default
/// (unarmed) → a neutral "reached ExitProcess(N)" report; the code is validated
/// by the parent's reap (see `user_init`).
static EXIT42_SENTINEL_ARMED: AtomicBool = AtomicBool::new(false);

/// Arm the `ExitProcess(42)` sentinel verdict for the next ExitProcess (the
/// `Target::BundledExit42` launcher path). See [`EXIT42_SENTINEL_ARMED`].
pub fn arm_exit42_milestone() {
    EXIT42_SENTINEL_ARMED.store(true, Ordering::SeqCst);
}

/// Append bytes the guest wrote to stdout into the capture buffer, if the tee
/// is enabled. No-op (one relaxed load) when disabled.
fn capture_stdout(bytes: &[u8]) {
    if !STDOUT_CAPTURE.enabled.load(Ordering::Acquire) {
        return;
    }
    capture_lock();
    // SAFETY: lock held — exclusive access to the buffer.
    unsafe {
        (*STDOUT_CAPTURE.buf.get()).extend_from_slice(bytes);
    }
    STDOUT_CAPTURE.lock.store(false, Ordering::Release);
}

/// Print a marker to the kernel serial port, then terminate the process.
/// Used when Windows code reaches a shim in an unrunnable state.
fn die(msg: &str) -> ! {
    // SAFETY: SYS_DEBUG_PRINT only reads the buffer; sys_exit never returns.
    unsafe {
        syscalls::sys_debug_print(msg.as_bytes());
        syscalls::sys_exit(0xDEAD);
    }
}

fn with_ctx<R>(f: impl FnOnce(&mut FullCompatSession) -> R) -> R {
    while HOST_CTX
        .lock
        .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        core::hint::spin_loop();
    }
    // SAFETY: spinlock held — exclusive access to the cell for the whole call.
    let result = unsafe {
        match (*HOST_CTX.ctx.get()).as_mut() {
            Some(ctx) => f(ctx),
            None => {
                HOST_CTX.lock.store(false, Ordering::Release);
                die("[raebridge] FATAL: Win32 shim called before install_host_context\n");
            }
        }
    };
    HOST_CTX.lock.store(false, Ordering::Release);
    result
}

// ---------------------------------------------------------------------------
// Pointer helpers (guest VA == host VA in the Wine-model host)
// ---------------------------------------------------------------------------

/// Length of a NUL-terminated UTF-16 string, capped so a missing terminator
/// cannot walk off into unmapped memory forever.
const MAX_CSTR_LEN: usize = 64 * 1024;

unsafe fn wide_cstr<'a>(ptr: *const u16) -> &'a [u16] {
    if ptr.is_null() {
        return &[];
    }
    let mut len = 0usize;
    while len < MAX_CSTR_LEN && *ptr.add(len) != 0 {
        len += 1;
    }
    core::slice::from_raw_parts(ptr, len)
}

unsafe fn ansi_cstr<'a>(ptr: *const u8) -> &'a [u8] {
    if ptr.is_null() {
        return &[];
    }
    let mut len = 0usize;
    while len < MAX_CSTR_LEN && *ptr.add(len) != 0 {
        len += 1;
    }
    core::slice::from_raw_parts(ptr, len)
}

/// A nullable UTF-16 string argument (e.g. the `lpName` of `CreateMutexW`):
/// `NULL` → anonymous object (`None`), otherwise the NUL-terminated wide slice.
unsafe fn wide_cstr_opt<'a>(ptr: *const u16) -> Option<&'a [u16]> {
    if ptr.is_null() {
        None
    } else {
        Some(wide_cstr(ptr))
    }
}

// ---------------------------------------------------------------------------
// kernel32 shims — Phase B top-20
// ---------------------------------------------------------------------------

pub unsafe extern "win64" fn shim_exit_process(exit_code: u32) -> ! {
    // ── Real-MSVC-exe milestone ───────────────────────────────────────────
    // If armed, this ExitProcess is the real /MT CRT terminating after `main`.
    // Reaching here AT ALL means the genuine CRT startup ran to and through
    // main (a fault before main never reaches ExitProcess). The PASS condition:
    // the guest's `main` printed REAL_EXE_MSG via WriteFile (captured by the
    // tee) AND exited 0. This is THE "real Windows software runs" verdict.
    if REAL_EXE_MILESTONE_ARMED.swap(false, Ordering::SeqCst) {
        let kind = REAL_EXE_MILESTONE_KIND.swap(0, Ordering::SeqCst);
        let captured = peek_captured_stdout();
        match kind {
            2 => {
                // printf fixture: the CRT buffered-stdio + format engine ran.
                let stdout_ok = captured.as_slice() == crate::testpe::REAL_PRINTF_MSG;
                if stdout_ok && exit_code == 0 {
                    syscalls::sys_debug_print(
                        b"[raebridge] smoketest: real MSVC /MT printf exe -> stdout \"printf says hi 42 from a real exe\" + exit 0 PASS\n",
                    );
                } else {
                    let msg = alloc::format!(
                        "[raebridge] smoketest: real MSVC /MT printf exe -> reached printf but stdout_ok={} (got {} bytes) exit={} FAIL\n",
                        stdout_ok,
                        captured.len(),
                        exit_code
                    );
                    syscalls::sys_debug_print(msg.as_bytes());
                }
            }
            3 => {
                // C++ fixture: the C++ static-initializer table walk ran. PASS
                // requires BOTH the static ctor's line AND main's line, in order
                // (REAL_CPP_MSG = "ctor ran\r\nhello from c++ 7\r\n"). If only the
                // main line appears, static-init was skipped — FAIL loud. This is
                // THE "C++ runtime runs (static init fired before main)" verdict.
                let stdout_ok = captured.as_slice() == crate::testpe::REAL_CPP_MSG;
                if stdout_ok && exit_code == 0 {
                    syscalls::sys_debug_print(
                        b"[raebridge] smoketest: real MSVC /MT C++ exe -> static-ctor ran + main + exit 0 PASS\n",
                    );
                } else {
                    let ctor_seen = captured
                        .windows(crate::testpe::CPP_CTOR_LINE.len())
                        .any(|w| w == crate::testpe::CPP_CTOR_LINE);
                    let msg = alloc::format!(
                        "[raebridge] smoketest: real MSVC /MT C++ exe -> static-ctor ran + main + exit 0 (ctor_line_seen={} stdout_ok={} got {} bytes exit={}) FAIL\n",
                        ctor_seen,
                        stdout_ok,
                        captured.len(),
                        exit_code
                    );
                    syscalls::sys_debug_print(msg.as_bytes());
                }
            }
            _ => {
                // WriteFile fixture (default / kind 1).
                let stdout_ok = captured.as_slice() == crate::testpe::REAL_EXE_MSG;
                if stdout_ok && exit_code == 0 {
                    syscalls::sys_debug_print(
                        b"[raebridge] smoketest: real MSVC /MT exe -> reached main + stdout \"real windows exe\" + exit 0 PASS\n",
                    );
                } else {
                    let msg = alloc::format!(
                        "[raebridge] smoketest: real MSVC /MT exe -> reached main but stdout_ok={} (got {} bytes) exit={} FAIL\n",
                        stdout_ok,
                        captured.len(),
                        exit_code
                    );
                    syscalls::sys_debug_print(msg.as_bytes());
                }
            }
        }
        syscalls::sys_exit(exit_code as u64);
    }

    // Boot-log proof that Windows code reached a Win32 exit, and with what code.
    // The guest's `call [IAT:ExitProcess]` landed here, in real machine code
    // mapped + relocated + IAT-patched from a PE32+ image — reaching this point
    // AT ALL proves the binary executed (a fault before exit never gets here).
    //
    // Verdict split (fixes a real `.exe`'s clean exit printing a spurious FAIL):
    // ONLY the hand-built exit-42 SENTINEL fixture asserts a specific code
    // (==`TEST_EXE_EXIT_CODE`), and only when explicitly armed for it. Every
    // OTHER target is a real program exiting with its OWN (correct) code —
    // reaching ExitProcess is the proof and the parent reaps + validates the
    // code (see `user_init`), so we report it neutrally, with no PASS/FAIL claim.
    if EXIT42_SENTINEL_ARMED.swap(false, Ordering::SeqCst) {
        let verdict = if exit_code == crate::testpe::TEST_EXE_EXIT_CODE {
            "PASS"
        } else {
            // The sentinel propagated the WRONG code — a real corruption FAIL.
            "FAIL"
        };
        let msg = alloc::format!(
            "[raebridge] guest ExitProcess({}) -> exit {} {}\n",
            exit_code,
            exit_code,
            verdict
        );
        syscalls::sys_debug_print(msg.as_bytes());
    } else {
        let msg = alloc::format!(
            "[raebridge] guest ExitProcess({}) -> exit {} (reaped by parent)\n",
            exit_code,
            exit_code
        );
        syscalls::sys_debug_print(msg.as_bytes());
    }
    syscalls::sys_exit(exit_code as u64)
}

pub unsafe extern "win64" fn shim_get_last_error() -> u32 {
    with_ctx(|ctx| k::get_last_error(ctx).0)
}

pub unsafe extern "win64" fn shim_set_last_error(code: u32) {
    with_ctx(|ctx| k::set_last_error_api(ctx, DWord(code)))
}

pub unsafe extern "win64" fn shim_get_current_process_id() -> u32 {
    with_ctx(|ctx| k::get_current_process_id(ctx))
}

pub unsafe extern "win64" fn shim_get_current_thread_id() -> u32 {
    with_ctx(|ctx| k::get_current_thread_id(ctx))
}

pub unsafe extern "win64" fn shim_get_process_heap() -> u64 {
    with_ctx(|ctx| k::get_process_heap(ctx).0)
}

pub unsafe extern "win64" fn shim_heap_alloc(heap: u64, flags: u32, bytes: u64) -> u64 {
    with_ctx(|ctx| k::heap_alloc(ctx, WinHandle(heap), flags, bytes))
}

pub unsafe extern "win64" fn shim_heap_free(heap: u64, flags: u32, mem: u64) -> i32 {
    with_ctx(|ctx| k::heap_free(ctx, WinHandle(heap), flags, mem).0)
}

pub unsafe extern "win64" fn shim_virtual_alloc(
    address: u64,
    size: u64,
    allocation_type: u32,
    protect: u32,
) -> u64 {
    with_ctx(|ctx| k::virtual_alloc(ctx, address, size, allocation_type, protect))
}

pub unsafe extern "win64" fn shim_virtual_free(address: u64, size: u64, free_type: u32) -> i32 {
    with_ctx(|ctx| k::virtual_free(ctx, address, size, free_type).0)
}

pub unsafe extern "win64" fn shim_get_tick_count() -> u32 {
    with_ctx(|ctx| k::get_tick_count(ctx))
}

pub unsafe extern "win64" fn shim_get_tick_count_64() -> u64 {
    with_ctx(|ctx| k::get_tick_count_64(ctx))
}

pub unsafe extern "win64" fn shim_get_system_time_as_file_time(filetime: *mut u64) {
    if filetime.is_null() {
        return;
    }
    let t = with_ctx(|ctx| k::get_system_time_as_file_time(ctx));
    // FILETIME is two unaligned-tolerant u32s; write as one unaligned u64.
    filetime.write_unaligned(t);
}

pub unsafe extern "win64" fn shim_sleep(milliseconds: u32) {
    with_ctx(|ctx| k::sleep(ctx, milliseconds))
}

pub unsafe extern "win64" fn shim_output_debug_string_a(output_string: *const u8) {
    let bytes = ansi_cstr(output_string);
    syscalls::sys_debug_print(bytes);
}

pub unsafe extern "win64" fn shim_get_command_line_w() -> *const u16 {
    // The buffer lives in the Box'd session; its address is stable for the
    // process lifetime because nothing mutates `command_line_w` after
    // construction.
    with_ctx(|ctx| ctx.command_line_w.as_ptr())
}

pub unsafe extern "win64" fn shim_get_std_handle(std_handle: u32) -> u64 {
    with_ctx(|ctx| k::get_std_handle(ctx, std_handle).0)
}

pub unsafe extern "win64" fn shim_close_handle(handle: u64) -> i32 {
    with_ctx(|ctx| k::close_handle(ctx, WinHandle(handle)).0)
}

pub unsafe extern "win64" fn shim_create_file_w(
    file_name: *const u16,
    desired_access: u32,
    share_mode: u32,
    security_attributes: u64,
    creation_disposition: u32,
    flags_and_attributes: u32,
    template_file: u64,
) -> u64 {
    let name = wide_cstr(file_name);
    with_ctx(|ctx| {
        k::create_file_w(
            ctx,
            name,
            desired_access,
            share_mode,
            security_attributes,
            creation_disposition,
            flags_and_attributes,
            WinHandle(template_file),
        )
        .0
    })
}

pub unsafe extern "win64" fn shim_read_file(
    handle: u64,
    buffer: *mut u8,
    bytes_to_read: u32,
    bytes_read: *mut u32,
    overlapped: u64,
) -> i32 {
    if buffer.is_null() && bytes_to_read != 0 {
        return 0;
    }
    let buf = core::slice::from_raw_parts_mut(buffer, bytes_to_read as usize);
    let mut local_read: u32 = 0;
    let ok = with_ctx(|ctx| {
        k::read_file(
            ctx,
            WinHandle(handle),
            buf,
            bytes_to_read,
            &mut local_read,
            overlapped,
        )
        .0
    });
    if !bytes_read.is_null() {
        bytes_read.write_unaligned(local_read);
    }
    ok
}

pub unsafe extern "win64" fn shim_write_file(
    handle: u64,
    buffer: *const u8,
    bytes_to_write: u32,
    bytes_written: *mut u32,
    overlapped: u64,
) -> i32 {
    if buffer.is_null() && bytes_to_write != 0 {
        return 0;
    }
    let buf = core::slice::from_raw_parts(buffer, bytes_to_write as usize);
    let mut local_written: u32 = 0;
    let (ok, is_stdout) = with_ctx(|ctx| {
        // Tee only writes to the *stdout* fd (native_id 1) so the capture buffer
        // reflects exactly what reached the console, not arbitrary file writes.
        let is_stdout = ctx.handle_table.get(handle).and_then(|e| e.native_id) == Some(1);
        let r = k::write_file(
            ctx,
            WinHandle(handle),
            buf,
            bytes_to_write,
            &mut local_written,
            overlapped,
        )
        .0;
        (r, is_stdout)
    });
    if ok != 0 && is_stdout {
        capture_stdout(&buf[..local_written as usize]);
    }
    if !bytes_written.is_null() {
        bytes_written.write_unaligned(local_written);
    }
    ok
}

// ---------------------------------------------------------------------------
// Module / proc resolution (CRT startup: GetModuleHandle*, GetProcAddress)
// ---------------------------------------------------------------------------

/// The HMODULE for `GetModuleHandle(NULL)` — the REAL mapped base of the main
/// executable when the loader recorded one (so the CRT's post-main PE-header
/// walk hits mapped memory), else the synthetic `MAIN_MODULE_BASE` for the
/// hand-built bring-up images that aren't run through `load_pe_executable`.
fn main_module_handle() -> u64 {
    let real = crt::main_module_base();
    if real != 0 {
        real
    } else {
        crate::MAIN_MODULE_BASE
    }
}

pub unsafe extern "win64" fn shim_get_module_handle_w(module_name: *const u16) -> u64 {
    if module_name.is_null() {
        // NULL => handle of the main executable (its real mapped base).
        return main_module_handle();
    }
    let name = wide_cstr(module_name);
    with_ctx(|ctx| k::get_module_handle_w(ctx, Some(name)).0)
}

pub unsafe extern "win64" fn shim_get_module_handle_a(module_name: *const u8) -> u64 {
    if module_name.is_null() {
        return main_module_handle();
    }
    let name = ansi_cstr(module_name);
    with_ctx(|ctx| k::get_module_handle_a(ctx, Some(name)).0)
}

/// `GetModuleHandleExW` — flags in ECX, name in RDX, out-handle in R8.
/// We honor the common `GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS` (0x4) and the
/// plain-name forms the CRT uses; on success we store the base in `*module`.
pub unsafe extern "win64" fn shim_get_module_handle_ex_w(
    flags: u32,
    module_name: *const u16,
    module: *mut u64,
) -> i32 {
    const FROM_ADDRESS: u32 = 0x0000_0004;
    let base = if flags & FROM_ADDRESS != 0 {
        // module_name is actually an address inside a module (the CRT's __scrt
        // passes its own &main). Map it to its real image base via the module
        // registry; fall back to the main module if it isn't covered.
        let addr = module_name as u64;
        crt::module_for_pc(addr)
            .map(|m| m.base)
            .unwrap_or_else(main_module_handle)
    } else if module_name.is_null() {
        main_module_handle()
    } else {
        let name = wide_cstr(module_name);
        with_ctx(|ctx| k::get_module_handle_w(ctx, Some(name)).0)
    };
    if !module.is_null() {
        module.write_unaligned(base);
    }
    if base == 0 {
        0
    } else {
        1
    }
}

pub unsafe extern "win64" fn shim_get_module_file_name_w(
    module: u64,
    filename: *mut u16,
    size: u32,
) -> u32 {
    if filename.is_null() || size == 0 {
        return 0;
    }
    let buf = core::slice::from_raw_parts_mut(filename, size as usize);
    with_ctx(|ctx| k::get_module_file_name_w(ctx, WinHandle(module), buf))
}

pub unsafe extern "win64" fn shim_get_proc_address(module: u64, proc_name: *const u8) -> u64 {
    if proc_name.is_null() {
        return 0;
    }
    // Ordinal imports arrive with the high bits zero and the name pointer
    // actually being a small integer (< 0x10000). We don't host ordinal-only
    // exports, so treat those as not-found rather than dereferencing.
    if (proc_name as u64) < 0x1_0000 {
        return 0;
    }
    let bytes = ansi_cstr(proc_name);
    let name = match core::str::from_utf8(bytes) {
        Ok(s) => s,
        Err(_) => return 0,
    };
    with_ctx(|ctx| k::get_proc_address(ctx, WinHandle(module), name))
}

// ---------------------------------------------------------------------------
// Heap (CRT /MT backs all allocation on the process heap)
// ---------------------------------------------------------------------------

pub unsafe extern "win64" fn shim_heap_realloc(heap: u64, flags: u32, mem: u64, bytes: u64) -> u64 {
    with_ctx(|ctx| k::heap_realloc(ctx, WinHandle(heap), flags, mem, bytes))
}

pub unsafe extern "win64" fn shim_heap_size(heap: u64, _flags: u32, mem: u64) -> u64 {
    with_ctx(|ctx| {
        if ctx.virtual_regions.get(&heap).is_none() {
            return u64::MAX; // (SIZE_T)-1 on error
        }
        match ctx.heap_allocations.get(&mem) {
            Some(&(size, _)) => size,
            None => u64::MAX,
        }
    })
}

pub unsafe extern "win64" fn shim_heap_create(
    options: u32,
    initial_size: u64,
    maximum_size: u64,
) -> u64 {
    with_ctx(|ctx| k::heap_create(ctx, options, initial_size, maximum_size).0)
}

pub unsafe extern "win64" fn shim_heap_destroy(heap: u64) -> i32 {
    with_ctx(|ctx| k::heap_destroy(ctx, WinHandle(heap)).0)
}

// ---------------------------------------------------------------------------
// TLS (CRT __scrt sets up its own TLS slot before main)
// ---------------------------------------------------------------------------

pub unsafe extern "win64" fn shim_tls_alloc() -> u32 {
    with_ctx(|ctx| k::tls_alloc(ctx))
}

pub unsafe extern "win64" fn shim_tls_free(index: u32) -> i32 {
    with_ctx(|ctx| k::tls_free(ctx, index).0)
}

pub unsafe extern "win64" fn shim_tls_get_value(index: u32) -> u64 {
    with_ctx(|ctx| k::tls_get_value(ctx, index))
}

pub unsafe extern "win64" fn shim_tls_set_value(index: u32, value: u64) -> i32 {
    with_ctx(|ctx| k::tls_set_value(ctx, index, value).0)
}

// ---------------------------------------------------------------------------
// Environment / startup
// ---------------------------------------------------------------------------

pub unsafe extern "win64" fn shim_get_command_line_a() -> *const u8 {
    with_ctx(|ctx| ctx.command_line_a.as_ptr())
}

/// Build (once, lazily) the double-NUL-terminated UTF-16 environment block and
/// return a pointer to it. The block is owned by the session for its lifetime.
pub unsafe extern "win64" fn shim_get_environment_strings_w() -> *const u16 {
    with_ctx(|ctx| {
        if ctx.environment_block_w.is_empty() {
            let mut block: Vec<u16> = Vec::new();
            for (k_, v) in ctx.environment.iter() {
                for c in k_.encode_utf16() {
                    block.push(c);
                }
                block.push(b'=' as u16);
                for c in v.encode_utf16() {
                    block.push(c);
                }
                block.push(0);
            }
            // Final extra NUL terminates the whole block.
            block.push(0);
            ctx.environment_block_w = block;
        }
        ctx.environment_block_w.as_ptr()
    })
}

pub unsafe extern "win64" fn shim_free_environment_strings_w(_env: *const u16) -> i32 {
    // The block is session-owned; freeing is a no-op success (the pointer the
    // guest holds stays valid until the session ends).
    1
}

pub unsafe extern "win64" fn shim_get_startup_info_w(info: *mut k::StartupInfoW) {
    if info.is_null() {
        return;
    }
    // Fill a stack/struct the guest provided. get_startup_info_w writes through
    // a &mut, so build into a local then copy out (the guest struct is #[repr(C)]
    // and matches StartupInfoW's layout for the fields the CRT reads).
    with_ctx(|ctx| {
        let mut local = k::StartupInfoW {
            cb: 0,
            desktop: [0],
            title: [0],
            x: 0,
            y: 0,
            x_size: 0,
            y_size: 0,
            x_count_chars: 0,
            y_count_chars: 0,
            fill_attribute: 0,
            flags: 0,
            show_window: 0,
            std_input: crate::NULL_HANDLE,
            std_output: crate::NULL_HANDLE,
            std_error: crate::NULL_HANDLE,
        };
        k::get_startup_info_w(ctx, &mut local);
        info.write_unaligned(local);
    });
}

pub unsafe extern "win64" fn shim_get_current_process() -> u64 {
    with_ctx(|ctx| k::get_current_process(ctx).0)
}

pub unsafe extern "win64" fn shim_get_current_thread() -> u64 {
    // GetCurrentThread pseudo-handle, identical convention to Windows.
    0xFFFF_FFFF_FFFF_FFFD
}

// ---------------------------------------------------------------------------
// Console (CRT writes startup banners / asserts via WriteConsole)
// ---------------------------------------------------------------------------

pub unsafe extern "win64" fn shim_write_console_w(
    handle: u64,
    buffer: *const u16,
    chars_to_write: u32,
    chars_written: *mut u32,
    _reserved: u64,
) -> i32 {
    if buffer.is_null() && chars_to_write != 0 {
        return 0;
    }
    let wide = core::slice::from_raw_parts(buffer, chars_to_write as usize);
    // Down-convert to bytes (CP-437/UTF-8-ish: ASCII passes through, others to
    // '?') and route to the stdout fd exactly like WriteFile so the capture tee
    // sees CRT console output too.
    let mut bytes: Vec<u8> = Vec::with_capacity(wide.len());
    for &c in wide {
        bytes.push(if c < 0x80 { c as u8 } else { b'?' });
    }
    let mut local_written: u32 = 0;
    let (ok, is_stdout) = with_ctx(|ctx| {
        let is_stdout = ctx.handle_table.get(handle).and_then(|e| e.native_id) == Some(1)
            || ctx.handle_table.get(handle).and_then(|e| e.native_id) == Some(2);
        let r = k::write_file(
            ctx,
            WinHandle(handle),
            &bytes,
            bytes.len() as u32,
            &mut local_written,
            0,
        )
        .0;
        (r, is_stdout)
    });
    if ok != 0 && is_stdout {
        capture_stdout(&bytes[..local_written as usize]);
    }
    if !chars_written.is_null() {
        // Report characters written (1 byte == 1 char in this down-conversion).
        chars_written.write_unaligned(local_written);
    }
    ok
}

pub unsafe extern "win64" fn shim_write_console_a(
    handle: u64,
    buffer: *const u8,
    chars_to_write: u32,
    chars_written: *mut u32,
    _reserved: u64,
) -> i32 {
    // ANSI console write is byte-for-byte WriteFile semantics.
    shim_write_file(handle, buffer, chars_to_write, chars_written, 0)
}

pub unsafe extern "win64" fn shim_get_console_mode(handle: u64, mode: *mut u32) -> i32 {
    if mode.is_null() {
        return 0;
    }
    let mut local: u32 = 0;
    let ok = with_ctx(|ctx| k::get_console_mode(ctx, WinHandle(handle), &mut local).0);
    mode.write_unaligned(local);
    ok
}

pub unsafe extern "win64" fn shim_set_console_mode(handle: u64, mode: u32) -> i32 {
    with_ctx(|ctx| k::set_console_mode(ctx, WinHandle(handle), mode).0)
}

pub unsafe extern "win64" fn shim_get_console_output_cp() -> u32 {
    // OEM US (CP-437) — what a default console reports.
    437
}

pub unsafe extern "win64" fn shim_get_console_cp() -> u32 {
    437
}

pub unsafe extern "win64" fn shim_get_acp() -> u32 {
    with_ctx(|ctx| k::get_acp(ctx))
}

pub unsafe extern "win64" fn shim_get_oemcp() -> u32 {
    with_ctx(|ctx| k::get_oemcp(ctx))
}

/// GetCPInfo — fills a CPINFO { MaxCharSize:u32, DefaultChar[2]:u8, LeadByte[12]:u8 }.
pub unsafe extern "win64" fn shim_get_cp_info(_code_page: u32, cp_info: *mut u8) -> i32 {
    if cp_info.is_null() {
        return 0;
    }
    // CPINFO is 18 bytes: u32 + 2 + 12.
    let buf = core::slice::from_raw_parts_mut(cp_info, 18);
    buf[0..4].copy_from_slice(&1u32.to_le_bytes()); // MaxCharSize = 1 (SBCS)
    buf[4] = b'?'; // DefaultChar[0]
    buf[5] = 0;
    for b in &mut buf[6..18] {
        *b = 0; // no lead-byte ranges (single-byte codepage)
    }
    1
}

// ---------------------------------------------------------------------------
// Feature / debugger / timing probes (CRT __scrt feature detection)
// ---------------------------------------------------------------------------

pub unsafe extern "win64" fn shim_is_processor_feature_present(feature: u32) -> i32 {
    with_ctx(|ctx| k::is_processor_feature_present(ctx, feature).0)
}

pub unsafe extern "win64" fn shim_is_debugger_present() -> i32 {
    with_ctx(|ctx| k::is_debugger_present(ctx).0)
}

pub unsafe extern "win64" fn shim_query_performance_counter(counter: *mut i64) -> i32 {
    if counter.is_null() {
        return 0;
    }
    let mut li = crate::LargeInteger(0);
    with_ctx(|ctx| k::query_performance_counter(ctx, &mut li));
    counter.write_unaligned(li.0);
    1
}

pub unsafe extern "win64" fn shim_query_performance_frequency(freq: *mut i64) -> i32 {
    if freq.is_null() {
        return 0;
    }
    let mut li = crate::LargeInteger(0);
    with_ctx(|ctx| k::query_performance_frequency(ctx, &mut li));
    freq.write_unaligned(li.0);
    1
}

// ---------------------------------------------------------------------------
// CriticalSection (CRT locks during init). Single-threaded-safe in-session.
// ---------------------------------------------------------------------------
//
// The guest passes a pointer to a CRITICAL_SECTION (an opaque 40-byte blob on
// x64). We do not interpret its internal layout; we maintain a tiny recursion
// counter in the first 8 bytes so Enter/Leave balance, which is all a
// single-threaded CRT observes. No cross-thread blocking is possible yet
// (one guest thread), so this is correct, not a stub.

pub unsafe extern "win64" fn shim_initialize_critical_section(cs: *mut u64) {
    if cs.is_null() {
        return;
    }
    // Recursion count starts at 0.
    cs.write_unaligned(0);
}

pub unsafe extern "win64" fn shim_initialize_critical_section_ex(
    cs: *mut u64,
    _spin_count: u32,
    _flags: u32,
) -> i32 {
    if cs.is_null() {
        return 0;
    }
    cs.write_unaligned(0);
    1
}

pub unsafe extern "win64" fn shim_initialize_critical_section_and_spin_count(
    cs: *mut u64,
    _spin_count: u32,
) -> i32 {
    if cs.is_null() {
        return 0;
    }
    cs.write_unaligned(0);
    1
}

pub unsafe extern "win64" fn shim_enter_critical_section(cs: *mut u64) {
    if cs.is_null() {
        return;
    }
    let n = cs.read_unaligned();
    cs.write_unaligned(n + 1);
}

pub unsafe extern "win64" fn shim_leave_critical_section(cs: *mut u64) {
    if cs.is_null() {
        return;
    }
    let n = cs.read_unaligned();
    cs.write_unaligned(n.saturating_sub(1));
}

pub unsafe extern "win64" fn shim_try_enter_critical_section(cs: *mut u64) -> i32 {
    if cs.is_null() {
        return 0;
    }
    // Single-threaded: always acquirable.
    let n = cs.read_unaligned();
    cs.write_unaligned(n + 1);
    1
}

pub unsafe extern "win64" fn shim_delete_critical_section(cs: *mut u64) {
    if cs.is_null() {
        return;
    }
    cs.write_unaligned(0);
}

/// InitOnceExecuteOnce — run the callback exactly once. The InitOnce blob's
/// first 8 bytes track state (0 = not run, 2 = done), mirroring kernel32's
/// InitOnce contract closely enough for the CRT's single-threaded use.
pub unsafe extern "win64" fn shim_init_once_execute_once(
    init_once: *mut u64,
    init_fn: u64,
    parameter: u64,
    context: *mut u64,
) -> i32 {
    if init_once.is_null() {
        return 0;
    }
    let state = init_once.read_unaligned();
    if state == 2 {
        return 1; // already initialized
    }
    if init_fn != 0 {
        // PINIT_ONCE_FN: BOOL (*)(PINIT_ONCE, PVOID Parameter, PVOID *Context)
        type InitOnceFn = unsafe extern "win64" fn(*mut u64, u64, *mut u64) -> i32;
        let f: InitOnceFn = core::mem::transmute(init_fn as usize);
        let ok = f(init_once, parameter, context);
        if ok == 0 {
            return 0;
        }
    }
    init_once.write_unaligned(2);
    1
}

// ---------------------------------------------------------------------------
// ntdll shims (the heap/error aliases kernel32 forwards to ntdll)
// ---------------------------------------------------------------------------

pub unsafe extern "win64" fn shim_rtl_allocate_heap(heap: u64, flags: u32, bytes: u64) -> u64 {
    with_ctx(|ctx| k::heap_alloc(ctx, WinHandle(heap), flags, bytes))
}

pub unsafe extern "win64" fn shim_rtl_free_heap(heap: u64, flags: u32, mem: u64) -> i32 {
    with_ctx(|ctx| k::heap_free(ctx, WinHandle(heap), flags, mem).0)
}

pub unsafe extern "win64" fn shim_rtl_get_last_win32_error() -> u32 {
    with_ctx(|ctx| k::get_last_error(ctx).0)
}

pub unsafe extern "win64" fn shim_rtl_set_last_win32_error(code: u32) {
    with_ctx(|ctx| k::set_last_error_api(ctx, DWord(code)))
}

// ---------------------------------------------------------------------------
// MSVC /MT CRT-startup imports (the set a real cl.exe console .exe pulls before
// main). Each routes to crate::crt_startup pure logic, the existing session
// state, or the table-based crate::seh unwinder. See
// docs/raebridge-real-crt-abi.md.
// ---------------------------------------------------------------------------

use crate::crt_startup as crt;

// ── Pointer / interlocked SList ────────────────────────────────────────────

/// `InitializeSListHead(PSLIST_HEADER)` — zero the 16-byte header (empty list).
pub unsafe extern "win64" fn shim_initialize_slist_head(head: *mut u8) {
    if head.is_null() {
        return;
    }
    core::ptr::write_bytes(head, 0, crt::SLIST_HEADER_SIZE);
}

/// `EncodePointer(PVOID)` — XOR/rotate with the per-process cookie.
pub unsafe extern "win64" fn shim_encode_pointer(ptr: u64) -> u64 {
    crt::encode_pointer(ptr)
}

/// `DecodePointer(PVOID)` — exact inverse of EncodePointer.
pub unsafe extern "win64" fn shim_decode_pointer(ptr: u64) -> u64 {
    crt::decode_pointer(ptr)
}

// ── Fiber-Local Storage ────────────────────────────────────────────────────

/// `FlsAlloc(PFLS_CALLBACK_FUNCTION)` — allocate an FLS index (callback stored).
pub unsafe extern "win64" fn shim_fls_alloc(callback: u64) -> u32 {
    with_ctx(|ctx| {
        let idx = crt::fls_alloc(&mut ctx.fls_slots, callback);
        if idx == crt::FLS_OUT_OF_INDEXES {
            k::set_last_error_api(ctx, DWord(8 /* ERROR_NOT_ENOUGH_MEMORY */));
        }
        idx
    })
}

/// `FlsFree(DWORD index)` — release the slot. Returns BOOL.
pub unsafe extern "win64" fn shim_fls_free(index: u32) -> i32 {
    with_ctx(|ctx| {
        if crt::fls_free(&mut ctx.fls_slots, index) {
            1
        } else {
            0
        }
    })
}

/// `FlsGetValue(DWORD index)` — stored value, or 0 (+LastError) for a bad index.
pub unsafe extern "win64" fn shim_fls_get_value(index: u32) -> u64 {
    with_ctx(|ctx| match crt::fls_get_value(&ctx.fls_slots, index) {
        Some(v) => {
            k::set_last_error_api(ctx, DWord(0));
            v
        }
        None => {
            k::set_last_error_api(ctx, DWord(87 /* ERROR_INVALID_PARAMETER */));
            0
        }
    })
}

/// `FlsSetValue(DWORD index, PVOID value)` — store; BOOL.
pub unsafe extern "win64" fn shim_fls_set_value(index: u32, value: u64) -> i32 {
    with_ctx(|ctx| {
        if crt::fls_set_value(&mut ctx.fls_slots, index, value) {
            1
        } else {
            0
        }
    })
}

// ── Code-page / locale ─────────────────────────────────────────────────────

/// `IsValidCodePage(UINT)` — BOOL for the code pages this layer backs.
pub unsafe extern "win64" fn shim_is_valid_code_page(code_page: u32) -> i32 {
    if crt::is_valid_code_page(code_page) {
        1
    } else {
        0
    }
}

/// `MultiByteToWideChar(cp, flags, src, srclen, dst, dstlen)`.
/// `srclen == -1` means the source is NUL-terminated (count includes the NUL).
/// `dstlen == 0` returns the required count in UTF-16 units without writing.
pub unsafe extern "win64" fn shim_multibyte_to_widechar(
    code_page: u32,
    _flags: u32,
    src: *const u8,
    src_len: i32,
    dst: *mut u16,
    dst_len: i32,
) -> i32 {
    if src.is_null() {
        return 0;
    }
    let bytes: &[u8] = if src_len < 0 {
        // NUL-terminated: measure (excluding the NUL), then include it.
        let mut n = 0usize;
        while n < MAX_CSTR_LEN && *src.add(n) != 0 {
            n += 1;
        }
        core::slice::from_raw_parts(src, n + 1)
    } else {
        core::slice::from_raw_parts(src, src_len as usize)
    };
    let wide = crt::mb_to_wide(code_page, bytes);
    if dst_len == 0 {
        return wide.len() as i32;
    }
    if dst.is_null() || (dst_len as usize) < wide.len() {
        with_ctx(|ctx| k::set_last_error_api(ctx, DWord(122 /* ERROR_INSUFFICIENT_BUFFER */)));
        return 0;
    }
    let out = core::slice::from_raw_parts_mut(dst, wide.len());
    out.copy_from_slice(&wide);
    wide.len() as i32
}

/// `WideCharToMultiByte(cp, flags, src, srclen, dst, dstlen, defChar, usedDef)`.
pub unsafe extern "win64" fn shim_widechar_to_multibyte(
    code_page: u32,
    _flags: u32,
    src: *const u16,
    src_len: i32,
    dst: *mut u8,
    dst_len: i32,
    _default_char: *const u8,
    used_default: *mut i32,
) -> i32 {
    if src.is_null() {
        return 0;
    }
    let wide: &[u16] = if src_len < 0 {
        let mut n = 0usize;
        while n < MAX_CSTR_LEN && *src.add(n) != 0 {
            n += 1;
        }
        core::slice::from_raw_parts(src, n + 1)
    } else {
        core::slice::from_raw_parts(src, src_len as usize)
    };
    let bytes = crt::wide_to_mb(code_page, wide);
    if !used_default.is_null() {
        used_default.write_unaligned(0);
    }
    if dst_len == 0 {
        return bytes.len() as i32;
    }
    if dst.is_null() || (dst_len as usize) < bytes.len() {
        with_ctx(|ctx| k::set_last_error_api(ctx, DWord(122)));
        return 0;
    }
    let out = core::slice::from_raw_parts_mut(dst, bytes.len());
    out.copy_from_slice(&bytes);
    bytes.len() as i32
}

/// `GetStringTypeW(dwInfoType, lpSrcStr, cchSrc, lpCharType)` — CT_CTYPE1 only.
pub unsafe extern "win64" fn shim_get_string_type_w(
    _info_type: u32,
    src: *const u16,
    cch: i32,
    char_type: *mut u16,
) -> i32 {
    if src.is_null() || char_type.is_null() {
        return 0;
    }
    let wide: &[u16] = if cch < 0 {
        wide_cstr(src)
    } else {
        core::slice::from_raw_parts(src, cch as usize)
    };
    let out = core::slice::from_raw_parts_mut(char_type, wide.len());
    for (i, &u) in wide.iter().enumerate() {
        out[i] = crt::string_type_ctype1(u);
    }
    1
}

/// `LCMapStringW(locale, flags, src, srclen, dst, dstlen)`.
pub unsafe extern "win64" fn shim_lcmap_string_w(
    _locale: u32,
    flags: u32,
    src: *const u16,
    src_len: i32,
    dst: *mut u16,
    dst_len: i32,
) -> i32 {
    if src.is_null() {
        return 0;
    }
    let wide: &[u16] = if src_len < 0 {
        wide_cstr(src)
    } else {
        core::slice::from_raw_parts(src, src_len as usize)
    };
    let mapped = crt::lc_map_string(flags, wide);
    if dst_len == 0 {
        return mapped.len() as i32;
    }
    if dst.is_null() || (dst_len as usize) < mapped.len() {
        with_ctx(|ctx| k::set_last_error_api(ctx, DWord(122)));
        return 0;
    }
    let out = core::slice::from_raw_parts_mut(dst, mapped.len());
    out.copy_from_slice(&mapped);
    mapped.len() as i32
}

/// `CompareStringW(locale, flags, s1, len1, s2, len2)` — returns CSTR_* (1/2/3).
pub unsafe extern "win64" fn shim_compare_string_w(
    _locale: u32,
    flags: u32,
    s1: *const u16,
    len1: i32,
    s2: *const u16,
    len2: i32,
) -> i32 {
    if s1.is_null() || s2.is_null() {
        return 0;
    }
    let a: &[u16] = if len1 < 0 {
        wide_cstr(s1)
    } else {
        core::slice::from_raw_parts(s1, len1 as usize)
    };
    let b: &[u16] = if len2 < 0 {
        wide_cstr(s2)
    } else {
        core::slice::from_raw_parts(s2, len2 as usize)
    };
    crt::compare_string(flags, a, b)
}

// ── File / module / env / handle (the rest the /MT CRT touches) ────────────

/// `GetFileType(HANDLE)` — FILE_TYPE_CHAR for std handles, FILE_TYPE_DISK else,
/// FILE_TYPE_UNKNOWN for a bad handle. The CRT calls this on its std handles to
/// decide line-buffering; std handles report CHAR (a character device).
pub unsafe extern "win64" fn shim_get_file_type(handle: u64) -> u32 {
    const FILE_TYPE_UNKNOWN: u32 = 0x0000;
    const FILE_TYPE_DISK: u32 = 0x0001;
    const FILE_TYPE_CHAR: u32 = 0x0002;
    with_ctx(
        |ctx| match ctx.handle_table.get(handle).and_then(|e| e.native_id) {
            Some(0) | Some(1) | Some(2) => FILE_TYPE_CHAR,
            Some(_) => FILE_TYPE_DISK,
            None => FILE_TYPE_UNKNOWN,
        },
    )
}

/// `GetFileSizeEx(HANDLE, PLARGE_INTEGER)` — the printf `/MT` CRT calls this on
/// its std handles while sizing stdio buffers. For a character device (a std
/// handle) there is no meaningful file size, so real Windows fails the call with
/// ERROR_INVALID_FUNCTION; the CRT treats that as "not a sizeable file" and uses
/// its default buffering — exactly the behavior printf needs. For a real backing
/// fd we report the size from `sys_stat`. Returns BOOL; on FALSE the OUT param is
/// left untouched (matching Win32). A NULL/unknown handle fails INVALID_HANDLE.
pub unsafe extern "win64" fn shim_get_file_size_ex(handle: u64, file_size: *mut i64) -> i32 {
    const FILE_TYPE_CHAR: u32 = 0x0002;
    const ERROR_INVALID_FUNCTION: u32 = 1;
    const ERROR_INVALID_HANDLE: u32 = 6;
    let native = with_ctx(|ctx| ctx.handle_table.get(handle).and_then(|e| e.native_id));
    match native {
        // std handles (stdin/stdout/stderr) are character devices: no file size.
        Some(0) | Some(1) | Some(2) => {
            // Match the file-type the CRT just observed (CHAR) and fail the size
            // query the way GetFileSizeEx does on a console handle.
            let _ = FILE_TYPE_CHAR;
            with_ctx(|ctx| k::set_last_error_api(ctx, DWord(ERROR_INVALID_FUNCTION)));
            0
        }
        Some(fd) => {
            let sz = syscalls::sys_stat(fd as u64);
            if sz == u64::MAX {
                with_ctx(|ctx| k::set_last_error_api(ctx, DWord(ERROR_INVALID_FUNCTION)));
                return 0;
            }
            if !file_size.is_null() {
                file_size.write_unaligned(sz as i64);
            }
            1
        }
        None => {
            with_ctx(|ctx| k::set_last_error_api(ctx, DWord(ERROR_INVALID_HANDLE)));
            0
        }
    }
}

/// `SetStdHandle(nStdHandle, hHandle)` — accepted; the std handle table is fixed
/// in this model (the CRT only sets it back to what it read). BOOL TRUE.
pub unsafe extern "win64" fn shim_set_std_handle(_n: u32, _handle: u64) -> i32 {
    1
}

/// `FlushFileBuffers(HANDLE)` — writes are unbuffered here (sys_write is
/// synchronous), so this is a successful no-op for a known handle. BOOL.
pub unsafe extern "win64" fn shim_flush_file_buffers(handle: u64) -> i32 {
    with_ctx(|ctx| {
        if ctx.handle_table.get(handle).is_some() {
            1
        } else {
            0
        }
    })
}

/// `SetFilePointerEx(h, liDistance, lpNewPos, method)` — routes to sys_seek for
/// the absolute (FILE_BEGIN) case; reports the new position. BOOL.
pub unsafe extern "win64" fn shim_set_file_pointer_ex(
    handle: u64,
    distance: i64,
    new_pos: *mut i64,
    method: u32,
) -> i32 {
    const FILE_BEGIN_M: u32 = 0;
    if method != FILE_BEGIN_M || distance < 0 {
        // Only absolute forward seeks are modeled; report success at the
        // requested offset without moving (std handles are non-seekable).
        if !new_pos.is_null() {
            new_pos.write_unaligned(distance.max(0));
        }
        return 1;
    }
    let native = with_ctx(|ctx| ctx.handle_table.get(handle).and_then(|e| e.native_id));
    match native {
        Some(fd) => {
            let r = syscalls::sys_seek(fd as u64, distance as u64);
            if r == u64::MAX {
                return 0;
            }
            if !new_pos.is_null() {
                new_pos.write_unaligned(distance);
            }
            1
        }
        None => 0,
    }
}

/// `SetEnvironmentVariableW(name, value)` — store into the session environment.
pub unsafe extern "win64" fn shim_set_environment_variable_w(
    name: *const u16,
    value: *const u16,
) -> i32 {
    if name.is_null() {
        return 0;
    }
    let name_s = alloc::string::String::from_utf16_lossy(wide_cstr(name));
    with_ctx(|ctx| {
        if value.is_null() {
            ctx.environment.remove(&name_s);
        } else {
            let val_s = alloc::string::String::from_utf16_lossy(wide_cstr(value));
            ctx.environment.insert(name_s.clone(), val_s);
        }
        1
    })
}

/// `LoadLibraryExW(name, file, flags)` — returns the seeded module base for an
/// already-loaded system DLL, else kernel32's base as a stand-in so a following
/// GetProcAddress still resolves through the kernel32 shims. (Startup never
/// executes real DLL code beyond the kernel32 exports we provide.)
pub unsafe extern "win64" fn shim_load_library_ex_w(
    name: *const u16,
    _file: u64,
    _flags: u32,
) -> u64 {
    if name.is_null() {
        return 0;
    }
    let n = alloc::string::String::from_utf16_lossy(wide_cstr(name)).to_ascii_lowercase();
    with_ctx(|ctx| {
        let bare = n.rsplit(['\\', '/']).next().unwrap_or(&n);
        if let Some(&base) = ctx.loaded_modules.get(bare) {
            return base;
        }
        *ctx.loaded_modules.get("kernel32.dll").unwrap_or(&0)
    })
}

/// `FreeLibrary(HMODULE)` — refcount-free model: always succeeds. BOOL TRUE.
pub unsafe extern "win64" fn shim_free_library(_module: u64) -> i32 {
    1
}

/// `TerminateProcess(hProcess, uExitCode)` — the CRT's abnormal-exit path; for
/// our own process this is sys_exit. Never returns.
pub unsafe extern "win64" fn shim_terminate_process(_process: u64, exit_code: u32) -> ! {
    let msg = alloc::format!(
        "[raebridge] guest TerminateProcess({}) -> exit {}\n",
        exit_code,
        exit_code
    );
    syscalls::sys_debug_print(msg.as_bytes());
    syscalls::sys_exit(exit_code as u64)
}

// ── Find-file (the CRT's startup probe) ────────────────────────────────────

/// `FindFirstFileExW(...)` — startup probe; no matching file in this model.
/// Returns INVALID_HANDLE_VALUE + ERROR_FILE_NOT_FOUND, which the CRT treats as
/// "no such file" and proceeds.
pub unsafe extern "win64" fn shim_find_first_file_ex_w(
    _name: *const u16,
    _info_level: u32,
    _find_data: *mut u8,
    _search_op: u32,
    _filter: u64,
    _flags: u32,
) -> u64 {
    with_ctx(|ctx| k::set_last_error_api(ctx, DWord(2 /* ERROR_FILE_NOT_FOUND */)));
    u64::MAX // INVALID_HANDLE_VALUE
}

/// `FindNextFileW(...)` — no more files. BOOL FALSE + ERROR_NO_MORE_FILES.
pub unsafe extern "win64" fn shim_find_next_file_w(_handle: u64, _find_data: *mut u8) -> i32 {
    with_ctx(|ctx| k::set_last_error_api(ctx, DWord(18 /* ERROR_NO_MORE_FILES */)));
    0
}

/// `FindClose(HANDLE)` — close a find handle. BOOL TRUE.
pub unsafe extern "win64" fn shim_find_close(_handle: u64) -> i32 {
    1
}

// ── SEH-filter machinery (wired to crate::seh; no-throw startup path) ───────

/// `RtlCaptureContext(PCONTEXT)` — fill the integer-register slots of an AMD64
/// CONTEXT from the current register file. The CRT seeds a CONTEXT it then
/// virtual-unwinds; for the no-throw startup path it only needs a self-
/// consistent snapshot. Offsets are the documented AMD64 CONTEXT layout.
#[unsafe(naked)]
pub unsafe extern "win64" fn shim_rtl_capture_context(_context: *mut u8) {
    // rcx = PCONTEXT (win64 first arg). Integer GPRs: Rax 0x78 .. Rip 0xF8.
    core::arch::naked_asm!(
        "mov [rcx + 0x78], rax",
        "mov [rcx + 0x80], rcx",
        "mov [rcx + 0x88], rdx",
        "mov [rcx + 0x90], rbx",
        "lea rax, [rsp + 8]", // Rsp at the call site = current rsp + 8
        "mov [rcx + 0x98], rax",
        "mov [rcx + 0xA0], rbp",
        "mov [rcx + 0xA8], rsi",
        "mov [rcx + 0xB0], rdi",
        "mov [rcx + 0xB8], r8",
        "mov [rcx + 0xC0], r9",
        "mov [rcx + 0xC8], r10",
        "mov [rcx + 0xD0], r11",
        "mov [rcx + 0xD8], r12",
        "mov [rcx + 0xE0], r13",
        "mov [rcx + 0xE8], r14",
        "mov [rcx + 0xF0], r15",
        "mov rax, [rsp]", // Rip = return address
        "mov [rcx + 0xF8], rax",
        "mov dword ptr [rcx + 0x30], 0x10000B", // ContextFlags = CONTEXT_FULL
        "ret",
    )
}

/// `RtlLookupFunctionEntry(ControlPc, ImageBase*, HistoryTable*)` — find the
/// RUNTIME_FUNCTION covering ControlPc via the module registry, write the module
/// base, and return the absolute VA of the matching .pdata entry (0 = leaf / no
/// unwind data). Wired to the same .pdata the table-based crate::seh dispatcher
/// walks.
pub unsafe extern "win64" fn shim_rtl_lookup_function_entry(
    control_pc: u64,
    image_base: *mut u64,
    _history: u64,
) -> u64 {
    match crt::module_for_pc(control_pc) {
        Some(m) => {
            if !image_base.is_null() {
                image_base.write_unaligned(m.base);
            }
            // SAFETY: the module's pdata range was mapped by the loader.
            m.lookup_function_entry(control_pc)
        }
        None => {
            if !image_base.is_null() {
                image_base.write_unaligned(0);
            }
            0
        }
    }
}

/// `RtlPcToFileHeader(PcValue, BaseOfImage*)` — return the image base whose
/// range covers PcValue (also written through `base_of_image`), or 0.
pub unsafe extern "win64" fn shim_rtl_pc_to_file_header(pc: u64, base_of_image: *mut u64) -> u64 {
    let base = crt::module_for_pc(pc).map(|m| m.base).unwrap_or(0);
    if !base_of_image.is_null() {
        base_of_image.write_unaligned(base);
    }
    base
}

/// `RtlVirtualUnwind(...)` — the table-based unwind step. DEFERRED on the
/// no-exception-thrown startup path: the CRT installs this but does not unwind
/// before `main`. Provides the correct leaf-frame behavior (pop the return
/// address: Rip = [Rsp], Rsp += 8) so any defensive startup walk terminates
/// cleanly. Full UNWIND_CODE register-restoring replay is HUMAN-GATED guest-
/// execution work in crate::seh / MasterChecklist Phase 11.2. Returns the
/// handler address (0 = none).
pub unsafe extern "win64" fn shim_rtl_virtual_unwind(
    _handler_type: u32,
    _image_base: u64,
    _control_pc: u64,
    _function_entry: u64,
    context: *mut u8,
    handler_data: *mut u64,
    establisher_frame: *mut u64,
    _context_pointers: u64,
) -> u64 {
    if !handler_data.is_null() {
        handler_data.write_unaligned(0);
    }
    if !establisher_frame.is_null() {
        establisher_frame.write_unaligned(0);
    }
    if !context.is_null() {
        let rsp = (context.add(0x98) as *const u64).read_unaligned();
        if let Some(ret) = read_guest_u64(rsp) {
            (context.add(0xF8) as *mut u64).write_unaligned(ret);
            (context.add(0x98) as *mut u64).write_unaligned(rsp.wrapping_add(8));
        }
    }
    0
}

/// `RtlUnwindEx(...)` — unwind-to-target (runs termination handlers, transfers
/// control). DEFERRED: not exercised before `main` on the no-throw startup path.
/// If actually called during startup it fails loud (naming itself) rather than
/// corrupting the guest by half-unwinding. Full unwind-to-target is HUMAN-GATED
/// Phase 11.2 work. Never returns.
pub unsafe extern "win64" fn shim_rtl_unwind_ex(
    _target_frame: u64,
    _target_ip: u64,
    _record: u64,
    _return_value: u64,
    _context: *mut u8,
    _history: u64,
) -> ! {
    syscalls::sys_debug_print(
        b"[raebridge] FATAL: RtlUnwindEx invoked before main (live SEH dispatch is HUMAN-GATED Phase 11.2)\n",
    );
    syscalls::sys_exit(0xDEAD)
}

/// `RaiseException(code, flags, nargs, args)` — DEFERRED: the no-throw startup
/// path never raises. Fail loud if it happens. Full dispatch into the guest's
/// `__C_specific_handler` is HUMAN-GATED Phase 11.2 work. Never returns.
pub unsafe extern "win64" fn shim_raise_exception(
    code: u32,
    _flags: u32,
    _nargs: u32,
    _args: u64,
) -> ! {
    let msg = alloc::format!(
        "[raebridge] FATAL: RaiseException(code={:#x}) before main (SEH dispatch HUMAN-GATED Phase 11.2)\n",
        code
    );
    syscalls::sys_debug_print(msg.as_bytes());
    syscalls::sys_exit(0xDEAD)
}

/// `UnhandledExceptionFilter(PEXCEPTION_POINTERS)` — default top-level filter.
/// Installed but never called on the no-throw startup path; returns
/// EXCEPTION_CONTINUE_SEARCH (0) so the OS default action proceeds.
pub unsafe extern "win64" fn shim_unhandled_exception_filter(_pointers: u64) -> i32 {
    0 // EXCEPTION_CONTINUE_SEARCH
}

/// `SetUnhandledExceptionFilter(filter)` — store the new filter, return prev.
pub unsafe extern "win64" fn shim_set_unhandled_exception_filter(filter: u64) -> u64 {
    crt::set_unhandled_exception_filter(filter)
}

/// Read 8 bytes of guest memory (guest VA == host VA), checked. Used by the
/// unwind shim to pop a return address without faulting on a smashed stack.
#[inline]
unsafe fn read_guest_u64(addr: u64) -> Option<u64> {
    if addr == 0 || addr & 0x7 != 0 {
        return None;
    }
    Some((addr as *const u64).read_unaligned())
}

// ---------------------------------------------------------------------------
// Shim table — the loader's source of IAT patch addresses
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// kernel32 shims — synchronization objects (mutex / event / semaphore)
// ---------------------------------------------------------------------------
//
// These back the `raebridge_server` broker's in-process object model
// (`docs/components/raebridge-wine-strategy.md` §6.1). The previous code never
// exposed these to the IAT, so a guest importing CreateMutexW hit the fail-loud
// trampoline; now they resolve to real state-machine-backed shims.

use crate::WinBool;

pub unsafe extern "win64" fn shim_create_mutex_w(
    attrs: u64,
    initial_owner: i32,
    name: *const u16,
) -> u64 {
    let name = wide_cstr_opt(name);
    with_ctx(|ctx| k::create_mutex_w(ctx, attrs, WinBool(initial_owner), name).0)
}

pub unsafe extern "win64" fn shim_create_mutex_a(
    attrs: u64,
    initial_owner: i32,
    name: *const u8,
) -> u64 {
    let wide = ansi_name_to_wide(name);
    with_ctx(|ctx| k::create_mutex_w(ctx, attrs, WinBool(initial_owner), wide.as_deref()).0)
}

pub unsafe extern "win64" fn shim_open_mutex_w(access: u32, inherit: i32, name: *const u16) -> u64 {
    let name = wide_cstr_opt(name);
    with_ctx(|ctx| k::open_mutex_w(ctx, access, WinBool(inherit), name).0)
}

pub unsafe extern "win64" fn shim_release_mutex(handle: u64) -> i32 {
    with_ctx(|ctx| k::release_mutex(ctx, WinHandle(handle)).0)
}

pub unsafe extern "win64" fn shim_create_event_w(
    attrs: u64,
    manual_reset: i32,
    initial_state: i32,
    name: *const u16,
) -> u64 {
    let name = wide_cstr_opt(name);
    with_ctx(|ctx| {
        k::create_event_w(
            ctx,
            attrs,
            WinBool(manual_reset),
            WinBool(initial_state),
            name,
        )
        .0
    })
}

pub unsafe extern "win64" fn shim_create_event_a(
    attrs: u64,
    manual_reset: i32,
    initial_state: i32,
    name: *const u8,
) -> u64 {
    let wide = ansi_name_to_wide(name);
    with_ctx(|ctx| {
        k::create_event_w(
            ctx,
            attrs,
            WinBool(manual_reset),
            WinBool(initial_state),
            wide.as_deref(),
        )
        .0
    })
}

pub unsafe extern "win64" fn shim_open_event_w(access: u32, inherit: i32, name: *const u16) -> u64 {
    let name = wide_cstr_opt(name);
    with_ctx(|ctx| k::open_event_w(ctx, access, WinBool(inherit), name).0)
}

pub unsafe extern "win64" fn shim_set_event(handle: u64) -> i32 {
    with_ctx(|ctx| k::set_event(ctx, WinHandle(handle)).0)
}

pub unsafe extern "win64" fn shim_reset_event(handle: u64) -> i32 {
    with_ctx(|ctx| k::reset_event(ctx, WinHandle(handle)).0)
}

pub unsafe extern "win64" fn shim_pulse_event(handle: u64) -> i32 {
    with_ctx(|ctx| k::pulse_event(ctx, WinHandle(handle)).0)
}

pub unsafe extern "win64" fn shim_create_semaphore_w(
    attrs: u64,
    initial: i32,
    maximum: i32,
    name: *const u16,
) -> u64 {
    let name = wide_cstr_opt(name);
    with_ctx(|ctx| k::create_semaphore_w(ctx, attrs, initial, maximum, name).0)
}

pub unsafe extern "win64" fn shim_create_semaphore_a(
    attrs: u64,
    initial: i32,
    maximum: i32,
    name: *const u8,
) -> u64 {
    let wide = ansi_name_to_wide(name);
    with_ctx(|ctx| k::create_semaphore_w(ctx, attrs, initial, maximum, wide.as_deref()).0)
}

pub unsafe extern "win64" fn shim_open_semaphore_w(
    access: u32,
    inherit: i32,
    name: *const u16,
) -> u64 {
    let name = wide_cstr_opt(name);
    with_ctx(|ctx| k::open_semaphore_w(ctx, access, WinBool(inherit), name).0)
}

pub unsafe extern "win64" fn shim_release_semaphore(
    handle: u64,
    release_count: i32,
    previous_count: *mut i32,
) -> i32 {
    with_ctx(|ctx| {
        let mut prev = 0i32;
        let r = k::release_semaphore(ctx, WinHandle(handle), release_count, Some(&mut prev)).0;
        if !previous_count.is_null() {
            *previous_count = prev;
        }
        r
    })
}

pub unsafe extern "win64" fn shim_wait_for_single_object(handle: u64, ms: u32) -> u32 {
    with_ctx(|ctx| k::wait_for_single_object(ctx, WinHandle(handle), ms))
}

pub unsafe extern "win64" fn shim_wait_for_single_object_ex(
    handle: u64,
    ms: u32,
    _alertable: i32,
) -> u32 {
    with_ctx(|ctx| k::wait_for_single_object(ctx, WinHandle(handle), ms))
}

pub unsafe extern "win64" fn shim_wait_for_multiple_objects(
    count: u32,
    handles: *const u64,
    wait_all: i32,
    ms: u32,
) -> u32 {
    if handles.is_null() || count == 0 {
        return crate::WAIT_FAILED;
    }
    let raw = core::slice::from_raw_parts(handles, count as usize);
    let wins: Vec<WinHandle> = raw.iter().map(|&h| WinHandle(h)).collect();
    with_ctx(|ctx| k::wait_for_multiple_objects(ctx, &wins, WinBool(wait_all), ms))
}

/// Convert a nullable ANSI `lpName` into an owned UTF-16 `Vec` (or `None` for a
/// NULL/anonymous name) so the `*A` create shims can reuse the `*W` core.
unsafe fn ansi_name_to_wide(ptr: *const u8) -> Option<Vec<u16>> {
    if ptr.is_null() {
        return None;
    }
    let bytes = ansi_cstr(ptr);
    Some(bytes.iter().map(|&b| b as u16).collect())
}

// ---------------------------------------------------------------------------
// advapi32 shims — registry (RegOpenKeyEx / RegQueryValueEx / …)
// ---------------------------------------------------------------------------
//
// The registry FUNCTIONS already existed in `advapi32.rs` and are backed by the
// real `ctx.registry` hive (versioned-config-mirrored, snapshot/rollback — see
// `win_registry.rs`), but they were never exposed to the IAT, so a PE importing
// `RegOpenKeyExW` hit the fail-loud trampoline. These `extern "win64"` thunks
// close that gap (`docs/components/raebridge-wine-strategy.md` §7 / Phase A.3).

use crate::advapi32 as adv;
use crate::wide_to_string;

/// Build a `&str` key/value name from a nullable wide pointer (NULL → "").
unsafe fn wide_name(ptr: *const u16) -> alloc::string::String {
    wide_to_string(wide_cstr(ptr))
}

pub unsafe extern "win64" fn shim_reg_open_key_ex_w(
    hkey: u64,
    sub_key: *const u16,
    options: u32,
    sam: u32,
    result: *mut u64,
) -> i32 {
    let key = wide_name(sub_key);
    with_ctx(|ctx| {
        let mut out = 0u64;
        let r = adv::reg_open_key_ex_w(ctx, hkey, &key, options, sam, &mut out);
        if !result.is_null() {
            *result = out;
        }
        r
    })
}

pub unsafe extern "win64" fn shim_reg_open_key_ex_a(
    hkey: u64,
    sub_key: *const u8,
    options: u32,
    sam: u32,
    result: *mut u64,
) -> i32 {
    let key = ansi_cstr(sub_key);
    with_ctx(|ctx| {
        let mut out = 0u64;
        let r = adv::reg_open_key_ex_a(ctx, hkey, key, options, sam, &mut out);
        if !result.is_null() {
            *result = out;
        }
        r
    })
}

pub unsafe extern "win64" fn shim_reg_close_key(hkey: u64) -> i32 {
    with_ctx(|ctx| adv::reg_close_key(ctx, hkey))
}

pub unsafe extern "win64" fn shim_reg_create_key_ex_w(
    hkey: u64,
    sub_key: *const u16,
    reserved: u32,
    class: *const u16,
    options: u32,
    sam: u32,
    security: u64,
    result: *mut u64,
    disposition: *mut u32,
) -> i32 {
    let key = wide_name(sub_key);
    let class_s = if class.is_null() {
        None
    } else {
        Some(wide_name(class))
    };
    with_ctx(|ctx| {
        let mut out = 0u64;
        let mut disp = 0u32;
        let r = adv::reg_create_key_ex_w(
            ctx,
            hkey,
            &key,
            reserved,
            class_s.as_deref(),
            options,
            sam,
            security,
            &mut out,
            &mut disp,
        );
        if !result.is_null() {
            *result = out;
        }
        if !disposition.is_null() {
            *disposition = disp;
        }
        r
    })
}

pub unsafe extern "win64" fn shim_reg_create_key_ex_a(
    hkey: u64,
    sub_key: *const u8,
    reserved: u32,
    class: *const u8,
    options: u32,
    sam: u32,
    security: u64,
    result: *mut u64,
    disposition: *mut u32,
) -> i32 {
    let key = ansi_cstr(sub_key);
    let class_b = if class.is_null() {
        None
    } else {
        Some(ansi_cstr(class))
    };
    with_ctx(|ctx| {
        let mut out = 0u64;
        let mut disp = 0u32;
        let r = adv::reg_create_key_ex_a(
            ctx, hkey, key, reserved, class_b, options, sam, security, &mut out, &mut disp,
        );
        if !result.is_null() {
            *result = out;
        }
        if !disposition.is_null() {
            *disposition = disp;
        }
        r
    })
}

pub unsafe extern "win64" fn shim_reg_query_value_ex_w(
    hkey: u64,
    value_name: *const u16,
    _reserved: *mut u32,
    reg_type: *mut u32,
    data: *mut u8,
    cb_data: *mut u32,
) -> i32 {
    let name = wide_name(value_name);
    let cap = if cb_data.is_null() { 0 } else { *cb_data };
    with_ctx(|ctx| {
        let buf: &mut [u8] = if data.is_null() || cap == 0 {
            &mut []
        } else {
            core::slice::from_raw_parts_mut(data, cap as usize)
        };
        let mut ty = 0u32;
        let mut size = cap;
        let r = adv::reg_query_value_ex_w(ctx, hkey, &name, 0, &mut ty, buf, &mut size);
        if !reg_type.is_null() {
            *reg_type = ty;
        }
        if !cb_data.is_null() {
            *cb_data = size;
        }
        r
    })
}

pub unsafe extern "win64" fn shim_reg_query_value_ex_a(
    hkey: u64,
    value_name: *const u8,
    _reserved: *mut u32,
    reg_type: *mut u32,
    data: *mut u8,
    cb_data: *mut u32,
) -> i32 {
    let name = ansi_cstr(value_name);
    let cap = if cb_data.is_null() { 0 } else { *cb_data };
    with_ctx(|ctx| {
        let buf: &mut [u8] = if data.is_null() || cap == 0 {
            &mut []
        } else {
            core::slice::from_raw_parts_mut(data, cap as usize)
        };
        let mut ty = 0u32;
        let mut size = cap;
        let r = adv::reg_query_value_ex_a(ctx, hkey, name, 0, &mut ty, buf, &mut size);
        if !reg_type.is_null() {
            *reg_type = ty;
        }
        if !cb_data.is_null() {
            *cb_data = size;
        }
        r
    })
}

pub unsafe extern "win64" fn shim_reg_set_value_ex_w(
    hkey: u64,
    value_name: *const u16,
    reserved: u32,
    reg_type: u32,
    data: *const u8,
    cb_data: u32,
) -> i32 {
    let name = wide_name(value_name);
    let bytes: &[u8] = if data.is_null() || cb_data == 0 {
        &[]
    } else {
        core::slice::from_raw_parts(data, cb_data as usize)
    };
    with_ctx(|ctx| adv::reg_set_value_ex_w(ctx, hkey, &name, reserved, reg_type, bytes))
}

pub unsafe extern "win64" fn shim_reg_set_value_ex_a(
    hkey: u64,
    value_name: *const u8,
    reserved: u32,
    reg_type: u32,
    data: *const u8,
    cb_data: u32,
) -> i32 {
    let name = ansi_cstr(value_name);
    let bytes: &[u8] = if data.is_null() || cb_data == 0 {
        &[]
    } else {
        core::slice::from_raw_parts(data, cb_data as usize)
    };
    with_ctx(|ctx| adv::reg_set_value_ex_a(ctx, hkey, name, reserved, reg_type, bytes))
}

pub unsafe extern "win64" fn shim_reg_delete_key_w(hkey: u64, sub_key: *const u16) -> i32 {
    let key = wide_name(sub_key);
    with_ctx(|ctx| adv::reg_delete_key_w(ctx, hkey, &key))
}

pub unsafe extern "win64" fn shim_reg_delete_value_w(hkey: u64, value_name: *const u16) -> i32 {
    let name = wide_name(value_name);
    with_ctx(|ctx| adv::reg_delete_value_w(ctx, hkey, &name))
}

pub unsafe extern "win64" fn shim_reg_enum_key_ex_w(
    hkey: u64,
    index: u32,
    name: *mut u16,
    cch_name: *mut u32,
    _reserved: *mut u32,
    _class: *mut u16,
    _cch_class: *mut u32,
    _last_write: *mut u8,
) -> i32 {
    if cch_name.is_null() {
        return crate::ERROR_INVALID_PARAMETER as i32;
    }
    let cap = *cch_name;
    with_ctx(|ctx| {
        let buf: &mut [u16] = if name.is_null() || cap == 0 {
            &mut []
        } else {
            core::slice::from_raw_parts_mut(name, cap as usize)
        };
        let mut size = cap;
        let r = adv::reg_enum_key_ex_w(ctx, hkey, index, buf, &mut size, None, None);
        *cch_name = size;
        r
    })
}

pub unsafe extern "win64" fn shim_reg_enum_value_w(
    hkey: u64,
    index: u32,
    value_name: *mut u16,
    cch_name: *mut u32,
    _reserved: *mut u32,
    reg_type: *mut u32,
    data: *mut u8,
    cb_data: *mut u32,
) -> i32 {
    if cch_name.is_null() {
        return crate::ERROR_INVALID_PARAMETER as i32;
    }
    let name_cap = *cch_name;
    let data_cap = if cb_data.is_null() { 0 } else { *cb_data };
    with_ctx(|ctx| {
        let name_buf: &mut [u16] = if value_name.is_null() || name_cap == 0 {
            &mut []
        } else {
            core::slice::from_raw_parts_mut(value_name, name_cap as usize)
        };
        let data_buf: &mut [u8] = if data.is_null() || data_cap == 0 {
            &mut []
        } else {
            core::slice::from_raw_parts_mut(data, data_cap as usize)
        };
        let mut name_size = name_cap;
        let mut ty = 0u32;
        let mut data_size = data_cap;
        let r = adv::reg_enum_value_w(
            ctx,
            hkey,
            index,
            name_buf,
            &mut name_size,
            &mut ty,
            data_buf,
            &mut data_size,
        );
        *cch_name = name_size;
        if !reg_type.is_null() {
            *reg_type = ty;
        }
        if !cb_data.is_null() {
            *cb_data = data_size;
        }
        r
    })
}

pub unsafe extern "win64" fn shim_reg_query_info_key_w(
    hkey: u64,
    _class: *mut u16,
    cch_class: *mut u32,
    _reserved: *mut u32,
    c_subkeys: *mut u32,
    cb_max_subkey: *mut u32,
    cb_max_class: *mut u32,
    c_values: *mut u32,
    cb_max_value_name: *mut u32,
    cb_max_value_data: *mut u32,
    cb_security: *mut u32,
    _last_write: *mut u8,
) -> i32 {
    with_ctx(|ctx| {
        let mut n_sub = 0u32;
        let mut max_sub = 0u32;
        let mut n_val = 0u32;
        let mut max_vname = 0u32;
        let mut max_vdata = 0u32;
        let r = adv::reg_query_info_key_w(
            ctx,
            hkey,
            None,
            &mut n_sub,
            &mut max_sub,
            &mut n_val,
            &mut max_vname,
            &mut max_vdata,
        );
        if !c_subkeys.is_null() {
            *c_subkeys = n_sub;
        }
        if !cb_max_subkey.is_null() {
            *cb_max_subkey = max_sub;
        }
        if !c_values.is_null() {
            *c_values = n_val;
        }
        if !cb_max_value_name.is_null() {
            *cb_max_value_name = max_vname;
        }
        if !cb_max_value_data.is_null() {
            *cb_max_value_data = max_vdata;
        }
        // Class + security-descriptor sizes are not modeled — report zero.
        if !cch_class.is_null() {
            *cch_class = 0;
        }
        if !cb_max_class.is_null() {
            *cb_max_class = 0;
        }
        if !cb_security.is_null() {
            *cb_security = 0;
        }
        r
    })
}

pub unsafe extern "win64" fn shim_reg_flush_key(hkey: u64) -> i32 {
    with_ctx(|ctx| adv::reg_flush_key(ctx, hkey))
}

/// One IAT-patchable export: (dll, function name, shim address).
pub type ShimEntry = (&'static str, &'static str, u64);

/// Every Win32 name that resolves to a real callable shim. The PE loader
/// writes `addr` straight into the IAT slot for (dll, name) imports.
/// Function-pointer casts go through `usize` so the table is uniform u64.
// ---------------------------------------------------------------------------
// user32.dll — window class + window creation (Phase C GUI IAT wiring)
//
// The dispatch core (DispatchMessage/SendMessage -> the stored lpfnWndProc) is
// real (user32.rs); these shims make the windowing entry points reachable by a
// guest PE so RegisterClassExW captures the WndProc and CreateWindowEx spawns a
// compositor surface. The message-loop shims (GetMessage/DispatchMessage with
// MSG marshaling) are the immediate follow-up.
// ---------------------------------------------------------------------------

/// Marshal a guest `WNDCLASSEXW` (x64 layout, 80 bytes) into our `WndClassExW`.
/// Field offsets: `style` @4, `lpfnWndProc` @8, `hIcon` @32, `hCursor` @40,
/// `hbrBackground` @48, `lpszMenuName` @56, `lpszClassName` @64, `hIconSm` @72.
/// Guest pointers are host pointers (in-process model); reads are unaligned and
/// strings are bounded by `wide_cstr`.
///
/// # Safety
/// `p` must point at a readable ≥80-byte guest `WNDCLASSEXW`.
unsafe fn marshal_wndclassexw(p: *const u8) -> crate::WndClassExW {
    let rd32 = |off: usize| core::ptr::read_unaligned(p.add(off) as *const u32);
    let rd64 = |off: usize| core::ptr::read_unaligned(p.add(off) as *const u64);
    let menu_ptr = rd64(56) as *const u16;
    crate::WndClassExW {
        style: rd32(4),
        wnd_proc: rd64(8),
        class_name: crate::wide_to_string(wide_cstr(rd64(64) as *const u16)),
        icon: WinHandle(rd64(32)),
        cursor: WinHandle(rd64(40)),
        background: WinHandle(rd64(48)),
        menu_name: if menu_ptr.is_null() {
            None
        } else {
            Some(crate::wide_to_string(wide_cstr(menu_ptr)))
        },
        icon_sm: WinHandle(rd64(72)),
    }
}

/// `RegisterClassExW(const WNDCLASSEXW*)` -> ATOM. Captures the guest's
/// `lpfnWndProc` so `DispatchMessage`/`SendMessage` can later route to it.
pub unsafe extern "win64" fn shim_register_class_ex_w(wndclass: *const u8) -> u16 {
    if wndclass.is_null() {
        return 0;
    }
    let wc = marshal_wndclassexw(wndclass);
    with_ctx(|ctx| u::register_class_ex_w(ctx, &wc).0)
}

/// `CreateWindowExW(...)` -> HWND. `lpClassName` may be an ATOM (low value) — not
/// yet resolved by atom, so a non-pointer class name yields an empty name (the
/// create then fails with INVALID_PARAMETER rather than dereferencing an atom).
#[allow(clippy::too_many_arguments)]
pub unsafe extern "win64" fn shim_create_window_ex_w(
    ex_style: u32,
    class_name: *const u16,
    window_name: *const u16,
    style: u32,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    parent: u64,
    menu: u64,
    instance: u64,
    param: u64,
) -> u64 {
    let cls: &[u16] = if (class_name as usize) < 0x1_0000 {
        &[]
    } else {
        wide_cstr(class_name)
    };
    let title = wide_cstr(window_name);
    with_ctx(|ctx| {
        u::create_window_ex_w(
            ctx,
            ex_style,
            cls,
            title,
            style,
            x,
            y,
            width,
            height,
            WinHandle(parent),
            WinHandle(menu),
            WinHandle(instance),
            param,
        )
        .0
    })
}

/// `ShowWindow(HWND, int)` -> BOOL (previous visibility). Presents the window's
/// compositor surface when shown.
pub unsafe extern "win64" fn shim_show_window(hwnd: u64, cmd_show: i32) -> i32 {
    with_ctx(|ctx| u::show_window(ctx, WinHandle(hwnd), cmd_show).0)
}

/// `UpdateWindow(HWND)` -> BOOL. Synchronously paints the window (WM_PAINT).
pub unsafe extern "win64" fn shim_update_window(hwnd: u64) -> i32 {
    // Validate + resolve the WndProc under the lock, RELEASE, then deliver the
    // synchronous WM_PAINT outside the lock (same reentrancy rule as dispatch).
    let resolved = with_ctx(|ctx| {
        if !ctx.windows.contains_key(&hwnd) {
            ctx.last_error = crate::ERROR_INVALID_HANDLE;
            return None;
        }
        Some(u::resolve_wndproc(ctx, WinHandle(hwnd)))
    });
    match resolved {
        None => 0, // invalid hwnd -> FALSE
        Some(proc_addr) => {
            if proc_addr != 0 {
                let _ = u::invoke_wndproc(proc_addr, hwnd, crate::WM_PAINT, 0, 0);
            }
            1
        }
    }
}

/// `DefWindowProcW(HWND, UINT, WPARAM, LPARAM)` -> LRESULT — the default handler
/// a WndProc tail-calls for messages it does not handle. The window-text
/// messages (WM_SETTEXT / WM_GETTEXT / WM_GETTEXTLENGTH) are handled here
/// against the same `WindowObject.title` storage as `Set/GetWindowTextW`, so a
/// guest WndProc that forwards them to DefWindowProcW gets faithful behavior;
/// they carry guest buffers, so the marshaling lives in the shim (the pure
/// `def_window_proc_w` never touches raw pointers).
pub unsafe extern "win64" fn shim_def_window_proc_w(
    hwnd: u64,
    msg: u32,
    wparam: u64,
    lparam: i64,
) -> i64 {
    match msg {
        crate::WM_SETTEXT => {
            // lParam = LPCWSTR new text (may be NULL = clear).
            let text = if lparam == 0 {
                alloc::string::String::new()
            } else {
                crate::wide_to_string(wide_cstr(lparam as usize as *const u16))
            };
            with_ctx(|ctx| u::set_window_text(ctx, WinHandle(hwnd), &text).0 as i64)
        }
        crate::WM_GETTEXTLENGTH => {
            with_ctx(|ctx| u::get_window_text_length(ctx, WinHandle(hwnd)) as i64)
        }
        crate::WM_GETTEXT => {
            // wParam = buffer size in chars (incl NUL), lParam = LPWSTR buffer.
            let buf = lparam as usize as *mut u16;
            if buf.is_null() || wparam == 0 {
                return 0;
            }
            let text = with_ctx(|ctx| u::get_window_text(ctx, WinHandle(hwnd))).unwrap_or_default();
            let (wbuf, n) = u::copy_text_truncated(&text, wparam as usize);
            core::ptr::copy_nonoverlapping(wbuf.as_ptr(), buf, wbuf.len());
            n as i64
        }
        _ => with_ctx(|ctx| u::def_window_proc_w(ctx, WinHandle(hwnd), msg, wparam, lparam)),
    }
}

/// `SetWindowTextW(HWND, LPCWSTR)` -> BOOL. Stores the window text (caption /
/// EDIT-control buffer). A notepad-class app's File->Save handler reads this
/// back via `GetWindowTextW`.
pub unsafe extern "win64" fn shim_set_window_text_w(hwnd: u64, lp: *const u16) -> i32 {
    let text = crate::wide_to_string(wide_cstr(lp));
    with_ctx(|ctx| u::set_window_text(ctx, WinHandle(hwnd), &text).0)
}

/// `GetWindowTextW(HWND, LPWSTR, int nMaxCount)` -> int (chars copied, excl NUL).
/// Truncates to `nMaxCount - 1` code units + NUL.
pub unsafe extern "win64" fn shim_get_window_text_w(hwnd: u64, buf: *mut u16, max: i32) -> i32 {
    if buf.is_null() || max <= 0 {
        return 0;
    }
    let text = with_ctx(|ctx| u::get_window_text(ctx, WinHandle(hwnd))).unwrap_or_default();
    let (wbuf, n) = u::copy_text_truncated(&text, max as usize);
    core::ptr::copy_nonoverlapping(wbuf.as_ptr(), buf, wbuf.len());
    n as i32
}

/// `GetWindowTextLengthW(HWND)` -> int (text length in chars, excl NUL).
pub unsafe extern "win64" fn shim_get_window_text_length_w(hwnd: u64) -> i32 {
    with_ctx(|ctx| u::get_window_text_length(ctx, WinHandle(hwnd)))
}

/// `PostQuitMessage(int)` — posts WM_QUIT so the message loop's `GetMessage`
/// returns FALSE and the app exits cleanly.
pub unsafe extern "win64" fn shim_post_quit_message(exit_code: i32) {
    with_ctx(|ctx| u::post_quit_message(ctx, exit_code))
}

/// A zeroed `Msg` (it derives neither Default nor a const ctor) for the get/peek
/// shims to fill.
fn blank_msg() -> crate::Msg {
    crate::Msg {
        hwnd: WinHandle(0),
        message: 0,
        wparam: 0,
        lparam: 0,
        time: 0,
        pt: crate::Point { x: 0, y: 0 },
    }
}

/// Read a guest `MSG` (x64 layout, 48 bytes) into our `Msg`. Offsets: `hwnd` @0,
/// `message` @8, `wParam` @16, `lParam` @24, `time` @32, `pt.x` @36, `pt.y` @40.
///
/// # Safety
/// `p` must point at a readable >=48-byte guest `MSG`.
unsafe fn read_guest_msg(p: *const u8) -> crate::Msg {
    crate::Msg {
        hwnd: WinHandle(core::ptr::read_unaligned(p as *const u64)),
        message: core::ptr::read_unaligned(p.add(8) as *const u32),
        wparam: core::ptr::read_unaligned(p.add(16) as *const u64),
        lparam: core::ptr::read_unaligned(p.add(24) as *const i64),
        time: core::ptr::read_unaligned(p.add(32) as *const u32),
        pt: crate::Point {
            x: core::ptr::read_unaligned(p.add(36) as *const i32),
            y: core::ptr::read_unaligned(p.add(40) as *const i32),
        },
    }
}

/// Write our `Msg` into a guest `MSG` (same 48-byte layout as [`read_guest_msg`]).
///
/// # Safety
/// `p` must point at a writable >=48-byte guest `MSG`.
unsafe fn write_guest_msg(p: *mut u8, m: &crate::Msg) {
    core::ptr::write_unaligned(p as *mut u64, m.hwnd.0);
    core::ptr::write_unaligned(p.add(8) as *mut u32, m.message);
    core::ptr::write_unaligned(p.add(16) as *mut u64, m.wparam);
    core::ptr::write_unaligned(p.add(24) as *mut i64, m.lparam);
    core::ptr::write_unaligned(p.add(32) as *mut u32, m.time);
    core::ptr::write_unaligned(p.add(36) as *mut i32, m.pt.x);
    core::ptr::write_unaligned(p.add(40) as *mut i32, m.pt.y);
}

/// `GetMessageW(LPMSG, HWND, UINT, UINT)` -> BOOL. Fills the guest `MSG`; returns
/// FALSE on WM_QUIT (loop exit), nonzero otherwise.
pub unsafe extern "win64" fn shim_get_message_w(
    lp_msg: *mut u8,
    hwnd: u64,
    filter_min: u32,
    filter_max: u32,
) -> i32 {
    if lp_msg.is_null() {
        return -1;
    }
    let mut m = blank_msg();
    let r = with_ctx(|ctx| u::get_message_w(ctx, &mut m, WinHandle(hwnd), filter_min, filter_max));
    write_guest_msg(lp_msg, &m);
    r.0
}

/// `PeekMessageW(LPMSG, HWND, UINT, UINT, UINT)` -> BOOL. Non-blocking; writes the
/// guest `MSG` only when one is available.
pub unsafe extern "win64" fn shim_peek_message_w(
    lp_msg: *mut u8,
    hwnd: u64,
    filter_min: u32,
    filter_max: u32,
    remove_msg: u32,
) -> i32 {
    if lp_msg.is_null() {
        return 0;
    }
    let mut m = blank_msg();
    let r = with_ctx(|ctx| {
        u::peek_message_w(
            ctx,
            &mut m,
            WinHandle(hwnd),
            filter_min,
            filter_max,
            remove_msg,
        )
    });
    if r.0 != 0 {
        write_guest_msg(lp_msg, &m);
    }
    r.0
}

/// `TranslateMessage(const MSG*)` -> BOOL. Reads the guest `MSG`.
pub unsafe extern "win64" fn shim_translate_message(lp_msg: *const u8) -> i32 {
    if lp_msg.is_null() {
        return 0;
    }
    let m = read_guest_msg(lp_msg);
    with_ctx(|ctx| u::translate_message(ctx, &m).0)
}

/// `DispatchMessageW(const MSG*)` -> LRESULT. Reads the guest `MSG` and routes it
/// to the window's stored WndProc (the real dispatch core in `user32.rs`).
pub unsafe extern "win64" fn shim_dispatch_message_w(lp_msg: *const u8) -> i64 {
    if lp_msg.is_null() {
        return 0;
    }
    let m = read_guest_msg(lp_msg);
    // Classify under the ctx lock. A system control (EDIT/…) runs its built-in
    // proc IN the lock — it's host Rust that does NOT re-enter the API. A guest
    // WndProc must run OUTSIDE the lock: it re-enters the API
    // (BeginPaint/FillRect/...) -> `with_ctx`, which is NOT reentrant, so calling
    // it under the lock would deadlock (every GUI app would hang).
    enum Action {
        Guest(u64),
        Handled(i64),
    }
    let action = with_ctx(|ctx| match u::classify_dispatch(ctx, m.hwnd) {
        u::Dispatch::Guest(addr) => Action::Guest(addr),
        u::Dispatch::Builtin => Action::Handled(u::run_builtin_proc(
            ctx, m.hwnd, m.message, m.wparam, m.lparam,
        )),
        u::Dispatch::None => Action::Handled(0),
    });
    match action {
        Action::Guest(addr) => u::invoke_wndproc(addr, m.hwnd.0, m.message, m.wparam, m.lparam),
        Action::Handled(r) => r,
    }
}

/// `PostMessageW(HWND, UINT, WPARAM, LPARAM)` -> BOOL. Enqueues a message (no
/// WndProc call — pure enqueue, so no reentrancy concern). Used to deliver input
/// (WM_KEYDOWN) into the queue, which `TranslateMessage` turns into WM_CHAR.
pub unsafe extern "win64" fn shim_post_message_w(
    hwnd: u64,
    msg: u32,
    wparam: u64,
    lparam: i64,
) -> i32 {
    with_ctx(|ctx| u::post_message_w(ctx, WinHandle(hwnd), msg, wparam, lparam).0)
}

// ---------------------------------------------------------------------------
// gdi32.dll — WM_PAINT software raster into the window surface (Phase C)
// ---------------------------------------------------------------------------

/// Read a guest `RECT` (x64: left@0, top@4, right@8, bottom@12, all LONG).
///
/// # Safety
/// `p` must point at a readable >=16-byte guest `RECT`.
unsafe fn read_guest_rect(p: *const u8) -> crate::Rect {
    crate::Rect {
        left: core::ptr::read_unaligned(p as *const i32),
        top: core::ptr::read_unaligned(p.add(4) as *const i32),
        right: core::ptr::read_unaligned(p.add(8) as *const i32),
        bottom: core::ptr::read_unaligned(p.add(12) as *const i32),
    }
}

/// Write a `crate::Rect` into a guest `RECT` (same x64 layout as `read_guest_rect`).
///
/// # Safety
/// `p` must point at a writable >=16-byte guest `RECT`.
unsafe fn write_guest_rect(p: *mut u8, r: &crate::Rect) {
    core::ptr::write_unaligned(p as *mut i32, r.left);
    core::ptr::write_unaligned(p.add(4) as *mut i32, r.top);
    core::ptr::write_unaligned(p.add(8) as *mut i32, r.right);
    core::ptr::write_unaligned(p.add(12) as *mut i32, r.bottom);
}

/// Read a guest `POINT` (x64: x@0, y@4, both LONG).
///
/// # Safety
/// `p` must point at a readable >=8-byte guest `POINT`.
unsafe fn read_guest_point(p: *const u8) -> crate::Point {
    crate::Point {
        x: core::ptr::read_unaligned(p as *const i32),
        y: core::ptr::read_unaligned(p.add(4) as *const i32),
    }
}

/// Write a `crate::Point` into a guest `POINT`.
///
/// # Safety
/// `p` must point at a writable >=8-byte guest `POINT`.
unsafe fn write_guest_point(p: *mut u8, pt: &crate::Point) {
    core::ptr::write_unaligned(p as *mut i32, pt.x);
    core::ptr::write_unaligned(p.add(4) as *mut i32, pt.y);
}

/// `CreateSolidBrush(COLORREF)` -> HBRUSH. Stores the color so `FillRect` can use it.
pub unsafe extern "win64" fn shim_create_solid_brush(color: u32) -> u64 {
    with_ctx(|ctx| g::create_solid_brush(ctx, color).0)
}

/// `GetDC(HWND)` -> HDC (window-bound).
pub unsafe extern "win64" fn shim_get_dc(hwnd: u64) -> u64 {
    with_ctx(|ctx| g::get_dc(ctx, WinHandle(hwnd)).0)
}

/// `ReleaseDC(HWND, HDC)` -> int.
pub unsafe extern "win64" fn shim_release_dc(hwnd: u64, hdc: u64) -> i32 {
    with_ctx(|ctx| g::release_dc(ctx, WinHandle(hwnd), WinHandle(hdc)))
}

/// `FillRect(HDC, const RECT*, HBRUSH)` -> int. Rasters into the window surface.
pub unsafe extern "win64" fn shim_fill_rect(hdc: u64, rect: *const u8, brush: u64) -> i32 {
    if rect.is_null() {
        return 0;
    }
    let r = read_guest_rect(rect);
    with_ctx(|ctx| g::fill_rect(ctx, WinHandle(hdc), &r, WinHandle(brush)))
}

/// `BeginPaint(HWND, LPPAINTSTRUCT)` -> HDC. Fills the guest PAINTSTRUCT
/// (`hdc`@0, `fErase`@8, `rcPaint`@12).
pub unsafe extern "win64" fn shim_begin_paint(hwnd: u64, ps: *mut u8) -> u64 {
    let mut s = crate::PaintStruct {
        hdc: WinHandle(0),
        erase: crate::WinBool(0),
        rc_paint: crate::Rect::default(),
    };
    let hdc = with_ctx(|ctx| g::begin_paint(ctx, WinHandle(hwnd), &mut s));
    if !ps.is_null() {
        core::ptr::write_unaligned(ps as *mut u64, s.hdc.0);
        core::ptr::write_unaligned(ps.add(8) as *mut i32, s.erase.0);
        core::ptr::write_unaligned(ps.add(12) as *mut i32, s.rc_paint.left);
        core::ptr::write_unaligned(ps.add(16) as *mut i32, s.rc_paint.top);
        core::ptr::write_unaligned(ps.add(20) as *mut i32, s.rc_paint.right);
        core::ptr::write_unaligned(ps.add(24) as *mut i32, s.rc_paint.bottom);
    }
    hdc.0
}

/// `EndPaint(HWND, const PAINTSTRUCT*)` -> BOOL. Presents the window surface.
pub unsafe extern "win64" fn shim_end_paint(hwnd: u64, _ps: *const u8) -> i32 {
    with_ctx(|ctx| g::end_paint(ctx, WinHandle(hwnd)).0)
}

/// `TextOutW(HDC, int x, int y, LPCWSTR, int c)` -> BOOL. `c` is the char count
/// (the string is NOT NUL-terminated). Rasters 8x8 glyphs into the window surface.
pub unsafe extern "win64" fn shim_text_out_w(
    hdc: u64,
    x: i32,
    y: i32,
    lp: *const u16,
    c: i32,
) -> i32 {
    if lp.is_null() || c < 0 {
        return 0;
    }
    let s = core::slice::from_raw_parts(lp, c as usize);
    with_ctx(|ctx| g::text_out_w(ctx, WinHandle(hdc), x, y, s).0)
}

// ---------------------------------------------------------------------------
// comdlg32.dll — File Open/Save common dialogs (headless auto-confirm)
// ---------------------------------------------------------------------------

/// Shared body for `GetSaveFileNameW`/`GetOpenFileNameW`: marshal `OPENFILENAMEW`
/// (x64), confirm a path via [`c::resolve_dialog_path`], and write it back. There
/// is no interactive picker — the app's pre-set `lpstrFile` is honored (or a
/// default supplied), exactly like an automated Windows session.
///
/// `OPENFILENAMEW` x64 offsets used: `lpstrFile` (LPWSTR, in/out) @48,
/// `nMaxFile` (DWORD, buffer size in WCHARs incl NUL) @56.
///
/// # Safety
/// `ofn` must point at a readable/writable `OPENFILENAMEW` whose `lpstrFile`
/// names a writable `nMaxFile`-WCHAR buffer.
unsafe fn common_dialog_confirm(ofn: *mut u8) -> i32 {
    if ofn.is_null() {
        return 0; // FALSE — no struct (the app treats this as cancel)
    }
    let file_ptr = core::ptr::read_unaligned(ofn.add(48) as *const u64) as usize as *mut u16;
    let n_max = core::ptr::read_unaligned(ofn.add(56) as *const u32) as usize;
    if file_ptr.is_null() || n_max == 0 {
        return 0;
    }
    let current = crate::wide_to_string(wide_cstr(file_ptr as *const u16));
    let path = c::resolve_dialog_path(&current);
    // Write the resolved path back (at most n_max-1 WCHARs + NUL).
    let (wbuf, _) = u::copy_text_truncated(&path, n_max);
    core::ptr::copy_nonoverlapping(wbuf.as_ptr(), file_ptr, wbuf.len());
    1 // TRUE — a path was confirmed
}

/// `GetSaveFileNameW(LPOPENFILENAMEW)` -> BOOL. Headless auto-confirm Save dialog.
pub unsafe extern "win64" fn shim_get_save_file_name_w(ofn: *mut u8) -> i32 {
    common_dialog_confirm(ofn)
}

/// `GetOpenFileNameW(LPOPENFILENAMEW)` -> BOOL. Headless auto-confirm Open dialog.
pub unsafe extern "win64" fn shim_get_open_file_name_w(ofn: *mut u8) -> i32 {
    common_dialog_confirm(ofn)
}

// ---------------------------------------------------------------------------
// user32.dll — menus (menu bar / popup -> WM_COMMAND)
// ---------------------------------------------------------------------------

/// `CreateMenu()` -> HMENU.
pub unsafe extern "win64" fn shim_create_menu() -> u64 {
    with_ctx(|ctx| u::create_menu(ctx).0)
}

/// `CreatePopupMenu()` -> HMENU.
pub unsafe extern "win64" fn shim_create_popup_menu() -> u64 {
    with_ctx(|ctx| u::create_popup_menu(ctx).0)
}

/// `AppendMenuW(HMENU, UINT uFlags, UINT_PTR uIDNewItem, LPCWSTR lpNewItem)` ->
/// BOOL. For MF_POPUP, `uIDNewItem` is the submenu HMENU; for MF_STRING it is the
/// command id; `lpNewItem` is the label (NULL for a separator).
pub unsafe extern "win64" fn shim_append_menu_w(
    hmenu: u64,
    flags: u32,
    id_or_submenu: u64,
    lp_text: *const u16,
) -> i32 {
    let text = if flags & u::MF_SEPARATOR != 0 || lp_text.is_null() {
        alloc::string::String::new()
    } else {
        crate::wide_to_string(wide_cstr(lp_text))
    };
    with_ctx(|ctx| u::append_menu(ctx, WinHandle(hmenu), flags, id_or_submenu, &text).0)
}

/// `SetMenu(HWND, HMENU)` -> BOOL.
pub unsafe extern "win64" fn shim_set_menu(hwnd: u64, hmenu: u64) -> i32 {
    with_ctx(|ctx| u::set_menu(ctx, WinHandle(hwnd), WinHandle(hmenu)).0)
}

/// `GetMenu(HWND)` -> HMENU (0 if none).
pub unsafe extern "win64" fn shim_get_menu(hwnd: u64) -> u64 {
    with_ctx(|ctx| u::get_menu(ctx, WinHandle(hwnd)).0)
}

/// `GetMenuItemCount(HMENU)` -> int (-1 on error).
pub unsafe extern "win64" fn shim_get_menu_item_count(hmenu: u64) -> i32 {
    with_ctx(|ctx| u::get_menu_item_count(ctx, WinHandle(hmenu)))
}

/// `GetMenuItemID(HMENU, int nPos)` -> UINT (0xFFFFFFFF on error / submenu).
pub unsafe extern "win64" fn shim_get_menu_item_id(hmenu: u64, pos: i32) -> u32 {
    with_ctx(|ctx| u::get_menu_item_id(ctx, WinHandle(hmenu), pos))
}

/// `DestroyMenu(HMENU)` -> BOOL.
pub unsafe extern "win64" fn shim_destroy_menu(hmenu: u64) -> i32 {
    with_ctx(|ctx| u::destroy_menu(ctx, WinHandle(hmenu)).0)
}

// ---------------------------------------------------------------------------
// user32.dll — common window management / dialogs (implemented in user32.rs,
// now IAT-wired so a guest reaches them instead of the fail-loud trampoline)
// ---------------------------------------------------------------------------

/// `MessageBoxW(HWND, LPCWSTR text, LPCWSTR caption, UINT type)` -> int. Headless
/// auto-confirm (returns IDOK/IDYES per the buttons) — nearly every GUI app shows
/// one for errors/confirmations.
pub unsafe extern "win64" fn shim_message_box_w(
    hwnd: u64,
    text: *const u16,
    caption: *const u16,
    utype: u32,
) -> i32 {
    let t = wide_cstr(text);
    let c = wide_cstr(caption);
    with_ctx(|ctx| u::message_box_w(ctx, WinHandle(hwnd), t, c, utype))
}

/// `MessageBoxA(HWND, LPCSTR, LPCSTR, UINT)` -> int.
pub unsafe extern "win64" fn shim_message_box_a(
    hwnd: u64,
    text: *const u8,
    caption: *const u8,
    utype: u32,
) -> i32 {
    let t = ansi_cstr(text);
    let c = ansi_cstr(caption);
    with_ctx(|ctx| u::message_box_a(ctx, WinHandle(hwnd), t, c, utype))
}

/// `GetClientRect(HWND, LPRECT)` -> BOOL. Writes the client rect out.
pub unsafe extern "win64" fn shim_get_client_rect(hwnd: u64, rect: *mut u8) -> i32 {
    if rect.is_null() {
        return 0;
    }
    let mut r = crate::Rect::default();
    let ok = with_ctx(|ctx| u::get_client_rect(ctx, WinHandle(hwnd), &mut r).0);
    if ok != 0 {
        write_guest_rect(rect, &r);
    }
    ok
}

/// `GetWindowRect(HWND, LPRECT)` -> BOOL. Writes the window rect out.
pub unsafe extern "win64" fn shim_get_window_rect(hwnd: u64, rect: *mut u8) -> i32 {
    if rect.is_null() {
        return 0;
    }
    let mut r = crate::Rect::default();
    let ok = with_ctx(|ctx| u::get_window_rect(ctx, WinHandle(hwnd), &mut r).0);
    if ok != 0 {
        write_guest_rect(rect, &r);
    }
    ok
}

/// `DestroyWindow(HWND)` -> BOOL.
pub unsafe extern "win64" fn shim_destroy_window(hwnd: u64) -> i32 {
    with_ctx(|ctx| u::destroy_window(ctx, WinHandle(hwnd)).0)
}

/// `MoveWindow(HWND, int x, int y, int w, int h, BOOL repaint)` -> BOOL.
pub unsafe extern "win64" fn shim_move_window(
    hwnd: u64,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    repaint: i32,
) -> i32 {
    with_ctx(|ctx| u::move_window(ctx, WinHandle(hwnd), x, y, w, h, WinBool(repaint)).0)
}

/// `SetWindowPos(HWND, HWND insertAfter, int x, int y, int cx, int cy, UINT flags)` -> BOOL.
#[allow(clippy::too_many_arguments)]
pub unsafe extern "win64" fn shim_set_window_pos(
    hwnd: u64,
    insert_after: u64,
    x: i32,
    y: i32,
    cx: i32,
    cy: i32,
    flags: u32,
) -> i32 {
    with_ctx(|ctx| {
        u::set_window_pos(
            ctx,
            WinHandle(hwnd),
            WinHandle(insert_after),
            x,
            y,
            cx,
            cy,
            flags,
        )
        .0
    })
}

/// `GetWindowLongW(HWND, int index)` -> LONG (32-bit; truncates a 64-bit value).
pub unsafe extern "win64" fn shim_get_window_long_w(hwnd: u64, index: i32) -> i32 {
    with_ctx(|ctx| u::get_window_long_w(ctx, WinHandle(hwnd), index) as i32)
}

/// `SetWindowLongW(HWND, int index, LONG newLong)` -> LONG (previous value).
pub unsafe extern "win64" fn shim_set_window_long_w(hwnd: u64, index: i32, new_long: i32) -> i32 {
    with_ctx(|ctx| u::set_window_long_w(ctx, WinHandle(hwnd), index, new_long as i64) as i32)
}

/// `GetWindowLongPtrW(HWND, int index)` -> LONG_PTR (full 64-bit).
pub unsafe extern "win64" fn shim_get_window_long_ptr_w(hwnd: u64, index: i32) -> i64 {
    with_ctx(|ctx| u::get_window_long_ptr_w(ctx, WinHandle(hwnd), index))
}

/// `SetWindowLongPtrW(HWND, int index, LONG_PTR newLong)` -> LONG_PTR (previous).
pub unsafe extern "win64" fn shim_set_window_long_ptr_w(
    hwnd: u64,
    index: i32,
    new_long: i64,
) -> i64 {
    with_ctx(|ctx| u::set_window_long_ptr_w(ctx, WinHandle(hwnd), index, new_long))
}

/// `InvalidateRect(HWND, const RECT*, BOOL erase)` -> BOOL.
pub unsafe extern "win64" fn shim_invalidate_rect(hwnd: u64, rect: *const u8, erase: i32) -> i32 {
    let r = if rect.is_null() {
        None
    } else {
        Some(read_guest_rect(rect))
    };
    with_ctx(|ctx| u::invalidate_rect(ctx, WinHandle(hwnd), r.as_ref(), WinBool(erase)).0)
}

/// `ScreenToClient(HWND, LPPOINT)` -> BOOL (in/out POINT).
pub unsafe extern "win64" fn shim_screen_to_client(hwnd: u64, pt: *mut u8) -> i32 {
    if pt.is_null() {
        return 0;
    }
    let mut p = read_guest_point(pt);
    let ok = with_ctx(|ctx| u::screen_to_client(ctx, WinHandle(hwnd), &mut p).0);
    if ok != 0 {
        write_guest_point(pt, &p);
    }
    ok
}

/// `ClientToScreen(HWND, LPPOINT)` -> BOOL (in/out POINT).
pub unsafe extern "win64" fn shim_client_to_screen(hwnd: u64, pt: *mut u8) -> i32 {
    if pt.is_null() {
        return 0;
    }
    let mut p = read_guest_point(pt);
    let ok = with_ctx(|ctx| u::client_to_screen(ctx, WinHandle(hwnd), &mut p).0);
    if ok != 0 {
        write_guest_point(pt, &p);
    }
    ok
}

/// `AdjustWindowRect(LPRECT, DWORD style, BOOL menu)` -> BOOL (in/out RECT).
pub unsafe extern "win64" fn shim_adjust_window_rect(rect: *mut u8, style: u32, menu: i32) -> i32 {
    if rect.is_null() {
        return 0;
    }
    let mut r = read_guest_rect(rect);
    let ok = with_ctx(|ctx| u::adjust_window_rect(ctx, &mut r, style, WinBool(menu)).0);
    if ok != 0 {
        write_guest_rect(rect, &r);
    }
    ok
}

/// `AdjustWindowRectEx(LPRECT, DWORD style, BOOL menu, DWORD exStyle)` -> BOOL.
pub unsafe extern "win64" fn shim_adjust_window_rect_ex(
    rect: *mut u8,
    style: u32,
    menu: i32,
    ex_style: u32,
) -> i32 {
    if rect.is_null() {
        return 0;
    }
    let mut r = read_guest_rect(rect);
    let ok =
        with_ctx(|ctx| u::adjust_window_rect_ex(ctx, &mut r, style, WinBool(menu), ex_style).0);
    if ok != 0 {
        write_guest_rect(rect, &r);
    }
    ok
}

// ---------------------------------------------------------------------------
// gdi32.dll — common DC / object / drawing primitives (implemented in gdi32.rs,
// now IAT-wired; previously only CreateSolidBrush + TextOutW were exposed)
// ---------------------------------------------------------------------------

pub unsafe extern "win64" fn shim_delete_dc(hdc: u64) -> i32 {
    with_ctx(|ctx| g::delete_dc(ctx, WinHandle(hdc)).0)
}
pub unsafe extern "win64" fn shim_create_compatible_dc(hdc: u64) -> u64 {
    with_ctx(|ctx| g::create_compatible_dc(ctx, WinHandle(hdc)).0)
}
pub unsafe extern "win64" fn shim_select_object(hdc: u64, obj: u64) -> u64 {
    with_ctx(|ctx| g::select_object(ctx, WinHandle(hdc), WinHandle(obj)).0)
}
pub unsafe extern "win64" fn shim_delete_object(obj: u64) -> i32 {
    with_ctx(|ctx| g::delete_object(ctx, WinHandle(obj)).0)
}
pub unsafe extern "win64" fn shim_create_pen(style: i32, width: i32, color: u32) -> u64 {
    with_ctx(|ctx| g::create_pen(ctx, style, width, color).0)
}
pub unsafe extern "win64" fn shim_get_stock_object(index: i32) -> u64 {
    with_ctx(|ctx| g::get_stock_object(ctx, index).0)
}
pub unsafe extern "win64" fn shim_get_device_caps(hdc: u64, index: i32) -> i32 {
    with_ctx(|ctx| g::get_device_caps(ctx, WinHandle(hdc), index))
}
pub unsafe extern "win64" fn shim_set_bk_mode(hdc: u64, mode: i32) -> i32 {
    with_ctx(|ctx| g::set_bk_mode(ctx, WinHandle(hdc), mode))
}
pub unsafe extern "win64" fn shim_set_text_color(hdc: u64, color: u32) -> u32 {
    with_ctx(|ctx| g::set_text_color(ctx, WinHandle(hdc), color))
}
pub unsafe extern "win64" fn shim_set_bk_color(hdc: u64, color: u32) -> u32 {
    with_ctx(|ctx| g::set_bk_color(ctx, WinHandle(hdc), color))
}
pub unsafe extern "win64" fn shim_get_text_color(hdc: u64) -> u32 {
    with_ctx(|ctx| g::get_text_color(ctx, WinHandle(hdc)))
}
pub unsafe extern "win64" fn shim_rectangle(hdc: u64, l: i32, t: i32, r: i32, b: i32) -> i32 {
    with_ctx(|ctx| g::rectangle(ctx, WinHandle(hdc), l, t, r, b).0)
}
pub unsafe extern "win64" fn shim_ellipse(hdc: u64, l: i32, t: i32, r: i32, b: i32) -> i32 {
    with_ctx(|ctx| g::ellipse(ctx, WinHandle(hdc), l, t, r, b).0)
}
pub unsafe extern "win64" fn shim_line_to(hdc: u64, x: i32, y: i32) -> i32 {
    with_ctx(|ctx| g::line_to(ctx, WinHandle(hdc), x, y).0)
}
/// `MoveToEx(HDC, int x, int y, LPPOINT prev)` -> BOOL (prev may be NULL).
pub unsafe extern "win64" fn shim_move_to_ex(hdc: u64, x: i32, y: i32, prev: *mut u8) -> i32 {
    let mut pt = crate::Point { x: 0, y: 0 };
    let ok = with_ctx(|ctx| g::move_to_ex(ctx, WinHandle(hdc), x, y, Some(&mut pt)).0);
    if ok != 0 && !prev.is_null() {
        write_guest_point(prev, &pt);
    }
    ok
}
pub unsafe extern "win64" fn shim_set_pixel(hdc: u64, x: i32, y: i32, color: u32) -> u32 {
    with_ctx(|ctx| g::set_pixel(ctx, WinHandle(hdc), x, y, color))
}
pub unsafe extern "win64" fn shim_get_pixel(hdc: u64, x: i32, y: i32) -> u32 {
    with_ctx(|ctx| g::get_pixel(ctx, WinHandle(hdc), x, y))
}
pub unsafe extern "win64" fn shim_create_compatible_bitmap(hdc: u64, w: i32, h: i32) -> u64 {
    with_ctx(|ctx| g::create_compatible_bitmap(ctx, WinHandle(hdc), w, h).0)
}
pub unsafe extern "win64" fn shim_save_dc(hdc: u64) -> i32 {
    with_ctx(|ctx| g::save_dc(ctx, WinHandle(hdc)))
}
pub unsafe extern "win64" fn shim_restore_dc(hdc: u64, saved: i32) -> i32 {
    with_ctx(|ctx| g::restore_dc(ctx, WinHandle(hdc), saved).0)
}
/// `BitBlt(HDC dst, x,y,w,h, HDC src, x1,y1, DWORD rop)` -> BOOL.
#[allow(clippy::too_many_arguments)]
pub unsafe extern "win64" fn shim_bit_blt(
    hdc_dst: u64,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    hdc_src: u64,
    x1: i32,
    y1: i32,
    rop: u32,
) -> i32 {
    with_ctx(|ctx| {
        g::bit_blt(
            ctx,
            WinHandle(hdc_dst),
            x,
            y,
            w,
            h,
            WinHandle(hdc_src),
            x1,
            y1,
            rop,
        )
        .0
    })
}
/// `CreateFontW(...14 ints..., LPCWSTR face)` -> HFONT.
#[allow(clippy::too_many_arguments)]
pub unsafe extern "win64" fn shim_create_font_w(
    height: i32,
    width: i32,
    escapement: i32,
    orientation: i32,
    weight: i32,
    italic: u32,
    underline: u32,
    strike_out: u32,
    char_set: u32,
    out_precision: u32,
    clip_precision: u32,
    quality: u32,
    pitch_and_family: u32,
    face: *const u16,
) -> u64 {
    let f = wide_cstr(face);
    with_ctx(|ctx| {
        g::create_font_w(
            ctx,
            height,
            width,
            escapement,
            orientation,
            weight,
            italic,
            underline,
            strike_out,
            char_set,
            out_precision,
            clip_precision,
            quality,
            pitch_and_family,
            f,
        )
        .0
    })
}

// ============================================================================
// 2026-06-30 coverage batch — highest-frequency still-missing imports from the
// real System32 survey (tools/analyze-coverage.ps1 ranks them by how many
// shipping Windows binaries import each name; see
// docs/components/raebridge-readiness.md "coverage hit list"). Every entry is
// real behavior, not a fail-loud stub — Concept §"the compat layer must be
// invisible": an app that imports a common name must keep running.
// ============================================================================

// --- msvcrt CRT: pure mem/str intrinsics (imported dynamically by the /MD CRT;
//     our earlier proof path was /MT, which links them statically). Leaf,
//     allocation-free ops on guest memory (guest VA == host VA) -> native speed,
//     no broker hop. ---

/// `void* memset(void* dest, int c, size_t count)` — returns `dest`.
pub unsafe extern "win64" fn shim_crt_memset(dest: u64, c: i32, count: u64) -> u64 {
    if dest != 0 && count != 0 {
        core::ptr::write_bytes(dest as *mut u8, c as u8, count as usize);
    }
    dest
}

/// `void* memcpy(void* dest, const void* src, size_t count)` — returns `dest`.
pub unsafe extern "win64" fn shim_crt_memcpy(dest: u64, src: u64, count: u64) -> u64 {
    if dest != 0 && src != 0 && count != 0 {
        core::ptr::copy_nonoverlapping(src as *const u8, dest as *mut u8, count as usize);
    }
    dest
}

/// `void* memmove(void* dest, const void* src, size_t count)` — overlap-safe.
pub unsafe extern "win64" fn shim_crt_memmove(dest: u64, src: u64, count: u64) -> u64 {
    if dest != 0 && src != 0 && count != 0 {
        core::ptr::copy(src as *const u8, dest as *mut u8, count as usize);
    }
    dest
}

/// `int memcmp(const void* a, const void* b, size_t count)`.
pub unsafe extern "win64" fn shim_crt_memcmp(a: u64, b: u64, count: u64) -> i32 {
    if count == 0 || a == 0 || b == 0 {
        return 0;
    }
    let (pa, pb) = (a as *const u8, b as *const u8);
    for i in 0..count as usize {
        let (x, y) = (*pa.add(i), *pb.add(i));
        if x != y {
            return x as i32 - y as i32;
        }
    }
    0
}

fn ascii_lower_u16(c: u16) -> u16 {
    if (b'A' as u16..=b'Z' as u16).contains(&c) {
        c + 32
    } else {
        c
    }
}

/// `wchar_t* wcschr(const wchar_t* s, wchar_t c)` — the terminating NUL is part
/// of the string, so `wcschr(s, 0)` returns the NUL slot (matches CRT).
pub unsafe extern "win64" fn shim_crt_wcschr(s: u64, ch: u32) -> u64 {
    if s == 0 {
        return 0;
    }
    let needle = ch as u16;
    let mut p = s as *const u16;
    loop {
        let v = *p;
        if v == needle {
            return p as u64;
        }
        if v == 0 {
            return 0;
        }
        p = p.add(1);
    }
}

/// `int _wcsicmp(const wchar_t* a, const wchar_t* b)` — ASCII case-insensitive.
pub unsafe extern "win64" fn shim_crt_wcsicmp(a: u64, b: u64) -> i32 {
    if a == 0 || b == 0 {
        return if a == b {
            0
        } else if a == 0 {
            -1
        } else {
            1
        };
    }
    let (mut pa, mut pb) = (a as *const u16, b as *const u16);
    loop {
        let (ca, cb) = (ascii_lower_u16(*pa), ascii_lower_u16(*pb));
        if ca != cb {
            return ca as i32 - cb as i32;
        }
        if ca == 0 {
            return 0;
        }
        pa = pa.add(1);
        pb = pb.add(1);
    }
}

// --- msvcrt / ucrtbase stdio: the real formatted-output family, driven by the
//     va_list-consuming engine in crate::msvcrt (host-KAT'd). The modern UCRT
//     routes sprintf/snprintf/printf through the __stdio_common_v*printf core;
//     legacy msvcrt exports the classic variadic entry points. Both are wired so
//     a binary linked either way renders its numbers/strings instead of leaking
//     the raw format string. Concept §"the compat layer must be invisible". ---

/// Number of variadic slots the classic (`...`) printf shims materialize. In
/// the win64 ABI the first slots land in RDX/R8/R9 and the rest are read from
/// the caller's stack frame; 12 covers every real-world printf call (the format
/// engine only consumes as many as the format string names, so unused slots are
/// never inspected).
const VA_SLOTS: usize = 12;

/// UCRT `__stdio_common_vsprintf(opts, buf, count, fmt, locale, va)` — the
/// C99-`snprintf` core. Returns the full formatted length (may exceed `count`);
/// NUL-terminates within `count`. `buf==0`/`count==0` is a valid sizing call.
pub unsafe extern "win64" fn shim_stdio_common_vsprintf(
    _options: u64,
    buffer: u64,
    count: u64,
    format: u64,
    _locale: u64,
    va: u64,
) -> i32 {
    let fmt = crate::msvcrt::read_format_bytes(format);
    let s = crate::msvcrt::vformat_narrow(&fmt, va);
    if buffer == 0 || count == 0 {
        return s.chars().count() as i32;
    }
    let buf = core::slice::from_raw_parts_mut(buffer as *mut u8, count as usize);
    crate::msvcrt::write_narrow_c99(buf, &s)
}

/// UCRT `__stdio_common_vswprintf(opts, buf, count, fmt, locale, va)` (wide).
pub unsafe extern "win64" fn shim_stdio_common_vswprintf(
    _options: u64,
    buffer: u64,
    count: u64,
    format: u64,
    _locale: u64,
    va: u64,
) -> i32 {
    let fmt = crate::msvcrt::read_format_wide(format);
    let s = crate::msvcrt::vformat_wide(&fmt, va);
    if buffer == 0 || count == 0 {
        return s.encode_utf16().count() as i32;
    }
    let buf = core::slice::from_raw_parts_mut(buffer as *mut u16, count as usize);
    crate::msvcrt::write_wide_c99(buf, &s)
}

/// UCRT `__stdio_common_vsnprintf_s(opts, buf, count, maxcount, fmt, locale,
/// va)` — the bounded "secure" narrow core. We honor the buffer `count` cap.
pub unsafe extern "win64" fn shim_stdio_common_vsnprintf_s(
    _options: u64,
    buffer: u64,
    count: u64,
    _maxcount: u64,
    format: u64,
    _locale: u64,
    va: u64,
) -> i32 {
    let fmt = crate::msvcrt::read_format_bytes(format);
    let s = crate::msvcrt::vformat_narrow(&fmt, va);
    if buffer == 0 || count == 0 {
        return s.chars().count() as i32;
    }
    let buf = core::slice::from_raw_parts_mut(buffer as *mut u8, count as usize);
    crate::msvcrt::write_narrow_c99(buf, &s)
}

/// UCRT `__stdio_common_vfprintf(opts, stream, fmt, locale, va)` — formatted
/// write to a stdio stream. We model only the process console (fd 1); stderr is
/// folded onto the same visible output. Returns the byte count written.
pub unsafe extern "win64" fn shim_stdio_common_vfprintf(
    _options: u64,
    _stream: u64,
    format: u64,
    _locale: u64,
    va: u64,
) -> i32 {
    let fmt = crate::msvcrt::read_format_bytes(format);
    let s = crate::msvcrt::vformat_narrow(&fmt, va);
    let bytes = latin1_bytes(&s);
    syscalls::sys_write(1, &bytes);
    bytes.len() as i32
}

/// `FILE* __acrt_iob_func(unsigned index)` — the UCRT stdio-stream accessor
/// (`stdin`/`stdout`/`stderr` == 0/1/2). We return a synthetic non-null handle
/// (`index + 1`) so the guest never dereferences NULL; the vfprintf shim ignores
/// the specific stream and writes to the console.
pub unsafe extern "win64" fn shim_acrt_iob_func(index: u32) -> u64 {
    (index as u64) + 1
}

/// Latin-1 flatten of a formatted `String` to output bytes (chars > 0xFF → '?').
fn latin1_bytes(s: &str) -> Vec<u8> {
    s.chars()
        .map(|c| if (c as u32) <= 0xFF { c as u8 } else { b'?' })
        .collect()
}

/// Classic variadic `int sprintf(char* buf, const char* fmt, ...)`. Unbounded
/// per contract (caller sized `buf`); writes the whole string + NUL.
pub unsafe extern "win64" fn shim_sprintf(
    buffer: u64,
    format: u64,
    a0: u64,
    a1: u64,
    a2: u64,
    a3: u64,
    a4: u64,
    a5: u64,
    a6: u64,
    a7: u64,
    a8: u64,
    a9: u64,
    a10: u64,
    a11: u64,
) -> i32 {
    let args: [u64; VA_SLOTS] = [a0, a1, a2, a3, a4, a5, a6, a7, a8, a9, a10, a11];
    let fmt = crate::msvcrt::read_format_bytes(format);
    let s = crate::msvcrt::vformat_args_narrow(&fmt, &args);
    if buffer == 0 {
        return s.chars().count() as i32;
    }
    let total = s.chars().count() + 1;
    let buf = core::slice::from_raw_parts_mut(buffer as *mut u8, total);
    crate::msvcrt::write_narrow_c99(buf, &s)
}

/// Classic variadic `int snprintf(char* buf, size_t count, const char* fmt,
/// ...)` (C99: returns the full length, NUL-terminates within `count`).
pub unsafe extern "win64" fn shim_snprintf(
    buffer: u64,
    count: u64,
    format: u64,
    a0: u64,
    a1: u64,
    a2: u64,
    a3: u64,
    a4: u64,
    a5: u64,
    a6: u64,
    a7: u64,
    a8: u64,
    a9: u64,
    a10: u64,
    a11: u64,
) -> i32 {
    let args: [u64; VA_SLOTS] = [a0, a1, a2, a3, a4, a5, a6, a7, a8, a9, a10, a11];
    let fmt = crate::msvcrt::read_format_bytes(format);
    let s = crate::msvcrt::vformat_args_narrow(&fmt, &args);
    if buffer == 0 || count == 0 {
        return s.chars().count() as i32;
    }
    let buf = core::slice::from_raw_parts_mut(buffer as *mut u8, count as usize);
    crate::msvcrt::write_narrow_c99(buf, &s)
}

/// Classic variadic `int _snprintf(char* buf, size_t count, const char* fmt,
/// ...)` — legacy MS contract: returns `-1` on truncation (not the full length).
pub unsafe extern "win64" fn shim_legacy_snprintf(
    buffer: u64,
    count: u64,
    format: u64,
    a0: u64,
    a1: u64,
    a2: u64,
    a3: u64,
    a4: u64,
    a5: u64,
    a6: u64,
    a7: u64,
    a8: u64,
    a9: u64,
    a10: u64,
    a11: u64,
) -> i32 {
    let args: [u64; VA_SLOTS] = [a0, a1, a2, a3, a4, a5, a6, a7, a8, a9, a10, a11];
    let fmt = crate::msvcrt::read_format_bytes(format);
    let s = crate::msvcrt::vformat_args_narrow(&fmt, &args);
    if buffer == 0 || count == 0 {
        return -1;
    }
    let buf = core::slice::from_raw_parts_mut(buffer as *mut u8, count as usize);
    crate::msvcrt::write_narrow_into(buf, count as usize, &s)
}

/// Classic variadic wide `int swprintf(wchar_t* buf, size_t count,
/// const wchar_t* fmt, ...)` (modern MSVC count form; C99 return).
pub unsafe extern "win64" fn shim_swprintf(
    buffer: u64,
    count: u64,
    format: u64,
    a0: u64,
    a1: u64,
    a2: u64,
    a3: u64,
    a4: u64,
    a5: u64,
    a6: u64,
    a7: u64,
    a8: u64,
    a9: u64,
    a10: u64,
    a11: u64,
) -> i32 {
    let args: [u64; VA_SLOTS] = [a0, a1, a2, a3, a4, a5, a6, a7, a8, a9, a10, a11];
    let fmt = crate::msvcrt::read_format_wide(format);
    let s = crate::msvcrt::vformat_args_wide(&fmt, &args);
    if buffer == 0 || count == 0 {
        return s.encode_utf16().count() as i32;
    }
    let buf = core::slice::from_raw_parts_mut(buffer as *mut u16, count as usize);
    crate::msvcrt::write_wide_c99(buf, &s)
}

/// Classic variadic `int printf(const char* fmt, ...)` — formats and writes to
/// the process console (fd 1). Returns the byte count written.
pub unsafe extern "win64" fn shim_printf(
    format: u64,
    a0: u64,
    a1: u64,
    a2: u64,
    a3: u64,
    a4: u64,
    a5: u64,
    a6: u64,
    a7: u64,
    a8: u64,
    a9: u64,
    a10: u64,
    a11: u64,
) -> i32 {
    let args: [u64; VA_SLOTS] = [a0, a1, a2, a3, a4, a5, a6, a7, a8, a9, a10, a11];
    let fmt = crate::msvcrt::read_format_bytes(format);
    let s = crate::msvcrt::vformat_args_narrow(&fmt, &args);
    let bytes = latin1_bytes(&s);
    syscalls::sys_write(1, &bytes);
    bytes.len() as i32
}

/// `int vprintf(const char* fmt, va_list va)` — non-variadic (va is a pointer).
pub unsafe extern "win64" fn shim_vprintf(format: u64, va: u64) -> i32 {
    let fmt = crate::msvcrt::read_format_bytes(format);
    let s = crate::msvcrt::vformat_narrow(&fmt, va);
    let bytes = latin1_bytes(&s);
    syscalls::sys_write(1, &bytes);
    bytes.len() as i32
}

/// `int fputs(const char* str, FILE* stream)` — write a NUL-terminated string to
/// the console. Returns a non-negative value on success (CRT returns `>= 0`).
pub unsafe extern "win64" fn shim_fputs(str_ptr: u64, _stream: u64) -> i32 {
    if str_ptr == 0 {
        return -1;
    }
    let bytes = crate::msvcrt::read_format_bytes(str_ptr);
    let n = bytes.len().saturating_sub(1); // drop trailing NUL
    syscalls::sys_write(1, &bytes[..n]);
    0
}

/// `int puts(const char* str)` — `fputs` + a trailing newline. Returns `>= 0`.
pub unsafe extern "win64" fn shim_puts(str_ptr: u64) -> i32 {
    if str_ptr == 0 {
        return -1;
    }
    let mut bytes = crate::msvcrt::read_format_bytes(str_ptr);
    let n = bytes.len().saturating_sub(1);
    bytes.truncate(n);
    bytes.push(b'\n');
    syscalls::sys_write(1, &bytes);
    0
}

/// `int fputc(int ch, FILE* stream)` — write one byte to the console.
pub unsafe extern "win64" fn shim_fputc(ch: i32, _stream: u64) -> i32 {
    let b = [ch as u8];
    syscalls::sys_write(1, &b);
    ch & 0xFF
}

/// `size_t fwrite(const void* ptr, size_t size, size_t count, FILE* stream)` —
/// write `size*count` bytes to the console. Returns `count` on success.
pub unsafe extern "win64" fn shim_fwrite(ptr: u64, size: u64, count: u64, _stream: u64) -> u64 {
    let total = size.saturating_mul(count);
    if ptr == 0 || total == 0 {
        return 0;
    }
    let bytes = core::slice::from_raw_parts(ptr as *const u8, total as usize);
    syscalls::sys_write(1, bytes);
    count
}

// --- msvcrt / ucrtbase: the C string / char-class / conversion / math library.
//     Leaf, allocation-free operations on guest memory (guest VA == host VA), so
//     they run at native speed with no broker hop. The pure numeric/parse logic
//     is the host-KAT'd code in crate::msvcrt; the string/mem ops are inlined
//     directly on pointers here (exact CRT semantics, including the NUL as part
//     of the string). These are the highest-frequency dynamic /MD CRT imports
//     after the stdio family. Concept §"the compat layer must be invisible". ---

/// `size_t strlen(const char* s)`.
pub unsafe extern "win64" fn shim_strlen(s: u64) -> u64 {
    if s == 0 {
        return 0;
    }
    let mut n = 0u64;
    while core::ptr::read((s + n) as *const u8) != 0 {
        n += 1;
    }
    n
}

/// `size_t strnlen(const char* s, size_t maxlen)`.
pub unsafe extern "win64" fn shim_strnlen(s: u64, maxlen: u64) -> u64 {
    if s == 0 {
        return 0;
    }
    let mut n = 0u64;
    while n < maxlen && core::ptr::read((s + n) as *const u8) != 0 {
        n += 1;
    }
    n
}

/// `int strcmp(const char* a, const char* b)`.
pub unsafe extern "win64" fn shim_strcmp(a: u64, b: u64) -> i32 {
    if a == 0 || b == 0 {
        return (a != b) as i32;
    }
    let mut i = 0u64;
    loop {
        let (x, y) = (
            core::ptr::read((a + i) as *const u8),
            core::ptr::read((b + i) as *const u8),
        );
        if x != y {
            return x as i32 - y as i32;
        }
        if x == 0 {
            return 0;
        }
        i += 1;
    }
}

/// `int strncmp(const char* a, const char* b, size_t n)`.
pub unsafe extern "win64" fn shim_strncmp(a: u64, b: u64, n: u64) -> i32 {
    if n == 0 || a == 0 || b == 0 {
        return 0;
    }
    for i in 0..n {
        let (x, y) = (
            core::ptr::read((a + i) as *const u8),
            core::ptr::read((b + i) as *const u8),
        );
        if x != y {
            return x as i32 - y as i32;
        }
        if x == 0 {
            return 0;
        }
    }
    0
}

#[inline]
fn ascii_lower(c: u8) -> u8 {
    if c.is_ascii_uppercase() {
        c + 32
    } else {
        c
    }
}

/// `int _stricmp(const char* a, const char* b)` — ASCII case-insensitive.
pub unsafe extern "win64" fn shim_stricmp(a: u64, b: u64) -> i32 {
    if a == 0 || b == 0 {
        return (a != b) as i32;
    }
    let mut i = 0u64;
    loop {
        let (x, y) = (
            ascii_lower(core::ptr::read((a + i) as *const u8)),
            ascii_lower(core::ptr::read((b + i) as *const u8)),
        );
        if x != y {
            return x as i32 - y as i32;
        }
        if x == 0 {
            return 0;
        }
        i += 1;
    }
}

/// `int _strcmpi(const char* a, const char* b)` — the older alias of `_stricmp`
/// (distinct fn so the whole-table distinct-address self-test holds).
pub unsafe extern "win64" fn shim_strcmpi(a: u64, b: u64) -> i32 {
    shim_stricmp(a, b)
}

/// `int _strnicmp(const char* a, const char* b, size_t n)`.
pub unsafe extern "win64" fn shim_strnicmp(a: u64, b: u64, n: u64) -> i32 {
    if n == 0 || a == 0 || b == 0 {
        return 0;
    }
    for i in 0..n {
        let (x, y) = (
            ascii_lower(core::ptr::read((a + i) as *const u8)),
            ascii_lower(core::ptr::read((b + i) as *const u8)),
        );
        if x != y {
            return x as i32 - y as i32;
        }
        if x == 0 {
            return 0;
        }
    }
    0
}

/// `char* strcpy(char* dst, const char* src)` — returns `dst`.
pub unsafe extern "win64" fn shim_strcpy(dst: u64, src: u64) -> u64 {
    if dst == 0 || src == 0 {
        return dst;
    }
    let mut i = 0u64;
    loop {
        let b = core::ptr::read((src + i) as *const u8);
        core::ptr::write((dst + i) as *mut u8, b);
        if b == 0 {
            break;
        }
        i += 1;
    }
    dst
}

/// `char* strncpy(char* dst, const char* src, size_t n)` — pads with NUL, does
/// not guarantee termination when `src` is longer than `n` (exact CRT contract).
pub unsafe extern "win64" fn shim_strncpy(dst: u64, src: u64, n: u64) -> u64 {
    if dst == 0 || src == 0 {
        return dst;
    }
    let mut i = 0u64;
    let mut hit_nul = false;
    while i < n {
        let b = if hit_nul {
            0
        } else {
            core::ptr::read((src + i) as *const u8)
        };
        core::ptr::write((dst + i) as *mut u8, b);
        if b == 0 {
            hit_nul = true;
        }
        i += 1;
    }
    dst
}

/// `char* strcat(char* dst, const char* src)` — returns `dst`.
pub unsafe extern "win64" fn shim_strcat(dst: u64, src: u64) -> u64 {
    if dst == 0 || src == 0 {
        return dst;
    }
    let end = dst + shim_strlen(dst);
    shim_strcpy(end, src);
    dst
}

/// `char* strncat(char* dst, const char* src, size_t n)` — appends ≤`n` chars
/// then a NUL. Returns `dst`.
pub unsafe extern "win64" fn shim_strncat(dst: u64, src: u64, n: u64) -> u64 {
    if dst == 0 || src == 0 {
        return dst;
    }
    let mut d = dst + shim_strlen(dst);
    let mut i = 0u64;
    while i < n {
        let b = core::ptr::read((src + i) as *const u8);
        if b == 0 {
            break;
        }
        core::ptr::write(d as *mut u8, b);
        d += 1;
        i += 1;
    }
    core::ptr::write(d as *mut u8, 0);
    dst
}

/// `char* strchr(const char* s, int c)` — the NUL is part of the string.
pub unsafe extern "win64" fn shim_strchr(s: u64, c: i32) -> u64 {
    if s == 0 {
        return 0;
    }
    let needle = c as u8;
    let mut i = 0u64;
    loop {
        let b = core::ptr::read((s + i) as *const u8);
        if b == needle {
            return s + i;
        }
        if b == 0 {
            return 0;
        }
        i += 1;
    }
}

/// `char* strrchr(const char* s, int c)` — last occurrence (NUL matchable).
pub unsafe extern "win64" fn shim_strrchr(s: u64, c: i32) -> u64 {
    if s == 0 {
        return 0;
    }
    let needle = c as u8;
    let mut i = 0u64;
    let mut last = 0u64;
    loop {
        let b = core::ptr::read((s + i) as *const u8);
        if b == needle {
            last = s + i;
        }
        if b == 0 {
            return last;
        }
        i += 1;
    }
}

/// `char* strstr(const char* hay, const char* needle)`.
pub unsafe extern "win64" fn shim_strstr(hay: u64, needle: u64) -> u64 {
    if hay == 0 || needle == 0 {
        return 0;
    }
    let nlen = shim_strlen(needle);
    if nlen == 0 {
        return hay;
    }
    let hlen = shim_strlen(hay);
    if nlen > hlen {
        return 0;
    }
    let first = core::ptr::read(needle as *const u8);
    let mut i = 0u64;
    while i + nlen <= hlen {
        if core::ptr::read((hay + i) as *const u8) == first {
            let mut j = 1u64;
            while j < nlen
                && core::ptr::read((hay + i + j) as *const u8)
                    == core::ptr::read((needle + j) as *const u8)
            {
                j += 1;
            }
            if j == nlen {
                return hay + i;
            }
        }
        i += 1;
    }
    0
}

/// `void* memchr(const void* s, int c, size_t n)`.
pub unsafe extern "win64" fn shim_memchr(s: u64, c: i32, n: u64) -> u64 {
    if s == 0 {
        return 0;
    }
    let needle = c as u8;
    for i in 0..n {
        if core::ptr::read((s + i) as *const u8) == needle {
            return s + i;
        }
    }
    0
}

/// `size_t wcslen(const wchar_t* s)`.
pub unsafe extern "win64" fn shim_wcslen(s: u64) -> u64 {
    if s == 0 {
        return 0;
    }
    let mut n = 0u64;
    while core::ptr::read((s + n * 2) as *const u16) != 0 {
        n += 1;
    }
    n
}

/// `int wcscmp(const wchar_t* a, const wchar_t* b)`.
pub unsafe extern "win64" fn shim_wcscmp(a: u64, b: u64) -> i32 {
    if a == 0 || b == 0 {
        return (a != b) as i32;
    }
    let mut i = 0u64;
    loop {
        let (x, y) = (
            core::ptr::read((a + i * 2) as *const u16),
            core::ptr::read((b + i * 2) as *const u16),
        );
        if x != y {
            return x as i32 - y as i32;
        }
        if x == 0 {
            return 0;
        }
        i += 1;
    }
}

/// `int wcsncmp(const wchar_t* a, const wchar_t* b, size_t n)`.
pub unsafe extern "win64" fn shim_wcsncmp(a: u64, b: u64, n: u64) -> i32 {
    if n == 0 || a == 0 || b == 0 {
        return 0;
    }
    for i in 0..n {
        let (x, y) = (
            core::ptr::read((a + i * 2) as *const u16),
            core::ptr::read((b + i * 2) as *const u16),
        );
        if x != y {
            return x as i32 - y as i32;
        }
        if x == 0 {
            return 0;
        }
    }
    0
}

/// `wchar_t* wcscpy(wchar_t* dst, const wchar_t* src)` — returns `dst`.
pub unsafe extern "win64" fn shim_wcscpy(dst: u64, src: u64) -> u64 {
    if dst == 0 || src == 0 {
        return dst;
    }
    let mut i = 0u64;
    loop {
        let u = core::ptr::read((src + i * 2) as *const u16);
        core::ptr::write((dst + i * 2) as *mut u16, u);
        if u == 0 {
            break;
        }
        i += 1;
    }
    dst
}

/// `wchar_t* wcscat(wchar_t* dst, const wchar_t* src)` — returns `dst`.
pub unsafe extern "win64" fn shim_wcscat(dst: u64, src: u64) -> u64 {
    if dst == 0 || src == 0 {
        return dst;
    }
    let end = dst + shim_wcslen(dst) * 2;
    shim_wcscpy(end, src);
    dst
}

// --- char classification (`<ctype.h>`); operate on an int codepoint, ASCII. ---

pub unsafe extern "win64" fn shim_toupper(c: i32) -> i32 {
    let b = c as u8;
    if b.is_ascii_lowercase() {
        (b - 32) as i32
    } else {
        c
    }
}
pub unsafe extern "win64" fn shim_tolower(c: i32) -> i32 {
    let b = c as u8;
    if b.is_ascii_uppercase() {
        (b + 32) as i32
    } else {
        c
    }
}
pub unsafe extern "win64" fn shim_isdigit(c: i32) -> i32 {
    (c >= 0 && (c as u8).is_ascii_digit()) as i32
}
pub unsafe extern "win64" fn shim_isalpha(c: i32) -> i32 {
    (c >= 0 && (c as u8).is_ascii_alphabetic()) as i32
}
pub unsafe extern "win64" fn shim_isalnum(c: i32) -> i32 {
    (c >= 0 && (c as u8).is_ascii_alphanumeric()) as i32
}
pub unsafe extern "win64" fn shim_isspace(c: i32) -> i32 {
    (c >= 0 && (c as u8).is_ascii_whitespace()) as i32
}
pub unsafe extern "win64" fn shim_isupper(c: i32) -> i32 {
    (c >= 0 && (c as u8).is_ascii_uppercase()) as i32
}
pub unsafe extern "win64" fn shim_islower(c: i32) -> i32 {
    (c >= 0 && (c as u8).is_ascii_lowercase()) as i32
}
pub unsafe extern "win64" fn shim_isxdigit(c: i32) -> i32 {
    (c >= 0 && (c as u8).is_ascii_hexdigit()) as i32
}
pub unsafe extern "win64" fn shim_isprint(c: i32) -> i32 {
    (c >= 0 && (c as u8).is_ascii_graphic() || c == 0x20) as i32
}
pub unsafe extern "win64" fn shim_iscntrl(c: i32) -> i32 {
    (c >= 0 && (c as u8).is_ascii_control()) as i32
}
pub unsafe extern "win64" fn shim_ispunct(c: i32) -> i32 {
    (c >= 0 && (c as u8).is_ascii_punctuation()) as i32
}

// --- string → number conversions (delegate to the host-KAT'd numeric core). ---

unsafe fn cstr_slice<'a>(ptr: u64) -> &'a [u8] {
    if ptr == 0 {
        return &[];
    }
    let len = shim_strlen(ptr) as usize;
    core::slice::from_raw_parts(ptr as *const u8, len)
}

/// `int atoi(const char* s)`.
pub unsafe extern "win64" fn shim_atoi(s: u64) -> i32 {
    crate::msvcrt::atoi(cstr_slice(s))
}
/// `long atol(const char* s)` — Windows `long` is 32-bit.
pub unsafe extern "win64" fn shim_atol(s: u64) -> i32 {
    crate::msvcrt::atol(cstr_slice(s)) as i32
}
/// `__int64 _atoi64(const char* s)`.
pub unsafe extern "win64" fn shim_atoi64(s: u64) -> i64 {
    crate::msvcrt::atol(cstr_slice(s))
}
/// `double atof(const char* s)`.
pub unsafe extern "win64" fn shim_atof(s: u64) -> f64 {
    crate::msvcrt::atof(cstr_slice(s))
}

/// `long strtol(const char* s, char** endptr, int base)`.
pub unsafe extern "win64" fn shim_strtol(s: u64, endptr: u64, base: i32) -> i32 {
    let slice = cstr_slice(s);
    let (val, consumed) = crate::msvcrt::strtol(slice, base as u32);
    if endptr != 0 {
        core::ptr::write(endptr as *mut u64, s + consumed as u64);
    }
    val as i32
}

/// `unsigned long strtoul(const char* s, char** endptr, int base)`.
pub unsafe extern "win64" fn shim_strtoul(s: u64, endptr: u64, base: i32) -> u32 {
    let slice = cstr_slice(s);
    let (val, consumed) = crate::msvcrt::strtoul(slice, base as u32);
    if endptr != 0 {
        core::ptr::write(endptr as *mut u64, s + consumed as u64);
    }
    val as u32
}

/// `__int64 _strtoi64(const char* s, char** endptr, int base)`.
pub unsafe extern "win64" fn shim_strtoi64(s: u64, endptr: u64, base: i32) -> i64 {
    let slice = cstr_slice(s);
    let (val, consumed) = crate::msvcrt::strtol(slice, base as u32);
    if endptr != 0 {
        core::ptr::write(endptr as *mut u64, s + consumed as u64);
    }
    val
}

/// `double strtod(const char* s, char** endptr)`.
pub unsafe extern "win64" fn shim_strtod(s: u64, endptr: u64) -> f64 {
    let slice = cstr_slice(s);
    let (val, consumed) = crate::msvcrt::strtod(slice);
    if endptr != 0 {
        core::ptr::write(endptr as *mut u64, s + consumed as u64);
    }
    val
}

// --- <math.h> (f64; win64 passes/returns in XMM0). Delegate to the libm core. ---

pub unsafe extern "win64" fn shim_sin(x: f64) -> f64 {
    crate::msvcrt::sin(x)
}
pub unsafe extern "win64" fn shim_cos(x: f64) -> f64 {
    crate::msvcrt::cos(x)
}
pub unsafe extern "win64" fn shim_tan(x: f64) -> f64 {
    crate::msvcrt::tan(x)
}
pub unsafe extern "win64" fn shim_atan2(y: f64, x: f64) -> f64 {
    crate::msvcrt::atan2(y, x)
}
pub unsafe extern "win64" fn shim_exp(x: f64) -> f64 {
    crate::msvcrt::exp(x)
}
pub unsafe extern "win64" fn shim_log(x: f64) -> f64 {
    crate::msvcrt::log(x)
}
pub unsafe extern "win64" fn shim_log10(x: f64) -> f64 {
    crate::msvcrt::log10(x)
}
pub unsafe extern "win64" fn shim_pow(b: f64, e: f64) -> f64 {
    crate::msvcrt::pow(b, e)
}
pub unsafe extern "win64" fn shim_sqrt(x: f64) -> f64 {
    crate::msvcrt::sqrt(x)
}
pub unsafe extern "win64" fn shim_ceil(x: f64) -> f64 {
    crate::msvcrt::ceil(x)
}
pub unsafe extern "win64" fn shim_floor(x: f64) -> f64 {
    crate::msvcrt::floor(x)
}
pub unsafe extern "win64" fn shim_fabs(x: f64) -> f64 {
    crate::msvcrt::fabs(x)
}
pub unsafe extern "win64" fn shim_fmod(x: f64, y: f64) -> f64 {
    crate::msvcrt::fmod(x, y)
}

// --- msvcrt / ucrtbase startup + teardown: the sequence __scrt_common_main runs
//     around a /MD binary's main(). `_initterm` runs the C++ static-constructor
//     table; the onexit table + `exit`/`_cexit` run destructors in LIFO order.
//     Getting these right is what lets an unmodified /MD .exe reach main() and
//     leave cleanly (the /MT path was already proven; this generalizes it to the
//     dynamically-linked CRT). Concept §"AthBridge runs Windows apps on day
//     one." The guest-pointer calls run only on-target (guest VA == host VA);
//     the registry bookkeeping they build on is host-KAT'd in crate::msvcrt. ---

/// `void _initterm(_PVFV* first, _PVFV* last)` — call every non-null
/// `void(*)(void)` in `[first, last)` (the C++ static-constructor table).
pub unsafe extern "win64" fn shim_initterm(first: u64, last: u64) {
    let mut p = first;
    while p + 8 <= last.max(first) {
        let fp = core::ptr::read(p as *const u64);
        if fp != 0 {
            let f: extern "win64" fn() = core::mem::transmute(fp);
            f();
        }
        p += 8;
    }
}

/// `int _initterm_e(_PIFV* first, _PIFV* last)` — like `_initterm` but each entry
/// is `int(*)(void)`; stop and return the first non-zero result (an init error).
pub unsafe extern "win64" fn shim_initterm_e(first: u64, last: u64) -> i32 {
    let mut p = first;
    while p + 8 <= last.max(first) {
        let fp = core::ptr::read(p as *const u64);
        if fp != 0 {
            let f: extern "win64" fn() -> i32 = core::mem::transmute(fp);
            let r = f();
            if r != 0 {
                return r;
            }
        }
        p += 8;
    }
    0
}

/// Run every registered onexit/atexit callback in LIFO order (drains once).
unsafe fn run_onexit_callbacks() {
    for fp in crate::msvcrt::onexit_take_all() {
        if fp != 0 {
            let f: extern "win64" fn() = core::mem::transmute(fp);
            f();
        }
    }
}

/// `int atexit(void (*func)(void))` — register a normal-exit callback. Returns 0.
pub unsafe extern "win64" fn shim_atexit(func: u64) -> i32 {
    if crate::msvcrt::onexit_register(func) {
        0
    } else {
        -1
    }
}

/// `_onexit_t _onexit(_onexit_t func)` — legacy alias; returns `func` on success.
pub unsafe extern "win64" fn shim_onexit(func: u64) -> u64 {
    if crate::msvcrt::onexit_register(func) {
        func
    } else {
        0
    }
}

/// `int _crt_atexit(void (*func)(void))` — the UCRT internal atexit. Returns 0.
pub unsafe extern "win64" fn shim_crt_atexit(func: u64) -> i32 {
    if crate::msvcrt::onexit_register(func) {
        0
    } else {
        -1
    }
}

/// `int _register_onexit_function(_onexit_table_t* table, _onexit_t func)`. We
/// keep a single process onexit list (the table identity is irrelevant to the
/// LIFO drain), so the table pointer is accepted and ignored. Returns 0.
pub unsafe extern "win64" fn shim_register_onexit_function(_table: u64, func: u64) -> i32 {
    if crate::msvcrt::onexit_register(func) {
        0
    } else {
        -1
    }
}

/// `int _initialize_onexit_table(_onexit_table_t* table)` — nothing to do; the
/// registry is process-global and always ready. Returns 0.
pub unsafe extern "win64" fn shim_initialize_onexit_table(_table: u64) -> i32 {
    0
}

/// `int _execute_onexit_table(_onexit_table_t* table)` — run the callbacks now.
pub unsafe extern "win64" fn shim_execute_onexit_table(_table: u64) -> i32 {
    run_onexit_callbacks();
    0
}

/// `int _register_thread_local_exe_atexit_callback(void (*func)(void))` — the
/// TLS-destructor registration; folded onto the process onexit list. Returns 0.
pub unsafe extern "win64" fn shim_register_thread_local_exe_atexit_callback(func: u64) -> i32 {
    crate::msvcrt::onexit_register(func);
    0
}

/// `void exit(int code)` — run destructors (LIFO) then terminate the process.
pub unsafe extern "win64" fn shim_exit(code: i32) -> ! {
    run_onexit_callbacks();
    syscalls::sys_exit(code as u64)
}

/// `void _cexit(void)` — run destructors but do NOT terminate (the CRT calls
/// this on the normal return path before its own teardown).
pub unsafe extern "win64" fn shim_cexit() {
    run_onexit_callbacks();
}

/// `void _c_exit(void)` — fast teardown: skip destructors, do not terminate.
pub unsafe extern "win64" fn shim_c_exit() {}

/// `void _exit(int code)` — terminate WITHOUT running atexit destructors.
pub unsafe extern "win64" fn shim_fast_exit(code: i32) -> ! {
    syscalls::sys_exit(code as u64)
}

/// `void _Exit(int code)` — C99 immediate termination (distinct fn from `_exit`
/// so the whole-table distinct-address self-test holds).
pub unsafe extern "win64" fn shim_exit_c99(code: i32) -> ! {
    syscalls::sys_exit(code as u64)
}

/// `void quick_exit(int code)` — C11 quick termination (distinct fn).
pub unsafe extern "win64" fn shim_quick_exit(code: i32) -> ! {
    syscalls::sys_exit(code as u64)
}

/// `void abort(void)` — abnormal termination (exit code 3, the CRT convention).
pub unsafe extern "win64" fn shim_abort() -> ! {
    syscalls::sys_exit(3)
}

/// `void terminate(void)` — the C++ terminate handler; abnormal exit.
pub unsafe extern "win64" fn shim_terminate() -> ! {
    syscalls::sys_exit(3)
}

// CRT global mode words (`_commode`/`_fmode`). The startup reads/writes these
// through `__p__commode`/`__p__fmode`; back them with real, writable storage.
static CRT_COMMODE: core::sync::atomic::AtomicI32 = core::sync::atomic::AtomicI32::new(0);
static CRT_FMODE: core::sync::atomic::AtomicI32 = core::sync::atomic::AtomicI32::new(0);

/// `int* __p__commode(void)` — pointer to the `_commode` global.
pub unsafe extern "win64" fn shim_p_commode() -> u64 {
    CRT_COMMODE.as_ptr() as u64
}

/// `int* __p__fmode(void)` — pointer to the `_fmode` global.
pub unsafe extern "win64" fn shim_p_fmode() -> u64 {
    CRT_FMODE.as_ptr() as u64
}

/// `void _set_app_type(int at)` — records console vs GUI; we don't branch on it.
pub unsafe extern "win64" fn shim_set_app_type(_app_type: i32) {}

/// `int _configthreadlocale(int flag)` — per-thread locale control; the "C"
/// locale is always active, so return the previous state (0).
pub unsafe extern "win64" fn shim_configthreadlocale(_flag: i32) -> i32 {
    0
}

/// `int _set_fmode(int mode)` — set the default file translation mode. 0 = OK.
pub unsafe extern "win64" fn shim_set_fmode(mode: i32) -> i32 {
    CRT_FMODE.store(mode, core::sync::atomic::Ordering::Relaxed);
    0
}

/// `int _get_fmode(int* pmode)` — read the default file translation mode.
pub unsafe extern "win64" fn shim_get_fmode(pmode: u64) -> i32 {
    if pmode != 0 {
        core::ptr::write(
            pmode as *mut i32,
            CRT_FMODE.load(core::sync::atomic::Ordering::Relaxed),
        );
    }
    0
}

/// `void __setusermatherr(...)` — install a math-error callback; unused here.
pub unsafe extern "win64" fn shim_setusermatherr(_handler: u64) {}

/// `int _set_new_mode(int mode)`.
pub unsafe extern "win64" fn shim_set_new_mode(mode: i32) -> i32 {
    crate::msvcrt::_set_new_mode(mode)
}

/// `int _configure_narrow_argv(int mode)`.
pub unsafe extern "win64" fn shim_configure_narrow_argv(mode: i32) -> i32 {
    crate::msvcrt::_configure_narrow_argv(mode)
}

/// `int _initialize_narrow_environment(void)`.
pub unsafe extern "win64" fn shim_initialize_narrow_environment() -> i32 {
    crate::msvcrt::_initialize_narrow_environment()
}

/// `char** _get_initial_narrow_environment(void)`.
pub unsafe extern "win64" fn shim_get_initial_narrow_environment() -> u64 {
    crate::msvcrt::_get_initial_narrow_environment()
}

/// `int** __p___argc(void)` — pointer to the CRT `__argc` global.
pub unsafe extern "win64" fn shim_p_argc() -> u64 {
    crate::msvcrt::__p___argc() as u64
}

/// `char*** __p___argv(void)`.
pub unsafe extern "win64" fn shim_p_argv() -> u64 {
    crate::msvcrt::__p___argv()
}

/// `int _seh_filter_exe(unsigned long code, EXCEPTION_POINTERS* ep)` — the CRT's
/// top-level SEH filter. With no live exception dispatch before main on the
/// no-throw path, EXECUTE_HANDLER (1) is the safe verdict.
pub unsafe extern "win64" fn shim_seh_filter_exe(_code: u32, _ep: u64) -> i32 {
    1
}

// --- CRT heap (`malloc` family) + startup helpers still on the survey hit list. ---

/// `void* malloc(size_t size)`.
pub unsafe extern "win64" fn shim_malloc(size: u64) -> u64 {
    crate::msvcrt::malloc(size as usize)
}

/// `void free(void* ptr)`.
pub unsafe extern "win64" fn shim_free(ptr: u64) {
    crate::msvcrt::free(ptr);
}

/// `void* calloc(size_t nmemb, size_t size)`.
pub unsafe extern "win64" fn shim_calloc(nmemb: u64, size: u64) -> u64 {
    crate::msvcrt::calloc(nmemb as usize, size as usize)
}

/// `void* realloc(void* ptr, size_t size)`.
pub unsafe extern "win64" fn shim_realloc(ptr: u64, size: u64) -> u64 {
    crate::msvcrt::realloc(ptr, size as usize)
}

/// `int _vsnwprintf(wchar_t* buf, size_t count, const wchar_t* fmt, va_list va)`.
pub unsafe extern "win64" fn shim_vsnwprintf(buffer: u64, count: u64, format: u64, va: u64) -> i32 {
    let fmt = crate::msvcrt::read_format_wide(format);
    if buffer == 0 || count == 0 {
        let s = crate::msvcrt::vformat_wide(&fmt, va);
        return s.encode_utf16().count() as i32;
    }
    let buf = core::slice::from_raw_parts_mut(buffer as *mut u16, count as usize);
    crate::msvcrt::_vsnwprintf(buf, count as usize, &fmt, va)
}

/// `int __getmainargs(int* argc, char*** argv, char*** envp, int expand, ...)`.
pub unsafe extern "win64" fn shim_getmainargs(
    argc_ptr: u64,
    argv_ptr: u64,
    env_ptr: u64,
    expand: i32,
    start_info: u64,
) -> i32 {
    let mut argc = 0i32;
    let mut argv = 0u64;
    let mut envp = 0u64;
    let rc = crate::msvcrt::__getmainargs(&mut argc, &mut argv, &mut envp, expand, start_info);
    if argc_ptr != 0 {
        core::ptr::write(argc_ptr as *mut i32, argc);
    }
    if argv_ptr != 0 {
        core::ptr::write(argv_ptr as *mut u64, argv);
    }
    if env_ptr != 0 {
        core::ptr::write(env_ptr as *mut u64, envp);
    }
    rc
}

/// `int __wgetmainargs(int* argc, wchar_t*** argv, wchar_t*** envp, ...)`.
pub unsafe extern "win64" fn shim_wgetmainargs(
    argc_ptr: u64,
    argv_ptr: u64,
    env_ptr: u64,
    expand: i32,
    start_info: u64,
) -> i32 {
    let mut argc = 0i32;
    let mut argv = 0u64;
    let mut envp = 0u64;
    let rc = crate::msvcrt::__wgetmainargs(&mut argc, &mut argv, &mut envp, expand, start_info);
    if argc_ptr != 0 {
        core::ptr::write(argc_ptr as *mut i32, argc);
    }
    if argv_ptr != 0 {
        core::ptr::write(argv_ptr as *mut u64, argv);
    }
    if env_ptr != 0 {
        core::ptr::write(env_ptr as *mut u64, envp);
    }
    rc
}

/// `void _amsg_exit(int rterr)` — CRT fatal runtime error; terminate the process.
pub unsafe extern "win64" fn shim_amsg_exit(_rterr: i32) -> ! {
    syscalls::sys_exit(255)
}

/// `int _XcptFilter(unsigned long code, struct _EXCEPTION_POINTERS* ep)`.
pub unsafe extern "win64" fn shim_xcpt_filter(_code: u32, _ep: u64) -> i32 {
    1
}

/// `void _lock(int fd)` / `void _unlock(int fd)` — CRT stdio locks.
pub unsafe extern "win64" fn shim_crt_lock(_fd: i32) {
    crate::msvcrt::_lock(_fd);
}
pub unsafe extern "win64" fn shim_crt_unlock(_fd: i32) {
    crate::msvcrt::_unlock(_fd);
}

// UCRT private forwarders (`_o_*`) — distinct entry points, same semantics.
pub unsafe extern "win64" fn shim_o_exit(code: i32) -> ! {
    shim_exit(code)
}
pub unsafe extern "win64" fn shim_o__exit(code: i32) -> ! {
    shim_fast_exit(code)
}
pub unsafe extern "win64" fn shim_o_terminate() -> ! {
    shim_terminate()
}
pub unsafe extern "win64" fn shim_o__cexit() {
    shim_cexit()
}
pub unsafe extern "win64" fn shim_o_free(ptr: u64) {
    shim_free(ptr)
}
pub unsafe extern "win64" fn shim_o__crt_atexit(handler: u64) -> i32 {
    shim_crt_atexit(handler)
}
pub unsafe extern "win64" fn shim_o__initialize_onexit_table(table: u64) -> i32 {
    shim_initialize_onexit_table(table)
}
pub unsafe extern "win64" fn shim_o__register_onexit_function(table: u64, fn_ptr: u64) -> i32 {
    shim_register_onexit_function(table, fn_ptr)
}
pub unsafe extern "win64" fn shim_o__set_fmode(mode: i32) -> i32 {
    shim_set_fmode(mode)
}
pub unsafe extern "win64" fn shim_o__set_new_mode(mode: i32) -> i32 {
    shim_set_new_mode(mode)
}
pub unsafe extern "win64" fn shim_o__set_app_type(app_type: i32) {
    shim_set_app_type(app_type)
}
pub unsafe extern "win64" fn shim_o__configthreadlocale(flag: i32) -> i32 {
    shim_configthreadlocale(flag)
}
pub unsafe extern "win64" fn shim_o__seh_filter_exe(code: u32, ep: u64) -> i32 {
    shim_seh_filter_exe(code, ep)
}
pub unsafe extern "win64" fn shim_o_p_commode() -> u64 {
    shim_p_commode()
}

// --- kernel32 synch: Slim Reader/Writer locks. RTL_SRWLOCK is a single
//     pointer-sized word the guest owns; we model lock state directly in it
//     (bit0 = held exclusive, higher bits = shared reader count). Correct and
//     lock-free for the uncontended path that dominates CRT/app init; genuine
//     cross-thread *blocking* is deferred to the raebridge_server broker/futex
//     (see docs/components/raebridge-server-design.md). Mirrors the existing
//     CRITICAL_SECTION model, which likewise counts rather than blocks. ---
const SRWLOCK_EXCLUSIVE: u64 = 1;
const SRWLOCK_SHARED_UNIT: u64 = 2;

pub unsafe extern "win64" fn shim_initialize_srwlock(lock: *mut u64) {
    if !lock.is_null() {
        lock.write_unaligned(0);
    }
}

pub unsafe extern "win64" fn shim_acquire_srwlock_exclusive(lock: *mut u64) {
    if !lock.is_null() {
        lock.write_unaligned(lock.read_unaligned() | SRWLOCK_EXCLUSIVE);
    }
}

pub unsafe extern "win64" fn shim_release_srwlock_exclusive(lock: *mut u64) {
    if !lock.is_null() {
        lock.write_unaligned(lock.read_unaligned() & !SRWLOCK_EXCLUSIVE);
    }
}

pub unsafe extern "win64" fn shim_acquire_srwlock_shared(lock: *mut u64) {
    if !lock.is_null() {
        lock.write_unaligned(lock.read_unaligned().wrapping_add(SRWLOCK_SHARED_UNIT));
    }
}

pub unsafe extern "win64" fn shim_release_srwlock_shared(lock: *mut u64) {
    if !lock.is_null() {
        lock.write_unaligned(lock.read_unaligned().saturating_sub(SRWLOCK_SHARED_UNIT));
    }
}

pub unsafe extern "win64" fn shim_try_acquire_srwlock_exclusive(lock: *mut u64) -> u8 {
    if lock.is_null() {
        return 0;
    }
    if lock.read_unaligned() == 0 {
        lock.write_unaligned(SRWLOCK_EXCLUSIVE);
        1
    } else {
        0
    }
}

pub unsafe extern "win64" fn shim_try_acquire_srwlock_shared(lock: *mut u64) -> u8 {
    if lock.is_null() {
        return 0;
    }
    let v = lock.read_unaligned();
    if v & SRWLOCK_EXCLUSIVE == 0 {
        lock.write_unaligned(v.wrapping_add(SRWLOCK_SHARED_UNIT));
        1
    } else {
        0
    }
}

// --- kernel32: local heap, wide debug output, ANSI module path, heap tuning ---
const LMEM_ZEROINIT: u32 = 0x0040;

/// `HLOCAL LocalAlloc(UINT uFlags, SIZE_T uBytes)` — backed by the process heap.
pub unsafe extern "win64" fn shim_local_alloc(flags: u32, bytes: u64) -> u64 {
    with_ctx(|ctx| {
        let heap = k::get_process_heap(ctx);
        let hf = if flags & LMEM_ZEROINIT != 0 {
            k::HEAP_ZERO_MEMORY
        } else {
            0
        };
        k::heap_alloc(ctx, heap, hf, bytes)
    })
}

/// `HLOCAL LocalFree(HLOCAL hMem)` — returns NULL on success.
pub unsafe extern "win64" fn shim_local_free(mem: u64) -> u64 {
    with_ctx(|ctx| {
        let heap = k::get_process_heap(ctx);
        let _ = k::heap_free(ctx, heap, 0, mem);
    });
    0
}

/// `void OutputDebugStringW(LPCWSTR)` — mirror to the serial debug sink.
pub unsafe extern "win64" fn shim_output_debug_string_w(s: *const u16) {
    let w = wide_cstr(s);
    let mut bytes = alloc::vec::Vec::with_capacity(w.len());
    for &c in w {
        bytes.push(if c < 0x80 { c as u8 } else { b'?' });
    }
    syscalls::sys_debug_print(&bytes);
}

/// `void DebugBreak()` — no debugger is attached in the sandbox, so an INT3
/// would be an unhandled EXCEPTION_BREAKPOINT that kills the app. Report and
/// continue (matches "no debugger present"); real fault delivery is the gated
/// SEH path.
pub unsafe extern "win64" fn shim_debug_break() {
    syscalls::sys_debug_print(b"[raebridge] DebugBreak (ignored: no debugger)\n");
}

/// `BOOL HeapSetInformation(...)` — heap tuning is advisory; accept and no-op.
pub unsafe extern "win64" fn shim_heap_set_information(
    _heap: u64,
    _class: i32,
    _info: u64,
    _len: u64,
) -> i32 {
    1
}

/// `DWORD GetModuleFileNameA(HMODULE, LPSTR, DWORD)` — ANSI of the W variant.
pub unsafe extern "win64" fn shim_get_module_file_name_a(
    module: u64,
    filename: *mut u8,
    size: u32,
) -> u32 {
    if filename.is_null() || size == 0 {
        return 0;
    }
    let mut wbuf = [0u16; 260];
    let n = with_ctx(|ctx| k::get_module_file_name_w(ctx, WinHandle(module), &mut wbuf));
    let out = core::slice::from_raw_parts_mut(filename, size as usize);
    let mut written = 0usize;
    for &c in wbuf.iter().take((n as usize).min(wbuf.len())) {
        if written + 1 >= out.len() {
            break;
        }
        out[written] = if c < 0x80 { c as u8 } else { b'?' };
        written += 1;
    }
    out[written.min(out.len() - 1)] = 0;
    written as u32
}

// --- kernel32 synch: modern Ex object creators (map onto the base creators) ---
const CREATE_MUTEX_INITIAL_OWNER: u32 = 0x0000_0001;

pub unsafe extern "win64" fn shim_create_mutex_ex_w(
    attrs: u64,
    name: *const u16,
    flags: u32,
    _access: u32,
) -> u64 {
    let name = wide_cstr_opt(name);
    let owner = WinBool(if flags & CREATE_MUTEX_INITIAL_OWNER != 0 {
        1
    } else {
        0
    });
    with_ctx(|ctx| k::create_mutex_w(ctx, attrs, owner, name).0)
}

pub unsafe extern "win64" fn shim_create_semaphore_ex_w(
    attrs: u64,
    initial: i32,
    max: i32,
    name: *const u16,
    _flags: u32,
    _access: u32,
) -> u64 {
    let name = wide_cstr_opt(name);
    with_ctx(|ctx| k::create_semaphore_w(ctx, attrs, initial, max, name).0)
}

// --- advapi32 ETW provider: fire-and-forget telemetry. With no trace consumer
//     attached, real Windows succeeds and drops the events; we do the same
//     (ERROR_SUCCESS, no-op) rather than a fail-loud stub. ---

pub unsafe extern "win64" fn shim_event_register(
    _provider: u64,
    _cb: u64,
    _cb_ctx: u64,
    reg: *mut u64,
) -> u32 {
    if !reg.is_null() {
        reg.write_unaligned(0);
    }
    0
}

pub unsafe extern "win64" fn shim_event_unregister(_reg: u64) -> u32 {
    0
}

pub unsafe extern "win64" fn shim_event_write_transfer(
    _reg: u64,
    _desc: u64,
    _activity: u64,
    _related: u64,
    _count: u32,
    _data: u64,
) -> u32 {
    0
}

pub unsafe extern "win64" fn shim_event_set_information(
    _reg: u64,
    _class: i32,
    _info: u64,
    _len: u32,
) -> u32 {
    0
}

pub fn shim_table() -> Vec<ShimEntry> {
    // Each function item is coerced to its concrete fn-pointer type before
    // the integer cast (a direct fn-item → integer cast is a lint error,
    // and rightly so — the coercion makes the address-taking explicit).
    macro_rules! entry {
        ($name:literal, $f:ident: $ty:ty) => {
            ("kernel32.dll", $name, ($f as $ty) as usize as u64)
        };
    }
    macro_rules! nt_entry {
        ($name:literal, $f:ident: $ty:ty) => {
            ("ntdll.dll", $name, ($f as $ty) as usize as u64)
        };
    }
    macro_rules! adv_entry {
        ($name:literal, $f:ident: $ty:ty) => {
            ("advapi32.dll", $name, ($f as $ty) as usize as u64)
        };
    }
    macro_rules! user_entry {
        ($name:literal, $f:ident: $ty:ty) => {
            ("user32.dll", $name, ($f as $ty) as usize as u64)
        };
    }
    macro_rules! gdi_entry {
        ($name:literal, $f:ident: $ty:ty) => {
            ("gdi32.dll", $name, ($f as $ty) as usize as u64)
        };
    }
    macro_rules! comdlg_entry {
        ($name:literal, $f:ident: $ty:ty) => {
            ("comdlg32.dll", $name, ($f as $ty) as usize as u64)
        };
    }
    macro_rules! crt_entry {
        ($name:literal, $f:ident: $ty:ty) => {
            ("msvcrt.dll", $name, ($f as $ty) as usize as u64)
        };
    }
    alloc::vec![
        // --- original Phase-B top-20 (kernel32) ---
        entry!("ExitProcess", shim_exit_process: unsafe extern "win64" fn(u32) -> !),
        // --- user32: window class + creation (Phase C GUI wiring) ---
        user_entry!("RegisterClassExW", shim_register_class_ex_w: unsafe extern "win64" fn(*const u8) -> u16),
        user_entry!("CreateWindowExW", shim_create_window_ex_w: unsafe extern "win64" fn(u32, *const u16, *const u16, u32, i32, i32, i32, i32, u64, u64, u64, u64) -> u64),
        user_entry!("ShowWindow", shim_show_window: unsafe extern "win64" fn(u64, i32) -> i32),
        user_entry!("UpdateWindow", shim_update_window: unsafe extern "win64" fn(u64) -> i32),
        user_entry!("DefWindowProcW", shim_def_window_proc_w: unsafe extern "win64" fn(u64, u32, u64, i64) -> i64),
        user_entry!("PostQuitMessage", shim_post_quit_message: unsafe extern "win64" fn(i32)),
        user_entry!("GetMessageW", shim_get_message_w: unsafe extern "win64" fn(*mut u8, u64, u32, u32) -> i32),
        user_entry!("PeekMessageW", shim_peek_message_w: unsafe extern "win64" fn(*mut u8, u64, u32, u32, u32) -> i32),
        user_entry!("TranslateMessage", shim_translate_message: unsafe extern "win64" fn(*const u8) -> i32),
        user_entry!("DispatchMessageW", shim_dispatch_message_w: unsafe extern "win64" fn(*const u8) -> i64),
        user_entry!("PostMessageW", shim_post_message_w: unsafe extern "win64" fn(u64, u32, u64, i64) -> i32),
        user_entry!("GetDC", shim_get_dc: unsafe extern "win64" fn(u64) -> u64),
        user_entry!("ReleaseDC", shim_release_dc: unsafe extern "win64" fn(u64, u64) -> i32),
        user_entry!("BeginPaint", shim_begin_paint: unsafe extern "win64" fn(u64, *mut u8) -> u64),
        user_entry!("EndPaint", shim_end_paint: unsafe extern "win64" fn(u64, *const u8) -> i32),
        user_entry!("FillRect", shim_fill_rect: unsafe extern "win64" fn(u64, *const u8, u64) -> i32),
        // --- user32: menus (menu bar / popup -> WM_COMMAND) ---
        user_entry!("CreateMenu", shim_create_menu: unsafe extern "win64" fn() -> u64),
        user_entry!("CreatePopupMenu", shim_create_popup_menu: unsafe extern "win64" fn() -> u64),
        user_entry!("AppendMenuW", shim_append_menu_w: unsafe extern "win64" fn(u64, u32, u64, *const u16) -> i32),
        user_entry!("SetMenu", shim_set_menu: unsafe extern "win64" fn(u64, u64) -> i32),
        user_entry!("GetMenu", shim_get_menu: unsafe extern "win64" fn(u64) -> u64),
        user_entry!("GetMenuItemCount", shim_get_menu_item_count: unsafe extern "win64" fn(u64) -> i32),
        user_entry!("GetMenuItemID", shim_get_menu_item_id: unsafe extern "win64" fn(u64, i32) -> u32),
        user_entry!("DestroyMenu", shim_destroy_menu: unsafe extern "win64" fn(u64) -> i32),
        // --- user32: common window management / dialogs ---
        user_entry!("MessageBoxW", shim_message_box_w: unsafe extern "win64" fn(u64, *const u16, *const u16, u32) -> i32),
        user_entry!("MessageBoxA", shim_message_box_a: unsafe extern "win64" fn(u64, *const u8, *const u8, u32) -> i32),
        user_entry!("GetClientRect", shim_get_client_rect: unsafe extern "win64" fn(u64, *mut u8) -> i32),
        user_entry!("GetWindowRect", shim_get_window_rect: unsafe extern "win64" fn(u64, *mut u8) -> i32),
        user_entry!("DestroyWindow", shim_destroy_window: unsafe extern "win64" fn(u64) -> i32),
        user_entry!("MoveWindow", shim_move_window: unsafe extern "win64" fn(u64, i32, i32, i32, i32, i32) -> i32),
        user_entry!("SetWindowPos", shim_set_window_pos: unsafe extern "win64" fn(u64, u64, i32, i32, i32, i32, u32) -> i32),
        user_entry!("GetWindowLongW", shim_get_window_long_w: unsafe extern "win64" fn(u64, i32) -> i32),
        user_entry!("SetWindowLongW", shim_set_window_long_w: unsafe extern "win64" fn(u64, i32, i32) -> i32),
        user_entry!("GetWindowLongPtrW", shim_get_window_long_ptr_w: unsafe extern "win64" fn(u64, i32) -> i64),
        user_entry!("SetWindowLongPtrW", shim_set_window_long_ptr_w: unsafe extern "win64" fn(u64, i32, i64) -> i64),
        user_entry!("InvalidateRect", shim_invalidate_rect: unsafe extern "win64" fn(u64, *const u8, i32) -> i32),
        user_entry!("ScreenToClient", shim_screen_to_client: unsafe extern "win64" fn(u64, *mut u8) -> i32),
        user_entry!("ClientToScreen", shim_client_to_screen: unsafe extern "win64" fn(u64, *mut u8) -> i32),
        user_entry!("AdjustWindowRect", shim_adjust_window_rect: unsafe extern "win64" fn(*mut u8, u32, i32) -> i32),
        user_entry!("AdjustWindowRectEx", shim_adjust_window_rect_ex: unsafe extern "win64" fn(*mut u8, u32, i32, u32) -> i32),
        // --- user32: window text (caption / EDIT buffer) — notepad save path ---
        user_entry!("SetWindowTextW", shim_set_window_text_w: unsafe extern "win64" fn(u64, *const u16) -> i32),
        user_entry!("GetWindowTextW", shim_get_window_text_w: unsafe extern "win64" fn(u64, *mut u16, i32) -> i32),
        user_entry!("GetWindowTextLengthW", shim_get_window_text_length_w: unsafe extern "win64" fn(u64) -> i32),
        // --- comdlg32: File Open/Save dialogs (headless auto-confirm) ---
        comdlg_entry!("GetSaveFileNameW", shim_get_save_file_name_w: unsafe extern "win64" fn(*mut u8) -> i32),
        comdlg_entry!("GetOpenFileNameW", shim_get_open_file_name_w: unsafe extern "win64" fn(*mut u8) -> i32),
        // --- gdi32 ---
        gdi_entry!("CreateSolidBrush", shim_create_solid_brush: unsafe extern "win64" fn(u32) -> u64),
        gdi_entry!("TextOutW", shim_text_out_w: unsafe extern "win64" fn(u64, i32, i32, *const u16, i32) -> i32),
        // --- gdi32: common DC / object / drawing primitives (now IAT-wired) ---
        gdi_entry!("DeleteDC", shim_delete_dc: unsafe extern "win64" fn(u64) -> i32),
        gdi_entry!("CreateCompatibleDC", shim_create_compatible_dc: unsafe extern "win64" fn(u64) -> u64),
        gdi_entry!("SelectObject", shim_select_object: unsafe extern "win64" fn(u64, u64) -> u64),
        gdi_entry!("DeleteObject", shim_delete_object: unsafe extern "win64" fn(u64) -> i32),
        gdi_entry!("CreatePen", shim_create_pen: unsafe extern "win64" fn(i32, i32, u32) -> u64),
        gdi_entry!("GetStockObject", shim_get_stock_object: unsafe extern "win64" fn(i32) -> u64),
        gdi_entry!("GetDeviceCaps", shim_get_device_caps: unsafe extern "win64" fn(u64, i32) -> i32),
        gdi_entry!("SetBkMode", shim_set_bk_mode: unsafe extern "win64" fn(u64, i32) -> i32),
        gdi_entry!("SetTextColor", shim_set_text_color: unsafe extern "win64" fn(u64, u32) -> u32),
        gdi_entry!("SetBkColor", shim_set_bk_color: unsafe extern "win64" fn(u64, u32) -> u32),
        gdi_entry!("GetTextColor", shim_get_text_color: unsafe extern "win64" fn(u64) -> u32),
        gdi_entry!("Rectangle", shim_rectangle: unsafe extern "win64" fn(u64, i32, i32, i32, i32) -> i32),
        gdi_entry!("Ellipse", shim_ellipse: unsafe extern "win64" fn(u64, i32, i32, i32, i32) -> i32),
        gdi_entry!("LineTo", shim_line_to: unsafe extern "win64" fn(u64, i32, i32) -> i32),
        gdi_entry!("MoveToEx", shim_move_to_ex: unsafe extern "win64" fn(u64, i32, i32, *mut u8) -> i32),
        gdi_entry!("SetPixel", shim_set_pixel: unsafe extern "win64" fn(u64, i32, i32, u32) -> u32),
        gdi_entry!("GetPixel", shim_get_pixel: unsafe extern "win64" fn(u64, i32, i32) -> u32),
        gdi_entry!("CreateCompatibleBitmap", shim_create_compatible_bitmap: unsafe extern "win64" fn(u64, i32, i32) -> u64),
        gdi_entry!("SaveDC", shim_save_dc: unsafe extern "win64" fn(u64) -> i32),
        gdi_entry!("RestoreDC", shim_restore_dc: unsafe extern "win64" fn(u64, i32) -> i32),
        gdi_entry!("BitBlt", shim_bit_blt: unsafe extern "win64" fn(u64, i32, i32, i32, i32, u64, i32, i32, u32) -> i32),
        gdi_entry!("CreateFontW", shim_create_font_w: unsafe extern "win64" fn(i32, i32, i32, i32, i32, u32, u32, u32, u32, u32, u32, u32, u32, *const u16) -> u64),
        entry!("GetLastError", shim_get_last_error: unsafe extern "win64" fn() -> u32),
        entry!("SetLastError", shim_set_last_error: unsafe extern "win64" fn(u32)),
        entry!("GetCurrentProcessId", shim_get_current_process_id: unsafe extern "win64" fn() -> u32),
        entry!("GetCurrentThreadId", shim_get_current_thread_id: unsafe extern "win64" fn() -> u32),
        entry!("GetProcessHeap", shim_get_process_heap: unsafe extern "win64" fn() -> u64),
        entry!("HeapAlloc", shim_heap_alloc: unsafe extern "win64" fn(u64, u32, u64) -> u64),
        entry!("HeapFree", shim_heap_free: unsafe extern "win64" fn(u64, u32, u64) -> i32),
        entry!("VirtualAlloc", shim_virtual_alloc: unsafe extern "win64" fn(u64, u64, u32, u32) -> u64),
        entry!("VirtualFree", shim_virtual_free: unsafe extern "win64" fn(u64, u64, u32) -> i32),
        entry!("GetTickCount", shim_get_tick_count: unsafe extern "win64" fn() -> u32),
        entry!("GetTickCount64", shim_get_tick_count_64: unsafe extern "win64" fn() -> u64),
        entry!("GetSystemTimeAsFileTime", shim_get_system_time_as_file_time: unsafe extern "win64" fn(*mut u64)),
        entry!("Sleep", shim_sleep: unsafe extern "win64" fn(u32)),
        entry!("OutputDebugStringA", shim_output_debug_string_a: unsafe extern "win64" fn(*const u8)),
        entry!("GetCommandLineW", shim_get_command_line_w: unsafe extern "win64" fn() -> *const u16),
        entry!("GetStdHandle", shim_get_std_handle: unsafe extern "win64" fn(u32) -> u64),
        entry!("CloseHandle", shim_close_handle: unsafe extern "win64" fn(u64) -> i32),
        entry!("CreateFileW", shim_create_file_w: unsafe extern "win64" fn(*const u16, u32, u32, u64, u32, u32, u64) -> u64),
        entry!("ReadFile", shim_read_file: unsafe extern "win64" fn(u64, *mut u8, u32, *mut u32, u64) -> i32),
        entry!("WriteFile", shim_write_file: unsafe extern "win64" fn(u64, *const u8, u32, *mut u32, u64) -> i32),
        // --- CRT-startup module/proc resolution ---
        entry!("GetModuleHandleW", shim_get_module_handle_w: unsafe extern "win64" fn(*const u16) -> u64),
        entry!("GetModuleHandleA", shim_get_module_handle_a: unsafe extern "win64" fn(*const u8) -> u64),
        entry!("GetModuleHandleExW", shim_get_module_handle_ex_w: unsafe extern "win64" fn(u32, *const u16, *mut u64) -> i32),
        entry!("GetModuleFileNameW", shim_get_module_file_name_w: unsafe extern "win64" fn(u64, *mut u16, u32) -> u32),
        entry!("GetProcAddress", shim_get_proc_address: unsafe extern "win64" fn(u64, *const u8) -> u64),
        // --- heap ---
        entry!("HeapReAlloc", shim_heap_realloc: unsafe extern "win64" fn(u64, u32, u64, u64) -> u64),
        entry!("HeapSize", shim_heap_size: unsafe extern "win64" fn(u64, u32, u64) -> u64),
        entry!("HeapCreate", shim_heap_create: unsafe extern "win64" fn(u32, u64, u64) -> u64),
        entry!("HeapDestroy", shim_heap_destroy: unsafe extern "win64" fn(u64) -> i32),
        // --- TLS ---
        entry!("TlsAlloc", shim_tls_alloc: unsafe extern "win64" fn() -> u32),
        entry!("TlsFree", shim_tls_free: unsafe extern "win64" fn(u32) -> i32),
        entry!("TlsGetValue", shim_tls_get_value: unsafe extern "win64" fn(u32) -> u64),
        entry!("TlsSetValue", shim_tls_set_value: unsafe extern "win64" fn(u32, u64) -> i32),
        // --- env / startup ---
        entry!("GetCommandLineA", shim_get_command_line_a: unsafe extern "win64" fn() -> *const u8),
        entry!("GetEnvironmentStringsW", shim_get_environment_strings_w: unsafe extern "win64" fn() -> *const u16),
        entry!("FreeEnvironmentStringsW", shim_free_environment_strings_w: unsafe extern "win64" fn(*const u16) -> i32),
        entry!("GetStartupInfoW", shim_get_startup_info_w: unsafe extern "win64" fn(*mut k::StartupInfoW)),
        entry!("GetCurrentProcess", shim_get_current_process: unsafe extern "win64" fn() -> u64),
        entry!("GetCurrentThread", shim_get_current_thread: unsafe extern "win64" fn() -> u64),
        // --- console ---
        entry!("WriteConsoleW", shim_write_console_w: unsafe extern "win64" fn(u64, *const u16, u32, *mut u32, u64) -> i32),
        entry!("WriteConsoleA", shim_write_console_a: unsafe extern "win64" fn(u64, *const u8, u32, *mut u32, u64) -> i32),
        entry!("GetConsoleMode", shim_get_console_mode: unsafe extern "win64" fn(u64, *mut u32) -> i32),
        entry!("SetConsoleMode", shim_set_console_mode: unsafe extern "win64" fn(u64, u32) -> i32),
        entry!("GetConsoleOutputCP", shim_get_console_output_cp: unsafe extern "win64" fn() -> u32),
        entry!("GetConsoleCP", shim_get_console_cp: unsafe extern "win64" fn() -> u32),
        entry!("GetACP", shim_get_acp: unsafe extern "win64" fn() -> u32),
        entry!("GetOEMCP", shim_get_oemcp: unsafe extern "win64" fn() -> u32),
        entry!("GetCPInfo", shim_get_cp_info: unsafe extern "win64" fn(u32, *mut u8) -> i32),
        // --- feature / debugger / timing ---
        entry!("IsProcessorFeaturePresent", shim_is_processor_feature_present: unsafe extern "win64" fn(u32) -> i32),
        entry!("IsDebuggerPresent", shim_is_debugger_present: unsafe extern "win64" fn() -> i32),
        entry!("QueryPerformanceCounter", shim_query_performance_counter: unsafe extern "win64" fn(*mut i64) -> i32),
        entry!("QueryPerformanceFrequency", shim_query_performance_frequency: unsafe extern "win64" fn(*mut i64) -> i32),
        // --- critical sections / init-once (CRT init locks) ---
        entry!("InitializeCriticalSection", shim_initialize_critical_section: unsafe extern "win64" fn(*mut u64)),
        entry!("InitializeCriticalSectionEx", shim_initialize_critical_section_ex: unsafe extern "win64" fn(*mut u64, u32, u32) -> i32),
        entry!("InitializeCriticalSectionAndSpinCount", shim_initialize_critical_section_and_spin_count: unsafe extern "win64" fn(*mut u64, u32) -> i32),
        entry!("EnterCriticalSection", shim_enter_critical_section: unsafe extern "win64" fn(*mut u64)),
        entry!("LeaveCriticalSection", shim_leave_critical_section: unsafe extern "win64" fn(*mut u64)),
        entry!("TryEnterCriticalSection", shim_try_enter_critical_section: unsafe extern "win64" fn(*mut u64) -> i32),
        entry!("DeleteCriticalSection", shim_delete_critical_section: unsafe extern "win64" fn(*mut u64)),
        entry!("InitOnceExecuteOnce", shim_init_once_execute_once: unsafe extern "win64" fn(*mut u64, u64, u64, *mut u64) -> i32),
        // --- MSVC /MT CRT-startup: pointer / SList ---
        entry!("InitializeSListHead", shim_initialize_slist_head: unsafe extern "win64" fn(*mut u8)),
        entry!("EncodePointer", shim_encode_pointer: unsafe extern "win64" fn(u64) -> u64),
        entry!("DecodePointer", shim_decode_pointer: unsafe extern "win64" fn(u64) -> u64),
        // --- MSVC /MT CRT-startup: Fiber-Local Storage ---
        entry!("FlsAlloc", shim_fls_alloc: unsafe extern "win64" fn(u64) -> u32),
        entry!("FlsFree", shim_fls_free: unsafe extern "win64" fn(u32) -> i32),
        entry!("FlsGetValue", shim_fls_get_value: unsafe extern "win64" fn(u32) -> u64),
        entry!("FlsSetValue", shim_fls_set_value: unsafe extern "win64" fn(u32, u64) -> i32),
        // --- MSVC /MT CRT-startup: code-page / locale ---
        entry!("IsValidCodePage", shim_is_valid_code_page: unsafe extern "win64" fn(u32) -> i32),
        entry!("MultiByteToWideChar", shim_multibyte_to_widechar: unsafe extern "win64" fn(u32, u32, *const u8, i32, *mut u16, i32) -> i32),
        entry!("WideCharToMultiByte", shim_widechar_to_multibyte: unsafe extern "win64" fn(u32, u32, *const u16, i32, *mut u8, i32, *const u8, *mut i32) -> i32),
        entry!("GetStringTypeW", shim_get_string_type_w: unsafe extern "win64" fn(u32, *const u16, i32, *mut u16) -> i32),
        entry!("LCMapStringW", shim_lcmap_string_w: unsafe extern "win64" fn(u32, u32, *const u16, i32, *mut u16, i32) -> i32),
        entry!("CompareStringW", shim_compare_string_w: unsafe extern "win64" fn(u32, u32, *const u16, i32, *const u16, i32) -> i32),
        // --- MSVC /MT CRT-startup: file / module / env / handle ---
        entry!("GetFileType", shim_get_file_type: unsafe extern "win64" fn(u64) -> u32),
        entry!("GetFileSizeEx", shim_get_file_size_ex: unsafe extern "win64" fn(u64, *mut i64) -> i32),
        entry!("SetStdHandle", shim_set_std_handle: unsafe extern "win64" fn(u32, u64) -> i32),
        entry!("FlushFileBuffers", shim_flush_file_buffers: unsafe extern "win64" fn(u64) -> i32),
        entry!("SetFilePointerEx", shim_set_file_pointer_ex: unsafe extern "win64" fn(u64, i64, *mut i64, u32) -> i32),
        entry!("SetEnvironmentVariableW", shim_set_environment_variable_w: unsafe extern "win64" fn(*const u16, *const u16) -> i32),
        entry!("LoadLibraryExW", shim_load_library_ex_w: unsafe extern "win64" fn(*const u16, u64, u32) -> u64),
        entry!("FreeLibrary", shim_free_library: unsafe extern "win64" fn(u64) -> i32),
        entry!("TerminateProcess", shim_terminate_process: unsafe extern "win64" fn(u64, u32) -> !),
        entry!("FindFirstFileExW", shim_find_first_file_ex_w: unsafe extern "win64" fn(*const u16, u32, *mut u8, u32, u64, u32) -> u64),
        entry!("FindNextFileW", shim_find_next_file_w: unsafe extern "win64" fn(u64, *mut u8) -> i32),
        entry!("FindClose", shim_find_close: unsafe extern "win64" fn(u64) -> i32),
        // --- MSVC /MT CRT-startup: SEH-filter machinery (kernel32 forwarders) ---
        entry!("RtlCaptureContext", shim_rtl_capture_context: unsafe extern "win64" fn(*mut u8)),
        entry!("RtlLookupFunctionEntry", shim_rtl_lookup_function_entry: unsafe extern "win64" fn(u64, *mut u64, u64) -> u64),
        entry!("RtlVirtualUnwind", shim_rtl_virtual_unwind: unsafe extern "win64" fn(u32, u64, u64, u64, *mut u8, *mut u64, *mut u64, u64) -> u64),
        entry!("RtlUnwindEx", shim_rtl_unwind_ex: unsafe extern "win64" fn(u64, u64, u64, u64, *mut u8, u64) -> !),
        entry!("RtlPcToFileHeader", shim_rtl_pc_to_file_header: unsafe extern "win64" fn(u64, *mut u64) -> u64),
        entry!("RaiseException", shim_raise_exception: unsafe extern "win64" fn(u32, u32, u32, u64) -> !),
        entry!("UnhandledExceptionFilter", shim_unhandled_exception_filter: unsafe extern "win64" fn(u64) -> i32),
        entry!("SetUnhandledExceptionFilter", shim_set_unhandled_exception_filter: unsafe extern "win64" fn(u64) -> u64),
        // (The MSVC /MT CRT imports the Rtl* SEH functions from kernel32 — see
        //  fixtures/real_msvc_mt_hello.dumpbin.txt — so no ntdll aliases are
        //  needed here; aliasing the same fn-pointer across DLLs would also trip
        //  the table's distinct-address self-test.)
        // --- synchronization objects (mutex / event / semaphore) ---
        entry!("CreateMutexW", shim_create_mutex_w: unsafe extern "win64" fn(u64, i32, *const u16) -> u64),
        entry!("CreateMutexA", shim_create_mutex_a: unsafe extern "win64" fn(u64, i32, *const u8) -> u64),
        entry!("OpenMutexW", shim_open_mutex_w: unsafe extern "win64" fn(u32, i32, *const u16) -> u64),
        entry!("ReleaseMutex", shim_release_mutex: unsafe extern "win64" fn(u64) -> i32),
        entry!("CreateEventW", shim_create_event_w: unsafe extern "win64" fn(u64, i32, i32, *const u16) -> u64),
        entry!("CreateEventA", shim_create_event_a: unsafe extern "win64" fn(u64, i32, i32, *const u8) -> u64),
        entry!("OpenEventW", shim_open_event_w: unsafe extern "win64" fn(u32, i32, *const u16) -> u64),
        entry!("SetEvent", shim_set_event: unsafe extern "win64" fn(u64) -> i32),
        entry!("ResetEvent", shim_reset_event: unsafe extern "win64" fn(u64) -> i32),
        entry!("PulseEvent", shim_pulse_event: unsafe extern "win64" fn(u64) -> i32),
        entry!("CreateSemaphoreW", shim_create_semaphore_w: unsafe extern "win64" fn(u64, i32, i32, *const u16) -> u64),
        entry!("CreateSemaphoreA", shim_create_semaphore_a: unsafe extern "win64" fn(u64, i32, i32, *const u8) -> u64),
        entry!("OpenSemaphoreW", shim_open_semaphore_w: unsafe extern "win64" fn(u32, i32, *const u16) -> u64),
        entry!("ReleaseSemaphore", shim_release_semaphore: unsafe extern "win64" fn(u64, i32, *mut i32) -> i32),
        entry!("WaitForSingleObject", shim_wait_for_single_object: unsafe extern "win64" fn(u64, u32) -> u32),
        entry!("WaitForSingleObjectEx", shim_wait_for_single_object_ex: unsafe extern "win64" fn(u64, u32, i32) -> u32),
        entry!("WaitForMultipleObjects", shim_wait_for_multiple_objects: unsafe extern "win64" fn(u32, *const u64, i32, u32) -> u32),
        // --- advapi32 registry (backed by the real versioned-config hive) ---
        adv_entry!("RegOpenKeyExW", shim_reg_open_key_ex_w: unsafe extern "win64" fn(u64, *const u16, u32, u32, *mut u64) -> i32),
        adv_entry!("RegOpenKeyExA", shim_reg_open_key_ex_a: unsafe extern "win64" fn(u64, *const u8, u32, u32, *mut u64) -> i32),
        adv_entry!("RegCloseKey", shim_reg_close_key: unsafe extern "win64" fn(u64) -> i32),
        adv_entry!("RegCreateKeyExW", shim_reg_create_key_ex_w: unsafe extern "win64" fn(u64, *const u16, u32, *const u16, u32, u32, u64, *mut u64, *mut u32) -> i32),
        adv_entry!("RegCreateKeyExA", shim_reg_create_key_ex_a: unsafe extern "win64" fn(u64, *const u8, u32, *const u8, u32, u32, u64, *mut u64, *mut u32) -> i32),
        adv_entry!("RegQueryValueExW", shim_reg_query_value_ex_w: unsafe extern "win64" fn(u64, *const u16, *mut u32, *mut u32, *mut u8, *mut u32) -> i32),
        adv_entry!("RegQueryValueExA", shim_reg_query_value_ex_a: unsafe extern "win64" fn(u64, *const u8, *mut u32, *mut u32, *mut u8, *mut u32) -> i32),
        adv_entry!("RegSetValueExW", shim_reg_set_value_ex_w: unsafe extern "win64" fn(u64, *const u16, u32, u32, *const u8, u32) -> i32),
        adv_entry!("RegSetValueExA", shim_reg_set_value_ex_a: unsafe extern "win64" fn(u64, *const u8, u32, u32, *const u8, u32) -> i32),
        adv_entry!("RegDeleteKeyW", shim_reg_delete_key_w: unsafe extern "win64" fn(u64, *const u16) -> i32),
        adv_entry!("RegDeleteValueW", shim_reg_delete_value_w: unsafe extern "win64" fn(u64, *const u16) -> i32),
        adv_entry!("RegEnumKeyExW", shim_reg_enum_key_ex_w: unsafe extern "win64" fn(u64, u32, *mut u16, *mut u32, *mut u32, *mut u16, *mut u32, *mut u8) -> i32),
        adv_entry!("RegEnumValueW", shim_reg_enum_value_w: unsafe extern "win64" fn(u64, u32, *mut u16, *mut u32, *mut u32, *mut u32, *mut u8, *mut u32) -> i32),
        adv_entry!("RegQueryInfoKeyW", shim_reg_query_info_key_w: unsafe extern "win64" fn(u64, *mut u16, *mut u32, *mut u32, *mut u32, *mut u32, *mut u32, *mut u32, *mut u32, *mut u32, *mut u32, *mut u8) -> i32),
        adv_entry!("RegFlushKey", shim_reg_flush_key: unsafe extern "win64" fn(u64) -> i32),
        // --- ntdll heap/error aliases ---
        nt_entry!("RtlAllocateHeap", shim_rtl_allocate_heap: unsafe extern "win64" fn(u64, u32, u64) -> u64),
        nt_entry!("RtlFreeHeap", shim_rtl_free_heap: unsafe extern "win64" fn(u64, u32, u64) -> i32),
        nt_entry!("RtlGetLastWin32Error", shim_rtl_get_last_win32_error: unsafe extern "win64" fn() -> u32),
        nt_entry!("RtlSetLastWin32Error", shim_rtl_set_last_win32_error: unsafe extern "win64" fn(u32)),
        // --- 2026-06-30 coverage batch: highest-frequency still-missing imports ---
        // msvcrt pure mem/str intrinsics (dynamic /MD CRT)
        crt_entry!("memset", shim_crt_memset: unsafe extern "win64" fn(u64, i32, u64) -> u64),
        crt_entry!("memcpy", shim_crt_memcpy: unsafe extern "win64" fn(u64, u64, u64) -> u64),
        crt_entry!("memmove", shim_crt_memmove: unsafe extern "win64" fn(u64, u64, u64) -> u64),
        crt_entry!("memcmp", shim_crt_memcmp: unsafe extern "win64" fn(u64, u64, u64) -> i32),
        crt_entry!("wcschr", shim_crt_wcschr: unsafe extern "win64" fn(u64, u32) -> u64),
        crt_entry!("_wcsicmp", shim_crt_wcsicmp: unsafe extern "win64" fn(u64, u64) -> i32),
        // msvcrt/ucrtbase stdio: the real va_list-consuming formatted-output family
        crt_entry!("__stdio_common_vsprintf", shim_stdio_common_vsprintf: unsafe extern "win64" fn(u64, u64, u64, u64, u64, u64) -> i32),
        crt_entry!("__stdio_common_vswprintf", shim_stdio_common_vswprintf: unsafe extern "win64" fn(u64, u64, u64, u64, u64, u64) -> i32),
        crt_entry!("__stdio_common_vsnprintf_s", shim_stdio_common_vsnprintf_s: unsafe extern "win64" fn(u64, u64, u64, u64, u64, u64, u64) -> i32),
        crt_entry!("__stdio_common_vfprintf", shim_stdio_common_vfprintf: unsafe extern "win64" fn(u64, u64, u64, u64, u64) -> i32),
        crt_entry!("__acrt_iob_func", shim_acrt_iob_func: unsafe extern "win64" fn(u32) -> u64),
        crt_entry!("sprintf", shim_sprintf: unsafe extern "win64" fn(u64, u64, u64, u64, u64, u64, u64, u64, u64, u64, u64, u64, u64, u64) -> i32),
        crt_entry!("snprintf", shim_snprintf: unsafe extern "win64" fn(u64, u64, u64, u64, u64, u64, u64, u64, u64, u64, u64, u64, u64, u64, u64) -> i32),
        crt_entry!("_snprintf", shim_legacy_snprintf: unsafe extern "win64" fn(u64, u64, u64, u64, u64, u64, u64, u64, u64, u64, u64, u64, u64, u64, u64) -> i32),
        crt_entry!("swprintf", shim_swprintf: unsafe extern "win64" fn(u64, u64, u64, u64, u64, u64, u64, u64, u64, u64, u64, u64, u64, u64, u64) -> i32),
        crt_entry!("printf", shim_printf: unsafe extern "win64" fn(u64, u64, u64, u64, u64, u64, u64, u64, u64, u64, u64, u64, u64) -> i32),
        crt_entry!("vprintf", shim_vprintf: unsafe extern "win64" fn(u64, u64) -> i32),
        crt_entry!("fputs", shim_fputs: unsafe extern "win64" fn(u64, u64) -> i32),
        crt_entry!("puts", shim_puts: unsafe extern "win64" fn(u64) -> i32),
        crt_entry!("fputc", shim_fputc: unsafe extern "win64" fn(i32, u64) -> i32),
        crt_entry!("fwrite", shim_fwrite: unsafe extern "win64" fn(u64, u64, u64, u64) -> u64),
        // msvcrt C string / char-class / conversion / math library
        crt_entry!("strlen", shim_strlen: unsafe extern "win64" fn(u64) -> u64),
        crt_entry!("strnlen", shim_strnlen: unsafe extern "win64" fn(u64, u64) -> u64),
        crt_entry!("strcmp", shim_strcmp: unsafe extern "win64" fn(u64, u64) -> i32),
        crt_entry!("strncmp", shim_strncmp: unsafe extern "win64" fn(u64, u64, u64) -> i32),
        crt_entry!("_stricmp", shim_stricmp: unsafe extern "win64" fn(u64, u64) -> i32),
        crt_entry!("_strcmpi", shim_strcmpi: unsafe extern "win64" fn(u64, u64) -> i32),
        crt_entry!("_strnicmp", shim_strnicmp: unsafe extern "win64" fn(u64, u64, u64) -> i32),
        crt_entry!("strcpy", shim_strcpy: unsafe extern "win64" fn(u64, u64) -> u64),
        crt_entry!("strncpy", shim_strncpy: unsafe extern "win64" fn(u64, u64, u64) -> u64),
        crt_entry!("strcat", shim_strcat: unsafe extern "win64" fn(u64, u64) -> u64),
        crt_entry!("strncat", shim_strncat: unsafe extern "win64" fn(u64, u64, u64) -> u64),
        crt_entry!("strchr", shim_strchr: unsafe extern "win64" fn(u64, i32) -> u64),
        crt_entry!("strrchr", shim_strrchr: unsafe extern "win64" fn(u64, i32) -> u64),
        crt_entry!("strstr", shim_strstr: unsafe extern "win64" fn(u64, u64) -> u64),
        crt_entry!("memchr", shim_memchr: unsafe extern "win64" fn(u64, i32, u64) -> u64),
        crt_entry!("wcslen", shim_wcslen: unsafe extern "win64" fn(u64) -> u64),
        crt_entry!("wcscmp", shim_wcscmp: unsafe extern "win64" fn(u64, u64) -> i32),
        crt_entry!("wcsncmp", shim_wcsncmp: unsafe extern "win64" fn(u64, u64, u64) -> i32),
        crt_entry!("wcscpy", shim_wcscpy: unsafe extern "win64" fn(u64, u64) -> u64),
        crt_entry!("wcscat", shim_wcscat: unsafe extern "win64" fn(u64, u64) -> u64),
        crt_entry!("toupper", shim_toupper: unsafe extern "win64" fn(i32) -> i32),
        crt_entry!("tolower", shim_tolower: unsafe extern "win64" fn(i32) -> i32),
        crt_entry!("isdigit", shim_isdigit: unsafe extern "win64" fn(i32) -> i32),
        crt_entry!("isalpha", shim_isalpha: unsafe extern "win64" fn(i32) -> i32),
        crt_entry!("isalnum", shim_isalnum: unsafe extern "win64" fn(i32) -> i32),
        crt_entry!("isspace", shim_isspace: unsafe extern "win64" fn(i32) -> i32),
        crt_entry!("isupper", shim_isupper: unsafe extern "win64" fn(i32) -> i32),
        crt_entry!("islower", shim_islower: unsafe extern "win64" fn(i32) -> i32),
        crt_entry!("isxdigit", shim_isxdigit: unsafe extern "win64" fn(i32) -> i32),
        crt_entry!("isprint", shim_isprint: unsafe extern "win64" fn(i32) -> i32),
        crt_entry!("iscntrl", shim_iscntrl: unsafe extern "win64" fn(i32) -> i32),
        crt_entry!("ispunct", shim_ispunct: unsafe extern "win64" fn(i32) -> i32),
        crt_entry!("atoi", shim_atoi: unsafe extern "win64" fn(u64) -> i32),
        crt_entry!("atol", shim_atol: unsafe extern "win64" fn(u64) -> i32),
        crt_entry!("_atoi64", shim_atoi64: unsafe extern "win64" fn(u64) -> i64),
        crt_entry!("atof", shim_atof: unsafe extern "win64" fn(u64) -> f64),
        crt_entry!("strtol", shim_strtol: unsafe extern "win64" fn(u64, u64, i32) -> i32),
        crt_entry!("strtoul", shim_strtoul: unsafe extern "win64" fn(u64, u64, i32) -> u32),
        crt_entry!("_strtoi64", shim_strtoi64: unsafe extern "win64" fn(u64, u64, i32) -> i64),
        crt_entry!("strtod", shim_strtod: unsafe extern "win64" fn(u64, u64) -> f64),
        crt_entry!("sin", shim_sin: unsafe extern "win64" fn(f64) -> f64),
        crt_entry!("cos", shim_cos: unsafe extern "win64" fn(f64) -> f64),
        crt_entry!("tan", shim_tan: unsafe extern "win64" fn(f64) -> f64),
        crt_entry!("atan2", shim_atan2: unsafe extern "win64" fn(f64, f64) -> f64),
        crt_entry!("exp", shim_exp: unsafe extern "win64" fn(f64) -> f64),
        crt_entry!("log", shim_log: unsafe extern "win64" fn(f64) -> f64),
        crt_entry!("log10", shim_log10: unsafe extern "win64" fn(f64) -> f64),
        crt_entry!("pow", shim_pow: unsafe extern "win64" fn(f64, f64) -> f64),
        crt_entry!("sqrt", shim_sqrt: unsafe extern "win64" fn(f64) -> f64),
        crt_entry!("ceil", shim_ceil: unsafe extern "win64" fn(f64) -> f64),
        crt_entry!("floor", shim_floor: unsafe extern "win64" fn(f64) -> f64),
        crt_entry!("fabs", shim_fabs: unsafe extern "win64" fn(f64) -> f64),
        crt_entry!("fmod", shim_fmod: unsafe extern "win64" fn(f64, f64) -> f64),
        // msvcrt/ucrtbase startup + teardown (the __scrt_common_main sequence)
        crt_entry!("_initterm", shim_initterm: unsafe extern "win64" fn(u64, u64)),
        crt_entry!("_initterm_e", shim_initterm_e: unsafe extern "win64" fn(u64, u64) -> i32),
        crt_entry!("atexit", shim_atexit: unsafe extern "win64" fn(u64) -> i32),
        crt_entry!("_onexit", shim_onexit: unsafe extern "win64" fn(u64) -> u64),
        crt_entry!("_crt_atexit", shim_crt_atexit: unsafe extern "win64" fn(u64) -> i32),
        crt_entry!("_register_onexit_function", shim_register_onexit_function: unsafe extern "win64" fn(u64, u64) -> i32),
        crt_entry!("_initialize_onexit_table", shim_initialize_onexit_table: unsafe extern "win64" fn(u64) -> i32),
        crt_entry!("_execute_onexit_table", shim_execute_onexit_table: unsafe extern "win64" fn(u64) -> i32),
        crt_entry!("_register_thread_local_exe_atexit_callback", shim_register_thread_local_exe_atexit_callback: unsafe extern "win64" fn(u64) -> i32),
        crt_entry!("exit", shim_exit: unsafe extern "win64" fn(i32) -> !),
        crt_entry!("_cexit", shim_cexit: unsafe extern "win64" fn()),
        crt_entry!("_c_exit", shim_c_exit: unsafe extern "win64" fn()),
        crt_entry!("_exit", shim_fast_exit: unsafe extern "win64" fn(i32) -> !),
        crt_entry!("_Exit", shim_exit_c99: unsafe extern "win64" fn(i32) -> !),
        crt_entry!("quick_exit", shim_quick_exit: unsafe extern "win64" fn(i32) -> !),
        crt_entry!("abort", shim_abort: unsafe extern "win64" fn() -> !),
        crt_entry!("terminate", shim_terminate: unsafe extern "win64" fn() -> !),
        crt_entry!("__p__commode", shim_p_commode: unsafe extern "win64" fn() -> u64),
        crt_entry!("__p__fmode", shim_p_fmode: unsafe extern "win64" fn() -> u64),
        crt_entry!("_set_app_type", shim_set_app_type: unsafe extern "win64" fn(i32)),
        crt_entry!("_configthreadlocale", shim_configthreadlocale: unsafe extern "win64" fn(i32) -> i32),
        crt_entry!("_set_fmode", shim_set_fmode: unsafe extern "win64" fn(i32) -> i32),
        crt_entry!("_get_fmode", shim_get_fmode: unsafe extern "win64" fn(u64) -> i32),
        crt_entry!("__setusermatherr", shim_setusermatherr: unsafe extern "win64" fn(u64)),
        crt_entry!("_set_new_mode", shim_set_new_mode: unsafe extern "win64" fn(i32) -> i32),
        crt_entry!("_configure_narrow_argv", shim_configure_narrow_argv: unsafe extern "win64" fn(i32) -> i32),
        crt_entry!("_initialize_narrow_environment", shim_initialize_narrow_environment: unsafe extern "win64" fn() -> i32),
        crt_entry!("_get_initial_narrow_environment", shim_get_initial_narrow_environment: unsafe extern "win64" fn() -> u64),
        crt_entry!("__p___argc", shim_p_argc: unsafe extern "win64" fn() -> u64),
        crt_entry!("__p___argv", shim_p_argv: unsafe extern "win64" fn() -> u64),
        crt_entry!("_seh_filter_exe", shim_seh_filter_exe: unsafe extern "win64" fn(u32, u64) -> i32),
        crt_entry!("malloc", shim_malloc: unsafe extern "win64" fn(u64) -> u64),
        crt_entry!("free", shim_free: unsafe extern "win64" fn(u64)),
        crt_entry!("calloc", shim_calloc: unsafe extern "win64" fn(u64, u64) -> u64),
        crt_entry!("realloc", shim_realloc: unsafe extern "win64" fn(u64, u64) -> u64),
        crt_entry!("_vsnwprintf", shim_vsnwprintf: unsafe extern "win64" fn(u64, u64, u64, u64) -> i32),
        crt_entry!("__getmainargs", shim_getmainargs: unsafe extern "win64" fn(u64, u64, u64, i32, u64) -> i32),
        crt_entry!("__wgetmainargs", shim_wgetmainargs: unsafe extern "win64" fn(u64, u64, u64, i32, u64) -> i32),
        crt_entry!("_amsg_exit", shim_amsg_exit: unsafe extern "win64" fn(i32) -> !),
        crt_entry!("_XcptFilter", shim_xcpt_filter: unsafe extern "win64" fn(u32, u64) -> i32),
        crt_entry!("_lock", shim_crt_lock: unsafe extern "win64" fn(i32)),
        crt_entry!("_unlock", shim_crt_unlock: unsafe extern "win64" fn(i32)),
        crt_entry!("_o_exit", shim_o_exit: unsafe extern "win64" fn(i32) -> !),
        crt_entry!("_o__exit", shim_o__exit: unsafe extern "win64" fn(i32) -> !),
        crt_entry!("_o_terminate", shim_o_terminate: unsafe extern "win64" fn() -> !),
        crt_entry!("_o__cexit", shim_o__cexit: unsafe extern "win64" fn()),
        crt_entry!("_o_free", shim_o_free: unsafe extern "win64" fn(u64)),
        crt_entry!("_o__crt_atexit", shim_o__crt_atexit: unsafe extern "win64" fn(u64) -> i32),
        crt_entry!("_o__initialize_onexit_table", shim_o__initialize_onexit_table: unsafe extern "win64" fn(u64) -> i32),
        crt_entry!("_o__register_onexit_function", shim_o__register_onexit_function: unsafe extern "win64" fn(u64, u64) -> i32),
        crt_entry!("_o__set_fmode", shim_o__set_fmode: unsafe extern "win64" fn(i32) -> i32),
        crt_entry!("_o__set_new_mode", shim_o__set_new_mode: unsafe extern "win64" fn(i32) -> i32),
        crt_entry!("_o__set_app_type", shim_o__set_app_type: unsafe extern "win64" fn(i32)),
        crt_entry!("_o__configthreadlocale", shim_o__configthreadlocale: unsafe extern "win64" fn(i32) -> i32),
        crt_entry!("_o__seh_filter_exe", shim_o__seh_filter_exe: unsafe extern "win64" fn(u32, u64) -> i32),
        crt_entry!("_o___p__commode", shim_o_p_commode: unsafe extern "win64" fn() -> u64),
        // kernel32 Slim Reader/Writer locks (in-process, uncontended-exact)
        entry!("InitializeSRWLock", shim_initialize_srwlock: unsafe extern "win64" fn(*mut u64)),
        entry!("AcquireSRWLockExclusive", shim_acquire_srwlock_exclusive: unsafe extern "win64" fn(*mut u64)),
        entry!("ReleaseSRWLockExclusive", shim_release_srwlock_exclusive: unsafe extern "win64" fn(*mut u64)),
        entry!("AcquireSRWLockShared", shim_acquire_srwlock_shared: unsafe extern "win64" fn(*mut u64)),
        entry!("ReleaseSRWLockShared", shim_release_srwlock_shared: unsafe extern "win64" fn(*mut u64)),
        entry!("TryAcquireSRWLockExclusive", shim_try_acquire_srwlock_exclusive: unsafe extern "win64" fn(*mut u64) -> u8),
        entry!("TryAcquireSRWLockShared", shim_try_acquire_srwlock_shared: unsafe extern "win64" fn(*mut u64) -> u8),
        // kernel32 local heap / debug / module path / heap tuning
        entry!("LocalAlloc", shim_local_alloc: unsafe extern "win64" fn(u32, u64) -> u64),
        entry!("LocalFree", shim_local_free: unsafe extern "win64" fn(u64) -> u64),
        entry!("OutputDebugStringW", shim_output_debug_string_w: unsafe extern "win64" fn(*const u16)),
        entry!("DebugBreak", shim_debug_break: unsafe extern "win64" fn()),
        entry!("HeapSetInformation", shim_heap_set_information: unsafe extern "win64" fn(u64, i32, u64, u64) -> i32),
        entry!("GetModuleFileNameA", shim_get_module_file_name_a: unsafe extern "win64" fn(u64, *mut u8, u32) -> u32),
        // kernel32 modern Ex synch creators
        entry!("CreateMutexExW", shim_create_mutex_ex_w: unsafe extern "win64" fn(u64, *const u16, u32, u32) -> u64),
        entry!("CreateSemaphoreExW", shim_create_semaphore_ex_w: unsafe extern "win64" fn(u64, i32, i32, *const u16, u32, u32) -> u64),
        // advapi32 ETW provider (no active trace session -> success no-op)
        adv_entry!("EventRegister", shim_event_register: unsafe extern "win64" fn(u64, u64, u64, *mut u64) -> u32),
        adv_entry!("EventUnregister", shim_event_unregister: unsafe extern "win64" fn(u64) -> u32),
        adv_entry!("EventWriteTransfer", shim_event_write_transfer: unsafe extern "win64" fn(u64, u64, u64, u64, u32, u64) -> u32),
        adv_entry!("EventSetInformation", shim_event_set_information: unsafe extern "win64" fn(u64, i32, u64, u32) -> u32),
    ]
}

/// Look up a single shim address by (dll, name). The DLL is first canonicalized
/// through the API Set schema ([`crate::apiset`]) so contract DLLs
/// (`api-ms-win-*`) and host aliases (`kernelbase`/`ucrtbase`/…) resolve to the
/// module that actually implements the export — the same redirection real
/// Windows performs. Name match is exact (imports name a specific export).
pub fn resolve_shim(dll: &str, name: &str) -> Option<u64> {
    let dll_lc = crate::apiset::canonical_dll(dll);
    shim_table()
        .into_iter()
        .find(|(d, n, _)| *d == dll_lc.as_str() && *n == name)
        .map(|(_, _, addr)| addr)
}

/// A build-once, sorted index over the shim table for fast *repeated* lookups
/// during import resolution.
///
/// Speed/efficiency (Concept §Gaming-First: the compat layer must be invisible,
/// which means fast): [`resolve_shim`] rebuilds the entire multi-thousand-entry
/// `shim_table()` — a fresh `Vec` allocation plus every fn-pointer cast — on
/// *every* call. The PE import loop calls it once per imported symbol, so a
/// binary importing N names pays O(N x table) work and N full allocations just
/// to bind its IAT. This resolver builds the table **once**, sorts it, and
/// answers each import with a branch-predictable O(log table) binary search and
/// zero per-lookup allocation — the hashed-symbol-table efficiency Wine gets
/// from the dynamic linker, done natively. Use it whenever resolving more than
/// a couple of names (i.e. the loader); `resolve_shim` remains for one-shot
/// lookups and tests.
pub struct ShimResolver {
    /// The shim table, sorted by (dll, name) for binary search.
    table: Vec<ShimEntry>,
}

impl ShimResolver {
    /// Build the sorted index once. Cost is paid a single time per image load.
    pub fn new() -> Self {
        let mut table = shim_table();
        table.sort_unstable_by(|a, b| (a.0, a.1).cmp(&(b.0, b.1)));
        Self { table }
    }

    /// Number of indexed shims (equals `shim_table().len()`).
    pub fn len(&self) -> usize {
        self.table.len()
    }

    /// True if the index is empty (never, in practice — kept for lint parity).
    pub fn is_empty(&self) -> bool {
        self.table.is_empty()
    }

    /// Resolve `(dll, name)` to a shim address. The DLL is canonicalized through
    /// the API Set schema first (same redirection as [`resolve_shim`]), then the
    /// prebuilt table is binary-searched. Returns `None` for an unbound import.
    pub fn resolve(&self, dll: &str, name: &str) -> Option<u64> {
        let key = crate::apiset::canonical_dll(dll);
        let k = key.as_str();
        self.table
            .binary_search_by(|(d, n, _)| (*d, *n).cmp(&(k, name)))
            .ok()
            .map(|idx| self.table[idx].2)
    }
}

impl Default for ShimResolver {
    fn default() -> Self {
        Self::new()
    }
}

/// Self-test for boot/smoke paths: every table entry has a distinct,
/// non-null address and resolves through `resolve_shim`. Returns
/// (table_len, verified_count).
pub fn shim_selftest() -> (usize, usize) {
    let table = shim_table();
    let mut verified = 0usize;
    for (dll, name, addr) in &table {
        if *addr == 0 {
            continue;
        }
        let dup = table.iter().filter(|(_, _, other)| other == addr).count();
        if dup == 1 && resolve_shim(dll, name) == Some(*addr) {
            verified += 1;
        }
    }
    (table.len(), verified)
}

/// The representative import set a typical MSVC `/MT` console CRT pulls from
/// kernel32 during `mainCRTStartup` → `__scrt_common_main` → `main`. Each is a
/// `(dll, name)` the loader must be able to IAT-patch to a real shim before the
/// guest runs, or startup hits a fail-loud stub. Used by the CRT-startup
/// readiness KAT and the boot smoketest.
pub const CRT_STARTUP_IMPORTS: &[(&str, &str)] = &[
    ("kernel32.dll", "GetModuleHandleW"),
    ("kernel32.dll", "GetModuleHandleExW"),
    ("kernel32.dll", "GetProcAddress"),
    ("kernel32.dll", "GetModuleFileNameW"),
    ("kernel32.dll", "GetProcessHeap"),
    ("kernel32.dll", "HeapAlloc"),
    ("kernel32.dll", "HeapFree"),
    ("kernel32.dll", "HeapReAlloc"),
    ("kernel32.dll", "HeapSize"),
    ("kernel32.dll", "TlsAlloc"),
    ("kernel32.dll", "TlsGetValue"),
    ("kernel32.dll", "TlsSetValue"),
    ("kernel32.dll", "TlsFree"),
    ("kernel32.dll", "GetCommandLineA"),
    ("kernel32.dll", "GetCommandLineW"),
    ("kernel32.dll", "GetEnvironmentStringsW"),
    ("kernel32.dll", "FreeEnvironmentStringsW"),
    ("kernel32.dll", "GetStartupInfoW"),
    ("kernel32.dll", "GetCurrentProcess"),
    ("kernel32.dll", "GetCurrentThread"),
    ("kernel32.dll", "GetCurrentProcessId"),
    ("kernel32.dll", "GetCurrentThreadId"),
    ("kernel32.dll", "GetStdHandle"),
    ("kernel32.dll", "WriteFile"),
    ("kernel32.dll", "WriteConsoleW"),
    ("kernel32.dll", "GetConsoleMode"),
    ("kernel32.dll", "SetConsoleMode"),
    ("kernel32.dll", "GetConsoleOutputCP"),
    ("kernel32.dll", "GetACP"),
    ("kernel32.dll", "GetCPInfo"),
    ("kernel32.dll", "GetLastError"),
    ("kernel32.dll", "SetLastError"),
    ("kernel32.dll", "IsProcessorFeaturePresent"),
    ("kernel32.dll", "IsDebuggerPresent"),
    ("kernel32.dll", "GetSystemTimeAsFileTime"),
    ("kernel32.dll", "QueryPerformanceCounter"),
    ("kernel32.dll", "QueryPerformanceFrequency"),
    ("kernel32.dll", "InitializeCriticalSectionEx"),
    ("kernel32.dll", "EnterCriticalSection"),
    ("kernel32.dll", "LeaveCriticalSection"),
    ("kernel32.dll", "DeleteCriticalSection"),
    ("kernel32.dll", "InitOnceExecuteOnce"),
    ("kernel32.dll", "ExitProcess"),
    ("kernel32.dll", "VirtualAlloc"),
    ("kernel32.dll", "VirtualFree"),
    // --- the further set a REAL cl.exe /MT console exe pulls before main
    //     (verified against fixtures/real_msvc_mt_hello.dumpbin.txt) ---
    ("kernel32.dll", "InitializeSListHead"),
    ("kernel32.dll", "EncodePointer"),
    ("kernel32.dll", "FlsAlloc"),
    ("kernel32.dll", "FlsFree"),
    ("kernel32.dll", "FlsGetValue"),
    ("kernel32.dll", "FlsSetValue"),
    ("kernel32.dll", "IsValidCodePage"),
    ("kernel32.dll", "MultiByteToWideChar"),
    ("kernel32.dll", "WideCharToMultiByte"),
    ("kernel32.dll", "GetStringTypeW"),
    ("kernel32.dll", "LCMapStringW"),
    ("kernel32.dll", "CompareStringW"),
    ("kernel32.dll", "GetOEMCP"),
    ("kernel32.dll", "GetFileType"),
    ("kernel32.dll", "GetFileSizeEx"),
    ("kernel32.dll", "SetStdHandle"),
    ("kernel32.dll", "FlushFileBuffers"),
    ("kernel32.dll", "SetFilePointerEx"),
    ("kernel32.dll", "SetEnvironmentVariableW"),
    ("kernel32.dll", "LoadLibraryExW"),
    ("kernel32.dll", "FreeLibrary"),
    ("kernel32.dll", "TerminateProcess"),
    ("kernel32.dll", "FindFirstFileExW"),
    ("kernel32.dll", "FindNextFileW"),
    ("kernel32.dll", "FindClose"),
    ("kernel32.dll", "RtlCaptureContext"),
    ("kernel32.dll", "RtlLookupFunctionEntry"),
    ("kernel32.dll", "RtlVirtualUnwind"),
    ("kernel32.dll", "RtlUnwindEx"),
    ("kernel32.dll", "RtlPcToFileHeader"),
    ("kernel32.dll", "RaiseException"),
    ("kernel32.dll", "UnhandledExceptionFilter"),
    ("kernel32.dll", "SetUnhandledExceptionFilter"),
    ("kernel32.dll", "InitializeCriticalSectionAndSpinCount"),
    ("ntdll.dll", "RtlAllocateHeap"),
    ("ntdll.dll", "RtlFreeHeap"),
];

/// Every import name the real MSVC `/MT` console fixture
/// (`testpe::REAL_MSVC_MT_EXE`) pulls from KERNEL32, as enumerated by
/// `dumpbin /imports` (see `fixtures/real_msvc_mt_hello.dumpbin.txt`). The
/// real-exe readiness KAT asserts EVERY one of these resolves to a real shim —
/// if any is missing the guest hits a fail-loud stub during CRT startup before
/// reaching `main`. This is the authoritative "can the real exe reach main"
/// import gate.
pub const REAL_MSVC_EXE_IMPORTS: &[&str] = &[
    "GetStdHandle",
    "WriteFile",
    "QueryPerformanceCounter",
    "GetCurrentProcessId",
    "GetCurrentThreadId",
    "GetSystemTimeAsFileTime",
    "InitializeSListHead",
    "RtlCaptureContext",
    "RtlLookupFunctionEntry",
    "RtlVirtualUnwind",
    "IsDebuggerPresent",
    "UnhandledExceptionFilter",
    "SetUnhandledExceptionFilter",
    "GetStartupInfoW",
    "IsProcessorFeaturePresent",
    "GetModuleHandleW",
    "WriteConsoleW",
    "RtlUnwindEx",
    "GetLastError",
    "SetLastError",
    "EnterCriticalSection",
    "LeaveCriticalSection",
    "DeleteCriticalSection",
    "InitializeCriticalSectionAndSpinCount",
    "TlsAlloc",
    "TlsGetValue",
    "TlsSetValue",
    "TlsFree",
    "FreeLibrary",
    "GetProcAddress",
    "LoadLibraryExW",
    "EncodePointer",
    "RaiseException",
    "RtlPcToFileHeader",
    "GetModuleFileNameW",
    "GetCurrentProcess",
    "ExitProcess",
    "TerminateProcess",
    "GetModuleHandleExW",
    "GetCommandLineA",
    "GetCommandLineW",
    "HeapAlloc",
    "HeapFree",
    "FindClose",
    "FindFirstFileExW",
    "FindNextFileW",
    "IsValidCodePage",
    "GetACP",
    "GetOEMCP",
    "GetCPInfo",
    "MultiByteToWideChar",
    "WideCharToMultiByte",
    "GetEnvironmentStringsW",
    "FreeEnvironmentStringsW",
    "SetEnvironmentVariableW",
    "SetStdHandle",
    "GetFileType",
    "GetFileSizeEx",
    "GetStringTypeW",
    "FlsAlloc",
    "FlsGetValue",
    "FlsSetValue",
    "FlsFree",
    "CompareStringW",
    "LCMapStringW",
    "GetProcessHeap",
    "HeapSize",
    "HeapReAlloc",
    "FlushFileBuffers",
    "GetConsoleOutputCP",
    "GetConsoleMode",
    "SetFilePointerEx",
    "CreateFileW",
    "CloseHandle",
];

/// Real-exe readiness: `(total, resolved)` over [`REAL_MSVC_EXE_IMPORTS`].
/// `total == resolved` ⇒ the real MSVC `/MT` exe's every IAT slot patches to a
/// real shim, so it can run CRT startup to `main` with no fail-loud stub.
pub fn real_exe_readiness() -> (usize, usize) {
    let mut resolved = 0usize;
    for name in REAL_MSVC_EXE_IMPORTS {
        if resolve_shim("kernel32.dll", name)
            .map(|a| a != 0)
            .unwrap_or(false)
        {
            resolved += 1;
        }
    }
    (REAL_MSVC_EXE_IMPORTS.len(), resolved)
}

/// CRT-startup readiness: returns `(total, resolved)` over [`CRT_STARTUP_IMPORTS`].
/// `total == resolved` means a real MSVC `/MT` console `.exe` importing this set
/// will have every IAT slot patched to a real (non-null) shim — i.e. it can run
/// to `main()` without hitting a fail-loud stub during startup. This is the
/// concrete "ready for a real .exe" proof (modulo the GS-base/TEB plumbing,
/// which is kernel-gated and separate).
pub fn crt_startup_readiness() -> (usize, usize) {
    let mut resolved = 0usize;
    for (dll, name) in CRT_STARTUP_IMPORTS {
        if resolve_shim(dll, name).map(|a| a != 0).unwrap_or(false) {
            resolved += 1;
        }
    }
    (CRT_STARTUP_IMPORTS.len(), resolved)
}

#[cfg(test)]
mod real_exe_tests {
    use super::*;
    use crate::pe_loader;
    use crate::testpe;

    #[test]
    fn user32_wndclassexw_marshal_and_shims_registered() {
        // Synthesize a guest WNDCLASSEXW (x64, 80 bytes): style @4, lpfnWndProc
        // @8, lpszClassName @64. The marshal must extract all three.
        let class_name: Vec<u16> = "MyWin\0".encode_utf16().collect();
        let mut wc = [0u8; 80];
        wc[4..8].copy_from_slice(&0x1234u32.to_le_bytes());
        wc[8..16].copy_from_slice(&0xDEAD_BEEFu64.to_le_bytes());
        wc[64..72].copy_from_slice(&(class_name.as_ptr() as u64).to_le_bytes());
        let got = unsafe { marshal_wndclassexw(wc.as_ptr()) };
        assert_eq!(got.style, 0x1234);
        assert_eq!(
            got.wnd_proc, 0xDEAD_BEEF,
            "lpfnWndProc must marshal from @8"
        );
        assert_eq!(
            got.class_name, "MyWin",
            "lpszClassName must marshal from @64"
        );
        assert!(got.menu_name.is_none(), "null lpszMenuName -> None");

        // The windowing entry points resolve to distinct, non-null shims (so the
        // IAT can patch them instead of the fail-loud trampoline).
        for name in [
            "RegisterClassExW",
            "CreateWindowExW",
            "ShowWindow",
            "UpdateWindow",
            "DefWindowProcW",
            "PostQuitMessage",
            "GetMessageW",
            "PeekMessageW",
            "TranslateMessage",
            "DispatchMessageW",
            "GetDC",
            "ReleaseDC",
            "BeginPaint",
            "EndPaint",
            "FillRect",
        ] {
            assert!(
                resolve_shim("user32.dll", name).is_some(),
                "{name} unregistered"
            );
            assert!(
                resolve_shim("USER32.DLL", name).is_some(),
                "{name} case-insensitive"
            );
        }
        // Whole-table integrity must still hold (every entry distinct + resolvable).
        let (len, verified) = shim_selftest();
        assert_eq!(
            len, verified,
            "shim table: all entries distinct + resolvable"
        );
    }

    #[test]
    fn user32_msg_marshal_round_trips() {
        // A guest MSG (x64, 48 bytes) must survive write -> read unchanged, so the
        // GetMessage/DispatchMessage shims hand the right fields to the pump.
        let src = crate::Msg {
            hwnd: WinHandle(0x0001_0007),
            message: 0x0100, // WM_KEYDOWN
            wparam: 0x41,    // 'A'
            lparam: 0x001E_0001,
            time: 0xCAFE,
            pt: crate::Point { x: 12, y: -7 },
        };
        let mut buf = [0u8; 48];
        unsafe { write_guest_msg(buf.as_mut_ptr(), &src) };
        let got = unsafe { read_guest_msg(buf.as_ptr()) };
        assert_eq!(got.hwnd.0, src.hwnd.0);
        assert_eq!(got.message, src.message);
        assert_eq!(got.wparam, src.wparam);
        assert_eq!(got.lparam, src.lparam);
        assert_eq!(got.time, src.time);
        assert_eq!(
            (got.pt.x, got.pt.y),
            (src.pt.x, src.pt.y),
            "POINT incl. negative y"
        );
    }
    use alloc::string::String;
    use alloc::vec::Vec;

    #[test]
    fn real_exe_import_list_all_resolves() {
        // Every import the enumerated list claims the real exe needs must
        // resolve to a real, non-null shim. FAIL-able: a missing/NULL shim is a
        // startup fail-loud stub for the real binary.
        let (total, resolved) = real_exe_readiness();
        assert_eq!(
            total, resolved,
            "real-exe imports not all resolved: {resolved}/{total}"
        );
    }

    #[test]
    fn real_exe_fixture_parses_as_pe32plus() {
        let info = crate::load_pe(testpe::REAL_MSVC_MT_EXE).expect("real exe must parse");
        assert_eq!(info.machine, crate::MachineType::Amd64);
        assert_eq!(info.format, crate::PeFormat::Pe32Plus);
    }

    #[test]
    fn real_exe_every_parsed_import_resolves() {
        // The STRONGEST gate: parse the actual fixture's import directory and
        // assert each (dll, name) it lists resolves to a real shim. Catches any
        // import the hand-maintained list missed; names exactly which one fails.
        let mut reg = pe_loader::DllRegistry::new();
        let loaded = pe_loader::load_pe(testpe::REAL_MSVC_MT_EXE, &mut reg).expect("load real exe");
        assert!(loaded.is_64bit, "real exe is 64-bit");
        let mut unresolved: Vec<String> = Vec::new();
        for imp in &loaded.imports {
            if imp.function_name.is_empty() {
                continue; // ordinal-only / table-terminator entries
            }
            let dll = imp.module_name.to_ascii_lowercase();
            if resolve_shim(&dll, &imp.function_name).is_none() {
                unresolved.push(alloc::format!("{}!{}", dll, imp.function_name));
            }
        }
        assert!(
            unresolved.is_empty(),
            "real exe imports with NO shim (would fail-loud before main): {:?}",
            unresolved
        );
        // A parse that found 0 imports would vacuously pass; the real /MT exe
        // imports 73 from KERNEL32.
        assert!(
            loaded.imports.len() >= 70,
            "expected ~73 imports, parsed {}",
            loaded.imports.len()
        );
    }

    #[test]
    fn real_printf_exe_fixture_parses_as_pe32plus() {
        let info =
            crate::load_pe(testpe::REAL_MSVC_MT_PRINTF_EXE).expect("real printf exe must parse");
        assert_eq!(info.machine, crate::MachineType::Amd64);
        assert_eq!(info.format, crate::PeFormat::Pe32Plus);
    }

    #[test]
    fn real_printf_exe_every_parsed_import_resolves() {
        // The STRONGEST gate for the printf fixture: parse the actual fixture's
        // import directory and assert each (dll, name) it lists resolves to a
        // real shim. The printf CRT pulls exactly one import the WriteFile
        // fixture didn't — GetFileSizeEx — so this fails loud if that shim is
        // missing. Mirrors `real_exe_every_parsed_import_resolves`.
        let mut reg = pe_loader::DllRegistry::new();
        let loaded = pe_loader::load_pe(testpe::REAL_MSVC_MT_PRINTF_EXE, &mut reg)
            .expect("load real printf exe");
        assert!(loaded.is_64bit, "real printf exe is 64-bit");
        let mut unresolved: Vec<String> = Vec::new();
        for imp in &loaded.imports {
            if imp.function_name.is_empty() {
                continue; // ordinal-only / table-terminator entries
            }
            let dll = imp.module_name.to_ascii_lowercase();
            if resolve_shim(&dll, &imp.function_name).is_none() {
                unresolved.push(alloc::format!("{}!{}", dll, imp.function_name));
            }
        }
        assert!(
            unresolved.is_empty(),
            "printf exe imports with NO shim (would fail-loud before printf): {:?}",
            unresolved
        );
        // The real /MT printf exe imports 74 from KERNEL32 (73 + GetFileSizeEx).
        assert!(
            loaded.imports.len() >= 70,
            "expected ~74 imports, parsed {}",
            loaded.imports.len()
        );
    }

    #[test]
    fn get_file_size_ex_shim_is_registered() {
        // The one new import the printf fixture adds over the WriteFile fixture
        // must resolve to a real, non-null shim.
        assert!(
            resolve_shim("kernel32.dll", "GetFileSizeEx").map(|a| a != 0) == Some(true),
            "GetFileSizeEx must resolve to a real shim for the printf milestone"
        );
    }

    #[test]
    fn real_cpp_exe_fixture_parses_as_pe32plus() {
        let info = crate::load_pe(testpe::REAL_MSVC_MT_CPP_EXE).expect("real C++ exe must parse");
        assert_eq!(info.machine, crate::MachineType::Amd64);
        assert_eq!(info.format, crate::PeFormat::Pe32Plus);
    }

    #[test]
    fn real_cpp_exe_every_parsed_import_resolves() {
        // The STRONGEST gate for the C++ fixture: parse the actual fixture's
        // import directory and assert each (dll, name) it lists resolves to a
        // real shim. The /MT C++ CRT pulls ZERO imports beyond the printf
        // fixture (static-ctor walk / atexit / EH personality are CRT-internal),
        // so the same 74 KERNEL32 imports must all resolve or the guest would
        // fail-loud before the static initializers run. Mirrors
        // `real_printf_exe_every_parsed_import_resolves`.
        let mut reg = pe_loader::DllRegistry::new();
        let loaded =
            pe_loader::load_pe(testpe::REAL_MSVC_MT_CPP_EXE, &mut reg).expect("load real C++ exe");
        assert!(loaded.is_64bit, "real C++ exe is 64-bit");
        let mut unresolved: Vec<String> = Vec::new();
        for imp in &loaded.imports {
            if imp.function_name.is_empty() {
                continue; // ordinal-only / table-terminator entries
            }
            let dll = imp.module_name.to_ascii_lowercase();
            if resolve_shim(&dll, &imp.function_name).is_none() {
                unresolved.push(alloc::format!("{}!{}", dll, imp.function_name));
            }
        }
        assert!(
            unresolved.is_empty(),
            "C++ exe imports with NO shim (would fail-loud before static-init): {:?}",
            unresolved
        );
        // The /MT C++ exe imports 74 from KERNEL32 (same set as the printf
        // fixture). A parse that found 0 imports would vacuously pass.
        assert!(
            loaded.imports.len() >= 70,
            "expected ~74 imports, parsed {}",
            loaded.imports.len()
        );
    }

    #[test]
    fn real_cpp_exe_imports_match_printf_fixture_exactly() {
        // Proves the design claim that the C++-runtime delta adds ZERO new
        // imports over the printf fixture (the static-ctor table walk, atexit,
        // and the C++ EH personality are CRT-internal in /MT). If a future
        // toolchain change pulled a new import, this names the delta loud so the
        // shim surface can be extended deliberately rather than silently.
        let mut reg = pe_loader::DllRegistry::new();
        let cpp = pe_loader::load_pe(testpe::REAL_MSVC_MT_CPP_EXE, &mut reg).expect("load C++ exe");
        let mut reg2 = pe_loader::DllRegistry::new();
        let printf = pe_loader::load_pe(testpe::REAL_MSVC_MT_PRINTF_EXE, &mut reg2)
            .expect("load printf exe");
        let mut cpp_names: Vec<String> = cpp
            .imports
            .iter()
            .filter(|i| !i.function_name.is_empty())
            .map(|i| alloc::format!("{}!{}", i.module_name.to_ascii_lowercase(), i.function_name))
            .collect();
        let mut printf_names: Vec<String> = printf
            .imports
            .iter()
            .filter(|i| !i.function_name.is_empty())
            .map(|i| alloc::format!("{}!{}", i.module_name.to_ascii_lowercase(), i.function_name))
            .collect();
        cpp_names.sort();
        cpp_names.dedup();
        printf_names.sort();
        printf_names.dedup();
        assert_eq!(
            cpp_names, printf_names,
            "C++ fixture import set diverged from printf fixture (C++-runtime delta is supposed to be CRT-internal / zero new imports)"
        );
    }

    #[test]
    fn real_cpp_msg_has_both_lines_ctor_before_main() {
        // The PASS-condition bytes the smoketest asserts: the static ctor's line
        // MUST precede main's line. This guards the verdict's "ctor before main"
        // ordering contract (a binary that skipped static-init would emit only
        // the main line). FAIL-demo: a slice missing the ctor line is not
        // REAL_CPP_MSG.
        let msg = testpe::REAL_CPP_MSG;
        let ctor_pos = msg
            .windows(testpe::CPP_CTOR_LINE.len())
            .position(|w| w == testpe::CPP_CTOR_LINE)
            .expect("ctor line must be present in REAL_CPP_MSG");
        let main_pos = msg
            .windows(b"hello from c++ 7".len())
            .position(|w| w == b"hello from c++ 7")
            .expect("main line must be present in REAL_CPP_MSG");
        assert!(
            ctor_pos < main_pos,
            "static ctor line must come BEFORE main line"
        );
        // The main-only output would NOT equal the PASS bytes (FAIL-able proof).
        assert_ne!(
            b"hello from c++ 7\r\n".as_slice(),
            msg,
            "main-only output must not satisfy the C++ milestone"
        );
    }
}

#[cfg(test)]
mod shim_tests {
    use super::*;
    use crate::testpe;
    use crate::{FullCompatSession, SessionId};
    use alloc::boxed::Box;
    use alloc::string::ToString;

    fn fresh_session() -> Box<FullCompatSession> {
        let exe = testpe::build_exit_process_exe();
        Box::new(
            FullCompatSession::new(
                SessionId(1),
                "app.exe".to_string(),
                exe,
                "app.exe".to_string(),
            )
            .unwrap(),
        )
    }

    // The whole suite shares ONE process-global `HOST_CTX` (the production model:
    // one guest per loader process). `cargo test` runs tests multi-threaded, so
    // without serialization a concurrent test's `install_host_context` clobbers
    // the ctx mid-body — e.g. `heap_size` returns `(SIZE_T)-1` for an allocation
    // a sibling test's `install` just replaced. Hold this mutex for the whole
    // closure so every HOST_CTX-mutating test is mutually exclusive. Poison-
    // recover so one panicking (assert-failing) test still reports instead of
    // wedging the rest of the suite.
    static CTX_SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn with_fresh_ctx<R>(f: impl FnOnce() -> R) -> R {
        let _serial = CTX_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        // Install a fresh session, run, then drop it so the next test reinstalls
        // cleanly — now race-free under the serial lock above.
        let _ = install_host_context(fresh_session());
        f()
    }

    #[test]
    fn crt_startup_set_all_resolves_to_real_addresses() {
        // THE readiness proof: every representative CRT-startup import must
        // resolve to a real, non-null shim. If a name is missing or NULL this
        // FAILS loudly — that's a startup-time fail-loud stub for a real .exe.
        let (total, resolved) = crt_startup_readiness();
        assert_eq!(
            total, resolved,
            "CRT-startup imports not all resolved: {resolved}/{total}"
        );
        // And spot-check that resolution is exact per (dll, name).
        for (dll, name) in CRT_STARTUP_IMPORTS {
            let a = resolve_shim(dll, name);
            assert!(a.is_some(), "{dll}!{name} did not resolve");
            assert_ne!(a.unwrap(), 0, "{dll}!{name} resolved to NULL");
        }
    }

    /// A WndProc (called via the exact `invoke_wndproc` transmute+call path a
    /// guest uses) that handles WM_PAINT by calling BACK into the Win32 API
    /// through the shims — every one re-enters `with_ctx`. If the dispatch shim
    /// held the ctx lock across the invoke, the first call here would deadlock.
    unsafe extern "win64" fn paint_wndproc(hwnd: u64, msg: u32, _w: u64, _l: i64) -> i64 {
        if msg == crate::WM_PAINT {
            let mut ps = [0u8; 72];
            let dc = shim_begin_paint(hwnd, ps.as_mut_ptr());
            let brush = shim_create_solid_brush(0x00FF_FFFF); // white
                                                              // rcPaint lives at PAINTSTRUCT offset 12.
            shim_fill_rect(dc, ps.as_ptr().add(12), brush);
            let txt = [b'H' as u16, b'I' as u16];
            shim_text_out_w(dc, 2, 2, txt.as_ptr(), 2);
            shim_end_paint(hwnd, ps.as_ptr());
        }
        0
    }

    #[test]
    fn gui_update_window_dispatches_to_wndproc_and_paints() {
        use crate::{Rect, WinHandle, WindowObject, WndClassExW};
        with_fresh_ctx(|| {
            // A 32x16 ARGB surface backed by a host buffer + a window of a class
            // whose WndProc is `paint_wndproc` (set as the guest lpfnWndProc).
            let mut surf = alloc::vec![0u32; 32 * 16];
            let surf_ptr = surf.as_mut_ptr() as u64;
            let hwnd = 0x0001_0000u64;
            let proc_addr = paint_wndproc as unsafe extern "win64" fn(u64, u32, u64, i64) -> i64
                as usize as u64;
            with_ctx(|ctx| {
                ctx.registered_classes.insert(
                    "RaeGuiTest".to_string(),
                    WndClassExW {
                        style: 0,
                        wnd_proc: proc_addr,
                        class_name: "RaeGuiTest".to_string(),
                        icon: WinHandle(0),
                        cursor: WinHandle(0),
                        background: WinHandle(0),
                        menu_name: None,
                        icon_sm: WinHandle(0),
                    },
                );
                ctx.windows.insert(
                    hwnd,
                    WindowObject {
                        handle: WinHandle(hwnd),
                        class_name: "RaeGuiTest".to_string(),
                        title: String::new(),
                        style: 0,
                        ex_style: 0,
                        rect: Rect {
                            left: 0,
                            top: 0,
                            right: 32,
                            bottom: 16,
                        },
                        client_rect: Rect {
                            left: 0,
                            top: 0,
                            right: 32,
                            bottom: 16,
                        },
                        parent: WinHandle(0),
                        visible: true,
                        enabled: true,
                        user_data: 0,
                        surface_id: None,
                        surface_vaddr: Some(surf_ptr),
                    },
                );
            });
            // The reentrancy test: UpdateWindow -> WM_PAINT -> paint_wndproc ->
            // BeginPaint/FillRect/TextOut/EndPaint (each re-enters with_ctx). If
            // this returns at all, the dispatch lock was released before the
            // invoke (no deadlock).
            let r = unsafe { shim_update_window(hwnd) };
            assert_eq!(r, 1, "UpdateWindow on a live window returns TRUE");
            // The WndProc actually rendered into the surface: the white FillRect
            // covered the background, and black TextOut drew the "HI" glyphs.
            let white = surf.iter().filter(|&&p| p == 0xFFFF_FFFF).count();
            let black = surf.iter().filter(|&&p| p == 0xFF00_0000).count();
            assert!(
                white > (32 * 16) / 2,
                "FillRect must paint most of the bg white (got {white})"
            );
            assert!(black > 0, "TextOut must draw glyph pixels (got {black})");
        });
    }

    /// Accumulates the WM_CHAR characters a WndProc receives, so the typing test
    /// can assert the keystrokes arrived translated + in order.
    static TYPED: std::sync::Mutex<Vec<u8>> = std::sync::Mutex::new(Vec::new());

    unsafe extern "win64" fn typing_wndproc(_hwnd: u64, msg: u32, wp: u64, _lp: i64) -> i64 {
        if msg == crate::WM_CHAR {
            TYPED
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(wp as u8);
        }
        0
    }

    #[test]
    fn typing_keydown_translates_to_wm_char_to_wndproc() {
        use crate::{Rect, WinHandle, WindowObject, WndClassExW};
        with_fresh_ctx(|| {
            TYPED.lock().unwrap_or_else(|e| e.into_inner()).clear();
            let hwnd = 0x0001_0000u64;
            let proc_addr = typing_wndproc as unsafe extern "win64" fn(u64, u32, u64, i64) -> i64
                as usize as u64;
            with_ctx(|ctx| {
                ctx.registered_classes.insert(
                    "RaeType".to_string(),
                    WndClassExW {
                        style: 0,
                        wnd_proc: proc_addr,
                        class_name: "RaeType".to_string(),
                        icon: WinHandle(0),
                        cursor: WinHandle(0),
                        background: WinHandle(0),
                        menu_name: None,
                        icon_sm: WinHandle(0),
                    },
                );
                ctx.windows.insert(
                    hwnd,
                    WindowObject {
                        handle: WinHandle(hwnd),
                        class_name: "RaeType".to_string(),
                        title: String::new(),
                        style: 0,
                        ex_style: 0,
                        rect: Rect {
                            left: 0,
                            top: 0,
                            right: 64,
                            bottom: 32,
                        },
                        client_rect: Rect {
                            left: 0,
                            top: 0,
                            right: 64,
                            bottom: 32,
                        },
                        parent: WinHandle(0),
                        visible: true,
                        enabled: true,
                        user_data: 0,
                        surface_id: None,
                        surface_vaddr: None,
                    },
                );
            });
            // Inject "HI" as keystrokes (VK letter codes ARE ASCII).
            unsafe {
                shim_post_message_w(hwnd, crate::WM_KEYDOWN, b'H' as u64, 0);
                shim_post_message_w(hwnd, crate::WM_KEYDOWN, b'I' as u64, 0);
            }
            // The standard pump: GetMessage -> TranslateMessage (KEYDOWN->WM_CHAR)
            // -> DispatchMessage (WM_CHAR -> WndProc). Drain until the queue empties
            // (our GetMessage returns message==0 on empty); guard against a hang.
            for _ in 0..100 {
                let mut buf = [0u8; 48];
                let got = unsafe { shim_get_message_w(buf.as_mut_ptr(), 0, 0, 0) };
                let m = unsafe { read_guest_msg(buf.as_ptr()) };
                if got == 0 || m.message == 0 {
                    break;
                }
                unsafe {
                    shim_translate_message(buf.as_ptr());
                    shim_dispatch_message_w(buf.as_ptr());
                }
            }
            let typed = TYPED.lock().unwrap_or_else(|e| e.into_inner()).clone();
            assert_eq!(
                typed, b"HI",
                "WM_KEYDOWN must translate to WM_CHAR and reach the WndProc in order"
            );
        });
    }

    #[test]
    fn window_text_shims_round_trip_through_guest_buffers() {
        use crate::{Rect, WinHandle, WindowObject};
        with_fresh_ctx(|| {
            let hwnd = 0x0001_0000u64;
            with_ctx(|ctx| {
                ctx.windows.insert(
                    hwnd,
                    WindowObject {
                        handle: WinHandle(hwnd),
                        class_name: "RaeText".to_string(),
                        title: String::new(),
                        style: 0,
                        ex_style: 0,
                        rect: Rect {
                            left: 0,
                            top: 0,
                            right: 64,
                            bottom: 32,
                        },
                        client_rect: Rect {
                            left: 0,
                            top: 0,
                            right: 64,
                            bottom: 32,
                        },
                        parent: WinHandle(0),
                        visible: true,
                        enabled: true,
                        user_data: 0,
                        surface_id: None,
                        surface_vaddr: None,
                    },
                );
            });

            // SetWindowTextW from a guest LPCWSTR stores the text.
            let src = crate::string_to_wide("Saved file body");
            let r = unsafe { shim_set_window_text_w(hwnd, src.as_ptr()) };
            assert_eq!(r, 1, "SetWindowTextW on a live window returns TRUE");

            // GetWindowTextLengthW reports the UTF-16 length (excl NUL).
            let len = unsafe { shim_get_window_text_length_w(hwnd) };
            assert_eq!(len, "Saved file body".encode_utf16().count() as i32);

            // GetWindowTextW into an ample buffer reads it back verbatim.
            let mut buf = [0u16; 64];
            let n = unsafe { shim_get_window_text_w(hwnd, buf.as_mut_ptr(), buf.len() as i32) };
            assert_eq!(n, 15);
            assert_eq!(crate::wide_to_string(&buf), "Saved file body");

            // GetWindowTextW into a SMALL buffer truncates to (max-1) chars + NUL —
            // the off-by-one a real save handler relies on not corrupting memory.
            let mut small = [0xFFFFu16; 5];
            let n = unsafe { shim_get_window_text_w(hwnd, small.as_mut_ptr(), small.len() as i32) };
            assert_eq!(n, 4, "5-unit buffer keeps 4 chars + NUL");
            assert_eq!(small[4], 0, "buffer NUL-terminated at index max-1");
            assert_eq!(crate::wide_to_string(&small), "Save");

            // The WM_* messages route to the same storage via DefWindowProcW.
            let l = unsafe { shim_def_window_proc_w(hwnd, crate::WM_GETTEXTLENGTH, 0, 0) };
            assert_eq!(l, 15);
            let new = crate::string_to_wide("X");
            let _ = unsafe {
                shim_def_window_proc_w(hwnd, crate::WM_SETTEXT, 0, new.as_ptr() as usize as i64)
            };
            let mut gbuf = [0u16; 8];
            let got = unsafe {
                shim_def_window_proc_w(
                    hwnd,
                    crate::WM_GETTEXT,
                    gbuf.len() as u64,
                    gbuf.as_mut_ptr() as usize as i64,
                )
            };
            assert_eq!(got, 1, "WM_GETTEXT returns 1 char after WM_SETTEXT(\"X\")");
            assert_eq!(crate::wide_to_string(&gbuf), "X");
        });
    }

    #[test]
    fn edit_control_typing_through_shim_pump() {
        // Closes the host-coverage gap the QEMU edit-control smoketest exposed:
        // NO prior host KAT ran the full shim pump into a system EDIT control
        // (the typing KAT used a guest WndProc; the EDIT KAT bypassed the pump).
        // Here the real pump — PostMessage(WM_KEYDOWN) -> GetMessage ->
        // TranslateMessage -> DispatchMessage -> built-in EDIT proc — types "HI"
        // into an unregistered "EDIT" window, then GetWindowTextW reads it back.
        use crate::{Rect, WinHandle, WindowObject};
        with_fresh_ctx(|| {
            let hwnd = 0x0003_0000u64;
            with_ctx(|ctx| {
                ctx.windows.insert(
                    hwnd,
                    WindowObject {
                        handle: WinHandle(hwnd),
                        class_name: "EDIT".to_string(),
                        title: String::new(),
                        style: 0,
                        ex_style: 0,
                        rect: Rect {
                            left: 0,
                            top: 0,
                            right: 120,
                            bottom: 24,
                        },
                        client_rect: Rect {
                            left: 0,
                            top: 0,
                            right: 120,
                            bottom: 24,
                        },
                        parent: WinHandle(0),
                        visible: true,
                        enabled: true,
                        user_data: 0,
                        surface_id: None,
                        surface_vaddr: None,
                    },
                );
            });
            unsafe {
                shim_post_message_w(hwnd, crate::WM_KEYDOWN, b'H' as u64, 0);
                shim_post_message_w(hwnd, crate::WM_KEYDOWN, b'I' as u64, 0);
            }
            for _ in 0..32 {
                let mut buf = [0u8; 48];
                let got = unsafe { shim_get_message_w(buf.as_mut_ptr(), 0, 0, 0) };
                let m = unsafe { read_guest_msg(buf.as_ptr()) };
                if got == 0 || m.message == 0 {
                    break;
                }
                unsafe {
                    shim_translate_message(buf.as_ptr());
                    shim_dispatch_message_w(buf.as_ptr());
                }
            }
            let mut out = [0u16; 16];
            let n = unsafe { shim_get_window_text_w(hwnd, out.as_mut_ptr(), out.len() as i32) };
            let text = crate::wide_to_string(&out[..(n as usize).min(out.len())]);
            assert_eq!(
                text, "HI",
                "EDIT control must accumulate typed text through the full shim pump"
            );
        });
    }

    #[test]
    fn comdlg32_dialogs_confirm_path_into_ofn() {
        // GetSaveFileNameW marshals OPENFILENAMEW (lpstrFile@48, nMaxFile@56),
        // resolves the path, and writes it back. Pure memory — no syscall, no ctx.
        // A bare default name -> anchored under C:\.
        let mut file_buf = [0u16; 64];
        for (i, u) in crate::string_to_wide("notes.txt").iter().enumerate() {
            file_buf[i] = *u;
        }
        let mut ofn = [0u8; 152];
        let fp = file_buf.as_mut_ptr() as u64;
        ofn[48..56].copy_from_slice(&fp.to_le_bytes());
        ofn[56..60].copy_from_slice(&(file_buf.len() as u32).to_le_bytes());
        let r = unsafe { shim_get_save_file_name_w(ofn.as_mut_ptr()) };
        assert_eq!(r, 1, "Save dialog confirms -> TRUE");
        assert_eq!(crate::wide_to_string(&file_buf), "C:\\notes.txt");

        // An empty default -> the deterministic fallback path.
        let mut empty = [0u16; 64];
        let mut ofn2 = [0u8; 152];
        let ep = empty.as_mut_ptr() as u64;
        ofn2[48..56].copy_from_slice(&ep.to_le_bytes());
        ofn2[56..60].copy_from_slice(&(empty.len() as u32).to_le_bytes());
        let r = unsafe { shim_get_open_file_name_w(ofn2.as_mut_ptr()) };
        assert_eq!(r, 1);
        assert_eq!(crate::wide_to_string(&empty), "C:\\untitled.txt");

        // A NULL struct is a clean FALSE (cancel), not a crash.
        assert_eq!(
            unsafe { shim_get_save_file_name_w(core::ptr::null_mut()) },
            0
        );
    }

    #[test]
    fn comdlg32_save_open_resolve_to_real_shims() {
        // The two new names must resolve to real, non-null shims (else a guest's
        // File->Save/Open hits the fail-loud trampoline).
        assert!(resolve_shim("comdlg32.dll", "GetSaveFileNameW").map(|a| a != 0) == Some(true));
        assert!(resolve_shim("comdlg32.dll", "GetOpenFileNameW").map(|a| a != 0) == Some(true));
    }

    #[test]
    fn window_mgmt_shims_marshal_and_resolve() {
        use crate::{Rect, WinHandle, WindowObject};
        with_fresh_ctx(|| {
            let hwnd = 0x0001_0000u64;
            with_ctx(|ctx| {
                ctx.windows.insert(
                    hwnd,
                    WindowObject {
                        handle: WinHandle(hwnd),
                        class_name: "RaeWM".to_string(),
                        title: String::new(),
                        style: 0,
                        ex_style: 0,
                        rect: Rect {
                            left: 10,
                            top: 20,
                            right: 110,
                            bottom: 80,
                        },
                        client_rect: Rect {
                            left: 0,
                            top: 0,
                            right: 100,
                            bottom: 60,
                        },
                        parent: WinHandle(0),
                        visible: false,
                        enabled: true,
                        user_data: 0,
                        surface_id: None,
                        surface_vaddr: None,
                    },
                );
            });
            // GetClientRect marshals the client rect out into the guest RECT.
            let mut rc = [0u8; 16];
            let ok = unsafe { shim_get_client_rect(hwnd, rc.as_mut_ptr()) };
            assert_eq!(ok, 1);
            let read_i32 = |o: usize| i32::from_le_bytes([rc[o], rc[o + 1], rc[o + 2], rc[o + 3]]);
            assert_eq!(
                (read_i32(0), read_i32(4), read_i32(8), read_i32(12)),
                (0, 0, 100, 60)
            );
            // GetWindowRect marshals the window rect out.
            let mut wr = [0u8; 16];
            assert_eq!(unsafe { shim_get_window_rect(hwnd, wr.as_mut_ptr()) }, 1);
            let wi = |o: usize| i32::from_le_bytes([wr[o], wr[o + 1], wr[o + 2], wr[o + 3]]);
            assert_eq!((wi(0), wi(4), wi(8), wi(12)), (10, 20, 110, 80));
            // SetWindowLongW(GWL_USERDATA) round-trips through GetWindowLongPtrW.
            unsafe { shim_set_window_long_ptr_w(hwnd, -21, 0x1234_5678) };
            assert_eq!(
                unsafe { shim_get_window_long_ptr_w(hwnd, -21) },
                0x1234_5678
            );
            // MessageBoxW (no real picker) confirms with IDOK.
            let txt = crate::string_to_wide("hi");
            let cap = crate::string_to_wide("t");
            assert_eq!(
                unsafe { shim_message_box_w(0, txt.as_ptr(), cap.as_ptr(), 0) },
                crate::IDOK
            );
            // An unknown hwnd is a clean FALSE, not a crash.
            let mut junk = [0u8; 16];
            assert_eq!(
                unsafe { shim_get_client_rect(0x9999, junk.as_mut_ptr()) },
                0
            );
        });
    }

    #[test]
    fn fail_demo_unknown_import_does_not_resolve() {
        // The readiness check must be able to FAIL: an import that is NOT in the
        // shim table resolves to None. (Flip a real name to this to watch the
        // readiness KAT go red — that's the FAIL-able property.)
        assert!(resolve_shim("kernel32.dll", "NoSuchExport_DEADBEEF").is_none());
        // A real entry, by contrast, resolves — proving the negative is real.
        assert!(resolve_shim("kernel32.dll", "GetProcAddress").is_some());
    }

    #[test]
    fn whole_table_distinct_and_resolvable() {
        let (len, verified) = shim_selftest();
        assert_eq!(len, verified, "every shim must be distinct + resolvable");
    }

    #[test]
    fn shim_resolver_matches_resolve_shim_and_builds_once() {
        // The fast path must be behavior-identical to the slow path. Build the
        // index once, then prove it answers every representative CRT-startup
        // import exactly as the per-call resolver, including API Set redirection.
        let resolver = ShimResolver::new();
        // Built from the full table (once): len parity with a fresh build.
        assert_eq!(resolver.len(), shim_table().len());
        assert!(!resolver.is_empty());

        // Equivalence on the real CRT startup import set.
        for (dll, name) in CRT_STARTUP_IMPORTS {
            assert_eq!(
                resolver.resolve(dll, name),
                resolve_shim(dll, name),
                "resolver must match resolve_shim for a bound import"
            );
        }
        // Equivalence through the API Set redirect: contract-dll form resolves
        // to the same address as the canonical host form.
        assert_eq!(
            resolver.resolve(
                "api-ms-win-core-profile-l1-1-0.dll",
                "QueryPerformanceCounter"
            ),
            resolve_shim("kernel32.dll", "QueryPerformanceCounter"),
        );
        // FAIL-able: an unbound import is None on the fast path too.
        assert!(resolver
            .resolve("kernel32.dll", "NoSuchExport_DEADBEEF")
            .is_none());
    }

    #[test]
    fn api_set_imports_resolve_to_the_same_shim_as_the_host_dll() {
        // Regression guard for the API Set redirection fix. These are the exact
        // (contract-dll, function) pairs the win-api-survey found dominating real
        // Win10 System32 binaries' import tables — they are imported through
        // api-ms-win-* contracts, NOT kernel32.dll. Before redirection every one
        // resolved to a fail-loud stub (None); now each must resolve to the SAME
        // address as the canonical host export. If this fails, modern binaries
        // die at their first CRT/startup call.
        let cases: &[(&str, &str, &str)] = &[
            (
                "api-ms-win-core-profile-l1-1-0.dll",
                "kernel32.dll",
                "QueryPerformanceCounter",
            ),
            (
                "api-ms-win-core-processthreads-l1-1-0.dll",
                "kernel32.dll",
                "GetCurrentProcess",
            ),
            (
                "api-ms-win-core-heap-l1-1-0.dll",
                "kernel32.dll",
                "HeapAlloc",
            ),
            (
                "api-ms-win-core-libraryloader-l1-2-0.dll",
                "kernel32.dll",
                "GetProcAddress",
            ),
            (
                "api-ms-win-core-synch-l1-2-0.dll",
                "kernel32.dll",
                "WaitForSingleObject",
            ),
            ("api-ms-win-core-synch-l1-1-0.dll", "kernel32.dll", "Sleep"),
        ];
        for (contract, host, func) in cases {
            let via_contract = resolve_shim(contract, func);
            let via_host = resolve_shim(host, func);
            assert!(
                via_host.is_some(),
                "host export must exist for the test to be meaningful"
            );
            assert_eq!(
                via_contract, via_host,
                "api-set import must redirect to the host shim"
            );
        }
        // FAIL-able: a bogus contract name for a real function still must not
        // invent a resolution beyond the redirect target.
        assert!(
            resolve_shim("api-ms-win-core-synch-l1-2-0.dll", "NoSuchExport_DEADBEEF").is_none()
        );
    }

    #[test]
    fn get_module_handle_returns_seeded_bases() {
        with_fresh_ctx(|| unsafe {
            // NULL => main module base.
            assert_eq!(
                shim_get_module_handle_w(core::ptr::null()),
                crate::MAIN_MODULE_BASE
            );
            // Named system DLLs return their seeded synthetic bases.
            let k32: alloc::vec::Vec<u16> = "kernel32.dll"
                .encode_utf16()
                .chain(core::iter::once(0))
                .collect();
            assert_eq!(
                shim_get_module_handle_w(k32.as_ptr()),
                crate::KERNEL32_MODULE_BASE
            );
            let nt: alloc::vec::Vec<u16> = "ntdll.dll"
                .encode_utf16()
                .chain(core::iter::once(0))
                .collect();
            assert_eq!(
                shim_get_module_handle_w(nt.as_ptr()),
                crate::NTDLL_MODULE_BASE
            );
            // An unknown module returns NULL (0).
            let bogus: alloc::vec::Vec<u16> = "nope.dll"
                .encode_utf16()
                .chain(core::iter::once(0))
                .collect();
            assert_eq!(shim_get_module_handle_w(bogus.as_ptr()), 0);
        });
    }

    #[test]
    fn get_proc_address_resolves_known_and_rejects_unknown() {
        with_fresh_ctx(|| unsafe {
            let k32 = shim_get_module_handle_w(
                "kernel32.dll"
                    .encode_utf16()
                    .chain(core::iter::once(0))
                    .collect::<alloc::vec::Vec<u16>>()
                    .as_ptr(),
            );
            // Known export resolves to the SAME address the IAT patcher uses.
            let name = b"WriteFile\0";
            let got = shim_get_proc_address(k32, name.as_ptr());
            assert_ne!(got, 0, "GetProcAddress(WriteFile) must be non-null");
            assert_eq!(got, resolve_shim("kernel32.dll", "WriteFile").unwrap());
            // Unknown export returns NULL.
            let bad = b"TotallyNotAnExport\0";
            assert_eq!(shim_get_proc_address(k32, bad.as_ptr()), 0);
            // ntdll alias resolves through its own module base.
            let nt = shim_get_module_handle_w(
                "ntdll.dll"
                    .encode_utf16()
                    .chain(core::iter::once(0))
                    .collect::<alloc::vec::Vec<u16>>()
                    .as_ptr(),
            );
            let rtl = b"RtlAllocateHeap\0";
            assert_ne!(shim_get_proc_address(nt, rtl.as_ptr()), 0);
        });
    }

    #[test]
    fn tls_round_trips_through_shims() {
        with_fresh_ctx(|| unsafe {
            let idx = shim_tls_alloc();
            assert_ne!(idx, k::TLS_OUT_OF_INDEXES);
            // A fresh slot reads 0.
            assert_eq!(shim_tls_get_value(idx), 0);
            // Set then get round-trips the value.
            assert_eq!(shim_tls_set_value(idx, 0xDEAD_BEEF_CAFE), 1);
            assert_eq!(shim_tls_get_value(idx), 0xDEAD_BEEF_CAFE);
            // Free invalidates the slot.
            assert_eq!(shim_tls_free(idx), 1);
            assert_eq!(shim_tls_set_value(idx, 1), 0); // now invalid
        });
    }

    #[test]
    fn heap_realloc_preserves_bytes_through_shims() {
        with_fresh_ctx(|| unsafe {
            let heap = shim_get_process_heap();
            let p = shim_heap_alloc(heap, 0, 8);
            assert_ne!(p, 0);
            // Write a recognizable pattern.
            for i in 0..8u64 {
                (p as *mut u8).add(i as usize).write(i as u8 + 1);
            }
            // HeapSize reports the live size.
            assert_eq!(shim_heap_size(heap, 0, p), 8);
            // Grow; the original 8 bytes must survive.
            let q = shim_heap_realloc(heap, 0, p, 32);
            assert_ne!(q, 0);
            for i in 0..8u64 {
                assert_eq!((q as *const u8).add(i as usize).read(), i as u8 + 1);
            }
            assert_eq!(shim_heap_size(heap, 0, q), 32);
            assert_eq!(shim_heap_free(heap, 0, q), 1);
        });
    }

    // NOTE: WriteConsoleW's tee-to-stdout behavior is NOT host-KAT-able — it
    // routes to `k::write_file` → `sys_write` (a raw syscall that faults on the
    // host test box; see the linuxkpi-harness memory). It is exercised on-target
    // by the boot smoketest's stdout-capture path. The down-conversion math
    // (UTF-16 → bytes) is what is host-checkable, covered indirectly by the
    // WriteFile capture test in the harness.

    #[test]
    fn critical_section_enter_leave_balances() {
        unsafe {
            let mut cs: u64 = 0xAAAA; // garbage; Initialize must reset it
            shim_initialize_critical_section(&mut cs);
            assert_eq!(cs, 0);
            shim_enter_critical_section(&mut cs);
            shim_enter_critical_section(&mut cs);
            assert_eq!(cs, 2); // recursive acquire tracked
            shim_leave_critical_section(&mut cs);
            assert_eq!(cs, 1);
            shim_leave_critical_section(&mut cs);
            assert_eq!(cs, 0);
            shim_delete_critical_section(&mut cs);
        }
    }

    #[test]
    fn get_startup_info_w_fills_cb() {
        with_fresh_ctx(|| unsafe {
            let mut info: k::StartupInfoW = core::mem::zeroed();
            shim_get_startup_info_w(&mut info);
            assert_eq!(info.cb, core::mem::size_of::<k::StartupInfoW>() as u32);
        });
    }

    #[test]
    fn feature_and_debugger_probes_are_sane() {
        with_fresh_ctx(|| unsafe {
            // PF_XMMI64_INSTRUCTIONS_AVAILABLE (SSE2) is what the CRT checks.
            assert_eq!(shim_is_processor_feature_present(10), 1);
            assert_eq!(shim_is_debugger_present(), 0);
            assert_eq!(shim_get_acp(), 1252);
            assert_eq!(shim_get_console_output_cp(), 437);
            let mut ctr: i64 = 0;
            assert_eq!(shim_query_performance_counter(&mut ctr), 1);
            let mut freq: i64 = 0;
            assert_eq!(shim_query_performance_frequency(&mut freq), 1);
            assert!(freq > 0);
        });
    }

    // ---- 2026-06-30 coverage batch ----

    #[test]
    fn coverage_batch_shims_resolve_distinct_and_via_contract() {
        // Every newly-added name must bind (not a fail-loud stub), and must bind
        // through the API Set contract DLL a real binary imports it by — exactly
        // as through the canonical host module.
        let resolver = ShimResolver::new();
        let cases: &[(&str, &str, &str)] = &[
            ("api-ms-win-crt-string-l1-1-0.dll", "msvcrt.dll", "memset"),
            ("api-ms-win-crt-private-l1-1-0.dll", "msvcrt.dll", "memcpy"),
            ("api-ms-win-crt-private-l1-1-0.dll", "msvcrt.dll", "memmove"),
            ("api-ms-win-crt-private-l1-1-0.dll", "msvcrt.dll", "memcmp"),
            ("api-ms-win-crt-string-l1-1-0.dll", "msvcrt.dll", "wcschr"),
            ("api-ms-win-crt-string-l1-1-0.dll", "msvcrt.dll", "_wcsicmp"),
            (
                "api-ms-win-core-synch-l1-1-0.dll",
                "kernel32.dll",
                "AcquireSRWLockExclusive",
            ),
            (
                "api-ms-win-core-synch-l1-1-0.dll",
                "kernel32.dll",
                "ReleaseSRWLockShared",
            ),
            (
                "api-ms-win-core-synch-l1-1-0.dll",
                "kernel32.dll",
                "CreateMutexExW",
            ),
            (
                "api-ms-win-core-synch-l1-1-0.dll",
                "kernel32.dll",
                "CreateSemaphoreExW",
            ),
            (
                "api-ms-win-core-heap-l2-1-0.dll",
                "kernel32.dll",
                "LocalAlloc",
            ),
            (
                "api-ms-win-core-heap-l2-1-0.dll",
                "kernel32.dll",
                "LocalFree",
            ),
            (
                "api-ms-win-core-debug-l1-1-0.dll",
                "kernel32.dll",
                "OutputDebugStringW",
            ),
            (
                "api-ms-win-core-debug-l1-1-0.dll",
                "kernel32.dll",
                "DebugBreak",
            ),
            (
                "api-ms-win-core-libraryloader-l1-2-0.dll",
                "kernel32.dll",
                "GetModuleFileNameA",
            ),
            (
                "api-ms-win-eventing-provider-l1-1-0.dll",
                "advapi32.dll",
                "EventRegister",
            ),
            (
                "api-ms-win-eventing-provider-l1-1-0.dll",
                "advapi32.dll",
                "EventWriteTransfer",
            ),
        ];
        let mut seen = alloc::vec::Vec::new();
        for (contract, host, func) in cases {
            let via_host = resolve_shim(host, func);
            assert!(via_host.is_some(), "{func} must be bound under {host}");
            assert_eq!(
                resolver.resolve(contract, func),
                via_host,
                "{func} must redirect from {contract} to {host}"
            );
            seen.push(via_host.unwrap());
        }
        // Distinct-address contract: no two shims may alias (the selftest relies
        // on this; aliasing would also mask a copy/paste wiring bug).
        seen.sort_unstable();
        let before = seen.len();
        seen.dedup();
        assert_eq!(
            before,
            seen.len(),
            "coverage-batch shims must be distinct fns"
        );
    }

    #[test]
    fn crt_mem_intrinsics_behave_like_the_c_library() {
        unsafe {
            // memset fills and returns dest.
            let mut buf = [0u8; 8];
            let p = buf.as_mut_ptr() as u64;
            assert_eq!(shim_crt_memset(p, 0xAB, 4), p);
            assert_eq!(buf, [0xAB, 0xAB, 0xAB, 0xAB, 0, 0, 0, 0]);

            // memcpy (non-overlapping) copies and returns dest.
            let src = [1u8, 2, 3, 4, 5, 6, 7, 8];
            let mut dst = [0u8; 8];
            let dp = dst.as_mut_ptr() as u64;
            assert_eq!(shim_crt_memcpy(dp, src.as_ptr() as u64, 8), dp);
            assert_eq!(dst, src);

            // memmove handles forward-overlapping regions (memcpy may not).
            let mut ov = [1u8, 2, 3, 4, 5, 0, 0, 0];
            let base = ov.as_mut_ptr() as u64;
            assert_eq!(shim_crt_memmove(base + 2, base, 4), base + 2);
            assert_eq!(ov, [1, 2, 1, 2, 3, 4, 0, 0]);

            // memcmp sign + equality.
            let a = [1u8, 2, 3];
            let b = [1u8, 2, 4];
            assert_eq!(shim_crt_memcmp(a.as_ptr() as u64, a.as_ptr() as u64, 3), 0);
            assert!(shim_crt_memcmp(a.as_ptr() as u64, b.as_ptr() as u64, 3) < 0);
            assert!(shim_crt_memcmp(b.as_ptr() as u64, a.as_ptr() as u64, 3) > 0);
            // Zero count is always equal (must not deref).
            assert_eq!(shim_crt_memcmp(0, 0, 0), 0);
        }
    }

    #[test]
    fn crt_wide_string_search_and_compare() {
        unsafe {
            let s: [u16; 4] = [b'a' as u16, b'b' as u16, b'c' as u16, 0];
            let sp = s.as_ptr() as u64;
            // Found -> pointer to the matching element.
            assert_eq!(shim_crt_wcschr(sp, b'b' as u32), sp + 2);
            // Not found -> NULL.
            assert_eq!(shim_crt_wcschr(sp, b'z' as u32), 0);
            // NUL is part of the string -> pointer to the terminator.
            assert_eq!(shim_crt_wcschr(sp, 0), sp + 6);

            let upper: [u16; 4] = [b'A' as u16, b'B' as u16, b'C' as u16, 0];
            let lower: [u16; 4] = [b'a' as u16, b'b' as u16, b'c' as u16, 0];
            let diff: [u16; 4] = [b'a' as u16, b'b' as u16, b'd' as u16, 0];
            assert_eq!(
                shim_crt_wcsicmp(upper.as_ptr() as u64, lower.as_ptr() as u64),
                0
            );
            assert!(shim_crt_wcsicmp(lower.as_ptr() as u64, diff.as_ptr() as u64) < 0);
        }
    }

    #[test]
    fn srwlock_tracks_exclusive_and_shared_state() {
        unsafe {
            let mut lock: u64 = 0xDEAD; // garbage; Initialize must clear it
            shim_initialize_srwlock(&mut lock);
            assert_eq!(lock, 0);

            // Exclusive: acquire sets the bit; a contended try fails; release clears.
            shim_acquire_srwlock_exclusive(&mut lock);
            assert_eq!(lock & SRWLOCK_EXCLUSIVE, SRWLOCK_EXCLUSIVE);
            assert_eq!(shim_try_acquire_srwlock_exclusive(&mut lock), 0);
            shim_release_srwlock_exclusive(&mut lock);
            assert_eq!(lock, 0);
            // Uncontended try succeeds.
            assert_eq!(shim_try_acquire_srwlock_exclusive(&mut lock), 1);
            shim_release_srwlock_exclusive(&mut lock);

            // Shared: multiple readers accumulate; an exclusive try fails while
            // held; try-shared still succeeds (readers don't exclude readers).
            shim_acquire_srwlock_shared(&mut lock);
            shim_acquire_srwlock_shared(&mut lock);
            assert_eq!(lock, 2 * SRWLOCK_SHARED_UNIT);
            assert_eq!(shim_try_acquire_srwlock_exclusive(&mut lock), 0);
            assert_eq!(shim_try_acquire_srwlock_shared(&mut lock), 1);
            shim_release_srwlock_shared(&mut lock);
            shim_release_srwlock_shared(&mut lock);
            shim_release_srwlock_shared(&mut lock);
            assert_eq!(lock, 0);
        }
    }

    #[test]
    fn local_heap_and_no_op_success_shims() {
        with_fresh_ctx(|| unsafe {
            // LocalAlloc(LMEM_ZEROINIT) returns zeroed, writable memory.
            let p = shim_local_alloc(LMEM_ZEROINIT, 16);
            assert_ne!(p, 0);
            for i in 0..16u64 {
                assert_eq!((p as *const u8).add(i as usize).read(), 0);
                (p as *mut u8).add(i as usize).write(i as u8);
            }
            // LocalFree returns NULL (0) on success.
            assert_eq!(shim_local_free(p), 0);

            // Advisory / telemetry shims succeed without a backing subsystem.
            assert_eq!(shim_heap_set_information(0, 0, 0, 0), 1);
            let mut reg: u64 = 0xFF;
            assert_eq!(shim_event_register(0, 0, 0, &mut reg), 0);
            assert_eq!(reg, 0);
            assert_eq!(shim_event_write_transfer(reg, 0, 0, 0, 0, 0), 0);
            assert_eq!(shim_event_set_information(reg, 0, 0, 0), 0);
            assert_eq!(shim_event_unregister(reg), 0);

            // GetModuleFileNameA must always null-terminate within bounds.
            let mut namebuf = [0xFFu8; 64];
            let n = shim_get_module_file_name_a(0, namebuf.as_mut_ptr(), namebuf.len() as u32);
            assert!((n as usize) < namebuf.len());
            assert_eq!(namebuf[n as usize], 0);
        });
    }

    #[test]
    fn stdio_format_shims_render_values() {
        unsafe {
            // Variadic sprintf: the win64 ABI puts the first varargs in
            // RDX/R8/R9 and the rest on the stack; the shim reconstructs them.
            let fmt = b"x=%d s=%s\0";
            let s = b"hi\0";
            let mut out = [0u8; 32];
            let n = shim_sprintf(
                out.as_mut_ptr() as u64,
                fmt.as_ptr() as u64,
                7,
                s.as_ptr() as u64,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
            );
            assert_eq!(n, "x=7 s=hi".len() as i32);
            assert_eq!(&out[..8], b"x=7 s=hi");
            assert_eq!(out[8], 0);

            // C99 snprintf: returns the FULL length even when truncated, and
            // NUL-terminates within `count`.
            let mut small = [0u8; 4];
            let n2 = shim_snprintf(
                small.as_mut_ptr() as u64,
                4,
                b"%d\0".as_ptr() as u64,
                12345,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
            );
            assert_eq!(n2, 5); // "12345" would need 5 chars
            assert_eq!(&small, b"123\0");

            // __stdio_common_vsprintf via an explicit va_list array (the UCRT
            // core every /MD sprintf/snprintf funnels through).
            let va: [u64; 1] = [255];
            let mut b2 = [0u8; 16];
            let n3 = shim_stdio_common_vsprintf(
                0,
                b2.as_mut_ptr() as u64,
                16,
                b"%#x\0".as_ptr() as u64,
                0,
                va.as_ptr() as u64,
            );
            assert_eq!(n3, 4); // "0xff"
            assert_eq!(&b2[..4], b"0xff");

            // Wide swprintf renders into a UTF-16 buffer.
            let wfmt: [u16; 3] = [b'%' as u16, b'd' as u16, 0];
            let mut wout = [0u16; 8];
            let nw = shim_swprintf(
                wout.as_mut_ptr() as u64,
                8,
                wfmt.as_ptr() as u64,
                99,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
            );
            assert_eq!(nw, 2);
            assert_eq!(wout[0], b'9' as u16);
            assert_eq!(wout[1], b'9' as u16);
            assert_eq!(wout[2], 0);

            // __acrt_iob_func returns a synthetic non-null stream handle.
            assert_eq!(shim_acrt_iob_func(0), 1);
            assert_eq!(shim_acrt_iob_func(1), 2);
            assert_eq!(shim_acrt_iob_func(2), 3);
        }
    }

    #[test]
    fn crt_string_conversion_and_math_shims() {
        unsafe {
            // strlen / strcmp / _stricmp.
            let hello = b"Hello\0";
            assert_eq!(shim_strlen(hello.as_ptr() as u64), 5);
            assert_eq!(shim_strnlen(hello.as_ptr() as u64, 3), 3);
            let hello2 = b"Hello\0";
            let world = b"World\0";
            assert_eq!(
                shim_strcmp(hello.as_ptr() as u64, hello2.as_ptr() as u64),
                0
            );
            assert!(shim_strcmp(hello.as_ptr() as u64, world.as_ptr() as u64) < 0);
            let upper = b"HELLO\0";
            assert_eq!(
                shim_stricmp(hello.as_ptr() as u64, upper.as_ptr() as u64),
                0
            );

            // strcpy / strcat.
            let mut buf = [0u8; 16];
            shim_strcpy(buf.as_mut_ptr() as u64, b"foo\0".as_ptr() as u64);
            assert_eq!(&buf[..4], b"foo\0");
            shim_strcat(buf.as_mut_ptr() as u64, b"bar\0".as_ptr() as u64);
            assert_eq!(&buf[..7], b"foobar\0");

            // strchr / strrchr / strstr / memchr.
            let s = b"a.b.c\0";
            let sp = s.as_ptr() as u64;
            assert_eq!(shim_strchr(sp, b'.' as i32), sp + 1);
            assert_eq!(shim_strrchr(sp, b'.' as i32), sp + 3);
            assert_eq!(shim_strchr(sp, b'z' as i32), 0);
            assert_eq!(shim_strstr(sp, b"b.c\0".as_ptr() as u64), sp + 2);
            assert_eq!(shim_strstr(sp, b"xy\0".as_ptr() as u64), 0);
            assert_eq!(shim_memchr(sp, b'c' as i32, 5), sp + 4);

            // wide string ops.
            let w: [u16; 6] = [b'w' as u16, b'i' as u16, b'd' as u16, b'e' as u16, 0, 0];
            assert_eq!(shim_wcslen(w.as_ptr() as u64), 4);
            let mut wbuf = [0u16; 8];
            shim_wcscpy(wbuf.as_mut_ptr() as u64, w.as_ptr() as u64);
            assert_eq!(shim_wcslen(wbuf.as_ptr() as u64), 4);
            assert_eq!(shim_wcscmp(wbuf.as_ptr() as u64, w.as_ptr() as u64), 0);

            // char classification.
            assert_eq!(shim_toupper(b'a' as i32), b'A' as i32);
            assert_eq!(shim_tolower(b'Z' as i32), b'z' as i32);
            assert_eq!(shim_isdigit(b'7' as i32), 1);
            assert_eq!(shim_isdigit(b'x' as i32), 0);
            assert_eq!(shim_isspace(b' ' as i32), 1);
            assert_eq!(shim_isalpha(b'Q' as i32), 1);

            // conversions.
            assert_eq!(shim_atoi(b"-42\0".as_ptr() as u64), -42);
            assert_eq!(
                shim_atoi64(b"10000000000\0".as_ptr() as u64),
                10_000_000_000
            );
            let num = b"7fF\0";
            let mut endptr: u64 = 0;
            let v = shim_strtol(num.as_ptr() as u64, &mut endptr as *mut u64 as u64, 16);
            assert_eq!(v, 0x7FF);
            assert_eq!(endptr, num.as_ptr() as u64 + 3);
            let d = shim_atof(b"3.5\0".as_ptr() as u64);
            assert!((d - 3.5).abs() < 1e-9);

            // math (XMM0 in/out).
            assert!((shim_sqrt(16.0) - 4.0).abs() < 1e-9);
            assert!((shim_pow(2.0, 10.0) - 1024.0).abs() < 1e-9);
            assert_eq!(shim_floor(3.9), 3.0);
            assert_eq!(shim_ceil(3.1), 4.0);
            assert_eq!(shim_fabs(-2.5), 2.5);
        }
    }

    #[test]
    fn stdio_format_shims_bind_through_crt_contract() {
        // Real /MD binaries import these from the api-ms-win-crt-stdio contract
        // DLL; they must redirect to our msvcrt host module and bind (not stub).
        let resolver = ShimResolver::new();
        for func in [
            "__stdio_common_vsprintf",
            "__stdio_common_vswprintf",
            "sprintf",
            "snprintf",
            "printf",
            "puts",
            "fwrite",
        ] {
            let via_host = resolve_shim("msvcrt.dll", func);
            assert!(via_host.is_some(), "{func} must be bound");
            assert_eq!(
                via_host,
                resolver.resolve("api-ms-win-crt-stdio-l1-1-0.dll", func)
            );
        }
    }

    #[test]
    fn crt_heap_and_startup_shims() {
        unsafe {
            let p = shim_malloc(64);
            assert_ne!(p, 0);
            shim_free(p);

            let c = shim_calloc(4, 8);
            assert_ne!(c, 0);
            let r = shim_realloc(c, 128);
            assert_ne!(r, 0);
            shim_free(r);

            let mut buf = [0u16; 32];
            let fmt: [u16; 5] = [b'x' as u16, b'=' as u16, b'%' as u16, b'd' as u16, 0];
            let mut va: [u64; 2] = [42, 0];
            let n = shim_vsnwprintf(
                buf.as_mut_ptr() as u64,
                buf.len() as u64,
                fmt.as_ptr() as u64,
                va.as_mut_ptr() as u64,
            );
            assert!(n > 0);
            assert_eq!(buf[0], b'x' as u16);

            let mut argc = 0i32;
            let mut argv = 0u64;
            let mut envp = 0u64;
            assert_eq!(
                shim_getmainargs(
                    (&mut argc as *mut i32) as u64,
                    (&mut argv as *mut u64) as u64,
                    (&mut envp as *mut u64) as u64,
                    0,
                    0,
                ),
                0
            );
            assert_eq!(argc, 1);

            assert_eq!(
                resolve_shim("msvcrt.dll", "malloc").unwrap(),
                shim_malloc as u64
            );
            assert_eq!(
                resolve_shim("msvcrt.dll", "_o_free").unwrap(),
                shim_o_free as u64
            );
        }
    }
}
