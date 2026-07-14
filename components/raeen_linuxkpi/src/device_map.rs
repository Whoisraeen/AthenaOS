//! Live device MMIO mapping — the bridge from amdgpu's raw `ioremap(phys, size)`
//! to the RaeenOS kernel's BAR-indexed map syscall (`lkpi_ioremap(handle, bar)`).
//!
//! amdgpu maps its register aperture with `adev->rmmio = ioremap(pci_resource_
//! start(pdev, 5), size)` — a RAW physical address, not a BAR index. RaeenOS maps
//! by (device-handle, BAR-index), so this module carries the missing context: the
//! daemon claims the GPU, sets the current device handle, and registers each BAR's
//! physical [start,size). `ioremap(phys, size)` then finds the owning BAR, maps it
//! once via the kernel, and returns `bar_virt + (phys - bar_start)`.
//!
//! Graceful degradation (no feature flag needed): with no device registered
//! (CURRENT_DEV == 0 / empty registry — the host link test), `ioremap` returns
//! null, exactly like the old stub. On the live daemon it returns the real mapping.
//! The phys→BAR translation is pure and host-KAT'd; the syscall is only reached
//! once a real device is registered.

use core::sync::atomic::{AtomicU64, Ordering};

/// The GPU device handle the daemon obtained from `sys_claim_device`/`pci_enable`.
static CURRENT_DEV: AtomicU64 = AtomicU64::new(0);

/// Per-BAR physical [start, size). Index 0..6 (the 6 PCI BARs). start==0 => unset.
const NBARS: usize = 6;
static BAR_START: [AtomicU64; NBARS] = [const { AtomicU64::new(0) }; NBARS];
static BAR_SIZE: [AtomicU64; NBARS] = [const { AtomicU64::new(0) }; NBARS];
/// Cached BAR virtual base once mapped (0 => not yet mapped).
static BAR_VIRT: [AtomicU64; NBARS] = [const { AtomicU64::new(0) }; NBARS];

/// The daemon sets the claimed GPU's device handle before calling amdgpu_device_init.
#[no_mangle]
pub extern "C" fn lkpi_set_current_device(handle: u64) {
    CURRENT_DEV.store(handle, Ordering::SeqCst);
}

/// The daemon registers each BAR's physical window (from the PCI claim) so raw
/// `ioremap(phys, ..)` can resolve which BAR owns an address.
#[no_mangle]
pub extern "C" fn lkpi_register_bar(bar: u32, phys_start: u64, size: u64) {
    let i = bar as usize;
    if i < NBARS {
        BAR_START[i].store(phys_start, Ordering::SeqCst);
        BAR_SIZE[i].store(size, Ordering::SeqCst);
        BAR_VIRT[i].store(0, Ordering::SeqCst);
    }
}

/// Pure translation: which registered BAR contains `phys`, and the offset into it.
/// `None` if no device/BAR covers it (the host-link-test case). Host-KAT'd.
fn bar_for_phys(phys: u64) -> Option<(usize, u64)> {
    if phys == 0 {
        return None;
    }
    for i in 0..NBARS {
        let start = BAR_START[i].load(Ordering::SeqCst);
        let size = BAR_SIZE[i].load(Ordering::SeqCst);
        if start != 0 && size != 0 && phys >= start && phys < start.wrapping_add(size) {
            return Some((i, phys - start));
        }
    }
    None
}

/// If `virt_addr` falls inside BAR5's mapped window (the register aperture),
/// return its byte offset. Used by the readl intercept to spot specific registers
/// (e.g. RCC_IOV_FUNC_IDENTIFIER) during bring-up. Cheap: two atomic loads.
pub fn bar5_offset(virt_addr: u64) -> Option<u64> {
    let base = BAR_VIRT[5].load(Ordering::SeqCst);
    let size = BAR_SIZE[5].load(Ordering::SeqCst);
    if base != 0 && size != 0 && virt_addr >= base && virt_addr < base.wrapping_add(size) {
        Some(virt_addr - base)
    } else {
        None
    }
}

/// Back the C `ioremap(phys, size)` / `memremap`. Maps the owning BAR once (caching
/// its virtual base) and returns the address for `phys`. Null when no device is
/// registered — identical to the previous stub, so the host link test is unaffected.
pub fn ioremap_phys(phys: u64, size: usize) -> *mut u8 {
    let handle = CURRENT_DEV.load(Ordering::SeqCst);
    let (bar, offset) = match bar_for_phys(phys) {
        Some(x) => x,
        None => {
            // Not in any PCI BAR. On an APU the GPU's "VRAM" is a carved-out
            // region of system RAM at a high physical address, beyond the small
            // CPU-visible BAR0 aperture — CPU-mapped kernel BOs (the GART page
            // table, ring/fence buffers) live there. Map it directly via the
            // kernel's SYS_LINUXKPI_MAP_PHYS, which validates the range is
            // reserved/carveout (never usable RAM). Null when no device is set
            // (host link test) or the size is unknown (0).
            if handle == 0 || size == 0 {
                return core::ptr::null_mut();
            }
            let va = unsafe { crate::host::sys_map_phys(handle, phys, size as u64) };
            // The kernel returns E_* error codes with the high bit set.
            if va == 0 || va >= 0x8000_0000_0000_0000 {
                return core::ptr::null_mut();
            }
            return va as *mut u8;
        }
    };
    if handle == 0 {
        return core::ptr::null_mut();
    }
    // map the whole BAR once, then offset into it
    let mut base = BAR_VIRT[bar].load(Ordering::SeqCst);
    if base == 0 {
        base = unsafe { crate::host::sys_ioremap(handle, bar as u64) };
        // sys_ioremap returns u64::MAX on failure
        if base == 0 || base == u64::MAX {
            return core::ptr::null_mut();
        }
        BAR_VIRT[bar].store(base, Ordering::SeqCst);
    }
    (base + offset) as *mut u8
}

/// The current device handle (for dma_alloc/pci-config that need it).
pub fn current_device() -> u64 {
    CURRENT_DEV.load(Ordering::SeqCst)
}

/// A registered BAR's `(phys_start, size)` — `(0, 0)` when unset. Used by the
/// hostrun seam to size its fake BAR mappings from the same single source of
/// truth the live daemon registers.
pub fn registered_bar(bar: u32) -> (u64, u64) {
    let i = bar as usize;
    if i >= NBARS {
        return (0, 0);
    }
    (
        BAR_START[i].load(Ordering::SeqCst),
        BAR_SIZE[i].load(Ordering::SeqCst),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phys_translates_to_the_owning_bar() {
        // no device -> nothing resolves (host link-test behaviour)
        assert_eq!(bar_for_phys(0xdce0_0000), None);
        // register Athena-like BARs: BAR5 regs @ 0xdce00000/256K, BAR2 db @ 0xdc000000/64K
        lkpi_register_bar(5, 0xdce0_0000, 0x0004_0000);
        lkpi_register_bar(2, 0xdc00_0000, 0x0001_0000);
        // exact base -> (bar, 0)
        assert_eq!(bar_for_phys(0xdce0_0000), Some((5, 0)));
        // mid-BAR offset
        assert_eq!(bar_for_phys(0xdce0_1004), Some((5, 0x1004)));
        // the other BAR
        assert_eq!(bar_for_phys(0xdc00_0800), Some((2, 0x800)));
        // just past BAR5 end -> no match
        assert_eq!(bar_for_phys(0xdce4_0000), None);
        // an unrelated address -> no match
        assert_eq!(bar_for_phys(0x1_0000_0000), None);
        // ioremap with no current device set -> null even though a BAR matches
        assert!(ioremap_phys(0xdce0_0000, 4096).is_null());
    }
}
