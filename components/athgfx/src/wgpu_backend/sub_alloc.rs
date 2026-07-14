use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

/// Represents a massive 64MB or 128MB chunk of HOST_VISIBLE VRAM that was
/// mapped once by the kernel.
pub struct VramSlab {
    pub base_physical_address: u64,
    pub base_virtual_address: u64,
    pub size: u64,
    pub cursor: AtomicU64, // Extremely simple bump allocator for now
}

impl VramSlab {
    pub fn new(physical: u64, virtual_addr: u64, size: u64) -> Self {
        Self {
            base_physical_address: physical,
            base_virtual_address: virtual_addr,
            size,
            cursor: AtomicU64::new(0),
        }
    }

    /// Sub-allocate a chunk of VRAM without a kernel transition.
    /// Returns the physical address for VirtIO and the virtual address for CPU writing.
    pub fn allocate(&self, size: u64, alignment: u64) -> Option<(u64, u64)> {
        let mut current = self.cursor.load(Ordering::Relaxed);
        loop {
            let aligned = (current + alignment - 1) & !(alignment - 1);
            let next = aligned + size;

            if next > self.size {
                return None; // Out of memory in this slab
            }

            match self.cursor.compare_exchange_weak(
                current,
                next,
                Ordering::AcqRel,
                Ordering::Relaxed,
            ) {
                Ok(_) => {
                    let phys = self.base_physical_address + aligned;
                    let virt = self.base_virtual_address + aligned;
                    return Some((phys, virt));
                }
                Err(new_current) => current = new_current,
            }
        }
    }
}
