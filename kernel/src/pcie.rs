//! PCIe ECAM Discovery and Initialization.
//!
//! This module implements robust discovery of the PCIe Enhanced Configuration
//! Access Mechanism (ECAM) base address. ECAM allows accessing PCI configuration
//! space via MMIO instead of legacy I/O ports (0xCF8/0xCFC), which is required
//! for accessing extended capabilities and higher bus numbers on modern systems.
//!
//! Discovery order:
//! 1. ACPI MCFG table (standard).
//! 2. AMD MMIO_CONFIG_BASE MSR (Zen-specific).
//! 3. PCI Quirk Engine (hardcoded known bases for buggy firmware).

use crate::pci::PCIE_ECAM_BASE;
use core::sync::atomic::{AtomicU8, Ordering};
use spin::Once;

static MCFG_BASE0: Once<u64> = Once::new();

/// Firmware-reported highest ECAM bus number (segment 0). Set by [`init`] from
/// the MCFG `end_bus` (preferred) or the AMD `MMIO_CONFIG_BASE` bus-range MSR.
/// `pci::enumerate_inner` reads this as the ECAM scan upper bound instead of a
/// blind 0..=255 (boot-time live-fix #1: ~58 empty buses on Athena were each a
/// 32-device ECAM-MMIO round-trip — the bulk of Tier-7 `usb=` time). Default
/// `255` = scan the full window (no behavior change until [`init`] narrows it).
pub static PCIE_ECAM_MAX_BUS: AtomicU8 = AtomicU8::new(255);

/// Highest PCIe bus the firmware declares reachable via ECAM (segment 0). Used
/// by `pci::enumerate_inner` to cap the ECAM scan. Returns `255` (full window)
/// when ECAM is inactive or the firmware range was unavailable/bogus.
pub fn ecam_max_bus() -> u8 {
    PCIE_ECAM_MAX_BUS.load(Ordering::Relaxed)
}

/// Initialize PCIe ECAM discovery.
///
/// Tries multiple discovery methods to find the ECAM base address.
/// If found, updates `pci::PCIE_ECAM_BASE` to enable MMIO config access.
pub fn init() {
    let mut base = None;
    let mut source = "None";

    // Method A: ACPI MCFG (via acpi_full)
    // Most reliable method on standard-compliant firmware.
    if let Some(mcfg_base) = crate::acpi_full::ACPI_SUBSYSTEM.lock().parse_mcfg() {
        base = Some(mcfg_base);
        source = "ACPI MCFG";
    }

    // Method B: AMD MSR (MMIO_CONFIG_BASE)
    // Reliable on AMD Zen systems even if MCFG is missing or malformed.
    if base.is_none() {
        if let Some(amd_base) = discover_amd_ecam_msr() {
            base = Some(amd_base);
            source = "AMD MSR";
        }
    }

    // Method C: PCI Quirk Engine
    // Last resort for known buggy hardware.
    if base.is_none() {
        // Probe bus 0, device 0, function 0 using legacy I/O
        let vendor = crate::pci::read_config_16(0, 0, 0, 0x00);
        let device = crate::pci::read_config_16(0, 0, 0, 0x02);
        if vendor != 0xFFFF {
            if let Some(quirk) = crate::pcie_quirks::lookup_ecam_override(vendor, device) {
                base = Some(quirk.corrected_ecam_base);
                source = "PCI Quirk Engine";
            }
        }
    }

    if let Some(b) = base {
        MCFG_BASE0.call_once(|| b);

        // Ensure ECAM region is mapped (256MB for full bus range)
        let ecam_size = 256 * 1024 * 1024;
        let virt = crate::arch::mmu::kernel().map_mmio_range(
            x86_64::PhysAddr::new(b),
            ecam_size,
            crate::arch::mmu::PageFlags::DEVICE,
        );

        PCIE_ECAM_BASE.store(b, Ordering::Relaxed);
        crate::serial_println!("[pcie] ECAM active @ {:#x} (source: {})", b, source);

        // Boot-time live-fix #1: cap the ECAM scan at the firmware-declared
        // highest bus instead of a blind 0..=255 round-trip. Derive the bound
        // from the firmware (MCFG end-bus preferred, AMD MSR bus-range as a
        // fallback) and validate it against what Tier 1's legacy scan actually
        // saw, so we can never under-scan.
        derive_and_store_max_bus();
    } else {
        crate::serial_println!("[pcie] no ECAM found — falling back to legacy CF8/CFC access");
    }
}

/// Compute the ECAM scan upper bound and store it in [`PCIE_ECAM_MAX_BUS`].
///
/// IMPORTANT — why not the MCFG `end_bus`: on the Athena (and most AMD desktop
/// firmware) MCFG declares the FULL `buses=0-255` range even though the topmost
/// populated function is bus 0xC5. The PCI-spec-authoritative bound is therefore
/// 255 and yields no saving. The real, still-safe ceiling is *topological*: a
/// function can only exist on a bus that is in some PCI-PCI bridge's
/// `[secondary, subordinate]` window, and every such window is rooted at a
/// host-bridge/root-port on bus 0. So the highest `subordinate bus number`
/// (config offset 0x1A) across all bridges reachable from bus 0 is the highest
/// bus any device can live on. On Athena that collapses 0..=0xFF to ~0..=0xC5.
///
/// Derivation order (each strictly bounds device existence):
/// 1. Max subordinate-bus across the bridge tree rooted at bus 0 (topology).
/// 2. MCFG `end_bus` (PCI Firmware Spec hard ceiling) — clamps #1, also the
///    fallback when no bridges are present.
/// 3. `255` (full window) when neither is available.
///
/// SAFETY GUARD: if the computed bound is `0` or *below* the highest bus the
/// Tier-1 legacy scan already populated, treat it as bogus — fall back to 255 +
/// WARN rather than silently drop a bus.
fn derive_and_store_max_bus() {
    let seen_max = crate::pci::highest_bus_seen();

    // MCFG end-bus is the firmware hard ceiling (usually 0xFF here).
    let mcfg_end = crate::acpi_full::ACPI_SUBSYSTEM
        .lock()
        .parse_mcfg_end_bus()
        .filter(|&e| e != 0)
        .unwrap_or(255);

    // Topological ceiling: walk the bridge tree from bus 0, tracking the highest
    // subordinate bus number. Clamp to the MCFG ceiling so we never probe a bus
    // the firmware says is out of range.
    let (topo, source) = match max_subordinate_bus(mcfg_end) {
        Some(sub) => (sub.min(mcfg_end), "bridge subordinate"),
        None => (mcfg_end, "MCFG end-bus"),
    };

    if topo == 0 || topo < seen_max {
        crate::serial_println!(
            "[pcie] WARN: derived ECAM bound {:#04x} ({}) below highest seen bus {:#04x} — scanning full 0..=0xFF",
            topo,
            source,
            seen_max
        );
        PCIE_ECAM_MAX_BUS.store(255, Ordering::Relaxed);
    } else {
        PCIE_ECAM_MAX_BUS.store(topo, Ordering::Relaxed);
        crate::serial_println!(
            "[pcie] ECAM scan bound: max_bus={:#04x} (source: {}, mcfg_end={:#04x}, tier1-seen {:#04x})",
            topo,
            source,
            mcfg_end,
            seen_max
        );
    }
}

/// Walk the PCI-PCI bridge tree (via ECAM, which is now active) starting at the
/// root buses and return the highest `subordinate bus number` (config offset
/// 0x1A) found on any bridge. Every device-bearing bus is inside some bridge's
/// `[secondary, subordinate]` window, so this is a safe upper bound for the full
/// enumeration scan. Bounded by `ceiling` (the MCFG hard ceiling) and by a fixed
/// visited-bus budget so a malformed loop in firmware tables can't spin.
///
/// Returns `None` when no bridges are found (e.g. QEMU's flat bus 0), in which
/// case the caller keeps the firmware ceiling.
fn max_subordinate_bus(ceiling: u8) -> Option<u8> {
    // Type-1 (bridge) header: secondary @0x19, subordinate @0x1A.
    const HEADER_TYPE: u8 = 0x0E;
    const SUBORDINATE: u8 = 0x1A;
    const SECONDARY: u8 = 0x19;

    let mut max_sub: Option<u8> = None;
    // BFS over secondary buses, starting from root bus 0. A visited budget caps
    // work even if firmware presents a cyclic bridge graph.
    let mut queue: [u8; 256] = [0; 256];
    let mut qlen = 1usize; // queue[0] = bus 0
    let mut head = 0usize;
    let mut visited = [false; 256];
    visited[0] = true;
    let mut budget = 256usize;

    while head < qlen && budget > 0 {
        budget -= 1;
        let bus = queue[head];
        head += 1;
        if bus > ceiling {
            continue;
        }
        for device in 0u8..32 {
            let vendor = crate::pci::read_config_16(bus, device, 0, 0x00);
            if vendor == 0xFFFF {
                continue;
            }
            let hdr0 = crate::pci::read_config_8(bus, device, 0, HEADER_TYPE);
            let nfn = if (hdr0 & 0x80) != 0 { 8u8 } else { 1u8 };
            for function in 0..nfn {
                let v = crate::pci::read_config_16(bus, device, function, 0x00);
                if v == 0xFFFF {
                    continue;
                }
                let hdr = crate::pci::read_config_8(bus, device, function, HEADER_TYPE) & 0x7F;
                if hdr == 0x01 {
                    // PCI-PCI bridge — record subordinate, descend into secondary.
                    let sub = crate::pci::read_config_8(bus, device, function, SUBORDINATE);
                    let sec = crate::pci::read_config_8(bus, device, function, SECONDARY);
                    if sub <= ceiling {
                        max_sub = Some(max_sub.map_or(sub, |m| m.max(sub)));
                    }
                    if sec != 0 && sec <= ceiling && !visited[sec as usize] && qlen < queue.len() {
                        visited[sec as usize] = true;
                        queue[qlen] = sec;
                        qlen += 1;
                    }
                }
            }
        }
    }
    max_sub
}

/// AMD-specific ECAM discovery via MSR 0xC001_0058 (MMIO_CONFIG_BASE).
fn discover_amd_ecam_msr() -> Option<u64> {
    // Standard AMD vendor check via CPUID 0
    let mut bytes = [0u8; 12];
    let r = crate::cpu_features::cpuid_raw(0, 0);
    bytes[0..4].copy_from_slice(&r.ebx.to_le_bytes());
    bytes[4..8].copy_from_slice(&r.edx.to_le_bytes());
    bytes[8..12].copy_from_slice(&r.ecx.to_le_bytes());

    if &bytes != b"AuthenticAMD" {
        return None;
    }

    // MMIO_CONFIG_BASE MSR layout:
    // Bit 0: Enable
    // Bits 5:2: Bus Range (size = 2^range MB)
    // Bits 47:20: Base Address
    const MSR_AMD_MMIO_CONFIG_BASE: u32 = 0xC001_0058;

    unsafe {
        let val = rdmsr(MSR_AMD_MMIO_CONFIG_BASE);
        if val & 1 != 0 {
            // Extract bits 47:20 for the base address
            let base = val & 0x0000_FFFF_FFF0_0000;
            Some(base)
        } else {
            None
        }
    }
}

/// Helper to read a Model Specific Register (MSR).
unsafe fn rdmsr(msr: u32) -> u64 {
    let lo: u32;
    let hi: u32;
    core::arch::asm!(
        "rdmsr",
        in("ecx") msr,
        out("eax") lo,
        out("edx") hi,
        options(nomem, nostack, preserves_flags)
    );
    ((hi as u64) << 32) | (lo as u64)
}

/// Return the ECAM base for `(segment, bus)`. Currently only segment 0
/// is supported. Returns `None` when ECAM is unavailable.
pub fn get_ecam_base(segment: u16, _bus: u8) -> Option<u64> {
    if segment != 0 {
        return None;
    }
    MCFG_BASE0.get().copied()
}

pub fn run_boot_smoketest() {
    match MCFG_BASE0.get() {
        Some(b) => {
            crate::serial_println!("[pcie] smoketest: ECAM base {:#x} cached", b);
            // Verify pci::PCIE_ECAM_BASE is also set
            let pci_base = PCIE_ECAM_BASE.load(Ordering::Relaxed);
            if pci_base == *b {
                crate::serial_println!("[pcie] smoketest: pci::PCIE_ECAM_BASE matches");
            } else {
                crate::serial_println!(
                    "[pcie] [FAIL] pci::PCIE_ECAM_BASE ({:#x}) does not match cached base ({:#x})",
                    pci_base,
                    b
                );
            }
            // Boot-time live-fix #1: the ECAM scan bound must never be below a
            // bus we've already seen populated, or we'd silently drop devices.
            let bound = PCIE_ECAM_MAX_BUS.load(Ordering::Relaxed);
            let seen = crate::pci::highest_bus_seen();
            if bound >= seen {
                crate::serial_println!(
                    "[pcie] smoketest: scan bound {:#04x} >= highest seen bus {:#04x} (safe)",
                    bound,
                    seen
                );
            } else {
                crate::serial_println!(
                    "[pcie] [FAIL] scan bound {:#04x} below highest seen bus {:#04x} — would under-scan",
                    bound,
                    seen
                );
            }
        }
        None => crate::serial_println!("[pcie] smoketest: no ECAM (expected on legacy systems)"),
    }
}

/// Expose PCIe info via procfs.
pub fn dump_text() -> alloc::string::String {
    let mut out = alloc::string::String::new();
    out.push_str("# RaeenOS PCIe ECAM Status\n");
    if let Some(b) = MCFG_BASE0.get() {
        out.push_str(&alloc::format!("ecam_base: {:#x}\n", b));
        out.push_str("status: enabled\n");
        out.push_str(&alloc::format!(
            "scan_max_bus: {:#04x}\n",
            PCIE_ECAM_MAX_BUS.load(Ordering::Relaxed)
        ));
        out.push_str(&alloc::format!(
            "highest_bus_seen: {:#04x}\n",
            crate::pci::highest_bus_seen()
        ));
    } else {
        out.push_str("status: disabled (using legacy CF8/CFC)\n");
    }
    out
}
