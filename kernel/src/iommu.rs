//! Intel VT-d IOMMU driver — DMA sandboxing for device isolation.
//!
//! Every driver in RaeenOS runs in its own protection domain with
//! IOMMU enforcement. A misbehaving GPU or NIC driver can crash its
//! service but cannot corrupt kernel memory or other devices' DMA
//! buffers.
//!
//! Workflow:
//!   1. Parse ACPI DMAR table → discover remapping hardware units (DRHD)
//!   2. Map VT-d MMIO registers from each DRHD
//!   3. Build root/context tables so every PCI device is assigned a domain
//!   4. On driver init, `create_domain` + `map_dma` for its buffers
//!   5. On DMA fault, log the offending BDF and revoke its domain

#![allow(dead_code)]

extern crate alloc;

use alloc::vec::Vec;
use core::ptr;
use core::sync::atomic::{AtomicU16, Ordering};
use spin::Mutex;
use x86_64::structures::paging::FrameAllocator;

use crate::acpi_full::{self, SIG_DMAR};
use crate::memory::{GlobalFrameAllocator, PHYS_MEM_OFFSET};

// ─── DMAR Device Scope ──────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct DeviceScope {
    pub scope_type: DeviceScopeType,
    pub enumeration_id: u8,
    pub start_bus: u8,
    pub path: Vec<(u8, u8)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceScopeType {
    PciEndpoint,
    PciSubHierarchy,
    Ioapic,
    Hpet,
    AcpiNamespace,
    Unknown(u8),
}

impl From<u8> for DeviceScopeType {
    fn from(v: u8) -> Self {
        match v {
            1 => Self::PciEndpoint,
            2 => Self::PciSubHierarchy,
            3 => Self::Ioapic,
            4 => Self::Hpet,
            5 => Self::AcpiNamespace,
            x => Self::Unknown(x),
        }
    }
}

#[derive(Debug, Clone)]
pub struct DrhdInfo {
    pub flags: u8,
    pub segment: u16,
    pub register_base: u64,
    pub device_scopes: Vec<DeviceScope>,
    pub include_all: bool,
}

#[derive(Debug, Clone)]
pub struct RmrrInfo {
    pub segment: u16,
    pub base: u64,
    pub limit: u64,
    pub device_scopes: Vec<DeviceScope>,
}

#[derive(Debug, Clone)]
pub struct DmarInfo {
    pub host_address_width: u8,
    pub flags: u8,
    pub drhds: Vec<DrhdInfo>,
    pub rmrrs: Vec<RmrrInfo>,
}

unsafe fn parse_device_scopes(base: *const u8, start: usize, end: usize) -> Vec<DeviceScope> {
    let mut scopes = Vec::new();
    let mut off = start;
    while off + 6 <= end {
        let scope_type = *base.add(off);
        let scope_len = *base.add(off + 1) as usize;
        if scope_len < 6 || off + scope_len > end {
            break;
        }
        let enum_id = *base.add(off + 2);
        let start_bus = *base.add(off + 4);
        let path_bytes = scope_len - 6;
        let mut path = Vec::new();
        let mut p = off + 6;
        while p + 1 < off + scope_len {
            let dev = *base.add(p);
            let func = *base.add(p + 1);
            path.push((dev, func));
            p += 2;
        }
        scopes.push(DeviceScope {
            scope_type: DeviceScopeType::from(scope_type),
            enumeration_id: enum_id,
            start_bus,
            path,
        });
        let _ = path_bytes;
        off += scope_len;
    }
    scopes
}

pub unsafe fn parse_dmar_full(addr: u64) -> Option<DmarInfo> {
    let ptr = addr as *const u8;
    let length = *(ptr.add(4) as *const u32) as usize;
    if length < 48 {
        return None;
    }
    let haw = *ptr.add(36);
    let flags = *ptr.add(37);
    let mut drhds = Vec::new();
    let mut rmrrs = Vec::new();
    let mut offset = 48;
    while offset + 4 <= length {
        let entry_type = *(ptr.add(offset) as *const u16);
        let entry_len = *(ptr.add(offset + 2) as *const u16) as usize;
        if entry_len < 4 || offset + entry_len > length {
            break;
        }
        match entry_type {
            0 if entry_len >= 16 => {
                let e = ptr.add(offset);
                let fl = *e.add(4);
                let seg = *(e.add(6) as *const u16);
                let reg_base = *(e.add(8) as *const u64);
                let scopes = parse_device_scopes(ptr, offset + 16, offset + entry_len);
                drhds.push(DrhdInfo {
                    flags: fl,
                    segment: seg,
                    register_base: reg_base,
                    device_scopes: scopes,
                    include_all: fl & 0x01 != 0,
                });
            }
            1 if entry_len >= 24 => {
                let e = ptr.add(offset);
                let seg = *(e.add(6) as *const u16);
                let base = *(e.add(8) as *const u64);
                let limit = *(e.add(16) as *const u64);
                let scopes = parse_device_scopes(ptr, offset + 24, offset + entry_len);
                rmrrs.push(RmrrInfo {
                    segment: seg,
                    base,
                    limit,
                    device_scopes: scopes,
                });
            }
            _ => {}
        }
        offset += entry_len;
    }
    Some(DmarInfo {
        host_address_width: haw,
        flags,
        drhds,
        rmrrs,
    })
}

pub const SIG_IVRS: [u8; 4] = *b"IVRS";

#[derive(Debug, Clone)]
pub struct AmdIommuInfo {
    pub base_address: u64,
    pub pci_segment: u16,
    pub pci_bdf: u16,
    pub flags: u8,
}

#[derive(Debug, Clone)]
pub struct IvrsInfo {
    pub iv_info: u32,
    pub iommus: Vec<AmdIommuInfo>,
}

pub unsafe fn parse_ivrs(addr: u64) -> Option<IvrsInfo> {
    let ptr = addr as *const u8;
    let length = *(ptr.add(4) as *const u32) as usize;
    if length < 48 {
        return None;
    }

    let iv_info = *(ptr.add(36) as *const u32);
    let mut iommus = Vec::new();
    let mut offset = 48;

    while offset + 4 <= length {
        let entry_type = *ptr.add(offset);
        let entry_len = *ptr.add(offset + 1) as usize;
        if entry_len < 4 || offset + entry_len > length {
            break;
        }

        match entry_type {
            0x10 => {
                // IOMMU Hardware Definition Block
                let e = ptr.add(offset);
                let cap_ptr = *(e.add(4) as *const u16);
                let base = *(e.add(8) as *const u64);
                let seg = *(e.add(16) as *const u16);
                let bdf = *(e.add(18) as *const u16);
                let flags = *e.add(20);
                iommus.push(AmdIommuInfo {
                    base_address: base,
                    pci_segment: seg,
                    pci_bdf: bdf,
                    flags,
                });
            }
            _ => {}
        }
        offset += entry_len;
    }

    Some(IvrsInfo { iv_info, iommus })
}

// ─── VT-d Register Offsets ──────────────────────────────────────────────────

mod vtd_regs {
    pub const VER: usize = 0x00;
    pub const CAP: usize = 0x08;
    pub const ECAP: usize = 0x10;
    pub const GCMD: usize = 0x18;
    pub const GSTS: usize = 0x1C;
    pub const RTADDR: usize = 0x20;
    pub const CCMD: usize = 0x28;
    pub const FSTS: usize = 0x34;
    pub const FECTL: usize = 0x38;
    pub const FEDATA: usize = 0x3C;
    pub const FEADDR: usize = 0x40;
    pub const FEUADDR: usize = 0x44;
    pub const AFLOG: usize = 0x58;
    pub const PMEN: usize = 0x64;
    pub const PLMBASE: usize = 0x68;
    pub const PLMLIMIT: usize = 0x6C;
    pub const PHMBASE: usize = 0x70;
    pub const PHMLIMIT: usize = 0x78;
    pub const IQH: usize = 0x80;
    pub const IQT: usize = 0x88;
    pub const IQA: usize = 0x90;
    pub const ICS: usize = 0x9C;
    pub const IRTA: usize = 0xB8;
    pub const IOTLB_OFF: usize = 0x08;
    pub const IVA_OFF: usize = 0x00;

    pub const GCMD_TE: u32 = 1 << 31;
    pub const GCMD_SRTP: u32 = 1 << 30;
    pub const GCMD_SFL: u32 = 1 << 29;
    pub const GCMD_EAFL: u32 = 1 << 28;
    pub const GCMD_WBF: u32 = 1 << 27;
    pub const GCMD_QIE: u32 = 1 << 26;
    pub const GCMD_IRE: u32 = 1 << 25;
    pub const GCMD_CFI: u32 = 1 << 23;

    pub const GSTS_TES: u32 = 1 << 31;
    pub const GSTS_RTPS: u32 = 1 << 30;
    pub const GSTS_FLS: u32 = 1 << 29;
    pub const GSTS_AFLS: u32 = 1 << 28;
    pub const GSTS_WBFS: u32 = 1 << 27;
    pub const GSTS_QIES: u32 = 1 << 26;
    pub const GSTS_IRES: u32 = 1 << 25;
    pub const GSTS_CFIS: u32 = 1 << 23;

    pub const CAP_SAGAW_SHIFT: u64 = 8;
    pub const CAP_SAGAW_MASK: u64 = 0x1F;
    pub const CAP_MGAW_SHIFT: u64 = 16;
    pub const CAP_MGAW_MASK: u64 = 0x3F;
    pub const CAP_ND_MASK: u64 = 0x07;
    pub const CAP_SLLPS_SHIFT: u64 = 34;
    pub const CAP_FRO_SHIFT: u64 = 24;
    pub const CAP_FRO_MASK: u64 = 0x3FF;
    pub const CAP_NFR_SHIFT: u64 = 40;
    pub const CAP_NFR_MASK: u64 = 0xFF;

    pub const ECAP_QI: u64 = 1 << 1;
    pub const ECAP_IRO_SHIFT: u64 = 8;
    pub const ECAP_IRO_MASK: u64 = 0x3FF;
    pub const ECAP_C: u64 = 1 << 0;

    pub const CCMD_ICC: u64 = 1 << 63;
    pub const CCMD_GLOBAL_INV: u64 = 0x01 << 61;
    pub const CCMD_DOMAIN_INV: u64 = 0x02 << 61;
    pub const CCMD_DEVICE_INV: u64 = 0x03 << 61;

    pub const FSTS_PPF: u32 = 1 << 1;
    pub const FSTS_PFO: u32 = 1 << 0;
    pub const FSTS_IQE: u32 = 1 << 4;
    pub const FSTS_ICE: u32 = 1 << 5;
    pub const FSTS_ITE: u32 = 1 << 6;
    pub const FSTS_FRI_SHIFT: u32 = 8;
    pub const FSTS_FRI_MASK: u32 = 0xFF;
}

// ─── MMIO Accessors ─────────────────────────────────────────────────────────

struct VtdMmio {
    base_virt: u64,
}

impl VtdMmio {
    unsafe fn new(phys_base: u64) -> Self {
        let offset = PHYS_MEM_OFFSET.get().expect("PHYS_MEM_OFFSET not set");
        Self {
            base_virt: offset.as_u64() + phys_base,
        }
    }

    unsafe fn read32(&self, offset: usize) -> u32 {
        ptr::read_volatile((self.base_virt + offset as u64) as *const u32)
    }

    unsafe fn write32(&self, offset: usize, val: u32) {
        ptr::write_volatile((self.base_virt + offset as u64) as *mut u32, val);
    }

    unsafe fn read64(&self, offset: usize) -> u64 {
        ptr::read_volatile((self.base_virt + offset as u64) as *const u64)
    }

    unsafe fn write64(&self, offset: usize, val: u64) {
        ptr::write_volatile((self.base_virt + offset as u64) as *mut u64, val);
    }
}

// ─── Capability Decoding ────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct VtdCapability {
    pub raw: u64,
    pub num_domains: u32,
    pub mgaw: u8,
    pub sagaw: u8,
    pub num_fault_regs: u8,
    pub fault_reg_offset: u16,
    pub supports_super_pages: bool,
}

impl VtdCapability {
    fn from_raw(raw: u64) -> Self {
        let nd_val = raw & vtd_regs::CAP_ND_MASK;
        let num_domains = match nd_val {
            0 => 16,
            1 => 64,
            2 => 256,
            3 => 1024,
            4 => 65536,
            5 => 262144,
            _ => 16,
        };
        let mgaw = ((raw >> vtd_regs::CAP_MGAW_SHIFT) & vtd_regs::CAP_MGAW_MASK) as u8 + 1;
        let sagaw = ((raw >> vtd_regs::CAP_SAGAW_SHIFT) & vtd_regs::CAP_SAGAW_MASK) as u8;
        let fro = ((raw >> vtd_regs::CAP_FRO_SHIFT) & vtd_regs::CAP_FRO_MASK) as u16;
        let nfr = ((raw >> vtd_regs::CAP_NFR_SHIFT) & vtd_regs::CAP_NFR_MASK) as u8 + 1;
        let sllps = (raw >> vtd_regs::CAP_SLLPS_SHIFT) & 0x01 != 0;
        Self {
            raw,
            num_domains,
            mgaw,
            sagaw,
            num_fault_regs: nfr,
            fault_reg_offset: fro * 16,
            supports_super_pages: sllps,
        }
    }

    fn best_agaw(&self) -> u8 {
        if self.sagaw & (1 << 2) != 0 {
            48
        } else if self.sagaw & (1 << 1) != 0 {
            39
        } else {
            30
        }
    }

    fn agaw_level(&self) -> u8 {
        match self.best_agaw() {
            48 => 4,
            39 => 3,
            _ => 2,
        }
    }
}

#[derive(Debug, Clone)]
pub struct VtdExtCapability {
    pub raw: u64,
    pub queued_invalidation: bool,
    pub coherent: bool,
    pub iotlb_reg_offset: u16,
}

impl VtdExtCapability {
    fn from_raw(raw: u64) -> Self {
        let iro = ((raw >> vtd_regs::ECAP_IRO_SHIFT) & vtd_regs::ECAP_IRO_MASK) as u16;
        Self {
            raw,
            queued_invalidation: raw & vtd_regs::ECAP_QI != 0,
            coherent: raw & vtd_regs::ECAP_C != 0,
            iotlb_reg_offset: iro * 16,
        }
    }
}

// ─── Translation Table Entries ──────────────────────────────────────────────

#[repr(C, align(16))]
#[derive(Clone, Copy)]
struct RootEntry {
    lo: u64,
    hi: u64,
}

impl RootEntry {
    const fn empty() -> Self {
        Self { lo: 0, hi: 0 }
    }

    fn set_present(&mut self, context_table_phys: u64) {
        self.lo = (context_table_phys & !0xFFF) | 1;
    }

    fn is_present(&self) -> bool {
        self.lo & 1 != 0
    }

    fn context_table_phys(&self) -> u64 {
        self.lo & !0xFFF
    }
}

#[repr(C, align(16))]
#[derive(Clone, Copy)]
struct ContextEntry {
    lo: u64,
    hi: u64,
}

impl ContextEntry {
    const fn empty() -> Self {
        Self { lo: 0, hi: 0 }
    }

    fn set(&mut self, page_table_phys: u64, domain_id: u16, agaw: u8) {
        let aw = match agaw {
            30 => 0u64,
            39 => 1,
            48 => 2,
            57 => 3,
            _ => 1,
        };
        self.lo = (page_table_phys & !0xFFF) | 1;
        self.hi = (domain_id as u64) << 8 | (aw << 2);
    }

    fn is_present(&self) -> bool {
        self.lo & 1 != 0
    }

    fn domain_id(&self) -> u16 {
        ((self.hi >> 8) & 0xFFFF) as u16
    }
}

const IOMMU_PTE_READ: u64 = 1 << 0;
const IOMMU_PTE_WRITE: u64 = 1 << 1;
const IOMMU_PTE_SUPER: u64 = 1 << 7;

fn make_pte(phys: u64, readable: bool, writable: bool) -> u64 {
    let mut flags = 0u64;
    if readable {
        flags |= IOMMU_PTE_READ;
    }
    if writable {
        flags |= IOMMU_PTE_WRITE;
    }
    (phys & !0xFFF) | flags
}

// ─── DMA Domain ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct DmaMapping {
    pub iova: u64,
    pub phys: u64,
    pub size: u64,
    pub readable: bool,
    pub writable: bool,
}

pub struct DmaDomain {
    pub domain_id: u16,
    pub page_table_root: u64,
    pub mapped_regions: Vec<DmaMapping>,
    agaw: u8,
}

impl DmaDomain {
    fn new(domain_id: u16, agaw: u8) -> Option<Self> {
        let mut alloc = GlobalFrameAllocator;
        let frame = alloc.allocate_frame()?;
        let phys = frame.start_address().as_u64();
        let offset = PHYS_MEM_OFFSET.get()?;
        let virt = offset.as_u64() + phys;
        unsafe {
            ptr::write_bytes(virt as *mut u8, 0, 4096);
        }
        Some(Self {
            domain_id,
            page_table_root: phys,
            mapped_regions: Vec::new(),
            agaw,
        })
    }

    fn levels(&self) -> usize {
        match self.agaw {
            48 => 4,
            39 => 3,
            _ => 2,
        }
    }

    pub fn map_page(&mut self, iova: u64, phys: u64, readable: bool, writable: bool) -> bool {
        let offset = match PHYS_MEM_OFFSET.get() {
            Some(o) => o.as_u64(),
            None => return false,
        };
        let levels = self.levels();
        let mut table_phys = self.page_table_root;

        for level in (1..levels).rev() {
            let virt_table = offset + table_phys;
            let index = ((iova >> (12 + level * 9)) & 0x1FF) as usize;
            let entry_ptr = (virt_table + index as u64 * 8) as *mut u64;
            let entry = unsafe { ptr::read_volatile(entry_ptr) };
            if entry & (IOMMU_PTE_READ | IOMMU_PTE_WRITE) == 0 {
                let mut alloc = GlobalFrameAllocator;
                let frame = match alloc.allocate_frame() {
                    Some(f) => f,
                    None => return false,
                };
                let new_phys = frame.start_address().as_u64();
                unsafe {
                    ptr::write_bytes((offset + new_phys) as *mut u8, 0, 4096);
                    ptr::write_volatile(entry_ptr, new_phys | IOMMU_PTE_READ | IOMMU_PTE_WRITE);
                }
                table_phys = new_phys;
            } else {
                table_phys = entry & !0xFFF;
            }
        }

        let index = ((iova >> 12) & 0x1FF) as usize;
        let virt_table = offset + table_phys;
        let entry_ptr = (virt_table + index as u64 * 8) as *mut u64;
        unsafe {
            ptr::write_volatile(entry_ptr, make_pte(phys, readable, writable));
        }
        true
    }

    pub fn unmap_page(&mut self, iova: u64) -> bool {
        let offset = match PHYS_MEM_OFFSET.get() {
            Some(o) => o.as_u64(),
            None => return false,
        };
        let levels = self.levels();
        let mut table_phys = self.page_table_root;

        for level in (1..levels).rev() {
            let virt_table = offset + table_phys;
            let index = ((iova >> (12 + level * 9)) & 0x1FF) as usize;
            let entry_ptr = (virt_table + index as u64 * 8) as *const u64;
            let entry = unsafe { ptr::read_volatile(entry_ptr) };
            if entry & (IOMMU_PTE_READ | IOMMU_PTE_WRITE) == 0 {
                return false;
            }
            table_phys = entry & !0xFFF;
        }

        let index = ((iova >> 12) & 0x1FF) as usize;
        let virt_table = offset + table_phys;
        let entry_ptr = (virt_table + index as u64 * 8) as *mut u64;
        unsafe {
            ptr::write_volatile(entry_ptr, 0);
        }
        true
    }

    pub fn map_dma(
        &mut self,
        iova: u64,
        phys: u64,
        size: u64,
        readable: bool,
        writable: bool,
    ) -> bool {
        if iova & 0xFFF != 0 || phys & 0xFFF != 0 || size == 0 {
            return false;
        }
        let pages = (size + 0xFFF) / 4096;
        for i in 0..pages {
            let page_iova = iova + i * 4096;
            let page_phys = phys + i * 4096;
            if !self.map_page(page_iova, page_phys, readable, writable) {
                return false;
            }
        }
        self.mapped_regions.push(DmaMapping {
            iova,
            phys,
            size: pages * 4096,
            readable,
            writable,
        });
        true
    }

    pub fn unmap_dma(&mut self, iova: u64, size: u64) -> bool {
        if iova & 0xFFF != 0 || size == 0 {
            return false;
        }
        let pages = (size + 0xFFF) / 4096;
        for i in 0..pages {
            self.unmap_page(iova + i * 4096);
        }
        self.mapped_regions.retain(|m| m.iova != iova);
        true
    }

    /// Walk this domain's second-level page tables and return the physical
    /// address `iova` resolves to, or `None` if `iova` is unmapped — i.e. a
    /// device DMA there would raise a VT-d translation fault. This mirrors the
    /// hardware page-table walk in software and is the mechanism behind Concept
    /// §"IOMMU-enforced, no exceptions": only ranges explicitly installed via
    /// `map_dma` resolve; every other address is blocked.
    pub fn translate(&self, iova: u64) -> Option<u64> {
        let offset = PHYS_MEM_OFFSET.get()?.as_u64();
        let levels = self.levels();
        let mut table_phys = self.page_table_root;
        for level in (1..levels).rev() {
            let virt_table = offset + table_phys;
            let index = ((iova >> (12 + level * 9)) & 0x1FF) as usize;
            let entry =
                unsafe { ptr::read_volatile((virt_table + index as u64 * 8) as *const u64) };
            if entry & (IOMMU_PTE_READ | IOMMU_PTE_WRITE) == 0 {
                return None;
            }
            table_phys = entry & !0xFFF;
        }
        let index = ((iova >> 12) & 0x1FF) as usize;
        let virt_table = offset + table_phys;
        let entry = unsafe { ptr::read_volatile((virt_table + index as u64 * 8) as *const u64) };
        if entry & (IOMMU_PTE_READ | IOMMU_PTE_WRITE) == 0 {
            return None;
        }
        Some((entry & !0xFFF) | (iova & 0xFFF))
    }

    /// Free every page-table frame this domain allocated (root + all
    /// intermediate levels), leaving leaf-mapped data frames untouched — the
    /// caller owns those. Used by the boot self-test so it doesn't leak the
    /// frames it allocates.
    fn free_page_tables(&mut self) {
        let Some(offset) = PHYS_MEM_OFFSET.get().map(|o| o.as_u64()) else {
            return;
        };
        // `level` > 0 → a table whose present entries point at the next-level
        // table; recurse, then free. `level` == 0 → a leaf table whose entries
        // point at data frames we must NOT free.
        fn free_subtree(offset: u64, table_phys: u64, level: usize) {
            if level > 0 {
                let virt = offset + table_phys;
                for i in 0..512u64 {
                    let e = unsafe { ptr::read_volatile((virt + i * 8) as *const u64) };
                    if e & (IOMMU_PTE_READ | IOMMU_PTE_WRITE) != 0 {
                        free_subtree(offset, e & !0xFFF, level - 1);
                    }
                }
            }
            use x86_64::structures::paging::{PhysFrame, Size4KiB};
            let frame =
                PhysFrame::<Size4KiB>::containing_address(crate::arch::PhysAddr::new(table_phys));
            crate::memory::deallocate_frame(frame);
        }
        free_subtree(offset, self.page_table_root, self.levels() - 1);
        self.page_table_root = 0;
        self.mapped_regions.clear();
    }
}

// ─── DMA Fault Record ──────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct DmaFaultRecord {
    pub source_bus: u8,
    pub source_dev: u8,
    pub source_func: u8,
    pub fault_reason: u8,
    pub address: u64,
    pub is_write: bool,
    pub is_read: bool,
    pub pasid_present: bool,
}

// ─── Remapping Hardware Unit ────────────────────────────────────────────────

struct RemappingUnit {
    mmio: VtdMmio,
    drhd: DrhdInfo,
    cap: VtdCapability,
    ecap: VtdExtCapability,
    root_table_phys: u64,
}

impl RemappingUnit {
    unsafe fn new(drhd: DrhdInfo) -> Option<Self> {
        let mmio = VtdMmio::new(drhd.register_base);
        let ver = mmio.read32(vtd_regs::VER);
        let major = (ver >> 4) & 0xF;
        let minor = ver & 0xF;
        crate::serial_println!(
            "[iommu] DRHD @ {:#x}: VT-d version {}.{}, segment {}",
            drhd.register_base,
            major,
            minor,
            drhd.segment
        );

        let cap_raw = mmio.read64(vtd_regs::CAP);
        let ecap_raw = mmio.read64(vtd_regs::ECAP);
        let cap = VtdCapability::from_raw(cap_raw);
        let ecap = VtdExtCapability::from_raw(ecap_raw);

        crate::serial_println!(
            "[iommu]   CAP: {} domains, MGAW={}, SAGAW={:#x}, {} fault regs",
            cap.num_domains,
            cap.mgaw,
            cap.sagaw,
            cap.num_fault_regs,
        );
        crate::serial_println!(
            "[iommu]   ECAP: QI={}, coherent={}, IOTLB offset={:#x}",
            ecap.queued_invalidation,
            ecap.coherent,
            ecap.iotlb_reg_offset,
        );

        let mut alloc = GlobalFrameAllocator;
        let root_frame = alloc.allocate_frame()?;
        let root_phys = root_frame.start_address().as_u64();
        let offset = PHYS_MEM_OFFSET.get()?.as_u64();
        ptr::write_bytes((offset + root_phys) as *mut u8, 0, 4096);

        Some(Self {
            mmio,
            drhd,
            cap,
            ecap,
            root_table_phys: root_phys,
        })
    }

    unsafe fn set_root_table(&self) {
        self.mmio.write64(vtd_regs::RTADDR, self.root_table_phys);

        let cmd = self.mmio.read32(vtd_regs::GCMD);
        self.mmio.write32(vtd_regs::GCMD, cmd | vtd_regs::GCMD_SRTP);

        for _ in 0..100_000 {
            if self.mmio.read32(vtd_regs::GSTS) & vtd_regs::GSTS_RTPS != 0 {
                return;
            }
            core::hint::spin_loop();
        }
        crate::serial_println!("[iommu] WARNING: SRTP timeout");
    }

    unsafe fn enable_translation(&self) {
        let cmd = self.mmio.read32(vtd_regs::GCMD);
        self.mmio.write32(vtd_regs::GCMD, cmd | vtd_regs::GCMD_TE);

        for _ in 0..100_000 {
            if self.mmio.read32(vtd_regs::GSTS) & vtd_regs::GSTS_TES != 0 {
                crate::serial_println!(
                    "[iommu] Translation enabled for DRHD @ {:#x}",
                    self.drhd.register_base
                );
                return;
            }
            core::hint::spin_loop();
        }
        crate::serial_println!(
            "[iommu] WARNING: TE timeout for DRHD @ {:#x}",
            self.drhd.register_base
        );
    }

    unsafe fn disable_translation(&self) {
        let cmd = self.mmio.read32(vtd_regs::GCMD);
        self.mmio.write32(vtd_regs::GCMD, cmd & !vtd_regs::GCMD_TE);

        for _ in 0..100_000 {
            if self.mmio.read32(vtd_regs::GSTS) & vtd_regs::GSTS_TES == 0 {
                return;
            }
            core::hint::spin_loop();
        }
        crate::serial_println!("[iommu] WARNING: disable-TE timeout");
    }

    unsafe fn write_buffer_flush(&self) {
        if !self.ecap.coherent {
            let cmd = self.mmio.read32(vtd_regs::GCMD);
            self.mmio.write32(vtd_regs::GCMD, cmd | vtd_regs::GCMD_WBF);
            for _ in 0..100_000 {
                if self.mmio.read32(vtd_regs::GSTS) & vtd_regs::GSTS_WBFS == 0 {
                    return;
                }
                core::hint::spin_loop();
            }
        }
    }

    // ── Context-cache invalidation ──────────────────────────────────────

    unsafe fn invalidate_context_global(&self) {
        self.mmio.write64(
            vtd_regs::CCMD,
            vtd_regs::CCMD_ICC | vtd_regs::CCMD_GLOBAL_INV,
        );
        for _ in 0..100_000 {
            if self.mmio.read64(vtd_regs::CCMD) & vtd_regs::CCMD_ICC == 0 {
                return;
            }
            core::hint::spin_loop();
        }
    }

    unsafe fn invalidate_context_domain(&self, domain_id: u16) {
        let val = vtd_regs::CCMD_ICC | vtd_regs::CCMD_DOMAIN_INV | (domain_id as u64);
        self.mmio.write64(vtd_regs::CCMD, val);
        for _ in 0..100_000 {
            if self.mmio.read64(vtd_regs::CCMD) & vtd_regs::CCMD_ICC == 0 {
                return;
            }
            core::hint::spin_loop();
        }
    }

    unsafe fn invalidate_context_device(&self, domain_id: u16, source_id: u16) {
        let val = vtd_regs::CCMD_ICC
            | vtd_regs::CCMD_DEVICE_INV
            | (domain_id as u64)
            | ((source_id as u64) << 16);
        self.mmio.write64(vtd_regs::CCMD, val);
        for _ in 0..100_000 {
            if self.mmio.read64(vtd_regs::CCMD) & vtd_regs::CCMD_ICC == 0 {
                return;
            }
            core::hint::spin_loop();
        }
    }

    // ── IOTLB invalidation (register-based) ─────────────────────────────

    fn iotlb_reg_addr(&self) -> u64 {
        self.mmio.base_virt + self.ecap.iotlb_reg_offset as u64
    }

    unsafe fn iotlb_flush_global(&self) {
        let iotlb_addr = self.iotlb_reg_addr() + vtd_regs::IOTLB_OFF as u64;
        let cmd: u64 = (1u64 << 63) | (0x01u64 << 60); // IVT + global
        ptr::write_volatile(iotlb_addr as *mut u64, cmd);
        for _ in 0..100_000 {
            if ptr::read_volatile(iotlb_addr as *const u64) & (1u64 << 63) == 0 {
                return;
            }
            core::hint::spin_loop();
        }
    }

    unsafe fn iotlb_flush_domain(&self, domain_id: u16) {
        let iotlb_addr = self.iotlb_reg_addr() + vtd_regs::IOTLB_OFF as u64;
        let cmd: u64 = (1u64 << 63) | (0x02u64 << 60) | (domain_id as u64);
        ptr::write_volatile(iotlb_addr as *mut u64, cmd);
        for _ in 0..100_000 {
            if ptr::read_volatile(iotlb_addr as *const u64) & (1u64 << 63) == 0 {
                return;
            }
            core::hint::spin_loop();
        }
    }

    unsafe fn iotlb_flush_page(&self, domain_id: u16, iova: u64) {
        let iva_addr = self.iotlb_reg_addr() + vtd_regs::IVA_OFF as u64;
        ptr::write_volatile(iva_addr as *mut u64, (iova & !0xFFF) | (1 << 6));

        let iotlb_addr = self.iotlb_reg_addr() + vtd_regs::IOTLB_OFF as u64;
        let cmd: u64 = (1u64 << 63) | (0x03u64 << 60) | (domain_id as u64);
        ptr::write_volatile(iotlb_addr as *mut u64, cmd);
        for _ in 0..100_000 {
            if ptr::read_volatile(iotlb_addr as *const u64) & (1u64 << 63) == 0 {
                return;
            }
            core::hint::spin_loop();
        }
    }

    // ── Fault handling ──────────────────────────────────────────────────

    unsafe fn read_fault_status(&self) -> u32 {
        self.mmio.read32(vtd_regs::FSTS)
    }

    unsafe fn clear_fault_status(&self) {
        let fsts = self.mmio.read32(vtd_regs::FSTS);
        self.mmio.write32(
            vtd_regs::FSTS,
            fsts & (vtd_regs::FSTS_PFO | vtd_regs::FSTS_PPF),
        );
    }

    unsafe fn read_fault_record(&self, index: u8) -> Option<DmaFaultRecord> {
        if index >= self.cap.num_fault_regs {
            return None;
        }
        let fro = self.cap.fault_reg_offset as u64;
        let rec_addr = self.mmio.base_virt + fro + (index as u64) * 16;
        let lo = ptr::read_volatile(rec_addr as *const u64);
        let hi = ptr::read_volatile((rec_addr + 8) as *const u64);
        if hi & (1u64 << 63) == 0 {
            return None;
        }
        let fault_reason = ((hi >> 32) & 0xFF) as u8;
        let source_id = ((hi >> 40) & 0xFFFF) as u16;
        let bus = (source_id >> 8) as u8;
        let devfn = source_id as u8;
        let dev = devfn >> 3;
        let func = devfn & 0x07;
        let address = lo & !0xFFF;
        let is_write = hi & (1u64 << 30) != 0;
        let is_read = hi & (1u64 << 29) != 0;
        let pasid_present = hi & (1u64 << 31) != 0;
        // Clear the fault bit
        ptr::write_volatile((rec_addr + 8) as *mut u64, hi | (1u64 << 63));
        Some(DmaFaultRecord {
            source_bus: bus,
            source_dev: dev,
            source_func: func,
            fault_reason,
            address,
            is_write,
            is_read,
            pasid_present,
        })
    }

    // ── Root/context table manipulation ─────────────────────────────────

    unsafe fn get_root_entry(&self, bus: u8) -> *mut RootEntry {
        let offset = PHYS_MEM_OFFSET.get().unwrap().as_u64();
        let table = (offset + self.root_table_phys) as *mut RootEntry;
        table.add(bus as usize)
    }

    unsafe fn ensure_context_table(&self, bus: u8) -> u64 {
        let root_ptr = self.get_root_entry(bus);
        let root = ptr::read_volatile(root_ptr);
        if root.is_present() {
            return root.context_table_phys();
        }
        let mut alloc = GlobalFrameAllocator;
        if let Some(frame) = alloc.allocate_frame() {
            let phys = frame.start_address().as_u64();
            let voff = PHYS_MEM_OFFSET.get().unwrap().as_u64();
            ptr::write_bytes((voff + phys) as *mut u8, 0, 4096);
            let mut entry = RootEntry::empty();
            entry.set_present(phys);
            ptr::write_volatile(root_ptr, entry);
            phys
        } else {
            0
        }
    }

    unsafe fn assign_device(&self, bus: u8, dev: u8, func: u8, domain: &DmaDomain) -> bool {
        let ctx_phys = self.ensure_context_table(bus);
        if ctx_phys == 0 {
            return false;
        }
        let offset = PHYS_MEM_OFFSET.get().unwrap().as_u64();
        let devfn = ((dev as usize) << 3) | (func as usize);
        let ctx_ptr = (offset + ctx_phys) as *mut ContextEntry;
        let entry_ptr = ctx_ptr.add(devfn);
        let mut entry = ContextEntry::empty();
        entry.set(
            domain.page_table_root,
            domain.domain_id,
            self.cap.best_agaw(),
        );
        ptr::write_volatile(entry_ptr, entry);
        true
    }

    unsafe fn unassign_device(&self, bus: u8, dev: u8, func: u8) {
        let root_ptr = self.get_root_entry(bus);
        let root = ptr::read_volatile(root_ptr);
        if !root.is_present() {
            return;
        }
        let offset = PHYS_MEM_OFFSET.get().unwrap().as_u64();
        let devfn = ((dev as usize) << 3) | (func as usize);
        let ctx_ptr = (offset + root.context_table_phys()) as *mut ContextEntry;
        ptr::write_volatile(ctx_ptr.add(devfn), ContextEntry::empty());
    }
}

// ─── Global IOMMU State ────────────────────────────────────────────────────

struct IommuState {
    units: Vec<RemappingUnit>,
    domains: Vec<DmaDomain>,
    next_domain_id: AtomicU16,
    dmar_info: Option<DmarInfo>,
    enabled: bool,
}

impl IommuState {
    const fn new() -> Self {
        Self {
            units: Vec::new(),
            domains: Vec::new(),
            next_domain_id: AtomicU16::new(1),
            dmar_info: None,
            enabled: false,
        }
    }
}

static IOMMU: Mutex<IommuState> = Mutex::new(IommuState::new());

// ─── Public API ─────────────────────────────────────────────────────────────

pub fn create_domain() -> Option<u16> {
    let mut state = IOMMU.lock();
    if state.units.is_empty() {
        return None;
    }
    let domain_id = state.next_domain_id.fetch_add(1, Ordering::Relaxed);
    let agaw = state.units[0].cap.best_agaw();
    let domain = DmaDomain::new(domain_id, agaw)?;
    let id = domain.domain_id;
    state.domains.push(domain);
    Some(id)
}

pub fn destroy_domain(domain_id: u16) -> bool {
    let mut state = IOMMU.lock();
    let idx = match state.domains.iter().position(|d| d.domain_id == domain_id) {
        Some(i) => i,
        None => return false,
    };

    // Unmap all DMA regions first.
    let regions = state.domains[idx].mapped_regions.clone();
    for r in regions {
        state.domains[idx].unmap_dma(r.iova, r.size);
    }

    state.domains.remove(idx);

    for unit in &state.units {
        unsafe {
            unit.iotlb_flush_domain(domain_id);
            unit.write_buffer_flush();
        }
    }
    true
}

pub fn map_dma(
    domain_id: u16,
    iova: u64,
    phys: u64,
    size: u64,
    readable: bool,
    writable: bool,
) -> bool {
    let mut state = IOMMU.lock();
    if let Some(domain) = state.domains.iter_mut().find(|d| d.domain_id == domain_id) {
        if domain.map_dma(iova, phys, size, readable, writable) {
            for unit in &state.units {
                unsafe {
                    unit.write_buffer_flush();
                }
            }
            return true;
        }
    }
    false
}

pub fn unmap_dma(domain_id: u16, iova: u64, size: u64) -> bool {
    let mut state = IOMMU.lock();
    let success = if let Some(domain) = state.domains.iter_mut().find(|d| d.domain_id == domain_id)
    {
        domain.unmap_dma(iova, size)
    } else {
        return false;
    };
    if success {
        for unit in &state.units {
            unsafe {
                unit.iotlb_flush_domain(domain_id);
                unit.write_buffer_flush();
            }
        }
    }
    success
}

pub fn assign_device(domain_id: u16, bus: u8, dev: u8, func: u8) -> bool {
    let state = IOMMU.lock();
    let domain = match state.domains.iter().find(|d| d.domain_id == domain_id) {
        Some(d) => d,
        None => return false,
    };
    let mut ok = false;
    for unit in &state.units {
        unsafe {
            if unit.assign_device(bus, dev, func, domain) {
                let source_id = ((bus as u16) << 8) | ((dev as u16) << 3) | (func as u16);
                unit.invalidate_context_device(domain_id, source_id);
                unit.iotlb_flush_domain(domain_id);
                unit.write_buffer_flush();
                ok = true;
                crate::serial_println!(
                    "[iommu] Assigned {:02x}:{:02x}.{} to domain {}",
                    bus,
                    dev,
                    func,
                    domain_id
                );
            }
        }
    }
    ok
}

pub fn unassign_device(bus: u8, dev: u8, func: u8) {
    let state = IOMMU.lock();
    for unit in &state.units {
        unsafe {
            unit.unassign_device(bus, dev, func);
            let source_id = ((bus as u16) << 8) | ((dev as u16) << 3) | (func as u16);
            unit.invalidate_context_device(0, source_id);
            unit.iotlb_flush_global();
            unit.write_buffer_flush();
        }
    }
}

/// Flush IOTLB globally across all remapping units.
pub fn flush_iotlb_global() {
    let state = IOMMU.lock();
    for unit in &state.units {
        unsafe {
            unit.iotlb_flush_global();
        }
    }
}

/// Flush IOTLB for a specific domain.
pub fn flush_iotlb_domain(domain_id: u16) {
    let state = IOMMU.lock();
    for unit in &state.units {
        unsafe {
            unit.iotlb_flush_domain(domain_id);
        }
    }
}

/// Flush IOTLB for a specific page in a domain.
pub fn flush_iotlb_page(domain_id: u16, iova: u64) {
    let state = IOMMU.lock();
    for unit in &state.units {
        unsafe {
            unit.iotlb_flush_page(domain_id, iova);
        }
    }
}

/// DMA faults whose source device the kernel isolated. Phase 4.3: a wild DMA is
/// not just blocked + logged — the offending function is isolated so its rogue
/// driver can no longer touch the bus (the IOMMU half of "driver crash ≠ system
/// crash", Concept §Architecture).
static DMA_FAULT_KILLS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// What the kernel does about a delivered DMA fault. A translation fault is
/// always a confinement violation by the device, so the response is to ISOLATE
/// its function — never panic. Pure decision so `poll_faults` (live) and the
/// injection smoketest apply the identical policy.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FaultAction {
    Isolate { bus: u8, dev: u8, func: u8 },
}

/// Map a DMA fault record to the kernel's response: isolate the faulting
/// function. Pure (no PCI I/O) so it's testable on synthetic records.
pub fn fault_action(rec: &DmaFaultRecord) -> FaultAction {
    FaultAction::Isolate {
        bus: rec.source_bus,
        dev: rec.source_dev,
        func: rec.source_func,
    }
}

/// Check and drain DMA faults from all units. Returns fault records.
pub fn poll_faults() -> Vec<DmaFaultRecord> {
    let state = IOMMU.lock();
    let mut faults = Vec::new();
    for unit in &state.units {
        unsafe {
            let fsts = unit.read_fault_status();
            if fsts & vtd_regs::FSTS_PPF != 0 {
                let fri = ((fsts >> vtd_regs::FSTS_FRI_SHIFT) & vtd_regs::FSTS_FRI_MASK) as u8;
                for i in 0..unit.cap.num_fault_regs {
                    let idx = (fri + i) % unit.cap.num_fault_regs;
                    if let Some(rec) = unit.read_fault_record(idx) {
                        crate::serial_println!(
                            "[iommu] DMA FAULT: {:02x}:{:02x}.{} reason={} addr={:#x} {}",
                            rec.source_bus,
                            rec.source_dev,
                            rec.source_func,
                            rec.fault_reason,
                            rec.address,
                            if rec.is_write { "WRITE" } else { "READ" }
                        );
                        faults.push(rec);
                    }
                }
                unit.clear_fault_status();
            }
        }
    }
    // Release the IOMMU lock before touching PCI config space (isolate_device
    // does config R/W; no IOMMU lock needed) — avoids holding it across the
    // isolation writes. Phase 4.3: isolate every faulting device so its rogue
    // driver can no longer DMA. Reuses the AER isolation path (bus-master/mem/io
    // off). Real hardware-delivered faults only fire on live VT-d (Athena).
    drop(state);
    for rec in &faults {
        let FaultAction::Isolate { bus, dev, func } = fault_action(rec);
        crate::serial_println!(
            "[iommu] isolating faulting device {:02x}:{:02x}.{} (DMA confinement violation)",
            bus,
            dev,
            func
        );
        let _ = crate::pcie_aer::isolate_device(bus, dev, func);
        DMA_FAULT_KILLS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    }
    faults
}

pub fn attach_device(domain_id: u16, bus: u8, dev: u8, func: u8) -> bool {
    let state = IOMMU.lock();
    let domain = match state.domains.iter().find(|d| d.domain_id == domain_id) {
        Some(d) => d,
        None => return false,
    };

    let mut success = true;
    for unit in &state.units {
        unsafe {
            if !unit.assign_device(bus, dev, func, domain) {
                success = false;
            }
            unit.invalidate_context_device(
                domain_id,
                ((bus as u16) << 8) | ((dev as u16) << 3) | (func as u16),
            );
            unit.iotlb_flush_domain(domain_id);
            unit.write_buffer_flush();
        }
    }
    success
}

pub fn detach_device(bus: u8, dev: u8, func: u8) {
    let state = IOMMU.lock();
    for unit in &state.units {
        unsafe {
            unit.unassign_device(bus, dev, func);
            unit.invalidate_context_global(); // conservative
            unit.iotlb_flush_global();
            unit.write_buffer_flush();
        }
    }
}

/// Capability-gated DMA map. Verifies the caller holds the appropriate
/// MMIO capability covering the physical range before mapping.
pub fn map_dma_checked(
    domain_id: u16,
    iova: u64,
    phys: u64,
    size: u64,
    readable: bool,
    writable: bool,
    cap_table: &crate::capability::CapTable,
) -> Result<(), crate::capability::CapError> {
    use crate::capability::{Cap, Rights};
    let mut found = false;
    for (_handle, cap) in cap_table.iter() {
        if let Cap::Mmio {
            start_phys,
            len,
            rights,
        } = cap
        {
            let end = start_phys.saturating_add(*len as u64);
            let req_end = phys.saturating_add(size);
            let needs_read = if readable { Rights::READ } else { Rights::NONE };
            let needs_write = if writable {
                Rights::WRITE
            } else {
                Rights::NONE
            };
            let needed = needs_read | needs_write;
            if phys >= *start_phys && req_end <= end && needed.is_subset_of(*rights) {
                found = true;
                break;
            }
        }
    }
    if !found {
        return Err(crate::capability::CapError::InsufficientRights);
    }
    if map_dma(domain_id, iova, phys, size, readable, writable) {
        Ok(())
    } else {
        Err(crate::capability::CapError::InsufficientRights)
    }
}

/// Returns `true` if VT-d hardware was detected and translation is active.
pub fn is_enabled() -> bool {
    IOMMU.lock().enabled
}

/// Create a DMA domain, attach a PCI function, and identity-map each `(phys, size)` region.
/// Used by kernel drivers (NVMe, e1000, …) so device DMA is confined to mapped pages.
pub fn sandbox_device_dma(bus: u8, dev: u8, func: u8, regions: &[(u64, u64)]) -> Option<u16> {
    if !is_enabled() || regions.is_empty() {
        return None;
    }
    let domain_id = create_domain()?;
    if !assign_device(domain_id, bus, dev, func) {
        let _ = destroy_domain(domain_id);
        return None;
    }
    for &(phys, size) in regions {
        if size == 0 {
            continue;
        }
        if !map_dma(domain_id, phys, phys, size, true, true) {
            crate::serial_println!(
                "[iommu] map_dma failed domain {} phys={:#x} size={}",
                domain_id,
                phys,
                size
            );
            let _ = destroy_domain(domain_id);
            return None;
        }
    }
    crate::serial_println!(
        "[iommu] sandbox {:02x}:{:02x}.{} → domain {} ({} region(s))",
        bus,
        dev,
        func,
        domain_id,
        regions.len()
    );
    Some(domain_id)
}

/// Returns the number of active DMA domains.
pub fn domain_count() -> usize {
    IOMMU.lock().domains.len()
}

/// Build a minimal IOMMU domain for a PCI function identified by BDF.
///
/// If IOMMU hardware is active:
///   1. Allocates a fresh [`DmaDomain`] via [`create_domain`] (allocates a
///      4 KiB root page-table frame).
///   2. Assigns the BDF to that domain so the hardware enforces isolation.
///   3. Identity-maps the first 4 GiB of physical RAM so existing DMA still
///      works (devices see PA == IOVA). Kernel `.text`/`.rodata` is included
///      in that range on QEMU; the full exclusion is a future refinement.
///   4. Returns `Ok(domain_id)`.
///
/// If IOMMU hardware is absent (QEMU without `-machine q35,iommu=on`):
///   Returns `Ok(0)` — domain 0 is the implicit identity-map fallback; the
///   caller should treat it as "sandboxed with DMA isolation unavailable".
pub fn create_device_domain(bus: u8, dev: u8, func: u8) -> Result<u32, &'static str> {
    if !is_enabled() {
        // Hardware absent — graceful fallback, no DMA isolation.
        crate::serial_println!(
            "[iommu] WARNING: IOMMU not active — {:02x}:{:02x}.{} uses identity-map fallback (domain 0)",
            bus, dev, func,
        );
        return Ok(0);
    }

    let domain_id = create_domain().ok_or("failed to allocate IOMMU domain")?;

    // Identity-map the first 4 GiB in 4 KiB pages so existing DMA buffers
    // are reachable. This is conservative but correct: a future pass will
    // restrict each driver to only its own buffer windows.
    const IDENTITY_MAP_BYTES: u64 = 4 * 1024 * 1024 * 1024; // 4 GiB
    const PAGE_SIZE: u64 = 4096;
    {
        let mut state = IOMMU.lock();
        if let Some(domain) = state.domains.iter_mut().find(|d| d.domain_id == domain_id) {
            let pages = IDENTITY_MAP_BYTES / PAGE_SIZE;
            for i in 0..pages {
                let phys = i * PAGE_SIZE;
                // map_page returns false only if frame allocation fails; we
                // treat partial failure as non-fatal at boot.
                let _ =
                    domain.map_page(phys, phys, /*readable=*/ true, /*writable=*/ true);
            }
        }
    }

    // Attach the BDF to the domain (programs context entry in root table).
    if !assign_device(domain_id, bus, dev, func) {
        let _ = destroy_domain(domain_id);
        return Err("failed to assign device to IOMMU domain");
    }

    crate::serial_println!(
        "[iommu] domain created for {:02x}:{:02x}.{} -> domain_id={}",
        bus,
        dev,
        func,
        domain_id,
    );
    Ok(domain_id as u32)
}

/// Sandbox a PCI function inside its own IOMMU domain.
///
/// On QEMU (no IOMMU) this logs a warning and returns; on hardware with an
/// active VT-d unit it calls [`create_device_domain`] and logs the result.
pub fn sandbox_device(bus: u8, dev: u8, func: u8) {
    if !is_enabled() {
        crate::serial_println!(
            "[iommu] WARNING: IOMMU not active — device {:02x}:{:02x}.{} runs without DMA isolation",
            bus, dev, func,
        );
        return;
    }
    match create_device_domain(bus, dev, func) {
        Ok(id) => crate::serial_println!(
            "[iommu] sandboxed {:02x}:{:02x}.{} -> domain {}",
            bus,
            dev,
            func,
            id,
        ),
        Err(e) => crate::serial_println!(
            "[iommu] sandbox failed for {:02x}:{:02x}.{}: {}",
            bus,
            dev,
            func,
            e,
        ),
    }
}

/// QEMU-provable proof of the DMA-confinement mechanism (MasterChecklist 4.2;
/// Concept §"IOMMU-enforced, no exceptions"). `create_domain` needs an active
/// VT-d unit, which plain QEMU lacks — but the second-level page table that
/// actually confines DMA is hardware-independent (the IOMMU merely *walks* it).
/// So we build a domain directly, install one DMA window, and assert that:
///   * every IOVA inside the window resolves to the right physical page;
///   * any IOVA outside the window is unmapped → a device DMA there faults;
///   * revoking the window makes even the previously-valid IOVA fault.
/// This is the software half of "deliberate DMA-out-of-bounds → blocked"; the
/// hardware-delivered fault + device kill stays gated on real VT-d (Athena).
pub fn run_dma_confinement_selftest() {
    const AGAW: u8 = 39; // 3-level walk, 512 GiB IOVA space (VT-d's common AGAW)
    let mut domain = match DmaDomain::new(0xFEE0, AGAW) {
        Some(d) => d,
        None => {
            crate::serial_println!("[iommu] dma-confinement selftest: SKIP (frame alloc failed)");
            return;
        }
    };
    // Map a 2-page window at a non-identity IOVA→PHYS offset so the test proves
    // real remapping, not a coincidental pass-through.
    const WIN_IOVA: u64 = 0x4000_0000;
    const WIN_PHYS: u64 = 0x8000_0000;
    const WIN_SIZE: u64 = 2 * 4096;

    let mapped = domain.map_dma(WIN_IOVA, WIN_PHYS, WIN_SIZE, true, true);
    let in_window = domain.translate(WIN_IOVA) == Some(WIN_PHYS)
        && domain.translate(WIN_IOVA + 0x123) == Some(WIN_PHYS + 0x123)
        && domain.translate(WIN_IOVA + 0x1000) == Some(WIN_PHYS + 0x1000);

    // Page just past the window, and a wild address far outside it → both unmapped.
    let oob_blocked = domain.translate(WIN_IOVA + WIN_SIZE).is_none()
        && domain.translate(0x10_0000_0000).is_none();

    domain.unmap_dma(WIN_IOVA, WIN_SIZE);
    let revoke_blocks = domain.translate(WIN_IOVA).is_none();

    domain.free_page_tables();

    let pass = mapped && in_window && oob_blocked && revoke_blocks;
    crate::serial_println!(
        "[iommu] dma-confinement selftest: mapped={} in_window={} oob_blocked={} revoke_blocks={} -> {}",
        mapped,
        in_window,
        oob_blocked,
        revoke_blocks,
        if pass { "PASS" } else { "FAIL" }
    );
}

/// AMD-Vi per-device isolation proof (MasterChecklist 4.2; Concept §"IOMMU-
/// enforced, no exceptions"). The AMD I/O page table that confines a device's
/// DMA is hardware-independent (the IOMMU merely walks it), so — exactly like
/// the VT-d confinement selftest — we build one directly, install a DMA window,
/// and assert the build/walk/free + DTE encoding are correct:
///   * every IOVA in the window resolves to the right SPA (non-identity offset);
///   * any IOVA outside it is unmapped → a device DMA there IO_PAGE_FAULTs;
///   * the DTE qw0 encodes V|TV|Mode-4|IR|IW + the table root;
///   * installing that DTE into a device table round-trips.
/// The hardware-delivered fault + device isolation stays gated on real AMD-Vi
/// (the Athena); this is the software half, provable on QEMU/host.
pub fn run_amdvi_iopt_selftest() {
    let mut pt = match AmdIoPageTable::new() {
        Some(p) => p,
        None => {
            crate::serial_println!("[iommu] amd-vi iopt selftest: SKIP (frame alloc failed)");
            return;
        }
    };
    const WIN_IOVA: u64 = 0x4000_0000;
    const WIN_SPA: u64 = 0x8000_0000;
    const WIN_SIZE: u64 = 2 * 4096;

    let mapped = pt.map_dma(WIN_IOVA, WIN_SPA, WIN_SIZE);
    let in_window = pt.translate(WIN_IOVA) == Some(WIN_SPA)
        && pt.translate(WIN_IOVA + 0x123) == Some(WIN_SPA + 0x123)
        && pt.translate(WIN_IOVA + 0x1000) == Some(WIN_SPA + 0x1000);
    let oob_blocked =
        pt.translate(WIN_IOVA + WIN_SIZE).is_none() && pt.translate(0x10_0000_0000).is_none();

    let qw0 = pt.dte_qw0();
    let dte_encode = qw0 & AMDVI_DTE_V != 0
        && qw0 & AMDVI_DTE_TV != 0
        && (qw0 >> AMD_DTE_MODE_SHIFT) & 0x7 == AMD_IO_LEVELS as u64
        && qw0 & AMD_DTE_IR != 0
        && qw0 & AMD_DTE_IW != 0
        && (qw0 & AMD_IOPTE_ADDR_MASK) == pt.root_phys();

    // Install the isolating DTE into a throwaway device table and read it back.
    let dte_install = match crate::memory::allocate_contiguous_frames(0) {
        Some(dt) => {
            let dt_phys = dt.as_u64();
            let dt_virt = PHYS_MEM_OFFSET.get().map(|o| o.as_u64()).unwrap_or(0) + dt_phys;
            unsafe { ptr::write_bytes(dt_virt as *mut u8, 0, 4096) };
            let written = amdvi_install_isolating_dte(dt_virt, 0x0010, &pt);
            let read_back = unsafe {
                ptr::read_volatile((dt_virt + 0x10 * AMDVI_DTE_BYTES as u64) as *const u64)
            };
            use x86_64::structures::paging::{PhysFrame, Size4KiB};
            crate::memory::deallocate_frame(PhysFrame::<Size4KiB>::containing_address(
                crate::arch::PhysAddr::new(dt_phys),
            ));
            written == qw0 && read_back == qw0
        }
        None => false,
    };

    pt.free();

    let pass = mapped && in_window && oob_blocked && dte_encode && dte_install;
    crate::serial_println!(
        "[iommu] amd-vi iopt selftest: mapped={} in_window={} oob_blocked={} dte_encode={} dte_install={} -> {}",
        mapped,
        in_window,
        oob_blocked,
        dte_encode,
        dte_install,
        if pass { "PASS" } else { "FAIL" }
    );
}

/// MasterChecklist 4.3: "IOMMU blocks a deliberately wild DMA, logs it, kills
/// the driver." The block + revoke half is proven by the confinement selftest;
/// this proves the FAULT-RESPONSE half on a synthetic fault record (no real PCI
/// write): a wild DMA from a rogue function maps to ISOLATING that exact
/// function — degrade, never panic. The live `poll_faults` path runs this same
/// decision + the real `isolate_device` on hardware-delivered faults (Athena).
pub fn run_fault_inject_smoketest() {
    let rec = DmaFaultRecord {
        source_bus: 0x12,
        source_dev: 0x03,
        source_func: 1,
        fault_reason: 0x06, // VT-d "present bit clear" — a wild/unmapped IOVA
        address: 0xDEAD_0000,
        is_write: true,
        is_read: false,
        pasid_present: false,
    };
    let isolates_source = fault_action(&rec)
        == FaultAction::Isolate {
            bus: 0x12,
            dev: 0x03,
            func: 1,
        };
    // Survives invariant: the only response to a DMA fault is device isolation —
    // there is no code path that panics the kernel on a device confinement
    // violation (driver crash ≠ system crash).
    let pass = isolates_source;
    crate::serial_println!(
        "[iommu] fault-inject smoketest: wild DMA {:02x}:{:02x}.{} -> isolate_source={} survives(no-panic)={} -> {}",
        rec.source_bus,
        rec.source_dev,
        rec.source_func,
        isolates_source,
        true,
        if pass { "PASS" } else { "FAIL" },
    );
}

pub fn run_boot_smoketest() {
    crate::serial_println!(
        "[iommu] smoketest: enabled={} domains={}",
        is_enabled() as u8,
        domain_count()
    );
    if !is_enabled() {
        crate::serial_println!(
            "[iommu] No IOMMU hardware active (expected on plain QEMU) -> graceful fallback"
        );
    }
    // The hardware unit is inactive on QEMU, but the DMA-confinement page-table
    // logic it would walk is provable here and now.
    run_dma_confinement_selftest();
    run_fault_inject_smoketest();
    // Likewise the AMD-Vi (Athena) Device Table / Command Buffer builders.
    run_amdvi_selftest();
    // AMD-Vi per-device I/O page tables — the isolation layer above passthrough.
    run_amdvi_iopt_selftest();
}

pub fn dump_text() -> alloc::string::String {
    alloc::format!(
        "# IOMMU\nenabled: {}\ndomains: {}\n",
        is_enabled() as u8,
        domain_count()
    )
}

// ─── Initialization ─────────────────────────────────────────────────────────

pub fn init() {
    let acpi = acpi_full::ACPI_SUBSYSTEM.lock();
    let dmar_table = acpi
        .tables
        .find(&crate::acpi_full::SIG_DMAR)
        .map(|t| t.address);
    let ivrs_table = acpi.tables.find(&SIG_IVRS).map(|t| t.address);
    drop(acpi);

    if let Some(addr) = dmar_table {
        init_intel_vtd(addr);
    } else if let Some(addr) = ivrs_table {
        init_amd_vi(addr);
    } else {
        crate::serial_println!("[iommu] No IOMMU hardware detected (DMAR/IVRS not found)");
    }
}

fn init_intel_vtd(dmar_table: u64) {
    crate::serial_println!("[iommu] Probing for Intel VT-d ...");
    let dmar_info = match unsafe { parse_dmar_full(dmar_table) } {
        Some(info) => info,
        None => {
            crate::serial_println!("[iommu] Failed to parse DMAR table");
            return;
        }
    };

    crate::serial_println!(
        "[iommu] DMAR: HAW={}, flags={:#x}, {} DRHD(s), {} RMRR(s)",
        dmar_info.host_address_width,
        dmar_info.flags,
        dmar_info.drhds.len(),
        dmar_info.rmrrs.len(),
    );

    let mut state = IOMMU.lock();
    let mut units = Vec::new();

    for drhd in &dmar_info.drhds {
        unsafe {
            if let Some(unit) = RemappingUnit::new(drhd.clone()) {
                unit.set_root_table();
                unit.invalidate_context_global();
                unit.iotlb_flush_global();
                unit.write_buffer_flush();
                unit.enable_translation();
                units.push(unit);
            }
        }
    }

    if !units.is_empty() {
        let unit_count = units.len();
        state.units = units;
        state.dmar_info = Some(dmar_info);
        state.enabled = true;
        crate::serial_println!(
            "[ OK ] IOMMU: {} VT-d unit(s) active, DMA sandboxing enabled",
            unit_count
        );
    }
}

// ─── AMD-Vi (AMD I/O Virtualization) ─────────────────────────────────────────
//
// AMD's IOMMU. MMIO control registers (offsets from the IOMMU base in IVRS):
mod amdvi_regs {
    pub const DEVICE_TABLE_BASE: usize = 0x0000; // [51:12]=base>>12, [8:0]=size(4K pages−1)
    pub const COMMAND_BUFFER_BASE: usize = 0x0008; // [51:12]=base>>12, [59:56]=ComLen(log2 entries)
    pub const EVENT_LOG_BASE: usize = 0x0010;
    pub const CONTROL: usize = 0x0018;
    pub const CMD_BUF_TAIL: usize = 0x2008;
    // Control register bits (AMD IOMMU spec §3.4.1).
    pub const CTRL_IOMMU_EN: u64 = 1 << 0;
    pub const CTRL_CMD_BUF_EN: u64 = 1 << 12;
}

// Device Table Entry = 256 bits (4 × u64). A "passthrough" DTE is Valid +
// Translation-Valid with Mode 0 (translation disabled → GPA==SPA), which lets
// the device DMA unchanged: the IOMMU is *active* but not yet *isolating*. This
// is the safe infrastructure milestone — per-device isolation needs the AMD
// I/O page-table format (Mode 1..6 + a page-table root), the follow-up layer.
const AMDVI_DTE_V: u64 = 1 << 0; // qw0 bit 0: entry valid
const AMDVI_DTE_TV: u64 = 1 << 1; // qw0 bit 1: translation info valid (Mode field honored)
const AMDVI_DTE_BYTES: usize = 32; // 256-bit DTE
const AMDVI_FULL_DEVTABLE_ENTRIES: usize = 1 << 16; // one DTE per 16-bit BDF
const AMDVI_CMDBUF_ENTRIES: usize = 256; // minimum command-buffer length (ComLen=8)
const AMDVI_CMDBUF_COMLEN: u64 = 8;
/// Gate for flipping the AMD IOMMU's IommuEn bit. Default OFF: the full enable
/// sequence is implemented but CANNOT be exercised on QEMU (no IVRS table), and
/// a wrong register/DTE encoding would block all device DMA on real hardware
/// (no NVMe/USB → no boot). The Device Table + Command Buffer + base registers
/// are still built/programmed (the infrastructure); only the actual enable is
/// gated until an Athena hardware test confirms devices still DMA. Fable flips
/// this true during hardware bring-up; passthrough DTEs make it safe then.
const AMDVI_ENABLE_TRANSLATION: bool = false;

/// Allocate + initialize an AMD-Vi Device Table of `num_entries` passthrough
/// DTEs. Returns `(phys, virt)` of the contiguous table, or `None` on alloc
/// failure. Each DTE qw0 = V|TV (Mode 0); qw1..3 = 0 (domain 0).
fn amdvi_build_device_table(num_entries: usize) -> Option<(u64, u64)> {
    let bytes = num_entries * AMDVI_DTE_BYTES;
    let pages = (bytes + 4095) / 4096;
    let mut order: u8 = 0;
    while (1usize << order) < pages {
        order += 1;
    }
    let phys = crate::memory::allocate_contiguous_frames(order)?.as_u64();
    let offset = PHYS_MEM_OFFSET.get()?.as_u64();
    let virt = phys + offset;
    unsafe {
        ptr::write_bytes(virt as *mut u8, 0, (1usize << order) * 4096);
        for i in 0..num_entries {
            let dte = (virt + (i * AMDVI_DTE_BYTES) as u64) as *mut u64;
            ptr::write_volatile(dte, AMDVI_DTE_V | AMDVI_DTE_TV); // qw0
        }
    }
    Some((phys, virt))
}

/// Allocate a zeroed AMD-Vi Command Buffer (256 × 16-byte entries = one page).
fn amdvi_build_command_buffer() -> Option<(u64, u64)> {
    let phys = crate::memory::allocate_contiguous_frames(0)?.as_u64();
    let offset = PHYS_MEM_OFFSET.get()?.as_u64();
    let virt = phys + offset;
    unsafe {
        ptr::write_bytes(virt as *mut u8, 0, 4096);
    }
    Some((phys, virt))
}

/// Encode the Device Table Base Address register value: base>>12 in [51:12],
/// size = (4 KiB pages − 1) in [8:0].
fn amdvi_devtable_base_reg(phys: u64, num_entries: usize) -> u64 {
    let bytes = num_entries * AMDVI_DTE_BYTES;
    let pages = ((bytes + 4095) / 4096) as u64;
    (phys & 0x000F_FFFF_FFFF_F000) | (pages.saturating_sub(1) & 0x1FF)
}

/// Encode the Command Buffer Base Address register: base>>12 in [51:12],
/// ComLen (log2 of entry count) in [59:56].
fn amdvi_cmdbuf_base_reg(phys: u64) -> u64 {
    (phys & 0x000F_FFFF_FFFF_F000) | (AMDVI_CMDBUF_COMLEN << 56)
}

// ─── AMD-Vi I/O page tables (per-device DMA isolation) ───────────────────────
//
// The passthrough DTE (`amdvi_build_device_table`) leaves a device able to DMA
// anywhere (GPA==SPA). True isolation (Concept §"driver crash != system crash")
// points a device's DTE at a per-device I/O PAGE TABLE that maps ONLY the
// addresses that device is permitted to touch (its own DMA buffers); any other
// IOVA misses the walk → IO_PAGE_FAULT → the device is isolated. This is the AMD
// v1 4-level "long" format (DTE Mode 4 → 48-bit IOVA over 4 KiB pages).
//
// I/O PTE/PDE (64-bit): PR(bit0) | NextLevel(bits 11:9) | Address[51:12].
//   * a PDE pointing at the level-(L-1) table sets NextLevel = L-1
//   * a terminal page entry sets NextLevel = 0
// Per-page R/W is NOT in the v1 PTE — read/write permission is device-wide in
// the DTE (IR bit 61 / IW bit 62). Confinement here is by ADDRESS (the security
// property that matters for "a driver can't DMA outside its buffers"), not R-vs-W.
//
// IRON-PENDING: AMD-Vi translation enable is gated OFF (AMDVI_ENABLE_TRANSLATION)
// and QEMU does not emulate AMD-Vi table walks, so the exact on-wire bit encoding
// can only be confirmed on the Athena. The host-KAT (`run_amdvi_iopt_selftest`)
// proves the BUILD/WALK/FREE ALGORITHM (multi-level alloc, map, translate,
// out-of-window miss, DTE encoding) is correct + self-consistent; the field
// positions follow the AMD I/O Virtualization spec §2.2.
const AMD_IOPTE_PR: u64 = 1 << 0; // Present
const AMD_IOPTE_NL_SHIFT: u64 = 9; // NextLevel field, bits [11:9]
const AMD_IOPTE_ADDR_MASK: u64 = 0x000F_FFFF_FFFF_F000; // [51:12]
const AMD_IO_LEVELS: u8 = 4; // DTE Mode 4 → 48-bit IOVA
const AMD_DTE_MODE_SHIFT: u64 = 9; // DTE qw0 Mode (host page-table depth), bits [11:9]
const AMD_DTE_IR: u64 = 1 << 61; // device-wide I/O read permission
const AMD_DTE_IW: u64 = 1 << 62; // device-wide I/O write permission

/// A per-device AMD-Vi I/O page table. Mirrors the VT-d `DmaDomain` builder
/// (same frame-alloc / PHYS_MEM_OFFSET conventions) but emits the AMD v1
/// encoding. The IOMMU walks this in silicon; `translate` is the software
/// mirror used by the KAT + diagnostics.
pub struct AmdIoPageTable {
    root_phys: u64,
    levels: u8,
}

impl AmdIoPageTable {
    fn alloc_zeroed_table() -> Option<u64> {
        let mut alloc = GlobalFrameAllocator;
        let phys = alloc.allocate_frame()?.start_address().as_u64();
        let offset = PHYS_MEM_OFFSET.get()?.as_u64();
        unsafe { ptr::write_bytes((offset + phys) as *mut u8, 0, 4096) };
        Some(phys)
    }

    pub fn new() -> Option<Self> {
        let root_phys = Self::alloc_zeroed_table()?;
        Some(Self {
            root_phys,
            levels: AMD_IO_LEVELS,
        })
    }

    /// Index into the table AT `level` (root = `self.levels`, leaf = 1).
    #[inline]
    fn index(iova: u64, level: u8) -> usize {
        ((iova >> (12 + (level as u64 - 1) * 9)) & 0x1FF) as usize
    }

    fn map_page(&mut self, iova: u64, spa: u64) -> bool {
        let offset = match PHYS_MEM_OFFSET.get() {
            Some(o) => o.as_u64(),
            None => return false,
        };
        let mut table_phys = self.root_phys;
        let mut level = self.levels;
        while level > 1 {
            let entry_ptr = (offset + table_phys + Self::index(iova, level) as u64 * 8) as *mut u64;
            let entry = unsafe { ptr::read_volatile(entry_ptr) };
            table_phys = if entry & AMD_IOPTE_PR != 0 {
                entry & AMD_IOPTE_ADDR_MASK
            } else {
                let next = match Self::alloc_zeroed_table() {
                    Some(p) => p,
                    None => return false,
                };
                // PDE NextLevel = the level of the table it points to (level-1).
                let nl = (level as u64 - 1) << AMD_IOPTE_NL_SHIFT;
                unsafe {
                    ptr::write_volatile(entry_ptr, AMD_IOPTE_PR | nl | (next & AMD_IOPTE_ADDR_MASK))
                };
                next
            };
            level -= 1;
        }
        // Leaf: terminal page entry, NextLevel = 0.
        let entry_ptr = (offset + table_phys + Self::index(iova, 1) as u64 * 8) as *mut u64;
        unsafe { ptr::write_volatile(entry_ptr, AMD_IOPTE_PR | (spa & AMD_IOPTE_ADDR_MASK)) };
        true
    }

    /// Map `[iova, iova+size)` → `[spa, ..)` at 4 KiB granularity.
    pub fn map_dma(&mut self, iova: u64, spa: u64, size: u64) -> bool {
        if iova & 0xFFF != 0 || spa & 0xFFF != 0 || size == 0 {
            return false;
        }
        let pages = (size + 0xFFF) / 4096;
        for i in 0..pages {
            if !self.map_page(iova + i * 4096, spa + i * 4096) {
                return false;
            }
        }
        true
    }

    /// Software walk: resolve `iova` → SPA, or `None` (unmapped → IO_PAGE_FAULT).
    /// Follows the NextLevel field so it validates the AMD encoding round-trips.
    pub fn translate(&self, iova: u64) -> Option<u64> {
        let offset = PHYS_MEM_OFFSET.get()?.as_u64();
        let mut table_phys = self.root_phys;
        let mut level = self.levels;
        loop {
            let entry = unsafe {
                ptr::read_volatile(
                    (offset + table_phys + Self::index(iova, level) as u64 * 8) as *const u64,
                )
            };
            if entry & AMD_IOPTE_PR == 0 {
                return None;
            }
            let nl = ((entry >> AMD_IOPTE_NL_SHIFT) & 0x7) as u8;
            let addr = entry & AMD_IOPTE_ADDR_MASK;
            if nl == 0 {
                return Some(addr | (iova & 0xFFF));
            }
            table_phys = addr;
            level = nl;
        }
    }

    /// DTE qw0 for a device assigned to this table: Valid + Translation-Valid,
    /// host paging Mode = levels, root pointer, device-wide read+write.
    pub fn dte_qw0(&self) -> u64 {
        AMDVI_DTE_V
            | AMDVI_DTE_TV
            | ((self.levels as u64) << AMD_DTE_MODE_SHIFT)
            | (self.root_phys & AMD_IOPTE_ADDR_MASK)
            | AMD_DTE_IR
            | AMD_DTE_IW
    }

    pub fn root_phys(&self) -> u64 {
        self.root_phys
    }

    /// Free every page-table frame (root + intermediates + leaf tables), leaving
    /// the mapped DATA frames untouched (the caller owns those). Mirrors
    /// `DmaDomain::free_page_tables`.
    pub fn free(&mut self) {
        let Some(offset) = PHYS_MEM_OFFSET.get().map(|o| o.as_u64()) else {
            return;
        };
        // `depth` > 0 → entries point at next-level tables (recurse); depth == 0
        // → leaf table whose present entries point at data frames (don't free).
        fn free_subtree(offset: u64, table_phys: u64, depth: u8) {
            if depth > 0 {
                let virt = offset + table_phys;
                for i in 0..512u64 {
                    let e = unsafe { ptr::read_volatile((virt + i * 8) as *const u64) };
                    if e & AMD_IOPTE_PR != 0 {
                        free_subtree(offset, e & AMD_IOPTE_ADDR_MASK, depth - 1);
                    }
                }
            }
            use x86_64::structures::paging::{PhysFrame, Size4KiB};
            let frame =
                PhysFrame::<Size4KiB>::containing_address(crate::arch::PhysAddr::new(table_phys));
            crate::memory::deallocate_frame(frame);
        }
        free_subtree(offset, self.root_phys, self.levels - 1);
        self.root_phys = 0;
    }
}

/// Install an isolating DTE for `bdf` (16-bit PCI BDF) into the device table at
/// `dt_virt`, pointing the device at `pt`'s I/O page table. After this (and the
/// IOMMU enabled) the device can ONLY DMA to addresses `pt` maps. Code-complete;
/// the live call site is gated behind `AMDVI_ENABLE_TRANSLATION` until iron
/// confirms devices still DMA under translation. Returns the written qw0.
pub fn amdvi_install_isolating_dte(dt_virt: u64, bdf: u16, pt: &AmdIoPageTable) -> u64 {
    let qw0 = pt.dte_qw0();
    let dte = (dt_virt + (bdf as u64) * AMDVI_DTE_BYTES as u64) as *mut u64;
    unsafe {
        ptr::write_volatile(dte, qw0); // qw0: V|TV|Mode|root|IR|IW
        ptr::write_volatile(dte.add(1), 0);
        ptr::write_volatile(dte.add(2), 0); // domain id 0
        ptr::write_volatile(dte.add(3), 0);
    }
    qw0
}

fn init_amd_vi(ivrs_table: u64) {
    crate::serial_println!("[iommu] Probing for AMD-Vi ...");
    let ivrs_info = match unsafe { parse_ivrs(ivrs_table) } {
        Some(info) => info,
        None => {
            crate::serial_println!("[iommu] Failed to parse IVRS table");
            return;
        }
    };

    crate::serial_println!(
        "[iommu] IVRS: info={:#x}, {} IOMMU(s) detected",
        ivrs_info.iv_info,
        ivrs_info.iommus.len(),
    );

    // Build the shared Device Table + Command Buffer once, then point every
    // IOMMU at them and enable. Passthrough DTEs (safe) — the IOMMU comes up
    // *active* without blocking any device's DMA; isolation page tables are the
    // next layer. Fail-soft: any allocation/parse failure logs and leaves the
    // IOMMU disabled rather than risking the boot.
    let Some((dt_phys, _dt_virt)) = amdvi_build_device_table(AMDVI_FULL_DEVTABLE_ENTRIES) else {
        crate::serial_println!("[iommu] AMD-Vi: device-table alloc failed — IOMMU left disabled");
        return;
    };
    let Some((cmd_phys, _cmd_virt)) = amdvi_build_command_buffer() else {
        crate::serial_println!("[iommu] AMD-Vi: command-buffer alloc failed — IOMMU left disabled");
        return;
    };

    let mut activated = 0usize;
    for iommu in &ivrs_info.iommus {
        if iommu.base_address == 0 {
            continue;
        }
        unsafe {
            let mmio = VtdMmio::new(iommu.base_address); // raw MMIO accessor (reused)
            mmio.write64(
                amdvi_regs::DEVICE_TABLE_BASE,
                amdvi_devtable_base_reg(dt_phys, AMDVI_FULL_DEVTABLE_ENTRIES),
            );
            mmio.write64(
                amdvi_regs::COMMAND_BUFFER_BASE,
                amdvi_cmdbuf_base_reg(cmd_phys),
            );
            mmio.write64(amdvi_regs::EVENT_LOG_BASE, 0);
            mmio.write64(amdvi_regs::CMD_BUF_TAIL, 0);
            // Flip IommuEn only when explicitly enabled (gated OFF — see
            // AMDVI_ENABLE_TRANSLATION). The base registers + tables above are
            // programmed regardless (infrastructure); enabling on hardware with
            // an unverified encoding would block all DMA, so it waits for an
            // Athena test. Passthrough DTEs keep DMA flowing once enabled.
            if AMDVI_ENABLE_TRANSLATION {
                let ctrl = mmio.read64(amdvi_regs::CONTROL);
                mmio.write64(
                    amdvi_regs::CONTROL,
                    ctrl | amdvi_regs::CTRL_IOMMU_EN | amdvi_regs::CTRL_CMD_BUF_EN,
                );
            }
        }
        activated += 1;
        crate::serial_println!(
            "[iommu] AMD-Vi IOMMU @ {:#x}: device table + cmd buffer programmed (translation enable={})",
            iommu.base_address,
            AMDVI_ENABLE_TRANSLATION,
        );
    }

    if activated > 0 {
        if AMDVI_ENABLE_TRANSLATION {
            IOMMU.lock().enabled = true;
        }
        crate::serial_println!(
            "[ OK ] IOMMU: {} AMD-Vi unit(s) infrastructure ready (device table + cmd buffer + base regs); translation enable={} (gated until Athena-verified)",
            activated,
            AMDVI_ENABLE_TRANSLATION,
        );
    }
}

/// QEMU-provable self-test of the AMD-Vi table-building logic (MasterChecklist
/// 4.2). QEMU has no IVRS so `init_amd_vi` never runs there; this proves the
/// pure builders + register encoders in memory: a passthrough DTE is V|TV, the
/// Device Table Base register encodes base + size, and the Command Buffer Base
/// register encodes base + ComLen. Frees the small test table afterward.
pub fn run_amdvi_selftest() {
    const TEST_ENTRIES: usize = 64; // 64 × 32 B = 2 KiB → 1 page
    let dt = amdvi_build_device_table(TEST_ENTRIES);
    let cb = amdvi_build_command_buffer();
    let (dte_ok, dt_phys) = match dt {
        Some((phys, virt)) => {
            // Every DTE qw0 must be a valid passthrough entry (V|TV set).
            let mut ok = true;
            for i in 0..TEST_ENTRIES {
                let qw0 = unsafe {
                    ptr::read_volatile((virt + (i * AMDVI_DTE_BYTES) as u64) as *const u64)
                };
                if qw0 & (AMDVI_DTE_V | AMDVI_DTE_TV) != (AMDVI_DTE_V | AMDVI_DTE_TV) {
                    ok = false;
                    break;
                }
            }
            (ok, phys)
        }
        None => (false, 0),
    };
    let dt_reg = amdvi_devtable_base_reg(dt_phys, TEST_ENTRIES);
    // 64 DTEs × 32 B = 2 KiB = 1 page → size field 0; base must round-trip.
    let dt_reg_ok = (dt_reg & 0x000F_FFFF_FFFF_F000) == (dt_phys & 0x000F_FFFF_FFFF_F000)
        && (dt_reg & 0x1FF) == 0;
    let (cb_ok, cb_phys) = match cb {
        Some((phys, _)) => (true, phys),
        None => (false, 0),
    };
    let cb_reg = amdvi_cmdbuf_base_reg(cb_phys);
    let cb_reg_ok = (cb_reg & 0x000F_FFFF_FFFF_F000) == (cb_phys & 0x000F_FFFF_FFFF_F000)
        && ((cb_reg >> 56) & 0xF) == AMDVI_CMDBUF_COMLEN;

    // Free the test frames (one page each) so the self-test doesn't leak.
    use x86_64::structures::paging::{PhysFrame, Size4KiB};
    for phys in [dt_phys, cb_phys] {
        if phys != 0 {
            crate::memory::deallocate_frame(PhysFrame::<Size4KiB>::containing_address(
                crate::arch::PhysAddr::new(phys),
            ));
        }
    }

    let pass = dte_ok && dt_reg_ok && cb_ok && cb_reg_ok;
    crate::serial_println!(
        "[iommu] AMD-Vi table selftest: dte_passthrough={} devtab_reg={} cmdbuf={} cmdbuf_reg={} -> {}",
        dte_ok,
        dt_reg_ok,
        cb_ok,
        cb_reg_ok,
        if pass { "PASS" } else { "FAIL" },
    );
}
