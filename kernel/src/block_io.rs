#![allow(dead_code)]

extern crate alloc;

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use spin::Mutex;

// ─── Safe-Mode Storage Guard ────────────────────────────────────────────────
//
// When the kernel is built with `--features safe_mode` (or the `xtask --safe`
// shorthand), this static is initialized to `true` at boot and every
// `BlockDevice::write_sector` impl in the tree calls
// `safe_mode_guard_write` first. The guard logs the rejected LBA + length
// (first few times only, to avoid spamming serial when the installer or
// raefs retries) and returns an error so the caller propagates the
// failure without ever touching hardware.
//
// Read paths are completely unaffected, so the kernel still boots, mounts,
// probes, exercises smoketests — but cannot clobber a host OS partition
// on a dev machine where the disk already has Windows or Linux on it.
//
// The default is `cfg!(feature = "safe_mode")` so the flag is purely
// build-time controlled: a non-safe build can never accidentally enable
// safe mode, and a safe build can never accidentally disable it.

pub static SAFE_MODE: AtomicBool = AtomicBool::new(cfg!(feature = "safe_mode"));

/// Cumulative count of writes rejected by `safe_mode_guard_write` since boot.
pub static SAFE_MODE_REJECTS: AtomicU64 = AtomicU64::new(0);

const SAFE_MODE_LOG_LIMIT: u64 = 16;

/// Carveout: even in safe-mode, permit writes to these exact LBA ranges.
/// Used by bootlog_persist to flush the in-RAM serial ring into the
/// DATA clusters of a `BOOTLOG.TXT` file the user pre-created from
/// Windows. The kernel never touches FAT tables or the root directory —
/// it only overwrites the file's already-allocated data clusters, so
/// there is zero risk to the Windows ESP's filesystem metadata.
///
/// A pre-created file may be fragmented, so this is a *list* of
/// (start_lba, sector_count) ranges — one per contiguous run of the
/// file's clusters. A write is permitted only if its entire span falls
/// within one of these ranges; everything else stays blocked.
pub static SAFE_MODE_LOG_CARVEOUT: spin::Mutex<Vec<(u64, u64)>> = spin::Mutex::new(Vec::new());
pub static SAFE_MODE_LOG_WRITES: AtomicU64 = AtomicU64::new(0);

/// Replace the carveout with a fresh set of LBA ranges (the data-cluster
/// runs of the pre-created log file). Clears any previous ranges so a
/// re-init can't accumulate stale entries. The caller guarantees these
/// LBAs belong to a file the kernel owns the contents of — never FAT,
/// never the root dir, never anything Windows boots from.
pub fn set_log_lba_carveout(ranges: Vec<(u64, u64)>) {
    let total: u64 = ranges.iter().map(|(_, c)| *c).sum();
    let n = ranges.len();
    *SAFE_MODE_LOG_CARVEOUT.lock() = ranges;
    crate::serial_println!(
        "[safe-mode] log carveout: {} range(s), {} sectors total writable for bootlog flush",
        n,
        total,
    );
}

#[inline]
fn within_log_carveout(lba: u64, len_bytes: usize) -> bool {
    let ranges = SAFE_MODE_LOG_CARVEOUT.lock();
    if ranges.is_empty() {
        return false;
    }
    let sectors_needed = (len_bytes as u64).div_ceil(512).max(1);
    let end = lba.saturating_add(sectors_needed);
    ranges
        .iter()
        .any(|&(start, count)| lba >= start && end <= start.saturating_add(count))
}

/// Returns `Err` (and logs) when safe mode is on AND the target LBA
/// range is not in the registered log carveout. Otherwise `Ok(())`.
/// Every `BlockDevice::write_sector` impl in the kernel calls this
/// before dispatching to the underlying hardware write.
#[inline]
pub fn safe_mode_guard_write(lba: u64, len: usize, source: &str) -> Result<(), &'static str> {
    // TWO gates, both checked here at the single choke point every BlockDevice
    // write_sector calls (NVMe / AHCI / virtio / USB-MSC):
    //   1. read-only — `writes_enabled()` is false (default OFF; a `--safe`/
    //      safe_mode image never turns it on, so writes are off the WHOLE boot).
    //   2. safe-mode — the `safe_mode` feature's runtime flag.
    // Either active ⇒ block, so a bare-metal smoke boot physically cannot write
    // to a real disk (the protect-an-existing-OS guarantee).
    let read_only = !writes_enabled();
    let safe = SAFE_MODE.load(Ordering::Relaxed);
    if !read_only && !safe {
        return Ok(()); // fast path: writes permitted
    }
    // A gate is active. The bootlog_persist pre-allocated log file is a FIXED,
    // RaeenOS-owned LBA range (baked into the image's ESP) that cannot reach a
    // user partition — allow only it, so a safe boot can still flush its
    // diagnostic log to the stick for retrieval. Every other sector stays blocked.
    if within_log_carveout(lba, len) {
        SAFE_MODE_LOG_WRITES.fetch_add(1, Ordering::Relaxed);
        return Ok(());
    }
    let prior = SAFE_MODE_REJECTS.fetch_add(1, Ordering::Relaxed);
    if prior < SAFE_MODE_LOG_LIMIT {
        crate::serial_println!(
            "[safe-mode] BLOCKED {} write lba={} len={}B ({}, reject #{})",
            source,
            lba,
            len,
            if read_only {
                "storage read-only"
            } else {
                "safe-mode"
            },
            prior + 1,
        );
        if prior + 1 == SAFE_MODE_LOG_LIMIT {
            crate::serial_println!(
                "[safe-mode] further rejects will be silent; see /proc/raeen/safe_mode for the running total",
            );
        }
    }
    Err("storage writes are blocked (safe / read-only)")
}

pub fn safe_mode_enabled() -> bool {
    SAFE_MODE.load(Ordering::Relaxed)
}

pub fn safe_mode_rejects() -> u64 {
    SAFE_MODE_REJECTS.load(Ordering::Relaxed)
}

// ─── Block Errors ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockError {
    IoError,
    DeviceNotFound,
    InvalidSector,
    OutOfRange,
    ReadOnly,
    DeviceBusy,
    Timeout,
    BadChecksum,
    InvalidPartitionTable,
    UnsupportedFeature,
    MediaChanged,
    MediaNotPresent,
    HardwareFailure,
    ProtocolError,
    QueueFull,
}

impl core::fmt::Display for BlockError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::IoError => write!(f, "I/O error"),
            Self::DeviceNotFound => write!(f, "device not found"),
            Self::InvalidSector => write!(f, "invalid sector"),
            Self::OutOfRange => write!(f, "out of range"),
            Self::ReadOnly => write!(f, "device is read-only"),
            Self::DeviceBusy => write!(f, "device busy"),
            Self::Timeout => write!(f, "operation timed out"),
            Self::BadChecksum => write!(f, "bad checksum"),
            Self::InvalidPartitionTable => write!(f, "invalid partition table"),
            Self::UnsupportedFeature => write!(f, "unsupported feature"),
            Self::MediaChanged => write!(f, "media changed"),
            Self::MediaNotPresent => write!(f, "media not present"),
            Self::HardwareFailure => write!(f, "hardware failure"),
            Self::ProtocolError => write!(f, "protocol error"),
            Self::QueueFull => write!(f, "queue full"),
        }
    }
}

// ─── Bio (Block I/O Request) ────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BioOp {
    Read,
    Write,
    Flush,
    Discard,
    WriteZeroes,
    SecureErase,
}

#[derive(Debug, Clone, Copy)]
pub struct BioFlags {
    pub sync: bool,
    pub meta: bool,
    pub prio: bool,
    pub fua: bool,
    pub preflush: bool,
}

impl Default for BioFlags {
    fn default() -> Self {
        Self {
            sync: false,
            meta: false,
            prio: false,
            fua: false,
            preflush: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum BioPriority {
    Idle = 0,
    Background = 1,
    Normal = 2,
    Foreground = 3,
    RealTime = 4,
}

pub struct Bio {
    pub sector: u64,
    pub sector_count: u32,
    pub operation: BioOp,
    pub data: Vec<u8>,
    pub flags: BioFlags,
    pub priority: BioPriority,
    pub submitted_at: u64,
    pub completed: bool,
    pub error: Option<BlockError>,
    pub callback: Option<fn(&Bio)>,
}

impl Bio {
    pub fn new_read(sector: u64, count: u32) -> Self {
        Self {
            sector,
            sector_count: count,
            operation: BioOp::Read,
            data: Vec::new(),
            flags: BioFlags::default(),
            priority: BioPriority::Normal,
            submitted_at: 0,
            completed: false,
            error: None,
            callback: None,
        }
    }

    pub fn new_write(sector: u64, data: Vec<u8>, sector_size: u32) -> Self {
        let count = (data.len() as u32 + sector_size - 1) / sector_size;
        Self {
            sector,
            sector_count: count,
            operation: BioOp::Write,
            data,
            flags: BioFlags::default(),
            priority: BioPriority::Normal,
            submitted_at: 0,
            completed: false,
            error: None,
            callback: None,
        }
    }

    pub fn new_flush() -> Self {
        Self {
            sector: 0,
            sector_count: 0,
            operation: BioOp::Flush,
            data: Vec::new(),
            flags: BioFlags {
                sync: true,
                ..BioFlags::default()
            },
            priority: BioPriority::Foreground,
            submitted_at: 0,
            completed: false,
            error: None,
            callback: None,
        }
    }

    pub fn new_discard(sector: u64, count: u32) -> Self {
        Self {
            sector,
            sector_count: count,
            operation: BioOp::Discard,
            data: Vec::new(),
            flags: BioFlags::default(),
            priority: BioPriority::Background,
            submitted_at: 0,
            completed: false,
            error: None,
            callback: None,
        }
    }

    pub fn end_sector(&self) -> u64 {
        self.sector + self.sector_count as u64
    }

    pub fn is_adjacent(&self, other: &Bio) -> bool {
        self.operation == other.operation
            && (self.end_sector() == other.sector || other.end_sector() == self.sector)
    }

    pub fn complete(&mut self, error: Option<BlockError>) {
        self.completed = true;
        self.error = error;
        if let Some(cb) = self.callback {
            cb(self);
        }
    }
}

// ─── I/O Scheduler Trait ────────────────────────────────────────────────────

pub trait IoScheduler: Send {
    fn enqueue(&mut self, bio: Bio);
    fn dequeue(&mut self) -> Option<Bio>;
    fn peek(&self) -> Option<&Bio>;
    fn len(&self) -> usize;
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
    fn merge(&mut self, bio: &Bio) -> bool;
}

// ─── Noop Scheduler ─────────────────────────────────────────────────────────

pub struct NoopScheduler {
    queue: VecDeque<Bio>,
}

impl NoopScheduler {
    pub fn new() -> Self {
        Self {
            queue: VecDeque::new(),
        }
    }
}

impl IoScheduler for NoopScheduler {
    fn enqueue(&mut self, bio: Bio) {
        self.queue.push_back(bio);
    }

    fn dequeue(&mut self) -> Option<Bio> {
        self.queue.pop_front()
    }

    fn peek(&self) -> Option<&Bio> {
        self.queue.front()
    }

    fn len(&self) -> usize {
        self.queue.len()
    }

    fn merge(&mut self, bio: &Bio) -> bool {
        for existing in self.queue.iter() {
            if existing.is_adjacent(bio) {
                return true;
            }
        }
        false
    }
}

// ─── Deadline Scheduler ─────────────────────────────────────────────────────

pub struct DeadlineScheduler {
    read_queue: BTreeMap<u64, Bio>,
    write_queue: BTreeMap<u64, Bio>,
    read_fifo: VecDeque<u64>,
    write_fifo: VecDeque<u64>,
    read_expire_ms: u64,
    write_expire_ms: u64,
    writes_starved: u32,
    fifo_batch: u32,
    dispatched: u32,
}

impl DeadlineScheduler {
    pub fn new() -> Self {
        Self {
            read_queue: BTreeMap::new(),
            write_queue: BTreeMap::new(),
            read_fifo: VecDeque::new(),
            write_fifo: VecDeque::new(),
            read_expire_ms: 500,
            write_expire_ms: 5000,
            writes_starved: 0,
            fifo_batch: 16,
            dispatched: 0,
        }
    }

    fn has_expired_reads(&self, now_ms: u64) -> bool {
        if let Some(&sector) = self.read_fifo.front() {
            if let Some(bio) = self.read_queue.get(&sector) {
                return now_ms.saturating_sub(bio.submitted_at) >= self.read_expire_ms;
            }
        }
        false
    }

    fn has_expired_writes(&self, now_ms: u64) -> bool {
        if let Some(&sector) = self.write_fifo.front() {
            if let Some(bio) = self.write_queue.get(&sector) {
                return now_ms.saturating_sub(bio.submitted_at) >= self.write_expire_ms;
            }
        }
        false
    }

    fn dispatch_read_fifo(&mut self) -> Option<Bio> {
        if let Some(sector) = self.read_fifo.pop_front() {
            return self.read_queue.remove(&sector);
        }
        None
    }

    fn dispatch_write_fifo(&mut self) -> Option<Bio> {
        if let Some(sector) = self.write_fifo.pop_front() {
            return self.write_queue.remove(&sector);
        }
        None
    }

    fn dispatch_sorted_read(&mut self) -> Option<Bio> {
        let key = *self.read_queue.keys().next()?;
        let bio = self.read_queue.remove(&key)?;
        self.read_fifo.retain(|&s| s != key);
        Some(bio)
    }

    fn dispatch_sorted_write(&mut self) -> Option<Bio> {
        let key = *self.write_queue.keys().next()?;
        let bio = self.write_queue.remove(&key)?;
        self.write_fifo.retain(|&s| s != key);
        Some(bio)
    }
}

impl IoScheduler for DeadlineScheduler {
    fn enqueue(&mut self, bio: Bio) {
        let sector = bio.sector;
        match bio.operation {
            BioOp::Read => {
                self.read_fifo.push_back(sector);
                self.read_queue.insert(sector, bio);
            }
            BioOp::Write | BioOp::WriteZeroes => {
                self.write_fifo.push_back(sector);
                self.write_queue.insert(sector, bio);
            }
            _ => {
                self.read_fifo.push_back(sector);
                self.read_queue.insert(sector, bio);
            }
        }
    }

    fn dequeue(&mut self) -> Option<Bio> {
        if self.read_queue.is_empty() && self.write_queue.is_empty() {
            return None;
        }

        let now_ms = self.dispatched as u64 * 10; // approximate

        if self.has_expired_reads(now_ms) {
            self.dispatched += 1;
            self.writes_starved = 0;
            return self.dispatch_read_fifo();
        }

        if self.has_expired_writes(now_ms) {
            self.dispatched += 1;
            self.writes_starved = 0;
            return self.dispatch_write_fifo();
        }

        // Prefer reads unless writes are starved
        if !self.read_queue.is_empty() && self.writes_starved < 2 {
            self.dispatched += 1;
            if !self.write_queue.is_empty() {
                self.writes_starved += 1;
            }
            return self.dispatch_sorted_read();
        }

        if !self.write_queue.is_empty() {
            self.dispatched += 1;
            self.writes_starved = 0;
            return self.dispatch_sorted_write();
        }

        self.dispatch_sorted_read()
    }

    fn peek(&self) -> Option<&Bio> {
        if let Some((&_k, bio)) = self.read_queue.iter().next() {
            return Some(bio);
        }
        if let Some((&_k, bio)) = self.write_queue.iter().next() {
            return Some(bio);
        }
        None
    }

    fn len(&self) -> usize {
        self.read_queue.len() + self.write_queue.len()
    }

    fn merge(&mut self, bio: &Bio) -> bool {
        let queue = match bio.operation {
            BioOp::Read => &self.read_queue,
            _ => &self.write_queue,
        };
        for (_sector, existing) in queue.iter() {
            if existing.is_adjacent(bio) {
                return true;
            }
        }
        false
    }
}

// ─── CFQ (Completely Fair Queueing) Scheduler ───────────────────────────────

pub struct CfqScheduler {
    per_process: BTreeMap<u64, VecDeque<Bio>>,
    current_pid: Option<u64>,
    time_slice_ms: u64,
    slice_start: u64,
    round_robin: VecDeque<u64>,
    dispatched_in_slice: u32,
    max_dispatch_per_slice: u32,
}

impl CfqScheduler {
    pub fn new() -> Self {
        Self {
            per_process: BTreeMap::new(),
            current_pid: None,
            time_slice_ms: 100,
            slice_start: 0,
            round_robin: VecDeque::new(),
            dispatched_in_slice: 0,
            max_dispatch_per_slice: 8,
        }
    }

    fn select_next_process(&mut self) {
        if let Some(pid) = self.round_robin.pop_front() {
            if self.per_process.contains_key(&pid) && !self.per_process[&pid].is_empty() {
                self.current_pid = Some(pid);
                self.round_robin.push_back(pid);
                self.dispatched_in_slice = 0;
                self.slice_start += self.time_slice_ms;
                return;
            }
        }

        // Rebuild round-robin from active processes
        self.round_robin.clear();
        for (&pid, queue) in self.per_process.iter() {
            if !queue.is_empty() {
                self.round_robin.push_back(pid);
            }
        }

        if let Some(&pid) = self.round_robin.front() {
            self.current_pid = Some(pid);
            self.dispatched_in_slice = 0;
            self.slice_start += self.time_slice_ms;
        } else {
            self.current_pid = None;
        }
    }

    fn pid_for_bio(_bio: &Bio) -> u64 {
        0 // Default PID; real implementation would extract from task context
    }
}

impl IoScheduler for CfqScheduler {
    fn enqueue(&mut self, bio: Bio) {
        let pid = Self::pid_for_bio(&bio);
        let queue = self.per_process.entry(pid).or_insert_with(VecDeque::new);
        queue.push_back(bio);

        if !self.round_robin.contains(&pid) {
            self.round_robin.push_back(pid);
        }

        if self.current_pid.is_none() {
            self.current_pid = Some(pid);
            self.dispatched_in_slice = 0;
        }
    }

    fn dequeue(&mut self) -> Option<Bio> {
        loop {
            let pid = self.current_pid?;

            if self.dispatched_in_slice >= self.max_dispatch_per_slice {
                self.select_next_process();
                if self.current_pid.is_none() {
                    return None;
                }
                continue;
            }

            if let Some(queue) = self.per_process.get_mut(&pid) {
                if let Some(bio) = queue.pop_front() {
                    self.dispatched_in_slice += 1;
                    if queue.is_empty() {
                        self.per_process.remove(&pid);
                        self.round_robin.retain(|&p| p != pid);
                        self.select_next_process();
                    }
                    return Some(bio);
                }
            }

            self.select_next_process();
            if self.current_pid.is_none() {
                return None;
            }
        }
    }

    fn peek(&self) -> Option<&Bio> {
        let pid = self.current_pid?;
        self.per_process.get(&pid)?.front()
    }

    fn len(&self) -> usize {
        self.per_process.values().map(|q| q.len()).sum()
    }

    fn merge(&mut self, bio: &Bio) -> bool {
        for queue in self.per_process.values() {
            for existing in queue.iter() {
                if existing.is_adjacent(bio) {
                    return true;
                }
            }
        }
        false
    }
}

// ─── Storage write safety (fail-safe; protects the user's real disk) ─────────
//
// SAFETY-CRITICAL: writes to a real internal disk during a hardware bring-up
// boot would corrupt the host OS (e.g. a Windows ESP). This flag defaults to
// FALSE so the kernel CANNOT write to ANY block device until something
// explicitly enables it, and it is enforced for EVERY device at the single
// `safe_mode_guard_write` choke point (which NVMe/AHCI/virtio/USB-MSC all call).
// Current policy (hardened 2026-06-25 — the "FOLLOW-UP (stronger)" below, now
// done): a `--safe`/safe_mode image NEVER enables writes (read-only the whole
// boot). A STANDARD build enables writes at boot ONLY when the hardware profile
// is QEMU (throwaway disks; the CI smoketests + installer dry-runs need them) —
// on REAL hardware a standard image stays READ-ONLY at boot, so even the install
// image cannot touch the machine's disk until the user-confirmed installer
// flow (`installer_worker_entry` / `maybe_run_triggered_install`) brackets a
// `set_writes_enabled(true)` window around the actual destructive write and
// closes it again. This converts "only the wizard UX + a marker file stand
// between a normal boot and a wiped Windows partition" into "the machine is
// structurally read-only until the user clicks Install." `WRITE_WINDOW_OPENS`
// counts how many times the window was opened so a boot smoketest + procfs can
// prove the standard image booted read-only and the window is operator-visible.
// `AtomicBool` already imported at the top of this file alongside SAFE_MODE;
// re-import only the local alias used by code below.
use core::sync::atomic::Ordering as AtomicOrdering;
static STORAGE_WRITE_ENABLED: AtomicBool = AtomicBool::new(false);
static WRITE_WINDOW_OPENS: AtomicU64 = AtomicU64::new(0);

/// True if block-device writes are permitted in this boot.
#[inline]
pub fn writes_enabled() -> bool {
    STORAGE_WRITE_ENABLED.load(AtomicOrdering::Relaxed)
}

/// Number of times the write window has been opened (`set_writes_enabled(true)`)
/// this boot. 0 on a correctly-read-only standard boot on real hardware until an
/// install is confirmed; non-zero in QEMU (boot enable) or after a confirmed
/// install bracket. Surfaced for the install-safety smoketest + `/proc/raeen`.
#[inline]
pub fn write_window_opens() -> u64 {
    WRITE_WINDOW_OPENS.load(AtomicOrdering::Relaxed)
}

/// Pure policy decision (testable without hardware): may a STANDARD build enable
/// block writes at BOOT for this hardware family? Only QEMU (throwaway virtual
/// disks) — every real family stays read-only at boot until a confirmed install.
#[inline]
#[must_use]
pub fn boot_writes_default_on(family: crate::hardware_profile::HardwareFamily) -> bool {
    matches!(family, crate::hardware_profile::HardwareFamily::QemuVirtual)
}

/// Enable or disable block-device writes. Called at boot (QEMU only), and by the
/// user-confirmed installer flow on real hardware (bracketed around the install).
pub fn set_writes_enabled(on: bool) {
    if on {
        WRITE_WINDOW_OPENS.fetch_add(1, AtomicOrdering::Relaxed);
    }
    STORAGE_WRITE_ENABLED.store(on, AtomicOrdering::Relaxed);
}

/// Guard for every `write_sector` impl. Returns `Err` (a safe, non-destructive
/// failure the smoketests already tolerate) when storage is read-only.
#[inline]
pub fn write_guard() -> Result<(), &'static str> {
    if writes_enabled() {
        Ok(())
    } else {
        Err("storage is read-only (safe mode): write refused")
    }
}

// ─── Block Device Trait ──────────────────────────────────────────────────────

pub trait BlockDevice: Send {
    fn read_sector(&self, lba: u64, buf: &mut [u8]) -> Result<(), &'static str>;
    fn write_sector(&self, lba: u64, buf: &[u8]) -> Result<(), &'static str>;
    fn sector_size(&self) -> usize;
    fn total_sectors(&self) -> u64;

    /// Force the device's volatile write cache to non-volatile media.
    /// Default no-op for devices without a cache (RAM disks, etc.).
    ///
    /// **Critical for bare-metal power-cycle persistence.** A drive with
    /// a Volatile Write Cache buffers `write_sector` data in DRAM; on a
    /// power-cycle (not a clean shutdown) those buffered sectors are lost
    /// before reaching NAND. The bootlog-persist flush calls this after
    /// writing so the log actually survives the reboot. NVMe: FLUSH
    /// (opcode 0x00). AHCI: FLUSH CACHE EXT. virtio-blk: VIRTIO_BLK_T_FLUSH.
    fn flush_cache(&self) -> Result<(), &'static str> {
        Ok(())
    }
}

/// Global active block device — can be virtio-blk, NVMe, or AHCI.
pub static ACTIVE_BLOCK_DEVICE: Mutex<Option<Box<dyn BlockDevice>>> = Mutex::new(None);

/// LBA added to every sector access (GPT partition offset for root mount).
pub static ROOT_PARTITION_LBA: Mutex<u64> = Mutex::new(0);

pub fn set_active_block_device(dev: Box<dyn BlockDevice>) {
    *ACTIVE_BLOCK_DEVICE.lock() = Some(dev);
}

/// View of the active device starting at `sector_offset` (partition base LBA).
pub struct PartitionDevice {
    inner: Box<dyn BlockDevice>,
    sector_offset: u64,
}

impl PartitionDevice {
    pub fn new(inner: Box<dyn BlockDevice>, sector_offset: u64) -> Self {
        Self {
            inner,
            sector_offset,
        }
    }
}

impl BlockDevice for PartitionDevice {
    fn read_sector(&self, lba: u64, buf: &mut [u8]) -> Result<(), &'static str> {
        self.inner
            .read_sector(self.sector_offset.saturating_add(lba), buf)
    }

    fn write_sector(&self, lba: u64, buf: &[u8]) -> Result<(), &'static str> {
        self.inner
            .write_sector(self.sector_offset.saturating_add(lba), buf)
    }

    fn sector_size(&self) -> usize {
        self.inner.sector_size()
    }

    fn total_sectors(&self) -> u64 {
        self.inner
            .total_sectors()
            .saturating_sub(self.sector_offset)
    }

    fn flush_cache(&self) -> Result<(), &'static str> {
        self.inner.flush_cache()
    }
}

// ─── Block Device Info (Registration Metadata) ──────────────────────────────

pub struct BlockDeviceInfo {
    pub name: String,
    pub major: u16,
    pub minor: u16,
    pub sector_size: u32,
    pub total_sectors: u64,
    pub read_only: bool,
    pub removable: bool,
    pub rotational: bool,
    pub queue_depth: u32,
    pub model: String,
    pub serial: String,
    pub firmware: String,
    pub partitions: Vec<Partition>,
    pub stats: BlockDeviceStats,
    pub scheduler: Box<dyn IoScheduler + Send>,
}

impl BlockDeviceInfo {
    pub fn new(name: String, major: u16, minor: u16) -> Self {
        Self {
            name,
            major,
            minor,
            sector_size: 512,
            total_sectors: 0,
            read_only: false,
            removable: false,
            rotational: false,
            queue_depth: 32,
            model: String::new(),
            serial: String::new(),
            firmware: String::new(),
            partitions: Vec::new(),
            stats: BlockDeviceStats::new(),
            scheduler: Box::new(NoopScheduler::new()),
        }
    }

    pub fn capacity_bytes(&self) -> u64 {
        self.total_sectors * self.sector_size as u64
    }

    pub fn capacity_mb(&self) -> u64 {
        self.capacity_bytes() / (1024 * 1024)
    }

    pub fn capacity_gb(&self) -> u64 {
        self.capacity_bytes() / (1024 * 1024 * 1024)
    }

    pub fn submit_bio(&mut self, bio: Bio) -> Result<(), BlockError> {
        if self.read_only
            && matches!(
                bio.operation,
                BioOp::Write | BioOp::WriteZeroes | BioOp::SecureErase
            )
        {
            return Err(BlockError::ReadOnly);
        }

        if bio.sector + bio.sector_count as u64 > self.total_sectors {
            return Err(BlockError::OutOfRange);
        }

        if self.scheduler.len() >= self.queue_depth as usize {
            return Err(BlockError::QueueFull);
        }

        self.scheduler.enqueue(bio);
        Ok(())
    }

    pub fn process_queue(&mut self) -> Vec<Bio> {
        let mut completed = Vec::new();
        while let Some(mut bio) = self.scheduler.dequeue() {
            match bio.operation {
                BioOp::Read => {
                    self.stats.reads_completed += 1;
                    self.stats.sectors_read += bio.sector_count as u64;
                }
                BioOp::Write | BioOp::WriteZeroes => {
                    self.stats.writes_completed += 1;
                    self.stats.sectors_written += bio.sector_count as u64;
                }
                _ => {}
            }
            bio.complete(None);
            completed.push(bio);
        }
        completed
    }

    pub fn set_scheduler(&mut self, sched_type: SchedulerType) {
        let new_sched: Box<dyn IoScheduler + Send> = match sched_type {
            SchedulerType::Noop => Box::new(NoopScheduler::new()),
            SchedulerType::Deadline => Box::new(DeadlineScheduler::new()),
            SchedulerType::Cfq => Box::new(CfqScheduler::new()),
        };
        self.scheduler = new_sched;
    }

    pub fn device_id(&self) -> u64 {
        ((self.major as u64) << 16) | self.minor as u64
    }
}

#[derive(Debug, Clone, Copy)]
pub enum SchedulerType {
    Noop,
    Deadline,
    Cfq,
}

// ─── Partition ──────────────────────────────────────────────────────────────

pub struct Partition {
    pub number: u8,
    pub start_sector: u64,
    pub sector_count: u64,
    pub partition_type: PartitionType,
    pub name: String,
    pub filesystem: Option<String>,
    pub bootable: bool,
    pub uuid: [u8; 16],
}

impl Partition {
    pub fn size_bytes(&self, sector_size: u32) -> u64 {
        self.sector_count * sector_size as u64
    }

    pub fn end_sector(&self) -> u64 {
        self.start_sector + self.sector_count
    }

    pub fn contains_sector(&self, sector: u64) -> bool {
        sector >= self.start_sector && sector < self.end_sector()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartitionType {
    Efi,
    LinuxFs,
    LinuxSwap,
    LinuxLvm,
    WindowsNtfs,
    WindowsFat32,
    WindowsFat16,
    AppleHfs,
    RaeFs,
    Unknown(u8),
}

impl PartitionType {
    pub fn from_mbr_type(t: u8) -> Self {
        match t {
            0xEF => Self::Efi,
            0x83 => Self::LinuxFs,
            0x82 => Self::LinuxSwap,
            0x8E => Self::LinuxLvm,
            0x07 => Self::WindowsNtfs,
            0x0C | 0x0B => Self::WindowsFat32,
            0x04 | 0x06 | 0x0E => Self::WindowsFat16,
            0xAF => Self::AppleHfs,
            0xDA => Self::RaeFs,
            _ => Self::Unknown(t),
        }
    }

    pub fn from_gpt_guid(guid: &[u8; 16]) -> Self {
        // EFI System Partition: C12A7328-F81F-11D2-BA4B-00A0C93EC93B
        const EFI_GUID: [u8; 16] = [
            0x28, 0x73, 0x2A, 0xC1, 0x1F, 0xF8, 0xD2, 0x11, 0xBA, 0x4B, 0x00, 0xA0, 0xC9, 0x3E,
            0xC9, 0x3B,
        ];
        // Linux filesystem: 0FC63DAF-8483-4772-8E79-3D69D8477DE4
        const LINUX_FS_GUID: [u8; 16] = [
            0xAF, 0x3D, 0xC6, 0x0F, 0x83, 0x84, 0x72, 0x47, 0x8E, 0x79, 0x3D, 0x69, 0xD8, 0x47,
            0x7D, 0xE4,
        ];
        // Linux swap: 0657FD6D-A4AB-43C4-84E5-0933C84B4F4F
        const LINUX_SWAP_GUID: [u8; 16] = [
            0x6D, 0xFD, 0x57, 0x06, 0xAB, 0xA4, 0xC4, 0x43, 0x84, 0xE5, 0x09, 0x33, 0xC8, 0x4B,
            0x4F, 0x4F,
        ];
        // Microsoft basic data: EBD0A0A2-B9E5-4433-87C0-68B6B72699C7
        const NTFS_GUID: [u8; 16] = [
            0xA2, 0xA0, 0xD0, 0xEB, 0xE5, 0xB9, 0x33, 0x44, 0x87, 0xC0, 0x68, 0xB6, 0xB7, 0x26,
            0x99, 0xC7,
        ];
        // RaeFS (provisional): 52414546-534F-2147-5241-45454E4F5321  "RaeFS!RaeenOS!"
        const RAEFS_GUID: [u8; 16] = [
            0x46, 0x45, 0x41, 0x52, 0x4F, 0x53, 0x47, 0x21, 0x41, 0x52, 0x45, 0x45, 0x4E, 0x4F,
            0x53, 0x21,
        ];

        if guid == &EFI_GUID {
            Self::Efi
        } else if guid == &RAEFS_GUID {
            Self::RaeFs
        } else if guid == &LINUX_FS_GUID {
            Self::LinuxFs
        } else if guid == &LINUX_SWAP_GUID {
            Self::LinuxSwap
        } else if guid == &NTFS_GUID {
            Self::WindowsNtfs
        } else {
            Self::Unknown(0xFF)
        }
    }
}

// ─── Block Device Stats ─────────────────────────────────────────────────────

pub struct BlockDeviceStats {
    pub reads_completed: u64,
    pub reads_merged: u64,
    pub sectors_read: u64,
    pub read_time_ms: u64,
    pub writes_completed: u64,
    pub writes_merged: u64,
    pub sectors_written: u64,
    pub write_time_ms: u64,
    pub io_in_progress: u32,
    pub io_time_ms: u64,
    pub weighted_io_time_ms: u64,
}

impl BlockDeviceStats {
    pub fn new() -> Self {
        Self {
            reads_completed: 0,
            reads_merged: 0,
            sectors_read: 0,
            read_time_ms: 0,
            writes_completed: 0,
            writes_merged: 0,
            sectors_written: 0,
            write_time_ms: 0,
            io_in_progress: 0,
            io_time_ms: 0,
            weighted_io_time_ms: 0,
        }
    }

    pub fn reset(&mut self) {
        *self = Self::new();
    }

    pub fn total_ios(&self) -> u64 {
        self.reads_completed + self.writes_completed
    }

    pub fn read_ratio(&self) -> f32 {
        let total = self.total_ios();
        if total == 0 {
            return 0.0;
        }
        self.reads_completed as f32 / total as f32
    }

    pub fn avg_read_time_ms(&self) -> f32 {
        if self.reads_completed == 0 {
            return 0.0;
        }
        self.read_time_ms as f32 / self.reads_completed as f32
    }

    pub fn avg_write_time_ms(&self) -> f32 {
        if self.writes_completed == 0 {
            return 0.0;
        }
        self.write_time_ms as f32 / self.writes_completed as f32
    }
}

// ─── Partition Table Detection & Parsing ────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartitionTableType {
    Gpt,
    Mbr,
    None,
    Unknown,
}

const MBR_SIGNATURE: u16 = 0xAA55;
const GPT_SIGNATURE: u64 = 0x5452415020494645; // "EFI PART"
const GPT_HEADER_LBA: u64 = 1;
const GPT_ENTRY_SIZE: usize = 128;

pub fn detect_partition_table(first_sector: &[u8]) -> PartitionTableType {
    if first_sector.len() < 512 {
        return PartitionTableType::Unknown;
    }

    let sig = u16::from_le_bytes([first_sector[510], first_sector[511]]);
    if sig != MBR_SIGNATURE {
        return PartitionTableType::None;
    }

    // Check for a protective MBR (type 0xEE in first partition entry)
    let ptype = first_sector[0x1BE + 4];
    if ptype == 0xEE {
        return PartitionTableType::Gpt;
    }

    PartitionTableType::Mbr
}

pub fn parse_mbr(sector_data: &[u8]) -> Result<Vec<Partition>, BlockError> {
    if sector_data.len() < 512 {
        return Err(BlockError::InvalidPartitionTable);
    }

    let sig = u16::from_le_bytes([sector_data[510], sector_data[511]]);
    if sig != MBR_SIGNATURE {
        return Err(BlockError::InvalidPartitionTable);
    }

    let mut partitions = Vec::new();

    for i in 0..4u8 {
        let offset = 0x1BE + (i as usize * 16);
        let status = sector_data[offset];
        let ptype = sector_data[offset + 4];

        if ptype == 0 {
            continue;
        }

        let start_lba = u32::from_le_bytes([
            sector_data[offset + 8],
            sector_data[offset + 9],
            sector_data[offset + 10],
            sector_data[offset + 11],
        ]);

        let sector_count = u32::from_le_bytes([
            sector_data[offset + 12],
            sector_data[offset + 13],
            sector_data[offset + 14],
            sector_data[offset + 15],
        ]);

        if start_lba == 0 || sector_count == 0 {
            continue;
        }

        partitions.push(Partition {
            number: i + 1,
            start_sector: start_lba as u64,
            sector_count: sector_count as u64,
            partition_type: PartitionType::from_mbr_type(ptype),
            name: String::new(),
            filesystem: None,
            bootable: status == 0x80,
            uuid: [0u8; 16],
        });
    }

    Ok(partitions)
}

pub fn parse_gpt(sector_data: &[u8], sector_size: u32) -> Result<Vec<Partition>, BlockError> {
    let ss = sector_size as usize;
    if sector_data.len() < ss * 2 + GPT_ENTRY_SIZE {
        return Err(BlockError::InvalidPartitionTable);
    }

    let hdr = &sector_data[ss..ss * 2];

    let sig = u64::from_le_bytes([
        hdr[0], hdr[1], hdr[2], hdr[3], hdr[4], hdr[5], hdr[6], hdr[7],
    ]);
    if sig != GPT_SIGNATURE {
        return Err(BlockError::InvalidPartitionTable);
    }

    let revision = u32::from_le_bytes([hdr[8], hdr[9], hdr[10], hdr[11]]);
    let header_size = u32::from_le_bytes([hdr[12], hdr[13], hdr[14], hdr[15]]);
    if header_size < 92 || revision < 0x00010000 {
        return Err(BlockError::InvalidPartitionTable);
    }

    let partition_entry_lba = u64::from_le_bytes([
        hdr[72], hdr[73], hdr[74], hdr[75], hdr[76], hdr[77], hdr[78], hdr[79],
    ]);
    let num_entries = u32::from_le_bytes([hdr[80], hdr[81], hdr[82], hdr[83]]);
    let entry_size = u32::from_le_bytes([hdr[84], hdr[85], hdr[86], hdr[87]]);

    if entry_size < 128 {
        return Err(BlockError::InvalidPartitionTable);
    }

    let entries_offset = (partition_entry_lba as usize) * ss;
    let mut partitions = Vec::new();
    let max_entries = core::cmp::min(num_entries as usize, 128);

    for i in 0..max_entries {
        let ofs = entries_offset + i * entry_size as usize;
        if ofs + entry_size as usize > sector_data.len() {
            break;
        }

        let entry = &sector_data[ofs..ofs + entry_size as usize];

        // Type GUID (first 16 bytes) — all-zero means unused
        let type_guid: [u8; 16] = {
            let mut g = [0u8; 16];
            g.copy_from_slice(&entry[0..16]);
            g
        };
        if type_guid == [0u8; 16] {
            continue;
        }

        let unique_guid: [u8; 16] = {
            let mut g = [0u8; 16];
            g.copy_from_slice(&entry[16..32]);
            g
        };

        let first_lba = u64::from_le_bytes([
            entry[32], entry[33], entry[34], entry[35], entry[36], entry[37], entry[38], entry[39],
        ]);
        let last_lba = u64::from_le_bytes([
            entry[40], entry[41], entry[42], entry[43], entry[44], entry[45], entry[46], entry[47],
        ]);

        let attributes = u64::from_le_bytes([
            entry[48], entry[49], entry[50], entry[51], entry[52], entry[53], entry[54], entry[55],
        ]);

        // UTF-16LE name at offset 56, up to 72 bytes (36 UTF-16 code units)
        let mut name = String::new();
        let name_bytes = &entry[56..core::cmp::min(128, entry_size as usize)];
        for chunk in name_bytes.chunks(2) {
            if chunk.len() < 2 {
                break;
            }
            let ch = u16::from_le_bytes([chunk[0], chunk[1]]);
            if ch == 0 {
                break;
            }
            if let Some(c) = char::from_u32(ch as u32) {
                name.push(c);
            }
        }

        let bootable = (attributes & (1 << 2)) != 0; // Legacy BIOS bootable

        partitions.push(Partition {
            number: (i + 1) as u8,
            start_sector: first_lba,
            sector_count: last_lba - first_lba + 1,
            partition_type: PartitionType::from_gpt_guid(&type_guid),
            name,
            filesystem: None,
            bootable,
            uuid: unique_guid,
        });
    }

    Ok(partitions)
}

// ─── Page Cache ─────────────────────────────────────────────────────────────

pub struct PageCache {
    entries: BTreeMap<(u64, u64), CachePage>,
    max_pages: usize,
    dirty_pages: usize,
    hit_count: u64,
    miss_count: u64,
    writeback_threshold: f32,
    eviction_clock: u64,
}

pub struct CachePage {
    data: Vec<u8>,
    dirty: bool,
    accessed: u64,
    pinned: bool,
}

impl PageCache {
    pub fn new(max_pages: usize) -> Self {
        Self {
            entries: BTreeMap::new(),
            max_pages,
            dirty_pages: 0,
            hit_count: 0,
            miss_count: 0,
            writeback_threshold: 0.7,
            eviction_clock: 0,
        }
    }

    pub fn read(&mut self, dev: u64, sector: u64) -> Option<&[u8]> {
        let key = (dev, sector);
        if self.entries.contains_key(&key) {
            self.hit_count += 1;
            self.eviction_clock += 1;
            let page = self.entries.get_mut(&key).unwrap();
            page.accessed = self.eviction_clock;
            Some(&self.entries[&key].data)
        } else {
            self.miss_count += 1;
            None
        }
    }

    pub fn write(&mut self, dev: u64, sector: u64, data: &[u8]) {
        self.eviction_clock += 1;
        let key = (dev, sector);

        if let Some(page) = self.entries.get_mut(&key) {
            page.data.clear();
            page.data.extend_from_slice(data);
            if !page.dirty {
                page.dirty = true;
                self.dirty_pages += 1;
            }
            page.accessed = self.eviction_clock;
        } else {
            if self.entries.len() >= self.max_pages {
                self.evict_lru();
            }
            self.entries.insert(
                key,
                CachePage {
                    data: Vec::from(data),
                    dirty: true,
                    accessed: self.eviction_clock,
                    pinned: false,
                },
            );
            self.dirty_pages += 1;
        }
    }

    pub fn insert_clean(&mut self, dev: u64, sector: u64, data: &[u8]) {
        self.eviction_clock += 1;
        let key = (dev, sector);

        if self.entries.len() >= self.max_pages && !self.entries.contains_key(&key) {
            self.evict_lru();
        }

        self.entries.insert(
            key,
            CachePage {
                data: Vec::from(data),
                dirty: false,
                accessed: self.eviction_clock,
                pinned: false,
            },
        );
    }

    pub fn flush_device(&mut self, dev: u64) {
        for (key, page) in self.entries.iter_mut() {
            if key.0 == dev && page.dirty {
                page.dirty = false;
                self.dirty_pages = self.dirty_pages.saturating_sub(1);
            }
        }
    }

    pub fn flush_all(&mut self) {
        for page in self.entries.values_mut() {
            page.dirty = false;
        }
        self.dirty_pages = 0;
    }

    pub fn evict_lru(&mut self) -> usize {
        let mut evicted = 0;
        let target = self.max_pages * 3 / 4;

        while self.entries.len() > target {
            let lru_key = self
                .entries
                .iter()
                .filter(|(_, p)| !p.pinned && !p.dirty)
                .min_by_key(|(_, p)| p.accessed)
                .map(|(k, _)| *k);

            if let Some(key) = lru_key {
                self.entries.remove(&key);
                evicted += 1;
            } else {
                // All remaining pages are pinned or dirty; evict oldest dirty
                let dirty_lru = self
                    .entries
                    .iter()
                    .filter(|(_, p)| !p.pinned)
                    .min_by_key(|(_, p)| p.accessed)
                    .map(|(k, _)| *k);

                if let Some(key) = dirty_lru {
                    if self.entries.get(&key).map_or(false, |p| p.dirty) {
                        self.dirty_pages = self.dirty_pages.saturating_sub(1);
                    }
                    self.entries.remove(&key);
                    evicted += 1;
                } else {
                    break;
                }
            }
        }

        evicted
    }

    pub fn invalidate(&mut self, dev: u64, sector: u64) {
        let key = (dev, sector);
        if let Some(page) = self.entries.remove(&key) {
            if page.dirty {
                self.dirty_pages = self.dirty_pages.saturating_sub(1);
            }
        }
    }

    pub fn invalidate_device(&mut self, dev: u64) {
        let keys: Vec<(u64, u64)> = self
            .entries
            .keys()
            .filter(|k| k.0 == dev)
            .copied()
            .collect();
        for key in keys {
            if let Some(page) = self.entries.remove(&key) {
                if page.dirty {
                    self.dirty_pages = self.dirty_pages.saturating_sub(1);
                }
            }
        }
    }

    pub fn pin(&mut self, dev: u64, sector: u64) {
        if let Some(page) = self.entries.get_mut(&(dev, sector)) {
            page.pinned = true;
        }
    }

    pub fn unpin(&mut self, dev: u64, sector: u64) {
        if let Some(page) = self.entries.get_mut(&(dev, sector)) {
            page.pinned = false;
        }
    }

    pub fn hit_rate(&self) -> f32 {
        let total = self.hit_count + self.miss_count;
        if total == 0 {
            return 0.0;
        }
        self.hit_count as f32 / total as f32
    }

    pub fn dirty_ratio(&self) -> f32 {
        if self.entries.is_empty() {
            return 0.0;
        }
        self.dirty_pages as f32 / self.entries.len() as f32
    }

    pub fn needs_writeback(&self) -> bool {
        self.dirty_ratio() > self.writeback_threshold
    }

    pub fn cached_pages(&self) -> usize {
        self.entries.len()
    }

    pub fn dirty_sectors(&self, dev: u64) -> Vec<u64> {
        self.entries
            .iter()
            .filter(|(k, p)| k.0 == dev && p.dirty)
            .map(|(k, _)| k.1)
            .collect()
    }
}

// ─── Block Layer (Global Registry) ─────────────────────────────────────────

pub struct BlockLayer {
    devices: Vec<BlockDeviceInfo>,
    next_major: u16,
    page_cache: PageCache,
}

impl BlockLayer {
    pub fn new() -> Self {
        Self {
            devices: Vec::new(),
            next_major: 8, // Start at 8 like Linux sd*
            page_cache: PageCache::new(4096),
        }
    }

    pub fn register_device(&mut self, mut dev: BlockDeviceInfo) -> u64 {
        if dev.major == 0 {
            dev.major = self.next_major;
            self.next_major += 1;
        }
        let id = dev.device_id();
        crate::serial_println!(
            "[block] registered {} ({}:{}) {} sectors, {}MB",
            dev.name,
            dev.major,
            dev.minor,
            dev.total_sectors,
            dev.capacity_mb()
        );
        self.devices.push(dev);
        id
    }

    pub fn unregister_device(&mut self, major: u16, minor: u16) -> Option<BlockDeviceInfo> {
        let pos = self
            .devices
            .iter()
            .position(|d| d.major == major && d.minor == minor)?;
        let dev = self.devices.remove(pos);
        self.page_cache.invalidate_device(dev.device_id());
        Some(dev)
    }

    pub fn get_device(&self, major: u16, minor: u16) -> Option<&BlockDeviceInfo> {
        self.devices
            .iter()
            .find(|d| d.major == major && d.minor == minor)
    }

    pub fn get_device_mut(&mut self, major: u16, minor: u16) -> Option<&mut BlockDeviceInfo> {
        self.devices
            .iter_mut()
            .find(|d| d.major == major && d.minor == minor)
    }

    pub fn get_device_by_name(&self, name: &str) -> Option<&BlockDeviceInfo> {
        self.devices.iter().find(|d| d.name == name)
    }

    pub fn list_devices(&self) -> &[BlockDeviceInfo] {
        &self.devices
    }

    pub fn device_count(&self) -> usize {
        self.devices.len()
    }

    pub fn cached_read(&mut self, dev_id: u64, sector: u64) -> Option<&[u8]> {
        self.page_cache.read(dev_id, sector)
    }

    pub fn cached_write(&mut self, dev_id: u64, sector: u64, data: &[u8]) {
        self.page_cache.write(dev_id, sector, data);
    }

    pub fn cache_stats(&self) -> (u64, u64, f32) {
        (
            self.page_cache.hit_count,
            self.page_cache.miss_count,
            self.page_cache.hit_rate(),
        )
    }

    pub fn flush_cache(&mut self) {
        self.page_cache.flush_all();
    }

    pub fn page_cache(&self) -> &PageCache {
        &self.page_cache
    }

    pub fn page_cache_mut(&mut self) -> &mut PageCache {
        &mut self.page_cache
    }
}

// ─── Global State ───────────────────────────────────────────────────────────

pub static BLOCK_LAYER: Mutex<Option<BlockLayer>> = Mutex::new(None);

pub fn init() {
    let mut layer = BLOCK_LAYER.lock();
    *layer = Some(BlockLayer::new());
    crate::serial_println!("[ OK ] Block I/O layer initialized");
}

/// R10 smoketest for the install write-window safety gate (the disk-wipe guard).
/// Proves, FAIL-ably: (1) the pure boot-write policy only defaults writes-on for
/// QEMU — every real hardware family is read-only at boot; (2) the write window
/// mechanism actually flips `writes_enabled()` and counts opens. Snapshots and
/// RESTORES the live write state so the storage smoketests that follow (which
/// need writes on in QEMU) are undisturbed. Runs after the Tier-2 write gate.
pub fn run_boot_smoketest() {
    use crate::hardware_profile::HardwareFamily;
    // 1. Pure policy: ONLY QEMU may default writes-on at boot.
    let gate_ok = boot_writes_default_on(HardwareFamily::QemuVirtual)
        && !boot_writes_default_on(HardwareFamily::AmdDesktop)
        && !boot_writes_default_on(HardwareFamily::IntelDesktop)
        && !boot_writes_default_on(HardwareFamily::Laptop)
        && !boot_writes_default_on(HardwareFamily::Unknown);
    // 2. Window mechanism — snapshot + restore so the live state is preserved.
    let prev = writes_enabled();
    let opens0 = write_window_opens();
    set_writes_enabled(false);
    let off_ok = !writes_enabled();
    set_writes_enabled(true);
    let on_ok = writes_enabled() && write_window_opens() == opens0 + 1;
    set_writes_enabled(prev);
    let restored_ok = writes_enabled() == prev;
    let pass = gate_ok && off_ok && on_ok && restored_ok;
    crate::serial_println!(
        "[block_io] install write-window guard: gate_logic={} window(off/on/restore={}/{}/{}) boot_opens={} -> {}",
        gate_ok,
        off_ok,
        on_ok,
        restored_ok,
        opens0,
        if pass { "PASS" } else { "FAIL" }
    );
    crate::selftest::record_smoketest("block_write_window", pass);
}

pub fn register_block_device(dev: BlockDeviceInfo) -> Result<u64, BlockError> {
    let mut layer = BLOCK_LAYER.lock();
    let bl = layer.as_mut().ok_or(BlockError::DeviceNotFound)?;
    Ok(bl.register_device(dev))
}

pub fn submit_io(major: u16, minor: u16, bio: Bio) -> Result<(), BlockError> {
    let mut layer = BLOCK_LAYER.lock();
    let bl = layer.as_mut().ok_or(BlockError::DeviceNotFound)?;
    let dev = bl
        .get_device_mut(major, minor)
        .ok_or(BlockError::DeviceNotFound)?;
    dev.submit_bio(bio)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_bio(sector: u64, op: BioOp) -> Bio {
        Bio {
            sector,
            sector_count: 1,
            operation: op,
            data: Vec::new(),
            flags: BioFlags::default(),
            priority: BioPriority::Normal,
            submitted_at: 0,
            completed: false,
            error: None,
            callback: None,
        }
    }

    #[test]
    fn noop_scheduler_fifo() {
        let mut sched = NoopScheduler::new();
        sched.enqueue(make_test_bio(100, BioOp::Read));
        sched.enqueue(make_test_bio(50, BioOp::Read));
        sched.enqueue(make_test_bio(200, BioOp::Read));

        assert_eq!(sched.len(), 3);
        assert_eq!(sched.dequeue().unwrap().sector, 100);
        assert_eq!(sched.dequeue().unwrap().sector, 50);
        assert_eq!(sched.dequeue().unwrap().sector, 200);
        assert!(sched.dequeue().is_none());
    }

    #[test]
    fn deadline_sorts_by_sector() {
        let mut sched = DeadlineScheduler::new();
        sched.enqueue(make_test_bio(300, BioOp::Read));
        sched.enqueue(make_test_bio(100, BioOp::Read));
        sched.enqueue(make_test_bio(200, BioOp::Read));

        assert_eq!(sched.dequeue().unwrap().sector, 100);
        assert_eq!(sched.dequeue().unwrap().sector, 200);
        assert_eq!(sched.dequeue().unwrap().sector, 300);
    }

    #[test]
    fn page_cache_hit_miss() {
        let mut cache = PageCache::new(16);
        assert!(cache.read(1, 0).is_none());
        assert_eq!(cache.hit_rate(), 0.0);

        cache.write(1, 0, &[0xAA; 512]);
        assert!(cache.read(1, 0).is_some());
        assert!(cache.hit_rate() > 0.0);
    }
}
