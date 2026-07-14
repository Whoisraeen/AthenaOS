//! Fan Curve / Power Management — OS-level thermal and power control.
//!
//! Replaces the sprawl of vendor utilities (MSI Afterburner, Armoury Crate,
//! iCUE, etc.) with a single, unified, capability-gated interface.
//!
//! - `FanCurve` — user-editable temperature→speed mapping with up to 8 points.
//! - `PowerProfile` — named bundles of fan curves + CPU/GPU limits.
//! - `PowerManager` — runtime controller with AC/battery auto-switch.

#![allow(dead_code)]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

// ── Fan Curve ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FanPoint {
    pub temp_c: u8,
    pub speed_pct: u8,
}

impl FanPoint {
    pub const fn new(temp_c: u8, speed_pct: u8) -> Self {
        Self {
            temp_c,
            speed_pct: if speed_pct > 100 { 100 } else { speed_pct },
        }
    }
}

#[derive(Debug, Clone)]
pub struct FanCurve {
    pub name: String,
    pub points: Vec<FanPoint>,
    pub hysteresis_c: u8,
    pub min_speed_pct: u8,
    pub max_speed_pct: u8,
}

impl FanCurve {
    pub fn new(name: &str) -> Self {
        Self {
            name: String::from(name),
            points: Vec::new(),
            hysteresis_c: 3,
            min_speed_pct: 0,
            max_speed_pct: 100,
        }
    }

    pub fn add_point(&mut self, temp_c: u8, speed_pct: u8) {
        let point = FanPoint::new(temp_c, speed_pct);
        let pos = self.points.iter().position(|p| p.temp_c > temp_c);
        match pos {
            Some(i) => self.points.insert(i, point),
            None => self.points.push(point),
        }
    }

    pub fn remove_point(&mut self, index: usize) {
        if index < self.points.len() {
            self.points.remove(index);
        }
    }

    pub fn evaluate(&self, temp_c: u8) -> u8 {
        if self.points.is_empty() {
            return self.min_speed_pct;
        }
        if self.points.len() == 1 {
            return self.points[0].speed_pct;
        }

        if temp_c <= self.points[0].temp_c {
            return self.clamp_speed(self.points[0].speed_pct);
        }
        let last = self.points.len() - 1;
        if temp_c >= self.points[last].temp_c {
            return self.clamp_speed(self.points[last].speed_pct);
        }

        for i in 0..self.points.len() - 1 {
            let a = &self.points[i];
            let b = &self.points[i + 1];
            if temp_c >= a.temp_c && temp_c <= b.temp_c {
                let range_t = b.temp_c - a.temp_c;
                if range_t == 0 {
                    return self.clamp_speed(a.speed_pct);
                }
                let range_s = b.speed_pct as i16 - a.speed_pct as i16;
                let delta = temp_c - a.temp_c;
                let speed = a.speed_pct as i16 + (range_s * delta as i16) / range_t as i16;
                return self.clamp_speed(speed.max(0) as u8);
            }
        }

        self.min_speed_pct
    }

    fn clamp_speed(&self, speed: u8) -> u8 {
        speed.clamp(self.min_speed_pct, self.max_speed_pct)
    }

    pub fn evaluate_with_hysteresis(&self, temp_c: u8, last_speed: u8, rising: bool) -> u8 {
        let effective_temp = if rising {
            temp_c
        } else {
            temp_c.saturating_sub(self.hysteresis_c)
        };
        let target = self.evaluate(effective_temp);
        if rising {
            target.max(last_speed)
        } else {
            target
        }
    }
}

// ── Preset fan curves ────────────────────────────────────────────────────

pub fn fan_curve_silent() -> FanCurve {
    let mut c = FanCurve::new("Silent");
    c.add_point(0, 0);
    c.add_point(40, 0);
    c.add_point(55, 25);
    c.add_point(65, 40);
    c.add_point(75, 60);
    c.add_point(85, 80);
    c.add_point(95, 100);
    c.min_speed_pct = 0;
    c.hysteresis_c = 5;
    c
}

pub fn fan_curve_balanced() -> FanCurve {
    let mut c = FanCurve::new("Balanced");
    c.add_point(0, 20);
    c.add_point(35, 25);
    c.add_point(50, 35);
    c.add_point(60, 50);
    c.add_point(70, 65);
    c.add_point(80, 85);
    c.add_point(90, 100);
    c.min_speed_pct = 20;
    c.hysteresis_c = 3;
    c
}

pub fn fan_curve_performance() -> FanCurve {
    let mut c = FanCurve::new("Performance");
    c.add_point(0, 30);
    c.add_point(40, 40);
    c.add_point(50, 55);
    c.add_point(60, 70);
    c.add_point(70, 85);
    c.add_point(80, 95);
    c.add_point(85, 100);
    c.min_speed_pct = 30;
    c.hysteresis_c = 2;
    c
}

pub fn fan_curve_full_blast() -> FanCurve {
    let mut c = FanCurve::new("Full Blast");
    c.add_point(0, 100);
    c.add_point(100, 100);
    c.min_speed_pct = 100;
    c
}

// ── CPU / GPU frequency limits ───────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CpuLimits {
    pub max_freq_mhz: u32,
    pub min_freq_mhz: u32,
    pub max_cores: u8,
    pub turbo_enabled: bool,
    pub tdp_watts: u16,
}

impl CpuLimits {
    pub const fn unrestricted() -> Self {
        Self {
            max_freq_mhz: 0,
            min_freq_mhz: 0,
            max_cores: 0,
            turbo_enabled: true,
            tdp_watts: 0,
        }
    }

    pub const fn power_saver() -> Self {
        Self {
            max_freq_mhz: 2000,
            min_freq_mhz: 800,
            max_cores: 0,
            turbo_enabled: false,
            tdp_watts: 15,
        }
    }

    pub const fn balanced() -> Self {
        Self {
            max_freq_mhz: 0,
            min_freq_mhz: 800,
            max_cores: 0,
            turbo_enabled: true,
            tdp_watts: 65,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GpuLimits {
    pub power_limit_pct: u8,
    pub core_offset_mhz: i16,
    pub mem_offset_mhz: i16,
    pub temp_target_c: u8,
    pub max_temp_c: u8,
}

impl GpuLimits {
    pub const fn unrestricted() -> Self {
        Self {
            power_limit_pct: 100,
            core_offset_mhz: 0,
            mem_offset_mhz: 0,
            temp_target_c: 83,
            max_temp_c: 90,
        }
    }

    pub const fn power_saver() -> Self {
        Self {
            power_limit_pct: 60,
            core_offset_mhz: -100,
            mem_offset_mhz: 0,
            temp_target_c: 70,
            max_temp_c: 80,
        }
    }

    pub const fn performance() -> Self {
        Self {
            power_limit_pct: 115,
            core_offset_mhz: 0,
            mem_offset_mhz: 0,
            temp_target_c: 85,
            max_temp_c: 93,
        }
    }
}

// ── Display limits ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DisplayLimits {
    pub brightness_pct: u8,
    pub refresh_rate: u16,
    pub adaptive_sync: bool,
}

impl DisplayLimits {
    pub const fn default_display() -> Self {
        Self {
            brightness_pct: 80,
            refresh_rate: 0,
            adaptive_sync: true,
        }
    }

    pub const fn power_saver_display() -> Self {
        Self {
            brightness_pct: 40,
            refresh_rate: 60,
            adaptive_sync: false,
        }
    }
}

// ── Power profile ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerProfileKind {
    Performance,
    Balanced,
    PowerSaver,
    Silent,
    Custom,
}

#[derive(Debug, Clone)]
pub struct PowerProfile {
    pub name: String,
    pub kind: PowerProfileKind,
    pub cpu_fan: FanCurve,
    pub gpu_fan: FanCurve,
    pub case_fans: FanCurve,
    pub cpu_limits: CpuLimits,
    pub gpu_limits: GpuLimits,
    pub display: DisplayLimits,
    pub bg_throttle: bool,
    pub sleep_timeout_min: u16,
    pub screen_off_min: u16,
}

impl PowerProfile {
    pub fn performance() -> Self {
        Self {
            name: String::from("Performance"),
            kind: PowerProfileKind::Performance,
            cpu_fan: fan_curve_performance(),
            gpu_fan: fan_curve_performance(),
            case_fans: fan_curve_balanced(),
            cpu_limits: CpuLimits::unrestricted(),
            gpu_limits: GpuLimits::performance(),
            display: DisplayLimits::default_display(),
            bg_throttle: true,
            sleep_timeout_min: 0,
            screen_off_min: 30,
        }
    }

    pub fn balanced() -> Self {
        Self {
            name: String::from("Balanced"),
            kind: PowerProfileKind::Balanced,
            cpu_fan: fan_curve_balanced(),
            gpu_fan: fan_curve_balanced(),
            case_fans: fan_curve_balanced(),
            cpu_limits: CpuLimits::balanced(),
            gpu_limits: GpuLimits::unrestricted(),
            display: DisplayLimits::default_display(),
            bg_throttle: false,
            sleep_timeout_min: 30,
            screen_off_min: 15,
        }
    }

    pub fn power_saver() -> Self {
        Self {
            name: String::from("Power Saver"),
            kind: PowerProfileKind::PowerSaver,
            cpu_fan: fan_curve_silent(),
            gpu_fan: fan_curve_silent(),
            case_fans: fan_curve_silent(),
            cpu_limits: CpuLimits::power_saver(),
            gpu_limits: GpuLimits::power_saver(),
            display: DisplayLimits::power_saver_display(),
            bg_throttle: true,
            sleep_timeout_min: 10,
            screen_off_min: 5,
        }
    }

    pub fn silent() -> Self {
        Self {
            name: String::from("Silent"),
            kind: PowerProfileKind::Silent,
            cpu_fan: fan_curve_silent(),
            gpu_fan: fan_curve_silent(),
            case_fans: fan_curve_silent(),
            cpu_limits: CpuLimits::power_saver(),
            gpu_limits: GpuLimits::power_saver(),
            display: DisplayLimits::default_display(),
            bg_throttle: false,
            sleep_timeout_min: 20,
            screen_off_min: 10,
        }
    }
}

// ── Thermal sensor readings ──────────────────────────────────────────────

#[derive(Debug, Clone, Copy, Default)]
pub struct ThermalReading {
    pub cpu_temp_c: u8,
    pub gpu_temp_c: u8,
    pub case_temp_c: u8,
    pub vrm_temp_c: u8,
    pub nvme_temp_c: u8,
    pub cpu_fan_rpm: u16,
    pub gpu_fan_rpm: u16,
    pub case_fan_rpm: u16,
    pub cpu_fan_pct: u8,
    pub gpu_fan_pct: u8,
    pub case_fan_pct: u8,
}

// ── Power source ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerSource {
    AC,
    Battery,
    UPS,
    Unknown,
}

#[derive(Debug, Clone, Copy)]
pub struct BatteryState {
    pub source: PowerSource,
    pub charge_pct: u8,
    pub charging: bool,
    pub time_remain_min: u16,
    pub wattage: u16,
}

impl BatteryState {
    pub fn ac_power() -> Self {
        Self {
            source: PowerSource::AC,
            charge_pct: 100,
            charging: false,
            time_remain_min: 0,
            wattage: 0,
        }
    }
}

// ── Power Manager — runtime controller ───────────────────────────────────

pub struct PowerManager {
    pub profiles: Vec<PowerProfile>,
    pub active_profile: usize,
    pub ac_profile: Option<String>,
    pub battery_profile: Option<String>,
    pub auto_switch: bool,
    pub battery: BatteryState,
    pub thermals: ThermalReading,
    pub last_cpu_speed: u8,
    pub last_gpu_speed: u8,
    pub last_case_speed: u8,
    pub temp_rising: bool,
    pub last_cpu_temp: u8,
}

impl PowerManager {
    pub fn new() -> Self {
        let profiles = alloc::vec![
            PowerProfile::performance(),
            PowerProfile::balanced(),
            PowerProfile::power_saver(),
            PowerProfile::silent(),
        ];
        Self {
            profiles,
            active_profile: 1,
            ac_profile: Some(String::from("Balanced")),
            battery_profile: Some(String::from("Power Saver")),
            auto_switch: true,
            battery: BatteryState::ac_power(),
            thermals: ThermalReading::default(),
            last_cpu_speed: 0,
            last_gpu_speed: 0,
            last_case_speed: 0,
            temp_rising: false,
            last_cpu_temp: 0,
        }
    }

    pub fn active(&self) -> &PowerProfile {
        &self.profiles[self.active_profile]
    }

    pub fn set_profile(&mut self, name: &str) -> bool {
        if let Some(idx) = self.profiles.iter().position(|p| p.name == name) {
            self.active_profile = idx;
            true
        } else {
            false
        }
    }

    pub fn cycle_profile(&mut self) {
        self.active_profile = (self.active_profile + 1) % self.profiles.len();
    }

    pub fn add_custom_profile(&mut self, profile: PowerProfile) {
        if let Some(existing) = self.profiles.iter_mut().find(|p| p.name == profile.name) {
            *existing = profile;
        } else {
            self.profiles.push(profile);
        }
    }

    pub fn remove_profile(&mut self, name: &str) {
        let current_name = self.profiles[self.active_profile].name.clone();
        self.profiles.retain(|p| p.name != name);
        if self.profiles.is_empty() {
            self.profiles.push(PowerProfile::balanced());
            self.active_profile = 0;
        } else if current_name == name {
            self.active_profile = 0;
        } else {
            self.active_profile = self
                .profiles
                .iter()
                .position(|p| p.name == current_name)
                .unwrap_or(0);
        }
    }

    pub fn profile_names(&self) -> Vec<&str> {
        self.profiles.iter().map(|p| p.name.as_str()).collect()
    }

    // ── Thermal update loop ──────────────────────────────────────────

    pub fn update_thermals(&mut self, reading: ThermalReading) {
        self.temp_rising = reading.cpu_temp_c > self.last_cpu_temp;
        self.last_cpu_temp = reading.cpu_temp_c;
        self.thermals = reading;

        let profile = &self.profiles[self.active_profile];

        self.last_cpu_speed = profile.cpu_fan.evaluate_with_hysteresis(
            self.thermals.cpu_temp_c,
            self.last_cpu_speed,
            self.temp_rising,
        );
        self.last_gpu_speed = profile.gpu_fan.evaluate_with_hysteresis(
            self.thermals.gpu_temp_c,
            self.last_gpu_speed,
            self.temp_rising,
        );
        self.last_case_speed = profile.case_fans.evaluate_with_hysteresis(
            self.thermals.case_temp_c,
            self.last_case_speed,
            self.temp_rising,
        );
    }

    pub fn target_fan_speeds(&self) -> (u8, u8, u8) {
        (
            self.last_cpu_speed,
            self.last_gpu_speed,
            self.last_case_speed,
        )
    }

    // ── AC/Battery auto-switch ───────────────────────────────────────

    pub fn update_power_source(&mut self, battery: BatteryState) {
        let old_source = self.battery.source;
        self.battery = battery;

        if !self.auto_switch {
            return;
        }

        if battery.source != old_source {
            match battery.source {
                PowerSource::AC => {
                    if let Some(ref name) = self.ac_profile {
                        let n = name.clone();
                        self.set_profile(&n);
                    }
                }
                PowerSource::Battery => {
                    if let Some(ref name) = self.battery_profile {
                        let n = name.clone();
                        self.set_profile(&n);
                    }
                }
                _ => {}
            }
        }
    }

    pub fn set_ac_profile(&mut self, name: &str) {
        self.ac_profile = Some(String::from(name));
    }

    pub fn set_battery_profile(&mut self, name: &str) {
        self.battery_profile = Some(String::from(name));
    }

    // ── Emergency throttle ───────────────────────────────────────────

    pub fn emergency_check(&mut self) -> bool {
        let critical_cpu = self.thermals.cpu_temp_c >= 95;
        let critical_gpu = self.thermals.gpu_temp_c >= 95;
        let critical_vrm = self.thermals.vrm_temp_c >= 110;

        if critical_cpu || critical_gpu || critical_vrm {
            self.last_cpu_speed = 100;
            self.last_gpu_speed = 100;
            self.last_case_speed = 100;
            true
        } else {
            false
        }
    }

    // ── Query helpers ────────────────────────────────────────────────

    pub fn cpu_limits(&self) -> &CpuLimits {
        &self.profiles[self.active_profile].cpu_limits
    }

    pub fn gpu_limits(&self) -> &GpuLimits {
        &self.profiles[self.active_profile].gpu_limits
    }

    pub fn display_limits(&self) -> &DisplayLimits {
        &self.profiles[self.active_profile].display
    }

    pub fn is_on_battery(&self) -> bool {
        self.battery.source == PowerSource::Battery
    }

    pub fn battery_pct(&self) -> u8 {
        self.battery.charge_pct
    }
}
