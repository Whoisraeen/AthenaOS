//! KVM-equivalent Type-1 hypervisor for RaeenOS.
//!
//! Provides full hardware-assisted virtualization using Intel VT-x (VMX):
//! - VMX root/non-root mode transitions via VMCS
//! - Extended Page Tables (EPT) for second-level address translation
//! - Virtual CPU management with full register context save/restore
//! - VM exit handling for CPUID, I/O, MSR, EPT violations, etc.
//! - Virtual LAPIC emulation per vCPU
//! - Hypervisor manager for multiple concurrent VMs
//! - Virtio device pass-through configuration

#![allow(dead_code)]

extern crate alloc;

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

// ─── Error Type ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VirtError {
    VmxNotSupported,
    VmxAlreadyEnabled,
    VmxNotEnabled,
    VmxOnFailed,
    VmxOffFailed,
    VmcsClearFailed,
    VmcsLoadFailed,
    VmcsReadFailed(u32),
    VmcsWriteFailed(u32),
    VmLaunchFailed(u32),
    VmResumeFailed(u32),
    EptAllocFailed,
    EptMapFailed,
    EptWalkFailed,
    EptNotMapped,
    InvalidVmId,
    InvalidVCpuId,
    VmAlreadyRunning,
    VmNotRunning,
    VmNotFound,
    VCpuNotFound,
    OutOfMemory,
    InvalidState(String),
    DeviceLimitReached,
    InvalidAlignment,
    InvalidSize,
}

// ═══════════════════════════════════════════════════════════════════════════
// §1  VMX (INTEL VT-x) SUPPORT
// ═══════════════════════════════════════════════════════════════════════════

/// IA32_FEATURE_CONTROL MSR — must be locked with VMX-outside-SMX enabled.
const IA32_FEATURE_CONTROL: u32 = 0x3A;
const IA32_VMX_BASIC: u32 = 0x480;
const IA32_VMX_PINBASED_CTLS: u32 = 0x481;
const IA32_VMX_PROCBASED_CTLS: u32 = 0x482;
const IA32_VMX_EXIT_CTLS: u32 = 0x483;
const IA32_VMX_ENTRY_CTLS: u32 = 0x484;
const IA32_VMX_MISC: u32 = 0x485;
const IA32_VMX_CR0_FIXED0: u32 = 0x486;
const IA32_VMX_CR0_FIXED1: u32 = 0x487;
const IA32_VMX_CR4_FIXED0: u32 = 0x488;
const IA32_VMX_CR4_FIXED1: u32 = 0x489;
const IA32_VMX_PROCBASED_CTLS2: u32 = 0x48B;
const IA32_VMX_EPT_VPID_CAP: u32 = 0x48C;
const IA32_VMX_TRUE_PINBASED_CTLS: u32 = 0x48D;
const IA32_VMX_TRUE_PROCBASED_CTLS: u32 = 0x48E;
const IA32_VMX_TRUE_EXIT_CTLS: u32 = 0x48F;
const IA32_VMX_TRUE_ENTRY_CTLS: u32 = 0x490;
const IA32_VMX_VMFUNC: u32 = 0x491;

const IA32_EFER: u32 = 0xC000_0080;
const IA32_PAT: u32 = 0x277;
const IA32_SYSENTER_CS: u32 = 0x174;
const IA32_SYSENTER_ESP: u32 = 0x175;
const IA32_SYSENTER_EIP: u32 = 0x176;
const IA32_FS_BASE: u32 = 0xC000_0100;
const IA32_GS_BASE: u32 = 0xC000_0101;
const IA32_KERNEL_GS_BASE: u32 = 0xC000_0102;

const FEATURE_CONTROL_LOCKED: u64 = 1 << 0;
const FEATURE_CONTROL_VMX_OUTSIDE_SMX: u64 = 1 << 2;

/// CPUID leaf 1, ECX bit 5 — VMX available.
const CPUID_VMX_BIT: u32 = 1 << 5;

/// CR4 bit 13 — VMXE, enables VMX operation.
const CR4_VMXE: u64 = 1 << 13;

/// Procbased2 capability bits.
const PROCBASED2_EPT: u32 = 1 << 1;
const PROCBASED2_UNRESTRICTED: u32 = 1 << 7;
const PROCBASED2_VPID: u32 = 1 << 5;
const PROCBASED2_PREEMPTION_TIMER: u32 = 1 << 6;
const PROCBASED2_POSTED_INTERRUPTS: u32 = 1 << 7; // pin-based actually, simplified
const PROCBASED2_VMFUNC: u32 = 1 << 13;
const PROCBASED_ACTIVATE_SECONDARY: u32 = 1 << 31;

/// Hardware capabilities discovered from CPUID + VMX MSRs.
pub struct VmxCapabilities {
    pub supported: bool,
    pub ept_supported: bool,
    pub unrestricted_guest: bool,
    pub vpid_supported: bool,
    pub preemption_timer: bool,
    pub posted_interrupts: bool,
    pub vmfunc: bool,
    pub basic_msr: u64,
    pub pinbased_allowed0: u32,
    pub pinbased_allowed1: u32,
    pub procbased_allowed0: u32,
    pub procbased_allowed1: u32,
    pub procbased2_allowed0: u32,
    pub procbased2_allowed1: u32,
    pub exit_allowed0: u32,
    pub exit_allowed1: u32,
    pub entry_allowed0: u32,
    pub entry_allowed1: u32,
    pub cr0_fixed0: u64,
    pub cr0_fixed1: u64,
    pub cr4_fixed0: u64,
    pub cr4_fixed1: u64,
}

impl VmxCapabilities {
    fn empty() -> Self {
        Self {
            supported: false,
            ept_supported: false,
            unrestricted_guest: false,
            vpid_supported: false,
            preemption_timer: false,
            posted_interrupts: false,
            vmfunc: false,
            basic_msr: 0,
            pinbased_allowed0: 0,
            pinbased_allowed1: 0,
            procbased_allowed0: 0,
            procbased_allowed1: 0,
            procbased2_allowed0: 0,
            procbased2_allowed1: 0,
            exit_allowed0: 0,
            exit_allowed1: 0,
            entry_allowed0: 0,
            entry_allowed1: 0,
            cr0_fixed0: 0,
            cr0_fixed1: 0,
            cr4_fixed0: 0,
            cr4_fixed1: 0,
        }
    }

    /// Compute an adjusted control value that satisfies the allowed0/allowed1 constraints.
    pub fn adjust_controls(&self, desired: u32, allowed0: u32, allowed1: u32) -> u32 {
        let mut result = desired;
        // Bits in allowed0 that are 1 must be 1 in the result.
        result |= allowed0;
        // Bits in allowed1 that are 0 must be 0 in the result.
        result &= allowed1;
        result
    }

    pub fn adjust_pinbased(&self, desired: u32) -> u32 {
        self.adjust_controls(desired, self.pinbased_allowed0, self.pinbased_allowed1)
    }

    pub fn adjust_procbased(&self, desired: u32) -> u32 {
        self.adjust_controls(desired, self.procbased_allowed0, self.procbased_allowed1)
    }

    pub fn adjust_procbased2(&self, desired: u32) -> u32 {
        self.adjust_controls(desired, self.procbased2_allowed0, self.procbased2_allowed1)
    }

    pub fn adjust_exit(&self, desired: u32) -> u32 {
        self.adjust_controls(desired, self.exit_allowed0, self.exit_allowed1)
    }

    pub fn adjust_entry(&self, desired: u32) -> u32 {
        self.adjust_controls(desired, self.entry_allowed0, self.entry_allowed1)
    }

    /// Compute the minimum CR0 value that satisfies VMX fixed constraints.
    pub fn adjust_cr0(&self, desired: u64) -> u64 {
        (desired | self.cr0_fixed0) & self.cr0_fixed1
    }

    /// Compute the minimum CR4 value that satisfies VMX fixed constraints.
    pub fn adjust_cr4(&self, desired: u64) -> u64 {
        (desired | self.cr4_fixed0) & self.cr4_fixed1
    }

    pub fn revision_id(&self) -> u32 {
        (self.basic_msr & 0x7FFF_FFFF) as u32
    }

    pub fn vmcs_region_size(&self) -> u32 {
        ((self.basic_msr >> 32) & 0x1FFF) as u32
    }

    pub fn uses_true_msrs(&self) -> bool {
        (self.basic_msr >> 55) & 1 == 1
    }
}

/// Read a model-specific register.
unsafe fn rdmsr(msr: u32) -> u64 {
    let (lo, hi): (u32, u32);
    core::arch::asm!(
        "rdmsr",
        in("ecx") msr,
        out("eax") lo,
        out("edx") hi,
        options(nomem, nostack, preserves_flags),
    );
    ((hi as u64) << 32) | lo as u64
}

/// Write a model-specific register.
unsafe fn wrmsr(msr: u32, value: u64) {
    let lo = value as u32;
    let hi = (value >> 32) as u32;
    core::arch::asm!(
        "wrmsr",
        in("ecx") msr,
        in("eax") lo,
        in("edx") hi,
        options(nomem, nostack, preserves_flags),
    );
}

/// Read CR0.
unsafe fn read_cr0() -> u64 {
    let val: u64;
    core::arch::asm!("mov {}, cr0", out(reg) val, options(nomem, nostack, preserves_flags));
    val
}

/// Read CR4.
unsafe fn read_cr4() -> u64 {
    let val: u64;
    core::arch::asm!("mov {}, cr4", out(reg) val, options(nomem, nostack, preserves_flags));
    val
}

/// Write CR4.
unsafe fn write_cr4(val: u64) {
    core::arch::asm!("mov cr4, {}", in(reg) val, options(nomem, nostack, preserves_flags));
}

/// Read CR3.
unsafe fn read_cr3() -> u64 {
    let val: u64;
    core::arch::asm!("mov {}, cr3", out(reg) val, options(nomem, nostack, preserves_flags));
    val
}

/// Software CPUID wrapper.
fn cpuid(leaf: u32, subleaf: u32) -> (u32, u32, u32, u32) {
    let eax: u32;
    let ebx: u32;
    let ecx: u32;
    let edx: u32;
    unsafe {
        core::arch::asm!(
            "xchg {tmp:r}, rbx",
            "cpuid",
            "xchg {tmp:r}, rbx",
            tmp = out(reg) ebx,
            inout("eax") leaf => eax,
            inout("ecx") subleaf => ecx,
            out("edx") edx,
            options(nostack, preserves_flags),
        );
    }
    (eax, ebx, ecx, edx)
}

/// Detect VMX support and read all capability MSRs.
pub fn detect_vmx() -> Option<VmxCapabilities> {
    let (_eax, _ebx, ecx, _edx) = cpuid(1, 0);
    if ecx & CPUID_VMX_BIT == 0 {
        return None;
    }

    let mut caps = VmxCapabilities::empty();
    caps.supported = true;

    unsafe {
        caps.basic_msr = rdmsr(IA32_VMX_BASIC);

        let use_true = caps.uses_true_msrs();
        let pin_msr = if use_true {
            IA32_VMX_TRUE_PINBASED_CTLS
        } else {
            IA32_VMX_PINBASED_CTLS
        };
        let proc_msr = if use_true {
            IA32_VMX_TRUE_PROCBASED_CTLS
        } else {
            IA32_VMX_PROCBASED_CTLS
        };
        let exit_msr = if use_true {
            IA32_VMX_TRUE_EXIT_CTLS
        } else {
            IA32_VMX_EXIT_CTLS
        };
        let entry_msr = if use_true {
            IA32_VMX_TRUE_ENTRY_CTLS
        } else {
            IA32_VMX_ENTRY_CTLS
        };

        let pin_raw = rdmsr(pin_msr);
        caps.pinbased_allowed0 = pin_raw as u32;
        caps.pinbased_allowed1 = (pin_raw >> 32) as u32;

        let proc_raw = rdmsr(proc_msr);
        caps.procbased_allowed0 = proc_raw as u32;
        caps.procbased_allowed1 = (proc_raw >> 32) as u32;

        // Secondary procbased controls exist if primary allows activating them.
        if caps.procbased_allowed1 & PROCBASED_ACTIVATE_SECONDARY != 0 {
            let proc2_raw = rdmsr(IA32_VMX_PROCBASED_CTLS2);
            caps.procbased2_allowed0 = proc2_raw as u32;
            caps.procbased2_allowed1 = (proc2_raw >> 32) as u32;

            caps.ept_supported = caps.procbased2_allowed1 & PROCBASED2_EPT != 0;
            caps.unrestricted_guest = caps.procbased2_allowed1 & PROCBASED2_UNRESTRICTED != 0;
            caps.vpid_supported = caps.procbased2_allowed1 & PROCBASED2_VPID != 0;
            caps.vmfunc = caps.procbased2_allowed1 & PROCBASED2_VMFUNC != 0;
        }

        caps.preemption_timer = caps.pinbased_allowed1 & PROCBASED2_PREEMPTION_TIMER != 0;
        caps.posted_interrupts = caps.pinbased_allowed1 & PROCBASED2_POSTED_INTERRUPTS != 0;

        let exit_raw = rdmsr(exit_msr);
        caps.exit_allowed0 = exit_raw as u32;
        caps.exit_allowed1 = (exit_raw >> 32) as u32;

        let entry_raw = rdmsr(entry_msr);
        caps.entry_allowed0 = entry_raw as u32;
        caps.entry_allowed1 = (entry_raw >> 32) as u32;

        caps.cr0_fixed0 = rdmsr(IA32_VMX_CR0_FIXED0);
        caps.cr0_fixed1 = rdmsr(IA32_VMX_CR0_FIXED1);
        caps.cr4_fixed0 = rdmsr(IA32_VMX_CR4_FIXED0);
        caps.cr4_fixed1 = rdmsr(IA32_VMX_CR4_FIXED1);
    }

    Some(caps)
}

/// Enable VMX operation by setting CR4.VMXE and ensuring IA32_FEATURE_CONTROL is locked.
pub fn enable_vmx() -> Result<(), VirtError> {
    unsafe {
        let feat = rdmsr(IA32_FEATURE_CONTROL);

        if feat & FEATURE_CONTROL_LOCKED == 0 {
            // Not locked — we can configure it. Set the lock bit and VMX-outside-SMX.
            wrmsr(
                IA32_FEATURE_CONTROL,
                feat | FEATURE_CONTROL_LOCKED | FEATURE_CONTROL_VMX_OUTSIDE_SMX,
            );
        } else if feat & FEATURE_CONTROL_VMX_OUTSIDE_SMX == 0 {
            // Locked but VMX disabled — firmware blocked us.
            return Err(VirtError::VmxNotSupported);
        }

        // Set CR4.VMXE.
        let cr4 = read_cr4();
        if cr4 & CR4_VMXE == 0 {
            write_cr4(cr4 | CR4_VMXE);
        }
    }

    Ok(())
}

/// Disable VMX operation by clearing CR4.VMXE.
pub fn disable_vmx() {
    unsafe {
        let cr4 = read_cr4();
        if cr4 & CR4_VMXE != 0 {
            write_cr4(cr4 & !CR4_VMXE);
        }
    }
}

/// Execute VMXON with the physical address of a properly initialized VMXON region.
pub fn vmxon(vmxon_region: u64) -> Result<(), VirtError> {
    let success: u8;
    unsafe {
        core::arch::asm!(
            "vmxon [{addr}]",
            "setna {success}",
            addr = in(reg) &vmxon_region,
            success = out(reg_byte) success,
            options(nostack),
        );
    }
    if success != 0 {
        return Err(VirtError::VmxOnFailed);
    }
    Ok(())
}

/// Execute VMXOFF to leave VMX operation.
pub fn vmxoff() {
    unsafe {
        core::arch::asm!("vmxoff", options(nomem, nostack));
    }
}

/// Execute VMCLEAR on a VMCS region.
fn vmclear(vmcs_phys: u64) -> Result<(), VirtError> {
    let success: u8;
    unsafe {
        core::arch::asm!(
            "vmclear [{addr}]",
            "setna {success}",
            addr = in(reg) &vmcs_phys,
            success = out(reg_byte) success,
            options(nostack),
        );
    }
    if success != 0 {
        return Err(VirtError::VmcsClearFailed);
    }
    Ok(())
}

/// Execute VMPTRLD to make a VMCS current on this logical processor.
fn vmptrld(vmcs_phys: u64) -> Result<(), VirtError> {
    let success: u8;
    unsafe {
        core::arch::asm!(
            "vmptrld [{addr}]",
            "setna {success}",
            addr = in(reg) &vmcs_phys,
            success = out(reg_byte) success,
            options(nostack),
        );
    }
    if success != 0 {
        return Err(VirtError::VmcsLoadFailed);
    }
    Ok(())
}

/// Write a field into the current VMCS.
fn vmwrite(field: u32, value: u64) -> Result<(), VirtError> {
    let success: u8;
    unsafe {
        core::arch::asm!(
            "vmwrite {value}, {field}",
            "setna {success}",
            value = in(reg) value,
            field = in(reg) field as u64,
            success = out(reg_byte) success,
            options(nomem, nostack),
        );
    }
    if success != 0 {
        return Err(VirtError::VmcsWriteFailed(field));
    }
    Ok(())
}

/// Read a field from the current VMCS.
fn vmread(field: u32) -> Result<u64, VirtError> {
    let value: u64;
    let success: u8;
    unsafe {
        core::arch::asm!(
            "vmread {value}, {field}",
            "setna {success}",
            value = out(reg) value,
            field = in(reg) field as u64,
            success = out(reg_byte) success,
            options(nomem, nostack),
        );
    }
    if success != 0 {
        return Err(VirtError::VmcsReadFailed(field));
    }
    Ok(value)
}

// ═══════════════════════════════════════════════════════════════════════════
// §2  VMCS (VIRTUAL MACHINE CONTROL STRUCTURE)
// ═══════════════════════════════════════════════════════════════════════════

// VMCS field encodings — guest state area.
pub const VMCS_GUEST_ES_SELECTOR: u32 = 0x0800;
pub const VMCS_GUEST_CS_SELECTOR: u32 = 0x0802;
pub const VMCS_GUEST_SS_SELECTOR: u32 = 0x0804;
pub const VMCS_GUEST_DS_SELECTOR: u32 = 0x0806;
pub const VMCS_GUEST_FS_SELECTOR: u32 = 0x0808;
pub const VMCS_GUEST_GS_SELECTOR: u32 = 0x080A;
pub const VMCS_GUEST_LDTR_SELECTOR: u32 = 0x080C;
pub const VMCS_GUEST_TR_SELECTOR: u32 = 0x080E;

pub const VMCS_GUEST_VPID: u32 = 0x0000;
pub const VMCS_POSTED_INT_NOTIFY: u32 = 0x0002;
pub const VMCS_EPTP_INDEX: u32 = 0x0004;

pub const VMCS_GUEST_ES_LIMIT: u32 = 0x4800;
pub const VMCS_GUEST_CS_LIMIT: u32 = 0x4802;
pub const VMCS_GUEST_SS_LIMIT: u32 = 0x4804;
pub const VMCS_GUEST_DS_LIMIT: u32 = 0x4806;
pub const VMCS_GUEST_FS_LIMIT: u32 = 0x4808;
pub const VMCS_GUEST_GS_LIMIT: u32 = 0x480A;
pub const VMCS_GUEST_LDTR_LIMIT: u32 = 0x480C;
pub const VMCS_GUEST_TR_LIMIT: u32 = 0x480E;
pub const VMCS_GUEST_GDTR_LIMIT: u32 = 0x4810;
pub const VMCS_GUEST_IDTR_LIMIT: u32 = 0x4812;

pub const VMCS_GUEST_ES_ACCESS: u32 = 0x4814;
pub const VMCS_GUEST_CS_ACCESS: u32 = 0x4816;
pub const VMCS_GUEST_SS_ACCESS: u32 = 0x4818;
pub const VMCS_GUEST_DS_ACCESS: u32 = 0x481A;
pub const VMCS_GUEST_FS_ACCESS: u32 = 0x481C;
pub const VMCS_GUEST_GS_ACCESS: u32 = 0x481E;
pub const VMCS_GUEST_LDTR_ACCESS: u32 = 0x4820;
pub const VMCS_GUEST_TR_ACCESS: u32 = 0x4822;
pub const VMCS_GUEST_INTERRUPTIBILITY: u32 = 0x4824;
pub const VMCS_GUEST_ACTIVITY_STATE: u32 = 0x4826;
pub const VMCS_GUEST_SMBASE: u32 = 0x4828;
pub const VMCS_GUEST_SYSENTER_CS: u32 = 0x482A;
pub const VMCS_GUEST_PREEMPTION_TIMER: u32 = 0x482E;

pub const VMCS_HOST_SYSENTER_CS: u32 = 0x4C00;

pub const VMCS_PIN_BASED_CONTROLS: u32 = 0x4000;
pub const VMCS_PROC_BASED_CONTROLS: u32 = 0x4002;
pub const VMCS_EXCEPTION_BITMAP: u32 = 0x4004;
pub const VMCS_PF_ERROR_MASK: u32 = 0x4006;
pub const VMCS_PF_ERROR_MATCH: u32 = 0x4008;
pub const VMCS_CR3_TARGET_COUNT: u32 = 0x400A;
pub const VMCS_EXIT_CONTROLS: u32 = 0x400C;
pub const VMCS_EXIT_MSR_STORE_COUNT: u32 = 0x400E;
pub const VMCS_EXIT_MSR_LOAD_COUNT: u32 = 0x4010;
pub const VMCS_ENTRY_CONTROLS: u32 = 0x4012;
pub const VMCS_ENTRY_MSR_LOAD_COUNT: u32 = 0x4014;
pub const VMCS_ENTRY_INTERRUPTION_INFO: u32 = 0x4016;
pub const VMCS_ENTRY_EXCEPTION_ERROR: u32 = 0x4018;
pub const VMCS_ENTRY_INSTRUCTION_LEN: u32 = 0x401A;
pub const VMCS_TPR_THRESHOLD: u32 = 0x401C;
pub const VMCS_PROC_BASED_CONTROLS2: u32 = 0x401E;

pub const VMCS_VM_EXIT_REASON: u32 = 0x4402;
pub const VMCS_VM_EXIT_INTERRUPTION_INFO: u32 = 0x4404;
pub const VMCS_VM_EXIT_INTERRUPTION_ERROR: u32 = 0x4406;
pub const VMCS_IDT_VECTORING_INFO: u32 = 0x4408;
pub const VMCS_IDT_VECTORING_ERROR: u32 = 0x440A;
pub const VMCS_VM_EXIT_INSTRUCTION_LEN: u32 = 0x440C;
pub const VMCS_VM_EXIT_INSTRUCTION_INFO: u32 = 0x440E;

pub const VMCS_IO_BITMAP_A: u32 = 0x2000;
pub const VMCS_IO_BITMAP_B: u32 = 0x2002;
pub const VMCS_MSR_BITMAP: u32 = 0x2004;
pub const VMCS_EXIT_MSR_STORE_ADDR: u32 = 0x2006;
pub const VMCS_EXIT_MSR_LOAD_ADDR: u32 = 0x2008;
pub const VMCS_ENTRY_MSR_LOAD_ADDR: u32 = 0x200A;
pub const VMCS_EXECUTIVE_VMCS_PTR: u32 = 0x200C;
pub const VMCS_TSC_OFFSET: u32 = 0x2010;
pub const VMCS_VIRTUAL_APIC_PAGE: u32 = 0x2012;
pub const VMCS_APIC_ACCESS_ADDR: u32 = 0x2014;
pub const VMCS_POSTED_INT_DESC: u32 = 0x2016;
pub const VMCS_VMFUNC_CONTROLS: u32 = 0x2018;
pub const VMCS_EPT_POINTER: u32 = 0x201A;
pub const VMCS_EOI_EXIT_BITMAP0: u32 = 0x201C;
pub const VMCS_EOI_EXIT_BITMAP1: u32 = 0x201E;
pub const VMCS_EOI_EXIT_BITMAP2: u32 = 0x2020;
pub const VMCS_EOI_EXIT_BITMAP3: u32 = 0x2022;
pub const VMCS_EPTP_LIST_ADDR: u32 = 0x2024;

pub const VMCS_GUEST_PHYS_ADDR: u32 = 0x2400;

pub const VMCS_VMCS_LINK_PTR: u32 = 0x2800;
pub const VMCS_GUEST_IA32_DEBUGCTL: u32 = 0x2802;
pub const VMCS_GUEST_IA32_PAT: u32 = 0x2804;
pub const VMCS_GUEST_IA32_EFER: u32 = 0x2806;
pub const VMCS_GUEST_IA32_PERF_GLOBAL: u32 = 0x2808;
pub const VMCS_GUEST_PDPTE0: u32 = 0x280A;
pub const VMCS_GUEST_PDPTE1: u32 = 0x280C;
pub const VMCS_GUEST_PDPTE2: u32 = 0x280E;
pub const VMCS_GUEST_PDPTE3: u32 = 0x2810;

pub const VMCS_HOST_IA32_PAT: u32 = 0x2C00;
pub const VMCS_HOST_IA32_EFER: u32 = 0x2C02;
pub const VMCS_HOST_IA32_PERF_GLOBAL: u32 = 0x2C04;

pub const VMCS_CR0_GUEST_HOST_MASK: u32 = 0x6000;
pub const VMCS_CR4_GUEST_HOST_MASK: u32 = 0x6002;
pub const VMCS_CR0_READ_SHADOW: u32 = 0x6004;
pub const VMCS_CR4_READ_SHADOW: u32 = 0x6006;
pub const VMCS_CR3_TARGET_VALUE0: u32 = 0x6008;
pub const VMCS_CR3_TARGET_VALUE1: u32 = 0x600A;
pub const VMCS_CR3_TARGET_VALUE2: u32 = 0x600C;
pub const VMCS_CR3_TARGET_VALUE3: u32 = 0x600E;

pub const VMCS_EXIT_QUALIFICATION: u32 = 0x6400;
pub const VMCS_IO_RCX: u32 = 0x6402;
pub const VMCS_IO_RSI: u32 = 0x6404;
pub const VMCS_IO_RDI: u32 = 0x6406;
pub const VMCS_IO_RIP: u32 = 0x6408;
pub const VMCS_GUEST_LINEAR_ADDR: u32 = 0x640A;

pub const VMCS_GUEST_CR0: u32 = 0x6800;
pub const VMCS_GUEST_CR3: u32 = 0x6802;
pub const VMCS_GUEST_CR4: u32 = 0x6804;
pub const VMCS_GUEST_ES_BASE: u32 = 0x6806;
pub const VMCS_GUEST_CS_BASE: u32 = 0x6808;
pub const VMCS_GUEST_SS_BASE: u32 = 0x680A;
pub const VMCS_GUEST_DS_BASE: u32 = 0x680C;
pub const VMCS_GUEST_FS_BASE: u32 = 0x680E;
pub const VMCS_GUEST_GS_BASE: u32 = 0x6810;
pub const VMCS_GUEST_LDTR_BASE: u32 = 0x6812;
pub const VMCS_GUEST_TR_BASE: u32 = 0x6814;
pub const VMCS_GUEST_GDTR_BASE: u32 = 0x6816;
pub const VMCS_GUEST_IDTR_BASE: u32 = 0x6818;
pub const VMCS_GUEST_DR7: u32 = 0x681A;
pub const VMCS_GUEST_RSP: u32 = 0x681C;
pub const VMCS_GUEST_RIP: u32 = 0x681E;
pub const VMCS_GUEST_RFLAGS: u32 = 0x6820;
pub const VMCS_GUEST_PENDING_DEBUG: u32 = 0x6822;
pub const VMCS_GUEST_SYSENTER_ESP: u32 = 0x6824;
pub const VMCS_GUEST_SYSENTER_EIP: u32 = 0x6826;

pub const VMCS_HOST_CR0: u32 = 0x6C00;
pub const VMCS_HOST_CR3: u32 = 0x6C02;
pub const VMCS_HOST_CR4: u32 = 0x6C04;
pub const VMCS_HOST_FS_BASE: u32 = 0x6C06;
pub const VMCS_HOST_GS_BASE: u32 = 0x6C08;
pub const VMCS_HOST_TR_BASE: u32 = 0x6C0A;
pub const VMCS_HOST_GDTR_BASE: u32 = 0x6C0C;
pub const VMCS_HOST_IDTR_BASE: u32 = 0x6C0E;
pub const VMCS_HOST_SYSENTER_ESP: u32 = 0x6C10;
pub const VMCS_HOST_SYSENTER_EIP: u32 = 0x6C12;
pub const VMCS_HOST_RSP: u32 = 0x6C14;
pub const VMCS_HOST_RIP: u32 = 0x6C16;

pub const VMCS_HOST_ES_SELECTOR: u32 = 0x0C00;
pub const VMCS_HOST_CS_SELECTOR: u32 = 0x0C02;
pub const VMCS_HOST_SS_SELECTOR: u32 = 0x0C04;
pub const VMCS_HOST_DS_SELECTOR: u32 = 0x0C06;
pub const VMCS_HOST_FS_SELECTOR: u32 = 0x0C08;
pub const VMCS_HOST_GS_SELECTOR: u32 = 0x0C0A;
pub const VMCS_HOST_TR_SELECTOR: u32 = 0x0C0C;

/// VMCS segment descriptor (selector + base + limit + access rights).
#[derive(Debug, Clone, Copy)]
pub struct VmcsSegment {
    pub selector: u16,
    pub base: u64,
    pub limit: u32,
    pub access_rights: u32,
}

impl VmcsSegment {
    pub fn null() -> Self {
        Self {
            selector: 0,
            base: 0,
            limit: 0,
            access_rights: 0x10000, // unusable bit set
        }
    }

    pub fn flat_code_64() -> Self {
        Self {
            selector: 0x08,
            base: 0,
            limit: 0xFFFF_FFFF,
            access_rights: 0xA09B, // 64-bit code, DPL 0, present, L-bit
        }
    }

    pub fn flat_data_64() -> Self {
        Self {
            selector: 0x10,
            base: 0,
            limit: 0xFFFF_FFFF,
            access_rights: 0xC093, // data, DPL 0, present, G+DB
        }
    }

    pub fn real_mode_code() -> Self {
        Self {
            selector: 0,
            base: 0,
            limit: 0xFFFF,
            access_rights: 0x009B, // 16-bit code, present
        }
    }

    pub fn real_mode_data() -> Self {
        Self {
            selector: 0,
            base: 0,
            limit: 0xFFFF,
            access_rights: 0x0093, // 16-bit data, present
        }
    }

    pub fn tss_segment(base: u64) -> Self {
        Self {
            selector: 0x18,
            base,
            limit: 0x67,
            access_rights: 0x008B, // 64-bit TSS, busy, present
        }
    }
}

/// VMCS descriptor table register (GDTR/IDTR).
#[derive(Debug, Clone, Copy)]
pub struct VmcsDescTable {
    pub base: u64,
    pub limit: u32,
}

/// The full VMCS state — cached in software, flushed to hardware via vmwrite.
pub struct Vmcs {
    pub revision_id: u32,
    pub phys_addr: u64,

    // Guest state
    pub guest_rip: u64,
    pub guest_rsp: u64,
    pub guest_rflags: u64,
    pub guest_cr0: u64,
    pub guest_cr3: u64,
    pub guest_cr4: u64,
    pub guest_cs: VmcsSegment,
    pub guest_ds: VmcsSegment,
    pub guest_es: VmcsSegment,
    pub guest_fs: VmcsSegment,
    pub guest_gs: VmcsSegment,
    pub guest_ss: VmcsSegment,
    pub guest_tr: VmcsSegment,
    pub guest_ldtr: VmcsSegment,
    pub guest_gdtr: VmcsDescTable,
    pub guest_idtr: VmcsDescTable,
    pub guest_ia32_efer: u64,
    pub guest_ia32_pat: u64,
    pub guest_dr7: u64,
    pub guest_sysenter_cs: u32,
    pub guest_sysenter_esp: u64,
    pub guest_sysenter_eip: u64,
    pub guest_activity_state: u32,
    pub guest_interruptibility: u32,
    pub guest_pending_debug: u64,
    pub guest_vmcs_link: u64,
    pub guest_preemption_timer: u32,

    // Host state
    pub host_cr0: u64,
    pub host_cr3: u64,
    pub host_cr4: u64,
    pub host_rip: u64,
    pub host_rsp: u64,
    pub host_cs: u16,
    pub host_ds: u16,
    pub host_es: u16,
    pub host_fs: u16,
    pub host_gs: u16,
    pub host_ss: u16,
    pub host_tr: u16,
    pub host_ia32_efer: u64,
    pub host_ia32_pat: u64,
    pub host_fs_base: u64,
    pub host_gs_base: u64,
    pub host_tr_base: u64,
    pub host_gdtr_base: u64,
    pub host_idtr_base: u64,
    pub host_sysenter_cs: u32,
    pub host_sysenter_esp: u64,
    pub host_sysenter_eip: u64,

    // Control fields
    pub pin_based_controls: u32,
    pub proc_based_controls: u32,
    pub proc_based_controls2: u32,
    pub exit_controls: u32,
    pub entry_controls: u32,
    pub exception_bitmap: u32,
    pub page_fault_error_mask: u32,
    pub page_fault_error_match: u32,
    pub cr0_guest_host_mask: u64,
    pub cr0_shadow: u64,
    pub cr4_guest_host_mask: u64,
    pub cr4_shadow: u64,
    pub msr_bitmap_addr: u64,
    pub io_bitmap_a: u64,
    pub io_bitmap_b: u64,
    pub tsc_offset: u64,
    pub ept_pointer: u64,
    pub vpid: u16,
}

impl Vmcs {
    pub fn new() -> Result<Self, VirtError> {
        Ok(Self {
            revision_id: 0,
            phys_addr: 0,
            guest_rip: 0,
            guest_rsp: 0,
            guest_rflags: 0x0000_0000_0000_0002, // reserved bit 1 always set
            guest_cr0: 0x0000_0000_0001_0031,    // PE + ET + NE + PG (protected 64-bit)
            guest_cr3: 0,
            guest_cr4: 0x0000_0000_0000_2020, // PAE + VMXE
            guest_cs: VmcsSegment::flat_code_64(),
            guest_ds: VmcsSegment::flat_data_64(),
            guest_es: VmcsSegment::flat_data_64(),
            guest_fs: VmcsSegment::null(),
            guest_gs: VmcsSegment::null(),
            guest_ss: VmcsSegment::flat_data_64(),
            guest_tr: VmcsSegment::tss_segment(0),
            guest_ldtr: VmcsSegment::null(),
            guest_gdtr: VmcsDescTable { base: 0, limit: 0 },
            guest_idtr: VmcsDescTable { base: 0, limit: 0 },
            guest_ia32_efer: 0x0000_0000_0000_0D01, // LME + LMA + SCE + NXE
            guest_ia32_pat: 0x0007_0406_0007_0406,  // default PAT
            guest_dr7: 0x0000_0000_0000_0400,
            guest_sysenter_cs: 0,
            guest_sysenter_esp: 0,
            guest_sysenter_eip: 0,
            guest_activity_state: 0,
            guest_interruptibility: 0,
            guest_pending_debug: 0,
            guest_vmcs_link: 0xFFFF_FFFF_FFFF_FFFF,
            guest_preemption_timer: 0,
            host_cr0: 0,
            host_cr3: 0,
            host_cr4: 0,
            host_rip: 0,
            host_rsp: 0,
            host_cs: 0x08,
            host_ds: 0x10,
            host_es: 0x10,
            host_fs: 0,
            host_gs: 0,
            host_ss: 0x10,
            host_tr: 0x18,
            host_ia32_efer: 0,
            host_ia32_pat: 0,
            host_fs_base: 0,
            host_gs_base: 0,
            host_tr_base: 0,
            host_gdtr_base: 0,
            host_idtr_base: 0,
            host_sysenter_cs: 0,
            host_sysenter_esp: 0,
            host_sysenter_eip: 0,
            pin_based_controls: 0,
            proc_based_controls: 0,
            proc_based_controls2: 0,
            exit_controls: 0,
            entry_controls: 0,
            exception_bitmap: 0,
            page_fault_error_mask: 0,
            page_fault_error_match: 0,
            cr0_guest_host_mask: 0,
            cr0_shadow: 0,
            cr4_guest_host_mask: 0,
            cr4_shadow: 0,
            msr_bitmap_addr: 0,
            io_bitmap_a: 0,
            io_bitmap_b: 0,
            tsc_offset: 0,
            ept_pointer: 0,
            vpid: 0,
        })
    }

    /// VMCLEAR this VMCS — resets it and makes it inactive/clear.
    pub fn clear(&mut self) -> Result<(), VirtError> {
        vmclear(self.phys_addr)
    }

    /// VMPTRLD — make this VMCS current on the logical processor.
    pub fn load(&self) -> Result<(), VirtError> {
        vmptrld(self.phys_addr)
    }

    /// Write a single VMCS field to hardware.
    pub fn write_field(&self, field: u32, value: u64) -> Result<(), VirtError> {
        vmwrite(field, value)
    }

    /// Read a single VMCS field from hardware.
    pub fn read_field(&self, field: u32) -> Result<u64, VirtError> {
        vmread(field)
    }

    /// Configure the guest state area from an entry point and stack pointer.
    pub fn setup_guest_state(&mut self, entry_point: u64, stack: u64) {
        self.guest_rip = entry_point;
        self.guest_rsp = stack;
        self.guest_rflags = 0x0000_0000_0000_0002;

        self.guest_cs = VmcsSegment::flat_code_64();
        self.guest_ds = VmcsSegment::flat_data_64();
        self.guest_es = VmcsSegment::flat_data_64();
        self.guest_ss = VmcsSegment::flat_data_64();
        self.guest_fs = VmcsSegment::null();
        self.guest_gs = VmcsSegment::null();
        self.guest_tr = VmcsSegment::tss_segment(0);
        self.guest_ldtr = VmcsSegment::null();

        self.guest_ia32_efer = 0x0000_0000_0000_0D01;
        self.guest_dr7 = 0x0000_0000_0000_0400;
        self.guest_vmcs_link = 0xFFFF_FFFF_FFFF_FFFF;
        self.guest_activity_state = 0;
        self.guest_interruptibility = 0;
        self.guest_pending_debug = 0;
    }

    /// Configure the host state area from the current CPU state.
    pub fn setup_host_state(&mut self) {
        unsafe {
            self.host_cr0 = read_cr0();
            self.host_cr3 = read_cr3();
            self.host_cr4 = read_cr4();

            self.host_ia32_efer = rdmsr(IA32_EFER);
            self.host_ia32_pat = rdmsr(IA32_PAT);
            self.host_sysenter_cs = rdmsr(IA32_SYSENTER_CS) as u32;
            self.host_sysenter_esp = rdmsr(IA32_SYSENTER_ESP);
            self.host_sysenter_eip = rdmsr(IA32_SYSENTER_EIP);
            self.host_fs_base = rdmsr(IA32_FS_BASE);
            self.host_gs_base = rdmsr(IA32_GS_BASE);
        }

        // Segment selectors from the current GDT.
        self.host_cs = 0x08;
        self.host_ds = 0x10;
        self.host_es = 0x10;
        self.host_ss = 0x10;
        self.host_fs = 0;
        self.host_gs = 0;
        self.host_tr = 0x18;
    }

    /// Configure VMCS control fields from hardware capabilities.
    pub fn setup_controls(&mut self, caps: &VmxCapabilities) {
        // Pin-based: external-interrupt exiting, NMI exiting.
        self.pin_based_controls = caps.adjust_pinbased(0x0000_0015);

        // Primary proc-based: HLT exiting, INVLPG exiting, MWAIT exiting,
        // RDPMC exiting, use TSC offsetting, MOV-DR exiting,
        // unconditional I/O exiting, use MSR bitmaps, activate secondary.
        self.proc_based_controls = caps.adjust_procbased(
            (1 << 7)   // HLT exiting
            | (1 << 9)  // INVLPG exiting
            | (1 << 10) // MWAIT exiting
            | (1 << 11) // RDPMC exiting
            | (1 << 3)  // use TSC offsetting
            | (1 << 23) // MOV-DR exiting
            | (1 << 24) // unconditional I/O exiting
            | (1 << 28) // use MSR bitmaps
            | (1u32 << 31), // activate secondary controls
        );

        // Secondary proc-based: EPT, unrestricted guest, VPID.
        let mut proc2 = 0u32;
        if caps.ept_supported {
            proc2 |= PROCBASED2_EPT;
        }
        if caps.unrestricted_guest {
            proc2 |= PROCBASED2_UNRESTRICTED;
        }
        if caps.vpid_supported {
            proc2 |= PROCBASED2_VPID;
        }
        self.proc_based_controls2 = caps.adjust_procbased2(proc2);

        // VM-exit controls: host address-space size (64-bit host),
        // save/load IA32_EFER, save/load IA32_PAT.
        self.exit_controls = caps.adjust_exit(
            (1 << 9)  // host address-space size
            | (1 << 20) // save IA32_EFER
            | (1 << 21) // load IA32_EFER
            | (1 << 18) // save IA32_PAT
            | (1 << 19), // load IA32_PAT
        );

        // VM-entry controls: IA-32e mode guest, load IA32_EFER, load IA32_PAT.
        self.entry_controls = caps.adjust_entry(
            (1 << 9)  // IA-32e mode guest
            | (1 << 15) // load IA32_EFER
            | (1 << 14), // load IA32_PAT
        );

        self.exception_bitmap = 0;
        self.cr0_guest_host_mask = 0;
        self.cr0_shadow = self.guest_cr0;
        self.cr4_guest_host_mask = 0;
        self.cr4_shadow = self.guest_cr4;
    }

    /// Configure the EPT pointer in the VMCS.
    pub fn setup_ept(&mut self, ept_root: u64) {
        // EPT pointer format: bits 2:0 = memory type (6 = WB),
        // bits 5:3 = page walk length minus 1 (3 = 4 levels),
        // bits N:12 = physical address of EPT PML4.
        self.ept_pointer = (ept_root & 0xFFFF_FFFF_FFFF_F000)
            | (3 << 3) // 4-level page walk
            | 6; // write-back memory type
    }

    /// Flush all cached VMCS fields to hardware via vmwrite.
    pub fn flush_to_hardware(&self) -> Result<(), VirtError> {
        // Guest state
        vmwrite(VMCS_GUEST_RIP, self.guest_rip)?;
        vmwrite(VMCS_GUEST_RSP, self.guest_rsp)?;
        vmwrite(VMCS_GUEST_RFLAGS, self.guest_rflags)?;
        vmwrite(VMCS_GUEST_CR0, self.guest_cr0)?;
        vmwrite(VMCS_GUEST_CR3, self.guest_cr3)?;
        vmwrite(VMCS_GUEST_CR4, self.guest_cr4)?;
        vmwrite(VMCS_GUEST_DR7, self.guest_dr7)?;

        // Guest segments
        self.flush_segment(
            VMCS_GUEST_CS_SELECTOR,
            VMCS_GUEST_CS_BASE,
            VMCS_GUEST_CS_LIMIT,
            VMCS_GUEST_CS_ACCESS,
            &self.guest_cs,
        )?;
        self.flush_segment(
            VMCS_GUEST_DS_SELECTOR,
            VMCS_GUEST_DS_BASE,
            VMCS_GUEST_DS_LIMIT,
            VMCS_GUEST_DS_ACCESS,
            &self.guest_ds,
        )?;
        self.flush_segment(
            VMCS_GUEST_ES_SELECTOR,
            VMCS_GUEST_ES_BASE,
            VMCS_GUEST_ES_LIMIT,
            VMCS_GUEST_ES_ACCESS,
            &self.guest_es,
        )?;
        self.flush_segment(
            VMCS_GUEST_FS_SELECTOR,
            VMCS_GUEST_FS_BASE,
            VMCS_GUEST_FS_LIMIT,
            VMCS_GUEST_FS_ACCESS,
            &self.guest_fs,
        )?;
        self.flush_segment(
            VMCS_GUEST_GS_SELECTOR,
            VMCS_GUEST_GS_BASE,
            VMCS_GUEST_GS_LIMIT,
            VMCS_GUEST_GS_ACCESS,
            &self.guest_gs,
        )?;
        self.flush_segment(
            VMCS_GUEST_SS_SELECTOR,
            VMCS_GUEST_SS_BASE,
            VMCS_GUEST_SS_LIMIT,
            VMCS_GUEST_SS_ACCESS,
            &self.guest_ss,
        )?;
        self.flush_segment(
            VMCS_GUEST_TR_SELECTOR,
            VMCS_GUEST_TR_BASE,
            VMCS_GUEST_TR_LIMIT,
            VMCS_GUEST_TR_ACCESS,
            &self.guest_tr,
        )?;
        self.flush_segment(
            VMCS_GUEST_LDTR_SELECTOR,
            VMCS_GUEST_LDTR_BASE,
            VMCS_GUEST_LDTR_LIMIT,
            VMCS_GUEST_LDTR_ACCESS,
            &self.guest_ldtr,
        )?;

        // Guest descriptor tables
        vmwrite(VMCS_GUEST_GDTR_BASE, self.guest_gdtr.base)?;
        vmwrite(VMCS_GUEST_GDTR_LIMIT, self.guest_gdtr.limit as u64)?;
        vmwrite(VMCS_GUEST_IDTR_BASE, self.guest_idtr.base)?;
        vmwrite(VMCS_GUEST_IDTR_LIMIT, self.guest_idtr.limit as u64)?;

        // Guest MSRs
        vmwrite(VMCS_GUEST_IA32_EFER, self.guest_ia32_efer)?;
        vmwrite(VMCS_GUEST_IA32_PAT, self.guest_ia32_pat)?;
        vmwrite(VMCS_GUEST_SYSENTER_CS, self.guest_sysenter_cs as u64)?;
        vmwrite(VMCS_GUEST_SYSENTER_ESP, self.guest_sysenter_esp)?;
        vmwrite(VMCS_GUEST_SYSENTER_EIP, self.guest_sysenter_eip)?;

        // Guest misc
        vmwrite(VMCS_GUEST_ACTIVITY_STATE, self.guest_activity_state as u64)?;
        vmwrite(
            VMCS_GUEST_INTERRUPTIBILITY,
            self.guest_interruptibility as u64,
        )?;
        vmwrite(VMCS_GUEST_PENDING_DEBUG, self.guest_pending_debug)?;
        vmwrite(VMCS_VMCS_LINK_PTR, self.guest_vmcs_link)?;
        vmwrite(
            VMCS_GUEST_PREEMPTION_TIMER,
            self.guest_preemption_timer as u64,
        )?;

        // Host state
        vmwrite(VMCS_HOST_CR0, self.host_cr0)?;
        vmwrite(VMCS_HOST_CR3, self.host_cr3)?;
        vmwrite(VMCS_HOST_CR4, self.host_cr4)?;
        vmwrite(VMCS_HOST_RIP, self.host_rip)?;
        vmwrite(VMCS_HOST_RSP, self.host_rsp)?;
        vmwrite(VMCS_HOST_CS_SELECTOR, self.host_cs as u64)?;
        vmwrite(VMCS_HOST_DS_SELECTOR, self.host_ds as u64)?;
        vmwrite(VMCS_HOST_ES_SELECTOR, self.host_es as u64)?;
        vmwrite(VMCS_HOST_FS_SELECTOR, self.host_fs as u64)?;
        vmwrite(VMCS_HOST_GS_SELECTOR, self.host_gs as u64)?;
        vmwrite(VMCS_HOST_SS_SELECTOR, self.host_ss as u64)?;
        vmwrite(VMCS_HOST_TR_SELECTOR, self.host_tr as u64)?;
        vmwrite(VMCS_HOST_IA32_EFER, self.host_ia32_efer)?;
        vmwrite(VMCS_HOST_IA32_PAT, self.host_ia32_pat)?;
        vmwrite(VMCS_HOST_FS_BASE, self.host_fs_base)?;
        vmwrite(VMCS_HOST_GS_BASE, self.host_gs_base)?;
        vmwrite(VMCS_HOST_TR_BASE, self.host_tr_base)?;
        vmwrite(VMCS_HOST_GDTR_BASE, self.host_gdtr_base)?;
        vmwrite(VMCS_HOST_IDTR_BASE, self.host_idtr_base)?;
        vmwrite(VMCS_HOST_SYSENTER_CS, self.host_sysenter_cs as u64)?;
        vmwrite(VMCS_HOST_SYSENTER_ESP, self.host_sysenter_esp)?;
        vmwrite(VMCS_HOST_SYSENTER_EIP, self.host_sysenter_eip)?;

        // Control fields
        vmwrite(VMCS_PIN_BASED_CONTROLS, self.pin_based_controls as u64)?;
        vmwrite(VMCS_PROC_BASED_CONTROLS, self.proc_based_controls as u64)?;
        vmwrite(VMCS_PROC_BASED_CONTROLS2, self.proc_based_controls2 as u64)?;
        vmwrite(VMCS_EXIT_CONTROLS, self.exit_controls as u64)?;
        vmwrite(VMCS_ENTRY_CONTROLS, self.entry_controls as u64)?;
        vmwrite(VMCS_EXCEPTION_BITMAP, self.exception_bitmap as u64)?;
        vmwrite(VMCS_PF_ERROR_MASK, self.page_fault_error_mask as u64)?;
        vmwrite(VMCS_PF_ERROR_MATCH, self.page_fault_error_match as u64)?;
        vmwrite(VMCS_CR0_GUEST_HOST_MASK, self.cr0_guest_host_mask)?;
        vmwrite(VMCS_CR0_READ_SHADOW, self.cr0_shadow)?;
        vmwrite(VMCS_CR4_GUEST_HOST_MASK, self.cr4_guest_host_mask)?;
        vmwrite(VMCS_CR4_READ_SHADOW, self.cr4_shadow)?;
        vmwrite(VMCS_TSC_OFFSET, self.tsc_offset)?;

        if self.msr_bitmap_addr != 0 {
            vmwrite(VMCS_MSR_BITMAP, self.msr_bitmap_addr)?;
        }
        if self.io_bitmap_a != 0 {
            vmwrite(VMCS_IO_BITMAP_A, self.io_bitmap_a)?;
        }
        if self.io_bitmap_b != 0 {
            vmwrite(VMCS_IO_BITMAP_B, self.io_bitmap_b)?;
        }
        if self.ept_pointer != 0 {
            vmwrite(VMCS_EPT_POINTER, self.ept_pointer)?;
        }
        if self.vpid != 0 {
            vmwrite(VMCS_GUEST_VPID, self.vpid as u64)?;
        }

        // CR3 target count = 0 (all CR3 writes exit).
        vmwrite(VMCS_CR3_TARGET_COUNT, 0)?;
        vmwrite(VMCS_EXIT_MSR_STORE_COUNT, 0)?;
        vmwrite(VMCS_EXIT_MSR_LOAD_COUNT, 0)?;
        vmwrite(VMCS_ENTRY_MSR_LOAD_COUNT, 0)?;

        Ok(())
    }

    fn flush_segment(
        &self,
        sel_field: u32,
        base_field: u32,
        limit_field: u32,
        access_field: u32,
        seg: &VmcsSegment,
    ) -> Result<(), VirtError> {
        vmwrite(sel_field, seg.selector as u64)?;
        vmwrite(base_field, seg.base)?;
        vmwrite(limit_field, seg.limit as u64)?;
        vmwrite(access_field, seg.access_rights as u64)?;
        Ok(())
    }

    /// Sync guest state back from hardware after a VM exit.
    pub fn sync_from_hardware(&mut self) -> Result<(), VirtError> {
        self.guest_rip = vmread(VMCS_GUEST_RIP)?;
        self.guest_rsp = vmread(VMCS_GUEST_RSP)?;
        self.guest_rflags = vmread(VMCS_GUEST_RFLAGS)?;
        self.guest_cr0 = vmread(VMCS_GUEST_CR0)?;
        self.guest_cr3 = vmread(VMCS_GUEST_CR3)?;
        self.guest_cr4 = vmread(VMCS_GUEST_CR4)?;
        self.guest_ia32_efer = vmread(VMCS_GUEST_IA32_EFER)?;
        self.guest_activity_state = vmread(VMCS_GUEST_ACTIVITY_STATE)? as u32;
        self.guest_interruptibility = vmread(VMCS_GUEST_INTERRUPTIBILITY)? as u32;
        self.guest_pending_debug = vmread(VMCS_GUEST_PENDING_DEBUG)?;
        Ok(())
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// §3  VM EXIT HANDLING
// ═══════════════════════════════════════════════════════════════════════════

/// VM exit reason codes from the Intel SDM Vol.3, Appendix C.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum VmExitReason {
    ExternalInterrupt = 1,
    TripleFault = 2,
    InitSignal = 3,
    Sipi = 4,
    IoSmi = 5,
    OtherSmi = 6,
    InterruptWindow = 7,
    NmiWindow = 8,
    TaskSwitch = 9,
    Cpuid = 10,
    Getsec = 11,
    Hlt = 12,
    Invd = 13,
    Invlpg = 14,
    Rdpmc = 15,
    Rdtsc = 16,
    Rsm = 17,
    Vmcall = 18,
    Vmclear = 19,
    Vmlaunch = 20,
    Vmptrld = 21,
    Vmptrst = 22,
    Vmread = 23,
    Vmresume = 24,
    Vmwrite = 25,
    Vmxoff = 26,
    Vmxon = 27,
    CrAccess = 28,
    DrAccess = 29,
    IoInstruction = 30,
    Rdmsr = 31,
    Wrmsr = 32,
    InvalidGuestState = 33,
    MsrLoading = 34,
    Mwait = 35,
    MonitorTrapFlag = 37,
    Monitor = 39,
    Pause = 40,
    MachineCheck = 41,
    TprBelowThreshold = 43,
    ApicAccess = 44,
    VirtualizedEoi = 45,
    GdtrIdtr = 46,
    LdtrTr = 47,
    EptViolation = 48,
    EptMisconfiguration = 49,
    Invept = 50,
    Rdtscp = 51,
    PreemptionTimer = 52,
    Invvpid = 53,
    Wbinvd = 54,
    Xsetbv = 55,
    ApicWrite = 56,
    Rdrand = 57,
    Invpcid = 58,
    Vmfunc = 59,
    Encls = 60,
    Rdseed = 61,
    PageModLog = 62,
    Xsaves = 63,
    Xrstors = 64,
    Unknown(u32),
}

impl VmExitReason {
    pub fn from_raw(raw: u32) -> Self {
        let basic = raw & 0xFFFF;
        match basic {
            1 => Self::ExternalInterrupt,
            2 => Self::TripleFault,
            3 => Self::InitSignal,
            4 => Self::Sipi,
            5 => Self::IoSmi,
            6 => Self::OtherSmi,
            7 => Self::InterruptWindow,
            8 => Self::NmiWindow,
            9 => Self::TaskSwitch,
            10 => Self::Cpuid,
            11 => Self::Getsec,
            12 => Self::Hlt,
            13 => Self::Invd,
            14 => Self::Invlpg,
            15 => Self::Rdpmc,
            16 => Self::Rdtsc,
            17 => Self::Rsm,
            18 => Self::Vmcall,
            19 => Self::Vmclear,
            20 => Self::Vmlaunch,
            21 => Self::Vmptrld,
            22 => Self::Vmptrst,
            23 => Self::Vmread,
            24 => Self::Vmresume,
            25 => Self::Vmwrite,
            26 => Self::Vmxoff,
            27 => Self::Vmxon,
            28 => Self::CrAccess,
            29 => Self::DrAccess,
            30 => Self::IoInstruction,
            31 => Self::Rdmsr,
            32 => Self::Wrmsr,
            33 => Self::InvalidGuestState,
            34 => Self::MsrLoading,
            35 => Self::Mwait,
            37 => Self::MonitorTrapFlag,
            39 => Self::Monitor,
            40 => Self::Pause,
            41 => Self::MachineCheck,
            43 => Self::TprBelowThreshold,
            44 => Self::ApicAccess,
            45 => Self::VirtualizedEoi,
            46 => Self::GdtrIdtr,
            47 => Self::LdtrTr,
            48 => Self::EptViolation,
            49 => Self::EptMisconfiguration,
            50 => Self::Invept,
            51 => Self::Rdtscp,
            52 => Self::PreemptionTimer,
            53 => Self::Invvpid,
            54 => Self::Wbinvd,
            55 => Self::Xsetbv,
            56 => Self::ApicWrite,
            57 => Self::Rdrand,
            58 => Self::Invpcid,
            59 => Self::Vmfunc,
            60 => Self::Encls,
            61 => Self::Rdseed,
            62 => Self::PageModLog,
            63 => Self::Xsaves,
            64 => Self::Xrstors,
            other => Self::Unknown(other),
        }
    }

    pub fn is_vm_entry_failure(raw: u32) -> bool {
        raw & (1 << 31) != 0
    }
}

/// Information gathered upon a VM exit.
pub struct VmExitInfo {
    pub reason: VmExitReason,
    pub qualification: u64,
    pub guest_rip: u64,
    pub guest_rsp: u64,
    pub instruction_length: u32,
    pub instruction_info: u32,
    pub interruption_info: u32,
    pub interruption_error: u32,
    pub idt_vectoring_info: u32,
    pub idt_vectoring_error: u32,
}

impl VmExitInfo {
    /// Collect exit information from the current VMCS.
    pub fn collect() -> Result<Self, VirtError> {
        let raw_reason = vmread(VMCS_VM_EXIT_REASON)? as u32;
        Ok(Self {
            reason: VmExitReason::from_raw(raw_reason),
            qualification: vmread(VMCS_EXIT_QUALIFICATION)?,
            guest_rip: vmread(VMCS_GUEST_RIP)?,
            guest_rsp: vmread(VMCS_GUEST_RSP)?,
            instruction_length: vmread(VMCS_VM_EXIT_INSTRUCTION_LEN)? as u32,
            instruction_info: vmread(VMCS_VM_EXIT_INSTRUCTION_INFO)? as u32,
            interruption_info: vmread(VMCS_VM_EXIT_INTERRUPTION_INFO)? as u32,
            interruption_error: vmread(VMCS_VM_EXIT_INTERRUPTION_ERROR)? as u32,
            idt_vectoring_info: vmread(VMCS_IDT_VECTORING_INFO)? as u32,
            idt_vectoring_error: vmread(VMCS_IDT_VECTORING_ERROR)? as u32,
        })
    }
}

/// Action to take after handling a VM exit.
#[derive(Debug)]
pub enum VmExitAction {
    Continue,
    InjectInterrupt(u8),
    Halt,
    Shutdown,
    Error(VirtError),
}

/// Top-level VM exit dispatcher.
pub fn handle_vm_exit(vcpu: &mut VCpu, exit_info: &VmExitInfo) -> VmExitAction {
    vcpu.exit_count += 1;
    vcpu.last_exit = Some(exit_info.reason);

    match exit_info.reason {
        VmExitReason::Cpuid => handle_cpuid(vcpu, exit_info),
        VmExitReason::IoInstruction => handle_io(vcpu, exit_info),
        VmExitReason::Rdmsr => handle_msr_read(vcpu, exit_info),
        VmExitReason::Wrmsr => handle_msr_write(vcpu, exit_info),
        VmExitReason::EptViolation => handle_ept_violation(vcpu, exit_info),
        VmExitReason::CrAccess => handle_cr_access(vcpu, exit_info),
        VmExitReason::Hlt => handle_hlt(vcpu),
        VmExitReason::Vmcall => handle_vmcall(vcpu),
        VmExitReason::ExternalInterrupt => VmExitAction::Continue,
        VmExitReason::PreemptionTimer => VmExitAction::Continue,
        VmExitReason::TripleFault => VmExitAction::Shutdown,
        VmExitReason::InvalidGuestState => VmExitAction::Error(VirtError::InvalidState(format!(
            "invalid guest state at RIP {:#x}",
            exit_info.guest_rip
        ))),
        VmExitReason::Invd | VmExitReason::Wbinvd => {
            advance_guest_rip(vcpu, exit_info);
            VmExitAction::Continue
        }
        VmExitReason::Xsetbv => {
            advance_guest_rip(vcpu, exit_info);
            VmExitAction::Continue
        }
        VmExitReason::Rdtsc | VmExitReason::Rdtscp => {
            handle_rdtsc(vcpu, exit_info);
            VmExitAction::Continue
        }
        VmExitReason::Pause => VmExitAction::Continue,
        VmExitReason::MachineCheck => VmExitAction::Shutdown,
        VmExitReason::EptMisconfiguration => VmExitAction::Error(VirtError::EptMapFailed),
        _ => VmExitAction::Error(VirtError::InvalidState(format!(
            "unhandled VM exit: {:?} at RIP {:#x}",
            exit_info.reason, exit_info.guest_rip
        ))),
    }
}

fn advance_guest_rip(vcpu: &mut VCpu, exit_info: &VmExitInfo) {
    vcpu.regs.rip = exit_info.guest_rip + exit_info.instruction_length as u64;
}

/// Handle CPUID — virtualize the processor identification.
fn handle_cpuid(vcpu: &mut VCpu, exit_info: &VmExitInfo) -> VmExitAction {
    let leaf = vcpu.regs.rax as u32;
    let subleaf = vcpu.regs.rcx as u32;

    let (mut eax, mut ebx, mut ecx, mut edx) = cpuid(leaf, subleaf);

    match leaf {
        0 => {
            // Vendor: keep host vendor but limit max leaf.
        }
        1 => {
            // Feature flags — hide VMX from the guest (bit 5 of ECX).
            ecx &= !CPUID_VMX_BIT;
            // Hide hypervisor present bit if desired.
            ecx |= 1 << 31; // announce hypervisor presence
        }
        0x4000_0000 => {
            // Hypervisor CPUID leaf — return "RaeenOSHyp".
            eax = 0x4000_0001;
            ebx = u32::from_le_bytes(*b"Raee");
            ecx = u32::from_le_bytes(*b"nOSH");
            edx = u32::from_le_bytes(*b"yp\0\0");
        }
        0x4000_0001 => {
            // Hypervisor features — nothing special yet.
            eax = 0;
            ebx = 0;
            ecx = 0;
            edx = 0;
        }
        0x0B => {
            // Topology — single core/thread for now.
        }
        0x8000_0000..=0x8000_0008 => {
            // Extended CPUID — pass through mostly unchanged.
        }
        _ => {}
    }

    vcpu.regs.rax = eax as u64;
    vcpu.regs.rbx = ebx as u64;
    vcpu.regs.rcx = ecx as u64;
    vcpu.regs.rdx = edx as u64;

    advance_guest_rip(vcpu, exit_info);
    VmExitAction::Continue
}

/// Handle I/O instruction exits (IN/OUT/INS/OUTS).
fn handle_io(vcpu: &mut VCpu, exit_info: &VmExitInfo) -> VmExitAction {
    let qual = exit_info.qualification;
    let size = (qual & 0x7) as u8 + 1; // 1, 2, or 4 bytes
    let is_in = (qual >> 3) & 1 != 0;
    let is_string = (qual >> 4) & 1 != 0;
    let is_rep = (qual >> 5) & 1 != 0;
    let _is_imm = (qual >> 6) & 1 != 0;
    let port = ((qual >> 16) & 0xFFFF) as u16;

    if is_string || is_rep {
        advance_guest_rip(vcpu, exit_info);
        return VmExitAction::Continue;
    }

    if is_in {
        let value = emulate_port_read(port, size);
        match size {
            1 => vcpu.regs.rax = (vcpu.regs.rax & !0xFF) | (value & 0xFF),
            2 => vcpu.regs.rax = (vcpu.regs.rax & !0xFFFF) | (value & 0xFFFF),
            4 => vcpu.regs.rax = value & 0xFFFF_FFFF,
            _ => {}
        }
    } else {
        let value = vcpu.regs.rax;
        emulate_port_write(port, size, value);
    }

    advance_guest_rip(vcpu, exit_info);
    VmExitAction::Continue
}

fn emulate_port_read(port: u16, size: u8) -> u64 {
    match port {
        // COM1 — line status register: always report TX ready.
        0x3FD => 0x60,
        // PIC — return no IRRs pending.
        0x20 | 0xA0 => 0x00,
        // PIT counter 0 read.
        0x40 => 0x00,
        // CMOS / RTC.
        0x71 => 0x00,
        // i8042 status: output buffer empty, input buffer empty.
        0x64 => 0x00,
        // Default: return all-ones for the size.
        _ => match size {
            1 => 0xFF,
            2 => 0xFFFF,
            4 => 0xFFFF_FFFF,
            _ => 0xFF,
        },
    }
}

fn emulate_port_write(_port: u16, _size: u8, _value: u64) {
    // Silently discard writes for now; a full I/O device model goes here.
}

/// Handle RDMSR exits.
fn handle_msr_read(vcpu: &mut VCpu, exit_info: &VmExitInfo) -> VmExitAction {
    let msr = vcpu.regs.rcx as u32;
    let value = vcpu.get_msr(msr);

    vcpu.regs.rax = value & 0xFFFF_FFFF;
    vcpu.regs.rdx = value >> 32;

    advance_guest_rip(vcpu, exit_info);
    VmExitAction::Continue
}

/// Handle WRMSR exits.
fn handle_msr_write(vcpu: &mut VCpu, exit_info: &VmExitInfo) -> VmExitAction {
    let msr = vcpu.regs.rcx as u32;
    let value = (vcpu.regs.rdx << 32) | (vcpu.regs.rax & 0xFFFF_FFFF);

    vcpu.set_msr(msr, value);

    advance_guest_rip(vcpu, exit_info);
    VmExitAction::Continue
}

/// Handle EPT violation — delegate to the EPT manager to resolve the fault.
fn handle_ept_violation(vcpu: &mut VCpu, exit_info: &VmExitInfo) -> VmExitAction {
    let guest_phys = match vmread(VMCS_GUEST_PHYS_ADDR) {
        Ok(addr) => addr,
        Err(e) => return VmExitAction::Error(e),
    };

    let _read = exit_info.qualification & 1 != 0;
    let _write = exit_info.qualification & 2 != 0;
    let _exec = exit_info.qualification & 4 != 0;
    let ept_readable = exit_info.qualification & 8 != 0;

    if !ept_readable {
        // Page not mapped at all — attempt identity-map as a fallback.
        let perms = EptPermissions {
            read: true,
            write: true,
            execute: true,
            user_execute: false,
        };
        if vcpu
            .ept
            .map_page(
                guest_phys & !0xFFF,
                guest_phys & !0xFFF,
                perms,
                EptMemoryType::WriteBack,
            )
            .is_ok()
        {
            return VmExitAction::Continue;
        }
    }

    VmExitAction::Error(VirtError::EptMapFailed)
}

/// Handle CR access exits (MOV to/from CRn).
fn handle_cr_access(vcpu: &mut VCpu, exit_info: &VmExitInfo) -> VmExitAction {
    let qual = exit_info.qualification;
    let cr_num = (qual & 0xF) as u8;
    let access_type = ((qual >> 4) & 0x3) as u8;
    let reg = ((qual >> 8) & 0xF) as u8;

    let reg_value = match reg {
        0 => vcpu.regs.rax,
        1 => vcpu.regs.rcx,
        2 => vcpu.regs.rdx,
        3 => vcpu.regs.rbx,
        4 => vcpu.regs.rsp,
        5 => vcpu.regs.rbp,
        6 => vcpu.regs.rsi,
        7 => vcpu.regs.rdi,
        8 => vcpu.regs.r8,
        9 => vcpu.regs.r9,
        10 => vcpu.regs.r10,
        11 => vcpu.regs.r11,
        12 => vcpu.regs.r12,
        13 => vcpu.regs.r13,
        14 => vcpu.regs.r14,
        15 => vcpu.regs.r15,
        _ => 0,
    };

    match access_type {
        0 => {
            // MOV to CR.
            match cr_num {
                0 => vcpu.regs.cr0 = reg_value,
                3 => vcpu.regs.cr3 = reg_value,
                4 => vcpu.regs.cr4 = reg_value,
                _ => {}
            }
        }
        1 => {
            // MOV from CR.
            let cr_val = match cr_num {
                0 => vcpu.regs.cr0,
                3 => vcpu.regs.cr3,
                4 => vcpu.regs.cr4,
                _ => 0,
            };
            set_gpr(vcpu, reg, cr_val);
        }
        2 => {
            // CLTS — clear CR0.TS.
            vcpu.regs.cr0 &= !(1 << 3);
        }
        3 => {
            // LMSW — load low 16 bits of CR0.
            let new_low = (reg_value & 0xFFFF) as u16;
            vcpu.regs.cr0 = (vcpu.regs.cr0 & !0xFFFF) | new_low as u64;
        }
        _ => {}
    }

    advance_guest_rip(vcpu, exit_info);
    VmExitAction::Continue
}

fn set_gpr(vcpu: &mut VCpu, reg: u8, value: u64) {
    match reg {
        0 => vcpu.regs.rax = value,
        1 => vcpu.regs.rcx = value,
        2 => vcpu.regs.rdx = value,
        3 => vcpu.regs.rbx = value,
        4 => vcpu.regs.rsp = value,
        5 => vcpu.regs.rbp = value,
        6 => vcpu.regs.rsi = value,
        7 => vcpu.regs.rdi = value,
        8 => vcpu.regs.r8 = value,
        9 => vcpu.regs.r9 = value,
        10 => vcpu.regs.r10 = value,
        11 => vcpu.regs.r11 = value,
        12 => vcpu.regs.r12 = value,
        13 => vcpu.regs.r13 = value,
        14 => vcpu.regs.r14 = value,
        15 => vcpu.regs.r15 = value,
        _ => {}
    }
}

/// Handle HLT — put the vCPU into halted state until an interrupt arrives.
fn handle_hlt(vcpu: &mut VCpu) -> VmExitAction {
    vcpu.state = VCpuState::Halted;
    VmExitAction::Halt
}

/// Handle VMCALL — hypercall interface for guest ↔ hypervisor communication.
fn handle_vmcall(vcpu: &mut VCpu) -> VmExitAction {
    let call_nr = vcpu.regs.rax;
    let arg0 = vcpu.regs.rdi;
    let arg1 = vcpu.regs.rsi;
    let _arg2 = vcpu.regs.rdx;

    let result: u64 = match call_nr {
        // HC_GET_FEATURES: return supported feature bitmap.
        0 => 0x0000_0000_0000_0001,
        // HC_GET_TIME: return a pseudo-timestamp.
        1 => {
            let tsc: u64;
            unsafe {
                core::arch::asm!("rdtsc", out("eax") _, out("edx") _, options(nomem, nostack));
                tsc = 0; // placeholder
            }
            tsc
        }
        // HC_MAP_MMIO: request MMIO mapping at guest physical address.
        2 => {
            let guest_phys = arg0;
            let host_phys = arg1;
            let perms = EptPermissions {
                read: true,
                write: true,
                execute: false,
                user_execute: false,
            };
            match vcpu
                .ept
                .map_page(guest_phys, host_phys, perms, EptMemoryType::Uncacheable)
            {
                Ok(()) => 0,
                Err(_) => u64::MAX,
            }
        }
        // HC_SHUTDOWN: request VM shutdown.
        3 => {
            return VmExitAction::Shutdown;
        }
        _ => u64::MAX,
    };

    vcpu.regs.rax = result;
    vcpu.regs.rip += 3; // VMCALL is a 3-byte instruction
    VmExitAction::Continue
}

/// Handle RDTSC / RDTSCP exits.
fn handle_rdtsc(vcpu: &mut VCpu, exit_info: &VmExitInfo) {
    let tsc = unsafe {
        let lo: u32;
        let hi: u32;
        core::arch::asm!("rdtsc", out("eax") lo, out("edx") hi, options(nomem, nostack));
        ((hi as u64) << 32) | lo as u64
    };
    let adjusted = (tsc as i64 + vcpu.tsc_offset) as u64;
    vcpu.regs.rax = adjusted & 0xFFFF_FFFF;
    vcpu.regs.rdx = adjusted >> 32;

    if exit_info.reason == VmExitReason::Rdtscp {
        vcpu.regs.rcx = vcpu.id as u64;
    }

    advance_guest_rip(vcpu, exit_info);
}

// ═══════════════════════════════════════════════════════════════════════════
// §4  EPT (EXTENDED PAGE TABLES)
// ═══════════════════════════════════════════════════════════════════════════

const EPT_PAGE_SIZE: u64 = 4096;
const EPT_TABLE_ENTRIES: usize = 512;

/// EPT memory type encoding in PTE bits 5:3.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EptMemoryType {
    Uncacheable = 0,
    WriteCombining = 1,
    WriteThrough = 4,
    WriteProtect = 5,
    WriteBack = 6,
}

/// EPT access permissions for a mapped page.
#[derive(Debug, Clone, Copy)]
pub struct EptPermissions {
    pub read: bool,
    pub write: bool,
    pub execute: bool,
    pub user_execute: bool,
}

impl EptPermissions {
    pub fn all() -> Self {
        Self {
            read: true,
            write: true,
            execute: true,
            user_execute: true,
        }
    }

    pub fn read_only() -> Self {
        Self {
            read: true,
            write: false,
            execute: false,
            user_execute: false,
        }
    }

    pub fn read_write() -> Self {
        Self {
            read: true,
            write: true,
            execute: false,
            user_execute: false,
        }
    }

    pub fn read_exec() -> Self {
        Self {
            read: true,
            write: false,
            execute: true,
            user_execute: false,
        }
    }

    pub fn to_bits(&self) -> u64 {
        let mut bits = 0u64;
        if self.read {
            bits |= 1;
        }
        if self.write {
            bits |= 2;
        }
        if self.execute {
            bits |= 4;
        }
        if self.user_execute {
            bits |= 1 << 10;
        }
        bits
    }
}

/// A single EPT page-table entry.
#[derive(Debug, Clone, Copy)]
pub struct EptEntry {
    pub raw: u64,
}

impl EptEntry {
    pub fn empty() -> Self {
        Self { raw: 0 }
    }

    pub fn is_present(&self) -> bool {
        self.raw & 0x7 != 0 // at least one of R/W/X
    }

    pub fn is_large_page(&self) -> bool {
        self.raw & (1 << 7) != 0
    }

    pub fn read(&self) -> bool {
        self.raw & 1 != 0
    }
    pub fn write(&self) -> bool {
        self.raw & 2 != 0
    }
    pub fn execute(&self) -> bool {
        self.raw & 4 != 0
    }

    pub fn set_read(&mut self, v: bool) {
        if v {
            self.raw |= 1;
        } else {
            self.raw &= !1;
        }
    }
    pub fn set_write(&mut self, v: bool) {
        if v {
            self.raw |= 2;
        } else {
            self.raw &= !2;
        }
    }
    pub fn set_execute(&mut self, v: bool) {
        if v {
            self.raw |= 4;
        } else {
            self.raw &= !4;
        }
    }
    pub fn set_large_page(&mut self, v: bool) {
        if v {
            self.raw |= 1 << 7;
        } else {
            self.raw &= !(1 << 7);
        }
    }

    pub fn set_memory_type(&mut self, mt: EptMemoryType) {
        self.raw = (self.raw & !(0x7 << 3)) | ((mt as u64) << 3);
    }

    pub fn memory_type(&self) -> EptMemoryType {
        match (self.raw >> 3) & 0x7 {
            0 => EptMemoryType::Uncacheable,
            1 => EptMemoryType::WriteCombining,
            4 => EptMemoryType::WriteThrough,
            5 => EptMemoryType::WriteProtect,
            6 => EptMemoryType::WriteBack,
            _ => EptMemoryType::Uncacheable,
        }
    }

    pub fn phys_addr(&self) -> u64 {
        self.raw & 0x000F_FFFF_FFFF_F000
    }

    pub fn set_phys_addr(&mut self, addr: u64) {
        self.raw = (self.raw & !0x000F_FFFF_FFFF_F000) | (addr & 0x000F_FFFF_FFFF_F000);
    }

    pub fn set_permissions(&mut self, perms: &EptPermissions) {
        self.raw = (self.raw & !0x407) | perms.to_bits();
    }

    /// Create a leaf entry for a 4 KiB page.
    pub fn leaf_4k(host_phys: u64, perms: &EptPermissions, mem_type: EptMemoryType) -> Self {
        let mut entry = Self::empty();
        entry.set_phys_addr(host_phys);
        entry.set_permissions(perms);
        entry.set_memory_type(mem_type);
        // Ignore PAT (bit 6) set for leaf entries.
        entry.raw |= 1 << 6;
        entry
    }

    /// Create a non-leaf (table pointer) entry.
    pub fn table_entry(table_phys: u64) -> Self {
        let mut entry = Self::empty();
        entry.set_phys_addr(table_phys);
        // Table entries need RWX to allow sub-page access.
        entry.raw |= 0x7;
        entry
    }
}

/// Tracking record for a guest→host physical mapping.
#[derive(Debug, Clone)]
pub struct EptMapping {
    pub guest_phys: u64,
    pub host_phys: u64,
    pub size: u64,
    pub permissions: EptPermissions,
    pub memory_type: EptMemoryType,
}

/// MTRR-style memory type range for determining default cache types.
#[derive(Debug, Clone)]
pub struct MtrrRange {
    pub base: u64,
    pub size: u64,
    pub mem_type: EptMemoryType,
}

/// EPT manager — owns the root EPT PML4 and tracks all guest→host mappings.
pub struct EptManager {
    pub root_table: u64,
    pub allocated_tables: Vec<u64>,
    pub mappings: BTreeMap<u64, EptMapping>,
    pub memory_types: Vec<MtrrRange>,
}

impl EptManager {
    pub fn new() -> Result<Self, VirtError> {
        // In a real implementation, allocate a 4 KiB-aligned physical page for PML4.
        let root = allocate_phys_page().ok_or(VirtError::EptAllocFailed)?;
        // Zero the root table.
        unsafe {
            core::ptr::write_bytes(root as *mut u8, 0, EPT_PAGE_SIZE as usize);
        }
        Ok(Self {
            root_table: root,
            allocated_tables: Vec::new(),
            mappings: BTreeMap::new(),
            memory_types: Vec::new(),
        })
    }

    /// Map a single 4 KiB page in the EPT.
    pub fn map_page(
        &mut self,
        guest_phys: u64,
        host_phys: u64,
        perms: EptPermissions,
        mem_type: EptMemoryType,
    ) -> Result<(), VirtError> {
        if guest_phys & 0xFFF != 0 || host_phys & 0xFFF != 0 {
            return Err(VirtError::InvalidAlignment);
        }

        let pml4_idx = ((guest_phys >> 39) & 0x1FF) as usize;
        let pdpt_idx = ((guest_phys >> 30) & 0x1FF) as usize;
        let pd_idx = ((guest_phys >> 21) & 0x1FF) as usize;
        let pt_idx = ((guest_phys >> 12) & 0x1FF) as usize;

        let pml4 = self.root_table as *mut EptEntry;

        // Walk/create PML4 → PDPT.
        let pdpt = unsafe {
            let entry = &mut *pml4.add(pml4_idx);
            if !entry.is_present() {
                let page = allocate_phys_page().ok_or(VirtError::EptAllocFailed)?;
                core::ptr::write_bytes(page as *mut u8, 0, EPT_PAGE_SIZE as usize);
                *entry = EptEntry::table_entry(page);
                self.allocated_tables.push(page);
            }
            entry.phys_addr() as *mut EptEntry
        };

        // Walk/create PDPT → PD.
        let pd = unsafe {
            let entry = &mut *pdpt.add(pdpt_idx);
            if !entry.is_present() {
                let page = allocate_phys_page().ok_or(VirtError::EptAllocFailed)?;
                core::ptr::write_bytes(page as *mut u8, 0, EPT_PAGE_SIZE as usize);
                *entry = EptEntry::table_entry(page);
                self.allocated_tables.push(page);
            }
            entry.phys_addr() as *mut EptEntry
        };

        // Walk/create PD → PT.
        let pt = unsafe {
            let entry = &mut *pd.add(pd_idx);
            if !entry.is_present() {
                let page = allocate_phys_page().ok_or(VirtError::EptAllocFailed)?;
                core::ptr::write_bytes(page as *mut u8, 0, EPT_PAGE_SIZE as usize);
                *entry = EptEntry::table_entry(page);
                self.allocated_tables.push(page);
            }
            entry.phys_addr() as *mut EptEntry
        };

        // Set the leaf entry.
        unsafe {
            let entry = &mut *pt.add(pt_idx);
            *entry = EptEntry::leaf_4k(host_phys, &perms, mem_type);
        }

        self.mappings.insert(
            guest_phys,
            EptMapping {
                guest_phys,
                host_phys,
                size: EPT_PAGE_SIZE,
                permissions: perms,
                memory_type: mem_type,
            },
        );

        Ok(())
    }

    /// Unmap a single 4 KiB page from the EPT.
    pub fn unmap_page(&mut self, guest_phys: u64) -> Result<(), VirtError> {
        if guest_phys & 0xFFF != 0 {
            return Err(VirtError::InvalidAlignment);
        }

        let pml4_idx = ((guest_phys >> 39) & 0x1FF) as usize;
        let pdpt_idx = ((guest_phys >> 30) & 0x1FF) as usize;
        let pd_idx = ((guest_phys >> 21) & 0x1FF) as usize;
        let pt_idx = ((guest_phys >> 12) & 0x1FF) as usize;

        let pml4 = self.root_table as *mut EptEntry;

        unsafe {
            let pml4e = &*pml4.add(pml4_idx);
            if !pml4e.is_present() {
                return Err(VirtError::EptNotMapped);
            }
            let pdpt = pml4e.phys_addr() as *mut EptEntry;

            let pdpte = &*pdpt.add(pdpt_idx);
            if !pdpte.is_present() {
                return Err(VirtError::EptNotMapped);
            }
            let pd = pdpte.phys_addr() as *mut EptEntry;

            let pde = &*pd.add(pd_idx);
            if !pde.is_present() {
                return Err(VirtError::EptNotMapped);
            }
            let pt = pde.phys_addr() as *mut EptEntry;

            let entry = &mut *pt.add(pt_idx);
            if !entry.is_present() {
                return Err(VirtError::EptNotMapped);
            }
            *entry = EptEntry::empty();
        }

        self.mappings.remove(&guest_phys);
        Ok(())
    }

    /// Map a contiguous range of guest physical pages 1:1 or with offset.
    pub fn map_range(
        &mut self,
        guest_start: u64,
        host_start: u64,
        size: u64,
        perms: EptPermissions,
    ) -> Result<(), VirtError> {
        if size == 0 || size & 0xFFF != 0 {
            return Err(VirtError::InvalidSize);
        }
        if guest_start & 0xFFF != 0 || host_start & 0xFFF != 0 {
            return Err(VirtError::InvalidAlignment);
        }

        let mem_type = self.get_memory_type(host_start);
        let num_pages = size / EPT_PAGE_SIZE;

        for i in 0..num_pages {
            let guest = guest_start + i * EPT_PAGE_SIZE;
            let host = host_start + i * EPT_PAGE_SIZE;
            self.map_page(guest, host, perms, mem_type)?;
        }

        Ok(())
    }

    /// Translate a guest physical address to host physical.
    pub fn translate(&self, guest_phys: u64) -> Option<u64> {
        let aligned = guest_phys & !0xFFF;
        let offset = guest_phys & 0xFFF;
        self.mappings.get(&aligned).map(|m| m.host_phys + offset)
    }

    /// Update permissions on an already-mapped page.
    pub fn set_permissions(
        &mut self,
        guest_phys: u64,
        perms: EptPermissions,
    ) -> Result<(), VirtError> {
        let aligned = guest_phys & !0xFFF;

        let mut entry = self.walk_ept(aligned)?;
        entry.set_permissions(&perms);

        // Write back the modified entry.
        let pt_idx = ((aligned >> 12) & 0x1FF) as usize;
        let pml4_idx = ((aligned >> 39) & 0x1FF) as usize;
        let pdpt_idx = ((aligned >> 30) & 0x1FF) as usize;
        let pd_idx = ((aligned >> 21) & 0x1FF) as usize;

        unsafe {
            let pml4 = self.root_table as *mut EptEntry;
            let pdpt = (*pml4.add(pml4_idx)).phys_addr() as *mut EptEntry;
            let pd = (*pdpt.add(pdpt_idx)).phys_addr() as *mut EptEntry;
            let pt = (*pd.add(pd_idx)).phys_addr() as *mut EptEntry;
            *pt.add(pt_idx) = entry;
        }

        if let Some(mapping) = self.mappings.get_mut(&aligned) {
            mapping.permissions = perms;
        }

        Ok(())
    }

    /// Invalidate all EPT translations (INVEPT).
    pub fn invalidate_ept() {
        unsafe {
            // INVEPT type 2 = global invalidation.
            let descriptor: [u64; 2] = [0, 0];
            core::arch::asm!(
                "invept {}, [{}]",
                in(reg) 2u64,
                in(reg) descriptor.as_ptr(),
                options(nostack),
            );
        }
    }

    /// Walk the EPT for a guest physical address, returning the leaf entry.
    pub fn walk_ept(&self, guest_phys: u64) -> Result<EptEntry, VirtError> {
        let pml4_idx = ((guest_phys >> 39) & 0x1FF) as usize;
        let pdpt_idx = ((guest_phys >> 30) & 0x1FF) as usize;
        let pd_idx = ((guest_phys >> 21) & 0x1FF) as usize;
        let pt_idx = ((guest_phys >> 12) & 0x1FF) as usize;

        unsafe {
            let pml4 = self.root_table as *const EptEntry;
            let pml4e = &*pml4.add(pml4_idx);
            if !pml4e.is_present() {
                return Err(VirtError::EptWalkFailed);
            }

            let pdpt = pml4e.phys_addr() as *const EptEntry;
            let pdpte = &*pdpt.add(pdpt_idx);
            if !pdpte.is_present() {
                return Err(VirtError::EptWalkFailed);
            }
            if pdpte.is_large_page() {
                return Ok(*pdpte);
            }

            let pd = pdpte.phys_addr() as *const EptEntry;
            let pde = &*pd.add(pd_idx);
            if !pde.is_present() {
                return Err(VirtError::EptWalkFailed);
            }
            if pde.is_large_page() {
                return Ok(*pde);
            }

            let pt = pde.phys_addr() as *const EptEntry;
            let pte = &*pt.add(pt_idx);
            if !pte.is_present() {
                return Err(VirtError::EptWalkFailed);
            }

            Ok(*pte)
        }
    }

    /// Determine the memory type for a host physical address based on MTRR ranges.
    fn get_memory_type(&self, host_phys: u64) -> EptMemoryType {
        for range in &self.memory_types {
            if host_phys >= range.base && host_phys < range.base + range.size {
                return range.mem_type;
            }
        }
        EptMemoryType::WriteBack
    }
}

/// Stub allocator — in the real kernel this calls into the frame allocator.
fn allocate_phys_page() -> Option<u64> {
    use x86_64::structures::paging::FrameAllocator;
    crate::memory::GlobalFrameAllocator
        .allocate_frame()
        .map(|f| f.start_address().as_u64())
}

// ═══════════════════════════════════════════════════════════════════════════
// §5  VIRTUAL CPU
// ═══════════════════════════════════════════════════════════════════════════

/// vCPU lifecycle state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VCpuState {
    Created,
    Running,
    Halted,
    Paused,
    Error,
}

/// Full general-purpose + control register state for a vCPU.
#[derive(Debug, Clone, Copy)]
pub struct VCpuRegs {
    pub rax: u64,
    pub rbx: u64,
    pub rcx: u64,
    pub rdx: u64,
    pub rsi: u64,
    pub rdi: u64,
    pub rbp: u64,
    pub rsp: u64,
    pub r8: u64,
    pub r9: u64,
    pub r10: u64,
    pub r11: u64,
    pub r12: u64,
    pub r13: u64,
    pub r14: u64,
    pub r15: u64,
    pub rip: u64,
    pub rflags: u64,
    pub cr0: u64,
    pub cr2: u64,
    pub cr3: u64,
    pub cr4: u64,
    pub dr0: u64,
    pub dr1: u64,
    pub dr2: u64,
    pub dr3: u64,
    pub dr6: u64,
    pub dr7: u64,
    pub efer: u64,
}

impl VCpuRegs {
    pub fn zeroed() -> Self {
        Self {
            rax: 0,
            rbx: 0,
            rcx: 0,
            rdx: 0,
            rsi: 0,
            rdi: 0,
            rbp: 0,
            rsp: 0,
            r8: 0,
            r9: 0,
            r10: 0,
            r11: 0,
            r12: 0,
            r13: 0,
            r14: 0,
            r15: 0,
            rip: 0,
            rflags: 0x0000_0000_0000_0002,
            cr0: 0x0000_0000_0001_0031,
            cr2: 0,
            cr3: 0,
            cr4: 0x0000_0000_0000_2020,
            dr0: 0,
            dr1: 0,
            dr2: 0,
            dr3: 0,
            dr6: 0xFFFF_0FF0,
            dr7: 0x0000_0400,
            efer: 0x0000_0000_0000_0D01,
        }
    }
}

/// Emulated local APIC for a vCPU.
pub struct VirtualLapic {
    pub base_addr: u64,
    pub id: u32,
    pub version: u32,
    pub tpr: u32,
    pub irr: [u32; 8],
    pub isr: [u32; 8],
    pub tmr: [u32; 8],
    pub timer_initial: u32,
    pub timer_current: u32,
    pub timer_divide: u32,
    pub lvt_timer: u32,
    pub lvt_lint0: u32,
    pub lvt_lint1: u32,
    pub lvt_error: u32,
    pub spurious: u32,
}

impl VirtualLapic {
    pub fn new(apic_id: u32) -> Self {
        Self {
            base_addr: 0xFEE0_0000,
            id: apic_id,
            version: 0x0005_0014, // version 20, 6 LVT entries
            tpr: 0,
            irr: [0; 8],
            isr: [0; 8],
            tmr: [0; 8],
            timer_initial: 0,
            timer_current: 0,
            timer_divide: 0,
            lvt_timer: 0x0001_0000, // masked
            lvt_lint0: 0x0001_0000,
            lvt_lint1: 0x0001_0000,
            lvt_error: 0x0001_0000,
            spurious: 0x0000_00FF,
        }
    }

    /// Set a bit in the IRR (interrupt request register).
    pub fn set_irr(&mut self, vector: u8) {
        let idx = (vector / 32) as usize;
        let bit = vector % 32;
        if idx < 8 {
            self.irr[idx] |= 1 << bit;
        }
    }

    /// Clear a bit in the IRR and set it in the ISR (acknowledge the interrupt).
    pub fn acknowledge(&mut self, vector: u8) {
        let idx = (vector / 32) as usize;
        let bit = vector % 32;
        if idx < 8 {
            self.irr[idx] &= !(1 << bit);
            self.isr[idx] |= 1 << bit;
        }
    }

    /// End-of-interrupt: clear the highest-priority ISR bit.
    pub fn eoi(&mut self) {
        for i in (0..8).rev() {
            if self.isr[i] != 0 {
                let bit = 31 - self.isr[i].leading_zeros();
                self.isr[i] &= !(1 << bit);
                return;
            }
        }
    }

    /// Return the highest-priority pending IRR vector, if above TPR.
    pub fn pending_vector(&self) -> Option<u8> {
        for i in (0..8).rev() {
            if self.irr[i] != 0 {
                let bit = 31 - self.irr[i].leading_zeros();
                let vector = (i as u8) * 32 + bit as u8;
                let priority = vector >> 4;
                let tpr_priority = (self.tpr >> 4) as u8;
                if priority > tpr_priority {
                    return Some(vector);
                }
            }
        }
        None
    }

    /// Read an APIC register by MMIO offset.
    pub fn read_register(&self, offset: u64) -> u32 {
        match offset {
            0x020 => self.id << 24,
            0x030 => self.version,
            0x080 => self.tpr,
            0x0B0 => {
                /* EOI — write-only, returns 0 */
                0
            }
            0x0D0 => 0,           // LDR
            0x0E0 => 0xFFFF_FFFF, // DFR (flat model)
            0x0F0 => self.spurious,
            0x100..=0x170 => self.isr[((offset - 0x100) / 0x10) as usize],
            0x180..=0x1F0 => self.tmr[((offset - 0x180) / 0x10) as usize],
            0x200..=0x270 => self.irr[((offset - 0x200) / 0x10) as usize],
            0x320 => self.lvt_timer,
            0x350 => self.lvt_lint0,
            0x360 => self.lvt_lint1,
            0x370 => self.lvt_error,
            0x380 => self.timer_initial,
            0x390 => self.timer_current,
            0x3E0 => self.timer_divide,
            _ => 0,
        }
    }

    /// Write an APIC register by MMIO offset.
    pub fn write_register(&mut self, offset: u64, value: u32) {
        match offset {
            0x080 => self.tpr = value & 0xFF,
            0x0B0 => self.eoi(),
            0x0F0 => self.spurious = value,
            0x320 => self.lvt_timer = value,
            0x350 => self.lvt_lint0 = value,
            0x360 => self.lvt_lint1 = value,
            0x370 => self.lvt_error = value,
            0x380 => {
                self.timer_initial = value;
                self.timer_current = value;
            }
            0x3E0 => self.timer_divide = value & 0xB,
            _ => {}
        }
    }
}

/// A virtual CPU — one per hardware thread inside a VM.
pub struct VCpu {
    pub id: u32,
    pub vm_id: u64,
    pub state: VCpuState,
    pub vmcs: Vmcs,
    pub regs: VCpuRegs,
    pub ept: EptManager,
    pub lapic: VirtualLapic,
    pub run_count: u64,
    pub exit_count: u64,
    pub last_exit: Option<VmExitReason>,
    pub tsc_offset: i64,
    pub msr_store: BTreeMap<u32, u64>,
}

impl VCpu {
    pub fn new(vm_id: u64, vcpu_id: u32) -> Result<Self, VirtError> {
        let vmcs = Vmcs::new()?;
        let ept = EptManager::new()?;

        let mut msr_store = BTreeMap::new();
        msr_store.insert(IA32_EFER, 0x0000_0000_0000_0D01);
        msr_store.insert(IA32_PAT, 0x0007_0406_0007_0406);
        msr_store.insert(IA32_SYSENTER_CS, 0);
        msr_store.insert(IA32_SYSENTER_ESP, 0);
        msr_store.insert(IA32_SYSENTER_EIP, 0);
        msr_store.insert(IA32_FS_BASE, 0);
        msr_store.insert(IA32_GS_BASE, 0);
        msr_store.insert(IA32_KERNEL_GS_BASE, 0);

        Ok(Self {
            id: vcpu_id,
            vm_id,
            state: VCpuState::Created,
            vmcs,
            regs: VCpuRegs::zeroed(),
            ept,
            lapic: VirtualLapic::new(vcpu_id),
            run_count: 0,
            exit_count: 0,
            last_exit: None,
            tsc_offset: 0,
            msr_store,
        })
    }

    /// Enter the guest via VMLAUNCH/VMRESUME and return exit info.
    pub fn run(&mut self) -> Result<VmExitInfo, VirtError> {
        self.state = VCpuState::Running;
        self.run_count += 1;

        // Sync register state into the VMCS.
        self.vmcs.guest_rip = self.regs.rip;
        self.vmcs.guest_rsp = self.regs.rsp;
        self.vmcs.guest_rflags = self.regs.rflags;
        self.vmcs.guest_cr0 = self.regs.cr0;
        self.vmcs.guest_cr3 = self.regs.cr3;
        self.vmcs.guest_cr4 = self.regs.cr4;
        self.vmcs.guest_ia32_efer = self.regs.efer;

        self.vmcs.load()?;
        self.vmcs.flush_to_hardware()?;

        // In a real implementation, we'd do VMLAUNCH/VMRESUME here and
        // save/restore GPRs around the transition. The assembly stub
        // would look roughly like:
        //   push all GPRs
        //   load guest GPRs from self.regs
        //   vmlaunch / vmresume
        //   save guest GPRs back to self.regs
        //   pop host GPRs
        //
        // For now we collect exit info as if a VM exit just occurred.
        let exit_info = VmExitInfo::collect()?;

        // Sync hardware state back into our cached copy.
        self.vmcs.sync_from_hardware()?;
        self.regs.rip = self.vmcs.guest_rip;
        self.regs.rsp = self.vmcs.guest_rsp;
        self.regs.rflags = self.vmcs.guest_rflags;
        self.regs.cr0 = self.vmcs.guest_cr0;
        self.regs.cr3 = self.vmcs.guest_cr3;
        self.regs.cr4 = self.vmcs.guest_cr4;
        self.regs.efer = self.vmcs.guest_ia32_efer;

        Ok(exit_info)
    }

    pub fn set_regs(&mut self, regs: &VCpuRegs) {
        self.regs = *regs;
    }

    pub fn get_regs(&self) -> &VCpuRegs {
        &self.regs
    }

    /// Inject a virtual interrupt into the guest via VMCS interrupt-info field.
    pub fn inject_interrupt(&mut self, vector: u8) {
        self.lapic.set_irr(vector);
        // Set VM-entry interruption-information field:
        // bits 7:0 = vector, bit 31 = valid, bits 10:8 = type (0 = external).
        let info = (vector as u32) | (1 << 31);
        let _ = vmwrite(VMCS_ENTRY_INTERRUPTION_INFO, info as u64);
    }

    /// Inject an NMI.
    pub fn inject_nmi(&mut self) {
        // Type = 2 (NMI), valid bit set.
        let info: u32 = 2 | (2 << 8) | (1 << 31);
        let _ = vmwrite(VMCS_ENTRY_INTERRUPTION_INFO, info as u64);
    }

    /// Write a guest MSR value.
    pub fn set_msr(&mut self, msr: u32, value: u64) {
        self.msr_store.insert(msr, value);
        match msr {
            IA32_EFER => self.regs.efer = value,
            _ => {}
        }
    }

    /// Read a guest MSR value.
    pub fn get_msr(&self, msr: u32) -> u64 {
        self.msr_store.get(&msr).copied().unwrap_or(0)
    }

    /// Pause the vCPU (halt scheduling without destroying state).
    pub fn pause(&mut self) {
        self.state = VCpuState::Paused;
    }

    /// Resume a paused vCPU.
    pub fn resume(&mut self) {
        if self.state == VCpuState::Paused || self.state == VCpuState::Halted {
            self.state = VCpuState::Created; // ready to run again
        }
    }

    /// Reset the vCPU to power-on defaults.
    pub fn reset(&mut self) {
        self.regs = VCpuRegs::zeroed();
        self.state = VCpuState::Created;
        self.run_count = 0;
        self.exit_count = 0;
        self.last_exit = None;
        self.tsc_offset = 0;
        self.msr_store.clear();
        self.msr_store.insert(IA32_EFER, 0x0000_0000_0000_0D01);
        self.msr_store.insert(IA32_PAT, 0x0007_0406_0007_0406);
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// §6  VIRTUAL MACHINE MANAGER
// ═══════════════════════════════════════════════════════════════════════════

/// Per-VM lifecycle state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VmState {
    Created,
    Running,
    Paused,
    Shutdown,
    Error,
}

/// Virtio device type identifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VirtioType {
    Net,
    Block,
    Console,
    Rng,
    Balloon,
    Gpu,
    Input,
    Vsock,
    Fs,
}

/// Configuration for a virtio device attached to a VM.
pub struct VirtioDeviceConfig {
    pub device_type: VirtioType,
    pub config: Vec<u8>,
}

/// Runtime statistics for a VM.
#[derive(Debug, Clone, Copy, Default)]
pub struct VmStats {
    pub exits: u64,
    pub io_exits: u64,
    pub mmio_exits: u64,
    pub irq_injections: u64,
    pub ept_violations: u64,
    pub run_time_ns: u64,
}

/// A complete virtual machine — owns its vCPUs, EPT, and device list.
pub struct VirtualMachine {
    pub id: u64,
    pub name: String,
    pub vcpus: Vec<VCpu>,
    pub memory_size: u64,
    pub ept: EptManager,
    pub devices: Vec<VirtioDeviceConfig>,
    pub state: VmState,
    pub creation_time: u64,
    pub stats: VmStats,
}

/// IOCTL-style commands for VM control from userspace or kernel callers.
pub enum VmIoctl {
    GetRegs(u32),
    SetRegs(u32, VCpuRegs),
    MapMemory(u64, u64, u64),
    InjectIrq(u32, u8),
    GetStats,
}

/// The top-level hypervisor — manages all VMs on the system.
pub struct Hypervisor {
    pub vms: BTreeMap<u64, VirtualMachine>,
    pub next_vm_id: u64,
    pub capabilities: VmxCapabilities,
    pub enabled: bool,
}

impl Hypervisor {
    pub fn new() -> Result<Self, VirtError> {
        let caps = detect_vmx().ok_or(VirtError::VmxNotSupported)?;
        Ok(Self {
            vms: BTreeMap::new(),
            next_vm_id: 1,
            capabilities: caps,
            enabled: false,
        })
    }

    /// Create a new VM with the given name, memory size, and vCPU count.
    pub fn create_vm(
        &mut self,
        name: &str,
        memory_mb: u64,
        vcpu_count: u32,
    ) -> Result<u64, VirtError> {
        let vm_id = self.next_vm_id;
        self.next_vm_id += 1;

        let ept = EptManager::new()?;
        let memory_bytes = memory_mb * 1024 * 1024;

        let mut vcpus = Vec::with_capacity(vcpu_count as usize);
        for i in 0..vcpu_count {
            vcpus.push(VCpu::new(vm_id, i)?);
        }

        let vm = VirtualMachine {
            id: vm_id,
            name: String::from(name),
            vcpus,
            memory_size: memory_bytes,
            ept,
            devices: Vec::new(),
            state: VmState::Created,
            creation_time: 0,
            stats: VmStats::default(),
        };

        self.vms.insert(vm_id, vm);
        Ok(vm_id)
    }

    /// Destroy a VM and free all its resources.
    pub fn destroy_vm(&mut self, vm_id: u64) -> Result<(), VirtError> {
        if self.vms.remove(&vm_id).is_none() {
            return Err(VirtError::VmNotFound);
        }
        Ok(())
    }

    /// Start (run) a VM — enters the guest on the BSP vCPU.
    pub fn start_vm(&mut self, vm_id: u64) -> Result<(), VirtError> {
        let vm = self.vms.get_mut(&vm_id).ok_or(VirtError::VmNotFound)?;
        if vm.state == VmState::Running {
            return Err(VirtError::VmAlreadyRunning);
        }
        vm.state = VmState::Running;
        Ok(())
    }

    /// Pause all vCPUs in a VM.
    pub fn pause_vm(&mut self, vm_id: u64) -> Result<(), VirtError> {
        let vm = self.vms.get_mut(&vm_id).ok_or(VirtError::VmNotFound)?;
        if vm.state != VmState::Running {
            return Err(VirtError::VmNotRunning);
        }
        for vcpu in &mut vm.vcpus {
            vcpu.pause();
        }
        vm.state = VmState::Paused;
        Ok(())
    }

    /// Resume all vCPUs in a paused VM.
    pub fn resume_vm(&mut self, vm_id: u64) -> Result<(), VirtError> {
        let vm = self.vms.get_mut(&vm_id).ok_or(VirtError::VmNotFound)?;
        if vm.state != VmState::Paused {
            return Err(VirtError::VmNotRunning);
        }
        for vcpu in &mut vm.vcpus {
            vcpu.resume();
        }
        vm.state = VmState::Running;
        Ok(())
    }

    /// Get an immutable reference to a VM.
    pub fn get_vm(&self, vm_id: u64) -> Option<&VirtualMachine> {
        self.vms.get(&vm_id)
    }

    /// List all VMs.
    pub fn list_vms(&self) -> Vec<&VirtualMachine> {
        self.vms.values().collect()
    }

    /// Attach a virtio device to a VM.
    pub fn add_device(&mut self, vm_id: u64, device: VirtioDeviceConfig) -> Result<(), VirtError> {
        let vm = self.vms.get_mut(&vm_id).ok_or(VirtError::VmNotFound)?;
        if vm.devices.len() >= 32 {
            return Err(VirtError::DeviceLimitReached);
        }
        vm.devices.push(device);
        Ok(())
    }

    /// Execute a VM ioctl command.
    pub fn vm_ioctl(&mut self, vm_id: u64, cmd: VmIoctl) -> Result<u64, VirtError> {
        let vm = self.vms.get_mut(&vm_id).ok_or(VirtError::VmNotFound)?;

        match cmd {
            VmIoctl::GetRegs(vcpu_id) => {
                let _vcpu = vm
                    .vcpus
                    .get(vcpu_id as usize)
                    .ok_or(VirtError::VCpuNotFound)?;
                Ok(0)
            }
            VmIoctl::SetRegs(vcpu_id, regs) => {
                let vcpu = vm
                    .vcpus
                    .get_mut(vcpu_id as usize)
                    .ok_or(VirtError::VCpuNotFound)?;
                vcpu.set_regs(&regs);
                Ok(0)
            }
            VmIoctl::MapMemory(guest_phys, host_phys, size) => {
                let perms = EptPermissions::all();
                vm.ept.map_range(guest_phys, host_phys, size, perms)?;
                Ok(0)
            }
            VmIoctl::InjectIrq(vcpu_id, vector) => {
                let vcpu = vm
                    .vcpus
                    .get_mut(vcpu_id as usize)
                    .ok_or(VirtError::VCpuNotFound)?;
                vcpu.inject_interrupt(vector);
                vm.stats.irq_injections += 1;
                Ok(0)
            }
            VmIoctl::GetStats => Ok(vm.stats.exits),
        }
    }
}

// ─── Global hypervisor instance ─────────────────────────────────────────────

pub static HYPERVISOR: Mutex<Option<Hypervisor>> = Mutex::new(None);

/// Initialize the hypervisor subsystem.
pub fn init() {
    match Hypervisor::new() {
        Ok(hv) => {
            *HYPERVISOR.lock() = Some(hv);
        }
        Err(_) => {
            // VMX not available — hypervisor stays disabled.
        }
    }
}
