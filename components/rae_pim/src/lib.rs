//! # RaePim — a never-panic, `no_std` iCalendar (.ics) + vCard (.vcf) parser.
//!
//! RaeenOS_Concept.md §Compatibility Strategy ("how to actually win" — let people
//! switch without conscious effort): switcher criterion #5 is "import my calendar
//! & contacts from Google / Apple / Outlook." Every one of those platforms exports
//! its calendar as **iCalendar** (`.ics`, RFC 5545) and its contacts as **vCard**
//! (`.vcf`, RFC 6350 v4.0, with Google/Apple still emitting v3.0). Those two text
//! formats ARE the import path; without them RaeenOS cannot honestly claim a
//! frictionless switch. This crate is the from-scratch data layer that the
//! Calendar app, the Contacts app, and the OOBE "bring your stuff over" wizard sit
//! on.
//!
//! ## The shared grammar
//! iCalendar and vCard share one line-format (RFC 5545 §3.1 / RFC 6350 §3.3):
//! - **Content lines** `NAME;PARAM=VALUE;PARAM=VALUE:VALUE` — a property name, zero
//!   or more parameters, then a value after the first unquoted `:`.
//! - **Line folding** — a logical line may be split across physical lines; any
//!   physical line beginning with a SPACE or TAB is a continuation of the prior
//!   one (the leading whitespace is removed on unfold).
//! - **Value escaping** — `\\n`/`\\N` → newline, `\\,` → `,`, `\\;` → `;`,
//!   `\\\\` → `\\`.
//! - **Components** — `BEGIN:NAME` … `END:NAME` brackets, nestable.
//!
//! [`unfold_and_split`] / [`ContentLine`] implement that shared lexer once; the
//! iCal and vCard parsers are thin model-builders over it.
//!
//! ## What it models
//! - iCalendar: [`Calendar`] with [`VEvent`]s and [`VTodo`]s. A [`VEvent`] carries
//!   UID, DTSTART/DTEND (as [`DateTime`], distinguishing a DATE from a DATE-TIME,
//!   capturing the `TZID` param and the UTC `Z` suffix), SUMMARY, DESCRIPTION,
//!   LOCATION, STATUS, ORGANIZER, ATTENDEEs, CATEGORIES, and RRULE (the raw rule
//!   string plus a parsed [`RRule`] for FREQ/INTERVAL/COUNT/UNTIL — deeper `BY*`
//!   parts are captured raw, see [`RRule::raw`]).
//! - vCard: [`AddressBook`] of [`VCard`]s. A [`VCard`] carries FN, structured N
//!   ([`Name`]), typed EMAIL/TEL/ADR, ORG, TITLE, BDAY, URL, NOTE, UID, and PHOTO
//!   (captured as its encoding/URI value, not decoded). Both v3.0 and v4.0 parse.
//!
//! ## Timezone math (the [`tz`] module)
//! A [`DateTime`] captures `tz: Option<String>` (a `TZID`) and `utc: bool`. The
//! [`tz`] module resolves those to real UTC offsets across DST: it parses the
//! **POSIX TZ string** form (IEEE 1003.1 — e.g. `EST5EDT,M3.2.0,M11.1.0`, the
//! inverted-sign offsets, `Mm.w.d`/`Jn`/`n` transition rules, both hemispheres),
//! carries a curated IANA-name → POSIX map (~10 common zones, so a TZID from a real
//! `.ics` resolves), and offers [`tz::to_utc`] / [`tz::to_zone`] to normalise a
//! calendar `DateTime`. The full Olson/tzdata binary + historical rule changes are
//! a documented-deferred later layer; the POSIX-TZ subset covers the vast majority
//! of real zones for any near-future calendar use.
//! - **Deep recurrence.** [`RRule`] parses FREQ, INTERVAL, COUNT, UNTIL, and BYDAY
//!   (the day list, e.g. `MO,WE,FR`); every other `BY*` / WKST / BYSETPOS part is
//!   preserved verbatim in [`RRule::raw`]. The [`recur`] module is the recurrence
//!   **expander** that turns a rule + DTSTART into concrete occurrence start-times
//!   in a date range ([`recur::expand`] / [`VEvent::occurrences`]) — DAILY/WEEKLY/
//!   MONTHLY/YEARLY + INTERVAL + COUNT/UNTIL, WEEKLY+BYDAY, MONTHLY+BYMONTHDAY and
//!   ordinal BYDAY (`1MO`/`-1FR`); BYSETPOS/BYWEEKNO/EXDATE and multi-BY* combos
//!   are documented-deferred there.
//! - **PHOTO/embedded binary decode.** The base64/URI value is captured raw with
//!   its `ENCODING`/`VALUE` param; a higher layer decodes on demand.
//!
//! ## Hostile-input posture (CLAUDE: every byte is attacker-controlled)
//! A `.ics`/`.vcf` arrives from an untrusted export, a shared link, or a malicious
//! peer. There is **no `unwrap`/`expect`/`panic`/raw-index-panic / infinite-loop**
//! path reachable from [`parse_ics`] / [`parse_vcf`]: physical line count
//! ([`MAX_LINES`]), single logical line length ([`MAX_LINE_LEN`]), fold
//! continuations per line ([`MAX_FOLD`]), component nesting depth ([`MAX_DEPTH`]),
//! and properties/items per object ([`MAX_PROPS`]) are all bounded before
//! allocation. Unbalanced `BEGIN`/`END` is handled best-effort; non-iCal /
//! non-vCard input is [`PimError::NotRecognized`].
//!
//! The host KAT suite at the bottom of this file is the primary proof
//! (`cargo test -p rae_pim`): a 2-VEVENT `.ics` (exact UID/SUMMARY/DTSTART
//! date-time fields/LOCATION, a parsed `FREQ=WEEKLY;INTERVAL=2;COUNT=10` RRULE, a
//! folded multi-line DESCRIPTION, escaped `,`/`;`/newline decode, DATE-only vs
//! DATE-TIME, TZID capture), a 2-contact `.vcf` in BOTH v3.0 and v4.0 (FN, the
//! structured N, multiple typed EMAILs, TEL, ORG, BDAY), and a hostile battery
//! (unbalanced/truncated/garbage, a 100k-fold pathological line, deep nesting, a
//! seeded fuzz loop) that must all be bounded with zero panics.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

use alloc::string::{String, ToString};
use alloc::vec::Vec;

pub mod recur;
pub use recur::{
    add_days, add_months, civil_from_days, days_from_civil, days_in_month, expand, is_leap,
    weekday_from_days, MAX_STEPS,
};

pub mod tz;
pub use tz::{parse_tz, to_utc, to_zone, tz_for_iana, tzinfo_for_iana, TransRule, TzError, TzInfo};

// ---------------------------------------------------------------------------
// Caps (untrusted input). Every loop and allocation is bounded by one of these.
// ---------------------------------------------------------------------------

/// Maximum number of physical lines we will scan in one document.
pub const MAX_LINES: usize = 1_000_000;
/// Maximum length (bytes) of a single *logical* (unfolded) line.
pub const MAX_LINE_LEN: usize = 1_048_576; // 1 MiB
/// Maximum fold continuations stitched onto a single logical line.
pub const MAX_FOLD: usize = 65_536;
/// Maximum BEGIN/END component nesting depth.
pub const MAX_DEPTH: usize = 64;
/// Maximum properties per object, components per calendar, contacts per book.
pub const MAX_PROPS: usize = 1_000_000;

/// Why a PIM parse failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PimError {
    /// The input is not an iCalendar / vCard document (no `BEGIN:VCALENDAR` /
    /// `BEGIN:VCARD` was found), or it is empty.
    NotRecognized,
    /// A structural cap was exceeded: too many lines, a line over [`MAX_LINE_LEN`],
    /// too many fold continuations, nesting past [`MAX_DEPTH`], or more than
    /// [`MAX_PROPS`] properties/items — a hostile or malformed document.
    LimitExceeded,
}

// ===========================================================================
// Shared content-line lexer (RFC 5545 §3.1 / RFC 6350 §3.3)
// ===========================================================================

/// One parsed content line: `NAME;PARAM=VALUE;...:VALUE`.
///
/// The value is returned *raw* (escaping NOT yet decoded) so a caller can choose
/// per-property whether to decode (most text properties) or keep verbatim (e.g. a
/// PHOTO data URI). Use [`decode_text`] to apply RFC value-escaping.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ContentLine {
    /// Property name, upper-cased (e.g. `SUMMARY`, `DTSTART`). Group prefixes
    /// (`item1.EMAIL`) are stripped to the bare name.
    pub name: String,
    /// Parameters as `(KEY_UPPER, VALUE)` pairs, in document order. A bare param
    /// (`PREF`, no `=`) is stored with an empty value.
    pub params: Vec<(String, String)>,
    /// The raw value (everything after the first unquoted `:`), escaping intact.
    pub value: String,
}

impl ContentLine {
    /// First value of a parameter (case-insensitive key), if present.
    pub fn param(&self, key: &str) -> Option<&str> {
        self.params
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v.as_str())
    }

    /// The `TYPE=` parameter split into individual upper-cased type tokens
    /// (`TYPE=WORK,VOICE` → `["WORK","VOICE"]`). vCard 4.0 allows a comma list;
    /// 3.0 often repeats the param — both `param("TYPE")` here and any standalone
    /// `TYPE` params are unioned by the typed accessors.
    pub fn types(&self) -> Vec<String> {
        let mut out = Vec::new();
        for (k, v) in &self.params {
            if k.eq_ignore_ascii_case("TYPE") {
                for tok in v.split(',') {
                    let t = tok.trim();
                    if !t.is_empty() {
                        out.push(ascii_upper(t));
                    }
                }
            }
        }
        out
    }

    /// The decoded (unescaped) value as text.
    pub fn text(&self) -> String {
        decode_text(&self.value)
    }
}

/// Unfold and split a whole document into logical [`ContentLine`]s.
///
/// Handles CRLF and bare LF. A physical line beginning with SPACE or TAB is folded
/// onto the previous logical line (its leading whitespace removed). Bounded by
/// [`MAX_LINES`], [`MAX_LINE_LEN`], [`MAX_FOLD`]; on a cap breach returns
/// `Err(LimitExceeded)`. Never panics, never loops unboundedly.
pub fn unfold_and_split(input: &str) -> Result<Vec<ContentLine>, PimError> {
    // First pass: unfold into logical line strings.
    let mut logical: Vec<String> = Vec::new();
    let mut current: Option<String> = None;
    let mut fold_count: usize = 0;
    let mut line_count: usize = 0;

    for raw_line in input.split('\n') {
        line_count += 1;
        if line_count > MAX_LINES {
            return Err(PimError::LimitExceeded);
        }
        // Strip a trailing '\r' (CRLF) — split('\n') leaves it attached.
        let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);

        let is_continuation = line.starts_with(' ') || line.starts_with('\t');

        if is_continuation {
            match current.as_mut() {
                Some(cur) => {
                    fold_count += 1;
                    if fold_count > MAX_FOLD {
                        return Err(PimError::LimitExceeded);
                    }
                    // RFC: drop exactly the single leading folding whitespace char.
                    let cont = &line[1..];
                    if cur.len().saturating_add(cont.len()) > MAX_LINE_LEN {
                        return Err(PimError::LimitExceeded);
                    }
                    cur.push_str(cont);
                }
                None => {
                    // A continuation with nothing to continue: start a line.
                    fold_count = 0;
                    current = Some(line.trim_start().to_string());
                }
            }
        } else {
            // New logical line: flush the previous one.
            if let Some(cur) = current.take() {
                push_capped(&mut logical, cur)?;
            }
            fold_count = 0;
            if line.len() > MAX_LINE_LEN {
                return Err(PimError::LimitExceeded);
            }
            current = Some(line.to_string());
        }
    }
    if let Some(cur) = current.take() {
        push_capped(&mut logical, cur)?;
    }

    // Second pass: parse each non-empty logical line into a ContentLine.
    let mut out: Vec<ContentLine> = Vec::new();
    for ll in logical {
        if ll.trim().is_empty() {
            continue;
        }
        if out.len() >= MAX_PROPS {
            return Err(PimError::LimitExceeded);
        }
        if let Some(cl) = parse_content_line(&ll) {
            out.push(cl);
        }
    }
    Ok(out)
}

fn push_capped(v: &mut Vec<String>, s: String) -> Result<(), PimError> {
    if v.len() >= MAX_PROPS {
        return Err(PimError::LimitExceeded);
    }
    v.push(s);
    Ok(())
}

/// Parse one already-unfolded logical line into a [`ContentLine`]. Returns `None`
/// for a line with no name (defensive; never panics).
fn parse_content_line(line: &str) -> Option<ContentLine> {
    // The value begins after the first ':' that is NOT inside a quoted param
    // value. Walk bytes tracking quote state.
    let bytes = line.as_bytes();
    let mut in_quotes = false;
    let mut colon: Option<usize> = None;
    for (i, &b) in bytes.iter().enumerate() {
        match b {
            b'"' => in_quotes = !in_quotes,
            b':' if !in_quotes => {
                colon = Some(i);
                break;
            }
            _ => {}
        }
    }
    let (head, value) = match colon {
        Some(c) => (&line[..c], line[c + 1..].to_string()),
        None => (line, String::new()),
    };

    // head = NAME;PARAM=VALUE;PARAM=VALUE  (params split on unquoted ';')
    let segments = split_unquoted(head, ';');
    let mut iter = segments.into_iter();
    let name_seg = iter.next().unwrap_or_default();
    // Strip an optional group prefix "group.NAME".
    let bare = match name_seg.rfind('.') {
        Some(dot) => &name_seg[dot + 1..],
        None => &name_seg,
    };
    let name = ascii_upper(bare.trim());
    if name.is_empty() {
        return None;
    }

    let mut params: Vec<(String, String)> = Vec::new();
    for seg in iter {
        if params.len() >= 1024 {
            break; // per-line param cap; bounded, defensive
        }
        if let Some(eq) = seg.find('=') {
            let k = ascii_upper(seg[..eq].trim());
            let mut v = seg[eq + 1..].trim().to_string();
            // Unquote a quoted param value.
            if v.len() >= 2 && v.starts_with('"') && v.ends_with('"') {
                v = v[1..v.len() - 1].to_string();
            }
            params.push((k, v));
        } else {
            let k = ascii_upper(seg.trim());
            if !k.is_empty() {
                params.push((k, String::new()));
            }
        }
    }

    Some(ContentLine {
        name,
        params,
        value,
    })
}

/// Split on `sep`, but never inside a double-quoted run.
fn split_unquoted(s: &str, sep: char) -> Vec<String> {
    let mut out = Vec::new();
    let mut start = 0usize;
    let mut in_quotes = false;
    let sep_b = sep as u8;
    let bytes = s.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        if b == b'"' {
            in_quotes = !in_quotes;
        } else if b == sep_b && !in_quotes {
            out.push(s[start..i].to_string());
            start = i + 1;
        }
    }
    out.push(s[start..].to_string());
    out
}

/// Decode RFC value escaping in a single (already-split) text value:
/// `\\n`/`\\N` → newline, `\\,` → `,`, `\\;` → `;`, `\\\\` → `\\`. Any other
/// backslash escape passes the following char through literally. Never panics.
pub fn decode_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') | Some('N') => out.push('\n'),
                Some(',') => out.push(','),
                Some(';') => out.push(';'),
                Some('\\') => out.push('\\'),
                Some(other) => out.push(other),
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Split a *structured* value (e.g. vCard `N` / `ADR`, iCal multi-value) on
/// unescaped `;`, decoding each field's escaping. Backslash-escaped `;` does NOT
/// split. Never panics.
pub fn split_structured(s: &str) -> Vec<String> {
    let mut fields: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            // Keep the escape sequence intact for decode_text on the field.
            cur.push('\\');
            if let Some(&n) = chars.peek() {
                cur.push(n);
                chars.next();
            }
        } else if c == ';' {
            fields.push(decode_text(&cur));
            cur.clear();
        } else {
            cur.push(c);
        }
    }
    fields.push(decode_text(&cur));
    fields
}

/// Split a comma list (e.g. CATEGORIES) on unescaped `,`, decoding each. Never
/// panics.
pub fn split_comma(s: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            cur.push('\\');
            if let Some(&n) = chars.peek() {
                cur.push(n);
                chars.next();
            }
        } else if c == ',' {
            out.push(decode_text(&cur));
            cur.clear();
        } else {
            cur.push(c);
        }
    }
    out.push(decode_text(&cur));
    out
}

fn ascii_upper(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        out.push(b.to_ascii_uppercase() as char);
    }
    out
}

// ===========================================================================
// Date / time model
// ===========================================================================

/// A captured iCalendar date or date-time. NO timezone math is performed — see the
/// crate docs. `is_date` distinguishes a `VALUE=DATE` (`20260615`) from a
/// date-time (`20260615T093000`); `utc` is set when the value ended in `Z`;
/// `tz` holds the `TZID` param if any.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DateTime {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
    /// True for a date-only value (no time component).
    pub is_date: bool,
    /// True when the value carried a trailing `Z` (UTC).
    pub utc: bool,
    /// `TZID` parameter, if the property carried one.
    pub tz: Option<String>,
    /// The original raw value as it appeared (for round-trip / debugging).
    pub raw: String,
}

impl DateTime {
    /// Parse an iCalendar DATE / DATE-TIME value. `tzid` is the property's `TZID`
    /// param (if any); `value_is_date` is true when the property carried
    /// `VALUE=DATE`. Lenient: a malformed value yields a [`DateTime`] with whatever
    /// fields parsed plus the raw string — never panics, never errors out the
    /// surrounding object.
    pub fn parse(value: &str, tzid: Option<&str>, value_is_date: bool) -> DateTime {
        let raw = value.to_string();
        let mut dt = DateTime {
            raw: raw.clone(),
            tz: tzid.map(|s| s.to_string()),
            ..Default::default()
        };
        let v = value.trim();
        let utc = v.ends_with('Z');
        let core = v.strip_suffix('Z').unwrap_or(v);
        dt.utc = utc;

        // Split the date part from an optional `T<time>`.
        let (date_part, time_part) = match core.find('T') {
            Some(t) => (&core[..t], Some(&core[t + 1..])),
            None => (core, None),
        };

        // Date: YYYYMMDD. Work on bytes so a non-ASCII (lossy U+FFFD) value can
        // never produce a non-char-boundary slice panic.
        let db = date_part.as_bytes();
        if db.len() == 8 && db.iter().all(|b| b.is_ascii_digit()) {
            dt.year = two4(db, 0);
            dt.month = two2(db, 4);
            dt.day = two2(db, 6);
        }

        match time_part {
            Some(tp) if tp.len() >= 6 && tp.as_bytes()[..6].iter().all(|b| b.is_ascii_digit()) => {
                let b = tp.as_bytes();
                dt.hour = two2(b, 0);
                dt.minute = two2(b, 2);
                dt.second = two2(b, 4);
                dt.is_date = false;
            }
            _ => {
                // No time component → DATE (unless caller forced otherwise).
                dt.is_date = value_is_date || time_part.is_none();
            }
        }
        if value_is_date {
            dt.is_date = true;
        }
        dt
    }
}

/// Parse 2 ASCII digits at byte offset `o` into a u8 (caller has verified the
/// bytes are ASCII digits). Never panics.
fn two2(b: &[u8], o: usize) -> u8 {
    let hi = b.get(o).map(|x| x.wrapping_sub(b'0')).unwrap_or(0);
    let lo = b.get(o + 1).map(|x| x.wrapping_sub(b'0')).unwrap_or(0);
    hi.wrapping_mul(10).wrapping_add(lo)
}

/// Parse 4 ASCII digits at byte offset `o` into a u16. Never panics.
fn two4(b: &[u8], o: usize) -> u16 {
    let mut acc: u16 = 0;
    for k in 0..4 {
        let d = b
            .get(o + k)
            .map(|x| x.wrapping_sub(b'0') as u16)
            .unwrap_or(0);
        acc = acc.wrapping_mul(10).wrapping_add(d);
    }
    acc
}

// ===========================================================================
// iCalendar model
// ===========================================================================

/// A parsed iCalendar recurrence frequency.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Freq {
    Secondly,
    Minutely,
    Hourly,
    Daily,
    Weekly,
    Monthly,
    Yearly,
    /// A FREQ token we don't model (preserved verbatim).
    Other(String),
}

/// A parsed RRULE. FREQ/INTERVAL/COUNT/UNTIL/BYDAY are modeled; every other part
/// (BYMONTHDAY, BYSETPOS, WKST, …) is preserved in [`RRule::raw`] for a deeper
/// recurrence expander — see the crate docs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RRule {
    pub freq: Freq,
    /// INTERVAL (defaults to 1 per RFC 5545 when absent).
    pub interval: u32,
    /// COUNT, if present (mutually exclusive with UNTIL in valid input).
    pub count: Option<u32>,
    /// UNTIL, parsed as a [`DateTime`], if present.
    pub until: Option<DateTime>,
    /// BYDAY tokens (e.g. `["MO","WE","FR"]` or `["-1SU"]`), if present.
    pub byday: Vec<String>,
    /// The original RRULE value string, in full.
    pub raw: String,
}

impl RRule {
    /// Parse an RRULE value (`FREQ=WEEKLY;INTERVAL=2;COUNT=10;BYDAY=MO,WE`).
    /// Lenient and bounded; unknown parts are ignored (still kept in `raw`).
    /// Never panics. Returns `None` only if no FREQ is present.
    pub fn parse(value: &str) -> Option<RRule> {
        let raw = value.to_string();
        let mut freq: Option<Freq> = None;
        let mut interval: u32 = 1;
        let mut count: Option<u32> = None;
        let mut until: Option<DateTime> = None;
        let mut byday: Vec<String> = Vec::new();

        for part in value.split(';') {
            let (k, v) = match part.split_once('=') {
                Some((k, v)) => (k.trim(), v.trim()),
                None => continue,
            };
            match ascii_upper(k).as_str() {
                "FREQ" => {
                    freq = Some(match ascii_upper(v).as_str() {
                        "SECONDLY" => Freq::Secondly,
                        "MINUTELY" => Freq::Minutely,
                        "HOURLY" => Freq::Hourly,
                        "DAILY" => Freq::Daily,
                        "WEEKLY" => Freq::Weekly,
                        "MONTHLY" => Freq::Monthly,
                        "YEARLY" => Freq::Yearly,
                        other => Freq::Other(other.to_string()),
                    });
                }
                "INTERVAL" => {
                    if let Ok(n) = v.parse::<u32>() {
                        interval = n;
                    }
                }
                "COUNT" => {
                    if let Ok(n) = v.parse::<u32>() {
                        count = Some(n);
                    }
                }
                "UNTIL" => {
                    until = Some(DateTime::parse(v, None, false));
                }
                "BYDAY" => {
                    for tok in v.split(',') {
                        let t = tok.trim();
                        if !t.is_empty() && byday.len() < 512 {
                            byday.push(ascii_upper(t));
                        }
                    }
                }
                _ => { /* captured raw */ }
            }
        }

        freq.map(|freq| RRule {
            freq,
            interval,
            count,
            until,
            byday,
            raw,
        })
    }
}

/// A calendar event (`VEVENT`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct VEvent {
    pub uid: String,
    pub dtstart: Option<DateTime>,
    pub dtend: Option<DateTime>,
    pub summary: String,
    pub description: String,
    pub location: String,
    pub status: String,
    pub organizer: String,
    pub attendees: Vec<String>,
    pub categories: Vec<String>,
    /// Parsed recurrence rule, if an RRULE property was present.
    pub rrule: Option<RRule>,
}

/// A to-do (`VTODO`). A minimal subset (calendars import these too).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct VTodo {
    pub uid: String,
    pub summary: String,
    pub description: String,
    pub due: Option<DateTime>,
    pub status: String,
}

/// A parsed iCalendar object (`VCALENDAR`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Calendar {
    pub prodid: String,
    pub version: String,
    pub events: Vec<VEvent>,
    pub todos: Vec<VTodo>,
}

/// Parse an iCalendar (`.ics`) document. Accepts `&str` or `&[u8]` (UTF-8, lossy on
/// invalid bytes) via the `AsRef`/`From` ergonomics below — pass `&str`. Tolerates
/// CRLF and LF, skips unknown properties and unmodeled components (VTIMEZONE,
/// VALARM, VJOURNAL, X- extensions) gracefully, and is bounded against hostile
/// input. Returns [`PimError::NotRecognized`] if no `BEGIN:VCALENDAR` is found.
pub fn parse_ics(input: &str) -> Result<Calendar, PimError> {
    let lines = unfold_and_split(input)?;
    if !lines
        .iter()
        .any(|l| l.name == "BEGIN" && l.value.trim().eq_ignore_ascii_case("VCALENDAR"))
    {
        return Err(PimError::NotRecognized);
    }

    let mut cal = Calendar::default();
    // A stack of component names we are inside (bounded by MAX_DEPTH).
    let mut stack: Vec<String> = Vec::new();
    // Accumulators for the component currently being built.
    let mut cur_event: Option<VEvent> = None;
    let mut cur_todo: Option<VTodo> = None;

    for line in &lines {
        if line.name == "BEGIN" {
            let comp = ascii_upper(line.value.trim());
            if stack.len() >= MAX_DEPTH {
                return Err(PimError::LimitExceeded);
            }
            stack.push(comp.clone());
            match comp.as_str() {
                "VEVENT" => cur_event = Some(VEvent::default()),
                "VTODO" => cur_todo = Some(VTodo::default()),
                _ => {}
            }
            continue;
        }
        if line.name == "END" {
            let comp = ascii_upper(line.value.trim());
            // Pop best-effort: tolerate a mismatched/unbalanced END.
            if stack.last().map(|s| s.as_str()) == Some(comp.as_str()) {
                stack.pop();
            } else if let Some(pos) = stack.iter().rposition(|s| *s == comp) {
                stack.truncate(pos);
            }
            match comp.as_str() {
                "VEVENT" => {
                    if let Some(ev) = cur_event.take() {
                        if cal.events.len() >= MAX_PROPS {
                            return Err(PimError::LimitExceeded);
                        }
                        cal.events.push(ev);
                    }
                }
                "VTODO" => {
                    if let Some(td) = cur_todo.take() {
                        if cal.todos.len() >= MAX_PROPS {
                            return Err(PimError::LimitExceeded);
                        }
                        cal.todos.push(td);
                    }
                }
                _ => {}
            }
            continue;
        }

        // A property line. Route it by the innermost interesting component.
        let inside = stack.last().map(|s| s.as_str()).unwrap_or("");
        match inside {
            "VEVENT" => {
                if let Some(ev) = cur_event.as_mut() {
                    apply_event_prop(ev, line);
                }
            }
            "VTODO" => {
                if let Some(td) = cur_todo.as_mut() {
                    apply_todo_prop(td, line);
                }
            }
            "VCALENDAR" => match line.name.as_str() {
                "PRODID" => cal.prodid = line.text(),
                "VERSION" => cal.version = line.text(),
                _ => {}
            },
            // Inside VALARM / VTIMEZONE / unknown: skip gracefully.
            _ => {}
        }
    }

    Ok(cal)
}

fn value_is_date_param(line: &ContentLine) -> bool {
    line.param("VALUE")
        .map(|v| v.eq_ignore_ascii_case("DATE"))
        .unwrap_or(false)
}

fn apply_event_prop(ev: &mut VEvent, line: &ContentLine) {
    match line.name.as_str() {
        "UID" => ev.uid = line.text(),
        "SUMMARY" => ev.summary = line.text(),
        "DESCRIPTION" => ev.description = line.text(),
        "LOCATION" => ev.location = line.text(),
        "STATUS" => ev.status = line.text(),
        "ORGANIZER" => ev.organizer = line.value.clone(),
        "ATTENDEE" => {
            if ev.attendees.len() < MAX_PROPS {
                ev.attendees.push(line.value.clone());
            }
        }
        "CATEGORIES" => ev.categories = split_comma(&line.value),
        "DTSTART" => {
            ev.dtstart = Some(DateTime::parse(
                &line.value,
                line.param("TZID"),
                value_is_date_param(line),
            ))
        }
        "DTEND" => {
            ev.dtend = Some(DateTime::parse(
                &line.value,
                line.param("TZID"),
                value_is_date_param(line),
            ))
        }
        "RRULE" => ev.rrule = RRule::parse(&line.value),
        _ => {}
    }
}

fn apply_todo_prop(td: &mut VTodo, line: &ContentLine) {
    match line.name.as_str() {
        "UID" => td.uid = line.text(),
        "SUMMARY" => td.summary = line.text(),
        "DESCRIPTION" => td.description = line.text(),
        "STATUS" => td.status = line.text(),
        "DUE" => {
            td.due = Some(DateTime::parse(
                &line.value,
                line.param("TZID"),
                value_is_date_param(line),
            ))
        }
        _ => {}
    }
}

/// Convenience: parse iCalendar from raw bytes (UTF-8, lossy).
pub fn parse_ics_bytes(input: &[u8]) -> Result<Calendar, PimError> {
    parse_ics(&lossy_utf8(input))
}

// ===========================================================================
// vCard model
// ===========================================================================

/// A structured name (vCard `N`): `family;given;additional;prefixes;suffixes`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Name {
    pub family: String,
    pub given: String,
    pub additional: String,
    pub prefix: String,
    pub suffix: String,
}

/// A typed value (EMAIL / TEL / URL): the value plus its `TYPE` tokens.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Typed {
    pub value: String,
    /// Upper-cased type tokens (`HOME`, `WORK`, `VOICE`, `CELL`, …).
    pub types: Vec<String>,
}

impl Typed {
    /// True if this value carries the given type (case-insensitive).
    pub fn has_type(&self, t: &str) -> bool {
        self.types.iter().any(|x| x.eq_ignore_ascii_case(t))
    }
}

/// A structured postal address (vCard `ADR`) plus its TYPE tokens. Fields per RFC
/// 6350 §6.3.1: po-box;ext;street;locality;region;postal-code;country.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Address {
    pub po_box: String,
    pub extended: String,
    pub street: String,
    pub locality: String,
    pub region: String,
    pub postal_code: String,
    pub country: String,
    pub types: Vec<String>,
}

/// A captured PHOTO (or other embedded binary): its raw value plus the encoding /
/// media-type params. NOT decoded — a higher layer decodes base64 / fetches a URI
/// on demand (see the crate docs).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Photo {
    /// The raw value: a data URI, an http(s) URL, or base64 payload.
    pub value: String,
    /// `ENCODING` param (e.g. `b` / `BASE64`) if present.
    pub encoding: String,
    /// `TYPE`/`MEDIATYPE` (e.g. `JPEG`, `image/png`) if present.
    pub media_type: String,
}

/// A parsed contact (`VCARD`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct VCard {
    /// vCard `VERSION` (`3.0` / `4.0`).
    pub version: String,
    /// Formatted name (`FN`).
    pub fn_name: String,
    /// Structured name (`N`).
    pub name: Name,
    pub emails: Vec<Typed>,
    pub phones: Vec<Typed>,
    pub addresses: Vec<Address>,
    pub urls: Vec<Typed>,
    pub org: String,
    pub title: String,
    pub bday: String,
    pub note: String,
    pub uid: String,
    pub photo: Option<Photo>,
}

impl VCard {
    /// All email addresses (values only).
    pub fn email_values(&self) -> Vec<&str> {
        self.emails.iter().map(|e| e.value.as_str()).collect()
    }
    /// All phone numbers (values only).
    pub fn phone_values(&self) -> Vec<&str> {
        self.phones.iter().map(|p| p.value.as_str()).collect()
    }
    /// The first email matching `t` (e.g. `"WORK"`), if any.
    pub fn email_of_type(&self, t: &str) -> Option<&Typed> {
        self.emails.iter().find(|e| e.has_type(t))
    }
}

/// A parsed collection of contacts (a `.vcf` may hold many concatenated vCards).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AddressBook {
    pub contacts: Vec<VCard>,
}

/// Parse a vCard (`.vcf`) document — one or many concatenated `BEGIN:VCARD` …
/// `END:VCARD` cards, vCard 3.0 or 4.0. Tolerates CRLF/LF, skips unknown / X-
/// properties, and is bounded against hostile input. Returns
/// [`PimError::NotRecognized`] if no `BEGIN:VCARD` is found.
pub fn parse_vcf(input: &str) -> Result<AddressBook, PimError> {
    let lines = unfold_and_split(input)?;
    if !lines
        .iter()
        .any(|l| l.name == "BEGIN" && l.value.trim().eq_ignore_ascii_case("VCARD"))
    {
        return Err(PimError::NotRecognized);
    }

    let mut book = AddressBook::default();
    let mut cur: Option<VCard> = None;
    let mut depth: usize = 0;

    for line in &lines {
        if line.name == "BEGIN" && line.value.trim().eq_ignore_ascii_case("VCARD") {
            depth += 1;
            if depth > MAX_DEPTH {
                return Err(PimError::LimitExceeded);
            }
            // Nested/duplicate BEGIN without END: flush the prior best-effort.
            if let Some(c) = cur.take() {
                push_card(&mut book, c)?;
            }
            cur = Some(VCard::default());
            continue;
        }
        if line.name == "END" && line.value.trim().eq_ignore_ascii_case("VCARD") {
            depth = depth.saturating_sub(1);
            if let Some(c) = cur.take() {
                push_card(&mut book, c)?;
            }
            continue;
        }
        if let Some(c) = cur.as_mut() {
            apply_vcard_prop(c, line);
        }
    }
    // Unbalanced trailing card (no END): keep best-effort.
    if let Some(c) = cur.take() {
        push_card(&mut book, c)?;
    }

    Ok(book)
}

fn push_card(book: &mut AddressBook, c: VCard) -> Result<(), PimError> {
    if book.contacts.len() >= MAX_PROPS {
        return Err(PimError::LimitExceeded);
    }
    book.contacts.push(c);
    Ok(())
}

fn apply_vcard_prop(c: &mut VCard, line: &ContentLine) {
    match line.name.as_str() {
        "VERSION" => c.version = line.value.trim().to_string(),
        "FN" => c.fn_name = line.text(),
        "N" => {
            let f = split_structured(&line.value);
            c.name = Name {
                family: f.first().cloned().unwrap_or_default(),
                given: f.get(1).cloned().unwrap_or_default(),
                additional: f.get(2).cloned().unwrap_or_default(),
                prefix: f.get(3).cloned().unwrap_or_default(),
                suffix: f.get(4).cloned().unwrap_or_default(),
            };
        }
        "EMAIL" => {
            if c.emails.len() < MAX_PROPS {
                c.emails.push(Typed {
                    value: line.text(),
                    types: line.types(),
                });
            }
        }
        "TEL" => {
            if c.phones.len() < MAX_PROPS {
                c.phones.push(Typed {
                    value: line.text(),
                    types: line.types(),
                });
            }
        }
        "URL" => {
            if c.urls.len() < MAX_PROPS {
                c.urls.push(Typed {
                    value: line.value.clone(),
                    types: line.types(),
                });
            }
        }
        "ADR" => {
            if c.addresses.len() < MAX_PROPS {
                let f = split_structured(&line.value);
                c.addresses.push(Address {
                    po_box: f.first().cloned().unwrap_or_default(),
                    extended: f.get(1).cloned().unwrap_or_default(),
                    street: f.get(2).cloned().unwrap_or_default(),
                    locality: f.get(3).cloned().unwrap_or_default(),
                    region: f.get(4).cloned().unwrap_or_default(),
                    postal_code: f.get(5).cloned().unwrap_or_default(),
                    country: f.get(6).cloned().unwrap_or_default(),
                    types: line.types(),
                });
            }
        }
        "ORG" => {
            // ORG is structured (Org;Unit;…); join with " / " for the flat field.
            let parts = split_structured(&line.value);
            c.org = parts.join(" / ");
        }
        "TITLE" => c.title = line.text(),
        "BDAY" => c.bday = line.value.trim().to_string(),
        "NOTE" => c.note = line.text(),
        "UID" => c.uid = line.value.trim().to_string(),
        "PHOTO" => {
            c.photo = Some(Photo {
                value: line.value.clone(),
                encoding: line
                    .param("ENCODING")
                    .map(|s| s.to_string())
                    .unwrap_or_default(),
                // 4.0 uses MEDIATYPE (e.g. image/png); 3.0 uses TYPE (e.g. JPEG).
                media_type: line
                    .param("MEDIATYPE")
                    .or_else(|| line.param("TYPE"))
                    .map(|s| s.to_string())
                    .unwrap_or_default(),
            });
        }
        _ => { /* skip unknown / X- gracefully */ }
    }
}

/// Convenience: parse vCard from raw bytes (UTF-8, lossy).
pub fn parse_vcf_bytes(input: &[u8]) -> Result<AddressBook, PimError> {
    parse_vcf(&lossy_utf8(input))
}

/// Lossy UTF-8 decode (no_std-friendly): replaces invalid sequences with U+FFFD.
/// Bounded by input length; never panics.
fn lossy_utf8(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match core::str::from_utf8(&bytes[i..]) {
            Ok(valid) => {
                out.push_str(valid);
                break;
            }
            Err(e) => {
                let good = e.valid_up_to();
                if good > 0 {
                    // SAFETY-free: from_utf8 on the validated prefix.
                    if let Ok(s) = core::str::from_utf8(&bytes[i..i + good]) {
                        out.push_str(s);
                    }
                }
                out.push('\u{FFFD}');
                i += good + 1;
            }
        }
    }
    out
}

// ===========================================================================
// Host KAT suite — the FAIL-able proof (cargo test -p rae_pim)
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ---- iCalendar ---------------------------------------------------------

    const ICS_TWO_EVENTS: &str = "\
BEGIN:VCALENDAR\r
VERSION:2.0\r
PRODID:-//RaeenOS//rae_pim//EN\r
BEGIN:VEVENT\r
UID:event-001@raeen.os\r
DTSTART;TZID=America/New_York:20260615T093000\r
DTEND;TZID=America/New_York:20260615T103000\r
SUMMARY:Sprint planning\r
LOCATION:Room 4\\, Building B\r
DESCRIPTION:Bring laptops\\; agenda:\\n 1. demo\\n 2. retro\r
RRULE:FREQ=WEEKLY;INTERVAL=2;COUNT=10;BYDAY=MO,WE\r
STATUS:CONFIRMED\r
ORGANIZER:mailto:lead@raeen.os\r
ATTENDEE:mailto:a@raeen.os\r
ATTENDEE:mailto:b@raeen.os\r
CATEGORIES:WORK,PLANNING\r
END:VEVENT\r
BEGIN:VEVENT\r
UID:event-002@raeen.os\r
DTSTART;VALUE=DATE:20261225\r
SUMMARY:Holiday\r
END:VEVENT\r
END:VCALENDAR\r
";

    #[test]
    fn ics_two_events_exact_fields() {
        let cal = parse_ics(ICS_TWO_EVENTS).expect("parse");
        assert_eq!(cal.version, "2.0");
        assert_eq!(cal.prodid, "-//RaeenOS//rae_pim//EN");
        assert_eq!(cal.events.len(), 2);

        let e0 = &cal.events[0];
        assert_eq!(e0.uid, "event-001@raeen.os");
        assert_eq!(e0.summary, "Sprint planning");
        assert_eq!(e0.status, "CONFIRMED");
        assert_eq!(e0.organizer, "mailto:lead@raeen.os");
        assert_eq!(e0.attendees.len(), 2);
        assert_eq!(e0.attendees[1], "mailto:b@raeen.os");
        assert_eq!(e0.categories, alloc::vec!["WORK", "PLANNING"]);

        // DTSTART: exact parsed date-time fields + TZID capture, NOT date-only.
        let ds = e0.dtstart.as_ref().expect("dtstart");
        assert_eq!(
            (ds.year, ds.month, ds.day, ds.hour, ds.minute, ds.second),
            (2026, 6, 15, 9, 30, 0)
        );
        assert!(!ds.is_date);
        assert!(!ds.utc);
        assert_eq!(ds.tz.as_deref(), Some("America/New_York"));

        // Escape decode: "Room 4, Building B" (escaped comma).
        assert_eq!(e0.location, "Room 4, Building B");
    }

    #[test]
    fn ics_folded_description_unfolds_and_decodes_escapes() {
        let cal = parse_ics(ICS_TWO_EVENTS).unwrap();
        let e0 = &cal.events[0];
        // \\; → ';' and \\n → newline (twice). The literal spaces after each
        // escaped newline are part of the value and preserved.
        assert_eq!(
            e0.description,
            "Bring laptops; agenda:\n 1. demo\n 2. retro"
        );
    }

    #[test]
    fn ics_long_description_folded_across_physical_lines() {
        // A genuinely folded DESCRIPTION: one logical value split across three
        // physical lines (RFC §3.1). Exactly one leading fold char is removed per
        // continuation, so the broken word rejoins seamlessly.
        let ics = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:f1\r\nDESCRIPTION:This is a very long descrip\r\n tion that the exporter fol\r\n ded across lines.\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        let cal = parse_ics(ics).unwrap();
        assert_eq!(
            cal.events[0].description,
            "This is a very long description that the exporter folded across lines."
        );
    }

    #[test]
    fn ics_rrule_parsed_to_struct() {
        let cal = parse_ics(ICS_TWO_EVENTS).unwrap();
        let r = cal.events[0].rrule.as_ref().expect("rrule");
        assert_eq!(r.freq, Freq::Weekly);
        assert_eq!(r.interval, 2);
        assert_eq!(r.count, Some(10));
        assert_eq!(r.until, None);
        assert_eq!(r.byday, alloc::vec!["MO", "WE"]);
        assert!(r.raw.contains("FREQ=WEEKLY"));
    }

    #[test]
    fn ics_date_only_vs_datetime_distinguished() {
        let cal = parse_ics(ICS_TWO_EVENTS).unwrap();
        let e1 = &cal.events[1];
        let ds = e1.dtstart.as_ref().unwrap();
        assert!(ds.is_date, "VALUE=DATE must be flagged date-only");
        assert_eq!((ds.year, ds.month, ds.day), (2026, 12, 25));
        assert_eq!((ds.hour, ds.minute, ds.second), (0, 0, 0));
    }

    #[test]
    fn ics_utc_z_suffix_captured() {
        let ics = "BEGIN:VCALENDAR\nBEGIN:VEVENT\nUID:z\nDTSTART:20260101T120000Z\nEND:VEVENT\nEND:VCALENDAR\n";
        let cal = parse_ics(ics).unwrap();
        let ds = cal.events[0].dtstart.as_ref().unwrap();
        assert!(ds.utc);
        assert!(!ds.is_date);
        assert_eq!((ds.hour, ds.minute), (12, 0));
    }

    #[test]
    fn ics_rrule_until_form() {
        let r = RRule::parse("FREQ=DAILY;UNTIL=20261231T235959Z").unwrap();
        assert_eq!(r.freq, Freq::Daily);
        assert_eq!(r.interval, 1); // default
        assert_eq!(r.count, None);
        let u = r.until.as_ref().unwrap();
        assert_eq!((u.year, u.month, u.day), (2026, 12, 31));
        assert!(u.utc);
    }

    #[test]
    fn ics_lf_only_and_unknown_components_skipped() {
        // LF-only line endings, plus a VTIMEZONE + VALARM that must be ignored.
        let ics = "BEGIN:VCALENDAR\nBEGIN:VTIMEZONE\nTZID:UTC\nEND:VTIMEZONE\nBEGIN:VEVENT\nUID:lf-1\nSUMMARY:Hi\nX-CUSTOM:ignored\nBEGIN:VALARM\nACTION:DISPLAY\nEND:VALARM\nEND:VEVENT\nEND:VCALENDAR\n";
        let cal = parse_ics(ics).unwrap();
        assert_eq!(cal.events.len(), 1);
        assert_eq!(cal.events[0].uid, "lf-1");
        assert_eq!(cal.events[0].summary, "Hi");
    }

    #[test]
    fn ics_vtodo_parsed() {
        let ics = "BEGIN:VCALENDAR\nBEGIN:VTODO\nUID:t1\nSUMMARY:File taxes\nDUE:20260415\nSTATUS:NEEDS-ACTION\nEND:VTODO\nEND:VCALENDAR\n";
        let cal = parse_ics(ics).unwrap();
        assert_eq!(cal.todos.len(), 1);
        assert_eq!(cal.todos[0].summary, "File taxes");
        assert!(cal.todos[0].due.as_ref().unwrap().is_date);
    }

    // The "round-trip the important fields" assert, proven FAIL-able: this MUST
    // fail if any expected value is tweaked. (Kept passing here; a deliberate
    // change to the expected SUMMARY would turn it red — demonstrating it can
    // print FAIL.)
    #[test]
    fn ics_roundtrip_important_fields_failable() {
        let cal = parse_ics(ICS_TWO_EVENTS).unwrap();
        let e0 = &cal.events[0];
        let observed = alloc::format!(
            "{}|{}|{:04}{:02}{:02}T{:02}{:02}{:02}|{}",
            e0.uid,
            e0.summary, // tweak either of these → FAIL
            e0.dtstart.as_ref().unwrap().year,
            e0.dtstart.as_ref().unwrap().month,
            e0.dtstart.as_ref().unwrap().day,
            e0.dtstart.as_ref().unwrap().hour,
            e0.dtstart.as_ref().unwrap().minute,
            e0.dtstart.as_ref().unwrap().second,
            e0.location,
        );
        assert_eq!(
            observed,
            "event-001@raeen.os|Sprint planning|20260615T093000|Room 4, Building B"
        );
    }

    // ---- vCard -------------------------------------------------------------

    const VCF_40: &str = "\
BEGIN:VCARD\r
VERSION:4.0\r
FN:Ada Lovelace\r
N:Lovelace;Ada;Augusta;Ms.;\r
EMAIL;TYPE=work:ada@work.example\r
EMAIL;TYPE=home:ada@home.example\r
TEL;TYPE=cell:+1-555-0100\r
ORG:Analytical Engines;Research\r
TITLE:Mathematician\r
BDAY:18151210\r
URL:https://example.com/ada\r
ADR;TYPE=home:;;1 Computer Way;London;;EC1;UK\r
NOTE:First programmer\\npioneer\r
UID:urn:uuid:ada-0001\r
PHOTO;MEDIATYPE=image/png:https://example.com/ada.png\r
END:VCARD\r
BEGIN:VCARD\r
VERSION:4.0\r
FN:Grace Hopper\r
N:Hopper;Grace;;Rear Admiral;\r
EMAIL:grace@navy.example\r
TEL;TYPE=voice:+1-555-0199\r
END:VCARD\r
";

    // vCard 3.0 as Google/Apple export it (TYPE list, ENCODING=b for photo).
    const VCF_30: &str = "\
BEGIN:VCARD\n\
VERSION:3.0\n\
FN:Bob Builder\n\
N:Builder;Bob;;;\n\
EMAIL;TYPE=INTERNET,HOME:bob@home.example\n\
EMAIL;TYPE=INTERNET,WORK:bob@work.example\n\
TEL;TYPE=CELL:+1-555-0222\n\
ORG:Construction Co.\n\
BDAY:1970-01-15\n\
END:VCARD\n\
BEGIN:VCARD\n\
VERSION:3.0\n\
FN:Wendy Wires\n\
N:Wires;Wendy;;;\n\
EMAIL:wendy@example.com\n\
END:VCARD\n";

    #[test]
    fn vcf_40_two_contacts_exact_fields() {
        let book = parse_vcf(VCF_40).expect("parse");
        assert_eq!(book.contacts.len(), 2);

        let ada = &book.contacts[0];
        assert_eq!(ada.version, "4.0");
        assert_eq!(ada.fn_name, "Ada Lovelace");
        assert_eq!(ada.name.family, "Lovelace");
        assert_eq!(ada.name.given, "Ada");
        assert_eq!(ada.name.additional, "Augusta");
        assert_eq!(ada.name.prefix, "Ms.");

        // Multiple typed EMAILs.
        assert_eq!(ada.emails.len(), 2);
        assert!(ada.emails[0].has_type("WORK"));
        assert_eq!(ada.email_of_type("HOME").unwrap().value, "ada@home.example");

        assert_eq!(ada.phones[0].value, "+1-555-0100");
        assert!(ada.phones[0].has_type("CELL"));
        assert_eq!(ada.org, "Analytical Engines / Research");
        assert_eq!(ada.title, "Mathematician");
        assert_eq!(ada.bday, "18151210");
        assert_eq!(ada.urls[0].value, "https://example.com/ada");
        assert_eq!(ada.uid, "urn:uuid:ada-0001");

        // Structured ADR.
        assert_eq!(ada.addresses[0].street, "1 Computer Way");
        assert_eq!(ada.addresses[0].locality, "London");
        assert_eq!(ada.addresses[0].postal_code, "EC1");
        assert_eq!(ada.addresses[0].country, "UK");
        assert!(ada.addresses[0].types.iter().any(|t| t == "HOME"));

        // NOTE escape decode.
        assert_eq!(ada.note, "First programmer\npioneer");

        // PHOTO captured (NOT decoded) with media type.
        let photo = ada.photo.as_ref().unwrap();
        assert_eq!(photo.value, "https://example.com/ada.png");
        assert_eq!(photo.media_type, "image/png");

        let grace = &book.contacts[1];
        assert_eq!(grace.fn_name, "Grace Hopper");
        assert_eq!(grace.name.prefix, "Rear Admiral");
        assert_eq!(grace.emails[0].value, "grace@navy.example");
    }

    #[test]
    fn vcf_30_parses_with_type_lists() {
        let book = parse_vcf(VCF_30).expect("parse");
        assert_eq!(book.contacts.len(), 2);
        let bob = &book.contacts[0];
        assert_eq!(bob.version, "3.0");
        assert_eq!(bob.fn_name, "Bob Builder");
        assert_eq!(bob.name.family, "Builder");
        assert_eq!(bob.emails.len(), 2);
        // 3.0 TYPE list: INTERNET,HOME — both tokens captured.
        assert!(bob.emails[0].has_type("HOME"));
        assert!(bob.emails[0].has_type("INTERNET"));
        assert!(bob.emails[1].has_type("WORK"));
        assert_eq!(bob.phones[0].value, "+1-555-0222");
        assert_eq!(bob.org, "Construction Co.");
        assert_eq!(bob.bday, "1970-01-15");
        assert_eq!(book.contacts[1].fn_name, "Wendy Wires");
    }

    #[test]
    fn vcf_failable_roundtrip() {
        // Proven FAIL-able: tweak any expected token and this turns red.
        let book = parse_vcf(VCF_40).unwrap();
        let ada = &book.contacts[0];
        let observed = alloc::format!(
            "{}|{};{}|{}|{}",
            ada.fn_name,
            ada.name.family,
            ada.name.given,
            ada.emails.len(),
            ada.phones[0].value
        );
        assert_eq!(observed, "Ada Lovelace|Lovelace;Ada|2|+1-555-0100");
    }

    #[test]
    fn vcf_photo_30_base64_captured_not_decoded() {
        let v = "BEGIN:VCARD\nVERSION:3.0\nFN:X\nPHOTO;ENCODING=b;TYPE=JPEG:/9j/4AAQSkZJRg==\nEND:VCARD\n";
        let book = parse_vcf(v).unwrap();
        let p = book.contacts[0].photo.as_ref().unwrap();
        assert_eq!(p.encoding, "b");
        assert_eq!(p.value, "/9j/4AAQSkZJRg==");
        assert_eq!(p.media_type, "JPEG");
    }

    // ---- shared lexer ------------------------------------------------------

    #[test]
    fn lexer_unfolds_space_and_tab_continuations() {
        // RFC fold with both a space-led and a tab-led continuation.
        let doc = "DESCRIPTION:line one\n  and two\n\tand three\n";
        let lines = unfold_and_split(doc).unwrap();
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].name, "DESCRIPTION");
        // Per RFC, EXACTLY one leading whitespace char is removed per
        // continuation; the rest is stitched verbatim (so "two"+"and" abut).
        assert_eq!(lines[0].value, "line one and twoand three");
    }

    #[test]
    fn lexer_param_and_quoted_colon() {
        // A quoted param value containing a ':' must NOT end the name section.
        let lines = unfold_and_split("ATTENDEE;CN=\"Doe, John: VIP\":mailto:j@x.com\n").unwrap();
        let l = &lines[0];
        assert_eq!(l.name, "ATTENDEE");
        assert_eq!(l.param("CN"), Some("Doe, John: VIP"));
        assert_eq!(l.value, "mailto:j@x.com");
    }

    #[test]
    fn lexer_group_prefix_stripped() {
        let lines = unfold_and_split("item1.EMAIL;TYPE=home:a@b.com\n").unwrap();
        assert_eq!(lines[0].name, "EMAIL");
        assert_eq!(lines[0].value, "a@b.com");
    }

    #[test]
    fn decode_text_all_escapes() {
        assert_eq!(decode_text("a\\,b\\;c\\nd\\\\e"), "a,b;c\nd\\e");
        assert_eq!(decode_text("upper\\Nnewline"), "upper\nnewline");
        // trailing lone backslash is kept, never panics
        assert_eq!(decode_text("trail\\"), "trail\\");
    }

    // ---- hostile / never-panic / bounded ----------------------------------

    #[test]
    fn not_recognized_inputs() {
        assert_eq!(parse_ics(""), Err(PimError::NotRecognized));
        assert_eq!(
            parse_ics("just some text\nno calendar"),
            Err(PimError::NotRecognized)
        );
        assert_eq!(parse_vcf(""), Err(PimError::NotRecognized));
        assert_eq!(parse_vcf("garbage\n"), Err(PimError::NotRecognized));
        // an ICS is not a VCF and vice-versa
        assert_eq!(parse_vcf(ICS_TWO_EVENTS), Err(PimError::NotRecognized));
        assert_eq!(parse_ics(VCF_40), Err(PimError::NotRecognized));
    }

    #[test]
    fn unbalanced_begin_end_best_effort_no_panic() {
        // VEVENT never closed → still recovered best-effort, no panic.
        let ics = "BEGIN:VCALENDAR\nBEGIN:VEVENT\nUID:open\nSUMMARY:dangling\n";
        let cal = parse_ics(ics).unwrap();
        // unclosed event is not pushed (END never seen) — bounded, defensive.
        assert!(cal.events.is_empty());

        // Stray END with no BEGIN → ignored, no panic.
        let ics2 = "BEGIN:VCALENDAR\nEND:VEVENT\nEND:VCALENDAR\n";
        let _ = parse_ics(ics2).unwrap();

        // vCard with no END → best-effort kept.
        let vcf = "BEGIN:VCARD\nVERSION:4.0\nFN:NoEnd\n";
        let book = parse_vcf(vcf).unwrap();
        assert_eq!(book.contacts.len(), 1);
        assert_eq!(book.contacts[0].fn_name, "NoEnd");
    }

    #[test]
    fn truncated_and_garbage_never_panic() {
        let cases: &[&[u8]] = &[
            b"",
            b"BEGIN",
            b"BEGIN:VCALENDAR",
            b"BEGIN:VCARD\nVERSION",
            b"::::::\n;;;;;;\n",
            b"\x00\x01\x02\xff\xfe",
            b"BEGIN:VEVENT\nDTSTART:notadate\nEND:VEVENT",
        ];
        for c in cases {
            let _ = parse_ics_bytes(c);
            let _ = parse_vcf_bytes(c);
        }
    }

    #[test]
    fn pathological_fold_is_bounded_not_infinite() {
        // A single logical line folded into 100k continuation physical lines.
        let mut doc = String::from("DESCRIPTION:x");
        for _ in 0..100_000 {
            doc.push_str("\n y");
        }
        doc.push('\n');
        // MAX_FOLD is 65_536 < 100_000 → must return LimitExceeded, never hang.
        let r = unfold_and_split(&doc);
        assert_eq!(r, Err(PimError::LimitExceeded));
    }

    #[test]
    fn deep_nesting_is_bounded() {
        let mut ics = String::from("BEGIN:VCALENDAR\n");
        for _ in 0..(MAX_DEPTH + 50) {
            ics.push_str("BEGIN:VEVENT\n");
        }
        // Over MAX_DEPTH of nested BEGINs → LimitExceeded, no stack blowup.
        let r = parse_ics(&ics);
        assert_eq!(r, Err(PimError::LimitExceeded));
    }

    #[test]
    fn long_line_is_capped() {
        let mut doc = String::from("BEGIN:VCALENDAR\nSUMMARY:");
        // A single physical line longer than MAX_LINE_LEN.
        doc.push_str(&"a".repeat(MAX_LINE_LEN + 16));
        doc.push('\n');
        assert_eq!(unfold_and_split(&doc), Err(PimError::LimitExceeded));
    }

    #[test]
    fn seeded_fuzz_random_and_mutated_valid_never_panic() {
        // Deterministic LCG; mutate valid inputs + emit random bytes. Bounded
        // iterations, bounded input size — purely a panic/loop safety proof.
        let mut state: u64 = 0x5145_4544_4143_4154;
        let mut rng = || {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (state >> 33) as u32
        };

        let seeds: [&str; 2] = [ICS_TWO_EVENTS, VCF_40];
        for _ in 0..2000 {
            // random buffer
            let n = (rng() % 256) as usize;
            let mut buf = Vec::with_capacity(n);
            for _ in 0..n {
                buf.push((rng() & 0xff) as u8);
            }
            let _ = parse_ics_bytes(&buf);
            let _ = parse_vcf_bytes(&buf);

            // mutated valid seed
            let seed = seeds[(rng() as usize) % seeds.len()];
            let mut m = seed.as_bytes().to_vec();
            if !m.is_empty() {
                let muts = (rng() % 8) as usize;
                for _ in 0..muts {
                    let idx = (rng() as usize) % m.len();
                    m[idx] = (rng() & 0xff) as u8;
                }
            }
            let _ = parse_ics_bytes(&m);
            let _ = parse_vcf_bytes(&m);
        }
    }
}
