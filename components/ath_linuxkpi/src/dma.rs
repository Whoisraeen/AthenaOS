//! Zero-copy DMA bridge — Phase 3 of the LinuxKPI host.
//!
//! `dma_alloc_coherent` is the performance-critical interception. The Linux
//! driver believes it owns the DMA buffer; in reality the AthenaOS host allocates
//! physically-contiguous frames, programs the device's IOMMU domain to permit
//! DMA into exactly those frames, and hands back both the virtual address (for
//! the driver to write descriptors) and the DMA/physical address (for the driver
//! to program into the hardware).
//!
//! The actual payload (textures, vertices, packets) is written by the *app*
//! directly into the same physical frames via a shared-memory capability — the
//! LinuxKPI host copies zero bytes. The driver only ever touches metadata.

use crate::host;

/// Result of a coherent DMA allocation.
#[derive(Clone, Copy)]
pub struct DmaAlloc {
    /// CPU virtual address — the driver writes descriptors / metadata here.
    pub cpu_addr: *mut u8,
    /// DMA (physical) address — programmed into the hardware so the device
    /// knows where to read/write. IOMMU-restricted to this region only.
    pub dma_addr: u64,
    pub size: usize,
    /// Opaque token for `dma_free_coherent`.
    pub token: u64,
}

impl DmaAlloc {
    pub fn is_null(&self) -> bool {
        self.cpu_addr.is_null()
    }
}

/// `dma_alloc_coherent` — allocate IOMMU-sandboxed contiguous DMA memory.
pub fn dma_alloc_coherent(dev: u64, size: usize) -> DmaAlloc {
    // Host writes [virt, phys, size, token] (4 x u64) into this buffer.
    let mut result = [0u64; 4];
    let rc = unsafe { host::sys_dma_alloc(dev, size as u64, result.as_mut_ptr() as u64) };
    if rc != 0 {
        return DmaAlloc {
            cpu_addr: core::ptr::null_mut(),
            dma_addr: 0,
            size: 0,
            token: 0,
        };
    }
    DmaAlloc {
        cpu_addr: result[0] as *mut u8,
        dma_addr: result[1],
        size: result[2] as usize,
        token: result[3],
    }
}

/// `dma_free_coherent`.
pub fn dma_free_coherent(dev: u64, alloc: &DmaAlloc) {
    if alloc.token != 0 {
        unsafe { host::sys_dma_free(dev, alloc.token) };
    }
}
