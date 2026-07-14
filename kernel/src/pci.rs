use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, AtomicU8, Ordering};

const CONFIG_ADDRESS: u16 = 0xCF8;
const CONFIG_DATA: u16 = 0xCFC;

pub static PCIE_ECAM_BASE: AtomicU64 = AtomicU64::new(0);

/// One-shot guard: log the FIRST time an ECAM read returns all-ones but legacy
/// CF8/CFC sees a live device — the q35 GPU-passthrough quirk where ECAM decode
/// for the passed-through device degrades by daemon-run time (reads 0xFFFFFFFF)
/// though legacy port config still reaches it and ECAM read it fine at early boot.
static ECAM_FALLBACK_LOGGED: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// Highest PCI bus number on which `enumerate_inner` has ever seen a live
/// function. Recorded across the Tier-1 legacy scan so `pcie::init` can sanity-
/// check the firmware-declared ECAM bus range before narrowing the re-scan
/// bound (boot-time live-fix #1) — never under-scan below a bus we already
/// know is populated.
static HIGHEST_BUS_SEEN: AtomicU8 = AtomicU8::new(0);

/// The highest PCI bus a device was found on by any prior enumeration scan.
pub fn highest_bus_seen() -> u8 {
    HIGHEST_BUS_SEEN.load(Ordering::Relaxed)
}

#[derive(Debug, Clone)]
pub struct PciDevice {
    pub bus: u8,
    pub device: u8,
    pub function: u8,
    pub vendor_id: u16,
    pub device_id: u16,
    pub class: u8,
    pub subclass: u8,
    pub prog_if: u8,
    pub header_type: u8,
    pub bars: [u32; 6],
    pub irq_line: u8,
    pub irq_pin: u8, // 0=none, 1=INTA#, 2=INTB#, 3=INTC#, 4=INTD#
}

pub fn read_config_32(bus: u8, device: u8, function: u8, offset: u8) -> u32 {
    let ecam_base = PCIE_ECAM_BASE.load(Ordering::Relaxed);
    // ECAM inactive, OR the broken-ECAM quirk already latched (see below): use the
    // legacy CF8/CFC mechanism for BASIC config (offset < 0x100, always — u8 offset).
    if ecam_base == 0 || ECAM_FALLBACK_LOGGED.load(Ordering::Relaxed) {
        return read_config_32_legacy(bus, device, function, offset);
    }
    let addr = ecam_base
        + (((bus as u64) << 20)
            | ((device as u64) << 15)
            | ((function as u64) << 12)
            | (offset as u64 & 0xFFC));
    let val = unsafe {
        let ptr = crate::memory::phys_to_virt(addr).as_ptr::<u32>();
        core::ptr::read_volatile(ptr)
    };
    // Detect a BROKEN ECAM (e.g. q35 GPU-passthrough): the vendor/device dword
    // (offset 0) reads a no-device pattern — all-zeros OR all-ones — via ECAM, yet
    // legacy CF8/CFC sees a real device. On Athena-in-KVM the q35 MMCONFIG at
    // 0xe0000000 isn't reachable through the kernel direct map (ECAM reads 0x0), but
    // the legacy port mechanism reaches it (read 0x15bf1002). LATCH so ALL future
    // basic config routes through legacy. Bare-metal ECAM works (present devices read
    // their real vendor here, not 0/all-ones), so this never latches there.
    if offset == 0 && (val == 0x0000_0000 || val == 0xFFFF_FFFF) {
        let legacy = read_config_32_legacy(bus, device, function, offset);
        if legacy != 0x0000_0000 && legacy != 0xFFFF_FFFF {
            if !ECAM_FALLBACK_LOGGED.swap(true, Ordering::Relaxed) {
                crate::serial_println!(
                    "[pci] ECAM broken for {:02x}:{:02x}.{} (ecam={:#010x} legacy={:#010x} base={:#x}) — routing basic config via legacy CF8/CFC",
                    bus, device, function, val, legacy, ecam_base
                );
            }
            return legacy;
        }
    }
    val
}

/// Force a legacy port-0xCF8/0xCFC config read (bypass ECAM) for BASIC config
/// (offset < 0x100, which the legacy mechanism can address). Used directly when
/// ECAM is inactive and as the all-ones fallback in [`read_config_32`] for the
/// q35 GPU-passthrough quirk.
pub fn read_config_32_legacy(bus: u8, device: u8, function: u8, offset: u8) -> u32 {
    let address = 0x80000000u32
        | ((bus as u32) << 16)
        | ((device as u32) << 11)
        | ((function as u32) << 8)
        | (offset as u32 & 0xFC);
    let mut value: u32;
    unsafe {
        core::arch::asm!("out dx, eax", in("dx") CONFIG_ADDRESS, in("eax") address, options(nomem, nostack, preserves_flags));
        core::arch::asm!("in eax, dx", out("eax") value, in("dx") CONFIG_DATA, options(nomem, nostack, preserves_flags));
    }
    value
}

/// Read a 32-bit dword from PCIe **extended** config space (offset 0..0xFFF).
///
/// Extended config (>= 0x100, where PCIe Extended Capabilities like AER live)
/// is only reachable via ECAM MMIO — legacy port-0xCF8 I/O can't address it.
/// Returns `0xFFFF_FFFF` (all-ones, the "no device/unimplemented" pattern)
/// when ECAM is inactive so callers treat it as "no extended caps".
pub fn read_config_32_ext(bus: u8, device: u8, function: u8, offset: u16) -> u32 {
    let ecam_base = PCIE_ECAM_BASE.load(Ordering::Relaxed);
    if ecam_base == 0 {
        return 0xFFFF_FFFF;
    }
    let addr = ecam_base
        + (((bus as u64) << 20)
            | ((device as u64) << 15)
            | ((function as u64) << 12)
            | (offset as u64 & 0xFFC));
    unsafe {
        let ptr = crate::memory::phys_to_virt(addr).as_ptr::<u32>();
        core::ptr::read_volatile(ptr)
    }
}

/// Write a 32-bit dword to PCIe **extended** config space (offset 0..0xFFF).
/// Like [`read_config_32_ext`], only reachable via ECAM MMIO — a no-op when ECAM
/// is inactive (legacy port-0xCF8 I/O can't address >= 0x100). Used to program
/// AER mask/severity registers (PCIe spec §7.8.4).
pub fn write_config_32_ext(bus: u8, device: u8, function: u8, offset: u16, value: u32) {
    let ecam_base = PCIE_ECAM_BASE.load(Ordering::Relaxed);
    if ecam_base == 0 {
        return;
    }
    let addr = ecam_base
        + (((bus as u64) << 20)
            | ((device as u64) << 15)
            | ((function as u64) << 12)
            | (offset as u64 & 0xFFC));
    unsafe {
        let ptr = crate::memory::phys_to_virt(addr).as_mut_ptr::<u32>();
        core::ptr::write_volatile(ptr, value);
    }
}

pub fn read_config_16(bus: u8, device: u8, function: u8, offset: u8) -> u16 {
    let value = read_config_32(bus, device, function, offset);
    ((value >> ((offset & 2) * 8)) & 0xFFFF) as u16
}

pub fn read_config_8(bus: u8, device: u8, function: u8, offset: u8) -> u8 {
    let value = read_config_32(bus, device, function, offset);
    ((value >> ((offset & 3) * 8)) & 0xFF) as u8
}

pub fn write_config_32(bus: u8, device: u8, function: u8, offset: u8, value: u32) {
    let ecam_base = PCIE_ECAM_BASE.load(Ordering::Relaxed);
    // Once the ECAM-passthrough quirk has been detected this boot (the read fallback
    // in `read_config_32` fired), ECAM is unreliable for the passed-through device —
    // route basic-config WRITES through legacy CF8/CFC too (it addresses offset
    // < 0x100, which this u8-offset path always is, e.g. the command register's
    // bus-master enable). Bare metal never sets the flag, so its writes stay on ECAM.
    if ecam_base != 0 && !ECAM_FALLBACK_LOGGED.load(Ordering::Relaxed) {
        let addr = ecam_base
            + (((bus as u64) << 20)
                | ((device as u64) << 15)
                | ((function as u64) << 12)
                | (offset as u64 & 0xFFC));
        unsafe {
            let ptr = crate::memory::phys_to_virt(addr).as_mut_ptr::<u32>();
            core::ptr::write_volatile(ptr, value);
        }
    } else {
        write_config_32_legacy(bus, device, function, offset, value);
    }
}

/// Force a legacy port-0xCF8/0xCFC config write (bypass ECAM) for BASIC config
/// (offset < 0x100). Used when ECAM is inactive and for the q35-passthrough quirk.
pub fn write_config_32_legacy(bus: u8, device: u8, function: u8, offset: u8, value: u32) {
    let address = 0x80000000u32
        | ((bus as u32) << 16)
        | ((device as u32) << 11)
        | ((function as u32) << 8)
        | (offset as u32 & 0xFC);
    unsafe {
        core::arch::asm!("out dx, eax", in("dx") CONFIG_ADDRESS, in("eax") address, options(nomem, nostack, preserves_flags));
        core::arch::asm!("out dx, eax", in("dx") CONFIG_DATA, in("eax") value, options(nomem, nostack, preserves_flags));
    }
}

pub fn write_config_16(bus: u8, device: u8, function: u8, offset: u8, value: u16) {
    let old = read_config_32(bus, device, function, offset);
    let shift = (offset & 2) * 8;
    let mask = !(0xFFFFu32 << shift);
    let new = (old & mask) | ((value as u32) << shift);
    write_config_32(bus, device, function, offset, new);
}

/// Enable bus-mastering, memory space, and I/O space on a PCI device.
/// Required for DMA-capable devices (NVMe, AHCI, xHCI) on real hardware
/// where firmware may not have enabled these bits.
pub fn enable_bus_mastering(dev: &PciDevice) {
    let cmd = read_config_16(dev.bus, dev.device, dev.function, 0x04);
    let desired = cmd | 0x07; // bits 0=IO, 1=Memory, 2=Bus Master
    if cmd != desired {
        write_config_16(dev.bus, dev.device, dev.function, 0x04, desired);
    }
}

// ── PCI Capability IDs ───────────────────────────────────────────────────

pub const PCI_CAP_MSI: u8 = 0x05;
pub const PCI_CAP_MSIX: u8 = 0x11;

/// Walk the PCI capability linked list and find a capability by ID.
/// Returns the config-space offset of the capability header, or None.
pub fn find_capability(dev: &PciDevice, cap_id: u8) -> Option<u8> {
    let status = read_config_16(dev.bus, dev.device, dev.function, 0x06);
    if (status & (1 << 4)) == 0 {
        return None;
    }

    let mut ptr = read_config_8(dev.bus, dev.device, dev.function, 0x34) & 0xFC;

    for _ in 0..48 {
        if ptr == 0 {
            return None;
        }
        let id = read_config_8(dev.bus, dev.device, dev.function, ptr);
        if id == cap_id {
            return Some(ptr);
        }
        ptr = read_config_8(dev.bus, dev.device, dev.function, ptr + 1) & 0xFC;
    }
    None
}

/// Parsed MSI-X capability information from a PCI device.
#[derive(Debug, Clone)]
pub struct MsixCap {
    pub cap_offset: u8,
    /// Number of MSI-X table entries (1-based).
    pub table_size: u16,
    /// BAR index (0–5) that contains the MSI-X table.
    pub table_bar: u8,
    /// Byte offset within that BAR to the table base.
    pub table_offset: u32,
    /// BAR index for the Pending Bit Array.
    pub pba_bar: u8,
    /// Byte offset within that BAR for the PBA.
    pub pba_offset: u32,
}

/// Parse the MSI-X capability from a PCI device's config space.
pub fn parse_msix_cap(dev: &PciDevice) -> Option<MsixCap> {
    let cap_offset = find_capability(dev, PCI_CAP_MSIX)?;

    let msg_ctrl = read_config_16(dev.bus, dev.device, dev.function, cap_offset + 2);
    let table_size = (msg_ctrl & 0x7FF) + 1;

    let table_info = read_config_32(dev.bus, dev.device, dev.function, cap_offset + 4);
    let table_bar = (table_info & 0x07) as u8;
    let table_offset = table_info & !0x07;

    let pba_info = read_config_32(dev.bus, dev.device, dev.function, cap_offset + 8);
    let pba_bar = (pba_info & 0x07) as u8;
    let pba_offset = pba_info & !0x07;

    Some(MsixCap {
        cap_offset,
        table_size,
        table_bar,
        table_offset,
        pba_bar,
        pba_offset,
    })
}

/// Determine the size of a PCI Base Address Register (BAR).
/// Side-effect: temporarily modifies the BAR value and restores it.
pub fn probe_bar_size(dev: &PciDevice, bar_idx: u8) -> u64 {
    if bar_idx >= 6 {
        return 0;
    }
    let offset = 0x10 + (bar_idx * 4);

    // PCI spec: software MUST disable the device's I/O + memory decode (command
    // register bits 0-1) BEFORE sizing a BAR — writing 0xFFFFFFFF moves the live
    // aperture, and on a QEMU/KVM GPU-passthrough that is FATAL: QEMU remaps the
    // BAR to 0xFFFFFFFF_00000000 the instant the all-ones lands (before the restore
    // below runs) and KVM aborts the VM (kvm_set_user_memory_region: Invalid
    // argument). On bare metal it would just briefly corrupt decode. Save the
    // command word, clear decode, size, restore the BAR, then restore the command.
    let cmd = read_config_32(dev.bus, dev.device, dev.function, 0x04);
    write_config_32(dev.bus, dev.device, dev.function, 0x04, cmd & !0x0003);

    let original = read_config_32(dev.bus, dev.device, dev.function, offset);
    let is_64 = (original & 0x06) == 0x04;

    // Write all 1s to probe size, read the mask, restore.
    write_config_32(dev.bus, dev.device, dev.function, offset, 0xFFFF_FFFF);
    let mask = read_config_32(dev.bus, dev.device, dev.function, offset);
    write_config_32(dev.bus, dev.device, dev.function, offset, original);

    let size = if is_64 && bar_idx < 5 {
        let offset_hi = offset + 4;
        let original_hi = read_config_32(dev.bus, dev.device, dev.function, offset_hi);
        write_config_32(dev.bus, dev.device, dev.function, offset_hi, 0xFFFF_FFFF);
        let mask_hi = read_config_32(dev.bus, dev.device, dev.function, offset_hi);
        write_config_32(dev.bus, dev.device, dev.function, offset_hi, original_hi);

        let full_mask = ((mask_hi as u64) << 32) | (mask as u64);
        !(full_mask & !0x0F) + 1
    } else {
        (!(mask & !0x0F) + 1) as u64
    };

    // Restore the command register (re-enable whatever decode was on).
    write_config_32(dev.bus, dev.device, dev.function, 0x04, cmd);
    size
}

pub fn bar_address(dev: &PciDevice, bar_idx: u8) -> Option<u64> {
    let bar = dev.bars[bar_idx as usize];
    if bar & 1 != 0 {
        return None;
    }
    let bar_type = (bar >> 1) & 0x03;
    let base = (bar & !0x0F) as u64;
    match bar_type {
        0 => Some(base),
        2 if (bar_idx as usize) < 5 => {
            let upper = dev.bars[bar_idx as usize + 1] as u64;
            Some(base | (upper << 32))
        }
        _ => None,
    }
}

static PCI_DEVICES: spin::Mutex<Option<Vec<PciDevice>>> = spin::Mutex::new(None);

/// Return the cached PCI device list, scanning config space on first use.
///
/// NOTE: the first call freezes the cache. If that first call happens before
/// PCIe ECAM is established (see [`PCIE_ECAM_BASE`]), the scan is capped at the
/// legacy bus window (0..=8). Call [`refresh`] once ECAM is active to pick up
/// devices on higher buses (e.g. xHCI on AMD platforms).
pub fn enumerate() -> Vec<PciDevice> {
    let mut cache = PCI_DEVICES.lock();
    if let Some(devs) = cache.as_ref() {
        return devs.clone();
    }
    let devs = enumerate_inner();
    *cache = Some(devs.clone());
    devs
}

/// Re-scan PCI config space, replacing the cached device list. Call this after
/// ECAM (PCIe MMIO config) becomes available so devices on buses above the
/// legacy 0..=8 window are discovered. Idempotent and safe to call repeatedly.
pub fn refresh() -> Vec<PciDevice> {
    let devs = enumerate_inner();
    *PCI_DEVICES.lock() = Some(devs.clone());
    devs
}

fn enumerate_inner() -> Vec<PciDevice> {
    let mut devices = Vec::new();
    let ecam_base = PCIE_ECAM_BASE.load(Ordering::Relaxed);
    // ECAM scan: cap at the firmware-declared highest bus (boot-time live-fix
    // #1) instead of a blind 0..=255 — on Athena buses 0xC6..0xFF are empty and
    // each was a 32-device ECAM-MMIO round-trip (~58 buses, the bulk of the
    // Tier-7 `usb=` re-scan time). `pcie::ecam_max_bus()` returns 255 (full
    // window) when the firmware range was unavailable or looked bogus, so this
    // can never lose a device. Legacy scan stays capped at 8 to avoid hangs.
    let (max_bus, mode) = if ecam_base != 0 {
        (crate::pcie::ecam_max_bus(), "ecam")
    } else {
        (8u8, "legacy")
    };

    crate::serial_println!(
        "[pci] Enumerating devices ({}, max_bus={:#04x})..",
        mode,
        max_bus
    );
    let mut highest_seen = 0u8;
    for bus in 0..=max_bus {
        for device in 0..32 {
            let vendor_id = read_config_16(bus, device, 0, 0x00);
            if vendor_id != 0xFFFF {
                if bus > highest_seen {
                    highest_seen = bus;
                }
                let header_type = read_config_8(bus, device, 0, 0x0E);
                let num_functions = if (header_type & 0x80) != 0 { 8 } else { 1 };

                for function in 0..num_functions {
                    let v_id = read_config_16(bus, device, function, 0x00);
                    if v_id != 0xFFFF {
                        let device_id = read_config_16(bus, device, function, 0x02);
                        let class = read_config_8(bus, device, function, 0x0B);
                        let subclass = read_config_8(bus, device, function, 0x0A);
                        let prog_if = read_config_8(bus, device, function, 0x09);
                        let hdr_type = read_config_8(bus, device, function, 0x0E) & 0x7F;

                        let mut bars = [0; 6];
                        if hdr_type == 0x00 {
                            for i in 0..6 {
                                bars[i] =
                                    read_config_32(bus, device, function, 0x10 + (i as u8) * 4);
                            }
                        }

                        let irq_line = read_config_8(bus, device, function, 0x3C);
                        let irq_pin = read_config_8(bus, device, function, 0x3D);

                        devices.push(PciDevice {
                            bus,
                            device,
                            function,
                            vendor_id: v_id,
                            device_id,
                            class,
                            subclass,
                            prog_if,
                            header_type: hdr_type,
                            bars,
                            irq_line,
                            irq_pin,
                        });

                        let name = pcid::describe(v_id, device_id)
                            .map(|n| n as &str)
                            .unwrap_or("unknown");
                        crate::serial_println!(
                            "[pci] {:02x}:{:02x}.{} {:#06x}:{:#06x} ({}) class {:#04x}/{:#04x}",
                            bus,
                            device,
                            function,
                            v_id,
                            device_id,
                            name,
                            class,
                            subclass
                        );
                    }
                }
            }
        }
    }
    // Record the highest populated bus (monotonic — never lower a prior max, so
    // a narrowed ECAM re-scan can't erase the legacy scan's high-water mark used
    // by pcie::init's safety guard).
    let prev = HIGHEST_BUS_SEEN.load(Ordering::Relaxed);
    if highest_seen > prev {
        HIGHEST_BUS_SEEN.store(highest_seen, Ordering::Relaxed);
    }
    crate::serial_println!(
        "[ OK ] PCI enumeration complete (found {} devices)",
        devices.len()
    );
    devices
}
