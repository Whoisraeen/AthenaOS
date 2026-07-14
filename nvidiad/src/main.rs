//! nvidiad — AthenaOS native userspace NVIDIA GPU driver daemon.
//!
//! The from-scratch NVIDIA path, built to the same shape as `i915d`/`amdgpud`:
//! an IOMMU-sandboxed userspace daemon that claims the GPU and walks the
//! bring-up. Stage 1 is chip identification — map BAR0, read `NV_PMC_BOOT_0`,
//! and decode the architecture / chipset / revision with the host-tested
//! [`ath_nvidia::chip`] logic. It then reports the part's *firmware-requirement
//! tier* so the wall is stated up front:
//!
//! * Fermi — no external firmware; a native driver can go all the way.
//! * Kepler..Volta — acceleration needs NVIDIA-signed falcon microcode; modeset
//!   is reachable natively.
//! * Turing and later — full init is mediated by the GSP-RM firmware coprocessor;
//!   a from-scratch driver walls here for engine bring-up (the honest scope
//!   limit called out when the owner chose this path).
//!
//! QEMU has no NVIDIA GPU — the probe exits cleanly (`msg: 9499`). Real NVIDIA
//! silicon walks identification and prints the decoded part + its wall.

#![no_std]
#![no_main]

extern crate alloc;

use core::panic::PanicInfo;
use ath_abi::syscall as abi;
use ath_nvidia::chip::{self, GpuOps};
use ath_nvidia::regs;

const _: () = assert!(ath_abi::ABI_VERSION == 4);

struct LkpiAllocator;

unsafe impl core::alloc::GlobalAlloc for LkpiAllocator {
    unsafe fn alloc(&self, layout: core::alloc::Layout) -> *mut u8 {
        ath_linuxkpi::kmalloc(layout.size(), 0)
    }
    unsafe fn dealloc(&self, ptr: *mut u8, _layout: core::alloc::Layout) {
        ath_linuxkpi::kfree(ptr);
    }
}

#[global_allocator]
static ALLOCATOR: LkpiAllocator = LkpiAllocator;

#[inline(always)]
unsafe fn sys_print(value: u64) {
    core::arch::asm!(
        "syscall",
        in("rax") abi::SYS_PRINT,
        in("rdi") value,
        out("rcx") _, out("r11") _,
    );
}

#[inline(always)]
unsafe fn sys_exit(code: u64) -> ! {
    core::arch::asm!(
        "syscall",
        in("rax") abi::SYS_EXIT,
        in("rdi") code,
        options(noreturn),
    );
}

fn klog(msg: &str) {
    let mut buf = [0u8; 200];
    let n = msg.len().min(199);
    buf[..n].copy_from_slice(&msg.as_bytes()[..n]);
    buf[n] = 0;
    ath_linuxkpi::athena_printk(buf.as_ptr());
}

/// A claimed NVIDIA GPU: its LinuxKPI device handle and BAR0 (MMIO register)
/// mapping. `Nv` implements [`GpuOps`] over the BAR0 pointer so the shared
/// `ath_nvidia` decode runs against real MMIO.
struct Nv {
    lkpi_dev: u64,
    mmio: *mut u8,
}

impl GpuOps for Nv {
    fn reg_read(&mut self, off: u32) -> u32 {
        // Safety: `off` is a bring-up register offset within the mapped BAR0
        // register aperture (16 MiB); the daemon only reads the PMC block at the
        // bottom of that space.
        unsafe { ath_linuxkpi::pci::readl(self.mmio.add(off as usize) as *const u32) }
    }
    fn reg_write(&mut self, off: u32, val: u32) {
        // Safety: same bounded BAR0 aperture as `reg_read`.
        unsafe { ath_linuxkpi::pci::writel(val, self.mmio.add(off as usize) as *mut u32) }
    }
}

/// Id-table match (base-class `0x03` display + NVIDIA vendor) — finds the GPU
/// wherever firmware placed it, then verifies the claim and maps BAR0.
fn probe_match() -> Option<Nv> {
    let lkpi_dev = ath_linuxkpi::pci_enable_match(regs::PCI_CLASS_DISPLAY, regs::NVIDIA_VENDOR)?;
    verify_claimed(lkpi_dev)
}

fn verify_claimed(lkpi_dev: u64) -> Option<Nv> {
    let id = ath_linuxkpi::pci::read_config_dword(lkpi_dev, 0x00);
    let vendor = (id & 0xFFFF) as u16;
    if vendor != regs::NVIDIA_VENDOR {
        return None;
    }
    let class = ath_linuxkpi::pci::read_config_dword(lkpi_dev, 0x08);
    let base_class = ((class >> 24) & 0xFF) as u8;
    if base_class != regs::PCI_CLASS_DISPLAY {
        klog("[nvidia] NVIDIA device is not display class; skipping");
        return None;
    }
    let mmio = ath_linuxkpi::pci::ioremap(lkpi_dev, regs::BAR0_MMIO);
    if mmio.is_null() {
        klog("[nvidia] ioremap(BAR0) failed");
        return None;
    }
    klog("[nvidia] stage 1 PCI probe + ioremap(BAR0) OK");
    Some(Nv { lkpi_dev, mmio })
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    unsafe { sys_print(9400) };
    klog("[nvidia] nvidiad starting: NVIDIA bring-up (chip identification)");

    let mut gpu = match probe_match() {
        Some(g) => g,
        None => {
            klog("[nvidia] no NVIDIA GPU found — nvidiad exiting (expected on QEMU)");
            unsafe { sys_print(9499) };
            unsafe { sys_exit(0) };
        }
    };

    let _ = ath_linuxkpi::lkpi_supervisor_register(gpu.lkpi_dev);
    ath_linuxkpi::lkpi_supervisor_heartbeat(gpu.lkpi_dev);

    // Stage 1: identify the chip from NV_PMC_BOOT_0 (host-tested decode).
    let ident = match chip::identify(&mut gpu) {
        Some(id) => id,
        None => {
            klog("[nvidia] NV_PMC_BOOT_0 did not decode as a valid NVIDIA part (dead/unpowered aperture)");
            unsafe { sys_print(9401) };
            unsafe { sys_exit(1) };
        }
    };

    klog(&alloc::format!(
        "[nvidia] identified {} chipset {:#05x} rev {:#04x} (boot0={:#010x})",
        ident.arch.name(),
        ident.chipset,
        ident.revision,
        ident.boot0
    ));

    // Report the firmware wall up front (honest scope: where a native driver can
    // and cannot proceed on this part).
    let fw = ident.firmware_requirement();
    klog(&alloc::format!(
        "[nvidia] firmware tier: {} — {}",
        match fw {
            chip::FwRequirement::NoFirmware => "NONE",
            chip::FwRequirement::SignedUcode => "SIGNED-UCODE",
            chip::FwRequirement::GspRm => "GSP-RM",
        },
        fw.describe()
    ));
    match fw {
        chip::FwRequirement::GspRm => klog(
            "[nvidia] Turing+ part: engine bring-up requires GSP-RM firmware — native path reaches identification/modeset only",
        ),
        chip::FwRequirement::SignedUcode => klog(
            "[nvidia] pre-GSP part: modeset reachable natively; acceleration needs NVIDIA-signed microcode",
        ),
        chip::FwRequirement::NoFirmware => {
            klog("[nvidia] no firmware wall on this part — native bring-up can continue")
        }
    }

    klog("[nvidia] stage 1 identification complete");
    unsafe { sys_print(9490) };
    unsafe { sys_exit(0) };
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    unsafe {
        sys_print(9499);
        sys_exit(99);
    }
}
