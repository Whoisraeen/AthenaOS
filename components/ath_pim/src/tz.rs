//! # POSIX-TZ timezone engine for [`crate`] — UTC-offset + DST resolution.
//!
//! LEGACY_GAMING_CONCEPT.md §Compatibility Strategy ("let people switch without
//! conscious effort") — switcher criterion #5 is "import my calendar & contacts."
//! Importing the *data* ([`crate::parse_ics`]) and expanding recurrence
//! ([`crate::recur`]) still leaves one question a calendar UI must answer to be
//! trustworthy: **"my 3pm meeting — in MY timezone, across a DST change — is when,
//! exactly?"** A `.ics` DTSTART carries a `TZID` (e.g. `America/New_York`) or a
//! trailing `Z` (UTC); to render it in the user's local wall-clock time, or to
//! normalise it to UTC for storage/sorting, the calendar must resolve that zone's
//! UTC offset *at that instant*, which changes twice a year under DST. This module
//! is that resolver.
//!
//! ## What it models: the POSIX TZ string (IEEE 1003.1)
//! The portable, tzdata-free subset that covers the vast majority of real zones:
//!
//! ```text
//! std offset [dst [offset] [,start[/time],end[/time]]]
//! ```
//!
//! e.g. `EST5EDT,M3.2.0,M11.1.0` (US Eastern), `CET-1CEST,M3.5.0,M10.5.0/3`
//! (Central Europe), `AEST-10AEDT,M10.1.0,M4.1.0/3` (Sydney — DST wraps the
//! southern-hemisphere new year), `<+0530>-5:30` (India, bracketed numeric name),
//! `UTC0`.
//!
//! - **Zone abbreviations** — alphabetic (`EST`) or bracketed numeric (`<+05>`).
//! - **UTC offsets** — POSIX sign is **inverted from common usage**: `EST5` means
//!   UTC**−**5; `CET-1` means UTC**+**1. (This is the classic bug; the host KAT
//!   asserts it explicitly — see [`tests`].)
//! - **DST transition rules** — the `Mm.w.d` form (month, week `1..5` where `5` =
//!   last, weekday `0`=Sun) with an optional `/time` (default `02:00:00`). The
//!   `Jn` (Julian, no Feb 29) and `n` (zero-based, Feb 29 counted) day forms are
//!   also parsed.
//!
//! The exact transition instant for a given year is computed with the crate's own
//! civil-date algorithms ([`crate::recur::days_from_civil`] etc.) — the same code
//! the recurrence expander uses; this module reimplements no calendar math.
//!
//! ## The API
//! - [`parse_tz`] — POSIX string → [`TzInfo`] (`Err` on garbage, never panics).
//! - [`TzInfo::offset_seconds_at`] — the UTC offset (seconds, east-positive) in
//!   effect at a given **UTC** instant, accounting for DST in either hemisphere.
//! - [`TzInfo::is_dst_at`] — whether DST is active at a UTC instant.
//! - [`TzInfo::to_local`] — apply the offset → local civil [`DateTime`] with the
//!   right abbreviation.
//! - [`TzInfo::local_to_utc`] — best-effort inverse (see the fold/gap note below).
//! - [`tz_for_iana`] — a *curated* IANA-name → POSIX-string map (~10 common zones),
//!   so a TZID from a real `.ics` resolves without a full tzdata.
//!
//! ## Honest scope (documented-deferred)
//! - **Full Olson / tzdata binary** and **historical rule changes** (a zone's rules
//!   in 1985 ≠ today) are a later layer — the POSIX-TZ subset assumes the *current*
//!   rule applies for all years, which is correct for any near-future calendar use.
//! - The **curated IANA map** is ~10 hand-picked common zones, not the full ~600.
//! - **`local_to_utc` fold/gap ambiguity**: at a spring-forward gap a wall-clock
//!   time may not exist; at a fall-back fold it occurs twice. The convention here
//!   is: resolve the offset using the offset that *would* be in effect at the given
//!   local time interpreted as if it were UTC, then correct once — a standard,
//!   documented best-effort that is exact away from the ~1h transition windows.
//!
//! ## Hostile-input posture
//! A TZ string can arrive from an untrusted `.ics` `VTIMEZONE`/`X-` extension. There
//! is no `unwrap`/`expect`/`panic`/unbounded-loop path from [`parse_tz`]: parsing is
//! a single bounded left-to-right scan, malformed input yields [`TzError`], and a
//! seeded fuzz loop over arbitrary strings is part of the proof.

use alloc::string::{String, ToString};

use crate::recur::{days_from_civil, weekday_from_days};
use crate::DateTime;

/// Why a POSIX TZ string failed to parse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TzError {
    /// The string was empty.
    Empty,
    /// The standard-time portion (abbreviation + offset) was missing or malformed —
    /// e.g. `"EST"` with no offset, or a bracketed name that never closed.
    BadStd,
    /// The DST portion was present but its transition rules were malformed.
    BadDst,
}

/// A DST transition day-rule (the comma part of a POSIX TZ string).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransRule {
    /// `Mm.w.d` — month `m` (1..=12), week `w` (1..=5, 5 = last), weekday `d`
    /// (0 = Sunday .. 6 = Saturday).
    Month { month: u8, week: u8, weekday: u8 },
    /// `Jn` — Julian day 1..=365, Feb 29 is **never** counted (so day 60 is always
    /// March 1).
    JulianNoLeap(u16),
    /// `n` — zero-based day 0..=365, Feb 29 **is** counted in a leap year.
    ZeroBased(u16),
}

/// A fully parsed POSIX TZ string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TzInfo {
    /// Standard-time abbreviation (e.g. `EST`, `+05`).
    pub std_abbr: String,
    /// Standard-time UTC offset in **seconds, east-positive** (so US Eastern std is
    /// `-18000` = −5h — already sign-corrected from the inverted POSIX form).
    pub std_offset: i32,
    /// DST abbreviation (e.g. `EDT`), if the zone observes DST.
    pub dst_abbr: Option<String>,
    /// DST UTC offset in seconds, east-positive. Defaults to `std_offset + 3600`
    /// when the DST offset is omitted in the string (POSIX rule).
    pub dst_offset: Option<i32>,
    /// The DST start rule and its local-time-of-day (seconds since local midnight,
    /// default 7200 = 02:00). `None` when the zone has no DST.
    pub dst_start: Option<(TransRule, i32)>,
    /// The DST end rule and its local-time-of-day. `None` when no DST.
    pub dst_end: Option<(TransRule, i32)>,
}

impl TzInfo {
    /// `true` if this zone observes DST (has both a DST offset and transition rules).
    pub fn has_dst(&self) -> bool {
        self.dst_abbr.is_some() && self.dst_start.is_some() && self.dst_end.is_some()
    }

    /// The UTC offset (seconds, east-positive) in effect at the given **UTC**
    /// instant. Handles both northern (DST mid-year) and southern (DST wraps the
    /// new year) hemispheres. With no DST, always returns `std_offset`. Never panics.
    pub fn offset_seconds_at(&self, utc: &DateTime) -> i32 {
        if self.is_dst_at(utc) {
            self.dst_offset.unwrap_or(self.std_offset + 3600)
        } else {
            self.std_offset
        }
    }

    /// `true` if DST is in effect at the given **UTC** instant. Never panics.
    ///
    /// A transition rule is specified in *local* time, so the exact UTC instant of a
    /// transition depends on the offset just before it (standard offset for the
    /// spring transition, DST offset for the autumn transition). We compute both
    /// boundary instants in UTC seconds and test the interval — order-independent so
    /// the southern hemisphere (start month > end month) works automatically.
    pub fn is_dst_at(&self, utc: &DateTime) -> bool {
        if !self.has_dst() {
            return false;
        }
        let (start_rule, start_tod) = match &self.dst_start {
            Some(s) => s,
            None => return false,
        };
        let (end_rule, end_tod) = match &self.dst_end {
            Some(e) => e,
            None => return false,
        };
        let year = utc.year as i64;
        let now = utc_seconds(utc);

        // Spring transition (std -> dst): the local time is interpreted using the
        // standard offset (DST not yet in effect just before it).
        let start_utc = transition_utc_seconds(start_rule, *start_tod, year, self.std_offset);
        // Autumn transition (dst -> std): local time uses the DST offset.
        let dst_off = self.dst_offset.unwrap_or(self.std_offset + 3600);
        let end_utc = transition_utc_seconds(end_rule, *end_tod, year, dst_off);

        if start_utc < end_utc {
            // Northern hemisphere: DST is the interval [start, end).
            now >= start_utc && now < end_utc
        } else {
            // Southern hemisphere: DST wraps the new year — active OUTSIDE [end,start).
            now >= start_utc || now < end_utc
        }
    }

    /// Convert a **UTC** [`DateTime`] to local civil time in this zone, tagging the
    /// result with the active abbreviation (`std_abbr` / `dst_abbr`) in
    /// [`DateTime::tz`]. The result's `utc` flag is cleared (it is local now). Never
    /// panics.
    pub fn to_local(&self, utc: &DateTime) -> DateTime {
        let off = self.offset_seconds_at(utc);
        let dst = self.is_dst_at(utc);
        let abbr = if dst {
            self.dst_abbr
                .clone()
                .unwrap_or_else(|| self.std_abbr.clone())
        } else {
            self.std_abbr.clone()
        };
        let mut out = shift_datetime(utc, off as i64);
        out.utc = false;
        out.tz = Some(abbr);
        out
    }

    /// Best-effort inverse of [`to_local`]: a **local** [`DateTime`] in this zone →
    /// the equivalent **UTC** [`DateTime`].
    ///
    /// Fold/gap convention: we pick the offset that applies when the given local time
    /// is *first* interpreted as UTC, determine whether DST is in effect at that
    /// approximate instant, then subtract that offset. This is exact except inside the
    /// ~1h spring-forward gap (a non-existent wall time) and fall-back fold (an
    /// ambiguous wall time), where it resolves deterministically to the standard-time
    /// reading — documented, not an error. Never panics.
    pub fn local_to_utc(&self, local: &DateTime) -> DateTime {
        // First guess: treat the local fields as if UTC to probe which side of the
        // transition we are on, using the std offset as the approximate shift.
        let probe_utc = shift_datetime(local, -(self.std_offset as i64));
        let off = self.offset_seconds_at(&probe_utc);
        let mut out = shift_datetime(local, -(off as i64));
        out.utc = true;
        out.tz = None;
        out
    }
}

/// Total seconds since the Unix epoch for a [`DateTime`]'s civil fields (date +
/// time-of-day). Ignores the `tz`/`utc` flags — the caller asserts the meaning.
fn utc_seconds(dt: &DateTime) -> i64 {
    let days = days_from_civil(dt.year as i64, dt.month as i64, dt.day as i64);
    days * 86_400 + dt.hour as i64 * 3600 + dt.minute as i64 * 60 + dt.second as i64
}

/// Build a [`DateTime`] from total seconds since the Unix epoch (the inverse of
/// [`utc_seconds`]), preserving `is_date` from a template. Never panics.
fn datetime_from_seconds(secs: i64, template: &DateTime) -> DateTime {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (y, m, d) = crate::recur::civil_from_days(days);
    let hour = (rem / 3600) as u8;
    let minute = ((rem % 3600) / 60) as u8;
    let second = (rem % 60) as u8;
    let year = if (0..=9999).contains(&y) { y as u16 } else { 0 };
    DateTime {
        year,
        month: m,
        day: d,
        hour,
        minute,
        second,
        is_date: template.is_date,
        utc: false,
        tz: None,
        raw: String::new(),
    }
}

/// Shift a [`DateTime`] by `delta` seconds (the offset applied to convert between
/// UTC and local), returning a new civil [`DateTime`]. Never panics.
fn shift_datetime(dt: &DateTime, delta: i64) -> DateTime {
    datetime_from_seconds(utc_seconds(dt).saturating_add(delta), dt)
}

/// The civil day-of-month that a [`TransRule`] resolves to in `year`. Never panics;
/// returns a clamped, in-range day.
fn rule_day_of_month(rule: &TransRule, year: i64) -> (u8, u8) {
    match rule {
        TransRule::Month {
            month,
            week,
            weekday,
        } => {
            let m = (*month).clamp(1, 12);
            let dom = nth_weekday(year, m, *weekday, *week);
            (m, dom)
        }
        TransRule::JulianNoLeap(n) => {
            // 1..=365, Feb 29 never counted. Day 1 = Jan 1.
            let n = (*n).clamp(1, 365) as i64;
            // Walk the months, skipping Feb 29.
            let mut remaining = n;
            let mut month = 1u8;
            loop {
                let dim = std_days_in_month(month); // Feb always 28 here
                if remaining <= dim as i64 {
                    return (month, remaining as u8);
                }
                remaining -= dim as i64;
                month += 1;
                if month > 12 {
                    return (12, 31);
                }
            }
        }
        TransRule::ZeroBased(n) => {
            // 0..=365, Feb 29 counted in a leap year. Day 0 = Jan 1.
            let n = (*n).min(365) as i64;
            let base = days_from_civil(year, 1, 1) + n;
            let (_, m, d) = crate::recur::civil_from_days(base);
            (m, d)
        }
    }
}

/// Days in a month with February pinned to 28 (used only by the `Jn` Julian form,
/// which never counts Feb 29).
fn std_days_in_month(month: u8) -> u8 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => 28,
        _ => 0,
    }
}

/// Day-of-month (1..=31) of the `week`-th `weekday` of `(year, month)`. `week`
/// 1..=4 counts from the start; `week == 5` means the **last** such weekday in the
/// month (POSIX semantics). `weekday` is `0=Sun..6=Sat`. Never panics.
fn nth_weekday(year: i64, month: u8, weekday: u8, week: u8) -> u8 {
    let weekday = weekday % 7;
    if week >= 5 {
        // Last `weekday` of the month: start from the last day, walk back.
        let dim = real_days_in_month(year, month);
        let last_days = days_from_civil(year, month as i64, dim as i64);
        let last_wd = weekday_from_days(last_days);
        let back = (7 + last_wd - weekday) % 7;
        (dim as i64 - back as i64) as u8
    } else {
        let first_days = days_from_civil(year, month as i64, 1);
        let first_wd = weekday_from_days(first_days);
        let delta = (7 + weekday - first_wd) % 7; // 0..6 to first match
        let w = week.max(1) as i64;
        let day = 1 + delta as i64 + 7 * (w - 1);
        let dim = real_days_in_month(year, month) as i64;
        if day > dim {
            (day - 7) as u8 // clamp a non-existent 5th into the last (defensive)
        } else {
            day as u8
        }
    }
}

/// Days in `month` of `year`, honouring leap February (the real calendar).
fn real_days_in_month(year: i64, month: u8) -> u8 {
    crate::recur::days_in_month(year, month)
}

/// The exact UTC instant (seconds since epoch) of a transition specified by `rule`
/// at local-time `tod_secs` in `year`, given the UTC offset (`offset_before`,
/// east-positive seconds) in effect immediately before the transition. Never panics.
fn transition_utc_seconds(rule: &TransRule, tod_secs: i32, year: i64, offset_before: i32) -> i64 {
    let (month, dom) = rule_day_of_month(rule, year);
    let days = days_from_civil(year, month as i64, dom as i64);
    let local_secs = days * 86_400 + tod_secs as i64;
    // local = utc + offset  =>  utc = local - offset.
    local_secs - offset_before as i64
}

// ===========================================================================
// Parsing
// ===========================================================================

/// Parse a POSIX TZ string into a [`TzInfo`]. Returns an [`TzError`] on empty or
/// malformed input — never panics, never loops unbounded.
///
/// Accepts: `std offset [dst [offset] [,start[/time],end[/time]]]` with alphabetic
/// (`EST`) or bracketed (`<+0530>`) zone names, the inverted POSIX offset sign, and
/// `Mm.w.d` / `Jn` / `n` transition rules. A leading `:` (POSIX "implementation
/// defined" prefix) is rejected as not-a-POSIX-rule.
pub fn parse_tz(s: &str) -> Result<TzInfo, TzError> {
    let s = s.trim();
    if s.is_empty() {
        return Err(TzError::Empty);
    }
    if s.starts_with(':') {
        // POSIX reserves a leading ':' for implementation-defined (file) lookups —
        // not a parseable rule string.
        return Err(TzError::BadStd);
    }

    let bytes = s.as_bytes();
    let mut i = 0usize;

    // ---- std abbreviation ----
    let std_abbr = parse_abbr(bytes, &mut i).ok_or(TzError::BadStd)?;

    // ---- std offset (mandatory) ----
    let std_off = parse_offset(bytes, &mut i).ok_or(TzError::BadStd)?;
    // POSIX sign is inverted: the string's "+5" means 5 hours WEST of UTC.
    let std_offset = -std_off;

    // No DST portion → done.
    if i >= bytes.len() {
        return Ok(TzInfo {
            std_abbr,
            std_offset,
            dst_abbr: None,
            dst_offset: None,
            dst_start: None,
            dst_end: None,
        });
    }

    // ---- dst abbreviation ----
    let dst_abbr = parse_abbr(bytes, &mut i).ok_or(TzError::BadDst)?;

    // ---- optional dst offset ----
    let dst_offset = if i < bytes.len() && bytes[i] != b',' {
        let off = parse_offset(bytes, &mut i).ok_or(TzError::BadDst)?;
        Some(-off)
    } else {
        // Omitted → std + 1h (in east-positive seconds).
        Some(std_offset + 3600)
    };

    // ---- transition rules ",start[/time],end[/time]" ----
    if i >= bytes.len() {
        // DST name+offset but no rules: POSIX leaves this implementation-defined.
        // Treat as malformed rather than silently fabricate transitions.
        return Err(TzError::BadDst);
    }
    if bytes[i] != b',' {
        return Err(TzError::BadDst);
    }
    i += 1; // consume ','
    let (start_rule, start_tod) = parse_rule(bytes, &mut i).ok_or(TzError::BadDst)?;
    if i >= bytes.len() || bytes[i] != b',' {
        return Err(TzError::BadDst);
    }
    i += 1; // consume ','
    let (end_rule, end_tod) = parse_rule(bytes, &mut i).ok_or(TzError::BadDst)?;
    // Anything trailing after the second rule → malformed.
    if i != bytes.len() {
        return Err(TzError::BadDst);
    }

    Ok(TzInfo {
        std_abbr,
        std_offset,
        dst_abbr: Some(dst_abbr),
        dst_offset,
        dst_start: Some((start_rule, start_tod)),
        dst_end: Some((end_rule, end_tod)),
    })
}

/// Parse a zone abbreviation at `*i`: either `<...>` (bracketed, any chars except
/// `>`) or a run of ASCII letters (3+ per POSIX, but we accept any non-empty alpha
/// run). Advances `*i`. Returns `None` if no valid abbreviation is present.
fn parse_abbr(bytes: &[u8], i: &mut usize) -> Option<String> {
    if *i >= bytes.len() {
        return None;
    }
    if bytes[*i] == b'<' {
        // Bracketed: copy until '>'.
        let mut j = *i + 1;
        let start = j;
        let mut closed = false;
        while j < bytes.len() {
            if bytes[j] == b'>' {
                closed = true;
                break;
            }
            j += 1;
        }
        if !closed || j == start {
            return None;
        }
        let name = core::str::from_utf8(&bytes[start..j]).ok()?.to_string();
        *i = j + 1; // skip '>'
        Some(name)
    } else {
        let start = *i;
        let mut j = *i;
        while j < bytes.len() && bytes[j].is_ascii_alphabetic() {
            j += 1;
        }
        if j == start {
            return None;
        }
        let name = core::str::from_utf8(&bytes[start..j]).ok()?.to_string();
        *i = j;
        Some(name)
    }
}

/// Parse a signed `[+|-]hh[:mm[:ss]]` offset at `*i` into total **seconds** in the
/// raw POSIX sign convention (positive = west of UTC). Advances `*i`. Returns `None`
/// if no digits are present. Never panics; values are bounded.
fn parse_offset(bytes: &[u8], i: &mut usize) -> Option<i32> {
    if *i >= bytes.len() {
        return None;
    }
    let mut neg = false;
    if bytes[*i] == b'+' {
        *i += 1;
    } else if bytes[*i] == b'-' {
        neg = true;
        *i += 1;
    }
    let hh = parse_uint(bytes, i, 3)?; // hours can be up to 167 in POSIX; cap width 3
    let mut total = hh as i32 * 3600;
    if *i < bytes.len() && bytes[*i] == b':' {
        *i += 1;
        let mm = parse_uint(bytes, i, 2)?;
        total += (mm as i32).min(59) * 60;
        if *i < bytes.len() && bytes[*i] == b':' {
            *i += 1;
            let ss = parse_uint(bytes, i, 2)?;
            total += (ss as i32).min(59);
        }
    }
    // Bound to a sane offset range (|offset| <= 24h59m59s).
    total = total.min(24 * 3600 + 59 * 60 + 59);
    Some(if neg { -total } else { total })
}

/// Parse a `/time` local-time-of-day suffix (the `[/time]` after a transition rule),
/// in **seconds since local midnight**, defaulting to 7200 (02:00:00) when absent.
/// POSIX allows hours 0..=167 and a leading sign; we accept `[+|-]h[:mm[:ss]]`.
fn parse_tod(bytes: &[u8], i: &mut usize) -> i32 {
    if *i < bytes.len() && bytes[*i] == b'/' {
        *i += 1;
        // Reuse the offset parser's grammar (sign + hh:mm:ss) but it's a clock time.
        let mut neg = false;
        if *i < bytes.len() && (bytes[*i] == b'+' || bytes[*i] == b'-') {
            neg = bytes[*i] == b'-';
            *i += 1;
        }
        let hh = parse_uint(bytes, i, 3).unwrap_or(2);
        let mut total = hh as i32 * 3600;
        if *i < bytes.len() && bytes[*i] == b':' {
            *i += 1;
            let mm = parse_uint(bytes, i, 2).unwrap_or(0);
            total += (mm as i32).min(59) * 60;
            if *i < bytes.len() && bytes[*i] == b':' {
                *i += 1;
                let ss = parse_uint(bytes, i, 2).unwrap_or(0);
                total += (ss as i32).min(59);
            }
        }
        if neg {
            -total
        } else {
            total
        }
    } else {
        7200 // default 02:00:00
    }
}

/// Parse a transition rule at `*i`: `Mm.w.d` | `Jn` | `n`, followed by an optional
/// `/time`. Returns `(rule, tod_secs)`. Advances `*i`. Returns `None` if malformed.
fn parse_rule(bytes: &[u8], i: &mut usize) -> Option<(TransRule, i32)> {
    if *i >= bytes.len() {
        return None;
    }
    let rule = match bytes[*i] {
        b'M' => {
            *i += 1;
            let month = parse_uint(bytes, i, 2)? as u8;
            if *i >= bytes.len() || bytes[*i] != b'.' {
                return None;
            }
            *i += 1;
            let week = parse_uint(bytes, i, 1)? as u8;
            if *i >= bytes.len() || bytes[*i] != b'.' {
                return None;
            }
            *i += 1;
            let weekday = parse_uint(bytes, i, 1)? as u8;
            if !(1..=12).contains(&month) || !(1..=5).contains(&week) || weekday > 6 {
                return None;
            }
            TransRule::Month {
                month,
                week,
                weekday,
            }
        }
        b'J' => {
            *i += 1;
            let n = parse_uint(bytes, i, 3)? as u16;
            if !(1..=365).contains(&n) {
                return None;
            }
            TransRule::JulianNoLeap(n)
        }
        b'0'..=b'9' => {
            let n = parse_uint(bytes, i, 3)? as u16;
            if n > 365 {
                return None;
            }
            TransRule::ZeroBased(n)
        }
        _ => return None,
    };
    let tod = parse_tod(bytes, i);
    Some((rule, tod))
}

/// Parse up to `max_digits` ASCII decimal digits at `*i` into a u32. Returns `None`
/// if there is not at least one digit. Advances `*i` past the digits. Never panics,
/// never overflows (bounded digit count).
fn parse_uint(bytes: &[u8], i: &mut usize, max_digits: usize) -> Option<u32> {
    let start = *i;
    let mut val: u32 = 0;
    let mut n = 0usize;
    while *i < bytes.len() && n < max_digits && bytes[*i].is_ascii_digit() {
        val = val
            .saturating_mul(10)
            .saturating_add((bytes[*i] - b'0') as u32);
        *i += 1;
        n += 1;
    }
    if *i == start {
        None
    } else {
        Some(val)
    }
}

// ===========================================================================
// Curated IANA-name → POSIX-string map
// ===========================================================================

/// Resolve a common IANA zone name (e.g. `"America/New_York"`) to its POSIX TZ
/// string. This is a **curated subset** (~10 of the most common zones), NOT a full
/// tzdata — an unknown name returns `None`. Case-insensitive on the name.
///
/// The strings encode each zone's *current* rules (post-2007 US, EU, AU). Historical
/// rule changes are out of scope (see the module docs).
pub fn tz_for_iana(name: &str) -> Option<&'static str> {
    // Case-insensitive match without allocating: compare ascii-lowercased bytes.
    const TABLE: &[(&str, &str)] = &[
        ("utc", "UTC0"),
        ("etc/utc", "UTC0"),
        ("america/new_york", "EST5EDT,M3.2.0,M11.1.0"),
        ("america/chicago", "CST6CDT,M3.2.0,M11.1.0"),
        ("america/denver", "MST7MDT,M3.2.0,M11.1.0"),
        ("america/los_angeles", "PST8PDT,M3.2.0,M11.1.0"),
        ("america/phoenix", "MST7"), // Arizona: no DST
        ("europe/london", "GMT0BST,M3.5.0/1,M10.5.0/2"),
        ("europe/paris", "CET-1CEST,M3.5.0,M10.5.0/3"),
        ("europe/berlin", "CET-1CEST,M3.5.0,M10.5.0/3"),
        ("australia/sydney", "AEST-10AEDT,M10.1.0,M4.1.0/3"),
        ("asia/kolkata", "<+0530>-5:30"),
        ("asia/tokyo", "JST-9"),
    ];
    for (k, v) in TABLE {
        if k.len() == name.len()
            && k.bytes()
                .zip(name.bytes())
                .all(|(a, b)| a == b.to_ascii_lowercase())
        {
            return Some(v);
        }
    }
    None
}

/// Resolve an IANA name (via [`tz_for_iana`]) and parse it into a [`TzInfo`]. Returns
/// `None` if the name is not in the curated map, `Some(Err)` if (defensively) a map
/// entry failed to parse — never panics.
pub fn tzinfo_for_iana(name: &str) -> Option<Result<TzInfo, TzError>> {
    tz_for_iana(name).map(parse_tz)
}

// ===========================================================================
// ath_pim integration: normalise a calendar DateTime to UTC / a target zone
// ===========================================================================

/// Normalise a calendar [`DateTime`] to **UTC**, resolving its `tz` (a `TZID`,
/// either an IANA name or a raw POSIX string) and `utc` flag.
///
/// - If `dt.utc` is already set, the value is returned unchanged (already UTC).
/// - If `dt.is_date` (date-only, no time-of-day), it is returned unchanged — a
///   floating DATE has no instant to shift.
/// - Otherwise the `TZID` is resolved: first as a curated IANA name, then as a raw
///   POSIX TZ string. On success the *local* civil time is converted to UTC via
///   [`TzInfo::local_to_utc`]; on failure (unknown/garbage TZID) the value is
///   returned unchanged (best-effort — never errors out the surrounding import).
///
/// Additive: does not alter any existing parse API. Never panics.
pub fn to_utc(dt: &DateTime) -> DateTime {
    if dt.utc || dt.is_date {
        return dt.clone();
    }
    let tzid = match dt.tz.as_deref() {
        Some(t) if !t.is_empty() => t,
        _ => return dt.clone(),
    };
    let info = match resolve_tzid(tzid) {
        Some(i) => i,
        None => return dt.clone(),
    };
    info.local_to_utc(dt)
}

/// Normalise a calendar [`DateTime`] into a *target* zone's local wall-clock time.
/// The source is first taken to UTC (via [`to_utc`]), then projected into
/// `target` ([`TzInfo::to_local`]). A date-only value is returned unchanged. Never
/// panics.
pub fn to_zone(dt: &DateTime, target: &TzInfo) -> DateTime {
    if dt.is_date {
        return dt.clone();
    }
    let utc = if dt.utc { dt.clone() } else { to_utc(dt) };
    // `to_utc` only yields UTC if it could resolve the source zone; if it couldn't
    // (still local, utc flag clear), we cannot honestly project — return as-is.
    if !utc.utc {
        return dt.clone();
    }
    target.to_local(&utc)
}

/// Resolve a TZID string (IANA curated name first, then raw POSIX) to a [`TzInfo`].
fn resolve_tzid(tzid: &str) -> Option<TzInfo> {
    if let Some(s) = tz_for_iana(tzid) {
        return parse_tz(s).ok();
    }
    parse_tz(tzid).ok()
}

// ===========================================================================
// Host KAT suite — the FAIL-able proof (cargo test -p ath_pim)
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a UTC date-time for terse tests.
    fn utc(y: u16, mo: u8, d: u8, h: u8, mi: u8) -> DateTime {
        DateTime {
            year: y,
            month: mo,
            day: d,
            hour: h,
            minute: mi,
            second: 0,
            is_date: false,
            utc: true,
            tz: None,
            raw: String::new(),
        }
    }

    // ---- POSIX sign correctness (the load-bearing assert) ------------------

    #[test]
    fn posix_sign_is_inverted_us_eastern() {
        let tz = parse_tz("EST5EDT,M3.2.0,M11.1.0").expect("parse");
        assert_eq!(tz.std_abbr, "EST");
        assert_eq!(tz.dst_abbr.as_deref(), Some("EDT"));
        // "EST5" means UTC-5 → -5*3600 in east-positive seconds. THE classic bug.
        assert_eq!(tz.std_offset, -5 * 3600);
        // DST offset omitted → std + 1h = UTC-4.
        assert_eq!(tz.dst_offset, Some(-4 * 3600));
    }

    #[test]
    fn posix_sign_inverted_central_europe() {
        // "CET-1" means UTC+1; CEST = UTC+2.
        let tz = parse_tz("CET-1CEST,M3.5.0,M10.5.0/3").expect("parse");
        assert_eq!(tz.std_offset, 1 * 3600);
        assert_eq!(tz.dst_offset, Some(2 * 3600));
        assert_eq!(tz.std_abbr, "CET");
        assert_eq!(tz.dst_abbr.as_deref(), Some("CEST"));
    }

    #[test]
    fn bracketed_numeric_name_india_half_hour() {
        let tz = parse_tz("<+0530>-5:30").expect("parse");
        assert_eq!(tz.std_abbr, "+0530");
        // "-5:30" → UTC+5:30 = +19800s.
        assert_eq!(tz.std_offset, 5 * 3600 + 30 * 60);
        assert!(!tz.has_dst());
    }

    #[test]
    fn utc0_and_no_dst() {
        let tz = parse_tz("UTC0").expect("parse");
        assert_eq!(tz.std_offset, 0);
        assert_eq!(tz.std_abbr, "UTC");
        assert!(!tz.has_dst());
        assert!(!tz.is_dst_at(&utc(2024, 7, 1, 0, 0)));
        assert_eq!(tz.offset_seconds_at(&utc(2024, 7, 1, 0, 0)), 0);
    }

    // ---- DST in effect (US Eastern, 2024) ----------------------------------

    #[test]
    fn us_eastern_january_is_standard() {
        let tz = parse_tz("EST5EDT,M3.2.0,M11.1.0").unwrap();
        // Mid-January UTC → EST (-5h), DST off.
        let jan = utc(2024, 1, 15, 12, 0);
        assert!(!tz.is_dst_at(&jan));
        assert_eq!(tz.offset_seconds_at(&jan), -5 * 3600);
    }

    #[test]
    fn us_eastern_july_is_dst() {
        let tz = parse_tz("EST5EDT,M3.2.0,M11.1.0").unwrap();
        let jul = utc(2024, 7, 15, 12, 0);
        assert!(tz.is_dst_at(&jul));
        assert_eq!(tz.offset_seconds_at(&jul), -4 * 3600);
    }

    #[test]
    fn us_eastern_2024_transition_dates_compute() {
        // DST 2024 starts Sun Mar 10, ends Sun Nov 3 — via the M3.2.0 / M11.1.0
        // weekday-of-month calc. Assert the resolved civil days directly.
        let tz = parse_tz("EST5EDT,M3.2.0,M11.1.0").unwrap();
        let (start_rule, _) = tz.dst_start.as_ref().unwrap();
        let (end_rule, _) = tz.dst_end.as_ref().unwrap();
        // 2nd Sunday of March 2024 = Mar 10.
        assert_eq!(rule_day_of_month(start_rule, 2024), (3, 10));
        // 1st Sunday of November 2024 = Nov 3.
        assert_eq!(rule_day_of_month(end_rule, 2024), (11, 3));
    }

    #[test]
    fn us_eastern_spring_forward_boundary() {
        // The transition is at 02:00 local EST = 07:00 UTC on 2024-03-10.
        let tz = parse_tz("EST5EDT,M3.2.0,M11.1.0").unwrap();
        // Just before: 06:30 UTC → still EST.
        let before = utc(2024, 3, 10, 6, 30);
        assert!(!tz.is_dst_at(&before));
        // Just after: 07:30 UTC → EDT.
        let after = utc(2024, 3, 10, 7, 30);
        assert!(tz.is_dst_at(&after));
    }

    // ---- Southern hemisphere (Sydney) — DST wraps the new year -------------

    #[test]
    fn sydney_january_is_dst_july_is_standard() {
        // AEST-10 = UTC+10; AEDT = UTC+11. DST Oct..Apr (wraps the new year).
        let tz = parse_tz("AEST-10AEDT,M10.1.0,M4.1.0/3").unwrap();
        assert_eq!(tz.std_offset, 10 * 3600);
        assert_eq!(tz.dst_offset, Some(11 * 3600));
        // January → DST in effect (southern summer): +11h.
        let jan = utc(2024, 1, 15, 0, 0);
        assert!(tz.is_dst_at(&jan), "Sydney Jan must be DST");
        assert_eq!(tz.offset_seconds_at(&jan), 11 * 3600);
        // July → standard (southern winter): +10h.
        let jul = utc(2024, 7, 15, 0, 0);
        assert!(!tz.is_dst_at(&jul), "Sydney Jul must be standard");
        assert_eq!(tz.offset_seconds_at(&jul), 10 * 3600);
    }

    // ---- to_local across a DST boundary (spring-forward gap) ---------------

    #[test]
    fn to_local_eastern_across_spring_forward() {
        let tz = parse_tz("EST5EDT,M3.2.0,M11.1.0").unwrap();
        // 2024-03-10 06:30 UTC → 01:30 EST (still standard, before the 02:00 jump).
        let a = tz.to_local(&utc(2024, 3, 10, 6, 30));
        assert_eq!(
            (a.year, a.month, a.day, a.hour, a.minute),
            (2024, 3, 10, 1, 30)
        );
        assert_eq!(a.tz.as_deref(), Some("EST"));
        assert!(!a.utc);
        // 2024-03-10 07:30 UTC → 03:30 EDT (spring forward skipped 02:00–03:00).
        let b = tz.to_local(&utc(2024, 3, 10, 7, 30));
        assert_eq!(
            (b.year, b.month, b.day, b.hour, b.minute),
            (2024, 3, 10, 3, 30)
        );
        assert_eq!(b.tz.as_deref(), Some("EDT"));
    }

    #[test]
    fn to_local_known_summer_instant() {
        let tz = parse_tz("EST5EDT,M3.2.0,M11.1.0").unwrap();
        // 2024-07-04 16:00 UTC → 12:00 EDT.
        let local = tz.to_local(&utc(2024, 7, 4, 16, 0));
        assert_eq!(
            (local.year, local.month, local.day, local.hour, local.minute),
            (2024, 7, 4, 12, 0)
        );
        assert_eq!(local.tz.as_deref(), Some("EDT"));
    }

    // ---- local_to_utc inverse ----------------------------------------------

    #[test]
    fn local_to_utc_roundtrips_summer() {
        let tz = parse_tz("EST5EDT,M3.2.0,M11.1.0").unwrap();
        let original = utc(2024, 7, 4, 16, 0);
        let local = tz.to_local(&original);
        let back = tz.local_to_utc(&local);
        assert_eq!(
            (back.year, back.month, back.day, back.hour, back.minute),
            (
                original.year,
                original.month,
                original.day,
                original.hour,
                original.minute
            )
        );
        assert!(back.utc);
    }

    #[test]
    fn local_to_utc_roundtrips_winter() {
        let tz = parse_tz("CET-1CEST,M3.5.0,M10.5.0/3").unwrap();
        let original = utc(2024, 1, 15, 9, 0);
        let local = tz.to_local(&original); // CET = UTC+1 → 10:00
        assert_eq!(local.hour, 10);
        let back = tz.local_to_utc(&local);
        assert_eq!((back.hour, back.minute), (9, 0));
        assert!(back.utc);
    }

    // ---- IANA curated map --------------------------------------------------

    #[test]
    fn iana_name_resolution_curated_set() {
        assert_eq!(
            tz_for_iana("America/New_York"),
            Some("EST5EDT,M3.2.0,M11.1.0")
        );
        assert_eq!(
            tz_for_iana("america/new_york"),
            Some("EST5EDT,M3.2.0,M11.1.0")
        ); // case-insensitive
        assert_eq!(
            tz_for_iana("Europe/London"),
            Some("GMT0BST,M3.5.0/1,M10.5.0/2")
        );
        assert_eq!(
            tz_for_iana("Europe/Paris"),
            Some("CET-1CEST,M3.5.0,M10.5.0/3")
        );
        assert_eq!(
            tz_for_iana("America/Los_Angeles"),
            Some("PST8PDT,M3.2.0,M11.1.0")
        );
        assert_eq!(
            tz_for_iana("Australia/Sydney"),
            Some("AEST-10AEDT,M10.1.0,M4.1.0/3")
        );
        assert_eq!(tz_for_iana("Asia/Kolkata"), Some("<+0530>-5:30"));
        assert_eq!(tz_for_iana("UTC"), Some("UTC0"));
        // Unknown → None.
        assert_eq!(tz_for_iana("Mars/Olympus_Mons"), None);
        assert_eq!(tz_for_iana("America/Atlantis"), None);
        // tzinfo_for_iana resolves+parses.
        let ny = tzinfo_for_iana("America/New_York").unwrap().unwrap();
        assert_eq!(ny.std_offset, -5 * 3600);
        assert!(tzinfo_for_iana("nope/nope").is_none());
    }

    #[test]
    fn paris_dst_via_iana_resolution() {
        // Resolve Europe/Paris and check a summer/winter instant.
        let tz = tzinfo_for_iana("Europe/Paris").unwrap().unwrap();
        let jul = utc(2024, 7, 1, 0, 0);
        assert!(tz.is_dst_at(&jul));
        assert_eq!(tz.offset_seconds_at(&jul), 2 * 3600); // CEST = UTC+2
        let jan = utc(2024, 1, 1, 0, 0);
        assert!(!tz.is_dst_at(&jan));
        assert_eq!(tz.offset_seconds_at(&jan), 1 * 3600); // CET = UTC+1
    }

    // ---- Jn / n / last-week transition forms -------------------------------

    #[test]
    fn julian_no_leap_form() {
        // J60 = day 60 ignoring Feb 29 → always March 1.
        let r = TransRule::JulianNoLeap(60);
        assert_eq!(rule_day_of_month(&r, 2024), (3, 1)); // leap year, still Mar 1
        assert_eq!(rule_day_of_month(&r, 2023), (3, 1));
        // J1 = Jan 1.
        assert_eq!(rule_day_of_month(&TransRule::JulianNoLeap(1), 2024), (1, 1));
    }

    #[test]
    fn zero_based_form_counts_leap() {
        // n=59 (zero-based) in a leap year = Feb 29 (0=Jan1, 31+28=59th index = Feb29).
        let r = TransRule::ZeroBased(59);
        assert_eq!(rule_day_of_month(&r, 2024), (2, 29)); // leap: counts Feb 29
        assert_eq!(rule_day_of_month(&r, 2023), (3, 1)); // non-leap: Mar 1
        assert_eq!(rule_day_of_month(&TransRule::ZeroBased(0), 2024), (1, 1));
    }

    #[test]
    fn last_week_form_m5() {
        // M3.5.0 = last Sunday of March. 2024: Mar 31 is a Sunday.
        let r = TransRule::Month {
            month: 3,
            week: 5,
            weekday: 0,
        };
        assert_eq!(rule_day_of_month(&r, 2024), (3, 31));
        // M10.5.0 = last Sunday of October 2024 = Oct 27.
        let r2 = TransRule::Month {
            month: 10,
            week: 5,
            weekday: 0,
        };
        assert_eq!(rule_day_of_month(&r2, 2024), (10, 27));
    }

    #[test]
    fn london_dst_uses_last_sunday_rules() {
        // GMT0BST,M3.5.0/1,M10.5.0/2 — DST Mar 31 .. Oct 27 (2024).
        let tz = parse_tz("GMT0BST,M3.5.0/1,M10.5.0/2").unwrap();
        assert_eq!(tz.std_offset, 0);
        assert_eq!(tz.dst_offset, Some(3600)); // BST = UTC+1
                                               // Transition at 01:00 UTC Mar 31 (M3.5.0/1: 01:00 local GMT == 01:00 UTC).
        assert!(!tz.is_dst_at(&utc(2024, 3, 31, 0, 30)));
        assert!(tz.is_dst_at(&utc(2024, 3, 31, 1, 30)));
        // July is BST.
        assert!(tz.is_dst_at(&utc(2024, 7, 1, 0, 0)));
    }

    // ---- ath_pim integration: to_utc / to_zone ----------------------------

    #[test]
    fn to_utc_resolves_tzid_from_ics() {
        // A DTSTART with TZID=America/New_York, local 09:30 in July (EDT, UTC-4)
        // → 13:30 UTC.
        let dt = DateTime {
            year: 2024,
            month: 7,
            day: 15,
            hour: 9,
            minute: 30,
            second: 0,
            is_date: false,
            utc: false,
            tz: Some("America/New_York".to_string()),
            raw: String::new(),
        };
        let u = to_utc(&dt);
        assert!(u.utc);
        assert_eq!(
            (u.year, u.month, u.day, u.hour, u.minute),
            (2024, 7, 15, 13, 30)
        );
    }

    #[test]
    fn to_utc_already_utc_unchanged() {
        let dt = utc(2024, 1, 1, 12, 0);
        let u = to_utc(&dt);
        assert_eq!((u.hour, u.minute), (12, 0));
        assert!(u.utc);
    }

    #[test]
    fn to_utc_date_only_unchanged() {
        let mut dt = utc(2024, 1, 1, 0, 0);
        dt.utc = false;
        dt.is_date = true;
        dt.tz = Some("America/New_York".to_string());
        let u = to_utc(&dt);
        assert!(u.is_date);
        assert!(!u.utc); // floating date untouched
    }

    #[test]
    fn to_utc_unknown_tzid_best_effort_unchanged() {
        let dt = DateTime {
            year: 2024,
            month: 7,
            day: 15,
            hour: 9,
            minute: 30,
            second: 0,
            is_date: false,
            utc: false,
            tz: Some("Garbage/Nowhere".to_string()),
            raw: String::new(),
        };
        let u = to_utc(&dt);
        // Could not resolve → returned unchanged (still local), never panicked.
        assert!(!u.utc);
        assert_eq!((u.hour, u.minute), (9, 30));
    }

    #[test]
    fn to_zone_projects_into_target() {
        // 16:00 UTC on 2024-07-04 → Sydney (AEDT? no — July is AEST = UTC+10) = 02:00
        // next day.
        let src = utc(2024, 7, 4, 16, 0);
        let sydney = parse_tz("AEST-10AEDT,M10.1.0,M4.1.0/3").unwrap();
        let local = to_zone(&src, &sydney);
        assert_eq!(
            (local.year, local.month, local.day, local.hour),
            (2024, 7, 5, 2)
        );
        assert_eq!(local.tz.as_deref(), Some("AEST"));
    }

    // ---- never-panic / malformed -------------------------------------------

    #[test]
    fn malformed_strings_error_never_panic() {
        assert_eq!(parse_tz(""), Err(TzError::Empty));
        assert_eq!(parse_tz("   "), Err(TzError::Empty));
        // No offset after the std name.
        assert_eq!(parse_tz("EST"), Err(TzError::BadStd));
        // Garbage / no alpha name and no offset.
        assert!(parse_tz("ZZZ").is_err());
        assert!(parse_tz("123").is_err());
        assert!(parse_tz("!!!").is_err());
        // DST name but no transition rules → malformed.
        assert!(parse_tz("EST5EDT").is_err());
        // Unclosed bracket.
        assert!(parse_tz("<+05-5").is_err());
        // Leading ':' (implementation-defined file path) rejected.
        assert!(parse_tz(":/etc/localtime").is_err());
        // Trailing junk after the rules.
        assert!(parse_tz("EST5EDT,M3.2.0,M11.1.0,garbage").is_err());
        // Out-of-range month/week.
        assert!(parse_tz("EST5EDT,M13.2.0,M11.1.0").is_err());
        assert!(parse_tz("EST5EDT,M3.6.0,M11.1.0").is_err());
    }

    #[test]
    fn seeded_fuzz_never_panics() {
        // Deterministic LCG over arbitrary bytes turned into TZ candidate strings:
        // a pure panic/loop safety proof. Bounded iterations and input length.
        let mut state: u64 = 0x5A5A_1234_DEAD_BEEF;
        let mut rng = || {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (state >> 33) as u32
        };
        let alphabet = b"ESTDPCM<>+-:.,M0123456789JZ abcXYZ/";
        for _ in 0..5000 {
            let n = (rng() % 40) as usize;
            let mut s = String::with_capacity(n);
            for _ in 0..n {
                let idx = (rng() as usize) % alphabet.len();
                s.push(alphabet[idx] as char);
            }
            // Must not panic, must terminate; result is don't-care.
            let r = parse_tz(&s);
            if let Ok(info) = r {
                // Exercise the resolver paths too — they must also never panic.
                let _ = info.is_dst_at(&utc(2024, 6, 15, 12, 0));
                let _ = info.offset_seconds_at(&utc(2024, 1, 15, 12, 0));
                let _ = info.to_local(&utc(2024, 3, 10, 7, 30));
                let _ = info.local_to_utc(&utc(2024, 11, 3, 6, 30));
            }
        }
        // Also fuzz the IANA resolver with the same strings.
        for _ in 0..1000 {
            let n = (rng() % 30) as usize;
            let mut s = String::with_capacity(n);
            for _ in 0..n {
                let idx = (rng() as usize) % alphabet.len();
                s.push(alphabet[idx] as char);
            }
            let _ = tz_for_iana(&s);
        }
    }

    // ---- FAIL-able sentinel ------------------------------------------------

    #[test]
    fn failable_offset_summary_string() {
        // A flat string of the resolved offsets across the year — tweak any expected
        // value and this turns red, proving the assert can FAIL.
        let tz = parse_tz("EST5EDT,M3.2.0,M11.1.0").unwrap();
        let s = alloc::format!(
            "jan={} jul={} dstJul={} dstJan={}",
            tz.offset_seconds_at(&utc(2024, 1, 15, 12, 0)),
            tz.offset_seconds_at(&utc(2024, 7, 15, 12, 0)),
            tz.is_dst_at(&utc(2024, 7, 15, 12, 0)),
            tz.is_dst_at(&utc(2024, 1, 15, 12, 0)),
        );
        assert_eq!(s, "jan=-18000 jul=-14400 dstJul=true dstJan=false");
    }
}
