// kernel/src/rtc.rs
//
// CMOS Real-Time Clock — boot-time wall-clock reader.
//
// Concept §System tray + Time: "Real-time wall-clock + system tray clock"
// (kernelchecklist §65). Without this, every gettimeofday returns 0 and
// timestamps across the kernel (audit ring, WireGuard handshake times,
// TLS session creation) start from the same constant — which makes
// log forensics impossible.
//
// What this module does:
//   1. Reads the CMOS RTC over I/O ports 0x70/0x71 at boot, normalizes
//      BCD-vs-binary and 12/24-hour formats, and computes a Unix epoch.
//   2. Captures the HPET TSC reading at the same moment so subsequent
//      wall-clock queries can be derived as boot_epoch + (now_hpet -
//      boot_hpet) without re-polling the RTC (slow + can stall on
//      update-in-progress).
//   3. Exposes seconds_since_epoch_now() for callers + dump_text() for
//      /proc/athena/rtc.
//
// What this is NOT:
//   • A time-of-day timezone handler. Wall clock returned is UTC iff
//     the RTC is in UTC, otherwise local. Real world: 50/50. We log
//     the raw reading and let userspace apply the timezone.
//   • A NTP client. That belongs in athnet userspace.

#![allow(dead_code)]

use spin::Mutex;
use x86_64::instructions::port::Port;

const CMOS_ADDR: u16 = 0x70;
const CMOS_DATA: u16 = 0x71;

#[derive(Debug, Clone, Copy, Default)]
pub struct WallClock {
    pub year: u32,  // full 4-digit year (e.g. 2026)
    pub month: u8,  // 1..=12
    pub day: u8,    // 1..=31
    pub hour: u8,   // 0..=23
    pub minute: u8, // 0..=59
    pub second: u8, // 0..=59
}

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum TimeSource {
    #[default]
    InvariantTsc,
    Hpet,
    PitTsc,
}

#[derive(Default)]
struct RtcState {
    boot_wall: WallClock,
    boot_epoch: u64,
    boot_tsc: u64,
    tsc_per_sec: u64,
    time_source: TimeSource,
    boot_hpet: u64,
    hpet_period_fs: u32,
}

static RTC: Mutex<RtcState> = Mutex::new(RtcState {
    boot_wall: WallClock {
        year: 0,
        month: 0,
        day: 0,
        hour: 0,
        minute: 0,
        second: 0,
    },
    boot_epoch: 0,
    boot_tsc: 0,
    tsc_per_sec: 0,
    time_source: TimeSource::InvariantTsc,
    boot_hpet: 0,
    hpet_period_fs: 0,
});

#[inline(always)]
unsafe fn read_cmos(reg: u8) -> u8 {
    let mut addr: Port<u8> = Port::new(CMOS_ADDR);
    let mut data: Port<u8> = Port::new(CMOS_DATA);
    addr.write(reg);
    data.read()
}

#[inline(always)]
fn bcd_to_bin(v: u8) -> u8 {
    (v & 0x0F) + ((v >> 4) * 10)
}

/// Wait for the RTC's "update in progress" bit to clear before reading.
unsafe fn wait_update_finish() {
    // Bit 7 of Status Register A = update-in-progress. Spin until clear.
    // Cap iterations so a wedged RTC can't hang boot.
    for _ in 0..1_000_000 {
        if (read_cmos(0x0A) & 0x80) == 0 {
            return;
        }
    }
}

/// Read the CMOS RTC, normalizing BCD/binary and 12/24-hour formats.
unsafe fn read_wall_clock() -> WallClock {
    wait_update_finish();

    let s = read_cmos(0x00);
    let m = read_cmos(0x02);
    let h = read_cmos(0x04);
    let d = read_cmos(0x07);
    let mo = read_cmos(0x08);
    let y = read_cmos(0x09);
    let century = read_cmos(0x32); // may be 0 on some platforms

    let reg_b = read_cmos(0x0B);
    let bcd = reg_b & 0x04 == 0;
    let twelve_h = reg_b & 0x02 == 0;

    let mut sec = if bcd { bcd_to_bin(s) } else { s };
    let mut min = if bcd { bcd_to_bin(m) } else { m };
    let raw_h = if bcd { bcd_to_bin(h & 0x7F) } else { h & 0x7F };
    let pm = (h & 0x80) != 0;
    let mut hour = if twelve_h {
        let h12 = if raw_h == 12 { 0 } else { raw_h };
        if pm {
            h12 + 12
        } else {
            h12
        }
    } else {
        raw_h
    };
    let mut day = if bcd { bcd_to_bin(d) } else { d };
    let mut month = if bcd { bcd_to_bin(mo) } else { mo };
    let yr = if bcd { bcd_to_bin(y) } else { y } as u32;
    let cen = if century != 0 {
        if bcd {
            bcd_to_bin(century) as u32
        } else {
            century as u32
        }
    } else {
        // No century register — assume 20xx for years < 70, else 19xx.
        if yr < 70 {
            20
        } else {
            19
        }
    };

    if sec >= 60 {
        sec = 0;
    }
    if min >= 60 {
        min = 0;
    }
    if hour >= 24 {
        hour = 0;
    }
    if day == 0 {
        day = 1;
    }
    if month == 0 || month > 12 {
        month = 1;
    }

    WallClock {
        year: cen * 100 + yr,
        month,
        day,
        hour,
        minute: min,
        second: sec,
    }
}

/// Convert a `WallClock` (assumed UTC) to seconds since the Unix epoch
/// (1970-01-01 00:00:00 UTC). Civil-from-days algorithm (Howard Hinnant).
fn wall_to_unix_epoch(w: &WallClock) -> u64 {
    let y = if w.month <= 2 {
        w.year as i64 - 1
    } else {
        w.year as i64
    };
    let m = w.month as i64;
    let d = w.day as i64;
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe = (y - era * 400) as i64; // [0, 399]
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    let days = era * 146097 + doe - 719468;
    let s_per_day = 86_400i64;
    let secs = days * s_per_day + w.hour as i64 * 3600 + w.minute as i64 * 60 + w.second as i64;
    secs.max(0) as u64
}

pub fn init() {
    let (wall, tsc) = unsafe {
        let w = read_wall_clock();
        let t = core::arch::x86_64::_rdtsc();
        (w, t)
    };
    let epoch = wall_to_unix_epoch(&wall);

    // apic::TSC_FREQ_MHZ is populated by calibrate_tsc(). Convert MHz→Hz.
    let tsc_per_sec = crate::apic::TSC_FREQ_MHZ
        .load(core::sync::atomic::Ordering::Relaxed)
        .saturating_mul(1_000_000);

    {
        let mut s = RTC.lock();
        s.boot_wall = wall;
        s.boot_epoch = epoch;
        s.boot_tsc = tsc;
        s.tsc_per_sec = tsc_per_sec;
        if tsc_is_invariant() {
            s.time_source = TimeSource::InvariantTsc;
        } else {
            // HPET init (ACPI) may switch this to TimeSource::Hpet shortly.
            s.time_source = TimeSource::PitTsc;
        }
    }

    crate::serial_println!(
        "[ OK ] RTC wall-clock: {:04}-{:02}-{:02} {:02}:{:02}:{:02} UTC = epoch {}",
        wall.year,
        wall.month,
        wall.day,
        wall.hour,
        wall.minute,
        wall.second,
        epoch,
    );

    if tsc_is_invariant() {
        crate::serial_println!("[ OK ] TSC invariant (safe across CPU migration)");
    } else {
        crate::serial_println!(
            "[WARN] TSC not invariant — wall clock will use HPET when available (PIT/TSC interim)"
        );
    }
}

/// ACPI HPET init succeeded. Switch wall-clock delta to HPET when TSC is not invariant.
pub fn on_hpet_ready(period_fs: u32, boot_counter: u64) {
    if tsc_is_invariant() {
        return;
    }
    let mut s = RTC.lock();
    s.time_source = TimeSource::Hpet;
    s.boot_hpet = boot_counter;
    s.hpet_period_fs = period_fs;
    crate::serial_println!(
        "[rtc] wall clock source: HPET (TSC non-invariant; period {} fs)",
        period_fs,
    );
}

/// HPET table missing — keep RTC anchor, document PIT/TSC fallback path.
pub fn on_hpet_absent() {
    if tsc_is_invariant() {
        return;
    }
    let mut s = RTC.lock();
    if s.time_source != TimeSource::Hpet {
        s.time_source = TimeSource::PitTsc;
        crate::serial_println!(
            "[rtc] wall clock source: PIT-calibrated TSC (HPET absent, TSC non-invariant)"
        );
    }
}

/// CPUID 0x80000007:EDX[8] — Invariant TSC (Intel/AMD).
fn tsc_is_invariant() -> bool {
    let max_ext = unsafe { core::arch::x86_64::__cpuid(0x8000_0000).eax };
    if max_ext < 0x8000_0007 {
        return false;
    }
    let r = unsafe { core::arch::x86_64::__cpuid(0x8000_0007) };
    r.edx & (1 << 8) != 0
}

/// Alias for HPET fallback and other subsystems.
pub fn now_epoch_secs() -> u64 {
    seconds_since_epoch_now()
}

/// Seconds since Unix epoch as of *now* — derived from the boot epoch
/// + TSC or HPET delta, not from re-reading the RTC (which can stall on update).
pub fn seconds_since_epoch_now() -> u64 {
    let s = RTC.lock();
    match s.time_source {
        TimeSource::Hpet if s.hpet_period_fs > 0 => {
            let now = crate::hpet::read_counter().unwrap_or(s.boot_hpet);
            let delta_ticks = now.wrapping_sub(s.boot_hpet);
            let delta_fs = delta_ticks.saturating_mul(s.hpet_period_fs as u64);
            let delta_secs = delta_fs / 1_000_000_000_000_000;
            s.boot_epoch.saturating_add(delta_secs)
        }
        _ => {
            if s.tsc_per_sec == 0 {
                return s.boot_epoch;
            }
            let now = unsafe { core::arch::x86_64::_rdtsc() };
            let delta = now.saturating_sub(s.boot_tsc);
            s.boot_epoch + (delta / s.tsc_per_sec)
        }
    }
}

/// Nanoseconds since Unix epoch — millisecond precision in practice.
pub fn nanos_since_epoch_now() -> u128 {
    let s = RTC.lock();
    if s.tsc_per_sec == 0 {
        return (s.boot_epoch as u128) * 1_000_000_000;
    }
    let now = unsafe { core::arch::x86_64::_rdtsc() };
    let delta = now.saturating_sub(s.boot_tsc);
    let extra_ns = (delta as u128) * 1_000_000_000u128 / (s.tsc_per_sec as u128);
    (s.boot_epoch as u128) * 1_000_000_000 + extra_ns
}

pub fn boot_wall_clock() -> WallClock {
    RTC.lock().boot_wall
}

pub fn run_boot_smoketest() {
    let now = seconds_since_epoch_now();
    let boot = RTC.lock().boot_epoch;
    let delta = now.saturating_sub(boot);
    let pass = boot > 1_500_000_000; // ≈ year 2017+
    crate::serial_println!(
        "[rtc] smoketest: boot_epoch={} now={} delta={}s; sanity={}",
        boot,
        now,
        delta,
        if pass {
            "OK"
        } else {
            "FAIL (epoch suspiciously low)"
        },
    );

    // Round-trip via the same syscall surface userspace uses.
    let ns = crate::game_session::sys_wall_clock();
    let secs = ns / 1_000_000_000;
    let mins = secs / 60;
    let hours = (mins / 60) % 24;
    let mm = mins % 60;
    let ss = secs % 60;
    crate::serial_println!(
        "[rtc] sys_wall_clock -> {} ns = {:02}:{:02}:{:02} UTC (tray-ready format)",
        ns,
        hours,
        mm,
        ss,
    );
}

pub fn dump_text() -> alloc::string::String {
    use alloc::string::String;
    // Compute the current epoch FIRST: seconds_since_epoch_now() locks RTC
    // internally, so calling it while holding the lock below would re-enter the
    // spin::Mutex and deadlock the core (this froze the /proc/athena/rtc boot
    // dump deterministically once earlier blockers were cleared).
    let now_s = seconds_since_epoch_now();
    let s = RTC.lock();
    let source = match s.time_source {
        TimeSource::Hpet => "hpet",
        TimeSource::PitTsc => "pit-tsc",
        TimeSource::InvariantTsc => "invariant-tsc",
    };
    let mut out = String::new();
    out.push_str(&alloc::format!(
        "# AthenaOS wall clock (CMOS RTC + {} delta)\n\
         boot_wall: {:04}-{:02}-{:02} {:02}:{:02}:{:02} UTC\n\
         boot_epoch: {}\n\
         now_epoch:  {}\n\
         tsc_per_sec: {}\n\
         time_source: {}\n",
        source,
        s.boot_wall.year,
        s.boot_wall.month,
        s.boot_wall.day,
        s.boot_wall.hour,
        s.boot_wall.minute,
        s.boot_wall.second,
        s.boot_epoch,
        now_s,
        s.tsc_per_sec,
        source,
    ));
    out
}
