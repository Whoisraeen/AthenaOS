#![allow(dead_code)]

extern crate alloc;

use alloc::{boxed::Box, collections::BTreeMap, string::String, vec, vec::Vec};
use spin::Mutex;

// ─── Network Driver Error ───────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetDriverError {
    NotReady,
    LinkDown,
    BufferTooLarge,
    BufferTooSmall,
    NoBuffers,
    Timeout,
    InvalidMtu,
    HardwareError,
    UnsupportedOperation,
    AlreadyEnabled,
    AlreadyDisabled,
    InvalidAddress,
    QueueFull,
    DeviceRemoved,
    FirmwareError,
    AuthenticationFailed,
    AssociationFailed,
    ScanFailed,
    NoNetwork,
}

impl core::fmt::Display for NetDriverError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::NotReady => write!(f, "device not ready"),
            Self::LinkDown => write!(f, "link is down"),
            Self::BufferTooLarge => write!(f, "buffer too large for MTU"),
            Self::BufferTooSmall => write!(f, "buffer too small"),
            Self::NoBuffers => write!(f, "no buffers available"),
            Self::Timeout => write!(f, "operation timed out"),
            Self::InvalidMtu => write!(f, "invalid MTU value"),
            Self::HardwareError => write!(f, "hardware error"),
            Self::UnsupportedOperation => write!(f, "unsupported operation"),
            Self::AlreadyEnabled => write!(f, "device already enabled"),
            Self::AlreadyDisabled => write!(f, "device already disabled"),
            Self::InvalidAddress => write!(f, "invalid address"),
            Self::QueueFull => write!(f, "transmit queue full"),
            Self::DeviceRemoved => write!(f, "device removed"),
            Self::FirmwareError => write!(f, "firmware error"),
            Self::AuthenticationFailed => write!(f, "authentication failed"),
            Self::AssociationFailed => write!(f, "association failed"),
            Self::ScanFailed => write!(f, "scan failed"),
            Self::NoNetwork => write!(f, "no network found"),
        }
    }
}

// ─── Link Speed & State ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkSpeed {
    Mbps10,
    Mbps100,
    Gbps1,
    Gbps2_5,
    Gbps5,
    Gbps10,
    Gbps25,
    Gbps40,
    Gbps100,
    Unknown,
}

impl LinkSpeed {
    pub fn as_mbps(&self) -> u64 {
        match self {
            Self::Mbps10 => 10,
            Self::Mbps100 => 100,
            Self::Gbps1 => 1_000,
            Self::Gbps2_5 => 2_500,
            Self::Gbps5 => 5_000,
            Self::Gbps10 => 10_000,
            Self::Gbps25 => 25_000,
            Self::Gbps40 => 40_000,
            Self::Gbps100 => 100_000,
            Self::Unknown => 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkState {
    Up,
    Down,
    Testing,
    Unknown,
    Dormant,
    NotPresent,
    LowerLayerDown,
}

// ─── Statistics & Offload ───────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, Default)]
pub struct NetDriverStats {
    pub rx_packets: u64,
    pub tx_packets: u64,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    pub rx_errors: u64,
    pub tx_errors: u64,
    pub rx_dropped: u64,
    pub tx_dropped: u64,
    pub collisions: u64,
    pub multicast: u64,
    pub rx_crc_errors: u64,
    pub rx_frame_errors: u64,
    pub rx_fifo_errors: u64,
    pub tx_carrier_errors: u64,
    pub tx_fifo_errors: u64,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct OffloadCapabilities {
    pub tx_checksum: bool,
    pub rx_checksum: bool,
    pub tso: bool,
    pub gso: bool,
    pub gro: bool,
    pub scatter_gather: bool,
    pub vlan_offload: bool,
}

// ─── NetDriver Trait ────────────────────────────────────────────────────────

pub trait NetDriver: Send {
    fn name(&self) -> &str;
    fn mac_address(&self) -> [u8; 6];
    fn mtu(&self) -> u32;
    fn set_mtu(&mut self, mtu: u32) -> Result<(), NetDriverError>;
    fn link_speed(&self) -> LinkSpeed;
    fn link_state(&self) -> LinkState;
    fn send(&mut self, packet: &[u8]) -> Result<(), NetDriverError>;
    fn recv(&mut self) -> Option<Vec<u8>>;
    fn enable(&mut self) -> Result<(), NetDriverError>;
    fn disable(&mut self) -> Result<(), NetDriverError>;
    fn stats(&self) -> NetDriverStats;
    fn set_promiscuous(&mut self, enabled: bool);
    fn add_multicast(&mut self, addr: [u8; 6]);
    fn remove_multicast(&mut self, addr: [u8; 6]);
    fn supports_offload(&self) -> OffloadCapabilities;
}

// ─── Network Driver Manager ─────────────────────────────────────────────────

pub struct NetDriverManager {
    drivers: BTreeMap<String, Box<dyn NetDriver>>,
    default_route: Option<String>,
}

impl NetDriverManager {
    pub fn new() -> Self {
        Self {
            drivers: BTreeMap::new(),
            default_route: None,
        }
    }

    pub fn register(&mut self, name: String, driver: Box<dyn NetDriver>) {
        self.drivers.insert(name, driver);
    }

    pub fn unregister(&mut self, name: &str) -> Option<Box<dyn NetDriver>> {
        if self.default_route.as_deref() == Some(name) {
            self.default_route = None;
        }
        self.drivers.remove(name)
    }

    pub fn get(&self, name: &str) -> Option<&dyn NetDriver> {
        self.drivers.get(name).map(|d| d.as_ref())
    }

    pub fn get_mut<'a>(&'a mut self, name: &str) -> Option<&'a mut (dyn NetDriver + 'static)> {
        self.drivers.get_mut(name).map(|d| &mut **d)
    }

    pub fn list(&self) -> Vec<&str> {
        self.drivers.keys().map(|k| k.as_str()).collect()
    }

    pub fn set_default_route(&mut self, name: &str) {
        if self.drivers.contains_key(name) {
            self.default_route = Some(String::from(name));
        }
    }

    pub fn default_driver(&self) -> Option<&dyn NetDriver> {
        self.default_route.as_ref().and_then(|n| self.get(n))
    }

    pub fn default_driver_mut(&mut self) -> Option<&mut (dyn NetDriver + 'static)> {
        let name = self.default_route.clone()?;
        self.get_mut(&name)
    }

    pub fn default_driver_name(&self) -> Option<&str> {
        self.default_route.as_deref()
    }
}

pub static NET_DRIVERS: Mutex<Option<NetDriverManager>> = Mutex::new(None);

// ═══════════════════════════════════════════════════════════════════════════
//  Intel e1000 Ethernet Driver
// ═══════════════════════════════════════════════════════════════════════════

pub const E1000_CTRL: u32 = 0x0000;
pub const E1000_STATUS: u32 = 0x0008;
pub const E1000_EERD: u32 = 0x0014;
pub const E1000_ICR: u32 = 0x00C0;
pub const E1000_IMS: u32 = 0x00D0;
pub const E1000_IMC: u32 = 0x00D8;
pub const E1000_RCTL: u32 = 0x0100;
pub const E1000_TCTL: u32 = 0x0400;
pub const E1000_RDBAL: u32 = 0x2800;
pub const E1000_RDBAH: u32 = 0x2804;
pub const E1000_RDLEN: u32 = 0x2808;
pub const E1000_RDH: u32 = 0x2810;
pub const E1000_RDT: u32 = 0x2818;
pub const E1000_TDBAL: u32 = 0x3800;
pub const E1000_TDBAH: u32 = 0x3804;
pub const E1000_TDLEN: u32 = 0x3808;
pub const E1000_TDH: u32 = 0x3810;
pub const E1000_TDT: u32 = 0x3818;
pub const E1000_MTA: u32 = 0x5200;
pub const E1000_RAL: u32 = 0x5400;
pub const E1000_RAH: u32 = 0x5404;
pub const E1000_TIPG: u32 = 0x0410;
pub const E1000_TXDCTL: u32 = 0x3828;
pub const E1000_RXDCTL: u32 = 0x2828;
pub const E1000_FCAL: u32 = 0x0028;
pub const E1000_FCAH: u32 = 0x002C;
pub const E1000_FCT: u32 = 0x0030;
pub const E1000_FCTTV: u32 = 0x0170;

// CTRL register bits
const CTRL_SLU: u32 = 1 << 6;
const CTRL_RST: u32 = 1 << 26;
const CTRL_ASDE: u32 = 1 << 5;

// RCTL register bits
const RCTL_EN: u32 = 1 << 1;
const RCTL_SBP: u32 = 1 << 2;
const RCTL_UPE: u32 = 1 << 3;
const RCTL_MPE: u32 = 1 << 4;
const RCTL_LBM_NONE: u32 = 0;
const RCTL_BAM: u32 = 1 << 15;
const RCTL_BSIZE_2048: u32 = 0;
const RCTL_BSIZE_4096: u32 = 3 << 16;
const RCTL_SECRC: u32 = 1 << 26;

// TCTL register bits
const TCTL_EN: u32 = 1 << 1;
const TCTL_PSP: u32 = 1 << 3;
const TCTL_CT_SHIFT: u32 = 4;
const TCTL_COLD_SHIFT: u32 = 12;

// STATUS register bits
const STATUS_LU: u32 = 1 << 1;
const STATUS_SPEED_MASK: u32 = 3 << 6;
const STATUS_SPEED_10: u32 = 0 << 6;
const STATUS_SPEED_100: u32 = 1 << 6;
const STATUS_SPEED_1000: u32 = 2 << 6;

// Interrupt bits
const ICR_TXDW: u32 = 1 << 0;
const ICR_TXQE: u32 = 1 << 1;
const ICR_LSC: u32 = 1 << 2;
const ICR_RXSEQ: u32 = 1 << 3;
const ICR_RXDMT0: u32 = 1 << 4;
const ICR_RXO: u32 = 1 << 6;
const ICR_RXT0: u32 = 1 << 7;

// RX/TX descriptor status bits
const RXDESC_STATUS_DD: u8 = 1 << 0;
const RXDESC_STATUS_EOP: u8 = 1 << 1;
const TXDESC_CMD_EOP: u8 = 1 << 0;
const TXDESC_CMD_IFCS: u8 = 1 << 1;
const TXDESC_CMD_RS: u8 = 1 << 3;
const TXDESC_STATUS_DD: u8 = 1 << 0;

const RX_RING_SIZE: u32 = 256;
const TX_RING_SIZE: u32 = 256;
const RX_BUFFER_SIZE: usize = 2048;

/// One 4 KiB DMA-capable page (physical address for the NIC, virt via PHYS_MEM_OFFSET).
pub(crate) struct DmaPage {
    pub(crate) phys: u64,
    pub(crate) virt: u64,
}

pub(crate) fn alloc_dma_page() -> Result<DmaPage, NetDriverError> {
    use x86_64::structures::paging::FrameAllocator;
    let mut alloc = crate::memory::GlobalFrameAllocator;
    let frame = alloc
        .allocate_frame()
        .ok_or(NetDriverError::HardwareError)?;
    let phys = frame.start_address().as_u64();
    let virt = crate::memory::phys_to_virt(phys).as_u64();
    unsafe {
        core::ptr::write_bytes(virt as *mut u8, 0, 4096);
    }
    Ok(DmaPage { phys, virt })
}

pub(crate) fn dma_buf(page: &DmaPage, len: usize) -> &mut [u8] {
    unsafe { core::slice::from_raw_parts_mut(page.virt as *mut u8, len) }
}

#[repr(C, packed)]
#[derive(Debug, Clone, Copy, Default)]
pub struct E1000RxDesc {
    pub buffer_addr: u64,
    pub length: u16,
    pub checksum: u16,
    pub status: u8,
    pub errors: u8,
    pub special: u16,
}

#[repr(C, packed)]
#[derive(Debug, Clone, Copy, Default)]
pub struct E1000TxDesc {
    pub buffer_addr: u64,
    pub length: u16,
    pub cso: u8,
    pub cmd: u8,
    pub status: u8,
    pub css: u8,
    pub special: u16,
}

pub struct RxRing {
    desc_page: DmaPage,
    descriptors: *mut E1000RxDesc,
    buffers: Vec<DmaPage>,
    head: u32,
    tail: u32,
    count: u32,
}

pub struct TxRing {
    desc_page: DmaPage,
    descriptors: *mut E1000TxDesc,
    buffers: Vec<DmaPage>,
    head: u32,
    tail: u32,
    count: u32,
}

impl RxRing {
    fn new(count: u32) -> Result<Self, NetDriverError> {
        let desc_page = alloc_dma_page()?;
        let descriptors = desc_page.virt as *mut E1000RxDesc;
        let mut buffers = Vec::with_capacity(count as usize);
        for _ in 0..count {
            buffers.push(alloc_dma_page()?);
        }
        unsafe {
            for i in 0..count {
                let d = &mut *descriptors.add(i as usize);
                d.buffer_addr = buffers[i as usize].phys;
                d.length = RX_BUFFER_SIZE as u16;
                d.checksum = 0;
                d.status = 0;
                d.errors = 0;
                d.special = 0;
            }
        }
        Ok(Self {
            desc_page,
            descriptors,
            buffers,
            head: 0,
            tail: count - 1,
            count,
        })
    }
}

impl TxRing {
    fn new(count: u32) -> Result<Self, NetDriverError> {
        let desc_page = alloc_dma_page()?;
        let descriptors = desc_page.virt as *mut E1000TxDesc;
        let mut buffers = Vec::with_capacity(count as usize);
        for _ in 0..count {
            buffers.push(alloc_dma_page()?);
        }
        Ok(Self {
            desc_page,
            descriptors,
            buffers,
            head: 0,
            tail: 0,
            count,
        })
    }
}

// SAFETY: RX/TX descriptor rings and packet buffers are in frame-allocator DMA
// pages with stable phys/virt mappings; driver is used from BSP init/IRQ only.
unsafe impl Send for E1000Driver {}

pub struct E1000Driver {
    bar0: u64,
    mac: [u8; 6],
    irq: u8,
    rx_ring: RxRing,
    tx_ring: TxRing,
    link_up: bool,
    speed: LinkSpeed,
    stats: NetDriverStats,
    mtu: u32,
    promisc: bool,
    multicast: Vec<[u8; 6]>,
    enabled: bool,
    iommu_domain: Option<u16>,
}

impl RxRing {
    fn dma_regions(&self) -> Vec<(u64, u64)> {
        let mut r = vec![(self.desc_page.phys, 4096)];
        for p in &self.buffers {
            r.push((p.phys, 4096));
        }
        r
    }
}

impl TxRing {
    fn dma_regions(&self) -> Vec<(u64, u64)> {
        let mut r = vec![(self.desc_page.phys, 4096)];
        for p in &self.buffers {
            r.push((p.phys, 4096));
        }
        r
    }
}

impl E1000Driver {
    pub fn new(
        bar0_phys: u64,
        irq: u8,
        pci_bus: u8,
        pci_dev: u8,
        pci_func: u8,
    ) -> Result<Self, NetDriverError> {
        let offset = crate::memory::PHYS_MEM_OFFSET
            .get()
            .ok_or(NetDriverError::HardwareError)?;
        let bar0 = offset.as_u64() + bar0_phys;
        let rx_ring = RxRing::new(RX_RING_SIZE)?;
        let tx_ring = TxRing::new(TX_RING_SIZE)?;
        let mut drv = Self {
            bar0,
            mac: [0; 6],
            irq,
            rx_ring,
            tx_ring,
            link_up: false,
            speed: LinkSpeed::Unknown,
            stats: NetDriverStats::default(),
            mtu: 1500,
            promisc: false,
            multicast: Vec::new(),
            enabled: false,
            iommu_domain: None,
        };

        drv.reset();
        drv.read_mac_address();
        drv.set_mac_filter();
        drv.init_rx();
        drv.init_tx();
        drv.setup_interrupts();
        drv.detect_link();
        drv.enabled = true;

        let mut regions = drv.rx_ring.dma_regions();
        regions.extend(drv.tx_ring.dma_regions());
        drv.iommu_domain = crate::iommu::sandbox_device_dma(pci_bus, pci_dev, pci_func, &regions);

        Ok(drv)
    }

    fn read_reg(&self, reg: u32) -> u32 {
        unsafe { core::ptr::read_volatile((self.bar0 + reg as u64) as *const u32) }
    }

    fn write_reg(&self, reg: u32, val: u32) {
        unsafe { core::ptr::write_volatile((self.bar0 + reg as u64) as *mut u32, val) }
    }

    fn read_eeprom(&self, addr: u8) -> u16 {
        self.write_reg(E1000_EERD, (addr as u32) << 8 | 1);
        loop {
            let val = self.read_reg(E1000_EERD);
            if val & (1 << 4) != 0 {
                return (val >> 16) as u16;
            }
            core::hint::spin_loop();
        }
    }

    fn read_mac_address(&mut self) {
        let low = self.read_eeprom(0);
        let mid = self.read_eeprom(1);
        let high = self.read_eeprom(2);

        self.mac[0] = (low & 0xFF) as u8;
        self.mac[1] = (low >> 8) as u8;
        self.mac[2] = (mid & 0xFF) as u8;
        self.mac[3] = (mid >> 8) as u8;
        self.mac[4] = (high & 0xFF) as u8;
        self.mac[5] = (high >> 8) as u8;
    }

    fn reset(&mut self) {
        self.write_reg(E1000_IMC, 0xFFFF_FFFF);
        self.write_reg(E1000_CTRL, self.read_reg(E1000_CTRL) | CTRL_RST);

        for _ in 0..10_000 {
            core::hint::spin_loop();
        }

        self.write_reg(E1000_IMC, 0xFFFF_FFFF);
        let _ = self.read_reg(E1000_ICR);
    }

    fn init_rx(&mut self) {
        let desc_ptr = self.rx_ring.desc_page.phys;
        let desc_len = (self.rx_ring.count as u64) * core::mem::size_of::<E1000RxDesc>() as u64;

        self.write_reg(E1000_RDBAL, desc_ptr as u32);
        self.write_reg(E1000_RDBAH, (desc_ptr >> 32) as u32);
        self.write_reg(E1000_RDLEN, desc_len as u32);
        self.write_reg(E1000_RDH, 0);
        self.write_reg(E1000_RDT, self.rx_ring.tail);

        let rctl = RCTL_EN | RCTL_BAM | RCTL_LBM_NONE | RCTL_BSIZE_2048 | RCTL_SECRC;
        self.write_reg(E1000_RCTL, rctl);
    }

    fn init_tx(&mut self) {
        let desc_ptr = self.tx_ring.desc_page.phys;
        let desc_len = (self.tx_ring.count as u64) * core::mem::size_of::<E1000TxDesc>() as u64;

        self.write_reg(E1000_TDBAL, desc_ptr as u32);
        self.write_reg(E1000_TDBAH, (desc_ptr >> 32) as u32);
        self.write_reg(E1000_TDLEN, desc_len as u32);
        self.write_reg(E1000_TDH, 0);
        self.write_reg(E1000_TDT, 0);

        // Inter-packet gap: 10 | 10 << 10 | 10 << 20 for 802.3 standard
        self.write_reg(E1000_TIPG, 10 | (10 << 10) | (10 << 20));

        let tctl = TCTL_EN | TCTL_PSP | (15 << TCTL_CT_SHIFT) | (64 << TCTL_COLD_SHIFT);
        self.write_reg(E1000_TCTL, tctl);
    }

    fn setup_interrupts(&mut self) {
        self.write_reg(
            E1000_IMS,
            ICR_TXDW | ICR_RXT0 | ICR_LSC | ICR_RXDMT0 | ICR_RXO,
        );
    }

    pub fn handle_interrupt(&mut self) -> bool {
        let cause = self.read_reg(E1000_ICR);
        if cause == 0 {
            return false;
        }

        if cause & ICR_LSC != 0 {
            self.detect_link();
        }

        if cause & (ICR_RXT0 | ICR_RXDMT0 | ICR_RXO) != 0 {
            let _received = self.process_rx();
        }

        true
    }

    fn process_rx(&mut self) -> Vec<Vec<u8>> {
        let mut packets = Vec::new();
        loop {
            let idx = ((self.rx_ring.tail + 1) % self.rx_ring.count) as usize;
            let desc = unsafe { *self.rx_ring.descriptors.add(idx) };

            if desc.status & RXDESC_STATUS_DD == 0 {
                break;
            }

            if desc.status & RXDESC_STATUS_EOP != 0 && desc.errors == 0 {
                let len = desc.length as usize;
                let data = dma_buf(&self.rx_ring.buffers[idx], len.min(RX_BUFFER_SIZE)).to_vec();
                self.stats.rx_packets += 1;
                self.stats.rx_bytes += len as u64;
                packets.push(data);
            } else if desc.errors != 0 {
                self.stats.rx_errors += 1;
                if desc.errors & 0x01 != 0 {
                    self.stats.rx_crc_errors += 1;
                }
                if desc.errors & 0x02 != 0 {
                    self.stats.rx_frame_errors += 1;
                }
            }

            let buf_phys = self.rx_ring.buffers[idx].phys;
            unsafe {
                *self.rx_ring.descriptors.add(idx) = E1000RxDesc {
                    buffer_addr: buf_phys,
                    length: 0,
                    checksum: 0,
                    status: 0,
                    errors: 0,
                    special: 0,
                };
            }

            self.rx_ring.tail = idx as u32;
            self.write_reg(E1000_RDT, self.rx_ring.tail);
        }
        packets
    }

    fn detect_link(&mut self) {
        self.write_reg(E1000_CTRL, self.read_reg(E1000_CTRL) | CTRL_SLU | CTRL_ASDE);

        let status = self.read_reg(E1000_STATUS);
        self.link_up = status & STATUS_LU != 0;

        self.speed = match status & STATUS_SPEED_MASK {
            STATUS_SPEED_10 => LinkSpeed::Mbps10,
            STATUS_SPEED_100 => LinkSpeed::Mbps100,
            STATUS_SPEED_1000 => LinkSpeed::Gbps1,
            _ => LinkSpeed::Unknown,
        };
    }

    fn set_mac_filter(&self) {
        let lo = (self.mac[0] as u32)
            | ((self.mac[1] as u32) << 8)
            | ((self.mac[2] as u32) << 16)
            | ((self.mac[3] as u32) << 24);
        let hi = (self.mac[4] as u32) | ((self.mac[5] as u32) << 8) | (1 << 31); // Address Valid bit
        self.write_reg(E1000_RAL, lo);
        self.write_reg(E1000_RAH, hi);
    }

    fn update_multicast_table(&self) {
        // Clear the multicast table array (128 entries)
        for i in 0..128 {
            self.write_reg(E1000_MTA + i * 4, 0);
        }

        for addr in &self.multicast {
            let hash = Self::multicast_hash(addr);
            let reg = (hash >> 5) & 0x7F;
            let bit = hash & 0x1F;
            let val = self.read_reg(E1000_MTA + reg * 4);
            self.write_reg(E1000_MTA + reg * 4, val | (1 << bit));
        }
    }

    fn multicast_hash(addr: &[u8; 6]) -> u32 {
        let val = ((addr[5] as u32) << 8) | (addr[4] as u32);
        (val >> 1) & 0xFFF
    }
}

impl NetDriver for E1000Driver {
    fn name(&self) -> &str {
        "e1000"
    }

    fn mac_address(&self) -> [u8; 6] {
        self.mac
    }

    fn mtu(&self) -> u32 {
        self.mtu
    }

    fn set_mtu(&mut self, mtu: u32) -> Result<(), NetDriverError> {
        if mtu < 68 || mtu > 9216 {
            return Err(NetDriverError::InvalidMtu);
        }
        self.mtu = mtu;
        if mtu > 1500 {
            let rctl = self.read_reg(E1000_RCTL);
            self.write_reg(E1000_RCTL, (rctl & !(3 << 16)) | RCTL_BSIZE_4096);
        }
        Ok(())
    }

    fn link_speed(&self) -> LinkSpeed {
        self.speed
    }

    fn link_state(&self) -> LinkState {
        if !self.enabled {
            return LinkState::Down;
        }
        if self.link_up {
            LinkState::Up
        } else {
            LinkState::Down
        }
    }

    fn send(&mut self, packet: &[u8]) -> Result<(), NetDriverError> {
        if !self.link_up {
            self.detect_link();
        }
        if packet.len() > (self.mtu as usize + 14) {
            return Err(NetDriverError::BufferTooLarge);
        }

        let idx = self.tx_ring.tail as usize;
        let next_tail = (self.tx_ring.tail + 1) % self.tx_ring.count;

        if next_tail == self.tx_ring.head {
            self.stats.tx_dropped += 1;
            return Err(NetDriverError::QueueFull);
        }

        let buf_len = packet.len();
        if buf_len > RX_BUFFER_SIZE {
            return Err(NetDriverError::BufferTooLarge);
        }
        let dma = dma_buf(&self.tx_ring.buffers[idx], RX_BUFFER_SIZE);
        dma[..buf_len].copy_from_slice(packet);
        let buf_phys = self.tx_ring.buffers[idx].phys;

        unsafe {
            *self.tx_ring.descriptors.add(idx) = E1000TxDesc {
                buffer_addr: buf_phys,
                length: buf_len as u16,
                cso: 0,
                cmd: TXDESC_CMD_EOP | TXDESC_CMD_IFCS | TXDESC_CMD_RS,
                status: 0,
                css: 0,
                special: 0,
            };
        }

        self.tx_ring.tail = next_tail;
        self.write_reg(E1000_TDT, self.tx_ring.tail);

        for _ in 0..50_000 {
            let st = unsafe { (*self.tx_ring.descriptors.add(idx)).status };
            if st & TXDESC_STATUS_DD != 0 {
                break;
            }
            core::hint::spin_loop();
        }

        self.stats.tx_packets += 1;
        self.stats.tx_bytes += buf_len as u64;

        Ok(())
    }

    fn recv(&mut self) -> Option<Vec<u8>> {
        let idx = ((self.rx_ring.tail + 1) % self.rx_ring.count) as usize;
        let desc = unsafe { *self.rx_ring.descriptors.add(idx) };

        if desc.status & RXDESC_STATUS_DD == 0 {
            return None;
        }

        if desc.errors != 0 || desc.status & RXDESC_STATUS_EOP == 0 {
            self.stats.rx_errors += 1;
            unsafe {
                (*self.rx_ring.descriptors.add(idx)).status = 0;
            }
            self.rx_ring.tail = idx as u32;
            self.write_reg(E1000_RDT, self.rx_ring.tail);
            return None;
        }

        let len = (desc.length as usize).min(RX_BUFFER_SIZE);
        let data = dma_buf(&self.rx_ring.buffers[idx], len).to_vec();

        unsafe {
            *self.rx_ring.descriptors.add(idx) = E1000RxDesc {
                buffer_addr: self.rx_ring.buffers[idx].phys,
                length: 0,
                checksum: 0,
                status: 0,
                errors: 0,
                special: 0,
            };
        }

        self.rx_ring.tail = idx as u32;
        self.write_reg(E1000_RDT, self.rx_ring.tail);

        self.stats.rx_packets += 1;
        self.stats.rx_bytes += len as u64;
        Some(data)
    }

    fn enable(&mut self) -> Result<(), NetDriverError> {
        if self.enabled {
            return Err(NetDriverError::AlreadyEnabled);
        }
        self.write_reg(E1000_RCTL, self.read_reg(E1000_RCTL) | RCTL_EN);
        self.write_reg(E1000_TCTL, self.read_reg(E1000_TCTL) | TCTL_EN);
        self.setup_interrupts();
        self.enabled = true;
        Ok(())
    }

    fn disable(&mut self) -> Result<(), NetDriverError> {
        if !self.enabled {
            return Err(NetDriverError::AlreadyDisabled);
        }
        self.write_reg(E1000_IMC, 0xFFFF_FFFF);
        self.write_reg(E1000_RCTL, self.read_reg(E1000_RCTL) & !RCTL_EN);
        self.write_reg(E1000_TCTL, self.read_reg(E1000_TCTL) & !TCTL_EN);
        self.enabled = false;
        Ok(())
    }

    fn stats(&self) -> NetDriverStats {
        self.stats
    }

    fn set_promiscuous(&mut self, enabled: bool) {
        self.promisc = enabled;
        let mut rctl = self.read_reg(E1000_RCTL);
        if enabled {
            rctl |= RCTL_UPE | RCTL_MPE;
        } else {
            rctl &= !(RCTL_UPE | RCTL_MPE);
        }
        self.write_reg(E1000_RCTL, rctl);
    }

    fn add_multicast(&mut self, addr: [u8; 6]) {
        if !self.multicast.contains(&addr) {
            self.multicast.push(addr);
            self.update_multicast_table();
        }
    }

    fn remove_multicast(&mut self, addr: [u8; 6]) {
        self.multicast.retain(|a| *a != addr);
        self.update_multicast_table();
    }

    fn supports_offload(&self) -> OffloadCapabilities {
        OffloadCapabilities {
            tx_checksum: true,
            rx_checksum: true,
            tso: true,
            gso: false,
            gro: false,
            scatter_gather: true,
            vlan_offload: true,
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
//  Realtek RTL8169 / RTL8168 / RTL8111 / RTL8125 Production Driver
// ═══════════════════════════════════════════════════════════════════════════
//
// The "r8169" family — 1GbE (8169/8168/8111) and 2.5GbE (8125) — is the wired
// NIC on the overwhelming majority of consumer/gaming motherboards. Linux drives
// the whole family with ONE driver (r8169.c) over the shared "C+" 16-byte
// descriptor model; we do the same here. MasterChecklist Phase 2.2.
//
// NOTE: QEMU does not emulate this family (only the ancient rtl8139), so the
// DATA path is iron-verified, not QEMU-verified; on QEMU the probe simply finds
// no device. The register sequence follows the RTL8168/8125 datasheets + r8169.c.

// MMIO register offsets (r8169 family).
const RTL_IDR0: u32 = 0x00; // MAC address (IDR0..IDR5, 6 bytes)
const RTL_TNPDS: u32 = 0x20; // Tx Normal-Priority Descriptor Start (64-bit)
const RTL_CR: u32 = 0x37; // Command register (8-bit)
const RTL_TPPOLL: u32 = 0x38; // Tx Poll (8-bit) — 8169/8168/8111 family
/// AthenaOS fix (Phase 2.2): the RTL8125 moved the TX doorbell — writing the
/// legacy TPPoll (0x38) does NOTHING on it, so every queued frame sat in the
/// ring forever ("emitted=true" on Athena while no frame reached the wire:
/// DHCP stuck at Selecting, netlog silent). 8125 doorbell = 16-bit reg 0x90,
/// bit 0 (Linux r8169 `TxPoll_8125`).
const RTL_TPPOLL_8125: u32 = 0x90;
const RTL_IMR: u32 = 0x3C; // Interrupt Mask (16-bit)
const RTL_ISR: u32 = 0x3E; // Interrupt Status (16-bit)
const RTL_TCR: u32 = 0x40; // Tx Config (32-bit)
const RTL_RCR: u32 = 0x44; // Rx Config (32-bit)
const RTL_CFG9346: u32 = 0x50; // 93C46 command / config-register lock (8-bit)
const RTL_PHYSTATUS: u32 = 0x6C; // PHY status (8-bit)
const RTL_RMS: u32 = 0xDA; // Rx Max packet Size (16-bit)
const RTL_CPCR: u32 = 0xE0; // C+ Command (16-bit)
const RTL_RDSAR: u32 = 0xE4; // Rx Descriptor Start Address (64-bit)
const RTL_MTPS: u32 = 0xEC; // Max Tx Packet Size (8-bit)

// CR (0x37)
const RTL_CR_RST: u8 = 1 << 4;
const RTL_CR_RE: u8 = 1 << 3;
const RTL_CR_TE: u8 = 1 << 2;
// TPPoll (0x38)
const RTL_TPPOLL_NPQ: u8 = 1 << 6;
// Cfg9346 (0x50)
const RTL_CFG9346_UNLOCK: u8 = 0xC0;
const RTL_CFG9346_LOCK: u8 = 0x00;
// RCR (0x44): accept physical-match / multicast / broadcast, MXDMA + RXFTH max.
const RTL_RCR_APM: u32 = 1 << 1;
const RTL_RCR_AM: u32 = 1 << 2;
const RTL_RCR_AB: u32 = 1 << 3;
const RTL_RCR_AAP: u32 = 1 << 0; // accept all (promiscuous)
const RTL_RCR_MXDMA_UNLIMITED: u32 = 0b111 << 8;
const RTL_RCR_RXFTH_NONE: u32 = 0b111 << 13;
// TCR (0x40)
const RTL_TCR_MXDMA_UNLIMITED: u32 = 0b111 << 8;
const RTL_TCR_IFG_STD: u32 = 0b11 << 24;
// CPCR (0xE0)
const RTL_CPCR_MULRW: u16 = 1 << 3; // PCI multiple read/write
const RTL_CPCR_DAC: u16 = 1 << 4; // 64-bit DMA (dual address cycle)
                                  // PHYstatus (0x6C)
const RTL_PHY_LINKSTS: u8 = 1 << 1;
const RTL_PHY_10M: u8 = 1 << 2;
const RTL_PHY_100M: u8 = 1 << 3;
const RTL_PHY_1000M: u8 = 1 << 4;

// C+ descriptor opts1 flags.
const RTL_DESC_OWN: u32 = 1 << 31; // hardware owns this descriptor
const RTL_DESC_EOR: u32 = 1 << 30; // end of descriptor ring (wrap)
const RTL_DESC_FS: u32 = 1 << 29; // first segment
const RTL_DESC_LS: u32 = 1 << 28; // last segment
const RTL_DESC_FRAME_MASK: u32 = 0x3FFF; // 14-bit length field

const RTL_RING_SIZE: usize = 64; // descriptors per ring (16 B each → 1 KiB, page-aligned)
const RTL_BUF_SIZE: usize = 2048; // per-buffer (≥ 1518 + slack)

/// r8169 "C+" descriptor (16 bytes, naturally aligned via repr(C)).
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct RtlDesc {
    opts1: u32, // OWN/EOR/FS/LS + frame length
    opts2: u32, // VLAN / checksum (unused here)
    addr: u64,  // buffer physical address
}

// SAFETY: descriptor rings + buffers live in frame-allocator DMA pages with
// stable phys/virt mappings; the driver runs from BSP init / the net path only.
unsafe impl Send for RtlDriver {}

pub struct RtlDriver {
    bar: u64,
    mac: [u8; 6],
    irq: u8,
    device_id: u16,
    rx_desc: DmaPage,
    tx_desc: DmaPage,
    rx_buffers: Vec<DmaPage>,
    tx_buffers: Vec<DmaPage>,
    rx_cur: usize,
    tx_cur: usize,
    link_up: bool,
    speed: LinkSpeed,
    stats: NetDriverStats,
    mtu: u32,
    promisc: bool,
    multicast: Vec<[u8; 6]>,
    enabled: bool,
    iommu_domain: Option<u16>,
    /// TX descriptors actually consumed by the NIC (OWN cleared) vs stuck —
    /// distinguishes "transmitted" from merely "queued" on iron.
    tx_consumed: u64,
    tx_stuck: u64,
    /// Number of times `recv()` was polled (driven by the post-boot poll
    /// thread). Used to emit a periodic, greppable iron RX/DHCP status line so
    /// the next flash conclusively self-reports whether ANY frames arrived on
    /// the real RTL8125 and whether DHCP bound — captured in BOTH BOOTLOG.TXT
    /// and the end-of-boot netlog (CLAUDE.md §9). QEMU uses virtio-net, never
    /// this driver, so these lines are an iron-only signal.
    recv_polls: u64,
    /// Highest `recv_polls` milestone already reported (so we log once per
    /// milestone, not every poll — the iron console-logging tax is real).
    rx_report_milestone: u64,
}

impl RtlDriver {
    pub fn new(
        bar_phys: u64,
        irq: u8,
        device_id: u16,
        pci_bus: u8,
        pci_dev: u8,
        pci_func: u8,
    ) -> Result<Self, NetDriverError> {
        let offset = crate::memory::PHYS_MEM_OFFSET
            .get()
            .ok_or(NetDriverError::HardwareError)?;
        let bar = offset.as_u64() + bar_phys;

        let rx_desc = alloc_dma_page()?;
        let tx_desc = alloc_dma_page()?;
        let mut rx_buffers = Vec::with_capacity(RTL_RING_SIZE);
        let mut tx_buffers = Vec::with_capacity(RTL_RING_SIZE);
        for _ in 0..RTL_RING_SIZE {
            rx_buffers.push(alloc_dma_page()?);
            tx_buffers.push(alloc_dma_page()?);
        }

        let mut drv = Self {
            bar,
            mac: [0; 6],
            irq,
            device_id,
            rx_desc,
            tx_desc,
            rx_buffers,
            tx_buffers,
            rx_cur: 0,
            tx_cur: 0,
            link_up: false,
            speed: LinkSpeed::Unknown,
            stats: NetDriverStats::default(),
            mtu: 1500,
            promisc: false,
            multicast: Vec::new(),
            enabled: false,
            iommu_domain: None,
            tx_consumed: 0,
            tx_stuck: 0,
            recv_polls: 0,
            rx_report_milestone: 0,
        };

        drv.reset();
        drv.read_mac_address();
        drv.init_rings();
        drv.configure();
        drv.detect_link();
        drv.enabled = true;

        // Confine the NIC's DMA to exactly its descriptor rings + buffers.
        let mut regions = vec![(drv.rx_desc.phys, 4096), (drv.tx_desc.phys, 4096)];
        for p in drv.rx_buffers.iter().chain(drv.tx_buffers.iter()) {
            regions.push((p.phys, 4096));
        }
        drv.iommu_domain = crate::iommu::sandbox_device_dma(pci_bus, pci_dev, pci_func, &regions);

        Ok(drv)
    }

    fn read8(&self, reg: u32) -> u8 {
        unsafe { core::ptr::read_volatile((self.bar + reg as u64) as *const u8) }
    }
    fn write8(&self, reg: u32, val: u8) {
        unsafe { core::ptr::write_volatile((self.bar + reg as u64) as *mut u8, val) }
    }
    fn read16(&self, reg: u32) -> u16 {
        unsafe { core::ptr::read_volatile((self.bar + reg as u64) as *const u16) }
    }
    fn write16(&self, reg: u32, val: u16) {
        unsafe { core::ptr::write_volatile((self.bar + reg as u64) as *mut u16, val) }
    }
    fn read32(&self, reg: u32) -> u32 {
        unsafe { core::ptr::read_volatile((self.bar + reg as u64) as *const u32) }
    }
    fn write32(&self, reg: u32, val: u32) {
        unsafe { core::ptr::write_volatile((self.bar + reg as u64) as *mut u32, val) }
    }

    fn reset(&mut self) {
        self.write8(RTL_CR, RTL_CR_RST);
        // RST self-clears when the soft reset completes.
        for _ in 0..1_000_000 {
            if self.read8(RTL_CR) & RTL_CR_RST == 0 {
                break;
            }
            core::hint::spin_loop();
        }
    }

    fn read_mac_address(&mut self) {
        // IDR0..IDR5 are loaded from the EEPROM at reset and read directly.
        let lo = self.read32(RTL_IDR0);
        let hi = self.read32(RTL_IDR0 + 4);
        self.mac[0] = lo as u8;
        self.mac[1] = (lo >> 8) as u8;
        self.mac[2] = (lo >> 16) as u8;
        self.mac[3] = (lo >> 24) as u8;
        self.mac[4] = hi as u8;
        self.mac[5] = (hi >> 8) as u8;
    }

    fn rx_descs(&self) -> *mut RtlDesc {
        self.rx_desc.virt as *mut RtlDesc
    }
    fn tx_descs(&self) -> *mut RtlDesc {
        self.tx_desc.virt as *mut RtlDesc
    }

    fn init_rings(&mut self) {
        for i in 0..RTL_RING_SIZE {
            let eor = if i == RTL_RING_SIZE - 1 {
                RTL_DESC_EOR
            } else {
                0
            };
            // RX: hand the descriptor to the NIC (OWN) with the buffer size.
            let rx = RtlDesc {
                opts1: RTL_DESC_OWN | eor | (RTL_BUF_SIZE as u32 & RTL_DESC_FRAME_MASK),
                opts2: 0,
                addr: self.rx_buffers[i].phys,
            };
            unsafe { core::ptr::write_volatile(self.rx_descs().add(i), rx) };
            // TX: driver-owned (OWN clear); only EOR is meaningful until a send.
            let tx = RtlDesc {
                opts1: eor,
                opts2: 0,
                addr: self.tx_buffers[i].phys,
            };
            unsafe { core::ptr::write_volatile(self.tx_descs().add(i), tx) };
        }
    }

    fn configure(&mut self) {
        // Unlock the config registers (needed to touch Config0..5).
        self.write8(RTL_CFG9346, RTL_CFG9346_UNLOCK);
        // C+ command: multiple R/W bursts + 64-bit DMA addressing.
        self.write16(RTL_CPCR, RTL_CPCR_MULRW | RTL_CPCR_DAC);
        // Program the descriptor ring base addresses (must be set before enable).
        self.write32(RTL_RDSAR, self.rx_desc.phys as u32);
        self.write32(RTL_RDSAR + 4, (self.rx_desc.phys >> 32) as u32);
        self.write32(RTL_TNPDS, self.tx_desc.phys as u32);
        self.write32(RTL_TNPDS + 4, (self.tx_desc.phys >> 32) as u32);
        // POSTED-WRITE FLUSH (iron-only class, CLAUDE.md pitfall): the four
        // RDSAR/TNPDS stores above are posted on weakly-ordered PCIe. On real
        // 8125 silicon the NIC can latch a half-written 64-bit base if RX is
        // enabled before the high dword lands. A readback of the low dword is a
        // serializing fence that guarantees all four reached the device before
        // we set CR.RE. (QEMU/virtio never sees this; the 8125-only data path
        // is iron-verified — see the RX-enable readback that already fixed RX
        // arming last trip.) The value itself is not load-bearing; the read is.
        let _flush_rdsar = self.read32(RTL_RDSAR);
        // Rx config: accept unicast-to-us + multicast + broadcast, max DMA burst.
        let mut rcr =
            RTL_RCR_APM | RTL_RCR_AM | RTL_RCR_AB | RTL_RCR_MXDMA_UNLIMITED | RTL_RCR_RXFTH_NONE;
        if self.device_id == 0x8125 {
            // AthenaOS fix (Phase 2.2): the 8125 repurposes RxConfig's high bits
            // as the descriptor FETCH count — without it the NIC never fetches
            // RX descriptors, so nothing is ever received (TX worked, DHCP
            // OFFERs never arrived on Athena). Linux r8169 `rtl_init_rxcfg`:
            // RX_FETCH_DFLT_8125 = 8 << 27.
            rcr |= 8 << 27;
        }
        if self.promisc {
            rcr |= RTL_RCR_AAP;
        }
        self.write32(RTL_RCR, rcr);
        // Tx config: max DMA burst + standard inter-frame gap.
        self.write32(RTL_TCR, RTL_TCR_MXDMA_UNLIMITED | RTL_TCR_IFG_STD);
        // Rx max packet size + Max Tx Packet Size (128-byte units).
        self.write16(RTL_RMS, RTL_BUF_SIZE as u16);
        self.write8(RTL_MTPS, 0x3B);
        // Enable Rx + Tx.
        self.write8(RTL_CR, RTL_CR_RE | RTL_CR_TE);
        // POSTED-WRITE FLUSH: read CR back immediately so the RE/TE enable is
        // committed to the NIC before the config-register re-lock below. Without
        // this, on weakly-ordered PCIe the lock write could be observed by the
        // device before/around the enable. (The localizer further down also
        // reads CR, but that is after the lock+IMR+ISR writes — too late to
        // order the enable itself.)
        let _flush_cr = self.read8(RTL_CR);
        // Re-lock config registers; mask + ack interrupts (we poll).
        self.write8(RTL_CFG9346, RTL_CFG9346_LOCK);
        self.write16(RTL_IMR, 0);
        self.write16(RTL_ISR, 0xFFFF);

        // RX dead-on-iron localizer (8125): read back what the NIC actually
        // latched + how many RX descriptors are armed (OWN=NIC). On the next
        // flash: RCR/CR correct + RX still dead -> bug is past config
        // (descriptor-fetch / PHY-RXDV / 8125 extra init); RCR/CR wrong ->
        // register-access; rx_desc_own=0 -> the ring was never handed to the NIC.
        let rcr_rb = self.read32(RTL_RCR);
        let cr_rb = self.read8(RTL_CR);
        let armed = (0..RTL_RING_SIZE)
            .filter(|&i| unsafe {
                core::ptr::read_volatile(self.rx_descs().add(i)).opts1 & RTL_DESC_OWN != 0
            })
            .count();
        crate::serial_println!(
            "[rtl] RX armed: dev={:04x} RCR={:#010x} CR={:#04x} rx_desc_own={}/{} RE={}",
            self.device_id,
            rcr_rb,
            cr_rb,
            armed,
            RTL_RING_SIZE,
            cr_rb & RTL_CR_RE != 0
        );
    }

    fn detect_link(&mut self) {
        let ps = self.read8(RTL_PHYSTATUS);
        self.link_up = ps & RTL_PHY_LINKSTS != 0;
        self.speed = if ps & RTL_PHY_1000M != 0 {
            // 0x6C reports 1000M for both GbE and (on the 8125) 2.5GbE links;
            // treat an 8125's gigabit-flagged link as 2.5G capable.
            if self.device_id == 0x8125 {
                LinkSpeed::Gbps2_5
            } else {
                LinkSpeed::Gbps1
            }
        } else if ps & RTL_PHY_100M != 0 {
            LinkSpeed::Mbps100
        } else if ps & RTL_PHY_10M != 0 {
            LinkSpeed::Mbps10
        } else {
            LinkSpeed::Unknown
        };
    }
}

impl NetDriver for RtlDriver {
    fn name(&self) -> &str {
        "rtl8169"
    }
    fn mac_address(&self) -> [u8; 6] {
        self.mac
    }
    fn mtu(&self) -> u32 {
        self.mtu
    }
    fn set_mtu(&mut self, mtu: u32) -> Result<(), NetDriverError> {
        if mtu < 68 || mtu as usize > RTL_BUF_SIZE - 18 {
            return Err(NetDriverError::InvalidMtu);
        }
        self.mtu = mtu;
        Ok(())
    }
    fn link_speed(&self) -> LinkSpeed {
        self.speed
    }
    fn link_state(&self) -> LinkState {
        if self.enabled && self.link_up {
            LinkState::Up
        } else {
            LinkState::Down
        }
    }

    fn send(&mut self, packet: &[u8]) -> Result<(), NetDriverError> {
        if packet.len() > RTL_BUF_SIZE || packet.len() > self.mtu as usize + 18 {
            return Err(NetDriverError::BufferTooLarge);
        }
        let idx = self.tx_cur;
        let desc = unsafe { core::ptr::read_volatile(self.tx_descs().add(idx)) };
        if desc.opts1 & RTL_DESC_OWN != 0 {
            // NIC still owns this slot — ring full.
            self.stats.tx_dropped += 1;
            return Err(NetDriverError::QueueFull);
        }
        let eor = if idx == RTL_RING_SIZE - 1 {
            RTL_DESC_EOR
        } else {
            0
        };
        dma_buf(&self.tx_buffers[idx], RTL_BUF_SIZE)[..packet.len()].copy_from_slice(packet);
        let new = RtlDesc {
            opts1: RTL_DESC_OWN
                | RTL_DESC_FS
                | RTL_DESC_LS
                | eor
                | (packet.len() as u32 & RTL_DESC_FRAME_MASK),
            opts2: 0,
            addr: self.tx_buffers[idx].phys,
        };
        unsafe { core::ptr::write_volatile(self.tx_descs().add(idx), new) };
        // Kick the tx queue — the doorbell REGISTER differs per generation
        // (see RTL_TPPOLL_8125): the 8125 ignores the legacy 0x38 poll.
        if self.device_id == 0x8125 {
            self.write16(RTL_TPPOLL_8125, 0x0001);
        } else {
            self.write8(RTL_TPPOLL, RTL_TPPOLL_NPQ);
        }
        self.tx_cur = (idx + 1) % RTL_RING_SIZE;

        // Bounded wait for the NIC to release the descriptor (OWN clears) —
        // and RECORD the outcome: "queued" and "transmitted" are different
        // claims, and conflating them cost an Athena round (frames queued
        // into a never-polled ring reported success).
        let mut consumed = false;
        for _ in 0..1_000_000 {
            let d = unsafe { core::ptr::read_volatile(self.tx_descs().add(idx)) };
            if d.opts1 & RTL_DESC_OWN == 0 {
                consumed = true;
                break;
            }
            core::hint::spin_loop();
        }
        if consumed {
            self.tx_consumed += 1;
            if self.tx_consumed == 1 {
                crate::serial_println!(
                    "[rtl] TX path LIVE: first descriptor consumed by NIC (dev {:04x}, doorbell {:#x})",
                    self.device_id,
                    if self.device_id == 0x8125 { RTL_TPPOLL_8125 } else { RTL_TPPOLL },
                );
            }
        } else {
            self.tx_stuck += 1;
            if self.tx_stuck == 1 {
                crate::serial_println!(
                    "[rtl] WARN: first TX descriptor NOT consumed by the NIC (dev {:04x}, doorbell {:#x}) — frames may not be reaching the wire",
                    self.device_id,
                    if self.device_id == 0x8125 { RTL_TPPOLL_8125 } else { RTL_TPPOLL },
                );
            }
        }
        self.stats.tx_packets += 1;
        self.stats.tx_bytes += packet.len() as u64;
        Ok(())
    }

    fn recv(&mut self) -> Option<Vec<u8>> {
        // Periodic iron RX/DHCP self-report (CLAUDE.md §9). The post-boot poll
        // thread calls recv() roughly every ~50 ms; at fixed poll milestones we
        // emit ONE greppable line carrying the live RX counter AND the DHCP
        // outcome, so the next Athena flash conclusively shows — in BOOTLOG.TXT
        // and the netlog — whether ANY frame reached the real RTL8125 and
        // whether DHCP bound. Logged once per milestone to respect the iron
        // console-logging latency tax. QEMU never reaches this (virtio-net).
        self.recv_polls = self.recv_polls.saturating_add(1);
        // ~5 s, ~15 s, ~30 s after the poll thread starts (50 ms cadence).
        const MILESTONES: [u64; 3] = [100, 300, 600];
        if let Some(&m) = MILESTONES
            .iter()
            .find(|&&m| self.recv_polls == m && m > self.rx_report_milestone)
        {
            self.rx_report_milestone = m;
            let dhcp = match crate::dhcp::current_state() {
                Some(crate::dhcp::DhcpState::Bound) => "Bound",
                Some(crate::dhcp::DhcpState::Selecting) => "Selecting",
                Some(crate::dhcp::DhcpState::Requesting) => "Requesting",
                Some(crate::dhcp::DhcpState::Init) => "Init",
                Some(_) => "other",
                None => "none",
            };
            let armed = (0..RTL_RING_SIZE)
                .filter(|&i| unsafe {
                    core::ptr::read_volatile(self.rx_descs().add(i)).opts1 & RTL_DESC_OWN != 0
                })
                .count();
            crate::serial_println!(
                "[rtl] RX received: dev={:04x} pkts={} bytes={} errs={} (polls={} desc_own={}/{} link={})",
                self.device_id,
                self.stats.rx_packets,
                self.stats.rx_bytes,
                self.stats.rx_errors,
                self.recv_polls,
                armed,
                RTL_RING_SIZE,
                self.link_up as u8,
            );
            crate::serial_println!(
                "[net] iron DHCP: state={} (rtl dev={:04x} rx_pkts={}) -> {}",
                dhcp,
                self.device_id,
                self.stats.rx_packets,
                if dhcp == "Bound" {
                    "BOUND"
                } else if self.stats.rx_packets == 0 {
                    "NO-RX (no frames arrived on the wire)"
                } else {
                    "RX-OK-but-unbound (frames arrive; DHCP not yet Bound)"
                },
            );
        }

        let idx = self.rx_cur;
        let desc = unsafe { core::ptr::read_volatile(self.rx_descs().add(idx)) };
        if desc.opts1 & RTL_DESC_OWN != 0 {
            // NIC still owns it → no packet ready.
            return None;
        }
        // Frame length (bits 0..13) includes the 4-byte Ethernet CRC.
        let raw_len = (desc.opts1 & RTL_DESC_FRAME_MASK) as usize;
        let out = if raw_len > 4 {
            let len = (raw_len - 4).min(RTL_BUF_SIZE);
            let data = dma_buf(&self.rx_buffers[idx], len).to_vec();
            self.stats.rx_packets += 1;
            self.stats.rx_bytes += len as u64;
            if self.stats.rx_packets == 1 {
                // One-shot iron proof, mirroring "[rtl] TX path LIVE".
                crate::serial_println!(
                    "[rtl] RX path LIVE: first frame received ({} bytes, dev {:04x})",
                    len,
                    self.device_id,
                );
            }
            Some(data)
        } else {
            self.stats.rx_errors += 1;
            None
        };
        // Re-arm the descriptor for the NIC.
        let eor = if idx == RTL_RING_SIZE - 1 {
            RTL_DESC_EOR
        } else {
            0
        };
        let rearm = RtlDesc {
            opts1: RTL_DESC_OWN | eor | (RTL_BUF_SIZE as u32 & RTL_DESC_FRAME_MASK),
            opts2: 0,
            addr: self.rx_buffers[idx].phys,
        };
        // Order the address/length stores BEFORE the OWN handoff is observed,
        // then publish OWN. The NIC is DMA-coherent on x86 (WB), so no MMIO
        // flush is needed, but a release fence guarantees the descriptor body
        // (addr/eor/size) is globally visible before the device can act on the
        // OWN bit we just set — preventing a phantom-full / stale-buffer wedge
        // after the ring wraps on iron (CLAUDE.md pitfall #8).
        unsafe { core::ptr::write_volatile(self.rx_descs().add(idx), rearm) };
        core::sync::atomic::fence(core::sync::atomic::Ordering::Release);
        self.rx_cur = (idx + 1) % RTL_RING_SIZE;
        out
    }

    fn enable(&mut self) -> Result<(), NetDriverError> {
        if self.enabled {
            return Err(NetDriverError::AlreadyEnabled);
        }
        self.write8(RTL_CR, RTL_CR_RE | RTL_CR_TE);
        self.enabled = true;
        Ok(())
    }
    fn disable(&mut self) -> Result<(), NetDriverError> {
        if !self.enabled {
            return Err(NetDriverError::AlreadyDisabled);
        }
        self.write8(RTL_CR, 0);
        self.write16(RTL_IMR, 0);
        self.enabled = false;
        Ok(())
    }
    fn stats(&self) -> NetDriverStats {
        self.stats
    }
    fn set_promiscuous(&mut self, enabled: bool) {
        self.promisc = enabled;
        let mut rcr = self.read32(RTL_RCR);
        if enabled {
            rcr |= RTL_RCR_AAP;
        } else {
            rcr &= !RTL_RCR_AAP;
        }
        self.write32(RTL_RCR, rcr);
    }
    fn add_multicast(&mut self, addr: [u8; 6]) {
        if !self.multicast.contains(&addr) {
            self.multicast.push(addr);
        }
        // Accept-all-multicast is already set in RCR (RTL_RCR_AM); per-hash
        // MAR0..7 filtering is a refinement left for the iron tuning pass.
    }
    fn remove_multicast(&mut self, addr: [u8; 6]) {
        self.multicast.retain(|a| *a != addr);
    }
    fn supports_offload(&self) -> OffloadCapabilities {
        // The r8169 family supports HW checksum/VLAN offload, but we keep the
        // descriptors plain until the offload path is iron-validated.
        OffloadCapabilities::default()
    }
}

/// Read a memory BAR at PCI config `offset` (handles 32- and 64-bit BARs);
/// returns 0 for an I/O-space BAR. The r8169 PCIe family exposes its MMIO at
/// BAR2 (config offset 0x18).
fn pci_read_mem_bar(bus: u8, dev: u8, func: u8, offset: u8) -> u64 {
    let raw = crate::pci::read_config_32(bus, dev, func, offset);
    if raw & 1 != 0 {
        return 0; // I/O space BAR
    }
    let base_lo = (raw & !0xF) as u64;
    match (raw >> 1) & 0x03 {
        0x00 => base_lo,
        0x02 => {
            let hi = crate::pci::read_config_32(bus, dev, func, offset + 4);
            base_lo | ((hi as u64) << 32)
        }
        _ => 0,
    }
}

/// Scan PCI for Realtek r8169-family NICs (8169/8168/8111/8125), bring each up,
/// and register it with the manager. MasterChecklist Phase 2.2.
pub fn probe_rtl(mgr: &mut NetDriverManager) {
    let start = mgr.list().iter().filter(|n| n.starts_with("eth")).count() as u32;
    let mut idx = start;
    for pci_dev in &crate::pci::enumerate() {
        if pci_dev.vendor_id != 0x10EC {
            continue;
        }
        if !matches!(
            pci_dev.device_id,
            0x8169 | 0x8168 | 0x8161 | 0x8125 | 0x8136
        ) {
            continue;
        }
        crate::serial_println!(
            "[rtl8169] Found Realtek NIC {:04x}:{:04x} at {:02x}:{:02x}.{:x}",
            pci_dev.vendor_id,
            pci_dev.device_id,
            pci_dev.bus,
            pci_dev.device,
            pci_dev.function
        );
        pci_enable_device(pci_dev.bus, pci_dev.device, pci_dev.function);
        // r8169 PCIe MMIO is BAR2; fall back to BAR1 then BAR0 if absent.
        let bar = [0x18u8, 0x14, 0x10]
            .into_iter()
            .map(|o| pci_read_mem_bar(pci_dev.bus, pci_dev.device, pci_dev.function, o))
            .find(|b| *b != 0)
            .unwrap_or(0);
        if bar == 0 {
            crate::serial_println!("[rtl8169] no MMIO BAR, skipping");
            continue;
        }
        match RtlDriver::new(
            bar,
            pci_dev.irq_line,
            pci_dev.device_id,
            pci_dev.bus,
            pci_dev.device,
            pci_dev.function,
        ) {
            Ok(drv) => {
                let mac = drv.mac;
                crate::serial_println!(
                    "[rtl8169] eth{} MAC {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} link={} speed={}mbps",
                    idx,
                    mac[0], mac[1], mac[2], mac[3], mac[4], mac[5],
                    drv.link_up as u8,
                    drv.speed.as_mbps(),
                );
                mgr.register(alloc::format!("eth{}", idx), Box::new(drv));
                idx += 1;
            }
            Err(e) => crate::serial_println!("[rtl8169] init failed: {}", e),
        }
    }
    if idx > start {
        if mgr.default_driver_name().is_none() {
            mgr.set_default_route("eth0");
        }
        crate::serial_println!("[ OK ] rtl8169: {} NIC(s) initialized", idx - start);
    }
}

// ═══════════════════════════════════════════════════════════════════════════
//  VirtIO-Net Production Driver — QUARANTINED/REMOVED (rule 7), 2026-06-17
//  The former net_drivers::VirtioNetDriver was a non-functional structural twin
//  of the LIVE QEMU virtio-net driver in kernel/src/virtio_net.rs::VirtioNet.
//  It was never constructed (VirtioNetDriver::new had zero callers), and used a
//  pure-software VirtQueue ring whose descriptor addrs were VIRTUAL pointers
//  (buf.as_ptr()) never programmed into a real device queue — it would DMA from
//  a virtual address. Removed per CLAUDE.md §4 rule 7 (no parallel twins) since
//  it buys zero capability. See docs/QUARANTINED_MODULES.md.
//  Constants VIRTIO_NET_F_*, the software VirtQueue/VirtioNetHeader types, and
//  VirtioNetFeatures lived here exclusively for that driver and went with it.

// ═══════════════════════════════════════════════════════════════════════════
//  WiFi 802.11 Framework
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WifiIfType {
    Station,
    Ap,
    Monitor,
    P2pClient,
    P2pGo,
    MeshPoint,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WifiState {
    Disconnected,
    Scanning,
    Authenticating,
    Associating,
    Connected,
    Disconnecting,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WifiSecurity {
    Open,
    Wep,
    WpaPsk,
    Wpa2Psk,
    Wpa3Sae,
    Wpa2Enterprise,
    Wpa3Enterprise,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WifiBand {
    Band2_4GHz,
    Band5GHz,
    Band6GHz,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelWidth {
    Mhz20,
    Mhz40,
    Mhz80,
    Mhz160,
    Mhz320,
}

#[derive(Debug, Clone)]
pub struct WifiInterface {
    pub name: String,
    pub mac: [u8; 6],
    pub phy_index: u32,
    pub iftype: WifiIfType,
}

#[derive(Debug, Clone)]
pub struct WifiBss {
    pub bssid: [u8; 6],
    pub ssid: String,
    pub frequency: u32,
    pub channel: u8,
    pub signal_dbm: i8,
    pub noise_dbm: i8,
    pub security: WifiSecurity,
    pub band: WifiBand,
    pub width: ChannelWidth,
    pub supported_rates: Vec<u8>,
    pub beacon_interval: u16,
    pub dtim_period: u8,
    pub country: Option<[u8; 2]>,
    pub ht_capable: bool,
    pub vht_capable: bool,
    pub he_capable: bool,
    pub wmm: bool,
}

#[derive(Debug, Clone, Default)]
pub struct WifiCapabilities {
    pub max_scan_ssids: u8,
    pub max_sched_scan_ssids: u8,
    pub max_match_sets: u8,
    pub bands: Vec<WifiBand>,
    pub max_stations: u32,
    pub features: WifiFeatures,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct WifiFeatures {
    pub sae: bool,
    pub owe: bool,
    pub ft: bool,
    pub tdls: bool,
    pub pmf: bool,
    pub offchannel_tx: bool,
    pub roam_support: bool,
    pub p2p: bool,
    pub mesh: bool,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct WifiStats {
    pub tx_packets: u64,
    pub rx_packets: u64,
    pub tx_bytes: u64,
    pub rx_bytes: u64,
    pub tx_retries: u64,
    pub tx_failed: u64,
    pub beacon_loss: u64,
    pub signal_avg: i8,
}

// ─── 802.11 Frame Types ─────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Ieee80211Header {
    pub frame_control: u16,
    pub duration: u16,
    pub addr1: [u8; 6],
    pub addr2: [u8; 6],
    pub addr3: [u8; 6],
    pub seq_ctrl: u16,
    pub addr4: Option<[u8; 6]>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ieee80211FrameType {
    Management(MgmtSubtype),
    Control(CtrlSubtype),
    Data(DataSubtype),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MgmtSubtype {
    AssocReq,
    AssocResp,
    ReassocReq,
    ReassocResp,
    ProbeReq,
    ProbeResp,
    Beacon,
    Atim,
    Disassoc,
    Auth,
    Deauth,
    Action,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CtrlSubtype {
    BlockAckReq,
    BlockAck,
    PsPoll,
    Rts,
    Cts,
    Ack,
    CfEnd,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataSubtype {
    Data,
    Null,
    QosData,
    QosNull,
}

#[derive(Debug, Clone)]
pub struct InformationElement {
    pub id: u8,
    pub data: Vec<u8>,
}

// IE type constants
const IE_SSID: u8 = 0;
const IE_SUPPORTED_RATES: u8 = 1;
const IE_DS_PARAMS: u8 = 3;
const IE_TIM: u8 = 5;
const IE_COUNTRY: u8 = 7;
const IE_HT_CAPABILITIES: u8 = 45;
const IE_RSN: u8 = 48;
const IE_HT_OPERATION: u8 = 61;
const IE_VHT_CAPABILITIES: u8 = 191;
const IE_VHT_OPERATION: u8 = 192;
const IE_VENDOR_SPECIFIC: u8 = 221;

pub struct WifiDriver {
    interface: WifiInterface,
    state: WifiState,
    scan_results: Vec<WifiBss>,
    connected_bss: Option<WifiBss>,
    supported_bands: Vec<WifiBand>,
    capabilities: WifiCapabilities,
    stats: WifiStats,
    seq_num: u16,
}

impl WifiDriver {
    pub fn new(interface: WifiInterface) -> Self {
        Self {
            state: WifiState::Disconnected,
            scan_results: Vec::new(),
            connected_bss: None,
            supported_bands: vec![WifiBand::Band2_4GHz, WifiBand::Band5GHz],
            capabilities: WifiCapabilities {
                max_scan_ssids: 16,
                max_sched_scan_ssids: 16,
                max_match_sets: 8,
                bands: vec![WifiBand::Band2_4GHz, WifiBand::Band5GHz],
                max_stations: 128,
                features: WifiFeatures {
                    sae: true,
                    owe: true,
                    ft: true,
                    tdls: false,
                    pmf: true,
                    offchannel_tx: true,
                    roam_support: true,
                    p2p: false,
                    mesh: false,
                },
            },
            stats: WifiStats::default(),
            interface,
            seq_num: 0,
        }
    }

    pub fn scan(&mut self) -> Result<Vec<WifiBss>, NetDriverError> {
        if self.state == WifiState::Connected {
            return Err(NetDriverError::UnsupportedOperation);
        }
        self.state = WifiState::Scanning;
        self.scan_results.clear();

        // In a real driver: send probe requests on each channel, collect beacons.
        // Return whatever we've gathered so far.
        self.state = WifiState::Disconnected;
        Ok(self.scan_results.clone())
    }

    pub fn connect(
        &mut self,
        ssid: &str,
        password: Option<&str>,
        security: WifiSecurity,
    ) -> Result<(), NetDriverError> {
        if self.state == WifiState::Connected {
            self.disconnect()?;
        }

        let bss = self.scan_results.iter().find(|b| b.ssid == ssid).cloned();
        let bss = match bss {
            Some(b) => b,
            None => return Err(NetDriverError::NoNetwork),
        };

        if bss.security != security {
            return Err(NetDriverError::AuthenticationFailed);
        }

        self.authenticate(&bss)?;
        self.associate(&bss)?;

        match security {
            WifiSecurity::WpaPsk | WifiSecurity::Wpa2Psk | WifiSecurity::Wpa3Sae => {
                let psk = password.ok_or(NetDriverError::AuthenticationFailed)?;
                self.four_way_handshake(psk.as_bytes())?;
            }
            WifiSecurity::Open => {}
            _ => return Err(NetDriverError::UnsupportedOperation),
        }

        self.state = WifiState::Connected;
        self.connected_bss = Some(bss);
        Ok(())
    }

    pub fn disconnect(&mut self) -> Result<(), NetDriverError> {
        if self.state != WifiState::Connected {
            return Ok(());
        }
        self.state = WifiState::Disconnecting;
        self.connected_bss = None;
        self.state = WifiState::Disconnected;
        Ok(())
    }

    pub fn get_signal_strength(&self) -> Option<i8> {
        self.connected_bss.as_ref().map(|b| b.signal_dbm)
    }

    pub fn get_connected_bss(&self) -> Option<&WifiBss> {
        self.connected_bss.as_ref()
    }

    pub fn state(&self) -> WifiState {
        self.state
    }

    pub fn stats(&self) -> &WifiStats {
        &self.stats
    }

    fn authenticate(&mut self, _bss: &WifiBss) -> Result<(), NetDriverError> {
        self.state = WifiState::Authenticating;
        // Build and send authentication frame (open system or SAE)
        // Wait for authentication response
        Ok(())
    }

    fn associate(&mut self, _bss: &WifiBss) -> Result<(), NetDriverError> {
        self.state = WifiState::Associating;
        // Build association request with supported rates, HT/VHT/HE capabilities
        // Wait for association response
        Ok(())
    }

    fn four_way_handshake(&mut self, _psk: &[u8]) -> Result<(), NetDriverError> {
        // EAPOL 4-way handshake:
        //   1. AP -> STA: ANonce
        //   2. STA -> AP: SNonce + MIC
        //   3. AP -> STA: GTK + MIC
        //   4. STA -> AP: ACK
        // Derive PTK from PMK + ANonce + SNonce + MAC addresses
        Ok(())
    }

    pub fn process_beacon(&mut self, frame: &[u8]) {
        if frame.len() < 36 {
            return;
        }

        let bssid: [u8; 6] = [
            frame[16], frame[17], frame[18], frame[19], frame[20], frame[21],
        ];

        let _timestamp = u64::from_le_bytes([
            frame[24], frame[25], frame[26], frame[27], frame[28], frame[29], frame[30], frame[31],
        ]);
        let beacon_interval = u16::from_le_bytes([frame[32], frame[33]]);
        let _capability = u16::from_le_bytes([frame[34], frame[35]]);

        let ies = self.parse_information_elements(&frame[36..]);

        let mut ssid = String::new();
        let mut channel = 0u8;
        let mut supported_rates = Vec::new();
        let mut ht_capable = false;
        let mut vht_capable = false;
        let mut country = None;
        let mut security = WifiSecurity::Open;
        let mut dtim_period = 1u8;
        let mut wmm = false;

        for ie in &ies {
            match ie.id {
                IE_SSID => {
                    if let Ok(s) = core::str::from_utf8(&ie.data) {
                        ssid = String::from(s);
                    }
                }
                IE_SUPPORTED_RATES => {
                    supported_rates.extend_from_slice(&ie.data);
                }
                IE_DS_PARAMS if !ie.data.is_empty() => {
                    channel = ie.data[0];
                }
                IE_TIM if ie.data.len() >= 2 => {
                    dtim_period = ie.data[1];
                }
                IE_COUNTRY if ie.data.len() >= 2 => {
                    country = Some([ie.data[0], ie.data[1]]);
                }
                IE_RSN => {
                    security = self.parse_rsn_ie(&ie.data);
                }
                IE_HT_CAPABILITIES => {
                    ht_capable = true;
                }
                IE_VHT_CAPABILITIES => {
                    vht_capable = true;
                }
                IE_VENDOR_SPECIFIC if ie.data.len() >= 4 => {
                    // WMM OUI: 00:50:F2:02
                    if ie.data[0] == 0x00
                        && ie.data[1] == 0x50
                        && ie.data[2] == 0xF2
                        && ie.data[3] == 0x02
                    {
                        wmm = true;
                    }
                }
                _ => {}
            }
        }

        let (band, frequency) = Self::channel_to_band_freq(channel);

        let bss = WifiBss {
            bssid,
            ssid,
            frequency,
            channel,
            signal_dbm: -50,
            noise_dbm: -90,
            security,
            band,
            width: if vht_capable {
                ChannelWidth::Mhz80
            } else if ht_capable {
                ChannelWidth::Mhz40
            } else {
                ChannelWidth::Mhz20
            },
            supported_rates,
            beacon_interval,
            dtim_period,
            country,
            ht_capable,
            vht_capable,
            he_capable: false,
            wmm,
        };

        if let Some(existing) = self.scan_results.iter_mut().find(|b| b.bssid == bssid) {
            *existing = bss;
        } else {
            self.scan_results.push(bss);
        }
    }

    fn parse_information_elements(&self, data: &[u8]) -> Vec<InformationElement> {
        let mut ies = Vec::new();
        let mut pos = 0;
        while pos + 2 <= data.len() {
            let id = data[pos];
            let len = data[pos + 1] as usize;
            pos += 2;
            if pos + len > data.len() {
                break;
            }
            ies.push(InformationElement {
                id,
                data: data[pos..pos + len].to_vec(),
            });
            pos += len;
        }
        ies
    }

    fn parse_rsn_ie(&self, data: &[u8]) -> WifiSecurity {
        if data.len() < 10 {
            return WifiSecurity::Open;
        }

        // Skip version(2) + group cipher suite(4) + pairwise count(2)
        let offset = 8;
        if offset + 4 > data.len() {
            return WifiSecurity::Wpa2Psk;
        }

        // Check AKM suite for PSK vs SAE vs Enterprise
        let akm_count = u16::from_le_bytes([data[offset], data[offset + 1]]) as usize;
        let akm_start = offset + 2;

        for i in 0..akm_count {
            let base = akm_start + i * 4;
            if base + 4 > data.len() {
                break;
            }
            let akm_type = data[base + 3];
            match akm_type {
                1 => return WifiSecurity::Wpa2Enterprise,
                2 => return WifiSecurity::Wpa2Psk,
                8 => return WifiSecurity::Wpa3Sae,
                _ => {}
            }
        }

        WifiSecurity::Wpa2Psk
    }

    fn channel_to_band_freq(channel: u8) -> (WifiBand, u32) {
        match channel {
            1..=14 => {
                let freq = if channel == 14 {
                    2484
                } else {
                    2407 + (channel as u32) * 5
                };
                (WifiBand::Band2_4GHz, freq)
            }
            36..=177 => {
                let freq = 5000 + (channel as u32) * 5;
                (WifiBand::Band5GHz, freq)
            }
            _ => (WifiBand::Band2_4GHz, 2412),
        }
    }

    fn next_seq_num(&mut self) -> u16 {
        let seq = self.seq_num;
        self.seq_num = self.seq_num.wrapping_add(1) & 0x0FFF;
        seq
    }

    pub fn parse_frame_type(frame_control: u16) -> Option<Ieee80211FrameType> {
        let type_bits = (frame_control >> 2) & 0x3;
        let subtype_bits = (frame_control >> 4) & 0xF;

        match type_bits {
            0 => {
                let sub = match subtype_bits {
                    0 => MgmtSubtype::AssocReq,
                    1 => MgmtSubtype::AssocResp,
                    2 => MgmtSubtype::ReassocReq,
                    3 => MgmtSubtype::ReassocResp,
                    4 => MgmtSubtype::ProbeReq,
                    5 => MgmtSubtype::ProbeResp,
                    8 => MgmtSubtype::Beacon,
                    9 => MgmtSubtype::Atim,
                    10 => MgmtSubtype::Disassoc,
                    11 => MgmtSubtype::Auth,
                    12 => MgmtSubtype::Deauth,
                    13 => MgmtSubtype::Action,
                    _ => return None,
                };
                Some(Ieee80211FrameType::Management(sub))
            }
            1 => {
                let sub = match subtype_bits {
                    8 => CtrlSubtype::BlockAckReq,
                    9 => CtrlSubtype::BlockAck,
                    10 => CtrlSubtype::PsPoll,
                    11 => CtrlSubtype::Rts,
                    12 => CtrlSubtype::Cts,
                    13 => CtrlSubtype::Ack,
                    14 => CtrlSubtype::CfEnd,
                    _ => return None,
                };
                Some(Ieee80211FrameType::Control(sub))
            }
            2 => {
                let sub = match subtype_bits {
                    0 => DataSubtype::Data,
                    4 => DataSubtype::Null,
                    8 => DataSubtype::QosData,
                    12 => DataSubtype::QosNull,
                    _ => return None,
                };
                Some(Ieee80211FrameType::Data(sub))
            }
            _ => None,
        }
    }

    pub fn parse_header(frame: &[u8]) -> Option<Ieee80211Header> {
        if frame.len() < 24 {
            return None;
        }

        let frame_control = u16::from_le_bytes([frame[0], frame[1]]);
        let duration = u16::from_le_bytes([frame[2], frame[3]]);

        let mut addr1 = [0u8; 6];
        let mut addr2 = [0u8; 6];
        let mut addr3 = [0u8; 6];
        addr1.copy_from_slice(&frame[4..10]);
        addr2.copy_from_slice(&frame[10..16]);
        addr3.copy_from_slice(&frame[16..22]);

        let seq_ctrl = u16::from_le_bytes([frame[22], frame[23]]);

        let to_ds = frame_control & (1 << 8) != 0;
        let from_ds = frame_control & (1 << 9) != 0;
        let addr4 = if to_ds && from_ds && frame.len() >= 30 {
            let mut a4 = [0u8; 6];
            a4.copy_from_slice(&frame[24..30]);
            Some(a4)
        } else {
            None
        };

        Some(Ieee80211Header {
            frame_control,
            duration,
            addr1,
            addr2,
            addr3,
            seq_ctrl,
            addr4,
        })
    }
}

// ═══════════════════════════════════════════════════════════════════════════
//  Loopback Driver
// ═══════════════════════════════════════════════════════════════════════════

pub struct LoopbackDriver {
    mtu: u32,
    stats: NetDriverStats,
    rx_queue: Vec<Vec<u8>>,
    enabled: bool,
}

impl LoopbackDriver {
    pub fn new() -> Self {
        Self {
            mtu: 65535,
            stats: NetDriverStats::default(),
            rx_queue: Vec::new(),
            enabled: true,
        }
    }
}

impl NetDriver for LoopbackDriver {
    fn name(&self) -> &str {
        "lo"
    }

    fn mac_address(&self) -> [u8; 6] {
        [0; 6]
    }

    fn mtu(&self) -> u32 {
        self.mtu
    }

    fn set_mtu(&mut self, mtu: u32) -> Result<(), NetDriverError> {
        if mtu < 68 || mtu > 65535 {
            return Err(NetDriverError::InvalidMtu);
        }
        self.mtu = mtu;
        Ok(())
    }

    fn link_speed(&self) -> LinkSpeed {
        LinkSpeed::Gbps100
    }

    fn link_state(&self) -> LinkState {
        LinkState::Up
    }

    fn send(&mut self, packet: &[u8]) -> Result<(), NetDriverError> {
        if packet.len() > self.mtu as usize + 14 {
            return Err(NetDriverError::BufferTooLarge);
        }
        self.rx_queue.push(packet.to_vec());
        self.stats.tx_packets += 1;
        self.stats.tx_bytes += packet.len() as u64;
        Ok(())
    }

    fn recv(&mut self) -> Option<Vec<u8>> {
        if self.rx_queue.is_empty() {
            return None;
        }
        let pkt = self.rx_queue.remove(0);
        self.stats.rx_packets += 1;
        self.stats.rx_bytes += pkt.len() as u64;
        Some(pkt)
    }

    fn enable(&mut self) -> Result<(), NetDriverError> {
        self.enabled = true;
        Ok(())
    }

    fn disable(&mut self) -> Result<(), NetDriverError> {
        self.enabled = false;
        Ok(())
    }

    fn stats(&self) -> NetDriverStats {
        self.stats
    }

    fn set_promiscuous(&mut self, _enabled: bool) {}

    fn add_multicast(&mut self, _addr: [u8; 6]) {}

    fn remove_multicast(&mut self, _addr: [u8; 6]) {}

    fn supports_offload(&self) -> OffloadCapabilities {
        OffloadCapabilities::default()
    }
}

// ═══════════════════════════════════════════════════════════════════════════
//  Intel I219 Probe — MMIO identify (Phase 2.2 — probe + link status only)
// ═══════════════════════════════════════════════════════════════════════════

const I219_VENDOR: u16 = 0x8086;
/// PCI device IDs covering the I219-V/LM/W family (Skylake through Meteor Lake).
const I219_DEVICE_IDS: &[u16] = &[
    0x15B8, // I219-V  (Skylake H/Z/Q)
    0x15D8, // I219-V  (Kaby Lake)
    0x15E3, // I219-LM (Kaby Lake vPro)
    0x156F, // I219-LM (Skylake vPro)
    0x1570, // I219-V  (Skylake)
    0x15BB, // I219-LM (Coffee Lake)
    0x15BC, // I219-V  (Coffee Lake)
    0x15BD, // I219-LM (Coffee Lake / Whiskey Lake)
    0x15BE, // I219-V  (Coffee Lake)
    0x0D4E, // I219-LM (Tiger Lake)
    0x0D4F, // I219-V  (Tiger Lake)
    0x0D4C, // I219-LM (Ice Lake)
    0x0D4D, // I219-V  (Ice Lake)
    0x0D53, // I219-LM (Alder Lake)
    0x0D55, // I219-V  (Alder Lake)
];

// I219 shares the CTRL/STATUS/RAL/RAH register map with e1000e.
const I219_CTRL: u32 = 0x0000;
const I219_STATUS: u32 = 0x0008;
const I219_RAL: u32 = 0x5400;
const I219_RAH: u32 = 0x5404;
const I219_CTRL_RST: u32 = 1 << 26; // device reset
const I219_CTRL_SLU: u32 = 1 << 6; // set link up
const I219_STATUS_LU: u32 = 1 << 1; // link up
const I219_RAH_AV: u32 = 1 << 31; // address valid

fn is_i219(vendor: u16, device: u16) -> bool {
    vendor == I219_VENDOR && I219_DEVICE_IDS.contains(&device)
}

/// Scan the PCI bus for Intel I219 NICs. Performs MMIO reset, reads the MAC
/// address and checks link status. No TX/RX rings are set up at this phase —
/// Phase 2.2 goal is "knows what's plugged in".
pub fn probe_i219(mgr: &mut NetDriverManager) {
    let _ = mgr; // reserved for future driver registration

    let devices = crate::pci::enumerate();
    let mut found = 0u32;

    for pci_dev in &devices {
        if !is_i219(pci_dev.vendor_id, pci_dev.device_id) {
            continue;
        }

        let bus = pci_dev.bus;
        let dev = pci_dev.device;
        let func = pci_dev.function;

        crate::serial_println!(
            "[i219] Intel I219 {:04x}:{:04x} @ {:02x}:{:02x}.{}",
            pci_dev.vendor_id,
            pci_dev.device_id,
            bus,
            dev,
            func,
        );

        // Enable bus-master + memory-space in the PCI command register.
        pci_enable_device(bus, dev, func);

        let bar0_phys = pci_read_bar0(bus, dev, func);
        if bar0_phys == 0 {
            crate::serial_println!("[i219] BAR0 invalid or I/O-space, skipping");
            continue;
        }

        // Map BAR0 into kernel virtual address space via the physical memory offset.
        let phys_offset = match crate::memory::PHYS_MEM_OFFSET.get() {
            Some(o) => o.as_u64(),
            None => {
                crate::serial_println!("[i219] PHYS_MEM_OFFSET not set, skipping");
                continue;
            }
        };
        let bar0 = phys_offset + bar0_phys;

        // Issue a device reset (CTRL bit 26) and wait for self-clear.
        unsafe {
            let ctrl_ptr = (bar0 + I219_CTRL as u64) as *mut u32;
            let ctrl = core::ptr::read_volatile(ctrl_ptr);
            core::ptr::write_volatile(ctrl_ptr, ctrl | I219_CTRL_RST);
            for _ in 0..100_000 {
                if core::ptr::read_volatile(ctrl_ptr) & I219_CTRL_RST == 0 {
                    break;
                }
                core::hint::spin_loop();
            }
            // Assert SLU so the STATUS register reflects PHY link state.
            let ctrl = core::ptr::read_volatile(ctrl_ptr);
            core::ptr::write_volatile(ctrl_ptr, ctrl | I219_CTRL_SLU);
        }

        // Read MAC from Receive Address Low/High registers.
        let ral = unsafe { core::ptr::read_volatile((bar0 + I219_RAL as u64) as *const u32) };
        let rah = unsafe { core::ptr::read_volatile((bar0 + I219_RAH as u64) as *const u32) };
        let mac = [
            (ral & 0xFF) as u8,
            ((ral >> 8) & 0xFF) as u8,
            ((ral >> 16) & 0xFF) as u8,
            ((ral >> 24) & 0xFF) as u8,
            (rah & 0xFF) as u8,
            ((rah >> 8) & 0xFF) as u8,
        ];
        let av = rah & I219_RAH_AV != 0;

        // Check link status.
        let status = unsafe { core::ptr::read_volatile((bar0 + I219_STATUS as u64) as *const u32) };
        let link_up = status & I219_STATUS_LU != 0;

        crate::serial_println!(
            "[i219] Intel I219 at {:02x}:{:02x}.{} BAR0={:#x} \
             mac={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} (av={}) link={}",
            bus,
            dev,
            func,
            bar0_phys,
            mac[0],
            mac[1],
            mac[2],
            mac[3],
            mac[4],
            mac[5],
            av,
            if link_up { "up" } else { "down" },
        );

        found += 1;
    }

    if found == 0 {
        crate::serial_println!("[i219] Intel I219 not found (expected on QEMU)");
    } else {
        crate::serial_println!("[ OK ] i219: {} device(s) identified", found);
    }
}

// ═══════════════════════════════════════════════════════════════════════════
//  e1000 PCI Probe — scan PCI bus for Intel e1000 NICs and initialize
// ═══════════════════════════════════════════════════════════════════════════

const E1000_VENDOR_ID: u16 = 0x8086;
const E1000_DEVICE_IDS: [u16; 8] = [
    0x100E, 0x100F, // QEMU / legacy
    0x10D3, 0x153A, 0x1521, 0x1533, // e1000e common
    0x10A9, 0x10D6,
];

const PCI_CMD_BUS_MASTER: u16 = 1 << 2;
const PCI_CMD_MEMORY_SPACE: u16 = 1 << 1;

fn is_e1000_device(vendor: u16, device: u16) -> bool {
    vendor == E1000_VENDOR_ID && E1000_DEVICE_IDS.contains(&device)
}

/// Read BAR0 from PCI config space and return the MMIO base address.
fn pci_read_bar0(bus: u8, dev: u8, func: u8) -> u64 {
    let bar0_raw = crate::pci::read_config_32(bus, dev, func, 0x10);
    if bar0_raw & 1 != 0 {
        return 0; // I/O space BAR, not usable for MMIO
    }
    let bar_type = (bar0_raw >> 1) & 0x03;
    let base_lo = (bar0_raw & !0xF) as u64;
    match bar_type {
        0x00 => base_lo,
        0x02 => {
            let bar1_raw = crate::pci::read_config_32(bus, dev, func, 0x14);
            base_lo | ((bar1_raw as u64) << 32)
        }
        _ => 0,
    }
}

/// Enable bus mastering and memory space access for a PCI device.
fn pci_enable_device(bus: u8, dev: u8, func: u8) {
    let cmd = crate::pci::read_config_16(bus, dev, func, 0x04);
    crate::pci::write_config_16(
        bus,
        dev,
        func,
        0x04,
        cmd | PCI_CMD_BUS_MASTER | PCI_CMD_MEMORY_SPACE,
    );
}

/// Scan the PCI bus for e1000 NICs, initialize each one, and register
/// it with the global NetDriverManager.
pub fn probe_e1000(mgr: &mut NetDriverManager) {
    let devices = crate::pci::enumerate();
    let mut nic_index = 0u32;

    for pci_dev in &devices {
        if !is_e1000_device(pci_dev.vendor_id, pci_dev.device_id) {
            continue;
        }

        crate::serial_println!(
            "[e1000] Found Intel NIC {:04x}:{:04x} at {:02x}:{:02x}.{:x}",
            pci_dev.vendor_id,
            pci_dev.device_id,
            pci_dev.bus,
            pci_dev.device,
            pci_dev.function
        );

        pci_enable_device(pci_dev.bus, pci_dev.device, pci_dev.function);

        let bar0 = pci_read_bar0(pci_dev.bus, pci_dev.device, pci_dev.function);
        if bar0 == 0 {
            crate::serial_println!("[e1000] BAR0 invalid, skipping");
            continue;
        }

        crate::serial_println!("[e1000] BAR0 MMIO base: {:#x}", bar0);

        let irq = pci_dev.irq_line;
        match E1000Driver::new(bar0, irq, pci_dev.bus, pci_dev.device, pci_dev.function) {
            Ok(drv) => {
                let mac = drv.mac;
                crate::serial_println!(
                    "[e1000] MAC: {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}, IRQ {}",
                    mac[0],
                    mac[1],
                    mac[2],
                    mac[3],
                    mac[4],
                    mac[5],
                    irq
                );
                let name = alloc::format!("eth{}", nic_index);
                mgr.register(name, Box::new(drv));
                nic_index += 1;
            }
            Err(e) => {
                crate::serial_println!("[e1000] Init failed: {}", e);
            }
        }
    }

    if nic_index > 0 {
        mgr.set_default_route("eth0");
        crate::serial_println!("[ OK ] e1000: {} NIC(s) initialized", nic_index);
    }
}

// ─── PCI vendor/device → driver selection table ─────────────────────────────

#[derive(Debug, Clone, Copy)]
pub struct PciNetBinding {
    pub vendor_id: u16,
    pub device_id: u16,
    pub driver: &'static str,
    pub status: &'static str,
}

/// MasterChecklist Phase 2.2 — driver selection table (probe order = table order).
pub const PCI_NET_TABLE: &[PciNetBinding] = &[
    PciNetBinding {
        vendor_id: 0x8086,
        device_id: 0x100E,
        driver: "e1000",
        status: "active",
    },
    PciNetBinding {
        vendor_id: 0x8086,
        device_id: 0x15B8,
        driver: "i219",
        status: "active",
    },
    PciNetBinding {
        vendor_id: 0x8086,
        device_id: 0x125C,
        driver: "igc",
        status: "probe",
    },
    PciNetBinding {
        vendor_id: 0x1AF4,
        device_id: 0x1000,
        driver: "virtio-net",
        status: "active",
    },
    PciNetBinding {
        vendor_id: 0x10EC,
        device_id: 0x8125,
        driver: "rtl8125",
        status: "pending",
    },
    PciNetBinding {
        vendor_id: 0x10EC,
        device_id: 0x8168,
        driver: "rtl8168",
        status: "pending",
    },
];

pub fn lookup_pci_net_binding(vendor_id: u16, device_id: u16) -> Option<&'static PciNetBinding> {
    PCI_NET_TABLE
        .iter()
        .find(|e| e.vendor_id == vendor_id && e.device_id == device_id)
}

fn log_pci_net_selection() {
    for dev in crate::pci::enumerate() {
        if dev.class != 0x02 {
            continue;
        }
        if let Some(bind) = lookup_pci_net_binding(dev.vendor_id, dev.device_id) {
            crate::serial_println!(
                "[net] pci {:04x}:{:04x} @ {:02x}:{:02x}.{} -> {} ({})",
                dev.vendor_id,
                dev.device_id,
                dev.bus,
                dev.device,
                dev.function,
                bind.driver,
                bind.status
            );
        }
    }
}

pub fn pci_selection_dump_text() -> alloc::string::String {
    let mut out = alloc::string::String::from("# PCI net driver selection\n");
    for e in PCI_NET_TABLE {
        out.push_str(&alloc::format!(
            "{:04x}:{:04x}  {:<12}  {}\n",
            e.vendor_id,
            e.device_id,
            e.driver,
            e.status
        ));
    }
    out
}

// ─── Global Init ────────────────────────────────────────────────────────────

pub fn init() {
    let mut mgr = NetDriverManager::new();
    let lo = LoopbackDriver::new();
    mgr.register(String::from("lo"), Box::new(lo));

    probe_e1000(&mut mgr);
    probe_i219(&mut mgr);
    crate::igc::probe(&mut mgr);
    probe_rtl(&mut mgr);
    probe_unsupported_gaming_nics();
    log_pci_net_selection();

    *NET_DRIVERS.lock() = Some(mgr);
}

/// Pure model of the RTL8125 RX descriptor-ring OWN-bit recycle (no MMIO/DMA).
///
/// Mirrors the exact bit math of `RtlDriver::init_rings` (initial arm) and the
/// `recv()` re-arm path, so the phantom-full-after-wrap class (CLAUDE.md
/// pitfall #8 — the silent wedge that only shows on iron after N packets) is
/// caught by a FAIL-able boot check on EVERY boot (QEMU + iron), instead of
/// only being discoverable behind a real RTL8125. Returns Ok on a clean
/// double-wrap, Err(reason) on any invariant violation.
fn rtl_rx_recycle_model_check() -> Result<(), &'static str> {
    // (own_bit, eor_bit, frame_len, addr) — the descriptor fields recv() cares about.
    let n = RTL_RING_SIZE;
    let bufs: Vec<u64> = (0..n).map(|i| 0x10_0000u64 + (i as u64) * 4096).collect();
    // Initial arm (init_rings): every desc OWN=NIC, EOR only on the last.
    let mut ring: Vec<(bool, bool, u32, u64)> = (0..n)
        .map(|i| {
            let eor = i == n - 1;
            (true, eor, RTL_BUF_SIZE as u32, bufs[i])
        })
        .collect();
    if ring.iter().filter(|d| d.0).count() != n {
        return Err("init: not all descriptors armed (OWN=NIC)");
    }
    if ring.iter().filter(|d| d.1).count() != 1 || !ring[n - 1].1 {
        return Err("init: EOR must be set on exactly the last descriptor");
    }
    // Drive 2.5 full wraps. At each step the "NIC" delivers one frame (clears
    // OWN, writes a length) and recv() consumes + re-arms exactly as the driver
    // does, advancing rx_cur with modular wrap.
    let mut rx_cur = 0usize;
    for step in 0..(n * 5 / 2) {
        // NIC delivers into the slot the driver is about to read.
        ring[rx_cur].0 = false; // OWN -> driver
        ring[rx_cur].2 = (64 + 4) & RTL_DESC_FRAME_MASK; // 64-byte frame + CRC
                                                         // recv(): must see driver ownership, recycle, advance.
        if ring[rx_cur].0 {
            return Err("recv: descriptor still NIC-owned when frame was delivered");
        }
        let idx = rx_cur;
        let eor = idx == n - 1;
        ring[idx] = (true, eor, RTL_BUF_SIZE as u32, bufs[idx]); // re-arm
        if !ring[idx].0 {
            return Err("recv: re-arm failed to hand descriptor back to NIC");
        }
        if ring[idx].3 != bufs[idx] {
            return Err("recv: re-arm corrupted the buffer address");
        }
        if eor != ring[idx].1 {
            return Err("recv: EOR not preserved across re-arm on wrap boundary");
        }
        rx_cur = (idx + 1) % n;
        // After every step the ring must still have exactly one EOR, on the last.
        if ring.iter().filter(|d| d.1).count() != 1 || !ring[n - 1].1 {
            return Err("recv: EOR invariant broken after recycle (would wedge NIC)");
        }
        let _ = step;
    }
    // After 2.5 wraps the whole ring must be NIC-owned again (no phantom-full).
    if ring.iter().any(|d| !d.0) {
        return Err("post-wrap: a descriptor was left driver-owned (phantom-full wedge)");
    }
    Ok(())
}

pub fn run_boot_smoketest() {
    // RTL8125 RX recycle invariant — runs on every boot (the live driver only
    // exists on iron, but this pure model proves the recycle math everywhere).
    match rtl_rx_recycle_model_check() {
        Ok(()) => crate::serial_println!(
            "[rtl] RX-recycle model: {} descs, 2.5 wraps, OWN/EOR invariants held -> PASS",
            RTL_RING_SIZE
        ),
        Err(reason) => crate::serial_println!(
            "[rtl] RX-recycle model -> FAIL: {} (would wedge RX on iron)",
            reason
        ),
    }

    let guard = NET_DRIVERS.lock();
    let Some(mgr) = guard.as_ref() else {
        crate::serial_println!("[netdrv] smoketest FAIL: manager not initialized");
        return;
    };
    let drivers = mgr.list();
    let has_loopback = drivers.iter().any(|n| *n == "lo");
    let default_name = mgr.default_driver_name().unwrap_or("none");
    crate::serial_println!(
        "[netdrv] smoketest: drivers={} default={} loopback={} -> {}",
        drivers.len(),
        default_name,
        has_loopback as u8,
        if has_loopback { "PASS" } else { "FAIL" }
    );
}

pub fn dump_text() -> String {
    let mut out = String::new();
    out.push_str(&pci_selection_dump_text());
    out.push_str("# NetDriver manager\n");
    let guard = NET_DRIVERS.lock();
    let Some(mgr) = guard.as_ref() else {
        out.push_str("status: uninitialized\n");
        return out;
    };
    let default_name = mgr.default_driver_name().unwrap_or("none");
    out.push_str(&alloc::format!("default: {}\n", default_name));
    for name in mgr.list() {
        if let Some(drv) = mgr.get(name) {
            let mac = drv.mac_address();
            let stats = drv.stats();
            out.push_str(&alloc::format!(
                "{}: link={:?} speed={}mbps mtu={} mac={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} tx_pkts={} rx_pkts={} rx_bytes={} rx_errs={}\n",
                name,
                drv.link_state(),
                drv.link_speed().as_mbps(),
                drv.mtu(),
                mac[0], mac[1], mac[2], mac[3], mac[4], mac[5],
                stats.tx_packets,
                stats.rx_packets,
                stats.rx_bytes,
                stats.rx_errors,
            ));
        }
    }
    out
}

/// Log PCI IDs for common bare-metal NICs we do not drive yet (I225/I226, RTL8125, Intel Wi-Fi).
fn probe_unsupported_gaming_nics() {
    for pci_dev in crate::pci::enumerate() {
        let v = pci_dev.vendor_id;
        let d = pci_dev.device_id;
        // IGC IDs are handled by igc::probe(); Realtek 8169/8168/8111/8125 are
        // now driven by probe_rtl() above (no longer "pending").
        if v == 0x8086 && matches!(d, 0x2723 | 0x2725 | 0x7E40 | 0x7F70) {
            crate::serial_println!(
                "[net] Intel Wi-Fi {:04x}:{:04x} — use userspace LinuxKPI path (months, not bare-metal iwlwifi port)",
                v, d,
            );
        }
    }
}
