//! `dma_resv` — reservation object: the set of fences attached to a buffer.
//!
//! A `dma_resv` answers "what async work is still using this buffer?" — readers
//! and writers add their `dma_fence` with a usage level, and anyone wanting to
//! reuse/free the buffer waits on (or tests) the matching fences. It is the glue
//! between `dma_fence` and `dma_buf`, used by every GPU memory manager (TTM/GEM).
//!
//! **Layouts BTF-verified** (`pahole`, Linux 6.6): `dma_resv` (size 48, ww_mutex
//! lock@0, `fences`@40), `dma_resv_iter` (size 48, fence@16/fence_usage@24/
//! index@28/fences@32/num_fences@40), `dma_resv_list` (rcu@0, num_fences@16,
//! max_fences@20, flexible `table[]`@24). Const-assert guards at the bottom.
//!
//! **Internal encoding (documented assumption):** each `table[]` slot packs the
//! fence pointer with its 2-bit usage in the low bits (fences are ≥8-aligned, so
//! the low 3 bits are free) — the standard upstream scheme. Drivers reach the
//! list only through these functions / the `dma_resv_for_each_fence` macro (which
//! drives our iterator), never the raw table, so the encoding stays internal.
//! Usage ordering: KERNEL(0) < WRITE(1) < READ(2) < BOOKKEEP(3); iterating for
//! usage U yields every fence with `fence_usage <= U`.

use crate::dma_fence::{
    self, dma_fence_array_create, dma_fence_context_alloc, dma_fence_wait_timeout, DmaFence,
    DmaFenceArray,
};
use crate::mm;
use core::ptr;

const USAGE_MASK: usize = 0x3;
const ENOMEM: i32 = -12;

/// `struct dma_resv` (lock@0 is an opaque ww_mutex the caller holds; fences@40).
#[repr(C)]
pub struct DmaResv {
    pub lock: [u8; 40], // struct ww_mutex
    pub fences: *mut DmaResvList,
}

/// `struct dma_resv_list` — header then a flexible `table[max_fences]` of packed
/// `(fence | usage)` words.
#[repr(C)]
pub struct DmaResvList {
    pub rcu: [u8; 16], // struct callback_head (unused in the daemon)
    pub num_fences: u32,
    pub max_fences: u32,
    // table: [usize; max_fences] follows here
}

/// `struct dma_resv_iter` — the cursor a caller embeds for
/// `dma_resv_for_each_fence`. `dma_resv_iter_begin` (inline) presets obj+usage.
#[repr(C)]
pub struct DmaResvIter {
    pub obj: *mut DmaResv,
    pub usage: u32, // enum dma_resv_usage
    _pad: u32,
    pub fence: *mut DmaFence,
    pub fence_usage: u32,
    pub index: u32,
    pub fences: *mut DmaResvList,
    pub num_fences: u32,
    pub is_restarted: bool,
}

#[inline]
unsafe fn table(list: *mut DmaResvList) -> *mut usize {
    (list as *mut u8).add(core::mem::size_of::<DmaResvList>()) as *mut usize
}
#[inline]
fn unpack(entry: usize) -> (*mut DmaFence, u32) {
    (
        (entry & !USAGE_MASK) as *mut DmaFence,
        (entry & USAGE_MASK) as u32,
    )
}

/// `dma_resv_init(obj)` — empty reservation (no fences).
#[no_mangle]
pub unsafe extern "C" fn dma_resv_init(obj: *mut DmaResv) {
    if !obj.is_null() {
        (*obj).lock = [0u8; 40];
        (*obj).fences = ptr::null_mut();
    }
}

/// `dma_resv_fini(obj)` — release the fence list.
#[no_mangle]
pub unsafe extern "C" fn dma_resv_fini(obj: *mut DmaResv) {
    if !obj.is_null() && !(*obj).fences.is_null() {
        mm::kfree((*obj).fences as *mut u8);
        (*obj).fences = ptr::null_mut();
    }
}

/// `dma_resv_reserve_fences(obj, num)` — ensure room for `num` MORE fences,
/// growing (and copying) the list as needed. Returns 0 or `-ENOMEM`.
#[no_mangle]
pub unsafe extern "C" fn dma_resv_reserve_fences(obj: *mut DmaResv, num: u32) -> i32 {
    if obj.is_null() {
        return -22;
    }
    let cur = (*obj).fences;
    let (have, cap) = if cur.is_null() {
        (0, 0)
    } else {
        ((*cur).num_fences, (*cur).max_fences)
    };
    let need = have + num;
    if need <= cap && !cur.is_null() {
        return 0;
    }
    let new_cap = need.max(4);
    let size =
        core::mem::size_of::<DmaResvList>() + new_cap as usize * core::mem::size_of::<usize>();
    let nl = mm::kzalloc(size, 0) as *mut DmaResvList;
    if nl.is_null() {
        return ENOMEM;
    }
    (*nl).max_fences = new_cap;
    (*nl).num_fences = have;
    if !cur.is_null() {
        let (src, dst) = (table(cur), table(nl));
        for i in 0..have as usize {
            *dst.add(i) = *src.add(i);
        }
        mm::kfree(cur as *mut u8);
    }
    (*obj).fences = nl;
    0
}

/// `dma_resv_add_fence(obj, fence, usage)` — add (or replace a same-context)
/// fence. Requires a prior `dma_resv_reserve_fences`.
#[no_mangle]
pub unsafe extern "C" fn dma_resv_add_fence(obj: *mut DmaResv, fence: *mut DmaFence, usage: u32) {
    if obj.is_null() || fence.is_null() {
        return;
    }
    let list = (*obj).fences;
    if list.is_null() {
        return; // caller must reserve first (Linux contract)
    }
    let tbl = table(list);
    let packed = (fence as usize) | (usage as usize & USAGE_MASK);
    let fctx = (*fence).context;
    for i in 0..(*list).num_fences as usize {
        let (f, _) = unpack(*tbl.add(i));
        if !f.is_null() && (*f).context == fctx {
            *tbl.add(i) = packed; // replace same-context fence
            return;
        }
    }
    if (*list).num_fences < (*list).max_fences {
        let i = (*list).num_fences as usize;
        *tbl.add(i) = packed;
        (*list).num_fences += 1;
    }
}

// ── iterator ─────────────────────────────────────────────────────────────────

unsafe fn iter_advance(iter: *mut DmaResvIter) -> *mut DmaFence {
    let list = (*iter).fences;
    if !list.is_null() {
        let tbl = table(list);
        while (*iter).index < (*iter).num_fences {
            let i = (*iter).index as usize;
            (*iter).index += 1;
            let (f, u) = unpack(*tbl.add(i));
            if !f.is_null() && u <= (*iter).usage {
                (*iter).fence = f;
                (*iter).fence_usage = u;
                return f;
            }
        }
    }
    (*iter).fence = ptr::null_mut();
    ptr::null_mut()
}

/// `dma_resv_iter_first(cursor)` — start iterating (obj+usage preset by the
/// inline `dma_resv_iter_begin`).
#[no_mangle]
pub unsafe extern "C" fn dma_resv_iter_first(iter: *mut DmaResvIter) -> *mut DmaFence {
    (*iter).fences = (*(*iter).obj).fences;
    (*iter).num_fences = if (*iter).fences.is_null() {
        0
    } else {
        (*(*iter).fences).num_fences
    };
    (*iter).index = 0;
    (*iter).is_restarted = true;
    iter_advance(iter)
}

/// `dma_resv_iter_next(cursor)`.
#[no_mangle]
pub unsafe extern "C" fn dma_resv_iter_next(iter: *mut DmaResvIter) -> *mut DmaFence {
    (*iter).is_restarted = false;
    iter_advance(iter)
}

// No RCU / concurrent modification in the cooperative daemon, so the lockless
// variants behave identically to the locked ones.
#[no_mangle]
pub unsafe extern "C" fn dma_resv_iter_first_unlocked(iter: *mut DmaResvIter) -> *mut DmaFence {
    dma_resv_iter_first(iter)
}
#[no_mangle]
pub unsafe extern "C" fn dma_resv_iter_next_unlocked(iter: *mut DmaResvIter) -> *mut DmaFence {
    dma_resv_iter_next(iter)
}

// ── queries over the matching fence set ──────────────────────────────────────

/// `dma_resv_test_signaled(obj, usage)` → true if every matching fence is
/// signaled (or there are none).
#[no_mangle]
pub unsafe extern "C" fn dma_resv_test_signaled(obj: *mut DmaResv, usage: u32) -> bool {
    if obj.is_null() {
        return true;
    }
    let list = (*obj).fences;
    if list.is_null() {
        return true;
    }
    let tbl = table(list);
    for i in 0..(*list).num_fences as usize {
        let (f, u) = unpack(*tbl.add(i));
        if !f.is_null() && u <= usage && !dma_fence::is_signaled(f) {
            return false;
        }
    }
    true
}

/// `dma_resv_wait_timeout(obj, usage, intr, timeout)` — wait for all matching
/// fences; returns remaining jiffies (>0) or 0 on timeout.
#[no_mangle]
pub unsafe extern "C" fn dma_resv_wait_timeout(
    obj: *mut DmaResv,
    usage: u32,
    intr: bool,
    timeout: u64,
) -> i64 {
    let mut remaining = timeout as i64;
    if obj.is_null() {
        return remaining.max(1);
    }
    let list = (*obj).fences;
    if list.is_null() {
        return remaining.max(1);
    }
    let tbl = table(list);
    for i in 0..(*list).num_fences as usize {
        let (f, u) = unpack(*tbl.add(i));
        if !f.is_null() && u <= usage {
            let r = dma_fence_wait_timeout(f, intr, remaining);
            if r <= 0 {
                return 0;
            }
            remaining = r;
        }
    }
    remaining.max(0)
}

/// Count the matching fences (`fence_usage <= usage`).
unsafe fn count_matching(list: *mut DmaResvList, usage: u32) -> u32 {
    if list.is_null() {
        return 0;
    }
    let tbl = table(list);
    let mut n = 0;
    for i in 0..(*list).num_fences as usize {
        let (f, u) = unpack(*tbl.add(i));
        if !f.is_null() && u <= usage {
            n += 1;
        }
    }
    n
}

/// `dma_resv_get_fences(obj, usage, *num, **fences)` — return a freshly
/// allocated array of all matching fences (caller frees with `kfree`).
#[no_mangle]
pub unsafe extern "C" fn dma_resv_get_fences(
    obj: *mut DmaResv,
    usage: u32,
    num_out: *mut u32,
    fences_out: *mut *mut *mut DmaFence,
) -> i32 {
    let list = if obj.is_null() {
        ptr::null_mut()
    } else {
        (*obj).fences
    };
    let count = count_matching(list, usage);
    if count == 0 {
        if !num_out.is_null() {
            *num_out = 0;
        }
        if !fences_out.is_null() {
            *fences_out = ptr::null_mut();
        }
        return 0;
    }
    let arr = mm::kzalloc(count as usize * core::mem::size_of::<*mut DmaFence>(), 0)
        as *mut *mut DmaFence;
    if arr.is_null() {
        return ENOMEM;
    }
    let tbl = table(list);
    let mut k = 0usize;
    for i in 0..(*list).num_fences as usize {
        let (f, u) = unpack(*tbl.add(i));
        if !f.is_null() && u <= usage {
            *arr.add(k) = f;
            k += 1;
        }
    }
    if !num_out.is_null() {
        *num_out = count;
    }
    if !fences_out.is_null() {
        *fences_out = arr;
    }
    0
}

/// `dma_resv_get_singleton(obj, usage, *fence)` — one fence covering all
/// matching: NULL if none, the fence itself if one, else a `dma_fence_array`.
#[no_mangle]
pub unsafe extern "C" fn dma_resv_get_singleton(
    obj: *mut DmaResv,
    usage: u32,
    fence_out: *mut *mut DmaFence,
) -> i32 {
    let mut num: u32 = 0;
    let mut arr: *mut *mut DmaFence = ptr::null_mut();
    let rc = dma_resv_get_fences(obj, usage, &mut num, &mut arr);
    if rc != 0 {
        return rc;
    }
    let result = if num == 0 {
        ptr::null_mut()
    } else if num == 1 {
        let f = *arr;
        mm::kfree(arr as *mut u8);
        f
    } else {
        // The array takes over `arr` as its fences table (reclaimed at daemon
        // teardown alongside the array struct; see module note).
        let array: *mut DmaFenceArray =
            dma_fence_array_create(num as i32, arr, dma_fence_context_alloc(1), 1, false);
        if array.is_null() {
            mm::kfree(arr as *mut u8);
            return ENOMEM;
        }
        core::ptr::addr_of_mut!((*array).base)
    };
    if !fence_out.is_null() {
        *fence_out = result;
    }
    0
}

// ── compile-time layout guard (BTF: Linux 6.6 x86_64) ────────────────────────
const _: () = assert!(core::mem::size_of::<DmaResv>() == 48);
const _: () = assert!(core::mem::offset_of!(DmaResv, fences) == 40);
const _: () = assert!(core::mem::offset_of!(DmaResvList, num_fences) == 16);
const _: () = assert!(core::mem::offset_of!(DmaResvList, max_fences) == 20);
const _: () = assert!(core::mem::size_of::<DmaResvList>() == 24);
const _: () = assert!(core::mem::size_of::<DmaResvIter>() == 48);
const _: () = assert!(core::mem::offset_of!(DmaResvIter, fence) == 16);
const _: () = assert!(core::mem::offset_of!(DmaResvIter, fence_usage) == 24);
const _: () = assert!(core::mem::offset_of!(DmaResvIter, index) == 28);
const _: () = assert!(core::mem::offset_of!(DmaResvIter, fences) == 32);
const _: () = assert!(core::mem::offset_of!(DmaResvIter, num_fences) == 40);
