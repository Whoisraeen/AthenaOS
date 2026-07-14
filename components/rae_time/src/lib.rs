//! AthenaOS date/time + locale-aware formatting — *"an OS that only works in
//! English is a demo, not a competitor"* (LEGACY_GAMING_CONCEPT.md global-readiness:
//! AthenaOS must rival Windows + macOS in **every** language and locale).
//!
//! This crate is the reusable civil-date/time core plus the i18n formatting
//! surface the whole UI pulls from:
//!
//!   - **Civil date/time core** — the proven Howard-Hinnant `days_from_civil` /
//!     `civil_from_days` algorithms (harvested + host-verified inline in
//!     `apps/clock`), now reusable: [`civil_from_unix`] / [`unix_from_civil`]
//!     round-trip exactly for any epoch (pre-1970 negatives included),
//!     [`is_leap`], [`days_in_month`], [`weekday`].
//!   - **Duration math** — [`Duration`] (seconds) with add/sub onto a
//!     [`CivilDateTime`] and a human "2h 5m" formatter.
//!   - **ISO 8601** — [`format_iso8601`] / [`parse_iso8601`] over the common
//!     `YYYY-MM-DDThh:mm:ss[Z]` subset; never panics, malformed → `Err`.
//!   - **Locale-aware formatting** (the concrete global-readiness gap) — a
//!     [`Locale`] descriptor (date order, 12h/24h, day/month names, first day of
//!     week, decimal/grouping separators) with built-ins, then [`format_date`],
//!     [`format_time`], and [`format_number`] / [`format_integer_grouped`]. The
//!     *same instant* renders "3/14/2024, 2:30 PM" (en-US) vs "14.03.2024, 14:30"
//!     (de-DE) vs "2024/03/14 14:30" (ja-JP).
//!
//! Why its own crate (the `rae_calc` / `rae_tokens` pattern): `apps/clock` is a
//! `#![no_main]` bin that links raekit's `#[panic_handler]`, so `cargo test`
//! inside it trips the duplicate `panic_impl` lang-item gotcha. Factoring the
//! pure logic into a zero-dep `no_std` lib that toggles to `std` under
//! `cfg(test)` gives a clean FAIL-able proof: `cargo test -p rae_time`.
//!
//! NEVER PANICS: every conversion and formatter is total. Bad input (month 0/13,
//! malformed ISO, huge/negative epochs) saturates or returns `Err` — it never
//! unwraps, indexes out of bounds, or overflows in debug.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

use alloc::string::String;

// ───────────────────────────────────────────────────────────────────────────
// Civil date/time core (Howard-Hinnant algorithms; harvested from apps/clock)
// ───────────────────────────────────────────────────────────────────────────

/// Day of the week, ISO-style (Monday-first), but with explicit variants so a
/// caller never has to remember an integer convention.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Weekday {
    Monday,
    Tuesday,
    Wednesday,
    Thursday,
    Friday,
    Saturday,
    Sunday,
}

impl Weekday {
    /// 0 = Sunday .. 6 = Saturday (the C `tm_wday` / civil-day convention used
    /// internally by the Hinnant math and by the calendar grid).
    pub fn sunday_index(self) -> u32 {
        match self {
            Weekday::Sunday => 0,
            Weekday::Monday => 1,
            Weekday::Tuesday => 2,
            Weekday::Wednesday => 3,
            Weekday::Thursday => 4,
            Weekday::Friday => 5,
            Weekday::Saturday => 6,
        }
    }

    /// 0 = Monday .. 6 = Sunday (ISO-8601 convention).
    pub fn monday_index(self) -> u32 {
        match self {
            Weekday::Monday => 0,
            Weekday::Tuesday => 1,
            Weekday::Wednesday => 2,
            Weekday::Thursday => 3,
            Weekday::Friday => 4,
            Weekday::Saturday => 5,
            Weekday::Sunday => 6,
        }
    }

    fn from_sunday_index(i: u32) -> Weekday {
        match i % 7 {
            0 => Weekday::Sunday,
            1 => Weekday::Monday,
            2 => Weekday::Tuesday,
            3 => Weekday::Wednesday,
            4 => Weekday::Thursday,
            5 => Weekday::Friday,
            _ => Weekday::Saturday,
        }
    }
}

/// A broken-down UTC civil date + time. All fields are 1-based where natural
/// (month 1..=12, day 1..=31); time fields are 0-based.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CivilDateTime {
    pub year: i64,
    pub month: u32, // 1..=12
    pub day: u32,   // 1..=31
    pub hour: u32,  // 0..=23
    pub min: u32,   // 0..=59
    pub sec: u32,   // 0..=59
}

impl CivilDateTime {
    /// Day of the week for this date (uses the same serial-day path as
    /// [`weekday`], so it is consistent with [`civil_from_unix`]).
    pub fn weekday(&self) -> Weekday {
        let days = days_from_civil(self.year, self.month, self.day);
        Weekday::from_sunday_index((((days % 7) + 4) % 7 + 7) as u32 % 7)
    }
}

/// Gregorian leap-year rule: divisible by 4, except centuries not divisible by
/// 400.
pub fn is_leap(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
}

/// Days in `month` (1..=12) of `year`, honoring the Gregorian leap rule. An
/// out-of-range month yields 0 (never panics).
pub fn days_in_month(year: i64, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if is_leap(year) {
                29
            } else {
                28
            }
        }
        _ => 0,
    }
}

/// Hinnant's `days_from_civil`: serial day number from 1970-01-01 for a civil
/// date. Total and branch-free; no leap tables.
pub fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400; // [0, 399]
    let m = month as i64;
    let d = day as i64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146_097 + doe - 719_468
}

/// Convert whole Unix seconds (UTC, may be negative for pre-1970) to a civil
/// date+time. Uses Hinnant's `civil_from_days` inverse — branch-free, no leap
/// tables. Total: any `i64` second count decodes without panicking.
pub fn civil_from_unix(secs: i64) -> CivilDateTime {
    // Floor-divide so negative epochs land on the correct civil day and a
    // positive remainder for the time-of-day (Rust `/` and `%` truncate toward
    // zero, which is wrong for negatives).
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400) as u32;
    let hour = rem / 3600;
    let min = (rem % 3600) / 60;
    let sec = rem % 60;

    let (year, month, day) = civil_from_days(days);

    CivilDateTime {
        year,
        month,
        day,
        hour,
        min,
        sec,
    }
}

/// Hinnant's `civil_from_days`: inverse of [`days_from_civil`]. Returns
/// `(year, month, day)` for a serial day number from 1970-01-01.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let month = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32; // [1, 12]
    let year = if month <= 2 { y + 1 } else { y };
    (year, month, day)
}

/// Convert a civil date+time back to whole Unix seconds (UTC). Exact inverse of
/// [`civil_from_unix`] for any in-range date; out-of-range time fields are
/// folded in arithmetically (e.g. `sec = 90` adds 90 seconds), never panicking.
pub fn unix_from_civil(c: &CivilDateTime) -> i64 {
    let days = days_from_civil(c.year, c.month, c.day);
    days * 86_400 + (c.hour as i64) * 3600 + (c.min as i64) * 60 + (c.sec as i64)
}

/// Day of the week for a Unix-second instant (UTC). 1970-01-01 was a Thursday.
pub fn weekday(unix_secs: i64) -> Weekday {
    let days = unix_secs.div_euclid(86_400);
    Weekday::from_sunday_index((((days % 7) + 4) % 7 + 7) as u32 % 7)
}

// ───────────────────────────────────────────────────────────────────────────
// Duration math
// ───────────────────────────────────────────────────────────────────────────

/// A signed span of whole seconds. Arithmetic onto a [`CivilDateTime`] goes
/// through the serial-day representation, so it is calendar-correct (month
/// lengths, leap days, year rollover) without any per-field carry logic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Duration {
    secs: i64,
}

impl Duration {
    pub const fn from_secs(secs: i64) -> Duration {
        Duration { secs }
    }
    pub const fn from_minutes(m: i64) -> Duration {
        Duration { secs: m * 60 }
    }
    pub const fn from_hours(h: i64) -> Duration {
        Duration { secs: h * 3600 }
    }
    pub const fn from_days(d: i64) -> Duration {
        Duration { secs: d * 86_400 }
    }
    pub const fn as_secs(self) -> i64 {
        self.secs
    }

    /// Add this duration to a civil date+time, returning the new instant.
    /// Saturates on `i64` overflow instead of panicking.
    pub fn add_to(self, c: &CivilDateTime) -> CivilDateTime {
        let base = unix_from_civil(c);
        civil_from_unix(base.saturating_add(self.secs))
    }

    /// Subtract this duration from a civil date+time.
    pub fn sub_from(self, c: &CivilDateTime) -> CivilDateTime {
        let base = unix_from_civil(c);
        civil_from_unix(base.saturating_sub(self.secs))
    }

    /// Human, locale-neutral coarse rendering, e.g. `"2h 5m"`, `"3d 4h"`,
    /// `"45s"`, `"0s"`. Negative durations are prefixed with `-`. At most two
    /// significant units, so it stays glanceable.
    pub fn human(self) -> String {
        let mut out = String::new();
        let mut s = self.secs;
        if s < 0 {
            out.push('-');
            s = -s;
        }
        if s == 0 {
            out.push_str("0s");
            return out;
        }
        let days = s / 86_400;
        let hours = (s % 86_400) / 3600;
        let mins = (s % 3600) / 60;
        let secs = s % 60;

        // Pick the two highest non-zero units.
        let units: [(i64, char); 4] = [(days, 'd'), (hours, 'h'), (mins, 'm'), (secs, 's')];
        let mut written = 0;
        for (val, suffix) in units.iter() {
            if *val == 0 && written == 0 {
                continue; // skip leading zero units
            }
            if written >= 2 {
                break;
            }
            if written > 0 {
                out.push(' ');
            }
            push_uint(&mut out, *val as u64);
            out.push(*suffix);
            written += 1;
        }
        out
    }
}

// ───────────────────────────────────────────────────────────────────────────
// ISO 8601 (the `YYYY-MM-DDThh:mm:ss[Z]` subset)
// ───────────────────────────────────────────────────────────────────────────

/// Why an ISO 8601 parse failed. The parser never panics; every malformed input
/// maps to [`ParseError::Malformed`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseError {
    /// The string did not match the `YYYY-MM-DDThh:mm:ss[Z]` shape, or a field
    /// was out of range.
    Malformed,
}

/// Format a civil date+time as ISO 8601 UTC: `YYYY-MM-DDThh:mm:ssZ`. Years are
/// zero-padded to at least 4 digits; the trailing `Z` marks UTC.
pub fn format_iso8601(c: &CivilDateTime) -> String {
    let mut out = String::new();
    push_year(&mut out, c.year);
    out.push('-');
    push_2(&mut out, c.month);
    out.push('-');
    push_2(&mut out, c.day);
    out.push('T');
    push_2(&mut out, c.hour);
    out.push(':');
    push_2(&mut out, c.min);
    out.push(':');
    push_2(&mut out, c.sec);
    out.push('Z');
    out
}

/// Parse the common ISO 8601 subset `YYYY-MM-DDThh:mm:ss[Z]` (a trailing `Z` is
/// optional; a space may stand in for `T`). Returns `Err(ParseError::Malformed)`
/// on any deviation or out-of-range field — never panics.
pub fn parse_iso8601(s: &str) -> Result<CivilDateTime, ParseError> {
    let b = s.as_bytes();
    // Minimum: "YYYY-MM-DDThh:mm:ss" = 19 chars.
    if b.len() < 19 {
        return Err(ParseError::Malformed);
    }

    // Year: exactly 4 ASCII digits (this subset doesn't accept extended/neg years).
    let year = parse_fixed_digits(&b[0..4]).ok_or(ParseError::Malformed)? as i64;
    if b[4] != b'-' {
        return Err(ParseError::Malformed);
    }
    let month = parse_fixed_digits(&b[5..7]).ok_or(ParseError::Malformed)?;
    if b[7] != b'-' {
        return Err(ParseError::Malformed);
    }
    let day = parse_fixed_digits(&b[8..10]).ok_or(ParseError::Malformed)?;
    // Date/time separator: 'T' or a space.
    if b[10] != b'T' && b[10] != b' ' {
        return Err(ParseError::Malformed);
    }
    let hour = parse_fixed_digits(&b[11..13]).ok_or(ParseError::Malformed)?;
    if b[13] != b':' {
        return Err(ParseError::Malformed);
    }
    let min = parse_fixed_digits(&b[14..16]).ok_or(ParseError::Malformed)?;
    if b[16] != b':' {
        return Err(ParseError::Malformed);
    }
    let sec = parse_fixed_digits(&b[17..19]).ok_or(ParseError::Malformed)?;

    // Optional trailing 'Z' — and nothing else after it.
    match b.len() {
        19 => {}
        20 if b[19] == b'Z' => {}
        _ => return Err(ParseError::Malformed),
    }

    // Range-validate every field.
    if month < 1 || month > 12 {
        return Err(ParseError::Malformed);
    }
    if day < 1 || day > days_in_month(year, month) {
        return Err(ParseError::Malformed);
    }
    if hour > 23 || min > 59 || sec > 59 {
        return Err(ParseError::Malformed);
    }

    Ok(CivilDateTime {
        year,
        month,
        day,
        hour,
        min,
        sec,
    })
}

/// Parse a slice of bytes that must all be ASCII digits into a `u32`. Any
/// non-digit byte → `None` (the never-panic guard for the ISO parser).
fn parse_fixed_digits(b: &[u8]) -> Option<u32> {
    let mut v: u32 = 0;
    for &c in b {
        if !c.is_ascii_digit() {
            return None;
        }
        v = v * 10 + (c - b'0') as u32;
    }
    Some(v)
}

// ───────────────────────────────────────────────────────────────────────────
// Locale-aware formatting (the i18n value)
// ───────────────────────────────────────────────────────────────────────────

/// The order in which a date's components are written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DateOrder {
    /// Month / Day / Year (en-US).
    Mdy,
    /// Day / Month / Year (most of Europe).
    Dmy,
    /// Year / Month / Day (ISO, East Asia).
    Ymd,
}

/// 12-hour (AM/PM) vs 24-hour clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClockStyle {
    H12,
    H24,
}

/// A locale descriptor: everything the date/time/number formatters need to
/// render correctly for a given language/region. Built-ins are provided by
/// [`Locale::lookup`]; the fields are public so callers (e.g. a future
/// raelocale table) can synthesize custom locales.
#[derive(Debug, Clone, Copy)]
pub struct Locale {
    /// BCP-47-ish tag, e.g. `"en-US"`.
    pub tag: &'static str,
    pub date_order: DateOrder,
    pub clock: ClockStyle,
    /// Field separator used in the *numeric* date form, e.g. `/`, `.`, `-`.
    pub date_sep: char,
    /// Decimal separator for numbers, e.g. `.` (en-US) or `,` (de-DE).
    pub decimal_sep: char,
    /// Thousands-grouping separator, e.g. `,` (en-US), `.` (de-DE), `'` (some).
    /// `'\0'` disables grouping entirely.
    pub group_sep: char,
    /// First day of the week (calendar grids): Sunday (en-US) or Monday (most).
    pub first_day: Weekday,
    /// Long month names, index 0 = January .. 11 = December.
    pub months: &'static [&'static str; 12],
    /// Long weekday names, index 0 = Sunday .. 6 = Saturday.
    pub weekdays: &'static [&'static str; 7],
    /// AM / PM markers (only used when `clock == H12`).
    pub am_pm: (&'static str, &'static str),
}

const MONTHS_EN: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];
const WEEKDAYS_EN: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];

const MONTHS_DE: [&str; 12] = [
    "Januar",
    "Februar",
    "März",
    "April",
    "Mai",
    "Juni",
    "Juli",
    "August",
    "September",
    "Oktober",
    "November",
    "Dezember",
];
const WEEKDAYS_DE: [&str; 7] = [
    "Sonntag",
    "Montag",
    "Dienstag",
    "Mittwoch",
    "Donnerstag",
    "Freitag",
    "Samstag",
];

const MONTHS_FR: [&str; 12] = [
    "janvier",
    "février",
    "mars",
    "avril",
    "mai",
    "juin",
    "juillet",
    "août",
    "septembre",
    "octobre",
    "novembre",
    "décembre",
];
const WEEKDAYS_FR: [&str; 7] = [
    "dimanche", "lundi", "mardi", "mercredi", "jeudi", "vendredi", "samedi",
];

const MONTHS_JA: [&str; 12] = [
    "1月", "2月", "3月", "4月", "5月", "6月", "7月", "8月", "9月", "10月", "11月", "12月",
];
const WEEKDAYS_JA: [&str; 7] = [
    "日曜日",
    "月曜日",
    "火曜日",
    "水曜日",
    "木曜日",
    "金曜日",
    "土曜日",
];

const EN_US: Locale = Locale {
    tag: "en-US",
    date_order: DateOrder::Mdy,
    clock: ClockStyle::H12,
    date_sep: '/',
    decimal_sep: '.',
    group_sep: ',',
    first_day: Weekday::Sunday,
    months: &MONTHS_EN,
    weekdays: &WEEKDAYS_EN,
    am_pm: ("AM", "PM"),
};

const EN_GB: Locale = Locale {
    tag: "en-GB",
    date_order: DateOrder::Dmy,
    clock: ClockStyle::H24,
    date_sep: '/',
    decimal_sep: '.',
    group_sep: ',',
    first_day: Weekday::Monday,
    months: &MONTHS_EN,
    weekdays: &WEEKDAYS_EN,
    am_pm: ("AM", "PM"),
};

const DE_DE: Locale = Locale {
    tag: "de-DE",
    date_order: DateOrder::Dmy,
    clock: ClockStyle::H24,
    date_sep: '.',
    decimal_sep: ',',
    group_sep: '.',
    first_day: Weekday::Monday,
    months: &MONTHS_DE,
    weekdays: &WEEKDAYS_DE,
    am_pm: ("AM", "PM"),
};

const FR_FR: Locale = Locale {
    tag: "fr-FR",
    date_order: DateOrder::Dmy,
    clock: ClockStyle::H24,
    date_sep: '/',
    decimal_sep: ',',
    // French groups with a (narrow) space; we use a regular space here.
    group_sep: ' ',
    first_day: Weekday::Monday,
    months: &MONTHS_FR,
    weekdays: &WEEKDAYS_FR,
    am_pm: ("AM", "PM"),
};

const JA_JP: Locale = Locale {
    tag: "ja-JP",
    date_order: DateOrder::Ymd,
    clock: ClockStyle::H24,
    date_sep: '/',
    decimal_sep: '.',
    group_sep: ',',
    first_day: Weekday::Sunday,
    months: &MONTHS_JA,
    weekdays: &WEEKDAYS_JA,
    am_pm: ("午前", "午後"),
};

impl Locale {
    /// The default fallback locale (en-US).
    pub const fn en_us() -> Locale {
        EN_US
    }
    pub const fn en_gb() -> Locale {
        EN_GB
    }
    pub const fn de_de() -> Locale {
        DE_DE
    }
    pub const fn fr_fr() -> Locale {
        FR_FR
    }
    pub const fn ja_jp() -> Locale {
        JA_JP
    }

    /// All built-in locales (handy for a settings picker / test sweeps).
    pub const ALL: [Locale; 5] = [EN_US, EN_GB, DE_DE, FR_FR, JA_JP];

    /// Look up a built-in locale by BCP-47 tag (case-insensitive on the region).
    /// Unknown tags fall back to en-US so a caller always gets *something*.
    pub fn lookup(tag: &str) -> Locale {
        for loc in Locale::ALL.iter() {
            if eq_ignore_ascii_case(loc.tag, tag) {
                return *loc;
            }
        }
        EN_US
    }
}

fn eq_ignore_ascii_case(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    for i in 0..a.len() {
        if !a[i].eq_ignore_ascii_case(&b[i]) {
            return false;
        }
    }
    true
}

/// Format the *numeric* date for a locale, e.g. `3/14/2024` (en-US),
/// `14.03.2024` (de-DE), `2024/03/14` (ja-JP). Month/day are zero-padded for
/// DMY/YMD orders (the European/ISO convention) and unpadded for en-US MDY.
pub fn format_date(c: &CivilDateTime, loc: &Locale) -> String {
    let mut out = String::new();
    let sep = loc.date_sep;
    match loc.date_order {
        DateOrder::Mdy => {
            push_uint(&mut out, c.month as u64);
            out.push(sep);
            push_uint(&mut out, c.day as u64);
            out.push(sep);
            push_year(&mut out, c.year);
        }
        DateOrder::Dmy => {
            push_2(&mut out, c.day);
            out.push(sep);
            push_2(&mut out, c.month);
            out.push(sep);
            push_year(&mut out, c.year);
        }
        DateOrder::Ymd => {
            push_year(&mut out, c.year);
            out.push(sep);
            push_2(&mut out, c.month);
            out.push(sep);
            push_2(&mut out, c.day);
        }
    }
    out
}

/// Format a long, human date with the locale's weekday + month names, e.g.
/// `Thursday, March 14, 2024` (en-US), `Donnerstag, 14. März 2024` (de-DE),
/// `2024年3月14日 木曜日` (ja-JP).
pub fn format_date_long(c: &CivilDateTime, loc: &Locale) -> String {
    let mut out = String::new();
    let wd = loc.weekdays[(c.weekday().sunday_index() % 7) as usize];
    let mo_idx = (c.month.clamp(1, 12) - 1) as usize;
    let mo = loc.months[mo_idx];

    match loc.date_order {
        DateOrder::Ymd => {
            // East-Asian long form: 2024年3月14日 木曜日
            push_year(&mut out, c.year);
            out.push('年');
            push_uint(&mut out, c.month as u64);
            out.push('月');
            push_uint(&mut out, c.day as u64);
            out.push('日');
            out.push(' ');
            out.push_str(wd);
        }
        DateOrder::Dmy => {
            // "Donnerstag, 14. März 2024"
            out.push_str(wd);
            out.push_str(", ");
            push_uint(&mut out, c.day as u64);
            out.push_str(". ");
            out.push_str(mo);
            out.push(' ');
            push_year(&mut out, c.year);
        }
        DateOrder::Mdy => {
            // "Thursday, March 14, 2024"
            out.push_str(wd);
            out.push_str(", ");
            out.push_str(mo);
            out.push(' ');
            push_uint(&mut out, c.day as u64);
            out.push_str(", ");
            push_year(&mut out, c.year);
        }
    }
    out
}

/// Format the time-of-day for a locale: `2:30 PM` (12h) vs `14:30` (24h).
/// Seconds are included only if non-zero, matching desktop clock conventions.
pub fn format_time(c: &CivilDateTime, loc: &Locale) -> String {
    let mut out = String::new();
    match loc.clock {
        ClockStyle::H24 => {
            push_2(&mut out, c.hour);
            out.push(':');
            push_2(&mut out, c.min);
            if c.sec != 0 {
                out.push(':');
                push_2(&mut out, c.sec);
            }
        }
        ClockStyle::H12 => {
            let (h12, pm) = to_12h(c.hour);
            push_uint(&mut out, h12 as u64);
            out.push(':');
            push_2(&mut out, c.min);
            if c.sec != 0 {
                out.push(':');
                push_2(&mut out, c.sec);
            }
            out.push(' ');
            out.push_str(if pm { loc.am_pm.1 } else { loc.am_pm.0 });
        }
    }
    out
}

/// Convenience: `format_date` + the locale's date/time joiner + `format_time`,
/// e.g. `3/14/2024, 2:30 PM` (en-US) vs `14.03.2024, 14:30` (de-DE) vs
/// `2024/03/14 14:30` (ja-JP).
pub fn format_datetime(c: &CivilDateTime, loc: &Locale) -> String {
    let mut out = format_date(c, loc);
    // East-Asian locales conventionally join date and time with a space; the
    // Western locales use ", ".
    if matches!(loc.date_order, DateOrder::Ymd) {
        out.push(' ');
    } else {
        out.push_str(", ");
    }
    out.push_str(&format_time(c, loc));
    out
}

/// Map a 0..=23 hour to a 12-hour display hour + an `is_pm` flag.
fn to_12h(hour: u32) -> (u32, bool) {
    let h = hour % 24;
    let pm = h >= 12;
    let h12 = match h % 12 {
        0 => 12,
        n => n,
    };
    (h12, pm)
}

// ── Number formatting ─────────────────────────────────────────────────────

/// Format an integer with the locale's thousands grouping, e.g.
/// `1_234_567` → `"1,234,567"` (en-US) / `"1.234.567"` (de-DE). Negative values
/// keep a leading `-`.
pub fn format_integer_grouped(value: i64, loc: &Locale) -> String {
    let mut out = String::new();
    let neg = value < 0;
    // Use i128 to avoid overflow negating i64::MIN.
    let mag = (value as i128).unsigned_abs();
    if neg {
        out.push('-');
    }
    push_grouped_u128(&mut out, mag, loc.group_sep);
    out
}

/// Format a real value with the locale's grouping + decimal separator, e.g.
/// `1234567.89` → `"1,234,567.89"` (en-US) / `"1.234.567,89"` (de-DE). Rounds to
/// at most `frac_digits` decimals (trailing zeros within that width are kept so
/// currency-style output is stable). Non-finite input renders as `"NaN"`.
pub fn format_number_prec(value: f64, loc: &Locale, frac_digits: u32) -> String {
    if !value.is_finite() {
        let mut s = String::new();
        s.push_str("NaN");
        return s;
    }
    let mut out = String::new();
    let neg = value.is_sign_negative() && value != 0.0;
    let v = if neg { -value } else { value };

    // Round half-up at the requested precision.
    let scale = pow10(frac_digits);
    // (v * scale) rounded to nearest integer, in u128 to stay exact for our range.
    let scaled = (v * scale + 0.5) as u128;

    let int_part = scaled / (scale as u128);
    let frac_part = scaled % (scale as u128);

    if neg && scaled != 0 {
        out.push('-');
    }
    push_grouped_u128(&mut out, int_part, loc.group_sep);

    if frac_digits > 0 {
        out.push(loc.decimal_sep);
        // Zero-pad the fractional part to exactly `frac_digits`.
        let mut tmp = String::new();
        push_uint(&mut tmp, frac_part as u64);
        for _ in (tmp.len() as i64)..(frac_digits as i64) {
            out.push('0');
        }
        out.push_str(&tmp);
    }
    out
}

/// Format a real value with the locale's separators at the default precision
/// (2 fractional digits), e.g. `1234567.89` → `"1,234,567.89"` (en-US) /
/// `"1.234.567,89"` (de-DE).
pub fn format_number(value: f64, loc: &Locale) -> String {
    format_number_prec(value, loc, 2)
}

// ── Small formatting helpers (no panics, no libm) ─────────────────────────

fn pow10(n: u32) -> f64 {
    let mut r = 1.0;
    let mut i = 0;
    while i < n {
        r *= 10.0;
        i += 1;
    }
    r
}

/// Append a base-10 unsigned integer.
fn push_uint(out: &mut String, mut v: u64) {
    if v == 0 {
        out.push('0');
        return;
    }
    let mut tmp = [0u8; 20];
    let mut n = 0;
    while v > 0 {
        tmp[n] = b'0' + (v % 10) as u8;
        v /= 10;
        n += 1;
    }
    while n > 0 {
        n -= 1;
        out.push(tmp[n] as char);
    }
}

/// Append a base-10 unsigned 128-bit integer with optional thousands grouping.
/// `group == '\0'` disables grouping.
fn push_grouped_u128(out: &mut String, mut v: u128, group: char) {
    if v == 0 {
        out.push('0');
        return;
    }
    // Collect digits least-significant first.
    let mut digits = [0u8; 40];
    let mut n = 0;
    while v > 0 {
        digits[n] = b'0' + (v % 10) as u8;
        v /= 10;
        n += 1;
    }
    let grouping = group != '\0';
    // Emit most-significant first, inserting `group` every 3 from the right.
    let mut i = n;
    while i > 0 {
        i -= 1;
        out.push(digits[i] as char);
        if grouping && i > 0 && i % 3 == 0 {
            out.push(group);
        }
    }
}

/// Append a 2-digit zero-padded value (only the low two digits).
fn push_2(out: &mut String, v: u32) {
    out.push((b'0' + ((v / 10) % 10) as u8) as char);
    out.push((b'0' + (v % 10) as u8) as char);
}

/// Append a year zero-padded to at least 4 digits, with a leading `-` for
/// negative (BCE-ish) years.
fn push_year(out: &mut String, year: i64) {
    let neg = year < 0;
    let mag = (year as i128).unsigned_abs() as u64;
    if neg {
        out.push('-');
    }
    if mag < 1000 {
        // Pad to 4 digits.
        let mut tmp = String::new();
        push_uint(&mut tmp, mag);
        for _ in (tmp.len() as i64)..4 {
            out.push('0');
        }
        out.push_str(&tmp);
    } else {
        push_uint(out, mag);
    }
}

// ───────────────────────────────────────────────────────────────────────────
// Host KATs — the FAIL-able proof: `cargo test -p rae_time`
// ───────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Civil round-trip ──────────────────────────────────────────────────

    #[test]
    fn civil_round_trip_known_instants() {
        // epoch 0, a leap day, a pre-1970 negative, a far future.
        let cases: [i64; 6] = [
            0,
            1_709_210_096,  // 2024-02-29 12:34:56 UTC (leap day)
            1_710_426_600,  // 2024-03-14 14:30:00 UTC
            -1,             // 1969-12-31 23:59:59
            -2_208_988_800, // 1900-01-01 00:00:00
            32_503_680_000, // 3000-01-01 00:00:00
        ];
        for &t in cases.iter() {
            let c = civil_from_unix(t);
            assert_eq!(unix_from_civil(&c), t, "round-trip failed for {t}");
        }
    }

    #[test]
    fn epoch_is_1970_thursday() {
        let c = civil_from_unix(0);
        assert_eq!(
            (c.year, c.month, c.day, c.hour, c.min, c.sec),
            (1970, 1, 1, 0, 0, 0)
        );
        assert_eq!(weekday(0), Weekday::Thursday);
        assert_eq!(c.weekday(), Weekday::Thursday);
    }

    #[test]
    fn leap_day_decodes() {
        let c = civil_from_unix(1_709_210_096);
        assert_eq!(
            (c.year, c.month, c.day, c.hour, c.min, c.sec),
            (2024, 2, 29, 12, 34, 56)
        );
        assert_eq!(c.weekday(), Weekday::Thursday);
    }

    #[test]
    fn pre_1970_negative_epoch() {
        // -1 second = 1969-12-31 23:59:59 (the div_euclid/rem_euclid guard).
        let c = civil_from_unix(-1);
        assert_eq!(
            (c.year, c.month, c.day, c.hour, c.min, c.sec),
            (1969, 12, 31, 23, 59, 59)
        );
    }

    #[test]
    fn leap_year_rule_is_correct() {
        // FAIL-ABILITY: break the leap rule (e.g. drop the %400 exception) and
        // (2000,2) flips from 29 to 28 / (1900,2) flips from 28 to 29 here.
        assert_eq!(days_in_month(2024, 2), 29);
        assert_eq!(days_in_month(2023, 2), 28);
        assert_eq!(days_in_month(2000, 2), 29);
        assert_eq!(days_in_month(1900, 2), 28);
        assert!(is_leap(2024) && !is_leap(2023) && is_leap(2000) && !is_leap(1900));
        // Ordinary month lengths.
        assert_eq!(days_in_month(2023, 4), 30);
        assert_eq!(days_in_month(2023, 12), 31);
        // Out-of-range month never panics → 0.
        assert_eq!(days_in_month(2023, 0), 0);
        assert_eq!(days_in_month(2023, 13), 0);
    }

    // ── ISO 8601 ──────────────────────────────────────────────────────────

    #[test]
    fn iso_format_round_trip() {
        let c = CivilDateTime {
            year: 2024,
            month: 3,
            day: 14,
            hour: 14,
            min: 30,
            sec: 5,
        };
        let s = format_iso8601(&c);
        assert_eq!(s, "2024-03-14T14:30:05Z");
        let back = parse_iso8601(&s).expect("parse");
        assert_eq!(back, c);
    }

    #[test]
    fn iso_parse_accepts_space_and_no_z() {
        let a = parse_iso8601("2024-03-14T14:30:05").unwrap();
        let b = parse_iso8601("2024-03-14 14:30:05").unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn iso_malformed_is_err_not_panic() {
        // None of these may panic; all must be Err.
        let bad = [
            "",
            "2024",
            "2024-13-01T00:00:00Z", // month 13
            "2024-02-30T00:00:00Z", // Feb 30 invalid
            "2024-00-10T00:00:00Z", // month 0
            "2024-03-14T24:00:00Z", // hour 24
            "2024-03-14T12:60:00Z", // min 60
            "2024-03-14X12:00:00Z", // bad separator
            "20XX-03-14T12:00:00Z", // non-digit
            "2024-03-14T12:00:00Q", // bad trailing
            "2024/03/14T12:00:00Z", // wrong date sep
        ];
        for s in bad.iter() {
            assert_eq!(parse_iso8601(s), Err(ParseError::Malformed), "{s}");
        }
    }

    // ── Locale formatting (the i18n value) ────────────────────────────────

    #[test]
    fn same_instant_three_locales_differ_and_are_correct() {
        // 2024-03-14 14:30:00 UTC.
        let c = civil_from_unix(1_710_426_600);
        assert_eq!(
            (c.year, c.month, c.day, c.hour, c.min),
            (2024, 3, 14, 14, 30)
        );

        let us = format_datetime(&c, &Locale::en_us());
        let de = format_datetime(&c, &Locale::de_de());
        let ja = format_datetime(&c, &Locale::ja_jp());

        assert_eq!(us, "3/14/2024, 2:30 PM");
        assert_eq!(de, "14.03.2024, 14:30");
        assert_eq!(ja, "2024/03/14 14:30");

        // FAIL-ABILITY: a broken locale table (e.g. de-DE losing its DMY order or
        // 24h clock) collapses these into each other → these assert_ne! flip.
        assert_ne!(us, de);
        assert_ne!(de, ja);
        assert_ne!(us, ja);
    }

    #[test]
    fn time_12h_vs_24h() {
        let midnight = CivilDateTime {
            year: 2024,
            month: 1,
            day: 1,
            hour: 0,
            min: 5,
            sec: 0,
        };
        let noon = CivilDateTime {
            year: 2024,
            month: 1,
            day: 1,
            hour: 12,
            min: 0,
            sec: 0,
        };
        let evening = CivilDateTime {
            year: 2024,
            month: 1,
            day: 1,
            hour: 23,
            min: 9,
            sec: 0,
        };

        assert_eq!(format_time(&midnight, &Locale::en_us()), "12:05 AM");
        assert_eq!(format_time(&noon, &Locale::en_us()), "12:00 PM");
        assert_eq!(format_time(&evening, &Locale::en_us()), "11:09 PM");

        assert_eq!(format_time(&midnight, &Locale::en_gb()), "00:05");
        assert_eq!(format_time(&evening, &Locale::en_gb()), "23:09");
    }

    #[test]
    fn number_grouping_and_decimal_per_locale() {
        // The headline i18n contract.
        assert_eq!(
            format_number(1_234_567.89, &Locale::en_us()),
            "1,234,567.89"
        );
        assert_eq!(
            format_number(1_234_567.89, &Locale::de_de()),
            "1.234.567,89"
        );
        // FAIL-ABILITY: swap the de-DE group_sep/decimal_sep table entries and
        // this assertion flips to the en-US string.
        assert_ne!(
            format_number(1_234_567.89, &Locale::de_de()),
            "1,234,567.89"
        );

        // Integer grouping.
        assert_eq!(
            format_integer_grouped(1_234_567, &Locale::en_us()),
            "1,234,567"
        );
        assert_eq!(
            format_integer_grouped(1_234_567, &Locale::de_de()),
            "1.234.567"
        );
        assert_eq!(format_integer_grouped(-42, &Locale::en_us()), "-42");
        assert_eq!(format_integer_grouped(0, &Locale::en_us()), "0");
        assert_eq!(format_integer_grouped(999, &Locale::en_us()), "999");
    }

    #[test]
    fn date_orders_render_correctly() {
        let c = CivilDateTime {
            year: 2024,
            month: 3,
            day: 14,
            hour: 0,
            min: 0,
            sec: 0,
        };
        assert_eq!(format_date(&c, &Locale::en_us()), "3/14/2024");
        assert_eq!(format_date(&c, &Locale::en_gb()), "14/03/2024");
        assert_eq!(format_date(&c, &Locale::de_de()), "14.03.2024");
        assert_eq!(format_date(&c, &Locale::ja_jp()), "2024/03/14");
    }

    #[test]
    fn long_dates_use_locale_names() {
        let c = civil_from_unix(1_710_426_600); // 2024-03-14, a Thursday
        assert_eq!(
            format_date_long(&c, &Locale::en_us()),
            "Thursday, March 14, 2024"
        );
        assert_eq!(
            format_date_long(&c, &Locale::de_de()),
            "Donnerstag, 14. März 2024"
        );
        assert_eq!(
            format_date_long(&c, &Locale::ja_jp()),
            "2024年3月14日 木曜日"
        );
    }

    #[test]
    fn locale_lookup_falls_back() {
        assert_eq!(Locale::lookup("de-DE").tag, "de-DE");
        assert_eq!(Locale::lookup("DE-de").tag, "de-DE"); // case-insensitive
        assert_eq!(Locale::lookup("xx-YY").tag, "en-US"); // unknown → fallback
    }

    // ── Duration ──────────────────────────────────────────────────────────

    #[test]
    fn duration_human_and_arithmetic() {
        assert_eq!(Duration::from_secs(0).human(), "0s");
        assert_eq!(Duration::from_secs(45).human(), "45s");
        assert_eq!(Duration::from_secs(2 * 3600 + 5 * 60).human(), "2h 5m");
        assert_eq!(Duration::from_secs(3 * 86_400 + 4 * 3600).human(), "3d 4h");
        assert_eq!(Duration::from_secs(-90).human(), "-1m 30s");

        // Calendar-correct add across a month boundary + leap day.
        let feb28 = CivilDateTime {
            year: 2024,
            month: 2,
            day: 28,
            hour: 12,
            min: 0,
            sec: 0,
        };
        let plus2d = Duration::from_days(2).add_to(&feb28);
        // 2024 is a leap year → Feb 28 + 2 days = Mar 1 (Feb 29 exists).
        assert_eq!((plus2d.year, plus2d.month, plus2d.day), (2024, 3, 1));

        let back = Duration::from_days(2).sub_from(&plus2d);
        assert_eq!(back, feb28);
    }

    // ── Never-panic edge battery ──────────────────────────────────────────

    #[test]
    fn edge_inputs_never_panic() {
        // Extreme epochs.
        let _ = civil_from_unix(i64::MAX / 2);
        let _ = civil_from_unix(i64::MIN / 2);
        let _ = civil_from_unix(i64::MIN); // exercise div_euclid on the floor
                                           // Out-of-range civil fields fold arithmetically, no panic.
        let weird = CivilDateTime {
            year: 2024,
            month: 13,
            day: 0,
            hour: 99,
            min: 99,
            sec: 99,
        };
        let _ = unix_from_civil(&weird);
        let _ = format_iso8601(&weird);
        let _ = format_date(&weird, &Locale::en_us());
        // Huge numbers + non-finite formatting.
        let _ = format_integer_grouped(i64::MIN, &Locale::de_de());
        let _ = format_number(1e308, &Locale::en_us());
        assert_eq!(format_number(f64::NAN, &Locale::en_us()), "NaN");
        assert_eq!(format_number(f64::INFINITY, &Locale::en_us()), "NaN");
        // The whole battery returning means nothing panicked.
    }
}
