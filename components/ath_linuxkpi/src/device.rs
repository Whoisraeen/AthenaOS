//! Linux `printk` / `dev_*` logging — with `%`-specifier interpolation.
//!
//! Linux's logging functions are C varargs (`printk(fmt, ...)`). Using the
//! nightly `c_variadic` feature we read the trailing args (x86-64 SysV) and
//! interpolate the conversions drivers actually use — `%d %i %u %x %X %o %p %s
//! %c %%`, the `l`/`ll`/`z`/`t` length modifiers, plus width + `0`-fill — so
//! `dev_info("... %d", n)` shows the *value*, not the format string. Unknown
//! specifiers are emitted literally, so nothing is silently dropped.
//!
//! The interpolation ([`format_into`]) is kept independent of the vararg ABI so
//! it is host-testable with synthetic args (see `tools/linuxkpi_harness`).

use crate::host;
#[cfg(not(feature = "hosttest"))]
use core::sync::atomic::AtomicU64;
use core::sync::atomic::{AtomicBool, Ordering};

const BUF: usize = 512;

/// Diagnostic printk→netlog fence. When armed, the printk facade broadcasts the
/// kernel bootlog ring (`SYS_NETLOG_FLUSH`) after each log line — throttled — so
/// the netlog trail ends within `FENCE_THROTTLE_NS` of the exact line before a
/// CPU-0 hard hang in the real amdgpu init (a single monolithic C call whose only
/// interleave point is this facade). amdgpud arms it right before entering
/// `amdgpu_device_init`. Off by default: one relaxed load per printk when idle.
static NETLOG_FENCE: AtomicBool = AtomicBool::new(false);
#[cfg(not(feature = "hosttest"))]
static FENCE_LAST_NS: AtomicU64 = AtomicU64::new(0);
#[cfg(not(feature = "hosttest"))]
const FENCE_THROTTLE_NS: u64 = 40_000_000; // 40 ms — bounds the LAN flood on a deep init

/// Arm/disarm the printk→netlog diagnostic fence. Called from amdgpud around the
/// real `amdgpu_device_init` call.
pub fn set_netlog_fence(on: bool) {
    NETLOG_FENCE.store(on, Ordering::Relaxed);
}

/// Per-printk hook: when the fence is armed and the throttle has elapsed,
/// broadcast the bootlog ring so the last line before a hang reaches the wire.
#[inline]
fn netlog_fence_tick() {
    if !NETLOG_FENCE.load(Ordering::Relaxed) {
        return;
    }
    #[cfg(not(feature = "hosttest"))]
    {
        let now = crate::delay::ktime_get_ns();
        let last = FENCE_LAST_NS.load(Ordering::Relaxed);
        if now.wrapping_sub(last) >= FENCE_THROTTLE_NS {
            FENCE_LAST_NS.store(now, Ordering::Relaxed);
            unsafe {
                host::sys_netlog_flush();
            }
        }
    }
}

struct Writer<'a> {
    buf: &'a mut [u8],
    n: usize,
}
impl Writer<'_> {
    #[inline]
    fn put(&mut self, c: u8) {
        if self.n < self.buf.len() {
            self.buf[self.n] = c;
            self.n += 1;
        }
    }
    fn puts(&mut self, s: &[u8]) {
        for &c in s {
            self.put(c);
        }
    }
    fn pad(&mut self, n: usize, c: u8) {
        for _ in 0..n {
            self.put(c);
        }
    }
}

/// Length of a NUL-terminated C string, capped at `max`.
fn measure(s: *const u8, max: usize) -> usize {
    if s.is_null() {
        return 0;
    }
    let mut n = 0;
    while n < max && unsafe { *s.add(n) } != 0 {
        n += 1;
    }
    n
}

/// Emit `val` in `base`, optional sign, right-justified to `width` with space-
/// or zero-fill.
fn put_uint(w: &mut Writer, val: u64, base: u64, upper: bool, neg: bool, width: usize, zero: bool) {
    let mut tmp = [0u8; 24];
    let mut i = 0;
    let mut v = val;
    if v == 0 {
        tmp[0] = b'0';
        i = 1;
    }
    while v > 0 {
        let d = (v % base) as u8;
        tmp[i] = if d < 10 {
            b'0' + d
        } else if upper {
            b'A' + (d - 10)
        } else {
            b'a' + (d - 10)
        };
        i += 1;
        v /= base;
    }
    let body = i + usize::from(neg);
    if width > body {
        if zero {
            if neg {
                w.put(b'-');
            }
            w.pad(width - body, b'0');
        } else {
            w.pad(width - body, b' ');
            if neg {
                w.put(b'-');
            }
        }
    } else if neg {
        w.put(b'-');
    }
    while i > 0 {
        i -= 1;
        w.put(tmp[i]);
    }
}

/// Interpolate `fmt` into `out`, pulling each argument via `next` (returning the
/// next vararg's bits as u64; `true` = read a 64-bit slot, `false` = 32-bit).
/// Kept free of the C vararg ABI so it is host-testable with synthetic args.
/// Returns the number of bytes written (truncated at `out.len()`).
pub fn format_into(out: &mut [u8], fmt: &[u8], mut next: impl FnMut(bool) -> u64) -> usize {
    let mut w = Writer { buf: out, n: 0 };
    let mut i = 0usize;
    // Strip a leading KERN_* loglevel prefix (SOH 0x01 + level char).
    if fmt.first() == Some(&0x01) {
        i = 2.min(fmt.len());
    }
    while i < fmt.len() {
        let c = fmt[i];
        i += 1;
        if c != b'%' {
            w.put(c);
            continue;
        }
        // Flags.
        let mut zero = false;
        loop {
            match fmt.get(i) {
                Some(b'0') => {
                    zero = true;
                    i += 1;
                }
                Some(b'-') | Some(b'+') | Some(b' ') | Some(b'#') => i += 1,
                _ => break,
            }
        }
        // Width.
        let mut width = 0usize;
        while let Some(&d) = fmt.get(i) {
            if d.is_ascii_digit() {
                width = width * 10 + (d - b'0') as usize;
                i += 1;
            } else {
                break;
            }
        }
        // Precision (parsed, not applied).
        if fmt.get(i) == Some(&b'.') {
            i += 1;
            while fmt.get(i).is_some_and(u8::is_ascii_digit) {
                i += 1;
            }
        }
        // Length modifier → 64-bit slot for l/ll/z/t/q (LP64).
        let mut wide = false;
        match fmt.get(i) {
            Some(b'l') => {
                i += 1;
                if fmt.get(i) == Some(&b'l') {
                    i += 1;
                }
                wide = true;
            }
            Some(b'z') | Some(b'Z') | Some(b't') | Some(b'q') => {
                i += 1;
                wide = true;
            }
            Some(b'h') => {
                i += 1;
                if fmt.get(i) == Some(&b'h') {
                    i += 1;
                }
            }
            _ => {}
        }
        let conv = match fmt.get(i) {
            Some(&c) => {
                i += 1;
                c
            }
            None => {
                w.put(b'%');
                break;
            }
        };
        match conv {
            b'd' | b'i' => {
                let raw = next(wide);
                let v = if wide {
                    raw as i64
                } else {
                    raw as u32 as i32 as i64
                };
                put_uint(&mut w, v.unsigned_abs(), 10, false, v < 0, width, zero);
            }
            b'u' => {
                let v = if wide {
                    next(true)
                } else {
                    next(false) as u32 as u64
                };
                put_uint(&mut w, v, 10, false, false, width, zero);
            }
            b'x' | b'X' => {
                let v = if wide {
                    next(true)
                } else {
                    next(false) as u32 as u64
                };
                put_uint(&mut w, v, 16, conv == b'X', false, width, zero);
            }
            b'o' => {
                let v = if wide {
                    next(true)
                } else {
                    next(false) as u32 as u64
                };
                put_uint(&mut w, v, 8, false, false, width, zero);
            }
            b'p' => {
                w.puts(b"0x");
                put_uint(&mut w, next(true), 16, false, false, 0, false);
            }
            b'c' => w.put(next(false) as u8),
            b's' => {
                let sp = next(true) as *const u8;
                let addr = sp as usize;
                if sp.is_null() {
                    w.puts(b"(null)");
                } else if addr < 0x1000 || addr >= 0x0000_8000_0000_0000 {
                    // Near-null or non-canonical / kernel-half pointer — a bad %s arg
                    // (e.g. a stubbed struct's garbage name field). Walking it would
                    // #GP/#PF; emit a marker instead. Mirrors Linux vsnprintf's
                    // check_pointer sanity guard.
                    w.puts(b"(bad)");
                } else {
                    let n = measure(sp, 1024);
                    for k in 0..n {
                        w.put(unsafe { *sp.add(k) });
                    }
                }
            }
            b'%' => w.put(b'%'),
            other => {
                // Unknown specifier — emit it literally rather than drop it.
                w.put(b'%');
                w.put(other);
            }
        }
    }
    w.n
}

/// Format + emit one variadic printk-family call. The closure that reads the
/// vararg slots stays local to each `extern "C"` entry (so the `VaListImpl` is
/// the caller's), while [`format_into`] does the shared interpolation.
macro_rules! emit_va {
    ($fmt:expr, $args:ident) => {{
        let n = measure($fmt, BUF);
        let slice: &[u8] = if $fmt.is_null() {
            &[]
        } else {
            unsafe { core::slice::from_raw_parts($fmt, n) }
        };
        let mut out = [0u8; BUF];
        let len = format_into(&mut out, slice, |w64| unsafe {
            if w64 {
                $args.next_arg::<u64>()
            } else {
                $args.next_arg::<u32>() as u64
            }
        });
        let _ = unsafe { host::sys_linuxkpi_printk(out.as_ptr(), len as u64) };
        netlog_fence_tick();
    }};
}

/// `printk` (older) + `_printk` (5.15+) — same entry.
#[no_mangle]
pub unsafe extern "C" fn printk(fmt: *const u8, mut args: ...) -> i32 {
    emit_va!(fmt, args);
    0
}
#[no_mangle]
pub unsafe extern "C" fn _printk(fmt: *const u8, mut args: ...) -> i32 {
    emit_va!(fmt, args);
    0
}

/// `dev_printk(level, dev, fmt, ...)` and the per-level helpers the
/// `dev_err`/`dev_warn`/… macros expand to.
#[no_mangle]
pub unsafe extern "C" fn dev_printk(_level: *const u8, _dev: u64, fmt: *const u8, mut args: ...) {
    emit_va!(fmt, args);
}
#[no_mangle]
pub unsafe extern "C" fn _dev_err(_dev: u64, fmt: *const u8, mut args: ...) {
    emit_va!(fmt, args);
}
#[no_mangle]
pub unsafe extern "C" fn _dev_warn(_dev: u64, fmt: *const u8, mut args: ...) {
    emit_va!(fmt, args);
}
#[no_mangle]
pub unsafe extern "C" fn _dev_info(_dev: u64, fmt: *const u8, mut args: ...) {
    emit_va!(fmt, args);
}
#[no_mangle]
pub unsafe extern "C" fn _dev_notice(_dev: u64, fmt: *const u8, mut args: ...) {
    emit_va!(fmt, args);
}
#[no_mangle]
pub unsafe extern "C" fn dev_err(_dev: u64, fmt: *const u8, mut args: ...) {
    emit_va!(fmt, args);
}
#[no_mangle]
pub unsafe extern "C" fn dev_warn(_dev: u64, fmt: *const u8, mut args: ...) {
    emit_va!(fmt, args);
}
#[no_mangle]
pub unsafe extern "C" fn dev_info(_dev: u64, fmt: *const u8, mut args: ...) {
    emit_va!(fmt, args);
}
#[no_mangle]
pub unsafe extern "C" fn dev_dbg(_dev: u64, fmt: *const u8, mut args: ...) {
    emit_va!(fmt, args);
}

/// `__warn_printk` / WARN helpers — emit and continue (a real BUG_ON aborts the
/// daemon; the supervisor restarts it).
#[no_mangle]
pub unsafe extern "C" fn __warn_printk(fmt: *const u8, mut args: ...) {
    emit_va!(fmt, args);
}

/// `dump_stack` — best-effort marker (no unwinder in the daemon yet).
#[no_mangle]
pub extern "C" fn dump_stack() {
    let msg = b"[linuxkpi] dump_stack (no daemon unwinder)\n";
    unsafe { host::sys_linuxkpi_printk(msg.as_ptr(), msg.len() as u64) };
}
