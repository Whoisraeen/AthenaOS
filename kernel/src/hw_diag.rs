//! Built-in hardware diagnostics — temps, voltages, fan curves, SMART data,
//! power draw monitoring. Replaces HWiNFO / HWMonitor / Open Hardware Monitor.
//!
//! Provides a unified sensor interface for RaeShell's system monitor widget.

#![allow(dead_code)]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use spin::Mutex;

// ═══════════════════════════════════════════════════════════════════════════════
//  Constants
// ═══════════════════════════════════════════════════════════════════════════════

const POLL_INTERVAL_DEFAULT_MS: u32 = 1000;
const HISTORY_RING_SIZE: usize = 3600; // 1 hour at 1 sample/s
const MAX_SENSORS: usize = 64;
const MAX_ALERT_THRESHOLDS: usize = 32;

// Intel RAPL MSRs
const MSR_RAPL_POWER_UNIT: u32 = 0x606;
const MSR_PKG_ENERGY_STATUS: u32 = 0x611;
const MSR_PP0_ENERGY_STATUS: u32 = 0x639;
const MSR_DRAM_ENERGY_STATUS: u32 = 0x619;

// Thermal MSRs
const IA32_THERM_STATUS: u32 = 0x19C;
const IA32_PACKAGE_THERM_STATUS: u32 = 0x1B1;
const MSR_TEMPERATURE_TARGET: u32 = 0x1A2;

// Super I/O chip registers (common chips: IT8728F, NCT6775, W83627)
const SUPERIO_INDEX_PORT: u16 = 0x2E;
const SUPERIO_DATA_PORT: u16 = 0x2F;
const SUPERIO_ENTER_KEY: u8 = 0x87;
const SUPERIO_EXIT_KEY: u8 = 0xAA;

// ═══════════════════════════════════════════════════════════════════════════════
//  Sensor Types & IDs
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SensorCategory {
    Temperature,
    Voltage,
    Fan,
    Power,
    Storage,
    Frequency,
    Utilization,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SensorLocation {
    CpuPackage,
    CpuCore(u8),
    CpuVrm,
    GpuCore,
    GpuMemory,
    GpuVrm,
    NvmeDrive(u8),
    SataDrive(u8),
    Chipset,
    Ambient,
    DramModule(u8),
    Psu12V,
    Psu5V,
    Psu3V3,
    PsuVcore,
    PsuVdimm,
    CaseFan(u8),
    CpuFan,
    GpuFan(u8),
    PumpFan,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SensorId {
    pub category: SensorCategory,
    pub location: SensorLocation,
    pub index: u8,
}

// ═══════════════════════════════════════════════════════════════════════════════
//  1. Temperature Sensor
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy)]
pub struct TemperatureReading {
    pub current_mc: i32, // millidegrees Celsius
    pub min_mc: i32,
    pub max_mc: i32,
    pub critical_mc: i32,
    pub throttle_mc: i32,
}

pub struct TemperatureSensor {
    pub location: SensorLocation,
    pub name: String,
    pub current_mc: i32,
    pub min_mc: i32,
    pub max_mc: i32,
    pub critical_mc: i32,
    pub throttle_mc: i32,
    pub history: RingBuffer<i32>,
    pub tj_max: i32,
    pub valid: bool,
}

impl TemperatureSensor {
    pub fn new(location: SensorLocation, name: String, critical_mc: i32) -> Self {
        Self {
            location,
            name,
            current_mc: 0,
            min_mc: i32::MAX,
            max_mc: i32::MIN,
            critical_mc,
            throttle_mc: critical_mc - 10_000,
            history: RingBuffer::new(0),
            tj_max: 100_000,
            valid: false,
        }
    }

    pub fn update(&mut self, temp_mc: i32) {
        self.current_mc = temp_mc;
        if temp_mc < self.min_mc {
            self.min_mc = temp_mc;
        }
        if temp_mc > self.max_mc {
            self.max_mc = temp_mc;
        }
        self.history.push(temp_mc);
        self.valid = true;
    }

    pub fn is_critical(&self) -> bool {
        self.current_mc >= self.critical_mc
    }

    pub fn is_throttling(&self) -> bool {
        self.current_mc >= self.throttle_mc
    }

    pub fn reading(&self) -> TemperatureReading {
        TemperatureReading {
            current_mc: self.current_mc,
            min_mc: self.min_mc,
            max_mc: self.max_mc,
            critical_mc: self.critical_mc,
            throttle_mc: self.throttle_mc,
        }
    }

    pub fn read_cpu_package_temp(tj_max: i32) -> i32 {
        let therm_status = unsafe { read_msr_diag(IA32_PACKAGE_THERM_STATUS) };
        let digital_readout = ((therm_status >> 16) & 0x7F) as i32;
        (tj_max - digital_readout) * 1000 // convert to millidegrees
    }

    pub fn read_cpu_core_temp(tj_max: i32) -> i32 {
        let therm_status = unsafe { read_msr_diag(IA32_THERM_STATUS) };
        let digital_readout = ((therm_status >> 16) & 0x7F) as i32;
        (tj_max - digital_readout) * 1000
    }

    pub fn read_tj_max() -> i32 {
        let target = unsafe { read_msr_diag(MSR_TEMPERATURE_TARGET) };
        ((target >> 16) & 0xFF) as i32
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
//  2. Voltage Sensor
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy)]
pub struct VoltageReading {
    pub current_mv: u32,
    pub min_mv: u32,
    pub max_mv: u32,
    pub nominal_mv: u32,
}

pub struct VoltageSensor {
    pub location: SensorLocation,
    pub name: String,
    pub current_mv: u32,
    pub min_mv: u32,
    pub max_mv: u32,
    pub nominal_mv: u32,
    pub low_threshold_mv: u32,
    pub high_threshold_mv: u32,
    pub history: RingBuffer<u32>,
    pub valid: bool,
}

impl VoltageSensor {
    pub fn new(location: SensorLocation, name: String, nominal_mv: u32) -> Self {
        let tolerance = nominal_mv / 20; // 5% tolerance
        Self {
            location,
            name,
            current_mv: 0,
            min_mv: u32::MAX,
            max_mv: 0,
            nominal_mv,
            low_threshold_mv: nominal_mv.saturating_sub(tolerance),
            high_threshold_mv: nominal_mv + tolerance,
            history: RingBuffer::new(0),
            valid: false,
        }
    }

    pub fn update(&mut self, voltage_mv: u32) {
        self.current_mv = voltage_mv;
        if voltage_mv < self.min_mv {
            self.min_mv = voltage_mv;
        }
        if voltage_mv > self.max_mv {
            self.max_mv = voltage_mv;
        }
        self.history.push(voltage_mv);
        self.valid = true;
    }

    pub fn is_out_of_range(&self) -> bool {
        self.current_mv < self.low_threshold_mv || self.current_mv > self.high_threshold_mv
    }

    pub fn reading(&self) -> VoltageReading {
        VoltageReading {
            current_mv: self.current_mv,
            min_mv: self.min_mv,
            max_mv: self.max_mv,
            nominal_mv: self.nominal_mv,
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
//  3. Fan Sensor
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy)]
pub struct FanReading {
    pub rpm: u32,
    pub min_rpm: u32,
    pub max_rpm: u32,
    pub duty_percent: u8,
}

pub struct FanSensor {
    pub location: SensorLocation,
    pub name: String,
    pub current_rpm: u32,
    pub min_rpm: u32,
    pub max_rpm: u32,
    pub duty_percent: u8,
    pub max_duty: u8,
    pub stall_threshold_rpm: u32,
    pub history: RingBuffer<u32>,
    pub valid: bool,
}

impl FanSensor {
    pub fn new(location: SensorLocation, name: String) -> Self {
        Self {
            location,
            name,
            current_rpm: 0,
            min_rpm: u32::MAX,
            max_rpm: 0,
            duty_percent: 0,
            max_duty: 100,
            stall_threshold_rpm: 200,
            history: RingBuffer::new(0),
            valid: false,
        }
    }

    pub fn update(&mut self, rpm: u32, duty: u8) {
        self.current_rpm = rpm;
        self.duty_percent = duty;
        if rpm > 0 && rpm < self.min_rpm {
            self.min_rpm = rpm;
        }
        if rpm > self.max_rpm {
            self.max_rpm = rpm;
        }
        self.history.push(rpm);
        self.valid = true;
    }

    pub fn is_stalled(&self) -> bool {
        self.duty_percent > 0 && self.current_rpm < self.stall_threshold_rpm
    }

    pub fn reading(&self) -> FanReading {
        FanReading {
            rpm: self.current_rpm,
            min_rpm: self.min_rpm,
            max_rpm: self.max_rpm,
            duty_percent: self.duty_percent,
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
//  4. Power Sensor (RAPL)
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerDomain {
    CpuPackage,
    CpuCores,
    Dram,
    GpuPackage,
    SystemTotal,
}

#[derive(Debug, Clone, Copy)]
pub struct PowerReading {
    pub current_mw: u32,
    pub min_mw: u32,
    pub max_mw: u32,
    pub energy_uj: u64,
    pub tdp_mw: u32,
}

pub struct PowerSensor {
    pub domain: PowerDomain,
    pub name: String,
    pub current_mw: u32,
    pub min_mw: u32,
    pub max_mw: u32,
    pub energy_uj: u64,
    pub tdp_mw: u32,
    pub last_energy_counter: u64,
    pub last_read_tsc: u64,
    pub energy_unit_divisor: u64,
    pub history: RingBuffer<u32>,
    pub valid: bool,
}

impl PowerSensor {
    pub fn new(domain: PowerDomain, name: String, tdp_mw: u32) -> Self {
        Self {
            domain,
            name,
            current_mw: 0,
            min_mw: u32::MAX,
            max_mw: 0,
            energy_uj: 0,
            tdp_mw,
            last_energy_counter: 0,
            last_read_tsc: 0,
            energy_unit_divisor: 1,
            history: RingBuffer::new(0),
            valid: false,
        }
    }

    pub fn init_rapl_units(&mut self) {
        let units = unsafe { read_msr_diag(MSR_RAPL_POWER_UNIT) };
        let energy_unit = (units >> 8) & 0x1F;
        self.energy_unit_divisor = 1u64 << energy_unit;
    }

    pub fn update_from_rapl(&mut self, msr_addr: u32, tsc_freq_khz: u64) {
        let now_tsc = read_tsc_diag();
        let raw_energy = unsafe { read_msr_diag(msr_addr) } & 0xFFFF_FFFF;

        if self.last_energy_counter > 0 {
            let delta = raw_energy.wrapping_sub(self.last_energy_counter) & 0xFFFF_FFFF;
            let energy_uj = (delta * 1_000_000) / self.energy_unit_divisor;
            self.energy_uj += energy_uj;

            let dt_tsc = now_tsc.saturating_sub(self.last_read_tsc);
            if dt_tsc > 0 && tsc_freq_khz > 0 {
                let dt_us = dt_tsc / (tsc_freq_khz / 1000);
                if dt_us > 0 {
                    let power_mw = (energy_uj * 1000 / dt_us) as u32;
                    self.current_mw = power_mw;
                    if power_mw > 0 && power_mw < self.min_mw {
                        self.min_mw = power_mw;
                    }
                    if power_mw > self.max_mw {
                        self.max_mw = power_mw;
                    }
                    self.history.push(power_mw);
                }
            }
        }

        self.last_energy_counter = raw_energy;
        self.last_read_tsc = now_tsc;
        self.valid = true;
    }

    pub fn is_exceeding_tdp(&self) -> bool {
        self.current_mw > self.tdp_mw
    }

    pub fn reading(&self) -> PowerReading {
        PowerReading {
            current_mw: self.current_mw,
            min_mw: self.min_mw,
            max_mw: self.max_mw,
            energy_uj: self.energy_uj,
            tdp_mw: self.tdp_mw,
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
//  5. SMART Data (NVMe / SATA health)
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriveType {
    Nvme,
    Sata,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriveHealth {
    Good,
    Warning,
    Critical,
    Unknown,
}

#[derive(Debug, Clone)]
pub struct SmartData {
    pub drive_index: u8,
    pub drive_type: DriveType,
    pub model: String,
    pub serial: String,
    pub firmware_rev: String,
    pub capacity_gb: u32,
    pub temperature_c: u8,
    pub power_on_hours: u64,
    pub power_cycles: u64,
    pub total_bytes_written: u64,
    pub total_bytes_read: u64,
    pub wear_level_percent: u8,
    pub available_spare_percent: u8,
    pub critical_warning: u8,
    pub health: DriveHealth,
    pub media_errors: u64,
    pub unsafe_shutdowns: u64,
}

impl SmartData {
    pub fn new(index: u8, drive_type: DriveType) -> Self {
        Self {
            drive_index: index,
            drive_type,
            model: String::new(),
            serial: String::new(),
            firmware_rev: String::new(),
            capacity_gb: 0,
            temperature_c: 0,
            power_on_hours: 0,
            power_cycles: 0,
            total_bytes_written: 0,
            total_bytes_read: 0,
            wear_level_percent: 100,
            available_spare_percent: 100,
            critical_warning: 0,
            health: DriveHealth::Unknown,
            media_errors: 0,
            unsafe_shutdowns: 0,
        }
    }

    pub fn evaluate_health(&mut self) {
        if self.critical_warning != 0 || self.media_errors > 100 {
            self.health = DriveHealth::Critical;
        } else if self.wear_level_percent < 20
            || self.available_spare_percent < 20
            || self.temperature_c > 70
        {
            self.health = DriveHealth::Warning;
        } else {
            self.health = DriveHealth::Good;
        }
    }

    pub fn total_writes_tb(&self) -> u64 {
        self.total_bytes_written / (1024 * 1024 * 1024 * 1024)
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
//  6. Alert Threshold System
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlertSeverity {
    Info,
    Warning,
    Critical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlertCondition {
    Above(i64),
    Below(i64),
    OutOfRange(i64, i64),
}

#[derive(Debug, Clone)]
pub struct AlertThreshold {
    pub id: u32,
    pub sensor_id: SensorId,
    pub condition: AlertCondition,
    pub severity: AlertSeverity,
    pub triggered: bool,
    pub trigger_count: u64,
    pub last_trigger_tsc: u64,
    pub cooldown_ms: u32,
}

impl AlertThreshold {
    pub fn check(&mut self, value: i64, now_tsc: u64, tsc_freq_khz: u64) -> bool {
        let condition_met = match self.condition {
            AlertCondition::Above(threshold) => value > threshold,
            AlertCondition::Below(threshold) => value < threshold,
            AlertCondition::OutOfRange(low, high) => value < low || value > high,
        };

        if !condition_met {
            self.triggered = false;
            return false;
        }

        // Cooldown check
        if self.triggered && self.cooldown_ms > 0 {
            let elapsed_tsc = now_tsc.saturating_sub(self.last_trigger_tsc);
            let cooldown_tsc = self.cooldown_ms as u64 * tsc_freq_khz;
            if elapsed_tsc < cooldown_tsc {
                return false;
            }
        }

        self.triggered = true;
        self.trigger_count += 1;
        self.last_trigger_tsc = now_tsc;
        true
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
//  7. Ring Buffer (generic, for sensor history)
// ═══════════════════════════════════════════════════════════════════════════════

pub struct RingBuffer<T: Copy + Default> {
    data: Vec<T>,
    write_index: usize,
    count: usize,
    capacity: usize,
}

impl<T: Copy + Default> RingBuffer<T> {
    pub fn new(default: T) -> Self {
        let mut data = Vec::with_capacity(HISTORY_RING_SIZE);
        for _ in 0..HISTORY_RING_SIZE {
            data.push(default);
        }
        Self {
            data,
            write_index: 0,
            count: 0,
            capacity: HISTORY_RING_SIZE,
        }
    }

    pub fn push(&mut self, value: T) {
        self.data[self.write_index] = value;
        self.write_index = (self.write_index + 1) % self.capacity;
        if self.count < self.capacity {
            self.count += 1;
        }
    }

    pub fn latest(&self) -> Option<T> {
        if self.count == 0 {
            return None;
        }
        let idx = if self.write_index == 0 {
            self.capacity - 1
        } else {
            self.write_index - 1
        };
        Some(self.data[idx])
    }

    pub fn average_last_n(&self, n: usize) -> Option<T>
    where
        T: Into<i64> + From<i32>,
    {
        let count = n.min(self.count);
        if count == 0 {
            return None;
        }
        let mut sum: i64 = 0;
        for i in 0..count {
            let idx = (self.write_index + self.capacity - 1 - i) % self.capacity;
            sum += self.data[idx].into();
        }
        Some(T::from((sum / count as i64) as i32))
    }

    pub fn len(&self) -> usize {
        self.count
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
//  8. Hardware Monitor (unified interface)
// ═══════════════════════════════════════════════════════════════════════════════

pub struct HwMonitor {
    pub temp_sensors: Vec<TemperatureSensor>,
    pub voltage_sensors: Vec<VoltageSensor>,
    pub fan_sensors: Vec<FanSensor>,
    pub power_sensors: Vec<PowerSensor>,
    pub smart_data: Vec<SmartData>,
    pub alerts: Vec<AlertThreshold>,
    pub next_alert_id: u32,
    pub poll_interval_ms: u32,
    pub last_poll_tsc: u64,
    pub tsc_freq_khz: u64,
    pub poll_count: u64,
    pub superio_base: u16,
    pub superio_detected: bool,
    pub initialized: bool,
}

impl HwMonitor {
    pub const fn new() -> Self {
        Self {
            temp_sensors: Vec::new(),
            voltage_sensors: Vec::new(),
            fan_sensors: Vec::new(),
            power_sensors: Vec::new(),
            smart_data: Vec::new(),
            alerts: Vec::new(),
            next_alert_id: 1,
            poll_interval_ms: POLL_INTERVAL_DEFAULT_MS,
            last_poll_tsc: 0,
            tsc_freq_khz: 3_000_000,
            poll_count: 0,
            superio_base: SUPERIO_INDEX_PORT,
            superio_detected: false,
            initialized: false,
        }
    }

    pub fn detect_superio(&mut self) -> bool {
        unsafe {
            // Try entering Super I/O configuration mode
            port_write_u8(SUPERIO_INDEX_PORT, SUPERIO_ENTER_KEY);
            port_write_u8(SUPERIO_INDEX_PORT, SUPERIO_ENTER_KEY);

            // Read chip ID
            port_write_u8(SUPERIO_INDEX_PORT, 0x20);
            let chip_id_hi = port_read_u8(SUPERIO_DATA_PORT);
            port_write_u8(SUPERIO_INDEX_PORT, 0x21);
            let chip_id_lo = port_read_u8(SUPERIO_DATA_PORT);

            // Exit config mode
            port_write_u8(SUPERIO_INDEX_PORT, SUPERIO_EXIT_KEY);

            let chip_id = ((chip_id_hi as u16) << 8) | chip_id_lo as u16;
            self.superio_detected = chip_id != 0xFFFF && chip_id != 0x0000;
        }
        self.superio_detected
    }

    pub fn add_temp_sensor(&mut self, location: SensorLocation, name: String, critical_mc: i32) {
        self.temp_sensors
            .push(TemperatureSensor::new(location, name, critical_mc));
    }

    pub fn add_voltage_sensor(&mut self, location: SensorLocation, name: String, nominal_mv: u32) {
        self.voltage_sensors
            .push(VoltageSensor::new(location, name, nominal_mv));
    }

    pub fn add_fan_sensor(&mut self, location: SensorLocation, name: String) {
        self.fan_sensors.push(FanSensor::new(location, name));
    }

    pub fn add_power_sensor(&mut self, domain: PowerDomain, name: String, tdp_mw: u32) {
        let mut sensor = PowerSensor::new(domain, name, tdp_mw);
        sensor.init_rapl_units();
        self.power_sensors.push(sensor);
    }

    pub fn add_smart_device(&mut self, index: u8, drive_type: DriveType) {
        self.smart_data.push(SmartData::new(index, drive_type));
    }

    pub fn add_alert(
        &mut self,
        sensor_id: SensorId,
        condition: AlertCondition,
        severity: AlertSeverity,
        cooldown_ms: u32,
    ) -> u32 {
        let id = self.next_alert_id;
        self.next_alert_id += 1;
        self.alerts.push(AlertThreshold {
            id,
            sensor_id,
            condition,
            severity,
            triggered: false,
            trigger_count: 0,
            last_trigger_tsc: 0,
            cooldown_ms,
        });
        id
    }

    pub fn remove_alert(&mut self, id: u32) {
        self.alerts.retain(|a| a.id != id);
    }

    /// Main polling loop — reads all sensors.
    pub fn poll(&mut self) {
        let now = read_tsc_diag();

        // Check if enough time has elapsed
        let interval_tsc = self.poll_interval_ms as u64 * self.tsc_freq_khz;
        if now.saturating_sub(self.last_poll_tsc) < interval_tsc {
            return;
        }
        self.last_poll_tsc = now;
        self.poll_count += 1;

        self.poll_temperatures();
        self.poll_power();
    }

    fn poll_temperatures(&mut self) {
        let tj_max = TemperatureSensor::read_tj_max();

        for sensor in &mut self.temp_sensors {
            let temp = match sensor.location {
                SensorLocation::CpuPackage => TemperatureSensor::read_cpu_package_temp(tj_max),
                SensorLocation::CpuCore(_) => TemperatureSensor::read_cpu_core_temp(tj_max),
                _ => sensor.current_mc, // retain last value for unreadable sensors
            };
            sensor.update(temp);
        }
    }

    fn poll_power(&mut self) {
        for sensor in &mut self.power_sensors {
            let msr = match sensor.domain {
                PowerDomain::CpuPackage => MSR_PKG_ENERGY_STATUS,
                PowerDomain::CpuCores => MSR_PP0_ENERGY_STATUS,
                PowerDomain::Dram => MSR_DRAM_ENERGY_STATUS,
                _ => continue,
            };
            sensor.update_from_rapl(msr, self.tsc_freq_khz);
        }
    }

    /// Check all alert thresholds and return triggered alerts.
    pub fn check_alerts(&mut self) -> Vec<(u32, AlertSeverity)> {
        let now = read_tsc_diag();
        let tsc_freq = self.tsc_freq_khz;
        let mut triggered = Vec::new();

        // Collect sensor values first to avoid borrow conflict
        let values: Vec<(usize, Option<i64>)> = self
            .alerts
            .iter()
            .enumerate()
            .map(|(i, a)| (i, self.get_sensor_value(&a.sensor_id)))
            .collect();

        for (i, value) in values {
            if let Some(v) = value {
                if self.alerts[i].check(v, now, tsc_freq) {
                    triggered.push((self.alerts[i].id, self.alerts[i].severity));
                }
            }
        }

        triggered
    }

    fn get_sensor_value(&self, sensor_id: &SensorId) -> Option<i64> {
        match sensor_id.category {
            SensorCategory::Temperature => self
                .temp_sensors
                .get(sensor_id.index as usize)
                .map(|s| s.current_mc as i64),
            SensorCategory::Voltage => self
                .voltage_sensors
                .get(sensor_id.index as usize)
                .map(|s| s.current_mv as i64),
            SensorCategory::Fan => self
                .fan_sensors
                .get(sensor_id.index as usize)
                .map(|s| s.current_rpm as i64),
            SensorCategory::Power => self
                .power_sensors
                .get(sensor_id.index as usize)
                .map(|s| s.current_mw as i64),
            _ => None,
        }
    }

    pub fn set_poll_interval(&mut self, ms: u32) {
        self.poll_interval_ms = ms.max(100).min(10_000);
    }

    pub fn reset_min_max(&mut self) {
        for s in &mut self.temp_sensors {
            s.min_mc = s.current_mc;
            s.max_mc = s.current_mc;
        }
        for s in &mut self.voltage_sensors {
            s.min_mv = s.current_mv;
            s.max_mv = s.current_mv;
        }
        for s in &mut self.fan_sensors {
            s.min_rpm = s.current_rpm;
            s.max_rpm = s.current_rpm;
        }
        for s in &mut self.power_sensors {
            s.min_mw = s.current_mw;
            s.max_mw = s.current_mw;
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
//  9. Export Format (for RaeShell widget)
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct SensorSnapshot {
    pub timestamp_ms: u64,
    pub cpu_temp_mc: i32,
    pub gpu_temp_mc: i32,
    pub cpu_power_mw: u32,
    pub gpu_power_mw: u32,
    pub cpu_fan_rpm: u32,
    pub gpu_fan_rpm: u32,
    pub vcore_mv: u32,
    pub dram_voltage_mv: u32,
    pub nvme_temp_c: u8,
    pub cpu_freq_mhz: u32,
}

impl HwMonitor {
    pub fn snapshot(&self) -> SensorSnapshot {
        let cpu_temp = self
            .temp_sensors
            .iter()
            .find(|s| matches!(s.location, SensorLocation::CpuPackage))
            .map(|s| s.current_mc)
            .unwrap_or(0);

        let gpu_temp = self
            .temp_sensors
            .iter()
            .find(|s| matches!(s.location, SensorLocation::GpuCore))
            .map(|s| s.current_mc)
            .unwrap_or(0);

        let cpu_power = self
            .power_sensors
            .iter()
            .find(|s| s.domain == PowerDomain::CpuPackage)
            .map(|s| s.current_mw)
            .unwrap_or(0);

        let gpu_power = self
            .power_sensors
            .iter()
            .find(|s| s.domain == PowerDomain::GpuPackage)
            .map(|s| s.current_mw)
            .unwrap_or(0);

        let cpu_fan = self
            .fan_sensors
            .iter()
            .find(|s| matches!(s.location, SensorLocation::CpuFan))
            .map(|s| s.current_rpm)
            .unwrap_or(0);

        let gpu_fan = self
            .fan_sensors
            .iter()
            .find(|s| matches!(s.location, SensorLocation::GpuFan(_)))
            .map(|s| s.current_rpm)
            .unwrap_or(0);

        let vcore = self
            .voltage_sensors
            .iter()
            .find(|s| matches!(s.location, SensorLocation::PsuVcore))
            .map(|s| s.current_mv)
            .unwrap_or(0);

        let vdimm = self
            .voltage_sensors
            .iter()
            .find(|s| matches!(s.location, SensorLocation::PsuVdimm))
            .map(|s| s.current_mv)
            .unwrap_or(0);

        let nvme_temp = self
            .smart_data
            .iter()
            .find(|s| s.drive_type == DriveType::Nvme)
            .map(|s| s.temperature_c)
            .unwrap_or(0);

        SensorSnapshot {
            timestamp_ms: self.poll_count * self.poll_interval_ms as u64,
            cpu_temp_mc: cpu_temp,
            gpu_temp_mc: gpu_temp,
            cpu_power_mw: cpu_power,
            gpu_power_mw: gpu_power,
            cpu_fan_rpm: cpu_fan,
            gpu_fan_rpm: gpu_fan,
            vcore_mv: vcore,
            dram_voltage_mv: vdimm,
            nvme_temp_c: nvme_temp,
            cpu_freq_mhz: 0,
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
//  Port I/O Helpers
// ═══════════════════════════════════════════════════════════════════════════════

#[inline]
unsafe fn port_read_u8(port: u16) -> u8 {
    let val: u8;
    core::arch::asm!(
        "in al, dx",
        in("dx") port,
        out("al") val,
        options(nomem, nostack, preserves_flags),
    );
    val
}

#[inline]
unsafe fn port_write_u8(port: u16, val: u8) {
    core::arch::asm!(
        "out dx, al",
        in("dx") port,
        in("al") val,
        options(nomem, nostack, preserves_flags),
    );
}

#[inline]
unsafe fn read_msr_diag(msr: u32) -> u64 {
    crate::msr::rdmsr_safe(msr).unwrap_or(0)
}

#[inline]
fn read_tsc_diag() -> u64 {
    let lo: u32;
    let hi: u32;
    unsafe {
        core::arch::asm!(
            "rdtsc",
            out("eax") lo,
            out("edx") hi,
            options(nomem, nostack, preserves_flags),
        );
    }
    ((hi as u64) << 32) | lo as u64
}

// ═══════════════════════════════════════════════════════════════════════════════
//  Global State & Init
// ═══════════════════════════════════════════════════════════════════════════════

pub static HW_MONITOR: Mutex<HwMonitor> = Mutex::new(HwMonitor::new());

pub fn init() {
    let mut mon = HW_MONITOR.lock();

    // Detect Super I/O chip for voltage/fan readings
    mon.detect_superio();

    // Add CPU temperature sensors
    mon.add_temp_sensor(
        SensorLocation::CpuPackage,
        String::from("CPU Package"),
        105_000,
    );
    mon.add_temp_sensor(
        SensorLocation::CpuCore(0),
        String::from("CPU Core 0"),
        105_000,
    );
    mon.add_temp_sensor(
        SensorLocation::CpuCore(1),
        String::from("CPU Core 1"),
        105_000,
    );
    mon.add_temp_sensor(
        SensorLocation::CpuCore(2),
        String::from("CPU Core 2"),
        105_000,
    );
    mon.add_temp_sensor(
        SensorLocation::CpuCore(3),
        String::from("CPU Core 3"),
        105_000,
    );

    // Add GPU temperature
    mon.add_temp_sensor(SensorLocation::GpuCore, String::from("GPU Core"), 95_000);

    // Add NVMe temperature
    mon.add_temp_sensor(
        SensorLocation::NvmeDrive(0),
        String::from("NVMe SSD"),
        75_000,
    );

    // Add voltage sensors
    mon.add_voltage_sensor(SensorLocation::PsuVcore, String::from("Vcore"), 1200);
    mon.add_voltage_sensor(SensorLocation::PsuVdimm, String::from("DRAM"), 1100);
    mon.add_voltage_sensor(SensorLocation::Psu12V, String::from("+12V Rail"), 12000);
    mon.add_voltage_sensor(SensorLocation::Psu5V, String::from("+5V Rail"), 5000);
    mon.add_voltage_sensor(SensorLocation::Psu3V3, String::from("+3.3V Rail"), 3300);

    // Add fan sensors
    mon.add_fan_sensor(SensorLocation::CpuFan, String::from("CPU Fan"));
    mon.add_fan_sensor(SensorLocation::GpuFan(0), String::from("GPU Fan 1"));
    mon.add_fan_sensor(SensorLocation::CaseFan(0), String::from("Case Fan 1"));
    mon.add_fan_sensor(SensorLocation::CaseFan(1), String::from("Case Fan 2"));

    // Add power sensors (RAPL)
    mon.add_power_sensor(
        PowerDomain::CpuPackage,
        String::from("CPU Package Power"),
        125_000,
    );
    mon.add_power_sensor(
        PowerDomain::CpuCores,
        String::from("CPU Cores Power"),
        95_000,
    );
    mon.add_power_sensor(PowerDomain::Dram, String::from("DRAM Power"), 20_000);

    // Add NVMe SMART monitoring
    mon.add_smart_device(0, DriveType::Nvme);

    // Default alert: CPU over 95°C
    mon.add_alert(
        SensorId {
            category: SensorCategory::Temperature,
            location: SensorLocation::CpuPackage,
            index: 0,
        },
        AlertCondition::Above(95_000),
        AlertSeverity::Warning,
        5000,
    );

    // Default alert: CPU over 100°C (critical)
    mon.add_alert(
        SensorId {
            category: SensorCategory::Temperature,
            location: SensorLocation::CpuPackage,
            index: 0,
        },
        AlertCondition::Above(100_000),
        AlertSeverity::Critical,
        1000,
    );

    mon.initialized = true;
}
