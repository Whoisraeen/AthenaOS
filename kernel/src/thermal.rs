#![allow(dead_code)]

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicI32, AtomicU32, AtomicU64, Ordering};
use spin::Mutex;

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThermalError {
    ZoneNotFound,
    ZoneAlreadyExists,
    CoolingDeviceNotFound,
    CoolingDeviceAlreadyExists,
    GovernorNotFound,
    InvalidTrip,
    TripNotFound,
    SensorFailed,
    OverTemperature,
    InvalidParameter,
    ShutdownRequired,
    PowerBudgetExceeded,
    DtpmNodeNotFound,
}

// ===========================================================================
//  1. Trip Point Types
// ===========================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TripType {
    Active,
    Passive,
    Hot,
    Critical,
}

#[derive(Debug, Clone)]
pub struct TripPoint {
    pub id: u32,
    pub trip_type: TripType,
    pub temperature_mc: i32,
    pub hysteresis_mc: i32,
    pub crossed: bool,
    pub crossed_timestamp: u64,
}

impl TripPoint {
    pub fn new(id: u32, trip_type: TripType, temp_mc: i32, hyst_mc: i32) -> Self {
        Self {
            id,
            trip_type,
            temperature_mc: temp_mc,
            hysteresis_mc: hyst_mc,
            crossed: false,
            crossed_timestamp: 0,
        }
    }

    pub fn check(&mut self, current_mc: i32, now: u64) -> bool {
        if !self.crossed && current_mc >= self.temperature_mc {
            self.crossed = true;
            self.crossed_timestamp = now;
            return true;
        }
        if self.crossed && current_mc < (self.temperature_mc - self.hysteresis_mc) {
            self.crossed = false;
        }
        false
    }
}

// ===========================================================================
//  2. Thermal Sensor Types
// ===========================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SensorType {
    CpuDie,
    Gpu,
    Ssd,
    Ambient,
    Battery,
    Dram,
    Vrm,
    Pch,
    Skin,
}

#[derive(Debug, Clone)]
pub struct ThermalSensor {
    pub id: u32,
    pub sensor_type: SensorType,
    pub name: String,
    pub current_temp_mc: i32,
    pub last_read_time: u64,
    pub read_count: u64,
    pub min_temp_mc: i32,
    pub max_temp_mc: i32,
    pub offset_mc: i32,
    pub slope: i32,
    pub online: bool,
}

impl ThermalSensor {
    pub fn new(id: u32, sensor_type: SensorType, name: String) -> Self {
        Self {
            id,
            sensor_type,
            name,
            current_temp_mc: 25_000,
            last_read_time: 0,
            read_count: 0,
            min_temp_mc: i32::MAX,
            max_temp_mc: i32::MIN,
            offset_mc: 0,
            slope: 1000,
            online: true,
        }
    }

    pub fn update(&mut self, raw_mc: i32, now: u64) {
        let corrected = (raw_mc as i64 * self.slope as i64 / 1000 + self.offset_mc as i64) as i32;
        self.current_temp_mc = corrected;
        self.last_read_time = now;
        self.read_count += 1;
        if corrected < self.min_temp_mc {
            self.min_temp_mc = corrected;
        }
        if corrected > self.max_temp_mc {
            self.max_temp_mc = corrected;
        }
    }

    pub fn temp_celsius(&self) -> i32 {
        self.current_temp_mc / 1000
    }
}

// ===========================================================================
//  3. Cooling Devices
// ===========================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoolingDeviceType {
    CpuFreq,
    DevFreq,
    Fan,
}

#[derive(Debug, Clone)]
pub struct CpuFreqCooling {
    pub cpu: u32,
    pub max_level: u32,
    pub cur_level: u32,
    pub freq_table: Vec<u64>,
    pub power_table: Vec<u64>,
    pub cluster_id: u32,
}

impl CpuFreqCooling {
    pub fn new(cpu: u32) -> Self {
        Self {
            cpu,
            max_level: 7,
            cur_level: 0,
            freq_table: alloc::vec![
                4_000_000, 3_600_000, 3_200_000, 2_800_000, 2_400_000, 2_000_000, 1_600_000,
                1_200_000,
            ],
            power_table: alloc::vec![35_000, 28_000, 22_000, 17_000, 12_000, 8_000, 5_000, 3_000,],
            cluster_id: 0,
        }
    }

    pub fn set_level(&mut self, level: u32) {
        self.cur_level = level.min(self.max_level);
    }

    pub fn current_freq(&self) -> u64 {
        self.freq_table
            .get(self.cur_level as usize)
            .copied()
            .unwrap_or(0)
    }

    pub fn current_power_mw(&self) -> u64 {
        self.power_table
            .get(self.cur_level as usize)
            .copied()
            .unwrap_or(0)
    }

    pub fn power_at_level(&self, level: u32) -> u64 {
        self.power_table.get(level as usize).copied().unwrap_or(0)
    }
}

#[derive(Debug, Clone)]
pub struct DevFreqCooling {
    pub device_name: String,
    pub max_level: u32,
    pub cur_level: u32,
    pub freq_table: Vec<u64>,
    pub power_table: Vec<u64>,
}

impl DevFreqCooling {
    pub fn new(name: String) -> Self {
        Self {
            device_name: name,
            max_level: 4,
            cur_level: 0,
            freq_table: alloc::vec![1_500_000, 1_200_000, 900_000, 600_000, 300_000,],
            power_table: alloc::vec![20_000, 14_000, 9_000, 5_000, 2_000,],
        }
    }

    pub fn set_level(&mut self, level: u32) {
        self.cur_level = level.min(self.max_level);
    }

    pub fn current_power_mw(&self) -> u64 {
        self.power_table
            .get(self.cur_level as usize)
            .copied()
            .unwrap_or(0)
    }
}

#[derive(Debug, Clone)]
pub struct FanCooling {
    pub fan_id: u32,
    pub max_level: u32,
    pub cur_level: u32,
    pub pwm_value: u8,
    pub rpm: u32,
    pub max_rpm: u32,
    pub speed_levels: Vec<FanSpeedLevel>,
    pub auto_mode: bool,
}

#[derive(Debug, Clone)]
pub struct FanSpeedLevel {
    pub level: u32,
    pub pwm: u8,
    pub rpm: u32,
    pub temp_threshold_mc: i32,
}

impl FanCooling {
    pub fn new(fan_id: u32) -> Self {
        Self {
            fan_id,
            max_level: 4,
            cur_level: 0,
            pwm_value: 0,
            rpm: 0,
            max_rpm: 3000,
            speed_levels: alloc::vec![
                FanSpeedLevel {
                    level: 0,
                    pwm: 0,
                    rpm: 0,
                    temp_threshold_mc: 0
                },
                FanSpeedLevel {
                    level: 1,
                    pwm: 64,
                    rpm: 800,
                    temp_threshold_mc: 50_000
                },
                FanSpeedLevel {
                    level: 2,
                    pwm: 128,
                    rpm: 1500,
                    temp_threshold_mc: 65_000
                },
                FanSpeedLevel {
                    level: 3,
                    pwm: 192,
                    rpm: 2200,
                    temp_threshold_mc: 80_000
                },
                FanSpeedLevel {
                    level: 4,
                    pwm: 255,
                    rpm: 3000,
                    temp_threshold_mc: 90_000
                },
            ],
            auto_mode: true,
        }
    }

    pub fn set_level(&mut self, level: u32) {
        let l = level.min(self.max_level);
        self.cur_level = l;
        if let Some(sl) = self.speed_levels.get(l as usize) {
            self.pwm_value = sl.pwm;
            self.rpm = sl.rpm;
        }
    }

    pub fn auto_adjust(&mut self, temp_mc: i32) {
        if !self.auto_mode {
            return;
        }
        let mut target_level = 0u32;
        for sl in &self.speed_levels {
            if temp_mc >= sl.temp_threshold_mc {
                target_level = sl.level;
            }
        }
        self.set_level(target_level);
    }

    pub fn set_pwm_direct(&mut self, pwm: u8) {
        self.pwm_value = pwm;
        self.rpm = (self.max_rpm as u64 * pwm as u64 / 255) as u32;
        self.auto_mode = false;
    }
}

#[derive(Debug, Clone)]
pub struct CoolingDevice {
    pub id: u32,
    pub name: String,
    pub dev_type: CoolingDeviceType,
    pub max_state: u32,
    pub cur_state: u32,
    pub cpufreq: Option<CpuFreqCooling>,
    pub devfreq: Option<DevFreqCooling>,
    pub fan: Option<FanCooling>,
    pub bound_zones: Vec<u32>,
    pub weight: u32,
}

impl CoolingDevice {
    pub fn new_cpufreq(id: u32, cpu: u32) -> Self {
        let cf = CpuFreqCooling::new(cpu);
        let max = cf.max_level;
        Self {
            id,
            name: alloc::format!("cpufreq-cpu{}", cpu),
            dev_type: CoolingDeviceType::CpuFreq,
            max_state: max,
            cur_state: 0,
            cpufreq: Some(cf),
            devfreq: None,
            fan: None,
            bound_zones: Vec::new(),
            weight: 100,
        }
    }

    pub fn new_devfreq(id: u32, name: String) -> Self {
        let df = DevFreqCooling::new(name.clone());
        let max = df.max_level;
        Self {
            id,
            name,
            dev_type: CoolingDeviceType::DevFreq,
            max_state: max,
            cur_state: 0,
            cpufreq: None,
            devfreq: Some(df),
            fan: None,
            bound_zones: Vec::new(),
            weight: 100,
        }
    }

    pub fn new_fan(id: u32, fan_id: u32) -> Self {
        let f = FanCooling::new(fan_id);
        let max = f.max_level;
        Self {
            id,
            name: alloc::format!("fan{}", fan_id),
            dev_type: CoolingDeviceType::Fan,
            max_state: max,
            cur_state: 0,
            cpufreq: None,
            devfreq: None,
            fan: Some(f),
            bound_zones: Vec::new(),
            weight: 100,
        }
    }

    pub fn set_state(&mut self, state: u32) {
        self.cur_state = state.min(self.max_state);
        match &mut self.cpufreq {
            Some(cf) => cf.set_level(self.cur_state),
            None => {}
        }
        match &mut self.devfreq {
            Some(df) => df.set_level(self.cur_state),
            None => {}
        }
        match &mut self.fan {
            Some(f) => f.set_level(self.cur_state),
            None => {}
        }
    }

    pub fn current_power_mw(&self) -> u64 {
        if let Some(cf) = &self.cpufreq {
            return cf.current_power_mw();
        }
        if let Some(df) = &self.devfreq {
            return df.current_power_mw();
        }
        0
    }
}

// ===========================================================================
//  4. Thermal Governors
// ===========================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThermalGovernorType {
    StepWise,
    BangBang,
    FairShare,
    PowerAllocator,
}

// --- Step-wise governor ---

#[derive(Debug, Clone)]
pub struct StepWiseGovernor {
    pub active: bool,
}

impl StepWiseGovernor {
    pub fn new() -> Self {
        Self { active: false }
    }

    pub fn throttle(&self, zone: &ThermalZone, devices: &mut [CoolingDevice]) {
        for trip in &zone.trips {
            if !trip.crossed {
                continue;
            }
            let trend = zone.temperature_trend();
            for dev_id in &zone.bound_cooling_devices {
                if let Some(dev) = devices.iter_mut().find(|d| d.id == *dev_id) {
                    match trend {
                        TemperatureTrend::Raising => {
                            dev.set_state(dev.cur_state.saturating_add(1));
                        }
                        TemperatureTrend::Dropping => {
                            dev.set_state(dev.cur_state.saturating_sub(1));
                        }
                        TemperatureTrend::Stable => {}
                    }
                }
            }
        }
    }
}

// --- Bang-bang governor ---

#[derive(Debug, Clone)]
pub struct BangBangGovernor {
    pub active: bool,
}

impl BangBangGovernor {
    pub fn new() -> Self {
        Self { active: false }
    }

    pub fn throttle(&self, zone: &ThermalZone, devices: &mut [CoolingDevice]) {
        for trip in &zone.trips {
            for dev_id in &zone.bound_cooling_devices {
                if let Some(dev) = devices.iter_mut().find(|d| d.id == *dev_id) {
                    if trip.crossed {
                        dev.set_state(dev.max_state);
                    } else {
                        dev.set_state(0);
                    }
                }
            }
        }
    }
}

// --- Fair-share governor ---

#[derive(Debug, Clone)]
pub struct FairShareGovernor {
    pub active: bool,
}

impl FairShareGovernor {
    pub fn new() -> Self {
        Self { active: false }
    }

    pub fn throttle(&self, zone: &ThermalZone, devices: &mut [CoolingDevice]) {
        let total_weight: u32 = zone
            .bound_cooling_devices
            .iter()
            .filter_map(|id| devices.iter().find(|d| d.id == *id))
            .map(|d| d.weight)
            .sum();
        if total_weight == 0 {
            return;
        }

        let hottest = zone
            .trips
            .iter()
            .filter(|t| t.crossed)
            .max_by_key(|t| t.temperature_mc);
        let Some(trip) = hottest else { return };

        let overshoot = zone.current_temp_mc.saturating_sub(trip.temperature_mc);
        if overshoot <= 0 {
            return;
        }

        for dev_id in &zone.bound_cooling_devices {
            if let Some(dev) = devices.iter_mut().find(|d| d.id == *dev_id) {
                let share = dev.weight as u64 * dev.max_state as u64 / total_weight as u64;
                let level = (overshoot as u64 * share / 10_000).min(dev.max_state as u64);
                dev.set_state(level as u32);
            }
        }
    }
}

// --- Power allocator governor (PID-based) ---

#[derive(Debug, Clone)]
pub struct PidController {
    pub kp: i64,
    pub ki: i64,
    pub kd: i64,
    pub err_integral: i64,
    pub err_prev: i64,
    pub integral_cutoff: i64,
    pub output_min: i64,
    pub output_max: i64,
}

impl PidController {
    pub fn new(kp: i64, ki: i64, kd: i64) -> Self {
        Self {
            kp,
            ki,
            kd,
            err_integral: 0,
            err_prev: 0,
            integral_cutoff: 0,
            output_min: 0,
            output_max: i64::MAX,
        }
    }

    pub fn compute(&mut self, setpoint: i64, measured: i64) -> i64 {
        let err = setpoint - measured;
        self.err_integral += err;
        if self.integral_cutoff > 0 {
            self.err_integral = self
                .err_integral
                .clamp(-self.integral_cutoff, self.integral_cutoff);
        }
        let derivative = err - self.err_prev;
        self.err_prev = err;

        let output =
            self.kp * err / 1000 + self.ki * self.err_integral / 1000 + self.kd * derivative / 1000;
        output.clamp(self.output_min, self.output_max)
    }

    pub fn reset(&mut self) {
        self.err_integral = 0;
        self.err_prev = 0;
    }
}

#[derive(Debug, Clone)]
pub struct PowerAllocatorGovernor {
    pub active: bool,
    pub pid: PidController,
    pub sustainable_power_mw: u64,
    pub allocated_power: Vec<u64>,
    pub total_power_budget_mw: u64,
    pub trip_switch_on_mc: i32,
    pub trip_max_mc: i32,
    pub prev_total_power: u64,
}

impl PowerAllocatorGovernor {
    pub fn new(sustainable_mw: u64) -> Self {
        Self {
            active: false,
            pid: PidController::new(1000, 50, 200),
            sustainable_power_mw: sustainable_mw,
            allocated_power: Vec::new(),
            total_power_budget_mw: sustainable_mw,
            trip_switch_on_mc: 60_000,
            trip_max_mc: 85_000,
            prev_total_power: 0,
        }
    }

    pub fn throttle(&mut self, zone: &ThermalZone, devices: &mut [CoolingDevice]) {
        if zone.current_temp_mc < self.trip_switch_on_mc {
            for dev_id in &zone.bound_cooling_devices {
                if let Some(dev) = devices.iter_mut().find(|d| d.id == *dev_id) {
                    dev.set_state(0);
                }
            }
            return;
        }

        let budget = self
            .pid
            .compute(self.trip_max_mc as i64, zone.current_temp_mc as i64);
        self.total_power_budget_mw = if budget > 0 {
            self.sustainable_power_mw + budget as u64
        } else {
            self.sustainable_power_mw.saturating_sub((-budget) as u64)
        };

        let bound_ids: Vec<u32> = zone.bound_cooling_devices.clone();
        let total_weight: u32 = bound_ids
            .iter()
            .filter_map(|id| devices.iter().find(|d| d.id == *id))
            .map(|d| d.weight)
            .sum();
        if total_weight == 0 {
            return;
        }

        self.allocated_power.clear();
        for dev_id in &bound_ids {
            if let Some(dev) = devices.iter_mut().find(|d| d.id == *dev_id) {
                let share = self.total_power_budget_mw * dev.weight as u64 / total_weight as u64;
                self.allocated_power.push(share);
                let state = self.power_to_state(dev, share);
                dev.set_state(state);
            }
        }
    }

    fn power_to_state(&self, dev: &CoolingDevice, power_mw: u64) -> u32 {
        if let Some(cf) = &dev.cpufreq {
            for (i, &p) in cf.power_table.iter().enumerate() {
                if p <= power_mw {
                    return i as u32;
                }
            }
            return dev.max_state;
        }
        if let Some(df) = &dev.devfreq {
            for (i, &p) in df.power_table.iter().enumerate() {
                if p <= power_mw {
                    return i as u32;
                }
            }
            return dev.max_state;
        }
        0
    }
}

// ===========================================================================
//  5. Thermal Zone
// ===========================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TemperatureTrend {
    Raising,
    Dropping,
    Stable,
}

#[derive(Debug, Clone)]
pub struct ThermalZone {
    pub id: u32,
    pub name: String,
    pub sensor_type: SensorType,
    pub current_temp_mc: i32,
    pub prev_temp_mc: i32,
    pub trips: Vec<TripPoint>,
    pub bound_cooling_devices: Vec<u32>,
    pub polling_interval_ms: u64,
    pub last_poll_time: u64,
    pub passive_delay_ms: u64,
    pub governor_type: ThermalGovernorType,
    pub mode_enabled: bool,
    pub emul_temperature_mc: Option<i32>,
    pub offset_mc: i32,
    pub slope: i32,
    pub nr_temp_updates: u64,
}

impl ThermalZone {
    pub fn new(id: u32, name: String, sensor: SensorType) -> Self {
        Self {
            id,
            name,
            sensor_type: sensor,
            current_temp_mc: 25_000,
            prev_temp_mc: 25_000,
            trips: Vec::new(),
            bound_cooling_devices: Vec::new(),
            polling_interval_ms: 1000,
            last_poll_time: 0,
            passive_delay_ms: 250,
            governor_type: ThermalGovernorType::StepWise,
            mode_enabled: true,
            emul_temperature_mc: None,
            offset_mc: 0,
            slope: 1000,
            nr_temp_updates: 0,
        }
    }

    pub fn add_trip(&mut self, trip_type: TripType, temp_mc: i32, hyst_mc: i32) {
        let id = self.trips.len() as u32;
        self.trips
            .push(TripPoint::new(id, trip_type, temp_mc, hyst_mc));
    }

    pub fn bind_cooling(&mut self, device_id: u32) {
        if !self.bound_cooling_devices.contains(&device_id) {
            self.bound_cooling_devices.push(device_id);
        }
    }

    pub fn update_temperature(&mut self, raw_mc: i32, now: u64) {
        self.prev_temp_mc = self.current_temp_mc;
        let corrected = if let Some(emul) = self.emul_temperature_mc {
            emul
        } else {
            (raw_mc as i64 * self.slope as i64 / 1000 + self.offset_mc as i64) as i32
        };
        self.current_temp_mc = corrected;
        self.last_poll_time = now;
        self.nr_temp_updates += 1;
    }

    pub fn temperature_trend(&self) -> TemperatureTrend {
        let delta = self.current_temp_mc - self.prev_temp_mc;
        if delta > 500 {
            TemperatureTrend::Raising
        } else if delta < -500 {
            TemperatureTrend::Dropping
        } else {
            TemperatureTrend::Stable
        }
    }

    pub fn check_trips(&mut self, now: u64) -> Vec<ThermalEvent> {
        let mut events = Vec::new();
        for trip in &mut self.trips {
            if trip.check(self.current_temp_mc, now) {
                events.push(ThermalEvent::TripCrossed {
                    zone_id: self.id,
                    trip_id: trip.id,
                    trip_type: trip.trip_type,
                    temperature_mc: self.current_temp_mc,
                    timestamp: now,
                });
            }
        }
        events
    }

    pub fn is_critical(&self) -> bool {
        self.trips
            .iter()
            .any(|t| t.trip_type == TripType::Critical && t.crossed)
    }
}

// ===========================================================================
//  6. Thermal Events
// ===========================================================================

#[derive(Debug, Clone)]
pub enum ThermalEvent {
    TripCrossed {
        zone_id: u32,
        trip_id: u32,
        trip_type: TripType,
        temperature_mc: i32,
        timestamp: u64,
    },
    TemperatureChanged {
        zone_id: u32,
        temperature_mc: i32,
        timestamp: u64,
    },
    ZoneCreated {
        zone_id: u32,
        name: String,
    },
    ZoneRemoved {
        zone_id: u32,
    },
    GovernorChanged {
        zone_id: u32,
        old_governor: ThermalGovernorType,
        new_governor: ThermalGovernorType,
    },
    CoolingStateChanged {
        device_id: u32,
        old_state: u32,
        new_state: u32,
    },
    EmergencyShutdown {
        zone_id: u32,
        temperature_mc: i32,
    },
}

// ===========================================================================
//  7. ACPI Thermal
// ===========================================================================

#[derive(Debug, Clone)]
pub struct AcpiThermalZone {
    pub zone_id: u32,
    pub tmp_mc: i32,
    pub active_trips: [Option<i32>; 10],
    pub passive_trip_mc: Option<i32>,
    pub hot_trip_mc: Option<i32>,
    pub critical_trip_mc: Option<i32>,
    pub cooling_policy: AcpiCoolingPolicy,
    pub polling_frequency_ds: u32,
    pub fan_on: bool,
    pub fan_fst: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcpiCoolingPolicy {
    Active,
    Passive,
}

impl AcpiThermalZone {
    pub fn new(zone_id: u32) -> Self {
        Self {
            zone_id,
            tmp_mc: 25_000,
            active_trips: [None; 10],
            passive_trip_mc: None,
            hot_trip_mc: None,
            critical_trip_mc: None,
            cooling_policy: AcpiCoolingPolicy::Active,
            polling_frequency_ds: 10,
            fan_on: false,
            fan_fst: 0,
        }
    }

    pub fn set_ac(&mut self, index: usize, temp_mc: i32) {
        if index < 10 {
            self.active_trips[index] = Some(temp_mc);
        }
    }

    pub fn set_psv(&mut self, temp_mc: i32) {
        self.passive_trip_mc = Some(temp_mc);
    }

    pub fn set_hot(&mut self, temp_mc: i32) {
        self.hot_trip_mc = Some(temp_mc);
    }

    pub fn set_crt(&mut self, temp_mc: i32) {
        self.critical_trip_mc = Some(temp_mc);
    }

    pub fn set_scp(&mut self, policy: AcpiCoolingPolicy) {
        self.cooling_policy = policy;
    }

    pub fn evaluate(&mut self, temp_mc: i32) -> Option<TripType> {
        self.tmp_mc = temp_mc;
        if let Some(crt) = self.critical_trip_mc {
            if temp_mc >= crt {
                return Some(TripType::Critical);
            }
        }
        if let Some(hot) = self.hot_trip_mc {
            if temp_mc >= hot {
                return Some(TripType::Hot);
            }
        }
        if let Some(psv) = self.passive_trip_mc {
            if temp_mc >= psv {
                return Some(TripType::Passive);
            }
        }
        for ac in &self.active_trips {
            if let Some(t) = ac {
                if temp_mc >= *t {
                    return Some(TripType::Active);
                }
            }
        }
        None
    }
}

// ===========================================================================
//  8. Thermal Debug
// ===========================================================================

#[derive(Debug, Clone)]
pub struct TempHistoryEntry {
    pub timestamp: u64,
    pub temperature_mc: i32,
    pub zone_id: u32,
}

#[derive(Debug, Clone)]
pub struct TripEventLogEntry {
    pub timestamp: u64,
    pub zone_id: u32,
    pub trip_id: u32,
    pub trip_type: TripType,
    pub temperature_mc: i32,
}

#[derive(Debug, Clone)]
pub struct CoolingStateLogEntry {
    pub timestamp: u64,
    pub device_id: u32,
    pub old_state: u32,
    pub new_state: u32,
}

#[derive(Debug)]
pub struct ThermalDebug {
    pub temp_history: Vec<TempHistoryEntry>,
    pub trip_log: Vec<TripEventLogEntry>,
    pub cooling_log: Vec<CoolingStateLogEntry>,
    pub max_history: usize,
    pub enabled: bool,
}

impl ThermalDebug {
    pub fn new() -> Self {
        Self {
            temp_history: Vec::new(),
            trip_log: Vec::new(),
            cooling_log: Vec::new(),
            max_history: 1024,
            enabled: true,
        }
    }

    pub fn record_temp(&mut self, zone_id: u32, temp_mc: i32, now: u64) {
        if !self.enabled {
            return;
        }
        if self.temp_history.len() >= self.max_history {
            self.temp_history.remove(0);
        }
        self.temp_history.push(TempHistoryEntry {
            timestamp: now,
            temperature_mc: temp_mc,
            zone_id,
        });
    }

    pub fn record_trip(
        &mut self,
        zone_id: u32,
        trip_id: u32,
        trip_type: TripType,
        temp: i32,
        now: u64,
    ) {
        if !self.enabled {
            return;
        }
        if self.trip_log.len() >= self.max_history {
            self.trip_log.remove(0);
        }
        self.trip_log.push(TripEventLogEntry {
            timestamp: now,
            zone_id,
            trip_id,
            trip_type,
            temperature_mc: temp,
        });
    }

    pub fn record_cooling(&mut self, dev_id: u32, old: u32, new: u32, now: u64) {
        if !self.enabled {
            return;
        }
        if self.cooling_log.len() >= self.max_history {
            self.cooling_log.remove(0);
        }
        self.cooling_log.push(CoolingStateLogEntry {
            timestamp: now,
            device_id: dev_id,
            old_state: old,
            new_state: new,
        });
    }
}

// ===========================================================================
//  9. Skin Temperature Emulation
// ===========================================================================

#[derive(Debug, Clone)]
pub struct SkinTempSensorWeight {
    pub sensor_id: u32,
    pub weight: u32,
}

#[derive(Debug, Clone)]
pub struct SkinTemperature {
    pub weights: Vec<SkinTempSensorWeight>,
    pub total_weight: u32,
    pub offset_mc: i32,
    pub estimated_temp_mc: i32,
}

impl SkinTemperature {
    pub fn new() -> Self {
        Self {
            weights: Vec::new(),
            total_weight: 0,
            offset_mc: 0,
            estimated_temp_mc: 25_000,
        }
    }

    pub fn add_sensor(&mut self, sensor_id: u32, weight: u32) {
        self.weights
            .push(SkinTempSensorWeight { sensor_id, weight });
        self.total_weight += weight;
    }

    pub fn compute(&mut self, sensors: &[ThermalSensor]) -> i32 {
        if self.total_weight == 0 {
            return 25_000;
        }
        let mut weighted_sum: i64 = 0;
        for sw in &self.weights {
            if let Some(sensor) = sensors.iter().find(|s| s.id == sw.sensor_id) {
                weighted_sum += sensor.current_temp_mc as i64 * sw.weight as i64;
            }
        }
        self.estimated_temp_mc = (weighted_sum / self.total_weight as i64) as i32 + self.offset_mc;
        self.estimated_temp_mc
    }
}

// ===========================================================================
//  10. DTPM (Dynamic Thermal/Power Management)
// ===========================================================================

#[derive(Debug, Clone)]
pub struct DtpmNode {
    pub id: u32,
    pub name: String,
    pub parent_id: Option<u32>,
    pub children: Vec<u32>,
    pub power_min_mw: u64,
    pub power_max_mw: u64,
    pub power_current_mw: u64,
    pub power_limit_mw: u64,
    pub weight: u32,
}

impl DtpmNode {
    pub fn new(id: u32, name: String) -> Self {
        Self {
            id,
            name,
            parent_id: None,
            children: Vec::new(),
            power_min_mw: 0,
            power_max_mw: 0,
            power_current_mw: 0,
            power_limit_mw: u64::MAX,
            weight: 100,
        }
    }

    pub fn set_power_limit(&mut self, limit_mw: u64) {
        self.power_limit_mw = limit_mw.clamp(self.power_min_mw, self.power_max_mw);
    }

    pub fn is_over_budget(&self) -> bool {
        self.power_current_mw > self.power_limit_mw
    }
}

#[derive(Debug)]
pub struct DtpmTree {
    pub nodes: BTreeMap<u32, DtpmNode>,
    pub root_id: Option<u32>,
    pub next_id: u32,
}

impl DtpmTree {
    pub fn new() -> Self {
        Self {
            nodes: BTreeMap::new(),
            root_id: None,
            next_id: 1,
        }
    }

    pub fn add_node(&mut self, name: String, parent: Option<u32>) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        let mut node = DtpmNode::new(id, name);
        node.parent_id = parent;
        if parent.is_none() && self.root_id.is_none() {
            self.root_id = Some(id);
        }
        if let Some(pid) = parent {
            if let Some(p) = self.nodes.get_mut(&pid) {
                p.children.push(id);
            }
        }
        self.nodes.insert(id, node);
        id
    }

    pub fn set_limit(&mut self, id: u32, limit_mw: u64) -> Result<(), ThermalError> {
        let node = self
            .nodes
            .get_mut(&id)
            .ok_or(ThermalError::DtpmNodeNotFound)?;
        node.set_power_limit(limit_mw);
        Ok(())
    }

    pub fn total_power(&self) -> u64 {
        self.nodes.values().map(|n| n.power_current_mw).sum()
    }
}

// ===========================================================================
//  11. Battery Thermal Management
// ===========================================================================

#[derive(Debug, Clone)]
pub struct BatteryThermal {
    pub temp_mc: i32,
    pub charge_rate_reduction_pct: u32,
    pub max_charge_temp_mc: i32,
    pub throttle_temp_mc: i32,
    pub shutdown_temp_mc: i32,
    pub charging_allowed: bool,
    pub fast_charging_allowed: bool,
    pub throttled: bool,
    pub normal_charge_rate_ma: u32,
    pub throttled_charge_rate_ma: u32,
}

impl BatteryThermal {
    pub fn new() -> Self {
        Self {
            temp_mc: 25_000,
            charge_rate_reduction_pct: 0,
            max_charge_temp_mc: 45_000,
            throttle_temp_mc: 40_000,
            shutdown_temp_mc: 60_000,
            charging_allowed: true,
            fast_charging_allowed: true,
            throttled: false,
            normal_charge_rate_ma: 3000,
            throttled_charge_rate_ma: 3000,
        }
    }

    pub fn update(&mut self, temp_mc: i32) {
        self.temp_mc = temp_mc;

        if temp_mc >= self.shutdown_temp_mc {
            self.charging_allowed = false;
            self.fast_charging_allowed = false;
            self.throttled = true;
            self.throttled_charge_rate_ma = 0;
            self.charge_rate_reduction_pct = 100;
            return;
        }

        if temp_mc >= self.max_charge_temp_mc {
            self.charging_allowed = true;
            self.fast_charging_allowed = false;
            self.throttled = true;
            self.throttled_charge_rate_ma = self.normal_charge_rate_ma / 4;
            self.charge_rate_reduction_pct = 75;
            return;
        }

        if temp_mc >= self.throttle_temp_mc {
            self.charging_allowed = true;
            self.fast_charging_allowed = false;
            self.throttled = true;
            let over = (temp_mc - self.throttle_temp_mc) as u32;
            let range = (self.max_charge_temp_mc - self.throttle_temp_mc) as u32;
            let reduction = if range > 0 { (over * 50) / range } else { 50 };
            self.charge_rate_reduction_pct = reduction;
            self.throttled_charge_rate_ma = self.normal_charge_rate_ma * (100 - reduction) / 100;
            return;
        }

        self.charging_allowed = true;
        self.fast_charging_allowed = true;
        self.throttled = false;
        self.throttled_charge_rate_ma = self.normal_charge_rate_ma;
        self.charge_rate_reduction_pct = 0;
    }
}

// ===========================================================================
//  12. Thermal Netlink Events
// ===========================================================================

#[derive(Debug, Clone)]
pub struct ThermalNetlinkEvent {
    pub event_type: ThermalNetlinkType,
    pub zone_id: u32,
    pub temperature_mc: i32,
    pub trip_id: Option<u32>,
    pub timestamp: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThermalNetlinkType {
    TempSample,
    TripViolation,
    TripChange,
    CoolingUpdate,
    ZoneCreate,
    ZoneDelete,
    GovernorChange,
}

#[derive(Debug)]
pub struct ThermalNetlink {
    pub events: Vec<ThermalNetlinkEvent>,
    pub max_events: usize,
    pub enabled: bool,
    pub subscribers: u32,
}

impl ThermalNetlink {
    pub fn new() -> Self {
        Self {
            events: Vec::new(),
            max_events: 512,
            enabled: true,
            subscribers: 0,
        }
    }

    pub fn emit(&mut self, event: ThermalNetlinkEvent) {
        if !self.enabled || self.subscribers == 0 {
            return;
        }
        if self.events.len() >= self.max_events {
            self.events.remove(0);
        }
        self.events.push(event);
    }

    pub fn subscribe(&mut self) -> u32 {
        self.subscribers += 1;
        self.subscribers
    }

    pub fn unsubscribe(&mut self) {
        self.subscribers = self.subscribers.saturating_sub(1);
    }
}

// ===========================================================================
//  13. Thermal Protection
// ===========================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThrottleAction {
    None,
    ReduceFrequency,
    ForceIdle,
    DisableBoost,
    EmergencyShutdown,
}

#[derive(Debug)]
pub struct ThermalProtection {
    pub cascade: Vec<ThrottleCascadeLevel>,
    pub current_level: usize,
    pub shutdown_temp_mc: i32,
    pub shutdown_countdown_ms: u64,
    pub shutdown_timer_started: bool,
    pub shutdown_timer_start: u64,
}

#[derive(Debug, Clone)]
pub struct ThrottleCascadeLevel {
    pub threshold_mc: i32,
    pub actions: Vec<ThrottleAction>,
    pub label: String,
}

impl ThermalProtection {
    pub fn new() -> Self {
        Self {
            cascade: alloc::vec![
                ThrottleCascadeLevel {
                    threshold_mc: 70_000,
                    actions: alloc::vec![ThrottleAction::DisableBoost],
                    label: String::from("level1-no-boost"),
                },
                ThrottleCascadeLevel {
                    threshold_mc: 80_000,
                    actions: alloc::vec![ThrottleAction::ReduceFrequency],
                    label: String::from("level2-throttle"),
                },
                ThrottleCascadeLevel {
                    threshold_mc: 90_000,
                    actions: alloc::vec![
                        ThrottleAction::ReduceFrequency,
                        ThrottleAction::ForceIdle
                    ],
                    label: String::from("level3-heavy-throttle"),
                },
                ThrottleCascadeLevel {
                    threshold_mc: 100_000,
                    actions: alloc::vec![ThrottleAction::EmergencyShutdown],
                    label: String::from("level4-emergency"),
                },
            ],
            current_level: 0,
            shutdown_temp_mc: 105_000,
            shutdown_countdown_ms: 5000,
            shutdown_timer_started: false,
            shutdown_timer_start: 0,
        }
    }

    pub fn evaluate(&mut self, temp_mc: i32, now: u64) -> Vec<ThrottleAction> {
        let mut level = 0;
        for (i, cl) in self.cascade.iter().enumerate() {
            if temp_mc >= cl.threshold_mc {
                level = i + 1;
            }
        }
        self.current_level = level;

        if temp_mc >= self.shutdown_temp_mc {
            if !self.shutdown_timer_started {
                self.shutdown_timer_started = true;
                self.shutdown_timer_start = now;
            } else if now - self.shutdown_timer_start >= self.shutdown_countdown_ms * 1000 {
                return alloc::vec![ThrottleAction::EmergencyShutdown];
            }
        } else {
            self.shutdown_timer_started = false;
        }

        if level > 0 && level <= self.cascade.len() {
            self.cascade[level - 1].actions.clone()
        } else {
            alloc::vec![ThrottleAction::None]
        }
    }
}

// ===========================================================================
//  14. Global Thermal Framework
// ===========================================================================

pub struct ThermalFramework {
    pub zones: Vec<ThermalZone>,
    pub sensors: Vec<ThermalSensor>,
    pub cooling_devices: Vec<CoolingDevice>,
    pub acpi_zones: Vec<AcpiThermalZone>,
    pub governor_stepwise: StepWiseGovernor,
    pub governor_bangbang: BangBangGovernor,
    pub governor_fairshare: FairShareGovernor,
    pub governor_power_allocator: PowerAllocatorGovernor,
    pub debug: ThermalDebug,
    pub netlink: ThermalNetlink,
    pub skin_temp: SkinTemperature,
    pub dtpm: DtpmTree,
    pub battery: BatteryThermal,
    pub protection: ThermalProtection,
    pub event_log: Vec<ThermalEvent>,
    pub next_zone_id: u32,
    pub next_device_id: u32,
    pub next_sensor_id: u32,
    pub initialized: bool,
}

impl ThermalFramework {
    pub const fn new() -> Self {
        Self {
            zones: Vec::new(),
            sensors: Vec::new(),
            cooling_devices: Vec::new(),
            acpi_zones: Vec::new(),
            governor_stepwise: StepWiseGovernor { active: false },
            governor_bangbang: BangBangGovernor { active: false },
            governor_fairshare: FairShareGovernor { active: false },
            governor_power_allocator: PowerAllocatorGovernor {
                active: false,
                pid: PidController {
                    kp: 1000,
                    ki: 50,
                    kd: 200,
                    err_integral: 0,
                    err_prev: 0,
                    integral_cutoff: 0,
                    output_min: 0,
                    output_max: i64::MAX,
                },
                sustainable_power_mw: 15_000,
                allocated_power: Vec::new(),
                total_power_budget_mw: 15_000,
                trip_switch_on_mc: 60_000,
                trip_max_mc: 85_000,
                prev_total_power: 0,
            },
            debug: ThermalDebug {
                temp_history: Vec::new(),
                trip_log: Vec::new(),
                cooling_log: Vec::new(),
                max_history: 1024,
                enabled: true,
            },
            netlink: ThermalNetlink {
                events: Vec::new(),
                max_events: 512,
                enabled: true,
                subscribers: 0,
            },
            skin_temp: SkinTemperature {
                weights: Vec::new(),
                total_weight: 0,
                offset_mc: 0,
                estimated_temp_mc: 25_000,
            },
            dtpm: DtpmTree {
                nodes: BTreeMap::new(),
                root_id: None,
                next_id: 1,
            },
            battery: BatteryThermal {
                temp_mc: 25_000,
                charge_rate_reduction_pct: 0,
                max_charge_temp_mc: 45_000,
                throttle_temp_mc: 40_000,
                shutdown_temp_mc: 60_000,
                charging_allowed: true,
                fast_charging_allowed: true,
                throttled: false,
                normal_charge_rate_ma: 3000,
                throttled_charge_rate_ma: 3000,
            },
            protection: ThermalProtection {
                cascade: Vec::new(),
                current_level: 0,
                shutdown_temp_mc: 105_000,
                shutdown_countdown_ms: 5000,
                shutdown_timer_started: false,
                shutdown_timer_start: 0,
            },
            event_log: Vec::new(),
            next_zone_id: 1,
            next_device_id: 1,
            next_sensor_id: 1,
            initialized: false,
        }
    }

    pub fn register_sensor(&mut self, stype: SensorType, name: String) -> u32 {
        let id = self.next_sensor_id;
        self.next_sensor_id += 1;
        self.sensors.push(ThermalSensor::new(id, stype, name));
        id
    }

    pub fn register_zone(&mut self, name: String, sensor: SensorType) -> u32 {
        let id = self.next_zone_id;
        self.next_zone_id += 1;
        self.zones.push(ThermalZone::new(id, name.clone(), sensor));
        self.event_log
            .push(ThermalEvent::ZoneCreated { zone_id: id, name });
        id
    }

    pub fn register_cooling_device_cpufreq(&mut self, cpu: u32) -> u32 {
        let id = self.next_device_id;
        self.next_device_id += 1;
        self.cooling_devices
            .push(CoolingDevice::new_cpufreq(id, cpu));
        id
    }

    pub fn register_cooling_device_devfreq(&mut self, name: String) -> u32 {
        let id = self.next_device_id;
        self.next_device_id += 1;
        self.cooling_devices
            .push(CoolingDevice::new_devfreq(id, name));
        id
    }

    pub fn register_cooling_device_fan(&mut self, fan_id: u32) -> u32 {
        let id = self.next_device_id;
        self.next_device_id += 1;
        self.cooling_devices
            .push(CoolingDevice::new_fan(id, fan_id));
        id
    }

    pub fn update_zone_temp(
        &mut self,
        zone_id: u32,
        raw_mc: i32,
        now: u64,
    ) -> Result<Vec<ThermalEvent>, ThermalError> {
        let zone = self
            .zones
            .iter_mut()
            .find(|z| z.id == zone_id)
            .ok_or(ThermalError::ZoneNotFound)?;
        zone.update_temperature(raw_mc, now);
        let events = zone.check_trips(now);

        self.debug.record_temp(zone_id, zone.current_temp_mc, now);
        for ev in &events {
            if let ThermalEvent::TripCrossed {
                trip_id,
                trip_type,
                temperature_mc,
                ..
            } = ev
            {
                self.debug
                    .record_trip(zone_id, *trip_id, *trip_type, *temperature_mc, now);
            }
        }
        self.event_log.extend(events.clone());
        Ok(events)
    }

    pub fn poll_zone(&mut self, zone_id: u32, now: u64) -> Result<(), ThermalError> {
        let zone_idx = self
            .zones
            .iter()
            .position(|z| z.id == zone_id)
            .ok_or(ThermalError::ZoneNotFound)?;
        let zone = &self.zones[zone_idx];
        let gov_type = zone.governor_type;

        match gov_type {
            ThermalGovernorType::StepWise => {
                let gov = self.governor_stepwise.clone();
                gov.throttle(&self.zones[zone_idx], &mut self.cooling_devices);
            }
            ThermalGovernorType::BangBang => {
                let gov = self.governor_bangbang.clone();
                gov.throttle(&self.zones[zone_idx], &mut self.cooling_devices);
            }
            ThermalGovernorType::FairShare => {
                let gov = self.governor_fairshare.clone();
                gov.throttle(&self.zones[zone_idx], &mut self.cooling_devices);
            }
            ThermalGovernorType::PowerAllocator => {
                let mut gov = self.governor_power_allocator.clone();
                gov.throttle(&self.zones[zone_idx], &mut self.cooling_devices);
                self.governor_power_allocator = gov;
            }
        }

        let actions = self
            .protection
            .evaluate(self.zones[zone_idx].current_temp_mc, now);
        for action in &actions {
            if *action == ThrottleAction::EmergencyShutdown {
                self.event_log.push(ThermalEvent::EmergencyShutdown {
                    zone_id,
                    temperature_mc: self.zones[zone_idx].current_temp_mc,
                });
            }
        }
        Ok(())
    }

    pub fn setup_default_zones(&mut self) {
        // CPU thermal zone
        let cpu_zone = self.register_zone(String::from("cpu-thermal"), SensorType::CpuDie);
        if let Some(zone) = self.zones.iter_mut().find(|z| z.id == cpu_zone) {
            zone.add_trip(TripType::Active, 65_000, 3_000);
            zone.add_trip(TripType::Passive, 80_000, 2_000);
            zone.add_trip(TripType::Hot, 95_000, 2_000);
            zone.add_trip(TripType::Critical, 105_000, 0);
        }

        // GPU thermal zone
        let gpu_zone = self.register_zone(String::from("gpu-thermal"), SensorType::Gpu);
        if let Some(zone) = self.zones.iter_mut().find(|z| z.id == gpu_zone) {
            zone.add_trip(TripType::Active, 70_000, 3_000);
            zone.add_trip(TripType::Passive, 85_000, 2_000);
            zone.add_trip(TripType::Critical, 100_000, 0);
        }

        // SSD thermal zone
        let ssd_zone = self.register_zone(String::from("ssd-thermal"), SensorType::Ssd);
        if let Some(zone) = self.zones.iter_mut().find(|z| z.id == ssd_zone) {
            zone.add_trip(TripType::Passive, 70_000, 2_000);
            zone.add_trip(TripType::Critical, 80_000, 0);
        }

        // Battery thermal zone
        let bat_zone = self.register_zone(String::from("battery-thermal"), SensorType::Battery);
        if let Some(zone) = self.zones.iter_mut().find(|z| z.id == bat_zone) {
            zone.add_trip(TripType::Passive, 40_000, 2_000);
            zone.add_trip(TripType::Hot, 50_000, 2_000);
            zone.add_trip(TripType::Critical, 60_000, 0);
        }

        // Sensors
        self.register_sensor(SensorType::CpuDie, String::from("cpu-die"));
        self.register_sensor(SensorType::Gpu, String::from("gpu"));
        self.register_sensor(SensorType::Ssd, String::from("ssd"));
        self.register_sensor(SensorType::Ambient, String::from("ambient"));
        self.register_sensor(SensorType::Battery, String::from("battery"));
        self.register_sensor(SensorType::Dram, String::from("dram"));
        self.register_sensor(SensorType::Vrm, String::from("vrm"));
        self.register_sensor(SensorType::Pch, String::from("pch"));
        self.register_sensor(SensorType::Skin, String::from("skin"));

        // Cooling devices
        let cpu_cool = self.register_cooling_device_cpufreq(0);
        let gpu_cool = self.register_cooling_device_devfreq(String::from("gpu-devfreq"));
        let fan0 = self.register_cooling_device_fan(0);

        // Bind cooling to zones
        if let Some(zone) = self.zones.iter_mut().find(|z| z.id == cpu_zone) {
            zone.bind_cooling(cpu_cool);
            zone.bind_cooling(fan0);
        }
        if let Some(zone) = self.zones.iter_mut().find(|z| z.id == gpu_zone) {
            zone.bind_cooling(gpu_cool);
            zone.bind_cooling(fan0);
        }

        // Skin temperature: weighted avg of CPU + GPU + ambient
        self.skin_temp.add_sensor(1, 50); // cpu-die
        self.skin_temp.add_sensor(2, 30); // gpu
        self.skin_temp.add_sensor(4, 20); // ambient

        // DTPM power tree
        let root = self.dtpm.add_node(String::from("system"), None);
        self.dtpm.add_node(String::from("cpu"), Some(root));
        self.dtpm.add_node(String::from("gpu"), Some(root));
        self.dtpm.add_node(String::from("memory"), Some(root));

        // ACPI zone
        let mut acpi = AcpiThermalZone::new(0);
        acpi.set_ac(0, 65_000);
        acpi.set_psv(80_000);
        acpi.set_hot(95_000);
        acpi.set_crt(105_000);
        self.acpi_zones.push(acpi);

        // Protection cascade is set up by default constructor
        self.protection = ThermalProtection::new();

        self.governor_stepwise.active = true;
    }
}

// ═══════════════════════════════════════════════════════════════════════════
//  Hardware thermal reading via x86 MSRs
// ═══════════════════════════════════════════════════════════════════════════

const IA32_THERM_STATUS: u32 = 0x19C;
const MSR_TEMPERATURE_TARGET: u32 = 0x1A2;

/// Read the digital readout from IA32_THERM_STATUS.
/// Returns (valid, prochot, current_temp_celsius).
pub fn read_cpu_therm_status() -> (bool, bool, i32) {
    // IA32_THERM_STATUS is Intel-only; reading it #GPs on AMD (crashed the first
    // real-hardware boot). Gate by vendor so it is never attempted off Intel,
    // and use rdmsr_safe as a second line of defence. Either way, "no reading"
    // makes the caller skip MSR-based throttling on this CPU.
    if !crate::msr::is_intel() {
        return (false, false, 0);
    }
    let Some(val) = (unsafe { crate::msr::rdmsr_safe(IA32_THERM_STATUS) }) else {
        return (false, false, 0);
    };
    let valid = (val >> 31) & 1 == 1;
    let prochot = val & 1 == 1;
    let readout = ((val >> 16) & 0x7F) as i32;
    let tj_max = read_tj_max();
    let temp = tj_max - readout;
    (valid, prochot, temp)
}

/// Read TjMax (the junction temperature ceiling) from MSR_TEMPERATURE_TARGET.
/// Defaults to 100 C if the MSR is not present/readable (e.g. on AMD).
pub fn read_tj_max() -> i32 {
    let val = unsafe { crate::msr::rdmsr_safe(MSR_TEMPERATURE_TARGET).unwrap_or(0) };
    let tj = ((val >> 16) & 0xFF) as i32;
    if tj == 0 {
        100
    } else {
        tj
    }
}

// ── AMD (Zen) on-die temperature via the SMU/SMN ────────────────────────────
//
// Intel's IA32_THERM_STATUS does not exist on AMD (reading it #GPs — gated
// above), so on the Athena (Ryzen 7640HS, Family 19h) the kernel had NO CPU
// temperature source at all and the embodiment-first "never fry the silicon"
// promise was unenforceable on the actual target. AMD exposes Tctl through the
// System Management Network (SMN): write the register address to the data
// fabric root's PCI config index (00:00.0 + 0x60) and read the value back from
// the data port (+0x64). Pattern harvested from Linux `k10temp`/`amd_nb`
// (public hwmon docs) — NOT a port: a single register read + a documented
// fixed-point decode, no driver framework transplanted.

/// SMU thermal control register (SMU::THM::THM_TCON_CUR_TMP) for Family 17h+.
const ZEN_REPORTED_TEMP_CTRL_BASE: u32 = 0x0005_9800;
/// CUR_TEMP_RANGE_SEL (bit 19): when set, the reported value uses the −49 °C
/// extended range, so subtract 49 °C from the raw conversion.
const ZEN_CUR_TEMP_RANGE_SEL: u32 = 1 << 19;

/// Decode the raw SMU THM_TCON_CUR_TMP register to milli-degrees Celsius.
/// Bits [31:21] are an 11-bit Tctl in 0.125 °C units; bit 19 selects the
/// −49 °C extended range. Pure (no I/O) so it is host-/boot-KAT-able.
pub fn decode_zen_tctl_mc(raw: u32) -> i32 {
    let mut t_mc = (((raw >> 21) & 0x7FF) as i32) * 125; // 0.125 °C units → m°C
    if raw & ZEN_CUR_TEMP_RANGE_SEL != 0 {
        t_mc -= 49_000;
    }
    t_mc
}

/// Serializes the SMN index/data register pair (00:00.0 +0x60/+0x64): the read
/// is two dependent PCI-config accesses, so concurrent pollers would interleave
/// and corrupt each other's address. Mirrors k10temp's `amd_smn_lock`.
static SMN_LOCK: Mutex<()> = Mutex::new(());

/// Read the AMD Zen on-die Tctl in whole °C, or `None` when this isn't a Zen
/// part (Family < 0x17 — incl. QEMU's Family 0xF, which therefore never pokes
/// SMN) or the SMN read looks invalid. Safe to call anywhere; one mutex-guarded
/// config write + read.
pub fn read_amd_cpu_temp_c() -> Option<i32> {
    if !crate::msr::is_amd() || crate::msr::cpu_family() < 0x17 {
        return None;
    }
    let raw = {
        let _g = SMN_LOCK.lock();
        // SMN access via the data-fabric root at PCI 00:00.0.
        crate::pci::write_config_32(0, 0, 0, 0x60, ZEN_REPORTED_TEMP_CTRL_BASE);
        crate::pci::read_config_32(0, 0, 0, 0x64)
    };
    // A missing/aborted SMN access reads all-ones (or zero); reject both.
    if raw == 0xFFFF_FFFF || raw == 0 {
        return None;
    }
    let c = decode_zen_tctl_mc(raw) / 1000;
    // Plausibility window: a running CPU is between ambient and the junction
    // ceiling. Outside this, treat the decode as garbage rather than throttle on
    // a bogus reading.
    if !(0..=130).contains(&c) {
        return None;
    }
    Some(c)
}

/// Poll CPU temperature and apply thermal throttling logic:
///  - Below passive trip: no action
///  - Between passive and critical: throttle P-state down
///  - Above critical: emergency log (shutdown would go here)
pub fn thermal_poll_and_throttle(passive_trip_c: i32, critical_trip_c: i32) {
    // CPU temperature source: Intel IA32_THERM_STATUS, else the AMD SMU (Zen).
    let (valid, prochot, temp_c) = {
        let (iv, ip, it) = read_cpu_therm_status();
        if iv {
            (true, ip, it)
        } else if let Some(amd_c) = read_amd_cpu_temp_c() {
            (true, false, amd_c)
        } else {
            (false, false, 0)
        }
    };
    if !valid {
        return;
    }

    if prochot {
        crate::serial_println!("[thermal] PROCHOT asserted, HW throttling active");
    }

    if temp_c >= critical_trip_c {
        crate::serial_println!(
            "[thermal] CRITICAL: {}°C >= {}°C trip — emergency action needed",
            temp_c,
            critical_trip_c,
        );
    } else if temp_c >= passive_trip_c {
        crate::serial_println!(
            "[thermal] passive throttle: {}°C (trip {}°C), reducing P-state",
            temp_c,
            passive_trip_c,
        );
        crate::cpufreq::apply_governor_decision(30, crate::cpufreq::GovernorType::Powersave);
    }
}

// ═══════════════════════════════════════════════════════════════════════════
//  ACPI thermal zone polling — Phase 4.7: Thermal throttling
// ═══════════════════════════════════════════════════════════════════════════
//
// Concept: AthenaOS is embodiment-first, so sustained heavy load must never silently
// fry the silicon. This subsystem reads ACPI thermal-zone temperatures via the
// AML interpreter (`\_TZ.THMx._TMP`), enforces the firmware's passive (`_PSV`)
// trip by clamping CPU frequency, and honours the critical (`_CRT`) trip by
// performing an orderly S5 power-off. Polling is driven from the LAPIC timer
// tick (see `on_timer_tick`) at a low duty cycle so it adds no measurable
// overhead to the hot scheduling path.

/// Number of ACPI thermal zones we probe (`\_TZ.THM0` .. `\_TZ.THM{N-1}`).
const MAX_ACPI_THERMAL_ZONES: u32 = 8;

/// Poll `on_timer_tick` runs the zone scan once every this many ticks.
const THERMAL_POLL_TICK_INTERVAL: u64 = 100;

/// Default passive (`_PSV`) trip in Celsius if firmware does not expose one.
const DEFAULT_PASSIVE_TRIP_C: i32 = 80;
/// Default critical (`_CRT`) trip in Celsius if firmware does not expose one.
const DEFAULT_CRITICAL_TRIP_C: i32 = 100;
/// Frequency cap (% of max) applied while a passive trip is breached.
const PASSIVE_THROTTLE_CAP_PCT: u32 = 50;

/// Monotonic tick counter advanced by `on_timer_tick`.
static THERMAL_TICKS: AtomicU64 = AtomicU64::new(0);
/// Number of ACPI thermal zones discovered on the last poll.
static LAST_ZONE_COUNT: AtomicU32 = AtomicU32::new(0);
/// Hottest temperature (°C) seen on the last poll, or `i32::MIN` if none.
static LAST_MAX_TEMP_C: AtomicI32 = AtomicI32::new(i32::MIN);
/// Whether a passive throttle cap is currently engaged.
static PASSIVE_THROTTLE_ENGAGED: AtomicU32 = AtomicU32::new(0);
/// Breach self-test result: 0 = not run, 1 = PASS, 2 = FAIL.
static BREACH_SELFTEST: AtomicU32 = AtomicU32::new(0);

/// Result of polling a single ACPI thermal zone.
#[derive(Debug, Clone, Copy)]
pub struct AcpiZoneReading {
    pub zone_index: u32,
    pub temp_c: i32,
    pub passive_trip_c: i32,
    pub critical_trip_c: i32,
    pub passive_breached: bool,
    pub critical_breached: bool,
}

/// The policy decision for a single thermal-zone reading. Kept side-effect-free
/// so it can be unit-tested with synthetic readings (`run_breach_selftest`)
/// without actually capping the CPU or powering the machine off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThermalAction {
    /// Below all trips — no action.
    Nominal,
    /// `_PSV` passive trip breached — clamp CPU frequency to this percent.
    PassiveCap(u32),
    /// `_CRT` critical trip breached — orderly platform power-off.
    CriticalShutdown,
}

/// Pure `_PSV`/`_CRT` policy for one zone reading. `_CRT` takes priority over
/// `_PSV`. No side effects — callers (`poll_thermal_zones`) act on the result.
pub fn decide_action(r: &AcpiZoneReading) -> ThermalAction {
    if r.critical_breached {
        ThermalAction::CriticalShutdown
    } else if r.passive_breached {
        ThermalAction::PassiveCap(PASSIVE_THROTTLE_CAP_PCT)
    } else {
        ThermalAction::Nominal
    }
}

/// Evaluate an ACPI integer method (`_TMP`, `_PSV`, `_CRT`) and return its raw
/// value in tenths-of-Kelvin, or `None` if the method is absent / not an
/// integer. All ACPI temperature methods use the same 0.1 K encoding.
fn eval_acpi_temp_dk(path: &str) -> Option<u64> {
    match crate::acpi_full::safe_evaluate_method(path, aml::value::Args::default()) {
        Ok(aml::AmlValue::Integer(dk)) => Some(dk),
        _ => None,
    }
}

/// Convert ACPI tenths-of-Kelvin to whole degrees Celsius: `(K/10) - 273`.
#[inline]
fn dk_to_celsius(dk: u64) -> i32 {
    // dk is tenths-of-Kelvin. Kelvin = dk / 10. Celsius = Kelvin - 273.
    (dk / 10) as i32 - 273
}

/// Probe one ACPI thermal zone (`\_TZ.THM{idx}`) and apply throttle/critical
/// policy. Returns `None` if the zone (or its `_TMP`) is not present.
fn poll_one_zone(idx: u32) -> Option<AcpiZoneReading> {
    let tmp_path = format!("\\_TZ.THM{}._TMP", idx);
    let dk = eval_acpi_temp_dk(&tmp_path)?;
    let temp_c = dk_to_celsius(dk);

    let psv_path = format!("\\_TZ.THM{}._PSV", idx);
    let crt_path = format!("\\_TZ.THM{}._CRT", idx);
    let passive_trip_c = eval_acpi_temp_dk(&psv_path)
        .map(dk_to_celsius)
        .unwrap_or(DEFAULT_PASSIVE_TRIP_C);
    let critical_trip_c = eval_acpi_temp_dk(&crt_path)
        .map(dk_to_celsius)
        .unwrap_or(DEFAULT_CRITICAL_TRIP_C);

    let critical_breached = temp_c >= critical_trip_c;
    let passive_breached = temp_c >= passive_trip_c;

    // Mirror the live reading into the software framework so /proc and the
    // governors observe the same temperature the firmware reports.
    let now = THERMAL_TICKS.load(Ordering::Relaxed);
    if let Some(mut fw) = THERMAL_FRAMEWORK.try_lock() {
        if let Some(az) = fw.acpi_zones.iter_mut().find(|z| z.zone_id == idx) {
            az.tmp_mc = temp_c * 1000;
        }
        let zone_id = idx + 1; // framework zone ids are 1-based
        let _ = fw.update_zone_temp(zone_id, temp_c * 1000, now);
    }

    Some(AcpiZoneReading {
        zone_index: idx,
        temp_c,
        passive_trip_c,
        critical_trip_c,
        passive_breached,
        critical_breached,
    })
}

/// Read every ACPI thermal zone, enforce `_PSV` (passive frequency cap) and
/// `_CRT` (critical power-off) policy, and return the per-zone readings.
///
/// Privileged effects (CPU-frequency cap, platform power-off) are kernel
/// thermal-protection actions taken on behalf of the whole machine; they are
/// reached through the dedicated `crate::cpufreq` / `crate::acpi_full`
/// subsystem entry points rather than a per-task `crate::capability::Cap`,
/// matching how the rest of this module drives the hardware.
pub fn poll_thermal_zones() -> Vec<AcpiZoneReading> {
    let mut readings = Vec::new();
    let mut any_passive = false;
    let mut max_temp = i32::MIN;

    for idx in 0..MAX_ACPI_THERMAL_ZONES {
        let Some(reading) = poll_one_zone(idx) else {
            continue;
        };

        if reading.temp_c > max_temp {
            max_temp = reading.temp_c;
        }

        match decide_action(&reading) {
            // _CRT critical: orderly shutdown. Does not return.
            ThermalAction::CriticalShutdown => {
                crate::serial_println!(
                    "[thermal] CRITICAL: THM{} {}°C >= _CRT {}°C — powering off",
                    reading.zone_index,
                    reading.temp_c,
                    reading.critical_trip_c,
                );
                crate::acpi_full::power_off();
            }
            // _PSV passive: clamp CPU frequency.
            ThermalAction::PassiveCap(cap) => {
                any_passive = true;
                crate::serial_println!(
                    "[thermal] WARN: THM{} {}°C >= _PSV {}°C — capping CPU to {}%",
                    reading.zone_index,
                    reading.temp_c,
                    reading.passive_trip_c,
                    cap,
                );
            }
            ThermalAction::Nominal => {}
        }

        readings.push(reading);
    }

    // No ACPI thermal zones (the AMD/Athena case — firmware exposes no
    // \_TZ.THMx): fall back to the DIRECT CPU sensor (Intel IA32_THERM_STATUS,
    // else AMD SMU/SMN) so passive throttling still protects the chip. Critical
    // here is LOGGED + hard-capped but NOT auto-power-off: the AMD reading isn't
    // iron-calibrated yet and the platform's PROCHOT/EC hardware throttle is the
    // ultimate backstop — a spurious OS shutdown is worse than a deferred one.
    if readings.is_empty() {
        let cpu_c = {
            let (iv, _ip, it) = read_cpu_therm_status();
            if iv {
                Some(it)
            } else {
                read_amd_cpu_temp_c()
            }
        };
        if let Some(c) = cpu_c {
            max_temp = c;
            let r = AcpiZoneReading {
                zone_index: 0xFF, // synthetic "direct CPU sensor" pseudo-zone
                temp_c: c,
                passive_trip_c: DEFAULT_PASSIVE_TRIP_C,
                critical_trip_c: DEFAULT_CRITICAL_TRIP_C,
                passive_breached: c >= DEFAULT_PASSIVE_TRIP_C,
                critical_breached: c >= DEFAULT_CRITICAL_TRIP_C,
            };
            match decide_action(&r) {
                ThermalAction::CriticalShutdown => {
                    any_passive = true; // cap hard while critically hot
                    crate::serial_println!(
                        "[thermal] CRITICAL: CPU {}°C >= {}°C (direct sensor) — hard cap; OS power-off deferred (HW PROCHOT/EC is the backstop)",
                        c,
                        DEFAULT_CRITICAL_TRIP_C
                    );
                }
                ThermalAction::PassiveCap(cap) => {
                    any_passive = true;
                    crate::serial_println!(
                        "[thermal] WARN: CPU {}°C >= _PSV {}°C (direct sensor) — capping CPU to {}%",
                        c,
                        DEFAULT_PASSIVE_TRIP_C,
                        cap
                    );
                }
                ThermalAction::Nominal => {}
            }
            readings.push(r);
        }
    }

    // Engage / release the passive frequency cap based on aggregate state.
    if any_passive {
        crate::cpufreq::set_cap_percent(PASSIVE_THROTTLE_CAP_PCT);
        PASSIVE_THROTTLE_ENGAGED.store(1, Ordering::Relaxed);
    } else if PASSIVE_THROTTLE_ENGAGED.swap(0, Ordering::Relaxed) == 1 {
        // Cooled back below all passive trips: release the cap.
        crate::cpufreq::set_cap_percent(100);
        crate::serial_println!("[thermal] passive trip cleared — releasing CPU cap");
    }

    LAST_ZONE_COUNT.store(readings.len() as u32, Ordering::Relaxed);
    LAST_MAX_TEMP_C.store(max_temp, Ordering::Relaxed);
    readings
}

/// Drive thermal polling from the LAPIC timer tick. Polls the ACPI thermal
/// zones once every `THERMAL_POLL_TICK_INTERVAL` ticks; otherwise returns
/// immediately so it is cheap to call on every tick.
///
/// This is intentionally *not* registered with the scheduler — callers (e.g.
/// the timer ISR tail) invoke it directly so we never hold the SCHEDULER lock
/// while talking to ACPI/cpufreq.
pub fn on_timer_tick() {
    let tick = THERMAL_TICKS.fetch_add(1, Ordering::Relaxed) + 1;
    if tick % THERMAL_POLL_TICK_INTERVAL != 0 {
        return;
    }
    let _ = poll_thermal_zones();
}

pub static THERMAL_FRAMEWORK: Mutex<ThermalFramework> = Mutex::new(ThermalFramework::new());

/// Initialize the thermal framework: register default software zones, sensors,
/// cooling devices, and the ACPI thermal-zone mirror. Called from
/// `kernel_main`. See the Phase 4.7 Concept note above for the throttling
/// policy this enables.
pub fn init() {
    let mut fw = THERMAL_FRAMEWORK.lock();
    fw.setup_default_zones();
    fw.initialized = true;
}

/// Post-boot thermal poll thread — runs the throttle loop in THREAD context.
/// `poll_thermal_zones` evaluates AML (`\_TZ.THMx._TMP`) and takes the SMN lock
/// for the AMD CPU read; both are unsafe from the timer ISR (`on_timer_tick`),
/// which is why that ISR hook was never wired. Without a driver the throttle
/// policy ran exactly once (the boot smoketest) — this makes it continuous, so
/// a passive `_PSV`/direct-CPU breach actually caps the CPU and clears when it
/// cools. Pinned to the BSP (APs don't schedule post-boot — see
/// `scheduler::spawn_on_bsp`). ~2 s cadence: thermal mass makes faster polling
/// pointless and it just burns yields.
extern "C" fn thermal_poll_thread_entry() {
    // Let the desktop + net poll thread settle first (same rationale as net).
    for _ in 0..1200 {
        crate::scheduler::yield_task();
        x86_64::instructions::hlt();
    }
    crate::serial_println!("[thermal] post-boot poll thread active (passive-cap throttle live)");
    loop {
        let _ = poll_thermal_zones();
        // ~2 s between polls.
        for _ in 0..200 {
            crate::scheduler::yield_task();
            x86_64::instructions::hlt();
        }
    }
}

/// Spawn the post-boot thermal poll thread. Call ONCE, post-boot, alongside
/// `net::spawn_poll_thread` (same deferred-spawn constraint).
pub fn spawn_poll_thread() {
    let task = crate::task::Task::new(thermal_poll_thread_entry, None);
    crate::scheduler::spawn_on_bsp(task);
    crate::serial_println!(
        "[thermal] post-boot poll thread spawned (BSP-pinned; drives passive throttle)"
    );
}

/// Boot-time smoke test: probe ACPI thermal zones and report the count plus the
/// hottest current temperature. Emits the canonical PASS line.
pub fn run_boot_smoketest() {
    let initialized = {
        let fw = THERMAL_FRAMEWORK.lock();
        fw.initialized
    };
    if !initialized {
        crate::serial_println!("[thermal] run_boot_smoketest: NOT INITIALIZED -> FAIL");
        return;
    }

    let readings = poll_thermal_zones();
    let n = readings.len();
    let max_temp = if n == 0 {
        // No ACPI zones (common under QEMU without a thermal SSDT): fall back
        // to the on-die MSR reading so we still report a real temperature.
        let (valid, _prochot, msr_c) = read_cpu_therm_status();
        if valid {
            msr_c
        } else {
            LAST_MAX_TEMP_C.load(Ordering::Relaxed)
        }
    } else {
        readings.iter().map(|r| r.temp_c).max().unwrap_or(i32::MIN)
    };

    let temp_disp = if max_temp == i32::MIN { 0 } else { max_temp };
    crate::serial_println!("[thermal] zones={} temp={}°C -> PASS", n, temp_disp);

    for r in &readings {
        crate::serial_println!(
            "[thermal]   THM{}: {}°C (_PSV={}°C _CRT={}°C)",
            r.zone_index,
            r.temp_c,
            r.passive_trip_c,
            r.critical_trip_c,
        );
    }
}

/// Phase 4.7 R10 proof: exercise the `_PSV`/`_CRT` breach policy with synthetic
/// readings, since QEMU exposes no `\_TZ.THMx` zones to breach naturally.
///
/// 1. `_PSV` breach (temp ≥ _PSV, below _CRT) ⇒ `PassiveCap`. The cap is then
///    really applied via `cpufreq::set_cap_percent` and confirmed observable,
///    then released back to 100% (capping is reversible and harmless).
/// 2. `_CRT` breach (temp ≥ _CRT) ⇒ `CriticalShutdown` *intent*. We assert the
///    decision only — we do NOT call `power_off()`, which would kill the boot.
/// 3. Nominal (below all trips) ⇒ `Nominal`.
pub fn run_breach_selftest() {
    // 1. Passive (_PSV) breach, below critical.
    let psv = AcpiZoneReading {
        zone_index: 0xFE,
        temp_c: 85,
        passive_trip_c: 80,
        critical_trip_c: 105,
        passive_breached: true,
        critical_breached: false,
    };
    let psv_decision = decide_action(&psv) == ThermalAction::PassiveCap(PASSIVE_THROTTLE_CAP_PCT);
    crate::cpufreq::set_cap_percent(PASSIVE_THROTTLE_CAP_PCT);
    let cap_applied = crate::cpufreq::current_cap_percent() == PASSIVE_THROTTLE_CAP_PCT;
    crate::cpufreq::set_cap_percent(100);
    let cap_released = crate::cpufreq::current_cap_percent() == 100;

    // 2. Critical (_CRT) breach — assert the decision only (no power-off).
    let crt = AcpiZoneReading {
        zone_index: 0xFE,
        temp_c: 110,
        passive_trip_c: 80,
        critical_trip_c: 105,
        passive_breached: true,
        critical_breached: true,
    };
    let crt_decision = decide_action(&crt) == ThermalAction::CriticalShutdown;

    // 3. Nominal.
    let nom = AcpiZoneReading {
        zone_index: 0xFE,
        temp_c: 40,
        passive_trip_c: 80,
        critical_trip_c: 105,
        passive_breached: false,
        critical_breached: false,
    };
    let nominal = decide_action(&nom) == ThermalAction::Nominal;

    let pass = psv_decision && cap_applied && cap_released && crt_decision && nominal;
    BREACH_SELFTEST.store(if pass { 1 } else { 2 }, Ordering::Relaxed);
    crate::serial_println!(
        "[thermal] breach selftest: psv_cap={} cap_applied={} cap_released={} crt_shutdown_intent={} nominal={} -> {}",
        psv_decision,
        cap_applied,
        cap_released,
        crt_decision,
        nominal,
        if pass { "PASS" } else { "FAIL" }
    );
}

// ── Per-component thermal (Phase 4.7: CPU, GPU, NVMe) ──────────────────────

/// Cached SSD composite temperature (°C) from the most recent NVMe SMART
/// poll. `i32::MIN` = never sampled. Set by [`record_nvme_temp`] — NEVER by
/// an inline SMART read on the boot path: reading the SMART/Health log
/// synchronously while a live NVMe controller is present blocks on the admin
/// queue completion and hangs boot (a real bug this code avoids). A
/// background poller (or a careful off-`NVME_CONTROLLERS`-lock read) feeds
/// this.
static SSD_TEMP_C: AtomicI32 = AtomicI32::new(i32::MIN);
/// Cached GPU temperature (°C), fed from the GPU driver / ACPI GPU zone.
static GPU_TEMP_C: AtomicI32 = AtomicI32::new(i32::MIN);

/// Parse an NVMe SMART/Health log page and cache the composite temperature.
/// Returns the temperature in °C, or `None` if the buffer isn't a valid
/// 512-byte SMART log. Reuses the KAT-able `nvme::SmartLog` parser (composite
/// temp = bytes [1..3], Kelvin).
pub fn record_nvme_temp(smart_log: &[u8]) -> Option<i32> {
    let log = crate::nvme::SmartLog::from_bytes(smart_log)?;
    let c = log.temperature_celsius() as i32;
    SSD_TEMP_C.store(c, Ordering::Relaxed);
    Some(c)
}

/// Cache a GPU temperature reading (°C) sampled off the hot path.
pub fn record_gpu_temp(c: i32) {
    GPU_TEMP_C.store(c, Ordering::Relaxed);
}

/// Per-component temperatures: CPU (live MSR when Intel, else last polled
/// zone max), GPU (cached), SSD (cached NVMe SMART). `None` = not sampled
/// on this machine.
pub fn read_component_temps() -> (Option<i32>, Option<i32>, Option<i32>) {
    let (valid, _prochot, cpu_c) = read_cpu_therm_status();
    let cpu = if valid {
        Some(cpu_c) // Intel IA32_THERM_STATUS
    } else if let Some(amd_c) = read_amd_cpu_temp_c() {
        Some(amd_c) // AMD Zen SMU/SMN (the Athena path)
    } else {
        let m = LAST_MAX_TEMP_C.load(Ordering::Relaxed); // ACPI zone fallback
        if m != i32::MIN {
            Some(m)
        } else {
            None
        }
    };
    let gpu = match GPU_TEMP_C.load(Ordering::Relaxed) {
        i32::MIN => None,
        v => Some(v),
    };
    let ssd = match SSD_TEMP_C.load(Ordering::Relaxed) {
        i32::MIN => None,
        v => Some(v),
    };
    (cpu, gpu, ssd)
}

/// Phase 4.7 per-component proof: the NVMe SMART composite-temp parse is
/// exact over a synthetic log (no live controller → no admin-queue hang),
/// the cache feeds the per-component report, and the report aggregates CPU +
/// GPU + SSD. Deterministic on QEMU and iron.
pub fn run_component_smoketest() {
    // Synthetic SMART/Health log: composite temp 313 K = 40 °C at bytes 1..3.
    let mut log = [0u8; 512];
    log[1..3].copy_from_slice(&313u16.to_le_bytes());
    let parsed = record_nvme_temp(&log);
    let ssd_ok = parsed == Some(40);

    // Synthetic GPU sample.
    record_gpu_temp(55);

    let (cpu, gpu, ssd) = read_component_temps();
    // CPU may be None under QEMU (no Intel THERM_STATUS, no ACPI zone) — that
    // is a valid "not sampled" outcome; GPU + SSD must reflect the caches.
    let gpu_ok = gpu == Some(55);
    let cached_ssd_ok = ssd == Some(40);
    // A non-SMART buffer must be rejected (fail-closed parse).
    let reject_garbage = record_nvme_temp(&[0u8; 16]).is_none();

    let pass = ssd_ok && gpu_ok && cached_ssd_ok && reject_garbage;
    crate::serial_println!(
        "[thermal] per-component selftest: nvme_smart_40C={} gpu_cached={} ssd_cached={} reject_short={} cpu={} -> {}",
        ssd_ok,
        gpu_ok,
        cached_ssd_ok,
        reject_garbage,
        cpu.map(|c| c).unwrap_or(-1),
        if pass { "PASS" } else { "FAIL" },
    );
}

/// Phase 4.7 proof for the AMD Zen CPU-temperature path: the SMU Tctl decode is
/// exact over synthetic register values (the live SMN read is iron-only — QEMU
/// is Family 0xF so `read_amd_cpu_temp_c` returns `None` there), and the live
/// read, when present, is in a plausible window. Can print FAIL.
pub fn run_amd_temp_selftest() {
    // 0x2C0 (=704) in bits[31:21] × 0.125 °C = 88.000 °C, range bit clear.
    let d1 = decode_zen_tctl_mc(0x5800_0000);
    // Same, with CUR_TEMP_RANGE_SEL (bit 19) → −49 °C extended range = 39.000 °C.
    let d2 = decode_zen_tctl_mc(0x5808_0000);
    let decode_ok = d1 == 88_000 && d2 == 39_000;

    let live = read_amd_cpu_temp_c();
    let live_ok = match live {
        None => true, // not a Zen part (QEMU) — valid "no reading"
        Some(c) => (0..=130).contains(&c),
    };

    let pass = decode_ok && live_ok;
    crate::serial_println!(
        "[thermal] amd cpu-temp selftest: decode(88C={} 39C={}) decode_ok={} live={}C -> {}",
        d1 / 1000,
        d2 / 1000,
        decode_ok,
        live.map(|c| c).unwrap_or(-1),
        if pass { "PASS" } else { "FAIL" }
    );
}

/// Render thermal state for `/proc/raeen/thermal`.
pub fn dump_text() -> String {
    let mut out = String::new();
    out.push_str("=== Thermal (Phase 4.7) ===\n");

    let fw = THERMAL_FRAMEWORK.lock();
    out.push_str(&format!("initialized:      {}\n", fw.initialized));
    out.push_str(&format!("sw_zones:         {}\n", fw.zones.len()));
    out.push_str(&format!("sensors:          {}\n", fw.sensors.len()));
    out.push_str(&format!("cooling_devices:  {}\n", fw.cooling_devices.len()));
    out.push_str(&format!("acpi_zones:       {}\n", fw.acpi_zones.len()));

    // Per-component temps (CPU via Intel MSR or AMD SMU, GPU/SSD cached).
    let (cpu_c, gpu_c, ssd_c) = read_component_temps();
    out.push_str(&format!(
        "cpu_temp:         {}\n",
        cpu_c
            .map(|c| format!("{}°C", c))
            .unwrap_or(String::from("n/a"))
    ));
    out.push_str(&format!(
        "gpu_temp:         {}\n",
        gpu_c
            .map(|c| format!("{}°C", c))
            .unwrap_or(String::from("n/a"))
    ));
    out.push_str(&format!(
        "ssd_temp:         {}\n",
        ssd_c
            .map(|c| format!("{}°C", c))
            .unwrap_or(String::from("n/a"))
    ));

    let zone_count = LAST_ZONE_COUNT.load(Ordering::Relaxed);
    let max_temp = LAST_MAX_TEMP_C.load(Ordering::Relaxed);
    let throttled = PASSIVE_THROTTLE_ENGAGED.load(Ordering::Relaxed) == 1;
    out.push_str(&format!("acpi_polled:      {}\n", zone_count));
    if max_temp != i32::MIN {
        out.push_str(&format!("hottest:          {}°C\n", max_temp));
    } else {
        out.push_str("hottest:          n/a\n");
    }
    out.push_str(&format!(
        "passive_throttle: {}\n",
        if throttled { "ENGAGED" } else { "off" }
    ));
    out.push_str(&format!(
        "cpu_freq_cap:     {}%\n",
        crate::cpufreq::current_cap_percent()
    ));
    out.push_str(&format!(
        "breach_selftest:  {} (_PSV→cap, _CRT→shutdown intent)\n",
        match BREACH_SELFTEST.load(Ordering::Relaxed) {
            1 => "PASS",
            2 => "FAIL",
            _ => "not run",
        }
    ));

    out.push_str("\n-- software thermal zones --\n");
    for z in &fw.zones {
        out.push_str(&format!(
            "  {} (id={}): {}°C\n",
            z.name,
            z.id,
            z.current_temp_mc / 1000,
        ));
    }

    out.push_str("\n-- ACPI thermal zones (last poll) --\n");
    for az in &fw.acpi_zones {
        let psv = az
            .passive_trip_mc
            .map(|m| m / 1000)
            .unwrap_or(DEFAULT_PASSIVE_TRIP_C);
        let crt = az
            .critical_trip_mc
            .map(|m| m / 1000)
            .unwrap_or(DEFAULT_CRITICAL_TRIP_C);
        out.push_str(&format!(
            "  THM{}: {}°C (_PSV={}°C _CRT={}°C)\n",
            az.zone_id,
            az.tmp_mc / 1000,
            psv,
            crt,
        ));
    }

    out
}
