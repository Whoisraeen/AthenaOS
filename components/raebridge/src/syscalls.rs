//! Native AthenaOS syscall bindings for the compatibility layer.

// Syscall numbers (matching kernel/src/syscall.rs)
pub const SYS_SPAWN: u64 = 11;
pub const SYS_EXIT: u64 = 12;
pub const SYS_WAIT: u64 = 13;
pub const SYS_KILL: u64 = 14;
pub const SYS_OPEN: u64 = 15;
pub const SYS_READ: u64 = 16;
pub const SYS_WRITE: u64 = 17;
pub const SYS_CLOSE: u64 = 18;
pub const SYS_MMAP: u64 = 19;
pub const SYS_MUNMAP: u64 = 20;
pub const SYS_SEEK: u64 = 22;
pub const SYS_STAT: u64 = 23;
pub const SYS_GETPID: u64 = 29;
pub const SYS_DEBUG_PRINT: u64 = 141;
pub const SYS_SURFACE_CREATE: u64 = 24;
pub const SYS_SURFACE_PRESENT: u64 = 25;
pub const SYS_SURFACE_FOCUS: u64 = 26;
pub const SYS_SURFACE_CLOSE: u64 = 27;
pub const SYS_YIELD: u64 = 28;
/// `SYS_SET_GS_BASE(base)` — point the user-visible GS base at the Win32 TEB so
/// guest `gs:[0x30]` reads the TEB self-pointer (rae_abi syscall 282).
pub const SYS_SET_GS_BASE: u64 = 282;
/// `SYS_MPROTECT(addr, len, prot)` — change protection flags on already-mapped
/// user pages (rae_abi syscall 283). The W^X RW→RX flip the loader needs.
pub const SYS_MPROTECT: u64 = 283;

/// `prot` bits shared with `SYS_MMAP` (rae_abi `PROT_*`). `PROT_READ |
/// PROT_WRITE == 3` matches the existing `sys_mmap(.., 3, ..)` call sites.
pub const PROT_NONE: u64 = 0;
pub const PROT_READ: u64 = 1; // bit 0
pub const PROT_WRITE: u64 = 2; // bit 1
pub const PROT_EXEC: u64 = 4; // bit 2

/// Print a UTF-8 byte buffer to the kernel serial port (SYS_DEBUG_PRINT,
/// docs/SYSCALL_TABLE.md Block 23a). No capability required; ≤ 4096 bytes.
#[inline(always)]
pub unsafe fn sys_debug_print(buf: &[u8]) -> u64 {
    let n: u64;
    core::arch::asm!(
        "syscall",
        inout("rax") SYS_DEBUG_PRINT => n,
        in("rdi") buf.as_ptr() as u64,
        in("rsi") buf.len() as u64,
        out("rcx") _, out("r11") _,
    );
    n
}

#[inline(always)]
pub unsafe fn sys_open(path: &[u8], flags: u64) -> u64 {
    let fd: u64;
    core::arch::asm!(
        "syscall",
        inout("rax") SYS_OPEN => fd,
        in("rdi") path.as_ptr() as u64,
        in("rsi") path.len() as u64,
        in("rdx") flags,
        out("rcx") _, out("r11") _,
    );
    fd
}

#[inline(always)]
pub unsafe fn sys_read(fd: u64, buf: &mut [u8]) -> u64 {
    let n: u64;
    core::arch::asm!(
        "syscall",
        inout("rax") SYS_READ => n,
        in("rdi") fd,
        in("rsi") buf.as_mut_ptr() as u64,
        in("rdx") buf.len() as u64,
        out("rcx") _, out("r11") _,
    );
    n
}

#[inline(always)]
pub unsafe fn sys_write(fd: u64, buf: &[u8]) -> u64 {
    let n: u64;
    core::arch::asm!(
        "syscall",
        inout("rax") SYS_WRITE => n,
        in("rdi") fd,
        in("rsi") buf.as_ptr() as u64,
        in("rdx") buf.len() as u64,
        out("rcx") _, out("r11") _,
    );
    n
}

#[inline(always)]
pub unsafe fn sys_close(fd: u64) -> u64 {
    let r: u64;
    core::arch::asm!(
        "syscall",
        inout("rax") SYS_CLOSE => r,
        in("rdi") fd,
        out("rcx") _, out("r11") _,
    );
    r
}

/// Return this task's id (SYS_GETPID, docs/SYSCALL_TABLE.md nr 29). Used by the
/// per-process launcher to self-report its PID in the launch proof line.
#[inline(always)]
pub unsafe fn sys_getpid() -> u64 {
    let tid: u64;
    core::arch::asm!(
        "syscall",
        inout("rax") SYS_GETPID => tid,
        out("rcx") _, out("r11") _,
    );
    tid
}

#[inline(always)]
pub unsafe fn sys_mmap(addr: u64, length: u64, prot: u64, flags: u64, fd: u64, offset: u64) -> u64 {
    let result: u64;
    core::arch::asm!(
        "syscall",
        inout("rax") SYS_MMAP => result,
        in("rdi") addr,
        in("rsi") length,
        in("rdx") prot,
        in("r10") flags,
        in("r8") fd,
        in("r9") offset,
        out("rcx") _, out("r11") _,
    );
    result
}

#[inline(always)]
pub unsafe fn sys_munmap(addr: u64, length: u64) -> u64 {
    let result: u64;
    core::arch::asm!(
        "syscall",
        inout("rax") SYS_MUNMAP => result,
        in("rdi") addr,
        in("rsi") length,
        out("rcx") _, out("r11") _,
    );
    result
}

#[inline(always)]
pub unsafe fn sys_seek(fd: u64, offset: u64) -> u64 {
    let r: u64;
    core::arch::asm!(
        "syscall",
        inout("rax") SYS_SEEK => r,
        in("rdi") fd,
        in("rsi") offset,
        out("rcx") _, out("r11") _,
    );
    r
}

#[inline(always)]
pub unsafe fn sys_stat(fd: u64) -> u64 {
    let r: u64;
    core::arch::asm!(
        "syscall",
        inout("rax") SYS_STAT => r,
        in("rdi") fd,
        out("rcx") _, out("r11") _,
    );
    r
}

#[inline(always)]
pub unsafe fn sys_spawn(path: &[u8]) -> u64 {
    let pid: u64;
    core::arch::asm!(
        "syscall",
        inout("rax") SYS_SPAWN => pid,
        in("rdi") path.as_ptr() as u64,
        in("rsi") path.len() as u64,
        out("rcx") _, out("r11") _,
    );
    pid
}

#[inline(always)]
pub unsafe fn sys_exit(code: u64) -> ! {
    core::arch::asm!(
        "syscall",
        in("rax") SYS_EXIT,
        in("rdi") code,
        options(noreturn)
    );
}

#[inline(always)]
pub unsafe fn sys_wait(pid: u64) -> u64 {
    let code: u64;
    core::arch::asm!(
        "syscall",
        inout("rax") SYS_WAIT => code,
        in("rdi") pid,
        out("rcx") _, out("r11") _,
    );
    code
}

#[inline(always)]
pub unsafe fn sys_kill(pid: u64) -> u64 {
    let res: u64;
    core::arch::asm!(
        "syscall",
        inout("rax") SYS_KILL => res,
        in("rdi") pid,
        out("rcx") _, out("r11") _,
    );
    res
}

#[inline(always)]
pub unsafe fn sys_surface_create(width: u32, height: u32, uvirt: u64) -> u64 {
    let id: u64;
    core::arch::asm!(
        "syscall",
        inout("rax") SYS_SURFACE_CREATE => id,
        in("rdi") width as u64,
        in("rsi") height as u64,
        in("rdx") uvirt,
        out("rcx") _, out("r11") _,
    );
    id
}

#[inline(always)]
pub unsafe fn sys_surface_present(id: u64, x: i32, y: i32) -> u64 {
    let res: u64;
    core::arch::asm!(
        "syscall",
        inout("rax") SYS_SURFACE_PRESENT => res,
        in("rdi") id,
        in("rsi") x as u64,
        in("rdx") y as u64,
        out("rcx") _, out("r11") _,
    );
    res
}

#[inline(always)]
pub unsafe fn sys_surface_focus(id: u64) -> u64 {
    let res: u64;
    core::arch::asm!(
        "syscall",
        inout("rax") SYS_SURFACE_FOCUS => res,
        in("rdi") id,
        out("rcx") _, out("r11") _,
    );
    res
}

#[inline(always)]
pub unsafe fn sys_surface_close(id: u64) -> u64 {
    let res: u64;
    core::arch::asm!(
        "syscall",
        inout("rax") SYS_SURFACE_CLOSE => res,
        in("rdi") id,
        out("rcx") _, out("r11") _,
    );
    res
}

/// Voluntarily yield the CPU (SYS_YIELD, 28). Forces a context switch so the
/// gs-base survival proof can confirm the scheduler restored our GS base.
#[inline(always)]
pub unsafe fn sys_yield() {
    core::arch::asm!(
        "syscall",
        in("rax") SYS_YIELD,
        out("rcx") _, out("r11") _,
        // SYS_YIELD has no return value; rax is clobbered by the kernel.
        lateout("rax") _,
    );
}

/// Set this task's user-visible GS base to `base` (SYS_SET_GS_BASE, 282). After
/// this returns, guest `gs:[off]` accesses resolve against the structure at
/// `base` (the Win32 TEB) and the value survives context switches. Returns `0`
/// on success, `u64::MAX` on a non-canonical / kernel-half address.
#[inline(always)]
pub unsafe fn sys_set_gs_base(base: u64) -> u64 {
    let r: u64;
    core::arch::asm!(
        "syscall",
        inout("rax") SYS_SET_GS_BASE => r,
        in("rdi") base,
        out("rcx") _, out("r11") _,
    );
    r
}

/// Change the protection flags on `[addr, addr+len)` (SYS_MPROTECT, 283). `addr`
/// must be page-aligned; `prot` is a `PROT_*` bitmask. Used for the W^X RW→RX
/// flip on `.text` after copy+reloc+IAT. Returns `0` on success, `u64::MAX` on a
/// bad range / unmapped page / disallowed transition.
#[inline(always)]
pub unsafe fn sys_mprotect(addr: u64, len: u64, prot: u64) -> u64 {
    let r: u64;
    core::arch::asm!(
        "syscall",
        inout("rax") SYS_MPROTECT => r,
        in("rdi") addr,
        in("rsi") len,
        in("rdx") prot,
        out("rcx") _, out("r11") _,
    );
    r
}
