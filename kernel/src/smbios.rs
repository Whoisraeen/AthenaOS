//! SMBIOS / DMI parser — Foundation §1.
//!
//! Identifies OEM hardware, BIOS versions, and system quirks on real iron.
//! Essential for the M-A "Boots on Athena" milestone.

use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

#[repr(C, packed)]
struct EntryPoint32 {
    anchor: [u8; 4], // "_SM_"
    checksum: u8,
    length: u8,
    major: u8,
    minor: u8,
    max_structure_size: u16,
    revision: u8,
    formatted_area: [u8; 5],
    intermediate_anchor: [u8; 5], // "_DMI_"
    intermediate_checksum: u8,
    table_length: u16,
    table_address: u32,
    number_structures: u16,
    bcd_revision: u8,
}

#[repr(C, packed)]
struct EntryPoint64 {
    anchor: [u8; 5], // "_SM3_"
    checksum: u8,
    length: u8,
    major: u8,
    minor: u8,
    doc_rev: u8,
    revision: u8,
    reserved: u8,
    table_max_size: u32,
    table_address: u64,
}

#[derive(Debug, Clone)]
pub struct SmbiosHeader {
    pub kind: u8,
    pub length: u8,
    pub handle: u16,
}

pub struct SmbiosTable {
    pub header: SmbiosHeader,
    pub data: Vec<u8>,
    pub strings: Vec<String>,
}

impl SmbiosTable {
    pub fn get_string(&self, index: u8) -> Option<&String> {
        if index == 0 {
            return None;
        }
        self.strings.get((index - 1) as usize)
    }
}

pub struct SmbiosSubsystem {
    pub version_major: u8,
    pub version_minor: u8,
    pub manufacturer: String,
    pub product_name: String,
    pub bios_version: String,
    pub tables: Vec<SmbiosTable>,
}

pub static SMBIOS: Mutex<Option<SmbiosSubsystem>> = Mutex::new(None);

pub fn init(rsdp_addr: u64) {
    let _ = rsdp_addr;
    let mut lock = SMBIOS.lock();
    if lock.is_some() {
        return;
    }

    // Scan for 32-bit entry point "_SM_" or 64-bit entry point "_SM3_"
    // In legacy BIOS systems, this is at 0xF0000-0xFFFFF.
    let start = 0x000F0000u64;
    let end = 0x000FFFFFu64;

    for addr in (start..end).step_by(16) {
        let virt = crate::memory::phys_to_virt(addr);
        let ptr = virt.as_ptr::<u8>();

        unsafe {
            if core::slice::from_raw_parts(ptr, 4) == b"_SM_" {
                let ep = &*(ptr as *const EntryPoint32);
                if ep.length >= 0x1F && calculate_checksum(ptr, ep.length as usize) == 0 {
                    crate::serial_println!("[smbios] found v2.x entry point at {:#x}", addr);
                    *lock = parse_tables(
                        ep.table_address as u64,
                        ep.number_structures,
                        ep.table_length,
                    );
                    return;
                }
            } else if core::slice::from_raw_parts(ptr, 5) == b"_SM3_" {
                let ep = &*(ptr as *const EntryPoint64);
                if ep.length >= 0x18 && calculate_checksum(ptr, ep.length as usize) == 0 {
                    crate::serial_println!("[smbios] found v3.x entry point at {:#x}", addr);
                    *lock = parse_tables(ep.table_address, 100, 0); // structures count unknown for v3
                    return;
                }
            }
        }
    }
}

fn calculate_checksum(ptr: *const u8, len: usize) -> u8 {
    let mut sum: u8 = 0;
    for i in 0..len {
        sum = sum.wrapping_add(unsafe { *ptr.add(i) });
    }
    sum
}

pub unsafe fn parse_tables(addr: u64, count: u16, len: u16) -> Option<SmbiosSubsystem> {
    // The SMBIOS table_address from the entry point is a *physical* address
    // (BIOS data area). Convert through the kernel's phys_to_virt mapping
    // before dereferencing — bare raw access page-faults under KASLR.
    let mut ptr = crate::memory::phys_to_virt(addr).as_ptr::<u8>();
    let mut tables = Vec::new();

    for _ in 0..count {
        let kind = *ptr;
        let length = *ptr.add(1);
        let handle = u16::from_le_bytes([*ptr.add(2), *ptr.add(3)]);

        let mut data = Vec::with_capacity(length as usize);
        for i in 0..length {
            data.push(*ptr.add(i as usize));
        }

        let mut strings = Vec::new();
        let mut str_ptr = ptr.add(length as usize);

        loop {
            if *str_ptr == 0 && *str_ptr.add(1) == 0 {
                str_ptr = str_ptr.add(2);
                break;
            }

            let mut s = String::new();
            while *str_ptr != 0 {
                s.push(*str_ptr as char);
                str_ptr = str_ptr.add(1);
            }
            strings.push(s);
            str_ptr = str_ptr.add(1);
        }

        tables.push(SmbiosTable {
            header: SmbiosHeader {
                kind,
                length,
                handle,
            },
            data,
            strings,
        });

        ptr = str_ptr;
    }

    let mut manufacturer = String::from("Unknown");
    let mut product_name = String::from("Unknown");
    let mut bios_version = String::from("Unknown");

    for t in &tables {
        match t.header.kind {
            0 => {
                // BIOS Info — data includes the 4-byte header
                if t.data.len() > 5 {
                    if let Some(s) = t.get_string(t.data[5]) {
                        bios_version = s.clone();
                    }
                }
            }
            1 => {
                // System Info
                if t.data.len() > 5 {
                    if let Some(s) = t.get_string(t.data[4]) {
                        manufacturer = s.clone();
                    }
                    if let Some(s) = t.get_string(t.data[5]) {
                        product_name = s.clone();
                    }
                }
            }
            _ => {}
        }
    }

    Some(SmbiosSubsystem {
        version_major: 2,
        version_minor: 0, // placeholders
        manufacturer,
        product_name,
        bios_version,
        tables,
    })
}

pub fn dump_info() -> String {
    let lock = SMBIOS.lock();
    if let Some(ref s) = *lock {
        alloc::format!(
            "SMBIOS v{}.{}\nManufacturer: {}\nProduct: {}\nBIOS: {}\nTables: {}\n",
            s.version_major,
            s.version_minor,
            s.manufacturer,
            s.product_name,
            s.bios_version,
            s.tables.len()
        )
    } else {
        String::from("SMBIOS not initialized\n")
    }
}
