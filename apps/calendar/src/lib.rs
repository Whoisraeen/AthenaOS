//! RaeenOS Calendar & Contacts — *"bring my calendar & contacts over"*
//! (RaeenOS_Concept.md §Compatibility Strategy criterion #5: "import my calendar
//! & contacts from Google / Apple / Outlook").
//!
//! A first-party, fully-LOCAL calendar + address book — the macOS Calendar +
//! Contacts of RaeenOS, with zero networking. A switcher arrives with a Google /
//! Apple export: every one of those platforms emits its calendar as iCalendar
//! (`.ics`, RFC 5545) and its contacts as vCard (`.vcf`, RFC 6350 v3.0/4.0). This
//! app imports both off disk and makes them clickable — the offline on-ramp.
//!
//! Standalone userspace ELF launched from the start menu (`exec_path =
//! "calendar"`). The already-host-KAT'd [`rae_pim`] engine does ALL the parsing,
//! recurrence, and timezone work:
//!   * `parse_ics` / `parse_vcf` — the line-folded RFC grammars → typed models.
//!   * `VEvent::occurrences` / `recur::expand` — RRULE recurrence expansion over a
//!     visible window (DAILY/WEEKLY/MONTHLY/YEARLY + INTERVAL/COUNT/UNTIL/BYDAY).
//!   * `tz::to_zone` / `tz::tzinfo_for_iana` — POSIX-TZ engine that renders an
//!     event's start in the user's LOCAL time across DST.
//!   * `recur::{days_from_civil, civil_from_days, weekday_from_days, ...}` — the
//!     civil date math the month grid is built on.
//!
//! This crate is the clickable shell over them: an Import view, a month grid + an
//! agenda list of expanded occurrences, and a contacts list with a detail card.
//!
//! The decision/query logic lives in the syscall-free [`PimModel`] so the host
//! KAT (`cargo test -p calendar --features host`) links the LIVE engine with no
//! kernel: import a known `.ics` with a weekly RRULE and assert the expanded
//! occurrences fall on the right dates within a window; import a known `.vcf` and
//! assert the parsed contact's name + email; assert a TZ conversion gives the
//! expected local wall-clock time. Every assert is a real value (FAIL-able).
//!
//! TIME SOURCE: the live app reads `SYS_WALL_CLOCK` (syscall 40 — unix-epoch ns,
//! UTC), the SAME source the tray clock + Clock + Passwords apps read, to pick
//! "today" for the initial month. The recurrence + tz math are pure functions of
//! the imported data, so the host test drives them with fixed inputs.

// no_std for the real userspace ELF; std under `cargo test` so the host KAT can
// link. The live ELF entry point lives in the thin `src/main.rs` bin, which calls
// `run()` below. (`run` uses `Canvas::new`, which is `unsafe`, so the LIBRARY
// cannot `#![forbid(unsafe_code)]` — only the bin can; the one unsafe site is the
// surface-buffer Canvas + the raw `SYS_WALL_CLOCK` syscall, both documented.)
#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

use rae_pim::{
    civil_from_days, days_from_civil, days_in_month, parse_ics, parse_vcf, tz, AddressBook,
    Calendar, DateTime, PimError, VCard,
};

// `weekday_from_days` builds the month grid's first-column offset — render/click
// path only, so it would be an unused import under `cargo test`.
#[cfg(not(test))]
use rae_pim::weekday_from_days;

// The render/run path is live-ELF only; under `cargo test` only the PimModel
// (over rae_pim) is exercised, so the graphics/syscall imports are gated out to
// keep the host test warning-clean.
#[cfg(not(test))]
#[allow(unused_imports)]
use raekit;

#[cfg(not(test))]
use rae_tokens::DARK;
#[cfg(not(test))]
use raegfx::text::FontFamily;
#[cfg(not(test))]
use raegfx::Canvas;

// ── Window geometry (live ELF only) ──────────────────────────────────────

#[cfg(not(test))]
const WIN_W: usize = 640;
#[cfg(not(test))]
const WIN_H: usize = 560;
#[cfg(not(test))]
const SURFACE_VIRT: u64 = 0x0000_7C00_0000;

#[cfg(not(test))]
const TITLE_H: usize = 28;
#[cfg(not(test))]
const TABBAR_H: usize = 34;
#[cfg(not(test))]
const FOOTER_H: usize = 30;

/// On-screen present origin (`surface_present(sid, PRESENT_X, PRESENT_Y)`).
#[cfg(not(test))]
const PRESENT_X: i32 = 160;
#[cfg(not(test))]
const PRESENT_Y: i32 = 60;

/// How many recurrence occurrences we expand for a single visible month — a hard
/// cap so a pathological `FREQ=SECONDLY` rule can never flood the agenda (rae_pim
/// also bounds internally via `MAX_STEPS`). One per day of a long month is plenty.
const MAX_OCCURRENCES_PER_WINDOW: usize = 512;

/// The local zone the app renders event times in. v1 ships a single configurable
/// IANA name (no system-locale syscall yet); the rae_pim POSIX-TZ engine resolves
/// it across DST. A real `.ics` carries its own `TZID` per event, which is what
/// gets converted FROM; this is the zone converted TO. (Render-path only; the
/// host KAT passes the zone explicitly to [`PimModel::agenda`].)
#[cfg(not(test))]
const LOCAL_TZ_IANA: &str = "America/New_York";

// ── Default sample data (live ELF only) ──────────────────────────────────
//
// On first launch — before the user imports their own export — the app shows a
// tiny built-in sample so the views are never blank (the same "here is what it
// looks like" affordance a fresh macOS Calendar gives). Importing replaces it.

#[cfg(not(test))]
const SAMPLE_ICS: &str = "\
BEGIN:VCALENDAR\r
VERSION:2.0\r
PRODID:-//RaeenOS//calendar//EN\r
BEGIN:VEVENT\r
UID:welcome-001@raeen.os\r
DTSTART;TZID=America/New_York:20260601T090000\r
DTEND;TZID=America/New_York:20260601T093000\r
SUMMARY:Welcome to RaeenOS Calendar\r
LOCATION:RaeShell\r
RRULE:FREQ=WEEKLY;BYDAY=MO;COUNT=12\r
END:VEVENT\r
BEGIN:VEVENT\r
UID:welcome-002@raeen.os\r
DTSTART;VALUE=DATE:20260615\r
SUMMARY:Import your .ics and .vcf\r
END:VEVENT\r
END:VCALENDAR\r
";

#[cfg(not(test))]
const SAMPLE_VCF: &str = "\
BEGIN:VCARD\r
VERSION:4.0\r
FN:Rae Support\r
N:Support;Rae;;;\r
EMAIL;TYPE=work:hello@raeen.os\r
TEL;TYPE=cell:+1-555-0100\r
ORG:RaeenOS\r
END:VCARD\r
";

// ── Theme (live ELF only — the host KAT exercises only the PimModel) ──────

#[cfg(not(test))]
const BG: u32 = DARK.bg_base;
#[cfg(not(test))]
const PANEL: u32 = DARK.bg_raised;
#[cfg(not(test))]
const CELL_BG: u32 = DARK.bg_overlay;
#[cfg(not(test))]
const CELL_SEL: u32 = DARK.bg_elevated;
#[cfg(not(test))]
const STROKE: u32 = DARK.stroke_subtle;
#[cfg(not(test))]
const TEXT_PRIMARY: u32 = DARK.text_primary;
#[cfg(not(test))]
const TEXT_SECONDARY: u32 = DARK.text_secondary;
#[cfg(not(test))]
const TEXT_TERTIARY: u32 = DARK.text_tertiary;

#[cfg(not(test))]
fn accent() -> u32 {
    rae_tokens::derive_accent(raekit::sys::theme_accent(), &DARK).base
}

// ── Wall clock (SYS_WALL_CLOCK = 40, unix-epoch ns) ───────────────────────
//
// raekit has no wrapper for SYS_WALL_CLOCK, so we issue the raw syscall through
// the public `raekit::sys::syscall0` — the EXACT pattern the Clock + Passwords
// apps use (no new ABI surface for this slice). 0 means "unavailable" → fall back
// to the sample's month. Only compiled into the live ELF.
#[cfg(not(test))]
const SYS_WALL_CLOCK: u64 = 40;

#[cfg(not(test))]
fn wall_secs() -> u64 {
    let ns = unsafe { raekit::sys::syscall0(SYS_WALL_CLOCK) };
    ns / 1_000_000_000
}

// ===========================================================================
// PimModel — the syscall-free heart (host-KAT'd against the live rae_pim).
// ===========================================================================

/// One agenda row: an expanded occurrence's local-time start plus the event's
/// summary + location. `(hour, minute)` are 24h LOCAL wall-clock; `is_date` marks
/// an all-day (DATE-only) event for which no time is shown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgendaItem {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    /// True for an all-day (DATE-only, floating) event — render no time.
    pub is_date: bool,
    pub summary: String,
    pub location: String,
}

/// A safe-to-list contact summary: the formatted name + a single primary phone
/// and email (the first of each, the macOS-Contacts list affordance).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContactRow {
    pub name: String,
    pub email: String,
    pub phone: String,
}

/// The in-memory PIM state: the imported calendar + address book over the LIVE
/// rae_pim engine. All query logic (recurrence expansion into a visible window,
/// local-time conversion, contact summarisation) is here and syscall-free, so the
/// host KAT drives it directly.
pub struct PimModel {
    cal: Calendar,
    book: AddressBook,
}

impl Default for PimModel {
    fn default() -> Self {
        Self::new()
    }
}

impl PimModel {
    /// An empty model (no calendar, no contacts).
    pub fn new() -> PimModel {
        PimModel {
            cal: Calendar::default(),
            book: AddressBook::default(),
        }
    }

    /// Import (REPLACE) the calendar from an `.ics` document. Returns the number
    /// of VEVENTs parsed, or the parse error (an unrecognised/over-cap document
    /// leaves the existing calendar untouched).
    pub fn import_ics(&mut self, ics: &str) -> Result<usize, PimError> {
        let cal = parse_ics(ics)?;
        let n = cal.events.len();
        self.cal = cal;
        Ok(n)
    }

    /// Import (REPLACE) the contacts from a `.vcf` document. Returns the number of
    /// contacts parsed, or the parse error.
    pub fn import_vcf(&mut self, vcf: &str) -> Result<usize, PimError> {
        let book = parse_vcf(vcf)?;
        let n = book.contacts.len();
        self.book = book;
        Ok(n)
    }

    /// Number of imported calendar events (pre-expansion).
    pub fn event_count(&self) -> usize {
        self.cal.events.len()
    }

    /// Number of imported contacts.
    pub fn contact_count(&self) -> usize {
        self.book.contacts.len()
    }

    /// Expand every event's recurrences that fall within the inclusive civil day
    /// window `[start, end]`, converting each occurrence's start to the LOCAL zone
    /// `local_tz` (an IANA name; falls back to the raw occurrence time if the zone
    /// or the event's own TZID can't be resolved). Returns the agenda sorted
    /// chronologically by local start. Bounded by [`MAX_OCCURRENCES_PER_WINDOW`].
    ///
    /// This is the core "what's on my calendar this month, in MY time" query — it
    /// drives both the month grid (which days have events) and the agenda list.
    pub fn agenda(&self, start: &DateTime, end: &DateTime, local_tz: &str) -> Vec<AgendaItem> {
        // Resolve the local target zone once. `None` → render raw occurrence time.
        let target = tz::tzinfo_for_iana(local_tz).and_then(|r| r.ok());

        let mut out: Vec<AgendaItem> = Vec::new();
        for ev in &self.cal.events {
            if out.len() >= MAX_OCCURRENCES_PER_WINDOW {
                break;
            }
            let budget = MAX_OCCURRENCES_PER_WINDOW - out.len();
            for occ in ev.occurrences(start, end, budget) {
                // Convert to local wall-clock. An all-day (DATE) occurrence has no
                // instant, so it is shown on its civil day unchanged.
                let local = match (&target, occ.is_date) {
                    (Some(t), false) => tz::to_zone(&occ, t),
                    _ => occ.clone(),
                };
                out.push(AgendaItem {
                    year: local.year,
                    month: local.month,
                    day: local.day,
                    hour: local.hour,
                    minute: local.minute,
                    is_date: occ.is_date,
                    summary: ev.summary.clone(),
                    location: ev.location.clone(),
                });
                if out.len() >= MAX_OCCURRENCES_PER_WINDOW {
                    break;
                }
            }
        }
        out.sort_by(|a, b| {
            let ka = (a.year, a.month, a.day, a.hour, a.minute);
            let kb = (b.year, b.month, b.day, b.hour, b.minute);
            ka.cmp(&kb)
        });
        out
    }

    /// The set of civil days (1..=31) within month `(year, month)` that carry at
    /// least one occurrence — used to dot the month grid. Convenience over
    /// [`agenda`] for the full month window.
    pub fn busy_days(&self, year: u16, month: u8, local_tz: &str) -> Vec<u8> {
        let (start, end) = month_window(year, month);
        let mut days: Vec<u8> = Vec::new();
        for item in self.agenda(&start, &end, local_tz) {
            if item.year == year && item.month == month && !days.contains(&item.day) {
                days.push(item.day);
            }
        }
        days.sort_unstable();
        days
    }

    /// The agenda items that fall on a specific civil day, in local time order.
    pub fn agenda_for_day(&self, year: u16, month: u8, day: u8, local_tz: &str) -> Vec<AgendaItem> {
        let target = DateTime {
            year,
            month,
            day,
            ..Default::default()
        };
        self.agenda(&target, &day_end(&target), local_tz)
            .into_iter()
            .filter(|i| i.year == year && i.month == month && i.day == day)
            .collect()
    }

    /// The contact summary rows (name + primary email + primary phone), in the
    /// order the `.vcf` listed them.
    pub fn contact_rows(&self) -> Vec<ContactRow> {
        self.book.contacts.iter().map(contact_row).collect()
    }

    /// The full parsed contact at `idx` (for the detail card), if in range.
    pub fn contact(&self, idx: usize) -> Option<&VCard> {
        self.book.contacts.get(idx)
    }
}

/// Build the safe list summary for one contact: prefer FN, fall back to the
/// structured N "given family"; first email + first phone as the primaries.
fn contact_row(c: &VCard) -> ContactRow {
    let name = if !c.fn_name.is_empty() {
        c.fn_name.clone()
    } else {
        let mut n = String::new();
        if !c.name.given.is_empty() {
            n.push_str(&c.name.given);
        }
        if !c.name.family.is_empty() {
            if !n.is_empty() {
                n.push(' ');
            }
            n.push_str(&c.name.family);
        }
        if n.is_empty() {
            n.push_str("(no name)");
        }
        n
    };
    let email = c
        .emails
        .first()
        .map(|e| e.value.clone())
        .unwrap_or_default();
    let phone = c
        .phones
        .first()
        .map(|p| p.value.clone())
        .unwrap_or_default();
    ContactRow { name, email, phone }
}

/// The inclusive civil window covering all of month `(year, month)`: day 1
/// 00:00:00 .. last-day 23:59:59. Used for month-grid expansion.
fn month_window(year: u16, month: u8) -> (DateTime, DateTime) {
    let last = days_in_month(year as i64, month);
    let start = DateTime {
        year,
        month,
        day: 1,
        ..Default::default()
    };
    let end = DateTime {
        year,
        month,
        day: last,
        hour: 23,
        minute: 59,
        second: 59,
        ..Default::default()
    };
    (start, end)
}

/// End-of-day (23:59:59) for the civil day of `d`.
fn day_end(d: &DateTime) -> DateTime {
    DateTime {
        year: d.year,
        month: d.month,
        day: d.day,
        hour: 23,
        minute: 59,
        second: 59,
        ..Default::default()
    }
}

/// The civil `(year, month, day)` for a unix timestamp (UTC). Used by the live app
/// to pick the initial month; the conversion is pure rae_pim civil math.
pub fn civil_from_unix(unix_secs: u64) -> (u16, u8, u8) {
    // Days since the civil epoch 1970-01-01 (rae_pim's `days_from_civil` epoch).
    let days = (unix_secs / 86_400) as i64;
    let (y, m, d) = civil_from_days(days);
    (y as u16, m, d)
}

// ===========================================================================
// App state + render (live ELF only — syscall-touching).
// ===========================================================================

/// Which top-level view is showing.
#[cfg(not(test))]
#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    /// The month grid + agenda for the selected day.
    Calendar,
    /// The contact list + detail card.
    Contacts,
}

/// A short status line shown in the footer (e.g. "Imported 12 events").
#[cfg(not(test))]
struct Toast {
    text: String,
}

#[cfg(not(test))]
impl Toast {
    fn new() -> Toast {
        Toast {
            text: String::new(),
        }
    }
    fn set(&mut self, s: &str) {
        self.text.clear();
        self.text.push_str(s);
    }
    fn clear(&mut self) {
        self.text.clear();
    }
}

/// The whole live app.
#[cfg(not(test))]
struct App {
    model: PimModel,
    tab: Tab,
    /// The month currently shown in the grid.
    view_year: u16,
    view_month: u8,
    /// The selected civil day within the view month (1..=31), drives the agenda.
    sel_day: u8,
    /// Selected contact row index.
    sel_contact: usize,
    toast: Toast,
}

#[cfg(not(test))]
impl App {
    fn new() -> App {
        let mut model = PimModel::new();
        // Seed with the built-in sample (and any import the user does replaces it).
        let _ = model.import_ics(SAMPLE_ICS);
        let _ = model.import_vcf(SAMPLE_VCF);

        // Pick the initial month from the wall clock; fall back to the sample's
        // month (June 2026) when the clock is unavailable.
        let (y, m, d) = {
            let secs = wall_secs();
            if secs == 0 {
                (2026, 6, 1)
            } else {
                civil_from_unix(secs)
            }
        };

        App {
            model,
            tab: Tab::Calendar,
            view_year: y,
            view_month: m,
            sel_day: d,
            sel_contact: 0,
            toast: Toast::new(),
        }
    }

    /// Step the visible month by `delta` (±1), wrapping the year.
    fn step_month(&mut self, delta: i32) {
        let mut m = self.view_month as i32 + delta;
        let mut y = self.view_year as i32;
        while m < 1 {
            m += 12;
            y -= 1;
        }
        while m > 12 {
            m -= 12;
            y += 1;
        }
        self.view_year = y.max(1) as u16;
        self.view_month = m as u8;
        // Clamp the selected day into the new month.
        let last = days_in_month(self.view_year as i64, self.view_month);
        if self.sel_day > last {
            self.sel_day = last;
        }
        self.toast.clear();
    }

    /// Import the calendar/contacts from the conventional export paths in the
    /// user's home (`~/import.ics`, `~/import.vcf`). Best-effort: a missing file
    /// or parse error sets a toast and leaves the current data intact.
    #[cfg(not(test))]
    fn import_from_home(&mut self) {
        let mut imported = 0usize;
        let mut contacts = 0usize;
        if let Some(ics) = read_home_file("import.ics") {
            if let Ok(n) = self.model.import_ics(&ics) {
                imported = n;
            }
        }
        if let Some(vcf) = read_home_file("import.vcf") {
            if let Ok(n) = self.model.import_vcf(&vcf) {
                contacts = n;
            }
        }
        if imported == 0 && contacts == 0 {
            self.toast.set("No ~/import.ics or ~/import.vcf found");
        } else {
            let mut s = String::new();
            s.push_str("Imported ");
            push_num(&mut s, imported as u64);
            s.push_str(" events, ");
            push_num(&mut s, contacts as u64);
            s.push_str(" contacts");
            self.toast.set(&s);
            self.sel_contact = 0;
        }
    }
}

// ── Persistence / import (live ELF only) ──────────────────────────────────

/// Read a file from the session home (`<home>/<name>`) into a String, or `None`.
#[cfg(not(test))]
fn read_home_file(name: &str) -> Option<String> {
    let mut path = String::new();
    let mut info = [0u8; 96];
    if raekit::sys::session_info(&mut info).is_some() {
        if let Some(home) = raekit::sys::session_home_from(&info) {
            path.push_str(home);
            path.push('/');
            path.push_str(name);
        }
    }
    if path.is_empty() {
        path.push_str("/home/user/");
        path.push_str(name);
    }

    let fd = raekit::sys::open(path.as_str(), 0);
    if fd == u64::MAX {
        return None;
    }
    let mut data: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        // Cap: a calendar/contacts export is text, not a media file.
        if data.len() > 16 * 1024 * 1024 {
            break;
        }
        let n = raekit::sys::read(fd, &mut chunk) as usize;
        if n == 0 || n > chunk.len() {
            break;
        }
        data.extend_from_slice(&chunk[..n]);
    }
    let _ = raekit::sys::close(fd);
    if data.is_empty() {
        None
    } else {
        Some(lossy_string(data))
    }
}

/// Lossy UTF-8 decode of an owned byte vector (no_std-safe): the valid prefix is
/// kept and any invalid sequence becomes U+FFFD. (`String::from_utf8_lossy` exists
/// but borrows; the owned-vec variant is nightly-unstable, so do it inline.)
#[cfg(not(test))]
fn lossy_string(bytes: Vec<u8>) -> String {
    match String::from_utf8(bytes) {
        Ok(s) => s,
        Err(e) => {
            let bytes = e.into_bytes();
            let mut out = String::with_capacity(bytes.len());
            let mut i = 0;
            while i < bytes.len() {
                match core::str::from_utf8(&bytes[i..]) {
                    Ok(valid) => {
                        out.push_str(valid);
                        break;
                    }
                    Err(e2) => {
                        let good = e2.valid_up_to();
                        if good > 0 {
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
    }
}

// ── Render ─────────────────────────────────────────────────────────────────

#[cfg(not(test))]
const MONTH_NAMES: [&str; 12] = [
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
#[cfg(not(test))]
const WEEKDAY_HDR: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

#[cfg(not(test))]
fn render(app: &App, canvas: &mut Canvas) {
    canvas.fill_rect(0, 0, WIN_W, WIN_H, BG);

    // Title bar.
    canvas.fill_rect(0, 0, WIN_W, TITLE_H, PANEL);
    canvas.draw_text_aa(
        10,
        ((TITLE_H - rae_tokens::TYPE_SUBTITLE.line_height as usize) / 2) as i32,
        "Calendar & Contacts",
        rae_tokens::TYPE_SUBTITLE,
        TEXT_SECONDARY,
        FontFamily::Sans,
    );

    render_tabs(app, canvas);
    match app.tab {
        Tab::Calendar => render_calendar(app, canvas),
        Tab::Contacts => render_contacts(app, canvas),
    }
    render_footer(app, canvas);
}

#[cfg(not(test))]
fn render_tabs(app: &App, canvas: &mut Canvas) {
    let y = TITLE_H;
    canvas.fill_rect(0, y, WIN_W, TABBAR_H, PANEL);
    let half = WIN_W / 2;
    for (i, (label, tab)) in [("Calendar", Tab::Calendar), ("Contacts", Tab::Contacts)]
        .iter()
        .enumerate()
    {
        let tx = i * half;
        let active = app.tab == *tab;
        if active {
            canvas.fill_rect(tx, y, half, TABBAR_H, CELL_SEL);
            canvas.fill_rect(tx, y + TABBAR_H - 3, half, 3, accent());
        }
        let fg = if active { TEXT_PRIMARY } else { TEXT_SECONDARY };
        let lw = canvas.measure_text_aa(label, rae_tokens::TYPE_LABEL, FontFamily::Sans);
        canvas.draw_text_aa(
            (tx + half / 2) as i32 - lw / 2,
            (y + (TABBAR_H - rae_tokens::TYPE_LABEL.line_height as usize) / 2) as i32,
            label,
            rae_tokens::TYPE_LABEL,
            fg,
            FontFamily::Sans,
        );
    }
}

#[cfg(not(test))]
fn render_calendar(app: &App, canvas: &mut Canvas) {
    let top = TITLE_H + TABBAR_H + 8;

    // Month header: "June 2026".
    let mut header = String::new();
    header.push_str(MONTH_NAMES[(app.view_month.saturating_sub(1) as usize).min(11)]);
    header.push(' ');
    push_num(&mut header, app.view_year as u64);
    canvas.draw_text_aa(
        16,
        top as i32,
        &header,
        rae_tokens::TYPE_TITLE,
        TEXT_PRIMARY,
        FontFamily::Sans,
    );

    // Weekday header row (Mon-first).
    let grid_top = top + 34;
    let grid_left = 16usize;
    let grid_w = WIN_W - 32;
    let col_w = grid_w / 7;
    for (i, name) in WEEKDAY_HDR.iter().enumerate() {
        canvas.draw_text_aa(
            (grid_left + i * col_w + 4) as i32,
            grid_top as i32,
            name,
            rae_tokens::TYPE_CAPTION,
            TEXT_TERTIARY,
            FontFamily::Sans,
        );
    }

    // Month grid. weekday_from_days returns 0=Sun..6=Sat; convert to Mon-first.
    let cells_top = grid_top + 18;
    let row_h = 46usize;
    let first_days = days_from_civil(app.view_year as i64, app.view_month as i64, 1);
    let first_wd_sun0 = weekday_from_days(first_days); // 0=Sun
    let first_col = ((first_wd_sun0 + 6) % 7) as usize; // 0=Mon
    let last = days_in_month(app.view_year as i64, app.view_month);
    let busy = app
        .model
        .busy_days(app.view_year, app.view_month, LOCAL_TZ_IANA);

    for day in 1..=last {
        let idx = first_col + (day as usize - 1);
        let col = idx % 7;
        let row = idx / 7;
        let cx = grid_left + col * col_w;
        let cy = cells_top + row * row_h;
        let selected = day == app.sel_day;
        let bg = if selected { CELL_SEL } else { CELL_BG };
        canvas.fill_rounded_rect(
            cx + 2,
            cy,
            col_w - 4,
            row_h - 4,
            rae_tokens::RADIUS_SM as usize,
            bg,
        );
        if selected {
            // Accent outline on the selected day.
            canvas.fill_rect(cx + 2, cy, col_w - 4, 2, accent());
        }
        let mut ds = String::new();
        push_num(&mut ds, day as u64);
        canvas.draw_text_aa(
            (cx + 6) as i32,
            (cy + 4) as i32,
            &ds,
            rae_tokens::TYPE_BODY,
            if selected {
                TEXT_PRIMARY
            } else {
                TEXT_SECONDARY
            },
            FontFamily::Sans,
        );
        // Busy dot.
        if busy.contains(&day) {
            canvas.fill_circle(cx + col_w / 2, cy + row_h - 10, 3, accent());
        }
    }

    // Agenda for the selected day, below the grid.
    let grid_rows = ((first_col + last as usize) + 6) / 7;
    let agenda_top = cells_top + grid_rows * row_h + 8;
    render_agenda(app, canvas, agenda_top);
}

#[cfg(not(test))]
fn render_agenda(app: &App, canvas: &mut Canvas, top: usize) {
    let mut hdr = String::new();
    hdr.push_str("Events on ");
    hdr.push_str(MONTH_NAMES[(app.view_month.saturating_sub(1) as usize).min(11)]);
    hdr.push(' ');
    push_num(&mut hdr, app.sel_day as u64);
    canvas.draw_text_aa(
        16,
        top as i32,
        &hdr,
        rae_tokens::TYPE_LABEL,
        TEXT_SECONDARY,
        FontFamily::Sans,
    );

    let items = app
        .model
        .agenda_for_day(app.view_year, app.view_month, app.sel_day, LOCAL_TZ_IANA);
    let list_top = top + 20;
    if items.is_empty() {
        canvas.draw_text_aa(
            16,
            list_top as i32,
            "No events.  Press I to import ~/import.ics.",
            rae_tokens::TYPE_CAPTION,
            TEXT_TERTIARY,
            FontFamily::Sans,
        );
        return;
    }
    let row_h = 30usize;
    let max_rows = (WIN_H - list_top - FOOTER_H) / row_h;
    for (i, item) in items.iter().take(max_rows).enumerate() {
        let ry = list_top + i * row_h;
        canvas.fill_rounded_rect(
            12,
            ry,
            WIN_W - 24,
            row_h - 4,
            rae_tokens::RADIUS_SM as usize,
            CELL_BG,
        );
        // Time chip.
        let time = if item.is_date {
            String::from("all-day")
        } else {
            let mut t = String::new();
            two_digit(&mut t, item.hour as u64);
            t.push(':');
            two_digit(&mut t, item.minute as u64);
            t
        };
        canvas.draw_text_aa(
            20,
            (ry + 5) as i32,
            &time,
            rae_tokens::TYPE_CAPTION,
            accent(),
            FontFamily::Mono,
        );
        canvas.draw_text_aa(
            96,
            (ry + 5) as i32,
            &item.summary,
            rae_tokens::TYPE_BODY,
            TEXT_PRIMARY,
            FontFamily::Sans,
        );
    }
}

#[cfg(not(test))]
fn render_contacts(app: &App, canvas: &mut Canvas) {
    let top = TITLE_H + TABBAR_H + 8;
    let rows = app.model.contact_rows();
    if rows.is_empty() {
        canvas.draw_text_aa(
            16,
            top as i32,
            "No contacts.  Press I to import ~/import.vcf.",
            rae_tokens::TYPE_BODY,
            TEXT_TERTIARY,
            FontFamily::Sans,
        );
        return;
    }

    // Left list, right detail card.
    let list_w = WIN_W / 2;
    let row_h = 44usize;
    let max_rows = (WIN_H - top - FOOTER_H) / row_h;
    let sel = app.sel_contact.min(rows.len().saturating_sub(1));
    for (i, row) in rows.iter().take(max_rows).enumerate() {
        let ry = top + i * row_h;
        let selected = i == sel;
        let bg = if selected { CELL_SEL } else { CELL_BG };
        canvas.fill_rounded_rect(
            8,
            ry,
            list_w - 16,
            row_h - 6,
            rae_tokens::RADIUS_SM as usize,
            bg,
        );
        canvas.draw_text_aa(
            18,
            (ry + 5) as i32,
            &row.name,
            rae_tokens::TYPE_BODY,
            TEXT_PRIMARY,
            FontFamily::Sans,
        );
        let sub = if !row.email.is_empty() {
            row.email.as_str()
        } else {
            row.phone.as_str()
        };
        canvas.draw_text_aa(
            18,
            (ry + 23) as i32,
            sub,
            rae_tokens::TYPE_CAPTION,
            TEXT_SECONDARY,
            FontFamily::Sans,
        );
    }

    // Detail card.
    let cx = list_w + 8;
    let cw = WIN_W - cx - 12;
    let cy = top;
    let ch = WIN_H - top - FOOTER_H - 8;
    canvas.fill_rounded_rect(cx, cy, cw, ch, rae_tokens::RADIUS_MD as usize, PANEL);
    canvas.fill_rect(cx, cy, cw, 1, STROKE);
    if let Some(card) = app.model.contact(sel) {
        let mut y = cy + 16;
        let name = if !card.fn_name.is_empty() {
            card.fn_name.as_str()
        } else {
            "(no name)"
        };
        canvas.draw_text_aa(
            (cx + 16) as i32,
            y as i32,
            name,
            rae_tokens::TYPE_TITLE,
            TEXT_PRIMARY,
            FontFamily::Sans,
        );
        y += 36;
        if !card.org.is_empty() {
            canvas.draw_text_aa(
                (cx + 16) as i32,
                y as i32,
                &card.org,
                rae_tokens::TYPE_CAPTION,
                TEXT_TERTIARY,
                FontFamily::Sans,
            );
            y += 24;
        }
        for e in &card.emails {
            detail_line(canvas, cx + 16, &mut y, "email", &e.value);
        }
        for p in &card.phones {
            detail_line(canvas, cx + 16, &mut y, "phone", &p.value);
        }
        if !card.title.is_empty() {
            detail_line(canvas, cx + 16, &mut y, "title", &card.title);
        }
    }
}

#[cfg(not(test))]
fn detail_line(canvas: &mut Canvas, x: usize, y: &mut usize, label: &str, value: &str) {
    canvas.draw_text_aa(
        x as i32,
        *y as i32,
        label,
        rae_tokens::TYPE_CAPTION,
        TEXT_TERTIARY,
        FontFamily::Sans,
    );
    canvas.draw_text_aa(
        x as i32,
        (*y + 14) as i32,
        value,
        rae_tokens::TYPE_BODY,
        TEXT_PRIMARY,
        FontFamily::Sans,
    );
    *y += 40;
}

#[cfg(not(test))]
fn render_footer(app: &App, canvas: &mut Canvas) {
    let fy = WIN_H - FOOTER_H;
    canvas.fill_rect(0, fy, WIN_W, FOOTER_H, PANEL);
    let hint = match app.tab {
        Tab::Calendar => "<-/->: month   Up/Dn: day   I: import   1/2: tabs   Esc: quit",
        Tab::Contacts => "Up/Dn: select   I: import   1/2: tabs   Esc: quit",
    };
    canvas.draw_text_aa(
        10,
        fy as i32 + ((FOOTER_H - rae_tokens::TYPE_CAPTION.line_height as usize) / 2) as i32,
        if app.toast.text.is_empty() {
            hint
        } else {
            app.toast.text.as_str()
        },
        rae_tokens::TYPE_CAPTION,
        if app.toast.text.is_empty() {
            TEXT_TERTIARY
        } else {
            accent()
        },
        FontFamily::Sans,
    );
}

/// Append a decimal number to `s` (no_std-safe, no formatting machinery).
#[cfg(not(test))]
fn push_num(s: &mut String, mut n: u64) {
    if n == 0 {
        s.push('0');
        return;
    }
    let mut buf = [0u8; 20];
    let mut i = buf.len();
    while n > 0 {
        i -= 1;
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
    }
    if let Ok(t) = core::str::from_utf8(&buf[i..]) {
        s.push_str(t);
    }
}

/// Append a zero-padded two-digit number (hours / minutes).
#[cfg(not(test))]
fn two_digit(s: &mut String, n: u64) {
    if n < 10 {
        s.push('0');
    }
    push_num(s, n);
}

// ===========================================================================
// Live entry point.
// ===========================================================================

/// The freestanding userspace entry (called by the `_start` shim in `main.rs`).
/// Creates the window surface, seeds the sample data, runs the event loop, and
/// redraws on change.
#[cfg(not(test))]
pub fn run() -> ! {
    let sid = raekit::sys::surface_create(WIN_W as u64, WIN_H as u64, SURFACE_VIRT);
    if sid == u64::MAX {
        raekit::sys::exit(1);
    }
    let mut canvas = unsafe { Canvas::new(SURFACE_VIRT as *mut u8, WIN_W, WIN_H, 4) };

    let mut app = App::new();
    render(&app, &mut canvas);
    raekit::sys::surface_present(sid, PRESENT_X as u64, PRESENT_Y as u64);

    let mut extended = false;

    loop {
        // Mouse: a click on the tab bar switches tabs; a click on a month cell
        // selects that day; a click on a contact row selects it.
        let mut left_down = false;
        let mut mouse_edge = false;
        loop {
            let ev = raekit::sys::poll_mouse();
            if ev == 0 {
                break;
            }
            let now_down = (ev & 0x01) != 0;
            if now_down && !left_down {
                mouse_edge = true;
            }
            left_down = now_down;
        }
        if mouse_edge {
            let (cx, cy, _btn) = raekit::sys::cursor_pos();
            let (ox, oy) =
                raekit::sys::surface_origin(sid).unwrap_or((PRESENT_X as u32, PRESENT_Y as u32));
            let lx = (cx as i32).saturating_sub(ox as i32);
            let ly = (cy as i32).saturating_sub(oy as i32);
            if handle_click(&mut app, lx, ly) {
                render(&app, &mut canvas);
                raekit::sys::surface_present(sid, PRESENT_X as u64, PRESENT_Y as u64);
            }
        }

        let key = raekit::sys::read_key();
        if key == 0 {
            raekit::sys::yield_now();
            continue;
        }

        let sc = key as u8;
        if sc == 0xE0 {
            extended = true;
            continue;
        }
        let ext = core::mem::replace(&mut extended, false);
        let release = sc & 0x80 != 0;
        let code = sc & 0x7F;
        if release {
            continue;
        }

        let mut changed = true;

        // Esc → quit.
        if code == 0x01 {
            raekit::sys::exit(0);
        }
        // Arrow keys (extended).
        else if ext && code == 0x4B {
            // Left → previous month.
            if app.tab == Tab::Calendar {
                app.step_month(-1);
            } else {
                changed = false;
            }
        } else if ext && code == 0x4D {
            // Right → next month.
            if app.tab == Tab::Calendar {
                app.step_month(1);
            } else {
                changed = false;
            }
        } else if ext && code == 0x48 {
            // Up.
            match app.tab {
                Tab::Calendar => {
                    if app.sel_day > 7 {
                        app.sel_day -= 7;
                    }
                }
                Tab::Contacts => {
                    if app.sel_contact > 0 {
                        app.sel_contact -= 1;
                    }
                }
            }
        } else if ext && code == 0x50 {
            // Down.
            match app.tab {
                Tab::Calendar => {
                    let last = days_in_month(app.view_year as i64, app.view_month);
                    if app.sel_day + 7 <= last {
                        app.sel_day += 7;
                    }
                }
                Tab::Contacts => {
                    let n = app.model.contact_count();
                    if app.sel_contact + 1 < n {
                        app.sel_contact += 1;
                    }
                }
            }
        }
        // Non-extended command keys.
        else {
            match code {
                // '1' / '2' tabs.
                0x02 => {
                    app.tab = Tab::Calendar;
                    app.toast.clear();
                }
                0x03 => {
                    app.tab = Tab::Contacts;
                    app.toast.clear();
                }
                // 'i' import.
                0x17 => app.import_from_home(),
                _ => changed = false,
            }
        }

        if changed {
            render(&app, &mut canvas);
            raekit::sys::surface_present(sid, PRESENT_X as u64, PRESENT_Y as u64);
        }
    }
}

/// Hit-test a surface-local click. Returns whether anything changed (redraw).
#[cfg(not(test))]
fn handle_click(app: &mut App, lx: i32, ly: i32) -> bool {
    if lx < 0 || ly < 0 {
        return false;
    }
    // Tab bar.
    let tab_top = TITLE_H as i32;
    if ly >= tab_top && ly < tab_top + TABBAR_H as i32 {
        app.tab = if lx < (WIN_W / 2) as i32 {
            Tab::Calendar
        } else {
            Tab::Contacts
        };
        app.toast.clear();
        return true;
    }

    match app.tab {
        Tab::Calendar => {
            // Recompute the grid geometry exactly as render_calendar does.
            let top = TITLE_H + TABBAR_H + 8;
            let grid_top = top + 34;
            let grid_left = 16usize;
            let grid_w = WIN_W - 32;
            let col_w = grid_w / 7;
            let cells_top = grid_top + 18;
            let row_h = 46usize;
            let first_days = days_from_civil(app.view_year as i64, app.view_month as i64, 1);
            let first_col = ((weekday_from_days(first_days) + 6) % 7) as usize;
            let last = days_in_month(app.view_year as i64, app.view_month);
            let lxu = lx as usize;
            let lyu = ly as usize;
            if lxu >= grid_left && lyu >= cells_top {
                let col = (lxu - grid_left) / col_w;
                let row = (lyu - cells_top) / row_h;
                if col < 7 {
                    let idx = row * 7 + col;
                    if idx >= first_col {
                        let day = (idx - first_col + 1) as u8;
                        if day >= 1 && day <= last {
                            app.sel_day = day;
                            app.toast.clear();
                            return true;
                        }
                    }
                }
            }
            false
        }
        Tab::Contacts => {
            let top = TITLE_H + TABBAR_H + 8;
            let row_h = 44usize;
            let list_w = WIN_W / 2;
            if (lx as usize) < list_w && (ly as usize) >= top {
                let i = ((ly as usize) - top) / row_h;
                if i < app.model.contact_count() {
                    app.sel_contact = i;
                    return true;
                }
            }
            false
        }
    }
}

// ===========================================================================
// Host KAT — links the LIVE rae_pim engine, no kernel. `cargo test -p calendar
// --features host`.
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use rae_pim::tz::tzinfo_for_iana;

    // A known .ics: a WEEKLY-on-Monday event seeded 2026-06-01 (a Monday),
    // COUNT=12, plus an all-day DATE event on 2026-06-15. The weekly RRULE is the
    // FAIL-able anchor: the expanded occurrences MUST land on the Mondays of June.
    const ICS: &str = "\
BEGIN:VCALENDAR\r
VERSION:2.0\r
PRODID:-//RaeenOS//calendar-test//EN\r
BEGIN:VEVENT\r
UID:standup@raeen.os\r
DTSTART;TZID=America/New_York:20260601T093000\r
DTEND;TZID=America/New_York:20260601T094500\r
SUMMARY:Daily Standup\r
LOCATION:War Room\r
RRULE:FREQ=WEEKLY;BYDAY=MO;COUNT=12\r
END:VEVENT\r
BEGIN:VEVENT\r
UID:allhands@raeen.os\r
DTSTART;VALUE=DATE:20260615\r
SUMMARY:All Hands\r
END:VEVENT\r
END:VCALENDAR\r
";

    // A known .vcf (vCard 4.0): the FAIL-able anchor is the parsed FN + EMAIL.
    const VCF: &str = "\
BEGIN:VCARD\r
VERSION:4.0\r
FN:Grace Hopper\r
N:Hopper;Grace;;Rear Admiral;\r
EMAIL;TYPE=work:grace@navy.example\r
TEL;TYPE=cell:+1-555-0199\r
ORG:US Navy\r
END:VCARD\r
";

    #[test]
    fn import_ics_expands_weekly_rrule_onto_right_dates() {
        let mut m = PimModel::new();
        let n = m.import_ics(ICS).expect("parse ics");
        assert_eq!(n, 2, "two VEVENTs imported");
        assert_eq!(m.event_count(), 2);

        // Visible window: all of June 2026. The weekly-Monday event seeded
        // 2026-06-01 (a Monday) recurs on the 1st, 8th, 15th, 22nd, 29th.
        let (start, end) = month_window(2026, 6);
        let agenda = m.agenda(&start, &end, "America/New_York");

        // Collect just the standup occurrence days (the all-day "All Hands" is
        // also on the 15th — filter by summary so the anchor is precise).
        let mut standup_days: Vec<u8> = agenda
            .iter()
            .filter(|a| a.summary == "Daily Standup")
            .map(|a| a.day)
            .collect();
        standup_days.sort_unstable();
        standup_days.dedup();
        assert_eq!(
            standup_days,
            alloc::vec![1u8, 8, 15, 22, 29],
            "weekly RRULE must land on the Mondays of June 2026"
        );

        // The all-day event shows up on the 15th as a date-only item.
        let allhands: Vec<&AgendaItem> =
            agenda.iter().filter(|a| a.summary == "All Hands").collect();
        assert_eq!(allhands.len(), 1);
        assert!(allhands[0].is_date, "DATE event must be all-day");
        assert_eq!((allhands[0].month, allhands[0].day), (6, 15));

        // busy_days unions both events: the 15th has both.
        let busy = m.busy_days(2026, 6, "America/New_York");
        assert_eq!(busy, alloc::vec![1u8, 8, 15, 22, 29]);

        // A window OUTSIDE the recurrence (next year) yields nothing.
        let (s2, e2) = month_window(2027, 6);
        assert!(m.agenda(&s2, &e2, "America/New_York").is_empty());
    }

    #[test]
    fn import_vcf_parses_name_and_email() {
        let mut m = PimModel::new();
        let n = m.import_vcf(VCF).expect("parse vcf");
        assert_eq!(n, 1);
        assert_eq!(m.contact_count(), 1);

        let rows = m.contact_rows();
        assert_eq!(rows.len(), 1);
        // FAIL-able anchors: the exact formatted name + primary email + phone.
        assert_eq!(rows[0].name, "Grace Hopper");
        assert_eq!(rows[0].email, "grace@navy.example");
        assert_eq!(rows[0].phone, "+1-555-0199");

        // The detail card carries the full parsed contact.
        let card = m.contact(0).expect("contact 0");
        assert_eq!(card.org, "US Navy");
        assert_eq!(card.name.family, "Hopper");
        assert_eq!(card.name.given, "Grace");
    }

    #[test]
    fn timezone_conversion_renders_local_wall_clock() {
        // The standup's DTSTART is 09:30 in America/New_York. In June that is EDT
        // (UTC-4) → 13:30 UTC. Project that into Australia/Sydney: July/June is
        // AEST (UTC+10) → 23:30 the SAME civil day. This is the FAIL-able TZ
        // anchor: a wrong offset (or no conversion) gives a different wall-clock.
        let mut m = PimModel::new();
        m.import_ics(ICS).unwrap();

        let (start, end) = month_window(2026, 6);

        // Sanity: in its own zone the first occurrence renders at 09:30 local.
        let ny = m.agenda(&start, &end, "America/New_York");
        let first_ny = ny.iter().find(|a| a.summary == "Daily Standup").unwrap();
        assert_eq!((first_ny.hour, first_ny.minute), (9, 30));
        assert_eq!(first_ny.day, 1);

        // Projected into Sydney, the SAME instant is 23:30 local on June 1.
        let syd = m.agenda(&start, &end, "Australia/Sydney");
        let first_syd = syd.iter().find(|a| a.summary == "Daily Standup").unwrap();
        assert_eq!(
            (first_syd.hour, first_syd.minute),
            (23, 30),
            "09:30 EDT must render as 23:30 AEST in Sydney"
        );
        assert_eq!(first_syd.day, 1);
    }

    #[test]
    fn tz_engine_resolves_both_zones() {
        // Guard: the two IANA names the TZ test relies on must resolve through the
        // curated map (else the conversion test would silently fall back to raw).
        assert!(tzinfo_for_iana("America/New_York")
            .and_then(|r| r.ok())
            .is_some());
        assert!(tzinfo_for_iana("Australia/Sydney")
            .and_then(|r| r.ok())
            .is_some());
    }

    #[test]
    fn empty_model_is_quiet() {
        let m = PimModel::new();
        assert_eq!(m.event_count(), 0);
        assert_eq!(m.contact_count(), 0);
        assert!(m.contact_rows().is_empty());
        assert!(m.contact(0).is_none());
        let (s, e) = month_window(2026, 6);
        assert!(m.agenda(&s, &e, "America/New_York").is_empty());
        assert!(m.busy_days(2026, 6, "America/New_York").is_empty());
    }

    #[test]
    fn malformed_import_is_an_error_not_a_panic() {
        let mut m = PimModel::new();
        m.import_ics(ICS).unwrap();
        // A garbage document is rejected and leaves the prior calendar intact.
        assert!(m.import_ics("not a calendar at all").is_err());
        assert_eq!(m.event_count(), 2, "failed import must not clobber data");
    }

    #[test]
    fn civil_from_unix_matches_known_epoch() {
        // 2026-06-01 00:00:00 UTC. days_from_civil(2026,6,1) * 86400.
        let days = days_from_civil(2026, 6, 1);
        let unix = (days as u64) * 86_400;
        assert_eq!(civil_from_unix(unix), (2026, 6, 1));
        // The unix epoch itself.
        assert_eq!(civil_from_unix(0), (1970, 1, 1));
    }
}
