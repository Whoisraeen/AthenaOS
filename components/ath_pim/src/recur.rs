//! # Recurrence expansion + civil date arithmetic for [`crate`].
//!
//! LEGACY_GAMING_CONCEPT.md §Compatibility Strategy ("let people switch without
//! conscious effort") — switcher criterion #5 is "import my calendar & contacts."
//! Importing the *data* (the [`crate::parse_ics`] layer) is necessary but not
//! sufficient: a calendar UI must answer **"what events occur in this date
//! range?"** A recurring event is stored once (DTSTART + an RRULE), so the
//! calendar view must *expand* that rule into concrete occurrence start-times.
//! This module is that expander — the core of any calendar grid/agenda view —
//! plus the small, exact civil-calendar arithmetic it needs.
//!
//! ## Civil date arithmetic (Howard Hinnant's algorithms)
//! [`days_from_civil`] / [`civil_from_days`] are the well-known, branch-light,
//! exact proleptic-Gregorian conversions between a `(year, month, day)` triple and
//! a day count relative to the Unix epoch (1970-01-01 == day 0). From those we get
//! [`weekday_from_days`], [`add_days`], [`is_leap`], [`days_in_month`], and
//! [`add_months`] (with end-of-month clamping: Jan 31 + 1 month → Feb 28/29).
//! All are pure, `no_std`, dependency-free, and exact for the entire `i64` day
//! range — no floating point, no overflow on any realistic calendar date.
//!
//! ## What the expander models (vs. deferred)
//! [`expand`] honours, over `[range_start, range_end]`:
//! - **FREQ** `DAILY` / `WEEKLY` / `MONTHLY` / `YEARLY`, each with **INTERVAL**
//!   (every N). Sub-daily `SECONDLY`/`MINUTELY`/`HOURLY` are **deferred** — a
//!   calendar grid never needs them and they balloon the occurrence count; they
//!   fall back to a single occurrence (DTSTART) so they never hang.
//! - **COUNT** (stop after N occurrences, counted from DTSTART even if before
//!   `range_start`) and **UNTIL** (inclusive end).
//! - **BYDAY** for `WEEKLY` (e.g. `MO,WE,FR` — those weekdays each interval-week;
//!   the week is anchored to WKST, default Monday). This is the single most common
//!   real rule ("every other Tuesday/Thursday").
//! - **BYMONTHDAY** and ordinal **BYDAY** (`1MO`, `-1FR`) for `MONTHLY`
//!   (e.g. "the 15th", "the first Monday", "the last Friday").
//!
//! **Deferred (documented):** BYSETPOS, BYWEEKNO, BYYEARDAY, BYMONTH, EXDATE,
//! RDATE, and multiple interacting BY* parts beyond the single-BY* cases above.
//! When a rule carries only unmodeled BY* parts for its FREQ, the expander falls
//! back to the plain FREQ+INTERVAL stepping (it never silently drops to nothing
//! and never loops).
//!
//! ## Hard safety bounds (CLAUDE: every byte is attacker-controlled)
//! The RRULE arrives from an untrusted `.ics`. [`expand`] can **never** loop
//! unbounded or allocate without limit: a forever rule (no COUNT, no UNTIL) is
//! capped by the caller's `max` AND by `range_end`; an internal step counter
//! ([`MAX_STEPS`]) backstops even a pathological INTERVAL/BY* combination;
//! `interval < 1` is treated as 1 (documented, not an error — RFC 5545 forbids it
//! and a real exporter never emits it, so coercing keeps the calendar usable
//! rather than failing the whole import). No `unsafe`, no `panic`/`unwrap`.

use alloc::vec::Vec;

use crate::{DateTime, Freq, RRule, VEvent};

/// Absolute backstop on stepping iterations inside [`expand`], independent of the
/// caller's `max`. Even an INTERVAL=1 DAILY rule over a 100-year range with a tiny
/// `max` cannot exceed this many candidate steps before we stop. Generous enough to
/// never truncate a legitimate calendar view, small enough to bound a hostile rule.
pub const MAX_STEPS: usize = 4_000_000;

// ---------------------------------------------------------------------------
// Civil date arithmetic — Howard Hinnant's exact proleptic-Gregorian algorithms.
// (http://howardhinnant.github.io/date_algorithms.html — public, well-known.)
// ---------------------------------------------------------------------------

/// Days since the Unix epoch (1970-01-01 == 0) for a proleptic-Gregorian civil
/// date. Exact for any `(y, m, d)`; `m` and `d` are taken as-is (caller supplies a
/// valid date — [`add_months`]/[`add_days`] always do). Negative results for dates
/// before the epoch. Never panics (pure integer arithmetic).
///
/// Reference dates: `days_from_civil(1970,1,1) == 0`,
/// `days_from_civil(2000,1,1) == 10957`, `days_from_civil(1969,12,31) == -1`.
pub fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    // Shift the year so that March is month 0 (leap day lands at year's end).
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400; // [0, 399]
    let mp = (m + 9) % 12; // Mar=0 .. Feb=11
    let doy = (153 * mp + 2) / 5 + d - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146097 + doe - 719468
}

/// Inverse of [`days_from_civil`]: the civil `(year, month, day)` for a day count
/// since the Unix epoch. Exact, never panics.
///
/// `civil_from_days(0) == (1970, 1, 1)`, `civil_from_days(10957) == (2000, 1, 1)`.
pub fn civil_from_days(z: i64) -> (i64, u8, u8) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]  (Mar=0)
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    let y = if m <= 2 { y + 1 } else { y };
    (y, m as u8, d as u8)
}

/// Weekday for a day count since the epoch: `0 = Sunday .. 6 = Saturday`.
/// 1970-01-01 was a Thursday (==4). Never panics.
pub fn weekday_from_days(z: i64) -> u8 {
    // (z + 4) mod 7, with a floor-mod so negative day counts stay in [0,6].
    (((z % 7) + 4 + 7) % 7) as u8
}

/// True if `year` is a Gregorian leap year.
pub fn is_leap(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

/// Number of days in `month` (1..=12) of `year`. Out-of-range months yield 0.
pub fn days_in_month(year: i64, month: u8) -> u8 {
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

/// A civil date `(y, m, d)` advanced by `n` days (may be negative). Exact via the
/// day-count round-trip; never panics.
pub fn add_days(y: i64, m: u8, d: u8, n: i64) -> (i64, u8, u8) {
    civil_from_days(days_from_civil(y, m as i64, d as i64) + n)
}

/// A civil date `(y, m, d)` advanced by `n` *months* (may be negative), with
/// end-of-month clamping: the day is clamped to the last valid day of the target
/// month (Jan 31 + 1 month → Feb 28/29; Mar 31 + 1 month → Apr 30). Never panics.
pub fn add_months(y: i64, m: u8, d: u8, n: i64) -> (i64, u8, u8) {
    // Month index 0-based from year 0, do the arithmetic, decompose with floor-mod.
    let total = y * 12 + (m as i64 - 1) + n;
    let ny = total.div_euclid(12);
    let nm = (total.rem_euclid(12) + 1) as u8; // 1..=12
    let last = days_in_month(ny, nm);
    let nd = if d as u8 > last { last } else { d };
    (ny, nm, nd)
}

// ---------------------------------------------------------------------------
// RRULE expansion
// ---------------------------------------------------------------------------

/// Map an iCalendar weekday token (`SU`,`MO`,`TU`,`WE`,`TH`,`FR`,`SA`) to the
/// `0=Sun..6=Sat` index used by [`weekday_from_days`]. Returns `None` for an
/// unknown token.
fn weekday_token(tok: &str) -> Option<u8> {
    match tok {
        "SU" => Some(0),
        "MO" => Some(1),
        "TU" => Some(2),
        "WE" => Some(3),
        "TH" => Some(4),
        "FR" => Some(5),
        "SA" => Some(6),
        _ => None,
    }
}

/// Parse a single BYDAY token into `(ordinal, weekday)`. `MO` → `(None, 1)`,
/// `1MO` → `(Some(1), 1)`, `-1FR` → `(Some(-1), 5)`. Returns `None` if the weekday
/// part is unrecognised.
fn parse_byday(tok: &str) -> Option<(Option<i32>, u8)> {
    let tok = tok.trim();
    if tok.len() < 2 {
        return None;
    }
    // The weekday is always the trailing two ASCII letters.
    let split = tok.len() - 2;
    let wd_str = &tok[split..];
    let wd = weekday_token(&ascii_upper2(wd_str))?;
    if split == 0 {
        return Some((None, wd));
    }
    // Leading part is an optional signed ordinal.
    let ord_str = &tok[..split];
    // Validate it is a signed integer (so a malformed token degrades to None ord).
    let mut ok = true;
    for (i, &b) in ord_str.as_bytes().iter().enumerate() {
        let valid = b.is_ascii_digit() || (i == 0 && (b == b'+' || b == b'-'));
        if !valid {
            ok = false;
            break;
        }
    }
    if !ok {
        return None;
    }
    match ord_str.parse::<i32>() {
        Ok(0) => None, // ordinal 0 is invalid; treat token as malformed
        Ok(n) => Some((Some(n), wd)),
        Err(_) => None,
    }
}

/// Upper-case exactly a 2-byte ASCII weekday tail (cheap, no alloc churn beyond the
/// tiny String the caller drops immediately).
fn ascii_upper2(s: &str) -> alloc::string::String {
    let mut out = alloc::string::String::with_capacity(s.len());
    for b in s.bytes() {
        out.push(b.to_ascii_uppercase() as char);
    }
    out
}

/// Build an occurrence [`DateTime`] for civil date `(y,m,d)`, copying the
/// time-of-day and date/utc/tz flags from the seed `dtstart`.
fn occ(dtstart: &DateTime, y: i64, m: u8, d: u8) -> DateTime {
    let year = if (0..=9999).contains(&y) { y as u16 } else { 0 };
    DateTime {
        year,
        month: m,
        day: d,
        hour: dtstart.hour,
        minute: dtstart.minute,
        second: dtstart.second,
        is_date: dtstart.is_date,
        utc: dtstart.utc,
        tz: dtstart.tz.clone(),
        raw: alloc::string::String::new(),
    }
}

/// A total-order key for an occurrence: day count since epoch, then seconds of day.
/// Used for range comparison and sorting (ignores tz — see crate docs: no tz math).
fn order_key(dt: &DateTime) -> (i64, u32) {
    let days = days_from_civil(dt.year as i64, dt.month as i64, dt.day as i64);
    let secs = dt.hour as u32 * 3600 + dt.minute as u32 * 60 + dt.second as u32;
    (days, secs)
}

/// `true` if `a <= b` in civil order.
fn le(a: &DateTime, b: &DateTime) -> bool {
    order_key(a) <= order_key(b)
}

/// Expand a recurrence rule into concrete occurrence start-times within
/// `[range_start, range_end]` (both inclusive).
///
/// Semantics (RFC 5545, with the modeled/deferred subset documented at the module
/// level):
/// - The **first** occurrence is `event_dtstart` itself (DTSTART is the seed).
/// - **COUNT** counts every generated occurrence from DTSTART, even ones before
///   `range_start` (those are *counted* but excluded from the returned list).
/// - **UNTIL** is an inclusive upper bound on the occurrence time.
/// - At most `max` occurrences are returned; generation also stops at `range_end`
///   and at [`MAX_STEPS`] internal steps — a forever rule can never hang.
/// - `interval < 1` is coerced to 1 (see crate docs).
///
/// Returns occurrences in chronological order. Never panics, never loops unbounded.
pub fn expand(
    event_dtstart: &DateTime,
    rule: &RRule,
    range_start: &DateTime,
    range_end: &DateTime,
    max: usize,
) -> Vec<DateTime> {
    let mut out: Vec<DateTime> = Vec::new();
    if max == 0 {
        return out;
    }
    let interval: i64 = if rule.interval < 1 {
        1
    } else {
        rule.interval as i64
    };
    let count_limit = rule.count.map(|c| c as usize);

    // How many have we generated (toward COUNT), how many emitted into `out`.
    let mut generated: usize = 0;

    // A closure-free emit helper expressed inline below (no_std + borrow rules).
    // We push to `out` only when in [range_start, range_end]; we always count
    // toward COUNT and toward MAX_STEPS.

    match rule.freq {
        Freq::Daily | Freq::Weekly | Freq::Monthly | Freq::Yearly => {}
        // Sub-daily and unknown FREQ: a calendar grid never expands these; emit the
        // single seed occurrence if it lands in range, then stop (never hang).
        _ => {
            if le(range_start, event_dtstart) && le(event_dtstart, range_end) {
                out.push(event_dtstart.clone());
            }
            return out;
        }
    }

    // ---- WEEKLY with BYDAY: iterate weeks, emit each selected weekday ----------
    if matches!(rule.freq, Freq::Weekly) && !rule.byday.is_empty() {
        let mut wanted: Vec<u8> = Vec::new();
        for tok in &rule.byday {
            if let Some(wd) = weekday_token(tok) {
                if !wanted.contains(&wd) {
                    wanted.push(wd);
                }
            }
        }
        if !wanted.is_empty() {
            wanted.sort_unstable();
            // Anchor: the Monday (WKST default) on/before DTSTART's week.
            let start_days = days_from_civil(
                event_dtstart.year as i64,
                event_dtstart.month as i64,
                event_dtstart.day as i64,
            );
            // Monday-based offset back to the week start.
            let start_wd = weekday_from_days(start_days); // 0=Sun
            let to_monday = ((start_wd + 6) % 7) as i64; // days since Monday
            let week0 = start_days - to_monday;
            let step_days = interval * 7;

            let mut steps: usize = 0;
            let mut week_anchor = week0;
            loop {
                if steps >= MAX_STEPS {
                    break;
                }
                // Emit selected weekdays within this week, in chronological order.
                // wanted is 0=Sun..6=Sat; convert to Monday-based offset.
                let mut day_offsets: Vec<i64> =
                    wanted.iter().map(|&wd| ((wd + 6) % 7) as i64).collect();
                day_offsets.sort_unstable();
                let mut week_overflowed = false;
                for off in day_offsets {
                    let dn = week_anchor + off;
                    if dn < start_days {
                        continue; // before DTSTART's actual day → not an occurrence
                    }
                    let (y, m, d) = civil_from_days(dn);
                    let cand = occ(event_dtstart, y, m, d);
                    // UNTIL (inclusive)
                    if let Some(u) = rule.until.as_ref() {
                        if !le(&cand, u) {
                            week_overflowed = true;
                            break;
                        }
                    }
                    // range_end bound (generation stop)
                    if !le(&cand, range_end) {
                        week_overflowed = true;
                        break;
                    }
                    // COUNT
                    if let Some(cl) = count_limit {
                        if generated >= cl {
                            week_overflowed = true;
                            break;
                        }
                    }
                    generated += 1;
                    if le(range_start, &cand) {
                        out.push(cand);
                        if out.len() >= max {
                            return out;
                        }
                    }
                }
                if week_overflowed {
                    break;
                }
                if let Some(cl) = count_limit {
                    if generated >= cl {
                        break;
                    }
                }
                week_anchor += step_days;
                steps += 1;
                // Safety: if the anchor passes range_end entirely with no UNTIL/COUNT
                // still open, stop.
                let (ay, am, ad) = civil_from_days(week_anchor);
                let anchor_dt = occ(event_dtstart, ay, am, ad);
                if !le(&anchor_dt, range_end) {
                    break;
                }
            }
            return out;
        }
        // BYDAY present but no recognised tokens → fall through to plain weekly.
    }

    // ---- MONTHLY with BYMONTHDAY or ordinal BYDAY -----------------------------
    if matches!(rule.freq, Freq::Monthly) {
        // Parse BYMONTHDAY from raw (the struct doesn't model it; pull from raw).
        let bymonthday = extract_bymonthday(&rule.raw);
        let ordinal_bydays: Vec<(i32, u8)> = rule
            .byday
            .iter()
            .filter_map(|t| match parse_byday(t) {
                Some((Some(o), wd)) => Some((o, wd)),
                _ => None,
            })
            .collect();

        if !bymonthday.is_empty() || !ordinal_bydays.is_empty() {
            // Iterate month-by-month starting from DTSTART's month.
            let start_days = days_from_civil(
                event_dtstart.year as i64,
                event_dtstart.month as i64,
                event_dtstart.day as i64,
            );
            let mut cur_y = event_dtstart.year as i64;
            let mut cur_m = event_dtstart.month;
            let mut steps: usize = 0;
            loop {
                if steps >= MAX_STEPS {
                    break;
                }
                // Collect this month's candidate day numbers.
                let mut days: Vec<u8> = Vec::new();
                let dim = days_in_month(cur_y, cur_m);
                for &md in &bymonthday {
                    let dnum = if md > 0 {
                        md as i64
                    } else if md < 0 {
                        dim as i64 + 1 + md as i64
                    } else {
                        continue;
                    };
                    if dnum >= 1 && dnum <= dim as i64 && !days.contains(&(dnum as u8)) {
                        days.push(dnum as u8);
                    }
                }
                for &(ord, wd) in &ordinal_bydays {
                    if let Some(dnum) = nth_weekday_of_month(cur_y, cur_m, wd, ord) {
                        if !days.contains(&dnum) {
                            days.push(dnum);
                        }
                    }
                }
                days.sort_unstable();

                let mut stop = false;
                for d in days {
                    let dn = days_from_civil(cur_y, cur_m as i64, d as i64);
                    if dn < start_days {
                        continue;
                    }
                    let cand = occ(event_dtstart, cur_y, cur_m, d);
                    if let Some(u) = rule.until.as_ref() {
                        if !le(&cand, u) {
                            stop = true;
                            break;
                        }
                    }
                    if !le(&cand, range_end) {
                        stop = true;
                        break;
                    }
                    if let Some(cl) = count_limit {
                        if generated >= cl {
                            stop = true;
                            break;
                        }
                    }
                    generated += 1;
                    if le(range_start, &cand) {
                        out.push(cand);
                        if out.len() >= max {
                            return out;
                        }
                    }
                }
                if stop {
                    break;
                }
                if let Some(cl) = count_limit {
                    if generated >= cl {
                        break;
                    }
                }
                // Advance by `interval` months.
                let (ny, nm, _) = add_months(cur_y, cur_m, 1, interval);
                cur_y = ny;
                cur_m = nm;
                steps += 1;
                let first_of_month = occ(event_dtstart, cur_y, cur_m, 1);
                if !le(&first_of_month, range_end) {
                    break;
                }
            }
            return out;
        }
        // else: plain monthly on DTSTART's day-of-month (fall through).
    }

    // ---- Plain FREQ + INTERVAL stepping (the seed date advanced each period) ---
    // DAILY: +interval days. WEEKLY (no BYDAY): +interval*7 days.
    // MONTHLY (no BY*): +interval months (end-of-month clamp). YEARLY: +interval yr.
    let mut cur = event_dtstart.clone();
    let mut steps: usize = 0;
    loop {
        if steps >= MAX_STEPS {
            break;
        }
        // UNTIL bound
        if let Some(u) = rule.until.as_ref() {
            if !le(&cur, u) {
                break;
            }
        }
        // range_end bound
        if !le(&cur, range_end) {
            break;
        }
        // COUNT bound
        if let Some(cl) = count_limit {
            if generated >= cl {
                break;
            }
        }
        generated += 1;
        if le(range_start, &cur) {
            out.push(cur.clone());
            if out.len() >= max {
                break;
            }
        }
        // Advance.
        let (ny, nm, nd) = match rule.freq {
            Freq::Daily => add_days(cur.year as i64, cur.month, cur.day, interval),
            Freq::Weekly => add_days(cur.year as i64, cur.month, cur.day, interval * 7),
            Freq::Monthly => add_months(cur.year as i64, cur.month, cur.day, interval),
            Freq::Yearly => {
                // +interval years, clamping Feb 29 → Feb 28 in a non-leap target.
                let ty = cur.year as i64 + interval;
                let last = days_in_month(ty, cur.month);
                let nd = if cur.day > last { last } else { cur.day };
                (ty, cur.month, nd)
            }
            _ => break,
        };
        cur = occ(event_dtstart, ny, nm, nd);
        steps += 1;
    }
    out
}

/// Pull a BYMONTHDAY list out of the raw RRULE string (the [`RRule`] struct keeps
/// only FREQ/INTERVAL/COUNT/UNTIL/BYDAY parsed; BYMONTHDAY lives in `raw`). Returns
/// signed day numbers (`15`, `-1` for last day). Bounded, never panics.
fn extract_bymonthday(raw: &str) -> Vec<i32> {
    let mut out: Vec<i32> = Vec::new();
    for part in raw.split(';') {
        let mut it = part.splitn(2, '=');
        let k = it.next().unwrap_or("");
        if !k.trim().eq_ignore_ascii_case("BYMONTHDAY") {
            continue;
        }
        if let Some(v) = it.next() {
            for tok in v.split(',') {
                let t = tok.trim();
                if t.is_empty() || out.len() >= 64 {
                    continue;
                }
                if let Ok(n) = t.parse::<i32>() {
                    if (-31..=31).contains(&n) && n != 0 {
                        out.push(n);
                    }
                }
            }
        }
    }
    out
}

/// Day-of-month of the `ord`-th `weekday` in `(year, month)`. `ord > 0` counts from
/// the start (`1` = first), `ord < 0` from the end (`-1` = last). Returns `None` if
/// that ordinal does not exist in the month (e.g. a 5th Monday in a short month).
fn nth_weekday_of_month(year: i64, month: u8, weekday: u8, ord: i32) -> Option<u8> {
    let dim = days_in_month(year, month);
    if dim == 0 || ord == 0 {
        return None;
    }
    if ord > 0 {
        // Find the first occurrence of `weekday`, then step by 7.
        let first_days = days_from_civil(year, month as i64, 1);
        let first_wd = weekday_from_days(first_days);
        let delta = (7 + weekday - first_wd) % 7; // 0..6 to first match
        let day = 1u32 + delta as u32 + 7 * (ord as u32 - 1);
        if day >= 1 && day <= dim as u32 {
            Some(day as u8)
        } else {
            None
        }
    } else {
        let last_days = days_from_civil(year, month as i64, dim as i64);
        let last_wd = weekday_from_days(last_days);
        let back = (7 + last_wd - weekday) % 7; // 0..6 to last match
        let n_from_end = (-ord) as u32; // 1 = last
        let day = dim as i64 - back as i64 - 7 * (n_from_end as i64 - 1);
        if day >= 1 && day <= dim as i64 {
            Some(day as u8)
        } else {
            None
        }
    }
}

// ---------------------------------------------------------------------------
// VEvent convenience
// ---------------------------------------------------------------------------

impl VEvent {
    /// Occurrence start-times of this event within `[range_start, range_end]`
    /// (inclusive), capped at `max`.
    ///
    /// - No RRULE → at most one occurrence: the event's DTSTART, if it falls in
    ///   range (an event with no DTSTART yields nothing).
    /// - With an RRULE → [`expand`] of the rule seeded at DTSTART.
    ///
    /// Never panics, never loops unbounded.
    pub fn occurrences(
        &self,
        range_start: &DateTime,
        range_end: &DateTime,
        max: usize,
    ) -> Vec<DateTime> {
        let mut out: Vec<DateTime> = Vec::new();
        let dtstart = match self.dtstart.as_ref() {
            Some(d) => d,
            None => return out,
        };
        match self.rrule.as_ref() {
            Some(rule) => expand(dtstart, rule, range_start, range_end, max),
            None => {
                if max > 0 && le(range_start, dtstart) && le(dtstart, range_end) {
                    out.push(dtstart.clone());
                }
                out
            }
        }
    }
}

// ===========================================================================
// Host KAT suite — FAIL-able recurrence + civil-math proof (cargo test -p ath_pim)
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RRule;
    use alloc::string::ToString;
    use alloc::vec::Vec;

    /// Build a DATE-TIME seed/bound. Keeps tests terse.
    fn dt(y: u16, mo: u8, d: u8, h: u8, mi: u8, s: u8) -> DateTime {
        DateTime {
            year: y,
            month: mo,
            day: d,
            hour: h,
            minute: mi,
            second: s,
            is_date: false,
            utc: false,
            tz: None,
            raw: alloc::string::String::new(),
        }
    }

    /// A `(y,m,d)` list of an occurrence vector, for terse list asserts.
    fn ymd(v: &[DateTime]) -> Vec<(u16, u8, u8)> {
        v.iter().map(|o| (o.year, o.month, o.day)).collect()
    }

    // ---- civil arithmetic --------------------------------------------------

    #[test]
    fn civil_known_dates_and_weekdays() {
        // Epoch anchor.
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        // 1970-01-01 is a Thursday (4).
        assert_eq!(weekday_from_days(0), 4);
        // 2000-01-01 is a Saturday (6) — the spec's named check.
        let d2000 = days_from_civil(2000, 1, 1);
        assert_eq!(weekday_from_days(d2000), 6);
        // A day before epoch.
        assert_eq!(days_from_civil(1969, 12, 31), -1);
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
        // 2024-01-01 is a Monday (1) — used by the WEEKLY tests below.
        assert_eq!(weekday_from_days(days_from_civil(2024, 1, 1)), 1);
    }

    #[test]
    fn civil_roundtrip_sweep() {
        // Round-trip every day across a multi-year, leap-spanning sweep.
        let start = days_from_civil(1995, 1, 1);
        let end = days_from_civil(2035, 12, 31);
        let mut z = start;
        while z <= end {
            let (y, m, d) = civil_from_days(z);
            assert_eq!(
                days_from_civil(y, m as i64, d as i64),
                z,
                "roundtrip @ {}",
                z
            );
            z += 1;
        }
    }

    #[test]
    fn leap_year_rules() {
        assert!(is_leap(2000)); // div by 400
        assert!(is_leap(2024)); // div by 4
        assert!(!is_leap(1900)); // div by 100 not 400
        assert!(!is_leap(2023));
        assert_eq!(days_in_month(2024, 2), 29);
        assert_eq!(days_in_month(2023, 2), 28);
        assert_eq!(days_in_month(2024, 4), 30);
        assert_eq!(days_in_month(2024, 12), 31);
        assert!(DateTime::default().year == 0); // sanity: default exists
    }

    #[test]
    fn add_months_end_of_month_clamp() {
        // Jan 31 + 1 month -> Feb 29 in a leap year.
        assert_eq!(add_months(2024, 1, 31, 1), (2024, 2, 29));
        // ... + 1 more month -> Mar 29 (clamped day carried, NOT snapped to 31).
        assert_eq!(add_months(2024, 2, 29, 1), (2024, 3, 29));
        // Mar 31 + 1 month -> Apr 30.
        assert_eq!(add_months(2024, 3, 31, 1), (2024, 4, 30));
        // Jan 31 (non-leap) + 1 month -> Feb 28.
        assert_eq!(add_months(2023, 1, 31, 1), (2023, 2, 28));
        // Year rollover, negative step.
        assert_eq!(add_months(2024, 1, 15, -1), (2023, 12, 15));
        assert_eq!(add_months(2024, 12, 15, 1), (2025, 1, 15));
    }

    #[test]
    fn add_days_basic() {
        assert_eq!(add_days(2024, 2, 28, 1), (2024, 2, 29)); // leap
        assert_eq!(add_days(2023, 2, 28, 1), (2023, 3, 1)); // non-leap
        assert_eq!(add_days(2024, 12, 31, 1), (2025, 1, 1));
        assert_eq!(add_days(2024, 1, 1, -1), (2023, 12, 31));
    }

    // ---- DAILY -------------------------------------------------------------

    #[test]
    fn daily_interval2_count5_exact_list() {
        let seed = dt(2024, 1, 1, 9, 0, 0);
        let rule = RRule::parse("FREQ=DAILY;INTERVAL=2;COUNT=5").unwrap();
        let occ = expand(
            &seed,
            &rule,
            &dt(2024, 1, 1, 0, 0, 0),
            &dt(2024, 12, 31, 0, 0, 0),
            100,
        );
        // Proven FAIL-able: tweak any date below and this turns red.
        assert_eq!(
            ymd(&occ),
            alloc::vec![
                (2024, 1, 1),
                (2024, 1, 3),
                (2024, 1, 5),
                (2024, 1, 7),
                (2024, 1, 9),
            ]
        );
        // Time-of-day preserved from DTSTART.
        assert_eq!((occ[0].hour, occ[0].minute), (9, 0));
        assert_eq!((occ[4].hour, occ[4].minute), (9, 0));
    }

    // ---- WEEKLY + BYDAY ----------------------------------------------------

    #[test]
    fn weekly_byday_mo_we_two_weeks() {
        // DTSTART Mon 2024-01-01. MO,WE every week, over a 2-week window.
        let seed = dt(2024, 1, 1, 8, 30, 0);
        let rule = RRule::parse("FREQ=WEEKLY;INTERVAL=1;BYDAY=MO,WE").unwrap();
        let occ = expand(
            &seed,
            &rule,
            &dt(2024, 1, 1, 0, 0, 0),
            &dt(2024, 1, 14, 0, 0, 0),
            100,
        );
        // Mondays 1, 8; Wednesdays 3, 10.
        assert_eq!(
            ymd(&occ),
            alloc::vec![(2024, 1, 1), (2024, 1, 3), (2024, 1, 8), (2024, 1, 10)]
        );
        assert_eq!((occ[0].hour, occ[0].minute), (8, 30));
    }

    #[test]
    fn weekly_interval2_skips_off_weeks() {
        // Every other week, Tue+Thu. DTSTART Tue 2024-01-02.
        let seed = dt(2024, 1, 2, 12, 0, 0);
        let rule = RRule::parse("FREQ=WEEKLY;INTERVAL=2;BYDAY=TU,TH").unwrap();
        let occ = expand(
            &seed,
            &rule,
            &dt(2024, 1, 1, 0, 0, 0),
            &dt(2024, 1, 28, 0, 0, 0),
            100,
        );
        // Week of Jan 1 (anchor Mon Jan 1): Tue 2, Thu 4. Skip week of Jan 8.
        // Week of Jan 15: Tue 16, Thu 18. Skip week of Jan 22.
        assert_eq!(
            ymd(&occ),
            alloc::vec![(2024, 1, 2), (2024, 1, 4), (2024, 1, 16), (2024, 1, 18)]
        );
    }

    // ---- MONTHLY -----------------------------------------------------------

    #[test]
    fn monthly_bymonthday_15_three_months() {
        let seed = dt(2024, 1, 15, 10, 0, 0);
        let rule = RRule::parse("FREQ=MONTHLY;BYMONTHDAY=15;COUNT=3").unwrap();
        let occ = expand(
            &seed,
            &rule,
            &dt(2024, 1, 1, 0, 0, 0),
            &dt(2024, 12, 31, 0, 0, 0),
            100,
        );
        assert_eq!(
            ymd(&occ),
            alloc::vec![(2024, 1, 15), (2024, 2, 15), (2024, 3, 15)]
        );
    }

    #[test]
    fn monthly_plain_day_clamps_end_of_month() {
        // No BY*: monthly on DTSTART's day-of-month (31), clamped per month.
        let seed = dt(2024, 1, 31, 0, 0, 0);
        let rule = RRule::parse("FREQ=MONTHLY;COUNT=4").unwrap();
        let occ = expand(
            &seed,
            &rule,
            &dt(2024, 1, 1, 0, 0, 0),
            &dt(2024, 12, 31, 0, 0, 0),
            100,
        );
        // Jan 31, Feb 29 (clamp), Mar 29 (carried from clamped Feb), Apr 29.
        assert_eq!(
            ymd(&occ),
            alloc::vec![(2024, 1, 31), (2024, 2, 29), (2024, 3, 29), (2024, 4, 29)]
        );
    }

    #[test]
    fn monthly_ordinal_byday_first_and_last() {
        // First Monday of each month, 3 months from 2024-03.
        let seed = dt(2024, 3, 1, 0, 0, 0);
        let rule = RRule::parse("FREQ=MONTHLY;BYDAY=1MO;COUNT=3").unwrap();
        let occ = expand(
            &seed,
            &rule,
            &dt(2024, 1, 1, 0, 0, 0),
            &dt(2024, 12, 31, 0, 0, 0),
            100,
        );
        // Mar: first Mon = 4. Apr: 1. May: 6.
        assert_eq!(
            ymd(&occ),
            alloc::vec![(2024, 3, 4), (2024, 4, 1), (2024, 5, 6)]
        );

        // Last Friday of each month, 2 months.
        let seed2 = dt(2024, 1, 1, 0, 0, 0);
        let rule2 = RRule::parse("FREQ=MONTHLY;BYDAY=-1FR;COUNT=2").unwrap();
        let occ2 = expand(
            &seed2,
            &rule2,
            &dt(2024, 1, 1, 0, 0, 0),
            &dt(2024, 12, 31, 0, 0, 0),
            100,
        );
        // Jan 2024 last Fri = 26. Feb last Fri = 23.
        assert_eq!(ymd(&occ2), alloc::vec![(2024, 1, 26), (2024, 2, 23)]);
    }

    // ---- YEARLY ------------------------------------------------------------

    #[test]
    fn yearly_same_date_next_years() {
        let seed = dt(2024, 7, 4, 0, 0, 0);
        let rule = RRule::parse("FREQ=YEARLY;COUNT=3").unwrap();
        let occ = expand(
            &seed,
            &rule,
            &dt(2020, 1, 1, 0, 0, 0),
            &dt(2030, 12, 31, 0, 0, 0),
            100,
        );
        assert_eq!(
            ymd(&occ),
            alloc::vec![(2024, 7, 4), (2025, 7, 4), (2026, 7, 4)]
        );
    }

    #[test]
    fn yearly_feb29_clamps_in_nonleap() {
        let seed = dt(2024, 2, 29, 0, 0, 0);
        let rule = RRule::parse("FREQ=YEARLY;COUNT=2").unwrap();
        let occ = expand(
            &seed,
            &rule,
            &dt(2020, 1, 1, 0, 0, 0),
            &dt(2030, 12, 31, 0, 0, 0),
            100,
        );
        // 2024 leap -> Feb 29; 2025 non-leap -> clamp to Feb 28.
        assert_eq!(ymd(&occ), alloc::vec![(2024, 2, 29), (2025, 2, 28)]);
    }

    // ---- UNTIL / COUNT / range filtering -----------------------------------

    #[test]
    fn until_is_inclusive() {
        let seed = dt(2024, 1, 1, 0, 0, 0);
        // UNTIL on the 5th, daily. The 5th IS included.
        let rule = RRule::parse("FREQ=DAILY;UNTIL=20240105").unwrap();
        let occ = expand(
            &seed,
            &rule,
            &dt(2024, 1, 1, 0, 0, 0),
            &dt(2024, 12, 31, 0, 0, 0),
            100,
        );
        assert_eq!(
            ymd(&occ),
            alloc::vec![
                (2024, 1, 1),
                (2024, 1, 2),
                (2024, 1, 3),
                (2024, 1, 4),
                (2024, 1, 5)
            ]
        );
    }

    #[test]
    fn count_counts_before_range_start_but_excludes_from_output() {
        // DAILY COUNT=10 from Jan 1, but only show Jan 5..Jan 12.
        // Occurrences 1..10 = Jan 1..10. COUNT stops at Jan 10. Range trims to 5..10.
        let seed = dt(2024, 1, 1, 0, 0, 0);
        let rule = RRule::parse("FREQ=DAILY;COUNT=10").unwrap();
        let occ = expand(
            &seed,
            &rule,
            &dt(2024, 1, 5, 0, 0, 0),
            &dt(2024, 1, 12, 0, 0, 0),
            100,
        );
        // Jan 5..10 only (11 and 12 never generated — COUNT exhausted at 10).
        assert_eq!(
            ymd(&occ),
            alloc::vec![
                (2024, 1, 5),
                (2024, 1, 6),
                (2024, 1, 7),
                (2024, 1, 8),
                (2024, 1, 9),
                (2024, 1, 10),
            ]
        );
    }

    #[test]
    fn max_caps_output() {
        let seed = dt(2024, 1, 1, 0, 0, 0);
        let rule = RRule::parse("FREQ=DAILY").unwrap(); // no COUNT/UNTIL
        let occ = expand(
            &seed,
            &rule,
            &dt(2024, 1, 1, 0, 0, 0),
            &dt(2024, 12, 31, 0, 0, 0),
            7,
        );
        assert_eq!(occ.len(), 7);
        assert_eq!(occ[6], dt(2024, 1, 7, 0, 0, 0));
    }

    // ---- never infinite loop ----------------------------------------------

    #[test]
    fn forever_rule_capped_by_max_no_hang() {
        // No COUNT, no UNTIL, huge range — must stop at `max`, never hang.
        let seed = dt(2000, 1, 1, 0, 0, 0);
        let rule = RRule::parse("FREQ=DAILY").unwrap();
        let occ = expand(
            &seed,
            &rule,
            &dt(1900, 1, 1, 0, 0, 0),
            &dt(2200, 12, 31, 0, 0, 0),
            50,
        );
        assert_eq!(occ.len(), 50);
    }

    #[test]
    fn forever_rule_capped_by_range_end() {
        // No COUNT/UNTIL, generous max, but a narrow range bounds it.
        let seed = dt(2024, 1, 1, 0, 0, 0);
        let rule = RRule::parse("FREQ=DAILY").unwrap();
        let occ = expand(
            &seed,
            &rule,
            &dt(2024, 1, 1, 0, 0, 0),
            &dt(2024, 1, 10, 0, 0, 0),
            1_000_000,
        );
        assert_eq!(occ.len(), 10); // Jan 1..10 inclusive
    }

    #[test]
    fn interval_zero_does_not_hang() {
        // INTERVAL=0 is coerced to 1 — must terminate (bounded by max), not spin.
        let seed = dt(2024, 1, 1, 0, 0, 0);
        let rule = RRule::parse("FREQ=DAILY;INTERVAL=0;COUNT=3").unwrap();
        let occ = expand(
            &seed,
            &rule,
            &dt(2024, 1, 1, 0, 0, 0),
            &dt(2024, 12, 31, 0, 0, 0),
            100,
        );
        assert_eq!(
            ymd(&occ),
            alloc::vec![(2024, 1, 1), (2024, 1, 2), (2024, 1, 3)]
        );

        // WEEKLY INTERVAL=0 with BYDAY also coerced to 1.
        let rule2 = RRule::parse("FREQ=WEEKLY;INTERVAL=0;BYDAY=MO;COUNT=2").unwrap();
        let occ2 = expand(
            &seed,
            &rule2,
            &dt(2024, 1, 1, 0, 0, 0),
            &dt(2024, 12, 31, 0, 0, 0),
            100,
        );
        assert_eq!(ymd(&occ2), alloc::vec![(2024, 1, 1), (2024, 1, 8)]);
    }

    #[test]
    fn max_zero_returns_empty() {
        let seed = dt(2024, 1, 1, 0, 0, 0);
        let rule = RRule::parse("FREQ=DAILY;COUNT=5").unwrap();
        assert!(expand(
            &seed,
            &rule,
            &dt(2024, 1, 1, 0, 0, 0),
            &dt(2024, 12, 31, 0, 0, 0),
            0
        )
        .is_empty());
    }

    // ---- VEvent::occurrences ----------------------------------------------

    #[test]
    fn vevent_single_event_no_rrule() {
        let mut ev = VEvent::default();
        ev.dtstart = Some(dt(2024, 6, 15, 14, 0, 0));
        // In range -> one occurrence.
        let occ = ev.occurrences(&dt(2024, 6, 1, 0, 0, 0), &dt(2024, 6, 30, 0, 0, 0), 10);
        assert_eq!(ymd(&occ), alloc::vec![(2024, 6, 15)]);
        // Out of range -> none.
        let none = ev.occurrences(&dt(2024, 7, 1, 0, 0, 0), &dt(2024, 7, 31, 0, 0, 0), 10);
        assert!(none.is_empty());
        // No dtstart -> none.
        let empty =
            VEvent::default().occurrences(&dt(2024, 1, 1, 0, 0, 0), &dt(2024, 12, 31, 0, 0, 0), 10);
        assert!(empty.is_empty());
    }

    #[test]
    fn vevent_recurring_via_occurrences() {
        let mut ev = VEvent::default();
        ev.dtstart = Some(dt(2024, 1, 1, 9, 0, 0));
        ev.rrule = RRule::parse("FREQ=WEEKLY;BYDAY=MO,WE,FR;COUNT=6");
        let occ = ev.occurrences(&dt(2024, 1, 1, 0, 0, 0), &dt(2024, 12, 31, 0, 0, 0), 100);
        // Mon 1, Wed 3, Fri 5, Mon 8, Wed 10, Fri 12 (COUNT=6).
        assert_eq!(
            ymd(&occ),
            alloc::vec![
                (2024, 1, 1),
                (2024, 1, 3),
                (2024, 1, 5),
                (2024, 1, 8),
                (2024, 1, 10),
                (2024, 1, 12),
            ]
        );
        assert_eq!((occ[0].hour, occ[0].minute), (9, 0));
    }

    // ---- FAIL-able sentinel ------------------------------------------------

    #[test]
    fn failable_occurrence_string() {
        // A flat string of the WEEKLY+BYDAY expansion — tweak any token -> FAIL.
        let seed = dt(2024, 1, 1, 9, 0, 0);
        let rule = RRule::parse("FREQ=WEEKLY;BYDAY=MO,WE;COUNT=4").unwrap();
        let occ = expand(
            &seed,
            &rule,
            &dt(2024, 1, 1, 0, 0, 0),
            &dt(2024, 12, 31, 0, 0, 0),
            100,
        );
        let s: Vec<alloc::string::String> = occ
            .iter()
            .map(|o| {
                alloc::format!(
                    "{:04}{:02}{:02}T{:02}{:02}",
                    o.year,
                    o.month,
                    o.day,
                    o.hour,
                    o.minute
                )
            })
            .collect();
        assert_eq!(
            s.join(","),
            "20240101T0900,20240103T0900,20240108T0900,20240110T0900".to_string()
        );
    }
}
