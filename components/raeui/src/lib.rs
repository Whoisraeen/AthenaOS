//! AthUI v0 — minimal widget toolkit drawn through raegfx::Canvas.
//!
//! Today's surface is intentionally tiny: a Widget trait + Label + Button +
//! Frame (window-with-chrome). Layout is absolute coordinates. Color is
//! ARGB8888. Input events are stubs — there's no input routing in the
//! kernel yet, so widgets just react to programmatic `Event` values.
//!
//! Future work: a proper layout engine (taffy), reactive state, Skia-backed
//! text rendering, theme bundles, animation. This file is the seed of all
//! of that — keep the trait surface stable.

#![no_std]

extern crate alloc;
use alloc::boxed::Box;
use alloc::string::String;
use raegfx::Canvas;

/// Shared design tokens (ADR 0003 / `docs/design/design-language.md`): the
/// single source of truth for spacing, radius, palettes, the accent ramp +
/// `derive_accent`, elevation, type ramp, motion, and glass. Re-exported here
/// so the design-language handoff name `raeui::tokens` resolves; apps should
/// consume these instead of redefining a private `const ACCENT` / palette.
pub use rae_tokens as tokens;

pub mod accessibility;
pub mod animation;
pub mod binding;
pub mod containers;
pub mod data_display;
pub mod feedback;
pub mod i18n;
pub mod inputs;
pub mod layout;
pub mod navigation;
pub mod rich_text;
pub mod text;
pub mod tree;
#[cfg(feature = "gpu_userspace")]
pub mod wgpu_canvas;

// ── Theme ────────────────────────────────────────────────────────────────

/// AthenaOS default palette in ARGB8888. Tweak here to re-skin the whole UI.
pub mod theme {
    pub const WINDOW_BG: u32 = 0xFF_1A_1A_22; // deep blue-black
    pub const CHROME_BG: u32 = 0xFF_0A_0E_1A; // a notch darker than the body
    pub const BORDER: u32 = 0xFF_4E_9C_FF; // electric blue
    pub const TEXT_FG: u32 = 0xFF_E0_E0_FF; // near-white with blue tint
    pub const TITLE_FG: u32 = 0xFF_FF_FF_FF; // pure white for chrome text
    pub const BUTTON_BG: u32 = 0xFF_33_33_55; // muted
    pub const BUTTON_HOT: u32 = 0xFF_FF_2E_88; // magenta plasma
    pub const BUTTON_TEXT: u32 = 0xFF_FF_FF_FF;
}

const TITLE_BAR_H: usize = 24;
const PADDING: usize = 8;

// ── Events ───────────────────────────────────────────────────────────────

pub enum Event {
    None,
    KeyPress(u8),
    MouseClick { x: i32, y: i32 },
    MouseMove { dx: i32, dy: i32, buttons: u8 },
    MouseDown { x: i32, y: i32, button: u8 },
    MouseUp { x: i32, y: i32, button: u8 },
}

// ── Widget trait ─────────────────────────────────────────────────────────

pub trait Widget {
    fn render(&self, canvas: &mut Canvas);
    fn on_event(&mut self, event: &Event);
}

// ── Label ────────────────────────────────────────────────────────────────

pub struct Label {
    pub text: String,
    pub x: usize,
    pub y: usize,
    pub fg: u32,
}

impl Label {
    pub fn new(text: &str, x: usize, y: usize) -> Self {
        Self {
            text: String::from(text),
            x,
            y,
            fg: theme::TEXT_FG,
        }
    }
}

impl Widget for Label {
    fn render(&self, canvas: &mut Canvas) {
        canvas.draw_text(self.x, self.y, &self.text, self.fg, None);
    }
    fn on_event(&mut self, _event: &Event) {}
}

// ── Button (with text) ───────────────────────────────────────────────────

pub struct Button {
    pub text: String,
    pub x: usize,
    pub y: usize,
    pub width: usize,
    pub height: usize,
    pub pressed: bool,
}

impl Button {
    pub fn new(text: &str, x: usize, y: usize, width: usize, height: usize) -> Self {
        Self {
            text: String::from(text),
            x,
            y,
            width,
            height,
            pressed: false,
        }
    }
}

impl Widget for Button {
    fn render(&self, canvas: &mut Canvas) {
        let bg = if self.pressed {
            theme::BUTTON_HOT
        } else {
            theme::BUTTON_BG
        };
        canvas.fill_rect(self.x, self.y, self.width, self.height, bg);
        canvas.draw_rect_outline(self.x, self.y, self.width, self.height, theme::BORDER);

        // Center the text horizontally; align vertically to ~middle.
        let text_w = self.text.chars().count() * 8;
        let tx = self.x + self.width.saturating_sub(text_w) / 2;
        let ty = self.y + self.height.saturating_sub(8) / 2;
        canvas.draw_text(tx, ty, &self.text, theme::BUTTON_TEXT, None);
    }
    fn on_event(&mut self, event: &Event) {
        match event {
            Event::KeyPress(_) => self.pressed = !self.pressed,
            Event::MouseClick { x, y } => {
                let in_x = *x >= self.x as i32 && *x < (self.x + self.width) as i32;
                let in_y = *y >= self.y as i32 && *y < (self.y + self.height) as i32;
                if in_x && in_y {
                    self.pressed = !self.pressed;
                }
            }
            _ => {}
        }
    }
}

// ── Frame (a window with chrome) ─────────────────────────────────────────

/// A Frame draws the window-chrome that wraps a body widget:
///   ┌──────────────── title ──────────────────┐
///   │ Hello AthenaOS                         × │
///   ├─────────────────────────────────────────┤
///   │                                         │
///   │  (body content)                         │
///   │                                         │
///   └─────────────────────────────────────────┘
///
/// Coordinates are surface-local (0,0 = surface top-left).
pub struct Frame {
    pub title: String,
    pub width: usize,
    pub height: usize,
    pub body: Box<dyn Widget>,
}

impl Frame {
    pub fn new(title: &str, width: usize, height: usize, body: Box<dyn Widget>) -> Self {
        Self {
            title: String::from(title),
            width,
            height,
            body,
        }
    }
}

impl Widget for Frame {
    fn render(&self, canvas: &mut Canvas) {
        // Body fill
        canvas.fill_rect(0, 0, self.width, self.height, theme::WINDOW_BG);
        // Title bar
        canvas.fill_rect(0, 0, self.width, TITLE_BAR_H, theme::CHROME_BG);
        // Title text — 8 px left-padded, vertically centered in the title bar
        canvas.draw_text(
            PADDING,
            (TITLE_BAR_H - 8) / 2,
            &self.title,
            theme::TITLE_FG,
            None,
        );
        // Close-button glyph in the top-right corner — just an "×" for now
        canvas.draw_text(
            self.width.saturating_sub(PADDING + 8),
            (TITLE_BAR_H - 8) / 2,
            "x",
            theme::BUTTON_HOT,
            None,
        );
        // Title-bar bottom divider
        for x in 0..self.width {
            canvas.draw_pixel(x, TITLE_BAR_H - 1, theme::BORDER);
        }
        // Frame outline
        canvas.draw_rect_outline(0, 0, self.width, self.height, theme::BORDER);

        // Body content. Body draws at its own coordinates relative to the
        // surface — the body widget is expected to position itself within
        // the client area (below TITLE_BAR_H).
        self.body.render(canvas);
    }
    fn on_event(&mut self, event: &Event) {
        // Title-bar gets first refusal in a real WM; today we just forward
        // every event into the body so the demo can toggle.
        self.body.on_event(event);
    }
}

/// Y-coordinate at which a Frame's body content should start (below chrome).
pub const fn body_y_start() -> usize {
    TITLE_BAR_H + PADDING
}

// ── Declarative Theme Engine / Vibe Mode ─────────────────────────────────

/// A complete colour palette that skins every AthUI primitive.
#[derive(Clone, Copy, Debug)]
pub struct Theme {
    pub window_bg: u32,
    pub chrome_bg: u32,
    pub border: u32,
    pub text_fg: u32,
    pub title_fg: u32,
    pub button_bg: u32,
    pub button_hot: u32,
    pub button_text: u32,
}

/// Preset "vibes" — one-click personality changes for the whole OS.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VibeMode {
    Default,
    CyberpunkNight,
    StudioGhibli,
    Bauhaus,
    NeoNoir,
}

/// Returns the `Theme` that matches the current hardcoded `theme::*` constants.
pub const fn default_theme() -> Theme {
    Theme {
        window_bg: theme::WINDOW_BG,
        chrome_bg: theme::CHROME_BG,
        border: theme::BORDER,
        text_fg: theme::TEXT_FG,
        title_fg: theme::TITLE_FG,
        button_bg: theme::BUTTON_BG,
        button_hot: theme::BUTTON_HOT,
        button_text: theme::BUTTON_TEXT,
    }
}

/// Map a `VibeMode` to its curated colour palette.
pub fn theme_for_vibe(mode: VibeMode) -> Theme {
    match mode {
        VibeMode::Default => default_theme(),

        VibeMode::CyberpunkNight => Theme {
            window_bg: 0xFF_0D_0D_1A,
            chrome_bg: 0xFF_05_05_10,
            border: 0xFF_00_FF_E5, // cyan neon
            text_fg: 0xFF_C0_FF_F0,
            title_fg: 0xFF_00_FF_E5,
            button_bg: 0xFF_1A_00_33,
            button_hot: 0xFF_FF_00_7F, // hot pink
            button_text: 0xFF_FF_FF_FF,
        },

        VibeMode::StudioGhibli => Theme {
            window_bg: 0xFF_F5_F0_E1, // warm parchment
            chrome_bg: 0xFF_6B_8F_71, // forest green
            border: 0xFF_8B_6F_47,    // earthy brown
            text_fg: 0xFF_3A_3A_2E,
            title_fg: 0xFF_FF_FF_F0,
            button_bg: 0xFF_A8_C6_8F,  // soft moss
            button_hot: 0xFF_E8_6B_5A, // sunset red
            button_text: 0xFF_2E_2E_22,
        },

        VibeMode::Bauhaus => Theme {
            window_bg: 0xFF_F2_F2_F2, // off-white
            chrome_bg: 0xFF_1A_1A_1A, // near-black
            border: 0xFF_E3_1B_23,    // Bauhaus red
            text_fg: 0xFF_1A_1A_1A,
            title_fg: 0xFF_FF_FF_FF,
            button_bg: 0xFF_00_56_A4,  // primary blue
            button_hot: 0xFF_F7_C6_00, // primary yellow
            button_text: 0xFF_1A_1A_1A,
        },

        VibeMode::NeoNoir => Theme {
            window_bg: 0xFF_10_10_10, // almost black
            chrome_bg: 0xFF_08_08_08,
            border: 0xFF_44_44_44, // muted grey
            text_fg: 0xFF_B0_B0_B0,
            title_fg: 0xFF_D0_D0_D0,
            button_bg: 0xFF_22_22_22,
            button_hot: 0xFF_CC_00_00, // deep red accent
            button_text: 0xFF_E0_E0_E0,
        },
    }
}

// ── Backwards-compat re-export ───────────────────────────────────────────
//
// The earlier raeui shape had a `Window { root, canvas }` wrapper. Keep it
// around so older snippets still compile; new code should drive Frames
// directly into a surface-backed Canvas.
pub struct Window {
    pub root: Box<dyn Widget>,
    pub canvas: Canvas,
}

impl Window {
    pub fn new(root: Box<dyn Widget>, canvas: Canvas) -> Self {
        Self { root, canvas }
    }
    pub fn render(&mut self) {
        self.root.render(&mut self.canvas);
    }
    pub fn handle_event(&mut self, event: &Event) {
        self.root.on_event(event);
        self.render();
    }
}

// ── Layout Engine (FlexBox-style) ────────────────────────────────────────

use alloc::vec::Vec;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Row,
    Column,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Start,
    Center,
    End,
    Stretch,
    SpaceBetween,
    SpaceAround,
}

#[derive(Clone, Copy, Debug)]
pub struct LayoutConstraints {
    pub min_width: usize,
    pub max_width: usize,
    pub min_height: usize,
    pub max_height: usize,
}

impl LayoutConstraints {
    pub fn new(min_width: usize, max_width: usize, min_height: usize, max_height: usize) -> Self {
        Self {
            min_width,
            max_width,
            min_height,
            max_height,
        }
    }
    pub fn unconstrained() -> Self {
        Self {
            min_width: 0,
            max_width: usize::MAX,
            min_height: 0,
            max_height: usize::MAX,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct LayoutResult {
    pub x: usize,
    pub y: usize,
    pub width: usize,
    pub height: usize,
}

pub struct FlexContainer {
    pub direction: Direction,
    pub main_align: Align,
    pub cross_align: Align,
    pub gap: usize,
    pub padding: usize,
    pub children: Vec<Box<dyn Widget>>,
    pub bg_color: u32,
    pub x: usize,
    pub y: usize,
    pub width: usize,
    pub height: usize,
}

impl FlexContainer {
    pub fn new(direction: Direction, x: usize, y: usize, width: usize, height: usize) -> Self {
        Self {
            direction,
            main_align: Align::Start,
            cross_align: Align::Start,
            gap: 0,
            padding: 0,
            children: Vec::new(),
            bg_color: 0,
            x,
            y,
            width,
            height,
        }
    }

    pub fn add_child(&mut self, child: Box<dyn Widget>) {
        self.children.push(child);
    }
}

impl Widget for FlexContainer {
    fn render(&self, canvas: &mut Canvas) {
        if self.bg_color != 0 {
            canvas.fill_rect(self.x, self.y, self.width, self.height, self.bg_color);
        }

        let count = self.children.len();
        if count == 0 {
            return;
        }

        let inner_w = self.width.saturating_sub(self.padding * 2);
        let inner_h = self.height.saturating_sub(self.padding * 2);
        let total_gap = self.gap * count.saturating_sub(1);

        match self.direction {
            Direction::Row => {
                let child_w = if count > 0 {
                    inner_w.saturating_sub(total_gap) / count
                } else {
                    0
                };
                let mut cx = self.x + self.padding;

                let extra_space = inner_w.saturating_sub(child_w * count + total_gap);
                let (initial_offset, between) = match self.main_align {
                    Align::Center => (extra_space / 2, 0),
                    Align::End => (extra_space, 0),
                    Align::SpaceBetween if count > 1 => (0, extra_space / (count - 1)),
                    Align::SpaceAround if count > 0 => {
                        let sp = extra_space / (count * 2);
                        (sp, sp * 2)
                    }
                    _ => (0, 0),
                };
                cx += initial_offset;

                for (_i, child) in self.children.iter().enumerate() {
                    child.render(canvas);
                    cx += child_w + self.gap + between;
                    let _ = cx; // position tracking
                }
            }
            Direction::Column => {
                let child_h = if count > 0 {
                    inner_h.saturating_sub(total_gap) / count
                } else {
                    0
                };
                let mut cy = self.y + self.padding;

                let extra_space = inner_h.saturating_sub(child_h * count + total_gap);
                let (initial_offset, between) = match self.main_align {
                    Align::Center => (extra_space / 2, 0),
                    Align::End => (extra_space, 0),
                    Align::SpaceBetween if count > 1 => (0, extra_space / (count - 1)),
                    Align::SpaceAround if count > 0 => {
                        let sp = extra_space / (count * 2);
                        (sp, sp * 2)
                    }
                    _ => (0, 0),
                };
                cy += initial_offset;

                for (_i, child) in self.children.iter().enumerate() {
                    child.render(canvas);
                    cy += child_h + self.gap + between;
                    let _ = cy;
                }
            }
        }
    }

    fn on_event(&mut self, event: &Event) {
        for child in self.children.iter_mut() {
            child.on_event(event);
        }
    }
}

// ── Text Input Widget ────────────────────────────────────────────────────

pub struct TextInput {
    pub text: String,
    pub cursor_pos: usize,
    pub x: usize,
    pub y: usize,
    pub width: usize,
    pub focused: bool,
    pub placeholder: String,
}

impl TextInput {
    pub fn new(x: usize, y: usize, width: usize, placeholder: &str) -> Self {
        Self {
            text: String::new(),
            cursor_pos: 0,
            x,
            y,
            width,
            focused: false,
            placeholder: String::from(placeholder),
        }
    }

    pub fn insert_char(&mut self, ch: char) {
        if self.cursor_pos <= self.text.len() {
            self.text.insert(self.cursor_pos, ch);
            self.cursor_pos += 1;
        }
    }

    pub fn delete_char(&mut self) {
        if self.cursor_pos > 0 {
            self.cursor_pos -= 1;
            self.text.remove(self.cursor_pos);
        }
    }

    pub fn move_cursor_left(&mut self) {
        if self.cursor_pos > 0 {
            self.cursor_pos -= 1;
        }
    }

    pub fn move_cursor_right(&mut self) {
        if self.cursor_pos < self.text.len() {
            self.cursor_pos += 1;
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }
}

impl Widget for TextInput {
    fn render(&self, canvas: &mut Canvas) {
        let height = 20;
        let bg = if self.focused {
            0xFF_2A_2A_3A
        } else {
            0xFF_1E_1E_2E
        };
        canvas.fill_rect(self.x, self.y, self.width, height, bg);
        canvas.draw_rect_outline(self.x, self.y, self.width, height, theme::BORDER);

        let text_y = self.y + (height - 8) / 2;
        let text_x = self.x + 4;

        if self.text.is_empty() && !self.focused {
            canvas.draw_text(text_x, text_y, &self.placeholder, 0xFF_66_66_88, None);
        } else {
            canvas.draw_text(text_x, text_y, &self.text, theme::TEXT_FG, None);
        }

        if self.focused {
            let cursor_x = text_x + self.cursor_pos * 8;
            for dy in 0..12 {
                canvas.draw_pixel(cursor_x, self.y + 4 + dy, theme::BORDER);
            }
        }
    }

    fn on_event(&mut self, event: &Event) {
        match event {
            Event::KeyPress(ch) => {
                if self.focused {
                    match *ch {
                        8 => self.delete_char(), // backspace
                        0 => {}                  // null
                        c => self.insert_char(c as char),
                    }
                }
            }
            Event::MouseClick { x, y } => {
                let in_x = *x >= self.x as i32 && *x < (self.x + self.width) as i32;
                let in_y = *y >= self.y as i32 && *y < (self.y + 20) as i32;
                self.focused = in_x && in_y;
                if self.focused {
                    let rel_x = (*x as usize).saturating_sub(self.x + 4);
                    self.cursor_pos = (rel_x / 8).min(self.text.len());
                }
            }
            _ => {}
        }
    }
}

// ── Checkbox Widget ──────────────────────────────────────────────────────

pub struct Checkbox {
    pub label: String,
    pub checked: bool,
    pub x: usize,
    pub y: usize,
}

impl Checkbox {
    pub fn new(label: &str, x: usize, y: usize) -> Self {
        Self {
            label: String::from(label),
            checked: false,
            x,
            y,
        }
    }
}

impl Widget for Checkbox {
    fn render(&self, canvas: &mut Canvas) {
        let box_size = 14;
        let border_col = if self.checked {
            theme::BUTTON_HOT
        } else {
            theme::BORDER
        };
        canvas.fill_rect(self.x, self.y, box_size, box_size, 0xFF_1E_1E_2E);
        canvas.draw_rect_outline(self.x, self.y, box_size, box_size, border_col);

        if self.checked {
            // Draw a simple checkmark (two diagonal lines)
            for i in 0..4 {
                canvas.draw_pixel(self.x + 3 + i, self.y + 6 + i, theme::BUTTON_HOT);
            }
            for i in 0..6 {
                canvas.draw_pixel(self.x + 6 + i, self.y + 9 - i, theme::BUTTON_HOT);
            }
        }

        let label_x = self.x + box_size + 6;
        let label_y = self.y + (box_size - 8) / 2;
        canvas.draw_text(label_x, label_y, &self.label, theme::TEXT_FG, None);
    }

    fn on_event(&mut self, event: &Event) {
        if let Event::MouseClick { x, y } = event {
            let box_size = 14;
            let total_w = box_size + 6 + self.label.len() * 8;
            let in_x = *x >= self.x as i32 && *x < (self.x + total_w) as i32;
            let in_y = *y >= self.y as i32 && *y < (self.y + box_size) as i32;
            if in_x && in_y {
                self.checked = !self.checked;
            }
        }
    }
}

// ── Slider Widget ────────────────────────────────────────────────────────

pub struct Slider {
    pub x: usize,
    pub y: usize,
    pub width: usize,
    pub value: f32,
    pub dragging: bool,
    pub label: String,
}

impl Slider {
    pub fn new(label: &str, x: usize, y: usize, width: usize) -> Self {
        Self {
            x,
            y,
            width,
            value: 0.0,
            dragging: false,
            label: String::from(label),
        }
    }

    pub fn set_value(&mut self, v: f32) {
        self.value = if v < 0.0 {
            0.0
        } else if v > 1.0 {
            1.0
        } else {
            v
        };
    }
}

impl Widget for Slider {
    fn render(&self, canvas: &mut Canvas) {
        let track_h = 4;
        let knob_r = 6;
        let track_y = self.y + knob_r;
        let label_y = self.y.saturating_sub(12);

        // Label and value
        canvas.draw_text(self.x, label_y, &self.label, theme::TEXT_FG, None);
        let pct = (self.value * 100.0) as usize;
        let mut val_buf = [0u8; 4];
        let val_str = format_pct(pct, &mut val_buf);
        let val_x = self.x + self.width - val_str.len() * 8;
        canvas.draw_text(val_x, label_y, val_str, theme::TEXT_FG, None);

        // Track background
        canvas.fill_rect(self.x, track_y, self.width, track_h, 0xFF_33_33_44);

        // Filled portion
        let fill_w = ((self.width as f32) * self.value) as usize;
        canvas.fill_rect(self.x, track_y, fill_w, track_h, theme::BORDER);

        // Knob
        let knob_x = self.x + fill_w;
        let knob_y = track_y + track_h / 2;
        for dy in 0..(knob_r * 2) {
            for dx in 0..(knob_r * 2) {
                let rx = dx as i32 - knob_r as i32;
                let ry = dy as i32 - knob_r as i32;
                if rx * rx + ry * ry <= (knob_r as i32) * (knob_r as i32) {
                    let px = (knob_x as i32 + rx) as usize;
                    let py = (knob_y as i32 + ry) as usize;
                    canvas.draw_pixel(px, py, theme::BUTTON_HOT);
                }
            }
        }
    }

    fn on_event(&mut self, event: &Event) {
        match event {
            Event::MouseDown { x, y, .. } => {
                let track_y = self.y as i32;
                let in_x = *x >= self.x as i32 && *x < (self.x + self.width) as i32;
                let in_y = *y >= track_y && *y < track_y + 20;
                if in_x && in_y {
                    self.dragging = true;
                    let rel = (*x as usize).saturating_sub(self.x);
                    self.set_value(rel as f32 / self.width as f32);
                }
            }
            Event::MouseUp { .. } => {
                self.dragging = false;
            }
            Event::MouseMove { dx, .. } => {
                if self.dragging {
                    let delta = *dx as f32 / self.width as f32;
                    self.set_value(self.value + delta);
                }
            }
            Event::MouseClick { x, .. } => {
                let in_x = *x >= self.x as i32 && *x < (self.x + self.width) as i32;
                if in_x {
                    let rel = (*x as usize).saturating_sub(self.x);
                    self.set_value(rel as f32 / self.width as f32);
                }
            }
            _ => {}
        }
    }
}

fn format_pct(pct: usize, buf: &mut [u8; 4]) -> &str {
    let d2 = (pct / 100) as u8;
    let d1 = ((pct / 10) % 10) as u8;
    let d0 = (pct % 10) as u8;
    if pct >= 100 {
        buf[0] = b'0' + d2;
        buf[1] = b'0' + d1;
        buf[2] = b'0' + d0;
        buf[3] = b'%';
        // SAFETY: buf only contains ASCII
        unsafe { core::str::from_utf8_unchecked(&buf[..4]) }
    } else if pct >= 10 {
        buf[0] = b'0' + d1;
        buf[1] = b'0' + d0;
        buf[2] = b'%';
        unsafe { core::str::from_utf8_unchecked(&buf[..3]) }
    } else {
        buf[0] = b'0' + d0;
        buf[1] = b'%';
        unsafe { core::str::from_utf8_unchecked(&buf[..2]) }
    }
}

// ── ListView Widget ──────────────────────────────────────────────────────

pub struct ListItem {
    pub text: String,
    pub selected: bool,
}

impl ListItem {
    pub fn new(text: &str) -> Self {
        Self {
            text: String::from(text),
            selected: false,
        }
    }
}

pub struct ListView {
    pub x: usize,
    pub y: usize,
    pub width: usize,
    pub height: usize,
    pub items: Vec<ListItem>,
    pub selected_index: Option<usize>,
    pub scroll_offset: usize,
    pub item_height: usize,
}

impl ListView {
    pub fn new(x: usize, y: usize, width: usize, height: usize) -> Self {
        Self {
            x,
            y,
            width,
            height,
            items: Vec::new(),
            selected_index: None,
            scroll_offset: 0,
            item_height: 20,
        }
    }

    pub fn add_item(&mut self, text: &str) {
        self.items.push(ListItem::new(text));
    }

    pub fn visible_count(&self) -> usize {
        if self.item_height == 0 {
            return 0;
        }
        self.height / self.item_height
    }
}

impl Widget for ListView {
    fn render(&self, canvas: &mut Canvas) {
        canvas.fill_rect(self.x, self.y, self.width, self.height, 0xFF_1A_1A_2A);
        canvas.draw_rect_outline(self.x, self.y, self.width, self.height, theme::BORDER);

        let visible = self.visible_count();
        let end = (self.scroll_offset + visible).min(self.items.len());

        for i in self.scroll_offset..end {
            let rel_idx = i - self.scroll_offset;
            let item_y = self.y + rel_idx * self.item_height;

            let is_selected = self.selected_index == Some(i);
            if is_selected {
                canvas.fill_rect(
                    self.x + 1,
                    item_y,
                    self.width - 2,
                    self.item_height,
                    theme::BUTTON_BG,
                );
            }

            let text_y = item_y + (self.item_height - 8) / 2;
            let fg = if is_selected {
                theme::TITLE_FG
            } else {
                theme::TEXT_FG
            };
            canvas.draw_text(self.x + 6, text_y, &self.items[i].text, fg, None);
        }
    }

    fn on_event(&mut self, event: &Event) {
        match event {
            Event::MouseClick { x, y } => {
                let in_x = *x >= self.x as i32 && *x < (self.x + self.width) as i32;
                let in_y = *y >= self.y as i32 && *y < (self.y + self.height) as i32;
                if in_x && in_y {
                    let rel_y = (*y as usize).saturating_sub(self.y);
                    let idx = self.scroll_offset + rel_y / self.item_height;
                    if idx < self.items.len() {
                        if let Some(prev) = self.selected_index {
                            if prev < self.items.len() {
                                self.items[prev].selected = false;
                            }
                        }
                        self.selected_index = Some(idx);
                        self.items[idx].selected = true;
                    }
                }
            }
            Event::KeyPress(k) => {
                match *k {
                    // Up arrow approximation
                    72 => {
                        if let Some(idx) = self.selected_index {
                            if idx > 0 {
                                self.items[idx].selected = false;
                                self.selected_index = Some(idx - 1);
                                self.items[idx - 1].selected = true;
                                if idx - 1 < self.scroll_offset {
                                    self.scroll_offset = idx - 1;
                                }
                            }
                        }
                    }
                    // Down arrow approximation
                    80 => {
                        let next = match self.selected_index {
                            Some(idx) => idx + 1,
                            None => 0,
                        };
                        if next < self.items.len() {
                            if let Some(prev) = self.selected_index {
                                self.items[prev].selected = false;
                            }
                            self.selected_index = Some(next);
                            self.items[next].selected = true;
                            let visible = self.visible_count();
                            if next >= self.scroll_offset + visible {
                                self.scroll_offset = next - visible + 1;
                            }
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
}

// ── ProgressBar Widget ───────────────────────────────────────────────────

pub struct ProgressBar {
    pub x: usize,
    pub y: usize,
    pub width: usize,
    pub progress: f32,
    pub label: String,
    pub show_percent: bool,
}

impl ProgressBar {
    pub fn new(label: &str, x: usize, y: usize, width: usize) -> Self {
        Self {
            x,
            y,
            width,
            progress: 0.0,
            label: String::from(label),
            show_percent: true,
        }
    }

    pub fn set_progress(&mut self, p: f32) {
        self.progress = if p < 0.0 {
            0.0
        } else if p > 1.0 {
            1.0
        } else {
            p
        };
    }
}

impl Widget for ProgressBar {
    fn render(&self, canvas: &mut Canvas) {
        let bar_h = 12;
        let label_y = self.y;
        let bar_y = self.y + 14;

        canvas.draw_text(self.x, label_y, &self.label, theme::TEXT_FG, None);

        if self.show_percent {
            let pct = (self.progress * 100.0) as usize;
            let mut buf = [0u8; 4];
            let pct_str = format_pct(pct, &mut buf);
            let px = self.x + self.width - pct_str.len() * 8;
            canvas.draw_text(px, label_y, pct_str, theme::TEXT_FG, None);
        }

        // Background track
        canvas.fill_rect(self.x, bar_y, self.width, bar_h, 0xFF_2A_2A_3A);
        canvas.draw_rect_outline(self.x, bar_y, self.width, bar_h, theme::BORDER);

        // Filled portion
        let fill_w = ((self.width as f32) * self.progress) as usize;
        if fill_w > 0 {
            canvas.fill_rect(self.x, bar_y, fill_w, bar_h, theme::BORDER);
        }
    }

    fn on_event(&mut self, _event: &Event) {}
}

// ── TabView Widget ───────────────────────────────────────────────────────

pub struct Tab {
    pub label: String,
    pub content: Box<dyn Widget>,
}

impl Tab {
    pub fn new(label: &str, content: Box<dyn Widget>) -> Self {
        Self {
            label: String::from(label),
            content,
        }
    }
}

pub struct TabView {
    pub x: usize,
    pub y: usize,
    pub width: usize,
    pub height: usize,
    pub tabs: Vec<Tab>,
    pub active_tab: usize,
}

impl TabView {
    pub fn new(x: usize, y: usize, width: usize, height: usize) -> Self {
        Self {
            x,
            y,
            width,
            height,
            tabs: Vec::new(),
            active_tab: 0,
        }
    }

    pub fn add_tab(&mut self, label: &str, content: Box<dyn Widget>) {
        self.tabs.push(Tab::new(label, content));
    }
}

impl Widget for TabView {
    fn render(&self, canvas: &mut Canvas) {
        let tab_h = 24;

        // Tab header bar
        canvas.fill_rect(self.x, self.y, self.width, tab_h, theme::CHROME_BG);

        let mut tx = self.x;
        for (i, tab) in self.tabs.iter().enumerate() {
            let tab_w = tab.label.len() * 8 + 16;
            let bg = if i == self.active_tab {
                theme::WINDOW_BG
            } else {
                theme::CHROME_BG
            };
            canvas.fill_rect(tx, self.y, tab_w, tab_h, bg);
            canvas.draw_rect_outline(tx, self.y, tab_w, tab_h, theme::BORDER);

            let text_x = tx + 8;
            let text_y = self.y + (tab_h - 8) / 2;
            let fg = if i == self.active_tab {
                theme::TITLE_FG
            } else {
                theme::TEXT_FG
            };
            canvas.draw_text(text_x, text_y, &tab.label, fg, None);
            tx += tab_w;
        }

        // Content area
        let content_y = self.y + tab_h;
        let content_h = self.height.saturating_sub(tab_h);
        canvas.fill_rect(self.x, content_y, self.width, content_h, theme::WINDOW_BG);
        canvas.draw_rect_outline(self.x, content_y, self.width, content_h, theme::BORDER);

        if self.active_tab < self.tabs.len() {
            self.tabs[self.active_tab].content.render(canvas);
        }
    }

    fn on_event(&mut self, event: &Event) {
        if let Event::MouseClick { x, y } = event {
            let tab_h = 24;
            let in_header = *y >= self.y as i32 && *y < (self.y + tab_h) as i32;
            if in_header {
                let mut tx = self.x as i32;
                for (i, tab) in self.tabs.iter().enumerate() {
                    let tab_w = (tab.label.len() * 8 + 16) as i32;
                    if *x >= tx && *x < tx + tab_w {
                        self.active_tab = i;
                        return;
                    }
                    tx += tab_w;
                }
            }
        }

        if self.active_tab < self.tabs.len() {
            self.tabs[self.active_tab].content.on_event(event);
        }
    }
}

// ── Animation System ─────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Easing {
    Linear,
    EaseIn,
    EaseOut,
    EaseInOut,
}

pub fn ease(t: f32, easing: Easing) -> f32 {
    let t = if t < 0.0 {
        0.0
    } else if t > 1.0 {
        1.0
    } else {
        t
    };
    match easing {
        Easing::Linear => t,
        Easing::EaseIn => t * t,
        Easing::EaseOut => 1.0 - (1.0 - t) * (1.0 - t),
        Easing::EaseInOut => {
            if t < 0.5 {
                2.0 * t * t
            } else {
                1.0 - (-2.0 * t + 2.0) * (-2.0 * t + 2.0) / 2.0
            }
        }
    }
}

pub struct Animation {
    pub start_value: f32,
    pub end_value: f32,
    pub duration_ms: u64,
    pub elapsed_ms: u64,
    pub easing: Easing,
    pub looping: bool,
}

impl Animation {
    pub fn new(start_value: f32, end_value: f32, duration_ms: u64, easing: Easing) -> Self {
        Self {
            start_value,
            end_value,
            duration_ms,
            elapsed_ms: 0,
            easing,
            looping: false,
        }
    }

    pub fn tick(&mut self, delta_ms: u64) {
        self.elapsed_ms += delta_ms;
        if self.looping && self.elapsed_ms >= self.duration_ms {
            self.elapsed_ms %= self.duration_ms;
        }
    }

    pub fn value(&self) -> f32 {
        if self.duration_ms == 0 {
            return self.end_value;
        }
        let t = if self.elapsed_ms >= self.duration_ms {
            1.0
        } else {
            self.elapsed_ms as f32 / self.duration_ms as f32
        };
        let eased = ease(t, self.easing);
        self.start_value + (self.end_value - self.start_value) * eased
    }

    pub fn is_finished(&self) -> bool {
        !self.looping && self.elapsed_ms >= self.duration_ms
    }

    pub fn reset(&mut self) {
        self.elapsed_ms = 0;
    }
}

// ── Glassmorphism Helpers ────────────────────────────────────────────────

pub fn blend_colors(fg: u32, bg: u32, alpha: u8) -> u32 {
    let a = alpha as u32;
    let inv_a = 255 - a;

    let fg_r = (fg >> 16) & 0xFF;
    let fg_g = (fg >> 8) & 0xFF;
    let fg_b = fg & 0xFF;

    let bg_r = (bg >> 16) & 0xFF;
    let bg_g = (bg >> 8) & 0xFF;
    let bg_b = bg & 0xFF;

    let r = (fg_r * a + bg_r * inv_a) / 255;
    let g = (fg_g * a + bg_g * inv_a) / 255;
    let b = (fg_b * a + bg_b * inv_a) / 255;

    0xFF_00_00_00 | (r << 16) | (g << 8) | b
}

pub fn apply_tint(color: u32, tint: u32, strength: u8) -> u32 {
    blend_colors(tint, color, strength)
}

pub struct GlassPanel {
    pub x: usize,
    pub y: usize,
    pub width: usize,
    pub height: usize,
    pub bg_color: u32,
    pub opacity: u8,
    pub border_color: u32,
    pub border_radius: usize,
    pub child: Option<Box<dyn Widget>>,
}

impl GlassPanel {
    pub fn new(x: usize, y: usize, width: usize, height: usize) -> Self {
        Self {
            x,
            y,
            width,
            height,
            bg_color: 0xFF_FF_FF_FF,
            opacity: 80,
            border_color: theme::BORDER,
            border_radius: 0,
            child: None,
        }
    }
}

impl Widget for GlassPanel {
    fn render(&self, canvas: &mut Canvas) {
        let blended_bg = blend_colors(self.bg_color, theme::WINDOW_BG, self.opacity);

        if self.border_radius == 0 {
            canvas.fill_rect(self.x, self.y, self.width, self.height, blended_bg);
            canvas.draw_rect_outline(self.x, self.y, self.width, self.height, self.border_color);
        } else {
            // Approximate rounded rect: fill center, draw border
            let r = self.border_radius.min(self.width / 2).min(self.height / 2);
            // Fill main body (simplified — full corners would need circle rasterization)
            canvas.fill_rect(
                self.x + r,
                self.y,
                self.width - 2 * r,
                self.height,
                blended_bg,
            );
            canvas.fill_rect(
                self.x,
                self.y + r,
                self.width,
                self.height - 2 * r,
                blended_bg,
            );
            // Corner fills (quarter circles)
            for cy in 0..r {
                for cx in 0..r {
                    let dx = r - 1 - cx;
                    let dy = r - 1 - cy;
                    if dx * dx + dy * dy <= r * r {
                        // Top-left
                        canvas.draw_pixel(self.x + cx, self.y + cy, blended_bg);
                        // Top-right
                        canvas.draw_pixel(self.x + self.width - 1 - cx, self.y + cy, blended_bg);
                        // Bottom-left
                        canvas.draw_pixel(self.x + cx, self.y + self.height - 1 - cy, blended_bg);
                        // Bottom-right
                        canvas.draw_pixel(
                            self.x + self.width - 1 - cx,
                            self.y + self.height - 1 - cy,
                            blended_bg,
                        );
                    }
                }
            }
            canvas.draw_rect_outline(self.x, self.y, self.width, self.height, self.border_color);
        }

        if let Some(ref child) = self.child {
            child.render(canvas);
        }
    }

    fn on_event(&mut self, event: &Event) {
        if let Some(ref mut child) = self.child {
            child.on_event(event);
        }
    }
}

// ── Scroll Container ─────────────────────────────────────────────────────

pub struct ScrollContainer {
    pub x: usize,
    pub y: usize,
    pub width: usize,
    pub height: usize,
    pub content_height: usize,
    pub scroll_y: i32,
    pub child: Box<dyn Widget>,
}

impl ScrollContainer {
    pub fn new(
        x: usize,
        y: usize,
        width: usize,
        height: usize,
        content_height: usize,
        child: Box<dyn Widget>,
    ) -> Self {
        Self {
            x,
            y,
            width,
            height,
            content_height,
            scroll_y: 0,
            child,
        }
    }

    pub fn scroll_by(&mut self, dy: i32) {
        self.scroll_y += dy;
        let max_scroll = self.content_height as i32 - self.height as i32;
        if self.scroll_y < 0 {
            self.scroll_y = 0;
        }
        if max_scroll > 0 && self.scroll_y > max_scroll {
            self.scroll_y = max_scroll;
        }
    }

    fn draw_scrollbar(&self, canvas: &mut Canvas) {
        if self.content_height <= self.height {
            return;
        }
        let bar_w = 4;
        let bar_x = self.x + self.width - bar_w - 1;

        // Track
        canvas.fill_rect(bar_x, self.y, bar_w, self.height, 0xFF_22_22_33);

        // Thumb
        let ratio = self.height as f32 / self.content_height as f32;
        let thumb_h = ((self.height as f32) * ratio) as usize;
        let thumb_h = thumb_h.max(8);
        let scroll_ratio = self.scroll_y as f32 / (self.content_height as f32 - self.height as f32);
        let thumb_y = self.y + ((self.height - thumb_h) as f32 * scroll_ratio) as usize;
        canvas.fill_rect(bar_x, thumb_y, bar_w, thumb_h, theme::BORDER);
    }
}

impl Widget for ScrollContainer {
    fn render(&self, canvas: &mut Canvas) {
        canvas.fill_rect(self.x, self.y, self.width, self.height, theme::WINDOW_BG);
        canvas.draw_rect_outline(self.x, self.y, self.width, self.height, theme::BORDER);

        // Render child (content is offset by scroll_y — child is expected to
        // render at absolute coordinates; a real impl would use a clip region
        // and translate, but for now we just render and trust the child.)
        self.child.render(canvas);

        self.draw_scrollbar(canvas);
    }

    fn on_event(&mut self, event: &Event) {
        match event {
            Event::KeyPress(k) => {
                match *k {
                    // Page up / down approximations
                    73 => self.scroll_by(-(self.height as i32 / 2)),
                    81 => self.scroll_by(self.height as i32 / 2),
                    _ => self.child.on_event(event),
                }
            }
            Event::MouseClick { x, y } => {
                let in_x = *x >= self.x as i32 && *x < (self.x + self.width) as i32;
                let in_y = *y >= self.y as i32 && *y < (self.y + self.height) as i32;
                if in_x && in_y {
                    self.child.on_event(event);
                }
            }
            _ => self.child.on_event(event),
        }
    }
}
