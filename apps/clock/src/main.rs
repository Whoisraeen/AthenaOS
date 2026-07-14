//! AthenaOS Clock + Calendar — *"what time is it, and what's the date?"*
//! (LEGACY_GAMING_CONCEPT.md §Three User Experiences — the daily-driver parity bar vs
//! Windows 11 Clock (world-clock / alarms / timer / stopwatch) and macOS Clock +
//! Calendar).
//!
//! Five tabs across the top toolbar, accent-active like Notes' edit/preview chips:
//!   - **Clock**     — a large digital HH:MM:SS readout + the long date line, plus
//!                     an analog face drawn with raegfx circles/lines, ticking each
//!                     second off the system wall clock.
//!   - **Alarms**    — a list of HH:MM alarms with an enabled toggle; an alarm
//!                     whose minute matches the wall clock shows a "RINGING" state.
//!   - **Timer**     — a key-entered countdown (MM:SS) with start/pause/reset that
//!                     surfaces a visible "Time's up" banner when it reaches zero.
//!   - **Stopwatch** — start/stop/lap/reset with an elapsed display and lap list.
//!   - **Calendar**  — a month grid (weeks × days) for the current month, today
//!                     highlighted in the accent, prev/next month navigation.
//!
//! Standalone userspace ELF (`exec_path = "clock"`). Chrome rides the shared
//! `rae_tokens` design language; the live desktop accent comes through
//! `SYS_THEME_GET` (raekit::sys::theme_accent) at launch, so Clock matches the
//! desktop 1:1 (whole-OS cohesion).
//!
//! TIME SOURCE: the wall clock is `SYS_WALL_CLOCK` (syscall 40 — unix-epoch
//! nanoseconds, UTC), the SAME source the tray clock reads (kernel
//! `game_session::sys_wall_clock`). raekit exposes no wrapper for it, so we call
//! `raekit::sys::syscall0(40)` directly rather than add an ABI surface. Monotonic
//! elapsed time (timer / stopwatch) uses `raekit::sys::time_ns()` (SYS_TIME — ns
//! from boot) so it is immune to any wall-clock adjustment.
//!
//! NEVER PANICS: a bad/zero wall-clock read degrades the readout to `--:--:--`
//! and the app stays alive; the date math is total (saturating, no unwraps).
//!
//! PROOF: this ELF can't run `cargo test`, so `design_proof()` (a fail-able
//! runtime gate at `_start`) asserts the pure civil-date math — `days_in_month`
//! (leap-year Feb), the day-of-week of a known epoch date, and the month-grid
//! first-cell offset — AND the token wiring; exit(3) on any drift.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::vec::Vec;

#[allow(unused_imports)]
use raekit;

use alloc::string::String;
use rae_tokens::{TypeStyle, DARK, RAEBLUE};
use rae_toml::Toml;
use raegfx::text::FontFamily;
use raegfx::Canvas;

// ── Window geometry ─────────────────────────────────────────────────────

const WIN_W: usize = 720;
const WIN_H: usize = 520;
const SURFACE_VIRT: u64 = 0x0000_7F00_0000;

const TITLE_H: usize = 28;
const TOOLBAR_H: usize = 38;
const STATUS_H: usize = 22;

/// The on-screen origin we present this window at. Absolute cursor coordinates
/// from `cursor_pos()` are converted to surface-local space by subtracting this.
const PRESENT_X: i32 = 150;
const PRESENT_Y: i32 = 80;

// Tab-button layout — shared by draw + hit so they can't drift (mirrors the
// `render` toolbar loop: start x=8, w=96, h=26, gap=6, y=TITLE_H+6).
const TAB_BTN_W: usize = 96;
const TAB_BTN_H: usize = 26;
const TAB_BTN_GAP: usize = 6;
const TAB_BTN_START_X: usize = 8;

fn tab_btn_x(i: usize) -> usize {
    TAB_BTN_START_X + i * (TAB_BTN_W + TAB_BTN_GAP)
}
fn tab_btn_y() -> usize {
    TITLE_H + 6
}

// Content-area transport-button metrics (Timer/Stopwatch) — shared draw + hit.
const TR_BTN_W: usize = 96;
const TR_BTN_H: usize = 32;
const TR_BTN_GAP: usize = 12;

/// X of content transport button `i` for a cluster of `n` buttons, centered.
fn content_btn_x(i: usize, n: usize, area_w: usize) -> usize {
    let cluster_w = n * TR_BTN_W + (n.saturating_sub(1)) * TR_BTN_GAP;
    let start = (area_w.saturating_sub(cluster_w)) / 2;
    start + i * (TR_BTN_W + TR_BTN_GAP)
}

/// Y of the content transport-button row (below the big readout).
fn content_btn_y() -> usize {
    WIN_H - STATUS_H - TR_BTN_H - 24
}

// ── Palette (rae_tokens, docs/design/design-language.md) ──────────────────

const BG: u32 = DARK.bg_raised;
const TITLE_BG: u32 = DARK.bg_base;
const TOOLBAR_BG: u32 = DARK.bg_overlay;
const TEXT_FG: u32 = DARK.text_primary;
const TEXT_MUTED: u32 = DARK.text_secondary;
const TEXT_DIM: u32 = DARK.text_tertiary;
const STATUS_BG: u32 = DARK.bg_base;
const STROKE_HL: u32 = DARK.stroke_strong;
const PANEL_BG: u32 = DARK.bg_base;

fn theme_seed() -> u32 {
    raekit::sys::theme_accent()
}

/// Accent base, derived through the shared ramp from the live theme seed.
fn accent() -> u32 {
    rae_tokens::derive_accent(theme_seed(), &DARK).base
}

/// Opaque selection/active fill: the accent's pressed/active shade.
fn sel_fill() -> u32 {
    rae_tokens::derive_accent(theme_seed(), &DARK).active
}

// ── Wall clock (SYS_WALL_CLOCK = 40, unix-epoch ns) ───────────────────────
//
// raekit has no wrapper for SYS_WALL_CLOCK, so we issue the raw syscall through
// the public `raekit::sys::syscall0`. 0 means "unavailable" → the readout shows
// `--:--:--`. We never add a new ABI surface for this slice.
const SYS_WALL_CLOCK: u64 = 40;

fn wall_clock_ns() -> u64 {
    unsafe { raekit::sys::syscall0(SYS_WALL_CLOCK) }
}

/// Whole seconds since the Unix epoch (UTC). 0 if the clock is unavailable.
fn wall_secs() -> u64 {
    wall_clock_ns() / 1_000_000_000
}

// ── Pure civil date math (the design_proof subject) ───────────────────────
//
// Total, allocation-free, panic-free. All of this is asserted by `design_proof`
// because an ELF bin can't run `cargo test`.

/// Days in `month` (1..=12) of `year`, honoring the Gregorian leap rule.
pub fn days_in_month(year: i64, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if is_leap_year(year) {
                29
            } else {
                28
            }
        }
        _ => 0,
    }
}

/// Gregorian leap-year rule: divisible by 4, except centuries not by 400.
pub fn is_leap_year(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
}

/// A broken-down UTC civil date+time.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Civil {
    pub year: i64,
    pub month: u32, // 1..=12
    pub day: u32,   // 1..=31
    pub hour: u32,  // 0..=23
    pub min: u32,   // 0..=59
    pub sec: u32,   // 0..=59
    /// 0 = Sunday .. 6 = Saturday.
    pub weekday: u32,
}

/// Convert whole Unix seconds (UTC) to a civil date+time. Uses Howard Hinnant's
/// days-from-civil inverse (`civil_from_days`) — branch-free, no leap tables.
pub fn civil_from_unix(secs: u64) -> Civil {
    let days = (secs / 86_400) as i64;
    let rem = (secs % 86_400) as u32;
    let hour = rem / 3600;
    let min = (rem % 3600) / 60;
    let sec = rem % 60;

    // days is the count from 1970-01-01 (the Unix epoch).
    // weekday: 1970-01-01 was a Thursday (=4). Add and wrap.
    let weekday = (((days % 7) + 4) % 7 + 7) as u32 % 7;

    // civil_from_days (Hinnant): shift epoch to 0000-03-01.
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as i64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let month = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32; // [1, 12]
    let year = if month <= 2 { y + 1 } else { y };

    Civil {
        year,
        month,
        day,
        hour,
        min,
        sec,
        weekday,
    }
}

/// Day-of-week (0=Sun..6=Sat) of the 1st of `(year, month)`. Pure — used to lay
/// out the calendar grid's first cell. Computed via days-from-civil so it shares
/// no state with `civil_from_unix`'s inverse (an independent cross-check).
pub fn weekday_of_first(year: i64, month: u32) -> u32 {
    let days = days_from_civil(year, month, 1);
    (((days % 7) + 4) % 7 + 7) as u32 % 7
}

/// Hinnant's days_from_civil: serial day number from 1970-01-01 for a civil date.
pub fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as i64; // [0, 399]
    let m = month as i64;
    let d = day as i64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146_097 + doe - 719_468
}

// ── Tabs ──────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Clock,
    Alarms,
    Timer,
    Stopwatch,
    Calendar,
}

const TABS: [(Tab, &str); 5] = [
    (Tab::Clock, "Clock"),
    (Tab::Alarms, "Alarms"),
    (Tab::Timer, "Timer"),
    (Tab::Stopwatch, "Stopwatch"),
    (Tab::Calendar, "Calendar"),
];

impl Tab {
    /// Stable token persisted in the prefs file.
    fn as_token(self) -> &'static str {
        match self {
            Tab::Clock => "clock",
            Tab::Alarms => "alarms",
            Tab::Timer => "timer",
            Tab::Stopwatch => "stopwatch",
            Tab::Calendar => "calendar",
        }
    }
    /// Parse the persisted token; unknown / missing → the typed default (`Clock`).
    fn from_token(s: &str) -> Self {
        match s {
            "alarms" => Tab::Alarms,
            "timer" => Tab::Timer,
            "stopwatch" => Tab::Stopwatch,
            "calendar" => Tab::Calendar,
            _ => Tab::Clock,
        }
    }
}

// ── Timer ───────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
enum TimerState {
    Idle,
    Running,
    Paused,
    Fired,
}

struct CountdownTimer {
    /// Configured total seconds (set via key entry before start).
    set_secs: u64,
    /// Seconds remaining when last paused / at start.
    remaining: u64,
    /// Monotonic ns at which the current run started (Running only).
    run_start_ns: u64,
    state: TimerState,
    /// Digit-entry accumulator (MMSS, rightmost = seconds ones).
    entry: u64,
}

impl CountdownTimer {
    fn new() -> Self {
        Self {
            set_secs: 0,
            remaining: 0,
            run_start_ns: 0,
            state: TimerState::Idle,
            entry: 0,
        }
    }

    /// Seconds remaining right now (live while Running).
    fn live_remaining(&self) -> u64 {
        match self.state {
            TimerState::Running => {
                let elapsed =
                    raekit::sys::time_ns().saturating_sub(self.run_start_ns) / 1_000_000_000;
                self.remaining.saturating_sub(elapsed)
            }
            _ => self.remaining,
        }
    }

    /// Advance the state machine; returns true if it just fired this tick.
    fn poll(&mut self) -> bool {
        if self.state == TimerState::Running && self.live_remaining() == 0 {
            self.state = TimerState::Fired;
            self.remaining = 0;
            return true;
        }
        false
    }

    /// Push one digit into the MM:SS entry (max 99:59-ish; we cap at 4 digits).
    fn push_digit(&mut self, d: u64) {
        if self.state == TimerState::Running {
            return;
        }
        if self.entry < 10_000 {
            self.entry = self.entry * 10 + d;
        }
        // Reflect entry into the set/remaining (MM:SS form).
        let mm = self.entry / 100;
        let ss = self.entry % 100;
        self.set_secs = mm * 60 + ss.min(59);
        self.remaining = self.set_secs;
        self.state = TimerState::Idle;
    }

    fn start_pause(&mut self) {
        match self.state {
            TimerState::Idle => {
                if self.remaining == 0 {
                    return;
                }
                self.run_start_ns = raekit::sys::time_ns();
                self.state = TimerState::Running;
            }
            TimerState::Running => {
                // Freeze the live remaining and pause.
                self.remaining = self.live_remaining();
                self.state = TimerState::Paused;
            }
            TimerState::Paused => {
                self.run_start_ns = raekit::sys::time_ns();
                self.state = TimerState::Running;
            }
            TimerState::Fired => {}
        }
    }

    fn reset(&mut self) {
        self.remaining = self.set_secs;
        self.state = TimerState::Idle;
        self.entry = self.set_secs / 60 * 100 + self.set_secs % 60;
    }
}

// ── Stopwatch ────────────────────────────────────────────────────────────────

struct Stopwatch {
    running: bool,
    /// Accumulated ns from completed run segments.
    accum_ns: u64,
    /// Monotonic ns at which the current run segment started (running only).
    seg_start_ns: u64,
    laps: Vec<u64>, // elapsed-ns snapshots
}

impl Stopwatch {
    fn new() -> Self {
        Self {
            running: false,
            accum_ns: 0,
            seg_start_ns: 0,
            laps: Vec::new(),
        }
    }

    fn elapsed_ns(&self) -> u64 {
        if self.running {
            self.accum_ns
                .saturating_add(raekit::sys::time_ns().saturating_sub(self.seg_start_ns))
        } else {
            self.accum_ns
        }
    }

    fn start_stop(&mut self) {
        if self.running {
            self.accum_ns = self.elapsed_ns();
            self.running = false;
        } else {
            self.seg_start_ns = raekit::sys::time_ns();
            self.running = true;
        }
    }

    fn lap(&mut self) {
        if self.running && self.laps.len() < 64 {
            self.laps.push(self.elapsed_ns());
        }
    }

    fn reset(&mut self) {
        self.running = false;
        self.accum_ns = 0;
        self.seg_start_ns = 0;
        self.laps.clear();
    }
}

// ── Alarms ───────────────────────────────────────────────────────────────────

#[derive(Clone, Copy)]
struct Alarm {
    hour: u32,
    min: u32,
    enabled: bool,
}

const MAX_ALARMS: usize = 16;

// ── Persistent preferences (rae_toml) ─────────────────────────────────────────
//
// LEGACY_GAMING_CONCEPT.md §"The user owns the machine": "remember my settings" must be
// real. Clock persists its active tab AND — most importantly — the ALARM LIST
// (HH:MM + enabled per alarm) to `<home>/.config/clock.toml`, restoring them on
// launch. Alarms that vanish on relaunch are useless, so each is serialized as a
// `[[alarm]]` array-of-tables entry. Every load is hostile-input-tolerant: a
// missing, corrupt, or out-of-range config falls back to TYPED DEFAULTS and NEVER
// panics — the app always starts. This is the per-app prefs pattern the consumer
// apps follow (the proven Music recipe).

const PATH_CAP: usize = 256;

/// A small fixed-capacity path builder (Clock has no other PathBuf use).
#[derive(Clone, Copy)]
struct PathBuf {
    bytes: [u8; PATH_CAP],
    len: usize,
}

impl PathBuf {
    fn new() -> Self {
        Self {
            bytes: [0; PATH_CAP],
            len: 0,
        }
    }
    fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.len]).unwrap_or("/")
    }
    fn set(&mut self, s: &str) {
        let n = s.as_bytes().len().min(PATH_CAP);
        self.bytes[..n].copy_from_slice(&s.as_bytes()[..n]);
        self.len = n;
    }
    fn push_component(&mut self, name: &str) {
        if self.len > 0 && self.bytes[self.len - 1] != b'/' && self.len < PATH_CAP {
            self.bytes[self.len] = b'/';
            self.len += 1;
        }
        for &b in name.as_bytes() {
            if self.len >= PATH_CAP {
                break;
            }
            self.bytes[self.len] = b;
            self.len += 1;
        }
    }
}

/// The decoded, defaulted preferences restored at launch. Pure data: load builds
/// it from a parsed (or absent) TOML, save serializes the live App state.
#[derive(Clone)]
struct Prefs {
    /// The active tab on relaunch.
    tab: Tab,
    /// The persisted alarm list (HH:MM + enabled per alarm).
    alarms: Vec<Alarm>,
}

impl Prefs {
    /// The typed defaults used on first run or any config error: the Clock tab and
    /// the same two seeded (disabled) alarms `App::new` starts with.
    fn defaults() -> Self {
        Self {
            tab: Tab::Clock,
            alarms: default_alarms(),
        }
    }

    /// Build `Prefs` from a parsed TOML table, validating every field and
    /// substituting the typed default for any missing / wrong-typed value. Never
    /// panics. The alarm list is read PER ENTRY: each `[[alarm]]` with an in-range
    /// hour (0..=23) and minute (0..=59) is kept; garbage entries are skipped, not
    /// fatal. An entirely missing `alarm` key restores the seeded defaults.
    fn from_toml(t: &Toml) -> Self {
        let mut p = Self::defaults();
        if let Some(s) = t.get("tab").and_then(Toml::as_str) {
            p.tab = Tab::from_token(s);
        }
        // Only override the seeded defaults if the file actually carries an alarm
        // array (so a config that simply omits alarms keeps the defaults, while a
        // config with an explicit empty array yields an empty list).
        if let Some(arr) = t.get("alarm").and_then(Toml::as_array) {
            let mut alarms: Vec<Alarm> = Vec::new();
            for entry in arr.iter() {
                if alarms.len() >= MAX_ALARMS {
                    break;
                }
                // Each entry should be a table with hour/min/enabled. Read each
                // field defensively; clamp the time into range; skip a non-table.
                let hour = entry.get("hour").and_then(Toml::as_i64);
                let min = entry.get("min").and_then(Toml::as_i64);
                let (h, m) = match (hour, min) {
                    (Some(h), Some(m)) => (h, m),
                    _ => continue, // not a well-formed alarm entry → skip it
                };
                let enabled = entry
                    .get("enabled")
                    .and_then(Toml::as_bool)
                    .unwrap_or(false);
                alarms.push(Alarm {
                    hour: h.clamp(0, 23) as u32,
                    min: m.clamp(0, 59) as u32,
                    enabled,
                });
            }
            p.alarms = alarms;
        }
        p
    }

    /// Serialize the live preferences into an order-stable `Toml::Table` ready for
    /// `rae_toml::to_string`. The tab is a flat key; the alarm list is an
    /// array-of-tables (`[[alarm]]`) so it round-trips one entry per alarm.
    fn to_toml(&self) -> Toml {
        let mut table: Vec<(String, Toml)> = Vec::new();
        table.push((
            String::from("tab"),
            Toml::String(String::from(self.tab.as_token())),
        ));
        let mut arr: Vec<Toml> = Vec::new();
        for a in self.alarms.iter() {
            let mut entry: Vec<(String, Toml)> = Vec::new();
            entry.push((String::from("hour"), Toml::Integer(a.hour as i64)));
            entry.push((String::from("min"), Toml::Integer(a.min as i64)));
            entry.push((String::from("enabled"), Toml::Boolean(a.enabled)));
            arr.push(Toml::Table(entry));
        }
        table.push((String::from("alarm"), Toml::Array(arr)));
        Toml::Table(table)
    }
}

/// The two seeded (disabled) alarms the app starts with on first run.
fn default_alarms() -> Vec<Alarm> {
    let mut alarms = Vec::new();
    alarms.push(Alarm {
        hour: 7,
        min: 0,
        enabled: false,
    });
    alarms.push(Alarm {
        hour: 22,
        min: 30,
        enabled: false,
    });
    alarms
}

/// The per-app config DIRECTORY: `<session home>/.config`. Falls back to the same
/// `/home/user` default when no session is present. Created (idempotent) before
/// any write.
fn prefs_dir() -> PathBuf {
    let mut p = PathBuf::new();
    let mut info = [0u8; 96];
    if raekit::sys::session_info(&mut info).is_some() {
        if let Some(home) = raekit::sys::session_home_from(&info) {
            p.set(home);
            p.push_component(".config");
            return p;
        }
    }
    p.set("/home/user/.config");
    p
}

/// Load preferences from `<home>/.config/clock.toml`. On ANY failure — file
/// absent, unreadable, not UTF-8, or a `rae_toml::parse` error — returns the typed
/// defaults. Never panics, never blocks the app from launching.
fn load_prefs() -> Prefs {
    let mut path = prefs_dir();
    path.push_component("clock.toml");
    let fd = raekit::sys::open(path.as_str(), 0);
    if fd == u64::MAX {
        return Prefs::defaults();
    }
    let mut data: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        // Hard cap: a config file should be tiny; refuse to slurp a giant blob.
        if data.len() > 64 * 1024 {
            break;
        }
        let n = raekit::sys::read(fd, &mut chunk) as usize;
        if n == 0 || n > chunk.len() {
            break;
        }
        data.extend_from_slice(&chunk[..n]);
    }
    let _ = raekit::sys::close(fd);
    let text = match core::str::from_utf8(&data) {
        Ok(s) => s,
        Err(_) => return Prefs::defaults(),
    };
    match rae_toml::parse(text) {
        Ok(t) => Prefs::from_toml(&t),
        Err(_) => Prefs::defaults(),
    }
}

/// Persist `prefs` to `<home>/.config/clock.toml` (best effort). Creates the
/// `.config` directory if missing, serializes via `rae_toml::to_string`, and
/// writes O_CREAT|O_TRUNC. A failure is silent — the app keeps running.
fn save_prefs(prefs: &Prefs) {
    let dir = prefs_dir();
    let _ = raekit::sys::mkdir(dir.as_str());
    let mut path = dir;
    path.push_component("clock.toml");
    let text = rae_toml::to_string(&prefs.to_toml());
    // O_WRONLY | O_CREAT | O_TRUNC = 0x0241.
    let fd = raekit::sys::open(path.as_str(), 0x0241);
    if fd == u64::MAX {
        return;
    }
    let bytes = text.as_bytes();
    let mut off = 0usize;
    while off < bytes.len() {
        let end = (off + 4096).min(bytes.len());
        let n = raekit::sys::write(fd, &bytes[off..end]) as usize;
        if n == 0 {
            break;
        }
        off += n;
    }
    let _ = raekit::sys::close(fd);
}

// ── App state ─────────────────────────────────────────────────────────────

struct App {
    tab: Tab,
    /// Last whole-second rendered (so we only repaint when it changes).
    last_secs: u64,
    timer: CountdownTimer,
    stopwatch: Stopwatch,
    alarms: Vec<Alarm>,
    alarm_sel: usize,
    /// Alarm edit accumulator (HHMM digit entry) for the "add" row.
    alarm_entry: u32,
    /// Calendar view month/year (defaults to today, navigable).
    cal_year: i64,
    cal_month: u32,
    cal_initialized: bool,
    /// A calendar day the user clicked to select (0 = none), highlighted in-grid.
    cal_selected_day: u32,
    toast: [u8; 48],
    toast_len: usize,
}

impl App {
    fn new() -> Self {
        // Restore saved preferences (active tab + alarm list); typed defaults on
        // first run or any config error (the two seeded disabled alarms).
        let prefs = load_prefs();
        let mut alarm_sel = 0usize;
        if alarm_sel >= prefs.alarms.len() {
            alarm_sel = prefs.alarms.len().saturating_sub(1);
        }
        Self {
            tab: prefs.tab,
            last_secs: u64::MAX,
            timer: CountdownTimer::new(),
            stopwatch: Stopwatch::new(),
            alarms: prefs.alarms,
            alarm_sel,
            alarm_entry: 0,
            cal_year: 1970,
            cal_month: 1,
            cal_initialized: false,
            cal_selected_day: 0,
            toast: [0; 48],
            toast_len: 0,
        }
    }

    /// Snapshot the live persistable state (active tab + alarm list) into a `Prefs`
    /// and write it to disk. Called on every alarm change and tab switch. Best
    /// effort + silent on failure (the app never blocks on the config write).
    fn persist(&self) {
        let prefs = Prefs {
            tab: self.tab,
            alarms: self.alarms.clone(),
        };
        save_prefs(&prefs);
    }

    fn set_toast(&mut self, s: &str) {
        let n = s.as_bytes().len().min(48);
        self.toast[..n].copy_from_slice(&s.as_bytes()[..n]);
        self.toast_len = n;
    }
    fn toast_str(&self) -> &str {
        core::str::from_utf8(&self.toast[..self.toast_len]).unwrap_or("")
    }

    /// Lazily anchor the calendar on the real current month (once we have a clock).
    fn ensure_calendar(&mut self, now: Civil) {
        if !self.cal_initialized {
            self.cal_year = now.year;
            self.cal_month = now.month;
            self.cal_initialized = true;
        }
    }

    fn cal_prev(&mut self) {
        if self.cal_month == 1 {
            self.cal_month = 12;
            self.cal_year -= 1;
        } else {
            self.cal_month -= 1;
        }
        self.cal_selected_day = 0; // day numbers change meaning across months
    }
    fn cal_next(&mut self) {
        if self.cal_month == 12 {
            self.cal_month = 1;
            self.cal_year += 1;
        } else {
            self.cal_month += 1;
        }
        self.cal_selected_day = 0;
    }

    /// True if any enabled alarm matches the current H:M (the "ringing" check).
    fn ringing_alarm(&self, now: Civil) -> Option<usize> {
        self.alarms
            .iter()
            .position(|a| a.enabled && a.hour == now.hour && a.min == now.min && now.sec < 60)
    }

    fn add_alarm_from_entry(&mut self) {
        if self.alarms.len() >= MAX_ALARMS {
            self.set_toast("Alarm list full");
            return;
        }
        let hh = (self.alarm_entry / 100).min(23);
        let mm = (self.alarm_entry % 100).min(59);
        self.alarms.push(Alarm {
            hour: hh,
            min: mm,
            enabled: true,
        });
        self.alarm_entry = 0;
        self.set_toast("Alarm added");
        // Persist the alarm list so it survives relaunch.
        self.persist();
    }

    fn remove_alarm(&mut self) {
        if self.alarm_sel < self.alarms.len() {
            self.alarms.remove(self.alarm_sel);
            if self.alarm_sel >= self.alarms.len() {
                self.alarm_sel = self.alarms.len().saturating_sub(1);
            }
            self.set_toast("Alarm removed");
            self.persist();
        }
    }

    fn toggle_alarm(&mut self) {
        if let Some(a) = self.alarms.get_mut(self.alarm_sel) {
            a.enabled = !a.enabled;
            self.persist();
        }
    }
}

// ── Mouse hit-testing (single source of truth: draw-rects == hit-rects) ───
//
// Tab + transport-button + calendar geometry computed from the SAME constants
// the renderer uses, so a click can never drift from the visual. A click
// dispatches to the EXACT action the matching key fires; empty space resolves
// to `Action::None` (no-op, never panics).

/// What a left-click maps to — each mirrors a keyboard action 1:1.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Action {
    /// Switch to tab `t` (the 1-5 number keys / Tab cycle).
    SwitchTab(Tab),
    /// Timer/Stopwatch transport (Space).
    TimerStartPause,
    TimerReset,
    StopwatchStartStop,
    StopwatchLap,
    StopwatchReset,
    /// Calendar prev/next month (Left/Right) + a day-cell select.
    CalPrev,
    CalNext,
    CalSelectDay(u32),
    Close,
    None,
}

#[derive(Clone, Copy)]
struct Rect {
    x: usize,
    y: usize,
    w: usize,
    h: usize,
}

impl Rect {
    fn contains(&self, px: i32, py: i32) -> bool {
        px >= self.x as i32
            && px < (self.x + self.w) as i32
            && py >= self.y as i32
            && py < (self.y + self.h) as i32
    }
}

/// The window-close (X) rect in the title bar.
fn close_rect() -> Rect {
    Rect {
        x: WIN_W - 28,
        y: 4,
        w: 20,
        h: 20,
    }
}

// Calendar nav-arrow + grid metrics — duplicated from `render_calendar` so draw
// and hit share one geometry. The content area is (0, TITLE_H+TOOLBAR_H,
// WIN_W, WIN_H - that - STATUS_H).
const CAL_PAD: usize = 24;
const CAL_ARROW_W: usize = 28; // generous click target around the chevrons

fn content_rect() -> Rect {
    let cy = TITLE_H + TOOLBAR_H;
    Rect {
        x: 0,
        y: cy,
        w: WIN_W,
        h: WIN_H - cy - STATUS_H,
    }
}

/// The calendar "previous month" chevron click zone.
fn cal_prev_rect() -> Rect {
    let area = content_rect();
    Rect {
        x: area.x + CAL_PAD,
        y: area.y,
        w: CAL_ARROW_W,
        h: 36,
    }
}

/// The calendar "next month" chevron click zone.
fn cal_next_rect() -> Rect {
    let area = content_rect();
    Rect {
        x: area.x + area.w - CAL_PAD - CAL_ARROW_W,
        y: area.y,
        w: CAL_ARROW_W,
        h: 36,
    }
}

impl App {
    /// The surface-local rect of calendar day cell for `day` (1..=ndays) in the
    /// current view month, or `None` if out of range. Uses the SAME metrics
    /// `render_calendar` draws.
    fn cal_day_rect(&self, day: u32) -> Option<Rect> {
        let area = content_rect();
        let ndays = days_in_month(self.cal_year, self.cal_month);
        if day < 1 || day > ndays {
            return None;
        }
        let grid_x = area.x + CAL_PAD;
        let grid_w = area.w - CAL_PAD * 2;
        let col_w = grid_w / 7;
        let header_y = area.y + 48;
        let cell_h = ((area.h - 64) / 7).max(20);
        let cells_y = header_y + 22;
        let first_dow = weekday_of_first(self.cal_year, self.cal_month) as usize;
        let cell_index = first_dow + (day as usize - 1);
        let row = cell_index / 7;
        let col = cell_index % 7;
        Some(Rect {
            x: grid_x + col * col_w,
            y: cells_y + row * cell_h,
            w: col_w,
            h: cell_h,
        })
    }

    /// The number of content transport buttons in the current tab + their action
    /// for index `i`. Used by both `render` (draw) and `hit` (test).
    fn content_buttons(&self) -> &'static [(&'static str, Action)] {
        match self.tab {
            Tab::Timer => &[
                ("Start", Action::TimerStartPause),
                ("Reset", Action::TimerReset),
            ],
            Tab::Stopwatch => &[
                ("Start", Action::StopwatchStartStop),
                ("Lap", Action::StopwatchLap),
                ("Reset", Action::StopwatchReset),
            ],
            _ => &[],
        }
    }

    /// Hit-test a surface-local click. Order: close button, then a top tab, then
    /// per-tab content (Timer/Stopwatch transport buttons; Calendar arrows + day
    /// cells). Returns `Action::None` for empty space. Pure: builds the SAME rects
    /// the renderer draws.
    fn hit(&self, px: i32, py: i32) -> Action {
        if close_rect().contains(px, py) {
            return Action::Close;
        }
        // Top tabs.
        let ty = tab_btn_y();
        for (i, (tab, _label)) in TABS.iter().enumerate() {
            let r = Rect {
                x: tab_btn_x(i),
                y: ty,
                w: TAB_BTN_W,
                h: TAB_BTN_H,
            };
            if r.contains(px, py) {
                return Action::SwitchTab(*tab);
            }
        }
        // Content transport buttons (Timer/Stopwatch).
        let buttons = self.content_buttons();
        if !buttons.is_empty() {
            let area = content_rect();
            let by = content_btn_y();
            for (i, (_label, action)) in buttons.iter().enumerate() {
                let r = Rect {
                    x: content_btn_x(i, buttons.len(), area.w),
                    y: by,
                    w: TR_BTN_W,
                    h: TR_BTN_H,
                };
                if r.contains(px, py) {
                    return *action;
                }
            }
        }
        // Calendar arrows + day cells.
        if self.tab == Tab::Calendar {
            if cal_prev_rect().contains(px, py) {
                return Action::CalPrev;
            }
            if cal_next_rect().contains(px, py) {
                return Action::CalNext;
            }
            let ndays = days_in_month(self.cal_year, self.cal_month);
            for day in 1..=ndays {
                if let Some(r) = self.cal_day_rect(day) {
                    if r.contains(px, py) {
                        return Action::CalSelectDay(day);
                    }
                }
            }
        }
        Action::None
    }

    /// Apply an `Action` (shared by click dispatch + the hit-test proof). Returns
    /// true if anything changed (caller re-renders). `Close` exits. Each branch
    /// mirrors the matching key exactly.
    fn dispatch(&mut self, action: Action) -> bool {
        match action {
            Action::SwitchTab(t) => {
                let changed = self.tab != t;
                self.tab = t;
                if changed {
                    self.persist();
                }
                true
            }
            Action::TimerStartPause => {
                self.timer.start_pause();
                true
            }
            Action::TimerReset => {
                self.timer.reset();
                true
            }
            Action::StopwatchStartStop => {
                self.stopwatch.start_stop();
                true
            }
            Action::StopwatchLap => {
                self.stopwatch.lap();
                true
            }
            Action::StopwatchReset => {
                self.stopwatch.reset();
                true
            }
            Action::CalPrev => {
                self.cal_prev();
                true
            }
            Action::CalNext => {
                self.cal_next();
                true
            }
            Action::CalSelectDay(day) => {
                self.cal_selected_day = day;
                true
            }
            Action::Close => raekit::sys::exit(0),
            Action::None => false,
        }
    }
}

// ── Small formatting helpers (no_std, allocation-free) ────────────────────

/// Write a zero-padded 2-digit number into `out`, returns bytes written (2).
fn fmt_2(v: u32, out: &mut [u8]) -> usize {
    out[0] = b'0' + ((v / 10) % 10) as u8;
    out[1] = b'0' + (v % 10) as u8;
    2
}

fn fmt_u64(mut v: u64, out: &mut [u8]) -> usize {
    if v == 0 {
        out[0] = b'0';
        return 1;
    }
    let mut tmp = [0u8; 20];
    let mut i = 0;
    while v > 0 {
        tmp[i] = b'0' + (v % 10) as u8;
        v /= 10;
        i += 1;
    }
    let mut n = 0;
    while i > 0 {
        i -= 1;
        if n >= out.len() {
            break;
        }
        out[n] = tmp[i];
        n += 1;
    }
    n
}

const WEEKDAY_LONG: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];
const WEEKDAY_SHORT: [&str; 7] = ["Su", "Mo", "Tu", "We", "Th", "Fr", "Sa"];
const MONTH_LONG: [&str; 13] = [
    "",
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

/// Build "HH:MM:SS" into `out`, returns length (8).
fn fmt_hms(h: u32, m: u32, s: u32, out: &mut [u8]) -> usize {
    let mut n = 0;
    n += fmt_2(h, &mut out[n..]);
    out[n] = b':';
    n += 1;
    n += fmt_2(m, &mut out[n..]);
    out[n] = b':';
    n += 1;
    n += fmt_2(s, &mut out[n..]);
    n
}

/// Build "MM:SS" into `out` from a total-seconds count, returns length.
fn fmt_ms(total: u64, out: &mut [u8]) -> usize {
    let mm = (total / 60).min(99) as u32;
    let ss = (total % 60) as u32;
    let mut n = 0;
    n += fmt_2(mm, &mut out[n..]);
    out[n] = b':';
    n += 1;
    n += fmt_2(ss, &mut out[n..]);
    n
}

// ── A large clock-readout type style (px clamps to 256 in draw_text_aa) ───

const TYPE_CLOCK: TypeStyle = TypeStyle {
    px: 76,
    weight: 600,
    line_height: 84,
};

// ── Rendering: chrome ─────────────────────────────────────────────────────

fn render(app: &mut App, canvas: &mut Canvas) {
    canvas.fill_rect(0, 0, WIN_W, WIN_H, BG);

    // Title bar
    canvas.fill_rect_gradient(0, 0, WIN_W, TITLE_H, DARK.bg_elevated, TITLE_BG);
    canvas.draw_text_aa(
        12,
        ((TITLE_H.saturating_sub(rae_tokens::TYPE_SUBTITLE.line_height as usize)) / 2) as i32,
        "Clock",
        rae_tokens::TYPE_SUBTITLE,
        TEXT_FG,
        FontFamily::Sans,
    );
    canvas.fill_rounded_rect(
        WIN_W - 28,
        4,
        20,
        20,
        rae_tokens::RADIUS_XS as usize,
        DARK.state_danger,
    );
    let x_w = canvas.measure_text_aa("X", rae_tokens::TYPE_LABEL, FontFamily::Sans);
    canvas.draw_text_aa(
        (WIN_W - 18) as i32 - x_w / 2,
        (4 + (20 - rae_tokens::TYPE_LABEL.line_height as usize) / 2) as i32,
        "X",
        rae_tokens::TYPE_LABEL,
        0xFF_FF_FF_FF,
        FontFamily::Sans,
    );

    // Tab toolbar. Each tab draws at tab_btn_x(i)/tab_btn_y(), the SAME rect
    // `App::hit` tests, so the click target can't drift from the pixel.
    let tb_y = TITLE_H;
    canvas.fill_rect(0, tb_y, WIN_W, TOOLBAR_H, TOOLBAR_BG);
    let y = tab_btn_y();
    for (i, (tab, label)) in TABS.iter().enumerate() {
        let active = *tab == app.tab;
        let tx = tab_btn_x(i);
        let fill = if active { sel_fill() } else { DARK.bg_elevated };
        canvas.fill_rounded_rect(
            tx,
            y,
            TAB_BTN_W,
            TAB_BTN_H,
            rae_tokens::RADIUS_XS as usize,
            fill,
        );
        if active {
            canvas.fill_rect(tx, y + TAB_BTN_H - 2, TAB_BTN_W, 2, accent());
        }
        let lw = canvas.measure_text_aa(label, rae_tokens::TYPE_LABEL, FontFamily::Sans);
        let ty = (y + (TAB_BTN_H.saturating_sub(rae_tokens::TYPE_LABEL.line_height as usize)) / 2)
            as i32;
        canvas.draw_text_aa(
            tx as i32 + (TAB_BTN_W as i32 - lw) / 2,
            ty,
            label,
            rae_tokens::TYPE_LABEL,
            if active { TEXT_FG } else { TEXT_MUTED },
            FontFamily::Sans,
        );
    }

    let now_secs = wall_secs();
    let now = civil_from_unix(now_secs);
    app.ensure_calendar(now);

    let cx = 0usize;
    let cy = TITLE_H + TOOLBAR_H;
    let cw = WIN_W;
    let ch = WIN_H - cy - STATUS_H;

    match app.tab {
        Tab::Clock => render_clock(app, canvas, now_secs, now, cx, cy, cw, ch),
        Tab::Alarms => render_alarms(app, canvas, now, cx, cy, cw, ch),
        Tab::Timer => render_timer(app, canvas, cx, cy, cw, ch),
        Tab::Stopwatch => render_stopwatch(app, canvas, cx, cy, cw, ch),
        Tab::Calendar => render_calendar(app, canvas, now, cx, cy, cw, ch),
    }

    // Status bar (toast + hint).
    let st_y = WIN_H - STATUS_H;
    canvas.fill_rect(0, st_y, WIN_W, STATUS_H, STATUS_BG);
    let st_ty = (st_y
        + (STATUS_H.saturating_sub(rae_tokens::TYPE_CAPTION.line_height as usize)) / 2)
        as i32;
    if !app.toast_str().is_empty() {
        canvas.draw_text_aa(
            12,
            st_ty,
            app.toast_str(),
            rae_tokens::TYPE_CAPTION,
            accent(),
            FontFamily::Sans,
        );
    }
    let hint = match app.tab {
        Tab::Clock => "Tab/1-5:switch tabs   Esc:quit",
        Tab::Alarms => "Up/Down:select  0-9:HHMM  Enter:add  T:toggle  Del:remove",
        Tab::Timer => "0-9:MM:SS  Space:start/pause  R:reset",
        Tab::Stopwatch => "Space:start/stop  L:lap  R:reset",
        Tab::Calendar => "Left/Right:month   T:today   Esc:quit",
    };
    let hw = canvas.measure_text_aa(hint, rae_tokens::TYPE_CAPTION, FontFamily::Sans);
    canvas.draw_text_aa(
        (WIN_W - 12) as i32 - hw,
        st_ty,
        hint,
        rae_tokens::TYPE_CAPTION,
        TEXT_DIM,
        FontFamily::Sans,
    );
}

// ── Tab: Clock ──────────────────────────────────────────────────────────────

fn render_clock(
    app: &App,
    canvas: &mut Canvas,
    now_secs: u64,
    now: Civil,
    x: usize,
    y: usize,
    w: usize,
    h: usize,
) {
    let _ = app;
    canvas.fill_rect(x, y, w, h, BG);

    // Digital readout (or --:--:-- if the clock is unavailable).
    let mut buf = [0u8; 8];
    let txt: &str = if now_secs == 0 {
        "--:--:--"
    } else {
        fmt_hms(now.hour, now.min, now.sec, &mut buf);
        core::str::from_utf8(&buf).unwrap_or("--:--:--")
    };
    let tw = canvas.measure_text_aa(txt, TYPE_CLOCK, FontFamily::Sans);
    let read_y = (y + 40) as i32;
    canvas.draw_text_aa(
        ((w as i32) - tw) / 2,
        read_y,
        txt,
        TYPE_CLOCK,
        TEXT_FG,
        FontFamily::Sans,
    );

    // Long date line.
    let mut dbuf = [0u8; 64];
    let dn = fmt_long_date(now, &mut dbuf);
    if let Ok(ds) = core::str::from_utf8(&dbuf[..dn]) {
        let dw = canvas.measure_text_aa(ds, rae_tokens::TYPE_SUBTITLE, FontFamily::Sans);
        canvas.draw_text_aa(
            ((w as i32) - dw) / 2,
            read_y + TYPE_CLOCK.line_height as i32 + 6,
            if now_secs == 0 {
                "Clock unavailable"
            } else {
                ds
            },
            rae_tokens::TYPE_SUBTITLE,
            if now_secs == 0 { TEXT_DIM } else { accent() },
            FontFamily::Sans,
        );
    }

    // Analog face (drawn with circles + lines).
    let face_r = 84usize;
    let fcx = w / 2;
    let fcy = y + h - face_r - 24;
    if fcy > y + (TYPE_CLOCK.line_height as usize) + 60 {
        draw_analog_face(canvas, fcx, fcy, face_r, now);
    }
}

fn fmt_long_date(c: Civil, out: &mut [u8]) -> usize {
    // "Thursday, January 1, 1970"
    let wd = WEEKDAY_LONG[(c.weekday % 7) as usize];
    let mo = MONTH_LONG[(c.month.min(12)) as usize];
    let mut n = 0;
    let push = |out: &mut [u8], n: &mut usize, s: &str| {
        for &b in s.as_bytes() {
            if *n < out.len() {
                out[*n] = b;
                *n += 1;
            }
        }
    };
    push(out, &mut n, wd);
    push(out, &mut n, ", ");
    push(out, &mut n, mo);
    push(out, &mut n, " ");
    n += fmt_u64(c.day as u64, &mut out[n..]);
    push(out, &mut n, ", ");
    n += fmt_u64(c.year.max(0) as u64, &mut out[n..]);
    n
}

/// Draw a minimal analog clock: tick ring + hour/minute/second hands.
fn draw_analog_face(canvas: &mut Canvas, cx: usize, cy: usize, r: usize, now: Civil) {
    // Dial.
    canvas.fill_circle(cx, cy, r, PANEL_BG);
    // Ring (outline approximated by two concentric circles).
    canvas.fill_circle(cx, cy, r, STROKE_HL);
    canvas.fill_circle(cx, cy, r - 3, PANEL_BG);

    // Hour ticks (12 marks).
    for i in 0..12 {
        let ang = i as f32 * (PI / 6.0);
        let (sx, sy) = polar(cx, cy, ((r as f32) * 0.86) as usize, ang);
        let (ex, ey) = polar(cx, cy, ((r as f32) * 0.96) as usize, ang);
        canvas.draw_line(sx, sy, ex, ey, TEXT_MUTED);
    }

    let sec = now.sec as f32;
    let min = now.min as f32 + sec / 60.0;
    let hour = (now.hour % 12) as f32 + min / 60.0;

    // Hands: angle 0 = 12 o'clock (straight up), clockwise.
    let hour_ang = hour * (PI / 6.0); // 30° per hour
    let min_ang = min * (PI / 30.0); // 6° per minute
    let sec_ang = sec * (PI / 30.0); // 6° per second

    draw_hand(
        canvas,
        cx,
        cy,
        ((r as f32) * 0.50) as usize,
        hour_ang,
        TEXT_FG,
    );
    draw_hand(
        canvas,
        cx,
        cy,
        ((r as f32) * 0.74) as usize,
        min_ang,
        TEXT_FG,
    );
    draw_hand(
        canvas,
        cx,
        cy,
        ((r as f32) * 0.82) as usize,
        sec_ang,
        accent(),
    );

    // Hub.
    canvas.fill_circle(cx, cy, 4, accent());
}

// Minimal trig for the analog face (no_std — Taylor-series sin/cos, small range
// reduced into [0, 2π) so the 7-term series stays accurate enough for hands).
const PI: f32 = 3.14159265;
const TWO_PI: f32 = 6.2831853;

fn sin_approx(mut x: f32) -> f32 {
    // Range-reduce into [-PI, PI].
    while x > PI {
        x -= TWO_PI;
    }
    while x < -PI {
        x += TWO_PI;
    }
    let x2 = x * x;
    // 7th-order Taylor: x - x^3/6 + x^5/120 - x^7/5040
    x * (1.0 - x2 / 6.0 * (1.0 - x2 / 20.0 * (1.0 - x2 / 42.0)))
}

fn cos_approx(x: f32) -> f32 {
    sin_approx(x + PI / 2.0)
}

/// Point on a circle of radius `r` at clock-angle `ang` (0 = up, clockwise).
fn polar(cx: usize, cy: usize, r: usize, ang: f32) -> (i32, i32) {
    let dx = sin_approx(ang) * r as f32;
    let dy = -cos_approx(ang) * r as f32; // screen y grows downward
    (cx as i32 + dx as i32, cy as i32 + dy as i32)
}

fn draw_hand(canvas: &mut Canvas, cx: usize, cy: usize, len: usize, ang: f32, color: u32) {
    let (ex, ey) = polar(cx, cy, len, ang);
    canvas.draw_line(cx as i32, cy as i32, ex, ey, color);
}

// ── Tab: Alarms ──────────────────────────────────────────────────────────────

fn render_alarms(
    app: &App,
    canvas: &mut Canvas,
    now: Civil,
    x: usize,
    y: usize,
    w: usize,
    h: usize,
) {
    canvas.fill_rect(x, y, w, h, BG);
    let pad = 20usize;
    let ringing = app.ringing_alarm(now);

    canvas.draw_text_aa(
        (x + pad) as i32,
        (y + 12) as i32,
        "Alarms",
        rae_tokens::TYPE_TITLE,
        TEXT_FG,
        FontFamily::Sans,
    );

    let row_h = 40usize;
    let list_y = y + 56;
    if app.alarms.is_empty() {
        canvas.draw_text_aa(
            (x + pad) as i32,
            (list_y) as i32,
            "No alarms. Type HHMM then Enter to add one.",
            rae_tokens::TYPE_BODY,
            TEXT_MUTED,
            FontFamily::Sans,
        );
    }
    for (i, a) in app.alarms.iter().enumerate() {
        let ry = list_y + i * row_h;
        if ry + row_h > y + h {
            break;
        }
        let selected = i == app.alarm_sel;
        let is_ringing = ringing == Some(i);
        let fill = if is_ringing {
            DARK.state_warn
        } else if selected {
            sel_fill()
        } else {
            PANEL_BG
        };
        canvas.fill_rounded_rect(
            x + pad,
            ry,
            w - pad * 2,
            row_h - 8,
            rae_tokens::RADIUS_SM as usize,
            fill,
        );
        if selected {
            canvas.fill_rect(x + pad, ry, 3, row_h - 8, accent());
        }
        // HH:MM
        let mut tbuf = [0u8; 5];
        let mut n = fmt_2(a.hour, &mut tbuf);
        tbuf[n] = b':';
        n += 1;
        n += fmt_2(a.min, &mut tbuf[n..]);
        if let Ok(ts) = core::str::from_utf8(&tbuf[..n]) {
            canvas.draw_text_aa(
                (x + pad + 14) as i32,
                (ry + 4) as i32,
                ts,
                rae_tokens::TYPE_SUBTITLE,
                TEXT_FG,
                FontFamily::Sans,
            );
        }
        // Enabled pill / ringing label.
        let label = if is_ringing {
            "RINGING"
        } else if a.enabled {
            "On"
        } else {
            "Off"
        };
        let lw = canvas.measure_text_aa(label, rae_tokens::TYPE_LABEL, FontFamily::Sans);
        canvas.draw_text_aa(
            (x + w - pad - 16) as i32 - lw,
            (ry + 6) as i32,
            label,
            rae_tokens::TYPE_LABEL,
            if is_ringing {
                0xFF_FF_FF_FF
            } else if a.enabled {
                accent()
            } else {
                TEXT_DIM
            },
            FontFamily::Sans,
        );
    }

    // "Add" entry preview at the bottom.
    let entry_y = y + h - 30;
    let hh = (app.alarm_entry / 100).min(99);
    let mm = app.alarm_entry % 100;
    let mut ebuf = [0u8; 6];
    let mut en = fmt_2(hh, &mut ebuf);
    ebuf[en] = b':';
    en += 1;
    en += fmt_2(mm, &mut ebuf[en..]);
    let prefix = "New: ";
    let pw = canvas.draw_text_aa(
        (x + pad) as i32,
        entry_y as i32,
        prefix,
        rae_tokens::TYPE_BODY,
        TEXT_MUTED,
        FontFamily::Sans,
    );
    if let Ok(es) = core::str::from_utf8(&ebuf[..en]) {
        canvas.draw_text_aa(
            pw,
            entry_y as i32,
            es,
            rae_tokens::TYPE_BODY,
            accent(),
            FontFamily::Sans,
        );
    }
}

// ── Tab: Timer ──────────────────────────────────────────────────────────────

fn render_timer(app: &App, canvas: &mut Canvas, x: usize, y: usize, w: usize, h: usize) {
    canvas.fill_rect(x, y, w, h, BG);

    let remaining = app.timer.live_remaining();
    let mut buf = [0u8; 5];
    let n = fmt_ms(remaining, &mut buf);
    let txt = core::str::from_utf8(&buf[..n]).unwrap_or("00:00");
    let big = TypeStyle {
        px: 80,
        weight: 600,
        line_height: 88,
    };
    let tw = canvas.measure_text_aa(txt, big, FontFamily::Sans);
    let color = if app.timer.state == TimerState::Fired {
        DARK.state_warn
    } else {
        TEXT_FG
    };
    canvas.draw_text_aa(
        ((w as i32) - tw) / 2,
        (y + 60) as i32,
        txt,
        big,
        color,
        FontFamily::Sans,
    );

    // State line.
    let state_txt = match app.timer.state {
        TimerState::Idle => "Ready — type minutes/seconds, Space to start",
        TimerState::Running => "Running",
        TimerState::Paused => "Paused",
        TimerState::Fired => "",
    };
    let sw = canvas.measure_text_aa(state_txt, rae_tokens::TYPE_BODY, FontFamily::Sans);
    canvas.draw_text_aa(
        ((w as i32) - sw) / 2,
        (y + 60 + big.line_height as usize + 8) as i32,
        state_txt,
        rae_tokens::TYPE_BODY,
        TEXT_MUTED,
        FontFamily::Sans,
    );

    // Transport buttons (Start/Pause + Reset). Start is accented while running.
    let active = if app.timer.state == TimerState::Running {
        Some(0)
    } else {
        None
    };
    draw_content_buttons(app, canvas, w, active);

    // "Time's up" banner.
    if app.timer.state == TimerState::Fired {
        let banner = "Time's up!";
        let bw = canvas.measure_text_aa(banner, rae_tokens::TYPE_TITLE, FontFamily::Sans);
        let by = y + h - 70;
        canvas.fill_rounded_rect(
            (w - (bw as usize + 48)) / 2,
            by,
            bw as usize + 48,
            44,
            rae_tokens::RADIUS_MD as usize,
            DARK.state_warn,
        );
        canvas.draw_text_aa(
            ((w as i32) - bw) / 2,
            (by + 10) as i32,
            banner,
            rae_tokens::TYPE_TITLE,
            0xFF_FF_FF_FF,
            FontFamily::Sans,
        );
    }
}

// ── Tab: Stopwatch ───────────────────────────────────────────────────────────

fn render_stopwatch(app: &App, canvas: &mut Canvas, x: usize, y: usize, w: usize, h: usize) {
    canvas.fill_rect(x, y, w, h, BG);

    let elapsed = app.stopwatch.elapsed_ns();
    let mut buf = [0u8; 16];
    let n = fmt_stopwatch(elapsed, &mut buf);
    let txt = core::str::from_utf8(&buf[..n]).unwrap_or("00:00.00");
    let big = TypeStyle {
        px: 72,
        weight: 600,
        line_height: 80,
    };
    let tw = canvas.measure_text_aa(txt, big, FontFamily::Sans);
    canvas.draw_text_aa(
        ((w as i32) - tw) / 2,
        (y + 40) as i32,
        txt,
        big,
        if app.stopwatch.running {
            accent()
        } else {
            TEXT_FG
        },
        FontFamily::Sans,
    );

    // Lap list.
    let pad = 24usize;
    let mut ly = y + 40 + big.line_height as usize + 16;
    for (i, lap) in app.stopwatch.laps.iter().enumerate().rev().take(5).rev() {
        let mut lbuf = [0u8; 24];
        let mut ln = 0;
        let label = "Lap ";
        for &b in label.as_bytes() {
            lbuf[ln] = b;
            ln += 1;
        }
        ln += fmt_u64((i + 1) as u64, &mut lbuf[ln..]);
        lbuf[ln] = b':';
        ln += 1;
        lbuf[ln] = b' ';
        ln += 1;
        ln += fmt_stopwatch(*lap, &mut lbuf[ln..]);
        if let Ok(ls) = core::str::from_utf8(&lbuf[..ln]) {
            canvas.draw_text_aa(
                (x + pad) as i32,
                ly as i32,
                ls,
                rae_tokens::TYPE_BODY,
                TEXT_MUTED,
                FontFamily::Sans,
            );
        }
        ly += rae_tokens::TYPE_BODY.line_height as usize + 4;
        if ly + 20 > y + h {
            break;
        }
    }

    // Transport buttons (Start/Stop, Lap, Reset). Start is accented while running.
    let active = if app.stopwatch.running { Some(0) } else { None };
    draw_content_buttons(app, canvas, w, active);
}

/// Draw the current tab's content transport buttons (Timer/Stopwatch) at the
/// SAME rects `App::hit` tests. `active_idx` (if any) takes the accent fill (e.g.
/// the running Start/Pause). No-op for tabs without transport buttons.
fn draw_content_buttons(app: &App, canvas: &mut Canvas, area_w: usize, active_idx: Option<usize>) {
    let buttons = app.content_buttons();
    if buttons.is_empty() {
        return;
    }
    let by = content_btn_y();
    for (i, (label, _action)) in buttons.iter().enumerate() {
        let bx = content_btn_x(i, buttons.len(), area_w);
        let active = active_idx == Some(i);
        let fill = if active { sel_fill() } else { DARK.bg_elevated };
        canvas.fill_rounded_rect(
            bx,
            by,
            TR_BTN_W,
            TR_BTN_H,
            rae_tokens::RADIUS_XS as usize,
            fill,
        );
        let lw = canvas.measure_text_aa(label, rae_tokens::TYPE_LABEL, FontFamily::Sans);
        let ly = (by + (TR_BTN_H - rae_tokens::TYPE_LABEL.line_height as usize) / 2) as i32;
        canvas.draw_text_aa(
            bx as i32 + (TR_BTN_W as i32 - lw) / 2,
            ly,
            label,
            rae_tokens::TYPE_LABEL,
            if active { TEXT_FG } else { TEXT_MUTED },
            FontFamily::Sans,
        );
    }
}

/// "MM:SS.cc" (centiseconds) from a ns elapsed count.
fn fmt_stopwatch(ns: u64, out: &mut [u8]) -> usize {
    let total_cs = ns / 10_000_000; // centiseconds
    let cs = (total_cs % 100) as u32;
    let total_s = total_cs / 100;
    let ss = (total_s % 60) as u32;
    let mm = (total_s / 60).min(99) as u32;
    let mut n = 0;
    n += fmt_2(mm, &mut out[n..]);
    out[n] = b':';
    n += 1;
    n += fmt_2(ss, &mut out[n..]);
    out[n] = b'.';
    n += 1;
    n += fmt_2(cs, &mut out[n..]);
    n
}

// ── Tab: Calendar ────────────────────────────────────────────────────────────

fn render_calendar(
    app: &App,
    canvas: &mut Canvas,
    now: Civil,
    x: usize,
    y: usize,
    w: usize,
    h: usize,
) {
    canvas.fill_rect(x, y, w, h, BG);
    let pad = 24usize;

    // Month header: "January 1970"  with the year.
    let mo = MONTH_LONG[app.cal_month.min(12) as usize];
    let mut hbuf = [0u8; 32];
    let mut hn = 0;
    for &b in mo.as_bytes() {
        hbuf[hn] = b;
        hn += 1;
    }
    hbuf[hn] = b' ';
    hn += 1;
    hn += fmt_u64(app.cal_year.max(0) as u64, &mut hbuf[hn..]);
    if let Ok(hs) = core::str::from_utf8(&hbuf[..hn]) {
        let hw = canvas.measure_text_aa(hs, rae_tokens::TYPE_TITLE, FontFamily::Sans);
        canvas.draw_text_aa(
            ((w as i32) - hw) / 2,
            (y + 8) as i32,
            hs,
            rae_tokens::TYPE_TITLE,
            TEXT_FG,
            FontFamily::Sans,
        );
    }
    // Prev/Next chevrons.
    canvas.draw_text_aa(
        (x + pad) as i32,
        (y + 12) as i32,
        "<",
        rae_tokens::TYPE_TITLE,
        accent(),
        FontFamily::Sans,
    );
    let rw = canvas.measure_text_aa(">", rae_tokens::TYPE_TITLE, FontFamily::Sans);
    canvas.draw_text_aa(
        (x + w - pad) as i32 - rw,
        (y + 12) as i32,
        ">",
        rae_tokens::TYPE_TITLE,
        accent(),
        FontFamily::Sans,
    );

    // Grid metrics.
    let grid_x = x + pad;
    let grid_w = w - pad * 2;
    let col_w = grid_w / 7;
    let header_y = y + 48;
    let cell_h = ((h - 64) / 7).max(20);

    // Weekday header row.
    for (i, wd) in WEEKDAY_SHORT.iter().enumerate() {
        let cellx = grid_x + i * col_w;
        let ww = canvas.measure_text_aa(wd, rae_tokens::TYPE_LABEL, FontFamily::Sans);
        canvas.draw_text_aa(
            (cellx + (col_w - ww as usize) / 2) as i32,
            header_y as i32,
            wd,
            rae_tokens::TYPE_LABEL,
            TEXT_MUTED,
            FontFamily::Sans,
        );
    }

    let first_dow = weekday_of_first(app.cal_year, app.cal_month) as usize;
    let ndays = days_in_month(app.cal_year, app.cal_month);
    let is_current_month = now.year == app.cal_year && now.month == app.cal_month;

    let cells_y = header_y + 22;
    for day in 1..=ndays {
        let cell_index = first_dow + (day as usize - 1);
        let row = cell_index / 7;
        let col = cell_index % 7;
        let cellx = grid_x + col * col_w;
        let celly = cells_y + row * cell_h;
        if celly + cell_h > y + h {
            break;
        }

        let is_today = is_current_month && now.day == day;
        let is_selected = app.cal_selected_day == day;
        if is_today {
            // Accent ring: an accent disc with the selection fill punched into it.
            let r = (col_w.min(cell_h).saturating_sub(8)) / 2;
            let ccx = cellx + col_w / 2;
            let ccy = celly + cell_h / 2;
            canvas.fill_circle(ccx, ccy, r, accent());
            canvas.fill_circle(ccx, ccy, r.saturating_sub(2), sel_fill());
        } else if is_selected {
            // Clicked day: a filled selection-tint cell with an accent outline.
            canvas.fill_rounded_rect(
                cellx + 2,
                celly + 2,
                col_w.saturating_sub(4),
                cell_h.saturating_sub(4),
                rae_tokens::RADIUS_XS as usize,
                sel_fill(),
            );
            canvas.draw_rect_outline(
                cellx + 2,
                celly + 2,
                col_w.saturating_sub(4),
                cell_h.saturating_sub(4),
                accent(),
            );
        }

        let mut dbuf = [0u8; 2];
        let dn = fmt_u64(day as u64, &mut dbuf);
        if let Ok(ds) = core::str::from_utf8(&dbuf[..dn]) {
            let dw = canvas.measure_text_aa(ds, rae_tokens::TYPE_BODY, FontFamily::Sans);
            canvas.draw_text_aa(
                (cellx + (col_w - dw as usize) / 2) as i32,
                (celly + cell_h / 2 - rae_tokens::TYPE_BODY.line_height as usize / 2) as i32,
                ds,
                rae_tokens::TYPE_BODY,
                if is_today || is_selected {
                    TEXT_FG
                } else {
                    TEXT_MUTED
                },
                FontFamily::Sans,
            );
        }
    }
}

// ── Scancode digits (shared layout with the other apps) ───────────────────

/// Map a scancode (make code) to a 0-9 digit, or None.
fn scancode_digit(code: u8) -> Option<u64> {
    match code {
        0x02 => Some(1),
        0x03 => Some(2),
        0x04 => Some(3),
        0x05 => Some(4),
        0x06 => Some(5),
        0x07 => Some(6),
        0x08 => Some(7),
        0x09 => Some(8),
        0x0A => Some(9),
        0x0B => Some(0),
        _ => None,
    }
}

// ── Design proof (R10: fail-able date-math + token wiring gate) ────────────

/// True iff Clock's chrome is wired to the shared design tokens AND the pure
/// civil-date math is correct. Deliberately fail-able: a regression in the leap
/// rule, the day-of-week computation, or the month-grid offset flips this to
/// `false` (exit code 3 at startup). An ELF bin can't run `cargo test`, so this
/// is the date-math proof the app carries.
#[must_use]
pub fn design_proof() -> bool {
    let ramp = rae_tokens::derive_accent(theme_seed(), &DARK);
    let tokens_ok = accent() == ramp.base
        && sel_fill() == ramp.active
        && BG == DARK.bg_raised
        && TITLE_BG == DARK.bg_base
        && TOOLBAR_BG == DARK.bg_overlay
        && TEXT_FG == DARK.text_primary
        && TEXT_MUTED == DARK.text_secondary
        && TEXT_DIM == DARK.text_tertiary
        && STROKE_HL == DARK.stroke_strong
        && raekit::sys::THEME_DEFAULT_ACCENT == RAEBLUE;

    tokens_ok && date_math_ok() && hit_test_proof() && prefs_round_trip_ok()
}

/// Prove the Clock PREFS SCHEMA: a known non-default `Prefs` serialized via
/// `rae_toml` then re-parsed restores the active tab AND the full ALARM LIST
/// (count + each HH:MM + enabled) exactly, AND a corrupt / missing-key document
/// resolves to the typed defaults (NOT a panic, NOT a wrong value). The alarm-list
/// round-trip is the load-bearing assertion: alarms that don't survive a relaunch
/// are useless. Returns `false` on any drift (→ exit(3) at startup).
#[must_use]
fn prefs_round_trip_ok() -> bool {
    // (a) Full round-trip of a non-default Prefs with three distinct alarms.
    let mut alarms = Vec::new();
    alarms.push(Alarm {
        hour: 6,
        min: 15,
        enabled: true,
    });
    alarms.push(Alarm {
        hour: 13,
        min: 0,
        enabled: false,
    });
    alarms.push(Alarm {
        hour: 23,
        min: 59,
        enabled: true,
    });
    let p = Prefs {
        tab: Tab::Stopwatch,
        alarms,
    };
    let text = rae_toml::to_string(&p.to_toml());
    let parsed = match rae_toml::parse(&text) {
        Ok(t) => t,
        Err(_) => return false,
    };
    let back = Prefs::from_toml(&parsed);
    if back.tab != Tab::Stopwatch {
        return false;
    }
    // The alarm LIST round-trips: same count, and each HH:MM + enabled preserved.
    if back.alarms.len() != 3 {
        return false;
    }
    let expect = [(6u32, 15u32, true), (13, 0, false), (23, 59, true)];
    for (a, &(h, m, en)) in back.alarms.iter().zip(expect.iter()) {
        if a.hour != h || a.min != m || a.enabled != en {
            return false;
        }
    }

    // (b) Tab token round-trips through its stable string form.
    for t in [
        Tab::Clock,
        Tab::Alarms,
        Tab::Timer,
        Tab::Stopwatch,
        Tab::Calendar,
    ] {
        if Tab::from_token(t.as_token()) != t {
            return false;
        }
    }

    // (c) A corrupt document → typed defaults (parse FAILS, we don't panic). The
    // defaults restore the two seeded alarms + the Clock tab.
    let corrupt = "tab = = oops\n[[unterminated\n";
    let d = match rae_toml::parse(corrupt) {
        Ok(t) => Prefs::from_toml(&t), // shouldn't reach here for this input
        Err(_) => Prefs::defaults(),
    };
    if d.tab != Tab::Clock || d.alarms.len() != 2 {
        return false;
    }

    // (d) A well-formed doc MISSING every prefs key → typed defaults (Clock tab +
    // the two seeded alarms).
    let empty = match rae_toml::parse("unrelated = 1\n") {
        Ok(t) => t,
        Err(_) => return false,
    };
    let e = Prefs::from_toml(&empty);
    if e.tab != Tab::Clock || e.alarms.len() != 2 {
        return false;
    }

    // (e) An out-of-range alarm time is CLAMPED, not rejected, and a malformed
    // entry (missing `min`) is SKIPPED — never a panic, never the whole-file drop.
    let oor = match rae_toml::parse(
        "tab = \"bogus\"\n[[alarm]]\nhour = 99\nmin = 88\nenabled = true\n[[alarm]]\nhour = 5\n",
    ) {
        Ok(t) => t,
        Err(_) => return false,
    };
    let c = Prefs::from_toml(&oor);
    // Bad tab token → default Clock. The first alarm clamps to 23:59; the second
    // (no `min`) is skipped, so exactly one alarm survives.
    if c.tab != Tab::Clock || c.alarms.len() != 1 {
        return false;
    }
    if c.alarms[0].hour != 23 || c.alarms[0].min != 59 || !c.alarms[0].enabled {
        return false;
    }

    // (f) An explicit EMPTY alarm array yields an empty list (distinct from the
    // missing-key default that restores the seeds).
    let emptyarr = match rae_toml::parse("alarm = []\n") {
        Ok(t) => t,
        Err(_) => return false,
    };
    if !Prefs::from_toml(&emptyarr).alarms.is_empty() {
        return false;
    }

    true
}

/// Prove the mouse hit-test invariant: a click on a known element's rect-center
/// resolves to that element's action (the SAME rects `render` draws), and an
/// out-of-bounds click resolves to `Action::None`. Returns `false` on any drift
/// (→ exit(3) at startup). `App::new` is pure (no file I/O), so the proof drives
/// real app state directly.
#[must_use]
fn hit_test_proof() -> bool {
    // (1) Each top tab's center hits SwitchTab(that tab).
    let app = App::new();
    let ty = tab_btn_y();
    for (i, (tab, _label)) in TABS.iter().enumerate() {
        let cx = (tab_btn_x(i) + TAB_BTN_W / 2) as i32;
        let yc = (ty + TAB_BTN_H / 2) as i32;
        if app.hit(cx, yc) != Action::SwitchTab(*tab) {
            return false;
        }
    }

    // (2) A click on the Calendar tab switches to Calendar (dispatch).
    let cal_idx = TABS.iter().position(|(t, _)| *t == Tab::Calendar);
    let Some(ci) = cal_idx else { return false };
    let mut app2 = App::new();
    let ccx = (tab_btn_x(ci) + TAB_BTN_W / 2) as i32;
    let ccy = (tab_btn_y() + TAB_BTN_H / 2) as i32;
    if !app2.dispatch(app2.hit(ccx, ccy)) || app2.tab != Tab::Calendar {
        return false;
    }

    // (3) Timer transport buttons map correctly.
    let mut tapp = App::new();
    tapp.tab = Tab::Timer;
    let area_w = WIN_W;
    let by = content_btn_y();
    let tb = tapp.content_buttons();
    if tb.len() != 2 || tb[0].1 != Action::TimerStartPause || tb[1].1 != Action::TimerReset {
        return false;
    }
    let sp_x = (content_btn_x(0, 2, area_w) + TR_BTN_W / 2) as i32;
    let sp_y = (by + TR_BTN_H / 2) as i32;
    if tapp.hit(sp_x, sp_y) != Action::TimerStartPause {
        return false;
    }

    // (4) Stopwatch transport: 3 buttons, Lap is the middle one.
    let mut sapp = App::new();
    sapp.tab = Tab::Stopwatch;
    let lap_x = (content_btn_x(1, 3, area_w) + TR_BTN_W / 2) as i32;
    if sapp.hit(lap_x, sp_y) != Action::StopwatchLap {
        return false;
    }

    // (5) Calendar: known month (Feb 2024 = 29 days, starts Thursday). The prev/
    // next arrows + day-1 cell map correctly, and selecting day 15 sets state.
    let mut capp = App::new();
    capp.tab = Tab::Calendar;
    capp.cal_year = 2024;
    capp.cal_month = 2;
    capp.cal_initialized = true;
    let pr = cal_prev_rect();
    if capp.hit((pr.x + pr.w / 2) as i32, (pr.y + pr.h / 2) as i32) != Action::CalPrev {
        return false;
    }
    let nr = cal_next_rect();
    if capp.hit((nr.x + nr.w / 2) as i32, (nr.y + nr.h / 2) as i32) != Action::CalNext {
        return false;
    }
    let d15 = match capp.cal_day_rect(15) {
        Some(r) => r,
        None => return false,
    };
    if capp.hit((d15.x + d15.w / 2) as i32, (d15.y + d15.h / 2) as i32) != Action::CalSelectDay(15)
    {
        return false;
    }
    let _ = capp.dispatch(Action::CalSelectDay(15));
    if capp.cal_selected_day != 15 {
        return false;
    }

    // (6) Out-of-bounds clicks resolve to None on every tab.
    if app.hit(-100, -100) != Action::None {
        return false;
    }
    if capp.hit(WIN_W as i32 + 50, WIN_H as i32 + 50) != Action::None {
        return false;
    }
    true
}

/// Assert the civil-date math: leap-year February, century rules, a known
/// epoch-date decode, the day-of-week of two known dates, and the calendar
/// first-cell offset. Returns `false` on any drift.
fn date_math_ok() -> bool {
    // Leap rule: 2024 (div 4) = 29; 2023 = 28; 2000 (div 400) = 29; 1900 = 28.
    if days_in_month(2024, 2) != 29
        || days_in_month(2023, 2) != 28
        || days_in_month(2000, 2) != 29
        || days_in_month(1900, 2) != 28
    {
        return false;
    }
    // Month lengths.
    if days_in_month(2023, 4) != 30 || days_in_month(2023, 12) != 31 {
        return false;
    }

    // The Unix epoch: 1970-01-01 00:00:00 UTC, a Thursday (weekday 4).
    let e = civil_from_unix(0);
    if e.year != 1970 || e.month != 1 || e.day != 1 || e.hour != 0 || e.weekday != 4 {
        return false;
    }
    // 2024-02-29 12:34:56 UTC = 1709210096 seconds (a known leap day, Thursday).
    let leap = civil_from_unix(1_709_210_096);
    if leap.year != 2024
        || leap.month != 2
        || leap.day != 29
        || leap.hour != 12
        || leap.min != 34
        || leap.sec != 56
        || leap.weekday != 4
    {
        return false;
    }

    // weekday_of_first cross-check (independent days_from_civil path):
    //   1970-01-01 = Thursday (4); 2024-02-01 = Thursday (4); 2000-01-01 = Saturday (6).
    if weekday_of_first(1970, 1) != 4
        || weekday_of_first(2024, 2) != 4
        || weekday_of_first(2000, 1) != 6
    {
        return false;
    }

    // Calendar first-cell offset: Feb 2024 starts on Thursday (col 4) and has 29
    // days, so the day-1 cell sits at index 4 and day-29 at index 4+28 = 32
    // (row 4, col 4).
    let first = weekday_of_first(2024, 2) as usize;
    if first != 4 {
        return false;
    }
    let last_index = first + (days_in_month(2024, 2) as usize - 1);
    if last_index != 32 || last_index / 7 != 4 || last_index % 7 != 4 {
        return false;
    }

    // Round-trip days_from_civil ⇄ civil_from_unix on the leap day.
    let serial = days_from_civil(2024, 2, 29);
    let back = civil_from_unix((serial as u64) * 86_400);
    if back.year != 2024 || back.month != 2 || back.day != 29 {
        return false;
    }

    true
}

// ── Entry point ───────────────────────────────────────────────────────────

#[no_mangle]
pub extern "C" fn _start() -> ! {
    if !design_proof() {
        raekit::sys::exit(3);
    }
    let sid = raekit::sys::surface_create(WIN_W as u64, WIN_H as u64, SURFACE_VIRT);
    if sid == u64::MAX {
        raekit::sys::exit(1);
    }

    let mut canvas = unsafe { Canvas::new(SURFACE_VIRT as *mut u8, WIN_W, WIN_H, 4) };

    let mut app = App::new();
    render(&mut app, &mut canvas);
    raekit::sys::surface_present(sid, PRESENT_X as u64, PRESENT_Y as u64);
    app.last_secs = wall_secs();

    let mut extended = false;
    let mut left_was_down = false;

    loop {
        // ── Mouse: drain button events, hit-test the cursor on a click edge ──
        let mut mouse_activity = false;
        let mut left_down = left_was_down;
        loop {
            let ev = raekit::sys::poll_mouse();
            if ev == 0 {
                break;
            }
            left_down = (ev & 0x01) != 0;
            mouse_activity = true;
        }
        if mouse_activity || left_down != left_was_down {
            if left_down && !left_was_down {
                let (cx, cy, _btn) = raekit::sys::cursor_pos();
                // Subtract the LIVE window origin (not the stale present-time
                // PRESENT_X/Y) so clicks land correctly after the window manager
                // moves the window (Overview / Spaces / tiling). Falls back to the
                // present origin if the surface isn't found. Saturating-sub keeps a
                // cursor above/left of the window from underflowing.
                let (ox, oy) = raekit::sys::surface_origin(sid)
                    .unwrap_or((PRESENT_X as u32, PRESENT_Y as u32));
                let lx = (cx as i32).saturating_sub(ox as i32);
                let ly = (cy as i32).saturating_sub(oy as i32);
                if app.dispatch(app.hit(lx, ly)) {
                    app.toast_len = 0;
                    render(&mut app, &mut canvas);
                    raekit::sys::surface_present(sid, PRESENT_X as u64, PRESENT_Y as u64);
                    app.last_secs = wall_secs();
                }
            }
            left_was_down = left_down;
        }

        let key = raekit::sys::read_key();
        if key == 0 {
            // Idle: tick the clock once per second + run the timer state machine.
            let mut need_render = false;
            if app.timer.poll() {
                app.set_toast("Timer finished");
                need_render = true;
            }
            let secs = wall_secs();
            if secs != app.last_secs {
                app.last_secs = secs;
                need_render = true;
            } else if app.tab == Tab::Stopwatch && app.stopwatch.running {
                // Stopwatch wants sub-second refresh while running.
                need_render = true;
            } else if app.tab == Tab::Timer && app.timer.state == TimerState::Running {
                need_render = true;
            }
            if need_render {
                render(&mut app, &mut canvas);
                raekit::sys::surface_present(sid, PRESENT_X as u64, PRESENT_Y as u64);
            }
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

        app.toast_len = 0;
        // Snapshot the tab so we can persist a tab change driven by the keyboard
        // (Tab cycle / 1-5 jumps) without threading a return flag through every arm.
        let prev_tab = app.tab;

        // Esc always quits.
        if !ext && code == 0x01 {
            raekit::sys::exit(0);
        }

        // Tab cycles forward; 1-5 jump to a tab (when not consumed by a tab).
        if !ext && code == 0x0F {
            app.tab = match app.tab {
                Tab::Clock => Tab::Alarms,
                Tab::Alarms => Tab::Timer,
                Tab::Timer => Tab::Stopwatch,
                Tab::Stopwatch => Tab::Calendar,
                Tab::Calendar => Tab::Clock,
            };
        } else {
            match app.tab {
                Tab::Clock => {
                    handle_tab_keys(&mut app, ext, code);
                }
                Tab::Calendar => match (ext, code) {
                    (true, 0x4B) => app.cal_prev(), // Left
                    (true, 0x4D) => app.cal_next(), // Right
                    (false, 0x14) => {
                        // 'T' = jump to today
                        app.cal_initialized = false;
                    }
                    _ => {
                        handle_tab_keys(&mut app, ext, code);
                    }
                },
                Tab::Alarms => match (ext, code) {
                    (true, 0x48) => {
                        if app.alarm_sel > 0 {
                            app.alarm_sel -= 1;
                        }
                    }
                    (true, 0x50) => {
                        if app.alarm_sel + 1 < app.alarms.len() {
                            app.alarm_sel += 1;
                        }
                    }
                    (false, 0x1C) => app.add_alarm_from_entry(), // Enter
                    (false, 0x14) => app.toggle_alarm(),         // 'T' toggle
                    (true, 0x53) => app.remove_alarm(),          // Delete
                    (false, 0x0E) => app.alarm_entry /= 10,      // Backspace
                    _ => {
                        if let Some(d) = scancode_digit(code) {
                            if app.alarm_entry < 1000 {
                                app.alarm_entry = (app.alarm_entry * 10 + d as u32) % 10000;
                            }
                        } else {
                            handle_tab_keys(&mut app, ext, code);
                        }
                    }
                },
                Tab::Timer => match (ext, code) {
                    (false, 0x39) => app.timer.start_pause(), // Space
                    (false, 0x13) => app.timer.reset(),       // 'R'
                    (false, 0x0E) => {
                        app.timer.entry /= 10;
                        let mm = app.timer.entry / 100;
                        let ss = app.timer.entry % 100;
                        app.timer.set_secs = mm * 60 + ss.min(59);
                        app.timer.remaining = app.timer.set_secs;
                    } // Backspace
                    _ => {
                        if let Some(d) = scancode_digit(code) {
                            app.timer.push_digit(d);
                        } else {
                            handle_tab_keys(&mut app, ext, code);
                        }
                    }
                },
                Tab::Stopwatch => match (ext, code) {
                    (false, 0x39) => app.stopwatch.start_stop(), // Space
                    (false, 0x26) => app.stopwatch.lap(),        // 'L'
                    (false, 0x13) => app.stopwatch.reset(),      // 'R'
                    _ => {
                        handle_tab_keys(&mut app, ext, code);
                    }
                },
            }
        }

        // Persist a keyboard-driven tab change (the dispatch/alarm paths persist
        // their own changes; this covers Tab-cycle and the 1-5 jump keys).
        if app.tab != prev_tab {
            app.persist();
        }

        // Any handled keypress repaints (every branch above mutates view state).
        render(&mut app, &mut canvas);
        raekit::sys::surface_present(sid, PRESENT_X as u64, PRESENT_Y as u64);
        app.last_secs = wall_secs();
    }
}

/// Number-row 1..5 jump directly to the matching tab. Shared across tabs that
/// don't otherwise consume those scancodes (Clock/Stopwatch/Calendar always;
/// Alarms/Timer only when not in digit entry — handled by the caller).
fn handle_tab_keys(app: &mut App, ext: bool, code: u8) {
    if ext {
        return;
    }
    match code {
        0x02 => app.tab = Tab::Clock,
        0x03 => app.tab = Tab::Alarms,
        0x04 => app.tab = Tab::Timer,
        0x05 => app.tab = Tab::Stopwatch,
        0x06 => app.tab = Tab::Calendar,
        _ => {}
    }
}

// FOLLOW-UP: register Clock tile in raeshell/start_menu (the concurrent opus
// session owns raeshell/start_menu — this slice deliberately does NOT edit it).
// Once that session lands, add a "Clock" entry with exec_path = "clock" so the
// app is launchable from the Start menu, not only via the initramfs bundle.
