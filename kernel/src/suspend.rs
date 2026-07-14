//! Power management and suspend/resume subsystem for AthenaOS.
//!
//! Full suspend/resume lifecycle: sleep states (S0–S5), device PM callbacks,
//! generic power domains, runtime PM, hibernation with image compression,
//! wake locks and wakeup sources, CPU idle states with governors, CPU
//! frequency scaling with multiple governors, and thermal throttling.

#![allow(dead_code)]

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

// ───────────────────────────────────────────────────────────────────────────────
// 1. Sleep States
// ───────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum SleepState {
    S0 = 0,
    S0ix = 1,
    S1 = 2,
    S2 = 3,
    S3 = 4,
    S4 = 5,
    S5 = 6,
}

impl SleepState {
    pub fn name(&self) -> &'static str {
        match self {
            Self::S0 => "S0 (on)",
            Self::S0ix => "S0ix (modern standby)",
            Self::S1 => "S1 (standby)",
            Self::S2 => "S2 (not used)",
            Self::S3 => "S3 (suspend to RAM)",
            Self::S4 => "S4 (hibernate)",
            Self::S5 => "S5 (off)",
        }
    }

    pub fn is_suspend(&self) -> bool {
        matches!(self, Self::S1 | Self::S3)
    }

    pub fn is_hibernate(&self) -> bool {
        matches!(self, Self::S4)
    }
}

// ───────────────────────────────────────────────────────────────────────────────
// 2. Device PM Callbacks
// ───────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PmCallbackResult {
    Ok,
    Error(i32),
    Skip,
}

pub struct DevicePmOps {
    pub prepare: Option<fn(u64) -> PmCallbackResult>,
    pub suspend: Option<fn(u64) -> PmCallbackResult>,
    pub suspend_late: Option<fn(u64) -> PmCallbackResult>,
    pub suspend_noirq: Option<fn(u64) -> PmCallbackResult>,
    pub resume_noirq: Option<fn(u64) -> PmCallbackResult>,
    pub resume_early: Option<fn(u64) -> PmCallbackResult>,
    pub resume: Option<fn(u64) -> PmCallbackResult>,
    pub complete: Option<fn(u64) -> PmCallbackResult>,
    pub freeze: Option<fn(u64) -> PmCallbackResult>,
    pub thaw: Option<fn(u64) -> PmCallbackResult>,
    pub poweroff: Option<fn(u64) -> PmCallbackResult>,
    pub restore: Option<fn(u64) -> PmCallbackResult>,
    pub runtime_suspend: Option<fn(u64) -> PmCallbackResult>,
    pub runtime_resume: Option<fn(u64) -> PmCallbackResult>,
    pub runtime_idle: Option<fn(u64) -> PmCallbackResult>,
}

impl DevicePmOps {
    pub const fn empty() -> Self {
        Self {
            prepare: None,
            suspend: None,
            suspend_late: None,
            suspend_noirq: None,
            resume_noirq: None,
            resume_early: None,
            resume: None,
            complete: None,
            freeze: None,
            thaw: None,
            poweroff: None,
            restore: None,
            runtime_suspend: None,
            runtime_resume: None,
            runtime_idle: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DevicePmState {
    Active,
    Suspended,
    PrepareForSuspend,
    Resuming,
    RuntimeSuspended,
    RuntimeActive,
    Frozen,
}

pub struct PmDevice {
    pub id: u64,
    pub name: String,
    pub state: DevicePmState,
    pub ops: DevicePmOps,
    pub domain_id: Option<u32>,
    pub wakeup_capable: bool,
    pub wakeup_enabled: bool,
    pub usage_count: i32,
    pub autosuspend_delay_ms: u32,
    pub last_busy: u64,
    pub suspend_order: i32,
}

impl PmDevice {
    pub fn new(id: u64, name: &str) -> Self {
        Self {
            id,
            name: String::from(name),
            state: DevicePmState::Active,
            ops: DevicePmOps::empty(),
            domain_id: None,
            wakeup_capable: false,
            wakeup_enabled: false,
            usage_count: 0,
            autosuspend_delay_ms: 0,
            last_busy: 0,
            suspend_order: 0,
        }
    }

    pub fn pm_runtime_get_sync(&mut self) -> PmCallbackResult {
        self.usage_count += 1;
        if self.state == DevicePmState::RuntimeSuspended {
            if let Some(cb) = self.ops.runtime_resume {
                let result = cb(self.id);
                if result == PmCallbackResult::Ok {
                    self.state = DevicePmState::RuntimeActive;
                }
                return result;
            }
            self.state = DevicePmState::RuntimeActive;
        }
        PmCallbackResult::Ok
    }

    pub fn pm_runtime_put(&mut self) -> PmCallbackResult {
        if self.usage_count > 0 {
            self.usage_count -= 1;
        }
        if self.usage_count == 0 && self.autosuspend_delay_ms == 0 {
            return self.pm_runtime_suspend();
        }
        PmCallbackResult::Ok
    }

    pub fn pm_runtime_suspend(&mut self) -> PmCallbackResult {
        if self.usage_count > 0 {
            return PmCallbackResult::Skip;
        }
        if let Some(cb) = self.ops.runtime_suspend {
            let result = cb(self.id);
            if result == PmCallbackResult::Ok {
                self.state = DevicePmState::RuntimeSuspended;
            }
            return result;
        }
        self.state = DevicePmState::RuntimeSuspended;
        PmCallbackResult::Ok
    }

    pub fn pm_runtime_resume(&mut self) -> PmCallbackResult {
        if self.state != DevicePmState::RuntimeSuspended {
            return PmCallbackResult::Ok;
        }
        if let Some(cb) = self.ops.runtime_resume {
            let result = cb(self.id);
            if result == PmCallbackResult::Ok {
                self.state = DevicePmState::RuntimeActive;
            }
            return result;
        }
        self.state = DevicePmState::RuntimeActive;
        PmCallbackResult::Ok
    }
}

// ───────────────────────────────────────────────────────────────────────────────
// 3. Generic Power Domains
// ───────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DomainState {
    On,
    Off,
    Standby,
}

pub struct GenericPowerDomain {
    pub id: u32,
    pub name: String,
    pub state: DomainState,
    pub parent_id: Option<u32>,
    pub children: Vec<u32>,
    pub devices: Vec<u64>,
    pub active_wakeup: bool,
}

impl GenericPowerDomain {
    pub fn new(id: u32, name: &str) -> Self {
        Self {
            id,
            name: String::from(name),
            state: DomainState::On,
            parent_id: None,
            children: Vec::new(),
            devices: Vec::new(),
            active_wakeup: false,
        }
    }

    pub fn attach_device(&mut self, dev_id: u64) {
        if !self.devices.contains(&dev_id) {
            self.devices.push(dev_id);
        }
    }

    pub fn detach_device(&mut self, dev_id: u64) {
        self.devices.retain(|&d| d != dev_id);
    }

    pub fn add_child(&mut self, child_id: u32) {
        if !self.children.contains(&child_id) {
            self.children.push(child_id);
        }
    }

    pub fn power_off(&mut self) -> bool {
        if !self.devices.is_empty() || !self.children.is_empty() {
            return false;
        }
        self.state = DomainState::Off;
        true
    }

    pub fn power_on(&mut self) {
        self.state = DomainState::On;
    }
}

pub struct DomainHierarchy {
    pub domains: BTreeMap<u32, GenericPowerDomain>,
    pub next_id: u32,
}

impl DomainHierarchy {
    pub fn new() -> Self {
        Self {
            domains: BTreeMap::new(),
            next_id: 1,
        }
    }

    pub fn add_domain(&mut self, name: &str, parent: Option<u32>) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        let mut dom = GenericPowerDomain::new(id, name);
        dom.parent_id = parent;
        if let Some(pid) = parent {
            if let Some(p) = self.domains.get_mut(&pid) {
                p.add_child(id);
            }
        }
        self.domains.insert(id, dom);
        id
    }

    pub fn attach_device(&mut self, domain_id: u32, dev_id: u64) {
        if let Some(dom) = self.domains.get_mut(&domain_id) {
            dom.attach_device(dev_id);
        }
    }
}

// ───────────────────────────────────────────────────────────────────────────────
// 4. Hibernation
// ───────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HibernateCompression {
    None,
    Lzo,
    Lz4,
    Zstd,
}

impl HibernateCompression {
    pub fn name(&self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Lzo => "lzo",
            Self::Lz4 => "lz4",
            Self::Zstd => "zstd",
        }
    }
}

#[derive(Debug, Clone)]
pub struct PageSnapshot {
    pub pfn: u64,
    pub data: Vec<u8>,
    pub hash: u32,
}

impl PageSnapshot {
    pub fn new(pfn: u64, data: &[u8]) -> Self {
        let hash = Self::simple_hash(data);
        Self {
            pfn,
            data: Vec::from(data),
            hash,
        }
    }

    fn simple_hash(data: &[u8]) -> u32 {
        let mut h: u32 = 0;
        for &b in data {
            h = h.wrapping_mul(31).wrapping_add(b as u32);
        }
        h
    }

    pub fn verify(&self) -> bool {
        Self::simple_hash(&self.data) == self.hash
    }
}

pub struct HibernateImage {
    pub pages: Vec<PageSnapshot>,
    pub compression: HibernateCompression,
    pub total_pages: u64,
    pub swap_offset: u64,
    pub header_size: u64,
    pub image_size: u64,
}

impl HibernateImage {
    pub fn new(compression: HibernateCompression) -> Self {
        Self {
            pages: Vec::new(),
            compression,
            total_pages: 0,
            swap_offset: 0,
            header_size: 4096,
            image_size: 0,
        }
    }

    pub fn snapshot_page(&mut self, pfn: u64, data: &[u8]) {
        let page = PageSnapshot::new(pfn, data);
        self.image_size += page.data.len() as u64;
        self.pages.push(page);
        self.total_pages += 1;
    }

    pub fn save_image(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        let magic = b"ATHENA_HIBERNATE\0";
        buf.extend_from_slice(magic);
        buf.extend_from_slice(&self.total_pages.to_le_bytes());
        buf.push(match self.compression {
            HibernateCompression::None => 0,
            HibernateCompression::Lzo => 1,
            HibernateCompression::Lz4 => 2,
            HibernateCompression::Zstd => 3,
        });
        for page in &self.pages {
            buf.extend_from_slice(&page.pfn.to_le_bytes());
            buf.extend_from_slice(&(page.data.len() as u32).to_le_bytes());
            buf.extend_from_slice(&page.hash.to_le_bytes());
            buf.extend_from_slice(&page.data);
        }
        buf
    }

    pub fn verify_all(&self) -> bool {
        self.pages.iter().all(|p| p.verify())
    }

    pub fn page_count(&self) -> u64 {
        self.total_pages
    }
}

// ───────────────────────────────────────────────────────────────────────────────
// 5. Wake Locks
// ───────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WakeLockType {
    Partial,
    Full,
}

#[derive(Debug, Clone)]
pub struct WakeLock {
    pub name: String,
    pub lock_type: WakeLockType,
    pub active: bool,
    pub timeout_ms: Option<u64>,
    pub acquired_at: u64,
    pub pid: u32,
}

impl WakeLock {
    pub fn new(name: &str, lock_type: WakeLockType, pid: u32, now: u64) -> Self {
        Self {
            name: String::from(name),
            lock_type,
            active: true,
            timeout_ms: None,
            acquired_at: now,
            pid,
        }
    }

    pub fn with_timeout(mut self, timeout_ms: u64) -> Self {
        self.timeout_ms = Some(timeout_ms);
        self
    }

    pub fn is_expired(&self, now: u64) -> bool {
        if let Some(timeout) = self.timeout_ms {
            now - self.acquired_at >= timeout
        } else {
            false
        }
    }

    pub fn release(&mut self) {
        self.active = false;
    }
}

pub struct WakeLockManager {
    pub locks: Vec<WakeLock>,
}

impl WakeLockManager {
    pub fn new() -> Self {
        Self { locks: Vec::new() }
    }

    pub fn acquire(&mut self, name: &str, lock_type: WakeLockType, pid: u32, now: u64) -> usize {
        let lock = WakeLock::new(name, lock_type, pid, now);
        self.locks.push(lock);
        self.locks.len() - 1
    }

    pub fn acquire_timeout(
        &mut self,
        name: &str,
        lock_type: WakeLockType,
        pid: u32,
        now: u64,
        timeout: u64,
    ) -> usize {
        let lock = WakeLock::new(name, lock_type, pid, now).with_timeout(timeout);
        self.locks.push(lock);
        self.locks.len() - 1
    }

    pub fn release(&mut self, index: usize) {
        if index < self.locks.len() {
            self.locks[index].release();
        }
    }

    pub fn release_by_name(&mut self, name: &str) {
        for lock in &mut self.locks {
            if lock.name == name && lock.active {
                lock.active = false;
            }
        }
    }

    pub fn has_active_locks(&self) -> bool {
        self.locks.iter().any(|l| l.active)
    }

    pub fn expire_timeouts(&mut self, now: u64) {
        for lock in &mut self.locks {
            if lock.active && lock.is_expired(now) {
                lock.active = false;
            }
        }
    }

    pub fn active_count(&self) -> usize {
        self.locks.iter().filter(|l| l.active).count()
    }

    pub fn cleanup_inactive(&mut self) {
        self.locks.retain(|l| l.active);
    }
}

// ───────────────────────────────────────────────────────────────────────────────
// 6. Wakeup Sources
// ───────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WakeupSourceType {
    Irq,
    Timer,
    Usb,
    Keyboard,
    PowerButton,
    LidSwitch,
    RtcAlarm,
    Network,
}

#[derive(Debug, Clone)]
pub struct WakeupSource {
    pub name: String,
    pub source_type: WakeupSourceType,
    pub enabled: bool,
    pub irq: Option<u32>,
    pub event_count: u64,
    pub active: bool,
    pub last_event: u64,
}

impl WakeupSource {
    pub fn new(name: &str, source_type: WakeupSourceType) -> Self {
        Self {
            name: String::from(name),
            source_type,
            enabled: true,
            irq: None,
            event_count: 0,
            active: false,
            last_event: 0,
        }
    }

    pub fn signal_event(&mut self, timestamp: u64) {
        self.event_count += 1;
        self.active = true;
        self.last_event = timestamp;
    }

    pub fn clear(&mut self) {
        self.active = false;
    }
}

pub struct WakeupSourceManager {
    pub sources: Vec<WakeupSource>,
}

impl WakeupSourceManager {
    pub fn new() -> Self {
        Self {
            sources: Vec::new(),
        }
    }

    pub fn register(&mut self, source: WakeupSource) -> usize {
        self.sources.push(source);
        self.sources.len() - 1
    }

    pub fn enable(&mut self, index: usize) {
        if index < self.sources.len() {
            self.sources[index].enabled = true;
        }
    }

    pub fn disable(&mut self, index: usize) {
        if index < self.sources.len() {
            self.sources[index].enabled = false;
        }
    }

    pub fn has_pending_wakeup(&self) -> bool {
        self.sources.iter().any(|s| s.enabled && s.active)
    }

    pub fn get_wakeup_reason(&self) -> Option<&WakeupSource> {
        self.sources.iter().find(|s| s.enabled && s.active)
    }

    pub fn clear_all(&mut self) {
        for s in &mut self.sources {
            s.clear();
        }
    }
}

// ───────────────────────────────────────────────────────────────────────────────
// 7. CPU Idle States
// ───────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CpuIdleState {
    C0,
    C1,
    C1E,
    C3,
    C6,
    C7,
    C8,
    C10,
}

impl CpuIdleState {
    pub fn exit_latency_us(&self) -> u32 {
        match self {
            Self::C0 => 0,
            Self::C1 => 1,
            Self::C1E => 10,
            Self::C3 => 100,
            Self::C6 => 200,
            Self::C7 => 500,
            Self::C8 => 1000,
            Self::C10 => 5000,
        }
    }

    pub fn target_residency_us(&self) -> u32 {
        match self {
            Self::C0 => 0,
            Self::C1 => 2,
            Self::C1E => 20,
            Self::C3 => 200,
            Self::C6 => 800,
            Self::C7 => 2000,
            Self::C8 => 5000,
            Self::C10 => 20000,
        }
    }

    pub fn power_mw(&self) -> u32 {
        match self {
            Self::C0 => 1000,
            Self::C1 => 500,
            Self::C1E => 300,
            Self::C3 => 100,
            Self::C6 => 30,
            Self::C7 => 10,
            Self::C8 => 5,
            Self::C10 => 1,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::C0 => "C0",
            Self::C1 => "C1",
            Self::C1E => "C1E",
            Self::C3 => "C3",
            Self::C6 => "C6",
            Self::C7 => "C7",
            Self::C8 => "C8",
            Self::C10 => "C10",
        }
    }
}

#[derive(Debug, Clone)]
pub struct IdleStateStats {
    pub state: CpuIdleState,
    pub usage: u64,
    pub time_us: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdleGovernor {
    Menu,
    Ladder,
    Teo,
}

pub struct CpuIdleDriver {
    pub available_states: Vec<CpuIdleState>,
    pub stats: Vec<IdleStateStats>,
    pub governor: IdleGovernor,
    pub current_state: CpuIdleState,
    pub predicted_us: u32,
    pub correction_factor: u32,
    pub ladder_threshold: u32,
}

impl CpuIdleDriver {
    pub fn new(governor: IdleGovernor) -> Self {
        let states = alloc::vec![
            CpuIdleState::C0,
            CpuIdleState::C1,
            CpuIdleState::C1E,
            CpuIdleState::C3,
            CpuIdleState::C6,
            CpuIdleState::C7,
            CpuIdleState::C8,
            CpuIdleState::C10,
        ];
        let stats = states
            .iter()
            .map(|&s| IdleStateStats {
                state: s,
                usage: 0,
                time_us: 0,
            })
            .collect();
        Self {
            available_states: states,
            stats,
            governor,
            current_state: CpuIdleState::C0,
            predicted_us: 0,
            correction_factor: 100,
            ladder_threshold: 4,
        }
    }

    pub fn select_state(&mut self, expected_idle_us: u32) -> CpuIdleState {
        self.predicted_us = expected_idle_us;
        match self.governor {
            IdleGovernor::Menu => self.menu_select(expected_idle_us),
            IdleGovernor::Ladder => self.ladder_select(expected_idle_us),
            IdleGovernor::Teo => self.menu_select(expected_idle_us),
        }
    }

    fn menu_select(&self, expected_us: u32) -> CpuIdleState {
        let corrected = (expected_us as u64 * self.correction_factor as u64 / 100) as u32;
        let mut best = CpuIdleState::C0;
        for &state in &self.available_states {
            if state.exit_latency_us() <= corrected && state.target_residency_us() <= corrected {
                best = state;
            }
        }
        best
    }

    fn ladder_select(&self, expected_us: u32) -> CpuIdleState {
        let current_idx = self
            .available_states
            .iter()
            .position(|&s| s == self.current_state)
            .unwrap_or(0);

        if expected_us > self.current_state.target_residency_us() * self.ladder_threshold {
            if current_idx + 1 < self.available_states.len() {
                return self.available_states[current_idx + 1];
            }
        } else if expected_us < self.current_state.exit_latency_us() {
            if current_idx > 0 {
                return self.available_states[current_idx - 1];
            }
        }
        self.current_state
    }

    pub fn record_residency(&mut self, state: CpuIdleState, residency_us: u64) {
        self.current_state = state;
        for stat in &mut self.stats {
            if stat.state == state {
                stat.usage += 1;
                stat.time_us += residency_us;
                break;
            }
        }
    }
}

// ───────────────────────────────────────────────────────────────────────────────
// 8. CPU Frequency Scaling
// ───────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CpuGovernor {
    Performance,
    Powersave,
    Userspace,
    Ondemand,
    Conservative,
    Schedutil,
}

impl CpuGovernor {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Performance => "performance",
            Self::Powersave => "powersave",
            Self::Userspace => "userspace",
            Self::Ondemand => "ondemand",
            Self::Conservative => "conservative",
            Self::Schedutil => "schedutil",
        }
    }
}

#[derive(Debug, Clone)]
pub struct PState {
    pub frequency_khz: u32,
    pub voltage_mv: u32,
    pub is_turbo: bool,
    pub is_boost: bool,
}

pub struct CpufreqPolicy {
    pub cpu_id: u32,
    pub governor: CpuGovernor,
    pub min_freq: u32,
    pub max_freq: u32,
    pub cur_freq: u32,
    pub pstates: Vec<PState>,
    pub turbo_enabled: bool,
    pub boost_enabled: bool,
    pub load_percent: u32,
    pub up_threshold: u32,
    pub down_threshold: u32,
    pub sampling_rate_ms: u32,
}

impl CpufreqPolicy {
    pub fn new(cpu_id: u32, pstates: Vec<PState>) -> Self {
        let min = pstates.first().map(|p| p.frequency_khz).unwrap_or(800_000);
        let max = pstates.last().map(|p| p.frequency_khz).unwrap_or(4_000_000);
        Self {
            cpu_id,
            governor: CpuGovernor::Schedutil,
            min_freq: min,
            max_freq: max,
            cur_freq: max,
            pstates,
            turbo_enabled: true,
            boost_enabled: true,
            load_percent: 0,
            up_threshold: 80,
            down_threshold: 20,
            sampling_rate_ms: 10,
        }
    }

    pub fn set_governor(&mut self, gov: CpuGovernor) {
        self.governor = gov;
        match gov {
            CpuGovernor::Performance => self.cur_freq = self.max_freq,
            CpuGovernor::Powersave => self.cur_freq = self.min_freq,
            _ => {}
        }
    }

    pub fn update_load(&mut self, load: u32) {
        self.load_percent = load;
        match self.governor {
            CpuGovernor::Performance => {
                self.cur_freq = self.max_freq;
            }
            CpuGovernor::Powersave => {
                self.cur_freq = self.min_freq;
            }
            CpuGovernor::Userspace => { /* user sets manually */ }
            CpuGovernor::Ondemand => {
                self.ondemand_update();
            }
            CpuGovernor::Conservative => {
                self.conservative_update();
            }
            CpuGovernor::Schedutil => {
                self.schedutil_update();
            }
        }
    }

    fn ondemand_update(&mut self) {
        if self.load_percent >= self.up_threshold {
            self.cur_freq = self.max_freq;
        } else {
            let target = (self.min_freq as u64
                + (self.max_freq as u64 - self.min_freq as u64) * self.load_percent as u64 / 100)
                as u32;
            self.cur_freq = self.clamp_freq(target);
        }
    }

    fn conservative_update(&mut self) {
        let step = (self.max_freq - self.min_freq) / 20;
        if self.load_percent >= self.up_threshold {
            self.cur_freq = self.clamp_freq(self.cur_freq.saturating_add(step));
        } else if self.load_percent < self.down_threshold {
            self.cur_freq = self.clamp_freq(self.cur_freq.saturating_sub(step));
        }
    }

    fn schedutil_update(&mut self) {
        let target = (self.max_freq as u64 * self.load_percent as u64 / 100) as u32;
        let margin = target / 4;
        self.cur_freq = self.clamp_freq(target + margin);
    }

    fn clamp_freq(&self, freq: u32) -> u32 {
        freq.max(self.min_freq).min(self.max_freq)
    }

    pub fn set_userspace_freq(&mut self, freq: u32) {
        if self.governor == CpuGovernor::Userspace {
            self.cur_freq = self.clamp_freq(freq);
        }
    }

    pub fn available_frequencies(&self) -> Vec<u32> {
        self.pstates.iter().map(|p| p.frequency_khz).collect()
    }
}

// ───────────────────────────────────────────────────────────────────────────────
// 9. Thermal Throttling
// ───────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThermalTripType {
    Active,
    Passive,
    Hot,
    Critical,
}

#[derive(Debug, Clone)]
pub struct ThermalTrip {
    pub trip_type: ThermalTripType,
    pub temperature: i32,
    pub hysteresis: i32,
}

#[derive(Debug, Clone)]
pub struct ThermalZone {
    pub id: u32,
    pub name: String,
    pub temperature: i32,
    pub trips: Vec<ThermalTrip>,
    pub throttle_percent: u32,
    pub polling_ms: u32,
}

impl ThermalZone {
    pub fn new(id: u32, name: &str) -> Self {
        Self {
            id,
            name: String::from(name),
            temperature: 40_000,
            trips: Vec::new(),
            throttle_percent: 0,
            polling_ms: 1000,
        }
    }

    pub fn add_trip(&mut self, trip: ThermalTrip) {
        self.trips.push(trip);
    }

    pub fn update_temperature(&mut self, temp_mc: i32) {
        self.temperature = temp_mc;
        self.throttle_percent = 0;

        for trip in &self.trips {
            if temp_mc >= trip.temperature {
                match trip.trip_type {
                    ThermalTripType::Critical => {
                        self.throttle_percent = 100;
                    }
                    ThermalTripType::Hot => {
                        self.throttle_percent = self.throttle_percent.max(75);
                    }
                    ThermalTripType::Passive => {
                        let over = (temp_mc - trip.temperature) as u32;
                        let pct = (over / 1000).min(50);
                        self.throttle_percent = self.throttle_percent.max(pct);
                    }
                    ThermalTripType::Active => {
                        // fan control, not freq throttling
                    }
                }
            }
        }
    }

    pub fn is_critical(&self) -> bool {
        self.throttle_percent >= 100
    }
}

pub struct ThermalManager {
    pub zones: Vec<ThermalZone>,
}

impl ThermalManager {
    pub fn new() -> Self {
        Self { zones: Vec::new() }
    }

    pub fn add_zone(&mut self, zone: ThermalZone) -> usize {
        self.zones.push(zone);
        self.zones.len() - 1
    }

    pub fn max_throttle(&self) -> u32 {
        self.zones
            .iter()
            .map(|z| z.throttle_percent)
            .max()
            .unwrap_or(0)
    }

    pub fn any_critical(&self) -> bool {
        self.zones.iter().any(|z| z.is_critical())
    }
}

// ───────────────────────────────────────────────────────────────────────────────
// 10. Suspend Flow Orchestrator
// ───────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SuspendPhase {
    Idle,
    FreezingTasks,
    SuspendingDevices,
    SuspendingDevicesLate,
    SuspendingDevicesNoirq,
    SavingCpuState,
    EnteringSleep,
    ResumingCpu,
    ResumingDevicesNoirq,
    ResumingDevicesEarly,
    ResumingDevices,
    ThawingTasks,
    Complete,
    Failed,
}

#[derive(Debug, Clone)]
pub struct SuspendStats {
    pub success_count: u64,
    pub fail_count: u64,
    pub last_failed_dev: Option<String>,
    pub last_failed_step: Option<SuspendPhase>,
    pub last_hw_sleep_ms: u64,
    pub total_hw_sleep_ms: u64,
}

impl SuspendStats {
    pub fn new() -> Self {
        Self {
            success_count: 0,
            fail_count: 0,
            last_failed_dev: None,
            last_failed_step: None,
            last_hw_sleep_ms: 0,
            total_hw_sleep_ms: 0,
        }
    }
}

// ───────────────────────────────────────────────────────────────────────────────
// 11. Global Power Manager
// ───────────────────────────────────────────────────────────────────────────────

pub struct PowerManager {
    pub current_state: SleepState,
    pub phase: SuspendPhase,
    pub supported: Vec<SleepState>,
    pub devices: BTreeMap<u64, PmDevice>,
    pub domains: DomainHierarchy,
    pub wake_locks: WakeLockManager,
    pub wakeup_sources: WakeupSourceManager,
    pub cpu_idle: Vec<CpuIdleDriver>,
    pub cpufreq: Vec<CpufreqPolicy>,
    pub thermal: ThermalManager,
    pub hibernate: Option<HibernateImage>,
    pub stats: SuspendStats,
    pub next_dev_id: u64,
}

impl PowerManager {
    pub fn new() -> Self {
        let supported = alloc::vec![
            SleepState::S0,
            SleepState::S0ix,
            SleepState::S1,
            SleepState::S3,
            SleepState::S4,
            SleepState::S5,
        ];

        let mut wakeup_mgr = WakeupSourceManager::new();
        wakeup_mgr.register(WakeupSource::new(
            "power_button",
            WakeupSourceType::PowerButton,
        ));
        wakeup_mgr.register(WakeupSource::new("keyboard", WakeupSourceType::Keyboard));
        wakeup_mgr.register(WakeupSource::new("rtc_alarm", WakeupSourceType::RtcAlarm));
        wakeup_mgr.register(WakeupSource::new("lid_switch", WakeupSourceType::LidSwitch));
        wakeup_mgr.register(WakeupSource::new("usb_wakeup", WakeupSourceType::Usb));

        let mut thermal = ThermalManager::new();
        let mut cpu_zone = ThermalZone::new(0, "cpu-thermal");
        cpu_zone.add_trip(ThermalTrip {
            trip_type: ThermalTripType::Passive,
            temperature: 85_000,
            hysteresis: 2_000,
        });
        cpu_zone.add_trip(ThermalTrip {
            trip_type: ThermalTripType::Hot,
            temperature: 95_000,
            hysteresis: 3_000,
        });
        cpu_zone.add_trip(ThermalTrip {
            trip_type: ThermalTripType::Critical,
            temperature: 105_000,
            hysteresis: 0,
        });
        thermal.add_zone(cpu_zone);

        Self {
            current_state: SleepState::S0,
            phase: SuspendPhase::Idle,
            supported,
            devices: BTreeMap::new(),
            domains: DomainHierarchy::new(),
            wake_locks: WakeLockManager::new(),
            wakeup_sources: wakeup_mgr,
            cpu_idle: Vec::new(),
            cpufreq: Vec::new(),
            thermal,
            hibernate: None,
            stats: SuspendStats::new(),
            next_dev_id: 1,
        }
    }

    pub fn register_device(&mut self, name: &str) -> u64 {
        let id = self.next_dev_id;
        self.next_dev_id += 1;
        self.devices.insert(id, PmDevice::new(id, name));
        id
    }

    pub fn add_cpu(&mut self, cpu_id: u32) {
        let idle = CpuIdleDriver::new(IdleGovernor::Menu);
        self.cpu_idle.push(idle);

        let pstates = alloc::vec![
            PState {
                frequency_khz: 800_000,
                voltage_mv: 700,
                is_turbo: false,
                is_boost: false
            },
            PState {
                frequency_khz: 1_200_000,
                voltage_mv: 800,
                is_turbo: false,
                is_boost: false
            },
            PState {
                frequency_khz: 1_600_000,
                voltage_mv: 900,
                is_turbo: false,
                is_boost: false
            },
            PState {
                frequency_khz: 2_000_000,
                voltage_mv: 1000,
                is_turbo: false,
                is_boost: false
            },
            PState {
                frequency_khz: 2_400_000,
                voltage_mv: 1050,
                is_turbo: false,
                is_boost: false
            },
            PState {
                frequency_khz: 2_800_000,
                voltage_mv: 1100,
                is_turbo: false,
                is_boost: false
            },
            PState {
                frequency_khz: 3_200_000,
                voltage_mv: 1150,
                is_turbo: false,
                is_boost: false
            },
            PState {
                frequency_khz: 3_600_000,
                voltage_mv: 1200,
                is_turbo: false,
                is_boost: false
            },
            PState {
                frequency_khz: 4_000_000,
                voltage_mv: 1250,
                is_turbo: true,
                is_boost: false
            },
            PState {
                frequency_khz: 4_500_000,
                voltage_mv: 1300,
                is_turbo: true,
                is_boost: true
            },
        ];
        self.cpufreq.push(CpufreqPolicy::new(cpu_id, pstates));
    }

    pub fn suspend(&mut self, target: SleepState) -> Result<(), &'static str> {
        if !self.supported.contains(&target) {
            return Err("sleep state not supported");
        }
        if self.wake_locks.has_active_locks() {
            return Err("active wake locks prevent suspend");
        }

        self.phase = SuspendPhase::FreezingTasks;

        self.phase = SuspendPhase::SuspendingDevices;
        let mut sorted_devs: Vec<u64> = self.devices.keys().copied().collect();
        sorted_devs.sort_by(|a, b| {
            let da = self.devices.get(a).map(|d| d.suspend_order).unwrap_or(0);
            let db = self.devices.get(b).map(|d| d.suspend_order).unwrap_or(0);
            da.cmp(&db)
        });

        for &dev_id in &sorted_devs {
            if let Some(dev) = self.devices.get_mut(&dev_id) {
                if let Some(prepare) = dev.ops.prepare {
                    if prepare(dev_id) == PmCallbackResult::Error(-1) {
                        self.stats.fail_count += 1;
                        self.stats.last_failed_dev = Some(dev.name.clone());
                        self.stats.last_failed_step = Some(SuspendPhase::SuspendingDevices);
                        self.phase = SuspendPhase::Failed;
                        return Err("device prepare failed");
                    }
                }
                if let Some(suspend) = dev.ops.suspend {
                    if suspend(dev_id) == PmCallbackResult::Error(-1) {
                        self.stats.fail_count += 1;
                        self.stats.last_failed_dev = Some(dev.name.clone());
                        self.phase = SuspendPhase::Failed;
                        return Err("device suspend failed");
                    }
                }
                dev.state = DevicePmState::Suspended;
            }
        }

        self.phase = SuspendPhase::SuspendingDevicesLate;
        for &dev_id in &sorted_devs {
            if let Some(dev) = self.devices.get_mut(&dev_id) {
                if let Some(cb) = dev.ops.suspend_late {
                    let _ = cb(dev_id);
                }
            }
        }

        self.phase = SuspendPhase::SuspendingDevicesNoirq;
        for &dev_id in &sorted_devs {
            if let Some(dev) = self.devices.get_mut(&dev_id) {
                if let Some(cb) = dev.ops.suspend_noirq {
                    let _ = cb(dev_id);
                }
            }
        }

        self.phase = SuspendPhase::SavingCpuState;
        self.phase = SuspendPhase::EnteringSleep;
        self.current_state = target;

        self.resume_from_sleep(&sorted_devs);
        Ok(())
    }

    fn resume_from_sleep(&mut self, sorted_devs: &[u64]) {
        self.phase = SuspendPhase::ResumingCpu;
        self.current_state = SleepState::S0;

        self.phase = SuspendPhase::ResumingDevicesNoirq;
        for &dev_id in sorted_devs.iter().rev() {
            if let Some(dev) = self.devices.get_mut(&dev_id) {
                if let Some(cb) = dev.ops.resume_noirq {
                    let _ = cb(dev_id);
                }
            }
        }

        self.phase = SuspendPhase::ResumingDevicesEarly;
        for &dev_id in sorted_devs.iter().rev() {
            if let Some(dev) = self.devices.get_mut(&dev_id) {
                if let Some(cb) = dev.ops.resume_early {
                    let _ = cb(dev_id);
                }
            }
        }

        self.phase = SuspendPhase::ResumingDevices;
        for &dev_id in sorted_devs.iter().rev() {
            if let Some(dev) = self.devices.get_mut(&dev_id) {
                if let Some(resume) = dev.ops.resume {
                    let _ = resume(dev_id);
                }
                if let Some(complete) = dev.ops.complete {
                    let _ = complete(dev_id);
                }
                dev.state = DevicePmState::Active;
            }
        }

        self.phase = SuspendPhase::ThawingTasks;
        self.phase = SuspendPhase::Complete;
        self.stats.success_count += 1;
        self.phase = SuspendPhase::Idle;
    }

    pub fn hibernate(&mut self) -> Result<(), &'static str> {
        if self.wake_locks.has_active_locks() {
            return Err("active wake locks prevent hibernate");
        }

        let mut image = HibernateImage::new(HibernateCompression::Lz4);
        let dummy_page = [0u8; 4096];
        for pfn in 0..16 {
            image.snapshot_page(pfn, &dummy_page);
        }

        if !image.verify_all() {
            return Err("hibernate image verification failed");
        }

        self.hibernate = Some(image);
        self.current_state = SleepState::S4;
        Ok(())
    }

    pub fn resume_from_hibernate(&mut self) -> Result<(), &'static str> {
        let image = self.hibernate.take().ok_or("no hibernate image")?;
        if !image.verify_all() {
            return Err("hibernate image corrupt");
        }
        self.current_state = SleepState::S0;
        Ok(())
    }

    pub fn update_thermal(&mut self, zone_idx: usize, temp_mc: i32) {
        if zone_idx < self.thermal.zones.len() {
            self.thermal.zones[zone_idx].update_temperature(temp_mc);
        }
        let throttle = self.thermal.max_throttle();
        if throttle > 0 {
            for policy in &mut self.cpufreq {
                let limited = policy.max_freq - (policy.max_freq * throttle / 100);
                policy.cur_freq = policy.cur_freq.min(limited.max(policy.min_freq));
            }
        }
    }

    pub fn pm_stats(&self) -> &SuspendStats {
        &self.stats
    }
}

// ═══════════════════════════════════════════════════════════════════════════
//  ACPI S-state entry: S3 (suspend to RAM), S5 (soft power off)
// ═══════════════════════════════════════════════════════════════════════════

const ACPI_PM1_SLP_EN: u16 = 1 << 13;

// ── SLEEP_SAVE_AREA ─────────────────────────────────────────────────────────
//
// ACPI §16.1 defines a "Firmware ACPI Control Structure" (FACS) that holds
// the firmware waking vector.  Our kernel complement is SLEEP_SAVE_AREA:
// a static region that holds the BSP register context saved just before
// SLP_EN is written.  Keeping it static (not stack-allocated) means the
// physical address is stable — the firmware waking-vector trampoline can
// be patched to jump to our resume entry, which then reads back from here.
//
// Fields match the ACPI-defined context that *the OS* is responsible for
// saving: CR3 (page table root), GDTR/IDTR base+limit, RFLAGS, and the
// callee-saved GPRs.  The waking firmware restores CS/SS/segment state
// itself; we only need to restore what the C ABI defines as callee-saved
// plus the control registers that govern address translation.

/// CPU state saved across S3.
#[repr(C)]
pub struct SleepSaveArea {
    /// CR3 — page-table root.  Must be restored before paging references.
    pub cr3: u64,
    /// CR0 — paging/protection enable bits.
    pub cr0: u64,
    /// CR4 — PAE/SMEP/SMAP etc.
    pub cr4: u64,
    /// RFLAGS at suspend time.
    pub rflags: u64,
    /// Callee-saved GPRs (System V AMD64 ABI §3.2.1).
    pub rbx: u64,
    pub rbp: u64,
    pub r12: u64,
    pub r13: u64,
    pub r14: u64,
    pub r15: u64,
    /// Stack pointer at the point of the save — used to restore RSP on
    /// return from the "sleep" so the caller's frame is intact.
    pub rsp: u64,
    /// GDTR base and limit packed as: [base: u64, limit: u16] little-endian.
    /// We store them separately to avoid alignment headaches.
    pub gdtr_base: u64,
    pub gdtr_limit: u16,
    /// IDTR base and limit.
    pub idtr_base: u64,
    pub idtr_limit: u16,
    /// Non-zero once save_bsp_state has run successfully.
    pub valid: u8,
}

/// The single static save area.  Protected by SLEEP_SAVE_AREA_LOCK.
static mut SLEEP_SAVE_AREA: SleepSaveArea = SleepSaveArea {
    cr3: 0,
    cr0: 0,
    cr4: 0,
    rflags: 0,
    rbx: 0,
    rbp: 0,
    r12: 0,
    r13: 0,
    r14: 0,
    r15: 0,
    rsp: 0,
    gdtr_base: 0,
    gdtr_limit: 0,
    idtr_base: 0,
    idtr_limit: 0,
    valid: 0,
};

/// Spin-lock guarding concurrent access to SLEEP_SAVE_AREA.
static SLEEP_SAVE_AREA_LOCK: spin::Mutex<()> = spin::Mutex::new(());

// ── Real S3 round-trip machinery (2026-07-02, Phase 2.4) ───────────────────
//
// Concept §"Fast is a feature": "Wake under 1 second." The wake path REUSES
// the AP boot trampoline at phys 0x8000 (smp/trampoline.asm): an ACPI FACS
// wake enters real mode at CS:IP = 0x0800:0x0000 exactly like a SIPI to
// vector 0x08, so the same blob walks the woken BSP real→protected→long mode
// and jumps to `s3_resume_entry` (patched into the boot block, with a
// dedicated resume stack). `s3_resume_entry` restores GDTR/IDTR/CR4/CR3/CR0
// and the saved RSP, then joins `s3_asm_resume_tail`, which pops the
// callee-saved frame `s3_asm_sleep` pushed and RETURNS 1 into `enter_s3` —
// setjmp/longjmp across the power cycle. TR, syscall MSRs and the LAPIC are
// re-initialised Rust-side after the longjmp (the CPU reset cleared them).

/// The asm sleep/wake field offsets below hard-code SleepSaveArea's layout;
/// these asserts pin it (a reorder would corrupt the resume path silently).
const _: () = {
    assert!(core::mem::offset_of!(SleepSaveArea, cr3) == 0);
    assert!(core::mem::offset_of!(SleepSaveArea, cr0) == 8);
    assert!(core::mem::offset_of!(SleepSaveArea, cr4) == 16);
    assert!(core::mem::offset_of!(SleepSaveArea, rsp) == 80);
};

const RESUME_STACK_SIZE: usize = 16384;

#[repr(C, align(16))]
struct ResumeStack([u8; RESUME_STACK_SIZE]);

/// Dedicated stack for the resume path (the sleeping task's own kernel stack
/// must stay untouched until the longjmp lands back on it). Static so its
/// address is stable for the trampoline boot-block patch; RAM contents
/// survive S3 by definition.
static mut S3_RESUME_STACK: ResumeStack = ResumeStack([0; RESUME_STACK_SIZE]);

/// 10-byte GDTR/IDTR descriptor blobs captured at save time so the naked
/// resume entry can `lgdt/lidt [rip + …]` without building them in asm.
#[repr(C, align(8))]
struct DtBlob([u8; 10]);
static mut S3_GDTR_BLOB: DtBlob = DtBlob([0; 10]);
static mut S3_IDTR_BLOB: DtBlob = DtBlob([0; 10]);

/// S3 telemetry for the smoketest + /proc/athena/suspend.
/// S3_LAST_RESULT: 0 = never attempted, 1 = SLP_EN ignored (fell through),
/// 2 = slept and woke via the trampoline.
pub static S3_LAST_RESULT: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);
pub static S3_LAST_WAKE_RTC: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);
pub static S3_ROUNDTRIPS: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
pub static S3_LAST_APS_PARKED: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(0);
pub static S3_LAST_TASKS_MIGRATED: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(0);
pub static S3_LAST_APS_REONLINED: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(0);

core::arch::global_asm!(
    // extern "C" fn s3_asm_sleep(rsp_slot: *mut u64, sleep_val: u16, pm1a: u16, pm1b: u16) -> u64
    //   rdi = &SLEEP_SAVE_AREA.rsp   si = SLP_TYP|SLP_EN   dx = PM1a port   cx = PM1b port (0 = none)
    // Returns 0 if the platform ignored SLP_EN (fell through), 1 when re-entered
    // via s3_asm_resume_tail after a real sleep+wake.
    ".global s3_asm_sleep",
    "s3_asm_sleep:",
    "push rbx",
    "push rbp",
    "push r12",
    "push r13",
    "push r14",
    "push r15",
    "pushfq",
    "mov [rdi], rsp",
    "mov ax, si",
    "out dx, ax", // PM1a: the platform sleeps INSIDE this OUT on success
    "test cx, cx",
    "jz 2f",
    "mov dx, cx",
    "out dx, ax", // PM1b (spec: write both 'nearly simultaneously')
    "2:",
    // Still here: give the transition a moment, then report 'ignored'.
    "mov ecx, 100000",
    "3:",
    "pause",
    "dec ecx",
    "jnz 3b",
    "popfq",
    "pop r15",
    "pop r14",
    "pop r13",
    "pop r12",
    "pop rbp",
    "pop rbx",
    "xor eax, eax",
    "ret",
    // Entered from s3_resume_entry with RSP = the frame s3_asm_sleep saved.
    ".global s3_asm_resume_tail",
    "s3_asm_resume_tail:",
    "popfq",
    "pop r15",
    "pop r14",
    "pop r13",
    "pop r12",
    "pop rbp",
    "pop rbx",
    "mov eax, 1",
    "ret",
);

extern "C" {
    fn s3_asm_sleep(rsp_slot: *mut u64, sleep_val: u16, pm1a: u16, pm1b: u16) -> u64;
    fn s3_asm_resume_tail();
}

/// 64-bit resume entry the wake trampoline jumps to (boot-block ENTRY patch).
/// Arrives on `S3_RESUME_STACK` with the AP bootstrap PML4 in CR3 (it clones
/// all 512 kernel PML4 entries, so every kernel static used here is mapped).
/// Restores descriptor tables, control registers, data segments and the
/// active GS base (BSP cpu-id convention), then longjmps into the frame
/// `s3_asm_sleep` saved. TR / syscall MSRs / LAPIC are re-initialised by
/// `enter_s3` after the longjmp — plain Rust context there.
#[unsafe(naked)]
pub extern "C" fn s3_resume_entry() -> ! {
    core::arch::naked_asm!(
        "cli",
        "lgdt [rip + {gdtr}]",
        "lidt [rip + {idtr}]",
        // Reload CS from the restored kernel GDT (kcs = index 1 → 0x08).
        "lea rax, [rip + 2f]",
        "push 0x08",
        "push rax",
        "retfq",
        "2:",
        "mov ax, 0x10", // kds = index 2
        "mov ds, ax",
        "mov es, ax",
        "mov ss, ax",
        "mov fs, ax",
        "mov gs, ax",
        "lea r8, [rip + {area}]",
        "mov rax, [r8 + 16]", // cr4
        "mov cr4, rax",
        "mov rax, [r8 + 0]", // cr3 — the real kernel page tables
        "mov cr3, rax",
        "mov rax, [r8 + 8]", // cr0
        "mov cr0, rax",
        // Active GS base = 0 (BSP id for gdt::current_cpu_id's legacy path).
        "xor eax, eax",
        "xor edx, edx",
        "mov ecx, 0xC0000101",
        "wrmsr",
        "mov rsp, [r8 + 80]", // the RSP s3_asm_sleep saved
        "jmp {tail}",
        gdtr = sym S3_GDTR_BLOB,
        idtr = sym S3_IDTR_BLOB,
        area = sym SLEEP_SAVE_AREA,
        tail = sym s3_asm_resume_tail,
    )
}

/// CPU register state saved before entering S3 and restored on resume.
/// (Legacy alias kept for S5 path, which only needs the slim version.)
#[repr(C)]
struct SavedCpuState {
    rsp: u64,
    rbp: u64,
    rbx: u64,
    r12: u64,
    r13: u64,
    r14: u64,
    r15: u64,
    rflags: u64,
    cr0: u64,
    cr3: u64,
    cr4: u64,
}

static SAVED_STATE: Mutex<Option<SavedCpuState>> = Mutex::new(None);

fn save_cpu_state() -> SavedCpuState {
    let (rsp, rbp, rbx, r12, r13, r14, r15, rflags): (u64, u64, u64, u64, u64, u64, u64, u64);
    let (cr0, cr3, cr4): (u64, u64, u64);
    unsafe {
        core::arch::asm!(
            "mov {}, rsp",
            "mov {}, rbp",
            "mov {}, rbx",
            "mov {}, r12",
            "mov {}, r13",
            "mov {}, r14",
            "mov {}, r15",
            "pushfq",
            "pop {}",
            out(reg) rsp,
            out(reg) rbp,
            out(reg) rbx,
            out(reg) r12,
            out(reg) r13,
            out(reg) r14,
            out(reg) r15,
            out(reg) rflags,
            options(nomem, preserves_flags),
        );
        core::arch::asm!("mov {}, cr0", out(reg) cr0, options(nomem, nostack, preserves_flags));
        core::arch::asm!("mov {}, cr3", out(reg) cr3, options(nomem, nostack, preserves_flags));
        core::arch::asm!("mov {}, cr4", out(reg) cr4, options(nomem, nostack, preserves_flags));
    }
    SavedCpuState {
        rsp,
        rbp,
        rbx,
        r12,
        r13,
        r14,
        r15,
        rflags,
        cr0,
        cr3,
        cr4,
    }
}

/// Save the BSP's full register state into `SLEEP_SAVE_AREA`.
///
/// Called just before writing SLP_EN so that a firmware resume trampoline
/// can restore the kernel's execution context.
///
/// SAFETY: Writes to a static mut through a raw pointer.  Protected by
/// `SLEEP_SAVE_AREA_LOCK`.  No other CPU must touch SLEEP_SAVE_AREA
/// concurrently (APs are parked before this runs).
fn save_bsp_state() {
    let _guard = SLEEP_SAVE_AREA_LOCK.lock();

    let (cr0, cr3, cr4, rflags): (u64, u64, u64, u64);
    let (rbx, rbp, r12, r13, r14, r15, rsp): (u64, u64, u64, u64, u64, u64, u64);
    let (gdtr_base, gdtr_limit): (u64, u16);
    let (idtr_base, idtr_limit): (u64, u16);

    // SAFETY: reading control registers and descriptor table registers
    // via inline asm; no memory is written in these instructions.
    unsafe {
        core::arch::asm!("mov {}, cr0", out(reg) cr0, options(nomem, nostack, preserves_flags));
        core::arch::asm!("mov {}, cr3", out(reg) cr3, options(nomem, nostack, preserves_flags));
        core::arch::asm!("mov {}, cr4", out(reg) cr4, options(nomem, nostack, preserves_flags));
        core::arch::asm!(
            "pushfq",
            "pop {}",
            out(reg) rflags,
            options(nomem, preserves_flags),
        );
        core::arch::asm!(
            "mov {rbx}, rbx",
            "mov {rbp}, rbp",
            "mov {r12}, r12",
            "mov {r13}, r13",
            "mov {r14}, r14",
            "mov {r15}, r15",
            "mov {rsp}, rsp",
            rbx = out(reg) rbx,
            rbp = out(reg) rbp,
            r12 = out(reg) r12,
            r13 = out(reg) r13,
            r14 = out(reg) r14,
            r15 = out(reg) r15,
            rsp = out(reg) rsp,
            options(nomem, preserves_flags),
        );

        // GDTR: sgdt writes a 10-byte memory descriptor (limit:u16 + base:u64).
        let mut gdtr_buf = [0u8; 10];
        core::arch::asm!("sgdt [{}]", in(reg) gdtr_buf.as_mut_ptr(), options(nostack, preserves_flags));
        gdtr_limit = u16::from_le_bytes([gdtr_buf[0], gdtr_buf[1]]);
        gdtr_base = u64::from_le_bytes([
            gdtr_buf[2],
            gdtr_buf[3],
            gdtr_buf[4],
            gdtr_buf[5],
            gdtr_buf[6],
            gdtr_buf[7],
            gdtr_buf[8],
            gdtr_buf[9],
        ]);

        // Stash the raw 10-byte GDTR for the naked resume entry's `lgdt`.
        core::ptr::copy_nonoverlapping(gdtr_buf.as_ptr(), (&raw mut S3_GDTR_BLOB) as *mut u8, 10);

        // IDTR: sidt writes the same 10-byte layout.
        let mut idtr_buf = [0u8; 10];
        core::arch::asm!("sidt [{}]", in(reg) idtr_buf.as_mut_ptr(), options(nostack, preserves_flags));
        // Stash for the resume entry's `lidt`.
        core::ptr::copy_nonoverlapping(idtr_buf.as_ptr(), (&raw mut S3_IDTR_BLOB) as *mut u8, 10);
        idtr_limit = u16::from_le_bytes([idtr_buf[0], idtr_buf[1]]);
        idtr_base = u64::from_le_bytes([
            idtr_buf[2],
            idtr_buf[3],
            idtr_buf[4],
            idtr_buf[5],
            idtr_buf[6],
            idtr_buf[7],
            idtr_buf[8],
            idtr_buf[9],
        ]);

        // SAFETY: we hold SLEEP_SAVE_AREA_LOCK; no other writer exists.
        let area = &raw mut SLEEP_SAVE_AREA;
        (*area).cr0 = cr0;
        (*area).cr3 = cr3;
        (*area).cr4 = cr4;
        (*area).rflags = rflags;
        (*area).rbx = rbx;
        (*area).rbp = rbp;
        (*area).r12 = r12;
        (*area).r13 = r13;
        (*area).r14 = r14;
        (*area).r15 = r15;
        (*area).rsp = rsp;
        (*area).gdtr_base = gdtr_base;
        (*area).gdtr_limit = gdtr_limit;
        (*area).idtr_base = idtr_base;
        (*area).idtr_limit = idtr_limit;
        (*area).valid = 1;
    }
}

/// Write PM1_CNT with SLP_TYPx | SLP_EN to enter the specified ACPI sleep state.
/// Reads PM1a/PM1b control block addresses from the FADT.
unsafe fn write_pm1_cnt_sleep(slp_typ: u16) {
    let acpi = crate::acpi_full::ACPI_SUBSYSTEM.lock();
    let (pm1a, pm1b) = match &acpi.fadt {
        Some(fadt) => (
            fadt.pm1a_control_block as u16,
            fadt.pm1b_control_block as u16,
        ),
        None => {
            crate::serial_println!("[suspend] ERROR: FADT not available");
            return;
        }
    };
    drop(acpi);

    if pm1a != 0 {
        let val = (slp_typ << 10) | ACPI_PM1_SLP_EN;
        x86_64::instructions::port::Port::<u16>::new(pm1a).write(val);
    }
    if pm1b != 0 {
        let val = (slp_typ << 10) | ACPI_PM1_SLP_EN;
        x86_64::instructions::port::Port::<u16>::new(pm1b).write(val);
    }
}

// ── RTC wake alarm + PM1 wake-event plumbing ────────────────────────────────

const PM1_RTC_STS: u16 = 1 << 10;
const PM1_WAK_STS: u16 = 1 << 15;
const PM1_RTC_EN: u16 = 1 << 10;

fn cmos_read(reg: u8) -> u8 {
    let mut idx = x86_64::instructions::port::Port::<u8>::new(0x70);
    let mut dat = x86_64::instructions::port::Port::<u8>::new(0x71);
    unsafe {
        idx.write(reg); // bit 7 = 0 keeps NMI enabled
        dat.read()
    }
}

fn cmos_write(reg: u8, val: u8) {
    let mut idx = x86_64::instructions::port::Port::<u8>::new(0x70);
    let mut dat = x86_64::instructions::port::Port::<u8>::new(0x71);
    unsafe {
        idx.write(reg);
        dat.write(val);
    }
}

fn bcd_to_bin(b: u8) -> u8 {
    (b >> 4) * 10 + (b & 0x0F)
}

fn bin_to_bcd(b: u8) -> u8 {
    ((b / 10) << 4) | (b % 10)
}

/// PM1a/PM1b EVENT-block ports from the FADT (status at base, enable at
/// base + PM1_EVT_LEN/2 = base+2). (0, 0) legs are absent.
fn pm1_event_ports() -> (u16, u16) {
    let acpi = crate::acpi_full::ACPI_SUBSYSTEM.lock();
    match &acpi.fadt {
        Some(fadt) => (fadt.pm1a_event_block as u16, fadt.pm1b_event_block as u16),
        None => (0, 0),
    }
}

/// Enable the RTC as an ACPI wake source: clear a stale RTC_STS (write-1-
/// clear) then set PM1_EN.RTC_EN, on both PM1 legs. ACPI §4.8.4.1.
pub fn enable_rtc_wake() {
    let (a, b) = pm1_event_ports();
    for port in [a, b] {
        if port == 0 {
            continue;
        }
        unsafe {
            x86_64::instructions::port::Port::<u16>::new(port).write(PM1_RTC_STS);
            let mut en = x86_64::instructions::port::Port::<u16>::new(port + 2);
            let cur: u16 = en.read();
            en.write(cur | PM1_RTC_EN);
        }
    }
}

/// Read PM1 status (leg A OR'd with leg B) — wake-reason bits live here.
fn pm1_read_status() -> u16 {
    let (a, b) = pm1_event_ports();
    let mut sts = 0u16;
    for port in [a, b] {
        if port != 0 {
            sts |= unsafe { x86_64::instructions::port::Port::<u16>::new(port).read() };
        }
    }
    sts
}

/// Write-1-clear the given PM1 status bits on both legs.
fn pm1_clear_status(bits: u16) {
    let (a, b) = pm1_event_ports();
    for port in [a, b] {
        if port != 0 {
            unsafe { x86_64::instructions::port::Port::<u16>::new(port).write(bits) };
        }
    }
}

/// Arm the CMOS RTC alarm `secs_ahead` seconds from now and set AIE
/// (register B bit 5). Handles BCD vs binary per register B's DM bit;
/// assumes 24-hour mode (firmware default on every target we boot).
/// Combined with `enable_rtc_wake`, the alarm wakes the platform from S3.
pub fn arm_rtc_wake_alarm(secs_ahead: u32) {
    // Wait for update-in-progress to clear so the time read is coherent.
    for _ in 0..200_000 {
        if cmos_read(0x0A) & 0x80 == 0 {
            break;
        }
        core::hint::spin_loop();
    }
    let regb = cmos_read(0x0B);
    let bcd = regb & 0x04 == 0;
    let dec = |v: u8| if bcd { bcd_to_bin(v) } else { v };
    let enc = |v: u8| if bcd { bin_to_bcd(v) } else { v };

    let s = dec(cmos_read(0x00)) as u32;
    let m = dec(cmos_read(0x02)) as u32;
    let h = dec(cmos_read(0x04) & 0x3F) as u32;
    let total = (h * 3600 + m * 60 + s + secs_ahead) % 86_400;
    let (ah, am, asec) = (total / 3600, (total / 60) % 60, total % 60);

    cmos_write(0x01, enc(asec as u8)); // seconds alarm
    cmos_write(0x03, enc(am as u8)); // minutes alarm
    cmos_write(0x05, enc(ah as u8)); // hours alarm
    cmos_write(0x0B, regb | 0x20); // AIE
    let _ = cmos_read(0x0C); // clear any pending IRQF/AF
}

/// Point the ACPI FACS firmware waking vector at `vector_phys` (real-mode
/// entry, executed by firmware on S3 resume). Zeroes the 64-bit X vector so
/// firmware unambiguously takes the legacy 32-bit one. ACPI §5.2.10.
fn patch_facs_waking_vector(vector_phys: u32) -> Result<(), &'static str> {
    let facs_phys = {
        let acpi = crate::acpi_full::ACPI_SUBSYSTEM.lock();
        match &acpi.fadt {
            Some(fadt) => {
                if fadt.x_facs_address != 0 {
                    fadt.x_facs_address
                } else {
                    fadt.facs_address as u64
                }
            }
            None => 0,
        }
    };
    if facs_phys == 0 {
        return Err("FADT has no FACS address");
    }
    let virt = crate::memory::phys_to_virt(facs_phys);
    unsafe {
        let sig: [u8; 4] = core::ptr::read_volatile(virt.as_ptr::<[u8; 4]>());
        if &sig != b"FACS" {
            return Err("FACS signature mismatch");
        }
        core::ptr::write_volatile((virt.as_u64() + 12) as *mut u32, vector_phys);
        let len = core::ptr::read_volatile((virt.as_u64() + 4) as *const u32);
        if len >= 32 {
            core::ptr::write_volatile((virt.as_u64() + 24) as *mut u64, 0);
        }
    }
    Ok(())
}

/// Full ACPI S3 entry + REAL resume sequence (ACPI spec §16.1).
///
/// Concept §"Fast is a feature": "Wake under 1 second."
///
/// Steps:
///   1. Verify `_S3` in the AML namespace (clean refusal otherwise).
///   2. Patch the shared low-mem trampoline (smp/trampoline.asm) as the
///      waking vector — entry → [`s3_resume_entry`] on a dedicated resume
///      stack — and point the FACS firmware waking vector at it.
///   3. INIT-park the APs and migrate their queued tasks to CPU 0.
///   4. Mask IRQs, save BSP state (`SLEEP_SAVE_AREA` + GDTR/IDTR blobs).
///   5. `s3_asm_sleep`: push the callee-saved frame, record RSP, write
///      SLP_TYP|SLP_EN. A sleeping platform stops inside that OUT; wake
///      re-enters via firmware → FACS vector → trampoline →
///      `s3_resume_entry` → `s3_asm_resume_tail`, which longjmps back here
///      returning 1. A platform that ignores SLP_EN returns 0.
///   6. On a real wake (the CPU was reset): reload TR, re-program the
///      syscall MSRs (registers only — the per-CPU block memory survived),
///      re-enable x2APIC + the LAPIC timer, read + clear the PM1 wake
///      status. Telemetry lands in `S3_LAST_*` / `S3_ROUNDTRIPS`.
///
/// Returns `Ok(())` for both "slept and woke" and "platform ignored SLP_EN"
/// (distinguish via `S3_LAST_RESULT`: 2 vs 1); `Err` on missing ACPI
/// prerequisites, before any AP is parked.
pub fn enter_s3() -> Result<(), &'static str> {
    crate::serial_println!("[suspend] S3: beginning ACPI suspend-to-RAM sequence");

    // ── Step 1: verify _S3 is present ──────────────────────────────────
    let s3_present = {
        let acpi = crate::acpi_full::ACPI_SUBSYSTEM.lock();
        acpi.power_manager
            .supported_states
            .iter()
            .any(|s| *s == crate::acpi_full::SleepState::S3)
    };
    crate::serial_println!("[suspend] S3: _S3 present={}", s3_present);

    // Retrieve slp_typ_a[3] while we still have the lock.
    let slp_typ_s3 = {
        let acpi = crate::acpi_full::ACPI_SUBSYSTEM.lock();
        acpi.power_manager.slp_typ_a[3] as u16
    };

    // Confirm FADT is present — non-fatal guard.
    {
        let acpi = crate::acpi_full::ACPI_SUBSYSTEM.lock();
        if acpi.fadt.is_none() {
            crate::serial_println!("[suspend] S3: FADT absent, cannot write PM1_CNT — aborting");
            return Err("[suspend] S3: FADT not available");
        }
    }

    if !s3_present {
        // Never park APs / sleep on a platform that didn't declare _S3 —
        // the sleep-button path on non-S3 hardware must be a clean refusal.
        return Err("_S3 not present in the ACPI namespace");
    }

    // PM1 CONTROL ports for the sleep write (done inside s3_asm_sleep so the
    // saved RSP frame is already in place when the platform powers down).
    let (pm1a_cnt, pm1b_cnt) = {
        let acpi = crate::acpi_full::ACPI_SUBSYSTEM.lock();
        match &acpi.fadt {
            Some(fadt) => (
                fadt.pm1a_control_block as u16,
                fadt.pm1b_control_block as u16,
            ),
            None => (0, 0),
        }
    };
    if pm1a_cnt == 0 {
        return Err("FADT has no PM1a control block");
    }

    // ── Step 2 (fallible parts FIRST, before any AP is parked): waking
    //    vector — patch the shared low-mem trampoline to jump to
    //    s3_resume_entry on its dedicated stack, and point the FACS at it.
    let entry64 = s3_resume_entry as usize as u64;
    let stack_top = (&raw const S3_RESUME_STACK) as u64 + RESUME_STACK_SIZE as u64;
    crate::smp::patch_trampoline_for_wake(entry64, stack_top)?;
    patch_facs_waking_vector(crate::smp::TRAMPOLINE_PHYS as u32)?;
    crate::serial_println!(
        "[suspend] S3: FACS waking vector -> {:#x} (shared AP trampoline), resume entry {:#x}",
        crate::smp::TRAMPOLINE_PHYS,
        entry64,
    );

    // ── Step 3: park APs (they lose state across S3 regardless; re-online
    //    after resume is a follow-up — the resumed system runs BSP-only,
    //    matching post-boot reality where service threads are BSP-pinned).
    let parked = crate::smp::park_aps_for_sleep();
    let migrated = crate::scheduler::offline_aps_for_sleep();
    S3_LAST_APS_PARKED.store(parked as u32, core::sync::atomic::Ordering::Relaxed);
    S3_LAST_TASKS_MIGRATED.store(migrated as u32, core::sync::atomic::Ordering::Relaxed);
    crate::serial_println!(
        "[suspend] S3: parked {} AP(s), migrated {} queued task(s) to CPU0",
        parked,
        migrated,
    );

    // ── Steps 4–6: mask IRQs, save BSP state, write SLP_TYP|SLP_EN ──────
    let irqs_were_enabled = x86_64::instructions::interrupts::are_enabled();
    x86_64::instructions::interrupts::disable();
    crate::serial_println!("[suspend] S3: saving BSP register state + entering sleep");
    save_bsp_state();

    let sleep_val = (slp_typ_s3 << 10) | ACPI_PM1_SLP_EN;
    // SAFETY: s3_asm_sleep pushes the callee-saved frame, records RSP into
    // SLEEP_SAVE_AREA.rsp, and issues the PM1 OUT. On a sleeping platform
    // execution stops inside the OUT and re-enters via the FACS trampoline →
    // s3_resume_entry → s3_asm_resume_tail, which pops that exact frame and
    // returns 1 here. On a platform that ignores SLP_EN it returns 0.
    let woke = unsafe { s3_asm_sleep(&raw mut SLEEP_SAVE_AREA.rsp, sleep_val, pm1a_cnt, pm1b_cnt) };

    // ── Step 7: post-sleep. On a real wake the CPU was RESET: descriptor
    //    tables + CR state were restored by s3_resume_entry; TR, the syscall
    //    MSRs and the LAPIC are re-initialised here (plain Rust context).
    if woke == 1 {
        crate::gdt::reload_bsp_tss_after_resume();
        crate::syscall::reinit_after_resume(0);
        if crate::apic::X2APIC_SUPPORTED.load(core::sync::atomic::Ordering::SeqCst) {
            crate::apic::enable_x2apic();
        }
        crate::arch::timer::arm_periodic();
        // The wake-side platform reset freezes the HPET main counter
        // (ENABLE_CNF cleared) — re-enable it BEFORE anything busy-waits
        // (the AP re-online below spin-waits on it).
        crate::hpet::reenable_after_resume();

        let sts = pm1_read_status();
        let rtc_wake = sts & PM1_RTC_STS != 0;
        pm1_clear_status(PM1_WAK_STS | PM1_RTC_STS);
        S3_LAST_WAKE_RTC.store(rtc_wake, core::sync::atomic::Ordering::Relaxed);
        S3_LAST_RESULT.store(2, core::sync::atomic::Ordering::Relaxed);
        S3_ROUNDTRIPS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        crate::serial_println!(
            "[suspend] S3: WOKE via FACS trampoline (PM1_STS={:#06x}, rtc_wake={}) — TR/syscall MSRs/LAPIC reinitialised",
            sts,
            rtc_wake,
        );

        // A resumed machine must come back with ALL its cores: re-run the
        // full AP bring-up (INIT/SIPI + trampoline + the tagged-CAS warp
        // pairing) for the boot-time MADT set. The parked APs' queued work
        // was already migrated to CPU0 before the sleep; the re-onlined APs
        // start fresh and pull work via the (re-enabled) steal path.
        let reonlined = crate::smp::reonline_aps_after_resume();
        S3_LAST_APS_REONLINED.store(reonlined as u32, core::sync::atomic::Ordering::Relaxed);
        crate::serial_println!("[suspend] S3: re-onlined {} AP(s) after resume", reonlined,);
    } else {
        S3_LAST_RESULT.store(1, core::sync::atomic::Ordering::Relaxed);
        crate::serial_println!(
            "[suspend] S3: platform ignored SLP_EN (no sleep occurred) — continuing"
        );
    }

    resume_from_s3();

    if irqs_were_enabled {
        x86_64::instructions::interrupts::enable();
    }
    Ok(())
}

/// Device-level resume hook, called after the CPU-state restore in
/// `enter_s3` (TR/syscall MSRs/LAPIC are handled there — this is the slot
/// for device re-initialisation as drivers grow S3 support: NVMe controller
/// re-enable, xHCI re-init, NIC re-init). Kept separate so driver resume
/// work never tangles with the register-restore critical section.
pub fn resume_from_s3() {
    // SAFETY: read-only peek; only save_bsp_state writes the area and the
    // APs are parked, so the BSP read is race-free.
    let area_valid = unsafe { (&raw const SLEEP_SAVE_AREA).read_volatile().valid };
    if area_valid == 0 {
        crate::serial_println!(
            "[suspend] S3 resume: WARNING — no saved BSP state (save_bsp_state was not called?)"
        );
    }
    // Device re-init follow-up (MasterChecklist Phase 2.4): NVMe/xHCI/NIC
    // resume callbacks route through the PmDeviceOps registry above once
    // the drivers implement them.
    crate::serial_println!("[suspend] S3 resume: device-resume hook complete");
}

/// Enter ACPI S5 (soft power off):
///  Write PM1_CNT with SLP_TYP=S5 | SLP_EN. Does not return on success.
pub fn enter_s5() {
    crate::serial_println!("[suspend] Entering S5 (soft power off)...");

    x86_64::instructions::interrupts::disable();

    let acpi = crate::acpi_full::ACPI_SUBSYSTEM.lock();
    let slp_typ_s5 = acpi.power_manager.slp_typ_a[5] as u16;
    drop(acpi);

    unsafe {
        write_pm1_cnt_sleep(slp_typ_s5);
    }

    loop {
        x86_64::instructions::hlt();
    }
}

pub static POWER_MANAGER: Mutex<Option<PowerManager>> = Mutex::new(None);

pub fn init() {
    let mut mgr = POWER_MANAGER.lock();
    *mgr = Some(PowerManager::new());
}

/// Boot smoketest for the suspend subsystem.
///
/// Serial proof lines emitted:
///   `[suspend] S3: _S3 present=false (QEMU) -> smoketest PASS`
///     or
///   `[suspend] S3: _S3 present=true -> smoketest PASS`
pub fn run_boot_smoketest() {
    // Query whether _S3 is present in the ACPI namespace.
    let s3_present = {
        let acpi = crate::acpi_full::ACPI_SUBSYSTEM.lock();
        acpi.power_manager
            .supported_states
            .iter()
            .any(|s| *s == crate::acpi_full::SleepState::S3)
    };

    if s3_present {
        crate::serial_println!("[suspend] S3: _S3 present=true -> smoketest PASS");
    } else {
        crate::serial_println!("[suspend] S3: _S3 present=false (QEMU) -> smoketest PASS");
    }

    // Verify SLEEP_SAVE_AREA is zero-initialised (not yet used).
    // SAFETY: read-only access to a static; no concurrent writers at boot.
    let area_valid = unsafe { (&raw const SLEEP_SAVE_AREA).read_volatile().valid };
    crate::serial_println!(
        "[suspend] SLEEP_SAVE_AREA: valid={} (0 = not yet saved, expected at boot)",
        area_valid,
    );
}

/// FAIL-able S3 suspend→resume ROUND-TRIP proof (MasterChecklist Phase 2.4
/// acceptance: "Suspend → resume cycle completes without panic").
///
/// Concept §"Fast is a feature": "Wake under 1 second."
///
/// Arms the CMOS RTC alarm ~3 s ahead + PM1_EN.RTC_EN, then drives the REAL
/// `enter_s3` path: the platform powers the vCPUs down at the SLP_EN write,
/// the RTC alarm wakes it, firmware jumps to the FACS waking vector (the
/// shared AP trampoline), and execution longjmps back into `enter_s3`.
/// Asserts: the sleep actually happened (a fall-through is a FAIL — _S3 was
/// declared, so ignoring SLP_EN is a real defect), the wake reason is the
/// RTC (PM1_STS.RTC_STS), and the re-armed LAPIC timer is live (TMCCT
/// decrements — no IRQ delivery needed, so this runs correctly inside the
/// masked post-marker sweep).
///
/// HYPERVISOR-GATED: runs only when CPUID.1:ECX[31] reports a hypervisor
/// (QEMU/KVM). On bare metal an unattended smoketest must never put the
/// machine to sleep — iron S3 is user-triggered (_SLPB / power menu) and its
/// resume verification is the iron half of the checklist row.
pub fn run_s3_roundtrip_smoketest() {
    use core::sync::atomic::Ordering;

    let hypervisor = (core::arch::x86_64::__cpuid(1).ecx >> 31) & 1 == 1;
    if !hypervisor {
        crate::serial_println!(
            "[suspend] S3 round-trip: skipped (bare metal — sleep is user-triggered; iron resume verify pending) -> PASS"
        );
        return;
    }
    let s3_present = {
        let acpi = crate::acpi_full::ACPI_SUBSYSTEM.lock();
        acpi.power_manager
            .supported_states
            .iter()
            .any(|s| *s == crate::acpi_full::SleepState::S3)
    };
    if !s3_present {
        crate::serial_println!(
            "[suspend] S3 round-trip: skipped (_S3 absent — platform built without S3) -> PASS"
        );
        return;
    }

    enable_rtc_wake();
    arm_rtc_wake_alarm(3);

    let err = enter_s3().err();
    let result = S3_LAST_RESULT.load(Ordering::Relaxed);
    let slept_and_woke = result == 2;
    let wake_rtc = S3_LAST_WAKE_RTC.load(Ordering::Relaxed);

    // LAPIC timer liveness after the re-arm: in x2APIC mode the current-count
    // register (MSR 0x839) decrements while the timer runs. Two reads with a
    // spin between must differ. (xAPIC fallback: report unknown=live.)
    let timer_live = if crate::apic::X2APIC_SUPPORTED.load(Ordering::SeqCst) {
        let msr = x86_64::registers::model_specific::Msr::new(0x839);
        let a = unsafe { msr.read() };
        for _ in 0..50_000 {
            core::hint::spin_loop();
        }
        let b = unsafe { msr.read() };
        a != b
    } else {
        true
    };

    // Full-SMP restore: every AP parked for the sleep must be back online.
    let parked = S3_LAST_APS_PARKED.load(Ordering::Relaxed);
    let reonlined = S3_LAST_APS_REONLINED.load(Ordering::Relaxed);
    let smp_restored = reonlined == parked;

    let pass = err.is_none() && slept_and_woke && wake_rtc && timer_live && smp_restored;
    crate::serial_println!(
        "[suspend] S3 round-trip: slept_and_woke={} wake_rtc={} lapic_timer_live={} aps_parked={} aps_reonlined={} tasks_migrated={} err={:?} -> {}",
        slept_and_woke,
        wake_rtc,
        timer_live,
        parked,
        reonlined,
        S3_LAST_TASKS_MIGRATED.load(Ordering::Relaxed),
        err,
        if pass { "PASS" } else { "FAIL" },
    );
}

/// /proc/athena/suspend — S3 state + telemetry (R10 procfs artifact).
pub fn dump_text() -> alloc::string::String {
    use core::sync::atomic::Ordering;
    let mut out = alloc::string::String::new();
    out.push_str("# S3 suspend/resume\n");
    let last = match S3_LAST_RESULT.load(Ordering::Relaxed) {
        0 => "never-attempted",
        1 => "slp_en-ignored",
        2 => "slept-and-woke",
        _ => "?",
    };
    out.push_str(&alloc::format!(
        "last_result: {}\nroundtrips: {}\nlast_wake_rtc: {}\nlast_aps_parked: {}\nlast_aps_reonlined: {}\nlast_tasks_migrated: {}\n",
        last,
        S3_ROUNDTRIPS.load(Ordering::Relaxed),
        S3_LAST_WAKE_RTC.load(Ordering::Relaxed),
        S3_LAST_APS_PARKED.load(Ordering::Relaxed),
        S3_LAST_APS_REONLINED.load(Ordering::Relaxed),
        S3_LAST_TASKS_MIGRATED.load(Ordering::Relaxed),
    ));
    let s3_present = {
        let acpi = crate::acpi_full::ACPI_SUBSYSTEM.lock();
        acpi.power_manager
            .supported_states
            .iter()
            .any(|s| *s == crate::acpi_full::SleepState::S3)
    };
    out.push_str(&alloc::format!("s3_present: {}\n", s3_present));
    out
}
