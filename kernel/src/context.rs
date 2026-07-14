use core::arch::global_asm;

global_asm!(
    ".global switch_context",
    "switch_context:",
    // rdi = *mut usize (pointer to old_rsp), OR null to discard the save
    // rsi = usize (new_rsp)
    // rdx = u64 (new_cr3, or 0 to keep current)
    // rcx = *mut FxSaveArea (outgoing task FPU buf), null to skip
    // r8  = *mut FxSaveArea (incoming task FPU buf), null to skip

    // 1. Save outgoing task's FPU/SSE state.
    "test rcx, rcx",
    "jz 3f",
    "fxsave64 [rcx]",
    "3:",
    // Push callee-saved registers
    "push rbx",
    "push rbp",
    "push r12",
    "push r13",
    "push r14",
    "push r15",
    // 3. Save old RSP (if non-null).
    "test rdi, rdi",
    "jz 2f",
    "mov [rdi], rsp",
    "2:",
    // 4. Load new stack pointer.
    "mov rsp, rsi",
    // 5. Switch CR3 unless rdx == 0.
    "test rdx, rdx",
    "jz 1f",
    "mov cr3, rdx",
    "1:",
    // 6. Restore incoming task's callee-saved regs.
    "pop r15",
    "pop r14",
    "pop r13",
    "pop r12",
    "pop rbp",
    "pop rbx",
    // 7. Restore incoming task's FPU/SSE state using r8.
    "test r8, r8",
    "jz 4f",
    "fxrstor64 [r8]",
    "4:",
    "ret"
);

extern "C" {
    pub fn switch_context(
        old_rsp: *mut usize,
        new_rsp: usize,
        new_cr3: u64,
        old_fpu: *mut u8,
        new_fpu: *mut u8,
    );
}

/// Trampoline for freshly created kernel threads. switch_context's `ret` jumps
/// here with the real entry point stashed in r12.  We enable interrupts (the
/// caller always disables them for the switch), call the entry function, and
/// then cleanly exit the task if it returns instead of crashing.
#[unsafe(naked)]
pub extern "C" fn kernel_thread_entry() {
    core::arch::naked_asm!(
        "call {hook}",
        "sti",
        "call r12",
        "xor edi, edi",
        "call {exit}",
        "ud2",
        hook = sym crate::scheduler::finish_task_switch,
        exit = sym crate::scheduler::exit_current_task,
    );
}

#[unsafe(naked)]
pub extern "C" fn thread_entry_user() {
    core::arch::naked_asm!(
        // We arrive here when switch_context returns for a newly spawned user task.
        "call {hook}",
        // The task creation logic will place:
        // rbx = User RIP (Entry Point)
        // rbp = User RSP (Stack Pointer)
        "push 0x23",  // User SS (Data Segment RPL=3)
        "push rbp",   // User RSP
        "push 0x202", // User RFLAGS
        "push 0x2B",  // User CS (Code Segment RPL=3)
        "push rbx",   // User RIP
        // Zero all GPRs to prevent leaking kernel data to userspace
        "xor rax, rax",
        "xor rcx, rcx",
        "xor rdx, rdx",
        "xor rsi, rsi",
        "xor rdi, rdi",
        "xor r8, r8",
        "xor r9, r9",
        "xor r10, r10",
        "xor r11, r11",
        "xor r12, r12",
        "xor r13, r13",
        "xor r14, r14",
        "xor r15, r15",
        "xor rbx, rbx",
        "xor rbp, rbp",
        "iretq",
        hook = sym crate::scheduler::finish_task_switch,
    );
}
