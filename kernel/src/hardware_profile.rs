//! Hardware profile detection — Concept §M-A "Boots on Athena" enabler.
//!
//! Reads SMBIOS / DMI strings populated by `crate::smbios::init` and
//! dispatches a `HardwareProfile` describing the specific board + vendor
//! quirks the kernel needs to apply (ACPI workarounds, embedded-controller
//! address, fan-curve register set, expected NIC vendor, suspend path
//! variants, etc.).
//!
//! Today we ship explicit profiles for:
//!   * Beelink EliteMini "Athena" (target dev box: Ryzen 5 7640HS)
//!   * Generic AMD desktop
//!   * Generic Intel desktop
//!   * QEMU (the every-day dev profile)
//!   * Unknown (safe defaults)
//!
//! Adding a new profile = adding one row to `PROFILES` plus any quirk
//! overrides. Per `kernelchecklist.md` R3 every entry emits an `[hwprof]`
//! line at boot so the dev loop sees what was matched.

#![allow(dead_code)]

extern crate alloc;

use alloc::string::String;
use spin::Mutex;

use crate::firmware::{get_dmi, ChassisType};

/// Coarse hardware family — the kernel dispatches on this for
/// platform-wide behavior (e.g. AMD-specific MSR addresses).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HardwareFamily {
    /// QEMU TCG or KVM (the dev environment).
    QemuVirtual,
    /// AMD Zen 2/3/4 mini-PC or desktop.
    AmdDesktop,
    /// Intel Alder Lake / Raptor Lake / Meteor Lake.
    IntelDesktop,
    /// Laptop with discrete + integrated GPU + embedded controller.
    Laptop,
    /// Unknown — apply safe defaults.
    Unknown,
}

/// What the kernel needs to know to behave correctly on this hardware.
#[derive(Debug, Clone)]
pub struct HardwareProfile {
    /// Internal identifier for the profile (e.g. `"beelink-athena"`).
    pub id: &'static str,
    /// Human-readable description for logs and `/proc/raeen/hardware`.
    pub description: &'static str,
    pub family: HardwareFamily,
    /// Concept-doc "RaeReady" baseline match?
    pub is_concept_target: bool,
    /// Expected number of CPU cores (sanity check vs SMP enumeration).
    pub expected_cpus_min: u32,
    pub expected_cpus_max: u32,
    /// Expected RAM in MiB (informational, not enforced).
    pub expected_ram_min_mib: u32,
    /// Quirks bitmap — see `quirks` constants.
    pub quirks: u32,
}

// ── Quirk flags ────────────────────────────────────────────────────────

/// AMD systems sometimes report TSC_DEADLINE but it's unreliable on
/// pre-Zen3 silicon; use HPET periodic instead.
pub const QUIRK_AMD_TSC_DEADLINE_UNRELIABLE: u32 = 1 << 0;

/// Some boards omit the I/O APIC redirection entries for legacy IRQs;
/// fall back to identity mapping.
pub const QUIRK_IOAPIC_LEGACY_REDIRECT: u32 = 1 << 1;

/// Beelink, Minisforum, GMKtec mini-PCs use AMD SoC iGPU only; no
/// discrete GPU, no MUX switch logic needed.
pub const QUIRK_IGPU_ONLY: u32 = 1 << 2;

/// Laptops with hybrid AMD CPU + Intel WiFi need MSI-X allocator to
/// reserve the first 16 vectors for the Intel module.
pub const QUIRK_INTEL_WIFI_MSI_RESERVE: u32 = 1 << 3;

/// QEMU: enable extra debug helpers, expose virtio devices, skip ACPI
/// quirks that only exist on real silicon.
pub const QUIRK_QEMU_DEV_MODE: u32 = 1 << 4;

/// Skip USB 3.0 SuperSpeed transition; some buggy USB-C hubs require it.
pub const QUIRK_USB3_SLOW_RESET: u32 = 1 << 5;

/// AMD Zen 4 SMCA banks need explicit ACPI _OSC handshake.
pub const QUIRK_AMD_ZEN4_SMCA: u32 = 1 << 6;

/// NVMe controllers behind certain platforms need 100ms longer reset.
pub const QUIRK_NVME_SLOW_RESET: u32 = 1 << 7;

// ── Static profile table ───────────────────────────────────────────────

static PROFILES: &[HardwareProfile] = &[
    // -- Concept-doc primary dev target ------------------------------
    HardwareProfile {
        id: "beelink-athena",
        description: "Beelink EliteMini Athena — AMD Ryzen 5 7640HS (Zen 4) + Radeon 760M",
        family: HardwareFamily::AmdDesktop,
        is_concept_target: true,
        expected_cpus_min: 12,
        expected_cpus_max: 12,
        expected_ram_min_mib: 16 * 1024,
        quirks: QUIRK_IGPU_ONLY | QUIRK_AMD_ZEN4_SMCA,
    },
    HardwareProfile {
        id: "minisforum-um790",
        description: "Minisforum UM790 Pro / UM780 — AMD Ryzen 9 7940HS",
        family: HardwareFamily::AmdDesktop,
        is_concept_target: true,
        expected_cpus_min: 16,
        expected_cpus_max: 16,
        expected_ram_min_mib: 16 * 1024,
        quirks: QUIRK_IGPU_ONLY | QUIRK_AMD_ZEN4_SMCA,
    },
    HardwareProfile {
        id: "framework-13-amd",
        description: "Framework Laptop 13 — AMD Ryzen 7040 / Phoenix",
        family: HardwareFamily::Laptop,
        is_concept_target: true,
        expected_cpus_min: 12,
        expected_cpus_max: 16,
        expected_ram_min_mib: 8 * 1024,
        quirks: QUIRK_IGPU_ONLY | QUIRK_AMD_ZEN4_SMCA | QUIRK_INTEL_WIFI_MSI_RESERVE,
    },
    // -- Common Intel reference ---------------------------------------
    HardwareProfile {
        id: "intel-12th-13th-gen-desktop",
        description: "Intel Alder/Raptor Lake desktop",
        family: HardwareFamily::IntelDesktop,
        is_concept_target: false,
        expected_cpus_min: 6,
        expected_cpus_max: 32,
        expected_ram_min_mib: 8 * 1024,
        quirks: QUIRK_IOAPIC_LEGACY_REDIRECT,
    },
    // -- Generic AMD desktop (no specific quirks) ---------------------
    HardwareProfile {
        id: "amd-generic-desktop",
        description: "Generic AMD desktop (no Athena-specific quirks)",
        family: HardwareFamily::AmdDesktop,
        is_concept_target: false,
        expected_cpus_min: 4,
        expected_cpus_max: 32,
        expected_ram_min_mib: 4 * 1024,
        quirks: 0,
    },
    // -- QEMU dev profile ---------------------------------------------
    HardwareProfile {
        id: "qemu",
        description: "QEMU TCG / KVM virtual machine",
        family: HardwareFamily::QemuVirtual,
        is_concept_target: false,
        expected_cpus_min: 1,
        expected_cpus_max: 256,
        expected_ram_min_mib: 64,
        quirks: QUIRK_QEMU_DEV_MODE,
    },
    // -- Fallback -----------------------------------------------------
    HardwareProfile {
        id: "unknown",
        description: "Unknown hardware — applying safe defaults",
        family: HardwareFamily::Unknown,
        is_concept_target: false,
        expected_cpus_min: 1,
        expected_cpus_max: 256,
        expected_ram_min_mib: 64,
        quirks: 0,
    },
];

// ── Matching logic ─────────────────────────────────────────────────────

fn matches_athena(manufacturer: &str, product: &str) -> bool {
    let m = manufacturer.to_ascii_lowercase();
    let p = product.to_ascii_lowercase();
    m.contains("beelink") && (p.contains("athena") || p.contains("elitemini"))
}

fn matches_minisforum(manufacturer: &str, product: &str) -> bool {
    let m = manufacturer.to_ascii_lowercase();
    let p = product.to_ascii_lowercase();
    m.contains("minisforum") && (p.contains("um7") || p.contains("um790") || p.contains("um780"))
}

fn matches_framework_amd(manufacturer: &str, product: &str, bios: &str) -> bool {
    let m = manufacturer.to_ascii_lowercase();
    let p = product.to_ascii_lowercase();
    let b = bios.to_ascii_lowercase();
    m.contains("framework") && p.contains("13") && b.contains("amd")
}

fn matches_qemu(manufacturer: &str, product: &str, bios_vendor: &str) -> bool {
    let m = manufacturer.to_ascii_lowercase();
    let p = product.to_ascii_lowercase();
    let bv = bios_vendor.to_ascii_lowercase();
    m.contains("qemu")
        || p.contains("qemu")
        || p.contains("standard pc")
        || bv.contains("seabios")
        || bv.contains("bochs")
}

/// True when the DMI block is still the RaeenOS placeholder — the SMBIOS
/// scanner couldn't read real tables. Happens on BOTH QEMU-UEFI stages and
/// real UEFI boards: the legacy 0xF0000 scan misses tables that live in the
/// EFI config table. An unread DMI therefore proves nothing about being
/// virtual — the CPU check below must get priority (Athena was photographed
/// matching "profile = qemu" through this exact hole).
fn dmi_unread(manufacturer: &str) -> bool {
    manufacturer
        .to_ascii_lowercase()
        .contains("raeenos virtual")
}

/// Run the match logic. Returns the best-fit `HardwareProfile`.
pub fn detect() -> HardwareProfile {
    let dmi = get_dmi();
    let m = dmi.system_manufacturer.as_str();
    let p = dmi.product_name.as_str();
    let bv = dmi.bios_vendor.as_str();

    if matches_athena(m, p) {
        return PROFILES[0].clone();
    }
    if matches_minisforum(m, p) {
        return PROFILES[1].clone();
    }
    if matches_framework_amd(m, p, &dmi.bios_version) {
        return PROFILES[2].clone();
    }
    // The placeholder DMI is "AthenaOS Virtual Machine" / "AthenaOS QEMU" —
    // its PRODUCT string contains "qemu", so an ungated matches_qemu()
    // claims every machine whose SMBIOS was unreadable (photographed on
    // Athena: "profile = qemu" persisted through the first reorder fix
    // because this match fired before the CPU check was ever reached).
    // Only trust a QEMU match derived from REAL DMI strings.
    let unread = dmi_unread(m);
    if !unread && matches_qemu(m, p, bv) {
        return PROFILES[5].clone();
    }

    // Fall back by family hint from CPU vendor (if cpu_features detected one).
    // This MUST run before the unread-DMI -> QEMU assumption: a Zen 4 CPU is
    // real hardware regardless of whether the SMBIOS tables were readable.
    let cpu_vendor = crate::cpu_features::is_athena_profile();
    if cpu_vendor {
        // Detected Zen 4 family but not a known board — use generic AMD desktop.
        return PROFILES[4].clone();
    }

    // DMI unread + no real-hardware CPU hint: keep the historical QEMU
    // assumption (QEMU-UEFI stages hit this; the qemu profile also keeps
    // the GOP-scanout path that QEMU's display window needs).
    if unread {
        return PROFILES[5].clone();
    }

    PROFILES[6].clone()
}

// ── Cached active profile ──────────────────────────────────────────────

static ACTIVE: Mutex<Option<HardwareProfile>> = Mutex::new(None);

pub fn init() {
    let profile = detect();
    let id = profile.id;
    let desc = profile.description;
    let quirks = profile.quirks;
    let target = profile.is_concept_target;
    *ACTIVE.lock() = Some(profile);
    // Tier-0 boot artifact (MasterChecklist §1.8): the Athena acceptance grep
    // looks for this exact line. The richer [hwprof] line below is for humans.
    crate::serial_println!("[smbios] hardware profile = {}", id);
    crate::serial_println!("[hwprof] matched profile: {} ({})", id, desc,);
    if target {
        crate::serial_println!("[hwprof] [OK] this is a Concept-doc RaeReady target board");
    }
    if quirks != 0 {
        crate::serial_println!("[hwprof] applying quirks: 0x{:08x}", quirks);
        decode_quirks(quirks);
    }
}

fn decode_quirks(q: u32) {
    let table: &[(u32, &str)] = &[
        (
            QUIRK_AMD_TSC_DEADLINE_UNRELIABLE,
            "AMD TSC-deadline timer unreliable (use HPET)",
        ),
        (
            QUIRK_IOAPIC_LEGACY_REDIRECT,
            "IOAPIC legacy redirect fallback",
        ),
        (QUIRK_IGPU_ONLY, "iGPU only (no discrete-GPU MUX)"),
        (
            QUIRK_INTEL_WIFI_MSI_RESERVE,
            "Reserve MSI-X vectors 0-15 for Intel Wi-Fi",
        ),
        (
            QUIRK_QEMU_DEV_MODE,
            "QEMU dev mode: skip real-silicon-only paths",
        ),
        (QUIRK_USB3_SLOW_RESET, "USB3 slow-reset workaround"),
        (QUIRK_AMD_ZEN4_SMCA, "AMD Zen 4 SMCA _OSC handshake"),
        (QUIRK_NVME_SLOW_RESET, "NVMe extended reset timeout"),
    ];
    for (mask, name) in table {
        if q & mask != 0 {
            crate::serial_println!("[hwprof]   - {}", name);
        }
    }
}

/// Public accessor — what board are we on?
pub fn active() -> Option<HardwareProfile> {
    ACTIVE.lock().clone()
}

pub fn has_quirk(mask: u32) -> bool {
    ACTIVE
        .lock()
        .as_ref()
        .map(|p| p.quirks & mask != 0)
        .unwrap_or(false)
}

/// Golden quirk expectations — MasterChecklist §1.7 "per-quirk regression
/// tests so removing one is detectable". Each `(id, quirks)` here is the
/// authoritative quirk set that MUST hold for that profile. If someone edits
/// `PROFILES` and drops (or adds) a quirk, [`run_quirk_regression`] catches
/// the divergence at boot instead of silently shipping a board with the wrong
/// platform workarounds. Keep this in lockstep with `PROFILES` deliberately:
/// a real quirk change updates BOTH, an accidental one trips the smoketest.
const QUIRK_GOLDEN: &[(&str, u32)] = &[
    ("beelink-athena", QUIRK_IGPU_ONLY | QUIRK_AMD_ZEN4_SMCA),
    ("minisforum-um790", QUIRK_IGPU_ONLY | QUIRK_AMD_ZEN4_SMCA),
    (
        "framework-13-amd",
        QUIRK_IGPU_ONLY | QUIRK_AMD_ZEN4_SMCA | QUIRK_INTEL_WIFI_MSI_RESERVE,
    ),
    ("intel-12th-13th-gen-desktop", QUIRK_IOAPIC_LEGACY_REDIRECT),
    ("amd-generic-desktop", 0),
    ("qemu", QUIRK_QEMU_DEV_MODE),
    ("unknown", 0),
];

/// Every quirk bit that `decode_quirks` knows how to name. A new `QUIRK_*`
/// constant used in `PROFILES` but missing here would log as an unnamed bit;
/// the regression test asserts the union of all profile quirks is fully
/// covered so a quirk can't ship without a human-readable decode line.
const QUIRK_DECODE_COVERAGE: u32 = QUIRK_AMD_TSC_DEADLINE_UNRELIABLE
    | QUIRK_IOAPIC_LEGACY_REDIRECT
    | QUIRK_IGPU_ONLY
    | QUIRK_INTEL_WIFI_MSI_RESERVE
    | QUIRK_QEMU_DEV_MODE
    | QUIRK_USB3_SLOW_RESET
    | QUIRK_AMD_ZEN4_SMCA
    | QUIRK_NVME_SLOW_RESET;

/// Assert every `PROFILES` entry still carries exactly its golden quirk set,
/// every golden id exists in `PROFILES` (and vice-versa), and every quirk bit
/// any profile uses is decodable. Returns true on PASS. Pure data check — no
/// hardware, fully QEMU-provable.
fn run_quirk_regression() -> bool {
    let mut ok = true;

    // 1. Each shipped profile matches its golden quirk set.
    for p in PROFILES {
        match QUIRK_GOLDEN.iter().find(|(id, _)| *id == p.id) {
            Some((_, expected)) if *expected == p.quirks => {}
            Some((_, expected)) => {
                ok = false;
                crate::serial_println!(
                    "[hwprof] quirk-regression FAIL: {} quirks=0x{:08x} expected=0x{:08x} (added 0x{:08x}, removed 0x{:08x})",
                    p.id,
                    p.quirks,
                    expected,
                    p.quirks & !expected,
                    expected & !p.quirks,
                );
            }
            None => {
                ok = false;
                crate::serial_println!(
                    "[hwprof] quirk-regression FAIL: profile {} has no golden entry (add it to QUIRK_GOLDEN)",
                    p.id,
                );
            }
        }
    }

    // 2. No golden id was deleted from PROFILES.
    for (id, _) in QUIRK_GOLDEN {
        if !PROFILES.iter().any(|p| p.id == *id) {
            ok = false;
            crate::serial_println!(
                "[hwprof] quirk-regression FAIL: golden profile {} missing from PROFILES",
                id,
            );
        }
    }

    // 3. Every quirk bit any profile uses has a decode entry.
    let used: u32 = PROFILES.iter().fold(0, |acc, p| acc | p.quirks);
    let undecoded = used & !QUIRK_DECODE_COVERAGE;
    if undecoded != 0 {
        ok = false;
        crate::serial_println!(
            "[hwprof] quirk-regression FAIL: quirk bit(s) 0x{:08x} used by a profile but not in decode_quirks",
            undecoded,
        );
    }

    crate::serial_println!(
        "[hwprof] quirk-regression: {} profile(s), used_mask=0x{:08x} -> {}",
        PROFILES.len(),
        used,
        if ok { "PASS" } else { "FAIL" },
    );
    ok
}

pub fn run_boot_smoketest() {
    // §1.7: per-quirk regression — catch an accidental quirk add/removal.
    run_quirk_regression();

    // Sanity: number of CPUs SMP brought up matches the profile's expected range.
    let active = match active() {
        Some(a) => a,
        None => return,
    };
    // Real online-CPU count from SMP bring-up (was a hardcoded 4).
    let actual_cpus = crate::smp::ONLINE_CPUS
        .load(core::sync::atomic::Ordering::Relaxed)
        .max(1);
    if actual_cpus < active.expected_cpus_min || actual_cpus > active.expected_cpus_max {
        crate::serial_println!(
            "[hwprof] [WARN] CPU count {} outside expected {}..={} for profile {}",
            actual_cpus,
            active.expected_cpus_min,
            active.expected_cpus_max,
            active.id,
        );
    } else {
        crate::serial_println!(
            "[hwprof] smoketest: {} CPUs online matches profile {} expected range",
            actual_cpus,
            active.id,
        );
    }
}

// ── /proc/raeen/hardware ───────────────────────────────────────────────

pub fn dump_text() -> String {
    let dmi = get_dmi();
    let mut out = String::new();
    out.push_str("# RaeenOS hardware profile + DMI snapshot\n");
    out.push_str(&alloc::format!(
        "system_manufacturer: {}\n",
        dmi.system_manufacturer
    ));
    out.push_str(&alloc::format!(
        "product_name:        {}\n",
        dmi.product_name
    ));
    out.push_str(&alloc::format!(
        "serial_number:       {}\n",
        dmi.serial_number
    ));
    out.push_str(&alloc::format!(
        "chassis_type:        {:?}\n",
        dmi.chassis_type
    ));
    out.push_str(&alloc::format!(
        "bios_vendor:         {}\n",
        dmi.bios_vendor
    ));
    out.push_str(&alloc::format!(
        "bios_version:        {}\n",
        dmi.bios_version
    ));
    out.push_str(&alloc::format!("bios_date:           {}\n", dmi.bios_date));
    out.push_str(&alloc::format!("board_name:          {}\n", dmi.board_name));
    out.push_str("\n# Matched profile\n");
    match active() {
        Some(p) => {
            out.push_str(&alloc::format!("id:                  {}\n", p.id));
            out.push_str(&alloc::format!("description:         {}\n", p.description));
            out.push_str(&alloc::format!("family:              {:?}\n", p.family));
            out.push_str(&alloc::format!(
                "is_concept_target:   {}\n",
                p.is_concept_target
            ));
            out.push_str(&alloc::format!(
                "expected_cpus:       {}..={}\n",
                p.expected_cpus_min,
                p.expected_cpus_max
            ));
            out.push_str(&alloc::format!(
                "expected_ram_min:    {} MiB\n",
                p.expected_ram_min_mib
            ));
            out.push_str(&alloc::format!("quirks:              0x{:08x}\n", p.quirks));
        }
        None => out.push_str("(profile not initialized)\n"),
    }
    out
}
