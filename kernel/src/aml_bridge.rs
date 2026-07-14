use aml::{AmlError, Handler};
use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{AtomicU32, Ordering};

pub struct RaeAmlHandler;

#[derive(Clone, Copy, Debug)]
pub struct PhysicalAddress(pub usize); // Wait, aml crate passes `usize`

/// Count of AML memory accesses refused because they targeted OS-usable RAM.
/// Real firmware AML only writes to NVS / reserved / MMIO; an access into RAM
/// the kernel owns is always a misparse or a hostile/buggy table and would
/// corrupt the heap or the buddy allocator's free list (observed as a later
/// non-canonical-pointer / BTreeMap-navigate panic). We refuse it instead.
static REFUSED_AML_ACCESSES: AtomicU32 = AtomicU32::new(0);

/// Returns `true` (access allowed) if `address` is NOT OS-usable RAM. Logs the
/// first few refusals so a problematic DSDT is visible without flooding serial.
fn aml_access_ok(address: usize, size: usize, write: bool) -> bool {
    if crate::memory::phys_is_usable_ram(address as u64) {
        let n = REFUSED_AML_ACCESSES.fetch_add(1, Ordering::Relaxed);
        if n < 16 {
            crate::serial_println!(
                "[aml][guard] refused {} of {}B at phys {:#x} (targets OS RAM, not MMIO/NVS)",
                if write { "write" } else { "read" },
                size,
                address
            );
        }
        return false;
    }
    true
}

/// Total AML accesses refused for targeting OS RAM (for /proc + smoketests).
pub fn refused_aml_access_count() -> u32 {
    REFUSED_AML_ACCESSES.load(Ordering::Relaxed)
}

/// Map an AML operation-region/ECAM MMIO access through the arch-neutral paging
/// seam, returning the physmap-consistent virtual address (sub-page offset
/// preserved) the caller dereferences. Delegates to
/// `arch::mmu::AddressSpace::map_mmio_range` with [`crate::arch::mmu::PageFlags::DEVICE`]
/// — byte-identical to the previous direct `memory::map_mmio_region` call (the
/// seam's x86 backend forwards to it), but no longer names the raw MMIO-map path
/// (Slice 1.5b — docs/research/slice1_5-arch-paging-trait.md §4).
#[inline]
fn map_aml_mmio(address: usize, size: usize) -> x86_64::VirtAddr {
    crate::arch::mmu::kernel().map_mmio_range(
        x86_64::PhysAddr::new(address as u64),
        size,
        crate::arch::mmu::PageFlags::DEVICE,
    )
}

impl Handler for RaeAmlHandler {
    /// AML `Sleep(ms)` — firmware uses it for EC/device settle times (Athena:
    /// 43 sites). HPET wall-clock, immune to TSC calibration. Capped so a
    /// pathological table can't stall boot for minutes.
    fn sleep(&self, milliseconds: u64) {
        let capped_us = milliseconds.min(2_000) * 1_000;
        let _ = crate::hpet::spin_until_us(capped_us, || false);
    }

    /// AML `Stall(µs)` — spec-bounded to ≤100 µs per call; cap accordingly.
    fn stall(&self, microseconds: u64) {
        let _ = crate::hpet::spin_until_us(microseconds.min(100), || false);
    }

    fn read_u8(&self, address: usize) -> u8 {
        if !aml_access_ok(address, 1, false) {
            return 0;
        }
        let virt = map_aml_mmio(address, 1).as_ptr::<u8>();
        unsafe { read_volatile(virt) }
    }

    fn read_u16(&self, address: usize) -> u16 {
        if !aml_access_ok(address, 2, false) {
            return 0;
        }
        let virt = map_aml_mmio(address, 2).as_ptr::<u16>();
        unsafe { read_volatile(virt) }
    }

    fn read_u32(&self, address: usize) -> u32 {
        if !aml_access_ok(address, 4, false) {
            return 0;
        }
        let virt = map_aml_mmio(address, 4).as_ptr::<u32>();
        unsafe { read_volatile(virt) }
    }

    fn read_u64(&self, address: usize) -> u64 {
        if !aml_access_ok(address, 8, false) {
            return 0;
        }
        let virt = map_aml_mmio(address, 8).as_ptr::<u64>();
        unsafe { read_volatile(virt) }
    }

    fn write_u8(&mut self, address: usize, value: u8) {
        if !aml_access_ok(address, 1, true) {
            return;
        }
        let virt = map_aml_mmio(address, 1).as_mut_ptr::<u8>();
        unsafe { write_volatile(virt, value) }
    }

    fn write_u16(&mut self, address: usize, value: u16) {
        if !aml_access_ok(address, 2, true) {
            return;
        }
        let virt = map_aml_mmio(address, 2).as_mut_ptr::<u16>();
        unsafe { write_volatile(virt, value) }
    }

    fn write_u32(&mut self, address: usize, value: u32) {
        if !aml_access_ok(address, 4, true) {
            return;
        }
        let virt = map_aml_mmio(address, 4).as_mut_ptr::<u32>();
        unsafe { write_volatile(virt, value) }
    }

    fn write_u64(&mut self, address: usize, value: u64) {
        if !aml_access_ok(address, 8, true) {
            return;
        }
        let virt = map_aml_mmio(address, 8).as_mut_ptr::<u64>();
        unsafe { write_volatile(virt, value) }
    }

    fn read_io_u8(&self, port: u16) -> u8 {
        unsafe { x86_64::instructions::port::PortReadOnly::<u8>::new(port).read() }
    }

    fn read_io_u16(&self, port: u16) -> u16 {
        unsafe { x86_64::instructions::port::PortReadOnly::<u16>::new(port).read() }
    }

    fn read_io_u32(&self, port: u16) -> u32 {
        unsafe { x86_64::instructions::port::PortReadOnly::<u32>::new(port).read() }
    }

    fn write_io_u8(&self, port: u16, value: u8) {
        unsafe { x86_64::instructions::port::PortWriteOnly::<u8>::new(port).write(value) }
    }

    fn write_io_u16(&self, port: u16, value: u16) {
        unsafe { x86_64::instructions::port::PortWriteOnly::<u16>::new(port).write(value) }
    }

    fn write_io_u32(&self, port: u16, value: u32) {
        unsafe { x86_64::instructions::port::PortWriteOnly::<u32>::new(port).write(value) }
    }

    fn read_pci_u8(&self, segment: u16, bus: u8, device: u8, function: u8, offset: u16) -> u8 {
        if let Some(ecam_base) = crate::pcie::get_ecam_base(segment, bus) {
            let pci_addr = ecam_base
                | ((bus as u64) << 20)
                | ((device as u64) << 15)
                | ((function as u64) << 12)
                | (offset as u64);
            unsafe { read_volatile(map_aml_mmio(pci_addr as usize, 4).as_ptr::<u8>()) }
        } else {
            0xFF
        }
    }

    fn read_pci_u16(&self, segment: u16, bus: u8, device: u8, function: u8, offset: u16) -> u16 {
        if let Some(ecam_base) = crate::pcie::get_ecam_base(segment, bus) {
            let pci_addr = ecam_base
                | ((bus as u64) << 20)
                | ((device as u64) << 15)
                | ((function as u64) << 12)
                | (offset as u64);
            unsafe { read_volatile(map_aml_mmio(pci_addr as usize, 4).as_ptr::<u16>()) }
        } else {
            0xFFFF
        }
    }

    fn read_pci_u32(&self, segment: u16, bus: u8, device: u8, function: u8, offset: u16) -> u32 {
        if let Some(ecam_base) = crate::pcie::get_ecam_base(segment, bus) {
            let pci_addr = ecam_base
                | ((bus as u64) << 20)
                | ((device as u64) << 15)
                | ((function as u64) << 12)
                | (offset as u64);
            unsafe { read_volatile(map_aml_mmio(pci_addr as usize, 4).as_ptr::<u32>()) }
        } else {
            0xFFFFFFFF
        }
    }

    fn write_pci_u8(
        &self,
        segment: u16,
        bus: u8,
        device: u8,
        function: u8,
        offset: u16,
        value: u8,
    ) {
        if let Some(ecam_base) = crate::pcie::get_ecam_base(segment, bus) {
            let pci_addr = ecam_base
                | ((bus as u64) << 20)
                | ((device as u64) << 15)
                | ((function as u64) << 12)
                | (offset as u64);
            unsafe { write_volatile(map_aml_mmio(pci_addr as usize, 4).as_mut_ptr::<u8>(), value) }
        }
    }

    fn write_pci_u16(
        &self,
        segment: u16,
        bus: u8,
        device: u8,
        function: u8,
        offset: u16,
        value: u16,
    ) {
        if let Some(ecam_base) = crate::pcie::get_ecam_base(segment, bus) {
            let pci_addr = ecam_base
                | ((bus as u64) << 20)
                | ((device as u64) << 15)
                | ((function as u64) << 12)
                | (offset as u64);
            unsafe {
                write_volatile(
                    map_aml_mmio(pci_addr as usize, 4).as_mut_ptr::<u16>(),
                    value,
                )
            }
        }
    }

    fn write_pci_u32(
        &self,
        segment: u16,
        bus: u8,
        device: u8,
        function: u8,
        offset: u16,
        value: u32,
    ) {
        if let Some(ecam_base) = crate::pcie::get_ecam_base(segment, bus) {
            let pci_addr = ecam_base
                | ((bus as u64) << 20)
                | ((device as u64) << 15)
                | ((function as u64) << 12)
                | (offset as u64);
            unsafe {
                write_volatile(
                    map_aml_mmio(pci_addr as usize, 4).as_mut_ptr::<u32>(),
                    value,
                )
            }
        }
    }
}
