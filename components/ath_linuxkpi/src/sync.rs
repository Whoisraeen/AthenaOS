//! Linux locking + completion primitives.
//!
//! The daemon is cooperatively scheduled on the AthenaOS host, so a contended
//! lock yields the CPU (`msleep(0)`) rather than spinning forever. Lock words
//! are the driver's own `spinlock_t`/`struct mutex` storage, addressed by the
//! pointer the driver passes — we never allocate lock state ourselves.

use crate::host;
use core::sync::atomic::{AtomicU32, Ordering};

#[inline]
unsafe fn lockword<'a>(p: *mut u32) -> &'a AtomicU32 {
    &*(p as *const AtomicU32)
}

/// Atomic acquire of a driver-owned lock word: CAS 0→1, yielding the CPU on
/// contention (the daemon is cooperative, so a true spin would deadlock the
/// holder). Shared with the `spin_lock`/`mutex_lock` C exports in lib.rs so
/// there is ONE correct, atomic lock implementation — not a non-atomic twin.
pub(crate) fn acquire(p: *mut u32) {
    if p.is_null() {
        return;
    }
    let l = unsafe { lockword(p) };
    while l
        .compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        unsafe { host::sys_linuxkpi_msleep(0) }; // yield
    }
}

pub(crate) fn release(p: *mut u32) {
    if !p.is_null() {
        unsafe { lockword(p) }.store(0, Ordering::Release);
    }
}

// ── Raw spinlocks (the symbols the spin_lock* macros expand to) ───────────────

#[no_mangle]
pub extern "C" fn _raw_spin_lock(lock: *mut u32) {
    acquire(lock);
}
#[no_mangle]
pub extern "C" fn _raw_spin_unlock(lock: *mut u32) {
    release(lock);
}
#[no_mangle]
pub extern "C" fn _raw_spin_lock_bh(lock: *mut u32) {
    acquire(lock);
}
#[no_mangle]
pub extern "C" fn _raw_spin_unlock_bh(lock: *mut u32) {
    release(lock);
}

/// `spin_lock_irqsave` → returns saved flags. The daemon has no maskable IRQ
/// state of its own (host owns interrupt delivery), so flags is a sentinel.
#[no_mangle]
pub extern "C" fn _raw_spin_lock_irqsave(lock: *mut u32) -> u64 {
    acquire(lock);
    0
}
#[no_mangle]
pub extern "C" fn _raw_spin_unlock_irqrestore(lock: *mut u32, _flags: u64) {
    release(lock);
}
#[no_mangle]
pub extern "C" fn _raw_spin_trylock(lock: *mut u32) -> i32 {
    if lock.is_null() {
        return 1;
    }
    unsafe { lockword(lock) }
        .compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed)
        .is_ok() as i32
}

// ── Mutexes ──────────────────────────────────────────────────────────────────

#[no_mangle]
pub extern "C" fn __mutex_init(lock: *mut u32, _name: *const u8, _key: u64) {
    if !lock.is_null() {
        unsafe { lockword(lock) }.store(0, Ordering::Relaxed);
    }
}
#[no_mangle]
pub extern "C" fn mutex_trylock(lock: *mut u32) -> i32 {
    _raw_spin_trylock(lock)
}

// ── Completions (struct completion { u32 done; ... }) ─────────────────────────
// We treat the first word of `struct completion` as an atomic "done" counter.

#[no_mangle]
pub extern "C" fn __init_completion(c: *mut u32) {
    if !c.is_null() {
        unsafe { lockword(c) }.store(0, Ordering::Relaxed);
    }
}
#[no_mangle]
pub extern "C" fn init_completion(c: *mut u32) {
    __init_completion(c);
}
#[no_mangle]
pub extern "C" fn reinit_completion(c: *mut u32) {
    __init_completion(c);
}

/// `complete` — signal one waiter (increment done).
#[no_mangle]
pub extern "C" fn complete(c: *mut u32) {
    if !c.is_null() {
        unsafe { lockword(c) }.fetch_add(1, Ordering::Release);
    }
}
/// `complete_all` — signal all (saturate done high).
#[no_mangle]
pub extern "C" fn complete_all(c: *mut u32) {
    if !c.is_null() {
        unsafe { lockword(c) }.store(u32::MAX, Ordering::Release);
    }
}

/// `wait_for_completion` — block until `done > 0`, then consume one.
#[no_mangle]
pub extern "C" fn wait_for_completion(c: *mut u32) {
    if c.is_null() {
        return;
    }
    let d = unsafe { lockword(c) };
    loop {
        let cur = d.load(Ordering::Acquire);
        if cur > 0 {
            if cur == u32::MAX {
                return; // complete_all: stays signalled
            }
            if d.compare_exchange(cur, cur - 1, Ordering::AcqRel, Ordering::Relaxed)
                .is_ok()
            {
                return;
            }
        } else {
            unsafe { host::sys_linuxkpi_msleep(1) };
        }
    }
}

/// `wait_for_completion_timeout` — returns remaining jiffies (>0) on success,
/// 0 on timeout. `timeout` is in jiffies (host: 1 jiffy = 1 ms).
#[no_mangle]
pub extern "C" fn wait_for_completion_timeout(c: *mut u32, timeout: u64) -> u64 {
    if c.is_null() {
        return 0;
    }
    let d = unsafe { lockword(c) };
    let mut remaining = timeout.max(1);
    loop {
        let cur = d.load(Ordering::Acquire);
        if cur > 0 {
            if cur == u32::MAX {
                return remaining;
            }
            if d.compare_exchange(cur, cur - 1, Ordering::AcqRel, Ordering::Relaxed)
                .is_ok()
            {
                return remaining;
            }
        }
        if remaining == 0 {
            return 0;
        }
        unsafe { host::sys_linuxkpi_msleep(1) };
        remaining -= 1;
    }
}

// ── Read/write semaphores (init/down/up) — map to the mutex word ──────────────
#[no_mangle]
pub extern "C" fn down_read(sem: *mut u32) {
    acquire(sem);
}
#[no_mangle]
pub extern "C" fn up_read(sem: *mut u32) {
    release(sem);
}
#[no_mangle]
pub extern "C" fn down_write(sem: *mut u32) {
    acquire(sem);
}
#[no_mangle]
pub extern "C" fn up_write(sem: *mut u32) {
    release(sem);
}

/// `schedule` / `cond_resched` / `yield` — give up the CPU to the host.
#[no_mangle]
pub extern "C" fn schedule() {
    unsafe { host::sys_linuxkpi_msleep(0) };
}
#[no_mangle]
pub extern "C" fn cond_resched() -> i32 {
    unsafe { host::sys_linuxkpi_msleep(0) };
    0
}

/// `schedule_timeout(timeout)` — sleep up to `timeout` jiffies (1 jiffy = 1 ms
/// on the host), returning the jiffies "remaining" (0 — we slept the full
/// duration). `schedule_timeout_uninterruptible`/`_interruptible` alias it.
#[no_mangle]
pub extern "C" fn schedule_timeout(timeout: i64) -> i64 {
    if timeout > 0 {
        unsafe { host::sys_linuxkpi_msleep(timeout as u64) };
    }
    0
}
#[no_mangle]
pub extern "C" fn schedule_timeout_uninterruptible(timeout: i64) -> i64 {
    schedule_timeout(timeout)
}
#[no_mangle]
pub extern "C" fn schedule_timeout_interruptible(timeout: i64) -> i64 {
    schedule_timeout(timeout)
}

// ── Counting/binary semaphores (struct semaphore) — binary via the lock word ──
// Driver use is mutual exclusion, so a binary acquire/release over the first
// word is faithful. NOTE the Linux return polarity differs per call.

/// `down` — acquire (uninterruptible).
#[no_mangle]
pub extern "C" fn down(sem: *mut u32) {
    acquire(sem);
}
/// `up` — release.
#[no_mangle]
pub extern "C" fn up(sem: *mut u32) {
    release(sem);
}
/// `down_interruptible`/`down_killable` — acquire; the daemon never delivers a
/// signal mid-acquire, so always success (0).
#[no_mangle]
pub extern "C" fn down_interruptible(sem: *mut u32) -> i32 {
    acquire(sem);
    0
}
#[no_mangle]
pub extern "C" fn down_killable(sem: *mut u32) -> i32 {
    acquire(sem);
    0
}
/// `down_trylock` — Linux polarity: 0 on success, 1 if already held.
#[no_mangle]
pub extern "C" fn down_trylock(sem: *mut u32) -> i32 {
    (_raw_spin_trylock(sem) == 0) as i32
}

// ── rwsem killable/trylock variants (map to the mutex word) ──────────────────

#[no_mangle]
pub extern "C" fn down_read_killable(sem: *mut u32) -> i32 {
    acquire(sem);
    0
}
#[no_mangle]
pub extern "C" fn down_write_killable(sem: *mut u32) -> i32 {
    acquire(sem);
    0
}
/// `down_read_trylock`/`down_write_trylock` — return 1 on success (rwsem polarity).
#[no_mangle]
pub extern "C" fn down_read_trylock(sem: *mut u32) -> i32 {
    _raw_spin_trylock(sem)
}
#[no_mangle]
pub extern "C" fn down_write_trylock(sem: *mut u32) -> i32 {
    _raw_spin_trylock(sem)
}

// ── Mutex query + interruptible/killable acquire ─────────────────────────────

/// `mutex_is_locked(lock)` → 1 if currently held.
#[no_mangle]
pub extern "C" fn mutex_is_locked(lock: *mut u32) -> i32 {
    if lock.is_null() {
        return 0;
    }
    (unsafe { lockword(lock) }.load(Ordering::Acquire) != 0) as i32
}
#[no_mangle]
pub extern "C" fn mutex_lock_interruptible(lock: *mut u32) -> i32 {
    acquire(lock);
    0
}
#[no_mangle]
pub extern "C" fn mutex_lock_killable(lock: *mut u32) -> i32 {
    acquire(lock);
    0
}

// ── _raw spinlock _irq variants ──────────────────────────────────────────────

#[no_mangle]
pub extern "C" fn _raw_spin_lock_irq(lock: *mut u32) {
    acquire(lock);
}
#[no_mangle]
pub extern "C" fn _raw_spin_unlock_irq(lock: *mut u32) {
    release(lock);
}

// ── Completion extras ─────────────────────────────────────────────────────────

/// `try_wait_for_completion(c)` — non-blocking: consume one `done` and return
/// true, else false. (No syscall — host-testable.)
#[no_mangle]
pub extern "C" fn try_wait_for_completion(c: *mut u32) -> bool {
    if c.is_null() {
        return false;
    }
    let d = unsafe { lockword(c) };
    let cur = d.load(Ordering::Acquire);
    if cur == 0 {
        return false;
    }
    if cur == u32::MAX {
        return true; // complete_all stays signalled
    }
    d.compare_exchange(cur, cur - 1, Ordering::AcqRel, Ordering::Relaxed)
        .is_ok()
}

/// `completion_done(c)` → true if a `complete` is pending (does not consume).
#[no_mangle]
pub extern "C" fn completion_done(c: *mut u32) -> bool {
    if c.is_null() {
        return false;
    }
    unsafe { lockword(c) }.load(Ordering::Acquire) != 0
}

/// `wait_for_completion_interruptible(c)` — the daemon delivers no signal, so
/// this always completes (0).
#[no_mangle]
pub extern "C" fn wait_for_completion_interruptible(c: *mut u32) -> i32 {
    wait_for_completion(c);
    0
}
#[no_mangle]
pub extern "C" fn wait_for_completion_killable(c: *mut u32) -> i32 {
    wait_for_completion(c);
    0
}
/// `wait_for_completion_interruptible_timeout` — same as the plain timeout
/// variant here (no interrupting signal); returns remaining jiffies or 0.
#[no_mangle]
pub extern "C" fn wait_for_completion_interruptible_timeout(c: *mut u32, timeout: u64) -> i64 {
    wait_for_completion_timeout(c, timeout) as i64
}

// ── Wait queues ───────────────────────────────────────────────────────────────
// `wait_event*` is a C macro that polls the driver's CONDITION around
// `schedule()`; the only exported symbols are the queue primitives. In a
// cooperative single-threaded daemon the waiter re-checks its own condition, so
// there is no cross-thread wakeup to deliver — these manage the queue word and
// yield, and exist so a real driver's `wait_event*`/`wake_up*` sites link.

/// `init_waitqueue_head(q)` — zero the queue's first word.
#[no_mangle]
pub extern "C" fn init_waitqueue_head(q: *mut u32) {
    if !q.is_null() {
        unsafe { lockword(q) }.store(0, Ordering::Relaxed);
    }
}

/// `__wake_up(q, mode, nr, key)` — bump the queue's wake generation. A waiter
/// polling its condition makes progress on the next `schedule()`; the counter
/// lets `wait_event`-style sites that read the head observe that a wake fired.
#[no_mangle]
pub extern "C" fn __wake_up(q: *mut u32, _mode: u32, _nr: i32, _key: *mut u8) {
    if !q.is_null() {
        unsafe { lockword(q) }.fetch_add(1, Ordering::Release);
    }
}
#[no_mangle]
pub extern "C" fn __wake_up_all_locked(q: *mut u32, _mode: u32, _nr: i32) {
    __wake_up(q, 0, 0, core::ptr::null_mut());
}

/// `wake_up_all(q)` — Linux macro `__wake_up(q, TASK_NORMAL, 0, NULL)`; the shim
/// externs it as a symbol, so provide it.
#[no_mangle]
pub extern "C" fn wake_up_all(q: *mut u32) {
    __wake_up(q, 3, 0, core::ptr::null_mut());
}

/// `__wait_block_timeout(wq, timeout)` — the cooperative-pump backend of the
/// shim's `wait_event_timeout` (`while (!cond && wet > 0) wet = __wait_block_
/// timeout(&wq, wet)`). Sleep ~1 jiffy (1 ms host clock) so the timeout measures
/// real time, then return the decremented remaining timeout; the caller re-checks
/// its condition between calls — a polling wait, correct for the cooperative daemon.
#[no_mangle]
pub extern "C" fn __wait_block_timeout(_wq: *mut u32, timeout: i64) -> i64 {
    unsafe { crate::host::sys_linuxkpi_msleep(1) };
    (timeout - 1).max(0)
}

/// `__wait_block(wq)` — the untimed sibling: the shim's `wait_event` loop calls
/// this while its condition is false (`while (!cond) __wait_block(&wq)`). Sleep
/// ~1 jiffy and return; the caller re-checks the condition — a cooperative poll.
#[no_mangle]
pub extern "C" fn __wait_block(_wq: *mut u32) {
    unsafe { crate::host::sys_linuxkpi_msleep(1) };
}

/// `prepare_to_wait_event(q, wait, state)` → 0 (no pending signal to report).
#[no_mangle]
pub extern "C" fn prepare_to_wait_event(_q: *mut u32, _wait: *mut u8, _state: i32) -> i32 {
    0
}
/// `finish_wait(q, wait)` — nothing to dequeue in the cooperative model.
#[no_mangle]
pub extern "C" fn finish_wait(_q: *mut u32, _wait: *mut u8) {
    unsafe { host::sys_linuxkpi_msleep(0) }; // yield, matching the schedule() in wait_event
}

#[cfg(test)]
mod tests {
    //! Pure host KATs for the COMPLETION non-blocking paths (no syscall path —
    //! safe on Windows-native per project memory). The blocking
    //! `wait_for_completion*` variants yield via `sys_linuxkpi_msleep` and so are
    //! NOT exercised here; their state machine (consume one `done`, saturate on
    //! `complete_all`) is identical to `try_wait_for_completion`, which IS tested.
    //! Every assert is a concrete expected-vs-actual comparison and can FAIL.
    use super::*;

    #[test]
    fn complete_then_try_wait_consumes_one() {
        let mut c: u32 = 0xDEAD;
        let p = &mut c as *mut u32;
        init_completion(p);
        assert_eq!(c, 0, "init_completion zeroes the done count");
        assert!(!completion_done(p), "fresh completion has nothing pending");
        assert!(
            !try_wait_for_completion(p),
            "try_wait on a fresh completion must be false"
        );

        complete(p); // signal one
        assert!(completion_done(p), "after complete, a signal is pending");
        assert!(
            try_wait_for_completion(p),
            "try_wait consumes the one pending completion → true"
        );
        assert_eq!(c, 0, "consuming the single signal returns done to 0");
        assert!(
            !try_wait_for_completion(p),
            "no further signal → second try_wait must be false"
        );
    }

    #[test]
    fn complete_is_counting() {
        let mut c: u32 = 0;
        let p = &mut c as *mut u32;
        init_completion(p);
        complete(p);
        complete(p);
        complete(p);
        assert_eq!(c, 3, "three completes accumulate three signals");
        assert!(try_wait_for_completion(p));
        assert!(try_wait_for_completion(p));
        assert!(try_wait_for_completion(p));
        assert!(
            !try_wait_for_completion(p),
            "exactly three signals were available, no more"
        );
    }

    #[test]
    fn complete_all_stays_signalled() {
        let mut c: u32 = 0;
        let p = &mut c as *mut u32;
        init_completion(p);
        complete_all(p);
        assert_eq!(c, u32::MAX, "complete_all saturates the done count high");
        // complete_all wakes ALL waiters and is not consumed by a wait.
        assert!(completion_done(p));
        assert!(
            try_wait_for_completion(p),
            "first observer sees it signalled"
        );
        assert_eq!(c, u32::MAX, "complete_all must NOT be consumed");
        assert!(
            try_wait_for_completion(p),
            "a second observer also sees complete_all signalled"
        );
    }

    #[test]
    fn reinit_resets_done_count() {
        let mut c: u32 = 0;
        let p = &mut c as *mut u32;
        init_completion(p);
        complete(p);
        complete(p);
        assert!(completion_done(p));
        reinit_completion(p);
        assert_eq!(c, 0, "reinit_completion clears all pending signals");
        assert!(!completion_done(p), "after reinit nothing is pending");
        assert!(
            !try_wait_for_completion(p),
            "reinit must drop the previously-accumulated completions"
        );
    }
}
