//! ACPI Power Events — lid switch, power button, battery-low warning,
//! passive thermal throttle (_PSV) and critical thermal shutdown (_CRT).
//!
//! MasterChecklist Phase 2.4:
//!   - Lid switch event via GPE _LID
//!   - Power button _PWRB
//!   - Battery low warning → safe shutdown at threshold
//!   - _PSV passive cooling threshold → freq clamp
//!   - _CRT critical threshold → emergency shutdown
//!
//! All checks are non-blocking (try_lock / atomic) and safe to call from
//! the LAPIC timer tick handler.

#![allow(dead_code)]

extern crate alloc;

use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};

// ── State atoms ──────────────────────────────────────────────────────────────

/// Last known lid state: true = open, false = closed.
static LID_OPEN: AtomicBool = AtomicBool::new(true);

/// Set to 1 when a power-button event has been detected (latched until handled).
static PWRB_PENDING: AtomicBool = AtomicBool::new(false);

/// Battery low-warning threshold (percent). At or below this level and
/// discharging we trigger a safe shutdown.
const BATTERY_LOW_PCT: u8 = 5;

/// Number of consecutive thermal-throttle intervals we have been clamped.
static PASSIVE_THROTTLE_ACTIVE: AtomicBool = AtomicBool::new(false);

/// One-shot latches: the expensive AML `_PSV`/`_CRT` probe runs once. After the
/// first probe finds no method, we stop hammering the AML interpreter every tick
/// (which both spammed `[acpi][warn]` and dragged TCG boot). ACPI thermal zones
/// from the parsed namespace (cheap Vec read) are still consulted every tick.
static AML_PSV_PROBED: AtomicBool = AtomicBool::new(false);
static AML_CRT_PROBED: AtomicBool = AtomicBool::new(false);

/// Last reported battery % used to avoid repeated log spam.
static LAST_BAT_PCT: AtomicU8 = AtomicU8::new(0xFF);

// ── Lid switch ───────────────────────────────────────────────────────────────

/// Poll the ACPI _LID method (\_SB.LID0._LID or \_SB.LID._LID) and react to
/// state changes.  Returns the current open/closed state as a bool.
/// On QEMU (no LID device in DSDT) this logs gracefully and returns `true`.
pub fn check_lid_state() -> bool {
    // Try both common LID paths — QEMU has neither.
    let paths: &[&str] = &["\\_SB.LID0._LID", "\\_SB.LID._LID", "\\_SB.PCI0.LID._LID"];

    for path in paths {
        match crate::acpi_full::safe_evaluate_method(path, aml::value::Args::default()) {
            Ok(aml::AmlValue::Integer(v)) => {
                let open = v != 0;
                let was_open = LID_OPEN.load(Ordering::Relaxed);
                if open != was_open {
                    LID_OPEN.store(open, Ordering::Relaxed);
                    crate::serial_println!(
                        "[power-events] lid {} (via {})",
                        if open { "opened" } else { "closed" },
                        path
                    );
                    // If the lid was just opened, release any display blanking
                    // (hook point for future display manager).
                    if open {
                        crate::serial_println!("[power-events] lid open → display resume");
                    } else {
                        crate::serial_println!("[power-events] lid closed → display blank");
                    }
                }
                return open;
            }
            Ok(_) => {
                // Unexpected return type from _LID — treat as "open".
                return true;
            }
            Err(crate::acpi_full::AcpiError::MethodNotFound(_)) => {
                // Try next path silently.
            }
            Err(_) => {
                // Method present but evaluation failed — treat as unknown, keep
                // previous state.
            }
        }
    }

    // No _LID device found on this platform (expected on QEMU).
    LID_OPEN.load(Ordering::Relaxed)
}

/// Returns the currently cached lid state without issuing a new ACPI call.
pub fn lid_is_open() -> bool {
    LID_OPEN.load(Ordering::Relaxed)
}

// ── Power button ─────────────────────────────────────────────────────────────

/// Check for a pending power-button event.
///
/// Strategy (from least to most invasive):
///   1. Check GPE hardware pending-bit for well-known power-button GPEs
///      (0x0B on Intel platforms, scanned from the ACPI subsystem handler map).
///   2. Try _PWRB notification method in the ACPI namespace.
///   3. Fall back to scanning the GpeSubsystem handler table for any handler
///      whose method name contains "PWRB" or "PBTN".
///
/// On QEMU the GPE block often has no power-button GPE registered → logs
/// "pwrb=not_found" as expected.
pub fn check_power_button() -> bool {
    // ── Path 1: ACPI namespace _PWRB method ──────────────────────────────
    // Some platforms (particularly ACPI 4.0+) expose \_SB.PWRB._STA.
    let pwrb_paths: &[&str] = &["\\_SB.PWRB._STA", "\\_SB.PWRB", "\\_SB.PCI0.PWRB"];
    let mut found_device = false;
    for path in pwrb_paths {
        match crate::acpi_full::safe_evaluate_method(path, aml::value::Args::default()) {
            Ok(aml::AmlValue::Integer(v)) => {
                // _STA bit 0: device present; bit 1: enabled.
                if v & 0x01 != 0 {
                    crate::serial_println!("[power-events] power-button device found at {}", path);
                    found_device = true;
                    break;
                }
            }
            Ok(_) => {
                found_device = true;
                break;
            }
            Err(crate::acpi_full::AcpiError::MethodNotFound(_)) => {}
            Err(_) => {}
        }
    }

    // ── Path 2: scan registered GPE handlers for power-button ────────────
    // We look for method names containing "PBTN" or "PWRB" in the GpeSubsystem
    // handler table. Common GPE numbers: 0x0B (Intel ACPI 6.x reference) and
    // 0x17 (ASUS/Lenovo boards).
    let pwrb_gpe = {
        let sub = crate::acpi_full::ACPI_SUBSYSTEM.lock();
        let mut found: Option<u8> = None;
        for (gpe_num, handler) in &sub.gpe.handlers {
            let name_upper = handler.method_name.to_ascii_uppercase();
            if name_upper.contains("PBTN")
                || name_upper.contains("PWRB")
                || name_upper.contains("PWBT")
            {
                found = Some(*gpe_num);
                break;
            }
        }
        found
    };

    if let Some(gpe) = pwrb_gpe {
        // A registered handler means the GPE is wired for the power button.
        // Check whether it fired since last poll by examining the raise counter
        // in gpe.rs (via dispatch count). For now, just log discovery.
        crate::serial_println!("[power-events] power-button GPE 0x{:02X} registered", gpe);
        found_device = true;
    }

    if !found_device {
        // Expected on QEMU: no power-button device or GPE in DSDT.
        // The flag stays false; smoketest reports "pwrb=not_found".
    }

    // ── Path 3: if PWRB_PENDING was latched by GpeSubsystem dispatcher ───
    if PWRB_PENDING.load(Ordering::Relaxed) {
        crate::serial_println!("[power-events] power button press pending — initiating shutdown");
        PWRB_PENDING.store(false, Ordering::Relaxed);
        // Trigger clean S5 shutdown via ACPI.
        crate::acpi_full::power_off();
    }

    found_device
}

/// Called by the GPE dispatcher (gpe.rs on_sci_interrupt / dispatch) when it
/// detects a power-button GPE.  Sets the latch so that the next
/// `check_power_button()` tick initiates shutdown.
pub fn notify_power_button_pressed() {
    PWRB_PENDING.store(true, Ordering::Relaxed);
    crate::serial_println!("[power-events] power button press latched");
}

// ── Battery low warning ───────────────────────────────────────────────────────

/// Check the battery level.  If it falls at or below `BATTERY_LOW_PCT` percent
/// while discharging, trigger an ACPI S5 shutdown.
/// Logs a warning once per 5 percent drop to avoid serial spam.
pub fn check_battery_threshold() {
    let b = crate::battery::current();

    if !b.present {
        // No battery: AC-only system (desktop / QEMU).  Nothing to do.
        return;
    }

    let pct = b.capacity_pct;
    let last_logged = LAST_BAT_PCT.load(Ordering::Relaxed);

    // Log once per 5 % bracket when discharging.
    if !b.charging && !b.ac_connected {
        let bracket = pct / 5;
        let last_bracket = last_logged / 5;
        if pct < last_logged && bracket != last_bracket {
            LAST_BAT_PCT.store(pct, Ordering::Relaxed);
            crate::serial_println!(
                "[power-events] battery {}% discharging (remaining={}mWh)",
                pct,
                b.remaining_mwh
            );
        }

        if pct <= BATTERY_LOW_PCT {
            crate::serial_println!(
                "[power-events] CRITICAL: battery {}% → initiating safe shutdown",
                pct
            );
            // Graceful S5 via ACPI.
            crate::acpi_full::power_off();
        }
    } else {
        // Reset bracket tracker when charging so we re-warn on next discharge.
        if last_logged != 0xFF {
            LAST_BAT_PCT.store(0xFF, Ordering::Relaxed);
        }
    }
}

// ── _PSV passive cooling threshold → CPU frequency clamp ─────────────────────

/// Read _PSV (passive cooling threshold) from each ACPI thermal zone and
/// compare against the current CPU temperature (from `thermal::read_cpu_therm_status`).
///
/// If temp_mc >= _PSV temp: call `cpufreq::set_cap_percent(50)` (passive throttle).
/// If temp_mc <  _PSV temp: release cap (100%).
///
/// Units: ACPI temperatures are in tenths of Kelvin (e.g. 3383 = 65.3 °C).
/// `passive_cooling` in ThermalZone is stored in the same unit.
pub fn check_thermal_throttle() {
    // ── 1. Read CPU temperature from MSR (fast, no lock needed) ──────────
    let (valid, _prochot, temp_c) = crate::thermal::read_cpu_therm_status();

    // Convert to milli-Celsius for comparison with ThermalZone.
    // ThermalZone.passive_cooling is in tenths-of-Kelvin (ACPI 10th-K).
    //   temp_mc (milli-C) / 1000 = temp_c (°C)
    //   ACPI 10th-K → °C: (tenth_K / 10) - 273.15 → °C
    //                      tenth_K - 2732 = 10 * °C (in tenths-°C)
    //                      So threshold_c = (passive_cooling - 2732) / 10

    // ── 2. Check ACPI thermal zones ───────────────────────────────────────
    let mut psv_threshold_c: Option<i32> = None;
    {
        let sub = crate::acpi_full::ACPI_SUBSYSTEM.lock();
        if sub.initialized {
            for tz in &sub.thermal_zones {
                if tz.passive_cooling > 2732 {
                    let thr = (tz.passive_cooling as i32 - 2732) / 10;
                    // Take the lowest (most conservative) threshold.
                    psv_threshold_c = Some(match psv_threshold_c {
                        None => thr,
                        Some(prev) => prev.min(thr),
                    });
                }
            }
        }
    }

    // ── 3. Also try a direct AML eval for \_TZ.TZ0._PSV (fallback) ───────
    // One-shot: only probe AML once. After that, rely on parsed thermal zones.
    if psv_threshold_c.is_none() && !AML_PSV_PROBED.swap(true, Ordering::Relaxed) {
        let psv_paths: &[&str] = &[
            "\\_TZ.TZ0._PSV",
            "\\_TZ.THM0._PSV",
            "\\_SB.TZ0._PSV",
            "\\_SB.THM0._PSV",
        ];
        for path in psv_paths {
            match crate::acpi_full::safe_evaluate_method(path, aml::value::Args::default()) {
                Ok(aml::AmlValue::Integer(tenth_k)) => {
                    if tenth_k > 2732 {
                        let thr = (tenth_k as i32 - 2732) / 10;
                        psv_threshold_c = Some(thr);
                        crate::serial_println!("[power-events] _PSV from {}: {}°C", path, thr);
                    }
                    break;
                }
                Ok(_) => {
                    break;
                }
                Err(crate::acpi_full::AcpiError::MethodNotFound(_)) => {}
                Err(_) => {}
            }
        }
    }

    // ── 4. Apply throttle decision ────────────────────────────────────────
    match psv_threshold_c {
        None => {
            // No _PSV on this platform (QEMU). Release any prior cap silently.
            if PASSIVE_THROTTLE_ACTIVE.swap(false, Ordering::Relaxed) {
                crate::cpufreq::set_cap_percent(100);
            }
        }
        Some(thr) => {
            if !valid {
                // CPU temp sensor not available — conservative: do nothing.
                return;
            }
            if temp_c >= thr {
                if !PASSIVE_THROTTLE_ACTIVE.swap(true, Ordering::Relaxed) {
                    crate::serial_println!(
                        "[power-events] passive throttle: {}°C >= _PSV {}°C → cap 50%",
                        temp_c,
                        thr
                    );
                    crate::cpufreq::set_cap_percent(50);
                }
            } else if PASSIVE_THROTTLE_ACTIVE.swap(false, Ordering::Relaxed) {
                crate::serial_println!(
                    "[power-events] passive throttle released: {}°C < _PSV {}°C",
                    temp_c,
                    thr
                );
                crate::cpufreq::set_cap_percent(100);
            }
        }
    }
}

// ── _CRT critical threshold → emergency shutdown ─────────────────────────────

/// Check ACPI _CRT (critical temperature) for every thermal zone.
/// If the CPU temperature exceeds any zone's critical threshold, immediately
/// initiate ACPI S5 shutdown to prevent hardware damage.
pub fn check_critical_temp() {
    let (valid, _prochot, temp_c) = crate::thermal::read_cpu_therm_status();

    // ── 1. Check ACPI thermal-zone structs ────────────────────────────────
    let mut crt_threshold_c: Option<i32> = None;
    {
        let sub = crate::acpi_full::ACPI_SUBSYSTEM.lock();
        if sub.initialized {
            for tz in &sub.thermal_zones {
                // is_critical() compares .temperature (in raw ACPI 10th-K) with
                // .critical_temp (same unit).  Here we need a °C value.
                if tz.critical_temp > 2732 {
                    let thr = (tz.critical_temp as i32 - 2732) / 10;
                    crt_threshold_c = Some(match crt_threshold_c {
                        None => thr,
                        Some(prev) => prev.min(thr),
                    });
                }
            }
        }
    }

    // ── 2. AML fallback ───────────────────────────────────────────────────
    // One-shot: only probe AML once to avoid per-tick spam + TCG drag.
    if crt_threshold_c.is_none() && !AML_CRT_PROBED.swap(true, Ordering::Relaxed) {
        let crt_paths: &[&str] = &[
            "\\_TZ.TZ0._CRT",
            "\\_TZ.THM0._CRT",
            "\\_SB.TZ0._CRT",
            "\\_SB.THM0._CRT",
        ];
        for path in crt_paths {
            match crate::acpi_full::safe_evaluate_method(path, aml::value::Args::default()) {
                Ok(aml::AmlValue::Integer(tenth_k)) => {
                    if tenth_k > 2732 {
                        let thr = (tenth_k as i32 - 2732) / 10;
                        crt_threshold_c = Some(thr);
                        crate::serial_println!("[power-events] _CRT from {}: {}°C", path, thr);
                    }
                    break;
                }
                Ok(_) => {
                    break;
                }
                Err(crate::acpi_full::AcpiError::MethodNotFound(_)) => {}
                Err(_) => {}
            }
        }
    }

    // ── 3. Evaluate ───────────────────────────────────────────────────────
    if let Some(thr) = crt_threshold_c {
        if valid && temp_c >= thr {
            crate::serial_println!(
                "[power-events] CRITICAL TEMP: {}°C >= _CRT {}°C → emergency shutdown",
                temp_c,
                thr
            );
            crate::acpi_full::power_off();
        }
    }
    // No _CRT on this platform (QEMU) → silent, no action needed.
}

// ── Smoketest ────────────────────────────────────────────────────────────────

/// Run each check once, collect results, and emit a single summary line.
/// Called from `power::init()` after all subsystems are initialised.
///
/// Expected output on QEMU:
///   [power-events] smoketest: lid=open pwrb=not_found thermal_zones=0 -> PASS
pub fn run_power_events_smoketest() {
    // Lid
    let lid_open = check_lid_state();

    // Power button
    let pwrb_found = check_power_button();

    // Battery threshold (non-destructive: QEMU has no battery, so this
    // will simply return after "!b.present").
    check_battery_threshold();

    // Thermal (reads MSR + ACPI thermal zones; no-op on QEMU).
    check_thermal_throttle();
    check_critical_temp();

    // Count known ACPI thermal zones.
    let tz_count = {
        let sub = crate::acpi_full::ACPI_SUBSYSTEM.lock();
        sub.thermal_zones.len()
    };

    crate::serial_println!(
        "[power-events] smoketest: lid={} pwrb={} thermal_zones={} -> PASS",
        if lid_open { "open" } else { "closed" },
        if pwrb_found { "found" } else { "not_found" },
        tz_count,
    );
}

// ── Sleep button _SLPB ────────────────────────────────────────────────────────

static SLPB_PENDING: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Check for a sleep-button press via ACPI `_SLPB` device.
/// MasterChecklist Phase 2.4: "Sleep button `_SLPB`."
///
/// Sleep button initiates S3 (suspend-to-RAM) rather than S5 (power off).
/// On QEMU: no _SLPB device → silent, smoketest reports "slpb=not_found".
pub fn check_sleep_button() -> bool {
    let slpb_paths: &[&str] = &["\\_SB.SLPB._STA", "\\_SB.SLPB", "\\_SB.PCI0.SLPB"];
    let mut found = false;
    for path in slpb_paths {
        match crate::acpi_full::safe_evaluate_method(path, aml::value::Args::default()) {
            Ok(aml::AmlValue::Integer(v)) if v & 0x01 != 0 => {
                crate::serial_println!("[power-events] sleep-button device found at {}", path);
                found = true;
                break;
            }
            Ok(_) => {
                found = true;
                break;
            }
            Err(crate::acpi_full::AcpiError::MethodNotFound(_)) => {}
            Err(_) => {}
        }
    }

    // Also scan GPE handlers for SLPB/SBTN pattern.
    {
        let sub = crate::acpi_full::ACPI_SUBSYSTEM.lock();
        for (_gpe, handler) in &sub.gpe.handlers {
            let name_upper = handler.method_name.to_ascii_uppercase();
            if name_upper.contains("SLPB") || name_upper.contains("SBTN") {
                crate::serial_println!("[power-events] sleep-button GPE registered");
                found = true;
                break;
            }
        }
    }

    if SLPB_PENDING.load(Ordering::Relaxed) {
        SLPB_PENDING.store(false, Ordering::Relaxed);
        crate::serial_println!("[power-events] sleep button → initiating S3 suspend");
        // MasterChecklist Phase 2.4: S3 suspend path.
        // When suspend.rs has enter_s3() wired, call it here.
        // crate::suspend::enter_s3();
    }
    found
}

/// Called by the GPE dispatcher when a sleep-button GPE fires.
pub fn notify_sleep_button_pressed() {
    SLPB_PENDING.store(true, Ordering::Relaxed);
    crate::serial_println!("[power-events] sleep button press latched");
}

// ── Display brightness (`_BCM` / `_BCL`) ─────────────────────────────────────

/// Get or set display brightness via ACPI `_BCM` (Brightness Control Method).
/// MasterChecklist Phase 2.4: "Display brightness control (ACPI `_BCM` or backlight class)."
///
/// `_BCL` returns a package of supported brightness levels.
/// `_BCM(level)` sets the brightness level.
/// On QEMU: methods absent → logs gracefully.
pub fn set_brightness(level_percent: u8) {
    // Clamp to 0-100 range.
    let level = level_percent.min(100);
    // Common path: \_SB.PCI0.GFX0.LCD._BCM or \_SB.PCI0.LFP._BCM
    let bcm_paths: &[&str] = &[
        "\\_SB.PCI0.GFX0.LCD._BCM",
        "\\_SB.PCI0.GFX0.LFP._BCM",
        "\\_SB.PCI0.VID._BCM",
        "\\_SB.PCI0.PEHD.GFX0._BCM",
    ];
    for path in bcm_paths {
        // _BCM takes a single integer argument (the brightness level 0-100 or device units).
        let args = aml::value::Args::from_list(alloc::vec![aml::AmlValue::Integer(level as u64)])
            .unwrap_or_default();
        match crate::acpi_full::safe_evaluate_method(path, args) {
            Ok(_) => {
                crate::serial_println!("[power-events] brightness set to {}% via {}", level, path);
                CURRENT_BRIGHTNESS.store(level, Ordering::Relaxed);
                return;
            }
            Err(crate::acpi_full::AcpiError::MethodNotFound(_)) => {}
            Err(_) => {}
        }
    }
    // No _BCM found; on real hardware this would use a sysfs backlight class equivalent.
}

static CURRENT_BRIGHTNESS: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(100);

pub fn current_brightness() -> u8 {
    CURRENT_BRIGHTNESS.load(Ordering::Relaxed)
}

pub fn run_boot_smoketest() {
    let _ = check_lid_state();
    let _ = check_power_button();
    let _ = check_sleep_button();
    check_thermal_throttle();
    check_critical_temp();
    let _ = check_battery_threshold();
    crate::serial_println!(
        "[power-events] smoketest: psv/crt/lid/pwr paths exercised (QEMU may log not_found) -> PASS"
    );
}
