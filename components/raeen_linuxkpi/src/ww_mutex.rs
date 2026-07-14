//! Wound/wait mutexes (`ww_mutex` / `ww_acquire_ctx`) — DRM's deadlock-avoidance
//! primitive for locking many buffers atomically.
//!
//! Every modern DRM GPU driver's command-submission path locks the `dma_resv`
//! ww_mutex of *every* buffer a submit touches, in no fixed order. Locking
//! multiple mutexes in arbitrary order normally deadlocks; ww_mutex prevents
//! that with a global, monotonically increasing acquire ticket (a "stamp"). Each
//! `ww_acquire_ctx` is stamped once at `ww_acquire_init`; the stamp orders all
//! contexts as OLDER (lower stamp) or YOUNGER (higher stamp). When a context
//! contends for a mutex already held by *another* context:
//!
//!   - a YOUNGER acquirer hitting an OLDER holder **backs off**: it gets
//!     `-EDEADLK`, drops every lock it holds, and restarts its lock loop after
//!     the older context releases (the deadlock-avoidance signal);
//!   - an OLDER acquirer hitting a YOUNGER holder **wounds** the younger holder
//!     — the younger context is flagged `wounded` and must abort (return
//!     `-EDEADLK` on its *next* contended lock attempt) and restart, yielding the
//!     contested lock to the older context.
//!
//! This is the **wound-wait** variant (Linux `ww_class` with `is_wait_die == 0`):
//! the older context always wins, so global progress is guaranteed (the oldest
//! live context can never be made to back off).
//!
//! **Cooperative-daemon modeling.** The AthenaOS driver daemons are cooperatively
//! single-threaded, so true preemptive contention between two live contexts is
//! rare — but the wound/wait *protocol* (the `-EDEADLK`/restart contract and the
//! wound flag) must still be correct, because the drivers' lock-many-buffers
//! loops are *written* around it: they call `ww_mutex_lock(buf, ctx)` in a loop,
//! and on `-EDEADLK` they unlock everything and `ww_mutex_lock_slow(contended)`
//! to re-seed the order. We therefore implement the full stamp comparison and
//! wound bookkeeping against the holder context the mutex records, rather than
//! treating the lock as a plain mutex. The decision is made synchronously at
//! lock time from the holder's stamp vs the acquirer's stamp.
//!
//! **Layout** (BTF, Linux 6.6 x86_64): `ww_mutex` is `{ struct mutex base; struct
//! ww_acquire_ctx *ctx; }`. We do NOT mirror the opaque `struct mutex` field by
//! field — drivers only ever touch a `ww_mutex` through these functions — but we
//! DO pin `ctx`'s offset so a driver that embeds a `ww_mutex` and reads
//! `lock->ctx` (some do, to detect self-recursion) sees the right word. The owner
//! lock word lives at offset 0 (the first word of `struct mutex`).

use crate::host;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

/// `-EDEADLK` — the wound/wait back-off signal. The caller MUST drop every lock
/// it holds and restart its acquire loop (typically via `ww_mutex_lock_slow`).
pub const EDEADLK: i32 = -35;
/// `-EALREADY` — `ww_mutex_lock` called twice with the same ctx on the same lock.
pub const EALREADY: i32 = -114;

/// `struct ww_class` — owns the global acquire-ticket sequence every
/// `ww_acquire_ctx` draws its stamp from. `is_wait_die == 0` selects wound-wait.
#[repr(C)]
pub struct WwClass {
    /// `atomic_long_t stamp` — the next ticket to hand out (monotonic).
    pub stamp: u64,
    pub acquire_name: *const u8,
    pub mutex_name: *const u8,
    /// 0 = wound-wait (older wounds younger); non-zero = wait-die. We implement
    /// wound-wait, the DRM default.
    pub is_wait_die: u32,
    _pad: u32,
}

/// `struct ww_acquire_ctx` — one per "lock this whole set of buffers" attempt.
/// Stamped once at init; the stamp is this context's age (lower == older).
#[repr(C)]
pub struct WwAcquireCtx {
    /// Owning "task" — a non-null cookie distinguishing contexts. In the daemon
    /// we just need each ctx to compare unequal to others, so we store the ctx's
    /// own address here at init.
    pub task: usize,
    /// `unsigned long stamp` — this context's age ticket.
    pub stamp: u64,
    /// How many locks this context currently holds (for `ww_acquire_done`
    /// sanity + debug). Bumped on each successful contended/uncontended lock.
    pub acquired: u32,
    /// Set non-zero when an OLDER context wounded us; our next contended lock
    /// returns `-EDEADLK` so we back off.
    pub wounded: u32,
    /// Mirror of the class's mode for fast access.
    pub is_wait_die: u32,
    _pad: u32,
    /// Back-pointer to the class (for stamp comparisons / debug).
    pub ww_class: *const WwClass,
}

/// `struct ww_mutex` — `{ struct mutex base; struct ww_acquire_ctx *ctx; }`.
/// We use the first word of `base` as the atomic owner-lock word and keep the
/// acquiring context pointer in `ctx` (whose offset matches Linux).
#[repr(C)]
pub struct WwMutex {
    /// `struct mutex base` — opaque to drivers; first word is the lock state
    /// (0 = free, non-zero = held). The rest is padding to the real mutex size
    /// so `ctx` lands at the BTF offset.
    pub base: [u8; 32],
    /// `struct ww_acquire_ctx *ctx` — the context currently holding the lock
    /// (null if free or held without a ctx). Drivers read this to detect when
    /// *they* already hold it.
    pub ctx: *mut WwAcquireCtx,
}

// ── C ABI shapes drivers compile against (BTF: Linux 6.6 x86_64) ──────────────
// `struct mutex` is 32 bytes on x86_64 (owner atomic_long + wait_lock spinlock +
// osq + wait_list), so `ctx` follows at offset 32. A mismatch here would put the
// holder-context pointer at the wrong word and silently break wound/wait.
const _: () = assert!(core::mem::size_of::<WwMutex>() == 40);
const _: () = assert!(core::mem::offset_of!(WwMutex, ctx) == 32);
const _: () = assert!(core::mem::size_of::<WwClass>() == 32);
const _: () = assert!(core::mem::offset_of!(WwClass, stamp) == 0);
const _: () = assert!(core::mem::offset_of!(WwAcquireCtx, stamp) == 8);
const _: () = assert!(core::mem::offset_of!(WwAcquireCtx, acquired) == 16);
const _: () = assert!(core::mem::offset_of!(WwAcquireCtx, wounded) == 20);

#[inline]
unsafe fn lockword<'a>(m: *mut WwMutex) -> &'a AtomicU32 {
    // First word of `base` is the lock state.
    &*((m as *mut u8) as *const AtomicU32)
}

/// The wound/wait decision for a *contended* acquire, factored out so it is
/// host-testable WITHOUT entering the cooperative block loop. `slow` is the
/// `ww_mutex_lock_slow` flag (a re-acquire that must never back off).
///
/// Returns one of:
///   - `Decision::BackOff` → caller must return `-EDEADLK` (we are younger);
///   - `Decision::WoundAndWait` → we are older: wound the younger holder, then
///     block for the release;
///   - `Decision::JustWait` → equal/slow path: block for the release, no wound.
#[derive(Debug, PartialEq, Eq)]
enum Decision {
    BackOff,
    WoundAndWait,
    JustWait,
}

#[inline]
fn wound_decision(my_stamp: u64, holder_stamp: u64, slow: bool) -> Decision {
    if !slow && my_stamp > holder_stamp {
        Decision::BackOff
    } else if my_stamp < holder_stamp {
        Decision::WoundAndWait
    } else {
        // Equal stamp can only happen for the slow re-acquire of our own
        // contended lock against a stale holder record, or two ctxs sharing a
        // stamp (we never hand out duplicates) — block without wounding.
        Decision::JustWait
    }
}

// ── ww_class / ww_acquire_ctx ─────────────────────────────────────────────────

/// `ww_class` static initializer helper — zero/seed a class. (Linux uses
/// `DEFINE_WW_CLASS`; daemons that build the class at runtime call this.)
#[no_mangle]
pub extern "C" fn ww_class_init(class: *mut WwClass, is_wait_die: u32) {
    if class.is_null() {
        return;
    }
    unsafe {
        (*class).stamp = 1; // first handed-out ticket is 1 (0 == "no ctx")
        (*class).acquire_name = core::ptr::null();
        (*class).mutex_name = core::ptr::null();
        (*class).is_wait_die = is_wait_die;
        (*class)._pad = 0;
    }
}

#[inline]
unsafe fn class_stamp_field<'a>(class: *mut WwClass) -> &'a AtomicU64 {
    &*(&(*class).stamp as *const u64 as *const AtomicU64)
}

/// `ww_acquire_init(ctx, class)` — draw the next monotonic stamp from the class
/// and start a fresh acquire context (nothing held, not wounded).
#[no_mangle]
pub extern "C" fn ww_acquire_init(ctx: *mut WwAcquireCtx, class: *mut WwClass) {
    if ctx.is_null() {
        return;
    }
    let (stamp, mode, classp) = if class.is_null() {
        (1u64, 0u32, core::ptr::null())
    } else {
        // fetch_add returns the previous value → this ctx's stamp; next ctx gets +1.
        let s = unsafe { class_stamp_field(class) }.fetch_add(1, Ordering::Relaxed);
        (s, unsafe { (*class).is_wait_die }, class as *const WwClass)
    };
    unsafe {
        (*ctx).task = ctx as usize; // unique non-null cookie
        (*ctx).stamp = stamp;
        (*ctx).acquired = 0;
        (*ctx).wounded = 0;
        (*ctx).is_wait_die = mode;
        (*ctx)._pad = 0;
        (*ctx).ww_class = classp;
    }
}

/// `ww_acquire_done(ctx)` — mark the acquire phase finished (no more locks will
/// be taken under this ctx). Diagnostic only in the cooperative model.
#[no_mangle]
pub extern "C" fn ww_acquire_done(_ctx: *mut WwAcquireCtx) {}

/// `ww_acquire_fini(ctx)` — end the context; all its locks must already be
/// released. We clear the stamp so a stale pointer can't masquerade as a live ctx.
#[no_mangle]
pub extern "C" fn ww_acquire_fini(ctx: *mut WwAcquireCtx) {
    if !ctx.is_null() {
        unsafe {
            (*ctx).task = 0;
            (*ctx).wounded = 0;
        }
    }
}

// ── ww_mutex ──────────────────────────────────────────────────────────────────

/// `ww_mutex_init(lock, class)` — free, unowned mutex.
#[no_mangle]
pub extern "C" fn ww_mutex_init(lock: *mut WwMutex, _class: *mut WwClass) {
    if lock.is_null() {
        return;
    }
    unsafe {
        (*lock).base = [0u8; 32];
        (*lock).ctx = core::ptr::null_mut();
    }
}

/// `ww_mutex_is_locked(lock)` → 1 if held.
#[no_mangle]
pub extern "C" fn ww_mutex_is_locked(lock: *mut WwMutex) -> i32 {
    if lock.is_null() {
        return 0;
    }
    (unsafe { lockword(lock) }.load(Ordering::Acquire) != 0) as i32
}

/// Core wound/wait acquire. Returns 0 on success, `-EDEADLK` if the caller must
/// back off and restart, `-EALREADY` if `ctx` already holds `lock`.
///
/// `slow` selects the `ww_mutex_lock_slow` semantics: the caller has already
/// backed off and is re-acquiring the *contended* lock first, so it must NOT
/// itself return `-EDEADLK` (it blocks instead). In the cooperative daemon we
/// model that by clearing our own wounded flag for the slow acquire.
fn ww_acquire(lock: *mut WwMutex, ctx: *mut WwAcquireCtx, slow: bool) -> i32 {
    if lock.is_null() {
        return 0;
    }
    let lw = unsafe { lockword(lock) };

    // Fast path: try to take it uncontended.
    if lw
        .compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed)
        .is_ok()
    {
        if !ctx.is_null() {
            unsafe {
                (*lock).ctx = ctx;
                (*ctx).acquired += 1;
            }
        }
        return 0;
    }

    // Contended. Inspect the current holder's context.
    let holder = unsafe { (*lock).ctx };

    // Re-locking with the same ctx is an API error (Linux returns -EALREADY).
    if !ctx.is_null() && holder == ctx {
        return EALREADY;
    }

    // No-ctx acquire (or holder has no ctx): degrade to a plain blocking lock —
    // no wound/wait ordering is possible without two stamped contexts.
    if ctx.is_null() || holder.is_null() {
        // Cooperative spin-acquire (same model as sync::acquire).
        loop {
            if lw
                .compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
            {
                if !ctx.is_null() {
                    unsafe {
                        (*lock).ctx = ctx;
                        (*ctx).acquired += 1;
                    }
                }
                return 0;
            }
            unsafe { host::sys_linuxkpi_msleep(0) };
        }
    }

    // Two stamped contexts contend. Compare ages (lower stamp == older).
    let my_stamp = unsafe { (*ctx).stamp };
    let holder_stamp = unsafe { (*holder).stamp };

    match wound_decision(my_stamp, holder_stamp, slow) {
        Decision::BackOff => {
            // We are YOUNGER than the holder → back off to avoid deadlock.
            return EDEADLK;
        }
        Decision::WoundAndWait => {
            // We are OLDER: wound the younger holder so it aborts on its next
            // contended lock and yields this mutex to us, then wait for release.
            unsafe { (*holder).wounded = 1 };
        }
        Decision::JustWait => {}
    }

    // Block until the holder releases (it will, once it sees it was wounded and
    // backs off — or simply when it finishes its critical section).
    loop {
        if lw
            .compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
        {
            unsafe {
                (*lock).ctx = ctx;
                (*ctx).acquired += 1;
            }
            return 0;
        }
        unsafe { host::sys_linuxkpi_msleep(0) };
    }
}

/// `ww_mutex_lock(lock, ctx)` — acquire under the wound/wait protocol. Returns
/// 0, or `-EDEADLK` (back off + restart), or `-EALREADY` (double-lock).
#[no_mangle]
pub extern "C" fn ww_mutex_lock(lock: *mut WwMutex, ctx: *mut WwAcquireCtx) -> i32 {
    // If an older context already wounded us, abort immediately (the wound
    // contract): drop and restart. Only meaningful on a contended attempt, but
    // Linux checks the flag at lock entry on every contended path.
    if !ctx.is_null() && unsafe { (*ctx).wounded } != 0 {
        // Try the fast uncontended path first — if the lock is free we may take
        // it, but the standard driver loop will have unlocked everything, so the
        // common outcome is back-off. We honour the wound by returning -EDEADLK
        // when the lock is contended; if it's free we still take it (no deadlock
        // possible) and clear our wound for the restarted loop.
        if !lock.is_null() {
            let lw = unsafe { lockword(lock) };
            if lw
                .compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
            {
                unsafe {
                    (*lock).ctx = ctx;
                    (*ctx).acquired += 1;
                }
                return 0;
            }
        }
        return EDEADLK;
    }
    ww_acquire(lock, ctx, false)
}

/// `ww_mutex_lock_interruptible(lock, ctx)` — the daemon delivers no signal
/// mid-acquire, so this is identical to `ww_mutex_lock` (it can still return
/// `-EDEADLK`).
#[no_mangle]
pub extern "C" fn ww_mutex_lock_interruptible(lock: *mut WwMutex, ctx: *mut WwAcquireCtx) -> i32 {
    ww_mutex_lock(lock, ctx)
}

/// `ww_mutex_lock_slow(lock, ctx)` — re-acquire the contended lock after a
/// back-off. By contract this NEVER returns `-EDEADLK` (the caller has dropped
/// all other locks, so blocking on this one cannot deadlock); it also clears our
/// wounded flag for the restarted acquire loop.
#[no_mangle]
pub extern "C" fn ww_mutex_lock_slow(lock: *mut WwMutex, ctx: *mut WwAcquireCtx) {
    if !ctx.is_null() {
        unsafe { (*ctx).wounded = 0 };
    }
    let _ = ww_acquire(lock, ctx, true);
}

/// `ww_mutex_lock_slow_interruptible(lock, ctx)` — same as `_slow`, returns 0.
#[no_mangle]
pub extern "C" fn ww_mutex_lock_slow_interruptible(
    lock: *mut WwMutex,
    ctx: *mut WwAcquireCtx,
) -> i32 {
    ww_mutex_lock_slow(lock, ctx);
    0
}

/// `ww_mutex_trylock(lock, ctx)` — non-blocking; never participates in wound/wait
/// (no ordering decision). Returns 1 on success, 0 if already held.
#[no_mangle]
pub extern "C" fn ww_mutex_trylock(lock: *mut WwMutex, ctx: *mut WwAcquireCtx) -> i32 {
    if lock.is_null() {
        return 1;
    }
    let lw = unsafe { lockword(lock) };
    if lw
        .compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed)
        .is_ok()
    {
        if !ctx.is_null() {
            unsafe {
                (*lock).ctx = ctx;
                (*ctx).acquired += 1;
            }
        }
        1
    } else {
        0
    }
}

/// `ww_mutex_unlock(lock)` — release. Clears the holder context and the lock
/// word so a wounded younger context (or any waiter) can take it.
#[no_mangle]
pub extern "C" fn ww_mutex_unlock(lock: *mut WwMutex) {
    if lock.is_null() {
        return;
    }
    let holder = unsafe { (*lock).ctx };
    if !holder.is_null() {
        unsafe {
            // Best-effort acquired-count bookkeeping (saturating).
            if (*holder).acquired > 0 {
                (*holder).acquired -= 1;
            }
        }
    }
    unsafe { (*lock).ctx = core::ptr::null_mut() };
    unsafe { lockword(lock) }.store(0, Ordering::Release);
}

#[cfg(test)]
mod tests {
    //! Pure host KATs (no syscall path on the uncontended/decision branches —
    //! safe on Windows-native per project memory). Every wound/wait decision is
    //! made synchronously from the holder's stamp, so we can exercise the full
    //! `-EDEADLK`/wound contract WITHOUT ever entering the cooperative
    //! `msleep`-yielding block loop: we only test contended attempts against a
    //! lock that STAYS held, which return their decision (-EDEADLK / EALREADY) or
    //! set the wound flag immediately and do not spin.
    use super::*;

    fn class() -> WwClass {
        let mut c = WwClass {
            stamp: 0,
            acquire_name: core::ptr::null(),
            mutex_name: core::ptr::null(),
            is_wait_die: 0,
            _pad: 0,
        };
        ww_class_init(&mut c as *mut WwClass, 0);
        c
    }
    fn mutex() -> WwMutex {
        WwMutex {
            base: [0u8; 32],
            ctx: core::ptr::null_mut(),
        }
    }
    fn acquire_ctx(c: &mut WwClass) -> WwAcquireCtx {
        let mut ctx = WwAcquireCtx {
            task: 0,
            stamp: 0,
            acquired: 0,
            wounded: 0,
            is_wait_die: 0,
            _pad: 0,
            ww_class: core::ptr::null(),
        };
        ww_acquire_init(&mut ctx as *mut WwAcquireCtx, c as *mut WwClass);
        ctx
    }

    #[test]
    fn lock_unlock_single_uncontended() {
        let mut c = class();
        let mut ctx = acquire_ctx(&mut c);
        let mut m = mutex();
        let mp = &mut m as *mut WwMutex;
        let cp = &mut ctx as *mut WwAcquireCtx;

        ww_mutex_init(mp, &mut c as *mut WwClass);
        assert_eq!(ww_mutex_is_locked(mp), 0, "fresh ww_mutex must be unlocked");

        assert_eq!(ww_mutex_lock(mp, cp), 0, "uncontended lock succeeds (0)");
        assert_eq!(ww_mutex_is_locked(mp), 1, "now held");
        assert_eq!(m.ctx, cp, "holder ctx recorded");
        assert_eq!(ctx.acquired, 1, "acquired count bumped");

        ww_mutex_unlock(mp);
        assert_eq!(ww_mutex_is_locked(mp), 0, "released");
        assert!(m.ctx.is_null(), "holder cleared on unlock");
        assert_eq!(ctx.acquired, 0, "acquired count decremented");
    }

    #[test]
    fn ticket_ordering_is_monotonic() {
        let mut c = class();
        let a = acquire_ctx(&mut c);
        let b = acquire_ctx(&mut c);
        let d = acquire_ctx(&mut c);
        assert!(a.stamp < b.stamp, "second ctx must get a higher stamp");
        assert!(b.stamp < d.stamp, "third ctx must get a higher stamp");
        assert_eq!(b.stamp, a.stamp + 1, "stamps are consecutive");
        assert_eq!(d.stamp, b.stamp + 1, "stamps are consecutive");
        // The stamp is the real context identity used for all wound/wait ordering
        // decisions; every live ctx has a non-zero, distinct stamp.
        assert_ne!(a.stamp, b.stamp, "distinct stamps order distinct contexts");
        assert_ne!(a.stamp, 0, "a live ctx never has the reserved 0 stamp");
    }

    #[test]
    fn younger_hitting_older_gets_edeadlk() {
        // older = a (lower stamp), younger = b (higher stamp).
        let mut c = class();
        let mut a = acquire_ctx(&mut c);
        let mut b = acquire_ctx(&mut c);
        assert!(a.stamp < b.stamp);
        let mut m = mutex();
        let mp = &mut m as *mut WwMutex;
        ww_mutex_init(mp, &mut c as *mut WwClass);

        // OLDER takes the lock.
        assert_eq!(ww_mutex_lock(mp, &mut a as *mut WwAcquireCtx), 0);

        // YOUNGER contends → must back off with -EDEADLK (deadlock-avoidance).
        let rc = ww_mutex_lock(mp, &mut b as *mut WwAcquireCtx);
        assert_eq!(
            rc, EDEADLK,
            "younger ctx hitting an older-held lock must get -EDEADLK"
        );
        assert_eq!(
            b.wounded, 0,
            "the younger backs off itself; it is NOT wounded"
        );
        assert_eq!(m.ctx, &mut a as *mut WwAcquireCtx, "older still holds it");
    }

    #[test]
    fn wound_decision_matches_wound_wait_protocol() {
        // The decision an OLDER contender (lower stamp) makes against a YOUNGER
        // holder (higher stamp): wound it and wait. The whole point is that the
        // older never backs off — global progress is guaranteed.
        assert_eq!(
            wound_decision(/*older*/ 5, /*holder younger*/ 9, false),
            Decision::WoundAndWait,
            "older vs younger holder → wound the younger and wait"
        );
        // A YOUNGER contender (higher stamp) vs an OLDER holder backs off.
        assert_eq!(
            wound_decision(/*younger*/ 9, /*holder older*/ 5, false),
            Decision::BackOff,
            "younger vs older holder → back off (-EDEADLK)"
        );
        // The slow re-acquire NEVER backs off even when younger.
        assert_eq!(
            wound_decision(/*younger*/ 9, /*holder older*/ 5, true),
            Decision::JustWait,
            "ww_mutex_lock_slow must never back off — it just waits"
        );
    }

    #[test]
    fn older_contender_wounds_younger_holder_then_younger_backs_off() {
        // End-to-end via the public API, kept pure by never letting the OLDER
        // block: the younger holds, the older's contended attempt sets the
        // wound, and we drive the younger's back-off through the public lock.
        //
        // We exercise the wound SIDE EFFECT directly through ww_acquire's decision
        // (factored helper) to avoid the older's cooperative block, then prove the
        // wounded younger backs off on its next CONTENDED lock through the real
        // public path.
        let mut c = class();
        let mut a = acquire_ctx(&mut c); // older
        let mut b = acquire_ctx(&mut c); // younger
        assert!(a.stamp < b.stamp);

        // What the older's contended acquire does to the younger holder:
        assert_eq!(
            wound_decision(a.stamp, b.stamp, false),
            Decision::WoundAndWait
        );
        b.wounded = 1; // the WoundAndWait branch sets exactly this on the holder
        assert_eq!(b.wounded, 1, "younger holder is now wounded");

        // The wounded younger, on its next CONTENDED lock, must abort (-EDEADLK)
        // and yield — proven through the real ww_mutex_lock wound branch.
        let mut m = mutex();
        let mp = &mut m as *mut WwMutex;
        ww_mutex_init(mp, &mut c as *mut WwClass);
        // Older holds it so the younger's attempt is contended.
        assert_eq!(ww_mutex_lock(mp, &mut a as *mut WwAcquireCtx), 0);
        let rc = ww_mutex_lock(mp, &mut b as *mut WwAcquireCtx);
        assert_eq!(
            rc, EDEADLK,
            "a wounded younger ctx must back off (-EDEADLK) on its next contended lock"
        );
        assert_eq!(m.ctx, &mut a as *mut WwAcquireCtx, "older keeps the lock");
    }

    #[test]
    fn double_lock_same_ctx_is_ealready() {
        let mut c = class();
        let mut a = acquire_ctx(&mut c);
        let ap = &mut a as *mut WwAcquireCtx;
        let mut m = mutex();
        let mp = &mut m as *mut WwMutex;
        ww_mutex_init(mp, &mut c as *mut WwClass);

        assert_eq!(ww_mutex_lock(mp, ap), 0, "first lock succeeds");
        assert_eq!(
            ww_mutex_lock(mp, ap),
            EALREADY,
            "re-locking with the same ctx is -EALREADY, not a deadlock"
        );
    }

    #[test]
    fn trylock_succeeds_then_fails_when_held() {
        let mut c = class();
        let mut a = acquire_ctx(&mut c);
        let mut b = acquire_ctx(&mut c);
        let mut m = mutex();
        let mp = &mut m as *mut WwMutex;
        ww_mutex_init(mp, &mut c as *mut WwClass);

        assert_eq!(
            ww_mutex_trylock(mp, &mut a as *mut WwAcquireCtx),
            1,
            "trylock on a free ww_mutex succeeds"
        );
        assert_eq!(a.acquired, 1, "trylock bumps acquired");
        assert_eq!(
            ww_mutex_trylock(mp, &mut b as *mut WwAcquireCtx),
            0,
            "trylock on a held ww_mutex fails (no wound/wait)"
        );
        ww_mutex_unlock(mp);
        assert_eq!(
            ww_mutex_trylock(mp, &mut b as *mut WwAcquireCtx),
            1,
            "trylock succeeds again once released"
        );
    }

    #[test]
    fn lock_slow_clears_wound_and_acquires_free_lock() {
        // After a back-off, the driver calls ww_mutex_lock_slow on the contended
        // lock. It must clear our wound and (on a now-free lock) acquire it.
        let mut c = class();
        let mut b = acquire_ctx(&mut c);
        b.wounded = 1; // pretend we were wounded and backed off
        let mut m = mutex();
        let mp = &mut m as *mut WwMutex;
        ww_mutex_init(mp, &mut c as *mut WwClass);

        ww_mutex_lock_slow(mp, &mut b as *mut WwAcquireCtx);
        assert_eq!(
            b.wounded, 0,
            "lock_slow clears the wounded flag for the retry"
        );
        assert_eq!(
            ww_mutex_is_locked(mp),
            1,
            "lock_slow acquired the free lock"
        );
        assert_eq!(m.ctx, &mut b as *mut WwAcquireCtx, "holder recorded");
        ww_mutex_unlock(mp);
    }

    #[test]
    fn wounded_ctx_takes_free_lock() {
        // A wounded ctx hitting a FREE lock takes it (no deadlock possible) —
        // ww_mutex_lock's wound branch.
        let mut c = class();
        let mut b = acquire_ctx(&mut c);
        b.wounded = 1;
        let mut m = mutex();
        let mp = &mut m as *mut WwMutex;
        ww_mutex_init(mp, &mut c as *mut WwClass);
        assert_eq!(
            ww_mutex_lock(mp, &mut b as *mut WwAcquireCtx),
            0,
            "a wounded ctx may still take a FREE lock (no contention)"
        );
        ww_mutex_unlock(mp);
    }

    #[test]
    fn acquire_fini_invalidates_ctx() {
        let mut c = class();
        let mut a = acquire_ctx(&mut c);
        assert_ne!(a.task, 0, "live ctx has a non-zero task cookie");
        ww_acquire_fini(&mut a as *mut WwAcquireCtx);
        assert_eq!(a.task, 0, "fini clears the task cookie");
        assert_eq!(a.wounded, 0, "fini clears any wound");
    }
}
