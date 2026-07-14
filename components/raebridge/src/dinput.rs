#![allow(dead_code)]

//! DirectInput, XInput, and DirectSound compatibility layer.
//!
//! Provides the input and audio APIs that Windows games use for controller,
//! keyboard, mouse, and sound buffer access. These are intercepted by
//! RaeBridge and mapped to RaeenOS native input/audio subsystems.

extern crate alloc;

use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::vec::Vec;

// ═══════════════════════════════════════════════════════════════════════════
// DirectInput — device types and structures
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DInputDeviceType {
    Mouse,
    Keyboard,
    Joystick,
    Gamepad,
    Wheel,
    Flight,
    FirstPerson,
    Device,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CooperativeLevel {
    Exclusive,
    NonExclusive,
    Foreground,
    Background,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DInputDeviceState {
    Unacquired,
    Acquired,
    Lost,
}

// ── Axes, buttons, POVs ──────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct DInputAxis {
    pub id: u32,
    pub name: String,
    pub min: i32,
    pub max: i32,
    pub deadzone: u32,
    pub saturation: u32,
    pub value: i32,
}

impl DInputAxis {
    pub fn new(id: u32, name: &str, min: i32, max: i32) -> Self {
        Self {
            id,
            name: String::from(name),
            min,
            max,
            deadzone: 0,
            saturation: 10000,
            value: 0,
        }
    }

    pub fn normalized(&self) -> f32 {
        let range = (self.max - self.min) as f32;
        if range == 0.0 {
            return 0.0;
        }
        ((self.value - self.min) as f32 / range) * 2.0 - 1.0
    }

    pub fn apply_deadzone(&self) -> i32 {
        let center = (self.min + self.max) / 2;
        let range = (self.max - self.min) as u32;
        let dz_range = (range as u64 * self.deadzone as u64 / 10000) as i32;
        let delta = self.value - center;
        if delta.abs() < dz_range / 2 {
            center
        } else {
            self.value
        }
    }
}

#[derive(Debug, Clone)]
pub struct DInputButton {
    pub id: u32,
    pub name: String,
    pub pressed: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct DInputPov {
    pub id: u32,
    pub value: u32,
}

impl DInputPov {
    pub const CENTERED: u32 = 0xFFFFFFFF;

    pub fn is_centered(&self) -> bool {
        self.value == Self::CENTERED
    }

    pub fn angle_degrees(&self) -> Option<f32> {
        if self.is_centered() {
            return None;
        }
        Some(self.value as f32 / 100.0)
    }

    pub fn direction(&self) -> PovDirection {
        if self.is_centered() {
            return PovDirection::Centered;
        }
        let angle = self.value;
        match angle {
            0..=4500 => PovDirection::Up,
            4501..=13500 => PovDirection::Right,
            13501..=22500 => PovDirection::Down,
            22501..=31500 => PovDirection::Left,
            _ => PovDirection::Up,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PovDirection {
    Centered,
    Up,
    Right,
    Down,
    Left,
}

// ── Events ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct DInputEvent {
    pub device_type: DInputDeviceType,
    pub offset: u32,
    pub data: i32,
    pub timestamp: u64,
    pub sequence: u32,
}

// ═══════════════════════════════════════════════════════════════════════════
// Force feedback
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FfEffectType {
    ConstantForce,
    RampForce,
    Square,
    Sine,
    Triangle,
    SawtoothUp,
    SawtoothDown,
    Spring,
    Damper,
    Inertia,
    Friction,
    Custom,
}

#[derive(Debug, Clone)]
pub struct FfEnvelope {
    pub attack_level: u32,
    pub attack_time: u32,
    pub fade_level: u32,
    pub fade_time: u32,
}

#[derive(Debug, Clone)]
pub enum FfTypeSpecific {
    Constant {
        magnitude: i32,
    },
    Periodic {
        magnitude: i32,
        offset: i32,
        phase: u32,
        period: u32,
    },
    Condition {
        center: i32,
        deadband: i32,
        pos_coeff: i32,
        neg_coeff: i32,
        pos_sat: u32,
        neg_sat: u32,
    },
    Ramp {
        start: i32,
        end: i32,
    },
}

#[derive(Debug, Clone)]
pub struct FfEffect {
    pub effect_type: FfEffectType,
    pub duration_ms: u32,
    pub gain: u32,
    pub direction: u32,
    pub envelope: Option<FfEnvelope>,
    pub type_specific: FfTypeSpecific,
    pub playing: bool,
    pub start_time: u64,
}

impl FfEffect {
    pub fn is_expired(&self, current_time: u64) -> bool {
        if self.duration_ms == u32::MAX {
            return false;
        }
        current_time >= self.start_time + self.duration_ms as u64
    }
}

#[derive(Debug, Clone)]
pub struct ForceFeedback {
    pub supported_effects: Vec<FfEffectType>,
    pub axes: u32,
    pub active_effects: Vec<FfEffect>,
    pub max_effects: u32,
    pub gain: u32,
    pub auto_center: bool,
}

impl ForceFeedback {
    pub fn new(axes: u32, max_effects: u32) -> Self {
        Self {
            supported_effects: Vec::from([
                FfEffectType::ConstantForce,
                FfEffectType::RampForce,
                FfEffectType::Sine,
                FfEffectType::Square,
                FfEffectType::Triangle,
                FfEffectType::Spring,
                FfEffectType::Damper,
            ]),
            axes,
            active_effects: Vec::new(),
            max_effects,
            gain: 10000,
            auto_center: true,
        }
    }

    pub fn create_effect(&mut self, effect: FfEffect) -> Result<u32, i32> {
        if self.active_effects.len() >= self.max_effects as usize {
            return Err(DI_ERR_DEVICE_FULL);
        }
        let id = self.active_effects.len() as u32;
        self.active_effects.push(effect);
        Ok(id)
    }

    pub fn start_effect(&mut self, id: u32, current_time: u64) -> i32 {
        if let Some(effect) = self.active_effects.get_mut(id as usize) {
            effect.playing = true;
            effect.start_time = current_time;
            DI_OK
        } else {
            DI_ERR_INVALID_PARAM
        }
    }

    pub fn stop_effect(&mut self, id: u32) -> i32 {
        if let Some(effect) = self.active_effects.get_mut(id as usize) {
            effect.playing = false;
            DI_OK
        } else {
            DI_ERR_INVALID_PARAM
        }
    }

    pub fn stop_all(&mut self) {
        for effect in &mut self.active_effects {
            effect.playing = false;
        }
    }

    pub fn remove_effect(&mut self, id: u32) -> i32 {
        if (id as usize) < self.active_effects.len() {
            self.active_effects.remove(id as usize);
            DI_OK
        } else {
            DI_ERR_INVALID_PARAM
        }
    }

    pub fn set_gain(&mut self, gain: u32) {
        self.gain = gain.min(10000);
    }

    pub fn set_auto_center(&mut self, enabled: bool) {
        self.auto_center = enabled;
    }

    pub fn update(&mut self, current_time: u64) {
        for effect in &mut self.active_effects {
            if effect.playing && effect.is_expired(current_time) {
                effect.playing = false;
            }
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// DirectInput device
// ═══════════════════════════════════════════════════════════════════════════

pub struct DirectInputDevice {
    pub guid: [u8; 16],
    pub device_type: DInputDeviceType,
    pub name: String,
    pub axes: Vec<DInputAxis>,
    pub buttons: Vec<DInputButton>,
    pub povs: Vec<DInputPov>,
    pub force_feedback: Option<ForceFeedback>,
    pub state: DInputDeviceState,
    pub buffer: VecDeque<DInputEvent>,
    pub cooperative_level: CooperativeLevel,
    pub buffer_size: u32,
    next_sequence: u32,
}

// DirectInput HRESULT codes
pub const DI_OK: i32 = 0;
pub const DI_BUFFEROVERFLOW: i32 = 1;
pub const DI_ERR_INVALID_PARAM: i32 = -2147024809_i32; // E_INVALIDARG
pub const DI_ERR_NOT_INITIALIZED: i32 = -2147024875_i32;
pub const DI_ERR_NOT_ACQUIRED: i32 = -2147024866_i32;
pub const DI_ERR_INPUT_LOST: i32 = -2147024884_i32;
pub const DI_ERR_DEVICE_FULL: i32 = -2147024882_i32;
pub const DI_ERR_NOT_FOUND: i32 = -2147024894_i32;

impl DirectInputDevice {
    pub fn new_gamepad(guid: [u8; 16], name: &str) -> Self {
        let axes = Vec::from([
            DInputAxis::new(0, "X Axis", -32768, 32767),
            DInputAxis::new(1, "Y Axis", -32768, 32767),
            DInputAxis::new(2, "Z Axis", -32768, 32767),
            DInputAxis::new(3, "X Rotation", -32768, 32767),
            DInputAxis::new(4, "Y Rotation", -32768, 32767),
            DInputAxis::new(5, "Z Rotation", -32768, 32767),
            DInputAxis::new(6, "Slider 0", 0, 255),
            DInputAxis::new(7, "Slider 1", 0, 255),
        ]);

        let mut buttons = Vec::new();
        for i in 0..16 {
            buttons.push(DInputButton {
                id: i,
                name: {
                    let mut s = String::from("Button ");
                    if i < 10 {
                        s.push((b'0' + i as u8) as char);
                    } else {
                        s.push('1');
                        s.push((b'0' + (i - 10) as u8) as char);
                    }
                    s
                },
                pressed: false,
            });
        }

        let povs = Vec::from([DInputPov {
            id: 0,
            value: DInputPov::CENTERED,
        }]);

        Self {
            guid,
            device_type: DInputDeviceType::Gamepad,
            name: String::from(name),
            axes,
            buttons,
            povs,
            force_feedback: Some(ForceFeedback::new(2, 16)),
            state: DInputDeviceState::Unacquired,
            buffer: VecDeque::new(),
            cooperative_level: CooperativeLevel::NonExclusive,
            buffer_size: 32,
            next_sequence: 0,
        }
    }

    pub fn new_keyboard(guid: [u8; 16]) -> Self {
        let mut buttons = Vec::new();
        for i in 0..256u32 {
            buttons.push(DInputButton {
                id: i,
                name: keyboard_key_name(i),
                pressed: false,
            });
        }

        Self {
            guid,
            device_type: DInputDeviceType::Keyboard,
            name: String::from("RaeenOS Keyboard"),
            axes: Vec::new(),
            buttons,
            povs: Vec::new(),
            force_feedback: None,
            state: DInputDeviceState::Unacquired,
            buffer: VecDeque::new(),
            cooperative_level: CooperativeLevel::NonExclusive,
            buffer_size: 64,
            next_sequence: 0,
        }
    }

    pub fn new_mouse(guid: [u8; 16]) -> Self {
        let axes = Vec::from([
            DInputAxis::new(0, "X Axis", -32768, 32767),
            DInputAxis::new(1, "Y Axis", -32768, 32767),
            DInputAxis::new(2, "Z Axis (Wheel)", -32768, 32767),
        ]);

        let buttons = Vec::from([
            DInputButton {
                id: 0,
                name: String::from("Left Button"),
                pressed: false,
            },
            DInputButton {
                id: 1,
                name: String::from("Right Button"),
                pressed: false,
            },
            DInputButton {
                id: 2,
                name: String::from("Middle Button"),
                pressed: false,
            },
            DInputButton {
                id: 3,
                name: String::from("Button 4"),
                pressed: false,
            },
            DInputButton {
                id: 4,
                name: String::from("Button 5"),
                pressed: false,
            },
        ]);

        Self {
            guid,
            device_type: DInputDeviceType::Mouse,
            name: String::from("RaeenOS Mouse"),
            axes,
            buttons,
            povs: Vec::new(),
            force_feedback: None,
            state: DInputDeviceState::Unacquired,
            buffer: VecDeque::new(),
            cooperative_level: CooperativeLevel::NonExclusive,
            buffer_size: 64,
            next_sequence: 0,
        }
    }

    pub fn acquire(&mut self) -> i32 {
        match self.state {
            DInputDeviceState::Acquired => DI_OK,
            DInputDeviceState::Unacquired | DInputDeviceState::Lost => {
                self.state = DInputDeviceState::Acquired;
                self.buffer.clear();
                DI_OK
            }
        }
    }

    pub fn unacquire(&mut self) -> i32 {
        self.state = DInputDeviceState::Unacquired;
        DI_OK
    }

    pub fn set_cooperative_level(&mut self, level: CooperativeLevel) -> i32 {
        if self.state == DInputDeviceState::Acquired {
            return DI_ERR_NOT_INITIALIZED;
        }
        self.cooperative_level = level;
        DI_OK
    }

    pub fn set_buffer_size(&mut self, size: u32) -> i32 {
        if self.state == DInputDeviceState::Acquired {
            return DI_ERR_NOT_INITIALIZED;
        }
        self.buffer_size = size;
        DI_OK
    }

    pub fn push_event(&mut self, offset: u32, data: i32, timestamp: u64) -> i32 {
        if self.state != DInputDeviceState::Acquired {
            return DI_ERR_NOT_ACQUIRED;
        }

        let seq = self.next_sequence;
        self.next_sequence = self.next_sequence.wrapping_add(1);

        let event = DInputEvent {
            device_type: self.device_type,
            offset,
            data,
            timestamp,
            sequence: seq,
        };

        if self.buffer.len() >= self.buffer_size as usize {
            self.buffer.pop_front();
        }
        self.buffer.push_back(event);
        DI_OK
    }

    pub fn get_buffered_data(&mut self) -> Vec<DInputEvent> {
        let events: Vec<DInputEvent> = self.buffer.drain(..).collect();
        events
    }

    pub fn poll(&mut self) -> i32 {
        match self.state {
            DInputDeviceState::Acquired => DI_OK,
            DInputDeviceState::Lost => DI_ERR_INPUT_LOST,
            DInputDeviceState::Unacquired => DI_ERR_NOT_ACQUIRED,
        }
    }

    pub fn set_axis_value(&mut self, axis_id: u32, value: i32, timestamp: u64) -> i32 {
        if let Some(axis) = self.axes.iter_mut().find(|a| a.id == axis_id) {
            let clamped = value.max(axis.min).min(axis.max);
            axis.value = clamped;
            self.push_event(axis_id, clamped, timestamp)
        } else {
            DI_ERR_INVALID_PARAM
        }
    }

    pub fn set_button_state(&mut self, button_id: u32, pressed: bool, timestamp: u64) -> i32 {
        if let Some(button) = self.buttons.iter_mut().find(|b| b.id == button_id) {
            button.pressed = pressed;
            let data = if pressed { 0x80 } else { 0x00 };
            let offset = 0x30 + button_id;
            self.push_event(offset, data, timestamp)
        } else {
            DI_ERR_INVALID_PARAM
        }
    }

    pub fn set_pov_value(&mut self, pov_id: u32, value: u32, timestamp: u64) -> i32 {
        if let Some(pov) = self.povs.iter_mut().find(|p| p.id == pov_id) {
            pov.value = value;
            let offset = 0x20 + pov_id;
            self.push_event(offset, value as i32, timestamp)
        } else {
            DI_ERR_INVALID_PARAM
        }
    }

    pub fn get_device_state_gamepad(&self) -> Result<DInputGamepadState, i32> {
        if self.state != DInputDeviceState::Acquired {
            return Err(DI_ERR_NOT_ACQUIRED);
        }
        Ok(DInputGamepadState {
            x: self.axes.get(0).map(|a| a.value).unwrap_or(0),
            y: self.axes.get(1).map(|a| a.value).unwrap_or(0),
            z: self.axes.get(2).map(|a| a.value).unwrap_or(0),
            rx: self.axes.get(3).map(|a| a.value).unwrap_or(0),
            ry: self.axes.get(4).map(|a| a.value).unwrap_or(0),
            rz: self.axes.get(5).map(|a| a.value).unwrap_or(0),
            slider: [
                self.axes.get(6).map(|a| a.value).unwrap_or(0),
                self.axes.get(7).map(|a| a.value).unwrap_or(0),
            ],
            pov: [self
                .povs
                .get(0)
                .map(|p| p.value)
                .unwrap_or(DInputPov::CENTERED)],
            buttons: {
                let mut b = [false; 16];
                for (i, btn) in self.buttons.iter().enumerate().take(16) {
                    b[i] = btn.pressed;
                }
                b
            },
        })
    }

    pub fn get_capabilities(&self) -> DInputDeviceCaps {
        DInputDeviceCaps {
            device_type: self.device_type,
            axes_count: self.axes.len() as u32,
            button_count: self.buttons.len() as u32,
            pov_count: self.povs.len() as u32,
            has_force_feedback: self.force_feedback.is_some(),
            ff_axes: self.force_feedback.as_ref().map(|ff| ff.axes).unwrap_or(0),
        }
    }
}

#[derive(Debug, Clone)]
pub struct DInputGamepadState {
    pub x: i32,
    pub y: i32,
    pub z: i32,
    pub rx: i32,
    pub ry: i32,
    pub rz: i32,
    pub slider: [i32; 2],
    pub pov: [u32; 1],
    pub buttons: [bool; 16],
}

#[derive(Debug, Clone)]
pub struct DInputDeviceCaps {
    pub device_type: DInputDeviceType,
    pub axes_count: u32,
    pub button_count: u32,
    pub pov_count: u32,
    pub has_force_feedback: bool,
    pub ff_axes: u32,
}

fn keyboard_key_name(scancode: u32) -> String {
    match scancode {
        0x01 => String::from("Escape"),
        0x02 => String::from("1"),
        0x03 => String::from("2"),
        0x04 => String::from("3"),
        0x05 => String::from("4"),
        0x06 => String::from("5"),
        0x07 => String::from("6"),
        0x08 => String::from("7"),
        0x09 => String::from("8"),
        0x0A => String::from("9"),
        0x0B => String::from("0"),
        0x0E => String::from("Backspace"),
        0x0F => String::from("Tab"),
        0x1C => String::from("Enter"),
        0x1D => String::from("Left Ctrl"),
        0x2A => String::from("Left Shift"),
        0x36 => String::from("Right Shift"),
        0x38 => String::from("Left Alt"),
        0x39 => String::from("Space"),
        0x3A => String::from("Caps Lock"),
        0x3B..=0x44 => {
            let mut s = String::from("F");
            let num = scancode - 0x3A;
            if num < 10 {
                s.push((b'0' + num as u8) as char);
            } else {
                s.push('1');
                s.push((b'0' + (num - 10) as u8) as char);
            }
            s
        }
        0xC8 => String::from("Up"),
        0xCB => String::from("Left"),
        0xCD => String::from("Right"),
        0xD0 => String::from("Down"),
        _ => {
            let mut s = String::from("Key_0x");
            let hi = (scancode >> 4) & 0xF;
            let lo = scancode & 0xF;
            fn hex_char(v: u32) -> char {
                if v < 10 {
                    (b'0' + v as u8) as char
                } else {
                    (b'A' + (v - 10) as u8) as char
                }
            }
            s.push(hex_char(hi));
            s.push(hex_char(lo));
            s
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// DirectInput context (IDirectInput8)
// ═══════════════════════════════════════════════════════════════════════════

pub struct DirectInput {
    pub devices: Vec<DirectInputDevice>,
    pub version: u32,
}

impl DirectInput {
    pub fn create(version: u32) -> Self {
        Self {
            devices: Vec::new(),
            version,
        }
    }

    pub fn enumerate_devices(
        &self,
        device_type: Option<DInputDeviceType>,
    ) -> Vec<DInputDeviceInfo> {
        self.devices
            .iter()
            .filter(|d| device_type.is_none() || device_type == Some(d.device_type))
            .map(|d| DInputDeviceInfo {
                guid: d.guid,
                device_type: d.device_type,
                name: d.name.clone(),
            })
            .collect()
    }

    pub fn create_device(&mut self, guid: [u8; 16]) -> Result<usize, i32> {
        if let Some(idx) = self.devices.iter().position(|d| d.guid == guid) {
            Ok(idx)
        } else {
            Err(DI_ERR_NOT_FOUND)
        }
    }

    pub fn add_device(&mut self, device: DirectInputDevice) {
        self.devices.push(device);
    }

    pub fn get_device(&self, index: usize) -> Option<&DirectInputDevice> {
        self.devices.get(index)
    }

    pub fn get_device_mut(&mut self, index: usize) -> Option<&mut DirectInputDevice> {
        self.devices.get_mut(index)
    }
}

#[derive(Debug, Clone)]
pub struct DInputDeviceInfo {
    pub guid: [u8; 16],
    pub device_type: DInputDeviceType,
    pub name: String,
}

// ═══════════════════════════════════════════════════════════════════════════
// XInput — Xbox controller API
// ═══════════════════════════════════════════════════════════════════════════

pub const XINPUT_MAX_CONTROLLERS: u32 = 4;

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
pub const XINPUT_GAMEPAD_A: u16 = 0x1000;
pub const XINPUT_GAMEPAD_B: u16 = 0x2000;
pub const XINPUT_GAMEPAD_X: u16 = 0x4000;
pub const XINPUT_GAMEPAD_Y: u16 = 0x8000;

pub const XINPUT_GAMEPAD_LEFT_THUMB_DEADZONE: i16 = 7849;
pub const XINPUT_GAMEPAD_RIGHT_THUMB_DEADZONE: i16 = 8689;
pub const XINPUT_GAMEPAD_TRIGGER_THRESHOLD: u8 = 30;

pub const ERROR_SUCCESS: u32 = 0;
pub const ERROR_DEVICE_NOT_CONNECTED: u32 = 1167;

#[derive(Debug, Clone, Copy)]
pub struct XInputGamepad {
    pub buttons: u16,
    pub left_trigger: u8,
    pub right_trigger: u8,
    pub thumb_lx: i16,
    pub thumb_ly: i16,
    pub thumb_rx: i16,
    pub thumb_ry: i16,
}

impl Default for XInputGamepad {
    fn default() -> Self {
        Self {
            buttons: 0,
            left_trigger: 0,
            right_trigger: 0,
            thumb_lx: 0,
            thumb_ly: 0,
            thumb_rx: 0,
            thumb_ry: 0,
        }
    }
}

impl XInputGamepad {
    pub fn is_button_pressed(&self, button: u16) -> bool {
        self.buttons & button != 0
    }

    pub fn left_stick_normalized(&self) -> (f32, f32) {
        (
            self.thumb_lx as f32 / 32767.0,
            self.thumb_ly as f32 / 32767.0,
        )
    }

    pub fn right_stick_normalized(&self) -> (f32, f32) {
        (
            self.thumb_rx as f32 / 32767.0,
            self.thumb_ry as f32 / 32767.0,
        )
    }

    pub fn left_trigger_normalized(&self) -> f32 {
        self.left_trigger as f32 / 255.0
    }

    pub fn right_trigger_normalized(&self) -> f32 {
        self.right_trigger as f32 / 255.0
    }

    pub fn apply_left_stick_deadzone(&self) -> (i16, i16) {
        apply_stick_deadzone(
            self.thumb_lx,
            self.thumb_ly,
            XINPUT_GAMEPAD_LEFT_THUMB_DEADZONE,
        )
    }

    pub fn apply_right_stick_deadzone(&self) -> (i16, i16) {
        apply_stick_deadzone(
            self.thumb_rx,
            self.thumb_ry,
            XINPUT_GAMEPAD_RIGHT_THUMB_DEADZONE,
        )
    }
}

fn apply_stick_deadzone(x: i16, y: i16, deadzone: i16) -> (i16, i16) {
    let fx = x as f32;
    let fy = y as f32;
    let magnitude = libm::sqrtf(fx * fx + fy * fy);
    if magnitude < deadzone as f32 {
        (0, 0)
    } else {
        (x, y)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct XInputState {
    pub packet_number: u32,
    pub gamepad: XInputGamepad,
}

#[derive(Debug, Clone, Copy)]
pub struct XInputVibration {
    pub left_motor_speed: u16,
    pub right_motor_speed: u16,
}

#[derive(Debug, Clone, Copy)]
pub struct XInputCapabilities {
    pub controller_type: u8,
    pub sub_type: u8,
    pub flags: u16,
    pub gamepad: XInputGamepad,
    pub vibration: XInputVibration,
}

pub const XINPUT_DEVTYPE_GAMEPAD: u8 = 0x01;
pub const XINPUT_DEVSUBTYPE_GAMEPAD: u8 = 0x01;
pub const XINPUT_DEVSUBTYPE_WHEEL: u8 = 0x02;
pub const XINPUT_DEVSUBTYPE_ARCADE_STICK: u8 = 0x03;
pub const XINPUT_DEVSUBTYPE_FLIGHT_STICK: u8 = 0x04;
pub const XINPUT_DEVSUBTYPE_DANCE_PAD: u8 = 0x05;
pub const XINPUT_DEVSUBTYPE_GUITAR: u8 = 0x06;
pub const XINPUT_DEVSUBTYPE_DRUM_KIT: u8 = 0x08;

#[derive(Debug, Clone, Copy)]
pub struct XInputBatteryInfo {
    pub battery_type: u8,
    pub battery_level: u8,
}

pub const BATTERY_TYPE_DISCONNECTED: u8 = 0x00;
pub const BATTERY_TYPE_WIRED: u8 = 0x01;
pub const BATTERY_TYPE_ALKALINE: u8 = 0x02;
pub const BATTERY_TYPE_NIMH: u8 = 0x03;
pub const BATTERY_TYPE_UNKNOWN: u8 = 0xFF;
pub const BATTERY_LEVEL_EMPTY: u8 = 0x00;
pub const BATTERY_LEVEL_LOW: u8 = 0x01;
pub const BATTERY_LEVEL_MEDIUM: u8 = 0x02;
pub const BATTERY_LEVEL_FULL: u8 = 0x03;

// ── XInput controller state ──────────────────────────────────────────────

pub struct XInputController {
    pub connected: bool,
    pub state: XInputState,
    pub vibration: XInputVibration,
    pub capabilities: XInputCapabilities,
    pub battery: XInputBatteryInfo,
}

impl Default for XInputController {
    fn default() -> Self {
        Self {
            connected: false,
            state: XInputState {
                packet_number: 0,
                gamepad: XInputGamepad::default(),
            },
            vibration: XInputVibration {
                left_motor_speed: 0,
                right_motor_speed: 0,
            },
            capabilities: XInputCapabilities {
                controller_type: XINPUT_DEVTYPE_GAMEPAD,
                sub_type: XINPUT_DEVSUBTYPE_GAMEPAD,
                flags: 0,
                gamepad: XInputGamepad::default(),
                vibration: XInputVibration {
                    left_motor_speed: 0,
                    right_motor_speed: 0,
                },
            },
            battery: XInputBatteryInfo {
                battery_type: BATTERY_TYPE_DISCONNECTED,
                battery_level: BATTERY_LEVEL_EMPTY,
            },
        }
    }
}

pub struct XInputSystem {
    pub controllers: [XInputController; 4],
}

impl XInputSystem {
    pub fn new() -> Self {
        Self {
            controllers: [
                XInputController::default(),
                XInputController::default(),
                XInputController::default(),
                XInputController::default(),
            ],
        }
    }

    pub fn connect_controller(&mut self, index: u32) {
        if let Some(ctrl) = self.controllers.get_mut(index as usize) {
            ctrl.connected = true;
            ctrl.battery.battery_type = BATTERY_TYPE_WIRED;
            ctrl.battery.battery_level = BATTERY_LEVEL_FULL;
        }
    }

    pub fn disconnect_controller(&mut self, index: u32) {
        if let Some(ctrl) = self.controllers.get_mut(index as usize) {
            ctrl.connected = false;
            ctrl.battery.battery_type = BATTERY_TYPE_DISCONNECTED;
            ctrl.battery.battery_level = BATTERY_LEVEL_EMPTY;
        }
    }

    pub fn update_state(&mut self, index: u32, gamepad: XInputGamepad) {
        if let Some(ctrl) = self.controllers.get_mut(index as usize) {
            if ctrl.connected {
                ctrl.state.packet_number = ctrl.state.packet_number.wrapping_add(1);
                ctrl.state.gamepad = gamepad;
            }
        }
    }
}

pub fn xinput_get_state(system: &XInputSystem, user_index: u32) -> Result<XInputState, u32> {
    if user_index >= XINPUT_MAX_CONTROLLERS {
        return Err(ERROR_DEVICE_NOT_CONNECTED);
    }
    let ctrl = &system.controllers[user_index as usize];
    if !ctrl.connected {
        return Err(ERROR_DEVICE_NOT_CONNECTED);
    }
    Ok(ctrl.state)
}

pub fn xinput_set_state(
    system: &mut XInputSystem,
    user_index: u32,
    vibration: &XInputVibration,
) -> u32 {
    if user_index >= XINPUT_MAX_CONTROLLERS {
        return ERROR_DEVICE_NOT_CONNECTED;
    }
    let ctrl = &mut system.controllers[user_index as usize];
    if !ctrl.connected {
        return ERROR_DEVICE_NOT_CONNECTED;
    }
    ctrl.vibration = *vibration;
    ERROR_SUCCESS
}

pub fn xinput_get_capabilities(
    system: &XInputSystem,
    user_index: u32,
) -> Result<XInputCapabilities, u32> {
    if user_index >= XINPUT_MAX_CONTROLLERS {
        return Err(ERROR_DEVICE_NOT_CONNECTED);
    }
    let ctrl = &system.controllers[user_index as usize];
    if !ctrl.connected {
        return Err(ERROR_DEVICE_NOT_CONNECTED);
    }
    Ok(ctrl.capabilities)
}

pub fn xinput_get_battery_info(
    system: &XInputSystem,
    user_index: u32,
) -> Result<XInputBatteryInfo, u32> {
    if user_index >= XINPUT_MAX_CONTROLLERS {
        return Err(ERROR_DEVICE_NOT_CONNECTED);
    }
    let ctrl = &system.controllers[user_index as usize];
    if !ctrl.connected {
        return Err(ERROR_DEVICE_NOT_CONNECTED);
    }
    Ok(ctrl.battery)
}

pub fn xinput_enable(system: &mut XInputSystem, enable: bool) {
    if !enable {
        for ctrl in &mut system.controllers {
            ctrl.vibration = XInputVibration {
                left_motor_speed: 0,
                right_motor_speed: 0,
            };
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// DirectSound
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DsBufferType {
    Primary,
    Secondary,
    Hardware,
    Software,
}

#[derive(Debug, Clone, Copy)]
pub struct WaveFormat {
    pub format_tag: u16,
    pub channels: u16,
    pub samples_per_sec: u32,
    pub avg_bytes_per_sec: u32,
    pub block_align: u16,
    pub bits_per_sample: u16,
}

pub const WAVE_FORMAT_PCM: u16 = 0x0001;
pub const WAVE_FORMAT_IEEE_FLOAT: u16 = 0x0003;
pub const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;

impl WaveFormat {
    pub fn pcm(channels: u16, samples_per_sec: u32, bits_per_sample: u16) -> Self {
        let block_align = channels * bits_per_sample / 8;
        Self {
            format_tag: WAVE_FORMAT_PCM,
            channels,
            samples_per_sec,
            avg_bytes_per_sec: samples_per_sec * block_align as u32,
            block_align,
            bits_per_sample,
        }
    }

    pub fn cd_quality() -> Self {
        Self::pcm(2, 44100, 16)
    }
    pub fn dvd_quality() -> Self {
        Self::pcm(2, 48000, 16)
    }
    pub fn mono_8bit() -> Self {
        Self::pcm(1, 22050, 8)
    }

    pub fn frame_size(&self) -> usize {
        self.block_align as usize
    }

    pub fn duration_ms(&self, byte_count: usize) -> u64 {
        if self.avg_bytes_per_sec == 0 {
            return 0;
        }
        (byte_count as u64 * 1000) / self.avg_bytes_per_sec as u64
    }
}

// ── Buffer descriptor ────────────────────────────────────────────────────

pub const DSBCAPS_PRIMARYBUFFER: u32 = 0x00000001;
pub const DSBCAPS_STATIC: u32 = 0x00000002;
pub const DSBCAPS_LOCHARDWARE: u32 = 0x00000004;
pub const DSBCAPS_LOCSOFTWARE: u32 = 0x00000008;
pub const DSBCAPS_CTRL3D: u32 = 0x00000010;
pub const DSBCAPS_CTRLFREQUENCY: u32 = 0x00000020;
pub const DSBCAPS_CTRLPAN: u32 = 0x00000040;
pub const DSBCAPS_CTRLVOLUME: u32 = 0x00000080;
pub const DSBCAPS_CTRLPOSITIONNOTIFY: u32 = 0x00000100;
pub const DSBCAPS_GETCURRENTPOSITION2: u32 = 0x00010000;
pub const DSBCAPS_GLOBALFOCUS: u32 = 0x00008000;

pub const DSBPLAY_LOOPING: u32 = 0x00000001;

pub const DSBSTATUS_PLAYING: u32 = 0x00000001;
pub const DSBSTATUS_BUFFERLOST: u32 = 0x00000002;
pub const DSBSTATUS_LOOPING: u32 = 0x00000004;

pub const DSBVOLUME_MIN: i32 = -10000;
pub const DSBVOLUME_MAX: i32 = 0;
pub const DSBPAN_LEFT: i32 = -10000;
pub const DSBPAN_CENTER: i32 = 0;
pub const DSBPAN_RIGHT: i32 = 10000;
pub const DSBFREQUENCY_MIN: u32 = 100;
pub const DSBFREQUENCY_MAX: u32 = 200000;

pub const DS_OK: i32 = 0;
pub const DSERR_INVALIDPARAM: i32 = -2147024809_i32;
pub const DSERR_BUFFERLOST: i32 = -2005401450;
pub const DSERR_INVALIDCALL: i32 = -2005401440;
pub const DSERR_OUTOFMEMORY: i32 = -2147024882_i32;

#[derive(Debug, Clone)]
pub struct DsBufferDesc {
    pub flags: u32,
    pub buffer_bytes: u32,
    pub format: Option<WaveFormat>,
}

pub struct DsLockResult {
    pub ptr1_offset: u32,
    pub ptr1_size: u32,
    pub ptr2_offset: u32,
    pub ptr2_size: u32,
}

pub struct DirectSoundBuffer {
    pub id: u64,
    pub format: WaveFormat,
    pub data: Vec<u8>,
    pub position: u64,
    pub write_position: u64,
    pub playing: bool,
    pub looping: bool,
    pub volume: i32,
    pub pan: i32,
    pub frequency: u32,
    pub buffer_type: DsBufferType,
    pub flags: u32,
    pub locked: bool,
}

impl DirectSoundBuffer {
    pub fn volume_linear(&self) -> f32 {
        if self.volume <= DSBVOLUME_MIN {
            return 0.0;
        }
        if self.volume >= DSBVOLUME_MAX {
            return 1.0;
        }
        libm::powf(10.0, self.volume as f32 / 2000.0)
    }

    pub fn pan_left_right(&self) -> (f32, f32) {
        if self.pan == DSBPAN_CENTER {
            (1.0, 1.0)
        } else if self.pan < DSBPAN_CENTER {
            let right = libm::powf(10.0, self.pan as f32 / 2000.0);
            (1.0, right)
        } else {
            let left = libm::powf(10.0, -self.pan as f32 / 2000.0);
            (left, 1.0)
        }
    }

    pub fn status(&self) -> u32 {
        let mut s = 0u32;
        if self.playing {
            s |= DSBSTATUS_PLAYING;
        }
        if self.looping {
            s |= DSBSTATUS_LOOPING;
        }
        s
    }

    pub fn duration_ms(&self) -> u64 {
        self.format.duration_ms(self.data.len())
    }
}

// ── DirectSound context ──────────────────────────────────────────────────

pub struct DirectSound {
    pub primary_buffer: Option<DirectSoundBuffer>,
    pub secondary_buffers: Vec<DirectSoundBuffer>,
    pub device: String,
    pub speaker_config: u32,
    next_buffer_id: u64,
}

pub const DSSPEAKER_HEADPHONE: u32 = 0x00000001;
pub const DSSPEAKER_MONO: u32 = 0x00000002;
pub const DSSPEAKER_QUAD: u32 = 0x00000003;
pub const DSSPEAKER_STEREO: u32 = 0x00000004;
pub const DSSPEAKER_SURROUND: u32 = 0x00000005;
pub const DSSPEAKER_5POINT1: u32 = 0x00000006;
pub const DSSPEAKER_7POINT1: u32 = 0x00000007;

impl DirectSound {
    pub fn create(device: &str) -> Self {
        Self {
            primary_buffer: None,
            secondary_buffers: Vec::new(),
            device: String::from(device),
            speaker_config: DSSPEAKER_STEREO,
            next_buffer_id: 1,
        }
    }

    pub fn set_cooperative_level(&mut self, _hwnd: u64, _level: u32) -> i32 {
        DS_OK
    }

    fn alloc_id(&mut self) -> u64 {
        let id = self.next_buffer_id;
        self.next_buffer_id += 1;
        id
    }
}

pub fn ds_create_sound_buffer(ds: &mut DirectSound, desc: &DsBufferDesc) -> Result<u64, i32> {
    let format = desc.format.unwrap_or_else(WaveFormat::cd_quality);

    let buffer_type = if desc.flags & DSBCAPS_PRIMARYBUFFER != 0 {
        DsBufferType::Primary
    } else if desc.flags & DSBCAPS_LOCHARDWARE != 0 {
        DsBufferType::Hardware
    } else {
        DsBufferType::Software
    };

    let id = ds.alloc_id();
    let mut data = Vec::new();
    data.resize(desc.buffer_bytes as usize, 0u8);

    let buffer = DirectSoundBuffer {
        id,
        format,
        data,
        position: 0,
        write_position: 0,
        playing: false,
        looping: false,
        volume: DSBVOLUME_MAX,
        pan: DSBPAN_CENTER,
        frequency: format.samples_per_sec,
        buffer_type,
        flags: desc.flags,
        locked: false,
    };

    if buffer_type == DsBufferType::Primary {
        ds.primary_buffer = Some(buffer);
    } else {
        ds.secondary_buffers.push(buffer);
    }

    Ok(id)
}

fn find_buffer_mut<'a>(
    ds: &'a mut DirectSound,
    buffer_id: u64,
) -> Option<&'a mut DirectSoundBuffer> {
    if let Some(ref mut primary) = ds.primary_buffer {
        if primary.id == buffer_id {
            return Some(primary);
        }
    }
    ds.secondary_buffers.iter_mut().find(|b| b.id == buffer_id)
}

fn find_buffer<'a>(ds: &'a DirectSound, buffer_id: u64) -> Option<&'a DirectSoundBuffer> {
    if let Some(ref primary) = ds.primary_buffer {
        if primary.id == buffer_id {
            return Some(primary);
        }
    }
    ds.secondary_buffers.iter().find(|b| b.id == buffer_id)
}

pub fn ds_play(ds: &mut DirectSound, buffer_id: u64, flags: u32) -> i32 {
    if let Some(buf) = find_buffer_mut(ds, buffer_id) {
        buf.playing = true;
        buf.looping = flags & DSBPLAY_LOOPING != 0;
        DS_OK
    } else {
        DSERR_INVALIDPARAM
    }
}

pub fn ds_stop(ds: &mut DirectSound, buffer_id: u64) -> i32 {
    if let Some(buf) = find_buffer_mut(ds, buffer_id) {
        buf.playing = false;
        buf.looping = false;
        DS_OK
    } else {
        DSERR_INVALIDPARAM
    }
}

pub fn ds_set_volume(ds: &mut DirectSound, buffer_id: u64, volume: i32) -> i32 {
    if volume < DSBVOLUME_MIN || volume > DSBVOLUME_MAX {
        return DSERR_INVALIDPARAM;
    }
    if let Some(buf) = find_buffer_mut(ds, buffer_id) {
        if buf.flags & DSBCAPS_CTRLVOLUME == 0 {
            return DSERR_INVALIDCALL;
        }
        buf.volume = volume;
        DS_OK
    } else {
        DSERR_INVALIDPARAM
    }
}

pub fn ds_set_pan(ds: &mut DirectSound, buffer_id: u64, pan: i32) -> i32 {
    if pan < DSBPAN_LEFT || pan > DSBPAN_RIGHT {
        return DSERR_INVALIDPARAM;
    }
    if let Some(buf) = find_buffer_mut(ds, buffer_id) {
        if buf.flags & DSBCAPS_CTRLPAN == 0 {
            return DSERR_INVALIDCALL;
        }
        buf.pan = pan;
        DS_OK
    } else {
        DSERR_INVALIDPARAM
    }
}

pub fn ds_set_frequency(ds: &mut DirectSound, buffer_id: u64, frequency: u32) -> i32 {
    if frequency != 0 && (frequency < DSBFREQUENCY_MIN || frequency > DSBFREQUENCY_MAX) {
        return DSERR_INVALIDPARAM;
    }
    if let Some(buf) = find_buffer_mut(ds, buffer_id) {
        if buf.flags & DSBCAPS_CTRLFREQUENCY == 0 {
            return DSERR_INVALIDCALL;
        }
        buf.frequency = if frequency == 0 {
            buf.format.samples_per_sec
        } else {
            frequency
        };
        DS_OK
    } else {
        DSERR_INVALIDPARAM
    }
}

pub fn ds_lock(
    ds: &mut DirectSound,
    buffer_id: u64,
    offset: u32,
    size: u32,
) -> Result<DsLockResult, i32> {
    if let Some(buf) = find_buffer_mut(ds, buffer_id) {
        if buf.locked {
            return Err(DSERR_INVALIDCALL);
        }
        let buf_len = buf.data.len() as u32;
        if buf_len == 0 {
            return Err(DSERR_INVALIDPARAM);
        }
        let actual_offset = offset % buf_len;
        let end = actual_offset + size;

        buf.locked = true;

        if end <= buf_len {
            Ok(DsLockResult {
                ptr1_offset: actual_offset,
                ptr1_size: size,
                ptr2_offset: 0,
                ptr2_size: 0,
            })
        } else {
            let first_part = buf_len - actual_offset;
            let second_part = size - first_part;
            Ok(DsLockResult {
                ptr1_offset: actual_offset,
                ptr1_size: first_part,
                ptr2_offset: 0,
                ptr2_size: second_part.min(buf_len),
            })
        }
    } else {
        Err(DSERR_INVALIDPARAM)
    }
}

pub fn ds_unlock(ds: &mut DirectSound, buffer_id: u64) -> i32 {
    if let Some(buf) = find_buffer_mut(ds, buffer_id) {
        buf.locked = false;
        DS_OK
    } else {
        DSERR_INVALIDPARAM
    }
}

pub fn ds_get_status(ds: &DirectSound, buffer_id: u64) -> u32 {
    find_buffer(ds, buffer_id).map(|b| b.status()).unwrap_or(0)
}

pub fn ds_set_current_position(ds: &mut DirectSound, buffer_id: u64, position: u32) -> i32 {
    if let Some(buf) = find_buffer_mut(ds, buffer_id) {
        let buf_len = buf.data.len() as u64;
        if buf_len == 0 {
            return DSERR_INVALIDPARAM;
        }
        buf.position = position as u64 % buf_len;
        DS_OK
    } else {
        DSERR_INVALIDPARAM
    }
}

pub fn ds_get_current_position(ds: &DirectSound, buffer_id: u64) -> Result<(u32, u32), i32> {
    if let Some(buf) = find_buffer(ds, buffer_id) {
        Ok((buf.position as u32, buf.write_position as u32))
    } else {
        Err(DSERR_INVALIDPARAM)
    }
}

pub fn ds_get_format(ds: &DirectSound, buffer_id: u64) -> Result<WaveFormat, i32> {
    if let Some(buf) = find_buffer(ds, buffer_id) {
        Ok(buf.format)
    } else {
        Err(DSERR_INVALIDPARAM)
    }
}

pub fn ds_destroy_buffer(ds: &mut DirectSound, buffer_id: u64) {
    if let Some(ref primary) = ds.primary_buffer {
        if primary.id == buffer_id {
            ds.primary_buffer = None;
            return;
        }
    }
    ds.secondary_buffers.retain(|b| b.id != buffer_id);
}
