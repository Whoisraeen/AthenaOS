//! `drm_exec` — the DRM "lock these N buffers atomically, with automatic
//! deadlock-backoff retry" helper that wraps the raw `ww_mutex`/`ww_acquire_ctx`
//! loop every modern GPU driver's command-submission path is written around.
//!
//! A GPU submission touches an arbitrary set of buffers (the BOs the command
//! buffer references). To validate + fence them the driver must hold *every*
//! buffer's `dma_resv` lock at once. Locking many `ww_mutex`es in no fixed order
//! would deadlock, so `ww_mutex` hands back `-EDEADLK` to the YOUNGER contender
//! (see `ww_mutex.rs`). `drm_exec` turns that raw protocol into a structured
//! retry loop so drivers stop open-coding it:
//!
//! ```c
//! drm_exec_init(&exec, flags, nr);
//! drm_exec_until_all_locked(&exec) {            // restart point
//!     ret = drm_exec_prepare_array(&exec, objs, nr, 0);
//!     drm_exec_retry_on_contention(&exec);      // if -EDEADLK, `continue` the loop
//!     if (ret) goto error;
//! }
//! ... use the now-fully-locked set ...
//! drm_exec_fini(&exec);
//! ```
//!
//! The contract on contention (the part that MUST be correct):
//!   1. a `_lock_obj` that hits `-EDEADLK` records the *contended* object and
//!      returns `-EDEADLK`;
//!   2. `drm_exec_until_all_locked`/`retry_on_contention` unlocks EVERYTHING this
//!      exec holds and restarts the loop;
//!   3. on the restart, the recorded contended object is `ww_mutex_lock_slow`'d
//!      FIRST (so it is guaranteed on this pass — `prelocked`), seeding the
//!      acquire order so forward progress is monotonic;
//!   4. the loop terminates because each retry locks at least one more "oldest
//!      contended" buffer before any younger one, and the oldest `ww_acquire_ctx`
//!      can never be made to back off.
//!
//! amdgpu's `amdgpu_cs_parser_bos` + GEM validate paths drive exactly this.
//!
//! **Cooperative-daemon modeling.** As with `ww_mutex`, the daemons are
//! cooperatively single-threaded, so we model the *protocol* faithfully (the
//! contended-object bookkeeping, the unlock-all + lock_slow-first restart, the
//! `wounded`→`-EDEADLK` propagation) rather than relying on true preemptive
//! contention. The retry loop is expressed as functions (`..._begin` / `..._loop`
//! / `retry_on_contention`) instead of C macros; a driver written against the
//! macros maps onto them mechanically (`drm_exec_until_all_locked(e){...}` →
//! `while drm_exec_loop_condition(e){ ...; }`).
//!
//! **Layout** (Linux 6.6 x86_64, `pahole`): `struct drm_exec` is
//! `{ u32 flags; struct ww_acquire_ctx ticket; unsigned num_objects;
//!    unsigned max_objects; struct drm_gem_object **objects;
//!    struct drm_gem_object *contended; struct drm_gem_object *prelocked; }`.
//! We pin the field offsets with const-asserts at the bottom. A driver that
//! embeds a `drm_exec` and reads `exec.num_objects` / `exec.objects` after a
//! prepare sees the right words.

use crate::mm;
use crate::ww_mutex::{
    ww_acquire_fini, ww_acquire_init, ww_class_init, ww_mutex_lock, ww_mutex_lock_slow,
    ww_mutex_unlock, WwClass, WwMutex,
};
use core::ptr;
use core::sync::atomic::{AtomicBool, Ordering};

/// `-EDEADLK` — the wound/wait back-off signal surfaced by `ww_mutex_lock`. The
/// caller restarts its `drm_exec_until_all_locked` loop. (Mirror of
/// `ww_mutex::EDEADLK` so a driver only including `drm_exec.h` still sees it.)
pub const EDEADLK: i32 = -35;
/// `-EALREADY` — this exec already locked the object (idempotent re-prepare).
pub const EALREADY: i32 = -114;
/// `-ENOMEM` — the `objects` array could not grow.
pub const ENOMEM: i32 = -12;
/// `-EINVAL` — null exec / bad argument.
pub const EINVAL: i32 = -22;

/// `DRM_EXEC_INTERRUPTIBLE_WAIT` — bit 0 of `flags`. The daemon delivers no
/// signal mid-acquire, so interruptible == uninterruptible here, but we store the
/// flag so a driver reading `exec.flags` sees what it asked for.
pub const DRM_EXEC_INTERRUPTIBLE_WAIT: u32 = 1 << 0;
/// `DRM_EXEC_IGNORE_DUPLICATES` — bit 1: a duplicate `_lock_obj` returns 0
/// instead of `-EALREADY` (amdgpu sets this for its BO list).
pub const DRM_EXEC_IGNORE_DUPLICATES: u32 = 1 << 1;

/// `struct drm_exec` — the execution context: an embedded `ww_acquire_ctx`
/// (`ticket`), the growable array of locked GEM objects, and the
/// contended/prelocked bookkeeping that drives the retry loop.
///
/// In DRM `objects` is `struct drm_gem_object **`, and each GEM object embeds a
/// `dma_resv` whose lock@0 is the `ww_mutex` we take. In this shim a "GEM object"
/// is modeled by its `dma_resv` (the only part `drm_exec` touches) — i.e. a
/// `*mut WwMutex` *is* the lockable handle. `objects` therefore holds the resv
/// ww_mutex pointers, which is byte-identical to holding object pointers whose
/// first embedded lockable member is that ww_mutex.
#[repr(C)]
pub struct DrmExec {
    /// `u32 flags` — `DRM_EXEC_*`.
    pub flags: u32,
    _pad0: u32,
    /// `struct ww_acquire_ctx ticket` — the wound/wait context shared by every
    /// lock this exec takes. Stamped once at `drm_exec_init`.
    pub ticket: crate::ww_mutex::WwAcquireCtx,
    /// `unsigned int num_objects` — how many objects are currently locked.
    pub num_objects: u32,
    /// `unsigned int max_objects` — capacity of `objects`.
    pub max_objects: u32,
    /// `struct drm_gem_object **objects` — the locked set (resv ww_mutex ptrs).
    pub objects: *mut *mut WwMutex,
    /// `struct drm_gem_object *contended` — the object that returned `-EDEADLK`
    /// last pass; it is `lock_slow`'d FIRST on the next pass. Null when none.
    pub contended: *mut WwMutex,
    /// `struct drm_gem_object *prelocked` — the object locked by `lock_slow` at
    /// the top of a retry, to be folded into `objects` by the first `_lock_obj`.
    pub prelocked: *mut WwMutex,
}

// ── C ABI / layout guard (Linux 6.6 x86_64) ──────────────────────────────────
// `ww_acquire_ctx` is 40 bytes (see ww_mutex.rs); with the leading {u32 flags,
// u32 pad} the ticket lands at offset 8, num_objects at 48, objects at 56.
const _: () = assert!(core::mem::offset_of!(DrmExec, flags) == 0);
const _: () = assert!(core::mem::offset_of!(DrmExec, ticket) == 8);
const _: () = assert!(core::mem::offset_of!(DrmExec, num_objects) == 48);
const _: () = assert!(core::mem::offset_of!(DrmExec, max_objects) == 52);
const _: () = assert!(core::mem::offset_of!(DrmExec, objects) == 56);
const _: () = assert!(core::mem::offset_of!(DrmExec, contended) == 64);
const _: () = assert!(core::mem::offset_of!(DrmExec, prelocked) == 72);
const _: () = assert!(core::mem::size_of::<DrmExec>() == 80);

/// The class every `drm_exec` ticket is stamped from. Real DRM uses a single
/// global `DEFINE_WW_CLASS(drm_exec_ww_class)`; we mirror that with one process-
/// wide class seeded on first use via `ww_class_init` (which sets the wound-wait
/// mode and the starting stamp). `stamp` is thereafter only touched through the
/// atomic helper inside `ww_mutex`.
///
/// Zero-initialized; `class_ptr()` seeds it exactly once. (We cannot use a struct
/// literal here because `WwClass::_pad` is private to `ww_mutex` — `zeroed` plus
/// `ww_class_init` is the construction path the crate already supports.)
static mut DRM_EXEC_WW_CLASS: WwClass =
    unsafe { core::mem::MaybeUninit::<WwClass>::zeroed().assume_init() };
static DRM_EXEC_WW_CLASS_INIT: AtomicBool = AtomicBool::new(false);

#[inline]
fn class_ptr() -> *mut WwClass {
    let p = ptr::addr_of_mut!(DRM_EXEC_WW_CLASS);
    // Seed once. In the cooperative daemon there is no preemption mid-init; the
    // AtomicBool guards the (idempotent) seed against repeated re-initialization.
    if DRM_EXEC_WW_CLASS_INIT
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
    {
        ww_class_init(p, 0); // 0 = wound-wait, the DRM default
    }
    p
}

#[inline]
fn ignore_dup(exec: &DrmExec) -> bool {
    exec.flags & DRM_EXEC_IGNORE_DUPLICATES != 0
}

/// Grow `objects` to hold at least `want` entries. Returns true on success.
unsafe fn ensure_capacity(exec: *mut DrmExec, want: u32) -> bool {
    if want <= (*exec).max_objects {
        return true;
    }
    // Double, like upstream's `drm_exec` (amortized O(1) prepare).
    let mut new_cap = (*exec).max_objects.max(4);
    while new_cap < want {
        new_cap = new_cap.saturating_mul(2);
    }
    let new_size = new_cap as usize * core::mem::size_of::<*mut WwMutex>();
    let new_arr = mm::kzalloc(new_size, 0) as *mut *mut WwMutex;
    if new_arr.is_null() {
        return false;
    }
    let old = (*exec).objects;
    if !old.is_null() {
        for i in 0..(*exec).num_objects as usize {
            *new_arr.add(i) = *old.add(i);
        }
        mm::kfree(old as *mut u8);
    }
    (*exec).objects = new_arr;
    (*exec).max_objects = new_cap;
    true
}

/// Is `obj` already in the locked set?
unsafe fn already_locked(exec: *mut DrmExec, obj: *mut WwMutex) -> bool {
    let arr = (*exec).objects;
    if arr.is_null() {
        return false;
    }
    for i in 0..(*exec).num_objects as usize {
        if *arr.add(i) == obj {
            return true;
        }
    }
    false
}

/// Append `obj` to the locked set (caller guarantees it is locked + has room).
unsafe fn push_locked(exec: *mut DrmExec, obj: *mut WwMutex) {
    let i = (*exec).num_objects as usize;
    *(*exec).objects.add(i) = obj;
    (*exec).num_objects += 1;
}

// ── init / fini ───────────────────────────────────────────────────────────────

/// `drm_exec_init(exec, flags, nr)` — start a fresh exec: stamp the ticket, clear
/// the (empty) locked set, and pre-size `objects` to `nr` (a hint; it still
/// grows). `nr == 0` is allowed (array grows lazily).
#[no_mangle]
pub extern "C" fn drm_exec_init(exec: *mut DrmExec, flags: u32, nr: u32) {
    if exec.is_null() {
        return;
    }
    unsafe {
        (*exec).flags = flags;
        (*exec)._pad0 = 0;
        (*exec).num_objects = 0;
        (*exec).max_objects = 0;
        (*exec).objects = ptr::null_mut();
        (*exec).contended = ptr::null_mut();
        (*exec).prelocked = ptr::null_mut();
        ww_acquire_init(ptr::addr_of_mut!((*exec).ticket), class_ptr());
        if nr > 0 {
            // Best-effort pre-size; a null result just means we grow on demand.
            let _ = ensure_capacity(exec, nr);
        }
    }
}

/// `drm_exec_fini(exec)` — unlock everything still held, free the array, and end
/// the ticket. Idempotent: safe to call after `drm_exec_cleanup` already unlocked.
#[no_mangle]
pub extern "C" fn drm_exec_fini(exec: *mut DrmExec) {
    if exec.is_null() {
        return;
    }
    unsafe {
        unlock_all(exec);
        if !(*exec).objects.is_null() {
            mm::kfree((*exec).objects as *mut u8);
            (*exec).objects = ptr::null_mut();
        }
        (*exec).max_objects = 0;
        ww_acquire_fini(ptr::addr_of_mut!((*exec).ticket));
    }
}

/// Unlock every object in the set + any contended/prelocked stragglers, and reset
/// the set to empty (the array allocation is kept for reuse across retries).
unsafe fn unlock_all(exec: *mut DrmExec) {
    let arr = (*exec).objects;
    if !arr.is_null() {
        for i in 0..(*exec).num_objects as usize {
            let obj = *arr.add(i);
            if !obj.is_null() {
                ww_mutex_unlock(obj);
            }
            *arr.add(i) = ptr::null_mut();
        }
    }
    (*exec).num_objects = 0;
    // A prelocked-but-not-yet-folded object must also be released on full unlock.
    if !(*exec).prelocked.is_null() {
        ww_mutex_unlock((*exec).prelocked);
        (*exec).prelocked = ptr::null_mut();
    }
    // NOTE: `contended` is NOT unlocked here — it was never locked by us (the
    // `_lock_obj` attempt that recorded it returned -EDEADLK before taking it).
    // It is consumed (lock_slow'd) at the top of the next retry pass.
}

// ── the retry loop (macro semantics expressed as functions) ───────────────────

/// `drm_exec_until_all_locked(exec) { ... }` begin half. Call once before the
/// loop body each time the loop (re)starts. It consumes any recorded `contended`
/// object by `ww_mutex_lock_slow`'ing it FIRST (the prelocked seed) so it is
/// guaranteed on this pass, establishing the acquire order for the rest.
///
/// In C this is the macro's hidden top-of-loop step; here the driver calls it at
/// the head of each iteration (or relies on `drm_exec_loop_condition` to do so).
#[no_mangle]
pub extern "C" fn drm_exec_until_all_locked_begin(exec: *mut DrmExec) {
    if exec.is_null() {
        return;
    }
    unsafe {
        let contended = (*exec).contended;
        if !contended.is_null() {
            (*exec).contended = ptr::null_mut();
            // lock_slow never returns -EDEADLK: by contract we hold nothing else
            // (unlock_all ran), so blocking on the contended lock cannot deadlock.
            ww_mutex_lock_slow(contended, ptr::addr_of_mut!((*exec).ticket));
            (*exec).prelocked = contended;
        }
    }
}

/// Loop-condition form of `drm_exec_until_all_locked`: returns true while the
/// loop should run another pass and performs the top-of-loop `_begin` step.
///
/// Usage maps the C macro mechanically:
/// ```text
/// drm_exec_until_all_locked(&exec) { BODY }
///   ⇒  while drm_exec_loop_condition(&exec) { BODY }
/// ```
/// It returns true on the first call (enter the loop) and after every pass that
/// hit contention (a `contended` object was recorded → another pass needed); it
/// returns false once a full pass completes with nothing contended.
#[no_mangle]
pub extern "C" fn drm_exec_loop_condition(exec: *mut DrmExec) -> bool {
    if exec.is_null() {
        return false;
    }
    unsafe {
        // First entry: num_objects==0, contended==null, prelocked==null → enter.
        // Subsequent entry after a clean pass: caller signalled completion by
        // NOT leaving a contended object → stop. After a contended pass:
        // `unlock_all` cleared the set and left `contended` set → re-seed + enter.
        let first_entry = (*exec).num_objects == 0
            && (*exec).prelocked.is_null()
            && (*exec).contended.is_null()
            && (*exec).flags & DRM_EXEC_LOOP_STARTED == 0;
        if first_entry {
            (*exec).flags |= DRM_EXEC_LOOP_STARTED;
            drm_exec_until_all_locked_begin(exec);
            return true;
        }
        if !(*exec).contended.is_null() {
            // A `retry_on_contention` was invoked: unlock everything, re-seed.
            unlock_all(exec);
            drm_exec_until_all_locked_begin(exec);
            return true;
        }
        // Clean pass completed.
        (*exec).flags &= !DRM_EXEC_LOOP_STARTED;
        false
    }
}

/// Internal flag bit (above the public `DRM_EXEC_*` flags) marking that the
/// loop-condition helper has been entered at least once this run.
const DRM_EXEC_LOOP_STARTED: u32 = 1 << 31;

/// `drm_exec_retry_on_contention(exec)` — the macro that, inside the loop body,
/// turns a `-EDEADLK` from a prepare/lock call into a loop restart. In C it is a
/// `if (unlikely(ret == -EDEADLK)) continue;`. Here it returns true if the caller
/// should `continue` (restart the loop); the actual unlock-all + re-seed is done
/// by `drm_exec_loop_condition` on the next pass (which sees the recorded
/// `contended` object). Returns false if there was no contention.
#[no_mangle]
pub extern "C" fn drm_exec_retry_on_contention(exec: *mut DrmExec) -> bool {
    if exec.is_null() {
        return false;
    }
    // Contention is recorded by `_lock_obj` setting `contended`. If set, the
    // caller must restart (and the next loop_condition() will unlock+re-seed).
    unsafe { !(*exec).contended.is_null() }
}

// ── locking one / many objects ────────────────────────────────────────────────

/// `drm_exec_lock_obj(exec, obj)` — lock one GEM object's resv ww_mutex under the
/// exec ticket and add it to the set. Returns 0 on success, `-EALREADY` if
/// already locked (unless `IGNORE_DUPLICATES`, then 0), `-EDEADLK` on contention
/// (the object is recorded as `contended`; caller must
/// `drm_exec_retry_on_contention`), or `-ENOMEM` if the set could not grow.
#[no_mangle]
pub extern "C" fn drm_exec_lock_obj(exec: *mut DrmExec, obj: *mut WwMutex) -> i32 {
    if exec.is_null() || obj.is_null() {
        return EINVAL;
    }
    unsafe {
        // If this is the object we lock_slow'd at the top of the retry, it is
        // already held — just fold it into the set (don't re-lock).
        if (*exec).prelocked == obj {
            (*exec).prelocked = ptr::null_mut();
            if !ensure_capacity(exec, (*exec).num_objects + 1) {
                ww_mutex_unlock(obj);
                return ENOMEM;
            }
            push_locked(exec, obj);
            return 0;
        }

        if already_locked(exec, obj) {
            return if ignore_dup(&*exec) { 0 } else { EALREADY };
        }

        let rc = ww_mutex_lock(obj, ptr::addr_of_mut!((*exec).ticket));
        match rc {
            0 => {
                if !ensure_capacity(exec, (*exec).num_objects + 1) {
                    ww_mutex_unlock(obj);
                    return ENOMEM;
                }
                push_locked(exec, obj);
                0
            }
            EDEADLK => {
                // Record the contended object so the retry pass lock_slow's it
                // first. The standard drm_exec contract.
                (*exec).contended = obj;
                EDEADLK
            }
            EALREADY => {
                if ignore_dup(&*exec) {
                    0
                } else {
                    EALREADY
                }
            }
            other => other,
        }
    }
}

/// `drm_exec_unlock_obj(exec, obj)` — drop one object from the set (and unlock
/// it). Returns 0 if removed, `-EINVAL` if not present. Rarely used directly;
/// most drivers unlock the whole set via `drm_exec_fini`.
#[no_mangle]
pub extern "C" fn drm_exec_unlock_obj(exec: *mut DrmExec, obj: *mut WwMutex) -> i32 {
    if exec.is_null() || obj.is_null() {
        return EINVAL;
    }
    unsafe {
        let arr = (*exec).objects;
        if arr.is_null() {
            return EINVAL;
        }
        let n = (*exec).num_objects as usize;
        let mut found = None;
        for i in 0..n {
            if *arr.add(i) == obj {
                found = Some(i);
                break;
            }
        }
        let i = match found {
            Some(i) => i,
            None => return EINVAL,
        };
        ww_mutex_unlock(obj);
        // Compact: move the last entry into the hole.
        *arr.add(i) = *arr.add(n - 1);
        *arr.add(n - 1) = ptr::null_mut();
        (*exec).num_objects -= 1;
        0
    }
}

/// `drm_exec_prepare_obj(exec, obj, num_fences)` — lock the object and reserve
/// room for `num_fences` fences on its resv. In this shim the fence reservation
/// is the caller's concern (via `dma_resv_reserve_fences`); we perform the lock
/// (the deadlock-prone part) and accept `num_fences` for ABI compatibility.
/// Returns the same codes as `drm_exec_lock_obj`.
#[no_mangle]
pub extern "C" fn drm_exec_prepare_obj(
    exec: *mut DrmExec,
    obj: *mut WwMutex,
    _num_fences: u32,
) -> i32 {
    drm_exec_lock_obj(exec, obj)
}

/// `drm_exec_prepare_array(exec, objects, num_objects, num_fences)` — the bulk CS
/// path: lock all `num_objects` in order. On the FIRST `-EDEADLK` it stops and
/// returns `-EDEADLK` with the contended object recorded (the caller's
/// `drm_exec_retry_on_contention` restarts the whole prepare on the next pass,
/// where the contended object is locked first). Returns 0 once all are locked.
#[no_mangle]
pub extern "C" fn drm_exec_prepare_array(
    exec: *mut DrmExec,
    objects: *const *mut WwMutex,
    num_objects: u32,
    num_fences: u32,
) -> i32 {
    if exec.is_null() || (objects.is_null() && num_objects > 0) {
        return EINVAL;
    }
    unsafe {
        for i in 0..num_objects as usize {
            let obj = *objects.add(i);
            let rc = drm_exec_prepare_obj(exec, obj, num_fences);
            if rc == EDEADLK {
                return EDEADLK; // stop; caller retries the whole array
            }
            if rc != 0 && rc != EALREADY {
                return rc; // hard error (-ENOMEM / -EINVAL)
            }
        }
        0
    }
}

/// `drm_exec_cleanup(exec)` — unlock everything currently held WITHOUT freeing
/// the array or ending the ticket, so the exec can be re-run (a manual restart
/// outside the loop helpers). After this `num_objects == 0`.
#[no_mangle]
pub extern "C" fn drm_exec_cleanup(exec: *mut DrmExec) {
    if exec.is_null() {
        return;
    }
    unsafe {
        unlock_all(exec);
    }
}

#[cfg(test)]
mod tests {
    //! Pure host KATs (no syscall path: every lock here is either uncontended or
    //! a contended attempt against a lock that STAYS held, which returns its
    //! decision `-EDEADLK` synchronously without entering the cooperative
    //! `msleep` block loop — see ww_mutex.rs's tests for the same discipline).
    use super::*;
    use crate::ww_mutex::{ww_mutex_init, ww_mutex_is_locked, EDEADLK as WW_EDEADLK};

    fn exec_zeroed() -> DrmExec {
        // Safe: `ww_acquire_init` (run by drm_exec_init) overwrites `ticket`.
        unsafe { core::mem::zeroed() }
    }
    fn mutex() -> WwMutex {
        WwMutex {
            base: [0u8; 32],
            ctx: ptr::null_mut(),
        }
    }

    #[test]
    fn init_stamps_ticket_and_starts_empty() {
        let mut e = exec_zeroed();
        let ep = &mut e as *mut DrmExec;
        drm_exec_init(ep, 0, 4);
        assert_eq!(e.num_objects, 0, "fresh exec locks nothing");
        assert!(e.contended.is_null(), "no contended object yet");
        assert!(e.prelocked.is_null(), "no prelocked object yet");
        assert_ne!(e.ticket.stamp, 0, "ticket got a live (non-reserved) stamp");
        assert!(e.max_objects >= 4, "nr hint pre-sized the array");
        assert!(!e.objects.is_null(), "nr hint allocated the array");
        drm_exec_fini(ep);
        assert!(e.objects.is_null(), "fini freed the array");
    }

    #[test]
    fn prepare_array_locks_all_uncontended() {
        // The happy CS path: one exec, N free buffers, all lock first try.
        let mut e = exec_zeroed();
        let ep = &mut e as *mut DrmExec;
        drm_exec_init(ep, 0, 0);

        let mut m = [mutex(), mutex(), mutex(), mutex()];
        let mut objs = [ptr::null_mut::<WwMutex>(); 4];
        for i in 0..4 {
            let mp = &mut m[i] as *mut WwMutex;
            ww_mutex_init(mp, class_ptr());
            objs[i] = mp;
        }

        let rc = drm_exec_prepare_array(ep, objs.as_ptr(), 4, 0);
        assert_eq!(rc, 0, "all-free prepare_array succeeds");
        assert_eq!(e.num_objects, 4, "all four objects are in the locked set");
        for i in 0..4 {
            assert_eq!(ww_mutex_is_locked(objs[i]), 1, "object {i} is locked");
        }

        drm_exec_fini(ep);
        for i in 0..4 {
            assert_eq!(ww_mutex_is_locked(objs[i]), 0, "fini unlocked object {i}");
        }
    }

    #[test]
    fn contention_unlocks_all_then_lock_slow_contended_first_then_succeeds() {
        // The core drm_exec contract. A YOUNGER exec contends on obj[1], which an
        // OLDER ctx already holds → -EDEADLK on the first pass. The exec must:
        //   1) record obj[1] as contended,
        //   2) unlock everything it took on pass 1 (obj[0]),
        //   3) lock_slow obj[1] FIRST on the retry (prelocked),
        //   4) then lock the rest → all four locked, contended cleared.
        let mut older = exec_zeroed();
        let op = &mut older as *mut DrmExec;
        drm_exec_init(op, 0, 0); // OLDER (lower stamp — inited first)

        let mut younger = exec_zeroed();
        let yp = &mut younger as *mut DrmExec;
        drm_exec_init(yp, 0, 0); // YOUNGER (higher stamp)
        assert!(
            older.ticket.stamp < younger.ticket.stamp,
            "first-inited exec is older"
        );

        let mut m = [mutex(), mutex(), mutex(), mutex()];
        let mut objs = [ptr::null_mut::<WwMutex>(); 4];
        for i in 0..4 {
            let mp = &mut m[i] as *mut WwMutex;
            ww_mutex_init(mp, class_ptr());
            objs[i] = mp;
        }

        // OLDER pre-holds obj[1] (the one the younger will contend on) and keeps it.
        assert_eq!(
            ww_mutex_lock(objs[1], ptr::addr_of_mut!(older.ticket)),
            0,
            "older takes obj[1]"
        );

        // ── PASS 1 (younger) ──
        assert!(drm_exec_loop_condition(yp), "first loop entry");
        // obj[0]: free → locked.
        assert_eq!(drm_exec_lock_obj(yp, objs[0]), 0, "obj[0] locks free");
        assert_eq!(younger.num_objects, 1);
        // obj[1]: held by OLDER, younger contends → -EDEADLK, recorded contended.
        assert_eq!(
            drm_exec_lock_obj(yp, objs[1]),
            WW_EDEADLK,
            "obj[1] contended → -EDEADLK"
        );
        assert_eq!(younger.contended, objs[1], "contended object recorded");
        assert!(
            drm_exec_retry_on_contention(yp),
            "retry_on_contention signals a restart"
        );

        // Older now releases obj[1] so the younger's lock_slow can take it.
        ww_mutex_unlock(objs[1]);

        // ── PASS 2 (younger restart) ──
        // loop_condition must: unlock all pass-1 locks, then lock_slow contended.
        assert!(drm_exec_loop_condition(yp), "retry re-enters the loop");
        assert_eq!(
            ww_mutex_is_locked(objs[0]),
            0,
            "pass-1 obj[0] was unlocked on retry"
        );
        assert!(younger.contended.is_null(), "contended consumed");
        // obj[1] is now prelocked (lock_slow'd FIRST) — it is ACQUIRED before the
        // prepare_array loop even runs. THIS is the deadlock-avoidance guarantee:
        // the contended object is locked first on the retry, before any other
        // buffer of this pass is touched.
        assert_eq!(younger.prelocked, objs[1], "contended is prelocked first");
        assert_eq!(
            ww_mutex_is_locked(objs[1]),
            1,
            "the contended object is LOCKED FIRST on retry (before prepare_array)"
        );
        // Nothing else of this pass is locked yet — proves obj[1] was strictly first.
        assert_eq!(ww_mutex_is_locked(objs[0]), 0, "obj[0] not yet (re)locked");
        assert_eq!(ww_mutex_is_locked(objs[2]), 0, "obj[2] not yet locked");
        assert_eq!(ww_mutex_is_locked(objs[3]), 0, "obj[3] not yet locked");

        // Re-prepare the whole array; obj[1] folds in from prelocked, rest lock.
        let rc = drm_exec_prepare_array(yp, objs.as_ptr(), 4, 0);
        assert_eq!(rc, 0, "second pass locks everything");
        assert!(younger.prelocked.is_null(), "prelocked folded into the set");
        assert_eq!(younger.num_objects, 4, "all four locked on the retry");
        for i in 0..4 {
            assert_eq!(ww_mutex_is_locked(objs[i]), 1, "obj[{i}] locked on pass 2");
        }
        // obj[1] is present in the locked set (folded in from prelocked). Its array
        // position is insertion order; the load-bearing guarantee (locked FIRST)
        // was asserted above via the lock state before prepare_array ran.
        unsafe {
            let mut found = false;
            for i in 0..younger.num_objects as usize {
                if *younger.objects.add(i) == objs[1] {
                    found = true;
                    break;
                }
            }
            assert!(
                found,
                "the contended object is in the locked set after retry"
            );
        }

        // ── loop terminates ──
        assert!(
            !drm_exec_loop_condition(yp),
            "a clean pass (no contended) terminates the loop — no infinite retry"
        );

        drm_exec_fini(yp);
        drm_exec_fini(op);
    }

    #[test]
    fn loop_terminates_with_no_contention() {
        // A bounded retry loop driven purely by loop_condition over a free set:
        // exactly one body pass, then termination. Proves no infinite spin.
        let mut e = exec_zeroed();
        let ep = &mut e as *mut DrmExec;
        drm_exec_init(ep, 0, 0);
        let mut m = [mutex(), mutex()];
        let mut objs = [ptr::null_mut::<WwMutex>(); 2];
        for i in 0..2 {
            let mp = &mut m[i] as *mut WwMutex;
            ww_mutex_init(mp, class_ptr());
            objs[i] = mp;
        }

        let mut passes = 0;
        while drm_exec_loop_condition(ep) {
            passes += 1;
            assert!(passes <= 8, "loop must terminate, not spin");
            let rc = drm_exec_prepare_array(ep, objs.as_ptr(), 2, 0);
            if drm_exec_retry_on_contention(ep) {
                continue;
            }
            assert_eq!(rc, 0);
        }
        assert_eq!(passes, 1, "uncontended set needs exactly one pass");
        assert_eq!(e.num_objects, 2, "both locked");
        drm_exec_fini(ep);
    }

    #[test]
    fn duplicate_lock_obj_is_ealready_unless_ignore_flag() {
        let mut e = exec_zeroed();
        let ep = &mut e as *mut DrmExec;
        drm_exec_init(ep, 0, 0);
        let mut m = mutex();
        let mp = &mut m as *mut WwMutex;
        ww_mutex_init(mp, class_ptr());

        assert_eq!(drm_exec_lock_obj(ep, mp), 0, "first lock");
        assert_eq!(
            drm_exec_lock_obj(ep, mp),
            EALREADY,
            "duplicate without IGNORE flag is -EALREADY"
        );
        assert_eq!(e.num_objects, 1, "duplicate did not double-add");
        drm_exec_fini(ep);

        // With IGNORE_DUPLICATES the duplicate returns 0.
        let mut e2 = exec_zeroed();
        let e2p = &mut e2 as *mut DrmExec;
        drm_exec_init(e2p, DRM_EXEC_IGNORE_DUPLICATES, 0);
        let mut m2 = mutex();
        let m2p = &mut m2 as *mut WwMutex;
        ww_mutex_init(m2p, class_ptr());
        assert_eq!(drm_exec_lock_obj(e2p, m2p), 0, "first lock");
        assert_eq!(
            drm_exec_lock_obj(e2p, m2p),
            0,
            "duplicate with IGNORE_DUPLICATES is silently 0"
        );
        assert_eq!(e2.num_objects, 1, "still only one entry");
        drm_exec_fini(e2p);
    }

    #[test]
    fn cleanup_unlocks_all_keeps_array_then_fini() {
        let mut e = exec_zeroed();
        let ep = &mut e as *mut DrmExec;
        drm_exec_init(ep, 0, 0);
        let mut m = [mutex(), mutex(), mutex()];
        let mut objs = [ptr::null_mut::<WwMutex>(); 3];
        for i in 0..3 {
            let mp = &mut m[i] as *mut WwMutex;
            ww_mutex_init(mp, class_ptr());
            objs[i] = mp;
        }
        assert_eq!(drm_exec_prepare_array(ep, objs.as_ptr(), 3, 0), 0);
        assert_eq!(e.num_objects, 3);
        let arr_before = e.objects;

        drm_exec_cleanup(ep);
        assert_eq!(e.num_objects, 0, "cleanup empties the set");
        for i in 0..3 {
            assert_eq!(ww_mutex_is_locked(objs[i]), 0, "cleanup unlocked obj[{i}]");
        }
        assert_eq!(e.objects, arr_before, "cleanup keeps the array for reuse");
        assert!(!e.objects.is_null(), "array still allocated after cleanup");

        drm_exec_fini(ep);
        assert!(e.objects.is_null(), "fini frees the array");
    }

    #[test]
    fn unlock_obj_removes_one_from_set() {
        let mut e = exec_zeroed();
        let ep = &mut e as *mut DrmExec;
        drm_exec_init(ep, 0, 0);
        let mut m = [mutex(), mutex(), mutex()];
        let mut objs = [ptr::null_mut::<WwMutex>(); 3];
        for i in 0..3 {
            let mp = &mut m[i] as *mut WwMutex;
            ww_mutex_init(mp, class_ptr());
            objs[i] = mp;
        }
        assert_eq!(drm_exec_prepare_array(ep, objs.as_ptr(), 3, 0), 0);

        assert_eq!(drm_exec_unlock_obj(ep, objs[1]), 0, "remove the middle one");
        assert_eq!(e.num_objects, 2, "set shrank by one");
        assert_eq!(ww_mutex_is_locked(objs[1]), 0, "removed object is unlocked");
        assert_eq!(ww_mutex_is_locked(objs[0]), 1, "others stay locked");
        assert_eq!(ww_mutex_is_locked(objs[2]), 1, "others stay locked");
        assert_eq!(
            drm_exec_unlock_obj(ep, objs[1]),
            EINVAL,
            "removing a non-member is -EINVAL"
        );
        drm_exec_fini(ep);
    }

    #[test]
    fn prepare_obj_grows_array_past_nr_hint() {
        // nr hint of 1 but lock 5 → array must grow (no -ENOMEM, all locked).
        let mut e = exec_zeroed();
        let ep = &mut e as *mut DrmExec;
        drm_exec_init(ep, 0, 1);
        let mut m = [mutex(), mutex(), mutex(), mutex(), mutex()];
        for i in 0..5 {
            let mp = &mut m[i] as *mut WwMutex;
            ww_mutex_init(mp, class_ptr());
            assert_eq!(drm_exec_prepare_obj(ep, mp, 0), 0, "obj {i} locks + grows");
        }
        assert_eq!(e.num_objects, 5, "all five locked despite nr hint of 1");
        assert!(e.max_objects >= 5, "array grew to fit");
        drm_exec_fini(ep);
    }
}
