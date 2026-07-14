//! Workqueue + timer facade — deferred-execution surface for Linux drivers.
//!
//! Linux drivers schedule deferred work three ways, all of which the kernel
//! runs on its own threads/timer-IRQ — context a userspace LinuxKPI daemon
//! doesn't have. So, like `irq.rs`, we record the callback in a fixed registry
//! and run it from a **daemon-driven pump** instead of a real worker thread:
//!
//!   * `schedule_work` / `queue_work`        → drained by `lkpi_run_work()`
//!   * `mod_timer` / `add_timer`             → fired by `lkpi_run_timers(now)`
//!   * `schedule_delayed_work` (work+timer)  → armed in the timer registry
//!
//! KEY LAYOUT FACT: in the Linux ABI the callback sits at the **same offset 24**
//! in all three structs — `work_struct.func`, `timer_list.function`, and (since
//! `delayed_work`'s first member is its `work_struct`) `delayed_work` too. So a
//! single `fire(ptr)` reads the fn pointer at `ptr+24` and calls it with `ptr`,
//! uniformly. The structs below are `#[repr(C)]` with that exact layout, so both
//! the host harness (which builds them in Rust) and a real `.ko` (compiled
//! against matching headers) hit the same offsets.
//!
//! Time is in jiffies. The pump owns the clock: `lkpi_run_timers(now)` records
//! `now`, and `schedule_delayed_work(_, delay)` arms at `now + delay`. The
//! daemon feeds real jiffies via the pump; the host harness feeds a controlled
//! value — so the timer path is fully deterministic off-target (no jiffies
//! syscall dependency in this module).

use core::ffi::c_void;
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

/// Linux `work_func_t` — `void (*)(struct work_struct *)`.
pub type WorkFunc = extern "C" fn(*mut WorkStruct);
/// Linux timer callback — `void (*)(struct timer_list *)`.
pub type TimerFunc = extern "C" fn(*mut TimerList);

/// `struct work_struct` (offsets match Linux on LP64: func @ 24).
#[repr(C)]
pub struct WorkStruct {
    pub data: u64,              // atomic_long_t       (offset 0)
    pub entry: [usize; 2],      // struct list_head    (offset 8)
    pub func: Option<WorkFunc>, // work_func_t         (offset 24)
}

/// `struct timer_list` (offsets match Linux on LP64: function @ 24).
#[repr(C)]
pub struct TimerList {
    pub entry: [usize; 2],           // struct hlist_node (offset 0)
    pub expires: u64,                // unsigned long     (offset 16)
    pub function: Option<TimerFunc>, // callback          (offset 24)
    pub flags: u32,                  // (offset 32)
}

/// Byte offset of the callback fn-pointer in work_struct / timer_list /
/// delayed_work (work-first). The whole uniform-fire trick rests on this.
const FUNC_OFFSET: usize = 24;

const MAX_WORK: usize = 64;
const MAX_TIMERS: usize = 64;

// Pending work: each slot holds a `*mut WorkStruct` as usize (0 = empty).
static WORK: [AtomicUsize; MAX_WORK] = [const { AtomicUsize::new(0) }; MAX_WORK];
// Armed timers: parallel ptr/expiry arrays (0 ptr = empty).
static TIMER_PTR: [AtomicUsize; MAX_TIMERS] = [const { AtomicUsize::new(0) }; MAX_TIMERS];
static TIMER_EXP: [AtomicU64; MAX_TIMERS] = [const { AtomicU64::new(0) }; MAX_TIMERS];
// Last jiffies the timer pump was driven with (the shim's notion of "now").
static NOW_JIFFIES: AtomicU64 = AtomicU64::new(0);

/// Read the callback at `ptr + FUNC_OFFSET` and invoke it with `ptr`.
/// `Option<fn>` is niche-encoded as the bare fn pointer (None = 0), and a real C
/// struct stores a plain fn pointer there — so reading a `usize` works for both.
///
/// # Safety
/// `ptr` must be a live `work_struct`/`timer_list`/`delayed_work` whose callback
/// field is initialised (or null). The pump only ever passes pointers the driver
/// itself handed to `schedule_*`/`mod_timer`.
#[inline]
unsafe fn fire(ptr: usize) {
    if ptr == 0 {
        return;
    }
    let func_addr = core::ptr::read((ptr + FUNC_OFFSET) as *const usize);
    if func_addr != 0 {
        let f: extern "C" fn(*mut c_void) = core::mem::transmute(func_addr);
        f(ptr as *mut c_void);
    }
}

// ── Work ────────────────────────────────────────────────────────────────────

/// Linux `bool schedule_work(struct work_struct *work)` — queue `work` on the
/// system workqueue. Returns true if it was queued, false if already pending
/// (Linux dedups) or the queue is full.
#[no_mangle]
pub extern "C" fn schedule_work(work: *mut WorkStruct) -> bool {
    let p = work as usize;
    if p == 0 {
        return false;
    }
    for slot in &WORK {
        if slot.load(Ordering::Acquire) == p {
            return false; // already pending
        }
    }
    for slot in &WORK {
        if slot
            .compare_exchange(0, p, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
        {
            return true;
        }
    }
    false // queue full
}

/// Linux `bool queue_work(struct workqueue_struct *wq, struct work_struct *work)`.
/// All work runs on the one daemon pump, so `wq` is ignored.
#[no_mangle]
pub extern "C" fn queue_work(_wq: *mut c_void, work: *mut WorkStruct) -> bool {
    schedule_work(work)
}

/// Linux `bool cancel_work_sync(struct work_struct *work)` — dequeue if pending.
/// Returns true if it was pending (i.e. a queued run was cancelled).
#[no_mangle]
pub extern "C" fn cancel_work_sync(work: *mut WorkStruct) -> bool {
    let p = work as usize;
    if p == 0 {
        return false;
    }
    let mut found = false;
    for slot in &WORK {
        if slot
            .compare_exchange(p, 0, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
        {
            found = true;
        }
    }
    found
}

/// Linux `bool flush_work(struct work_struct *work)` — run a pending `work` now
/// (synchronously) rather than waiting for the pump. Returns true if it ran.
#[no_mangle]
pub extern "C" fn flush_work(work: *mut WorkStruct) -> bool {
    let p = work as usize;
    if p == 0 {
        return false;
    }
    for slot in &WORK {
        if slot
            .compare_exchange(p, 0, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
        {
            unsafe { fire(p) };
            return true;
        }
    }
    false
}

/// Daemon pump: run every pending work item once, in slot order. Returns the
/// number fired. A callback may re-queue itself (the slot is freed before it
/// runs). The daemon calls this from its main loop; the harness drives it.
#[no_mangle]
pub extern "C" fn lkpi_run_work() -> u32 {
    let mut n = 0u32;
    for slot in &WORK {
        let p = slot.swap(0, Ordering::AcqRel);
        if p != 0 {
            unsafe { fire(p) };
            n += 1;
        }
    }
    n
}

// ── Timers ────────────────────────────────────────────────────────────────────

/// Linux `timer_setup(timer, callback, flags)` — bind the callback. (In Linux
/// this is a macro over `__init_timer`; the facade exposes it as a function.)
#[no_mangle]
pub extern "C" fn timer_setup(timer: *mut TimerList, func: TimerFunc, _flags: u32) {
    if !timer.is_null() {
        unsafe { (*timer).function = Some(func) };
    }
}

/// Linux `int mod_timer(struct timer_list *timer, unsigned long expires)` — arm
/// or re-arm `timer` to fire at absolute jiffies `expires`. Returns 1 if the
/// timer was already active (re-armed), 0 if it was newly armed.
#[no_mangle]
pub extern "C" fn mod_timer(timer: *mut TimerList, expires: u64) -> i32 {
    let p = timer as usize;
    if p == 0 {
        return 0;
    }
    for i in 0..MAX_TIMERS {
        if TIMER_PTR[i].load(Ordering::Acquire) == p {
            TIMER_EXP[i].store(expires, Ordering::Release);
            return 1; // was active
        }
    }
    for i in 0..MAX_TIMERS {
        if TIMER_PTR[i]
            .compare_exchange(0, p, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
        {
            TIMER_EXP[i].store(expires, Ordering::Release);
            return 0; // newly armed
        }
    }
    0 // registry full
}

/// Linux `void add_timer(struct timer_list *timer)` — arm using `timer->expires`.
#[no_mangle]
pub extern "C" fn add_timer(timer: *mut TimerList) {
    if !timer.is_null() {
        let exp = unsafe { (*timer).expires };
        mod_timer(timer, exp);
    }
}

/// Linux `int del_timer(struct timer_list *timer)` — disarm. Returns 1 if it was
/// active. (`del_timer_sync` / `timer_delete_sync` are the same here — there is
/// no concurrent timer base to synchronise against in the cooperative pump.)
#[no_mangle]
pub extern "C" fn del_timer(timer: *mut TimerList) -> i32 {
    let p = timer as usize;
    if p == 0 {
        return 0;
    }
    for i in 0..MAX_TIMERS {
        if TIMER_PTR[i]
            .compare_exchange(p, 0, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
        {
            TIMER_EXP[i].store(0, Ordering::Release);
            return 1;
        }
    }
    0
}

#[no_mangle]
pub extern "C" fn del_timer_sync(timer: *mut TimerList) -> i32 {
    del_timer(timer)
}

/// Newer Linux name for `del_timer_sync` (6.2+).
#[no_mangle]
pub extern "C" fn timer_delete_sync(timer: *mut TimerList) -> i32 {
    del_timer(timer)
}

/// Newer Linux name for `del_timer` (6.2+) — disarm; returns 1 if it was active.
#[no_mangle]
pub extern "C" fn timer_delete(timer: *mut TimerList) -> i32 {
    del_timer(timer)
}

/// `timer_pending(timer)` — true while the timer is armed (registered + not yet
/// fired). Matches Linux's `timer_pending`.
#[no_mangle]
pub extern "C" fn timer_pending(timer: *const TimerList) -> bool {
    let p = timer as usize;
    if p == 0 {
        return false;
    }
    for i in 0..MAX_TIMERS {
        if TIMER_PTR[i].load(Ordering::Acquire) == p {
            return true;
        }
    }
    false
}

/// Daemon pump: record `now_jiffies` as the shim clock, then fire every armed
/// timer whose expiry has passed (one-shot — the slot is cleared before the
/// callback runs, so it may re-arm). Returns the number fired. The daemon calls
/// this each loop with `get_jiffies_64()`; the harness passes a controlled time.
#[no_mangle]
pub extern "C" fn lkpi_run_timers(now_jiffies: u64) -> u32 {
    NOW_JIFFIES.store(now_jiffies, Ordering::Release);
    let mut n = 0u32;
    for i in 0..MAX_TIMERS {
        let p = TIMER_PTR[i].load(Ordering::Acquire);
        if p != 0 && TIMER_EXP[i].load(Ordering::Acquire) <= now_jiffies {
            if TIMER_PTR[i]
                .compare_exchange(p, 0, Ordering::AcqRel, Ordering::Relaxed)
                .is_ok()
            {
                TIMER_EXP[i].store(0, Ordering::Release);
                unsafe { fire(p) };
                n += 1;
            }
        }
    }
    n
}

// ── Delayed work (work + timer) ───────────────────────────────────────────────

/// Linux `bool schedule_delayed_work(struct delayed_work *dwork, unsigned long
/// delay)` — run `dwork`'s work after `delay` jiffies. `dwork`'s first member is
/// its `work_struct`, so `&dwork == &dwork->work` and the work callback is at
/// `dwork + 24` — armed in the timer registry and fired uniformly. Returns true
/// if newly armed.
#[no_mangle]
pub extern "C" fn schedule_delayed_work(dwork: *mut WorkStruct, delay: u64) -> bool {
    let p = dwork as usize;
    if p == 0 {
        return false;
    }
    let expires = NOW_JIFFIES.load(Ordering::Acquire).wrapping_add(delay);
    mod_timer(dwork as *mut TimerList, expires) == 0
}

/// Linux `bool queue_delayed_work(wq, dwork, delay)` — `wq` ignored.
#[no_mangle]
pub extern "C" fn queue_delayed_work(_wq: *mut c_void, dwork: *mut WorkStruct, delay: u64) -> bool {
    schedule_delayed_work(dwork, delay)
}

/// Linux `bool cancel_delayed_work_sync(struct delayed_work *dwork)`. Returns
/// true if a pending run was cancelled.
#[no_mangle]
pub extern "C" fn cancel_delayed_work_sync(dwork: *mut WorkStruct) -> bool {
    del_timer(dwork as *mut TimerList) == 1
}

/// Linux `bool cancel_delayed_work(struct delayed_work *dwork)` — the
/// non-blocking sibling of `cancel_delayed_work_sync`. The two differ in Linux
/// only in whether they wait for an in-progress run to finish before returning
/// (there is none to wait for here — same cooperative-pump reasoning as
/// `del_timer`/`del_timer_sync` above), so this is the identical cancel.
#[no_mangle]
pub extern "C" fn cancel_delayed_work(dwork: *mut WorkStruct) -> bool {
    del_timer(dwork as *mut TimerList) == 1
}

/// Linux `bool mod_delayed_work(wq, dwork, delay)` — reschedule `dwork` to fire
/// `delay` jiffies from now, arming it fresh if it wasn't already pending.
/// `wq` is ignored (single daemon pump, as elsewhere in this file). Returns
/// true if `dwork` was already pending (and got rescheduled), false if it was
/// newly armed by this call.
#[no_mangle]
pub extern "C" fn mod_delayed_work(_wq: *mut c_void, dwork: *mut WorkStruct, delay: u64) -> bool {
    let expires = NOW_JIFFIES.load(Ordering::Acquire).wrapping_add(delay);
    mod_timer(dwork as *mut TimerList, expires) == 1
}

// ── Workqueue objects ─────────────────────────────────────────────────────────
// Drivers allocate their own workqueues, but everything runs on the single
// daemon pump regardless — so a workqueue handle is just a non-null sentinel and
// flushing a wq drains the global work queue.

#[no_mangle]
pub extern "C" fn alloc_workqueue(_name: *const u8, _flags: u32, _max_active: i32) -> *mut c_void {
    1 as *mut c_void
}

/// Linux `alloc_ordered_workqueue(fmt, flags, ...)` — a workqueue that runs its
/// work items strictly in queue order (max_active=1). The single daemon pump
/// already drains `WORK` in slot order every call to `lkpi_run_work()`, so it's
/// already ordered — same non-null sentinel as `alloc_workqueue` above.
#[no_mangle]
pub extern "C" fn alloc_ordered_workqueue(_fmt: *const u8, _flags: u32) -> *mut c_void {
    1 as *mut c_void
}

#[no_mangle]
pub extern "C" fn create_singlethread_workqueue(_name: *const u8) -> *mut c_void {
    1 as *mut c_void
}

#[no_mangle]
pub extern "C" fn create_workqueue(_name: *const u8) -> *mut c_void {
    1 as *mut c_void
}

#[no_mangle]
pub extern "C" fn destroy_workqueue(_wq: *mut c_void) {}

#[no_mangle]
pub extern "C" fn flush_workqueue(_wq: *mut c_void) {
    lkpi_run_work();
}

#[no_mangle]
pub extern "C" fn flush_scheduled_work() {
    lkpi_run_work();
}
