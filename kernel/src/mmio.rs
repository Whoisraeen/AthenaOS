#![allow(dead_code)]

extern crate alloc;

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use spin::Mutex;

// ---------------------------------------------------------------------------
// Memory barriers
// ---------------------------------------------------------------------------

#[inline(always)]
pub fn rmb() {
    core::sync::atomic::fence(Ordering::Acquire);
}

#[inline(always)]
pub fn wmb() {
    core::sync::atomic::fence(Ordering::Release);
}

#[inline(always)]
pub fn mb() {
    core::sync::atomic::fence(Ordering::SeqCst);
}

#[inline(always)]
pub fn smp_rmb() {
    core::sync::atomic::compiler_fence(Ordering::Acquire);
}

#[inline(always)]
pub fn smp_wmb() {
    core::sync::atomic::compiler_fence(Ordering::Release);
}

#[inline(always)]
pub fn smp_mb() {
    core::sync::atomic::compiler_fence(Ordering::SeqCst);
}

#[inline(always)]
pub fn io_rmb() {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        core::arch::asm!("lfence", options(nostack, preserves_flags));
    }
    #[cfg(not(target_arch = "x86_64"))]
    rmb();
}

#[inline(always)]
pub fn io_wmb() {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        core::arch::asm!("sfence", options(nostack, preserves_flags));
    }
    #[cfg(not(target_arch = "x86_64"))]
    wmb();
}

// ---------------------------------------------------------------------------
// x86 port I/O
// ---------------------------------------------------------------------------

#[inline(always)]
pub unsafe fn inb(port: u16) -> u8 {
    let val: u8;
    core::arch::asm!("in al, dx", out("al") val, in("dx") port, options(nostack, preserves_flags));
    val
}

#[inline(always)]
pub unsafe fn inw(port: u16) -> u16 {
    let val: u16;
    core::arch::asm!("in ax, dx", out("ax") val, in("dx") port, options(nostack, preserves_flags));
    val
}

#[inline(always)]
pub unsafe fn inl(port: u16) -> u32 {
    let val: u32;
    core::arch::asm!("in eax, dx", out("eax") val, in("dx") port, options(nostack, preserves_flags));
    val
}

#[inline(always)]
pub unsafe fn outb(port: u16, val: u8) {
    core::arch::asm!("out dx, al", in("dx") port, in("al") val, options(nostack, preserves_flags));
}

#[inline(always)]
pub unsafe fn outw(port: u16, val: u16) {
    core::arch::asm!("out dx, ax", in("dx") port, in("ax") val, options(nostack, preserves_flags));
}

#[inline(always)]
pub unsafe fn outl(port: u16, val: u32) {
    core::arch::asm!("out dx, eax", in("dx") port, in("eax") val, options(nostack, preserves_flags));
}

// ---------------------------------------------------------------------------
// IoMem — typed volatile MMIO accessor
// ---------------------------------------------------------------------------

pub struct IoMem {
    base: usize,
    size: usize,
}

impl IoMem {
    pub fn new(base: usize, size: usize) -> Self {
        Self { base, size }
    }

    fn check_offset(&self, offset: usize, width: usize) {
        assert!(
            offset + width <= self.size,
            "MMIO access at offset {:#x} width {} exceeds region size {:#x}",
            offset,
            width,
            self.size
        );
    }

    pub fn read8(&self, offset: usize) -> u8 {
        self.check_offset(offset, 1);
        let addr = (self.base + offset) as *const u8;
        let val = unsafe { core::ptr::read_volatile(addr) };
        io_rmb();
        MMIO_TRACER.trace_read(self.base + offset, val as u64, 1);
        val
    }

    pub fn read16(&self, offset: usize) -> u16 {
        self.check_offset(offset, 2);
        let addr = (self.base + offset) as *const u16;
        let val = unsafe { core::ptr::read_volatile(addr) };
        io_rmb();
        MMIO_TRACER.trace_read(self.base + offset, val as u64, 2);
        val
    }

    pub fn read32(&self, offset: usize) -> u32 {
        self.check_offset(offset, 4);
        let addr = (self.base + offset) as *const u32;
        let val = unsafe { core::ptr::read_volatile(addr) };
        io_rmb();
        MMIO_TRACER.trace_read(self.base + offset, val as u64, 4);
        val
    }

    pub fn read64(&self, offset: usize) -> u64 {
        self.check_offset(offset, 8);
        let addr = (self.base + offset) as *const u64;
        let val = unsafe { core::ptr::read_volatile(addr) };
        io_rmb();
        MMIO_TRACER.trace_read(self.base + offset, val, 8);
        val
    }

    pub fn write8(&self, offset: usize, val: u8) {
        self.check_offset(offset, 1);
        MMIO_TRACER.trace_write(self.base + offset, val as u64, 1);
        io_wmb();
        let addr = (self.base + offset) as *mut u8;
        unsafe { core::ptr::write_volatile(addr, val) };
    }

    pub fn write16(&self, offset: usize, val: u16) {
        self.check_offset(offset, 2);
        MMIO_TRACER.trace_write(self.base + offset, val as u64, 2);
        io_wmb();
        let addr = (self.base + offset) as *mut u16;
        unsafe { core::ptr::write_volatile(addr, val) };
    }

    pub fn write32(&self, offset: usize, val: u32) {
        self.check_offset(offset, 4);
        MMIO_TRACER.trace_write(self.base + offset, val as u64, 4);
        io_wmb();
        let addr = (self.base + offset) as *mut u32;
        unsafe { core::ptr::write_volatile(addr, val) };
    }

    pub fn write64(&self, offset: usize, val: u64) {
        self.check_offset(offset, 8);
        MMIO_TRACER.trace_write(self.base + offset, val, 8);
        io_wmb();
        let addr = (self.base + offset) as *mut u64;
        unsafe { core::ptr::write_volatile(addr, val) };
    }

    pub fn base(&self) -> usize {
        self.base
    }

    pub fn size(&self) -> usize {
        self.size
    }
}

// ---------------------------------------------------------------------------
// MMIO region management
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct MmioRegion {
    pub phys_addr: u64,
    pub virt_addr: usize,
    pub size: usize,
    pub name: String,
    pub flags: MmioFlags,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MmioFlags(pub u32);

impl MmioFlags {
    pub const NONE: Self = Self(0);
    pub const NOCACHE: Self = Self(1 << 0);
    pub const WRITE_COMBINE: Self = Self(1 << 1);
    pub const WRITE_THROUGH: Self = Self(1 << 2);
    pub const READ_ONLY: Self = Self(1 << 3);
    pub const DEVICE: Self = Self(1 << 4);
}

// ---------------------------------------------------------------------------
// Resource management (memory + I/O regions)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceType {
    Mem,
    Io,
    Irq,
    Dma,
    Bus,
}

#[derive(Debug, Clone)]
pub struct Resource {
    pub start: u64,
    pub end: u64,
    pub name: String,
    pub res_type: ResourceType,
    pub flags: u32,
}

impl Resource {
    pub fn size(&self) -> u64 {
        self.end - self.start + 1
    }

    pub fn overlaps(&self, other: &Resource) -> bool {
        self.res_type == other.res_type && self.start <= other.end && other.start <= self.end
    }
}

// ---------------------------------------------------------------------------
// IO remapping
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IoRemapType {
    Normal,
    NoCache,
    WriteCombine,
    WriteThrough,
}

pub fn ioremap(phys_addr: u64, size: usize) -> usize {
    ioremap_inner(phys_addr, size, IoRemapType::Normal)
}

pub fn ioremap_nocache(phys_addr: u64, size: usize) -> usize {
    ioremap_inner(phys_addr, size, IoRemapType::NoCache)
}

pub fn ioremap_wc(phys_addr: u64, size: usize) -> usize {
    ioremap_inner(phys_addr, size, IoRemapType::WriteCombine)
}

pub fn ioremap_wt(phys_addr: u64, size: usize) -> usize {
    ioremap_inner(phys_addr, size, IoRemapType::WriteThrough)
}

fn ioremap_inner(phys_addr: u64, size: usize, remap_type: IoRemapType) -> usize {
    let flags = match remap_type {
        IoRemapType::Normal => MmioFlags::NONE,
        IoRemapType::NoCache => MmioFlags::NOCACHE,
        IoRemapType::WriteCombine => MmioFlags::WRITE_COMBINE,
        IoRemapType::WriteThrough => MmioFlags::WRITE_THROUGH,
    };

    // Ensure the region is actually mapped in the page tables. The PTE cache mode
    // is always device/uncached (the requested `remap_type`/`flags` above is the
    // manager's bookkeeping record, not the PTE cache bits — preexisting behavior;
    // `map_mmio_region` only ever maps NO_CACHE).
    let virt_addr = crate::arch::mmu::kernel()
        .map_mmio_range(
            x86_64::PhysAddr::new(phys_addr),
            size,
            crate::arch::mmu::PageFlags::DEVICE,
        )
        .as_u64() as usize;

    let mut mgr = MMIO_MANAGER.lock();
    if let Some(mgr) = mgr.as_mut() {
        let region = MmioRegion {
            phys_addr,
            virt_addr,
            size,
            name: String::from("ioremap"),
            flags,
        };
        mgr.regions.push(region);
    }
    virt_addr
}

pub fn iounmap(virt_addr: usize) {
    let mut mgr = MMIO_MANAGER.lock();
    if let Some(mgr) = mgr.as_mut() {
        mgr.regions.retain(|r| r.virt_addr != virt_addr);
    }
}

// ---------------------------------------------------------------------------
// PCI BAR mapping
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct PciBarMapping {
    pub bar_index: u8,
    pub phys_addr: u64,
    pub virt_addr: usize,
    pub size: usize,
    pub is_io: bool,
    pub is_prefetchable: bool,
    pub is_64bit: bool,
}

pub fn pci_map_bar(bus: u8, device: u8, function: u8, bar_index: u8) -> Option<PciBarMapping> {
    let bar_offset = 0x10 + (bar_index as u16) * 4;
    let bar_value = pci_config_read32(bus, device, function, bar_offset);

    if bar_value == 0 {
        return None;
    }

    let is_io = (bar_value & 1) != 0;
    if is_io {
        let port_base = (bar_value & 0xFFFF_FFFC) as u64;
        return Some(PciBarMapping {
            bar_index,
            phys_addr: port_base,
            virt_addr: port_base as usize,
            size: 256,
            is_io: true,
            is_prefetchable: false,
            is_64bit: false,
        });
    }

    let is_64bit = (bar_value >> 1) & 0x3 == 2;
    let is_prefetchable = (bar_value & 0x8) != 0;

    let phys_addr = if is_64bit && bar_index < 5 {
        let upper = pci_config_read32(bus, device, function, bar_offset + 4);
        ((upper as u64) << 32) | ((bar_value & 0xFFFF_FFF0) as u64)
    } else {
        (bar_value & 0xFFFF_FFF0) as u64
    };

    let size = pci_bar_size(bus, device, function, bar_offset, is_64bit);
    let virt_addr = if is_prefetchable {
        ioremap_wc(phys_addr, size)
    } else {
        ioremap_nocache(phys_addr, size)
    };

    Some(PciBarMapping {
        bar_index,
        phys_addr,
        virt_addr,
        size,
        is_io: false,
        is_prefetchable,
        is_64bit,
    })
}

/// Probe the size in bytes of a memory BAR via the standard
/// write-all-ones / read-back-mask protocol (original value is restored).
/// Returns 0 for an I/O BAR or an unimplemented BAR.
pub fn pci_bar_size_bytes(bus: u8, device: u8, function: u8, bar_index: u8) -> usize {
    let bar_offset = 0x10 + (bar_index as u16) * 4;
    let bar_value = pci_config_read32(bus, device, function, bar_offset);
    if bar_value & 1 != 0 {
        return 0; // I/O BAR
    }
    let is_64bit = (bar_value >> 1) & 0x3 == 2;
    pci_bar_size(bus, device, function, bar_offset, is_64bit)
}

pub fn pci_unmap_bar(mapping: &PciBarMapping) {
    if !mapping.is_io {
        iounmap(mapping.virt_addr);
    }
}

fn pci_config_read32(bus: u8, dev: u8, func: u8, offset: u16) -> u32 {
    let address: u32 = 0x8000_0000
        | ((bus as u32) << 16)
        | ((dev as u32) << 11)
        | ((func as u32) << 8)
        | ((offset as u32) & 0xFC);
    unsafe {
        outl(0xCF8, address);
        inl(0xCFC)
    }
}

fn pci_bar_size(bus: u8, dev: u8, func: u8, offset: u16, _is_64bit: bool) -> usize {
    let original = pci_config_read32(bus, dev, func, offset);
    let addr = 0x8000_0000u32
        | ((bus as u32) << 16)
        | ((dev as u32) << 11)
        | ((func as u32) << 8)
        | ((offset as u32) & 0xFC);
    unsafe {
        outl(0xCF8, addr);
        outl(0xCFC, 0xFFFF_FFFF);
        outl(0xCF8, addr);
        let size_mask = inl(0xCFC);
        outl(0xCF8, addr);
        outl(0xCFC, original);
        let size_mask = size_mask & 0xFFFF_FFF0;
        if size_mask == 0 {
            return 0;
        }
        ((!size_mask).wrapping_add(1)) as usize
    }
}

// ---------------------------------------------------------------------------
// MMIO register fields — bitfield extraction/insertion
// ---------------------------------------------------------------------------

pub struct RegField {
    pub offset: usize,
    pub bit_lo: u8,
    pub bit_hi: u8,
}

impl RegField {
    pub const fn new(offset: usize, bit_lo: u8, bit_hi: u8) -> Self {
        Self {
            offset,
            bit_lo,
            bit_hi,
        }
    }

    pub fn mask(&self) -> u32 {
        let width = self.bit_hi - self.bit_lo + 1;
        ((1u32 << width) - 1) << self.bit_lo
    }

    pub fn extract(&self, value: u32) -> u32 {
        (value & self.mask()) >> self.bit_lo
    }

    pub fn insert(&self, reg: u32, field_val: u32) -> u32 {
        let width = self.bit_hi - self.bit_lo + 1;
        let clamped = field_val & ((1u32 << width) - 1);
        (reg & !self.mask()) | (clamped << self.bit_lo)
    }

    pub fn read(&self, io: &IoMem) -> u32 {
        let val = io.read32(self.offset);
        self.extract(val)
    }

    pub fn write(&self, io: &IoMem, field_val: u32) {
        let old = io.read32(self.offset);
        let new = self.insert(old, field_val);
        io.write32(self.offset, new);
    }
}

pub struct RegFieldSet {
    pub name: &'static str,
    pub fields: &'static [(&'static str, RegField)],
}

// ---------------------------------------------------------------------------
// Register polling
// ---------------------------------------------------------------------------

pub fn wait_for_bit_set(io: &IoMem, offset: usize, bit: u8, timeout_us: u64) -> bool {
    let mask = 1u32 << bit;
    let deadline = read_timestamp() + timeout_us;
    loop {
        if io.read32(offset) & mask != 0 {
            return true;
        }
        if read_timestamp() >= deadline {
            return false;
        }
        core::hint::spin_loop();
    }
}

pub fn wait_for_bit_clear(io: &IoMem, offset: usize, bit: u8, timeout_us: u64) -> bool {
    let mask = 1u32 << bit;
    let deadline = read_timestamp() + timeout_us;
    loop {
        if io.read32(offset) & mask == 0 {
            return true;
        }
        if read_timestamp() >= deadline {
            return false;
        }
        core::hint::spin_loop();
    }
}

pub fn wait_for_value(
    io: &IoMem,
    offset: usize,
    mask: u32,
    expected: u32,
    timeout_us: u64,
) -> bool {
    let deadline = read_timestamp() + timeout_us;
    loop {
        if io.read32(offset) & mask == expected {
            return true;
        }
        if read_timestamp() >= deadline {
            return false;
        }
        core::hint::spin_loop();
    }
}

fn read_timestamp() -> u64 {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        core::arch::x86_64::_rdtsc()
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        0
    }
}

// ---------------------------------------------------------------------------
// MMIO tracing
// ---------------------------------------------------------------------------

struct MmioTracer {
    enabled: AtomicBool,
    read_count: AtomicU64,
    write_count: AtomicU64,
}

impl MmioTracer {
    const fn new() -> Self {
        Self {
            enabled: AtomicBool::new(false),
            read_count: AtomicU64::new(0),
            write_count: AtomicU64::new(0),
        }
    }

    fn enable(&self) {
        self.enabled.store(true, Ordering::SeqCst);
    }

    fn disable(&self) {
        self.enabled.store(false, Ordering::SeqCst);
    }

    fn trace_read(&self, addr: usize, value: u64, width: u8) {
        if self.enabled.load(Ordering::Relaxed) {
            self.read_count.fetch_add(1, Ordering::Relaxed);
            let _ = (addr, value, width);
        }
    }

    fn trace_write(&self, addr: usize, value: u64, width: u8) {
        if self.enabled.load(Ordering::Relaxed) {
            self.write_count.fetch_add(1, Ordering::Relaxed);
            let _ = (addr, value, width);
        }
    }

    fn stats(&self) -> (u64, u64) {
        (
            self.read_count.load(Ordering::Relaxed),
            self.write_count.load(Ordering::Relaxed),
        )
    }
}

static MMIO_TRACER: MmioTracer = MmioTracer::new();

// ---------------------------------------------------------------------------
// IO space allocation
// ---------------------------------------------------------------------------

struct IoPortAllocator {
    free_ranges: Vec<(u16, u16)>,
}

impl IoPortAllocator {
    fn new() -> Self {
        Self {
            free_ranges: alloc::vec![(0x1000, 0xFFFF)],
        }
    }

    fn allocate(&mut self, count: u16) -> Option<u16> {
        for i in 0..self.free_ranges.len() {
            let (start, end) = self.free_ranges[i];
            let avail = end - start + 1;
            if avail >= count {
                let base = start;
                if avail == count {
                    self.free_ranges.remove(i);
                } else {
                    self.free_ranges[i].0 = start + count;
                }
                return Some(base);
            }
        }
        None
    }

    fn release(&mut self, base: u16, count: u16) {
        let end = base + count - 1;
        self.free_ranges.push((base, end));
        self.free_ranges.sort_by_key(|r| r.0);
        self.coalesce();
    }

    fn coalesce(&mut self) {
        let mut i = 0;
        while i + 1 < self.free_ranges.len() {
            if self.free_ranges[i].1 + 1 >= self.free_ranges[i + 1].0 {
                self.free_ranges[i].1 =
                    core::cmp::max(self.free_ranges[i].1, self.free_ranges[i + 1].1);
                self.free_ranges.remove(i + 1);
            } else {
                i += 1;
            }
        }
    }
}

struct IoMemAllocator {
    free_ranges: Vec<(u64, u64)>,
}

impl IoMemAllocator {
    fn new() -> Self {
        Self {
            free_ranges: alloc::vec![(0xE000_0000, 0xEFFF_FFFF)],
        }
    }

    fn allocate(&mut self, size: u64, align: u64) -> Option<u64> {
        for i in 0..self.free_ranges.len() {
            let (start, end) = self.free_ranges[i];
            let aligned = (start + align - 1) & !(align - 1);
            if aligned + size - 1 <= end {
                if aligned == start && aligned + size - 1 == end {
                    self.free_ranges.remove(i);
                } else if aligned == start {
                    self.free_ranges[i].0 = aligned + size;
                } else if aligned + size - 1 == end {
                    self.free_ranges[i].1 = aligned - 1;
                } else {
                    self.free_ranges[i].1 = aligned - 1;
                    self.free_ranges.insert(i + 1, (aligned + size, end));
                }
                return Some(aligned);
            }
        }
        None
    }

    fn release(&mut self, addr: u64, size: u64) {
        self.free_ranges.push((addr, addr + size - 1));
        self.free_ranges.sort_by_key(|r| r.0);
        self.coalesce();
    }

    fn coalesce(&mut self) {
        let mut i = 0;
        while i + 1 < self.free_ranges.len() {
            if self.free_ranges[i].1 + 1 >= self.free_ranges[i + 1].0 {
                self.free_ranges[i].1 =
                    core::cmp::max(self.free_ranges[i].1, self.free_ranges[i + 1].1);
                self.free_ranges.remove(i + 1);
            } else {
                i += 1;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Resource management functions
// ---------------------------------------------------------------------------

pub fn request_mem_region(start: u64, size: u64, name: &str) -> Result<(), &'static str> {
    let res = Resource {
        start,
        end: start + size - 1,
        name: String::from(name),
        res_type: ResourceType::Mem,
        flags: 0,
    };
    let mut mgr = MMIO_MANAGER.lock();
    if let Some(mgr) = mgr.as_mut() {
        for existing in &mgr.resources {
            if existing.overlaps(&res) {
                return Err("resource conflict: memory region already claimed");
            }
        }
        mgr.resources.push(res);
        Ok(())
    } else {
        Err("MMIO manager not initialized")
    }
}

pub fn release_mem_region(start: u64, size: u64) {
    let mut mgr = MMIO_MANAGER.lock();
    if let Some(mgr) = mgr.as_mut() {
        mgr.resources.retain(|r| {
            !(r.res_type == ResourceType::Mem && r.start == start && r.end == start + size - 1)
        });
    }
}

pub fn request_region(start: u64, size: u64, name: &str) -> Result<(), &'static str> {
    let res = Resource {
        start,
        end: start + size - 1,
        name: String::from(name),
        res_type: ResourceType::Io,
        flags: 0,
    };
    let mut mgr = MMIO_MANAGER.lock();
    if let Some(mgr) = mgr.as_mut() {
        for existing in &mgr.resources {
            if existing.overlaps(&res) {
                return Err("resource conflict: IO region already claimed");
            }
        }
        mgr.resources.push(res);
        Ok(())
    } else {
        Err("MMIO manager not initialized")
    }
}

pub fn release_region(start: u64, size: u64) {
    let mut mgr = MMIO_MANAGER.lock();
    if let Some(mgr) = mgr.as_mut() {
        mgr.resources.retain(|r| {
            !(r.res_type == ResourceType::Io && r.start == start && r.end == start + size - 1)
        });
    }
}

// ---------------------------------------------------------------------------
// Platform device / driver
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct PlatformResource {
    pub res_type: ResourceType,
    pub start: u64,
    pub end: u64,
    pub name: String,
}

pub struct PlatformDevice {
    pub name: String,
    pub id: i32,
    pub resources: Vec<PlatformResource>,
    pub compatible: Vec<String>,
    pub properties: Vec<(String, DeviceProperty)>,
}

impl PlatformDevice {
    pub fn new(name: &str, id: i32) -> Self {
        Self {
            name: String::from(name),
            id,
            resources: Vec::new(),
            compatible: Vec::new(),
            properties: Vec::new(),
        }
    }

    pub fn add_resource(&mut self, res: PlatformResource) {
        self.resources.push(res);
    }

    pub fn get_resource(&self, res_type: ResourceType, index: usize) -> Option<&PlatformResource> {
        self.resources
            .iter()
            .filter(|r| r.res_type == res_type)
            .nth(index)
    }

    pub fn get_irq(&self, index: usize) -> Option<u64> {
        self.get_resource(ResourceType::Irq, index).map(|r| r.start)
    }

    pub fn get_mem(&self, index: usize) -> Option<(u64, u64)> {
        self.get_resource(ResourceType::Mem, index)
            .map(|r| (r.start, r.end - r.start + 1))
    }
}

#[derive(Debug, Clone)]
pub enum DeviceProperty {
    U32(u32),
    U64(u64),
    Str(String),
    Bytes(Vec<u8>),
    Bool(bool),
}

pub trait PlatformDriver: Send + Sync {
    fn probe(&self, dev: &PlatformDevice) -> Result<(), &'static str>;
    fn remove(&self, dev: &PlatformDevice) -> Result<(), &'static str>;
    fn compatible(&self) -> &[&str];
    fn name(&self) -> &str;
}

struct DriverEntry {
    driver: Box<dyn PlatformDriver>,
}

// ---------------------------------------------------------------------------
// Device Tree (FDT) parsing
// ---------------------------------------------------------------------------

const FDT_MAGIC: u32 = 0xD00DFEED;
const FDT_BEGIN_NODE: u32 = 1;
const FDT_END_NODE: u32 = 2;
const FDT_PROP: u32 = 3;
const FDT_NOP: u32 = 4;
const FDT_END: u32 = 9;

pub struct FdtHeader {
    pub magic: u32,
    pub totalsize: u32,
    pub off_dt_struct: u32,
    pub off_dt_strings: u32,
    pub off_mem_rsvmap: u32,
    pub version: u32,
    pub last_comp_version: u32,
    pub boot_cpuid_phys: u32,
    pub size_dt_strings: u32,
    pub size_dt_struct: u32,
}

pub struct FdtNode {
    pub name: String,
    pub properties: Vec<FdtProperty>,
    pub children: Vec<FdtNode>,
}

#[derive(Clone)]
pub struct FdtProperty {
    pub name: String,
    pub value: Vec<u8>,
}

impl FdtProperty {
    pub fn as_u32(&self) -> Option<u32> {
        if self.value.len() >= 4 {
            Some(u32::from_be_bytes([
                self.value[0],
                self.value[1],
                self.value[2],
                self.value[3],
            ]))
        } else {
            None
        }
    }

    pub fn as_u64(&self) -> Option<u64> {
        if self.value.len() >= 8 {
            Some(u64::from_be_bytes([
                self.value[0],
                self.value[1],
                self.value[2],
                self.value[3],
                self.value[4],
                self.value[5],
                self.value[6],
                self.value[7],
            ]))
        } else {
            None
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        let data = if self.value.last() == Some(&0) {
            &self.value[..self.value.len() - 1]
        } else {
            &self.value
        };
        core::str::from_utf8(data).ok()
    }

    pub fn as_string_list(&self) -> Vec<&str> {
        let mut result = Vec::new();
        let mut start = 0;
        for (i, &b) in self.value.iter().enumerate() {
            if b == 0 && i > start {
                if let Ok(s) = core::str::from_utf8(&self.value[start..i]) {
                    result.push(s);
                }
                start = i + 1;
            }
        }
        result
    }
}

impl FdtNode {
    pub fn get_property(&self, name: &str) -> Option<&FdtProperty> {
        self.properties.iter().find(|p| p.name == name)
    }

    pub fn compatible(&self) -> Vec<&str> {
        self.get_property("compatible")
            .map(|p| p.as_string_list())
            .unwrap_or_default()
    }

    pub fn is_compatible(&self, compat: &str) -> bool {
        self.compatible().iter().any(|c| *c == compat)
    }

    pub fn reg(&self) -> Vec<(u64, u64)> {
        let mut result = Vec::new();
        if let Some(prop) = self.get_property("reg") {
            let data = &prop.value;
            let mut offset = 0;
            while offset + 16 <= data.len() {
                let addr = u64::from_be_bytes([
                    data[offset],
                    data[offset + 1],
                    data[offset + 2],
                    data[offset + 3],
                    data[offset + 4],
                    data[offset + 5],
                    data[offset + 6],
                    data[offset + 7],
                ]);
                let size = u64::from_be_bytes([
                    data[offset + 8],
                    data[offset + 9],
                    data[offset + 10],
                    data[offset + 11],
                    data[offset + 12],
                    data[offset + 13],
                    data[offset + 14],
                    data[offset + 15],
                ]);
                result.push((addr, size));
                offset += 16;
            }
        }
        result
    }

    pub fn interrupts(&self) -> Vec<u32> {
        let mut result = Vec::new();
        if let Some(prop) = self.get_property("interrupts") {
            let data = &prop.value;
            let mut offset = 0;
            while offset + 4 <= data.len() {
                let irq = u32::from_be_bytes([
                    data[offset],
                    data[offset + 1],
                    data[offset + 2],
                    data[offset + 3],
                ]);
                result.push(irq);
                offset += 4;
            }
        }
        result
    }

    pub fn find_node(&self, path: &str) -> Option<&FdtNode> {
        let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
        self.find_node_recursive(&parts, 0)
    }

    fn find_node_recursive(&self, parts: &[&str], depth: usize) -> Option<&FdtNode> {
        if depth >= parts.len() {
            return Some(self);
        }
        for child in &self.children {
            let node_name = child.name.split('@').next().unwrap_or(&child.name);
            if node_name == parts[depth] {
                return child.find_node_recursive(parts, depth + 1);
            }
        }
        None
    }

    pub fn find_compatible(&self, compat: &str) -> Vec<&FdtNode> {
        let mut result = Vec::new();
        if self.is_compatible(compat) {
            result.push(self);
        }
        for child in &self.children {
            result.extend(child.find_compatible(compat));
        }
        result
    }
}

pub fn parse_fdt(data: &[u8]) -> Option<FdtNode> {
    if data.len() < 40 {
        return None;
    }
    let magic = u32::from_be_bytes([data[0], data[1], data[2], data[3]]);
    if magic != FDT_MAGIC {
        return None;
    }
    let off_struct = u32::from_be_bytes([data[8], data[9], data[10], data[11]]) as usize;
    let off_strings = u32::from_be_bytes([data[12], data[13], data[14], data[15]]) as usize;
    let mut offset = off_struct;
    parse_fdt_node(data, &mut offset, off_strings)
}

fn parse_fdt_node(data: &[u8], offset: &mut usize, str_off: usize) -> Option<FdtNode> {
    if *offset + 4 > data.len() {
        return None;
    }
    let token = read_be32(data, *offset);
    *offset += 4;
    if token != FDT_BEGIN_NODE {
        return None;
    }

    let name_start = *offset;
    while *offset < data.len() && data[*offset] != 0 {
        *offset += 1;
    }
    let name = core::str::from_utf8(&data[name_start..*offset])
        .unwrap_or("")
        .into();
    *offset += 1;
    *offset = (*offset + 3) & !3;

    let mut node = FdtNode {
        name,
        properties: Vec::new(),
        children: Vec::new(),
    };

    loop {
        if *offset + 4 > data.len() {
            break;
        }
        let token = read_be32(data, *offset);
        match token {
            FDT_PROP => {
                *offset += 4;
                let len = read_be32(data, *offset) as usize;
                *offset += 4;
                let name_off = read_be32(data, *offset) as usize;
                *offset += 4;
                let value = data[*offset..*offset + len].to_vec();
                *offset += len;
                *offset = (*offset + 3) & !3;
                let prop_name = read_fdt_string(data, str_off + name_off);
                node.properties.push(FdtProperty {
                    name: prop_name,
                    value,
                });
            }
            FDT_BEGIN_NODE => {
                if let Some(child) = parse_fdt_node(data, offset, str_off) {
                    node.children.push(child);
                }
            }
            FDT_END_NODE => {
                *offset += 4;
                break;
            }
            FDT_NOP => {
                *offset += 4;
            }
            FDT_END | _ => {
                *offset += 4;
                break;
            }
        }
    }

    Some(node)
}

fn read_be32(data: &[u8], offset: usize) -> u32 {
    u32::from_be_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
    ])
}

fn read_fdt_string(data: &[u8], offset: usize) -> String {
    let mut end = offset;
    while end < data.len() && data[end] != 0 {
        end += 1;
    }
    String::from(core::str::from_utf8(&data[offset..end]).unwrap_or(""))
}

// ---------------------------------------------------------------------------
// Clock framework
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClockType {
    Fixed,
    Gate,
    Divider,
    Mux,
    Pll,
    Composite,
}

pub struct Clock {
    pub id: u32,
    pub name: String,
    pub clock_type: ClockType,
    pub parent_id: Option<u32>,
    pub rate_hz: u64,
    pub enabled: bool,
    pub gate_reg: Option<usize>,
    pub gate_bit: Option<u8>,
    pub div_reg: Option<usize>,
    pub div_shift: u8,
    pub div_width: u8,
    pub mux_reg: Option<usize>,
    pub mux_shift: u8,
    pub mux_width: u8,
    pub pll_params: Option<PllParams>,
}

#[derive(Debug, Clone, Copy)]
pub struct PllParams {
    pub ref_rate: u64,
    pub mult_min: u32,
    pub mult_max: u32,
    pub div_min: u32,
    pub div_max: u32,
    pub frac_bits: u8,
    pub lock_timeout_us: u64,
}

struct ClockTree {
    clocks: Vec<Clock>,
    next_id: u32,
}

impl ClockTree {
    fn new() -> Self {
        Self {
            clocks: Vec::new(),
            next_id: 1,
        }
    }

    fn register(&mut self, mut clk: Clock) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        clk.id = id;
        self.clocks.push(clk);
        id
    }

    fn find(&self, id: u32) -> Option<&Clock> {
        self.clocks.iter().find(|c| c.id == id)
    }

    fn find_mut(&mut self, id: u32) -> Option<&mut Clock> {
        self.clocks.iter_mut().find(|c| c.id == id)
    }

    fn find_by_name(&self, name: &str) -> Option<&Clock> {
        self.clocks.iter().find(|c| c.name == name)
    }

    fn get_rate(&self, id: u32) -> u64 {
        if let Some(clk) = self.find(id) {
            let parent_rate = clk.parent_id.map(|p| self.get_rate(p)).unwrap_or(0);
            match clk.clock_type {
                ClockType::Fixed => clk.rate_hz,
                ClockType::Gate => parent_rate,
                ClockType::Divider => {
                    let div = if clk.div_width > 0 {
                        core::cmp::max(1, clk.rate_hz)
                    } else {
                        1
                    };
                    parent_rate / div
                }
                ClockType::Mux => parent_rate,
                ClockType::Pll => clk.rate_hz,
                ClockType::Composite => clk.rate_hz,
            }
        } else {
            0
        }
    }
}

pub fn clk_get(name: &str) -> Option<u32> {
    let mgr = MMIO_MANAGER.lock();
    mgr.as_ref()?.clock_tree.find_by_name(name).map(|c| c.id)
}

pub fn clk_enable(id: u32) -> Result<(), &'static str> {
    let mut mgr = MMIO_MANAGER.lock();
    let mgr = mgr.as_mut().ok_or("MMIO manager not initialized")?;
    let clk = mgr.clock_tree.find_mut(id).ok_or("clock not found")?;
    clk.enabled = true;
    Ok(())
}

pub fn clk_disable(id: u32) -> Result<(), &'static str> {
    let mut mgr = MMIO_MANAGER.lock();
    let mgr = mgr.as_mut().ok_or("MMIO manager not initialized")?;
    let clk = mgr.clock_tree.find_mut(id).ok_or("clock not found")?;
    clk.enabled = false;
    Ok(())
}

pub fn clk_set_rate(id: u32, rate: u64) -> Result<u64, &'static str> {
    let mut mgr = MMIO_MANAGER.lock();
    let mgr = mgr.as_mut().ok_or("MMIO manager not initialized")?;
    let clk = mgr.clock_tree.find_mut(id).ok_or("clock not found")?;
    clk.rate_hz = rate;
    Ok(rate)
}

pub fn clk_get_rate(id: u32) -> u64 {
    let mgr = MMIO_MANAGER.lock();
    mgr.as_ref().map(|m| m.clock_tree.get_rate(id)).unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Reset controller
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResetState {
    Asserted,
    Deasserted,
}

pub struct ResetControl {
    pub id: u32,
    pub name: String,
    pub state: ResetState,
    pub reg_offset: usize,
    pub bit: u8,
}

struct ResetController {
    controls: Vec<ResetControl>,
    next_id: u32,
}

impl ResetController {
    fn new() -> Self {
        Self {
            controls: Vec::new(),
            next_id: 1,
        }
    }

    fn register(&mut self, name: &str, reg_offset: usize, bit: u8) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        self.controls.push(ResetControl {
            id,
            name: String::from(name),
            state: ResetState::Deasserted,
            reg_offset,
            bit,
        });
        id
    }

    fn find_mut(&mut self, id: u32) -> Option<&mut ResetControl> {
        self.controls.iter_mut().find(|c| c.id == id)
    }

    fn find(&self, id: u32) -> Option<&ResetControl> {
        self.controls.iter().find(|c| c.id == id)
    }
}

pub fn reset_control_get(name: &str) -> Option<u32> {
    let mgr = MMIO_MANAGER.lock();
    mgr.as_ref()?
        .reset_ctrl
        .controls
        .iter()
        .find(|r| r.name == name)
        .map(|r| r.id)
}

pub fn reset_control_assert(id: u32) -> Result<(), &'static str> {
    let mut mgr = MMIO_MANAGER.lock();
    let mgr = mgr.as_mut().ok_or("MMIO manager not initialized")?;
    let ctrl = mgr.reset_ctrl.find_mut(id).ok_or("reset not found")?;
    ctrl.state = ResetState::Asserted;
    Ok(())
}

pub fn reset_control_deassert(id: u32) -> Result<(), &'static str> {
    let mut mgr = MMIO_MANAGER.lock();
    let mgr = mgr.as_mut().ok_or("MMIO manager not initialized")?;
    let ctrl = mgr.reset_ctrl.find_mut(id).ok_or("reset not found")?;
    ctrl.state = ResetState::Deasserted;
    Ok(())
}

pub fn reset_control_reset(id: u32) -> Result<(), &'static str> {
    reset_control_assert(id)?;
    for _ in 0..1000 {
        core::hint::spin_loop();
    }
    reset_control_deassert(id)
}

pub fn reset_control_status(id: u32) -> Result<ResetState, &'static str> {
    let mgr = MMIO_MANAGER.lock();
    let mgr = mgr.as_ref().ok_or("MMIO manager not initialized")?;
    let ctrl = mgr.reset_ctrl.find(id).ok_or("reset not found")?;
    Ok(ctrl.state)
}

// ---------------------------------------------------------------------------
// MMIO Manager (global state)
// ---------------------------------------------------------------------------

struct MmioManager {
    regions: Vec<MmioRegion>,
    resources: Vec<Resource>,
    io_port_alloc: IoPortAllocator,
    io_mem_alloc: IoMemAllocator,
    drivers: Vec<DriverEntry>,
    devices: Vec<PlatformDevice>,
    clock_tree: ClockTree,
    reset_ctrl: ResetController,
}

impl MmioManager {
    fn new() -> Self {
        Self {
            regions: Vec::new(),
            resources: Vec::new(),
            io_port_alloc: IoPortAllocator::new(),
            io_mem_alloc: IoMemAllocator::new(),
            drivers: Vec::new(),
            devices: Vec::new(),
            clock_tree: ClockTree::new(),
            reset_ctrl: ResetController::new(),
        }
    }

    fn register_region(&mut self, region: MmioRegion) {
        self.regions.push(region);
    }

    fn unregister_region(&mut self, phys_addr: u64) {
        self.regions.retain(|r| r.phys_addr != phys_addr);
    }

    fn lookup_by_phys(&self, phys_addr: u64) -> Option<&MmioRegion> {
        self.regions
            .iter()
            .find(|r| phys_addr >= r.phys_addr && phys_addr < r.phys_addr + r.size as u64)
    }

    fn register_driver(&mut self, driver: Box<dyn PlatformDriver>) {
        self.try_bind_driver(&driver);
        self.drivers.push(DriverEntry { driver });
    }

    fn register_device(&mut self, dev: PlatformDevice) {
        for entry in &self.drivers {
            for compat in entry.driver.compatible() {
                if dev.compatible.iter().any(|c| c == compat) {
                    let _ = entry.driver.probe(&dev);
                    break;
                }
            }
        }
        self.devices.push(dev);
    }

    fn try_bind_driver(&self, driver: &Box<dyn PlatformDriver>) {
        for dev in &self.devices {
            for compat in driver.compatible() {
                if dev.compatible.iter().any(|c| c == compat) {
                    let _ = driver.probe(dev);
                    break;
                }
            }
        }
    }

    fn allocate_io_ports(&mut self, count: u16) -> Option<u16> {
        self.io_port_alloc.allocate(count)
    }

    fn release_io_ports(&mut self, base: u16, count: u16) {
        self.io_port_alloc.release(base, count);
    }

    fn allocate_io_mem(&mut self, size: u64, align: u64) -> Option<u64> {
        self.io_mem_alloc.allocate(size, align)
    }

    fn release_io_mem(&mut self, addr: u64, size: u64) {
        self.io_mem_alloc.release(addr, size);
    }
}

pub static MMIO_MANAGER: Mutex<Option<MmioManager>> = Mutex::new(None);

pub fn init() {
    let mut mgr = MMIO_MANAGER.lock();
    *mgr = Some(MmioManager::new());
}

// ---------------------------------------------------------------------------
// Typed registers (volatile crate)
// ---------------------------------------------------------------------------
//
// The driver fleet hand-writes raw read_volatile/write_volatile at base+offset
// everywhere — the class that bit us (xHCI DCI off-by-one, HDA RIRB offset).
// `Reg<T>` wraps a register in the `volatile` crate's guaranteed-volatile access
// behind a typed handle, so a register block can be declared once with named
// offsets. New driver register maps should adopt it.

/// A single memory-mapped register of type `T` at a fixed address.
#[derive(Clone, Copy)]
pub struct Reg<T: Copy> {
    addr: *mut T,
}

impl<T: Copy> Reg<T> {
    /// # Safety
    /// `addr` must be a valid, correctly-aligned MMIO register address for `T`
    /// that stays mapped for the lifetime of all accesses.
    pub const unsafe fn new(addr: usize) -> Self {
        Self {
            addr: addr as *mut T,
        }
    }

    /// Volatile read.
    pub fn read(&self) -> T {
        unsafe { volatile::Volatile::new(&*self.addr).read() }
    }

    /// Volatile write.
    pub fn write(&self, val: T) {
        unsafe { volatile::Volatile::new(&mut *self.addr).write(val) }
    }
}

// Safety: a register is just an address; volatile read/write are the unit of
// access and carry no invariants beyond the caller's `new` contract.
unsafe impl<T: Copy> Send for Reg<T> {}
unsafe impl<T: Copy> Sync for Reg<T> {}

// A typed 32-bit control register, declared field-by-field instead of
// hand-masking bits (modular-bitfield). Layout (LSB first): bit0 enable,
// bit1 reset, bits2-3 mode, bits4-11 irq_vector, rest reserved. New driver
// register maps should model their control/status words this way and store
// them through a `Reg<u32>`.
#[modular_bitfield::bitfield(bits = 32)]
#[derive(Clone, Copy)]
pub struct CtrlReg {
    pub enable: bool,
    pub reset: bool,
    pub mode: modular_bitfield::specifiers::B2,
    pub irq_vector: modular_bitfield::specifiers::B8,
    #[skip]
    __: modular_bitfield::specifiers::B20,
}

/// R10 smoketest — exercises the typed volatile read/write against a scratch
/// memory cell, and the modular-bitfield register field encoding. Deterministic;
/// not a real device, so it can print FAIL.
pub fn run_boot_smoketest() {
    let mut cell: u32 = 0;
    let reg = unsafe { Reg::<u32>::new(&mut cell as *mut u32 as usize) };
    reg.write(0xDEAD_BEEF);
    let got = reg.read();
    let reg_pass = got == 0xDEAD_BEEF && cell == 0xDEAD_BEEF;

    // Build a control word by name and confirm the bit layout, then round-trip.
    let ctrl = CtrlReg::new()
        .with_enable(true) // bit0
        .with_mode(0b10) // bits2-3 = 0b10 -> 0x8
        .with_irq_vector(0x42); // bits4-11 -> 0x420
    let word = u32::from_le_bytes(ctrl.into_bytes());
    // enable(0x1) | mode<<2 (0b10<<2=0x8) | irq_vector<<4 (0x42<<4=0x420) = 0x429
    let layout_ok = word == 0x0000_0429;
    let back = CtrlReg::from_bytes(word.to_le_bytes());
    let rt_ok = back.enable() && back.mode() == 0b10 && back.irq_vector() == 0x42;

    let pass = reg_pass && layout_ok && rt_ok;
    crate::selftest::record_smoketest("mmio", pass);
    crate::serial_println!(
        "[mmio] reg scratch=0x{:08x} ctrl_word=0x{:08x} bitfield_rt={} -> {}",
        got,
        word,
        rt_ok,
        if pass { "PASS" } else { "FAIL" }
    );
}
