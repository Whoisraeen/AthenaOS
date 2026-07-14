use crate::arch::VirtAddr;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use x86_64::registers::model_specific::{Efer, EferFlags, LStar, SFMask, Star};

// ── Interface contract guard ─────────────────────────────────────────────────
// The syscall dispatch below MUST agree with the frozen ABI in `ath_abi`. These
// compile-time assertions make any drift a build failure, so a subsystem agent
// can never quietly fork a syscall number. Changing these requires an Opus
// `[interface]` commit that edits ath_abi and this dispatch together.
const _: () = {
    use ath_abi::syscall as abi;
    assert!(abi::SYS_DRIVER_CLAIM_DEVICE == 111);
    assert!(abi::SYS_DRIVER_DMA_UNMAP == 118);
    assert!(abi::SYS_ATHENA_SHUTDOWN == 120);
    assert!(abi::SYS_OOM_SUBSCRIBE == 100);
    assert!(abi::SYS_ATHFS_SNAPSHOT_CREATE == 101);
    assert!(abi::SYS_ATHFS_SNAPSHOT_RESTORE == 102);
    assert!(abi::SYS_ATHFS_SNAPSHOT_DELETE == 103);
    assert!(abi::SYS_LINUXKPI_VERSION == 127);
    assert!(abi::SYS_LINUXKPI_SUPERVISOR == 140);
    assert!(abi::SYS_DEBUG_PRINT == 141);
    assert!(abi::SYS_LINUXKPI_REQUEST_FIRMWARE == 142);
    assert!(abi::SYS_INSTALL_RUN == 256);
    assert!(abi::SYS_INSTALL_CREATE_ACCOUNT == 257);
    assert!(abi::SYS_SEARCH_QUERY_RESOLVED == 281);
    // Anti-cheat block relocated to 284–290 (was the colliding 100–106). The
    // dispatch range arm below MUST match these.
    assert!(abi::SYS_AC_REQUEST_ATTESTATION == 284);
    assert!(abi::SYS_AC_HEARTBEAT == 290);
    assert!(ath_abi::ABI_VERSION == 4);
};

pub fn init() {
    let selectors = crate::gdt::GDT.1;

    unsafe {
        Efer::update(|efer| efer.insert(EferFlags::SYSTEM_CALL_EXTENSIONS));
    }
    crate::serial_println!(
        "BSP GDT INDICES: u32cs={}, uds={}, ucs={}, kcs={}, kds={}",
        selectors.user32_code_selector.index(),
        selectors.user_data_selector.index(),
        selectors.user_code_selector.index(),
        selectors.kernel_code_selector.index(),
        selectors.kernel_data_selector.index()
    );
    Star::write(
        selectors.user_code_selector, // 64-bit code (must be index 5)
        selectors.user_data_selector, // 64-bit data (must be index 4)
        selectors.kernel_code_selector,
        selectors.kernel_data_selector,
    )
    .expect("Failed to write STAR MSR. Check GDT layout.");

    LStar::write(VirtAddr::new(syscall_handler as *const () as usize as u64));

    use x86_64::registers::rflags::RFlags;
    SFMask::write(RFlags::INTERRUPT_FLAG | RFlags::ALIGNMENT_CHECK); // AC cleared on entry: ring 3 must not be able to pre-open the SMAP window

    // BSP is cpu_id 0.
    init_percpu_syscall(0);
}

/// Configure syscall MSRs and per-CPU data on an Application Processor.
pub fn init_ap(cpu_id: usize) {
    unsafe {
        Efer::update(|efer| efer.insert(EferFlags::SYSTEM_CALL_EXTENSIONS));
    }

    let selectors = crate::gdt::GDT.1;
    crate::serial_println!(
        "AP GDT INDICES: u32cs={}, uds={}, ucs={}, kcs={}, kds={}",
        selectors.user32_code_selector.index(),
        selectors.user_data_selector.index(),
        selectors.user_code_selector.index(),
        selectors.kernel_code_selector.index(),
        selectors.kernel_data_selector.index()
    );
    Star::write(
        selectors.user_code_selector,
        selectors.user_data_selector,
        selectors.kernel_code_selector,
        selectors.kernel_data_selector,
    )
    .expect("Failed to write STAR MSR on AP");

    LStar::write(VirtAddr::new(syscall_handler as *const () as usize as u64));

    use x86_64::registers::rflags::RFlags;
    SFMask::write(RFlags::INTERRUPT_FLAG | RFlags::ALIGNMENT_CHECK); // AC cleared on entry: ring 3 must not be able to pre-open the SMAP window

    init_percpu_syscall(cpu_id);
}

// ── Per-CPU syscall infrastructure (SMP-safe via SWAPGS) ─────────────────

const SYSCALL_STACK_SIZE: usize = 8192;

/// Per-CPU data block accessed via GS segment after SWAPGS.
/// Layout must stay in sync with the assembly offsets below.
#[repr(C, align(64))]
pub struct PerCpuSyscall {
    /// gs:[0x00]. TRANSIENT scratch only — the syscall entry parks the user RSP
    /// here for the one instruction between SWAPGS and the kernel-stack switch,
    /// then immediately copies it onto the per-task kernel stack. It is NOT the
    /// exit restore source (that would be a per-CPU value shared across tasks —
    /// the CLONE_THREAD interleave bug). Kept at offset 0x00 so the hardcoded
    /// gs:[0x08]/gs:[0x10] offsets below stay put.
    pub saved_user_rsp: u64, // gs:[0x00]
    pub kernel_stack_top: u64, // gs:[0x08]
    /// Logical CPU id (0..MAX_CPUS). The authoritative source for
    /// `gdt::current_cpu_id()`. It used to live in the active (user-visible)
    /// GS base, but once AthBridge guests own their user GS base (the Win32
    /// TEB, via SYS_SET_GS_BASE) the active GS base is guest-controlled and can
    /// no longer carry the CPU id. This per-CPU block is pointed at by
    /// IA32_KERNEL_GS_BASE, which is stable per physical CPU and never changes
    /// across task switches, so the id read from here is always correct
    /// regardless of what the guest put in the active GS base. gs:[0x10].
    pub cpu_id: u64,
}

/// 8-KiB kernel-side stack per CPU, used exclusively during SYSCALL handling.
#[repr(C, align(16))]
struct SyscallKernelStack([u8; SYSCALL_STACK_SIZE]);

use crate::gdt::MAX_CPUS;

static mut PERCPU_SYSCALL: [PerCpuSyscall; MAX_CPUS] = {
    const EMPTY: PerCpuSyscall = PerCpuSyscall {
        saved_user_rsp: 0,
        kernel_stack_top: 0,
        cpu_id: 0,
    };
    [EMPTY; MAX_CPUS]
};

static PERCPU_STACKS: [SyscallKernelStack; MAX_CPUS] = {
    const EMPTY_STACK: SyscallKernelStack = SyscallKernelStack([0u8; SYSCALL_STACK_SIZE]);
    [EMPTY_STACK; MAX_CPUS]
};

const MAX_SYSCALL_PATH_BYTES: u64 = 4096;
const MAX_SYSCALL_CLIPBOARD_BYTES: u64 = crate::clipboard::MAX_CLIPBOARD_BYTES as u64;

static GUARD_PTR_REJECTS: AtomicU64 = AtomicU64::new(0);
static GUARD_BOUNDS_REJECTS: AtomicU64 = AtomicU64::new(0);
static GUARD_FS_CAP_REJECTS: AtomicU64 = AtomicU64::new(0);
static GUARD_PROC_CAP_REJECTS: AtomicU64 = AtomicU64::new(0);

/// Set up per-CPU syscall data for the given CPU and program IA32_KERNEL_GS_BASE.
/// Must be called on each CPU (BSP in init, APs in ap_entry).
pub fn init_percpu_syscall(cpu_id: usize) {
    assert!(cpu_id < MAX_CPUS);
    let stack_top = unsafe { PERCPU_STACKS[cpu_id].0.as_ptr().add(SYSCALL_STACK_SIZE) as u64 };
    unsafe {
        PERCPU_SYSCALL[cpu_id].kernel_stack_top = stack_top;
        PERCPU_SYSCALL[cpu_id].saved_user_rsp = 0;
        // Authoritative CPU id read by gdt::current_cpu_id() — see the field
        // docstring. Must be set before the first current_cpu_id() call on
        // this CPU (init runs this before scheduling starts).
        PERCPU_SYSCALL[cpu_id].cpu_id = cpu_id as u64;
        let gs_base = &PERCPU_SYSCALL[cpu_id] as *const PerCpuSyscall as u64;
        x86_64::registers::model_specific::KernelGsBase::write(VirtAddr::new(gs_base));
    }
}

/// Re-program the syscall register state after S3 resume. The CPU reset
/// cleared EFER.SCE, STAR/LSTAR/SFMASK and IA32_KERNEL_GS_BASE; the per-CPU
/// block's MEMORY survived the sleep — critically `kernel_stack_top` still
/// points at the CURRENT task's kernel stack (set by the last context
/// switch), so unlike `init_percpu_syscall` this must NOT rewrite the block:
/// resetting it to the static bootstrap stack would hand the next syscall a
/// stack another task may own (the MasterChecklist 4.8 shared-kernel-stack
/// class). Registers only.
pub fn reinit_after_resume(cpu_id: usize) {
    assert!(cpu_id < MAX_CPUS);
    unsafe {
        Efer::update(|efer| efer.insert(EferFlags::SYSTEM_CALL_EXTENSIONS));
    }
    let selectors = crate::gdt::GDT.1;
    Star::write(
        selectors.user_code_selector,
        selectors.user_data_selector,
        selectors.kernel_code_selector,
        selectors.kernel_data_selector,
    )
    .expect("Failed to rewrite STAR MSR after S3 resume");
    LStar::write(VirtAddr::new(syscall_handler as *const () as usize as u64));
    use x86_64::registers::rflags::RFlags;
    SFMask::write(RFlags::INTERRUPT_FLAG | RFlags::ALIGNMENT_CHECK); // AC cleared on entry: ring 3 must not be able to pre-open the SMAP window
    unsafe {
        let gs_base = &PERCPU_SYSCALL[cpu_id] as *const PerCpuSyscall as u64;
        x86_64::registers::model_specific::KernelGsBase::write(VirtAddr::new(gs_base));
    }
}

/// Read the logical CPU id from the kernel-GS per-CPU block.
///
/// `IA32_KERNEL_GS_BASE` points at `&PERCPU_SYSCALL[cpu_id]` whenever Rust
/// kernel code runs (the syscall handler swaps the per-CPU pointer into
/// KERNEL_GS while Rust executes; interrupt handlers run with the per-CPU
/// pointer parked in KERNEL_GS too, since they don't swapgs). The id therefore
/// no longer depends on the *active* GS base — which AthBridge guests now own
/// (the Win32 TEB, via SYS_SET_GS_BASE). Returns `None` before
/// `init_percpu_syscall` has run on this CPU (KERNEL_GS still 0) or if the
/// pointer is outside the static `PERCPU_SYSCALL` array, so the caller can
/// fall back to the legacy active-GS read for the early-boot window.
pub fn current_cpu_id_from_kernel_gs() -> Option<usize> {
    let ptr = x86_64::registers::model_specific::KernelGsBase::read().as_u64();
    if ptr == 0 {
        return None;
    }
    // Validate the pointer lies within the static PERCPU_SYSCALL array before
    // dereferencing — a stray KERNEL_GS value must never cause a wild read.
    let base = unsafe { core::ptr::addr_of!(PERCPU_SYSCALL) as u64 };
    let stride = core::mem::size_of::<PerCpuSyscall>() as u64;
    let end = base + stride * (MAX_CPUS as u64);
    if ptr < base || ptr >= end || (ptr - base) % stride != 0 {
        return None;
    }
    let pc = ptr as *const PerCpuSyscall;
    let id = unsafe { (*pc).cpu_id } as usize;
    if id < MAX_CPUS {
        Some(id)
    } else {
        None
    }
}

/// Update the kernel stack used by the syscall handler for the current CPU.
/// This must be called during context switches so that if the new task makes
/// a syscall, it uses its own kernel stack rather than the shared boot stack.
pub fn set_syscall_kernel_stack(cpu_id: usize, stack_top: u64) {
    unsafe {
        PERCPU_SYSCALL[cpu_id].kernel_stack_top = stack_top;
    }
}

#[unsafe(naked)]
pub extern "C" fn syscall_handler() {
    core::arch::naked_asm!(
        // On entry: RSP = user stack, RCX = user RIP, R11 = user RFLAGS.
        // SFMASK already cleared RFLAGS.IF so we won't be interrupted here.

        // 1. SWAPGS: load kernel per-CPU data into GS base.
        "swapgs",
        // 2. Stash user RSP in the per-CPU slot TRANSIENTLY (a scratch across the
        //    stack switch only), then switch to this task's kernel stack.
        "mov gs:[0x00], rsp", // PerCpuSyscall::saved_user_rsp (transient scratch)
        "mov rsp, gs:[0x08]", // PerCpuSyscall::kernel_stack_top
        // 2b. Save the user RSP PER-TASK on the kernel stack, ABOVE the
        //     SyscallRegisters frame. The per-CPU gs:[0x00] slot is NOT a safe
        //     restore source: if this syscall yields/blocks mid-call, another
        //     task's syscall entry overwrites that one slot, and our exit would
        //     `mov rsp, gs:[0x00]` the WRONG task's RSP — the iron-only
        //     CLONE_THREAD double fault (#DF rip=syscall-exit, rsp=other task's
        //     user stack, cr2=rsp-8 in a freed AS). The kernel stack is per-task,
        //     so a value saved here survives any number of interleaved syscalls.
        "push qword ptr gs:[0x00]", // user RSP (per-task)
        "sub rsp, 8",               // pad: keep the pre-`call` stack 16-byte aligned
        // 3. Push user RIP and RFLAGS onto the KERNEL stack.
        "push rcx", // user RIP
        "push r11", // user RFLAGS
        // 4. Save all GPRs onto kernel stack.
        "push r15",
        "push r14",
        "push r13",
        "push r12",
        "push r10",
        "push r9",
        "push r8",
        "push rdi",
        "push rsi",
        "push rbp",
        "push rbx",
        "push rdx",
        "push rax",
        // 5. Pass pointer to the saved register frame as the first argument.
        "mov rdi, rsp",
        // 6.5. Swap GS back so that current_cpu_id() reads the correct CPU ID inside Rust code.
        "swapgs",
        "call syscall_handler_inner",
        // 6.6. Swap GS again to get PerCpuSyscall back for restoring state.
        "swapgs",
        // 7. Restore GPRs.
        "pop rax",
        "pop rdx",
        "pop rbx",
        "pop rbp",
        "pop rsi",
        "pop rdi",
        "pop r8",
        "pop r9",
        "pop r10",
        "pop r12",
        "pop r13",
        "pop r14",
        "pop r15",
        // 7. Restore user RFLAGS (R11) and RIP (RCX).
        "pop r11",
        "pop rcx",
        // 8. Validate RCX is canonical (< 2^47) to avoid the Intel SYSRET #GP
        // vulnerability — a non-canonical RCX causes a #GP at CPL-0 with the
        // user's RSP, which is exploitable.
        //
        // CRITICAL #1: this check must NOT clobber RAX — RAX holds the syscall
        // return value userspace expects. Save/restore RAX around the check.
        //
        // CRITICAL #2 (SMAP): the scratch `push rax` MUST run while RSP still
        // points at the KERNEL stack — i.e. BEFORE we restore the user RSP and
        // SWAPGS. With CR4.SMAP set, a ring-0 push to the (user-accessible)
        // user stack faults (#PF → #DF); the old code pushed here AFTER the
        // stack switch, which double-faulted the instant SMAP went live. GS is
        // kernel per-cpu at this point (SWAPGS after the call), so the
        // non-canonical path below can read gs:[0x08] directly.
        "push rax",
        "mov rax, rcx",
        "shr rax, 47",
        "cmp rax, 0",
        "pop rax",
        "jne 1f",
        // 9. RCX canonical: restore user RSP from the PER-TASK kernel-stack slot
        //    (see entry 2b — `mov rsp, gs:[0x00]` here was the interleave bug),
        //    SWAPGS back to user GS, and return to user mode.
        "add rsp, 8", // drop the alignment pad
        "pop rsp",    // user RSP (per-task)
        "swapgs",
        "sysretq",
        // 10. RCX non-canonical — kill the task instead of faulting. We are
        //     STILL on the kernel stack and GS is STILL kernel per-cpu (we
        //     never restored the user RSP or swapped), so entering Rust is
        //     safe. Reset RSP to a clean per-cpu kernel stack top, then SWAPGS
        //     so KERNEL_GS holds the per-cpu pointer (the invariant
        //     current_cpu_id_from_kernel_gs relies on), matching the canonical
        //     path's exit GS state.
        "1:",
        "mov rsp, gs:[0x08]", // PerCpuSyscall::kernel_stack_top (GS=kernel per-cpu)
        "swapgs",
        "mov rdi, 0xDEAD",
        "call exit_current_task",
        "ud2", // unreachable
    );
}

#[repr(C)]
#[derive(Debug)]
pub struct SyscallRegisters {
    pub rax: u64,
    pub rdx: u64,
    pub rbx: u64,
    pub rbp: u64,
    pub rsi: u64,
    pub rdi: u64,
    pub r8: u64,
    pub r9: u64,
    pub r10: u64,
    pub r12: u64,
    pub r13: u64,
    pub r14: u64,
    pub r15: u64,
    pub r11: u64, // RFLAGS
    pub rcx: u64, // RIP
}

fn user_leaf_flags(addr: u64) -> Option<x86_64::structures::paging::PageTableFlags> {
    use x86_64::registers::control::Cr3;
    use x86_64::structures::paging::page_table::PageTable;
    use x86_64::structures::paging::PageTableFlags;

    let offset = *crate::memory::PHYS_MEM_OFFSET.get()?;
    let virt = VirtAddr::new(addr);
    let (pml4_frame, _) = Cr3::read();

    unsafe {
        let pml4 = &*(offset + pml4_frame.start_address().as_u64()).as_ptr::<PageTable>();
        let pml4e = &pml4[virt.p4_index()];
        if !pml4e
            .flags()
            .contains(PageTableFlags::PRESENT | PageTableFlags::USER_ACCESSIBLE)
        {
            return None;
        }

        let pdpt = &*(offset + pml4e.addr().as_u64()).as_ptr::<PageTable>();
        let pdpte = &pdpt[virt.p3_index()];
        if !pdpte
            .flags()
            .contains(PageTableFlags::PRESENT | PageTableFlags::USER_ACCESSIBLE)
        {
            return None;
        }
        if pdpte.flags().contains(PageTableFlags::HUGE_PAGE) {
            return Some(pml4e.flags() & pdpte.flags());
        }

        let pd = &*(offset + pdpte.addr().as_u64()).as_ptr::<PageTable>();
        let pde = &pd[virt.p2_index()];
        if !pde
            .flags()
            .contains(PageTableFlags::PRESENT | PageTableFlags::USER_ACCESSIBLE)
        {
            return None;
        }
        if pde.flags().contains(PageTableFlags::HUGE_PAGE) {
            return Some(pml4e.flags() & pdpte.flags() & pde.flags());
        }

        let pt = &*(offset + pde.addr().as_u64()).as_ptr::<PageTable>();
        let pte = &pt[virt.p1_index()];
        if !pte
            .flags()
            .contains(PageTableFlags::PRESENT | PageTableFlags::USER_ACCESSIBLE)
        {
            return None;
        }
        Some(pml4e.flags() & pdpte.flags() & pde.flags() & pte.flags())
    }
}

fn validate_user_range(ptr: u64, len: u64, write: bool) -> Result<(), ()> {
    use x86_64::structures::paging::PageTableFlags;

    const USER_SPACE_END: u64 = 0x0000_8000_0000_0000;
    let end = ptr.checked_add(len).ok_or(())?; // overflow check
    if end > USER_SPACE_END {
        return Err(());
    }
    if ptr == 0 && len > 0 {
        return Err(());
    } // null pointer
    if len == 0 {
        return Ok(());
    }

    let start_page = ptr & !0xFFF;
    let last_page = (end - 1) & !0xFFF;
    let mut page = start_page;
    loop {
        let flags = user_leaf_flags(page).ok_or(())?;
        if write && !flags.contains(PageTableFlags::WRITABLE) {
            return Err(());
        }
        if page == last_page {
            break;
        }
        page = page.checked_add(4096).ok_or(())?;
    }
    // Spectre v1 (bounds-check bypass): serialize before any caller
    // dereferences the now-validated pointer. Without this the CPU can
    // speculatively execute the dependent user copy past the bounds/permission
    // branches above — even when the architectural result is Err — and leak via
    // a cache side channel. `lfence` is the AMD/Intel-recommended barrier here
    // (cf. Linux `barrier_nospec()`); it sits on the SUCCESS path only, so the
    // (rejecting) Err returns above are unaffected and the hot path pays one
    // fence per validated syscall buffer. The previous `nop`-with-`nomem`
    // markers gave NO ordering guarantee (codebase_review §Critical 6).
    unsafe {
        core::arch::asm!("lfence", options(nomem, nostack, preserves_flags));
    }
    Ok(())
}

fn note_guard_ptr_reject() {
    GUARD_PTR_REJECTS.fetch_add(1, Ordering::Relaxed);
}

fn note_guard_bounds_reject() {
    GUARD_BOUNDS_REJECTS.fetch_add(1, Ordering::Relaxed);
}

fn note_guard_fs_cap_reject() {
    GUARD_FS_CAP_REJECTS.fetch_add(1, Ordering::Relaxed);
}

fn note_guard_proc_cap_reject() {
    GUARD_PROC_CAP_REJECTS.fetch_add(1, Ordering::Relaxed);
}

/// Compatibility bridge: if a task has no Filesystem caps yet, keep legacy behavior.
fn has_filesystem_write_cap_if_declared() -> bool {
    use crate::capability::{Cap, Rights};
    crate::scheduler::with_current_task(|task| {
        let mut saw_fs = false;
        let mut allow = false;
        for (_, cap) in task.cap_table.iter() {
            if let Cap::Filesystem { rights, .. } = cap {
                saw_fs = true;
                if rights.contains(Rights::WRITE) {
                    allow = true;
                    break;
                }
            }
        }
        !saw_fs || allow
    })
    .unwrap_or(true)
}

/// Does the current task hold a `Cap::ScreenCapture`? Unlike the filesystem/
/// process compatibility bridges (which fail-OPEN when no cap of that flavor is
/// declared, for migration), screen capture is privacy-sensitive and fails
/// CLOSED: a task must explicitly hold `Cap::ScreenCapture` to read screen
/// pixels. The screenshot tool / Game Bar are seeded this cap; everything else
/// is refused. Returns `false` if there is no current task.
fn has_screen_capture_cap() -> bool {
    use crate::capability::Cap;
    crate::scheduler::with_current_task(|task| {
        task.cap_table
            .iter()
            .any(|(_, cap)| matches!(cap, Cap::ScreenCapture { .. }))
    })
    .unwrap_or(false)
}

/// Does the current task hold a `Cap::Accessibility` with `needed` rights?
/// Like `Cap::ScreenCapture`, the AT surface fails CLOSED: an assistive-tech
/// client must explicitly hold `Cap::Accessibility{READ}` to snapshot the tree
/// and `{WRITE}` to dispatch actions. Reading another app's UI structure +
/// labels (and driving its widgets) is privileged. Returns `false` if there is
/// no current task.
fn has_accessibility_cap(needed: crate::capability::Rights) -> bool {
    use crate::capability::Cap;
    crate::scheduler::with_current_task(|task| {
        task.cap_table
            .iter()
            .any(|(_, cap)| matches!(cap, Cap::Accessibility { rights } if rights.contains(needed)))
    })
    .unwrap_or(false)
}

/// Compatibility bridge for SYS_LINUX_EXEC: enforce EXEC when process caps exist.
fn has_process_exec_cap_if_declared() -> bool {
    use crate::capability::{Cap, Rights};
    crate::scheduler::with_current_task(|task| {
        let mut saw_proc = false;
        let mut allow = false;
        for (_, cap) in task.cap_table.iter() {
            if let Cap::Process { rights, .. } = cap {
                saw_proc = true;
                if rights.contains(Rights::EXEC) {
                    allow = true;
                    break;
                }
            }
        }
        !saw_proc || allow
    })
    .unwrap_or(true)
}

fn copy_from_user(ptr: u64, len: u64) -> Result<Vec<u8>, ()> {
    let len = usize::try_from(len).map_err(|_| ())?;
    validate_user_range(ptr, len as u64, false)?;
    let mut out = Vec::with_capacity(len);
    unsafe {
        out.set_len(len);
        // Run the copy under an extable fixup so a TOCTOU race (sibling CPU
        // unmaps the validated page between validate_user_range and the
        // copy itself) is recovered via page_fault_inner rewriting our RIP
        // to a fault label, instead of falling through to
        // has_current_task() → SCHEDULER.lock() which can deadlock if the
        // interrupted code holds SCHEDULER (e.g. a with_current_task
        // closure). See kernel/src/extable.rs.
        crate::extable::copy_user_with_fixup(ptr as *const u8, out.as_mut_ptr(), len)?;
    }
    Ok(out)
}

fn copy_to_user(ptr: u64, bytes: &[u8]) -> Result<(), ()> {
    validate_user_range(ptr, bytes.len() as u64, true)?;
    unsafe {
        // Same TOCTOU protection as copy_from_user — the user page could
        // be unmapped between validate and the actual write.
        crate::extable::copy_user_with_fixup(bytes.as_ptr(), ptr as *mut u8, bytes.len())?;
    }
    Ok(())
}

fn read_user_cstr(ptr: u64, max_len: usize) -> Option<alloc::string::String> {
    if ptr == 0 {
        return None;
    }
    // SMAP-safe: the scan runs through the uaccess/extable chokepoint (no raw
    // user deref); semantics unchanged (NUL excluded, truncate at max_len,
    // strict UTF-8).
    let bytes = crate::uaccess::read_user_cstr_bytes(ptr, max_len).ok()?;
    alloc::string::String::from_utf8(bytes).ok()
}

pub fn dump_guard_text() -> alloc::string::String {
    alloc::format!(
        "# AthenaOS syscall hardening\nptr_rejects: {}\nbounds_rejects: {}\nfs_cap_rejects: {}\nproc_cap_rejects: {}\npath_len_limit: {}\nclipboard_len_limit: {}\n",
        GUARD_PTR_REJECTS.load(Ordering::Relaxed),
        GUARD_BOUNDS_REJECTS.load(Ordering::Relaxed),
        GUARD_FS_CAP_REJECTS.load(Ordering::Relaxed),
        GUARD_PROC_CAP_REJECTS.load(Ordering::Relaxed),
        MAX_SYSCALL_PATH_BYTES,
        MAX_SYSCALL_CLIPBOARD_BYTES,
    )
}

pub fn run_boot_smoketest() {
    // Keep this trivial and deterministic: ensure procfs dump path is wired.
    let text = dump_guard_text();
    if text.starts_with("# AthenaOS syscall hardening") {
        crate::serial_println!("[syscall] guard smoketest OK");
    } else {
        crate::serial_println!("[syscall] guard smoketest FAIL");
    }

    gsbase_msr_roundtrip_smoketest();
    crate::memory::run_mprotect_smoketest();
}

/// FAIL-able proof of the `SYS_SET_GS_BASE` (282) MSR mechanics: write a
/// TEB-like value into the ACTIVE GS base (the MSR the syscall arm writes),
/// read it back via the same MSR, assert it survived, then RESTORE the boot
/// CPU's original GS base. This catches a wrong-MSR write (the single most
/// error-prone line per the spec) — if the arm wrote KernelGsBase instead,
/// `GsBase::read()` here would not observe the value.
///
/// Runs interrupts-masked and restores the original active GS base before
/// re-enabling, so the BSP's `current_cpu_id()` fast path (active GS == cpu_id
/// for the boot thread) is never left perturbed. The cross-context-switch
/// survival (the scheduler restore) is proven end-to-end by AthBridge's gs-PE
/// smoketest (spec §5) once the loader wiring lands; here we prove the MSR
/// plumbing the syscall depends on.
fn gsbase_msr_roundtrip_smoketest() {
    #[cfg(target_arch = "x86_64")]
    {
        use crate::arch::VirtAddr;
        use x86_64::registers::model_specific::GsBase;

        // A canonical user-half address standing in for a TEB self-pointer.
        const TEST_TEB: u64 = 0x0000_7FFF_DEAD_B000;

        x86_64::instructions::interrupts::without_interrupts(|| {
            let original = GsBase::read().as_u64();
            GsBase::write(VirtAddr::new(TEST_TEB));
            let read_back = GsBase::read().as_u64();
            // Restore BEFORE logging (logging may touch current_cpu_id()).
            GsBase::write(VirtAddr::new(original));

            if read_back == TEST_TEB {
                crate::serial_println!(
                    "[gsbase] smoketest: wrote={:#x} read_back={:#x} (active GS MSR round-trip) -> PASS",
                    TEST_TEB,
                    read_back
                );
            } else {
                crate::serial_println!(
                    "[gsbase] smoketest: wrote={:#x} read_back={:#x} MISMATCH (wrong-MSR write?) -> FAIL",
                    TEST_TEB,
                    read_back
                );
            }
        });
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        crate::serial_println!("[gsbase] smoketest: non-x86_64 -> SKIP");
    }
}

#[no_mangle]
pub extern "C" fn syscall_handler_inner(regs: &mut SyscallRegisters) {
    crate::gpu_render::drain_deferred_wakes();
    // (Removed the unconditional per-syscall serial trace: it flooded the bootlog RAM
    // ring — wrapping out late diagnostics like the amdgpud PSP bring-up — and on iron
    // every serial_println blocks CPU0 ~5ms on the UART, a severe per-syscall latency
    // tax. Re-add a GATED trace if syscall-level debugging is needed again.)
    let pid = crate::scheduler::current_task_id()
        .map(|t| t.raw())
        .unwrap_or(0);
    let is_linux = crate::linux_syscall::is_linux_task(pid);

    // ── AthGuard sandbox gate (Phase 9) ─────────────────────────────────────
    // Per-task enforcement: a sandboxed app is denied the device/network/install
    // syscall classes its policy forbids. Trusted tasks (the default) short-circuit
    // on a single atomic load, so the hot path is unaffected.
    //
    // The gate runs BEFORE ABI dispatch — the Linux branch used to return
    // first, so a sandboxed Linux binary bypassed AthGuard entirely (fixed
    // 2026-06-10). The deny value u64::MAX reads as -1 = -EPERM under the
    // Linux ABI and as the native error sentinel under the AthenaOS ABI, so
    // one encoding serves both tables.
    let gate_ok = if is_linux {
        crate::sandbox::check_linux_syscall(pid, regs.rax)
    } else {
        // Device-claim (SYS_DRIVER_CLAIM_DEVICE=111) carries the target's
        // device id (packed PCI BDF) in rsi; thread it so the gate evaluates
        // the claim against the device's SPECIFIC kind (NIC vs GPU vs storage)
        // instead of the old aliased `Gpu`. For every other syscall the device
        // id is ignored by the gate.
        crate::sandbox::check_syscall_dev(pid, regs.rax, regs.rsi)
    };
    if !gate_ok {
        regs.rax = u64::MAX;
        return;
    }

    // Linux ELF tasks route through the Linux syscall translation layer
    // (Linux x86_64 numbers instead of AthenaOS native numbers).
    if is_linux {
        crate::linux_syscall::linux_syscall_dispatch(regs);
        return;
    }

    match regs.rax {
        // SYS_PRINT
        1 => {
            crate::serial_println!("[user-thread] msg: {}", regs.rdi);
        }
        // SYS_SEND
        2 => {
            use crate::capability::{Cap, CapHandle, Rights};
            let cap_handle = CapHandle::from_raw(regs.rdi);
            let msg = crate::ipc::Message {
                msg_type: regs.rsi,
                arg1: regs.rdx,
                arg2: regs.r10,
                arg3: regs.r8,
            };

            let mut sent = false;
            let mut should_block_on = None;
            let mut target_chan = None;
            let mut chan_id_opt = None;

            crate::scheduler::with_current_task(|task| {
                if let Some(Cap::Channel { chan_id, rights }) = task.cap_table.get(cap_handle) {
                    if rights.contains(Rights::WRITE) {
                        chan_id_opt = Some(chan_id);
                    }
                }
            });

            if let Some(chan_id) = chan_id_opt {
                match crate::ipc::IPC.lock().send(chan_id as usize, msg) {
                    Ok(()) => {
                        sent = true;
                        target_chan = Some(chan_id as usize);
                    }
                    Err(_) => {
                        should_block_on = Some(chan_id as usize);
                    }
                }
            }

            if let Some(chan_id) = should_block_on {
                // Retry the syscall when unblocked (queue has space now)
                regs.rcx -= 2;
                // Block the task and yield
                crate::scheduler::block_current_task(
                    crate::task::TaskState::BlockedOnSend(chan_id),
                    regs as *mut _ as usize,
                );
                return;
            } else if sent {
                regs.rax = 0; // Success
                if let Some(chan) = target_chan {
                    crate::scheduler::unblock_receivers(chan);
                }
            } else {
                regs.rax = u64::MAX; // Error
            }
        }
        // SYS_RECV
        3 => {
            use crate::capability::{Cap, CapHandle, Rights};
            let cap_handle = CapHandle::from_raw(regs.rdi);

            let mut received_msg = None;
            let mut should_block_on = None;
            let mut target_chan = None;
            let mut chan_id_opt = None;

            crate::scheduler::with_current_task(|task| {
                if let Some(Cap::Channel { chan_id, rights }) = task.cap_table.get(cap_handle) {
                    if rights.contains(Rights::READ) {
                        chan_id_opt = Some(chan_id);
                    }
                }
            });

            if let Some(chan_id) = chan_id_opt {
                if let Some(msg) = crate::ipc::IPC.lock().try_recv(chan_id as usize) {
                    received_msg = Some(msg);
                    target_chan = Some(chan_id as usize);
                } else {
                    should_block_on = Some(chan_id as usize);
                }
            }

            if let Some(chan_id) = should_block_on {
                // When we wake up, we need to retry the syscall because the queue has a message now.
                // Decrease RIP by 2 (size of 'syscall' instruction) so we re-execute it in user-space.
                regs.rcx -= 2;
                // Block the task and yield
                crate::scheduler::block_current_task(
                    crate::task::TaskState::BlockedOnReceive(chan_id),
                    regs as *mut _ as usize,
                );
                return;
            } else if let Some(msg) = received_msg {
                regs.rax = 0; // Success
                regs.rsi = msg.msg_type;
                regs.rdx = msg.arg1;
                regs.r10 = msg.arg2;
                regs.r8 = msg.arg3;

                if let Some(chan) = target_chan {
                    crate::scheduler::unblock_senders(chan);
                }
            } else {
                regs.rax = u64::MAX; // Empty / Error (e.g. invalid capability)
            }
        }

        // SYS_LINUX_EXEC (dedicated Linux ELF entry point)
        // rdi = path (C string), rsi = applet (C string, optional; if null, run path as argv[0])
        5000 => {
            if !has_process_exec_cap_if_declared() {
                note_guard_proc_cap_reject();
                regs.rax = crate::capability::E_RIGHTS;
                return;
            }
            let path = read_user_cstr(regs.rdi, 4096);
            if path.is_none() {
                note_guard_ptr_reject();
                regs.rax = u64::MAX;
                return;
            }
            let path = path.unwrap();
            if path.is_empty() {
                note_guard_bounds_reject();
                regs.rax = u64::MAX;
                return;
            }
            let applet = if regs.rsi == 0 {
                None
            } else {
                match read_user_cstr(regs.rsi, 256) {
                    Some(s) => Some(s),
                    None => {
                        note_guard_ptr_reject();
                        regs.rax = u64::MAX;
                        return;
                    }
                }
            };

            let res = if let Some(applet) = applet {
                crate::linux_exec::linux_exec_busybox(&path, &applet)
            } else {
                crate::linux_exec::linux_exec(&path, &[&path])
            };
            match res {
                Ok(tid) => regs.rax = tid.raw(),
                Err(e) => regs.rax = e.as_neg() as u64,
            }
        }
        // SYS_CAP_GRANT — derive a narrower cap from one we hold and deposit
        // it into another task. See docs/design/capabilities.md.
        //   rdi = target task id
        //   rsi = source cap handle (must have GRANT right)
        //   rdx = desired rights bitset (truncated to defined bits)
        //   r10 = derive arg 0 (Mmio: phys-start offset added to parent.start;
        //                        Port: port-base offset; Channel/Irq: ignored)
        //   r8  = derive arg 1 (Mmio: byte length of sub-range;
        //                        Port: count; Channel/Irq: ignored)
        // Returns the new CapHandle.raw() in the target — or a cap error code.
        4 => {
            use crate::capability::{self, Cap, CapHandle, Rights};
            use crate::task::TaskId;

            let target = TaskId::from_raw(regs.rdi);
            let src = CapHandle::from_raw(regs.rsi);
            let new_rights = Rights::from_bits_truncate(regs.rdx as u32);
            let derive0 = regs.r10;
            let derive1 = regs.r8 as usize;

            let granter = match crate::scheduler::current_task_id() {
                Some(id) => id,
                None => {
                    regs.rax = capability::E_NO_TASK;
                    return;
                }
            };

            // Read the parent so we can synthesize the derived cap.
            let parent =
                crate::scheduler::with_task_by_id(granter, |t| t.cap_table.get(src)).flatten();
            let parent = match parent {
                Some(p) => p,
                None => {
                    regs.rax = capability::E_NO_HANDLE;
                    return;
                }
            };

            let derived = match parent {
                Cap::Channel { chan_id, .. } => Cap::Channel {
                    chan_id,
                    rights: new_rights,
                },
                Cap::Mmio {
                    start_phys, len, ..
                } => Cap::Mmio {
                    start_phys: start_phys.saturating_add(derive0),
                    len: if derive1 == 0 { len } else { derive1 },
                    rights: new_rights,
                },
                Cap::Irq { vector, .. } => Cap::Irq {
                    vector,
                    rights: new_rights,
                },
                Cap::Port { base, count, .. } => Cap::Port {
                    base: base.saturating_add(derive0 as u16),
                    count: if derive1 == 0 { count } else { derive1 as u16 },
                    rights: new_rights,
                },
                Cap::Filesystem { root_inode, .. } => Cap::Filesystem {
                    root_inode,
                    rights: new_rights,
                },
                Cap::Network {
                    port_range_start,
                    port_range_end,
                    ..
                } => Cap::Network {
                    port_range_start,
                    port_range_end,
                    rights: new_rights,
                },
                Cap::Gpu { device_id, .. } => Cap::Gpu {
                    device_id,
                    rights: new_rights,
                },
                Cap::Audio { device_id, .. } => Cap::Audio {
                    device_id,
                    rights: new_rights,
                },
                Cap::Camera { device_id, .. } => Cap::Camera {
                    device_id,
                    rights: new_rights,
                },
                Cap::Process { target_pid, .. } => Cap::Process {
                    target_pid,
                    rights: new_rights,
                },
                Cap::CryptoKey { key_id, .. } => Cap::CryptoKey {
                    key_id,
                    rights: new_rights,
                },
                Cap::Hypervisor { vm_id, .. } => Cap::Hypervisor {
                    vm_id,
                    rights: new_rights,
                },
                Cap::Attestation { session_id, .. } => Cap::Attestation {
                    session_id,
                    rights: new_rights,
                },
                Cap::Debug { scope, .. } => Cap::Debug {
                    scope,
                    rights: new_rights,
                },
                Cap::System { rights: _ } => Cap::System { rights: new_rights },
                Cap::ScreenCapture { rights: _ } => Cap::ScreenCapture { rights: new_rights },
                Cap::Accessibility { rights: _ } => Cap::Accessibility { rights: new_rights },
            };

            match capability::grant(granter, src, target, derived) {
                Ok(new_h) => {
                    regs.rax = new_h.raw();
                }
                Err(e) => {
                    regs.rax = e.as_u64();
                }
            }
        }
        // SYS_CAP_REVOKE — revoke a cap from `target`.
        //   rdi = target task id
        //   rsi = handle in target's table
        5 => {
            use crate::capability::{self, CapHandle};
            use crate::task::TaskId;

            let target = TaskId::from_raw(regs.rdi);
            let handle = CapHandle::from_raw(regs.rsi);

            let revoker = match crate::scheduler::current_task_id() {
                Some(id) => id,
                None => {
                    regs.rax = capability::E_NO_TASK;
                    return;
                }
            };

            match capability::revoke(revoker, target, handle) {
                Ok(()) => {
                    regs.rax = 0;
                }
                Err(e) => {
                    regs.rax = e.as_u64();
                }
            }
        }
        // SYS_CAP_QUERY — inspect one of OUR own caps.
        //   rdi = handle in our table
        // Returns:
        //   rax = 0 on success, error code on failure
        //   rsi = flavor (1=Channel, 2=Mmio, 3=Irq, 4=Port)
        //   rdx = rights bits
        6 => {
            use crate::capability::{self, CapHandle};

            let h = CapHandle::from_raw(regs.rdi);
            let cap = crate::scheduler::with_current_task(|t| t.cap_table.get(h)).flatten();

            match cap {
                Some(c) => {
                    regs.rax = 0;
                    regs.rsi = c.flavor_id() as u64;
                    regs.rdx = c.rights().bits() as u64;
                }
                None => {
                    regs.rax = capability::E_NO_HANDLE;
                }
            }
        }
        // SYS_MMIO_MAP — redeem an Mmio cap: map its physical pages into the
        // current task's PML4 at the chosen user virtual address.
        //   rdi = cap_handle
        //   rsi = user virtual base (page-aligned, must lie in lower half)
        //   rdx = length in bytes (page-aligned, must be ≤ cap.len)
        // Returns 0 on success, error code otherwise.
        7 => {
            use crate::arch::{PhysAddr, VirtAddr};
            use crate::capability::{self, Cap, CapHandle, Rights};
            use x86_64::structures::paging::{Page, PageTableFlags, PhysFrame, Size4KiB};

            let handle = CapHandle::from_raw(regs.rdi);
            let user_virt = regs.rsi;
            let length = regs.rdx as usize;

            let cap = crate::scheduler::with_current_task(|t| t.cap_table.get(handle)).flatten();
            let (start_phys, cap_len, rights) = match cap {
                Some(Cap::Mmio {
                    start_phys,
                    len,
                    rights,
                }) => (start_phys, len, rights),
                Some(_) => {
                    regs.rax = capability::E_WRONG_FLAVOR;
                    return;
                }
                None => {
                    regs.rax = capability::E_NO_HANDLE;
                    return;
                }
            };

            if !rights.contains(Rights::MAP) {
                regs.rax = capability::E_RIGHTS;
                return;
            }
            if length == 0
                || length > cap_len
                || (user_virt & 0xFFF) != 0
                || (length & 0xFFF) != 0
                || user_virt >= 0x0000_8000_0000_0000
            {
                regs.rax = capability::E_INVAL;
                return;
            }

            let pml4 = crate::scheduler::with_current_task(|t| t.pml4).flatten();
            let pml4 = match pml4 {
                Some(p) => p,
                None => {
                    regs.rax = capability::E_INVAL;
                    return;
                }
            };

            // MMIO mappings: PRESENT + WRITABLE + USER + NO_CACHE + WRITE_THROUGH
            // so device writes aren't reordered/coalesced through the CPU cache.
            let flags = PageTableFlags::PRESENT
                | PageTableFlags::WRITABLE
                | PageTableFlags::USER_ACCESSIBLE
                | PageTableFlags::NO_CACHE
                | PageTableFlags::WRITE_THROUGH;

            let pages = length / 4096;
            for i in 0..pages {
                let phys = PhysAddr::new(start_phys + (i * 4096) as u64);
                let virt = VirtAddr::new(user_virt + (i * 4096) as u64);
                let frame: PhysFrame<Size4KiB> = PhysFrame::containing_address(phys);
                let page: Page<Size4KiB> = Page::containing_address(virt);
                unsafe {
                    crate::memory::map_page_in_pml4(pml4, page, frame, flags);
                }
            }
            // We just mutated the active PML4 (the user is calling us, so the
            // process's PML4 *is* live). Flush so the new mappings are visible.
            x86_64::instructions::tlb::flush_all();

            crate::serial_println!(
                "[syscall] MMIO_MAP: {} bytes phys 0x{:x} -> user 0x{:x}",
                length,
                start_phys,
                user_virt,
            );
            regs.rax = 0;
        }
        // SYS_IRQ_WAIT — block until the IRQ vector named by an Irq cap fires.
        //   rdi = cap_handle
        // Returns 0 when we resume after the IRQ fired, error code otherwise.
        8 => {
            use crate::capability::{self, Cap, CapHandle, Rights};

            let handle = CapHandle::from_raw(regs.rdi);
            let cap = crate::scheduler::with_current_task(|t| t.cap_table.get(handle)).flatten();
            let vector = match cap {
                Some(Cap::Irq { vector, rights }) => {
                    if !rights.contains(Rights::WAIT) {
                        regs.rax = capability::E_RIGHTS;
                        return;
                    }
                    vector
                }
                Some(_) => {
                    regs.rax = capability::E_WRONG_FLAVOR;
                    return;
                }
                None => {
                    regs.rax = capability::E_NO_HANDLE;
                    return;
                }
            };

            crate::serial_println!("[syscall] IRQ_WAIT: blocking on vector {}", vector);
            crate::scheduler::block_current_task(
                crate::task::TaskState::BlockedOnIrq(vector),
                regs as *mut _ as usize,
            );
            // When we resume, the IRQ has fired and the scheduler put us
            // back on the ready queue. Return success.
            regs.rax = 0;
        }
        // SYS_PORT_READ — read a byte from an x86 I/O port via a Port cap.
        //   rdi = cap_handle
        //   rsi = port number (must satisfy base ≤ port < base+count)
        // Returns the byte in rax on success, or an error code.
        9 => {
            use crate::capability::{self, Cap, CapHandle, Rights};
            let handle = CapHandle::from_raw(regs.rdi);
            let port = regs.rsi as u16;
            let cap = crate::scheduler::with_current_task(|t| t.cap_table.get(handle)).flatten();
            match cap {
                Some(Cap::Port {
                    base,
                    count,
                    rights,
                }) => {
                    if !rights.contains(Rights::READ) {
                        regs.rax = capability::E_RIGHTS;
                        return;
                    }
                    if port < base || port >= base.saturating_add(count) {
                        regs.rax = capability::E_INVAL;
                        return;
                    }
                    let v: u8;
                    unsafe {
                        core::arch::asm!(
                            "in al, dx",
                            in("dx") port,
                            out("al") v,
                            options(nomem, nostack, preserves_flags),
                        );
                    }
                    regs.rax = v as u64;
                }
                Some(_) => {
                    regs.rax = capability::E_WRONG_FLAVOR;
                }
                None => {
                    regs.rax = capability::E_NO_HANDLE;
                }
            }
        }
        // SYS_PORT_WRITE — write a byte to an x86 I/O port via a Port cap.
        //   rdi = cap_handle
        //   rsi = port number
        //   rdx = byte value (low 8 bits)
        // Returns 0 on success or an error code.
        10 => {
            use crate::capability::{self, Cap, CapHandle, Rights};
            let handle = CapHandle::from_raw(regs.rdi);
            let port = regs.rsi as u16;
            let value = regs.rdx as u8;
            let cap = crate::scheduler::with_current_task(|t| t.cap_table.get(handle)).flatten();
            match cap {
                Some(Cap::Port {
                    base,
                    count,
                    rights,
                }) => {
                    if !rights.contains(Rights::WRITE) {
                        regs.rax = capability::E_RIGHTS;
                        return;
                    }
                    if port < base || port >= base.saturating_add(count) {
                        regs.rax = capability::E_INVAL;
                        return;
                    }
                    unsafe {
                        core::arch::asm!(
                            "out dx, al",
                            in("dx") port,
                            in("al") value,
                            options(nomem, nostack, preserves_flags),
                        );
                    }
                    regs.rax = 0;
                }
                Some(_) => {
                    regs.rax = capability::E_WRONG_FLAVOR;
                }
                None => {
                    regs.rax = capability::E_NO_HANDLE;
                }
            }
        }
        // (Legacy SYS_DRIVER_REGISTER arm removed — collided with the
        // canonical Path-C userspace driver framework arm at 109 further
        // below. The userspace_driver::sys_register arm wins by the
        // ath_abi contract; this scaffold path was dead code. See
        // docs/SYSCALL_TABLE.md.)
        // SYS_SPAWN — spawn a child process from an ELF binary in the VFS
        //   rdi = pointer to path string
        //   rsi = length of path
        //   rdx = optional PTY slave id (0 = none) for shell children
        // Returns PID on success, MAX on error.
        11 => {
            let name_bytes = match copy_from_user(regs.rdi, regs.rsi) {
                Ok(s) => s,
                Err(_) => {
                    regs.rax = u64::MAX - 3;
                    return;
                }
            };
            let pty_id = if regs.rdx != 0 {
                Some(regs.rdx as u32)
            } else {
                None
            };
            if let Ok(name) = core::str::from_utf8(&name_bytes) {
                if let Some(data) = crate::vfs::read_file(name) {
                    let parent_id = crate::scheduler::current_task_id();
                    // Route by ELF OS/ABI byte: native AthenaOS apps are stamped
                    // ELFOSABI_ATHENAOS (0xAE) by xtask; anything carrying a
                    // Linux identity (0x00 SysV / 0x03 Linux) is loaded through
                    // linux_exec — Linux auxv stack, Linux syscall-table
                    // marking, POSIX state + console fds — so unmodified Linux
                    // binaries spawn with the ABI they were compiled against.
                    let is_linux_elf = matches!(
                        crate::elf_loader::detect_elf_origin(&data),
                        Ok(crate::elf_loader::ElfOrigin::Linux)
                    );
                    if is_linux_elf {
                        // PTY attach stays native-only; linux_exec installs
                        // console fds 1/2 for output instead.
                        match crate::linux_exec::linux_exec(name, &[name]) {
                            Ok(id) => regs.rax = id.raw(),
                            Err(_) => regs.rax = u64::MAX,
                        }
                    } else {
                        match crate::task::Task::new_elf_with_pty(&data, parent_id, pty_id) {
                            Ok(mut child) => {
                                // Seed BEFORE enqueue. Once spawn() publishes the
                                // task, another CPU may run it immediately; looking
                                // it up afterward raced that transition on QEMU and
                                // intermittently left amdgpud without claim authority.
                                crate::userspace_driver::maybe_seed_driver_daemon_task(
                                    &mut child, name,
                                );
                                let id = child.id;
                                crate::scheduler::spawn(child);
                                regs.rax = id.raw();
                            }
                            Err(_) => regs.rax = u64::MAX,
                        }
                    }
                } else {
                    regs.rax = u64::MAX - 1; // File not found
                }
            } else {
                regs.rax = u64::MAX - 2; // Invalid UTF-8
            }
        }
        // SYS_EXIT — terminate the current process
        //   rdi = exit code
        12 => {
            let exit_code = regs.rdi;
            crate::scheduler::exit_current_task(exit_code);
        }
        // SYS_WAIT — wait for a process to exit and return its exit code
        //   rdi = target PID
        // Returns exit code, or MAX if not found.
        13 => {
            let target_id = crate::task::TaskId::from_raw(regs.rdi);
            match crate::scheduler::try_wait_task(target_id) {
                crate::scheduler::WaitResult::Reaped(code) => {
                    regs.rax = code;
                }
                crate::scheduler::WaitResult::NotFound => {
                    regs.rax = u64::MAX;
                }
                crate::scheduler::WaitResult::Blocked => {
                    crate::scheduler::block_current_task(
                        crate::task::TaskState::BlockedOnWait(target_id),
                        regs as *mut _ as usize,
                    );
                    regs.rcx -= 2; // retry syscall
                }
            }
        }
        // SYS_KILL — immediately terminate another process
        //   rdi = target PID
        // Returns 0 on success, MAX on error.
        14 => {
            let target_id = crate::task::TaskId::from_raw(regs.rdi);
            // BUG M-1: Refuse to kill kernel/idle system tasks (ids 0 and 1).
            if target_id.raw() < 2 {
                regs.rax = u64::MAX;
                return;
            }
            if crate::scheduler::kill_task(target_id).is_ok() {
                regs.rax = 0;
            } else {
                regs.rax = u64::MAX;
            }
        }
        // SYS_OPEN — open a file by path
        //   rdi = path ptr, rsi = path len, rdx = flags
        // Returns fd, or MAX on error
        15 => {
            let flags = regs.rdx as u32;
            let path_bytes = match copy_from_user(regs.rdi, regs.rsi) {
                Ok(s) => s,
                Err(_) => {
                    regs.rax = u64::MAX - 3;
                    return;
                }
            };
            if let Ok(path) = core::str::from_utf8(&path_bytes) {
                if let Some(inode) = crate::vfs::open_path(path) {
                    let file = crate::vfs::File::new(inode, flags);
                    let mut fd_allocated = false;
                    crate::scheduler::with_current_task_mut(|task| {
                        for (i, fd_slot) in task.fds.iter_mut().enumerate() {
                            if fd_slot.is_none() {
                                *fd_slot = Some(alloc::sync::Arc::new(spin::Mutex::new(file)));
                                regs.rax = i as u64;
                                fd_allocated = true;
                                break;
                            }
                        }
                    });
                    if !fd_allocated {
                        regs.rax = u64::MAX;
                    }
                } else {
                    regs.rax = u64::MAX - 1;
                }
            } else {
                regs.rax = u64::MAX - 2;
            }
        }
        // SYS_READ — read from a file descriptor
        //   rdi = fd, rsi = buf ptr, rdx = buf len
        16 => {
            let fd = regs.rdi as usize;
            let len = match usize::try_from(regs.rdx) {
                Ok(len) => len,
                Err(_) => {
                    regs.rax = u64::MAX;
                    return;
                }
            };
            if validate_user_range(regs.rsi, regs.rdx, true).is_err() {
                regs.rax = u64::MAX;
                return;
            }

            let file_arc = crate::scheduler::with_current_task(|task| {
                if fd < task.fds.len() {
                    task.fds[fd].clone()
                } else {
                    None
                }
            })
            .flatten();

            if let Some(file_mutex) = file_arc {
                let mut file = file_mutex.lock();
                let mut buf = Vec::new();
                if buf.try_reserve_exact(len).is_err() {
                    regs.rax = u64::MAX;
                    return;
                }
                buf.resize(len, 0);
                let bytes_read = file.read(&mut buf);
                if copy_to_user(regs.rsi, &buf[..bytes_read]).is_err() {
                    regs.rax = u64::MAX;
                    return;
                }
                regs.rax = bytes_read as u64;
            } else {
                regs.rax = u64::MAX;
            }
        }
        // SYS_WRITE — write to a file descriptor
        //   rdi = fd, rsi = buf ptr, rdx = buf len
        17 => {
            let fd = regs.rdi as usize;
            let buf = match copy_from_user(regs.rsi, regs.rdx) {
                Ok(s) => s,
                Err(_) => {
                    regs.rax = u64::MAX;
                    return;
                }
            };

            let file_arc = crate::scheduler::with_current_task(|task| {
                if fd < task.fds.len() {
                    task.fds[fd].clone()
                } else {
                    None
                }
            })
            .flatten();

            if let Some(file_mutex) = file_arc {
                let mut file = file_mutex.lock();
                let bytes_written = file.write(&buf);
                regs.rax = bytes_written as u64;
            } else if fd == 1 || fd == 2 {
                // Fallback for stdout/stderr if not piped: print to kernel serial console
                if let Ok(s) = core::str::from_utf8(&buf) {
                    crate::serial_print!("{}", s);
                } else {
                    for b in &buf {
                        crate::serial_print!("{}", *b as char);
                    }
                }
                regs.rax = buf.len() as u64;
            } else {
                regs.rax = u64::MAX;
            }
        }
        // SYS_CLOSE — close a file descriptor
        //   rdi = fd
        18 => {
            let fd = regs.rdi as usize;
            let mut closed = false;
            crate::scheduler::with_current_task_mut(|task| {
                if fd < task.fds.len() && task.fds[fd].is_some() {
                    task.fds[fd] = None;
                    closed = true;
                }
            });
            regs.rax = if closed { 0 } else { u64::MAX };
        }
        // SYS_MMAP — map anonymous memory
        //   rdi = requested virtual address
        //   rsi = length in bytes
        19 => {
            let vaddr = regs.rdi;
            let len = regs.rsi;

            let mut start = vaddr & !0xFFF;
            // Round length up to a page without wrapping (a len near u64::MAX
            // would otherwise wrap the size DOWN). Reject overflow.
            let size = match len.checked_add(0xFFF) {
                Some(s) => s & !0xFFF,
                None => {
                    regs.rax = u64::MAX;
                    return;
                }
            };

            // Simple bump allocator for anonymous user mappings when vaddr is 0
            if start == 0 {
                static NEXT_USER_MMAP: core::sync::atomic::AtomicU64 =
                    core::sync::atomic::AtomicU64::new(0x0000_1000_0000_0000);
                start = NEXT_USER_MMAP.fetch_add(size as u64, core::sync::atomic::Ordering::SeqCst);
            }

            // BUG H-1: Prevent mapping into kernel space.
            const USER_SPACE_END: u64 = 0x0000_8000_0000_0000;
            if start
                .checked_add(size)
                .map_or(true, |end| end > USER_SPACE_END)
            {
                regs.rax = u64::MAX;
                return;
            }

            use crate::arch::mmu::{PageFlags, PageProt};
            use crate::arch::VirtAddr;
            use x86_64::structures::paging::Page;
            let mut alloc = crate::memory::GlobalFrameAllocator;

            // Anonymous user mapping: present, writable, user-accessible (the
            // native SYS_MMAP default — RW data). Targets the CALLER's own user
            // mapping → current_user() (active CR3), NOT kernel() (§10.2). The
            // seam's map_page delegates to map_page_in_pml4_fallible.
            let page_flags =
                PageFlags::new(PageProt::PRESENT | PageProt::WRITABLE | PageProt::USER);
            let mut aspace = crate::arch::mmu::current_user();

            let mut pt = crate::memory::active_page_table();
            let mut mapped = 0u64; // bytes successfully mapped THIS call
            let mut success = true;
            for offset in (0..size).step_by(4096) {
                let v = VirtAddr::new(start + offset);
                use x86_64::structures::paging::FrameAllocator;
                if let Some(frame) = alloc.allocate_frame() {
                    let frame_pa = frame.start_address();
                    if aspace.map_page(v, frame_pa, page_flags).is_ok() {
                        unsafe {
                            let phys_offset = *crate::memory::PHYS_MEM_OFFSET.get().unwrap();
                            let ptr = (phys_offset + frame_pa.as_u64()).as_mut_ptr::<u8>();
                            core::ptr::write_bytes(ptr, 0, 4096);
                        }
                        mapped += 4096;
                        continue;
                    }
                    // map_page failed — the frame we pulled is orphaned;
                    // give it back before we unwind the rest.
                    use x86_64::structures::paging::FrameDeallocator;
                    unsafe { crate::memory::GlobalFrameAllocator.deallocate_frame(frame) };
                }
                success = false;
                break;
            }
            if success {
                regs.rax = start;
            } else {
                // Roll back every page we mapped THIS call so a mid-loop
                // failure doesn't leak frames until task exit (mirror SYS_MUNMAP).
                for offset in (0..mapped).step_by(4096) {
                    let page = Page::containing_address(VirtAddr::new(start + offset));
                    use x86_64::structures::paging::{FrameDeallocator, Mapper};
                    unsafe {
                        if let Ok((frame, flush)) = pt.unmap(page) {
                            flush.ignore();
                            crate::memory::GlobalFrameAllocator.deallocate_frame(frame);
                        }
                    }
                }
                x86_64::instructions::tlb::flush_all();
                regs.rax = u64::MAX;
            }
        }
        // SYS_MUNMAP — unmap anonymous memory
        //   rdi = requested virtual address
        //   rsi = length in bytes
        20 => {
            let vaddr = regs.rdi;
            let len = regs.rsi;

            let start = vaddr & !0xFFF;
            // Round up without wrapping (see SYS_MMAP). Reject overflow.
            let size = match len.checked_add(0xFFF) {
                Some(s) => s & !0xFFF,
                None => {
                    regs.rax = u64::MAX;
                    return;
                }
            };

            // BUG H-1: Prevent unmapping kernel space.
            const USER_SPACE_END: u64 = 0x0000_8000_0000_0000;
            if start
                .checked_add(size)
                .map_or(true, |end| end > USER_SPACE_END)
            {
                regs.rax = u64::MAX;
                return;
            }

            use crate::arch::VirtAddr;
            use x86_64::structures::paging::Page;

            let mut pt = crate::memory::active_page_table();
            for offset in (0..size).step_by(4096) {
                let page = Page::containing_address(VirtAddr::new(start + offset));
                use x86_64::structures::paging::Mapper;
                if let Ok((frame, flush)) = pt.unmap(page) {
                    flush.ignore();
                    use x86_64::structures::paging::FrameDeallocator;
                    unsafe { crate::memory::GlobalFrameAllocator.deallocate_frame(frame) };
                }
            }
            x86_64::instructions::tlb::flush_all();
            regs.rax = 0;
        }
        // SYS_SETPRIORITY — change the priority of a task
        //   rdi = target task id (0 for self)
        //   rsi = 0 for Normal, 1 for Game
        21 => {
            let target = if regs.rdi == 0 {
                crate::scheduler::current_task_id().unwrap_or(crate::task::TaskId::new())
            } else {
                crate::task::TaskId::from_raw(regs.rdi)
            };

            let prio = if regs.rsi == 1 {
                crate::task::TaskPriority::Game
            } else {
                crate::task::TaskPriority::Normal
            };

            // BUG M-2: Only allow a task to promote ITSELF to Game priority.
            // Promoting another task to Game requires privilege we don't model yet.
            let caller = crate::scheduler::current_task_id();
            let self_change = regs.rdi == 0 || caller.map_or(false, |c| c.raw() == regs.rdi);
            if !self_change && prio == crate::task::TaskPriority::Game {
                // Cannot promote another task to Game without privilege.
                regs.rax = u64::MAX;
                return;
            }

            if crate::scheduler::set_priority(target, prio).is_ok() {
                regs.rax = 0;
            } else {
                regs.rax = u64::MAX;
            }
        }
        // SYS_DEBUG_PRINT (ath_abi v2 = 141) — Print a string from userspace
        // to kernel serial (for debugging only).
        //   rdi = ptr, rsi = len
        //
        // Was syscall 27 in ABI v1. That collided with SYS_SURFACE_CLOSE (the
        // compositor close-surface arm further down), and because Rust match
        // dispatches on first-arm-wins, SYS_SURFACE_CLOSE was dead code while
        // every relibc printf hit this arm with a string. Moved to 141 in v2
        // (components/ath_abi/src/lib.rs::syscall::SYS_DEBUG_PRINT); relibc
        // was updated in the same commit.
        141 => {
            let ptr = regs.rdi;
            let len = regs.rsi as usize;
            if len > 4096 {
                regs.rax = u64::MAX;
                return;
            }
            if let Ok(buf) = copy_from_user(ptr, len as u64) {
                if let Ok(s) = core::str::from_utf8(&buf) {
                    crate::serial_print!("{}", s);
                } else {
                    for b in buf {
                        crate::serial_print!("{}", b as char);
                    }
                }
                regs.rax = len as u64;
            } else {
                regs.rax = u64::MAX;
            }
        }
        // SYS_SEEK — set the file position for a file descriptor.
        //   rdi = fd, rsi = new absolute offset (bytes from start)
        // Returns the new offset, or u64::MAX on bad fd.
        22 => {
            let fd = regs.rdi as usize;
            let new_offset = regs.rsi as usize;

            let file_arc = crate::scheduler::with_current_task(|task| {
                if fd < task.fds.len() {
                    task.fds[fd].clone()
                } else {
                    None
                }
            })
            .flatten();

            if let Some(file_mutex) = file_arc {
                let mut file = file_mutex.lock();
                regs.rax = file.seek(new_offset) as u64;
            } else {
                regs.rax = u64::MAX;
            }
        }
        // SYS_STAT — report the size of an open file.
        //   rdi = fd
        // Returns the size in bytes, or u64::MAX on bad fd.
        23 => {
            let fd = regs.rdi as usize;

            let file_arc = crate::scheduler::with_current_task(|task| {
                if fd < task.fds.len() {
                    task.fds[fd].clone()
                } else {
                    None
                }
            })
            .flatten();

            if let Some(file_mutex) = file_arc {
                let file = file_mutex.lock();
                regs.rax = file.inode.size() as u64;
            } else {
                regs.rax = u64::MAX;
            }
        }
        // SYS_SURFACE_CREATE — register a new compositor surface.
        //   rdi = width  (pixels)
        //   rsi = height (pixels)
        //   rdx = user virtual base (page-aligned, in lower half) where the
        //         kernel will map the surface bytes (ARGB8888, w*h*4 bytes).
        // Returns surface id, or u64::MAX on failure.
        24 => {
            let width = regs.rdi as u32;
            let height = regs.rsi as u32;
            let uvirt = regs.rdx;
            match crate::compositor::create_surface(width, height, uvirt) {
                Some(id) => {
                    regs.rax = id;
                }
                None => {
                    regs.rax = u64::MAX;
                }
            }
        }
        // SYS_SURFACE_PRESENT — blit the surface to (x, y) on screen.
        //   rdi = surface id (from CREATE)
        //   rsi = x (signed; may be negative for off-screen clip)
        //   rdx = y
        // Returns 0 on success or u64::MAX.
        25 => {
            let id = regs.rdi;
            let x = regs.rsi as i32;
            let y = regs.rdx as i32;
            match crate::compositor::present_surface(id, x, y) {
                Ok(()) => {
                    regs.rax = 0;
                }
                Err(()) => {
                    regs.rax = u64::MAX;
                }
            }
        }
        // SYS_SURFACE_FOCUS — bring a surface to the front.
        //   rdi = surface id
        26 => {
            let id = regs.rdi;
            match crate::compositor::focus_surface(id) {
                Ok(()) => {
                    regs.rax = 0;
                }
                Err(()) => {
                    regs.rax = u64::MAX;
                }
            }
        }
        // SYS_SURFACE_CLOSE — destroy a surface.
        //   rdi = surface id
        27 => {
            let id = regs.rdi;
            match crate::compositor::close_surface(id) {
                Ok(()) => {
                    regs.rax = 0;
                }
                Err(()) => {
                    regs.rax = u64::MAX;
                }
            }
        }
        // SYS_YIELD — voluntarily relinquish the CPU.
        28 => {
            crate::scheduler::yield_task();
        }
        // SYS_GETPID — return the current task's ID.
        29 => {
            regs.rax = crate::scheduler::current_task_id()
                .map(|id| id.raw())
                .unwrap_or(u64::MAX);
        }
        // SYS_TIME — return a monotonic nanosecond timestamp.
        30 => {
            let jiffies = crate::timers::JIFFIES.load(core::sync::atomic::Ordering::Relaxed);
            regs.rax = crate::timers::jiffies_to_ns(jiffies);
        }
        // SYS_READ_KEY — pop a scancode from the current task's keyboard buffer.
        // Returns the scancode, or 0 if the buffer is empty.
        31 => {
            regs.rax =
                crate::scheduler::with_current_task_mut(|task| task.pop_key().unwrap_or(0) as u64)
                    .unwrap_or(0);
        }
        // SYS_POLL_MOUSE — pop a mouse event from the current task's buffer.
        // Returns a packed u64: bits [7:0] = buttons, [23:8] = dx (i16),
        // [39:24] = dy (i16). Returns 0 if empty.
        32 => {
            regs.rax = crate::scheduler::with_current_task_mut(|task| {
                task.pop_mouse_packed().unwrap_or(0)
            })
            .unwrap_or(0);
        }
        // SYS_READDIR — list files from the VFS root into a user buffer.
        //   rdi = user buffer pointer
        //   rsi = buffer length in bytes
        // Writes entries as: [name_len: u16][size: u32][name: u8 * name_len]
        // Returns the number of entries written, or u64::MAX on error.
        33 => {
            let buf_ptr = regs.rdi;
            let buf_len = regs.rsi;
            if validate_user_range(buf_ptr, buf_len, true).is_err() {
                regs.rax = u64::MAX;
                return;
            }
            let entries = crate::vfs::list_dir_at("/");
            // SMAP-safe: assemble kernel-side, then one validated extable copy
            // (the raw per-entry copy_nonoverlapping into the user buffer was
            // the TOCTOU/SMAP-exposed pattern).
            let buf_len = buf_len as usize;
            let mut out: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
            let mut count: u64 = 0;
            for entry in &entries {
                let name_bytes = entry.name.as_bytes();
                let entry_size = 2 + 4 + name_bytes.len();
                if out.len() + entry_size > buf_len {
                    break;
                }
                out.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
                out.extend_from_slice(&(entry.size as u32).to_le_bytes());
                out.extend_from_slice(name_bytes);
                count += 1;
            }
            regs.rax = if copy_to_user(buf_ptr, &out).is_ok() {
                count
            } else {
                u64::MAX
            };
        }
        // SYS_SCREEN_INFO — return screen dimensions.
        //   Returns width in rdi, height in rsi. rax = 0 on success.
        34 => {
            if let Some((w, h)) = crate::compositor::screen_dimensions() {
                regs.rax = 0;
                regs.rdi = w as u64;
                regs.rsi = h as u64;
            } else {
                regs.rax = u64::MAX;
            }
        }
        // SYS_PTY_OPEN — allocate a new PTY pair. Returns master id.
        35 => {
            let mut sub = crate::tty::TTY_SUBSYSTEM.lock();
            match sub.as_mut() {
                Some(s) => match s.pty_manager.allocate() {
                    Ok((master_id, _)) => {
                        let _ = s.pty_manager.unlock(master_id);
                        regs.rax = master_id as u64;
                    }
                    Err(_) => regs.rax = u64::MAX,
                },
                None => regs.rax = u64::MAX,
            }
        }
        // SYS_PTY_READ — read shell output from master (slave wrote it).
        //   rdi = pty id, rsi = user buf, rdx = buflen → bytes read or MAX.
        36 => {
            let id = regs.rdi as u32;
            let buf_ptr = regs.rsi;
            let buf_len = regs.rdx as usize;
            if validate_user_range(buf_ptr, buf_len as u64, true).is_err() {
                regs.rax = u64::MAX;
                return;
            }
            let mut tmp = alloc::vec![0u8; buf_len];
            let mut sub = crate::tty::TTY_SUBSYSTEM.lock();
            match sub.as_mut() {
                Some(s) => match s.pty_manager.master_read(id, &mut tmp) {
                    Ok(n) => {
                        let _ = copy_to_user(buf_ptr, &tmp[..n]);
                        regs.rax = n as u64;
                    }
                    Err(crate::tty::TtyError::WouldBlock) => regs.rax = 0,
                    Err(_) => regs.rax = u64::MAX,
                },
                None => regs.rax = u64::MAX,
            }
        }
        // SYS_PTY_WRITE — write user keystrokes to master (slave reads it).
        //   rdi = pty id, rsi = user buf, rdx = buflen → bytes written or MAX.
        37 => {
            let id = regs.rdi as u32;
            let data = match copy_from_user(regs.rsi, regs.rdx) {
                Ok(d) => d,
                Err(_) => {
                    regs.rax = u64::MAX;
                    return;
                }
            };
            let mut sub = crate::tty::TTY_SUBSYSTEM.lock();
            regs.rax = match sub.as_mut() {
                Some(s) => match s.pty_manager.master_write(id, &data) {
                    Ok(n) => n as u64,
                    Err(_) => u64::MAX,
                },
                None => u64::MAX,
            };
        }
        // SYS_PTY_POLL — bytes available to read on master. rdi = pty id.
        38 => {
            let id = regs.rdi as u32;
            let mut sub = crate::tty::TTY_SUBSYSTEM.lock();
            regs.rax = match sub
                .as_ref()
                .and_then(|s| s.pty_manager.master_pending(id).ok())
            {
                Some(n) => n as u64,
                None => 0,
            };
        }
        // SYS_PTY_SLAVE_READ / WRITE — use current task's bound pty slave.
        //   rdi = 0 read, 1 write; rsi = buf; rdx = len
        39 => {
            let is_write = regs.rdi != 0;
            let buf_ptr = regs.rsi;
            let buf_len = regs.rdx;
            let pty_id = match crate::scheduler::with_current_task(|t| t.pty_slave_id) {
                Some(Some(id)) => id,
                _ => {
                    regs.rax = u64::MAX;
                    return;
                }
            };
            if is_write {
                let data = match copy_from_user(buf_ptr, buf_len) {
                    Ok(d) => d,
                    Err(_) => {
                        regs.rax = u64::MAX;
                        return;
                    }
                };
                let mut sub = crate::tty::TTY_SUBSYSTEM.lock();
                regs.rax = match sub.as_mut() {
                    Some(s) => match s.pty_manager.slave_write(pty_id, &data) {
                        Ok(n) => n as u64,
                        Err(_) => u64::MAX,
                    },
                    None => u64::MAX,
                };
            } else {
                let len = buf_len as usize;
                if validate_user_range(buf_ptr, buf_len, true).is_err() {
                    regs.rax = u64::MAX;
                    return;
                }
                let mut tmp = alloc::vec![0u8; len];
                let mut sub = crate::tty::TTY_SUBSYSTEM.lock();
                match sub.as_mut() {
                    Some(s) => match s.pty_manager.slave_read(pty_id, &mut tmp) {
                        Ok(n) => {
                            let _ = copy_to_user(buf_ptr, &tmp[..n]);
                            regs.rax = n as u64;
                        }
                        Err(crate::tty::TtyError::WouldBlock) => regs.rax = 0,
                        Err(_) => regs.rax = u64::MAX,
                    },
                    None => regs.rax = u64::MAX,
                }
            }
        }
        // ── Embodiment-first syscalls (40-49) ── see kernel/src/game_session.rs
        // SYS_WALL_CLOCK — unix-epoch nanoseconds.
        40 => {
            regs.rax = crate::game_session::sys_wall_clock();
        }
        // SYS_GAME_MODE_ENTER
        41 => {
            regs.rax = crate::game_session::sys_game_mode_enter();
        }
        // SYS_GAME_MODE_EXIT
        42 => {
            regs.rax = crate::game_session::sys_game_mode_exit();
        }
        // SYS_GAME_MODE_STATUS — bit0=active, bits[31:8]=throttle ratio
        43 => {
            regs.rax = crate::game_session::sys_game_mode_status();
        }
        // SYS_NULL_LATENCY_ENTER — rdi = task id (0 = self)
        44 => {
            regs.rax = crate::game_session::sys_null_latency_enter(regs.rdi);
        }
        // SYS_NULL_LATENCY_EXIT
        45 => {
            regs.rax = crate::game_session::sys_null_latency_exit();
        }
        // SYS_PIN_MEMORY — rdi = virt addr, rsi = byte length
        46 => {
            regs.rax = crate::game_session::sys_pin_memory(regs.rdi, regs.rsi);
        }
        // SYS_UNPIN_MEMORY — rdi = virt addr, rsi = byte length
        47 => {
            regs.rax = crate::game_session::sys_unpin_memory(regs.rdi, regs.rsi);
        }
        // SYS_DEADLINE_STATS — rdi = user buffer (≥ 32 bytes), rsi = len
        48 => {
            let buf_ptr = regs.rdi;
            let buf_len = regs.rsi;
            regs.rax = crate::game_session::sys_deadline_stats(buf_ptr, buf_len, |p, l, w| {
                validate_user_range(p, l, w).is_ok()
            });
        }
        // SYS_PERF_TSC — raw RDTSC cycle counter
        49 => {
            regs.rax = crate::game_session::sys_perf_tsc();
        }
        // ── Versioned config registry (50-53) ── Concept §Windows pain
        // points: the "registry is a graveyard" rebuttal.
        50 => {
            regs.rax = crate::config_registry::sys_config_get(
                regs.rdi,
                regs.rsi,
                regs.rdx,
                regs.r10,
                |p, l, w| validate_user_range(p, l, w).is_ok(),
                |p, l, w| validate_user_range(p, l, w).is_ok(),
            );
        }
        51 => {
            regs.rax = crate::config_registry::sys_config_set(
                regs.rdi,
                regs.rsi,
                regs.rdx,
                regs.r10,
                |p, l, w| validate_user_range(p, l, w).is_ok(),
            );
        }
        52 => {
            regs.rax = crate::config_registry::sys_config_snapshot();
        }
        53 => {
            regs.rax = crate::config_registry::sys_config_rollback(regs.rdi);
        }
        // ── Local-first search index (54-57) ── Concept §Windows pain
        // points: the "search is broken" rebuttal.
        54 => {
            regs.rax =
                crate::search_index::sys_search_add(regs.rdi, regs.rsi, regs.rdx, |p, l, w| {
                    validate_user_range(p, l, w).is_ok()
                });
        }
        55 => {
            regs.rax = crate::search_index::sys_search_remove(regs.rdi);
        }
        56 => {
            regs.rax = crate::search_index::sys_search_query(
                regs.rdi,
                regs.rsi,
                regs.rdx,
                regs.r10,
                |p, l, w| validate_user_range(p, l, w).is_ok(),
                |p, l, w| validate_user_range(p, l, w).is_ok(),
            );
        }
        57 => {
            regs.rax = crate::search_index::sys_search_stats(regs.rdi, regs.rsi, |p, l, w| {
                validate_user_range(p, l, w).is_ok()
            });
        }
        // ── Per-game profiles (58-61) ── Concept §Gaming Features.
        58 => {
            regs.rax = crate::game_profile::sys_set(regs.rdi, regs.rsi, regs.rdx, |p, l, w| {
                validate_user_range(p, l, w).is_ok()
            });
        }
        59 => {
            regs.rax = crate::game_profile::sys_get(
                regs.rdi,
                regs.rsi,
                regs.rdx,
                |p, l, w| validate_user_range(p, l, w).is_ok(),
                |p, l, w| validate_user_range(p, l, w).is_ok(),
            );
        }
        60 => {
            regs.rax = crate::game_profile::sys_apply(regs.rdi, regs.rsi, |p, l, w| {
                validate_user_range(p, l, w).is_ok()
            });
        }
        61 => {
            regs.rax = crate::game_profile::sys_list(regs.rdi, regs.rsi, |p, l, w| {
                validate_user_range(p, l, w).is_ok()
            });
        }
        // ── Unified RGB (62-65) ── Concept §Customization Engine.
        62 => {
            regs.rax = crate::rgb::sys_list(regs.rdi, regs.rsi, |p, l, w| {
                validate_user_range(p, l, w).is_ok()
            });
        }
        63 => {
            regs.rax = crate::rgb::sys_query(regs.rdi, regs.rsi, regs.rdx, |p, l, w| {
                validate_user_range(p, l, w).is_ok()
            });
        }
        64 => {
            regs.rax = crate::rgb::sys_set(regs.rdi, regs.rsi, regs.rdx, regs.r10);
        }
        65 => {
            regs.rax = crate::rgb::sys_effect(regs.rdi, regs.rsi, regs.rdx, regs.r10);
        }
        // ── App bundle manifest verifier (66-67) ── Concept §Windows pain points.
        66 => {
            regs.rax = crate::app_bundle::sys_verify(regs.rdi, regs.rsi, |p, l, w| {
                validate_user_range(p, l, w).is_ok()
            });
        }
        67 => {
            regs.rax = crate::app_bundle::sys_register(
                regs.rdi,
                regs.rsi,
                regs.rdx,
                regs.r10,
                |p, l, w| validate_user_range(p, l, w).is_ok(),
            );
        }
        // ── Compositor capture (68-70) ── Concept §Gaming Features:
        // "Capture & stream at the compositor — zero-cost recording, no
        //  OBS overhead." The compositor module already exposes
        // start_capture/stop_capture/read_capture — this is the userspace
        // ABI on top of it.
        //
        // SYS_CAPTURE_BEGIN — rdi=region_xy (x | y<<32), rsi=region_wh
        //                     (w | h<<32), rdx=format (0=ARGB, 1=BGRA),
        //                     r10=continuous(0/1) → session_id
        68 => {
            let rx = (regs.rdi & 0xFFFF_FFFF) as u32;
            let ry = ((regs.rdi >> 32) & 0xFFFF_FFFF) as u32;
            let rw = (regs.rsi & 0xFFFF_FFFF) as u32;
            let rh = ((regs.rsi >> 32) & 0xFFFF_FFFF) as u32;
            let fmt = if regs.rdx == 1 {
                crate::compositor::CaptureFormat::Bgra32
            } else {
                crate::compositor::CaptureFormat::Argb32
            };
            let cont = regs.r10 != 0;
            regs.rax = crate::compositor::start_capture(rx, ry, rw, rh, fmt, cont);
        }
        // SYS_CAPTURE_END — rdi=session_id
        69 => {
            crate::compositor::stop_capture(regs.rdi);
            regs.rax = 0;
        }
        // SYS_CAPTURE_READ — rdi=session_id, rsi=out_ptr, rdx=out_cap_bytes
        // Returns bytes copied, or 0 if no frame / bad session.
        70 => {
            let out_ptr = regs.rsi;
            let out_cap = regs.rdx;
            if out_cap > 0 && validate_user_range(out_ptr, out_cap, true).is_err() {
                regs.rax = 0;
            } else if let Some((pixels, _w, _h)) = crate::compositor::read_capture(regs.rdi) {
                let byte_len = pixels.len().saturating_mul(4) as u64;
                let n = core::cmp::min(byte_len, out_cap) as usize;
                // SMAP-safe: one validated extable copy of the kernel-side
                // pixel buffer (was a raw copy_nonoverlapping to the user ptr).
                let bytes = unsafe { core::slice::from_raw_parts(pixels.as_ptr() as *const u8, n) };
                regs.rax = if copy_to_user(out_ptr, bytes).is_ok() {
                    n as u64
                } else {
                    0
                };
            } else {
                regs.rax = 0;
            }
        }
        // ── Permission prompt queue (71-73) ── Concept §Security.
        71 => {
            regs.rax = crate::perm_syscalls::sys_perm_list(regs.rdi, regs.rsi, |p, l, w| {
                validate_user_range(p, l, w).is_ok()
            });
        }
        72 => {
            regs.rax = crate::perm_syscalls::sys_perm_respond(regs.rdi, regs.rsi);
        }
        73 => {
            regs.rax = crate::perm_syscalls::sys_perm_stats(regs.rdi, regs.rsi, |p, l, w| {
                validate_user_range(p, l, w).is_ok()
            });
        }
        // ── Theme engine (74-77) ── Concept §Customization Engine.
        74 => {
            regs.rax = crate::theme_engine::sys_list(regs.rdi, regs.rsi, |p, l, w| {
                validate_user_range(p, l, w).is_ok()
            });
        }
        75 => {
            regs.rax = crate::theme_engine::sys_query(regs.rdi, regs.rsi, regs.rdx, |p, l, w| {
                validate_user_range(p, l, w).is_ok()
            });
        }
        76 => {
            regs.rax = crate::theme_engine::sys_apply(regs.rdi);
        }
        77 => {
            regs.rax = crate::theme_engine::sys_register(regs.rdi, regs.rsi, |p, l, w| {
                validate_user_range(p, l, w).is_ok()
            });
        }
        // ── Rae scripting (78-80) ──
        78 => {
            regs.rax = crate::scripting::sys_run(regs.rdi, regs.rsi, regs.rdx, |p, l, w| {
                validate_user_range(p, l, w).is_ok()
            });
        }
        79 => {
            regs.rax = crate::scripting::sys_status(regs.rdi, regs.rsi, regs.rdx, |p, l, w| {
                validate_user_range(p, l, w).is_ok()
            });
        }
        80 => {
            regs.rax = crate::scripting::sys_kill(regs.rdi);
        }
        // ── WireGuard (81-84) ── Concept §AthNet.
        81 => {
            regs.rax = crate::wireguard::sys_list(regs.rdi, regs.rsi, |p, l, w| {
                validate_user_range(p, l, w).is_ok()
            });
        }
        82 => {
            regs.rax =
                crate::wireguard::sys_add(regs.rdi, regs.rsi, regs.rdx, regs.r10, |p, l, w| {
                    validate_user_range(p, l, w).is_ok()
                });
        }
        83 => {
            regs.rax = crate::wireguard::sys_remove(regs.rdi);
        }
        84 => {
            regs.rax = crate::wireguard::sys_stats(regs.rdi, regs.rsi, regs.rdx, |p, l, w| {
                validate_user_range(p, l, w).is_ok()
            });
        }
        // ── Live wallpaper engine (85-87) ── Concept §Customization Engine.
        85 => {
            regs.rax = crate::live_wallpaper::sys_list(regs.rdi, regs.rsi, |p, l, w| {
                validate_user_range(p, l, w).is_ok()
            });
        }
        86 => {
            regs.rax = crate::live_wallpaper::sys_set(regs.rdi);
        }
        87 => {
            regs.rax = crate::live_wallpaper::sys_status(regs.rdi, regs.rsi, |p, l, w| {
                validate_user_range(p, l, w).is_ok()
            });
        }
        // ── AthID session (88-93) ── Concept §AthID / login gate.
        // SYS_SESSION_LOGIN — rdi=user, rsi=user_len, rdx=pass, r10=pass_len → 0 ok, 1 fail
        88 => {
            let user = match copy_from_user(regs.rdi, regs.rsi) {
                Ok(b) => b,
                Err(_) => {
                    regs.rax = u64::MAX;
                    return;
                }
            };
            let pass = match copy_from_user(regs.rdx, regs.r10) {
                Ok(b) => b,
                Err(_) => {
                    regs.rax = u64::MAX;
                    return;
                }
            };
            match core::str::from_utf8(&user) {
                Ok(u) => {
                    regs.rax = if crate::session::login_password(u, &pass) {
                        0
                    } else {
                        1
                    };
                }
                Err(_) => regs.rax = u64::MAX,
            }
        }
        // SYS_SESSION_GUEST
        89 => {
            regs.rax = if crate::session::login_guest() {
                0
            } else {
                u64::MAX
            };
        }
        // SYS_SESSION_LOCK
        90 => {
            crate::session::lock();
            regs.rax = 0;
        }
        // SYS_SESSION_UNLOCK — rdx=pass ptr, r10=pass len
        91 => {
            let pass = match copy_from_user(regs.rdx, regs.r10) {
                Ok(b) => b,
                Err(_) => {
                    regs.rax = u64::MAX;
                    return;
                }
            };
            regs.rax = if crate::session::unlock_password(&pass) {
                0
            } else {
                1
            };
        }
        // SYS_SESSION_INFO — rdi=buf, rsi=len → bytes written or MAX
        92 => {
            let buf_ptr = regs.rdi;
            let buf_len = regs.rsi as usize;
            if validate_user_range(buf_ptr, buf_len as u64, true).is_err() {
                regs.rax = u64::MAX;
                return;
            }
            let mut tmp = alloc::vec![0u8; buf_len];
            regs.rax = crate::session::write_info(&mut tmp);
            if regs.rax != u64::MAX {
                let n = core::cmp::min(buf_len, regs.rax as usize);
                let _ = copy_to_user(buf_ptr, &tmp[..n]);
            }
        }
        // SYS_SESSION_LOGOUT
        93 => {
            crate::session::logout();
            regs.rax = 0;
        }
        // SYS_READDIR_AT — rdi=path, rsi=path_len, rdx=buf, r10=buf_len
        95 => {
            let path = match copy_from_user(regs.rdi, regs.rsi) {
                Ok(b) => b,
                Err(_) => {
                    regs.rax = u64::MAX;
                    return;
                }
            };
            let buf_ptr = regs.rdx;
            let buf_len = regs.r10 as usize;
            if validate_user_range(buf_ptr, buf_len as u64, true).is_err() {
                regs.rax = u64::MAX;
                return;
            }
            let path_str = match core::str::from_utf8(&path) {
                Ok(s) => s,
                Err(_) => {
                    regs.rax = u64::MAX;
                    return;
                }
            };
            let entries = crate::vfs::list_dir_at(path_str);
            // SMAP-safe: kernel-side assembly + one validated extable copy.
            let mut out: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
            let mut count: u64 = 0;
            for entry in &entries {
                let name_bytes = entry.name.as_bytes();
                let entry_size = 2 + 4 + name_bytes.len();
                if out.len() + entry_size > buf_len {
                    break;
                }
                out.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
                out.extend_from_slice(&(entry.size as u32).to_le_bytes());
                out.extend_from_slice(name_bytes);
                count += 1;
            }
            regs.rax = if copy_to_user(buf_ptr, &out).is_ok() {
                count
            } else {
                u64::MAX
            };
        }
        // SYS_PROCLIST — rdi=buf, rsi=len; each entry is 24 bytes:
        // [pid: u64][state: u8][priority: u8][pad: u16][vruntime: u64]
        94 => {
            let buf_ptr = regs.rdi;
            let buf_len = regs.rsi as usize;
            if validate_user_range(buf_ptr, buf_len as u64, true).is_err() {
                regs.rax = u64::MAX;
                return;
            }
            let tasks = crate::scheduler::list_task_summaries();
            // SMAP-safe: kernel-side assembly + one validated extable copy.
            const ENTRY: usize = 24;
            let mut out: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
            let mut count = 0u64;
            for t in &tasks {
                if out.len() + ENTRY > buf_len {
                    break;
                }
                out.extend_from_slice(&t.id.to_le_bytes());
                out.push(t.state);
                out.push(t.priority);
                out.extend_from_slice(&[0u8; 6]); // pad to the vruntime field
                out.extend_from_slice(&t.vruntime.to_le_bytes());
                count += 1;
            }
            regs.rax = if copy_to_user(buf_ptr, &out).is_ok() {
                count
            } else {
                u64::MAX
            };
        }
        // SYS_MKDIR — rdi=path_ptr, rsi=path_len, rdx=mode (ignored)
        96 => {
            if !has_filesystem_write_cap_if_declared() {
                note_guard_fs_cap_reject();
                regs.rax = crate::capability::E_RIGHTS;
                return;
            }
            if regs.rsi == 0 || regs.rsi > MAX_SYSCALL_PATH_BYTES {
                note_guard_bounds_reject();
                regs.rax = u64::MAX;
                return;
            }
            let path = match copy_from_user(regs.rdi, regs.rsi) {
                Ok(b) => b,
                Err(_) => {
                    note_guard_ptr_reject();
                    regs.rax = u64::MAX;
                    return;
                }
            };
            match core::str::from_utf8(&path) {
                Ok(p) => {
                    regs.rax = match crate::vfs::mkdir_at(p, regs.rdx as u32) {
                        Ok(()) => 0,
                        Err(e) => e,
                    };
                }
                Err(_) => regs.rax = u64::MAX,
            }
        }
        // SYS_UNLINK — rdi=path_ptr, rsi=path_len
        97 => {
            if !has_filesystem_write_cap_if_declared() {
                note_guard_fs_cap_reject();
                regs.rax = crate::capability::E_RIGHTS;
                return;
            }
            if regs.rsi == 0 || regs.rsi > MAX_SYSCALL_PATH_BYTES {
                note_guard_bounds_reject();
                regs.rax = u64::MAX;
                return;
            }
            let path = match copy_from_user(regs.rdi, regs.rsi) {
                Ok(b) => b,
                Err(_) => {
                    note_guard_ptr_reject();
                    regs.rax = u64::MAX;
                    return;
                }
            };
            match core::str::from_utf8(&path) {
                Ok(p) => {
                    regs.rax = match crate::vfs::unlink_at(p) {
                        Ok(()) => 0,
                        Err(e) => e,
                    };
                }
                Err(_) => regs.rax = u64::MAX,
            }
        }
        // SYS_RENAME — rdi=old_ptr, rsi=old_len, rdx=new_ptr, r10=new_len
        98 => {
            if !has_filesystem_write_cap_if_declared() {
                note_guard_fs_cap_reject();
                regs.rax = crate::capability::E_RIGHTS;
                return;
            }
            if regs.rsi == 0
                || regs.rsi > MAX_SYSCALL_PATH_BYTES
                || regs.r10 == 0
                || regs.r10 > MAX_SYSCALL_PATH_BYTES
            {
                note_guard_bounds_reject();
                regs.rax = u64::MAX;
                return;
            }
            let old = match copy_from_user(regs.rdi, regs.rsi) {
                Ok(b) => b,
                Err(_) => {
                    note_guard_ptr_reject();
                    regs.rax = u64::MAX;
                    return;
                }
            };
            let new = match copy_from_user(regs.rdx, regs.r10) {
                Ok(b) => b,
                Err(_) => {
                    note_guard_ptr_reject();
                    regs.rax = u64::MAX;
                    return;
                }
            };
            match (core::str::from_utf8(&old), core::str::from_utf8(&new)) {
                (Ok(o), Ok(n)) => {
                    regs.rax = match crate::vfs::rename_at(o, n) {
                        Ok(()) => 0,
                        Err(e) => e,
                    };
                }
                _ => regs.rax = u64::MAX,
            }
        }
        // SYS_ATHFS_GAME_INSTALL_HINT — rdi=path_ptr, rsi=path_len, rdx=expected_size
        99 => {
            if !has_filesystem_write_cap_if_declared() {
                note_guard_fs_cap_reject();
                regs.rax = crate::capability::E_RIGHTS;
                return;
            }
            if regs.rsi == 0 || regs.rsi > MAX_SYSCALL_PATH_BYTES {
                note_guard_bounds_reject();
                regs.rax = crate::athfs::E_ATHFS_BAD_PATH;
                return;
            }
            regs.rax = crate::athfs::sys_athfs_game_install_hint(
                regs.rdi,
                regs.rsi,
                regs.rdx,
                |ptr, len| copy_from_user(ptr, len),
            );
        }
        // SYS_OOM_SUBSCRIBE — rdi = IPC channel id to notify on low memory.
        // The kernel pushes an OOM_MSG_LOW_MEMORY message + wakes the receiver
        // before the OOM killer fires. Returns 0. (MasterChecklist Phase 4.1.)
        100 => {
            let chan_id = regs.rdi;
            let pid = crate::scheduler::current_task_id()
                .map(|t| t.raw())
                .unwrap_or(0);
            crate::oom::register_oom_subscriber(pid, chan_id);
            regs.rax = 0;
        }
        // SYS_ATHFS_SNAPSHOT_CREATE — rdi=name_ptr, rsi=name_len.
        // Returns the new snapshot id (>0) or an E_ATHFS_* sentinel. Requires
        // the filesystem write capability (mutates live FS metadata). (5.1)
        101 => {
            if !has_filesystem_write_cap_if_declared() {
                note_guard_fs_cap_reject();
                regs.rax = crate::capability::E_RIGHTS;
                return;
            }
            if regs.rsi > MAX_SYSCALL_PATH_BYTES {
                note_guard_bounds_reject();
                regs.rax = crate::athfs::E_ATHFS_BAD_PATH;
                return;
            }
            // Empty name is allowed (-> recorded as the given empty label).
            let name = match copy_from_user(regs.rdi, regs.rsi) {
                Ok(bytes) => alloc::string::String::from_utf8_lossy(&bytes).into_owned(),
                Err(()) => {
                    note_guard_ptr_reject();
                    regs.rax = crate::athfs::E_ATHFS_BAD_PATH;
                    return;
                }
            };
            regs.rax = crate::athfs::snapshot_create(&name);
        }
        // SYS_ATHFS_SNAPSHOT_RESTORE — rdi=snap_id. Returns 0 or E_ATHFS_*.
        102 => {
            if !has_filesystem_write_cap_if_declared() {
                note_guard_fs_cap_reject();
                regs.rax = crate::capability::E_RIGHTS;
                return;
            }
            regs.rax = crate::athfs::snapshot_restore(regs.rdi as u32);
        }
        // SYS_ATHFS_SNAPSHOT_DELETE — rdi=snap_id. Returns 0 or E_ATHFS_*.
        103 => {
            if !has_filesystem_write_cap_if_declared() {
                note_guard_fs_cap_reject();
                regs.rax = crate::capability::E_RIGHTS;
                return;
            }
            regs.rax = crate::athfs::snapshot_delete(regs.rdi as u32);
        }
        // SYS_CLIPBOARD_GET — rdi=buf_ptr, rsi=buf_len
        107 => {
            let buf_ptr = regs.rdi;
            let buf_len = regs.rsi as usize;
            if regs.rsi > MAX_SYSCALL_CLIPBOARD_BYTES {
                note_guard_bounds_reject();
                regs.rax = u64::MAX;
                return;
            }
            if validate_user_range(buf_ptr, buf_len as u64, true).is_err() {
                note_guard_ptr_reject();
                regs.rax = u64::MAX;
                return;
            }
            let mut tmp = alloc::vec![0u8; buf_len];
            let n = crate::clipboard::get(&mut tmp);
            let _ = copy_to_user(buf_ptr, &tmp[..n]);
            regs.rax = n as u64;
        }
        // SYS_CLIPBOARD_SET — rdi=buf_ptr, rsi=buf_len
        108 => {
            if regs.rsi > MAX_SYSCALL_CLIPBOARD_BYTES {
                note_guard_bounds_reject();
                regs.rax = u64::MAX;
                return;
            }
            let data = match copy_from_user(regs.rdi, regs.rsi) {
                Ok(d) => d,
                Err(_) => {
                    note_guard_ptr_reject();
                    regs.rax = u64::MAX;
                    return;
                }
            };
            regs.rax = match crate::clipboard::set(&data) {
                Ok(()) => 0,
                Err(()) => u64::MAX,
            };
        }
        // ── Userspace driver framework (109-116) ── Concept §Architecture
        // path-C. Driver supervisor registers, claims devices, gets MMIO
        // + IRQ capability handles, and (eventually) sits in an
        // IOMMU-enforced sandbox.
        109 => {
            regs.rax =
                crate::userspace_driver::sys_register(regs.rdi, regs.rsi, regs.rdx, |p, l, w| {
                    validate_user_range(p, l, w).is_ok()
                });
        }
        110 => {
            regs.rax = crate::userspace_driver::sys_unregister(regs.rdi);
        }
        111 => {
            regs.rax = crate::userspace_driver::sys_claim_device(regs.rdi, regs.rsi);
        }
        112 => {
            regs.rax = crate::userspace_driver::sys_release_device(regs.rdi);
        }
        113 => {
            regs.rax = crate::userspace_driver::sys_enable_dma(regs.rdi, regs.rsi);
        }
        114 => {
            regs.rax = crate::userspace_driver::sys_list(regs.rdi, regs.rsi, |p, l, w| {
                validate_user_range(p, l, w).is_ok()
            });
        }
        115 => {
            regs.rax =
                crate::userspace_driver::sys_query(regs.rdi, regs.rsi, regs.rdx, |p, l, w| {
                    validate_user_range(p, l, w).is_ok()
                });
        }
        116 => {
            regs.rax = crate::userspace_driver::sys_deliver_irq_setup(regs.rdi, regs.rsi, regs.rdx);
        }
        117 => {
            regs.rax =
                crate::userspace_driver::sys_dma_map(regs.rdi, regs.rsi, regs.rdx, |p, l, w| {
                    validate_user_range(p, l, w).is_ok()
                });
        }
        118 => {
            regs.rax = crate::userspace_driver::sys_dma_unmap(regs.rdi, regs.rsi);
        }
        // SYS_CHANNEL_SHMEM_MAP (119) — map shared frame of an IPC channel
        //   rdi = channel cap handle
        //   rsi = target virtual address (4KB aligned)
        119 => {
            use crate::capability::{Cap, CapHandle, Rights};
            let cap_handle = CapHandle::from_raw(regs.rdi);
            let target_virt = regs.rsi;

            if target_virt % 4096 != 0 {
                regs.rax = u64::MAX;
                return;
            }

            let mut phys_frame_opt = None;
            let mut pml4_opt = None;
            let mut rights_opt = None;

            crate::scheduler::with_current_task(|task| {
                if let Some(Cap::Channel { chan_id, rights }) = task.cap_table.get(cap_handle) {
                    let ipc = crate::ipc::IPC.lock();
                    phys_frame_opt = ipc.get_channel_shared_frame(chan_id as usize);
                    if phys_frame_opt.is_some() {
                        pml4_opt = task.pml4;
                        rights_opt = Some(rights);
                    }
                }
            });

            if let (Some(phys), Some(pml4), Some(rights)) = (phys_frame_opt, pml4_opt, rights_opt) {
                use crate::arch::{PhysAddr, VirtAddr};
                use x86_64::structures::paging::{Page, PageTableFlags, PhysFrame};

                let page = Page::containing_address(VirtAddr::new(target_virt));
                let frame = PhysFrame::containing_address(PhysAddr::new(phys));

                let mut flags = PageTableFlags::PRESENT | PageTableFlags::USER_ACCESSIBLE;
                if rights.contains(Rights::WRITE) {
                    flags |= PageTableFlags::WRITABLE;
                }
                if !rights.contains(Rights::EXEC) {
                    flags |= PageTableFlags::NO_EXECUTE;
                }

                unsafe {
                    crate::memory::map_page_in_pml4(pml4, page, frame, flags);
                }
                regs.rax = 0;
            } else {
                regs.rax = u64::MAX;
            }
        }
        // ── Anti-cheat attestation (284-290) ──
        //
        // Concept §Security: "No kernel-level anti-cheat needed — AthGuard
        // exposes an attestation API that anti-cheat vendors (EAC, BattlEye,
        // Vanguard) can use without owning ring 0."
        //
        // The actual handlers live in anticheat.rs. They take an args slice
        // in the SysV order (rdi, rsi, rdx, r10, r8, r9). Pack it up and
        // forward.
        // Anti-cheat attestation (284–290). RENUMBERED from 100–106, which
        // collided with SYS_OOM_SUBSCRIBE (100) + AthFS snapshot (101–103) above
        // — first-match-wins meant SYS_AC_REGISTER_GAME (102) ran the destructive
        // athfs::snapshot_restore. See ath_abi Block 34 + docs/SYSCALL_TABLE.md.
        284..=290 => {
            let args = [regs.rdi, regs.rsi, regs.rdx, regs.r10, regs.r8, regs.r9];
            regs.rax = crate::anticheat::handle_anticheat_syscall(regs.rax, &args);
        }
        // SYS_ATHENA_SHUTDOWN (120) — ACPI power-off
        120 => {
            use crate::capability::{Cap, Rights};
            let mut allowed = false;
            crate::scheduler::with_current_task(|task| {
                for (_, cap) in task.cap_table.iter() {
                    if let Cap::System { rights } = cap {
                        if rights.contains(Rights::WRITE) {
                            allowed = true;
                            break;
                        }
                    }
                }
            });

            if allowed {
                crate::acpi_full::power_off();
            } else {
                regs.rax = u64::MAX;
            }
        }
        // ── TCP/UDP socket API — MasterChecklist Phase 10 ────────────────────
        // SYS_NET_SOCKET (121): create a socket. rdi=proto (0=TCP 1=UDP). Returns fd.
        121 => {
            let pid = crate::scheduler::current_task_id()
                .map(|t| t.raw())
                .unwrap_or(0);
            regs.rax = crate::net::sys_net_socket(regs.rdi, pid);
        }
        // SYS_NET_CONNECT (122): connect TCP. rdi=fd, rsi=ip(packed u32), rdx=port.
        122 => {
            let pid = crate::scheduler::current_task_id()
                .map(|t| t.raw())
                .unwrap_or(0);
            regs.rax = crate::net::sys_net_connect(regs.rdi, regs.rsi, regs.rdx, pid);
        }
        // SYS_NET_SEND (123): send data. rdi=fd, rsi=buf_ptr, rdx=len.
        123 => {
            let pid = crate::scheduler::current_task_id()
                .map(|t| t.raw())
                .unwrap_or(0);
            // Copy the user buffer into a kernel-owned Vec through the
            // extable-fixup'd copy_from_user — never deref the raw user
            // pointer directly (an unmapped/non-canonical address would
            // fault the kernel with no fixup on this path).
            if regs.rsi != 0 && regs.rdx > 0 && regs.rdx < 65536 {
                match copy_from_user(regs.rsi, regs.rdx) {
                    Ok(data) => {
                        regs.rax = crate::net::sys_net_send(regs.rdi, &data, pid);
                    }
                    Err(_) => {
                        regs.rax = u64::MAX;
                    }
                }
            } else {
                regs.rax = crate::net::sys_net_send(regs.rdi, &[], pid);
            }
        }
        // SYS_NET_RECV (124): receive data. rdi=fd, rsi=buf_ptr, rdx=buf_len.
        124 => {
            let pid = crate::scheduler::current_task_id()
                .map(|t| t.raw())
                .unwrap_or(0);
            // Receive into a kernel-owned buffer, then copy_to_user the
            // result through an extable fixup. NEVER write directly through
            // the raw user pointer (an attacker-chosen unmapped/read-only
            // address would fault the kernel — and recv is a kernel WRITE).
            if regs.rsi != 0 && regs.rdx > 0 && regs.rdx < 65536 {
                // Validate the destination is writable BEFORE receiving so we
                // don't consume socket data we then can't deliver.
                if validate_user_range(regs.rsi, regs.rdx, true).is_err() {
                    regs.rax = u64::MAX;
                } else {
                    let len = regs.rdx as usize;
                    let mut kbuf = alloc::vec![0u8; len];
                    let n = crate::net::sys_net_recv(regs.rdi, &mut kbuf, pid);
                    if n == u64::MAX {
                        regs.rax = u64::MAX;
                    } else {
                        let n = (n as usize).min(len);
                        match copy_to_user(regs.rsi, &kbuf[..n]) {
                            Ok(()) => regs.rax = n as u64,
                            Err(_) => regs.rax = u64::MAX,
                        }
                    }
                }
            } else {
                regs.rax = crate::net::sys_net_recv(regs.rdi, &mut [], pid);
            }
        }
        // SYS_NET_CLOSE (125): close socket. rdi=fd.
        125 => {
            let pid = crate::scheduler::current_task_id()
                .map(|t| t.raw())
                .unwrap_or(0);
            regs.rax = crate::net::sys_net_close(regs.rdi, pid);
        }
        // SYS_NET_STATUS (265): socket readiness flags. rdi=fd. Returns
        // CONNECTED|READABLE|SENDABLE|CLOSED bits, or u64::MAX for a bad fd.
        265 => {
            let pid = crate::scheduler::current_task_id()
                .map(|t| t.raw())
                .unwrap_or(0);
            regs.rax = crate::net::sys_net_status(regs.rdi, pid);
        }
        // SYS_NET_DNS (264): resolve a hostname. rdi=name ptr, rsi=len.
        // Returns the IPv4 as a packed big-endian u32, or u64::MAX on failure.
        264 => {
            // Copy the hostname into a kernel-owned Vec via copy_from_user
            // (extable fixup) instead of dereferencing the raw user pointer.
            let name_buf = if regs.rdi != 0 && regs.rsi > 0 && regs.rsi < 256 {
                copy_from_user(regs.rdi, regs.rsi).ok()
            } else {
                None
            };
            let name = name_buf
                .as_deref()
                .and_then(|b| core::str::from_utf8(b).ok());
            regs.rax = match name.and_then(crate::dns::resolve_blocking) {
                Some(ip) => {
                    ((ip[0] as u64) << 24)
                        | ((ip[1] as u64) << 16)
                        | ((ip[2] as u64) << 8)
                        | (ip[3] as u64)
                }
                None => u64::MAX,
            };
        }
        // SYS_THEME_GET (266): live desktop theme for separate-process apps.
        //   rdi = out ptr (ath_abi::ThemeInfo), rsi = out capacity (bytes).
        // The 6 bundled apps are distinct ELFs that cannot call
        // theme_engine::active_accent() directly; this hands them the SAME live
        // accent + palette the in-kernel surfaces read, so Vibe Mode re-skins
        // running apps ("one tap re-skins the WHOLE desktop" — Concept
        // §Customization Engine). Read-only, no Cap, allowed in safe mode
        // (theme colours carry no secret). The struct is built kernel-side and
        // copy_to_user'd; the user pointer is validated (no raw deref).
        266 => {
            let out_ptr = regs.rdi;
            let out_cap = regs.rsi;
            let need = core::mem::size_of::<ath_abi::ThemeInfo>() as u64;
            if out_cap < need {
                regs.rax = u64::MAX;
            } else {
                let (accent, bg, fg, is_dark, blur, pid) = crate::theme_engine::theme_info();
                let info = ath_abi::ThemeInfo {
                    version: ath_abi::ThemeInfo::VERSION,
                    accent_argb: accent,
                    bg_argb: bg,
                    fg_argb: fg,
                    is_dark,
                    blur_radius: blur,
                    palette_id: pid,
                    reserved: 0,
                };
                // SAFETY: ThemeInfo is #[repr(C)], all-u32, no padding/pointers;
                // viewing it as its raw bytes for copy_to_user is sound.
                let bytes: &[u8] = unsafe {
                    core::slice::from_raw_parts(
                        (&info as *const ath_abi::ThemeInfo) as *const u8,
                        need as usize,
                    )
                };
                regs.rax = match copy_to_user(out_ptr, bytes) {
                    Ok(()) => need,
                    Err(()) => u64::MAX,
                };
            }
        }
        // SYS_AUDIO_SUBMIT (267): feed app PCM into the AthAudio mixer.
        //   rdi = samples ptr (*const i16), rsi = frame_count, rdx = format_flags.
        // Fixed format: interleaved 48 kHz i16 stereo (4 bytes/frame). The user
        // buffer is validated (validate_user_range, read) and copy_from_user'd
        // into a kernel Vec — NO raw deref — then handed to audio::submit_samples
        // which registers/feeds the calling task's per-PID SourceKind::Pcm voice
        // (the mixer drains it into AUDIO_RING → HDA, the production path).
        // frame_count is bounded by AUDIO_SUBMIT_MAX_FRAMES; format_flags must be
        // 0 (reserved). Returns frames accepted, or u64::MAX on any rejection.
        // No Cap gate (audio output carries no secret; every app may make sound)
        // and allowed in safe mode (it writes no block device).
        267 => {
            let samples_ptr = regs.rdi;
            let frame_count = regs.rsi;
            let format_flags = regs.rdx;
            if format_flags != 0
                || frame_count == 0
                || frame_count > ath_abi::syscall::AUDIO_SUBMIT_MAX_FRAMES
            {
                regs.rax = ath_abi::syscall::AUDIO_SUBMIT_ERR;
            } else {
                let byte_len = frame_count * ath_abi::syscall::AUDIO_SUBMIT_BYTES_PER_FRAME;
                match copy_from_user(samples_ptr, byte_len) {
                    Ok(bytes) => {
                        // Reinterpret the validated byte buffer as i16 samples.
                        // The Vec is 2-byte aligned for i16 (Vec<u8> from the
                        // global allocator is at least usize-aligned), and
                        // byte_len is a multiple of 2; build the i16 slice by
                        // value to avoid any alignment assumptions.
                        let n_samples = (bytes.len() / 2) as usize;
                        let mut pcm = alloc::vec::Vec::with_capacity(n_samples);
                        for i in 0..n_samples {
                            pcm.push(i16::from_le_bytes([bytes[i * 2], bytes[i * 2 + 1]]));
                        }
                        regs.rax = match crate::audio::submit_samples(&pcm, pid) {
                            Some(frames) => frames as u64,
                            None => ath_abi::syscall::AUDIO_SUBMIT_ERR,
                        };
                    }
                    Err(()) => regs.rax = ath_abi::syscall::AUDIO_SUBMIT_ERR,
                }
            }
        }
        // ── Clipboard history (268-273) — Concept §"The user owns the machine".
        // Win+V-class history + pin over the session clipboard. RAM-only /
        // local by default (no cloud, no telemetry — the ownership posture).
        // No capability gate: clipboard contents are the user's own, same as
        // GET 107 — and allowed in safe mode (history is RAM, writes no block
        // device). The kernel ring is text-first; the entry header carries a
        // format tag so Image/Files/Url can follow additively.
        //
        // SYS_CLIP_HIST_COUNT (268): -> count | (pinned_count << 32).
        268 => {
            let (count, pinned) = crate::clipboard::history_count();
            regs.rax = (count as u64) | ((pinned as u64) << 32);
        }
        // SYS_CLIP_HIST_GET (269): rdi=index, rsi=out_ptr, rdx=out_cap ->
        // ClipEntryHeader + UTF-8 payload, total bytes written / CLIP_ERR.
        269 => {
            let index = regs.rdi as usize;
            let out_ptr = regs.rsi;
            let out_cap = regs.rdx as usize;
            match crate::clipboard::history_entry_bytes(index) {
                Some(bytes) if bytes.len() <= out_cap => {
                    if validate_user_range(out_ptr, bytes.len() as u64, true).is_err() {
                        note_guard_ptr_reject();
                        regs.rax = ath_abi::syscall::CLIP_ERR;
                    } else {
                        regs.rax = match copy_to_user(out_ptr, &bytes) {
                            Ok(()) => bytes.len() as u64,
                            Err(()) => ath_abi::syscall::CLIP_ERR,
                        };
                    }
                }
                _ => regs.rax = ath_abi::syscall::CLIP_ERR,
            }
        }
        // SYS_CLIP_HIST_PIN (270): rdi=index, rsi=pin(1)/unpin(0) -> 0/CLIP_ERR.
        270 => {
            let index = regs.rdi as usize;
            let pinned = regs.rsi != 0;
            regs.rax = if crate::clipboard::history_pin(index, pinned) {
                0
            } else {
                ath_abi::syscall::CLIP_ERR
            };
        }
        // SYS_CLIP_HIST_DELETE (271): rdi=index -> 0/CLIP_ERR (refuses pinned).
        271 => {
            let index = regs.rdi as usize;
            regs.rax = if crate::clipboard::history_delete(index) {
                0
            } else {
                ath_abi::syscall::CLIP_ERR
            };
        }
        // SYS_CLIP_HIST_CLEAR (272): clear unpinned, keep pinned -> # removed.
        272 => {
            regs.rax = crate::clipboard::history_clear_keep_pinned() as u64;
        }
        // SYS_CLIP_HIST_PROMOTE (273): rdi=index -> promote to active / CLIP_ERR.
        273 => {
            let index = regs.rdi as usize;
            regs.rax = if crate::clipboard::history_promote(index) {
                0
            } else {
                ath_abi::syscall::CLIP_ERR
            };
        }
        // ── Screen capture (274-276) — Concept §creators: "capture & stream at
        // the compositor — zero-cost recording, no OBS overhead." Wraps the
        // EXISTING in-kernel compositor capture engine (start/read/stop_capture,
        // which already read real composited pixels off the front buffer) in a
        // PRIVACY-GATED syscall surface. All three require Cap::ScreenCapture
        // (privacy-sensitive — screen pixels can carry passwords/PII); START is
        // additionally refused in safe mode. READ uses VALIDATED copy_to_user
        // (no raw user-pointer deref — the net-syscall hardening pattern). This
        // is the properly-gated replacement for the legacy ungated 68-70 block.
        //
        // SYS_CAPTURE_START (274): rdi=region_xy (x|y<<32), rsi=region_wh
        //   (w|h<<32), rdx=format (CAPTURE_FMT_*), r10=flags (CAPTURE_FLAG_*)
        //   -> capture_id / CAPTURE_ERR.
        274 => {
            if !has_screen_capture_cap() {
                regs.rax = ath_abi::syscall::CAPTURE_ERR;
            } else if crate::block_io::safe_mode_enabled() {
                // Privacy-sensitive: no screen reads in safe mode.
                regs.rax = ath_abi::syscall::CAPTURE_ERR;
            } else {
                let rx = (regs.rdi & 0xFFFF_FFFF) as u32;
                let ry = ((regs.rdi >> 32) & 0xFFFF_FFFF) as u32;
                let rw = (regs.rsi & 0xFFFF_FFFF) as u32;
                let rh = ((regs.rsi >> 32) & 0xFFFF_FFFF) as u32;
                let fmt = if regs.rdx == ath_abi::syscall::CAPTURE_FMT_BGRA32 as u64 {
                    crate::compositor::CaptureFormat::Bgra32
                } else {
                    crate::compositor::CaptureFormat::Argb32
                };
                let continuous = regs.r10 & ath_abi::syscall::CAPTURE_FLAG_CONTINUOUS != 0;
                if rw == 0 || rh == 0 {
                    regs.rax = ath_abi::syscall::CAPTURE_ERR;
                } else {
                    let owner = crate::scheduler::current_task_id()
                        .map(|t| t.raw())
                        .unwrap_or(0);
                    regs.rax = crate::compositor::start_capture_owned(
                        rx, ry, rw, rh, fmt, continuous, owner,
                    );
                }
            }
        }
        // SYS_CAPTURE_READ (275): rdi=capture_id, rsi=out_ptr, rdx=out_cap ->
        //   CaptureHeader (16 bytes) + pixel payload, total bytes written /
        //   CAPTURE_ERR. Validated copy_to_user; no raw deref.
        275 => {
            if !has_screen_capture_cap() {
                regs.rax = ath_abi::syscall::CAPTURE_ERR;
            } else {
                let id = regs.rdi;
                let out_ptr = regs.rsi;
                let out_cap = regs.rdx as usize;
                match crate::compositor::read_capture_fmt(id) {
                    Some((pixels, w, h, fmt)) => {
                        let payload_len = pixels.len().saturating_mul(4);
                        let total = core::mem::size_of::<ath_abi::CaptureHeader>() + payload_len;
                        if out_cap < total {
                            regs.rax = ath_abi::syscall::CAPTURE_ERR;
                        } else {
                            // Build the header + pixel bytes in a kernel buffer,
                            // then copy_to_user (validated) in one shot.
                            let mut buf = alloc::vec::Vec::with_capacity(total);
                            let fmt_tag = match fmt {
                                crate::compositor::CaptureFormat::Bgra32 => {
                                    ath_abi::syscall::CAPTURE_FMT_BGRA32
                                }
                                crate::compositor::CaptureFormat::Argb32 => {
                                    ath_abi::syscall::CAPTURE_FMT_ARGB32
                                }
                            };
                            let hdr = ath_abi::CaptureHeader {
                                width: w,
                                height: h,
                                format: fmt_tag,
                                bytes: payload_len as u32,
                            };
                            // Header (4 u32 LE).
                            buf.extend_from_slice(&hdr.width.to_le_bytes());
                            buf.extend_from_slice(&hdr.height.to_le_bytes());
                            buf.extend_from_slice(&hdr.format.to_le_bytes());
                            buf.extend_from_slice(&hdr.bytes.to_le_bytes());
                            // Pixel payload (native-endian u32 ARGB/BGRA words).
                            for px in pixels.iter() {
                                buf.extend_from_slice(&px.to_ne_bytes());
                            }
                            regs.rax = match copy_to_user(out_ptr, &buf) {
                                Ok(()) => total as u64,
                                Err(()) => {
                                    note_guard_ptr_reject();
                                    ath_abi::syscall::CAPTURE_ERR
                                }
                            };
                        }
                    }
                    None => regs.rax = ath_abi::syscall::CAPTURE_ERR,
                }
            }
        }
        // SYS_CAPTURE_STOP (276): rdi=capture_id -> 0 / CAPTURE_ERR.
        276 => {
            if !has_screen_capture_cap() {
                regs.rax = ath_abi::syscall::CAPTURE_ERR;
            } else {
                crate::compositor::stop_capture(regs.rdi);
                regs.rax = 0;
            }
        }
        // ── Accessibility tree (277-278) — Concept §Security + Phase 19 a11y ──
        // The assistive-tech read/dispatch surface (screen reader, magnifier,
        // keyboard-nav all consume it). Both gated on Cap::Accessibility (fails
        // CLOSED — reading another app's UI tree / driving its widgets is
        // privileged, like macOS TCC / Windows UIA). SNAPSHOT uses VALIDATED
        // copy_to_user (no raw deref — the net/capture hardening pattern).
        //
        // SYS_A11Y_SNAPSHOT (277): rdi=out_ptr, rsi=out_cap_bytes -> total bytes
        //   written (A11ySnapshotHeader + node array) / A11Y_ERR. Cap READ.
        277 => {
            use crate::capability::Rights;
            if !has_accessibility_cap(Rights::READ) {
                regs.rax = ath_abi::syscall::A11Y_ERR;
            } else {
                let out_ptr = regs.rdi;
                let out_cap = regs.rsi as usize;
                match crate::a11y::snapshot_for_client(true) {
                    Ok(nodes) => {
                        let focused = crate::a11y::focused_node_id();
                        let buf = crate::a11y::serialize_snapshot(&nodes, focused);
                        if out_cap < buf.len() {
                            regs.rax = ath_abi::syscall::A11Y_ERR;
                        } else {
                            regs.rax = match copy_to_user(out_ptr, &buf) {
                                Ok(()) => buf.len() as u64,
                                Err(()) => {
                                    note_guard_ptr_reject();
                                    ath_abi::syscall::A11Y_ERR
                                }
                            };
                        }
                    }
                    Err(_) => regs.rax = ath_abi::syscall::A11Y_ERR,
                }
            }
        }
        // SYS_A11Y_ACTION (278): rdi=node_id, rsi=action (A11Y_ACTION_*),
        //   rdx=arg -> 0 / A11Y_ERR. Cap WRITE. Routes to the owning surface.
        278 => {
            use crate::capability::Rights;
            if !has_accessibility_cap(Rights::WRITE) {
                regs.rax = ath_abi::syscall::A11Y_ERR;
            } else {
                let node_id = regs.rdi;
                let action = regs.rsi;
                let arg = regs.rdx;
                regs.rax = if crate::a11y::dispatch_action(node_id, action, arg, true) {
                    0
                } else {
                    ath_abi::syscall::A11Y_ERR
                };
            }
        }
        // SYS_INPUT_CURSOR (279): no args -> packed `x | (y << 16)` ABSOLUTE
        //   cursor position so an app can hit-test where a click landed. Reads a
        //   lock-free atomic the compositor updates on every cursor move (never
        //   blocks, no compositor lock). Bits [63:32] RESERVED (0) for a future
        //   button bitmask; buttons today come from SYS_POLL_MOUSE (32). No
        //   capability gate, allowed in every sandbox level / safe mode — cursor
        //   position carries no secret (same posture as reading input).
        279 => {
            let (x, y) = crate::compositor::cursor_position_fast();
            regs.rax = (x as u64 & 0xFFFF) | ((y as u64 & 0xFFFF) << 16);
        }
        // SYS_SURFACE_ORIGIN (280): rdi = surface id -> packed `x | (y << 16)`
        //   absolute origin so an app can convert the absolute cursor (279) into
        //   surface-local coords for hit-testing AFTER the window manager moves the
        //   window (Overview / Spaces / tiling call set_surface_origin). Returns
        //   SURFACE_ORIGIN_ERR (u64::MAX) for an unknown id / compositor down. No
        //   capability gate, allowed in every sandbox level / safe mode — a
        //   surface's screen position carries no secret (same posture as 279).
        280 => {
            let sid = regs.rdi;
            regs.rax = match crate::compositor::surface_origin(sid) {
                Some((x, y)) => (x as u64 & 0xFFFF) | ((y as u64 & 0xFFFF) << 16),
                None => ath_abi::syscall::SURFACE_ORIGIN_ERR,
            };
        }
        // ── SYS_SEARCH_QUERY_RESOLVED (281) ── Concept §"Search is broken".
        // The NAMED-result counterpart of SYS_SEARCH_QUERY (56): 56 returns only
        // opaque (id, kind) 16-byte records; this serializes the RESOLVED hits
        // (name + path + kind + folder flag) so the Files app / command palette
        // can render clickable rows. Variable-length records — a 24-byte header
        // (ath_abi::SearchResolvedHeader) + name + path bytes each. The kernel
        // serializes into a kernel-owned buffer (search_index::serialize_resolved,
        // which acquires INDEX via lock_index → IF=0-safe, no deadlock) then
        // copy_to_user's it (validated, no raw deref). Ungated + safe-mode-OK,
        // same posture as SYS_SEARCH_QUERY (search names carry no secret beyond
        // what the indexer already holds). rdi=q_ptr, rsi=q_len, rdx=out_ptr,
        // r10=out_cap_bytes -> record count in rax (0 = empty/no-match/no-index;
        // never an error sentinel — graceful empty-result, like 56).
        281 => {
            let q_ptr = regs.rdi;
            let q_len = regs.rsi;
            let out_ptr = regs.rdx;
            let out_cap = regs.r10 as usize;
            // An empty query or zero/unwritable output buffer yields 0 results
            // (same empty-on-bad-args posture as SYS_SEARCH_QUERY 56 — never an
            // error sentinel). copy_from_user validates the query read range; the
            // output write range is validated below before serialization.
            if q_len == 0
                || out_cap == 0
                || validate_user_range(out_ptr, out_cap as u64, true).is_err()
            {
                regs.rax = 0;
            } else {
                let query = match copy_from_user(q_ptr, q_len) {
                    Ok(bytes) => alloc::string::String::from_utf8_lossy(&bytes).into_owned(),
                    Err(()) => {
                        note_guard_ptr_reject();
                        alloc::string::String::new()
                    }
                };
                if query.is_empty() {
                    regs.rax = 0;
                } else {
                    let (buf, count) = crate::search_index::serialize_resolved(&query, out_cap);
                    if buf.is_empty() {
                        regs.rax = 0;
                    } else {
                        regs.rax = match copy_to_user(out_ptr, &buf) {
                            Ok(()) => count as u64,
                            Err(()) => {
                                note_guard_ptr_reject();
                                0
                            }
                        };
                    }
                }
            }
        }
        // SYS_SURFACE_RESIZE_REQ (291): rdi = surface id. Poll whether the window
        //   manager wants this client at a new size (it tiled/snapped the window
        //   into a cell). Returns the requested size packed `w | (h<<16)` when a
        //   resize is pending, SURFACE_RESIZE_NONE (0) when none, or
        //   SURFACE_RESIZE_ERR (u64::MAX) for an unknown id / compositor down. No
        //   capability gate, all-sandbox — a window's requested size is no secret
        //   (same posture as SYS_SURFACE_ORIGIN 280).
        291 => {
            let sid = regs.rdi;
            regs.rax = match crate::compositor::surface_resize_request(sid) {
                Some((w, h)) => (w as u64 & 0xFFFF) | ((h as u64 & 0xFFFF) << 16),
                None => {
                    // Distinguish "no request pending" (0) from "unknown id"
                    // (u64::MAX): the origin poll proves the surface exists.
                    if crate::compositor::surface_origin(sid).is_some() {
                        ath_abi::syscall::SURFACE_RESIZE_NONE
                    } else {
                        ath_abi::syscall::SURFACE_RESIZE_ERR
                    }
                }
            };
        }
        // SYS_SURFACE_RESIZE (292): rdi = id, rsi = w, rdx = h, r10 = new_buf
        //   (page-aligned user vaddr). The client acks a pending resize: it has
        //   allocated a fresh w*h*4 buffer at new_buf, and the kernel rebinds the
        //   surface to it (alloc new frames, map at new_buf in the caller's PML4,
        //   unmap+free the old frames, update dimensions, clear the request). The
        //   caller MUST own the surface (compositor checks owner_task). Returns 0
        //   on success or SURFACE_RESIZE_ERR (u64::MAX) on bad id / non-owner /
        //   bad dims / unaligned new_buf / alloc fail. All-sandbox, ungated — a
        //   task resizes only its own window and reaches no other address space.
        292 => {
            let sid = regs.rdi;
            let w = regs.rsi as u32;
            let h = regs.rdx as u32;
            let new_buf = regs.r10;
            regs.rax = match crate::scheduler::current_task_id() {
                Some(caller) => {
                    match crate::compositor::resize_user_surface(sid, w, h, new_buf, caller) {
                        Ok(()) => 0,
                        Err(()) => ath_abi::syscall::SURFACE_RESIZE_ERR,
                    }
                }
                None => ath_abi::syscall::SURFACE_RESIZE_ERR,
            };
        }
        // SYS_FUTEX (258): native futex for relibc sync (mutex/once/rwlock).
        //   rdi = uaddr, rsi = op (0=WAIT, 1=WAKE), rdx = val (WAIT: expected
        //   *uaddr; WAKE: max waiters). Reuses the kernel futex table that the
        //   Linux ABI (syscall 202) already drives. WAIT → 0 woken / 1 EAGAIN /
        //   2 fault; WAKE → count woken. Native relibc apps hit ENOSYS without
        //   this; the Linux path had it but native ELFs don't take that path.
        //   NOTE: a draft assigned 119, which is the live SYS_CHANNEL_SHMEM_MAP
        //   arm — only the FIRST match arm runs, so the futex would have been
        //   dead code (and a relibc caller would have hit the shmem-map arm
        //   with garbage args). Renumbered to the 258-263 native-sync block.
        258 => {
            let addr = regs.rdi;
            let op = regs.rsi;
            let val = regs.rdx as u32;
            match op {
                0 => {
                    // Native futex WAIT: really BLOCK (BlockedOnFutex, keyed by
                    // the word's shared physical frame) and retry the syscall on
                    // wake so the word is re-checked — vs the old single
                    // cooperative yield that never parked. This is the primitive
                    // under every AthBridge Win32 sync object (WaitForSingleObject
                    // → Event/Mutex/Semaphore). MasterChecklist item 1828.
                    match crate::linux_syscall::futex_prepare_wait(addr, val) {
                        crate::linux_syscall::FutexPrep::Block(phys) => {
                            crate::scheduler::block_current_task(
                                crate::task::TaskState::BlockedOnFutex(phys),
                                regs as *mut _ as usize,
                            );
                            regs.rcx -= 2; // re-execute `syscall` on wake
                            return;
                        }
                        crate::linux_syscall::FutexPrep::Eagain => regs.rax = 1,
                        crate::linux_syscall::FutexPrep::Fault => regs.rax = 2,
                    }
                }
                1 => {
                    regs.rax = crate::linux_syscall::futex_wake(addr, val).unwrap_or(0) as u64;
                }
                _ => regs.rax = 2,
            }
        }
        // SYS_SET_FS_BASE (126): set FS base for Thread Local Storage. rdi=addr.
        // Persist in Task::fs_base — the scheduler restores IA32_FS_BASE from
        // it on every switch-in, so the TLS pointer survives context switches
        // (a bare MSR write here would be clobbered by the next task change).
        126 => {
            if regs.rdi >= 0x0000_8000_0000_0000 {
                regs.rax = u64::MAX; // non-canonical / kernel-half address
                return;
            }
            crate::scheduler::with_current_task_mut(|t| t.fs_base = regs.rdi);
            #[cfg(target_arch = "x86_64")]
            {
                x86_64::registers::model_specific::FsBase::write(VirtAddr::new(regs.rdi));
            }
            regs.rax = 0;
        }
        // SYS_SET_GS_BASE (282, ath_abi): set the user-visible GS base to the
        // Win32 TEB pointer for a AthBridge guest (MSVC CRT reads gs:[0x30]).
        // rdi = TEB virtual address. Mirrors SYS_SET_FS_BASE (126).
        //
        // WHICH MSR — the load-bearing subtlety. The spec (athbridge-real-crt-
        // abi.md §2) prescribes KernelGsBase on the assumption of a SINGLE
        // trailing swapgs before sysretq. AthenaOS's `syscall_handler` actually
        // does an EVEN number of swapgs between this Rust arm and sysret: the
        // pair at lines ~184/187 (swap GS back to the per-CPU block for the
        // call, then swap again after) PLUS the final one at ~208. Trace the
        // active GS base (G) and kernel-GS (K) from here to sysret:
        //   arm runs:  G = user(active),  K = per-CPU ptr
        //   ~187 swapgs: G = per-CPU ptr, K = user
        //   ~208 swapgs: G = user,        K = per-CPU ptr
        //   sysret:      active GS = user
        // So the value active at sysret is whatever we leave in the *active*
        // GsBase here, and the per-CPU pointer in KernelGsBase is preserved for
        // the next syscall entry's first swapgs. Writing GsBase (active) is
        // therefore correct; writing KernelGsBase would (a) NOT install the TEB
        // at sysret and (b) DESTROY the per-CPU pointer the next swapgs needs.
        // FS needs no swap, so arm 126 writes FsBase directly — symmetric.
        // The scheduler restores Task::gs_base via GsBase on every switch-in.
        282 => {
            if regs.rdi >= 0x0000_8000_0000_0000 {
                regs.rax = u64::MAX; // non-canonical / kernel-half address
                return;
            }
            crate::scheduler::with_current_task_mut(|t| t.gs_base = regs.rdi);
            #[cfg(target_arch = "x86_64")]
            {
                x86_64::registers::model_specific::GsBase::write(VirtAddr::new(regs.rdi));
            }
            regs.rax = 0;
        }
        // SYS_MPROTECT (283, ath_abi): flip protection flags on already-mapped
        // 4 KiB user pages. rdi=addr, rsi=len, rdx=prot (PROT_READ=1/WRITE=2/
        // EXEC=4). AthBridge's loader uses this to flip relocated .text RW->RX.
        // The AthGuard W^X gate (refuse W+X) lives in sys_mprotect.
        283 => {
            regs.rax = crate::memory::sys_mprotect(regs.rdi, regs.rsi, regs.rdx);
        }
        // ── Rae scripting daemon half (294-295) — athlangd drains >64 KiB
        // scripts the kernel won't run inline (Concept §Customization
        // Engine; scripting.rs owns the lifecycle). 284-290 are anti-cheat
        // (first-match-wins — see the renumbering note above them).
        // SYS_SCRIPT_FETCH (294): rdi=out_ptr, rsi=out_cap ->
        //   bytes written (ScriptJobAbi header + source) / 0 none / ERR_*.
        294 => {
            regs.rax = crate::scripting::sys_fetch(regs.rdi, regs.rsi, |p, l, w| {
                validate_user_range(p, l, w).is_ok()
            });
        }
        // SYS_SCRIPT_COMPLETE (295): rdi=script_id, rsi=exit_code (two's
        //   complement i64), rdx=out_ptr, r10=out_len -> 0 / ERR_*.
        295 => {
            regs.rax = crate::scripting::sys_complete(
                regs.rdi,
                regs.rsi,
                regs.rdx,
                regs.r10,
                |p, l, w| validate_user_range(p, l, w).is_ok(),
            );
        }
        // SYS_NETLOG_FLUSH (296): broadcast the bootlog ring over UDP NOW so a
        // marker survives a later hard hang (the end-of-boot flush + BOOTLOG
        // persist both live on CPU 0 and die with it). No args; returns chunks
        // sent. Safe-mode safe (UDP TX, not a sector write). amdgpud fences each
        // real-amdgpu init phase with this so the netlog trail ends at the exact
        // stage CPU 0 freezes on.
        296 => {
            regs.rax = crate::netlog::broadcast_ring("amdgpu-diag") as u64;
        }
        // DRM render broker: the kernel owns /dev/dri/renderD128 and bounded
        // client copies; the retained upstream object graph stays in amdgpud.
        297 => {
            regs.rax = crate::gpu_render::sys_register(regs.rdi);
        }
        298 => {
            regs.rax = crate::gpu_render::sys_fetch(regs.rdi, regs.rsi, regs.rdx);
        }
        299 => {
            regs.rax = crate::gpu_render::sys_complete(regs.rdi, regs.rsi, regs.rdx, regs.r10);
        }
        // LinuxKPI host (127–131) — userspace driver daemon timing + logging.
        127 => {
            regs.rax = crate::linuxkpi_host::sys_version();
        }
        128 => {
            regs.rax = crate::linuxkpi_host::sys_jiffies();
        }
        129 => {
            crate::linuxkpi_host::sys_msleep(regs.rdi);
            regs.rax = 0;
        }
        131 => {
            regs.rax =
                crate::linuxkpi_host::sys_printk(regs.rdi, regs.rsi, |p, l| copy_from_user(p, l));
        }
        // ── LinuxKPI Phase 2-4 hardware bridge (130, 132-140) ────────────────
        // SYS_LINUXKPI_IOREMAP: rdi=dev_handle, rsi=bar_index -> virt ptr
        130 => {
            regs.rax = crate::linuxkpi_host::lkpi_ioremap(regs.rdi, regs.rsi);
        }
        // SYS_LINUXKPI_PCI_ENABLE: rdi=packed_bdf -> device handle
        132 => {
            regs.rax = crate::linuxkpi_host::lkpi_pci_enable(regs.rdi);
        }
        // SYS_LINUXKPI_PCI_READ_CFG: rdi=dev_handle, rsi=offset -> value
        133 => {
            regs.rax = crate::linuxkpi_host::lkpi_pci_read_cfg(regs.rdi, regs.rsi);
        }
        // SYS_LINUXKPI_PCI_WRITE_CFG: rdi=dev_handle, rsi=offset, rdx=value
        134 => {
            regs.rax = crate::linuxkpi_host::lkpi_pci_write_cfg(regs.rdi, regs.rsi, regs.rdx);
        }
        // SYS_LINUXKPI_DMA_ALLOC: rdi=dev_handle, rsi=size, rdx=out_ptr ([virt,phys,size,token])
        135 => {
            regs.rax =
                crate::linuxkpi_host::lkpi_dma_alloc(regs.rdi, regs.rsi, regs.rdx, |dst, bytes| {
                    copy_to_user(dst, bytes)
                });
        }
        // SYS_LINUXKPI_DMA_FREE: rdi=dev_handle, rsi=token
        136 => {
            regs.rax = crate::linuxkpi_host::lkpi_dma_free(regs.rdi, regs.rsi);
        }
        // SYS_LINUXKPI_REQUEST_IRQ: rdi=dev_handle, rsi=vector -> irq_handle
        137 => {
            regs.rax = crate::linuxkpi_host::lkpi_request_irq(regs.rdi, regs.rsi);
        }
        // SYS_LINUXKPI_IRQ_WAIT: rdi=irq_cap_handle -> block until IRQ (same as syscall 8)
        138 => {
            use crate::task::TaskState;
            if let Some(vector) = crate::linuxkpi_host::irq_wait_try_ready(regs.rdi) {
                regs.rax = vector as u64;
            } else if let Some(vector) = crate::linuxkpi_host::irq_vector_for_cap(regs.rdi) {
                crate::scheduler::block_current_task(
                    TaskState::BlockedOnIrq(vector),
                    regs as *mut _ as usize,
                );
                regs.rax = vector as u64;
            } else {
                regs.rax = crate::linuxkpi_host::E_NO_IRQ;
            }
        }
        // SYS_LINUXKPI_IOUNMAP: rdi=virt, rsi=len
        139 => {
            regs.rax = crate::linuxkpi_host::lkpi_iounmap(regs.rdi, regs.rsi);
        }
        // SYS_LINUXKPI_MAP_PHYS: rdi=dev_handle, rsi=phys, rdx=size -> user virt.
        // Maps a NON-BAR reserved/carveout physical range (APU/UMA VRAM) into the
        // owning daemon; refuses any range overlapping usable RAM.
        144 => {
            regs.rax = crate::linuxkpi_host::lkpi_map_phys(regs.rdi, regs.rsi, regs.rdx);
        }
        // SYS_LINUXKPI_SUPERVISOR: rdi=op, rsi=arg
        140 => {
            regs.rax = crate::linuxkpi_host::lkpi_supervisor(regs.rdi, regs.rsi);
        }
        // SYS_LINUXKPI_REQUEST_FIRMWARE: rdi=name_ptr, rsi=name_len, rdx=out_ptr ([virt,size])
        142 => {
            regs.rax = crate::linuxkpi_host::lkpi_request_firmware(
                regs.rdi,
                regs.rsi,
                regs.rdx,
                |p, l| copy_from_user(p, l),
                |dst, bytes| copy_to_user(dst, bytes),
            );
        }
        // SYS_RAEGFX_REGISTER_SCANOUT (143): a GPU driver daemon registers its
        // display scanout framebuffer with the compositor (amdgpu DCN path).
        //   rdi = dev_handle, rsi = phys, rdx = (width << 32 | height), r10 = stride
        // The handler verifies the caller OWNS `phys` as a DMA region on
        // `dev_handle` (security gate). -> 1 attached, 0 reject.
        143 => {
            regs.rax =
                crate::linuxkpi_host::lkpi_register_scanout(regs.rdi, regs.rsi, regs.rdx, regs.r10);
        }
        // SYS_INSTALL_RUN (256) — MasterChecklist Phase 3: run the installer onto
        // the active block device. Returns a stage bitmask (5 bits = full install).
        // Requires Cap::System{WRITE} (same gate as SYS_ATHENA_SHUTDOWN).
        256 => {
            use crate::capability::{Cap, Rights};
            let mut allowed = false;
            crate::scheduler::with_current_task(|task| {
                for (_, cap) in task.cap_table.iter() {
                    if let Cap::System { rights } = cap {
                        if rights.contains(Rights::WRITE) {
                            allowed = true;
                            break;
                        }
                    }
                }
            });
            if allowed {
                regs.rax = crate::installer::run_install();
            } else {
                crate::serial_println!(
                    "[install] SYS_INSTALL_RUN denied: needs Cap::System{{WRITE}}"
                );
                regs.rax = u64::MAX;
            }
        }
        // SYS_INSTALL_CREATE_ACCOUNT (257) — create a local account during install.
        //   rdi = username ptr, rsi = username len
        //   rdx = password ptr, r10 = password len
        //   r8  = display-name ptr, r9 = display-name len (0 = use username)
        // Returns the new user id, or u64::MAX on failure.
        257 => {
            let uname = read_user_cstr(regs.rdi, regs.rsi.min(32) as usize);
            let pass = copy_from_user(regs.rdx, regs.r10.min(128)).ok();
            let disp = if regs.r9 > 0 {
                read_user_cstr(regs.r8, regs.r9.min(64) as usize)
            } else {
                uname.clone()
            };
            match (uname, pass, disp) {
                (Some(u), Some(p), Some(d)) => {
                    match crate::session::create_local_account(&u, &d, &p) {
                        Some(uid) => regs.rax = uid,
                        None => regs.rax = u64::MAX,
                    }
                }
                _ => regs.rax = u64::MAX,
            }
        }

        _ => {
            crate::serial_println!("[syscall] Unknown syscall: {}", regs.rax);
            regs.rax = u64::MAX;
        }
    }
}
