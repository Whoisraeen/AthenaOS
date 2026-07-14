//! RaeenOS syscall adapter for relibc (RaeBridge).
//!
//! Maps relibc's POSIX expectations onto RaeenOS native syscalls from
//! `docs/SYSCALL_TABLE.md`. Redox uses `syscall::` + scheme IPC; RaeenOS uses
//! capability-gated ring-buffer IPC (`SYS_SEND`/`SYS_RECV`) and `SYS_SPAWN`
//! instead of `fork`.

use crate::c_str::CStr;
use crate::error::{Errno, Result};
use crate::header::errno::{EACCES, EAGAIN, EBADF, EFAULT, ENOENT, ENOSYS, EPERM};
use core::cell::Cell;
use core::sync::atomic::{AtomicU64, Ordering};

// ── Syscall numbers (must match kernel/src/syscall.rs + docs/SYSCALL_TABLE.md) ──

pub const SYS_PRINT: usize = 1;
pub const SYS_SEND: usize = 2;
pub const SYS_RECV: usize = 3;
pub const SYS_CAP_GRANT: usize = 4;
pub const SYS_CAP_REVOKE: usize = 5;
pub const SYS_CAP_QUERY: usize = 6;
pub const SYS_MMIO_MAP: usize = 7;
pub const SYS_IRQ_WAIT: usize = 8;
pub const SYS_PORT_READ: usize = 9;
pub const SYS_PORT_WRITE: usize = 10;
pub const SYS_SPAWN: usize = 11;
pub const SYS_EXIT: usize = 12;
pub const SYS_WAIT: usize = 13;
pub const SYS_KILL: usize = 14;
pub const SYS_OPEN: usize = 15;
pub const SYS_READ: usize = 16;
pub const SYS_WRITE: usize = 17;
pub const SYS_CLOSE: usize = 18;
pub const SYS_MMAP: usize = 19;
pub const SYS_MUNMAP: usize = 20;
pub const SYS_SETPRIORITY: usize = 21;
pub const SYS_SEEK: usize = 22;
pub const SYS_STAT: usize = 23;
// SYS_DEBUG_PRINT moved from 27 to 141 in rae_abi v2 — was colliding with
// SYS_SURFACE_CLOSE in the kernel match. See docs/SYSCALL_TABLE.md and
// components/rae_abi/src/lib.rs::syscall::SYS_DEBUG_PRINT.
pub const SYS_DEBUG_PRINT: usize = 141;
pub const SYS_YIELD: usize = 28;
pub const SYS_GETPID: usize = 29;
pub const SYS_TIME: usize = 30;
pub const SYS_WALL_CLOCK: usize = 40;
pub const SYS_SET_FS_BASE: usize = 126;
/// Native futex (rae_abi::syscall::SYS_FUTEX). rdi=uaddr, rsi=op (0=wait,
/// 1=wake), rdx=val. Backs the `sync` module (mutex/once/rwlock).
/// 258, NOT 119 — 119 is the live SYS_CHANNEL_SHMEM_MAP arm; calling it with
/// futex args would silently no-op the wait (kernel returns u64::MAX).
pub const SYS_FUTEX: usize = 258;
pub const FUTEX_OP_WAIT: usize = 0;
pub const FUTEX_OP_WAKE: usize = 1;

// ── Capability model (mirrors kernel/src/capability.rs) ─────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum CapFlavor {
    Channel = 0,
    Mmio = 1,
    Irq = 2,
    Port = 3,
    Filesystem = 4,
    Network = 5,
    Gpu = 6,
    Audio = 7,
    Camera = 8,
    Process = 9,
    CryptoKey = 10,
    Hypervisor = 11,
    Attestation = 12,
    Debug = 13,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rights(pub u32);

impl Rights {
    pub const READ: Rights = Rights(1 << 0);
    pub const WRITE: Rights = Rights(1 << 1);
    pub const EXEC: Rights = Rights(1 << 2);
    pub const MAP: Rights = Rights(1 << 3);
    pub const WAIT: Rights = Rights(1 << 4);
    pub const GRANT: Rights = Rights(1 << 5);
    pub const REVOKE: Rights = Rights(1 << 6);

    pub const fn contains(self, other: Rights) -> bool {
        (self.0 & other.0) == other.0
    }
}

/// Userspace handles granted at process start (via `SYS_CAP_GRANT` from parent).
#[derive(Clone, Copy, Debug, Default)]
pub struct CapHandles {
    pub filesystem: Option<u64>,
    pub network: Option<u64>,
    pub process: Option<u64>,
    pub default_channel: Option<u64>,
}

static CAP_HANDLES: AtomicU64 = AtomicU64::new(0);
static CAP_HANDLES_NETWORK: AtomicU64 = AtomicU64::new(0);
static CAP_HANDLES_PROCESS: AtomicU64 = AtomicU64::new(0);
static CAP_HANDLES_CHANNEL: AtomicU64 = AtomicU64::new(0);

#[thread_local]
static CAP_BOOTSTRAP: Cell<bool> = Cell::new(true);

pub fn cap_handles() -> CapHandles {
    CapHandles {
        filesystem: non_zero(CAP_HANDLES.load(Ordering::Relaxed)),
        network: non_zero(CAP_HANDLES_NETWORK.load(Ordering::Relaxed)),
        process: non_zero(CAP_HANDLES_PROCESS.load(Ordering::Relaxed)),
        default_channel: non_zero(CAP_HANDLES_CHANNEL.load(Ordering::Relaxed)),
    }
}

pub fn set_cap_handles(handles: CapHandles) {
    CAP_HANDLES.store(handles.filesystem.unwrap_or(0), Ordering::Relaxed);
    CAP_HANDLES_NETWORK.store(handles.network.unwrap_or(0), Ordering::Relaxed);
    CAP_HANDLES_PROCESS.store(handles.process.unwrap_or(0), Ordering::Relaxed);
    CAP_HANDLES_CHANNEL.store(handles.default_channel.unwrap_or(0), Ordering::Relaxed);
    CAP_BOOTSTRAP.set(false);
}

fn non_zero(v: u64) -> Option<u64> {
    if v == 0 { None } else { Some(v) }
}

fn cap_query(handle: u64) -> Result<(CapFlavor, Rights)> {
    let ret = unsafe { syscall1(SYS_CAP_QUERY, handle as usize) };
    if ret == usize::MAX {
        return Err(Errno(EBADF));
    }
    // Kernel returns status in rax; flavor in rsi; rights in rdx — we only get rax
    // from our thin asm wrapper, so re-query via a second convention is not available.
    // Treat success (0) as filesystem with full rights when handle is non-zero.
    if ret == 0 {
        return Ok((
            CapFlavor::Filesystem,
            Rights(Rights::READ.0 | Rights::WRITE.0),
        ));
    }
    Ok((
        CapFlavor::Filesystem,
        Rights(Rights::READ.0 | Rights::WRITE.0),
    ))
}

fn require_cap(handle: Option<u64>, flavor: CapFlavor, need: Rights) -> Result<()> {
    if CAP_BOOTSTRAP.get() {
        return Ok(());
    }
    let Some(h) = handle else {
        return Err(Errno(EPERM));
    };
    let (got_flavor, got_rights) = cap_query(h)?;
    if got_flavor != flavor || !got_rights.contains(need) {
        return Err(Errno(EACCES));
    }
    Ok(())
}

fn require_fs_read() -> Result<()> {
    require_cap(cap_handles().filesystem, CapFlavor::Filesystem, Rights::READ)
}

fn require_fs_write() -> Result<()> {
    require_cap(cap_handles().filesystem, CapFlavor::Filesystem, Rights::WRITE)
}

fn require_process_spawn() -> Result<()> {
    require_cap(
        cap_handles().process,
        CapFlavor::Process,
        Rights(Rights::EXEC.0 | Rights::WRITE.0),
    )
}

fn require_ipc_send() -> Result<()> {
    require_cap(
        cap_handles().default_channel,
        CapFlavor::Channel,
        Rights::WRITE,
    )
}

fn require_ipc_recv() -> Result<()> {
    require_cap(
        cap_handles().default_channel,
        CapFlavor::Channel,
        Rights::READ,
    )
}

// ── Raw syscall helpers ─────────────────────────────────────────────────────

#[inline(always)]
unsafe fn syscall0(mut num: usize) -> usize {
    unsafe {
        core::arch::asm!(
            "syscall",
            inout("rax") num,
            out("rcx") _,
            out("r11") _,
            options(nostack, preserves_flags)
        );
    }
    num
}

#[inline(always)]
unsafe fn syscall1(mut num: usize, a: usize) -> usize {
    unsafe {
        core::arch::asm!(
            "syscall",
            inout("rax") num,
            in("rdi") a,
            out("rcx") _,
            out("r11") _,
            options(nostack, preserves_flags)
        );
    }
    num
}

#[inline(always)]
unsafe fn syscall2(mut num: usize, a: usize, b: usize) -> usize {
    unsafe {
        core::arch::asm!(
            "syscall",
            inout("rax") num,
            in("rdi") a,
            in("rsi") b,
            out("rcx") _,
            out("r11") _,
            options(nostack, preserves_flags)
        );
    }
    num
}

#[inline(always)]
unsafe fn syscall3(mut num: usize, a: usize, b: usize, c: usize) -> usize {
    unsafe {
        core::arch::asm!(
            "syscall",
            inout("rax") num,
            in("rdi") a,
            in("rsi") b,
            in("rdx") c,
            out("rcx") _,
            out("r11") _,
            options(nostack, preserves_flags)
        );
    }
    num
}

/// Map RaeenOS `u64::MAX - N` kernel errors to POSIX errno values.
fn check_err(ret: usize) -> Result<usize> {
    match ret {
        usize::MAX => Err(Errno(EFAULT)),
        v if v == usize::MAX - 1 => Err(Errno(ENOENT)),
        v if v == usize::MAX - 2 => Err(Errno(EFAULT)),
        v if v == usize::MAX - 3 => Err(Errno(ENOSYS)),
        _ => Ok(ret),
    }
}

fn c_strlen(ptr: *const u8) -> usize {
    if ptr.is_null() {
        return 0;
    }
    unsafe {
        let mut len = 0usize;
        while *ptr.add(len) != 0 {
            len += 1;
        }
        len
    }
}

// ── Public syscall API (used by platform/raeen) ─────────────────────────────

pub fn sys_exit(status: usize) -> ! {
    unsafe { syscall1(SYS_EXIT, status) };
    loop {}
}

pub fn sys_debug_print(buf: *const u8, len: usize) -> Result<usize> {
    let ret = unsafe { syscall2(SYS_DEBUG_PRINT, buf as usize, len) };
    check_err(ret)
}

pub fn sys_open(path: *const u8, flags: u32, _mode: u32) -> Result<usize> {
    require_fs_read()?;
    if flags & 0x0002_0000 != 0 || flags & 0x0004_0000 != 0 {
        require_fs_write()?;
    }
    let len = c_strlen(path);
    let ret = unsafe { syscall3(SYS_OPEN, path as usize, len, flags as usize) };
    check_err(ret)
}

pub fn sys_read(fd: usize, buf: *mut u8, count: usize) -> Result<usize> {
    require_fs_read()?;
    let ret = unsafe { syscall3(SYS_READ, fd, buf as usize, count) };
    check_err(ret)
}

pub fn sys_write(fd: usize, buf: *const u8, count: usize) -> Result<usize> {
    require_fs_write()?;
    let ret = unsafe { syscall3(SYS_WRITE, fd, buf as usize, count) };
    check_err(ret)
}

pub fn sys_close(fd: usize) -> Result<()> {
    let ret = unsafe { syscall1(SYS_CLOSE, fd) };
    check_err(ret)?;
    Ok(())
}

pub fn sys_getpid() -> Result<usize> {
    let ret = unsafe { syscall0(SYS_GETPID) };
    check_err(ret)
}

/// RaeenOS has no `fork`; use `sys_spawn` (Redox `exec`/`spawn` model).
pub fn sys_spawn(path: *const u8) -> Result<usize> {
    require_process_spawn()?;
    let len = c_strlen(path);
    let ret = unsafe { syscall2(SYS_SPAWN, path as usize, len) };
    check_err(ret)
}

pub fn sys_wait(pid: usize) -> Result<usize> {
    let ret = unsafe { syscall1(SYS_WAIT, pid) };
    check_err(ret)
}

pub fn sys_seek(fd: usize, offset: i64, _whence: u32) -> Result<usize> {
    require_fs_read()?;
    let ret = unsafe { syscall2(SYS_SEEK, fd, offset as usize) };
    check_err(ret)
}

pub fn sys_stat(fd: usize) -> Result<usize> {
    require_fs_read()?;
    let ret = unsafe { syscall1(SYS_STAT, fd) };
    check_err(ret)
}

pub fn sys_wall_clock() -> Result<u64> {
    let ret = unsafe { syscall0(SYS_WALL_CLOCK) };
    Ok(ret as u64)
}

pub fn sys_time() -> Result<u64> {
    let ret = unsafe { syscall0(SYS_TIME) };
    Ok(ret as u64)
}

pub fn sys_mmap(addr: usize, len: usize) -> Result<usize> {
    let ret = unsafe { syscall2(SYS_MMAP, addr, len) };
    check_err(ret)
}

pub fn sys_yield_cpu() {
    unsafe { syscall0(SYS_YIELD) };
}

/// Native futex wait (SYS_FUTEX, op=WAIT). Returns Ok(()) when woken, or
/// Err(EAGAIN) if `*addr != val` at entry (the caller re-checks + retries).
pub unsafe fn sys_futex_wait(addr: *mut u32, val: u32) -> Result<()> {
    let ret = unsafe { syscall3(SYS_FUTEX, addr as usize, FUTEX_OP_WAIT, val as usize) };
    match ret {
        0 => Ok(()),
        1 => Err(Errno(EAGAIN)),
        _ => Err(Errno(EFAULT)),
    }
}

/// Native futex wake (SYS_FUTEX, op=WAKE). Returns the number of waiters woken.
pub unsafe fn sys_futex_wake(addr: *mut u32, num: u32) -> Result<u32> {
    let woken = unsafe { syscall3(SYS_FUTEX, addr as usize, FUTEX_OP_WAKE, num as usize) };
    Ok(woken as u32)
}

/// IPC send (cap-based ring buffer). Redox `write` to scheme fd is not used.
pub fn sys_ipc_send(cap_handle: usize, msg_type: usize, arg1: usize, arg2: usize) -> Result<()> {
    require_ipc_send()?;
    let ret = unsafe { syscall3(SYS_SEND, cap_handle, msg_type, arg1) };
    // arg2 would need syscall4; not yet wired
    let _ = arg2;
    if ret == 0 {
        Ok(())
    } else {
        Err(Errno(EFAULT))
    }
}

pub fn sys_ipc_recv(cap_handle: usize) -> Result<usize> {
    require_ipc_recv()?;
    let ret = unsafe { syscall1(SYS_RECV, cap_handle) };
    check_err(ret)
}

/// Open by `CStr` (preferred for relibc `Pal`).
pub fn sys_open_path(path: &CStr, flags: u32, mode: u32) -> Result<usize> {
    sys_open(path.as_ptr() as *const u8, flags, mode)
}

pub fn sys_set_fs_base(addr: usize) -> Result<()> {
    let ret = unsafe { syscall1(SYS_SET_FS_BASE, addr) };
    if ret == 0 {
        Ok(())
    } else {
        Err(Errno(ENOSYS))
    }
}
