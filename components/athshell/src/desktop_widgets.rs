//! Desktop Widget System — Rainmeter-style, sandboxed widgets for the desktop.
//!
//! Widgets are self-contained UI elements pinned to the desktop layer.
//! Each widget runs in a sandbox with limited capabilities (no filesystem,
//! no network unless explicitly granted).
//!
//! Built-in widgets: Clock, Weather, SystemMonitor, NowPlaying, Calendar,
//! QuickNotes.  Third-party widgets are loaded as declarative bundles.

#![allow(dead_code)]

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

// ── Geometry ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WidgetRect {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

impl WidgetRect {
    pub const fn new(x: i32, y: i32, w: u32, h: u32) -> Self {
        Self { x, y, w, h }
    }

    pub fn contains(&self, px: i32, py: i32) -> bool {
        px >= self.x && px < self.x + self.w as i32 && py >= self.y && py < self.y + self.h as i32
    }
}

// ── Sandbox capabilities ─────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WidgetCapability {
    ReadTime,
    ReadSystemStats,
    ReadAudioState,
    ReadNetwork,
    ReadCalendar,
    ReadWeather,
    WriteNotes,
    CustomDraw,
}

#[derive(Debug, Clone)]
pub struct WidgetSandbox {
    pub capabilities: Vec<WidgetCapability>,
    pub memory_limit_kb: u32,
    pub cpu_time_limit_us: u32,
}

impl WidgetSandbox {
    pub fn minimal() -> Self {
        Self {
            capabilities: Vec::new(),
            memory_limit_kb: 256,
            cpu_time_limit_us: 1000,
        }
    }

    pub fn with_cap(mut self, cap: WidgetCapability) -> Self {
        if !self.capabilities.contains(&cap) {
            self.capabilities.push(cap);
        }
        self
    }

    pub fn has_cap(&self, cap: WidgetCapability) -> bool {
        self.capabilities.contains(&cap)
    }
}

// ── Click target ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClickAction {
    None,
    Toggle,
    NextPage,
    PrevPage,
    OpenSettings,
    LaunchApp,
    Custom(u32),
}

#[derive(Debug, Clone, Copy)]
pub struct ClickZone {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
    pub action: ClickAction,
}

impl ClickZone {
    pub fn contains(&self, px: i32, py: i32) -> bool {
        px >= self.x && px < self.x + self.w as i32 && py >= self.y && py < self.y + self.h as i32
    }
}

// ── Draw commands (widget renders via commands, not raw canvas access) ───

#[derive(Debug, Clone)]
pub enum DrawCommand {
    FillRect {
        x: i32,
        y: i32,
        w: u32,
        h: u32,
        color: u32,
    },
    DrawRect {
        x: i32,
        y: i32,
        w: u32,
        h: u32,
        color: u32,
    },
    DrawText {
        x: i32,
        y: i32,
        text: String,
        color: u32,
        size: u8,
    },
    DrawGlyph {
        x: i32,
        y: i32,
        ch: char,
        color: u32,
    },
    DrawLine {
        x1: i32,
        y1: i32,
        x2: i32,
        y2: i32,
        color: u32,
    },
    DrawCircle {
        cx: i32,
        cy: i32,
        r: u32,
        color: u32,
        filled: bool,
    },
    DrawArc {
        cx: i32,
        cy: i32,
        r: u32,
        start_deg: i16,
        end_deg: i16,
        color: u32,
    },
    SetOpacity {
        alpha: u8,
    },
}

#[derive(Debug, Clone)]
pub struct WidgetRenderOutput {
    pub commands: Vec<DrawCommand>,
    pub click_zones: Vec<ClickZone>,
}

impl WidgetRenderOutput {
    pub fn new() -> Self {
        Self {
            commands: Vec::new(),
            click_zones: Vec::new(),
        }
    }

    pub fn fill_rect(&mut self, x: i32, y: i32, w: u32, h: u32, color: u32) {
        self.commands
            .push(DrawCommand::FillRect { x, y, w, h, color });
    }

    pub fn draw_rect(&mut self, x: i32, y: i32, w: u32, h: u32, color: u32) {
        self.commands
            .push(DrawCommand::DrawRect { x, y, w, h, color });
    }

    pub fn draw_text(&mut self, x: i32, y: i32, text: &str, color: u32, size: u8) {
        self.commands.push(DrawCommand::DrawText {
            x,
            y,
            text: String::from(text),
            color,
            size,
        });
    }

    pub fn draw_glyph(&mut self, x: i32, y: i32, ch: char, color: u32) {
        self.commands
            .push(DrawCommand::DrawGlyph { x, y, ch, color });
    }

    pub fn draw_line(&mut self, x1: i32, y1: i32, x2: i32, y2: i32, color: u32) {
        self.commands.push(DrawCommand::DrawLine {
            x1,
            y1,
            x2,
            y2,
            color,
        });
    }

    pub fn add_click_zone(&mut self, x: i32, y: i32, w: u32, h: u32, action: ClickAction) {
        self.click_zones.push(ClickZone { x, y, w, h, action });
    }
}

// ── DesktopWidget trait ──────────────────────────────────────────────────

pub trait DesktopWidget {
    fn name(&self) -> &str;
    fn preferred_size(&self) -> (u32, u32);
    fn render(&self, output: &mut WidgetRenderOutput);
    fn update(&mut self, ctx: &WidgetContext);
    fn on_click(&mut self, action: ClickAction);
    fn sandbox(&self) -> &WidgetSandbox;
}

// ── Widget context (data from the OS) ────────────────────────────────────

#[derive(Debug, Clone)]
pub struct SystemStats {
    pub cpu_usage_pct: u8,
    pub ram_used_mb: u32,
    pub ram_total_mb: u32,
    pub gpu_usage_pct: u8,
    pub gpu_temp_c: u8,
    pub cpu_temp_c: u8,
    pub disk_used_gb: u32,
    pub disk_total_gb: u32,
    pub net_down_kbps: u32,
    pub net_up_kbps: u32,
}

impl SystemStats {
    pub fn zero() -> Self {
        Self {
            cpu_usage_pct: 0,
            ram_used_mb: 0,
            ram_total_mb: 0,
            gpu_usage_pct: 0,
            gpu_temp_c: 0,
            cpu_temp_c: 0,
            disk_used_gb: 0,
            disk_total_gb: 0,
            net_down_kbps: 0,
            net_up_kbps: 0,
        }
    }

    pub fn ram_pct(&self) -> u8 {
        if self.ram_total_mb == 0 {
            0
        } else {
            ((self.ram_used_mb as u64 * 100) / self.ram_total_mb as u64) as u8
        }
    }

    pub fn disk_pct(&self) -> u8 {
        if self.disk_total_gb == 0 {
            0
        } else {
            ((self.disk_used_gb as u64 * 100) / self.disk_total_gb as u64) as u8
        }
    }
}

#[derive(Debug, Clone)]
pub struct AudioState {
    pub playing: bool,
    pub track_title: String,
    pub artist: String,
    pub album: String,
    pub progress_pct: u8,
    pub volume_pct: u8,
}

impl AudioState {
    pub fn idle() -> Self {
        Self {
            playing: false,
            track_title: String::new(),
            artist: String::new(),
            album: String::new(),
            progress_pct: 0,
            volume_pct: 75,
        }
    }
}

#[derive(Debug, Clone)]
pub struct WeatherData {
    pub temp_c: i8,
    pub feels_like: i8,
    pub humidity_pct: u8,
    pub condition: String,
    pub icon: char,
    pub wind_kph: u8,
    pub location: String,
}

impl WeatherData {
    pub fn unknown() -> Self {
        Self {
            temp_c: 0,
            feels_like: 0,
            humidity_pct: 0,
            condition: String::from("Unknown"),
            icon: '?',
            wind_kph: 0,
            location: String::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct CalendarEvent {
    pub title: String,
    pub hour: u8,
    pub minute: u8,
    pub duration_min: u16,
}

#[derive(Debug, Clone)]
pub struct WidgetContext {
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
    pub day: u8,
    pub month: u8,
    pub year: u16,
    pub day_of_week: u8,
    pub stats: SystemStats,
    pub audio: AudioState,
    pub weather: WeatherData,
    pub events: Vec<CalendarEvent>,
    pub accent_color: u32,
    pub bg_color: u32,
    pub text_color: u32,
}

impl WidgetContext {
    pub fn default_ctx() -> Self {
        Self {
            hour: 0,
            minute: 0,
            second: 0,
            day: 1,
            month: 1,
            year: 2026,
            day_of_week: 0,
            stats: SystemStats::zero(),
            audio: AudioState::idle(),
            weather: WeatherData::unknown(),
            events: Vec::new(),
            accent_color: 0xFF_4E_9C_FF,
            bg_color: 0xFF_0A_0E_1A,
            text_color: 0xFF_E0_E0_FF,
        }
    }
}

// ── Built-in: Clock Widget ───────────────────────────────────────────────

pub struct ClockWidget {
    hour: u8,
    minute: u8,
    second: u8,
    show_seconds: bool,
    show_date: bool,
    format_24h: bool,
    day: u8,
    month: u8,
    year: u16,
    sandbox: WidgetSandbox,
}

impl ClockWidget {
    pub fn new() -> Self {
        Self {
            hour: 0,
            minute: 0,
            second: 0,
            show_seconds: true,
            show_date: true,
            format_24h: true,
            day: 1,
            month: 1,
            year: 2026,
            sandbox: WidgetSandbox::minimal().with_cap(WidgetCapability::ReadTime),
        }
    }
}

impl DesktopWidget for ClockWidget {
    fn name(&self) -> &str {
        "Clock"
    }
    fn preferred_size(&self) -> (u32, u32) {
        (180, 80)
    }
    fn sandbox(&self) -> &WidgetSandbox {
        &self.sandbox
    }

    fn update(&mut self, ctx: &WidgetContext) {
        self.hour = ctx.hour;
        self.minute = ctx.minute;
        self.second = ctx.second;
        self.day = ctx.day;
        self.month = ctx.month;
        self.year = ctx.year;
    }

    fn render(&self, out: &mut WidgetRenderOutput) {
        out.fill_rect(0, 0, 180, 80, 0xCC_0A_0E_1A);
        out.draw_rect(0, 0, 180, 80, 0xFF_4E_9C_FF);

        let h = if self.format_24h {
            self.hour
        } else {
            let h12 = self.hour % 12;
            if h12 == 0 {
                12
            } else {
                h12
            }
        };

        let mut time_buf = [0u8; 8];
        let time_len = fmt_time(
            h,
            self.minute,
            self.second,
            self.show_seconds,
            &mut time_buf,
        );
        let time_str = unsafe { core::str::from_utf8_unchecked(&time_buf[..time_len]) };
        out.draw_text(16, 16, time_str, 0xFF_FF_FF_FF, 24);

        if self.show_date {
            let mut date_buf = [0u8; 12];
            let date_len = fmt_date(self.year, self.month, self.day, &mut date_buf);
            let date_str = unsafe { core::str::from_utf8_unchecked(&date_buf[..date_len]) };
            out.draw_text(16, 52, date_str, 0xFF_88_88_AA, 12);
        }
    }

    fn on_click(&mut self, action: ClickAction) {
        if action == ClickAction::Toggle {
            self.format_24h = !self.format_24h;
        }
    }
}

// ── Built-in: Weather Widget ─────────────────────────────────────────────

pub struct WeatherWidget {
    data: WeatherData,
    sandbox: WidgetSandbox,
}

impl WeatherWidget {
    pub fn new() -> Self {
        Self {
            data: WeatherData::unknown(),
            sandbox: WidgetSandbox::minimal()
                .with_cap(WidgetCapability::ReadWeather)
                .with_cap(WidgetCapability::ReadNetwork),
        }
    }
}

impl DesktopWidget for WeatherWidget {
    fn name(&self) -> &str {
        "Weather"
    }
    fn preferred_size(&self) -> (u32, u32) {
        (200, 100)
    }
    fn sandbox(&self) -> &WidgetSandbox {
        &self.sandbox
    }

    fn update(&mut self, ctx: &WidgetContext) {
        self.data = ctx.weather.clone();
    }

    fn render(&self, out: &mut WidgetRenderOutput) {
        out.fill_rect(0, 0, 200, 100, 0xCC_0A_0E_1A);
        out.draw_rect(0, 0, 200, 100, 0xFF_4E_9C_FF);

        out.draw_glyph(16, 16, self.data.icon, 0xFF_FF_CC_44);

        let mut temp_buf = [0u8; 6];
        let temp_len = fmt_temp(self.data.temp_c, &mut temp_buf);
        let temp_str = unsafe { core::str::from_utf8_unchecked(&temp_buf[..temp_len]) };
        out.draw_text(40, 16, temp_str, 0xFF_FF_FF_FF, 20);

        out.draw_text(16, 44, &self.data.condition, 0xFF_CC_CC_DD, 12);

        let mut hum_buf = [0u8; 8];
        let hum_len = fmt_humidity(self.data.humidity_pct, &mut hum_buf);
        let hum_str = unsafe { core::str::from_utf8_unchecked(&hum_buf[..hum_len]) };
        out.draw_text(16, 64, hum_str, 0xFF_88_88_AA, 10);

        if !self.data.location.is_empty() {
            out.draw_text(16, 82, &self.data.location, 0xFF_66_66_88, 10);
        }
    }

    fn on_click(&mut self, _action: ClickAction) {}
}

// ── Built-in: System Monitor Widget ──────────────────────────────────────

pub struct SystemMonitorWidget {
    stats: SystemStats,
    page: u8,
    sandbox: WidgetSandbox,
}

impl SystemMonitorWidget {
    pub fn new() -> Self {
        Self {
            stats: SystemStats::zero(),
            page: 0,
            sandbox: WidgetSandbox::minimal().with_cap(WidgetCapability::ReadSystemStats),
        }
    }
}

impl DesktopWidget for SystemMonitorWidget {
    fn name(&self) -> &str {
        "System Monitor"
    }
    fn preferred_size(&self) -> (u32, u32) {
        (220, 160)
    }
    fn sandbox(&self) -> &WidgetSandbox {
        &self.sandbox
    }

    fn update(&mut self, ctx: &WidgetContext) {
        self.stats = ctx.stats.clone();
    }

    fn render(&self, out: &mut WidgetRenderOutput) {
        out.fill_rect(0, 0, 220, 160, 0xCC_0A_0E_1A);
        out.draw_rect(0, 0, 220, 160, 0xFF_4E_9C_FF);

        out.draw_text(12, 8, "SYSTEM", 0xFF_4E_9C_FF, 10);

        self.render_bar(
            out,
            12,
            28,
            196,
            "CPU",
            self.stats.cpu_usage_pct,
            0xFF_4E_9C_FF,
        );
        self.render_bar(out, 12, 52, 196, "RAM", self.stats.ram_pct(), 0xFF_FF_2E_88);
        self.render_bar(
            out,
            12,
            76,
            196,
            "GPU",
            self.stats.gpu_usage_pct,
            0xFF_00_D4_AA,
        );
        self.render_bar(
            out,
            12,
            100,
            196,
            "DSK",
            self.stats.disk_pct(),
            0xFF_FF_AA_33,
        );

        let mut cpu_temp_buf = [0u8; 6];
        let temp_len = fmt_temp(self.stats.cpu_temp_c as i8, &mut cpu_temp_buf);
        let temp_str = unsafe { core::str::from_utf8_unchecked(&cpu_temp_buf[..temp_len]) };
        out.draw_text(12, 128, "CPU:", 0xFF_88_88_AA, 10);
        out.draw_text(50, 128, temp_str, 0xFF_CC_CC_DD, 10);

        let mut gpu_temp_buf = [0u8; 6];
        let gt_len = fmt_temp(self.stats.gpu_temp_c as i8, &mut gpu_temp_buf);
        let gt_str = unsafe { core::str::from_utf8_unchecked(&gpu_temp_buf[..gt_len]) };
        out.draw_text(100, 128, "GPU:", 0xFF_88_88_AA, 10);
        out.draw_text(138, 128, gt_str, 0xFF_CC_CC_DD, 10);

        out.add_click_zone(0, 0, 220, 160, ClickAction::NextPage);
    }

    fn on_click(&mut self, action: ClickAction) {
        if action == ClickAction::NextPage {
            self.page = (self.page + 1) % 2;
        }
    }
}

impl SystemMonitorWidget {
    fn render_bar(
        &self,
        out: &mut WidgetRenderOutput,
        x: i32,
        y: i32,
        w: u32,
        label: &str,
        pct: u8,
        color: u32,
    ) {
        out.draw_text(x, y, label, 0xFF_88_88_AA, 10);
        let bar_x = x + 36;
        let bar_w = w - 72;
        out.fill_rect(bar_x, y + 2, bar_w, 10, 0xFF_22_22_33);
        let fill = (bar_w as u32 * pct as u32) / 100;
        if fill > 0 {
            out.fill_rect(bar_x, y + 2, fill, 10, color);
        }
        let mut pct_buf = [0u8; 4];
        let pct_len = fmt_pct(pct, &mut pct_buf);
        let pct_str = unsafe { core::str::from_utf8_unchecked(&pct_buf[..pct_len]) };
        out.draw_text(bar_x + bar_w as i32 + 4, y, pct_str, 0xFF_CC_CC_DD, 10);
    }
}

// ── Built-in: Now Playing Widget ─────────────────────────────────────────

pub struct NowPlayingWidget {
    audio: AudioState,
    sandbox: WidgetSandbox,
}

impl NowPlayingWidget {
    pub fn new() -> Self {
        Self {
            audio: AudioState::idle(),
            sandbox: WidgetSandbox::minimal().with_cap(WidgetCapability::ReadAudioState),
        }
    }
}

impl DesktopWidget for NowPlayingWidget {
    fn name(&self) -> &str {
        "Now Playing"
    }
    fn preferred_size(&self) -> (u32, u32) {
        (240, 90)
    }
    fn sandbox(&self) -> &WidgetSandbox {
        &self.sandbox
    }

    fn update(&mut self, ctx: &WidgetContext) {
        self.audio = ctx.audio.clone();
    }

    fn render(&self, out: &mut WidgetRenderOutput) {
        out.fill_rect(0, 0, 240, 90, 0xCC_0A_0E_1A);
        out.draw_rect(0, 0, 240, 90, 0xFF_4E_9C_FF);

        if !self.audio.playing && self.audio.track_title.is_empty() {
            out.draw_text(16, 36, "No audio playing", 0xFF_66_66_88, 12);
            return;
        }

        let status_icon = if self.audio.playing { '>' } else { '|' };
        out.draw_glyph(16, 12, status_icon, 0xFF_4E_9C_FF);

        let title = crate::text_util::truncate_chars(&self.audio.track_title, 28);
        out.draw_text(32, 12, title, 0xFF_FF_FF_FF, 12);

        let artist = crate::text_util::truncate_chars(&self.audio.artist, 30);
        out.draw_text(32, 32, artist, 0xFF_AA_AA_CC, 10);

        let bar_w = 200u32;
        let bar_x = 16i32;
        let bar_y = 56i32;
        out.fill_rect(bar_x, bar_y, bar_w, 4, 0xFF_22_22_33);
        let progress = (bar_w * self.audio.progress_pct as u32) / 100;
        if progress > 0 {
            out.fill_rect(bar_x, bar_y, progress, 4, 0xFF_4E_9C_FF);
        }

        out.add_click_zone(0, 0, 240, 50, ClickAction::Toggle);
        out.add_click_zone(0, 50, 120, 40, ClickAction::PrevPage);
        out.add_click_zone(120, 50, 120, 40, ClickAction::NextPage);
    }

    fn on_click(&mut self, _action: ClickAction) {}
}

// ── Built-in: Calendar Widget ────────────────────────────────────────────

pub struct CalendarWidget {
    day: u8,
    month: u8,
    year: u16,
    dow: u8,
    events: Vec<CalendarEvent>,
    sandbox: WidgetSandbox,
}

impl CalendarWidget {
    pub fn new() -> Self {
        Self {
            day: 1,
            month: 1,
            year: 2026,
            dow: 0,
            events: Vec::new(),
            sandbox: WidgetSandbox::minimal()
                .with_cap(WidgetCapability::ReadTime)
                .with_cap(WidgetCapability::ReadCalendar),
        }
    }
}

impl DesktopWidget for CalendarWidget {
    fn name(&self) -> &str {
        "Calendar"
    }
    fn preferred_size(&self) -> (u32, u32) {
        (200, 180)
    }
    fn sandbox(&self) -> &WidgetSandbox {
        &self.sandbox
    }

    fn update(&mut self, ctx: &WidgetContext) {
        self.day = ctx.day;
        self.month = ctx.month;
        self.year = ctx.year;
        self.dow = ctx.day_of_week;
        self.events = ctx.events.clone();
    }

    fn render(&self, out: &mut WidgetRenderOutput) {
        out.fill_rect(0, 0, 200, 180, 0xCC_0A_0E_1A);
        out.draw_rect(0, 0, 200, 180, 0xFF_4E_9C_FF);

        let month_name = month_str(self.month);
        let mut header_buf = [0u8; 16];
        let header_len = fmt_month_year(month_name, self.year, &mut header_buf);
        let header_str = unsafe { core::str::from_utf8_unchecked(&header_buf[..header_len]) };
        out.draw_text(12, 8, header_str, 0xFF_FF_FF_FF, 12);

        let days_header = "Su Mo Tu We Th Fr Sa";
        out.draw_text(12, 30, days_header, 0xFF_88_88_AA, 8);

        let first_dow = zeller_first_day(self.year, self.month);
        let days_in = days_in_month(self.year, self.month);
        let mut dx = 0i32;
        let mut dy = 0i32;
        let cell_w = 24i32;
        let cell_h = 16i32;
        let start_x = 12i32;
        let start_y = 46i32;

        dx = first_dow as i32;

        for d in 1..=days_in {
            let px = start_x + dx * cell_w;
            let py = start_y + dy * cell_h;
            let color = if d == self.day {
                0xFF_4E_9C_FF
            } else {
                0xFF_CC_CC_DD
            };
            if d == self.day {
                out.fill_rect(px - 2, py - 2, 20, 14, 0xFF_1A_2A_44);
            }
            let mut d_buf = [0u8; 2];
            let d_len = fmt_u8_2(d, &mut d_buf);
            let d_str = unsafe { core::str::from_utf8_unchecked(&d_buf[..d_len]) };
            out.draw_text(px, py, d_str, color, 8);
            dx += 1;
            if dx >= 7 {
                dx = 0;
                dy += 1;
            }
        }

        let ev_y = start_y + (dy + 1) * cell_h + 4;
        for (i, ev) in self.events.iter().take(3).enumerate() {
            let ey = ev_y + i as i32 * 14;
            let mut time_buf = [0u8; 5];
            let tl = fmt_hm(ev.hour, ev.minute, &mut time_buf);
            let ts = unsafe { core::str::from_utf8_unchecked(&time_buf[..tl]) };
            out.draw_text(12, ey, ts, 0xFF_4E_9C_FF, 8);
            let title = crate::text_util::truncate_chars(&ev.title, 18);
            out.draw_text(52, ey, title, 0xFF_CC_CC_DD, 8);
        }
    }

    fn on_click(&mut self, _action: ClickAction) {}
}

// ── Built-in: Quick Notes Widget ─────────────────────────────────────────

pub struct QuickNotesWidget {
    notes: Vec<String>,
    sandbox: WidgetSandbox,
}

impl QuickNotesWidget {
    pub fn new() -> Self {
        Self {
            notes: Vec::new(),
            sandbox: WidgetSandbox::minimal().with_cap(WidgetCapability::WriteNotes),
        }
    }

    pub fn add_note(&mut self, text: &str) {
        self.notes.push(String::from(text));
    }

    pub fn remove_note(&mut self, index: usize) {
        if index < self.notes.len() {
            self.notes.remove(index);
        }
    }
}

impl DesktopWidget for QuickNotesWidget {
    fn name(&self) -> &str {
        "Quick Notes"
    }
    fn preferred_size(&self) -> (u32, u32) {
        (200, 160)
    }
    fn sandbox(&self) -> &WidgetSandbox {
        &self.sandbox
    }

    fn update(&mut self, _ctx: &WidgetContext) {}

    fn render(&self, out: &mut WidgetRenderOutput) {
        out.fill_rect(0, 0, 200, 160, 0xCC_0A_0E_1A);
        out.draw_rect(0, 0, 200, 160, 0xFF_4E_9C_FF);

        out.draw_text(12, 8, "NOTES", 0xFF_4E_9C_FF, 10);

        if self.notes.is_empty() {
            out.draw_text(12, 36, "No notes yet", 0xFF_66_66_88, 10);
            return;
        }

        for (i, note) in self.notes.iter().take(8).enumerate() {
            let ny = 28 + i as i32 * 16;
            out.draw_glyph(12, ny, '-', 0xFF_4E_9C_FF);
            let txt = crate::text_util::truncate_chars(note, 22);
            out.draw_text(24, ny, txt, 0xFF_CC_CC_DD, 10);
        }
    }

    fn on_click(&mut self, _action: ClickAction) {}
}

// ── Widget placement & management ────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapGrid {
    Free,
    Grid16,
    Grid32,
    Grid64,
}

impl SnapGrid {
    pub fn snap(self, val: i32) -> i32 {
        match self {
            SnapGrid::Free => val,
            SnapGrid::Grid16 => (val / 16) * 16,
            SnapGrid::Grid32 => (val / 32) * 32,
            SnapGrid::Grid64 => (val / 64) * 64,
        }
    }
}

pub struct WidgetPlacement {
    pub widget_id: u64,
    pub name: String,
    pub rect: WidgetRect,
    pub locked: bool,
    pub visible: bool,
    pub opacity: u8,
    pub blur_bg: bool,
}

pub struct WidgetManager {
    pub placements: Vec<WidgetPlacement>,
    pub snap_grid: SnapGrid,
    pub next_id: u64,
    pub editing: bool,
    pub dragging: Option<u64>,
    pub drag_offset: (i32, i32),
}

impl WidgetManager {
    pub fn new() -> Self {
        Self {
            placements: Vec::new(),
            snap_grid: SnapGrid::Grid32,
            next_id: 1,
            editing: false,
            dragging: None,
            drag_offset: (0, 0),
        }
    }

    pub fn add_widget(&mut self, name: &str, x: i32, y: i32, w: u32, h: u32) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.placements.push(WidgetPlacement {
            widget_id: id,
            name: String::from(name),
            rect: WidgetRect::new(self.snap_grid.snap(x), self.snap_grid.snap(y), w, h),
            locked: false,
            visible: true,
            opacity: 0xCC,
            blur_bg: true,
        });
        id
    }

    pub fn remove_widget(&mut self, id: u64) {
        self.placements.retain(|p| p.widget_id != id);
    }

    pub fn move_widget(&mut self, id: u64, x: i32, y: i32) {
        if let Some(p) = self.placements.iter_mut().find(|p| p.widget_id == id) {
            if !p.locked {
                p.rect.x = self.snap_grid.snap(x);
                p.rect.y = self.snap_grid.snap(y);
            }
        }
    }

    pub fn toggle_lock(&mut self, id: u64) {
        if let Some(p) = self.placements.iter_mut().find(|p| p.widget_id == id) {
            p.locked = !p.locked;
        }
    }

    pub fn toggle_visibility(&mut self, id: u64) {
        if let Some(p) = self.placements.iter_mut().find(|p| p.widget_id == id) {
            p.visible = !p.visible;
        }
    }

    pub fn set_opacity(&mut self, id: u64, alpha: u8) {
        if let Some(p) = self.placements.iter_mut().find(|p| p.widget_id == id) {
            p.opacity = alpha;
        }
    }

    pub fn widget_at(&self, px: i32, py: i32) -> Option<u64> {
        self.placements
            .iter()
            .rev()
            .filter(|p| p.visible)
            .find(|p| p.rect.contains(px, py))
            .map(|p| p.widget_id)
    }

    pub fn begin_drag(&mut self, id: u64, mx: i32, my: i32) {
        if let Some(p) = self
            .placements
            .iter()
            .find(|p| p.widget_id == id && !p.locked)
        {
            self.dragging = Some(id);
            self.drag_offset = (mx - p.rect.x, my - p.rect.y);
        }
    }

    pub fn on_drag_move(&mut self, mx: i32, my: i32) {
        if let Some(id) = self.dragging {
            let nx = mx - self.drag_offset.0;
            let ny = my - self.drag_offset.1;
            self.move_widget(id, nx, ny);
        }
    }

    pub fn end_drag(&mut self) {
        self.dragging = None;
    }

    pub fn toggle_edit_mode(&mut self) {
        self.editing = !self.editing;
    }

    pub fn visible_widgets(&self) -> Vec<&WidgetPlacement> {
        self.placements.iter().filter(|p| p.visible).collect()
    }
}

// ── Widget bundle (for packaging / sharing) ──────────────────────────────

#[derive(Debug, Clone)]
pub struct WidgetBundle {
    pub name: String,
    pub author: String,
    pub version: u32,
    pub description: String,
    pub capabilities: Vec<WidgetCapability>,
    pub default_size: (u32, u32),
    pub properties: BTreeMap<String, String>,
    pub signed: bool,
    pub hash: u64,
}

impl WidgetBundle {
    pub fn new(name: &str, author: &str) -> Self {
        Self {
            name: String::from(name),
            author: String::from(author),
            version: 1,
            description: String::new(),
            capabilities: Vec::new(),
            default_size: (200, 100),
            properties: BTreeMap::new(),
            signed: false,
            hash: 0,
        }
    }
}

pub struct WidgetBundleRegistry {
    pub bundles: Vec<WidgetBundle>,
}

impl WidgetBundleRegistry {
    pub fn new() -> Self {
        Self {
            bundles: Vec::new(),
        }
    }

    pub fn register(&mut self, bundle: WidgetBundle) {
        self.bundles.push(bundle);
    }

    pub fn find(&self, name: &str) -> Option<&WidgetBundle> {
        self.bundles.iter().find(|b| b.name == name)
    }

    pub fn list_names(&self) -> Vec<&str> {
        self.bundles.iter().map(|b| b.name.as_str()).collect()
    }
}

// ── Formatting helpers ───────────────────────────────────────────────────

fn fmt_u8_2(n: u8, buf: &mut [u8; 2]) -> usize {
    if n >= 10 {
        buf[0] = b'0' + n / 10;
        buf[1] = b'0' + n % 10;
        2
    } else {
        buf[0] = b'0' + n;
        1
    }
}

fn fmt_time(h: u8, m: u8, s: u8, show_s: bool, buf: &mut [u8; 8]) -> usize {
    buf[0] = b'0' + h / 10;
    buf[1] = b'0' + h % 10;
    buf[2] = b':';
    buf[3] = b'0' + m / 10;
    buf[4] = b'0' + m % 10;
    if show_s {
        buf[5] = b':';
        buf[6] = b'0' + s / 10;
        buf[7] = b'0' + s % 10;
        8
    } else {
        5
    }
}

fn fmt_date(y: u16, m: u8, d: u8, buf: &mut [u8; 12]) -> usize {
    buf[0] = b'0' + (y / 1000) as u8;
    buf[1] = b'0' + ((y / 100) % 10) as u8;
    buf[2] = b'0' + ((y / 10) % 10) as u8;
    buf[3] = b'0' + (y % 10) as u8;
    buf[4] = b'-';
    buf[5] = b'0' + m / 10;
    buf[6] = b'0' + m % 10;
    buf[7] = b'-';
    buf[8] = b'0' + d / 10;
    buf[9] = b'0' + d % 10;
    10
}

fn fmt_hm(h: u8, m: u8, buf: &mut [u8; 5]) -> usize {
    buf[0] = b'0' + h / 10;
    buf[1] = b'0' + h % 10;
    buf[2] = b':';
    buf[3] = b'0' + m / 10;
    buf[4] = b'0' + m % 10;
    5
}

fn fmt_temp(t: i8, buf: &mut [u8; 6]) -> usize {
    let mut pos = 0;
    if t < 0 {
        buf[pos] = b'-';
        pos += 1;
    }
    let abs = (if t < 0 { -(t as i16) } else { t as i16 }) as u8;
    if abs >= 100 {
        buf[pos] = b'0' + abs / 100;
        pos += 1;
    }
    if abs >= 10 {
        buf[pos] = b'0' + (abs / 10) % 10;
        pos += 1;
    }
    buf[pos] = b'0' + abs % 10;
    pos += 1;
    buf[pos] = b'C';
    pos += 1;
    pos
}

fn fmt_humidity(h: u8, buf: &mut [u8; 8]) -> usize {
    let mut pos = 0;
    if h >= 100 {
        buf[pos] = b'1';
        pos += 1;
        buf[pos] = b'0';
        pos += 1;
        buf[pos] = b'0';
        pos += 1;
    } else if h >= 10 {
        buf[pos] = b'0' + h / 10;
        pos += 1;
        buf[pos] = b'0' + h % 10;
        pos += 1;
    } else {
        buf[pos] = b'0' + h;
        pos += 1;
    }
    buf[pos] = b'%';
    pos += 1;
    buf[pos] = b' ';
    pos += 1;
    buf[pos] = b'R';
    pos += 1;
    buf[pos] = b'H';
    pos += 1;
    pos
}

fn fmt_pct(pct: u8, buf: &mut [u8; 4]) -> usize {
    let mut pos = 0;
    if pct >= 100 {
        buf[pos] = b'1';
        pos += 1;
        buf[pos] = b'0';
        pos += 1;
        buf[pos] = b'0';
        pos += 1;
    } else if pct >= 10 {
        buf[pos] = b'0' + pct / 10;
        pos += 1;
        buf[pos] = b'0' + pct % 10;
        pos += 1;
    } else {
        buf[pos] = b'0' + pct;
        pos += 1;
    }
    buf[pos] = b'%';
    pos += 1;
    pos
}

fn fmt_month_year(month: &str, year: u16, buf: &mut [u8; 16]) -> usize {
    let mut pos = 0;
    for &b in month.as_bytes() {
        if pos >= 12 {
            break;
        }
        buf[pos] = b;
        pos += 1;
    }
    buf[pos] = b' ';
    pos += 1;
    buf[pos] = b'0' + (year / 1000) as u8;
    pos += 1;
    buf[pos] = b'0' + ((year / 100) % 10) as u8;
    pos += 1;
    buf[pos] = b'0' + ((year / 10) % 10) as u8;
    pos += 1;
    // bounds check
    if pos < 16 {
        buf[pos] = b'0' + (year % 10) as u8;
        pos += 1;
    }
    pos
}

fn month_str(m: u8) -> &'static str {
    match m {
        1 => "Jan",
        2 => "Feb",
        3 => "Mar",
        4 => "Apr",
        5 => "May",
        6 => "Jun",
        7 => "Jul",
        8 => "Aug",
        9 => "Sep",
        10 => "Oct",
        11 => "Nov",
        12 => "Dec",
        _ => "???",
    }
}

fn days_in_month(year: u16, month: u8) -> u8 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 {
                29
            } else {
                28
            }
        }
        _ => 30,
    }
}

fn zeller_first_day(year: u16, month: u8) -> u8 {
    let y = if month <= 2 { year - 1 } else { year } as i32;
    let m = if month <= 2 { month + 12 } else { month } as i32;
    let q = 1i32;
    let k = y % 100;
    let j = y / 100;
    let h = (q + (13 * (m + 1)) / 5 + k + k / 4 + j / 4 + 5 * j) % 7;
    ((h + 6) % 7) as u8
}
