//! Streaming DMA mapping API (`dma_map_single` / `dma_map_page` / sync).
//!
//! Coherent DMA (`dma.rs`) is the correct, host-backed path: the host allocates
//! contiguous frames and programs the IOMMU. Streaming DMA instead maps a
//! *pre-existing* driver buffer for one transfer. Doing that correctly needs a
//! host syscall to pin a daemon virtual address and return its IOMMU IO-VA —
//! which does not exist yet. Until it does, these shims pass the address through
//! (valid only under an identity IOMMU domain for the daemon heap) and exist so
//! drivers that call the streaming API LINK and run; `dma_mapping_error`
//! correctly flags the unmappable case. The limitation is documented, not faked
//! as success: a driver that depends on a real distinct IO-VA must use
//! `dma_alloc_coherent`.
//!
//! NOTE for follow-up: add `SYS_LINUXKPI_DMA_PIN(va, len, dir) -> iova` to the
//! host so this becomes a real translation. Tracked in docs/LINUXKPI_PHASE2.md.

/// Linux `dma_data_direction`.
pub const DMA_BIDIRECTIONAL: i32 = 0;
pub const DMA_TO_DEVICE: i32 = 1;
pub const DMA_FROM_DEVICE: i32 = 2;

/// Sentinel returned when a mapping cannot be created.
const DMA_MAPPING_ERROR: u64 = u64::MAX;

/// `dma_map_single(dev, cpu_addr, size, dir)` → dma_addr_t.
#[no_mangle]
pub extern "C" fn dma_map_single(_dev: u64, cpu_addr: *mut u8, size: usize, _dir: i32) -> u64 {
    if cpu_addr.is_null() || size == 0 {
        return DMA_MAPPING_ERROR;
    }
    // Identity pass-through (see module note). Real IO-VA needs the host pin.
    cpu_addr as u64
}

#[no_mangle]
pub extern "C" fn dma_unmap_single(_dev: u64, _dma_addr: u64, _size: usize, _dir: i32) {}

/// `dma_map_page(dev, page, offset, size, dir)`. `page` here is the page virtual
/// base (the daemon has no `struct page`); add the offset.
#[no_mangle]
pub extern "C" fn dma_map_page(
    _dev: u64,
    page: *mut u8,
    offset: usize,
    size: usize,
    _dir: i32,
) -> u64 {
    if page.is_null() || size == 0 {
        return DMA_MAPPING_ERROR;
    }
    (page as u64).wrapping_add(offset as u64)
}

#[no_mangle]
pub extern "C" fn dma_unmap_page(_dev: u64, _dma_addr: u64, _size: usize, _dir: i32) {}

/// `dma_mapping_error(dev, dma_addr)` → nonzero if the mapping failed.
#[no_mangle]
pub extern "C" fn dma_mapping_error(_dev: u64, dma_addr: u64) -> i32 {
    (dma_addr == DMA_MAPPING_ERROR || dma_addr == 0) as i32
}

// Cache-sync ops: the daemon DMA region is coherent (host maps it uncached /
// the platform is cache-coherent for PCIe), so these are no-ops.
#[no_mangle]
pub extern "C" fn dma_sync_single_for_cpu(_dev: u64, _dma_addr: u64, _size: usize, _dir: i32) {}
#[no_mangle]
pub extern "C" fn dma_sync_single_for_device(_dev: u64, _dma_addr: u64, _size: usize, _dir: i32) {}
