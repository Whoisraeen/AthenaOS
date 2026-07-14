//! GEM — Graphics Execution Manager: buffer-object handles shared between the
//! driver, userspace, and the GPU. amdgpu wraps GEM objects in `amdgpu_bo`.

extern crate alloc;

/// `struct drm_gem_object` — a named GPU memory buffer.
pub struct DrmGemObject {
    pub handle: u64,
    pub size: usize,
    /// CPU virtual address (if mapped), else null.
    pub cpu_addr: *mut u8,
    /// GPU/DMA address the hardware uses.
    pub gpu_addr: u64,
    /// Reference count (`drm_gem_object_get/put`).
    pub refcount: u32,
}

impl DrmGemObject {
    /// `drm_gem_object_init` — create a GEM object backed by a DMA allocation.
    /// `dev` is the LinuxKPI device handle (for `dma_alloc_coherent`).
    pub fn create(dev: u64, size: usize) -> Option<Self> {
        let alloc = raeen_linuxkpi::dma::dma_alloc_coherent(dev, size);
        if alloc.is_null() {
            return None;
        }
        Some(Self {
            handle: 0, // assigned by DrmDevice::alloc_handle
            size: alloc.size,
            cpu_addr: alloc.cpu_addr,
            gpu_addr: alloc.dma_addr,
            refcount: 1,
        })
    }

    pub fn get(&mut self) {
        self.refcount += 1;
    }

    pub fn put(&mut self, dev: u64) {
        if self.refcount > 0 {
            self.refcount -= 1;
        }
        if self.refcount == 0 && !self.cpu_addr.is_null() {
            // The DMA token is tracked by the daemon's DmaAlloc; freed there.
            let _ = dev;
        }
    }
}
