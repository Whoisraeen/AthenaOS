//! Event Loop + Async Runtime — Kernel-space async event infrastructure
//!
//! Implements epoll, poll, select, eventfd, timerfd, signalfd, pidfd,
//! userfaultfd, splice/tee/sendfile, wait queues, and completions.

#![allow(dead_code)]

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::collections::VecDeque;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use spin::Mutex;

// ─── Epoll Flags ─────────────────────────────────────────────────────────────

pub const EPOLLIN: u32 = 0x001;
pub const EPOLLOUT: u32 = 0x004;
pub const EPOLLRDHUP: u32 = 0x2000;
pub const EPOLLPRI: u32 = 0x002;
pub const EPOLLERR: u32 = 0x008;
pub const EPOLLHUP: u32 = 0x010;
pub const EPOLLET: u32 = 1 << 31;
pub const EPOLLONESHOT: u32 = 1 << 30;
pub const EPOLLWAKEUP: u32 = 1 << 29;
pub const EPOLLEXCLUSIVE: u32 = 1 << 28;

pub const EPOLL_CTL_ADD: u32 = 1;
pub const EPOLL_CTL_DEL: u32 = 2;
pub const EPOLL_CTL_MOD: u32 = 3;

pub const EPOLL_CLOEXEC: u32 = 0o2000000;

// ─── Poll Flags ──────────────────────────────────────────────────────────────

pub const POLLIN: u16 = 0x0001;
pub const POLLOUT: u16 = 0x0004;
pub const POLLERR: u16 = 0x0008;
pub const POLLHUP: u16 = 0x0010;
pub const POLLNVAL: u16 = 0x0020;
pub const POLLPRI: u16 = 0x0002;
pub const POLLRDNORM: u16 = 0x0040;
pub const POLLRDBAND: u16 = 0x0080;
pub const POLLWRNORM: u16 = 0x0100;
pub const POLLWRBAND: u16 = 0x0200;

// ─── Eventfd Flags ───────────────────────────────────────────────────────────

pub const EFD_CLOEXEC: u32 = 0o2000000;
pub const EFD_NONBLOCK: u32 = 0o4000;
pub const EFD_SEMAPHORE: u32 = 0o1;

// ─── Timerfd Flags ───────────────────────────────────────────────────────────

pub const TFD_CLOEXEC: u32 = 0o2000000;
pub const TFD_NONBLOCK: u32 = 0o4000;
pub const TFD_TIMER_ABSTIME: u32 = 1 << 0;

// ─── Signalfd Flags ──────────────────────────────────────────────────────────

pub const SFD_CLOEXEC: u32 = 0o2000000;
pub const SFD_NONBLOCK: u32 = 0o4000;

// ─── Splice Flags ────────────────────────────────────────────────────────────

pub const SPLICE_F_MOVE: u32 = 1;
pub const SPLICE_F_NONBLOCK: u32 = 2;
pub const SPLICE_F_MORE: u32 = 4;
pub const SPLICE_F_GIFT: u32 = 8;

// ─── Wait Queue Flags ────────────────────────────────────────────────────────

pub const WQ_FLAG_EXCLUSIVE: u32 = 0x01;
pub const WQ_FLAG_WOKEN: u32 = 0x02;
pub const WQ_FLAG_BOOKMARK: u32 = 0x04;

// ─── Userfaultfd IOCTLs ──────────────────────────────────────────────────────

pub const UFFDIO_API: u64 = 0xC018AA3F;
pub const UFFDIO_REGISTER: u64 = 0xC020AA00;
pub const UFFDIO_UNREGISTER: u64 = 0x8010AA01;
pub const UFFDIO_COPY: u64 = 0xC028AA03;
pub const UFFDIO_ZEROPAGE: u64 = 0xC020AA04;
pub const UFFDIO_WAKE: u64 = 0x8010AA02;
pub const UFFDIO_WRITEPROTECT: u64 = 0xC018AA06;

pub const UFFD_FEATURE_MISSING: u64 = 1 << 0;
pub const UFFD_FEATURE_WP: u64 = 1 << 1;
pub const UFFD_FEATURE_MINOR: u64 = 1 << 2;

// ─── Pidfd Flags ─────────────────────────────────────────────────────────────

pub const P_PIDFD: u32 = 3;
pub const PIDFD_NONBLOCK: u32 = 0o4000;

// ─── Timespec ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, Default)]
pub struct Timespec {
    pub tv_sec: i64,
    pub tv_nsec: i64,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ItimerSpec {
    pub it_interval: Timespec,
    pub it_value: Timespec,
}

// ═══════════════════════════════════════════════════════════════════════════════
// EPOLL
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy)]
pub struct EpollEvent {
    pub events: u32,
    pub data: u64,
}

#[derive(Debug, Clone)]
struct EpollEntry {
    fd: i32,
    events: u32,
    data: u64,
    edge_triggered: bool,
    oneshot: bool,
    exclusive: bool,
    disabled: bool,
    ready: bool,
}

pub struct EpollInstance {
    id: u32,
    entries: BTreeMap<i32, EpollEntry>,
    ready_list: VecDeque<i32>,
    flags: u32,
    nested_epoll_fds: Vec<i32>,
}

impl EpollInstance {
    pub fn new(id: u32, flags: u32) -> Self {
        Self {
            id,
            entries: BTreeMap::new(),
            ready_list: VecDeque::new(),
            flags,
            nested_epoll_fds: Vec::new(),
        }
    }

    pub fn ctl_add(&mut self, fd: i32, event: &EpollEvent) -> Result<(), EventError> {
        if self.entries.contains_key(&fd) {
            return Err(EventError::AlreadyExists);
        }
        let entry = EpollEntry {
            fd,
            events: event.events & !(EPOLLET | EPOLLONESHOT | EPOLLEXCLUSIVE),
            data: event.data,
            edge_triggered: event.events & EPOLLET != 0,
            oneshot: event.events & EPOLLONESHOT != 0,
            exclusive: event.events & EPOLLEXCLUSIVE != 0,
            disabled: false,
            ready: false,
        };
        self.entries.insert(fd, entry);
        Ok(())
    }

    pub fn ctl_mod(&mut self, fd: i32, event: &EpollEvent) -> Result<(), EventError> {
        let entry = self.entries.get_mut(&fd).ok_or(EventError::NotFound)?;
        entry.events = event.events & !(EPOLLET | EPOLLONESHOT | EPOLLEXCLUSIVE);
        entry.data = event.data;
        entry.edge_triggered = event.events & EPOLLET != 0;
        entry.oneshot = event.events & EPOLLONESHOT != 0;
        entry.disabled = false;
        Ok(())
    }

    pub fn ctl_del(&mut self, fd: i32) -> Result<(), EventError> {
        self.entries.remove(&fd).ok_or(EventError::NotFound)?;
        self.ready_list.retain(|&f| f != fd);
        Ok(())
    }

    pub fn signal_ready(&mut self, fd: i32, events: u32) {
        if let Some(entry) = self.entries.get_mut(&fd) {
            if entry.disabled {
                return;
            }
            if entry.events & events != 0 {
                entry.ready = true;
                if !self.ready_list.contains(&fd) {
                    if entry.exclusive {
                        self.ready_list.push_back(fd);
                    } else {
                        self.ready_list.push_front(fd);
                    }
                }
            }
        }
    }

    pub fn wait(&mut self, max_events: usize) -> Vec<EpollEvent> {
        let mut results = Vec::new();
        let mut to_disable = Vec::new();

        while let Some(fd) = self.ready_list.pop_front() {
            if results.len() >= max_events {
                self.ready_list.push_front(fd);
                break;
            }
            if let Some(entry) = self.entries.get_mut(&fd) {
                if entry.disabled {
                    continue;
                }
                results.push(EpollEvent {
                    events: entry.events,
                    data: entry.data,
                });
                entry.ready = false;
                if entry.oneshot {
                    to_disable.push(fd);
                }
                if entry.edge_triggered {
                    // Edge-triggered: don't re-add to ready list
                }
            }
        }

        for fd in to_disable {
            if let Some(entry) = self.entries.get_mut(&fd) {
                entry.disabled = true;
            }
        }

        results
    }

    pub fn add_nested(&mut self, epoll_fd: i32) {
        if !self.nested_epoll_fds.contains(&epoll_fd) {
            self.nested_epoll_fds.push(epoll_fd);
        }
    }
}

pub struct EpollRegistry {
    instances: BTreeMap<u32, EpollInstance>,
    next_id: AtomicU32,
}

impl EpollRegistry {
    pub const fn new() -> Self {
        Self {
            instances: BTreeMap::new(),
            next_id: AtomicU32::new(1),
        }
    }

    pub fn epoll_create1(&mut self, flags: u32) -> u32 {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.instances.insert(id, EpollInstance::new(id, flags));
        id
    }

    pub fn epoll_ctl(
        &mut self,
        epfd: u32,
        op: u32,
        fd: i32,
        event: Option<&EpollEvent>,
    ) -> Result<(), EventError> {
        let inst = self.instances.get_mut(&epfd).ok_or(EventError::BadFd)?;
        match op {
            EPOLL_CTL_ADD => inst.ctl_add(fd, event.ok_or(EventError::InvalidArgument)?),
            EPOLL_CTL_MOD => inst.ctl_mod(fd, event.ok_or(EventError::InvalidArgument)?),
            EPOLL_CTL_DEL => inst.ctl_del(fd),
            _ => Err(EventError::InvalidArgument),
        }
    }

    pub fn epoll_wait(
        &mut self,
        epfd: u32,
        max_events: usize,
        _timeout_ms: i32,
    ) -> Result<Vec<EpollEvent>, EventError> {
        let inst = self.instances.get_mut(&epfd).ok_or(EventError::BadFd)?;
        Ok(inst.wait(max_events))
    }

    pub fn epoll_pwait(
        &mut self,
        epfd: u32,
        max_events: usize,
        timeout_ms: i32,
        _sigmask: u64,
    ) -> Result<Vec<EpollEvent>, EventError> {
        self.epoll_wait(epfd, max_events, timeout_ms)
    }

    pub fn epoll_pwait2(
        &mut self,
        epfd: u32,
        max_events: usize,
        _timeout: Option<&Timespec>,
        _sigmask: u64,
    ) -> Result<Vec<EpollEvent>, EventError> {
        let inst = self.instances.get_mut(&epfd).ok_or(EventError::BadFd)?;
        Ok(inst.wait(max_events))
    }

    pub fn signal_fd_ready(&mut self, epfd: u32, fd: i32, events: u32) {
        if let Some(inst) = self.instances.get_mut(&epfd) {
            inst.signal_ready(fd, events);
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// POLL
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy)]
pub struct PollFd {
    pub fd: i32,
    pub events: u16,
    pub revents: u16,
}

impl PollFd {
    pub fn new(fd: i32, events: u16) -> Self {
        Self {
            fd,
            events,
            revents: 0,
        }
    }
}

pub struct PollSubsystem;

impl PollSubsystem {
    pub fn poll(fds: &mut [PollFd], _timeout_ms: i32) -> i32 {
        let mut ready_count = 0i32;
        for pfd in fds.iter_mut() {
            pfd.revents = 0;
            if pfd.fd < 0 {
                continue;
            }
            // In a real kernel, we'd check the actual fd state
            // Here we simulate by marking writable fds as ready
            if pfd.events & POLLOUT != 0 {
                pfd.revents |= POLLOUT;
                ready_count += 1;
            }
        }
        ready_count
    }

    pub fn ppoll(fds: &mut [PollFd], timeout: Option<&Timespec>, _sigmask: u64) -> i32 {
        let timeout_ms = timeout.map_or(-1, |t| (t.tv_sec * 1000 + t.tv_nsec / 1_000_000) as i32);
        Self::poll(fds, timeout_ms)
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// SELECT
// ═══════════════════════════════════════════════════════════════════════════════

pub const FD_SETSIZE: usize = 1024;

#[derive(Clone)]
pub struct FdSet {
    bits: [u64; FD_SETSIZE / 64],
}

impl FdSet {
    pub fn new() -> Self {
        Self {
            bits: [0; FD_SETSIZE / 64],
        }
    }

    pub fn zero(&mut self) {
        self.bits = [0; FD_SETSIZE / 64];
    }

    pub fn set(&mut self, fd: usize) {
        if fd < FD_SETSIZE {
            self.bits[fd / 64] |= 1u64 << (fd % 64);
        }
    }

    pub fn clr(&mut self, fd: usize) {
        if fd < FD_SETSIZE {
            self.bits[fd / 64] &= !(1u64 << (fd % 64));
        }
    }

    pub fn isset(&self, fd: usize) -> bool {
        if fd >= FD_SETSIZE {
            return false;
        }
        self.bits[fd / 64] & (1u64 << (fd % 64)) != 0
    }

    pub fn count_set(&self) -> usize {
        self.bits.iter().map(|b| b.count_ones() as usize).sum()
    }
}

pub struct SelectSubsystem;

impl SelectSubsystem {
    pub fn select(
        nfds: usize,
        readfds: Option<&mut FdSet>,
        writefds: Option<&mut FdSet>,
        exceptfds: Option<&mut FdSet>,
        _timeout: Option<&Timespec>,
    ) -> i32 {
        let mut ready = 0i32;

        if let Some(wfds) = writefds {
            for fd in 0..nfds.min(FD_SETSIZE) {
                if wfds.isset(fd) {
                    ready += 1;
                }
            }
        }

        if let Some(rfds) = readfds {
            for fd in 0..nfds.min(FD_SETSIZE) {
                if rfds.isset(fd) {
                    rfds.clr(fd);
                }
            }
        }

        if let Some(efds) = exceptfds {
            efds.zero();
        }

        ready
    }

    pub fn pselect6(
        nfds: usize,
        readfds: Option<&mut FdSet>,
        writefds: Option<&mut FdSet>,
        exceptfds: Option<&mut FdSet>,
        timeout: Option<&Timespec>,
        _sigmask: u64,
    ) -> i32 {
        Self::select(nfds, readfds, writefds, exceptfds, timeout)
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// EVENTFD
// ═══════════════════════════════════════════════════════════════════════════════

pub struct EventFd {
    pub id: u32,
    pub counter: AtomicU64,
    pub flags: u32,
    pub semaphore_mode: bool,
}

impl EventFd {
    pub fn new(id: u32, initval: u64, flags: u32) -> Self {
        Self {
            id,
            counter: AtomicU64::new(initval),
            flags,
            semaphore_mode: flags & EFD_SEMAPHORE != 0,
        }
    }

    pub fn read(&self) -> Result<u64, EventError> {
        loop {
            let val = self.counter.load(Ordering::Acquire);
            if val == 0 {
                if self.flags & EFD_NONBLOCK != 0 {
                    return Err(EventError::WouldBlock);
                }
                return Err(EventError::WouldBlock);
            }
            if self.semaphore_mode {
                if self
                    .counter
                    .compare_exchange(val, val - 1, Ordering::AcqRel, Ordering::Acquire)
                    .is_ok()
                {
                    return Ok(1);
                }
            } else {
                if self
                    .counter
                    .compare_exchange(val, 0, Ordering::AcqRel, Ordering::Acquire)
                    .is_ok()
                {
                    return Ok(val);
                }
            }
        }
    }

    pub fn write(&self, val: u64) -> Result<(), EventError> {
        if val == u64::MAX {
            return Err(EventError::InvalidArgument);
        }
        loop {
            let current = self.counter.load(Ordering::Acquire);
            let new_val = current.checked_add(val).ok_or(EventError::WouldBlock)?;
            if new_val > u64::MAX - 1 {
                if self.flags & EFD_NONBLOCK != 0 {
                    return Err(EventError::WouldBlock);
                }
                return Err(EventError::WouldBlock);
            }
            if self
                .counter
                .compare_exchange(current, new_val, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                return Ok(());
            }
        }
    }
}

pub struct EventFdRegistry {
    fds: BTreeMap<u32, EventFd>,
    next_id: AtomicU32,
}

impl EventFdRegistry {
    pub const fn new() -> Self {
        Self {
            fds: BTreeMap::new(),
            next_id: AtomicU32::new(1),
        }
    }

    pub fn eventfd(&mut self, initval: u64, flags: u32) -> u32 {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.fds.insert(id, EventFd::new(id, initval, flags));
        id
    }

    pub fn eventfd_read(&self, fd: u32) -> Result<u64, EventError> {
        self.fds.get(&fd).ok_or(EventError::BadFd)?.read()
    }

    pub fn eventfd_write(&self, fd: u32, val: u64) -> Result<(), EventError> {
        self.fds.get(&fd).ok_or(EventError::BadFd)?.write(val)
    }

    pub fn close(&mut self, fd: u32) {
        self.fds.remove(&fd);
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// TIMERFD
// ═══════════════════════════════════════════════════════════════════════════════

pub struct TimerFd {
    pub id: u32,
    pub clockid: u32,
    pub flags: u32,
    pub spec: ItimerSpec,
    pub expirations: AtomicU64,
    pub armed: AtomicBool,
}

impl TimerFd {
    pub fn new(id: u32, clockid: u32, flags: u32) -> Self {
        Self {
            id,
            clockid,
            flags,
            spec: ItimerSpec::default(),
            expirations: AtomicU64::new(0),
            armed: AtomicBool::new(false),
        }
    }

    pub fn settime(&mut self, flags: u32, new_value: &ItimerSpec) -> ItimerSpec {
        let old = self.spec;
        self.spec = *new_value;
        let is_armed = new_value.it_value.tv_sec != 0 || new_value.it_value.tv_nsec != 0;
        self.armed.store(is_armed, Ordering::Release);
        self.expirations.store(0, Ordering::Release);
        let _ = flags;
        old
    }

    pub fn gettime(&self) -> ItimerSpec {
        self.spec
    }

    pub fn read(&self) -> Result<u64, EventError> {
        let val = self.expirations.swap(0, Ordering::AcqRel);
        if val == 0 {
            if self.flags & TFD_NONBLOCK != 0 {
                return Err(EventError::WouldBlock);
            }
            return Err(EventError::WouldBlock);
        }
        Ok(val)
    }

    pub fn fire(&self) {
        self.expirations.fetch_add(1, Ordering::Release);
    }
}

pub struct TimerFdRegistry {
    timers: BTreeMap<u32, TimerFd>,
    next_id: AtomicU32,
}

impl TimerFdRegistry {
    pub const fn new() -> Self {
        Self {
            timers: BTreeMap::new(),
            next_id: AtomicU32::new(1),
        }
    }

    pub fn timerfd_create(&mut self, clockid: u32, flags: u32) -> u32 {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.timers.insert(id, TimerFd::new(id, clockid, flags));
        id
    }

    pub fn timerfd_settime(
        &mut self,
        fd: u32,
        flags: u32,
        new_value: &ItimerSpec,
    ) -> Result<ItimerSpec, EventError> {
        let timer = self.timers.get_mut(&fd).ok_or(EventError::BadFd)?;
        Ok(timer.settime(flags, new_value))
    }

    pub fn timerfd_gettime(&self, fd: u32) -> Result<ItimerSpec, EventError> {
        let timer = self.timers.get(&fd).ok_or(EventError::BadFd)?;
        Ok(timer.gettime())
    }

    pub fn close(&mut self, fd: u32) {
        self.timers.remove(&fd);
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// SIGNALFD
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy)]
pub struct SignalfdSiginfo {
    pub ssi_signo: u32,
    pub ssi_errno: i32,
    pub ssi_code: i32,
    pub ssi_pid: u32,
    pub ssi_uid: u32,
    pub ssi_fd: i32,
    pub ssi_tid: u32,
    pub ssi_band: u32,
    pub ssi_overrun: u32,
    pub ssi_trapno: u32,
    pub ssi_status: i32,
    pub ssi_int: i32,
    pub ssi_ptr: u64,
    pub ssi_utime: u64,
    pub ssi_stime: u64,
    pub ssi_addr: u64,
}

impl Default for SignalfdSiginfo {
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
        }
    }
}

pub struct SignalFd {
    pub id: u32,
    pub mask: u64,
    pub flags: u32,
    pub pending: VecDeque<SignalfdSiginfo>,
}

impl SignalFd {
    pub fn new(id: u32, mask: u64, flags: u32) -> Self {
        Self {
            id,
            mask,
            flags,
            pending: VecDeque::new(),
        }
    }

    pub fn deliver_signal(&mut self, siginfo: SignalfdSiginfo) {
        if self.mask & (1u64 << siginfo.ssi_signo) != 0 {
            self.pending.push_back(siginfo);
        }
    }

    pub fn read(&mut self) -> Result<SignalfdSiginfo, EventError> {
        self.pending.pop_front().ok_or(EventError::WouldBlock)
    }

    pub fn update_mask(&mut self, new_mask: u64) {
        self.mask = new_mask;
    }
}

pub struct SignalFdRegistry {
    fds: BTreeMap<u32, SignalFd>,
    next_id: AtomicU32,
}

impl SignalFdRegistry {
    pub const fn new() -> Self {
        Self {
            fds: BTreeMap::new(),
            next_id: AtomicU32::new(1),
        }
    }

    pub fn signalfd(&mut self, existing_fd: Option<u32>, mask: u64, flags: u32) -> u32 {
        if let Some(fd) = existing_fd {
            if let Some(sfd) = self.fds.get_mut(&fd) {
                sfd.update_mask(mask);
                return fd;
            }
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.fds.insert(id, SignalFd::new(id, mask, flags));
        id
    }

    pub fn read(&mut self, fd: u32) -> Result<SignalfdSiginfo, EventError> {
        self.fds.get_mut(&fd).ok_or(EventError::BadFd)?.read()
    }

    pub fn deliver(&mut self, fd: u32, siginfo: SignalfdSiginfo) {
        if let Some(sfd) = self.fds.get_mut(&fd) {
            sfd.deliver_signal(siginfo);
        }
    }

    pub fn close(&mut self, fd: u32) {
        self.fds.remove(&fd);
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// PIDFD
// ═══════════════════════════════════════════════════════════════════════════════

pub struct PidFd {
    pub id: u32,
    pub pid: u32,
    pub flags: u32,
    pub exited: AtomicBool,
    pub exit_code: AtomicU32,
}

impl PidFd {
    pub fn new(id: u32, pid: u32, flags: u32) -> Self {
        Self {
            id,
            pid,
            flags,
            exited: AtomicBool::new(false),
            exit_code: AtomicU32::new(0),
        }
    }

    pub fn notify_exit(&self, code: u32) {
        self.exit_code.store(code, Ordering::Release);
        self.exited.store(true, Ordering::Release);
    }

    pub fn is_exited(&self) -> bool {
        self.exited.load(Ordering::Acquire)
    }
}

pub struct PidFdRegistry {
    fds: BTreeMap<u32, PidFd>,
    next_id: AtomicU32,
}

impl PidFdRegistry {
    pub const fn new() -> Self {
        Self {
            fds: BTreeMap::new(),
            next_id: AtomicU32::new(1),
        }
    }

    pub fn pidfd_open(&mut self, pid: u32, flags: u32) -> u32 {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.fds.insert(id, PidFd::new(id, pid, flags));
        id
    }

    pub fn pidfd_send_signal(&self, fd: u32, _sig: u32) -> Result<(), EventError> {
        let pfd = self.fds.get(&fd).ok_or(EventError::BadFd)?;
        if pfd.is_exited() {
            return Err(EventError::ProcessExited);
        }
        Ok(())
    }

    pub fn pidfd_getfd(&self, fd: u32, _target_fd: i32) -> Result<i32, EventError> {
        let _pfd = self.fds.get(&fd).ok_or(EventError::BadFd)?;
        Ok(0) // placeholder: would duplicate target_fd from the process
    }

    pub fn close(&mut self, fd: u32) {
        self.fds.remove(&fd);
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// USERFAULTFD
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy)]
pub struct UffdRange {
    pub start: u64,
    pub len: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UffdMode {
    Missing,
    WriteProtect,
    Minor,
}

#[derive(Debug, Clone)]
pub struct UffdRegistration {
    pub range: UffdRange,
    pub mode: UffdMode,
    pub active: bool,
}

pub struct UserfaultFd {
    pub id: u32,
    pub features: u64,
    pub registrations: Vec<UffdRegistration>,
    pub pending_faults: VecDeque<UffdFaultEvent>,
}

#[derive(Debug, Clone)]
pub struct UffdFaultEvent {
    pub address: u64,
    pub flags: u64,
    pub mode: UffdMode,
}

impl UserfaultFd {
    pub fn new(id: u32) -> Self {
        Self {
            id,
            features: UFFD_FEATURE_MISSING | UFFD_FEATURE_WP | UFFD_FEATURE_MINOR,
            registrations: Vec::new(),
            pending_faults: VecDeque::new(),
        }
    }

    pub fn ioctl_api(&self) -> u64 {
        self.features
    }

    pub fn ioctl_register(&mut self, range: UffdRange, mode: UffdMode) -> Result<(), EventError> {
        self.registrations.push(UffdRegistration {
            range,
            mode,
            active: true,
        });
        Ok(())
    }

    pub fn ioctl_unregister(&mut self, range: UffdRange) -> Result<(), EventError> {
        self.registrations
            .retain(|r| !(r.range.start == range.start && r.range.len == range.len));
        Ok(())
    }

    pub fn ioctl_copy(&mut self, dst: u64, src: u64, len: u64) -> Result<u64, EventError> {
        let _ = (dst, src);
        Ok(len)
    }

    pub fn ioctl_zeropage(&mut self, range: UffdRange) -> Result<u64, EventError> {
        Ok(range.len)
    }

    pub fn ioctl_wake(&mut self, range: UffdRange) -> Result<(), EventError> {
        self.pending_faults
            .retain(|f| !(f.address >= range.start && f.address < range.start + range.len));
        Ok(())
    }

    pub fn ioctl_writeprotect(
        &mut self,
        range: UffdRange,
        protect: bool,
    ) -> Result<(), EventError> {
        for reg in &mut self.registrations {
            if reg.range.start == range.start && reg.range.len == range.len {
                if protect {
                    reg.mode = UffdMode::WriteProtect;
                }
            }
        }
        Ok(())
    }

    pub fn report_fault(&mut self, address: u64, mode: UffdMode) {
        self.pending_faults.push_back(UffdFaultEvent {
            address,
            flags: 0,
            mode,
        });
    }

    pub fn read_event(&mut self) -> Option<UffdFaultEvent> {
        self.pending_faults.pop_front()
    }
}

pub struct UserfaultFdRegistry {
    fds: BTreeMap<u32, UserfaultFd>,
    next_id: AtomicU32,
}

impl UserfaultFdRegistry {
    pub const fn new() -> Self {
        Self {
            fds: BTreeMap::new(),
            next_id: AtomicU32::new(1),
        }
    }

    pub fn userfaultfd(&mut self) -> u32 {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.fds.insert(id, UserfaultFd::new(id));
        id
    }

    pub fn get_mut(&mut self, fd: u32) -> Option<&mut UserfaultFd> {
        self.fds.get_mut(&fd)
    }

    pub fn close(&mut self, fd: u32) {
        self.fds.remove(&fd);
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// SPLICE / TEE / VMSPLICE / SENDFILE / COPY_FILE_RANGE
// ═══════════════════════════════════════════════════════════════════════════════

pub struct SpliceSubsystem;

impl SpliceSubsystem {
    pub fn splice(
        fd_in: i32,
        off_in: Option<&mut u64>,
        fd_out: i32,
        off_out: Option<&mut u64>,
        len: usize,
        flags: u32,
    ) -> Result<usize, EventError> {
        let _ = (fd_in, fd_out, flags);
        if let Some(off) = off_in {
            *off += len as u64;
        }
        if let Some(off) = off_out {
            *off += len as u64;
        }
        Ok(len)
    }

    pub fn tee(fd_in: i32, fd_out: i32, len: usize, flags: u32) -> Result<usize, EventError> {
        let _ = (fd_in, fd_out, flags);
        Ok(len)
    }

    pub fn vmsplice(fd: i32, iov: &[(usize, usize)], flags: u32) -> Result<usize, EventError> {
        let _ = (fd, flags);
        let total: usize = iov.iter().map(|(_, len)| *len).sum();
        Ok(total)
    }

    pub fn sendfile64(
        out_fd: i32,
        in_fd: i32,
        offset: Option<&mut u64>,
        count: usize,
    ) -> Result<usize, EventError> {
        let _ = (out_fd, in_fd);
        if let Some(off) = offset {
            *off += count as u64;
        }
        Ok(count)
    }

    pub fn copy_file_range(
        fd_in: i32,
        off_in: Option<&mut u64>,
        fd_out: i32,
        off_out: Option<&mut u64>,
        len: usize,
        flags: u32,
    ) -> Result<usize, EventError> {
        let _ = (fd_in, fd_out, flags);
        if let Some(off) = off_in {
            *off += len as u64;
        }
        if let Some(off) = off_out {
            *off += len as u64;
        }
        Ok(len)
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// WAIT QUEUE
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitQueueState {
    Waiting,
    Woken,
    TimedOut,
    Interrupted,
}

#[derive(Debug, Clone)]
pub struct WaitQueueEntry {
    pub task_id: u32,
    pub flags: u32,
    pub state: WaitQueueState,
    pub private_data: u64,
}

impl WaitQueueEntry {
    pub fn new(task_id: u32, flags: u32) -> Self {
        Self {
            task_id,
            flags,
            state: WaitQueueState::Waiting,
            private_data: 0,
        }
    }

    pub fn is_exclusive(&self) -> bool {
        self.flags & WQ_FLAG_EXCLUSIVE != 0
    }
}

pub struct WaitQueueHead {
    pub name: &'static str,
    pub waiters: VecDeque<WaitQueueEntry>,
}

impl WaitQueueHead {
    pub const fn new(name: &'static str) -> Self {
        Self {
            name,
            waiters: VecDeque::new(),
        }
    }

    pub fn add_waiter(&mut self, entry: WaitQueueEntry) {
        if entry.is_exclusive() {
            self.waiters.push_back(entry);
        } else {
            self.waiters.push_front(entry);
        }
    }

    pub fn remove_waiter(&mut self, task_id: u32) {
        self.waiters.retain(|w| w.task_id != task_id);
    }

    pub fn wake_up(&mut self, nr_exclusive: u32) -> u32 {
        let mut woken = 0u32;
        let mut exclusive_woken = 0u32;

        for entry in self.waiters.iter_mut() {
            if entry.state == WaitQueueState::Waiting {
                entry.state = WaitQueueState::Woken;
                woken += 1;
                if entry.is_exclusive() {
                    exclusive_woken += 1;
                    if exclusive_woken >= nr_exclusive {
                        break;
                    }
                }
            }
        }
        self.waiters
            .retain(|w| w.state != WaitQueueState::Woken || w.is_exclusive());
        woken
    }

    pub fn wake_up_all(&mut self) -> u32 {
        let mut woken = 0u32;
        for entry in self.waiters.iter_mut() {
            if entry.state == WaitQueueState::Waiting {
                entry.state = WaitQueueState::Woken;
                woken += 1;
            }
        }
        self.waiters.clear();
        woken
    }

    pub fn wake_up_interruptible(&mut self) -> u32 {
        self.wake_up(1)
    }

    pub fn is_empty(&self) -> bool {
        self.waiters.is_empty()
    }

    pub fn nr_waiters(&self) -> usize {
        self.waiters.len()
    }
}

pub fn wait_event(wq: &mut WaitQueueHead, task_id: u32) {
    let entry = WaitQueueEntry::new(task_id, 0);
    wq.add_waiter(entry);
}

pub fn wait_event_timeout(wq: &mut WaitQueueHead, task_id: u32, _timeout_jiffies: u64) {
    let entry = WaitQueueEntry::new(task_id, 0);
    wq.add_waiter(entry);
}

pub fn wait_event_interruptible(wq: &mut WaitQueueHead, task_id: u32) {
    let entry = WaitQueueEntry::new(task_id, 0);
    wq.add_waiter(entry);
}

pub fn wait_event_interruptible_timeout(wq: &mut WaitQueueHead, task_id: u32, _timeout: u64) {
    let entry = WaitQueueEntry::new(task_id, 0);
    wq.add_waiter(entry);
}

pub fn wake_up(wq: &mut WaitQueueHead) -> u32 {
    wq.wake_up(1)
}

pub fn wake_up_all(wq: &mut WaitQueueHead) -> u32 {
    wq.wake_up_all()
}

pub fn wake_up_interruptible(wq: &mut WaitQueueHead) -> u32 {
    wq.wake_up_interruptible()
}

// ═══════════════════════════════════════════════════════════════════════════════
// COMPLETION
// ═══════════════════════════════════════════════════════════════════════════════

pub struct Completion {
    pub done: AtomicU32,
    pub wait_queue: WaitQueueHead,
}

impl Completion {
    pub const fn new() -> Self {
        Self {
            done: AtomicU32::new(0),
            wait_queue: WaitQueueHead::new("completion"),
        }
    }

    pub fn init(&self) {
        self.done.store(0, Ordering::Release);
    }

    pub fn wait_for_completion(&mut self, task_id: u32) -> bool {
        if self.done.load(Ordering::Acquire) > 0 {
            self.done.fetch_sub(1, Ordering::AcqRel);
            return true;
        }
        wait_event(&mut self.wait_queue, task_id);
        true
    }

    pub fn wait_for_completion_timeout(&mut self, task_id: u32, _timeout_jiffies: u64) -> bool {
        if self.done.load(Ordering::Acquire) > 0 {
            self.done.fetch_sub(1, Ordering::AcqRel);
            return true;
        }
        wait_event_timeout(&mut self.wait_queue, task_id, _timeout_jiffies);
        self.done.load(Ordering::Acquire) > 0
    }

    pub fn try_wait_for_completion(&self) -> bool {
        let val = self.done.load(Ordering::Acquire);
        if val > 0 {
            self.done.fetch_sub(1, Ordering::AcqRel);
            true
        } else {
            false
        }
    }

    pub fn complete(&mut self) {
        self.done.fetch_add(1, Ordering::Release);
        wake_up(&mut self.wait_queue);
    }

    pub fn complete_all(&mut self) {
        self.done.store(u32::MAX / 2, Ordering::Release);
        wake_up_all(&mut self.wait_queue);
    }

    pub fn is_done(&self) -> bool {
        self.done.load(Ordering::Acquire) > 0
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// EVENT LOOP — Main Loop
// ═══════════════════════════════════════════════════════════════════════════════

pub type IdleCallback = fn();
pub type TimeoutCallback = fn(u64);

#[derive(Debug, Clone, Copy)]
pub struct EventSource {
    pub fd: i32,
    pub events: u32,
    pub callback_id: u32,
    pub active: bool,
}

pub struct ScheduledTimeout {
    pub id: u64,
    pub deadline_ns: u64,
    pub callback_id: u32,
    pub repeat_interval_ns: u64,
    pub active: bool,
}

pub struct EventLoopCore {
    pub sources: Vec<EventSource>,
    pub idle_callbacks: Vec<u32>,
    pub timeouts: Vec<ScheduledTimeout>,
    pub running: AtomicBool,
    pub iteration_count: AtomicU64,
    pub next_timeout_id: AtomicU64,
    pub epoll: EpollRegistry,
    pub eventfds: EventFdRegistry,
    pub timerfds: TimerFdRegistry,
    pub signalfds: SignalFdRegistry,
    pub pidfds: PidFdRegistry,
    pub userfaultfds: UserfaultFdRegistry,
}

impl EventLoopCore {
    pub const fn new() -> Self {
        Self {
            sources: Vec::new(),
            idle_callbacks: Vec::new(),
            timeouts: Vec::new(),
            running: AtomicBool::new(false),
            iteration_count: AtomicU64::new(0),
            next_timeout_id: AtomicU64::new(1),
            epoll: EpollRegistry::new(),
            eventfds: EventFdRegistry::new(),
            timerfds: TimerFdRegistry::new(),
            signalfds: SignalFdRegistry::new(),
            pidfds: PidFdRegistry::new(),
            userfaultfds: UserfaultFdRegistry::new(),
        }
    }

    pub fn add_source(&mut self, fd: i32, events: u32, callback_id: u32) {
        self.sources.push(EventSource {
            fd,
            events,
            callback_id,
            active: true,
        });
    }

    pub fn remove_source(&mut self, fd: i32) {
        self.sources.retain(|s| s.fd != fd);
    }

    pub fn add_idle_callback(&mut self, callback_id: u32) {
        self.idle_callbacks.push(callback_id);
    }

    pub fn add_timeout(&mut self, deadline_ns: u64, callback_id: u32, repeat_ns: u64) -> u64 {
        let id = self.next_timeout_id.fetch_add(1, Ordering::Relaxed);
        self.timeouts.push(ScheduledTimeout {
            id,
            deadline_ns,
            callback_id,
            repeat_interval_ns: repeat_ns,
            active: true,
        });
        id
    }

    pub fn cancel_timeout(&mut self, id: u64) {
        if let Some(t) = self.timeouts.iter_mut().find(|t| t.id == id) {
            t.active = false;
        }
    }

    pub fn tick(&mut self, now_ns: u64) -> EventLoopResult {
        self.iteration_count.fetch_add(1, Ordering::Relaxed);

        let mut fired_timeouts = Vec::new();
        let mut reschedule = Vec::new();

        for timeout in &mut self.timeouts {
            if timeout.active && now_ns >= timeout.deadline_ns {
                fired_timeouts.push(timeout.callback_id);
                if timeout.repeat_interval_ns > 0 {
                    reschedule.push((timeout.id, now_ns + timeout.repeat_interval_ns));
                } else {
                    timeout.active = false;
                }
            }
        }

        for (id, new_deadline) in reschedule {
            if let Some(t) = self.timeouts.iter_mut().find(|t| t.id == id) {
                t.deadline_ns = new_deadline;
            }
        }

        self.timeouts.retain(|t| t.active);

        let active_sources: Vec<EventSource> =
            self.sources.iter().filter(|s| s.active).copied().collect();

        EventLoopResult {
            fired_timeouts,
            active_sources,
            idle_pending: !self.idle_callbacks.is_empty(),
        }
    }

    pub fn start(&self) {
        self.running.store(true, Ordering::Release);
    }

    pub fn stop(&self) {
        self.running.store(false, Ordering::Release);
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::Acquire)
    }
}

pub struct EventLoopResult {
    pub fired_timeouts: Vec<u32>,
    pub active_sources: Vec<EventSource>,
    pub idle_pending: bool,
}

// ═══════════════════════════════════════════════════════════════════════════════
// ERROR TYPE
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventError {
    BadFd,
    NotFound,
    AlreadyExists,
    InvalidArgument,
    WouldBlock,
    Interrupted,
    NoMemory,
    ProcessExited,
    Overflow,
    PermissionDenied,
}

impl EventError {
    pub fn to_errno(self) -> i32 {
        match self {
            Self::BadFd => -9,            // EBADF
            Self::NotFound => -2,         // ENOENT
            Self::AlreadyExists => -17,   // EEXIST
            Self::InvalidArgument => -22, // EINVAL
            Self::WouldBlock => -11,      // EAGAIN
            Self::Interrupted => -4,      // EINTR
            Self::NoMemory => -12,        // ENOMEM
            Self::ProcessExited => -3,    // ESRCH
            Self::Overflow => -75,        // EOVERFLOW
            Self::PermissionDenied => -1, // EPERM
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// GLOBAL STATE
// ═══════════════════════════════════════════════════════════════════════════════

pub static EVENT_LOOP: Mutex<EventLoopCore> = Mutex::new(EventLoopCore::new());

pub fn init() {
    let el = EVENT_LOOP.lock();
    el.start();
}
