//! raelangd — RaeenOS userspace Rae-script interpreter daemon.
//!
//! Concept §Customization Engine: "Scripting layer — Swift scripts for
//! automation, no PowerShell archaeology required." The kernel runs
//! sources ≤64 KiB inline at submit; anything larger is queued, and THIS
//! daemon is what drains that queue — one language, one implementation
//! (`components/raelang`), two execution sites.
//!
//! Protocol (docs/SYSCALL_TABLE.md Block 14):
//! 1. `SYS_SCRIPT_FETCH` (294) claims the next queued job — the kernel
//!    hands over `ScriptJobAbi { id, cap_mask, source_len }` + the source
//!    and flips the script Queued → Running.
//! 2. The shared interpreter runs it fuel-limited under a [`DaemonHost`]
//!    gated on the SAME `SCRIPT_CAP_*` bits the kernel host enforces.
//! 3. `SYS_SCRIPT_COMPLETE` (295) reports exit code + captured output
//!    (negative exit = Failed).
//!
//! Memory: a fixed 8 MiB bump arena, RESET BETWEEN JOBS — nothing a
//! script allocates outlives its run (output is copied to a static buffer
//! before the reset), so the daemon's footprint is flat no matter how
//! many scripts it executes. Deterministic like the fuel limit.
//!
//! Serial sentinels: 8900 daemon up · 8901+(id&7) job completed ·
//! 8909 job failed · 8999 panic.

#![no_std]
#![no_main]

extern crate alloc;

use core::alloc::{GlobalAlloc, Layout};
use core::panic::PanicInfo;
use core::sync::atomic::{AtomicUsize, Ordering};
use rae_abi::syscall as abi;

const _: () = assert!(rae_abi::ABI_VERSION == 4);

// ── Bump arena, reset between jobs ─────────────────────────────────────

const ARENA_SIZE: usize = 8 * 1024 * 1024;
static mut ARENA: [u8; ARENA_SIZE] = [0; ARENA_SIZE];
static ARENA_OFF: AtomicUsize = AtomicUsize::new(0);

struct Arena;

unsafe impl GlobalAlloc for Arena {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let align = layout.align().max(8);
        let size = layout.size().max(1);
        let mut claimed = usize::MAX;
        let _ = ARENA_OFF.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |off| {
            let start = (off + align - 1) & !(align - 1);
            let end = start.checked_add(size)?;
            if end > ARENA_SIZE {
                return None;
            }
            claimed = start;
            Some(end)
        });
        if claimed == usize::MAX {
            return core::ptr::null_mut();
        }
        (core::ptr::addr_of_mut!(ARENA) as *mut u8).add(claimed)
    }
    unsafe fn dealloc(&self, _ptr: *mut u8, _layout: Layout) {
        // Arena allocator: freed en masse by `arena_reset` between jobs.
    }
}

#[global_allocator]
static ALLOCATOR: Arena = Arena;

/// Everything allocated during a job is dead once its output has been
/// copied out — reclaim the whole arena in O(1).
fn arena_reset() {
    ARENA_OFF.store(0, Ordering::SeqCst);
}

// ── Raw syscalls ───────────────────────────────────────────────────────

#[inline(always)]
unsafe fn syscall3(nr: u64, a: u64, b: u64, c: u64) -> u64 {
    let ret: u64;
    core::arch::asm!(
        "syscall",
        inlateout("rax") nr => ret,
        in("rdi") a, in("rsi") b, in("rdx") c,
        out("rcx") _, out("r11") _,
    );
    ret
}

#[inline(always)]
unsafe fn syscall4(nr: u64, a: u64, b: u64, c: u64, d: u64) -> u64 {
    let ret: u64;
    core::arch::asm!(
        "syscall",
        inlateout("rax") nr => ret,
        in("rdi") a, in("rsi") b, in("rdx") c, in("r10") d,
        out("rcx") _, out("r11") _,
    );
    ret
}

unsafe fn sys_print(value: u64) {
    let _ = syscall3(abi::SYS_PRINT, value, 0, 0);
}

unsafe fn sys_exit(code: u64) -> ! {
    core::arch::asm!(
        "syscall",
        in("rax") abi::SYS_EXIT,
        in("rdi") code,
        options(noreturn),
    );
}

/// SYS_LINUXKPI_MSLEEP — the shared blocking-sleep syscall daemons use
/// between poll rounds (keeps the queue drain off the hot path).
unsafe fn msleep(ms: u64) {
    let _ = syscall3(abi::SYS_LINUXKPI_MSLEEP, ms, 0, 0);
}

fn klog(msg: &str) {
    unsafe {
        let _ = syscall3(
            abi::SYS_DEBUG_PRINT,
            msg.as_ptr() as u64,
            msg.len() as u64,
            0,
        );
    }
}

// ── Job buffers (static — survive arena resets) ────────────────────────

/// `ScriptJobAbi` header (32 bytes) + up to the kernel's 1 MiB source cap.
const JOB_BUF_SIZE: usize = 32 + 1024 * 1024;
static mut JOB_BUF: [u8; JOB_BUF_SIZE] = [0; JOB_BUF_SIZE];
/// Kernel truncates output at 4 KiB — same cap here.
static mut OUT_BUF: [u8; 4096] = [0; 4096];

/// Fuel per daemon job. Bigger than the kernel's inline budget (1M) —
/// big scripts are the whole point of this path — but still finite: a
/// runaway still dies deterministically.
const DAEMON_FUEL: u64 = 10_000_000;

fn read_u64(buf: &[u8], off: usize) -> u64 {
    let mut b = [0u8; 8];
    b.copy_from_slice(&buf[off..off + 8]);
    u64::from_le_bytes(b)
}

// ── Host bindings (daemon side) ────────────────────────────────────────

/// The daemon's [`raelang::Host`], gated on the SAME `SCRIPT_CAP_*` bits
/// as the kernel host. Bindings that have a userspace syscall route work
/// here; the rest honestly report they're inline-only (they need kernel
/// entry points that don't exist as syscalls yet) instead of no-opping.
struct DaemonHost {
    cap_mask: u64,
}

impl DaemonHost {
    fn require(&self, bit: u64, name: &str) -> Result<(), raelang::HostError> {
        if self.cap_mask & bit != 0 {
            Ok(())
        } else {
            Err(raelang::HostError::Denied(alloc::format!(
                "{}: capability not granted (cap_mask=0x{:x})",
                name,
                self.cap_mask
            )))
        }
    }
}

/// SYS_TIME (30) — monotonic ns since boot (docs/SYSCALL_TABLE.md; no
/// rae_abi constant, same as the clock app's direct use of 40).
const SYS_TIME: u64 = 30;
/// SYS_CONFIG_GET/SET (50/51) — the versioned config registry.
const SYS_CONFIG_GET: u64 = 50;
const SYS_CONFIG_SET: u64 = 51;

impl raelang::Host for DaemonHost {
    fn call(
        &mut self,
        name: &str,
        args: &[raelang::Value],
    ) -> Result<raelang::Value, raelang::HostError> {
        use raelang::{HostError, Value};
        match name {
            "uptimeMs" => {
                self.require(abi::SCRIPT_CAP_SYSINFO, name)?;
                let ns = unsafe { syscall3(SYS_TIME, 0, 0, 0) };
                Ok(Value::Int((ns / 1_000_000) as i64))
            }
            "getConfig" => {
                self.require(abi::SCRIPT_CAP_CONFIG, name)?;
                let key = match args {
                    [Value::Str(k)] => k,
                    _ => {
                        return Err(HostError::Failed(alloc::format!(
                            "{} takes one String argument",
                            name
                        )))
                    }
                };
                let mut out = [0u8; 512];
                let n = unsafe {
                    syscall4(
                        SYS_CONFIG_GET,
                        key.as_ptr() as u64,
                        key.len() as u64,
                        out.as_mut_ptr() as u64,
                        out.len() as u64,
                    )
                };
                if n == 0 || n > out.len() as u64 {
                    return Ok(Value::Unit);
                }
                match core::str::from_utf8(&out[..n as usize]) {
                    Ok(s) => Ok(Value::Str(alloc::string::String::from(s))),
                    Err(_) => Ok(Value::Unit),
                }
            }
            "setConfig" => {
                self.require(abi::SCRIPT_CAP_CONFIG, name)?;
                match args {
                    [Value::Str(k), Value::Str(v)] => {
                        let _ = unsafe {
                            syscall4(
                                SYS_CONFIG_SET,
                                k.as_ptr() as u64,
                                k.len() as u64,
                                v.as_ptr() as u64,
                                v.len() as u64,
                            )
                        };
                        Ok(Value::Unit)
                    }
                    _ => Err(HostError::Failed(alloc::string::String::from(
                        "setConfig takes (String key, String value)",
                    ))),
                }
            }
            // Bindings whose kernel entry points have no syscall route yet:
            // fail honestly rather than silently no-op.
            "notify" | "getAccent" | "setAccent" | "setWallpaper" | "launchApp" | "wallClock"
            | "windowCount" | "osVersion" => Err(HostError::Failed(alloc::format!(
                "{}: inline-only binding (submit scripts under 64 KiB to use it)",
                name
            ))),
            _ => Err(HostError::Unknown),
        }
    }
}

// ── Job execution ──────────────────────────────────────────────────────

fn run_job(id: u64, cap_mask: u64, src: &[u8]) {
    let mut host = DaemonHost { cap_mask };
    let result = core::str::from_utf8(src)
        .map_err(|_| raelang::RaeError::Lex(alloc::string::String::from("source is not UTF-8")))
        .and_then(|s| raelang::run_with_host(s, DAEMON_FUEL, &mut host));

    // Copy the outcome into STATIC buffers before the arena reset frees
    // everything the interpreter allocated.
    let (exit_code, out_len) = unsafe {
        let out = &mut *core::ptr::addr_of_mut!(OUT_BUF);
        match result {
            Ok(outcome) => {
                let bytes = outcome.output.as_bytes();
                let n = bytes.len().min(out.len());
                out[..n].copy_from_slice(&bytes[..n]);
                (outcome.exit_code, n)
            }
            Err(err) => {
                let msg = alloc::format!("error: {:?}", err);
                let bytes = msg.as_bytes();
                let n = bytes.len().min(out.len());
                out[..n].copy_from_slice(&bytes[..n]);
                (-1i64, n)
            }
        }
    };

    let rc = unsafe {
        syscall4(
            abi::SYS_SCRIPT_COMPLETE,
            id,
            exit_code as u64,
            core::ptr::addr_of!(OUT_BUF) as u64,
            out_len as u64,
        )
    };
    unsafe {
        if rc == 0 && exit_code >= 0 {
            sys_print(8901 + (id & 7));
        } else {
            sys_print(8909);
        }
    }
    arena_reset();
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    unsafe { sys_print(8900) };
    klog("[raelangd] up: draining queued Rae scripts (fetch/run/complete)");

    loop {
        let fetched = unsafe {
            syscall3(
                abi::SYS_SCRIPT_FETCH,
                core::ptr::addr_of_mut!(JOB_BUF) as u64,
                JOB_BUF_SIZE as u64,
                0,
            )
        };
        if fetched == 0 {
            // Queue empty — sleep off the hot path and poll again.
            unsafe { msleep(100) };
            continue;
        }
        if fetched > JOB_BUF_SIZE as u64 {
            // ERR_* sentinel (high bits set) — engine not up yet, or a
            // transient claim failure. Back off harder.
            unsafe { msleep(500) };
            continue;
        }
        let (id, cap_mask, src_len) = unsafe {
            let buf = &*core::ptr::addr_of!(JOB_BUF);
            (
                read_u64(&buf[..], 8),
                read_u64(&buf[..], 16),
                read_u64(&buf[..], 24) as usize,
            )
        };
        if 32 + src_len > fetched as usize {
            klog("[raelangd] short job record — dropping");
            unsafe { sys_print(8909) };
            continue;
        }
        let src = unsafe { &(&*core::ptr::addr_of!(JOB_BUF))[32..32 + src_len] };
        run_job(id, cap_mask, src);
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    unsafe {
        sys_print(8999);
        sys_exit(99);
    }
}
