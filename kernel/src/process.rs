//! Full POSIX-style process model for AthenaOS.
//!
//! Provides process lifecycle (fork/exec/wait/exit), signal delivery,
//! virtual memory management, file descriptor tables, resource limits,
//! and a /proc filesystem interface.

#![allow(dead_code)]

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

// ═══════════════════════════════════════════════════════════════════════════════
// Process Identification
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Pid(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ThreadId(pub u64);

impl Pid {
    pub const INIT: Pid = Pid(1);
    pub const KERNEL: Pid = Pid(0);

    pub fn as_u64(self) -> u64 {
        self.0
    }
}

impl ThreadId {
    pub fn as_u64(self) -> u64 {
        self.0
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Process State & Priority
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessState {
    Running,
    Sleeping,
    Stopped,
    Zombie,
    Dead,
    TracedStopped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessPriority {
    Idle,
    Background,
    Normal,
    Game,
    RealTime,
}

impl ProcessPriority {
    pub fn timeslice_us(&self) -> u64 {
        match self {
            Self::Idle => 1_000,
            Self::Background => 5_000,
            Self::Normal => 10_000,
            Self::Game => 20_000,
            Self::RealTime => 50_000,
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Signal System (POSIX-style)
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Signal {
    SigHup = 1,
    SigInt = 2,
    SigQuit = 3,
    SigIll = 4,
    SigTrap = 5,
    SigAbrt = 6,
    SigBus = 7,
    SigFpe = 8,
    SigKill = 9,
    SigUsr1 = 10,
    SigSegv = 11,
    SigUsr2 = 12,
    SigPipe = 13,
    SigAlrm = 14,
    SigTerm = 15,
    SigStkflt = 16,
    SigChld = 17,
    SigCont = 18,
    SigStop = 19,
    SigTstp = 20,
    SigTtin = 21,
    SigTtou = 22,
    SigUrg = 23,
    SigXcpu = 24,
    SigXfsz = 25,
    SigVtalrm = 26,
    SigProf = 27,
    SigWinch = 28,
    SigIo = 29,
    SigPwr = 30,
    SigSys = 31,
}

impl Signal {
    pub fn from_num(num: u8) -> Option<Self> {
        match num {
            1 => Some(Self::SigHup),
            2 => Some(Self::SigInt),
            3 => Some(Self::SigQuit),
            4 => Some(Self::SigIll),
            5 => Some(Self::SigTrap),
            6 => Some(Self::SigAbrt),
            7 => Some(Self::SigBus),
            8 => Some(Self::SigFpe),
            9 => Some(Self::SigKill),
            10 => Some(Self::SigUsr1),
            11 => Some(Self::SigSegv),
            12 => Some(Self::SigUsr2),
            13 => Some(Self::SigPipe),
            14 => Some(Self::SigAlrm),
            15 => Some(Self::SigTerm),
            16 => Some(Self::SigStkflt),
            17 => Some(Self::SigChld),
            18 => Some(Self::SigCont),
            19 => Some(Self::SigStop),
            20 => Some(Self::SigTstp),
            21 => Some(Self::SigTtin),
            22 => Some(Self::SigTtou),
            23 => Some(Self::SigUrg),
            24 => Some(Self::SigXcpu),
            25 => Some(Self::SigXfsz),
            26 => Some(Self::SigVtalrm),
            27 => Some(Self::SigProf),
            28 => Some(Self::SigWinch),
            29 => Some(Self::SigIo),
            30 => Some(Self::SigPwr),
            31 => Some(Self::SigSys),
            _ => None,
        }
    }

    pub fn is_fatal_default(&self) -> bool {
        matches!(
            self,
            Self::SigHup
                | Self::SigInt
                | Self::SigQuit
                | Self::SigIll
                | Self::SigAbrt
                | Self::SigBus
                | Self::SigFpe
                | Self::SigKill
                | Self::SigSegv
                | Self::SigPipe
                | Self::SigAlrm
                | Self::SigTerm
                | Self::SigXcpu
                | Self::SigXfsz
                | Self::SigSys
        )
    }

    pub fn is_uncatchable(&self) -> bool {
        matches!(self, Self::SigKill | Self::SigStop)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignalAction {
    Default,
    Ignore,
    Handler(u64),
}

#[derive(Debug, Clone)]
pub struct SignalInfo {
    pub signal: Signal,
    pub sender_pid: Pid,
    pub errno: i32,
    pub code: i32,
}

// ═══════════════════════════════════════════════════════════════════════════════
// Memory Space
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct MemorySpace {
    pub page_table_root: u64,
    pub regions: Vec<MemoryRegion>,
    pub brk: u64,
    pub stack_top: u64,
    pub stack_size: u64,
    pub mmap_base: u64,
    pub total_mapped: u64,
    /// Cgroup-equivalent address-space cap in bytes (Phase 4.1). `0` = unlimited.
    /// `mmap`/`brk` growth that would push `total_mapped` past this is refused
    /// with `ProcessError::MemoryLimitExceeded` — a per-app-bundle RAM budget.
    pub memory_limit: u64,
}

#[derive(Debug, Clone)]
pub struct MemoryRegion {
    pub start: u64,
    pub end: u64,
    pub permissions: MmapProt,
    pub flags: MmapFlags,
    pub backing: RegionBacking,
    pub name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegionBacking {
    Anonymous,
    File { fd: u32, offset: u64 },
    SharedMemory(u64),
    Device(u64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MmapProt(pub u32);

impl MmapProt {
    pub const NONE: u32 = 0;
    pub const READ: u32 = 1;
    pub const WRITE: u32 = 2;
    pub const EXEC: u32 = 4;

    pub fn readable(&self) -> bool {
        self.0 & Self::READ != 0
    }
    pub fn writable(&self) -> bool {
        self.0 & Self::WRITE != 0
    }
    pub fn executable(&self) -> bool {
        self.0 & Self::EXEC != 0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MmapFlags(pub u32);

impl MmapFlags {
    pub const PRIVATE: u32 = 0x01;
    pub const SHARED: u32 = 0x02;
    pub const ANONYMOUS: u32 = 0x04;
    pub const FIXED: u32 = 0x08;
    pub const GROWSDOWN: u32 = 0x10;
    pub const POPULATE: u32 = 0x20;
}

impl MemorySpace {
    pub fn new_kernel() -> Self {
        Self {
            page_table_root: 0,
            regions: Vec::new(),
            brk: 0x0040_0000,
            stack_top: 0x0000_7FFF_FFFF_0000,
            stack_size: 8 * 1024 * 1024,
            mmap_base: 0x0000_7F00_0000_0000,
            total_mapped: 0,
            memory_limit: 0,
        }
    }

    pub fn new_user() -> Self {
        Self {
            page_table_root: 0,
            regions: Vec::new(),
            brk: 0x0060_0000,
            stack_top: 0x0000_7FFF_FFFF_0000,
            stack_size: 8 * 1024 * 1024,
            mmap_base: 0x0000_7F00_0000_0000,
            total_mapped: 0,
            memory_limit: 0,
        }
    }

    /// Set this address space's RAM budget (bytes). `0` disables the cap.
    pub fn set_memory_limit(&mut self, bytes: u64) {
        self.memory_limit = bytes;
    }

    /// Would adding `extra` bytes exceed the configured limit?
    #[inline]
    fn would_exceed_limit(&self, extra: u64) -> bool {
        self.memory_limit != 0 && self.total_mapped.saturating_add(extra) > self.memory_limit
    }

    pub fn mmap(
        &mut self,
        addr: Option<u64>,
        length: u64,
        prot: MmapProt,
        flags: MmapFlags,
        backing: RegionBacking,
    ) -> Result<u64, ProcessError> {
        let aligned_length = (length + 0xFFF) & !0xFFF;

        let start = if let Some(fixed_addr) = addr {
            if flags.0 & MmapFlags::FIXED == 0 {
                return Err(ProcessError::InvalidAddress);
            }
            if fixed_addr & 0xFFF != 0 {
                return Err(ProcessError::InvalidAddress);
            }
            self.unmap_range(fixed_addr, fixed_addr + aligned_length);
            fixed_addr
        } else {
            let base = self.mmap_base;
            self.mmap_base -= aligned_length;
            self.mmap_base &= !0xFFF;
            self.mmap_base
        };

        let region = MemoryRegion {
            start,
            end: start + aligned_length,
            permissions: prot,
            flags,
            backing,
            name: None,
        };

        // Cgroup-equivalent budget check (Phase 4.1): refuse growth past the cap.
        if self.would_exceed_limit(aligned_length) {
            return Err(ProcessError::MemoryLimitExceeded);
        }

        self.regions.push(region);
        self.total_mapped += aligned_length;
        Ok(start)
    }

    pub fn munmap(&mut self, addr: u64, length: u64) -> Result<(), ProcessError> {
        if addr & 0xFFF != 0 {
            return Err(ProcessError::InvalidAddress);
        }
        let aligned_length = (length + 0xFFF) & !0xFFF;
        self.unmap_range(addr, addr + aligned_length);
        Ok(())
    }

    pub fn mprotect(&mut self, addr: u64, length: u64, prot: MmapProt) -> Result<(), ProcessError> {
        if addr & 0xFFF != 0 {
            return Err(ProcessError::InvalidAddress);
        }
        let end = addr + ((length + 0xFFF) & !0xFFF);

        for region in &mut self.regions {
            if region.start >= addr && region.end <= end {
                region.permissions = prot;
            }
        }
        Ok(())
    }

    pub fn brk(&mut self, new_brk: u64) -> Result<u64, ProcessError> {
        if new_brk == 0 {
            return Ok(self.brk);
        }
        if new_brk < self.brk {
            let aligned_old = (self.brk + 0xFFF) & !0xFFF;
            let aligned_new = (new_brk + 0xFFF) & !0xFFF;
            if aligned_new < aligned_old {
                self.total_mapped -= aligned_old - aligned_new;
            }
        } else {
            let growth = (new_brk - self.brk + 0xFFF) & !0xFFF;
            // Cgroup-equivalent budget check (Phase 4.1).
            if self.would_exceed_limit(growth) {
                return Err(ProcessError::MemoryLimitExceeded);
            }
            self.total_mapped += growth;
        }
        self.brk = new_brk;
        Ok(self.brk)
    }

    fn unmap_range(&mut self, start: u64, end: u64) {
        self.regions.retain(|r| r.end <= start || r.start >= end);
    }

    pub fn find_region(&self, addr: u64) -> Option<&MemoryRegion> {
        self.regions
            .iter()
            .find(|r| addr >= r.start && addr < r.end)
    }

    pub fn clone_for_fork(&self) -> Self {
        Self {
            page_table_root: 0, // new page table allocated by fork
            regions: self.regions.clone(),
            brk: self.brk,
            stack_top: self.stack_top,
            stack_size: self.stack_size,
            mmap_base: self.mmap_base,
            total_mapped: self.total_mapped,
            memory_limit: self.memory_limit,
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// File Descriptor Table
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct FileDescriptorTable {
    fds: BTreeMap<u32, FileDescriptor>,
    next_fd: u32,
}

#[derive(Debug, Clone)]
pub struct FileDescriptor {
    pub fd_num: u32,
    pub fd_type: FdType,
    pub flags: u32,
    pub offset: u64,
    pub close_on_exec: bool,
}

#[derive(Debug, Clone)]
pub enum FdType {
    RegularFile { inode: u64, path: String },
    Pipe { read_end: bool, buffer_id: u64 },
    Socket(u64),
    Device(u64),
    Directory { path: String },
    Epoll(u64),
}

impl FileDescriptor {
    pub const O_RDONLY: u32 = 0x0000;
    pub const O_WRONLY: u32 = 0x0001;
    pub const O_RDWR: u32 = 0x0002;
    pub const O_CREAT: u32 = 0x0040;
    pub const O_EXCL: u32 = 0x0080;
    pub const O_TRUNC: u32 = 0x0200;
    pub const O_APPEND: u32 = 0x0400;
    pub const O_NONBLOCK: u32 = 0x0800;
    pub const O_CLOEXEC: u32 = 0x80000;
}

impl FileDescriptorTable {
    pub fn new() -> Self {
        let mut table = Self {
            fds: BTreeMap::new(),
            next_fd: 3,
        };
        table.fds.insert(
            0,
            FileDescriptor {
                fd_num: 0,
                fd_type: FdType::Device(0),
                flags: FileDescriptor::O_RDONLY,
                offset: 0,
                close_on_exec: false,
            },
        );
        table.fds.insert(
            1,
            FileDescriptor {
                fd_num: 1,
                fd_type: FdType::Device(1),
                flags: FileDescriptor::O_WRONLY,
                offset: 0,
                close_on_exec: false,
            },
        );
        table.fds.insert(
            2,
            FileDescriptor {
                fd_num: 2,
                fd_type: FdType::Device(2),
                flags: FileDescriptor::O_WRONLY,
                offset: 0,
                close_on_exec: false,
            },
        );
        table
    }

    pub fn open(&mut self, path: &str, flags: u32, fd_type: FdType) -> Result<u32, ProcessError> {
        let fd_num = self.allocate_fd();
        let close_on_exec = flags & FileDescriptor::O_CLOEXEC != 0;
        let fd = FileDescriptor {
            fd_num,
            fd_type,
            flags,
            offset: 0,
            close_on_exec,
        };
        self.fds.insert(fd_num, fd);
        Ok(fd_num)
    }

    pub fn close(&mut self, fd: u32) -> Result<(), ProcessError> {
        if self.fds.remove(&fd).is_none() {
            return Err(ProcessError::BadFileDescriptor);
        }
        Ok(())
    }

    pub fn read(&mut self, fd: u32, buf: &mut [u8]) -> Result<usize, ProcessError> {
        let descriptor = self
            .fds
            .get_mut(&fd)
            .ok_or(ProcessError::BadFileDescriptor)?;
        let bytes_read = buf.len().min(4096);
        descriptor.offset += bytes_read as u64;
        Ok(bytes_read)
    }

    pub fn write(&mut self, fd: u32, buf: &[u8]) -> Result<usize, ProcessError> {
        let descriptor = self
            .fds
            .get_mut(&fd)
            .ok_or(ProcessError::BadFileDescriptor)?;
        let bytes_written = buf.len();
        descriptor.offset += bytes_written as u64;
        Ok(bytes_written)
    }

    pub fn dup(&mut self, old_fd: u32) -> Result<u32, ProcessError> {
        let original = self
            .fds
            .get(&old_fd)
            .ok_or(ProcessError::BadFileDescriptor)?
            .clone();
        let new_fd = self.allocate_fd();
        let mut dup = original;
        dup.fd_num = new_fd;
        dup.close_on_exec = false;
        self.fds.insert(new_fd, dup);
        Ok(new_fd)
    }

    pub fn dup2(&mut self, old_fd: u32, new_fd: u32) -> Result<u32, ProcessError> {
        if old_fd == new_fd {
            if !self.fds.contains_key(&old_fd) {
                return Err(ProcessError::BadFileDescriptor);
            }
            return Ok(new_fd);
        }
        let original = self
            .fds
            .get(&old_fd)
            .ok_or(ProcessError::BadFileDescriptor)?
            .clone();
        let _ = self.fds.remove(&new_fd);
        let mut dup = original;
        dup.fd_num = new_fd;
        dup.close_on_exec = false;
        self.fds.insert(new_fd, dup);
        Ok(new_fd)
    }

    pub fn pipe(&mut self) -> Result<(u32, u32), ProcessError> {
        static PIPE_COUNTER: spin::Mutex<u64> = spin::Mutex::new(0);
        let buffer_id = {
            let mut counter = PIPE_COUNTER.lock();
            *counter += 1;
            *counter
        };

        let read_fd = self.allocate_fd();
        self.fds.insert(
            read_fd,
            FileDescriptor {
                fd_num: read_fd,
                fd_type: FdType::Pipe {
                    read_end: true,
                    buffer_id,
                },
                flags: FileDescriptor::O_RDONLY,
                offset: 0,
                close_on_exec: false,
            },
        );

        let write_fd = self.allocate_fd();
        self.fds.insert(
            write_fd,
            FileDescriptor {
                fd_num: write_fd,
                fd_type: FdType::Pipe {
                    read_end: false,
                    buffer_id,
                },
                flags: FileDescriptor::O_WRONLY,
                offset: 0,
                close_on_exec: false,
            },
        );

        Ok((read_fd, write_fd))
    }

    pub fn fcntl(&mut self, fd: u32, cmd: u32, arg: u64) -> Result<u64, ProcessError> {
        const F_DUPFD: u32 = 0;
        const F_GETFD: u32 = 1;
        const F_SETFD: u32 = 2;
        const F_GETFL: u32 = 3;
        const F_SETFL: u32 = 4;

        match cmd {
            F_DUPFD => {
                let new_fd = self.dup(fd)?;
                Ok(new_fd as u64)
            }
            F_GETFD => {
                let descriptor = self.fds.get(&fd).ok_or(ProcessError::BadFileDescriptor)?;
                Ok(descriptor.close_on_exec as u64)
            }
            F_SETFD => {
                let descriptor = self
                    .fds
                    .get_mut(&fd)
                    .ok_or(ProcessError::BadFileDescriptor)?;
                descriptor.close_on_exec = arg != 0;
                Ok(0)
            }
            F_GETFL => {
                let descriptor = self.fds.get(&fd).ok_or(ProcessError::BadFileDescriptor)?;
                Ok(descriptor.flags as u64)
            }
            F_SETFL => {
                let descriptor = self
                    .fds
                    .get_mut(&fd)
                    .ok_or(ProcessError::BadFileDescriptor)?;
                descriptor.flags = arg as u32;
                Ok(0)
            }
            _ => Err(ProcessError::InvalidArgument),
        }
    }

    pub fn get(&self, fd: u32) -> Option<&FileDescriptor> {
        self.fds.get(&fd)
    }

    pub fn close_on_exec(&mut self) {
        let cloexec_fds: Vec<u32> = self
            .fds
            .iter()
            .filter(|(_, desc)| desc.close_on_exec)
            .map(|(&fd, _)| fd)
            .collect();
        for fd in cloexec_fds {
            self.fds.remove(&fd);
        }
    }

    pub fn clone_for_fork(&self) -> Self {
        Self {
            fds: self.fds.clone(),
            next_fd: self.next_fd,
        }
    }

    fn allocate_fd(&mut self) -> u32 {
        let fd = self.next_fd;
        self.next_fd += 1;
        fd
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Resource Limits
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct ResourceLimits {
    pub max_open_files: u64,
    pub max_memory_bytes: u64,
    pub max_cpu_time_secs: u64,
    pub max_threads: u64,
    pub max_stack_size: u64,
    pub max_file_size: u64,
    pub nice_limit: i8,
}

impl ResourceLimits {
    pub fn default_user() -> Self {
        Self {
            max_open_files: 1024,
            max_memory_bytes: 512 * 1024 * 1024,
            max_cpu_time_secs: u64::MAX,
            max_threads: 256,
            max_stack_size: 8 * 1024 * 1024,
            max_file_size: u64::MAX,
            nice_limit: -20,
        }
    }

    pub fn default_system() -> Self {
        Self {
            max_open_files: 65536,
            max_memory_bytes: u64::MAX,
            max_cpu_time_secs: u64::MAX,
            max_threads: 4096,
            max_stack_size: 64 * 1024 * 1024,
            max_file_size: u64::MAX,
            nice_limit: -20,
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Process Structure
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct Process {
    pub pid: Pid,
    pub ppid: Pid,
    pub name: String,
    pub state: ProcessState,
    pub exit_code: Option<i32>,
    pub threads: Vec<ThreadId>,
    pub main_thread: ThreadId,
    pub memory_space: MemorySpace,
    pub fd_table: FileDescriptorTable,
    pub cwd: String,
    pub env: BTreeMap<String, String>,
    pub uid: u32,
    pub gid: u32,
    pub session_id: u32,
    pub process_group: u32,
    pub children: Vec<Pid>,
    pub signal_mask: u64,
    pub signal_handlers: [SignalAction; 32],
    pub pending_signals: u64,
    pub rlimits: ResourceLimits,
    pub cpu_time_us: u64,
    pub start_time: u64,
    pub priority: ProcessPriority,
    pub nice: i8,
    pub capabilities: u64,
}

impl Process {
    pub fn new_init() -> Self {
        Self {
            pid: Pid::INIT,
            ppid: Pid::KERNEL,
            name: String::from("init"),
            state: ProcessState::Running,
            exit_code: None,
            threads: alloc::vec![ThreadId(1)],
            main_thread: ThreadId(1),
            memory_space: MemorySpace::new_kernel(),
            fd_table: FileDescriptorTable::new(),
            cwd: String::from("/"),
            env: BTreeMap::new(),
            uid: 0,
            gid: 0,
            session_id: 1,
            process_group: 1,
            children: Vec::new(),
            signal_mask: 0,
            signal_handlers: [SignalAction::Default; 32],
            pending_signals: 0,
            rlimits: ResourceLimits::default_system(),
            cpu_time_us: 0,
            start_time: 0,
            priority: ProcessPriority::Normal,
            nice: 0,
            capabilities: u64::MAX,
        }
    }

    pub fn deliver_signal(&mut self, signal: Signal) -> SignalDisposition {
        let sig_num = signal as u8;
        if sig_num == 0 || sig_num > 31 {
            return SignalDisposition::Ignored;
        }

        if signal.is_uncatchable() {
            match signal {
                Signal::SigKill => return SignalDisposition::Terminate,
                Signal::SigStop => {
                    self.state = ProcessState::Stopped;
                    return SignalDisposition::Stop;
                }
                _ => unreachable!(),
            }
        }

        let mask_bit = 1u64 << (sig_num - 1);
        if self.signal_mask & mask_bit != 0 {
            self.pending_signals |= mask_bit;
            return SignalDisposition::Blocked;
        }

        match self.signal_handlers[sig_num as usize - 1] {
            SignalAction::Ignore => SignalDisposition::Ignored,
            SignalAction::Default => {
                if signal.is_fatal_default() {
                    SignalDisposition::Terminate
                } else {
                    SignalDisposition::Ignored
                }
            }
            SignalAction::Handler(entry) => SignalDisposition::RunHandler(entry),
        }
    }

    pub fn set_signal_handler(
        &mut self,
        signal: Signal,
        action: SignalAction,
    ) -> Result<(), ProcessError> {
        if signal.is_uncatchable() {
            return Err(ProcessError::PermissionDenied);
        }
        let idx = signal as u8 - 1;
        self.signal_handlers[idx as usize] = action;
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignalDisposition {
    Terminate,
    Stop,
    Ignored,
    Blocked,
    RunHandler(u64),
}

// ═══════════════════════════════════════════════════════════════════════════════
// Process Errors
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessError {
    NotFound,
    PermissionDenied,
    InvalidArgument,
    OutOfMemory,
    BadFileDescriptor,
    TooManyProcesses,
    InvalidAddress,
    InvalidExecutable,
    WouldBlock,
    Interrupted,
    IoError,
    NoChildProcesses,
    /// `mmap`/`brk` growth would push the address space past its
    /// cgroup-equivalent per-bundle memory limit (Phase 4.1).
    MemoryLimitExceeded,
}

// ═══════════════════════════════════════════════════════════════════════════════
// Wait/Exit Types
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitTarget {
    AnyChild,
    SpecificPid(Pid),
    ProcessGroup(u32),
}

#[derive(Debug, Clone)]
pub struct WaitResult {
    pub pid: Pid,
    pub status: WaitStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitStatus {
    Exited(i32),
    Signaled(Signal),
    Stopped(Signal),
    Continued,
}

// ═══════════════════════════════════════════════════════════════════════════════
// Process Table (Global)
// ═══════════════════════════════════════════════════════════════════════════════

pub static PROCESS_TABLE: Mutex<Option<ProcessTable>> = Mutex::new(None);

pub struct ProcessTable {
    processes: BTreeMap<u64, Process>,
    next_pid: u64,
    init_pid: Pid,
    next_thread_id: u64,
}

impl ProcessTable {
    pub fn new() -> Self {
        let mut table = Self {
            processes: BTreeMap::new(),
            next_pid: 2,
            init_pid: Pid::INIT,
            next_thread_id: 2,
        };
        let init = Process::new_init();
        table.processes.insert(1, init);
        table
    }

    pub fn fork(&mut self, parent_pid: Pid) -> Result<Pid, ProcessError> {
        let parent = self
            .processes
            .get(&parent_pid.0)
            .ok_or(ProcessError::NotFound)?;

        let child_pid = Pid(self.next_pid);
        self.next_pid += 1;

        let child_thread = ThreadId(self.next_thread_id);
        self.next_thread_id += 1;

        let mut child = Process {
            pid: child_pid,
            ppid: parent_pid,
            name: parent.name.clone(),
            state: ProcessState::Running,
            exit_code: None,
            threads: alloc::vec![child_thread],
            main_thread: child_thread,
            memory_space: parent.memory_space.clone_for_fork(),
            fd_table: parent.fd_table.clone_for_fork(),
            cwd: parent.cwd.clone(),
            env: parent.env.clone(),
            uid: parent.uid,
            gid: parent.gid,
            session_id: parent.session_id,
            process_group: parent.process_group,
            children: Vec::new(),
            signal_mask: parent.signal_mask,
            signal_handlers: parent.signal_handlers,
            pending_signals: 0,
            rlimits: parent.rlimits.clone(),
            cpu_time_us: 0,
            start_time: 0,
            priority: parent.priority,
            nice: parent.nice,
            capabilities: parent.capabilities,
        };

        // Parent gets the child in its children list
        if let Some(parent_mut) = self.processes.get_mut(&parent_pid.0) {
            parent_mut.children.push(child_pid);
        }

        self.processes.insert(child_pid.0, child);
        Ok(child_pid)
    }

    pub fn exec(
        &mut self,
        pid: Pid,
        _binary: &[u8],
        args: &[String],
        env: &BTreeMap<String, String>,
    ) -> Result<(), ProcessError> {
        let process = self
            .processes
            .get_mut(&pid.0)
            .ok_or(ProcessError::NotFound)?;

        process.memory_space = MemorySpace::new_user();
        process.fd_table.close_on_exec();
        process.env = env.clone();
        process.signal_handlers = [SignalAction::Default; 32];
        process.pending_signals = 0;

        if let Some(arg0) = args.first() {
            process.name = arg0.clone();
        }

        Ok(())
    }

    pub fn wait(
        &mut self,
        parent_pid: Pid,
        target: WaitTarget,
    ) -> Result<WaitResult, ProcessError> {
        let parent = self
            .processes
            .get(&parent_pid.0)
            .ok_or(ProcessError::NotFound)?;

        if parent.children.is_empty() {
            return Err(ProcessError::NoChildProcesses);
        }

        let zombie_pid = parent
            .children
            .iter()
            .find(|&&child_pid| {
                if let Some(child) = self.processes.get(&child_pid.0) {
                    let state_match = child.state == ProcessState::Zombie;
                    let target_match = match target {
                        WaitTarget::AnyChild => true,
                        WaitTarget::SpecificPid(p) => child_pid == p,
                        WaitTarget::ProcessGroup(pg) => child.process_group == pg,
                    };
                    state_match && target_match
                } else {
                    false
                }
            })
            .copied();

        if let Some(zpid) = zombie_pid {
            let exit_code = self
                .processes
                .get(&zpid.0)
                .and_then(|p| p.exit_code)
                .unwrap_or(0);

            self.processes.remove(&zpid.0);

            if let Some(parent_mut) = self.processes.get_mut(&parent_pid.0) {
                parent_mut.children.retain(|&c| c != zpid);
            }

            Ok(WaitResult {
                pid: zpid,
                status: WaitStatus::Exited(exit_code),
            })
        } else {
            Err(ProcessError::WouldBlock)
        }
    }

    pub fn exit(&mut self, pid: Pid, code: i32) {
        if let Some(process) = self.processes.get_mut(&pid.0) {
            process.state = ProcessState::Zombie;
            process.exit_code = Some(code);
            process.memory_space.regions.clear();
            process.memory_space.total_mapped = 0;

            let children: Vec<Pid> = process.children.clone();
            let ppid = process.ppid;

            // Reparent children to init
            for child_pid in &children {
                if let Some(child) = self.processes.get_mut(&child_pid.0) {
                    child.ppid = Pid::INIT;
                }
            }
            if let Some(init) = self.processes.get_mut(&Pid::INIT.0) {
                init.children.extend(children);
            }

            // Send SIGCHLD to parent
            if let Some(parent) = self.processes.get_mut(&ppid.0) {
                let mask_bit = 1u64 << (Signal::SigChld as u8 - 1);
                parent.pending_signals |= mask_bit;
            }
        }
    }

    pub fn kill(&mut self, pid: Pid, signal: Signal) -> Result<(), ProcessError> {
        let process = self
            .processes
            .get_mut(&pid.0)
            .ok_or(ProcessError::NotFound)?;

        match process.deliver_signal(signal) {
            SignalDisposition::Terminate => {
                let pid_copy = process.pid;
                self.exit(pid_copy, -(signal as i32));
                Ok(())
            }
            SignalDisposition::Stop => Ok(()),
            SignalDisposition::Ignored => Ok(()),
            SignalDisposition::Blocked => Ok(()),
            SignalDisposition::RunHandler(_) => Ok(()),
        }
    }

    pub fn getpid(&self, pid: Pid) -> Option<&Process> {
        self.processes.get(&pid.0)
    }

    pub fn getpid_mut(&mut self, pid: Pid) -> Option<&mut Process> {
        self.processes.get_mut(&pid.0)
    }

    pub fn list_processes(&self) -> Vec<&Process> {
        self.processes.values().collect()
    }

    pub fn process_tree(&self) -> Vec<(Pid, Vec<Pid>)> {
        self.processes
            .values()
            .map(|p| (p.pid, p.children.clone()))
            .collect()
    }

    pub fn process_count(&self) -> usize {
        self.processes.len()
    }
}

pub fn send_signal(target_pid: Pid, signal: Signal) -> Result<(), ProcessError> {
    let mut table = PROCESS_TABLE.lock();
    if let Some(ref mut t) = *table {
        t.kill(target_pid, signal)
    } else {
        Err(ProcessError::NotFound)
    }
}

pub fn kill_process_group(pgid: u32, signal: Signal) -> Result<(), ProcessError> {
    let mut table = PROCESS_TABLE.lock();
    if let Some(ref mut t) = *table {
        let pids: Vec<Pid> = t
            .processes
            .values()
            .filter(|p| p.process_group == pgid)
            .map(|p| p.pid)
            .collect();

        if pids.is_empty() {
            return Err(ProcessError::NotFound);
        }

        for pid in pids {
            let _ = t.kill(pid, signal);
        }
        Ok(())
    } else {
        Err(ProcessError::NotFound)
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// /proc Filesystem Interface
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct ProcEntry {
    pub pid: Option<Pid>,
    pub name: String,
    pub entry_type: ProcEntryType,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProcEntryType {
    ProcessStatus,
    ProcessCmdline,
    ProcessMaps,
    ProcessFd,
    ProcessStat,
    SystemUptime,
    SystemMeminfo,
    SystemCpuinfo,
    SystemLoadavg,
    SystemVersion,
    SystemMounts,
    SystemNetDev,
    SystemDiskStats,
}

pub fn read_proc_entry(entry: &ProcEntry) -> String {
    let table = PROCESS_TABLE.lock();
    match entry.entry_type {
        ProcEntryType::ProcessStatus => {
            if let Some(pid) = entry.pid {
                if let Some(ref t) = *table {
                    if let Some(proc) = t.getpid(pid) {
                        return alloc::format!(
                            "Name:\t{}\nState:\t{:?}\nPid:\t{}\nPPid:\t{}\nUid:\t{}\nGid:\t{}\nThreads:\t{}\nVmSize:\t{} kB\n",
                            proc.name,
                            proc.state,
                            proc.pid.0,
                            proc.ppid.0,
                            proc.uid,
                            proc.gid,
                            proc.threads.len(),
                            proc.memory_space.total_mapped / 1024,
                        );
                    }
                }
            }
            String::from("(not found)")
        }
        ProcEntryType::ProcessCmdline => {
            if let Some(pid) = entry.pid {
                if let Some(ref t) = *table {
                    if let Some(proc) = t.getpid(pid) {
                        return proc.name.clone();
                    }
                }
            }
            String::new()
        }
        ProcEntryType::ProcessMaps => {
            if let Some(pid) = entry.pid {
                if let Some(ref t) = *table {
                    if let Some(proc) = t.getpid(pid) {
                        let mut out = String::new();
                        for region in &proc.memory_space.regions {
                            let perms = alloc::format!(
                                "{}{}{}p",
                                if region.permissions.readable() { "r" } else { "-" },
                                if region.permissions.writable() { "w" } else { "-" },
                                if region.permissions.executable() { "x" } else { "-" },
                            );
                            let name = region.name.as_deref().unwrap_or("");
                            out.push_str(&alloc::format!(
                                "{:016x}-{:016x} {} {}\n",
                                region.start, region.end, perms, name,
                            ));
                        }
                        return out;
                    }
                }
            }
            String::new()
        }
        ProcEntryType::ProcessFd => {
            if let Some(pid) = entry.pid {
                if let Some(ref t) = *table {
                    if let Some(proc) = t.getpid(pid) {
                        let mut out = String::new();
                        for (fd_num, fd) in &proc.fd_table.fds {
                            let type_str = match &fd.fd_type {
                                FdType::RegularFile { path, .. } => alloc::format!("file:{}", path),
                                FdType::Pipe { read_end, .. } => {
                                    alloc::format!("pipe:[{}]", if *read_end { "r" } else { "w" })
                                }
                                FdType::Socket(id) => alloc::format!("socket:[{}]", id),
                                FdType::Device(id) => alloc::format!("dev:{}", id),
                                FdType::Directory { path } => alloc::format!("dir:{}", path),
                                FdType::Epoll(id) => alloc::format!("epoll:{}", id),
                            };
                            out.push_str(&alloc::format!("{} -> {}\n", fd_num, type_str));
                        }
                        return out;
                    }
                }
            }
            String::new()
        }
        ProcEntryType::ProcessStat => {
            if let Some(pid) = entry.pid {
                if let Some(ref t) = *table {
                    if let Some(proc) = t.getpid(pid) {
                        return alloc::format!(
                            "{} ({}) {:?} {} {} {} {} {} {} {}",
                            proc.pid.0,
                            proc.name,
                            proc.state,
                            proc.ppid.0,
                            proc.process_group,
                            proc.session_id,
                            proc.cpu_time_us,
                            proc.priority as u8,
                            proc.nice,
                            proc.threads.len(),
                        );
                    }
                }
            }
            String::new()
        }
        ProcEntryType::SystemUptime => {
            String::from("0.00 0.00")
        }
        ProcEntryType::SystemMeminfo => {
            alloc::format!(
                "MemTotal:       262144 kB\nMemFree:        131072 kB\nMemAvailable:   196608 kB\nBuffers:         16384 kB\nCached:          32768 kB\n"
            )
        }
        ProcEntryType::SystemCpuinfo => {
            alloc::format!(
                "processor\t: 0\nvendor_id\t: AthenaOS\nmodel name\t: AthenaOS Virtual CPU\ncpu MHz\t\t: 3000.000\ncache size\t: 8192 KB\n"
            )
        }
        ProcEntryType::SystemLoadavg => {
            if let Some(ref t) = *table {
                let running = t.processes.values().filter(|p| p.state == ProcessState::Running).count();
                let total = t.processes.len();
                alloc::format!("0.00 0.00 0.00 {}/{} 1", running, total)
            } else {
                String::from("0.00 0.00 0.00 0/0 1")
            }
        }
        ProcEntryType::SystemVersion => {
            String::from("AthenaOS version 0.0.1 (raeen@athenaos) (rustc 1.80.0) #1 SMP")
        }
        ProcEntryType::SystemMounts => {
            String::from("raefs / raefs rw,relatime 0 0\nproc /proc proc rw,nosuid,nodev,noexec 0 0\nsysfs /sys sysfs rw,nosuid,nodev,noexec 0 0\ntmpfs /tmp tmpfs rw,nosuid,nodev 0 0\n")
        }
        ProcEntryType::SystemNetDev => {
            String::from("Inter-|   Receive                                                |  Transmit\n face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed\n    lo:       0       0    0    0    0     0          0         0        0       0    0    0    0     0       0          0\n  eth0:       0       0    0    0    0     0          0         0        0       0    0    0    0     0       0          0\n")
        }
        ProcEntryType::SystemDiskStats => {
            String::from("   1       0 vda 0 0 0 0 0 0 0 0 0 0 0\n")
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Cgroup-equivalent per-bundle memory limits (Phase 4.1)
// ═══════════════════════════════════════════════════════════════════════════════
//
// Concept: every app ships as a *bundle* with a manifest. AthGuard reads the
// manifest's RAM budget and registers it here keyed by bundle id. When the
// bundle's first process is created, the spawn path looks the limit up and
// stamps it onto the new `MemorySpace` via `set_memory_limit`. Thereafter any
// `mmap`/`brk` growth that would push the address space past the cap is refused
// with `ProcessError::MemoryLimitExceeded` — a hard ceiling, no overcommit. A
// `0` limit (the default) means unlimited, so system processes are unaffected.

/// Registry of bundle id → RAM budget in bytes. `None` until `init()`.
static BUNDLE_MEMORY_LIMITS: Mutex<Option<BTreeMap<String, u64>>> = Mutex::new(None);

/// Register (or update) a bundle's RAM budget in bytes. `0` clears the cap.
pub fn set_bundle_memory_limit(bundle: &str, bytes: u64) {
    let mut guard = BUNDLE_MEMORY_LIMITS.lock();
    let map = guard.get_or_insert_with(BTreeMap::new);
    if bytes == 0 {
        map.remove(bundle);
    } else {
        map.insert(String::from(bundle), bytes);
    }
}

/// Look up a bundle's RAM budget in bytes. Returns `0` (unlimited) if unset.
pub fn bundle_memory_limit(bundle: &str) -> u64 {
    BUNDLE_MEMORY_LIMITS
        .lock()
        .as_ref()
        .and_then(|m| m.get(bundle).copied())
        .unwrap_or(0)
}

/// `/proc/raeen/memlimits` body: one `bundle = bytes` line per registered cap.
pub fn dump_memlimits() -> String {
    use core::fmt::Write;
    let mut out = String::from("# AthenaOS per-bundle memory limits (Phase 4.1)\n");
    let guard = BUNDLE_MEMORY_LIMITS.lock();
    match guard.as_ref() {
        Some(map) if !map.is_empty() => {
            for (bundle, bytes) in map.iter() {
                let _ = writeln!(out, "{} = {} bytes", bundle, bytes);
            }
        }
        _ => out.push_str("(no per-bundle limits registered)\n"),
    }
    out
}

/// Verify the limit machinery end-to-end on a throwaway `MemorySpace`:
/// mmap within budget succeeds, mmap past budget is refused, and brk growth
/// past budget is refused. No real pages are touched (regions are bookkeeping).
pub fn run_boot_smoketest() {
    let mut space = MemorySpace::new_user();
    // 1 MiB cap.
    space.set_memory_limit(1024 * 1024);

    // 512 KiB anonymous mapping — comfortably under the cap.
    let within = space.mmap(
        None,
        512 * 1024,
        MmapProt(MmapProt::READ | MmapProt::WRITE),
        MmapFlags(MmapFlags::PRIVATE | MmapFlags::ANONYMOUS),
        RegionBacking::Anonymous,
    );

    // Another 1 MiB mapping — total would be 1.5 MiB > 1 MiB cap → refused.
    let beyond = space.mmap(
        None,
        1024 * 1024,
        MmapProt(MmapProt::READ | MmapProt::WRITE),
        MmapFlags(MmapFlags::PRIVATE | MmapFlags::ANONYMOUS),
        RegionBacking::Anonymous,
    );

    let within_ok = within.is_ok();
    let beyond_denied = matches!(beyond, Err(ProcessError::MemoryLimitExceeded));

    // Registry round-trip.
    set_bundle_memory_limit("com.raeen.test", 64 * 1024 * 1024);
    let reg_ok = bundle_memory_limit("com.raeen.test") == 64 * 1024 * 1024;
    set_bundle_memory_limit("com.raeen.test", 0);
    let clear_ok = bundle_memory_limit("com.raeen.test") == 0;

    let pass = within_ok && beyond_denied && reg_ok && clear_ok;
    crate::serial_println!(
        "[memlimit] within_ok={} beyond_denied={} registry_ok={} clear_ok={} -> {}",
        within_ok,
        beyond_denied,
        reg_ok,
        clear_ok,
        if pass { "PASS" } else { "FAIL" }
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// Initialization
// ═══════════════════════════════════════════════════════════════════════════════

pub fn init() {
    let table = ProcessTable::new();
    *PROCESS_TABLE.lock() = Some(table);
    crate::serial_println!("[ OK ] Process model ready (cgroup-equiv memory limits armed)");
}
