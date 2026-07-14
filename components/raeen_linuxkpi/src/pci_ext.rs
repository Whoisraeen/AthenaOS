//! Extended PCI surface — the `pci_*` calls a real driver makes beyond
//! `pci_enable_device` + config-dword + ioremap (all in `pci.rs`/lib.rs).
//!
//! The device is already claimed + IOMMU-sandboxed by the host at
//! `pci_enable_device` time, so region request / bus-master / IRQ-vector
//! allocation are validated host-side; these shims expose the Linux-shaped
//! entry points. Config byte/word reads decompose the host's dword accessor.

use crate::host;
use crate::pci::{self, DevHandle};

/// `pci_set_master` — enable bus-mastering (DMA). The host already sets the
/// bus-master bit at claim time; re-assert the COMMAND register bit for drivers
/// that toggle it explicitly.
#[no_mangle]
pub extern "C" fn pci_set_master(dev: DevHandle) {
    let cmd = pci::read_config_dword(dev, 0x04);
    pci::write_config_dword(dev, 0x04, cmd | 0x0004); // bit2 = Bus Master Enable
}
#[no_mangle]
pub extern "C" fn pci_clear_master(dev: DevHandle) {
    let cmd = pci::read_config_dword(dev, 0x04);
    pci::write_config_dword(dev, 0x04, cmd & !0x0004);
}

/// `pci_request_regions` / `pci_request_selected_regions` — reserve the BARs.
/// Ownership is already exclusive via the device claim, so this succeeds (0).
#[no_mangle]
pub extern "C" fn pci_request_regions(_dev: DevHandle, _name: *const u8) -> i32 {
    0
}
#[no_mangle]
pub extern "C" fn pci_request_selected_regions(
    _dev: DevHandle,
    _mask: i32,
    _name: *const u8,
) -> i32 {
    0
}
#[no_mangle]
pub extern "C" fn pci_release_regions(_dev: DevHandle) {}
#[no_mangle]
pub extern "C" fn pci_disable_device(_dev: DevHandle) {}

/// `pci_iomap` — map BAR `bar` (Linux passes the BAR index directly).
#[no_mangle]
pub extern "C" fn pci_iomap(dev: DevHandle, bar: i32, _maxlen: u64) -> *mut u8 {
    pci::ioremap(dev, bar as u8)
}
#[no_mangle]
pub extern "C" fn pci_iounmap(_dev: DevHandle, addr: *mut u8) {
    pci::iounmap(addr, 0);
}

// ── Config byte/word accessors (decomposed from the host dword accessor) ──────

#[no_mangle]
pub extern "C" fn pci_read_config_word(dev: DevHandle, offset: u16, out: *mut u16) -> i32 {
    if out.is_null() {
        return -1;
    }
    let aligned = offset & !0x3;
    let shift = (offset & 0x3) * 8;
    let dword = pci::read_config_dword(dev, aligned);
    unsafe { *out = ((dword >> shift) & 0xFFFF) as u16 };
    0
}
#[no_mangle]
pub extern "C" fn pci_read_config_byte(dev: DevHandle, offset: u16, out: *mut u8) -> i32 {
    if out.is_null() {
        return -1;
    }
    let aligned = offset & !0x3;
    let shift = (offset & 0x3) * 8;
    let dword = pci::read_config_dword(dev, aligned);
    unsafe { *out = ((dword >> shift) & 0xFF) as u8 };
    0
}
#[no_mangle]
pub extern "C" fn pci_write_config_word(dev: DevHandle, offset: u16, value: u16) -> i32 {
    let aligned = offset & !0x3;
    let shift = (offset & 0x3) * 8;
    let mut dword = pci::read_config_dword(dev, aligned);
    dword &= !(0xFFFFu32 << shift);
    dword |= (value as u32) << shift;
    pci::write_config_dword(dev, aligned, dword);
    0
}
#[no_mangle]
pub extern "C" fn pci_write_config_byte(dev: DevHandle, offset: u16, value: u8) -> i32 {
    let aligned = offset & !0x3;
    let shift = (offset & 0x3) * 8;
    let mut dword = pci::read_config_dword(dev, aligned);
    dword &= !(0xFFu32 << shift);
    dword |= (value as u32) << shift;
    pci::write_config_dword(dev, aligned, dword);
    0
}

// ── BAR resource queries (pci_resource_start/len/flags) ───────────────────────
// BAR layout: config offset 0x10 + index*4.

fn bar_offset(bar: u32) -> u16 {
    (0x10 + bar * 4) as u16
}

#[no_mangle]
pub extern "C" fn pci_resource_start(dev: DevHandle, bar: u32) -> u64 {
    let lo = pci::read_config_dword(dev, bar_offset(bar));
    if lo & 0x1 != 0 {
        // I/O space BAR.
        return (lo & !0x3) as u64;
    }
    let is_64 = (lo >> 1) & 0x3 == 0x2;
    let base_lo = (lo & !0xF) as u64;
    if is_64 {
        let hi = pci::read_config_dword(dev, bar_offset(bar + 1)) as u64;
        base_lo | (hi << 32)
    } else {
        base_lo
    }
}

/// `pci_resource_len` — probe BAR size by writing all-ones and reading back.
#[no_mangle]
pub extern "C" fn pci_resource_len(dev: DevHandle, bar: u32) -> u64 {
    let off = bar_offset(bar);
    let orig = pci::read_config_dword(dev, off);
    pci::write_config_dword(dev, off, 0xFFFF_FFFF);
    let sized = pci::read_config_dword(dev, off);
    pci::write_config_dword(dev, off, orig);
    if sized == 0 {
        return 0;
    }
    let mask = if orig & 0x1 != 0 {
        sized & !0x3 // I/O
    } else {
        sized & !0xF // memory
    };
    (!mask).wrapping_add(1) as u64
}

#[no_mangle]
pub extern "C" fn pci_resource_flags(dev: DevHandle, bar: u32) -> u64 {
    let lo = pci::read_config_dword(dev, bar_offset(bar));
    // Linux IORESOURCE_MEM=0x200, IORESOURCE_IO=0x100.
    if lo & 0x1 != 0 {
        0x100
    } else {
        0x200
    }
}

// ── DMA mask + IRQ vector allocation ──────────────────────────────────────────

/// The host DMA path is 64-bit capable (contiguous frames + IOMMU), so any
/// requested mask is satisfiable.
#[no_mangle]
pub extern "C" fn dma_set_mask_and_coherent(_dev: u64, _mask: u64) -> i32 {
    0
}
#[no_mangle]
pub extern "C" fn dma_set_mask(_dev: u64, _mask: u64) -> i32 {
    0
}
#[no_mangle]
pub extern "C" fn dma_set_coherent_mask(_dev: u64, _mask: u64) -> i32 {
    0
}

/// `pci_alloc_irq_vectors(dev, min, max, flags)` — the host mints the device's
/// MSI/MSI-X caps at claim time; report the requested count as available.
#[no_mangle]
pub extern "C" fn pci_alloc_irq_vectors(_dev: DevHandle, min: u32, max: u32, _flags: u32) -> i32 {
    max.max(min) as i32
}
#[no_mangle]
pub extern "C" fn pci_free_irq_vectors(_dev: DevHandle) {}

/// `pci_irq_vector(dev, nr)` — map a vector index to its host IRQ handle.
#[no_mangle]
pub extern "C" fn pci_irq_vector(_dev: DevHandle, nr: u32) -> i32 {
    // amdgpu_irq_init() calls this as pci_irq_vector(adev->pdev, nr): the C `dev`
    // is a `struct pci_dev *` pointer, NOT our DevHandle, so it must not be passed
    // to the kernel. Resolve the active device from lkpi_set_current_device (set at
    // daemon init) — the same convention ioremap_phys uses.
    let handle = unsafe { host::sys_request_irq(crate::device_map::current_device(), nr as u64) };
    if handle >= 0xFFFF_FFFF_FFFF_F000 {
        -1
    } else {
        // Encode the low bits as the Linux irq number; the daemon's IRQ thread
        // re-derives the handle via request_irq on the same (dev, vector).
        (nr as i32) + 1
    }
}

/// `pci_find_capability` — walk the PCI capability list for `cap_id`.
#[no_mangle]
pub extern "C" fn pci_find_capability(dev: DevHandle, cap_id: u8) -> u8 {
    // Status register bit 4 = capabilities list present.
    let status = pci::read_config_dword(dev, 0x04) >> 16;
    if status & 0x10 == 0 {
        return 0;
    }
    let mut ptr = (pci::read_config_dword(dev, 0x34) & 0xFF) as u16;
    let mut guard = 0;
    while ptr >= 0x40 && guard < 48 {
        let header = pci::read_config_dword(dev, ptr);
        let id = (header & 0xFF) as u8;
        if id == cap_id {
            return ptr as u8;
        }
        ptr = ((header >> 8) & 0xFF) as u16;
        guard += 1;
    }
    0
}

/// Walk the PCIe *extended* capability list (config space ≥ 0x100) for `cap_id`,
/// returning its offset or 0. Each 32-bit header packs `[15:0]=cap id`,
/// `[19:16]=version`, `[31:20]=next offset`. Factored over a config-dword reader
/// so the walk logic is host-testable without the device.
pub fn walk_ext_cap(read: impl Fn(u16) -> u32, cap_id: u16) -> u16 {
    let mut off: u16 = 0x100;
    let mut guard = 0;
    while off >= 0x100 && guard < 64 {
        let header = read(off);
        if header == 0 || header == 0xFFFF_FFFF {
            break;
        }
        if (header & 0xFFFF) as u16 == cap_id {
            return off;
        }
        off = ((header >> 20) & 0xFFF) as u16; // next ext-cap offset
        guard += 1;
    }
    0
}

/// `pci_find_ext_capability(dev, cap_id)`.
#[no_mangle]
pub extern "C" fn pci_find_ext_capability(dev: DevHandle, cap_id: u16) -> i32 {
    walk_ext_cap(|off| pci::read_config_dword(dev, off), cap_id) as i32
}

/// `pci_device_is_present(dev)` → true if the vendor/device dword is not the
/// all-ones "no device" pattern.
#[no_mangle]
pub extern "C" fn pci_device_is_present(dev: DevHandle) -> bool {
    pci::read_config_dword(dev, 0x00) != 0xFFFF_FFFF
}

/// `pci_msix_vec_count(dev)` → MSI-X table size, or `-EINVAL` if no MSI-X cap.
#[no_mangle]
pub extern "C" fn pci_msix_vec_count(dev: DevHandle) -> i32 {
    let cap = pci_find_capability(dev, 0x11); // PCI_CAP_ID_MSIX
    if cap == 0 {
        return -22;
    }
    // Message Control is the 16 bits at cap+2; table size = bits[10:0] + 1.
    let dword = pci::read_config_dword(dev, cap as u16);
    (((dword >> 16) & 0x7FF) + 1) as i32
}

// ── Power management + driver-model glue ─────────────────────────────────────
// The host owns config-space persistence and PCI power transitions (the device
// is claimed + sandboxed at enable time), so save/restore/power are success
// no-ops; the daemon itself IS the "driver", so register/unregister are
// structural. These exist so a real driver's PM and registration sites link.

#[no_mangle]
pub extern "C" fn pci_save_state(_dev: DevHandle) -> i32 {
    0
}
#[no_mangle]
pub extern "C" fn pci_restore_state(_dev: DevHandle) {}
#[no_mangle]
pub extern "C" fn pci_store_saved_state(_dev: DevHandle) -> *mut u8 {
    core::ptr::null_mut()
}
#[no_mangle]
pub extern "C" fn pci_load_saved_state(_dev: DevHandle, _state: *mut u8) -> i32 {
    0
}
#[no_mangle]
pub extern "C" fn pci_set_power_state(_dev: DevHandle, _state: i32) -> i32 {
    0
}
#[no_mangle]
pub extern "C" fn pci_wake_from_d3(_dev: DevHandle, _enable: bool) -> i32 {
    0
}
#[no_mangle]
pub extern "C" fn pci_ignore_hotplug(_dev: DevHandle) {}
#[no_mangle]
pub extern "C" fn pci_wait_for_pending_transaction(_dev: DevHandle) -> i32 {
    0
}
#[no_mangle]
pub extern "C" fn pci_dev_get(dev: DevHandle) -> DevHandle {
    dev
}
#[no_mangle]
pub extern "C" fn pci_dev_put(_dev: DevHandle) {}
#[no_mangle]
pub extern "C" fn __pci_register_driver(_drv: *mut u8, _owner: *mut u8, _name: *const u8) -> i32 {
    0
}
#[no_mangle]
pub extern "C" fn pci_unregister_driver(_drv: *mut u8) {}
