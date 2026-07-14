//! Container widgets for RaeUI.
//!
//! ScrollView, SplitView, CollapsibleSection, Card, StackView, Popover,
//! Modal, Drawer, ContextMenu — all theme-aware, keyboard-navigable,
//! and accessibility-annotated.

extern crate alloc;

use crate::accessibility::AccessibilityRole;
use crate::{blend_colors, default_theme, Event, Theme};
use alloc::string::String;
use alloc::vec::Vec;
use raegfx::Canvas;

fn hit(mx: i32, my: i32, x: i32, y: i32, w: u32, h: u32) -> bool {
    mx >= x && mx < x + w as i32 && my >= y && my < y + h as i32
}

fn clamp_i32(v: i32, lo: i32, hi: i32) -> i32 {
    if v < lo {
        lo
    } else if v > hi {
        hi
    } else {
        v
    }
}

// ── ScrollView ──────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScrollAxis {
    Vertical,
    Horizontal,
    Both,
}

pub struct ScrollView {
    pub axis: ScrollAxis,
    pub scroll_x: i32,
    pub scroll_y: i32,
    pub content_w: u32,
    pub content_h: u32,
    /// Momentum velocity (pixels per tick) for smooth scrolling.
    pub velocity_x: i32,
    pub velocity_y: i32,
    pub focused: bool,
    pub theme: Theme,
}

impl ScrollView {
    pub fn new(axis: ScrollAxis, content_w: u32, content_h: u32) -> Self {
        Self {
            axis,
            scroll_x: 0,
            scroll_y: 0,
            content_w,
            content_h,
            velocity_x: 0,
            velocity_y: 0,
            focused: false,
            theme: default_theme(),
        }
    }

    pub fn scroll_to(&mut self, sx: i32, sy: i32, view_w: u32, view_h: u32) {
        let max_x = (self.content_w as i32 - view_w as i32).max(0);
        let max_y = (self.content_h as i32 - view_h as i32).max(0);
        self.scroll_x = clamp_i32(sx, 0, max_x);
        self.scroll_y = clamp_i32(sy, 0, max_y);
    }

    pub fn tick_momentum(&mut self, view_w: u32, view_h: u32) {
        if self.velocity_x != 0 || self.velocity_y != 0 {
            self.scroll_to(
                self.scroll_x + self.velocity_x,
                self.scroll_y + self.velocity_y,
                view_w,
                view_h,
            );
            self.velocity_x = self.velocity_x * 9 / 10;
            self.velocity_y = self.velocity_y * 9 / 10;
        }
    }

    pub fn render(&self, canvas: &mut Canvas, x: i32, y: i32, w: u32, h: u32) {
        let ux = x as usize;
        let uy = y as usize;
        canvas.fill_rect(ux, uy, w as usize, h as usize, self.theme.window_bg);
        canvas.draw_rect_outline(ux, uy, w as usize, h as usize, self.theme.border);
        // Vertical scrollbar
        if matches!(self.axis, ScrollAxis::Vertical | ScrollAxis::Both) && self.content_h > h {
            let bar_w: usize = 4;
            let bar_x = ux + w as usize - bar_w - 1;
            canvas.fill_rect(bar_x, uy, bar_w, h as usize, 0xFF_22_22_33);
            let ratio = h as f32 / self.content_h as f32;
            let thumb_h = ((h as f32) * ratio) as usize;
            let thumb_h = if thumb_h < 8 { 8 } else { thumb_h };
            let max_scroll = (self.content_h as f32 - h as f32).max(1.0);
            let scroll_ratio = self.scroll_y as f32 / max_scroll;
            let thumb_y = uy + ((h as usize - thumb_h) as f32 * scroll_ratio) as usize;
            canvas.fill_rect(bar_x, thumb_y, bar_w, thumb_h, self.theme.border);
        }
        // Horizontal scrollbar
        if matches!(self.axis, ScrollAxis::Horizontal | ScrollAxis::Both) && self.content_w > w {
            let bar_h: usize = 4;
            let bar_y = uy + h as usize - bar_h - 1;
            canvas.fill_rect(ux, bar_y, w as usize, bar_h, 0xFF_22_22_33);
            let ratio = w as f32 / self.content_w as f32;
            let thumb_w = ((w as f32) * ratio) as usize;
            let thumb_w = if thumb_w < 8 { 8 } else { thumb_w };
            let max_scroll = (self.content_w as f32 - w as f32).max(1.0);
            let scroll_ratio = self.scroll_x as f32 / max_scroll;
            let thumb_x = ux + ((w as usize - thumb_w) as f32 * scroll_ratio) as usize;
            canvas.fill_rect(thumb_x, bar_y, thumb_w, bar_h, self.theme.border);
        }
        if self.focused {
            canvas.draw_rect_outline(ux, uy, w as usize, h as usize, self.theme.button_hot);
        }
    }

    pub fn handle_event(&mut self, event: &Event, x: i32, y: i32, w: u32, h: u32) {
        match event {
            Event::MouseClick { x: mx, y: my } => {
                self.focused = hit(*mx, *my, x, y, w, h);
            }
            Event::KeyPress(k) if self.focused => {
                let step = 20i32;
                match *k {
                    72 => self.scroll_to(self.scroll_x, self.scroll_y - step, w, h),
                    80 => self.scroll_to(self.scroll_x, self.scroll_y + step, w, h),
                    75 => self.scroll_to(self.scroll_x - step, self.scroll_y, w, h),
                    77 => self.scroll_to(self.scroll_x + step, self.scroll_y, w, h),
                    _ => {}
                }
            }
            _ => {}
        }
    }

    pub fn accessibility_role(&self) -> AccessibilityRole {
        AccessibilityRole::ScrollView
    }
    pub fn accessibility_label(&self) -> &str {
        "Scroll view"
    }
}

// ── SplitView ───────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SplitDirection {
    Horizontal,
    Vertical,
}

pub struct SplitView {
    pub direction: SplitDirection,
    /// Split position as fraction 0.0–1.0.
    pub split: f32,
    pub min_split: f32,
    pub max_split: f32,
    pub dragging: bool,
    pub handle_size: u32,
    pub theme: Theme,
}

impl SplitView {
    pub fn new(direction: SplitDirection) -> Self {
        Self {
            direction,
            split: 0.5,
            min_split: 0.1,
            max_split: 0.9,
            dragging: false,
            handle_size: 6,
            theme: default_theme(),
        }
    }

    pub fn panel_rects(
        &self,
        x: i32,
        y: i32,
        w: u32,
        h: u32,
    ) -> ((i32, i32, u32, u32), (i32, i32, u32, u32)) {
        let hs = self.handle_size;
        match self.direction {
            SplitDirection::Horizontal => {
                let left_w = ((w as f32 * self.split) as u32).saturating_sub(hs / 2);
                let right_x = x + left_w as i32 + hs as i32;
                let right_w = w.saturating_sub(left_w + hs);
                ((x, y, left_w, h), (right_x, y, right_w, h))
            }
            SplitDirection::Vertical => {
                let top_h = ((h as f32 * self.split) as u32).saturating_sub(hs / 2);
                let bot_y = y + top_h as i32 + hs as i32;
                let bot_h = h.saturating_sub(top_h + hs);
                ((x, y, w, top_h), (x, bot_y, w, bot_h))
            }
        }
    }

    pub fn render(&self, canvas: &mut Canvas, x: i32, y: i32, w: u32, h: u32) {
        let ux = x as usize;
        let uy = y as usize;
        canvas.fill_rect(ux, uy, w as usize, h as usize, self.theme.window_bg);
        // Draw handle
        let hs = self.handle_size as usize;
        let handle_col = if self.dragging {
            self.theme.button_hot
        } else {
            self.theme.border
        };
        match self.direction {
            SplitDirection::Horizontal => {
                let hx = ux + (w as f32 * self.split) as usize - hs / 2;
                canvas.fill_rect(hx, uy, hs, h as usize, handle_col);
            }
            SplitDirection::Vertical => {
                let hy = uy + (h as f32 * self.split) as usize - hs / 2;
                canvas.fill_rect(ux, hy, w as usize, hs, handle_col);
            }
        }
        canvas.draw_rect_outline(ux, uy, w as usize, h as usize, self.theme.border);
    }

    pub fn handle_event(&mut self, event: &Event, x: i32, y: i32, w: u32, h: u32) {
        match event {
            Event::MouseDown { x: mx, y: my, .. } => {
                if hit(*mx, *my, x, y, w, h) {
                    self.dragging = true;
                    self.update_split(*mx, *my, x, y, w, h);
                }
            }
            Event::MouseUp { .. } => {
                self.dragging = false;
            }
            Event::MouseMove { dx, dy, .. } if self.dragging => {
                let frac = match self.direction {
                    SplitDirection::Horizontal => *dx as f32 / w as f32,
                    SplitDirection::Vertical => *dy as f32 / h as f32,
                };
                self.split = clamp_f32(self.split + frac, self.min_split, self.max_split);
            }
            _ => {}
        }
    }

    fn update_split(&mut self, mx: i32, my: i32, x: i32, y: i32, w: u32, h: u32) {
        let frac = match self.direction {
            SplitDirection::Horizontal => (mx - x) as f32 / w as f32,
            SplitDirection::Vertical => (my - y) as f32 / h as f32,
        };
        self.split = clamp_f32(frac, self.min_split, self.max_split);
    }

    pub fn accessibility_role(&self) -> AccessibilityRole {
        AccessibilityRole::Group
    }
}

fn clamp_f32(v: f32, lo: f32, hi: f32) -> f32 {
    if v < lo {
        lo
    } else if v > hi {
        hi
    } else {
        v
    }
}

// ── CollapsibleSection ──────────────────────────────────────────────────

pub struct CollapsibleSection {
    pub title: String,
    pub expanded: bool,
    pub focused: bool,
    pub header_height: u32,
    pub theme: Theme,
}

impl CollapsibleSection {
    pub fn new(title: &str) -> Self {
        Self {
            title: String::from(title),
            expanded: true,
            focused: false,
            header_height: 24,
            theme: default_theme(),
        }
    }

    pub fn content_rect(&self, x: i32, y: i32, w: u32, h: u32) -> Option<(i32, i32, u32, u32)> {
        if self.expanded {
            Some((
                x,
                y + self.header_height as i32,
                w,
                h.saturating_sub(self.header_height),
            ))
        } else {
            None
        }
    }

    pub fn render(&self, canvas: &mut Canvas, x: i32, y: i32, w: u32, _h: u32) {
        let ux = x as usize;
        let uy = y as usize;
        let hh = self.header_height as usize;
        canvas.fill_rect(ux, uy, w as usize, hh, self.theme.chrome_bg);
        canvas.draw_rect_outline(ux, uy, w as usize, hh, self.theme.border);
        let arrow = if self.expanded { "v" } else { ">" };
        canvas.draw_text(ux + 6, uy + (hh - 8) / 2, arrow, self.theme.title_fg, None);
        canvas.draw_text(
            ux + 20,
            uy + (hh - 8) / 2,
            &self.title,
            self.theme.title_fg,
            None,
        );
        if self.focused {
            canvas.draw_rect_outline(ux, uy, w as usize, hh, self.theme.button_hot);
        }
    }

    pub fn handle_event(&mut self, event: &Event, x: i32, y: i32, w: u32, _h: u32) {
        match event {
            Event::MouseClick { x: mx, y: my } => {
                if hit(*mx, *my, x, y, w, self.header_height) {
                    self.expanded = !self.expanded;
                    self.focused = true;
                } else {
                    self.focused = false;
                }
            }
            Event::KeyPress(13) if self.focused => {
                self.expanded = !self.expanded;
            }
            _ => {}
        }
    }

    pub fn accessibility_role(&self) -> AccessibilityRole {
        AccessibilityRole::Group
    }
    pub fn accessibility_label(&self) -> &str {
        &self.title
    }
}

// ── Card ────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Elevation {
    Flat,
    Low,
    Medium,
    High,
}

pub struct Card {
    pub elevation: Elevation,
    pub corner_radius: u32,
    pub padding: u32,
    pub theme: Theme,
}

impl Card {
    pub fn new(elevation: Elevation) -> Self {
        Self {
            elevation,
            corner_radius: 6,
            padding: 12,
            theme: default_theme(),
        }
    }

    pub fn content_rect(&self, x: i32, y: i32, w: u32, h: u32) -> (i32, i32, u32, u32) {
        let p = self.padding as i32;
        (
            x + p,
            y + p,
            w.saturating_sub(self.padding * 2),
            h.saturating_sub(self.padding * 2),
        )
    }

    pub fn render(&self, canvas: &mut Canvas, x: i32, y: i32, w: u32, h: u32) {
        let ux = x as usize;
        let uy = y as usize;
        let uw = w as usize;
        let uh = h as usize;
        // Shadow layers based on elevation
        let shadow_offsets: &[(usize, u32)] = match self.elevation {
            Elevation::Flat => &[],
            Elevation::Low => &[(1, 0x18_00_00_00)],
            Elevation::Medium => &[(2, 0x20_00_00_00), (1, 0x10_00_00_00)],
            Elevation::High => &[(4, 0x30_00_00_00), (2, 0x18_00_00_00)],
        };
        for &(off, col) in shadow_offsets {
            let blended = blend_colors(col, self.theme.window_bg, (col >> 24) as u8);
            canvas.fill_rect(ux + off, uy + off, uw, uh, blended);
        }
        canvas.fill_rect(ux, uy, uw, uh, self.theme.window_bg);
        canvas.draw_rect_outline(ux, uy, uw, uh, self.theme.border);
    }

    pub fn accessibility_role(&self) -> AccessibilityRole {
        AccessibilityRole::Group
    }
}

// ── StackView ───────────────────────────────────────────────────────────

pub struct StackView {
    pub depth: usize,
    pub transition_progress: f32,
    pub theme: Theme,
}

impl StackView {
    pub fn new() -> Self {
        Self {
            depth: 0,
            transition_progress: 1.0,
            theme: default_theme(),
        }
    }

    pub fn push(&mut self) {
        self.depth += 1;
        self.transition_progress = 0.0;
    }

    pub fn pop(&mut self) {
        if self.depth > 0 {
            self.depth -= 1;
            self.transition_progress = 0.0;
        }
    }

    pub fn tick(&mut self, dt: f32) {
        if self.transition_progress < 1.0 {
            self.transition_progress = clamp_f32(self.transition_progress + dt * 4.0, 0.0, 1.0);
        }
    }

    pub fn render(&self, canvas: &mut Canvas, x: i32, y: i32, w: u32, h: u32) {
        let ux = x as usize;
        let uy = y as usize;
        canvas.fill_rect(ux, uy, w as usize, h as usize, self.theme.window_bg);
        canvas.draw_rect_outline(ux, uy, w as usize, h as usize, self.theme.border);
        // Depth indicator bar at top
        if self.depth > 0 {
            let bar_w = (w as usize).min(self.depth * 20);
            canvas.fill_rect(ux, uy, bar_w, 2, self.theme.button_hot);
        }
    }

    pub fn accessibility_role(&self) -> AccessibilityRole {
        AccessibilityRole::Group
    }
}

// ── Popover ─────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PopoverEdge {
    Top,
    Bottom,
    Left,
    Right,
}

pub struct Popover {
    pub visible: bool,
    pub preferred_edge: PopoverEdge,
    pub arrow_size: u32,
    pub theme: Theme,
}

impl Popover {
    pub fn new(edge: PopoverEdge) -> Self {
        Self {
            visible: false,
            preferred_edge: edge,
            arrow_size: 6,
            theme: default_theme(),
        }
    }

    pub fn show(&mut self) {
        self.visible = true;
    }
    pub fn hide(&mut self) {
        self.visible = false;
    }
    pub fn toggle(&mut self) {
        self.visible = !self.visible;
    }

    /// Compute popover rect given the anchor rect and desired content size.
    pub fn content_rect(
        &self,
        anchor_x: i32,
        anchor_y: i32,
        anchor_w: u32,
        anchor_h: u32,
        pop_w: u32,
        pop_h: u32,
        screen_w: u32,
        screen_h: u32,
    ) -> (i32, i32) {
        let arrow = self.arrow_size as i32;
        let (mut px, mut py) = match self.preferred_edge {
            PopoverEdge::Bottom => (
                anchor_x + (anchor_w as i32 - pop_w as i32) / 2,
                anchor_y + anchor_h as i32 + arrow,
            ),
            PopoverEdge::Top => (
                anchor_x + (anchor_w as i32 - pop_w as i32) / 2,
                anchor_y - pop_h as i32 - arrow,
            ),
            PopoverEdge::Right => (
                anchor_x + anchor_w as i32 + arrow,
                anchor_y + (anchor_h as i32 - pop_h as i32) / 2,
            ),
            PopoverEdge::Left => (
                anchor_x - pop_w as i32 - arrow,
                anchor_y + (anchor_h as i32 - pop_h as i32) / 2,
            ),
        };
        // Clamp to screen
        if px < 0 {
            px = 0;
        }
        if py < 0 {
            py = 0;
        }
        if px + pop_w as i32 > screen_w as i32 {
            px = screen_w as i32 - pop_w as i32;
        }
        if py + pop_h as i32 > screen_h as i32 {
            py = screen_h as i32 - pop_h as i32;
        }
        (px, py)
    }

    pub fn render(&self, canvas: &mut Canvas, x: i32, y: i32, w: u32, h: u32) {
        if !self.visible {
            return;
        }
        let ux = x as usize;
        let uy = y as usize;
        let bg = blend_colors(self.theme.chrome_bg, self.theme.window_bg, 220);
        canvas.fill_rect(ux, uy, w as usize, h as usize, bg);
        canvas.draw_rect_outline(ux, uy, w as usize, h as usize, self.theme.border);
    }

    pub fn handle_event(&mut self, event: &Event, x: i32, y: i32, w: u32, h: u32) {
        if !self.visible {
            return;
        }
        if let Event::MouseClick { x: mx, y: my } = event {
            if !hit(*mx, *my, x, y, w, h) {
                self.visible = false;
            }
        }
    }

    pub fn accessibility_role(&self) -> AccessibilityRole {
        AccessibilityRole::Dialog
    }
}

// ── Modal ───────────────────────────────────────────────────────────────

pub struct Modal {
    pub visible: bool,
    pub dismiss_on_backdrop: bool,
    pub backdrop_opacity: u8,
    pub theme: Theme,
}

impl Modal {
    pub fn new() -> Self {
        Self {
            visible: false,
            dismiss_on_backdrop: true,
            backdrop_opacity: 128,
            theme: default_theme(),
        }
    }

    pub fn show(&mut self) {
        self.visible = true;
    }
    pub fn dismiss(&mut self) {
        self.visible = false;
    }

    pub fn render(&self, canvas: &mut Canvas, x: i32, y: i32, w: u32, h: u32) {
        if !self.visible {
            return;
        }
        // Backdrop over the full provided area
        let backdrop = blend_colors(0xFF_00_00_00, self.theme.window_bg, self.backdrop_opacity);
        canvas.fill_rect(x as usize, y as usize, w as usize, h as usize, backdrop);
        // Centered modal box — 60% width, 50% height
        let mw = (w as usize) * 3 / 5;
        let mh = (h as usize) / 2;
        let mx = x as usize + (w as usize - mw) / 2;
        let my = y as usize + (h as usize - mh) / 2;
        canvas.fill_rect(mx, my, mw, mh, self.theme.window_bg);
        canvas.draw_rect_outline(mx, my, mw, mh, self.theme.border);
    }

    pub fn handle_event(&mut self, event: &Event, x: i32, y: i32, w: u32, h: u32) {
        if !self.visible {
            return;
        }
        if let Event::MouseClick { x: mx, y: my } = event {
            let mw = (w * 3 / 5) as i32;
            let mh = (h / 2) as i32;
            let bx = x + (w as i32 - mw) / 2;
            let by = y + (h as i32 - mh) / 2;
            if !hit(*mx, *my, bx, by, mw as u32, mh as u32) && self.dismiss_on_backdrop {
                self.visible = false;
            }
        }
        if let Event::KeyPress(27) = event {
            self.visible = false;
        }
    }

    pub fn content_rect(&self, x: i32, y: i32, w: u32, h: u32) -> (i32, i32, u32, u32) {
        let mw = w * 3 / 5;
        let mh = h / 2;
        let mx = x + (w as i32 - mw as i32) / 2;
        let my = y + (h as i32 - mh as i32) / 2;
        (mx + 8, my + 8, mw.saturating_sub(16), mh.saturating_sub(16))
    }

    pub fn accessibility_role(&self) -> AccessibilityRole {
        AccessibilityRole::Dialog
    }
}

// ── Drawer ──────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DrawerEdge {
    Left,
    Right,
    Bottom,
}

pub struct Drawer {
    pub edge: DrawerEdge,
    pub open: bool,
    /// 0.0 = closed, 1.0 = fully open.
    pub progress: f32,
    pub size: u32,
    pub theme: Theme,
}

impl Drawer {
    pub fn new(edge: DrawerEdge, size: u32) -> Self {
        Self {
            edge,
            open: false,
            progress: 0.0,
            size,
            theme: default_theme(),
        }
    }

    pub fn toggle(&mut self) {
        self.open = !self.open;
    }

    pub fn tick(&mut self, dt: f32) {
        let target = if self.open { 1.0 } else { 0.0 };
        let speed = 5.0;
        if self.progress < target {
            self.progress = clamp_f32(self.progress + dt * speed, 0.0, target);
        } else if self.progress > target {
            self.progress = clamp_f32(self.progress - dt * speed, target, 1.0);
        }
    }

    pub fn render(&self, canvas: &mut Canvas, x: i32, y: i32, w: u32, h: u32) {
        if self.progress < 0.001 {
            return;
        }
        let visible_size = (self.size as f32 * self.progress) as usize;
        if visible_size == 0 {
            return;
        }
        let backdrop = blend_colors(
            0xFF_00_00_00,
            self.theme.window_bg,
            (self.progress * 100.0) as u8,
        );
        canvas.fill_rect(x as usize, y as usize, w as usize, h as usize, backdrop);
        match self.edge {
            DrawerEdge::Left => {
                canvas.fill_rect(
                    x as usize,
                    y as usize,
                    visible_size,
                    h as usize,
                    self.theme.chrome_bg,
                );
                canvas.draw_rect_outline(
                    x as usize,
                    y as usize,
                    visible_size,
                    h as usize,
                    self.theme.border,
                );
            }
            DrawerEdge::Right => {
                let dx = x as usize + w as usize - visible_size;
                canvas.fill_rect(
                    dx,
                    y as usize,
                    visible_size,
                    h as usize,
                    self.theme.chrome_bg,
                );
                canvas.draw_rect_outline(
                    dx,
                    y as usize,
                    visible_size,
                    h as usize,
                    self.theme.border,
                );
            }
            DrawerEdge::Bottom => {
                let dy = y as usize + h as usize - visible_size;
                canvas.fill_rect(
                    x as usize,
                    dy,
                    w as usize,
                    visible_size,
                    self.theme.chrome_bg,
                );
                canvas.draw_rect_outline(
                    x as usize,
                    dy,
                    w as usize,
                    visible_size,
                    self.theme.border,
                );
            }
        }
    }

    pub fn handle_event(&mut self, event: &Event, _x: i32, _y: i32, _w: u32, _h: u32) {
        if let Event::KeyPress(27) = event {
            if self.open {
                self.open = false;
            }
        }
    }

    pub fn accessibility_role(&self) -> AccessibilityRole {
        AccessibilityRole::Dialog
    }
}

// ── ContextMenu ─────────────────────────────────────────────────────────

pub struct MenuItem {
    pub label: String,
    pub shortcut: Option<String>,
    pub disabled: bool,
    pub submenu: Option<Vec<MenuItem>>,
}

impl MenuItem {
    pub fn new(label: &str) -> Self {
        Self {
            label: String::from(label),
            shortcut: None,
            disabled: false,
            submenu: None,
        }
    }

    pub fn with_shortcut(mut self, sc: &str) -> Self {
        self.shortcut = Some(String::from(sc));
        self
    }

    pub fn with_submenu(mut self, items: Vec<MenuItem>) -> Self {
        self.submenu = Some(items);
        self
    }
}

pub struct ContextMenu {
    pub items: Vec<MenuItem>,
    pub visible: bool,
    pub selected: Option<usize>,
    pub item_height: u32,
    pub theme: Theme,
}

impl ContextMenu {
    pub fn new(items: Vec<MenuItem>) -> Self {
        Self {
            items,
            visible: false,
            selected: None,
            item_height: 22,
            theme: default_theme(),
        }
    }

    pub fn show(&mut self) {
        self.visible = true;
        self.selected = None;
    }
    pub fn hide(&mut self) {
        self.visible = false;
        self.selected = None;
    }

    pub fn render(&self, canvas: &mut Canvas, x: i32, y: i32, w: u32, _h: u32) {
        if !self.visible {
            return;
        }
        let total_h = self.items.len() as usize * self.item_height as usize;
        let ux = x as usize;
        let uy = y as usize;
        let uw = w as usize;
        canvas.fill_rect(ux, uy, uw, total_h, self.theme.chrome_bg);
        canvas.draw_rect_outline(ux, uy, uw, total_h, self.theme.border);
        for (i, item) in self.items.iter().enumerate() {
            let iy = uy + i * self.item_height as usize;
            let is_sel = self.selected == Some(i);
            if is_sel {
                canvas.fill_rect(
                    ux + 1,
                    iy,
                    uw - 2,
                    self.item_height as usize,
                    self.theme.button_bg,
                );
            }
            let fg = if item.disabled {
                0xFF_66_66_88
            } else if is_sel {
                self.theme.title_fg
            } else {
                self.theme.text_fg
            };
            canvas.draw_text(
                ux + 8,
                iy + (self.item_height as usize - 8) / 2,
                &item.label,
                fg,
                None,
            );
            if let Some(ref sc) = item.shortcut {
                let sx = ux + uw - sc.len() * 8 - 8;
                canvas.draw_text(
                    sx,
                    iy + (self.item_height as usize - 8) / 2,
                    sc,
                    0xFF_66_66_88,
                    None,
                );
            }
            if item.submenu.is_some() {
                canvas.draw_text(
                    ux + uw - 14,
                    iy + (self.item_height as usize - 8) / 2,
                    ">",
                    fg,
                    None,
                );
            }
        }
    }

    pub fn handle_event(
        &mut self,
        event: &Event,
        x: i32,
        y: i32,
        w: u32,
        _h: u32,
    ) -> Option<usize> {
        if !self.visible {
            return None;
        }
        match event {
            Event::MouseClick { x: mx, y: my } => {
                let total_h = self.items.len() as u32 * self.item_height;
                if hit(*mx, *my, x, y, w, total_h) {
                    let idx = ((*my - y) as u32 / self.item_height) as usize;
                    if idx < self.items.len() && !self.items[idx].disabled {
                        self.visible = false;
                        return Some(idx);
                    }
                } else {
                    self.visible = false;
                }
            }
            Event::MouseMove { .. } => {
                // Would need absolute position tracking; stub for now
            }
            Event::KeyPress(k) => match *k {
                72 => {
                    let len = self.items.len();
                    if len > 0 {
                        self.selected = Some(match self.selected {
                            Some(0) | None => len - 1,
                            Some(i) => i - 1,
                        });
                    }
                }
                80 => {
                    let len = self.items.len();
                    if len > 0 {
                        self.selected = Some(match self.selected {
                            Some(i) if i + 1 < len => i + 1,
                            _ => 0,
                        });
                    }
                }
                13 => {
                    if let Some(idx) = self.selected {
                        if idx < self.items.len() && !self.items[idx].disabled {
                            self.visible = false;
                            return Some(idx);
                        }
                    }
                }
                27 => {
                    self.visible = false;
                }
                _ => {}
            },
            _ => {}
        }
        None
    }

    pub fn accessibility_role(&self) -> AccessibilityRole {
        AccessibilityRole::Menu
    }
}
