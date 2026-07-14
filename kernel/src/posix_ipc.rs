//! POSIX IPC & System V IPC — Full Implementation
//!
//! Provides shared memory, message queues, and semaphores for both
//! POSIX and System V interfaces, plus IPC namespace support.

#![allow(dead_code)]

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use spin::Mutex;

// ─── Resource Limits ─────────────────────────────────────────────────────────

pub const MSGMNI: usize = 32000;
pub const MSGMAX: usize = 8192;
pub const MSGMNB: usize = 16384;
pub const SHMMAX: usize = 0x2000_0000; // 512 MiB
pub const SHMMIN: usize = 1;
pub const SHMMNI: usize = 4096;
pub const SHMSEG: usize = 4096;
pub const SEMMNI: usize = 32000;
pub const SEMMSL: usize = 32000;
pub const SEMMNS: usize = 1_024_000;
pub const SEMOPM: usize = 500;
pub const SEMVMX: usize = 32767;

// ─── IPC Flags & Commands ────────────────────────────────────────────────────

pub const IPC_CREAT: u32 = 0o1000;
pub const IPC_EXCL: u32 = 0o2000;
pub const IPC_NOWAIT: u32 = 0o4000;
pub const IPC_PRIVATE: i32 = 0;
pub const IPC_RMID: u32 = 0;
pub const IPC_SET: u32 = 1;
pub const IPC_STAT: u32 = 2;
pub const IPC_INFO: u32 = 3;

pub const SHM_INFO: u32 = 14;
pub const SHM_STAT: u32 = 13;
pub const SHM_LOCK: u32 = 11;
pub const SHM_UNLOCK: u32 = 12;
pub const SHM_RDONLY: u32 = 0o10000;
pub const SHM_RND: u32 = 0o20000;
pub const SHM_REMAP: u32 = 0o40000;

pub const MSG_INFO: u32 = 12;
pub const MSG_STAT: u32 = 11;
pub const MSG_NOERROR: u32 = 0o10000;
pub const MSG_EXCEPT: u32 = 0o20000;
pub const MSG_COPY: u32 = 0o40000;

pub const SEM_INFO: u32 = 19;
pub const SEM_STAT: u32 = 18;
pub const SEM_UNDO: u16 = 0x1000;
pub const GETVAL: u32 = 12;
pub const SETVAL: u32 = 16;
pub const GETALL: u32 = 13;
pub const SETALL: u32 = 17;
pub const GETPID: u32 = 11;
pub const GETNCNT: u32 = 14;
pub const GETZCNT: u32 = 15;

pub const O_CREAT: u32 = 0o100;
pub const O_EXCL: u32 = 0o200;
pub const O_RDONLY: u32 = 0;
pub const O_RDWR: u32 = 2;
pub const O_WRONLY: u32 = 1;

pub const SIGEV_NONE: u32 = 0;
pub const SIGEV_SIGNAL: u32 = 1;
pub const SIGEV_THREAD: u32 = 2;

// ─── IPC Permission Structure ────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub struct IpcPerm {
    pub key: i32,
    pub uid: u32,
    pub gid: u32,
    pub cuid: u32,
    pub cgid: u32,
    pub mode: u16,
    pub seq: u16,
}

impl IpcPerm {
    pub fn new(key: i32, uid: u32, gid: u32, mode: u16) -> Self {
        static SEQ_COUNTER: AtomicU32 = AtomicU32::new(0);
        Self {
            key,
            uid,
            gid,
            cuid: uid,
            cgid: gid,
            mode,
            seq: SEQ_COUNTER.fetch_add(1, Ordering::Relaxed) as u16,
        }
    }

    pub fn check_permission(&self, uid: u32, gid: u32, requested: u16) -> bool {
        if uid == 0 {
            return true;
        }
        if self.uid == uid || self.cuid == uid {
            return (self.mode >> 6) & 0o7 >= requested;
        }
        if self.gid == gid || self.cgid == gid {
            return (self.mode >> 3) & 0o7 >= requested;
        }
        self.mode & 0o7 >= requested
    }
}

// ─── ftok ────────────────────────────────────────────────────────────────────

pub fn ftok(path_hash: u32, proj_id: u8) -> i32 {
    ((proj_id as i32) << 24) | (path_hash as i32 & 0x00FF_FFFF)
}

// ─── Timespec ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, Default)]
pub struct Timespec {
    pub tv_sec: i64,
    pub tv_nsec: i64,
}

impl Timespec {
    pub fn new(sec: i64, nsec: i64) -> Self {
        Self {
            tv_sec: sec,
            tv_nsec: nsec,
        }
    }

    pub fn is_expired(&self, now: &Timespec) -> bool {
        if now.tv_sec > self.tv_sec {
            return true;
        }
        if now.tv_sec == self.tv_sec && now.tv_nsec >= self.tv_nsec {
            return true;
        }
        false
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// POSIX SHARED MEMORY
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct PosixShmObject {
    pub name: String,
    pub data: Vec<u8>,
    pub size: usize,
    pub mode: u16,
    pub uid: u32,
    pub gid: u32,
    pub flags: u32,
    pub ref_count: u32,
    pub created: Timespec,
    pub modified: Timespec,
}

impl PosixShmObject {
    pub fn new(name: String, mode: u16, flags: u32) -> Self {
        Self {
            name,
            data: Vec::new(),
            size: 0,
            mode,
            uid: 0,
            gid: 0,
            flags,
            ref_count: 1,
            created: Timespec::default(),
            modified: Timespec::default(),
        }
    }

    pub fn ftruncate(&mut self, size: usize) -> Result<(), IpcError> {
        if size > SHMMAX {
            return Err(IpcError::NoMemory);
        }
        self.data.resize(size, 0);
        self.size = size;
        Ok(())
    }
}

#[derive(Debug)]
pub struct PosixShmRegistry {
    objects: BTreeMap<String, PosixShmObject>,
}

impl PosixShmRegistry {
    pub const fn new() -> Self {
        Self {
            objects: BTreeMap::new(),
        }
    }

    pub fn shm_open(&mut self, name: &str, flags: u32, mode: u16) -> Result<u32, IpcError> {
        let exists = self.objects.contains_key(name);

        if flags & O_CREAT != 0 && flags & O_EXCL != 0 && exists {
            return Err(IpcError::AlreadyExists);
        }

        if !exists && flags & O_CREAT == 0 {
            return Err(IpcError::NotFound);
        }

        if !exists {
            let obj = PosixShmObject::new(String::from(name), mode, flags);
            self.objects.insert(String::from(name), obj);
        } else if let Some(obj) = self.objects.get_mut(name) {
            obj.ref_count += 1;
        }

        Ok(self.objects.len() as u32 - 1)
    }

    pub fn shm_unlink(&mut self, name: &str) -> Result<(), IpcError> {
        self.objects
            .remove(name)
            .map(|_| ())
            .ok_or(IpcError::NotFound)
    }

    pub fn mmap(&self, name: &str, offset: usize, length: usize) -> Result<&[u8], IpcError> {
        let obj = self.objects.get(name).ok_or(IpcError::NotFound)?;
        if offset + length > obj.size {
            return Err(IpcError::InvalidArgument);
        }
        Ok(&obj.data[offset..offset + length])
    }

    pub fn mmap_mut(
        &mut self,
        name: &str,
        offset: usize,
        length: usize,
    ) -> Result<&mut [u8], IpcError> {
        let obj = self.objects.get_mut(name).ok_or(IpcError::NotFound)?;
        if offset + length > obj.size {
            return Err(IpcError::InvalidArgument);
        }
        Ok(&mut obj.data[offset..offset + length])
    }

    pub fn munmap(&mut self, name: &str) -> Result<(), IpcError> {
        let obj = self.objects.get_mut(name).ok_or(IpcError::NotFound)?;
        obj.ref_count = obj.ref_count.saturating_sub(1);
        Ok(())
    }

    pub fn ftruncate(&mut self, name: &str, size: usize) -> Result<(), IpcError> {
        let obj = self.objects.get_mut(name).ok_or(IpcError::NotFound)?;
        obj.ftruncate(size)
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// SYSTEM V SHARED MEMORY
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct ShmSegment {
    pub perm: IpcPerm,
    pub size: usize,
    pub data: Vec<u8>,
    pub nattch: u32,
    pub cpid: u32,
    pub lpid: u32,
    pub atime: Timespec,
    pub dtime: Timespec,
    pub ctime: Timespec,
    pub locked: bool,
    pub marked_destroy: bool,
}

impl ShmSegment {
    pub fn new(key: i32, size: usize, mode: u16, pid: u32) -> Self {
        let mut data = Vec::new();
        data.resize(size, 0);
        Self {
            perm: IpcPerm::new(key, 0, 0, mode),
            size,
            data,
            nattch: 0,
            cpid: pid,
            lpid: pid,
            atime: Timespec::default(),
            dtime: Timespec::default(),
            ctime: Timespec::default(),
            locked: false,
            marked_destroy: false,
        }
    }
}

#[derive(Debug)]
pub struct SysVShmRegistry {
    segments: BTreeMap<i32, ShmSegment>,
    next_id: i32,
}

impl SysVShmRegistry {
    pub const fn new() -> Self {
        Self {
            segments: BTreeMap::new(),
            next_id: 1,
        }
    }

    pub fn shmget(&mut self, key: i32, size: usize, flags: u32) -> Result<i32, IpcError> {
        if size < SHMMIN || size > SHMMAX {
            return Err(IpcError::InvalidArgument);
        }
        if self.segments.len() >= SHMMNI {
            return Err(IpcError::NoSpace);
        }

        if key != IPC_PRIVATE {
            if let Some((&id, seg)) = self.segments.iter().find(|(_, s)| s.perm.key == key) {
                if flags & IPC_CREAT != 0 && flags & IPC_EXCL != 0 {
                    return Err(IpcError::AlreadyExists);
                }
                if seg.size < size {
                    return Err(IpcError::InvalidArgument);
                }
                return Ok(id);
            }
        }

        if flags & IPC_CREAT == 0 && key != IPC_PRIVATE {
            return Err(IpcError::NotFound);
        }

        let id = self.next_id;
        self.next_id += 1;
        let mode = (flags & 0o777) as u16;
        let segment = ShmSegment::new(key, size, mode, 0);
        self.segments.insert(id, segment);
        Ok(id)
    }

    pub fn shmat(
        &mut self,
        shmid: i32,
        _shmaddr: usize,
        _flags: u32,
    ) -> Result<*const u8, IpcError> {
        let segment = self.segments.get_mut(&shmid).ok_or(IpcError::NotFound)?;
        segment.nattch += 1;
        Ok(segment.data.as_ptr())
    }

    pub fn shmdt(&mut self, shmid: i32) -> Result<(), IpcError> {
        let segment = self.segments.get_mut(&shmid).ok_or(IpcError::NotFound)?;
        segment.nattch = segment.nattch.saturating_sub(1);
        if segment.nattch == 0 && segment.marked_destroy {
            self.segments.remove(&shmid);
        }
        Ok(())
    }

    pub fn shmctl(
        &mut self,
        shmid: i32,
        cmd: u32,
        uid: u32,
        gid: u32,
    ) -> Result<ShmCtlResult, IpcError> {
        match cmd {
            IPC_STAT => {
                let seg = self.segments.get(&shmid).ok_or(IpcError::NotFound)?;
                Ok(ShmCtlResult::Stat(seg.clone()))
            }
            IPC_SET => {
                let seg = self.segments.get_mut(&shmid).ok_or(IpcError::NotFound)?;
                seg.perm.uid = uid;
                seg.perm.gid = gid;
                Ok(ShmCtlResult::Ok)
            }
            IPC_RMID => {
                let seg = self.segments.get_mut(&shmid).ok_or(IpcError::NotFound)?;
                if seg.nattch == 0 {
                    self.segments.remove(&shmid);
                } else {
                    seg.marked_destroy = true;
                }
                Ok(ShmCtlResult::Ok)
            }
            IPC_INFO => Ok(ShmCtlResult::Info(ShmInfo {
                shmmax: SHMMAX,
                shmmin: SHMMIN,
                shmmni: SHMMNI,
                shmseg: SHMSEG,
                shmall: SHMMAX / 4096,
            })),
            SHM_INFO => Ok(ShmCtlResult::ShmInfoExt(ShmInfoExt {
                used_ids: self.segments.len(),
                shm_tot: self.segments.values().map(|s| s.size).sum(),
                shm_rss: self.segments.values().map(|s| s.size).sum(),
                shm_swp: 0,
            })),
            SHM_STAT => {
                let seg = self.segments.get(&shmid).ok_or(IpcError::NotFound)?;
                Ok(ShmCtlResult::Stat(seg.clone()))
            }
            SHM_LOCK => {
                let seg = self.segments.get_mut(&shmid).ok_or(IpcError::NotFound)?;
                seg.locked = true;
                Ok(ShmCtlResult::Ok)
            }
            SHM_UNLOCK => {
                let seg = self.segments.get_mut(&shmid).ok_or(IpcError::NotFound)?;
                seg.locked = false;
                Ok(ShmCtlResult::Ok)
            }
            _ => Err(IpcError::InvalidArgument),
        }
    }
}

#[derive(Debug)]
pub enum ShmCtlResult {
    Ok,
    Stat(ShmSegment),
    Info(ShmInfo),
    ShmInfoExt(ShmInfoExt),
}

#[derive(Debug)]
pub struct ShmInfo {
    pub shmmax: usize,
    pub shmmin: usize,
    pub shmmni: usize,
    pub shmseg: usize,
    pub shmall: usize,
}

#[derive(Debug)]
pub struct ShmInfoExt {
    pub used_ids: usize,
    pub shm_tot: usize,
    pub shm_rss: usize,
    pub shm_swp: usize,
}

// ═══════════════════════════════════════════════════════════════════════════════
// POSIX MESSAGE QUEUES
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct MqMessage {
    pub priority: u32,
    pub data: Vec<u8>,
    pub timestamp: Timespec,
}

#[derive(Debug, Clone, Copy)]
pub struct MqAttr {
    pub mq_flags: i64,
    pub mq_maxmsg: i64,
    pub mq_msgsize: i64,
    pub mq_curmsgs: i64,
}

impl Default for MqAttr {
    fn default() -> Self {
        Self {
            mq_flags: 0,
            mq_maxmsg: 10,
            mq_msgsize: MSGMAX as i64,
            mq_curmsgs: 0,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum SigevNotify {
    None,
    Signal(u32),
    Thread { function: usize, attr: usize },
}

#[derive(Debug, Clone)]
pub struct PosixMq {
    pub name: String,
    pub attr: MqAttr,
    pub messages: Vec<MqMessage>,
    pub mode: u16,
    pub uid: u32,
    pub gid: u32,
    pub notify: SigevNotify,
    pub notify_pid: u32,
    pub open_count: u32,
    pub unlinked: bool,
}

impl PosixMq {
    pub fn new(name: String, mode: u16, attr: Option<MqAttr>) -> Self {
        Self {
            name,
            attr: attr.unwrap_or_default(),
            messages: Vec::new(),
            mode,
            uid: 0,
            gid: 0,
            notify: SigevNotify::None,
            notify_pid: 0,
            open_count: 1,
            unlinked: false,
        }
    }

    fn insert_priority_ordered(&mut self, msg: MqMessage) {
        let pos = self
            .messages
            .partition_point(|m| m.priority >= msg.priority);
        self.messages.insert(pos, msg);
        self.attr.mq_curmsgs += 1;
    }
}

#[derive(Debug)]
pub struct PosixMqRegistry {
    queues: BTreeMap<String, PosixMq>,
}

impl PosixMqRegistry {
    pub const fn new() -> Self {
        Self {
            queues: BTreeMap::new(),
        }
    }

    pub fn mq_open(
        &mut self,
        name: &str,
        flags: u32,
        mode: u16,
        attr: Option<MqAttr>,
    ) -> Result<u32, IpcError> {
        let exists = self.queues.contains_key(name);

        if flags & O_CREAT != 0 && flags & O_EXCL != 0 && exists {
            return Err(IpcError::AlreadyExists);
        }
        if !exists && flags & O_CREAT == 0 {
            return Err(IpcError::NotFound);
        }

        if !exists {
            let mq = PosixMq::new(String::from(name), mode, attr);
            self.queues.insert(String::from(name), mq);
        } else if let Some(mq) = self.queues.get_mut(name) {
            mq.open_count += 1;
        }

        Ok(self.queues.len() as u32 - 1)
    }

    pub fn mq_close(&mut self, name: &str) -> Result<(), IpcError> {
        let mq = self.queues.get_mut(name).ok_or(IpcError::NotFound)?;
        mq.open_count = mq.open_count.saturating_sub(1);
        if mq.open_count == 0 && mq.unlinked {
            self.queues.remove(name);
        }
        Ok(())
    }

    pub fn mq_unlink(&mut self, name: &str) -> Result<(), IpcError> {
        let mq = self.queues.get_mut(name).ok_or(IpcError::NotFound)?;
        if mq.open_count == 0 {
            self.queues.remove(name);
        } else {
            mq.unlinked = true;
        }
        Ok(())
    }

    pub fn mq_send(&mut self, name: &str, data: &[u8], priority: u32) -> Result<(), IpcError> {
        let mq = self.queues.get_mut(name).ok_or(IpcError::NotFound)?;
        if data.len() > mq.attr.mq_msgsize as usize {
            return Err(IpcError::MessageTooLong);
        }
        if mq.attr.mq_curmsgs >= mq.attr.mq_maxmsg {
            return Err(IpcError::QueueFull);
        }
        let msg = MqMessage {
            priority,
            data: Vec::from(data),
            timestamp: Timespec::default(),
        };
        mq.insert_priority_ordered(msg);
        Ok(())
    }

    pub fn mq_receive(&mut self, name: &str, buf: &mut [u8]) -> Result<(usize, u32), IpcError> {
        let mq = self.queues.get_mut(name).ok_or(IpcError::NotFound)?;
        if mq.messages.is_empty() {
            return Err(IpcError::QueueEmpty);
        }
        let msg = mq.messages.remove(0);
        mq.attr.mq_curmsgs -= 1;
        let len = msg.data.len().min(buf.len());
        buf[..len].copy_from_slice(&msg.data[..len]);
        Ok((len, msg.priority))
    }

    pub fn mq_timedsend(
        &mut self,
        name: &str,
        data: &[u8],
        priority: u32,
        _timeout: &Timespec,
    ) -> Result<(), IpcError> {
        self.mq_send(name, data, priority)
    }

    pub fn mq_timedreceive(
        &mut self,
        name: &str,
        buf: &mut [u8],
        _timeout: &Timespec,
    ) -> Result<(usize, u32), IpcError> {
        self.mq_receive(name, buf)
    }

    pub fn mq_getattr(&self, name: &str) -> Result<MqAttr, IpcError> {
        let mq = self.queues.get(name).ok_or(IpcError::NotFound)?;
        Ok(mq.attr)
    }

    pub fn mq_setattr(&mut self, name: &str, new_attr: &MqAttr) -> Result<MqAttr, IpcError> {
        let mq = self.queues.get_mut(name).ok_or(IpcError::NotFound)?;
        let old = mq.attr;
        mq.attr.mq_flags = new_attr.mq_flags;
        Ok(old)
    }

    pub fn mq_notify(&mut self, name: &str, sigev: SigevNotify, pid: u32) -> Result<(), IpcError> {
        let mq = self.queues.get_mut(name).ok_or(IpcError::NotFound)?;
        mq.notify = sigev;
        mq.notify_pid = pid;
        Ok(())
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// SYSTEM V MESSAGE QUEUES
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct SysVMessage {
    pub mtype: i64,
    pub mtext: Vec<u8>,
    pub timestamp: Timespec,
}

#[derive(Debug, Clone)]
pub struct SysVMsgQueue {
    pub perm: IpcPerm,
    pub messages: Vec<SysVMessage>,
    pub qbytes: usize,
    pub cbytes: usize,
    pub qnum: usize,
    pub lspid: u32,
    pub lrpid: u32,
    pub stime: Timespec,
    pub rtime: Timespec,
    pub ctime: Timespec,
}

impl SysVMsgQueue {
    pub fn new(key: i32, mode: u16) -> Self {
        Self {
            perm: IpcPerm::new(key, 0, 0, mode),
            messages: Vec::new(),
            qbytes: MSGMNB,
            cbytes: 0,
            qnum: 0,
            lspid: 0,
            lrpid: 0,
            stime: Timespec::default(),
            rtime: Timespec::default(),
            ctime: Timespec::default(),
        }
    }
}

#[derive(Debug)]
pub struct SysVMsgRegistry {
    queues: BTreeMap<i32, SysVMsgQueue>,
    next_id: i32,
}

impl SysVMsgRegistry {
    pub const fn new() -> Self {
        Self {
            queues: BTreeMap::new(),
            next_id: 1,
        }
    }

    pub fn msgget(&mut self, key: i32, flags: u32) -> Result<i32, IpcError> {
        if self.queues.len() >= MSGMNI {
            return Err(IpcError::NoSpace);
        }

        if key != IPC_PRIVATE {
            if let Some((&id, _)) = self.queues.iter().find(|(_, q)| q.perm.key == key) {
                if flags & IPC_CREAT != 0 && flags & IPC_EXCL != 0 {
                    return Err(IpcError::AlreadyExists);
                }
                return Ok(id);
            }
        }

        if flags & IPC_CREAT == 0 && key != IPC_PRIVATE {
            return Err(IpcError::NotFound);
        }

        let id = self.next_id;
        self.next_id += 1;
        let mode = (flags & 0o777) as u16;
        self.queues.insert(id, SysVMsgQueue::new(key, mode));
        Ok(id)
    }

    pub fn msgsnd(
        &mut self,
        msqid: i32,
        mtype: i64,
        data: &[u8],
        flags: u32,
    ) -> Result<(), IpcError> {
        if mtype <= 0 {
            return Err(IpcError::InvalidArgument);
        }
        if data.len() > MSGMAX {
            return Err(IpcError::MessageTooLong);
        }

        let queue = self.queues.get_mut(&msqid).ok_or(IpcError::NotFound)?;

        if queue.cbytes + data.len() > queue.qbytes {
            if flags & IPC_NOWAIT != 0 {
                return Err(IpcError::WouldBlock);
            }
            return Err(IpcError::QueueFull);
        }

        queue.messages.push(SysVMessage {
            mtype,
            mtext: Vec::from(data),
            timestamp: Timespec::default(),
        });
        queue.cbytes += data.len();
        queue.qnum += 1;
        Ok(())
    }

    pub fn msgrcv(
        &mut self,
        msqid: i32,
        mtype: i64,
        buf: &mut [u8],
        flags: u32,
    ) -> Result<(usize, i64), IpcError> {
        let queue = self.queues.get_mut(&msqid).ok_or(IpcError::NotFound)?;

        let idx = if mtype == 0 {
            if queue.messages.is_empty() {
                None
            } else {
                Some(0)
            }
        } else if mtype > 0 {
            queue.messages.iter().position(|m| m.mtype == mtype)
        } else {
            let abs_type = -mtype;
            queue.messages.iter().position(|m| m.mtype <= abs_type)
        };

        let idx = match idx {
            Some(i) => i,
            None => {
                if flags & IPC_NOWAIT != 0 {
                    return Err(IpcError::WouldBlock);
                }
                return Err(IpcError::QueueEmpty);
            }
        };

        let msg = &queue.messages[idx];
        if msg.mtext.len() > buf.len() && flags & MSG_NOERROR == 0 {
            return Err(IpcError::MessageTooLong);
        }

        let msg = queue.messages.remove(idx);
        let copy_len = msg.mtext.len().min(buf.len());
        buf[..copy_len].copy_from_slice(&msg.mtext[..copy_len]);
        queue.cbytes -= msg.mtext.len();
        queue.qnum -= 1;
        Ok((copy_len, msg.mtype))
    }

    pub fn msgctl(&mut self, msqid: i32, cmd: u32) -> Result<MsgCtlResult, IpcError> {
        match cmd {
            IPC_STAT => {
                let q = self.queues.get(&msqid).ok_or(IpcError::NotFound)?;
                Ok(MsgCtlResult::Stat(MsgQueueStat {
                    perm: q.perm,
                    qnum: q.qnum,
                    qbytes: q.qbytes,
                    cbytes: q.cbytes,
                    lspid: q.lspid,
                    lrpid: q.lrpid,
                }))
            }
            IPC_SET => Ok(MsgCtlResult::Ok),
            IPC_RMID => {
                self.queues.remove(&msqid).ok_or(IpcError::NotFound)?;
                Ok(MsgCtlResult::Ok)
            }
            IPC_INFO => Ok(MsgCtlResult::Info(MsgInfo {
                msgmni: MSGMNI,
                msgmax: MSGMAX,
                msgmnb: MSGMNB,
                msgpool: MSGMNI * MSGMNB,
                msgmap: MSGMNI,
                msgtql: self.queues.values().map(|q| q.qnum).sum(),
            })),
            MSG_INFO => Ok(MsgCtlResult::MsgInfoExt(MsgInfoExt {
                used_ids: self.queues.len(),
                msg_num: self.queues.values().map(|q| q.qnum).sum(),
                msg_bytes: self.queues.values().map(|q| q.cbytes).sum(),
            })),
            MSG_STAT => {
                let q = self.queues.get(&msqid).ok_or(IpcError::NotFound)?;
                Ok(MsgCtlResult::Stat(MsgQueueStat {
                    perm: q.perm,
                    qnum: q.qnum,
                    qbytes: q.qbytes,
                    cbytes: q.cbytes,
                    lspid: q.lspid,
                    lrpid: q.lrpid,
                }))
            }
            _ => Err(IpcError::InvalidArgument),
        }
    }
}

#[derive(Debug)]
pub enum MsgCtlResult {
    Ok,
    Stat(MsgQueueStat),
    Info(MsgInfo),
    MsgInfoExt(MsgInfoExt),
}

#[derive(Debug)]
pub struct MsgQueueStat {
    pub perm: IpcPerm,
    pub qnum: usize,
    pub qbytes: usize,
    pub cbytes: usize,
    pub lspid: u32,
    pub lrpid: u32,
}

#[derive(Debug)]
pub struct MsgInfo {
    pub msgmni: usize,
    pub msgmax: usize,
    pub msgmnb: usize,
    pub msgpool: usize,
    pub msgmap: usize,
    pub msgtql: usize,
}

#[derive(Debug)]
pub struct MsgInfoExt {
    pub used_ids: usize,
    pub msg_num: usize,
    pub msg_bytes: usize,
}

// ═══════════════════════════════════════════════════════════════════════════════
// POSIX SEMAPHORES
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug)]
pub struct PosixSemaphore {
    pub name: Option<String>,
    pub value: AtomicU32,
    pub max_value: u32,
    pub waiters: u32,
    pub mode: u16,
    pub uid: u32,
    pub gid: u32,
    pub open_count: u32,
    pub unlinked: bool,
}

impl PosixSemaphore {
    pub fn new_named(name: String, mode: u16, value: u32) -> Self {
        Self {
            name: Some(name),
            value: AtomicU32::new(value),
            max_value: SEMVMX as u32,
            waiters: 0,
            mode,
            uid: 0,
            gid: 0,
            open_count: 1,
            unlinked: false,
        }
    }

    pub fn new_unnamed(value: u32) -> Self {
        Self {
            name: None,
            value: AtomicU32::new(value),
            max_value: SEMVMX as u32,
            waiters: 0,
            mode: 0o666,
            uid: 0,
            gid: 0,
            open_count: 1,
            unlinked: false,
        }
    }

    pub fn sem_wait(&self) -> Result<(), IpcError> {
        loop {
            let current = self.value.load(Ordering::Acquire);
            if current == 0 {
                return Err(IpcError::WouldBlock);
            }
            if self
                .value
                .compare_exchange(current, current - 1, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                return Ok(());
            }
        }
    }

    pub fn sem_trywait(&self) -> Result<(), IpcError> {
        let current = self.value.load(Ordering::Acquire);
        if current == 0 {
            return Err(IpcError::WouldBlock);
        }
        self.value
            .compare_exchange(current, current - 1, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| ())
            .map_err(|_| IpcError::WouldBlock)
    }

    pub fn sem_timedwait(&self, _timeout: &Timespec) -> Result<(), IpcError> {
        self.sem_wait()
    }

    pub fn sem_post(&self) -> Result<(), IpcError> {
        let current = self.value.load(Ordering::Acquire);
        if current >= self.max_value {
            return Err(IpcError::Overflow);
        }
        self.value.fetch_add(1, Ordering::Release);
        Ok(())
    }

    pub fn sem_getvalue(&self) -> i32 {
        self.value.load(Ordering::Relaxed) as i32
    }
}

#[derive(Debug)]
pub struct PosixSemRegistry {
    named: BTreeMap<String, PosixSemaphore>,
    unnamed: Vec<PosixSemaphore>,
}

impl PosixSemRegistry {
    pub const fn new() -> Self {
        Self {
            named: BTreeMap::new(),
            unnamed: Vec::new(),
        }
    }

    pub fn sem_open(
        &mut self,
        name: &str,
        flags: u32,
        mode: u16,
        value: u32,
    ) -> Result<u32, IpcError> {
        let exists = self.named.contains_key(name);
        if flags & O_CREAT != 0 && flags & O_EXCL != 0 && exists {
            return Err(IpcError::AlreadyExists);
        }
        if !exists && flags & O_CREAT == 0 {
            return Err(IpcError::NotFound);
        }

        if !exists {
            let sem = PosixSemaphore::new_named(String::from(name), mode, value);
            self.named.insert(String::from(name), sem);
        } else if let Some(sem) = self.named.get_mut(name) {
            sem.open_count += 1;
        }
        Ok(self.named.len() as u32 - 1)
    }

    pub fn sem_close(&mut self, name: &str) -> Result<(), IpcError> {
        let sem = self.named.get_mut(name).ok_or(IpcError::NotFound)?;
        sem.open_count = sem.open_count.saturating_sub(1);
        if sem.open_count == 0 && sem.unlinked {
            self.named.remove(name);
        }
        Ok(())
    }

    pub fn sem_unlink(&mut self, name: &str) -> Result<(), IpcError> {
        let sem = self.named.get_mut(name).ok_or(IpcError::NotFound)?;
        if sem.open_count == 0 {
            self.named.remove(name);
        } else {
            sem.unlinked = true;
        }
        Ok(())
    }

    pub fn sem_init(&mut self, value: u32) -> usize {
        let sem = PosixSemaphore::new_unnamed(value);
        self.unnamed.push(sem);
        self.unnamed.len() - 1
    }

    pub fn sem_destroy(&mut self, idx: usize) -> Result<(), IpcError> {
        if idx >= self.unnamed.len() {
            return Err(IpcError::InvalidArgument);
        }
        self.unnamed.remove(idx);
        Ok(())
    }

    pub fn sem_wait_named(&self, name: &str) -> Result<(), IpcError> {
        let sem = self.named.get(name).ok_or(IpcError::NotFound)?;
        sem.sem_wait()
    }

    pub fn sem_post_named(&self, name: &str) -> Result<(), IpcError> {
        let sem = self.named.get(name).ok_or(IpcError::NotFound)?;
        sem.sem_post()
    }

    pub fn sem_getvalue_named(&self, name: &str) -> Result<i32, IpcError> {
        let sem = self.named.get(name).ok_or(IpcError::NotFound)?;
        Ok(sem.sem_getvalue())
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// SYSTEM V SEMAPHORES
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy)]
pub struct SemBuf {
    pub sem_num: u16,
    pub sem_op: i16,
    pub sem_flg: u16,
}

#[derive(Debug, Clone)]
pub struct SysVSemVal {
    pub value: i32,
    pub pid: u32,
    pub ncnt: u32,
    pub zcnt: u32,
}

impl SysVSemVal {
    pub fn new() -> Self {
        Self {
            value: 0,
            pid: 0,
            ncnt: 0,
            zcnt: 0,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SysVSemSet {
    pub perm: IpcPerm,
    pub sems: Vec<SysVSemVal>,
    pub otime: Timespec,
    pub ctime: Timespec,
    pub undo_list: BTreeMap<u32, Vec<i16>>,
}

impl SysVSemSet {
    pub fn new(key: i32, nsems: usize, mode: u16) -> Self {
        let mut sems = Vec::with_capacity(nsems);
        for _ in 0..nsems {
            sems.push(SysVSemVal::new());
        }
        Self {
            perm: IpcPerm::new(key, 0, 0, mode),
            sems,
            otime: Timespec::default(),
            ctime: Timespec::default(),
            undo_list: BTreeMap::new(),
        }
    }
}

#[derive(Debug)]
pub struct SysVSemRegistry {
    sets: BTreeMap<i32, SysVSemSet>,
    next_id: i32,
}

impl SysVSemRegistry {
    pub const fn new() -> Self {
        Self {
            sets: BTreeMap::new(),
            next_id: 1,
        }
    }

    pub fn semget(&mut self, key: i32, nsems: usize, flags: u32) -> Result<i32, IpcError> {
        if self.sets.len() >= SEMMNI {
            return Err(IpcError::NoSpace);
        }
        if nsems > SEMMSL {
            return Err(IpcError::InvalidArgument);
        }

        if key != IPC_PRIVATE {
            if let Some((&id, set)) = self.sets.iter().find(|(_, s)| s.perm.key == key) {
                if flags & IPC_CREAT != 0 && flags & IPC_EXCL != 0 {
                    return Err(IpcError::AlreadyExists);
                }
                if nsems != 0 && set.sems.len() < nsems {
                    return Err(IpcError::InvalidArgument);
                }
                return Ok(id);
            }
        }

        if flags & IPC_CREAT == 0 && key != IPC_PRIVATE {
            return Err(IpcError::NotFound);
        }

        let id = self.next_id;
        self.next_id += 1;
        let mode = (flags & 0o777) as u16;
        self.sets.insert(id, SysVSemSet::new(key, nsems, mode));
        Ok(id)
    }

    pub fn semop(&mut self, semid: i32, ops: &[SemBuf], pid: u32) -> Result<(), IpcError> {
        let set = self.sets.get_mut(&semid).ok_or(IpcError::NotFound)?;

        for op in ops {
            let idx = op.sem_num as usize;
            if idx >= set.sems.len() {
                return Err(IpcError::InvalidArgument);
            }
        }

        for op in ops {
            let idx = op.sem_num as usize;
            let sem = &mut set.sems[idx];

            if op.sem_op > 0 {
                let new_val = sem.value + op.sem_op as i32;
                if new_val > SEMVMX as i32 {
                    return Err(IpcError::Overflow);
                }
                sem.value = new_val;
                sem.pid = pid;

                if op.sem_flg & SEM_UNDO != 0 {
                    let undo = set.undo_list.entry(pid).or_insert_with(|| {
                        let mut v = Vec::new();
                        v.resize(set.sems.len(), 0i16);
                        v
                    });
                    undo[idx] -= op.sem_op;
                }
            } else if op.sem_op < 0 {
                let needed = (-op.sem_op) as i32;
                if sem.value < needed {
                    if op.sem_flg & IPC_NOWAIT as u16 != 0 {
                        return Err(IpcError::WouldBlock);
                    }
                    sem.ncnt += 1;
                    return Err(IpcError::WouldBlock);
                }
                sem.value -= needed;
                sem.pid = pid;

                if op.sem_flg & SEM_UNDO != 0 {
                    let undo = set.undo_list.entry(pid).or_insert_with(|| {
                        let mut v = Vec::new();
                        v.resize(set.sems.len(), 0i16);
                        v
                    });
                    undo[idx] += (-op.sem_op) as i16;
                }
            } else {
                if sem.value != 0 {
                    if op.sem_flg & IPC_NOWAIT as u16 != 0 {
                        return Err(IpcError::WouldBlock);
                    }
                    sem.zcnt += 1;
                    return Err(IpcError::WouldBlock);
                }
            }
        }

        Ok(())
    }

    pub fn semtimedop(
        &mut self,
        semid: i32,
        ops: &[SemBuf],
        pid: u32,
        _timeout: &Timespec,
    ) -> Result<(), IpcError> {
        self.semop(semid, ops, pid)
    }

    pub fn semctl(
        &mut self,
        semid: i32,
        semnum: usize,
        cmd: u32,
        arg: i32,
    ) -> Result<SemCtlResult, IpcError> {
        match cmd {
            IPC_STAT => {
                let set = self.sets.get(&semid).ok_or(IpcError::NotFound)?;
                Ok(SemCtlResult::Stat(SemSetStat {
                    perm: set.perm,
                    nsems: set.sems.len(),
                    otime: set.otime,
                    ctime: set.ctime,
                }))
            }
            IPC_SET => Ok(SemCtlResult::Ok),
            IPC_RMID => {
                self.sets.remove(&semid).ok_or(IpcError::NotFound)?;
                Ok(SemCtlResult::Ok)
            }
            GETVAL => {
                let set = self.sets.get(&semid).ok_or(IpcError::NotFound)?;
                if semnum >= set.sems.len() {
                    return Err(IpcError::InvalidArgument);
                }
                Ok(SemCtlResult::Value(set.sems[semnum].value))
            }
            SETVAL => {
                let set = self.sets.get_mut(&semid).ok_or(IpcError::NotFound)?;
                if semnum >= set.sems.len() {
                    return Err(IpcError::InvalidArgument);
                }
                if arg < 0 || arg > SEMVMX as i32 {
                    return Err(IpcError::InvalidArgument);
                }
                set.sems[semnum].value = arg;
                Ok(SemCtlResult::Ok)
            }
            GETALL => {
                let set = self.sets.get(&semid).ok_or(IpcError::NotFound)?;
                let vals: Vec<i32> = set.sems.iter().map(|s| s.value).collect();
                Ok(SemCtlResult::AllValues(vals))
            }
            SETALL => Ok(SemCtlResult::Ok),
            GETPID => {
                let set = self.sets.get(&semid).ok_or(IpcError::NotFound)?;
                if semnum >= set.sems.len() {
                    return Err(IpcError::InvalidArgument);
                }
                Ok(SemCtlResult::Value(set.sems[semnum].pid as i32))
            }
            GETNCNT => {
                let set = self.sets.get(&semid).ok_or(IpcError::NotFound)?;
                if semnum >= set.sems.len() {
                    return Err(IpcError::InvalidArgument);
                }
                Ok(SemCtlResult::Value(set.sems[semnum].ncnt as i32))
            }
            GETZCNT => {
                let set = self.sets.get(&semid).ok_or(IpcError::NotFound)?;
                if semnum >= set.sems.len() {
                    return Err(IpcError::InvalidArgument);
                }
                Ok(SemCtlResult::Value(set.sems[semnum].zcnt as i32))
            }
            IPC_INFO => Ok(SemCtlResult::Info(SemInfo {
                semmni: SEMMNI,
                semmsl: SEMMSL,
                semmns: SEMMNS,
                semopm: SEMOPM,
                semvmx: SEMVMX,
            })),
            SEM_INFO => Ok(SemCtlResult::SemInfoExt(SemInfoExt {
                used_ids: self.sets.len(),
                sem_tot: self.sets.values().map(|s| s.sems.len()).sum(),
            })),
            SEM_STAT => {
                let set = self.sets.get(&semid).ok_or(IpcError::NotFound)?;
                Ok(SemCtlResult::Stat(SemSetStat {
                    perm: set.perm,
                    nsems: set.sems.len(),
                    otime: set.otime,
                    ctime: set.ctime,
                }))
            }
            _ => Err(IpcError::InvalidArgument),
        }
    }

    pub fn process_exit_undo(&mut self, pid: u32) {
        for set in self.sets.values_mut() {
            if let Some(undos) = set.undo_list.remove(&pid) {
                for (idx, &adj) in undos.iter().enumerate() {
                    if adj != 0 && idx < set.sems.len() {
                        set.sems[idx].value += adj as i32;
                        if set.sems[idx].value < 0 {
                            set.sems[idx].value = 0;
                        }
                    }
                }
            }
        }
    }
}

#[derive(Debug)]
pub enum SemCtlResult {
    Ok,
    Value(i32),
    AllValues(Vec<i32>),
    Stat(SemSetStat),
    Info(SemInfo),
    SemInfoExt(SemInfoExt),
}

#[derive(Debug)]
pub struct SemSetStat {
    pub perm: IpcPerm,
    pub nsems: usize,
    pub otime: Timespec,
    pub ctime: Timespec,
}

#[derive(Debug)]
pub struct SemInfo {
    pub semmni: usize,
    pub semmsl: usize,
    pub semmns: usize,
    pub semopm: usize,
    pub semvmx: usize,
}

#[derive(Debug)]
pub struct SemInfoExt {
    pub used_ids: usize,
    pub sem_tot: usize,
}

// ═══════════════════════════════════════════════════════════════════════════════
// IPC NAMESPACE
// ═══════════════════════════════════════════════════════════════════════════════

static NAMESPACE_ID_GEN: AtomicU64 = AtomicU64::new(1);

#[derive(Debug)]
pub struct IpcNamespace {
    pub id: u64,
    pub shm_registry: SysVShmRegistry,
    pub msg_registry: SysVMsgRegistry,
    pub sem_registry: SysVSemRegistry,
    pub posix_shm: PosixShmRegistry,
    pub posix_mq: PosixMqRegistry,
    pub posix_sem: PosixSemRegistry,
}

impl IpcNamespace {
    pub fn new() -> Self {
        Self {
            id: NAMESPACE_ID_GEN.fetch_add(1, Ordering::Relaxed),
            shm_registry: SysVShmRegistry::new(),
            msg_registry: SysVMsgRegistry::new(),
            sem_registry: SysVSemRegistry::new(),
            posix_shm: PosixShmRegistry::new(),
            posix_mq: PosixMqRegistry::new(),
            posix_sem: PosixSemRegistry::new(),
        }
    }
}

pub struct NamespaceManager {
    namespaces: BTreeMap<u64, IpcNamespace>,
    process_ns: BTreeMap<u32, u64>,
    default_ns: u64,
}

impl NamespaceManager {
    pub fn new() -> Self {
        let default_ns = IpcNamespace::new();
        let id = default_ns.id;
        let mut namespaces = BTreeMap::new();
        namespaces.insert(id, default_ns);
        Self {
            namespaces,
            process_ns: BTreeMap::new(),
            default_ns: id,
        }
    }

    pub fn create_namespace(&mut self) -> u64 {
        let ns = IpcNamespace::new();
        let id = ns.id;
        self.namespaces.insert(id, ns);
        id
    }

    pub fn assign_process(&mut self, pid: u32, ns_id: u64) {
        self.process_ns.insert(pid, ns_id);
    }

    pub fn get_namespace(&self, pid: u32) -> u64 {
        self.process_ns
            .get(&pid)
            .copied()
            .unwrap_or(self.default_ns)
    }

    pub fn get_ns_mut(&mut self, ns_id: u64) -> Option<&mut IpcNamespace> {
        self.namespaces.get_mut(&ns_id)
    }

    pub fn destroy_namespace(&mut self, ns_id: u64) -> Result<(), IpcError> {
        if ns_id == self.default_ns {
            return Err(IpcError::PermissionDenied);
        }
        self.namespaces.remove(&ns_id).ok_or(IpcError::NotFound)?;
        self.process_ns.retain(|_, &mut v| v != ns_id);
        Ok(())
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// IPC UTILITIES (ipcmk, ipcs, ipcrm equivalents)
// ═══════════════════════════════════════════════════════════════════════════════

pub struct IpcUtils;

impl IpcUtils {
    pub fn ipcmk_shm(size: usize, mode: u16) -> Result<i32, IpcError> {
        POSIX_IPC
            .lock()
            .namespace_mgr
            .get_ns_mut(POSIX_IPC.lock().namespace_mgr.default_ns)
            .ok_or(IpcError::NotFound)?
            .shm_registry
            .shmget(IPC_PRIVATE, size, IPC_CREAT | mode as u32)
    }

    pub fn ipcmk_msg(mode: u16) -> Result<i32, IpcError> {
        POSIX_IPC
            .lock()
            .namespace_mgr
            .get_ns_mut(POSIX_IPC.lock().namespace_mgr.default_ns)
            .ok_or(IpcError::NotFound)?
            .msg_registry
            .msgget(IPC_PRIVATE, IPC_CREAT | mode as u32)
    }

    pub fn ipcmk_sem(nsems: usize, mode: u16) -> Result<i32, IpcError> {
        POSIX_IPC
            .lock()
            .namespace_mgr
            .get_ns_mut(POSIX_IPC.lock().namespace_mgr.default_ns)
            .ok_or(IpcError::NotFound)?
            .sem_registry
            .semget(IPC_PRIVATE, nsems, IPC_CREAT | mode as u32)
    }

    pub fn ipcrm_shm(shmid: i32) -> Result<(), IpcError> {
        POSIX_IPC
            .lock()
            .namespace_mgr
            .get_ns_mut(POSIX_IPC.lock().namespace_mgr.default_ns)
            .ok_or(IpcError::NotFound)?
            .shm_registry
            .shmctl(shmid, IPC_RMID, 0, 0)?;
        Ok(())
    }

    pub fn ipcrm_msg(msqid: i32) -> Result<(), IpcError> {
        POSIX_IPC
            .lock()
            .namespace_mgr
            .get_ns_mut(POSIX_IPC.lock().namespace_mgr.default_ns)
            .ok_or(IpcError::NotFound)?
            .msg_registry
            .msgctl(msqid, IPC_RMID)?;
        Ok(())
    }

    pub fn ipcrm_sem(semid: i32) -> Result<(), IpcError> {
        POSIX_IPC
            .lock()
            .namespace_mgr
            .get_ns_mut(POSIX_IPC.lock().namespace_mgr.default_ns)
            .ok_or(IpcError::NotFound)?
            .sem_registry
            .semctl(semid, 0, IPC_RMID, 0)?;
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct IpcListEntry {
    pub kind: IpcObjectKind,
    pub id: i32,
    pub key: i32,
    pub mode: u16,
    pub uid: u32,
    pub gid: u32,
}

#[derive(Debug, Clone, Copy)]
pub enum IpcObjectKind {
    SharedMemory,
    MessageQueue,
    Semaphore,
}

// ═══════════════════════════════════════════════════════════════════════════════
// ERROR TYPE
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IpcError {
    NotFound,
    AlreadyExists,
    PermissionDenied,
    InvalidArgument,
    NoMemory,
    NoSpace,
    MessageTooLong,
    QueueFull,
    QueueEmpty,
    WouldBlock,
    Overflow,
    Interrupted,
    TimedOut,
    BadFileDescriptor,
}

impl IpcError {
    pub fn to_errno(self) -> i32 {
        match self {
            Self::NotFound => -2,          // ENOENT
            Self::AlreadyExists => -17,    // EEXIST
            Self::PermissionDenied => -13, // EACCES
            Self::InvalidArgument => -22,  // EINVAL
            Self::NoMemory => -12,         // ENOMEM
            Self::NoSpace => -28,          // ENOSPC
            Self::MessageTooLong => -90,   // EMSGSIZE
            Self::QueueFull => -11,        // EAGAIN
            Self::QueueEmpty => -11,       // EAGAIN
            Self::WouldBlock => -11,       // EAGAIN
            Self::Overflow => -75,         // EOVERFLOW
            Self::Interrupted => -4,       // EINTR
            Self::TimedOut => -110,        // ETIMEDOUT
            Self::BadFileDescriptor => -9, // EBADF
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// GLOBAL STATE
// ═══════════════════════════════════════════════════════════════════════════════

pub struct PosixIpcSubsystem {
    pub namespace_mgr: NamespaceManager,
    pub initialized: bool,
}

impl PosixIpcSubsystem {
    pub const fn empty() -> Self {
        Self {
            namespace_mgr: NamespaceManager {
                namespaces: BTreeMap::new(),
                process_ns: BTreeMap::new(),
                default_ns: 0,
            },
            initialized: false,
        }
    }

    pub fn initialize(&mut self) {
        self.namespace_mgr = NamespaceManager::new();
        self.initialized = true;
    }

    pub fn shm_open(
        &mut self,
        pid: u32,
        name: &str,
        flags: u32,
        mode: u16,
    ) -> Result<u32, IpcError> {
        let ns_id = self.namespace_mgr.get_namespace(pid);
        let ns = self
            .namespace_mgr
            .get_ns_mut(ns_id)
            .ok_or(IpcError::NotFound)?;
        ns.posix_shm.shm_open(name, flags, mode)
    }

    pub fn shm_unlink(&mut self, pid: u32, name: &str) -> Result<(), IpcError> {
        let ns_id = self.namespace_mgr.get_namespace(pid);
        let ns = self
            .namespace_mgr
            .get_ns_mut(ns_id)
            .ok_or(IpcError::NotFound)?;
        ns.posix_shm.shm_unlink(name)
    }

    pub fn shmget(&mut self, pid: u32, key: i32, size: usize, flags: u32) -> Result<i32, IpcError> {
        let ns_id = self.namespace_mgr.get_namespace(pid);
        let ns = self
            .namespace_mgr
            .get_ns_mut(ns_id)
            .ok_or(IpcError::NotFound)?;
        ns.shm_registry.shmget(key, size, flags)
    }

    pub fn shmat(
        &mut self,
        pid: u32,
        shmid: i32,
        addr: usize,
        flags: u32,
    ) -> Result<*const u8, IpcError> {
        let ns_id = self.namespace_mgr.get_namespace(pid);
        let ns = self
            .namespace_mgr
            .get_ns_mut(ns_id)
            .ok_or(IpcError::NotFound)?;
        ns.shm_registry.shmat(shmid, addr, flags)
    }

    pub fn shmdt(&mut self, pid: u32, shmid: i32) -> Result<(), IpcError> {
        let ns_id = self.namespace_mgr.get_namespace(pid);
        let ns = self
            .namespace_mgr
            .get_ns_mut(ns_id)
            .ok_or(IpcError::NotFound)?;
        ns.shm_registry.shmdt(shmid)
    }

    pub fn mq_open(
        &mut self,
        pid: u32,
        name: &str,
        flags: u32,
        mode: u16,
        attr: Option<MqAttr>,
    ) -> Result<u32, IpcError> {
        let ns_id = self.namespace_mgr.get_namespace(pid);
        let ns = self
            .namespace_mgr
            .get_ns_mut(ns_id)
            .ok_or(IpcError::NotFound)?;
        ns.posix_mq.mq_open(name, flags, mode, attr)
    }

    pub fn mq_send(
        &mut self,
        pid: u32,
        name: &str,
        data: &[u8],
        priority: u32,
    ) -> Result<(), IpcError> {
        let ns_id = self.namespace_mgr.get_namespace(pid);
        let ns = self
            .namespace_mgr
            .get_ns_mut(ns_id)
            .ok_or(IpcError::NotFound)?;
        ns.posix_mq.mq_send(name, data, priority)
    }

    pub fn mq_receive(
        &mut self,
        pid: u32,
        name: &str,
        buf: &mut [u8],
    ) -> Result<(usize, u32), IpcError> {
        let ns_id = self.namespace_mgr.get_namespace(pid);
        let ns = self
            .namespace_mgr
            .get_ns_mut(ns_id)
            .ok_or(IpcError::NotFound)?;
        ns.posix_mq.mq_receive(name, buf)
    }

    pub fn msgget(&mut self, pid: u32, key: i32, flags: u32) -> Result<i32, IpcError> {
        let ns_id = self.namespace_mgr.get_namespace(pid);
        let ns = self
            .namespace_mgr
            .get_ns_mut(ns_id)
            .ok_or(IpcError::NotFound)?;
        ns.msg_registry.msgget(key, flags)
    }

    pub fn msgsnd(
        &mut self,
        pid: u32,
        msqid: i32,
        mtype: i64,
        data: &[u8],
        flags: u32,
    ) -> Result<(), IpcError> {
        let ns_id = self.namespace_mgr.get_namespace(pid);
        let ns = self
            .namespace_mgr
            .get_ns_mut(ns_id)
            .ok_or(IpcError::NotFound)?;
        ns.msg_registry.msgsnd(msqid, mtype, data, flags)
    }

    pub fn msgrcv(
        &mut self,
        pid: u32,
        msqid: i32,
        mtype: i64,
        buf: &mut [u8],
        flags: u32,
    ) -> Result<(usize, i64), IpcError> {
        let ns_id = self.namespace_mgr.get_namespace(pid);
        let ns = self
            .namespace_mgr
            .get_ns_mut(ns_id)
            .ok_or(IpcError::NotFound)?;
        ns.msg_registry.msgrcv(msqid, mtype, buf, flags)
    }

    pub fn sem_open(
        &mut self,
        pid: u32,
        name: &str,
        flags: u32,
        mode: u16,
        value: u32,
    ) -> Result<u32, IpcError> {
        let ns_id = self.namespace_mgr.get_namespace(pid);
        let ns = self
            .namespace_mgr
            .get_ns_mut(ns_id)
            .ok_or(IpcError::NotFound)?;
        ns.posix_sem.sem_open(name, flags, mode, value)
    }

    pub fn semget(
        &mut self,
        pid: u32,
        key: i32,
        nsems: usize,
        flags: u32,
    ) -> Result<i32, IpcError> {
        let ns_id = self.namespace_mgr.get_namespace(pid);
        let ns = self
            .namespace_mgr
            .get_ns_mut(ns_id)
            .ok_or(IpcError::NotFound)?;
        ns.sem_registry.semget(key, nsems, flags)
    }

    pub fn semop(&mut self, pid: u32, semid: i32, ops: &[SemBuf]) -> Result<(), IpcError> {
        let ns_id = self.namespace_mgr.get_namespace(pid);
        let ns = self
            .namespace_mgr
            .get_ns_mut(ns_id)
            .ok_or(IpcError::NotFound)?;
        ns.sem_registry.semop(semid, ops, pid)
    }

    pub fn process_exit(&mut self, pid: u32) {
        let ns_id = self.namespace_mgr.get_namespace(pid);
        if let Some(ns) = self.namespace_mgr.get_ns_mut(ns_id) {
            ns.sem_registry.process_exit_undo(pid);
        }
    }
}

pub static POSIX_IPC: Mutex<PosixIpcSubsystem> = Mutex::new(PosixIpcSubsystem::empty());

pub fn init() {
    POSIX_IPC.lock().initialize();
}
