#![allow(dead_code)]

extern crate alloc;

use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerState {
    Running,
    Idle,
    Suspend,
    Hibernate,
    Shutdown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CpuFreqGovernor {
    Performance,
    Powersave,
    Ondemand,
    Schedutil,
}

/// Fan curve mapping temperature thresholds to fan speeds.
/// Each point is (temp_c, fan_percent).
pub struct FanCurve {
    points: Vec<(u8, u8)>,
}

impl FanCurve {
    pub fn new() -> Self {
        Self { points: Vec::new() }
    }

    pub fn add_point(&mut self, temp_c: u8, fan_percent: u8) {
        self.points.push((temp_c, fan_percent));
        self.points.sort_by_key(|&(t, _)| t);
    }

    /// Linear interpolation between curve points.
    pub fn fan_speed_for(&self, temp: u8) -> u8 {
        if self.points.is_empty() {
            return 100;
        }
        if temp <= self.points[0].0 {
            return self.points[0].1;
        }
        if temp >= self.points[self.points.len() - 1].0 {
            return self.points[self.points.len() - 1].1;
        }
        for window in self.points.windows(2) {
            let (t0, f0) = window[0];
            let (t1, f1) = window[1];
            if temp >= t0 && temp <= t1 {
                if t1 == t0 {
                    return f0;
                }
                let range_t = (t1 - t0) as u32;
                let range_f = (f1 as i32) - (f0 as i32);
                let offset = (temp - t0) as u32;
                return (f0 as i32 + (range_f * offset as i32 / range_t as i32)) as u8;
            }
        }
        100
    }
}

#[derive(Debug, Clone)]
pub struct ThermalZone {
    pub name: &'static str,
    pub temp_mc: i32,
    pub critical_mc: i32,
    pub throttle_mc: i32,
}

#[derive(Debug, Clone)]
pub struct BatteryInfo {
    pub present: bool,
    pub percent: u8,
    pub charging: bool,
    pub voltage_mv: u32,
    pub current_ma: i32,
    pub design_capacity_mah: u32,
    pub full_capacity_mah: u32,
    pub time_to_empty_min: Option<u32>,
    pub time_to_full_min: Option<u32>,
}

impl BatteryInfo {
    fn none() -> Self {
        Self {
            present: false,
            percent: 0,
            charging: false,
            voltage_mv: 0,
            current_ma: 0,
            design_capacity_mah: 0,
            full_capacity_mah: 0,
            time_to_empty_min: None,
            time_to_full_min: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerProfile {
    Performance,
    Balanced,
    PowerSaver,
    Gaming,
}

pub struct PowerManager {
    pub state: PowerState,
    pub governor: CpuFreqGovernor,
    pub profile: PowerProfile,
    pub thermal_zones: Vec<ThermalZone>,
    pub battery: BatteryInfo,
    pub fan_curve: FanCurve,
    pub bg_throttle_gaming: bool,
}

impl PowerManager {
    pub fn new() -> Self {
        let mut fan = FanCurve::new();
        fan.add_point(30, 0);
        fan.add_point(50, 30);
        fan.add_point(70, 60);
        fan.add_point(85, 100);

        Self {
            state: PowerState::Running,
            governor: CpuFreqGovernor::Schedutil,
            profile: PowerProfile::Balanced,
            thermal_zones: Vec::new(),
            battery: BatteryInfo::none(),
            fan_curve: fan,
            bg_throttle_gaming: false,
        }
    }

    pub fn set_profile(&mut self, profile: PowerProfile) {
        self.profile = profile;
        match profile {
            PowerProfile::Performance => self.governor = CpuFreqGovernor::Performance,
            PowerProfile::Balanced => self.governor = CpuFreqGovernor::Schedutil,
            PowerProfile::PowerSaver => self.governor = CpuFreqGovernor::Powersave,
            PowerProfile::Gaming => {
                self.governor = CpuFreqGovernor::Performance;
                self.bg_throttle_gaming = true;
            }
        }
    }

    pub fn set_governor(&mut self, governor: CpuFreqGovernor) {
        self.governor = governor;
    }

    pub fn update_thermals(&mut self) {
        // Stub: real implementation reads ACPI thermal zone registers
        // or embedded-controller temperature sensors.
    }

    pub fn should_throttle(&self) -> bool {
        self.thermal_zones
            .iter()
            .any(|z| z.temp_mc >= z.throttle_mc)
    }

    pub fn request_state(&mut self, state: PowerState) {
        self.state = state;

        let acpi = crate::acpi_full::ACPI_SUBSYSTEM.lock();
        if !acpi.initialized {
            return;
        }

        if acpi.fadt.is_none() {
            return;
        }
        let fadt = acpi.fadt.as_ref().unwrap();

        let sleep_val = match state {
            PowerState::Suspend => 3,   // S3
            PowerState::Hibernate => 4, // S4
            PowerState::Shutdown => 5,  // S5
            _ => return,
        };

        let typ_a = acpi.power_manager.slp_typ_a[sleep_val as usize];
        let typ_b = acpi.power_manager.slp_typ_b[sleep_val as usize];

        // SLP_EN bit is 13
        let val_a = (typ_a as u16) << 10 | (1 << 13);
        let val_b = (typ_b as u16) << 10 | (1 << 13);

        crate::serial_println!(
            "[power] transition to {:?} (S{}, a={:#x}, b={:#x})",
            state,
            sleep_val,
            val_a,
            val_b
        );

        unsafe {
            x86_64::instructions::interrupts::disable();

            if fadt.pm1a_control_block != 0 {
                core::arch::asm!("out dx, ax", in("dx") fadt.pm1a_control_block as u16, in("ax") val_a);
            }
            if fadt.pm1b_control_block != 0 {
                core::arch::asm!("out dx, ax", in("dx") fadt.pm1b_control_block as u16, in("ax") val_b);
            }
        }

        // If we reach here, the transition failed (or it was Suspend and we woke up)
        if state == PowerState::Suspend {
            crate::serial_println!("[power] back from S3 Suspend");
            self.state = PowerState::Running;
        }
    }

    pub fn battery_info(&self) -> &BatteryInfo {
        &self.battery
    }

    pub fn cpu_temp(&self) -> Option<i32> {
        self.thermal_zones.first().map(|z| z.temp_mc)
    }

    pub fn set_fan_curve(&mut self, curve: FanCurve) {
        self.fan_curve = curve;
    }
}

// ═══════════════════════════════════════════════════════════════════════════
//  ACPI Fan Control via Embedded Controller (EC)
// ═══════════════════════════════════════════════════════════════════════════

/// Standard EC register addresses used by many laptop/desktop BIOS implementations.
/// These are common defaults; real hardware may differ.
const EC_FAN_RPM_HI: u8 = 0x84;
const EC_FAN_RPM_LO: u8 = 0x85;
const EC_FAN_TARGET: u8 = 0x94;
const EC_FAN_MODE: u8 = 0x93;
const EC_FAN_MODE_AUTO: u8 = 0x00;
const EC_FAN_MODE_MANUAL: u8 = 0x14;

/// Read the current fan RPM from the Embedded Controller.
/// Returns `None` if the EC is not available.
pub fn read_fan_rpm() -> Option<u16> {
    let acpi = crate::acpi_full::ACPI_SUBSYSTEM.lock();
    let ec = acpi.ec.as_ref()?;
    let rpm = unsafe {
        let hi = ec.read(EC_FAN_RPM_HI).ok()? as u16;
        let lo = ec.read(EC_FAN_RPM_LO).ok()? as u16;
        (hi << 8) | lo
    };
    Some(rpm)
}

/// Write a target fan speed percentage (0-100) to the EC.
/// Puts the fan controller into manual mode first.
pub fn write_fan_speed(percent: u8) {
    let acpi = crate::acpi_full::ACPI_SUBSYSTEM.lock();
    let ec = match acpi.ec.as_ref() {
        Some(e) => e,
        None => return,
    };
    unsafe {
        let _ = ec.write(EC_FAN_MODE, EC_FAN_MODE_MANUAL);
        let speed = ((percent as u16) * 255 / 100) as u8;
        let _ = ec.write(EC_FAN_TARGET, speed);
    }
}

/// Restore automatic fan control via the EC.
pub fn restore_fan_auto() {
    let acpi = crate::acpi_full::ACPI_SUBSYSTEM.lock();
    let ec = match acpi.ec.as_ref() {
        Some(e) => e,
        None => return,
    };
    unsafe {
        let _ = ec.write(EC_FAN_MODE, EC_FAN_MODE_AUTO);
    }
}

/// Apply the fan curve based on current CPU temperature.
/// Reads thermal data and writes the interpolated fan speed to the EC.
pub fn apply_fan_curve(curve: &FanCurve) {
    let (valid, _, temp_c) = crate::thermal::read_cpu_therm_status();
    if !valid || temp_c < 0 {
        return;
    }

    let speed_percent = curve.fan_speed_for(temp_c as u8);
    write_fan_speed(speed_percent);
}

pub static POWER: Mutex<Option<PowerManager>> = Mutex::new(None);
static TELEMETRY_POLLS: AtomicU64 = AtomicU64::new(0);
static LAST_TELEMETRY_TICK: AtomicU64 = AtomicU64::new(0);
const TELEMETRY_POLL_INTERVAL_TICKS: u64 = 100; // ~1s at 100Hz LAPIC tick

// Separate tick counters for power-event polling intervals.
static LAST_SLOW_TICK: AtomicU64 = AtomicU64::new(0); // lid / pwrb / battery
static LAST_THERMAL_TICK: AtomicU64 = AtomicU64::new(0); // thermal throttle / crit
const SLOW_POLL_TICKS: u64 = 100; // lid + power button + battery: every 100 ticks
const THERMAL_POLL_TICKS: u64 = 10; // thermal checks: every 10 ticks

pub fn refresh_battery_from_acpi() {
    crate::battery::poll();
}

pub fn on_timer_tick() {
    let now = crate::timers::JIFFIES.load(Ordering::Relaxed);

    // ── P-state governor (every governor interval) ───────────────────────
    crate::cpufreq::on_timer_tick();

    // ── Telemetry + battery (every 100 ticks) ─────────────────────────────
    {
        let last = LAST_TELEMETRY_TICK.load(Ordering::Relaxed);
        if now.saturating_sub(last) >= TELEMETRY_POLL_INTERVAL_TICKS
            && LAST_TELEMETRY_TICK
                .compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
        {
            // Disabled: ACPI evaluation in hard IRQ context causes heap/serial deadlocks
            // refresh_battery_from_acpi();
            TELEMETRY_POLLS.fetch_add(1, Ordering::Relaxed);
        }
    }

    // ── Lid, power button, battery-low (every SLOW_POLL_TICKS) ───────────
    {
        let last = LAST_SLOW_TICK.load(Ordering::Relaxed);
        if now.saturating_sub(last) >= SLOW_POLL_TICKS
            && LAST_SLOW_TICK
                .compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
        {
            // Disabled: ACPI evaluation in hard IRQ context causes heap/serial deadlocks
            // crate::power_events::check_lid_state();
            // crate::power_events::check_power_button();
            // crate::power_events::check_battery_threshold();
        }
    }

    // ── Thermal throttle + critical-temp (every THERMAL_POLL_TICKS) ──────
    {
        let last = LAST_THERMAL_TICK.load(Ordering::Relaxed);
        if now.saturating_sub(last) >= THERMAL_POLL_TICKS
            && LAST_THERMAL_TICK
                .compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
        {
            // Disabled: ACPI evaluation in hard IRQ context causes heap/serial deadlocks
            // crate::power_events::check_thermal_throttle();
            // crate::power_events::check_critical_temp();
        }
    }
}

pub fn run_boot_smoketest() {
    refresh_battery_from_acpi();
    let polls = TELEMETRY_POLLS.load(Ordering::Relaxed);
    let b = crate::battery::current();
    crate::serial_println!(
        "[power] telemetry: polls={} present={} ac={} charging={} cap={}%",
        polls,
        b.present as u8,
        b.ac_connected as u8,
        b.charging as u8,
        b.capacity_pct
    );
}

pub fn dump_text() -> alloc::string::String {
    let mut out = alloc::string::String::new();
    out.push_str("# power telemetry\n");
    out.push_str(&alloc::format!(
        "polls: {}\ninterval_ticks: {}\n",
        TELEMETRY_POLLS.load(Ordering::Relaxed),
        TELEMETRY_POLL_INTERVAL_TICKS
    ));
    if let Some(mgr) = POWER.lock().as_ref() {
        out.push_str(&alloc::format!(
            "state: {:?}\nprofile: {:?}\ngovernor: {:?}\n",
            mgr.state,
            mgr.profile,
            mgr.governor
        ));
        out.push_str(&alloc::format!(
            "battery_present: {}\nbattery_percent: {}\nbattery_charging: {}\n",
            mgr.battery.present as u8,
            mgr.battery.percent,
            mgr.battery.charging as u8
        ));
    } else {
        out.push_str("state: uninitialized\n");
    }
    let snap = crate::battery::current();
    out.push_str(&alloc::format!(
        "ac_connected: {}\nvoltage_mv: {}\ncurrent_ma: {}\nremaining_mwh: {}\n",
        snap.ac_connected as u8,
        snap.voltage_mv,
        snap.current_ma,
        snap.remaining_mwh
    ));
    out
}

pub fn init() {
    let mut mgr = PowerManager::new();
    mgr.fan_curve.add_point(30, 0);
    mgr.fan_curve.add_point(50, 30);
    mgr.fan_curve.add_point(70, 60);
    mgr.fan_curve.add_point(80, 80);
    mgr.fan_curve.add_point(90, 100);
    *POWER.lock() = Some(mgr);
    // Run ACPI power-event smoketest (lid, pwrb, battery, thermal).
    crate::power_events::run_power_events_smoketest();
}
