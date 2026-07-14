//! Linux busy-delay + monotonic-time primitives.
//!
//! `udelay`/`ndelay`/`mdelay` are busy-wait spins (the driver is mid-init and
//! must not sleep). We calibrate against the TSC via `rdtsc`; without a measured
//! frequency we fall back to a bounded `pause`-loop that is never shorter than
//! requested. `usleep_range` and longer sleeps route to the host `msleep`.
//! `ktime_get_ns` derives nanoseconds from the host jiffies clock.

use crate::host;

/// Assumed TSC frequency in MHz when calibration is unavailable. Conservative
/// (low) so a spin is never *shorter* than asked — a too-long delay is safe,
/// a too-short one races the hardware.
const ASSUMED_TSC_MHZ: u64 = 1000;

#[inline(always)]
fn rdtsc() -> u64 {
    let lo: u32;
    let hi: u32;
    unsafe {
        core::arch::asm!("rdtsc", out("eax") lo, out("edx") hi, options(nomem, nostack, preserves_flags));
    }
    ((hi as u64) << 32) | (lo as u64)
}

/// Busy-spin for at least `cycles` TSC ticks.
#[inline]
fn spin_cycles(cycles: u64) {
    let start = rdtsc();
    while rdtsc().wrapping_sub(start) < cycles {
        core::hint::spin_loop();
    }
}

#[no_mangle]
pub extern "C" fn udelay(usecs: u64) {
    spin_cycles(usecs.saturating_mul(ASSUMED_TSC_MHZ));
}

#[no_mangle]
pub extern "C" fn ndelay(nsecs: u64) {
    // cycles = ns * MHz / 1000
    spin_cycles((nsecs.saturating_mul(ASSUMED_TSC_MHZ)) / 1000 + 1);
}

#[no_mangle]
pub extern "C" fn mdelay(msecs: u64) {
    for _ in 0..msecs {
        udelay(1000);
    }
}

/// `usleep_range(min, max)` — sleepable wait. We honor the lower bound via the
/// host millisecond sleep (rounding up so we never undersleep).
#[no_mangle]
pub extern "C" fn usleep_range(min_us: u64, _max_us: u64) {
    let ms = (min_us + 999) / 1000;
    unsafe { host::sys_linuxkpi_msleep(ms.max(1)) };
}

/// `msleep` is also exported from lib.rs; `msleep_interruptible` aliases it.
#[no_mangle]
pub extern "C" fn msleep_interruptible(msecs: u32) -> u32 {
    unsafe { host::sys_linuxkpi_msleep(msecs as u64) };
    0
}

/// Nanoseconds since boot, derived from the host jiffies clock (1 kHz → 1 ms
/// granularity). Good enough for driver timeout math; hot-path timestamping
/// can read the TSC directly.
#[no_mangle]
pub extern "C" fn ktime_get_ns() -> u64 {
    let jiffies = unsafe { host::sys_linuxkpi_jiffies() };
    jiffies.saturating_mul(1_000_000) // 1 jiffy = 1 ms = 1e6 ns
}

#[no_mangle]
pub extern "C" fn ktime_get() -> u64 {
    ktime_get_ns()
}

#[no_mangle]
pub extern "C" fn ktime_get_real_ns() -> u64 {
    ktime_get_ns()
}

/// `get_jiffies_64` is exported from lib.rs; `jiffies_to_msecs` is pure math
/// (host jiffies are already 1 kHz, so 1 jiffy == 1 ms).
#[no_mangle]
pub extern "C" fn jiffies_to_msecs(j: u64) -> u32 {
    j as u32
}
#[no_mangle]
pub extern "C" fn msecs_to_jiffies(m: u32) -> u64 {
    m as u64
}
/// 1 jiffy = 1 ms = 1000 us on the host clock.
#[no_mangle]
pub extern "C" fn jiffies_to_usecs(j: u64) -> u32 {
    j.saturating_mul(1000) as u32
}
#[no_mangle]
pub extern "C" fn usecs_to_jiffies(u: u32) -> u64 {
    (u as u64).div_ceil(1000)
}

// ── ktime_get_* family — all derive from the host monotonic (jiffies) clock ──
// The host has one coherent monotonic source; "raw"/"mono_fast"/"with_offset"
// distinctions collapse onto it (documented). Real/wall-clock has no epoch in
// the daemon, so "real" is boot-relative — fine for driver timeout/delta math.

#[no_mangle]
pub extern "C" fn ktime_get_raw() -> u64 {
    ktime_get_ns()
}
#[no_mangle]
pub extern "C" fn ktime_get_mono_fast_ns() -> u64 {
    ktime_get_ns()
}
#[no_mangle]
pub extern "C" fn ktime_get_boottime_ns() -> u64 {
    ktime_get_ns()
}
#[no_mangle]
pub extern "C" fn ktime_get_with_offset(_offs: i32) -> u64 {
    ktime_get_ns()
}
#[no_mangle]
pub extern "C" fn ktime_get_real_seconds() -> i64 {
    (ktime_get_ns() / 1_000_000_000) as i64
}
#[no_mangle]
pub extern "C" fn ktime_get_seconds() -> i64 {
    ktime_get_real_seconds()
}

/// `struct timespec64 { time64_t tv_sec; long tv_nsec; }`.
#[repr(C)]
pub struct Timespec64 {
    pub tv_sec: i64,
    pub tv_nsec: i64,
}

/// `ktime_get_ts64(ts)` — split the monotonic nanosecond clock into sec/nsec.
#[no_mangle]
pub extern "C" fn ktime_get_ts64(ts: *mut Timespec64) {
    if ts.is_null() {
        return;
    }
    let ns = ktime_get_ns();
    unsafe {
        (*ts).tv_sec = (ns / 1_000_000_000) as i64;
        (*ts).tv_nsec = (ns % 1_000_000_000) as i64;
    }
}
#[no_mangle]
pub extern "C" fn ktime_get_real_ts64(ts: *mut Timespec64) {
    ktime_get_ts64(ts);
}

/// `usleep_range_state(min, max, state)` — state is irrelevant to the host sleep.
#[no_mangle]
pub extern "C" fn usleep_range_state(min_us: u64, max_us: u64, _state: u32) {
    usleep_range(min_us, max_us);
}
#[no_mangle]
pub extern "C" fn fsleep(usecs: u64) {
    usleep_range(usecs, usecs);
}

/// `__udelay(usecs)` — the runtime (non-constant) udelay entry the kernel's
/// `udelay()` macro dispatches to; same spin as [`udelay`].
#[no_mangle]
pub extern "C" fn __udelay(usecs: u64) {
    udelay(usecs);
}

/// `__const_udelay(xloops)` — the constant-fold udelay entry. The `udelay(n)`
/// macro passes `n * 0x10c7` (its loops-per-usec scale); recover usecs and spin.
#[no_mangle]
pub extern "C" fn __const_udelay(xloops: u64) {
    udelay(xloops / 0x10c7);
}

/// `jiffies64_to_msecs` — host jiffies are already 1 kHz (1 jiffy == 1 ms).
#[no_mangle]
pub extern "C" fn jiffies64_to_msecs(j: u64) -> u32 {
    j as u32
}
