//! `refcount_t` + `kref` — atomic reference counts over driver-owned storage.
//!
//! Object lifetime in DRM/TTM/amdgpu is reference-counted: `drm_device`,
//! `dma_fence`, GEM objects, and connectors all embed a `kref` (which wraps a
//! `refcount_t`) and free themselves when the last reference drops. The shim
//! must implement the real saturating-atomic semantics — a non-atomic or
//! "always 1" stub would either leak every object or free a live one.
//!
//! Linux `refcount_t` is `struct { atomic_t refs; }` — a single 32-bit word the
//! driver owns; `struct kref` is `{ refcount_t refcount; }`, so a `kref *` and
//! its `refcount_t *` share an address. We operate on that word directly via an
//! `AtomicI32` view (no shim-side allocation), matching the lock-word approach
//! in `sync.rs`. Counts saturate instead of wrapping (Linux pins a
//! use-after-free at `REFCOUNT_SATURATED` rather than overflowing to a valid
//! count); the daemon is single address space, so a wrap would be a security
//! bug, not a glitch.

use core::sync::atomic::{AtomicI32, Ordering};

#[inline]
unsafe fn refs<'a>(p: *mut i32) -> &'a AtomicI32 {
    &*(p as *const AtomicI32)
}

/// `refcount_set(r, n)`.
#[no_mangle]
pub extern "C" fn refcount_set(r: *mut i32, n: i32) {
    if !r.is_null() {
        unsafe { refs(r) }.store(n, Ordering::Release);
    }
}

/// `refcount_read(r)`.
#[no_mangle]
pub extern "C" fn refcount_read(r: *mut i32) -> i32 {
    if r.is_null() {
        return 0;
    }
    unsafe { refs(r) }.load(Ordering::Acquire)
}

/// `refcount_add(i, r)` — saturating, never wraps past `i32::MAX`.
#[no_mangle]
pub extern "C" fn refcount_add(i: i32, r: *mut i32) {
    if r.is_null() {
        return;
    }
    let a = unsafe { refs(r) };
    let mut cur = a.load(Ordering::Relaxed);
    loop {
        let next = cur.saturating_add(i).max(0);
        match a.compare_exchange_weak(cur, next, Ordering::AcqRel, Ordering::Relaxed) {
            Ok(_) => return,
            Err(e) => cur = e,
        }
    }
}

/// `refcount_inc(r)`.
#[no_mangle]
pub extern "C" fn refcount_inc(r: *mut i32) {
    refcount_add(1, r);
}

/// `refcount_dec(r)` — decrement, clamping at 0 (a dec-to-0 should have used
/// `refcount_dec_and_test`; clamping avoids a negative count).
#[no_mangle]
pub extern "C" fn refcount_dec(r: *mut i32) {
    refcount_add(-1, r);
}

/// `refcount_inc_not_zero(r)` → true if the count was non-zero and incremented.
/// The guard against resurrecting a freed object (count already 0).
#[no_mangle]
pub extern "C" fn refcount_inc_not_zero(r: *mut i32) -> bool {
    if r.is_null() {
        return false;
    }
    let a = unsafe { refs(r) };
    let mut cur = a.load(Ordering::Relaxed);
    loop {
        if cur == 0 {
            return false;
        }
        let next = cur.saturating_add(1);
        match a.compare_exchange_weak(cur, next, Ordering::AcqRel, Ordering::Relaxed) {
            Ok(_) => return true,
            Err(e) => cur = e,
        }
    }
}

/// `refcount_sub_and_test(i, r)` → true if the count reached exactly 0.
#[no_mangle]
pub extern "C" fn refcount_sub_and_test(i: i32, r: *mut i32) -> bool {
    if r.is_null() {
        return false;
    }
    let a = unsafe { refs(r) };
    let mut cur = a.load(Ordering::Relaxed);
    loop {
        let next = cur.saturating_sub(i).max(0);
        match a.compare_exchange_weak(cur, next, Ordering::AcqRel, Ordering::Relaxed) {
            Ok(_) => return next == 0,
            Err(e) => cur = e,
        }
    }
}

/// `refcount_dec_and_test(r)` → true if this decrement dropped the count to 0
/// (the caller now owns teardown).
#[no_mangle]
pub extern "C" fn refcount_dec_and_test(r: *mut i32) -> bool {
    refcount_sub_and_test(1, r)
}

// ── kref (struct kref { refcount_t refcount; }) ──────────────────────────────
// A kref* aliases its refcount_t* (first and only member).

/// Release callback invoked by `kref_put` when the count hits 0.
pub type KrefRelease = extern "C" fn(*mut i32);

/// `kref_init(kref)` — start at 1.
#[no_mangle]
pub extern "C" fn kref_init(kref: *mut i32) {
    refcount_set(kref, 1);
}

/// `kref_read(kref)`.
#[no_mangle]
pub extern "C" fn kref_read(kref: *mut i32) -> i32 {
    refcount_read(kref)
}

/// `kref_get(kref)`.
#[no_mangle]
pub extern "C" fn kref_get(kref: *mut i32) {
    refcount_inc(kref);
}

/// `kref_get_unless_zero(kref)` → 1 if a live reference was taken.
#[no_mangle]
pub extern "C" fn kref_get_unless_zero(kref: *mut i32) -> i32 {
    refcount_inc_not_zero(kref) as i32
}

/// `kref_put(kref, release)` — drop a reference; on the last one, invoke
/// `release(kref)` and return 1. Returns 0 if references remain.
#[no_mangle]
pub extern "C" fn kref_put(kref: *mut i32, release: Option<KrefRelease>) -> i32 {
    if refcount_dec_and_test(kref) {
        if let Some(rel) = release {
            rel(kref);
        }
        1
    } else {
        0
    }
}

/// `refcount_warn_saturate(r, type)` — the slow-path the inlined
/// `refcount_*_checked` helpers call when a count saturates or underflows (a
/// use-after-free signal). Our RMW path already saturates safely, so this just
/// reports; exported so a real driver's inlined refcount sites link.
#[no_mangle]
pub extern "C" fn refcount_warn_saturate(_r: *mut i32, _saturation_type: u32) {
    let msg = b"[linuxkpi] refcount_warn_saturate: count saturated (use-after-free?)\n";
    unsafe { crate::host::sys_linuxkpi_printk(msg.as_ptr(), msg.len() as u64) };
}
