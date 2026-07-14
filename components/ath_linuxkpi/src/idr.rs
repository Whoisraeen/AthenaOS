//! ID allocators — `ida` (ID bitmap) and `idr` (ID → pointer map).
//!
//! Drivers hand out small dense integer IDs constantly: DRM minor numbers, GEM
//! handles, connector/CRTC indices, fence contexts. `ida` answers "give me the
//! lowest free integer in a range"; `idr` additionally maps each ID back to a
//! pointer. amdgpu/DRM lean on both, so the shim implements the real allocation
//! semantics (lowest-free, range-bounded, freeing reuses the slot) — not a
//! monotonic counter that never reuses and eventually overflows.
//!
//! **Daemon model.** Linux's `struct ida`/`struct idr` are opaque to drivers
//! (an `xarray` inside), allocated by the driver and zero-initialized by
//! `DEFINE_IDA`/`ida_init`. We only ever touch the FIRST machine word, using it
//! as a lazily-allocated handle to shim-side state on the daemon heap (`mm`).
//! The C structs are ≥3 words, so writing the first word is safe; zero-init
//! means "uninitialized" and the first allocation creates the backing store.
//! Single-threaded cooperative daemon → no locking needed around the state.

use crate::mm;
use core::ptr;

/// Linux negative errnos returned in the `int` result.
const ENOSPC: i32 = -28;
const ENOMEM: i32 = -12;
/// Default upper bound when a caller passes "no max".
const ID_MAX: i32 = i32::MAX;

// ── ida — a growable bitmap of allocated IDs ─────────────────────────────────

/// Shim-side `ida` state: a bitmap of `cap_bits` slots on the daemon heap.
#[repr(C)]
struct IdaState {
    words: *mut u64,
    cap_bits: u32,
}

const IDA_INIT_BITS: u32 = 64;

#[inline]
fn words_for(bits: u32) -> usize {
    (bits as usize).div_ceil(64)
}

/// Fetch (and lazily create) the `IdaState` behind a `struct ida`'s first word.
/// Returns null only if the heap is exhausted.
unsafe fn ida_state(ida: *mut usize) -> *mut IdaState {
    if ida.is_null() {
        return ptr::null_mut();
    }
    let handle = *ida;
    if handle != 0 {
        return handle as *mut IdaState;
    }
    let st = mm::kzalloc(core::mem::size_of::<IdaState>(), 0) as *mut IdaState;
    if st.is_null() {
        return ptr::null_mut();
    }
    let nwords = words_for(IDA_INIT_BITS);
    let words = mm::kzalloc(nwords * 8, 0) as *mut u64;
    if words.is_null() {
        mm::kfree(st as *mut u8);
        return ptr::null_mut();
    }
    (*st).words = words;
    (*st).cap_bits = IDA_INIT_BITS;
    *ida = st as usize;
    st
}

/// Grow an `IdaState` bitmap to cover at least `need_bits` (zeroed tail = free).
unsafe fn ida_grow(st: *mut IdaState, need_bits: u32) -> bool {
    if need_bits <= (*st).cap_bits {
        return true;
    }
    let mut new_bits = (*st).cap_bits;
    while new_bits < need_bits {
        new_bits = new_bits.saturating_mul(2);
        if new_bits == 0 {
            new_bits = need_bits;
            break;
        }
    }
    let old_words = words_for((*st).cap_bits);
    let new_words = words_for(new_bits);
    let fresh = mm::kzalloc(new_words * 8, 0) as *mut u64;
    if fresh.is_null() {
        return false;
    }
    ptr::copy_nonoverlapping((*st).words, fresh, old_words);
    mm::kfree((*st).words as *mut u8);
    (*st).words = fresh;
    (*st).cap_bits = new_bits;
    true
}

#[inline]
unsafe fn bit_test(st: *mut IdaState, id: u32) -> bool {
    if id >= (*st).cap_bits {
        return false;
    }
    let w = *(*st).words.add((id / 64) as usize);
    (w >> (id % 64)) & 1 != 0
}
#[inline]
unsafe fn bit_set(st: *mut IdaState, id: u32) {
    let p = (*st).words.add((id / 64) as usize);
    *p |= 1u64 << (id % 64);
}
#[inline]
unsafe fn bit_clear(st: *mut IdaState, id: u32) {
    let p = (*st).words.add((id / 64) as usize);
    *p &= !(1u64 << (id % 64));
}

/// `ida_alloc_range(ida, min, max, gfp)` — lowest free ID in `[min, max]`.
/// Returns the ID, `-ENOSPC` if the range is full, or `-ENOMEM`.
#[no_mangle]
pub extern "C" fn ida_alloc_range(ida: *mut usize, min: u32, max: u32, _gfp: u32) -> i32 {
    if min > max {
        return ENOSPC;
    }
    unsafe {
        let st = ida_state(ida);
        if st.is_null() {
            return ENOMEM;
        }
        let mut id = min;
        loop {
            if id > max {
                return ENOSPC;
            }
            if id >= (*st).cap_bits {
                // beyond the bitmap → definitely free; grow to hold it
                if !ida_grow(st, id + 1) {
                    return ENOMEM;
                }
            }
            if !bit_test(st, id) {
                bit_set(st, id);
                return id as i32;
            }
            id += 1;
        }
    }
}

/// `ida_alloc(ida, gfp)` — lowest free ID from 0.
#[no_mangle]
pub extern "C" fn ida_alloc(ida: *mut usize, gfp: u32) -> i32 {
    ida_alloc_range(ida, 0, ID_MAX as u32, gfp)
}

/// `ida_alloc_min(ida, min, gfp)`.
#[no_mangle]
pub extern "C" fn ida_alloc_min(ida: *mut usize, min: u32, gfp: u32) -> i32 {
    ida_alloc_range(ida, min, ID_MAX as u32, gfp)
}

/// `ida_alloc_max(ida, max, gfp)`.
#[no_mangle]
pub extern "C" fn ida_alloc_max(ida: *mut usize, max: u32, gfp: u32) -> i32 {
    ida_alloc_range(ida, 0, max, gfp)
}

/// `ida_free(ida, id)` — release an ID for reuse.
#[no_mangle]
pub extern "C" fn ida_free(ida: *mut usize, id: u32) {
    if ida.is_null() || unsafe { *ida } == 0 {
        return;
    }
    unsafe {
        let st = *ida as *mut IdaState;
        if bit_test(st, id) {
            bit_clear(st, id);
        }
    }
}

/// `ida_init(ida)` — mark uninitialized (state created on first alloc).
#[no_mangle]
pub extern "C" fn ida_init(ida: *mut usize) {
    if !ida.is_null() {
        unsafe { *ida = 0 };
    }
}

/// `ida_destroy(ida)` — free the backing bitmap and reset the handle.
#[no_mangle]
pub extern "C" fn ida_destroy(ida: *mut usize) {
    if ida.is_null() || unsafe { *ida } == 0 {
        return;
    }
    unsafe {
        let st = *ida as *mut IdaState;
        if !(*st).words.is_null() {
            mm::kfree((*st).words as *mut u8);
        }
        mm::kfree(st as *mut u8);
        *ida = 0;
    }
}

/// `ida_simple_get(ida, start, end, gfp)` — legacy: ID in `[start, end)`
/// (`end == 0` means no upper bound). Returns the ID or `-ENOSPC`.
#[no_mangle]
pub extern "C" fn ida_simple_get(ida: *mut usize, start: u32, end: u32, gfp: u32) -> i32 {
    let max = if end == 0 { ID_MAX as u32 } else { end - 1 };
    ida_alloc_range(ida, start, max, gfp)
}

/// `ida_simple_remove(ida, id)`.
#[no_mangle]
pub extern "C" fn ida_simple_remove(ida: *mut usize, id: u32) {
    ida_free(ida, id);
}

// ── idr — ID → pointer map ───────────────────────────────────────────────────
// A growable array indexed by ID. `EMPTY` (all-ones, never a valid daemon heap
// pointer) marks a free slot, which lets a driver legitimately store a NULL
// pointer at a reserved ID and still distinguish it from "unallocated".

/// Free-slot sentinel — `usize::MAX` is not a valid daemon pointer.
const EMPTY: usize = usize::MAX;
const IDR_INIT_CAP: u32 = 16;

#[repr(C)]
struct IdrState {
    slots: *mut usize,
    cap: u32,
    base: u32,
}

unsafe fn idr_alloc_slots(cap: u32) -> *mut usize {
    let slots = mm::kmalloc((cap as usize) * core::mem::size_of::<usize>(), 0) as *mut usize;
    if !slots.is_null() {
        for i in 0..cap as usize {
            *slots.add(i) = EMPTY;
        }
    }
    slots
}

unsafe fn idr_state(idr: *mut usize) -> *mut IdrState {
    if idr.is_null() {
        return ptr::null_mut();
    }
    let handle = *idr;
    // Low bit tags a base-only init (idr_init_base) so the base survives until
    // the slots are created; a real state pointer is heap-aligned (low bits 0).
    if handle != 0 && handle & 1 == 0 {
        return handle as *mut IdrState;
    }
    let base = (handle >> 1) as u32; // 0 unless idr_init_base stashed one
    let st = mm::kzalloc(core::mem::size_of::<IdrState>(), 0) as *mut IdrState;
    if st.is_null() {
        return ptr::null_mut();
    }
    let slots = idr_alloc_slots(IDR_INIT_CAP);
    if slots.is_null() {
        mm::kfree(st as *mut u8);
        return ptr::null_mut();
    }
    (*st).slots = slots;
    (*st).cap = IDR_INIT_CAP;
    (*st).base = base;
    *idr = st as usize;
    st
}

unsafe fn idr_grow(st: *mut IdrState, need: u32) -> bool {
    if need <= (*st).cap {
        return true;
    }
    let mut new_cap = (*st).cap;
    while new_cap < need {
        new_cap = new_cap.saturating_mul(2);
        if new_cap == 0 {
            new_cap = need;
            break;
        }
    }
    let fresh = idr_alloc_slots(new_cap);
    if fresh.is_null() {
        return false;
    }
    ptr::copy_nonoverlapping((*st).slots, fresh, (*st).cap as usize);
    mm::kfree((*st).slots as *mut u8);
    (*st).slots = fresh;
    (*st).cap = new_cap;
    true
}

/// `idr_alloc(idr, ptr, start, end, gfp)` — store `ptr` at the lowest free ID in
/// `[start, end)` (`end <= 0` means no upper bound). Returns the ID or
/// `-ENOSPC`/`-ENOMEM`.
#[no_mangle]
pub extern "C" fn idr_alloc(
    idr: *mut usize,
    ptr_val: *mut u8,
    start: i32,
    end: i32,
    _gfp: u32,
) -> i32 {
    let hi = if end <= 0 { ID_MAX } else { end - 1 };
    unsafe {
        let st = idr_state(idr);
        if st.is_null() {
            return ENOMEM;
        }
        // Linux clamps the start up to idr_base — the base is the lowest ID the
        // idr will ever hand out.
        let lo = start.max(0).max((*st).base as i32);
        if lo > hi {
            return ENOSPC;
        }
        let mut id = lo;
        loop {
            if id > hi {
                return ENOSPC;
            }
            if id as u32 >= (*st).cap {
                if !idr_grow(st, id as u32 + 1) {
                    return ENOMEM;
                }
            }
            let slot = (*st).slots.add(id as usize);
            if *slot == EMPTY {
                *slot = ptr_val as usize;
                return id;
            }
            id += 1;
        }
    }
}

/// `idr_find(idr, id)` → stored pointer, or NULL if the slot is empty.
#[no_mangle]
pub extern "C" fn idr_find(idr: *mut usize, id: i32) -> *mut u8 {
    if idr.is_null() || id < 0 {
        return ptr::null_mut();
    }
    unsafe {
        let handle = *idr;
        if handle == 0 || handle & 1 != 0 {
            return ptr::null_mut();
        }
        let st = handle as *mut IdrState;
        if (id as u32) >= (*st).cap {
            return ptr::null_mut();
        }
        let v = *(*st).slots.add(id as usize);
        if v == EMPTY {
            ptr::null_mut()
        } else {
            v as *mut u8
        }
    }
}

/// `idr_remove(idr, id)` → the pointer that was stored (NULL if none), freeing
/// the slot.
#[no_mangle]
pub extern "C" fn idr_remove(idr: *mut usize, id: i32) -> *mut u8 {
    if idr.is_null() || id < 0 {
        return ptr::null_mut();
    }
    unsafe {
        let handle = *idr;
        if handle == 0 || handle & 1 != 0 {
            return ptr::null_mut();
        }
        let st = handle as *mut IdrState;
        if (id as u32) >= (*st).cap {
            return ptr::null_mut();
        }
        let slot = (*st).slots.add(id as usize);
        let v = *slot;
        *slot = EMPTY;
        if v == EMPTY {
            ptr::null_mut()
        } else {
            v as *mut u8
        }
    }
}

/// `idr_replace(idr, ptr, id)` → the previous pointer (NULL if the slot was
/// empty); stores `ptr` at `id`.
#[no_mangle]
pub extern "C" fn idr_replace(idr: *mut usize, ptr_val: *mut u8, id: i32) -> *mut u8 {
    if idr.is_null() || id < 0 {
        return ptr::null_mut();
    }
    unsafe {
        let st = idr_state(idr);
        if st.is_null() {
            return ptr::null_mut();
        }
        if (id as u32) >= (*st).cap && !idr_grow(st, id as u32 + 1) {
            return ptr::null_mut();
        }
        let slot = (*st).slots.add(id as usize);
        let old = *slot;
        *slot = ptr_val as usize;
        if old == EMPTY {
            ptr::null_mut()
        } else {
            old as *mut u8
        }
    }
}

/// `idr_init_base(idr, base)` — set the starting ID. Stashed as a tagged handle
/// until the first allocation materializes the state.
#[no_mangle]
pub extern "C" fn idr_init_base(idr: *mut usize, base: i32) {
    if !idr.is_null() {
        unsafe { *idr = ((base.max(0) as usize) << 1) | 1 };
    }
}

/// `idr_init(idr)` — base 0.
#[no_mangle]
pub extern "C" fn idr_init(idr: *mut usize) {
    if !idr.is_null() {
        unsafe { *idr = 0 };
    }
}

/// `idr_destroy(idr)` — free the slot array and reset the handle.
#[no_mangle]
pub extern "C" fn idr_destroy(idr: *mut usize) {
    if idr.is_null() {
        return;
    }
    unsafe {
        let handle = *idr;
        if handle != 0 && handle & 1 == 0 {
            let st = handle as *mut IdrState;
            if !(*st).slots.is_null() {
                mm::kfree((*st).slots as *mut u8);
            }
            mm::kfree(st as *mut u8);
        }
        *idr = 0;
    }
}

/// `idr_is_empty(idr)` → 1 if no IDs are allocated.
#[no_mangle]
pub extern "C" fn idr_is_empty(idr: *mut usize) -> i32 {
    if idr.is_null() {
        return 1;
    }
    unsafe {
        let handle = *idr;
        if handle == 0 || handle & 1 != 0 {
            return 1;
        }
        let st = handle as *mut IdrState;
        for i in 0..(*st).cap as usize {
            if *(*st).slots.add(i) != EMPTY {
                return 0;
            }
        }
        1
    }
}
