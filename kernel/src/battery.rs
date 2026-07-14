//! Battery monitor — ACPI _BIF/_BST polling into `power::POWER`.
//!
//! MasterChecklist Phase 2.4 / Tier 1.6.

extern crate alloc;

use alloc::string::String;
use spin::Mutex;

#[derive(Debug, Clone, Copy, Default)]
pub struct BatteryStatus {
    pub present: bool,
    pub charging: bool,
    pub ac_connected: bool,
    pub capacity_pct: u8,
    pub remaining_mwh: u32,
    pub design_mwh: u32,
    pub last_full_mwh: u32,
    pub voltage_mv: u32,
    pub current_ma: i32,
    pub time_remaining_min: u32,
    pub cycle_count: u32,
}

static BATTERY: Mutex<BatteryStatus> = Mutex::new(BatteryStatus {
    present: false,
    charging: false,
    ac_connected: false,
    capacity_pct: 0,
    remaining_mwh: 0,
    design_mwh: 0,
    last_full_mwh: 0,
    voltage_mv: 0,
    current_ma: 0,
    time_remaining_min: 0,
    cycle_count: 0,
});

pub fn init() {
    poll();
    crate::serial_println!("[ OK ] Battery monitor (ACPI _BST/_BIF)");
}

/// Refresh from ACPI and mirror into the global power manager.
pub fn poll() {
    let mut acpi = crate::acpi_full::ACPI_SUBSYSTEM.lock();
    if !acpi.initialized {
        return;
    }
    acpi.update_battery_status();

    if let Some(mgr) = crate::power::POWER.lock().as_ref() {
        let b = &mgr.battery;
        let mut snap = BATTERY.lock();
        snap.present = b.present;
        snap.charging = b.charging;
        snap.ac_connected = b.charging || b.voltage_mv > 0;
        snap.capacity_pct = b.percent;
        snap.voltage_mv = b.voltage_mv;
        snap.current_ma = b.current_ma;
        snap.design_mwh = b.design_capacity_mah;
        snap.last_full_mwh = b.full_capacity_mah;
        snap.remaining_mwh = (b.percent as u32).saturating_mul(b.full_capacity_mah) / 100;
    }
}

pub fn current() -> BatteryStatus {
    *BATTERY.lock()
}

pub fn run_boot_smoketest() {
    poll();
    let s = current();
    crate::serial_println!(
        "[battery] smoketest: present={} cap={}% charging={}",
        s.present,
        s.capacity_pct,
        s.charging,
    );
}

pub fn dump_text() -> String {
    let s = current();
    alloc::format!(
        "# battery\npresent: {}\nac: {}\ncharging: {}\ncapacity: {}%\nremaining_mwh: {}\nvoltage_mv: {}\ncurrent_ma: {}\n",
        s.present,
        s.ac_connected,
        s.charging,
        s.capacity_pct,
        s.remaining_mwh,
        s.voltage_mv,
        s.current_ma,
    )
}
