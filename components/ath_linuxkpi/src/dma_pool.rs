//! `dma_pool` — fixed-size coherent-DMA object allocator.
//!
//! Drivers create a `dma_pool` once (fixed object size + alignment) and then
//! cheaply hand out many same-sized DMA-able chunks: amdgpu uses them for SDMA
//! descriptors, fence/semaphore words, and ring metadata. The shim must return
//! real, alignment-correct memory plus its DMA address and free it later — a
//! stub that leaked or returned NULL would wedge ring setup.
//!
//! **Backing + DMA-address model.** Each object is one (or more) pages from the
//! daemon heap (`mm::alloc_pages`), so it is page-aligned — which satisfies
//! every realistic `dma_pool` alignment (always ≤ PAGE_SIZE). The DMA address is
//! identity (= CPU address), valid under an identity IOMMU domain for the daemon
//! heap — exactly the documented limitation shared by `dma_stream.rs` /
//! `scatterlist.rs`, pending the `SYS_LINUXKPI_DMA_PIN` host call
//! (docs/LINUXKPI_PHASE2.md). A buffer that needs a *distinct* device address
//! must use `dma_alloc_coherent` (host-backed, real IOMMU-programmed).
//!
//! **One page minimum per object** (no sub-page packing yet): correct but
//! wastes space for tiny objects. amdgpu's pools are modest, so this is fine for
//! bring-up; packing is a later optimization, not a correctness gap. Because
//! every object in a pool is the same size, the free path recomputes the page
//! `order` from the pool — no per-object bookkeeping is needed.

use crate::mm;

const PAGE_SIZE: usize = 4096;

/// Shim-side `struct dma_pool` (opaque to the driver — only our pointer escapes).
#[repr(C)]
struct DmaPool {
    /// Object size in bytes (what the driver asked for).
    obj_size: usize,
    /// Page-allocation order covering one object.
    order: u32,
}

/// log2(ceil(bytes / PAGE_SIZE)) — the `alloc_pages` order for one object.
fn order_for(bytes: usize) -> u32 {
    let pages = bytes.div_ceil(PAGE_SIZE).max(1);
    // smallest order whose 2^order >= pages
    let mut order = 0u32;
    while (1usize << order) < pages {
        order += 1;
    }
    order
}

/// `dma_pool_create(name, dev, size, align, boundary)` — returns an opaque pool
/// handle, or NULL on allocation failure / invalid size.
///
/// `align` is satisfied implicitly (objects are page-aligned, ≥ any sane align);
/// `boundary` is not separately enforced — each object is a standalone
/// allocation, so it never straddles a boundary unless `size` itself exceeds it
/// (caller-controlled).
#[no_mangle]
pub extern "C" fn dma_pool_create(
    _name: *const u8,
    _dev: u64,
    size: usize,
    _align: usize,
    _boundary: usize,
) -> *mut u8 {
    if size == 0 {
        return core::ptr::null_mut();
    }
    let pool = mm::kzalloc(core::mem::size_of::<DmaPool>(), 0) as *mut DmaPool;
    if pool.is_null() {
        return core::ptr::null_mut();
    }
    unsafe {
        (*pool).obj_size = size;
        (*pool).order = order_for(size);
    }
    pool as *mut u8
}

/// `dmam_pool_create` — device-managed variant. The daemon frees everything on
/// teardown, so managed == plain create here.
#[no_mangle]
pub extern "C" fn dmam_pool_create(
    name: *const u8,
    dev: u64,
    size: usize,
    align: usize,
    boundary: usize,
) -> *mut u8 {
    dma_pool_create(name, dev, size, align, boundary)
}

fn alloc_obj(pool: *mut u8, handle: *mut u64, zero: bool) -> *mut u8 {
    if pool.is_null() {
        return core::ptr::null_mut();
    }
    let order = unsafe { (*(pool as *mut DmaPool)).order };
    let p = mm::alloc_pages(order, zero);
    if p.is_null() {
        return core::ptr::null_mut();
    }
    if !handle.is_null() {
        // Identity DMA address (see module note).
        unsafe { *handle = p as u64 };
    }
    p
}

/// `dma_pool_alloc(pool, flags, handle)` — one object; writes its DMA address to
/// `*handle`. Returns the CPU address (NULL on failure).
#[no_mangle]
pub extern "C" fn dma_pool_alloc(pool: *mut u8, _flags: u32, handle: *mut u64) -> *mut u8 {
    alloc_obj(pool, handle, false)
}

/// `dma_pool_zalloc(pool, flags, handle)` — zeroed object.
#[no_mangle]
pub extern "C" fn dma_pool_zalloc(pool: *mut u8, _flags: u32, handle: *mut u64) -> *mut u8 {
    alloc_obj(pool, handle, true)
}

/// `dma_pool_free(pool, vaddr, dma)` — return an object to the heap. The page
/// `order` is recomputed from the pool (all objects share a size), so the `dma`
/// address is not needed for the free.
#[no_mangle]
pub extern "C" fn dma_pool_free(pool: *mut u8, vaddr: *mut u8, _dma: u64) {
    if pool.is_null() || vaddr.is_null() {
        return;
    }
    let order = unsafe { (*(pool as *mut DmaPool)).order };
    mm::free_pages(vaddr, order);
}

/// `dma_pool_destroy(pool)` — free the pool descriptor. Objects must already be
/// freed (Linux requires the same; outstanding objects are a driver bug).
#[no_mangle]
pub extern "C" fn dma_pool_destroy(pool: *mut u8) {
    if !pool.is_null() {
        mm::kfree(pool);
    }
}

/// `dmam_pool_destroy` — device-managed destroy (same as plain here).
#[no_mangle]
pub extern "C" fn dmam_pool_destroy(pool: *mut u8) {
    dma_pool_destroy(pool);
}
