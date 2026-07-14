//! TTM — Translation Table Maps: the VRAM / system-memory buffer manager amdgpu
//! uses for all GPU-visible memory. TTM decides whether a buffer lives in VRAM,
//! GTT (GPU-accessible system RAM via the GART), or is evicted to system memory.

extern crate alloc;
use alloc::vec::Vec;

/// TTM placement domains (`TTM_PL_*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TtmPlacement {
    /// Dedicated video memory (on an iGPU, a carve-out of system RAM).
    Vram,
    /// GPU-accessible system memory through the GART/GPUVM.
    Gtt,
    /// CPU system memory (evicted / not GPU-resident).
    System,
}

/// `struct ttm_resource` — where a buffer currently lives.
#[derive(Debug, Clone, Copy)]
pub struct TtmResource {
    pub placement: TtmPlacement,
    pub start_page: u64,
    pub num_pages: u64,
    pub gpu_offset: u64,
}

/// `struct ttm_buffer_object` — a managed GPU buffer with migration support.
pub struct TtmBufferObject {
    pub bo_id: u64,
    pub size: usize,
    pub resource: TtmResource,
    pub pinned: bool,
    /// Backing DMA address (constant for pinned VRAM/GTT bos).
    pub dma_addr: u64,
    pub cpu_addr: *mut u8,
}

/// The VRAM manager: tracks a contiguous video-memory region and hands out
/// page ranges. On an APU (Radeon 780M) "VRAM" is a carve-out of system DRAM.
pub struct TtmVramManager {
    pub base: u64,
    pub size_bytes: u64,
    pub next_page: u64,
    pub total_pages: u64,
    allocations: Vec<(u64, u64)>, // (start_page, num_pages)
}

impl TtmVramManager {
    /// `ttm_range_man_init` — initialise the VRAM range manager.
    pub fn init(base: u64, size_bytes: u64) -> Self {
        Self {
            base,
            size_bytes,
            next_page: 0,
            total_pages: size_bytes / 4096,
            allocations: Vec::new(),
        }
    }

    /// `ttm_bo_init` + `ttm_bo_validate` — allocate a buffer in the given domain.
    pub fn alloc_bo(
        &mut self,
        dev: u64,
        size: usize,
        placement: TtmPlacement,
    ) -> Option<TtmBufferObject> {
        let num_pages = ((size + 4095) / 4096) as u64;

        let (gpu_offset, start_page, dma_addr, cpu_addr) = match placement {
            TtmPlacement::Vram => {
                if self.next_page + num_pages > self.total_pages {
                    return None; // VRAM full → caller evicts and retries
                }
                let start = self.next_page;
                self.next_page += num_pages;
                self.allocations.push((start, num_pages));
                (
                    self.base + start * 4096,
                    start,
                    self.base + start * 4096,
                    core::ptr::null_mut(),
                )
            }
            TtmPlacement::Gtt | TtmPlacement::System => {
                // GTT/system: back with a coherent DMA allocation (GART-mapped).
                let a = raeen_linuxkpi::dma::dma_alloc_coherent(dev, size);
                if a.is_null() {
                    return None;
                }
                (a.dma_addr, 0, a.dma_addr, a.cpu_addr)
            }
        };

        Some(TtmBufferObject {
            bo_id: 0,
            size,
            resource: TtmResource {
                placement,
                start_page,
                num_pages,
                gpu_offset,
            },
            pinned: false,
            dma_addr,
            cpu_addr,
        })
    }

    pub fn used_pages(&self) -> u64 {
        self.next_page
    }
}
