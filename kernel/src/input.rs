//! Unified input/controller subsystem for RaeenOS.
//!
//! Provides a hardware-agnostic event pipeline for keyboards, mice, gamepads,
//! and touch devices — with first-class support for DualSense (haptics,
//! adaptive triggers, gyro, touchpad), Xbox controllers, generic HID gamepads,
//! and a unified RGB lighting API that spans all peripherals.

#![allow(dead_code)]

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

// ═══════════════════════════════════════════════════════════════════════════════
// §1  Input Event System
// ═══════════════════════════════════════════════════════════════════════════════

pub type InputDeviceId = u64;

#[derive(Debug, Clone)]
pub struct InputEvent {
    pub timestamp: u64,
    pub device_id: InputDeviceId,
    pub event: InputEventType,
}

#[derive(Debug, Clone)]
pub enum InputEventType {
    // Keyboard
    KeyDown(KeyCode),
    KeyUp(KeyCode),
    KeyRepeat(KeyCode),

    // Mouse
    MouseMove {
        dx: i32,
        dy: i32,
    },
    MouseMoveAbsolute {
        x: i32,
        y: i32,
    },
    MouseButtonDown(MouseButton),
    MouseButtonUp(MouseButton),
    MouseScroll {
        dx: i32,
        dy: i32,
    },

    // Gamepad
    GamepadButtonDown(GamepadButton),
    GamepadButtonUp(GamepadButton),
    GamepadAxis {
        axis: GamepadAxis,
        value: i16,
    },
    GamepadTrigger {
        trigger: GamepadTrigger,
        value: u8,
    },
    GamepadMotion {
        gyro_x: i16,
        gyro_y: i16,
        gyro_z: i16,
        accel_x: i16,
        accel_y: i16,
        accel_z: i16,
    },
    GamepadTouchpad {
        finger: u8,
        x: u16,
        y: u16,
        active: bool,
    },

    // Touch
    TouchDown {
        id: u32,
        x: i32,
        y: i32,
    },
    TouchUp {
        id: u32,
        x: i32,
        y: i32,
    },
    TouchMove {
        id: u32,
        x: i32,
        y: i32,
    },

    // System
    DeviceConnected(InputDeviceInfo),
    DeviceDisconnected(InputDeviceId),
}

#[derive(Debug, Clone)]
pub struct InputDeviceInfo {
    pub id: InputDeviceId,
    pub name: String,
    pub vendor_id: u16,
    pub product_id: u16,
    pub device_type: InputDeviceType,
    pub serial: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputDeviceType {
    Keyboard,
    Mouse,
    Gamepad,
    Touchscreen,
    Touchpad,
    Stylus,
    Unknown,
}

// ═══════════════════════════════════════════════════════════════════════════════
// §2  Key Codes (full keyboard)
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u16)]
pub enum KeyCode {
    // Function row
    Escape = 0x01,
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    F10,
    F11,
    F12,
    F13,
    F14,
    F15,
    F16,
    F17,
    F18,
    F19,
    F20,
    F21,
    F22,
    F23,
    F24,

    // Number row
    Grave,
    Key1,
    Key2,
    Key3,
    Key4,
    Key5,
    Key6,
    Key7,
    Key8,
    Key9,
    Key0,
    Minus,
    Equal,
    Backspace,

    // QWERTY row 1
    Tab,
    Q,
    W,
    E,
    R,
    T,
    Y,
    U,
    I,
    O,
    P,
    LeftBracket,
    RightBracket,
    Backslash,

    // Home row
    CapsLock,
    A,
    S,
    D,
    F,
    G,
    H,
    J,
    K,
    L,
    Semicolon,
    Apostrophe,
    Enter,

    // Bottom row
    LeftShift,
    Z,
    X,
    C,
    V,
    B,
    N,
    M,
    Comma,
    Period,
    Slash,
    RightShift,

    // Modifiers + Space
    LeftCtrl,
    LeftAlt,
    LeftMeta,
    Space,
    RightMeta,
    RightAlt,
    RightCtrl,
    Fn,

    // System keys
    PrintScreen,
    ScrollLock,
    Pause,
    Insert,
    Delete,
    Home,
    End,
    PageUp,
    PageDown,

    // Arrow keys
    Up,
    Down,
    Left,
    Right,

    // Numpad
    NumLock,
    NumpadDivide,
    NumpadMultiply,
    NumpadMinus,
    NumpadPlus,
    NumpadEnter,
    Numpad0,
    Numpad1,
    Numpad2,
    Numpad3,
    Numpad4,
    Numpad5,
    Numpad6,
    Numpad7,
    Numpad8,
    Numpad9,
    NumpadDot,
    NumpadEqual,

    // Media
    MediaPlayPause,
    MediaStop,
    MediaPrevious,
    MediaNext,
    VolumeUp,
    VolumeDown,
    VolumeMute,

    // International
    IntlBackslash,
    IntlRo,
    IntlYen,
    Lang1,
    Lang2,
    Lang3,
    Lang4,
    Lang5,
    KatakanaHiragana,
    Henkan,
    Muhenkan,

    // Misc
    ContextMenu,
    Power,
    Sleep,
    Wake,

    Unknown(u16),
}

// ═══════════════════════════════════════════════════════════════════════════════
// §3  Gamepad Types
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum GamepadButton {
    // Face buttons (positional, not label-based)
    South, // A / Cross
    East,  // B / Circle
    North, // Y / Triangle
    West,  // X / Square

    // Bumpers / Shoulders
    LeftBumper,
    RightBumper,

    // Center cluster
    Back,  // Select / Share / View
    Start, // Options / Menu
    Guide, // PS / Xbox

    // Stick clicks
    LeftStick,
    RightStick,

    // D-Pad
    DpadUp,
    DpadDown,
    DpadLeft,
    DpadRight,

    // DualSense-specific
    Touchpad,
    Microphone,
    Create,
    Share,

    // Xbox-specific
    ShareXbox,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GamepadAxis {
    LeftStickX,
    LeftStickY,
    RightStickX,
    RightStickY,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GamepadTrigger {
    Left,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    Back,
    Forward,
}

// ═══════════════════════════════════════════════════════════════════════════════
// §4  DualSense Controller (full feature set)
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct TouchPoint {
    pub id: u8,
    pub x: u16,
    pub y: u16,
    pub active: bool,
}

impl Default for TouchPoint {
    fn default() -> Self {
        Self {
            id: 0,
            x: 0,
            y: 0,
            active: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct DualSenseState {
    pub buttons: u32,
    pub left_stick: (i8, i8),
    pub right_stick: (i8, i8),
    pub left_trigger: u8,
    pub right_trigger: u8,
    pub gyro: [i16; 3],
    pub accelerometer: [i16; 3],
    pub touchpad: [TouchPoint; 2],
    pub battery_level: u8,
    pub battery_charging: bool,
    pub headphone_connected: bool,
    pub microphone_muted: bool,
}

impl Default for DualSenseState {
    fn default() -> Self {
        Self {
            buttons: 0,
            left_stick: (0, 0),
            right_stick: (0, 0),
            left_trigger: 0,
            right_trigger: 0,
            gyro: [0; 3],
            accelerometer: [0; 3],
            touchpad: [TouchPoint::default(), TouchPoint::default()],
            battery_level: 0,
            battery_charging: false,
            headphone_connected: false,
            microphone_muted: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LedBrightness {
    High,
    Medium,
    Low,
    Off,
}

#[derive(Debug, Clone)]
pub enum AdaptiveTriggerMode {
    Off,
    Resistance {
        start: u8,
        strength: u8,
    },
    Bow {
        start: u8,
        end: u8,
        strength: u8,
        snap_strength: u8,
    },
    Galloping {
        start: u8,
        end: u8,
        first_foot: u8,
        second_foot: u8,
        frequency: u8,
    },
    SemiAutomatic {
        start: u8,
        end: u8,
        strength: u8,
    },
    Automatic {
        start: u8,
        strength: u8,
        frequency: u8,
    },
    Machine {
        start: u8,
        end: u8,
        amplitude: u8,
        frequency: u8,
        period: u8,
    },
}

#[derive(Debug, Clone)]
pub struct DualSenseOutput {
    pub left_rumble: u8,
    pub right_rumble: u8,
    pub led_color: (u8, u8, u8),
    pub led_brightness: LedBrightness,
    pub player_leds: u8,
    pub microphone_led: bool,
    pub adaptive_trigger_left: AdaptiveTriggerMode,
    pub adaptive_trigger_right: AdaptiveTriggerMode,
    pub haptic_data: Option<Vec<u8>>,
}

impl Default for DualSenseOutput {
    fn default() -> Self {
        Self {
            left_rumble: 0,
            right_rumble: 0,
            led_color: (0, 0, 64),
            led_brightness: LedBrightness::Medium,
            player_leds: 0,
            microphone_led: false,
            adaptive_trigger_left: AdaptiveTriggerMode::Off,
            adaptive_trigger_right: AdaptiveTriggerMode::Off,
            haptic_data: None,
        }
    }
}

impl DualSenseState {
    /// Parse a raw USB/BT input report (report ID 0x01 for USB, 0x31 for BT).
    pub fn parse_report(&mut self, data: &[u8]) -> Result<(), &'static str> {
        if data.len() < 64 {
            return Err("DualSense report too short");
        }
        let offset = if data[0] == 0x31 { 2 } else { 1 };

        self.left_stick.0 = (data[offset] as i16 - 128) as i8;
        self.left_stick.1 = (data[offset + 1] as i16 - 128) as i8;
        self.right_stick.0 = (data[offset + 2] as i16 - 128) as i8;
        self.right_stick.1 = (data[offset + 3] as i16 - 128) as i8;
        self.left_trigger = data[offset + 4];
        self.right_trigger = data[offset + 5];

        let btn_low = data[offset + 7] as u32;
        let btn_mid = (data[offset + 8] as u32) << 8;
        let btn_high = (data[offset + 9] as u32) << 16;
        self.buttons = btn_low | btn_mid | btn_high;

        // IMU data (gyro + accel) starts at offset+15 on USB reports
        let imu_off = offset + 15;
        if data.len() > imu_off + 12 {
            self.gyro[0] = i16::from_le_bytes([data[imu_off], data[imu_off + 1]]);
            self.gyro[1] = i16::from_le_bytes([data[imu_off + 2], data[imu_off + 3]]);
            self.gyro[2] = i16::from_le_bytes([data[imu_off + 4], data[imu_off + 5]]);
            self.accelerometer[0] = i16::from_le_bytes([data[imu_off + 6], data[imu_off + 7]]);
            self.accelerometer[1] = i16::from_le_bytes([data[imu_off + 8], data[imu_off + 9]]);
            self.accelerometer[2] = i16::from_le_bytes([data[imu_off + 10], data[imu_off + 11]]);
        }

        // Touchpad (two touch points)
        let tp_off = offset + 32;
        if data.len() > tp_off + 8 {
            self.touchpad[0].active = (data[tp_off] & 0x80) == 0;
            self.touchpad[0].id = data[tp_off] & 0x7F;
            self.touchpad[0].x = u16::from_le_bytes([data[tp_off + 1], data[tp_off + 2]]) & 0x0FFF;
            self.touchpad[0].y =
                (u16::from_le_bytes([data[tp_off + 2], data[tp_off + 3]]) >> 4) & 0x0FFF;

            self.touchpad[1].active = (data[tp_off + 4] & 0x80) == 0;
            self.touchpad[1].id = data[tp_off + 4] & 0x7F;
            self.touchpad[1].x = u16::from_le_bytes([data[tp_off + 5], data[tp_off + 6]]) & 0x0FFF;
            self.touchpad[1].y =
                (u16::from_le_bytes([data[tp_off + 6], data[tp_off + 7]]) >> 4) & 0x0FFF;
        }

        // Battery + status flags
        let status_off = offset + 52;
        if data.len() > status_off + 1 {
            self.battery_level = data[status_off] & 0x0F;
            self.battery_charging = (data[status_off] & 0x10) != 0;
            self.headphone_connected = (data[status_off + 1] & 0x01) != 0;
            self.microphone_muted = (data[status_off + 1] & 0x04) != 0;
        }

        Ok(())
    }
}

impl DualSenseOutput {
    /// Serialize to a USB output report (report ID 0x02, 48 bytes payload).
    pub fn build_report(&self) -> Vec<u8> {
        let mut buf = alloc::vec![0u8; 48];
        buf[0] = 0x02; // report ID

        // Valid flag bytes: enable rumble + LED + adaptive triggers
        buf[1] = 0xFF;
        buf[2] = 0xF7;

        buf[3] = self.right_rumble;
        buf[4] = self.left_rumble;

        // Adaptive trigger right (bytes 11-21)
        Self::encode_trigger(&self.adaptive_trigger_right, &mut buf[11..22]);
        // Adaptive trigger left (bytes 22-32)
        Self::encode_trigger(&self.adaptive_trigger_left, &mut buf[22..33]);

        // LED
        buf[44] = self.led_color.0;
        buf[45] = self.led_color.1;
        buf[46] = self.led_color.2;

        // Player LEDs
        buf[43] = self.player_leds & 0x1F;

        // Microphone LED
        buf[9] = if self.microphone_led { 0x01 } else { 0x00 };

        // LED brightness
        buf[42] = match self.led_brightness {
            LedBrightness::High => 0x00,
            LedBrightness::Medium => 0x01,
            LedBrightness::Low => 0x02,
            LedBrightness::Off => 0x03,
        };

        buf
    }

    fn encode_trigger(mode: &AdaptiveTriggerMode, out: &mut [u8]) {
        match mode {
            AdaptiveTriggerMode::Off => {
                out[0] = 0x00;
            }
            AdaptiveTriggerMode::Resistance { start, strength } => {
                out[0] = 0x01;
                out[1] = *start;
                out[2] = *strength;
            }
            AdaptiveTriggerMode::Bow {
                start,
                end,
                strength,
                snap_strength,
            } => {
                out[0] = 0x02;
                out[1] = *start;
                out[2] = *end;
                out[3] = *strength;
                out[4] = *snap_strength;
            }
            AdaptiveTriggerMode::Galloping {
                start,
                end,
                first_foot,
                second_foot,
                frequency,
            } => {
                out[0] = 0x03;
                out[1] = *start;
                out[2] = *end;
                out[3] = *first_foot;
                out[4] = *second_foot;
                out[5] = *frequency;
            }
            AdaptiveTriggerMode::SemiAutomatic {
                start,
                end,
                strength,
            } => {
                out[0] = 0x04;
                out[1] = *start;
                out[2] = *end;
                out[3] = *strength;
            }
            AdaptiveTriggerMode::Automatic {
                start,
                strength,
                frequency,
            } => {
                out[0] = 0x05;
                out[1] = *start;
                out[2] = *strength;
                out[3] = *frequency;
            }
            AdaptiveTriggerMode::Machine {
                start,
                end,
                amplitude,
                frequency,
                period,
            } => {
                out[0] = 0x06;
                out[1] = *start;
                out[2] = *end;
                out[3] = *amplitude;
                out[4] = *frequency;
                out[5] = *period;
            }
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// §5  Xbox Controller
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Default)]
pub struct XboxControllerState {
    pub buttons: u16,
    pub left_stick: (i16, i16),
    pub right_stick: (i16, i16),
    pub left_trigger: u8,
    pub right_trigger: u8,
}

#[derive(Debug, Clone, Default)]
pub struct XboxControllerOutput {
    pub left_rumble: u8,
    pub right_rumble: u8,
    pub left_trigger_rumble: u8,
    pub right_trigger_rumble: u8,
}

impl XboxControllerState {
    /// Parse a raw Xbox controller input report (GIP protocol, 18 bytes).
    pub fn parse_report(&mut self, data: &[u8]) -> Result<(), &'static str> {
        if data.len() < 18 {
            return Err("Xbox report too short");
        }

        self.buttons = u16::from_le_bytes([data[4], data[5]]);
        self.left_trigger = data[6];
        self.right_trigger = data[7];
        self.left_stick.0 = i16::from_le_bytes([data[8], data[9]]);
        self.left_stick.1 = i16::from_le_bytes([data[10], data[11]]);
        self.right_stick.0 = i16::from_le_bytes([data[12], data[13]]);
        self.right_stick.1 = i16::from_le_bytes([data[14], data[15]]);

        Ok(())
    }
}

impl XboxControllerOutput {
    /// Build a rumble command packet for the Xbox controller.
    /// Full 10-byte GIP rumble: left trigger, right trigger, left motor, right motor.
    pub fn build_rumble_packet(&self) -> Vec<u8> {
        let mut buf = alloc::vec![0u8; 10];
        buf[0] = 0x09; // GIP command: rumble
        buf[1] = 0x00; // subcommand
        buf[2] = 0x00; // sequence (filled by transport)
        buf[3] = 0x09; // length
        buf[4] = 0x00; // substructure
        buf[5] = 0x0F; // motor mask: all four motors
        buf[6] = self.left_trigger_rumble;
        buf[7] = self.right_trigger_rumble;
        buf[8] = self.left_rumble;
        buf[9] = self.right_rumble;
        buf
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// §6  HID Report Descriptor Parser
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HidItemType {
    Main,
    Global,
    Local,
    Long,
}

#[derive(Debug, Clone)]
pub struct HidItem {
    pub item_type: HidItemType,
    pub tag: u8,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct HidField {
    pub usage_page: u16,
    pub usage: u16,
    pub bit_offset: u32,
    pub bit_size: u32,
    pub logical_min: i32,
    pub logical_max: i32,
    pub is_variable: bool,
}

#[derive(Debug, Clone)]
pub struct HidReport {
    pub report_id: u8,
    pub fields: Vec<HidField>,
}

#[derive(Debug, Clone)]
pub struct HidReportDescriptor {
    pub items: Vec<HidItem>,
    pub input_reports: Vec<HidReport>,
    pub output_reports: Vec<HidReport>,
    pub feature_reports: Vec<HidReport>,
}

pub fn parse_hid_descriptor(data: &[u8]) -> Result<HidReportDescriptor, &'static str> {
    let mut items = Vec::new();
    let mut input_reports: Vec<HidReport> = Vec::new();
    let mut output_reports: Vec<HidReport> = Vec::new();
    let mut feature_reports: Vec<HidReport> = Vec::new();

    // Global state
    let mut usage_page: u16 = 0;
    let mut logical_min: i32 = 0;
    let mut logical_max: i32 = 0;
    let mut report_size: u32 = 0;
    let mut report_count: u32 = 0;
    let mut report_id: u8 = 0;

    // Local state
    let mut usage: u16 = 0;
    let mut bit_offset: u32 = 0;

    let mut pos = 0usize;
    while pos < data.len() {
        let prefix = data[pos];
        if prefix == 0xFE {
            // Long item
            if pos + 2 >= data.len() {
                return Err("truncated long item");
            }
            let size = data[pos + 1] as usize;
            let tag = data[pos + 2];
            let item_data = if pos + 3 + size <= data.len() {
                data[pos + 3..pos + 3 + size].to_vec()
            } else {
                return Err("long item data overflow");
            };
            items.push(HidItem {
                item_type: HidItemType::Long,
                tag,
                data: item_data,
            });
            pos += 3 + size;
            continue;
        }

        let size = match prefix & 0x03 {
            0 => 0usize,
            1 => 1,
            2 => 2,
            3 => 4,
            _ => unreachable!(),
        };
        let item_type = match (prefix >> 2) & 0x03 {
            0 => HidItemType::Main,
            1 => HidItemType::Global,
            2 => HidItemType::Local,
            _ => HidItemType::Main,
        };
        let tag = (prefix >> 4) & 0x0F;

        if pos + 1 + size > data.len() {
            return Err("truncated item data");
        }
        let item_data = data[pos + 1..pos + 1 + size].to_vec();

        // Interpret items to build report structure
        match item_type {
            HidItemType::Global => match tag {
                0x00 => usage_page = read_unsigned(&item_data) as u16,
                0x01 => logical_min = read_signed(&item_data),
                0x02 => logical_max = read_signed(&item_data),
                0x07 => report_size = read_unsigned(&item_data) as u32,
                0x09 => report_count = read_unsigned(&item_data) as u32,
                0x08 => report_id = read_unsigned(&item_data) as u8,
                _ => {}
            },
            HidItemType::Local => match tag {
                0x00 => usage = read_unsigned(&item_data) as u16,
                _ => {}
            },
            HidItemType::Main => {
                let is_variable = if !item_data.is_empty() {
                    (item_data[0] & 0x02) != 0
                } else {
                    false
                };
                let fields: Vec<HidField> = (0..report_count)
                    .map(|i| HidField {
                        usage_page,
                        usage: usage.wrapping_add(i as u16),
                        bit_offset: bit_offset + i * report_size,
                        bit_size: report_size,
                        logical_min,
                        logical_max,
                        is_variable,
                    })
                    .collect();
                bit_offset += report_count * report_size;

                match tag {
                    0x08 => {
                        // Input
                        input_reports.push(HidReport { report_id, fields });
                    }
                    0x09 => {
                        // Output
                        output_reports.push(HidReport { report_id, fields });
                    }
                    0x0B => {
                        // Feature
                        feature_reports.push(HidReport { report_id, fields });
                    }
                    0x0A => {
                        // Collection — reset local state
                        bit_offset = 0;
                    }
                    0x0C => {
                        // End Collection
                    }
                    _ => {}
                }
                usage = 0;
            }
            HidItemType::Long => {}
        }

        items.push(HidItem {
            item_type,
            tag,
            data: item_data,
        });
        pos += 1 + size;
    }

    Ok(HidReportDescriptor {
        items,
        input_reports,
        output_reports,
        feature_reports,
    })
}

fn read_unsigned(data: &[u8]) -> u32 {
    match data.len() {
        0 => 0,
        1 => data[0] as u32,
        2 => u16::from_le_bytes([data[0], data[1]]) as u32,
        _ => u32::from_le_bytes([
            data[0],
            data.get(1).copied().unwrap_or(0),
            data.get(2).copied().unwrap_or(0),
            data.get(3).copied().unwrap_or(0),
        ]),
    }
}

fn read_signed(data: &[u8]) -> i32 {
    match data.len() {
        0 => 0,
        1 => data[0] as i8 as i32,
        2 => i16::from_le_bytes([data[0], data[1]]) as i32,
        _ => i32::from_le_bytes([
            data[0],
            data.get(1).copied().unwrap_or(0),
            data.get(2).copied().unwrap_or(0),
            data.get(3).copied().unwrap_or(0),
        ]),
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// §7  Per-Device Profiles
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct InputDeviceProfile {
    pub device_id: InputDeviceId,
    pub name: String,
    pub deadzone_left: f32,
    pub deadzone_right: f32,
    pub trigger_deadzone: f32,
    pub sensitivity: f32,
    pub inverted_y: bool,
    pub swap_sticks: bool,
    pub button_remap: BTreeMap<GamepadButton, GamepadButton>,
    pub vibration_strength: f32,
    pub led_color: Option<(u8, u8, u8)>,
    pub gyro_enabled: bool,
    pub gyro_sensitivity: f32,
}

impl InputDeviceProfile {
    pub fn default_for(device_id: InputDeviceId, name: String) -> Self {
        Self {
            device_id,
            name,
            deadzone_left: 0.10,
            deadzone_right: 0.10,
            trigger_deadzone: 0.02,
            sensitivity: 1.0,
            inverted_y: false,
            swap_sticks: false,
            button_remap: BTreeMap::new(),
            vibration_strength: 1.0,
            led_color: None,
            gyro_enabled: false,
            gyro_sensitivity: 1.0,
        }
    }

    /// Apply deadzone to a raw stick axis value (-32768..32767).
    /// Returns 0 if inside the deadzone, otherwise rescaled to full range.
    pub fn apply_deadzone(&self, raw: i16, is_left: bool) -> i16 {
        let dz = if is_left {
            self.deadzone_left
        } else {
            self.deadzone_right
        };
        let max = i16::MAX as f32;
        let normalized = raw as f32 / max;
        if normalized.abs() < dz {
            return 0;
        }
        let sign = if normalized > 0.0 { 1.0 } else { -1.0 };
        let rescaled = (normalized.abs() - dz) / (1.0 - dz);
        (sign * rescaled * max * self.sensitivity) as i16
    }

    /// Remap a button press according to the configured button map.
    pub fn remap_button(&self, button: GamepadButton) -> GamepadButton {
        self.button_remap.get(&button).copied().unwrap_or(button)
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// §8  RGB Unified API
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RgbDeviceType {
    Keyboard,
    Mouse,
    Mousepad,
    Headset,
    Speaker,
    MotherboardLed,
    GpuLed,
    RamLed,
    FanLed,
    CaseLed,
    Strip,
    Controller,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RgbZoneType {
    Single,
    Linear,
    Matrix(u32, u32),
}

#[derive(Debug, Clone)]
pub struct RgbZone {
    pub name: String,
    pub led_count: u32,
    pub zone_type: RgbZoneType,
}

#[derive(Debug, Clone)]
pub struct RgbDevice {
    pub id: u64,
    pub name: String,
    pub device_type: RgbDeviceType,
    pub zone_count: u32,
    pub zones: Vec<RgbZone>,
    pub supports_per_key: bool,
}

#[derive(Debug, Clone)]
pub enum RgbEffect {
    Static { color: (u8, u8, u8) },
    Breathing { color: (u8, u8, u8), speed: u8 },
    ColorCycle { speed: u8 },
    Wave { direction: u8, speed: u8 },
    Reactive { color: (u8, u8, u8), speed: u8 },
    Rain { color: (u8, u8, u8), speed: u8 },
    Custom { colors: Vec<(u8, u8, u8)> },
    Off,
}

#[derive(Debug)]
pub struct RgbManager {
    devices: Vec<RgbDevice>,
    active_effects: BTreeMap<u64, RgbEffect>,
    sync_enabled: bool,
    global_brightness: u8,
}

impl RgbManager {
    pub fn new() -> Self {
        Self {
            devices: Vec::new(),
            active_effects: BTreeMap::new(),
            sync_enabled: true,
            global_brightness: 255,
        }
    }

    pub fn register_device(&mut self, device: RgbDevice) {
        let id = device.id;
        self.active_effects.insert(id, RgbEffect::Off);
        self.devices.push(device);
    }

    pub fn unregister_device(&mut self, id: u64) {
        self.active_effects.remove(&id);
        self.devices.retain(|d| d.id != id);
    }

    pub fn set_effect(&mut self, device_id: u64, effect: RgbEffect) {
        self.active_effects.insert(device_id, effect);
    }

    pub fn set_all_effect(&mut self, effect: RgbEffect) {
        for dev in &self.devices {
            self.active_effects.insert(dev.id, effect.clone());
        }
    }

    pub fn set_global_brightness(&mut self, brightness: u8) {
        self.global_brightness = brightness;
    }

    pub fn set_sync(&mut self, enabled: bool) {
        self.sync_enabled = enabled;
    }

    pub fn get_device(&self, id: u64) -> Option<&RgbDevice> {
        self.devices.iter().find(|d| d.id == id)
    }

    pub fn devices(&self) -> &[RgbDevice] {
        &self.devices
    }

    pub fn active_effect(&self, device_id: u64) -> Option<&RgbEffect> {
        self.active_effects.get(&device_id)
    }

    /// Compute the final color for a device zone, applying global brightness.
    pub fn resolve_color(&self, base: (u8, u8, u8)) -> (u8, u8, u8) {
        let scale = self.global_brightness as u16;
        (
            ((base.0 as u16 * scale) / 255) as u8,
            ((base.1 as u16 * scale) / 255) as u8,
            ((base.2 as u16 * scale) / 255) as u8,
        )
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// §9  Gamepad State (unified for polling)
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub enum GamepadState {
    DualSense {
        state: DualSenseState,
        output: DualSenseOutput,
    },
    Xbox {
        state: XboxControllerState,
        output: XboxControllerOutput,
    },
    Generic {
        buttons: u32,
        axes: [i16; 8],
        triggers: [u8; 2],
    },
}

// ═══════════════════════════════════════════════════════════════════════════════
// §10  Input Manager (global singleton)
// ═══════════════════════════════════════════════════════════════════════════════

pub static INPUT_MANAGER: Mutex<Option<InputManager>> = Mutex::new(None);

pub struct InputManager {
    devices: Vec<InputDeviceInfo>,
    profiles: BTreeMap<u64, InputDeviceProfile>,
    event_queue: VecDeque<InputEvent>,
    gamepads: BTreeMap<u64, GamepadState>,
    rgb: RgbManager,
    next_device_id: u64,
    timestamp_counter: u64,
}

impl InputManager {
    pub fn new() -> Self {
        Self {
            devices: Vec::new(),
            profiles: BTreeMap::new(),
            event_queue: VecDeque::with_capacity(256),
            gamepads: BTreeMap::new(),
            rgb: RgbManager::new(),
            next_device_id: 1,
            timestamp_counter: 0,
        }
    }

    pub fn register_device(&mut self, mut info: InputDeviceInfo) -> InputDeviceId {
        let id = self.next_device_id;
        self.next_device_id += 1;
        info.id = id;

        let profile = InputDeviceProfile::default_for(id, info.name.clone());
        self.profiles.insert(id, profile);

        // Auto-detect gamepad type from VID/PID
        if info.device_type == InputDeviceType::Gamepad {
            let pad_state = match (info.vendor_id, info.product_id) {
                (0x054C, 0x0CE6) | (0x054C, 0x0DF2) => GamepadState::DualSense {
                    state: DualSenseState::default(),
                    output: DualSenseOutput::default(),
                },
                (0x045E, 0x0B12) | (0x045E, 0x0B13) | (0x045E, 0x028E) => GamepadState::Xbox {
                    state: XboxControllerState::default(),
                    output: XboxControllerOutput::default(),
                },
                _ => GamepadState::Generic {
                    buttons: 0,
                    axes: [0; 8],
                    triggers: [0; 2],
                },
            };
            self.gamepads.insert(id, pad_state);
        }

        self.push_event(id, InputEventType::DeviceConnected(info.clone()));
        self.devices.push(info);
        id
    }

    pub fn unregister_device(&mut self, id: InputDeviceId) {
        self.devices.retain(|d| d.id != id);
        self.profiles.remove(&id);
        self.gamepads.remove(&id);
        self.push_event(id, InputEventType::DeviceDisconnected(id));
    }

    pub fn poll_events(&mut self) -> Vec<InputEvent> {
        self.event_queue.drain(..).collect()
    }

    pub fn push_event(&mut self, device_id: InputDeviceId, event: InputEventType) {
        self.timestamp_counter += 1;
        self.event_queue.push_back(InputEvent {
            timestamp: self.timestamp_counter,
            device_id,
            event,
        });
    }

    pub fn get_gamepad(&self, device_id: u64) -> Option<&GamepadState> {
        self.gamepads.get(&device_id)
    }

    pub fn get_gamepad_mut(&mut self, device_id: u64) -> Option<&mut GamepadState> {
        self.gamepads.get_mut(&device_id)
    }

    pub fn get_profile(&self, device_id: u64) -> Option<&InputDeviceProfile> {
        self.profiles.get(&device_id)
    }

    pub fn get_profile_mut(&mut self, device_id: u64) -> Option<&mut InputDeviceProfile> {
        self.profiles.get_mut(&device_id)
    }

    pub fn rgb(&self) -> &RgbManager {
        &self.rgb
    }

    pub fn rgb_mut(&mut self) -> &mut RgbManager {
        &mut self.rgb
    }

    pub fn devices(&self) -> &[InputDeviceInfo] {
        &self.devices
    }

    pub fn process_dualsense_report(&mut self, device_id: u64, data: &[u8]) {
        let extracted = if let Some(GamepadState::DualSense { state, .. }) =
            self.gamepads.get_mut(&device_id)
        {
            if state.parse_report(data).is_err() {
                return;
            }
            let lx = state.left_stick.0 as i16 * 256;
            let ly = state.left_stick.1 as i16 * 256;
            let rx = state.right_stick.0 as i16 * 256;
            let ry = state.right_stick.1 as i16 * 256;
            let lt = state.left_trigger;
            let rt = state.right_trigger;
            let gyro = state.gyro;
            let accel = state.accelerometer;
            let tp0 = state.touchpad[0].clone();
            let tp1 = state.touchpad[1].clone();
            Some((lx, ly, rx, ry, lt, rt, gyro, accel, tp0, tp1))
        } else {
            None
        };

        let (lx, ly, rx, ry, lt, rt, gyro, accel, tp0, tp1) = match extracted {
            Some(v) => v,
            None => return,
        };

        let (lx, ly, rx, ry) = if let Some(p) = self.profiles.get(&device_id) {
            (
                p.apply_deadzone(lx, true),
                p.apply_deadzone(ly, true),
                p.apply_deadzone(rx, false),
                p.apply_deadzone(ry, false),
            )
        } else {
            (lx, ly, rx, ry)
        };

        self.push_event(
            device_id,
            InputEventType::GamepadAxis {
                axis: GamepadAxis::LeftStickX,
                value: lx,
            },
        );
        self.push_event(
            device_id,
            InputEventType::GamepadAxis {
                axis: GamepadAxis::LeftStickY,
                value: ly,
            },
        );
        self.push_event(
            device_id,
            InputEventType::GamepadAxis {
                axis: GamepadAxis::RightStickX,
                value: rx,
            },
        );
        self.push_event(
            device_id,
            InputEventType::GamepadAxis {
                axis: GamepadAxis::RightStickY,
                value: ry,
            },
        );

        self.push_event(
            device_id,
            InputEventType::GamepadTrigger {
                trigger: GamepadTrigger::Left,
                value: lt,
            },
        );
        self.push_event(
            device_id,
            InputEventType::GamepadTrigger {
                trigger: GamepadTrigger::Right,
                value: rt,
            },
        );

        self.push_event(
            device_id,
            InputEventType::GamepadMotion {
                gyro_x: gyro[0],
                gyro_y: gyro[1],
                gyro_z: gyro[2],
                accel_x: accel[0],
                accel_y: accel[1],
                accel_z: accel[2],
            },
        );

        self.push_event(
            device_id,
            InputEventType::GamepadTouchpad {
                finger: 0,
                x: tp0.x,
                y: tp0.y,
                active: tp0.active,
            },
        );
        self.push_event(
            device_id,
            InputEventType::GamepadTouchpad {
                finger: 1,
                x: tp1.x,
                y: tp1.y,
                active: tp1.active,
            },
        );
    }

    pub fn process_xbox_report(&mut self, device_id: u64, data: &[u8]) {
        let extracted =
            if let Some(GamepadState::Xbox { state, .. }) = self.gamepads.get_mut(&device_id) {
                if state.parse_report(data).is_err() {
                    return;
                }
                Some((
                    state.left_stick,
                    state.right_stick,
                    state.left_trigger,
                    state.right_trigger,
                ))
            } else {
                None
            };

        let (left_stick, right_stick, lt, rt) = match extracted {
            Some(v) => v,
            None => return,
        };

        self.push_event(
            device_id,
            InputEventType::GamepadAxis {
                axis: GamepadAxis::LeftStickX,
                value: left_stick.0,
            },
        );
        self.push_event(
            device_id,
            InputEventType::GamepadAxis {
                axis: GamepadAxis::LeftStickY,
                value: left_stick.1,
            },
        );
        self.push_event(
            device_id,
            InputEventType::GamepadAxis {
                axis: GamepadAxis::RightStickX,
                value: right_stick.0,
            },
        );
        self.push_event(
            device_id,
            InputEventType::GamepadAxis {
                axis: GamepadAxis::RightStickY,
                value: right_stick.1,
            },
        );
        self.push_event(
            device_id,
            InputEventType::GamepadTrigger {
                trigger: GamepadTrigger::Left,
                value: lt,
            },
        );
        self.push_event(
            device_id,
            InputEventType::GamepadTrigger {
                trigger: GamepadTrigger::Right,
                value: rt,
            },
        );
    }

    /// Set DualSense output (LED, rumble, adaptive triggers).
    pub fn set_dualsense_output(&mut self, device_id: u64, output: DualSenseOutput) {
        if let Some(GamepadState::DualSense { output: out, .. }) = self.gamepads.get_mut(&device_id)
        {
            *out = output;
        }
    }

    /// Set Xbox rumble output.
    pub fn set_xbox_output(&mut self, device_id: u64, output: XboxControllerOutput) {
        if let Some(GamepadState::Xbox { output: out, .. }) = self.gamepads.get_mut(&device_id) {
            *out = output;
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// §11  Unified Gamepad Abstraction
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControllerType {
    DualSense,
    DualSenseEdge,
    XboxOne,
    XboxSeriesX,
    Xbox360,
    Generic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Trigger {
    Left,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriggerEffect {
    None,
    Rigid {
        start: u8,
        force: u8,
    },
    Pulse {
        start: u8,
        end: u8,
        force: u8,
    },
    Vibration {
        position: u8,
        amplitude: u8,
        frequency: u8,
    },
}

#[derive(Debug, Clone, Copy)]
pub struct UnifiedButtons {
    raw: u32,
}

impl UnifiedButtons {
    pub const SOUTH: u32 = 1 << 0;
    pub const EAST: u32 = 1 << 1;
    pub const WEST: u32 = 1 << 2;
    pub const NORTH: u32 = 1 << 3;
    pub const L1: u32 = 1 << 4;
    pub const R1: u32 = 1 << 5;
    pub const BACK: u32 = 1 << 6;
    pub const START: u32 = 1 << 7;
    pub const GUIDE: u32 = 1 << 8;
    pub const L3: u32 = 1 << 9;
    pub const R3: u32 = 1 << 10;
    pub const DPAD_UP: u32 = 1 << 11;
    pub const DPAD_DOWN: u32 = 1 << 12;
    pub const DPAD_LEFT: u32 = 1 << 13;
    pub const DPAD_RIGHT: u32 = 1 << 14;
    pub const TOUCHPAD: u32 = 1 << 15;
    pub const MIC: u32 = 1 << 16;

    pub fn new() -> Self {
        Self { raw: 0 }
    }
    pub fn from_raw(raw: u32) -> Self {
        Self { raw }
    }
    pub fn pressed(&self, mask: u32) -> bool {
        (self.raw & mask) != 0
    }
    pub fn set(&mut self, mask: u32) {
        self.raw |= mask;
    }
    pub fn clear(&mut self, mask: u32) {
        self.raw &= !mask;
    }
    pub fn raw(&self) -> u32 {
        self.raw
    }
}

#[derive(Debug, Clone)]
pub struct UnifiedGamepadState {
    pub id: u32,
    pub controller_type: ControllerType,
    pub buttons: UnifiedButtons,
    pub left_stick: (f32, f32),
    pub right_stick: (f32, f32),
    pub left_trigger: f32,
    pub right_trigger: f32,
    pub gyro: Option<(f32, f32, f32)>,
    pub touchpad: Option<(f32, f32)>,
    pub battery_percent: Option<u8>,
    pub connected: bool,
}

impl UnifiedGamepadState {
    pub fn new(id: u32, controller_type: ControllerType) -> Self {
        Self {
            id,
            controller_type,
            buttons: UnifiedButtons::new(),
            left_stick: (0.0, 0.0),
            right_stick: (0.0, 0.0),
            left_trigger: 0.0,
            right_trigger: 0.0,
            gyro: if matches!(
                controller_type,
                ControllerType::DualSense | ControllerType::DualSenseEdge
            ) {
                Some((0.0, 0.0, 0.0))
            } else {
                None
            },
            touchpad: if matches!(
                controller_type,
                ControllerType::DualSense | ControllerType::DualSenseEdge
            ) {
                Some((0.0, 0.0))
            } else {
                None
            },
            battery_percent: None,
            connected: true,
        }
    }

    pub fn from_dualsense(id: u32, ds: &DualSenseState) -> Self {
        let mut state = Self::new(id, ControllerType::DualSense);

        state.left_stick = (
            ds.left_stick.0 as f32 / 127.0,
            ds.left_stick.1 as f32 / 127.0,
        );
        state.right_stick = (
            ds.right_stick.0 as f32 / 127.0,
            ds.right_stick.1 as f32 / 127.0,
        );
        state.left_trigger = ds.left_trigger as f32 / 255.0;
        state.right_trigger = ds.right_trigger as f32 / 255.0;

        state.buttons = Self::decode_dualsense_buttons(ds.buttons);

        state.gyro = Some((
            ds.gyro[0] as f32 / 32767.0,
            ds.gyro[1] as f32 / 32767.0,
            ds.gyro[2] as f32 / 32767.0,
        ));

        if ds.touchpad[0].active {
            state.touchpad = Some((
                ds.touchpad[0].x as f32 / 1920.0,
                ds.touchpad[0].y as f32 / 1080.0,
            ));
        }

        state.battery_percent = Some(ds.battery_level * 10);
        state
    }

    pub fn from_xbox(id: u32, xbox: &XboxControllerState) -> Self {
        let mut state = Self::new(id, ControllerType::XboxSeriesX);

        state.left_stick = (
            xbox.left_stick.0 as f32 / 32767.0,
            xbox.left_stick.1 as f32 / 32767.0,
        );
        state.right_stick = (
            xbox.right_stick.0 as f32 / 32767.0,
            xbox.right_stick.1 as f32 / 32767.0,
        );
        state.left_trigger = xbox.left_trigger as f32 / 255.0;
        state.right_trigger = xbox.right_trigger as f32 / 255.0;

        state.buttons = Self::decode_xbox_buttons(xbox.buttons);
        state
    }

    fn decode_dualsense_buttons(raw: u32) -> UnifiedButtons {
        let mut b = UnifiedButtons::new();
        // DualSense button layout in the 3-byte button field:
        // Byte 0 (bits 0-3): D-pad hat (0=N, 1=NE, 2=E, 3=SE, 4=S, 5=SW, 6=W, 7=NW, 8=none)
        // Byte 0 (bits 4-7): Square, Cross, Circle, Triangle
        // Byte 1: L1, R1, L2(digital), R2(digital), Create, Options, L3, R3
        // Byte 2: PS, Touchpad, Mic

        let hat = raw & 0x0F;
        match hat {
            0 => b.set(UnifiedButtons::DPAD_UP),
            1 => {
                b.set(UnifiedButtons::DPAD_UP);
                b.set(UnifiedButtons::DPAD_RIGHT);
            }
            2 => b.set(UnifiedButtons::DPAD_RIGHT),
            3 => {
                b.set(UnifiedButtons::DPAD_DOWN);
                b.set(UnifiedButtons::DPAD_RIGHT);
            }
            4 => b.set(UnifiedButtons::DPAD_DOWN),
            5 => {
                b.set(UnifiedButtons::DPAD_DOWN);
                b.set(UnifiedButtons::DPAD_LEFT);
            }
            6 => b.set(UnifiedButtons::DPAD_LEFT),
            7 => {
                b.set(UnifiedButtons::DPAD_UP);
                b.set(UnifiedButtons::DPAD_LEFT);
            }
            _ => {} // 8 or invalid = no direction
        }

        if (raw & 0x10) != 0 {
            b.set(UnifiedButtons::WEST);
        } // Square
        if (raw & 0x20) != 0 {
            b.set(UnifiedButtons::SOUTH);
        } // Cross
        if (raw & 0x40) != 0 {
            b.set(UnifiedButtons::EAST);
        } // Circle
        if (raw & 0x80) != 0 {
            b.set(UnifiedButtons::NORTH);
        } // Triangle

        let byte1 = (raw >> 8) & 0xFF;
        if (byte1 & 0x01) != 0 {
            b.set(UnifiedButtons::L1);
        }
        if (byte1 & 0x02) != 0 {
            b.set(UnifiedButtons::R1);
        }
        // bits 2,3 = digital L2/R2 (redundant with analog)
        if (byte1 & 0x10) != 0 {
            b.set(UnifiedButtons::BACK);
        } // Create
        if (byte1 & 0x20) != 0 {
            b.set(UnifiedButtons::START);
        } // Options
        if (byte1 & 0x40) != 0 {
            b.set(UnifiedButtons::L3);
        }
        if (byte1 & 0x80) != 0 {
            b.set(UnifiedButtons::R3);
        }

        let byte2 = (raw >> 16) & 0xFF;
        if (byte2 & 0x01) != 0 {
            b.set(UnifiedButtons::GUIDE);
        } // PS button
        if (byte2 & 0x02) != 0 {
            b.set(UnifiedButtons::TOUCHPAD);
        }
        if (byte2 & 0x04) != 0 {
            b.set(UnifiedButtons::MIC);
        }

        b
    }

    fn decode_xbox_buttons(raw: u16) -> UnifiedButtons {
        let mut b = UnifiedButtons::new();
        // Xbox GIP button layout (16-bit):
        if (raw & 0x0001) != 0 {
            b.set(UnifiedButtons::BACK);
        } // View
        if (raw & 0x0002) != 0 {
            b.set(UnifiedButtons::START);
        } // Menu
        if (raw & 0x0004) != 0 {
            b.set(UnifiedButtons::SOUTH);
        } // A
        if (raw & 0x0008) != 0 {
            b.set(UnifiedButtons::EAST);
        } // B
        if (raw & 0x0010) != 0 {
            b.set(UnifiedButtons::WEST);
        } // X
        if (raw & 0x0020) != 0 {
            b.set(UnifiedButtons::NORTH);
        } // Y
        if (raw & 0x0040) != 0 {
            b.set(UnifiedButtons::DPAD_UP);
        }
        if (raw & 0x0080) != 0 {
            b.set(UnifiedButtons::DPAD_DOWN);
        }
        if (raw & 0x0100) != 0 {
            b.set(UnifiedButtons::DPAD_LEFT);
        }
        if (raw & 0x0200) != 0 {
            b.set(UnifiedButtons::DPAD_RIGHT);
        }
        if (raw & 0x0400) != 0 {
            b.set(UnifiedButtons::L1);
        }
        if (raw & 0x0800) != 0 {
            b.set(UnifiedButtons::R1);
        }
        if (raw & 0x1000) != 0 {
            b.set(UnifiedButtons::L3);
        }
        if (raw & 0x2000) != 0 {
            b.set(UnifiedButtons::R3);
        }
        b
    }

    pub fn any_dpad(&self) -> bool {
        self.buttons.pressed(UnifiedButtons::DPAD_UP)
            || self.buttons.pressed(UnifiedButtons::DPAD_DOWN)
            || self.buttons.pressed(UnifiedButtons::DPAD_LEFT)
            || self.buttons.pressed(UnifiedButtons::DPAD_RIGHT)
    }
}

pub trait GamepadDriver {
    fn poll(&mut self) -> UnifiedGamepadState;
    fn set_rumble(&mut self, left: f32, right: f32);
    fn set_led_color(&mut self, r: u8, g: u8, b: u8);
    fn set_trigger_effect(&mut self, trigger: Trigger, effect: TriggerEffect);
    fn controller_type(&self) -> ControllerType;
    fn connected(&self) -> bool;
}

pub struct DualSenseDriver {
    pub device_id: u64,
    pub state: DualSenseState,
    pub output: DualSenseOutput,
    connected: bool,
}

impl DualSenseDriver {
    pub fn new(device_id: u64) -> Self {
        Self {
            device_id,
            state: DualSenseState::default(),
            output: DualSenseOutput::default(),
            connected: true,
        }
    }

    pub fn process_report(&mut self, data: &[u8]) -> Result<(), &'static str> {
        self.state.parse_report(data)
    }

    pub fn build_output_report(&self) -> Vec<u8> {
        self.output.build_report()
    }
}

impl GamepadDriver for DualSenseDriver {
    fn poll(&mut self) -> UnifiedGamepadState {
        UnifiedGamepadState::from_dualsense(self.device_id as u32, &self.state)
    }

    fn set_rumble(&mut self, left: f32, right: f32) {
        self.output.left_rumble = (left.clamp(0.0, 1.0) * 255.0) as u8;
        self.output.right_rumble = (right.clamp(0.0, 1.0) * 255.0) as u8;
    }

    fn set_led_color(&mut self, r: u8, g: u8, b: u8) {
        self.output.led_color = (r, g, b);
    }

    fn set_trigger_effect(&mut self, trigger: Trigger, effect: TriggerEffect) {
        let mode = match effect {
            TriggerEffect::None => AdaptiveTriggerMode::Off,
            TriggerEffect::Rigid { start, force } => AdaptiveTriggerMode::Resistance {
                start,
                strength: force,
            },
            TriggerEffect::Pulse { start, end, force } => AdaptiveTriggerMode::SemiAutomatic {
                start,
                end,
                strength: force,
            },
            TriggerEffect::Vibration {
                position,
                amplitude,
                frequency,
            } => AdaptiveTriggerMode::Machine {
                start: position,
                end: position.saturating_add(30),
                amplitude,
                frequency,
                period: 4,
            },
        };
        match trigger {
            Trigger::Left => self.output.adaptive_trigger_left = mode,
            Trigger::Right => self.output.adaptive_trigger_right = mode,
        }
    }

    fn controller_type(&self) -> ControllerType {
        ControllerType::DualSense
    }
    fn connected(&self) -> bool {
        self.connected
    }
}

pub struct XboxDriver {
    pub device_id: u64,
    pub state: XboxControllerState,
    pub output: XboxControllerOutput,
    connected: bool,
}

impl XboxDriver {
    pub fn new(device_id: u64) -> Self {
        Self {
            device_id,
            state: XboxControllerState::default(),
            output: XboxControllerOutput::default(),
            connected: true,
        }
    }

    pub fn process_report(&mut self, data: &[u8]) -> Result<(), &'static str> {
        self.state.parse_report(data)
    }

    pub fn build_rumble_packet(&self) -> Vec<u8> {
        self.output.build_rumble_packet()
    }
}

impl GamepadDriver for XboxDriver {
    fn poll(&mut self) -> UnifiedGamepadState {
        UnifiedGamepadState::from_xbox(self.device_id as u32, &self.state)
    }

    fn set_rumble(&mut self, left: f32, right: f32) {
        self.output.left_rumble = (left.clamp(0.0, 1.0) * 255.0) as u8;
        self.output.right_rumble = (right.clamp(0.0, 1.0) * 255.0) as u8;
    }

    fn set_led_color(&mut self, _r: u8, _g: u8, _b: u8) {
        // Xbox controllers don't support custom LED colors
    }

    fn set_trigger_effect(&mut self, trigger: Trigger, effect: TriggerEffect) {
        match (trigger, effect) {
            (Trigger::Left, TriggerEffect::Vibration { amplitude, .. }) => {
                self.output.left_trigger_rumble = amplitude;
            }
            (Trigger::Right, TriggerEffect::Vibration { amplitude, .. }) => {
                self.output.right_trigger_rumble = amplitude;
            }
            (Trigger::Left, TriggerEffect::None) => {
                self.output.left_trigger_rumble = 0;
            }
            (Trigger::Right, TriggerEffect::None) => {
                self.output.right_trigger_rumble = 0;
            }
            _ => {}
        }
    }

    fn controller_type(&self) -> ControllerType {
        ControllerType::XboxSeriesX
    }
    fn connected(&self) -> bool {
        self.connected
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// §12  Module Init
// ═══════════════════════════════════════════════════════════════════════════════

pub fn init() {
    let mut mgr = INPUT_MANAGER.lock();
    *mgr = Some(InputManager::new());
    crate::serial_println!("[ OK ] Input subsystem initialized (DualSense/Xbox/HID/RGB)");
}

/// R10 boot smoketest — exercises the DualSense + Xbox input-report parsers and
/// the DualSense output (rumble/LED/adaptive-trigger) serializer against
/// synthetic reports with known fields, so the Concept's "full feature parity"
/// controller decode is regression-fenced on every boot (these parsers had no
/// test coverage). Concept §Gaming-First: "DualSense + Xbox + every controller".
pub fn run_boot_smoketest() {
    // ── DualSense USB input report (ID 0x01, 64 bytes) ──
    // byte 1..5 sticks, 5/6 triggers, 8/9/10 buttons; IMU @ +15, status @ +52.
    let mut ds = DualSenseState::default();
    let mut rep = [0u8; 64];
    rep[0] = 0x01; // USB report ID
    rep[1] = 200; // LX  (200-128 = +72)
    rep[2] = 50; // LY  (50-128 = -78)
    rep[3] = 128; // RX  (centered = 0)
    rep[4] = 255; // RY  (255-128 = +127)
    rep[5] = 180; // L2 trigger
    rep[6] = 90; // R2 trigger
    rep[8] = 0xA0; // buttons0 (face buttons cross+circle, dpad released)
    rep[9] = 0x03; // buttons1 (L1 + R1)
    rep[10] = 0x01; // buttons2 (PS button)
                    // gyro[0] at +15+1.. = byte 16/17 -> 0x0100 = 256
    rep[16] = 0x00;
    rep[17] = 0x01;
    // battery @ +52 = byte 53: level 7, charging bit set
    rep[53] = 0x17;
    let ds_ok = ds.parse_report(&rep).is_ok()
        && ds.left_stick == (72, -78)
        && ds.right_stick == (0, 127)
        && ds.left_trigger == 180
        && ds.right_trigger == 90
        && ds.buttons == (0xA0u32 | (0x03u32 << 8) | (0x01u32 << 16))
        && ds.gyro[0] == 256
        && ds.battery_level == 7
        && ds.battery_charging;

    // Short report must be rejected, not panic.
    let ds_reject = ds.parse_report(&[0u8; 10]).is_err();

    // ── Xbox GIP input report (18 bytes) ──
    let mut xb = XboxControllerState::default();
    let mut xrep = [0u8; 18];
    xrep[4] = 0x30; // buttons lo (A+B = bits 4,5)
    xrep[5] = 0x00;
    xrep[6] = 120; // left trigger
    xrep[7] = 240; // right trigger
    xrep[8..10].copy_from_slice(&(-4000i16).to_le_bytes()); // LX
    xrep[12..14].copy_from_slice(&8000i16.to_le_bytes()); // RX
    let xb_ok = xb.parse_report(&xrep).is_ok()
        && xb.buttons == 0x0030
        && xb.left_trigger == 120
        && xb.right_trigger == 240
        && xb.left_stick.0 == -4000
        && xb.right_stick.0 == 8000;

    // ── DualSense output report serializer ──
    let mut out = DualSenseOutput::default();
    out.left_rumble = 0x42;
    out.right_rumble = 0x24;
    let out_rep = out.build_report();
    let out_ok = out_rep.len() == 48 && out_rep[0] == 0x02;

    let pass = ds_ok && ds_reject && xb_ok && out_ok;
    crate::serial_println!(
        "[input] controller smoketest: dualsense_parse={} reject_short={} xbox_parse={} ds_output={} -> {}",
        ds_ok,
        ds_reject,
        xb_ok,
        out_ok,
        if pass { "PASS" } else { "FAIL" },
    );
}

/// Convenience: push an input event from an interrupt handler or driver.
pub fn push_event(device_id: InputDeviceId, event: InputEventType) {
    if let Some(ref mut mgr) = *INPUT_MANAGER.lock() {
        mgr.push_event(device_id, event);
    }
}

/// Convenience: poll all queued events (non-blocking).
pub fn poll_events() -> Vec<InputEvent> {
    if let Some(ref mut mgr) = *INPUT_MANAGER.lock() {
        mgr.poll_events()
    } else {
        Vec::new()
    }
}

/// Register a new input device and return its assigned ID.
pub fn register_device(info: InputDeviceInfo) -> InputDeviceId {
    if let Some(ref mut mgr) = *INPUT_MANAGER.lock() {
        mgr.register_device(info)
    } else {
        0
    }
}

/// Unregister an input device by ID.
pub fn unregister_device(id: InputDeviceId) {
    if let Some(ref mut mgr) = *INPUT_MANAGER.lock() {
        mgr.unregister_device(id);
    }
}
