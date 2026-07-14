//! Extended Linux allocator surface — the `*alloc`/`*free` family beyond the
//! bare `kmalloc`/`kzalloc`/`kfree` in lib.rs. Every Linux driver pulls in a
//! handful of these; all route to the daemon-local bump heap (`mm.rs`).
//!
//! `vmalloc` (virtually-contiguous) and `kmalloc` (physically-contiguous) are
//! the same backing store here — the daemon is one address space and bulk DMA
//! goes through `dma_alloc_coherent`, so the distinction collapses safely.

use crate::mm;

#[no_mangle]
pub extern "C" fn vmalloc(size: usize) -> *mut u8 {
    mm::kmalloc(size, 0)
}
#[no_mangle]
pub extern "C" fn vzalloc(size: usize) -> *mut u8 {
    mm::kzalloc(size, 0)
}
#[no_mangle]
pub extern "C" fn vfree(ptr: *mut u8) {
    mm::kfree(ptr);
}
#[no_mangle]
pub extern "C" fn kvmalloc(size: usize, _flags: u32) -> *mut u8 {
    mm::kmalloc(size, 0)
}
#[no_mangle]
pub extern "C" fn kvzalloc(size: usize, _flags: u32) -> *mut u8 {
    mm::kzalloc(size, 0)
}
#[no_mangle]
pub extern "C" fn kvfree(ptr: *mut u8) {
    mm::kfree(ptr);
}

/// `kcalloc(n, size, flags)` — zeroed array, overflow-checked like Linux.
#[no_mangle]
pub extern "C" fn kcalloc(n: usize, size: usize, _flags: u32) -> *mut u8 {
    match n.checked_mul(size) {
        Some(total) => mm::kzalloc(total, 0),
        None => core::ptr::null_mut(),
    }
}
#[no_mangle]
pub extern "C" fn kmalloc_array(n: usize, size: usize, flags: u32) -> *mut u8 {
    match n.checked_mul(size) {
        Some(total) => mm::kmalloc(total, flags),
        None => core::ptr::null_mut(),
    }
}

#[no_mangle]
pub extern "C" fn krealloc(ptr: *mut u8, new_size: usize, _flags: u32) -> *mut u8 {
    mm::krealloc(ptr, new_size, false)
}

/// `kmemdup(src, len, flags)` — allocate + copy.
#[no_mangle]
pub unsafe extern "C" fn kmemdup(src: *const u8, len: usize, flags: u32) -> *mut u8 {
    let dst = mm::kmalloc(len, flags);
    if !dst.is_null() && !src.is_null() {
        core::ptr::copy_nonoverlapping(src, dst, len);
    }
    dst
}

/// `kstrdup(s, flags)` — duplicate a NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn kstrdup(s: *const u8, flags: u32) -> *mut u8 {
    if s.is_null() {
        return core::ptr::null_mut();
    }
    let mut len = 0usize;
    while *s.add(len) != 0 {
        len += 1;
    }
    let dst = mm::kmalloc(len + 1, flags);
    if !dst.is_null() {
        core::ptr::copy_nonoverlapping(s, dst, len + 1);
    }
    dst
}

// ── devm_* (device-managed) ──────────────────────────────────────────────────
// Linux auto-frees devm_ allocations on driver detach. The daemon model frees
// everything on teardown/restart, so managed == plain alloc here.

#[no_mangle]
pub extern "C" fn devm_kzalloc(_dev: u64, size: usize, _flags: u32) -> *mut u8 {
    mm::kzalloc(size, 0)
}
#[no_mangle]
pub extern "C" fn devm_kmalloc(_dev: u64, size: usize, flags: u32) -> *mut u8 {
    mm::kmalloc(size, flags)
}
#[no_mangle]
pub extern "C" fn devm_kfree(_dev: u64, ptr: *mut u8) {
    mm::kfree(ptr);
}

// ── Page allocator (__get_free_pages family) ─────────────────────────────────
// `order` is log2(pages). Page-aligned, and freed via the order (which Linux's
// `free_pages` passes back), so no size header is needed.

#[no_mangle]
pub extern "C" fn __get_free_pages(_flags: u32, order: u32) -> *mut u8 {
    mm::alloc_pages(order, false)
}
#[no_mangle]
pub extern "C" fn get_zeroed_page(_flags: u32) -> *mut u8 {
    mm::alloc_pages(0, true)
}
#[no_mangle]
pub extern "C" fn __get_free_page(_flags: u32) -> *mut u8 {
    mm::alloc_pages(0, false)
}
#[no_mangle]
pub extern "C" fn free_pages(addr: *mut u8, order: u32) {
    mm::free_pages(addr, order);
}
#[no_mangle]
pub extern "C" fn free_page(addr: *mut u8) {
    mm::free_pages(addr, 0);
}

// ── Modern slab ABI ───────────────────────────────────────────────────────────
// On a recent kernel `kmalloc(size, flags)` is an inline: a constant size hits
// `kmalloc_trace(kmalloc_caches[type][index], flags, size)`, a large size hits
// `kmalloc_large`, and a runtime size hits `__kmalloc`. To LINK a real modern
// `.ko` the shim must export all four plus the `kmalloc_caches` data symbol the
// inline indexes. The cache pointer is irrelevant here (we size-allocate from
// the daemon heap), so `kmalloc_caches` is just a generously over-sized zero
// array: whatever `[type][index]` slot the inline reads is in-bounds and yields
// a (null, ignored) cache pointer.

/// `kmalloc_caches[type][index]` — the inline reads a `struct kmem_cache *` here
/// then passes it to `kmalloc_trace` (which ignores it). Over-sized vs any real
/// `NR_KMALLOC_TYPES` × `KMALLOC_SHIFT_HIGH+1`; all-zero so any in-bounds offset
/// reads a null (ignored) pointer regardless of the kernel's exact stride.
#[no_mangle]
pub static kmalloc_caches: [[usize; 64]; 16] = [[0; 64]; 16];

/// `kmalloc_trace(cache, flags, size)` — constant-size kmalloc fast path; the
/// cache is ignored, we allocate `size` from the daemon heap.
#[no_mangle]
pub extern "C" fn kmalloc_trace(_cache: *mut u8, flags: u32, size: usize) -> *mut u8 {
    mm::kmalloc(size, flags)
}

/// `__kmalloc(size, flags)` — runtime-size kmalloc.
#[no_mangle]
pub extern "C" fn __kmalloc(size: usize, flags: u32) -> *mut u8 {
    mm::kmalloc(size, flags)
}
#[no_mangle]
pub extern "C" fn __kmalloc_node(size: usize, flags: u32, _node: i32) -> *mut u8 {
    mm::kmalloc(size, flags)
}

/// `kmalloc_large(size, flags)` — large-allocation path (still the daemon heap;
/// `mm` serves multi-page allocations from the same arena).
#[no_mangle]
pub extern "C" fn kmalloc_large(size: usize, flags: u32) -> *mut u8 {
    mm::kmalloc(size, flags)
}
#[no_mangle]
pub extern "C" fn kmalloc_large_node(size: usize, flags: u32, _node: i32) -> *mut u8 {
    mm::kmalloc(size, flags)
}

/// `kvmalloc_node(size, flags, node)` — kvmalloc backing (virt-contiguous ==
/// phys-contiguous here; bulk DMA uses `dma_alloc_coherent`).
#[no_mangle]
pub extern "C" fn kvmalloc_node(size: usize, flags: u32, _node: i32) -> *mut u8 {
    mm::kmalloc(size, flags)
}

/// `krealloc_array(p, new_n, size, flags)` — overflow-checked array realloc.
#[no_mangle]
pub extern "C" fn krealloc_array(p: *mut u8, new_n: usize, size: usize, _flags: u32) -> *mut u8 {
    match new_n.checked_mul(size) {
        Some(total) => mm::krealloc(p, total, false),
        None => core::ptr::null_mut(),
    }
}

// ── kernel 7.0 allocation-profiling (`_noprof`) variants ──────────────────────
// Since 6.10 the alloc-profiling macros wrap each allocator: the real entry point
// carries a `_noprof` suffix and the ORIGINAL signature, while the bare name is a
// macro that injects a codegen tag. A compiled `.ko` references the `_noprof`
// symbols directly, so alias them to the base impls (the tag is irrelevant here).

#[no_mangle]
pub extern "C" fn __kmalloc_noprof(size: usize, flags: u32) -> *mut u8 {
    mm::kmalloc(size, flags)
}
#[no_mangle]
pub extern "C" fn __kmalloc_node_noprof(size: usize, flags: u32, _node: i32) -> *mut u8 {
    mm::kmalloc(size, flags)
}
#[no_mangle]
pub extern "C" fn __kmalloc_large_noprof(size: usize, flags: u32) -> *mut u8 {
    mm::kmalloc(size, flags)
}
/// `__kmalloc_cache_noprof(cache, flags, size)` — constant-size fast path; the
/// cache pointer is ignored (we size-allocate from the daemon heap).
#[no_mangle]
pub extern "C" fn __kmalloc_cache_noprof(_cache: *mut u8, flags: u32, size: usize) -> *mut u8 {
    mm::kmalloc(size, flags)
}
#[no_mangle]
pub extern "C" fn __kvmalloc_node_noprof(size: usize, flags: u32, _node: i32) -> *mut u8 {
    mm::kmalloc(size, flags)
}
#[no_mangle]
pub extern "C" fn vmalloc_noprof(size: usize) -> *mut u8 {
    mm::kmalloc(size, 0)
}
#[no_mangle]
pub extern "C" fn vzalloc_noprof(size: usize) -> *mut u8 {
    mm::kzalloc(size, 0)
}
#[no_mangle]
pub unsafe extern "C" fn kmemdup_noprof(src: *const u8, len: usize, flags: u32) -> *mut u8 {
    kmemdup(src, len, flags)
}
/// `get_free_pages_noprof(flags, order)` — returns the page virtual address as an
/// `unsigned long` (0 on failure), like Linux's `__get_free_pages`.
#[no_mangle]
pub extern "C" fn get_free_pages_noprof(_flags: u32, order: u32) -> usize {
    mm::alloc_pages(order, false) as usize
}
#[no_mangle]
pub extern "C" fn alloc_pages_noprof(_flags: u32, order: u32) -> *mut u8 {
    mm::alloc_pages(order, false)
}
