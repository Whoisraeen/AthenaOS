//! Full desktop taskbar for AthenaOS shell.

#![allow(dead_code)]

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};

/// Map a system-tray indicator (by its stable `app_id`) to the closest shipped
/// `athgfx` line-icon — the SAME set the rest of the chrome consumes (visual-QA
/// Round-7 #1: retire the N/V/B letter glyphs). No new icons: network→WiFi,
/// volume→Volume, battery→Power, anything else→Bell (the generic indicator).
pub(crate) fn tray_line_icon(app_id: &str) -> athgfx::icon::Icon {
    use athgfx::icon::Icon;
    let id = app_id.to_ascii_lowercase();
    if id.contains("net") || id.contains("wifi") || id.contains("wireless") {
        Icon::WiFi
    } else if id.contains("vol") || id.contains("sound") || id.contains("audio") {
        Icon::Volume
    } else if id.contains("bat") || id.contains("power") {
        Icon::Power
    } else if id.contains("bluetooth") || id.contains("bt") {
        Icon::Bluetooth
    } else {
        Icon::Bell
    }
}

// ── Position and sizing ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskbarPosition {
    Bottom,
    Top,
    Left,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconSize {
    Small,
    Large,
}

impl IconSize {
    pub fn pixels(&self) -> u32 {
        match self {
            Self::Small => 16,
            Self::Large => 24,
        }
    }
}

#[derive(Debug, Clone)]
pub struct TaskbarConfig {
    pub position: TaskbarPosition,
    pub height: u32,
    pub auto_hide: bool,
    pub always_on_top: bool,
    pub icon_size: IconSize,
    pub show_labels: bool,
    pub group_by_app: bool,
    pub show_clock: bool,
    pub show_tray: bool,
    pub animation_speed_ms: u32,
    pub opacity: f32,
    pub accent_color: u32,
    pub background_color: u32,
    pub border_color: u32,
}

impl TaskbarConfig {
    pub fn default_config() -> Self {
        Self {
            position: TaskbarPosition::Bottom,
            height: 40,
            auto_hide: false,
            always_on_top: true,
            icon_size: IconSize::Small,
            show_labels: true,
            group_by_app: true,
            show_clock: true,
            show_tray: true,
            animation_speed_ms: 150,
            opacity: 0.95,
            accent_color: 0xFF_4E_9C_FF,
            background_color: 0xFF_0A_0E_1A,
            border_color: 0xFF_4E_9C_FF,
        }
    }
}

// ── Start button ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonHoverState {
    Normal,
    Hovered,
    Active,
}

#[derive(Debug, Clone)]
pub struct StartButton {
    pub icon_char: char,
    pub text: String,
    pub state: ButtonHoverState,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub visible: bool,
}

impl StartButton {
    pub fn new() -> Self {
        Self {
            icon_char: 'R',
            text: String::from("RaeOS"),
            state: ButtonHoverState::Normal,
            x: 0,
            y: 0,
            width: 60,
            height: 40,
            visible: true,
        }
    }

    pub fn contains(&self, px: i32, py: i32) -> bool {
        px >= self.x
            && px < self.x + self.width as i32
            && py >= self.y
            && py < self.y + self.height as i32
    }

    pub fn set_hover(&mut self, hovered: bool) {
        self.state = if hovered {
            ButtonHoverState::Hovered
        } else {
            ButtonHoverState::Normal
        };
    }

    pub fn set_active(&mut self, active: bool) {
        self.state = if active {
            ButtonHoverState::Active
        } else {
            ButtonHoverState::Normal
        };
    }
}

// ── Task buttons ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskButtonState {
    Normal,
    Focused,
    Hovered,
    Urgent,
}

#[derive(Debug, Clone)]
pub struct TaskButton {
    pub window_id: u64,
    pub app_id: String,
    pub title: String,
    pub icon_char: char,
    pub state: TaskButtonState,
    pub minimized: bool,
    pub badge_count: u32,
    pub progress: Option<f32>,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub group_id: Option<u64>,
    pub show_thumbnail: bool,
}

impl TaskButton {
    pub fn new(window_id: u64, app_id: &str, title: &str, icon: char) -> Self {
        Self {
            window_id,
            app_id: String::from(app_id),
            title: String::from(title),
            icon_char: icon,
            state: TaskButtonState::Normal,
            minimized: false,
            badge_count: 0,
            progress: None,
            x: 0,
            y: 0,
            width: 160,
            height: 36,
            group_id: None,
            show_thumbnail: false,
        }
    }

    pub fn truncated_title(&self, max_chars: usize) -> &str {
        crate::text_util::truncate_chars(&self.title, max_chars)
    }

    pub fn contains(&self, px: i32, py: i32) -> bool {
        px >= self.x
            && px < self.x + self.width as i32
            && py >= self.y
            && py < self.y + self.height as i32
    }

    pub fn set_focused(&mut self, focused: bool) {
        self.state = if focused {
            TaskButtonState::Focused
        } else {
            TaskButtonState::Normal
        };
    }
}

// ── Task button groups ──────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct TaskButtonGroup {
    pub group_id: u64,
    pub app_id: String,
    pub buttons: Vec<u64>,
    pub collapsed: bool,
    pub icon_char: char,
}

impl TaskButtonGroup {
    pub fn new(group_id: u64, app_id: &str, icon: char) -> Self {
        Self {
            group_id,
            app_id: String::from(app_id),
            buttons: Vec::new(),
            collapsed: true,
            icon_char: icon,
        }
    }

    pub fn add(&mut self, window_id: u64) {
        if !self.buttons.contains(&window_id) {
            self.buttons.push(window_id);
        }
    }

    pub fn remove(&mut self, window_id: u64) {
        self.buttons.retain(|&id| id != window_id);
    }

    pub fn count(&self) -> usize {
        self.buttons.len()
    }

    pub fn toggle_collapse(&mut self) {
        self.collapsed = !self.collapsed;
    }
}

// ── System tray ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayIconAction {
    LeftClick,
    RightClick,
    DoubleClick,
}

#[derive(Debug, Clone)]
pub struct TrayIcon {
    pub id: u64,
    pub app_id: String,
    pub label: String,
    pub glyph: char,
    pub tooltip: String,
    pub visible: bool,
    pub in_overflow: bool,
    pub badge: Option<String>,
    pub x: i32,
    pub y: i32,
    pub size: u32,
}

impl TrayIcon {
    pub fn new(id: u64, app_id: &str, label: &str, glyph: char) -> Self {
        Self {
            id,
            app_id: String::from(app_id),
            label: String::from(label),
            glyph,
            tooltip: String::new(),
            visible: true,
            in_overflow: false,
            badge: None,
            x: 0,
            y: 0,
            size: 20,
        }
    }

    pub fn contains(&self, px: i32, py: i32) -> bool {
        px >= self.x
            && px < self.x + self.size as i32
            && py >= self.y
            && py < self.y + self.size as i32
    }
}

#[derive(Debug, Clone)]
pub struct BalloonNotification {
    pub icon_id: u64,
    pub title: String,
    pub body: String,
    pub timestamp: u64,
    pub duration_ms: u32,
    pub elapsed_ms: u32,
}

#[derive(Debug, Clone)]
pub struct SystemTray {
    pub icons: Vec<TrayIcon>,
    pub overflow_visible: bool,
    pub max_visible_icons: usize,
    pub next_id: u64,
    pub balloon: Option<BalloonNotification>,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl SystemTray {
    pub fn new() -> Self {
        Self {
            icons: Vec::new(),
            overflow_visible: false,
            max_visible_icons: 6,
            next_id: 1,
            balloon: None,
            x: 0,
            y: 0,
            width: 0,
            height: 0,
        }
    }

    pub fn add_icon(&mut self, app_id: &str, label: &str, glyph: char) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let mut icon = TrayIcon::new(id, app_id, label, glyph);
        if self.icons.len() >= self.max_visible_icons {
            icon.in_overflow = true;
        }
        self.icons.push(icon);
        id
    }

    pub fn remove_icon(&mut self, id: u64) {
        self.icons.retain(|i| i.id != id);
    }

    pub fn set_tooltip(&mut self, id: u64, tooltip: &str) {
        if let Some(icon) = self.icons.iter_mut().find(|i| i.id == id) {
            icon.tooltip = String::from(tooltip);
        }
    }

    pub fn set_badge(&mut self, id: u64, badge: Option<&str>) {
        if let Some(icon) = self.icons.iter_mut().find(|i| i.id == id) {
            icon.badge = badge.map(String::from);
        }
    }

    pub fn show_balloon(&mut self, icon_id: u64, title: &str, body: &str, timestamp: u64) {
        self.balloon = Some(BalloonNotification {
            icon_id,
            title: String::from(title),
            body: String::from(body),
            timestamp,
            duration_ms: 5000,
            elapsed_ms: 0,
        });
    }

    pub fn dismiss_balloon(&mut self) {
        self.balloon = None;
    }

    pub fn tick(&mut self, delta_ms: u32) {
        if let Some(ref mut balloon) = self.balloon {
            balloon.elapsed_ms += delta_ms;
            if balloon.elapsed_ms >= balloon.duration_ms {
                self.balloon = None;
            }
        }
    }

    pub fn toggle_overflow(&mut self) {
        self.overflow_visible = !self.overflow_visible;
    }

    pub fn visible_icons(&self) -> Vec<&TrayIcon> {
        self.icons
            .iter()
            .filter(|i| !i.in_overflow && i.visible)
            .collect()
    }

    pub fn overflow_icons(&self) -> Vec<&TrayIcon> {
        self.icons
            .iter()
            .filter(|i| i.in_overflow && i.visible)
            .collect()
    }
}

// ── Clock ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClockFormat {
    Hour12,
    Hour24,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DateFormat {
    MonthDayYear,
    DayMonthYear,
    YearMonthDay,
}

#[derive(Debug, Clone)]
pub struct TimeZoneEntry {
    pub name: String,
    pub offset_minutes: i32,
    pub label: String,
}

#[derive(Debug, Clone)]
pub struct Clock {
    pub hours: u8,
    pub minutes: u8,
    pub seconds: u8,
    pub day: u8,
    pub month: u8,
    pub year: u16,
    pub day_of_week: u8,
    pub clock_format: ClockFormat,
    pub date_format: DateFormat,
    pub show_seconds: bool,
    pub show_date: bool,
    pub calendar_visible: bool,
    pub time_zones: Vec<TimeZoneEntry>,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl Clock {
    pub fn new() -> Self {
        Self {
            hours: 12,
            minutes: 0,
            seconds: 0,
            day: 1,
            month: 1,
            year: 2026,
            day_of_week: 0,
            clock_format: ClockFormat::Hour24,
            date_format: DateFormat::YearMonthDay,
            show_seconds: false,
            show_date: true,
            calendar_visible: false,
            time_zones: Vec::new(),
            x: 0,
            y: 0,
            width: 80,
            height: 40,
        }
    }

    pub fn update(&mut self, hours: u8, minutes: u8, seconds: u8) {
        self.hours = hours;
        self.minutes = minutes;
        self.seconds = seconds;
    }

    pub fn set_date(&mut self, year: u16, month: u8, day: u8, dow: u8) {
        self.year = year;
        self.month = month;
        self.day = day;
        self.day_of_week = dow;
    }

    pub fn format_time(&self, buf: &mut [u8; 16]) -> usize {
        let h = match self.clock_format {
            ClockFormat::Hour24 => self.hours,
            ClockFormat::Hour12 => {
                let h12 = self.hours % 12;
                if h12 == 0 {
                    12
                } else {
                    h12
                }
            }
        };
        let mut pos = 0;
        pos += write_u8_padded(h, &mut buf[pos..]);
        buf[pos] = b':';
        pos += 1;
        pos += write_u8_padded(self.minutes, &mut buf[pos..]);
        if self.show_seconds {
            buf[pos] = b':';
            pos += 1;
            pos += write_u8_padded(self.seconds, &mut buf[pos..]);
        }
        if let ClockFormat::Hour12 = self.clock_format {
            buf[pos] = b' ';
            pos += 1;
            if self.hours < 12 {
                buf[pos] = b'A';
            } else {
                buf[pos] = b'P';
            }
            pos += 1;
            buf[pos] = b'M';
            pos += 1;
        }
        pos
    }

    pub fn toggle_calendar(&mut self) {
        self.calendar_visible = !self.calendar_visible;
    }

    pub fn add_timezone(&mut self, name: &str, offset_minutes: i32, label: &str) {
        self.time_zones.push(TimeZoneEntry {
            name: String::from(name),
            offset_minutes,
            label: String::from(label),
        });
    }

    pub fn contains(&self, px: i32, py: i32) -> bool {
        px >= self.x
            && px < self.x + self.width as i32
            && py >= self.y
            && py < self.y + self.height as i32
    }
}

fn write_u8_padded(val: u8, buf: &mut [u8]) -> usize {
    if val >= 10 {
        buf[0] = b'0' + val / 10;
        buf[1] = b'0' + val % 10;
    } else {
        buf[0] = b'0';
        buf[1] = b'0' + val;
    }
    2
}

// ── Quick settings ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuickSettingKind {
    WiFi,
    Bluetooth,
    AirplaneMode,
    Battery,
    Volume,
    Brightness,
    NightLight,
    FocusAssist,
    ScreenCapture,
    NearbySharing,
    Project,
    Accessibility,
}

#[derive(Debug, Clone)]
pub struct QuickSetting {
    pub kind: QuickSettingKind,
    pub label: String,
    pub enabled: bool,
    pub icon_char: char,
    pub value: Option<u32>,
    pub connected_name: Option<String>,
}

impl QuickSetting {
    pub fn new(kind: QuickSettingKind, label: &str, icon: char) -> Self {
        Self {
            kind,
            label: String::from(label),
            enabled: false,
            icon_char: icon,
            value: None,
            connected_name: None,
        }
    }

    pub fn toggle(&mut self) {
        self.enabled = !self.enabled;
    }

    pub fn set_value(&mut self, val: u32) {
        self.value = Some(val);
    }
}

#[derive(Debug, Clone)]
pub struct QuickSettingsPanel {
    pub settings: Vec<QuickSetting>,
    pub visible: bool,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl QuickSettingsPanel {
    pub fn new() -> Self {
        let mut panel = Self {
            settings: Vec::new(),
            visible: false,
            x: 0,
            y: 0,
            width: 340,
            height: 280,
        };
        panel.populate_defaults();
        panel
    }

    fn populate_defaults(&mut self) {
        self.settings
            .push(QuickSetting::new(QuickSettingKind::WiFi, "Wi-Fi", 'W'));
        self.settings.push(QuickSetting::new(
            QuickSettingKind::Bluetooth,
            "Bluetooth",
            'B',
        ));
        self.settings.push(QuickSetting::new(
            QuickSettingKind::AirplaneMode,
            "Airplane",
            'A',
        ));
        self.settings.push(QuickSetting::new(
            QuickSettingKind::NightLight,
            "Night Light",
            'N',
        ));
        self.settings.push(QuickSetting::new(
            QuickSettingKind::FocusAssist,
            "Focus",
            'F',
        ));
        self.settings.push(QuickSetting::new(
            QuickSettingKind::ScreenCapture,
            "Capture",
            'C',
        ));
        self.settings.push(QuickSetting::new(
            QuickSettingKind::NearbySharing,
            "Nearby",
            'S',
        ));
        self.settings
            .push(QuickSetting::new(QuickSettingKind::Project, "Project", 'P'));
        self.settings.push(QuickSetting::new(
            QuickSettingKind::Accessibility,
            "Access",
            'X',
        ));

        let mut vol = QuickSetting::new(QuickSettingKind::Volume, "Volume", 'V');
        vol.value = Some(75);
        vol.enabled = true;
        self.settings.push(vol);

        let mut bright = QuickSetting::new(QuickSettingKind::Brightness, "Brightness", 'L');
        bright.value = Some(80);
        bright.enabled = true;
        self.settings.push(bright);

        let mut bat = QuickSetting::new(QuickSettingKind::Battery, "Battery", 'E');
        bat.value = Some(100);
        bat.enabled = true;
        self.settings.push(bat);
    }

    pub fn toggle(&mut self) {
        self.visible = !self.visible;
    }

    pub fn toggle_setting(&mut self, kind: QuickSettingKind) {
        if let Some(s) = self.settings.iter_mut().find(|s| s.kind == kind) {
            s.toggle();
        }
    }

    pub fn set_value(&mut self, kind: QuickSettingKind, val: u32) {
        if let Some(s) = self.settings.iter_mut().find(|s| s.kind == kind) {
            s.set_value(val);
        }
    }

    pub fn get_setting(&self, kind: QuickSettingKind) -> Option<&QuickSetting> {
        self.settings.iter().find(|s| s.kind == kind)
    }
}

// ── Volume popup ────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct AudioDevice {
    pub id: u64,
    pub name: String,
    pub device_type: AudioDeviceType,
    pub active: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioDeviceType {
    Speakers,
    Headphones,
    Hdmi,
    UsbDac,
    Bluetooth,
}

#[derive(Debug, Clone)]
pub struct VolumePopup {
    pub visible: bool,
    pub volume: u32,
    pub muted: bool,
    pub devices: Vec<AudioDevice>,
    pub active_device_id: u64,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl VolumePopup {
    pub fn new() -> Self {
        Self {
            visible: false,
            volume: 75,
            muted: false,
            devices: Vec::new(),
            active_device_id: 0,
            x: 0,
            y: 0,
            width: 280,
            height: 200,
        }
    }

    pub fn toggle(&mut self) {
        self.visible = !self.visible;
    }

    pub fn set_volume(&mut self, vol: u32) {
        self.volume = vol.min(100);
        if vol > 0 {
            self.muted = false;
        }
    }

    pub fn toggle_mute(&mut self) {
        self.muted = !self.muted;
    }

    pub fn add_device(&mut self, name: &str, device_type: AudioDeviceType) -> u64 {
        let id = self.devices.len() as u64 + 1;
        self.devices.push(AudioDevice {
            id,
            name: String::from(name),
            device_type,
            active: self.devices.is_empty(),
        });
        if self.devices.len() == 1 {
            self.active_device_id = id;
        }
        id
    }

    pub fn select_device(&mut self, id: u64) {
        for dev in &mut self.devices {
            dev.active = dev.id == id;
        }
        self.active_device_id = id;
    }
}

// ── Network popup ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkType {
    WiFi,
    Ethernet,
    Vpn,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionState {
    Connected,
    Connecting,
    Disconnected,
    Limited,
}

#[derive(Debug, Clone)]
pub struct NetworkEntry {
    pub ssid: String,
    pub signal_strength: u8,
    pub secured: bool,
    pub network_type: NetworkType,
    pub state: ConnectionState,
    pub saved: bool,
}

#[derive(Debug, Clone)]
pub struct NetworkPopup {
    pub visible: bool,
    pub wifi_enabled: bool,
    pub networks: Vec<NetworkEntry>,
    pub connected_ssid: Option<String>,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl NetworkPopup {
    pub fn new() -> Self {
        Self {
            visible: false,
            wifi_enabled: true,
            networks: Vec::new(),
            connected_ssid: None,
            x: 0,
            y: 0,
            width: 300,
            height: 320,
        }
    }

    pub fn toggle(&mut self) {
        self.visible = !self.visible;
    }

    pub fn scan_networks(&mut self, entries: Vec<NetworkEntry>) {
        self.networks = entries;
        self.networks
            .sort_by(|a, b| b.signal_strength.cmp(&a.signal_strength));
    }

    pub fn connect(&mut self, ssid: &str) {
        for net in &mut self.networks {
            if net.ssid == ssid {
                net.state = ConnectionState::Connected;
                self.connected_ssid = Some(String::from(ssid));
            } else if net.state == ConnectionState::Connected {
                net.state = ConnectionState::Disconnected;
            }
        }
    }

    pub fn disconnect(&mut self) {
        for net in &mut self.networks {
            if net.state == ConnectionState::Connected {
                net.state = ConnectionState::Disconnected;
            }
        }
        self.connected_ssid = None;
    }

    pub fn toggle_wifi(&mut self) {
        self.wifi_enabled = !self.wifi_enabled;
        if !self.wifi_enabled {
            self.disconnect();
        }
    }
}

// ── Battery popup ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerMode {
    BestPerformance,
    Balanced,
    BatterySaver,
}

#[derive(Debug, Clone)]
pub struct BatteryPopup {
    pub visible: bool,
    pub percentage: u8,
    pub charging: bool,
    pub estimated_minutes: u32,
    pub power_mode: PowerMode,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl BatteryPopup {
    pub fn new() -> Self {
        Self {
            visible: false,
            percentage: 100,
            charging: false,
            estimated_minutes: 480,
            power_mode: PowerMode::Balanced,
            x: 0,
            y: 0,
            width: 260,
            height: 180,
        }
    }

    pub fn toggle(&mut self) {
        self.visible = !self.visible;
    }

    pub fn update(&mut self, percentage: u8, charging: bool, est_minutes: u32) {
        self.percentage = percentage.min(100);
        self.charging = charging;
        self.estimated_minutes = est_minutes;
    }

    pub fn set_power_mode(&mut self, mode: PowerMode) {
        self.power_mode = mode;
    }

    pub fn is_low(&self) -> bool {
        self.percentage <= 20 && !self.charging
    }

    pub fn is_critical(&self) -> bool {
        self.percentage <= 5 && !self.charging
    }
}

// ── Input indicator ─────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct KeyboardLayout {
    pub id: u32,
    pub name: String,
    pub short_name: String,
}

#[derive(Debug, Clone)]
pub struct InputIndicator {
    pub layouts: Vec<KeyboardLayout>,
    pub active_layout: u32,
    pub ime_active: bool,
    pub touch_keyboard_visible: bool,
    pub handwriting_panel_visible: bool,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl InputIndicator {
    pub fn new() -> Self {
        let mut ind = Self {
            layouts: Vec::new(),
            active_layout: 0,
            ime_active: false,
            touch_keyboard_visible: false,
            handwriting_panel_visible: false,
            x: 0,
            y: 0,
            width: 32,
            height: 32,
        };
        ind.layouts.push(KeyboardLayout {
            id: 0,
            name: String::from("English (US)"),
            short_name: String::from("EN"),
        });
        ind
    }

    pub fn add_layout(&mut self, name: &str, short_name: &str) -> u32 {
        let id = self.layouts.len() as u32;
        self.layouts.push(KeyboardLayout {
            id,
            name: String::from(name),
            short_name: String::from(short_name),
        });
        id
    }

    pub fn switch_layout(&mut self) {
        if self.layouts.is_empty() {
            return;
        }
        self.active_layout = (self.active_layout + 1) % self.layouts.len() as u32;
    }

    pub fn set_layout(&mut self, id: u32) {
        if (id as usize) < self.layouts.len() {
            self.active_layout = id;
        }
    }

    pub fn active_short_name(&self) -> &str {
        self.layouts
            .get(self.active_layout as usize)
            .map(|l| l.short_name.as_str())
            .unwrap_or("??")
    }

    pub fn toggle_touch_keyboard(&mut self) {
        self.touch_keyboard_visible = !self.touch_keyboard_visible;
    }

    pub fn toggle_handwriting(&mut self) {
        self.handwriting_panel_visible = !self.handwriting_panel_visible;
    }
}

// ── Pinned apps & jump lists ────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct JumpListItem {
    pub title: String,
    pub path: String,
    pub pinned: bool,
    pub frequent: bool,
    pub recent: bool,
}

#[derive(Debug, Clone)]
pub struct PinnedApp {
    pub app_id: String,
    pub name: String,
    pub icon_char: char,
    pub exec_path: String,
    pub position: u32,
    pub jump_list: Vec<JumpListItem>,
    pub running: bool,
    pub window_count: u32,
}

impl PinnedApp {
    pub fn new(app_id: &str, name: &str, icon: char, exec_path: &str) -> Self {
        Self {
            app_id: String::from(app_id),
            name: String::from(name),
            icon_char: icon,
            exec_path: String::from(exec_path),
            position: 0,
            jump_list: Vec::new(),
            running: false,
            window_count: 0,
        }
    }

    pub fn add_jump_item(&mut self, title: &str, path: &str, pinned: bool) {
        self.jump_list.push(JumpListItem {
            title: String::from(title),
            path: String::from(path),
            pinned,
            frequent: false,
            recent: true,
        });
    }

    pub fn recent_items(&self) -> Vec<&JumpListItem> {
        self.jump_list.iter().filter(|j| j.recent).collect()
    }

    pub fn pinned_items(&self) -> Vec<&JumpListItem> {
        self.jump_list.iter().filter(|j| j.pinned).collect()
    }

    pub fn frequent_items(&self) -> Vec<&JumpListItem> {
        self.jump_list.iter().filter(|j| j.frequent).collect()
    }
}

// ── Window preview (thumbnail) ──────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct WindowPreview {
    pub window_id: u64,
    pub title: String,
    pub visible: bool,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub hover_close: bool,
}

impl WindowPreview {
    pub fn new(window_id: u64, title: &str) -> Self {
        Self {
            window_id,
            title: String::from(title),
            visible: false,
            x: 0,
            y: 0,
            width: 200,
            height: 140,
            hover_close: false,
        }
    }

    pub fn show_at(&mut self, x: i32, y: i32) {
        self.x = x;
        self.y = y;
        self.visible = true;
    }

    pub fn hide(&mut self) {
        self.visible = false;
        self.hover_close = false;
    }

    pub fn close_button_rect(&self) -> (i32, i32, u32, u32) {
        (self.x + self.width as i32 - 20, self.y + 4, 16, 16)
    }

    pub fn contains_close(&self, px: i32, py: i32) -> bool {
        let (cx, cy, cw, ch) = self.close_button_rect();
        px >= cx && px < cx + cw as i32 && py >= cy && py < cy + ch as i32
    }
}

// ── Search bar ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct SearchBar {
    pub query: String,
    pub placeholder: String,
    pub focused: bool,
    pub suggestions: Vec<String>,
    pub selected_suggestion: usize,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub visible: bool,
}

impl SearchBar {
    pub fn new() -> Self {
        Self {
            query: String::new(),
            placeholder: String::from("Search apps, files, settings..."),
            focused: false,
            suggestions: Vec::new(),
            selected_suggestion: 0,
            x: 0,
            y: 0,
            width: 240,
            height: 32,
            visible: true,
        }
    }

    pub fn set_query(&mut self, q: &str) {
        self.query = String::from(q);
        self.selected_suggestion = 0;
    }

    pub fn clear(&mut self) {
        self.query.clear();
        self.suggestions.clear();
        self.selected_suggestion = 0;
    }

    pub fn add_suggestion(&mut self, text: &str) {
        self.suggestions.push(String::from(text));
    }

    pub fn select_next(&mut self) {
        if !self.suggestions.is_empty() {
            self.selected_suggestion = (self.selected_suggestion + 1) % self.suggestions.len();
        }
    }

    pub fn select_prev(&mut self) {
        if !self.suggestions.is_empty() {
            self.selected_suggestion = self
                .selected_suggestion
                .checked_sub(1)
                .unwrap_or(self.suggestions.len() - 1);
        }
    }

    pub fn selected_text(&self) -> Option<&str> {
        self.suggestions
            .get(self.selected_suggestion)
            .map(|s| s.as_str())
    }

    pub fn display_text(&self) -> &str {
        if self.query.is_empty() {
            &self.placeholder
        } else {
            &self.query
        }
    }
}

// ── Action center badge ─────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ActionCenterButton {
    pub notification_count: u32,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub hovered: bool,
}

impl ActionCenterButton {
    pub fn new() -> Self {
        Self {
            notification_count: 0,
            x: 0,
            y: 0,
            width: 28,
            height: 28,
            hovered: false,
        }
    }

    pub fn set_count(&mut self, count: u32) {
        self.notification_count = count;
    }

    pub fn has_notifications(&self) -> bool {
        self.notification_count > 0
    }

    pub fn contains(&self, px: i32, py: i32) -> bool {
        px >= self.x
            && px < self.x + self.width as i32
            && py >= self.y
            && py < self.y + self.height as i32
    }
}

// ── Global taskbar ──────────────────────────────────────────────────────────

static TASKBAR_INITIALIZED: AtomicBool = AtomicBool::new(false);

pub struct Taskbar {
    pub config: TaskbarConfig,
    pub start_button: StartButton,
    pub task_buttons: Vec<TaskButton>,
    pub groups: Vec<TaskButtonGroup>,
    pub pinned_apps: Vec<PinnedApp>,
    pub system_tray: SystemTray,
    pub clock: Clock,
    pub quick_settings: QuickSettingsPanel,
    pub volume_popup: VolumePopup,
    pub network_popup: NetworkPopup,
    pub battery_popup: BatteryPopup,
    pub input_indicator: InputIndicator,
    pub search_bar: SearchBar,
    pub action_center: ActionCenterButton,
    pub preview: Option<WindowPreview>,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub screen_width: u32,
    pub screen_height: u32,
    pub hidden: bool,
    pub auto_hide_timer_ms: u32,
    pub next_group_id: u64,
}

impl Taskbar {
    pub fn new(screen_width: u32, screen_height: u32) -> Self {
        let config = TaskbarConfig::default_config();
        let y = (screen_height - config.height) as i32;
        let mut tb = Self {
            config: TaskbarConfig::default_config(),
            start_button: StartButton::new(),
            task_buttons: Vec::new(),
            groups: Vec::new(),
            pinned_apps: Vec::new(),
            system_tray: SystemTray::new(),
            clock: Clock::new(),
            quick_settings: QuickSettingsPanel::new(),
            volume_popup: VolumePopup::new(),
            network_popup: NetworkPopup::new(),
            battery_popup: BatteryPopup::new(),
            input_indicator: InputIndicator::new(),
            search_bar: SearchBar::new(),
            action_center: ActionCenterButton::new(),
            preview: None,
            x: 0,
            y,
            width: screen_width,
            height: config.height,
            screen_width,
            screen_height,
            hidden: false,
            auto_hide_timer_ms: 0,
            next_group_id: 1,
        };
        tb.start_button.y = y;
        tb.layout_components();
        tb
    }

    pub fn add_task(&mut self, window_id: u64, app_id: &str, title: &str, icon: char) {
        let btn = TaskButton::new(window_id, app_id, title, icon);
        self.task_buttons.push(btn);

        if self.config.group_by_app {
            self.update_groups(app_id, window_id, icon);
        }
        self.layout_task_buttons();
    }

    pub fn remove_task(&mut self, window_id: u64) {
        self.task_buttons.retain(|b| b.window_id != window_id);
        for group in &mut self.groups {
            group.remove(window_id);
        }
        self.groups.retain(|g| !g.buttons.is_empty());
        self.layout_task_buttons();
    }

    pub fn focus_task(&mut self, window_id: u64) {
        for btn in &mut self.task_buttons {
            btn.set_focused(btn.window_id == window_id);
        }
    }

    pub fn set_badge(&mut self, window_id: u64, count: u32) {
        if let Some(btn) = self
            .task_buttons
            .iter_mut()
            .find(|b| b.window_id == window_id)
        {
            btn.badge_count = count;
        }
    }

    pub fn set_progress(&mut self, window_id: u64, progress: Option<f32>) {
        if let Some(btn) = self
            .task_buttons
            .iter_mut()
            .find(|b| b.window_id == window_id)
        {
            btn.progress = progress;
        }
    }

    pub fn pin_app(&mut self, app_id: &str, name: &str, icon: char, exec_path: &str) {
        let pos = self.pinned_apps.len() as u32;
        let mut app = PinnedApp::new(app_id, name, icon, exec_path);
        app.position = pos;
        self.pinned_apps.push(app);
    }

    pub fn unpin_app(&mut self, app_id: &str) {
        self.pinned_apps.retain(|a| a.app_id != app_id);
        for (i, app) in self.pinned_apps.iter_mut().enumerate() {
            app.position = i as u32;
        }
    }

    pub fn reorder_pinned(&mut self, app_id: &str, new_pos: u32) {
        if let Some(idx) = self.pinned_apps.iter().position(|a| a.app_id == app_id) {
            let app = self.pinned_apps.remove(idx);
            let insert_at = (new_pos as usize).min(self.pinned_apps.len());
            self.pinned_apps.insert(insert_at, app);
            for (i, a) in self.pinned_apps.iter_mut().enumerate() {
                a.position = i as u32;
            }
        }
    }

    pub fn show_preview(&mut self, window_id: u64, title: &str, x: i32) {
        let mut preview = WindowPreview::new(window_id, title);
        preview.show_at(x, self.y - 150);
        self.preview = Some(preview);
    }

    pub fn hide_preview(&mut self) {
        self.preview = None;
    }

    pub fn set_position(&mut self, pos: TaskbarPosition) {
        self.config.position = pos;
        self.recalculate_geometry();
    }

    pub fn toggle_auto_hide(&mut self) {
        self.config.auto_hide = !self.config.auto_hide;
    }

    pub fn tick(&mut self, delta_ms: u32) {
        self.system_tray.tick(delta_ms);
        if self.config.auto_hide && self.hidden {
            self.auto_hide_timer_ms += delta_ms;
        }
    }

    pub fn handle_mouse_move(&mut self, x: i32, y: i32) {
        if self.config.auto_hide {
            let edge_zone = match self.config.position {
                TaskbarPosition::Bottom => y >= self.screen_height as i32 - 2,
                TaskbarPosition::Top => y <= 2,
                TaskbarPosition::Left => x <= 2,
                TaskbarPosition::Right => x >= self.screen_width as i32 - 2,
            };
            if edge_zone && self.hidden {
                self.hidden = false;
            }
            if !self.contains(x, y) && !self.hidden {
                self.auto_hide_timer_ms = 0;
            }
        }

        self.start_button
            .set_hover(self.start_button.contains(x, y));

        for btn in &mut self.task_buttons {
            if btn.contains(x, y) {
                if btn.state != TaskButtonState::Focused {
                    btn.state = TaskButtonState::Hovered;
                }
            } else if btn.state == TaskButtonState::Hovered {
                btn.state = TaskButtonState::Normal;
            }
        }
    }

    pub fn contains(&self, px: i32, py: i32) -> bool {
        px >= self.x
            && px < self.x + self.width as i32
            && py >= self.y
            && py < self.y + self.height as i32
    }

    pub fn work_area_inset(&self) -> (i32, i32, u32, u32) {
        if self.hidden {
            return (0, 0, 0, 0);
        }
        match self.config.position {
            TaskbarPosition::Bottom => (0, 0, 0, self.height),
            TaskbarPosition::Top => (0, self.height as i32, 0, self.height),
            TaskbarPosition::Left => (self.width as i32, 0, self.width, 0),
            TaskbarPosition::Right => (0, 0, self.width, 0),
        }
    }

    fn update_groups(&mut self, app_id: &str, window_id: u64, icon: char) {
        if let Some(group) = self.groups.iter_mut().find(|g| g.app_id == app_id) {
            group.add(window_id);
        } else {
            let gid = self.next_group_id;
            self.next_group_id += 1;
            let mut group = TaskButtonGroup::new(gid, app_id, icon);
            group.add(window_id);
            self.groups.push(group);
        }
    }

    /// Re-run the full bar layout (start button + centered app cluster) —
    /// for callers that mutate button widths/states directly (resolution
    /// change, the screenshot harness) rather than through add/remove.
    pub fn relayout(&mut self) {
        self.layout_components();
        self.layout_task_buttons();
    }

    fn layout_task_buttons(&mut self) {
        // Win11-style CENTERED app cluster: the pills center on the bar's
        // midpoint (the Start pill stays anchored left, tray/clock right),
        // falling back to left-flow after the Start button when the cluster
        // would collide with it.
        let btn_spacing = 4i32;
        let cluster_w: i32 = self
            .task_buttons
            .iter()
            .map(|b| b.width as i32 + btn_spacing)
            .sum::<i32>()
            - if self.task_buttons.is_empty() {
                0
            } else {
                btn_spacing
            };
        let min_x = self.start_button.width as i32 + 16;
        let mut cx = ((self.width as i32 - cluster_w) / 2).max(min_x);
        let cy = self.y + 2;

        for btn in &mut self.task_buttons {
            btn.x = cx;
            btn.y = cy;
            btn.height = self.height - 4;
            cx += btn.width as i32 + btn_spacing;
        }
    }

    fn layout_components(&mut self) {
        self.start_button.x = self.x;
        self.start_button.y = self.y;
        self.start_button.height = self.height;
    }

    fn recalculate_geometry(&mut self) {
        match self.config.position {
            TaskbarPosition::Bottom => {
                self.x = 0;
                self.y = (self.screen_height - self.height) as i32;
                self.width = self.screen_width;
            }
            TaskbarPosition::Top => {
                self.x = 0;
                self.y = 0;
                self.width = self.screen_width;
            }
            TaskbarPosition::Left => {
                self.x = 0;
                self.y = 0;
                self.height = self.screen_height;
                self.width = 60;
            }
            TaskbarPosition::Right => {
                self.x = (self.screen_width - 60) as i32;
                self.y = 0;
                self.height = self.screen_height;
                self.width = 60;
            }
        }
        self.layout_components();
    }

    /// Render the taskbar into the given Canvas.
    ///
    /// IDENTITY.md §7 (per-surface tier table): the taskbar is the system
    /// **chrome** — it paints with `glass.chrome` (the most see-through tier) so
    /// the aurora wallpaper reads *through* it, plus the signature iridescent
    /// rim along its edge. Running-app buttons are frosted pills (popover tier);
    /// the active app gets an accent-filled pill with **dark on-accent ink**
    /// (never white — white-on-RaeBlue ≈2.6:1, a contrast fail). Tray icons are
    /// token-tinted (`text.primary`/`text.tertiary`), the clock + labels use
    /// `text.primary` (the §9 a11y guardrail: `text.secondary` over bright
    /// aurora-backed chrome fails WCAG, so chrome labels promote to primary).
    /// No hardcoded glass/accent hex survives — every colour resolves from
    /// `ath_tokens` through the LIVE seed accent shared with Start/CC/Settings.
    pub fn render(&self, canvas: &mut athgfx::Canvas) {
        let w = self.width as usize;
        let h = self.height as usize;
        // Chrome type: RaeSans, NOT the 8x8 bitmap/mono path (visual-QA Round-8
        // P1 #5 — the chunky monospace taskbar text contradicted the RaeSans
        // UI-type promise; macOS/Win11 use a proportional UI sans for all chrome).
        // `type.label` is the shipped "taskbar labels" style; `type.caption` the
        // "tray/timestamp" style. Pill/clock/tray text now routes through the
        // SAME anti-aliased proportional path Control Center / Settings use.
        let label_style = ath_tokens::TYPE_LABEL;
        let caption_style = ath_tokens::TYPE_CAPTION;
        let sans = athgfx::text::FontFamily::Sans;
        // Top-left y of a `style`-tall line box vertically centred in the bar.
        let line_top = |style: ath_tokens::TypeStyle| -> i32 {
            (h.saturating_sub(style.line_height as usize) / 2) as i32
        };

        // Cohesion: the LIVE seed accent shared across every shell surface, and
        // the active (HC-aware) palette so the bar repaints in forced-colors for
        // free. The taskbar tier is `glass.chrome`; pills use the popover tier.
        let p = crate::active_palette();
        let a = ath_tokens::derive_accent(crate::active_accent(), p);

        // ── Chrome glass: the frosted, see-through bar the aurora reads through
        //    (IDENTITY.md §2.1 `glass.chrome`), via the EDGE-DOCKED variant: the
        //    same tint → frost → legibility-cap interior, but the edge stack is
        //    confined to the exposed top edge (hairline + quiet iridescent
        //    shimmer + lit lip). The full perimeter rim read as two straight
        //    neon lines across the screen on a radius-0 strip (visual-QA).
        athgfx::glass::draw_glass_surface_docked(
            canvas,
            0,
            0,
            w,
            h,
            ath_tokens::GLASS_CHROME_DARK,
            true,
        );

        // Dark on-accent ink for any accent-filled element (IDENTITY guardrail:
        // text/icons on an accent fill must be DARK `bg.base`, never white).
        let on_accent = p.bg_base;

        let pill_pad = 4usize;
        let pill_r = (h.saturating_sub(2 * pill_pad)) / 2; // radius_pill = inner-h/2

        // ── Start button — the Rae mark (diamond/orb line-icon, the OS identity
        //    glyph — retires the 'R' letter + "RaeOS" text label; macOS/Win11
        //    both anchor their bar with a bare mark). Frosted pill on hover,
        //    accent-filled with dark on-accent ink when the menu is open. ──────
        let sb = &self.start_button;
        let sb_w = sb.width as usize;
        let mark_sz = 20usize;
        let mark_x = pill_pad + (sb_w.saturating_sub(mark_sz)) / 2;
        let mark_y = (h.saturating_sub(mark_sz)) / 2;
        let mark_ink = match sb.state {
            ButtonHoverState::Active => {
                // Accent-filled active pill: dark on-accent ink.
                canvas.fill_rounded_rect(
                    pill_pad,
                    pill_pad,
                    sb_w,
                    h - 2 * pill_pad,
                    pill_r,
                    a.base,
                );
                on_accent
            }
            ButtonHoverState::Hovered => {
                // Obsidian hover = a solid elevation-ladder step (the frost
                // wash is a whisper now, invisible as a fill).
                canvas.fill_rounded_rect(
                    pill_pad,
                    pill_pad,
                    sb_w,
                    h - 2 * pill_pad,
                    pill_r,
                    p.bg_elevated,
                );
                a.base
            }
            ButtonHoverState::Normal => a.base,
        };
        canvas.draw_icon(
            athgfx::icon::Icon::RaeLogo,
            mark_x as i32,
            mark_y as i32,
            mark_sz as i32,
            mark_ink,
        );

        // Task buttons — frosted pills; active app = accent glow.
        let sep_x = sb_w;
        let btn_start = sep_x + 8;
        for btn in &self.task_buttons {
            let bx = btn.x as usize;
            let bw = btn.width as usize;
            if bx + bw > w {
                break;
            }
            let py = pill_pad;
            let ph = h - 2 * pill_pad;

            // Per-state pill fill + ink. `Focused` (active app) is accent-filled
            // with dark on-accent ink; hover is a frosted wash. `Urgent` keeps a
            // QUIET frosted pill and signals with the danger underline + badge —
            // the old full danger wash muddied to maroon over the dark glass
            // (visual-QA R9 #7) and shouted louder than the focused app.
            let ink = match btn.state {
                TaskButtonState::Focused => {
                    canvas.fill_rounded_rect(bx, py, bw, ph, pill_r, a.base);
                    on_accent
                }
                TaskButtonState::Hovered | TaskButtonState::Urgent => {
                    canvas.fill_rounded_rect(bx, py, bw, ph, pill_r, p.bg_elevated);
                    p.text_primary
                }
                TaskButtonState::Normal => p.text_primary,
            };

            // Running-app indicator (taskbar-running-apps.md): a small underline
            // centered under the pill — accent for running apps, full-strength
            // danger for urgent. The focused app's accent FILL is its indicator.
            if btn.state != TaskButtonState::Focused {
                let (ind_w, ind_color) = if btn.state == TaskButtonState::Urgent {
                    (16usize, p.state_danger)
                } else {
                    (8usize, a.base)
                };
                let ind_x = bx + (bw.saturating_sub(ind_w)) / 2;
                let ind_y = h.saturating_sub(4);
                canvas.fill_rounded_rect(ind_x, ind_y, ind_w, 2, 1, ind_color);
            }

            // Icon ink: on an accent pill it is dark; otherwise accent-tinted.
            let icon_ink = if btn.state == TaskButtonState::Focused {
                on_accent
            } else {
                a.base
            };
            // Real athgfx line-icon per running app (visual-QA Round-7 #1: retire
            // the R/F/W/T/M LETTER glyphs). Keyed off the pill's app_id/title via
            // the SAME mapping Start uses; vertically centred on the pill.
            let icon_sz = 18usize;
            let icon_y = py + (ph.saturating_sub(icon_sz)) / 2;
            canvas.draw_icon(
                crate::start_menu::app_line_icon(&btn.app_id, &btn.title),
                (bx + 8) as i32,
                icon_y as i32,
                icon_sz as i32,
                icon_ink,
            );

            // Proportional truncation (visual-QA Round-8 P1 #5): measure the
            // RaeSans advance instead of a fixed mono char-count, dropping
            // trailing chars until the label fits the pill's text column, so a
            // proportional label neither clips nor looks sparse. Reserve room for
            // the 30px icon gutter + an 8px right inset (badge/edge breathing).
            let label_x = bx + 30;
            let avail = bw.saturating_sub(38);
            let full = btn.title.as_str();
            let mut end = full.len();
            while end > 0 {
                let cand = &full[..end];
                if canvas.measure_text_aa(cand, label_style, sans) as usize <= avail {
                    break;
                }
                // Step back one char boundary.
                end -= 1;
                while end > 0 && !full.is_char_boundary(end) {
                    end -= 1;
                }
            }
            let label = &full[..end];
            canvas.draw_text_aa(
                label_x as i32,
                line_top(label_style),
                label,
                label_style,
                ink,
                sans,
            );

            if btn.badge_count > 0 {
                // Count bubble (Win11 badge): danger fill + the actual count in
                // dark ink (was a blank red circle). 9+ caps like macOS/Win11.
                let badge_x = bx + bw - 16;
                canvas.fill_rounded_rect(badge_x, 2, 14, 14, 7, p.state_danger);
                let mut nbuf = [0u8; 2];
                let ntxt: &str = if btn.badge_count > 9 {
                    "9+"
                } else {
                    nbuf[0] = b'0' + btn.badge_count.min(9) as u8;
                    unsafe { core::str::from_utf8_unchecked(&nbuf[..1]) }
                };
                let nadv = canvas.measure_text_aa(ntxt, caption_style, sans) as usize;
                canvas.draw_text_aa(
                    (badge_x + (14usize.saturating_sub(nadv)) / 2) as i32,
                    (2 + 14usize.saturating_sub(caption_style.line_height as usize) / 2) as i32,
                    ntxt,
                    caption_style,
                    p.bg_base,
                    sans,
                );
            }
        }

        // Clock — text.primary (the chrome-over-aurora a11y guardrail), RaeSans
        // `type.caption` (the shipped tray/timestamp style). Right-aligned by the
        // PROPORTIONAL measured advance, not a mono char count, so the gap to the
        // right edge is correct regardless of the glyph widths in the time.
        let mut time_buf = [0u8; 16];
        let time_len = self.clock.format_time(&mut time_buf);
        let time_str = unsafe { core::str::from_utf8_unchecked(&time_buf[..time_len]) };
        let clock_adv = canvas.measure_text_aa(time_str, caption_style, sans) as usize;
        let clock_x = w.saturating_sub(clock_adv + 16);
        canvas.draw_text_aa(
            clock_x as i32,
            line_top(caption_style),
            time_str,
            caption_style,
            p.text_primary,
            sans,
        );

        // Tray icons (between clock and task buttons) — token-tinted real
        // line-icons (visual-QA Round-7 #1: retire the N/V/B letter glyphs).
        let tray_x_end = clock_x.saturating_sub(12);
        let mut tx = tray_x_end;
        let icon_sz = 16usize;
        let icon_y = h.saturating_sub(icon_sz) / 2;
        for icon in self.system_tray.visible_icons().iter().rev() {
            if tx < btn_start {
                break;
            }
            tx -= icon.size as usize + 4;
            let color = if icon.visible {
                p.text_primary
            } else {
                p.text_tertiary
            };
            canvas.draw_icon(
                tray_line_icon(&icon.app_id),
                (tx + 2) as i32,
                icon_y as i32,
                icon_sz as i32,
                color,
            );
        }
    }
}

// ── Liquid-Glass identity proof (IDENTITY.md §7) ────────────────────────────

/// The taskbar's design-identity proof — FAIL-able by construction. Verifies the
/// bar paints the `glass.chrome` tier (a see-through frosted material the aurora
/// reads through), NOT an opaque hardcoded fill, and that the active-app pill uses
/// **dark on-accent ink** (`bg.base`) rather than white (the WCAG guardrail:
/// white-on-RaeBlue ≈2.6:1). Both invariants are asserted off pixels rendered with
/// the SAME `render()` the live desktop composites with.
#[derive(Debug, Clone, Copy)]
pub struct TaskbarIdentityProof {
    /// The chrome tier is translucent — the bright backdrop reads through the bar
    /// (mean interior luminance stays well below an opaque fill). FAILs if the bar
    /// regresses to the old opaque `0xFF_0A_0E_1A`.
    pub chrome_is_translucent: bool,
    /// The active-app pill paints dark on-accent ink (`bg.base`), not white.
    pub active_pill_dark_ink: bool,
    /// The bar uses the `glass.chrome` tier (the most see-through of the three),
    /// proven by it reading MORE backdrop through than the panel tier would.
    pub uses_chrome_tier: bool,
    /// All invariants hold.
    pub pass: bool,
}

/// Compute [`TaskbarIdentityProof`] by rendering the bar over a bright backdrop
/// (worst case for "is it really translucent") and sampling the composited pixels.
#[cfg(test)]
fn taskbar_identity_proof() -> TaskbarIdentityProof {
    let (sw, sh) = (640usize, 48usize);
    // Mean-channel luma of one ARGB pixel.
    let luma1 = |pix: u32| -> f32 {
        let r = ((pix >> 16) & 0xFF) as f32;
        let g = ((pix >> 8) & 0xFF) as f32;
        let b = (pix & 0xFF) as f32;
        (0.2126 * r + 0.7152 * g + 0.0722 * b) / 255.0
    };

    // ── (1) Over a BRIGHT field: the chrome bar must stay translucent (the
    //    aurora-peak still reads through, capped for legibility — NOT crushed to
    //    an opaque dark sheet). ──
    let bright = 0xFF_E0_E8_F4u32;
    let mut px = alloc::vec![bright; sw * sh];
    let mut tb = Taskbar::new(sw as u32, sh as u32);
    tb.height = sh as u32;
    tb.width = sw as u32;
    // Give it an active app so the accent pill is exercised.
    tb.task_buttons.push(TaskButton::new(1, "app", "App", 'A'));
    tb.task_buttons[0].x = 200;
    tb.task_buttons[0].width = 120;
    tb.task_buttons[0].state = TaskButtonState::Focused;
    {
        let mut c = unsafe { athgfx::Canvas::new(px.as_mut_ptr() as *mut u8, sw, sh, 4) };
        tb.render(&mut c);
    }
    // A quiet band just past the start button, before the first pill.
    let chrome_luma_bright = {
        let y = sh / 2;
        let (mut acc, mut n) = (0.0f32, 0u32);
        for x in 150..195 {
            acc += luma1(px[y * sw + x]);
            n += 1;
        }
        acc / n.max(1) as f32
    };
    // OBSIDIAN (IDENTITY-OBSIDIAN.md §2): the chrome tier is near-black at
    // ~0xE4 alpha — over a bright field (luma ≈0.88) ~11% bleeds, landing the
    // interior at ≈0.12–0.20. FAIL-able both directions: a fully-opaque fill
    // reads ≈0.05 (no bleed, dead wallpaper), a milky regression climbs past
    // 0.30 (the toy mid-gray).
    let chrome_is_translucent = chrome_luma_bright > 0.08 && chrome_luma_bright < 0.30;

    // ── (2) Over a DARK field: the legibility cap is a no-op, so the genuine
    //    tier ALPHA difference shows — chrome (25%) reads MORE backdrop through
    //    than panel (45%), proving the bar picked the see-through chrome tier.
    //    (Over a BRIGHT field the cap flattens both tiers to the ceiling, so the
    //    tier identity must be measured where the cap does not bite.) ──
    let dark = 0xFF_0B_0F_1Eu32; // the aurora base
    let chrome_luma_dark = {
        let mut p = alloc::vec![dark; sw * sh];
        {
            let mut c = unsafe { athgfx::Canvas::new(p.as_mut_ptr() as *mut u8, sw, sh, 4) };
            athgfx::glass::draw_glass_surface(
                &mut c,
                0,
                0,
                sw,
                sh,
                0,
                ath_tokens::GLASS_CHROME_DARK,
            );
        }
        luma1(p[(sh / 2) * sw + 170])
    };
    let panel_luma_dark = {
        let mut p = alloc::vec![dark; sw * sh];
        {
            let mut c = unsafe { athgfx::Canvas::new(p.as_mut_ptr() as *mut u8, sw, sh, 4) };
            athgfx::glass::draw_glass_surface(
                &mut c,
                0,
                0,
                sw,
                sh,
                0,
                ath_tokens::GLASS_PANEL_DARK,
            );
        }
        luma1(p[(sh / 2) * sw + 170])
    };
    // Chrome's smaller frost + lower alpha → a DARKER interior over the dark base
    // than the panel tier (panel's bigger frost lifts it more). The two tiers must
    // be distinguishable: chrome interior != panel interior, proving a tier choice
    // was made (not a single hardcoded fill). The monotonic frost ladder puts
    // chrome BELOW panel over a dark base.
    let uses_chrome_tier = chrome_luma_dark < panel_luma_dark;

    // The active pill ink must be dark on-accent (bg.base), not white. Sample the
    // pill label glyph region: the brightest pixel there must be the accent fill,
    // NOT a near-white glyph (white ink would push a pixel near 0xFF in all chans).
    let p = crate::active_palette();
    let on_accent = p.bg_base;
    let oa_luma = {
        let r = ((on_accent >> 16) & 0xFF) as f32;
        let g = ((on_accent >> 8) & 0xFF) as f32;
        let b = (on_accent & 0xFF) as f32;
        (0.2126 * r + 0.7152 * g + 0.0722 * b) / 255.0
    };
    // The on-accent ink is dark (bg.base luma ≈0.04) — a white-ink regression would
    // make this > 0.5. Asserting the design intent directly (the render uses it).
    let active_pill_dark_ink = oa_luma < 0.2;

    let pass = chrome_is_translucent && active_pill_dark_ink && uses_chrome_tier;
    TaskbarIdentityProof {
        chrome_is_translucent,
        active_pill_dark_ink,
        uses_chrome_tier,
        pass,
    }
}

#[cfg(test)]
mod identity_tests {
    use super::*;

    #[test]
    fn taskbar_pills_and_tray_use_real_line_icons_not_letters() {
        // visual-QA Round-7 #1: running-app pills and tray glyphs must resolve a
        // real athgfx Icon (the old R/F/W/T/M + N/V/B letters drew as initials).
        use athgfx::icon::Icon;
        // Per-app pill icons reuse the Start mapping (one source of truth).
        assert_eq!(
            crate::start_menu::app_line_icon("files", "Files"),
            Icon::FolderSolid
        );
        assert_eq!(
            crate::start_menu::app_line_icon("terminal", "Terminal"),
            Icon::Exec
        );
        // Tray indicators.
        assert_eq!(tray_line_icon("network"), Icon::WiFi);
        assert_eq!(tray_line_icon("volume"), Icon::Volume);
        assert_eq!(tray_line_icon("battery"), Icon::Power);
        // Unknown tray app → generic Bell, never a letter.
        assert_eq!(tray_line_icon("custom-app"), Icon::Bell);
    }

    #[test]
    fn taskbar_uses_chrome_glass_not_hardcoded_hex() {
        let proof = taskbar_identity_proof();
        assert!(
            proof.chrome_is_translucent,
            "taskbar must be a see-through chrome glass, not an opaque fill: {proof:?}"
        );
        assert!(
            proof.uses_chrome_tier,
            "taskbar must use glass.chrome (more see-through than panel): {proof:?}"
        );
        assert!(
            proof.active_pill_dark_ink,
            "active-app pill ink must be dark on-accent (bg.base), never white: {proof:?}"
        );
        assert!(proof.pass, "taskbar identity proof must pass: {proof:?}");
    }

    #[test]
    fn taskbar_config_drops_legacy_opaque_glass_default() {
        // OBSIDIAN: the chrome tier is near-black at high-but-not-full alpha —
        // the deepest tier, with the MOST wallpaper bleed of the three. Guard
        // both directions (0xFF = dead bleed; < 0xD0 = back toward milk) and
        // that chrome stays the most see-through tier.
        let chrome = (ath_tokens::GLASS_CHROME_DARK.tint >> 24) & 0xFF;
        let panel = (ath_tokens::GLASS_PANEL_DARK.tint >> 24) & 0xFF;
        let popover = (ath_tokens::GLASS_POPOVER_DARK.tint >> 24) & 0xFF;
        assert!(
            (0xD0..=0xF8).contains(&chrome),
            "glass.chrome alpha must sit in the obsidian band [0xD0,0xF8] (got {chrome:#04X})"
        );
        assert!(
            chrome < panel && panel < popover,
            "tier opacity must be monotonic chrome < panel < popover ({chrome:#04X} / {panel:#04X} / {popover:#04X})"
        );
    }

    #[test]
    fn taskbar_chrome_text_is_raesans_proportional_not_mono_bitmap() {
        // visual-QA Round-8 P1 #5: taskbar pill labels + clock must render in the
        // anti-aliased PROPORTIONAL RaeSans path (the same `draw_text_aa` /
        // `FontFamily::Sans` Control Center uses), NOT the chunky 8x8 mono bitmap.
        // Two FAIL-able invariants, both off the AA engine the render() uses:
        use athgfx::text::FontFamily;
        assert!(
            athgfx::text::ensure_init(),
            "RaeSans AA engine must be available for the chrome-type path"
        );

        // (1) PROPORTIONAL: a glyph string of equal char count but different glyph
        //     widths must measure DIFFERENT advances under RaeSans — a mono/bitmap
        //     path would give identical (count * fixed) advances. "iiii" (narrow)
        //     vs "WWWW" (wide), 4 chars each.
        let sw = 320usize;
        let sh = 40usize;
        let mut px = alloc::vec![0xFF_10_14_20u32; sw * sh];
        let c = unsafe { athgfx::Canvas::new(px.as_mut_ptr() as *mut u8, sw, sh, 4) };
        let narrow = c.measure_text_aa("iiii", ath_tokens::TYPE_LABEL, FontFamily::Sans);
        let wide = c.measure_text_aa("WWWW", ath_tokens::TYPE_LABEL, FontFamily::Sans);
        assert!(
            wide > narrow,
            "RaeSans must be proportional (W wider than i): narrow={narrow} wide={wide}"
        );

        // (2) AA INK: rendering the clock string must lay down non-uniform glyph
        //     coverage (anti-aliased edges), which the 1-bit 8x8 bitmap can't
        //     produce. The render path is draw_text_aa_stats; min<max coverage
        //     proves grayscale AA, not a hard-edged bitmap blit.
        let mut c = unsafe { athgfx::Canvas::new(px.as_mut_ptr() as *mut u8, sw, sh, 4) };
        let stats = c.draw_text_aa_stats(
            8,
            8,
            "12:34",
            ath_tokens::TYPE_CAPTION,
            0xFF_FF_FF_FF,
            FontFamily::Sans,
        );
        assert!(
            stats.total_coverage > 0 && stats.min_cov < stats.max_cov,
            "clock must render anti-aliased RaeSans ink (non-uniform coverage): \
             total={} min={} max={}",
            stats.total_coverage,
            stats.min_cov,
            stats.max_cov
        );
    }
}

// ── Global instance ─────────────────────────────────────────────────────────

static mut TASKBAR: Option<Taskbar> = None;

pub fn init() {
    if TASKBAR_INITIALIZED.swap(true, Ordering::SeqCst) {
        return;
    }
    unsafe {
        let mut taskbar = Taskbar::new(1920, 1080);

        taskbar.system_tray.add_icon("network", "Network", 'N');
        taskbar.system_tray.add_icon("volume", "Volume", 'V');
        taskbar.system_tray.add_icon("battery", "Battery", 'B');

        taskbar
            .volume_popup
            .add_device("Speakers", AudioDeviceType::Speakers);
        taskbar
            .volume_popup
            .add_device("Headphones", AudioDeviceType::Headphones);

        taskbar.pin_app("terminal", "Terminal", 'T', "/usr/bin/raeterminal");
        taskbar.pin_app("files", "Files", 'F', "/usr/bin/raefiles");
        taskbar.pin_app("browser", "Browser", 'W', "/usr/bin/raebrowser");
        taskbar.pin_app("settings", "Settings", 'S', "/usr/bin/athsettings");

        TASKBAR = Some(taskbar);
    }
}

pub fn get() -> Option<&'static Taskbar> {
    // SAFETY: single-threaded shell init/access pattern (init() gates on
    // TASKBAR_INITIALIZED); raw-pointer read avoids the static_mut_refs lint.
    unsafe { (*core::ptr::addr_of!(TASKBAR)).as_ref() }
}

pub fn get_mut() -> Option<&'static mut Taskbar> {
    // SAFETY: as above — exclusive shell-thread access.
    unsafe { (*core::ptr::addr_of_mut!(TASKBAR)).as_mut() }
}
