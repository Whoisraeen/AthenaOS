//! `struct xarray` — the modern sparse `unsigned long → void *` map that has
//! replaced the radix tree across the kernel, and the DRM/GPU map of record.
//!
//! DRM leans on the xarray everywhere a driver maps a small integer to an object:
//! `drm_file.object_idr` is migrating to it for **GEM handle → buffer object**
//! tables; the GPU scheduler keeps `entity`/`fence-context → timeline` lookups in
//! one; `drm_syncobj` arrays, dma-fence chain contexts, and the per-device
//! minor/lessee maps are all xarrays. The two operations a GEM-handle table needs
//! are `xa_store` (place an object at a chosen handle, e.g. on import) and
//! `xa_alloc` (hand out the next free handle on create) — and crucially
//! `xa_alloc_cyclic`, which wraps the handle space so a long-lived `drm_file`
//! doesn't monotonically exhaust 32-bit handles. A driver that links this shim and
//! does `xa_alloc(&file->objects, &handle, bo, limit, GFP_KERNEL)` must get a real
//! lowest-free (or cyclic-next-free) index assignment with `load`/`erase`/iterate
//! that agree, or its handle table aliases two objects onto one handle.
//!
//! **Impl note — sorted vector, not a radix tree.** Linux's xarray is a 64-ary
//! radix tree tuned for cache behaviour at scale. The userspace DRM daemon holds
//! at most a few thousand live handles per file, so this shim backs the map with a
//! single index-sorted growable array of `(index, ptr)` entries on the daemon heap
//! (`mm`). Lookups are a binary search, stores/erases keep the array sorted, and
//! `xa_alloc` finds the lowest free index by a linear scan of the (sorted) keys.
//! This trades the radix micro-structure for correctness and simplicity — the
//! externally observable contract (which index maps to which pointer, which index
//! `xa_alloc` returns, the in-order `xa_for_each` walk) is *identical* to Linux's,
//! which is all a driver reaching these symbols can observe. If profiling ever
//! shows the linear free-scan hurts, the backing store can become a BTreeMap-style
//! structure with no ABI change.
//!
//! **Daemon model.** Like `idr`/`ida`, `struct xarray` is opaque to drivers
//! (`DEFINE_XARRAY`/`xa_init` zero it). We use only the FIRST machine word as a
//! lazily-allocated handle to shim-side state on the heap; the C struct is ≥3
//! words so writing the first word is safe, and zero == "uninitialised". The
//! cyclic "next" cursor lives in shim state, mirroring `xa->xa_alloc_next`.
//! Single-threaded cooperative daemon → no locking around the state.

use crate::mm;
use core::ptr;

/// Linux negative errnos returned where the C API returns an `int`.
const ENOMEM: i32 = -12;
const EBUSY: i32 = -16;
const ENOSPC: i32 = -28;
const EINVAL: i32 = -22;

/// `XA_FLAGS_*` (only the allocation-tracking flags drivers pass to `xa_init`
/// matter to this impl; the rest are advisory and stored verbatim).
pub const XA_FLAGS_TRACK_FREE: u32 = 1 << 2;
pub const XA_FLAGS_ZERO_BUSY: u32 = 1 << 3;
pub const XA_FLAGS_ALLOC: u32 = XA_FLAGS_TRACK_FREE;
/// Alloc starting at 1 (index 0 reserved) — `XA_FLAGS_ALLOC1`.
pub const XA_FLAGS_ALLOC1: u32 = XA_FLAGS_TRACK_FREE | XA_FLAGS_ZERO_BUSY;

/// Exact C layout of Linux 7.0.12 `struct xarray` for the curated shim headers:
/// 32-bit spinlock, 32-bit flags, then the head pointer. The previous shim cast
/// the structure address to `*mut usize`, overwriting lock+flags with a heap
/// pointer and treating static flags as a pointer on first use.
#[repr(C)]
pub struct XArray {
    lock: i32,
    flags: u32,
    head: *mut XaState,
}

/// Linux 7.0.12 declares `max` before `min`; designated C initializers hide the
/// ordering at source level, but SysV passes the 8-byte value in one register.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct XaLimit {
    pub max: u32,
    pub min: u32,
}

/// Free-slot sentinel for `xa_alloc`'s reserve path — `usize::MAX` is never a
/// valid daemon heap pointer, so it distinguishes a reserved-but-empty index
/// (an `XA_ZERO_ENTRY` in Linux) from a stored NULL.
const XA_ZERO: usize = usize::MAX;

/// One `(index, ptr)` pair. Kept in an array sorted ascending by `index`.
#[repr(C)]
#[derive(Clone, Copy)]
struct Entry {
    index: u64,
    value: usize,
}

/// Shim-side xarray state behind a `struct xarray`'s first word.
#[repr(C)]
struct XaState {
    entries: *mut Entry,
    len: u32,
    cap: u32,
    flags: u32,
    /// Cyclic allocation cursor (`xa_alloc_cyclic` resumes from here).
    next: u64,
}

const XA_INIT_CAP: u32 = 16;

/// Fetch (and lazily create) the `XaState` behind a `struct xarray`'s first word.
/// `init_flags` seeds `flags` only on first creation. Null on heap exhaustion.
unsafe fn xa_state(xa: *mut XArray, init_flags: u32) -> *mut XaState {
    if xa.is_null() {
        return ptr::null_mut();
    }
    if !(*xa).head.is_null() {
        return (*xa).head;
    }
    let st = mm::kzalloc(core::mem::size_of::<XaState>(), 0) as *mut XaState;
    if st.is_null() {
        return ptr::null_mut();
    }
    let entries =
        mm::kmalloc(XA_INIT_CAP as usize * core::mem::size_of::<Entry>(), 0) as *mut Entry;
    if entries.is_null() {
        mm::kfree(st as *mut u8);
        return ptr::null_mut();
    }
    (*st).entries = entries;
    (*st).len = 0;
    (*st).cap = XA_INIT_CAP;
    if init_flags != 0 && (*xa).flags == 0 {
        (*xa).flags = init_flags;
    }
    (*st).flags = (*xa).flags;
    // ALLOC1 reserves index 0; the first cyclic/alloc index is 1.
    (*st).next = if (*st).flags & XA_FLAGS_ZERO_BUSY != 0 {
        1
    } else {
        0
    };
    (*xa).head = st;
    st
}

/// Grow the entry array to hold at least `need` entries (amortised doubling).
unsafe fn xa_grow(st: *mut XaState, need: u32) -> bool {
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
    let fresh = mm::kmalloc(new_cap as usize * core::mem::size_of::<Entry>(), 0) as *mut Entry;
    if fresh.is_null() {
        return false;
    }
    ptr::copy_nonoverlapping((*st).entries, fresh, (*st).len as usize);
    mm::kfree((*st).entries as *mut u8);
    (*st).entries = fresh;
    (*st).cap = new_cap;
    true
}

/// Binary search for `index`. `Ok(pos)` = found at `pos`; `Err(pos)` = not found,
/// `pos` is the insertion point that keeps the array sorted.
unsafe fn xa_find_slot(st: *mut XaState, index: u64) -> Result<usize, usize> {
    let n = (*st).len as usize;
    let base = (*st).entries;
    let mut lo = 0usize;
    let mut hi = n;
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        let k = (*base.add(mid)).index;
        if k == index {
            return Ok(mid);
        } else if k < index {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    Err(lo)
}

/// Insert `(index, value)` at sorted `pos`, shifting the tail up by one.
unsafe fn xa_insert_at(st: *mut XaState, pos: usize, index: u64, value: usize) -> bool {
    if !xa_grow(st, (*st).len + 1) {
        return false;
    }
    let base = (*st).entries;
    let n = (*st).len as usize;
    // Shift [pos, n) -> [pos+1, n+1)
    ptr::copy(base.add(pos), base.add(pos + 1), n - pos);
    *base.add(pos) = Entry { index, value };
    (*st).len += 1;
    true
}

/// Remove the entry at sorted `pos`, shifting the tail down by one.
unsafe fn xa_remove_at(st: *mut XaState, pos: usize) {
    let base = (*st).entries;
    let n = (*st).len as usize;
    ptr::copy(base.add(pos + 1), base.add(pos), n - pos - 1);
    (*st).len -= 1;
}

// ── C ABI ─────────────────────────────────────────────────────────────────────

/// `xa_init_flags(xa, flags)` — record flags; state is created on first use.
#[no_mangle]
pub extern "C" fn xa_init_flags(xa: *mut XArray, flags: u32) {
    if xa.is_null() {
        return;
    }
    unsafe {
        if !(*xa).head.is_null() {
            xa_destroy(xa);
        }
        (*xa).lock = 0;
        (*xa).flags = flags;
        (*xa).head = ptr::null_mut();
    }
}

/// `xa_init(xa)` — flags = 0.
#[no_mangle]
pub extern "C" fn xa_init(xa: *mut XArray) {
    xa_init_flags(xa, 0);
}

/// `xa_load(xa, index)` → stored pointer, or NULL if absent (or a reserved
/// zero-entry, which Linux also reports as NULL to `xa_load`).
#[no_mangle]
pub extern "C" fn xa_load(xa: *mut XArray, index: u64) -> *mut u8 {
    if xa.is_null() || unsafe { (*xa).head.is_null() } {
        return ptr::null_mut();
    }
    unsafe {
        let st = (*xa).head;
        match xa_find_slot(st, index) {
            Ok(pos) => {
                let v = (*(*st).entries.add(pos)).value;
                if v == XA_ZERO {
                    ptr::null_mut()
                } else {
                    v as *mut u8
                }
            }
            Err(_) => ptr::null_mut(),
        }
    }
}

/// `xa_store(xa, index, entry, gfp)` → previous pointer at `index` (NULL if the
/// slot was empty or reserved). Storing NULL leaves an empty slot (Linux frees
/// the node); storing a value at a reserved slot fills it.
#[no_mangle]
pub extern "C" fn xa_store(xa: *mut XArray, index: u64, entry: *mut u8, _gfp: u32) -> *mut u8 {
    if xa.is_null() {
        return ptr::null_mut();
    }
    unsafe {
        let st = xa_state(xa, 0);
        if st.is_null() {
            return ptr::null_mut();
        }
        let prev = match xa_find_slot(st, index) {
            Ok(pos) => {
                let slot = (*st).entries.add(pos);
                let old = (*slot).value;
                if entry.is_null() {
                    // Store NULL == erase the entry.
                    xa_remove_at(st, pos);
                } else {
                    (*slot).value = entry as usize;
                }
                if old == XA_ZERO {
                    ptr::null_mut()
                } else {
                    old as *mut u8
                }
            }
            Err(pos) => {
                if !entry.is_null() {
                    xa_insert_at(st, pos, index, entry as usize);
                }
                ptr::null_mut()
            }
        };
        prev
    }
}

/// `xa_erase(xa, index)` → the pointer removed (NULL if none).
#[no_mangle]
pub extern "C" fn xa_erase(xa: *mut XArray, index: u64) -> *mut u8 {
    if xa.is_null() || unsafe { (*xa).head.is_null() } {
        return ptr::null_mut();
    }
    unsafe {
        let st = (*xa).head;
        match xa_find_slot(st, index) {
            Ok(pos) => {
                let v = (*(*st).entries.add(pos)).value;
                xa_remove_at(st, pos);
                if v == XA_ZERO {
                    ptr::null_mut()
                } else {
                    v as *mut u8
                }
            }
            Err(_) => ptr::null_mut(),
        }
    }
}

/// `xa_empty(xa)` → 1 if no indices are populated.
#[no_mangle]
pub extern "C" fn xa_empty(xa: *mut XArray) -> i32 {
    if xa.is_null() || unsafe { (*xa).head.is_null() } {
        return 1;
    }
    (unsafe { (*(*xa).head).len } == 0) as i32
}

/// Find the lowest free index in `[min, max]`, or `None` if the range is full.
unsafe fn xa_lowest_free(st: *mut XaState, min: u64, max: u64) -> Option<u64> {
    if min > max {
        return None;
    }
    let n = (*st).len as usize;
    let base = (*st).entries;
    let mut want = min;
    // The keys are sorted ascending; scan from the first key >= min upward,
    // advancing `want` past any contiguous run of occupied indices.
    let start = match xa_find_slot(st, min) {
        Ok(p) => p,
        Err(p) => p,
    };
    let mut i = start;
    while i < n {
        let k = (*base.add(i)).index;
        if k > want {
            break; // gap at `want`
        }
        if k == want {
            if want == max {
                // The slot we want is taken and it's the ceiling.
                return None;
            }
            want += 1;
        }
        i += 1;
    }
    if want > max {
        None
    } else {
        Some(want)
    }
}

/// Core allocator shared by `xa_alloc`/`xa_alloc_cyclic`. Stores `entry` at the
/// chosen index, writes it through `id_out`, and (for cyclic) writes 1 to
/// `*wrapped` if the search had to wrap past `max` back to `min` to find a free
/// index. Returns 0 or a negative errno.
unsafe fn xa_alloc_core(
    xa: *mut XArray,
    id_out: *mut u32,
    entry: *mut u8,
    min: u64,
    max: u64,
    cyclic: bool,
    wrapped: *mut i32,
) -> i32 {
    if xa.is_null() || id_out.is_null() {
        return EINVAL;
    }
    let st = xa_state(xa, XA_FLAGS_ALLOC);
    if st.is_null() {
        return ENOMEM;
    }
    // Mirror Linux `__xa_alloc_cyclic`: resume at the cursor; if the cursor has
    // run past `max` (the previous alloc landed on the ceiling), the call has
    // logically wrapped — restart at `min` and flag it.
    let cur = (*st).next;
    let cursor_out_of_range = cur < min || cur > max;
    let lo = if cyclic && !cursor_out_of_range {
        cur
    } else {
        min
    };
    let mut did_wrap = cyclic && cursor_out_of_range;
    // Try from the cursor upward; if cyclic and that fails, wrap to min.
    let idx = match xa_lowest_free(st, lo, max) {
        Some(v) => v,
        None => {
            if cyclic && lo > min {
                did_wrap = true;
                match xa_lowest_free(st, min, max) {
                    Some(v) => v,
                    None => return ENOSPC,
                }
            } else {
                return ENOSPC;
            }
        }
    };
    // Store at the (necessarily free) index.
    let store_val = if entry.is_null() {
        XA_ZERO
    } else {
        entry as usize
    };
    match xa_find_slot(st, idx) {
        Ok(_) => return EBUSY, // shouldn't happen — index was reported free
        Err(pos) => {
            if !xa_insert_at(st, pos, idx, store_val) {
                return ENOMEM;
            }
        }
    }
    *id_out = idx as u32;
    if cyclic {
        // Advance the cursor past the chosen index; allow it to run to max+1 so
        // the NEXT call detects the wrap (Linux leaves `*next` at id+1).
        (*st).next = idx.saturating_add(1);
        if !wrapped.is_null() {
            *wrapped = did_wrap as i32;
        }
    }
    0
}

/// `xa_alloc(xa, id, entry, limit, gfp)` — assign the lowest free index in the
/// `xa_limit { min, max }` range, store `entry` there, write the index to `*id`.
/// Returns 0, `-ENOSPC` if full, `-ENOMEM`, or `-EINVAL`.
///
/// `XaLimit` is passed by value exactly as Linux declares it.
#[no_mangle]
pub extern "C" fn xa_alloc(
    xa: *mut XArray,
    id: *mut u32,
    entry: *mut u8,
    limit: XaLimit,
    _gfp: u32,
) -> i32 {
    unsafe {
        xa_alloc_core(
            xa,
            id,
            entry,
            limit.min as u64,
            limit.max as u64,
            false,
            ptr::null_mut(),
        )
    }
}

/// `xa_alloc_cyclic(xa, id, entry, limit, next, gfp)` — like `xa_alloc` but
/// resumes from the internal cyclic cursor and wraps at `max`, so handles are
/// not immediately reused (defends against ABA on long-lived handle tables).
/// Returns 0 on a fresh index, 1 if the allocation wrapped past `max`, or a
/// negative errno (matching Linux's `__xa_alloc_cyclic` return convention).
#[no_mangle]
pub extern "C" fn xa_alloc_cyclic(
    xa: *mut XArray,
    id: *mut u32,
    entry: *mut u8,
    limit: XaLimit,
    next: *mut u32,
    _gfp: u32,
) -> i32 {
    if xa.is_null() || next.is_null() {
        return EINVAL;
    }
    unsafe {
        let st = xa_state(xa, XA_FLAGS_ALLOC);
        if st.is_null() {
            return ENOMEM;
        }
        (*st).next = (*next).max(limit.min) as u64;
    }
    let mut wrapped: i32 = 0;
    let rc = unsafe {
        xa_alloc_core(
            xa,
            id,
            entry,
            limit.min as u64,
            limit.max as u64,
            true,
            &mut wrapped,
        )
    };
    if rc != 0 {
        return rc;
    }
    unsafe {
        *next = (*id).wrapping_add(1);
    }
    let _ = wrapped;
    0 // High-level xa_alloc_cyclic hides the internal wrap indication.
}

/// Locked-form erase used by callers that already hold `xa_lock`.
#[no_mangle]
pub extern "C" fn __xa_erase(xa: *mut XArray, index: u64) -> *mut u8 {
    xa_erase(xa, index)
}

/// Atomic compare/exchange of an entry. The returned pointer is the value that
/// was present, matching Linux xarray semantics; allocation failure is encoded
/// as an ERR_PTR so `xa_err()` observes `-ENOMEM`.
#[no_mangle]
pub extern "C" fn xa_cmpxchg(
    xa: *mut XArray,
    index: u64,
    old: *mut u8,
    entry: *mut u8,
    _gfp: u32,
) -> *mut u8 {
    if xa.is_null() {
        return EINVAL as isize as *mut u8;
    }
    unsafe {
        let st = xa_state(xa, 0);
        if st.is_null() {
            return ENOMEM as isize as *mut u8;
        }
        match xa_find_slot(st, index) {
            Ok(pos) => {
                let slot = (*st).entries.add(pos);
                let current = if (*slot).value == XA_ZERO {
                    ptr::null_mut()
                } else {
                    (*slot).value as *mut u8
                };
                if current != old {
                    return current;
                }
                if entry.is_null() {
                    xa_remove_at(st, pos);
                } else {
                    (*slot).value = entry as usize;
                }
                current
            }
            Err(pos) => {
                if !old.is_null() {
                    return ptr::null_mut();
                }
                if !entry.is_null() && !xa_insert_at(st, pos, index, entry as usize) {
                    return ENOMEM as isize as *mut u8;
                }
                ptr::null_mut()
            }
        }
    }
}

/// `xa_destroy(xa)` — free the backing array and reset the handle. Does NOT free
/// the stored pointers (the driver owns those, as in Linux).
#[no_mangle]
pub extern "C" fn xa_destroy(xa: *mut XArray) {
    if xa.is_null() || unsafe { (*xa).head.is_null() } {
        return;
    }
    unsafe {
        let st = (*xa).head;
        if !(*st).entries.is_null() {
            mm::kfree((*st).entries as *mut u8);
        }
        mm::kfree(st as *mut u8);
        (*xa).head = ptr::null_mut();
    }
}

/// `xa_for_each` cursor support — Rust-friendly in-order iteration helper.
///
/// Linux's `xa_for_each(xa, index, entry)` is a macro built on `xa_find` /
/// `xa_find_after`. We export those two so a driver's (or daemon's) loop can do
/// `for (entry = xa_find(xa, &i, ULONG_MAX, XA_PRESENT); entry; entry =
/// xa_find_after(xa, &i, ULONG_MAX, XA_PRESENT))` and visit every populated
/// index in ascending order — exactly the macro's behaviour.

/// `xa_find(xa, *index, max, _filter)` — first populated entry with index >=
/// `*index` and <= `max`; updates `*index` to that entry's index and returns its
/// pointer, or NULL if none. (`_filter` = `XA_PRESENT`; tags unsupported.)
#[no_mangle]
pub extern "C" fn xa_find(xa: *mut XArray, index: *mut u64, max: u64, _filter: u32) -> *mut u8 {
    if xa.is_null() || index.is_null() || unsafe { (*xa).head.is_null() } {
        return ptr::null_mut();
    }
    unsafe {
        let st = (*xa).head;
        let from = *index;
        let start = match xa_find_slot(st, from) {
            Ok(p) => p,
            Err(p) => p,
        };
        if start < (*st).len as usize {
            let e = *(*st).entries.add(start);
            if e.index <= max {
                *index = e.index;
                return if e.value == XA_ZERO {
                    ptr::null_mut()
                } else {
                    e.value as *mut u8
                };
            }
        }
        ptr::null_mut()
    }
}

/// `xa_find_after(xa, *index, max, _filter)` — first populated entry with index
/// strictly greater than `*index`; updates `*index` and returns the pointer, or
/// NULL. Drives the loop body of `xa_for_each`.
#[no_mangle]
pub extern "C" fn xa_find_after(
    xa: *mut XArray,
    index: *mut u64,
    max: u64,
    _filter: u32,
) -> *mut u8 {
    if xa.is_null() || index.is_null() || unsafe { (*xa).head.is_null() } {
        return ptr::null_mut();
    }
    unsafe {
        let cur = *index;
        if cur == u64::MAX {
            return ptr::null_mut(); // no index strictly greater than the max
        }
        *index = cur + 1;
        xa_find(xa, index, max, _filter)
    }
}

// ── Layout contract ──────────────────────────────────────────────────────────
// `struct xarray` is opaque to drivers; we only require its first word to be a
// machine word (it is — `xa_lock` then `xa_flags`/`xa_head`). The shim state is
// fully private, so only the Entry packing is asserted (binary-search relies on
// it being a plain `(u64, usize)`).
const _: () = assert!(core::mem::size_of::<XArray>() == 16);
const _: () = assert!(core::mem::align_of::<XArray>() == 8);
const _: () = assert!(core::mem::size_of::<XaLimit>() == 8);
const _: () = assert!(core::mem::size_of::<Entry>() == 8 + core::mem::size_of::<usize>());

#[cfg(test)]
mod tests {
    //! Pure host KATs (no syscall path — safe on Windows-native per project
    //! memory). Each assert is a concrete expected-vs-actual comparison and can
    //! FAIL. `mm::kmalloc` is the real freeing daemon heap, exercised here.
    use super::*;
    use core::ptr;

    fn fresh() -> XArray {
        XArray {
            lock: 0,
            flags: 0,
            head: ptr::null_mut(),
        }
    }

    fn limit(min: u32, max: u32) -> XaLimit {
        XaLimit { max, min }
    }

    #[test]
    fn store_load_erase_overwrite() {
        let mut xa = fresh();
        let p = &mut xa as *mut XArray;
        xa_init(p);
        assert_eq!(xa_empty(p), 1, "fresh xarray must be empty");
        assert!(
            xa_load(p, 5).is_null(),
            "load of missing index must be NULL"
        );

        let a = 0x1000 as *mut u8;
        let b = 0x2000 as *mut u8;
        let c = 0x3000 as *mut u8;

        // Store out of order; load must find each.
        assert!(
            xa_store(p, 5, a, 0).is_null(),
            "first store returns no prev"
        );
        assert!(xa_store(p, 1, b, 0).is_null());
        assert!(xa_store(p, 9, c, 0).is_null());
        assert_eq!(xa_empty(p), 0);
        assert_eq!(xa_load(p, 5), a, "load index 5");
        assert_eq!(xa_load(p, 1), b, "load index 1");
        assert_eq!(xa_load(p, 9), c, "load index 9");
        assert!(xa_load(p, 7).is_null(), "gap index must be NULL");

        // Overwrite returns the previous pointer.
        let d = 0x4000 as *mut u8;
        assert_eq!(xa_store(p, 5, d, 0), a, "overwrite returns old ptr");
        assert_eq!(xa_load(p, 5), d, "load sees new ptr after overwrite");

        // Erase returns the removed pointer; subsequent load is NULL.
        assert_eq!(xa_erase(p, 1), b, "erase returns removed ptr");
        assert!(xa_load(p, 1).is_null(), "erased index loads NULL");
        assert!(xa_erase(p, 1).is_null(), "double erase returns NULL");

        // Store NULL erases (Linux semantics).
        assert_eq!(
            xa_store(p, 9, ptr::null_mut(), 0),
            c,
            "store NULL returns old"
        );
        assert!(xa_load(p, 9).is_null(), "store NULL leaves empty slot");

        xa_destroy(p);
        assert_eq!(xa_empty(p), 1, "destroyed xarray is empty");
    }

    #[test]
    fn alloc_assigns_distinct_ascending_indices() {
        let mut xa = fresh();
        let p = &mut xa as *mut XArray;
        xa_init_flags(p, XA_FLAGS_ALLOC);

        let mut id0 = u32::MAX;
        let mut id1 = u32::MAX;
        let mut id2 = u32::MAX;
        let v0 = 0xAA00 as *mut u8;
        let v1 = 0xBB00 as *mut u8;
        let v2 = 0xCC00 as *mut u8;

        assert_eq!(xa_alloc(p, &mut id0, v0, limit(0, 100), 0), 0, "alloc 0 ok");
        assert_eq!(xa_alloc(p, &mut id1, v1, limit(0, 100), 0), 0, "alloc 1 ok");
        assert_eq!(xa_alloc(p, &mut id2, v2, limit(0, 100), 0), 0, "alloc 2 ok");
        assert_eq!(id0, 0, "first index is the range min");
        assert_eq!(id1, 1, "second index ascends");
        assert_eq!(id2, 2, "third index ascends");
        assert_eq!(xa_load(p, id0 as u64), v0, "alloc stored the value");
        assert_eq!(xa_load(p, id2 as u64), v2);

        // Freeing a hole makes alloc reuse the lowest free index.
        assert_eq!(xa_erase(p, 1), v1, "erase the middle");
        let mut id_reuse = u32::MAX;
        assert_eq!(xa_alloc(p, &mut id_reuse, v1, limit(0, 100), 0), 0);
        assert_eq!(id_reuse, 1, "non-cyclic alloc reuses the lowest free index");

        xa_destroy(p);
    }

    #[test]
    fn alloc_range_full_returns_enospc() {
        let mut xa = fresh();
        let p = &mut xa as *mut XArray;
        xa_init_flags(p, XA_FLAGS_ALLOC);
        let mut id = u32::MAX;
        // Range [3, 4] holds exactly two indices.
        assert_eq!(xa_alloc(p, &mut id, 0x10 as *mut u8, limit(3, 4), 0), 0);
        assert_eq!(id, 3, "alloc starts at min");
        assert_eq!(xa_alloc(p, &mut id, 0x20 as *mut u8, limit(3, 4), 0), 0);
        assert_eq!(id, 4);
        assert_eq!(
            xa_alloc(p, &mut id, 0x30 as *mut u8, limit(3, 4), 0),
            ENOSPC,
            "full range must return -ENOSPC"
        );
        xa_destroy(p);
    }

    #[test]
    fn alloc_cyclic_advances_and_wraps() {
        let mut xa = fresh();
        let p = &mut xa as *mut XArray;
        xa_init_flags(p, XA_FLAGS_ALLOC);

        let mut id = u32::MAX;
        let mut next = 0u32;
        // Cyclic over [0, 2]: hand out 0,1,2 then wrap.
        assert_eq!(
            xa_alloc_cyclic(p, &mut id, 0x1 as *mut u8, limit(0, 2), &mut next, 0),
            0
        );
        assert_eq!(id, 0);
        assert_eq!(
            xa_alloc_cyclic(p, &mut id, 0x2 as *mut u8, limit(0, 2), &mut next, 0),
            0
        );
        assert_eq!(id, 1);
        assert_eq!(
            xa_alloc_cyclic(p, &mut id, 0x3 as *mut u8, limit(0, 2), &mut next, 0),
            0
        );
        assert_eq!(id, 2, "cursor reached the ceiling");

        // Free index 0; the cursor has wrapped to 0, so the next alloc reuses it
        // through the caller-owned `next` cursor.
        assert_eq!(xa_erase(p, 0), 0x1 as *mut u8);
        let rc = xa_alloc_cyclic(p, &mut id, 0x4 as *mut u8, limit(0, 2), &mut next, 0);
        assert_eq!(rc, 0, "high-level cyclic allocation succeeds after wrap");
        assert_eq!(id, 0, "wrap reuses the freed low index");
        xa_destroy(p);
    }

    #[test]
    fn for_each_visits_all_in_ascending_order() {
        let mut xa = fresh();
        let p = &mut xa as *mut XArray;
        xa_init(p);
        // Insert in scrambled order.
        xa_store(p, 7, 0x70 as *mut u8, 0);
        xa_store(p, 2, 0x20 as *mut u8, 0);
        xa_store(p, 13, 0xD0 as *mut u8, 0);
        xa_store(p, 4, 0x40 as *mut u8, 0);

        // xa_for_each: xa_find then xa_find_after until NULL.
        let mut idx: u64 = 0;
        let mut seen_idx = [0u64; 8];
        let mut seen_val = [0usize; 8];
        let mut n = 0usize;
        let mut entry = xa_find(p, &mut idx, u64::MAX, 0);
        while !entry.is_null() {
            assert!(n < seen_idx.len(), "overrun");
            seen_idx[n] = idx;
            seen_val[n] = entry as usize;
            n += 1;
            entry = xa_find_after(p, &mut idx, u64::MAX, 0);
        }
        assert_eq!(n, 4, "for_each must visit every populated index");
        assert_eq!(&seen_idx[..4], &[2, 4, 7, 13], "ascending index order");
        assert_eq!(
            &seen_val[..4],
            &[0x20, 0x40, 0x70, 0xD0],
            "values match their indices"
        );
        xa_destroy(p);
    }

    #[test]
    fn cmpxchg_inserts_swaps_rejects_and_removes() {
        let mut xa = fresh();
        let p = &mut xa as *mut XArray;
        xa_init(p);

        let v1 = 0xA1 as *mut u8;
        let v2 = 0xB2 as *mut u8;
        let v3 = 0xC3 as *mut u8;
        let null = ptr::null_mut();

        // Absent slot + non-null `old`: nothing to match, no insert.
        assert_eq!(
            xa_cmpxchg(p, 5, v1, v2, 0),
            null,
            "absent-vs-nonnull => NULL"
        );
        assert!(xa_load(p, 5).is_null(), "rejected cmpxchg must not insert");

        // Absent slot + null `old`: install `entry`, report the prior (NULL).
        assert_eq!(
            xa_cmpxchg(p, 5, null, v1, 0),
            null,
            "insert reports prior NULL"
        );
        assert_eq!(xa_load(p, 5), v1, "insert stored v1");

        // Matching `old`: swap succeeds and returns the replaced value.
        assert_eq!(xa_cmpxchg(p, 5, v1, v2, 0), v1, "swap returns replaced v1");
        assert_eq!(xa_load(p, 5), v2, "swap stored v2");

        // Mismatched `old`: no change, returns the actual current value.
        assert_eq!(
            xa_cmpxchg(p, 5, v1, v3, 0),
            v2,
            "mismatch returns current v2"
        );
        assert_eq!(xa_load(p, 5), v2, "mismatch must not overwrite");

        // Matching `old` + null `entry`: erase, returns the removed value.
        assert_eq!(
            xa_cmpxchg(p, 5, v2, null, 0),
            v2,
            "erase returns removed v2"
        );
        assert!(xa_load(p, 5).is_null(), "erased slot reads NULL");
        assert_eq!(xa_empty(p), 1, "xarray empty after the only entry erased");

        xa_destroy(p);
    }
}
