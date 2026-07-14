//! POSIX compatibility layer for AthenaOS.
//!
//! Maps standard POSIX syscall semantics onto the existing AthenaOS kernel
//! infrastructure (VFS, fd table, scheduler, memory management, signals).
//! This is NOT a Linux clone — it provides the POSIX interface that Linux
//! apps expect, backed by AthenaOS's own subsystems.

#![allow(dead_code)]

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;

// ═══════════════════════════════════════════════════════════════════════════════
// POSIX Error Numbers (errno)
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum Errno {
    Success = 0,
    Eperm = 1,
    Enoent = 2,
    Esrch = 3,
    Eintr = 4,
    Eio = 5,
    Enxio = 6,
    E2big = 7,
    Enoexec = 8,
    Ebadf = 9,
    Echild = 10,
    Eagain = 11,
    Enomem = 12,
    Eacces = 13,
    Efault = 14,
    Ebusy = 16,
    Eexist = 17,
    Enodev = 19,
    Enotdir = 20,
    Eisdir = 21,
    Einval = 22,
    Enfile = 23,
    Emfile = 24,
    Enotty = 25,
    Enospc = 28,
    Espipe = 29,
    Erofs = 30,
    Epipe = 32,
    Erange = 34,
    Enosys = 38,
    Enotempty = 39,
    Eloop = 40,
    Enodata = 61,
    Enotsock = 88,
    Eaddrinuse = 98,
    Econnrefused = 111,
    Etimedout = 110,
    Ealready = 114,
    Einprogress = 115,
    Eafnosupport = 97,
    Eprotonosupport = 93,
}

impl Errno {
    pub fn as_neg(self) -> i64 {
        -(self as i32 as i64)
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// POSIX Constants
// ═══════════════════════════════════════════════════════════════════════════════

pub const O_RDONLY: u32 = 0x0000;
pub const O_WRONLY: u32 = 0x0001;
pub const O_RDWR: u32 = 0x0002;
pub const O_CREAT: u32 = 0x0040;
pub const O_EXCL: u32 = 0x0080;
pub const O_TRUNC: u32 = 0x0200;
pub const O_APPEND: u32 = 0x0400;
pub const O_NONBLOCK: u32 = 0x0800;
pub const O_DIRECTORY: u32 = 0x10000;
pub const O_CLOEXEC: u32 = 0x80000;

pub const SEEK_SET: u32 = 0;
pub const SEEK_CUR: u32 = 1;
pub const SEEK_END: u32 = 2;

pub const F_DUPFD: u32 = 0;
pub const F_GETFD: u32 = 1;
pub const F_SETFD: u32 = 2;
pub const F_GETFL: u32 = 3;
pub const F_SETFL: u32 = 4;
pub const F_DUPFD_CLOEXEC: u32 = 0x406;
pub const FD_CLOEXEC: u32 = 1;

pub const PROT_NONE: u32 = 0x0;
pub const PROT_READ: u32 = 0x1;
pub const PROT_WRITE: u32 = 0x2;
pub const PROT_EXEC: u32 = 0x4;

pub const MAP_SHARED: u32 = 0x01;
pub const MAP_PRIVATE: u32 = 0x02;
pub const MAP_FIXED: u32 = 0x10;
pub const MAP_ANONYMOUS: u32 = 0x20;

pub const MAP_FAILED: u64 = u64::MAX;

pub const WNOHANG: u32 = 1;
pub const WUNTRACED: u32 = 2;
pub const WCONTINUED: u32 = 8;

pub const SIG_BLOCK: u32 = 0;
pub const SIG_UNBLOCK: u32 = 1;
pub const SIG_SETMASK: u32 = 2;

pub const CLOCK_REALTIME: u32 = 0;
pub const CLOCK_MONOTONIC: u32 = 1;

pub const AF_UNIX: u32 = 1;
pub const AF_INET: u32 = 2;
pub const AF_INET6: u32 = 10;

pub const SOCK_STREAM: u32 = 1;
pub const SOCK_DGRAM: u32 = 2;
pub const SOCK_RAW: u32 = 3;

pub const POLLIN: u16 = 0x001;
pub const POLLOUT: u16 = 0x004;
pub const POLLERR: u16 = 0x008;
pub const POLLHUP: u16 = 0x010;

pub const DT_UNKNOWN: u8 = 0;
pub const DT_FIFO: u8 = 1;
pub const DT_CHR: u8 = 2;
pub const DT_DIR: u8 = 4;
pub const DT_BLK: u8 = 6;
pub const DT_REG: u8 = 8;
pub const DT_LNK: u8 = 10;
pub const DT_SOCK: u8 = 12;

// Maximum FDs per process (matches task.rs fd array size)
const MAX_FDS: usize = 32;
const MAX_PATH: usize = 4096;

// ═══════════════════════════════════════════════════════════════════════════════
// POSIX Structures
// ═══════════════════════════════════════════════════════════════════════════════

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct Stat {
    pub st_dev: u64,
    pub st_ino: u64,
    pub st_nlink: u64,
    pub st_mode: u32,
    pub st_uid: u32,
    pub st_gid: u32,
    pub _pad0: u32,
    pub st_rdev: u64,
    pub st_size: i64,
    pub st_blksize: i64,
    pub st_blocks: i64,
    pub st_atime: i64,
    pub st_atime_nsec: i64,
    pub st_mtime: i64,
    pub st_mtime_nsec: i64,
    pub st_ctime: i64,
    pub st_ctime_nsec: i64,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct Timespec {
    pub tv_sec: i64,
    pub tv_nsec: i64,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct Timeval {
    pub tv_sec: i64,
    pub tv_usec: i64,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct Timezone {
    pub tz_minuteswest: i32,
    pub tz_dsttime: i32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct PollFd {
    pub fd: i32,
    pub events: u16,
    pub revents: u16,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Dirent {
    pub d_ino: u64,
    pub d_off: i64,
    pub d_reclen: u16,
    pub d_type: u8,
    pub d_name: [u8; 256],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct SockaddrIn {
    pub sin_family: u16,
    pub sin_port: u16,
    pub sin_addr: u32,
    pub sin_zero: [u8; 8],
}

// ═══════════════════════════════════════════════════════════════════════════════
// Pipe Infrastructure
// ═══════════════════════════════════════════════════════════════════════════════

const PIPE_BUF_SIZE: usize = 4096;

struct PipeBuffer {
    data: [u8; PIPE_BUF_SIZE],
    read_pos: usize,
    write_pos: usize,
    count: usize,
    readers: u32,
    writers: u32,
}

impl PipeBuffer {
    fn new() -> Self {
        Self {
            data: [0u8; PIPE_BUF_SIZE],
            read_pos: 0,
            write_pos: 0,
            count: 0,
            readers: 1,
            writers: 1,
        }
    }

    fn read(&mut self, buf: &mut [u8]) -> usize {
        let n = buf.len().min(self.count);
        for i in 0..n {
            buf[i] = self.data[self.read_pos];
            self.read_pos = (self.read_pos + 1) % PIPE_BUF_SIZE;
        }
        self.count -= n;
        n
    }

    fn write(&mut self, buf: &[u8]) -> usize {
        let avail = PIPE_BUF_SIZE - self.count;
        let n = buf.len().min(avail);
        for i in 0..n {
            self.data[self.write_pos] = buf[i];
            self.write_pos = (self.write_pos + 1) % PIPE_BUF_SIZE;
        }
        self.count += n;
        n
    }
}

static PIPE_TABLE: Mutex<BTreeMap<u64, Arc<Mutex<PipeBuffer>>>> = Mutex::new(BTreeMap::new());
static NEXT_PIPE_ID: AtomicU64 = AtomicU64::new(1);

// ═══════════════════════════════════════════════════════════════════════════════
// Socket Infrastructure (stub)
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SocketState {
    Unbound,
    Bound,
    Listening,
    Connected,
    Closed,
}

pub struct Socket {
    pub domain: u32,
    pub sock_type: u32,
    pub protocol: u32,
    pub state: SocketState,
    pub local_addr: Option<SockaddrIn>,
    pub remote_addr: Option<SockaddrIn>,
    pub backlog: u32,
    pub recv_buf: Vec<u8>,
    pub send_buf: Vec<u8>,
}

impl Socket {
    fn new(domain: u32, sock_type: u32, protocol: u32) -> Self {
        Self {
            domain,
            sock_type,
            protocol,
            state: SocketState::Unbound,
            local_addr: None,
            remote_addr: None,
            backlog: 0,
            recv_buf: Vec::new(),
            send_buf: Vec::new(),
        }
    }
}

static SOCKET_TABLE: Mutex<BTreeMap<u64, Socket>> = Mutex::new(BTreeMap::new());
static NEXT_SOCKET_ID: AtomicU64 = AtomicU64::new(1);

// ═══════════════════════════════════════════════════════════════════════════════
// Per-process POSIX state (tracks cwd, brk, supplementary info)
// ═══════════════════════════════════════════════════════════════════════════════

pub struct PosixProcessState {
    pub pid: u64,
    pub ppid: u64,
    pub uid: u32,
    pub gid: u32,
    pub euid: u32,
    pub egid: u32,
    pub cwd: String,
    pub brk_current: u64,
    pub brk_base: u64,
    pub umask: u32,
    pub pgid: u64,
    pub sid: u64,
    pub is_linux_elf: bool,
    /// Bump pointer for hint-less mmap() allocations. MUST advance per mapping:
    /// the old `brk_current + 0x100000` formula returned the SAME address for
    /// every addr=0 mmap, so ld.so's TLS block landed on top of libc's already
    /// mapped .gnu.hash/.dynsym (libc base 0x700000, TLS at 0x701100), and TLS
    /// init clobbered the symbol tables — breaking versioned lookups like
    /// `_res@GLIBC_2.2.5`. Start high, clear of brk and the user stack.
    pub mmap_cursor: u64,
}

/// Base for hint-less mmap allocations (64 GiB) — far above brk (~6 MiB) and the
/// program load, far below the user stack (0x7fff_ffff_xxxx) and USER_SPACE_END.
pub const MMAP_BASE: u64 = 0x10_0000_0000;

impl PosixProcessState {
    pub fn new(pid: u64, ppid: u64) -> Self {
        Self {
            pid,
            ppid,
            uid: 1000,
            gid: 1000,
            euid: 1000,
            egid: 1000,
            cwd: String::from("/"),
            brk_current: 0x0060_0000,
            brk_base: 0x0060_0000,
            umask: 0o022,
            pgid: pid,
            sid: pid,
            is_linux_elf: false,
            mmap_cursor: MMAP_BASE,
        }
    }

    pub fn fork_state(&self, child_pid: u64) -> Self {
        Self {
            pid: child_pid,
            ppid: self.pid,
            uid: self.uid,
            gid: self.gid,
            euid: self.euid,
            egid: self.egid,
            cwd: self.cwd.clone(),
            brk_current: self.brk_current,
            brk_base: self.brk_base,
            umask: self.umask,
            pgid: self.pgid,
            sid: self.sid,
            is_linux_elf: self.is_linux_elf,
            mmap_cursor: self.mmap_cursor,
        }
    }
}

pub static POSIX_STATE: Mutex<BTreeMap<u64, PosixProcessState>> = Mutex::new(BTreeMap::new());

fn with_posix_state<F, R>(pid: u64, f: F) -> Result<R, Errno>
where
    F: FnOnce(&PosixProcessState) -> R,
{
    let table = POSIX_STATE.lock();
    table.get(&pid).map(f).ok_or(Errno::Esrch)
}

fn with_posix_state_mut<F, R>(pid: u64, f: F) -> Result<R, Errno>
where
    F: FnOnce(&mut PosixProcessState) -> R,
{
    let mut table = POSIX_STATE.lock();
    table.get_mut(&pid).map(f).ok_or(Errno::Esrch)
}

// ═══════════════════════════════════════════════════════════════════════════════
// Section 1: File I/O
// ═══════════════════════════════════════════════════════════════════════════════

pub fn sys_open(path: &str, flags: u32, _mode: u32) -> Result<u64, Errno> {
    // When the caller explicitly opens a DIRECTORY (O_DIRECTORY — what glibc
    // opendir() and `ls` pass), build the getdents64 dirent stream FIRST.
    // open_path() can return a non-directory inode for a directory path (e.g.
    // an initramfs prefix match), which would shadow the dirent stream and make
    // getdents64 return nothing.
    let inode = if (flags & O_DIRECTORY) != 0 {
        crate::vfs::open_dir_as_dirent_stream(path)
            .or_else(|| crate::vfs::open_path(path))
            .ok_or(Errno::Enoent)?
    } else {
        crate::vfs::open_path(path).ok_or(Errno::Enoent)?
    };
    let file = crate::vfs::File::new(inode, flags);
    let file_arc = alloc::sync::Arc::new(spin::Mutex::new(file));

    let mut fd = Err(Errno::Emfile);
    crate::scheduler::with_current_task_mut(|task| {
        for (i, slot) in task.fds.iter_mut().enumerate() {
            if slot.is_none() {
                *slot = Some(file_arc.clone());
                fd = Ok(i as u64);
                break;
            }
        }
    });
    fd
}

pub fn sys_close(fd: u32) -> Result<(), Errno> {
    let mut closed = false;
    crate::scheduler::with_current_task_mut(|task| {
        let fd = fd as usize;
        if fd < task.fds.len() && task.fds[fd].is_some() {
            task.fds[fd] = None;
            closed = true;
        }
    });
    if closed {
        Ok(())
    } else {
        Err(Errno::Ebadf)
    }
}

pub fn sys_read(fd: u32, buf: &mut [u8]) -> Result<usize, Errno> {
    let file_arc = crate::scheduler::with_current_task(|task| {
        let fd = fd as usize;
        if fd < task.fds.len() {
            task.fds[fd].clone()
        } else {
            None
        }
    })
    .flatten();

    match file_arc {
        Some(f) => {
            let mut file = f.lock();
            Ok(file.read(buf))
        }
        None => Err(Errno::Ebadf),
    }
}

pub fn sys_write(fd: u32, buf: &[u8]) -> Result<usize, Errno> {
    let file_arc = crate::scheduler::with_current_task(|task| {
        let fd = fd as usize;
        if fd < task.fds.len() {
            task.fds[fd].clone()
        } else {
            None
        }
    })
    .flatten();

    match file_arc {
        Some(f) => {
            let mut file = f.lock();
            Ok(file.write(buf))
        }
        None => Err(Errno::Ebadf),
    }
}

pub fn sys_lseek(fd: u32, offset: i64, whence: u32) -> Result<u64, Errno> {
    let file_arc = crate::scheduler::with_current_task(|task| {
        let fd = fd as usize;
        if fd < task.fds.len() {
            task.fds[fd].clone()
        } else {
            None
        }
    })
    .flatten();

    match file_arc {
        Some(f) => {
            let mut file = f.lock();
            let new_pos = match whence {
                SEEK_SET => offset.max(0) as usize,
                SEEK_CUR => (file.offset as i64 + offset).max(0) as usize,
                SEEK_END => {
                    let size = file.inode.size() as i64;
                    (size + offset).max(0) as usize
                }
                _ => return Err(Errno::Einval),
            };
            file.seek(new_pos);
            Ok(new_pos as u64)
        }
        None => Err(Errno::Ebadf),
    }
}

pub fn sys_stat(path: &str, stat_buf: &mut Stat) -> Result<(), Errno> {
    *stat_buf = Stat::default();
    stat_buf.st_nlink = 1;
    stat_buf.st_blksize = 4096;
    stat_buf.st_ino = 1; // placeholder

    // Directories: resolve cheaply (no dirent enumeration) and report the
    // correct S_IFDIR mode. open_path() builds a full readdir stream for a
    // directory, so the old "open then read size" path made stat("/") crawl and
    // returned a regular-file mode for directories.
    if crate::vfs::is_dir(path) {
        stat_buf.st_mode = 0o040755; // S_IFDIR | rwxr-xr-x
        stat_buf.st_size = 0;
        return Ok(());
    }

    if crate::vfs::is_render_node(path) {
        stat_buf.st_mode = 0o020666; // S_IFCHR | rw-rw-rw- (authority is broker/caps)
        stat_buf.st_rdev = (226u64 << 8) | 128; // stable Linux DRM render minor
        stat_buf.st_dev = 1;
        stat_buf.st_ino = 0xD128;
        return Ok(());
    }

    let inode = crate::vfs::open_path(path).ok_or(Errno::Enoent)?;
    let size = inode.size();
    stat_buf.st_size = size as i64;
    stat_buf.st_mode = 0o100644; // S_IFREG | rw-r--r--
    stat_buf.st_blocks = ((size + 511) / 512) as i64;
    // Unique, nonzero (st_dev, st_ino) per inode. glibc's ld.so dedups loaded
    // objects by (st_dev, st_ino); a constant ino made it treat libc.so.6 as
    // "already loaded" (same ino 0 as the main exe) and never map it, so
    // __libc_start_main was undefined. The live inode pointer is a stable unique id.
    stat_buf.st_dev = 1;
    stat_buf.st_ino = alloc::sync::Arc::as_ptr(&inode) as *const () as u64;
    Ok(())
}

pub fn sys_fstat(fd: u32, stat_buf: &mut Stat) -> Result<(), Errno> {
    let file_arc = crate::scheduler::with_current_task(|task| {
        let fd = fd as usize;
        if fd < task.fds.len() {
            task.fds[fd].clone()
        } else {
            None
        }
    })
    .flatten();

    match file_arc {
        Some(f) => {
            let file = f.lock();
            let size = file.inode.size();
            *stat_buf = Stat::default();
            stat_buf.st_size = size as i64;
            stat_buf.st_mode = 0o100644;
            if file.inode.render_client_id().is_some() {
                stat_buf.st_mode = 0o020666;
                stat_buf.st_rdev = (226u64 << 8) | 128;
            }
            stat_buf.st_nlink = 1;
            stat_buf.st_blksize = 4096;
            stat_buf.st_blocks = ((size + 511) / 512) as i64;
            // Unique nonzero (st_dev, st_ino) — see sys_stat: ld.so dedups loaded
            // objects by inode identity, so libc.so.6 must not share ino 0 with
            // the main exe or it is treated as already-mapped (symbol lookup fails).
            stat_buf.st_dev = 1;
            stat_buf.st_ino = alloc::sync::Arc::as_ptr(&file.inode) as *const () as u64;
            Ok(())
        }
        None => Err(Errno::Ebadf),
    }
}

pub fn sys_dup(old_fd: u32) -> Result<u32, Errno> {
    let file_arc = crate::scheduler::with_current_task(|task| {
        let fd = old_fd as usize;
        if fd < task.fds.len() {
            task.fds[fd].clone()
        } else {
            None
        }
    })
    .flatten();

    let file_arc = file_arc.ok_or(Errno::Ebadf)?;

    let mut new_fd = Err(Errno::Emfile);
    crate::scheduler::with_current_task_mut(|task| {
        for (i, slot) in task.fds.iter_mut().enumerate() {
            if slot.is_none() {
                *slot = Some(file_arc.clone());
                new_fd = Ok(i as u32);
                break;
            }
        }
    });
    new_fd
}

pub fn sys_dup2(old_fd: u32, new_fd: u32) -> Result<u32, Errno> {
    if old_fd == new_fd {
        let exists = crate::scheduler::with_current_task(|task| {
            let fd = old_fd as usize;
            fd < task.fds.len() && task.fds[fd].is_some()
        })
        .unwrap_or(false);
        return if exists {
            Ok(new_fd)
        } else {
            Err(Errno::Ebadf)
        };
    }

    let file_arc = crate::scheduler::with_current_task(|task| {
        let fd = old_fd as usize;
        if fd < task.fds.len() {
            task.fds[fd].clone()
        } else {
            None
        }
    })
    .flatten();

    let file_arc = file_arc.ok_or(Errno::Ebadf)?;
    let nfd = new_fd as usize;

    crate::scheduler::with_current_task_mut(|task| {
        if nfd < task.fds.len() {
            task.fds[nfd] = Some(file_arc.clone());
        }
    });
    Ok(new_fd)
}

pub fn sys_pipe(fds: &mut [u32; 2]) -> Result<(), Errno> {
    let pipe_id = NEXT_PIPE_ID.fetch_add(1, Ordering::Relaxed);
    let buf = Arc::new(Mutex::new(PipeBuffer::new()));
    PIPE_TABLE.lock().insert(pipe_id, buf.clone());

    // Create read-end inode
    let read_inode: Arc<dyn crate::vfs::Inode> = Arc::new(PipeInode {
        pipe_id,
        is_read: true,
    });
    let write_inode: Arc<dyn crate::vfs::Inode> = Arc::new(PipeInode {
        pipe_id,
        is_read: false,
    });

    let read_file = crate::vfs::File::new(read_inode, O_RDONLY);
    let write_file = crate::vfs::File::new(write_inode, O_WRONLY);

    let r_arc = alloc::sync::Arc::new(spin::Mutex::new(read_file));
    let w_arc = alloc::sync::Arc::new(spin::Mutex::new(write_file));

    let mut read_fd = None;
    let mut write_fd = None;

    crate::scheduler::with_current_task_mut(|task| {
        for (i, slot) in task.fds.iter_mut().enumerate() {
            if slot.is_none() {
                if read_fd.is_none() {
                    *slot = Some(r_arc.clone());
                    read_fd = Some(i as u32);
                } else if write_fd.is_none() {
                    *slot = Some(w_arc.clone());
                    write_fd = Some(i as u32);
                    break;
                }
            }
        }
    });

    match (read_fd, write_fd) {
        (Some(r), Some(w)) => {
            fds[0] = r;
            fds[1] = w;
            Ok(())
        }
        _ => Err(Errno::Emfile),
    }
}

struct PipeInode {
    pipe_id: u64,
    is_read: bool,
}

// SAFETY: PipeInode only accesses the global PIPE_TABLE behind a Mutex.
unsafe impl Send for PipeInode {}
unsafe impl Sync for PipeInode {}

impl crate::vfs::Inode for PipeInode {
    fn read_at(&self, _offset: usize, buf: &mut [u8]) -> usize {
        if !self.is_read {
            return 0;
        }
        let table = PIPE_TABLE.lock();
        if let Some(pipe) = table.get(&self.pipe_id) {
            pipe.lock().read(buf)
        } else {
            0
        }
    }

    fn write_at(&self, _offset: usize, buf: &[u8]) -> usize {
        if self.is_read {
            return 0;
        }
        let table = PIPE_TABLE.lock();
        if let Some(pipe) = table.get(&self.pipe_id) {
            pipe.lock().write(buf)
        } else {
            0
        }
    }

    fn size(&self) -> usize {
        let table = PIPE_TABLE.lock();
        if let Some(pipe) = table.get(&self.pipe_id) {
            pipe.lock().count
        } else {
            0
        }
    }
}

pub fn sys_fcntl(fd: u32, cmd: u32, arg: u64) -> Result<u64, Errno> {
    match cmd {
        F_DUPFD => sys_dup(fd).map(|v| v as u64),
        F_DUPFD_CLOEXEC => sys_dup(fd).map(|v| v as u64),
        F_GETFD => Ok(0),
        F_SETFD => Ok(0),
        F_GETFL => {
            let file_arc = crate::scheduler::with_current_task(|task| {
                let fd = fd as usize;
                if fd < task.fds.len() {
                    task.fds[fd].clone()
                } else {
                    None
                }
            })
            .flatten();
            match file_arc {
                Some(f) => Ok(f.lock().flags as u64),
                None => Err(Errno::Ebadf),
            }
        }
        F_SETFL => {
            let file_arc = crate::scheduler::with_current_task(|task| {
                let fd = fd as usize;
                if fd < task.fds.len() {
                    task.fds[fd].clone()
                } else {
                    None
                }
            })
            .flatten();
            match file_arc {
                Some(f) => {
                    f.lock().flags = arg as u32;
                    Ok(0)
                }
                None => Err(Errno::Ebadf),
            }
        }
        _ => Err(Errno::Einval),
    }
}

pub fn sys_ioctl(fd: u32, request: u64, _arg: u64) -> Result<u64, Errno> {
    let exists = crate::scheduler::with_current_task(|task| {
        let fd = fd as usize;
        fd < task.fds.len() && task.fds[fd].is_some()
    })
    .unwrap_or(false);

    if !exists {
        return Err(Errno::Ebadf);
    }

    // Terminal-control ioctls on a non-terminal fd. None of our fds (console
    // device, regular files, pipes, dirent streams) is a real TTY, so the POSIX-
    // correct answer is ENOTTY ("inappropriate ioctl for device") — NOT EINVAL.
    // This matters: glibc `isatty()` issues `ioctl(fd, TCGETS, …)` and many
    // interactive tools (coreutils `ls`, etc.) check `errno == ENOTTY` after it;
    // EINVAL made them treat a benign non-tty as a hard error and abort during
    // startup before doing any real work.
    const TCGETS: u64 = 0x5401;
    const TCSETS: u64 = 0x5402;
    const TCSETSW: u64 = 0x5403;
    const TCSETSF: u64 = 0x5404;
    const TCGETA: u64 = 0x5405;
    const TIOCGPGRP: u64 = 0x540F;
    const TIOCSPGRP: u64 = 0x5410;
    const TIOCGWINSZ: u64 = 0x5413;
    const TIOCSWINSZ: u64 = 0x5414;
    const TIOCGSID: u64 = 0x5429;
    match request {
        TCGETS | TCSETS | TCSETSW | TCSETSF | TCGETA | TIOCGPGRP | TIOCSPGRP | TIOCGWINSZ
        | TIOCSWINSZ | TIOCGSID => Err(Errno::Enotty),
        _ => Err(Errno::Einval),
    }
}

/// Return the per-open render client id for a DRM render-node fd. Kept as a
/// narrow query so the Linux syscall dispatcher can perform its scheduler-aware
/// block/retry transaction without teaching generic POSIX ioctl about DRM.
pub fn render_client_for_fd(fd: u32) -> Result<Option<u64>, Errno> {
    let file_arc = crate::scheduler::with_current_task(|task| {
        task.fds.get(fd as usize).and_then(|slot| slot.clone())
    })
    .flatten()
    .ok_or(Errno::Ebadf)?;
    let client_id = file_arc.lock().inode.render_client_id();
    Ok(client_id)
}

// ═══════════════════════════════════════════════════════════════════════════════
// Section 2: Process Management
// ═══════════════════════════════════════════════════════════════════════════════

pub fn sys_fork(caller_pid: u64) -> Result<u64, Errno> {
    let parent_pml4 = crate::scheduler::with_current_task(|task| task.pml4)
        .flatten()
        .ok_or(Errno::Enomem)?;

    let _child_pml4 = fork_cow_page_tables(parent_pml4)?;

    let child_id = crate::task::TaskId::new();

    // Register child in POSIX state table
    {
        let mut table = POSIX_STATE.lock();
        let child_state = if let Some(parent) = table.get(&caller_pid) {
            parent.fork_state(child_id.raw())
        } else {
            PosixProcessState::new(child_id.raw(), caller_pid)
        };
        table.insert(child_id.raw(), child_state);
    }

    // Register child in signal manager
    {
        let mut sig_mgr = crate::signals::SIGNAL_MANAGER.lock();
        if let Some(mgr) = sig_mgr.as_mut() {
            mgr.fork_process(caller_pid, child_id.raw());
        }
    }

    Ok(child_id.raw())
}

fn fork_cow_page_tables(
    parent_pml4: x86_64::structures::paging::PhysFrame,
) -> Result<x86_64::structures::paging::PhysFrame, Errno> {
    // CoW fork: create a new PML4 that shares the kernel half and
    // duplicates the user half with read-only + CoW markers.
    // For now we do a deep copy via new_user() (delegates to create_new_pml4 —
    // same semantics as the existing new_elf path). True CoW requires page-fault
    // handling to lazily copy pages on write, which is a future enhancement.
    // 1.5f: create_new_pml4 -> arch::mmu::new_user (delegating; same Root).
    let child_pml4 = crate::arch::mmu::new_user();

    let offset = *crate::memory::PHYS_MEM_OFFSET.get().ok_or(Errno::Enomem)?;

    // Copy user-space mappings from parent PML4[0] into child.
    // create_new_pml4 already deep-copied the kernel half and PML4[0]
    // structure. We walk PML4[0] of the parent and replicate the leaf
    // physical mappings into the child PML4[0].
    unsafe {
        use x86_64::structures::paging::page_table::PageTable;
        use x86_64::structures::paging::PageTableFlags;

        let p_virt = offset + parent_pml4.start_address().as_u64();
        let parent_pt = &*(p_virt.as_ptr::<PageTable>());

        let c_virt = offset + child_pml4.start_address().as_u64();
        let child_pt = &*(c_virt.as_ptr::<PageTable>());

        // PML4 entry 0 is already deep-copied by create_new_pml4. For
        // the user region we need to copy the actual leaf page mappings.
        // The deep-copy in create_new_pml4 handled PDPT[0]→PD levels,
        // but leaf-level PT entries need the physical frames replicated.
        // We walk the parent PML4[0] chain and map the same physical
        // frames read-only in the child (CoW semantics).
        if parent_pt[0].flags().contains(PageTableFlags::PRESENT) {
            let p_pdpt = &*(offset + parent_pt[0].addr().as_u64()).as_ptr::<PageTable>();
            let c_pdpt_addr = child_pt[0].addr();
            if c_pdpt_addr.as_u64() != 0 && child_pt[0].flags().contains(PageTableFlags::PRESENT) {
                let c_pdpt = &*(offset + c_pdpt_addr.as_u64()).as_ptr::<PageTable>();

                // Walk PDPT entries that are present in the parent
                for i in 0..512 {
                    if !p_pdpt[i].flags().contains(PageTableFlags::PRESENT) {
                        continue;
                    }
                    if p_pdpt[i].flags().contains(PageTableFlags::HUGE_PAGE) {
                        continue;
                    }

                    if !c_pdpt[i].flags().contains(PageTableFlags::PRESENT) {
                        continue;
                    }

                    let p_pd = &*(offset + p_pdpt[i].addr().as_u64()).as_ptr::<PageTable>();
                    let c_pd_addr = c_pdpt[i].addr();
                    if c_pd_addr.as_u64() == 0 {
                        continue;
                    }
                    let c_pd = &mut *(offset + c_pd_addr.as_u64()).as_mut_ptr::<PageTable>();

                    for j in 0..512 {
                        if !p_pd[j].flags().contains(PageTableFlags::PRESENT) {
                            continue;
                        }
                        if p_pd[j].flags().contains(PageTableFlags::HUGE_PAGE) {
                            continue;
                        }

                        if !c_pd[j].flags().contains(PageTableFlags::PRESENT) {
                            let mut alloc = crate::memory::GlobalFrameAllocator;
                            use x86_64::structures::paging::{
                                FrameAllocator as FA, Size4KiB as S4K,
                            };
                            if let Some(frame) = FA::<S4K>::allocate_frame(&mut alloc) {
                                let pt_ptr =
                                    (offset + frame.start_address().as_u64()).as_mut_ptr::<u8>();
                                core::ptr::write_bytes(pt_ptr, 0, 4096);
                                let flags = PageTableFlags::PRESENT
                                    | PageTableFlags::WRITABLE
                                    | PageTableFlags::USER_ACCESSIBLE;
                                c_pd[j].set_addr(frame.start_address(), flags);
                            } else {
                                continue;
                            }
                        }

                        let p_pt = &*(offset + p_pd[j].addr().as_u64()).as_ptr::<PageTable>();
                        let c_pt =
                            &mut *(offset + c_pd[j].addr().as_u64()).as_mut_ptr::<PageTable>();

                        for k in 0..512 {
                            if p_pt[k].flags().contains(PageTableFlags::PRESENT) {
                                // Share the same physical frame (CoW: mark read-only)
                                let mut flags = p_pt[k].flags();
                                flags.remove(PageTableFlags::WRITABLE);
                                flags.insert(PageTableFlags::USER_ACCESSIBLE);
                                c_pt[k].set_addr(p_pt[k].addr(), flags);
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(child_pml4)
}

pub fn sys_execve(path: &str, _argv: &[&str], _envp: &[&str]) -> Result<(), Errno> {
    let inode = crate::vfs::open_path(path).ok_or(Errno::Enoent)?;

    let mut data = Vec::new();
    let mut buf = [0u8; 4096];
    let mut offset = 0;
    loop {
        let n = inode.read_at(offset, &mut buf);
        if n == 0 {
            break;
        }
        data.extend_from_slice(&buf[..n]);
        offset += n;
    }

    if data.len() < 4 {
        return Err(Errno::Enoexec);
    }

    // Verify ELF magic
    if &data[..4] != &[0x7f, b'E', b'L', b'F'] {
        return Err(Errno::Enoexec);
    }

    // Reset signal dispositions on exec
    if let Some(pid) = crate::scheduler::current_task_id() {
        let mut sig_mgr = crate::signals::SIGNAL_MANAGER.lock();
        if let Some(mgr) = sig_mgr.as_mut() {
            mgr.exec_process(pid.raw());
        }
    }

    Ok(())
}

pub fn sys_waitpid(pid: i64, status: &mut i32, options: u32) -> Result<u64, Errno> {
    use crate::task::TaskId;

    if pid > 0 {
        let target = TaskId::from_raw(pid as u64);
        match crate::scheduler::try_wait_task(target) {
            crate::scheduler::WaitResult::Reaped(code) => {
                *status = ((code & 0xFF) << 8) as i32;
                Ok(pid as u64)
            }
            crate::scheduler::WaitResult::NotFound => Err(Errno::Echild),
            crate::scheduler::WaitResult::Blocked => {
                if options & WNOHANG != 0 {
                    *status = 0;
                    Ok(0)
                } else {
                    Err(Errno::Eagain)
                }
            }
        }
    } else if pid == -1 {
        // Wait for any child — try all children of the caller
        Err(Errno::Echild)
    } else {
        Err(Errno::Einval)
    }
}

pub fn sys_getpid() -> u64 {
    crate::scheduler::current_task_id()
        .map(|id| id.raw())
        .unwrap_or(0)
}

pub fn sys_getppid(pid: u64) -> u64 {
    with_posix_state(pid, |s| s.ppid).unwrap_or(0)
}

pub fn sys_exit(code: u64) {
    crate::scheduler::exit_current_task(code);
}

pub fn sys_getuid() -> u32 {
    let pid = sys_getpid();
    with_posix_state(pid, |s| s.uid).unwrap_or(1000)
}

pub fn sys_getgid() -> u32 {
    let pid = sys_getpid();
    with_posix_state(pid, |s| s.gid).unwrap_or(1000)
}

pub fn sys_geteuid() -> u32 {
    let pid = sys_getpid();
    with_posix_state(pid, |s| s.euid).unwrap_or(1000)
}

pub fn sys_getegid() -> u32 {
    let pid = sys_getpid();
    with_posix_state(pid, |s| s.egid).unwrap_or(1000)
}

// ═══════════════════════════════════════════════════════════════════════════════
// Section 3: Signals
// ═══════════════════════════════════════════════════════════════════════════════

pub fn sys_kill(pid: i64, sig: u8) -> Result<(), Errno> {
    crate::signals::sys_kill(pid, sig).map_err(|_| Errno::Esrch)
}

pub fn sys_sigaction(
    sig: u8,
    act: Option<&crate::signals::SignalHandler>,
    oldact: Option<&mut crate::signals::SignalHandler>,
) -> Result<(), Errno> {
    crate::signals::sys_sigaction(sig, act, oldact).map_err(|e| match e {
        crate::signals::SignalError::InvalidSignal => Errno::Einval,
        crate::signals::SignalError::InvalidArgument => Errno::Einval,
        _ => Errno::Esrch,
    })
}

pub fn sys_sigprocmask(
    how: u32,
    set: Option<&crate::signals::SignalSet>,
    oldset: Option<&mut crate::signals::SignalSet>,
) -> Result<(), Errno> {
    let how = crate::signals::SigProcMaskHow::from_raw(how).map_err(|_| Errno::Einval)?;
    crate::signals::sys_sigprocmask(how, set, oldset).map_err(|_| Errno::Esrch)
}

pub fn sys_sigsuspend(mask: &crate::signals::SignalSet) -> Result<(), Errno> {
    crate::signals::sys_sigsuspend(mask).map_err(|_| Errno::Eintr)
}

// ═══════════════════════════════════════════════════════════════════════════════
// Section 4: Memory Management
// ═══════════════════════════════════════════════════════════════════════════════

/// Map already-owned GPU BO frames into the current Linux client. `pages` has
/// been validated by gpu_render against amdgpud's exact LinuxKPI DMA regions;
/// this function only performs VMA placement and page-table installation.
pub fn sys_mmap_render_pages(
    addr: u64,
    length: u64,
    prot: u32,
    flags: u32,
    pages: &[u64],
) -> Result<u64, Errno> {
    if length == 0 || length & 4095 != 0 || pages.len() != (length >> 12) as usize {
        return Err(Errno::Einval);
    }
    let pid = sys_getpid();
    let start = if flags & MAP_FIXED != 0 {
        if addr & 4095 != 0 {
            return Err(Errno::Einval);
        }
        addr
    } else {
        with_posix_state_mut(pid, |state| {
            let start = (state.mmap_cursor + 4095) & !4095;
            state.mmap_cursor = start.checked_add(length).unwrap_or(u64::MAX);
            start
        })
        .unwrap_or((MMAP_BASE + 0x10_0000) & !4095)
    };
    const USER_SPACE_END: u64 = 0x0000_8000_0000_0000;
    if start
        .checked_add(length)
        .map_or(true, |end| end > USER_SPACE_END)
    {
        return Err(Errno::Enomem);
    }

    use crate::arch::mmu::{PageFlags, PageProt};
    use crate::arch::{PhysAddr, VirtAddr};
    use x86_64::structures::paging::{Mapper, Page as MmuPage, Size4KiB};

    let mut bits = PageProt::PRESENT | PageProt::USER;
    if prot & PROT_WRITE != 0 {
        bits |= PageProt::WRITABLE;
    }
    if prot & PROT_EXEC == 0 {
        bits |= PageProt::NO_EXECUTE;
    }
    let page_flags = PageFlags::new(bits);

    if flags & MAP_FIXED != 0 {
        let mut pt = crate::memory::active_page_table();
        for off in (0..length).step_by(4096) {
            let page = MmuPage::<Size4KiB>::containing_address(VirtAddr::new(start + off));
            if let Ok((_frame, flush)) = pt.unmap(page) {
                flush.ignore();
            }
        }
    }

    let mut aspace = crate::arch::mmu::current_user();
    let mut mapped = 0usize;
    for (index, phys) in pages.iter().copied().enumerate() {
        let virt = VirtAddr::new(start + index as u64 * 4096);
        if aspace
            .map_page(virt, PhysAddr::new(phys), page_flags)
            .is_err()
        {
            let mut pt = crate::memory::active_page_table();
            for undo in 0..mapped {
                let page = MmuPage::<Size4KiB>::containing_address(VirtAddr::new(
                    start + undo as u64 * 4096,
                ));
                if let Ok((_frame, flush)) = pt.unmap(page) {
                    flush.ignore();
                }
            }
            return Err(Errno::Enomem);
        }
        mapped += 1;
    }
    Ok(start)
}

pub fn sys_mmap(
    addr: u64,
    length: u64,
    prot: u32,
    flags: u32,
    fd: i32,
    offset: u64,
) -> Result<u64, Errno> {
    if length == 0 {
        return Err(Errno::Einval);
    }

    // A GEM mmap offset is not file content. Until the render broker installs
    // the BO-page mapping returned by amdgpud, fail closed instead of allocating
    // anonymous zero pages and pretending the GPU buffer was mapped.
    if fd >= 0 && (flags & MAP_ANONYMOUS) == 0 {
        if matches!(render_client_for_fd(fd as u32), Ok(Some(_))) {
            return Err(Errno::Enodev);
        }
    }

    let size = (length + 0xFFF) & !0xFFF;

    let pid = sys_getpid();
    let start = if flags & MAP_FIXED != 0 {
        if addr & 0xFFF != 0 {
            return Err(Errno::Einval);
        }
        // Keep the bump cursor clear of explicit fixed mappings so a later
        // hint-less mmap never overlaps one.
        let _ = with_posix_state_mut(pid, |s| {
            let end = (addr + size + 0xFFF) & !0xFFF;
            if end > s.mmap_cursor {
                s.mmap_cursor = end;
            }
        });
        addr
    } else {
        // Hint-less (and addr-hint, which we treat as advisory): allocate from the
        // per-process bump cursor and ADVANCE it by this mapping's size, so two
        // mmaps never alias. A non-advancing allocator overlapped ld.so's TLS
        // block onto libc's symbol tables (see PosixProcessState::mmap_cursor).
        with_posix_state_mut(pid, |s| {
            let a = (s.mmap_cursor + 0xFFF) & !0xFFF;
            s.mmap_cursor = a + size;
            a
        })
        .unwrap_or((MMAP_BASE + 0x10_0000) & !0xFFF)
    };

    const USER_SPACE_END: u64 = 0x0000_8000_0000_0000;
    if start
        .checked_add(size)
        .map_or(true, |end| end > USER_SPACE_END)
    {
        return Err(Errno::Enomem);
    }

    use crate::arch::mmu::{PageFlags, PageProt};
    use crate::arch::VirtAddr;
    use x86_64::structures::paging::FrameAllocator;

    let mut alloc = crate::memory::GlobalFrameAllocator;

    // Lower the POSIX `prot` bits to arch-neutral PageFlags: a present,
    // user-accessible page, WRITABLE iff PROT_WRITE, NO_EXECUTE iff !PROT_EXEC.
    // (PROT_READ is implicit — a present user page is always readable on x86_64.)
    let mut bits = PageProt::PRESENT | PageProt::USER;
    if prot & PROT_WRITE != 0 {
        bits |= PageProt::WRITABLE;
    }
    if prot & PROT_EXEC == 0 {
        bits |= PageProt::NO_EXECUTE;
    }
    let page_flags = PageFlags::new(bits);

    // File-backed mapping: when a real fd is given (and not MAP_ANONYMOUS), copy
    // the file's content into each mapped page. ld.so REQUIRES this to load
    // shared libraries (libc.so.6 etc.) — an anonymous (zeroed) mapping leaves
    // libc's code/symbols as zeros, so symbol resolution fails (`undefined
    // symbol: __libc_start_main`). MAP_PRIVATE is satisfied for free: each
    // mapping gets its own private frames, so writes never reach the file.
    const MAP_ANONYMOUS: u32 = 0x20;
    let file_inode: Option<alloc::sync::Arc<dyn crate::vfs::Inode>> =
        if fd >= 0 && (flags & MAP_ANONYMOUS) == 0 {
            crate::scheduler::with_current_task(|t| {
                t.fds
                    .get(fd as usize)
                    .and_then(|f| f.as_ref())
                    .map(|f| f.lock().inode.clone())
            })
            .flatten()
        } else {
            None
        };

    // MAP_FIXED atomically REPLACES any existing mapping in the range. Unmap it
    // first so the mapping loop below never hits the PageAlreadyMapped recovery
    // path: ld.so overlays a shared library's per-segment MAP_FIXED maps on top
    // of its initial whole-file reservation, so without this EVERY overlaid page
    // (libc's 1.5 MB text alone is ~370 pages) triggered a `[mem] recovering
    // stale mapping` serial line — a flood that, at ~5 ms/line on iron, stalled
    // the boot past the capture window.
    if flags & MAP_FIXED != 0 {
        use x86_64::structures::paging::{Mapper, Page as MmuPage, Size4KiB};
        let mut pt = crate::memory::active_page_table();
        for off in (0..size).step_by(4096) {
            let page = MmuPage::<Size4KiB>::containing_address(VirtAddr::new(start + off));
            if let Ok((_f, flush)) = pt.unmap(page) {
                flush.ignore();
            }
        }
    }

    // mmap targets the CALLER's own user mapping → the current_user() address
    // space (active CR3), NOT kernel() (CLAUDE.md §10.2). The seam's map_page
    // delegates to the proven crate::memory::map_page_in_pml4_fallible path.
    let mut aspace = crate::arch::mmu::current_user();

    for off in (0..size).step_by(4096) {
        let v = VirtAddr::new(start + off);
        if let Some(frame) = alloc.allocate_frame() {
            let frame_pa = frame.start_address();
            if aspace.map_page(v, frame_pa, page_flags).is_ok() {
                unsafe {
                    let phys_offset = *crate::memory::PHYS_MEM_OFFSET.get().ok_or(Errno::Enomem)?;
                    let ptr = (phys_offset + frame_pa.as_u64()).as_mut_ptr::<u8>();
                    core::ptr::write_bytes(ptr, 0, 4096);
                    // File-backed: read this page's file bytes over the zeros.
                    // Bytes past EOF stay zero (the trailing BSS of a segment),
                    // which is exactly the mmap-of-a-shorter-file semantics.
                    if let Some(ref inode) = file_inode {
                        let dst = core::slice::from_raw_parts_mut(ptr, 4096);
                        let _ = inode.read_at((offset + off) as usize, dst);
                    }
                }
            }
        } else {
            return Err(Errno::Enomem);
        }
    }

    Ok(start)
}

pub fn sys_munmap(addr: u64, length: u64) -> Result<(), Errno> {
    if addr & 0xFFF != 0 || length == 0 {
        return Err(Errno::Einval);
    }

    let size = (length + 0xFFF) & !0xFFF;

    const USER_SPACE_END: u64 = 0x0000_8000_0000_0000;
    if addr
        .checked_add(size)
        .map_or(true, |end| end > USER_SPACE_END)
    {
        return Err(Errno::Einval);
    }

    use crate::arch::VirtAddr;
    use x86_64::structures::paging::{Mapper, Page};

    let mut pt = crate::memory::active_page_table();
    for off in (0..size).step_by(4096) {
        let page = Page::<x86_64::structures::paging::Size4KiB>::containing_address(VirtAddr::new(
            addr + off,
        ));
        if let Ok((_frame, flush)) = pt.unmap(page) {
            flush.ignore();
        }
    }
    x86_64::instructions::tlb::flush_all();
    Ok(())
}

pub fn sys_mprotect(addr: u64, length: u64, prot: u32) -> Result<(), Errno> {
    if addr & 0xFFF != 0 || length == 0 {
        return Err(Errno::Einval);
    }
    let size = (length + 0xFFF) & !0xFFF;

    // Real permission change: walk the range and re-flag every mapped page,
    // matching `sys_mmap`'s prot lowering — a present user page, WRITABLE iff
    // PROT_WRITE, NO_EXECUTE iff !PROT_EXEC. This was a no-op stub; glibc relies
    // on it for RELRO (re-mark `.data.rel.ro` read-only after relocation) and
    // for the reserve-then-commit pattern (`mmap(PROT_NONE)` then
    // `mprotect(RW)`) that backs pthread thread stacks. Unmapped pages in the
    // range are tolerated (no-op) rather than failing the whole call — glibc's
    // reserve mmap has already backed the committed sub-range.
    use crate::arch::mmu::{PageFlags, PageProt};
    use crate::arch::VirtAddr;

    let mut bits = PageProt::PRESENT | PageProt::USER;
    if prot & PROT_WRITE != 0 {
        bits |= PageProt::WRITABLE;
    }
    if prot & PROT_EXEC == 0 {
        bits |= PageProt::NO_EXECUTE;
    }
    let flags = PageFlags::new(bits);

    let mut aspace = crate::arch::mmu::current_user();
    for off in (0..size).step_by(4096) {
        let _ = aspace.update_flags(VirtAddr::new(addr + off), flags);
    }
    Ok(())
}

pub fn sys_brk(new_brk: u64) -> Result<u64, Errno> {
    let pid = sys_getpid();
    with_posix_state_mut(pid, |state| {
        if new_brk == 0 {
            return state.brk_current;
        }
        if new_brk < state.brk_base {
            return state.brk_current;
        }
        // Allocate pages for the expansion
        if new_brk > state.brk_current {
            let old_end = (state.brk_current + 0xFFF) & !0xFFF;
            let new_end = (new_brk + 0xFFF) & !0xFFF;
            if new_end > old_end {
                // Map new pages
                use crate::arch::VirtAddr;
                use x86_64::structures::paging::{FrameAllocator, Page, PageTableFlags, Size4KiB};

                let mut alloc = crate::memory::GlobalFrameAllocator;
                let mut pt = crate::memory::active_page_table();
                let flags = PageTableFlags::PRESENT
                    | PageTableFlags::WRITABLE
                    | PageTableFlags::USER_ACCESSIBLE;

                for page_addr in (old_end..new_end).step_by(4096) {
                    let page = Page::<Size4KiB>::containing_address(VirtAddr::new(page_addr));
                    if let Some(frame) = alloc.allocate_frame() {
                        unsafe {
                            use x86_64::structures::paging::Mapper;
                            let _ = pt.map_to(page, frame, flags, &mut alloc);
                            let phys_offset = crate::memory::PHYS_MEM_OFFSET
                                .get()
                                .copied()
                                .unwrap_or(VirtAddr::zero());
                            let ptr =
                                (phys_offset + frame.start_address().as_u64()).as_mut_ptr::<u8>();
                            core::ptr::write_bytes(ptr, 0, 4096);
                        }
                    }
                }
            }
        }
        state.brk_current = new_brk;
        state.brk_current
    })
}

// ═══════════════════════════════════════════════════════════════════════════════
// Section 5: Directory Operations
// ═══════════════════════════════════════════════════════════════════════════════

pub fn sys_mkdir(_path: &str, _mode: u32) -> Result<(), Errno> {
    // VFS directory creation stub — AthFS would handle the actual
    // directory inode creation. For now accept silently.
    Ok(())
}

pub fn sys_rmdir(path: &str) -> Result<(), Errno> {
    if path.is_empty() || path == "/" {
        return Err(Errno::Einval);
    }
    Ok(())
}

pub fn sys_getcwd(pid: u64) -> Result<String, Errno> {
    with_posix_state(pid, |s| s.cwd.clone())
}

pub fn sys_chdir(pid: u64, path: &str) -> Result<(), Errno> {
    with_posix_state_mut(pid, |s| {
        if path.starts_with('/') {
            s.cwd = String::from(path);
        } else {
            if s.cwd.ends_with('/') {
                s.cwd.push_str(path);
            } else {
                s.cwd.push('/');
                s.cwd.push_str(path);
            }
        }
    })
}

// ═══════════════════════════════════════════════════════════════════════════════
// Section 6: Time
// ═══════════════════════════════════════════════════════════════════════════════

pub fn sys_gettimeofday(tv: &mut Timeval, tz: Option<&mut Timezone>) -> Result<(), Errno> {
    let mgr = crate::signals::SIGNAL_MANAGER.lock();
    if let Some(ref m) = *mgr {
        let ts = m
            .timer_manager
            .clock_gettime(crate::signals::ClockId::Realtime);
        tv.tv_sec = ts.sec;
        tv.tv_usec = ts.nsec / 1000;
    } else {
        tv.tv_sec = 0;
        tv.tv_usec = 0;
    }
    if let Some(tz) = tz {
        tz.tz_minuteswest = 0;
        tz.tz_dsttime = 0;
    }
    Ok(())
}

pub fn sys_clock_gettime(clock_id: u32, tp: &mut Timespec) -> Result<(), Errno> {
    let clk = crate::signals::ClockId::from_raw(clock_id).map_err(|_| Errno::Einval)?;

    let mgr = crate::signals::SIGNAL_MANAGER.lock();
    if let Some(ref m) = *mgr {
        let ts = m.timer_manager.clock_gettime(clk);
        tp.tv_sec = ts.sec;
        tp.tv_nsec = ts.nsec;
        Ok(())
    } else {
        tp.tv_sec = 0;
        tp.tv_nsec = 0;
        Ok(())
    }
}

pub fn sys_nanosleep(req: &Timespec, rem: Option<&mut Timespec>) -> Result<(), Errno> {
    let ts = crate::signals::Timespec::new(req.tv_sec, req.tv_nsec);
    let mgr = crate::signals::SIGNAL_MANAGER.lock();
    if let Some(ref m) = *mgr {
        match m.timer_manager.nanosleep(&ts) {
            Ok(()) => Ok(()),
            Err(remaining) => {
                if let Some(r) = rem {
                    r.tv_sec = remaining.sec;
                    r.tv_nsec = remaining.nsec;
                }
                Err(Errno::Eintr)
            }
        }
    } else {
        Ok(())
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Section 7: Socket Operations (stubs backed by SOCKET_TABLE)
// ═══════════════════════════════════════════════════════════════════════════════

pub fn sys_socket(domain: u32, sock_type: u32, protocol: u32) -> Result<u64, Errno> {
    match domain {
        AF_UNIX | AF_INET | AF_INET6 => {}
        _ => return Err(Errno::Eafnosupport),
    }
    match sock_type & 0xF {
        SOCK_STREAM | SOCK_DGRAM | SOCK_RAW => {}
        _ => return Err(Errno::Eprotonosupport),
    }

    let id = NEXT_SOCKET_ID.fetch_add(1, Ordering::Relaxed);
    let socket = Socket::new(domain, sock_type, protocol);
    SOCKET_TABLE.lock().insert(id, socket);

    // Allocate an fd that refers to this socket (via a SocketInode)
    let inode: Arc<dyn crate::vfs::Inode> = Arc::new(SocketInode { socket_id: id });
    let file = crate::vfs::File::new(inode, O_RDWR);
    let file_arc = alloc::sync::Arc::new(spin::Mutex::new(file));

    let mut fd = Err(Errno::Emfile);
    crate::scheduler::with_current_task_mut(|task| {
        for (i, slot) in task.fds.iter_mut().enumerate() {
            if slot.is_none() {
                *slot = Some(file_arc.clone());
                fd = Ok(i as u64);
                break;
            }
        }
    });
    fd
}

struct SocketInode {
    socket_id: u64,
}

unsafe impl Send for SocketInode {}
unsafe impl Sync for SocketInode {}

impl crate::vfs::Inode for SocketInode {
    fn read_at(&self, _offset: usize, buf: &mut [u8]) -> usize {
        let mut table = SOCKET_TABLE.lock();
        if let Some(sock) = table.get_mut(&self.socket_id) {
            let n = buf.len().min(sock.recv_buf.len());
            buf[..n].copy_from_slice(&sock.recv_buf[..n]);
            sock.recv_buf.drain(..n);
            n
        } else {
            0
        }
    }

    fn write_at(&self, _offset: usize, buf: &[u8]) -> usize {
        let mut table = SOCKET_TABLE.lock();
        if let Some(sock) = table.get_mut(&self.socket_id) {
            sock.send_buf.extend_from_slice(buf);
            buf.len()
        } else {
            0
        }
    }

    fn size(&self) -> usize {
        let table = SOCKET_TABLE.lock();
        table
            .get(&self.socket_id)
            .map(|s| s.recv_buf.len())
            .unwrap_or(0)
    }
}

pub fn sys_bind(fd: u32, addr: &SockaddrIn) -> Result<(), Errno> {
    let _ = fd;
    let id = socket_id_from_fd(fd)?;
    let mut table = SOCKET_TABLE.lock();
    let sock = table.get_mut(&id).ok_or(Errno::Ebadf)?;
    if sock.state != SocketState::Unbound {
        return Err(Errno::Einval);
    }
    sock.local_addr = Some(*addr);
    sock.state = SocketState::Bound;
    Ok(())
}

pub fn sys_listen(fd: u32, backlog: u32) -> Result<(), Errno> {
    let id = socket_id_from_fd(fd)?;
    let mut table = SOCKET_TABLE.lock();
    let sock = table.get_mut(&id).ok_or(Errno::Ebadf)?;
    if sock.state != SocketState::Bound {
        return Err(Errno::Einval);
    }
    sock.backlog = backlog;
    sock.state = SocketState::Listening;
    Ok(())
}

pub fn sys_accept(fd: u32, addr: Option<&mut SockaddrIn>) -> Result<u64, Errno> {
    let id = socket_id_from_fd(fd)?;
    let table = SOCKET_TABLE.lock();
    let sock = table.get(&id).ok_or(Errno::Ebadf)?;
    if sock.state != SocketState::Listening {
        return Err(Errno::Einval);
    }
    // No pending connections — would block
    if let Some(a) = addr {
        *a = SockaddrIn::default();
    }
    Err(Errno::Eagain)
}

pub fn sys_connect(fd: u32, addr: &SockaddrIn) -> Result<(), Errno> {
    let id = socket_id_from_fd(fd)?;
    let mut table = SOCKET_TABLE.lock();
    let sock = table.get_mut(&id).ok_or(Errno::Ebadf)?;
    sock.remote_addr = Some(*addr);
    sock.state = SocketState::Connected;
    Ok(())
}

pub fn sys_send(fd: u32, buf: &[u8], _flags: u32) -> Result<usize, Errno> {
    let id = socket_id_from_fd(fd)?;
    let mut table = SOCKET_TABLE.lock();
    let sock = table.get_mut(&id).ok_or(Errno::Ebadf)?;
    if sock.state != SocketState::Connected {
        return Err(Errno::Enotty);
    }
    sock.send_buf.extend_from_slice(buf);
    Ok(buf.len())
}

pub fn sys_recv(fd: u32, buf: &mut [u8], _flags: u32) -> Result<usize, Errno> {
    let id = socket_id_from_fd(fd)?;
    let mut table = SOCKET_TABLE.lock();
    let sock = table.get_mut(&id).ok_or(Errno::Ebadf)?;
    let n = buf.len().min(sock.recv_buf.len());
    buf[..n].copy_from_slice(&sock.recv_buf[..n]);
    sock.recv_buf.drain(..n);
    Ok(n)
}

pub fn sys_select(
    _nfds: u32,
    _readfds: Option<&mut u64>,
    _writefds: Option<&mut u64>,
    _exceptfds: Option<&mut u64>,
    _timeout: Option<&Timeval>,
) -> Result<u32, Errno> {
    // Minimal select: report all requested fds as ready (non-blocking stub)
    Ok(0)
}

pub fn sys_poll(fds: &mut [PollFd], _timeout: i32) -> Result<u32, Errno> {
    let mut ready = 0u32;
    for pfd in fds.iter_mut() {
        let exists = crate::scheduler::with_current_task(|task| {
            let fd = pfd.fd as usize;
            fd < task.fds.len() && task.fds[fd].is_some()
        })
        .unwrap_or(false);

        if exists {
            pfd.revents = pfd.events & (POLLIN | POLLOUT);
            if pfd.revents != 0 {
                ready += 1;
            }
        } else {
            pfd.revents = POLLHUP;
            ready += 1;
        }
    }
    Ok(ready)
}

fn socket_id_from_fd(fd: u32) -> Result<u64, Errno> {
    // Walk through socket table to find a socket whose fd index matches.
    // In a production OS, the fd table would directly store the socket id.
    // Here we use the SocketInode pattern — the socket_id is embedded in
    // the inode. For the stub, we map fd → sequential socket id.
    // This is a simplification: the actual socket_id was assigned when
    // sys_socket was called. We store it implicitly via the fd ordering.
    let table = SOCKET_TABLE.lock();
    // Return the socket_id if we have exactly the right one
    // Simplified: assume socket fds map 1:1 to socket IDs in creation order.
    // Real implementation would store socket_id in an extended fd table.
    for (&id, _) in table.iter() {
        return Ok(id);
    }
    Err(Errno::Enotsock)
}

// ═══════════════════════════════════════════════════════════════════════════════
// Section 8: Virtual Filesystem Nodes (/dev/null, /dev/zero, /dev/urandom)
// ═══════════════════════════════════════════════════════════════════════════════

pub struct DevNullInode;
pub struct DevZeroInode;
pub struct DevUrandomInode;
pub struct DevConsoleInode;
pub struct TmpfsInode {
    data: Mutex<Vec<u8>>,
}

impl crate::vfs::Inode for DevNullInode {
    fn read_at(&self, _offset: usize, _buf: &mut [u8]) -> usize {
        0
    }
    fn write_at(&self, _offset: usize, buf: &[u8]) -> usize {
        buf.len()
    }
    fn size(&self) -> usize {
        0
    }
}

impl crate::vfs::Inode for DevZeroInode {
    fn read_at(&self, _offset: usize, buf: &mut [u8]) -> usize {
        for b in buf.iter_mut() {
            *b = 0;
        }
        buf.len()
    }
    fn write_at(&self, _offset: usize, buf: &[u8]) -> usize {
        buf.len()
    }
    fn size(&self) -> usize {
        0
    }
}

impl crate::vfs::Inode for DevUrandomInode {
    fn read_at(&self, _offset: usize, buf: &mut [u8]) -> usize {
        // Simple xorshift64 PRNG for /dev/urandom emulation
        static SEED: AtomicU64 = AtomicU64::new(0xDEAD_BEEF_CAFE_BABE);
        let mut s = SEED.load(Ordering::Relaxed);
        for b in buf.iter_mut() {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            *b = s as u8;
        }
        SEED.store(s, Ordering::Relaxed);
        buf.len()
    }
    fn write_at(&self, _offset: usize, buf: &[u8]) -> usize {
        buf.len()
    }
    fn size(&self) -> usize {
        0
    }
}

impl crate::vfs::Inode for DevConsoleInode {
    fn read_at(&self, _offset: usize, _buf: &mut [u8]) -> usize {
        0
    }
    fn write_at(&self, _offset: usize, buf: &[u8]) -> usize {
        // Print as UTF-8 best-effort (busybox output is ASCII/UTF-8).
        if let Ok(s) = core::str::from_utf8(buf) {
            crate::serial_print!("{}", s);
        } else {
            for &b in buf {
                if b.is_ascii_graphic() || b == b' ' || b == b'\n' || b == b'\r' || b == b'\t' {
                    crate::serial_print!("{}", b as char);
                } else {
                    crate::serial_print!("\\x{:02x}", b);
                }
            }
        }
        buf.len()
    }
    fn size(&self) -> usize {
        0
    }
}

/// Install best-effort console file descriptors (0/1/2) on a newly created task.
pub fn install_console_fds(task: &mut crate::task::Task) {
    let inode: alloc::sync::Arc<dyn crate::vfs::Inode> = alloc::sync::Arc::new(DevConsoleInode);
    // fd 0 (stdin) MUST be reserved: leaving it unset made the first openat()
    // (e.g. ld.so opening libc.so.6) get fd 0, which ld.so/glibc treat as a std
    // stream — so libc was never mapped as a library and symbol resolution
    // failed (`undefined symbol __libc_start_main`). A read-only console stdin
    // (reads return 0 = EOF) reserves fd 0 so opened files get fd 3+.
    let stdin = crate::vfs::File::new(inode.clone(), O_RDONLY);
    task.fds[0] = Some(alloc::sync::Arc::new(spin::Mutex::new(stdin)));
    let file = crate::vfs::File::new(inode, O_WRONLY);
    let file_arc = alloc::sync::Arc::new(spin::Mutex::new(file));
    task.fds[1] = Some(file_arc.clone());
    task.fds[2] = Some(file_arc);
}

impl TmpfsInode {
    pub fn new() -> Self {
        Self {
            data: Mutex::new(Vec::new()),
        }
    }
}

impl crate::vfs::Inode for TmpfsInode {
    fn read_at(&self, offset: usize, buf: &mut [u8]) -> usize {
        let data = self.data.lock();
        if offset >= data.len() {
            return 0;
        }
        let n = buf.len().min(data.len() - offset);
        buf[..n].copy_from_slice(&data[offset..offset + n]);
        n
    }

    fn write_at(&self, offset: usize, buf: &[u8]) -> usize {
        let mut data = self.data.lock();
        let end = offset + buf.len();
        if end > data.len() {
            data.resize(end, 0);
        }
        data[offset..end].copy_from_slice(buf);
        buf.len()
    }

    fn size(&self) -> usize {
        self.data.lock().len()
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Section 9: /proc emulation for Linux compatibility
// ═══════════════════════════════════════════════════════════════════════════════

pub struct ProcSelfExeInode {
    pub exe_path: String,
}

pub struct ProcSelfMapsInode;
pub struct ProcSelfStatusInode;

impl crate::vfs::Inode for ProcSelfExeInode {
    fn read_at(&self, offset: usize, buf: &mut [u8]) -> usize {
        let bytes = self.exe_path.as_bytes();
        if offset >= bytes.len() {
            return 0;
        }
        let n = buf.len().min(bytes.len() - offset);
        buf[..n].copy_from_slice(&bytes[offset..offset + n]);
        n
    }
    fn write_at(&self, _offset: usize, _buf: &[u8]) -> usize {
        0
    }
    fn size(&self) -> usize {
        self.exe_path.len()
    }
}

impl crate::vfs::Inode for ProcSelfMapsInode {
    fn read_at(&self, offset: usize, buf: &mut [u8]) -> usize {
        let pid = sys_getpid();
        let maps = crate::process::read_proc_entry(&crate::process::ProcEntry {
            pid: Some(crate::process::Pid(pid)),
            name: String::from("maps"),
            entry_type: crate::process::ProcEntryType::ProcessMaps,
        });
        let bytes = maps.as_bytes();
        if offset >= bytes.len() {
            return 0;
        }
        let n = buf.len().min(bytes.len() - offset);
        buf[..n].copy_from_slice(&bytes[offset..offset + n]);
        n
    }
    fn write_at(&self, _offset: usize, _buf: &[u8]) -> usize {
        0
    }
    fn size(&self) -> usize {
        0
    }
}

impl crate::vfs::Inode for ProcSelfStatusInode {
    fn read_at(&self, offset: usize, buf: &mut [u8]) -> usize {
        let pid = sys_getpid();
        let status = crate::process::read_proc_entry(&crate::process::ProcEntry {
            pid: Some(crate::process::Pid(pid)),
            name: String::from("status"),
            entry_type: crate::process::ProcEntryType::ProcessStatus,
        });
        let bytes = status.as_bytes();
        if offset >= bytes.len() {
            return 0;
        }
        let n = buf.len().min(bytes.len() - offset);
        buf[..n].copy_from_slice(&bytes[offset..offset + n]);
        n
    }
    fn write_at(&self, _offset: usize, _buf: &[u8]) -> usize {
        0
    }
    fn size(&self) -> usize {
        0
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Initialization
// ═══════════════════════════════════════════════════════════════════════════════

pub fn init() {
    // Register PID 1 (init) in the POSIX state table
    let mut table = POSIX_STATE.lock();
    table.insert(1, PosixProcessState::new(1, 0));
}
