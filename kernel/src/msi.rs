//! MSI/MSI-X interrupt support for modern PCI devices (NVMe, xHCI, NICs).
//!
//! Concept §Hardware: "Robust MSI/MSI-X vector management at scale".
//!
//! This module provides a bitmap-based vector allocator and support for
//! routing interrupts to specific CPUs.

use crate::interrupts;
use crate::pci::{self, PciDevice};
use alloc::vec::Vec;
use spin::Mutex;

// ── Vector allocator ─────────────────────────────────────────────────────

const MSI_VECTOR_MIN: u8 = 32;
const MSI_VECTOR_MAX: u8 = 255;

struct MsiVectorAllocator {
    bitmap: [u64; 4], // 256 bits
}

impl MsiVectorAllocator {
    const fn new() -> Self {
        Self { bitmap: [0; 4] }
    }

    fn alloc(&mut self) -> Option<u8> {
        // Start from MSI_VEC_BASE
        let start = interrupts::MSI_VEC_BASE as usize;
        let end = core::cmp::min(255, start + interrupts::MSI_VEC_COUNT);

        for i in start..end {
            let word = i / 64;
            let bit = i % 64;
            if (self.bitmap[word] & (1 << bit)) == 0 {
                self.bitmap[word] |= 1 << bit;
                return Some(i as u8);
            }
        }
        None
    }

    fn free(&mut self, vector: u8) {
        let i = vector as usize;
        let word = i / 64;
        let bit = i % 64;
        self.bitmap[word] &= !(1 << bit);
    }
}

static ALLOCATOR: Mutex<MsiVectorAllocator> = Mutex::new(MsiVectorAllocator::new());

pub fn allocate_msi_vector() -> Option<u8> {
    ALLOCATOR.lock().alloc()
}

pub fn free_msi_vector(vector: u8) {
    ALLOCATOR.lock().free(vector);
}

// ── MSI-X table entry layout (16 bytes, device MMIO) ────────────────────

const MSIX_ENTRY_SIZE: u64 = 16;
const MSI_ADDR_BASE: u32 = 0xFEE0_0000;

// ── Public API ───────────────────────────────────────────────────────────

/// Build a 32-bit MSI message address for a specific APIC ID.
pub fn msi_address(apic_id: u32) -> u32 {
    // Bits 31-20: 0xFEE (base)
    // Bits 19-12: Destination APIC ID
    // Bit 3: Redirection Hint (0 = disabled)
    // Bit 2: Destination Mode (0 = physical)
    MSI_ADDR_BASE | (apic_id << 12)
}

/// Enable MSI-X on `dev` and allocate `count` interrupt vectors.
pub fn enable_msix(dev: &PciDevice, count: usize) -> Result<Vec<u8>, &'static str> {
    let cap = pci::parse_msix_cap(dev).ok_or("device does not support MSI-X")?;

    if count == 0 || count > cap.table_size as usize {
        return Err("invalid vector count");
    }

    let bar_phys = pci::bar_address(dev, cap.table_bar).ok_or("invalid BAR")?;
    // 64-bit BARs can sit above the linear physmap; map the MMIO region
    // (creates PTEs + disables caching) instead of assuming it's mapped.
    let mut bar_size = crate::pci::probe_bar_size(dev, cap.table_bar);
    if bar_size == 0 {
        bar_size = 0x4000;
    } // Fallback if probing fails while MemSpace is enabled
    let table_virt = crate::arch::mmu::kernel()
        .map_mmio_range(
            x86_64::PhysAddr::new(bar_phys),
            bar_size as usize,
            crate::arch::mmu::PageFlags::DEVICE,
        )
        .as_u64()
        + cap.table_offset as u64;

    // Set Function Mask to pause interrupts
    let orig_ctrl = pci::read_config_16(dev.bus, dev.device, dev.function, cap.cap_offset + 2);
    pci::write_config_16(
        dev.bus,
        dev.device,
        dev.function,
        cap.cap_offset + 2,
        orig_ctrl | (1 << 14),
    );

    let mut vectors = Vec::with_capacity(count);
    for i in 0..count {
        let vector = allocate_msi_vector().ok_or("out of MSI vectors")?;
        vectors.push(vector);

        let entry_ptr = (table_virt + (i as u64) * MSIX_ENTRY_SIZE) as *mut u32;
        unsafe {
            // Address: Target BSP (APIC ID 0) by default.
            // Real drivers can later use `re-route` API to target other cores.
            core::ptr::write_volatile(entry_ptr, msi_address(0));
            core::ptr::write_volatile(entry_ptr.add(1), 0); // addr_hi
            core::ptr::write_volatile(entry_ptr.add(2), vector as u32); // data
            core::ptr::write_volatile(entry_ptr.add(3), 0); // unmask
        }
    }

    // Enable MSI-X and clear mask
    pci::write_config_16(
        dev.bus,
        dev.device,
        dev.function,
        cap.cap_offset + 2,
        (orig_ctrl | (1 << 15)) & !(1 << 14),
    );

    // Disable legacy INTx
    let cmd = pci::read_config_16(dev.bus, dev.device, dev.function, 0x04);
    pci::write_config_16(dev.bus, dev.device, dev.function, 0x04, cmd | (1 << 10));

    Ok(vectors)
}

/// Try MSI-X first, then fall back to legacy INTx.
/// Returns allocated vectors on success, or `None` when using INTx.
pub fn try_enable_msix_or_intx(dev: &PciDevice, count: usize) -> Option<Vec<u8>> {
    match enable_msix(dev, count) {
        Ok(v) => Some(v),
        Err(_) => {
            let cmd = pci::read_config_16(dev.bus, dev.device, dev.function, 0x04);
            // Clear INTx disable bit so legacy pin interrupts remain active.
            pci::write_config_16(dev.bus, dev.device, dev.function, 0x04, cmd & !(1 << 10));
            None
        }
    }
}
