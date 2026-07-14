//! XInput + HID deep input subsystem for AthBridge.
//!
//! Full XInput 1.4 emulation, HID device parsing, and extended controller support.

#![allow(non_camel_case_types, non_snake_case, dead_code)]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};

// ---------------------------------------------------------------------------
// XInput error codes
// ---------------------------------------------------------------------------

pub const ERROR_SUCCESS: u32 = 0;
pub const ERROR_DEVICE_NOT_CONNECTED: u32 = 1167;
pub const ERROR_EMPTY: u32 = 4306;

// ---------------------------------------------------------------------------
// XInput button masks
// ---------------------------------------------------------------------------

pub const XINPUT_GAMEPAD_DPAD_UP: u16 = 0x0001;
pub const XINPUT_GAMEPAD_DPAD_DOWN: u16 = 0x0002;
pub const XINPUT_GAMEPAD_DPAD_LEFT: u16 = 0x0004;
pub const XINPUT_GAMEPAD_DPAD_RIGHT: u16 = 0x0008;
pub const XINPUT_GAMEPAD_START: u16 = 0x0010;
pub const XINPUT_GAMEPAD_BACK: u16 = 0x0020;
pub const XINPUT_GAMEPAD_LEFT_THUMB: u16 = 0x0040;
pub const XINPUT_GAMEPAD_RIGHT_THUMB: u16 = 0x0080;
pub const XINPUT_GAMEPAD_LEFT_SHOULDER: u16 = 0x0100;
pub const XINPUT_GAMEPAD_RIGHT_SHOULDER: u16 = 0x0200;
pub const XINPUT_GAMEPAD_GUIDE: u16 = 0x0400;
pub const XINPUT_GAMEPAD_A: u16 = 0x1000;
pub const XINPUT_GAMEPAD_B: u16 = 0x2000;
pub const XINPUT_GAMEPAD_X: u16 = 0x4000;
pub const XINPUT_GAMEPAD_Y: u16 = 0x8000;

// ---------------------------------------------------------------------------
// Deadzone constants
// ---------------------------------------------------------------------------

pub const XINPUT_GAMEPAD_LEFT_THUMB_DEADZONE: i16 = 7849;
pub const XINPUT_GAMEPAD_RIGHT_THUMB_DEADZONE: i16 = 8689;
pub const XINPUT_GAMEPAD_TRIGGER_THRESHOLD: u8 = 30;

// ---------------------------------------------------------------------------
// Battery types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum BatteryType {
    Disconnected = 0x00,
    Wired = 0x01,
    Alkaline = 0x02,
    NiMH = 0x03,
    Unknown = 0xFF,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum BatteryLevel {
    Empty = 0x00,
    Low = 0x01,
    Medium = 0x02,
    Full = 0x03,
}

#[derive(Debug, Clone, Copy)]
pub struct XINPUT_BATTERY_INFORMATION {
    pub battery_type: BatteryType,
    pub battery_level: BatteryLevel,
}

// ---------------------------------------------------------------------------
// Gamepad state
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Default)]
pub struct XINPUT_GAMEPAD {
    pub buttons: u16,
    pub left_trigger: u8,
    pub right_trigger: u8,
    pub thumb_lx: i16,
    pub thumb_ly: i16,
    pub thumb_rx: i16,
    pub thumb_ry: i16,
}

impl XINPUT_GAMEPAD {
    pub fn is_button_pressed(&self, button: u16) -> bool {
        self.buttons & button != 0
    }

    pub fn left_stick_magnitude(&self) -> f32 {
        let x = self.thumb_lx as f32;
        let y = self.thumb_ly as f32;
        libm::sqrtf(x * x + y * y)
    }

    pub fn right_stick_magnitude(&self) -> f32 {
        let x = self.thumb_rx as f32;
        let y = self.thumb_ry as f32;
        libm::sqrtf(x * x + y * y)
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct XINPUT_STATE {
    pub packet_number: u32,
    pub gamepad: XINPUT_GAMEPAD,
}

#[derive(Debug, Clone, Copy)]
pub struct XINPUT_VIBRATION {
    pub left_motor_speed: u16,
    pub right_motor_speed: u16,
}

// ---------------------------------------------------------------------------
// Capabilities
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ControllerType {
    Gamepad = 0x01,
    Wheel = 0x02,
    ArcadeStick = 0x03,
    FlightStick = 0x04,
    DancePad = 0x05,
    Guitar = 0x06,
    DrumKit = 0x08,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ControllerSubType {
    Unknown = 0x00,
    Gamepad = 0x01,
    Wheel = 0x02,
    ArcadeStick = 0x03,
    FlightStick = 0x04,
    DancePad = 0x05,
    Guitar = 0x06,
    GuitarAlternate = 0x07,
    DrumKit = 0x08,
    GuitarBass = 0x0B,
    ArcadePad = 0x13,
}

#[derive(Debug, Clone, Copy)]
pub struct XINPUT_CAPABILITIES {
    pub controller_type: ControllerType,
    pub sub_type: ControllerSubType,
    pub flags: u16,
    pub gamepad: XINPUT_GAMEPAD,
    pub vibration: XINPUT_VIBRATION,
}

// ---------------------------------------------------------------------------
// Keystroke
// ---------------------------------------------------------------------------

pub const XINPUT_KEYSTROKE_KEYDOWN: u16 = 0x0001;
pub const XINPUT_KEYSTROKE_KEYUP: u16 = 0x0002;
pub const XINPUT_KEYSTROKE_REPEAT: u16 = 0x0004;

#[derive(Debug, Clone, Copy)]
pub struct XINPUT_KEYSTROKE {
    pub virtual_key: u16,
    pub unicode: u16,
    pub flags: u16,
    pub user_index: u8,
    pub hid_code: u8,
}

// ---------------------------------------------------------------------------
// Deadzone handling
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeadzoneShape {
    Circular,
    Square,
}

#[derive(Debug, Clone, Copy)]
pub struct DeadzoneConfig {
    pub shape: DeadzoneShape,
    pub left_stick_threshold: f32,
    pub right_stick_threshold: f32,
    pub trigger_threshold: f32,
}

impl Default for DeadzoneConfig {
    fn default() -> Self {
        Self {
            shape: DeadzoneShape::Circular,
            left_stick_threshold: XINPUT_GAMEPAD_LEFT_THUMB_DEADZONE as f32 / 32767.0,
            right_stick_threshold: XINPUT_GAMEPAD_RIGHT_THUMB_DEADZONE as f32 / 32767.0,
            trigger_threshold: XINPUT_GAMEPAD_TRIGGER_THRESHOLD as f32 / 255.0,
        }
    }
}

pub fn apply_circular_deadzone(x: i16, y: i16, threshold: f32) -> (f32, f32) {
    let fx = x as f32 / 32767.0;
    let fy = y as f32 / 32767.0;
    let magnitude = libm::sqrtf(fx * fx + fy * fy);

    if magnitude < threshold {
        return (0.0, 0.0);
    }

    let normalized_magnitude = (magnitude - threshold) / (1.0 - threshold);
    let scale = normalized_magnitude / magnitude;
    (fx * scale, fy * scale)
}

pub fn apply_square_deadzone(x: i16, y: i16, threshold: f32) -> (f32, f32) {
    let fx = x as f32 / 32767.0;
    let fy = y as f32 / 32767.0;

    let ax = if libm::fabsf(fx) < threshold {
        0.0
    } else {
        let sign = if fx > 0.0 { 1.0 } else { -1.0 };
        sign * (libm::fabsf(fx) - threshold) / (1.0 - threshold)
    };

    let ay = if libm::fabsf(fy) < threshold {
        0.0
    } else {
        let sign = if fy > 0.0 { 1.0 } else { -1.0 };
        sign * (libm::fabsf(fy) - threshold) / (1.0 - threshold)
    };

    (ax, ay)
}

pub fn apply_trigger_deadzone(value: u8, threshold: f32) -> f32 {
    let fv = value as f32 / 255.0;
    if fv < threshold {
        0.0
    } else {
        (fv - threshold) / (1.0 - threshold)
    }
}

// ---------------------------------------------------------------------------
// Force feedback effects
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum ForceFeedbackType {
    Constant = 0,
    Ramp = 1,
    Square = 2,
    Sine = 3,
    Triangle = 4,
    SawtoothUp = 5,
    SawtoothDown = 6,
    Spring = 7,
    Damper = 8,
    Inertia = 9,
    Friction = 10,
}

#[derive(Debug, Clone, Copy)]
pub struct ForceFeedbackEnvelope {
    pub attack_level: u16,
    pub attack_time: u32,
    pub fade_level: u16,
    pub fade_time: u32,
}

#[derive(Debug, Clone, Copy)]
pub struct ForceFeedbackEffect {
    pub effect_type: ForceFeedbackType,
    pub duration_ms: u32,
    pub magnitude: i16,
    pub offset: i16,
    pub phase: u16,
    pub period: u32,
    pub envelope: Option<ForceFeedbackEnvelope>,
    pub direction_x: i32,
    pub direction_y: i32,
}

// ---------------------------------------------------------------------------
// Rumble / impulse triggers
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
pub struct ImpulseTriggerVibration {
    pub left_trigger: u16,
    pub right_trigger: u16,
    pub left_motor: u16,
    pub right_motor: u16,
}

// ---------------------------------------------------------------------------
// HID types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum HidUsagePage {
    GenericDesktop = 0x01,
    Simulation = 0x02,
    VR = 0x03,
    Sport = 0x04,
    Game = 0x05,
    GenericDevice = 0x06,
    Keyboard = 0x07,
    LED = 0x08,
    Button = 0x09,
    Ordinal = 0x0A,
    Telephony = 0x0B,
    Consumer = 0x0C,
    Digitizers = 0x0D,
    Haptics = 0x0E,
    PID = 0x0F,
    VendorDefined = 0xFF00,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum HidGenericDesktopUsage {
    Pointer = 0x01,
    Mouse = 0x02,
    Joystick = 0x04,
    Gamepad = 0x05,
    Keyboard = 0x06,
    Keypad = 0x07,
    MultiAxisController = 0x08,
    X = 0x30,
    Y = 0x31,
    Z = 0x32,
    Rx = 0x33,
    Ry = 0x34,
    Rz = 0x35,
    Slider = 0x36,
    Dial = 0x37,
    Wheel = 0x38,
    HatSwitch = 0x39,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum HidCollectionType {
    Physical = 0x00,
    Application = 0x01,
    Logical = 0x02,
    Report = 0x03,
    NamedArray = 0x04,
    UsageSwitch = 0x05,
    UsageModifier = 0x06,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HidReportType {
    Input,
    Output,
    Feature,
}

#[derive(Debug, Clone)]
pub struct HidReportField {
    pub usage_page: u16,
    pub usage_id: u16,
    pub report_type: HidReportType,
    pub logical_min: i32,
    pub logical_max: i32,
    pub physical_min: i32,
    pub physical_max: i32,
    pub unit: u32,
    pub unit_exponent: i8,
    pub report_size: u32,
    pub report_count: u32,
    pub bit_offset: u32,
    pub is_variable: bool,
    pub is_relative: bool,
    pub is_wrapping: bool,
    pub is_nonlinear: bool,
    pub has_null_state: bool,
    pub is_buffered: bool,
}

#[derive(Debug, Clone)]
pub struct HidReportDescriptor {
    pub report_id: u8,
    pub report_type: HidReportType,
    pub fields: Vec<HidReportField>,
    pub total_bits: u32,
}

#[derive(Debug, Clone)]
pub struct HidCollection {
    pub collection_type: HidCollectionType,
    pub usage_page: u16,
    pub usage_id: u16,
    pub children: Vec<HidCollection>,
    pub fields: Vec<HidReportField>,
}

// ---------------------------------------------------------------------------
// HID device
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct HidDeviceInfo {
    pub vendor_id: u16,
    pub product_id: u16,
    pub version_number: u16,
    pub device_path: String,
    pub manufacturer: String,
    pub product_name: String,
    pub serial_number: String,
    pub usage_page: u16,
    pub usage: u16,
    pub input_report_length: u16,
    pub output_report_length: u16,
    pub feature_report_length: u16,
}

#[derive(Debug, Clone)]
pub struct HidDevice {
    pub info: HidDeviceInfo,
    pub collections: Vec<HidCollection>,
    pub input_reports: Vec<HidReportDescriptor>,
    pub output_reports: Vec<HidReportDescriptor>,
    pub feature_reports: Vec<HidReportDescriptor>,
    pub connected: bool,
}

impl HidDevice {
    pub fn parse_report(&self, report_type: HidReportType, data: &[u8]) -> Vec<i32> {
        let mut values = Vec::new();
        let reports: &[HidReportDescriptor] = match report_type {
            HidReportType::Input => &self.input_reports,
            HidReportType::Output => &self.output_reports,
            HidReportType::Feature => &self.feature_reports,
        };

        if data.is_empty() {
            return values;
        }

        let report_id = data[0];
        for report in reports {
            if report.report_id != report_id {
                continue;
            }
            for field in &report.fields {
                let bit_start = field.bit_offset as usize;
                let bit_size = field.report_size as usize;
                for i in 0..field.report_count as usize {
                    let offset = bit_start + i * bit_size;
                    let byte_idx = (offset / 8) + 1; // +1 for report ID
                    if byte_idx >= data.len() {
                        values.push(0);
                        continue;
                    }
                    let bit_in_byte = offset % 8;
                    let mask = ((1u32 << bit_size) - 1) as u8;
                    let raw = (data[byte_idx] >> bit_in_byte) & mask;
                    values.push(raw as i32);
                }
            }
        }
        values
    }
}

// ---------------------------------------------------------------------------
// Controller profiles
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControllerProfile {
    Xbox360,
    XboxOne,
    XboxSeries,
    PS4DualShock4,
    PS5DualSense,
    SwitchPro,
    GenericHID,
}

impl ControllerProfile {
    pub fn from_vid_pid(vid: u16, pid: u16) -> Self {
        match (vid, pid) {
            (0x045E, 0x028E) => Self::Xbox360,
            (0x045E, 0x02D1) | (0x045E, 0x02DD) => Self::XboxOne,
            (0x045E, 0x0B12) | (0x045E, 0x0B13) => Self::XboxSeries,
            (0x054C, 0x05C4) | (0x054C, 0x09CC) => Self::PS4DualShock4,
            (0x054C, 0x0CE6) | (0x054C, 0x0DF2) => Self::PS5DualSense,
            (0x057E, 0x2009) => Self::SwitchPro,
            _ => Self::GenericHID,
        }
    }

    pub fn has_motion_sensors(&self) -> bool {
        matches!(
            self,
            Self::PS4DualShock4 | Self::PS5DualSense | Self::SwitchPro
        )
    }

    pub fn has_touchpad(&self) -> bool {
        matches!(self, Self::PS4DualShock4 | Self::PS5DualSense)
    }

    pub fn has_adaptive_triggers(&self) -> bool {
        matches!(self, Self::PS5DualSense)
    }

    pub fn has_lightbar(&self) -> bool {
        matches!(self, Self::PS4DualShock4 | Self::PS5DualSense)
    }

    pub fn has_impulse_triggers(&self) -> bool {
        matches!(self, Self::XboxOne | Self::XboxSeries)
    }
}

// ---------------------------------------------------------------------------
// Button remapping
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GamepadButton {
    A,
    B,
    X,
    Y,
    DPadUp,
    DPadDown,
    DPadLeft,
    DPadRight,
    Start,
    Back,
    Guide,
    LeftThumb,
    RightThumb,
    LeftShoulder,
    RightShoulder,
    LeftTrigger,
    RightTrigger,
}

#[derive(Debug, Clone)]
pub struct ButtonRemapEntry {
    pub source: GamepadButton,
    pub target: GamepadButton,
}

#[derive(Debug, Clone)]
pub struct AxisConfig {
    pub inverted: bool,
    pub sensitivity: f32,
    pub deadzone_override: Option<f32>,
}

impl Default for AxisConfig {
    fn default() -> Self {
        Self {
            inverted: false,
            sensitivity: 1.0,
            deadzone_override: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ControllerRemapProfile {
    pub profile: ControllerProfile,
    pub button_remaps: Vec<ButtonRemapEntry>,
    pub left_stick_x: AxisConfig,
    pub left_stick_y: AxisConfig,
    pub right_stick_x: AxisConfig,
    pub right_stick_y: AxisConfig,
    pub left_trigger: AxisConfig,
    pub right_trigger: AxisConfig,
}

impl ControllerRemapProfile {
    pub fn new(profile: ControllerProfile) -> Self {
        Self {
            profile,
            button_remaps: Vec::new(),
            left_stick_x: AxisConfig::default(),
            left_stick_y: AxisConfig::default(),
            right_stick_x: AxisConfig::default(),
            right_stick_y: AxisConfig::default(),
            left_trigger: AxisConfig::default(),
            right_trigger: AxisConfig::default(),
        }
    }

    pub fn apply_axis(&self, value: f32, config: &AxisConfig) -> f32 {
        let v = if config.inverted { -value } else { value };
        let scaled = v * config.sensitivity;
        if scaled > 1.0 {
            1.0
        } else if scaled < -1.0 {
            -1.0
        } else {
            scaled
        }
    }
}

// ---------------------------------------------------------------------------
// Motion sensors
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Default)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct MotionState {
    pub accelerometer: Vec3,
    pub gyroscope: Vec3,
    pub timestamp_us: u64,
}

impl MotionState {
    pub fn acceleration_magnitude(&self) -> f32 {
        let a = &self.accelerometer;
        libm::sqrtf(a.x * a.x + a.y * a.y + a.z * a.z)
    }

    pub fn angular_velocity_magnitude(&self) -> f32 {
        let g = &self.gyroscope;
        libm::sqrtf(g.x * g.x + g.y * g.y + g.z * g.z)
    }
}

// ---------------------------------------------------------------------------
// Touchpad (PS4/PS5)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Default)]
pub struct TouchPoint {
    pub id: u8,
    pub active: bool,
    pub x: u16,
    pub y: u16,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct TouchpadState {
    pub points: [TouchPoint; 2],
    pub button_pressed: bool,
}

impl TouchpadState {
    pub fn active_touch_count(&self) -> u8 {
        self.points.iter().filter(|p| p.active).count() as u8
    }
}

// ---------------------------------------------------------------------------
// Adaptive triggers (PS5 DualSense)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdaptiveTriggerMode {
    Off,
    Feedback {
        position: u8,
        strength: u8,
    },
    Weapon {
        start_position: u8,
        end_position: u8,
        strength: u8,
    },
    Vibration {
        position: u8,
        amplitude: u8,
        frequency: u8,
    },
}

#[derive(Debug, Clone, Copy)]
pub struct AdaptiveTriggerState {
    pub left_mode: AdaptiveTriggerMode,
    pub right_mode: AdaptiveTriggerMode,
}

impl Default for AdaptiveTriggerState {
    fn default() -> Self {
        Self {
            left_mode: AdaptiveTriggerMode::Off,
            right_mode: AdaptiveTriggerMode::Off,
        }
    }
}

impl AdaptiveTriggerState {
    pub fn encode_left(&self) -> [u8; 11] {
        Self::encode_mode(&self.left_mode)
    }

    pub fn encode_right(&self) -> [u8; 11] {
        Self::encode_mode(&self.right_mode)
    }

    fn encode_mode(mode: &AdaptiveTriggerMode) -> [u8; 11] {
        let mut data = [0u8; 11];
        match mode {
            AdaptiveTriggerMode::Off => {
                data[0] = 0x00;
            }
            AdaptiveTriggerMode::Feedback { position, strength } => {
                data[0] = 0x01;
                data[1] = *position;
                data[2] = *strength;
            }
            AdaptiveTriggerMode::Weapon {
                start_position,
                end_position,
                strength,
            } => {
                data[0] = 0x02;
                data[1] = *start_position;
                data[2] = *end_position;
                data[3] = *strength;
            }
            AdaptiveTriggerMode::Vibration {
                position,
                amplitude,
                frequency,
            } => {
                data[0] = 0x06;
                data[1] = *position;
                data[2] = *amplitude;
                data[3] = *frequency;
            }
        }
        data
    }
}

// ---------------------------------------------------------------------------
// LED / lightbar control
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Default)]
pub struct LightbarColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

#[derive(Debug, Clone, Copy)]
pub struct LightbarState {
    pub color: LightbarColor,
    pub on_duration_ms: u16,
    pub off_duration_ms: u16,
}

impl Default for LightbarState {
    fn default() -> Self {
        Self {
            color: LightbarColor { r: 0, g: 0, b: 255 },
            on_duration_ms: 0,
            off_duration_ms: 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PlayerIndicator {
    Off = 0,
    Player1 = 0x04,
    Player2 = 0x0A,
    Player3 = 0x15,
    Player4 = 0x1B,
}

// ---------------------------------------------------------------------------
// Controller hotplug events
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControllerEvent {
    Connected(u8),
    Disconnected(u8),
}

// ---------------------------------------------------------------------------
// Full controller state
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct ControllerState {
    pub user_index: u8,
    pub connected: bool,
    pub profile: ControllerProfile,
    pub xinput_state: XINPUT_STATE,
    pub vibration: XINPUT_VIBRATION,
    pub impulse_triggers: Option<ImpulseTriggerVibration>,
    pub battery: XINPUT_BATTERY_INFORMATION,
    pub motion: Option<MotionState>,
    pub touchpad: Option<TouchpadState>,
    pub adaptive_triggers: Option<AdaptiveTriggerState>,
    pub lightbar: Option<LightbarState>,
    pub player_indicator: PlayerIndicator,
    pub deadzone_config: DeadzoneConfig,
    pub remap_profile: Option<ControllerRemapProfile>,
    pub force_feedback_effects: Vec<ForceFeedbackEffect>,
}

impl ControllerState {
    pub fn new(user_index: u8) -> Self {
        Self {
            user_index,
            connected: false,
            profile: ControllerProfile::GenericHID,
            xinput_state: XINPUT_STATE::default(),
            vibration: XINPUT_VIBRATION {
                left_motor_speed: 0,
                right_motor_speed: 0,
            },
            impulse_triggers: None,
            battery: XINPUT_BATTERY_INFORMATION {
                battery_type: BatteryType::Disconnected,
                battery_level: BatteryLevel::Empty,
            },
            motion: None,
            touchpad: None,
            adaptive_triggers: None,
            lightbar: None,
            player_indicator: PlayerIndicator::Off,
            deadzone_config: DeadzoneConfig::default(),
            remap_profile: None,
            force_feedback_effects: Vec::new(),
        }
    }

    pub fn connect(&mut self, profile: ControllerProfile) {
        self.connected = true;
        self.profile = profile;
        self.battery.battery_type = BatteryType::Unknown;
        self.battery.battery_level = BatteryLevel::Full;

        if profile.has_motion_sensors() {
            self.motion = Some(MotionState::default());
        }
        if profile.has_touchpad() {
            self.touchpad = Some(TouchpadState::default());
        }
        if profile.has_adaptive_triggers() {
            self.adaptive_triggers = Some(AdaptiveTriggerState::default());
        }
        if profile.has_lightbar() {
            self.lightbar = Some(LightbarState::default());
        }
        if profile.has_impulse_triggers() {
            self.impulse_triggers = Some(ImpulseTriggerVibration {
                left_trigger: 0,
                right_trigger: 0,
                left_motor: 0,
                right_motor: 0,
            });
        }

        self.player_indicator = match self.user_index {
            0 => PlayerIndicator::Player1,
            1 => PlayerIndicator::Player2,
            2 => PlayerIndicator::Player3,
            3 => PlayerIndicator::Player4,
            _ => PlayerIndicator::Off,
        };
    }

    pub fn disconnect(&mut self) {
        self.connected = false;
        self.battery.battery_type = BatteryType::Disconnected;
        self.battery.battery_level = BatteryLevel::Empty;
        self.motion = None;
        self.touchpad = None;
        self.adaptive_triggers = None;
        self.lightbar = None;
        self.impulse_triggers = None;
        self.xinput_state = XINPUT_STATE::default();
    }
}

// ---------------------------------------------------------------------------
// XInput API functions
// ---------------------------------------------------------------------------

pub struct XInputSystem {
    pub controllers: [ControllerState; 4],
    pub enabled: bool,
    pub hid_devices: Vec<HidDevice>,
    pub event_queue: Vec<ControllerEvent>,
}

impl XInputSystem {
    pub fn new() -> Self {
        Self {
            controllers: [
                ControllerState::new(0),
                ControllerState::new(1),
                ControllerState::new(2),
                ControllerState::new(3),
            ],
            enabled: true,
            hid_devices: Vec::new(),
            event_queue: Vec::new(),
        }
    }

    pub fn xinput_get_state(&self, user_index: u32) -> Result<XINPUT_STATE, u32> {
        if !self.enabled {
            return Err(ERROR_DEVICE_NOT_CONNECTED);
        }
        if user_index >= 4 {
            return Err(ERROR_DEVICE_NOT_CONNECTED);
        }
        let controller = &self.controllers[user_index as usize];
        if !controller.connected {
            return Err(ERROR_DEVICE_NOT_CONNECTED);
        }
        Ok(controller.xinput_state)
    }

    pub fn xinput_set_state(&mut self, user_index: u32, vibration: XINPUT_VIBRATION) -> u32 {
        if !self.enabled {
            return ERROR_DEVICE_NOT_CONNECTED;
        }
        if user_index >= 4 {
            return ERROR_DEVICE_NOT_CONNECTED;
        }
        let controller = &mut self.controllers[user_index as usize];
        if !controller.connected {
            return ERROR_DEVICE_NOT_CONNECTED;
        }
        controller.vibration = vibration;
        ERROR_SUCCESS
    }

    pub fn xinput_get_capabilities(
        &self,
        user_index: u32,
        _flags: u32,
    ) -> Result<XINPUT_CAPABILITIES, u32> {
        if user_index >= 4 {
            return Err(ERROR_DEVICE_NOT_CONNECTED);
        }
        let controller = &self.controllers[user_index as usize];
        if !controller.connected {
            return Err(ERROR_DEVICE_NOT_CONNECTED);
        }
        Ok(XINPUT_CAPABILITIES {
            controller_type: ControllerType::Gamepad,
            sub_type: ControllerSubType::Gamepad,
            flags: 0x0004, // voice supported
            gamepad: XINPUT_GAMEPAD {
                buttons: 0xFFFF,
                left_trigger: 255,
                right_trigger: 255,
                thumb_lx: i16::MAX,
                thumb_ly: i16::MAX,
                thumb_rx: i16::MAX,
                thumb_ry: i16::MAX,
            },
            vibration: XINPUT_VIBRATION {
                left_motor_speed: u16::MAX,
                right_motor_speed: u16::MAX,
            },
        })
    }

    pub fn xinput_get_audio_device_ids(&self, user_index: u32) -> Result<(String, String), u32> {
        if user_index >= 4 {
            return Err(ERROR_DEVICE_NOT_CONNECTED);
        }
        let controller = &self.controllers[user_index as usize];
        if !controller.connected {
            return Err(ERROR_DEVICE_NOT_CONNECTED);
        }
        Ok((String::new(), String::new()))
    }

    pub fn xinput_get_battery_information(
        &self,
        user_index: u32,
        _dev_type: u8,
    ) -> Result<XINPUT_BATTERY_INFORMATION, u32> {
        if user_index >= 4 {
            return Err(ERROR_DEVICE_NOT_CONNECTED);
        }
        let controller = &self.controllers[user_index as usize];
        if !controller.connected {
            return Err(ERROR_DEVICE_NOT_CONNECTED);
        }
        Ok(controller.battery)
    }

    pub fn xinput_get_keystroke(&self, user_index: u32) -> Result<XINPUT_KEYSTROKE, u32> {
        if user_index >= 4 {
            return Err(ERROR_DEVICE_NOT_CONNECTED);
        }
        let controller = &self.controllers[user_index as usize];
        if !controller.connected {
            return Err(ERROR_DEVICE_NOT_CONNECTED);
        }
        Err(ERROR_EMPTY)
    }

    pub fn xinput_enable(&mut self, enable: bool) {
        self.enabled = enable;
        if !enable {
            for controller in &mut self.controllers {
                controller.vibration = XINPUT_VIBRATION {
                    left_motor_speed: 0,
                    right_motor_speed: 0,
                };
            }
        }
    }

    pub fn poll_events(&mut self) -> Vec<ControllerEvent> {
        core::mem::take(&mut self.event_queue)
    }

    pub fn connect_controller(&mut self, user_index: u8, profile: ControllerProfile) {
        if (user_index as usize) < 4 {
            self.controllers[user_index as usize].connect(profile);
            self.event_queue
                .push(ControllerEvent::Connected(user_index));
        }
    }

    pub fn disconnect_controller(&mut self, user_index: u8) {
        if (user_index as usize) < 4 {
            self.controllers[user_index as usize].disconnect();
            self.event_queue
                .push(ControllerEvent::Disconnected(user_index));
        }
    }

    pub fn update_state(&mut self, user_index: u8, gamepad: XINPUT_GAMEPAD) {
        if (user_index as usize) < 4 {
            let controller = &mut self.controllers[user_index as usize];
            controller.xinput_state.packet_number =
                controller.xinput_state.packet_number.wrapping_add(1);
            controller.xinput_state.gamepad = gamepad;
        }
    }

    pub fn update_motion(&mut self, user_index: u8, motion: MotionState) {
        if (user_index as usize) < 4 {
            let controller = &mut self.controllers[user_index as usize];
            controller.motion = Some(motion);
        }
    }

    pub fn update_touchpad(&mut self, user_index: u8, touchpad: TouchpadState) {
        if (user_index as usize) < 4 {
            let controller = &mut self.controllers[user_index as usize];
            controller.touchpad = Some(touchpad);
        }
    }

    pub fn set_adaptive_trigger(
        &mut self,
        user_index: u8,
        left: AdaptiveTriggerMode,
        right: AdaptiveTriggerMode,
    ) {
        if (user_index as usize) < 4 {
            let controller = &mut self.controllers[user_index as usize];
            if let Some(ref mut at) = controller.adaptive_triggers {
                at.left_mode = left;
                at.right_mode = right;
            }
        }
    }

    pub fn set_lightbar(&mut self, user_index: u8, color: LightbarColor, on_ms: u16, off_ms: u16) {
        if (user_index as usize) < 4 {
            let controller = &mut self.controllers[user_index as usize];
            if let Some(ref mut lb) = controller.lightbar {
                lb.color = color;
                lb.on_duration_ms = on_ms;
                lb.off_duration_ms = off_ms;
            }
        }
    }

    pub fn add_force_feedback(&mut self, user_index: u8, effect: ForceFeedbackEffect) {
        if (user_index as usize) < 4 {
            self.controllers[user_index as usize]
                .force_feedback_effects
                .push(effect);
        }
    }

    pub fn clear_force_feedback(&mut self, user_index: u8) {
        if (user_index as usize) < 4 {
            self.controllers[user_index as usize]
                .force_feedback_effects
                .clear();
        }
    }

    pub fn enumerate_hid_devices(&self) -> &[HidDevice] {
        &self.hid_devices
    }

    pub fn find_hid_device(&self, vid: u16, pid: u16) -> Option<&HidDevice> {
        self.hid_devices
            .iter()
            .find(|d| d.info.vendor_id == vid && d.info.product_id == pid)
    }

    pub fn register_hid_device(&mut self, device: HidDevice) {
        self.hid_devices.push(device);
    }
}

// ---------------------------------------------------------------------------
// Global INPUT_SYSTEM
// ---------------------------------------------------------------------------

pub struct InputSystem {
    pub initialized: AtomicBool,
    pub xinput: Option<XInputSystem>,
}

impl InputSystem {
    pub const fn new() -> Self {
        Self {
            initialized: AtomicBool::new(false),
            xinput: None,
        }
    }

    pub fn init(&mut self) {
        if self.initialized.load(Ordering::Acquire) {
            return;
        }
        self.xinput = Some(XInputSystem::new());
        self.initialized.store(true, Ordering::Release);
    }

    pub fn shutdown(&mut self) {
        self.xinput = None;
        self.initialized.store(false, Ordering::Release);
    }

    pub fn is_initialized(&self) -> bool {
        self.initialized.load(Ordering::Acquire)
    }

    pub fn xinput(&mut self) -> Option<&mut XInputSystem> {
        self.xinput.as_mut()
    }
}

pub static mut INPUT_SYSTEM: InputSystem = InputSystem::new();

pub fn init() {
    unsafe { INPUT_SYSTEM.init() }
}
