//! AMD-specific model-specific registers — MasterChecklist Phase 1.3.
//!
//! Concept §"Hardware: First-class AMD support (Beelink Athena = Ryzen Zen 4)"
//! requires the kernel to *know* the AMD platform knobs it sits on top of:
//!
//!   * **CPPC** (Collaborative Processor Performance Control) — the modern
//!     hardware-managed P-state interface that replaces the legacy ACPI
//!     `_PSS` table on Zen 2+. We detect support and read the firmware's
//!     highest / nominal / lowest performance levels so `cpufreq` can later
//!     drive autonomous frequency selection instead of fixed P-states.
//!   * **SVM** (Secure Virtual Machine = AMD-V) — the AMD virtualization
//!     extension. RaeenOS is *not* a hypervisor, so we deliberately do **not**
//!     set `EFER.SVME`; we only detect and log its presence (and current
//!     EFER state) so RaeBridge / future VM tooling knows what the silicon
//!     can do.
//!   * **SMCA** (Scalable Machine Check Architecture) — Zen-era MCA with a
//!     wider, banked error-reporting layout. We detect support and log the
//!     legacy `IA32_MCG_CAP` bank count so the MCE handler knows how many
//!     banks to walk.
//!
//! All probing here runs once on the bootstrap processor during early init,
//! before any userspace task exists. There is therefore no `CapHandle` to
//! gate against — direct `RDMSR`/`CPUID` here is the same kernel-internal
//! boot-probe path used by `cpu_features` and `cpufreq`. The privileged
//! surface that userspace *can* reach (P-state writes) lives behind
//! `crate::capability::Cap::System` in the `cpufreq` driver, not here; this
//! module is read-only telemetry.
//!
//! Output: `[msr-amd] …` lines at boot + `/proc/raeen/msr_amd`.

#![allow(dead_code)]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use spin::Mutex;

use crate::cpu_features::cpuid_raw;

// ── MSR / CPUID constants ───────────────────────────────────────────────

/// CPPC enable MSR (AMD PPR: `MSR_AMD_CPPC_ENABLE`).
const MSR_AMD_CPPC_ENABLE: u32 = 0xC001_102B;
/// CPPC capability/request MSR (`MSR_AMD_CPPC_CAP1`) — holds the packed
/// highest/nominal/lowest-nonlinear/lowest performance levels.
const MSR_AMD_CPPC_CAP1: u32 = 0xC001_1029; // Wraps PPR CppcCapability1
/// CPPC request register (`MSR_AMD_CPPC_REQ`).
const MSR_AMD_CPPC_REQ: u32 = 0xC001_102C;

/// Extended Feature Enable Register.
const MSR_EFER: u32 = 0xC000_0080;
/// EFER.SVME — Secure Virtual Machine Enable (bit 12).
const EFER_SVME: u64 = 1 << 12;

/// First SMCA per-bank status MSR (`MCA_STATUS` for bank 0 in the SMCA
/// register space). Banks are strided by 0x10.
const MSR_SMCA_MC0_STATUS: u32 = 0xC000_0408;
/// SMCA per-bank MSR stride.
const SMCA_BANK_STRIDE: u32 = 0x10;

/// Legacy MCA global capability register; bits[7:0] = bank count.
const MSR_IA32_MCG_CAP: u32 = 0x0179;

/// AMD extended feature leaf carrying CPPC + SMCA capability bits.
const CPUID_AMD_EXT_FEATURES: u32 = 0x8000_0021;
/// `CPUID 0x80000021 EAX[0]` = CPPC supported.
const CPPC_EAX_BIT: u32 = 0;
/// `CPUID 0x80000021 EAX[3]` = SMCA supported.
const SMCA_EAX_BIT: u32 = 3;

/// AMD extended feature leaf 1.
const CPUID_EXT_FEATURE_1: u32 = 0x8000_0001;
/// `CPUID 0x80000001 ECX[2]` = SVM (AMD-V).
const SVM_ECX_BIT: u32 = 2;

// ── Raw MSR access ──────────────────────────────────────────────────────

/// Read a 64-bit MSR. Mirrors the `rdmsr` idiom in `apic.rs`.
fn rdmsr(msr: u32) -> u64 {
    let lo: u32;
    let hi: u32;
    // SAFETY: `rdmsr` is a privileged read of the named MSR. We only ever
    // call this on the BSP during early boot from `init()` after CPUID has
    // confirmed `Vendor::Amd` and the relevant feature bit, so every MSR
    // number passed here is architecturally valid on this CPU. No memory is
    // touched and flags are preserved.
    unsafe {
        core::arch::asm!(
            "rdmsr",
            in("ecx") msr,
            out("eax") lo,
            out("edx") hi,
            options(nomem, nostack, preserves_flags),
        );
    }
    ((hi as u64) << 32) | (lo as u64)
}

// ── Detected state snapshot ─────────────────────────────────────────────

#[derive(Debug, Clone, Copy, Default)]
pub struct CppcInfo {
    pub supported: bool,
    pub enabled: bool,
    pub highest: u8,
    pub nominal: u8,
    pub lowest_nonlinear: u8,
    pub lowest: u8,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct SvmInfo {
    pub supported: bool,
    /// `EFER.SVME` — true only if firmware/another agent enabled it. We
    /// never set it ourselves.
    pub enabled: bool,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct SmcaInfo {
    pub supported: bool,
    /// MCA bank count from `IA32_MCG_CAP[7:0]`.
    pub bank_count: u8,
    /// Snapshot of SMCA bank-0 status at boot (0 = no logged error).
    pub mc0_status: u64,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct AmdMsrInfo {
    pub is_amd: bool,
    pub cppc: CppcInfo,
    pub svm: SvmInfo,
    pub smca: SmcaInfo,
}

static INFO: Mutex<Option<AmdMsrInfo>> = Mutex::new(None);

// ── Probing ─────────────────────────────────────────────────────────────

fn vendor_is_amd() -> bool {
    let r = cpuid_raw(0, 0);
    // "AuthenticAMD": EBX="Auth", EDX="enti", ECX="cAMD".
    r.ebx == 0x6874_7541 && r.edx == 0x6974_6E65 && r.ecx == 0x444D_4163
}

fn ext_leaf_supported(leaf: u32) -> bool {
    cpuid_raw(0x8000_0000, 0).eax >= leaf
}

fn probe_cppc() -> CppcInfo {
    let mut c = CppcInfo::default();
    if !ext_leaf_supported(CPUID_AMD_EXT_FEATURES) {
        return c;
    }
    let eax = cpuid_raw(CPUID_AMD_EXT_FEATURES, 0).eax;
    c.supported = (eax & (1 << CPPC_EAX_BIT)) != 0;
    if !c.supported {
        return c;
    }
    // CppcEnable[0] = 1 once CPPC is turned on.
    c.enabled = (rdmsr(MSR_AMD_CPPC_ENABLE) & 1) != 0;

    // CppcCapability1 packs the four perf levels, one per byte:
    //   [7:0]   lowest performance
    //   [15:8]  lowest-nonlinear performance
    //   [23:16] nominal performance
    //   [31:24] highest performance
    let cap = rdmsr(MSR_AMD_CPPC_CAP1);
    c.lowest = (cap & 0xFF) as u8;
    c.lowest_nonlinear = ((cap >> 8) & 0xFF) as u8;
    c.nominal = ((cap >> 16) & 0xFF) as u8;
    c.highest = ((cap >> 24) & 0xFF) as u8;
    c
}

fn probe_svm() -> SvmInfo {
    let mut s = SvmInfo::default();
    if !ext_leaf_supported(CPUID_EXT_FEATURE_1) {
        return s;
    }
    let ecx = cpuid_raw(CPUID_EXT_FEATURE_1, 0).ecx;
    s.supported = (ecx & (1 << SVM_ECX_BIT)) != 0;
    // Read-only: report whether EFER.SVME happens to be set. We never write it.
    s.enabled = (rdmsr(MSR_EFER) & EFER_SVME) != 0;
    s
}

fn probe_smca() -> SmcaInfo {
    let mut m = SmcaInfo::default();
    // Bank count comes from the legacy MCG capability register, always present
    // on any CPU that reports the MCA feature (true for all Zen parts).
    m.bank_count = (rdmsr(MSR_IA32_MCG_CAP) & 0xFF) as u8;

    if !ext_leaf_supported(CPUID_AMD_EXT_FEATURES) {
        return m;
    }
    let eax = cpuid_raw(CPUID_AMD_EXT_FEATURES, 0).eax;
    m.supported = (eax & (1 << SMCA_EAX_BIT)) != 0;
    if m.supported {
        // Snapshot bank-0 status; non-zero would mean a previously logged
        // machine-check error. Reading is side-effect-free.
        m.mc0_status = rdmsr(MSR_SMCA_MC0_STATUS);
    }
    m
}

// ── R10 contract ────────────────────────────────────────────────────────

pub fn init() {
    let mut info = AmdMsrInfo::default();
    info.is_amd = vendor_is_amd();

    if !info.is_amd {
        crate::serial_println!("[msr-amd] non-AMD CPU — skipping AMD MSR probe");
        *INFO.lock() = Some(info);
        return;
    }

    info.cppc = probe_cppc();
    info.svm = probe_svm();
    info.smca = probe_smca();

    if info.cppc.supported {
        crate::serial_println!(
            "[msr-amd] CPPC: supported enabled={} highest={} nominal={} lowest_nonlinear={} lowest={}",
            info.cppc.enabled,
            info.cppc.highest,
            info.cppc.nominal,
            info.cppc.lowest_nonlinear,
            info.cppc.lowest,
        );
    } else {
        crate::serial_println!("[msr-amd] CPPC: not supported (legacy ACPI _PSS P-states)");
    }

    if info.svm.supported {
        crate::serial_println!(
            "[msr-amd] SVM (AMD-V): supported EFER.SVME={} (not enabled by RaeenOS — we are not a hypervisor)",
            info.svm.enabled,
        );
    } else {
        crate::serial_println!("[msr-amd] SVM (AMD-V): not supported");
    }

    if info.smca.supported {
        crate::serial_println!(
            "[msr-amd] SMCA: supported banks={} mc0_status={:#x}",
            info.smca.bank_count,
            info.smca.mc0_status,
        );
    } else {
        crate::serial_println!(
            "[msr-amd] SMCA: not supported (legacy MCA, banks={})",
            info.smca.bank_count,
        );
    }

    *INFO.lock() = Some(info);
    crate::serial_println!("[ OK ] AMD MSR detection complete");
}

pub fn run_boot_smoketest() {
    let g = INFO.lock();
    let info = match g.as_ref() {
        Some(i) => *i,
        None => {
            crate::serial_println!("[msr-amd] smoketest -> SKIP (init not run)");
            return;
        }
    };

    if !info.is_amd {
        // Nothing AMD-specific to validate on Intel/other; the probe correctly
        // no-op'd, which is the pass condition.
        crate::serial_println!(
            "[msr-amd] smoketest -> PASS (non-AMD, AMD MSR probe correctly skipped)"
        );
        return;
    }

    // On AMD, MCG_CAP must report a non-zero bank count or our MCE handler
    // would have nothing to walk — that's our hard invariant. CPPC/SVM are
    // capability-dependent and only logged, so they can't fail the smoketest.
    if info.smca.bank_count == 0 {
        crate::serial_println!(
            "[msr-amd] smoketest -> FAIL (IA32_MCG_CAP reported 0 MCA banks on AMD CPU)"
        );
        return;
    }

    crate::serial_println!(
        "[msr-amd] amd MSR smoketest -> PASS (cppc={} svm={} smca={} banks={})",
        info.cppc.supported,
        info.svm.supported,
        info.smca.supported,
        info.smca.bank_count,
    );
}

/// Snapshot accessor for other subsystems (e.g. `cpufreq` reading CPPC perf
/// levels) without re-probing CPUID/MSRs.
pub fn get_info() -> AmdMsrInfo {
    INFO.lock().unwrap_or_default()
}

// ── /proc/raeen/msr_amd ─────────────────────────────────────────────────

pub fn dump_text() -> String {
    let g = INFO.lock();
    let info = match g.as_ref() {
        Some(i) => *i,
        None => return String::from("# msr_amd not initialized\n"),
    };

    let mut out = String::new();
    out.push_str("# RaeenOS AMD MSR detection\n");
    out.push_str(&format!("is_amd: {}\n", info.is_amd));
    if !info.is_amd {
        out.push_str("(AMD MSR probing skipped on non-AMD CPU)\n");
        return out;
    }

    out.push_str("\n## CPPC (Collaborative Processor Performance Control)\n");
    out.push_str(&format!("supported:        {}\n", info.cppc.supported));
    if info.cppc.supported {
        out.push_str(&format!("enabled:          {}\n", info.cppc.enabled));
        out.push_str(&format!("highest_perf:     {}\n", info.cppc.highest));
        out.push_str(&format!("nominal_perf:     {}\n", info.cppc.nominal));
        out.push_str(&format!(
            "lowest_nonlinear: {}\n",
            info.cppc.lowest_nonlinear
        ));
        out.push_str(&format!("lowest_perf:      {}\n", info.cppc.lowest));
    }

    out.push_str("\n## SVM (AMD-V virtualization)\n");
    out.push_str(&format!("supported:        {}\n", info.svm.supported));
    out.push_str(&format!("efer_svme:        {}\n", info.svm.enabled));
    out.push_str("note:             not enabled by RaeenOS (we are not a hypervisor)\n");

    out.push_str("\n## SMCA (Scalable Machine Check Architecture)\n");
    out.push_str(&format!("supported:        {}\n", info.smca.supported));
    out.push_str(&format!("mca_bank_count:   {}\n", info.smca.bank_count));
    out.push_str(&format!("mc0_status:       {:#x}\n", info.smca.mc0_status));

    out
}
