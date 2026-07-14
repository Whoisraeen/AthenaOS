//! Fault-tolerant Model-Specific Register access + CPU vendor detection — the
//! foundation for booting on any motherboard.
//!
//! `rdmsr`/`wrmsr` against an MSR the CPU does not implement raises `#GP`. The
//! available MSR set differs wildly across vendors and generations, so any
//! hardcoded MSR access is a latent `#GP` that crashes boot on the "wrong" CPU
//! (observed on first real hardware: an Intel `IA32_THERM_STATUS` read on an AMD
//! box took down the whole boot). Two complementary defenses live here:
//!
//!   1. [`is_intel`] / [`is_amd`] / [`cpu_vendor`] — gate vendor-specific MSRs so
//!      they are never even attempted on the wrong CPU (fully testable).
//!   2. [`rdmsr_safe`] / [`wrmsr_safe`] — a trap during the access is *recovered*
//!      by the `#GP` handler (it skips the 2-byte instruction and the wrapper
//!      returns `None`/`false`), covering MSRs we don't have a vendor rule for.
//!
//! ## Recovery mechanism
//!
//! Before the `rdmsr`/`wrmsr`, the wrapper sets [`MSR_ARMED`]. If the instruction
//! faults, the fault handler calls [`gp_recover_armed`], records the fault, and
//! advances the saved RIP by 2 (the fixed length of `rdmsr`/`wrmsr`) so `iretq`
//! resumes *after* it. No label addresses, no fixup pointer — just "skip the
//! instruction", which is robust and simple. `MSR_ARMED` is only set across the
//! single faulting instruction, so any trap in that window is the MSR op.
//!
//! Boot-time probing runs single-threaded on the BSP. Per-CPU arming +
//! interrupt-safety for heavy runtime multi-core MSR polling is a follow-up
//! (MasterChecklist: bare-metal hardening).

extern crate alloc;

use core::sync::atomic::{AtomicBool, Ordering};

/// Bring fault-tolerant MSR access online (logs the detected vendor). The
/// recovery itself works as soon as the #GP handler is installed; this is the
/// R10 init hook and the place that announces vendor-gating is active.
pub fn init() {
    crate::serial_println!(
        "[msr] fault-tolerant MSR access online (vendor={:?})",
        cpu_vendor()
    );
}

/// `/proc/raeen/msr` — CPU vendor + fault-tolerant MSR access status.
pub fn dump_text() -> alloc::string::String {
    let v = cpu_vendor();
    let tsc = unsafe { rdmsr_safe(0x10) }.is_some();
    alloc::format!(
        "# AthenaOS MSR access\nvendor: {:?}\nis_intel: {}\nis_amd: {}\nfault_tolerant: yes (rdmsr_safe/wrmsr_safe)\ntsc_readable: {}\n",
        v,
        is_intel(),
        is_amd(),
        tsc,
    )
}

/// Set across a fault-tolerant `rdmsr`/`wrmsr`. While true, a `#GP`/`#PF` is
/// expected and the faulting instruction should be skipped.
static MSR_ARMED: AtomicBool = AtomicBool::new(false);
/// Records that the most recent armed access faulted.
static MSR_FAULTED: AtomicBool = AtomicBool::new(false);

/// Called by the fault handler. If a fault-tolerant MSR access is in flight,
/// records the fault and returns `true` (handler skips the 2-byte instruction);
/// otherwise `false` (a real fault → the handler panics as usual).
#[inline]
pub fn gp_recover_armed() -> bool {
    if MSR_ARMED.load(Ordering::SeqCst) {
        MSR_FAULTED.store(true, Ordering::SeqCst);
        true
    } else {
        false
    }
}

/// Read an MSR, returning `None` if it `#GP`s (unimplemented on this CPU).
///
/// # Safety
/// `rdmsr` is privileged; call only in ring 0.
#[inline]
pub unsafe fn rdmsr_safe(msr: u32) -> Option<u64> {
    MSR_FAULTED.store(false, Ordering::SeqCst);
    MSR_ARMED.store(true, Ordering::SeqCst);
    let lo: u32;
    let hi: u32;
    core::arch::asm!(
        "rdmsr",
        in("ecx") msr,
        out("eax") lo,
        out("edx") hi,
        options(nostack, preserves_flags),
    );
    MSR_ARMED.store(false, Ordering::SeqCst);
    if MSR_FAULTED.load(Ordering::SeqCst) {
        None
    } else {
        Some(((hi as u64) << 32) | (lo as u64))
    }
}

/// Write an MSR, returning `false` if it `#GP`s (unimplemented/read-only).
///
/// # Safety
/// `wrmsr` is privileged and can change CPU state; call only in ring 0 with a
/// value valid for `msr`.
#[inline]
pub unsafe fn wrmsr_safe(msr: u32, val: u64) -> bool {
    let lo = val as u32;
    let hi = (val >> 32) as u32;
    MSR_FAULTED.store(false, Ordering::SeqCst);
    MSR_ARMED.store(true, Ordering::SeqCst);
    core::arch::asm!(
        "wrmsr",
        in("ecx") msr,
        in("eax") lo,
        in("edx") hi,
        options(nostack, preserves_flags),
    );
    MSR_ARMED.store(false, Ordering::SeqCst);
    !MSR_FAULTED.load(Ordering::SeqCst)
}

// ─── CPU vendor ──────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CpuVendor {
    Intel,
    Amd,
    Other,
}

/// Identify the CPU vendor from CPUID leaf 0 (`EBX:EDX:ECX` = vendor string).
pub fn cpu_vendor() -> CpuVendor {
    let r = core::arch::x86_64::__cpuid(0);
    // "GenuineIntel" = EBX 0x756e6547, EDX 0x49656e69, ECX 0x6c65746e
    // "AuthenticAMD" = EBX 0x68747541, EDX 0x69746e65, ECX 0x444d4163
    match (r.ebx, r.ecx) {
        (0x756e_6547, 0x6c65_746e) => CpuVendor::Intel,
        (0x6874_7541, 0x444d_4163) => CpuVendor::Amd,
        _ => CpuVendor::Other,
    }
}

#[inline]
pub fn is_intel() -> bool {
    cpu_vendor() == CpuVendor::Intel
}

#[inline]
pub fn is_amd() -> bool {
    cpu_vendor() == CpuVendor::Amd
}

/// Display family from CPUID leaf 1 EAX (base family + extended family when
/// base == 0xF, per the AMD/Intel encoding). AMD Zen = 0x17 (Zen1/2), 0x19
/// (Zen3/4), 0x1A (Zen5); QEMU's default CPU reports 0xF. Used to gate the
/// Zen-only SMU temperature read so it never pokes SMN on a non-Zen part.
pub fn cpu_family() -> u32 {
    let r = core::arch::x86_64::__cpuid(1);
    let base = (r.eax >> 8) & 0xF;
    if base == 0xF {
        base + ((r.eax >> 20) & 0xFF)
    } else {
        base
    }
}

/// Boot smoketest. Confirms vendor detection and that the fault-tolerant
/// wrappers do not crash the boot. Reading an absent MSR returns `None` on real
/// hardware (recovered) and may return `Some` on permissive emulators; the
/// load-bearing proof either way is that we keep running. A universally-present
/// MSR (`IA32_TSC`) must read back.
pub fn run_boot_smoketest() {
    const BOGUS_MSR: u32 = 0xFFFF_FFFF; // architecturally absent everywhere
    const IA32_TSC: u32 = 0x10; // present on every x86-64 CPU
    let vendor = cpu_vendor();
    let bogus = unsafe { rdmsr_safe(BOGUS_MSR) };
    let tsc = unsafe { rdmsr_safe(IA32_TSC) };
    let pass = tsc.is_some();
    crate::serial_println!(
        "[msr] run_boot_smoketest: vendor={:?} bogus_rd={:?} tsc_present={} -> {}",
        vendor,
        bogus,
        tsc.is_some(),
        if pass { "PASS" } else { "FAIL" }
    );
}
