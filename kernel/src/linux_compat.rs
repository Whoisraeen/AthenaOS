#![allow(dead_code)]

//! Linux Driver Compatibility Layer for RaeenOS.
//!
//! Provides a shim that translates Linux kernel driver APIs into RaeenOS
//! kernel interfaces for **userspace LinuxKPI / vendor driver hosting**
//! (see `docs/LINUX_DRIVER_STRATEGY.md`). Not a monolithic in-kernel Linux
//! clone — DMA uses the frame allocator; IOMMU sandboxing is future work.
//!
//! R10: `init()` + `run_boot_smoketest()` + `/proc/raeen/linux_compat`.

extern crate alloc;

use crate::arch::PhysAddr;
use alloc::boxed::Box;
use alloc::collections::{BTreeMap, VecDeque};
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use spin::Mutex;
use x86_64::structures::paging::FrameAllocator;

// ────────────────────────────────────────────────────────────────────────────
// 1. Linux Device Model Shim
// ────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BusType {
    Pci,
    Usb,
    Platform,
    I2c,
    Spi,
    Acpi,
    Virtual,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DevicePowerState {
    D0Active,
    D1Sleep,
    D2Sleep,
    D3Hot,
    D3Cold,
    Unknown,
}

#[derive(Debug, Clone)]
pub struct LinuxDevice {
    pub name: String,
    pub driver_name: String,
    pub bus_type: BusType,
    pub device_id: u64,
    pub parent: Option<u64>,
    pub driver_data: Option<*mut u8>,
    pub power_state: DevicePowerState,
    pub irq: Option<u32>,
    pub dma_mask: u64,
}

unsafe impl Send for LinuxDevice {}
unsafe impl Sync for LinuxDevice {}

impl LinuxDevice {
    pub fn new(name: &str, bus: BusType, id: u64) -> Self {
        Self {
            name: String::from(name),
            driver_name: String::new(),
            bus_type: bus,
            device_id: id,
            parent: None,
            driver_data: None,
            power_state: DevicePowerState::D0Active,
            irq: None,
            dma_mask: 0xFFFF_FFFF,
        }
    }

    pub fn set_driver_data(&mut self, data: *mut u8) {
        self.driver_data = Some(data);
    }

    pub fn get_driver_data(&self) -> Option<*mut u8> {
        self.driver_data
    }

    pub fn set_power_state(&mut self, state: DevicePowerState) {
        self.power_state = state;
        printk(
            LogLevel::Debug,
            &alloc::format!("{}: power state -> {:?}", self.name, state),
        );
    }

    pub fn set_dma_mask(&mut self, mask: u64) -> i32 {
        if mask == 0 {
            return -1; // -EINVAL
        }
        self.dma_mask = mask;
        0
    }
}

pub struct DeviceRegistry {
    devices: BTreeMap<u64, LinuxDevice>,
    next_id: u64,
}

impl DeviceRegistry {
    pub const fn new() -> Self {
        Self {
            devices: BTreeMap::new(),
            next_id: 1,
        }
    }

    pub fn register(&mut self, mut dev: LinuxDevice) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        dev.device_id = id;
        printk(
            LogLevel::Info,
            &alloc::format!("linux_compat: registered device '{}' id={}", dev.name, id),
        );
        self.devices.insert(id, dev);
        id
    }

    pub fn unregister(&mut self, id: u64) -> Option<LinuxDevice> {
        let dev = self.devices.remove(&id);
        if let Some(ref d) = dev {
            printk(
                LogLevel::Info,
                &alloc::format!("linux_compat: unregistered device '{}'", d.name),
            );
        }
        dev
    }

    pub fn get(&self, id: u64) -> Option<&LinuxDevice> {
        self.devices.get(&id)
    }

    pub fn get_mut(&mut self, id: u64) -> Option<&mut LinuxDevice> {
        self.devices.get_mut(&id)
    }

    pub fn count(&self) -> usize {
        self.devices.len()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&u64, &LinuxDevice)> {
        self.devices.iter()
    }

    pub fn find_by_bus(&self, bus: BusType) -> Vec<&LinuxDevice> {
        self.devices
            .values()
            .filter(|d| d.bus_type == bus)
            .collect()
    }
}

pub static DEVICE_REGISTRY: Mutex<DeviceRegistry> = Mutex::new(DeviceRegistry::new());

// ────────────────────────────────────────────────────────────────────────────
// 2. PCI Driver Compatibility
// ────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinuxPciDeviceId {
    pub vendor: u32,
    pub device: u32,
    pub subvendor: u32,
    pub subdevice: u32,
    pub class: u32,
    pub class_mask: u32,
}

impl LinuxPciDeviceId {
    pub const ANY: u32 = 0xFFFF_FFFF;

    pub fn matches(&self, vendor: u32, device: u32, class: u32) -> bool {
        let vendor_ok = self.vendor == Self::ANY || self.vendor == vendor;
        let device_ok = self.device == Self::ANY || self.device == device;
        let class_ok =
            self.class_mask == 0 || (class & self.class_mask) == (self.class & self.class_mask);
        vendor_ok && device_ok && class_ok
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PciBarType {
    Memory32,
    Memory64,
    Io,
    Unused,
}

#[derive(Debug, Clone, Copy)]
pub struct PciBar {
    pub bar_type: PciBarType,
    pub base_address: u64,
    pub size: u64,
    pub prefetchable: bool,
}

impl PciBar {
    pub const EMPTY: Self = Self {
        bar_type: PciBarType::Unused,
        base_address: 0,
        size: 0,
        prefetchable: false,
    };
}

pub struct LinuxPciDriver {
    pub name: &'static str,
    pub id_table: Vec<LinuxPciDeviceId>,
    pub probe: Option<fn(&LinuxPciDevice) -> i32>,
    pub remove: Option<fn(&LinuxPciDevice)>,
    pub suspend: Option<fn(&LinuxPciDevice) -> i32>,
    pub resume: Option<fn(&LinuxPciDevice) -> i32>,
    pub shutdown: Option<fn(&LinuxPciDevice)>,
}

impl LinuxPciDriver {
    pub fn match_device(&self, vendor: u32, device: u32, class: u32) -> bool {
        self.id_table
            .iter()
            .any(|id| id.matches(vendor, device, class))
    }
}

#[derive(Debug, Clone)]
pub struct LinuxPciDevice {
    pub vendor: u16,
    pub device: u16,
    pub subsystem_vendor: u16,
    pub subsystem_device: u16,
    pub class: u32,
    pub revision: u8,
    pub irq: u32,
    pub bars: [PciBar; 6],
    pub dev: LinuxDevice,
    pub msi_enabled: bool,
    pub msix_enabled: bool,
    pub bus_master: bool,
}

impl LinuxPciDevice {
    pub fn new(vendor: u16, device: u16, class: u32) -> Self {
        let name = alloc::format!("pci:{:04x}:{:04x}", vendor, device);
        Self {
            vendor,
            device,
            subsystem_vendor: 0,
            subsystem_device: 0,
            class,
            revision: 0,
            irq: 0,
            bars: [PciBar::EMPTY; 6],
            dev: LinuxDevice::new(&name, BusType::Pci, 0),
            msi_enabled: false,
            msix_enabled: false,
            bus_master: false,
        }
    }

    pub fn enable_device(&mut self) -> i32 {
        self.dev.set_power_state(DevicePowerState::D0Active);
        printk(
            LogLevel::Info,
            &alloc::format!("pci: enabled {:04x}:{:04x}", self.vendor, self.device),
        );
        0
    }

    pub fn set_master(&mut self) {
        self.bus_master = true;
    }

    pub fn enable_msi(&mut self) -> i32 {
        self.msi_enabled = true;
        self.msix_enabled = false;
        0
    }

    pub fn enable_msix(&mut self, num_vectors: u32) -> i32 {
        if num_vectors == 0 {
            return -1;
        }
        self.msix_enabled = true;
        self.msi_enabled = false;
        0
    }

    pub fn disable_msi(&mut self) {
        self.msi_enabled = false;
    }

    pub fn iomap_bar(&self, bar_index: usize) -> Option<u64> {
        if bar_index >= 6 {
            return None;
        }
        let bar = &self.bars[bar_index];
        match bar.bar_type {
            PciBarType::Unused => None,
            _ => Some(bar.base_address),
        }
    }

    pub fn bar_size(&self, bar_index: usize) -> u64 {
        if bar_index >= 6 {
            return 0;
        }
        self.bars[bar_index].size
    }

    pub fn set_dma_mask(&mut self, mask: u64) -> i32 {
        self.dev.set_dma_mask(mask)
    }

    pub fn read_config_byte(&self, _offset: u8) -> u8 {
        0
    }

    pub fn read_config_word(&self, _offset: u8) -> u16 {
        0
    }

    pub fn read_config_dword(&self, _offset: u8) -> u32 {
        0
    }

    pub fn write_config_byte(&self, _offset: u8, _value: u8) {}
    pub fn write_config_word(&self, _offset: u8, _value: u16) {}
    pub fn write_config_dword(&self, _offset: u8, _value: u32) {}
}

// ────────────────────────────────────────────────────────────────────────────
// 3. Network Device Compatibility (struct net_device)
// ────────────────────────────────────────────────────────────────────────────

bitflags::bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct NetDeviceFlags: u32 {
        const UP            = 1 << 0;
        const BROADCAST     = 1 << 1;
        const LOOPBACK      = 1 << 3;
        const POINTOPOINT   = 1 << 4;
        const MULTICAST     = 1 << 12;
        const PROMISC       = 1 << 8;
        const ALLMULTI       = 1 << 9;
        const RUNNING       = 1 << 6;
        const NOARP         = 1 << 7;
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct NetDeviceStats {
    pub rx_packets: u64,
    pub tx_packets: u64,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    pub rx_errors: u64,
    pub tx_errors: u64,
    pub rx_dropped: u64,
    pub tx_dropped: u64,
    pub multicast: u64,
    pub collisions: u64,
    pub rx_length_errors: u64,
    pub rx_over_errors: u64,
    pub rx_crc_errors: u64,
    pub rx_frame_errors: u64,
    pub rx_fifo_errors: u64,
    pub tx_aborted_errors: u64,
    pub tx_carrier_errors: u64,
    pub tx_fifo_errors: u64,
    pub tx_heartbeat_errors: u64,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct NetDeviceFeatures {
    pub checksum_offload: bool,
    pub scatter_gather: bool,
    pub tso: bool,
    pub gso: bool,
    pub gro: bool,
    pub vlan: bool,
    pub hw_timestamp: bool,
    pub rx_hash: bool,
    pub lro: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetDeviceState {
    Uninitialized,
    Registered,
    Up,
    Running,
    Dormant,
    Down,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetdevTxResult {
    Ok,
    Busy,
    Locked,
}

pub struct LinuxNetDevice {
    pub name: String,
    pub mac_address: [u8; 6],
    pub mtu: u32,
    pub flags: NetDeviceFlags,
    pub stats: NetDeviceStats,
    pub features: NetDeviceFeatures,
    pub state: NetDeviceState,
    pub tx_queue_len: u32,
    pub hard_header_len: u16,
    pub addr_len: u8,
    pub ifindex: u32,
    pub dev: LinuxDevice,
}

impl LinuxNetDevice {
    pub fn new(name: &str) -> Self {
        Self {
            name: String::from(name),
            mac_address: [0u8; 6],
            mtu: 1500,
            flags: NetDeviceFlags::BROADCAST | NetDeviceFlags::MULTICAST,
            stats: NetDeviceStats::default(),
            features: NetDeviceFeatures::default(),
            state: NetDeviceState::Uninitialized,
            tx_queue_len: 1000,
            hard_header_len: 14,
            addr_len: 6,
            ifindex: 0,
            dev: LinuxDevice::new(name, BusType::Virtual, 0),
        }
    }

    pub fn register(&mut self) -> i32 {
        static NEXT_IFINDEX: AtomicU64 = AtomicU64::new(1);
        self.ifindex = NEXT_IFINDEX.fetch_add(1, Ordering::Relaxed) as u32;
        self.state = NetDeviceState::Registered;
        printk(
            LogLevel::Info,
            &alloc::format!(
                "net: registered device '{}' ifindex={} mac={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
                self.name,
                self.ifindex,
                self.mac_address[0], self.mac_address[1], self.mac_address[2],
                self.mac_address[3], self.mac_address[4], self.mac_address[5],
            ),
        );
        0
    }

    pub fn unregister(&mut self) {
        self.state = NetDeviceState::Down;
        printk(
            LogLevel::Info,
            &alloc::format!("net: unregistered device '{}'", self.name),
        );
    }

    pub fn carrier_on(&mut self) {
        self.flags |= NetDeviceFlags::RUNNING;
        self.state = NetDeviceState::Running;
    }

    pub fn carrier_off(&mut self) {
        self.flags.remove(NetDeviceFlags::RUNNING);
        self.state = NetDeviceState::Down;
    }
}

pub trait LinuxNetDeviceOps: Send {
    fn ndo_open(&mut self) -> i32;
    fn ndo_stop(&mut self) -> i32;
    fn ndo_start_xmit(&mut self, skb: &SkBuff) -> NetdevTxResult;
    fn ndo_set_mac_address(&mut self, addr: &[u8; 6]) -> i32;
    fn ndo_get_stats(&self) -> &NetDeviceStats;
    fn ndo_set_rx_mode(&mut self);
    fn ndo_change_mtu(&mut self, new_mtu: u32) -> i32;
    fn ndo_validate_addr(&self) -> i32 {
        0
    }
    fn ndo_tx_timeout(&mut self) {}
}

// ────────────────────────────────────────────────────────────────────────────
// 4. SK_BUFF (Socket Buffer) Compatibility
// ────────────────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct SkBuff {
    pub data: Vec<u8>,
    pub head: usize,
    pub tail: usize,
    pub end: usize,
    pub len: u32,
    pub data_len: u32,
    pub protocol: u16,
    pub mac_header: usize,
    pub network_header: usize,
    pub transport_header: usize,
    pub priority: u32,
    pub mark: u32,
    pub queue_mapping: u16,
}

impl SkBuff {
    pub fn alloc_skb(size: usize) -> Option<Self> {
        if size == 0 {
            return None;
        }
        let mut data = Vec::with_capacity(size);
        data.resize(size, 0);
        Some(Self {
            data,
            head: 0,
            tail: 0,
            end: size,
            len: 0,
            data_len: 0,
            protocol: 0,
            mac_header: 0,
            network_header: 0,
            transport_header: 0,
            priority: 0,
            mark: 0,
            queue_mapping: 0,
        })
    }

    /// Reserve headroom at the front of the buffer. Must be called before any
    /// put/push operations.
    pub fn skb_reserve(&mut self, len: usize) {
        if self.tail + len <= self.end {
            self.head += len;
            self.tail += len;
        }
    }

    /// Append `len` bytes of payload space at the tail. Returns a mutable
    /// slice into the newly available region.
    pub fn skb_put(&mut self, len: usize) -> Option<&mut [u8]> {
        let new_tail = self.tail + len;
        if new_tail > self.end {
            return None;
        }
        let old_tail = self.tail;
        self.tail = new_tail;
        self.len += len as u32;
        Some(&mut self.data[old_tail..new_tail])
    }

    /// Prepend `len` bytes by moving the data pointer backward. Returns a
    /// mutable slice into the newly available header space.
    pub fn skb_push(&mut self, len: usize) -> Option<&mut [u8]> {
        if len > self.head {
            return None;
        }
        self.head -= len;
        self.len += len as u32;
        Some(&mut self.data[self.head..self.head + len])
    }

    /// Strip `len` bytes from the front by advancing the data pointer.
    /// Returns a slice to the removed region.
    pub fn skb_pull(&mut self, len: usize) -> Option<&[u8]> {
        if len as u32 > self.len {
            return None;
        }
        let old_head = self.head;
        self.head += len;
        self.len -= len as u32;
        Some(&self.data[old_head..old_head + len])
    }

    /// Returns the active payload region.
    pub fn payload(&self) -> &[u8] {
        &self.data[self.head..self.tail]
    }

    /// Full copy of this socket buffer.
    pub fn skb_copy(&self) -> Self {
        self.clone()
    }

    /// Shallow clone that shares the underlying allocation. Because we use
    /// `Vec` (not a refcounted pointer), this is effectively a full copy.
    pub fn skb_clone(&self) -> Self {
        self.clone()
    }

    pub fn headroom(&self) -> usize {
        self.head
    }

    pub fn tailroom(&self) -> usize {
        self.end - self.tail
    }

    pub fn set_mac_header(&mut self) {
        self.mac_header = self.head;
    }

    pub fn set_network_header(&mut self) {
        self.network_header = self.head;
    }

    pub fn set_transport_header(&mut self) {
        self.transport_header = self.head;
    }

    /// Construct an SKB from a raw Ethernet frame received from hardware.
    pub fn from_rx_frame(frame: &[u8], protocol: u16) -> Self {
        let mut skb = Self::alloc_skb(frame.len() + 64).unwrap();
        skb.skb_reserve(2); // align IP header to 16-byte boundary
        if let Some(buf) = skb.skb_put(frame.len()) {
            buf.copy_from_slice(frame);
        }
        skb.protocol = protocol;
        skb.set_mac_header();
        skb
    }
}

// ────────────────────────────────────────────────────────────────────────────
// 5. DMA Memory Management
// ────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DmaDirection {
    ToDevice,
    FromDevice,
    Bidirectional,
    None,
}

#[derive(Debug)]
pub struct DmaMapping {
    pub virt_addr: u64,
    pub phys_addr: u64,
    pub size: usize,
    pub direction: DmaDirection,
    pub coherent: bool,
    /// Buddy allocator order (`2^order` contiguous 4 KiB pages). `0` = one page.
    pub alloc_order: u8,
}

pub struct DmaPool {
    name: String,
    element_size: usize,
    alignment: usize,
    free_list: Vec<DmaMapping>,
    allocated: usize,
}

impl DmaPool {
    pub fn alloc(&mut self) -> Option<DmaMapping> {
        if let Some(mapping) = self.free_list.pop() {
            self.allocated += 1;
            Some(mapping)
        } else {
            match dma_alloc_coherent(self.element_size) {
                Ok(mapping) => {
                    self.allocated += 1;
                    Some(mapping)
                }
                Err(_) => None,
            }
        }
    }

    pub fn free(&mut self, mapping: DmaMapping) {
        if self.allocated > 0 {
            self.allocated -= 1;
        }
        self.free_list.push(mapping);
    }

    pub fn allocated_count(&self) -> usize {
        self.allocated
    }

    pub fn destroy(self) {
        printk(
            LogLevel::Debug,
            &alloc::format!(
                "dma: pool '{}' destroyed ({} still allocated)",
                self.name,
                self.allocated
            ),
        );
    }
}

fn buddy_order_for_pages(pages: usize) -> u8 {
    pages.next_power_of_two().trailing_zeros() as u8
}

/// Allocates physically contiguous DMA memory via the kernel frame allocator.
pub fn dma_alloc_coherent(size: usize) -> Result<DmaMapping, i32> {
    if size == 0 {
        return Err(-1); // -EINVAL
    }
    let aligned_size = (size + 0xFFF) & !0xFFF;
    let pages = aligned_size / 4096;
    let order = if pages <= 1 {
        0
    } else {
        buddy_order_for_pages(pages)
    };

    let phys = if order == 0 {
        let mut alloc = crate::memory::GlobalFrameAllocator;
        let frame = alloc.allocate_frame().ok_or(-12)?; // -ENOMEM
        frame.start_address().as_u64()
    } else {
        crate::memory::allocate_contiguous_frames(order)
            .ok_or(-12)?
            .as_u64()
    };

    let offset = crate::memory::PHYS_MEM_OFFSET
        .get()
        .copied()
        .ok_or(-5)?
        .as_u64(); // -EIO
    let virt = offset + phys;
    unsafe {
        core::ptr::write_bytes(virt as *mut u8, 0, aligned_size);
    }

    Ok(DmaMapping {
        virt_addr: virt,
        phys_addr: phys,
        size: aligned_size,
        direction: DmaDirection::Bidirectional,
        coherent: true,
        alloc_order: order,
    })
}

/// Release memory from [`dma_alloc_coherent`].
pub fn dma_free_coherent(mapping: DmaMapping) {
    if mapping.alloc_order == 0 {
        let frame = x86_64::structures::paging::PhysFrame::containing_address(PhysAddr::new(
            mapping.phys_addr,
        ));
        crate::memory::deallocate_frame(frame);
        return;
    }
    let mut buddy_lock = crate::memory::BUDDY_ALLOCATORS.lock();
    if let Some(buddy) = buddy_lock.get_mut(0) {
        unsafe {
            buddy.free_block(mapping.phys_addr, mapping.alloc_order);
        }
    }
}

/// Maps a virtual address into device-visible DMA space for streaming access.
pub fn dma_map_single(virt: u64, size: usize, dir: DmaDirection) -> Result<u64, i32> {
    if size == 0 {
        return Err(-1);
    }
    // In a real implementation this would program the IOMMU. For now we
    // return the virtual address as-is (identity mapping).
    let _ = dir;
    Ok(virt)
}

pub fn dma_unmap_single(_dma_addr: u64, _size: usize, _dir: DmaDirection) {
    // IOMMU teardown would go here.
}

pub fn dma_pool_create(name: &str, size: usize, align: usize) -> DmaPool {
    printk(
        LogLevel::Debug,
        &alloc::format!(
            "dma: pool '{}' created (elem_size={}, align={})",
            name,
            size,
            align
        ),
    );
    DmaPool {
        name: String::from(name),
        element_size: size,
        alignment: align,
        free_list: Vec::new(),
        allocated: 0,
    }
}

pub fn dma_sync_single_for_cpu(_dma_addr: u64, _size: usize, _dir: DmaDirection) {
    // Cache invalidation for non-coherent DMA.
}

pub fn dma_sync_single_for_device(_dma_addr: u64, _size: usize, _dir: DmaDirection) {
    // Cache writeback for non-coherent DMA.
}

// ────────────────────────────────────────────────────────────────────────────
// 6. Interrupt Management
// ────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IrqType {
    None,
    Edge,
    Level,
    MsiX,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IrqReturn {
    None,
    Handled,
    WakeThread,
}

pub const IRQF_SHARED: u32 = 1 << 0;
pub const IRQF_ONESHOT: u32 = 1 << 1;
pub const IRQF_NO_THREAD: u32 = 1 << 2;

pub struct IrqHandler {
    pub vector: u32,
    pub handler: fn(u32, *mut u8) -> IrqReturn,
    pub thread_fn: Option<fn(u32, *mut u8) -> IrqReturn>,
    pub dev_id: *mut u8,
    pub name: String,
    pub flags: u32,
    pub enabled: bool,
}

unsafe impl Send for IrqHandler {}
unsafe impl Sync for IrqHandler {}

static IRQ_HANDLERS: Mutex<BTreeMap<u32, Vec<IrqHandler>>> = Mutex::new(BTreeMap::new());

/// Register an interrupt handler, analogous to Linux `request_irq()`.
pub fn request_irq(
    irq: u32,
    handler: fn(u32, *mut u8) -> IrqReturn,
    flags: u32,
    name: &str,
    dev_id: *mut u8,
) -> i32 {
    let entry = IrqHandler {
        vector: irq,
        handler,
        thread_fn: None,
        dev_id,
        name: String::from(name),
        flags,
        enabled: true,
    };
    let mut map = IRQ_HANDLERS.lock();
    map.entry(irq).or_insert_with(Vec::new).push(entry);
    printk(
        LogLevel::Debug,
        &alloc::format!("irq: registered handler '{}' on vector {}", name, irq),
    );
    0
}

/// Register a threaded interrupt handler. The primary `handler` runs in hard
/// IRQ context; if it returns `WakeThread`, `thread_fn` runs in a schedulable
/// context.
pub fn request_threaded_irq(
    irq: u32,
    handler: fn(u32, *mut u8) -> IrqReturn,
    thread_fn: fn(u32, *mut u8) -> IrqReturn,
    flags: u32,
    name: &str,
    dev_id: *mut u8,
) -> i32 {
    let entry = IrqHandler {
        vector: irq,
        handler,
        thread_fn: Some(thread_fn),
        dev_id,
        name: String::from(name),
        flags,
        enabled: true,
    };
    let mut map = IRQ_HANDLERS.lock();
    map.entry(irq).or_insert_with(Vec::new).push(entry);
    printk(
        LogLevel::Debug,
        &alloc::format!(
            "irq: registered threaded handler '{}' on vector {}",
            name,
            irq
        ),
    );
    0
}

pub fn free_irq(irq: u32, dev_id: *mut u8) {
    let mut map = IRQ_HANDLERS.lock();
    if let Some(handlers) = map.get_mut(&irq) {
        handlers.retain(|h| h.dev_id != dev_id);
        if handlers.is_empty() {
            map.remove(&irq);
        }
    }
}

pub fn enable_irq(irq: u32) {
    let mut map = IRQ_HANDLERS.lock();
    if let Some(handlers) = map.get_mut(&irq) {
        for h in handlers.iter_mut() {
            h.enabled = true;
        }
    }
}

pub fn disable_irq(irq: u32) {
    let mut map = IRQ_HANDLERS.lock();
    if let Some(handlers) = map.get_mut(&irq) {
        for h in handlers.iter_mut() {
            h.enabled = false;
        }
    }
}

pub fn disable_irq_nosync(irq: u32) {
    disable_irq(irq);
}

/// Dispatch an IRQ to all registered handlers. Returns `true` if any handler
/// claimed the interrupt.
pub fn dispatch_irq(irq: u32) -> bool {
    let map = IRQ_HANDLERS.lock();
    let mut handled = false;
    if let Some(handlers) = map.get(&irq) {
        for h in handlers {
            if !h.enabled {
                continue;
            }
            let ret = (h.handler)(irq, h.dev_id);
            match ret {
                IrqReturn::Handled => {
                    handled = true;
                }
                IrqReturn::WakeThread => {
                    handled = true;
                    // A real implementation would wake the IRQ thread here.
                }
                IrqReturn::None => {}
            }
        }
    }
    handled
}

// ────────────────────────────────────────────────────────────────────────────
// 7. Workqueue / Tasklet System
// ────────────────────────────────────────────────────────────────────────────

pub struct WorkItem {
    pub handler: fn(*mut u8),
    pub data: *mut u8,
    pub pending: bool,
}

unsafe impl Send for WorkItem {}
unsafe impl Sync for WorkItem {}

pub struct Workqueue {
    pub name: String,
    items: VecDeque<WorkItem>,
    active: bool,
}

impl Workqueue {
    pub fn new(name: &str) -> Self {
        printk(
            LogLevel::Debug,
            &alloc::format!("workqueue: created '{}'", name),
        );
        Self {
            name: String::from(name),
            items: VecDeque::new(),
            active: true,
        }
    }

    pub fn queue_work(&mut self, work: WorkItem) {
        if self.active {
            self.items.push_back(work);
        }
    }

    pub fn drain(&mut self) {
        while let Some(item) = self.items.pop_front() {
            (item.handler)(item.data);
        }
    }

    pub fn pending_count(&self) -> usize {
        self.items.len()
    }

    pub fn destroy(mut self) {
        self.active = false;
        self.drain();
        printk(
            LogLevel::Debug,
            &alloc::format!("workqueue: destroyed '{}'", self.name),
        );
    }
}

pub fn schedule_work(wq: &mut Workqueue, work: WorkItem) {
    wq.queue_work(work);
}

pub fn flush_workqueue(wq: &mut Workqueue) {
    wq.drain();
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskletState {
    Idle,
    Scheduled,
    Running,
    Disabled,
}

pub struct Tasklet {
    pub handler: fn(u64),
    pub data: u64,
    pub state: TaskletState,
    pub count: u32,
}

impl Tasklet {
    pub fn new(handler: fn(u64), data: u64) -> Self {
        Self {
            handler,
            data,
            state: TaskletState::Idle,
            count: 0,
        }
    }

    pub fn disable(&mut self) {
        self.count += 1;
        self.state = TaskletState::Disabled;
    }

    pub fn enable(&mut self) {
        if self.count > 0 {
            self.count -= 1;
        }
        if self.count == 0 && self.state == TaskletState::Disabled {
            self.state = TaskletState::Idle;
        }
    }
}

pub fn tasklet_schedule(tasklet: &mut Tasklet) {
    if tasklet.state == TaskletState::Disabled {
        return;
    }
    tasklet.state = TaskletState::Scheduled;
}

pub fn tasklet_run(tasklet: &mut Tasklet) {
    if tasklet.state != TaskletState::Scheduled {
        return;
    }
    tasklet.state = TaskletState::Running;
    (tasklet.handler)(tasklet.data);
    tasklet.state = TaskletState::Idle;
}

pub fn tasklet_kill(tasklet: &mut Tasklet) {
    tasklet.state = TaskletState::Disabled;
    tasklet.count = u32::MAX;
}

// ────────────────────────────────────────────────────────────────────────────
// 8. Timer / Jiffies Compatibility
// ────────────────────────────────────────────────────────────────────────────

pub static JIFFIES: AtomicU64 = AtomicU64::new(0);

/// Linux-style HZ for `msecs_to_jiffies` helpers. Jiffies advance on each
/// LAPIC timer tick (100 Hz after APIC calibration = 10 ms/jiffy), not at 1 ms.
/// Was a false `1000`, which made `msecs_to_jiffies`/`usecs_to_jiffies` and the
/// `wait_event_timeout`/LinuxKPI `msleep` paths ~10× too long. Matches
/// `timers::HZ`.
pub const HZ: u64 = 100;

pub struct LinuxTimer {
    pub expires: u64,
    pub handler: fn(*mut u8),
    pub data: *mut u8,
    pub active: bool,
    pub name: &'static str,
}

unsafe impl Send for LinuxTimer {}
unsafe impl Sync for LinuxTimer {}

impl LinuxTimer {
    pub fn new(name: &'static str, handler: fn(*mut u8), data: *mut u8) -> Self {
        Self {
            expires: 0,
            handler,
            data,
            active: false,
            name,
        }
    }
}

pub fn jiffies_to_msecs(j: u64) -> u64 {
    j * 1000 / HZ
}

pub fn msecs_to_jiffies(ms: u64) -> u64 {
    ms * HZ / 1000
}

pub fn usecs_to_jiffies(us: u64) -> u64 {
    (us * HZ + 999_999) / 1_000_000
}

pub fn get_jiffies() -> u64 {
    JIFFIES.load(Ordering::Relaxed)
}

pub fn time_after(a: u64, b: u64) -> bool {
    (a as i64).wrapping_sub(b as i64) > 0
}

pub fn time_before(a: u64, b: u64) -> bool {
    time_after(b, a)
}

/// Arm or re-arm a timer. Returns `true` if the timer was already active.
pub fn mod_timer(timer: &mut LinuxTimer, expires: u64) -> bool {
    let was_active = timer.active;
    timer.expires = expires;
    timer.active = true;
    was_active
}

/// Cancel a pending timer. Returns `true` if it was active.
pub fn del_timer(timer: &mut LinuxTimer) -> bool {
    let was_active = timer.active;
    timer.active = false;
    was_active
}

pub fn timer_pending(timer: &LinuxTimer) -> bool {
    timer.active
}

/// Check whether a timer has expired and fire it if so. Called from the
/// RaeenOS tick handler.
pub fn check_timer(timer: &mut LinuxTimer) {
    if timer.active && time_after(get_jiffies(), timer.expires) {
        timer.active = false;
        (timer.handler)(timer.data);
    }
}

/// Increment the global jiffies counter. Should be called once per tick
/// (every 1 ms at HZ=1000).
pub fn tick_jiffies() {
    JIFFIES.fetch_add(1, Ordering::Relaxed);
}

// ────────────────────────────────────────────────────────────────────────────
// 9. Kernel Memory Allocation Compatibility
// ────────────────────────────────────────────────────────────────────────────

pub const GFP_KERNEL: u32 = 0x0;
pub const GFP_ATOMIC: u32 = 0x1;
pub const GFP_DMA: u32 = 0x2;
pub const GFP_NOIO: u32 = 0x4;
pub const GFP_NOWAIT: u32 = 0x8;
pub const GFP_HIGHUSER: u32 = 0x10;
pub const GFP_ZERO: u32 = 0x100;

const KMALLOC_HDR: usize = core::mem::size_of::<usize>();

/// Allocate `size` bytes of kernel memory. The `flags` parameter mirrors
/// Linux GFP flags but has no effect on the underlying allocator today.
pub fn kmalloc(size: usize, _flags: u32) -> *mut u8 {
    if size == 0 {
        return core::ptr::null_mut();
    }
    let layout = match size
        .checked_add(KMALLOC_HDR)
        .and_then(|t| core::alloc::Layout::from_size_align(t, 8).ok())
    {
        Some(l) => l,
        None => return core::ptr::null_mut(),
    };
    let raw = unsafe { alloc::alloc::alloc(layout) };
    if raw.is_null() {
        return core::ptr::null_mut();
    }
    unsafe {
        *(raw as *mut usize) = size;
    }
    unsafe { raw.add(KMALLOC_HDR) }
}

/// Like `kmalloc` but zeroes the returned memory.
pub fn kzalloc(size: usize, flags: u32) -> *mut u8 {
    let ptr = kmalloc(size, flags);
    if !ptr.is_null() {
        unsafe { core::ptr::write_bytes(ptr, 0, size) };
    }
    ptr
}

pub fn kfree(ptr: *mut u8) {
    if ptr.is_null() {
        return;
    }
    let raw = unsafe { ptr.sub(KMALLOC_HDR) };
    let size = unsafe { *(raw as *mut usize) };
    let layout = unsafe { core::alloc::Layout::from_size_align_unchecked(size + KMALLOC_HDR, 8) };
    unsafe { alloc::alloc::dealloc(raw, layout) };
}

/// Allocate virtually-contiguous (but not necessarily physically-contiguous)
/// kernel memory. Falls back to the regular heap in this stub.
pub fn vmalloc(size: usize) -> *mut u8 {
    kmalloc(size, GFP_KERNEL)
}

pub fn vfree(ptr: *mut u8) {
    kfree(ptr);
}

pub fn krealloc(ptr: *mut u8, new_size: usize, flags: u32) -> *mut u8 {
    if ptr.is_null() {
        return kmalloc(new_size, flags);
    }
    if new_size == 0 {
        kfree(ptr);
        return core::ptr::null_mut();
    }
    let old_size = unsafe { *((ptr.sub(KMALLOC_HDR)) as *mut usize) };
    let new_ptr = kmalloc(new_size, flags);
    if !new_ptr.is_null() {
        let copy_len = old_size.min(new_size);
        unsafe { core::ptr::copy_nonoverlapping(ptr, new_ptr, copy_len) };
        kfree(ptr);
    }
    new_ptr
}

pub fn kcalloc(count: usize, size: usize, flags: u32) -> *mut u8 {
    let total = count.checked_mul(size).unwrap_or(0);
    kzalloc(total, flags)
}

// ────────────────────────────────────────────────────────────────────────────
// 10. Firmware Loading
// ────────────────────────────────────────────────────────────────────────────

pub struct Firmware {
    pub data: Vec<u8>,
    pub size: usize,
    pub name: String,
}

/// Request a firmware blob by name. In a full implementation this would
/// search the initramfs, a firmware cache, or a user-space helper. The stub
/// always returns `Err(-ENOENT)`.
pub fn request_firmware(name: &str) -> Result<Firmware, i32> {
    printk(
        LogLevel::Info,
        &alloc::format!("firmware: request '{}'", name),
    );

    // Try the initramfs first.
    let archive = crate::tar::TarArchive::new(crate::INITRAMFS);
    let path = alloc::format!("firmware/{}", name);
    if let Some(entry) = archive.get_file(&path) {
        let data = entry.data.to_vec();
        let size = data.len();
        printk(
            LogLevel::Info,
            &alloc::format!(
                "firmware: loaded '{}' ({} bytes) from initramfs",
                name,
                size
            ),
        );
        return Ok(Firmware {
            data,
            size,
            name: String::from(name),
        });
    }

    printk(
        LogLevel::Warning,
        &alloc::format!("firmware: '{}' not found", name),
    );
    Err(-2) // -ENOENT
}

pub fn release_firmware(fw: Firmware) {
    printk(
        LogLevel::Debug,
        &alloc::format!("firmware: released '{}' ({} bytes)", fw.name, fw.size),
    );
    drop(fw);
}

// ────────────────────────────────────────────────────────────────────────────
// 11. Kernel Logging (printk compat)
// ────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum LogLevel {
    Emergency = 0,
    Alert = 1,
    Critical = 2,
    Error = 3,
    Warning = 4,
    Notice = 5,
    Info = 6,
    Debug = 7,
}

static LOG_LEVEL: AtomicU64 = AtomicU64::new(LogLevel::Info as u64);

pub fn set_log_level(level: LogLevel) {
    LOG_LEVEL.store(level as u64, Ordering::Relaxed);
}

pub fn printk(level: LogLevel, msg: &str) {
    let current_level = LOG_LEVEL.load(Ordering::Relaxed);
    if (level as u64) > current_level {
        return;
    }
    let prefix = match level {
        LogLevel::Emergency => "EMERG",
        LogLevel::Alert => "ALERT",
        LogLevel::Critical => "CRIT ",
        LogLevel::Error => "ERROR",
        LogLevel::Warning => "WARN ",
        LogLevel::Notice => "NOTE ",
        LogLevel::Info => "INFO ",
        LogLevel::Debug => "DEBUG",
    };
    crate::serial_println!("[linux_compat][{}] {}", prefix, msg);
}

pub fn dev_err(dev: &LinuxDevice, msg: &str) {
    crate::serial_println!("[linux_compat][ERROR] {}: {}", dev.name, msg);
}

pub fn dev_warn(dev: &LinuxDevice, msg: &str) {
    crate::serial_println!("[linux_compat][WARN ] {}: {}", dev.name, msg);
}

pub fn dev_info(dev: &LinuxDevice, msg: &str) {
    crate::serial_println!("[linux_compat][INFO ] {}: {}", dev.name, msg);
}

pub fn dev_dbg(dev: &LinuxDevice, msg: &str) {
    let current_level = LOG_LEVEL.load(Ordering::Relaxed);
    if (LogLevel::Debug as u64) <= current_level {
        crate::serial_println!("[linux_compat][DEBUG] {}: {}", dev.name, msg);
    }
}

// ────────────────────────────────────────────────────────────────────────────
// 12. Driver Registration Registry
// ────────────────────────────────────────────────────────────────────────────

pub struct DriverRegistry {
    pub pci_drivers: Vec<LinuxPciDriver>,
    pub net_drivers: Vec<(String, Box<dyn LinuxNetDeviceOps + Send>)>,
    pub usb_drivers: Vec<LinuxUsbDriver>,
    pub registered_devices: BTreeMap<u64, LinuxDevice>,
    next_device_id: u64,
}

impl DriverRegistry {
    pub const fn new() -> Self {
        Self {
            pci_drivers: Vec::new(),
            net_drivers: Vec::new(),
            usb_drivers: Vec::new(),
            registered_devices: BTreeMap::new(),
            next_device_id: 1,
        }
    }

    pub fn register_pci_driver(&mut self, driver: LinuxPciDriver) -> i32 {
        printk(
            LogLevel::Info,
            &alloc::format!("driver: registered PCI driver '{}'", driver.name),
        );
        self.pci_drivers.push(driver);
        0
    }

    pub fn unregister_pci_driver(&mut self, name: &str) {
        self.pci_drivers.retain(|d| d.name != name);
        printk(
            LogLevel::Info,
            &alloc::format!("driver: unregistered PCI driver '{}'", name),
        );
    }

    pub fn register_net_driver(
        &mut self,
        name: &str,
        ops: Box<dyn LinuxNetDeviceOps + Send>,
    ) -> i32 {
        printk(
            LogLevel::Info,
            &alloc::format!("driver: registered net driver '{}'", name),
        );
        self.net_drivers.push((String::from(name), ops));
        0
    }

    pub fn unregister_net_driver(&mut self, name: &str) {
        self.net_drivers.retain(|(n, _)| n != name);
        printk(
            LogLevel::Info,
            &alloc::format!("driver: unregistered net driver '{}'", name),
        );
    }

    pub fn register_usb_driver(&mut self, driver: LinuxUsbDriver) -> i32 {
        printk(
            LogLevel::Info,
            &alloc::format!("driver: registered USB driver '{}'", driver.name),
        );
        self.usb_drivers.push(driver);
        0
    }

    pub fn unregister_usb_driver(&mut self, name: &str) {
        self.usb_drivers.retain(|d| d.name != name);
        printk(
            LogLevel::Info,
            &alloc::format!("driver: unregistered USB driver '{}'", name),
        );
    }

    pub fn add_device(&mut self, dev: LinuxDevice) -> u64 {
        let id = self.next_device_id;
        self.next_device_id += 1;
        self.registered_devices.insert(id, dev);
        id
    }

    pub fn remove_device(&mut self, id: u64) -> Option<LinuxDevice> {
        self.registered_devices.remove(&id)
    }

    /// Try to match a PCI device against all registered PCI drivers and call
    /// the matching driver's `probe` function.
    pub fn probe_pci_device(&self, pci_dev: &LinuxPciDevice) -> i32 {
        for driver in &self.pci_drivers {
            if driver.match_device(pci_dev.vendor as u32, pci_dev.device as u32, pci_dev.class) {
                printk(
                    LogLevel::Info,
                    &alloc::format!(
                        "driver: PCI {:04x}:{:04x} matched '{}'",
                        pci_dev.vendor,
                        pci_dev.device,
                        driver.name,
                    ),
                );
                if let Some(probe) = driver.probe {
                    return probe(pci_dev);
                }
            }
        }
        -19 // -ENODEV
    }

    /// Try to match a USB interface against all registered USB drivers.
    pub fn probe_usb_device(&self, iface: &LinuxUsbInterface) -> i32 {
        for driver in &self.usb_drivers {
            for id in &driver.id_table {
                if id.matches_interface(iface) {
                    printk(
                        LogLevel::Info,
                        &alloc::format!(
                            "driver: USB {:04x}:{:04x} matched '{}'",
                            iface.vendor_id,
                            iface.product_id,
                            driver.name,
                        ),
                    );
                    if let Some(probe) = driver.probe {
                        return probe(iface);
                    }
                }
            }
        }
        -19 // -ENODEV
    }

    /// Probe all registered devices against all drivers.
    pub fn probe_all_devices(&self) {
        printk(
            LogLevel::Info,
            &alloc::format!(
                "driver: probing {} registered device(s) against {} PCI + {} USB driver(s)",
                self.registered_devices.len(),
                self.pci_drivers.len(),
                self.usb_drivers.len(),
            ),
        );
    }

    /// Search for a PCI driver by name.
    pub fn find_pci_driver(&self, name: &str) -> Option<&LinuxPciDriver> {
        self.pci_drivers.iter().find(|d| d.name == name)
    }

    pub fn pci_driver_count(&self) -> usize {
        self.pci_drivers.len()
    }

    pub fn usb_driver_count(&self) -> usize {
        self.usb_drivers.len()
    }

    pub fn net_driver_count(&self) -> usize {
        self.net_drivers.len()
    }
}

pub static DRIVER_REGISTRY: Mutex<DriverRegistry> = Mutex::new(DriverRegistry::new());

pub fn register_pci_driver(driver: LinuxPciDriver) -> i32 {
    DRIVER_REGISTRY.lock().register_pci_driver(driver)
}

pub fn unregister_pci_driver(name: &str) {
    DRIVER_REGISTRY.lock().unregister_pci_driver(name);
}

pub fn register_net_driver(name: &str, ops: Box<dyn LinuxNetDeviceOps + Send>) -> i32 {
    DRIVER_REGISTRY.lock().register_net_driver(name, ops)
}

pub fn unregister_net_driver(name: &str) {
    DRIVER_REGISTRY.lock().unregister_net_driver(name);
}

// ────────────────────────────────────────────────────────────────────────────
// 13. USB Driver Compatibility
// ────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinuxUsbDeviceId {
    pub vendor: u16,
    pub product: u16,
    pub class: u8,
    pub subclass: u8,
    pub protocol: u8,
}

impl LinuxUsbDeviceId {
    pub const ANY_VENDOR: u16 = 0xFFFF;
    pub const ANY_PRODUCT: u16 = 0xFFFF;
    pub const ANY_CLASS: u8 = 0xFF;

    pub fn matches_interface(&self, iface: &LinuxUsbInterface) -> bool {
        let vendor_ok = self.vendor == Self::ANY_VENDOR || self.vendor == iface.vendor_id;
        let product_ok = self.product == Self::ANY_PRODUCT || self.product == iface.product_id;
        let class_ok = self.class == Self::ANY_CLASS || self.class == iface.class;
        vendor_ok && product_ok && class_ok
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsbTransferType {
    Control,
    Bulk,
    Interrupt,
    Isochronous,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsbDirection {
    In,
    Out,
}

#[derive(Debug, Clone)]
pub struct LinuxUsbEndpoint {
    pub address: u8,
    pub direction: UsbDirection,
    pub transfer_type: UsbTransferType,
    pub max_packet_size: u16,
    pub interval: u8,
}

#[derive(Debug, Clone)]
pub struct LinuxUsbInterface {
    pub vendor_id: u16,
    pub product_id: u16,
    pub interface_number: u8,
    pub alternate_setting: u8,
    pub class: u8,
    pub subclass: u8,
    pub protocol: u8,
    pub endpoints: Vec<LinuxUsbEndpoint>,
    pub dev: LinuxDevice,
}

impl LinuxUsbInterface {
    pub fn find_endpoint(
        &self,
        transfer_type: UsbTransferType,
        direction: UsbDirection,
    ) -> Option<&LinuxUsbEndpoint> {
        self.endpoints
            .iter()
            .find(|ep| ep.transfer_type == transfer_type && ep.direction == direction)
    }

    pub fn find_bulk_in(&self) -> Option<&LinuxUsbEndpoint> {
        self.find_endpoint(UsbTransferType::Bulk, UsbDirection::In)
    }

    pub fn find_bulk_out(&self) -> Option<&LinuxUsbEndpoint> {
        self.find_endpoint(UsbTransferType::Bulk, UsbDirection::Out)
    }

    pub fn find_interrupt_in(&self) -> Option<&LinuxUsbEndpoint> {
        self.find_endpoint(UsbTransferType::Interrupt, UsbDirection::In)
    }
}

pub struct LinuxUsbDriver {
    pub name: &'static str,
    pub id_table: Vec<LinuxUsbDeviceId>,
    pub probe: Option<fn(&LinuxUsbInterface) -> i32>,
    pub disconnect: Option<fn(&LinuxUsbInterface)>,
    pub suspend: Option<fn(&LinuxUsbInterface) -> i32>,
    pub resume: Option<fn(&LinuxUsbInterface) -> i32>,
}

pub fn usb_register_driver(driver: LinuxUsbDriver) -> i32 {
    DRIVER_REGISTRY.lock().register_usb_driver(driver)
}

pub fn usb_deregister_driver(name: &str) {
    DRIVER_REGISTRY.lock().unregister_usb_driver(name);
}

// ── URB (USB Request Block) ─────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UrbStatus {
    Pending,
    Completed,
    Error(i32),
    Cancelled,
}

pub struct Urb {
    pub endpoint: u8,
    pub transfer_type: UsbTransferType,
    pub direction: UsbDirection,
    pub buffer: Vec<u8>,
    pub actual_length: usize,
    pub status: UrbStatus,
    pub complete: Option<fn(&mut Urb)>,
    pub context: *mut u8,
    pub interval: u32,
}

unsafe impl Send for Urb {}
unsafe impl Sync for Urb {}

impl Urb {
    pub fn new(
        endpoint: u8,
        transfer_type: UsbTransferType,
        direction: UsbDirection,
        buf_size: usize,
    ) -> Self {
        Self {
            endpoint,
            transfer_type,
            direction,
            buffer: alloc::vec![0u8; buf_size],
            actual_length: 0,
            status: UrbStatus::Pending,
            complete: None,
            context: core::ptr::null_mut(),
            interval: 0,
        }
    }

    pub fn submit(&mut self) -> i32 {
        self.status = UrbStatus::Pending;
        0
    }

    pub fn cancel(&mut self) {
        self.status = UrbStatus::Cancelled;
    }

    pub fn complete_urb(&mut self, actual_length: usize, status: UrbStatus) {
        self.actual_length = actual_length;
        self.status = status;
        if let Some(cb) = self.complete {
            cb(self);
        }
    }
}

// ────────────────────────────────────────────────────────────────────────────
// 14. Wait Queues
// ────────────────────────────────────────────────────────────────────────────

pub struct WaitQueueHead {
    waiters: AtomicU64,
    condition: AtomicU64,
}

impl WaitQueueHead {
    pub const fn new() -> Self {
        Self {
            waiters: AtomicU64::new(0),
            condition: AtomicU64::new(0),
        }
    }

    pub fn wake_up(&self) {
        self.condition.store(1, Ordering::Release);
    }

    pub fn wake_up_all(&self) {
        self.condition.store(1, Ordering::Release);
    }

    pub fn wait_event(&self) {
        self.waiters.fetch_add(1, Ordering::Relaxed);
        while self.condition.load(Ordering::Acquire) == 0 {
            core::hint::spin_loop();
        }
        self.waiters.fetch_sub(1, Ordering::Relaxed);
        self.condition.store(0, Ordering::Relaxed);
    }

    pub fn wait_event_timeout(&self, timeout_jiffies: u64) -> bool {
        self.waiters.fetch_add(1, Ordering::Relaxed);
        let deadline = get_jiffies() + timeout_jiffies;
        let mut timed_out = false;
        while self.condition.load(Ordering::Acquire) == 0 {
            if time_after(get_jiffies(), deadline) {
                timed_out = true;
                break;
            }
            core::hint::spin_loop();
        }
        self.waiters.fetch_sub(1, Ordering::Relaxed);
        if !timed_out {
            self.condition.store(0, Ordering::Relaxed);
        }
        !timed_out
    }

    pub fn waiter_count(&self) -> u64 {
        self.waiters.load(Ordering::Relaxed)
    }
}

// ────────────────────────────────────────────────────────────────────────────
// 15. Completion
// ────────────────────────────────────────────────────────────────────────────

pub struct Completion {
    done: AtomicU64,
}

impl Completion {
    pub const fn new() -> Self {
        Self {
            done: AtomicU64::new(0),
        }
    }

    pub fn wait_for_completion(&self) {
        while self.done.load(Ordering::Acquire) == 0 {
            core::hint::spin_loop();
        }
    }

    pub fn wait_for_completion_timeout(&self, timeout_jiffies: u64) -> bool {
        let deadline = get_jiffies() + timeout_jiffies;
        while self.done.load(Ordering::Acquire) == 0 {
            if time_after(get_jiffies(), deadline) {
                return false;
            }
            core::hint::spin_loop();
        }
        true
    }

    pub fn complete(&self) {
        self.done.store(1, Ordering::Release);
    }

    pub fn complete_all(&self) {
        self.done.store(u64::MAX, Ordering::Release);
    }

    pub fn reinit(&self) {
        self.done.store(0, Ordering::Relaxed);
    }

    pub fn is_done(&self) -> bool {
        self.done.load(Ordering::Relaxed) != 0
    }
}

// ────────────────────────────────────────────────────────────────────────────
// 16. Mutex / Spinlock wrappers (Linux API surface)
// ────────────────────────────────────────────────────────────────────────────

/// A thin wrapper around `spin::Mutex` that presents the Linux `mutex_lock` /
/// `mutex_unlock` API.
pub struct LinuxMutex<T> {
    inner: Mutex<T>,
}

impl<T> LinuxMutex<T> {
    pub const fn new(val: T) -> Self {
        Self {
            inner: Mutex::new(val),
        }
    }

    pub fn lock(&self) -> spin::MutexGuard<T> {
        self.inner.lock()
    }

    pub fn try_lock(&self) -> Option<spin::MutexGuard<T>> {
        self.inner.try_lock()
    }
}

/// Reader-writer spinlock wrapper (uses `spin::RwLock` underneath).
pub struct LinuxRwLock<T> {
    inner: spin::RwLock<T>,
}

impl<T> LinuxRwLock<T> {
    pub const fn new(val: T) -> Self {
        Self {
            inner: spin::RwLock::new(val),
        }
    }

    pub fn read(&self) -> spin::RwLockReadGuard<T> {
        self.inner.read()
    }

    pub fn write(&self) -> spin::RwLockWriteGuard<T> {
        self.inner.write()
    }
}

// ────────────────────────────────────────────────────────────────────────────
// 17. MMIO helpers
// ────────────────────────────────────────────────────────────────────────────

/// Read a 32-bit value from a memory-mapped I/O register.
#[inline(always)]
pub unsafe fn ioread32(addr: u64) -> u32 {
    core::ptr::read_volatile(addr as *const u32)
}

/// Write a 32-bit value to a memory-mapped I/O register.
#[inline(always)]
pub unsafe fn iowrite32(val: u32, addr: u64) {
    core::ptr::write_volatile(addr as *mut u32, val);
}

#[inline(always)]
pub unsafe fn ioread16(addr: u64) -> u16 {
    core::ptr::read_volatile(addr as *const u16)
}

#[inline(always)]
pub unsafe fn iowrite16(val: u16, addr: u64) {
    core::ptr::write_volatile(addr as *mut u16, val);
}

#[inline(always)]
pub unsafe fn ioread8(addr: u64) -> u8 {
    core::ptr::read_volatile(addr as *const u8)
}

#[inline(always)]
pub unsafe fn iowrite8(val: u8, addr: u64) {
    core::ptr::write_volatile(addr as *mut u8, val);
}

/// Enforce an MMIO ordering barrier.
#[inline(always)]
pub fn io_mb() {
    core::sync::atomic::fence(Ordering::SeqCst);
}

// ────────────────────────────────────────────────────────────────────────────
// 18. Error codes (subset of Linux errno values used by drivers)
// ────────────────────────────────────────────────────────────────────────────

pub const EPERM: i32 = -1;
pub const ENOENT: i32 = -2;
pub const EIO: i32 = -5;
pub const ENOMEM: i32 = -12;
pub const EBUSY: i32 = -16;
pub const ENODEV: i32 = -19;
pub const EINVAL: i32 = -22;
pub const ENOSPC: i32 = -28;
pub const ENOSYS: i32 = -38;
pub const ETIMEDOUT: i32 = -110;
pub const EOPNOTSUPP: i32 = -95;

pub fn is_err(code: i32) -> bool {
    code < 0
}

pub fn err_name(code: i32) -> &'static str {
    match code {
        0 => "OK",
        -1 => "EPERM",
        -2 => "ENOENT",
        -5 => "EIO",
        -12 => "ENOMEM",
        -16 => "EBUSY",
        -19 => "ENODEV",
        -22 => "EINVAL",
        -28 => "ENOSPC",
        -38 => "ENOSYS",
        -95 => "EOPNOTSUPP",
        -110 => "ETIMEDOUT",
        _ => "UNKNOWN",
    }
}

// ────────────────────────────────────────────────────────────────────────────
// 19. Notifier chains (driver event bus)
// ────────────────────────────────────────────────────────────────────────────

pub type NotifierCallback = fn(event: u64, data: *mut u8) -> i32;

pub struct NotifierBlock {
    pub callback: NotifierCallback,
    pub priority: i32,
}

pub struct NotifierChain {
    blocks: Vec<NotifierBlock>,
}

impl NotifierChain {
    pub const fn new() -> Self {
        Self { blocks: Vec::new() }
    }

    pub fn register(&mut self, block: NotifierBlock) {
        let pos = self
            .blocks
            .iter()
            .position(|b| b.priority < block.priority)
            .unwrap_or(self.blocks.len());
        self.blocks.insert(pos, block);
    }

    pub fn unregister(&mut self, callback: NotifierCallback) {
        self.blocks
            .retain(|b| b.callback as usize != callback as usize);
    }

    pub fn call_chain(&self, event: u64, data: *mut u8) -> i32 {
        let mut ret = 0;
        for block in &self.blocks {
            ret = (block.callback)(event, data);
            if ret != 0 {
                break;
            }
        }
        ret
    }
}

// ────────────────────────────────────────────────────────────────────────────
// 20. Module initialization entry point
// ────────────────────────────────────────────────────────────────────────────

static IRQ_SMOKETEST_FIRED: AtomicBool = AtomicBool::new(false);

fn smoketest_irq_handler(_irq: u32, _dev_id: *mut u8) -> IrqReturn {
    IRQ_SMOKETEST_FIRED.store(true, Ordering::Relaxed);
    IrqReturn::Handled
}

/// Called from the LAPIC timer path (~100 Hz). Advances jiffies and could
/// drain deferred work in a future slice.
pub fn on_timer_tick() {
    tick_jiffies();
}

/// Boot-time exercises for kmalloc, DMA, device registry, IRQ dispatch, workqueue.
pub fn run_boot_smoketest() {
    let mut ok = 0u32;
    let mut fail = 0u32;

    let ptr = kzalloc(32, GFP_KERNEL);
    if ptr.is_null() {
        fail += 1;
    } else {
        let first = unsafe { *ptr };
        kfree(ptr);
        if first == 0 {
            ok += 1;
        } else {
            fail += 1;
        }
    }

    match dma_alloc_coherent(4096) {
        Ok(mapping) => {
            let offset = crate::memory::PHYS_MEM_OFFSET.get().copied();
            let virt_ok = offset.map_or(false, |o| {
                mapping.virt_addr == o.as_u64() + mapping.phys_addr
            });
            let phys_ok = mapping.phys_addr != 0 && mapping.size >= 4096;
            dma_free_coherent(mapping);
            if virt_ok && phys_ok {
                ok += 1;
            } else {
                fail += 1;
            }
        }
        Err(_) => fail += 1,
    }

    let dev = LinuxDevice::new("linux_compat-smoke", BusType::Virtual, 0);
    let id = DEVICE_REGISTRY.lock().register(dev);
    if id > 0 {
        ok += 1;
    } else {
        fail += 1;
    }

    IRQ_SMOKETEST_FIRED.store(false, Ordering::Relaxed);
    let irq = 0xDEAD_BEEF_u32;
    if request_irq(
        irq,
        smoketest_irq_handler,
        0,
        "linux_compat-smoke",
        core::ptr::null_mut(),
    ) == 0
        && dispatch_irq(irq)
        && IRQ_SMOKETEST_FIRED.load(Ordering::Relaxed)
    {
        ok += 1;
    } else {
        fail += 1;
    }
    free_irq(irq, core::ptr::null_mut());

    let mut wq = Workqueue::new("linux_compat-smoke");
    static WQ_DONE: AtomicBool = AtomicBool::new(false);
    fn wq_handler(_data: *mut u8) {
        WQ_DONE.store(true, Ordering::Relaxed);
    }
    WQ_DONE.store(false, Ordering::Relaxed);
    wq.queue_work(WorkItem {
        handler: wq_handler,
        data: core::ptr::null_mut(),
        pending: true,
    });
    wq.drain();
    if WQ_DONE.load(Ordering::Relaxed) {
        ok += 1;
    } else {
        fail += 1;
    }

    on_timer_tick();
    let j = get_jiffies();
    if j > 0 {
        ok += 1;
    } else {
        fail += 1;
    }

    if fail == 0 {
        crate::serial_println!(
            "[linux_compat] smoketest PASS: {} checks (jiffies={}, devices={})",
            ok,
            j,
            DEVICE_REGISTRY.lock().count(),
        );
    } else {
        crate::serial_println!(
            "[linux_compat] smoketest FAIL: {} ok {} fail (jiffies={})",
            ok,
            fail,
            j,
        );
    }
}

/// `/proc/raeen/linux_compat` — layer status (not GPL Linux code).
pub fn dump_text() -> String {
    let devs = DEVICE_REGISTRY.lock().count();
    let drivers = DRIVER_REGISTRY.lock();
    let pci = drivers.pci_drivers.len();
    let net = drivers.net_drivers.len();
    let usb = drivers.usb_drivers.len();
    let irq_vecs = IRQ_HANDLERS.lock().len();
    let jiffies = get_jiffies();

    let mut out = String::from("# RaeenOS Linux driver compat layer\n");
    out.push_str("# Shims for userspace LinuxKPI hosting — see docs/LINUX_DRIVER_STRATEGY.md\n");
    out.push_str(&alloc::format!("jiffies: {jiffies}\n"));
    out.push_str(&alloc::format!(
        "HZ_constant: {HZ} (helpers only; tick ~100 Hz via LAPIC)\n"
    ));
    out.push_str(&alloc::format!("registered_devices: {devs}\n"));
    out.push_str(&alloc::format!("pci_drivers: {pci}\n"));
    out.push_str(&alloc::format!("net_drivers: {net}\n"));
    out.push_str(&alloc::format!("usb_drivers: {usb}\n"));
    out.push_str(&alloc::format!("irq_handler_vectors: {irq_vecs}\n"));
    out.push_str("dma: frame_allocator (coherent stub, no IOMMU yet)\n");
    out.push_str("status: init+smoketest wired on boot path\n");
    out
}

/// Initialize the Linux compatibility layer. Called during kernel boot.
pub fn init() {
    printk(
        LogLevel::Info,
        "Linux driver compatibility layer initializing",
    );
    printk(
        LogLevel::Info,
        &alloc::format!(
            "  jiffies helpers HZ={}, log_level={:?}",
            HZ,
            LogLevel::Info
        ),
    );
    printk(LogLevel::Info, "Linux driver compatibility layer ready");
    crate::serial_println!("[ OK ] Linux driver compat layer (shim, not GPL kernel)");
}
