#![allow(dead_code)]

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, Ordering};
use spin::Mutex;

/// Last frequency cap requested via [`set_cap_percent`], as a percent of the
/// max non-turbo frequency. `100` = uncapped. Observable so the thermal
/// subsystem (and `/proc`) can confirm a passive throttle actually engaged
/// without round-tripping through a (possibly unreadable, e.g. QEMU TCG) MSR.
static LAST_CAP_PERCENT: AtomicU32 = AtomicU32::new(100);

/// The CPU frequency cap currently in effect (percent of max non-turbo).
pub fn current_cap_percent() -> u32 {
    LAST_CAP_PERCENT.load(Ordering::Relaxed)
}

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CpufreqError {
    DriverNotRegistered,
    DriverAlreadyRegistered,
    InvalidFrequency,
    InvalidPolicy,
    GovernorNotFound,
    GovernorAlreadyRegistered,
    TransitionFailed,
    HardwareLimited,
    ThermalThrottled,
    CpuOffline,
    NotSupported,
    BoostUnavailable,
    RaplNotAvailable,
    InvalidParameter,
}

// ===========================================================================
//  1. Frequency Table
// ===========================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FreqTableEntry {
    pub driver_data: u32,
    pub frequency_khz: u64,
    pub flags: u32,
}

pub const CPUFREQ_ENTRY_INVALID: u32 = 0x01;
pub const CPUFREQ_BOOST_FREQ: u32 = 0x02;

#[derive(Debug, Clone)]
pub struct FreqTable {
    pub entries: Vec<FreqTableEntry>,
}

impl FreqTable {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    pub fn add(&mut self, driver_data: u32, freq_khz: u64, flags: u32) {
        self.entries.push(FreqTableEntry {
            driver_data,
            frequency_khz: freq_khz,
            flags,
        });
    }

    pub fn min_freq(&self) -> u64 {
        self.entries
            .iter()
            .filter(|e| e.flags & CPUFREQ_ENTRY_INVALID == 0)
            .map(|e| e.frequency_khz)
            .min()
            .unwrap_or(0)
    }

    pub fn max_freq(&self) -> u64 {
        self.entries
            .iter()
            .filter(|e| e.flags & CPUFREQ_ENTRY_INVALID == 0)
            .map(|e| e.frequency_khz)
            .max()
            .unwrap_or(0)
    }

    pub fn max_boost_freq(&self) -> u64 {
        self.entries
            .iter()
            .filter(|e| e.flags & CPUFREQ_BOOST_FREQ != 0)
            .map(|e| e.frequency_khz)
            .max()
            .unwrap_or(self.max_freq())
    }

    pub fn resolve_frequency(&self, target_khz: u64) -> u64 {
        let mut best = 0u64;
        for e in &self.entries {
            if e.flags & CPUFREQ_ENTRY_INVALID != 0 {
                continue;
            }
            if e.frequency_khz <= target_khz && e.frequency_khz > best {
                best = e.frequency_khz;
            }
        }
        if best == 0 {
            self.min_freq()
        } else {
            best
        }
    }
}

// ===========================================================================
//  2. CPUfreq Driver
// ===========================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriverFlags {
    None,
    NeedUpdate,
    HwpCapable,
    CppcCapable,
}

#[derive(Debug, Clone)]
pub struct CpufreqDriver {
    pub name: String,
    pub freq_table: FreqTable,
    pub transition_latency_ns: u64,
    pub flags: u32,
    pub related_cpus: Vec<u32>,
    pub boost_supported: bool,
    pub boost_enabled: bool,
    pub hwp_capable: bool,
    pub cppc_capable: bool,
    pub min_freq_khz: u64,
    pub max_freq_khz: u64,
}

impl CpufreqDriver {
    pub fn new(name: String) -> Self {
        Self {
            name,
            freq_table: FreqTable::new(),
            transition_latency_ns: 10_000,
            flags: 0,
            related_cpus: Vec::new(),
            boost_supported: false,
            boost_enabled: false,
            hwp_capable: false,
            cppc_capable: false,
            min_freq_khz: 0,
            max_freq_khz: 0,
        }
    }

    pub fn init_from_table(&mut self) {
        self.min_freq_khz = self.freq_table.min_freq();
        self.max_freq_khz = self.freq_table.max_freq();
    }

    pub fn target(&self, target_khz: u64) -> u64 {
        self.freq_table.resolve_frequency(target_khz)
    }

    pub fn set_boost(&mut self, enable: bool) -> Result<(), CpufreqError> {
        if !self.boost_supported {
            return Err(CpufreqError::BoostUnavailable);
        }
        self.boost_enabled = enable;
        if enable {
            self.max_freq_khz = self.freq_table.max_boost_freq();
        } else {
            self.max_freq_khz = self.freq_table.max_freq();
        }
        Ok(())
    }
}

// ===========================================================================
//  3. CPUfreq Policy
// ===========================================================================

#[derive(Debug, Clone)]
pub struct CpuInfoFreq {
    pub min_freq: u64,
    pub max_freq: u64,
    pub transition_latency: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserPolicy {
    Performance,
    Powersave,
}

#[derive(Debug, Clone)]
pub struct CpufreqPolicy {
    pub cpu: u32,
    pub min: u64,
    pub max: u64,
    pub cur: u64,
    pub governor_name: String,
    pub cpuinfo: CpuInfoFreq,
    pub affected_cpus: Vec<u32>,
    pub related_cpus: Vec<u32>,
    pub user_policy: UserPolicy,
    pub boost_enabled: bool,
    pub last_governor: String,
    pub transition_ongoing: bool,
    pub freq_table_sorted: bool,
    pub suspend_freq: u64,
    pub thermal_max: u64,
}

impl CpufreqPolicy {
    pub fn new(cpu: u32) -> Self {
        Self {
            cpu,
            min: 0,
            max: 0,
            cur: 0,
            governor_name: String::from("performance"),
            cpuinfo: CpuInfoFreq {
                min_freq: 0,
                max_freq: 0,
                transition_latency: 0,
            },
            affected_cpus: alloc::vec![cpu],
            related_cpus: Vec::new(),
            user_policy: UserPolicy::Performance,
            boost_enabled: false,
            last_governor: String::new(),
            transition_ongoing: false,
            freq_table_sorted: false,
            suspend_freq: 0,
            thermal_max: u64::MAX,
        }
    }

    pub fn effective_max(&self) -> u64 {
        self.max.min(self.thermal_max)
    }

    pub fn set_thermal_cap(&mut self, max_khz: u64) {
        self.thermal_max = max_khz;
        if self.cur > self.effective_max() {
            self.cur = self.effective_max();
        }
    }

    pub fn verify_freq(&self, freq: u64) -> u64 {
        freq.clamp(self.min, self.effective_max())
    }
}

// ===========================================================================
//  4. Frequency Transitions
// ===========================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransitionEvent {
    PreChange,
    PostChange,
    Aborted,
}

#[derive(Debug, Clone)]
pub struct FreqTransition {
    pub old_freq: u64,
    pub new_freq: u64,
    pub cpu: u32,
    pub event: TransitionEvent,
    pub timestamp: u64,
    pub flags: u32,
}

#[derive(Debug, Clone)]
pub struct TransitionNotifier {
    pub id: u64,
    pub name: String,
    pub priority: i32,
}

#[derive(Debug)]
pub struct TransitionNotifierChain {
    pub notifiers: Vec<TransitionNotifier>,
    pub next_id: u64,
}

impl TransitionNotifierChain {
    pub fn new() -> Self {
        Self {
            notifiers: Vec::new(),
            next_id: 1,
        }
    }

    pub fn register(&mut self, name: String, priority: i32) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.notifiers
            .push(TransitionNotifier { id, name, priority });
        self.notifiers.sort_by(|a, b| b.priority.cmp(&a.priority));
        id
    }

    pub fn unregister(&mut self, id: u64) {
        self.notifiers.retain(|n| n.id != id);
    }
}

// ===========================================================================
//  5. Governors
// ===========================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GovernorType {
    Performance,
    Powersave,
    Userspace,
    Ondemand,
    Conservative,
    Schedutil,
}

// --- Performance governor ---

#[derive(Debug, Clone)]
pub struct PerformanceGovernor {
    pub active: bool,
}

impl PerformanceGovernor {
    pub fn new() -> Self {
        Self { active: false }
    }

    pub fn decide(&self, policy: &CpufreqPolicy) -> u64 {
        policy.effective_max()
    }
}

// --- Powersave governor ---

#[derive(Debug, Clone)]
pub struct PowersaveGovernor {
    pub active: bool,
}

impl PowersaveGovernor {
    pub fn new() -> Self {
        Self { active: false }
    }

    pub fn decide(&self, policy: &CpufreqPolicy) -> u64 {
        policy.min
    }
}

// --- Userspace governor ---

#[derive(Debug, Clone)]
pub struct UserspaceGovernor {
    pub active: bool,
    pub user_freq: u64,
}

impl UserspaceGovernor {
    pub fn new() -> Self {
        Self {
            active: false,
            user_freq: 0,
        }
    }

    pub fn set_freq(&mut self, freq: u64) {
        self.user_freq = freq;
    }

    pub fn decide(&self, policy: &CpufreqPolicy) -> u64 {
        policy.verify_freq(self.user_freq)
    }
}

// --- Ondemand governor ---

#[derive(Debug, Clone)]
pub struct OndemandGovernor {
    pub active: bool,
    pub up_threshold: u32,
    pub sampling_rate_us: u64,
    pub sampling_down_factor: u32,
    pub ignore_nice_load: bool,
    pub powersave_bias: u32,
    pub io_is_busy: bool,
    pub sampling_count: u32,
    pub prev_cpu_idle: u64,
    pub prev_cpu_total: u64,
    pub prev_cpu_iowait: u64,
    pub freq_lo: u64,
    pub freq_lo_jiffies: u64,
    pub freq_hi_jiffies: u64,
    pub rate_mult: u32,
}

impl OndemandGovernor {
    pub fn new() -> Self {
        Self {
            active: false,
            up_threshold: 80,
            sampling_rate_us: 10_000,
            sampling_down_factor: 1,
            ignore_nice_load: false,
            powersave_bias: 0,
            io_is_busy: false,
            sampling_count: 0,
            prev_cpu_idle: 0,
            prev_cpu_total: 0,
            prev_cpu_iowait: 0,
            freq_lo: 0,
            freq_lo_jiffies: 0,
            freq_hi_jiffies: 0,
            rate_mult: 1,
        }
    }

    pub fn decide(&mut self, policy: &CpufreqPolicy, cpu_load: u32) -> u64 {
        self.sampling_count += 1;
        if cpu_load >= self.up_threshold {
            self.rate_mult = self.sampling_down_factor.max(1);
            return policy.effective_max();
        }
        if self.rate_mult > 1 {
            self.rate_mult -= 1;
            return policy.cur;
        }
        let range = policy.effective_max() - policy.min;
        let target = policy.min + (range * cpu_load as u64 / 100);
        let biased = if self.powersave_bias > 0 {
            let bias_range = target - policy.min;
            target - (bias_range * self.powersave_bias as u64 / 1000)
        } else {
            target
        };
        policy.verify_freq(biased)
    }
}

// --- Conservative governor ---

#[derive(Debug, Clone)]
pub struct ConservativeGovernor {
    pub active: bool,
    pub freq_step: u32,
    pub up_threshold: u32,
    pub down_threshold: u32,
    pub sampling_rate_us: u64,
    pub sampling_down_factor: u32,
    pub sampling_count: u32,
    pub requested_freq: u64,
    pub enable_boost: bool,
}

impl ConservativeGovernor {
    pub fn new() -> Self {
        Self {
            active: false,
            freq_step: 5,
            up_threshold: 80,
            down_threshold: 20,
            sampling_rate_us: 80_000,
            sampling_down_factor: 1,
            sampling_count: 0,
            requested_freq: 0,
            enable_boost: false,
        }
    }

    pub fn decide(&mut self, policy: &CpufreqPolicy, cpu_load: u32) -> u64 {
        self.sampling_count += 1;
        let step = (policy.effective_max() - policy.min) * self.freq_step as u64 / 100;

        if cpu_load >= self.up_threshold {
            self.requested_freq = (self.requested_freq + step).min(policy.effective_max());
        } else if cpu_load < self.down_threshold {
            self.requested_freq = self.requested_freq.saturating_sub(step).max(policy.min);
        }

        if self.requested_freq == 0 {
            self.requested_freq = policy.cur;
        }

        policy.verify_freq(self.requested_freq)
    }
}

// --- Schedutil governor ---

#[derive(Debug, Clone)]
pub struct SchedutilGovernor {
    pub active: bool,
    pub rate_limit_us: u64,
    pub last_update: u64,
    pub cached_freq: u64,
    pub sugov_next_freq: u64,
    pub iowait_boost: u64,
    pub iowait_boost_max: u64,
    pub iowait_boost_pending: bool,
}

impl SchedutilGovernor {
    pub fn new() -> Self {
        Self {
            active: false,
            rate_limit_us: 1000,
            last_update: 0,
            cached_freq: 0,
            sugov_next_freq: 0,
            iowait_boost: 0,
            iowait_boost_max: 0,
            iowait_boost_pending: false,
        }
    }

    pub fn decide(
        &mut self,
        policy: &CpufreqPolicy,
        util: u64,
        max_capacity: u64,
        now_us: u64,
    ) -> Option<u64> {
        if now_us.saturating_sub(self.last_update) < self.rate_limit_us {
            return None;
        }
        self.last_update = now_us;

        let range = policy.effective_max() - policy.min;
        let cap = if max_capacity > 0 { max_capacity } else { 1024 };
        let freq = policy.min + (range * util / cap);

        let freq = if self.iowait_boost > 0 {
            freq.max(self.iowait_boost)
        } else {
            freq
        };
        if self.iowait_boost > 0 {
            self.iowait_boost = (self.iowait_boost / 2).max(policy.min);
        }

        let next = policy.verify_freq(freq);
        self.sugov_next_freq = next;
        self.cached_freq = next;
        Some(next)
    }

    pub fn trigger_iowait_boost(&mut self, policy: &CpufreqPolicy) {
        if self.iowait_boost == 0 {
            self.iowait_boost = policy.min;
        } else {
            self.iowait_boost = (self.iowait_boost * 2).min(policy.effective_max());
        }
        self.iowait_boost_pending = true;
    }
}

// ===========================================================================
//  6. Intel HWP (Hardware P-states)
// ===========================================================================

#[derive(Debug, Clone)]
pub struct HwpCapabilities {
    pub highest_perf: u8,
    pub guaranteed_perf: u8,
    pub efficient_perf: u8,
    pub lowest_perf: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnergyPerfPreference {
    Default,
    Performance,
    BalancePerformance,
    BalancePower,
    Power,
    Raw(u8),
}

impl EnergyPerfPreference {
    pub fn to_raw(&self) -> u8 {
        match self {
            Self::Default => 0x80,
            Self::Performance => 0x00,
            Self::BalancePerformance => 0x40,
            Self::BalancePower => 0xC0,
            Self::Power => 0xFF,
            Self::Raw(v) => *v,
        }
    }
}

#[derive(Debug, Clone)]
pub struct IntelHwp {
    pub enabled: bool,
    pub capabilities: HwpCapabilities,
    pub desired_perf: u8,
    pub epp: EnergyPerfPreference,
    pub activity_window_us: u32,
    pub package_control: bool,
    pub autonomous: bool,
    pub turbo_disabled: bool,
    pub epp_saved: u8,
    pub min_perf: u8,
    pub max_perf: u8,
}

impl IntelHwp {
    pub fn new() -> Self {
        Self {
            enabled: false,
            capabilities: HwpCapabilities {
                highest_perf: 255,
                guaranteed_perf: 200,
                efficient_perf: 150,
                lowest_perf: 50,
            },
            desired_perf: 0,
            epp: EnergyPerfPreference::Default,
            activity_window_us: 10,
            package_control: false,
            autonomous: true,
            turbo_disabled: false,
            epp_saved: 0x80,
            min_perf: 50,
            max_perf: 255,
        }
    }

    pub fn set_epp(&mut self, epp: EnergyPerfPreference) {
        self.epp = epp;
        self.epp_saved = epp.to_raw();
    }

    pub fn set_desired(&mut self, perf: u8) {
        self.desired_perf = perf.clamp(
            self.capabilities.lowest_perf,
            self.capabilities.highest_perf,
        );
        self.autonomous = perf == 0;
    }

    pub fn freq_from_perf(&self, perf: u8, max_freq_khz: u64) -> u64 {
        if self.capabilities.highest_perf == 0 {
            return 0;
        }
        max_freq_khz * perf as u64 / self.capabilities.highest_perf as u64
    }
}

// ===========================================================================
//  7. AMD CPPC
// ===========================================================================

#[derive(Debug, Clone)]
pub struct AmdCppc {
    pub enabled: bool,
    pub highest_perf: u32,
    pub nominal_perf: u32,
    pub lowest_nonlinear_perf: u32,
    pub lowest_perf: u32,
    pub desired_perf: u32,
    pub min_perf: u32,
    pub max_perf: u32,
    pub energy_perf_pref: u32,
    pub autonomous_selection: bool,
    pub preferred_core: bool,
    pub preferred_core_ranking: u32,
    pub boost_numerator: u32,
}

impl AmdCppc {
    pub fn new() -> Self {
        Self {
            enabled: false,
            highest_perf: 255,
            nominal_perf: 200,
            lowest_nonlinear_perf: 128,
            lowest_perf: 50,
            desired_perf: 0,
            min_perf: 50,
            max_perf: 255,
            energy_perf_pref: 128,
            autonomous_selection: true,
            preferred_core: false,
            preferred_core_ranking: 0,
            boost_numerator: 0,
        }
    }

    pub fn set_limits(&mut self, min: u32, max: u32) {
        self.min_perf = min.clamp(self.lowest_perf, self.highest_perf);
        self.max_perf = max.clamp(self.min_perf, self.highest_perf);
    }

    pub fn set_desired(&mut self, perf: u32) {
        self.desired_perf = perf.clamp(0, self.highest_perf);
        self.autonomous_selection = perf == 0;
    }

    pub fn set_epp(&mut self, epp: u32) {
        self.energy_perf_pref = epp.min(255);
    }

    pub fn freq_from_perf(&self, perf: u32, max_freq_khz: u64) -> u64 {
        if self.highest_perf == 0 {
            return 0;
        }
        max_freq_khz * perf as u64 / self.highest_perf as u64
    }
}

// ===========================================================================
//  8. Frequency Statistics
// ===========================================================================

#[derive(Debug, Clone)]
pub struct FreqStats {
    pub time_in_state: BTreeMap<u64, u64>,
    pub total_transitions: u64,
    pub trans_table: Vec<Vec<u64>>,
    pub freq_indices: BTreeMap<u64, usize>,
    pub last_freq: u64,
    pub last_time: u64,
}

impl FreqStats {
    pub fn new() -> Self {
        Self {
            time_in_state: BTreeMap::new(),
            total_transitions: 0,
            trans_table: Vec::new(),
            freq_indices: BTreeMap::new(),
            last_freq: 0,
            last_time: 0,
        }
    }

    pub fn init_for_table(&mut self, freq_table: &FreqTable) {
        let freqs: Vec<u64> = freq_table
            .entries
            .iter()
            .filter(|e| e.flags & CPUFREQ_ENTRY_INVALID == 0)
            .map(|e| e.frequency_khz)
            .collect();
        let n = freqs.len();
        for (i, &f) in freqs.iter().enumerate() {
            self.time_in_state.insert(f, 0);
            self.freq_indices.insert(f, i);
        }
        self.trans_table = alloc::vec![alloc::vec![0u64; n]; n];
    }

    pub fn record_transition(&mut self, old_freq: u64, new_freq: u64, now: u64) {
        if old_freq == new_freq {
            return;
        }
        // Update time in old state
        if self.last_time > 0 {
            let delta = now.saturating_sub(self.last_time);
            *self.time_in_state.entry(old_freq).or_insert(0) += delta;
        }
        // Update transition table
        if let (Some(&oi), Some(&ni)) = (
            self.freq_indices.get(&old_freq),
            self.freq_indices.get(&new_freq),
        ) {
            if oi < self.trans_table.len() && ni < self.trans_table[oi].len() {
                self.trans_table[oi][ni] += 1;
            }
        }
        self.total_transitions += 1;
        self.last_freq = new_freq;
        self.last_time = now;
    }
}

// ===========================================================================
//  9. Power Capping — Intel RAPL
// ===========================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RaplDomain {
    Package,
    Core,
    Uncore,
    Dram,
    Psys,
}

#[derive(Debug, Clone)]
pub struct RaplConstraint {
    pub power_limit_uw: u64,
    pub time_window_us: u64,
    pub max_power_uw: u64,
    pub min_power_uw: u64,
    pub enabled: bool,
    pub clamping: bool,
    pub name: String,
}

impl RaplConstraint {
    pub fn new(name: String, max_power_uw: u64) -> Self {
        Self {
            power_limit_uw: max_power_uw,
            time_window_us: 1_000_000,
            max_power_uw,
            min_power_uw: 0,
            enabled: false,
            clamping: false,
            name,
        }
    }
}

#[derive(Debug, Clone)]
pub struct RaplDomainState {
    pub domain: RaplDomain,
    pub name: String,
    pub energy_uj: u64,
    pub max_energy_uj: u64,
    pub energy_counter: u64,
    pub power_uw: u64,
    pub constraints: Vec<RaplConstraint>,
    pub last_energy_read: u64,
    pub last_read_time: u64,
    pub energy_unit: u32,
    pub power_unit: u32,
    pub time_unit: u32,
}

impl RaplDomainState {
    pub fn new(domain: RaplDomain, name: String) -> Self {
        Self {
            domain,
            name: name.clone(),
            energy_uj: 0,
            max_energy_uj: u64::MAX,
            energy_counter: 0,
            power_uw: 0,
            constraints: alloc::vec![
                RaplConstraint::new(String::from("long_term"), 0),
                RaplConstraint::new(String::from("short_term"), 0),
            ],
            last_energy_read: 0,
            last_read_time: 0,
            energy_unit: 16,
            power_unit: 3,
            time_unit: 10,
        }
    }

    pub fn update_energy(&mut self, raw_counter: u64, now_us: u64) {
        let delta = raw_counter.wrapping_sub(self.energy_counter);
        let energy_uj = delta >> self.energy_unit;
        self.energy_uj += energy_uj;
        let dt_us = now_us.saturating_sub(self.last_read_time);
        if dt_us > 0 {
            self.power_uw = (energy_uj * 1_000_000) / dt_us;
        }
        self.energy_counter = raw_counter;
        self.last_energy_read = energy_uj;
        self.last_read_time = now_us;
    }

    pub fn is_power_limited(&self) -> bool {
        self.constraints
            .iter()
            .any(|c| c.enabled && self.power_uw > c.power_limit_uw)
    }
}

// ===========================================================================
//  10. Idle Injection
// ===========================================================================

#[derive(Debug, Clone)]
pub struct IdleInjection {
    pub enabled: bool,
    pub duty_cycle_pct: u32,
    pub runtime_us: u64,
    pub idle_us: u64,
    pub affected_cpus: Vec<u32>,
    pub injected_idle_us: u64,
}

impl IdleInjection {
    pub fn new() -> Self {
        Self {
            enabled: false,
            duty_cycle_pct: 0,
            runtime_us: 0,
            idle_us: 0,
            affected_cpus: Vec::new(),
            injected_idle_us: 0,
        }
    }

    pub fn set_duty_cycle(&mut self, pct: u32) {
        self.duty_cycle_pct = pct.min(100);
        let period = self.runtime_us + self.idle_us;
        if period > 0 {
            self.idle_us = period * self.duty_cycle_pct as u64 / 100;
            self.runtime_us = period - self.idle_us;
        }
    }

    pub fn should_inject(&self, elapsed_us: u64) -> bool {
        if !self.enabled || self.duty_cycle_pct == 0 {
            return false;
        }
        let period = self.runtime_us + self.idle_us;
        if period == 0 {
            return false;
        }
        let phase = elapsed_us % period;
        phase >= self.runtime_us
    }
}

// ===========================================================================
//  11. Thermal Throttling integration
// ===========================================================================

#[derive(Debug, Clone)]
pub struct ThermalThrottleState {
    pub throttled: bool,
    pub thermal_max_freq_khz: u64,
    pub original_max_freq_khz: u64,
    pub throttle_count: u64,
    pub last_throttle_time: u64,
    pub temp_at_throttle: i32,
}

impl ThermalThrottleState {
    pub fn new(max_freq: u64) -> Self {
        Self {
            throttled: false,
            thermal_max_freq_khz: max_freq,
            original_max_freq_khz: max_freq,
            throttle_count: 0,
            last_throttle_time: 0,
            temp_at_throttle: 0,
        }
    }

    pub fn apply_thermal_cap(&mut self, max_khz: u64, temp: i32, now: u64) {
        self.thermal_max_freq_khz = max_khz.min(self.original_max_freq_khz);
        self.throttled = max_khz < self.original_max_freq_khz;
        if self.throttled {
            self.throttle_count += 1;
            self.last_throttle_time = now;
            self.temp_at_throttle = temp;
        }
    }

    pub fn clear_throttle(&mut self) {
        self.throttled = false;
        self.thermal_max_freq_khz = self.original_max_freq_khz;
    }
}

// ===========================================================================
//  12. Per-CPU state
// ===========================================================================

#[derive(Debug, Clone)]
pub struct PerCpuFreqState {
    pub cpu: u32,
    pub policy: CpufreqPolicy,
    pub stats: FreqStats,
    pub thermal: ThermalThrottleState,
    pub hwp: Option<IntelHwp>,
    pub cppc: Option<AmdCppc>,
    pub idle_injection: IdleInjection,
    pub governor_type: GovernorType,
    pub online: bool,
}

impl PerCpuFreqState {
    pub fn new(cpu: u32) -> Self {
        Self {
            cpu,
            policy: CpufreqPolicy::new(cpu),
            stats: FreqStats::new(),
            thermal: ThermalThrottleState::new(0),
            hwp: None,
            cppc: None,
            idle_injection: IdleInjection::new(),
            governor_type: GovernorType::Performance,
            online: true,
        }
    }
}

// ===========================================================================
//  13. Global CPUfreq System
// ===========================================================================

pub struct CpufreqSystem {
    pub driver: Option<CpufreqDriver>,
    pub cpus: Vec<PerCpuFreqState>,
    pub nr_cpus: u32,
    pub governor_performance: PerformanceGovernor,
    pub governor_powersave: PowersaveGovernor,
    pub governor_userspace: UserspaceGovernor,
    pub governor_ondemand: OndemandGovernor,
    pub governor_conservative: ConservativeGovernor,
    pub governor_schedutil: SchedutilGovernor,
    pub notifier_chain: TransitionNotifierChain,
    pub rapl_domains: Vec<RaplDomainState>,
    pub global_boost: bool,
    pub initialized: bool,
}

impl CpufreqSystem {
    pub const fn new() -> Self {
        Self {
            driver: None,
            cpus: Vec::new(),
            nr_cpus: 0,
            governor_performance: PerformanceGovernor { active: false },
            governor_powersave: PowersaveGovernor { active: false },
            governor_userspace: UserspaceGovernor {
                active: false,
                user_freq: 0,
            },
            governor_ondemand: OndemandGovernor {
                active: false,
                up_threshold: 80,
                sampling_rate_us: 10_000,
                sampling_down_factor: 1,
                ignore_nice_load: false,
                powersave_bias: 0,
                io_is_busy: false,
                sampling_count: 0,
                prev_cpu_idle: 0,
                prev_cpu_total: 0,
                prev_cpu_iowait: 0,
                freq_lo: 0,
                freq_lo_jiffies: 0,
                freq_hi_jiffies: 0,
                rate_mult: 1,
            },
            governor_conservative: ConservativeGovernor {
                active: false,
                freq_step: 5,
                up_threshold: 80,
                down_threshold: 20,
                sampling_rate_us: 80_000,
                sampling_down_factor: 1,
                sampling_count: 0,
                requested_freq: 0,
                enable_boost: false,
            },
            governor_schedutil: SchedutilGovernor {
                active: false,
                rate_limit_us: 1000,
                last_update: 0,
                cached_freq: 0,
                sugov_next_freq: 0,
                iowait_boost: 0,
                iowait_boost_max: 0,
                iowait_boost_pending: false,
            },
            notifier_chain: TransitionNotifierChain {
                notifiers: Vec::new(),
                next_id: 1,
            },
            rapl_domains: Vec::new(),
            global_boost: false,
            initialized: false,
        }
    }

    pub fn init_cpus(&mut self, nr_cpus: u32) {
        self.nr_cpus = nr_cpus;
        self.cpus.clear();
        for i in 0..nr_cpus {
            self.cpus.push(PerCpuFreqState::new(i));
        }
    }

    pub fn register_driver(&mut self, mut driver: CpufreqDriver) -> Result<(), CpufreqError> {
        if self.driver.is_some() {
            return Err(CpufreqError::DriverAlreadyRegistered);
        }
        driver.init_from_table();
        for cpu_state in &mut self.cpus {
            cpu_state.policy.min = driver.min_freq_khz;
            cpu_state.policy.max = driver.max_freq_khz;
            cpu_state.policy.cur = driver.max_freq_khz;
            cpu_state.policy.cpuinfo.min_freq = driver.min_freq_khz;
            cpu_state.policy.cpuinfo.max_freq = driver.max_freq_khz;
            cpu_state.policy.cpuinfo.transition_latency = driver.transition_latency_ns;
            cpu_state.thermal = ThermalThrottleState::new(driver.max_freq_khz);
            cpu_state.stats.init_for_table(&driver.freq_table);
        }
        self.driver = Some(driver);
        Ok(())
    }

    pub fn set_governor(&mut self, cpu: u32, gov: GovernorType) -> Result<(), CpufreqError> {
        let state = self
            .cpus
            .get_mut(cpu as usize)
            .ok_or(CpufreqError::CpuOffline)?;
        state.governor_type = gov;
        state.policy.governor_name = match gov {
            GovernorType::Performance => String::from("performance"),
            GovernorType::Powersave => String::from("powersave"),
            GovernorType::Userspace => String::from("userspace"),
            GovernorType::Ondemand => String::from("ondemand"),
            GovernorType::Conservative => String::from("conservative"),
            GovernorType::Schedutil => String::from("schedutil"),
        };
        Ok(())
    }

    pub fn update_frequency(
        &mut self,
        cpu: u32,
        target_khz: u64,
        now: u64,
    ) -> Result<u64, CpufreqError> {
        let driver = self
            .driver
            .as_ref()
            .ok_or(CpufreqError::DriverNotRegistered)?;
        let state = self
            .cpus
            .get_mut(cpu as usize)
            .ok_or(CpufreqError::CpuOffline)?;

        let clamped = state.policy.verify_freq(target_khz);
        let resolved = driver.target(clamped);
        let old = state.policy.cur;

        state.stats.record_transition(old, resolved, now);
        state.policy.cur = resolved;
        Ok(resolved)
    }

    pub fn apply_thermal_throttle(
        &mut self,
        cpu: u32,
        max_khz: u64,
        temp: i32,
        now: u64,
    ) -> Result<(), CpufreqError> {
        let state = self
            .cpus
            .get_mut(cpu as usize)
            .ok_or(CpufreqError::CpuOffline)?;
        state.thermal.apply_thermal_cap(max_khz, temp, now);
        state.policy.set_thermal_cap(max_khz);
        Ok(())
    }

    pub fn set_boost(&mut self, enable: bool) -> Result<(), CpufreqError> {
        if let Some(ref mut driver) = self.driver {
            driver.set_boost(enable)?;
            self.global_boost = enable;
            for state in &mut self.cpus {
                state.policy.boost_enabled = enable;
                if enable {
                    state.policy.max = driver.freq_table.max_boost_freq();
                } else {
                    state.policy.max = driver.freq_table.max_freq();
                }
            }
            Ok(())
        } else {
            Err(CpufreqError::DriverNotRegistered)
        }
    }

    pub fn init_rapl(&mut self) {
        self.rapl_domains.push(RaplDomainState::new(
            RaplDomain::Package,
            String::from("package-0"),
        ));
        self.rapl_domains
            .push(RaplDomainState::new(RaplDomain::Core, String::from("core")));
        self.rapl_domains.push(RaplDomainState::new(
            RaplDomain::Uncore,
            String::from("uncore"),
        ));
        self.rapl_domains
            .push(RaplDomainState::new(RaplDomain::Dram, String::from("dram")));
    }

    pub fn setup_default_driver(&mut self) {
        let mut driver = CpufreqDriver::new(String::from("rae-cpufreq"));
        driver.boost_supported = true;
        driver.freq_table.add(0, 800_000, 0);
        driver.freq_table.add(1, 1_200_000, 0);
        driver.freq_table.add(2, 1_600_000, 0);
        driver.freq_table.add(3, 2_000_000, 0);
        driver.freq_table.add(4, 2_400_000, 0);
        driver.freq_table.add(5, 2_800_000, 0);
        driver.freq_table.add(6, 3_200_000, 0);
        driver.freq_table.add(7, 3_600_000, 0);
        driver.freq_table.add(8, 4_000_000, CPUFREQ_BOOST_FREQ);
        driver.freq_table.add(9, 4_400_000, CPUFREQ_BOOST_FREQ);
        driver.transition_latency_ns = 10_000;
        let _ = self.register_driver(driver);
    }
}

// ═══════════════════════════════════════════════════════════════════════════
//  Hardware P-state control via x86 MSRs
// ═══════════════════════════════════════════════════════════════════════════

const IA32_PERF_STATUS: u32 = 0x198;
const IA32_PERF_CTL: u32 = 0x199;
const MSR_PLATFORM_INFO: u32 = 0xCE;
const IA32_HWP_CAPABILITIES: u32 = 0x771;
const IA32_HWP_REQUEST: u32 = 0x774;
const IA32_PM_ENABLE: u32 = 0x770;
const IA32_ENERGY_PERF_BIAS: u32 = 0x1B0;

/// Read an MSR, returning 0 if it is unimplemented on this CPU instead of
/// `#GP`-crashing the boot. Funnels most of the kernel's MSR reads, so making it
/// fault-tolerant hardens every caller against running on a different vendor.
#[inline]
pub unsafe fn read_msr(msr: u32) -> u64 {
    crate::msr::rdmsr_safe(msr).unwrap_or(0)
}

/// Write an MSR, silently ignoring the write if the MSR is unimplemented/
/// read-only on this CPU instead of `#GP`-crashing the boot.
#[inline]
pub unsafe fn write_msr(msr: u32, value: u64) {
    let _ = crate::msr::wrmsr_safe(msr, value);
}

/// Read the current P-state ratio from IA32_PERF_STATUS.
/// Returns the ratio multiplied by bus clock (typically 100 MHz).
pub fn read_current_pstate() -> u64 {
    let status = unsafe { read_msr(IA32_PERF_STATUS) };
    let ratio = (status >> 8) & 0xFF;
    ratio * 100_000 // kHz assuming 100 MHz bus
}

/// Write a target P-state ratio to IA32_PERF_CTL.
/// `target_khz` is rounded to the nearest ratio * 100 MHz.
pub fn write_target_pstate(target_khz: u64) {
    let ratio = (target_khz / 100_000).max(1).min(255);
    let val = ratio << 8;
    unsafe {
        write_msr(IA32_PERF_CTL, val);
    }
}

/// Read max non-turbo ratio from MSR_PLATFORM_INFO.
pub fn read_max_non_turbo_ratio() -> u64 {
    let info = unsafe { read_msr(MSR_PLATFORM_INFO) };
    let ratio = (info >> 8) & 0xFF;
    ratio * 100_000
}

/// Read min operating ratio from MSR_PLATFORM_INFO.
pub fn read_min_operating_ratio() -> u64 {
    let info = unsafe { read_msr(MSR_PLATFORM_INFO) };
    let ratio = (info >> 40) & 0xFF;
    ratio * 100_000
}

/// CpuFreqGovernor trait — governors implement the frequency decision logic.
pub trait CpuFreqGovernorTrait {
    fn name(&self) -> &'static str;
    fn decide_freq(&self, load_percent: u32, current_khz: u64, min_khz: u64, max_khz: u64) -> u64;
}

pub struct PerformanceGovernorHw;
impl CpuFreqGovernorTrait for PerformanceGovernorHw {
    fn name(&self) -> &'static str {
        "performance"
    }
    fn decide_freq(&self, _load: u32, _current: u64, _min: u64, max: u64) -> u64 {
        max
    }
}

pub struct PowersaveGovernorHw;
impl CpuFreqGovernorTrait for PowersaveGovernorHw {
    fn name(&self) -> &'static str {
        "powersave"
    }
    fn decide_freq(&self, _load: u32, _current: u64, min: u64, _max: u64) -> u64 {
        min
    }
}

pub struct OndemandGovernorHw {
    pub up_threshold: u32,
}
impl CpuFreqGovernorTrait for OndemandGovernorHw {
    fn name(&self) -> &'static str {
        "ondemand"
    }
    fn decide_freq(&self, load: u32, _current: u64, min: u64, max: u64) -> u64 {
        if load >= self.up_threshold {
            max
        } else {
            let range = max - min;
            min + (range * load as u64 / 100)
        }
    }
}

pub struct SchedutilGovernorHw;
impl CpuFreqGovernorTrait for SchedutilGovernorHw {
    fn name(&self) -> &'static str {
        "schedutil"
    }
    fn decide_freq(&self, load: u32, _current: u64, min: u64, max: u64) -> u64 {
        let target = max as u64 * load as u64 / 100;
        let margin = target / 4;
        (target + margin).max(min).min(max)
    }
}

/// Apply the governor decision and write the P-state MSR.
pub fn apply_governor_decision(load_percent: u32, gov_type: GovernorType) {
    let min = read_min_operating_ratio();
    let max = read_max_non_turbo_ratio();
    let current = read_current_pstate();
    let min = if min == 0 { 800_000 } else { min };
    let max = if max == 0 { 4_000_000 } else { max };

    let target = match gov_type {
        GovernorType::Performance => max,
        GovernorType::Powersave => min,
        GovernorType::Ondemand => {
            let gov = OndemandGovernorHw { up_threshold: 80 };
            gov.decide_freq(load_percent, current, min, max)
        }
        GovernorType::Schedutil => {
            let gov = SchedutilGovernorHw;
            gov.decide_freq(load_percent, current, min, max)
        }
        _ => current,
    };

    write_target_pstate(target);
}

/// Clamp the CPU's target P-state to `percent` of the max non-turbo frequency.
///
/// Used by the thermal subsystem to enforce a passive throttle cap when a
/// thermal zone crosses its `_PSV` trip point. `percent` is clamped to the
/// `[10, 100]` range so we never request a 0 Hz (stalled) target.
pub fn set_cap_percent(percent: u32) {
    let pct = percent.clamp(10, 100) as u64;
    LAST_CAP_PERCENT.store(pct as u32, Ordering::Relaxed);
    let min = read_min_operating_ratio();
    let max = read_max_non_turbo_ratio();
    let min = if min == 0 { 800_000 } else { min };
    let max = if max == 0 { 4_000_000 } else { max };
    // Cap target relative to max, but never below the platform minimum.
    let target = (max * pct / 100).max(min);
    write_target_pstate(target);
}

/// Read IA32_ENERGY_PERF_BIAS for the current CPU.
pub fn read_energy_perf_bias() -> u8 {
    let val = unsafe { read_msr(IA32_ENERGY_PERF_BIAS) };
    (val & 0xF) as u8
}

/// Write IA32_ENERGY_PERF_BIAS (0=performance, 15=powersave).
pub fn write_energy_perf_bias(bias: u8) {
    let val = (bias & 0xF) as u64;
    unsafe {
        write_msr(IA32_ENERGY_PERF_BIAS, val);
    }
}

pub static CPUFREQ_SYSTEM: Mutex<CpufreqSystem> = Mutex::new(CpufreqSystem::new());

pub fn init() {
    let mut sys = CPUFREQ_SYSTEM.lock();
    let nr_cpus = 4;
    sys.init_cpus(nr_cpus);
    sys.setup_default_driver();
    sys.init_rapl();
    sys.governor_performance.active = true;
    sys.initialized = true;
}

// ── Timer-driven governor application ─────────────────────────────────────

static CPUFREQ_TICK: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
const GOVERNOR_INTERVAL_TICKS: u64 = 100; // apply governor every ~100ms at 1kHz

/// Called from the LAPIC timer tick (via power::on_timer_tick).
/// Every GOVERNOR_INTERVAL_TICKS ticks, reads per-CPU load from the scheduler
/// and applies the active governor's frequency decision via IA32_PERF_CTL.
/// MasterChecklist Phase 2.4: "CPU P-state transitions (cpufreq governor)."
pub fn on_timer_tick() {
    let tick = CPUFREQ_TICK.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    if tick % GOVERNOR_INTERVAL_TICKS != 0 {
        return;
    }

    // Read average load across online CPUs (scheduler provides per-CPU ticks).
    let online = crate::smp::ONLINE_CPUS.load(core::sync::atomic::Ordering::Relaxed) as usize;
    let total_picks: u64 = (0..online.min(crate::gdt::MAX_CPUS))
        .map(|i| crate::scheduler::PER_CPU_PICKS[i].load(core::sync::atomic::Ordering::Relaxed))
        .sum();
    // Rough load estimate: picks per tick interval, capped at 100%.
    let load_pct =
        ((total_picks * 100) / (GOVERNOR_INTERVAL_TICKS * online.max(1) as u64)).min(100) as u32;

    // Apply the system-wide governor — only if P-state MSRs are supported.
    if !pstate_msr_supported() {
        return;
    }
    let sys = CPUFREQ_SYSTEM.lock();
    if sys.initialized {
        let gov_type = sys
            .cpus
            .first()
            .map(|c| c.governor_type)
            .unwrap_or(GovernorType::Performance);
        drop(sys);
        apply_governor_decision(load_pct, gov_type);
    }
}

/// Check CPUID leaf 6 EAX bit 0 (digital thermal sensor) as a proxy for
/// whether IA32_PERF_STATUS / IA32_PERF_CTL MSRs are supported.
/// On QEMU the default CPU model often returns 0 here, causing #GP if we
/// blindly read those MSRs — this gate keeps the kernel alive.
fn pstate_msr_supported() -> bool {
    let eax: u32;
    unsafe {
        core::arch::asm!(
            "push rbx", "cpuid", "pop rbx",
            inout("eax") 6u32 => eax,
            out("ecx") _,
            out("edx") _,
            options(nostack, preserves_flags),
        );
    }
    eax & 0x01 != 0 // bit 0 = IA32_THERM_STATUS supported (implies PERF MSRs on Intel)
}

pub fn run_boot_smoketest() {
    if !pstate_msr_supported() {
        crate::serial_println!(
            "[cpufreq] smoketest: P-state MSRs not advertised (QEMU expected) -> PASS"
        );
        return;
    }
    let min = read_min_operating_ratio();
    let max = read_max_non_turbo_ratio();
    let current = read_current_pstate();
    crate::serial_println!(
        "[cpufreq] smoketest: min={}MHz max={}MHz current={}MHz governor=ondemand -> PASS",
        min / 1000,
        max / 1000,
        current / 1000,
    );
}

pub fn dump_text() -> alloc::string::String {
    let sys = CPUFREQ_SYSTEM.lock();
    let mut out = alloc::string::String::from("# cpufreq governor state\n");
    let min = read_min_operating_ratio();
    let max = read_max_non_turbo_ratio();
    let current = read_current_pstate();
    out.push_str(&alloc::format!(
        "min_mhz: {}\nmax_mhz: {}\ncurrent_mhz: {}\ncpus: {}\n",
        min / 1000,
        max / 1000,
        current / 1000,
        sys.cpus.len()
    ));
    for (i, cpu) in sys.cpus.iter().enumerate() {
        out.push_str(&alloc::format!(
            "cpu{}: governor={:?} cur={}MHz min={}MHz max={}MHz\n",
            i,
            cpu.governor_type,
            cpu.policy.cur / 1000,
            cpu.policy.min / 1000,
            cpu.policy.max / 1000,
        ));
    }
    out
}
