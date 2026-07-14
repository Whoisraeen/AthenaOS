//! Linux x86_64 syscall translation layer for AthenaOS.
//!
//! Maps Linux syscall numbers (the real x86_64 ABI from `asm/unistd_64.h`)
//! to our POSIX layer implementations. When a Linux ELF binary issues a
//! `syscall` instruction, the kernel detects it via the ELF OS/ABI field
//! and routes through this dispatch table instead of the native AthenaOS
//! syscall handler.
//!
//! This is NOT a Linux kernel clone — it's a translation shim that lets
//! unmodified Linux binaries run on AthenaOS by mapping their syscalls to
//! AthenaOS's own subsystems.

#![allow(dead_code)]

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use spin::Mutex;

use crate::posix::{self, Errno};
use crate::syscall::SyscallRegisters;

// ═══════════════════════════════════════════════════════════════════════════════
// Linux x86_64 Syscall Numbers (from asm/unistd_64.h)
// ═══════════════════════════════════════════════════════════════════════════════

pub const SYS_READ: u64 = 0;
pub const SYS_WRITE: u64 = 1;
pub const SYS_OPEN: u64 = 2;
pub const SYS_CLOSE: u64 = 3;
pub const SYS_STAT: u64 = 4;
pub const SYS_FSTAT: u64 = 5;
pub const SYS_LSTAT: u64 = 6;
pub const SYS_POLL: u64 = 7;
pub const SYS_LSEEK: u64 = 8;
pub const SYS_MMAP: u64 = 9;
pub const SYS_MPROTECT: u64 = 10;
pub const SYS_MUNMAP: u64 = 11;
pub const SYS_BRK: u64 = 12;
pub const SYS_RT_SIGACTION: u64 = 13;
pub const SYS_RT_SIGPROCMASK: u64 = 14;
pub const SYS_RT_SIGRETURN: u64 = 15;
pub const SYS_IOCTL: u64 = 16;
pub const SYS_PREAD64: u64 = 17;
pub const SYS_PWRITE64: u64 = 18;
pub const SYS_READV: u64 = 19;
pub const SYS_WRITEV: u64 = 20;
pub const SYS_ACCESS: u64 = 21;
pub const SYS_PIPE: u64 = 22;
pub const SYS_SELECT: u64 = 23;
pub const SYS_SCHED_YIELD: u64 = 24;
pub const SYS_MREMAP: u64 = 25;
pub const SYS_MSYNC: u64 = 26;
pub const SYS_DUP: u64 = 32;
pub const SYS_DUP2: u64 = 33;
pub const SYS_NANOSLEEP: u64 = 35;
pub const SYS_GETPID: u64 = 39;
pub const SYS_SOCKET: u64 = 41;
pub const SYS_CONNECT: u64 = 42;
pub const SYS_ACCEPT: u64 = 43;
pub const SYS_SENDTO: u64 = 44;
pub const SYS_RECVFROM: u64 = 45;
pub const SYS_BIND: u64 = 49;
pub const SYS_LISTEN: u64 = 50;
pub const SYS_CLONE: u64 = 56;
pub const SYS_FORK: u64 = 57;
pub const SYS_VFORK: u64 = 58;
pub const SYS_EXECVE: u64 = 59;
pub const SYS_EXIT: u64 = 60;
pub const SYS_WAIT4: u64 = 61;
pub const SYS_KILL: u64 = 62;
pub const SYS_UNAME: u64 = 63;
pub const SYS_FCNTL: u64 = 72;
pub const SYS_FTRUNCATE: u64 = 77;
pub const SYS_GETCWD: u64 = 79;
pub const SYS_CHDIR: u64 = 80;
pub const SYS_MKDIR: u64 = 83;
pub const SYS_RMDIR: u64 = 84;
pub const SYS_UNLINK: u64 = 87;
pub const SYS_READLINK: u64 = 89;
pub const SYS_GETTIMEOFDAY: u64 = 96;
pub const SYS_GETUID: u64 = 102;
pub const SYS_GETGID: u64 = 104;
pub const SYS_SETUID: u64 = 105;
pub const SYS_SETGID: u64 = 106;
pub const SYS_GETEUID: u64 = 107;
pub const SYS_GETEGID: u64 = 108;
pub const SYS_GETPPID: u64 = 110;
pub const SYS_GETPGRP: u64 = 111;
pub const SYS_SETSID: u64 = 112;
pub const SYS_GETGROUPS: u64 = 115;
pub const SYS_SIGALTSTACK: u64 = 131;
pub const SYS_PRCTL: u64 = 157;
pub const SYS_ARCH_PRCTL: u64 = 158;
pub const SYS_GETTID: u64 = 186;
pub const SYS_FUTEX: u64 = 202;
pub const SYS_SET_TID_ADDRESS: u64 = 218;
pub const SYS_CLOCK_GETTIME: u64 = 228;
pub const SYS_CLOCK_GETRES: u64 = 229;
pub const SYS_CLOCK_NANOSLEEP: u64 = 230;
pub const SYS_EXIT_GROUP: u64 = 231;
pub const SYS_EPOLL_CREATE: u64 = 213;
pub const SYS_EPOLL_CTL: u64 = 233;
pub const SYS_EPOLL_WAIT: u64 = 232;
pub const SYS_EPOLL_PWAIT: u64 = 281;
pub const SYS_EPOLL_PWAIT2: u64 = 441;
// Extended-attribute family. AthFS/the VFS expose no xattrs, so the LIST
// variants report an empty set (0) and the GET variants report "no such attr"
// (ENODATA) — what a real `ls -l` / `cp` expects on a no-xattr filesystem
// (oracle: a stock dynamically-linked `ls` calls llistxattr per entry).
pub const SYS_SETXATTR: u64 = 188;
pub const SYS_LSETXATTR: u64 = 189;
pub const SYS_FSETXATTR: u64 = 190;
pub const SYS_GETXATTR: u64 = 191;
pub const SYS_LGETXATTR: u64 = 192;
pub const SYS_FGETXATTR: u64 = 193;
pub const SYS_LISTXATTR: u64 = 194;
pub const SYS_LLISTXATTR: u64 = 195;
pub const SYS_FLISTXATTR: u64 = 196;
pub const SYS_OPENAT: u64 = 257;
pub const SYS_MKDIRAT: u64 = 258;
pub const SYS_NEWFSTATAT: u64 = 262;
pub const SYS_UNLINKAT: u64 = 263;
pub const SYS_SET_ROBUST_LIST: u64 = 273;
pub const SYS_EVENTFD: u64 = 284;
pub const SYS_EVENTFD2: u64 = 290;
pub const SYS_EPOLL_CREATE1: u64 = 291;
pub const SYS_DUP3: u64 = 292;
pub const SYS_PIPE2: u64 = 293;
pub const SYS_TIMERFD_CREATE: u64 = 283;
pub const SYS_TIMERFD_SETTIME: u64 = 286;
pub const SYS_TIMERFD_GETTIME: u64 = 287;
pub const SYS_SIGNALFD: u64 = 282;
pub const SYS_SIGNALFD4: u64 = 289;
pub const SYS_GETRANDOM: u64 = 318;
pub const SYS_PRLIMIT64: u64 = 302;
pub const SYS_RSEQ: u64 = 334;
pub const SYS_GETDENTS64: u64 = 217;
pub const SYS_GETDENTS: u64 = 78;
// Linux-compat batch (Athena-oracle-found gaps, 2026-06-23): advisory/no-op + *at
// variants real static binaries issue at startup/runtime (strace ground truth).
pub const SYS_MADVISE: u64 = 28;
pub const SYS_FCHMOD: u64 = 91;
pub const SYS_FCHOWN: u64 = 93;
pub const SYS_UMASK: u64 = 95;
pub const SYS_SETPGID: u64 = 109;
pub const SYS_SCHED_SETAFFINITY: u64 = 203;
pub const SYS_FADVISE64: u64 = 221;
pub const SYS_READLINKAT: u64 = 267;
pub const SYS_FACCESSAT: u64 = 269;
pub const SYS_UTIMENSAT: u64 = 280;
pub const SYS_MEMBARRIER: u64 = 324;
pub const SYS_FACCESSAT2: u64 = 439;
pub const SYS_GETRUSAGE: u64 = 98;
pub const SYS_TIMES: u64 = 100;
pub const SYS_SCHED_GETAFFINITY: u64 = 204;
pub const SYS_STATX: u64 = 332;
// Linux-compat batch 3 (the remaining common gaps).
pub const SYS_SENDMSG: u64 = 46;
pub const SYS_RECVMSG: u64 = 47;
pub const SYS_SHUTDOWN: u64 = 48;
pub const SYS_GETSOCKNAME: u64 = 51;
pub const SYS_GETPEERNAME: u64 = 52;
pub const SYS_SETSOCKOPT: u64 = 54;
pub const SYS_GETSOCKOPT: u64 = 55;
pub const SYS_SYSINFO: u64 = 99;
pub const SYS_RT_SIGTIMEDWAIT: u64 = 128;
pub const SYS_STATFS: u64 = 137;
pub const SYS_FSTATFS: u64 = 138;
pub const SYS_WAITID: u64 = 247;
pub const SYS_PSELECT6: u64 = 270;
pub const SYS_PPOLL: u64 = 271;
pub const SYS_FALLOCATE: u64 = 285;

// ═══════════════════════════════════════════════════════════════════════════════
// Linux-specific constants
// ═══════════════════════════════════════════════════════════════════════════════

pub const CLONE_VM: u64 = 0x0000_0100;
pub const CLONE_FS: u64 = 0x0000_0200;
pub const CLONE_FILES: u64 = 0x0000_0400;
pub const CLONE_SIGHAND: u64 = 0x0000_0800;
pub const CLONE_THREAD: u64 = 0x0001_0000;
pub const CLONE_NEWNS: u64 = 0x0002_0000;
pub const CLONE_SYSVSEM: u64 = 0x0004_0000;
pub const CLONE_SETTLS: u64 = 0x0008_0000;
pub const CLONE_PARENT_SETTID: u64 = 0x0010_0000;
pub const CLONE_CHILD_CLEARTID: u64 = 0x0020_0000;
pub const CLONE_CHILD_SETTID: u64 = 0x0100_0000;

pub const FUTEX_WAIT: u32 = 0;
pub const FUTEX_WAKE: u32 = 1;
pub const FUTEX_PRIVATE_FLAG: u32 = 128;
pub const FUTEX_WAIT_PRIVATE: u32 = FUTEX_WAIT | FUTEX_PRIVATE_FLAG;
pub const FUTEX_WAKE_PRIVATE: u32 = FUTEX_WAKE | FUTEX_PRIVATE_FLAG;

pub const PR_SET_NAME: u64 = 15;
pub const PR_GET_NAME: u64 = 16;
pub const PR_SET_NO_NEW_PRIVS: u64 = 38;
pub const PR_GET_NO_NEW_PRIVS: u64 = 39;
pub const PR_SET_SECCOMP: u64 = 22;

pub const ARCH_SET_GS: u64 = 0x1001;
pub const ARCH_SET_FS: u64 = 0x1002;
pub const ARCH_GET_FS: u64 = 0x1003;
pub const ARCH_GET_GS: u64 = 0x1004;

pub const SIG_DFL: u64 = 0;
pub const SIG_IGN: u64 = 1;
pub const SI_USER: i32 = 0;
pub const SI_KERNEL: i32 = 0x80;
pub const SA_SIGINFO: u64 = 0x0000_0004;

pub const AT_FDCWD: i32 = -100;
pub const AT_REMOVEDIR: u64 = 0x200;

// Epoll constants
pub const EPOLL_CTL_ADD: u32 = 1;
pub const EPOLL_CTL_DEL: u32 = 2;
pub const EPOLL_CTL_MOD: u32 = 3;
pub const EPOLLIN: u32 = 0x001;
pub const EPOLLOUT: u32 = 0x004;
pub const EPOLLERR: u32 = 0x008;
pub const EPOLLHUP: u32 = 0x010;
pub const EPOLLET: u32 = 1 << 31;

// ═══════════════════════════════════════════════════════════════════════════════
// Linux uname structure
// ═══════════════════════════════════════════════════════════════════════════════

#[repr(C)]
pub struct Utsname {
    pub sysname: [u8; 65],
    pub nodename: [u8; 65],
    pub release: [u8; 65],
    pub version: [u8; 65],
    pub machine: [u8; 65],
    pub domainname: [u8; 65],
}

impl Utsname {
    fn athenaos_default() -> Self {
        let mut u = Utsname {
            sysname: [0; 65],
            nodename: [0; 65],
            release: [0; 65],
            version: [0; 65],
            machine: [0; 65],
            domainname: [0; 65],
        };
        Self::fill_field(&mut u.sysname, b"Linux");
        Self::fill_field(&mut u.nodename, b"athenaos");
        Self::fill_field(&mut u.release, b"6.1.0-athenaos");
        Self::fill_field(&mut u.version, b"#1 SMP AthenaOS");
        Self::fill_field(&mut u.machine, b"x86_64");
        Self::fill_field(&mut u.domainname, b"(none)");
        u
    }

    fn fill_field(field: &mut [u8; 65], value: &[u8]) {
        let n = value.len().min(64);
        field[..n].copy_from_slice(&value[..n]);
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Epoll infrastructure
// ═══════════════════════════════════════════════════════════════════════════════

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct EpollEvent {
    pub events: u32,
    pub data: u64,
}

struct EpollInstance {
    fds: BTreeMap<i32, EpollEvent>,
}

impl EpollInstance {
    fn new() -> Self {
        Self {
            fds: BTreeMap::new(),
        }
    }
}

static EPOLL_TABLE: Mutex<BTreeMap<u64, EpollInstance>> = Mutex::new(BTreeMap::new());
static NEXT_EPOLL_ID: AtomicU64 = AtomicU64::new(1);
/// Process file-mode creation mask (`umask`). Default 022, as on Linux. Tracked so
/// `umask(new)` returns the real previous value (programs save/restore it).
static UMASK: AtomicU64 = AtomicU64::new(0o022);
static EPOLL_FD_TO_ID: Mutex<BTreeMap<i32, u64>> = Mutex::new(BTreeMap::new());

// ═══════════════════════════════════════════════════════════════════════════════
// Futex infrastructure
// ═══════════════════════════════════════════════════════════════════════════════

// The wait queue itself lives in `crate::sync::FUTEX_MANAGER` — physical-frame
// keyed so a word shared between two processes (SYS_CHANNEL_SHMEM_MAP) resolves
// to one queue, and shared with the in-kernel NVMe completion-park path. These
// functions are the syscall-facing entry points. (MasterChecklist item 1828.)
static ROBUST_LIST_HEADS: Mutex<BTreeMap<u64, (u64, u64)>> = Mutex::new(BTreeMap::new());

/// Outcome of preparing a futex WAIT for a caller that CAN block — the native
/// SYS_FUTEX(258) path, which holds `regs`. `Block(phys)` means the waiter is
/// already registered in the queue and the caller should
/// `block_current_task(BlockedOnFutex(phys), regs)` then retry the syscall on
/// wake (re-checking the word).
pub enum FutexPrep {
    Block(u64),
    Eagain,
    Fault,
}

/// Native (blocking) futex WAIT preparation. Translates the user word to its
/// physical-frame key and, under the futex-table lock, checks the word still
/// equals `val` before registering the current task — so a waker that already
/// mutated the word is seen here and we do not park on a stale value
/// (expected-compare under the wait-queue lock). See `sync::FutexManager::wait`
/// for the residual post-register/pre-park window and its backstop.
pub fn futex_prepare_wait(addr: u64, val: u32) -> FutexPrep {
    let phys = match crate::sync::phys_key(addr) {
        Some(p) => p,
        None => return FutexPrep::Fault,
    };
    let task_id = match crate::scheduler::current_task_id() {
        Some(t) => t,
        None => return FutexPrep::Eagain,
    };
    if crate::sync::FUTEX_MANAGER.lock().wait(phys, val, task_id) {
        FutexPrep::Block(phys)
    } else {
        FutexPrep::Eagain
    }
}

/// Cooperative futex WAIT for the Linux ABI path (syscall 202), which returns a
/// value into rax with no `regs` in scope and so cannot use the block+retry
/// idiom the native path uses. Registers the waiter phys-keyed (so a
/// cross-process wake reaches it), yields once, then reports whether it is
/// still parked (→ EAGAIN) or was drained by a waker (→ Ok = woken). A woken
/// cooperative waiter is left Ready — the drain already removed it from the
/// queue, so it observes its own absence here.
pub fn futex_wait(addr: u64, val: u32, _timeout: u64) -> Result<(), Errno> {
    let phys = match crate::sync::phys_key(addr) {
        Some(p) => p,
        None => return Err(Errno::Efault),
    };
    let task_id = match crate::scheduler::current_task_id() {
        Some(t) => t,
        None => return Err(Errno::Eagain),
    };
    // Expected-compare + register under the table lock (Invariant 3): if the
    // word already changed, don't park.
    if !crate::sync::FUTEX_MANAGER.lock().wait(phys, val, task_id) {
        return Err(Errno::Eagain);
    }
    crate::scheduler::yield_task();
    if crate::sync::FUTEX_MANAGER.lock().deregister(phys, task_id) {
        Err(Errno::Eagain) // still parked → nobody woke us
    } else {
        Ok(()) // drained by a waker → woken
    }
}

/// Wake up to `count` futex waiters on the word at `addr`. Phys-keyed, so a
/// waker in one process reaches waiters in any process sharing the frame.
pub fn futex_wake(addr: u64, count: u32) -> Result<u32, Errno> {
    let phys = match crate::sync::phys_key(addr) {
        Some(p) => p,
        None => return Err(Errno::Efault),
    };
    let woken = crate::sync::FUTEX_MANAGER.lock().wake(phys, count as usize);
    Ok(woken as u32)
}

/// On task exit, honor `CLONE_CHILD_CLEARTID` / `set_tid_address(2)`: zero the
/// registered TID word in user space and futex-wake one waiter. This is the
/// kernel half of `pthread_join` — the joiner futex-waits on the joinee's TID
/// slot, and the joinee's exit clears it (→ join returns). Runs while the
/// exiting task's address space is still active (its CR3), so the user write
/// lands in the right AS. No-op when no address was registered.
fn clear_child_tid_on_exit() {
    let addr = crate::scheduler::with_current_task(|t| t.clear_child_tid).unwrap_or(0);
    if addr != 0 {
        write_user_buf(addr, &0i32.to_le_bytes());
        let _ = futex_wake(addr, 1);
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// EventFD infrastructure
// ═══════════════════════════════════════════════════════════════════════════════

struct EventFd {
    counter: AtomicU64,
    flags: u32,
}

static EVENTFD_TABLE: Mutex<BTreeMap<u64, EventFd>> = Mutex::new(BTreeMap::new());
static NEXT_EVENTFD_ID: AtomicU64 = AtomicU64::new(1);
static EVENTFD_FD_TO_ID: Mutex<BTreeMap<i32, u64>> = Mutex::new(BTreeMap::new());

// ═══════════════════════════════════════════════════════════════════════════════
// TimerFD infrastructure
// ═══════════════════════════════════════════════════════════════════════════════

struct TimerFd {
    clock_id: u32,
    armed: AtomicBool,
    expiry_ns: AtomicU64,
    interval_ns: AtomicU64,
}

static TIMERFD_TABLE: Mutex<BTreeMap<u64, TimerFd>> = Mutex::new(BTreeMap::new());
static NEXT_TIMERFD_ID: AtomicU64 = AtomicU64::new(1);
static TIMERFD_FD_TO_ID: Mutex<BTreeMap<i32, u64>> = Mutex::new(BTreeMap::new());

// ═══════════════════════════════════════════════════════════════════════════════
// SignalFD infrastructure
// ═══════════════════════════════════════════════════════════════════════════════

#[repr(C)]
#[derive(Clone, Copy)]
struct LinuxSignalfdSiginfo {
    ssi_signo: u32,
    ssi_errno: i32,
    ssi_code: i32,
    ssi_pid: u32,
    ssi_uid: u32,
    ssi_fd: i32,
    ssi_tid: u32,
    ssi_band: u32,
    ssi_overrun: u32,
    ssi_trapno: u32,
    ssi_status: i32,
    ssi_int: i32,
    ssi_ptr: u64,
    ssi_utime: u64,
    ssi_stime: u64,
    ssi_addr: u64,
    _pad: [u8; 48],
}

impl Default for LinuxSignalfdSiginfo {
    fn default() -> Self {
        Self {
            ssi_signo: 0,
            ssi_errno: 0,
            ssi_code: 0,
            ssi_pid: 0,
            ssi_uid: 0,
            ssi_fd: 0,
            ssi_tid: 0,
            ssi_band: 0,
            ssi_overrun: 0,
            ssi_trapno: 0,
            ssi_status: 0,
            ssi_int: 0,
            ssi_ptr: 0,
            ssi_utime: 0,
            ssi_stime: 0,
            ssi_addr: 0,
            _pad: [0u8; 48],
        }
    }
}

struct SignalFd {
    mask: u64,
    flags: u32,
    pending: VecDeque<LinuxSignalfdSiginfo>,
}

static SIGNALFD_TABLE: Mutex<BTreeMap<u64, SignalFd>> = Mutex::new(BTreeMap::new());
static NEXT_SIGNALFD_ID: AtomicU64 = AtomicU64::new(1);
static SIGNALFD_FD_TO_ID: Mutex<BTreeMap<i32, u64>> = Mutex::new(BTreeMap::new());
static LINUX_RT_SIGACTION_TABLE: Mutex<BTreeMap<(u64, u8), LinuxRtSigAction>> =
    Mutex::new(BTreeMap::new());
static LINUX_SIGMASK_TABLE: Mutex<BTreeMap<u64, u64>> = Mutex::new(BTreeMap::new());

#[derive(Clone, Copy)]
struct SavedSyscallRegs {
    rax: u64,
    rdx: u64,
    rbx: u64,
    rbp: u64,
    rsi: u64,
    rdi: u64,
    r8: u64,
    r9: u64,
    r10: u64,
    r12: u64,
    r13: u64,
    r14: u64,
    r15: u64,
    r11: u64,
    rcx: u64,
}

impl SavedSyscallRegs {
    fn from_regs(regs: &SyscallRegisters) -> Self {
        Self {
            rax: regs.rax,
            rdx: regs.rdx,
            rbx: regs.rbx,
            rbp: regs.rbp,
            rsi: regs.rsi,
            rdi: regs.rdi,
            r8: regs.r8,
            r9: regs.r9,
            r10: regs.r10,
            r12: regs.r12,
            r13: regs.r13,
            r14: regs.r14,
            r15: regs.r15,
            r11: regs.r11,
            rcx: regs.rcx,
        }
    }

    fn restore_into(&self, regs: &mut SyscallRegisters) {
        regs.rax = self.rax;
        regs.rdx = self.rdx;
        regs.rbx = self.rbx;
        regs.rbp = self.rbp;
        regs.rsi = self.rsi;
        regs.rdi = self.rdi;
        regs.r8 = self.r8;
        regs.r9 = self.r9;
        regs.r10 = self.r10;
        regs.r12 = self.r12;
        regs.r13 = self.r13;
        regs.r14 = self.r14;
        regs.r15 = self.r15;
        regs.r11 = self.r11;
        regs.rcx = self.rcx;
    }
}

#[derive(Clone, Copy)]
struct RtSigFrame {
    saved: SavedSyscallRegs,
    old_mask: u64,
}

static RT_SIGFRAME_STACKS: Mutex<BTreeMap<u64, Vec<RtSigFrame>>> = Mutex::new(BTreeMap::new());

// ═══════════════════════════════════════════════════════════════════════════════
// /proc/self/* emulation
// ═══════════════════════════════════════════════════════════════════════════════

pub fn proc_self_read(path: &str, buf: &mut [u8]) -> Result<usize, Errno> {
    let pid = posix::sys_getpid();

    if path == "/proc/self/exe" || path.starts_with("/proc/self/exe") {
        let exe = b"/usr/bin/app";
        let n = buf.len().min(exe.len());
        buf[..n].copy_from_slice(&exe[..n]);
        return Ok(n);
    }

    if path == "/proc/self/maps" {
        let maps_str = crate::process::read_proc_entry(&crate::process::ProcEntry {
            pid: Some(crate::process::Pid(pid)),
            name: String::from("maps"),
            entry_type: crate::process::ProcEntryType::ProcessMaps,
        });
        let bytes = maps_str.as_bytes();
        let n = buf.len().min(bytes.len());
        buf[..n].copy_from_slice(&bytes[..n]);
        return Ok(n);
    }

    if path == "/proc/self/status" {
        let status = crate::process::read_proc_entry(&crate::process::ProcEntry {
            pid: Some(crate::process::Pid(pid)),
            name: String::from("status"),
            entry_type: crate::process::ProcEntryType::ProcessStatus,
        });
        let bytes = status.as_bytes();
        let n = buf.len().min(bytes.len());
        buf[..n].copy_from_slice(&bytes[..n]);
        return Ok(n);
    }

    Err(Errno::Enoent)
}

// ═══════════════════════════════════════════════════════════════════════════════
// prctl handler
// ═══════════════════════════════════════════════════════════════════════════════

fn handle_prctl(option: u64, arg2: u64, _arg3: u64, _arg4: u64, _arg5: u64) -> i64 {
    match option {
        PR_SET_NAME => {
            // arg2 is a pointer to a 16-byte name string. We'd copy it
            // from userspace and set the task name. Stub: accept silently.
            let _ = arg2;
            0
        }
        PR_GET_NAME => {
            // Would copy the task name to arg2. Stub: zero-fill.
            0
        }
        PR_SET_NO_NEW_PRIVS => {
            // Accept: we always enforce no-new-privs by default via
            // AthGuard's capability model.
            0
        }
        PR_GET_NO_NEW_PRIVS => 1, // Always set
        PR_SET_SECCOMP => {
            // AthGuard handles sandboxing natively; accept the seccomp
            // request but don't install a BPF filter (not applicable).
            0
        }
        _ => Errno::Einval.as_neg(),
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// arch_prctl handler
// ═══════════════════════════════════════════════════════════════════════════════

fn handle_arch_prctl(code: u64, addr: u64) -> i64 {
    match code {
        ARCH_SET_FS => {
            // Set the TLS pointer. Persist in the task (the scheduler restores
            // IA32_FS_BASE from Task::fs_base on every switch-in) AND write the
            // MSR now so the value is live when this syscall returns. The MSR
            // path (not wrfsbase) because QEMU's default cpu lacks FSGSBASE.
            if addr >= 0x0000_8000_0000_0000 {
                return Errno::Eperm.as_neg(); // non-canonical / kernel-half address
            }
            let persisted = crate::scheduler::with_current_task_mut(|t| t.fs_base = addr).is_some();
            if !persisted {
                return Errno::Esrch.as_neg();
            }
            #[cfg(target_arch = "x86_64")]
            {
                x86_64::registers::model_specific::FsBase::write(crate::arch::VirtAddr::new(addr));
            }
            0
        }
        ARCH_SET_GS => {
            // Refused by design. AthenaOS keeps the CPU id in the user-visible
            // GS base (gdt::current_cpu_id) and PerCpuSyscall in
            // IA32_KERNEL_GS_BASE — honoring a user GS base would corrupt the
            // per-CPU scheme. x86_64 Linux libcs use FS exclusively for TLS,
            // so EINVAL here matches what real software tolerates.
            crate::serial_println!(
                "[linux] arch_prctl(ARCH_SET_GS, {:#x}) refused: GS carries the per-CPU id",
                addr
            );
            Errno::Einval.as_neg()
        }
        ARCH_GET_FS => {
            let fs = crate::scheduler::with_current_task(|t| t.fs_base).unwrap_or(0);
            if write_user_u64(addr, fs) {
                0
            } else {
                Errno::Efault.as_neg()
            }
        }
        ARCH_GET_GS => {
            // SET_GS is refused, so the observable user GS base is always 0.
            if write_user_u64(addr, 0) {
                0
            } else {
                Errno::Efault.as_neg()
            }
        }
        _ => Errno::Einval.as_neg(),
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Helper: read a null-terminated string from user memory
// ═══════════════════════════════════════════════════════════════════════════════

fn read_user_string(ptr: u64, max_len: usize) -> Option<String> {
    if ptr == 0 {
        return None;
    }
    let mut s = Vec::new();
    for i in 0..max_len {
        let addr = ptr.checked_add(i as u64)?;
        if addr >= 0x0000_8000_0000_0000 {
            return None;
        }
        // Fault-fixup read (matches the native copy_from_user path): an unmapped
        // or TOCTOU-unmapped user page returns Err instead of faulting the kernel
        // with no recovery. The old raw `*(addr as *const u8)` let a bad user
        // pointer fault ring 0 (and risk deadlock in the page-fault handler).
        let mut byte = [0u8; 1];
        unsafe {
            crate::extable::copy_user_with_fixup(addr as *const u8, byte.as_mut_ptr(), 1).ok()?;
        }
        if byte[0] == 0 {
            break;
        }
        s.push(byte[0]);
    }
    String::from_utf8(s).ok()
}

fn read_user_buf(ptr: u64, len: usize) -> Option<Vec<u8>> {
    if ptr == 0 || len == 0 {
        return Some(Vec::new());
    }
    if ptr
        .checked_add(len as u64)
        .map_or(true, |e| e > 0x0000_8000_0000_0000)
    {
        return None;
    }
    let mut buf = Vec::with_capacity(len);
    unsafe {
        buf.set_len(len);
        // Fault-fixup copy: an unmapped/raced user page yields Err (the caller
        // maps it to EFAULT) instead of an unrecoverable kernel page fault.
        if crate::extable::copy_user_with_fixup(ptr as *const u8, buf.as_mut_ptr(), len).is_err() {
            return None;
        }
    }
    Some(buf)
}

fn write_user_buf(ptr: u64, data: &[u8]) -> bool {
    if ptr == 0 || data.is_empty() {
        return true;
    }
    if ptr
        .checked_add(data.len() as u64)
        .map_or(true, |e| e > 0x0000_8000_0000_0000)
    {
        return false;
    }
    // Fault-fixup copy-out: a bad user destination returns false (EFAULT) rather
    // than faulting the kernel mid-write.
    unsafe {
        crate::extable::copy_user_with_fixup(data.as_ptr(), ptr as *mut u8, data.len()).is_ok()
    }
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct LinuxIovec {
    base: u64,
    len: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct LinuxFdSetWord {
    bits: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct LinuxItimerspec {
    interval: posix::Timespec,
    value: posix::Timespec,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct LinuxRtSigAction {
    handler: u64,
    flags: u64,
    restorer: u64,
    mask: u64,
}

fn read_user_timespec(ptr: u64) -> Option<posix::Timespec> {
    let size = core::mem::size_of::<posix::Timespec>();
    let buf = read_user_buf(ptr, size)?;
    if buf.len() != size {
        return None;
    }
    let mut ts = posix::Timespec::default();
    unsafe {
        core::ptr::copy_nonoverlapping(buf.as_ptr(), &mut ts as *mut _ as *mut u8, size);
    }
    Some(ts)
}

fn read_user_iovecs(ptr: u64, cnt: usize) -> Option<Vec<LinuxIovec>> {
    let size = core::mem::size_of::<LinuxIovec>();
    let total = cnt.checked_mul(size)?;
    let raw = read_user_buf(ptr, total)?;
    let mut out = Vec::with_capacity(cnt);
    for i in 0..cnt {
        let off = i * size;
        let mut iov = LinuxIovec::default();
        unsafe {
            core::ptr::copy_nonoverlapping(
                raw.as_ptr().add(off),
                &mut iov as *mut _ as *mut u8,
                size,
            );
        }
        out.push(iov);
    }
    Some(out)
}

fn read_user_u64(ptr: u64) -> Option<u64> {
    let buf = read_user_buf(ptr, 8)?;
    if buf.len() != 8 {
        return None;
    }
    Some(u64::from_le_bytes([
        buf[0], buf[1], buf[2], buf[3], buf[4], buf[5], buf[6], buf[7],
    ]))
}

fn write_user_u64(ptr: u64, value: u64) -> bool {
    write_user_buf(ptr, &value.to_le_bytes())
}

fn read_user_rt_sigaction(ptr: u64) -> Option<LinuxRtSigAction> {
    let size = core::mem::size_of::<LinuxRtSigAction>();
    let raw = read_user_buf(ptr, size)?;
    if raw.len() != size {
        return None;
    }
    let mut out = LinuxRtSigAction::default();
    unsafe {
        core::ptr::copy_nonoverlapping(raw.as_ptr(), &mut out as *mut _ as *mut u8, size);
    }
    Some(out)
}

fn write_user_rt_sigaction(ptr: u64, act: &LinuxRtSigAction) -> bool {
    let bytes = unsafe {
        core::slice::from_raw_parts(
            act as *const _ as *const u8,
            core::mem::size_of::<LinuxRtSigAction>(),
        )
    };
    write_user_buf(ptr, bytes)
}

fn set_linux_sigmask(pid: u64, new_mask: u64) {
    LINUX_SIGMASK_TABLE.lock().insert(pid, new_mask);
    let set = crate::signals::SignalSet(new_mask);
    let _ = posix::sys_sigprocmask(posix::SIG_SETMASK, Some(&set), None);
}

fn linux_sigmask(pid: u64) -> u64 {
    LINUX_SIGMASK_TABLE.lock().get(&pid).copied().unwrap_or(0)
}

fn dequeue_unmasked_pending_signal(pid: u64, mask: u64) -> Option<u32> {
    let mut guard = crate::process::PROCESS_TABLE.lock();
    let table = guard.as_mut()?;
    let proc_ = table.getpid_mut(crate::process::Pid(pid))?;
    let pending = proc_.pending_signals & !mask;
    if pending == 0 {
        return None;
    }
    for signo in 1u32..=64 {
        let bit = 1u64 << (signo - 1);
        if (pending & bit) != 0 {
            proc_.pending_signals &= !bit;
            return Some(signo);
        }
    }
    None
}

fn build_nonkill_siginfo(pid: u64, signo: u32) -> LinuxSignalfdSiginfo {
    let mut info = LinuxSignalfdSiginfo {
        ssi_signo: signo,
        ssi_code: SI_KERNEL,
        ..LinuxSignalfdSiginfo::default()
    };
    if signo == crate::process::Signal::SigChld as u32 {
        let guard = crate::process::PROCESS_TABLE.lock();
        if let Some(table) = guard.as_ref() {
            let children = table
                .getpid(crate::process::Pid(pid))
                .map(|p| p.children.clone())
                .unwrap_or_default();
            for child_pid in children.iter() {
                if let Some(child) = table.getpid(*child_pid) {
                    if child.state == crate::process::ProcessState::Zombie {
                        info.ssi_pid = child.pid.0 as u32;
                        info.ssi_uid = child.uid;
                        info.ssi_status = child.exit_code.unwrap_or(0);
                        break;
                    }
                }
            }
        }
    }
    info
}

fn maybe_deliver_signal_handler(regs: &mut SyscallRegisters, nr: u64) {
    if nr == SYS_RT_SIGRETURN {
        return;
    }
    let pid = posix::sys_getpid();
    let cur_mask = linux_sigmask(pid);
    let signo = match dequeue_unmasked_pending_signal(pid, cur_mask) {
        Some(s) => s,
        None => return,
    };
    let action = LINUX_RT_SIGACTION_TABLE
        .lock()
        .get(&(pid, signo as u8))
        .copied()
        .unwrap_or(LinuxRtSigAction {
            handler: SIG_DFL,
            flags: 0,
            restorer: 0,
            mask: 0,
        });

    if action.handler == SIG_DFL || action.handler == SIG_IGN {
        return;
    }

    let new_mask = cur_mask | action.mask | (1u64 << (signo - 1));
    set_linux_sigmask(pid, new_mask);

    let frame = RtSigFrame {
        saved: SavedSyscallRegs::from_regs(regs),
        old_mask: cur_mask,
    };
    RT_SIGFRAME_STACKS
        .lock()
        .entry(pid)
        .or_insert_with(Vec::new)
        .push(frame);

    // Route execution to handler on return to userspace.
    regs.rcx = action.handler;
    regs.rax = 0;
    regs.rdi = signo as u64;
    if (action.flags & SA_SIGINFO) != 0 {
        regs.rsi = 0; // would point to user siginfo_t frame
        regs.rdx = 0; // would point to user ucontext frame
    }
}

fn vfs_err_to_errno(code: u64) -> Errno {
    match code {
        0xFFFF_FFFF_FFFF_FD01 => Errno::Erofs,     // E_VFS_READONLY
        0xFFFF_FFFF_FFFF_FD02 => Errno::Enoent,    // E_VFS_NOT_FOUND
        0xFFFF_FFFF_FFFF_FD03 => Errno::Eexist,    // E_VFS_EXISTS
        0xFFFF_FFFF_FFFF_FD04 => Errno::Enotempty, // E_VFS_NOT_EMPTY
        0xFFFF_FFFF_FFFF_FD05 => Errno::Einval,    // E_VFS_INVAL
        _ => Errno::Eio,
    }
}

fn alloc_anon_fd() -> Result<i32, Errno> {
    let inode: alloc::sync::Arc<dyn crate::vfs::Inode> = alloc::sync::Arc::new(posix::DevNullInode);
    let file = crate::vfs::File::new(inode, 0);
    let file_arc = alloc::sync::Arc::new(spin::Mutex::new(file));

    let mut fd_result: i64 = Errno::Emfile.as_neg();
    crate::scheduler::with_current_task_mut(|task| {
        for (i, slot) in task.fds.iter_mut().enumerate() {
            if slot.is_none() {
                *slot = Some(file_arc.clone());
                fd_result = i as i64;
                break;
            }
        }
    });
    if fd_result < 0 {
        Err(Errno::Emfile)
    } else {
        Ok(fd_result as i32)
    }
}

fn fd_exists(fd: i32) -> bool {
    if fd < 0 {
        return false;
    }
    crate::scheduler::with_current_task(|task| {
        let idx = fd as usize;
        idx < task.fds.len() && task.fds[idx].is_some()
    })
    .unwrap_or(false)
}

fn now_ns(clock_id: u32) -> u64 {
    let mut ts = posix::Timespec::default();
    if posix::sys_clock_gettime(clock_id, &mut ts).is_ok() {
        return (ts.tv_sec as u64)
            .saturating_mul(1_000_000_000)
            .saturating_add(ts.tv_nsec as u64);
    }
    0
}

fn timespec_to_ns(ts: &posix::Timespec) -> u64 {
    if ts.tv_sec < 0 || ts.tv_nsec < 0 {
        return 0;
    }
    (ts.tv_sec as u64)
        .saturating_mul(1_000_000_000)
        .saturating_add(ts.tv_nsec as u64)
}

fn ns_to_timespec(ns: u64) -> posix::Timespec {
    posix::Timespec {
        tv_sec: (ns / 1_000_000_000) as i64,
        tv_nsec: (ns % 1_000_000_000) as i64,
    }
}

fn read_user_itimerspec(ptr: u64) -> Option<LinuxItimerspec> {
    let size = core::mem::size_of::<LinuxItimerspec>();
    let buf = read_user_buf(ptr, size)?;
    if buf.len() != size {
        return None;
    }
    let mut out = LinuxItimerspec::default();
    unsafe {
        core::ptr::copy_nonoverlapping(buf.as_ptr(), &mut out as *mut _ as *mut u8, size);
    }
    Some(out)
}

fn write_user_itimerspec(ptr: u64, spec: &LinuxItimerspec) -> bool {
    let bytes = unsafe {
        core::slice::from_raw_parts(
            spec as *const _ as *const u8,
            core::mem::size_of::<LinuxItimerspec>(),
        )
    };
    write_user_buf(ptr, bytes)
}

fn timerfd_readable(id: u64) -> bool {
    let table = TIMERFD_TABLE.lock();
    if let Some(t) = table.get(&id) {
        if !t.armed.load(Ordering::Relaxed) {
            return false;
        }
        let now = now_ns(t.clock_id);
        now >= t.expiry_ns.load(Ordering::Relaxed)
    } else {
        false
    }
}

fn signalfd_readable(id: u64) -> bool {
    refill_signalfd_from_process_pending(id);
    let table = SIGNALFD_TABLE.lock();
    table
        .get(&id)
        .map(|s| !s.pending.is_empty())
        .unwrap_or(false)
}

fn refill_signalfd_from_process_pending(id: u64) {
    let mask = {
        let table = SIGNALFD_TABLE.lock();
        match table.get(&id) {
            Some(sfd) => sfd.mask,
            None => return,
        }
    };
    if mask == 0 {
        return;
    }

    let pid = posix::sys_getpid();
    let pending_mask = {
        let mut guard = crate::process::PROCESS_TABLE.lock();
        if let Some(table) = guard.as_mut() {
            if let Some(proc_) = table.getpid_mut(crate::process::Pid(pid)) {
                let pending = proc_.pending_signals & mask;
                if pending != 0 {
                    proc_.pending_signals &= !pending;
                }
                pending
            } else {
                0
            }
        } else {
            0
        }
    };
    if pending_mask == 0 {
        return;
    }

    let mut table = SIGNALFD_TABLE.lock();
    if let Some(sfd) = table.get_mut(&id) {
        let pid = posix::sys_getpid();
        for signo in 1u32..=64 {
            let bit = 1u64 << (signo - 1);
            if (pending_mask & bit) != 0 {
                sfd.pending.push_back(build_nonkill_siginfo(pid, signo));
            }
        }
    }
}

fn fd_read_ready(fd: i32) -> bool {
    if let Some(id) = SIGNALFD_FD_TO_ID.lock().get(&fd).copied() {
        return signalfd_readable(id);
    }
    if let Some(id) = EVENTFD_FD_TO_ID.lock().get(&fd).copied() {
        if let Some(ev) = EVENTFD_TABLE.lock().get(&id) {
            return ev.counter.load(Ordering::Relaxed) > 0;
        }
        return false;
    }
    if let Some(id) = TIMERFD_FD_TO_ID.lock().get(&fd).copied() {
        return timerfd_readable(id);
    }
    fd_exists(fd)
}

fn fd_write_ready(fd: i32) -> bool {
    if SIGNALFD_FD_TO_ID.lock().contains_key(&fd) {
        return false;
    }
    if TIMERFD_FD_TO_ID.lock().contains_key(&fd) {
        return false;
    }
    fd_exists(fd)
}

// ═══════════════════════════════════════════════════════════════════════════════
// Main Dispatch: linux_syscall_dispatch
// ═══════════════════════════════════════════════════════════════════════════════

/// Entry point for Linux syscall translation. Called from the main syscall
/// handler when the calling process is a Linux ELF binary.
///
/// Linux x86_64 syscall ABI:
///   rax = syscall number
///   rdi = arg1, rsi = arg2, rdx = arg3
///   r10 = arg4, r8  = arg5, r9  = arg6
///   Return value in rax (negative = -errno on error)
pub fn linux_syscall_dispatch(regs: &mut SyscallRegisters) {
    let nr = regs.rax;
    TOTAL_DISPATCHED.fetch_add(1, Ordering::Relaxed);
    LAST_NR.store(nr, Ordering::Relaxed);
    let a1 = regs.rdi;
    let a2 = regs.rsi;
    let a3 = regs.rdx;
    let a4 = regs.r10;
    let a5 = regs.r8;
    let _a6 = regs.r9;

    let result: i64 = match nr {
        // ── File I/O ─────────────────────────────────────────────────
        SYS_READ => {
            let fd = a1 as i32;
            if let Some(signal_id) = SIGNALFD_FD_TO_ID.lock().get(&fd).copied() {
                refill_signalfd_from_process_pending(signal_id);
                let info_size = core::mem::size_of::<LinuxSignalfdSiginfo>();
                if (a3 as usize) < info_size {
                    Errno::Einval.as_neg()
                } else {
                    let mut table = SIGNALFD_TABLE.lock();
                    if let Some(sfd) = table.get_mut(&signal_id) {
                        if let Some(info) = sfd.pending.pop_front() {
                            let bytes = unsafe {
                                core::slice::from_raw_parts(
                                    &info as *const _ as *const u8,
                                    core::mem::size_of::<LinuxSignalfdSiginfo>(),
                                )
                            };
                            if write_user_buf(a2, bytes) {
                                info_size as i64
                            } else {
                                Errno::Efault.as_neg()
                            }
                        } else {
                            Errno::Eagain.as_neg()
                        }
                    } else {
                        Errno::Ebadf.as_neg()
                    }
                }
            } else if let Some(event_id) = EVENTFD_FD_TO_ID.lock().get(&fd).copied() {
                if a3 < 8 {
                    Errno::Einval.as_neg()
                } else if let Some(ev) = EVENTFD_TABLE.lock().get(&event_id) {
                    let value = ev.counter.swap(0, Ordering::Relaxed);
                    if value == 0 {
                        Errno::Eagain.as_neg()
                    } else if write_user_buf(a2, &value.to_le_bytes()) {
                        8
                    } else {
                        Errno::Efault.as_neg()
                    }
                } else {
                    Errno::Ebadf.as_neg()
                }
            } else if let Some(timer_id) = TIMERFD_FD_TO_ID.lock().get(&fd).copied() {
                if a3 < 8 {
                    Errno::Einval.as_neg()
                } else {
                    let mut table = TIMERFD_TABLE.lock();
                    if let Some(t) = table.get_mut(&timer_id) {
                        if !t.armed.load(Ordering::Relaxed) {
                            Errno::Eagain.as_neg()
                        } else {
                            let now = now_ns(t.clock_id);
                            let expiry = t.expiry_ns.load(Ordering::Relaxed);
                            if now < expiry {
                                Errno::Eagain.as_neg()
                            } else {
                                let interval = t.interval_ns.load(Ordering::Relaxed);
                                let expirations = if interval > 0 {
                                    let overdue = now.saturating_sub(expiry);
                                    let n = 1 + (overdue / interval);
                                    t.expiry_ns.store(
                                        expiry.saturating_add(n.saturating_mul(interval)),
                                        Ordering::Relaxed,
                                    );
                                    n
                                } else {
                                    t.armed.store(false, Ordering::Relaxed);
                                    1
                                };
                                if write_user_buf(a2, &expirations.to_le_bytes()) {
                                    8
                                } else {
                                    Errno::Efault.as_neg()
                                }
                            }
                        }
                    } else {
                        Errno::Ebadf.as_neg()
                    }
                }
            } else if let Some(mut buf) = read_user_buf(a2, a3 as usize) {
                match posix::sys_read(a1 as u32, &mut buf) {
                    Ok(n) => {
                        write_user_buf(a2, &buf[..n]);
                        n as i64
                    }
                    Err(e) => e.as_neg(),
                }
            } else {
                Errno::Efault.as_neg()
            }
        }

        SYS_WRITE => {
            let fd = a1 as i32;
            if let Some(event_id) = EVENTFD_FD_TO_ID.lock().get(&fd).copied() {
                if a3 < 8 {
                    Errno::Einval.as_neg()
                } else if let Some(buf) = read_user_buf(a2, 8) {
                    let add = u64::from_le_bytes([
                        buf[0], buf[1], buf[2], buf[3], buf[4], buf[5], buf[6], buf[7],
                    ]);
                    if add == u64::MAX {
                        Errno::Einval.as_neg()
                    } else if let Some(ev) = EVENTFD_TABLE.lock().get(&event_id) {
                        let cur = ev.counter.load(Ordering::Relaxed);
                        if cur > u64::MAX - add {
                            Errno::Eagain.as_neg()
                        } else {
                            ev.counter.store(cur + add, Ordering::Relaxed);
                            8
                        }
                    } else {
                        Errno::Ebadf.as_neg()
                    }
                } else {
                    Errno::Efault.as_neg()
                }
            } else if TIMERFD_FD_TO_ID.lock().contains_key(&fd) {
                Errno::Einval.as_neg()
            } else if let Some(buf) = read_user_buf(a2, a3 as usize) {
                match posix::sys_write(a1 as u32, &buf) {
                    Ok(n) => n as i64,
                    Err(e) => e.as_neg(),
                }
            } else {
                Errno::Efault.as_neg()
            }
        }

        SYS_OPEN => {
            if let Some(path) = read_user_string(a1, 4096) {
                match posix::sys_open(&path, a2 as u32, a3 as u32) {
                    Ok(fd) => fd as i64,
                    Err(e) => e.as_neg(),
                }
            } else {
                Errno::Efault.as_neg()
            }
        }

        SYS_OPENAT => {
            if let Some(path) = read_user_string(a2, 4096) {
                let effective_path = if a1 as i32 == AT_FDCWD || path.starts_with('/') {
                    path
                } else {
                    path
                };
                match posix::sys_open(&effective_path, a3 as u32, a4 as u32) {
                    Ok(fd) => fd as i64,
                    Err(e) => e.as_neg(),
                }
            } else {
                Errno::Efault.as_neg()
            }
        }

        SYS_CLOSE => {
            let fd = a1 as i32;
            if let Some(id) = EPOLL_FD_TO_ID.lock().remove(&fd) {
                EPOLL_TABLE.lock().remove(&id);
            }
            if let Some(id) = SIGNALFD_FD_TO_ID.lock().remove(&fd) {
                SIGNALFD_TABLE.lock().remove(&id);
            }
            if let Some(id) = EVENTFD_FD_TO_ID.lock().remove(&fd) {
                EVENTFD_TABLE.lock().remove(&id);
            }
            if let Some(id) = TIMERFD_FD_TO_ID.lock().remove(&fd) {
                TIMERFD_TABLE.lock().remove(&id);
            }
            match posix::sys_close(a1 as u32) {
                Ok(()) => 0,
                Err(e) => e.as_neg(),
            }
        }

        SYS_STAT | SYS_LSTAT => {
            if let Some(path) = read_user_string(a1, 4096) {
                let mut stat = posix::Stat::default();
                match posix::sys_stat(&path, &mut stat) {
                    Ok(()) => {
                        let bytes = unsafe {
                            core::slice::from_raw_parts(
                                &stat as *const _ as *const u8,
                                core::mem::size_of::<posix::Stat>(),
                            )
                        };
                        if write_user_buf(a2, bytes) {
                            0
                        } else {
                            Errno::Efault.as_neg()
                        }
                    }
                    Err(e) => e.as_neg(),
                }
            } else {
                Errno::Efault.as_neg()
            }
        }

        SYS_FSTAT => {
            let mut stat = posix::Stat::default();
            match posix::sys_fstat(a1 as u32, &mut stat) {
                Ok(()) => {
                    let bytes = unsafe {
                        core::slice::from_raw_parts(
                            &stat as *const _ as *const u8,
                            core::mem::size_of::<posix::Stat>(),
                        )
                    };
                    if write_user_buf(a2, bytes) {
                        0
                    } else {
                        Errno::Efault.as_neg()
                    }
                }
                Err(e) => e.as_neg(),
            }
        }

        SYS_NEWFSTATAT => {
            if let Some(path) = read_user_string(a2, 4096) {
                let mut stat = posix::Stat::default();
                match posix::sys_stat(&path, &mut stat) {
                    Ok(()) => {
                        let bytes = unsafe {
                            core::slice::from_raw_parts(
                                &stat as *const _ as *const u8,
                                core::mem::size_of::<posix::Stat>(),
                            )
                        };
                        if write_user_buf(a3, bytes) {
                            0
                        } else {
                            Errno::Efault.as_neg()
                        }
                    }
                    Err(e) => e.as_neg(),
                }
            } else {
                Errno::Efault.as_neg()
            }
        }

        SYS_STATX => {
            // statx(dirfd=a1, path=a2, flags=a3, mask=a4, buf=a5): map the WORKING
            // stat() data into the modern 256-byte statx struct (real metadata, not a
            // stub). Offsets verified against linux/stat.h via the Athena oracle.
            // AT_EMPTY_PATH(0x1000) + empty path => fstat the dirfd.
            let res = match read_user_string(a2, 4096) {
                Some(path) => {
                    let mut st = posix::Stat::default();
                    if path.is_empty() && (a3 & 0x1000) != 0 {
                        posix::sys_fstat(a1 as u32, &mut st).map(|_| st)
                    } else {
                        posix::sys_stat(&path, &mut st).map(|_| st)
                    }
                }
                None => Err(Errno::Efault),
            };
            match res {
                Ok(st) => {
                    let mut b = [0u8; 256];
                    let s32 = |b: &mut [u8; 256], o: usize, v: u32| {
                        b[o..o + 4].copy_from_slice(&v.to_le_bytes())
                    };
                    let s64 = |b: &mut [u8; 256], o: usize, v: u64| {
                        b[o..o + 8].copy_from_slice(&v.to_le_bytes())
                    };
                    const STATX_BASIC_STATS: u32 = 0x07ff; // TYPE..BLOCKS, all filled
                    s32(&mut b, 0, STATX_BASIC_STATS); // stx_mask
                    s32(&mut b, 4, st.st_blksize as u32); // stx_blksize
                    s32(&mut b, 16, st.st_nlink as u32); // stx_nlink
                    s32(&mut b, 20, st.st_uid); // stx_uid
                    s32(&mut b, 24, st.st_gid); // stx_gid
                    b[28..30].copy_from_slice(&(st.st_mode as u16).to_le_bytes()); // stx_mode
                    s64(&mut b, 32, st.st_ino); // stx_ino
                    s64(&mut b, 40, st.st_size as u64); // stx_size
                    s64(&mut b, 48, st.st_blocks as u64); // stx_blocks
                                                          // statx_timestamp = { s64 sec; u32 nsec; s32 _; } at a/b/c/m = 64/80/96/112
                    s64(&mut b, 64, st.st_atime as u64);
                    s32(&mut b, 72, st.st_atime_nsec as u32);
                    s64(&mut b, 96, st.st_ctime as u64);
                    s32(&mut b, 104, st.st_ctime_nsec as u32);
                    s64(&mut b, 112, st.st_mtime as u64);
                    s32(&mut b, 120, st.st_mtime_nsec as u32);
                    if write_user_buf(a5, &b) {
                        0
                    } else {
                        Errno::Efault.as_neg()
                    }
                }
                Err(e) => e.as_neg(),
            }
        }

        SYS_LSEEK => match posix::sys_lseek(a1 as u32, a2 as i64, a3 as u32) {
            Ok(pos) => pos as i64,
            Err(e) => e.as_neg(),
        },

        SYS_DUP => match posix::sys_dup(a1 as u32) {
            Ok(fd) => fd as i64,
            Err(e) => e.as_neg(),
        },

        SYS_DUP2 => match posix::sys_dup2(a1 as u32, a2 as u32) {
            Ok(fd) => fd as i64,
            Err(e) => e.as_neg(),
        },

        SYS_DUP3 => match posix::sys_dup2(a1 as u32, a2 as u32) {
            Ok(fd) => fd as i64,
            Err(e) => e.as_neg(),
        },

        SYS_PIPE => {
            let mut fds = [0u32; 2];
            match posix::sys_pipe(&mut fds) {
                Ok(()) => {
                    let bytes = [fds[0].to_le_bytes(), fds[1].to_le_bytes()];
                    let flat: [u8; 8] = unsafe { core::mem::transmute(bytes) };
                    if write_user_buf(a1, &flat) {
                        0
                    } else {
                        Errno::Efault.as_neg()
                    }
                }
                Err(e) => e.as_neg(),
            }
        }

        SYS_PIPE2 => {
            let mut fds = [0u32; 2];
            match posix::sys_pipe(&mut fds) {
                Ok(()) => {
                    let bytes = [fds[0].to_le_bytes(), fds[1].to_le_bytes()];
                    let flat: [u8; 8] = unsafe { core::mem::transmute(bytes) };
                    if write_user_buf(a1, &flat) {
                        0
                    } else {
                        Errno::Efault.as_neg()
                    }
                }
                Err(e) => e.as_neg(),
            }
        }

        SYS_FCNTL => match posix::sys_fcntl(a1 as u32, a2 as u32, a3) {
            Ok(v) => v as i64,
            Err(e) => e.as_neg(),
        },

        SYS_IOCTL => match posix::render_client_for_fd(a1 as u32) {
            Ok(Some(client_id)) => {
                let task_id = crate::scheduler::current_task_id()
                    .map(|id| id.raw())
                    .unwrap_or(0);
                match crate::gpu_render::prepare_ioctl(task_id, client_id, a2 as u32, a3) {
                    crate::gpu_render::IoctlAction::Complete(result) => result,
                    crate::gpu_render::IoctlAction::BlockNew {
                        request_id,
                        request,
                    } => {
                        regs.rcx -= 2;
                        crate::scheduler::block_current_task_with(
                            crate::task::TaskState::BlockedOnDrm(request_id),
                            || crate::gpu_render::enqueue_ioctl(request),
                        );
                        return;
                    }
                    crate::gpu_render::IoctlAction::BlockExisting { request_id } => {
                        regs.rcx -= 2;
                        crate::scheduler::block_current_task_with(
                            crate::task::TaskState::BlockedOnDrm(request_id),
                            || crate::gpu_render::wake_if_response(task_id, request_id),
                        );
                        return;
                    }
                }
            }
            Ok(None) => match posix::sys_ioctl(a1 as u32, a2, a3) {
                Ok(v) => v as i64,
                Err(e) => e.as_neg(),
            },
            Err(e) => e.as_neg(),
        },

        SYS_ACCESS => {
            if let Some(path) = read_user_string(a1, 4096) {
                if crate::vfs::open_path(&path).is_some() {
                    0
                } else {
                    Errno::Enoent.as_neg()
                }
            } else {
                Errno::Efault.as_neg()
            }
        }

        SYS_FACCESSAT | SYS_FACCESSAT2 => {
            // faccessat(dirfd=a1, path=a2, mode=a3[, flags=a4]). Resolve like access()
            // for AT_FDCWD/absolute paths (a dirfd-relative base isn't modeled yet).
            if let Some(path) = read_user_string(a2, 4096) {
                if crate::vfs::open_path(&path).is_some() {
                    0
                } else {
                    Errno::Enoent.as_neg()
                }
            } else {
                Errno::Efault.as_neg()
            }
        }

        SYS_GETDENTS | SYS_GETDENTS64 => {
            // `getdents64(fd, buf, count)` returns a stream of `linux_dirent64`.
            // We implement it by exposing a synthetic directory inode that
            // provides a stable `linux_dirent64` byte stream via read_at.
            if a2 == 0 || a3 == 0 {
                Errno::Efault.as_neg()
            } else if let Some(mut buf) = read_user_buf(a2, a3 as usize) {
                match posix::sys_read(a1 as u32, &mut buf) {
                    Ok(n) => {
                        write_user_buf(a2, &buf[..n]);
                        n as i64
                    }
                    Err(e) => e.as_neg(),
                }
            } else {
                Errno::Efault.as_neg()
            }
        }

        SYS_READLINK => {
            if let Some(path) = read_user_string(a1, 4096) {
                if path.starts_with("/proc/self/") {
                    let mut buf = alloc::vec![0u8; a3 as usize];
                    match proc_self_read(&path, &mut buf) {
                        Ok(n) => {
                            write_user_buf(a2, &buf[..n]);
                            n as i64
                        }
                        Err(e) => e.as_neg(),
                    }
                } else {
                    Errno::Einval.as_neg()
                }
            } else {
                Errno::Efault.as_neg()
            }
        }

        SYS_READLINKAT => {
            // readlinkat(dirfd=a1, path=a2, buf=a3, bufsize=a4): readlink() with the
            // dirfd-shifted args (a static glibc binary resolves /proc/self/exe here).
            if let Some(path) = read_user_string(a2, 4096) {
                if path.starts_with("/proc/self/") {
                    let mut buf = alloc::vec![0u8; a4 as usize];
                    match proc_self_read(&path, &mut buf) {
                        Ok(n) => {
                            write_user_buf(a3, &buf[..n]);
                            n as i64
                        }
                        Err(e) => e.as_neg(),
                    }
                } else {
                    Errno::Einval.as_neg()
                }
            } else {
                Errno::Efault.as_neg()
            }
        }

        // ── Memory ───────────────────────────────────────────────────
        SYS_MMAP => {
            const MAP_ANONYMOUS: u32 = 0x20;
            let render_client = if a5 as i32 >= 0 && (a4 as u32 & MAP_ANONYMOUS) == 0 {
                posix::render_client_for_fd(a5 as u32).ok().flatten()
            } else {
                None
            };
            if let Some(client_id) = render_client {
                let Some(map_len) = a2.checked_add(4095).map(|len| len & !4095) else {
                    return regs.rax = Errno::Enomem.as_neg() as u64;
                };
                let task_id = crate::scheduler::current_task_id()
                    .map(|id| id.raw())
                    .unwrap_or(0);
                match crate::gpu_render::prepare_mmap(task_id, client_id, _a6, map_len) {
                    crate::gpu_render::MmapAction::Complete(Ok(pages)) => {
                        match posix::sys_mmap_render_pages(
                            a1, map_len, a3 as u32, a4 as u32, &pages,
                        ) {
                            Ok(addr) => addr as i64,
                            Err(e) => e.as_neg(),
                        }
                    }
                    crate::gpu_render::MmapAction::Complete(Err(error)) => error,
                    crate::gpu_render::MmapAction::BlockNew {
                        request_id,
                        request,
                    } => {
                        regs.rcx -= 2;
                        crate::scheduler::block_current_task_with(
                            crate::task::TaskState::BlockedOnDrm(request_id),
                            || crate::gpu_render::enqueue_mmap(request),
                        );
                        return;
                    }
                    crate::gpu_render::MmapAction::BlockExisting { request_id } => {
                        regs.rcx -= 2;
                        crate::scheduler::block_current_task_with(
                            crate::task::TaskState::BlockedOnDrm(request_id),
                            || crate::gpu_render::wake_if_response(task_id, request_id),
                        );
                        return;
                    }
                }
            } else {
                match posix::sys_mmap(a1, a2, a3 as u32, a4 as u32, a5 as i32, _a6) {
                    Ok(addr) => addr as i64,
                    Err(e) => e.as_neg(),
                }
            }
        }

        SYS_MUNMAP => match posix::sys_munmap(a1, a2) {
            Ok(()) => 0,
            Err(e) => e.as_neg(),
        },

        SYS_MPROTECT => match posix::sys_mprotect(a1, a2, a3 as u32) {
            Ok(()) => 0,
            Err(e) => e.as_neg(),
        },

        SYS_BRK => match posix::sys_brk(a1) {
            Ok(brk) => brk as i64,
            Err(e) => e.as_neg(),
        },

        SYS_MREMAP => Errno::Enomem.as_neg(),
        SYS_MSYNC => 0,

        // ── Linux-compat: advisory no-ops + metadata the kernel may ignore ──────
        // memory/file access hints, the membarrier fence (single ops are already
        // ordered), affinity (the scheduler owns placement), and mode/owner/time
        // metadata a CoW FS treats as no-ops. Returning success keeps real Linux
        // programs (allocators, build tools, runtimes) from aborting on a hard
        // ENOSYS for an advisory call.
        SYS_MADVISE | SYS_FADVISE64 | SYS_MEMBARRIER => 0,
        SYS_FCHMOD | SYS_FCHOWN | SYS_UTIMENSAT => 0,
        SYS_SCHED_SETAFFINITY => 0,
        SYS_SETPGID => 0,
        SYS_UMASK => {
            // umask(mask): set the file-mode creation mask, return the previous.
            UMASK.swap((a1 as u32 & 0o777) as u64, Ordering::Relaxed) as i64
        }

        SYS_SCHED_GETAFFINITY => {
            // sched_getaffinity(pid=a1, cpusetsize=a2, mask=a3): report the ONLINE
            // CPUs so programs size thread pools correctly (this is how nproc/glibc
            // count usable CPUs). Low ONLINE_CPUS bits set; return bytes written.
            let cpus = crate::smp::ONLINE_CPUS.load(Ordering::Relaxed).min(64);
            let mask: u64 = if cpus >= 64 {
                u64::MAX
            } else {
                (1u64 << cpus) - 1
            };
            let n = (a2 as usize).min(8);
            if write_user_buf(a3, &mask.to_le_bytes()[..n]) {
                n as i64
            } else {
                Errno::Efault.as_neg()
            }
        }
        SYS_GETRUSAGE => {
            // getrusage(who=a1, usage=a2): no per-process accounting yet — return a
            // zeroed `struct rusage` (144 bytes) + success so readers get zeros
            // instead of aborting on ENOSYS.
            if write_user_buf(a2, &[0u8; 144]) {
                0
            } else {
                Errno::Efault.as_neg()
            }
        }
        SYS_TIMES => {
            // times(buf=a1): zeroed `struct tms` (32 bytes); return 0 ticks (a valid
            // monotonic epoch). The tms fields are what most callers read.
            if a1 != 0 && !write_user_buf(a1, &[0u8; 32]) {
                Errno::Efault.as_neg()
            } else {
                0
            }
        }

        // ── Linux-compat batch 3: socket opts/names + fs-stat + signal/alloc ────
        SYS_SHUTDOWN => 0, // shutdown(fd, how): accept (commonly called before close)
        SYS_SETSOCKOPT => 0, // accept + ignore (SO_REUSEADDR / TCP_NODELAY / ...)
        SYS_GETSOCKOPT => {
            // getsockopt(fd, level, optname, optval=a4, optlen=a5): report a 0 value
            // (SO_ERROR=0 "no error" is the common probe) + set *optlen = 4.
            if a4 != 0 && !write_user_buf(a4, &0u32.to_le_bytes()) {
                Errno::Efault.as_neg()
            } else {
                if a5 != 0 {
                    let _ = write_user_buf(a5, &4u32.to_le_bytes());
                }
                0
            }
        }
        SYS_GETSOCKNAME | SYS_GETPEERNAME => {
            // get{sock,peer}name(fd, addr=a2, addrlen=a3): socket addrs aren't tracked
            // yet — return a zeroed AF_INET sockaddr (16 bytes) + set *addrlen so the
            // caller reads a well-formed (if empty) address instead of aborting.
            let mut sa = [0u8; 16];
            sa[0] = 2; // sa_family = AF_INET
            if a2 != 0 && !write_user_buf(a2, &sa) {
                Errno::Efault.as_neg()
            } else {
                if a3 != 0 {
                    let _ = write_user_buf(a3, &16u32.to_le_bytes());
                }
                0
            }
        }
        SYS_RT_SIGTIMEDWAIT => Errno::Eagain.as_neg(), // timed out, no signal pending
        SYS_FALLOCATE => 0, // best-effort: the FS allocates on write; succeed so callers proceed
        SYS_STATFS | SYS_FSTATFS => {
            // statfs(path=a1, buf=a2) / fstatfs(fd=a1, buf=a2): no real AthFS block
            // accounting yet — report a plausible filesystem with ample free space (so
            // free-space checks pass) instead of ENOSYS. 120-byte layout from Athena's
            // asm-generic/statfs.h.
            let mut b = [0u8; 120];
            let s64 =
                |b: &mut [u8; 120], o: usize, v: i64| b[o..o + 8].copy_from_slice(&v.to_le_bytes());
            s64(&mut b, 0, 0x5241_4566); // f_type (AthFS magic)
            s64(&mut b, 8, 4096); // f_bsize
            s64(&mut b, 16, 0x0100_0000); // f_blocks (16M * 4K = 64 GB)
            s64(&mut b, 24, 0x00C0_0000); // f_bfree
            s64(&mut b, 32, 0x00C0_0000); // f_bavail
            s64(&mut b, 40, 0x0010_0000); // f_files
            s64(&mut b, 48, 0x000F_0000); // f_ffree
            s64(&mut b, 64, 255); // f_namelen
            s64(&mut b, 72, 4096); // f_frsize
            if write_user_buf(a2, &b) {
                0
            } else {
                Errno::Efault.as_neg()
            }
        }
        SYS_SYSINFO => {
            // sysinfo(info=a1): 112-byte struct (offsets verified via Athena offsetof:
            // totalram@32, freeram@40, procs@80, mem_unit@104). totalram is the REAL
            // physical total; freeram a conservative estimate (no global live free-page
            // counter yet); mem_unit=1 (ram values are bytes). uptime/loads/swap left 0.
            let total = crate::memory::physical_total_bytes().unwrap_or(0);
            let mut b = [0u8; 112];
            b[32..40].copy_from_slice(&total.to_le_bytes()); // totalram
            b[40..48].copy_from_slice(&(total / 4).to_le_bytes()); // freeram (conservative)
            b[80..82].copy_from_slice(&16u16.to_le_bytes()); // procs (approx)
            b[104..108].copy_from_slice(&1u32.to_le_bytes()); // mem_unit = 1 (bytes)
            if write_user_buf(a1, &b) {
                0
            } else {
                Errno::Efault.as_neg()
            }
        }

        // ── Process ──────────────────────────────────────────────────
        SYS_FORK | SYS_VFORK => {
            let pid = posix::sys_getpid();
            match posix::sys_fork(pid) {
                Ok(child_pid) => child_pid as i64,
                Err(e) => e.as_neg(),
            }
        }

        SYS_CLONE => {
            let flags = a1;
            if flags & CLONE_THREAD != 0 {
                // Real CLONE_THREAD: a thread that SHARES the caller's address
                // space (CLONE_VM), resumes at the parent's clone-return RIP
                // (regs.rcx) with RSP=child_stack and RAX=0 (thread_entry_user
                // zeros GPRs). x86_64 clone: a1=flags a2=child_stack a3=parent_tid
                // a4=child_tid a5=tls. The shared PML4 is refcounted (commit
                // 773e25e) so the parent-reap/pml4-free iron DF can't recur.
                let child_stack = a2;
                let parent = crate::scheduler::with_current_task(|t| (t.pml4, t.fs_base, t.id));
                match parent {
                    Some((Some(parent_pml4), parent_fs, parent_id)) if child_stack != 0 => {
                        let tls = if flags & CLONE_SETTLS != 0 {
                            a5
                        } else {
                            parent_fs
                        };
                        let parent_pid = parent_id.raw();
                        match crate::task::Task::new_linux_thread(
                            regs.rcx,
                            child_stack,
                            tls,
                            parent_pml4,
                            Some(parent_id),
                        ) {
                            Ok(mut child) => {
                                // BSP-pinned (APs loop{hlt} post-boot) so it runs.
                                child.affinity = crate::task::CpuAffinity::from_mask(1);
                                // CLONE_CHILD_CLEARTID: remember the TID slot to
                                // zero + futex-wake when this thread exits — the
                                // word pthread_join blocks on (a4 = child_tid ptr).
                                if flags & CLONE_CHILD_CLEARTID != 0 && a4 != 0 {
                                    child.clear_child_tid = a4;
                                }
                                // CLONE_CHILD_SETTID: also write the TID into the
                                // child's TID slot now (pthread reads it).
                                if flags & CLONE_CHILD_SETTID != 0 && a4 != 0 {
                                    write_user_buf(a4, &(child.id.raw() as i32).to_le_bytes());
                                }
                                let tid = child.id;
                                mark_task_as_linux(tid.raw());
                                {
                                    let mut table = crate::posix::POSIX_STATE.lock();
                                    table.insert(
                                        tid.raw(),
                                        crate::posix::PosixProcessState::new(tid.raw(), parent_pid),
                                    );
                                }
                                crate::posix::install_console_fds(&mut child);
                                // CLONE_PARENT_SETTID: hand the child TID back to
                                // the parent's a3 (pthread reads it).
                                if flags & CLONE_PARENT_SETTID != 0 && a3 != 0 {
                                    write_user_buf(a3, &(tid.raw() as i32).to_le_bytes());
                                }
                                crate::scheduler::spawn(child);
                                tid.raw() as i64
                            }
                            Err(_) => Errno::Enomem.as_neg(),
                        }
                    }
                    _ => Errno::Einval.as_neg(),
                }
            } else {
                let pid = posix::sys_getpid();
                match posix::sys_fork(pid) {
                    Ok(child_pid) => child_pid as i64,
                    Err(e) => e.as_neg(),
                }
            }
        }

        SYS_EXECVE => {
            if let Some(path) = read_user_string(a1, 4096) {
                match posix::sys_execve(&path, &[], &[]) {
                    Ok(()) => 0,
                    Err(e) => e.as_neg(),
                }
            } else {
                Errno::Efault.as_neg()
            }
        }

        SYS_EXIT => {
            clear_child_tid_on_exit();
            posix::sys_exit(a1);
            0
        }

        SYS_EXIT_GROUP => {
            clear_child_tid_on_exit();
            posix::sys_exit(a1);
            0
        }

        SYS_WAIT4 => {
            let mut status = 0i32;
            match posix::sys_waitpid(a1 as i64, &mut status, a3 as u32) {
                Ok(pid) => {
                    if a2 != 0 {
                        write_user_buf(a2, &status.to_le_bytes());
                    }
                    pid as i64
                }
                Err(e) => e.as_neg(),
            }
        }

        SYS_WAITID => {
            // waitid(idtype=a1, id=a2, infop=a3, options=a4, rusage=a5): wait via the
            // wait4 core, then translate the wait status into a siginfo_t (128 bytes).
            let pid: i64 = match a1 {
                1 => a2 as i64,    // P_PID
                2 => -(a2 as i64), // P_PGID -> negative pid (process group)
                _ => -1,           // P_ALL
            };
            let mut status = 0i32;
            match posix::sys_waitpid(pid, &mut status, a4 as u32) {
                Ok(rpid) => {
                    if a3 != 0 {
                        let mut si = [0u8; 128];
                        let (code, st): (i32, i32) = if (status & 0x7f) == 0 {
                            (1, (status >> 8) & 0xff) // CLD_EXITED
                        } else {
                            (2, status & 0x7f) // CLD_KILLED
                        };
                        si[0..4].copy_from_slice(&17i32.to_le_bytes()); // si_signo = SIGCHLD
                        si[8..12].copy_from_slice(&code.to_le_bytes()); // si_code
                        si[16..20].copy_from_slice(&(rpid as i32).to_le_bytes()); // si_pid
                        si[24..28].copy_from_slice(&st.to_le_bytes()); // si_status
                        write_user_buf(a3, &si);
                    }
                    0
                }
                Err(e) => e.as_neg(),
            }
        }

        SYS_GETPID => posix::sys_getpid() as i64,
        SYS_GETTID => posix::sys_getpid() as i64,
        SYS_GETPPID => {
            let pid = posix::sys_getpid();
            posix::sys_getppid(pid) as i64
        }

        SYS_GETUID | SYS_GETEUID => {
            let n = LOG_GETUID.fetch_add(1, Ordering::Relaxed);
            if n < 3 {
                crate::serial_println!("[linux_syscall] getuid");
            }
            posix::sys_getuid() as i64
        }
        SYS_GETGID | SYS_GETEGID => posix::sys_getgid() as i64,
        SYS_SETUID | SYS_SETGID => 0,
        SYS_GETPGRP => posix::sys_getpid() as i64,
        SYS_SETSID => posix::sys_getpid() as i64,

        SYS_GETGROUPS => {
            // getgroups(int size, gid_t *list)
            let size = a1 as usize;
            let gid = posix::sys_getgid() as u32;
            if size == 0 {
                // Linux returns the number of groups available.
                1
            } else if a2 == 0 {
                Errno::Efault.as_neg()
            } else {
                let bytes = gid.to_le_bytes();
                if write_user_buf(a2, &bytes) {
                    1
                } else {
                    Errno::Efault.as_neg()
                }
            }
        }

        // ── Signals ──────────────────────────────────────────────────
        SYS_KILL => match posix::sys_kill(a1 as i64, a2 as u8) {
            Ok(()) => {
                let target = a1 as i64;
                let self_pid = posix::sys_getpid() as i64;
                let signo = a2 as u32;
                if signo > 0 && signo <= 64 && (target == self_pid || target == 0 || target == -1) {
                    let bit = 1u64 << (signo - 1);
                    let sender_pid = self_pid as u32;
                    let sender_uid = posix::sys_getuid();
                    let mut table = SIGNALFD_TABLE.lock();
                    for sfd in table.values_mut() {
                        if (sfd.mask & bit) != 0 {
                            sfd.pending.push_back(LinuxSignalfdSiginfo {
                                ssi_signo: signo,
                                ssi_code: SI_USER,
                                ssi_pid: sender_pid,
                                ssi_uid: sender_uid,
                                ssi_tid: sender_pid,
                                ..LinuxSignalfdSiginfo::default()
                            });
                        }
                    }
                }
                0
            }
            Err(e) => e.as_neg(),
        },

        SYS_RT_SIGACTION => {
            let sig = a1 as u8;
            let sigset_size = a4 as usize;
            if (a2 != 0 || a3 != 0) && sigset_size < 8 {
                Errno::Einval.as_neg()
            } else {
                let pid = posix::sys_getpid();
                let old_raw = LINUX_RT_SIGACTION_TABLE
                    .lock()
                    .get(&(pid, sig))
                    .copied()
                    .unwrap_or(LinuxRtSigAction {
                        handler: SIG_DFL,
                        flags: 0,
                        restorer: 0,
                        mask: 0,
                    });
                let set_result = if a2 != 0 {
                    if let Some(new_raw) = read_user_rt_sigaction(a2) {
                        LINUX_RT_SIGACTION_TABLE.lock().insert((pid, sig), new_raw);
                        let mut new_native = crate::signals::SignalHandler::default();
                        new_native.handler = new_raw.handler;
                        new_native.flags = new_raw.flags as u32;
                        new_native.mask = crate::signals::SignalSet(new_raw.mask);
                        posix::sys_sigaction(sig, Some(&new_native), None)
                    } else {
                        Err(Errno::Efault)
                    }
                } else {
                    Ok(())
                };

                match set_result {
                    Ok(()) => {
                        if a3 != 0 && !write_user_rt_sigaction(a3, &old_raw) {
                            Errno::Efault.as_neg()
                        } else {
                            0
                        }
                    }
                    Err(e) => e.as_neg(),
                }
            }
        }

        SYS_RT_SIGPROCMASK => {
            let sigset_size = a4 as usize;
            if (a2 != 0 || a3 != 0) && sigset_size < 8 {
                Errno::Einval.as_neg()
            } else {
                let pid = posix::sys_getpid();
                let old_mask = LINUX_SIGMASK_TABLE.lock().get(&pid).copied().unwrap_or(0);
                if a3 != 0 && !write_user_u64(a3, old_mask) {
                    Errno::Efault.as_neg()
                } else {
                    if a2 != 0 {
                        if let Some(input_mask) = read_user_u64(a2) {
                            let new_mask_opt = match a1 as u32 {
                                posix::SIG_BLOCK => Some(old_mask | input_mask),
                                posix::SIG_UNBLOCK => Some(old_mask & !input_mask),
                                posix::SIG_SETMASK => Some(input_mask),
                                _ => None,
                            };
                            if let Some(new_mask) = new_mask_opt {
                                LINUX_SIGMASK_TABLE.lock().insert(pid, new_mask);
                                let set = crate::signals::SignalSet(new_mask);
                                let _ =
                                    posix::sys_sigprocmask(posix::SIG_SETMASK, Some(&set), None);
                                0
                            } else {
                                Errno::Einval.as_neg()
                            }
                        } else {
                            Errno::Efault.as_neg()
                        }
                    } else {
                        0
                    }
                }
            }
        }

        SYS_RT_SIGRETURN => {
            let pid = posix::sys_getpid();
            let mut stacks = RT_SIGFRAME_STACKS.lock();
            if let Some(stack) = stacks.get_mut(&pid) {
                if let Some(frame) = stack.pop() {
                    set_linux_sigmask(pid, frame.old_mask);
                    frame.saved.restore_into(regs);
                    regs.rax as i64
                } else {
                    Errno::Einval.as_neg()
                }
            } else {
                Errno::Einval.as_neg()
            }
        }

        SYS_SIGALTSTACK => 0,

        // ── Time ─────────────────────────────────────────────────────
        SYS_GETTIMEOFDAY => {
            let mut tv = posix::Timeval::default();
            match posix::sys_gettimeofday(&mut tv, None) {
                Ok(()) => {
                    let bytes = unsafe {
                        core::slice::from_raw_parts(
                            &tv as *const _ as *const u8,
                            core::mem::size_of::<posix::Timeval>(),
                        )
                    };
                    if write_user_buf(a1, bytes) {
                        0
                    } else {
                        Errno::Efault.as_neg()
                    }
                }
                Err(e) => e.as_neg(),
            }
        }

        SYS_CLOCK_GETTIME => {
            let mut ts = posix::Timespec::default();
            match posix::sys_clock_gettime(a1 as u32, &mut ts) {
                Ok(()) => {
                    let bytes = unsafe {
                        core::slice::from_raw_parts(
                            &ts as *const _ as *const u8,
                            core::mem::size_of::<posix::Timespec>(),
                        )
                    };
                    if write_user_buf(a2, bytes) {
                        0
                    } else {
                        Errno::Efault.as_neg()
                    }
                }
                Err(e) => e.as_neg(),
            }
        }

        SYS_CLOCK_GETRES => {
            let ts = posix::Timespec {
                tv_sec: 0,
                tv_nsec: 1,
            };
            let bytes = unsafe {
                core::slice::from_raw_parts(
                    &ts as *const _ as *const u8,
                    core::mem::size_of::<posix::Timespec>(),
                )
            };
            if write_user_buf(a2, bytes) {
                0
            } else {
                Errno::Efault.as_neg()
            }
        }

        SYS_NANOSLEEP | SYS_CLOCK_NANOSLEEP => {
            // nanosleep(req, rem), clock_nanosleep(clock, flags, req, rem)
            let req_ptr = if nr == SYS_NANOSLEEP { a1 } else { a3 };
            let rem_ptr = if nr == SYS_NANOSLEEP { a2 } else { a4 };
            match read_user_timespec(req_ptr) {
                Some(req) => {
                    let mut rem = posix::Timespec::default();
                    let rem_opt = if rem_ptr != 0 { Some(&mut rem) } else { None };
                    match posix::sys_nanosleep(&req, rem_opt) {
                        Ok(()) => 0,
                        Err(e) => {
                            if rem_ptr != 0 {
                                let bytes = unsafe {
                                    core::slice::from_raw_parts(
                                        &rem as *const _ as *const u8,
                                        core::mem::size_of::<posix::Timespec>(),
                                    )
                                };
                                let _ = write_user_buf(rem_ptr, bytes);
                            }
                            e.as_neg()
                        }
                    }
                }
                None => Errno::Efault.as_neg(),
            }
        }

        // ── Directory ────────────────────────────────────────────────
        SYS_GETCWD => {
            let pid = posix::sys_getpid();
            match posix::sys_getcwd(pid) {
                Ok(cwd) => {
                    let bytes = cwd.as_bytes();
                    let n = bytes.len().min(a2 as usize - 1);
                    let mut out = Vec::from(&bytes[..n]);
                    out.push(0);
                    if write_user_buf(a1, &out) {
                        a1 as i64
                    } else {
                        Errno::Efault.as_neg()
                    }
                }
                Err(e) => e.as_neg(),
            }
        }

        SYS_CHDIR => {
            if let Some(path) = read_user_string(a1, 4096) {
                let pid = posix::sys_getpid();
                match posix::sys_chdir(pid, &path) {
                    Ok(()) => 0,
                    Err(e) => e.as_neg(),
                }
            } else {
                Errno::Efault.as_neg()
            }
        }

        SYS_MKDIR => {
            if let Some(path) = read_user_string(a1, 4096) {
                match crate::vfs::mkdir_at(&path, a2 as u32) {
                    Ok(()) => 0,
                    Err(code) => vfs_err_to_errno(code).as_neg(),
                }
            } else {
                Errno::Efault.as_neg()
            }
        }
        SYS_MKDIRAT => {
            if let Some(path) = read_user_string(a2, 4096) {
                // For now support AT_FDCWD and absolute paths.
                if (a1 as i32) != AT_FDCWD && !path.starts_with('/') {
                    Errno::Enosys.as_neg()
                } else {
                    match crate::vfs::mkdir_at(&path, a3 as u32) {
                        Ok(()) => 0,
                        Err(code) => vfs_err_to_errno(code).as_neg(),
                    }
                }
            } else {
                Errno::Efault.as_neg()
            }
        }

        SYS_RMDIR => {
            if let Some(path) = read_user_string(a1, 4096) {
                match crate::vfs::unlink_at(&path) {
                    Ok(()) => 0,
                    Err(code) => vfs_err_to_errno(code).as_neg(),
                }
            } else {
                Errno::Efault.as_neg()
            }
        }

        SYS_UNLINK => {
            if let Some(path) = read_user_string(a1, 4096) {
                match crate::vfs::unlink_at(&path) {
                    Ok(()) => 0,
                    Err(code) => vfs_err_to_errno(code).as_neg(),
                }
            } else {
                Errno::Efault.as_neg()
            }
        }
        SYS_UNLINKAT => {
            if let Some(path) = read_user_string(a2, 4096) {
                if (a1 as i32) != AT_FDCWD && !path.starts_with('/') {
                    Errno::Enosys.as_neg()
                } else if (a3 & AT_REMOVEDIR) != 0 {
                    match crate::vfs::unlink_at(&path) {
                        Ok(()) => 0,
                        Err(code) => vfs_err_to_errno(code).as_neg(),
                    }
                } else {
                    match crate::vfs::unlink_at(&path) {
                        Ok(()) => 0,
                        Err(code) => vfs_err_to_errno(code).as_neg(),
                    }
                }
            } else {
                Errno::Efault.as_neg()
            }
        }

        // ── Sockets ──────────────────────────────────────────────────
        SYS_SOCKET => match posix::sys_socket(a1 as u32, a2 as u32, a3 as u32) {
            Ok(fd) => fd as i64,
            Err(e) => e.as_neg(),
        },

        SYS_BIND => {
            let addr = posix::SockaddrIn::default();
            match posix::sys_bind(a1 as u32, &addr) {
                Ok(()) => 0,
                Err(e) => e.as_neg(),
            }
        }

        SYS_LISTEN => match posix::sys_listen(a1 as u32, a2 as u32) {
            Ok(()) => 0,
            Err(e) => e.as_neg(),
        },

        SYS_ACCEPT => match posix::sys_accept(a1 as u32, None) {
            Ok(fd) => fd as i64,
            Err(e) => e.as_neg(),
        },

        SYS_CONNECT => {
            let addr = posix::SockaddrIn::default();
            match posix::sys_connect(a1 as u32, &addr) {
                Ok(()) => 0,
                Err(e) => e.as_neg(),
            }
        }

        SYS_SENDTO => {
            if let Some(buf) = read_user_buf(a2, a3 as usize) {
                match posix::sys_send(a1 as u32, &buf, a4 as u32) {
                    Ok(n) => n as i64,
                    Err(e) => e.as_neg(),
                }
            } else {
                Errno::Efault.as_neg()
            }
        }

        SYS_SENDMSG => {
            // sendmsg(fd=a1, msghdr=a2, flags=a3): gather the iovecs into one buffer
            // and send. msghdr: msg_iov @16, msg_iovlen @24; iovec: base @0, len @8.
            match read_user_buf(a2, 56) {
                Some(hdr) => {
                    let iov_ptr = u64::from_le_bytes(hdr[16..24].try_into().unwrap_or([0; 8]));
                    let iov_len =
                        u64::from_le_bytes(hdr[24..32].try_into().unwrap_or([0; 8])) as usize;
                    let mut data: Vec<u8> = Vec::new();
                    for i in 0..iov_len.min(1024) {
                        if let Some(iov) = read_user_buf(iov_ptr + (i as u64) * 16, 16) {
                            let base = u64::from_le_bytes(iov[0..8].try_into().unwrap_or([0; 8]));
                            let len = u64::from_le_bytes(iov[8..16].try_into().unwrap_or([0; 8]))
                                as usize;
                            if len > 0 {
                                if let Some(chunk) = read_user_buf(base, len.min(1 << 20)) {
                                    data.extend_from_slice(&chunk);
                                }
                            }
                        }
                    }
                    match posix::sys_send(a1 as u32, &data, a3 as u32) {
                        Ok(n) => n as i64,
                        Err(e) => e.as_neg(),
                    }
                }
                None => Errno::Efault.as_neg(),
            }
        }

        SYS_RECVMSG => {
            // recvmsg(fd=a1, msghdr=a2, flags=a3): recv into a buffer, scatter across
            // the iovecs in order.
            match read_user_buf(a2, 56) {
                Some(hdr) => {
                    let iov_ptr = u64::from_le_bytes(hdr[16..24].try_into().unwrap_or([0; 8]));
                    let iov_len =
                        u64::from_le_bytes(hdr[24..32].try_into().unwrap_or([0; 8])) as usize;
                    let mut iovs: Vec<(u64, usize)> = Vec::new();
                    let mut total = 0usize;
                    for i in 0..iov_len.min(1024) {
                        if let Some(iov) = read_user_buf(iov_ptr + (i as u64) * 16, 16) {
                            let base = u64::from_le_bytes(iov[0..8].try_into().unwrap_or([0; 8]));
                            let len = (u64::from_le_bytes(iov[8..16].try_into().unwrap_or([0; 8]))
                                as usize)
                                .min(1 << 20);
                            iovs.push((base, len));
                            total = total.saturating_add(len);
                        }
                    }
                    let mut buf = alloc::vec![0u8; total.min(1 << 20)];
                    match posix::sys_recv(a1 as u32, &mut buf, a3 as u32) {
                        Ok(n) => {
                            let mut off = 0usize;
                            for (base, len) in iovs {
                                if off >= n {
                                    break;
                                }
                                let take = len.min(n - off);
                                write_user_buf(base, &buf[off..off + take]);
                                off += take;
                            }
                            n as i64
                        }
                        Err(e) => e.as_neg(),
                    }
                }
                None => Errno::Efault.as_neg(),
            }
        }

        SYS_RECVFROM => {
            if let Some(mut buf) = read_user_buf(a2, a3 as usize) {
                match posix::sys_recv(a1 as u32, &mut buf, a4 as u32) {
                    Ok(n) => {
                        write_user_buf(a2, &buf[..n]);
                        n as i64
                    }
                    Err(e) => e.as_neg(),
                }
            } else {
                Errno::Efault.as_neg()
            }
        }

        // pselect6(nfds,r,w,e,timeout_ts,sigmask) shares select's fd processing.
        SYS_SELECT | SYS_PSELECT6 => {
            let nfds = (a1 as usize).min(1024);
            let words = nfds.div_ceil(64);
            let bytes = words * core::mem::size_of::<LinuxFdSetWord>();
            let mut read_set = if a2 != 0 {
                read_user_buf(a2, bytes)
            } else {
                Some(Vec::new())
            };
            let mut write_set = if a3 != 0 {
                read_user_buf(a3, bytes)
            } else {
                Some(Vec::new())
            };
            let mut except_set = if a4 != 0 {
                read_user_buf(a4, bytes)
            } else {
                Some(Vec::new())
            };
            if read_set.is_none() || write_set.is_none() || except_set.is_none() {
                Errno::Efault.as_neg()
            } else {
                let mut ready = 0i64;
                let mut seen = [false; 1024];
                let mut process_set = |set: &mut Vec<u8>, check: fn(i32) -> bool| {
                    for fd in 0..nfds {
                        let wi = fd / 8;
                        let bi = fd % 8;
                        if wi >= set.len() {
                            break;
                        }
                        let bit = 1u8 << bi;
                        if (set[wi] & bit) == 0 {
                            continue;
                        }
                        if check(fd as i32) {
                            if !seen[fd] {
                                seen[fd] = true;
                                ready += 1;
                            }
                        } else {
                            set[wi] &= !bit;
                        }
                    }
                };
                if let Some(ref mut s) = read_set {
                    process_set(s, fd_read_ready);
                }
                if let Some(ref mut s) = write_set {
                    process_set(s, fd_write_ready);
                }
                if let Some(ref mut s) = except_set {
                    process_set(s, |_| false);
                }
                let mut ok = true;
                if let Some(ref s) = read_set {
                    ok &= a2 == 0 || write_user_buf(a2, s);
                }
                if let Some(ref s) = write_set {
                    ok &= a3 == 0 || write_user_buf(a3, s);
                }
                if let Some(ref s) = except_set {
                    ok &= a4 == 0 || write_user_buf(a4, s);
                }
                if ok {
                    ready
                } else {
                    Errno::Efault.as_neg()
                }
            }
        }

        // ppoll(fds,nfds,timeout_ts,sigmask,..) shares poll's fd processing — poll is
        // non-blocking and ignores the timeout; the atomic-sigmask swap is the only
        // semantic we approximate (rarely load-bearing).
        SYS_POLL | SYS_PPOLL => {
            let nfds = (a2 as usize).min(1024);
            let size = core::mem::size_of::<posix::PollFd>();
            let total = nfds.saturating_mul(size);
            if a1 == 0 && nfds > 0 {
                Errno::Efault.as_neg()
            } else if let Some(raw) = read_user_buf(a1, total) {
                let mut fds = Vec::with_capacity(nfds);
                for i in 0..nfds {
                    let mut p = posix::PollFd {
                        fd: 0,
                        events: 0,
                        revents: 0,
                    };
                    unsafe {
                        core::ptr::copy_nonoverlapping(
                            raw.as_ptr().add(i * size),
                            &mut p as *mut _ as *mut u8,
                            size,
                        );
                    }
                    if let Some(id) = EVENTFD_FD_TO_ID.lock().get(&p.fd).copied() {
                        let mut rev = 0u16;
                        if let Some(ev) = EVENTFD_TABLE.lock().get(&id) {
                            if ev.counter.load(Ordering::Relaxed) > 0 {
                                rev |= posix::POLLIN;
                            }
                            rev |= posix::POLLOUT;
                        }
                        p.revents = p.events & rev;
                    } else if let Some(id) = TIMERFD_FD_TO_ID.lock().get(&p.fd).copied() {
                        let mut rev = 0u16;
                        if timerfd_readable(id) {
                            rev |= posix::POLLIN;
                        }
                        p.revents = p.events & rev;
                    }
                    fds.push(p);
                }
                match posix::sys_poll(&mut fds, a3 as i32) {
                    Ok(base_ready) => {
                        let mut out: Vec<u8> = Vec::with_capacity(total);
                        unsafe {
                            out.set_len(total);
                        }
                        for (i, p) in fds.iter().enumerate() {
                            unsafe {
                                core::ptr::copy_nonoverlapping(
                                    p as *const _ as *const u8,
                                    out.as_mut_ptr().add(i * size),
                                    size,
                                );
                            }
                        }
                        if write_user_buf(a1, &out) {
                            base_ready as i64
                        } else {
                            Errno::Efault.as_neg()
                        }
                    }
                    Err(e) => e.as_neg(),
                }
            } else {
                Errno::Efault.as_neg()
            }
        }

        // ── Linux-specific: uname ────────────────────────────────────
        SYS_UNAME => {
            let n = LOG_UNAME.fetch_add(1, Ordering::Relaxed);
            if n < 3 {
                crate::serial_println!("[linux_syscall] uname");
            }
            let uname = Utsname::athenaos_default();
            let bytes = unsafe {
                core::slice::from_raw_parts(
                    &uname as *const _ as *const u8,
                    core::mem::size_of::<Utsname>(),
                )
            };
            if write_user_buf(a1, bytes) {
                0
            } else {
                Errno::Efault.as_neg()
            }
        }

        // ── Linux-specific: clone, futex, epoll, eventfd, timerfd ────
        SYS_FUTEX => {
            let op = (a2 as u32) & 0x7F;
            match op {
                FUTEX_WAIT => match futex_wait(a1, a3 as u32, a4) {
                    Ok(()) => 0,
                    Err(e) => e.as_neg(),
                },
                FUTEX_WAKE => match futex_wake(a1, a3 as u32) {
                    Ok(n) => n as i64,
                    Err(e) => e.as_neg(),
                },
                _ => 0,
            }
        }

        SYS_EPOLL_CREATE | SYS_EPOLL_CREATE1 => {
            let id = NEXT_EPOLL_ID.fetch_add(1, Ordering::Relaxed);
            EPOLL_TABLE.lock().insert(id, EpollInstance::new());
            match alloc_anon_fd() {
                Ok(fd) => {
                    EPOLL_FD_TO_ID.lock().insert(fd, id);
                    fd as i64
                }
                Err(e) => e.as_neg(),
            }
        }

        SYS_EPOLL_CTL => {
            let epfd = a1 as i32;
            let op = a2 as u32;
            let fd = a3 as i32;
            if let Some(ep_id) = EPOLL_FD_TO_ID.lock().get(&epfd).copied() {
                let mut table = EPOLL_TABLE.lock();
                if let Some(inst) = table.get_mut(&ep_id) {
                    match op {
                        EPOLL_CTL_ADD => {
                            if inst.fds.contains_key(&fd) {
                                Errno::Eexist.as_neg()
                            } else if a4 == 0 {
                                Errno::Efault.as_neg()
                            } else if let Some(raw) =
                                read_user_buf(a4, core::mem::size_of::<EpollEvent>())
                            {
                                let mut e = EpollEvent {
                                    events: EPOLLIN,
                                    data: fd as u64,
                                };
                                unsafe {
                                    core::ptr::copy_nonoverlapping(
                                        raw.as_ptr(),
                                        &mut e as *mut _ as *mut u8,
                                        core::mem::size_of::<EpollEvent>(),
                                    );
                                }
                                inst.fds.insert(fd, e);
                                0
                            } else {
                                Errno::Efault.as_neg()
                            }
                        }
                        EPOLL_CTL_MOD => {
                            if !inst.fds.contains_key(&fd) {
                                Errno::Enoent.as_neg()
                            } else if a4 == 0 {
                                Errno::Efault.as_neg()
                            } else if let Some(raw) =
                                read_user_buf(a4, core::mem::size_of::<EpollEvent>())
                            {
                                let mut e = EpollEvent {
                                    events: EPOLLIN,
                                    data: fd as u64,
                                };
                                unsafe {
                                    core::ptr::copy_nonoverlapping(
                                        raw.as_ptr(),
                                        &mut e as *mut _ as *mut u8,
                                        core::mem::size_of::<EpollEvent>(),
                                    );
                                }
                                inst.fds.insert(fd, e);
                                0
                            } else {
                                Errno::Efault.as_neg()
                            }
                        }
                        EPOLL_CTL_DEL => {
                            inst.fds.remove(&fd);
                            0
                        }
                        _ => Errno::Einval.as_neg(),
                    }
                } else {
                    Errno::Ebadf.as_neg()
                }
            } else {
                Errno::Ebadf.as_neg()
            }
        }

        // Extended attributes: AthFS/the VFS store none. LIST -> 0 bytes (empty
        // set), GET -> ENODATA (no such attribute), SET -> 0 (accepted, no-op).
        // This is the behaviour a no-xattr filesystem presents; a stock `ls -l`
        // calls llistxattr per entry and treats 0 as "no '+' indicator".
        SYS_LISTXATTR | SYS_LLISTXATTR | SYS_FLISTXATTR => 0,
        SYS_GETXATTR | SYS_LGETXATTR | SYS_FGETXATTR => Errno::Enodata.as_neg(),
        SYS_SETXATTR | SYS_LSETXATTR | SYS_FSETXATTR => 0,

        // epoll_pwait / epoll_pwait2 share the first three args (epfd, events,
        // maxevents) with epoll_wait; the trailing timeout/sigmask are ignored
        // by this non-blocking ready-scan poll. A stock `ls` uses epoll_pwait2.
        SYS_EPOLL_WAIT | SYS_EPOLL_PWAIT | SYS_EPOLL_PWAIT2 => {
            let epfd = a1 as i32;
            let events_ptr = a2;
            let maxevents = (a3 as usize).min(256);
            if maxevents == 0 || events_ptr == 0 {
                Errno::Einval.as_neg()
            } else {
                if let Some(ep_id) = EPOLL_FD_TO_ID.lock().get(&epfd).copied() {
                    let table = EPOLL_TABLE.lock();
                    if let Some(inst) = table.get(&ep_id) {
                        let mut out = Vec::new();
                        for (&fd, ev) in inst.fds.iter() {
                            if out.len() >= maxevents {
                                break;
                            }
                            let mut revents = 0u32;
                            if (ev.events & EPOLLIN) != 0 && fd_read_ready(fd) {
                                revents |= EPOLLIN;
                            }
                            if (ev.events & EPOLLOUT) != 0 && fd_write_ready(fd) {
                                revents |= EPOLLOUT;
                            }
                            if revents != 0 {
                                out.push(EpollEvent {
                                    events: revents,
                                    data: ev.data,
                                });
                            }
                        }
                        let bytes = unsafe {
                            core::slice::from_raw_parts(
                                out.as_ptr() as *const u8,
                                out.len() * core::mem::size_of::<EpollEvent>(),
                            )
                        };
                        if write_user_buf(events_ptr, bytes) {
                            out.len() as i64
                        } else {
                            Errno::Efault.as_neg()
                        }
                    } else {
                        Errno::Ebadf.as_neg()
                    }
                } else {
                    Errno::Ebadf.as_neg()
                }
            }
        }

        SYS_EVENTFD | SYS_EVENTFD2 => {
            let id = NEXT_EVENTFD_ID.fetch_add(1, Ordering::Relaxed);
            EVENTFD_TABLE.lock().insert(
                id,
                EventFd {
                    counter: AtomicU64::new(a1),
                    flags: a2 as u32,
                },
            );
            match alloc_anon_fd() {
                Ok(fd) => {
                    EVENTFD_FD_TO_ID.lock().insert(fd, id);
                    fd as i64
                }
                Err(e) => e.as_neg(),
            }
        }

        SYS_TIMERFD_CREATE => {
            let id = NEXT_TIMERFD_ID.fetch_add(1, Ordering::Relaxed);
            TIMERFD_TABLE.lock().insert(
                id,
                TimerFd {
                    clock_id: a1 as u32,
                    armed: AtomicBool::new(false),
                    expiry_ns: AtomicU64::new(0),
                    interval_ns: AtomicU64::new(0),
                },
            );
            match alloc_anon_fd() {
                Ok(fd) => {
                    TIMERFD_FD_TO_ID.lock().insert(fd, id);
                    fd as i64
                }
                Err(e) => e.as_neg(),
            }
        }

        SYS_TIMERFD_SETTIME => {
            let fd = a1 as i32;
            let new_ptr = a3;
            let old_ptr = a4;
            if let Some(id) = TIMERFD_FD_TO_ID.lock().get(&fd).copied() {
                if new_ptr == 0 {
                    Errno::Efault.as_neg()
                } else if let Some(new_spec) = read_user_itimerspec(new_ptr) {
                    let mut table = TIMERFD_TABLE.lock();
                    if let Some(t) = table.get_mut(&id) {
                        let mut old_write_ok = true;
                        if old_ptr != 0 {
                            let now = now_ns(t.clock_id);
                            let expiry = t.expiry_ns.load(Ordering::Relaxed);
                            let rem_ns = if t.armed.load(Ordering::Relaxed) && expiry > now {
                                expiry - now
                            } else {
                                0
                            };
                            let old_spec = LinuxItimerspec {
                                interval: ns_to_timespec(t.interval_ns.load(Ordering::Relaxed)),
                                value: ns_to_timespec(rem_ns),
                            };
                            old_write_ok = write_user_itimerspec(old_ptr, &old_spec);
                        }
                        if !old_write_ok {
                            Errno::Efault.as_neg()
                        } else {
                            let val_ns = timespec_to_ns(&new_spec.value);
                            let int_ns = timespec_to_ns(&new_spec.interval);
                            if val_ns == 0 {
                                t.armed.store(false, Ordering::Relaxed);
                                t.expiry_ns.store(0, Ordering::Relaxed);
                                t.interval_ns.store(int_ns, Ordering::Relaxed);
                            } else {
                                let now = now_ns(t.clock_id);
                                t.armed.store(true, Ordering::Relaxed);
                                t.expiry_ns
                                    .store(now.saturating_add(val_ns), Ordering::Relaxed);
                                t.interval_ns.store(int_ns, Ordering::Relaxed);
                            }
                            0
                        }
                    } else {
                        Errno::Ebadf.as_neg()
                    }
                } else {
                    Errno::Efault.as_neg()
                }
            } else {
                Errno::Ebadf.as_neg()
            }
        }
        SYS_TIMERFD_GETTIME => {
            let fd = a1 as i32;
            let out_ptr = a2;
            if let Some(id) = TIMERFD_FD_TO_ID.lock().get(&fd).copied() {
                if out_ptr == 0 {
                    Errno::Efault.as_neg()
                } else {
                    let table = TIMERFD_TABLE.lock();
                    if let Some(t) = table.get(&id) {
                        let now = now_ns(t.clock_id);
                        let expiry = t.expiry_ns.load(Ordering::Relaxed);
                        let rem_ns = if t.armed.load(Ordering::Relaxed) && expiry > now {
                            expiry - now
                        } else {
                            0
                        };
                        let spec = LinuxItimerspec {
                            interval: ns_to_timespec(t.interval_ns.load(Ordering::Relaxed)),
                            value: ns_to_timespec(rem_ns),
                        };
                        if write_user_itimerspec(out_ptr, &spec) {
                            0
                        } else {
                            Errno::Efault.as_neg()
                        }
                    } else {
                        Errno::Ebadf.as_neg()
                    }
                }
            } else {
                Errno::Ebadf.as_neg()
            }
        }

        SYS_SIGNALFD | SYS_SIGNALFD4 => {
            let existing_fd = a1 as i32;
            let mask_ptr = a2;
            let sigset_size = a3 as usize;
            let flags = if nr == SYS_SIGNALFD4 { a4 as u32 } else { 0 };
            if mask_ptr == 0 || sigset_size < 8 {
                Errno::Einval.as_neg()
            } else if let Some(mask) = read_user_u64(mask_ptr) {
                if existing_fd >= 0 {
                    if let Some(id) = SIGNALFD_FD_TO_ID.lock().get(&existing_fd).copied() {
                        let mut table = SIGNALFD_TABLE.lock();
                        if let Some(sfd) = table.get_mut(&id) {
                            sfd.mask = mask;
                            sfd.flags = flags;
                            existing_fd as i64
                        } else {
                            Errno::Ebadf.as_neg()
                        }
                    } else {
                        Errno::Ebadf.as_neg()
                    }
                } else {
                    let id = NEXT_SIGNALFD_ID.fetch_add(1, Ordering::Relaxed);
                    SIGNALFD_TABLE.lock().insert(
                        id,
                        SignalFd {
                            mask,
                            flags,
                            pending: VecDeque::new(),
                        },
                    );
                    match alloc_anon_fd() {
                        Ok(fd) => {
                            SIGNALFD_FD_TO_ID.lock().insert(fd, id);
                            fd as i64
                        }
                        Err(e) => e.as_neg(),
                    }
                }
            } else {
                Errno::Efault.as_neg()
            }
        }

        // ── prctl / arch_prctl ───────────────────────────────────────
        SYS_PRCTL => handle_prctl(a1, a2, a3, a4, a5),
        SYS_ARCH_PRCTL => handle_arch_prctl(a1, a2),

        // ── Misc stubs ───────────────────────────────────────────────
        SYS_SCHED_YIELD => {
            crate::scheduler::yield_task();
            0
        }
        SYS_SET_TID_ADDRESS => {
            // set_tid_address(tidptr): register the word to clear + futex-wake on
            // exit (glibc's main thread + every pthread call this). Returns the
            // caller's TID. Storing it makes a later join on this thread wake.
            crate::scheduler::with_current_task_mut(|t| t.clear_child_tid = a1);
            posix::sys_getpid() as i64
        }
        SYS_SET_ROBUST_LIST => {
            let head = a1;
            let len = a2;
            let pid = posix::sys_getpid();
            if head == 0 || len == 0 {
                ROBUST_LIST_HEADS.lock().remove(&pid);
                0
            } else {
                ROBUST_LIST_HEADS.lock().insert(pid, (head, len));
                0
            }
        }
        SYS_PRLIMIT64 => {
            // prlimit64(pid, resource, new_limit, old_limit). We don't enforce
            // limits, but we MUST fill `old_limit` (a4) when requested — glibc
            // reads RLIMIT_STACK here to size pthread thread stacks. Returning
            // success with an UNFILLED struct left garbage there, so
            // pthread_create computed a bogus stack size, its stack mmap failed,
            // and pthread_join then dereferenced an uninitialized pthread_t →
            // user #PF (the glibc-pthread bring-up bug). `struct rlimit64` is
            // { u64 rlim_cur; u64 rlim_max; }.
            const RLIM_INFINITY: u64 = u64::MAX;
            const RLIMIT_STACK: u64 = 3;
            const RLIMIT_NOFILE: u64 = 7;
            const RLIMIT_AS: u64 = 9;
            let old_limit = a4;
            if old_limit != 0 {
                let (cur, max) = match a2 {
                    RLIMIT_STACK => (0x80_0000u64, RLIM_INFINITY), // 8 MiB soft
                    RLIMIT_NOFILE => (1024, 4096),
                    RLIMIT_AS => (RLIM_INFINITY, RLIM_INFINITY),
                    _ => (RLIM_INFINITY, RLIM_INFINITY),
                };
                let mut buf = [0u8; 16];
                buf[0..8].copy_from_slice(&cur.to_le_bytes());
                buf[8..16].copy_from_slice(&max.to_le_bytes());
                write_user_buf(old_limit, &buf);
            }
            0
        }
        SYS_RSEQ => Errno::Enosys.as_neg(),
        SYS_FTRUNCATE => 0,

        SYS_GETRANDOM => {
            let inode = posix::DevUrandomInode;
            let len = a2 as usize;
            let mut buf = alloc::vec![0u8; len];
            let n = crate::vfs::Inode::read_at(&inode, 0, &mut buf);
            if write_user_buf(a1, &buf[..n]) {
                n as i64
            } else {
                Errno::Efault.as_neg()
            }
        }

        SYS_PREAD64 => {
            if let Some(mut buf) = read_user_buf(a2, a3 as usize) {
                let file_arc = crate::scheduler::with_current_task(|task| {
                    let fd = a1 as usize;
                    if fd < task.fds.len() {
                        task.fds[fd].clone()
                    } else {
                        None
                    }
                })
                .flatten();

                if let Some(f) = file_arc {
                    let file = f.lock();
                    let n = file.inode.read_at(a4 as usize, &mut buf);
                    write_user_buf(a2, &buf[..n]);
                    n as i64
                } else {
                    Errno::Ebadf.as_neg()
                }
            } else {
                Errno::Efault.as_neg()
            }
        }

        SYS_PWRITE64 => {
            if let Some(buf) = read_user_buf(a2, a3 as usize) {
                let file_arc = crate::scheduler::with_current_task(|task| {
                    let fd = a1 as usize;
                    if fd < task.fds.len() {
                        task.fds[fd].clone()
                    } else {
                        None
                    }
                })
                .flatten();

                if let Some(f) = file_arc {
                    let file = f.lock();
                    let n = file.inode.write_at(a4 as usize, &buf);
                    n as i64
                } else {
                    Errno::Ebadf.as_neg()
                }
            } else {
                Errno::Efault.as_neg()
            }
        }

        SYS_READV => {
            let cnt = a3 as usize;
            if cnt == 0 {
                0
            } else if cnt > 1024 {
                Errno::Einval.as_neg()
            } else {
                match read_user_iovecs(a2, cnt) {
                    Some(iovs) => {
                        let mut total = 0usize;
                        let mut first_err: Option<i64> = None;
                        for iov in iovs {
                            if iov.len == 0 {
                                continue;
                            }
                            let mut buf = alloc::vec![0u8; iov.len as usize];
                            match posix::sys_read(a1 as u32, &mut buf) {
                                Ok(n) => {
                                    if !write_user_buf(iov.base, &buf[..n]) {
                                        first_err = Some(Errno::Efault.as_neg());
                                        break;
                                    }
                                    total = total.saturating_add(n);
                                    if n < iov.len as usize {
                                        break;
                                    }
                                }
                                Err(e) => {
                                    if total == 0 {
                                        first_err = Some(e.as_neg());
                                    }
                                    break;
                                }
                            }
                        }
                        if total == 0 {
                            first_err.unwrap_or(0)
                        } else {
                            total as i64
                        }
                    }
                    None => Errno::Efault.as_neg(),
                }
            }
        }
        SYS_WRITEV => {
            let cnt = a3 as usize;
            if cnt == 0 {
                0
            } else if cnt > 1024 {
                Errno::Einval.as_neg()
            } else {
                match read_user_iovecs(a2, cnt) {
                    Some(iovs) => {
                        let mut total = 0usize;
                        let mut first_err: Option<i64> = None;
                        for iov in iovs {
                            if iov.len == 0 {
                                continue;
                            }
                            let buf = match read_user_buf(iov.base, iov.len as usize) {
                                Some(b) => b,
                                None => {
                                    first_err = Some(Errno::Efault.as_neg());
                                    break;
                                }
                            };
                            match posix::sys_write(a1 as u32, &buf) {
                                Ok(n) => {
                                    total = total.saturating_add(n);
                                    if n < iov.len as usize {
                                        break;
                                    }
                                }
                                Err(e) => {
                                    if total == 0 {
                                        first_err = Some(e.as_neg());
                                    }
                                    break;
                                }
                            }
                        }
                        if total == 0 {
                            first_err.unwrap_or(0)
                        } else {
                            total as i64
                        }
                    }
                    None => Errno::Efault.as_neg(),
                }
            }
        }

        // ── Unknown syscall ──────────────────────────────────────────
        _ => {
            UNHANDLED_DISPATCHED.fetch_add(1, Ordering::Relaxed);
            crate::serial_println!("[linux-compat] unhandled syscall nr={} (0x{:x})", nr, nr,);
            Errno::Enosys.as_neg()
        }
    };

    regs.rax = result as u64;
    maybe_deliver_signal_handler(regs, nr);
}

// ═══════════════════════════════════════════════════════════════════════════════
// Detection: is this process a Linux ELF?
// ═══════════════════════════════════════════════════════════════════════════════

static LINUX_TASKS: Mutex<BTreeMap<u64, bool>> = Mutex::new(BTreeMap::new());

static LOG_UNAME: AtomicU64 = AtomicU64::new(0);
static LOG_GETUID: AtomicU64 = AtomicU64::new(0);
static TOTAL_DISPATCHED: AtomicU64 = AtomicU64::new(0);
static UNHANDLED_DISPATCHED: AtomicU64 = AtomicU64::new(0);
static LAST_NR: AtomicU64 = AtomicU64::new(u64::MAX);

pub fn mark_task_as_linux(task_id: u64) {
    LINUX_TASKS.lock().insert(task_id, true);
}

pub fn is_linux_task(task_id: u64) -> bool {
    LINUX_TASKS.lock().get(&task_id).copied().unwrap_or(false)
}

pub fn unmark_linux_task(task_id: u64) {
    LINUX_TASKS.lock().remove(&task_id);
}

// ═══════════════════════════════════════════════════════════════════════════════
// Initialization
// ═══════════════════════════════════════════════════════════════════════════════

pub fn init() {
    crate::serial_println!("[ OK ] Linux syscall translation layer initialized");
}

pub fn run_boot_smoketest() {
    let tracked = LINUX_TASKS.lock().len();

    // Prove the user-pointer accessors fail closed on a bad pointer instead of
    // faulting the kernel. A canonical user-half address that is virtually
    // certain to be unmapped at boot must yield EFAULT (None/false) via the
    // extable fault-fixup path — NOT a kernel page fault. This is the FAIL-able
    // half: if the fixup machinery regressed, reading it would crash the boot
    // (a hard, visible failure) rather than silently passing.
    const UNMAPPED_USER: u64 = 0x6FFF_FFFF_F000;
    let read_efaults = read_user_buf(UNMAPPED_USER, 8).is_none();
    let str_efaults = read_user_string(UNMAPPED_USER, 8).is_none();
    let write_efaults = !write_user_buf(UNMAPPED_USER, &[0u8; 8]);
    // Happy path: the trivial valid inputs (empty buffer / null+zero-len) must
    // still succeed, so the test distinguishes "fails closed on bad pointers"
    // from "rejects everything". (The actual copy round-trip is proven by the
    // `[extable]` smoketest, which exercises copy_user_with_fixup directly —
    // these helpers reject the higher-half kernel addresses such a copy would
    // use, by design.)
    let happy_ok =
        read_user_buf(0, 0).map(|v| v.is_empty()).unwrap_or(false) && write_user_buf(0, &[]);
    let fault_fixup_pass = read_efaults && str_efaults && write_efaults && happy_ok;

    crate::serial_println!(
        "[linux_syscall] smoketest: tracked={} efault_read={} efault_str={} efault_write={} happy={} -> {}",
        tracked,
        read_efaults,
        str_efaults,
        write_efaults,
        happy_ok,
        if fault_fixup_pass { "PASS" } else { "FAIL" }
    );
    crate::selftest::record_smoketest("linux_user_ptr_fixup", fault_fixup_pass);
}

/// `/proc/raeen/linux_syscall` — Linux ABI translation runtime counters.
pub fn dump_text() -> String {
    let total = TOTAL_DISPATCHED.load(Ordering::Relaxed);
    let unhandled = UNHANDLED_DISPATCHED.load(Ordering::Relaxed);
    let handled = total.saturating_sub(unhandled);
    let last = LAST_NR.load(Ordering::Relaxed);
    let tracked = LINUX_TASKS.lock().len();

    let mut out = String::from("# AthenaOS Linux syscall translation\n");
    out.push_str("mode: Linux x86_64 ABI -> AthenaOS posix/syscall shims\n");
    out.push_str(&alloc::format!("linux_tasks_tracked: {}\n", tracked));
    out.push_str(&alloc::format!("total_dispatched: {}\n", total));
    out.push_str(&alloc::format!("handled_or_stubbed: {}\n", handled));
    out.push_str(&alloc::format!("unhandled_enosys: {}\n", unhandled));
    if last == u64::MAX {
        out.push_str("last_syscall_nr: none\n");
    } else {
        out.push_str(&alloc::format!("last_syscall_nr: {}\n", last));
    }
    out
}
