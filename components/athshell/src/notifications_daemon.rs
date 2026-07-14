#![allow(dead_code)]

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

// ═══════════════════════════════════════════════════════════════════════════
// Notification Daemon — D-Bus org.freedesktop.Notifications compliant
// ═══════════════════════════════════════════════════════════════════════════

const DBUS_INTERFACE: &str = "org.freedesktop.Notifications";
const DBUS_PATH: &str = "/org/freedesktop/Notifications";
const SERVER_NAME: &str = "AthenaOS Notification Daemon";
const SERVER_VENDOR: &str = "AthenaOS";
const SERVER_VERSION: &str = "1.0.0";
const SPEC_VERSION: &str = "1.2";
const DEFAULT_EXPIRE_MS: i32 = 5000;
const DEFAULT_HISTORY_MAX: usize = 512;

static NEXT_NOTIFICATION_ID: AtomicU32 = AtomicU32::new(1);
static DAEMON_INITIALIZED: AtomicBool = AtomicBool::new(false);

// ── Urgency ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum Urgency {
    Low = 0,
    Normal = 1,
    Critical = 2,
}

impl Urgency {
    pub fn from_byte(b: u8) -> Self {
        match b {
            0 => Self::Low,
            2 => Self::Critical,
            _ => Self::Normal,
        }
    }
}

// ── Image data (D-Bus image-data hint) ───────────────────────────────────

#[derive(Debug, Clone)]
pub struct ImageData {
    pub width: i32,
    pub height: i32,
    pub rowstride: i32,
    pub has_alpha: bool,
    pub bits_per_sample: i32,
    pub channels: i32,
    pub data: Vec<u8>,
}

impl ImageData {
    pub fn pixel_at(&self, x: usize, y: usize) -> Option<u32> {
        if x >= self.width as usize || y >= self.height as usize {
            return None;
        }
        let offset = y * self.rowstride as usize + x * self.channels as usize;
        if offset + 2 >= self.data.len() {
            return None;
        }
        let r = self.data[offset] as u32;
        let g = self.data[offset + 1] as u32;
        let b = self.data[offset + 2] as u32;
        let a = if self.has_alpha && offset + 3 < self.data.len() {
            self.data[offset + 3] as u32
        } else {
            255
        };
        Some((a << 24) | (r << 16) | (g << 8) | b)
    }
}

// ── Notification hints ───────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct NotificationHints {
    pub urgency: Urgency,
    pub category: Option<String>,
    pub desktop_entry: Option<String>,
    pub image_data: Option<ImageData>,
    pub image_path: Option<String>,
    pub sound_file: Option<String>,
    pub sound_name: Option<String>,
    pub suppress_sound: bool,
    pub transient: bool,
    pub x: Option<i32>,
    pub y: Option<i32>,
    pub action_icons: bool,
    pub resident: bool,
}

impl NotificationHints {
    pub fn new() -> Self {
        Self {
            urgency: Urgency::Normal,
            category: None,
            desktop_entry: None,
            image_data: None,
            image_path: None,
            sound_file: None,
            sound_name: None,
            suppress_sound: false,
            transient: false,
            x: None,
            y: None,
            action_icons: false,
            resident: false,
        }
    }

    pub fn with_urgency(mut self, urgency: Urgency) -> Self {
        self.urgency = urgency;
        self
    }

    pub fn with_category(mut self, cat: &str) -> Self {
        self.category = Some(String::from(cat));
        self
    }

    pub fn with_sound(mut self, name: &str) -> Self {
        self.sound_name = Some(String::from(name));
        self
    }

    pub fn with_position(mut self, x: i32, y: i32) -> Self {
        self.x = Some(x);
        self.y = Some(y);
        self
    }
}

// ── Notification actions ─────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct NotificationAction {
    pub key: String,
    pub label: String,
}

impl NotificationAction {
    pub fn new(key: &str, label: &str) -> Self {
        Self {
            key: String::from(key),
            label: String::from(label),
        }
    }

    pub fn default_action() -> Self {
        Self::new("default", "Default")
    }

    pub fn close_action() -> Self {
        Self::new("close", "Close")
    }
}

// ── Core notification struct ─────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Notification {
    pub id: u32,
    pub app_name: String,
    pub app_icon: String,
    pub summary: String,
    pub body: String,
    pub actions: Vec<NotificationAction>,
    pub hints: NotificationHints,
    pub expire_timeout: i32,
    pub timestamp: u64,
    pub dismissed: bool,
    pub read: bool,
    pub progress: Option<i32>,
}

impl Notification {
    pub fn new(app_name: &str, summary: &str) -> Self {
        Self {
            id: NEXT_NOTIFICATION_ID.fetch_add(1, Ordering::Relaxed),
            app_name: String::from(app_name),
            app_icon: String::new(),
            summary: String::from(summary),
            body: String::new(),
            actions: Vec::new(),
            hints: NotificationHints::new(),
            expire_timeout: DEFAULT_EXPIRE_MS,
            timestamp: 0,
            dismissed: false,
            read: false,
            progress: None,
        }
    }

    pub fn with_body(mut self, body: &str) -> Self {
        self.body = String::from(body);
        self
    }

    pub fn with_icon(mut self, icon: &str) -> Self {
        self.app_icon = String::from(icon);
        self
    }

    pub fn with_timeout(mut self, ms: i32) -> Self {
        self.expire_timeout = ms;
        self
    }

    pub fn with_action(mut self, key: &str, label: &str) -> Self {
        self.actions.push(NotificationAction::new(key, label));
        self
    }

    pub fn with_progress(mut self, pct: i32) -> Self {
        self.progress = Some(pct.clamp(0, 100));
        self
    }

    pub fn with_hints(mut self, hints: NotificationHints) -> Self {
        self.hints = hints;
        self
    }

    pub fn with_timestamp(mut self, ts: u64) -> Self {
        self.timestamp = ts;
        self
    }

    pub fn is_expired(&self, now: u64) -> bool {
        if self.expire_timeout <= 0 {
            return false;
        }
        now.saturating_sub(self.timestamp) >= self.expire_timeout as u64
    }

    pub fn is_critical(&self) -> bool {
        self.hints.urgency == Urgency::Critical
    }
}

// ── Do Not Disturb ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DndMode {
    Off,
    PriorityOnly,
    AlarmsOnly,
    TotalSilence,
    Scheduled {
        start_hour: u8,
        start_min: u8,
        end_hour: u8,
        end_min: u8,
    },
}

impl DndMode {
    pub fn should_suppress(&self, urgency: Urgency, current_hour: u8, current_min: u8) -> bool {
        match self {
            Self::Off => false,
            Self::PriorityOnly => urgency < Urgency::Critical,
            Self::AlarmsOnly => true,
            Self::TotalSilence => true,
            Self::Scheduled {
                start_hour,
                start_min,
                end_hour,
                end_min,
            } => {
                let now = current_hour as u16 * 60 + current_min as u16;
                let start = *start_hour as u16 * 60 + *start_min as u16;
                let end = *end_hour as u16 * 60 + *end_min as u16;
                if start <= end {
                    now >= start && now < end
                } else {
                    now >= start || now < end
                }
            }
        }
    }
}

// ── Per-app notification settings ────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupStyle {
    None,
    ByApp,
    Stacked,
    Automatic,
}

#[derive(Debug, Clone)]
pub struct AppNotificationSettings {
    pub app_id: String,
    pub allowed: bool,
    pub sound_enabled: bool,
    pub banner_enabled: bool,
    pub lock_screen_visible: bool,
    pub badge_count: bool,
    pub group_style: GroupStyle,
    pub priority_override: Option<Urgency>,
}

impl AppNotificationSettings {
    pub fn new(app_id: &str) -> Self {
        Self {
            app_id: String::from(app_id),
            allowed: true,
            sound_enabled: true,
            banner_enabled: true,
            lock_screen_visible: true,
            badge_count: true,
            group_style: GroupStyle::Automatic,
            priority_override: None,
        }
    }

    pub fn silent(app_id: &str) -> Self {
        Self {
            app_id: String::from(app_id),
            allowed: true,
            sound_enabled: false,
            banner_enabled: false,
            lock_screen_visible: false,
            badge_count: true,
            group_style: GroupStyle::ByApp,
            priority_override: None,
        }
    }

    pub fn blocked(app_id: &str) -> Self {
        let mut s = Self::new(app_id);
        s.allowed = false;
        s
    }
}

// ── Notification filtering ───────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterAction {
    Allow,
    Suppress,
    Silent,
    ReduceUrgency,
}

#[derive(Debug, Clone)]
pub enum FilterRule {
    Keyword {
        pattern: String,
        action: FilterAction,
        match_body: bool,
        match_summary: bool,
    },
    TimeBased {
        start_hour: u8,
        start_min: u8,
        end_hour: u8,
        end_min: u8,
        action: FilterAction,
    },
    SenderBased {
        app_name: String,
        action: FilterAction,
    },
    UrgencyBased {
        min_urgency: Urgency,
        action: FilterAction,
    },
}

impl FilterRule {
    pub fn evaluate(
        &self,
        notif: &Notification,
        current_hour: u8,
        current_min: u8,
    ) -> Option<FilterAction> {
        match self {
            Self::Keyword {
                pattern,
                action,
                match_body,
                match_summary,
            } => {
                let in_summary = *match_summary && notif.summary.contains(pattern.as_str());
                let in_body = *match_body && notif.body.contains(pattern.as_str());
                if in_summary || in_body {
                    Some(*action)
                } else {
                    None
                }
            }
            Self::TimeBased {
                start_hour,
                start_min,
                end_hour,
                end_min,
                action,
            } => {
                let now = current_hour as u16 * 60 + current_min as u16;
                let start = *start_hour as u16 * 60 + *start_min as u16;
                let end = *end_hour as u16 * 60 + *end_min as u16;
                let in_range = if start <= end {
                    now >= start && now < end
                } else {
                    now >= start || now < end
                };
                if in_range {
                    Some(*action)
                } else {
                    None
                }
            }
            Self::SenderBased { app_name, action } => {
                if notif.app_name == *app_name {
                    Some(*action)
                } else {
                    None
                }
            }
            Self::UrgencyBased {
                min_urgency,
                action,
            } => {
                if notif.hints.urgency < *min_urgency {
                    Some(*action)
                } else {
                    None
                }
            }
        }
    }
}

// ── Notification sounds ──────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinSound {
    Default,
    MessageNew,
    MessageRead,
    DeviceAdded,
    DeviceRemoved,
    BatteryLow,
    BatteryCritical,
    AlarmClock,
    PhoneRinging,
    ScreenCapture,
    TrashEmpty,
    WindowClose,
    WindowMaximize,
    Complete,
    Error,
}

impl BuiltinSound {
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "default" => Some(Self::Default),
            "message-new-instant" | "message-new-email" => Some(Self::MessageNew),
            "message-read" => Some(Self::MessageRead),
            "device-added" => Some(Self::DeviceAdded),
            "device-removed" => Some(Self::DeviceRemoved),
            "battery-low" => Some(Self::BatteryLow),
            "battery-caution" => Some(Self::BatteryCritical),
            "alarm-clock-elapsed" => Some(Self::AlarmClock),
            "phone-incoming-call" => Some(Self::PhoneRinging),
            "screen-capture" => Some(Self::ScreenCapture),
            "trash-empty" => Some(Self::TrashEmpty),
            "window-close" => Some(Self::WindowClose),
            "complete" | "dialog-information" => Some(Self::Complete),
            "dialog-error" => Some(Self::Error),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SoundConfig {
    pub master_volume: u8,
    pub notification_volume: u8,
    pub custom_sounds: BTreeMap<String, String>,
    pub muted: bool,
}

impl SoundConfig {
    pub fn new() -> Self {
        Self {
            master_volume: 100,
            notification_volume: 80,
            custom_sounds: BTreeMap::new(),
            muted: false,
        }
    }

    pub fn effective_volume(&self) -> u8 {
        if self.muted {
            return 0;
        }
        let combined = (self.master_volume as u16 * self.notification_volume as u16) / 100;
        combined.min(100) as u8
    }

    pub fn register_custom_sound(&mut self, name: &str, path: &str) {
        self.custom_sounds
            .insert(String::from(name), String::from(path));
    }

    pub fn resolve_sound(&self, notif: &Notification) -> Option<SoundPlayback> {
        if self.muted || notif.hints.suppress_sound {
            return None;
        }

        let volume = self.effective_volume();

        if let Some(ref file) = notif.hints.sound_file {
            return Some(SoundPlayback::File {
                path: file.clone(),
                volume,
            });
        }

        if let Some(ref name) = notif.hints.sound_name {
            if let Some(custom_path) = self.custom_sounds.get(name) {
                return Some(SoundPlayback::File {
                    path: custom_path.clone(),
                    volume,
                });
            }
            if let Some(builtin) = BuiltinSound::from_name(name) {
                return Some(SoundPlayback::Builtin {
                    sound: builtin,
                    volume,
                });
            }
        }

        Some(SoundPlayback::Builtin {
            sound: BuiltinSound::Default,
            volume,
        })
    }
}

#[derive(Debug, Clone)]
pub enum SoundPlayback {
    Builtin { sound: BuiltinSound, volume: u8 },
    File { path: String, volume: u8 },
}

// ── Notification group ───────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct NotificationGroup {
    pub app_name: String,
    pub ids: Vec<u32>,
    pub collapsed: bool,
    pub summary_text: String,
}

impl NotificationGroup {
    pub fn new(app_name: &str) -> Self {
        Self {
            app_name: String::from(app_name),
            ids: Vec::new(),
            collapsed: true,
            summary_text: String::new(),
        }
    }

    pub fn add(&mut self, id: u32) {
        self.ids.push(id);
        self.update_summary();
    }

    pub fn remove(&mut self, id: u32) {
        self.ids.retain(|&i| i != id);
        self.update_summary();
    }

    fn update_summary(&mut self) {
        self.summary_text.clear();
        let count = self.ids.len();
        if count == 0 {
            return;
        }
        self.summary_text.push_str(&self.app_name);
        self.summary_text.push_str(": ");
        let mut buf = [0u8; 10];
        let s = fmt_usize(count, &mut buf);
        self.summary_text.push_str(s);
        self.summary_text.push_str(" notification");
        if count != 1 {
            self.summary_text.push('s');
        }
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    pub fn count(&self) -> usize {
        self.ids.len()
    }

    pub fn toggle_collapse(&mut self) {
        self.collapsed = !self.collapsed;
    }
}

// ── Toast layout / rendering model ───────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastPosition {
    TopRight,
    TopLeft,
    TopCenter,
    BottomRight,
    BottomLeft,
    BottomCenter,
}

#[derive(Debug, Clone, Copy)]
pub struct ToastGeometry {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Copy)]
pub struct ToastLayoutConfig {
    pub position: ToastPosition,
    pub max_visible: usize,
    pub toast_width: u32,
    pub toast_height: u32,
    pub toast_spacing: u32,
    pub margin_x: u32,
    pub margin_y: u32,
    pub progress_bar_height: u32,
    pub icon_size: u32,
    pub action_button_height: u32,
}

impl ToastLayoutConfig {
    pub fn new() -> Self {
        Self {
            position: ToastPosition::TopRight,
            max_visible: 5,
            toast_width: 360,
            toast_height: 90,
            toast_spacing: 8,
            margin_x: 16,
            margin_y: 40,
            progress_bar_height: 4,
            icon_size: 32,
            action_button_height: 28,
        }
    }

    pub fn compute_toast_position(
        &self,
        index: usize,
        screen_width: u32,
        screen_height: u32,
    ) -> ToastGeometry {
        let slot_height = self.toast_height + self.toast_spacing;
        let vertical_offset = self.margin_y + (index as u32 * slot_height);

        let (x, y) = match self.position {
            ToastPosition::TopRight => {
                let x = screen_width as i32 - self.toast_width as i32 - self.margin_x as i32;
                (x, vertical_offset as i32)
            }
            ToastPosition::TopLeft => (self.margin_x as i32, vertical_offset as i32),
            ToastPosition::TopCenter => {
                let x = (screen_width as i32 - self.toast_width as i32) / 2;
                (x, vertical_offset as i32)
            }
            ToastPosition::BottomRight => {
                let x = screen_width as i32 - self.toast_width as i32 - self.margin_x as i32;
                let y = screen_height as i32 - vertical_offset as i32 - self.toast_height as i32;
                (x, y)
            }
            ToastPosition::BottomLeft => {
                let y = screen_height as i32 - vertical_offset as i32 - self.toast_height as i32;
                (self.margin_x as i32, y)
            }
            ToastPosition::BottomCenter => {
                let x = (screen_width as i32 - self.toast_width as i32) / 2;
                let y = screen_height as i32 - vertical_offset as i32 - self.toast_height as i32;
                (x, y)
            }
        };

        ToastGeometry {
            x,
            y,
            width: self.toast_width,
            height: self.toast_height,
        }
    }

    pub fn expanded_height(&self, notif: &Notification) -> u32 {
        let mut h = self.toast_height;
        if !notif.actions.is_empty() {
            h += self.action_button_height + 4;
        }
        if notif.progress.is_some() {
            h += self.progress_bar_height + 8;
        }
        h
    }
}

// ── Focus assist / gaming mode ───────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusMode {
    Normal,
    FocusAssist,
    Gaming,
    Presenting,
    Sleeping,
}

impl FocusMode {
    pub fn allows_notification(&self, urgency: Urgency) -> bool {
        match self {
            Self::Normal => true,
            Self::FocusAssist => urgency >= Urgency::Normal,
            Self::Gaming => urgency == Urgency::Critical,
            Self::Presenting => urgency == Urgency::Critical,
            Self::Sleeping => false,
        }
    }

    pub fn allows_sound(&self) -> bool {
        matches!(self, Self::Normal)
    }

    pub fn allows_banner(&self) -> bool {
        matches!(self, Self::Normal | Self::FocusAssist)
    }
}

// ── Action invocation result ─────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseReason {
    Expired,
    DismissedByUser,
    ClosedByApi,
    Undefined,
}

#[derive(Debug, Clone)]
pub enum DaemonEvent {
    NotificationPosted { id: u32 },
    NotificationClosed { id: u32, reason: CloseReason },
    ActionInvoked { id: u32, action_key: String },
    DndChanged { mode: DndMode },
    FocusChanged { mode: FocusMode },
}

// ── Notification history ─────────────────────────────────────────────────

pub struct NotificationHistory {
    entries: Vec<Notification>,
    max_entries: usize,
}

impl NotificationHistory {
    pub fn new(max_entries: usize) -> Self {
        Self {
            entries: Vec::new(),
            max_entries,
        }
    }

    pub fn record(&mut self, notif: Notification) {
        if self.entries.len() >= self.max_entries {
            self.entries.remove(0);
        }
        self.entries.push(notif);
    }

    pub fn get(&self, id: u32) -> Option<&Notification> {
        self.entries.iter().find(|n| n.id == id)
    }

    pub fn entries(&self) -> &[Notification] {
        &self.entries
    }

    pub fn entries_for_app(&self, app_name: &str) -> Vec<&Notification> {
        self.entries
            .iter()
            .filter(|n| n.app_name == app_name)
            .collect()
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    pub fn clear_for_app(&mut self, app_name: &str) {
        self.entries.retain(|n| n.app_name != app_name);
    }

    pub fn count(&self) -> usize {
        self.entries.len()
    }

    pub fn recent(&self, count: usize) -> &[Notification] {
        let start = self.entries.len().saturating_sub(count);
        &self.entries[start..]
    }
}

// ── Server info (D-Bus GetServerInformation) ─────────────────────────────

#[derive(Debug, Clone)]
pub struct ServerInfo {
    pub name: String,
    pub vendor: String,
    pub version: String,
    pub spec_version: String,
}

impl ServerInfo {
    pub fn athena() -> Self {
        Self {
            name: String::from(SERVER_NAME),
            vendor: String::from(SERVER_VENDOR),
            version: String::from(SERVER_VERSION),
            spec_version: String::from(SPEC_VERSION),
        }
    }
}

// ── Capabilities (D-Bus GetCapabilities) ─────────────────────────────────

pub fn server_capabilities() -> Vec<String> {
    alloc::vec![
        String::from("actions"),
        String::from("action-icons"),
        String::from("body"),
        String::from("body-hyperlinks"),
        String::from("body-images"),
        String::from("body-markup"),
        String::from("icon-multi"),
        String::from("icon-static"),
        String::from("persistence"),
        String::from("sound"),
    ]
}

// ═══════════════════════════════════════════════════════════════════════════
// NotificationDaemon — the global daemon instance
// ═══════════════════════════════════════════════════════════════════════════

pub struct NotificationDaemon {
    active: BTreeMap<u32, Notification>,
    groups: BTreeMap<String, NotificationGroup>,
    history: NotificationHistory,
    app_settings: BTreeMap<String, AppNotificationSettings>,
    filter_rules: Vec<FilterRule>,
    sound_config: SoundConfig,
    toast_layout: ToastLayoutConfig,
    dnd_mode: DndMode,
    focus_mode: FocusMode,
    events: Vec<DaemonEvent>,
    server_info: ServerInfo,
    screen_width: u32,
    screen_height: u32,
}

impl NotificationDaemon {
    pub fn new(screen_width: u32, screen_height: u32) -> Self {
        Self {
            active: BTreeMap::new(),
            groups: BTreeMap::new(),
            history: NotificationHistory::new(DEFAULT_HISTORY_MAX),
            app_settings: BTreeMap::new(),
            filter_rules: Vec::new(),
            sound_config: SoundConfig::new(),
            toast_layout: ToastLayoutConfig::new(),
            dnd_mode: DndMode::Off,
            focus_mode: FocusMode::Normal,
            events: Vec::new(),
            server_info: ServerInfo::athena(),
            screen_width,
            screen_height,
        }
    }

    pub fn notify(
        &mut self,
        app_name: &str,
        replaces_id: u32,
        app_icon: &str,
        summary: &str,
        body: &str,
        actions: &[(&str, &str)],
        hints: NotificationHints,
        expire_timeout: i32,
        now: u64,
        current_hour: u8,
        current_min: u8,
    ) -> u32 {
        if let Some(settings) = self.app_settings.get(app_name) {
            if !settings.allowed {
                return 0;
            }
        }

        let timeout = if expire_timeout < 0 {
            DEFAULT_EXPIRE_MS
        } else {
            expire_timeout
        };

        let mut notif = Notification::new(app_name, summary)
            .with_body(body)
            .with_icon(app_icon)
            .with_timeout(timeout)
            .with_hints(hints)
            .with_timestamp(now);

        for &(key, label) in actions {
            notif.actions.push(NotificationAction::new(key, label));
        }

        if replaces_id > 0 && self.active.contains_key(&replaces_id) {
            notif.id = replaces_id;
            NEXT_NOTIFICATION_ID.fetch_sub(1, Ordering::Relaxed);
        }

        let id = notif.id;

        for rule in &self.filter_rules {
            if let Some(action) = rule.evaluate(&notif, current_hour, current_min) {
                match action {
                    FilterAction::Suppress => return 0,
                    FilterAction::Silent => {
                        notif.hints.suppress_sound = true;
                    }
                    FilterAction::ReduceUrgency => {
                        notif.hints.urgency = Urgency::Low;
                    }
                    FilterAction::Allow => {}
                }
            }
        }

        if self
            .dnd_mode
            .should_suppress(notif.hints.urgency, current_hour, current_min)
        {
            self.history.record(notif);
            return 0;
        }

        if !self.focus_mode.allows_notification(notif.hints.urgency) {
            self.history.record(notif);
            return 0;
        }

        let group = self
            .groups
            .entry(notif.app_name.clone())
            .or_insert_with(|| NotificationGroup::new(&notif.app_name));
        group.add(id);

        self.active.insert(id, notif);
        self.events.push(DaemonEvent::NotificationPosted { id });
        id
    }

    pub fn close_notification(&mut self, id: u32, reason: CloseReason) {
        if let Some(notif) = self.active.remove(&id) {
            if let Some(group) = self.groups.get_mut(&notif.app_name) {
                group.remove(id);
            }
            self.history.record(notif);
            self.events
                .push(DaemonEvent::NotificationClosed { id, reason });
        }
    }

    pub fn invoke_action(&mut self, id: u32, action_key: &str) {
        self.events.push(DaemonEvent::ActionInvoked {
            id,
            action_key: String::from(action_key),
        });

        if let Some(notif) = self.active.get(&id) {
            if !notif.hints.resident {
                let app = notif.app_name.clone();
                if let Some(removed) = self.active.remove(&id) {
                    if let Some(group) = self.groups.get_mut(&app) {
                        group.remove(id);
                    }
                    self.history.record(removed);
                }
            }
        }
    }

    pub fn tick(&mut self, now: u64) {
        let expired: Vec<u32> = self
            .active
            .iter()
            .filter(|(_, n)| n.is_expired(now) && !n.is_critical())
            .map(|(&id, _)| id)
            .collect();

        for id in expired {
            self.close_notification(id, CloseReason::Expired);
        }
    }

    pub fn set_dnd(&mut self, mode: DndMode) {
        self.dnd_mode = mode;
        self.events.push(DaemonEvent::DndChanged { mode });
    }

    pub fn set_focus_mode(&mut self, mode: FocusMode) {
        self.focus_mode = mode;
        self.events.push(DaemonEvent::FocusChanged { mode });
    }

    pub fn configure_app(&mut self, settings: AppNotificationSettings) {
        self.app_settings.insert(settings.app_id.clone(), settings);
    }

    pub fn add_filter_rule(&mut self, rule: FilterRule) {
        self.filter_rules.push(rule);
    }

    pub fn clear_filter_rules(&mut self) {
        self.filter_rules.clear();
    }

    pub fn sound_config_mut(&mut self) -> &mut SoundConfig {
        &mut self.sound_config
    }

    pub fn toast_layout_mut(&mut self) -> &mut ToastLayoutConfig {
        &mut self.toast_layout
    }

    pub fn active_count(&self) -> usize {
        self.active.len()
    }

    pub fn active_notifications(&self) -> Vec<&Notification> {
        self.active.values().collect()
    }

    pub fn visible_toasts(&self) -> Vec<(&Notification, ToastGeometry)> {
        let mut toasts: Vec<&Notification> =
            self.active.values().filter(|n| !n.dismissed).collect();
        toasts.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
        toasts.truncate(self.toast_layout.max_visible);

        toasts
            .iter()
            .enumerate()
            .map(|(i, notif)| {
                let geo = self.toast_layout.compute_toast_position(
                    i,
                    self.screen_width,
                    self.screen_height,
                );
                (*notif, geo)
            })
            .collect()
    }

    pub fn groups(&self) -> &BTreeMap<String, NotificationGroup> {
        &self.groups
    }

    pub fn history(&self) -> &NotificationHistory {
        &self.history
    }

    pub fn drain_events(&mut self) -> Vec<DaemonEvent> {
        let events = core::mem::take(&mut self.events);
        events
    }

    pub fn server_info(&self) -> &ServerInfo {
        &self.server_info
    }

    pub fn capabilities(&self) -> Vec<String> {
        server_capabilities()
    }

    pub fn dismiss_all(&mut self) {
        let ids: Vec<u32> = self.active.keys().copied().collect();
        for id in ids {
            self.close_notification(id, CloseReason::DismissedByUser);
        }
    }

    pub fn dismiss_for_app(&mut self, app_name: &str) {
        let ids: Vec<u32> = self
            .active
            .iter()
            .filter(|(_, n)| n.app_name == app_name)
            .map(|(&id, _)| id)
            .collect();
        for id in ids {
            self.close_notification(id, CloseReason::DismissedByUser);
        }
    }

    pub fn unread_count(&self) -> usize {
        self.active.values().filter(|n| !n.read).count()
    }

    pub fn mark_read(&mut self, id: u32) {
        if let Some(notif) = self.active.get_mut(&id) {
            notif.read = true;
        }
    }

    pub fn mark_all_read(&mut self) {
        for notif in self.active.values_mut() {
            notif.read = true;
        }
    }

    pub fn resolve_sound(&self, id: u32) -> Option<SoundPlayback> {
        let notif = self.active.get(&id)?;
        if !self.focus_mode.allows_sound() {
            return None;
        }
        if let Some(settings) = self.app_settings.get(&notif.app_name) {
            if !settings.sound_enabled {
                return None;
            }
        }
        self.sound_config.resolve_sound(notif)
    }
}

// ── Colour palette ───────────────────────────────────────────────────────
//
// Toasts and the notification center are LUMINOUS FROSTED CARDS on the Aurora
// backdrop (visual-QA Round-4 P0 #2 — they were dark athshell slates punched into
// the now-luminous glass). The fill is the popover glass tier (the brightest, most
// opaque tier — transient surfaces need instant legibility): `popover.tint`
// (translucent slate the backdrop reads through) → `popover.frost` (the WHITE
// luminance-add sheen that lifts the card ABOVE the backdrop). Foreground ink
// resolves from the LIVE palette / accent ramp (Vibe-Mode cohesion); urgency
// colours track the accent except Critical (a fixed semantic red).

/// One frosted toast/center card = popover tint then frost, composited over the
/// already-drawn backdrop, in `glass_tier_interior` order (so it reads as a raised
/// frosted element, never a dark hole). Token-derived — no hardcoded slate.
fn draw_frosted_card(
    canvas: &mut athgfx::Canvas,
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    radius: usize,
) {
    canvas.fill_rounded_rect(x, y, w, h, radius, ath_tokens::GLASS_POPOVER_DARK.tint);
    canvas.fill_rounded_rect(x, y, w, h, radius, ath_tokens::GLASS_POPOVER_DARK.frost);
}

/// Replace the alpha channel of an ARGB colour, keeping RGB (surface-local; the
/// ath_tokens `with_alpha` is private). Used for the critical-toast glow halo.
#[inline]
const fn with_alpha(color: u32, alpha: u32) -> u32 {
    (color & 0x00_FF_FF_FF) | ((alpha & 0xFF) << 24)
}

const ND_CRITICAL: u32 = 0xFF_FF_44_44;
const GLYPH_W: usize = 8;
const GLYPH_H: usize = 8;
/// Toast/card corner radius — `radius.lg` (16), matching the Control Center panel
/// + the other glass surfaces (visual-QA "corner radii consistent" — no defect).
const ND_RADIUS: usize = ath_tokens::RADIUS_LG as usize;

// ── Rendering ────────────────────────────────────────────────────────────

impl NotificationDaemon {
    pub fn render_toasts(&self, canvas: &mut athgfx::Canvas) {
        let p = crate::active_palette();
        let a = crate::accent();
        let toasts = self.visible_toasts();
        for (notif, geo) in &toasts {
            let x = geo.x.max(0) as usize;
            let y = geo.y.max(0) as usize;
            let w = geo.width as usize;
            let h = geo.height as usize;

            // Luminous frosted CARD on the backdrop (Round-4 P0 #2 — kill the
            // dark slate). Critical toasts add an accent-glow halo in the semantic
            // red so a critical alert still reads urgent against the frost.
            if notif.is_critical() {
                canvas.draw_rounded_rect_outline(
                    x.saturating_sub(1),
                    y.saturating_sub(1),
                    w + 2,
                    h + 2,
                    ND_RADIUS,
                    with_alpha(ND_CRITICAL, 0x66),
                );
            }
            draw_frosted_card(canvas, x, y, w, h, ND_RADIUS);
            canvas.draw_rounded_rect_outline(x, y, w, h, ND_RADIUS, p.stroke_subtle);

            // Urgency colour tracks the accent (Vibe cohesion); Critical = the
            // fixed semantic red. Body/summary ink from the live palette.
            let title_color = match notif.hints.urgency {
                Urgency::Critical => ND_CRITICAL,
                _ => a.text,
            };

            // Accessibility audit P1: the app-name label used `text.secondary`,
            // which FAILS 4.5:1 over the now-LUMINOUS frosted toast (L84-93 after
            // the Round-4 lift). athena-ui confirmed: PROMOTE to `text.primary` over
            // the bright surface (don't darken the token). The dimmed close "x"
            // stays tertiary (a non-essential affordance).
            canvas.draw_text_aa(
                (x + 12) as i32,
                (y + 8) as i32,
                &notif.app_name,
                ath_tokens::TYPE_CAPTION,
                p.text_primary,
                athgfx::text::FontFamily::Sans,
            );
            canvas.draw_text_aa(
                (x + w - 18) as i32,
                (y + 8) as i32,
                "x",
                ath_tokens::TYPE_CAPTION,
                p.text_tertiary,
                athgfx::text::FontFamily::Sans,
            );

            let max_chars = (w.saturating_sub(24)) / GLYPH_W;
            let summary = crate::text_util::truncate_chars(&notif.summary, max_chars);
            canvas.draw_text_aa(
                (x + 12) as i32,
                (y + 26) as i32,
                summary,
                ath_tokens::TYPE_LABEL,
                title_color,
                athgfx::text::FontFamily::Sans,
            );

            if !notif.body.is_empty() {
                let body = crate::text_util::truncate_chars(&notif.body, max_chars);
                canvas.draw_text_aa(
                    (x + 12) as i32,
                    (y + 44) as i32,
                    body,
                    ath_tokens::TYPE_BODY,
                    p.text_primary,
                    athgfx::text::FontFamily::Sans,
                );
            }

            if let Some(progress) = notif.progress {
                let bar_y = y + h - 8;
                let bar_w = w - 24;
                let bar_r = ath_tokens::radius_pill(4) as usize;
                canvas.fill_rounded_rect(x + 12, bar_y, bar_w, 4, bar_r, p.bg_overlay);
                let fill = (bar_w as i32 * progress / 100).max(0) as usize;
                canvas.fill_rounded_rect(x + 12, bar_y, fill, 4, bar_r, a.base);
            }

            if !notif.actions.is_empty() {
                let act_y = y + h - 24;
                let mut ax = x + 12;
                for action in notif.actions.iter().take(3) {
                    let aw = action.label.len() * GLYPH_W + 16;
                    let ah = GLYPH_H + 10;
                    // Accent-glow PILL action button (Round-4 P1 #6).
                    let pill_r = ath_tokens::radius_pill(ah as u32) as usize;
                    canvas.fill_rounded_rect(ax, act_y, aw, ah, pill_r, a.subtle);
                    canvas.draw_rounded_rect_outline(ax, act_y, aw, ah, pill_r, a.glow);
                    canvas.draw_text_aa(
                        (ax + 8) as i32,
                        (act_y + 4) as i32,
                        &action.label,
                        ath_tokens::TYPE_CAPTION,
                        a.text,
                        athgfx::text::FontFamily::Sans,
                    );
                    ax += aw + 6;
                }
            }
        }
    }

    pub fn render_center(
        &self,
        canvas: &mut athgfx::Canvas,
        ox: usize,
        oy: usize,
        w: usize,
        h: usize,
    ) {
        let p = crate::active_palette();
        let a = crate::accent();
        // Notification-center panel = the panel glass tier as a frosted surface
        // (Round-4 P0 #2), not a dark slate.
        canvas.fill_rounded_rect(ox, oy, w, h, ND_RADIUS, ath_tokens::GLASS_PANEL_DARK.tint);
        canvas.fill_rounded_rect(ox, oy, w, h, ND_RADIUS, ath_tokens::GLASS_PANEL_DARK.frost);
        canvas.draw_rounded_rect_outline(ox, oy, w, h, ND_RADIUS, p.stroke_subtle);
        canvas.draw_text_aa(
            (ox + 12) as i32,
            (oy + 10) as i32,
            "Notifications",
            ath_tokens::TYPE_SUBTITLE,
            p.text_primary,
            athgfx::text::FontFamily::Sans,
        );

        let mut buf = [0u8; 10];
        let count_str = fmt_usize(self.active_count(), &mut buf);
        canvas.draw_text_aa(
            (ox + w - 60) as i32,
            (oy + 10) as i32,
            count_str,
            ath_tokens::TYPE_CAPTION,
            p.text_secondary,
            athgfx::text::FontFamily::Sans,
        );

        let list_y = oy + 32;
        let item_h = 56usize;
        let max_visible = (h.saturating_sub(36)) / item_h;

        let mut items: Vec<&Notification> = self.active.values().collect();
        items.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));

        for (i, notif) in items.iter().take(max_visible).enumerate() {
            let ny = list_y + i * item_h;
            if ny + item_h > oy + h {
                break;
            }

            // Each row = a luminous frosted CARD on the center panel; read rows are
            // dimmer (less frost) but still ON the glass, never a dark hole.
            let row_x = ox + 6;
            let row_w = w - 12;
            let row_h = item_h - 4;
            canvas.fill_rounded_rect(
                row_x,
                ny,
                row_w,
                row_h,
                ath_tokens::RADIUS_MD as usize,
                ath_tokens::GLASS_POPOVER_DARK.tint,
            );
            let frost = if notif.read {
                ath_tokens::GLASS_CHROME_DARK.frost
            } else {
                ath_tokens::GLASS_POPOVER_DARK.frost
            };
            canvas.fill_rounded_rect(
                row_x,
                ny,
                row_w,
                row_h,
                ath_tokens::RADIUS_MD as usize,
                frost,
            );

            let title_color = match notif.hints.urgency {
                Urgency::Critical => ND_CRITICAL,
                _ => p.text_primary,
            };

            canvas.draw_text_aa(
                (ox + 14) as i32,
                (ny + 6) as i32,
                &notif.app_name,
                ath_tokens::TYPE_CAPTION,
                a.text,
                athgfx::text::FontFamily::Sans,
            );
            let max_ch = (w.saturating_sub(32)) / GLYPH_W;
            let summary = crate::text_util::truncate_chars(&notif.summary, max_ch);
            canvas.draw_text_aa(
                (ox + 14) as i32,
                (ny + 20) as i32,
                summary,
                ath_tokens::TYPE_LABEL,
                title_color,
                athgfx::text::FontFamily::Sans,
            );

            if !notif.body.is_empty() {
                let body = crate::text_util::truncate_chars(&notif.body, max_ch);
                // Accessibility audit P1: history-row body promoted `text.secondary`
                // → `text.primary` — it sits over a luminous frosted card (popover/
                // chrome frost) where secondary fails 4.5:1. athena-ui: promote, don't
                // darken the token.
                canvas.draw_text_aa(
                    (ox + 14) as i32,
                    (ny + 36) as i32,
                    body,
                    ath_tokens::TYPE_BODY,
                    p.text_primary,
                    athgfx::text::FontFamily::Sans,
                );
            }
        }
    }
}

// ── Global instance ──────────────────────────────────────────────────────

static mut DAEMON: Option<NotificationDaemon> = None;

pub unsafe fn init(screen_width: u32, screen_height: u32) {
    if !DAEMON_INITIALIZED.swap(true, Ordering::SeqCst) {
        DAEMON = Some(NotificationDaemon::new(screen_width, screen_height));
    }
}

pub unsafe fn daemon() -> &'static mut NotificationDaemon {
    DAEMON
        .as_mut()
        .expect("NotificationDaemon not initialized — call init() first")
}

pub unsafe fn is_initialized() -> bool {
    DAEMON_INITIALIZED.load(Ordering::SeqCst)
}

// ── Helpers ──────────────────────────────────────────────────────────────

fn fmt_usize(mut n: usize, buf: &mut [u8; 10]) -> &str {
    if n == 0 {
        buf[0] = b'0';
        return unsafe { core::str::from_utf8_unchecked(&buf[..1]) };
    }
    let mut pos = 10;
    while n > 0 && pos > 0 {
        pos -= 1;
        buf[pos] = b'0' + (n % 10) as u8;
        n /= 10;
    }
    unsafe { core::str::from_utf8_unchecked(&buf[pos..10]) }
}
