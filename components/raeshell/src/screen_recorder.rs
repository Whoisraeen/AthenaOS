//! Full-featured screen recorder for the RaeenOS desktop shell.
//!
//! Supports recording full screen, windows, or regions with configurable
//! video/audio settings, webcam overlay, cursor effects, live annotation,
//! post-processing (trim, split, speed), encoding presets, streaming
//! output, and hotkey control.

#![allow(unused)]

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

// ── Constants ────────────────────────────────────────────────────────────

const REC_DOT_COLOR: u32 = 0xFF_FF_22_22;
const REC_BG: u32 = 0xCC_0A_0E_1A;
const TIMER_FG: u32 = 0xFF_F0_F0_F8;
const COUNTDOWN_FG: u32 = 0xFF_FF_FF_FF;
const COUNTDOWN_BG: u32 = 0xCC_00_00_00;
const OVERLAY_BORDER: u32 = 0xFF_4E_9C_FF;
const ANNOTATION_DEFAULT: u32 = 0xFF_FF_44_44;
const SPOTLIGHT_DIM: u32 = 0xAA_00_00_00;

// ── Capture mode ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordCaptureMode {
    FullScreen,
    SpecificWindow { surface_id: u64 },
    RectangularRegion,
    SpecificMonitor { index: u8 },
}

// ── Video settings ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolution {
    Native,
    Res2160p,
    Res1440p,
    Res1080p,
    Res720p,
    Custom { width: u32, height: u32 },
}

impl Resolution {
    pub fn dimensions(&self, native_w: u32, native_h: u32) -> (u32, u32) {
        match self {
            Resolution::Native => (native_w, native_h),
            Resolution::Res2160p => (3840, 2160),
            Resolution::Res1440p => (2560, 1440),
            Resolution::Res1080p => (1920, 1080),
            Resolution::Res720p => (1280, 720),
            Resolution::Custom { width, height } => (*width, *height),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameRate {
    Fps15,
    Fps24,
    Fps30,
    Fps60,
    Custom(u16),
}

impl FrameRate {
    pub fn value(&self) -> u16 {
        match self {
            FrameRate::Fps15 => 15,
            FrameRate::Fps24 => 24,
            FrameRate::Fps30 => 30,
            FrameRate::Fps60 => 60,
            FrameRate::Custom(v) => *v,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BitratePreset {
    Low,
    Medium,
    High,
    Custom(u32),
}

impl BitratePreset {
    pub fn kbps(&self) -> u32 {
        match self {
            BitratePreset::Low => 2500,
            BitratePreset::Medium => 6000,
            BitratePreset::High => 15000,
            BitratePreset::Custom(k) => *k,
        }
    }
}

#[derive(Debug, Clone)]
pub struct VideoSettings {
    pub resolution: Resolution,
    pub frame_rate: FrameRate,
    pub bitrate: BitratePreset,
    pub native_width: u32,
    pub native_height: u32,
}

impl VideoSettings {
    pub fn new(native_w: u32, native_h: u32) -> Self {
        Self {
            resolution: Resolution::Native,
            frame_rate: FrameRate::Fps30,
            bitrate: BitratePreset::Medium,
            native_width: native_w,
            native_height: native_h,
        }
    }

    pub fn output_dimensions(&self) -> (u32, u32) {
        self.resolution
            .dimensions(self.native_width, self.native_height)
    }
}

// ── Audio capture ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioSource {
    None,
    SystemAudio,
    Microphone,
    Both,
}

#[derive(Debug, Clone)]
pub struct AudioDevice {
    pub id: u32,
    pub name: String,
    pub is_input: bool,
    pub sample_rate: u32,
    pub channels: u8,
}

#[derive(Debug, Clone)]
pub struct AudioSettings {
    pub source: AudioSource,
    pub system_device_id: Option<u32>,
    pub mic_device_id: Option<u32>,
    pub system_volume: u8,
    pub mic_volume: u8,
    pub noise_suppression: bool,
    pub available_devices: Vec<AudioDevice>,
}

impl AudioSettings {
    pub fn new() -> Self {
        Self {
            source: AudioSource::Both,
            system_device_id: None,
            mic_device_id: None,
            system_volume: 100,
            mic_volume: 80,
            noise_suppression: true,
            available_devices: Vec::new(),
        }
    }

    pub fn set_source(&mut self, src: AudioSource) {
        self.source = src;
    }

    pub fn set_system_volume(&mut self, vol: u8) {
        self.system_volume = vol.min(100);
    }

    pub fn set_mic_volume(&mut self, vol: u8) {
        self.mic_volume = vol.min(100);
    }

    pub fn add_device(
        &mut self,
        id: u32,
        name: &str,
        is_input: bool,
        sample_rate: u32,
        channels: u8,
    ) {
        self.available_devices.push(AudioDevice {
            id,
            name: String::from(name),
            is_input,
            sample_rate,
            channels,
        });
    }

    pub fn select_system_device(&mut self, id: u32) {
        if self
            .available_devices
            .iter()
            .any(|d| d.id == id && !d.is_input)
        {
            self.system_device_id = Some(id);
        }
    }

    pub fn select_mic_device(&mut self, id: u32) {
        if self
            .available_devices
            .iter()
            .any(|d| d.id == id && d.is_input)
        {
            self.mic_device_id = Some(id);
        }
    }

    pub fn is_recording_audio(&self) -> bool {
        self.source != AudioSource::None
    }
}

// ── Webcam overlay ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PipPosition {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PipSize {
    Small,
    Medium,
    Large,
    Custom { width: u32, height: u32 },
}

impl PipSize {
    pub fn dimensions(&self) -> (u32, u32) {
        match self {
            PipSize::Small => (160, 120),
            PipSize::Medium => (240, 180),
            PipSize::Large => (320, 240),
            PipSize::Custom { width, height } => (*width, *height),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PipShape {
    Circle,
    Rectangle,
    RoundedRect,
}

#[derive(Debug, Clone)]
pub struct WebcamOverlay {
    pub enabled: bool,
    pub position: PipPosition,
    pub size: PipSize,
    pub shape: PipShape,
    pub border_color: u32,
    pub border_width: u8,
    pub device_id: Option<u32>,
    pub mirror: bool,
}

impl WebcamOverlay {
    pub fn new() -> Self {
        Self {
            enabled: false,
            position: PipPosition::BottomRight,
            size: PipSize::Small,
            shape: PipShape::Circle,
            border_color: OVERLAY_BORDER,
            border_width: 2,
            device_id: None,
            mirror: true,
        }
    }

    pub fn pip_rect(&self, screen_w: u32, screen_h: u32) -> (i32, i32, u32, u32) {
        let (pw, ph) = self.size.dimensions();
        let margin = 16i32;
        let (x, y) = match self.position {
            PipPosition::TopLeft => (margin, margin),
            PipPosition::TopRight => (screen_w as i32 - pw as i32 - margin, margin),
            PipPosition::BottomLeft => (margin, screen_h as i32 - ph as i32 - margin),
            PipPosition::BottomRight => (
                screen_w as i32 - pw as i32 - margin,
                screen_h as i32 - ph as i32 - margin,
            ),
        };
        (x, y, pw, ph)
    }
}

// ── Cursor settings ──────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct CursorSettings {
    pub show_cursor: bool,
    pub highlight_clicks: bool,
    pub click_ripple_color: u32,
    pub click_ripple_radius: u16,
    pub click_ripple_duration_ms: u16,
    pub spotlight_enabled: bool,
    pub spotlight_radius: u16,
    pub spotlight_dim_color: u32,
}

impl CursorSettings {
    pub fn new() -> Self {
        Self {
            show_cursor: true,
            highlight_clicks: true,
            click_ripple_color: 0x88_4E_9C_FF,
            click_ripple_radius: 30,
            click_ripple_duration_ms: 400,
            spotlight_enabled: false,
            spotlight_radius: 120,
            spotlight_dim_color: SPOTLIGHT_DIM,
        }
    }
}

// ── Live annotation ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveAnnotationKind {
    Draw,
    Text,
    Arrow,
    Spotlight,
    Zoom,
}

#[derive(Debug, Clone)]
pub struct LiveAnnotation {
    pub kind: LiveAnnotationKind,
    pub x: i32,
    pub y: i32,
    pub x2: i32,
    pub y2: i32,
    pub points: Vec<(i32, i32)>,
    pub text: String,
    pub color: u32,
    pub line_width: u8,
    pub zoom_factor: f32,
    pub frame_start: u64,
    pub frame_end: Option<u64>,
}

impl LiveAnnotation {
    pub fn new_draw(color: u32, line_width: u8) -> Self {
        Self {
            kind: LiveAnnotationKind::Draw,
            x: 0,
            y: 0,
            x2: 0,
            y2: 0,
            points: Vec::new(),
            text: String::new(),
            color,
            line_width,
            zoom_factor: 1.0,
            frame_start: 0,
            frame_end: None,
        }
    }

    pub fn new_text(x: i32, y: i32, text: &str, color: u32) -> Self {
        Self {
            kind: LiveAnnotationKind::Text,
            x,
            y,
            x2: x,
            y2: y,
            points: Vec::new(),
            text: String::from(text),
            color,
            line_width: 1,
            zoom_factor: 1.0,
            frame_start: 0,
            frame_end: None,
        }
    }

    pub fn new_arrow(x: i32, y: i32, x2: i32, y2: i32, color: u32) -> Self {
        Self {
            kind: LiveAnnotationKind::Arrow,
            x,
            y,
            x2,
            y2,
            points: Vec::new(),
            text: String::new(),
            color,
            line_width: 2,
            zoom_factor: 1.0,
            frame_start: 0,
            frame_end: None,
        }
    }

    pub fn new_spotlight(x: i32, y: i32, radius: i32) -> Self {
        Self {
            kind: LiveAnnotationKind::Spotlight,
            x,
            y,
            x2: radius,
            y2: 0,
            points: Vec::new(),
            text: String::new(),
            color: SPOTLIGHT_DIM,
            line_width: 1,
            zoom_factor: 1.0,
            frame_start: 0,
            frame_end: None,
        }
    }

    pub fn new_zoom(x: i32, y: i32, radius: i32, factor: f32) -> Self {
        Self {
            kind: LiveAnnotationKind::Zoom,
            x,
            y,
            x2: radius,
            y2: 0,
            points: Vec::new(),
            text: String::new(),
            color: 0xFF_FF_FF_FF,
            line_width: 2,
            zoom_factor: factor,
            frame_start: 0,
            frame_end: None,
        }
    }

    pub fn add_point(&mut self, px: i32, py: i32) {
        self.points.push((px, py));
    }
}

// ── Output formats ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoFormat {
    Mp4H264,
    WebmVp8,
    WebmVp9,
    Mkv,
    Gif,
}

impl VideoFormat {
    pub fn extension(&self) -> &'static str {
        match self {
            VideoFormat::Mp4H264 => "mp4",
            VideoFormat::WebmVp8 | VideoFormat::WebmVp9 => "webm",
            VideoFormat::Mkv => "mkv",
            VideoFormat::Gif => "gif",
        }
    }

    pub fn supports_audio(&self) -> bool {
        !matches!(self, VideoFormat::Gif)
    }
}

// ── Encoding settings ───────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Codec {
    H264,
    H265,
    Vp8,
    Vp9,
    Av1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncoderPreset {
    Ultrafast,
    Superfast,
    Veryfast,
    Faster,
    Fast,
    Medium,
    Slow,
    Slower,
    Veryslow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncoderProfile {
    Baseline,
    Main,
    High,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    Yuv420p,
    Yuv444p,
    Rgb24,
    Nv12,
}

#[derive(Debug, Clone)]
pub struct EncodingSettings {
    pub codec: Codec,
    pub crf: u8,
    pub preset: EncoderPreset,
    pub profile: EncoderProfile,
    pub pixel_format: PixelFormat,
    pub keyframe_interval: u16,
    pub two_pass: bool,
    pub hardware_accel: bool,
}

impl EncodingSettings {
    pub fn new() -> Self {
        Self {
            codec: Codec::H264,
            crf: 23,
            preset: EncoderPreset::Medium,
            profile: EncoderProfile::High,
            pixel_format: PixelFormat::Yuv420p,
            keyframe_interval: 250,
            two_pass: false,
            hardware_accel: true,
        }
    }

    pub fn set_crf(&mut self, crf: u8) {
        self.crf = crf.min(51);
    }

    pub fn estimated_file_size_mb(&self, duration_s: u32, bitrate_kbps: u32) -> u32 {
        (bitrate_kbps as u64 * duration_s as u64 / 8 / 1024) as u32
    }
}

// ── Post-processing ──────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct TrimRange {
    pub start_ms: u64,
    pub end_ms: u64,
}

#[derive(Debug, Clone)]
pub struct SplitPoint {
    pub timestamp_ms: u64,
    pub label: String,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SpeedMultiplier {
    Half,
    ThreeQuarters,
    Normal,
    OneAndHalf,
    Double,
    Triple,
    Quadruple,
    Custom(f32),
}

impl SpeedMultiplier {
    pub fn factor(&self) -> f32 {
        match self {
            SpeedMultiplier::Half => 0.5,
            SpeedMultiplier::ThreeQuarters => 0.75,
            SpeedMultiplier::Normal => 1.0,
            SpeedMultiplier::OneAndHalf => 1.5,
            SpeedMultiplier::Double => 2.0,
            SpeedMultiplier::Triple => 3.0,
            SpeedMultiplier::Quadruple => 4.0,
            SpeedMultiplier::Custom(f) => *f,
        }
    }
}

#[derive(Debug, Clone)]
pub struct PostProcessing {
    pub trim: Option<TrimRange>,
    pub splits: Vec<SplitPoint>,
    pub extract_frames: Vec<u64>,
    pub speed: SpeedMultiplier,
}

impl PostProcessing {
    pub fn new() -> Self {
        Self {
            trim: None,
            splits: Vec::new(),
            extract_frames: Vec::new(),
            speed: SpeedMultiplier::Normal,
        }
    }

    pub fn set_trim(&mut self, start_ms: u64, end_ms: u64) {
        if end_ms > start_ms {
            self.trim = Some(TrimRange { start_ms, end_ms });
        }
    }

    pub fn add_split(&mut self, ts_ms: u64, label: &str) {
        self.splits.push(SplitPoint {
            timestamp_ms: ts_ms,
            label: String::from(label),
        });
        self.splits.sort_by_key(|s| s.timestamp_ms);
    }

    pub fn add_frame_extraction(&mut self, ts_ms: u64) {
        self.extract_frames.push(ts_ms);
        self.extract_frames.sort();
    }

    pub fn set_speed(&mut self, speed: SpeedMultiplier) {
        self.speed = speed;
    }

    pub fn effective_duration_ms(&self, original_ms: u64) -> u64 {
        let trimmed = if let Some(ref t) = self.trim {
            t.end_ms - t.start_ms
        } else {
            original_ms
        };
        (trimmed as f32 / self.speed.factor()) as u64
    }
}

// ── Hotkeys ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordHotkeyAction {
    StartStop,
    PauseResume,
    Cancel,
    ToggleAnnotation,
    ToggleWebcam,
    ToggleMic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordModifier {
    None,
    Ctrl,
    Alt,
    Shift,
    CtrlShift,
    CtrlAlt,
    Super,
    SuperShift,
}

#[derive(Debug, Clone)]
pub struct RecordHotkey {
    pub action: RecordHotkeyAction,
    pub modifier: RecordModifier,
    pub key_code: u16,
    pub description: String,
}

pub struct HotkeyManager {
    pub bindings: Vec<RecordHotkey>,
}

impl HotkeyManager {
    pub fn new() -> Self {
        let mut mgr = Self {
            bindings: Vec::new(),
        };
        mgr.register_defaults();
        mgr
    }

    fn register_defaults(&mut self) {
        self.bindings.push(RecordHotkey {
            action: RecordHotkeyAction::StartStop,
            modifier: RecordModifier::CtrlShift,
            key_code: 0x52, // R
            description: String::from("Start/Stop recording"),
        });
        self.bindings.push(RecordHotkey {
            action: RecordHotkeyAction::PauseResume,
            modifier: RecordModifier::CtrlShift,
            key_code: 0x50, // P
            description: String::from("Pause/Resume recording"),
        });
        self.bindings.push(RecordHotkey {
            action: RecordHotkeyAction::Cancel,
            modifier: RecordModifier::None,
            key_code: 0x1B, // Escape
            description: String::from("Cancel recording"),
        });
        self.bindings.push(RecordHotkey {
            action: RecordHotkeyAction::ToggleAnnotation,
            modifier: RecordModifier::CtrlShift,
            key_code: 0x41, // A
            description: String::from("Toggle annotation mode"),
        });
        self.bindings.push(RecordHotkey {
            action: RecordHotkeyAction::ToggleWebcam,
            modifier: RecordModifier::CtrlShift,
            key_code: 0x57, // W
            description: String::from("Toggle webcam overlay"),
        });
        self.bindings.push(RecordHotkey {
            action: RecordHotkeyAction::ToggleMic,
            modifier: RecordModifier::CtrlShift,
            key_code: 0x4D, // M
            description: String::from("Toggle microphone"),
        });
    }

    pub fn find_action(
        &self,
        modifier: RecordModifier,
        key_code: u16,
    ) -> Option<RecordHotkeyAction> {
        self.bindings
            .iter()
            .find(|b| b.modifier == modifier && b.key_code == key_code)
            .map(|b| b.action)
    }

    pub fn rebind(&mut self, action: RecordHotkeyAction, modifier: RecordModifier, key_code: u16) {
        if let Some(b) = self.bindings.iter_mut().find(|b| b.action == action) {
            b.modifier = modifier;
            b.key_code = key_code;
        }
    }
}

// ── Recording state ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordingState {
    Idle,
    Countdown { remaining_s: u8 },
    Recording,
    Paused,
    Stopping,
    PostProcessing,
}

// ── Auto-stop conditions ─────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct AutoStopConfig {
    pub max_duration_ms: Option<u64>,
    pub max_file_size_bytes: Option<u64>,
    pub min_free_disk_bytes: u64,
    pub enabled: bool,
}

impl AutoStopConfig {
    pub fn new() -> Self {
        Self {
            max_duration_ms: None,
            max_file_size_bytes: None,
            min_free_disk_bytes: 512 * 1024 * 1024,
            enabled: true,
        }
    }

    pub fn should_stop(&self, elapsed_ms: u64, file_size: u64, free_disk: u64) -> bool {
        if !self.enabled {
            return false;
        }
        if let Some(max_dur) = self.max_duration_ms {
            if elapsed_ms >= max_dur {
                return true;
            }
        }
        if let Some(max_sz) = self.max_file_size_bytes {
            if file_size >= max_sz {
                return true;
            }
        }
        if free_disk < self.min_free_disk_bytes {
            return true;
        }
        false
    }
}

// ── Status indicator ─────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct StatusIndicator {
    pub show_dot: bool,
    pub show_timer: bool,
    pub show_tray: bool,
    pub overlay_position: (i32, i32),
    pub blink_interval_ms: u32,
    pub blink_state: bool,
    pub last_blink_ms: u64,
}

impl StatusIndicator {
    pub fn new() -> Self {
        Self {
            show_dot: true,
            show_timer: true,
            show_tray: true,
            overlay_position: (8, 8),
            blink_interval_ms: 500,
            blink_state: true,
            last_blink_ms: 0,
        }
    }

    pub fn tick(&mut self, now_ms: u64) {
        if now_ms - self.last_blink_ms >= self.blink_interval_ms as u64 {
            self.blink_state = !self.blink_state;
            self.last_blink_ms = now_ms;
        }
    }

    pub fn format_timer(elapsed_ms: u64) -> (u32, u32, u32) {
        let total_s = (elapsed_ms / 1000) as u32;
        let h = total_s / 3600;
        let m = (total_s % 3600) / 60;
        let s = total_s % 60;
        (h, m, s)
    }
}

// ── Streaming output ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamProtocol {
    Rtmp,
    Srt,
}

#[derive(Debug, Clone)]
pub struct StreamConfig {
    pub enabled: bool,
    pub protocol: StreamProtocol,
    pub endpoint_url: String,
    pub stream_key: String,
    pub bitrate_kbps: u32,
    pub reconnect_on_drop: bool,
    pub reconnect_attempts: u8,
    pub buffer_ms: u32,
}

impl StreamConfig {
    pub fn new() -> Self {
        Self {
            enabled: false,
            protocol: StreamProtocol::Rtmp,
            endpoint_url: String::new(),
            stream_key: String::new(),
            bitrate_kbps: 6000,
            reconnect_on_drop: true,
            reconnect_attempts: 5,
            buffer_ms: 2000,
        }
    }

    pub fn set_rtmp(&mut self, url: &str, key: &str) {
        self.protocol = StreamProtocol::Rtmp;
        self.endpoint_url = String::from(url);
        self.stream_key = String::from(key);
    }

    pub fn set_srt(&mut self, url: &str) {
        self.protocol = StreamProtocol::Srt;
        self.endpoint_url = String::from(url);
        self.stream_key.clear();
    }

    pub fn is_configured(&self) -> bool {
        self.enabled && !self.endpoint_url.is_empty()
    }
}

// ── Recording region ─────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct RecordRegion {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub selecting: bool,
    pub anchor_x: i32,
    pub anchor_y: i32,
}

impl RecordRegion {
    pub fn full_screen(w: u32, h: u32) -> Self {
        Self {
            x: 0,
            y: 0,
            width: w,
            height: h,
            selecting: false,
            anchor_x: 0,
            anchor_y: 0,
        }
    }

    pub fn begin_selection(&mut self, px: i32, py: i32) {
        self.selecting = true;
        self.anchor_x = px;
        self.anchor_y = py;
        self.x = px;
        self.y = py;
        self.width = 0;
        self.height = 0;
    }

    pub fn update_selection(&mut self, px: i32, py: i32) {
        if !self.selecting {
            return;
        }
        self.x = self.anchor_x.min(px);
        self.y = self.anchor_y.min(py);
        self.width = (px - self.anchor_x).unsigned_abs();
        self.height = (py - self.anchor_y).unsigned_abs();
    }

    pub fn finish_selection(&mut self) {
        self.selecting = false;
    }
}

// ── Click ripple effect ──────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ClickRipple {
    pub x: i32,
    pub y: i32,
    pub color: u32,
    pub max_radius: u16,
    pub duration_ms: u16,
    pub elapsed_ms: u16,
    pub is_right_click: bool,
}

impl ClickRipple {
    pub fn new(x: i32, y: i32, settings: &CursorSettings, right: bool) -> Self {
        Self {
            x,
            y,
            color: settings.click_ripple_color,
            max_radius: settings.click_ripple_radius,
            duration_ms: settings.click_ripple_duration_ms,
            elapsed_ms: 0,
            is_right_click: right,
        }
    }

    pub fn tick(&mut self, dt_ms: u16) -> bool {
        self.elapsed_ms = self.elapsed_ms.saturating_add(dt_ms);
        self.elapsed_ms < self.duration_ms
    }

    pub fn current_radius(&self) -> u16 {
        let progress = self.elapsed_ms as u32 * 1000 / self.duration_ms.max(1) as u32;
        (self.max_radius as u32 * progress / 1000) as u16
    }

    pub fn current_opacity(&self) -> u8 {
        let progress = self.elapsed_ms as u32 * 255 / self.duration_ms.max(1) as u32;
        255u8.saturating_sub(progress as u8)
    }
}

// ── Frame buffer for recording ───────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct RecordedFrame {
    pub index: u64,
    pub timestamp_ms: u64,
    pub data_offset: u64,
    pub data_size: u32,
    pub is_keyframe: bool,
}

pub struct FrameBuffer {
    pub frames: Vec<RecordedFrame>,
    pub total_bytes_written: u64,
    pub dropped_frames: u64,
    pub target_interval_ms: u32,
    pub last_frame_ms: u64,
}

impl FrameBuffer {
    pub fn new(fps: u16) -> Self {
        Self {
            frames: Vec::new(),
            total_bytes_written: 0,
            dropped_frames: 0,
            target_interval_ms: (1000 / fps.max(1) as u32),
            last_frame_ms: 0,
        }
    }

    pub fn should_capture(&self, now_ms: u64) -> bool {
        now_ms - self.last_frame_ms >= self.target_interval_ms as u64
    }

    pub fn push_frame(&mut self, ts_ms: u64, size: u32, keyframe: bool) {
        let idx = self.frames.len() as u64;
        self.frames.push(RecordedFrame {
            index: idx,
            timestamp_ms: ts_ms,
            data_offset: self.total_bytes_written,
            data_size: size,
            is_keyframe: keyframe,
        });
        self.total_bytes_written += size as u64;
        self.last_frame_ms = ts_ms;
    }

    pub fn frame_count(&self) -> u64 {
        self.frames.len() as u64
    }

    pub fn total_duration_ms(&self) -> u64 {
        if self.frames.is_empty() {
            return 0;
        }
        self.frames.last().unwrap().timestamp_ms - self.frames.first().unwrap().timestamp_ms
    }
}

// ── Main screen recorder ─────────────────────────────────────────────────

pub struct ScreenRecorder {
    pub state: RecordingState,
    pub capture_mode: RecordCaptureMode,
    pub video: VideoSettings,
    pub audio: AudioSettings,
    pub webcam: WebcamOverlay,
    pub cursor: CursorSettings,
    pub annotations: Vec<LiveAnnotation>,
    pub output_format: VideoFormat,
    pub output_directory: String,
    pub filename_template: String,
    pub encoding: EncodingSettings,
    pub post: PostProcessing,
    pub hotkeys: HotkeyManager,
    pub status: StatusIndicator,
    pub auto_stop: AutoStopConfig,
    pub stream: StreamConfig,
    pub region: RecordRegion,
    pub ripples: Vec<ClickRipple>,
    pub frame_buffer: FrameBuffer,
    pub countdown_value: u8,
    pub elapsed_ms: u64,
    pub paused_elapsed_ms: u64,
    pub recording_start_ms: u64,
    pub annotating: bool,
    pub screen_width: u32,
    pub screen_height: u32,
    pub file_counter: u32,
}

impl ScreenRecorder {
    pub fn new(screen_w: u32, screen_h: u32) -> Self {
        Self {
            state: RecordingState::Idle,
            capture_mode: RecordCaptureMode::FullScreen,
            video: VideoSettings::new(screen_w, screen_h),
            audio: AudioSettings::new(),
            webcam: WebcamOverlay::new(),
            cursor: CursorSettings::new(),
            annotations: Vec::new(),
            output_format: VideoFormat::Mp4H264,
            output_directory: String::from("/home/recordings"),
            filename_template: String::from("recording_{counter}"),
            encoding: EncodingSettings::new(),
            post: PostProcessing::new(),
            hotkeys: HotkeyManager::new(),
            status: StatusIndicator::new(),
            auto_stop: AutoStopConfig::new(),
            stream: StreamConfig::new(),
            region: RecordRegion::full_screen(screen_w, screen_h),
            ripples: Vec::new(),
            frame_buffer: FrameBuffer::new(30),
            countdown_value: 3,
            elapsed_ms: 0,
            paused_elapsed_ms: 0,
            recording_start_ms: 0,
            annotating: false,
            screen_width: screen_w,
            screen_height: screen_h,
            file_counter: 0,
        }
    }

    pub fn start_countdown(&mut self) {
        self.state = RecordingState::Countdown {
            remaining_s: self.countdown_value,
        };
    }

    pub fn tick_countdown(&mut self) -> bool {
        if let RecordingState::Countdown { remaining_s } = &mut self.state {
            if *remaining_s == 0 {
                return true;
            }
            *remaining_s -= 1;
            *remaining_s == 0
        } else {
            false
        }
    }

    pub fn start_recording(&mut self, now_ms: u64) {
        self.state = RecordingState::Recording;
        self.recording_start_ms = now_ms;
        self.elapsed_ms = 0;
        self.paused_elapsed_ms = 0;
        let fps = self.video.frame_rate.value();
        self.frame_buffer = FrameBuffer::new(fps);
        self.annotations.clear();
        self.ripples.clear();
    }

    pub fn pause(&mut self) {
        if self.state == RecordingState::Recording {
            self.state = RecordingState::Paused;
        }
    }

    pub fn resume(&mut self) {
        if self.state == RecordingState::Paused {
            self.state = RecordingState::Recording;
        }
    }

    pub fn toggle_pause(&mut self) {
        match self.state {
            RecordingState::Recording => self.pause(),
            RecordingState::Paused => self.resume(),
            _ => {}
        }
    }

    pub fn stop(&mut self) {
        match self.state {
            RecordingState::Recording | RecordingState::Paused => {
                self.state = RecordingState::Stopping;
            }
            _ => {}
        }
    }

    pub fn cancel(&mut self) {
        self.state = RecordingState::Idle;
        self.elapsed_ms = 0;
        self.frame_buffer = FrameBuffer::new(self.video.frame_rate.value());
    }

    pub fn tick(&mut self, now_ms: u64, free_disk: u64) {
        if self.state == RecordingState::Recording {
            self.elapsed_ms =
                now_ms.saturating_sub(self.recording_start_ms) - self.paused_elapsed_ms;
            self.status.tick(now_ms);

            self.ripples.retain_mut(|r| r.tick(16));

            if self.auto_stop.should_stop(
                self.elapsed_ms,
                self.frame_buffer.total_bytes_written,
                free_disk,
            ) {
                self.stop();
            }
        }
    }

    pub fn on_click(&mut self, x: i32, y: i32, right: bool) {
        if self.state != RecordingState::Recording {
            return;
        }
        if self.cursor.highlight_clicks {
            self.ripples
                .push(ClickRipple::new(x, y, &self.cursor, right));
        }
    }

    pub fn add_annotation(&mut self, ann: LiveAnnotation) {
        self.annotations.push(ann);
    }

    pub fn generate_filename(&mut self) -> String {
        let c = self.file_counter;
        self.file_counter += 1;
        let ext = self.output_format.extension();
        format!("{}/recording_{}.{}", self.output_directory, c, ext)
    }

    pub fn handle_hotkey(
        &mut self,
        modifier: RecordModifier,
        key_code: u16,
        now_ms: u64,
    ) -> Option<RecordHotkeyAction> {
        let action = self.hotkeys.find_action(modifier, key_code)?;
        match action {
            RecordHotkeyAction::StartStop => match self.state {
                RecordingState::Idle => self.start_countdown(),
                RecordingState::Recording | RecordingState::Paused => self.stop(),
                _ => {}
            },
            RecordHotkeyAction::PauseResume => self.toggle_pause(),
            RecordHotkeyAction::Cancel => self.cancel(),
            RecordHotkeyAction::ToggleAnnotation => {
                self.annotating = !self.annotating;
            }
            RecordHotkeyAction::ToggleWebcam => {
                self.webcam.enabled = !self.webcam.enabled;
            }
            RecordHotkeyAction::ToggleMic => {
                let new_src = match self.audio.source {
                    AudioSource::Both => AudioSource::SystemAudio,
                    AudioSource::Microphone => AudioSource::None,
                    AudioSource::SystemAudio => AudioSource::Both,
                    AudioSource::None => AudioSource::Microphone,
                };
                self.audio.set_source(new_src);
            }
        }
        Some(action)
    }

    pub fn is_recording(&self) -> bool {
        matches!(self.state, RecordingState::Recording)
    }

    pub fn is_paused(&self) -> bool {
        matches!(self.state, RecordingState::Paused)
    }

    pub fn elapsed_time(&self) -> (u32, u32, u32) {
        StatusIndicator::format_timer(self.elapsed_ms)
    }

    pub fn estimated_file_size_mb(&self) -> u32 {
        let duration_s = (self.elapsed_ms / 1000) as u32;
        self.encoding
            .estimated_file_size_mb(duration_s, self.video.bitrate.kbps())
    }
}

// ── Global instance ──────────────────────────────────────────────────────

static mut SCREEN_RECORDER: Option<ScreenRecorder> = None;

pub unsafe fn init() {
    SCREEN_RECORDER = Some(ScreenRecorder::new(1920, 1080));
}

pub unsafe fn get() -> &'static mut ScreenRecorder {
    SCREEN_RECORDER
        .as_mut()
        .expect("screen_recorder::init() not called")
}
