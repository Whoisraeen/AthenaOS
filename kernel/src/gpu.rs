//! GPU hardware acceleration — PCI discovery, BAR mapping, VRAM management,
//! command ring, VirtIO-GPU protocol, and display scanout.
//!
//! Priority path: VirtIO-GPU in QEMU, with stubs for real Intel/AMD/NVIDIA
//! hardware that read GPU identity registers.

#![allow(dead_code)]

extern crate alloc;

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::vec;
use alloc::vec::Vec;
use core::ptr;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use spin::Mutex;

use crate::memory::{GlobalFrameAllocator, PHYS_MEM_OFFSET};
use crate::pci::{self, PciDevice};
use x86_64::structures::paging::FrameAllocator;

// ─── GPU Type & Device Structures ───────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuType {
    VirtioGpu,
    BochsVbe,
    IntelIntegrated,
    AmdDiscrete,
    NvidiaDiscrete,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuVendor {
    Intel,
    Amd,
    Nvidia,
    Virtio,
    Unknown(u16),
}

impl GpuVendor {
    fn from_id(vendor_id: u16) -> Self {
        match vendor_id {
            0x8086 => Self::Intel,
            0x1002 => Self::Amd,
            0x10DE => Self::Nvidia,
            0x1AF4 => Self::Virtio,
            v => Self::Unknown(v),
        }
    }
}

#[derive(Debug)]
pub struct GpuDevice {
    pub vendor_id: u16,
    pub device_id: u16,
    pub pci_bus: u8,
    pub pci_dev: u8,
    pub pci_func: u8,
    pub mmio_base: u64,
    pub mmio_size: u64,
    pub vram_base: u64,
    pub vram_size: u64,
    pub gpu_type: GpuType,
}

#[derive(Debug, Clone, Copy)]
pub struct DisplayMode {
    pub width: u32,
    pub height: u32,
    pub refresh_hz: u32,
    pub bpp: u8,
    pub hdr: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct GpuCapabilities {
    pub vulkan: bool,
    pub compute: bool,
    pub ray_tracing: bool,
    pub vrr: bool,
    pub hdr: bool,
    pub hw_video_decode: bool,
}

// ─── GPU MMIO Interface ─────────────────────────────────────────────────────

struct GpuMmio {
    base_virt: u64,
    size: u64,
}

impl GpuMmio {
    /// Map a GPU BAR region into kernel virtual space via the bootloader's
    /// physical-memory identity map. The region is accessed uncacheable
    /// through volatile reads/writes.
    fn map(phys_base: u64, size: u64) -> Option<Self> {
        let offset = PHYS_MEM_OFFSET.get()?.as_u64();
        Some(Self {
            base_virt: offset + phys_base,
            size,
        })
    }

    #[inline]
    unsafe fn read32(&self, offset: u64) -> u32 {
        debug_assert!(offset + 4 <= self.size);
        ptr::read_volatile((self.base_virt + offset) as *const u32)
    }

    #[inline]
    unsafe fn write32(&self, offset: u64, val: u32) {
        debug_assert!(offset + 4 <= self.size);
        ptr::write_volatile((self.base_virt + offset) as *mut u32, val);
    }

    #[inline]
    unsafe fn read64(&self, offset: u64) -> u64 {
        debug_assert!(offset + 8 <= self.size);
        ptr::read_volatile((self.base_virt + offset) as *const u64)
    }

    #[inline]
    unsafe fn write64(&self, offset: u64, val: u64) {
        debug_assert!(offset + 8 <= self.size);
        ptr::write_volatile((self.base_virt + offset) as *mut u64, val);
    }

    fn virt_ptr(&self) -> *mut u8 {
        self.base_virt as *mut u8
    }
}

// ─── VRAM Allocator ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BufferUsage {
    VertexBuffer,
    IndexBuffer,
    UniformBuffer,
    Framebuffer,
    Texture,
    CommandBuffer,
    Staging,
}

#[derive(Debug, Clone)]
pub struct VramRegion {
    pub offset: u64,
    pub size: u64,
}

#[derive(Debug, Clone)]
pub struct GpuBuffer {
    pub id: u32,
    pub vram_offset: u64,
    pub size: u64,
    pub usage: BufferUsage,
}

pub struct VramAllocator {
    base: u64,
    size: u64,
    free_list: Vec<VramRegion>,
    next_buffer_id: AtomicU32,
}

impl VramAllocator {
    pub fn new(base: u64, size: u64) -> Self {
        let free_list = vec![VramRegion { offset: 0, size }];
        Self {
            base,
            size,
            free_list,
            next_buffer_id: AtomicU32::new(1),
        }
    }

    pub fn alloc_vram(
        &mut self,
        size: u64,
        alignment: u64,
        usage: BufferUsage,
    ) -> Option<GpuBuffer> {
        let align = if alignment == 0 { 1 } else { alignment };

        for i in 0..self.free_list.len() {
            let region = &self.free_list[i];
            let aligned_offset = (region.offset + align - 1) & !(align - 1);
            let padding = aligned_offset - region.offset;
            let total_needed = padding + size;

            if region.size >= total_needed {
                let id = self.next_buffer_id.fetch_add(1, Ordering::Relaxed);

                let remaining_before = padding;
                let remaining_after = region.size - total_needed;
                let old_offset = region.offset;

                self.free_list.remove(i);

                if remaining_before > 0 {
                    self.free_list.push(VramRegion {
                        offset: old_offset,
                        size: remaining_before,
                    });
                }
                if remaining_after > 0 {
                    self.free_list.push(VramRegion {
                        offset: aligned_offset + size,
                        size: remaining_after,
                    });
                }

                return Some(GpuBuffer {
                    id,
                    vram_offset: aligned_offset,
                    size,
                    usage,
                });
            }
        }
        None
    }

    pub fn free_vram(&mut self, buffer: &GpuBuffer) {
        self.free_list.push(VramRegion {
            offset: buffer.vram_offset,
            size: buffer.size,
        });
        self.coalesce();
    }

    fn coalesce(&mut self) {
        if self.free_list.len() < 2 {
            return;
        }
        self.free_list.sort_by_key(|r| r.offset);

        let mut merged = Vec::with_capacity(self.free_list.len());
        merged.push(self.free_list[0].clone());

        for region in self.free_list.iter().skip(1) {
            let last = merged.last_mut().unwrap();
            if last.offset + last.size == region.offset {
                last.size += region.size;
            } else {
                merged.push(region.clone());
            }
        }
        self.free_list = merged;
    }

    pub fn vram_stats(&self) -> (u64, u64, u64) {
        let free: u64 = self.free_list.iter().map(|r| r.size).sum();
        let used = self.size - free;
        (self.size, used, free)
    }
}

// ─── GPU Command Ring ───────────────────────────────────────────────────────

const GPU_CMD_NOP: u32 = 0;
const GPU_CMD_FILL: u32 = 1;
const GPU_CMD_COPY: u32 = 2;
const GPU_CMD_FLIP: u32 = 3;
const GPU_CMD_FENCE: u32 = 4;

const RING_SIZE: u32 = 256;
const RING_ENTRY_SIZE: u32 = 64;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct GpuCommandHeader {
    pub cmd_type: u32,
    pub flags: u32,
    pub fence_id: u64,
    pub size: u32,
    pub _pad: u32,
}

#[repr(C)]
pub struct GpuCommandRing {
    base_virt: u64,
    base_phys: u64,
    size: u32,
    head: AtomicU32,
    tail: AtomicU32,
    fence_completed: AtomicU64,
    doorbell_mmio: Option<u64>,
    doorbell_offset: u32,
}

impl GpuCommandRing {
    fn new(vram_alloc: &mut VramAllocator, mmio: Option<(&GpuMmio, u32)>) -> Option<Self> {
        let ring_bytes = (RING_SIZE * RING_ENTRY_SIZE) as u64;

        let buffer = vram_alloc.alloc_vram(ring_bytes, 4096, BufferUsage::CommandBuffer)?;

        let (db_mmio, db_offset) = match mmio {
            Some((m, off)) => (Some(m.base_virt), off),
            None => (None, 0),
        };

        Some(Self {
            base_virt: buffer.vram_offset,
            base_phys: buffer.vram_offset,
            size: RING_SIZE,
            head: AtomicU32::new(0),
            tail: AtomicU32::new(0),
            fence_completed: AtomicU64::new(0),
            doorbell_mmio: db_mmio,
            doorbell_offset: db_offset,
        })
    }

    pub fn submit_command(&self, cmd_type: u32, fence_id: u64, payload: &[u8]) -> bool {
        let tail = self.tail.load(Ordering::Acquire);
        let head = self.head.load(Ordering::Acquire);
        let next_tail = (tail + 1) % self.size;

        if next_tail == head {
            return false;
        }

        let entry_offset = (tail as u64) * (RING_ENTRY_SIZE as u64);
        let entry_ptr = (self.base_virt + entry_offset) as *mut u8;

        let header = GpuCommandHeader {
            cmd_type,
            flags: 0,
            fence_id,
            size: payload.len() as u32,
            _pad: 0,
        };

        unsafe {
            ptr::write_volatile(entry_ptr as *mut GpuCommandHeader, header);

            let payload_ptr = entry_ptr.add(core::mem::size_of::<GpuCommandHeader>());
            let max_payload = RING_ENTRY_SIZE as usize - core::mem::size_of::<GpuCommandHeader>();
            let copy_len = payload.len().min(max_payload);
            for i in 0..copy_len {
                ptr::write_volatile(payload_ptr.add(i), payload[i]);
            }
        }

        core::sync::atomic::fence(Ordering::Release);
        self.tail.store(next_tail, Ordering::Release);

        self.ring_doorbell();
        true
    }

    pub fn poll_completion(&self) -> bool {
        let head = self.head.load(Ordering::Acquire);
        let tail = self.tail.load(Ordering::Acquire);
        head == tail
    }

    pub fn fence_wait(&self, fence_id: u64) {
        loop {
            if self.fence_completed.load(Ordering::Acquire) >= fence_id {
                return;
            }
            core::hint::spin_loop();
        }
    }

    pub fn advance_head(&self) {
        let head = self.head.load(Ordering::Acquire);
        let new_head = (head + 1) % self.size;
        self.head.store(new_head, Ordering::Release);
    }

    pub fn signal_fence(&self, fence_id: u64) {
        self.fence_completed.fetch_max(fence_id, Ordering::Release);
    }

    fn ring_doorbell(&self) {
        if let Some(mmio_base) = self.doorbell_mmio {
            unsafe {
                ptr::write_volatile(
                    (mmio_base + self.doorbell_offset as u64) as *mut u32,
                    self.tail.load(Ordering::Relaxed),
                );
            }
        }
    }
}

// ─── VirtIO-GPU Protocol ────────────────────────────────────────────────────

mod virtio_gpu_cmd {
    pub const VIRTIO_GPU_F_VIRGL: u32 = 1;

    // 2D Commands
    pub const GET_DISPLAY_INFO: u32 = 0x0100;
    pub const RESOURCE_CREATE_2D: u32 = 0x0101;
    pub const RESOURCE_UNREF: u32 = 0x0102;
    pub const SET_SCANOUT: u32 = 0x0103;
    pub const RESOURCE_FLUSH: u32 = 0x0104;
    pub const TRANSFER_TO_HOST_2D: u32 = 0x0105;
    pub const RESOURCE_ATTACH_BACKING: u32 = 0x0106;
    pub const RESOURCE_DETACH_BACKING: u32 = 0x0107;

    // 3D Commands
    pub const VIRTIO_GPU_CMD_CTX_CREATE: u32 = 0x0200;
    pub const VIRTIO_GPU_CMD_CTX_DESTROY: u32 = 0x0201;
    pub const VIRTIO_GPU_CMD_CTX_ATTACH_RESOURCE: u32 = 0x0202;
    pub const VIRTIO_GPU_CMD_CTX_DETACH_RESOURCE: u32 = 0x0203;
    pub const VIRTIO_GPU_CMD_RESOURCE_CREATE_3D: u32 = 0x0204;
    pub const VIRTIO_GPU_CMD_TRANSFER_TO_HOST_3D: u32 = 0x0205;
    pub const VIRTIO_GPU_CMD_TRANSFER_FROM_HOST_3D: u32 = 0x0206;
    pub const VIRTIO_GPU_CMD_SUBMIT_3D: u32 = 0x0207;

    pub const RESP_OK_NODATA: u32 = 0x1100;
    pub const RESP_OK_DISPLAY_INFO: u32 = 0x1101;
    pub const RESP_ERR_UNSPEC: u32 = 0x1200;

    pub const FORMAT_B8G8R8A8_UNORM: u32 = 1;
    pub const FORMAT_R8G8B8A8_UNORM: u32 = 67;
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct VirtioGpuCtrlHdr {
    hdr_type: u32,
    flags: u32,
    fence_id: u64,
    ctx_id: u32,
    _padding: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct VirtioGpuResourceCreate2d {
    hdr: VirtioGpuCtrlHdr,
    resource_id: u32,
    format: u32,
    width: u32,
    height: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct VirtioGpuSetScanout {
    hdr: VirtioGpuCtrlHdr,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    scanout_id: u32,
    resource_id: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct VirtioGpuTransferToHost2d {
    hdr: VirtioGpuCtrlHdr,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    offset: u64,
    resource_id: u32,
    _padding: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct VirtioGpuResourceFlush {
    hdr: VirtioGpuCtrlHdr,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    resource_id: u32,
    _padding: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct VirtioGpuResourceAttachBacking {
    hdr: VirtioGpuCtrlHdr,
    resource_id: u32,
    nr_entries: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct VirtioGpuMemEntry {
    addr: u64,
    length: u32,
    _padding: u32,
}

// VirtIO common config offsets (within the MMIO BAR, transport-specific)
mod virtio_regs {
    pub const DEVICE_FEATURES: u64 = 0x00;
    pub const GUEST_FEATURES: u64 = 0x04;
    pub const QUEUE_ADDRESS: u64 = 0x08;
    pub const QUEUE_SIZE: u64 = 0x0C;
    pub const QUEUE_SELECT: u64 = 0x0E;
    pub const QUEUE_NOTIFY: u64 = 0x10;
    pub const DEVICE_STATUS: u64 = 0x12;
    pub const ISR_STATUS: u64 = 0x13;
}

const VIRTQ_SIZE: u16 = 64;

#[repr(C, align(16))]
#[derive(Clone, Copy)]
struct VirtqDesc {
    addr: u64,
    len: u32,
    flags: u16,
    next: u16,
}

const VIRTQ_DESC_F_NEXT: u16 = 1;
const VIRTQ_DESC_F_WRITE: u16 = 2;

#[repr(C, align(2))]
#[derive(Clone, Copy)]
struct VirtqAvail {
    flags: u16,
    idx: u16,
    ring: [u16; 64],
}

#[repr(C, align(4))]
#[derive(Clone, Copy)]
struct VirtqUsedElem {
    id: u32,
    len: u32,
}

#[repr(C, align(4))]
#[derive(Clone, Copy)]
struct VirtqUsed {
    flags: u16,
    idx: u16,
    ring: [VirtqUsedElem; 64],
}

struct Virtqueue {
    desc_phys: u64,
    desc_virt: *mut VirtqDesc,
    avail_virt: *mut VirtqAvail,
    used_virt: *mut VirtqUsed,
    size: u16,
    free_head: u16,
    last_used_idx: u16,
    num_free: u16,
}

impl Virtqueue {
    fn alloc(queue_size: u16) -> Option<Self> {
        let desc_bytes = (queue_size as usize) * core::mem::size_of::<VirtqDesc>();
        let avail_bytes = 6 + 2 * (queue_size as usize);
        let used_bytes = 6 + 8 * (queue_size as usize);
        let total = desc_bytes + avail_bytes + used_bytes;
        let pages = (total + 4095) / 4096;

        // The split virtqueue lays desc/avail/used out at fixed byte offsets
        // within one region, so the backing frames MUST be physically
        // contiguous. Allocating frames one-at-a-time (the old code) only kept
        // the first frame's address while the avail/used structures and the
        // zero-fill spanned `pages` frames that were never contiguous — the same
        // class of bug that, in create_scanout, trampled the heap on multi-vCPU
        // boots. Harmless today at qsize=64 (1 page) but wrong for any
        // multi-page queue; allocate one contiguous power-of-two block.
        let order = pages.next_power_of_two().trailing_zeros() as u8;
        let phys = crate::memory::allocate_contiguous_frames(order)?.as_u64();
        let offset = PHYS_MEM_OFFSET.get()?.as_u64();
        let virt = offset + phys;

        unsafe {
            ptr::write_bytes(virt as *mut u8, 0, pages * 4096);
        }

        let desc_virt = virt as *mut VirtqDesc;
        let avail_virt = (virt + desc_bytes as u64) as *mut VirtqAvail;
        let avail_end = virt + desc_bytes as u64 + avail_bytes as u64;
        let used_virt_addr = (avail_end + 4095) & !4095;
        let used_virt = used_virt_addr as *mut VirtqUsed;

        unsafe {
            for i in 0..queue_size {
                let d = &mut *desc_virt.add(i as usize);
                d.addr = 0;
                d.len = 0;
                d.flags = 0;
                d.next = if i + 1 < queue_size { i + 1 } else { 0 };
            }
        }

        Some(Self {
            desc_phys: phys,
            desc_virt,
            avail_virt,
            used_virt,
            size: queue_size,
            free_head: 0,
            last_used_idx: 0,
            num_free: queue_size,
        })
    }

    fn page_number(&self) -> u32 {
        (self.desc_phys / 4096) as u32
    }

    unsafe fn push_request(&mut self, bufs: &[(u64, u32, u16)]) -> Option<u16> {
        if bufs.is_empty() || self.num_free < bufs.len() as u16 {
            return None;
        }

        let head = self.free_head;
        let mut idx = head;

        for (i, &(addr, len, flags)) in bufs.iter().enumerate() {
            let d = &mut *self.desc_virt.add(idx as usize);
            d.addr = addr;
            d.len = len;
            d.flags = flags;
            if i + 1 < bufs.len() {
                d.flags |= VIRTQ_DESC_F_NEXT;
                d.next = (idx + 1) % self.size;
            } else {
                d.flags &= !VIRTQ_DESC_F_NEXT;
            }
            self.free_head = d.next;
            self.num_free -= 1;
            idx = d.next;
        }

        let avail = &mut *self.avail_virt;
        let avail_idx = avail.idx;
        avail.ring[(avail_idx % self.size) as usize] = head;
        core::sync::atomic::fence(Ordering::Release);
        avail.idx = avail_idx.wrapping_add(1);

        Some(head)
    }

    unsafe fn poll_used(&mut self) -> Option<(u16, u32)> {
        core::sync::atomic::fence(Ordering::Acquire);
        let used = &*self.used_virt;
        if self.last_used_idx == used.idx {
            return None;
        }
        let elem = used.ring[(self.last_used_idx % self.size) as usize];
        self.last_used_idx = self.last_used_idx.wrapping_add(1);

        let mut idx = elem.id as u16;
        loop {
            let d = &*self.desc_virt.add(idx as usize);
            let has_next = d.flags & VIRTQ_DESC_F_NEXT != 0;
            let next = d.next;
            let old_free = self.free_head;
            let dd = &mut *self.desc_virt.add(idx as usize);
            dd.next = old_free;
            self.free_head = idx;
            self.num_free += 1;
            if !has_next {
                break;
            }
            idx = next;
        }

        Some((elem.id as u16, elem.len))
    }
}

// ─── Display Scanout ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    Bgra8,
    Rgba8,
}

#[derive(Debug, Clone)]
pub struct Scanout {
    pub id: u32,
    pub resource_id: u32,
    pub fb_vram_offset: u64,
    pub fb_phys: u64,
    pub fb_virt: *mut u8,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub format: PixelFormat,
}

unsafe impl Send for Scanout {}
unsafe impl Sync for Scanout {}

// ─── Shader Cache (retained from original) ──────────────────────────────────

pub struct ShaderCacheEntry {
    pub hash: [u8; 32],
    pub binary: Vec<u8>,
    pub pipeline_id: u64,
}

pub struct ShaderCache {
    entries: BTreeMap<u64, ShaderCacheEntry>,
    max_entries: usize,
}

impl ShaderCache {
    pub fn new(max_entries: usize) -> Self {
        Self {
            entries: BTreeMap::new(),
            max_entries,
        }
    }

    pub fn insert(&mut self, id: u64, entry: ShaderCacheEntry) {
        if self.entries.len() >= self.max_entries {
            self.evict_oldest();
        }
        self.entries.insert(id, entry);
    }

    pub fn get(&self, id: &u64) -> Option<&ShaderCacheEntry> {
        self.entries.get(id)
    }

    pub fn evict_oldest(&mut self) {
        if let Some(&oldest_key) = self.entries.keys().next() {
            self.entries.remove(&oldest_key);
        }
    }
}

// ─── GPU Driver Trait ───────────────────────────────────────────────────────

#[derive(Debug)]
pub struct GpuInfo {
    pub vendor: GpuVendor,
    pub device_id: u16,
    pub vram_bytes: u64,
    pub name: &'static str,
    pub pci_bus: u8,
    pub pci_slot: u8,
}

pub enum GpuCommand {
    SetMode(DisplayMode),
    Present { fb_addr: u64, stride: u32 },
    Vsync,
    FenceWait(u64),
    FenceSignal(u64),
}

pub trait GpuDriver: Send {
    fn info(&self) -> &GpuInfo;
    fn capabilities(&self) -> GpuCapabilities;
    fn submit(&mut self, cmd: &GpuCommand) -> Result<(), &'static str>;
    fn current_mode(&self) -> DisplayMode;
    fn supported_modes(&self) -> Vec<DisplayMode>;
}

// ─── VirtIO-GPU Driver ─────────────────────────────────────────────────────

struct VirtioGpuDriver {
    gpu_info: GpuInfo,
    device: GpuDevice,
    mode: DisplayMode,
    mmio: GpuMmio,
    controlq: Virtqueue,
    cursorq: Virtqueue,
    vram_alloc: VramAllocator,
    cmd_ring: Option<GpuCommandRing>,
    scanout: Option<Scanout>,
    next_resource_id: u32,
    iommu_domain: Option<u16>,
    req_buf_phys: u64,
    req_buf_virt: *mut u8,
    resp_buf_phys: u64,
    resp_buf_virt: *mut u8,
}

// SAFETY: VirtioGpuDriver is only accessed behind COMPOSITOR/GPU mutex.
// The raw pointers (mmio, virtqueue, buf pointers) reference kernel-owned
// memory that outlives the driver and is never aliased outside the lock.
unsafe impl Send for VirtioGpuDriver {}

impl VirtioGpuDriver {
    fn new(pci_dev: &PciDevice) -> Option<Self> {
        let vendor_id = pci_dev.vendor_id;
        let device_id = pci_dev.device_id;

        let mmio_base = pci::bar_address(pci_dev, 0)?;
        let mmio_size = Self::probe_bar_size(pci_dev, 0);

        let vram_base = pci::bar_address(pci_dev, 2).unwrap_or(0);
        let vram_size = if vram_base != 0 {
            Self::probe_bar_size(pci_dev, 2)
        } else {
            16 * 1024 * 1024
        };

        let device = GpuDevice {
            vendor_id,
            device_id,
            pci_bus: pci_dev.bus,
            pci_dev: pci_dev.device,
            pci_func: pci_dev.function,
            mmio_base,
            mmio_size,
            vram_base,
            vram_size,
            gpu_type: GpuType::VirtioGpu,
        };

        let mmio = GpuMmio::map(mmio_base, mmio_size)?;

        enable_bus_master(pci_dev);

        unsafe {
            mmio.write32(virtio_regs::DEVICE_STATUS, 0);
            mmio.write32(virtio_regs::DEVICE_STATUS, 1);
            mmio.write32(virtio_regs::DEVICE_STATUS, 1 | 2);

            let features = mmio.read32(virtio_regs::DEVICE_FEATURES);
            let mut guest_features = 0;
            if (features & virtio_gpu_cmd::VIRTIO_GPU_F_VIRGL) != 0 {
                guest_features |= virtio_gpu_cmd::VIRTIO_GPU_F_VIRGL;
                crate::serial_println!("[gpu] VirtIO-GPU Virgl 3D feature negotiated");
            }
            mmio.write32(virtio_regs::GUEST_FEATURES, guest_features);
            mmio.write32(virtio_regs::DEVICE_STATUS, 1 | 2 | 8);
        }

        let controlq = Self::setup_virtqueue(&mmio, 0)?;
        let cursorq = Self::setup_virtqueue(&mmio, 1)?;

        unsafe {
            mmio.write32(virtio_regs::DEVICE_STATUS, 1 | 2 | 4 | 8);
        }

        let mut alloc = GlobalFrameAllocator;
        let req_frame = alloc.allocate_frame()?;
        let resp_frame = alloc.allocate_frame()?;
        let offset = PHYS_MEM_OFFSET.get()?.as_u64();

        let req_phys = req_frame.start_address().as_u64();
        let req_virt = (offset + req_phys) as *mut u8;
        let resp_phys = resp_frame.start_address().as_u64();
        let resp_virt = (offset + resp_phys) as *mut u8;

        unsafe {
            ptr::write_bytes(req_virt, 0, 4096);
            ptr::write_bytes(resp_virt, 0, 4096);
        }

        let vram_alloc = VramAllocator::new(vram_base, vram_size);

        let iommu_domain = crate::iommu::create_domain();
        if let Some(dom_id) = iommu_domain {
            crate::iommu::assign_device(dom_id, pci_dev.bus, pci_dev.device, pci_dev.function);

            crate::iommu::map_dma(dom_id, req_phys, req_phys, 4096, true, true);
            crate::iommu::map_dma(dom_id, resp_phys, resp_phys, 4096, true, true);

            let desc_phys = controlq.desc_phys;
            crate::iommu::map_dma(dom_id, desc_phys, desc_phys, 4096 * 4, true, true);
            if cursorq.desc_phys != desc_phys {
                crate::iommu::map_dma(
                    dom_id,
                    cursorq.desc_phys,
                    cursorq.desc_phys,
                    4096 * 4,
                    true,
                    true,
                );
            }

            crate::serial_println!(
                "[gpu] IOMMU domain {} assigned to VirtIO-GPU {:02x}:{:02x}.{}",
                dom_id,
                pci_dev.bus,
                pci_dev.device,
                pci_dev.function
            );
        }

        let gpu_info = GpuInfo {
            vendor: GpuVendor::Virtio,
            device_id,
            vram_bytes: vram_size,
            name: "VirtIO GPU",
            pci_bus: pci_dev.bus,
            pci_slot: pci_dev.device,
        };

        Some(Self {
            gpu_info,
            device,
            mode: DisplayMode {
                width: 1024,
                height: 768,
                refresh_hz: 60,
                bpp: 32,
                hdr: false,
            },
            mmio,
            controlq,
            cursorq,
            vram_alloc,
            cmd_ring: None,
            scanout: None,
            next_resource_id: 1,
            iommu_domain,
            req_buf_phys: req_phys,
            req_buf_virt: req_virt,
            resp_buf_phys: resp_phys,
            resp_buf_virt: resp_virt,
        })
    }

    fn probe_bar_size(pci_dev: &PciDevice, bar_idx: u8) -> u64 {
        let offset = 0x10 + (bar_idx * 4);
        let original = pci::read_config_32(pci_dev.bus, pci_dev.device, pci_dev.function, offset);

        pci::write_config_32(
            pci_dev.bus,
            pci_dev.device,
            pci_dev.function,
            offset,
            0xFFFF_FFFF,
        );
        let readback = pci::read_config_32(pci_dev.bus, pci_dev.device, pci_dev.function, offset);

        pci::write_config_32(
            pci_dev.bus,
            pci_dev.device,
            pci_dev.function,
            offset,
            original,
        );

        if readback == 0 || readback == 0xFFFF_FFFF {
            return 0;
        }

        let is_io = original & 1 != 0;
        let mask = if is_io {
            readback & !0x03
        } else {
            readback & !0x0F
        };
        let size = (!mask).wrapping_add(1) as u64;
        if size == 0 {
            4096
        } else {
            size
        }
    }

    fn setup_virtqueue(mmio: &GpuMmio, queue_idx: u16) -> Option<Virtqueue> {
        unsafe {
            mmio.write32(virtio_regs::QUEUE_SELECT, queue_idx as u32);

            let max_size = mmio.read32(virtio_regs::QUEUE_SIZE) as u16;
            let qsize = if max_size == 0 || max_size > VIRTQ_SIZE {
                VIRTQ_SIZE
            } else {
                max_size
            };

            let vq = Virtqueue::alloc(qsize)?;

            mmio.write32(virtio_regs::QUEUE_SIZE, qsize as u32);
            mmio.write32(virtio_regs::QUEUE_ADDRESS, vq.page_number());

            Some(vq)
        }
    }

    fn send_ctrl_cmd<Req: Copy, Resp: Copy + Default>(&mut self, req: &Req) -> Option<Resp> {
        let req_size = core::mem::size_of::<Req>();
        let resp_size = core::mem::size_of::<Resp>();

        unsafe {
            ptr::copy_nonoverlapping(req as *const Req as *const u8, self.req_buf_virt, req_size);
            ptr::write_bytes(self.resp_buf_virt, 0, resp_size);

            let bufs = [
                (self.req_buf_phys, req_size as u32, 0u16),
                (self.resp_buf_phys, resp_size as u32, VIRTQ_DESC_F_WRITE),
            ];

            self.controlq.push_request(&bufs)?;

            self.mmio.write32(virtio_regs::QUEUE_NOTIFY, 0);

            for _ in 0..1_000_000 {
                if self.controlq.poll_used().is_some() {
                    let resp = ptr::read_volatile(self.resp_buf_virt as *const Resp);
                    return Some(resp);
                }
                core::hint::spin_loop();
            }
        }
        None
    }

    fn create_resource_2d(&mut self, width: u32, height: u32) -> Option<u32> {
        let resource_id = self.next_resource_id;
        self.next_resource_id += 1;

        let req = VirtioGpuResourceCreate2d {
            hdr: VirtioGpuCtrlHdr {
                hdr_type: virtio_gpu_cmd::RESOURCE_CREATE_2D,
                ..Default::default()
            },
            resource_id,
            format: virtio_gpu_cmd::FORMAT_B8G8R8A8_UNORM,
            width,
            height,
        };

        let resp: VirtioGpuCtrlHdr = self.send_ctrl_cmd(&req)?;
        if resp.hdr_type == virtio_gpu_cmd::RESP_OK_NODATA {
            Some(resource_id)
        } else {
            crate::serial_println!(
                "[gpu] resource_create_2d failed: resp type {:#x}",
                resp.hdr_type
            );
            None
        }
    }

    fn attach_backing(&mut self, resource_id: u32, phys: u64, size: u32) -> bool {
        #[repr(C)]
        #[derive(Clone, Copy)]
        struct AttachBacking {
            hdr: VirtioGpuCtrlHdr,
            resource_id: u32,
            nr_entries: u32,
            entry: VirtioGpuMemEntry,
        }

        let req = AttachBacking {
            hdr: VirtioGpuCtrlHdr {
                hdr_type: virtio_gpu_cmd::RESOURCE_ATTACH_BACKING,
                ..Default::default()
            },
            resource_id,
            nr_entries: 1,
            entry: VirtioGpuMemEntry {
                addr: phys,
                length: size,
                _padding: 0,
            },
        };

        let resp: Option<VirtioGpuCtrlHdr> = self.send_ctrl_cmd(&req);
        match resp {
            Some(r) if r.hdr_type == virtio_gpu_cmd::RESP_OK_NODATA => true,
            _ => {
                crate::serial_println!("[gpu] attach_backing failed for resource {}", resource_id);
                false
            }
        }
    }

    fn set_scanout_cmd(
        &mut self,
        scanout_id: u32,
        resource_id: u32,
        width: u32,
        height: u32,
    ) -> bool {
        let req = VirtioGpuSetScanout {
            hdr: VirtioGpuCtrlHdr {
                hdr_type: virtio_gpu_cmd::SET_SCANOUT,
                ..Default::default()
            },
            x: 0,
            y: 0,
            width,
            height,
            scanout_id,
            resource_id,
        };

        let resp: Option<VirtioGpuCtrlHdr> = self.send_ctrl_cmd(&req);
        match resp {
            Some(r) if r.hdr_type == virtio_gpu_cmd::RESP_OK_NODATA => true,
            _ => {
                crate::serial_println!("[gpu] set_scanout failed");
                false
            }
        }
    }

    fn transfer_to_host_2d(&mut self, resource_id: u32, width: u32, height: u32) -> bool {
        let req = VirtioGpuTransferToHost2d {
            hdr: VirtioGpuCtrlHdr {
                hdr_type: virtio_gpu_cmd::TRANSFER_TO_HOST_2D,
                ..Default::default()
            },
            x: 0,
            y: 0,
            width,
            height,
            offset: 0,
            resource_id,
            _padding: 0,
        };

        let resp: Option<VirtioGpuCtrlHdr> = self.send_ctrl_cmd(&req);
        match resp {
            Some(r) if r.hdr_type == virtio_gpu_cmd::RESP_OK_NODATA => true,
            _ => false,
        }
    }

    fn resource_flush(&mut self, resource_id: u32, width: u32, height: u32) -> bool {
        let req = VirtioGpuResourceFlush {
            hdr: VirtioGpuCtrlHdr {
                hdr_type: virtio_gpu_cmd::RESOURCE_FLUSH,
                ..Default::default()
            },
            x: 0,
            y: 0,
            width,
            height,
            resource_id,
            _padding: 0,
        };

        let resp: Option<VirtioGpuCtrlHdr> = self.send_ctrl_cmd(&req);
        match resp {
            Some(r) if r.hdr_type == virtio_gpu_cmd::RESP_OK_NODATA => true,
            _ => false,
        }
    }

    /// Returns the physical address of the lockless command ring (GpuRingControl)
    /// to be mapped into the user-space RaeGFX process.
    pub fn get_command_ring_phys(&self) -> Option<u64> {
        self.cmd_ring.as_ref().map(|ring| ring.base_phys)
    }

    /// Returns the physical address of the MMIO doorbell register
    /// to be memory-mapped into user-space for zero-syscall command submission.
    pub fn get_doorbell_phys(&self) -> Option<u64> {
        self.cmd_ring.as_ref().map(|ring| {
            // Reconstruct the physical MMIO doorbell address from the VirtIO MMIO base
            // The GpuCommandRing doorbell_offset is relative to the MMIO BAR0 base.
            self.device.mmio_base + (ring.doorbell_offset as u64)
        })
    }

    pub fn create_scanout(
        &mut self,
        width: u32,
        height: u32,
        format: PixelFormat,
    ) -> Option<Scanout> {
        let stride = width * 4;
        let fb_size = (stride * height) as u64;
        let pages = ((fb_size + 4095) / 4096) as usize;
        if pages == 0 {
            return None;
        }

        // The scanout framebuffer MUST be physically contiguous: attach_backing
        // hands the device a single (phys, fb_size) backing region, and the
        // zero-fill below clears `pages * 4096` contiguous bytes. Allocating
        // `pages` frames one-at-a-time (the old code) does NOT guarantee
        // contiguity — it kept only the FIRST frame's address yet zeroed (and
        // backed) `pages` frames of whatever physical memory happened to follow
        // it, trampling unrelated frames. That was the deterministic multi-vCPU
        // boot heap corruption: layout-sensitive (the trampled ~4 MiB span
        // depends on the QEMU memory map, so it smashed the search-index BTree
        // under -smp 2 but landed harmlessly under -smp 1) and a synchronous
        // wild write the boundary heap-canary could never catch. Allocate one
        // power-of-two contiguous block from the buddy instead.
        let order = pages.next_power_of_two().trailing_zeros() as u8;
        let phys_addr = crate::memory::allocate_contiguous_frames(order)?;
        let phys = phys_addr.as_u64();
        let offset = PHYS_MEM_OFFSET.get()?.as_u64();
        let virt = (offset + phys) as *mut u8;

        unsafe {
            ptr::write_bytes(virt, 0, pages * 4096);
        }

        if let Some(dom_id) = self.iommu_domain {
            crate::iommu::map_dma(dom_id, phys, phys, (pages * 4096) as u64, true, true);
        }

        let resource_id = match self.create_resource_2d(width, height) {
            Some(r) => r,
            None => {
                crate::memory::deallocate_contiguous_frames(phys_addr, order);
                return None;
            }
        };

        if !self.attach_backing(resource_id, phys, fb_size as u32) {
            crate::memory::deallocate_contiguous_frames(phys_addr, order);
            return None;
        }

        if !self.set_scanout_cmd(0, resource_id, width, height) {
            crate::memory::deallocate_contiguous_frames(phys_addr, order);
            return None;
        }

        let scanout = Scanout {
            id: 0,
            resource_id,
            fb_vram_offset: 0,
            fb_phys: phys,
            fb_virt: virt,
            width,
            height,
            stride,
            format,
        };

        crate::serial_println!(
            "[gpu] Scanout created: {}x{} resource={} phys={:#x}",
            width,
            height,
            resource_id,
            phys
        );

        Some(scanout)
    }

    pub fn present_scanout(&mut self, scanout: &Scanout) {
        self.transfer_to_host_2d(scanout.resource_id, scanout.width, scanout.height);
        self.resource_flush(scanout.resource_id, scanout.width, scanout.height);
    }
}

impl GpuDriver for VirtioGpuDriver {
    fn info(&self) -> &GpuInfo {
        &self.gpu_info
    }

    fn capabilities(&self) -> GpuCapabilities {
        GpuCapabilities {
            vulkan: false,
            compute: false,
            ray_tracing: false,
            vrr: false,
            hdr: false,
            hw_video_decode: false,
        }
    }

    fn submit(&mut self, cmd: &GpuCommand) -> Result<(), &'static str> {
        match cmd {
            GpuCommand::SetMode(mode) => {
                self.mode = *mode;
                Ok(())
            }
            GpuCommand::Present { .. } => {
                if let Some(ref scanout) = self.scanout.clone() {
                    self.present_scanout(scanout);
                }
                Ok(())
            }
            GpuCommand::Vsync => Ok(()),
            GpuCommand::FenceWait(_) => Ok(()),
            GpuCommand::FenceSignal(_) => Ok(()),
        }
    }

    fn current_mode(&self) -> DisplayMode {
        self.mode
    }

    fn supported_modes(&self) -> Vec<DisplayMode> {
        vec![
            DisplayMode {
                width: 640,
                height: 480,
                refresh_hz: 60,
                bpp: 32,
                hdr: false,
            },
            DisplayMode {
                width: 800,
                height: 600,
                refresh_hz: 60,
                bpp: 32,
                hdr: false,
            },
            DisplayMode {
                width: 1024,
                height: 768,
                refresh_hz: 60,
                bpp: 32,
                hdr: false,
            },
            DisplayMode {
                width: 1920,
                height: 1080,
                refresh_hz: 60,
                bpp: 32,
                hdr: false,
            },
        ]
    }
}

// ─── Fallback Stub Driver ───────────────────────────────────────────────────

struct FallbackGpu {
    gpu_info: GpuInfo,
    mode: DisplayMode,
}

impl FallbackGpu {
    fn new() -> Self {
        Self {
            gpu_info: GpuInfo {
                vendor: GpuVendor::Unknown(0),
                device_id: 0,
                vram_bytes: 0,
                name: "Software Fallback",
                pci_bus: 0,
                pci_slot: 0,
            },
            mode: DisplayMode {
                width: 1024,
                height: 768,
                refresh_hz: 60,
                bpp: 32,
                hdr: false,
            },
        }
    }
}

impl GpuDriver for FallbackGpu {
    fn info(&self) -> &GpuInfo {
        &self.gpu_info
    }

    fn capabilities(&self) -> GpuCapabilities {
        GpuCapabilities {
            vulkan: false,
            compute: false,
            ray_tracing: false,
            vrr: false,
            hdr: false,
            hw_video_decode: false,
        }
    }

    fn submit(&mut self, cmd: &GpuCommand) -> Result<(), &'static str> {
        match cmd {
            GpuCommand::SetMode(mode) => {
                self.mode = *mode;
                Ok(())
            }
            _ => Ok(()),
        }
    }

    fn current_mode(&self) -> DisplayMode {
        self.mode
    }

    fn supported_modes(&self) -> Vec<DisplayMode> {
        vec![
            DisplayMode {
                width: 640,
                height: 480,
                refresh_hz: 60,
                bpp: 32,
                hdr: false,
            },
            DisplayMode {
                width: 800,
                height: 600,
                refresh_hz: 60,
                bpp: 32,
                hdr: false,
            },
            DisplayMode {
                width: 1024,
                height: 768,
                refresh_hz: 60,
                bpp: 32,
                hdr: false,
            },
            DisplayMode {
                width: 1920,
                height: 1080,
                refresh_hz: 60,
                bpp: 32,
                hdr: false,
            },
        ]
    }
}

// ─── Bochs VBE / QEMU stdvga Driver ────────────────────────────────────────

const VBE_DISPI_IOPORT_INDEX: u16 = 0x01CE;
const VBE_DISPI_IOPORT_DATA: u16 = 0x01CF;
const VBE_DISPI_INDEX_ID: u16 = 0;
const VBE_DISPI_INDEX_XRES: u16 = 1;
const VBE_DISPI_INDEX_YRES: u16 = 2;
const VBE_DISPI_INDEX_BPP: u16 = 3;
const VBE_DISPI_INDEX_ENABLE: u16 = 4;
const VBE_DISPI_INDEX_VIRT_WIDTH: u16 = 6;
const VBE_DISPI_INDEX_VIRT_HEIGHT: u16 = 7;
const VBE_DISPI_INDEX_X_OFFSET: u16 = 8;
const VBE_DISPI_INDEX_Y_OFFSET: u16 = 9;
const VBE_DISPI_ENABLED: u16 = 0x01;
const VBE_DISPI_LFB_ENABLED: u16 = 0x40;
const BOCHS_VENDOR: u16 = 0x1234;
const BOCHS_DEVICE_STDVGA: u16 = 0x1111;

fn vbe_write(index: u16, value: u16) {
    unsafe {
        x86_64::instructions::port::Port::new(VBE_DISPI_IOPORT_INDEX).write(index);
        x86_64::instructions::port::Port::new(VBE_DISPI_IOPORT_DATA).write(value);
    }
}

fn vbe_read(index: u16) -> u16 {
    unsafe {
        x86_64::instructions::port::Port::new(VBE_DISPI_IOPORT_INDEX).write(index);
        x86_64::instructions::port::Port::<u16>::new(VBE_DISPI_IOPORT_DATA).read()
    }
}

struct BochsVbeDriver {
    gpu_info: GpuInfo,
    mode: DisplayMode,
    fb_phys: u64,
}

impl BochsVbeDriver {
    fn new(pci_dev: &PciDevice) -> Option<Self> {
        let id = vbe_read(VBE_DISPI_INDEX_ID);
        if id < 0xB0C0 || id > 0xB0CF {
            crate::serial_println!("[gpu] Bochs VBE not detected (id={:#06x})", id);
            return None;
        }
        crate::serial_println!("[gpu] Bochs VBE version {:#06x}", id);

        let fb_bar_raw = pci_dev.bars[0];
        let fb_phys = if fb_bar_raw & 1 == 0 {
            // 64-bit memory BAR (type bits 2:1 == 0b10): upper half in BAR1.
            let mut phys = (fb_bar_raw as u64) & !0x0F;
            if (fb_bar_raw >> 1) & 0x3 == 2 {
                phys |= (pci_dev.bars[1] as u64) << 32;
            }
            phys
        } else {
            return None;
        };
        if fb_phys == 0 {
            return None;
        }

        Some(Self {
            gpu_info: GpuInfo {
                vendor: GpuVendor::Unknown(BOCHS_VENDOR),
                device_id: BOCHS_DEVICE_STDVGA,
                vram_bytes: 16 * 1024 * 1024,
                name: "QEMU stdvga (Bochs VBE)",
                pci_bus: pci_dev.bus,
                pci_slot: pci_dev.device,
            },
            mode: DisplayMode {
                width: 1280,
                height: 720,
                refresh_hz: 60,
                bpp: 32,
                hdr: false,
            },
            fb_phys,
        })
    }

    fn set_mode_hw(&self, width: u16, height: u16, bpp: u16) {
        vbe_write(VBE_DISPI_INDEX_ENABLE, 0);
        vbe_write(VBE_DISPI_INDEX_XRES, width);
        vbe_write(VBE_DISPI_INDEX_YRES, height);
        vbe_write(VBE_DISPI_INDEX_BPP, bpp);
        vbe_write(VBE_DISPI_INDEX_VIRT_WIDTH, width);
        vbe_write(VBE_DISPI_INDEX_VIRT_HEIGHT, height);
        vbe_write(VBE_DISPI_INDEX_X_OFFSET, 0);
        vbe_write(VBE_DISPI_INDEX_Y_OFFSET, 0);
        vbe_write(
            VBE_DISPI_INDEX_ENABLE,
            VBE_DISPI_ENABLED | VBE_DISPI_LFB_ENABLED,
        );
    }

    fn create_scanout(&self, width: u32, height: u32) -> Option<Scanout> {
        let stride = width;
        let phys_off = crate::memory::PHYS_MEM_OFFSET.get()?;
        let fb_virt = (phys_off.as_u64() + self.fb_phys) as *mut u8;

        Some(Scanout {
            id: 0,
            resource_id: 0,
            fb_vram_offset: 0,
            fb_phys: self.fb_phys,
            fb_virt,
            width,
            height,
            stride,
            format: PixelFormat::Bgra8,
        })
    }
}

impl GpuDriver for BochsVbeDriver {
    fn info(&self) -> &GpuInfo {
        &self.gpu_info
    }

    fn capabilities(&self) -> GpuCapabilities {
        GpuCapabilities {
            vulkan: false,
            compute: false,
            ray_tracing: false,
            vrr: false,
            hdr: false,
            hw_video_decode: false,
        }
    }

    fn submit(&mut self, cmd: &GpuCommand) -> Result<(), &'static str> {
        match cmd {
            GpuCommand::SetMode(mode) => {
                self.set_mode_hw(mode.width as u16, mode.height as u16, mode.bpp as u16);
                self.mode = *mode;
                Ok(())
            }
            GpuCommand::Present { .. } | GpuCommand::Vsync => Ok(()),
            GpuCommand::FenceWait(_) | GpuCommand::FenceSignal(_) => Ok(()),
        }
    }

    fn current_mode(&self) -> DisplayMode {
        self.mode
    }

    fn supported_modes(&self) -> Vec<DisplayMode> {
        alloc::vec![
            DisplayMode {
                width: 640,
                height: 480,
                refresh_hz: 60,
                bpp: 32,
                hdr: false
            },
            DisplayMode {
                width: 800,
                height: 600,
                refresh_hz: 60,
                bpp: 32,
                hdr: false
            },
            DisplayMode {
                width: 1024,
                height: 768,
                refresh_hz: 60,
                bpp: 32,
                hdr: false
            },
            DisplayMode {
                width: 1280,
                height: 720,
                refresh_hz: 60,
                bpp: 32,
                hdr: false
            },
            DisplayMode {
                width: 1920,
                height: 1080,
                refresh_hz: 60,
                bpp: 32,
                hdr: false
            },
        ]
    }
}

// ─── PCI GPU Discovery ─────────────────────────────────────────────────────

const PCI_CLASS_DISPLAY: u8 = 0x03;
const PCI_SUBCLASS_VGA: u8 = 0x00;
const PCI_SUBCLASS_3D: u8 = 0x02;

const VENDOR_INTEL: u16 = 0x8086;
const VENDOR_AMD: u16 = 0x1002;
const VENDOR_NVIDIA: u16 = 0x10DE;
const VENDOR_VIRTIO: u16 = 0x1AF4;
const DEVICE_VIRTIO_GPU: u16 = 0x1050;

fn classify_gpu(pci_dev: &PciDevice) -> GpuType {
    match pci_dev.vendor_id {
        VENDOR_VIRTIO if pci_dev.device_id == DEVICE_VIRTIO_GPU => GpuType::VirtioGpu,
        BOCHS_VENDOR if pci_dev.device_id == BOCHS_DEVICE_STDVGA => GpuType::BochsVbe,
        VENDOR_INTEL => GpuType::IntelIntegrated,
        VENDOR_AMD => GpuType::AmdDiscrete,
        VENDOR_NVIDIA => GpuType::NvidiaDiscrete,
        _ => GpuType::Unknown,
    }
}

fn is_display_controller(pci_dev: &PciDevice) -> bool {
    if pci_dev.class == PCI_CLASS_DISPLAY
        && (pci_dev.subclass == PCI_SUBCLASS_VGA || pci_dev.subclass == PCI_SUBCLASS_3D)
    {
        return true;
    }
    if pci_dev.vendor_id == VENDOR_VIRTIO && pci_dev.device_id == DEVICE_VIRTIO_GPU {
        return true;
    }
    false
}

fn discover_gpus() -> Vec<(PciDevice, GpuType)> {
    let all_devices = pci::enumerate();
    let mut gpus = Vec::new();

    for dev in all_devices {
        if is_display_controller(&dev) {
            let gpu_type = classify_gpu(&dev);
            crate::serial_println!(
                "[gpu] Found {:?} GPU: vendor={:#06x} device={:#06x} at {:02x}:{:02x}.{}",
                gpu_type,
                dev.vendor_id,
                dev.device_id,
                dev.bus,
                dev.device,
                dev.function
            );
            gpus.push((dev, gpu_type));
        }
    }

    gpus.sort_by_key(|&(_, ref t)| match t {
        GpuType::VirtioGpu => 0,
        GpuType::BochsVbe => 1,
        GpuType::IntelIntegrated => 2,
        GpuType::AmdDiscrete => 3,
        GpuType::NvidiaDiscrete => 4,
        GpuType::Unknown => 5,
    });

    gpus
}

fn enable_bus_master(pci_dev: &PciDevice) {
    let cmd = pci::read_config_16(pci_dev.bus, pci_dev.device, pci_dev.function, 0x04);
    pci::write_config_16(
        pci_dev.bus,
        pci_dev.device,
        pci_dev.function,
        0x04,
        cmd | (1 << 2) | (1 << 1),
    );
}

fn probe_real_gpu(pci_dev: &PciDevice, gpu_type: GpuType) {
    // CONFIG-SPACE ONLY — never map or read a real GPU's MMIO here. The old
    // "identity peek" (map BAR0, read32(0)/read32(4)) HARD-HUNG Athena's boot
    // inside gpu::init: on AMD APUs (Radeon 760M, 1002:15bf) an aperture read
    // with the device uninitialized (no PSP/SMU bring-up, power state at
    // firmware handoff) stalls the data fabric — no page fault, no #DF, the
    // core just never gets the load back. Both Athena BOOTLOG captures ended
    // at the checkpoint immediately before gpu::init for exactly this reason.
    // QEMU never exercises this path (Bochs/VirtIO take their own arms), so
    // it only ever detonated on bare metal. Real bring-up of AMD/Intel/NVIDIA
    // silicon belongs to the userspace LinuxKPI drivers (amdgpud — Phase 6),
    // which driver_manifest already matched for this device.
    let mmio_base = pci::bar_address(pci_dev, 0);
    let vram_bar = pci::bar_address(pci_dev, 2);

    crate::serial_println!(
        "[gpu] {:?}: BAR0(MMIO)={:#x?} BAR2(VRAM)={:#x?} — left untouched (LinuxKPI driver's job)",
        gpu_type,
        mmio_base,
        vram_bar,
    );
}

// ─── Global State ───────────────────────────────────────────────────────────

pub static GPU: Mutex<Option<Box<dyn GpuDriver + Send>>> = Mutex::new(None);
pub static GPU_SCANOUT: Mutex<Option<Scanout>> = Mutex::new(None);
static GPU_HW_PRESENT: spin::Once<bool> = spin::Once::new();

pub fn has_hw_gpu() -> bool {
    *GPU_HW_PRESENT.get().unwrap_or(&false)
}

/// Get a raw pointer + geometry for the GPU scanout framebuffer.
/// Returns None if no GPU scanout is active.
pub fn gpu_framebuffer() -> Option<GpuFramebuffer> {
    let guard = GPU_SCANOUT.lock();
    guard.as_ref().map(|s| GpuFramebuffer {
        ptr: s.fb_virt,
        width: s.width,
        height: s.height,
        stride: s.stride,
        bytes_per_pixel: 4,
    })
}

#[derive(Clone, Copy)]
pub struct GpuFramebuffer {
    pub ptr: *mut u8,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub bytes_per_pixel: u32,
}

// SAFETY: the GPU framebuffer memory is kernel-owned and only accessed
// behind the GPU_SCANOUT mutex.
unsafe impl Send for GpuFramebuffer {}
unsafe impl Sync for GpuFramebuffer {}

/// Flush the GPU scanout to the display (VirtIO transfer+flush).
pub fn present_gpu_scanout() {
    let mut gpu_guard = GPU.lock();
    if let Some(ref mut driver) = *gpu_guard {
        let _ = driver.submit(&GpuCommand::Present {
            fb_addr: 0,
            stride: 0,
        });
    }
    // No in-kernel driver bound (e.g. amdgpu DCN, owned by the userspace daemon):
    // the display engine scans the registered buffer CONTINUOUSLY, so there is no
    // per-frame flush to issue here — the compositor's blit into `fb_virt` is the
    // present. (Coherency follow-up: see `register_external_scanout`.)
}

/// Register an externally-prepared scanout buffer (a userspace KMS driver's
/// framebuffer, e.g. amdgpu's DCN) so the in-kernel compositor presents THROUGH
/// it. `phys` is the contiguous physical base of a framebuffer the device's
/// display engine is already pointed at (amdgpud GART-maps it + points the DCN
/// HUBP at the matching GPU VA); the kernel direct-maps the SAME physical pages
/// (`PHYS_MEM_OFFSET + phys`) as `fb_virt`, so the compositor blits each frame at
/// full cached speed and the display engine scans the identical bytes out — no
/// per-frame copy. Returns true iff the scanout was attached to the compositor.
///
/// Phase 6 (RaeGFX): the seam from the userspace KMS path (amdgpud →
/// `raeen_drm::kms::atomic_commit` → `SYS_LINUXKPI_REGISTER_SCANOUT`) to the
/// compositor's `ScanoutBackend::GpuFb`. SAFE FALLBACK: invalid geometry or no
/// physical-memory direct map returns false WITHOUT touching `GPU_SCANOUT`, so a
/// malformed driver request can never disturb the working GOP/virtio desktop.
///
/// COHERENCY (the "fix after" the owner flagged): `fb_virt` is the WRITE-BACK
/// cached kernel direct map. On an APU whose DCN reads framebuffer memory without
/// snooping the CPU cache, the compositor's cached writes must reach DRAM before
/// the DCN reads them — so a full-rate desktop may need this buffer mapped
/// write-combining, or a cache flush per frame. Wired WB first (the simplest
/// honest path); the flush/WC upgrade is the follow-up if iron shows tearing or
/// stale pixels.
pub fn register_external_scanout(phys: u64, width: u32, height: u32, stride: u32) -> bool {
    if width == 0 || height == 0 || width > 8192 || height > 8192 {
        return false;
    }
    if (stride as u64) < (width as u64).saturating_mul(4) {
        return false;
    }
    if phys == 0 || (phys & 0xFFF) != 0 {
        return false;
    }
    let Some(offset) = PHYS_MEM_OFFSET.get() else {
        return false;
    };
    // Usable-RAM scanouts (Bochs/virtio) are already in the cached physmap. An APU
    // DCN scanout buffer lives in the FIRMWARE-RESERVED VRAM carveout, which is ABOVE
    // the usable-RAM physmap (max_phys is computed from Usable regions only), so
    // `offset + phys` there points at UNMAPPED memory. Map that carveout range
    // WRITE-BACK cached so the compositor's full-frame blit runs at cache speed (an
    // UC mapping is ~1 uncached store/pixel = ~200ms/frame at 1080p, which pegs CPU0);
    // the compositor `clflush`es the range after each present so the non-snooped DCN
    // read path sees the pixels.
    if !crate::memory::phys_is_usable_ram(phys) {
        let len = (height as usize).saturating_mul(stride as usize);
        crate::memory::map_phys_wb(phys, len);
    }
    let fb_virt = offset.as_u64().wrapping_add(phys) as *mut u8;

    // The syscall contract passes stride in BYTES (validated >= width*4 above), but
    // `Scanout.stride` — like the virtio path (`stride = width`) — is consumed by the
    // compositor in PIXELS (`fb_ptr.add(y * stride + x)` on a *mut u32). Storing bytes
    // here spaced every desktop row 4x apart on the panel (the "garbage screen" bug).
    let stride_px = stride / 4;

    *GPU_SCANOUT.lock() = Some(Scanout {
        id: 0,
        resource_id: 0,
        fb_vram_offset: 0,
        fb_phys: phys,
        fb_virt,
        width,
        height,
        stride: stride_px,
        format: PixelFormat::Bgra8,
    });
    crate::serial_println!(
        "[gpu] external scanout registered (userspace KMS / amdgpu DCN): {}x{} stride={} phys={:#x} -> compositor",
        width,
        height,
        stride,
        phys
    );
    crate::compositor::attach_gpu_scanout();
    // The real-GPU present pipeline targets 120 fps (docs/PERFORMANCE_TARGETS.md):
    // the row-blit present sustains it, and the sub-frame input contract wants the
    // shorter interval. Then bench 24 flat-out frames through the freshly-attached
    // scanout so every iron boot self-reports the achieved rate on THIS hardware.
    crate::compositor::set_target_frame_us(8_333);
    crate::compositor::run_present_bench("gpu-scanout", 24);
    true
}

// ─── Initialization ─────────────────────────────────────────────────────────

/// Program a display mode on the active GPU driver (Bochs VBE / VirtIO-GPU).
pub fn request_display_mode(width: u32, height: u32, bpp: u8) -> bool {
    if !has_hw_gpu() {
        return false;
    }
    let mut guard = GPU.lock();
    let Some(ref mut driver) = *guard else {
        return false;
    };
    let mode = DisplayMode {
        width,
        height,
        refresh_hz: 60,
        bpp,
        hdr: false,
    };
    driver.submit(&GpuCommand::SetMode(mode)).is_ok()
}

/// MasterChecklist Phase 2.5 — runtime mode-set path (hardware when present).
pub fn run_boot_smoketest() {
    let (phys_w, phys_h) = crate::framebuffer::physical_dimensions().unwrap_or((1280, 800));
    let target_w = 1024u32.min(phys_w);
    let target_h = 768u32.min(phys_h);
    if has_hw_gpu() && request_display_mode(target_w, target_h, 32) {
        crate::serial_println!(
            "[gpu] smoketest: SetMode {}x{} OK (hw driver)",
            target_w,
            target_h
        );
    } else {
        let ok = crate::framebuffer::set_mode(target_w, target_h);
        crate::serial_println!(
            "[gpu] smoketest: logical mode_set {}x{} -> {}",
            target_w,
            target_h,
            ok
        );
    }
}

pub fn init() {
    crate::serial_println!("[gpu] Scanning PCI for display controllers...");

    let gpus = discover_gpus();
    if gpus.is_empty() {
        crate::serial_println!("[gpu] No GPU found — using software fallback");
        GPU_HW_PRESENT.call_once(|| false);
        *GPU.lock() = Some(Box::new(FallbackGpu::new()));
        return;
    }

    let mut initialized = false;

    for (pci_dev, gpu_type) in &gpus {
        match gpu_type {
            GpuType::VirtioGpu => {
                crate::serial_println!("[gpu] Initializing VirtIO-GPU...");
                if let Some(mut driver) = VirtioGpuDriver::new(pci_dev) {
                    let mode = driver.mode;
                    match driver.create_scanout(mode.width, mode.height, PixelFormat::Bgra8) {
                        Some(scanout) => {
                            crate::serial_println!(
                                "[gpu] VirtIO-GPU scanout ready: {}x{}",
                                scanout.width,
                                scanout.height
                            );
                            *GPU_SCANOUT.lock() = Some(scanout.clone());
                            driver.scanout = Some(scanout);
                            GPU_HW_PRESENT.call_once(|| true);
                            *GPU.lock() = Some(Box::new(driver));
                            initialized = true;
                        }
                        None => {
                            crate::serial_println!(
                                "[gpu] VirtIO-GPU scanout creation failed, driver still usable"
                            );
                            GPU_HW_PRESENT.call_once(|| true);
                            *GPU.lock() = Some(Box::new(driver));
                            initialized = true;
                        }
                    }
                    break;
                }
            }
            GpuType::BochsVbe => {
                crate::serial_println!("[gpu] Initializing Bochs VBE (QEMU stdvga)...");
                enable_bus_master(pci_dev);
                if let Some(mut driver) = BochsVbeDriver::new(pci_dev) {
                    let w = driver.mode.width as u16;
                    let h = driver.mode.height as u16;
                    driver.set_mode_hw(w, h, 32);
                    if let Some(scanout) = driver.create_scanout(w as u32, h as u32) {
                        crate::serial_println!(
                            "[gpu] Bochs VBE scanout ready: {}x{} LFB={:#010x}",
                            w,
                            h,
                            driver.fb_phys
                        );
                        *GPU_SCANOUT.lock() = Some(scanout.clone());
                    }
                    GPU_HW_PRESENT.call_once(|| true);
                    *GPU.lock() = Some(Box::new(driver));
                    initialized = true;
                    break;
                }
            }
            other => {
                probe_real_gpu(pci_dev, *other);
            }
        }
    }

    if !initialized {
        crate::serial_println!("[gpu] No usable GPU driver — software fallback");
        GPU_HW_PRESENT.call_once(|| false);
        *GPU.lock() = Some(Box::new(FallbackGpu::new()));
    }
}
