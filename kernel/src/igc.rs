//! Intel IGC (I225-V / I226-V, 2.5 GbE) driver.
//!
//! The I225/I226 is — alongside the Realtek RTL8125 — the 2.5 GbE NIC on most
//! current gaming/enthusiast motherboards. Unlike the legacy e1000 descriptor
//! model, igc (like igb) uses the Intel "advanced" 16-byte descriptors and a
//! per-queue enable (RXDCTL/TXDCTL). This module implements reset, MAC, the
//! advanced RX/TX rings, and the full `NetDriver` trait. MasterChecklist 2.2.
//!
//! NOTE: QEMU does not emulate igc (only e1000/e1000e/igb), so the DATA path is
//! iron-verified, not QEMU-verified; on QEMU the probe finds no device.

#![allow(dead_code)]

use alloc::boxed::Box;
use alloc::vec::Vec;

use crate::net_drivers::{
    alloc_dma_page, dma_buf, DmaPage, LinkSpeed, LinkState, NetDriver, NetDriverError,
    NetDriverManager, NetDriverStats, OffloadCapabilities,
};

// ─── Register offsets (igc / igb register file) ──────────────────────────────
const IGC_CTRL: u32 = 0x0000;
const IGC_STATUS: u32 = 0x0008;
const IGC_RCTL: u32 = 0x0100;
const IGC_TCTL: u32 = 0x0400;
const IGC_RAL0: u32 = 0x5400;
const IGC_RAH0: u32 = 0x5404;
// RX queue 0
const IGC_RDBAL0: u32 = 0xC000;
const IGC_RDBAH0: u32 = 0xC004;
const IGC_RDLEN0: u32 = 0xC008;
const IGC_SRRCTL0: u32 = 0xC00C;
const IGC_RDH0: u32 = 0xC010;
const IGC_RDT0: u32 = 0xC018;
const IGC_RXDCTL0: u32 = 0xC028;
// TX queue 0
const IGC_TDBAL0: u32 = 0xE000;
const IGC_TDBAH0: u32 = 0xE004;
const IGC_TDLEN0: u32 = 0xE008;
const IGC_TDH0: u32 = 0xE010;
const IGC_TDT0: u32 = 0xE018;
const IGC_TXDCTL0: u32 = 0xE028;
// Interrupt mask clear (mask everything — we poll).
const IGC_IMC: u32 = 0x150C;

// CTRL bits
const IGC_CTRL_SLU: u32 = 1 << 6; // set link up
const IGC_CTRL_DEV_RST: u32 = 1 << 26;
// STATUS bits
const IGC_STATUS_LU: u32 = 1 << 1;
const IGC_STATUS_SPEED_MASK: u32 = 0b11 << 6;
const IGC_STATUS_SPEED_10: u32 = 0b00 << 6;
const IGC_STATUS_SPEED_100: u32 = 0b01 << 6;
const IGC_STATUS_SPEED_1000: u32 = 0b10 << 6;
const IGC_STATUS_SPEED_2500: u32 = 0b11 << 6;
// RCTL bits
const IGC_RCTL_EN: u32 = 1 << 1;
const IGC_RCTL_UPE: u32 = 1 << 3; // unicast promiscuous
const IGC_RCTL_MPE: u32 = 1 << 4; // multicast promiscuous
const IGC_RCTL_BAM: u32 = 1 << 15; // broadcast accept
const IGC_RCTL_SECRC: u32 = 1 << 26; // strip Ethernet CRC
                                     // TCTL bits
const IGC_TCTL_EN: u32 = 1 << 1;
const IGC_TCTL_PSP: u32 = 1 << 3; // pad short packets
                                  // SRRCTL bits
const IGC_SRRCTL_BSIZE_2K: u32 = 2; // BSIZEPACKET in 1 KiB units
const IGC_SRRCTL_DESCTYPE_ADV: u32 = 1 << 25; // advanced, one buffer
const IGC_SRRCTL_DROP_EN: u32 = 1 << 31;
// per-queue enable
const IGC_RXDCTL_ENABLE: u32 = 1 << 25;
const IGC_TXDCTL_ENABLE: u32 = 1 << 25;
const IGC_RAH_AV: u32 = 1 << 31; // address valid

// Advanced TX data-descriptor cmd_type_len fields
const IGC_TXD_DTYP_DATA: u32 = 0x3 << 20;
const IGC_TXD_DCMD_EOP: u32 = 1 << 24;
const IGC_TXD_DCMD_IFCS: u32 = 1 << 25;
const IGC_TXD_DCMD_RS: u32 = 1 << 27;
const IGC_TXD_DCMD_DEXT: u32 = 1 << 29;
// Advanced descriptor writeback status (low dword)
const IGC_RXD_STAT_DD: u32 = 1 << 0;
const IGC_RXD_STAT_EOP: u32 = 1 << 1;
const IGC_TXD_STAT_DD: u32 = 1 << 0;

const IGC_VENDOR: u16 = 0x8086;
const IGC_DEVICE_IDS: [u16; 6] = [0x15F2, 0x15F3, 0x125B, 0x125C, 0x125D, 0x125E];

const IGC_RING_SIZE: usize = 64; // descriptors (16 B each → 1 KiB, page-aligned)
const IGC_BUF_SIZE: usize = 2048; // per-buffer

fn is_igc(vendor: u16, device: u16) -> bool {
    vendor == IGC_VENDOR && IGC_DEVICE_IDS.contains(&device)
}

fn pci_read_bar0(bus: u8, dev: u8, func: u8) -> u64 {
    let bar0_raw = crate::pci::read_config_32(bus, dev, func, 0x10);
    if bar0_raw & 1 != 0 {
        return 0;
    }
    let base_lo = (bar0_raw & !0xF) as u64;
    match (bar0_raw >> 1) & 0x03 {
        0x00 => base_lo,
        0x02 => {
            let hi = crate::pci::read_config_32(bus, dev, func, 0x14);
            base_lo | ((hi as u64) << 32)
        }
        _ => 0,
    }
}

// SAFETY: descriptor rings + buffers are frame-allocator DMA pages with stable
// phys/virt mappings; the driver runs from BSP init / the net path only.
unsafe impl Send for IgcDriver {}

pub struct IgcDriver {
    bar0: u64,
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
    enabled: bool,
    iommu_domain: Option<u16>,
}

impl IgcDriver {
    pub fn new(
        bar0_phys: u64,
        irq: u8,
        device_id: u16,
        bus: u8,
        dev: u8,
        func: u8,
    ) -> Result<Self, NetDriverError> {
        let offset = crate::memory::PHYS_MEM_OFFSET
            .get()
            .ok_or(NetDriverError::HardwareError)?
            .as_u64();
        let rx_desc = alloc_dma_page()?;
        let tx_desc = alloc_dma_page()?;
        let mut rx_buffers = Vec::with_capacity(IGC_RING_SIZE);
        let mut tx_buffers = Vec::with_capacity(IGC_RING_SIZE);
        for _ in 0..IGC_RING_SIZE {
            rx_buffers.push(alloc_dma_page()?);
            tx_buffers.push(alloc_dma_page()?);
        }
        let mut drv = Self {
            bar0: offset + bar0_phys,
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
            enabled: false,
            iommu_domain: None,
        };

        drv.reset();
        drv.read_mac();
        drv.set_mac_filter();
        drv.init_rx();
        drv.init_tx();
        drv.detect_link();
        drv.enabled = true;

        let mut regions = alloc::vec![(drv.rx_desc.phys, 4096), (drv.tx_desc.phys, 4096)];
        for p in drv.rx_buffers.iter().chain(drv.tx_buffers.iter()) {
            regions.push((p.phys, 4096));
        }
        drv.iommu_domain = crate::iommu::sandbox_device_dma(bus, dev, func, &regions);

        Ok(drv)
    }

    fn read32(&self, reg: u32) -> u32 {
        unsafe { core::ptr::read_volatile((self.bar0 + reg as u64) as *const u32) }
    }
    fn write32(&self, reg: u32, val: u32) {
        unsafe { core::ptr::write_volatile((self.bar0 + reg as u64) as *mut u32, val) }
    }

    fn reset(&mut self) {
        self.write32(IGC_IMC, 0xFFFF_FFFF); // mask all interrupts (polled driver)
        self.write32(IGC_CTRL, self.read32(IGC_CTRL) | IGC_CTRL_DEV_RST);
        for _ in 0..1_000_000 {
            if self.read32(IGC_CTRL) & IGC_CTRL_DEV_RST == 0 {
                break;
            }
            core::hint::spin_loop();
        }
        self.write32(IGC_IMC, 0xFFFF_FFFF);
    }

    fn read_mac(&mut self) {
        let ral = self.read32(IGC_RAL0);
        let rah = self.read32(IGC_RAH0);
        self.mac = [
            ral as u8,
            (ral >> 8) as u8,
            (ral >> 16) as u8,
            (ral >> 24) as u8,
            rah as u8,
            (rah >> 8) as u8,
        ];
    }

    fn set_mac_filter(&self) {
        let lo = (self.mac[0] as u32)
            | ((self.mac[1] as u32) << 8)
            | ((self.mac[2] as u32) << 16)
            | ((self.mac[3] as u32) << 24);
        let hi = (self.mac[4] as u32) | ((self.mac[5] as u32) << 8) | IGC_RAH_AV;
        self.write32(IGC_RAL0, lo);
        self.write32(IGC_RAH0, hi);
    }

    /// Pointer to RX descriptor `i` (16-byte advanced descriptor).
    fn rx_desc_ptr(&self, i: usize) -> u64 {
        self.rx_desc.virt + (i * 16) as u64
    }
    fn tx_desc_ptr(&self, i: usize) -> u64 {
        self.tx_desc.virt + (i * 16) as u64
    }

    fn init_rx(&mut self) {
        // Advanced RX read descriptor: pkt_addr @0, hdr_addr @8 (no header split).
        for i in 0..IGC_RING_SIZE {
            unsafe {
                core::ptr::write_volatile(self.rx_desc_ptr(i) as *mut u64, self.rx_buffers[i].phys);
                core::ptr::write_volatile((self.rx_desc_ptr(i) + 8) as *mut u64, 0);
            }
        }
        let phys = self.rx_desc.phys;
        self.write32(IGC_RDBAL0, phys as u32);
        self.write32(IGC_RDBAH0, (phys >> 32) as u32);
        self.write32(IGC_RDLEN0, (IGC_RING_SIZE * 16) as u32);
        self.write32(
            IGC_SRRCTL0,
            IGC_SRRCTL_BSIZE_2K | IGC_SRRCTL_DESCTYPE_ADV | IGC_SRRCTL_DROP_EN,
        );
        self.write32(IGC_RDH0, 0);
        self.write32(IGC_RDT0, (IGC_RING_SIZE - 1) as u32);
        // Enable the queue and wait for the controller to ack.
        self.write32(IGC_RXDCTL0, IGC_RXDCTL_ENABLE);
        for _ in 0..100_000 {
            if self.read32(IGC_RXDCTL0) & IGC_RXDCTL_ENABLE != 0 {
                break;
            }
            core::hint::spin_loop();
        }
        let mut rctl = IGC_RCTL_EN | IGC_RCTL_BAM | IGC_RCTL_SECRC;
        if self.promisc {
            rctl |= IGC_RCTL_UPE | IGC_RCTL_MPE;
        }
        self.write32(IGC_RCTL, rctl);
    }

    fn init_tx(&mut self) {
        // Driver-owned until a send; zero the ring.
        for i in 0..IGC_RING_SIZE {
            unsafe {
                core::ptr::write_volatile(self.tx_desc_ptr(i) as *mut u64, 0);
                core::ptr::write_volatile((self.tx_desc_ptr(i) + 8) as *mut u64, 0);
            }
        }
        let phys = self.tx_desc.phys;
        self.write32(IGC_TDBAL0, phys as u32);
        self.write32(IGC_TDBAH0, (phys >> 32) as u32);
        self.write32(IGC_TDLEN0, (IGC_RING_SIZE * 16) as u32);
        self.write32(IGC_TDH0, 0);
        self.write32(IGC_TDT0, 0);
        self.write32(IGC_TXDCTL0, IGC_TXDCTL_ENABLE);
        for _ in 0..100_000 {
            if self.read32(IGC_TXDCTL0) & IGC_TXDCTL_ENABLE != 0 {
                break;
            }
            core::hint::spin_loop();
        }
        self.write32(IGC_TCTL, IGC_TCTL_EN | IGC_TCTL_PSP);
    }

    fn detect_link(&mut self) {
        self.write32(IGC_CTRL, self.read32(IGC_CTRL) | IGC_CTRL_SLU);
        let status = self.read32(IGC_STATUS);
        self.link_up = status & IGC_STATUS_LU != 0;
        self.speed = match status & IGC_STATUS_SPEED_MASK {
            IGC_STATUS_SPEED_10 => LinkSpeed::Mbps10,
            IGC_STATUS_SPEED_100 => LinkSpeed::Mbps100,
            IGC_STATUS_SPEED_1000 => LinkSpeed::Gbps1,
            IGC_STATUS_SPEED_2500 => LinkSpeed::Gbps2_5,
            _ => LinkSpeed::Unknown,
        };
    }
}

impl NetDriver for IgcDriver {
    fn name(&self) -> &str {
        "igc"
    }
    fn mac_address(&self) -> [u8; 6] {
        self.mac
    }
    fn mtu(&self) -> u32 {
        self.mtu
    }
    fn set_mtu(&mut self, mtu: u32) -> Result<(), NetDriverError> {
        if mtu < 68 || mtu as usize > IGC_BUF_SIZE - 18 {
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
        if packet.len() > IGC_BUF_SIZE || packet.len() > self.mtu as usize + 18 {
            return Err(NetDriverError::BufferTooLarge);
        }
        let idx = self.tx_cur;
        // If the slot we're about to use is still owned (no DD on a prior send),
        // the ring is full.
        let prev_status =
            unsafe { core::ptr::read_volatile((self.tx_desc_ptr(idx) + 12) as *const u32) };
        let in_flight =
            unsafe { core::ptr::read_volatile(self.tx_desc_ptr(idx) as *const u64) } != 0;
        if in_flight && prev_status & IGC_TXD_STAT_DD == 0 {
            self.stats.tx_dropped += 1;
            return Err(NetDriverError::QueueFull);
        }

        dma_buf(&self.tx_buffers[idx], IGC_BUF_SIZE)[..packet.len()].copy_from_slice(packet);
        let len = packet.len() as u32;
        let cmd_type_len = (len & 0xFFFF)
            | IGC_TXD_DTYP_DATA
            | IGC_TXD_DCMD_EOP
            | IGC_TXD_DCMD_IFCS
            | IGC_TXD_DCMD_RS
            | IGC_TXD_DCMD_DEXT;
        let olinfo_status = len << 14; // PAYLEN
        unsafe {
            core::ptr::write_volatile(self.tx_desc_ptr(idx) as *mut u64, self.tx_buffers[idx].phys);
            core::ptr::write_volatile((self.tx_desc_ptr(idx) + 8) as *mut u32, cmd_type_len);
            core::ptr::write_volatile((self.tx_desc_ptr(idx) + 12) as *mut u32, olinfo_status);
        }

        self.tx_cur = (idx + 1) % IGC_RING_SIZE;
        self.write32(IGC_TDT0, self.tx_cur as u32);

        // Bounded wait for the descriptor-done writeback.
        for _ in 0..1_000_000 {
            let st =
                unsafe { core::ptr::read_volatile((self.tx_desc_ptr(idx) + 12) as *const u32) };
            if st & IGC_TXD_STAT_DD != 0 {
                break;
            }
            core::hint::spin_loop();
        }
        self.stats.tx_packets += 1;
        self.stats.tx_bytes += packet.len() as u64;
        Ok(())
    }

    fn recv(&mut self) -> Option<Vec<u8>> {
        let idx = self.rx_cur;
        // Advanced RX writeback: status/error in the low dword of qword 1
        // (offset 8), length at offset 12.
        let status = unsafe { core::ptr::read_volatile((self.rx_desc_ptr(idx) + 8) as *const u32) };
        if status & IGC_RXD_STAT_DD == 0 {
            return None;
        }
        let len = unsafe { core::ptr::read_volatile((self.rx_desc_ptr(idx) + 12) as *const u16) };
        let out = if status & IGC_RXD_STAT_EOP != 0 {
            let n = (len as usize).min(IGC_BUF_SIZE);
            let data = dma_buf(&self.rx_buffers[idx], n).to_vec();
            self.stats.rx_packets += 1;
            self.stats.rx_bytes += n as u64;
            Some(data)
        } else {
            self.stats.rx_errors += 1;
            None
        };
        // Re-arm: rewrite the read descriptor (pkt_addr @0, hdr_addr @8 = 0).
        unsafe {
            core::ptr::write_volatile(self.rx_desc_ptr(idx) as *mut u64, self.rx_buffers[idx].phys);
            core::ptr::write_volatile((self.rx_desc_ptr(idx) + 8) as *mut u64, 0);
        }
        // Hand the slot back to the NIC by advancing the tail.
        self.write32(IGC_RDT0, idx as u32);
        self.rx_cur = (idx + 1) % IGC_RING_SIZE;
        out
    }

    fn enable(&mut self) -> Result<(), NetDriverError> {
        if self.enabled {
            return Err(NetDriverError::AlreadyEnabled);
        }
        self.write32(IGC_RCTL, self.read32(IGC_RCTL) | IGC_RCTL_EN);
        self.write32(IGC_TCTL, self.read32(IGC_TCTL) | IGC_TCTL_EN);
        self.enabled = true;
        Ok(())
    }
    fn disable(&mut self) -> Result<(), NetDriverError> {
        if !self.enabled {
            return Err(NetDriverError::AlreadyDisabled);
        }
        self.write32(IGC_RCTL, self.read32(IGC_RCTL) & !IGC_RCTL_EN);
        self.write32(IGC_TCTL, self.read32(IGC_TCTL) & !IGC_TCTL_EN);
        self.write32(IGC_IMC, 0xFFFF_FFFF);
        self.enabled = false;
        Ok(())
    }
    fn stats(&self) -> NetDriverStats {
        self.stats
    }
    fn set_promiscuous(&mut self, enabled: bool) {
        self.promisc = enabled;
        let mut rctl = self.read32(IGC_RCTL);
        if enabled {
            rctl |= IGC_RCTL_UPE | IGC_RCTL_MPE;
        } else {
            rctl &= !(IGC_RCTL_UPE | IGC_RCTL_MPE);
        }
        self.write32(IGC_RCTL, rctl);
    }
    fn add_multicast(&mut self, _addr: [u8; 6]) {
        // Broadcast/multicast accepted via RCTL.BAM; per-hash MTA filtering is an
        // iron-tuning refinement.
    }
    fn remove_multicast(&mut self, _addr: [u8; 6]) {}
    fn supports_offload(&self) -> OffloadCapabilities {
        OffloadCapabilities::default()
    }
}

/// Scan PCI for Intel IGC controllers (I225/I226), bring each up, and register
/// it with the manager as a full NetDriver. MasterChecklist Phase 2.2.
pub fn probe(mgr: &mut NetDriverManager) {
    let start = mgr.list().iter().filter(|n| n.starts_with("eth")).count() as u32;
    let mut idx = start;
    for pci_dev in crate::pci::enumerate() {
        if !is_igc(pci_dev.vendor_id, pci_dev.device_id) {
            continue;
        }
        crate::pci::enable_bus_mastering(&pci_dev);
        let bar0 = pci_read_bar0(pci_dev.bus, pci_dev.device, pci_dev.function);
        if bar0 == 0 {
            crate::serial_println!(
                "[igc] {:02x}:{:02x}.{} BAR0 invalid",
                pci_dev.bus,
                pci_dev.device,
                pci_dev.function
            );
            continue;
        }
        match IgcDriver::new(
            bar0,
            pci_dev.irq_line,
            pci_dev.device_id,
            pci_dev.bus,
            pci_dev.device,
            pci_dev.function,
        ) {
            Ok(drv) => {
                let mac = drv.mac;
                crate::serial_println!(
                    "[igc] eth{} I225/I226 {:04x} MAC {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} link={} speed={}mbps",
                    idx,
                    pci_dev.device_id,
                    mac[0], mac[1], mac[2], mac[3], mac[4], mac[5],
                    drv.link_up as u8,
                    drv.speed.as_mbps(),
                );
                mgr.register(alloc::format!("eth{}", idx), Box::new(drv));
                idx += 1;
            }
            Err(e) => crate::serial_println!("[igc] init failed: {}", e),
        }
    }
    if idx > start {
        if mgr.default_driver_name().is_none() {
            mgr.set_default_route("eth0");
        }
        crate::serial_println!("[ OK ] igc: {} NIC(s) initialized", idx - start);
    }
}

pub fn init() {
    crate::serial_println!("[ OK ] IGC (I225/I226) probe ready");
}
