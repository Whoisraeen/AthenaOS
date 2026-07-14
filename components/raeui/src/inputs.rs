//! Advanced input widgets for AthUI.
//!
//! TextArea, NumberInput, Checkbox (tri-state), RadioGroup, DropdownSelect,
//! MultiSelect, DatePicker, TimePicker, ColorPicker, FileInput, SearchInput,
//! PasswordInput, RangeSlider — all theme-aware and keyboard-navigable.

extern crate alloc;

use crate::accessibility::AccessibilityRole;
use crate::{default_theme, Event, Theme};
use alloc::string::String;
use alloc::vec::Vec;
use raegfx::Canvas;

fn hit(mx: i32, my: i32, x: i32, y: i32, w: u32, h: u32) -> bool {
    mx >= x && mx < x + w as i32 && my >= y && my < y + h as i32
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

fn i32_to_decimal(val: i32, buf: &mut [u8; 12]) -> usize {
    let negative = val < 0;
    let mut v = if negative {
        (-(val as i64)) as u32
    } else {
        val as u32
    };
    let mut pos = 12usize;
    if v == 0 {
        buf[0] = b'0';
        return 1;
    }
    while v > 0 && pos > 0 {
        pos -= 1;
        buf[pos] = b'0' + (v % 10) as u8;
        v /= 10;
    }
    if negative && pos > 0 {
        pos -= 1;
        buf[pos] = b'-';
    }
    let len = 12 - pos;
    buf.copy_within(pos..12, 0);
    len
}

// ── TextArea ────────────────────────────────────────────────────────────

pub struct TextArea {
    pub lines: Vec<String>,
    pub cursor_line: usize,
    pub cursor_col: usize,
    pub scroll_offset: usize,
    pub focused: bool,
    pub line_height: u32,
    pub theme: Theme,
}

impl TextArea {
    pub fn new() -> Self {
        Self {
            lines: {
                let mut v = Vec::new();
                v.push(String::new());
                v
            },
            cursor_line: 0,
            cursor_col: 0,
            scroll_offset: 0,
            focused: false,
            line_height: 16,
            theme: default_theme(),
        }
    }

    pub fn set_text(&mut self, text: &str) {
        self.lines.clear();
        for line in text.split('\n') {
            self.lines.push(String::from(line));
        }
        if self.lines.is_empty() {
            self.lines.push(String::new());
        }
        self.cursor_line = 0;
        self.cursor_col = 0;
    }

    pub fn insert_char(&mut self, ch: char) {
        if ch == '\n' {
            let rest = if self.cursor_col < self.lines[self.cursor_line].len() {
                String::from(&self.lines[self.cursor_line][self.cursor_col..])
            } else {
                String::new()
            };
            self.lines[self.cursor_line].truncate(self.cursor_col);
            self.cursor_line += 1;
            self.lines.insert(self.cursor_line, rest);
            self.cursor_col = 0;
        } else if self.cursor_line < self.lines.len() {
            let line = &mut self.lines[self.cursor_line];
            if self.cursor_col <= line.len() {
                line.insert(self.cursor_col, ch);
                self.cursor_col += 1;
            }
        }
    }

    pub fn delete_char(&mut self) {
        if self.cursor_col > 0 && self.cursor_line < self.lines.len() {
            self.cursor_col -= 1;
            self.lines[self.cursor_line].remove(self.cursor_col);
        } else if self.cursor_line > 0 {
            let current = self.lines.remove(self.cursor_line);
            self.cursor_line -= 1;
            self.cursor_col = self.lines[self.cursor_line].len();
            self.lines[self.cursor_line].push_str(&current);
        }
    }

    fn visible_lines(&self, h: u32) -> usize {
        if self.line_height == 0 {
            return 0;
        }
        h as usize / self.line_height as usize
    }

    fn ensure_cursor_visible(&mut self, h: u32) {
        let vis = self.visible_lines(h);
        if vis == 0 {
            return;
        }
        if self.cursor_line < self.scroll_offset {
            self.scroll_offset = self.cursor_line;
        } else if self.cursor_line >= self.scroll_offset + vis {
            self.scroll_offset = self.cursor_line - vis + 1;
        }
    }

    pub fn render(&self, canvas: &mut Canvas, x: i32, y: i32, w: u32, h: u32) {
        let ux = x as usize;
        let uy = y as usize;
        let bg = if self.focused {
            0xFF_22_22_38
        } else {
            0xFF_1A_1A_2A
        };
        canvas.fill_rect(ux, uy, w as usize, h as usize, bg);
        canvas.draw_rect_outline(ux, uy, w as usize, h as usize, self.theme.border);

        let vis = self.visible_lines(h);
        let end = (self.scroll_offset + vis).min(self.lines.len());
        for i in self.scroll_offset..end {
            let ly = uy + (i - self.scroll_offset) * self.line_height as usize;
            let fg = self.theme.text_fg;
            canvas.draw_text(ux + 4, ly + 2, &self.lines[i], fg, None);
        }

        if self.focused && self.cursor_line >= self.scroll_offset && self.cursor_line < end {
            let cy = uy + (self.cursor_line - self.scroll_offset) * self.line_height as usize;
            let cx = ux + 4 + self.cursor_col * 8;
            for dy in 0..self.line_height as usize {
                if cx < ux + w as usize {
                    canvas.draw_pixel(cx, cy + dy, self.theme.button_hot);
                }
            }
        }

        if self.focused {
            canvas.draw_rect_outline(ux, uy, w as usize, h as usize, self.theme.button_hot);
        }
    }

    pub fn handle_event(&mut self, event: &Event, x: i32, y: i32, w: u32, h: u32) {
        match event {
            Event::MouseClick { x: mx, y: my } => {
                self.focused = hit(*mx, *my, x, y, w, h);
                if self.focused {
                    let rel_y = (*my - y) as usize;
                    let line_idx = self.scroll_offset + rel_y / self.line_height as usize;
                    if line_idx < self.lines.len() {
                        self.cursor_line = line_idx;
                        let rel_x = ((*mx - x) as usize).saturating_sub(4);
                        self.cursor_col = (rel_x / 8).min(self.lines[self.cursor_line].len());
                    }
                }
            }
            Event::KeyPress(k) if self.focused => {
                match *k {
                    8 => self.delete_char(),
                    13 => self.insert_char('\n'),
                    72 => {
                        if self.cursor_line > 0 {
                            self.cursor_line -= 1;
                            self.cursor_col =
                                self.cursor_col.min(self.lines[self.cursor_line].len());
                        }
                    }
                    80 => {
                        if self.cursor_line + 1 < self.lines.len() {
                            self.cursor_line += 1;
                            self.cursor_col =
                                self.cursor_col.min(self.lines[self.cursor_line].len());
                        }
                    }
                    75 => {
                        if self.cursor_col > 0 {
                            self.cursor_col -= 1;
                        }
                    }
                    77 => {
                        if self.cursor_line < self.lines.len()
                            && self.cursor_col < self.lines[self.cursor_line].len()
                        {
                            self.cursor_col += 1;
                        }
                    }
                    c if c >= 32 => self.insert_char(c as char),
                    _ => {}
                }
                self.ensure_cursor_visible(h);
            }
            _ => {}
        }
    }

    pub fn accessibility_role(&self) -> AccessibilityRole {
        AccessibilityRole::TextField
    }
}

// ── NumberInput ─────────────────────────────────────────────────────────

pub struct NumberInput {
    pub value: i32,
    pub min: i32,
    pub max: i32,
    pub step: i32,
    pub focused: bool,
    pub theme: Theme,
}

impl NumberInput {
    pub fn new(min: i32, max: i32, step: i32) -> Self {
        Self {
            value: min,
            min,
            max,
            step,
            focused: false,
            theme: default_theme(),
        }
    }

    pub fn increment(&mut self) {
        self.value = (self.value + self.step).min(self.max);
    }

    pub fn decrement(&mut self) {
        self.value = (self.value - self.step).max(self.min);
    }

    pub fn render(&self, canvas: &mut Canvas, x: i32, y: i32, w: u32, h: u32) {
        let ux = x as usize;
        let uy = y as usize;
        let btn_w = h as usize;
        let field_w = (w as usize).saturating_sub(btn_w * 2);

        // Decrement button
        canvas.fill_rect(ux, uy, btn_w, h as usize, self.theme.button_bg);
        canvas.draw_rect_outline(ux, uy, btn_w, h as usize, self.theme.border);
        canvas.draw_text(
            ux + btn_w / 2 - 4,
            uy + (h as usize - 8) / 2,
            "-",
            self.theme.button_text,
            None,
        );

        // Value field
        let fx = ux + btn_w;
        let bg = if self.focused {
            0xFF_22_22_38
        } else {
            0xFF_1A_1A_2A
        };
        canvas.fill_rect(fx, uy, field_w, h as usize, bg);
        canvas.draw_rect_outline(fx, uy, field_w, h as usize, self.theme.border);

        let mut buf = [0u8; 12];
        let len = i32_to_decimal(self.value, &mut buf);
        let val_str = unsafe { core::str::from_utf8_unchecked(&buf[..len]) };
        let tx = fx + (field_w - val_str.len() * 8) / 2;
        canvas.draw_text(
            tx,
            uy + (h as usize - 8) / 2,
            val_str,
            self.theme.text_fg,
            None,
        );

        // Increment button
        let ix = fx + field_w;
        canvas.fill_rect(ix, uy, btn_w, h as usize, self.theme.button_bg);
        canvas.draw_rect_outline(ix, uy, btn_w, h as usize, self.theme.border);
        canvas.draw_text(
            ix + btn_w / 2 - 4,
            uy + (h as usize - 8) / 2,
            "+",
            self.theme.button_text,
            None,
        );

        if self.focused {
            canvas.draw_rect_outline(ux, uy, w as usize, h as usize, self.theme.button_hot);
        }
    }

    pub fn handle_event(&mut self, event: &Event, x: i32, y: i32, w: u32, h: u32) {
        let btn_w = h;
        match event {
            Event::MouseClick { x: mx, y: my } => {
                self.focused = hit(*mx, *my, x, y, w, h);
                if hit(*mx, *my, x, y, btn_w, h) {
                    self.decrement();
                } else if hit(*mx, *my, x + w as i32 - btn_w as i32, y, btn_w, h) {
                    self.increment();
                }
            }
            Event::KeyPress(k) if self.focused => match *k {
                72 => self.increment(),
                80 => self.decrement(),
                _ => {}
            },
            _ => {}
        }
    }

    pub fn accessibility_role(&self) -> AccessibilityRole {
        AccessibilityRole::Slider
    }
}

// ── CheckState (tri-state) ──────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckState {
    Unchecked,
    Checked,
    Indeterminate,
}

pub struct TriCheckbox {
    pub label: String,
    pub state: CheckState,
    pub focused: bool,
    pub theme: Theme,
}

impl TriCheckbox {
    pub fn new(label: &str) -> Self {
        Self {
            label: String::from(label),
            state: CheckState::Unchecked,
            focused: false,
            theme: default_theme(),
        }
    }

    pub fn toggle(&mut self) {
        self.state = match self.state {
            CheckState::Unchecked => CheckState::Checked,
            CheckState::Checked => CheckState::Unchecked,
            CheckState::Indeterminate => CheckState::Checked,
        };
    }

    pub fn render(&self, canvas: &mut Canvas, x: i32, y: i32, _w: u32, h: u32) {
        let ux = x as usize;
        let uy = y as usize;
        let box_s = (h as usize).min(16);
        let border_col = match self.state {
            CheckState::Checked => self.theme.button_hot,
            CheckState::Indeterminate => self.theme.border,
            CheckState::Unchecked => self.theme.border,
        };
        canvas.fill_rect(ux, uy, box_s, box_s, 0xFF_1E_1E_2E);
        canvas.draw_rect_outline(ux, uy, box_s, box_s, border_col);
        match self.state {
            CheckState::Checked => {
                for i in 0..4usize {
                    let px = ux + 3 + i;
                    let py = uy + box_s / 2 + i;
                    if py < uy + box_s {
                        canvas.draw_pixel(px, py, self.theme.button_hot);
                    }
                }
                for i in 0..6usize {
                    let px = ux + 6 + i;
                    let py = uy + box_s / 2 + 3 - i;
                    if py >= uy && py < uy + box_s {
                        canvas.draw_pixel(px, py, self.theme.button_hot);
                    }
                }
            }
            CheckState::Indeterminate => {
                let bar_y = uy + box_s / 2 - 1;
                canvas.fill_rect(ux + 3, bar_y, box_s - 6, 2, self.theme.border);
            }
            CheckState::Unchecked => {}
        }
        let label_x = ux + box_s + 6;
        let label_y = uy + (box_s.saturating_sub(8)) / 2;
        canvas.draw_text(label_x, label_y, &self.label, self.theme.text_fg, None);
        if self.focused {
            canvas.draw_rect_outline(ux, uy, box_s, box_s, self.theme.button_hot);
        }
    }

    pub fn handle_event(&mut self, event: &Event, x: i32, y: i32, w: u32, h: u32) {
        match event {
            Event::MouseClick { x: mx, y: my } => {
                if hit(*mx, *my, x, y, w, h) {
                    self.toggle();
                    self.focused = true;
                } else {
                    self.focused = false;
                }
            }
            Event::KeyPress(32) if self.focused => self.toggle(),
            _ => {}
        }
    }

    pub fn accessibility_role(&self) -> AccessibilityRole {
        AccessibilityRole::Checkbox
    }
    pub fn accessibility_label(&self) -> &str {
        &self.label
    }
}

// ── RadioGroup ──────────────────────────────────────────────────────────

pub struct RadioGroup {
    pub options: Vec<String>,
    pub selected: usize,
    pub focused: bool,
    pub focus_idx: usize,
    pub item_height: u32,
    pub theme: Theme,
}

impl RadioGroup {
    pub fn new(options: &[&str]) -> Self {
        let mut v = Vec::new();
        for o in options {
            v.push(String::from(*o));
        }
        Self {
            options: v,
            selected: 0,
            focused: false,
            focus_idx: 0,
            item_height: 20,
            theme: default_theme(),
        }
    }

    pub fn render(&self, canvas: &mut Canvas, x: i32, y: i32, w: u32, _h: u32) {
        let ux = x as usize;
        let uy = y as usize;
        let r = 6usize;
        for (i, opt) in self.options.iter().enumerate() {
            let iy = uy + i * self.item_height as usize;
            let cx = ux + r;
            let cy = iy + self.item_height as usize / 2;
            // Outer circle
            for dy in 0..r * 2 {
                for dx in 0..r * 2 {
                    let rx = dx as i32 - r as i32;
                    let ry = dy as i32 - r as i32;
                    if rx * rx + ry * ry <= (r as i32) * (r as i32) {
                        let px = (cx as i32 + rx) as usize;
                        let py = (cy as i32 + ry) as usize;
                        canvas.draw_pixel(px, py, self.theme.border);
                    }
                }
            }
            // Inner dot for selected
            if i == self.selected {
                let ir = 3usize;
                for dy in 0..ir * 2 {
                    for dx in 0..ir * 2 {
                        let rx = dx as i32 - ir as i32;
                        let ry = dy as i32 - ir as i32;
                        if rx * rx + ry * ry <= (ir as i32) * (ir as i32) {
                            let px = (cx as i32 + rx) as usize;
                            let py = (cy as i32 + ry) as usize;
                            canvas.draw_pixel(px, py, self.theme.button_hot);
                        }
                    }
                }
            }
            // Focus ring on current
            if self.focused && i == self.focus_idx {
                canvas.draw_rect_outline(
                    ux,
                    iy,
                    w as usize,
                    self.item_height as usize,
                    self.theme.button_hot,
                );
            }
            let tx = ux + r * 2 + 8;
            canvas.draw_text(
                tx,
                iy + (self.item_height as usize - 8) / 2,
                opt,
                self.theme.text_fg,
                None,
            );
        }
    }

    pub fn handle_event(&mut self, event: &Event, x: i32, y: i32, w: u32, _h: u32) {
        match event {
            Event::MouseClick { x: mx, y: my } => {
                for (i, _) in self.options.iter().enumerate() {
                    let iy = y + i as i32 * self.item_height as i32;
                    if hit(*mx, *my, x, iy, w, self.item_height) {
                        self.selected = i;
                        self.focus_idx = i;
                        self.focused = true;
                        return;
                    }
                }
                self.focused = false;
            }
            Event::KeyPress(k) if self.focused => match *k {
                72 => {
                    if self.focus_idx > 0 {
                        self.focus_idx -= 1;
                        self.selected = self.focus_idx;
                    }
                }
                80 => {
                    if self.focus_idx + 1 < self.options.len() {
                        self.focus_idx += 1;
                        self.selected = self.focus_idx;
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }

    pub fn accessibility_role(&self) -> AccessibilityRole {
        AccessibilityRole::Group
    }
}

// ── DropdownSelect ──────────────────────────────────────────────────────

pub struct DropdownSelect {
    pub options: Vec<String>,
    pub selected: usize,
    pub open: bool,
    pub focused: bool,
    pub filter: String,
    pub item_height: u32,
    pub max_visible: usize,
    pub theme: Theme,
}

impl DropdownSelect {
    pub fn new(options: &[&str]) -> Self {
        let mut v = Vec::new();
        for o in options {
            v.push(String::from(*o));
        }
        Self {
            options: v,
            selected: 0,
            open: false,
            focused: false,
            filter: String::new(),
            item_height: 22,
            max_visible: 6,
            theme: default_theme(),
        }
    }

    fn filtered_indices(&self) -> Vec<usize> {
        if self.filter.is_empty() {
            (0..self.options.len()).collect()
        } else {
            let mut out = Vec::new();
            for (i, opt) in self.options.iter().enumerate() {
                if opt
                    .as_bytes()
                    .iter()
                    .zip(self.filter.as_bytes().iter())
                    .all(|(a, b)| a.to_ascii_lowercase() == b.to_ascii_lowercase())
                    || opt.len() >= self.filter.len()
                {
                    out.push(i);
                }
            }
            out
        }
    }

    pub fn render(&self, canvas: &mut Canvas, x: i32, y: i32, w: u32, h: u32) {
        let ux = x as usize;
        let uy = y as usize;
        let bg = if self.focused {
            0xFF_22_22_38
        } else {
            0xFF_1A_1A_2A
        };
        canvas.fill_rect(ux, uy, w as usize, h as usize, bg);
        canvas.draw_rect_outline(ux, uy, w as usize, h as usize, self.theme.border);
        // Current value
        if self.selected < self.options.len() {
            canvas.draw_text(
                ux + 6,
                uy + (h as usize - 8) / 2,
                &self.options[self.selected],
                self.theme.text_fg,
                None,
            );
        }
        // Arrow
        let arrow = if self.open { "^" } else { "v" };
        canvas.draw_text(
            ux + w as usize - 14,
            uy + (h as usize - 8) / 2,
            arrow,
            self.theme.text_fg,
            None,
        );
        if self.focused {
            canvas.draw_rect_outline(ux, uy, w as usize, h as usize, self.theme.button_hot);
        }
        // Dropdown list
        if self.open {
            let indices = self.filtered_indices();
            let show = indices.len().min(self.max_visible);
            let list_y = uy + h as usize;
            let list_h = show * self.item_height as usize;
            canvas.fill_rect(ux, list_y, w as usize, list_h, self.theme.chrome_bg);
            canvas.draw_rect_outline(ux, list_y, w as usize, list_h, self.theme.border);
            for (vi, &idx) in indices.iter().take(show).enumerate() {
                let iy = list_y + vi * self.item_height as usize;
                let is_sel = idx == self.selected;
                if is_sel {
                    canvas.fill_rect(
                        ux + 1,
                        iy,
                        w as usize - 2,
                        self.item_height as usize,
                        self.theme.button_bg,
                    );
                }
                let fg = if is_sel {
                    self.theme.title_fg
                } else {
                    self.theme.text_fg
                };
                canvas.draw_text(
                    ux + 6,
                    iy + (self.item_height as usize - 8) / 2,
                    &self.options[idx],
                    fg,
                    None,
                );
            }
        }
    }

    pub fn handle_event(&mut self, event: &Event, x: i32, y: i32, w: u32, h: u32) {
        match event {
            Event::MouseClick { x: mx, y: my } => {
                if hit(*mx, *my, x, y, w, h) {
                    self.open = !self.open;
                    self.focused = true;
                } else if self.open {
                    let indices = self.filtered_indices();
                    let show = indices.len().min(self.max_visible);
                    let list_y = y + h as i32;
                    let list_h = show as u32 * self.item_height;
                    if hit(*mx, *my, x, list_y, w, list_h) {
                        let vi = ((*my - list_y) as u32 / self.item_height) as usize;
                        if vi < indices.len() {
                            self.selected = indices[vi];
                        }
                        self.open = false;
                    } else {
                        self.open = false;
                        self.focused = false;
                    }
                } else {
                    self.focused = false;
                }
            }
            Event::KeyPress(k) if self.focused => match *k {
                13 | 32 => {
                    self.open = !self.open;
                }
                27 => {
                    self.open = false;
                }
                72 => {
                    if self.selected > 0 {
                        self.selected -= 1;
                    }
                }
                80 => {
                    if self.selected + 1 < self.options.len() {
                        self.selected += 1;
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }

    pub fn accessibility_role(&self) -> AccessibilityRole {
        AccessibilityRole::Menu
    }
}

// ── MultiSelect ─────────────────────────────────────────────────────────

pub struct MultiSelect {
    pub options: Vec<String>,
    pub selected: Vec<bool>,
    pub focused: bool,
    pub focus_idx: usize,
    pub tag_height: u32,
    pub theme: Theme,
}

impl MultiSelect {
    pub fn new(options: &[&str]) -> Self {
        let len = options.len();
        let mut v = Vec::new();
        for o in options {
            v.push(String::from(*o));
        }
        Self {
            options: v,
            selected: {
                let mut s = Vec::new();
                s.resize(len, false);
                s
            },
            focused: false,
            focus_idx: 0,
            tag_height: 20,
            theme: default_theme(),
        }
    }

    pub fn selected_labels(&self) -> Vec<&str> {
        let mut out = Vec::new();
        for (i, sel) in self.selected.iter().enumerate() {
            if *sel {
                out.push(self.options[i].as_str());
            }
        }
        out
    }

    pub fn render(&self, canvas: &mut Canvas, x: i32, y: i32, w: u32, h: u32) {
        let ux = x as usize;
        let uy = y as usize;
        canvas.fill_rect(ux, uy, w as usize, h as usize, 0xFF_1A_1A_2A);
        canvas.draw_rect_outline(ux, uy, w as usize, h as usize, self.theme.border);
        // Tags for selected items
        let mut tx = ux + 4;
        let tag_y = uy + 2;
        let th = self.tag_height as usize - 4;
        for (i, opt) in self.options.iter().enumerate() {
            if !self.selected[i] {
                continue;
            }
            let tw = opt.len() * 8 + 20;
            if tx + tw > ux + w as usize {
                break;
            }
            canvas.fill_rect(tx, tag_y, tw, th, self.theme.button_bg);
            canvas.draw_text(tx + 4, tag_y + (th - 8) / 2, opt, self.theme.text_fg, None);
            canvas.draw_text(
                tx + tw - 12,
                tag_y + (th - 8) / 2,
                "x",
                self.theme.button_hot,
                None,
            );
            tx += tw + 4;
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
            Event::KeyPress(k) if self.focused => match *k {
                72 => {
                    if self.focus_idx > 0 {
                        self.focus_idx -= 1;
                    }
                }
                80 => {
                    if self.focus_idx + 1 < self.options.len() {
                        self.focus_idx += 1;
                    }
                }
                32 => {
                    if self.focus_idx < self.selected.len() {
                        self.selected[self.focus_idx] = !self.selected[self.focus_idx];
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }

    pub fn accessibility_role(&self) -> AccessibilityRole {
        AccessibilityRole::List
    }
}

// ── DatePicker ──────────────────────────────────────────────────────────

pub struct DatePicker {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub open: bool,
    pub focused: bool,
    pub theme: Theme,
}

impl DatePicker {
    pub fn new(year: u16, month: u8, day: u8) -> Self {
        Self {
            year,
            month,
            day,
            open: false,
            focused: false,
            theme: default_theme(),
        }
    }

    fn days_in_month(&self) -> u8 {
        match self.month {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            4 | 6 | 9 | 11 => 30,
            2 => {
                if self.year % 4 == 0 && (self.year % 100 != 0 || self.year % 400 == 0) {
                    29
                } else {
                    28
                }
            }
            _ => 30,
        }
    }

    pub fn render(&self, canvas: &mut Canvas, x: i32, y: i32, w: u32, h: u32) {
        let ux = x as usize;
        let uy = y as usize;
        canvas.fill_rect(ux, uy, w as usize, h as usize, 0xFF_1A_1A_2A);
        canvas.draw_rect_outline(ux, uy, w as usize, h as usize, self.theme.border);

        let mut ybuf = [0u8; 10];
        let ystr = u32_to_decimal(self.year as u32, &mut ybuf);
        let mut mbuf = [0u8; 10];
        let mstr = u32_to_decimal(self.month as u32, &mut mbuf);
        let mut dbuf = [0u8; 10];
        let dstr = u32_to_decimal(self.day as u32, &mut dbuf);

        let mut display = String::new();
        display.push_str(ystr);
        display.push('-');
        if self.month < 10 {
            display.push('0');
        }
        display.push_str(mstr);
        display.push('-');
        if self.day < 10 {
            display.push('0');
        }
        display.push_str(dstr);

        canvas.draw_text(
            ux + 6,
            uy + (h as usize - 8) / 2,
            &display,
            self.theme.text_fg,
            None,
        );

        if self.open {
            let cal_y = uy + h as usize;
            let cell_w = w as usize / 7;
            let rows = 6usize;
            let cal_h = rows * 16 + 20;
            canvas.fill_rect(ux, cal_y, w as usize, cal_h, self.theme.chrome_bg);
            canvas.draw_rect_outline(ux, cal_y, w as usize, cal_h, self.theme.border);
            let days = [" S", " M", " T", " W", " T", " F", " S"];
            for (i, d) in days.iter().enumerate() {
                canvas.draw_text(ux + i * cell_w + 2, cal_y + 4, d, self.theme.title_fg, None);
            }
            let dim = self.days_in_month();
            for d in 0..dim as usize {
                let col = (d) % 7;
                let row = (d) / 7;
                let cx = ux + col * cell_w + 2;
                let cy = cal_y + 20 + row * 16;
                let mut nbuf = [0u8; 10];
                let ns = u32_to_decimal((d + 1) as u32, &mut nbuf);
                let fg = if d + 1 == self.day as usize {
                    self.theme.button_hot
                } else {
                    self.theme.text_fg
                };
                canvas.draw_text(cx, cy, ns, fg, None);
            }
        }

        if self.focused {
            canvas.draw_rect_outline(ux, uy, w as usize, h as usize, self.theme.button_hot);
        }
    }

    pub fn handle_event(&mut self, event: &Event, x: i32, y: i32, w: u32, h: u32) {
        match event {
            Event::MouseClick { x: mx, y: my } => {
                if hit(*mx, *my, x, y, w, h) {
                    self.open = !self.open;
                    self.focused = true;
                } else if self.open {
                    let cal_y = y + h as i32;
                    let cell_w = w / 7;
                    if *my >= cal_y + 20 {
                        let col = ((*mx - x) as u32 / cell_w) as u8;
                        let row = ((*my - cal_y - 20) / 16) as u8;
                        let d = row * 7 + col + 1;
                        if d >= 1 && d <= self.days_in_month() {
                            self.day = d;
                        }
                        self.open = false;
                    }
                } else {
                    self.focused = false;
                }
            }
            Event::KeyPress(27) if self.open => {
                self.open = false;
            }
            _ => {}
        }
    }

    pub fn accessibility_role(&self) -> AccessibilityRole {
        AccessibilityRole::Button
    }
    pub fn accessibility_label(&self) -> String {
        let mut s = String::new();
        let mut ybuf = [0u8; 10];
        s.push_str(u32_to_decimal(self.year as u32, &mut ybuf));
        s.push('-');
        let mut mbuf = [0u8; 10];
        s.push_str(u32_to_decimal(self.month as u32, &mut mbuf));
        s.push('-');
        let mut dbuf = [0u8; 10];
        s.push_str(u32_to_decimal(self.day as u32, &mut dbuf));
        s
    }
}

// ── TimePicker ──────────────────────────────────────────────────────────

pub struct TimePicker {
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
    pub use_24h: bool,
    pub focused: bool,
    pub edit_field: u8,
    pub theme: Theme,
}

impl TimePicker {
    pub fn new(use_24h: bool) -> Self {
        Self {
            hour: 0,
            minute: 0,
            second: 0,
            use_24h,
            focused: false,
            edit_field: 0,
            theme: default_theme(),
        }
    }

    pub fn render(&self, canvas: &mut Canvas, x: i32, y: i32, w: u32, h: u32) {
        let ux = x as usize;
        let uy = y as usize;
        canvas.fill_rect(ux, uy, w as usize, h as usize, 0xFF_1A_1A_2A);
        canvas.draw_rect_outline(ux, uy, w as usize, h as usize, self.theme.border);

        let display_h = if !self.use_24h && self.hour > 12 {
            self.hour - 12
        } else {
            self.hour
        };
        let mut buf = String::new();
        if display_h < 10 {
            buf.push('0');
        }
        let mut nbuf = [0u8; 10];
        buf.push_str(u32_to_decimal(display_h as u32, &mut nbuf));
        buf.push(':');
        if self.minute < 10 {
            buf.push('0');
        }
        let mut nbuf2 = [0u8; 10];
        buf.push_str(u32_to_decimal(self.minute as u32, &mut nbuf2));
        buf.push(':');
        if self.second < 10 {
            buf.push('0');
        }
        let mut nbuf3 = [0u8; 10];
        buf.push_str(u32_to_decimal(self.second as u32, &mut nbuf3));
        if !self.use_24h {
            buf.push(' ');
            buf.push_str(if self.hour < 12 { "AM" } else { "PM" });
        }

        canvas.draw_text(
            ux + 6,
            uy + (h as usize - 8) / 2,
            &buf,
            self.theme.text_fg,
            None,
        );

        // Highlight active field
        if self.focused {
            let field_x = ux + 6 + self.edit_field as usize * 24;
            canvas.draw_rect_outline(field_x, uy + 2, 16, h as usize - 4, self.theme.button_hot);
        }
    }

    pub fn handle_event(&mut self, event: &Event, x: i32, y: i32, w: u32, h: u32) {
        match event {
            Event::MouseClick { x: mx, y: my } => {
                self.focused = hit(*mx, *my, x, y, w, h);
            }
            Event::KeyPress(k) if self.focused => match *k {
                75 => {
                    if self.edit_field > 0 {
                        self.edit_field -= 1;
                    }
                }
                77 => {
                    if self.edit_field < 2 {
                        self.edit_field += 1;
                    }
                }
                72 => match self.edit_field {
                    0 => {
                        self.hour = if self.hour >= 23 { 0 } else { self.hour + 1 };
                    }
                    1 => {
                        self.minute = if self.minute >= 59 {
                            0
                        } else {
                            self.minute + 1
                        };
                    }
                    2 => {
                        self.second = if self.second >= 59 {
                            0
                        } else {
                            self.second + 1
                        };
                    }
                    _ => {}
                },
                80 => match self.edit_field {
                    0 => {
                        self.hour = if self.hour == 0 { 23 } else { self.hour - 1 };
                    }
                    1 => {
                        self.minute = if self.minute == 0 {
                            59
                        } else {
                            self.minute - 1
                        };
                    }
                    2 => {
                        self.second = if self.second == 0 {
                            59
                        } else {
                            self.second - 1
                        };
                    }
                    _ => {}
                },
                _ => {}
            },
            _ => {}
        }
    }

    pub fn accessibility_role(&self) -> AccessibilityRole {
        AccessibilityRole::Slider
    }
}

// ── ColorPicker ─────────────────────────────────────────────────────────

pub struct ColorPicker {
    pub hue: u16,
    pub saturation: u8,
    pub value: u8,
    pub alpha: u8,
    pub focused: bool,
    pub theme: Theme,
}

impl ColorPicker {
    pub fn new() -> Self {
        Self {
            hue: 0,
            saturation: 100,
            value: 100,
            alpha: 255,
            focused: false,
            theme: default_theme(),
        }
    }

    pub fn to_argb(&self) -> u32 {
        let (r, g, b) = hsv_to_rgb(self.hue, self.saturation, self.value);
        (self.alpha as u32) << 24 | (r as u32) << 16 | (g as u32) << 8 | b as u32
    }

    pub fn render(&self, canvas: &mut Canvas, x: i32, y: i32, w: u32, h: u32) {
        let ux = x as usize;
        let uy = y as usize;
        canvas.fill_rect(ux, uy, w as usize, h as usize, 0xFF_1A_1A_2A);
        canvas.draw_rect_outline(ux, uy, w as usize, h as usize, self.theme.border);
        // Color preview swatch
        let swatch_s = (h as usize).min(32);
        let argb = self.to_argb();
        canvas.fill_rect(ux + 4, uy + 4, swatch_s, swatch_s, argb);
        canvas.draw_rect_outline(ux + 4, uy + 4, swatch_s, swatch_s, self.theme.border);
        // Hue bar
        let bar_x = ux + swatch_s + 12;
        let bar_w = (w as usize).saturating_sub(swatch_s + 16);
        let bar_h = 8usize;
        for bx in 0..bar_w {
            let hue = (bx * 360 / bar_w) as u16;
            let (r, g, b) = hsv_to_rgb(hue, 100, 100);
            let c = 0xFF_00_00_00 | (r as u32) << 16 | (g as u32) << 8 | b as u32;
            for by in 0..bar_h {
                canvas.draw_pixel(bar_x + bx, uy + 4 + by, c);
            }
        }
        // Hue indicator
        let hue_pos = bar_x + (self.hue as usize * bar_w / 360).min(bar_w.saturating_sub(1));
        for by in 0..bar_h + 4 {
            canvas.draw_pixel(hue_pos, uy + 2 + by, self.theme.title_fg);
        }
        if self.focused {
            canvas.draw_rect_outline(ux, uy, w as usize, h as usize, self.theme.button_hot);
        }
    }

    pub fn handle_event(&mut self, event: &Event, x: i32, y: i32, w: u32, h: u32) {
        match event {
            Event::MouseClick { x: mx, y: my } => {
                self.focused = hit(*mx, *my, x, y, w, h);
                if self.focused {
                    let swatch_s = (h as usize).min(32);
                    let bar_x = x + swatch_s as i32 + 12;
                    let bar_w = (w as i32) - swatch_s as i32 - 16;
                    if *mx >= bar_x && bar_w > 0 {
                        let rel = (*mx - bar_x).max(0) as u16;
                        self.hue = (rel as u32 * 360 / bar_w as u32).min(359) as u16;
                    }
                }
            }
            Event::KeyPress(k) if self.focused => match *k {
                75 => {
                    self.hue = if self.hue > 0 { self.hue - 5 } else { 355 };
                }
                77 => {
                    self.hue = (self.hue + 5) % 360;
                }
                72 => {
                    self.value = self.value.saturating_add(5).min(100);
                }
                80 => {
                    self.value = self.value.saturating_sub(5);
                }
                _ => {}
            },
            _ => {}
        }
    }

    pub fn accessibility_role(&self) -> AccessibilityRole {
        AccessibilityRole::Slider
    }
}

fn hsv_to_rgb(h: u16, s: u8, v: u8) -> (u8, u8, u8) {
    if s == 0 {
        let vv = (v as u16 * 255 / 100) as u8;
        return (vv, vv, vv);
    }
    let s = s as u32;
    let v = v as u32 * 255 / 100;
    let h = h as u32 % 360;
    let sector = h / 60;
    let frac = (h % 60) * 255 / 60;
    let p = (v * (255 - s * 255 / 100)) / 255;
    let q = (v * (255 - frac * s / 100 / 255)) / 255;
    let t = (v * (255 - (255 - frac) * s / 100 / 255)) / 255;
    let (r, g, b) = match sector {
        0 => (v, t, p),
        1 => (q, v, p),
        2 => (p, v, t),
        3 => (p, q, v),
        4 => (t, p, v),
        _ => (v, p, q),
    };
    (r as u8, g as u8, b as u8)
}

// ── SearchInput ─────────────────────────────────────────────────────────

pub struct SearchInput {
    pub text: String,
    pub cursor_pos: usize,
    pub focused: bool,
    pub suggestions: Vec<String>,
    pub show_suggestions: bool,
    pub selected_suggestion: Option<usize>,
    pub theme: Theme,
}

impl SearchInput {
    pub fn new() -> Self {
        Self {
            text: String::new(),
            cursor_pos: 0,
            focused: false,
            suggestions: Vec::new(),
            show_suggestions: false,
            selected_suggestion: None,
            theme: default_theme(),
        }
    }

    pub fn set_suggestions(&mut self, items: Vec<String>) {
        self.suggestions = items;
        self.show_suggestions = !self.suggestions.is_empty();
        self.selected_suggestion = None;
    }

    pub fn clear(&mut self) {
        self.text.clear();
        self.cursor_pos = 0;
        self.suggestions.clear();
        self.show_suggestions = false;
    }

    pub fn render(&self, canvas: &mut Canvas, x: i32, y: i32, w: u32, h: u32) {
        let ux = x as usize;
        let uy = y as usize;
        let bg = if self.focused {
            0xFF_22_22_38
        } else {
            0xFF_1A_1A_2A
        };
        canvas.fill_rect(ux, uy, w as usize, h as usize, bg);
        canvas.draw_rect_outline(ux, uy, w as usize, h as usize, self.theme.border);
        // Search icon (magnifier approximation)
        canvas.draw_text(
            ux + 4,
            uy + (h as usize - 8) / 2,
            "Q",
            self.theme.border,
            None,
        );
        // Text
        let tx = ux + 16;
        if self.text.is_empty() && !self.focused {
            canvas.draw_text(
                tx,
                uy + (h as usize - 8) / 2,
                "Search...",
                0xFF_66_66_88,
                None,
            );
        } else {
            canvas.draw_text(
                tx,
                uy + (h as usize - 8) / 2,
                &self.text,
                self.theme.text_fg,
                None,
            );
        }
        // Clear button
        if !self.text.is_empty() {
            canvas.draw_text(
                ux + w as usize - 14,
                uy + (h as usize - 8) / 2,
                "x",
                self.theme.button_hot,
                None,
            );
        }
        // Cursor
        if self.focused {
            let cx = tx + self.cursor_pos * 8;
            for dy in 0..12 {
                if cx < ux + w as usize {
                    canvas.draw_pixel(cx, uy + (h as usize - 12) / 2 + dy, self.theme.border);
                }
            }
        }
        // Suggestions dropdown
        if self.show_suggestions && !self.suggestions.is_empty() {
            let list_y = uy + h as usize;
            let item_h = 20usize;
            let show_count = self.suggestions.len().min(6);
            let list_h = show_count * item_h;
            canvas.fill_rect(ux, list_y, w as usize, list_h, self.theme.chrome_bg);
            canvas.draw_rect_outline(ux, list_y, w as usize, list_h, self.theme.border);
            for (i, sug) in self.suggestions.iter().take(show_count).enumerate() {
                let iy = list_y + i * item_h;
                let is_sel = self.selected_suggestion == Some(i);
                if is_sel {
                    canvas.fill_rect(ux + 1, iy, w as usize - 2, item_h, self.theme.button_bg);
                }
                let fg = if is_sel {
                    self.theme.title_fg
                } else {
                    self.theme.text_fg
                };
                canvas.draw_text(ux + 6, iy + (item_h - 8) / 2, sug, fg, None);
            }
        }
    }

    pub fn handle_event(&mut self, event: &Event, x: i32, y: i32, w: u32, h: u32) {
        match event {
            Event::MouseClick { x: mx, y: my } => {
                if hit(*mx, *my, x, y, w, h) {
                    self.focused = true;
                    // Clear button hit
                    if *mx >= x + w as i32 - 16 && !self.text.is_empty() {
                        self.clear();
                    }
                } else {
                    self.focused = false;
                    self.show_suggestions = false;
                }
            }
            Event::KeyPress(k) if self.focused => match *k {
                8 => {
                    if self.cursor_pos > 0 {
                        self.cursor_pos -= 1;
                        self.text.remove(self.cursor_pos);
                    }
                }
                75 => {
                    if self.cursor_pos > 0 {
                        self.cursor_pos -= 1;
                    }
                }
                77 => {
                    if self.cursor_pos < self.text.len() {
                        self.cursor_pos += 1;
                    }
                }
                72 => {
                    if let Some(ref mut idx) = self.selected_suggestion {
                        if *idx > 0 {
                            *idx -= 1;
                        }
                    } else if !self.suggestions.is_empty() {
                        self.selected_suggestion = Some(self.suggestions.len() - 1);
                    }
                }
                80 => {
                    if let Some(ref mut idx) = self.selected_suggestion {
                        if *idx + 1 < self.suggestions.len() {
                            *idx += 1;
                        }
                    } else if !self.suggestions.is_empty() {
                        self.selected_suggestion = Some(0);
                    }
                }
                13 => {
                    if let Some(idx) = self.selected_suggestion {
                        if idx < self.suggestions.len() {
                            self.text = self.suggestions[idx].clone();
                            self.cursor_pos = self.text.len();
                        }
                    }
                    self.show_suggestions = false;
                }
                27 => {
                    self.show_suggestions = false;
                }
                c if c >= 32 => {
                    self.text.insert(self.cursor_pos, c as char);
                    self.cursor_pos += 1;
                }
                _ => {}
            },
            _ => {}
        }
    }

    pub fn accessibility_role(&self) -> AccessibilityRole {
        AccessibilityRole::TextField
    }
}

// ── PasswordInput ───────────────────────────────────────────────────────

pub struct PasswordInput {
    pub text: String,
    pub cursor_pos: usize,
    pub focused: bool,
    pub reveal: bool,
    pub theme: Theme,
}

impl PasswordInput {
    pub fn new() -> Self {
        Self {
            text: String::new(),
            cursor_pos: 0,
            focused: false,
            reveal: false,
            theme: default_theme(),
        }
    }

    pub fn render(&self, canvas: &mut Canvas, x: i32, y: i32, w: u32, h: u32) {
        let ux = x as usize;
        let uy = y as usize;
        let bg = if self.focused {
            0xFF_22_22_38
        } else {
            0xFF_1A_1A_2A
        };
        canvas.fill_rect(ux, uy, w as usize, h as usize, bg);
        canvas.draw_rect_outline(ux, uy, w as usize, h as usize, self.theme.border);

        let tx = ux + 4;
        let ty = uy + (h as usize - 8) / 2;
        if self.reveal {
            canvas.draw_text(tx, ty, &self.text, self.theme.text_fg, None);
        } else {
            let mut masked = String::new();
            for _ in 0..self.text.len() {
                masked.push('*');
            }
            canvas.draw_text(tx, ty, &masked, self.theme.text_fg, None);
        }
        // Toggle eye icon
        let eye = if self.reveal { "O" } else { "-" };
        canvas.draw_text(ux + w as usize - 14, ty, eye, self.theme.border, None);

        if self.focused {
            let cx = tx + self.cursor_pos * 8;
            for dy in 0..12 {
                if cx < ux + w as usize {
                    canvas.draw_pixel(cx, uy + 4 + dy, self.theme.border);
                }
            }
            canvas.draw_rect_outline(ux, uy, w as usize, h as usize, self.theme.button_hot);
        }
    }

    pub fn handle_event(&mut self, event: &Event, x: i32, y: i32, w: u32, h: u32) {
        match event {
            Event::MouseClick { x: mx, y: my } => {
                self.focused = hit(*mx, *my, x, y, w, h);
                if self.focused && *mx >= x + w as i32 - 16 {
                    self.reveal = !self.reveal;
                }
            }
            Event::KeyPress(k) if self.focused => match *k {
                8 => {
                    if self.cursor_pos > 0 {
                        self.cursor_pos -= 1;
                        self.text.remove(self.cursor_pos);
                    }
                }
                75 => {
                    if self.cursor_pos > 0 {
                        self.cursor_pos -= 1;
                    }
                }
                77 => {
                    if self.cursor_pos < self.text.len() {
                        self.cursor_pos += 1;
                    }
                }
                c if c >= 32 => {
                    self.text.insert(self.cursor_pos, c as char);
                    self.cursor_pos += 1;
                }
                _ => {}
            },
            _ => {}
        }
    }

    pub fn accessibility_role(&self) -> AccessibilityRole {
        AccessibilityRole::TextField
    }
}

// ── RangeSlider ─────────────────────────────────────────────────────────

pub struct RangeSlider {
    pub low: f32,
    pub high: f32,
    pub min: f32,
    pub max: f32,
    pub step: f32,
    pub dragging_low: bool,
    pub dragging_high: bool,
    pub focused: bool,
    pub theme: Theme,
}

impl RangeSlider {
    pub fn new(min: f32, max: f32, step: f32) -> Self {
        Self {
            low: min,
            high: max,
            min,
            max,
            step,
            dragging_low: false,
            dragging_high: false,
            focused: false,
            theme: default_theme(),
        }
    }

    fn frac(&self, v: f32) -> f32 {
        let range = self.max - self.min;
        if range <= 0.0 {
            0.0
        } else {
            (v - self.min) / range
        }
    }

    fn snap(&self, v: f32) -> f32 {
        if self.step <= 0.0 {
            return v;
        }
        let steps = ((v - self.min) / self.step + 0.5) as i32;
        clamp_f32(self.min + steps as f32 * self.step, self.min, self.max)
    }

    pub fn render(&self, canvas: &mut Canvas, x: i32, y: i32, w: u32, h: u32) {
        let ux = x as usize;
        let uy = y as usize;
        let track_h = 4usize;
        let knob_r = 6usize;
        let track_y = uy + h as usize / 2;

        // Track background
        canvas.fill_rect(ux, track_y, w as usize, track_h, 0xFF_33_33_44);
        // Filled range
        let low_px = (w as f32 * self.frac(self.low)) as usize;
        let high_px = (w as f32 * self.frac(self.high)) as usize;
        canvas.fill_rect(
            ux + low_px,
            track_y,
            high_px - low_px,
            track_h,
            self.theme.border,
        );
        // Knobs
        for &(px, is_active) in &[(low_px, self.dragging_low), (high_px, self.dragging_high)] {
            let kx = ux + px;
            let ky = track_y + track_h / 2;
            let col = if is_active {
                self.theme.title_fg
            } else {
                self.theme.button_hot
            };
            for dy in 0..knob_r * 2 {
                for dx in 0..knob_r * 2 {
                    let rx = dx as i32 - knob_r as i32;
                    let ry = dy as i32 - knob_r as i32;
                    if rx * rx + ry * ry <= (knob_r as i32) * (knob_r as i32) {
                        let ppx = (kx as i32 + rx) as usize;
                        let ppy = (ky as i32 + ry) as usize;
                        canvas.draw_pixel(ppx, ppy, col);
                    }
                }
            }
        }
        if self.focused {
            canvas.draw_rect_outline(ux, uy, w as usize, h as usize, self.theme.button_hot);
        }
    }

    pub fn handle_event(&mut self, event: &Event, x: i32, y: i32, w: u32, h: u32) {
        match event {
            Event::MouseDown { x: mx, y: my, .. } => {
                if hit(*mx, *my, x, y, w, h) {
                    self.focused = true;
                    let frac = (*mx - x) as f32 / w as f32;
                    let val = self.min + frac * (self.max - self.min);
                    let dist_low = (val - self.low).abs();
                    let dist_high = (val - self.high).abs();
                    if dist_low <= dist_high {
                        self.dragging_low = true;
                        self.low = self.snap(val);
                    } else {
                        self.dragging_high = true;
                        self.high = self.snap(val);
                    }
                }
            }
            Event::MouseUp { .. } => {
                self.dragging_low = false;
                self.dragging_high = false;
            }
            Event::MouseMove { dx, .. } => {
                let delta = *dx as f32 / w as f32 * (self.max - self.min);
                if self.dragging_low {
                    self.low = self.snap(clamp_f32(self.low + delta, self.min, self.high));
                }
                if self.dragging_high {
                    self.high = self.snap(clamp_f32(self.high + delta, self.low, self.max));
                }
            }
            _ => {}
        }
    }

    pub fn accessibility_role(&self) -> AccessibilityRole {
        AccessibilityRole::Slider
    }
}

// ── FileInput ───────────────────────────────────────────────────────────

pub struct FileInput {
    pub path: String,
    pub focused: bool,
    pub theme: Theme,
}

impl FileInput {
    pub fn new() -> Self {
        Self {
            path: String::new(),
            focused: false,
            theme: default_theme(),
        }
    }

    pub fn set_path(&mut self, p: &str) {
        self.path = String::from(p);
    }

    pub fn render(&self, canvas: &mut Canvas, x: i32, y: i32, w: u32, h: u32) {
        let ux = x as usize;
        let uy = y as usize;
        let btn_w = 60usize;
        let field_w = (w as usize).saturating_sub(btn_w + 4);

        canvas.fill_rect(ux, uy, field_w, h as usize, 0xFF_1A_1A_2A);
        canvas.draw_rect_outline(ux, uy, field_w, h as usize, self.theme.border);
        let display = if self.path.is_empty() {
            "No file selected"
        } else {
            &self.path
        };
        canvas.draw_text(
            ux + 4,
            uy + (h as usize - 8) / 2,
            display,
            self.theme.text_fg,
            None,
        );

        let bx = ux + field_w + 4;
        canvas.fill_rect(bx, uy, btn_w, h as usize, self.theme.button_bg);
        canvas.draw_rect_outline(bx, uy, btn_w, h as usize, self.theme.border);
        canvas.draw_text(
            bx + 6,
            uy + (h as usize - 8) / 2,
            "Browse",
            self.theme.button_text,
            None,
        );

        if self.focused {
            canvas.draw_rect_outline(ux, uy, w as usize, h as usize, self.theme.button_hot);
        }
    }

    pub fn handle_event(&mut self, event: &Event, x: i32, y: i32, w: u32, h: u32) {
        if let Event::MouseClick { x: mx, y: my } = event {
            self.focused = hit(*mx, *my, x, y, w, h);
        }
    }

    pub fn accessibility_role(&self) -> AccessibilityRole {
        AccessibilityRole::Button
    }
}

// ── Host KATs (dev box, `cargo test -p raeui`) ──────────────────────────
// MasterChecklist Phase 8 (AthUI inputs): TextArea editing invariants —
// insert-at-cursor, the newline split, and the backspace line-merge are
// what every editor / terminal / text field depends on. FAIL-ably pinned.
#[cfg(test)]
mod input_kat {
    use super::*;

    #[test]
    fn textarea_insert_advances_cursor() {
        let mut ta = TextArea::new();
        ta.insert_char('h');
        ta.insert_char('i');
        assert_eq!(ta.lines, alloc::vec![String::from("hi")]);
        assert_eq!((ta.cursor_line, ta.cursor_col), (0, 2));
    }

    #[test]
    fn textarea_backspace_removes_then_merges() {
        let mut ta = TextArea::new();
        ta.set_text("a\nb");
        assert_eq!(ta.lines.len(), 2);
        // Backspace inside a line removes the char before the cursor.
        ta.cursor_line = 1;
        ta.cursor_col = 1; // after "b"
        ta.delete_char();
        assert_eq!(ta.lines, alloc::vec![String::from("a"), String::new()]);
        assert_eq!(ta.cursor_col, 0);
        // Backspace at column 0 merges this line into the one above.
        ta.delete_char();
        assert_eq!(ta.lines, alloc::vec![String::from("a")]);
        assert_eq!((ta.cursor_line, ta.cursor_col), (0, 1));
    }

    #[test]
    fn textarea_newline_splits_at_cursor() {
        let mut ta = TextArea::new();
        ta.set_text("ab");
        ta.cursor_col = 1; // between 'a' and 'b'
        ta.insert_char('\n');
        assert_eq!(ta.lines, alloc::vec![String::from("a"), String::from("b")]);
        assert_eq!((ta.cursor_line, ta.cursor_col), (1, 0));
    }

    #[test]
    fn textarea_set_text_splits_lines_and_resets_cursor() {
        let mut ta = TextArea::new();
        ta.cursor_line = 5;
        ta.cursor_col = 9;
        ta.set_text("x\ny\nz");
        assert_eq!(
            ta.lines,
            alloc::vec![String::from("x"), String::from("y"), String::from("z")]
        );
        assert_eq!((ta.cursor_line, ta.cursor_col), (0, 0));
    }
}
