//! Feedback widgets for RaeUI.
//!
//! Toast, Snackbar, Skeleton, Spinner, AlertDialog, Banner, EmptyState,
//! StepIndicator — all theme-aware and keyboard-navigable.

extern crate alloc;

use crate::accessibility::AccessibilityRole;
use crate::{blend_colors, default_theme, Event, Theme};
use alloc::string::String;
use alloc::vec::Vec;
use raegfx::Canvas;

fn hit(mx: i32, my: i32, x: i32, y: i32, w: u32, h: u32) -> bool {
    mx >= x && mx < x + w as i32 && my >= y && my < y + h as i32
}

// ── Toast ───────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToastSeverity {
    Info,
    Success,
    Warning,
    Error,
}

pub struct Toast {
    pub message: String,
    pub severity: ToastSeverity,
    pub visible: bool,
    pub duration_ms: u32,
    pub elapsed_ms: u32,
    pub theme: Theme,
}

impl Toast {
    pub fn new(message: &str, severity: ToastSeverity, duration_ms: u32) -> Self {
        Self {
            message: String::from(message),
            severity,
            visible: false,
            duration_ms,
            elapsed_ms: 0,
            theme: default_theme(),
        }
    }

    pub fn show(&mut self) {
        self.visible = true;
        self.elapsed_ms = 0;
    }

    pub fn tick(&mut self, dt_ms: u32) {
        if !self.visible {
            return;
        }
        self.elapsed_ms += dt_ms;
        if self.elapsed_ms >= self.duration_ms {
            self.visible = false;
        }
    }

    fn accent_color(&self) -> u32 {
        match self.severity {
            ToastSeverity::Info => 0xFF_4E_9C_FF,
            ToastSeverity::Success => 0xFF_00_CC_66,
            ToastSeverity::Warning => 0xFF_FF_CC_00,
            ToastSeverity::Error => 0xFF_FF_33_33,
        }
    }

    pub fn render(&self, canvas: &mut Canvas, x: i32, y: i32, w: u32, h: u32) {
        if !self.visible {
            return;
        }
        let ux = x as usize;
        let uy = y as usize;
        let bg = blend_colors(self.theme.chrome_bg, self.theme.window_bg, 230);
        canvas.fill_rect(ux, uy, w as usize, h as usize, bg);
        // Left accent stripe
        canvas.fill_rect(ux, uy, 4, h as usize, self.accent_color());
        canvas.draw_rect_outline(ux, uy, w as usize, h as usize, self.theme.border);
        // Icon char
        let icon = match self.severity {
            ToastSeverity::Info => "i",
            ToastSeverity::Success => "v",
            ToastSeverity::Warning => "!",
            ToastSeverity::Error => "x",
        };
        canvas.draw_text(
            ux + 10,
            uy + (h as usize - 8) / 2,
            icon,
            self.accent_color(),
            None,
        );
        canvas.draw_text(
            ux + 24,
            uy + (h as usize - 8) / 2,
            &self.message,
            self.theme.text_fg,
            None,
        );
        // Dismiss X
        canvas.draw_text(
            ux + w as usize - 14,
            uy + (h as usize - 8) / 2,
            "x",
            self.theme.text_fg,
            None,
        );
    }

    pub fn handle_event(&mut self, event: &Event, x: i32, y: i32, w: u32, h: u32) {
        if !self.visible {
            return;
        }
        if let Event::MouseClick { x: mx, y: my } = event {
            if hit(*mx, *my, x + w as i32 - 16, y, 16, h) {
                self.visible = false;
            }
        }
    }

    pub fn accessibility_role(&self) -> AccessibilityRole {
        AccessibilityRole::Alert
    }
}

// ── Snackbar ────────────────────────────────────────────────────────────

pub struct Snackbar {
    pub message: String,
    pub action_label: Option<String>,
    pub visible: bool,
    pub duration_ms: u32,
    pub elapsed_ms: u32,
    pub theme: Theme,
}

impl Snackbar {
    pub fn new(message: &str) -> Self {
        Self {
            message: String::from(message),
            action_label: None,
            visible: false,
            duration_ms: 4000,
            elapsed_ms: 0,
            theme: default_theme(),
        }
    }

    pub fn with_action(mut self, label: &str) -> Self {
        self.action_label = Some(String::from(label));
        self
    }

    pub fn show(&mut self) {
        self.visible = true;
        self.elapsed_ms = 0;
    }

    pub fn tick(&mut self, dt_ms: u32) {
        if !self.visible {
            return;
        }
        self.elapsed_ms += dt_ms;
        if self.elapsed_ms >= self.duration_ms {
            self.visible = false;
        }
    }

    pub fn render(&self, canvas: &mut Canvas, x: i32, y: i32, w: u32, h: u32) {
        if !self.visible {
            return;
        }
        let ux = x as usize;
        let uy = y as usize;
        let bg = blend_colors(0xFF_33_33_33, self.theme.window_bg, 240);
        canvas.fill_rect(ux, uy, w as usize, h as usize, bg);
        canvas.draw_rect_outline(ux, uy, w as usize, h as usize, self.theme.border);
        canvas.draw_text(
            ux + 12,
            uy + (h as usize - 8) / 2,
            &self.message,
            self.theme.text_fg,
            None,
        );
        if let Some(ref action) = self.action_label {
            let ax = ux + w as usize - action.len() * 8 - 12;
            canvas.draw_text(
                ax,
                uy + (h as usize - 8) / 2,
                action,
                self.theme.button_hot,
                None,
            );
        }
    }

    /// Returns true if the action button was clicked.
    pub fn handle_event(&mut self, event: &Event, x: i32, y: i32, w: u32, h: u32) -> bool {
        if !self.visible {
            return false;
        }
        if let Event::MouseClick { x: mx, y: my } = event {
            if let Some(ref action) = self.action_label {
                let ax = x + w as i32 - action.len() as i32 * 8 - 12;
                if hit(*mx, *my, ax, y, (action.len() * 8 + 8) as u32, h) {
                    self.visible = false;
                    return true;
                }
            }
            if hit(*mx, *my, x, y, w, h) {
                self.visible = false;
            }
        }
        false
    }

    pub fn accessibility_role(&self) -> AccessibilityRole {
        AccessibilityRole::Alert
    }
}

// ── Skeleton ────────────────────────────────────────────────────────────

pub struct Skeleton {
    pub variant: SkeletonVariant,
    pub shimmer_phase: f32,
    pub theme: Theme,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SkeletonVariant {
    Text,
    Circle,
    Rect,
}

impl Skeleton {
    pub fn new(variant: SkeletonVariant) -> Self {
        Self {
            variant,
            shimmer_phase: 0.0,
            theme: default_theme(),
        }
    }

    pub fn tick(&mut self, dt: f32) {
        self.shimmer_phase += dt * 2.0;
        if self.shimmer_phase > 1.0 {
            self.shimmer_phase -= 1.0;
        }
    }

    pub fn render(&self, canvas: &mut Canvas, x: i32, y: i32, w: u32, h: u32) {
        let ux = x as usize;
        let uy = y as usize;
        let base = 0xFF_2A_2A_3A;
        let shimmer_col = 0xFF_3A_3A_4A;
        match self.variant {
            SkeletonVariant::Text => {
                canvas.fill_rect(ux, uy, w as usize, h as usize, base);
                let shimmer_x = ux + (w as f32 * self.shimmer_phase) as usize;
                let sw = (w as usize) / 4;
                if shimmer_x + sw <= ux + w as usize {
                    canvas.fill_rect(shimmer_x, uy, sw, h as usize, shimmer_col);
                }
            }
            SkeletonVariant::Circle => {
                let r = (w.min(h) / 2) as usize;
                let cx = ux + w as usize / 2;
                let cy = uy + h as usize / 2;
                for dy in 0..r * 2 {
                    for dx in 0..r * 2 {
                        let rx = dx as i32 - r as i32;
                        let ry = dy as i32 - r as i32;
                        if rx * rx + ry * ry <= (r as i32) * (r as i32) {
                            canvas.draw_pixel(
                                (cx as i32 + rx) as usize,
                                (cy as i32 + ry) as usize,
                                base,
                            );
                        }
                    }
                }
            }
            SkeletonVariant::Rect => {
                canvas.fill_rect(ux, uy, w as usize, h as usize, base);
                let shimmer_x = ux + (w as f32 * self.shimmer_phase) as usize;
                let sw = (w as usize) / 4;
                if shimmer_x + sw <= ux + w as usize {
                    canvas.fill_rect(shimmer_x, uy, sw, h as usize, shimmer_col);
                }
            }
        }
    }

    pub fn accessibility_role(&self) -> AccessibilityRole {
        AccessibilityRole::Group
    }
}

// ── Spinner ─────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpinnerSize {
    Small,
    Medium,
    Large,
}

pub struct Spinner {
    pub size: SpinnerSize,
    pub angle: f32,
    pub theme: Theme,
}

impl Spinner {
    pub fn new(size: SpinnerSize) -> Self {
        Self {
            size,
            angle: 0.0,
            theme: default_theme(),
        }
    }

    pub fn tick(&mut self, dt: f32) {
        self.angle += dt * 360.0;
        if self.angle >= 360.0 {
            self.angle -= 360.0;
        }
    }

    fn radius(&self) -> usize {
        match self.size {
            SpinnerSize::Small => 6,
            SpinnerSize::Medium => 12,
            SpinnerSize::Large => 20,
        }
    }

    pub fn render(&self, canvas: &mut Canvas, x: i32, y: i32, w: u32, h: u32) {
        let r = self.radius();
        let cx = x as usize + w as usize / 2;
        let cy = y as usize + h as usize / 2;
        let track_col = 0xFF_33_33_44;
        // Full circle (track)
        for dy in 0..r * 2 {
            for dx in 0..r * 2 {
                let rx = dx as i32 - r as i32;
                let ry = dy as i32 - r as i32;
                let dist_sq = rx * rx + ry * ry;
                let outer = (r as i32) * (r as i32);
                let inner = ((r as i32) - 2) * ((r as i32) - 2);
                if dist_sq <= outer && dist_sq >= inner {
                    canvas.draw_pixel(
                        (cx as i32 + rx) as usize,
                        (cy as i32 + ry) as usize,
                        track_col,
                    );
                }
            }
        }
        // Arc (active) — draw a 90-degree arc at the current angle
        let start = self.angle;
        let end = self.angle + 90.0;
        for dy in 0..r * 2 {
            for dx in 0..r * 2 {
                let rx = dx as i32 - r as i32;
                let ry = dy as i32 - r as i32;
                let dist_sq = rx * rx + ry * ry;
                let outer = (r as i32) * (r as i32);
                let inner = ((r as i32) - 2) * ((r as i32) - 2);
                if dist_sq <= outer && dist_sq >= inner {
                    let angle = libm::atan2f(ry as f32, rx as f32) * 180.0 / core::f32::consts::PI;
                    let angle = if angle < 0.0 { angle + 360.0 } else { angle };
                    let in_arc = if start <= end {
                        angle >= start && angle < end
                    } else {
                        angle >= start || angle < (end - 360.0)
                    };
                    if in_arc {
                        canvas.draw_pixel(
                            (cx as i32 + rx) as usize,
                            (cy as i32 + ry) as usize,
                            self.theme.border,
                        );
                    }
                }
            }
        }
    }

    pub fn accessibility_role(&self) -> AccessibilityRole {
        AccessibilityRole::ProgressBar
    }
}

// ── AlertDialog ─────────────────────────────────────────────────────────

pub struct AlertDialog {
    pub title: String,
    pub message: String,
    pub primary_label: String,
    pub secondary_label: Option<String>,
    pub cancel_label: Option<String>,
    pub visible: bool,
    pub focused_button: u8,
    pub theme: Theme,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlertResult {
    Primary,
    Secondary,
    Cancel,
    None,
}

impl AlertDialog {
    pub fn new(title: &str, message: &str, primary: &str) -> Self {
        Self {
            title: String::from(title),
            message: String::from(message),
            primary_label: String::from(primary),
            secondary_label: None,
            cancel_label: None,
            visible: false,
            focused_button: 0,
            theme: default_theme(),
        }
    }

    pub fn with_secondary(mut self, label: &str) -> Self {
        self.secondary_label = Some(String::from(label));
        self
    }

    pub fn with_cancel(mut self, label: &str) -> Self {
        self.cancel_label = Some(String::from(label));
        self
    }

    pub fn show(&mut self) {
        self.visible = true;
        self.focused_button = 0;
    }

    fn button_count(&self) -> u8 {
        1 + self.secondary_label.is_some() as u8 + self.cancel_label.is_some() as u8
    }

    pub fn render(&self, canvas: &mut Canvas, x: i32, y: i32, w: u32, h: u32) {
        if !self.visible {
            return;
        }
        let ux = x as usize;
        let uy = y as usize;
        // Backdrop
        let backdrop = blend_colors(0xFF_00_00_00, self.theme.window_bg, 128);
        canvas.fill_rect(ux, uy, w as usize, h as usize, backdrop);
        // Dialog box centered
        let dw = (w as usize) * 2 / 3;
        let dh = (h as usize) * 2 / 5;
        let dx = ux + (w as usize - dw) / 2;
        let dy = uy + (h as usize - dh) / 2;
        canvas.fill_rect(dx, dy, dw, dh, self.theme.window_bg);
        canvas.draw_rect_outline(dx, dy, dw, dh, self.theme.border);
        // Title
        canvas.draw_text(dx + 12, dy + 12, &self.title, self.theme.title_fg, None);
        // Message
        canvas.draw_text(dx + 12, dy + 32, &self.message, self.theme.text_fg, None);
        // Buttons at bottom
        let btn_h = 28usize;
        let btn_y = dy + dh - btn_h - 8;
        let btn_w = 80usize;
        let gap = 8usize;
        let total_btns = self.button_count() as usize;
        let btns_total_w = total_btns * btn_w + (total_btns - 1) * gap;
        let btn_start_x = dx + dw - btns_total_w - 12;

        let mut bx = btn_start_x;
        let labels: Vec<(&str, u32)> = {
            let mut v = Vec::new();
            v.push((self.primary_label.as_str(), self.theme.button_hot));
            if let Some(ref s) = self.secondary_label {
                v.push((s.as_str(), self.theme.button_bg));
            }
            if let Some(ref s) = self.cancel_label {
                v.push((s.as_str(), self.theme.button_bg));
            }
            v
        };
        for (i, &(label, bg)) in labels.iter().enumerate() {
            let is_focused = self.focused_button == i as u8;
            let bg_col = if is_focused {
                self.theme.button_hot
            } else {
                bg
            };
            canvas.fill_rect(bx, btn_y, btn_w, btn_h, bg_col);
            canvas.draw_rect_outline(bx, btn_y, btn_w, btn_h, self.theme.border);
            let tx = bx + (btn_w - label.len() * 8) / 2;
            canvas.draw_text(
                tx,
                btn_y + (btn_h - 8) / 2,
                label,
                self.theme.button_text,
                None,
            );
            bx += btn_w + gap;
        }
    }

    pub fn handle_event(&mut self, event: &Event, x: i32, y: i32, w: u32, h: u32) -> AlertResult {
        if !self.visible {
            return AlertResult::None;
        }
        match event {
            Event::KeyPress(k) => match *k {
                9 | 77 => {
                    self.focused_button = (self.focused_button + 1) % self.button_count();
                }
                75 => {
                    self.focused_button = if self.focused_button == 0 {
                        self.button_count() - 1
                    } else {
                        self.focused_button - 1
                    };
                }
                13 => {
                    self.visible = false;
                    return match self.focused_button {
                        0 => AlertResult::Primary,
                        1 if self.secondary_label.is_some() => AlertResult::Secondary,
                        _ => AlertResult::Cancel,
                    };
                }
                27 => {
                    self.visible = false;
                    return AlertResult::Cancel;
                }
                _ => {}
            },
            Event::MouseClick { x: mx, y: my } => {
                let dw = (w * 2 / 3) as i32;
                let dh = (h * 2 / 5) as i32;
                let dx = x + (w as i32 - dw) / 2;
                let dy = y + (h as i32 - dh) / 2;
                let btn_h = 28i32;
                let btn_y = dy + dh - btn_h - 8;
                let btn_w = 80i32;
                let gap = 8i32;
                let total_btns = self.button_count() as i32;
                let btns_total_w = total_btns * btn_w + (total_btns - 1) * gap;
                let btn_start_x = dx + dw - btns_total_w - 12;

                let mut bx = btn_start_x;
                for i in 0..self.button_count() {
                    if hit(*mx, *my, bx, btn_y, btn_w as u32, btn_h as u32) {
                        self.visible = false;
                        return match i {
                            0 => AlertResult::Primary,
                            1 if self.secondary_label.is_some() => AlertResult::Secondary,
                            _ => AlertResult::Cancel,
                        };
                    }
                    bx += btn_w + gap;
                }
            }
            _ => {}
        }
        AlertResult::None
    }

    pub fn accessibility_role(&self) -> AccessibilityRole {
        AccessibilityRole::Alert
    }
}

// ── Banner ──────────────────────────────────────────────────────────────

pub struct Banner {
    pub message: String,
    pub severity: ToastSeverity,
    pub visible: bool,
    pub dismissible: bool,
    pub theme: Theme,
}

impl Banner {
    pub fn new(message: &str, severity: ToastSeverity) -> Self {
        Self {
            message: String::from(message),
            severity,
            visible: true,
            dismissible: true,
            theme: default_theme(),
        }
    }

    fn accent(&self) -> u32 {
        match self.severity {
            ToastSeverity::Info => 0xFF_4E_9C_FF,
            ToastSeverity::Success => 0xFF_00_CC_66,
            ToastSeverity::Warning => 0xFF_FF_CC_00,
            ToastSeverity::Error => 0xFF_FF_33_33,
        }
    }

    pub fn render(&self, canvas: &mut Canvas, x: i32, y: i32, w: u32, h: u32) {
        if !self.visible {
            return;
        }
        let ux = x as usize;
        let uy = y as usize;
        canvas.fill_rect(ux, uy, w as usize, h as usize, self.accent());
        canvas.draw_text(
            ux + 12,
            uy + (h as usize - 8) / 2,
            &self.message,
            0xFF_FF_FF_FF,
            None,
        );
        if self.dismissible {
            canvas.draw_text(
                ux + w as usize - 14,
                uy + (h as usize - 8) / 2,
                "x",
                0xFF_FF_FF_FF,
                None,
            );
        }
    }

    pub fn handle_event(&mut self, event: &Event, x: i32, y: i32, w: u32, h: u32) {
        if !self.visible || !self.dismissible {
            return;
        }
        if let Event::MouseClick { x: mx, y: my } = event {
            if hit(*mx, *my, x + w as i32 - 16, y, 16, h) {
                self.visible = false;
            }
        }
    }

    pub fn accessibility_role(&self) -> AccessibilityRole {
        AccessibilityRole::Alert
    }
}

// ── EmptyState ──────────────────────────────────────────────────────────

pub struct EmptyState {
    pub icon_char: char,
    pub title: String,
    pub subtitle: String,
    pub action_label: Option<String>,
    pub focused: bool,
    pub theme: Theme,
}

impl EmptyState {
    pub fn new(title: &str, subtitle: &str) -> Self {
        Self {
            icon_char: '?',
            title: String::from(title),
            subtitle: String::from(subtitle),
            action_label: None,
            focused: false,
            theme: default_theme(),
        }
    }

    pub fn with_action(mut self, label: &str) -> Self {
        self.action_label = Some(String::from(label));
        self
    }

    pub fn render(&self, canvas: &mut Canvas, x: i32, y: i32, w: u32, h: u32) {
        let ux = x as usize;
        let uy = y as usize;
        canvas.fill_rect(ux, uy, w as usize, h as usize, self.theme.window_bg);
        let center_x = ux + w as usize / 2;
        let center_y = uy + h as usize / 2;
        // Icon
        let mut ibuf = [0u8; 4];
        let icon_str = self.icon_char.encode_utf8(&mut ibuf);
        canvas.draw_text(
            center_x.saturating_sub(4),
            center_y.saturating_sub(30),
            icon_str,
            self.theme.border,
            None,
        );
        // Title
        let tw = self.title.len() * 8;
        canvas.draw_text(
            center_x.saturating_sub(tw / 2),
            center_y.saturating_sub(12),
            &self.title,
            self.theme.title_fg,
            None,
        );
        // Subtitle
        let sw = self.subtitle.len() * 8;
        canvas.draw_text(
            center_x.saturating_sub(sw / 2),
            center_y + 6,
            &self.subtitle,
            self.theme.text_fg,
            None,
        );
        // Action button
        if let Some(ref action) = self.action_label {
            let btn_w = action.len() * 8 + 16;
            let bx = center_x.saturating_sub(btn_w / 2);
            let by = center_y + 24;
            let bg = if self.focused {
                self.theme.button_hot
            } else {
                self.theme.button_bg
            };
            canvas.fill_rect(bx, by, btn_w, 24, bg);
            canvas.draw_rect_outline(bx, by, btn_w, 24, self.theme.border);
            canvas.draw_text(bx + 8, by + 8, action, self.theme.button_text, None);
        }
    }

    pub fn handle_event(&mut self, event: &Event, x: i32, y: i32, w: u32, h: u32) -> bool {
        if let Event::MouseClick { x: mx, y: my } = event {
            if let Some(ref action) = self.action_label {
                let center_x = x + w as i32 / 2;
                let center_y = y + h as i32 / 2;
                let btn_w = (action.len() * 8 + 16) as i32;
                let bx = center_x - btn_w / 2;
                let by = center_y + 24;
                if hit(*mx, *my, bx, by, btn_w as u32, 24) {
                    return true;
                }
            }
        }
        if let Event::KeyPress(13) = event {
            if self.focused && self.action_label.is_some() {
                return true;
            }
        }
        false
    }

    pub fn accessibility_role(&self) -> AccessibilityRole {
        AccessibilityRole::Group
    }
}

// ── StepIndicator ───────────────────────────────────────────────────────

pub struct StepIndicator {
    pub labels: Vec<String>,
    pub current_step: usize,
    pub theme: Theme,
}

impl StepIndicator {
    pub fn new(labels: &[&str]) -> Self {
        let mut v = Vec::new();
        for l in labels {
            v.push(String::from(*l));
        }
        Self {
            labels: v,
            current_step: 0,
            theme: default_theme(),
        }
    }

    pub fn set_step(&mut self, step: usize) {
        if step < self.labels.len() {
            self.current_step = step;
        }
    }

    pub fn render(&self, canvas: &mut Canvas, x: i32, y: i32, w: u32, h: u32) {
        let ux = x as usize;
        let uy = y as usize;
        let count = self.labels.len();
        if count == 0 {
            return;
        }
        let step_w = w as usize / count;
        let circle_r = 10usize;
        let center_y = uy + h as usize / 2;

        for (i, label) in self.labels.iter().enumerate() {
            let cx = ux + i * step_w + step_w / 2;
            let is_done = i < self.current_step;
            let is_active = i == self.current_step;

            // Connector line (not for first)
            if i > 0 {
                let prev_cx = ux + (i - 1) * step_w + step_w / 2 + circle_r;
                let line_end = cx.saturating_sub(circle_r);
                let col = if is_done {
                    self.theme.border
                } else {
                    0xFF_33_33_44
                };
                for px in prev_cx..line_end {
                    canvas.draw_pixel(px, center_y, col);
                }
            }

            // Circle
            let circle_col = if is_done || is_active {
                self.theme.border
            } else {
                0xFF_33_33_44
            };
            for dy in 0..circle_r * 2 {
                for dx in 0..circle_r * 2 {
                    let rx = dx as i32 - circle_r as i32;
                    let ry = dy as i32 - circle_r as i32;
                    if rx * rx + ry * ry <= (circle_r as i32) * (circle_r as i32) {
                        canvas.draw_pixel(
                            (cx as i32 + rx) as usize,
                            (center_y as i32 + ry) as usize,
                            circle_col,
                        );
                    }
                }
            }

            // Step number
            let num = i + 1;
            let mut nbuf = [0u8; 10];
            let num_str = u32_to_decimal(num as u32, &mut nbuf);
            let fg = if is_done || is_active {
                self.theme.title_fg
            } else {
                self.theme.text_fg
            };
            canvas.draw_text(
                cx.saturating_sub(4),
                center_y.saturating_sub(4),
                num_str,
                fg,
                None,
            );

            // Label below
            let lw = label.len() * 8;
            let lx = cx.saturating_sub(lw / 2);
            canvas.draw_text(lx, center_y + circle_r + 4, label, self.theme.text_fg, None);
        }
    }

    pub fn accessibility_role(&self) -> AccessibilityRole {
        AccessibilityRole::ProgressBar
    }
}

fn u32_to_decimal(mut val: u32, buf: &mut [u8; 10]) -> &str {
    if val == 0 {
        buf[0] = b'0';
        return unsafe { core::str::from_utf8_unchecked(&buf[..1]) };
    }
    let mut pos = 10;
    while val > 0 && pos > 0 {
        pos -= 1;
        buf[pos] = b'0' + (val % 10) as u8;
        val /= 10;
    }
    unsafe { core::str::from_utf8_unchecked(&buf[pos..]) }
}
