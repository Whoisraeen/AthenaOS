//! Hardware-ID → driver-package matcher (auto driver scan + install).
//!
//! Concept §"it just works" / driver model: RaeenOS does NOT ship every driver.
//! The installer scans the PCIe + USB buses, collects every Hardware ID
//! (Vendor:Device), and matches each against this **built-in manifest**. It then
//! copies only the LinuxKPI userspace driver packages the machine actually needs
//! (e.g. `amdgpud.elf` for a Radeon 780M, `iwlwifi.elf` for an Intel Wi-Fi part)
//! onto the target drive. At boot, the driver-manager daemon reads the hardware
//! list, launches those exact executables, and each one calls `sys_claim_device`
//! (syscall 111, `rae_abi::SYS_DRIVER_CLAIM_DEVICE`) to take its device from the
//! kernel.
//!
//! This module is the **Scan + Match** half of that pipeline (the kernel side).
//! It classifies every PCI function as:
//!   - [`DriverKind::Builtin`]  — an in-kernel RaeenOS driver already handles it
//!                                (NVMe, AHCI, xHCI, virtio, e1000, igc, HDA);
//!                                nothing to install.
//!   - [`DriverKind::LinuxKpi`] — needs a userspace LinuxKPI driver package the
//!                                installer must copy + the daemon must launch
//!                                (amdgpud, i915d, iwlwifi, …).
//!   - [`DriverKind::None`]     — no driver required (host bridges, etc.).
//!
//! R10: `init()` + `run_boot_smoketest()` + `/proc/raeen/drivers` (`dump_text`) +
//! this Concept docstring.

#![allow(dead_code)]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, Ordering};

// ── PCI class codes (base class) ────────────────────────────────────────────
const CLASS_STORAGE: u8 = 0x01;
const CLASS_NETWORK: u8 = 0x02;
const CLASS_DISPLAY: u8 = 0x03;
const CLASS_MULTIMEDIA: u8 = 0x04;
const CLASS_BRIDGE: u8 = 0x06;
const CLASS_SERIAL_BUS: u8 = 0x0C;

// Storage subclasses
const SUB_SATA: u8 = 0x06;
const SUB_NVME: u8 = 0x08;
// Network subclasses
const SUB_ETHERNET: u8 = 0x00;
const SUB_NET_OTHER: u8 = 0x80; // Wi-Fi commonly reports here
                                // Serial-bus subclasses
const SUB_USB: u8 = 0x03;
// Multimedia subclasses
const SUB_HDA: u8 = 0x03;

// ── Vendor IDs ──────────────────────────────────────────────────────────────
const VEN_AMD: u16 = 0x1002; // AMD / ATI GPUs
const VEN_INTEL: u16 = 0x8086;
const VEN_NVIDIA: u16 = 0x10DE;
const VEN_VIRTIO: u16 = 0x1AF4; // Red Hat / virtio
const VEN_QEMU: u16 = 0x1B36; // Red Hat / QEMU devices
const VEN_REALTEK: u16 = 0x10EC;
const VEN_BROADCOM: u16 = 0x14E4;
const VEN_QEMU_VGA: u16 = 0x1234; // QEMU stdvga / bochs

/// How a matched device is serviced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriverKind {
    /// In-kernel RaeenOS driver handles it — no install, no daemon.
    Builtin,
    /// Needs a userspace LinuxKPI driver package (installer copies, daemon
    /// launches, driver claims via `sys_claim_device`).
    LinuxKpi,
    /// No driver required (bridge / unclassified).
    None,
}

/// The manifest's decision for one device.
#[derive(Debug, Clone, Copy)]
pub struct DriverMatch {
    /// Package / driver name (e.g. "amdgpud", "nvme", "iwlwifi", "-").
    pub package: &'static str,
    pub kind: DriverKind,
}

impl DriverMatch {
    const fn builtin(name: &'static str) -> Self {
        Self {
            package: name,
            kind: DriverKind::Builtin,
        }
    }
    const fn linuxkpi(name: &'static str) -> Self {
        Self {
            package: name,
            kind: DriverKind::LinuxKpi,
        }
    }
    const fn none() -> Self {
        Self {
            package: "-",
            kind: DriverKind::None,
        }
    }
}

/// Exact `(vendor, device)` overrides for parts we care about by name. Checked
/// before the class-based fallback so a specific board gets its specific driver.
fn match_exact(vendor: u16, device: u16) -> Option<DriverMatch> {
    Some(match (vendor, device) {
        // AMD Radeon 780M (Phoenix RDNA3 iGPU — the Athena target).
        (VEN_AMD, 0x15BF) => DriverMatch::linuxkpi("amdgpud"),
        // Intel I225-V 2.5G NIC — handled by the in-kernel `igc` driver.
        (VEN_INTEL, 0x15F3) => DriverMatch::builtin("igc"),
        (VEN_INTEL, 0x15F2) => DriverMatch::builtin("igc"),
        // Intel I219 family — in-kernel e1000e-class path.
        (VEN_INTEL, 0x15FB) => DriverMatch::builtin("e1000"),
        // Realtek RTL8125 2.5G NIC — needs a userspace driver.
        (VEN_REALTEK, 0x8125) => DriverMatch::linuxkpi("r8125"),
        (VEN_REALTEK, 0x8168) => DriverMatch::linuxkpi("r8169"),
        // QEMU/emulated parts we boot with (proves the matcher under QEMU).
        (VEN_INTEL, 0x100E) => DriverMatch::builtin("e1000"), // e1000 NIC
        (VEN_INTEL, 0x2922) => DriverMatch::builtin("ahci"),  // ICH9 AHCI
        (VEN_QEMU, 0x0010) => DriverMatch::builtin("nvme"),   // QEMU NVMe
        (VEN_QEMU, 0x000D) => DriverMatch::builtin("xhci"),   // qemu-xhci
        _ => return None,
    })
}

/// Class-based fallback for parts without an exact override.
fn match_by_class(vendor: u16, class: u8, subclass: u8) -> DriverMatch {
    match (class, subclass) {
        (CLASS_DISPLAY, _) => match vendor {
            VEN_AMD => DriverMatch::linuxkpi("amdgpud"),
            VEN_INTEL => DriverMatch::linuxkpi("i915d"),
            VEN_NVIDIA => DriverMatch::linuxkpi("nvidiad"),
            VEN_VIRTIO => DriverMatch::builtin("virtio-gpu"),
            VEN_QEMU_VGA => DriverMatch::builtin("vga"),
            _ => DriverMatch::builtin("vga"),
        },
        (CLASS_NETWORK, SUB_ETHERNET) => match vendor {
            VEN_INTEL => DriverMatch::builtin("igc"),
            VEN_VIRTIO => DriverMatch::builtin("virtio-net"),
            VEN_REALTEK => DriverMatch::linuxkpi("r8169"),
            _ => DriverMatch::linuxkpi("net-generic"),
        },
        // Wi-Fi typically reports as Network/Other (0x02/0x80).
        (CLASS_NETWORK, SUB_NET_OTHER) => match vendor {
            VEN_INTEL => DriverMatch::linuxkpi("iwlwifi"),
            VEN_BROADCOM => DriverMatch::linuxkpi("brcmfmac"),
            VEN_REALTEK => DriverMatch::linuxkpi("rtw89"),
            _ => DriverMatch::linuxkpi("wifi-generic"),
        },
        (CLASS_STORAGE, sub) => match (vendor, sub) {
            (VEN_VIRTIO, _) => DriverMatch::builtin("virtio-blk"),
            (_, SUB_NVME) => DriverMatch::builtin("nvme"),
            (_, SUB_SATA) => DriverMatch::builtin("ahci"),
            _ => DriverMatch::none(), // legacy IDE etc.
        },
        (CLASS_SERIAL_BUS, SUB_USB) => DriverMatch::builtin("xhci"),
        (CLASS_MULTIMEDIA, SUB_HDA) => DriverMatch::builtin("hda"),
        (CLASS_BRIDGE, _) => DriverMatch::none(),
        _ => DriverMatch::none(),
    }
}

/// Resolve the driver for a PCI function. Exact `(vendor, device)` wins; then
/// class/subclass; then nothing.
pub fn match_pci(vendor: u16, device: u16, class: u8, subclass: u8) -> DriverMatch {
    if let Some(m) = match_exact(vendor, device) {
        return m;
    }
    match_by_class(vendor, class, subclass)
}

// ── Last-scan results (lock-free counters for procfs/smoketest) ──────────────
static SCAN_DEVICES: AtomicU32 = AtomicU32::new(0);
static SCAN_BUILTIN: AtomicU32 = AtomicU32::new(0);
static SCAN_LINUXKPI: AtomicU32 = AtomicU32::new(0);
static SCAN_NONE: AtomicU32 = AtomicU32::new(0);

/// Scan PCI and return `(device, match)` for every function. Read-only.
pub fn scan_pci() -> Vec<(crate::pci::PciDevice, DriverMatch)> {
    let devs = crate::pci::enumerate();
    let mut out = Vec::with_capacity(devs.len());
    let (mut nb, mut nl, mut nn) = (0u32, 0u32, 0u32);
    for d in devs {
        let m = match_pci(d.vendor_id, d.device_id, d.class, d.subclass);
        match m.kind {
            DriverKind::Builtin => nb += 1,
            DriverKind::LinuxKpi => nl += 1,
            DriverKind::None => nn += 1,
        }
        out.push((d, m));
    }
    SCAN_DEVICES.store(out.len() as u32, Ordering::Relaxed);
    SCAN_BUILTIN.store(nb, Ordering::Relaxed);
    SCAN_LINUXKPI.store(nl, Ordering::Relaxed);
    SCAN_NONE.store(nn, Ordering::Relaxed);
    out
}

/// The distinct LinuxKPI driver packages this machine needs installed — the
/// list the installer copies and the driver-manager daemon launches.
pub fn required_linuxkpi_packages() -> Vec<&'static str> {
    let mut pkgs: Vec<&'static str> = Vec::new();
    for (_, m) in scan_pci() {
        if m.kind == DriverKind::LinuxKpi && !pkgs.contains(&m.package) {
            pkgs.push(m.package);
        }
    }
    pkgs
}

pub fn init() {
    crate::serial_println!("[ OK ] Driver manifest (HWID → package matcher) ready");
}

/// R10 boot smoketest: scan the live PCI bus and resolve every device, then
/// run synthetic probes for real-hardware targets QEMU does not expose (AMD
/// 780M, Intel iGPU, Intel I225-V, Intel Wi-Fi) so the manifest is proven for
/// the machines RaeenOS actually targets.
pub fn run_boot_smoketest() {
    let matches = scan_pci();
    crate::serial_println!(
        "[drvman] PCI scan: {} devices, builtin={} linuxkpi={} none={}",
        SCAN_DEVICES.load(Ordering::Relaxed),
        SCAN_BUILTIN.load(Ordering::Relaxed),
        SCAN_LINUXKPI.load(Ordering::Relaxed),
        SCAN_NONE.load(Ordering::Relaxed),
    );
    for (d, m) in &matches {
        let kind = match m.kind {
            DriverKind::Builtin => "builtin",
            DriverKind::LinuxKpi => "linuxkpi",
            DriverKind::None => "none",
        };
        crate::serial_println!(
            "[drvman]   {:02x}:{:02x}.{} {:04x}:{:04x} class {:02x}/{:02x} -> {} ({})",
            d.bus,
            d.device,
            d.function,
            d.vendor_id,
            d.device_id,
            d.class,
            d.subclass,
            m.package,
            kind,
        );
    }

    // Synthetic probes for the real targets (vendor, device, class, subclass).
    let amd_780m = match_pci(VEN_AMD, 0x15BF, CLASS_DISPLAY, 0x00);
    let intel_igpu = match_pci(VEN_INTEL, 0xA7A0, CLASS_DISPLAY, 0x00); // generic Intel iGPU
    let nv_gpu = match_pci(VEN_NVIDIA, 0x2684, CLASS_DISPLAY, 0x00); // RTX 4090 (AD102)
    let i225v = match_pci(VEN_INTEL, 0x15F3, CLASS_NETWORK, SUB_ETHERNET);
    let iwlwifi = match_pci(VEN_INTEL, 0x2725, CLASS_NETWORK, SUB_NET_OTHER); // AX210
    let nvme = match_pci(VEN_QEMU, 0x0010, CLASS_STORAGE, SUB_NVME);

    let amd_ok = amd_780m.package == "amdgpud" && amd_780m.kind == DriverKind::LinuxKpi;
    let intel_gpu_ok = intel_igpu.package == "i915d" && intel_igpu.kind == DriverKind::LinuxKpi;
    let nv_gpu_ok = nv_gpu.package == "nvidiad" && nv_gpu.kind == DriverKind::LinuxKpi;
    let i225_ok = i225v.package == "igc" && i225v.kind == DriverKind::Builtin;
    let wifi_ok = iwlwifi.package == "iwlwifi" && iwlwifi.kind == DriverKind::LinuxKpi;
    let nvme_ok = nvme.package == "nvme" && nvme.kind == DriverKind::Builtin;

    let pass = amd_ok && intel_gpu_ok && nv_gpu_ok && i225_ok && wifi_ok && nvme_ok;
    crate::serial_println!(
        "[drvman] manifest selftest: amd780m={} intel_igpu={} nvidia={} i225v={} iwlwifi={} nvme={} -> {}",
        amd_ok,
        intel_gpu_ok,
        nv_gpu_ok,
        i225_ok,
        wifi_ok,
        nvme_ok,
        if pass { "PASS" } else { "FAIL" }
    );

    let pkgs = required_linuxkpi_packages();
    crate::serial_println!(
        "[drvman] LinuxKPI packages to install on this machine: {}",
        if pkgs.is_empty() {
            String::from("(none — all devices builtin)")
        } else {
            pkgs.join(", ")
        }
    );
}

/// `/proc/raeen/drivers` body: the hardware list + matched driver per device,
/// plus the LinuxKPI install set. This is the literal artifact the installer
/// and the driver-manager daemon consume.
pub fn dump_text() -> String {
    let matches = scan_pci();
    let mut out = String::from("# RaeenOS driver manifest (HWID → driver package)\n");
    out.push_str(&format!(
        "devices: {}  builtin: {}  linuxkpi: {}  none: {}\n\n",
        SCAN_DEVICES.load(Ordering::Relaxed),
        SCAN_BUILTIN.load(Ordering::Relaxed),
        SCAN_LINUXKPI.load(Ordering::Relaxed),
        SCAN_NONE.load(Ordering::Relaxed),
    ));
    out.push_str("# bdf        hwid        class  driver         kind\n");
    for (d, m) in &matches {
        let kind = match m.kind {
            DriverKind::Builtin => "builtin",
            DriverKind::LinuxKpi => "linuxkpi",
            DriverKind::None => "none",
        };
        out.push_str(&format!(
            "{:02x}:{:02x}.{}    {:04x}:{:04x}   {:02x}/{:02x}  {:<14} {}\n",
            d.bus,
            d.device,
            d.function,
            d.vendor_id,
            d.device_id,
            d.class,
            d.subclass,
            m.package,
            kind,
        ));
    }
    out.push_str("\n# LinuxKPI packages the installer copies + the driver manager launches:\n");
    let pkgs = required_linuxkpi_packages();
    if pkgs.is_empty() {
        out.push_str("  (none — every device is handled by an in-kernel driver)\n");
    } else {
        for p in pkgs {
            out.push_str(&format!("  {}.elf\n", p));
        }
    }
    out
}
