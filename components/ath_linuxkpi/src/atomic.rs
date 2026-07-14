//! Linux `atomic_t` / `atomic64_t` operations + memory barriers.
//!
//! Linux's `atomic_t` is `struct { int counter; }` — a driver passes `&atomic`
//! and the kernel does the RMW. We mirror that: each function takes the pointer
//! to the counter word and performs the operation with `core::sync::atomic`
//! intrinsics (SeqCst, matching Linux's fully-ordered `atomic_*` defaults).
//! Barriers map to real `core::sync::atomic::fence` + `compiler_fence`.

use core::sync::atomic::{compiler_fence, fence, AtomicI32, AtomicI64, Ordering};

#[inline]
unsafe fn a32<'a>(p: *mut i32) -> &'a AtomicI32 {
    &*(p as *const AtomicI32)
}
#[inline]
unsafe fn a64<'a>(p: *mut i64) -> &'a AtomicI64 {
    &*(p as *const AtomicI64)
}

// ── atomic_t (32-bit) ────────────────────────────────────────────────────────

#[no_mangle]
pub unsafe extern "C" fn atomic_read(v: *const i32) -> i32 {
    (*(v as *const AtomicI32)).load(Ordering::SeqCst)
}
#[no_mangle]
pub unsafe extern "C" fn atomic_set(v: *mut i32, i: i32) {
    a32(v).store(i, Ordering::SeqCst)
}
#[no_mangle]
pub unsafe extern "C" fn atomic_add(i: i32, v: *mut i32) {
    a32(v).fetch_add(i, Ordering::SeqCst);
}
#[no_mangle]
pub unsafe extern "C" fn atomic_sub(i: i32, v: *mut i32) {
    a32(v).fetch_sub(i, Ordering::SeqCst);
}
#[no_mangle]
pub unsafe extern "C" fn atomic_inc(v: *mut i32) {
    a32(v).fetch_add(1, Ordering::SeqCst);
}
#[no_mangle]
pub unsafe extern "C" fn atomic_dec(v: *mut i32) {
    a32(v).fetch_sub(1, Ordering::SeqCst);
}
#[no_mangle]
pub unsafe extern "C" fn atomic_add_return(i: i32, v: *mut i32) -> i32 {
    a32(v).fetch_add(i, Ordering::SeqCst) + i
}
#[no_mangle]
pub unsafe extern "C" fn atomic_sub_return(i: i32, v: *mut i32) -> i32 {
    a32(v).fetch_sub(i, Ordering::SeqCst) - i
}
#[no_mangle]
pub unsafe extern "C" fn atomic_inc_return(v: *mut i32) -> i32 {
    a32(v).fetch_add(1, Ordering::SeqCst) + 1
}
#[no_mangle]
pub unsafe extern "C" fn atomic_dec_return(v: *mut i32) -> i32 {
    a32(v).fetch_sub(1, Ordering::SeqCst) - 1
}
#[no_mangle]
pub unsafe extern "C" fn atomic_cmpxchg(v: *mut i32, old: i32, new: i32) -> i32 {
    match a32(v).compare_exchange(old, new, Ordering::SeqCst, Ordering::SeqCst) {
        Ok(prev) => prev,
        Err(prev) => prev,
    }
}
#[no_mangle]
pub unsafe extern "C" fn atomic_xchg(v: *mut i32, new: i32) -> i32 {
    a32(v).swap(new, Ordering::SeqCst)
}
/// `atomic_dec_and_test` — decrement and return true (1) if the result is 0.
#[no_mangle]
pub unsafe extern "C" fn atomic_dec_and_test(v: *mut i32) -> i32 {
    (a32(v).fetch_sub(1, Ordering::SeqCst) - 1 == 0) as i32
}

// ── atomic64_t (64-bit) ──────────────────────────────────────────────────────

#[no_mangle]
pub unsafe extern "C" fn atomic64_read(v: *const i64) -> i64 {
    (*(v as *const AtomicI64)).load(Ordering::SeqCst)
}
#[no_mangle]
pub unsafe extern "C" fn atomic64_set(v: *mut i64, i: i64) {
    a64(v).store(i, Ordering::SeqCst)
}
#[no_mangle]
pub unsafe extern "C" fn atomic64_add(i: i64, v: *mut i64) {
    a64(v).fetch_add(i, Ordering::SeqCst);
}
#[no_mangle]
pub unsafe extern "C" fn atomic64_inc_return(v: *mut i64) -> i64 {
    a64(v).fetch_add(1, Ordering::SeqCst) + 1
}

// ── Memory barriers ──────────────────────────────────────────────────────────
// Linux mb()/rmb()/wmb() are full/load/store CPU fences; the smp_* variants are
// identical on a coherent SMP x86 target. Compiler barriers prevent reordering.

#[no_mangle]
pub extern "C" fn __lkpi_mb() {
    fence(Ordering::SeqCst);
}
#[no_mangle]
pub extern "C" fn __lkpi_rmb() {
    fence(Ordering::Acquire);
}
#[no_mangle]
pub extern "C" fn __lkpi_wmb() {
    fence(Ordering::Release);
}
#[no_mangle]
pub extern "C" fn __lkpi_barrier() {
    compiler_fence(Ordering::SeqCst);
}

/// `test_and_set_bit` — set bit `nr` in the long-array at `addr`, return old value.
#[no_mangle]
pub unsafe extern "C" fn test_and_set_bit(nr: i64, addr: *mut u64) -> i32 {
    let word = addr.add((nr as usize) / 64);
    let mask = 1u64 << ((nr as usize) % 64);
    let a = &*(word as *const core::sync::atomic::AtomicU64);
    let prev = a.fetch_or(mask, Ordering::SeqCst);
    ((prev & mask) != 0) as i32
}

/// `test_and_clear_bit`.
#[no_mangle]
pub unsafe extern "C" fn test_and_clear_bit(nr: i64, addr: *mut u64) -> i32 {
    let word = addr.add((nr as usize) / 64);
    let mask = 1u64 << ((nr as usize) % 64);
    let a = &*(word as *const core::sync::atomic::AtomicU64);
    let prev = a.fetch_and(!mask, Ordering::SeqCst);
    ((prev & mask) != 0) as i32
}

/// `set_bit` / `clear_bit` (no return).
#[no_mangle]
pub unsafe extern "C" fn set_bit(nr: i64, addr: *mut u64) {
    let word = addr.add((nr as usize) / 64);
    let mask = 1u64 << ((nr as usize) % 64);
    (*(word as *const core::sync::atomic::AtomicU64)).fetch_or(mask, Ordering::SeqCst);
}
#[no_mangle]
pub unsafe extern "C" fn clear_bit(nr: i64, addr: *mut u64) {
    let word = addr.add((nr as usize) / 64);
    let mask = 1u64 << ((nr as usize) % 64);
    (*(word as *const core::sync::atomic::AtomicU64)).fetch_and(!mask, Ordering::SeqCst);
}
