//! Full-featured screenshot tool for the RaeenOS desktop shell.
//!
//! Supports multiple capture modes (full screen, active window, region,
//! freeform, scrolling, delayed), multi-monitor stitching, annotation
//! tools, OCR integration, image editing, history, pin-to-screen, and
//! configurable keybindings.

#![allow(unused)]

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

// ── no_std math helpers ──────────────────────────────────────────────────

fn f64_sqrt(x: f64) -> f64 {
    if x <= 0.0 {
        return 0.0;
    }
    let mut g = x / 2.0;
    for _ in 0..20 {
        g = (g + x / g) * 0.5;
    }
    g
}

fn f64_acos(x: f64) -> f64 {
    let x_clamped = if x < -1.0 {
        -1.0
    } else if x > 1.0 {
        1.0
    } else {
        x
    };
    let pi_half = core::f64::consts::PI / 2.0;
    pi_half - f64_asin(x_clamped)
}

fn f64_asin(x: f64) -> f64 {
    let x2 = x * x;
    let mut result = x;
    let mut term = x;
    for n in 1..15u64 {
        term *=
            x2 * (2 * n - 1) as f64 * (2 * n - 1) as f64 / ((2 * n) as f64 * (2 * n + 1) as f64);
        result += term;
    }
    result
}

// ── Colour constants ─────────────────────────────────────────────────────

const TOOLBAR_BG: u32 = 0xFF_0A_0E_1A;
const TOOLBAR_BORDER: u32 = 0xFF_4E_9C_FF;
const OVERLAY_DIM: u32 = 0x88_00_00_00;
const SELECTION_BORDER: u32 = 0xFF_4E_9C_FF;
const CROSSHAIR_COLOR: u32 = 0xFF_FF_FF_FF;
const HANDLE_COLOR: u32 = 0xFF_4E_9C_FF;
const TEXT_FG: u32 = 0xFF_F0_F0_F8;
const TEXT_DIM: u32 = 0xFF_88_8C_A0;
const RULER_FG: u32 = 0xFF_AA_CC_FF;

// ── Capture mode ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureMode {
    FullScreen,
    ActiveWindow,
    RectangularRegion,
    FreeformRegion,
    ScrollingCapture,
    Delayed3s,
    Delayed5s,
    Delayed10s,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MonitorTarget {
    Primary,
    ByIndex(u8),
    AllStitched,
}

// ── Output format ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageFormat {
    Png,
    Jpeg,
    Bmp,
    WebP,
    Clipboard,
}

#[derive(Debug, Clone)]
pub struct OutputSettings {
    pub format: ImageFormat,
    pub jpeg_quality: u8,
    pub save_directory: String,
    pub filename_template: String,
    pub auto_copy_clipboard: bool,
    pub auto_save: bool,
    pub counter: u32,
}

impl OutputSettings {
    pub fn new() -> Self {
        Self {
            format: ImageFormat::Png,
            jpeg_quality: 85,
            save_directory: String::from("/home/screenshots"),
            filename_template: String::from("{date}_{time}_{counter}"),
            auto_copy_clipboard: true,
            auto_save: true,
            counter: 0,
        }
    }

    pub fn generate_filename(&mut self, window_title: &str) -> String {
        let c = self.counter;
        self.counter += 1;
        let ext = match self.format {
            ImageFormat::Png => "png",
            ImageFormat::Jpeg => "jpg",
            ImageFormat::Bmp => "bmp",
            ImageFormat::WebP => "webp",
            ImageFormat::Clipboard => "png",
        };
        format!(
            "{}/screenshot_{}_{}.{}",
            self.save_directory, c, window_title, ext
        )
    }
}

// ── Annotation types ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnnotationKind {
    Arrow,
    Line,
    Rectangle,
    Ellipse,
    Text,
    NumberMarker,
    BlurRegion,
    PixelateRegion,
    Highlight,
    Crop,
    Pen,
    Marker,
    StickerEmoji,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FillMode {
    Outline,
    Filled,
    SemiTransparent,
}

#[derive(Debug, Clone)]
pub struct AnnotationProperties {
    pub line_width: u8,
    pub font_size: u8,
    pub color: u32,
    pub fill_mode: FillMode,
    pub opacity: u8,
}

impl AnnotationProperties {
    pub fn new() -> Self {
        Self {
            line_width: 2,
            font_size: 14,
            color: 0xFF_FF_00_00,
            fill_mode: FillMode::Outline,
            opacity: 255,
        }
    }

    pub fn set_line_width(&mut self, w: u8) {
        self.line_width = w.clamp(1, 10);
    }

    pub fn set_opacity(&mut self, o: u8) {
        self.opacity = o;
    }
}

#[derive(Debug, Clone)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

#[derive(Debug, Clone)]
pub struct Annotation {
    pub kind: AnnotationKind,
    pub start: Point,
    pub end: Point,
    pub points: Vec<Point>,
    pub text: String,
    pub number: u32,
    pub props: AnnotationProperties,
}

impl Annotation {
    pub fn new_line(x0: i32, y0: i32, x1: i32, y1: i32, props: AnnotationProperties) -> Self {
        Self {
            kind: AnnotationKind::Line,
            start: Point { x: x0, y: y0 },
            end: Point { x: x1, y: y1 },
            points: Vec::new(),
            text: String::new(),
            number: 0,
            props,
        }
    }

    pub fn new_arrow(x0: i32, y0: i32, x1: i32, y1: i32, props: AnnotationProperties) -> Self {
        Self {
            kind: AnnotationKind::Arrow,
            start: Point { x: x0, y: y0 },
            end: Point { x: x1, y: y1 },
            points: Vec::new(),
            text: String::new(),
            number: 0,
            props,
        }
    }

    pub fn new_rect(x0: i32, y0: i32, x1: i32, y1: i32, props: AnnotationProperties) -> Self {
        Self {
            kind: AnnotationKind::Rectangle,
            start: Point { x: x0, y: y0 },
            end: Point { x: x1, y: y1 },
            points: Vec::new(),
            text: String::new(),
            number: 0,
            props,
        }
    }

    pub fn new_ellipse(x0: i32, y0: i32, x1: i32, y1: i32, props: AnnotationProperties) -> Self {
        Self {
            kind: AnnotationKind::Ellipse,
            start: Point { x: x0, y: y0 },
            end: Point { x: x1, y: y1 },
            points: Vec::new(),
            text: String::new(),
            number: 0,
            props,
        }
    }

    pub fn new_text(x: i32, y: i32, content: &str, props: AnnotationProperties) -> Self {
        Self {
            kind: AnnotationKind::Text,
            start: Point { x, y },
            end: Point { x, y },
            points: Vec::new(),
            text: String::from(content),
            number: 0,
            props,
        }
    }

    pub fn new_number_marker(x: i32, y: i32, num: u32, props: AnnotationProperties) -> Self {
        Self {
            kind: AnnotationKind::NumberMarker,
            start: Point { x, y },
            end: Point { x, y },
            points: Vec::new(),
            text: String::new(),
            number: num,
            props,
        }
    }

    pub fn new_blur(x0: i32, y0: i32, x1: i32, y1: i32) -> Self {
        Self {
            kind: AnnotationKind::BlurRegion,
            start: Point { x: x0, y: y0 },
            end: Point { x: x1, y: y1 },
            points: Vec::new(),
            text: String::new(),
            number: 0,
            props: AnnotationProperties::new(),
        }
    }

    pub fn new_pixelate(x0: i32, y0: i32, x1: i32, y1: i32) -> Self {
        Self {
            kind: AnnotationKind::PixelateRegion,
            start: Point { x: x0, y: y0 },
            end: Point { x: x1, y: y1 },
            points: Vec::new(),
            text: String::new(),
            number: 0,
            props: AnnotationProperties::new(),
        }
    }

    pub fn new_highlight(x0: i32, y0: i32, x1: i32, y1: i32, color: u32) -> Self {
        let mut props = AnnotationProperties::new();
        props.color = color;
        props.opacity = 100;
        props.fill_mode = FillMode::SemiTransparent;
        Self {
            kind: AnnotationKind::Highlight,
            start: Point { x: x0, y: y0 },
            end: Point { x: x1, y: y1 },
            points: Vec::new(),
            text: String::new(),
            number: 0,
            props,
        }
    }

    pub fn new_pen(pts: Vec<Point>, props: AnnotationProperties) -> Self {
        let start = pts
            .first()
            .map(|p| p.clone())
            .unwrap_or(Point { x: 0, y: 0 });
        let end = pts
            .last()
            .map(|p| p.clone())
            .unwrap_or(Point { x: 0, y: 0 });
        Self {
            kind: AnnotationKind::Pen,
            start,
            end,
            points: pts,
            text: String::new(),
            number: 0,
            props,
        }
    }

    pub fn bounding_rect(&self) -> (i32, i32, i32, i32) {
        let min_x = self.start.x.min(self.end.x);
        let min_y = self.start.y.min(self.end.y);
        let max_x = self.start.x.max(self.end.x);
        let max_y = self.start.y.max(self.end.y);
        (min_x, min_y, max_x, max_y)
    }
}

// ── Colour picker ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorSpace {
    Rgb,
    Hsl,
    Hex,
}

#[derive(Debug, Clone)]
pub struct ColorPicker {
    pub active: bool,
    pub current_color: u32,
    pub recent_colors: Vec<u32>,
    pub custom_r: u8,
    pub custom_g: u8,
    pub custom_b: u8,
    pub custom_h: u16,
    pub custom_s: u8,
    pub custom_l: u8,
    pub mode: ColorSpace,
    pub eyedropper_active: bool,
    pub max_recent: usize,
}

impl ColorPicker {
    pub fn new() -> Self {
        Self {
            active: false,
            current_color: 0xFF_FF_00_00,
            recent_colors: Vec::new(),
            custom_r: 255,
            custom_g: 0,
            custom_b: 0,
            custom_h: 0,
            custom_s: 100,
            custom_l: 50,
            mode: ColorSpace::Rgb,
            eyedropper_active: false,
            max_recent: 16,
        }
    }

    pub fn set_rgb(&mut self, r: u8, g: u8, b: u8) {
        self.custom_r = r;
        self.custom_g = g;
        self.custom_b = b;
        self.current_color = 0xFF_00_00_00 | (r as u32) << 16 | (g as u32) << 8 | b as u32;
        self.push_recent(self.current_color);
    }

    pub fn set_hex(&mut self, hex: u32) {
        self.custom_r = ((hex >> 16) & 0xFF) as u8;
        self.custom_g = ((hex >> 8) & 0xFF) as u8;
        self.custom_b = (hex & 0xFF) as u8;
        self.current_color = 0xFF_00_00_00 | (hex & 0x00_FF_FF_FF);
        self.push_recent(self.current_color);
    }

    pub fn set_hsl(&mut self, h: u16, s: u8, l: u8) {
        self.custom_h = h % 360;
        self.custom_s = s.min(100);
        self.custom_l = l.min(100);
        let (r, g, b) = hsl_to_rgb(self.custom_h, self.custom_s, self.custom_l);
        self.custom_r = r;
        self.custom_g = g;
        self.custom_b = b;
        self.current_color = 0xFF_00_00_00 | (r as u32) << 16 | (g as u32) << 8 | b as u32;
        self.push_recent(self.current_color);
    }

    pub fn pick_from_screen(&mut self, pixel_color: u32) {
        self.set_hex(pixel_color & 0x00_FF_FF_FF);
        self.eyedropper_active = false;
    }

    pub fn start_eyedropper(&mut self) {
        self.eyedropper_active = true;
    }

    fn push_recent(&mut self, c: u32) {
        self.recent_colors.retain(|&rc| rc != c);
        self.recent_colors.insert(0, c);
        if self.recent_colors.len() > self.max_recent {
            self.recent_colors.pop();
        }
    }
}

fn hsl_to_rgb(h: u16, s: u8, l: u8) -> (u8, u8, u8) {
    if s == 0 {
        let v = (l as u32 * 255 / 100) as u8;
        return (v, v, v);
    }
    let h = h as u32;
    let s = s as u32;
    let l = l as u32;
    let q = if l < 50 {
        l * (100 + s) / 100
    } else {
        l + s - l * s / 100
    };
    let p = 2 * l - q;
    let r = hue_to_channel(p, q, h + 120);
    let g = hue_to_channel(p, q, h);
    let b = hue_to_channel(p, q, h + 240);
    (r, g, b)
}

fn hue_to_channel(p: u32, q: u32, mut t: u32) -> u8 {
    t %= 360;
    let val = if t < 60 {
        p + (q - p) * t / 60
    } else if t < 180 {
        q
    } else if t < 240 {
        p + (q - p) * (240 - t) / 60
    } else {
        p
    };
    (val * 255 / 100) as u8
}

// ── Image editing ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RotateAngle {
    Cw90,
    Ccw90,
    Rotate180,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlipDirection {
    Horizontal,
    Vertical,
}

#[derive(Debug, Clone)]
pub struct ImageAdjustments {
    pub brightness: i8,
    pub contrast: i8,
    pub resize_width: Option<u32>,
    pub resize_height: Option<u32>,
    pub rotation: Option<RotateAngle>,
    pub flip: Option<FlipDirection>,
}

impl ImageAdjustments {
    pub fn new() -> Self {
        Self {
            brightness: 0,
            contrast: 0,
            resize_width: None,
            resize_height: None,
            rotation: None,
            flip: None,
        }
    }

    pub fn set_brightness(&mut self, b: i8) {
        self.brightness = b.clamp(-100, 100);
    }

    pub fn set_contrast(&mut self, c: i8) {
        self.contrast = c.clamp(-100, 100);
    }

    pub fn set_resize(&mut self, w: u32, h: u32) {
        self.resize_width = Some(w);
        self.resize_height = Some(h);
    }

    pub fn rotate(&mut self, angle: RotateAngle) {
        self.rotation = Some(angle);
    }

    pub fn flip(&mut self, dir: FlipDirection) {
        self.flip = Some(dir);
    }

    pub fn apply_brightness_pixel(&self, pixel: u32) -> u32 {
        let a = (pixel >> 24) & 0xFF;
        let r = ((pixel >> 16) & 0xFF) as i32;
        let g = ((pixel >> 8) & 0xFF) as i32;
        let b = (pixel & 0xFF) as i32;
        let adj = self.brightness as i32 * 255 / 100;
        let r2 = (r + adj).clamp(0, 255) as u32;
        let g2 = (g + adj).clamp(0, 255) as u32;
        let b2 = (b + adj).clamp(0, 255) as u32;
        (a << 24) | (r2 << 16) | (g2 << 8) | b2
    }

    pub fn apply_contrast_pixel(&self, pixel: u32) -> u32 {
        let a = (pixel >> 24) & 0xFF;
        let r = ((pixel >> 16) & 0xFF) as i32;
        let g = ((pixel >> 8) & 0xFF) as i32;
        let b = (pixel & 0xFF) as i32;
        let factor = (100 + self.contrast as i32) * 256 / 100;
        let r2 = (((r - 128) * factor / 256) + 128).clamp(0, 255) as u32;
        let g2 = (((g - 128) * factor / 256) + 128).clamp(0, 255) as u32;
        let b2 = (((b - 128) * factor / 256) + 128).clamp(0, 255) as u32;
        (a << 24) | (r2 << 16) | (g2 << 8) | b2
    }
}

// ── Screenshot history ───────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct HistoryEntry {
    pub id: u64,
    pub filepath: String,
    pub timestamp: u64,
    pub width: u32,
    pub height: u32,
    pub format: ImageFormat,
    pub file_size: u64,
    pub thumbnail: Vec<u8>,
    pub window_title: String,
    pub mode: CaptureMode,
}

pub struct ScreenshotHistory {
    pub entries: Vec<HistoryEntry>,
    pub next_id: u64,
    pub max_entries: usize,
}

impl ScreenshotHistory {
    pub fn new(max: usize) -> Self {
        Self {
            entries: Vec::new(),
            next_id: 1,
            max_entries: max,
        }
    }

    pub fn add(
        &mut self,
        filepath: &str,
        ts: u64,
        w: u32,
        h: u32,
        fmt: ImageFormat,
        size: u64,
        thumb: Vec<u8>,
        win_title: &str,
        mode: CaptureMode,
    ) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.entries.push(HistoryEntry {
            id,
            filepath: String::from(filepath),
            timestamp: ts,
            width: w,
            height: h,
            format: fmt,
            file_size: size,
            thumbnail: thumb,
            window_title: String::from(win_title),
            mode,
        });
        while self.entries.len() > self.max_entries {
            self.entries.remove(0);
        }
        id
    }

    pub fn remove(&mut self, id: u64) {
        self.entries.retain(|e| e.id != id);
    }

    pub fn get(&self, id: u64) -> Option<&HistoryEntry> {
        self.entries.iter().find(|e| e.id == id)
    }

    pub fn recent(&self, count: usize) -> &[HistoryEntry] {
        let start = self.entries.len().saturating_sub(count);
        &self.entries[start..]
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

// ── Pin to screen ────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct PinnedScreenshot {
    pub id: u64,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub opacity: u8,
    pub always_on_top: bool,
    pub dragging: bool,
    pub data: Vec<u8>,
}

impl PinnedScreenshot {
    pub fn new(id: u64, x: i32, y: i32, w: u32, h: u32, data: Vec<u8>) -> Self {
        Self {
            id,
            x,
            y,
            width: w,
            height: h,
            opacity: 255,
            always_on_top: true,
            dragging: false,
            data,
        }
    }

    pub fn set_opacity(&mut self, o: u8) {
        self.opacity = o;
    }

    pub fn move_to(&mut self, x: i32, y: i32) {
        self.x = x;
        self.y = y;
    }

    pub fn contains(&self, px: i32, py: i32) -> bool {
        px >= self.x
            && px < self.x + self.width as i32
            && py >= self.y
            && py < self.y + self.height as i32
    }
}

// ── Sharing ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShareTarget {
    SaveFile,
    CopyClipboard,
    Upload,
    Print,
}

#[derive(Debug, Clone)]
pub struct ShareOptions {
    pub target: ShareTarget,
    pub upload_url: String,
    pub auto_open_after_save: bool,
}

impl ShareOptions {
    pub fn new() -> Self {
        Self {
            target: ShareTarget::SaveFile,
            upload_url: String::from("https://upload.raeenos.local/screenshot"),
            auto_open_after_save: false,
        }
    }
}

// ── Keybinding ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Modifier {
    None,
    Shift,
    Ctrl,
    Alt,
    Super,
    CtrlShift,
    SuperShift,
    CtrlAlt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyAction {
    CaptureFullScreen,
    CaptureActiveWindow,
    CaptureRegion,
    CaptureFreeform,
    OpenSnippingToolbar,
    ToggleRecording,
    QuickAnnotate,
    CopyLastScreenshot,
}

#[derive(Debug, Clone)]
pub struct KeyBinding {
    pub action: KeyAction,
    pub modifier: Modifier,
    pub key_code: u16,
    pub description: String,
}

pub struct KeyBindingManager {
    pub bindings: Vec<KeyBinding>,
}

impl KeyBindingManager {
    pub fn new() -> Self {
        let mut mgr = Self {
            bindings: Vec::new(),
        };
        mgr.register_defaults();
        mgr
    }

    fn register_defaults(&mut self) {
        self.bindings.push(KeyBinding {
            action: KeyAction::CaptureFullScreen,
            modifier: Modifier::None,
            key_code: 0x9A, // PrintScreen
            description: String::from("Capture full screen"),
        });
        self.bindings.push(KeyBinding {
            action: KeyAction::CaptureActiveWindow,
            modifier: Modifier::Alt,
            key_code: 0x9A,
            description: String::from("Capture active window"),
        });
        self.bindings.push(KeyBinding {
            action: KeyAction::CaptureRegion,
            modifier: Modifier::SuperShift,
            key_code: 0x53, // S key
            description: String::from("Capture rectangular region"),
        });
        self.bindings.push(KeyBinding {
            action: KeyAction::CaptureFreeform,
            modifier: Modifier::SuperShift,
            key_code: 0x46, // F key
            description: String::from("Capture freeform region"),
        });
        self.bindings.push(KeyBinding {
            action: KeyAction::OpenSnippingToolbar,
            modifier: Modifier::SuperShift,
            key_code: 0x54, // T key
            description: String::from("Open snipping toolbar"),
        });
        self.bindings.push(KeyBinding {
            action: KeyAction::CopyLastScreenshot,
            modifier: Modifier::CtrlShift,
            key_code: 0x43, // C key
            description: String::from("Copy last screenshot to clipboard"),
        });
    }

    pub fn find_action(&self, modifier: Modifier, key_code: u16) -> Option<KeyAction> {
        self.bindings
            .iter()
            .find(|b| b.modifier == modifier && b.key_code == key_code)
            .map(|b| b.action)
    }

    pub fn rebind(&mut self, action: KeyAction, modifier: Modifier, key_code: u16) {
        if let Some(b) = self.bindings.iter_mut().find(|b| b.action == action) {
            b.modifier = modifier;
            b.key_code = key_code;
        }
    }
}

// ── Snipping toolbar ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolbarButton {
    ModeFullScreen,
    ModeWindow,
    ModeRect,
    ModeFreeform,
    ModeScrolling,
    Delay3s,
    Delay5s,
    Delay10s,
    Close,
}

pub struct SnippingToolbar {
    pub visible: bool,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub selected_mode: CaptureMode,
    pub buttons: Vec<ToolbarButton>,
    pub hovered: Option<usize>,
}

impl SnippingToolbar {
    pub fn new() -> Self {
        Self {
            visible: false,
            x: 0,
            y: 0,
            width: 480,
            height: 40,
            selected_mode: CaptureMode::RectangularRegion,
            buttons: vec![
                ToolbarButton::ModeFullScreen,
                ToolbarButton::ModeWindow,
                ToolbarButton::ModeRect,
                ToolbarButton::ModeFreeform,
                ToolbarButton::ModeScrolling,
                ToolbarButton::Delay3s,
                ToolbarButton::Delay5s,
                ToolbarButton::Delay10s,
                ToolbarButton::Close,
            ],
            hovered: None,
        }
    }

    pub fn show(&mut self, screen_w: u32) {
        self.visible = true;
        self.x = ((screen_w - self.width) / 2) as i32;
        self.y = 24;
    }

    pub fn hide(&mut self) {
        self.visible = false;
    }

    pub fn select_button(&mut self, idx: usize) -> Option<CaptureMode> {
        let btn = self.buttons.get(idx)?;
        match btn {
            ToolbarButton::ModeFullScreen => {
                self.selected_mode = CaptureMode::FullScreen;
                Some(CaptureMode::FullScreen)
            }
            ToolbarButton::ModeWindow => {
                self.selected_mode = CaptureMode::ActiveWindow;
                Some(CaptureMode::ActiveWindow)
            }
            ToolbarButton::ModeRect => {
                self.selected_mode = CaptureMode::RectangularRegion;
                Some(CaptureMode::RectangularRegion)
            }
            ToolbarButton::ModeFreeform => {
                self.selected_mode = CaptureMode::FreeformRegion;
                Some(CaptureMode::FreeformRegion)
            }
            ToolbarButton::ModeScrolling => {
                self.selected_mode = CaptureMode::ScrollingCapture;
                Some(CaptureMode::ScrollingCapture)
            }
            ToolbarButton::Delay3s => {
                self.selected_mode = CaptureMode::Delayed3s;
                Some(CaptureMode::Delayed3s)
            }
            ToolbarButton::Delay5s => {
                self.selected_mode = CaptureMode::Delayed5s;
                Some(CaptureMode::Delayed5s)
            }
            ToolbarButton::Delay10s => {
                self.selected_mode = CaptureMode::Delayed10s;
                Some(CaptureMode::Delayed10s)
            }
            ToolbarButton::Close => None,
        }
    }

    pub fn hit_test(&self, px: i32, py: i32) -> Option<usize> {
        if !self.visible {
            return None;
        }
        if py < self.y || py >= self.y + self.height as i32 {
            return None;
        }
        if px < self.x || px >= self.x + self.width as i32 {
            return None;
        }
        let rel_x = (px - self.x) as u32;
        let btn_w = self.width / self.buttons.len() as u32;
        Some((rel_x / btn_w) as usize)
    }
}

// ── Ruler / Protractor overlay ───────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeasurementTool {
    Ruler,
    Protractor,
}

#[derive(Debug, Clone)]
pub struct MeasurementOverlay {
    pub active: bool,
    pub tool: MeasurementTool,
    pub start_x: i32,
    pub start_y: i32,
    pub end_x: i32,
    pub end_y: i32,
    pub pivot_x: i32,
    pub pivot_y: i32,
    pub show_pixels: bool,
    pub show_angle: bool,
}

impl MeasurementOverlay {
    pub fn new() -> Self {
        Self {
            active: false,
            tool: MeasurementTool::Ruler,
            start_x: 0,
            start_y: 0,
            end_x: 0,
            end_y: 0,
            pivot_x: 0,
            pivot_y: 0,
            show_pixels: true,
            show_angle: false,
        }
    }

    pub fn distance_px(&self) -> u32 {
        let dx = (self.end_x - self.start_x) as f64;
        let dy = (self.end_y - self.start_y) as f64;
        f64_sqrt(dx * dx + dy * dy) as u32
    }

    pub fn angle_degrees(&self) -> i32 {
        let dx = (self.end_x - self.pivot_x) as f64;
        let dy = (self.end_y - self.pivot_y) as f64;
        let dx0 = (self.start_x - self.pivot_x) as f64;
        let dy0 = (self.start_y - self.pivot_y) as f64;
        let dot = dx * dx0 + dy * dy0;
        let mag1 = f64_sqrt(dx * dx + dy * dy);
        let mag2 = f64_sqrt(dx0 * dx0 + dy0 * dy0);
        if mag1 < 0.001 || mag2 < 0.001 {
            return 0;
        }
        let cos_angle = (dot / (mag1 * mag2)).clamp(-1.0, 1.0);
        (f64_acos(cos_angle) * 180.0 / core::f64::consts::PI) as i32
    }

    pub fn set_ruler(&mut self, x0: i32, y0: i32, x1: i32, y1: i32) {
        self.tool = MeasurementTool::Ruler;
        self.start_x = x0;
        self.start_y = y0;
        self.end_x = x1;
        self.end_y = y1;
        self.show_pixels = true;
        self.show_angle = false;
    }

    pub fn set_protractor(
        &mut self,
        pivot_x: i32,
        pivot_y: i32,
        x0: i32,
        y0: i32,
        x1: i32,
        y1: i32,
    ) {
        self.tool = MeasurementTool::Protractor;
        self.pivot_x = pivot_x;
        self.pivot_y = pivot_y;
        self.start_x = x0;
        self.start_y = y0;
        self.end_x = x1;
        self.end_y = y1;
        self.show_angle = true;
    }
}

// ── OCR integration point ────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct OcrResult {
    pub text: String,
    pub confidence: u8,
    pub bounding_boxes: Vec<(i32, i32, u32, u32)>,
    pub language: String,
}

pub trait OcrEngine {
    fn extract_text(&self, pixels: &[u8], width: u32, height: u32) -> Option<OcrResult>;
    fn supported_languages(&self) -> Vec<String>;
}

pub struct OcrStub;

impl OcrEngine for OcrStub {
    fn extract_text(&self, _pixels: &[u8], _width: u32, _height: u32) -> Option<OcrResult> {
        None
    }
    fn supported_languages(&self) -> Vec<String> {
        Vec::new()
    }
}

// ── Captured image buffer ────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct CapturedImage {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
    pub format: ImageFormat,
    pub annotations: Vec<Annotation>,
    pub adjustments: ImageAdjustments,
    pub source_mode: CaptureMode,
    pub monitor: MonitorTarget,
    pub timestamp: u64,
}

impl CapturedImage {
    pub fn new(
        w: u32,
        h: u32,
        pixels: Vec<u8>,
        mode: CaptureMode,
        monitor: MonitorTarget,
        ts: u64,
    ) -> Self {
        Self {
            width: w,
            height: h,
            pixels,
            format: ImageFormat::Png,
            annotations: Vec::new(),
            adjustments: ImageAdjustments::new(),
            source_mode: mode,
            monitor,
            timestamp: ts,
        }
    }

    pub fn add_annotation(&mut self, ann: Annotation) {
        self.annotations.push(ann);
    }

    pub fn remove_last_annotation(&mut self) -> Option<Annotation> {
        self.annotations.pop()
    }

    pub fn clear_annotations(&mut self) {
        self.annotations.clear();
    }

    pub fn pixel_at(&self, x: u32, y: u32) -> u32 {
        if x >= self.width || y >= self.height {
            return 0;
        }
        let idx = ((y * self.width + x) * 4) as usize;
        if idx + 3 >= self.pixels.len() {
            return 0;
        }
        let r = self.pixels[idx] as u32;
        let g = self.pixels[idx + 1] as u32;
        let b = self.pixels[idx + 2] as u32;
        let a = self.pixels[idx + 3] as u32;
        (a << 24) | (r << 16) | (g << 8) | b
    }

    pub fn crop(&mut self, x: u32, y: u32, w: u32, h: u32) {
        if x + w > self.width || y + h > self.height {
            return;
        }
        let mut cropped = Vec::with_capacity((w * h * 4) as usize);
        for row in y..y + h {
            let start = ((row * self.width + x) * 4) as usize;
            let end = start + (w * 4) as usize;
            if end <= self.pixels.len() {
                cropped.extend_from_slice(&self.pixels[start..end]);
            }
        }
        self.pixels = cropped;
        self.width = w;
        self.height = h;
    }

    pub fn byte_size(&self) -> usize {
        self.pixels.len()
    }
}

// ── Selection region state ───────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionState {
    Idle,
    Selecting,
    Selected,
    Moving,
    ResizingTopLeft,
    ResizingTopRight,
    ResizingBottomLeft,
    ResizingBottomRight,
}

#[derive(Debug, Clone)]
pub struct SelectionRegion {
    pub state: SelectionState,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub anchor_x: i32,
    pub anchor_y: i32,
    pub freeform_points: Vec<Point>,
}

impl SelectionRegion {
    pub fn new() -> Self {
        Self {
            state: SelectionState::Idle,
            x: 0,
            y: 0,
            width: 0,
            height: 0,
            anchor_x: 0,
            anchor_y: 0,
            freeform_points: Vec::new(),
        }
    }

    pub fn begin(&mut self, px: i32, py: i32) {
        self.state = SelectionState::Selecting;
        self.anchor_x = px;
        self.anchor_y = py;
        self.x = px;
        self.y = py;
        self.width = 0;
        self.height = 0;
        self.freeform_points.clear();
    }

    pub fn update(&mut self, px: i32, py: i32) {
        if self.state != SelectionState::Selecting {
            return;
        }
        self.x = self.anchor_x.min(px);
        self.y = self.anchor_y.min(py);
        self.width = (px - self.anchor_x).unsigned_abs();
        self.height = (py - self.anchor_y).unsigned_abs();
    }

    pub fn finish(&mut self) {
        if self.state == SelectionState::Selecting {
            self.state = SelectionState::Selected;
        }
    }

    pub fn reset(&mut self) {
        self.state = SelectionState::Idle;
        self.width = 0;
        self.height = 0;
        self.freeform_points.clear();
    }

    pub fn add_freeform_point(&mut self, px: i32, py: i32) {
        self.freeform_points.push(Point { x: px, y: py });
    }

    pub fn contains(&self, px: i32, py: i32) -> bool {
        px >= self.x
            && px < self.x + self.width as i32
            && py >= self.y
            && py < self.y + self.height as i32
    }
}

// ── Global screenshot tool ───────────────────────────────────────────────

pub struct ScreenshotTool {
    pub mode: CaptureMode,
    pub monitor: MonitorTarget,
    pub output: OutputSettings,
    pub color_picker: ColorPicker,
    pub annotation_props: AnnotationProperties,
    pub current_annotation_kind: AnnotationKind,
    pub history: ScreenshotHistory,
    pub pinned: Vec<PinnedScreenshot>,
    pub share_opts: ShareOptions,
    pub keybindings: KeyBindingManager,
    pub toolbar: SnippingToolbar,
    pub measurement: MeasurementOverlay,
    pub selection: SelectionRegion,
    pub current_image: Option<CapturedImage>,
    pub next_marker_number: u32,
    pub delay_remaining_ms: u32,
    pub capturing: bool,
    pub annotating: bool,
    pub screen_width: u32,
    pub screen_height: u32,
    pub next_pin_id: u64,
}

impl ScreenshotTool {
    pub fn new() -> Self {
        Self {
            mode: CaptureMode::RectangularRegion,
            monitor: MonitorTarget::Primary,
            output: OutputSettings::new(),
            color_picker: ColorPicker::new(),
            annotation_props: AnnotationProperties::new(),
            current_annotation_kind: AnnotationKind::Arrow,
            history: ScreenshotHistory::new(100),
            pinned: Vec::new(),
            share_opts: ShareOptions::new(),
            keybindings: KeyBindingManager::new(),
            toolbar: SnippingToolbar::new(),
            measurement: MeasurementOverlay::new(),
            selection: SelectionRegion::new(),
            current_image: None,
            next_marker_number: 1,
            delay_remaining_ms: 0,
            capturing: false,
            annotating: false,
            screen_width: 1920,
            screen_height: 1080,
            next_pin_id: 1,
        }
    }

    pub fn start_capture(&mut self, mode: CaptureMode) {
        self.mode = mode;
        self.capturing = true;
        self.selection.reset();
        match mode {
            CaptureMode::Delayed3s => self.delay_remaining_ms = 3000,
            CaptureMode::Delayed5s => self.delay_remaining_ms = 5000,
            CaptureMode::Delayed10s => self.delay_remaining_ms = 10000,
            _ => self.delay_remaining_ms = 0,
        }
    }

    pub fn tick_delay(&mut self, elapsed_ms: u32) -> bool {
        if self.delay_remaining_ms == 0 {
            return true;
        }
        self.delay_remaining_ms = self.delay_remaining_ms.saturating_sub(elapsed_ms);
        self.delay_remaining_ms == 0
    }

    pub fn finish_capture(&mut self, pixels: Vec<u8>, w: u32, h: u32, ts: u64) {
        self.capturing = false;
        self.annotating = true;
        let img = CapturedImage::new(w, h, pixels, self.mode, self.monitor, ts);
        self.current_image = Some(img);
    }

    pub fn add_current_annotation(&mut self, start: Point, end: Point) {
        let ann = Annotation {
            kind: self.current_annotation_kind,
            start,
            end,
            points: Vec::new(),
            text: String::new(),
            number: if self.current_annotation_kind == AnnotationKind::NumberMarker {
                let n = self.next_marker_number;
                self.next_marker_number += 1;
                n
            } else {
                0
            },
            props: self.annotation_props.clone(),
        };
        if let Some(ref mut img) = self.current_image {
            img.add_annotation(ann);
        }
    }

    pub fn undo_annotation(&mut self) {
        if let Some(ref mut img) = self.current_image {
            img.remove_last_annotation();
        }
    }

    pub fn save_current(&mut self, ts: u64) -> Option<String> {
        let img = self.current_image.as_ref()?;
        let path = self.output.generate_filename("");
        let thumb = Vec::new();
        self.history.add(
            &path,
            ts,
            img.width,
            img.height,
            self.output.format,
            img.byte_size() as u64,
            thumb,
            "",
            img.source_mode,
        );
        Some(path)
    }

    pub fn pin_current(&mut self) {
        if let Some(ref img) = self.current_image {
            let pin = PinnedScreenshot::new(
                self.next_pin_id,
                100,
                100,
                img.width,
                img.height,
                img.pixels.clone(),
            );
            self.next_pin_id += 1;
            self.pinned.push(pin);
        }
    }

    pub fn unpin(&mut self, id: u64) {
        self.pinned.retain(|p| p.id != id);
    }

    pub fn cancel(&mut self) {
        self.capturing = false;
        self.annotating = false;
        self.selection.reset();
        self.current_image = None;
    }

    pub fn handle_key(&mut self, modifier: Modifier, key_code: u16) -> Option<KeyAction> {
        let action = self.keybindings.find_action(modifier, key_code)?;
        match action {
            KeyAction::CaptureFullScreen => self.start_capture(CaptureMode::FullScreen),
            KeyAction::CaptureActiveWindow => self.start_capture(CaptureMode::ActiveWindow),
            KeyAction::CaptureRegion => {
                self.toolbar.show(self.screen_width);
                self.start_capture(CaptureMode::RectangularRegion);
            }
            KeyAction::CaptureFreeform => self.start_capture(CaptureMode::FreeformRegion),
            KeyAction::OpenSnippingToolbar => self.toolbar.show(self.screen_width),
            _ => {}
        }
        Some(action)
    }
}

// ── Global instance ──────────────────────────────────────────────────────

static mut SCREENSHOT_TOOL: Option<ScreenshotTool> = None;

pub unsafe fn init() {
    SCREENSHOT_TOOL = Some(ScreenshotTool::new());
}

pub unsafe fn get() -> &'static mut ScreenshotTool {
    SCREENSHOT_TOOL
        .as_mut()
        .expect("screenshot::init() not called")
}
