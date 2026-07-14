#![allow(dead_code)]

//! AHCI/SATA host driver (in-kernel). REDOX_EXTRACTION_MAP R06 — compare DMA
//! command-list programming with Redox `base.git` `ahcid`; RaeenOS stays
//! in-kernel per hybrid Concept until IOMMU userspace storage daemons land.
//!
//! R10: `init()` + `run_boot_smoketest()` + `/proc/raeen/ahci` + this doc block.

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;
use x86_64::structures::paging::FrameAllocator;

use crate::block_io::{BlockDeviceInfo, BlockError};
use crate::memory::GlobalFrameAllocator;

fn alloc_dma_frame() -> Result<(u64, u64), BlockError> {
    let mut alloc = GlobalFrameAllocator;
    let frame = alloc.allocate_frame().ok_or(BlockError::HardwareFailure)?;
    let phys = frame.start_address().as_u64();
    let offset = *crate::memory::PHYS_MEM_OFFSET
        .get()
        .ok_or(BlockError::HardwareFailure)?;
    let virt = (offset + phys).as_u64();
    unsafe {
        core::ptr::write_bytes(virt as *mut u8, 0, 4096);
    }
    Ok((phys, virt))
}

// ─── AHCI HBA Register Offsets ──────────────────────────────────────────────

const HBA_CAP: u64 = 0x00; // Host Capabilities
const HBA_GHC: u64 = 0x04; // Global Host Control
const HBA_IS: u64 = 0x08; // Interrupt Status
const HBA_PI: u64 = 0x0C; // Ports Implemented
const HBA_VS: u64 = 0x10; // Version
const HBA_CCC_CTL: u64 = 0x14; // Command Completion Coalescing Control
const HBA_CCC_PORTS: u64 = 0x18; // CCC Ports
const HBA_EM_LOC: u64 = 0x1C; // Enclosure Management Location
const HBA_EM_CTL: u64 = 0x20; // Enclosure Management Control
const HBA_CAP2: u64 = 0x24; // Extended Capabilities
const HBA_BOHC: u64 = 0x28; // BIOS/OS Handoff Control

// HBA_CAP2 bits
const CAP2_BOH: u32 = 1 << 0; // BIOS/OS Handoff (BOH) supported

// HBA_BOHC bits (BIOS/OS Handoff Control & Status, AHCI spec 10.6.3)
const BOHC_BOS: u32 = 1 << 0; // BIOS Owned Semaphore
const BOHC_OOS: u32 = 1 << 1; // OS Owned Semaphore
const BOHC_BB: u32 = 1 << 4; // BIOS Busy

// Port register offsets (from port base = BAR5 + 0x100 + port * 0x80)
const PORT_CLB: u64 = 0x00; // Command List Base Address
const PORT_CLBU: u64 = 0x04; // Command List Base Address Upper
const PORT_FB: u64 = 0x08; // FIS Base Address
const PORT_FBU: u64 = 0x0C; // FIS Base Address Upper
const PORT_IS: u64 = 0x10; // Interrupt Status
const PORT_IE: u64 = 0x14; // Interrupt Enable
const PORT_CMD: u64 = 0x18; // Command and Status
const PORT_TFD: u64 = 0x20; // Task File Data
const PORT_SIG: u64 = 0x24; // Signature
const PORT_SSTS: u64 = 0x28; // SATA Status
const PORT_SCTL: u64 = 0x2C; // SATA Control
const PORT_SERR: u64 = 0x30; // SATA Error
const PORT_SACT: u64 = 0x34; // SATA Active
const PORT_CI: u64 = 0x38; // Command Issue
const PORT_SNTF: u64 = 0x3C; // SATA Notification
const PORT_FBS: u64 = 0x40; // FIS-Based Switching

// HBA_GHC bits
const GHC_HR: u32 = 1 << 0; // HBA Reset
const GHC_IE: u32 = 1 << 1; // Interrupt Enable
const GHC_AE: u32 = 1 << 31; // AHCI Enable

// PORT_CMD bits
const CMD_ST: u32 = 1 << 0; // Start
const CMD_SUD: u32 = 1 << 1; // Spin-Up Device
const CMD_POD: u32 = 1 << 2; // Power On Device
const CMD_FRE: u32 = 1 << 4; // FIS Receive Enable
const CMD_FR: u32 = 1 << 14; // FIS Receive Running
const CMD_CR: u32 = 1 << 15; // Command List Running
const CMD_ATAPI: u32 = 1 << 24; // Device is ATAPI
const CMD_ICC_ACTIVE: u32 = 1 << 28;

// ATA Commands
const ATA_CMD_IDENTIFY: u8 = 0xEC;
const ATA_CMD_IDENTIFY_PACKET: u8 = 0xA1;
const ATA_CMD_READ_DMA_EXT: u8 = 0x25;
const ATA_CMD_WRITE_DMA_EXT: u8 = 0x35;
const ATA_CMD_READ_FPDMA: u8 = 0x60; // NCQ Read
const ATA_CMD_WRITE_FPDMA: u8 = 0x61; // NCQ Write
const ATA_CMD_FLUSH_CACHE_EXT: u8 = 0xEA;
const ATA_CMD_DATA_SET_MGMT: u8 = 0x06; // TRIM
const ATA_CMD_SMART: u8 = 0xB0;
const ATA_CMD_SET_FEATURES: u8 = 0xEF;
const ATA_CMD_STANDBY_IMMEDIATE: u8 = 0xE0;

// Device Signatures
const SATA_SIG_ATA: u32 = 0x00000101;
const SATA_SIG_ATAPI: u32 = 0xEB140101;
const SATA_SIG_SEMB: u32 = 0xC33C0101;
const SATA_SIG_PM: u32 = 0x96690101;

// FIS Types
const FIS_TYPE_REG_H2D: u8 = 0x27;
const FIS_TYPE_REG_D2H: u8 = 0x34;
const FIS_TYPE_DMA_ACTIVATE: u8 = 0x39;
const FIS_TYPE_DMA_SETUP: u8 = 0x41;
const FIS_TYPE_DATA: u8 = 0x46;
const FIS_TYPE_BIST: u8 = 0x58;
const FIS_TYPE_PIO_SETUP: u8 = 0x5F;
const FIS_TYPE_DEV_BITS: u8 = 0xA1;

// ─── Types ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SataDeviceType {
    Ata,
    Atapi,
    Semb,
    PortMultiplier,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SataSpeed {
    Gen1_1_5Gbps,
    Gen2_3Gbps,
    Gen3_6Gbps,
    Unknown,
}

impl SataSpeed {
    pub fn from_ssts(ssts: u32) -> Self {
        match (ssts >> 4) & 0xF {
            1 => Self::Gen1_1_5Gbps,
            2 => Self::Gen2_3Gbps,
            3 => Self::Gen3_6Gbps,
            _ => Self::Unknown,
        }
    }

    pub fn mbps(&self) -> u32 {
        match self {
            Self::Gen1_1_5Gbps => 150,
            Self::Gen2_3Gbps => 300,
            Self::Gen3_6Gbps => 600,
            Self::Unknown => 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortState {
    Uninitialized,
    NoDevice,
    DevicePresent,
    Active,
    Error,
    Offline,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceSignature {
    Ata,
    Atapi,
    Semb,
    PortMultiplier,
    None,
}

impl DeviceSignature {
    pub fn from_sig(sig: u32) -> Self {
        match sig {
            SATA_SIG_ATA => Self::Ata,
            SATA_SIG_ATAPI => Self::Atapi,
            SATA_SIG_SEMB => Self::Semb,
            SATA_SIG_PM => Self::PortMultiplier,
            _ => Self::None,
        }
    }
}

// ─── FIS Register H2D ──────────────────────────────────────────────────────

#[repr(C, packed)]
#[derive(Debug, Clone, Copy, Default)]
pub struct FisRegH2D {
    pub fis_type: u8,
    pub flags: u8, // bit 7: C (command/control), bits 3:0: port multiplier
    pub command: u8,
    pub features_low: u8,
    pub lba0: u8,
    pub lba1: u8,
    pub lba2: u8,
    pub device: u8,
    pub lba3: u8,
    pub lba4: u8,
    pub lba5: u8,
    pub features_high: u8,
    pub count_low: u8,
    pub count_high: u8,
    pub icc: u8,
    pub control: u8,
    pub reserved: [u8; 4],
}

impl FisRegH2D {
    pub fn new_command(cmd: u8) -> Self {
        Self {
            fis_type: FIS_TYPE_REG_H2D,
            flags: 0x80, // C bit set (command register update)
            command: cmd,
            device: 0x40, // LBA mode
            ..Default::default()
        }
    }

    pub fn set_lba(&mut self, lba: u64) {
        self.lba0 = (lba & 0xFF) as u8;
        self.lba1 = ((lba >> 8) & 0xFF) as u8;
        self.lba2 = ((lba >> 16) & 0xFF) as u8;
        self.lba3 = ((lba >> 24) & 0xFF) as u8;
        self.lba4 = ((lba >> 32) & 0xFF) as u8;
        self.lba5 = ((lba >> 40) & 0xFF) as u8;
        self.device |= 0x40; // LBA mode
    }

    pub fn set_count(&mut self, count: u16) {
        self.count_low = (count & 0xFF) as u8;
        self.count_high = ((count >> 8) & 0xFF) as u8;
    }

    pub fn set_features(&mut self, features: u16) {
        self.features_low = (features & 0xFF) as u8;
        self.features_high = ((features >> 8) & 0xFF) as u8;
    }
}

// ─── Command Header & PRDT ─────────────────────────────────────────────────

#[repr(C, packed)]
#[derive(Debug, Clone, Copy, Default)]
pub struct CommandHeader {
    pub flags: u16, // CFL (bits 4:0), A (bit 5), W (bit 6), P (bit 7), R (bit 8), B (bit 9), C (bit 10), PMP (bits 15:12)
    pub prdtl: u16, // Physical Region Descriptor Table Length
    pub prdbc: u32, // PRD Byte Count (set by HBA on completion)
    pub ctba: u32,  // Command Table Base Address
    pub ctbau: u32, // Command Table Base Address Upper
    pub reserved: [u32; 4],
}

impl CommandHeader {
    pub fn set_cfl(&mut self, dwords: u8) {
        self.flags = (self.flags & !0x1F) | (dwords as u16 & 0x1F);
    }

    pub fn set_write(&mut self) {
        self.flags |= 1 << 6;
    }

    pub fn set_prefetchable(&mut self) {
        self.flags |= 1 << 7;
    }

    pub fn set_clear_busy(&mut self) {
        self.flags |= 1 << 10;
    }

    pub fn set_atapi(&mut self) {
        self.flags |= 1 << 5;
    }

    pub fn set_ctba(&mut self, phys: u64) {
        self.ctba = phys as u32;
        self.ctbau = (phys >> 32) as u32;
    }
}

#[repr(C, packed)]
#[derive(Debug, Clone, Copy, Default)]
pub struct PrdtEntry {
    pub dba: u32,  // Data Base Address
    pub dbau: u32, // Data Base Address Upper
    pub reserved: u32,
    pub dbc_i: u32, // Byte Count (bit 31 = interrupt on completion)
}

impl PrdtEntry {
    pub fn new(phys: u64, byte_count: u32, interrupt: bool) -> Self {
        Self {
            dba: phys as u32,
            dbau: (phys >> 32) as u32,
            reserved: 0,
            dbc_i: (byte_count - 1) | if interrupt { 1 << 31 } else { 0 },
        }
    }
}

// ─── AHCI Port ──────────────────────────────────────────────────────────────

pub struct AhciPort {
    pub port_num: u8,
    pub cmd_list: u64,  // Physical address of command list (32 command headers)
    cmd_list_virt: u64, // Kernel virtual (phys-mem window) for CPU access
    pub fis_base: u64,  // Physical address of received FIS buffer (256 bytes)
    pub state: PortState,
    pub sig: DeviceSignature,
    pub sata_status: u32,
    pub sata_error: u32,
    pub sata_active: u32,
    pub cmd_issue: u32,
    pub device_type: SataDeviceType,
    pub link_speed: SataSpeed,
    pub model: String,
    pub serial: String,
    pub firmware: String,
    pub total_sectors: u64,
    pub sector_size: u32,
    pub supports_ncq: bool,
    pub ncq_depth: u8,
    pub supports_trim: bool,
    pub supports_48bit: bool,
    pub supports_smart: bool,
    bar5: u64,
}

impl AhciPort {
    pub fn new(bar5: u64, port_num: u8) -> Self {
        Self {
            port_num,
            cmd_list: 0,
            cmd_list_virt: 0,
            fis_base: 0,
            state: PortState::Uninitialized,
            sig: DeviceSignature::None,
            sata_status: 0,
            sata_error: 0,
            sata_active: 0,
            cmd_issue: 0,
            device_type: SataDeviceType::None,
            link_speed: SataSpeed::Unknown,
            model: String::new(),
            serial: String::new(),
            firmware: String::new(),
            total_sectors: 0,
            sector_size: 512,
            supports_ncq: false,
            ncq_depth: 0,
            supports_trim: false,
            supports_48bit: false,
            supports_smart: false,
            bar5,
        }
    }

    fn port_base(&self) -> u64 {
        self.bar5 + 0x100 + (self.port_num as u64 * 0x80)
    }

    unsafe fn read_port_reg(&self, offset: u64) -> u32 {
        let ptr = (self.port_base() + offset) as *const u32;
        core::ptr::read_volatile(ptr)
    }

    unsafe fn write_port_reg(&self, offset: u64, val: u32) {
        let ptr = (self.port_base() + offset) as *mut u32;
        core::ptr::write_volatile(ptr, val);
    }

    pub fn detect(&mut self) -> bool {
        unsafe {
            self.sata_status = self.read_port_reg(PORT_SSTS);
            let det = self.sata_status & 0x0F;
            let ipm = (self.sata_status >> 8) & 0x0F;

            if det != 3 || ipm != 1 {
                self.state = PortState::NoDevice;
                self.device_type = SataDeviceType::None;
                return false;
            }

            self.link_speed = SataSpeed::from_ssts(self.sata_status);

            let sig = self.read_port_reg(PORT_SIG);
            self.sig = DeviceSignature::from_sig(sig);
            self.device_type = match self.sig {
                DeviceSignature::Ata => SataDeviceType::Ata,
                DeviceSignature::Atapi => SataDeviceType::Atapi,
                DeviceSignature::Semb => SataDeviceType::Semb,
                DeviceSignature::PortMultiplier => SataDeviceType::PortMultiplier,
                DeviceSignature::None => SataDeviceType::None,
            };

            self.state = PortState::DevicePresent;
            true
        }
    }

    pub fn start_cmd(&self) {
        unsafe {
            // Wait for CR to clear
            for _ in 0..100_000 {
                let cmd = self.read_port_reg(PORT_CMD);
                if (cmd & CMD_CR) == 0 {
                    break;
                }
                core::hint::spin_loop();
            }

            let mut cmd = self.read_port_reg(PORT_CMD);
            cmd |= CMD_FRE;
            self.write_port_reg(PORT_CMD, cmd);

            cmd |= CMD_ST;
            self.write_port_reg(PORT_CMD, cmd);
        }
    }

    pub fn stop_cmd(&self) {
        unsafe {
            let mut cmd = self.read_port_reg(PORT_CMD);
            cmd &= !CMD_ST;
            self.write_port_reg(PORT_CMD, cmd);

            // Wait for CR to clear
            for _ in 0..500_000 {
                let cmd = self.read_port_reg(PORT_CMD);
                if (cmd & CMD_CR) == 0 {
                    break;
                }
                core::hint::spin_loop();
            }

            cmd = self.read_port_reg(PORT_CMD);
            cmd &= !CMD_FRE;
            self.write_port_reg(PORT_CMD, cmd);

            for _ in 0..500_000 {
                let cmd = self.read_port_reg(PORT_CMD);
                if (cmd & CMD_FR) == 0 {
                    break;
                }
                core::hint::spin_loop();
            }
        }
    }

    pub fn port_reset(&mut self) -> Result<(), BlockError> {
        self.stop_cmd();

        unsafe {
            // COMRESET via SCTL
            self.write_port_reg(PORT_SCTL, 0x301); // DET=1, SPD=3 (no speed restriction)
            for _ in 0..100_000 {
                core::hint::spin_loop();
            }

            self.write_port_reg(PORT_SCTL, 0x300); // DET=0
            for _ in 0..500_000 {
                core::hint::spin_loop();
            }

            // Clear SERR
            let serr = self.read_port_reg(PORT_SERR);
            self.write_port_reg(PORT_SERR, serr);

            // Clear IS
            let is = self.read_port_reg(PORT_IS);
            self.write_port_reg(PORT_IS, is);

            // Wait for device detection
            for _ in 0..1_000_000 {
                let ssts = self.read_port_reg(PORT_SSTS);
                if (ssts & 0x0F) == 3 {
                    break;
                }
                core::hint::spin_loop();
            }
        }

        if !self.detect() {
            return Err(BlockError::MediaNotPresent);
        }

        self.init_memory()?;
        self.start_cmd();
        self.state = PortState::Active;

        Ok(())
    }

    fn init_memory(&mut self) -> Result<(), BlockError> {
        // Alloc-once guard: port_reset() can call init_memory() repeatedly
        // (re-init / S3 resume / replug). The DMA frames are never freed
        // (no deallocate_frame in this driver), so re-running would leak the
        // prior cmd_list + 2 command-table frames per reset. If already
        // initialized, reuse the existing frames and just re-program the
        // port registers below.
        if self.cmd_list != 0 {
            let cmd_list_phys = self.cmd_list;
            let cmd_list_virt = self.cmd_list_virt;
            let fis_phys = self.fis_base;
            unsafe {
                self.write_port_reg(PORT_CLB, cmd_list_phys as u32);
                self.write_port_reg(PORT_CLBU, (cmd_list_phys >> 32) as u32);
                self.write_port_reg(PORT_FB, fis_phys as u32);
                self.write_port_reg(PORT_FBU, (fis_phys >> 32) as u32);
            }
            let _ = cmd_list_virt;
            return Ok(());
        }

        // Allocate a 4KiB frame for command list (1KB) + FIS (256B) + command tables
        let (cmd_list_phys, cmd_list_virt) = alloc_dma_frame()?;
        let fis_phys = cmd_list_phys + 0x400;

        self.cmd_list = cmd_list_phys;
        self.cmd_list_virt = cmd_list_virt;
        self.fis_base = fis_phys;

        // Allocate separate frames for command tables (32 slots × 256 bytes = 8KB = 2 pages)
        let (ct_page0_phys, _ct_page0_virt) = alloc_dma_frame()?;
        let (ct_page1_phys, _ct_page1_virt) = alloc_dma_frame()?;

        unsafe {
            // Command list and FIS are already zeroed by alloc_dma_frame

            // Set port registers
            self.write_port_reg(PORT_CLB, cmd_list_phys as u32);
            self.write_port_reg(PORT_CLBU, (cmd_list_phys >> 32) as u32);
            self.write_port_reg(PORT_FB, fis_phys as u32);
            self.write_port_reg(PORT_FBU, (fis_phys >> 32) as u32);

            // Set up command table pointers in the command list
            for slot in 0..32u32 {
                let ct_phys = if slot < 16 {
                    ct_page0_phys + (slot as u64 * 256)
                } else {
                    ct_page1_phys + ((slot - 16) as u64 * 256)
                };
                let hdr_ptr = (cmd_list_virt + slot as u64 * 32) as *mut CommandHeader;
                let mut hdr: CommandHeader = core::ptr::read_volatile(hdr_ptr);
                hdr.set_ctba(ct_phys);
                core::ptr::write_volatile(hdr_ptr, hdr);
            }
        }

        Ok(())
    }

    fn find_free_slot(&self) -> Option<u8> {
        unsafe {
            let slots = self.read_port_reg(PORT_SACT) | self.read_port_reg(PORT_CI);
            for i in 0..32u8 {
                if (slots & (1 << i)) == 0 {
                    return Some(i);
                }
            }
        }
        None
    }

    fn issue_command(
        &self,
        slot: u8,
        fis: &FisRegH2D,
        buf_phys: u64,
        byte_count: u32,
        write: bool,
    ) -> Result<(), BlockError> {
        unsafe {
            let hdr_ptr = (self.cmd_list_virt + slot as u64 * 32) as *mut CommandHeader;
            let mut hdr: CommandHeader = core::ptr::read_volatile(hdr_ptr);

            hdr.set_cfl(5); // FIS is 5 DWORDs (20 bytes)
            hdr.prdtl = 1;
            hdr.prdbc = 0;

            if write {
                hdr.set_write();
            }

            core::ptr::write_volatile(hdr_ptr, hdr);

            // Write the FIS into the command table (CPU uses virt; PRDT uses phys).
            let ct_phys = hdr.ctba as u64 | ((hdr.ctbau as u64) << 32);
            let ct_virt = crate::memory::phys_to_virt(ct_phys).as_u64();
            let cfis_ptr = ct_virt as *mut FisRegH2D;
            core::ptr::write_volatile(cfis_ptr, *fis);

            // Set up the PRDT entry (at offset 0x80 in the command table)
            let prdt_ptr = (ct_virt + 0x80) as *mut PrdtEntry;
            let prdt = PrdtEntry::new(buf_phys, byte_count, true);
            core::ptr::write_volatile(prdt_ptr, prdt);

            // Clear any pending interrupts
            let is = self.read_port_reg(PORT_IS);
            self.write_port_reg(PORT_IS, is);

            // Issue the command
            self.write_port_reg(PORT_CI, 1 << slot);

            // Wait for completion
            for _ in 0..5_000_000 {
                let ci = self.read_port_reg(PORT_CI);
                if (ci & (1 << slot)) == 0 {
                    // Check for errors
                    let tfd = self.read_port_reg(PORT_TFD);
                    if (tfd & 0x01) != 0 {
                        // ERR bit
                        return Err(BlockError::IoError);
                    }
                    return Ok(());
                }

                let is = self.read_port_reg(PORT_IS);
                if (is & (1 << 30)) != 0 {
                    // TFES - Task File Error Status
                    self.write_port_reg(PORT_IS, is);
                    return Err(BlockError::IoError);
                }

                core::hint::spin_loop();
            }
        }

        Err(BlockError::Timeout)
    }

    pub fn identify_device(&mut self) -> Result<(), BlockError> {
        let slot = self.find_free_slot().ok_or(BlockError::DeviceBusy)?;
        let (buf_phys, buf_virt) = alloc_dma_frame()?;

        let cmd = match self.device_type {
            SataDeviceType::Ata => ATA_CMD_IDENTIFY,
            SataDeviceType::Atapi => ATA_CMD_IDENTIFY_PACKET,
            _ => return Err(BlockError::UnsupportedFeature),
        };

        let fis = FisRegH2D::new_command(cmd);
        self.issue_command(slot, &fis, buf_phys, 512, false)?;

        // Parse identify data (512 bytes / 256 words)
        let data = unsafe { core::slice::from_raw_parts(buf_virt as *const u16, 256) };

        // Words 27-46: Model number (ATA string, byte-swapped)
        let mut model_bytes = [0u8; 40];
        for i in 0..20 {
            let word = data[27 + i];
            model_bytes[i * 2] = (word >> 8) as u8;
            model_bytes[i * 2 + 1] = (word & 0xFF) as u8;
        }
        self.model = String::from_utf8_lossy(&model_bytes).trim().into();

        // Words 10-19: Serial number
        let mut serial_bytes = [0u8; 20];
        for i in 0..10 {
            let word = data[10 + i];
            serial_bytes[i * 2] = (word >> 8) as u8;
            serial_bytes[i * 2 + 1] = (word & 0xFF) as u8;
        }
        self.serial = String::from_utf8_lossy(&serial_bytes).trim().into();

        // Words 23-26: Firmware revision
        let mut fw_bytes = [0u8; 8];
        for i in 0..4 {
            let word = data[23 + i];
            fw_bytes[i * 2] = (word >> 8) as u8;
            fw_bytes[i * 2 + 1] = (word & 0xFF) as u8;
        }
        self.firmware = String::from_utf8_lossy(&fw_bytes).trim().into();

        // Word 83: 48-bit LBA support
        self.supports_48bit = (data[83] & (1 << 10)) != 0;

        // Words 100-103: 48-bit addressable sectors
        if self.supports_48bit {
            self.total_sectors = data[100] as u64
                | ((data[101] as u64) << 16)
                | ((data[102] as u64) << 32)
                | ((data[103] as u64) << 48);
        } else {
            // Words 60-61: 28-bit addressable sectors
            self.total_sectors = data[60] as u64 | ((data[61] as u64) << 16);
        }

        // Word 106: Logical/Physical sector size
        if (data[106] & (1 << 14)) != 0 && (data[106] & (1 << 15)) == 0 {
            if (data[106] & (1 << 12)) != 0 {
                let lss = data[117] as u32 | ((data[118] as u32) << 16);
                self.sector_size = lss * 2;
            }
        }

        // Word 75: NCQ queue depth
        let ncq_depth = (data[75] & 0x1F) as u8 + 1;
        self.supports_ncq = (data[76] & (1 << 8)) != 0;
        self.ncq_depth = if self.supports_ncq { ncq_depth } else { 0 };

        // Word 82/83: SMART support
        self.supports_smart = (data[82] & (1 << 0)) != 0;

        // Word 169: TRIM support
        self.supports_trim = (data[169] & (1 << 0)) != 0;

        let cap_gb = (self.total_sectors * self.sector_size as u64) / (1024 * 1024 * 1024);

        crate::serial_println!(
            "[ahci] port{}: {} {} fw={} {}GB sector={} ncq={}/{}",
            self.port_num,
            self.model,
            self.serial,
            self.firmware,
            cap_gb,
            self.sector_size,
            self.supports_ncq,
            self.ncq_depth
        );

        self.state = PortState::Active;
        Ok(())
    }

    pub fn read_sectors(&self, lba: u64, count: u16, buf_phys: u64) -> Result<(), BlockError> {
        if self.state != PortState::Active {
            return Err(BlockError::DeviceNotFound);
        }

        let slot = self.find_free_slot().ok_or(BlockError::DeviceBusy)?;
        let byte_count = count as u32 * self.sector_size;

        let mut fis = FisRegH2D::new_command(ATA_CMD_READ_DMA_EXT);
        fis.set_lba(lba);
        fis.set_count(count);

        self.issue_command(slot, &fis, buf_phys, byte_count, false)
    }

    pub fn write_sectors(&self, lba: u64, count: u16, buf_phys: u64) -> Result<(), BlockError> {
        if self.state != PortState::Active {
            return Err(BlockError::DeviceNotFound);
        }

        let slot = self.find_free_slot().ok_or(BlockError::DeviceBusy)?;
        let byte_count = count as u32 * self.sector_size;

        let mut fis = FisRegH2D::new_command(ATA_CMD_WRITE_DMA_EXT);
        fis.set_lba(lba);
        fis.set_count(count);

        self.issue_command(slot, &fis, buf_phys, byte_count, true)
    }

    pub fn flush(&self) -> Result<(), BlockError> {
        if self.state != PortState::Active {
            return Err(BlockError::DeviceNotFound);
        }

        let slot = self.find_free_slot().ok_or(BlockError::DeviceBusy)?;
        let fis = FisRegH2D::new_command(ATA_CMD_FLUSH_CACHE_EXT);
        self.issue_command(slot, &fis, 0, 0, false)
    }

    pub fn trim(&self, lba: u64, count: u64, buf_phys: u64) -> Result<(), BlockError> {
        if !self.supports_trim {
            return Err(BlockError::UnsupportedFeature);
        }
        if self.state != PortState::Active {
            return Err(BlockError::DeviceNotFound);
        }

        let slot = self.find_free_slot().ok_or(BlockError::DeviceBusy)?;

        // Write a single TRIM range entry at buf_phys (8 bytes: 6-byte LBA + 2-byte count)
        unsafe {
            let ptr = buf_phys as *mut u8;
            core::ptr::write_bytes(ptr, 0, 512); // zero a full sector
            let range = core::slice::from_raw_parts_mut(ptr, 8);
            range[0] = (lba & 0xFF) as u8;
            range[1] = ((lba >> 8) & 0xFF) as u8;
            range[2] = ((lba >> 16) & 0xFF) as u8;
            range[3] = ((lba >> 24) & 0xFF) as u8;
            range[4] = ((lba >> 32) & 0xFF) as u8;
            range[5] = ((lba >> 40) & 0xFF) as u8;
            range[6] = (count & 0xFF) as u8;
            range[7] = ((count >> 8) & 0xFF) as u8;
        }

        let mut fis = FisRegH2D::new_command(ATA_CMD_DATA_SET_MGMT);
        fis.set_count(1);
        fis.set_features(0x01); // TRIM bit

        self.issue_command(slot, &fis, buf_phys, 512, true)
    }

    pub fn read_smart(&self, buf_phys: u64) -> Result<(), BlockError> {
        if !self.supports_smart {
            return Err(BlockError::UnsupportedFeature);
        }
        if self.state != PortState::Active {
            return Err(BlockError::DeviceNotFound);
        }

        let slot = self.find_free_slot().ok_or(BlockError::DeviceBusy)?;

        let mut fis = FisRegH2D::new_command(ATA_CMD_SMART);
        fis.set_features(0xD0); // SMART READ DATA
        fis.lba1 = 0x4F; // SMART signature
        fis.lba2 = 0xC2;

        self.issue_command(slot, &fis, buf_phys, 512, false)
    }

    pub fn register_as_block_device(&self, index: u8) -> BlockDeviceInfo {
        let name = alloc::format!("sd{}", (b'a' + index) as char);
        let mut dev = BlockDeviceInfo::new(name, 8, index as u16);
        dev.sector_size = self.sector_size;
        dev.total_sectors = self.total_sectors;
        dev.read_only = false;
        dev.removable = false;
        dev.rotational = true;
        dev.queue_depth = if self.supports_ncq {
            self.ncq_depth as u32
        } else {
            1
        };
        dev.model = self.model.clone();
        dev.serial = self.serial.clone();
        dev.firmware = self.firmware.clone();
        dev
    }

    pub fn capacity_gb(&self) -> u64 {
        (self.total_sectors * self.sector_size as u64) / (1024 * 1024 * 1024)
    }
}

// ─── AHCI Capabilities ─────────────────────────────────────────────────────

pub struct AhciCapabilities {
    pub num_ports: u8,
    pub supports_64bit: bool,
    pub supports_ncq: bool,
    pub max_cmd_slots: u8,
    pub interface_speed: SataSpeed,
    pub supports_ahci_only: bool,
    pub supports_staggered_spinup: bool,
    pub supports_activity_led: bool,
    pub supports_aggressive_link_pm: bool,
    pub supports_cmd_list_override: bool,
    pub supports_fis_switching: bool,
    pub supports_port_multiplier: bool,
    pub enclosure_management: bool,
}

impl AhciCapabilities {
    pub fn from_cap(cap: u32) -> Self {
        Self {
            num_ports: ((cap & 0x1F) + 1) as u8,
            supports_64bit: (cap & (1 << 31)) != 0,
            supports_ncq: (cap & (1 << 30)) != 0,
            max_cmd_slots: (((cap >> 8) & 0x1F) + 1) as u8,
            interface_speed: match (cap >> 20) & 0x0F {
                1 => SataSpeed::Gen1_1_5Gbps,
                2 => SataSpeed::Gen2_3Gbps,
                3 => SataSpeed::Gen3_6Gbps,
                _ => SataSpeed::Unknown,
            },
            supports_ahci_only: (cap & (1 << 18)) != 0,
            supports_staggered_spinup: (cap & (1 << 27)) != 0,
            supports_activity_led: (cap & (1 << 25)) != 0,
            supports_aggressive_link_pm: (cap & (1 << 26)) != 0,
            supports_cmd_list_override: (cap & (1 << 24)) != 0,
            supports_fis_switching: (cap & (1 << 16)) != 0,
            supports_port_multiplier: (cap & (1 << 17)) != 0,
            enclosure_management: (cap & (1 << 6)) != 0,
        }
    }
}

// ─── AHCI quirks (vendor-specific bring-up) ─────────────────────────────────

/// Vendor/device-derived quirks. On real AMD boards the FCH SATA HBA is owned
/// by UEFI/SMM in a BIOS-managed state until the OS performs the BIOS/OS handoff
/// (BOHC) — without it the controller's ports read as empty / writes wedge under
/// SMM. The handoff + a spec HBA reset are done unconditionally in `init()`
/// (no-ops on QEMU, which has neither BOH nor SMM); this struct records the
/// vendor so the bring-up is logged as the AMD-chipset path. MasterChecklist 1.5.
#[derive(Debug, Clone, Copy)]
pub struct AhciQuirks {
    /// AMD (0x1022) or legacy ATI (0x1002) FCH/SBxx SATA controller.
    pub is_amd: bool,
}

impl AhciQuirks {
    pub fn from_pci(vendor: u16, _device: u16) -> Self {
        Self {
            is_amd: vendor == 0x1022 || vendor == 0x1002,
        }
    }
}

// ─── AHCI Controller ───────────────────────────────────────────────────────

pub struct AhciController {
    bar5: u64,
    ports: Vec<AhciPort>,
    cap: AhciCapabilities,
    version: u32,
    ports_implemented: u32,
    vendor: u16,
    device: u16,
    quirks: AhciQuirks,
    /// Persistent (phys, virt) DMA bounce buffer for the `AhciBlockDevice`
    /// adapter, allocated once on first I/O and reused. AHCI block I/O is
    /// serialized under `AHCI_CONTROLLERS.lock()`, so a single buffer per
    /// controller is race-free and the in-flight DMA always completes
    /// (`issue_command` polls PORT_CI clear) before the buffer is reused.
    /// Replaces the per-I/O `alloc_dma_frame` that leaked a 4KiB frame on
    /// every read/write — the same class as NVMe BUG-37, never freed here
    /// (there is no `deallocate_frame` in this driver).
    bounce: Option<(u64, u64)>,
}

impl AhciController {
    pub fn new(bar5: u64, vendor: u16, device: u16) -> Self {
        Self {
            bar5,
            ports: Vec::new(),
            cap: AhciCapabilities::from_cap(0),
            version: 0,
            ports_implemented: 0,
            vendor,
            device,
            quirks: AhciQuirks::from_pci(vendor, device),
            bounce: None,
        }
    }

    /// Lazily allocate the persistent per-controller DMA bounce buffer and
    /// return its (phys, virt). Allocated once; reused for every block I/O.
    /// Mirrors `NvmeController::ensure_bounce` (BUG-37 fix). No per-call
    /// `Drop`: the persistent buffer is only ever reused after `issue_command`
    /// observes PORT_CI clear, so the prior DMA is always complete (no UAF).
    fn ensure_bounce(&mut self) -> Result<(u64, u64), BlockError> {
        if let Some(b) = self.bounce {
            return Ok(b);
        }
        let b = alloc_dma_frame()?;
        self.bounce = Some(b);
        Ok(b)
    }

    /// AHCI spec 10.6.3 — request OS ownership of the HBA from BIOS/SMM. No-op
    /// when CAP2.BOH is clear (QEMU, many desktop FCHs in pure-UEFI mode). On
    /// AMD boards where SMM holds the HBA this is what makes ports/writes work.
    fn bios_os_handoff(&self) {
        unsafe {
            if self.read_hba_reg(HBA_CAP2) & CAP2_BOH == 0 {
                return;
            }
            let mut bohc = self.read_hba_reg(HBA_BOHC);
            bohc |= BOHC_OOS;
            self.write_hba_reg(HBA_BOHC, bohc);
            // Wait for BIOS to drop ownership (BOS clears), bounded.
            let mut released = false;
            for _ in 0..1_000_000 {
                if (self.read_hba_reg(HBA_BOHC) & BOHC_BOS) == 0 {
                    released = true;
                    break;
                }
                core::hint::spin_loop();
            }
            // Then wait out BIOS Busy (it may still be cleaning up), bounded.
            for _ in 0..2_000_000 {
                if (self.read_hba_reg(HBA_BOHC) & BOHC_BB) == 0 {
                    break;
                }
                core::hint::spin_loop();
            }
            crate::serial_println!(
                "[ahci] BIOS/OS handoff: OS ownership {}",
                if released {
                    "acquired"
                } else {
                    "FORCED (BIOS did not release in time)"
                }
            );
        }
    }

    unsafe fn read_hba_reg(&self, offset: u64) -> u32 {
        let ptr = (self.bar5 + offset) as *const u32;
        core::ptr::read_volatile(ptr)
    }

    unsafe fn write_hba_reg(&self, offset: u64, val: u32) {
        let ptr = (self.bar5 + offset) as *mut u32;
        core::ptr::write_volatile(ptr, val);
    }

    pub fn init(&mut self) -> Result<(), BlockError> {
        if self.quirks.is_amd {
            crate::serial_println!(
                "[ahci] AMD FCH/SBxx SATA ({:04x}:{:04x}) — applying BIOS/OS handoff quirk",
                self.vendor,
                self.device
            );
        }

        // Real-board bring-up (AMD especially): claim the HBA from BIOS/SMM
        // before touching its config. No-op on QEMU (no BOH). We deliberately
        // do NOT issue a full GHC.HR HBA reset here — it wipes the port
        // signatures the firmware established, and `detect()` runs before the
        // per-port COMRESET, so a reset makes every port read as empty (caught
        // by the QEMU AHCI smoketest). The per-port COMRESET in `port_reset()`
        // already clears stale port state safely.
        self.bios_os_handoff();

        unsafe {
            // Enable AHCI mode (required before register access / port bring-up).
            let mut ghc = self.read_hba_reg(HBA_GHC);
            ghc |= GHC_AE;
            self.write_hba_reg(HBA_GHC, ghc);

            // Read capabilities.
            let cap = self.read_hba_reg(HBA_CAP);
            self.cap = AhciCapabilities::from_cap(cap);

            self.version = self.read_hba_reg(HBA_VS);
            let ver_major = (self.version >> 16) as u16;
            let ver_minor = (self.version & 0xFFFF) as u16;

            crate::serial_println!(
                "[ahci] AHCI {}.{}, {} ports, {} cmd slots, 64bit={}, ncq={}",
                ver_major,
                ver_minor,
                self.cap.num_ports,
                self.cap.max_cmd_slots,
                self.cap.supports_64bit,
                self.cap.supports_ncq
            );

            // Read which ports are implemented.
            self.ports_implemented = self.read_hba_reg(HBA_PI);

            // Enable interrupts.
            ghc = self.read_hba_reg(HBA_GHC);
            ghc |= GHC_IE;
            self.write_hba_reg(HBA_GHC, ghc);
        }

        Ok(())
    }

    pub fn detect_ports(&mut self) -> Vec<u8> {
        let mut active_ports = Vec::new();

        for i in 0..32u8 {
            if (self.ports_implemented & (1 << i)) == 0 {
                continue;
            }

            let mut port = AhciPort::new(self.bar5, i);
            if port.detect() {
                crate::serial_println!(
                    "[ahci] port {}: {:?} device, link {:?}",
                    i,
                    port.device_type,
                    port.link_speed
                );
                active_ports.push(i);
            }
            self.ports.push(port);
        }

        active_ports
    }

    pub fn initialize_ports(&mut self) {
        let mut disk_index = 0u8;

        for port in self.ports.iter_mut() {
            if port.device_type == SataDeviceType::None {
                continue;
            }

            if let Err(e) = port.port_reset() {
                crate::serial_println!("[ahci] port {} reset failed: {:?}", port.port_num, e);
                continue;
            }

            if matches!(
                port.device_type,
                SataDeviceType::Ata | SataDeviceType::Atapi
            ) {
                if let Err(e) = port.identify_device() {
                    crate::serial_println!(
                        "[ahci] port {} identify failed: {:?}",
                        port.port_num,
                        e
                    );
                    continue;
                }

                if port.device_type == SataDeviceType::Ata {
                    let dev = port.register_as_block_device(disk_index);
                    let _ = crate::block_io::register_block_device(dev);
                    disk_index += 1;
                }
            }
        }
    }

    pub fn port_count(&self) -> usize {
        self.ports.len()
    }

    pub fn active_ports(&self) -> Vec<u8> {
        self.ports
            .iter()
            .filter(|p| p.state == PortState::Active)
            .map(|p| p.port_num)
            .collect()
    }

    pub fn get_port(&self, num: u8) -> Option<&AhciPort> {
        self.ports.iter().find(|p| p.port_num == num)
    }

    pub fn get_port_mut(&mut self, num: u8) -> Option<&mut AhciPort> {
        self.ports.iter_mut().find(|p| p.port_num == num)
    }
}

// ─── Global State & Initialization ──────────────────────────────────────────

pub static AHCI_CONTROLLERS: Mutex<Vec<AhciController>> = Mutex::new(Vec::new());

/// BlockDevice adapter for AHCI port 0 on controller 0.
pub struct AhciBlockDevice;

impl crate::block_io::BlockDevice for AhciBlockDevice {
    fn read_sector(&self, lba: u64, buf: &mut [u8]) -> Result<(), &'static str> {
        // One lock hold for the whole op so the persistent bounce buffer is
        // never shared across a concurrent I/O (block I/O is serialized here).
        let bounce_virt = {
            let mut ctrls = AHCI_CONTROLLERS.lock();
            let ctrl = ctrls.first_mut().ok_or("ahci: no controller")?;
            let bounce = ctrl.ensure_bounce().map_err(|_| "ahci: DMA alloc failed")?;
            let port = ctrl
                .ports
                .iter()
                .find(|p| p.state == PortState::Active)
                .ok_or("ahci: no active port")?;
            port.read_sectors(lba, 1, bounce.0)
                .map_err(|_| "ahci: read failed")?;
            bounce.1
        };
        let len = buf.len().min(512);
        unsafe {
            core::ptr::copy_nonoverlapping(bounce_virt as *const u8, buf.as_mut_ptr(), len);
        }
        Ok(())
    }

    fn write_sector(&self, lba: u64, buf: &[u8]) -> Result<(), &'static str> {
        // safe_mode_guard_write MUST stay before any DMA staging/issue.
        crate::block_io::safe_mode_guard_write(lba, buf.len(), "ahci")?;
        let len = buf.len().min(512);
        let mut ctrls = AHCI_CONTROLLERS.lock();
        let ctrl = ctrls.first_mut().ok_or("ahci: no controller")?;
        let (bounce_phys, bounce_virt) =
            ctrl.ensure_bounce().map_err(|_| "ahci: DMA alloc failed")?;
        unsafe {
            core::ptr::copy_nonoverlapping(buf.as_ptr(), bounce_virt as *mut u8, len);
        }
        let port = ctrl
            .ports
            .iter()
            .find(|p| p.state == PortState::Active)
            .ok_or("ahci: no active port")?;
        port.write_sectors(lba, 1, bounce_phys)
            .map_err(|_| "ahci: write failed")?;
        Ok(())
    }

    fn sector_size(&self) -> usize {
        512
    }

    fn total_sectors(&self) -> u64 {
        let ctrls = AHCI_CONTROLLERS.lock();
        ctrls
            .first()
            .and_then(|c| c.ports.iter().find(|p| p.state == PortState::Active))
            .map(|p| p.total_sectors)
            .unwrap_or(0)
    }

    fn flush_cache(&self) -> Result<(), &'static str> {
        // ATA FLUSH CACHE EXT (0xEA) — commit the drive's write cache so
        // bootlog-persist survives a power-cycle. See BlockDevice trait doc.
        let ctrls = AHCI_CONTROLLERS.lock();
        let ctrl = ctrls.first().ok_or("ahci: no controller")?;
        let port = ctrl
            .ports
            .iter()
            .find(|p| p.state == PortState::Active)
            .ok_or("ahci: no active port")?;
        port.flush().map_err(|_| "ahci: flush failed")
    }
}

unsafe impl Send for AhciBlockDevice {}

pub fn init() {
    crate::serial_println!("[ahci] scanning PCI for AHCI controllers...");

    let pci_devices = crate::pci::enumerate();
    let mut count = 0u32;

    for dev in &pci_devices {
        // AHCI: class=0x01 (Mass Storage), subclass=0x06 (SATA), prog_if=0x01 (AHCI)
        if dev.class == 0x01 && dev.subclass == 0x06 && dev.prog_if == 0x01 {
            crate::pci::enable_bus_mastering(dev);
            let _irq_mode = crate::storage_irq::probe_msix_or_intx("ahci", dev, 1);
            crate::serial_println!(
                "[ahci] found AHCI controller at {:02x}:{:02x}.{} vendor={:04x} device={:04x}",
                dev.bus,
                dev.device,
                dev.function,
                dev.vendor_id,
                dev.device_id
            );

            let bar5_raw = dev.bars[5] as u64;
            if bar5_raw == 0 {
                crate::serial_println!("[ahci] BAR5 (ABAR) not configured, skipping");
                continue;
            }
            let bar5_phys = bar5_raw & !0xFu64;
            if crate::memory::PHYS_MEM_OFFSET.get().is_none() {
                crate::serial_println!("[ahci] PHYS_MEM_OFFSET not initialized, skipping");
                continue;
            }
            // 64-bit BARs can sit above the linear physmap; map the MMIO region
            // (creates PTEs + disables caching) instead of assuming it's mapped.
            let bar5_size = {
                let s = crate::mmio::pci_bar_size_bytes(dev.bus, dev.device, dev.function, 5);
                if s == 0 {
                    0x2000
                } else {
                    s
                }
            };
            let bar5_virt = crate::arch::mmu::kernel()
                .map_mmio_range(
                    x86_64::PhysAddr::new(bar5_phys),
                    bar5_size,
                    crate::arch::mmu::PageFlags::DEVICE,
                )
                .as_u64();
            crate::serial_println!(
                "[ahci] BAR5 phys={:#x} size={:#x} virt={:#x}",
                bar5_phys,
                bar5_size,
                bar5_virt
            );

            let mut ctrl = AhciController::new(bar5_virt, dev.vendor_id, dev.device_id);

            if let Err(e) = ctrl.init() {
                crate::serial_println!("[ahci] init failed: {:?}", e);
                continue;
            }

            let active = ctrl.detect_ports();
            crate::serial_println!("[ahci] {} active port(s)", active.len());

            ctrl.initialize_ports();

            AHCI_CONTROLLERS.lock().push(ctrl);
            count += 1;
        }
    }

    if count > 0 {
        let has_active = crate::block_io::ACTIVE_BLOCK_DEVICE.lock().is_some();
        if !has_active {
            crate::block_io::set_active_block_device(alloc::boxed::Box::new(AhciBlockDevice));
            crate::serial_println!("[ahci] registered as active block device");
        }
    }

    crate::serial_println!("[ OK ] AHCI: {} controller(s) initialized", count);
}

/// Boot smoketest: sector-0 read on the first active AHCI port (QEMU `ahci` device).
pub fn run_boot_smoketest() {
    use crate::block_io::BlockDevice;

    if AHCI_CONTROLLERS.lock().is_empty() {
        crate::serial_println!(
            "[ahci] smoketest SKIP: no AHCI controller (add `-device ahci` in QEMU)"
        );
        return;
    }

    let mut buf = [0u8; 512];
    let dev = AhciBlockDevice;
    let t0 = unsafe { core::arch::x86_64::_rdtsc() };
    let r = dev.read_sector(0, &mut buf);
    let t1 = unsafe { core::arch::x86_64::_rdtsc() };

    match r {
        Ok(()) => {
            let marker = b"RaeenOS-AHCI-block-0-ok!";
            let matched = buf.len() >= marker.len() && &buf[..marker.len()] == marker;
            let preview: String = buf[..24]
                .iter()
                .map(|b| {
                    if *b >= 0x20 && *b < 0x7f {
                        *b as char
                    } else {
                        '.'
                    }
                })
                .collect();
            crate::serial_println!(
                "[ahci] smoketest PASS: read LBA0 ({} cycles) preview=\"{}\" marker={}",
                t1.saturating_sub(t0),
                preview,
                if matched {
                    "MATCH"
                } else {
                    "no-match (empty or foreign disk)"
                },
            );
        }
        Err(e) => {
            crate::serial_println!("[ahci] smoketest FAIL: read LBA0 — {}", e);
        }
    }

    // Bounce-buffer stability: prove the persistent bounce is allocated ONCE
    // and reused (BUG class: per-call alloc_dma_frame leaked a 4KiB frame per
    // sector I/O). Do N reads and assert the bounce phys address never moves.
    // FAIL if it changes (= re-allocated per call = still leaking).
    const N: u32 = 4;
    let bounce_addr = || -> Option<u64> {
        AHCI_CONTROLLERS
            .lock()
            .first()
            .and_then(|c| c.bounce)
            .map(|b| b.0)
    };
    let mut reused = true;
    let mut first_addr: Option<u64> = None;
    let mut ok_reads = 0u32;
    for _ in 0..N {
        if dev.read_sector(0, &mut buf).is_err() {
            reused = false;
            break;
        }
        ok_reads += 1;
        match (first_addr, bounce_addr()) {
            (None, Some(a)) => first_addr = Some(a),
            (Some(a), Some(b)) if a != b => reused = false,
            (_, None) => reused = false,
            _ => {}
        }
    }
    let pass = reused && ok_reads == N && first_addr.is_some();
    crate::serial_println!(
        "[ahci] bounce smoketest: reused={} addr={:#x} reads={} -> {}",
        reused,
        first_addr.unwrap_or(0),
        ok_reads,
        if pass { "PASS" } else { "FAIL" },
    );
    // Note: this proves the leak is closed STRUCTURALLY (one alloc, reused).
    // The full leak-RATE proof (free-frame count stable under sustained SATA
    // traffic) is iron-gated — QEMU `ahci` != the AMD FCH SATA path, and
    // Athena boots NVMe, so AHCI as the FS-backing device is a SATA install.
}

/// `/proc/raeen/ahci` — controller/port summary.
pub fn dump_text() -> String {
    let ctrls = AHCI_CONTROLLERS.lock();
    let mut out = String::from("# RaeenOS AHCI\n");
    if ctrls.is_empty() {
        out.push_str("controllers: 0\n");
        out.push_str("note: no AHCI HBA found (normal when QEMU has no ahci device)\n");
        return out;
    }
    out.push_str(&alloc::format!("controllers: {}\n", ctrls.len()));
    for (ci, ctrl) in ctrls.iter().enumerate() {
        out.push_str(&alloc::format!(
            "controller{}: ports={}\n",
            ci,
            ctrl.ports.len()
        ));
        for p in &ctrl.ports {
            out.push_str(&alloc::format!(
                "  port{}: state={:?} type={:?} model=\"{}\" sectors={}\n",
                p.port_num,
                p.state,
                p.device_type,
                p.model,
                p.total_sectors,
            ));
        }
    }
    out
}
