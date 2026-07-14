//! Scatter/gather list facade (`struct scatterlist` / `struct sg_table`).
//!
//! GPU and block drivers describe non-contiguous buffers as a scatterlist: an
//! array of `(page, offset, length)` tuples that `dma_map_sg` turns into
//! `(dma_address, dma_length)` pairs the device can walk. amdgpu's GEM/TTM and
//! DRM-prime import paths lean on this constantly, so the shim must expose the
//! real ABI (exact struct offsets + the `sg_next` chaining walk) — not a stub.
//!
//! **Daemon model — `struct page` is the page's virtual base.** The userspace
//! driver daemon has one flat address space with no `struct page` array, so a
//! "page" pointer here is the 4 KiB-aligned virtual address itself (the same
//! convention `dma_stream.rs::dma_map_page` already uses). `sg_page` therefore
//! returns that base, and `page_address(page) == page`.
//!
//! **DMA mapping is identity, and that is a documented limitation, not a fake.**
//! `dma_map_sg` sets `dma_address = sg_phys(sg)` (= virtual base + offset),
//! valid only under an identity IOMMU domain for the daemon heap — exactly the
//! same caveat as the streaming-DMA shim. A real distinct IO-VA needs the
//! pending `SYS_LINUXKPI_DMA_PIN(va,len,dir) -> iova` host call (tracked in
//! docs/LINUXKPI_PHASE2.md); until then a driver needing a *separate* device
//! address must use `dma_alloc_coherent`. Coherent buffers are already physical
//! and IOMMU-programmed, so they do not flow through here.
//!
//! Struct layout matches the x86-64 (`CONFIG_NEED_SG_DMA_LENGTH`, no
//! `CONFIG_DEBUG_SG`) kernel exactly, so a real `.ko` compiled against C headers
//! and the host harness (Rust) hit identical offsets.

use crate::mm;

/// 4 KiB page, matching `mm.rs`. A "page" pointer is aligned to this.
const PAGE_SIZE: usize = 4096;
const PAGE_MASK: usize = !(PAGE_SIZE - 1);

/// Low-bit flags packed into `scatterlist.page_link` (Linux `SG_CHAIN`/`SG_END`).
const SG_CHAIN: usize = 0x01;
const SG_END: usize = 0x02;
const SG_FLAG_MASK: usize = SG_CHAIN | SG_END;
const SG_PAGE_MASK: usize = !SG_FLAG_MASK;

/// `-ENOMEM` / `-EIO` as Linux negative errnos.
const ENOMEM: i32 = -12;
const EIO: i32 = -5;

/// `struct scatterlist` — LP64, `CONFIG_NEED_SG_DMA_LENGTH`, no `DEBUG_SG`.
/// Offsets: page_link@0, offset@8, length@12, dma_address@16, dma_length@24.
#[repr(C)]
pub struct Scatterlist {
    /// Page virtual base OR-ed with `SG_CHAIN`/`SG_END` flags in the low 2 bits.
    pub page_link: usize,
    /// Byte offset of the buffer within the page.
    pub offset: u32,
    /// Buffer length in bytes.
    pub length: u32,
    /// Device DMA address (set by `dma_map_sg`).
    pub dma_address: u64,
    /// Mapped length (set by `dma_map_sg`).
    pub dma_length: u32,
}

/// `struct sg_table` — sgl pointer + nents/orig_nents (offsets 0/8/12).
#[repr(C)]
pub struct SgTable {
    pub sgl: *mut Scatterlist,
    pub nents: u32,
    pub orig_nents: u32,
}

#[inline]
fn is_chain(sg: *const Scatterlist) -> bool {
    unsafe { (*sg).page_link & SG_CHAIN != 0 }
}
#[inline]
fn is_last(sg: *const Scatterlist) -> bool {
    unsafe { (*sg).page_link & SG_END != 0 }
}
#[inline]
fn chain_ptr(sg: *const Scatterlist) -> *mut Scatterlist {
    unsafe { ((*sg).page_link & SG_PAGE_MASK) as *mut Scatterlist }
}

/// `sg_page(sg)` — the page virtual base with flag bits masked off.
#[no_mangle]
pub extern "C" fn sg_page(sg: *const Scatterlist) -> *mut u8 {
    if sg.is_null() {
        return core::ptr::null_mut();
    }
    unsafe { ((*sg).page_link & SG_PAGE_MASK) as *mut u8 }
}

/// `sg_assign_page` — set the page, preserving the CHAIN/END flag bits (Linux
/// keeps the marker on re-assignment).
#[no_mangle]
pub extern "C" fn sg_assign_page(sg: *mut Scatterlist, page: *mut u8) {
    if sg.is_null() {
        return;
    }
    unsafe {
        let flags = (*sg).page_link & SG_FLAG_MASK;
        (*sg).page_link = (page as usize & SG_PAGE_MASK) | flags;
    }
}

/// `sg_set_page(sg, page, len, offset)`.
#[no_mangle]
pub extern "C" fn sg_set_page(sg: *mut Scatterlist, page: *mut u8, len: u32, offset: u32) {
    sg_assign_page(sg, page);
    if !sg.is_null() {
        unsafe {
            (*sg).offset = offset;
            (*sg).length = len;
        }
    }
}

/// `sg_set_buf(sg, buf, buflen)` — split a virtual buffer into page base +
/// in-page offset.
#[no_mangle]
pub extern "C" fn sg_set_buf(sg: *mut Scatterlist, buf: *mut u8, buflen: u32) {
    let addr = buf as usize;
    let page = (addr & PAGE_MASK) as *mut u8;
    let offset = (addr & (PAGE_SIZE - 1)) as u32;
    sg_set_page(sg, page, buflen, offset);
}

/// `sg_mark_end(sg)` — mark the final entry; clears any stale CHAIN bit.
#[no_mangle]
pub extern "C" fn sg_mark_end(sg: *mut Scatterlist) {
    if sg.is_null() {
        return;
    }
    unsafe {
        (*sg).page_link |= SG_END;
        (*sg).page_link &= !SG_CHAIN;
    }
}

/// `sg_unmark_end(sg)` — clear the END marker (used before extending a list).
#[no_mangle]
pub extern "C" fn sg_unmark_end(sg: *mut Scatterlist) {
    if !sg.is_null() {
        unsafe { (*sg).page_link &= !SG_END };
    }
}

/// `sg_init_table(sgl, nents)` — zero the array and mark the last entry END.
#[no_mangle]
pub extern "C" fn sg_init_table(sgl: *mut Scatterlist, nents: u32) {
    if sgl.is_null() || nents == 0 {
        return;
    }
    unsafe {
        core::ptr::write_bytes(sgl, 0, nents as usize);
        sg_mark_end(sgl.add((nents - 1) as usize));
    }
}

/// `sg_init_one(sg, buf, buflen)` — single-entry table over one buffer.
#[no_mangle]
pub extern "C" fn sg_init_one(sg: *mut Scatterlist, buf: *mut u8, buflen: u32) {
    sg_init_table(sg, 1);
    sg_set_buf(sg, buf, buflen);
}

/// `sg_next(sg)` — advance to the next entry, following a chain link and
/// stopping at the END marker (returns NULL past the end).
#[no_mangle]
pub extern "C" fn sg_next(sg: *mut Scatterlist) -> *mut Scatterlist {
    if sg.is_null() || is_last(sg) {
        return core::ptr::null_mut();
    }
    let next = unsafe { sg.add(1) };
    if is_chain(next) {
        chain_ptr(next)
    } else {
        next
    }
}

/// `sg_virt(sg)` — CPU virtual address of the buffer (page base + offset).
#[no_mangle]
pub extern "C" fn sg_virt(sg: *const Scatterlist) -> *mut u8 {
    if sg.is_null() {
        return core::ptr::null_mut();
    }
    unsafe { sg_page(sg).add((*sg).offset as usize) }
}

/// `sg_phys(sg)` — physical address (identity == virtual base + offset; see the
/// module note on the daemon's flat address space).
#[no_mangle]
pub extern "C" fn sg_phys(sg: *const Scatterlist) -> u64 {
    sg_virt(sg) as u64
}

/// `sg_nents(sgl)` — count entries by walking `sg_next` to the END marker.
#[no_mangle]
pub extern "C" fn sg_nents(sgl: *mut Scatterlist) -> i32 {
    let mut n = 0i32;
    let mut sg = sgl;
    while !sg.is_null() {
        n += 1;
        sg = sg_next(sg);
    }
    n
}

/// `sg_last(sgl, nents)` — the last entry of an `nents`-long contiguous list.
#[no_mangle]
pub extern "C" fn sg_last(sgl: *mut Scatterlist, nents: u32) -> *mut Scatterlist {
    if sgl.is_null() || nents == 0 {
        return core::ptr::null_mut();
    }
    unsafe { sgl.add((nents - 1) as usize) }
}

/// `sg_alloc_table(table, nents, gfp)` — allocate the sgl array on the daemon
/// heap and initialize it. Returns 0 on success, `-ENOMEM` if the heap is full.
#[no_mangle]
pub extern "C" fn sg_alloc_table(table: *mut SgTable, nents: u32, _gfp: u32) -> i32 {
    if table.is_null() {
        return EIO;
    }
    if nents == 0 {
        unsafe {
            (*table).sgl = core::ptr::null_mut();
            (*table).nents = 0;
            (*table).orig_nents = 0;
        }
        return 0;
    }
    let bytes = (nents as usize) * core::mem::size_of::<Scatterlist>();
    let sgl = mm::kzalloc(bytes, 0) as *mut Scatterlist;
    if sgl.is_null() {
        return ENOMEM;
    }
    sg_init_table(sgl, nents);
    unsafe {
        (*table).sgl = sgl;
        (*table).nents = nents;
        (*table).orig_nents = nents;
    }
    0
}

/// `sg_alloc_table_from_pages(sgt, pages, n_pages, offset, size, gfp)` — build a
/// table mapping a page array. One entry per page (no coalescing — correct, just
/// not minimal); the first entry carries `offset`, lengths sum to `size`.
#[no_mangle]
pub extern "C" fn sg_alloc_table_from_pages(
    sgt: *mut SgTable,
    pages: *mut *mut u8,
    n_pages: u32,
    offset: u32,
    size: u64,
    gfp: u32,
) -> i32 {
    let r = sg_alloc_table(sgt, n_pages, gfp);
    if r != 0 {
        return r;
    }
    if n_pages == 0 || pages.is_null() {
        return 0;
    }
    const PAGE: u64 = 4096;
    unsafe {
        let mut sg = (*sgt).sgl;
        let mut remaining = size;
        let mut off = offset;
        for i in 0..n_pages {
            let page = *pages.add(i as usize);
            let avail = PAGE - off as u64;
            let chunk = if remaining < avail { remaining } else { avail } as u32;
            sg_set_page(sg, page, chunk, off);
            remaining = remaining.saturating_sub(chunk as u64);
            off = 0;
            if i + 1 < n_pages {
                sg = sg_next(sg);
            } else {
                sg_mark_end(sg);
            }
        }
    }
    0
}

/// `sg_free_table(table)` — release the sgl array allocated by `sg_alloc_table`.
#[no_mangle]
pub extern "C" fn sg_free_table(table: *mut SgTable) {
    if table.is_null() {
        return;
    }
    unsafe {
        if !(*table).sgl.is_null() {
            mm::kfree((*table).sgl as *mut u8);
        }
        (*table).sgl = core::ptr::null_mut();
        (*table).nents = 0;
        (*table).orig_nents = 0;
    }
}

// ── Streaming DMA over a scatterlist ─────────────────────────────────────────
// Identity mapping (see module note): each entry's dma_address/dma_length are
// filled from its CPU buffer. Valid under an identity IOMMU domain only.

/// `dma_map_sg(dev, sgl, nents, dir)` — map every entry; returns the number of
/// mapped entries (Linux returns 0 on failure, which is never here).
#[no_mangle]
pub extern "C" fn dma_map_sg(_dev: u64, sgl: *mut Scatterlist, nents: i32, _dir: i32) -> i32 {
    if sgl.is_null() || nents <= 0 {
        return 0;
    }
    let mut sg = sgl;
    let mut mapped = 0i32;
    while !sg.is_null() && mapped < nents {
        unsafe {
            (*sg).dma_address = sg_phys(sg);
            (*sg).dma_length = (*sg).length;
        }
        mapped += 1;
        sg = sg_next(sg);
    }
    mapped
}

#[no_mangle]
pub extern "C" fn dma_map_sg_attrs(
    dev: u64,
    sgl: *mut Scatterlist,
    nents: i32,
    dir: i32,
    _attrs: u64,
) -> i32 {
    dma_map_sg(dev, sgl, nents, dir)
}

/// Coherent-region mapping is identity here, so unmap is a no-op (no IO-VA to
/// release until `SYS_LINUXKPI_DMA_PIN` lands).
#[no_mangle]
pub extern "C" fn dma_unmap_sg(_dev: u64, _sgl: *mut Scatterlist, _nents: i32, _dir: i32) {}

#[no_mangle]
pub extern "C" fn dma_unmap_sg_attrs(
    _dev: u64,
    _sgl: *mut Scatterlist,
    _nents: i32,
    _dir: i32,
    _attrs: u64,
) {
}

/// `dma_map_sgtable(dev, sgt, dir, attrs)` — map `orig_nents` entries and record
/// the mapped count in `sgt->nents`. Returns 0 on success, `-EIO` on failure.
#[no_mangle]
pub extern "C" fn dma_map_sgtable(dev: u64, sgt: *mut SgTable, dir: i32, attrs: u64) -> i32 {
    if sgt.is_null() {
        return EIO;
    }
    let (sgl, orig) = unsafe { ((*sgt).sgl, (*sgt).orig_nents as i32) };
    let n = dma_map_sg_attrs(dev, sgl, orig, dir, attrs);
    if n == 0 {
        return EIO;
    }
    unsafe { (*sgt).nents = n as u32 };
    0
}

#[no_mangle]
pub extern "C" fn dma_unmap_sgtable(_dev: u64, _sgt: *mut SgTable, _dir: i32, _attrs: u64) {}
