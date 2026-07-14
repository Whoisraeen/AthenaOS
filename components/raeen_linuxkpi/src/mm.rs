//! `kmalloc` / `kfree` — a real *freeing* heap inside the driver daemon sandbox.
//!
//! Phase 1 was a bump allocator whose `kfree` was a no-op ("daemon restart
//! clears the heap"). That is fine for a one-shot `*_probe()` but FATAL for a
//! running driver: amdgpu allocates metadata (jobs, fences, ring descriptors)
//! on every command submission, so steady-state churn exhausts the 8 MiB heap
//! and `kmalloc` starts returning null. This routes the daemon-local heap
//! through `linked_list_allocator` (the same crate the kernel uses) so
//! `kfree`/`krealloc`/`free_pages` actually reclaim — the shim can host a
//! *running* driver, not just an init-time probe.
//!
//! Bulk DMA payloads still ride `dma_alloc_coherent` (host-backed, physically
//! contiguous); this heap is for driver/daemon metadata only.
//!
//! Concurrency: the daemon is single-threaded and cooperatively scheduled (IRQs
//! arrive via the blocking `irq_wait` syscall, not async preemption), so the
//! `LockedHeap`'s spinlock is always uncontended and never re-entered. If that
//! model ever changes, this stays *correct* (the lock is real) but could spin.

use core::alloc::Layout;
use core::ptr::{self, NonNull};
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use linked_list_allocator::LockedHeap;

/// 256 MiB daemon-local heap. The real amdgpu GFX bring-up (gfx_v11_0_init_microcode
/// + amdgpu_gfx_cp/rlc_init_microcode) kmemdups ~1.2 MiB of GFX ucode plus RLC
/// register/save-restore lists, on top of the PSP/VBIOS/discovery allocations —
/// which exhausted the old 8 MiB heap (gfx_v11_0 early_init returned -ENOMEM).
/// The RLC-backdoor autoload path (RaeenOS bring-up on this passthrough APU)
/// assembles the WHOLE ucode set (pfp/me/mec/rlc/imu/sdma/mes ~1.6 MiB) into an
/// autoload buffer on top of that, so 64 MiB was raised to 256 MiB after amdgpud
/// took a NULL-alloc write right after `rlc autoload enabled` (cap10).
const HEAP_SIZE: usize = 256 * 1024 * 1024;
const PAGE_SIZE: usize = 4096;

/// Minimum alignment guaranteed to every `kmalloc` pointer. 16 matches the
/// x86-64 malloc guarantee and covers u128/most driver structs; allocations
/// needing page alignment go through [`alloc_pages`], and bulk DMA through
/// `dma_alloc_coherent`.
const MIN_ALIGN: usize = 16;
/// Per-allocation header (a multiple of `MIN_ALIGN`) storing the user size, so
/// Linux's `kfree(ptr)` (pointer only, no size) can reconstruct the `Layout`
/// that `linked_list_allocator::deallocate` requires. The user pointer is
/// `raw + HEADER_BYTES`, which stays `MIN_ALIGN`-aligned.
const HEADER_BYTES: usize = MIN_ALIGN;

/// Linux `kmalloc(0)`/`kzalloc(0)` return `ZERO_SIZE_PTR` (the sentinel
/// `(void*)16`), NOT null. Callers test `if (!ptr)` for OOM and treat this
/// non-null sentinel as success, then never dereference it (their length is 0).
/// amdgpu_gfx_rlc_init_microcode_v2_0 does exactly this (reg_list_format is
/// size 0 for RLC v2.x); returning null made it report -ENOMEM. `kfree`/`ksize`
/// recognise the sentinel and skip the header math.
const ZERO_SIZE_PTR: usize = 16;

/// `__GFP_ZERO` (`<linux/gfp_types.h>` bit 8): the allocation must be zeroed. The
/// shim's `kzalloc`/`kcalloc`/`kzalloc_objs` are `static inline`s that call the
/// C-ABI `kmalloc` with this flag OR'd into `flags` — they NEVER reach the Rust
/// `kzalloc` export. So honoring this flag inside `kmalloc` is the ONLY thing that
/// makes a driver's `kzalloc` actually return cleared memory. Dropping it (the pre-
/// 2026-07-01 bug) returned reused-but-unzeroed blocks: amdgpu's IRQ source array
/// (`amdgpu_irq_add_id`) read a stale "[amdgpu]" log-string pointer instead of NULL
/// and `gmc_v11_0_sw_init` failed with -EINVAL. Early allocs hid it (they sit on the
/// pristine, statically-zero heap); the first `kzalloc` to reuse a freed block exposed it.
pub const GFP_ZERO: u32 = 0x100;

static mut HEAP_MEM: [u8; HEAP_SIZE] = [0; HEAP_SIZE];
static HEAP: LockedHeap = LockedHeap::empty();
static HEAP_INIT: AtomicBool = AtomicBool::new(false);
/// Most-recent request rejected by the daemon heap.  Kept separately from the
/// allocator internals so a no_std panic path can report an OOM without trying
/// to allocate or format text itself.
static LAST_ALLOC_FAILURE: AtomicUsize = AtomicUsize::new(0);

pub fn last_alloc_failure_size() -> usize {
    LAST_ALLOC_FAILURE.load(Ordering::Relaxed)
}

/// Initialize the heap region on first use (the static address is only known at
/// runtime). The daemon is single-threaded, but the host KATs run this code from
/// parallel test threads, so the flag alone is not enough: a loser of the old
/// CAS could reach `allocate_first_fit` before the winner finished `init` and
/// get a spurious ENOMEM. Double-check under the heap's own lock instead; the
/// flag stays as the contention-free fast path.
fn ensure_init() {
    if HEAP_INIT.load(Ordering::Acquire) {
        return;
    }
    let mut heap = HEAP.lock();
    if !HEAP_INIT.load(Ordering::Acquire) {
        // SAFETY: runs exactly once (guarded by the heap lock); `addr_of_mut!`
        // avoids a `&mut` to the static (the heap is the sole owner thereafter).
        unsafe {
            heap.init(ptr::addr_of_mut!(HEAP_MEM) as *mut u8, HEAP_SIZE);
        }
        HEAP_INIT.store(true, Ordering::Release);
    }
}

fn alloc_bytes(size: usize, zero: bool) -> *mut u8 {
    if size == 0 {
        return ZERO_SIZE_PTR as *mut u8; // Linux kmalloc(0) semantics (non-null)
    }
    ensure_init();
    let total = match size.checked_add(HEADER_BYTES) {
        Some(t) => t,
        None => return ptr::null_mut(),
    };
    let layout = match Layout::from_size_align(total, MIN_ALIGN) {
        Ok(l) => l,
        Err(_) => return ptr::null_mut(),
    };
    let raw = match HEAP.lock().allocate_first_fit(layout) {
        Ok(p) => p.as_ptr(),
        Err(_) => {
            LAST_ALLOC_FAILURE.store(size, Ordering::Relaxed);
            return ptr::null_mut(); // heap exhausted (out of memory)
        }
    };
    unsafe {
        *(raw as *mut usize) = size;
        let user = raw.add(HEADER_BYTES);
        if zero {
            ptr::write_bytes(user, 0, size);
        }
        user
    }
}

pub fn kmalloc(size: usize, flags: u32) -> *mut u8 {
    // Honor __GFP_ZERO: the shim's kzalloc/kcalloc/kzalloc_objs inline to
    // kmalloc(size, flags | __GFP_ZERO), so this is where driver zeroing is
    // actually enforced (see GFP_ZERO). Non-zeroing kmalloc callers pass flags
    // without the bit and are unaffected.
    alloc_bytes(size, flags & GFP_ZERO != 0)
}

/// Linux `kzalloc` semantics: ALWAYS zeroed, regardless of gfp flags —
/// `kzalloc` is `kmalloc(size, flags | __GFP_ZERO)`, so a driver calling it
/// relies on cleared memory.
pub fn kzalloc(size: usize, _flags: u32) -> *mut u8 {
    alloc_bytes(size, true)
}

/// Free a `kmalloc`/`kzalloc` block — reconstructs the `Layout` from the stored
/// size header and returns the block to the free list.
pub fn kfree(user: *mut u8) {
    if user.is_null() || user as usize == ZERO_SIZE_PTR {
        return;
    }
    unsafe {
        let raw = user.sub(HEADER_BYTES);
        let size = *(raw as *const usize);
        let total = size + HEADER_BYTES; // cannot overflow: it allocated
        if let (Ok(layout), Some(nn)) =
            (Layout::from_size_align(total, MIN_ALIGN), NonNull::new(raw))
        {
            HEAP.lock().deallocate(nn, layout);
        }
    }
}

/// Size recorded in the header below `user` (set by `alloc_bytes`). Used by
/// `krealloc` to copy the old contents. Returns 0 for null.
pub fn alloc_size(user: *mut u8) -> usize {
    if user.is_null() || user as usize == ZERO_SIZE_PTR {
        return 0;
    }
    unsafe { *((user.sub(HEADER_BYTES)) as *const usize) }
}

/// Grow/shrink an allocation (Linux `krealloc`). Allocate-new + copy-min, then
/// (now that the heap frees) release the old block instead of leaking it.
pub fn krealloc(old: *mut u8, new_size: usize, zero: bool) -> *mut u8 {
    if new_size == 0 {
        kfree(old);
        return ptr::null_mut();
    }
    let fresh = alloc_bytes(new_size, zero);
    if fresh.is_null() {
        return ptr::null_mut(); // Linux preserves `old` on failure — don't free it
    }
    if !old.is_null() {
        let copy = alloc_size(old).min(new_size);
        unsafe { ptr::copy_nonoverlapping(old, fresh, copy) };
        kfree(old);
    }
    fresh
}

/// Page allocator backing `__get_free_pages`/`free_pages`. Returns a
/// `PAGE_SIZE`-aligned block of `2^order` pages with no header — `free_pages`
/// reconstructs the `Layout` from `order` alone (Linux's API passes the order
/// back), so the page path frees correctly without a size prefix.
pub fn alloc_pages(order: u32, zero: bool) -> *mut u8 {
    ensure_init();
    let pages = 1usize << order;
    let size = match pages.checked_mul(PAGE_SIZE) {
        Some(s) => s,
        None => return ptr::null_mut(),
    };
    let layout = match Layout::from_size_align(size, PAGE_SIZE) {
        Ok(l) => l,
        Err(_) => return ptr::null_mut(),
    };
    let p = match HEAP.lock().allocate_first_fit(layout) {
        Ok(p) => p.as_ptr(),
        Err(_) => return ptr::null_mut(),
    };
    if zero {
        unsafe { ptr::write_bytes(p, 0, size) };
    }
    p
}

/// Free a block from [`alloc_pages`]; `order` must match the allocation.
pub fn free_pages(p: *mut u8, order: u32) {
    if p.is_null() {
        return;
    }
    let size = (1usize << order) * PAGE_SIZE; // cannot overflow: it allocated
    if let (Ok(layout), Some(nn)) = (Layout::from_size_align(size, PAGE_SIZE), NonNull::new(p)) {
        unsafe { HEAP.lock().deallocate(nn, layout) };
    }
}

/// `struct sysinfo` — layout mirrors `<linux/mm.h>` (see linuxkpi-drm shim).
/// TTM reads `totalram * mem_unit` in `ttm_device_init` to size its GTT address
/// space and page pools to ~50% of host RAM.
#[repr(C)]
pub struct Sysinfo {
    pub uptime: i64,
    pub totalram: u64,
    pub freeram: u64,
    pub sharedram: u64,
    pub bufferram: u64,
    pub totalswap: u64,
    pub freeswap: u64,
    pub totalhigh: u64,
    pub freehigh: u64,
    pub mem_unit: u32,
}

/// `si_meminfo` — report a plausible host memory size so TTM sizes the GTT
/// address-space manager (and the shrinker page limit) sensibly. GTT is an
/// address-space allocator (drm_mm ranges); reporting a large total does not
/// reserve physical memory — BOs still populate lazily from the daemon heap /
/// `dma_alloc_coherent`. Without this the struct is left uninitialised and GTT
/// comes up at 0 MiB, so kernel GART BOs fail to allocate. 8 GiB matches the
/// Athena's real RAM; mem_unit = 1 keeps totalram in bytes.
#[no_mangle]
pub unsafe extern "C" fn si_meminfo(si: *mut Sysinfo) {
    if si.is_null() {
        return;
    }
    let total_bytes: u64 = 8 * 1024 * 1024 * 1024; // 8 GiB
    core::ptr::write(
        si,
        Sysinfo {
            uptime: 0,
            totalram: total_bytes,
            freeram: total_bytes,
            sharedram: 0,
            bufferram: 0,
            totalswap: 0,
            freeswap: 0,
            totalhigh: 0, // x86-64: no highmem
            freehigh: 0,
            mem_unit: 1,
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// THE regression guard for the gmc_v11_0_sw_init bug: the shim's `kzalloc`
    /// inlines to `kmalloc(size, flags | __GFP_ZERO)`, so `kmalloc` MUST clear the
    /// block when that bit is set — even when first-fit hands back a just-freed,
    /// dirty block. Before the fix this returned the stale bytes (amdgpu read an
    /// "[amdgpu]" log-string pointer where a NULL IRQ-source slot belonged) and
    /// gmc sw_init failed -EINVAL. FAIL-able: dirty → free → re-request zeroed.
    #[test]
    fn kmalloc_gfp_zero_clears_reused_block() {
        const N: usize = 512;
        let a = kmalloc(N, 0);
        assert!(!a.is_null(), "first kmalloc failed");
        unsafe { ptr::write_bytes(a, 0xAB, N) };
        kfree(a);
        // The kzalloc path: first-fit should reuse the block just freed.
        let b = kmalloc(N, GFP_ZERO);
        assert!(!b.is_null(), "zeroed kmalloc failed");
        let bytes = unsafe { core::slice::from_raw_parts(b, N) };
        assert!(
            bytes.iter().all(|&x| x == 0),
            "kmalloc(__GFP_ZERO) returned NON-zeroed reused memory — the gmc sw_init bug"
        );
        kfree(b);
    }

    /// Plain `kmalloc` (no __GFP_ZERO) must NOT pay the zeroing cost — proves the
    /// flag is actually gating, not that we blanket-zero everything.
    #[test]
    fn kmalloc_without_gfp_zero_skips_clear() {
        const N: usize = 512;
        let a = kmalloc(N, 0);
        assert!(!a.is_null());
        unsafe { ptr::write_bytes(a, 0xCD, N) };
        kfree(a);
        let b = kmalloc(N, 0); // no zero flag
        assert!(!b.is_null());
        // Reused block retains the dirty pattern (documents the flag gates zeroing).
        let first = unsafe { *b };
        kfree(b);
        assert_eq!(
            first, 0xCD,
            "expected reused block to keep its bytes when __GFP_ZERO is absent"
        );
    }
}
