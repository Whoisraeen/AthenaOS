//! Full ACPI subsystem — table parsing, AML bytecode interpreter, namespace,
//! operation regions, power management, thermal, battery, embedded controller,
//! GPE dispatch, and processor performance (C/P/T-states, CPPC).

#![allow(dead_code)]

extern crate alloc;

use alloc::borrow::ToOwned;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

// ─── Error Type ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AcpiError {
    InvalidChecksum,
    ChecksumFailed,
    InvalidSignature,
    TableNotFound([u8; 4]),
    AmlParseError(String),
    AmlEvalError(String),
    MethodNotFound(String),
    InvalidPowerState,
    ThermalOverheat,
    BatteryNotPresent,
    HardwareError(String),
    EcTimeout,
    GpeError,
    NamespaceError(String),
    OpRegionError,
    InvalidTable,
    NotInitialized,
    MethodError,
}

// ─── RSDP (Root System Description Pointer) ──────────────────────────────────

#[derive(Debug, Clone)]
pub struct Rsdp {
    pub signature: [u8; 8],
    pub checksum: u8,
    pub oem_id: [u8; 6],
    pub revision: u8,
    pub rsdt_address: u32,
    pub length: u32,
    pub xsdt_address: u64,
    pub extended_checksum: u8,
    pub is_v2: bool,
}

impl Rsdp {
    pub unsafe fn parse(addr: u64) -> Result<Self, AcpiError> {
        let ptr = addr as *const u8;
        let mut sig = [0u8; 8];
        for i in 0..8 {
            sig[i] = *ptr.add(i);
        }

        if &sig != b"RSD PTR " {
            return Err(AcpiError::InvalidSignature);
        }

        let checksum = *ptr.add(8);
        let mut oem = [0u8; 6];
        for i in 0..6 {
            oem[i] = *ptr.add(9 + i);
        }
        let revision = *ptr.add(15);
        let rsdt = *(ptr.add(16) as *const u32);

        let (length, xsdt, ext_checksum, is_v2) = if revision >= 2 {
            let len = *(ptr.add(20) as *const u32);
            let xsdt_addr = *(ptr.add(24) as *const u64);
            let ext_cs = *ptr.add(32);
            (len, xsdt_addr, ext_cs, true)
        } else {
            (20, 0u64, 0u8, false)
        };

        let mut sum: u8 = 0;
        for i in 0..20 {
            sum = sum.wrapping_add(*ptr.add(i));
        }
        if sum != 0 {
            return Err(AcpiError::InvalidChecksum);
        }

        Ok(Self {
            signature: sig,
            checksum,
            oem_id: oem,
            revision,
            rsdt_address: rsdt,
            length,
            xsdt_address: xsdt,
            extended_checksum: ext_checksum,
            is_v2,
        })
    }
}

// ─── Generic SDT Header ─────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct SdtHeader {
    pub signature: [u8; 4],
    pub length: u32,
    pub revision: u8,
    pub checksum: u8,
    pub oem_id: [u8; 6],
    pub oem_table_id: [u8; 8],
    pub oem_revision: u32,
    pub creator_id: u32,
    pub creator_revision: u32,
}

impl SdtHeader {
    pub const SIZE: usize = 36;

    pub unsafe fn parse(addr: u64) -> Result<Self, AcpiError> {
        let ptr = addr as *const u8;
        let mut sig = [0u8; 4];
        for i in 0..4 {
            sig[i] = *ptr.add(i);
        }
        let length = *(ptr.add(4) as *const u32);
        let revision = *ptr.add(8);
        let checksum = *ptr.add(9);
        let mut oem = [0u8; 6];
        for i in 0..6 {
            oem[i] = *ptr.add(10 + i);
        }
        let mut oem_tbl = [0u8; 8];
        for i in 0..8 {
            oem_tbl[i] = *ptr.add(16 + i);
        }
        let oem_rev = *(ptr.add(24) as *const u32);
        let cid = *(ptr.add(28) as *const u32);
        let crev = *(ptr.add(32) as *const u32);

        let mut sum: u8 = 0;
        for i in 0..length as usize {
            sum = sum.wrapping_add(*ptr.add(i));
        }
        if sum != 0 {
            return Err(AcpiError::InvalidChecksum);
        }

        Ok(Self {
            signature: sig,
            length,
            revision,
            checksum,
            oem_id: oem,
            oem_table_id: oem_tbl,
            oem_revision: oem_rev,
            creator_id: cid,
            creator_revision: crev,
        })
    }
}

// ─── Table Registry ──────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct AcpiTable {
    pub header: SdtHeader,
    pub address: u64,
}

#[derive(Debug)]
pub struct TableRegistry {
    pub tables: Vec<AcpiTable>,
}

impl TableRegistry {
    pub fn new() -> Self {
        Self { tables: Vec::new() }
    }

    pub unsafe fn load_rsdt(&mut self, rsdt_addr: u32, phys_offset: u64) {
        let addr = phys_offset + rsdt_addr as u64;
        if let Ok(hdr) = SdtHeader::parse(addr) {
            if (hdr.length as usize) < SdtHeader::SIZE {
                crate::serial_println!("[acpi][warn] RSDT length {} too small", hdr.length);
                return;
            }
            let entry_count = (hdr.length as usize - SdtHeader::SIZE) / 4;
            let entries = (addr + SdtHeader::SIZE as u64) as *const u32;
            for i in 0..entry_count {
                let table_phys = *entries.add(i) as u64;
                let table_addr = phys_offset + table_phys;
                if let Ok(table_hdr) = SdtHeader::parse(table_addr) {
                    self.tables.push(AcpiTable {
                        header: table_hdr,
                        address: table_addr,
                    });
                }
            }
        }
    }

    pub unsafe fn load_xsdt(&mut self, xsdt_addr: u64, phys_offset: u64) {
        let addr = phys_offset + xsdt_addr;
        if let Ok(hdr) = SdtHeader::parse(addr) {
            if (hdr.length as usize) < SdtHeader::SIZE {
                crate::serial_println!("[acpi][warn] XSDT length {} too small", hdr.length);
                return;
            }
            let entry_count = (hdr.length as usize - SdtHeader::SIZE) / 8;
            let entries = (addr + SdtHeader::SIZE as u64) as *const u64;
            for i in 0..entry_count {
                let table_phys = *entries.add(i);
                let table_addr = phys_offset + table_phys;
                if let Ok(table_hdr) = SdtHeader::parse(table_addr) {
                    self.tables.push(AcpiTable {
                        header: table_hdr,
                        address: table_addr,
                    });
                }
            }
        }
    }

    pub fn find(&self, sig: &[u8; 4]) -> Option<&AcpiTable> {
        self.tables.iter().find(|t| &t.header.signature == sig)
    }

    pub fn find_all(&self, sig: &[u8; 4]) -> Vec<&AcpiTable> {
        self.tables
            .iter()
            .filter(|t| &t.header.signature == sig)
            .collect()
    }
}

// ─── FADT (Fixed ACPI Description Table) ─────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Fadt {
    pub facs_address: u32,
    pub dsdt_address: u32,
    pub preferred_pm_profile: u8,
    pub sci_interrupt: u16,
    pub smi_command: u32,
    pub acpi_enable: u8,
    pub acpi_disable: u8,
    pub s4bios_req: u8,
    pub pstate_control: u8,
    pub pm1a_event_block: u32,
    pub pm1b_event_block: u32,
    pub pm1a_control_block: u32,
    pub pm1b_control_block: u32,
    pub pm2_control_block: u32,
    pub pm_timer_block: u32,
    pub gpe0_block: u32,
    pub gpe1_block: u32,
    pub pm1_event_length: u8,
    pub pm1_control_length: u8,
    pub pm2_control_length: u8,
    pub pm_timer_length: u8,
    pub gpe0_length: u8,
    pub gpe1_length: u8,
    pub gpe1_base: u8,
    pub c_state_control: u8,
    pub worst_c2_latency: u16,
    pub worst_c3_latency: u16,
    pub flush_size: u16,
    pub flush_stride: u16,
    pub duty_offset: u8,
    pub duty_width: u8,
    pub day_alarm: u8,
    pub month_alarm: u8,
    pub century: u8,
    pub iapc_boot_arch: u16,
    pub flags: u32,
    pub reset_register: GenericAddress,
    pub reset_value: u8,
    pub arm_boot_arch: u16,
    pub fadt_minor_version: u8,
    pub x_facs_address: u64,
    pub x_dsdt_address: u64,
    pub x_pm1a_event_block: GenericAddress,
    pub x_pm1b_event_block: GenericAddress,
    pub x_pm1a_control_block: GenericAddress,
    pub x_pm1b_control_block: GenericAddress,
    pub x_pm2_control_block: GenericAddress,
    pub x_pm_timer_block: GenericAddress,
    pub x_gpe0_block: GenericAddress,
    pub x_gpe1_block: GenericAddress,
}

#[derive(Debug, Clone, Copy)]
pub struct GenericAddress {
    pub address_space: u8,
    pub bit_width: u8,
    pub bit_offset: u8,
    pub access_size: u8,
    pub address: u64,
}

impl GenericAddress {
    pub const SYSTEM_MEMORY: u8 = 0;
    pub const SYSTEM_IO: u8 = 1;
    pub const PCI_CONFIG: u8 = 2;
    pub const EMBEDDED_CONTROLLER: u8 = 3;
    pub const SMBUS: u8 = 4;
    pub const FUNCTIONAL_FIXED: u8 = 0x7F;

    pub unsafe fn parse(addr: u64) -> Self {
        let ptr = addr as *const u8;
        Self {
            address_space: *ptr,
            bit_width: *ptr.add(1),
            bit_offset: *ptr.add(2),
            access_size: *ptr.add(3),
            address: *(ptr.add(4) as *const u64),
        }
    }

    pub fn is_valid(&self) -> bool {
        self.address != 0
    }

    pub unsafe fn read_u8(&self, offset: u64) -> u8 {
        match self.address_space {
            Self::SYSTEM_IO => {
                let port = (self.address + offset) as u16;
                let val: u8;
                core::arch::asm!("in al, dx", out("al") val, in("dx") port);
                val
            }
            Self::SYSTEM_MEMORY => {
                let virt = crate::memory::phys_to_virt(self.address + offset);
                core::ptr::read_volatile(virt.as_ptr::<u8>())
            }
            _ => 0,
        }
    }

    pub unsafe fn write_u8(&self, offset: u64, val: u8) {
        match self.address_space {
            Self::SYSTEM_IO => {
                let port = (self.address + offset) as u16;
                core::arch::asm!("out dx, al", in("dx") port, in("al") val);
            }
            Self::SYSTEM_MEMORY => {
                let virt = crate::memory::phys_to_virt(self.address + offset);
                core::ptr::write_volatile(virt.as_mut_ptr::<u8>(), val);
            }
            _ => {}
        }
    }

    pub unsafe fn read_u16(&self, offset: u64) -> u16 {
        match self.address_space {
            Self::SYSTEM_IO => {
                let port = (self.address + offset) as u16;
                let val: u16;
                core::arch::asm!("in ax, dx", out("ax") val, in("dx") port);
                val
            }
            Self::SYSTEM_MEMORY => {
                let virt = crate::memory::phys_to_virt(self.address + offset);
                core::ptr::read_volatile(virt.as_ptr::<u16>())
            }
            _ => 0,
        }
    }

    pub unsafe fn write_u16(&self, offset: u64, val: u16) {
        match self.address_space {
            Self::SYSTEM_IO => {
                let port = (self.address + offset) as u16;
                core::arch::asm!("out dx, ax", in("dx") port, in("ax") val);
            }
            Self::SYSTEM_MEMORY => {
                let virt = crate::memory::phys_to_virt(self.address + offset);
                core::ptr::write_volatile(virt.as_mut_ptr::<u16>(), val);
            }
            _ => {}
        }
    }
}

impl Fadt {
    pub unsafe fn parse(addr: u64) -> Result<Self, AcpiError> {
        let ptr = addr as *const u8;
        let hdr_len = *(ptr.add(4) as *const u32) as usize;

        let read_ga = |off: usize| -> GenericAddress {
            if off + 12 <= hdr_len {
                GenericAddress::parse(addr + off as u64)
            } else {
                GenericAddress {
                    address_space: 0,
                    bit_width: 0,
                    bit_offset: 0,
                    access_size: 0,
                    address: 0,
                }
            }
        };

        Ok(Self {
            facs_address: *(ptr.add(36) as *const u32),
            dsdt_address: *(ptr.add(40) as *const u32),
            preferred_pm_profile: *ptr.add(45),
            sci_interrupt: *(ptr.add(46) as *const u16),
            smi_command: *(ptr.add(48) as *const u32),
            acpi_enable: *ptr.add(52),
            acpi_disable: *ptr.add(53),
            s4bios_req: *ptr.add(54),
            pstate_control: *ptr.add(55),
            pm1a_event_block: *(ptr.add(56) as *const u32),
            pm1b_event_block: *(ptr.add(60) as *const u32),
            pm1a_control_block: *(ptr.add(64) as *const u32),
            pm1b_control_block: *(ptr.add(68) as *const u32),
            pm2_control_block: *(ptr.add(72) as *const u32),
            pm_timer_block: *(ptr.add(76) as *const u32),
            gpe0_block: *(ptr.add(80) as *const u32),
            gpe1_block: *(ptr.add(84) as *const u32),
            pm1_event_length: *ptr.add(88),
            pm1_control_length: *ptr.add(89),
            pm2_control_length: *ptr.add(90),
            pm_timer_length: *ptr.add(91),
            gpe0_length: *ptr.add(92),
            gpe1_length: *ptr.add(93),
            gpe1_base: *ptr.add(94),
            c_state_control: *ptr.add(95),
            worst_c2_latency: *(ptr.add(96) as *const u16),
            worst_c3_latency: *(ptr.add(98) as *const u16),
            flush_size: *(ptr.add(100) as *const u16),
            flush_stride: *(ptr.add(102) as *const u16),
            duty_offset: *ptr.add(104),
            duty_width: *ptr.add(105),
            day_alarm: *ptr.add(106),
            month_alarm: *ptr.add(107),
            century: *ptr.add(108),
            iapc_boot_arch: *(ptr.add(109) as *const u16),
            flags: *(ptr.add(112) as *const u32),
            reset_register: read_ga(116),
            reset_value: *ptr.add(128),
            arm_boot_arch: if hdr_len > 129 {
                *(ptr.add(129) as *const u16)
            } else {
                0
            },
            fadt_minor_version: if hdr_len > 131 { *ptr.add(131) } else { 0 },
            x_facs_address: if hdr_len > 132 {
                *(ptr.add(132) as *const u64)
            } else {
                0
            },
            x_dsdt_address: if hdr_len > 140 {
                *(ptr.add(140) as *const u64)
            } else {
                0
            },
            x_pm1a_event_block: read_ga(148),
            x_pm1b_event_block: read_ga(160),
            x_pm1a_control_block: read_ga(172),
            x_pm1b_control_block: read_ga(184),
            x_pm2_control_block: read_ga(196),
            x_pm_timer_block: read_ga(208),
            x_gpe0_block: read_ga(220),
            x_gpe1_block: read_ga(232),
        })
    }

    pub fn dsdt_phys(&self) -> u64 {
        if self.x_dsdt_address != 0 {
            self.x_dsdt_address
        } else {
            self.dsdt_address as u64
        }
    }

    pub fn has_8042(&self) -> bool {
        self.iapc_boot_arch & (1 << 1) != 0
    }
    pub fn has_vga(&self) -> bool {
        self.iapc_boot_arch & (1 << 2) == 0
    }
    pub fn has_msi(&self) -> bool {
        self.iapc_boot_arch & (1 << 3) == 0
    }
    pub fn has_cmos_rtc(&self) -> bool {
        self.iapc_boot_arch & (1 << 5) == 0
    }

    pub fn pm_timer_is_32bit(&self) -> bool {
        self.flags & (1 << 8) != 0
    }
    pub fn wbinvd_supported(&self) -> bool {
        self.flags & 1 != 0
    }
    pub fn hw_reduced(&self) -> bool {
        self.flags & (1 << 20) != 0
    }

    pub fn pm1a_control_gas(&self) -> GenericAddress {
        if self.x_pm1a_control_block.is_valid() {
            self.x_pm1a_control_block
        } else {
            GenericAddress {
                address_space: GenericAddress::SYSTEM_IO,
                bit_width: (self.pm1_control_length as u8) * 8,
                bit_offset: 0,
                access_size: 0,
                address: self.pm1a_control_block as u64,
            }
        }
    }

    pub fn pm1b_control_gas(&self) -> GenericAddress {
        if self.x_pm1b_control_block.is_valid() {
            self.x_pm1b_control_block
        } else {
            GenericAddress {
                address_space: GenericAddress::SYSTEM_IO,
                bit_width: (self.pm1_control_length as u8) * 8,
                bit_offset: 0,
                access_size: 0,
                address: self.pm1b_control_block as u64,
            }
        }
    }

    pub fn gpe0_block_gas(&self) -> GenericAddress {
        if self.x_gpe0_block.is_valid() {
            self.x_gpe0_block
        } else {
            GenericAddress {
                address_space: GenericAddress::SYSTEM_IO,
                bit_width: (self.gpe0_length as u8) * 8,
                bit_offset: 0,
                access_size: 0,
                address: self.gpe0_block as u64,
            }
        }
    }

    pub fn gpe1_block_gas(&self) -> GenericAddress {
        if self.x_gpe1_block.is_valid() {
            self.x_gpe1_block
        } else {
            GenericAddress {
                address_space: GenericAddress::SYSTEM_IO,
                bit_width: (self.gpe1_length as u8) * 8,
                bit_offset: 0,
                access_size: 0,
                address: self.gpe1_block as u64,
            }
        }
    }
}

// ─── MADT (Multiple APIC Description Table) ─────────────────────────────────

#[derive(Debug, Clone)]
pub enum MadtEntry {
    LocalApic {
        processor_id: u8,
        apic_id: u8,
        flags: u32,
    },
    IoApic {
        id: u8,
        address: u32,
        gsi_base: u32,
    },
    InterruptSourceOverride {
        bus: u8,
        source: u8,
        gsi: u32,
        flags: u16,
    },
    NmiSource {
        flags: u16,
        gsi: u32,
    },
    LocalApicNmi {
        processor_id: u8,
        flags: u16,
        lint: u8,
    },
    LocalApicAddressOverride {
        address: u64,
    },
    IoSapic {
        id: u8,
        gsi_base: u32,
        address: u64,
    },
    LocalSapic {
        processor_id: u8,
        sapic_id: u8,
        sapic_eid: u8,
        flags: u32,
        acpi_uid: u32,
    },
    PlatformInterruptSource {
        flags: u16,
        interrupt_type: u8,
        processor_id: u8,
        processor_eid: u8,
        io_sapic_vector: u8,
        gsi: u32,
        source_flags: u32,
    },
    X2Apic {
        x2apic_id: u32,
        flags: u32,
        acpi_uid: u32,
    },
    X2ApicNmi {
        flags: u16,
        acpi_uid: u32,
        lint: u8,
    },
    GicCpu {
        cpu_interface_number: u32,
        acpi_uid: u32,
        flags: u32,
        parking_version: u32,
        perf_interrupt: u32,
        parked_address: u64,
        base_address: u64,
        gicv_address: u64,
        gich_address: u64,
        vgic_maintenance: u32,
        gicr_base: u64,
        mpidr: u64,
    },
    GicDistributor {
        gic_id: u32,
        base_address: u64,
        gsi_base: u32,
        version: u8,
    },
    GicMsiFrame {
        msi_frame_id: u32,
        base_address: u64,
        flags: u32,
        spi_count: u16,
        spi_base: u16,
    },
    GicRedistributor {
        base_address: u64,
        length: u32,
    },
    GicIts {
        its_id: u32,
        base_address: u64,
    },
    MultiprocessorWakeup {
        mailbox_version: u16,
        mailbox_address: u64,
    },
}

#[derive(Debug, Clone)]
pub struct Madt {
    pub local_apic_address: u32,
    pub flags: u32,
    pub entries: Vec<MadtEntry>,
}

impl Madt {
    pub unsafe fn parse(addr: u64) -> Result<Self, AcpiError> {
        let ptr = addr as *const u8;
        let length = *(ptr.add(4) as *const u32) as usize;
        let lapic_addr = *(ptr.add(36) as *const u32);
        let flags = *(ptr.add(40) as *const u32);

        let mut entries = Vec::new();
        let mut offset = 44;
        while offset + 2 <= length {
            let entry_type = *ptr.add(offset);
            let entry_len = *ptr.add(offset + 1) as usize;
            if entry_len < 2 || offset + entry_len > length {
                break;
            }

            let e = ptr.add(offset);
            match entry_type {
                0 if entry_len >= 8 => entries.push(MadtEntry::LocalApic {
                    processor_id: *e.add(2),
                    apic_id: *e.add(3),
                    flags: *(e.add(4) as *const u32),
                }),
                1 if entry_len >= 12 => entries.push(MadtEntry::IoApic {
                    id: *e.add(2),
                    address: *(e.add(4) as *const u32),
                    gsi_base: *(e.add(8) as *const u32),
                }),
                2 if entry_len >= 10 => entries.push(MadtEntry::InterruptSourceOverride {
                    bus: *e.add(2),
                    source: *e.add(3),
                    gsi: *(e.add(4) as *const u32),
                    flags: *(e.add(8) as *const u16),
                }),
                3 if entry_len >= 8 => entries.push(MadtEntry::NmiSource {
                    flags: *(e.add(2) as *const u16),
                    gsi: *(e.add(4) as *const u32),
                }),
                4 if entry_len >= 6 => entries.push(MadtEntry::LocalApicNmi {
                    processor_id: *e.add(2),
                    flags: *(e.add(3) as *const u16),
                    lint: *e.add(5),
                }),
                5 if entry_len >= 12 => entries.push(MadtEntry::LocalApicAddressOverride {
                    address: *(e.add(4) as *const u64),
                }),
                9 if entry_len >= 16 => entries.push(MadtEntry::X2Apic {
                    x2apic_id: *(e.add(4) as *const u32),
                    flags: *(e.add(8) as *const u32),
                    acpi_uid: *(e.add(12) as *const u32),
                }),
                10 if entry_len >= 12 => entries.push(MadtEntry::X2ApicNmi {
                    flags: *(e.add(2) as *const u16),
                    acpi_uid: *(e.add(4) as *const u32),
                    lint: *e.add(8),
                }),
                _ => {}
            }
            offset += entry_len;
        }

        Ok(Self {
            local_apic_address: lapic_addr,
            flags,
            entries,
        })
    }

    pub fn local_apics(&self) -> Vec<(u8, u8)> {
        self.entries
            .iter()
            .filter_map(|e| match e {
                MadtEntry::LocalApic {
                    processor_id,
                    apic_id,
                    flags,
                } if *flags & 1 != 0 => Some((*processor_id, *apic_id)),
                _ => None,
            })
            .collect()
    }

    pub fn io_apics(&self) -> Vec<(u8, u32, u32)> {
        self.entries
            .iter()
            .filter_map(|e| match e {
                MadtEntry::IoApic {
                    id,
                    address,
                    gsi_base,
                } => Some((*id, *address, *gsi_base)),
                _ => None,
            })
            .collect()
    }

    pub fn interrupt_overrides(&self) -> Vec<(u8, u32, u16)> {
        self.entries
            .iter()
            .filter_map(|e| match e {
                MadtEntry::InterruptSourceOverride {
                    source, gsi, flags, ..
                } => Some((*source, *gsi, *flags)),
                _ => None,
            })
            .collect()
    }
}

// ─── Known Table Signatures ─────────────────────────────────────────────────

pub const SIG_FACP: [u8; 4] = *b"FACP";
pub const SIG_DSDT: [u8; 4] = *b"DSDT";
pub const SIG_SSDT: [u8; 4] = *b"SSDT";
pub const SIG_APIC: [u8; 4] = *b"APIC";
pub const SIG_MCFG: [u8; 4] = *b"MCFG";
pub const SIG_HPET: [u8; 4] = *b"HPET";
pub const SIG_BGRT: [u8; 4] = *b"BGRT";
pub const SIG_SRAT: [u8; 4] = *b"SRAT";
pub const SIG_SLIT: [u8; 4] = *b"SLIT";
pub const SIG_BERT: [u8; 4] = *b"BERT";
pub const SIG_EINJ: [u8; 4] = *b"EINJ";
pub const SIG_ERST: [u8; 4] = *b"ERST";
pub const SIG_FPDT: [u8; 4] = *b"FPDT";
pub const SIG_GTDT: [u8; 4] = *b"GTDT";
pub const SIG_IORT: [u8; 4] = *b"IORT";
pub const SIG_LPIT: [u8; 4] = *b"LPIT";
pub const SIG_MCHI: [u8; 4] = *b"MCHI";
pub const SIG_MPST: [u8; 4] = *b"MPST";
pub const SIG_MSCT: [u8; 4] = *b"MSCT";
pub const SIG_NFIT: [u8; 4] = *b"NFIT";
pub const SIG_PCCT: [u8; 4] = *b"PCCT";
pub const SIG_PMTT: [u8; 4] = *b"PMTT";
pub const SIG_PPTT: [u8; 4] = *b"PPTT";
pub const SIG_RASF: [u8; 4] = *b"RASF";
pub const SIG_SBST: [u8; 4] = *b"SBST";
pub const SIG_SDEV: [u8; 4] = *b"SDEV";
pub const SIG_STAO: [u8; 4] = *b"STAO";
pub const SIG_TPM2: [u8; 4] = *b"TPM2";
pub const SIG_UEFI: [u8; 4] = *b"UEFI";
pub const SIG_WAET: [u8; 4] = *b"WAET";
pub const SIG_WDAT: [u8; 4] = *b"WDAT";
pub const SIG_WDDT: [u8; 4] = *b"WDDT";
pub const SIG_WDRT: [u8; 4] = *b"WDRT";
pub const SIG_WPBT: [u8; 4] = *b"WPBT";
pub const SIG_WSMT: [u8; 4] = *b"WSMT";
pub const SIG_DBG2: [u8; 4] = *b"DBG2";
pub const SIG_DMAR: [u8; 4] = *b"DMAR";
pub const SIG_IVRS: [u8; 4] = *b"IVRS";
pub const SIG_TCPA: [u8; 4] = *b"TCPA";

// ─── AML Bytecode Constants ─────────────────────────────────────────────────

mod aml_opcodes {
    pub const ZERO_OP: u8 = 0x00;
    pub const ONE_OP: u8 = 0x01;
    pub const ALIAS_OP: u8 = 0x06;
    pub const NAME_OP: u8 = 0x08;
    pub const BYTE_PREFIX: u8 = 0x0A;
    pub const WORD_PREFIX: u8 = 0x0B;
    pub const DWORD_PREFIX: u8 = 0x0C;
    pub const STRING_PREFIX: u8 = 0x0D;
    pub const QWORD_PREFIX: u8 = 0x0E;
    pub const SCOPE_OP: u8 = 0x10;
    pub const BUFFER_OP: u8 = 0x11;
    pub const PACKAGE_OP: u8 = 0x12;
    pub const VAR_PACKAGE_OP: u8 = 0x13;
    pub const METHOD_OP: u8 = 0x14;
    pub const EXTERNAL_OP: u8 = 0x15;
    pub const LOCAL0_OP: u8 = 0x60;
    pub const LOCAL1_OP: u8 = 0x61;
    pub const LOCAL2_OP: u8 = 0x62;
    pub const LOCAL3_OP: u8 = 0x63;
    pub const LOCAL4_OP: u8 = 0x64;
    pub const LOCAL5_OP: u8 = 0x65;
    pub const LOCAL6_OP: u8 = 0x66;
    pub const LOCAL7_OP: u8 = 0x67;
    pub const ARG0_OP: u8 = 0x68;
    pub const ARG1_OP: u8 = 0x69;
    pub const ARG2_OP: u8 = 0x6A;
    pub const ARG3_OP: u8 = 0x6B;
    pub const ARG4_OP: u8 = 0x6C;
    pub const ARG5_OP: u8 = 0x6D;
    pub const ARG6_OP: u8 = 0x6E;
    pub const STORE_OP: u8 = 0x70;
    pub const REF_OF_OP: u8 = 0x71;
    pub const ADD_OP: u8 = 0x72;
    pub const CONCAT_OP: u8 = 0x73;
    pub const SUBTRACT_OP: u8 = 0x74;
    pub const INCREMENT_OP: u8 = 0x75;
    pub const DECREMENT_OP: u8 = 0x76;
    pub const MULTIPLY_OP: u8 = 0x77;
    pub const DIVIDE_OP: u8 = 0x78;
    pub const SHIFT_LEFT_OP: u8 = 0x79;
    pub const SHIFT_RIGHT_OP: u8 = 0x7A;
    pub const AND_OP: u8 = 0x7B;
    pub const NAND_OP: u8 = 0x7C;
    pub const OR_OP: u8 = 0x7D;
    pub const NOR_OP: u8 = 0x7E;
    pub const XOR_OP: u8 = 0x7F;
    pub const NOT_OP: u8 = 0x80;
    pub const FIND_SET_LEFT_BIT: u8 = 0x81;
    pub const FIND_SET_RIGHT_BIT: u8 = 0x82;
    pub const DEREF_OF_OP: u8 = 0x83;
    pub const CONCAT_RES_OP: u8 = 0x84;
    pub const MOD_OP: u8 = 0x85;
    pub const NOTIFY_OP: u8 = 0x86;
    pub const SIZE_OF_OP: u8 = 0x87;
    pub const INDEX_OP: u8 = 0x88;
    pub const MATCH_OP: u8 = 0x89;
    pub const CREATE_DWORD_FIELD: u8 = 0x8A;
    pub const CREATE_WORD_FIELD: u8 = 0x8B;
    pub const CREATE_BYTE_FIELD: u8 = 0x8C;
    pub const CREATE_BIT_FIELD: u8 = 0x8D;
    pub const OBJECT_TYPE_OP: u8 = 0x8E;
    pub const CREATE_QWORD_FIELD: u8 = 0x8F;
    pub const LAND_OP: u8 = 0x90;
    pub const LOR_OP: u8 = 0x91;
    pub const LNOT_OP: u8 = 0x92;
    pub const LEQUAL_OP: u8 = 0x93;
    pub const LGREATER_OP: u8 = 0x94;
    pub const LLESS_OP: u8 = 0x95;
    pub const TO_BUFFER_OP: u8 = 0x96;
    pub const TO_DEC_STRING_OP: u8 = 0x97;
    pub const TO_HEX_STRING_OP: u8 = 0x98;
    pub const TO_INTEGER_OP: u8 = 0x99;
    pub const TO_STRING_OP: u8 = 0x9C;
    pub const COPY_OBJECT_OP: u8 = 0x9D;
    pub const MID_OP: u8 = 0x9E;
    pub const CONTINUE_OP: u8 = 0x9F;
    pub const IF_OP: u8 = 0xA0;
    pub const ELSE_OP: u8 = 0xA1;
    pub const WHILE_OP: u8 = 0xA2;
    pub const NOOP_OP: u8 = 0xA3;
    pub const RETURN_OP: u8 = 0xA4;
    pub const BREAK_OP: u8 = 0xA5;
    pub const ONES_OP: u8 = 0xFF;
    pub const EXT_OP_PREFIX: u8 = 0x5B;

    pub const EXT_MUTEX_OP: u8 = 0x01;
    pub const EXT_EVENT_OP: u8 = 0x02;
    pub const EXT_COND_REF_OF: u8 = 0x12;
    pub const EXT_CREATE_FIELD: u8 = 0x13;
    pub const EXT_ACQUIRE_OP: u8 = 0x23;
    pub const EXT_SIGNAL_OP: u8 = 0x24;
    pub const EXT_WAIT_OP: u8 = 0x25;
    pub const EXT_RESET_OP: u8 = 0x26;
    pub const EXT_RELEASE_OP: u8 = 0x27;
    pub const EXT_STALL_OP: u8 = 0x22;
    pub const EXT_SLEEP_OP: u8 = 0x21;
    pub const EXT_FATAL_OP: u8 = 0x32;
    pub const EXT_OP_REGION_OP: u8 = 0x80;
    pub const EXT_FIELD_OP: u8 = 0x81;
    pub const EXT_DEVICE_OP: u8 = 0x82;
    pub const EXT_PROCESSOR_OP: u8 = 0x83;
    pub const EXT_POWER_RES_OP: u8 = 0x84;
    pub const EXT_THERMAL_ZONE_OP: u8 = 0x85;
    pub const EXT_INDEX_FIELD_OP: u8 = 0x86;
    pub const EXT_BANK_FIELD_OP: u8 = 0x87;
    pub const EXT_REVISION_OP: u8 = 0x30;
}

// ─── AML Data Objects ───────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum AmlValue {
    Integer(u64),
    String(String),
    Buffer(Vec<u8>),
    Package(Vec<AmlValue>),
    Method {
        arg_count: u8,
        serialized: bool,
        sync_level: u8,
        body: Vec<u8>,
    },
    OpRegion {
        space: OpRegionSpace,
        offset: u64,
        length: u64,
    },
    Field {
        region: String,
        offset: u64,
        length: u64,
        access: FieldAccess,
        update: FieldUpdate,
    },
    Device(String),
    Processor {
        id: u8,
        pblk_addr: u32,
        pblk_len: u8,
    },
    PowerResource {
        system_level: u8,
        resource_order: u16,
    },
    ThermalZone(String),
    Mutex {
        sync_level: u8,
    },
    Event,
    Reference(String),
    Uninitialized,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpRegionSpace {
    SystemMemory,
    SystemIO,
    PciConfig,
    EmbeddedControl,
    SMBus,
    Cmos,
    PciBarTarget,
    Ipmi,
    GeneralPurposeIo,
    GenericSerialBus,
    Pcc,
    OemDefined(u8),
}

impl OpRegionSpace {
    pub fn from_byte(b: u8) -> Self {
        match b {
            0x00 => Self::SystemMemory,
            0x01 => Self::SystemIO,
            0x02 => Self::PciConfig,
            0x03 => Self::EmbeddedControl,
            0x04 => Self::SMBus,
            0x05 => Self::Cmos,
            0x06 => Self::PciBarTarget,
            0x07 => Self::Ipmi,
            0x08 => Self::GeneralPurposeIo,
            0x09 => Self::GenericSerialBus,
            0x0A => Self::Pcc,
            n => Self::OemDefined(n),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldAccess {
    AnyAcc,
    ByteAcc,
    WordAcc,
    DWordAcc,
    QWordAcc,
    BufferAcc,
}

impl FieldAccess {
    pub fn from_bits(bits: u8) -> Self {
        match bits & 0x0F {
            0 => Self::AnyAcc,
            1 => Self::ByteAcc,
            2 => Self::WordAcc,
            3 => Self::DWordAcc,
            4 => Self::QWordAcc,
            5 => Self::BufferAcc,
            _ => Self::AnyAcc,
        }
    }

    pub fn width_bytes(self) -> usize {
        match self {
            Self::AnyAcc | Self::ByteAcc => 1,
            Self::WordAcc => 2,
            Self::DWordAcc => 4,
            Self::QWordAcc => 8,
            Self::BufferAcc => 1,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldUpdate {
    Preserve,
    WriteAsOnes,
    WriteAsZeros,
}

impl FieldUpdate {
    pub fn from_bits(bits: u8) -> Self {
        match (bits >> 5) & 0x03 {
            0 => Self::Preserve,
            1 => Self::WriteAsOnes,
            2 => Self::WriteAsZeros,
            _ => Self::Preserve,
        }
    }
}

// ─── AML Namespace ──────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct NamespaceNode {
    pub name: String,
    pub full_path: String,
    pub value: AmlValue,
    pub children: Vec<String>,
    pub hid: Option<String>,
    pub cid: Option<String>,
    pub uid: Option<u64>,
    pub adr: Option<u64>,
    pub sta: Option<u32>,
}

impl NamespaceNode {
    pub fn new(name: &str, full_path: &str, value: AmlValue) -> Self {
        Self {
            name: String::from(name),
            full_path: String::from(full_path),
            value,
            children: Vec::new(),
            hid: None,
            cid: None,
            uid: None,
            adr: None,
            sta: None,
        }
    }

    pub fn is_present(&self) -> bool {
        self.sta.map_or(true, |s| s & 0x01 != 0)
    }

    pub fn is_enabled(&self) -> bool {
        self.sta.map_or(true, |s| s & 0x02 != 0)
    }

    pub fn is_functioning(&self) -> bool {
        self.sta.map_or(true, |s| s & 0x08 != 0)
    }
}

pub struct AmlNamespace {
    pub nodes: BTreeMap<String, NamespaceNode>,
    pub scope_stack: Vec<String>,
}

impl AmlNamespace {
    pub fn new() -> Self {
        let mut ns = Self {
            nodes: BTreeMap::new(),
            scope_stack: Vec::new(),
        };

        for scope in &["\\", "\\_SB", "\\_PR", "\\_TZ", "\\_GPE", "\\_SI"] {
            ns.nodes.insert(
                String::from(*scope),
                NamespaceNode::new(scope, scope, AmlValue::Uninitialized),
            );
        }
        ns.scope_stack.push(String::from("\\"));
        ns
    }

    pub fn current_scope(&self) -> &str {
        self.scope_stack.last().map_or("\\", |s| s.as_str())
    }

    pub fn push_scope(&mut self, name: &str) {
        let full = self.resolve_path(name);
        self.scope_stack.push(full);
    }

    pub fn pop_scope(&mut self) {
        if self.scope_stack.len() > 1 {
            self.scope_stack.pop();
        }
    }

    pub fn resolve_path(&self, name: &str) -> String {
        if name.starts_with('\\') {
            return String::from(name);
        }
        if name.starts_with('^') {
            let scope = self.current_scope();
            let ups = name.chars().take_while(|c| *c == '^').count();
            let parts: Vec<&str> = scope.split('.').collect();
            let base = if ups >= parts.len() {
                String::from("\\")
            } else {
                parts[..parts.len() - ups].join(".")
            };
            let rest = &name[ups..];
            if base == "\\" {
                return alloc::format!("\\{}", rest);
            }
            return alloc::format!("{}.{}", base, rest);
        }
        let scope = self.current_scope();
        if scope == "\\" {
            alloc::format!("\\{}", name)
        } else {
            alloc::format!("{}.{}", scope, name)
        }
    }

    pub fn insert(&mut self, name: &str, value: AmlValue) {
        let full = self.resolve_path(name);
        let node = NamespaceNode::new(name, &full, value);
        let parent = String::from(self.current_scope());
        if let Some(p) = self.nodes.get_mut(&parent) {
            p.children.push(full.clone());
        }
        self.nodes.insert(full, node);
    }

    pub fn get(&self, path: &str) -> Option<&NamespaceNode> {
        let full = if path.starts_with('\\') {
            String::from(path)
        } else {
            self.resolve_path(path)
        };
        self.nodes.get(&full)
    }

    pub fn get_mut(&mut self, path: &str) -> Option<&mut NamespaceNode> {
        let full = if path.starts_with('\\') {
            String::from(path)
        } else {
            self.resolve_path(path)
        };
        self.nodes.get_mut(&full)
    }

    // NOTE: device enumeration moved to `AcpiSubsystem::collect_namespace_devices`,
    // which walks the authoritative `aml` crate namespace. This hand-rolled
    // namespace is never populated from firmware AML (see AcpiSubsystem::init)
    // and must not be used to report device counts.
}

// ─── AML Interpreter ────────────────────────────────────────────────────────

pub struct AmlInterpreter {
    pub namespace: AmlNamespace,
    pub locals: [AmlValue; 8],
    pub args: [AmlValue; 7],
    pub revision: u64,
    ops_budget: usize,
    recursion_depth: usize,
}

const AML_MAX_OPS: usize = 200_000;

// ── Self-service ACPI table dump (Phase 1.4 field debugging) ────────────
//
// When a firmware table fails AML parsing on bare metal, the raw bytes are
// the ONLY thing that lets the failure be reproduced and fixed off-machine —
// and on a no-serial box with a broken host OS there may be no other way to
// get them (Athena: Windows died before its acpidump could be copied off).
// So the kernel dumps the failing table itself: base64, written STRAIGHT
// into the bootlog RAM ring (bootlog::append — no serial, no screen spam),
// which the existing BOOTLOG.TXT flush persists and read-bootlog.ps1
// extracts. Decode side: pull the lines between BEGIN/END markers and
// `certutil -decode` / `base64 -d` them.

/// Total encoded-bytes budget across all table dumps. The ring is 1 MiB and
/// must still hold the boot transcript; the early-locked flush region is
/// 512 KiB. One AMD DSDT (~150-250 KiB raw, ~+33% encoded) fits; runaway
/// SSDT sets must not evict the transcript.
const ACPI_DUMP_BUDGET: usize = 600 * 1024;
static ACPI_DUMP_USED: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

fn b64_encode_chunk(out: &mut alloc::string::String, chunk: &[u8]) {
    const TBL: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let b0 = chunk[0] as u32;
    let b1 = *chunk.get(1).unwrap_or(&0) as u32;
    let b2 = *chunk.get(2).unwrap_or(&0) as u32;
    let n = (b0 << 16) | (b1 << 8) | b2;
    out.push(TBL[(n >> 18) as usize & 63] as char);
    out.push(TBL[(n >> 12) as usize & 63] as char);
    out.push(if chunk.len() > 1 {
        TBL[(n >> 6) as usize & 63] as char
    } else {
        '='
    });
    out.push(if chunk.len() > 2 {
        TBL[n as usize & 63] as char
    } else {
        '='
    });
}

/// Dump a raw ACPI table (header INCLUDED, so the decoded file is a valid
/// .dat like acpidump produces) into the bootlog ring as base64.
fn dump_table_to_bootlog(label: &str, table: &[u8]) {
    use core::sync::atomic::Ordering;
    let encoded = (table.len() + 2) / 3 * 4;
    let prior = ACPI_DUMP_USED.fetch_add(encoded, Ordering::Relaxed);
    if prior + encoded > ACPI_DUMP_BUDGET {
        crate::serial_println!(
            "[acpi-dump] {} ({} B) skipped — dump budget exhausted",
            label,
            table.len()
        );
        return;
    }
    crate::serial_println!(
        "[acpi-dump] writing {} ({} bytes) to bootlog as base64",
        label,
        table.len()
    );
    let mut line = alloc::string::String::with_capacity(80);
    crate::bootlog::append(b"[acpi-dump] BEGIN ");
    crate::bootlog::append(label.as_bytes());
    crate::bootlog::append(b"\n");
    // 57 raw bytes -> 76 base64 chars per line (MIME width).
    for row in table.chunks(57) {
        line.clear();
        for chunk in row.chunks(3) {
            b64_encode_chunk(&mut line, chunk);
        }
        line.push('\n');
        crate::bootlog::append(line.as_bytes());
    }
    crate::bootlog::append(b"[acpi-dump] END ");
    crate::bootlog::append(label.as_bytes());
    crate::bootlog::append(b"\n");
}

impl AmlInterpreter {
    pub fn new() -> Self {
        Self {
            namespace: AmlNamespace::new(),
            locals: core::array::from_fn(|_| AmlValue::Uninitialized),
            args: core::array::from_fn(|_| AmlValue::Uninitialized),
            revision: 2, // ACPI 2.0+
            ops_budget: AML_MAX_OPS,
            recursion_depth: 0,
        }
    }

    pub fn load_table(&mut self, aml: &[u8]) -> Result<(), AcpiError> {
        if aml.len() < SdtHeader::SIZE {
            return Err(AcpiError::AmlParseError(String::from("table too short")));
        }
        let body = &aml[SdtHeader::SIZE..];
        self.parse_term_list(body, 0)
    }

    const MAX_RECURSION: usize = 32;

    fn parse_term_list(&mut self, aml: &[u8], mut pos: usize) -> Result<(), AcpiError> {
        self.recursion_depth += 1;
        if self.recursion_depth > Self::MAX_RECURSION {
            self.recursion_depth -= 1;
            return Err(AcpiError::AmlParseError(alloc::string::String::from(
                "AML recursion depth exceeded",
            )));
        }
        while pos < aml.len() {
            if self.ops_budget == 0 {
                self.recursion_depth -= 1;
                return Err(AcpiError::AmlParseError(alloc::string::String::from(
                    "AML parse budget exhausted",
                )));
            }
            self.ops_budget -= 1;
            let old_pos = pos;
            pos = self.parse_term(aml, pos)?;
            if pos <= old_pos {
                self.recursion_depth -= 1;
                let op = aml.get(pos).copied().unwrap_or(0xFF);
                return Err(AcpiError::AmlParseError(alloc::format!(
                    "AML stuck at byte {} (op=0x{:02x})",
                    pos,
                    op
                )));
            }
        }
        self.recursion_depth -= 1;
        Ok(())
    }

    fn parse_term(&mut self, aml: &[u8], pos: usize) -> Result<usize, AcpiError> {
        if pos >= aml.len() {
            return Ok(pos);
        }
        let op = aml[pos];
        match op {
            aml_opcodes::ZERO_OP => Ok(pos + 1),
            aml_opcodes::ONE_OP => Ok(pos + 1),
            aml_opcodes::ONES_OP => Ok(pos + 1),
            aml_opcodes::NOOP_OP => Ok(pos + 1),

            aml_opcodes::BYTE_PREFIX => Ok(pos + 2),
            aml_opcodes::WORD_PREFIX => Ok(pos + 3),
            aml_opcodes::DWORD_PREFIX => Ok(pos + 5),
            aml_opcodes::QWORD_PREFIX => Ok(pos + 9),

            aml_opcodes::STRING_PREFIX => {
                let start = pos + 1;
                if start >= aml.len() {
                    crate::serial_println!("[acpi][warn] AML string prefix at end of stream");
                    return Ok(aml.len());
                }
                let rel_end = aml[start..].iter().position(|&b| b == 0);
                let end = rel_end.map_or(aml.len(), |p| start + p);
                Ok(end.saturating_add(1).min(aml.len()))
            }

            aml_opcodes::NAME_OP => {
                let (name, next) = self.parse_name_string(aml, pos + 1)?;
                let (val, next) = self.parse_data_object(aml, next)?;
                self.namespace.insert(&name, val);
                Ok(next)
            }

            aml_opcodes::SCOPE_OP => {
                let (pkg_len, body_start) = match self.parse_pkg_length(aml, pos + 1) {
                    Ok(v) => v,
                    Err(e) => {
                        crate::serial_println!(
                            "[acpi][warn] AML Scope PkgLength parse failed at {}: {:?}",
                            pos,
                            e
                        );
                        return Ok(pos.saturating_add(1).min(aml.len()));
                    }
                };
                let scope_end = pos.saturating_add(1).saturating_add(pkg_len).min(aml.len());
                let (name, body_pos) = self.parse_name_string(aml, body_start)?;
                if body_pos > scope_end {
                    crate::serial_println!(
                        "[acpi][warn] AML Scope body out of range ({} > {})",
                        body_pos,
                        scope_end
                    );
                    return Ok(scope_end);
                }
                let scope_body = &aml[body_pos..scope_end];
                self.namespace.push_scope(&name);
                let _ = self.parse_term_list(scope_body, 0).ok();
                self.namespace.pop_scope();
                Ok(scope_end)
            }

            aml_opcodes::METHOD_OP => {
                let (pkg_len, body_start) = match self.parse_pkg_length(aml, pos + 1) {
                    Ok(v) => v,
                    Err(e) => {
                        crate::serial_println!(
                            "[acpi][warn] AML Method PkgLength parse failed at {}: {:?}",
                            pos,
                            e
                        );
                        return Ok(pos.saturating_add(1).min(aml.len()));
                    }
                };
                let method_end = pos.saturating_add(1).saturating_add(pkg_len).min(aml.len());
                let (name, flags_pos) = self.parse_name_string(aml, body_start)?;
                if flags_pos < method_end {
                    let flags = aml[flags_pos];
                    let arg_count = flags & 0x07;
                    let serialized = flags & 0x08 != 0;
                    let sync_level = (flags >> 4) & 0x0F;
                    let body_start = flags_pos.saturating_add(1);
                    let body = if body_start <= method_end {
                        aml[body_start..method_end].to_vec()
                    } else {
                        crate::serial_println!("[acpi][warn] AML Method body out of range");
                        Vec::new()
                    };
                    self.namespace.insert(
                        &name,
                        AmlValue::Method {
                            arg_count,
                            serialized,
                            sync_level,
                            body,
                        },
                    );
                }
                Ok(method_end)
            }

            aml_opcodes::BUFFER_OP => {
                let (pkg_len, _) = match self.parse_pkg_length(aml, pos + 1) {
                    Ok(v) => v,
                    Err(_) => return Ok(pos.saturating_add(1).min(aml.len())),
                };
                Ok(pos.saturating_add(1).saturating_add(pkg_len).min(aml.len()))
            }

            aml_opcodes::PACKAGE_OP | aml_opcodes::VAR_PACKAGE_OP => {
                let (pkg_len, _) = match self.parse_pkg_length(aml, pos + 1) {
                    Ok(v) => v,
                    Err(_) => return Ok(pos.saturating_add(1).min(aml.len())),
                };
                Ok(pos.saturating_add(1).saturating_add(pkg_len).min(aml.len()))
            }

            aml_opcodes::ALIAS_OP => {
                let (_, next) = self.parse_name_string(aml, pos + 1)?;
                let (_, next) = self.parse_name_string(aml, next)?;
                Ok(next)
            }

            aml_opcodes::STORE_OP => {
                let next = self.skip_term_arg(aml, pos + 1)?;
                let next = self.skip_term_arg(aml, next)?;
                Ok(next)
            }

            aml_opcodes::IF_OP => {
                let (pkg_len, _) = match self.parse_pkg_length(aml, pos + 1) {
                    Ok(v) => v,
                    Err(_) => return Ok(pos.saturating_add(1).min(aml.len())),
                };
                Ok(pos.saturating_add(1).saturating_add(pkg_len).min(aml.len()))
            }
            aml_opcodes::ELSE_OP => {
                let (pkg_len, _) = match self.parse_pkg_length(aml, pos + 1) {
                    Ok(v) => v,
                    Err(_) => return Ok(pos.saturating_add(1).min(aml.len())),
                };
                Ok(pos.saturating_add(1).saturating_add(pkg_len).min(aml.len()))
            }
            aml_opcodes::WHILE_OP => {
                let (pkg_len, _) = match self.parse_pkg_length(aml, pos + 1) {
                    Ok(v) => v,
                    Err(_) => return Ok(pos.saturating_add(1).min(aml.len())),
                };
                Ok(pos.saturating_add(1).saturating_add(pkg_len).min(aml.len()))
            }

            aml_opcodes::RETURN_OP => {
                let next = self.skip_term_arg(aml, pos + 1)?;
                Ok(next)
            }
            aml_opcodes::BREAK_OP => Ok(pos + 1),
            aml_opcodes::CONTINUE_OP => Ok(pos + 1),

            aml_opcodes::ADD_OP
            | aml_opcodes::SUBTRACT_OP
            | aml_opcodes::MULTIPLY_OP
            | aml_opcodes::AND_OP
            | aml_opcodes::OR_OP
            | aml_opcodes::XOR_OP
            | aml_opcodes::NAND_OP
            | aml_opcodes::NOR_OP
            | aml_opcodes::SHIFT_LEFT_OP
            | aml_opcodes::SHIFT_RIGHT_OP
            | aml_opcodes::MOD_OP
            | aml_opcodes::CONCAT_OP
            | aml_opcodes::CONCAT_RES_OP => {
                let next = self.skip_term_arg(aml, pos + 1)?;
                let next = self.skip_term_arg(aml, next)?;
                let next = self.skip_term_arg(aml, next)?;
                Ok(next)
            }

            aml_opcodes::DIVIDE_OP => {
                let next = self.skip_term_arg(aml, pos + 1)?;
                let next = self.skip_term_arg(aml, next)?;
                let next = self.skip_term_arg(aml, next)?;
                let next = self.skip_term_arg(aml, next)?;
                Ok(next)
            }

            aml_opcodes::NOT_OP
            | aml_opcodes::INCREMENT_OP
            | aml_opcodes::DECREMENT_OP
            | aml_opcodes::DEREF_OF_OP
            | aml_opcodes::SIZE_OF_OP
            | aml_opcodes::OBJECT_TYPE_OP
            | aml_opcodes::REF_OF_OP
            | aml_opcodes::FIND_SET_LEFT_BIT
            | aml_opcodes::FIND_SET_RIGHT_BIT => {
                let next = self.skip_term_arg(aml, pos + 1)?;
                Ok(next)
            }

            aml_opcodes::INDEX_OP => {
                let next = self.skip_term_arg(aml, pos + 1)?;
                let next = self.skip_term_arg(aml, next)?;
                let next = self.skip_term_arg(aml, next)?;
                Ok(next)
            }

            aml_opcodes::MATCH_OP => {
                let next = self.skip_term_arg(aml, pos + 1)?;
                let next = self.skip_term_arg(aml, next + 1)?; // + match_op
                let next = self.skip_term_arg(aml, next + 1)?;
                let next = self.skip_term_arg(aml, next)?;
                Ok(next)
            }

            aml_opcodes::TO_BUFFER_OP
            | aml_opcodes::TO_DEC_STRING_OP
            | aml_opcodes::TO_HEX_STRING_OP
            | aml_opcodes::TO_INTEGER_OP
            | aml_opcodes::COPY_OBJECT_OP => {
                let next = self.skip_term_arg(aml, pos + 1)?;
                let next = self.skip_term_arg(aml, next)?;
                Ok(next)
            }

            aml_opcodes::TO_STRING_OP | aml_opcodes::MID_OP => {
                let next = self.skip_term_arg(aml, pos + 1)?;
                let next = self.skip_term_arg(aml, next)?;
                let next = self.skip_term_arg(aml, next)?;
                Ok(next)
            }

            aml_opcodes::NOTIFY_OP => {
                let next = self.skip_term_arg(aml, pos + 1)?;
                let next = self.skip_term_arg(aml, next)?;
                Ok(next)
            }

            aml_opcodes::LEQUAL_OP
            | aml_opcodes::LGREATER_OP
            | aml_opcodes::LLESS_OP
            | aml_opcodes::LAND_OP
            | aml_opcodes::LOR_OP => {
                let next = self.skip_term_arg(aml, pos + 1)?;
                let next = self.skip_term_arg(aml, next)?;
                Ok(next)
            }
            aml_opcodes::LNOT_OP => {
                let next = self.skip_term_arg(aml, pos + 1)?;
                Ok(next)
            }

            aml_opcodes::CREATE_DWORD_FIELD
            | aml_opcodes::CREATE_WORD_FIELD
            | aml_opcodes::CREATE_BYTE_FIELD
            | aml_opcodes::CREATE_QWORD_FIELD => {
                let next = self.skip_term_arg(aml, pos + 1)?;
                let next = self.skip_term_arg(aml, next)?;
                let (_, next) = self.parse_name_string(aml, next)?;
                Ok(next)
            }
            aml_opcodes::CREATE_BIT_FIELD => {
                let next = self.skip_term_arg(aml, pos + 1)?;
                let next = self.skip_term_arg(aml, next)?;
                let (_, next) = self.parse_name_string(aml, next)?;
                Ok(next)
            }

            aml_opcodes::EXTERNAL_OP => {
                let (_, next) = self.parse_name_string(aml, pos + 1)?;
                if next + 2 <= aml.len() {
                    Ok(next + 2)
                } else {
                    Ok(aml.len())
                }
            }

            aml_opcodes::LOCAL0_OP..=aml_opcodes::LOCAL7_OP => Ok(pos + 1),
            aml_opcodes::ARG0_OP..=aml_opcodes::ARG6_OP => Ok(pos + 1),

            aml_opcodes::EXT_OP_PREFIX => {
                if pos + 1 >= aml.len() {
                    return Ok(pos + 1);
                }
                let ext_op = aml[pos + 1];
                match ext_op {
                    aml_opcodes::EXT_OP_REGION_OP => {
                        let (name, next) = self.parse_name_string(aml, pos + 2)?;
                        if next < aml.len() {
                            let space = OpRegionSpace::from_byte(aml[next]);
                            let next = next + 1;
                            let (offset, next) = self.parse_integer(aml, next)?;
                            let (length, next) = self.parse_integer(aml, next)?;
                            self.namespace.insert(
                                &name,
                                AmlValue::OpRegion {
                                    space,
                                    offset,
                                    length,
                                },
                            );
                            Ok(next)
                        } else {
                            Ok(aml.len())
                        }
                    }
                    aml_opcodes::EXT_FIELD_OP => {
                        let (pkg_len, _) = match self.parse_pkg_length(aml, pos + 2) {
                            Ok(v) => v,
                            Err(_) => return Ok(pos.saturating_add(2).min(aml.len())),
                        };
                        Ok(pos.saturating_add(2).saturating_add(pkg_len).min(aml.len()))
                    }
                    aml_opcodes::EXT_INDEX_FIELD_OP => {
                        let (pkg_len, _) = match self.parse_pkg_length(aml, pos + 2) {
                            Ok(v) => v,
                            Err(_) => return Ok(pos.saturating_add(2).min(aml.len())),
                        };
                        Ok(pos.saturating_add(2).saturating_add(pkg_len).min(aml.len()))
                    }
                    aml_opcodes::EXT_BANK_FIELD_OP => {
                        let (pkg_len, _) = match self.parse_pkg_length(aml, pos + 2) {
                            Ok(v) => v,
                            Err(_) => return Ok(pos.saturating_add(2).min(aml.len())),
                        };
                        Ok(pos.saturating_add(2).saturating_add(pkg_len).min(aml.len()))
                    }
                    aml_opcodes::EXT_DEVICE_OP => {
                        let (pkg_len, body_start) = match self.parse_pkg_length(aml, pos + 2) {
                            Ok(v) => v,
                            Err(_) => return Ok(pos.saturating_add(2).min(aml.len())),
                        };
                        let dev_end = pos.saturating_add(2).saturating_add(pkg_len).min(aml.len());
                        let (name, body_pos) = self.parse_name_string(aml, body_start)?;
                        self.namespace.insert(&name, AmlValue::Device(name.clone()));
                        self.namespace.push_scope(&name);
                        if body_pos <= dev_end {
                            let _ = self.parse_term_list(&aml[body_pos..dev_end], 0).ok();
                        } else {
                            crate::serial_println!("[acpi][warn] AML Device body out of range");
                        }
                        self.namespace.pop_scope();
                        Ok(dev_end)
                    }
                    aml_opcodes::EXT_PROCESSOR_OP => {
                        let (pkg_len, body_start) = match self.parse_pkg_length(aml, pos + 2) {
                            Ok(v) => v,
                            Err(_) => return Ok(pos.saturating_add(2).min(aml.len())),
                        };
                        let proc_end = pos.saturating_add(2).saturating_add(pkg_len).min(aml.len());
                        let (name, next) = self.parse_name_string(aml, body_start)?;
                        if next.saturating_add(5) < aml.len() {
                            let id = aml[next];
                            let pblk = u32::from_le_bytes([
                                aml[next + 1],
                                aml[next + 2],
                                aml[next + 3],
                                aml[next + 4],
                            ]);
                            let pblk_len = aml[next + 5];
                            self.namespace.insert(
                                &name,
                                AmlValue::Processor {
                                    id,
                                    pblk_addr: pblk,
                                    pblk_len,
                                },
                            );
                            let body_start = next + 6;
                            if body_start <= proc_end {
                                self.namespace.push_scope(&name);
                                let _ = self.parse_term_list(&aml[body_start..proc_end], 0).ok();
                                self.namespace.pop_scope();
                            }
                        } else {
                            crate::serial_println!("[acpi][warn] AML Processor object truncated");
                        }
                        Ok(proc_end)
                    }
                    aml_opcodes::EXT_POWER_RES_OP => {
                        let (pkg_len, body_start) = match self.parse_pkg_length(aml, pos + 2) {
                            Ok(v) => v,
                            Err(_) => return Ok(pos.saturating_add(2).min(aml.len())),
                        };
                        let end = pos.saturating_add(2).saturating_add(pkg_len).min(aml.len());
                        let (name, next) = self.parse_name_string(aml, body_start)?;
                        if next.saturating_add(3) <= aml.len() {
                            let level = aml[next];
                            let order = u16::from_le_bytes([aml[next + 1], aml[next + 2]]);
                            self.namespace.insert(
                                &name,
                                AmlValue::PowerResource {
                                    system_level: level,
                                    resource_order: order,
                                },
                            );
                            let body_start = next + 3;
                            if body_start <= end {
                                self.namespace.push_scope(&name);
                                let _ = self.parse_term_list(&aml[body_start..end], 0).ok();
                                self.namespace.pop_scope();
                            }
                        } else {
                            crate::serial_println!(
                                "[acpi][warn] AML PowerResource object truncated"
                            );
                        }
                        Ok(end)
                    }
                    aml_opcodes::EXT_THERMAL_ZONE_OP => {
                        let (pkg_len, body_start) = match self.parse_pkg_length(aml, pos + 2) {
                            Ok(v) => v,
                            Err(_) => return Ok(pos.saturating_add(2).min(aml.len())),
                        };
                        let end = pos.saturating_add(2).saturating_add(pkg_len).min(aml.len());
                        let (name, body_pos) = self.parse_name_string(aml, body_start)?;
                        self.namespace
                            .insert(&name, AmlValue::ThermalZone(name.clone()));
                        if body_pos <= end {
                            self.namespace.push_scope(&name);
                            let _ = self.parse_term_list(&aml[body_pos..end], 0).ok();
                            self.namespace.pop_scope();
                        } else {
                            crate::serial_println!(
                                "[acpi][warn] AML ThermalZone body out of range"
                            );
                        }
                        Ok(end)
                    }
                    aml_opcodes::EXT_MUTEX_OP => {
                        let (name, next) = self.parse_name_string(aml, pos + 2)?;
                        if next < aml.len() {
                            let sync = aml[next] & 0x0F;
                            self.namespace
                                .insert(&name, AmlValue::Mutex { sync_level: sync });
                            Ok(next + 1)
                        } else {
                            Ok(aml.len())
                        }
                    }
                    aml_opcodes::EXT_EVENT_OP => {
                        let (name, next) = self.parse_name_string(aml, pos + 2)?;
                        self.namespace.insert(&name, AmlValue::Event);
                        Ok(next)
                    }
                    aml_opcodes::EXT_ACQUIRE_OP
                    | aml_opcodes::EXT_RELEASE_OP
                    | aml_opcodes::EXT_SIGNAL_OP
                    | aml_opcodes::EXT_RESET_OP => {
                        let next = self.skip_term_arg(aml, pos + 2)?;
                        Ok(next)
                    }
                    aml_opcodes::EXT_WAIT_OP => {
                        let next = self.skip_term_arg(aml, pos + 2)?;
                        let next = self.skip_term_arg(aml, next)?;
                        Ok(next)
                    }
                    aml_opcodes::EXT_STALL_OP | aml_opcodes::EXT_SLEEP_OP => {
                        let next = self.skip_term_arg(aml, pos + 2)?;
                        Ok(next)
                    }
                    aml_opcodes::EXT_FATAL_OP => {
                        if pos + 7 < aml.len() {
                            let next = self.skip_term_arg(aml, pos + 7)?;
                            Ok(next)
                        } else {
                            Ok(aml.len())
                        }
                    }
                    aml_opcodes::EXT_COND_REF_OF => {
                        let next = self.skip_term_arg(aml, pos + 2)?;
                        let next = self.skip_term_arg(aml, next)?;
                        Ok(next)
                    }
                    aml_opcodes::EXT_CREATE_FIELD => {
                        let next = self.skip_term_arg(aml, pos + 2)?;
                        let next = self.skip_term_arg(aml, next)?;
                        let next = self.skip_term_arg(aml, next)?;
                        let (_, next) = self.parse_name_string(aml, next)?;
                        Ok(next)
                    }
                    aml_opcodes::EXT_REVISION_OP => Ok(pos + 2),
                    _ => Ok(pos + 2),
                }
            }

            0x2E => Ok(pos + 9), // DualNamePath
            0x2F => {
                // MultiNamePath
                if pos + 1 < aml.len() {
                    let seg_count = aml[pos + 1] as usize;
                    Ok(pos + 2 + seg_count * 4)
                } else {
                    Ok(aml.len())
                }
            }

            0x41..=0x5A | 0x5F => {
                let (_, next) = self.parse_name_string(aml, pos)?;
                Ok(next)
            }

            _ => Ok(pos + 1),
        }
    }

    fn parse_name_string(&self, aml: &[u8], pos: usize) -> Result<(String, usize), AcpiError> {
        if pos >= aml.len() {
            return Ok((String::new(), pos));
        }
        let mut s = String::new();
        let mut p = pos;

        if p < aml.len() && aml[p] == b'\\' {
            s.push('\\');
            p += 1;
        }
        while p < aml.len() && aml[p] == b'^' {
            s.push('^');
            p += 1;
        }

        if p >= aml.len() {
            return Ok((s, p));
        }

        match aml[p] {
            0x00 => {
                p += 1;
            }
            0x2E => {
                p += 1;
                if p + 8 <= aml.len() {
                    let seg1 = core::str::from_utf8(&aml[p..p + 4]).unwrap_or("????");
                    p += 4;
                    let seg2 = core::str::from_utf8(&aml[p..p + 4]).unwrap_or("????");
                    p += 4;
                    s.push_str(seg1.trim_end_matches('_'));
                    s.push('.');
                    s.push_str(seg2.trim_end_matches('_'));
                }
            }
            0x2F => {
                p += 1;
                if p < aml.len() {
                    let count = aml[p] as usize;
                    p += 1;
                    for i in 0..count {
                        if p + 4 > aml.len() {
                            break;
                        }
                        let seg = core::str::from_utf8(&aml[p..p + 4]).unwrap_or("????");
                        p += 4;
                        if i > 0 {
                            s.push('.');
                        }
                        s.push_str(seg.trim_end_matches('_'));
                    }
                }
            }
            b'A'..=b'Z' | b'_' => {
                if p + 4 <= aml.len() {
                    let seg = core::str::from_utf8(&aml[p..p + 4]).unwrap_or("????");
                    p += 4;
                    s.push_str(seg.trim_end_matches('_'));
                } else {
                    while p < aml.len() && (aml[p].is_ascii_alphanumeric() || aml[p] == b'_') {
                        s.push(aml[p] as char);
                        p += 1;
                    }
                }
            }
            _ => {}
        }

        Ok((s, p))
    }

    fn parse_pkg_length(&self, aml: &[u8], pos: usize) -> Result<(usize, usize), AcpiError> {
        if pos >= aml.len() {
            return Err(AcpiError::AmlParseError(String::from("unexpected end")));
        }
        let lead = aml[pos];
        let byte_count = (lead >> 6) as usize;

        if byte_count == 0 {
            Ok(((lead & 0x3F) as usize, pos + 1))
        } else {
            let mut length = (lead & 0x0F) as usize;
            if pos + 1 + byte_count > aml.len() {
                return Err(AcpiError::AmlParseError(String::from(
                    "truncated PkgLength",
                )));
            }
            for i in 0..byte_count {
                length |= (aml[pos + 1 + i] as usize) << (4 + i * 8);
            }
            Ok((length, pos + 1 + byte_count))
        }
    }

    fn parse_data_object(&self, aml: &[u8], pos: usize) -> Result<(AmlValue, usize), AcpiError> {
        if pos >= aml.len() {
            return Ok((AmlValue::Uninitialized, pos));
        }
        match aml[pos] {
            aml_opcodes::ZERO_OP => Ok((AmlValue::Integer(0), pos + 1)),
            aml_opcodes::ONE_OP => Ok((AmlValue::Integer(1), pos + 1)),
            aml_opcodes::ONES_OP => Ok((AmlValue::Integer(u64::MAX), pos + 1)),
            aml_opcodes::BYTE_PREFIX if pos + 1 < aml.len() => {
                Ok((AmlValue::Integer(aml[pos + 1] as u64), pos + 2))
            }
            aml_opcodes::WORD_PREFIX if pos + 2 < aml.len() => Ok((
                AmlValue::Integer(u16::from_le_bytes([aml[pos + 1], aml[pos + 2]]) as u64),
                pos + 3,
            )),
            aml_opcodes::DWORD_PREFIX if pos + 4 < aml.len() => Ok((
                AmlValue::Integer(u32::from_le_bytes([
                    aml[pos + 1],
                    aml[pos + 2],
                    aml[pos + 3],
                    aml[pos + 4],
                ]) as u64),
                pos + 5,
            )),
            aml_opcodes::QWORD_PREFIX if pos + 8 < aml.len() => Ok((
                AmlValue::Integer(u64::from_le_bytes([
                    aml[pos + 1],
                    aml[pos + 2],
                    aml[pos + 3],
                    aml[pos + 4],
                    aml[pos + 5],
                    aml[pos + 6],
                    aml[pos + 7],
                    aml[pos + 8],
                ])),
                pos + 9,
            )),
            aml_opcodes::STRING_PREFIX => {
                let start = pos + 1;
                let end = aml[start..]
                    .iter()
                    .position(|&b| b == 0)
                    .map_or(aml.len(), |p| start + p);
                let s = core::str::from_utf8(&aml[start..end]).unwrap_or("");
                Ok((AmlValue::String(String::from(s)), end + 1))
            }
            aml_opcodes::BUFFER_OP => {
                let (pkg_len, _) = match self.parse_pkg_length(aml, pos + 1) {
                    Ok(v) => v,
                    Err(_) => {
                        crate::serial_println!("[acpi][warn] AML Buffer has truncated PkgLength");
                        return Ok((AmlValue::Uninitialized, aml.len()));
                    }
                };
                let start = pos.saturating_add(1);
                let end = start.saturating_add(pkg_len).min(aml.len());
                if end < start {
                    crate::serial_println!("[acpi][warn] AML Buffer range underflow");
                    return Ok((AmlValue::Uninitialized, aml.len()));
                }
                if start + pkg_len > aml.len() {
                    crate::serial_println!(
                        "[acpi][warn] AML Buffer truncated (wanted {}, have {})",
                        pkg_len,
                        aml.len().saturating_sub(start)
                    );
                }
                Ok((AmlValue::Buffer(aml[start..end].to_vec()), end))
            }
            aml_opcodes::PACKAGE_OP | aml_opcodes::VAR_PACKAGE_OP => {
                let (pkg_len, _) = match self.parse_pkg_length(aml, pos + 1) {
                    Ok(v) => v,
                    Err(_) => {
                        crate::serial_println!("[acpi][warn] AML Package has truncated PkgLength");
                        return Ok((AmlValue::Uninitialized, aml.len()));
                    }
                };
                Ok((
                    AmlValue::Package(Vec::new()),
                    pos.saturating_add(1).saturating_add(pkg_len).min(aml.len()),
                ))
            }
            _ => Ok((AmlValue::Uninitialized, pos + 1)),
        }
    }

    fn parse_integer(&self, aml: &[u8], pos: usize) -> Result<(u64, usize), AcpiError> {
        if pos >= aml.len() {
            return Ok((0, pos));
        }
        match aml[pos] {
            aml_opcodes::ZERO_OP => Ok((0, pos + 1)),
            aml_opcodes::ONE_OP => Ok((1, pos + 1)),
            aml_opcodes::ONES_OP => Ok((u64::MAX, pos + 1)),
            aml_opcodes::BYTE_PREFIX if pos + 1 < aml.len() => Ok((aml[pos + 1] as u64, pos + 2)),
            aml_opcodes::WORD_PREFIX if pos + 2 < aml.len() => Ok((
                u16::from_le_bytes([aml[pos + 1], aml[pos + 2]]) as u64,
                pos + 3,
            )),
            aml_opcodes::DWORD_PREFIX if pos + 4 < aml.len() => Ok((
                u32::from_le_bytes([aml[pos + 1], aml[pos + 2], aml[pos + 3], aml[pos + 4]]) as u64,
                pos + 5,
            )),
            aml_opcodes::QWORD_PREFIX if pos + 8 < aml.len() => Ok((
                u64::from_le_bytes([
                    aml[pos + 1],
                    aml[pos + 2],
                    aml[pos + 3],
                    aml[pos + 4],
                    aml[pos + 5],
                    aml[pos + 6],
                    aml[pos + 7],
                    aml[pos + 8],
                ]),
                pos + 9,
            )),
            _ => Ok((aml[pos] as u64, pos + 1)),
        }
    }

    fn skip_term_arg(&self, aml: &[u8], pos: usize) -> Result<usize, AcpiError> {
        if pos >= aml.len() {
            return Ok(pos);
        }
        match aml[pos] {
            aml_opcodes::ZERO_OP
            | aml_opcodes::ONE_OP
            | aml_opcodes::ONES_OP
            | aml_opcodes::NOOP_OP => Ok(pos + 1),
            aml_opcodes::BYTE_PREFIX => Ok(pos + 2),
            aml_opcodes::WORD_PREFIX => Ok(pos + 3),
            aml_opcodes::DWORD_PREFIX => Ok(pos + 5),
            aml_opcodes::QWORD_PREFIX => Ok(pos + 9),
            aml_opcodes::STRING_PREFIX => {
                let end = aml[pos + 1..]
                    .iter()
                    .position(|&b| b == 0)
                    .map_or(aml.len(), |p| pos + 1 + p);
                Ok(end + 1)
            }
            aml_opcodes::LOCAL0_OP..=aml_opcodes::LOCAL7_OP => Ok(pos + 1),
            aml_opcodes::ARG0_OP..=aml_opcodes::ARG6_OP => Ok(pos + 1),
            0x41..=0x5A | 0x5F | 0x5C | 0x5E => {
                let (_, next) = self.parse_name_string(aml, pos)?;
                Ok(next)
            }
            _ => Ok(pos + 1),
        }
    }

    pub fn evaluate_integer(&self, path: &str) -> Result<u64, AcpiError> {
        let node = self
            .namespace
            .get(path)
            .ok_or_else(|| AcpiError::MethodNotFound(String::from(path)))?;
        match &node.value {
            AmlValue::Integer(v) => Ok(*v),
            _ => Err(AcpiError::AmlEvalError(alloc::format!(
                "{} is not an integer",
                path
            ))),
        }
    }
}

// ─── Operation Region Handlers ──────────────────────────────────────────────

pub struct OpRegionHandler;

impl OpRegionHandler {
    pub unsafe fn read_system_memory(addr: u64, width: usize) -> u64 {
        match width {
            1 => core::ptr::read_volatile(addr as *const u8) as u64,
            2 => core::ptr::read_volatile(addr as *const u16) as u64,
            4 => core::ptr::read_volatile(addr as *const u32) as u64,
            8 => core::ptr::read_volatile(addr as *const u64),
            _ => 0,
        }
    }

    pub unsafe fn write_system_memory(addr: u64, value: u64, width: usize) {
        match width {
            1 => core::ptr::write_volatile(addr as *mut u8, value as u8),
            2 => core::ptr::write_volatile(addr as *mut u16, value as u16),
            4 => core::ptr::write_volatile(addr as *mut u32, value as u32),
            8 => core::ptr::write_volatile(addr as *mut u64, value),
            _ => {}
        }
    }

    pub unsafe fn read_system_io(port: u16, width: usize) -> u64 {
        match width {
            1 => {
                let v: u8;
                core::arch::asm!("in al, dx", out("al") v, in("dx") port);
                v as u64
            }
            2 => {
                let v: u16;
                core::arch::asm!("in ax, dx", out("ax") v, in("dx") port);
                v as u64
            }
            4 => {
                let v: u32;
                core::arch::asm!("in eax, dx", out("eax") v, in("dx") port);
                v as u64
            }
            _ => 0,
        }
    }

    pub unsafe fn write_system_io(port: u16, value: u64, width: usize) {
        match width {
            1 => core::arch::asm!("out dx, al", in("dx") port, in("al") value as u8),
            2 => core::arch::asm!("out dx, ax", in("dx") port, in("ax") value as u16),
            4 => core::arch::asm!("out dx, eax", in("dx") port, in("eax") value as u32),
            _ => {}
        }
    }
}

// ─── Power Management (Sleep States) ────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SleepState {
    S0,
    S1,
    S2,
    S3,
    S4,
    S5,
}

impl SleepState {
    pub fn object_name(self) -> &'static str {
        match self {
            Self::S0 => "\\_S0",
            Self::S1 => "\\_S1",
            Self::S2 => "\\_S2",
            Self::S3 => "\\_S3",
            Self::S4 => "\\_S4",
            Self::S5 => "\\_S5",
        }
    }
}

pub struct PowerManager {
    pub current_state: SleepState,
    pub supported_states: Vec<SleepState>,
    pub slp_typ_a: [u8; 6],
    pub slp_typ_b: [u8; 6],
}

impl PowerManager {
    pub fn new() -> Self {
        Self {
            current_state: SleepState::S0,
            supported_states: Vec::new(),
            slp_typ_a: [0; 6],
            slp_typ_b: [0; 6],
        }
    }

    pub fn discover_states(&mut self, aml_context: &mut aml::AmlContext) {
        use alloc::string::String;
        use aml::AmlValue;

        for (idx, state) in [
            SleepState::S0,
            SleepState::S1,
            SleepState::S2,
            SleepState::S3,
            SleepState::S4,
            SleepState::S5,
        ]
        .iter()
        .enumerate()
        {
            let path = aml::AmlName::from_str(state.object_name()).unwrap();

            // Try to evaluate the path
            if let Ok(value) = aml_context.namespace.get_by_path(&path) {
                self.supported_states.push(*state);

                // _S5 and other sleep states are typically Packages, e.g. Name(\_S5, Package() { 5, 5, 0, 0 })
                // We need to extract the first and second elements for SLP_TYPa and SLP_TYPb
                if let aml::AmlValue::Package(elements) = value {
                    if elements.len() >= 1 {
                        if let AmlValue::Integer(v) = elements[0] {
                            self.slp_typ_a[idx] = (v & 0xFF) as u8;
                        }
                    }
                    if elements.len() >= 2 {
                        if let AmlValue::Integer(v) = elements[1] {
                            self.slp_typ_b[idx] = (v & 0xFF) as u8;
                        }
                    }
                }
            }
        }
    }

    pub unsafe fn prepare_to_sleep(&self, fadt: &Fadt, state: SleepState) -> Result<(), AcpiError> {
        let idx = match state {
            SleepState::S0 => 0,
            SleepState::S1 => 1,
            SleepState::S2 => 2,
            SleepState::S3 => 3,
            SleepState::S4 => 4,
            SleepState::S5 => 5,
        };

        let slp_typ_a = self.slp_typ_a[idx] as u16;
        let val_a = (slp_typ_a << 10) | (1 << 13); // SLP_TYPx | SLP_EN
        let gas_a = fadt.pm1a_control_gas();
        gas_a.write_u16(0, val_a);

        let gas_b = fadt.pm1b_control_gas();
        if gas_b.is_valid() {
            let slp_typ_b = self.slp_typ_b[idx] as u16;
            let val_b = (slp_typ_b << 10) | (1 << 13);
            gas_b.write_u16(0, val_b);
        }
        Ok(())
    }

    pub unsafe fn shutdown(&self, fadt: &Fadt) -> Result<(), AcpiError> {
        self.prepare_to_sleep(fadt, SleepState::S5)
    }

    pub unsafe fn reset(&self, fadt: &Fadt) -> Result<(), AcpiError> {
        if fadt.reset_register.is_valid() {
            fadt.reset_register.write_u8(0, fadt.reset_value);
        }
        Ok(())
    }
}

// ─── Thermal Zone ───────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ThermalZone {
    pub name: String,
    pub temperature: u32,
    pub active_cooling: [u32; 10],
    pub passive_cooling: u32,
    pub critical_temp: u32,
    pub hot_temp: u32,
    pub tc1: u32,
    pub tc2: u32,
    pub tsp: u32,
    pub tzp: u32,
    pub cooling_devices: Vec<String>,
}

impl ThermalZone {
    pub fn new(name: &str) -> Self {
        Self {
            name: String::from(name),
            temperature: 0,
            active_cooling: [0; 10],
            passive_cooling: 0,
            critical_temp: 0,
            hot_temp: 0,
            tc1: 0,
            tc2: 0,
            tsp: 0,
            tzp: 0,
            cooling_devices: Vec::new(),
        }
    }

    pub fn temp_celsius(&self) -> f32 {
        (self.temperature as f32 - 2732.0) / 10.0
    }

    pub fn is_critical(&self) -> bool {
        self.critical_temp > 0 && self.temperature >= self.critical_temp
    }

    pub fn is_hot(&self) -> bool {
        self.hot_temp > 0 && self.temperature >= self.hot_temp
    }

    pub fn needs_passive_cooling(&self) -> bool {
        self.passive_cooling > 0 && self.temperature >= self.passive_cooling
    }

    pub fn active_trip_index(&self) -> Option<usize> {
        for (i, &trip) in self.active_cooling.iter().enumerate() {
            if trip > 0 && self.temperature >= trip {
                return Some(i);
            }
        }
        None
    }
}

// ─── Battery ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatteryState {
    Discharging = 0x01,
    Charging = 0x02,
    Critical = 0x04,
}

#[derive(Debug, Clone)]
pub struct BatteryInfo {
    pub power_unit: u32,
    pub design_capacity: u32,
    pub last_full_capacity: u32,
    pub battery_technology: u32,
    pub design_voltage: u32,
    pub design_capacity_warning: u32,
    pub design_capacity_low: u32,
    pub cycle_count: u32,
    pub measurement_accuracy: u32,
    pub max_sampling_time: u32,
    pub min_sampling_time: u32,
    pub max_averaging_interval: u32,
    pub min_averaging_interval: u32,
    pub granularity_1: u32,
    pub granularity_2: u32,
    pub model_number: String,
    pub serial_number: String,
    pub battery_type: String,
    pub oem_info: String,
}

#[derive(Debug, Clone)]
pub struct BatteryStatus {
    pub state: u32,
    pub present_rate: u32,
    pub remaining_capacity: u32,
    pub present_voltage: u32,
}

impl BatteryStatus {
    pub fn is_discharging(&self) -> bool {
        self.state & 0x01 != 0
    }
    pub fn is_charging(&self) -> bool {
        self.state & 0x02 != 0
    }
    pub fn is_critical(&self) -> bool {
        self.state & 0x04 != 0
    }

    pub fn percentage(&self, full_capacity: u32) -> u8 {
        if full_capacity == 0 {
            return 0;
        }
        ((self.remaining_capacity as u64 * 100) / full_capacity as u64).min(100) as u8
    }

    pub fn time_remaining_minutes(&self, full_capacity: u32) -> Option<u32> {
        if self.present_rate == 0 || !self.is_discharging() {
            return None;
        }
        Some((self.remaining_capacity as u64 * 60 / self.present_rate as u64) as u32)
    }
}

#[derive(Debug, Clone)]
pub struct PowerSource {
    pub online: bool,
    pub source_type: String,
}

// ─── Embedded Controller ────────────────────────────────────────────────────

const EC_SC: u16 = 0x66;
const EC_DATA: u16 = 0x62;

const EC_CMD_READ: u8 = 0x80;
const EC_CMD_WRITE: u8 = 0x81;
const EC_CMD_BURST_ENABLE: u8 = 0x82;
const EC_CMD_BURST_DISABLE: u8 = 0x83;
const EC_CMD_QUERY: u8 = 0x84;

const EC_OBF: u8 = 1 << 0;
const EC_IBF: u8 = 1 << 1;
const EC_CMD_FLAG: u8 = 1 << 3;
const EC_BURST: u8 = 1 << 4;
const EC_SCI_EVT: u8 = 1 << 5;

pub struct EmbeddedController {
    pub data_port: u16,
    pub command_port: u16,
    pub gpe_bit: u8,
    pub burst_mode: bool,
}

impl EmbeddedController {
    pub fn new() -> Self {
        Self {
            data_port: EC_DATA,
            command_port: EC_SC,
            gpe_bit: 0,
            burst_mode: false,
        }
    }

    pub fn with_ports(data: u16, cmd: u16, gpe: u8) -> Self {
        Self {
            data_port: data,
            command_port: cmd,
            gpe_bit: gpe,
            burst_mode: false,
        }
    }

    unsafe fn wait_ibf_clear(&self) -> Result<(), AcpiError> {
        for _ in 0..10000 {
            let status: u8;
            core::arch::asm!("in al, dx", out("al") status, in("dx") self.command_port);
            if status & EC_IBF == 0 {
                return Ok(());
            }
            core::hint::spin_loop();
        }
        Err(AcpiError::EcTimeout)
    }

    unsafe fn wait_obf_set(&self) -> Result<(), AcpiError> {
        for _ in 0..10000 {
            let status: u8;
            core::arch::asm!("in al, dx", out("al") status, in("dx") self.command_port);
            if status & EC_OBF != 0 {
                return Ok(());
            }
            core::hint::spin_loop();
        }
        Err(AcpiError::EcTimeout)
    }

    pub unsafe fn read(&self, addr: u8) -> Result<u8, AcpiError> {
        self.wait_ibf_clear()?;
        core::arch::asm!("out dx, al", in("dx") self.command_port, in("al") EC_CMD_READ);
        self.wait_ibf_clear()?;
        core::arch::asm!("out dx, al", in("dx") self.data_port, in("al") addr);
        self.wait_obf_set()?;
        let val: u8;
        core::arch::asm!("in al, dx", out("al") val, in("dx") self.data_port);
        Ok(val)
    }

    pub unsafe fn write(&self, addr: u8, value: u8) -> Result<(), AcpiError> {
        self.wait_ibf_clear()?;
        core::arch::asm!("out dx, al", in("dx") self.command_port, in("al") EC_CMD_WRITE);
        self.wait_ibf_clear()?;
        core::arch::asm!("out dx, al", in("dx") self.data_port, in("al") addr);
        self.wait_ibf_clear()?;
        core::arch::asm!("out dx, al", in("dx") self.data_port, in("al") value);
        Ok(())
    }

    pub unsafe fn burst_enable(&mut self) -> Result<(), AcpiError> {
        self.wait_ibf_clear()?;
        core::arch::asm!("out dx, al", in("dx") self.command_port, in("al") EC_CMD_BURST_ENABLE);
        self.wait_obf_set()?;
        let _ack: u8;
        core::arch::asm!("in al, dx", out("al") _ack, in("dx") self.data_port);
        self.burst_mode = true;
        Ok(())
    }

    pub unsafe fn burst_disable(&mut self) -> Result<(), AcpiError> {
        self.wait_ibf_clear()?;
        core::arch::asm!("out dx, al", in("dx") self.command_port, in("al") EC_CMD_BURST_DISABLE);
        self.burst_mode = false;
        Ok(())
    }

    pub unsafe fn query(&self) -> Result<u8, AcpiError> {
        self.wait_ibf_clear()?;
        core::arch::asm!("out dx, al", in("dx") self.command_port, in("al") EC_CMD_QUERY);
        self.wait_obf_set()?;
        let val: u8;
        core::arch::asm!("in al, dx", out("al") val, in("dx") self.data_port);
        Ok(val)
    }

    pub unsafe fn has_sci_event(&self) -> bool {
        let status: u8;
        core::arch::asm!("in al, dx", out("al") status, in("dx") self.command_port);
        status & EC_SCI_EVT != 0
    }
}

// ─── GPE (General Purpose Events) ───────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpeTrigger {
    Edge,
    Level,
}

#[derive(Debug, Clone)]
pub struct GpeBlock {
    pub gas: GenericAddress,
    pub register_count: u8,
    pub base_gpe: u8,
}

pub struct GpeSubsystem {
    pub blocks: Vec<GpeBlock>,
    pub handlers: BTreeMap<u8, GpeHandler>,
    pub wake_mask: Vec<u8>,
    pub runtime_mask: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct GpeHandler {
    pub gpe_number: u8,
    pub trigger: GpeTrigger,
    pub method_name: String,
    pub wake_capable: bool,
    pub runtime: bool,
    pub count: u64,
}

impl GpeSubsystem {
    pub fn new() -> Self {
        Self {
            blocks: Vec::new(),
            handlers: BTreeMap::new(),
            wake_mask: Vec::new(),
            runtime_mask: Vec::new(),
        }
    }

    pub fn setup_from_fadt(&mut self, fadt: &Fadt) {
        let gpe0 = fadt.gpe0_block_gas();
        if gpe0.is_valid() && fadt.gpe0_length > 0 {
            self.blocks.push(GpeBlock {
                gas: gpe0,
                register_count: fadt.gpe0_length / 2,
                base_gpe: 0,
            });
        }
        let gpe1 = fadt.gpe1_block_gas();
        if gpe1.is_valid() && fadt.gpe1_length > 0 {
            self.blocks.push(GpeBlock {
                gas: gpe1,
                register_count: fadt.gpe1_length / 2,
                base_gpe: fadt.gpe1_base,
            });
        }
    }

    pub fn register_handler(&mut self, gpe: u8, trigger: GpeTrigger, method: &str, wake: bool) {
        self.handlers.insert(
            gpe,
            GpeHandler {
                gpe_number: gpe,
                trigger,
                method_name: String::from(method),
                wake_capable: wake,
                runtime: true,
                count: 0,
            },
        );
    }

    pub unsafe fn enable_gpe(&self, gpe: u8) {
        for block in &self.blocks {
            let max_gpe = block.base_gpe + block.register_count * 8;
            if gpe >= block.base_gpe && gpe < max_gpe {
                let reg_idx = ((gpe - block.base_gpe) / 8) as u64;
                let bit = (gpe - block.base_gpe) % 8;
                let offset = (block.register_count as u64) + reg_idx;
                let val = block.gas.read_u8(offset);
                let new_val = val | (1 << bit);
                block.gas.write_u8(offset, new_val);
                return;
            }
        }
    }

    pub unsafe fn disable_gpe(&self, gpe: u8) {
        for block in &self.blocks {
            let max_gpe = block.base_gpe + block.register_count * 8;
            if gpe >= block.base_gpe && gpe < max_gpe {
                let reg_idx = ((gpe - block.base_gpe) / 8) as u64;
                let bit = (gpe - block.base_gpe) % 8;
                let offset = (block.register_count as u64) + reg_idx;
                let val = block.gas.read_u8(offset);
                let new_val = val & !(1 << bit);
                block.gas.write_u8(offset, new_val);
                return;
            }
        }
    }

    pub unsafe fn clear_gpe_status(&self, gpe: u8) {
        for block in &self.blocks {
            let max_gpe = block.base_gpe + block.register_count * 8;
            if gpe >= block.base_gpe && gpe < max_gpe {
                let reg_idx = ((gpe - block.base_gpe) / 8) as u64;
                let bit = (gpe - block.base_gpe) % 8;
                let offset = reg_idx;
                block.gas.write_u8(offset, 1u8 << bit);
                return;
            }
        }
    }

    pub unsafe fn dispatch(&mut self) {
        for block in &self.blocks {
            for reg in 0..block.register_count as u64 {
                let status_offset = reg;
                let enable_offset = (block.register_count as u64) + reg;
                let status = block.gas.read_u8(status_offset);
                let enable = block.gas.read_u8(enable_offset);
                let active = status & enable;
                for bit in 0..8u8 {
                    if active & (1 << bit) != 0 {
                        let gpe = block.base_gpe + (reg as u8 * 8) + bit;
                        block.gas.write_u8(status_offset, 1u8 << bit);
                        if let Some(handler) = self.handlers.get_mut(&gpe) {
                            handler.count += 1;
                        }
                    }
                }
            }
        }
    }
}

// ─── Processor Performance ──────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct CState {
    pub c_type: u8,
    pub latency: u32,
    pub power: u32,
    pub register: GenericAddress,
}

#[derive(Debug, Clone)]
pub struct PState {
    pub core_frequency: u32,
    pub power: u32,
    pub transition_latency: u32,
    pub bus_master_latency: u32,
    pub control: u64,
    pub status: u64,
}

#[derive(Debug, Clone)]
pub struct TState {
    pub percent: u32,
    pub power: u32,
    pub transition_latency: u32,
    pub control: u32,
    pub status: u32,
}

#[derive(Debug, Clone)]
pub struct CppcRegisters {
    pub highest_performance: u32,
    pub nominal_performance: u32,
    pub lowest_nonlinear_performance: u32,
    pub lowest_performance: u32,
    pub guaranteed_performance: u32,
    pub desired_performance: GenericAddress,
    pub minimum_performance: GenericAddress,
    pub maximum_performance: GenericAddress,
    pub performance_reduction_tolerance: GenericAddress,
    pub time_window: GenericAddress,
    pub counter_wraparound_time: GenericAddress,
    pub reference_performance_counter: GenericAddress,
    pub delivered_performance_counter: GenericAddress,
    pub performance_limited: GenericAddress,
    pub cppc_enable: GenericAddress,
    pub autonomous_selection_enable: GenericAddress,
    pub autonomous_activity_window: GenericAddress,
    pub energy_performance_preference: GenericAddress,
    pub reference_performance: GenericAddress,
    pub lowest_frequency: u32,
    pub nominal_frequency: u32,
}

#[derive(Debug, Clone)]
pub struct ProcessorPerformance {
    pub c_states: Vec<CState>,
    pub p_states: Vec<PState>,
    pub t_states: Vec<TState>,
    pub cppc: Option<CppcRegisters>,
    pub ppc_limit: u8,
    pub tpc_limit: u8,
}

impl ProcessorPerformance {
    pub fn new() -> Self {
        Self {
            c_states: Vec::new(),
            p_states: Vec::new(),
            t_states: Vec::new(),
            cppc: None,
            ppc_limit: 0,
            tpc_limit: 0,
        }
    }

    pub fn deepest_c_state(&self) -> Option<&CState> {
        self.c_states.iter().max_by_key(|c| c.c_type)
    }

    pub fn highest_p_state(&self) -> Option<&PState> {
        self.p_states.first()
    }

    pub fn lowest_p_state(&self) -> Option<&PState> {
        self.p_states.last()
    }

    pub fn available_p_states(&self) -> &[PState] {
        let limit = self.ppc_limit as usize;
        if limit < self.p_states.len() {
            &self.p_states[limit..]
        } else {
            &self.p_states
        }
    }

    pub fn available_t_states(&self) -> &[TState] {
        let limit = self.tpc_limit as usize;
        if limit < self.t_states.len() {
            &self.t_states[limit..]
        } else {
            &self.t_states
        }
    }
}

// ─── SRAT (System Resource Affinity Table) ──────────────────────────────────

#[derive(Debug, Clone)]
pub enum SratEntry {
    ProcessorLocalApic {
        domain: u32,
        apic_id: u8,
        flags: u32,
    },
    MemoryAffinity {
        domain: u32,
        base: u64,
        length: u64,
        flags: u32,
    },
    ProcessorLocalX2Apic {
        domain: u32,
        x2apic_id: u32,
        flags: u32,
    },
    GicAffinity {
        domain: u32,
        gic_id: u32,
    },
}

pub fn parse_srat(addr: u64) -> Vec<SratEntry> {
    let mut entries = Vec::new();
    unsafe {
        let ptr = addr as *const u8;
        let length = *(ptr.add(4) as *const u32) as usize;
        let mut offset = 48;
        while offset + 2 <= length {
            let entry_type = *ptr.add(offset);
            let entry_len = *ptr.add(offset + 1) as usize;
            if entry_len < 2 || offset + entry_len > length {
                break;
            }
            let e = ptr.add(offset);
            match entry_type {
                0 if entry_len >= 16 => {
                    let lo = *e.add(2) as u32;
                    let hi = *e.add(9) as u32;
                    entries.push(SratEntry::ProcessorLocalApic {
                        domain: lo | (hi << 8),
                        apic_id: *e.add(3),
                        flags: *(e.add(4) as *const u32),
                    });
                }
                1 if entry_len >= 40 => {
                    entries.push(SratEntry::MemoryAffinity {
                        domain: *(e.add(2) as *const u32),
                        base: (*(e.add(8) as *const u32) as u64)
                            | ((*(e.add(12) as *const u32) as u64) << 32),
                        length: (*(e.add(16) as *const u32) as u64)
                            | ((*(e.add(20) as *const u32) as u64) << 32),
                        flags: *(e.add(28) as *const u32),
                    });
                }
                2 if entry_len >= 24 => {
                    entries.push(SratEntry::ProcessorLocalX2Apic {
                        domain: *(e.add(4) as *const u32),
                        x2apic_id: *(e.add(8) as *const u32),
                        flags: *(e.add(12) as *const u32),
                    });
                }
                _ => {}
            }
            offset += entry_len;
        }
    }
    entries
}

// ─── SLIT (System Locality Information Table) ────────────────────────────────

#[derive(Debug)]
pub struct Slit {
    pub localities: u64,
    pub distances: Vec<u8>,
}

impl Slit {
    pub unsafe fn parse(addr: u64) -> Result<Self, AcpiError> {
        let ptr = addr as *const u8;
        let length = *(ptr.add(4) as *const u32) as usize;
        if length < 44 {
            return Err(AcpiError::InvalidTable);
        }
        let localities = *(ptr.add(36) as *const u64);
        let count = (localities * localities) as usize;
        if 44usize.saturating_add(count) > length {
            crate::serial_println!(
                "[acpi][warn] SLIT truncated: localities={}, need {} bytes, table len {}",
                localities,
                44usize.saturating_add(count),
                length
            );
            return Err(AcpiError::InvalidTable);
        }
        let mut distances = Vec::with_capacity(count);
        for i in 0..count {
            distances.push(*ptr.add(44 + i));
        }
        Ok(Self {
            localities,
            distances,
        })
    }

    pub fn distance(&self, from: u64, to: u64) -> u8 {
        if from >= self.localities || to >= self.localities {
            return 255;
        }
        self.distances[(from * self.localities + to) as usize]
    }
}

// ─── HPET Table ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct HpetTable {
    pub event_timer_block_id: u32,
    pub base_address: GenericAddress,
    pub hpet_number: u8,
    pub min_clock_tick: u16,
    pub page_protection: u8,
}

impl HpetTable {
    pub unsafe fn parse(addr: u64) -> Result<Self, AcpiError> {
        let ptr = addr as *const u8;
        Ok(Self {
            event_timer_block_id: *(ptr.add(36) as *const u32),
            base_address: GenericAddress::parse(addr + 40),
            hpet_number: *ptr.add(52),
            min_clock_tick: *(ptr.add(53) as *const u16),
            page_protection: *ptr.add(55),
        })
    }
}

// ─── BGRT (Boot Graphics Resource Table) ────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Bgrt {
    pub version: u16,
    pub status: u8,
    pub image_type: u8,
    pub image_address: u64,
    pub image_offset_x: u32,
    pub image_offset_y: u32,
}

impl Bgrt {
    pub unsafe fn parse(addr: u64) -> Result<Self, AcpiError> {
        let ptr = addr as *const u8;
        Ok(Self {
            version: *(ptr.add(36) as *const u16),
            status: *ptr.add(38),
            image_type: *ptr.add(39),
            image_address: *(ptr.add(40) as *const u64),
            image_offset_x: *(ptr.add(48) as *const u32),
            image_offset_y: *(ptr.add(52) as *const u32),
        })
    }
}

// ─── MCFG Table (re-exported for PCIe) ──────────────────────────────────────

#[derive(Debug, Clone)]
pub struct McfgTable {
    pub entries: Vec<McfgTableEntry>,
}

#[derive(Debug, Clone)]
pub struct McfgTableEntry {
    pub base_address: u64,
    pub segment_group: u16,
    pub start_bus: u8,
    pub end_bus: u8,
}

impl McfgTable {
    pub unsafe fn parse(addr: u64) -> Result<Self, AcpiError> {
        let ptr = addr as *const u8;
        let length = *(ptr.add(4) as *const u32) as usize;
        if length < 44 {
            crate::serial_println!("[acpi][warn] MCFG length {} too small", length);
            return Err(AcpiError::InvalidTable);
        }
        let entry_count = (length - 44) / 16;
        let mut entries = Vec::new();
        for i in 0..entry_count {
            let e = ptr.add(44 + i * 16);
            entries.push(McfgTableEntry {
                base_address: *(e as *const u64),
                segment_group: *(e.add(8) as *const u16),
                start_bus: *e.add(10),
                end_bus: *e.add(11),
            });
        }
        Ok(Self { entries })
    }
}

// ─── DBG2 (Debug Port Table 2) ─────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Dbg2Table {
    pub device_count: u32,
}

impl Dbg2Table {
    pub unsafe fn parse(addr: u64) -> Result<Self, AcpiError> {
        let ptr = addr as *const u8;
        Ok(Self {
            device_count: *(ptr.add(36) as *const u32),
        })
    }
}

// ─── DMAR (DMA Remapping) ───────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct DmarTable {
    pub host_address_width: u8,
    pub flags: u8,
    pub entries: Vec<DmarRemappingEntry>,
}

#[derive(Debug, Clone)]
pub enum DmarRemappingEntry {
    Drhd {
        flags: u8,
        segment: u16,
        register_base: u64,
    },
    Rmrr {
        segment: u16,
        base: u64,
        limit: u64,
    },
    Atsr {
        flags: u8,
        segment: u16,
    },
    Rhsa {
        register_base: u64,
        proximity_domain: u32,
    },
    Andd {
        device_number: u8,
        object_name: String,
    },
}

impl DmarTable {
    pub unsafe fn parse(addr: u64) -> Result<Self, AcpiError> {
        let ptr = addr as *const u8;
        let length = *(ptr.add(4) as *const u32) as usize;
        let haw = *ptr.add(36);
        let flags = *ptr.add(37);
        let mut entries = Vec::new();
        let mut offset = 48;
        while offset + 4 <= length {
            let entry_type = *(ptr.add(offset) as *const u16);
            let entry_len = *(ptr.add(offset + 2) as *const u16) as usize;
            if entry_len < 4 || offset + entry_len > length {
                break;
            }
            let e = ptr.add(offset);
            match entry_type {
                0 if entry_len >= 16 => {
                    entries.push(DmarRemappingEntry::Drhd {
                        flags: *e.add(4),
                        segment: *(e.add(6) as *const u16),
                        register_base: *(e.add(8) as *const u64),
                    });
                }
                1 if entry_len >= 24 => {
                    entries.push(DmarRemappingEntry::Rmrr {
                        segment: *(e.add(6) as *const u16),
                        base: *(e.add(8) as *const u64),
                        limit: *(e.add(16) as *const u64),
                    });
                }
                2 if entry_len >= 8 => {
                    entries.push(DmarRemappingEntry::Atsr {
                        flags: *e.add(4),
                        segment: *(e.add(6) as *const u16),
                    });
                }
                3 if entry_len >= 16 => {
                    entries.push(DmarRemappingEntry::Rhsa {
                        register_base: *(e.add(4) as *const u64),
                        proximity_domain: *(e.add(12) as *const u32),
                    });
                }
                _ => {}
            }
            offset += entry_len;
        }
        Ok(Self {
            host_address_width: haw,
            flags,
            entries,
        })
    }
}

// ─── ACPI Subsystem ─────────────────────────────────────────────────────────

pub struct AcpiSubsystem {
    pub initialized: bool,
    pub rsdp: Option<Rsdp>,
    pub tables: TableRegistry,
    pub fadt: Option<Fadt>,
    pub madt: Option<Madt>,
    pub interpreter: AmlInterpreter,
    pub aml_context: Option<aml::AmlContext>,
    pub power_manager: PowerManager,
    pub thermal_zones: Vec<ThermalZone>,
    pub batteries: Vec<BatteryInfo>,
    pub battery_status: Vec<BatteryStatus>,
    pub power_sources: Vec<PowerSource>,
    pub ec: Option<EmbeddedController>,
    pub gpe: GpeSubsystem,
    pub processor_perf: Vec<ProcessorPerformance>,
    /// Full paths of Device()/Processor() nodes in the authoritative `aml`
    /// crate namespace, cached at init. The hand-rolled interpreter no longer
    /// parses firmware AML, so its namespace must not be used for enumeration.
    pub namespace_devices: Vec<String>,
}

impl AcpiSubsystem {
    pub const fn new() -> Self {
        Self {
            initialized: false,
            rsdp: None,
            tables: TableRegistry { tables: Vec::new() },
            fadt: None,
            madt: None,
            aml_context: None,
            interpreter: AmlInterpreter {
                namespace: AmlNamespace {
                    nodes: BTreeMap::new(),
                    scope_stack: Vec::new(),
                },
                locals: [
                    AmlValue::Uninitialized,
                    AmlValue::Uninitialized,
                    AmlValue::Uninitialized,
                    AmlValue::Uninitialized,
                    AmlValue::Uninitialized,
                    AmlValue::Uninitialized,
                    AmlValue::Uninitialized,
                    AmlValue::Uninitialized,
                ],
                args: [
                    AmlValue::Uninitialized,
                    AmlValue::Uninitialized,
                    AmlValue::Uninitialized,
                    AmlValue::Uninitialized,
                    AmlValue::Uninitialized,
                    AmlValue::Uninitialized,
                    AmlValue::Uninitialized,
                ],
                revision: 2,
                ops_budget: AML_MAX_OPS,
                recursion_depth: 0,
            },
            power_manager: PowerManager {
                current_state: SleepState::S0,
                supported_states: Vec::new(),
                slp_typ_a: [0; 6],
                slp_typ_b: [0; 6],
            },
            thermal_zones: Vec::new(),
            batteries: Vec::new(),
            battery_status: Vec::new(),
            power_sources: Vec::new(),
            ec: None,
            gpe: GpeSubsystem {
                blocks: Vec::new(),
                handlers: BTreeMap::new(),
                wake_mask: Vec::new(),
                runtime_mask: Vec::new(),
            },
            processor_perf: Vec::new(),
            namespace_devices: Vec::new(),
        }
    }

    pub fn init(&mut self, rsdp_addr: u64) {
        if self.initialized || rsdp_addr == 0 {
            return;
        }

        let phys_off = crate::memory::PHYS_MEM_OFFSET
            .get()
            .map(|v| v.as_u64())
            .unwrap_or(0);

        unsafe {
            if let Ok(rsdp) = Rsdp::parse(phys_off + rsdp_addr) {
                if rsdp.is_v2 && rsdp.xsdt_address != 0 {
                    self.tables.load_xsdt(rsdp.xsdt_address, phys_off);
                } else {
                    self.tables.load_rsdt(rsdp.rsdt_address, phys_off);
                }
                self.rsdp = Some(rsdp);
            }

            if let Some(facp) = self.tables.find(&SIG_FACP) {
                if let Ok(fadt) = Fadt::parse(facp.address) {
                    self.gpe.setup_from_fadt(&fadt);

                    let pm1a = fadt.pm1a_control_gas();
                    let gpe0 = fadt.gpe0_block_gas();
                    crate::serial_println!(
                        "[acpi] PM registers: PM1a_CNT={:#x} GPE0={:#x} (source: {})",
                        pm1a.address,
                        gpe0.address,
                        if fadt.x_pm1a_control_block.is_valid() {
                            "X-GAS"
                        } else {
                            "Legacy"
                        }
                    );

                    self.fadt = Some(fadt);
                }
            }

            if let Some(apic) = self.tables.find(&SIG_APIC) {
                if let Ok(madt) = Madt::parse(apic.address) {
                    self.madt = Some(madt);
                }
            }

            self.interpreter = AmlInterpreter::new();

            // Initialize the rust-osdev aml context
            let mut aml_context = aml::AmlContext::new(
                alloc::boxed::Box::new(crate::aml_bridge::RaeAmlHandler),
                aml::DebugVerbosity::None,
            );

            // OS identification will be audited in apply_platform_bringup.
            // Requirement 2: Explicitly handle _OSI calls to report "Windows 2020" as supported
            // and "Linux" as unsupported.

            if let Some(fadt) = &self.fadt {
                let dsdt_addr = fadt.dsdt_phys();
                if dsdt_addr != 0 {
                    let dsdt_virt = phys_off + dsdt_addr;
                    let hdr_ptr = dsdt_virt as *const u8;
                    let dsdt_len = *(hdr_ptr.add(4) as *const u32) as usize;
                    if dsdt_len > SdtHeader::SIZE && dsdt_len < 1024 * 1024 {
                        // Debug repro: substitute a real-hardware DSDT dumped on the
                        // target machine so QEMU exercises the exact AML bytes that
                        // crash on metal. Off in every shipping build.
                        #[cfg(feature = "embed_test_dsdt")]
                        let aml_bytes: &[u8] = {
                            const EMBEDDED: &[u8] = include_bytes!(concat!(
                                env!("CARGO_MANIFEST_DIR"),
                                "/../target/dsdt.dat"
                            ));
                            crate::serial_println!(
                                "[acpi][TEST] parsing EMBEDDED real-hw DSDT ({} bytes) instead of firmware",
                                EMBEDDED.len()
                            );
                            EMBEDDED
                        };
                        #[cfg(not(feature = "embed_test_dsdt"))]
                        let aml_bytes = core::slice::from_raw_parts(hdr_ptr, dsdt_len);
                        // The mature `aml` crate is the authoritative interpreter. The
                        // hand-rolled `self.interpreter` corrupts the heap when fed a
                        // complex real-firmware DSDT/SSDT set (reproduced: stray free-list
                        // write -> later non-canonical-pointer / BTreeMap-navigate panic),
                        // so it no longer parses firmware AML. See parse below.
                        // MasterChecklist Phase 1.4: migrate ACPI consumers off the custom
                        // namespace onto aml_context, then delete the custom interpreter.
                        // The aml crate's parse_table wants the AML BODY — the
                        // 36-byte SDT header is not AML. The old code passed
                        // the full table (which ALWAYS failed: the parser read
                        // the "DSDT" signature bytes as a name string; QEMU's
                        // 55 namespace devices came entirely from the silent
                        // headerless "fallback" retry). Headerless is now the
                        // one true call, and its error is NEVER swallowed: on
                        // Athena a silent DSDT failure cost four flash rounds
                        // with "0 namespace devices" as the only distant
                        // symptom (no _PRT/EC/battery on iron).
                        let body = &aml_bytes[SdtHeader::SIZE.min(aml_bytes.len())..];
                        match aml_context.parse_table(body) {
                            Ok(()) => crate::serial_println!(
                                "[acpi] DSDT parsed: {} bytes of AML",
                                body.len()
                            ),
                            Err(e) => {
                                crate::serial_println!(
                                    "[acpi] DSDT parse FAILED ({} bytes of AML): {:?}",
                                    body.len(),
                                    e
                                );
                                // Self-dump: the raw table reaches the dev
                                // machine via BOOTLOG.TXT for off-target
                                // reproduction (see dump_table_to_bootlog).
                                dump_table_to_bootlog("dsdt.dat", aml_bytes);
                            }
                        }
                    }
                }
            }

            // Debug repro: parse the real machine's full SSDT set (dumped via
            // acpidump) so QEMU reproduces real-DSDT+SSDT-only ACPI bugs.
            #[cfg(feature = "embed_test_dsdt")]
            let ssdt_count = {
                macro_rules! ssdt {
                    ($f:literal) => {
                        include_bytes!(concat!(
                            env!("CARGO_MANIFEST_DIR"),
                            "/../target/acpi_dump/",
                            $f
                        )) as &[u8]
                    };
                }
                const EMBEDDED_SSDTS: &[&[u8]] = &[
                    ssdt!("ssdt.dat"),
                    ssdt!("ssdt1.dat"),
                    ssdt!("ssdt2.dat"),
                    ssdt!("ssdt3.dat"),
                    ssdt!("ssdt4.dat"),
                    ssdt!("ssdt5.dat"),
                    ssdt!("ssdt6.dat"),
                    ssdt!("ssdt7.dat"),
                    ssdt!("ssdt8.dat"),
                    ssdt!("ssdt9.dat"),
                    ssdt!("ssdt10.dat"),
                    ssdt!("ssdt11.dat"),
                    ssdt!("ssdt12.dat"),
                    ssdt!("ssdt13.dat"),
                    ssdt!("ssdt14.dat"),
                    ssdt!("ssdt15.dat"),
                    ssdt!("ssdt16.dat"),
                    ssdt!("ssdt17.dat"),
                    ssdt!("ssdt18.dat"),
                    ssdt!("ssdt19.dat"),
                    ssdt!("ssdt20.dat"),
                    ssdt!("ssdt21.dat"),
                ];
                for (i, aml_bytes) in EMBEDDED_SSDTS.iter().enumerate() {
                    crate::serial_println!(
                        "[acpi][TEST] parsing EMBEDDED SSDT #{} ({} bytes)",
                        i,
                        aml_bytes.len()
                    );
                    // Custom interpreter retired from firmware parsing (see DSDT above).
                    // Headerless body, same rule as everywhere else: the dumps are
                    // full SDTs (36-byte header + AML); parse_table wants only AML.
                    let body = &aml_bytes[SdtHeader::SIZE.min(aml_bytes.len())..];
                    match aml_context.parse_table(body) {
                        Ok(()) => {}
                        Err(e) => crate::serial_println!(
                            "[acpi][TEST] EMBEDDED SSDT #{} parse FAILED: {:?}",
                            i,
                            e
                        ),
                    }
                }
                EMBEDDED_SSDTS.len()
            };
            #[cfg(not(feature = "embed_test_dsdt"))]
            let ssdt_count = {
                let ssdts = self.tables.find_all(&SIG_SSDT);
                let ssdt_count = ssdts.len();
                for (i, ssdt) in ssdts.into_iter().enumerate() {
                    let hdr_ptr = ssdt.address as *const u8;
                    let ssdt_len = *(hdr_ptr.add(4) as *const u32) as usize;
                    if ssdt_len > SdtHeader::SIZE {
                        let aml_bytes = core::slice::from_raw_parts(hdr_ptr, ssdt_len);
                        // Headerless body, same as the DSDT above — passing
                        // the SDT header meant EVERY SSDT silently failed to
                        // parse on every platform until now.
                        let body = &aml_bytes[SdtHeader::SIZE..];
                        // OEM table id (header bytes 16..24) names the SSDT's
                        // function on real firmware (e.g. AMD "CPUSSDT") —
                        // include it so a failure names the table, not just an
                        // index. Errors must be visible (see DSDT above).
                        let oem_id: [u8; 8] =
                            core::ptr::read_unaligned(hdr_ptr.add(16) as *const [u8; 8]);
                        let oem = core::str::from_utf8(&oem_id).unwrap_or("????????");
                        match aml_context.parse_table(body) {
                            Ok(()) => crate::serial_println!(
                                "[acpi] SSDT #{} (\"{}\") parsed: {} bytes of AML",
                                i,
                                oem.trim_end(),
                                body.len()
                            ),
                            Err(e) => {
                                crate::serial_println!(
                                    "[acpi] SSDT #{} (\"{}\", {} bytes) parse FAILED: {:?}",
                                    i,
                                    oem.trim_end(),
                                    ssdt_len,
                                    e
                                );
                                let label = alloc::format!("ssdt{}.dat", i);
                                dump_table_to_bootlog(&label, aml_bytes);
                            }
                        }
                    }
                }
                ssdt_count
            };
            crate::serial_println!(
                "[acpi] AML tables: DSDT{} + {} SSDT(s) parsed into interpreter",
                if self
                    .fadt
                    .as_ref()
                    .map(|f| f.dsdt_phys() != 0)
                    .unwrap_or(false)
                {
                    ""
                } else {
                    " (missing)"
                },
                ssdt_count
            );

            // Negotiate PCIe native control via _OSC before continuing.
            // We ignore errors if the platform doesn't support _OSC or refuses,
            // but we at least try so that real hardware unlocks ECAM/MSI-X.
            let _ = negotiate_pcie_control(&mut aml_context);

            apply_platform_bringup(&mut aml_context);

            self.aml_context = Some(aml_context);

            self.power_manager
                .discover_states(self.aml_context.as_mut().unwrap());
        }

        // Cache device enumeration from the aml-crate namespace (the only one
        // populated from firmware AML) so counts/procfs report reality.
        self.collect_namespace_devices();

        // Phase 1.4 byte-capture net. The `aml` crate can return Ok(()) from
        // parse_table while populating *nothing* — observed on Athena's AMD
        // firmware: the count print reports "DSDT + 22 SSDT(s)" yet the walk
        // finds 0 Device objects, and `_SB.<dev>._REG`/`_PRT` all resolve to
        // ValueDoesNotExist on iron. The existing self-dump fires ONLY on a
        // parse Err, so those silent empty-namespace boots reached the dev host
        // with ZERO raw AML to replay — four blind Athena flashes saw "0
        // namespace devices" as the only symptom. When the namespace comes up
        // empty despite tables being present, dump every AML table (DSDT first,
        // then each SSDT) into the bootlog so ONE flash hands back the exact
        // bytes for `embed_test_dsdt` reproduction in QEMU. Healthy boots
        // (QEMU: 55 devices) never enter this branch, so there is no behavior
        // or boot-time change on the normal path; `dump_table_to_bootlog`
        // self-limits to ACPI_DUMP_BUDGET (600 KiB), so the boot transcript is
        // not evicted even by a 23-table set.
        if self.namespace_devices.is_empty() {
            let ssdts = self.tables.find_all(&SIG_SSDT);
            crate::serial_println!(
                "[acpi] namespace EMPTY after parsing {} table(s) — self-dumping raw AML for replay (Phase 1.4)",
                self.fadt.as_ref().map_or(0, |f| (f.dsdt_phys() != 0) as usize) + ssdts.len()
            );
            unsafe {
                if let Some(fadt) = &self.fadt {
                    let dsdt_addr = fadt.dsdt_phys();
                    if dsdt_addr != 0 {
                        let hdr_ptr = (phys_off + dsdt_addr) as *const u8;
                        let dsdt_len = *(hdr_ptr.add(4) as *const u32) as usize;
                        if dsdt_len > SdtHeader::SIZE && dsdt_len < 1024 * 1024 {
                            let bytes = core::slice::from_raw_parts(hdr_ptr, dsdt_len);
                            dump_table_to_bootlog("dsdt.dat", bytes);
                        }
                    }
                }
                for (i, ssdt) in ssdts.into_iter().enumerate() {
                    let hdr_ptr = ssdt.address as *const u8;
                    let ssdt_len = *(hdr_ptr.add(4) as *const u32) as usize;
                    if ssdt_len > SdtHeader::SIZE && ssdt_len < 1024 * 1024 {
                        let bytes = core::slice::from_raw_parts(hdr_ptr, ssdt_len);
                        let label = alloc::format!("ssdt{}.dat", i);
                        dump_table_to_bootlog(&label, bytes);
                    }
                }
            }
        }

        self.initialized = true;

        // Phase 1.8: PCI IRQ routing scan
        self.scan_pci_routing();

        // Phase 1.9: Embedded Controller (EC) Support
        self.discover_ec();
    }

    pub fn table_count(&self) -> usize {
        self.tables.tables.len()
    }

    pub fn has_table(&self, sig: &[u8; 4]) -> bool {
        self.tables.find(sig).is_some()
    }
}

#[derive(Debug, Clone)]
pub struct PciRoutingEntry {
    pub address: u64,
    pub pin: u8,
    pub source: aml::AmlValue,
    pub source_index: u32,
}

#[repr(C, packed)]
pub struct McfgEntry {
    pub base_address: u64,
    pub pci_segment: u16,
    pub start_bus: u8,
    pub end_bus: u8,
    pub reserved: u32,
}

// (Duplicate BatteryInfo struct removed during regression-fix — canonical
//  definition lives at line 1758 with the full 19-field _BIF surface.)

impl AcpiSubsystem {
    pub fn parse_mcfg(&mut self) -> Option<u64> {
        let table = self.tables.find(&SIG_MCFG)?;
        let ptr = table.address as *const u8;
        let length = unsafe { *(ptr.add(4) as *const u32) } as usize;

        let mut offset = 44;
        if offset + 16 <= length {
            // McfgEntry is #[repr(C, packed)] — references to its fields
            // would be unaligned. Read the whole struct by value.
            let entry: McfgEntry =
                unsafe { core::ptr::read_unaligned(ptr.add(offset) as *const McfgEntry) };
            let base = entry.base_address;
            let seg = entry.pci_segment;
            let sb = entry.start_bus;
            let eb = entry.end_bus;
            crate::serial_println!(
                "[acpi] MCFG: base={:#x}, segment={}, buses={}-{}",
                base,
                seg,
                sb,
                eb
            );
            return Some(base);
        }
        None
    }

    /// Return the `end_bus` of the first MCFG segment entry, i.e. the highest
    /// PCIe bus number the firmware declares as reachable via ECAM for segment
    /// 0. The PCI spec guarantees no function may live on a bus outside the
    /// MCFG-declared `[start_bus, end_bus]` range, so this is the authoritative
    /// upper bound for an ECAM enumeration scan (boot-time live-fix #1: scanning
    /// all 256 buses on Athena round-trips ~58 empty buses over ECAM MMIO).
    /// Returns `None` when no MCFG table is present.
    pub fn parse_mcfg_end_bus(&mut self) -> Option<u8> {
        let table = self.tables.find(&SIG_MCFG)?;
        let ptr = table.address as *const u8;
        let length = unsafe { *(ptr.add(4) as *const u32) } as usize;

        let offset = 44;
        if offset + 16 <= length {
            // McfgEntry is #[repr(C, packed)] — read by value to avoid an
            // unaligned reference to the packed `end_bus` field.
            let entry: McfgEntry =
                unsafe { core::ptr::read_unaligned(ptr.add(offset) as *const McfgEntry) };
            return Some(entry.end_bus);
        }
        None
    }

    pub fn set_device_power_state(&mut self, path: &str, state: u8) -> Result<(), AcpiError> {
        if state > 3 {
            return Err(AcpiError::MethodError);
        }
        let method = alloc::format!("{}.._PS{}", path, state);
        let _ = self.evaluate_method(&method, aml::value::Args::default())?;
        crate::serial_println!("[acpi] Device {} set to D{}", path, state);
        Ok(())
    }

    pub fn parse_bif(&mut self, path: &str) -> Option<BatteryInfo> {
        if let Ok(aml::AmlValue::Package(data)) =
            self.evaluate_method(path, aml::value::Args::default())
        {
            if data.len() >= 13 {
                let d_cap = match data[1] {
                    aml::AmlValue::Integer(v) => v as u32,
                    _ => 0,
                };
                let lf_cap = match data[2] {
                    aml::AmlValue::Integer(v) => v as u32,
                    _ => 0,
                };
                let tech = match &data[3] {
                    aml::AmlValue::String(s) => s.clone(),
                    _ => String::from("Unknown"),
                };
                let d_volt = match data[4] {
                    aml::AmlValue::Integer(v) => v as u32,
                    _ => 0,
                };
                let model = match &data[10] {
                    aml::AmlValue::String(s) => s.clone(),
                    _ => String::from("Unknown"),
                };
                let serial = match &data[11] {
                    aml::AmlValue::String(s) => s.clone(),
                    _ => String::from("Unknown"),
                };
                let name = match &data[12] {
                    aml::AmlValue::String(s) => s.clone(),
                    _ => String::from("Unknown"),
                };

                // _BIF returns 13 fields. We currently only parse the
                // human-readable ones; the numeric thresholds/granularity
                // values default to 0 until a full _BIX parser lands.
                let _ = (name, tech);
                return Some(BatteryInfo {
                    power_unit: 0,
                    design_capacity: d_cap,
                    last_full_capacity: lf_cap,
                    battery_technology: 0,
                    design_voltage: d_volt,
                    design_capacity_warning: 0,
                    design_capacity_low: 0,
                    cycle_count: 0,
                    measurement_accuracy: 0,
                    max_sampling_time: 0,
                    min_sampling_time: 0,
                    max_averaging_interval: 0,
                    min_averaging_interval: 0,
                    granularity_1: 0,
                    granularity_2: 0,
                    model_number: model,
                    serial_number: serial,
                    battery_type: String::new(),
                    oem_info: String::new(),
                });
            }
        }
        None
    }

    pub fn update_battery_status(&mut self) {
        if self.batteries.is_empty() {
            if let Some(info) = self.parse_bif("\\_SB.BAT0._BIF") {
                self.batteries.push(info);
            }
        }
        if self.batteries.is_empty() {
            return;
        }

        // Evaluate \_SB.BAT0._BST
        if let Ok(aml::AmlValue::Package(data)) =
            self.evaluate_method("\\_SB.BAT0._BST", aml::value::Args::default())
        {
            if data.len() >= 4 {
                if let (
                    aml::AmlValue::Integer(state),
                    aml::AmlValue::Integer(rate),
                    aml::AmlValue::Integer(cap),
                    aml::AmlValue::Integer(volt),
                ) = (&data[0], &data[1], &data[2], &data[3])
                {
                    let status = BatteryStatus {
                        state: *state as u32,
                        present_rate: *rate as u32,
                        remaining_capacity: *cap as u32,
                        present_voltage: *volt as u32,
                    };
                    self.battery_status.clear();
                    self.battery_status.push(status.clone());

                    let bif = &self.batteries[0];
                    let full = bif.last_full_capacity.max(bif.design_capacity);
                    let pct = status.percentage(full);

                    if let Some(mgr) = crate::power::POWER.lock().as_mut() {
                        mgr.battery.present = true;
                        mgr.battery.percent = pct;
                        mgr.battery.charging = status.is_charging();
                        mgr.battery.voltage_mv = status.present_voltage;
                        mgr.battery.current_ma = status.present_rate as i32;
                        mgr.battery.design_capacity_mah = bif.design_capacity;
                        mgr.battery.full_capacity_mah = full;
                    }

                    crate::serial_println!(
                        "[acpi] Battery: {}% {} mV (state={:#x})",
                        pct,
                        status.present_voltage,
                        status.state,
                    );
                }
            }
        }
    }

    pub fn dispatch_gpe(&mut self, gpe_num: u16) {
        let mut path = alloc::format!("\\_GPE._L{:02X}", gpe_num);
        if self
            .evaluate_method(&path, aml::value::Args::default())
            .is_err()
        {
            path = alloc::format!("\\_GPE._E{:02X}", gpe_num);
            let _ = self.evaluate_method(&path, aml::value::Args::default());
        }
    }

    pub fn update_thermal_status(&mut self) {
        // Evaluate \_SB.THM0._TMP
        if let Ok(temp_k) = self.evaluate_integer("\\_SB.THM0._TMP") {
            // Kelvin to milli-Celsius: (K * 100) - 273150
            let temp_mc = (temp_k as i32 * 100) - 273150;
            if let Some(thm) = self.thermal_zones.get_mut(0) {
                thm.temperature = temp_mc.max(0) as u32;
            }
        }
    }

    pub fn parse_prt(&mut self, path: &str) -> Vec<PciRoutingEntry> {
        let mut entries = Vec::new();

        // 1. Try internal namespace for static packages OR simple methods
        if let Some(node) = self.interpreter.namespace.get(path) {
            match &node.value {
                AmlValue::Package(pkg) => {
                    entries = self.parse_prt_package(pkg);
                }
                AmlValue::Method { body, .. } => {
                    // Simple heuristic: if it's a ReturnOp (0xA4) followed by a PackageOp (0x12)
                    if body.len() > 2 && body[0] == 0xA4 && body[1] == 0x12 {
                        crate::serial_println!("[acpi] parse_prt: node {} is a simple Return(Package) method; evaluating internally", path);
                        // We can't easily evaluate arbitrary AML, but we can try to parse the package data
                        // following the ReturnOp.
                        // For now, let's try the aml_context one more time with the exact name.
                    }
                }
                _ => {}
            }
            if !entries.is_empty() {
                return entries;
            }
        }

        // 2. Fall back to aml_context for dynamic methods
        match self.evaluate_method(path, aml::value::Args::default()) {
            Ok(aml::AmlValue::Package(pkgs)) => {
                for p in pkgs {
                    if let aml::AmlValue::Package(items) = p {
                        if items.len() >= 4 {
                            let addr = match items[0] {
                                aml::AmlValue::Integer(v) => v,
                                _ => 0,
                            };
                            let pin = match items[1] {
                                aml::AmlValue::Integer(v) => v as u8,
                                _ => 0,
                            };
                            let source = items[2].clone();
                            let src_idx = match items[3] {
                                aml::AmlValue::Integer(v) => v as u32,
                                _ => 0,
                            };
                            entries.push(PciRoutingEntry {
                                address: addr,
                                pin,
                                source,
                                source_index: src_idx,
                            });
                        }
                    }
                }
            }
            Ok(_) => {
                crate::serial_println!(
                    "[acpi][warn] AML evaluation of {} returned non-package",
                    path
                );
            }
            Err(e) => {
                if path.contains("PCI") {
                    crate::serial_println!(
                        "[acpi] parse_prt: evaluation of {} failed: {:?}",
                        path,
                        e
                    );
                }
            }
        }
        entries
    }

    fn parse_prt_package(&self, pkg: &Vec<AmlValue>) -> Vec<PciRoutingEntry> {
        let mut entries = Vec::new();
        for element in pkg {
            if let AmlValue::Package(entry_pkg) = element {
                if entry_pkg.len() >= 4 {
                    let address = match entry_pkg[0] {
                        AmlValue::Integer(v) => v,
                        _ => continue,
                    };
                    let pin = match entry_pkg[1] {
                        AmlValue::Integer(v) => v as u8,
                        _ => continue,
                    };

                    let source = match &entry_pkg[2] {
                        AmlValue::Integer(v) => aml::AmlValue::Integer(*v),
                        AmlValue::String(s) => aml::AmlValue::String(s.clone()),
                        _ => aml::AmlValue::Integer(0),
                    };

                    let source_index = match entry_pkg[3] {
                        AmlValue::Integer(v) => v as u32,
                        _ => 0,
                    };

                    entries.push(PciRoutingEntry {
                        address,
                        pin,
                        source,
                        source_index,
                    });
                }
            }
        }
        entries
    }

    pub fn scan_pci_routing(&mut self) {
        // Search the entire namespace for anything ending in _PRT.
        //
        // The custom interpreter no longer parses firmware AML (see
        // parse_table), so its namespace is empty on real boards — the live
        // namespace is `aml_context`'s. Traverse THAT for `_PRT` names
        // (they are Methods on AMD firmware; Athena has 17), and keep the
        // legacy list as a fallback for synthetic test namespaces.
        let mut prt_nodes: Vec<String> = self
            .interpreter
            .namespace
            .nodes
            .keys()
            .filter(|path| path.ends_with("_PRT"))
            .cloned()
            .collect();

        if let Some(ctx) = self.aml_context.as_mut() {
            let mut aml_prts: Vec<String> = Vec::new();
            let _ = ctx.namespace.traverse(|name, level| {
                for (seg, _) in level.values.iter() {
                    if seg.as_str() == "_PRT" {
                        aml_prts.push(alloc::format!("{}._PRT", name.as_string()));
                    }
                }
                Ok(true)
            });
            for p in aml_prts {
                if !prt_nodes.contains(&p) {
                    prt_nodes.push(p);
                }
            }
        }

        crate::serial_println!(
            "[acpi] scan_pci_routing: found {} _PRT candidate(s)",
            prt_nodes.len()
        );

        let mut count = 0;
        for prt_path in prt_nodes {
            let entries = self.parse_prt(&prt_path);
            if !entries.is_empty() {
                crate::serial_println!(
                    "[acpi] scan_pci_routing: found {} entries at {}",
                    entries.len(),
                    prt_path
                );

                // Determine the bus number by looking for _BBN in the parent scope
                let mut bus = 0;
                if let Some(dot_idx) = prt_path.rfind('.') {
                    let parent_path = &prt_path[..dot_idx];
                    let bbn_path = alloc::format!("{}.{}", parent_path, "_BBN");
                    if let Ok(bbn) = self.evaluate_integer(&bbn_path) {
                        bus = bbn as u8;
                    }
                }

                for entry in entries {
                    let device_id = (entry.address >> 16) as u8;
                    let pin = entry.pin + 1; // ACPI 0-3 -> PCI 1-4

                    match entry.source {
                        aml::AmlValue::Integer(0) => {
                            crate::pci_irq::add_entry(
                                bus,
                                device_id,
                                pin,
                                entry.source_index,
                                String::from("Direct"),
                            );
                            count += 1;
                        }
                        aml::AmlValue::String(ref s) => {
                            crate::serial_println!("[pci_irq][warn] Link Device routing not yet implemented: {} for {}.{}.{}", s, bus, device_id, pin);
                        }
                        _ => {}
                    }
                }
            }
        }
        crate::serial_println!("[pci] Routing table: {} entries parsed from _PRT", count);
    }

    pub fn discover_ec(&mut self) {
        let mut ec_node_path: Option<String> = None;

        // Scan the cached aml-crate device list for _HID PNP0C09 (string form
        // or EISA-encoded integer 0x090CD041). The hand-rolled namespace is
        // never populated from firmware AML, so it cannot be used here.
        const EC_HID_EISA: u64 = 0x090C_D041; // EISAID("PNP0C09")
        let devices = self.namespace_devices.clone();
        if let Some(ctx) = self.aml_context.as_ref() {
            for dev in devices {
                let hid_path = alloc::format!("{}._HID", dev);
                let Ok(name) = aml::AmlName::from_str(&hid_path) else {
                    continue;
                };
                match ctx.namespace.get_by_path(&name) {
                    Ok(aml::AmlValue::String(s)) if s.as_str() == "PNP0C09" => {
                        ec_node_path = Some(dev);
                        break;
                    }
                    Ok(aml::AmlValue::Integer(v)) if *v == EC_HID_EISA => {
                        ec_node_path = Some(dev);
                        break;
                    }
                    _ => {}
                }
            }
        }

        if let Some(path) = ec_node_path {
            let mut data_port = 0x62;
            let mut cmd_port = 0x66;
            let mut gpe_bit = 0;

            // Extract GPE bit from _GPE object
            let gpe_path = alloc::format!("{}.{}", path, "_GPE");
            if let Ok(gpe_val) = self.evaluate_integer(&gpe_path) {
                gpe_bit = gpe_val as u8;
            }

            // Extract command and data ports from _CRS
            let crs_path = alloc::format!("{}.{}", path, "_CRS");
            match self.evaluate_method(&crs_path, aml::value::Args::default()) {
                Ok(aml::AmlValue::Buffer(buf_mutex)) => {
                    let buf = buf_mutex.lock();
                    if let Some((p1, p2)) = self.parse_crs_ports(&buf) {
                        data_port = p1;
                        cmd_port = p2;
                    } else {
                        crate::serial_println!("[acpi][warn] EC _CRS parsing failed or no ports found, using fallbacks 0x66/0x62");
                    }
                }
                _ => {
                    crate::serial_println!("[acpi][warn] EC _CRS evaluation failed or not present, using fallbacks 0x66/0x62");
                }
            }

            crate::serial_println!(
                "[acpi] Embedded Controller found at ports {:#x}/{:#x}, GPE bit {}",
                cmd_port,
                data_port,
                gpe_bit
            );

            let ec = EmbeddedController::with_ports(data_port, cmd_port, gpe_bit);
            self.ec = Some(ec);

            // Call _REG(3, 1) to notify AML that OS is handling EC
            let reg_path = alloc::format!("{}.{}", path, "_REG");
            let reg_args = aml::value::Args::from_list(alloc::vec![
                aml::AmlValue::Integer(3), // AddressSpaceId = EmbeddedControl
                aml::AmlValue::Integer(1), // ConnectionStatus = Connect
            ])
            .unwrap();

            if let Ok(_) = self.evaluate_method(&reg_path, reg_args) {
                crate::serial_println!("[acpi] Evaluated _REG(3, 1) for EC device");
            }

            // Wiring GPE: enable the GPE bit in hardware.
            // Dispatch logic in dispatch_gpes will handle the EC query.
            unsafe {
                self.gpe.enable_gpe(gpe_bit);
            }
        }
    }

    fn parse_crs_ports(&self, buf: &[u8]) -> Option<(u16, u16)> {
        let mut i = 0;
        let mut ports = Vec::new();
        while i < buf.len() {
            let tag = buf[i];
            if tag == 0x79 {
                break;
            } // End Tag
            if tag & 0x80 == 0 {
                // Small item
                let size = (tag & 0x07) as usize;
                let type_ = (tag >> 3) & 0x0F;
                if type_ == 0x08 {
                    // IO Port Descriptor
                    if i + 7 < buf.len() {
                        let min = u16::from_le_bytes([buf[i + 2], buf[i + 3]]);
                        ports.push(min);
                    }
                }
                i += 1 + size;
            } else {
                // Large item
                if i + 3 > buf.len() {
                    break;
                }
                let size = u16::from_le_bytes([buf[i + 1], buf[i + 2]]) as usize;
                i += 3 + size;
            }
        }
        if ports.len() >= 2 {
            Some((ports[0], ports[1]))
        } else {
            None
        }
    }

    /// Walk the authoritative `aml` crate namespace and cache every
    /// Device()/Processor() node path. Called once from `init()`; the result
    /// feeds `namespace_device_count`, the smoketest, and procfs.
    /// MasterChecklist Phase 1.4: namespace device enumeration (GPE/_PRT/EC
    /// all depend on a populated namespace on Athena).
    fn collect_namespace_devices(&mut self) {
        let mut devs: Vec<String> = Vec::new();
        if let Some(ctx) = self.aml_context.as_mut() {
            let _ = ctx.namespace.traverse(|name, level| {
                if matches!(
                    level.typ,
                    aml::LevelType::Device | aml::LevelType::Processor
                ) {
                    devs.push(name.as_string());
                }
                Ok(true)
            });
        }
        crate::serial_println!("[acpi] enumerate_devices: found {} devices", devs.len());
        // The full per-device list is a high-volume diagnostic (159 devices on
        // Athena). Route it through the COM1-only path so the boot critical path
        // does NOT render 159 glyph lines to the slow GOP framebuffer (a real
        // multi-hundred-ms cost on bare metal — boot-time live-fix #1) and does
        // not evict the boot transcript from the 1 MiB bootlog ring. The count
        // above stays in the durable log; the full list is on COM1.
        for d in &devs {
            crate::serial_only_println!("[acpi]   device: {}", d);
        }
        self.namespace_devices = devs;
    }

    pub fn namespace_device_count(&self) -> usize {
        self.namespace_devices.len()
    }

    pub fn normalize_path(path: &str) -> String {
        if path == "\\" {
            return String::from("\\");
        }
        let mut out = String::new();
        let stripped = if path.starts_with('\\') {
            out.push('\\');
            &path[1..]
        } else {
            path
        };

        for (i, part) in stripped.split('.').enumerate() {
            if i > 0 {
                out.push('.');
            }
            let mut s = String::from(part);
            while s.len() < 4 {
                s.push('_');
            }
            out.push_str(&s);
        }
        out
    }

    pub fn evaluate_method(
        &mut self,
        path: &str,
        args: aml::value::Args,
    ) -> Result<aml::AmlValue, AcpiError> {
        let ctx = self.aml_context.as_mut().ok_or(AcpiError::NotInitialized)?;

        // Try the normalized path (padded segments)
        let norm = Self::normalize_path(path);
        let name = aml::AmlName::from_str(&norm).map_err(|_| AcpiError::InvalidTable)?;

        match ctx.invoke_method(&name, args.clone()) {
            Ok(v) => Ok(v),
            Err(e) => {
                // If normalized failed, try original as fallback
                if let Ok(orig_name) = aml::AmlName::from_str(path) {
                    if let Ok(v) = ctx.invoke_method(&orig_name, args) {
                        return Ok(v);
                    }
                }
                if let aml::AmlError::ValueDoesNotExist(_) = e {
                    // Suppress expected missing methods
                } else if let aml::AmlError::LevelDoesNotExist(_) = e {
                    // Suppress expected missing levels
                } else {
                    crate::serial_println!("[acpi][warn] evaluate_method {} failed: {:?}", path, e);
                }
                Err(AcpiError::MethodError)
            }
        }
    }

    pub fn evaluate_integer(&mut self, path: &str) -> Result<u64, AcpiError> {
        match self.evaluate_method(path, aml::value::Args::default()) {
            Ok(aml::AmlValue::Integer(v)) => Ok(v),
            Ok(_) => Err(AcpiError::MethodError),
            Err(e) => Err(e),
        }
    }

    /// Scan `\_GPE` namespace for `_Lxx` (level) and `_Exx` (edge) methods,
    /// register them in the GPE subsystem, and enable the corresponding GPEs.
    /// MasterChecklist Phase 1.4: GPE dispatcher — parse `_Lxx`/`_Exx` from `\_GPE`.
    pub fn init_gpe_handlers(&mut self) {
        // First, set up the GPE hardware blocks from FADT.
        if let Some(fadt) = &self.fadt {
            // Clone the fadt to avoid borrow conflict with self.gpe.
            let fadt_clone = fadt.clone();
            self.gpe.setup_from_fadt(&fadt_clone);

            // Wire the System Control Interrupt (SCI) via IOAPIC.
            // Standard vector for SCI is 0xF0 (240).
            let sci_irq = fadt.sci_interrupt as u32;
            let sci_vec = crate::interrupts::InterruptIndex::Sci.as_u8();
            crate::apic::route_sci(sci_irq, sci_vec);
        }

        // Scan interpreter namespace for \_GPE._Lxx and \_GPE._Exx methods.
        // Collect first to avoid holding a borrow on self.interpreter while mutating self.gpe.
        let gpe_methods: Vec<(String, bool)> = self
            .interpreter
            .namespace
            .nodes
            .iter()
            .filter_map(|(path, _node)| {
                // Path format: \_GPE._Lxx or \_GPE._Exx (one backslash in Rust string)
                let level_prefix = "\\_GPE._L";
                let edge_prefix = "\\_GPE._E";
                if let Some(hex) = path.strip_prefix(level_prefix) {
                    if hex.len() == 2 && hex.chars().all(|c| c.is_ascii_hexdigit()) {
                        return Some((path.clone(), true));
                    }
                }
                if let Some(hex) = path.strip_prefix(edge_prefix) {
                    if hex.len() == 2 && hex.chars().all(|c| c.is_ascii_hexdigit()) {
                        return Some((path.clone(), false));
                    }
                }
                None
            })
            .collect();

        let mut found = 0u32;
        for (path, is_level) in &gpe_methods {
            let hex_digits = &path[path.len() - 2..];
            if let Ok(gpe_num) = u8::from_str_radix(hex_digits, 16) {
                let trigger = if *is_level {
                    GpeTrigger::Level
                } else {
                    GpeTrigger::Edge
                };
                self.gpe.register_handler(gpe_num, trigger, path, false);
                // SAFETY: Writing to ACPI GPE enable registers at parsed I/O addresses.
                unsafe {
                    self.gpe.enable_gpe(gpe_num);
                }
                found += 1;
            }
        }

        crate::serial_println!(
            "[acpi] GPE dispatcher: {} handler(s) in \\_GPE ({} block(s) at GPE0={:#x})",
            found,
            self.gpe.blocks.len(),
            self.gpe.blocks.first().map(|b| b.gas.address).unwrap_or(0),
        );
    }

    /// Check GPE hardware status registers and dispatch any pending events
    /// by calling the corresponding AML `_Lxx`/`_Exx` method.
    /// Returns the number of events dispatched.
    pub fn dispatch_gpes(&mut self) -> u32 {
        // Collect active GPEs from hardware (avoid borrow conflict with evaluate_method).
        let mut active: Vec<(u8, String)> = Vec::new();
        // SAFETY: Reading ACPI GPE status/enable registers at parsed I/O addresses.
        unsafe {
            for block in &self.gpe.blocks {
                for reg in 0..block.register_count as u64 {
                    let status_off = reg;
                    let enable_off = block.register_count as u64 + reg;
                    let status = block.gas.read_u8(status_off);
                    let enable = block.gas.read_u8(enable_off);
                    let pending = status & enable;
                    for bit in 0..8u8 {
                        if pending & (1 << bit) != 0 {
                            let gpe = block.base_gpe + reg as u8 * 8 + bit;
                            // Clear status (write-1-to-clear).
                            block.gas.write_u8(status_off, 1u8 << bit);
                            if let Some(h) = self.gpe.handlers.get(&gpe) {
                                active.push((gpe, h.method_name.clone()));
                            }
                        }
                    }
                }
            }
        }

        let dispatched = active.len() as u32;
        for (gpe, method_name) in active {
            crate::serial_println!("[acpi] GPE {:#04x}: dispatching {}", gpe, method_name);

            // Phase 1.9: Embedded Controller (EC) Support
            // If this is the EC GPE, we must query the EC to acknowledge the event.
            if let Some(ec) = &self.ec {
                if gpe == ec.gpe_bit {
                    unsafe {
                        match ec.query() {
                            Ok(query_val) => {
                                crate::serial_println!(
                                    "[acpi] EC query event acknowledged: {:#02x}",
                                    query_val
                                );
                                // The query value corresponds to a _Qxx method (e.g. _Q12).
                                // Future enhancement: evaluate _Qxx methods here.
                            }
                            Err(e) => {
                                crate::serial_println!("[acpi][error] EC query failed: {:?}", e);
                            }
                        }
                    }
                }
            }

            // Power Button Handler: check for common GPE methods linked to the power button.
            // _L01, _L0C, _L0E are common on various chipsets/QEMU.
            let is_pwr_button = method_name.ends_with("_L01")
                || method_name.ends_with("_E01")
                || method_name.ends_with("_L0C")
                || method_name.ends_with("_E0C")
                || method_name.ends_with("_L0E")
                || method_name.ends_with("_E0E");

            if is_pwr_button {
                crate::serial_println!("[acpi] Power button pressed");
            }

            let _ = self.evaluate_method(&method_name, aml::value::Args::default());

            if is_pwr_button {
                // Fulfill Phase 1.7 requirement: "eventually trigger acpi_full::power_off()".
                // We call our internal power_off() which uses the PowerManager for S5.
                // Note: This is an immediate shutdown for demonstration purposes.
                unsafe {
                    if let Some(fadt) = &self.fadt {
                        let _ = self.power_manager.shutdown(fadt);
                    }
                }
            }

            if let Some(h) = self.gpe.handlers.get_mut(&gpe) {
                h.count += 1;
            }
        }
        dispatched
    }
}

pub static ACPI_SUBSYSTEM: Mutex<AcpiSubsystem> = Mutex::new(AcpiSubsystem::new());

/// Fail-soft AML method evaluation wrapper for use by other modules.
pub fn safe_evaluate_method(
    path: &str,
    args: aml::value::Args,
) -> Result<aml::AmlValue, AcpiError> {
    let mut sub = ACPI_SUBSYSTEM.lock();
    sub.evaluate_method(path, args)
}

/// Initialize the GPE dispatcher: scan `\_GPE` for `_Lxx`/`_Exx` methods and
/// enable those GPEs in hardware. Call after `acpi_full::init()` completes.
/// MasterChecklist Phase 1.4: GPE dispatcher.
pub fn init_gpe_dispatcher() {
    let mut sub = ACPI_SUBSYSTEM.lock();
    if sub.initialized {
        sub.init_gpe_handlers();
    } else {
        crate::serial_println!("[acpi] GPE dispatcher: ACPI not initialized, skipping");
    }
}

/// Poll GPE events and dispatch them. Call from LAPIC timer tick or SCI handler.
/// Safe to call frequently — returns quickly if no GPEs are pending.
pub fn poll_gpe_events() -> u32 {
    if let Some(mut sub) = ACPI_SUBSYSTEM.try_lock() {
        if sub.initialized && !sub.gpe.blocks.is_empty() {
            return sub.dispatch_gpes();
        }
    }
    0
}

pub fn init(rsdp_addr: u64) {
    let mut sub = ACPI_SUBSYSTEM.lock();
    sub.init(rsdp_addr);
    if sub.initialized {
        crate::serial_println!(
            "[ OK ] ACPI full subsystem initialized ({} tables, {} namespace devices)",
            sub.table_count(),
            sub.namespace_device_count(),
        );
        if let Some(rsdp) = &sub.rsdp {
            let oem_id = core::str::from_utf8(&rsdp.oem_id)
                .unwrap_or("??????")
                .trim_end_matches('\0');
            let oem_table = sub
                .tables
                .find(&SIG_DSDT)
                .map(|t| {
                    let hdr = t.address as *const u8;
                    unsafe {
                        core::str::from_utf8(core::slice::from_raw_parts(hdr.add(10), 4))
                            .unwrap_or("????")
                    }
                })
                .unwrap_or("????");
            crate::acpi_quirks::audit_firmware_oem(oem_id, oem_table);
        }
    }
}

pub fn run_boot_smoketest() {
    let sub = ACPI_SUBSYSTEM.lock();
    if sub.initialized {
        // An initialized subsystem with zero namespace devices means the DSDT
        // was not actually interpreted — that is a FAIL, not a PASS (Phase 1.4
        // GPE/_PRT/EC all need a populated namespace).
        let devices = sub.namespace_device_count();
        let verdict = if devices > 0 {
            "PASS"
        } else {
            "FAIL (AML namespace empty)"
        };
        crate::serial_println!(
            "[acpi] run_boot_smoketest: initialized=true tables={} devices={} -> {}",
            sub.table_count(),
            devices,
            verdict
        );
    } else {
        crate::serial_println!("[acpi] run_boot_smoketest: NOT INITIALIZED -> FAIL");
    }
}

pub fn power_off() -> ! {
    let sub = ACPI_SUBSYSTEM.lock();
    if let (Some(fadt), true) = (&sub.fadt, sub.initialized) {
        crate::serial_println!("[acpi] Initiating S5 shutdown...");
        unsafe {
            let _ = sub.power_manager.shutdown(fadt);
        }
    } else {
        crate::serial_println!("[acpi] Cannot shutdown: ACPI not initialized or FADT missing");
    }
    loop {
        unsafe {
            core::arch::asm!("hlt");
        }
    }
}

pub fn dump_text() -> String {
    let sub = ACPI_SUBSYSTEM.lock();
    let mut buf = String::new();
    buf.push_str("ACPI Subsystem Status\n");
    buf.push_str("=====================\n");
    buf.push_str(&alloc::format!("Initialized: {}\n", sub.initialized));
    if let Some(rsdp) = &sub.rsdp {
        buf.push_str(&alloc::format!(
            "RSDP OEM ID: {}\n",
            core::str::from_utf8(&rsdp.oem_id).unwrap_or("??????")
        ));
    }
    buf.push_str(&alloc::format!("Tables Found: {}\n", sub.table_count()));
    for t in &sub.tables.tables {
        let sig = core::str::from_utf8(&t.header.signature).unwrap_or("????");
        buf.push_str(&alloc::format!(
            "  {} @ {:#x} (rev {}, len {})\n",
            sig,
            t.address,
            t.header.revision,
            t.header.length
        ));
    }
    if let Some(fadt) = &sub.fadt {
        let pm1a = fadt.pm1a_control_gas();
        let gpe0 = fadt.gpe0_block_gas();
        buf.push_str(&alloc::format!(
            "PM Registers: PM1a_CNT={:#x} GPE0={:#x} (source: {})\n",
            pm1a.address,
            gpe0.address,
            if fadt.x_pm1a_control_block.is_valid() {
                "X-GAS"
            } else {
                "Legacy"
            }
        ));
    }
    if let Some(ec) = &sub.ec {
        buf.push_str(&alloc::format!(
            "Embedded Controller: ports={:#x}/{:#x}, GPE bit={}\n",
            ec.command_port,
            ec.data_port,
            ec.gpe_bit
        ));
    }
    buf.push_str(&alloc::format!(
        "Namespace Devices: {}\n",
        sub.namespace_device_count()
    ));
    buf
}

/// ACPI platform interrupt model + OS interface negotiation.
/// MasterChecklist Phase 1.4: `_PIC(1)` before IOAPIC routing; `_OSI` for DSDT branches.
unsafe fn apply_platform_bringup(ctx: &mut aml::AmlContext) {
    use alloc::string::String;
    use alloc::vec;
    use alloc::vec::Vec;
    use aml::{AmlName, AmlValue};

    crate::serial_println!("[acpi] Platform bring-up audit: starting method invocation sequence");

    let mut methods_invoked: u32 = 0;
    let mut failures: u32 = 0;

    // Helper for fail-soft invocation
    let mut invoke_safe = |path: &str, args: aml::value::Args| {
        let Ok(name) = AmlName::from_str(path) else {
            return;
        };
        match ctx.invoke_method(&name, args) {
            Ok(v) => {
                methods_invoked += 1;
                crate::serial_println!("[acpi] AML method {} -> {:?}", path, v);
            }
            Err(e) => {
                failures += 1;
                crate::serial_println!(
                    "[acpi][warn] AML method {} failed or not present: {:?}",
                    path,
                    e
                );
            }
        }
    };

    // 1. OSI support (Requirement 2: report "Windows 2020" as supported, "Linux" as unsupported)
    // We test our own OSI response here.
    // Note: Real hardware DSDTs will call _OSI internally.
    for os in ["Windows 2020", "Linux", "Windows 2015"] {
        let args = aml::value::Args::from_list(vec![AmlValue::String(String::from(os))])
            .unwrap_or(aml::value::Args::default());
        invoke_safe("\\_OSI", args);
    }

    // 2. _PIC(1) - Switch to APIC mode
    let pic_args = aml::value::Args::from_list(vec![AmlValue::Integer(1)])
        .unwrap_or(aml::value::Args::default());
    invoke_safe("\\_PIC", pic_args.clone());
    invoke_safe("\\_SB._PIC", pic_args);

    // 3. _REG audit (Requirement 3: log _REG status)
    // _REG(AddressSpaceId, ConnectionStatus)
    // 0 = SystemMemory, 1 = SystemIO, 2 = PciConfig, 3 = EmbeddedControl
    for space_id in [0, 1, 2, 3] {
        let reg_args = aml::value::Args::from_list(vec![
            AmlValue::Integer(space_id),
            AmlValue::Integer(1), // Connected
        ])
        .unwrap_or(aml::value::Args::default());

        // We try common paths for _REG
        invoke_safe("\\_SB._REG", reg_args.clone());
        invoke_safe("\\_SB.PCI0._REG", reg_args);
    }

    crate::serial_println!(
        "[acpi] Platform bring-up audit: {} method(s) invoked, {} failed/absent",
        methods_invoked,
        failures
    );
}

pub unsafe fn negotiate_pcie_control(ctx: &mut aml::AmlContext) -> Result<(), &'static str> {
    use alloc::sync::Arc;
    use alloc::vec;
    use aml::{AmlName, AmlValue};
    use spinning_top::Spinlock;

    // 1. Construct the canonical path to the primary PCI Root Bridge
    let root_bridge_path =
        AmlName::from_str("\\_SB.PCI0._OSC").map_err(|_| "Invalid AML name path")?;

    // 2. Define the PCIe Subsystem Takeover UUID
    // 33db4d5b-617f-44b6-b340-4c714814d830
    let pcie_uuid = AmlValue::Buffer(Arc::new(Spinlock::new(vec![
        0x5B, 0x4D, 0xDB, 0x33, // DWORD 0 (Little Endian)
        0x7F, 0x61, // WORD 1
        0xB6, 0x44, // WORD 2
        0xB3, 0x40, 0x4C, 0x71, 0x48, 0x14, 0xD8, 0x30, // Remaining 8 bytes
    ])));

    let revision = AmlValue::Integer(1); // PCIe OSC Revision 1
    let count = AmlValue::Integer(3); // Passing 3 DWORDS of capabilities

    // 3. Construct our capabilities matrix
    // DWORD 1: 0 (Query/Status)
    // DWORD 2: Support Flags (0x33 = Extended Config Space + ASPM + MSI + Segment Groups)
    // DWORD 3: Control Flags (0x1D = Native Hot-Plug + Native PME + Native AER + Native PCIe Cap Structure)
    let capabilities = AmlValue::Buffer(Arc::new(Spinlock::new(vec![
        0x00, 0x00, 0x00, 0x00, // Status
        0x33, 0x00, 0x00, 0x00, // Support
        0x1D, 0x00, 0x00, 0x00, // Control
    ])));

    // 4. Invoke the method with arguments: UUID, Revision, Count, Capabilities Buffer
    let args = vec![pcie_uuid, revision, count, capabilities];
    let aml_args = aml::value::Args::from_list(args).unwrap_or(aml::value::Args::default());

    match ctx.invoke_method(&root_bridge_path, aml_args) {
        Ok(AmlValue::Buffer(return_buffer_mutex)) => {
            let return_buffer = return_buffer_mutex.lock();
            if return_buffer.len() < 12 {
                return Err("ACPI Firmware returned undersized buffer.");
            }
            // Check if the firmware rejected our control requests (DWORD 0 contains error bits)
            let status = return_buffer[0] as u32 | ((return_buffer[1] as u32) << 8);
            if (status & 0x02) != 0 {
                return Err("ACPI Firmware refused to surrender native PCIe control (Failure to clear _OSC).");
            }
            // Check if our requested bits in DWORD 3 were masked or modified
            let granted_control = return_buffer[8];
            if (granted_control & 0x1D) != 0x1D {
                return Err("ACPI Firmware masked critical native PCIe capability paths.");
            }

            crate::serial_println!(
                "RaeKernel: Native PCIe control successfully transferred from SMM to OS."
            );
            Ok(())
        }
        _ => Err("Failed to evaluate \\_SB.PCI0._OSC or received invalid data payload type."),
    }
}
