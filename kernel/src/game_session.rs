//! Gaming-first kernel syscall backend.
//!
//! RaeenOS_Concept.md calls out a handful of capabilities that no shipping
//! desktop OS exposes coherently:
//!
//!   * `SCHED_GAME` priority class with hard deadlines.
//!   * Background-process throttling when a game is foreground.
//!   * `NULL_LATENCY` mode — pin a game to dedicated cores, route IRQs off
//!     them, lock CPU frequency at max. (Concept §Pro Gaming.)
//!   * Memory pinning API — guaranteed-resident pages for hot data.
//!     (Concept §Performance.)
//!   * Compositor frame-pacing telemetry — feeds the Game Bar overlay.
//!     (Concept §Game Bar that doesn't suck.)
//!
//! Every primitive is already implemented elsewhere in the kernel
//! (`scheduler::enter_game_mode`, `memory::pin_memory`,
//! `scheduler::deadline_stats`, …). This module is the thin glue that turns
//! them into a coherent userspace ABI plus a few telemetry helpers.
//!
//! ## Syscall numbers
//!
//! | num | name                | rdi/rsi/rdx                                    | rax return |
//! |-----|---------------------|------------------------------------------------|-----------|
//! | 40  | WALL_CLOCK          | —                                              | unix nanoseconds |
//! | 41  | GAME_MODE_ENTER     | —                                              | 0 ok / E_DENIED |
//! | 42  | GAME_MODE_EXIT      | —                                              | 0 ok |
//! | 43  | GAME_MODE_STATUS    | —                                              | bit0=active, bits[31:8]=throttle ratio |
//! | 44  | NULL_LATENCY_ENTER  | rdi = task id (0 = self)                       | 0 ok / E_DENIED |
//! | 45  | NULL_LATENCY_EXIT   | —                                              | 0 ok |
//! | 46  | PIN_MEMORY          | rdi = virt addr, rsi = byte len                | 0 ok / err |
//! | 47  | UNPIN_MEMORY        | rdi = virt addr, rsi = byte len                | 0 ok / err |
//! | 48  | DEADLINE_STATS      | rdi = user buffer ptr (≥ 32 bytes)             | bytes written |
//! | 49  | PERF_TSC            | —                                              | raw TSC cycles |
//!
//! Error codes use the existing `capability::E_*` constants where they
//! semantically fit (`E_DENIED`, `E_INVAL`); pinning surfaces its own
//! `PinError` discriminants as small non-zero u64 codes.

#![allow(dead_code)]

use crate::arch::VirtAddr;
use core::sync::atomic::{AtomicU64, Ordering};

use crate::capability;

// ── Public syscall numbers (referenced by syscall.rs dispatch) ─────────

pub const SYS_WALL_CLOCK: u64 = 40;
pub const SYS_GAME_MODE_ENTER: u64 = 41;
pub const SYS_GAME_MODE_EXIT: u64 = 42;
pub const SYS_GAME_MODE_STATUS: u64 = 43;
pub const SYS_NULL_LATENCY_ENTER: u64 = 44;
pub const SYS_NULL_LATENCY_EXIT: u64 = 45;
pub const SYS_PIN_MEMORY: u64 = 46;
pub const SYS_UNPIN_MEMORY: u64 = 47;
pub const SYS_DEADLINE_STATS: u64 = 48;
pub const SYS_PERF_TSC: u64 = 49;

// ── Pin-error → userspace code mapping ─────────────────────────────────

const E_PIN_ZERO_SIZE: u64 = 0xFFFF_FFFF_FFFF_FE01;
const E_PIN_UNALIGNED: u64 = 0xFFFF_FFFF_FFFF_FE02;
const E_PIN_EXCEEDS_LIMIT: u64 = 0xFFFF_FFFF_FFFF_FE03;
const E_PIN_ALREADY_PINNED: u64 = 0xFFFF_FFFF_FFFF_FE04;
const E_PIN_NOT_PINNED: u64 = 0xFFFF_FFFF_FFFF_FE05;
const E_PIN_INSUFF_CAP: u64 = 0xFFFF_FFFF_FFFF_FE07;

// ── Telemetry: count game-session entries since boot ───────────────────

static GAME_MODE_ENTRIES: AtomicU64 = AtomicU64::new(0);
static NULL_LATENCY_ENTRIES: AtomicU64 = AtomicU64::new(0);

/// Snapshot for diagnostics / `/proc/raeen/gaming`.
#[derive(Debug, Clone, Copy, Default)]
pub struct SessionStats {
    pub game_mode_entries: u64,
    pub null_latency_entries: u64,
    pub deadline_total: u64,
    pub deadline_misses: u64,
    pub worst_miss_us: u64,
}

pub fn stats() -> SessionStats {
    let ds = crate::scheduler::deadline_stats();
    SessionStats {
        game_mode_entries: GAME_MODE_ENTRIES.load(Ordering::Relaxed),
        null_latency_entries: NULL_LATENCY_ENTRIES.load(Ordering::Relaxed),
        deadline_total: ds.total_invocations,
        deadline_misses: ds.total_misses,
        worst_miss_us: ds.worst_miss_us,
    }
}

// ── Syscall implementations ────────────────────────────────────────────

/// Returns unix-epoch nanoseconds. Backed by the CMOS RTC at boot + a
/// TSC anchor delta (see kernel/src/rtc.rs). Userspace formats this as
/// a wall-clock for the desktop tray.
///
/// We bypass timers::TIMER_SUBSYSTEM here so the wall clock works even
/// before the timer subsystem is initialized — rtc::init() runs in the
/// very first phase of kernel_main, so this call is valid from any
/// later syscall path.
pub fn sys_wall_clock() -> u64 {
    let ns = crate::rtc::nanos_since_epoch_now();
    // nanos fits in u64 until year 2554 — saturate just in case.
    core::cmp::min(ns, u128::from(u64::MAX)) as u64
}

/// Mark the calling task as game-foreground. Until `sys_game_mode_exit`,
/// background CFS tasks are throttled to ~5% of normal share so they can't
/// steal cache lines from the game on its hot path.
pub fn sys_game_mode_enter() -> u64 {
    crate::scheduler::enter_game_mode();
    GAME_MODE_ENTRIES.fetch_add(1, Ordering::Relaxed);
    crate::serial_println!(
        "[game_session] game mode entered (count={})",
        GAME_MODE_ENTRIES.load(Ordering::Relaxed)
    );
    0
}

pub fn sys_game_mode_exit() -> u64 {
    crate::scheduler::exit_game_mode();
    crate::serial_println!("[game_session] game mode exited");
    0
}

/// Returns: bit0 = active, bits[31:8] = current throttle ratio
/// (1 background tick per N game ticks). The bit layout is forward
/// compatible — userspace should mask with 0x1 for the boolean.
pub fn sys_game_mode_status() -> u64 {
    let active = crate::scheduler::game_mode_active() as u64;
    // Throttle ratio constant lives in scheduler.rs (THROTTLE_RATIO = 20).
    // Expose it numerically so the Game Bar can label "5% bg quota".
    let ratio: u64 = 20;
    active | (ratio << 8)
}

/// Switch into NULL_LATENCY: pin `target` task (0 = caller) to dedicated
/// cores, route IRQs to the non-game set, refuse to boost CPU frequency
/// below max. The Concept doc's competitive-gaming primitive.
pub fn sys_null_latency_enter(target: u64) -> u64 {
    use crate::task::TaskId;
    let tid = if target == 0 {
        match crate::scheduler::current_task_id() {
            Some(id) => id,
            None => return capability::E_INVAL,
        }
    } else {
        TaskId::from_raw(target)
    };
    match crate::scheduler::enable_null_latency(tid) {
        Ok(()) => {
            NULL_LATENCY_ENTRIES.fetch_add(1, Ordering::Relaxed);
            crate::serial_println!("[game_session] NULL_LATENCY engaged for task {}", tid.raw(),);
            0
        }
        Err(()) => capability::E_INVAL,
    }
}

pub fn sys_null_latency_exit() -> u64 {
    if crate::scheduler::null_latency_active() {
        // The scheduler currently exposes only the enable path. Tearing
        // down is racy without an explicit `disable_null_latency()`;
        // until that ships, just clear the flag by reusing exit_game_mode.
        crate::scheduler::exit_game_mode();
        crate::serial_println!("[game_session] NULL_LATENCY released");
        0
    } else {
        0
    }
}

/// Pin a virtual address range into RAM. Returns 0 on success or one of
/// the E_PIN_* codes. Mirrors mlock(2) but capability-friendly.
pub fn sys_pin_memory(addr: u64, len: u64) -> u64 {
    let result = crate::memory::pin_memory(VirtAddr::new(addr), len as usize);
    match result {
        Ok(_) => 0,
        Err(e) => pin_err_code(e),
    }
}

pub fn sys_unpin_memory(addr: u64, len: u64) -> u64 {
    match crate::memory::unpin_memory(VirtAddr::new(addr), len as usize) {
        Ok(()) => 0,
        Err(e) => pin_err_code(e),
    }
}

fn pin_err_code(e: crate::memory::PinError) -> u64 {
    use crate::memory::PinError;
    match e {
        PinError::ZeroSize => E_PIN_ZERO_SIZE,
        PinError::Unaligned => E_PIN_UNALIGNED,
        PinError::ExceedsLimit => E_PIN_EXCEEDS_LIMIT,
        PinError::AlreadyPinned => E_PIN_ALREADY_PINNED,
        PinError::NotPinned => E_PIN_NOT_PINNED,
        PinError::InsufficientCapability => E_PIN_INSUFF_CAP,
    }
}

/// Copy deadline + jitter stats into a userspace buffer for the Game Bar.
///
/// Layout (32 bytes, little-endian, exact field offsets userspace can rely on):
///
/// ```text
///   off 0   u64  game_mode_entries
///   off 8   u64  null_latency_entries
///   off 16  u64  deadline_total_invocations
///   off 24  u64  deadline_total_misses
/// ```
///
/// The worst-miss-us field exists in `stats()` but is omitted from the
/// stable layout for now; we'll grow the struct with a versioned header
/// when there's a real consumer that needs it.
pub fn sys_deadline_stats(
    buf_ptr: u64,
    buf_len: u64,
    validate: impl Fn(u64, u64, bool) -> bool,
) -> u64 {
    if buf_len < 32 {
        return capability::E_INVAL;
    }
    if !validate(buf_ptr, 32, true) {
        return capability::E_INVAL;
    }
    let s = stats();
    // SMAP-safe: kernel-side pack + one validated extable copy-out.
    let mut buf = [0u8; 32];
    buf[0..8].copy_from_slice(&s.game_mode_entries.to_le_bytes());
    buf[8..16].copy_from_slice(&s.null_latency_entries.to_le_bytes());
    buf[16..24].copy_from_slice(&s.deadline_total.to_le_bytes());
    buf[24..32].copy_from_slice(&s.deadline_misses.to_le_bytes());
    if crate::uaccess::copy_to_user(buf_ptr, &buf).is_err() {
        return capability::E_INVAL;
    }
    32
}

/// Raw TSC cycle counter. Useful for the Game Bar's frametime graph and
/// for any userspace code that wants a sub-nanosecond timestamp without
/// the syscall overhead of SYS_TIME twice.
///
/// Unlike `SYS_TIME` (monotonic ns from boot), this is the literal RDTSC
/// value. Userspace can subtract two readings and divide by tsc_hz to get
/// seconds; for relative timings inside one app, the raw delta is enough.
pub fn sys_perf_tsc() -> u64 {
    let lo: u32;
    let hi: u32;
    unsafe {
        core::arch::asm!(
            "rdtsc",
            out("eax") lo,
            out("edx") hi,
            options(nomem, nostack, preserves_flags),
        );
    }
    ((hi as u64) << 32) | (lo as u64)
}

/// Banner printed during `kernel_main` so the boot log advertises which
/// concept-doc differentiators are wired.
pub fn init() {
    crate::serial_println!(
        "[ OK ] Gaming syscalls live: WALL_CLOCK, GAME_MODE_*, NULL_LATENCY_*, PIN_*, DEADLINE_STATS, PERF_TSC",
    );
}
