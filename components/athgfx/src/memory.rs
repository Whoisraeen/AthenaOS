//! GPU Memory Manager — userspace view of GPU memory allocation.
//!
//! This provides a Vulkan-style memory type system where allocations are
//! categorized by access pattern (device-local VRAM, host-visible staging,
//! host-coherent for streaming). The kernel's VramAllocator handles the
//! actual physical allocation; this module tracks the logical view.

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

// ═══════════════════════════════════════════════════════════════════════════
// Memory types
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryType {
    /// VRAM on the GPU die — fastest GPU access, not CPU-accessible.
    DeviceLocal,
    /// CPU-accessible memory visible to GPU — used for staging uploads.
    HostVisible,
    /// CPU-accessible, no explicit flush needed — for streaming buffers.
    HostCoherent,
    /// Shared memory accessible by both CPU and GPU at full speed (APU/iGPU).
    Shared,
}

impl MemoryType {
    pub fn is_host_accessible(&self) -> bool {
        matches!(self, Self::HostVisible | Self::HostCoherent | Self::Shared)
    }

    pub fn is_device_local(&self) -> bool {
        matches!(self, Self::DeviceLocal | Self::Shared)
    }
}

pub type MemoryTypeIndex = u32;

// ═══════════════════════════════════════════════════════════════════════════
// Allocation tracking
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AllocUsage {
    VertexBuffer,
    IndexBuffer,
    UniformBuffer,
    StorageBuffer,
    Texture,
    RenderTarget,
    DepthBuffer,
    Staging,
    CommandBuffer,
}

#[derive(Debug, Clone)]
pub struct GpuAllocation {
    pub id: u32,
    pub offset: u64,
    pub size: u64,
    pub alignment: u64,
    pub memory_type: MemoryTypeIndex,
    pub usage: AllocUsage,
    pub mapped: bool,
    pub label: Option<&'static str>,
}

// ═══════════════════════════════════════════════════════════════════════════
// Free block tracking (first-fit allocator with coalescing)
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
struct FreeBlock {
    offset: u64,
    size: u64,
}

#[derive(Debug)]
struct MemoryHeap {
    memory_type: MemoryType,
    total_size: u64,
    used_size: u64,
    free_list: Vec<FreeBlock>,
}

impl MemoryHeap {
    fn new(memory_type: MemoryType, total_size: u64) -> Self {
        Self {
            memory_type,
            total_size,
            used_size: 0,
            free_list: alloc::vec![FreeBlock {
                offset: 0,
                size: total_size
            }],
        }
    }

    fn allocate(&mut self, size: u64, alignment: u64) -> Option<u64> {
        let align = if alignment == 0 { 1 } else { alignment };

        for i in 0..self.free_list.len() {
            let block = &self.free_list[i];
            let aligned_offset = (block.offset + align - 1) & !(align - 1);
            let padding = aligned_offset - block.offset;
            let total_needed = padding + size;

            if block.size >= total_needed {
                let remaining_before = padding;
                let remaining_after = block.size - total_needed;
                let old_offset = block.offset;

                self.free_list.remove(i);

                if remaining_before > 0 {
                    self.free_list.push(FreeBlock {
                        offset: old_offset,
                        size: remaining_before,
                    });
                }
                if remaining_after > 0 {
                    self.free_list.push(FreeBlock {
                        offset: aligned_offset + size,
                        size: remaining_after,
                    });
                }

                self.used_size += size;
                return Some(aligned_offset);
            }
        }
        None
    }

    fn free(&mut self, offset: u64, size: u64) {
        self.free_list.push(FreeBlock { offset, size });
        self.used_size = self.used_size.saturating_sub(size);
        self.coalesce();
    }

    fn coalesce(&mut self) {
        if self.free_list.len() < 2 {
            return;
        }
        self.free_list.sort_by_key(|b| b.offset);

        let mut merged = Vec::with_capacity(self.free_list.len());
        merged.push(self.free_list[0].clone());

        for block in self.free_list.iter().skip(1) {
            let last = merged.last_mut().unwrap();
            if last.offset + last.size == block.offset {
                last.size += block.size;
            } else {
                merged.push(block.clone());
            }
        }
        self.free_list = merged;
    }

    fn available(&self) -> u64 {
        self.total_size - self.used_size
    }

    fn fragmentation(&self) -> f32 {
        if self.free_list.is_empty() || self.total_size == 0 {
            return 0.0;
        }
        let largest_block = self.free_list.iter().map(|b| b.size).max().unwrap_or(0);
        let total_free = self.available();
        if total_free == 0 {
            return 0.0;
        }
        1.0 - (largest_block as f32 / total_free as f32)
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// GPU Memory Manager
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy)]
pub struct MemoryStats {
    pub total_vram: u64,
    pub used_vram: u64,
    pub total_host: u64,
    pub used_host: u64,
    pub allocation_count: usize,
    pub fragmentation: f32,
}

pub struct GpuMemoryManager {
    heaps: Vec<MemoryHeap>,
    allocations: BTreeMap<u32, GpuAllocation>,
    next_id: u32,
    memory_types: Vec<MemoryType>,
}

impl GpuMemoryManager {
    pub fn new(vram_size: u64, host_visible_size: u64, host_coherent_size: u64) -> Self {
        let mut heaps = Vec::new();
        let mut memory_types = Vec::new();

        if vram_size > 0 {
            heaps.push(MemoryHeap::new(MemoryType::DeviceLocal, vram_size));
            memory_types.push(MemoryType::DeviceLocal);
        }
        if host_visible_size > 0 {
            heaps.push(MemoryHeap::new(MemoryType::HostVisible, host_visible_size));
            memory_types.push(MemoryType::HostVisible);
        }
        if host_coherent_size > 0 {
            heaps.push(MemoryHeap::new(
                MemoryType::HostCoherent,
                host_coherent_size,
            ));
            memory_types.push(MemoryType::HostCoherent);
        }

        Self {
            heaps,
            allocations: BTreeMap::new(),
            next_id: 1,
            memory_types,
        }
    }

    pub fn with_shared(total_size: u64) -> Self {
        let mut heaps = Vec::new();
        let mut memory_types = Vec::new();
        heaps.push(MemoryHeap::new(MemoryType::Shared, total_size));
        memory_types.push(MemoryType::Shared);
        Self {
            heaps,
            allocations: BTreeMap::new(),
            next_id: 1,
            memory_types,
        }
    }

    pub fn allocate(
        &mut self,
        size: u64,
        alignment: u64,
        memory_type_index: MemoryTypeIndex,
        usage: AllocUsage,
        label: Option<&'static str>,
    ) -> Option<u32> {
        let heap_idx = memory_type_index as usize;
        if heap_idx >= self.heaps.len() {
            return None;
        }
        if size == 0 {
            return None;
        }

        let offset = self.heaps[heap_idx].allocate(size, alignment)?;

        let id = self.next_id;
        self.next_id += 1;

        self.allocations.insert(
            id,
            GpuAllocation {
                id,
                offset,
                size,
                alignment,
                memory_type: memory_type_index,
                usage,
                mapped: false,
                label,
            },
        );

        Some(id)
    }

    pub fn free(&mut self, id: u32) -> bool {
        if let Some(alloc) = self.allocations.remove(&id) {
            let heap_idx = alloc.memory_type as usize;
            if heap_idx < self.heaps.len() {
                self.heaps[heap_idx].free(alloc.offset, alloc.size);
            }
            true
        } else {
            false
        }
    }

    pub fn map(&mut self, id: u32) -> bool {
        if let Some(alloc) = self.allocations.get_mut(&id) {
            let heap_idx = alloc.memory_type as usize;
            if heap_idx < self.heaps.len() && self.heaps[heap_idx].memory_type.is_host_accessible()
            {
                alloc.mapped = true;
                return true;
            }
        }
        false
    }

    pub fn unmap(&mut self, id: u32) -> bool {
        if let Some(alloc) = self.allocations.get_mut(&id) {
            alloc.mapped = false;
            true
        } else {
            false
        }
    }

    pub fn get_allocation(&self, id: u32) -> Option<&GpuAllocation> {
        self.allocations.get(&id)
    }

    pub fn find_memory_type(&self, usage: AllocUsage) -> MemoryTypeIndex {
        match usage {
            AllocUsage::VertexBuffer
            | AllocUsage::IndexBuffer
            | AllocUsage::Texture
            | AllocUsage::RenderTarget
            | AllocUsage::DepthBuffer => self.find_type_index(MemoryType::DeviceLocal).unwrap_or(0),
            AllocUsage::UniformBuffer | AllocUsage::StorageBuffer => self
                .find_type_index(MemoryType::HostCoherent)
                .or_else(|| self.find_type_index(MemoryType::HostVisible))
                .or_else(|| self.find_type_index(MemoryType::DeviceLocal))
                .unwrap_or(0),
            AllocUsage::Staging => self
                .find_type_index(MemoryType::HostVisible)
                .or_else(|| self.find_type_index(MemoryType::HostCoherent))
                .unwrap_or(0),
            AllocUsage::CommandBuffer => self.find_type_index(MemoryType::DeviceLocal).unwrap_or(0),
        }
    }

    fn find_type_index(&self, target: MemoryType) -> Option<MemoryTypeIndex> {
        for (i, mt) in self.memory_types.iter().enumerate() {
            if *mt == target {
                return Some(i as MemoryTypeIndex);
            }
        }
        None
    }

    pub fn stats(&self) -> MemoryStats {
        let mut total_vram = 0u64;
        let mut used_vram = 0u64;
        let mut total_host = 0u64;
        let mut used_host = 0u64;
        let mut max_frag = 0.0f32;

        for heap in &self.heaps {
            match heap.memory_type {
                MemoryType::DeviceLocal => {
                    total_vram += heap.total_size;
                    used_vram += heap.used_size;
                }
                MemoryType::HostVisible | MemoryType::HostCoherent => {
                    total_host += heap.total_size;
                    used_host += heap.used_size;
                }
                MemoryType::Shared => {
                    total_vram += heap.total_size;
                    used_vram += heap.used_size;
                    total_host += heap.total_size;
                    used_host += heap.used_size;
                }
            }
            let frag = heap.fragmentation();
            if frag > max_frag {
                max_frag = frag;
            }
        }

        MemoryStats {
            total_vram,
            used_vram,
            total_host,
            used_host,
            allocation_count: self.allocations.len(),
            fragmentation: max_frag,
        }
    }

    pub fn allocation_count(&self) -> usize {
        self.allocations.len()
    }

    pub fn defragment_hint(&self) -> bool {
        self.heaps.iter().any(|h| h.fragmentation() > 0.5)
    }

    pub fn memory_type_count(&self) -> usize {
        self.memory_types.len()
    }

    pub fn get_memory_type(&self, index: MemoryTypeIndex) -> Option<MemoryType> {
        self.memory_types.get(index as usize).copied()
    }
}
