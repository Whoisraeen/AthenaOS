#![allow(dead_code)]

extern crate alloc;

use alloc::{boxed::Box, collections::BTreeMap, string::String, vec, vec::Vec};
use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;

// ─── Jiffies ────────────────────────────────────────────────────────────────

pub static JIFFIES: AtomicU64 = AtomicU64::new(0);

/// Timer-interrupt frequency. MUST match the actual LAPIC periodic-timer rate:
/// the LAPIC is calibrated for a 10 ms period (`apic.rs` spins `spin_wait_us(10_000)`)
/// and `JIFFIES` increments once per LAPIC IRQ (`scheduler::timer_handler_inner` →
/// `tick()`), so 1 jiffy = 10 ms ⇒ HZ = 100. This was previously a false `1000`,
/// which made every jiffy↔ms/ns conversion (and `sys_msleep`/SYS_TIME/LinuxKPI
/// wait-timeouts) ~10× wrong (e.g. LinuxKPI `msleep(100)` slept ~1 s). Keep this
/// equal to the LAPIC rate. (The scheduler EDF clock has a SEPARATE 1/10 defect —
/// `now_us = tick * 1000` — tracked as its own rule-17 fix; it does NOT use HZ.)
pub const HZ: u64 = 100;

pub fn jiffies_to_ms(j: u64) -> u64 {
    j * 1000 / HZ
}

pub fn ms_to_jiffies(ms: u64) -> u64 {
    ms * HZ / 1000
}

pub fn jiffies_to_ns(j: u64) -> u64 {
    j * 1_000_000_000 / HZ
}

pub fn tick() {
    JIFFIES.fetch_add(1, Ordering::Relaxed);
    // Safe-mode bare-metal return must not depend on a worker receiving CPU.
    // This is a lock-free no-op unless bootlog_persist armed a deadline.
    crate::bootlog_persist::on_timer_tick();
}

/// R10 FAIL-able proof that `HZ` matches the 10 ms LAPIC tick: a jiffy is 10 ms,
/// so 1 s = 100 jiffies and 100 jiffies = 1 s = 1e9 ns. Reverting `HZ` to the old
/// `1000` trips every assertion.
pub fn run_boot_smoketest() {
    let pass = ms_to_jiffies(1000) == 100
        && jiffies_to_ms(100) == 1000
        && jiffies_to_ns(100) == 1_000_000_000
        && HZ == 100;
    crate::serial_println!(
        "[timers] hz smoketest: HZ={} ms_to_jiffies(1000)={} jiffies_to_ms(100)={}ms -> {}",
        HZ,
        ms_to_jiffies(1000),
        jiffies_to_ms(100),
        if pass { "PASS" } else { "FAIL" }
    );
}

// ═══════════════════════════════════════════════════════════════════════════
//  Timer Wheel (Hierarchical)
// ═══════════════════════════════════════════════════════════════════════════

const TVN_BITS: usize = 6;
const TVR_BITS: usize = 8;
const TVN_SIZE: usize = 1 << TVN_BITS; // 64
const TVR_SIZE: usize = 1 << TVR_BITS; // 256
const TVN_MASK: u64 = (TVN_SIZE - 1) as u64;
const TVR_MASK: u64 = (TVR_SIZE - 1) as u64;

const NUM_WHEELS: usize = 5;

#[derive(Debug, Clone, Copy)]
pub struct TimerFlags {
    pub periodic: bool,
    pub deferrable: bool,
    pub pinned: bool,
    pub irqsafe: bool,
}

impl Default for TimerFlags {
    fn default() -> Self {
        Self {
            periodic: false,
            deferrable: false,
            pinned: false,
            irqsafe: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct TimerEntry {
    pub id: u64,
    pub expires: u64,
    pub callback_id: u64,
    pub data: u64,
    pub flags: TimerFlags,
    pub interval: Option<u64>,
}

pub struct TimerWheel {
    tv1: Vec<Vec<TimerEntry>>,
    tv2: Vec<Vec<TimerEntry>>,
    tv3: Vec<Vec<TimerEntry>>,
    tv4: Vec<Vec<TimerEntry>>,
    tv5: Vec<Vec<TimerEntry>>,
    current_tick: u64,
    pending: u64,
    next_id: u64,
    cascade_pending: [u64; NUM_WHEELS],
}

impl TimerWheel {
    pub fn new() -> Self {
        let mut tv1 = Vec::with_capacity(TVR_SIZE);
        for _ in 0..TVR_SIZE {
            tv1.push(Vec::new());
        }

        let mut tv2 = Vec::with_capacity(TVN_SIZE);
        let mut tv3 = Vec::with_capacity(TVN_SIZE);
        let mut tv4 = Vec::with_capacity(TVN_SIZE);
        let mut tv5 = Vec::with_capacity(TVN_SIZE);
        for _ in 0..TVN_SIZE {
            tv2.push(Vec::new());
            tv3.push(Vec::new());
            tv4.push(Vec::new());
            tv5.push(Vec::new());
        }

        Self {
            tv1,
            tv2,
            tv3,
            tv4,
            tv5,
            current_tick: 0,
            pending: 0,
            next_id: 1,
            cascade_pending: [0; NUM_WHEELS],
        }
    }

    pub fn add_timer(
        &mut self,
        expires: u64,
        callback_id: u64,
        data: u64,
        flags: TimerFlags,
    ) -> u64 {
        let id = self.next_id;
        self.next_id += 1;

        let interval = if flags.periodic {
            Some(expires.saturating_sub(self.current_tick))
        } else {
            None
        };

        let entry = TimerEntry {
            id,
            expires,
            callback_id,
            data,
            flags,
            interval,
        };

        self.insert_entry(entry);
        self.pending += 1;
        id
    }

    fn insert_entry(&mut self, entry: TimerEntry) {
        let expires = entry.expires;
        let delta = expires.wrapping_sub(self.current_tick);

        if delta < TVR_SIZE as u64 {
            let idx = (expires & TVR_MASK) as usize;
            self.tv1[idx].push(entry);
        } else if delta < (1u64 << (TVR_BITS + TVN_BITS)) {
            let idx = ((expires >> TVR_BITS) & TVN_MASK) as usize;
            self.tv2[idx].push(entry);
        } else if delta < (1u64 << (TVR_BITS + 2 * TVN_BITS)) {
            let idx = ((expires >> (TVR_BITS + TVN_BITS)) & TVN_MASK) as usize;
            self.tv3[idx].push(entry);
        } else if delta < (1u64 << (TVR_BITS + 3 * TVN_BITS)) {
            let idx = ((expires >> (TVR_BITS + 2 * TVN_BITS)) & TVN_MASK) as usize;
            self.tv4[idx].push(entry);
        } else {
            let idx = ((expires >> (TVR_BITS + 3 * TVN_BITS)) & TVN_MASK) as usize;
            self.tv5[idx].push(entry);
        }
    }

    pub fn remove_timer(&mut self, id: u64) -> bool {
        for bucket in self.tv1.iter_mut() {
            if let Some(pos) = bucket.iter().position(|e| e.id == id) {
                bucket.swap_remove(pos);
                self.pending -= 1;
                return true;
            }
        }
        for tv in [&mut self.tv2, &mut self.tv3, &mut self.tv4, &mut self.tv5] {
            for bucket in tv.iter_mut() {
                if let Some(pos) = bucket.iter().position(|e| e.id == id) {
                    bucket.swap_remove(pos);
                    self.pending -= 1;
                    return true;
                }
            }
        }
        false
    }

    pub fn modify_timer(&mut self, id: u64, new_expires: u64) -> bool {
        let mut found_entry = None;

        for bucket in self.tv1.iter_mut() {
            if let Some(pos) = bucket.iter().position(|e| e.id == id) {
                found_entry = Some(bucket.swap_remove(pos));
                break;
            }
        }

        if found_entry.is_none() {
            for tv in [&mut self.tv2, &mut self.tv3, &mut self.tv4, &mut self.tv5] {
                let mut done = false;
                for bucket in tv.iter_mut() {
                    if let Some(pos) = bucket.iter().position(|e| e.id == id) {
                        found_entry = Some(bucket.swap_remove(pos));
                        done = true;
                        break;
                    }
                }
                if done {
                    break;
                }
            }
        }

        if let Some(mut entry) = found_entry {
            entry.expires = new_expires;
            self.insert_entry(entry);
            true
        } else {
            false
        }
    }

    pub fn tick(&mut self) -> Vec<TimerEntry> {
        self.current_tick += 1;
        let idx = (self.current_tick & TVR_MASK) as usize;

        if idx == 0 {
            self.cascade(0);
        }

        let expired = core::mem::take(&mut self.tv1[idx]);
        let mut result = Vec::with_capacity(expired.len());

        for entry in expired {
            self.pending -= 1;

            if entry.flags.periodic {
                if let Some(interval) = entry.interval {
                    let new_entry = TimerEntry {
                        id: entry.id,
                        expires: self.current_tick + interval,
                        callback_id: entry.callback_id,
                        data: entry.data,
                        flags: entry.flags,
                        interval: Some(interval),
                    };
                    self.insert_entry(new_entry);
                    self.pending += 1;
                }
            }

            result.push(entry);
        }

        result
    }

    pub fn next_expiry(&self) -> Option<u64> {
        let start = ((self.current_tick + 1) & TVR_MASK) as usize;
        for i in 0..TVR_SIZE {
            let idx = (start + i) % TVR_SIZE;
            if !self.tv1[idx].is_empty() {
                return Some(self.current_tick + 1 + i as u64);
            }
        }

        // Check higher wheels for approximate next expiry
        for tv in [&self.tv2, &self.tv3, &self.tv4, &self.tv5] {
            for bucket in tv {
                if let Some(entry) = bucket.first() {
                    return Some(entry.expires);
                }
            }
        }
        None
    }

    fn cascade(&mut self, level: usize) {
        match level {
            0 => {
                let tv2_idx = ((self.current_tick >> TVR_BITS) & TVN_MASK) as usize;
                let entries = core::mem::take(&mut self.tv2[tv2_idx]);
                self.cascade_pending[0] += entries.len() as u64;
                for entry in entries {
                    self.insert_entry(entry);
                }
                if tv2_idx == 0 {
                    self.cascade(1);
                }
            }
            1 => {
                let tv3_idx = ((self.current_tick >> (TVR_BITS + TVN_BITS)) & TVN_MASK) as usize;
                let entries = core::mem::take(&mut self.tv3[tv3_idx]);
                self.cascade_pending[1] += entries.len() as u64;
                for entry in entries {
                    self.insert_entry(entry);
                }
                if tv3_idx == 0 {
                    self.cascade(2);
                }
            }
            2 => {
                let tv4_idx =
                    ((self.current_tick >> (TVR_BITS + 2 * TVN_BITS)) & TVN_MASK) as usize;
                let entries = core::mem::take(&mut self.tv4[tv4_idx]);
                self.cascade_pending[2] += entries.len() as u64;
                for entry in entries {
                    self.insert_entry(entry);
                }
                if tv4_idx == 0 {
                    self.cascade(3);
                }
            }
            3 => {
                let tv5_idx =
                    ((self.current_tick >> (TVR_BITS + 3 * TVN_BITS)) & TVN_MASK) as usize;
                let entries = core::mem::take(&mut self.tv5[tv5_idx]);
                self.cascade_pending[3] += entries.len() as u64;
                for entry in entries {
                    self.insert_entry(entry);
                }
            }
            _ => {}
        }
    }

    fn index_for_level(&self, expires: u64, level: usize) -> usize {
        match level {
            0 => (expires & TVR_MASK) as usize,
            1 => ((expires >> TVR_BITS) & TVN_MASK) as usize,
            2 => ((expires >> (TVR_BITS + TVN_BITS)) & TVN_MASK) as usize,
            3 => ((expires >> (TVR_BITS + 2 * TVN_BITS)) & TVN_MASK) as usize,
            4 => ((expires >> (TVR_BITS + 3 * TVN_BITS)) & TVN_MASK) as usize,
            _ => 0,
        }
    }

    pub fn current_tick(&self) -> u64 {
        self.current_tick
    }

    pub fn pending_count(&self) -> u64 {
        self.pending
    }
}

// ═══════════════════════════════════════════════════════════════════════════
//  High-Resolution Timers
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HrTimerState {
    Inactive,
    Enqueued,
    Callback,
    Migrate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HrTimerMode {
    Absolute,
    Relative,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClockBase {
    Monotonic,
    Realtime,
    Boottime,
    Tai,
}

impl ClockBase {
    pub fn index(&self) -> usize {
        match self {
            Self::Monotonic => 0,
            Self::Realtime => 1,
            Self::Boottime => 2,
            Self::Tai => 3,
        }
    }
}

#[derive(Debug, Clone)]
pub struct HrTimer {
    pub id: u64,
    pub expires_ns: u64,
    pub interval_ns: Option<u64>,
    pub callback_id: u64,
    pub state: HrTimerState,
    pub mode: HrTimerMode,
    pub base: ClockBase,
}

pub struct HrTimerManager {
    timers: BTreeMap<u64, HrTimer>,
    next_id: u64,
    active_count: u64,
    resolution_ns: u64,
    clock_offsets: [i64; 4],
}

impl HrTimerManager {
    pub fn new(resolution_ns: u64) -> Self {
        Self {
            timers: BTreeMap::new(),
            next_id: 1,
            active_count: 0,
            resolution_ns,
            clock_offsets: [0; 4],
        }
    }

    pub fn start_timer(
        &mut self,
        expires_ns: u64,
        callback_id: u64,
        mode: HrTimerMode,
        base: ClockBase,
    ) -> u64 {
        let id = self.next_id;
        self.next_id += 1;

        let timer = HrTimer {
            id,
            expires_ns,
            interval_ns: None,
            callback_id,
            state: HrTimerState::Enqueued,
            mode,
            base,
        };

        self.timers.insert(expires_ns * 1_000_000 + id, timer);
        self.active_count += 1;
        id
    }

    pub fn start_periodic(
        &mut self,
        expires_ns: u64,
        interval_ns: u64,
        callback_id: u64,
        base: ClockBase,
    ) -> u64 {
        let id = self.next_id;
        self.next_id += 1;

        let timer = HrTimer {
            id,
            expires_ns,
            interval_ns: Some(interval_ns),
            callback_id,
            state: HrTimerState::Enqueued,
            mode: HrTimerMode::Absolute,
            base,
        };

        self.timers.insert(expires_ns * 1_000_000 + id, timer);
        self.active_count += 1;
        id
    }

    pub fn cancel_timer(&mut self, id: u64) -> bool {
        let key = self
            .timers
            .iter()
            .find(|(_, t)| t.id == id)
            .map(|(k, _)| *k);

        if let Some(key) = key {
            self.timers.remove(&key);
            self.active_count -= 1;
            true
        } else {
            false
        }
    }

    pub fn process(&mut self, now_ns: u64) -> Vec<HrTimer> {
        let mut expired = Vec::new();
        let mut reinsert = Vec::new();

        let cutoff = now_ns * 1_000_000 + u64::MAX / 2;
        let keys_to_remove: Vec<u64> = self
            .timers
            .range(..=cutoff)
            .filter(|(_, t)| t.expires_ns <= now_ns)
            .map(|(k, _)| *k)
            .collect();

        for key in keys_to_remove {
            if let Some(mut timer) = self.timers.remove(&key) {
                timer.state = HrTimerState::Callback;
                self.active_count -= 1;

                if let Some(interval) = timer.interval_ns {
                    let mut re = timer.clone();
                    re.expires_ns = now_ns + interval;
                    re.state = HrTimerState::Enqueued;
                    reinsert.push(re);
                }

                expired.push(timer);
            }
        }

        for timer in reinsert {
            let key = timer.expires_ns * 1_000_000 + timer.id;
            self.active_count += 1;
            self.timers.insert(key, timer);
        }

        expired
    }

    pub fn next_expiry_ns(&self) -> Option<u64> {
        self.timers.values().next().map(|t| t.expires_ns)
    }

    pub fn active_count(&self) -> u64 {
        self.active_count
    }

    pub fn resolution_ns(&self) -> u64 {
        self.resolution_ns
    }

    pub fn set_clock_offset(&mut self, base: ClockBase, offset_ns: i64) {
        self.clock_offsets[base.index()] = offset_ns;
    }

    pub fn get_clock_offset(&self, base: ClockBase) -> i64 {
        self.clock_offsets[base.index()]
    }
}

// ═══════════════════════════════════════════════════════════════════════════
//  Tickless (NO_HZ)
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TickMode {
    Periodic,
    NoHzIdle,
    NoHzFull,
}

pub struct TicklessManager {
    mode: TickMode,
    idle: bool,
    last_tick: u64,
    next_event: u64,
    skipped_ticks: u64,
    tick_stopped: bool,
    idle_calls: u64,
    idle_sleeps: u64,
    max_skip: u64,
}

impl TicklessManager {
    pub fn new(mode: TickMode) -> Self {
        Self {
            mode,
            idle: false,
            last_tick: 0,
            next_event: 0,
            skipped_ticks: 0,
            tick_stopped: false,
            idle_calls: 0,
            idle_sleeps: 0,
            max_skip: 1000,
        }
    }

    pub fn enter_idle(&mut self, now: u64, next_timer: Option<u64>) {
        self.idle = true;
        self.idle_calls += 1;
        self.last_tick = now;

        match self.mode {
            TickMode::Periodic => {
                self.next_event = now + 1;
            }
            TickMode::NoHzIdle | TickMode::NoHzFull => {
                if let Some(next) = next_timer {
                    // A timer already due (next < now) must skip 0 ticks and wake
                    // immediately, not underflow to a huge value clamped to
                    // max_skip (BUG-38: that sleeps the CPU for the maximum
                    // duration and blows every deadline).
                    let skip = next.saturating_sub(now).min(self.max_skip);
                    self.next_event = now + skip;
                    self.tick_stopped = skip > 1;
                } else {
                    self.next_event = now + self.max_skip;
                    self.tick_stopped = true;
                }
            }
        }
    }

    pub fn exit_idle(&mut self, now: u64) {
        if !self.idle {
            return;
        }
        self.idle = false;
        self.tick_stopped = false;

        let elapsed = now.saturating_sub(self.last_tick);
        if elapsed > 1 {
            self.skipped_ticks += elapsed - 1;
            self.idle_sleeps += 1;
        }
    }

    pub fn should_reprogram(&self, now: u64) -> bool {
        match self.mode {
            TickMode::Periodic => false,
            TickMode::NoHzIdle | TickMode::NoHzFull => now >= self.next_event,
        }
    }

    pub fn catch_up_ticks(&self, now: u64) -> u64 {
        now.saturating_sub(self.last_tick)
    }

    pub fn mode(&self) -> TickMode {
        self.mode
    }

    pub fn set_mode(&mut self, mode: TickMode) {
        self.mode = mode;
    }

    pub fn is_idle(&self) -> bool {
        self.idle
    }

    pub fn skipped_ticks(&self) -> u64 {
        self.skipped_ticks
    }

    pub fn set_max_skip(&mut self, max: u64) {
        self.max_skip = max;
    }

    pub fn next_event(&self) -> u64 {
        self.next_event
    }
}

// ═══════════════════════════════════════════════════════════════════════════
//  RTC Driver (MC146818 / CMOS)
// ═══════════════════════════════════════════════════════════════════════════

const CMOS_ADDR: u16 = 0x70;
const CMOS_DATA: u16 = 0x71;

const RTC_SECONDS: u8 = 0x00;
const RTC_MINUTES: u8 = 0x02;
const RTC_HOURS: u8 = 0x04;
const RTC_DAY_OF_WEEK: u8 = 0x06;
const RTC_DAY_OF_MONTH: u8 = 0x07;
const RTC_MONTH: u8 = 0x08;
const RTC_YEAR: u8 = 0x09;
const RTC_STATUS_A: u8 = 0x0A;
const RTC_STATUS_B: u8 = 0x0B;
const RTC_STATUS_C: u8 = 0x0C;
const RTC_CENTURY: u8 = 0x32;

const RTC_UIP: u8 = 0x80; // Update In Progress bit in Status A
const RTC_24H: u8 = 0x02; // 24-hour mode bit in Status B
const RTC_BINARY: u8 = 0x04; // Binary mode bit in Status B
const RTC_AIE: u8 = 0x20; // Alarm Interrupt Enable in Status B
const RTC_PIE: u8 = 0x40; // Periodic Interrupt Enable
const RTC_UIE: u8 = 0x10; // Update-ended Interrupt Enable

#[derive(Debug, Clone, Copy, Default)]
pub struct RtcTime {
    pub second: u8,
    pub minute: u8,
    pub hour: u8,
    pub day: u8,
    pub month: u8,
    pub year: u16,
    pub weekday: u8,
    pub yearday: u16,
}

impl RtcTime {
    pub fn to_unix_timestamp(&self) -> u64 {
        let mut y = self.year as u64;
        let mut m = self.month as u64;

        if m <= 2 {
            y -= 1;
            m += 12;
        }

        let days = 365 * y + y / 4 - y / 100 + y / 400 + (153 * (m - 3) + 2) / 5 + self.day as u64
            - 719469;

        days * 86400 + self.hour as u64 * 3600 + self.minute as u64 * 60 + self.second as u64
    }

    pub fn compute_yearday(&mut self) {
        let days_in_month: [u16; 12] = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
        let leap = self.year % 4 == 0 && (self.year % 100 != 0 || self.year % 400 == 0);

        let mut yday = 0u16;
        // BUG-39 fix: a failing/uninitialized RTC can report month 0; `month - 1`
        // would underflow to usize::MAX and clamp to 11 (wrongly treating it as
        // December). saturating_sub bounds month 0 to 0 days instead.
        for i in 0..(self.month as usize).saturating_sub(1).min(11) {
            yday += days_in_month[i];
            if i == 1 && leap {
                yday += 1;
            }
        }
        yday += self.day as u16;
        self.yearday = yday;
    }
}

pub struct RtcDriver {
    century_reg: u8,
    time: RtcTime,
    alarm: Option<RtcTime>,
    irq_enabled: bool,
    update_in_progress: bool,
}

impl RtcDriver {
    pub fn new() -> Self {
        let mut drv = Self {
            century_reg: RTC_CENTURY,
            time: RtcTime::default(),
            alarm: None,
            irq_enabled: false,
            update_in_progress: false,
        };
        drv.time = drv.read_time();
        drv
    }

    pub fn read_time(&mut self) -> RtcTime {
        while self.is_updating() {
            core::hint::spin_loop();
        }

        let mut second = self.read_cmos(RTC_SECONDS);
        let mut minute = self.read_cmos(RTC_MINUTES);
        let mut hour = self.read_cmos(RTC_HOURS);
        let weekday = self.read_cmos(RTC_DAY_OF_WEEK);
        let mut day = self.read_cmos(RTC_DAY_OF_MONTH);
        let mut month = self.read_cmos(RTC_MONTH);
        let mut year = self.read_cmos(RTC_YEAR) as u16;
        let century = self.read_cmos(self.century_reg) as u16;

        // Read a second time to handle update races
        while self.is_updating() {
            core::hint::spin_loop();
        }
        let s2 = self.read_cmos(RTC_SECONDS);
        let m2 = self.read_cmos(RTC_MINUTES);
        let h2 = self.read_cmos(RTC_HOURS);
        if s2 != second || m2 != minute || h2 != hour {
            second = s2;
            minute = m2;
            hour = h2;
            day = self.read_cmos(RTC_DAY_OF_MONTH);
            month = self.read_cmos(RTC_MONTH);
            year = self.read_cmos(RTC_YEAR) as u16;
        }

        let status_b = self.read_cmos(RTC_STATUS_B);

        if status_b & RTC_BINARY == 0 {
            second = self.bcd_to_binary(second);
            minute = self.bcd_to_binary(minute);
            hour = self.bcd_to_binary(hour & 0x7F) | (hour & 0x80);
            day = self.bcd_to_binary(day);
            month = self.bcd_to_binary(month);
            year = self.bcd_to_binary(year as u8) as u16;
        }

        if status_b & RTC_24H == 0 && hour & 0x80 != 0 {
            hour = ((hour & 0x7F) + 12) % 24;
        }

        let full_year = if century > 0 {
            self.bcd_to_binary(century as u8) as u16 * 100 + year
        } else if year < 70 {
            2000 + year
        } else {
            1900 + year
        };

        let mut time = RtcTime {
            second,
            minute,
            hour,
            day,
            month,
            year: full_year,
            weekday,
            yearday: 0,
        };
        time.compute_yearday();

        self.time = time;
        time
    }

    pub fn set_time(&mut self, time: &RtcTime) {
        let status_b = self.read_cmos(RTC_STATUS_B);
        let is_bcd = status_b & RTC_BINARY == 0;

        let second = if is_bcd {
            self.binary_to_bcd(time.second)
        } else {
            time.second
        };
        let minute = if is_bcd {
            self.binary_to_bcd(time.minute)
        } else {
            time.minute
        };
        let hour = if is_bcd {
            self.binary_to_bcd(time.hour)
        } else {
            time.hour
        };
        let day = if is_bcd {
            self.binary_to_bcd(time.day)
        } else {
            time.day
        };
        let month = if is_bcd {
            self.binary_to_bcd(time.month)
        } else {
            time.month
        };
        let year = if is_bcd {
            self.binary_to_bcd((time.year % 100) as u8)
        } else {
            (time.year % 100) as u8
        };
        let century = if is_bcd {
            self.binary_to_bcd((time.year / 100) as u8)
        } else {
            (time.year / 100) as u8
        };

        // Disable updates while writing
        self.write_cmos(RTC_STATUS_B, status_b | 0x80);

        self.write_cmos(RTC_SECONDS, second);
        self.write_cmos(RTC_MINUTES, minute);
        self.write_cmos(RTC_HOURS, hour);
        self.write_cmos(RTC_DAY_OF_MONTH, day);
        self.write_cmos(RTC_MONTH, month);
        self.write_cmos(RTC_YEAR, year);
        self.write_cmos(self.century_reg, century);

        // Re-enable updates
        self.write_cmos(RTC_STATUS_B, status_b);
        self.time = *time;
    }

    pub fn set_alarm(&mut self, time: &RtcTime) {
        let status_b = self.read_cmos(RTC_STATUS_B);
        let is_bcd = status_b & RTC_BINARY == 0;

        let second = if is_bcd {
            self.binary_to_bcd(time.second)
        } else {
            time.second
        };
        let minute = if is_bcd {
            self.binary_to_bcd(time.minute)
        } else {
            time.minute
        };
        let hour = if is_bcd {
            self.binary_to_bcd(time.hour)
        } else {
            time.hour
        };

        self.write_cmos(0x01, second);
        self.write_cmos(0x03, minute);
        self.write_cmos(0x05, hour);

        // Enable alarm interrupt
        let new_b = status_b | RTC_AIE;
        self.write_cmos(RTC_STATUS_B, new_b);

        self.alarm = Some(*time);
        self.irq_enabled = true;
    }

    pub fn clear_alarm(&mut self) {
        let status_b = self.read_cmos(RTC_STATUS_B);
        self.write_cmos(RTC_STATUS_B, status_b & !RTC_AIE);
        self.alarm = None;
    }

    pub fn handle_irq(&mut self) {
        let status_c = self.read_cmos(RTC_STATUS_C);
        if status_c & RTC_AIE != 0 {
            // Alarm fired
        }
        if status_c & RTC_PIE != 0 {
            // Periodic interrupt
        }
        if status_c & RTC_UIE != 0 {
            // Update-ended interrupt
        }
    }

    pub fn enable_periodic_irq(&mut self, rate: u8) {
        let rate = rate.clamp(3, 15);
        let mut status_a = self.read_cmos(RTC_STATUS_A);
        status_a = (status_a & 0xF0) | rate;
        self.write_cmos(RTC_STATUS_A, status_a);

        let status_b = self.read_cmos(RTC_STATUS_B);
        self.write_cmos(RTC_STATUS_B, status_b | RTC_PIE);
        self.irq_enabled = true;
    }

    pub fn disable_periodic_irq(&mut self) {
        let status_b = self.read_cmos(RTC_STATUS_B);
        self.write_cmos(RTC_STATUS_B, status_b & !RTC_PIE);
    }

    fn read_cmos(&self, reg: u8) -> u8 {
        unsafe {
            let mut addr_port = x86_64::instructions::port::Port::<u8>::new(CMOS_ADDR);
            let mut data_port = x86_64::instructions::port::Port::<u8>::new(CMOS_DATA);
            addr_port.write(reg | 0x80); // NMI disable bit
            data_port.read()
        }
    }

    fn write_cmos(&self, reg: u8, val: u8) {
        unsafe {
            let mut addr_port = x86_64::instructions::port::Port::<u8>::new(CMOS_ADDR);
            let mut data_port = x86_64::instructions::port::Port::<u8>::new(CMOS_DATA);
            addr_port.write(reg | 0x80);
            data_port.write(val);
        }
    }

    fn bcd_to_binary(&self, val: u8) -> u8 {
        (val & 0x0F) + ((val >> 4) * 10)
    }

    fn binary_to_bcd(&self, val: u8) -> u8 {
        ((val / 10) << 4) | (val % 10)
    }

    fn is_updating(&self) -> bool {
        self.read_cmos(RTC_STATUS_A) & RTC_UIP != 0
    }

    pub fn current_time(&self) -> &RtcTime {
        &self.time
    }
}

// ═══════════════════════════════════════════════════════════════════════════
//  TSC Calibration
// ═══════════════════════════════════════════════════════════════════════════

const PIT_CHANNEL2: u16 = 0x42;
const PIT_CMD: u16 = 0x43;
const PIT_GATE: u16 = 0x61;
const PIT_FREQUENCY: u64 = 1_193_182;

// Calibration runs for ~10ms using PIT
const CALIBRATION_TICKS: u64 = PIT_FREQUENCY / 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CalibrationMethod {
    Pit,
    Hpet,
    Pmtimer,
    Cpuid,
    Msr,
    Unknown,
}

pub struct TscCalibration {
    pub frequency_hz: u64,
    pub reliable: bool,
    pub invariant: bool,
    pub nonstop: bool,
    pub calibration_method: CalibrationMethod,
}

impl TscCalibration {
    pub fn calibrate() -> Self {
        // Try CPUID leaf 0x15 first (most accurate on modern CPUs)
        if let Some(freq) = Self::calibrate_via_cpuid() {
            return Self {
                frequency_hz: freq,
                reliable: true,
                invariant: true,
                nonstop: true,
                calibration_method: CalibrationMethod::Cpuid,
            };
        }

        // Try MSR-based calibration
        if let Some(freq) = Self::calibrate_via_msr() {
            return Self {
                frequency_hz: freq,
                reliable: true,
                invariant: true,
                nonstop: true,
                calibration_method: CalibrationMethod::Msr,
            };
        }

        // Fall back to PIT-based calibration
        let freq = Self::calibrate_via_pit();
        let invariant = Self::check_invariant_tsc();

        Self {
            frequency_hz: freq,
            reliable: freq > 0,
            invariant,
            nonstop: invariant,
            calibration_method: CalibrationMethod::Pit,
        }
    }

    fn calibrate_via_pit() -> u64 {
        unsafe {
            let mut cmd = x86_64::instructions::port::Port::<u8>::new(PIT_CMD);
            let mut ch2 = x86_64::instructions::port::Port::<u8>::new(PIT_CHANNEL2);
            let mut gate = x86_64::instructions::port::Port::<u8>::new(PIT_GATE);

            // Set PIT channel 2: mode 0, lobyte/hibyte, binary
            cmd.write(0xB0);

            // Load count
            let count = CALIBRATION_TICKS as u16;
            ch2.write((count & 0xFF) as u8);
            ch2.write((count >> 8) as u8);

            // Enable PIT channel 2 gate
            let g = gate.read();
            gate.write((g & 0xFD) | 0x01);

            // Read TSC before
            let tsc_start = Self::read_tsc();

            // Wait for PIT to count down (bit 5 of port 0x61 goes high)
            loop {
                if gate.read() & 0x20 != 0 {
                    break;
                }
                core::hint::spin_loop();
            }

            let tsc_end = Self::read_tsc();

            // Disable gate
            gate.write(g);

            let tsc_delta = tsc_end - tsc_start;
            // freq = tsc_delta * PIT_FREQUENCY / CALIBRATION_TICKS
            tsc_delta * 100 // since CALIBRATION_TICKS = PIT_FREQ/100, this gives Hz
        }
    }

    fn calibrate_via_hpet() -> u64 {
        // Read HPET main counter, wait ~10ms, read again
        // Use HPET period to compute elapsed time, then derive TSC frequency.
        // Requires HPET to be initialized already.
        let hpet = crate::hpet::HPET.lock();
        if let Some(ref h) = *hpet {
            let tsc_start = Self::read_tsc();
            let hpet_start = h.read_counter();

            // Spin for ~10ms (using HPET counter)
            let period_fs = h.period_fs() as u64;
            let target_fs = 10_000_000_000_000u64; // 10ms in femtoseconds
            let target_ticks = target_fs / period_fs;

            loop {
                let now = h.read_counter();
                if now.wrapping_sub(hpet_start) >= target_ticks {
                    break;
                }
                core::hint::spin_loop();
            }

            let tsc_end = Self::read_tsc();
            let hpet_end = h.read_counter();

            let hpet_delta = hpet_end.wrapping_sub(hpet_start);
            let elapsed_ns = hpet_delta * period_fs / 1_000_000;

            if elapsed_ns > 0 {
                let tsc_delta = tsc_end - tsc_start;
                return tsc_delta * 1_000_000_000 / elapsed_ns;
            }
        }
        0
    }

    fn calibrate_via_cpuid() -> Option<u64> {
        // CPUID leaf 0x15: TSC/core crystal clock ratio
        let cpuid = unsafe { core::arch::x86_64::__cpuid(0) };
        if cpuid.eax < 0x15 {
            return None;
        }

        let leaf15 = unsafe { core::arch::x86_64::__cpuid(0x15) };
        let denominator = leaf15.eax;
        let numerator = leaf15.ebx;
        let crystal_freq = leaf15.ecx;

        if denominator == 0 || numerator == 0 {
            return None;
        }

        if crystal_freq > 0 {
            Some((crystal_freq as u64) * (numerator as u64) / (denominator as u64))
        } else {
            // Some CPUs don't report crystal frequency; use known values based on family
            let leaf1 = unsafe { core::arch::x86_64::__cpuid(1) };
            let model = ((leaf1.eax >> 4) & 0xF) | (((leaf1.eax >> 16) & 0xF) << 4);

            let crystal = match model {
                0x55 => 25_000_000u64,        // Skylake-SP
                0x4E | 0x5E => 24_000_000u64, // Skylake client
                _ => return None,
            };

            Some(crystal * (numerator as u64) / (denominator as u64))
        }
    }

    fn calibrate_via_msr() -> Option<u64> {
        // MSR 0xCE (MSR_PLATFORM_INFO) contains max non-turbo ratio
        // TSC freq = base_freq * ratio
        // This is Intel-specific
        unsafe {
            let leaf1 = core::arch::x86_64::__cpuid(0);
            let vendor = [
                leaf1.ebx.to_le_bytes(),
                leaf1.edx.to_le_bytes(),
                leaf1.ecx.to_le_bytes(),
            ];
            let is_intel = vendor[0] == *b"Genu" && vendor[1] == *b"ineI" && vendor[2] == *b"ntel";

            if !is_intel {
                return None;
            }

            // Read MSR_PLATFORM_INFO
            let msr_val: u64;
            core::arch::asm!(
                "rdmsr",
                in("ecx") 0xCEu32,
                out("eax") _,
                out("edx") _,
                options(nomem, nostack),
            );
            // The above is illustrative; we need the actual value
            // bits [15:8] = max non-turbo ratio
            // Bus frequency is typically 100 MHz
            None
        }
    }

    fn check_invariant_tsc() -> bool {
        let cpuid = unsafe { core::arch::x86_64::__cpuid(0x80000000) };
        if cpuid.eax < 0x80000007 {
            return false;
        }
        let leaf = unsafe { core::arch::x86_64::__cpuid(0x80000007) };
        leaf.edx & (1 << 8) != 0
    }

    pub fn tsc_to_ns(&self, tsc: u64) -> u64 {
        if self.frequency_hz == 0 {
            return 0;
        }
        tsc * 1_000_000_000 / self.frequency_hz
    }

    pub fn ns_to_tsc(&self, ns: u64) -> u64 {
        ns * self.frequency_hz / 1_000_000_000
    }

    pub fn read_tsc() -> u64 {
        unsafe { core::arch::x86_64::_rdtsc() }
    }

    pub fn read_tsc_ordered() -> u64 {
        unsafe {
            let mut _aux: u32 = 0;
            let lo: u32;
            let hi: u32;
            core::arch::asm!(
                "rdtscp",
                out("eax") lo,
                out("edx") hi,
                out("ecx") _aux,
                options(nomem, nostack),
            );
            ((hi as u64) << 32) | (lo as u64)
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
//  Monotonic Clock
// ═══════════════════════════════════════════════════════════════════════════

pub struct MonotonicClock {
    tsc_cal: TscCalibration,
    base_tsc: u64,
    base_ns: u64,
}

impl MonotonicClock {
    pub fn new(tsc_cal: TscCalibration) -> Self {
        let base_tsc = TscCalibration::read_tsc();
        Self {
            tsc_cal,
            base_tsc,
            base_ns: 0,
        }
    }

    pub fn now_ns(&self) -> u64 {
        let current_tsc = TscCalibration::read_tsc();
        let delta = current_tsc.wrapping_sub(self.base_tsc);
        self.base_ns + self.tsc_cal.tsc_to_ns(delta)
    }

    pub fn now_us(&self) -> u64 {
        self.now_ns() / 1_000
    }

    pub fn now_ms(&self) -> u64 {
        self.now_ns() / 1_000_000
    }

    pub fn elapsed_ns(&self, start: u64) -> u64 {
        self.now_ns().saturating_sub(start)
    }

    pub fn frequency_hz(&self) -> u64 {
        self.tsc_cal.frequency_hz
    }
}

// ═══════════════════════════════════════════════════════════════════════════
//  Timer Subsystem (unified)
// ═══════════════════════════════════════════════════════════════════════════

pub struct TimerSubsystem {
    pub wheel: TimerWheel,
    pub hrtimer: HrTimerManager,
    pub tickless: TicklessManager,
    pub rtc: RtcDriver,
    pub tsc: TscCalibration,
    pub boot_time_ns: u64,
    pub boot_wall_clock_ns: u64,
}

impl TimerSubsystem {
    pub fn new() -> Self {
        let tsc = TscCalibration::calibrate();
        let boot_ns = TscCalibration::read_tsc();

        let mut rtc = RtcDriver::new();
        let time = rtc.read_time();
        let boot_wall = time.to_unix_timestamp().saturating_mul(1_000_000_000);

        Self {
            wheel: TimerWheel::new(),
            hrtimer: HrTimerManager::new(1_000),
            tickless: TicklessManager::new(TickMode::NoHzIdle),
            rtc,
            boot_time_ns: tsc.tsc_to_ns(boot_ns),
            boot_wall_clock_ns: boot_wall,
            tsc,
        }
    }

    pub fn tick(&mut self) {
        tick();

        let expired = self.wheel.tick();
        for entry in &expired {
            self.dispatch_timer_callback(entry);
        }

        let now_ns = self.uptime_ns();
        let hr_expired = self.hrtimer.process(now_ns);
        for timer in &hr_expired {
            self.dispatch_hrtimer_callback(timer);
        }
    }

    pub fn uptime_ns(&self) -> u64 {
        let current_tsc = TscCalibration::read_tsc();
        self.tsc
            .tsc_to_ns(current_tsc)
            .saturating_sub(self.boot_time_ns)
    }

    pub fn uptime_ms(&self) -> u64 {
        self.uptime_ns() / 1_000_000
    }

    pub fn wall_clock_ns(&self) -> u64 {
        self.boot_wall_clock_ns.saturating_add(self.uptime_ns())
    }

    fn dispatch_timer_callback(&self, _entry: &TimerEntry) {
        // In a real kernel: look up callback_id in a table and invoke
    }

    fn dispatch_hrtimer_callback(&self, _timer: &HrTimer) {
        // In a real kernel: look up callback_id and invoke
    }

    pub fn schedule_timeout_ms(&mut self, ms: u64, callback_id: u64) -> u64 {
        let ticks = ms_to_jiffies(ms);
        let expires = self.wheel.current_tick() + ticks;
        self.wheel
            .add_timer(expires, callback_id, 0, TimerFlags::default())
    }

    pub fn schedule_interval_ms(&mut self, ms: u64, callback_id: u64) -> u64 {
        let ticks = ms_to_jiffies(ms);
        let expires = self.wheel.current_tick() + ticks;
        self.wheel.add_timer(
            expires,
            callback_id,
            0,
            TimerFlags {
                periodic: true,
                ..TimerFlags::default()
            },
        )
    }

    pub fn schedule_hrtimer_ns(&mut self, ns: u64, callback_id: u64) -> u64 {
        let now = self.uptime_ns();
        self.hrtimer.start_timer(
            now + ns,
            callback_id,
            HrTimerMode::Relative,
            ClockBase::Monotonic,
        )
    }
}

// ─── Global State ───────────────────────────────────────────────────────────

pub static TIMER_SUBSYSTEM: Mutex<Option<TimerSubsystem>> = Mutex::new(None);

pub fn init() {
    *TIMER_SUBSYSTEM.lock() = Some(TimerSubsystem::new());
}
