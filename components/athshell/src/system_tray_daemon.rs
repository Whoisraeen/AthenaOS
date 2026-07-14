#![allow(dead_code)]

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

// ═══════════════════════════════════════════════════════════════════════════
// System Tray Daemon — Notification area icon management for AthenaOS
// ═══════════════════════════════════════════════════════════════════════════

static TRAY_INITIALIZED: AtomicBool = AtomicBool::new(false);
static NEXT_ICON_ID: AtomicU64 = AtomicU64::new(1);
static NEXT_NOTIFICATION_ID: AtomicU64 = AtomicU64::new(1);

const MAX_VISIBLE_ICONS: usize = 12;
const MAX_OVERFLOW_ICONS: usize = 32;
const TOOLTIP_DELAY_MS: u64 = 500;
const BALLOON_DEFAULT_TIMEOUT_MS: u64 = 5000;

// ── Icon types and states ────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconState {
    Normal,
    Active,
    Disabled,
    Attention,
    Hidden,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnimationType {
    None,
    Pulsing,
    Spinning,
    Bouncing,
    Blinking,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClickType {
    LeftClick,
    RightClick,
    DoubleClick,
    MiddleClick,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemIconType {
    Volume,
    Network,
    Battery,
    Bluetooth,
    Display,
    Location,
    Microphone,
    Camera,
    UsbSafeRemove,
    Printer,
    UpdatesAvailable,
    Vpn,
    Clock,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VolumeState {
    Muted,
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkState {
    Disconnected,
    Ethernet,
    WifiWeak,
    WifiFair,
    WifiGood,
    WifiExcellent,
    Airplane,
    Metered,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatteryState {
    Charging,
    Discharging,
    Full,
    BatterySaver,
    Critical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BluetoothState {
    Off,
    On,
    Connected,
    Discovering,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VpnState {
    Disconnected,
    Connecting,
    Connected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeVariant {
    Dark,
    Light,
    HighContrast,
}

// ── Badge ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct IconBadge {
    pub text: String,
    pub color: u32,
    pub visible: bool,
}

impl IconBadge {
    pub fn new(text: &str, color: u32) -> Self {
        Self {
            text: String::from(text),
            color,
            visible: true,
        }
    }

    pub fn number(n: u32, color: u32) -> Self {
        let mut s = String::new();
        if n > 99 {
            s.push_str("99+");
        } else {
            let mut buf = [0u8; 4];
            let mut val = n;
            let mut pos = 4;
            if val == 0 {
                s.push('0');
            } else {
                while val > 0 {
                    pos -= 1;
                    buf[pos] = b'0' + (val % 10) as u8;
                    val /= 10;
                }
                for &b in &buf[pos..4] {
                    s.push(b as char);
                }
            }
        }
        Self {
            text: s,
            color,
            visible: true,
        }
    }
}

// ── Context menu ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuItemType {
    Normal,
    Separator,
    Checkbox,
    Radio,
    Submenu,
}

#[derive(Debug, Clone)]
pub struct MenuItem {
    pub id: u64,
    pub label: String,
    pub icon_char: Option<char>,
    pub enabled: bool,
    pub checked: bool,
    pub item_type: MenuItemType,
    pub accelerator: Option<String>,
    pub submenu: Option<Vec<MenuItem>>,
}

impl MenuItem {
    pub fn new(label: &str) -> Self {
        Self {
            id: NEXT_ICON_ID.fetch_add(1, Ordering::Relaxed),
            label: String::from(label),
            icon_char: None,
            enabled: true,
            checked: false,
            item_type: MenuItemType::Normal,
            accelerator: None,
            submenu: None,
        }
    }

    pub fn separator() -> Self {
        Self {
            id: 0,
            label: String::new(),
            icon_char: None,
            enabled: false,
            checked: false,
            item_type: MenuItemType::Separator,
            accelerator: None,
            submenu: None,
        }
    }

    pub fn checkbox(label: &str, checked: bool) -> Self {
        Self {
            id: NEXT_ICON_ID.fetch_add(1, Ordering::Relaxed),
            label: String::from(label),
            icon_char: None,
            enabled: true,
            checked,
            item_type: MenuItemType::Checkbox,
            accelerator: None,
            submenu: None,
        }
    }

    pub fn with_icon(mut self, icon: char) -> Self {
        self.icon_char = Some(icon);
        self
    }

    pub fn with_accelerator(mut self, accel: &str) -> Self {
        self.accelerator = Some(String::from(accel));
        self
    }

    pub fn with_submenu(mut self, items: Vec<MenuItem>) -> Self {
        self.item_type = MenuItemType::Submenu;
        self.submenu = Some(items);
        self
    }

    pub fn disabled(mut self) -> Self {
        self.enabled = false;
        self
    }
}

#[derive(Debug, Clone)]
pub struct ContextMenu {
    pub items: Vec<MenuItem>,
    pub visible: bool,
    pub x: i32,
    pub y: i32,
    pub selected_index: Option<usize>,
    pub active_submenu: Option<usize>,
}

impl ContextMenu {
    pub fn new(items: Vec<MenuItem>) -> Self {
        Self {
            items,
            visible: false,
            x: 0,
            y: 0,
            selected_index: None,
            active_submenu: None,
        }
    }

    pub fn show_at(&mut self, x: i32, y: i32) {
        self.visible = true;
        self.x = x;
        self.y = y;
        self.selected_index = None;
        self.active_submenu = None;
    }

    pub fn hide(&mut self) {
        self.visible = false;
        self.selected_index = None;
        self.active_submenu = None;
    }

    pub fn select_next(&mut self) {
        let count = self.items.len();
        if count == 0 {
            return;
        }
        let mut idx = self.selected_index.map(|i| i + 1).unwrap_or(0);
        while idx < count && self.items[idx].item_type == MenuItemType::Separator {
            idx += 1;
        }
        if idx < count {
            self.selected_index = Some(idx);
        }
    }

    pub fn select_prev(&mut self) {
        let count = self.items.len();
        if count == 0 {
            return;
        }
        let mut idx = self.selected_index.unwrap_or(count).wrapping_sub(1);
        while idx < count && self.items[idx].item_type == MenuItemType::Separator {
            idx = idx.wrapping_sub(1);
        }
        if idx < count {
            self.selected_index = Some(idx);
        }
    }

    pub fn activate_selected(&mut self) -> Option<u64> {
        if let Some(idx) = self.selected_index {
            if idx < self.items.len() && self.items[idx].enabled {
                if self.items[idx].item_type == MenuItemType::Checkbox {
                    self.items[idx].checked = !self.items[idx].checked;
                }
                return Some(self.items[idx].id);
            }
        }
        None
    }
}

// ── Balloon/toast notification ───────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BalloonIcon {
    None,
    Info,
    Warning,
    Error,
    Custom(char),
}

#[derive(Debug, Clone)]
pub struct BalloonNotification {
    pub id: u64,
    pub title: String,
    pub message: String,
    pub icon: BalloonIcon,
    pub timeout_ms: u64,
    pub created_at: u64,
    pub click_action_id: Option<u64>,
    pub show_close: bool,
    pub dismissed: bool,
}

impl BalloonNotification {
    pub fn new(title: &str, message: &str) -> Self {
        Self {
            id: NEXT_NOTIFICATION_ID.fetch_add(1, Ordering::Relaxed),
            title: String::from(title),
            message: String::from(message),
            icon: BalloonIcon::Info,
            timeout_ms: BALLOON_DEFAULT_TIMEOUT_MS,
            created_at: 0,
            click_action_id: None,
            show_close: true,
            dismissed: false,
        }
    }

    pub fn with_icon(mut self, icon: BalloonIcon) -> Self {
        self.icon = icon;
        self
    }

    pub fn with_timeout(mut self, ms: u64) -> Self {
        self.timeout_ms = ms;
        self
    }

    pub fn with_click_action(mut self, action_id: u64) -> Self {
        self.click_action_id = Some(action_id);
        self
    }

    pub fn is_expired(&self, current_time: u64) -> bool {
        self.timeout_ms > 0 && current_time.saturating_sub(self.created_at) >= self.timeout_ms
    }

    pub fn dismiss(&mut self) {
        self.dismissed = true;
    }
}

// ── Tray icon ────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct TrayIconEntry {
    pub id: u64,
    pub app_name: String,
    pub tooltip: String,
    pub icon_char: char,
    pub state: IconState,
    pub animation: AnimationType,
    pub badge: Option<IconBadge>,
    pub context_menu: Option<ContextMenu>,
    pub visible: bool,
    pub in_overflow: bool,
    pub order: u32,
    pub status_lines: Vec<String>,
    pub auto_hide: bool,
    pub auto_hide_condition: Option<String>,
}

impl TrayIconEntry {
    pub fn new(app_name: &str, tooltip: &str, icon: char) -> Self {
        Self {
            id: NEXT_ICON_ID.fetch_add(1, Ordering::Relaxed),
            app_name: String::from(app_name),
            tooltip: String::from(tooltip),
            icon_char: icon,
            state: IconState::Normal,
            animation: AnimationType::None,
            badge: None,
            context_menu: None,
            visible: true,
            in_overflow: false,
            order: 0,
            status_lines: Vec::new(),
            auto_hide: false,
            auto_hide_condition: None,
        }
    }

    pub fn set_tooltip(&mut self, tooltip: &str) {
        self.tooltip = String::from(tooltip);
    }

    pub fn set_icon(&mut self, icon: char) {
        self.icon_char = icon;
    }

    pub fn set_state(&mut self, state: IconState) {
        self.state = state;
    }

    pub fn set_animation(&mut self, animation: AnimationType) {
        self.animation = animation;
    }

    pub fn set_badge(&mut self, badge: Option<IconBadge>) {
        self.badge = badge;
    }

    pub fn set_badge_text(&mut self, text: &str, color: u32) {
        self.badge = Some(IconBadge::new(text, color));
    }

    pub fn clear_badge(&mut self) {
        self.badge = None;
    }

    pub fn add_status_line(&mut self, line: &str) {
        self.status_lines.push(String::from(line));
    }

    pub fn clear_status(&mut self) {
        self.status_lines.clear();
    }

    pub fn set_context_menu(&mut self, menu: ContextMenu) {
        self.context_menu = Some(menu);
    }
}

// ── System icon states ───────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct VolumeIconState {
    pub state: VolumeState,
    pub level: u8,
    pub output_device: String,
}

#[derive(Debug, Clone)]
pub struct NetworkIconState {
    pub state: NetworkState,
    pub ssid: Option<String>,
    pub signal_strength: u8,
    pub download_speed: u32,
    pub upload_speed: u32,
}

#[derive(Debug, Clone)]
pub struct BatteryIconState {
    pub state: BatteryState,
    pub percentage: u8,
    pub time_remaining_mins: Option<u32>,
    pub power_draw_watts: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct BluetoothIconState {
    pub state: BluetoothState,
    pub connected_devices: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct DisplayIconState {
    pub brightness: u8,
    pub night_light_active: bool,
    pub auto_rotate_enabled: bool,
}

#[derive(Debug, Clone)]
pub struct LocationIconState {
    pub in_use: bool,
    pub requesting_app: Option<String>,
}

#[derive(Debug, Clone)]
pub struct MicrophoneIconState {
    pub in_use: bool,
    pub requesting_app: Option<String>,
    pub muted: bool,
}

#[derive(Debug, Clone)]
pub struct CameraIconState {
    pub in_use: bool,
    pub requesting_app: Option<String>,
}

#[derive(Debug, Clone)]
pub struct PrinterIconState {
    pub jobs_pending: u32,
    pub printer_name: String,
}

#[derive(Debug, Clone)]
pub struct VpnIconState {
    pub state: VpnState,
    pub server_name: Option<String>,
    pub protocol: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ClockIconState {
    pub time_text: String,
    pub date_text: String,
    pub timezone: String,
    pub show_seconds: bool,
    pub use_24h: bool,
}

// ── Overflow area ────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct OverflowPanel {
    pub visible: bool,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl OverflowPanel {
    pub fn new() -> Self {
        Self {
            visible: false,
            x: 0,
            y: 0,
            width: 240,
            height: 200,
        }
    }

    pub fn toggle(&mut self) {
        self.visible = !self.visible;
    }

    pub fn show_at(&mut self, x: i32, y: i32) {
        self.visible = true;
        self.x = x;
        self.y = y;
    }

    pub fn hide(&mut self) {
        self.visible = false;
    }
}

// ── Icon ordering and drag ───────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct IconOrderConfig {
    pub order_map: Vec<(u64, u32)>,
    pub user_hidden: Vec<u64>,
}

impl IconOrderConfig {
    pub fn new() -> Self {
        Self {
            order_map: Vec::new(),
            user_hidden: Vec::new(),
        }
    }

    pub fn set_order(&mut self, icon_id: u64, position: u32) {
        if let Some(entry) = self.order_map.iter_mut().find(|(id, _)| *id == icon_id) {
            entry.1 = position;
        } else {
            self.order_map.push((icon_id, position));
        }
    }

    pub fn hide_icon(&mut self, icon_id: u64) {
        if !self.user_hidden.contains(&icon_id) {
            self.user_hidden.push(icon_id);
        }
    }

    pub fn show_icon(&mut self, icon_id: u64) {
        self.user_hidden.retain(|&id| id != icon_id);
    }

    pub fn is_hidden(&self, icon_id: u64) -> bool {
        self.user_hidden.contains(&icon_id)
    }

    pub fn get_order(&self, icon_id: u64) -> u32 {
        self.order_map
            .iter()
            .find(|(id, _)| *id == icon_id)
            .map(|(_, o)| *o)
            .unwrap_or(u32::MAX)
    }
}

// ── Main system tray ─────────────────────────────────────────────────────

pub struct SystemTrayDaemon {
    pub icons: Vec<TrayIconEntry>,
    pub overflow: OverflowPanel,
    pub order_config: IconOrderConfig,
    pub active_balloons: Vec<BalloonNotification>,
    pub theme: ThemeVariant,
    pub volume: VolumeIconState,
    pub network: NetworkIconState,
    pub battery: BatteryIconState,
    pub bluetooth: BluetoothIconState,
    pub display: DisplayIconState,
    pub location: LocationIconState,
    pub microphone: MicrophoneIconState,
    pub camera: CameraIconState,
    pub printer: PrinterIconState,
    pub vpn: VpnIconState,
    pub clock: ClockIconState,
    pub tooltip_delay_ms: u64,
    pub show_clock: bool,
    pub show_date: bool,
}

impl SystemTrayDaemon {
    pub fn new() -> Self {
        Self {
            icons: Vec::new(),
            overflow: OverflowPanel::new(),
            order_config: IconOrderConfig::new(),
            active_balloons: Vec::new(),
            theme: ThemeVariant::Dark,
            volume: VolumeIconState {
                state: VolumeState::Medium,
                level: 75,
                output_device: String::from("Speakers"),
            },
            network: NetworkIconState {
                state: NetworkState::WifiExcellent,
                ssid: Some(String::from("AthenaNet")),
                signal_strength: 95,
                download_speed: 0,
                upload_speed: 0,
            },
            battery: BatteryIconState {
                state: BatteryState::Discharging,
                percentage: 85,
                time_remaining_mins: Some(240),
                power_draw_watts: Some(15),
            },
            bluetooth: BluetoothIconState {
                state: BluetoothState::On,
                connected_devices: Vec::new(),
            },
            display: DisplayIconState {
                brightness: 80,
                night_light_active: false,
                auto_rotate_enabled: false,
            },
            location: LocationIconState {
                in_use: false,
                requesting_app: None,
            },
            microphone: MicrophoneIconState {
                in_use: false,
                requesting_app: None,
                muted: false,
            },
            camera: CameraIconState {
                in_use: false,
                requesting_app: None,
            },
            printer: PrinterIconState {
                jobs_pending: 0,
                printer_name: String::new(),
            },
            vpn: VpnIconState {
                state: VpnState::Disconnected,
                server_name: None,
                protocol: None,
            },
            clock: ClockIconState {
                time_text: String::from("00:00"),
                date_text: String::from("Mon Jan 1"),
                timezone: String::from("UTC"),
                show_seconds: false,
                use_24h: true,
            },
            tooltip_delay_ms: TOOLTIP_DELAY_MS,
            show_clock: true,
            show_date: true,
        }
    }

    pub fn register_icon(&mut self, app_name: &str, tooltip: &str, icon: char) -> u64 {
        let entry = TrayIconEntry::new(app_name, tooltip, icon);
        let id = entry.id;
        let order = self.icons.len() as u32;
        let mut e = entry;
        e.order = order;
        if self.visible_count() >= MAX_VISIBLE_ICONS {
            e.in_overflow = true;
        }
        self.icons.push(e);
        id
    }

    pub fn unregister_icon(&mut self, id: u64) {
        self.icons.retain(|i| i.id != id);
        self.recompute_overflow();
    }

    pub fn update_icon(&mut self, id: u64, icon: char) {
        if let Some(entry) = self.icons.iter_mut().find(|i| i.id == id) {
            entry.set_icon(icon);
        }
    }

    pub fn update_tooltip(&mut self, id: u64, tooltip: &str) {
        if let Some(entry) = self.icons.iter_mut().find(|i| i.id == id) {
            entry.set_tooltip(tooltip);
        }
    }

    pub fn set_badge(&mut self, id: u64, text: &str, color: u32) {
        if let Some(entry) = self.icons.iter_mut().find(|i| i.id == id) {
            entry.set_badge_text(text, color);
        }
    }

    pub fn clear_badge(&mut self, id: u64) {
        if let Some(entry) = self.icons.iter_mut().find(|i| i.id == id) {
            entry.clear_badge();
        }
    }

    pub fn set_animation(&mut self, id: u64, anim: AnimationType) {
        if let Some(entry) = self.icons.iter_mut().find(|i| i.id == id) {
            entry.set_animation(anim);
        }
    }

    pub fn handle_click(&mut self, id: u64, click: ClickType) -> Option<u64> {
        match click {
            ClickType::RightClick => {
                if let Some(entry) = self.icons.iter_mut().find(|i| i.id == id) {
                    if let Some(ref mut menu) = entry.context_menu {
                        menu.show_at(0, 0);
                    }
                }
                None
            }
            ClickType::LeftClick => Some(id),
            ClickType::DoubleClick => Some(id),
            ClickType::MiddleClick => Some(id),
        }
    }

    pub fn show_balloon(
        &mut self,
        title: &str,
        message: &str,
        icon: BalloonIcon,
        timestamp: u64,
    ) -> u64 {
        let mut balloon = BalloonNotification::new(title, message).with_icon(icon);
        balloon.created_at = timestamp;
        let id = balloon.id;
        self.active_balloons.push(balloon);
        id
    }

    pub fn dismiss_balloon(&mut self, id: u64) {
        if let Some(b) = self.active_balloons.iter_mut().find(|b| b.id == id) {
            b.dismiss();
        }
    }

    pub fn cleanup_expired_balloons(&mut self, current_time: u64) {
        self.active_balloons
            .retain(|b| !b.dismissed && !b.is_expired(current_time));
    }

    pub fn set_theme(&mut self, theme: ThemeVariant) {
        self.theme = theme;
    }

    pub fn move_to_overflow(&mut self, id: u64) {
        if let Some(entry) = self.icons.iter_mut().find(|i| i.id == id) {
            entry.in_overflow = true;
        }
    }

    pub fn move_from_overflow(&mut self, id: u64) {
        if let Some(entry) = self.icons.iter_mut().find(|i| i.id == id) {
            entry.in_overflow = false;
        }
    }

    pub fn reorder_icon(&mut self, id: u64, new_position: u32) {
        self.order_config.set_order(id, new_position);
        if let Some(entry) = self.icons.iter_mut().find(|i| i.id == id) {
            entry.order = new_position;
        }
        self.icons.sort_by_key(|i| i.order);
    }

    pub fn visible_icons(&self) -> Vec<&TrayIconEntry> {
        self.icons
            .iter()
            .filter(|i| i.visible && !i.in_overflow && i.state != IconState::Hidden)
            .collect()
    }

    pub fn overflow_icons(&self) -> Vec<&TrayIconEntry> {
        self.icons
            .iter()
            .filter(|i| i.visible && i.in_overflow && i.state != IconState::Hidden)
            .collect()
    }

    fn visible_count(&self) -> usize {
        self.icons
            .iter()
            .filter(|i| i.visible && !i.in_overflow && i.state != IconState::Hidden)
            .count()
    }

    fn recompute_overflow(&mut self) {
        let mut visible_count = 0;
        for icon in &mut self.icons {
            if !icon.visible || icon.state == IconState::Hidden {
                continue;
            }
            if visible_count < MAX_VISIBLE_ICONS {
                icon.in_overflow = false;
                visible_count += 1;
            } else {
                icon.in_overflow = true;
            }
        }
    }

    pub fn update_volume(&mut self, level: u8, muted: bool) {
        self.volume.level = level;
        self.volume.state = if muted {
            VolumeState::Muted
        } else if level < 33 {
            VolumeState::Low
        } else if level < 66 {
            VolumeState::Medium
        } else {
            VolumeState::High
        };
    }

    pub fn update_network(&mut self, state: NetworkState, ssid: Option<&str>, strength: u8) {
        self.network.state = state;
        self.network.ssid = ssid.map(String::from);
        self.network.signal_strength = strength;
    }

    pub fn update_battery(&mut self, percentage: u8, state: BatteryState, time_mins: Option<u32>) {
        self.battery.percentage = percentage;
        self.battery.state = state;
        self.battery.time_remaining_mins = time_mins;
    }

    pub fn update_bluetooth(&mut self, state: BluetoothState) {
        self.bluetooth.state = state;
    }

    pub fn add_bluetooth_device(&mut self, name: &str) {
        self.bluetooth.connected_devices.push(String::from(name));
        self.bluetooth.state = BluetoothState::Connected;
    }

    pub fn remove_bluetooth_device(&mut self, name: &str) {
        self.bluetooth.connected_devices.retain(|d| d != name);
        if self.bluetooth.connected_devices.is_empty() {
            self.bluetooth.state = BluetoothState::On;
        }
    }

    pub fn update_display(&mut self, brightness: u8, night_light: bool) {
        self.display.brightness = brightness;
        self.display.night_light_active = night_light;
    }

    pub fn set_location_in_use(&mut self, in_use: bool, app: Option<&str>) {
        self.location.in_use = in_use;
        self.location.requesting_app = app.map(String::from);
    }

    pub fn set_microphone_in_use(&mut self, in_use: bool, app: Option<&str>) {
        self.microphone.in_use = in_use;
        self.microphone.requesting_app = app.map(String::from);
    }

    pub fn set_camera_in_use(&mut self, in_use: bool, app: Option<&str>) {
        self.camera.in_use = in_use;
        self.camera.requesting_app = app.map(String::from);
    }

    pub fn update_printer(&mut self, jobs: u32, name: &str) {
        self.printer.jobs_pending = jobs;
        self.printer.printer_name = String::from(name);
    }

    pub fn update_vpn(&mut self, state: VpnState, server: Option<&str>, protocol: Option<&str>) {
        self.vpn.state = state;
        self.vpn.server_name = server.map(String::from);
        self.vpn.protocol = protocol.map(String::from);
    }

    pub fn update_clock(&mut self, time: &str, date: &str) {
        self.clock.time_text = String::from(time);
        self.clock.date_text = String::from(date);
    }

    pub fn set_clock_format(&mut self, use_24h: bool, show_seconds: bool) {
        self.clock.use_24h = use_24h;
        self.clock.show_seconds = show_seconds;
    }

    pub fn system_icon_char(&self, icon_type: SystemIconType) -> char {
        match self.theme {
            ThemeVariant::Dark | ThemeVariant::Light => match icon_type {
                SystemIconType::Volume => match self.volume.state {
                    VolumeState::Muted => '\u{1F507}',
                    VolumeState::Low => '\u{1F508}',
                    VolumeState::Medium => '\u{1F509}',
                    VolumeState::High => '\u{1F50A}',
                },
                SystemIconType::Network => match self.network.state {
                    NetworkState::Disconnected => '\u{274C}',
                    NetworkState::Ethernet => '\u{1F4F6}',
                    NetworkState::Airplane => '\u{2708}',
                    _ => '\u{1F4F6}',
                },
                SystemIconType::Battery => match self.battery.state {
                    BatteryState::Charging => '\u{1F50C}',
                    BatteryState::Critical => '\u{1FAB6}',
                    _ => '\u{1F50B}',
                },
                SystemIconType::Bluetooth => match self.bluetooth.state {
                    BluetoothState::Off => '\u{2205}',
                    _ => '\u{1F4F6}',
                },
                SystemIconType::Display => '\u{1F4BB}',
                SystemIconType::Location => {
                    if self.location.in_use {
                        '\u{1F4CD}'
                    } else {
                        '\u{1F4CD}'
                    }
                }
                SystemIconType::Microphone => {
                    if self.microphone.in_use {
                        '\u{1F3A4}'
                    } else {
                        '\u{1F3A4}'
                    }
                }
                SystemIconType::Camera => '\u{1F4F7}',
                SystemIconType::UsbSafeRemove => '\u{1F50C}',
                SystemIconType::Printer => '\u{1F5A8}',
                SystemIconType::UpdatesAvailable => '\u{1F504}',
                SystemIconType::Vpn => match self.vpn.state {
                    VpnState::Connected => '\u{1F512}',
                    _ => '\u{1F513}',
                },
                SystemIconType::Clock => '\u{1F552}',
            },
            ThemeVariant::HighContrast => match icon_type {
                SystemIconType::Volume => 'V',
                SystemIconType::Network => 'N',
                SystemIconType::Battery => 'B',
                SystemIconType::Bluetooth => 'T',
                SystemIconType::Display => 'D',
                SystemIconType::Location => 'L',
                SystemIconType::Microphone => 'M',
                SystemIconType::Camera => 'C',
                SystemIconType::UsbSafeRemove => 'U',
                SystemIconType::Printer => 'P',
                SystemIconType::UpdatesAvailable => '!',
                SystemIconType::Vpn => 'S',
                SystemIconType::Clock => '#',
            },
        }
    }

    pub fn get_tooltip_text(&self, icon_type: SystemIconType) -> String {
        match icon_type {
            SystemIconType::Volume => {
                let mut s = String::from("Volume: ");
                let mut buf = [0u8; 4];
                let val = format_u8(self.volume.level, &mut buf);
                s.push_str(val);
                s.push('%');
                if self.volume.state == VolumeState::Muted {
                    s.push_str(" (Muted)");
                }
                s
            }
            SystemIconType::Network => {
                let mut s = match self.network.state {
                    NetworkState::Disconnected => String::from("No internet"),
                    NetworkState::Ethernet => String::from("Ethernet connected"),
                    NetworkState::Airplane => String::from("Airplane mode"),
                    _ => {
                        let mut w = String::from("Wi-Fi: ");
                        if let Some(ref ssid) = self.network.ssid {
                            w.push_str(ssid);
                        }
                        w
                    }
                };
                s
            }
            SystemIconType::Battery => {
                let mut s = String::from("Battery: ");
                let mut buf = [0u8; 4];
                let val = format_u8(self.battery.percentage, &mut buf);
                s.push_str(val);
                s.push('%');
                match self.battery.state {
                    BatteryState::Charging => s.push_str(" (Charging)"),
                    BatteryState::BatterySaver => s.push_str(" (Saver)"),
                    _ => {}
                }
                s
            }
            SystemIconType::Clock => {
                let mut s = self.clock.time_text.clone();
                s.push_str("\n");
                s.push_str(&self.clock.date_text);
                s
            }
            _ => String::from(""),
        }
    }
}

fn format_u8(mut n: u8, buf: &mut [u8; 4]) -> &str {
    if n == 0 {
        buf[0] = b'0';
        return unsafe { core::str::from_utf8_unchecked(&buf[..1]) };
    }
    let mut pos = 4;
    while n > 0 && pos > 0 {
        pos -= 1;
        buf[pos] = b'0' + (n % 10);
        n /= 10;
    }
    unsafe { core::str::from_utf8_unchecked(&buf[pos..4]) }
}

// ── Colour palette ───────────────────────────────────────────────────────

const ST_BG: u32 = 0xFF_0A_0E_1A;
const ST_FG: u32 = 0xFF_FF_FF_FF;
const ST_DIM: u32 = 0xFF_70_70_80;
const ST_ACCENT: u32 = 0xFF_4E_9C_FF;
const ST_POPUP_BG: u32 = 0xFF_14_16_22;
const ST_GREEN: u32 = 0xFF_44_DD_66;
const ST_YELLOW: u32 = 0xFF_FF_CC_33;
const ST_RED: u32 = 0xFF_FF_44_44;
const ST_SLIDER: u32 = 0xFF_33_33_44;
const GLYPH_W: usize = 8;
const GLYPH_H: usize = 8;
const ICON_SLOT: usize = 24;

// ── Rendering ────────────────────────────────────────────────────────────

impl SystemTrayDaemon {
    pub fn render(&self, canvas: &mut athgfx::Canvas, ox: usize, oy: usize, bar_h: usize) {
        let text_y = oy + (bar_h.saturating_sub(GLYPH_H)) / 2;
        let mut cx = ox;

        let vol_icon = match self.volume.state {
            VolumeState::Muted => 'M',
            VolumeState::Low => 'v',
            VolumeState::Medium | VolumeState::High => 'V',
        };
        let vol_color = if self.volume.state == VolumeState::Muted {
            ST_DIM
        } else {
            ST_ACCENT
        };
        canvas.draw_glyph(cx + 2, text_y, vol_icon, vol_color, None);
        cx += ICON_SLOT;

        let net_icon = match self.network.state {
            NetworkState::Disconnected => '!',
            NetworkState::Ethernet => 'E',
            _ => 'W',
        };
        let net_color = match self.network.state {
            NetworkState::Disconnected => ST_RED,
            NetworkState::WifiWeak => ST_YELLOW,
            _ => ST_GREEN,
        };
        canvas.draw_glyph(cx + 2, text_y, net_icon, net_color, None);
        cx += ICON_SLOT;

        let bat_color = if self.battery.percentage <= 20 {
            ST_RED
        } else if self.battery.percentage <= 50 {
            ST_YELLOW
        } else {
            ST_GREEN
        };
        canvas.draw_glyph(cx + 2, text_y, 'B', bat_color, None);
        let mut pct_buf = [0u8; 4];
        let pct_str = fmt_u8_pct(self.battery.percentage, &mut pct_buf);
        canvas.draw_text(cx + 12, text_y, pct_str, ST_DIM, None);
        cx += ICON_SLOT + pct_str.len() * GLYPH_W;

        for entry in self
            .icons
            .iter()
            .filter(|e| !e.in_overflow && e.state != IconState::Hidden)
        {
            let ic = if entry.state == IconState::Disabled {
                ST_DIM
            } else if entry.state == IconState::Attention {
                ST_YELLOW
            } else {
                ST_ACCENT
            };
            canvas.draw_glyph(cx + 2, text_y, entry.icon_char, ic, None);
            cx += ICON_SLOT;
        }

        cx += 4;
        for sy in oy + 4..oy + bar_h - 4 {
            canvas.draw_pixel(cx, sy, ST_DIM);
        }
        cx += 8;

        if self.show_clock {
            canvas.draw_text(cx, text_y, &self.clock.time_text, ST_FG, None);
        }
    }

    pub fn total_width(&self) -> usize {
        let system_icons = 3;
        let app_icons = self
            .icons
            .iter()
            .filter(|e| !e.in_overflow && e.state != IconState::Hidden)
            .count();
        let clock_w = if self.show_clock {
            self.clock.time_text.len() * GLYPH_W + 12
        } else {
            0
        };
        (system_icons + app_icons) * ICON_SLOT + clock_w + 20
    }

    pub fn render_popup(
        &self,
        canvas: &mut athgfx::Canvas,
        icon_type: SystemIconType,
        px: usize,
        py: usize,
    ) {
        let pw = 200usize;
        let ph = 120usize;
        canvas.fill_rect(px, py, pw, ph, ST_POPUP_BG);
        canvas.draw_rect_outline(px, py, pw, ph, ST_ACCENT);

        match icon_type {
            SystemIconType::Volume => {
                canvas.draw_text(px + 8, py + 8, "Volume", ST_FG, None);
                canvas.draw_text(px + 8, py + 24, &self.volume.output_device, ST_DIM, None);
                canvas.fill_rect(px + 8, py + 44, pw - 16, 4, ST_SLIDER);
                let fill = ((pw - 16) as u32 * self.volume.level as u32 / 100) as usize;
                canvas.fill_rect(px + 8, py + 44, fill, 4, ST_ACCENT);
                canvas.fill_rect(px + 8 + fill.saturating_sub(4), py + 40, 8, 12, ST_FG);
                let mut vbuf = [0u8; 4];
                let vs = fmt_u8_pct(self.volume.level, &mut vbuf);
                canvas.draw_text(px + pw - 40, py + 38, vs, ST_DIM, None);
            }
            SystemIconType::Network => {
                canvas.draw_text(px + 8, py + 8, "Network", ST_FG, None);
                let status = match self.network.state {
                    NetworkState::Disconnected => "Disconnected",
                    NetworkState::Ethernet => "Ethernet",
                    _ => "Wi-Fi",
                };
                canvas.draw_text(px + 8, py + 24, status, ST_DIM, None);
                if let Some(ref ssid) = self.network.ssid {
                    canvas.draw_text(px + 8, py + 40, ssid, ST_ACCENT, None);
                }
            }
            SystemIconType::Battery => {
                canvas.draw_text(px + 8, py + 8, "Battery", ST_FG, None);
                let state = match self.battery.state {
                    BatteryState::Charging => "Charging",
                    BatteryState::Discharging => "On battery",
                    BatteryState::Full => "Fully charged",
                    BatteryState::BatterySaver | BatteryState::Critical => "Low power",
                };
                canvas.draw_text(px + 8, py + 24, state, ST_DIM, None);
                canvas.fill_rect(px + 8, py + 44, pw - 16, 12, ST_SLIDER);
                let fill = ((pw - 16) as u32 * self.battery.percentage as u32 / 100) as usize;
                let fill_color = if self.battery.percentage <= 20 {
                    ST_RED
                } else {
                    ST_GREEN
                };
                canvas.fill_rect(px + 8, py + 44, fill, 12, fill_color);
                let mut pbuf = [0u8; 4];
                let ps = fmt_u8_pct(self.battery.percentage, &mut pbuf);
                canvas.draw_text(px + pw - 40, py + 44, ps, ST_FG, None);
                if let Some(mins) = self.battery.time_remaining_mins {
                    let hrs = mins / 60;
                    let m = mins % 60;
                    let mut tbuf = [0u8; 8];
                    let ts = fmt_time_remaining(hrs, m, &mut tbuf);
                    canvas.draw_text(px + 8, py + 64, ts, ST_DIM, None);
                }
            }
            _ => {
                canvas.draw_text(px + 8, py + 8, "Info", ST_FG, None);
            }
        }
    }
}

fn fmt_u8_pct(n: u8, buf: &mut [u8; 4]) -> &str {
    let mut val = n;
    let mut pos = 3;
    buf[pos] = b'%';
    if val == 0 {
        pos -= 1;
        buf[pos] = b'0';
    } else {
        while val > 0 && pos > 0 {
            pos -= 1;
            buf[pos] = b'0' + val % 10;
            val /= 10;
        }
    }
    unsafe { core::str::from_utf8_unchecked(&buf[pos..4]) }
}

fn fmt_time_remaining(hrs: u32, mins: u32, buf: &mut [u8; 8]) -> &str {
    let mut pos = 0;
    if hrs >= 10 {
        buf[pos] = b'0' + (hrs / 10) as u8;
        pos += 1;
    }
    buf[pos] = b'0' + (hrs % 10) as u8;
    pos += 1;
    buf[pos] = b'h';
    pos += 1;
    buf[pos] = b' ';
    pos += 1;
    if mins >= 10 {
        buf[pos] = b'0' + (mins / 10) as u8;
        pos += 1;
    }
    buf[pos] = b'0' + (mins % 10) as u8;
    pos += 1;
    buf[pos] = b'm';
    pos += 1;
    unsafe { core::str::from_utf8_unchecked(&buf[..pos]) }
}

// ── Global system tray singleton ─────────────────────────────────────────

static mut SYSTEM_TRAY: Option<SystemTrayDaemon> = None;

pub fn init() {
    unsafe {
        if !TRAY_INITIALIZED.swap(true, Ordering::SeqCst) {
            SYSTEM_TRAY = Some(SystemTrayDaemon::new());
        }
    }
}

pub fn tray() -> &'static SystemTrayDaemon {
    unsafe {
        SYSTEM_TRAY
            .as_ref()
            .expect("system_tray_daemon not initialized")
    }
}

pub fn tray_mut() -> &'static mut SystemTrayDaemon {
    unsafe {
        SYSTEM_TRAY
            .as_mut()
            .expect("system_tray_daemon not initialized")
    }
}
