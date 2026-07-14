//! PCI + MMIO bridge — Phase 2 of the LinuxKPI host.
//!
//! Linux drivers call `pci_enable_device`, `pci_iomap`/`ioremap`, and
//! `pci_read_config_dword` believing they have Ring-0 PCI access. These shims
//! route to the RaeenOS LinuxKPI host syscalls (132-134, 130, 139), which gate
//! every access behind a device claim + IOMMU sandbox.

use crate::host;

/// Opaque device handle returned by `pci_enable`. A Linux driver stashes this
/// in its `struct pci_dev` private area and passes it back for every later call.
pub type DevHandle = u64;

/// `pci_enable_device` — claim a PCI device by packed bus:device.function.
/// Pack with `(bus << 16) | (dev << 8) | func`.
pub fn pci_enable(bus: u8, dev: u8, func: u8) -> DevHandle {
    let packed = ((bus as u64) << 16) | ((dev as u64) << 8) | (func as u64);
    unsafe { host::sys_pci_enable(packed) }
}

/// Claim the first PCI device matching `class` (+ optional `vendor`, 0 = any) —
/// the Linux `pci_device_id`-table binding model. Use this instead of guessing
/// BDFs: the same call finds a GPU at 00:01.0 on QEMU and c4:00.0 on Athena.
/// Host-side resolution is `LINUXKPI_PCI_MATCH` (bit 63) on syscall 132.
pub fn pci_enable_match(class: u8, vendor: u16) -> DevHandle {
    let spec = rae_abi::syscall::LINUXKPI_PCI_MATCH | ((class as u64) << 16) | (vendor as u64);
    unsafe { host::sys_pci_enable(spec) }
}

/// `ioremap` / `pci_iomap` — map a BAR into a dereferenceable virtual pointer.
/// Returns null on failure (matching Linux semantics).
pub fn ioremap(dev: DevHandle, bar_index: u8) -> *mut u8 {
    let virt = unsafe { host::sys_ioremap(dev, bar_index as u64) };
    if virt >= 0xFFFF_FFFF_FFFF_F000 {
        core::ptr::null_mut()
    } else {
        virt as *mut u8
    }
}

pub fn iounmap(virt: *mut u8, len: usize) {
    if !virt.is_null() {
        unsafe { host::sys_iounmap(virt as u64, len as u64) };
    }
}

/// `pci_read_config_dword`.
pub fn read_config_dword(dev: DevHandle, offset: u16) -> u32 {
    unsafe { host::sys_pci_read_cfg(dev, offset as u64) as u32 }
}

/// `pci_write_config_dword`.
pub fn write_config_dword(dev: DevHandle, offset: u16, value: u32) {
    unsafe { host::sys_pci_write_cfg(dev, offset as u64, value as u64) };
}

// ── MMIO accessors (`readl`/`writel`/`readb`/`writeb`) ────────────────────────
// Linux drivers touch hardware registers through these. Once `ioremap` hands
// back a real virtual pointer to the BAR, these are plain volatile accesses.

#[inline(always)]
pub fn readl(addr: *const u32) -> u32 {
    unsafe { core::ptr::read_volatile(addr) }
}

#[inline(always)]
pub fn writel(value: u32, addr: *mut u32) {
    unsafe { core::ptr::write_volatile(addr, value) }
}

/// 64-bit MMIO write (Linux `writeq` / amdgpu `WDOORBELL64`). Needed for the GPU
/// doorbell aperture, where the engine latches a single atomic 64-bit write of the
/// ring write-pointer — two 32-bit writes could trigger the doorbell mid-update.
#[inline(always)]
pub fn writeq(value: u64, addr: *mut u64) {
    unsafe { core::ptr::write_volatile(addr, value) }
}

#[inline(always)]
pub fn readw(addr: *const u16) -> u16 {
    unsafe { core::ptr::read_volatile(addr) }
}

#[inline(always)]
pub fn writew(value: u16, addr: *mut u16) {
    unsafe { core::ptr::write_volatile(addr, value) }
}

#[inline(always)]
pub fn readb(addr: *const u8) -> u8 {
    unsafe { core::ptr::read_volatile(addr) }
}

#[inline(always)]
pub fn writeb(value: u8, addr: *mut u8) {
    unsafe { core::ptr::write_volatile(addr, value) }
}
