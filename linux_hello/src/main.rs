//! linux_hello — Linux x86_64 ABI conformance fixture (MasterChecklist: Linux
//! syscall translation / `linux_exec` route).
//!
//! This binary speaks ONLY the Linux x86_64 syscall ABI — raw `syscall`
//! instructions with Linux numbers, no relibc, no rae_abi. xtask stamps its
//! ELF osabi byte to ELFOSABI_LINUX (`config/base.toml`: `abi = "linux"`), so
//! the kernel's SYS_SPAWN routes it through `linux_exec`: Linux auxv stack,
//! Linux syscall-table marking, console fds. What it proves end-to-end:
//!
//!   1. `arch_prctl(ARCH_SET_FS)` installs a TLS block (Linux TLS model)
//!   2. the TLS pointer SURVIVES context switches — `sched_yield` ×8 with an
//!      `fs:`-relative magic read after every yield (Task::fs_base restore)
//!   3. `arch_prctl(ARCH_GET_FS)` copies the stored base back out
//!   4. `write(1, …)` reaches the console (serial) through the Linux fd table
//!   5. `exit_group(0)` terminates the task cleanly
//!
//! Serial proof: `[linux_hello] Linux ABI OK: TLS survived 8 yields` followed
//! by user_init's reap sentinel `msg: 8770`.

#![no_std]
#![no_main]

use core::arch::asm;

// Linux x86_64 syscall numbers (unistd_64.h).
const SYS_WRITE: u64 = 1;
const SYS_SCHED_YIELD: u64 = 24;
const SYS_ARCH_PRCTL: u64 = 158;
const SYS_EXIT_GROUP: u64 = 231;

const ARCH_SET_FS: u64 = 0x1002;
const ARCH_GET_FS: u64 = 0x1003;

const TLS_MAGIC: u64 = 0x5241_4545_4E4F_5321; // "RAEENOS!"

/// Minimal TLS block. Linux convention: `fs:0` holds the TCB self-pointer;
/// we keep a magic at `fs:8` to detect a clobbered or stale FS base.
#[repr(C, align(64))]
struct TlsBlock {
    self_ptr: u64,
    magic: u64,
}

static mut TLS: TlsBlock = TlsBlock {
    self_ptr: 0,
    magic: 0,
};

#[inline(always)]
unsafe fn syscall1(nr: u64, a1: u64) -> i64 {
    let ret: i64;
    asm!(
        "syscall",
        inlateout("rax") nr => ret,
        in("rdi") a1,
        out("rcx") _, out("r11") _,
        options(nostack),
    );
    ret
}

#[inline(always)]
unsafe fn syscall2(nr: u64, a1: u64, a2: u64) -> i64 {
    let ret: i64;
    asm!(
        "syscall",
        inlateout("rax") nr => ret,
        in("rdi") a1,
        in("rsi") a2,
        out("rcx") _, out("r11") _,
        options(nostack),
    );
    ret
}

#[inline(always)]
unsafe fn syscall3(nr: u64, a1: u64, a2: u64, a3: u64) -> i64 {
    let ret: i64;
    asm!(
        "syscall",
        inlateout("rax") nr => ret,
        in("rdi") a1,
        in("rsi") a2,
        in("rdx") a3,
        out("rcx") _, out("r11") _,
        options(nostack),
    );
    ret
}

unsafe fn write_str(s: &str) {
    let _ = syscall3(SYS_WRITE, 1, s.as_ptr() as u64, s.len() as u64);
}

/// Read the magic slot through the live FS base (`fs:8`).
#[inline(always)]
unsafe fn tls_magic_via_fs() -> u64 {
    let v: u64;
    asm!("mov {}, fs:[8]", out(reg) v, options(nostack, readonly));
    v
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    unsafe {
        // 1. Install the TLS block (RIP-relative addressing — works at any
        //    load base without relocations).
        let tls = core::ptr::addr_of_mut!(TLS);
        (*tls).self_ptr = tls as u64;
        (*tls).magic = TLS_MAGIC;
        if syscall2(SYS_ARCH_PRCTL, ARCH_SET_FS, tls as u64) != 0 {
            write_str("[linux_hello] FAIL: arch_prctl(ARCH_SET_FS)\n");
            syscall1(SYS_EXIT_GROUP, 2);
            unreachable!()
        }

        // 2. TLS must survive the scheduler: yield repeatedly and re-read the
        //    magic through FS after each switch-in.
        let mut ok = tls_magic_via_fs() == TLS_MAGIC;
        for _ in 0..8 {
            let _ = syscall1(SYS_SCHED_YIELD, 0);
            if tls_magic_via_fs() != TLS_MAGIC {
                ok = false;
            }
        }

        // 3. ARCH_GET_FS round-trip.
        let mut reported: u64 = 0;
        let get_rc = syscall2(
            SYS_ARCH_PRCTL,
            ARCH_GET_FS,
            core::ptr::addr_of_mut!(reported) as u64,
        );
        if get_rc != 0 || reported != tls as u64 {
            ok = false;
        }

        // 4. Report through the Linux write path.
        if ok {
            write_str("[linux_hello] Linux ABI OK: TLS survived 8 yields\n");
        } else {
            write_str("[linux_hello] FAIL: TLS lost across yields or GET_FS mismatch\n");
        }

        // 5. Clean Linux process exit.
        syscall1(SYS_EXIT_GROUP, if ok { 0 } else { 1 });
        unreachable!()
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    unsafe {
        syscall1(SYS_EXIT_GROUP, 99);
    }
    loop {}
}
