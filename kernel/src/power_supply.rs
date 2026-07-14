//! Power Supply + Charger + Battery subsystem
//!
//! Full Linux-style power supply class: battery/charger/mains properties,
//! USB Power Delivery, fuel gauge, charger IC control, SBS protocol,
//! battery protection, sysfs integration, and event notifications.

#![allow(dead_code)]

extern crate alloc;

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use spin::Mutex;

// ---------------------------------------------------------------------------
// Power supply type
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum PowerSupplyType {
    Battery = 0,
    Ups = 1,
    Mains = 2,
    Usb = 3,
    UsbDcp = 4,
    UsbCdp = 5,
    UsbAca = 6,
    UsbTypeC = 7,
    UsbPd = 8,
    UsbPdDrp = 9,
    AppleBrickId = 10,
    Wireless = 11,
}

impl PowerSupplyType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Battery => "Battery",
            Self::Ups => "UPS",
            Self::Mains => "Mains",
            Self::Usb => "USB",
            Self::UsbDcp => "USB_DCP",
            Self::UsbCdp => "USB_CDP",
            Self::UsbAca => "USB_ACA",
            Self::UsbTypeC => "USB_Type_C",
            Self::UsbPd => "USB_PD",
            Self::UsbPdDrp => "USB_PD_DRP",
            Self::AppleBrickId => "Apple_Brick_ID",
            Self::Wireless => "Wireless",
        }
    }
}

// ---------------------------------------------------------------------------
// Battery status
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum BatteryStatus {
    Unknown = 0,
    Charging = 1,
    Discharging = 2,
    NotCharging = 3,
    Full = 4,
}

impl BatteryStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Unknown => "Unknown",
            Self::Charging => "Charging",
            Self::Discharging => "Discharging",
            Self::NotCharging => "Not charging",
            Self::Full => "Full",
        }
    }
}

// ---------------------------------------------------------------------------
// Charge type
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum ChargeType {
    None = 0,
    Trickle = 1,
    Fast = 2,
    Standard = 3,
    Adaptive = 4,
    Custom = 5,
}

impl ChargeType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::None => "N/A",
            Self::Trickle => "Trickle",
            Self::Fast => "Fast",
            Self::Standard => "Standard",
            Self::Adaptive => "Adaptive",
            Self::Custom => "Custom",
        }
    }
}

// ---------------------------------------------------------------------------
// Battery health
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum BatteryHealth {
    Unknown = 0,
    Good = 1,
    Overheat = 2,
    Dead = 3,
    OverVoltage = 4,
    UnspecFailure = 5,
    Cold = 6,
    WatchdogTimerExpire = 7,
    SafetyTimerExpire = 8,
    OverCurrent = 9,
    CalibrationRequired = 10,
    Warm = 11,
    Cool = 12,
    Hot = 13,
}

impl BatteryHealth {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Unknown => "Unknown",
            Self::Good => "Good",
            Self::Overheat => "Overheat",
            Self::Dead => "Dead",
            Self::OverVoltage => "Over voltage",
            Self::UnspecFailure => "Unspecified failure",
            Self::Cold => "Cold",
            Self::WatchdogTimerExpire => "Watchdog timer expire",
            Self::SafetyTimerExpire => "Safety timer expire",
            Self::OverCurrent => "Over current",
            Self::CalibrationRequired => "Calibration required",
            Self::Warm => "Warm",
            Self::Cool => "Cool",
            Self::Hot => "Hot",
        }
    }
}

// ---------------------------------------------------------------------------
// Battery technology
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum BatteryTechnology {
    NiMH = 0,
    LiIon = 1,
    LiPoly = 2,
    LiFe = 3,
    NiCd = 4,
    LiMn = 5,
}

impl BatteryTechnology {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::NiMH => "NiMH",
            Self::LiIon => "Li-ion",
            Self::LiPoly => "Li-poly",
            Self::LiFe => "LiFe",
            Self::NiCd => "NiCd",
            Self::LiMn => "LiMn",
        }
    }
}

// ---------------------------------------------------------------------------
// Capacity level
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum CapacityLevel {
    Unknown = 0,
    Critical = 1,
    Low = 2,
    Normal = 3,
    High = 4,
    Full = 5,
}

impl CapacityLevel {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Unknown => "Unknown",
            Self::Critical => "Critical",
            Self::Low => "Low",
            Self::Normal => "Normal",
            Self::High => "High",
            Self::Full => "Full",
        }
    }

    pub fn from_percent(pct: u32) -> Self {
        match pct {
            0..=5 => Self::Critical,
            6..=15 => Self::Low,
            16..=79 => Self::Normal,
            80..=94 => Self::High,
            95..=100 => Self::Full,
            _ => Self::Unknown,
        }
    }
}

// ---------------------------------------------------------------------------
// Power supply properties
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct BatteryProperties {
    pub status: BatteryStatus,
    pub charge_type: ChargeType,
    pub health: BatteryHealth,
    pub present: bool,
    pub online: bool,
    pub authentic: bool,
    pub technology: BatteryTechnology,
    pub cycle_count: u32,

    // Capacity
    pub capacity: u32,
    pub capacity_level: CapacityLevel,
    pub charge_full_uah: u32,
    pub charge_full_design_uah: u32,
    pub charge_now_uah: u32,
    pub charge_counter: i32,
    pub energy_full_uwh: u32,
    pub energy_full_design_uwh: u32,
    pub energy_now_uwh: u32,
    pub energy_avg_uwh: u32,

    // Voltage (in microvolts)
    pub voltage_max_uv: u32,
    pub voltage_min_uv: u32,
    pub voltage_max_design_uv: u32,
    pub voltage_min_design_uv: u32,
    pub voltage_now_uv: u32,
    pub voltage_avg_uv: u32,
    pub voltage_ocv_uv: u32,
    pub voltage_boot_uv: u32,

    // Current (in microamps)
    pub current_max_ua: i32,
    pub current_now_ua: i32,
    pub current_avg_ua: i32,
    pub current_boot_ua: i32,

    // Temperature (in tenths of degree Celsius)
    pub temp: i32,
    pub temp_max: i32,
    pub temp_min: i32,
    pub temp_alert_min: i32,
    pub temp_alert_max: i32,
    pub temp_ambient: i32,
    pub temp_ambient_alert_min: i32,
    pub temp_ambient_alert_max: i32,

    // Charging control
    pub charge_control_limit_ua: u32,
    pub charge_control_limit_max_ua: u32,
    pub charge_control_start_threshold: u32,
    pub charge_control_end_threshold: u32,
    pub input_current_limit_ua: u32,
    pub input_voltage_limit_uv: u32,

    // Time estimates (in seconds)
    pub time_to_empty_now_s: u32,
    pub time_to_empty_avg_s: u32,
    pub time_to_full_now_s: u32,
    pub time_to_full_avg_s: u32,
}

impl BatteryProperties {
    pub fn default_battery() -> Self {
        Self {
            status: BatteryStatus::Unknown,
            charge_type: ChargeType::None,
            health: BatteryHealth::Unknown,
            present: false,
            online: false,
            authentic: false,
            technology: BatteryTechnology::LiIon,
            cycle_count: 0,
            capacity: 0,
            capacity_level: CapacityLevel::Unknown,
            charge_full_uah: 0,
            charge_full_design_uah: 0,
            charge_now_uah: 0,
            charge_counter: 0,
            energy_full_uwh: 0,
            energy_full_design_uwh: 0,
            energy_now_uwh: 0,
            energy_avg_uwh: 0,
            voltage_max_uv: 0,
            voltage_min_uv: 0,
            voltage_max_design_uv: 0,
            voltage_min_design_uv: 0,
            voltage_now_uv: 0,
            voltage_avg_uv: 0,
            voltage_ocv_uv: 0,
            voltage_boot_uv: 0,
            current_max_ua: 0,
            current_now_ua: 0,
            current_avg_ua: 0,
            current_boot_ua: 0,
            temp: 250,
            temp_max: 600,
            temp_min: -100,
            temp_alert_min: 0,
            temp_alert_max: 500,
            temp_ambient: 250,
            temp_ambient_alert_min: -200,
            temp_ambient_alert_max: 700,
            charge_control_limit_ua: 0,
            charge_control_limit_max_ua: 0,
            charge_control_start_threshold: 0,
            charge_control_end_threshold: 100,
            input_current_limit_ua: 0,
            input_voltage_limit_uv: 0,
            time_to_empty_now_s: 0,
            time_to_empty_avg_s: 0,
            time_to_full_now_s: 0,
            time_to_full_avg_s: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// USB Power Delivery
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum PdPowerRole {
    Source = 0,
    Sink = 1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum PdDataRole {
    Ufp = 0,
    Dfp = 1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum PdRevision {
    Rev20 = 0,
    Rev30 = 1,
    Rev31 = 2,
}

impl PdRevision {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Rev20 => "2.0",
            Self::Rev30 => "3.0",
            Self::Rev31 => "3.1",
        }
    }
}

#[derive(Debug, Clone)]
pub struct PdVoltageProfile {
    pub voltage_mv: u32,
    pub current_ma: u32,
    pub power_mw: u32,
}

impl PdVoltageProfile {
    pub fn new(voltage_mv: u32, current_ma: u32) -> Self {
        Self {
            voltage_mv,
            current_ma,
            power_mw: voltage_mv * current_ma / 1000,
        }
    }
}

#[derive(Debug, Clone)]
pub struct PdCapabilities {
    pub revision: PdRevision,
    pub power_role: PdPowerRole,
    pub data_role: PdDataRole,
    pub profiles: Vec<PdVoltageProfile>,
    pub max_voltage_mv: u32,
    pub max_current_ma: u32,
    pub max_power_mw: u32,
    pub pps_supported: bool,
    pub usb4_supported: bool,
}

impl PdCapabilities {
    pub fn new(revision: PdRevision) -> Self {
        Self {
            revision,
            power_role: PdPowerRole::Sink,
            data_role: PdDataRole::Ufp,
            profiles: Vec::new(),
            max_voltage_mv: 5000,
            max_current_ma: 900,
            max_power_mw: 4500,
            pps_supported: false,
            usb4_supported: false,
        }
    }

    pub fn add_profile(&mut self, profile: PdVoltageProfile) {
        if profile.voltage_mv > self.max_voltage_mv {
            self.max_voltage_mv = profile.voltage_mv;
        }
        if profile.current_ma > self.max_current_ma {
            self.max_current_ma = profile.current_ma;
        }
        if profile.power_mw > self.max_power_mw {
            self.max_power_mw = profile.power_mw;
        }
        self.profiles.push(profile);
    }

    pub fn negotiate_best(&self) -> Option<&PdVoltageProfile> {
        self.profiles.iter().max_by_key(|p| p.power_mw)
    }
}

// ---------------------------------------------------------------------------
// Battery fuel gauge
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct OcvTableEntry {
    pub capacity_pct: u32,
    pub voltage_uv: u32,
}

pub struct FuelGauge {
    pub coulomb_counter_uah: i64,
    pub ocv_table: Vec<OcvTableEntry>,
    pub learned_capacity_uah: u32,
    pub impedance_mohm: u32,
    pub last_soc: u32,
    pub smoothing_enabled: bool,
    pub delta_threshold_pct: u32,
}

impl FuelGauge {
    pub fn new() -> Self {
        Self {
            coulomb_counter_uah: 0,
            ocv_table: Vec::new(),
            learned_capacity_uah: 0,
            impedance_mohm: 0,
            last_soc: 0,
            smoothing_enabled: true,
            delta_threshold_pct: 1,
        }
    }

    pub fn add_ocv_entry(&mut self, capacity_pct: u32, voltage_uv: u32) {
        self.ocv_table.push(OcvTableEntry {
            capacity_pct,
            voltage_uv,
        });
        self.ocv_table.sort_by_key(|e| e.capacity_pct);
    }

    pub fn lookup_soc_from_voltage(&self, voltage_uv: u32) -> u32 {
        if self.ocv_table.is_empty() {
            return 0;
        }

        if voltage_uv <= self.ocv_table[0].voltage_uv {
            return self.ocv_table[0].capacity_pct;
        }
        let last = self.ocv_table.len() - 1;
        if voltage_uv >= self.ocv_table[last].voltage_uv {
            return self.ocv_table[last].capacity_pct;
        }

        for i in 0..last {
            let lo = &self.ocv_table[i];
            let hi = &self.ocv_table[i + 1];
            if voltage_uv >= lo.voltage_uv && voltage_uv <= hi.voltage_uv {
                let dv = hi.voltage_uv - lo.voltage_uv;
                let dc = hi.capacity_pct - lo.capacity_pct;
                if dv == 0 {
                    return lo.capacity_pct;
                }
                return lo.capacity_pct + (voltage_uv - lo.voltage_uv) * dc / dv;
            }
        }
        0
    }

    pub fn update_coulomb(&mut self, delta_uah: i64) {
        self.coulomb_counter_uah += delta_uah;
    }

    pub fn coulomb_soc(&self) -> u32 {
        if self.learned_capacity_uah == 0 {
            return 0;
        }
        let soc = (self.coulomb_counter_uah * 100) / self.learned_capacity_uah as i64;
        soc.clamp(0, 100) as u32
    }

    pub fn update_impedance(&mut self, voltage_uv: u32, current_ua: i32, ocv_uv: u32) {
        if current_ua == 0 {
            return;
        }
        let delta_v = if ocv_uv > voltage_uv {
            ocv_uv - voltage_uv
        } else {
            voltage_uv - ocv_uv
        };
        self.impedance_mohm = (delta_v as u64 * 1000 / current_ua.unsigned_abs() as u64) as u32;
    }

    pub fn learn_capacity(&mut self, full_charge_uah: u32) {
        if self.learned_capacity_uah == 0 {
            self.learned_capacity_uah = full_charge_uah;
        } else {
            self.learned_capacity_uah = (self.learned_capacity_uah * 3 + full_charge_uah) / 4;
        }
    }
}

// ---------------------------------------------------------------------------
// Charger IC interface
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct ChargerConfig {
    pub charge_voltage_uv: u32,
    pub charge_current_ua: u32,
    pub precharge_current_ua: u32,
    pub term_current_ua: u32,
    pub input_current_limit_ua: u32,
    pub safety_timer_min: u32,
    pub watchdog_timer_s: u32,
    pub thermal_reg_thresh_c: u32,
    pub jeita_enabled: bool,
}

impl ChargerConfig {
    pub fn default_config() -> Self {
        Self {
            charge_voltage_uv: 4_200_000,
            charge_current_ua: 2_000_000,
            precharge_current_ua: 256_000,
            term_current_ua: 128_000,
            input_current_limit_ua: 3_000_000,
            safety_timer_min: 480,
            watchdog_timer_s: 160,
            thermal_reg_thresh_c: 120,
            jeita_enabled: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum ChargerState {
    Idle = 0,
    Precharge = 1,
    FastCharge = 2,
    Taper = 3,
    TopOff = 4,
    Done = 5,
    Fault = 6,
    Inhibited = 7,
}

impl ChargerState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Idle => "Idle",
            Self::Precharge => "Pre-charge",
            Self::FastCharge => "Fast charge",
            Self::Taper => "Taper",
            Self::TopOff => "Top-off",
            Self::Done => "Done",
            Self::Fault => "Fault",
            Self::Inhibited => "Inhibited",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum JeitaZone {
    Cold = 0,
    Cool = 1,
    Normal = 2,
    Warm = 3,
    Hot = 4,
}

impl JeitaZone {
    pub fn from_temp_decidegc(temp: i32) -> Self {
        match temp {
            i32::MIN..=0 => Self::Cold,
            1..=100 => Self::Cool,
            101..=450 => Self::Normal,
            451..=600 => Self::Warm,
            _ => Self::Hot,
        }
    }

    pub fn charge_allowed(&self) -> bool {
        matches!(self, Self::Cool | Self::Normal | Self::Warm)
    }

    pub fn max_voltage_uv(&self) -> u32 {
        match self {
            Self::Cold => 0,
            Self::Cool => 4_200_000,
            Self::Normal => 4_200_000,
            Self::Warm => 4_100_000,
            Self::Hot => 0,
        }
    }

    pub fn max_current_factor(&self) -> u32 {
        match self {
            Self::Cold => 0,
            Self::Cool => 50,
            Self::Normal => 100,
            Self::Warm => 50,
            Self::Hot => 0,
        }
    }
}

pub struct ChargerIc {
    pub name: String,
    pub config: ChargerConfig,
    pub state: ChargerState,
    pub jeita: JeitaZone,
    pub fault_flags: u32,
    pub vin_uv: u32,
    pub iin_ua: u32,
    pub vbat_uv: u32,
    pub ibat_ua: u32,
}

impl ChargerIc {
    pub const FAULT_WATCHDOG: u32 = 1 << 0;
    pub const FAULT_OVP: u32 = 1 << 1;
    pub const FAULT_THERMAL: u32 = 1 << 2;
    pub const FAULT_SAFETY_TMR: u32 = 1 << 3;
    pub const FAULT_INPUT_OVP: u32 = 1 << 4;
    pub const FAULT_BATOVP: u32 = 1 << 5;

    pub fn new(name: &str) -> Self {
        Self {
            name: String::from(name),
            config: ChargerConfig::default_config(),
            state: ChargerState::Idle,
            jeita: JeitaZone::Normal,
            fault_flags: 0,
            vin_uv: 0,
            iin_ua: 0,
            vbat_uv: 0,
            ibat_ua: 0,
        }
    }

    pub fn set_charge_voltage(&mut self, uv: u32) {
        self.config.charge_voltage_uv = uv;
    }
    pub fn set_charge_current(&mut self, ua: u32) {
        self.config.charge_current_ua = ua;
    }
    pub fn set_precharge_current(&mut self, ua: u32) {
        self.config.precharge_current_ua = ua;
    }
    pub fn set_term_current(&mut self, ua: u32) {
        self.config.term_current_ua = ua;
    }
    pub fn set_input_current_limit(&mut self, ua: u32) {
        self.config.input_current_limit_ua = ua;
    }
    pub fn set_safety_timer(&mut self, min: u32) {
        self.config.safety_timer_min = min;
    }
    pub fn set_watchdog(&mut self, sec: u32) {
        self.config.watchdog_timer_s = sec;
    }
    pub fn set_thermal_reg(&mut self, c: u32) {
        self.config.thermal_reg_thresh_c = c;
    }

    pub fn kick_watchdog(&mut self) {
        self.fault_flags &= !Self::FAULT_WATCHDOG;
    }

    pub fn has_fault(&self) -> bool {
        self.fault_flags != 0
    }

    pub fn update_jeita(&mut self, temp_decidegc: i32) {
        self.jeita = JeitaZone::from_temp_decidegc(temp_decidegc);
        if !self.jeita.charge_allowed() && self.state != ChargerState::Idle {
            self.state = ChargerState::Inhibited;
        }
    }

    pub fn start_charge(&mut self) {
        if self.has_fault() {
            self.state = ChargerState::Fault;
            return;
        }
        if !self.jeita.charge_allowed() {
            self.state = ChargerState::Inhibited;
            return;
        }
        if self.vbat_uv < 3_000_000 {
            self.state = ChargerState::Precharge;
        } else {
            self.state = ChargerState::FastCharge;
        }
    }

    pub fn stop_charge(&mut self) {
        self.state = ChargerState::Idle;
    }

    pub fn update_state(&mut self) {
        if self.has_fault() {
            self.state = ChargerState::Fault;
            return;
        }
        match self.state {
            ChargerState::Precharge => {
                if self.vbat_uv >= 3_000_000 {
                    self.state = ChargerState::FastCharge;
                }
            }
            ChargerState::FastCharge => {
                if self.vbat_uv >= self.config.charge_voltage_uv {
                    self.state = ChargerState::Taper;
                }
            }
            ChargerState::Taper => {
                if self.ibat_ua <= self.config.term_current_ua {
                    self.state = ChargerState::Done;
                }
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Battery protection
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct BatteryProtection {
    pub overcharge_voltage_uv: u32,
    pub overdischarge_voltage_uv: u32,
    pub overcurrent_charge_ua: u32,
    pub overcurrent_discharge_ua: u32,
    pub thermal_shutdown_c: i32,
    pub thermal_recovery_c: i32,
    pub cell_balance_enabled: bool,
    pub cell_balance_threshold_uv: u32,
    pub cell_count: u32,

    pub overcharge_tripped: bool,
    pub overdischarge_tripped: bool,
    pub overcurrent_tripped: bool,
    pub thermal_tripped: bool,
}

impl BatteryProtection {
    pub fn new(cell_count: u32) -> Self {
        Self {
            overcharge_voltage_uv: 4_250_000,
            overdischarge_voltage_uv: 2_700_000,
            overcurrent_charge_ua: 5_000_000,
            overcurrent_discharge_ua: 8_000_000,
            thermal_shutdown_c: 65,
            thermal_recovery_c: 55,
            cell_balance_enabled: true,
            cell_balance_threshold_uv: 20_000,
            cell_count,
            overcharge_tripped: false,
            overdischarge_tripped: false,
            overcurrent_tripped: false,
            thermal_tripped: false,
        }
    }

    pub fn check_voltage(&mut self, cell_voltage_uv: u32) -> bool {
        if cell_voltage_uv >= self.overcharge_voltage_uv {
            self.overcharge_tripped = true;
            return false;
        }
        if cell_voltage_uv <= self.overdischarge_voltage_uv {
            self.overdischarge_tripped = true;
            return false;
        }
        true
    }

    pub fn check_current(&mut self, current_ua: i32) -> bool {
        let abs_current = current_ua.unsigned_abs();
        if current_ua > 0 && abs_current > self.overcurrent_charge_ua {
            self.overcurrent_tripped = true;
            return false;
        }
        if current_ua < 0 && abs_current > self.overcurrent_discharge_ua {
            self.overcurrent_tripped = true;
            return false;
        }
        true
    }

    pub fn check_temperature(&mut self, temp_c: i32) -> bool {
        if temp_c >= self.thermal_shutdown_c {
            self.thermal_tripped = true;
            return false;
        }
        if self.thermal_tripped && temp_c <= self.thermal_recovery_c {
            self.thermal_tripped = false;
        }
        !self.thermal_tripped
    }

    pub fn any_tripped(&self) -> bool {
        self.overcharge_tripped
            || self.overdischarge_tripped
            || self.overcurrent_tripped
            || self.thermal_tripped
    }

    pub fn clear_all(&mut self) {
        self.overcharge_tripped = false;
        self.overdischarge_tripped = false;
        self.overcurrent_tripped = false;
        self.thermal_tripped = false;
    }

    pub fn balance_cells(&self, cell_voltages: &[u32]) -> Vec<bool> {
        if !self.cell_balance_enabled || cell_voltages.is_empty() {
            return alloc::vec![false; cell_voltages.len()];
        }
        let max_v = *cell_voltages.iter().max().unwrap_or(&0);
        cell_voltages
            .iter()
            .map(|&v| max_v.saturating_sub(v) > self.cell_balance_threshold_uv)
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Smart Battery System (SBS)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct SmartBatteryData {
    pub manufacturer_name: String,
    pub device_name: String,
    pub device_chemistry: String,
    pub serial_number: u16,
    pub manufacture_date: u16,
    pub design_capacity: u16,
    pub design_voltage: u16,
    pub spec_info: u16,
    pub full_charge_capacity: u16,
    pub remaining_capacity: u16,
    pub run_time_to_empty: u16,
    pub avg_time_to_empty: u16,
    pub avg_time_to_full: u16,
    pub cycle_count: u16,
    pub max_error: u16,
    pub relative_soc: u16,
    pub absolute_soc: u16,
    pub voltage: u16,
    pub current: i16,
    pub avg_current: i16,
    pub temperature: u16,
    pub charging_current: u16,
    pub charging_voltage: u16,
    pub battery_status: u16,
    pub alarm_warning: u16,
}

impl SmartBatteryData {
    // SBS command addresses
    pub const CMD_MANUFACTURER_ACCESS: u8 = 0x00;
    pub const CMD_REMAINING_CAPACITY_ALARM: u8 = 0x01;
    pub const CMD_REMAINING_TIME_ALARM: u8 = 0x02;
    pub const CMD_BATTERY_MODE: u8 = 0x03;
    pub const CMD_AT_RATE: u8 = 0x04;
    pub const CMD_AT_RATE_TIME_TO_FULL: u8 = 0x05;
    pub const CMD_AT_RATE_TIME_TO_EMPTY: u8 = 0x06;
    pub const CMD_AT_RATE_OK: u8 = 0x07;
    pub const CMD_TEMPERATURE: u8 = 0x08;
    pub const CMD_VOLTAGE: u8 = 0x09;
    pub const CMD_CURRENT: u8 = 0x0A;
    pub const CMD_AVG_CURRENT: u8 = 0x0B;
    pub const CMD_MAX_ERROR: u8 = 0x0C;
    pub const CMD_RELATIVE_SOC: u8 = 0x0D;
    pub const CMD_ABSOLUTE_SOC: u8 = 0x0E;
    pub const CMD_REMAINING_CAPACITY: u8 = 0x0F;
    pub const CMD_FULL_CHARGE_CAPACITY: u8 = 0x10;
    pub const CMD_RUN_TIME_TO_EMPTY: u8 = 0x11;
    pub const CMD_AVG_TIME_TO_EMPTY: u8 = 0x12;
    pub const CMD_AVG_TIME_TO_FULL: u8 = 0x13;
    pub const CMD_CHARGING_CURRENT: u8 = 0x14;
    pub const CMD_CHARGING_VOLTAGE: u8 = 0x15;
    pub const CMD_BATTERY_STATUS: u8 = 0x16;
    pub const CMD_CYCLE_COUNT: u8 = 0x17;
    pub const CMD_DESIGN_CAPACITY: u8 = 0x18;
    pub const CMD_DESIGN_VOLTAGE: u8 = 0x19;
    pub const CMD_SPEC_INFO: u8 = 0x1A;
    pub const CMD_MANUFACTURE_DATE: u8 = 0x1B;
    pub const CMD_SERIAL_NUMBER: u8 = 0x1C;
    pub const CMD_MANUFACTURER_NAME: u8 = 0x20;
    pub const CMD_DEVICE_NAME: u8 = 0x21;
    pub const CMD_DEVICE_CHEMISTRY: u8 = 0x22;

    // Battery status flags
    pub const STATUS_OVER_CHARGED_ALARM: u16 = 1 << 15;
    pub const STATUS_TERMINATE_CHARGE_ALARM: u16 = 1 << 14;
    pub const STATUS_OVER_TEMP_ALARM: u16 = 1 << 12;
    pub const STATUS_TERMINATE_DISCHARGE_ALARM: u16 = 1 << 11;
    pub const STATUS_REMAINING_CAPACITY_ALARM: u16 = 1 << 9;
    pub const STATUS_REMAINING_TIME_ALARM: u16 = 1 << 8;
    pub const STATUS_INITIALIZED: u16 = 1 << 7;
    pub const STATUS_DISCHARGING: u16 = 1 << 6;
    pub const STATUS_FULLY_CHARGED: u16 = 1 << 5;
    pub const STATUS_FULLY_DISCHARGED: u16 = 1 << 4;

    pub fn new() -> Self {
        Self {
            manufacturer_name: String::from("AthenaOS Battery"),
            device_name: String::from("BAT0"),
            device_chemistry: String::from("LION"),
            serial_number: 0x1234,
            manufacture_date: 0,
            design_capacity: 5000,
            design_voltage: 3700,
            spec_info: 0x0031,
            full_charge_capacity: 4800,
            remaining_capacity: 2400,
            run_time_to_empty: 180,
            avg_time_to_empty: 175,
            avg_time_to_full: 120,
            cycle_count: 150,
            max_error: 5,
            relative_soc: 50,
            absolute_soc: 48,
            voltage: 3700,
            current: -500,
            avg_current: -480,
            temperature: 2981,
            charging_current: 2000,
            charging_voltage: 4200,
            battery_status: Self::STATUS_INITIALIZED | Self::STATUS_DISCHARGING,
            alarm_warning: 0,
        }
    }

    pub fn read_word(&self, cmd: u8) -> u16 {
        match cmd {
            Self::CMD_TEMPERATURE => self.temperature,
            Self::CMD_VOLTAGE => self.voltage,
            Self::CMD_CURRENT => self.current as u16,
            Self::CMD_AVG_CURRENT => self.avg_current as u16,
            Self::CMD_MAX_ERROR => self.max_error,
            Self::CMD_RELATIVE_SOC => self.relative_soc,
            Self::CMD_ABSOLUTE_SOC => self.absolute_soc,
            Self::CMD_REMAINING_CAPACITY => self.remaining_capacity,
            Self::CMD_FULL_CHARGE_CAPACITY => self.full_charge_capacity,
            Self::CMD_RUN_TIME_TO_EMPTY => self.run_time_to_empty,
            Self::CMD_AVG_TIME_TO_EMPTY => self.avg_time_to_empty,
            Self::CMD_AVG_TIME_TO_FULL => self.avg_time_to_full,
            Self::CMD_CHARGING_CURRENT => self.charging_current,
            Self::CMD_CHARGING_VOLTAGE => self.charging_voltage,
            Self::CMD_BATTERY_STATUS => self.battery_status,
            Self::CMD_CYCLE_COUNT => self.cycle_count,
            Self::CMD_DESIGN_CAPACITY => self.design_capacity,
            Self::CMD_DESIGN_VOLTAGE => self.design_voltage,
            Self::CMD_SPEC_INFO => self.spec_info,
            Self::CMD_MANUFACTURE_DATE => self.manufacture_date,
            Self::CMD_SERIAL_NUMBER => self.serial_number,
            _ => 0,
        }
    }

    pub fn read_block(&self, cmd: u8) -> String {
        match cmd {
            Self::CMD_MANUFACTURER_NAME => self.manufacturer_name.clone(),
            Self::CMD_DEVICE_NAME => self.device_name.clone(),
            Self::CMD_DEVICE_CHEMISTRY => self.device_chemistry.clone(),
            _ => String::new(),
        }
    }

    pub fn is_discharging(&self) -> bool {
        self.battery_status & Self::STATUS_DISCHARGING != 0
    }

    pub fn is_fully_charged(&self) -> bool {
        self.battery_status & Self::STATUS_FULLY_CHARGED != 0
    }
}

// ---------------------------------------------------------------------------
// Power supply event notifications
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum PowerSupplyEvent {
    CapacityChanged = 0,
    BatteryLow = 1,
    BatteryCritical = 2,
    ChargerConnected = 3,
    ChargerDisconnected = 4,
    TemperatureAlert = 5,
    HealthChanged = 6,
    StatusChanged = 7,
    ChargingStarted = 8,
    ChargingCompleted = 9,
    OvervoltageDetected = 10,
    OvercurrentDetected = 11,
}

impl PowerSupplyEvent {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::CapacityChanged => "capacity_changed",
            Self::BatteryLow => "battery_low",
            Self::BatteryCritical => "battery_critical",
            Self::ChargerConnected => "charger_connected",
            Self::ChargerDisconnected => "charger_disconnected",
            Self::TemperatureAlert => "temperature_alert",
            Self::HealthChanged => "health_changed",
            Self::StatusChanged => "status_changed",
            Self::ChargingStarted => "charging_started",
            Self::ChargingCompleted => "charging_completed",
            Self::OvervoltageDetected => "overvoltage_detected",
            Self::OvercurrentDetected => "overcurrent_detected",
        }
    }
}

pub struct PowerSupplyNotifier {
    listeners: Vec<fn(psy_id: u64, event: PowerSupplyEvent)>,
    capacity_low_threshold: u32,
    capacity_critical_threshold: u32,
}

impl PowerSupplyNotifier {
    fn new() -> Self {
        Self {
            listeners: Vec::new(),
            capacity_low_threshold: 15,
            capacity_critical_threshold: 5,
        }
    }

    pub fn register(&mut self, cb: fn(u64, PowerSupplyEvent)) {
        self.listeners.push(cb);
    }

    pub fn notify(&self, psy_id: u64, event: PowerSupplyEvent) {
        for cb in &self.listeners {
            cb(psy_id, event);
        }
    }

    pub fn check_capacity(&self, psy_id: u64, old_pct: u32, new_pct: u32) {
        if old_pct != new_pct {
            self.notify(psy_id, PowerSupplyEvent::CapacityChanged);
        }
        if new_pct <= self.capacity_critical_threshold && old_pct > self.capacity_critical_threshold
        {
            self.notify(psy_id, PowerSupplyEvent::BatteryCritical);
        } else if new_pct <= self.capacity_low_threshold && old_pct > self.capacity_low_threshold {
            self.notify(psy_id, PowerSupplyEvent::BatteryLow);
        }
    }
}

// ---------------------------------------------------------------------------
// Power supply device
// ---------------------------------------------------------------------------

static PSY_NEXT_ID: AtomicU64 = AtomicU64::new(1);

pub struct PowerSupply {
    pub id: u64,
    pub name: String,
    pub psy_type: PowerSupplyType,
    pub properties: BatteryProperties,
    pub pd_caps: Option<PdCapabilities>,
    pub fuel_gauge: Option<FuelGauge>,
    pub charger: Option<ChargerIc>,
    pub protection: Option<BatteryProtection>,
    pub sbs: Option<SmartBatteryData>,
    pub sysfs_path: String,
    pub supplied_to: Vec<String>,
    pub supplied_from: Vec<String>,
}

impl PowerSupply {
    pub fn new(name: &str, psy_type: PowerSupplyType) -> Self {
        Self {
            id: PSY_NEXT_ID.fetch_add(1, Ordering::Relaxed),
            name: String::from(name),
            psy_type,
            properties: BatteryProperties::default_battery(),
            pd_caps: None,
            fuel_gauge: None,
            charger: None,
            protection: None,
            sbs: None,
            sysfs_path: format!("/sys/class/power_supply/{}", name),
            supplied_to: Vec::new(),
            supplied_from: Vec::new(),
        }
    }

    pub fn new_battery(name: &str) -> Self {
        let mut psy = Self::new(name, PowerSupplyType::Battery);
        psy.properties.present = true;
        psy.properties.technology = BatteryTechnology::LiIon;
        psy.fuel_gauge = Some(FuelGauge::new());
        psy.protection = Some(BatteryProtection::new(1));
        psy.sbs = Some(SmartBatteryData::new());
        psy
    }

    pub fn new_charger(name: &str, psy_type: PowerSupplyType) -> Self {
        let mut psy = Self::new(name, psy_type);
        psy.charger = Some(ChargerIc::new(name));
        if matches!(
            psy_type,
            PowerSupplyType::UsbPd | PowerSupplyType::UsbPdDrp | PowerSupplyType::UsbTypeC
        ) {
            psy.pd_caps = Some(PdCapabilities::new(PdRevision::Rev30));
        }
        psy
    }

    pub fn update_capacity(&mut self, pct: u32) {
        let old = self.properties.capacity;
        self.properties.capacity = pct.min(100);
        self.properties.capacity_level = CapacityLevel::from_percent(self.properties.capacity);

        let subsys = POWER_SUPPLY_SUBSYSTEM.lock();
        subsys
            .notifier
            .check_capacity(self.id, old, self.properties.capacity);
    }

    pub fn set_status(&mut self, status: BatteryStatus) {
        if self.properties.status != status {
            self.properties.status = status;
            let subsys = POWER_SUPPLY_SUBSYSTEM.lock();
            subsys
                .notifier
                .notify(self.id, PowerSupplyEvent::StatusChanged);
        }
    }

    pub fn set_health(&mut self, health: BatteryHealth) {
        if self.properties.health != health {
            self.properties.health = health;
            let subsys = POWER_SUPPLY_SUBSYSTEM.lock();
            subsys
                .notifier
                .notify(self.id, PowerSupplyEvent::HealthChanged);
        }
    }

    pub fn sysfs_attr(&self, attr: &str) -> Option<String> {
        match attr {
            "type" => Some(String::from(self.psy_type.as_str())),
            "status" => Some(String::from(self.properties.status.as_str())),
            "charge_type" => Some(String::from(self.properties.charge_type.as_str())),
            "health" => Some(String::from(self.properties.health.as_str())),
            "present" => Some(format!("{}", self.properties.present as u32)),
            "online" => Some(format!("{}", self.properties.online as u32)),
            "authentic" => Some(format!("{}", self.properties.authentic as u32)),
            "technology" => Some(String::from(self.properties.technology.as_str())),
            "cycle_count" => Some(format!("{}", self.properties.cycle_count)),
            "capacity" => Some(format!("{}", self.properties.capacity)),
            "capacity_level" => Some(String::from(self.properties.capacity_level.as_str())),
            "charge_full" => Some(format!("{}", self.properties.charge_full_uah)),
            "charge_full_design" => Some(format!("{}", self.properties.charge_full_design_uah)),
            "charge_now" => Some(format!("{}", self.properties.charge_now_uah)),
            "charge_counter" => Some(format!("{}", self.properties.charge_counter)),
            "energy_full" => Some(format!("{}", self.properties.energy_full_uwh)),
            "energy_full_design" => Some(format!("{}", self.properties.energy_full_design_uwh)),
            "energy_now" => Some(format!("{}", self.properties.energy_now_uwh)),
            "energy_avg" => Some(format!("{}", self.properties.energy_avg_uwh)),
            "voltage_max" => Some(format!("{}", self.properties.voltage_max_uv)),
            "voltage_min" => Some(format!("{}", self.properties.voltage_min_uv)),
            "voltage_max_design" => Some(format!("{}", self.properties.voltage_max_design_uv)),
            "voltage_min_design" => Some(format!("{}", self.properties.voltage_min_design_uv)),
            "voltage_now" => Some(format!("{}", self.properties.voltage_now_uv)),
            "voltage_avg" => Some(format!("{}", self.properties.voltage_avg_uv)),
            "voltage_ocv" => Some(format!("{}", self.properties.voltage_ocv_uv)),
            "voltage_boot" => Some(format!("{}", self.properties.voltage_boot_uv)),
            "current_max" => Some(format!("{}", self.properties.current_max_ua)),
            "current_now" => Some(format!("{}", self.properties.current_now_ua)),
            "current_avg" => Some(format!("{}", self.properties.current_avg_ua)),
            "current_boot" => Some(format!("{}", self.properties.current_boot_ua)),
            "temp" => Some(format!("{}", self.properties.temp)),
            "temp_max" => Some(format!("{}", self.properties.temp_max)),
            "temp_min" => Some(format!("{}", self.properties.temp_min)),
            "temp_alert_min" => Some(format!("{}", self.properties.temp_alert_min)),
            "temp_alert_max" => Some(format!("{}", self.properties.temp_alert_max)),
            "temp_ambient" => Some(format!("{}", self.properties.temp_ambient)),
            "temp_ambient_alert_min" => Some(format!("{}", self.properties.temp_ambient_alert_min)),
            "temp_ambient_alert_max" => Some(format!("{}", self.properties.temp_ambient_alert_max)),
            "charge_control_limit" => Some(format!("{}", self.properties.charge_control_limit_ua)),
            "charge_control_limit_max" => {
                Some(format!("{}", self.properties.charge_control_limit_max_ua))
            }
            "charge_control_start_threshold" => Some(format!(
                "{}",
                self.properties.charge_control_start_threshold
            )),
            "charge_control_end_threshold" => {
                Some(format!("{}", self.properties.charge_control_end_threshold))
            }
            "input_current_limit" => Some(format!("{}", self.properties.input_current_limit_ua)),
            "input_voltage_limit" => Some(format!("{}", self.properties.input_voltage_limit_uv)),
            "time_to_empty_now" => Some(format!("{}", self.properties.time_to_empty_now_s)),
            "time_to_empty_avg" => Some(format!("{}", self.properties.time_to_empty_avg_s)),
            "time_to_full_now" => Some(format!("{}", self.properties.time_to_full_now_s)),
            "time_to_full_avg" => Some(format!("{}", self.properties.time_to_full_avg_s)),
            _ => None,
        }
    }

    pub fn uevent_vars(&self) -> Vec<(String, String)> {
        let mut vars = Vec::new();
        vars.push((String::from("POWER_SUPPLY_NAME"), self.name.clone()));
        vars.push((
            String::from("POWER_SUPPLY_TYPE"),
            String::from(self.psy_type.as_str()),
        ));
        vars.push((
            String::from("POWER_SUPPLY_STATUS"),
            String::from(self.properties.status.as_str()),
        ));
        vars.push((
            String::from("POWER_SUPPLY_PRESENT"),
            format!("{}", self.properties.present as u32),
        ));
        vars.push((
            String::from("POWER_SUPPLY_ONLINE"),
            format!("{}", self.properties.online as u32),
        ));
        vars.push((
            String::from("POWER_SUPPLY_HEALTH"),
            String::from(self.properties.health.as_str()),
        ));
        vars.push((
            String::from("POWER_SUPPLY_TECHNOLOGY"),
            String::from(self.properties.technology.as_str()),
        ));
        vars.push((
            String::from("POWER_SUPPLY_CAPACITY"),
            format!("{}", self.properties.capacity),
        ));
        vars.push((
            String::from("POWER_SUPPLY_CAPACITY_LEVEL"),
            String::from(self.properties.capacity_level.as_str()),
        ));
        vars.push((
            String::from("POWER_SUPPLY_VOLTAGE_NOW"),
            format!("{}", self.properties.voltage_now_uv),
        ));
        vars.push((
            String::from("POWER_SUPPLY_CURRENT_NOW"),
            format!("{}", self.properties.current_now_ua),
        ));
        vars.push((
            String::from("POWER_SUPPLY_TEMP"),
            format!("{}", self.properties.temp),
        ));
        vars.push((
            String::from("POWER_SUPPLY_CYCLE_COUNT"),
            format!("{}", self.properties.cycle_count),
        ));
        vars
    }
}

// ---------------------------------------------------------------------------
// Power supply subsystem state
// ---------------------------------------------------------------------------

pub struct PowerSupplyState {
    initialized: bool,
    supplies: BTreeMap<u64, PowerSupply>,
    name_to_id: BTreeMap<String, u64>,
    notifier: PowerSupplyNotifier,
    uevent_seq: AtomicU64,
}

impl PowerSupplyState {
    fn new() -> Self {
        Self {
            initialized: false,
            supplies: BTreeMap::new(),
            name_to_id: BTreeMap::new(),
            notifier: PowerSupplyNotifier::new(),
            uevent_seq: AtomicU64::new(0),
        }
    }
}

lazy_static::lazy_static! {
    pub static ref POWER_SUPPLY_SUBSYSTEM: Mutex<PowerSupplyState> = Mutex::new(PowerSupplyState::new());
}

// ---------------------------------------------------------------------------
// Registration / lookup
// ---------------------------------------------------------------------------

pub fn power_supply_register(psy: PowerSupply) -> Result<u64, PowerSupplyError> {
    let id = psy.id;
    let name = psy.name.clone();
    let mut subsys = POWER_SUPPLY_SUBSYSTEM.lock();
    if subsys.name_to_id.contains_key(&name) {
        return Err(PowerSupplyError::AlreadyExists);
    }
    subsys.name_to_id.insert(name, id);
    subsys.supplies.insert(id, psy);
    Ok(id)
}

pub fn power_supply_unregister(id: u64) -> Result<(), PowerSupplyError> {
    let mut subsys = POWER_SUPPLY_SUBSYSTEM.lock();
    let psy = subsys
        .supplies
        .remove(&id)
        .ok_or(PowerSupplyError::NotFound)?;
    subsys.name_to_id.remove(&psy.name);
    Ok(())
}

pub fn power_supply_changed(id: u64) {
    let subsys = POWER_SUPPLY_SUBSYSTEM.lock();
    if subsys.supplies.contains_key(&id) {
        subsys.uevent_seq.fetch_add(1, Ordering::Relaxed);
    }
}

pub fn power_supply_get_by_name(name: &str) -> Option<u64> {
    let subsys = POWER_SUPPLY_SUBSYSTEM.lock();
    subsys.name_to_id.get(name).copied()
}

pub fn power_supply_get_property(id: u64, attr: &str) -> Option<String> {
    let subsys = POWER_SUPPLY_SUBSYSTEM.lock();
    subsys
        .supplies
        .get(&id)
        .and_then(|psy| psy.sysfs_attr(attr))
}

pub fn power_supply_list() -> Vec<(u64, String, PowerSupplyType)> {
    let subsys = POWER_SUPPLY_SUBSYSTEM.lock();
    subsys
        .supplies
        .values()
        .map(|p| (p.id, p.name.clone(), p.psy_type))
        .collect()
}

#[derive(Debug)]
pub enum PowerSupplyError {
    NotFound,
    AlreadyExists,
    InvalidProperty,
    PermissionDenied,
}

// ---------------------------------------------------------------------------
// Default battery / charger for init
// ---------------------------------------------------------------------------

fn create_default_battery() -> PowerSupply {
    let mut bat = PowerSupply::new_battery("BAT0");
    bat.properties.status = BatteryStatus::Discharging;
    bat.properties.health = BatteryHealth::Good;
    bat.properties.present = true;
    bat.properties.online = true;
    bat.properties.authentic = true;
    bat.properties.technology = BatteryTechnology::LiIon;
    bat.properties.cycle_count = 152;
    bat.properties.capacity = 78;
    bat.properties.capacity_level = CapacityLevel::Normal;
    bat.properties.charge_full_uah = 4_800_000;
    bat.properties.charge_full_design_uah = 5_000_000;
    bat.properties.charge_now_uah = 3_744_000;
    bat.properties.energy_full_uwh = 17_760_000;
    bat.properties.energy_full_design_uwh = 18_500_000;
    bat.properties.energy_now_uwh = 13_852_800;
    bat.properties.voltage_max_design_uv = 4_200_000;
    bat.properties.voltage_min_design_uv = 3_000_000;
    bat.properties.voltage_now_uv = 3_870_000;
    bat.properties.voltage_avg_uv = 3_865_000;
    bat.properties.voltage_ocv_uv = 3_890_000;
    bat.properties.current_now_ua = -1_200_000;
    bat.properties.current_avg_ua = -1_150_000;
    bat.properties.temp = 285;
    bat.properties.temp_max = 600;
    bat.properties.temp_min = -100;
    bat.properties.temp_alert_max = 500;
    bat.properties.time_to_empty_avg_s = 11700;
    bat.properties.charge_control_end_threshold = 100;

    if let Some(ref mut fg) = bat.fuel_gauge {
        fg.add_ocv_entry(0, 3_000_000);
        fg.add_ocv_entry(10, 3_450_000);
        fg.add_ocv_entry(20, 3_550_000);
        fg.add_ocv_entry(30, 3_620_000);
        fg.add_ocv_entry(40, 3_680_000);
        fg.add_ocv_entry(50, 3_740_000);
        fg.add_ocv_entry(60, 3_800_000);
        fg.add_ocv_entry(70, 3_860_000);
        fg.add_ocv_entry(80, 3_930_000);
        fg.add_ocv_entry(90, 4_020_000);
        fg.add_ocv_entry(100, 4_180_000);
        fg.learned_capacity_uah = 4_800_000;
    }

    bat
}

fn create_default_ac_adapter() -> PowerSupply {
    let mut ac = PowerSupply::new("AC0", PowerSupplyType::Mains);
    ac.properties.online = true;
    ac.supplied_to.push(String::from("BAT0"));
    ac
}

fn create_default_usb_pd_charger() -> PowerSupply {
    let mut usb = PowerSupply::new_charger("USB_PD0", PowerSupplyType::UsbPd);
    usb.properties.online = true;
    usb.supplied_to.push(String::from("BAT0"));

    if let Some(ref mut pd) = usb.pd_caps {
        pd.add_profile(PdVoltageProfile::new(5000, 3000));
        pd.add_profile(PdVoltageProfile::new(9000, 3000));
        pd.add_profile(PdVoltageProfile::new(15000, 3000));
        pd.add_profile(PdVoltageProfile::new(20000, 5000));
        pd.pps_supported = true;
    }

    if let Some(ref mut charger) = usb.charger {
        charger.set_charge_voltage(4_200_000);
        charger.set_charge_current(3_000_000);
        charger.set_input_current_limit(5_000_000);
    }

    usb
}

// ---------------------------------------------------------------------------
// init()
// ---------------------------------------------------------------------------

pub fn init() {
    let mut subsys = POWER_SUPPLY_SUBSYSTEM.lock();
    if subsys.initialized {
        return;
    }
    subsys.initialized = true;
    drop(subsys);

    let bat = create_default_battery();
    let _ = power_supply_register(bat);

    let ac = create_default_ac_adapter();
    let _ = power_supply_register(ac);

    let usb = create_default_usb_pd_charger();
    let _ = power_supply_register(usb);
}

/// Deterministic proof of the battery fuel gauge with ZERO hardware access:
/// open-circuit-voltage → state-of-charge lookup with linear interpolation and
/// out-of-range clamping, coulomb counting against a learned capacity, and the
/// capacity-level thresholds. Runs against LOCAL `FuelGauge` instances so it
/// never perturbs the live subsystem and reads no MSR/EC. MasterChecklist
/// Phase 2.7 — "Battery percentage updates over time". Concept §power.
pub fn run_boot_smoketest() {
    let mut pass = 0u32;
    let mut total = 0u32;
    let mut check = |c: bool, n: &str| {
        total += 1;
        if c {
            pass += 1;
        } else {
            crate::serial_println!("[battery-selftest] FAIL {}", n);
        }
    };

    // OCV → SoC: a 3-point Li-ion-ish curve (0% @ 3.0V, 50% @ 3.7V, 100% @ 4.2V).
    let mut fg = FuelGauge::new();
    fg.add_ocv_entry(0, 3_000_000);
    fg.add_ocv_entry(50, 3_700_000);
    fg.add_ocv_entry(100, 4_200_000);
    check(fg.lookup_soc_from_voltage(3_000_000) == 0, "ocv-min");
    check(fg.lookup_soc_from_voltage(4_200_000) == 100, "ocv-max");
    check(fg.lookup_soc_from_voltage(3_700_000) == 50, "ocv-mid");
    // Midway between 3.7V (50%) and 4.2V (100%) → 75% by linear interpolation.
    check(fg.lookup_soc_from_voltage(3_950_000) == 75, "ocv-interp");
    check(fg.lookup_soc_from_voltage(2_500_000) == 0, "ocv-clamp-low");
    check(
        fg.lookup_soc_from_voltage(5_000_000) == 100,
        "ocv-clamp-high",
    );

    // Coulomb counting: 0.5 Ah accumulated against a 1.0 Ah learned capacity.
    fg.learn_capacity(1_000_000);
    fg.update_coulomb(500_000);
    check(fg.coulomb_soc() == 50, "coulomb-soc");

    // Capacity-level thresholds map percent → user-facing level.
    check(
        CapacityLevel::from_percent(3) == CapacityLevel::Critical,
        "cap-critical",
    );
    check(
        CapacityLevel::from_percent(50) == CapacityLevel::Normal,
        "cap-normal",
    );
    check(
        CapacityLevel::from_percent(100) == CapacityLevel::Full,
        "cap-full",
    );

    drop(check);
    crate::serial_println!(
        "[ OK ] battery fuel-gauge selftest: {}/{} checks passed (OCV interp + coulomb counting + capacity levels)",
        pass,
        total
    );
    if pass != total {
        crate::serial_println!(
            "[FAIL] battery fuel-gauge selftest: {} check(s) failed",
            total - pass
        );
    }
}
