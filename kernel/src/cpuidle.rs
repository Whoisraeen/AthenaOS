//! CPU C-state (cpuidle) support — Phase 2.4
//!
//! Detects which C-states the processor advertises via CPUID leaf 5
//! (MONITOR/MWAIT), and provides an `enter_idle` entry-point.
//!
//! Current policy:
//!   C1  — HLT (always available, no MWAIT needed)
//!   C3+ — MWAIT with the appropriate sub-state hint when leaf 5 is present
//!
//! On QEMU the MWAIT feature is typically not advertised; the code falls back
//! gracefully and logs what it found.

#![allow(dead_code)]

extern crate alloc;
use alloc::vec::Vec;

// ─── C-state Enumeration ────────────────────────────────────────────────────

/// Processor C-states as defined in the ACPI / Intel IA-32 architecture spec.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum CState {
    /// C0 — active (CPU executing instructions)
    C0,
    /// C1 — halt; entered with HLT or MWAIT hint 0x00
    C1,
    /// C1E — enhanced halt (auto-demotion variant)
    C1E,
    /// C3 — sleep; L2 flushed, bus agent must snoop
    C3,
    /// C6 — deep power-down; core state saved, voltage reduced
    C6,
    /// C7 — enhanced C6 with LLC-way flushing
    C7,
    /// C8 — deeper LLC flush (Haswell+)
    C8,
    /// C10 — all-core-off (Skylake+)
    C10,
}

impl CState {
    /// Human-readable name used in the serial log.
    pub fn name(self) -> &'static str {
        match self {
            CState::C0 => "C0",
            CState::C1 => "C1",
            CState::C1E => "C1E",
            CState::C3 => "C3",
            CState::C6 => "C6",
            CState::C7 => "C7",
            CState::C8 => "C8",
            CState::C10 => "C10",
        }
    }

    /// MWAIT hint value for this C-state (bits 7:4 = sub-state level,
    /// bits 3:0 = sub-state index within that level).
    ///
    /// These are the standard hints from the Intel SDM Vol.3 Table 4-12.
    fn mwait_hint(self) -> u32 {
        match self {
            CState::C0 => 0x00,
            CState::C1 => 0x00,
            CState::C1E => 0x01,
            CState::C3 => 0x10,
            CState::C6 => 0x20,
            CState::C7 => 0x30,
            CState::C8 => 0x40,
            CState::C10 => 0x60,
        }
    }
}

// ─── CPUID Leaf 5 — MONITOR/MWAIT ───────────────────────────────────────────

/// Result of the CPUID leaf 5 query (MONITOR/MWAIT).
#[derive(Debug, Clone, Copy, Default)]
pub struct MwaitInfo {
    /// MONITOR minimum line size in bytes (EAX[15:0]).
    pub monitor_min_line: u16,
    /// MONITOR maximum line size in bytes (EBX[15:0]).
    pub monitor_max_line: u16,
    /// MWAIT extensions are supported (ECX bit 0).
    pub extensions: bool,
    /// Interrupts break out of MWAIT even if EFLAGS.IF=0 (ECX bit 1).
    pub interrupt_break: bool,
    /// Number of C0 sub-states (EDX[3:0]).
    pub c0_substates: u8,
    /// Number of C1 sub-states (EDX[7:4]).
    pub c1_substates: u8,
    /// Number of C2 sub-states (EDX[11:8]).
    pub c2_substates: u8,
    /// Number of C3 sub-states (EDX[15:12]).
    pub c3_substates: u8,
    /// Number of C4 sub-states (EDX[19:16]).
    pub c4_substates: u8,
    /// Number of C5 sub-states (EDX[23:20]).
    pub c5_substates: u8,
    /// Number of C6 sub-states (EDX[27:24]).
    pub c6_substates: u8,
    /// Number of C7 sub-states (EDX[31:28]).
    pub c7_substates: u8,
}

/// Returns true when the processor advertises MONITOR/MWAIT (CPUID.01H:ECX[3]).
fn mwait_feature_present() -> bool {
    // LLVM uses rbx as a base pointer in position-independent code; we must
    // save and restore it around CPUID which clobbers rbx.
    let ecx: u32;
    unsafe {
        core::arch::asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 1u32 => _,
            out("ecx") ecx,
            out("edx") _,
            options(nomem, nostack),
        );
    }
    ecx & (1 << 3) != 0
}

/// Query CPUID leaf 5 and return detailed MONITOR/MWAIT information.
///
/// Returns `None` when MONITOR/MWAIT is not advertised.
pub fn query_mwait_info() -> Option<MwaitInfo> {
    if !mwait_feature_present() {
        return None;
    }

    // Save/restore rbx around CPUID — LLVM uses rbx as a base pointer.
    let (eax, ebx, ecx, edx): (u32, u32, u32, u32);
    unsafe {
        let ebx_out: u32;
        core::arch::asm!(
            "push rbx",
            "cpuid",
            "mov {ebx_out:e}, ebx",
            "pop rbx",
            inout("eax") 5u32 => eax,
            ebx_out = out(reg) ebx_out,
            out("ecx") ecx,
            out("edx") edx,
            options(nomem, nostack),
        );
        ebx = ebx_out;
    }

    Some(MwaitInfo {
        monitor_min_line: (eax & 0xFFFF) as u16,
        monitor_max_line: (ebx & 0xFFFF) as u16,
        extensions: ecx & (1 << 0) != 0,
        interrupt_break: ecx & (1 << 1) != 0,
        c0_substates: ((edx >> 0) & 0xF) as u8,
        c1_substates: ((edx >> 4) & 0xF) as u8,
        c2_substates: ((edx >> 8) & 0xF) as u8,
        c3_substates: ((edx >> 12) & 0xF) as u8,
        c4_substates: ((edx >> 16) & 0xF) as u8,
        c5_substates: ((edx >> 20) & 0xF) as u8,
        c6_substates: ((edx >> 24) & 0xF) as u8,
        c7_substates: ((edx >> 28) & 0xF) as u8,
    })
}

// ─── C-state Detection ──────────────────────────────────────────────────────

/// Build the list of C-states supported by this processor.
///
/// C1 (HLT) is always available. C1E, C3, C6, C7, C8, C10 require MWAIT
/// extension support and at least one sub-state in the corresponding
/// CPUID leaf-5 EDX nibble. C8/C10 are inferred from deep C6/C7 sub-states.
pub fn detect_cstates() -> Vec<CState> {
    let mut states = Vec::new();

    // C0 is trivially present (CPU running).
    states.push(CState::C0);

    // C1 via HLT is always available on x86-64.
    states.push(CState::C1);

    let mwait = query_mwait_info();
    let Some(info) = mwait else {
        // No MWAIT — only C0 and C1 via HLT.
        return states;
    };

    if !info.extensions {
        // MWAIT exists but the sub-state extension (ECX bit 0) is absent;
        // we cannot safely map CPUID EDX nibbles to C-states.
        return states;
    }

    // C1E: second C1 sub-state
    if info.c1_substates >= 2 {
        states.push(CState::C1E);
    }

    // C3: at least one C3 sub-state
    if info.c3_substates > 0 {
        states.push(CState::C3);
    }

    // C6: at least one C6 sub-state
    if info.c6_substates > 0 {
        states.push(CState::C6);

        // C8 and C10 are reported as additional C6 sub-states on modern CPUs
        // (Intel terminology: CC6 sub-states 2 and 3 correspond to package
        //  C-states 8 and 10). Two or more sub-states implies C8 capability.
        if info.c6_substates >= 2 {
            states.push(CState::C8);
        }
        if info.c6_substates >= 4 {
            states.push(CState::C10);
        }
    }

    // C7: at least one C7 sub-state
    if info.c7_substates > 0 {
        states.push(CState::C7);
    }

    states
}

// ─── Idle Entry Points ───────────────────────────────────────────────────────

/// Enter an idle state for the given logical CPU.
///
/// Current implementation uses HLT (C1) unconditionally. When MWAIT is
/// available deeper states can be requested by passing the MWAIT hint for the
/// desired C-state, but that requires OS coordination with the ACPI _CST
/// objects — deferred to Phase 3.
///
/// SAFETY: must be called with interrupts enabled so the CPU wakes up.
#[inline]
pub fn enter_idle(_cpu_id: usize) {
    // HLT: puts the processor into the C1 halt state until the next interrupt.
    unsafe {
        core::arch::asm!("hlt", options(nomem, nostack, preserves_flags));
    }
}

/// Enter idle via MWAIT (C3 or deeper) when MWAIT extensions are present.
///
/// `hint` should be the value from `CState::mwait_hint()`.
/// The caller must issue MONITOR before calling this.
///
/// SAFETY: requires interrupts enabled, and a prior MONITOR to set the
/// monitored address — otherwise the processor may spin.
#[inline]
pub unsafe fn enter_mwait(hint: u32) {
    core::arch::asm!(
        // EAX = hint (C-state sub-state selector).
        // ECX = 0 (no extensions flags: bus-lock break not enabled).
        "mwait",
        in("eax") hint,
        in("ecx") 0u32,
        options(nomem, nostack, preserves_flags),
    );
}

// ─── Initialization & Smoketest ─────────────────────────────────────────────

/// Initialize the cpuidle subsystem: detect C-states and log the result.
///
/// On QEMU the MWAIT feature leaf is typically absent; the log will show
/// only `[C0, C1]` with a note that HLT is used. On real hardware a richer
/// list is expected.
pub fn init_cpuidle() {
    let states = detect_cstates();

    // Build a compact name list for the serial log.
    let mut names: alloc::string::String = alloc::string::String::new();
    for (i, s) in states.iter().enumerate() {
        if i > 0 {
            names.push_str(", ");
        }
        names.push_str(s.name());
    }

    let has_mwait = mwait_feature_present();
    crate::serial_println!(
        "[cpuidle] supported C-states: [{}] (using HLT for C1{})",
        names,
        if has_mwait {
            ", MWAIT available for C3+"
        } else {
            ", MWAIT not advertised on this CPU/QEMU"
        },
    );
}

/// Boot smoketest: verify that detect_cstates returns at least C0 and C1,
/// and that enter_idle does not fault.
pub fn run_boot_smoketest() {
    let states = detect_cstates();
    let has_c0 = states.contains(&CState::C0);
    let has_c1 = states.contains(&CState::C1);

    // Exercise the HLT path; this will immediately return on the next timer
    // interrupt so the test is non-blocking in practice.
    enter_idle(0);

    let pass = has_c0 && has_c1;
    crate::serial_println!(
        "[cpuidle] smoketest: C0={} C1={} total_states={} -> {}",
        has_c0 as u8,
        has_c1 as u8,
        states.len(),
        if pass { "PASS" } else { "FAIL" },
    );
}
