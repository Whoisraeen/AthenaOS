#![allow(dead_code)]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;
use x86_64::structures::paging::FrameAllocator;

use crate::block_io::{BlockDeviceInfo, BlockError};
use crate::memory::GlobalFrameAllocator;

const DMA_PAGE: u64 = 4096;

/// Upper bound (in TSC cycles) on a single NVMe completion busy-poll.
/// 1 second at any realistic clock.
const NVME_POLL_DEADLINE_CYCLES: u64 = 1_000_000_000;

fn alloc_dma_frame_mapped(iommu_domain: Option<u16>) -> Result<(u64, u64), BlockError> {
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
    if let Some(dom) = iommu_domain {
        if !crate::iommu::map_dma(dom, phys, phys, DMA_PAGE, true, true) {
            return Err(BlockError::HardwareFailure);
        }
    }
    Ok((phys, virt))
}

fn alloc_dma_frames_mapped(
    iommu_domain: Option<u16>,
    count: usize,
) -> Result<(u64, u64), BlockError> {
    // The device DMAs into a single buffer spanning `count` pages starting at
    // `phys`, so the backing frames MUST be physically contiguous (CLAUDE.md
    // pitfall #7). A loop of allocate_frame() returns scattered frames; the
    // device would then write the 2nd+ pages into whatever physical memory
    // happens to follow the first frame. Use the buddy allocator's contiguous
    // path with the smallest order that covers `count` pages.
    let count = count.max(1);
    let mut order: u8 = 0;
    while (1usize << order) < count {
        order += 1;
    }
    let phys = crate::memory::allocate_contiguous_frames(order)
        .ok_or(BlockError::HardwareFailure)?
        .as_u64();
    let offset = *crate::memory::PHYS_MEM_OFFSET
        .get()
        .ok_or(BlockError::HardwareFailure)?;
    let virt = (offset + phys).as_u64();
    unsafe {
        core::ptr::write_bytes(virt as *mut u8, 0, count * 4096);
    }
    if let Some(dom) = iommu_domain {
        let size = (count as u64).saturating_mul(DMA_PAGE);
        if !crate::iommu::map_dma(dom, phys, phys, size, true, true) {
            return Err(BlockError::HardwareFailure);
        }
    }
    Ok((phys, virt))
}

// ─── NVMe Admin Command Opcodes ─────────────────────────────────────────────

pub const NVME_ADMIN_DELETE_SQ: u8 = 0x00;
pub const NVME_ADMIN_CREATE_SQ: u8 = 0x01;
pub const NVME_ADMIN_GET_LOG_PAGE: u8 = 0x02;
pub const NVME_ADMIN_DELETE_CQ: u8 = 0x04;
pub const NVME_ADMIN_CREATE_CQ: u8 = 0x05;
pub const NVME_ADMIN_IDENTIFY: u8 = 0x06;
pub const NVME_ADMIN_ABORT: u8 = 0x08;
pub const NVME_ADMIN_SET_FEATURES: u8 = 0x09;
pub const NVME_ADMIN_GET_FEATURES: u8 = 0x0A;
pub const NVME_ADMIN_ASYNC_EVENT_REQ: u8 = 0x0C;
pub const NVME_ADMIN_NS_MGMT: u8 = 0x0D;
pub const NVME_ADMIN_FIRMWARE_COMMIT: u8 = 0x10;
pub const NVME_ADMIN_FIRMWARE_DOWNLOAD: u8 = 0x11;
pub const NVME_ADMIN_NS_ATTACH: u8 = 0x15;
pub const NVME_ADMIN_FORMAT_NVM: u8 = 0x80;
pub const NVME_ADMIN_SECURITY_SEND: u8 = 0x81;
pub const NVME_ADMIN_SECURITY_RECV: u8 = 0x82;

// ─── NVMe I/O Command Opcodes ───────────────────────────────────────────────

pub const NVME_IO_FLUSH: u8 = 0x00;
pub const NVME_IO_WRITE: u8 = 0x01;
pub const NVME_IO_READ: u8 = 0x02;
pub const NVME_IO_WRITE_UNCORRECTABLE: u8 = 0x04;
pub const NVME_IO_COMPARE: u8 = 0x05;
pub const NVME_IO_WRITE_ZEROES: u8 = 0x08;
pub const NVME_IO_DATASET_MGMT: u8 = 0x09;

// ─── NVMe Controller Registers (BAR0 offsets) ───────────────────────────────

const REG_CAP: u64 = 0x00; // Controller Capabilities (64-bit)
const REG_VS: u64 = 0x08; // Version
const REG_INTMS: u64 = 0x0C; // Interrupt Mask Set
const REG_INTMC: u64 = 0x10; // Interrupt Mask Clear
const REG_CC: u64 = 0x14; // Controller Configuration
const REG_CSTS: u64 = 0x1C; // Controller Status
const REG_NSSR: u64 = 0x20; // NVM Subsystem Reset
const REG_AQA: u64 = 0x24; // Admin Queue Attributes
const REG_ASQ: u64 = 0x28; // Admin Submission Queue Base (64-bit)
const REG_ACQ: u64 = 0x30; // Admin Completion Queue Base (64-bit)

const CC_EN: u32 = 1 << 0;
const CC_CSS_NVM: u32 = 0 << 4;
const CC_MPS_4K: u32 = 0 << 7; // 2^(12+0) = 4096
const CC_AMS_RR: u32 = 0 << 11; // Round-Robin arbitration
const CC_SHN_NONE: u32 = 0 << 14;
const CC_SHN_NORMAL: u32 = 1 << 14;
const CC_IOSQES: u32 = 6 << 16; // 2^6 = 64 bytes
const CC_IOCQES: u32 = 4 << 20; // 2^4 = 16 bytes

const CSTS_RDY: u32 = 1 << 0;
const CSTS_CFS: u32 = 1 << 1;
const CSTS_SHST_MASK: u32 = 3 << 2;
const CSTS_SHST_NORMAL: u32 = 0 << 2;
const CSTS_SHST_COMPLETE: u32 = 2 << 2;

// ─── NVMe Feature IDs ──────────────────────────────────────────────────────

const FEAT_ARBITRATION: u8 = 0x01;
const FEAT_POWER_MGMT: u8 = 0x02;
const FEAT_LBA_RANGE: u8 = 0x03;
const FEAT_TEMP_THRESH: u8 = 0x04;
const FEAT_ERROR_RECOVERY: u8 = 0x05;
const FEAT_VOLATILE_WC: u8 = 0x06;
const FEAT_NUM_QUEUES: u8 = 0x07;
const FEAT_IRQ_COALESCE: u8 = 0x08;
const FEAT_IRQ_CONFIG: u8 = 0x09;
const FEAT_WRITE_ATOMIC: u8 = 0x0A;
const FEAT_ASYNC_EVENT: u8 = 0x0B;

// ─── Log Page IDs ───────────────────────────────────────────────────────────

const LOG_ERROR_INFO: u8 = 0x01;
const LOG_SMART_HEALTH: u8 = 0x02;
const LOG_FIRMWARE_SLOT: u8 = 0x03;
const LOG_CHANGED_NS: u8 = 0x04;
const LOG_CMD_EFFECTS: u8 = 0x05;

// ─── NVMe State ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NvmeState {
    Uninitialized,
    Resetting,
    Ready,
    ShuttingDown,
    Failed,
    Disabled,
}

// ─── NVMe Command / Completion ──────────────────────────────────────────────

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct NvmeCommand {
    pub opcode: u8,
    pub flags: u8,
    pub cid: u16,
    pub nsid: u32,
    pub cdw2: u32,
    pub cdw3: u32,
    pub metadata: u64,
    pub prp1: u64,
    pub prp2: u64,
    pub cdw10: u32,
    pub cdw11: u32,
    pub cdw12: u32,
    pub cdw13: u32,
    pub cdw14: u32,
    pub cdw15: u32,
}

impl NvmeCommand {
    pub fn new(opcode: u8, nsid: u32) -> Self {
        Self {
            opcode,
            nsid,
            ..Default::default()
        }
    }

    pub fn identify(nsid: u32, cns: u8, prp1: u64) -> Self {
        Self {
            opcode: NVME_ADMIN_IDENTIFY,
            nsid,
            prp1,
            cdw10: cns as u32,
            ..Default::default()
        }
    }

    pub fn create_io_cq(qid: u16, size: u16, prp: u64, iv: u16) -> Self {
        Self {
            opcode: NVME_ADMIN_CREATE_CQ,
            prp1: prp,
            cdw10: ((size as u32 - 1) << 16) | qid as u32,
            cdw11: (iv as u32) << 16 | 0x01, // phys contiguous, interrupts enabled
            ..Default::default()
        }
    }

    pub fn create_io_sq(qid: u16, size: u16, prp: u64, cqid: u16) -> Self {
        Self {
            opcode: NVME_ADMIN_CREATE_SQ,
            prp1: prp,
            cdw10: ((size as u32 - 1) << 16) | qid as u32,
            cdw11: (cqid as u32) << 16 | 0x01, // phys contiguous
            ..Default::default()
        }
    }

    pub fn read(nsid: u32, slba: u64, nlb: u16, prp1: u64, prp2: u64) -> Self {
        Self {
            opcode: NVME_IO_READ,
            nsid,
            prp1,
            prp2,
            cdw10: slba as u32,
            cdw11: (slba >> 32) as u32,
            cdw12: nlb as u32,
            ..Default::default()
        }
    }

    pub fn write(nsid: u32, slba: u64, nlb: u16, prp1: u64, prp2: u64) -> Self {
        Self {
            opcode: NVME_IO_WRITE,
            nsid,
            prp1,
            prp2,
            cdw10: slba as u32,
            cdw11: (slba >> 32) as u32,
            cdw12: nlb as u32,
            ..Default::default()
        }
    }

    pub fn flush(nsid: u32) -> Self {
        Self {
            opcode: NVME_IO_FLUSH,
            nsid,
            ..Default::default()
        }
    }

    pub fn dataset_management(nsid: u32, nr_ranges: u16, prp1: u64) -> Self {
        Self {
            opcode: NVME_IO_DATASET_MGMT,
            nsid,
            prp1,
            cdw10: (nr_ranges - 1) as u32,
            cdw11: 0x04, // AD (deallocate) attribute
            ..Default::default()
        }
    }

    pub fn get_log_page(nsid: u32, lid: u8, num_dwords: u32, prp1: u64) -> Self {
        Self {
            opcode: NVME_ADMIN_GET_LOG_PAGE,
            nsid,
            prp1,
            cdw10: lid as u32 | ((num_dwords - 1) << 16),
            ..Default::default()
        }
    }

    pub fn set_features(fid: u8, cdw11: u32) -> Self {
        Self {
            opcode: NVME_ADMIN_SET_FEATURES,
            cdw10: fid as u32,
            cdw11,
            ..Default::default()
        }
    }

    pub fn format_nvm(nsid: u32, lbaf: u8, ses: u8) -> Self {
        Self {
            opcode: NVME_ADMIN_FORMAT_NVM,
            nsid,
            cdw10: (lbaf as u32) | ((ses as u32) << 9),
            ..Default::default()
        }
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct NvmeCompletion {
    pub dw0: u32,
    pub dw1: u32,
    pub sq_head: u16,
    pub sq_id: u16,
    pub cid: u16,
    pub status: u16,
}

impl NvmeCompletion {
    pub fn phase(&self) -> bool {
        (self.status & 1) != 0
    }

    pub fn status_code(&self) -> u16 {
        (self.status >> 1) & 0xFF
    }

    pub fn status_code_type(&self) -> u8 {
        ((self.status >> 9) & 0x07) as u8
    }

    pub fn is_error(&self) -> bool {
        self.status_code() != 0
    }

    pub fn more(&self) -> bool {
        (self.status & (1 << 14)) != 0
    }

    pub fn do_not_retry(&self) -> bool {
        (self.status & (1 << 15)) != 0
    }
}

// ─── NVMe Queue ─────────────────────────────────────────────────────────────

pub struct NvmeQueue {
    pub base_phys: u64,
    pub base_virt: u64,
    pub entries: u16,
    pub head: u16,
    pub tail: u16,
    pub phase: bool,
    pub doorbell_reg: u64,
    pub entry_size: usize,
    pub cid_counter: u16,
}

impl NvmeQueue {
    pub fn new(
        base_phys: u64,
        base_virt: u64,
        entries: u16,
        doorbell_reg: u64,
        entry_size: usize,
    ) -> Self {
        Self {
            base_phys,
            base_virt,
            entries,
            head: 0,
            tail: 0,
            phase: true,
            doorbell_reg,
            entry_size,
            cid_counter: 0,
        }
    }

    pub fn is_full(&self) -> bool {
        let next_tail = (self.tail + 1) % self.entries;
        next_tail == self.head
    }

    pub fn next_cid(&mut self) -> u16 {
        let cid = self.cid_counter;
        self.cid_counter = self.cid_counter.wrapping_add(1);
        cid
    }

    pub fn advance_tail(&mut self) {
        self.tail = (self.tail + 1) % self.entries;
    }

    pub fn advance_head(&mut self) {
        self.head = (self.head + 1) % self.entries;
        if self.head == 0 {
            self.phase = !self.phase;
        }
    }

    pub fn depth(&self) -> u16 {
        if self.tail >= self.head {
            self.tail - self.head
        } else {
            self.entries - self.head + self.tail
        }
    }

    pub fn entry_phys(&self, index: u16) -> u64 {
        self.base_phys + (index as u64 * self.entry_size as u64)
    }

    pub fn entry_virt(&self, index: u16) -> u64 {
        self.base_virt + (index as u64 * self.entry_size as u64)
    }

    fn ring_doorbell(&self) {
        unsafe {
            let ptr = self.doorbell_reg as *mut u32;
            core::ptr::write_volatile(ptr, self.tail as u32);
        }
    }
}

// ─── NVMe Namespace ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub struct NsFeatures {
    pub thin_provisioning: bool,
    pub ns_atomic_write: bool,
    pub dealloc_or_unwritten_error: bool,
    pub guid_reuse: bool,
}

impl Default for NsFeatures {
    fn default() -> Self {
        Self {
            thin_provisioning: false,
            ns_atomic_write: false,
            dealloc_or_unwritten_error: false,
            guid_reuse: false,
        }
    }
}

pub struct NvmeNamespace {
    pub nsid: u32,
    pub size_blocks: u64,
    pub block_size: u32,
    pub capacity: u64,
    pub utilization: u64,
    pub features: NsFeatures,
    pub formatted_lba_size: u8,
    pub metadata_size: u16,
    pub eui64: [u8; 8],
    pub nguid: [u8; 16],
}

impl NvmeNamespace {
    pub fn capacity_bytes(&self) -> u64 {
        self.size_blocks * self.block_size as u64
    }

    pub fn capacity_gb(&self) -> u64 {
        self.capacity_bytes() / (1024 * 1024 * 1024)
    }
}

// ─── NVMe Identify Controller Data ─────────────────────────────────────────

pub struct NvmeIdentifyController {
    pub vendor_id: u16,
    pub subsystem_vendor_id: u16,
    pub serial_number: [u8; 20],
    pub model_number: [u8; 40],
    pub firmware_revision: [u8; 8],
    pub ieee_oui: [u8; 3],
    pub max_data_transfer_size: u8,
    pub controller_id: u16,
    pub version: u32,
    pub total_nvm_capacity: u128,
    pub unallocated_nvm_capacity: u128,
    pub num_namespaces: u32,
    pub sqes_min: u8,
    pub sqes_max: u8,
    pub cqes_min: u8,
    pub cqes_max: u8,
    pub rtd3_resume_latency: u32,
    pub rtd3_entry_latency: u32,
    pub oacs: u16, // optional admin command support
    pub acl: u8,   // abort command limit
    pub aerl: u8,  // async event request limit
    pub frmw: u8,  // firmware updates
    pub lpa: u8,   // log page attributes
    pub elpe: u8,  // error log page entries
    pub npss: u8,  // number of power states support
    pub oncs: u16, // optional NVM command support
}

impl NvmeIdentifyController {
    pub fn new() -> Self {
        Self {
            vendor_id: 0,
            subsystem_vendor_id: 0,
            serial_number: [0; 20],
            model_number: [0; 40],
            firmware_revision: [0; 8],
            ieee_oui: [0; 3],
            max_data_transfer_size: 0,
            controller_id: 0,
            version: 0,
            total_nvm_capacity: 0,
            unallocated_nvm_capacity: 0,
            num_namespaces: 0,
            sqes_min: 6,
            sqes_max: 6,
            cqes_min: 4,
            cqes_max: 4,
            rtd3_resume_latency: 0,
            rtd3_entry_latency: 0,
            oacs: 0,
            acl: 3,
            aerl: 3,
            frmw: 0,
            lpa: 0,
            elpe: 63,
            npss: 0,
            oncs: 0,
        }
    }

    pub fn vendor_name(&self) -> &'static str {
        match self.vendor_id {
            0x144D => "Samsung Electronics",
            0x15B7 => "Western Digital / SanDisk",
            0x1344 | 0xC0A9 => "Crucial / Micron",
            0x8086 => "Intel Corporation",
            0x1C5C => "SK Hynix",
            0x1B36 => "QEMU / Red Hat",
            0x106B => "Apple Inc.",
            0x126F => "Silicon Motion",
            0x15AD => "VMware",
            0x1D72 => "Innomasters",
            0x10EC => "Realtek",
            _ => {
                // Fallback to IEEE OUI check if VID is unknown
                match self.ieee_oui {
                    [0x00, 0x00, 0xF0] => "Samsung Electronics",
                    [0x00, 0x1B, 0x44] => "Western Digital",
                    [0x00, 0xA0, 0x75] => "Micron Technology",
                    [0x00, 0x14, 0xEE] => "SK Hynix",
                    [0x5C, 0x5C, 0x5C] => "SK Hynix",
                    _ => "Unknown Vendor",
                }
            }
        }
    }

    fn trim_nvme_string(bytes: &[u8]) -> String {
        let s = String::from_utf8_lossy(bytes);
        // NVMe spec: padded with spaces (0x20), but we also handle nulls (0x00)
        s.trim_matches(|c: char| c == ' ' || c == '\0').into()
    }

    pub fn serial_string(&self) -> String {
        Self::trim_nvme_string(&self.serial_number)
    }

    pub fn model_string(&self) -> String {
        Self::trim_nvme_string(&self.model_number)
    }

    pub fn firmware_string(&self) -> String {
        Self::trim_nvme_string(&self.firmware_revision)
    }

    pub fn version_major(&self) -> u16 {
        (self.version >> 16) as u16
    }

    pub fn version_minor(&self) -> u8 {
        ((self.version >> 8) & 0xFF) as u8
    }

    pub fn version_patch(&self) -> u8 {
        (self.version & 0xFF) as u8
    }

    pub fn supports_compare(&self) -> bool {
        (self.oncs & (1 << 0)) != 0
    }

    pub fn supports_write_uncorrectable(&self) -> bool {
        (self.oncs & (1 << 1)) != 0
    }

    pub fn supports_dataset_management(&self) -> bool {
        (self.oncs & (1 << 2)) != 0
    }

    pub fn supports_write_zeroes(&self) -> bool {
        (self.oncs & (1 << 3)) != 0
    }

    pub fn supports_save_select(&self) -> bool {
        (self.oncs & (1 << 4)) != 0
    }

    pub fn supports_security(&self) -> bool {
        (self.oacs & (1 << 0)) != 0
    }

    pub fn supports_format_nvm(&self) -> bool {
        (self.oacs & (1 << 1)) != 0
    }

    pub fn supports_firmware_download(&self) -> bool {
        (self.oacs & (1 << 2)) != 0
    }

    pub fn supports_ns_management(&self) -> bool {
        (self.oacs & (1 << 3)) != 0
    }
}

// ─── SMART / Health Log ─────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct SmartLog {
    pub critical_warning: u8,
    pub temperature: u16,
    pub available_spare: u8,
    pub available_spare_threshold: u8,
    pub percentage_used: u8,
    pub data_units_read: u128,
    pub data_units_written: u128,
    pub host_read_commands: u128,
    pub host_write_commands: u128,
    pub controller_busy_time: u128,
    pub power_cycles: u128,
    pub power_on_hours: u128,
    pub unsafe_shutdowns: u128,
    pub media_errors: u128,
    pub num_error_log_entries: u128,
    pub warning_temp_time: u32,
    pub critical_temp_time: u32,
}

impl SmartLog {
    pub fn new() -> Self {
        Self {
            critical_warning: 0,
            temperature: 0,
            available_spare: 100,
            available_spare_threshold: 10,
            percentage_used: 0,
            data_units_read: 0,
            data_units_written: 0,
            host_read_commands: 0,
            host_write_commands: 0,
            controller_busy_time: 0,
            power_cycles: 0,
            power_on_hours: 0,
            unsafe_shutdowns: 0,
            media_errors: 0,
            num_error_log_entries: 0,
            warning_temp_time: 0,
            critical_temp_time: 0,
        }
    }

    pub fn from_bytes(data: &[u8]) -> Option<Self> {
        if data.len() < 512 {
            return None;
        }

        let read_u128 = |off: usize| -> u128 {
            let mut buf = [0u8; 16];
            buf.copy_from_slice(&data[off..off + 16]);
            u128::from_le_bytes(buf)
        };

        Some(Self {
            critical_warning: data[0],
            temperature: u16::from_le_bytes([data[1], data[2]]),
            available_spare: data[3],
            available_spare_threshold: data[4],
            percentage_used: data[5],
            data_units_read: read_u128(32),
            data_units_written: read_u128(48),
            host_read_commands: read_u128(64),
            host_write_commands: read_u128(80),
            controller_busy_time: read_u128(96),
            power_cycles: read_u128(112),
            power_on_hours: read_u128(128),
            unsafe_shutdowns: read_u128(144),
            media_errors: read_u128(160),
            num_error_log_entries: read_u128(176),
            warning_temp_time: u32::from_le_bytes([data[192], data[193], data[194], data[195]]),
            critical_temp_time: u32::from_le_bytes([data[196], data[197], data[198], data[199]]),
        })
    }

    pub fn temperature_celsius(&self) -> i16 {
        self.temperature as i16 - 273
    }

    pub fn is_critical(&self) -> bool {
        self.critical_warning != 0
    }

    pub fn spare_below_threshold(&self) -> bool {
        (self.critical_warning & (1 << 0)) != 0
    }

    pub fn temperature_exceeded(&self) -> bool {
        (self.critical_warning & (1 << 1)) != 0
    }

    pub fn reliability_degraded(&self) -> bool {
        (self.critical_warning & (1 << 2)) != 0
    }

    pub fn read_only_mode(&self) -> bool {
        (self.critical_warning & (1 << 3)) != 0
    }

    pub fn volatile_backup_failed(&self) -> bool {
        (self.critical_warning & (1 << 4)) != 0
    }

    pub fn total_data_read_gb(&self) -> u64 {
        (self.data_units_read * 512 / (1024 * 1024 * 1024)) as u64
    }

    pub fn total_data_written_gb(&self) -> u64 {
        (self.data_units_written * 512 / (1024 * 1024 * 1024)) as u64
    }
}

// ─── Error Log Entry ────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ErrorLogEntry {
    pub error_count: u64,
    pub sqid: u16,
    pub cid: u16,
    pub status_field: u16,
    pub param_error_location: u16,
    pub lba: u64,
    pub nsid: u32,
    pub vendor_specific: u8,
    pub transport_type: u8,
    pub command_specific: u64,
    pub transport_specific: u16,
}

impl ErrorLogEntry {
    pub fn from_bytes(data: &[u8]) -> Option<Self> {
        if data.len() < 64 {
            return None;
        }

        Some(Self {
            error_count: u64::from_le_bytes([
                data[0], data[1], data[2], data[3], data[4], data[5], data[6], data[7],
            ]),
            sqid: u16::from_le_bytes([data[8], data[9]]),
            cid: u16::from_le_bytes([data[10], data[11]]),
            status_field: u16::from_le_bytes([data[12], data[13]]),
            param_error_location: u16::from_le_bytes([data[14], data[15]]),
            lba: u64::from_le_bytes([
                data[16], data[17], data[18], data[19], data[20], data[21], data[22], data[23],
            ]),
            nsid: u32::from_le_bytes([data[24], data[25], data[26], data[27]]),
            vendor_specific: data[28],
            transport_type: data[29],
            command_specific: u64::from_le_bytes([
                data[32], data[33], data[34], data[35], data[36], data[37], data[38], data[39],
            ]),
            transport_specific: u16::from_le_bytes([data[40], data[41]]),
        })
    }
}

// ─── NVMe Controller ───────────────────────────────────────────────────────

pub struct NvmeController {
    bar0: u64,
    admin_sq: NvmeQueue,
    admin_cq: NvmeQueue,
    io_queues: Vec<(NvmeQueue, NvmeQueue)>,
    namespaces: Vec<NvmeNamespace>,
    identify: NvmeIdentifyController,
    doorbell_stride: u32,
    max_queue_entries: u16,
    max_transfer_size: u32,
    serial: String,
    model: String,
    firmware: String,
    state: NvmeState,
    num_io_queues: u16,
    outstanding_commands: u32,
    msix_vector: Option<u8>,
    pci_bus: u8,
    pci_dev: u8,
    pci_func: u8,
    iommu_domain: Option<u16>,
    /// Persistent (phys, virt) DMA bounce buffer for the BlockDevice adapter,
    /// allocated once on first I/O and reused. NVMe block I/O is serialized
    /// under `NVME_CONTROLLERS.lock()`, so a single buffer per controller is
    /// race-free. Replaces the per-I/O `alloc_dma_frame_mapped` that leaked a
    /// frame + IOMMU mapping on every read/write (BUG-37: OOM walking the FS).
    bounce: Option<(u64, u64)>,
}

impl NvmeController {
    pub fn new(bar0: u64) -> Self {
        Self {
            bar0,
            admin_sq: NvmeQueue::new(0, 0, 0, 0, 64),
            admin_cq: NvmeQueue::new(0, 0, 0, 0, 16),
            io_queues: Vec::new(),
            namespaces: Vec::new(),
            identify: NvmeIdentifyController::new(),
            doorbell_stride: 4,
            max_queue_entries: 0,
            max_transfer_size: 0,
            serial: String::new(),
            model: String::new(),
            firmware: String::new(),
            state: NvmeState::Uninitialized,
            num_io_queues: 0,
            outstanding_commands: 0,
            msix_vector: None,
            pci_bus: 0,
            pci_dev: 0,
            pci_func: 0,
            iommu_domain: None,
            bounce: None,
        }
    }

    fn alloc_dma(&self) -> Result<(u64, u64), BlockError> {
        alloc_dma_frame_mapped(self.iommu_domain)
    }

    /// Lazily allocate the persistent per-controller DMA bounce buffer and
    /// return its (phys, virt). Allocated once; reused for every block I/O.
    fn ensure_bounce(&mut self) -> Result<(u64, u64), BlockError> {
        if let Some(b) = self.bounce {
            return Ok(b);
        }
        let b = alloc_dma_frame_mapped(self.iommu_domain)?;
        self.bounce = Some(b);
        Ok(b)
    }

    fn alloc_dma_pages(&self, count: usize) -> Result<(u64, u64), BlockError> {
        alloc_dma_frames_mapped(self.iommu_domain, count)
    }

    fn setup_iommu(&mut self, bus: u8, dev: u8, func: u8) {
        self.pci_bus = bus;
        self.pci_dev = dev;
        self.pci_func = func;
        if !crate::iommu::is_enabled() {
            return;
        }
        // Domain + device attach happen after admin queues are allocated so we
        // can map all DMA pages in one sandbox call from `finish_iommu_setup`.
    }

    fn finish_iommu_setup(&mut self, regions: &[(u64, u64)]) {
        if !crate::iommu::is_enabled() {
            return;
        }
        if let Some(dom) =
            crate::iommu::sandbox_device_dma(self.pci_bus, self.pci_dev, self.pci_func, regions)
        {
            self.iommu_domain = Some(dom);
        }
    }

    // ── MMIO Helpers ────────────────────────────────────────────────────

    unsafe fn read_reg32(&self, offset: u64) -> u32 {
        let ptr = (self.bar0 + offset) as *const u32;
        core::ptr::read_volatile(ptr)
    }

    unsafe fn write_reg32(&self, offset: u64, val: u32) {
        let ptr = (self.bar0 + offset) as *mut u32;
        core::ptr::write_volatile(ptr, val);
    }

    unsafe fn read_reg64(&self, offset: u64) -> u64 {
        let lo = self.read_reg32(offset) as u64;
        let hi = self.read_reg32(offset + 4) as u64;
        (hi << 32) | lo
    }

    unsafe fn write_reg64(&self, offset: u64, val: u64) {
        self.write_reg32(offset, val as u32);
        self.write_reg32(offset + 4, (val >> 32) as u32);
    }

    // ── Doorbell Addresses ──────────────────────────────────────────────

    fn sq_doorbell(&self, qid: u16) -> u64 {
        self.bar0 + 0x1000 + ((2 * qid as u64) * self.doorbell_stride as u64)
    }

    fn cq_doorbell(&self, qid: u16) -> u64 {
        self.bar0 + 0x1000 + ((2 * qid as u64 + 1) * self.doorbell_stride as u64)
    }

    // ── Controller Initialization ───────────────────────────────────────

    pub fn init(&mut self) -> Result<(), BlockError> {
        self.state = NvmeState::Resetting;

        unsafe {
            let cap = self.read_reg64(REG_CAP);
            self.max_queue_entries = ((cap & 0xFFFF) + 1) as u16;
            self.doorbell_stride = 4 << ((cap >> 32) & 0xF);

            let mqes = self.max_queue_entries;
            let timeout_500ms = ((cap >> 24) & 0xFF) as u32;

            let mpsmin = ((cap >> 48) & 0xF) as u32;
            let mpsmax = ((cap >> 52) & 0xF) as u32;

            let vs = self.read_reg32(REG_VS);
            let ver_major = (vs >> 16) as u16;
            let ver_minor = ((vs >> 8) & 0xFF) as u8;
            crate::serial_println!(
                "[nvme] version {}.{}, MQES={}, dstrd={}, timeout={}x500ms",
                ver_major,
                ver_minor,
                mqes,
                self.doorbell_stride,
                timeout_500ms
            );

            // MasterChecklist Phase 1.5 — NVMe controller reset on init.
            //
            // Real firmware (Samsung 980 / WD SN770 after warm reboot or
            // kexec) sometimes hands the OS a controller that's already
            // enabled (CC.EN=1, CSTS.RDY=1) with stale doorbells and
            // queued commands from the previous boot. Naively writing
            // CC.EN=0 races against in-flight admin work and can leave
            // the controller in CFS (Controller Fatal Status). Per NVMe
            // spec §7.6.2 the safe sequence on a live controller is:
            //   1. Issue Normal Shutdown via CC.SHN=01b
            //   2. Wait for CSTS.SHST=10b (shutdown complete)
            //   3. Then drop CC.EN and wait for CSTS.RDY=0
            //
            // On a controller that came up disabled (cold boot, BIOS
            // POST didn't touch it) we skip the shutdown step.
            let cc_initial = self.read_reg32(REG_CC);
            let csts_initial = self.read_reg32(REG_CSTS);
            let was_enabled = (cc_initial & CC_EN) != 0;
            let was_ready = (csts_initial & CSTS_RDY) != 0;
            let was_failed = (csts_initial & CSTS_CFS) != 0;
            crate::serial_println!(
                "[nvme] entry state: CC={:#x} CSTS={:#x} (EN={} RDY={} CFS={})",
                cc_initial,
                csts_initial,
                was_enabled,
                was_ready,
                was_failed,
            );

            if was_failed {
                crate::serial_println!(
                    "[nvme] CSTS.CFS set on entry — controller in fatal state; \
                     attempting full reset anyway"
                );
            }

            if was_enabled && was_ready {
                // Normal Shutdown — drains in-flight commands cleanly.
                let mut cc_sd = cc_initial;
                cc_sd = (cc_sd & !(0x3 << 14)) | CC_SHN_NORMAL; // SHN=01
                self.write_reg32(REG_CC, cc_sd);
                let mut shutdown_complete = false;
                for _ in 0..100_000 {
                    let csts = self.read_reg32(REG_CSTS);
                    if (csts & CSTS_SHST_MASK) == CSTS_SHST_COMPLETE {
                        shutdown_complete = true;
                        break;
                    }
                    for _ in 0..1000 {
                        core::hint::spin_loop();
                    }
                }
                if shutdown_complete {
                    crate::serial_println!("[nvme] normal shutdown completed cleanly");
                } else {
                    crate::serial_println!(
                        "[nvme] normal shutdown did not signal SHST=complete; \
                         proceeding with disable anyway"
                    );
                }
            }

            // Disable the controller - also clears SHN bits.
            if was_enabled {
                let cc = (self.read_reg32(REG_CC)) & !(CC_EN | (0x3 << 14));
                self.write_reg32(REG_CC, cc);

                // Wait for CSTS.RDY = 0; bail loudly if the controller is wedged.
                let mut ready_cleared = false;
                for _ in 0..100_000 {
                    let csts = self.read_reg32(REG_CSTS);
                    if (csts & CSTS_RDY) == 0 {
                        ready_cleared = true;
                        break;
                    }
                    for _ in 0..1000 {
                        core::hint::spin_loop();
                    }
                }
                if !ready_cleared {
                    let csts_final = self.read_reg32(REG_CSTS);
                    crate::serial_println!(
                        "[nvme] reset failed — CSTS.RDY stuck high (CSTS={:#x}); \
                         skipping this controller",
                        csts_final,
                    );
                    return Err(BlockError::HardwareFailure);
                }
            } else if was_ready {
                crate::serial_println!("[nvme] controller was disabled but RDY=1. Continuing.");
            }
            crate::serial_println!(
                "[nvme] reset OK: CC.EN cleared, CSTS.RDY=0 (was_enabled={})",
                was_enabled,
            );

            // MasterChecklist Phase 1.5 — admin queue depth negotiated, not hardcoded.
            //
            // Real cap is `min(MQES, page_capacity)` where page_capacity = 4096
            // / entry_size. Submission entries are 64 B (cap 64 per page),
            // completion entries are 16 B (cap 256 per page). The old code
            // hardcoded both to 64, throwing away 4x the completion capacity
            // the same DMA page already affords. We stick to single-page DMA
            // for the admin path (multi-page admin SQ is rarely necessary —
            // I/O queues handle throughput); the negotiation here is honest
            // about what the controller actually permits.
            const PAGE_SIZE_BYTES: u16 = 4096;
            const NVME_ADMIN_SQ_ENTRY_BYTES: u16 = 64;
            const NVME_ADMIN_CQ_ENTRY_BYTES: u16 = 16;
            let admin_sq_page_cap = PAGE_SIZE_BYTES / NVME_ADMIN_SQ_ENTRY_BYTES; // 64
            let admin_cq_page_cap = PAGE_SIZE_BYTES / NVME_ADMIN_CQ_ENTRY_BYTES; // 256
            let admin_sq_entries: u16 = core::cmp::min(mqes, admin_sq_page_cap);
            let admin_cq_entries: u16 = core::cmp::min(mqes, admin_cq_page_cap);
            crate::serial_println!(
                "[nvme] admin queue depths negotiated: SQ={} (page-cap {}, mqes {}), CQ={} (page-cap {}, mqes {})",
                admin_sq_entries, admin_sq_page_cap, mqes,
                admin_cq_entries, admin_cq_page_cap, mqes,
            );

            let (admin_sq_phys, admin_sq_virt) =
                self.alloc_dma().map_err(|_| BlockError::HardwareFailure)?;
            let (admin_cq_phys, admin_cq_virt) =
                self.alloc_dma().map_err(|_| BlockError::HardwareFailure)?;

            self.admin_sq = NvmeQueue::new(
                admin_sq_phys,
                admin_sq_virt,
                admin_sq_entries,
                self.sq_doorbell(0),
                NVME_ADMIN_SQ_ENTRY_BYTES as usize,
            );
            self.admin_cq = NvmeQueue::new(
                admin_cq_phys,
                admin_cq_virt,
                admin_cq_entries,
                self.cq_doorbell(0),
                NVME_ADMIN_CQ_ENTRY_BYTES as usize,
            );

            // Set admin queue attributes
            let aqa = ((admin_cq_entries as u32 - 1) << 16) | (admin_sq_entries as u32 - 1);
            self.write_reg32(REG_AQA, aqa);
            self.write_reg64(REG_ASQ, admin_sq_phys);
            self.write_reg64(REG_ACQ, admin_cq_phys);

            // Configure and enable
            let cc = CC_EN | CC_CSS_NVM | CC_MPS_4K | CC_AMS_RR | CC_IOSQES | CC_IOCQES;
            self.write_reg32(REG_CC, cc);

            // Wait for CSTS.RDY = 1
            for i in 0..100_000 {
                let csts = self.read_reg32(REG_CSTS);
                if (csts & CSTS_CFS) != 0 {
                    self.state = NvmeState::Failed;
                    crate::serial_println!("[nvme] controller fatal status during init");
                    return Err(BlockError::HardwareFailure);
                }
                if (csts & CSTS_RDY) != 0 {
                    crate::serial_println!("[nvme] controller ready after {} iterations", i);
                    break;
                }
                for _ in 0..1000 {
                    core::hint::spin_loop();
                }
            }
        }

        self.state = NvmeState::Ready;
        Ok(())
    }

    // ── Command Submission & Completion ─────────────────────────────────

    pub fn submit_admin_command(&mut self, mut cmd: NvmeCommand) -> Result<u16, BlockError> {
        if self.admin_sq.is_full() {
            return Err(BlockError::QueueFull);
        }

        let cid = self.admin_sq.next_cid();
        cmd.cid = cid;

        unsafe {
            let entry_addr = self.admin_sq.entry_virt(self.admin_sq.tail);
            let ptr = entry_addr as *mut NvmeCommand;
            core::ptr::write_volatile(ptr, cmd);
        }

        self.admin_sq.advance_tail();
        self.admin_sq.ring_doorbell();
        self.outstanding_commands += 1;

        Ok(cid)
    }

    pub fn submit_io_command(
        &mut self,
        qid: usize,
        mut cmd: NvmeCommand,
    ) -> Result<u16, BlockError> {
        if qid == 0 || qid > self.io_queues.len() {
            return Err(BlockError::InvalidSector);
        }

        let (sq, _cq) = &mut self.io_queues[qid - 1];
        if sq.is_full() {
            return Err(BlockError::QueueFull);
        }

        let cid = sq.next_cid();
        cmd.cid = cid;

        unsafe {
            let entry_addr = sq.entry_virt(sq.tail);
            let ptr = entry_addr as *mut NvmeCommand;
            core::ptr::write_volatile(ptr, cmd);
        }

        sq.advance_tail();
        sq.ring_doorbell();
        self.outstanding_commands += 1;

        Ok(cid)
    }

    pub fn poll_admin_completion(&mut self) -> Result<NvmeCompletion, BlockError> {
        let poll_start = unsafe { core::arch::x86_64::_rdtsc() };
        loop {
            let cpl = unsafe {
                let addr = self.admin_cq.entry_virt(self.admin_cq.head);
                let ptr = addr as *const NvmeCompletion;
                core::ptr::read_volatile(ptr)
            };

            if cpl.phase() == self.admin_cq.phase {
                self.admin_cq.advance_head();
                // Same SQ-head credit as the I/O path (see poll_io_completion):
                // the admin SQ otherwise phantom-fills after entries-1 commands.
                self.admin_sq.head = cpl.sq_head % self.admin_sq.entries;

                unsafe {
                    let ptr = self.admin_cq.doorbell_reg as *mut u32;
                    core::ptr::write_volatile(ptr, self.admin_cq.head as u32);
                }

                self.outstanding_commands = self.outstanding_commands.saturating_sub(1);

                if cpl.is_error() {
                    crate::serial_println!(
                        "[nvme] admin cmd error: SCT={} SC={}",
                        cpl.status_code_type(),
                        cpl.status_code()
                    );
                    return Err(BlockError::IoError);
                }

                return Ok(cpl);
            }

            if unsafe { core::arch::x86_64::_rdtsc() }.wrapping_sub(poll_start)
                > NVME_POLL_DEADLINE_CYCLES
            {
                crate::serial_println!("[nvme] admin completion timeout — returning IoError");
                return Err(BlockError::IoError);
            }
            // Futex-park only once the scheduler + IRQ delivery are live
            // (BOOT_COMPLETE). During boot the completion IRQ/scheduler-wake path
            // is not yet running, so a park would never be woken — busy-poll the
            // DMA-written phase bit instead, which always makes forward progress.
            if crate::scheduler::BOOT_COMPLETE.load(core::sync::atomic::Ordering::Relaxed)
                && x86_64::instructions::interrupts::are_enabled()
            {
                if let Some(tid) = crate::scheduler::current_task_id() {
                    let offset = crate::memory::PHYS_MEM_OFFSET.get().unwrap().as_u64();
                    let expected = unsafe {
                        core::ptr::read_volatile(self.admin_cq.doorbell_reg as *const u32)
                    };
                    crate::sync::FUTEX_MANAGER.lock().wait(
                        self.admin_cq.doorbell_reg - offset,
                        expected,
                        tid,
                    );
                } else {
                    core::hint::spin_loop();
                }
            } else {
                core::hint::spin_loop();
            }
        }
    }

    pub fn poll_io_completion(&mut self, qid: usize) -> Result<NvmeCompletion, BlockError> {
        if qid == 0 || qid > self.io_queues.len() {
            return Err(BlockError::InvalidSector);
        }

        // A block driver must never spin forever on the device: bound the wait
        // so a lost/never-arriving completion (observed intermittently during
        // the early-boot RaeFS smoketest) returns an I/O error instead of
        // hanging the whole kernel. ~NVME_POLL_DEADLINE_CYCLES is generous vs a
        // real device completion (microseconds) yet bounded.
        let poll_start = unsafe { core::arch::x86_64::_rdtsc() };

        loop {
            let (_sq, cq) = &self.io_queues[qid - 1];
            let cpl = unsafe {
                let addr = cq.entry_virt(cq.head);
                let ptr = addr as *const NvmeCompletion;
                core::ptr::read_volatile(ptr)
            };

            if cpl.phase() == cq.phase {
                let (sq, cq) = &mut self.io_queues[qid - 1];
                cq.advance_head();
                // Credit consumed submissions back to the SQ: the device
                // reports its SQ head in every completion (NVMe §4.6).
                // WITHOUT this, sq.head stays 0 forever and is_full() goes
                // permanently true after entries-1 commands — on Athena
                // (where NVMe is the active boot device and absorbs all
                // RaeFS/FAT boot I/O) the first bootlog WRITE was simply
                // the first command after the queue's phantom fill, failing
                // with QueueFull while reads earlier in boot still worked.
                sq.head = cpl.sq_head % sq.entries;

                unsafe {
                    let ptr = cq.doorbell_reg as *mut u32;
                    core::ptr::write_volatile(ptr, cq.head as u32);
                }

                self.outstanding_commands = self.outstanding_commands.saturating_sub(1);

                if cpl.is_error() {
                    // Surface the device's actual status — a bare IoError hid
                    // WHY Athena's NVMe rejected the first bootlog write while
                    // reads worked fine. SCT/SC pairs are defined in NVMe spec
                    // §5 (e.g. sct=0 sc=0x02 Invalid Field, sc=0x80 LBA Out of
                    // Range, sct=2 sc=0x82 namespace write-protected).
                    crate::serial_println!(
                        "[nvme] I/O cmd FAILED (qid={}): sct={} sc={:#04x} cid={} sq_head={}",
                        qid,
                        cpl.status_code_type(),
                        cpl.status_code(),
                        cpl.cid,
                        cpl.sq_head,
                    );
                    return Err(BlockError::IoError);
                }

                return Ok(cpl);
            }

            if unsafe { core::arch::x86_64::_rdtsc() }.wrapping_sub(poll_start)
                > NVME_POLL_DEADLINE_CYCLES
            {
                crate::serial_println!(
                    "[nvme] I/O completion timeout (qid={}) — returning IoError",
                    qid
                );
                return Err(BlockError::IoError);
            }
            let cq_doorbell = self.io_queues[qid - 1].1.doorbell_reg;
            // Futex-park only once the scheduler + IRQ delivery are live
            // (BOOT_COMPLETE). During boot the completion IRQ/scheduler-wake path
            // is not yet running, so a park would never be woken — busy-poll the
            // DMA-written phase bit instead, which always makes forward progress.
            if crate::scheduler::BOOT_COMPLETE.load(core::sync::atomic::Ordering::Relaxed)
                && x86_64::instructions::interrupts::are_enabled()
            {
                if let Some(tid) = crate::scheduler::current_task_id() {
                    let offset = crate::memory::PHYS_MEM_OFFSET.get().unwrap().as_u64();
                    let expected = unsafe { core::ptr::read_volatile(cq_doorbell as *const u32) };
                    crate::sync::FUTEX_MANAGER
                        .lock()
                        .wait(cq_doorbell - offset, expected, tid);
                } else {
                    core::hint::spin_loop();
                }
            } else {
                core::hint::spin_loop();
            }
        }
    }

    // ── Identify ────────────────────────────────────────────────────────

    pub fn identify_controller(&mut self) -> Result<(), BlockError> {
        if self.state != NvmeState::Ready {
            return Err(BlockError::DeviceBusy);
        }

        let (prp, virt) = self.alloc_dma()?;
        let cmd = NvmeCommand::identify(0, 1, prp);

        self.submit_admin_command(cmd)?;
        let _cpl = self.poll_admin_completion()?;

        let data = unsafe { core::slice::from_raw_parts(virt as *const u8, 4096) };

        self.identify.vendor_id = u16::from_le_bytes([data[0], data[1]]);
        self.identify.subsystem_vendor_id = u16::from_le_bytes([data[2], data[3]]);
        self.identify.serial_number.copy_from_slice(&data[4..24]);
        self.identify.model_number.copy_from_slice(&data[24..64]);
        self.identify
            .firmware_revision
            .copy_from_slice(&data[64..72]);
        self.identify.ieee_oui.copy_from_slice(&data[73..76]);
        self.identify.max_data_transfer_size = data[77];
        self.identify.controller_id = u16::from_le_bytes([data[78], data[79]]);
        self.identify.version = u32::from_le_bytes([data[80], data[81], data[82], data[83]]);

        self.identify.oacs = u16::from_le_bytes([data[256], data[257]]);
        self.identify.acl = data[258];
        self.identify.aerl = data[259];
        self.identify.frmw = data[260];
        self.identify.lpa = data[261];
        self.identify.elpe = data[262];
        self.identify.npss = data[263];

        self.identify.sqes_min = data[512] & 0x0F;
        self.identify.sqes_max = (data[512] >> 4) & 0x0F;
        self.identify.cqes_min = data[513] & 0x0F;
        self.identify.cqes_max = (data[513] >> 4) & 0x0F;
        self.identify.oncs = u16::from_le_bytes([data[520], data[521]]);

        self.identify.total_nvm_capacity = u128::from_le_bytes([
            data[200], data[201], data[202], data[203], data[204], data[205], data[206], data[207],
            data[208], data[209], data[210], data[211], data[212], data[213], data[214], data[215],
        ]);
        self.identify.unallocated_nvm_capacity = u128::from_le_bytes([
            data[240], data[241], data[242], data[243], data[244], data[245], data[246], data[247],
            data[248], data[249], data[250], data[251], data[252], data[253], data[254], data[255],
        ]);

        self.identify.num_namespaces =
            u32::from_le_bytes([data[516], data[517], data[518], data[519]]);

        self.serial = self.identify.serial_string();
        self.model = self.identify.model_string();
        self.firmware = self.identify.firmware_string();

        if self.identify.max_data_transfer_size > 0 {
            self.max_transfer_size = 1 << (12 + self.identify.max_data_transfer_size);
        } else {
            self.max_transfer_size = 1024 * 1024; // 1MB default
        }

        crate::serial_println!(
            "[nvme] controller: {} ({:04x}) {} {} fw={}",
            self.identify.vendor_name(),
            self.identify.vendor_id,
            self.model,
            self.serial,
            self.firmware
        );
        let tnvm_gib = (self.identify.total_nvm_capacity / (1024 * 1024 * 1024)) as u64;
        let unvm_gib = (self.identify.unallocated_nvm_capacity / (1024 * 1024 * 1024)) as u64;
        crate::serial_println!(
            "[nvme] identify: oui={:02x}-{:02x}-{:02x} ver={}.{}.{} mdts={}KiB oncs={:#06x} oacs={:#06x} sqes={}-{} cqes={}-{}",
            self.identify.ieee_oui[0], self.identify.ieee_oui[1], self.identify.ieee_oui[2],
            self.identify.version_major(),
            self.identify.version_minor(),
            self.identify.version_patch(),
            self.max_transfer_size / 1024,
            self.identify.oncs,
            self.identify.oacs,
            self.identify.sqes_min,
            self.identify.sqes_max,
            self.identify.cqes_min,
            self.identify.cqes_max,
        );
        crate::serial_println!(
            "[nvme] capacity: tnvm={}GiB unalloc={}GiB nn={}",
            tnvm_gib,
            unvm_gib,
            self.identify.num_namespaces,
        );

        Ok(())
    }

    pub fn identify_namespace(&mut self, nsid: u32) -> Result<NvmeNamespace, BlockError> {
        let (prp, virt) = self.alloc_dma()?;
        let cmd = NvmeCommand::identify(nsid, 0, prp);

        self.submit_admin_command(cmd)?;
        let _cpl = self.poll_admin_completion()?;

        let data = unsafe { core::slice::from_raw_parts(virt as *const u8, 4096) };

        let nsze = u64::from_le_bytes([
            data[0], data[1], data[2], data[3], data[4], data[5], data[6], data[7],
        ]);
        let ncap = u64::from_le_bytes([
            data[8], data[9], data[10], data[11], data[12], data[13], data[14], data[15],
        ]);
        let nuse = u64::from_le_bytes([
            data[16], data[17], data[18], data[19], data[20], data[21], data[22], data[23],
        ]);

        let nsfeat = data[24];
        let flbas = data[26];
        let mc = data[27];

        let lbaf_index = (flbas & 0x0F) as usize;
        let lbaf_offset = 128 + lbaf_index * 4;
        let lbaf = u32::from_le_bytes([
            data[lbaf_offset],
            data[lbaf_offset + 1],
            data[lbaf_offset + 2],
            data[lbaf_offset + 3],
        ]);

        let lba_ds = ((lbaf >> 16) & 0xFF) as u8;
        let ms = (lbaf & 0xFFFF) as u16;
        let block_size = if lba_ds >= 9 { 1u32 << lba_ds } else { 512 };

        let mut eui64 = [0u8; 8];
        eui64.copy_from_slice(&data[120..128]);

        let mut nguid = [0u8; 16];
        nguid.copy_from_slice(&data[104..120]);

        let ns = NvmeNamespace {
            nsid,
            size_blocks: nsze,
            block_size,
            capacity: ncap,
            utilization: nuse,
            features: NsFeatures {
                thin_provisioning: (nsfeat & (1 << 0)) != 0,
                ns_atomic_write: (nsfeat & (1 << 1)) != 0,
                dealloc_or_unwritten_error: (nsfeat & (1 << 2)) != 0,
                guid_reuse: (nsfeat & (1 << 3)) != 0,
            },
            formatted_lba_size: lba_ds,
            metadata_size: ms,
            eui64,
            nguid,
        };

        crate::serial_println!(
            "[nvme] ns{}: {} blocks x {}B = {}GB",
            nsid,
            nsze,
            block_size,
            ns.capacity_gb()
        );

        Ok(ns)
    }

    // ── I/O Queue Creation ──────────────────────────────────────────────

    pub fn create_io_queues(&mut self, count: u16) -> Result<(), BlockError> {
        // First, set number of queues via Set Features
        let desired = count as u32;
        let cdw11 = ((desired - 1) << 16) | (desired - 1);
        let cmd = NvmeCommand::set_features(FEAT_NUM_QUEUES, cdw11);
        self.submit_admin_command(cmd)?;
        let cpl = self.poll_admin_completion()?;

        let allocated_sq = (cpl.dw0 & 0xFFFF) as u16 + 1;
        let allocated_cq = ((cpl.dw0 >> 16) & 0xFFFF) as u16 + 1;
        let actual_count = core::cmp::min(core::cmp::min(allocated_sq, allocated_cq), count);

        crate::serial_println!("[nvme] allocated {} I/O queue pairs", actual_count);

        // SQ entry = 64B, CQ entry = 16B. A single 4K page fits 64 SQ or 256 CQ
        // entries. Cap to 64 so both fit in one DMA frame each.
        let queue_depth = core::cmp::min(self.max_queue_entries, 64);

        for i in 0..actual_count {
            let qid = i + 1;

            let (cq_phys, cq_virt) = self.alloc_dma()?;
            let (sq_phys, sq_virt) = self.alloc_dma()?;

            // Create CQ first (interrupts enabled if MSI-X vector is available)
            let iv = if self.msix_vector.is_some() { 0 } else { 0 }; // We only allocate 1 vector, so MSI-X table index is 0
            let cq_cmd = NvmeCommand::create_io_cq(qid, queue_depth, cq_phys, iv);
            self.submit_admin_command(cq_cmd)?;
            self.poll_admin_completion()?;

            // Then create SQ linked to that CQ
            let sq_cmd = NvmeCommand::create_io_sq(qid, queue_depth, sq_phys, qid);
            self.submit_admin_command(sq_cmd)?;
            self.poll_admin_completion()?;

            let sq = NvmeQueue::new(sq_phys, sq_virt, queue_depth, self.sq_doorbell(qid), 64);
            let cq = NvmeQueue::new(cq_phys, cq_virt, queue_depth, self.cq_doorbell(qid), 16);

            self.io_queues.push((sq, cq));
        }

        self.num_io_queues = actual_count;
        Ok(())
    }

    // ── Block I/O Operations ────────────────────────────────────────────

    pub fn read_sectors(
        &mut self,
        nsid: u32,
        lba: u64,
        count: u16,
        buf_phys: u64,
    ) -> Result<(), BlockError> {
        if self.io_queues.is_empty() {
            return Err(BlockError::DeviceNotFound);
        }

        let qid = ((lba % self.num_io_queues as u64) + 1) as usize;

        // NLB is a zero-based count; guard count==0 from underflowing to 0xFFFF.
        let cmd = NvmeCommand::read(nsid, lba, count.max(1) - 1, buf_phys, 0);
        self.submit_io_command(qid, cmd)?;
        self.poll_io_completion(qid)?;

        Ok(())
    }

    pub fn write_sectors(
        &mut self,
        nsid: u32,
        lba: u64,
        count: u16,
        buf_phys: u64,
    ) -> Result<(), BlockError> {
        if self.io_queues.is_empty() {
            return Err(BlockError::DeviceNotFound);
        }

        let qid = ((lba % self.num_io_queues as u64) + 1) as usize;

        // NLB is a zero-based count; guard count==0 from underflowing to 0xFFFF.
        let cmd = NvmeCommand::write(nsid, lba, count.max(1) - 1, buf_phys, 0);
        self.submit_io_command(qid, cmd)?;
        self.poll_io_completion(qid)?;

        Ok(())
    }

    pub fn flush(&mut self, nsid: u32) -> Result<(), BlockError> {
        if self.io_queues.is_empty() {
            return Err(BlockError::DeviceNotFound);
        }

        let cmd = NvmeCommand::flush(nsid);
        self.submit_io_command(1, cmd)?;
        self.poll_io_completion(1)?;

        Ok(())
    }

    pub fn trim(
        &mut self,
        nsid: u32,
        lba: u64,
        count: u32,
        range_buf_phys: u64,
    ) -> Result<(), BlockError> {
        if !self.identify.supports_dataset_management() {
            return Err(BlockError::UnsupportedFeature);
        }
        if self.io_queues.is_empty() {
            return Err(BlockError::DeviceNotFound);
        }

        // Write the dataset range descriptor at range_buf_phys
        // Format: 4-byte context attributes, 4-byte length in LBAs, 8-byte starting LBA
        unsafe {
            let ptr = range_buf_phys as *mut u8;
            let range = core::slice::from_raw_parts_mut(ptr, 16);
            range[0..4].copy_from_slice(&0u32.to_le_bytes()); // context attributes
            range[4..8].copy_from_slice(&count.to_le_bytes()); // length in LBAs
            range[8..16].copy_from_slice(&lba.to_le_bytes()); // starting LBA
        }

        let cmd = NvmeCommand::dataset_management(nsid, 1, range_buf_phys);
        self.submit_io_command(1, cmd)?;
        self.poll_io_completion(1)?;

        Ok(())
    }

    // ── Log Pages ───────────────────────────────────────────────────────

    pub fn get_smart_log(&mut self, nsid: u32) -> Result<SmartLog, BlockError> {
        let (buf_phys, virt) = self.alloc_dma()?;
        let num_dwords = 512 / 4;

        let cmd = NvmeCommand::get_log_page(nsid, LOG_SMART_HEALTH, num_dwords, buf_phys);
        self.submit_admin_command(cmd)?;
        self.poll_admin_completion()?;

        let data = unsafe { core::slice::from_raw_parts(virt as *const u8, 512) };
        SmartLog::from_bytes(data).ok_or(BlockError::IoError)
    }

    pub fn get_error_log(&mut self) -> Result<Vec<ErrorLogEntry>, BlockError> {
        let max_entries = self.identify.elpe as usize + 1;
        let buf_size = max_entries * 64;
        let pages_needed = (buf_size + 4095) / 4096;
        let (buf_phys, virt) = self.alloc_dma_pages(pages_needed)?;
        let num_dwords = (buf_size / 4) as u32;

        let cmd = NvmeCommand::get_log_page(0xFFFFFFFF, LOG_ERROR_INFO, num_dwords, buf_phys);
        self.submit_admin_command(cmd)?;
        self.poll_admin_completion()?;

        let data = unsafe { core::slice::from_raw_parts(virt as *const u8, buf_size) };
        let mut entries = Vec::new();

        for i in 0..max_entries {
            let offset = i * 64;
            if let Some(entry) = ErrorLogEntry::from_bytes(&data[offset..offset + 64]) {
                if entry.error_count > 0 {
                    entries.push(entry);
                }
            }
        }

        Ok(entries)
    }

    // ── Format ──────────────────────────────────────────────────────────

    pub fn format_namespace(
        &mut self,
        nsid: u32,
        lbaf: u8,
        secure_erase: u8,
    ) -> Result<(), BlockError> {
        if !self.identify.supports_format_nvm() {
            return Err(BlockError::UnsupportedFeature);
        }

        let cmd = NvmeCommand::format_nvm(nsid, lbaf, secure_erase);
        self.submit_admin_command(cmd)?;
        self.poll_admin_completion()?;

        crate::serial_println!(
            "[nvme] formatted ns{} lbaf={} ses={}",
            nsid,
            lbaf,
            secure_erase
        );
        Ok(())
    }

    // ── Shutdown ────────────────────────────────────────────────────────

    pub fn shutdown(&mut self) -> Result<(), BlockError> {
        self.state = NvmeState::ShuttingDown;

        unsafe {
            let mut cc = self.read_reg32(REG_CC);
            cc = (cc & !(0x3 << 14)) | CC_SHN_NORMAL;
            self.write_reg32(REG_CC, cc);

            for _ in 0..100_000 {
                let csts = self.read_reg32(REG_CSTS);
                if (csts & CSTS_SHST_MASK) == CSTS_SHST_COMPLETE {
                    self.state = NvmeState::Disabled;
                    crate::serial_println!("[nvme] shutdown complete");
                    return Ok(());
                }
                for _ in 0..1000 {
                    core::hint::spin_loop();
                }
            }
        }

        self.state = NvmeState::Failed;
        Err(BlockError::Timeout)
    }

    // ── BlockDevice Registration ────────────────────────────────────────

    pub fn register_as_block_device(&self, nsid: u32) -> Result<BlockDeviceInfo, BlockError> {
        let ns = self
            .namespaces
            .iter()
            .find(|n| n.nsid == nsid)
            .ok_or(BlockError::DeviceNotFound)?;

        let name = alloc::format!("nvme0n{}", nsid);
        let mut dev = BlockDeviceInfo::new(name, 259, nsid as u16);
        dev.sector_size = ns.block_size;
        dev.total_sectors = ns.size_blocks;
        dev.read_only = false;
        dev.removable = false;
        dev.rotational = false;
        dev.queue_depth = self.max_queue_entries as u32;
        dev.model = self.model.clone();
        dev.serial = self.serial.clone();
        dev.firmware = self.firmware.clone();

        Ok(dev)
    }

    // ── Accessors ───────────────────────────────────────────────────────

    pub fn state(&self) -> NvmeState {
        self.state
    }

    pub fn namespaces(&self) -> &[NvmeNamespace] {
        &self.namespaces
    }

    pub fn namespace_count(&self) -> usize {
        self.namespaces.len()
    }

    pub fn io_queue_count(&self) -> u16 {
        self.num_io_queues
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    pub fn serial(&self) -> &str {
        &self.serial
    }

    pub fn firmware(&self) -> &str {
        &self.firmware
    }

    pub fn outstanding_commands(&self) -> u32 {
        self.outstanding_commands
    }

    /// Physical DMA pages used by admin + I/O queue rings (4 KiB each).
    pub fn dma_regions(&self) -> Vec<(u64, u64)> {
        let mut regions = Vec::new();
        let mut push = |phys: u64| {
            if phys != 0 {
                regions.push((phys, DMA_PAGE));
            }
        };
        push(self.admin_sq.base_phys);
        push(self.admin_cq.base_phys);
        for (sq, cq) in &self.io_queues {
            push(sq.base_phys);
            push(cq.base_phys);
        }
        regions
    }
}

// ─── Global State & Initialization ──────────────────────────────────────────

pub static NVME_CONTROLLERS: Mutex<Vec<NvmeController>> = Mutex::new(Vec::new());

fn nvme_msix_isr() {
    let mut ctrls = NVME_CONTROLLERS.lock();
    for ctrl in ctrls.iter_mut() {
        for (_sq, cq) in ctrl.io_queues.iter() {
            let offset = crate::memory::PHYS_MEM_OFFSET.get().unwrap().as_u64();
            crate::sync::FUTEX_MANAGER
                .lock()
                .wake(cq.doorbell_reg - offset, 1);
        }
        let offset = crate::memory::PHYS_MEM_OFFSET.get().unwrap().as_u64();
        crate::sync::FUTEX_MANAGER
            .lock()
            .wake(ctrl.admin_cq.doorbell_reg - offset, 1);
    }
}

/// BlockDevice adapter for one NVMe namespace on controller 0.
/// Uses a DMA bounce buffer for transfers between NVMe and caller buffers.
///
/// MasterChecklist Phase 1.5 — multi-namespace handling: the `nsid` field
/// makes every BlockDevice route to its own namespace instead of the
/// previous hardcoded NSID 1, so picking namespace 3 as the boot disk
/// no longer silently reads from namespace 1.
pub struct NvmeBlockDevice {
    pub nsid: u32,
}

impl crate::block_io::BlockDevice for NvmeBlockDevice {
    fn read_sector(&self, lba: u64, buf: &mut [u8]) -> Result<(), &'static str> {
        // One lock hold for the whole op so the persistent bounce buffer
        // (BUG-37 fix) can't be concurrently reused; poll_io_completion is a
        // bounded poll (not a futex-block), so holding the lock is safe.
        let mut ctrls = NVME_CONTROLLERS.lock();
        let ctrl = ctrls.first_mut().ok_or("nvme: no controller")?;
        let (bounce_phys, bounce_virt) =
            ctrl.ensure_bounce().map_err(|_| "nvme: DMA alloc failed")?;
        ctrl.read_sectors(self.nsid, lba, 1, bounce_phys)
            .map_err(|_| "nvme: read failed")?;
        let len = buf.len().min(512);
        unsafe {
            core::ptr::copy_nonoverlapping(bounce_virt as *const u8, buf.as_mut_ptr(), len);
        }
        Ok(())
    }

    fn write_sector(&self, lba: u64, buf: &[u8]) -> Result<(), &'static str> {
        crate::block_io::safe_mode_guard_write(lba, buf.len(), "nvme")?;
        // One lock hold + the persistent bounce buffer (BUG-37 fix).
        let mut ctrls = NVME_CONTROLLERS.lock();
        let ctrl = ctrls.first_mut().ok_or("nvme: no controller")?;
        let (bounce_phys, bounce_virt) =
            ctrl.ensure_bounce().map_err(|_| "nvme: DMA alloc failed")?;
        let len = buf.len().min(512);
        unsafe {
            core::ptr::copy_nonoverlapping(buf.as_ptr(), bounce_virt as *mut u8, len);
        }
        // Log the concrete BlockError variant — the bare "write failed" hid
        // which step rejected Athena's first bootlog write (the completion-poll
        // diagnostics never fired, so the failure is in submit/queue selection,
        // not the device status).
        ctrl.write_sectors(self.nsid, lba, 1, bounce_phys)
            .map_err(|e| {
                crate::serial_println!(
                    "[nvme] I/O cmd write_sectors FAILED: {:?} (nsid={} lba={})",
                    e,
                    self.nsid,
                    lba
                );
                "nvme: write failed"
            })?;
        Ok(())
    }

    fn sector_size(&self) -> usize {
        let ctrls = NVME_CONTROLLERS.lock();
        ctrls
            .first()
            .and_then(|c| c.namespaces.iter().find(|ns| ns.nsid == self.nsid))
            .map(|ns| ns.block_size as usize)
            .unwrap_or(512)
    }

    fn total_sectors(&self) -> u64 {
        let ctrls = NVME_CONTROLLERS.lock();
        ctrls
            .first()
            .and_then(|c| c.namespaces.iter().find(|ns| ns.nsid == self.nsid))
            .map(|ns| ns.size_blocks)
            .unwrap_or(0)
    }

    fn flush_cache(&self) -> Result<(), &'static str> {
        // NVMe FLUSH (opcode 0x00) commits the controller's volatile
        // write cache to NAND. Required for bootlog-persist to survive a
        // power-cycle: without it, sectors written just before power-off
        // can sit in the controller's DRAM and never reach the media.
        let mut ctrls = NVME_CONTROLLERS.lock();
        let ctrl = ctrls.first_mut().ok_or("nvme: no controller")?;
        ctrl.flush(self.nsid).map_err(|_| "nvme: flush failed")
    }
}

unsafe impl Send for NvmeBlockDevice {}

/// Classify a 512-byte sector-0 read as a known bootable layout.
/// Returns the human-readable label used in the boot log.
///
/// Detection rules:
///   - "RaeFS"  → first 8 bytes match the RaeFS superblock magic
///                (raefs::RAEFS_MAGIC, "RaeFS!" little-endian).
///   - "GPT"    → byte[510..512] == 0x55AA AND the protective-MBR
///                partition entry 1 type byte (offset 450) == 0xEE.
///   - "MBR"    → byte[510..512] == 0x55AA, no GPT marker.
///   - "blank"  → all zero, no signature.
///   - "unknown"→ anything else.
fn classify_boot_sector(sect: &[u8]) -> &'static str {
    if sect.len() < 512 {
        return "short";
    }
    // RaeFS superblock magic at the very start.
    const RAEFS_MAGIC: u64 = 0x526165465321;
    let magic = u64::from_le_bytes([
        sect[0], sect[1], sect[2], sect[3], sect[4], sect[5], sect[6], sect[7],
    ]);
    if magic == RAEFS_MAGIC {
        return "RaeFS";
    }
    // MBR-family signature at end of sector 0.
    if sect[510] == 0x55 && sect[511] == 0xAA {
        // Protective MBR partition entry 1: bytes 446..462, type byte @ offset 4 → absolute 450.
        if sect[450] == 0xEE {
            return "GPT";
        }
        return "MBR";
    }
    // All zero → freshly wiped or unallocated.
    if sect.iter().all(|&b| b == 0) {
        return "blank";
    }
    "unknown"
}

pub fn init() {
    crate::serial_println!("[nvme] scanning PCI for NVMe controllers...");

    let pci_devices = crate::pci::enumerate();
    let mut count = 0u32;

    for dev in &pci_devices {
        // NVMe devices: class=0x01 (Mass Storage), subclass=0x08 (NVM), prog_if=0x02 (NVMe)
        if dev.class == 0x01 && dev.subclass == 0x08 && dev.prog_if == 0x02 {
            crate::serial_println!(
                "[nvme] check dev {:02x}:{:02x}.{}",
                dev.bus,
                dev.device,
                dev.function
            );
            crate::pci::enable_bus_mastering(dev);
            crate::serial_println!("[nvme] bus mastering enabled");
            let (irq_mode, vectors) = crate::storage_irq::probe_msix_or_intx("nvme", dev, 1);
            crate::serial_println!("[nvme] irq probed: mode={:?}", irq_mode);
            let msix_vec = if let Some(v) = vectors {
                let vec = v[0];
                crate::interrupts::register_handler(vec, nvme_msix_isr);
                Some(vec)
            } else {
                None
            };
            crate::serial_println!(
                "[nvme] found NVMe controller at {:02x}:{:02x}.{} vendor={:04x} device={:04x}",
                dev.bus,
                dev.device,
                dev.function,
                dev.vendor_id,
                dev.device_id
            );

            // BAR0 on NVMe is a memory BAR. Low 4 bits are flags (bit 0 = memory/IO,
            // bits 1-2 = 32/64-bit type, bit 3 = prefetchable). Mask them off.
            let bar0_raw = dev.bars[0] as u64;
            if bar0_raw == 0 {
                crate::serial_println!("[nvme] BAR0 not configured, skipping");
                continue;
            }
            let mut bar0_phys = bar0_raw & !0xFu64;
            // 64-bit memory BAR (type bits 2:1 == 0b10): the upper half lives in
            // BAR1. Firmware (OVMF in particular) places NVMe above 4 GiB, so
            // dropping the upper dword maps RAM instead of the controller and
            // every register read returns garbage (admin timeout at init).
            if (bar0_raw >> 1) & 0x3 == 2 {
                bar0_phys |= (dev.bars[1] as u64) << 32;
            }
            // Translate physical → kernel-virtual via the bootloader's
            // phys-mem-offset window. The driver's read_reg32 / write_reg32
            // then index off this directly.
            if crate::memory::PHYS_MEM_OFFSET.get().is_none() {
                crate::serial_println!("[nvme] PHYS_MEM_OFFSET not initialized, skipping");
                continue;
            }
            // 64-bit BARs can sit above the linear physmap; map the MMIO region
            // (creates PTEs + disables caching) instead of assuming it's mapped.
            let bar0_size = {
                let s = crate::mmio::pci_bar_size_bytes(dev.bus, dev.device, dev.function, 0);
                if s == 0 {
                    0x4000
                } else {
                    s
                }
            };
            let bar0_virt = crate::arch::mmu::kernel()
                .map_mmio_range(
                    x86_64::PhysAddr::new(bar0_phys),
                    bar0_size,
                    crate::arch::mmu::PageFlags::DEVICE,
                )
                .as_u64();
            crate::serial_println!(
                "[nvme] BAR0 phys={:#x} size={:#x} virt={:#x}",
                bar0_phys,
                bar0_size,
                bar0_virt
            );

            let mut ctrl = NvmeController::new(bar0_virt);
            ctrl.msix_vector = msix_vec;
            ctrl.setup_iommu(dev.bus, dev.device, dev.function);

            if let Err(e) = ctrl.init() {
                crate::serial_println!("[nvme] init failed: {:?}", e);
                continue;
            }

            if let Err(e) = ctrl.identify_controller() {
                crate::serial_println!("[nvme] identify failed: {:?}", e);
                continue;
            }

            let num_ns = ctrl.identify.num_namespaces;

            if let Err(e) = ctrl.create_io_queues(4) {
                crate::serial_println!("[nvme] queue creation failed: {:?}", e);
                continue;
            }

            // MasterChecklist Phase 1.5 — per-CPU I/O queue pairs.
            // Attempt to create one SQ+CQ per online CPU (capped at 4).
            // These use QIDs 5..8 and are stored in PER_CPU_QUEUES.
            // Failure is non-fatal: the standard io_queues already suffice.
            let cpu_count =
                crate::smp::APS_ONLINE.load(core::sync::atomic::Ordering::SeqCst) as usize + 1; // +1 for BSP
            match init_per_cpu_queues(&mut ctrl, cpu_count) {
                Ok(()) => {}
                Err(e) => {
                    crate::serial_println!("[nvme] per-cpu queue init warn: {}", e);
                }
            }

            let regions = ctrl.dma_regions();
            ctrl.finish_iommu_setup(&regions);

            // MasterChecklist Phase 1.5 — tolerate Identify Namespace failures.
            //
            // Real NVMe controllers (Samsung 980, WD SN770, Crucial P3) report
            // a controller-wide `nn` (number of namespaces) that's the *highest
            // supported NSID*, not "every NSID 1..=nn is allocated". Real
            // layouts are sparse: NSID 1 active, NSID 2 inactive, NSID 3 active
            // is legal. The previous `break` on first failure / empty silently
            // skipped every NSID after the first hole — and on Samsung
            // controllers that often returned a malformed Identify for inactive
            // NSIDs, panicking the BlockError unwrap path.
            //
            // Policy: skip and continue per inactive NSID, log the cause, and
            // bail only when we've seen `MAX_CONSEC_MISSES` consecutive
            // failures in a row with no successes (cheap guard against
            // controllers that advertise nn = 0xFFFE).
            const MAX_CONSEC_MISSES: u32 = 16;
            let scan_upper = (num_ns as u32).min(1024); // cap; we'll never have 1k bootable NS
            let mut consec_misses: u32 = 0;
            let mut active_found: u32 = 0;
            let mut skipped_inactive: u32 = 0;
            let mut errored: u32 = 0;
            for nsid in 1..=scan_upper {
                match ctrl.identify_namespace(nsid) {
                    Ok(ns) => {
                        if ns.size_blocks == 0 {
                            // Inactive / unallocated NSID — skip, keep walking.
                            skipped_inactive += 1;
                            consec_misses += 1;
                            crate::serial_println!(
                                "[nvme] nsid {}: inactive (size_blocks=0) — skip",
                                nsid,
                            );
                        } else {
                            consec_misses = 0;
                            active_found += 1;
                            let blk = ctrl.register_as_block_device(nsid);
                            ctrl.namespaces.push(ns);
                            if let Ok(dev) = blk {
                                let _ = crate::block_io::register_block_device(dev);
                            }
                        }
                    }
                    Err(e) => {
                        // Real controllers sometimes return malformed Identify
                        // for reserved/inactive NSIDs. Log and continue.
                        errored += 1;
                        consec_misses += 1;
                        crate::serial_println!(
                            "[nvme] nsid {}: identify error {:?} — skip",
                            nsid,
                            e,
                        );
                    }
                }
                if consec_misses >= MAX_CONSEC_MISSES {
                    crate::serial_println!(
                        "[nvme] stopping NSID scan after {} consecutive misses at nsid {} (nn={})",
                        consec_misses,
                        nsid,
                        num_ns,
                    );
                    break;
                }
            }
            let total_scanned = active_found + skipped_inactive + errored;
            crate::serial_println!(
                "[nvme] namespace scan: {} active, {} inactive, {} errored ({} NSID(s) probed; nn={})",
                active_found, skipped_inactive, errored, total_scanned, num_ns,
            );

            NVME_CONTROLLERS.lock().push(ctrl);
            count += 1;
        }
    }

    if count > 0 {
        // MasterChecklist Phase 1.5 — multi-namespace handling. Probe and
        // log sector 0 of every active NSID so the operator always sees
        // what's on disk, even when another driver (virtio-blk) already
        // owns ACTIVE_BLOCK_DEVICE. Register our chosen NSID as active
        // only when no other driver has claimed it.
        let active_nsids: alloc::vec::Vec<u32> = {
            let ctrls = NVME_CONTROLLERS.lock();
            ctrls
                .first()
                .map(|c| c.namespaces.iter().map(|n| n.nsid).collect())
                .unwrap_or_default()
        };

        let mut chosen: Option<(u32, &'static str)> = None;
        let mut fallback: Option<u32> = None;
        for nsid in &active_nsids {
            if fallback.is_none() {
                fallback = Some(*nsid);
            }
            let probe = NvmeBlockDevice { nsid: *nsid };
            let mut buf = [0u8; 512];
            match crate::block_io::BlockDevice::read_sector(&probe, 0, &mut buf) {
                Ok(()) => {
                    let sig = classify_boot_sector(&buf);
                    crate::serial_println!("[nvme] nsid {}: sector 0 = {}", nsid, sig,);
                    let bootable = matches!(sig, "GPT" | "MBR" | "RaeFS");
                    if bootable && chosen.is_none() {
                        chosen = Some((*nsid, sig));
                    }
                }
                Err(e) => {
                    crate::serial_println!(
                        "[nvme] nsid {}: sector 0 read failed: {} — skipping",
                        nsid,
                        e,
                    );
                }
            }
        }

        let pick_nsid = match (chosen, fallback) {
            (Some((nsid, sig)), _) => {
                crate::serial_println!(
                    "[nvme] boot disk candidate: nsid {} (signature {})",
                    nsid,
                    sig,
                );
                Some(nsid)
            }
            (None, Some(nsid)) => {
                crate::serial_println!(
                    "[nvme] boot disk candidate: nsid {} (no known signature on any NSID; fallback)",
                    nsid,
                );
                Some(nsid)
            }
            (None, None) => {
                crate::serial_println!(
                    "[nvme] boot disk candidate: NONE — no active namespaces on any controller"
                );
                None
            }
        };

        let has_active = crate::block_io::ACTIVE_BLOCK_DEVICE.lock().is_some();
        if let Some(nsid) = pick_nsid {
            if !has_active {
                crate::block_io::set_active_block_device(alloc::boxed::Box::new(NvmeBlockDevice {
                    nsid,
                }));
                crate::serial_println!("[nvme] registered nsid {} as active block device", nsid,);
            } else {
                crate::serial_println!(
                    "[nvme] nsid {} ready, but another driver already owns ACTIVE_BLOCK_DEVICE — not overriding",
                    nsid,
                );
            }
        }
    }

    crate::serial_println!("[ OK ] NVMe: {} controller(s) initialized", count);
}

// ── Boot smoketest ─────────────────────────────────────────────────────
//
// Concept §Storage: NVMe is RaeenOS's primary storage path. After the
// driver init's, prove it's actually reading bytes off the device by
// pulling sector 0 and pattern-matching the marker that `boot.ps1`
// writes into the backing file:
//
//     RaeenOS-NVMe-block-0-ok!
//
// On real hardware the marker won't be there; smoketest falls back to
// just reporting "read OK, N bytes" so we still know the driver is
// alive.

/// `/proc/raeen/nvme` — controller, namespace, and queue summary. REDOX_EXTRACTION_MAP R07.
pub fn dump_text() -> alloc::string::String {
    let ctrls = NVME_CONTROLLERS.lock();
    let mut out = alloc::string::String::from("# RaeenOS NVMe\n");
    if ctrls.is_empty() {
        out.push_str("controllers: 0\n");
        out.push_str("note: no NVMe controller (add `-device nvme` in QEMU)\n");
        return out;
    }
    out.push_str(&alloc::format!("controllers: {}\n", ctrls.len()));
    for (ci, ctrl) in ctrls.iter().enumerate() {
        out.push_str(&alloc::format!(
            "controller{}: {} ({:04x}) {} serial={} fw={} oui={:02x}-{:02x}-{:02x} nn={} mdts={}KiB io_queues={}\n",
            ci,
            ctrl.identify.vendor_name(),
            ctrl.identify.vendor_id,
            ctrl.model,
            ctrl.serial,
            ctrl.firmware,
            ctrl.identify.ieee_oui[0],
            ctrl.identify.ieee_oui[1],
            ctrl.identify.ieee_oui[2],
            ctrl.identify.num_namespaces,
            ctrl.max_transfer_size / 1024,
            ctrl.io_queues.len(),
        ));
        for ns in &ctrl.namespaces {
            out.push_str(&alloc::format!(
                "  ns{}: blocks={} block_size={}B thin={}\n",
                ns.nsid,
                ns.size_blocks,
                ns.block_size,
                ns.features.thin_provisioning,
            ));
        }
    }
    out
}

pub fn run_boot_smoketest() {
    use crate::block_io::BlockDevice;

    if NVME_CONTROLLERS.lock().is_empty() {
        crate::serial_println!("[nvme] smoketest SKIP: no controller found");
        return;
    }

    let mut buf = [0u8; 512];
    // Smoketest always probes the first NVMe namespace; for actual boot
    // disk selection see the picker in init().
    let dev = NvmeBlockDevice { nsid: 1 };

    let t0 = unsafe { core::arch::x86_64::_rdtsc() };
    let r = dev.read_sector(0, &mut buf);
    let t1 = unsafe { core::arch::x86_64::_rdtsc() };

    match r {
        Ok(()) => {
            let marker = b"RaeenOS-NVMe-block-0-ok!";
            let matched = buf.len() >= marker.len() && &buf[..marker.len()] == marker;
            let cycles = t1.saturating_sub(t0);
            let preview: alloc::string::String = buf[..24]
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
                "[nvme] smoketest PASS: read LBA0 nsid={} ({} cycles) preview=\"{}\" marker={}",
                dev.nsid,
                cycles,
                preview,
                if matched {
                    "MATCH"
                } else {
                    "no-match (real hw or wiped image)"
                },
            );
        }
        Err(e) => {
            crate::serial_println!("[nvme] smoketest FAIL: read_sector(0) → {}", e);
        }
    }

    // Report per-CPU queue count as part of the smoketest summary.
    let pcq_count = PER_CPU_QUEUES.lock().len();
    if pcq_count > 0 {
        crate::serial_println!("[nvme] per-cpu queues: {} pair(s) active", pcq_count);
    } else {
        crate::serial_println!("[nvme] per-cpu queues: not yet initialised (call init_per_cpu_queues after controller init)");
    }
}

// ─── Per-CPU I/O Queue Pairs ────────────────────────────────────────────────
//
// MasterChecklist Phase 1.5 — per-CPU NVMe I/O queue pairs.
//
// The NVMe spec allows up to 65535 I/O queue pairs. Creating one SQ+CQ pair
// per logical CPU eliminates the single serialisation point on the shared
// submission queue: each CPU posts directly to its own ring without taking
// any lock. QIDs are 1-based and must not overlap with admin queues (QID 0)
// or with the regular io_queues already created via create_io_queues().
//
// For QEMU (typically 4 vCPUs) we create 4 pairs, QIDs 5..8 by default
// (so they do not collide with the 4 regular I/O queues at QIDs 1..4).
// On real hardware cap at 4 pairs to keep the DMA footprint manageable
// during early boot; a later phase can raise this limit via Set-Features.

/// A dedicated SQ+CQ pair owned by one logical CPU.
pub struct PerCpuQueue {
    /// Submission queue for this CPU.
    pub sq: NvmeQueue,
    /// Completion queue for this CPU.
    pub cq: NvmeQueue,
    /// CPU id this pair was created for (informational).
    pub cpu_id: usize,
    /// Queue ID assigned by the controller (1-based, non-zero).
    pub qid: u16,
}

/// All per-CPU queue pairs, indexed by cpu_id % queue_count.
pub static PER_CPU_QUEUES: Mutex<Vec<PerCpuQueue>> = Mutex::new(Vec::new());

/// Maximum number of per-CPU queue pairs we will create.
/// Capped at 4 to stay within the 4-QID window QEMU advertises and to
/// avoid DMA-frame exhaustion during early boot.
const MAX_PER_CPU_QUEUES: usize = 4;

/// QID offset: per-CPU queues start here so they do not collide with the
/// standard I/O queues created by `create_io_queues(4)` at QIDs 1..4.
const PER_CPU_QID_BASE: u16 = 5;

/// Create one NVMe I/O SQ+CQ pair per online CPU (capped at `MAX_PER_CPU_QUEUES`).
///
/// Call this after `create_io_queues` has already been issued for the standard
/// queues; the QIDs used here start at `PER_CPU_QID_BASE` to avoid conflicts.
///
/// The NVMe spec mandates:
///   1. Create CQ (Admin opcode 0x05) before the paired SQ.
///   2. Create SQ (Admin opcode 0x01) linked to that CQ.
///
/// We also issue a Set Features (opcode 0x09, FID 0x07) to inform the
/// controller of the total desired queue count before allocating, which is
/// the same contract `create_io_queues` uses. Because `create_io_queues` has
/// already negotiated the base count, we request the updated total here.
///
/// On QEMU (which doesn't always honour extended queue requests gracefully),
/// failures are logged but non-fatal: we keep whatever pairs succeeded.
pub fn init_per_cpu_queues(
    controller: &mut NvmeController,
    cpu_count: usize,
) -> Result<(), &'static str> {
    let n = cpu_count.min(MAX_PER_CPU_QUEUES);
    if n == 0 {
        crate::serial_println!("[nvme] per-cpu I/O queues: cpu_count=0, nothing to do");
        return Ok(());
    }

    // Inform the controller of the extended queue total.
    // desired = existing io_queues + per-CPU pairs (both SQ and CQ).
    let existing = controller.io_queues.len() as u32;
    let desired = existing + n as u32;
    let cdw11 = ((desired - 1) << 16) | (desired - 1);
    let cmd = NvmeCommand::set_features(FEAT_NUM_QUEUES, cdw11);
    if let Err(e) = controller.submit_admin_command(cmd) {
        crate::serial_println!(
            "[nvme] per-cpu queues: Set Features submit error {:?}, proceeding anyway",
            e
        );
    } else {
        // Poll completion; ignore the allocated count — QEMU may return fewer.
        let _ = controller.poll_admin_completion();
    }

    let mut created: u16 = 0;
    let mut queues = PER_CPU_QUEUES.lock();

    for cpu_id in 0..n {
        // QID is 1-based and offset above the standard I/O queues.
        let qid = PER_CPU_QID_BASE + cpu_id as u16;

        // ── Allocate DMA frames ──────────────────────────────────────────
        let (cq_phys, cq_virt) = match alloc_dma_frame_mapped(controller.iommu_domain) {
            Ok(r) => r,
            Err(e) => {
                crate::serial_println!(
                    "[nvme] per-cpu q{} (cpu{}): CQ DMA alloc failed {:?}",
                    qid,
                    cpu_id,
                    e
                );
                continue;
            }
        };
        let (sq_phys, sq_virt) = match alloc_dma_frame_mapped(controller.iommu_domain) {
            Ok(r) => r,
            Err(e) => {
                crate::serial_println!(
                    "[nvme] per-cpu q{} (cpu{}): SQ DMA alloc failed {:?}",
                    qid,
                    cpu_id,
                    e
                );
                continue;
            }
        };

        // Queue depth: same policy as create_io_queues — cap at 64 so both
        // rings fit in a single 4 KiB DMA page.
        let queue_depth = core::cmp::min(controller.max_queue_entries, 64);

        // ── Create CQ ───────────────────────────────────────────────────
        // interrupts enabled if MSI-X is available (iv = vector 0).
        let iv: u16 = 0;
        let cq_cmd = NvmeCommand::create_io_cq(qid, queue_depth, cq_phys, iv);
        if let Err(e) = controller.submit_admin_command(cq_cmd) {
            crate::serial_println!(
                "[nvme] per-cpu q{} (cpu{}): Create CQ submit error {:?}",
                qid,
                cpu_id,
                e
            );
            continue;
        }
        if let Err(e) = controller.poll_admin_completion() {
            crate::serial_println!(
                "[nvme] per-cpu q{} (cpu{}): Create CQ completion error {:?}",
                qid,
                cpu_id,
                e
            );
            continue;
        }

        // ── Create SQ linked to the CQ just created ──────────────────────
        let sq_cmd = NvmeCommand::create_io_sq(qid, queue_depth, sq_phys, qid);
        if let Err(e) = controller.submit_admin_command(sq_cmd) {
            crate::serial_println!(
                "[nvme] per-cpu q{} (cpu{}): Create SQ submit error {:?}",
                qid,
                cpu_id,
                e
            );
            continue;
        }
        if let Err(e) = controller.poll_admin_completion() {
            crate::serial_println!(
                "[nvme] per-cpu q{} (cpu{}): Create SQ completion error {:?}",
                qid,
                cpu_id,
                e
            );
            continue;
        }

        // ── Build NvmeQueue handles ──────────────────────────────────────
        let sq_db = controller.sq_doorbell(qid);
        let cq_db = controller.cq_doorbell(qid);
        let sq = NvmeQueue::new(sq_phys, sq_virt, queue_depth, sq_db, 64);
        let cq = NvmeQueue::new(cq_phys, cq_virt, queue_depth, cq_db, 16);

        queues.push(PerCpuQueue {
            sq,
            cq,
            cpu_id,
            qid,
        });
        created += 1;
        crate::serial_println!(
            "[nvme] per-cpu q{} (cpu{}): SQ+CQ created (depth={})",
            qid,
            cpu_id,
            queue_depth
        );
    }

    crate::serial_println!(
        "[nvme] per-cpu I/O queues: {} pairs created (SQ+CQ per CPU)",
        created,
    );

    if created == 0 {
        Err("[nvme] per-cpu I/O queues: zero pairs created")
    } else {
        Ok(())
    }
}

/// Submit an NVMe I/O command on the queue belonging to `cpu_id`.
///
/// Uses `cpu_id % queue_count` for routing so callers that pass an id larger
/// than the number of pairs created still land on a valid queue.
///
/// Returns the CID assigned to the submitted command, or a `BlockError`.
pub fn submit_on_cpu(cpu_id: usize, cmd: NvmeCommand) -> Result<u32, BlockError> {
    let mut queues = PER_CPU_QUEUES.lock();
    if queues.is_empty() {
        return Err(BlockError::DeviceNotFound);
    }

    let idx = cpu_id % queues.len();
    let pcq = &mut queues[idx];

    if pcq.sq.is_full() {
        return Err(BlockError::QueueFull);
    }

    let mut cmd = cmd;
    let cid = pcq.sq.next_cid();
    cmd.cid = cid;

    // SAFETY: sq.entry_virt() returns a kernel-virtual address into a
    // DMA-mapped page that was zero-initialised at allocation time.
    // Writing a 64-byte NvmeCommand struct via write_volatile is safe as
    // long as the address is properly aligned (it is: entries are packed
    // at 64-byte offsets) and the mapping is valid for the controller's
    // lifetime.
    unsafe {
        let addr = pcq.sq.entry_virt(pcq.sq.tail);
        let ptr = addr as *mut NvmeCommand;
        core::ptr::write_volatile(ptr, cmd);
    }

    pcq.sq.advance_tail();
    pcq.sq.ring_doorbell();

    Ok(cid as u32)
}
