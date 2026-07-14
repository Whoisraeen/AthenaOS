//! Navigation widgets for RaeUI.
//!
//! Sidebar, Toolbar, Pagination, SegmentedControl, CommandPalette —
//! all theme-aware and keyboard-navigable.

extern crate alloc;

use crate::accessibility::AccessibilityRole;
use crate::{blend_colors, default_theme, Event, Theme};
use alloc::string::String;
use alloc::vec::Vec;
use raegfx::Canvas;

fn hit(mx: i32, my: i32, x: i32, y: i32, w: u32, h: u32) -> bool {
    mx >= x && mx < x + w as i32 && my >= y && my < y + h as i32
}

// ── Sidebar ─────────────────────────────────────────────────────────────

pub struct SidebarSection {
    pub label: String,
    pub items: Vec<SidebarItem>,
    pub collapsed: bool,
}

impl SidebarSection {
    pub fn new(label: &str) -> Self {
        Self {
            label: String::from(label),
            items: Vec::new(),
            collapsed: false,
        }
    }

    pub fn add_item(&mut self, item: SidebarItem) {
        self.items.push(item);
    }
}

pub struct SidebarItem {
    pub label: String,
    pub icon: Option<char>,
    pub badge_count: u32,
}

impl SidebarItem {
    pub fn new(label: &str) -> Self {
        Self {
            label: String::from(label),
            icon: None,
            badge_count: 0,
        }
    }

    pub fn with_icon(mut self, icon: char) -> Self {
        self.icon = Some(icon);
        self
    }

    pub fn with_badge(mut self, count: u32) -> Self {
        self.badge_count = count;
        self
    }
}

pub struct Sidebar {
    pub sections: Vec<SidebarSection>,
    pub selected_section: usize,
    pub selected_item: usize,
    pub collapsed: bool,
    pub collapsed_width: u32,
    pub expanded_width: u32,
    pub focused: bool,
    pub item_height: u32,
    pub section_header_height: u32,
    pub theme: Theme,
}

impl Sidebar {
    pub fn new() -> Self {
        Self {
            sections: Vec::new(),
            selected_section: 0,
            selected_item: 0,
            collapsed: false,
            collapsed_width: 40,
            expanded_width: 200,
            focused: false,
            item_height: 28,
            section_header_height: 24,
            theme: default_theme(),
        }
    }

    pub fn toggle_collapse(&mut self) {
        self.collapsed = !self.collapsed;
    }

    pub fn effective_width(&self) -> u32 {
        if self.collapsed {
            self.collapsed_width
        } else {
            self.expanded_width
        }
    }

    pub fn render(&self, canvas: &mut Canvas, x: i32, y: i32, _w: u32, h: u32) {
        let ux = x as usize;
        let uy = y as usize;
        let w = self.effective_width() as usize;
        canvas.fill_rect(ux, uy, w, h as usize, self.theme.chrome_bg);
        canvas.draw_rect_outline(ux, uy, w, h as usize, self.theme.border);

        let mut cy = uy;
        for (si, section) in self.sections.iter().enumerate() {
            if cy >= uy + h as usize {
                break;
            }
            // Section header
            let sh = self.section_header_height as usize;
            canvas.fill_rect(ux, cy, w, sh, self.theme.chrome_bg);
            if !self.collapsed {
                let arrow = if section.collapsed { ">" } else { "v" };
                canvas.draw_text(ux + 6, cy + (sh - 8) / 2, arrow, self.theme.text_fg, None);
                canvas.draw_text(
                    ux + 18,
                    cy + (sh - 8) / 2,
                    &section.label,
                    self.theme.title_fg,
                    None,
                );
            }
            cy += sh;

            if section.collapsed {
                continue;
            }

            for (ii, item) in section.items.iter().enumerate() {
                if cy >= uy + h as usize {
                    break;
                }
                let ih = self.item_height as usize;
                let is_selected = si == self.selected_section && ii == self.selected_item;
                if is_selected {
                    canvas.fill_rect(ux + 1, cy, w - 2, ih, self.theme.button_bg);
                }
                let fg = if is_selected {
                    self.theme.title_fg
                } else {
                    self.theme.text_fg
                };

                if self.collapsed {
                    // Only icon
                    if let Some(icon) = item.icon {
                        let mut ibuf = [0u8; 4];
                        let s = icon.encode_utf8(&mut ibuf);
                        canvas.draw_text(ux + w / 2 - 4, cy + (ih - 8) / 2, s, fg, None);
                    }
                } else {
                    let mut tx = ux + 12;
                    if let Some(icon) = item.icon {
                        let mut ibuf = [0u8; 4];
                        let s = icon.encode_utf8(&mut ibuf);
                        canvas.draw_text(tx, cy + (ih - 8) / 2, s, fg, None);
                        tx += 16;
                    }
                    canvas.draw_text(tx, cy + (ih - 8) / 2, &item.label, fg, None);
                    // Badge
                    if item.badge_count > 0 {
                        let mut nbuf = [0u8; 10];
                        let ns = u32_to_decimal(item.badge_count, &mut nbuf);
                        let bw = ns.len() * 8 + 8;
                        let bx = ux + w - bw - 8;
                        canvas.fill_rect(bx, cy + (ih - 14) / 2, bw, 14, self.theme.button_hot);
                        canvas.draw_text(bx + 4, cy + (ih - 8) / 2, ns, 0xFF_FF_FF_FF, None);
                    }
                }
                cy += ih;
            }
        }

        if self.focused {
            canvas.draw_rect_outline(ux, uy, w, h as usize, self.theme.button_hot);
        }
    }

    pub fn handle_event(&mut self, event: &Event, x: i32, y: i32, _w: u32, h: u32) {
        let w = self.effective_width();
        match event {
            Event::MouseClick { x: mx, y: my } => {
                if !hit(*mx, *my, x, y, w, h) {
                    self.focused = false;
                    return;
                }
                self.focused = true;
                let mut cy = y;
                for (si, section) in self.sections.iter_mut().enumerate() {
                    let sh = self.section_header_height as i32;
                    if hit(*mx, *my, x, cy, w, self.section_header_height) {
                        section.collapsed = !section.collapsed;
                        return;
                    }
                    cy += sh;
                    if section.collapsed {
                        continue;
                    }
                    for (ii, _) in section.items.iter().enumerate() {
                        if hit(*mx, *my, x, cy, w, self.item_height) {
                            self.selected_section = si;
                            self.selected_item = ii;
                            return;
                        }
                        cy += self.item_height as i32;
                    }
                }
            }
            Event::KeyPress(k) if self.focused => match *k {
                72 => {
                    if self.selected_item > 0 {
                        self.selected_item -= 1;
                    } else if self.selected_section > 0 {
                        self.selected_section -= 1;
                        if let Some(sec) = self.sections.get(self.selected_section) {
                            self.selected_item = sec.items.len().saturating_sub(1);
                        }
                    }
                }
                80 => {
                    if let Some(sec) = self.sections.get(self.selected_section) {
                        if self.selected_item + 1 < sec.items.len() {
                            self.selected_item += 1;
                        } else if self.selected_section + 1 < self.sections.len() {
                            self.selected_section += 1;
                            self.selected_item = 0;
                        }
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

// ── Toolbar ─────────────────────────────────────────────────────────────

pub struct ToolbarItem {
    pub icon: char,
    pub tooltip: String,
    pub disabled: bool,
    pub is_separator: bool,
}

impl ToolbarItem {
    pub fn button(icon: char, tooltip: &str) -> Self {
        Self {
            icon,
            tooltip: String::from(tooltip),
            disabled: false,
            is_separator: false,
        }
    }

    pub fn separator() -> Self {
        Self {
            icon: ' ',
            tooltip: String::new(),
            disabled: false,
            is_separator: true,
        }
    }
}

pub struct Toolbar {
    pub items: Vec<ToolbarItem>,
    pub focused_idx: Option<usize>,
    pub button_size: u32,
    pub theme: Theme,
}

impl Toolbar {
    pub fn new() -> Self {
        Self {
            items: Vec::new(),
            focused_idx: None,
            button_size: 28,
            theme: default_theme(),
        }
    }

    pub fn add_item(&mut self, item: ToolbarItem) {
        self.items.push(item);
    }

    pub fn render(&self, canvas: &mut Canvas, x: i32, y: i32, w: u32, h: u32) {
        let ux = x as usize;
        let uy = y as usize;
        canvas.fill_rect(ux, uy, w as usize, h as usize, self.theme.chrome_bg);
        canvas.draw_rect_outline(ux, uy, w as usize, h as usize, self.theme.border);

        let mut bx = ux + 4;
        let by = uy + (h as usize - self.button_size as usize) / 2;
        for (i, item) in self.items.iter().enumerate() {
            if item.is_separator {
                let sx = bx + 2;
                canvas.fill_rect(sx, uy + 4, 1, h as usize - 8, self.theme.border);
                bx += 8;
                continue;
            }
            let bs = self.button_size as usize;
            let is_focused = self.focused_idx == Some(i);
            let bg = if is_focused {
                self.theme.button_hot
            } else if item.disabled {
                0xFF_22_22_33
            } else {
                self.theme.button_bg
            };
            canvas.fill_rect(bx, by, bs, bs, bg);
            canvas.draw_rect_outline(bx, by, bs, bs, self.theme.border);
            let fg = if item.disabled {
                0xFF_66_66_88
            } else {
                self.theme.button_text
            };
            let mut ibuf = [0u8; 4];
            let icon_str = item.icon.encode_utf8(&mut ibuf);
            canvas.draw_text(bx + bs / 2 - 4, by + bs / 2 - 4, icon_str, fg, None);
            bx += bs + 4;
        }
    }

    /// Returns index of clicked non-disabled button, if any.
    pub fn handle_event(&mut self, event: &Event, x: i32, y: i32, w: u32, h: u32) -> Option<usize> {
        match event {
            Event::MouseClick { x: mx, y: my } => {
                if !hit(*mx, *my, x, y, w, h) {
                    self.focused_idx = None;
                    return None;
                }
                let mut bx = x + 4;
                let by = y + (h as i32 - self.button_size as i32) / 2;
                for (i, item) in self.items.iter().enumerate() {
                    if item.is_separator {
                        bx += 8;
                        continue;
                    }
                    if hit(*mx, *my, bx, by, self.button_size, self.button_size) && !item.disabled {
                        self.focused_idx = Some(i);
                        return Some(i);
                    }
                    bx += self.button_size as i32 + 4;
                }
            }
            Event::KeyPress(k) => match *k {
                75 => {
                    let non_sep: Vec<usize> = self
                        .items
                        .iter()
                        .enumerate()
                        .filter(|(_, it)| !it.is_separator && !it.disabled)
                        .map(|(i, _)| i)
                        .collect();
                    if let Some(cur) = self.focused_idx {
                        if let Some(pos) = non_sep.iter().position(|&i| i == cur) {
                            if pos > 0 {
                                self.focused_idx = Some(non_sep[pos - 1]);
                            }
                        }
                    } else if !non_sep.is_empty() {
                        self.focused_idx = Some(non_sep[non_sep.len() - 1]);
                    }
                }
                77 => {
                    let non_sep: Vec<usize> = self
                        .items
                        .iter()
                        .enumerate()
                        .filter(|(_, it)| !it.is_separator && !it.disabled)
                        .map(|(i, _)| i)
                        .collect();
                    if let Some(cur) = self.focused_idx {
                        if let Some(pos) = non_sep.iter().position(|&i| i == cur) {
                            if pos + 1 < non_sep.len() {
                                self.focused_idx = Some(non_sep[pos + 1]);
                            }
                        }
                    } else if !non_sep.is_empty() {
                        self.focused_idx = Some(non_sep[0]);
                    }
                }
                13 => {
                    if let Some(idx) = self.focused_idx {
                        if idx < self.items.len() && !self.items[idx].disabled {
                            return Some(idx);
                        }
                    }
                }
                _ => {}
            },
            _ => {}
        }
        None
    }

    pub fn accessibility_role(&self) -> AccessibilityRole {
        AccessibilityRole::Toolbar
    }
}

// ── Pagination ──────────────────────────────────────────────────────────

pub struct Pagination {
    pub current_page: usize,
    pub total_pages: usize,
    pub visible_page_buttons: usize,
    pub focused: bool,
    pub theme: Theme,
}

impl Pagination {
    pub fn new(total_pages: usize) -> Self {
        Self {
            current_page: 0,
            total_pages,
            visible_page_buttons: 5,
            focused: false,
            theme: default_theme(),
        }
    }

    pub fn set_page(&mut self, page: usize) {
        if page < self.total_pages {
            self.current_page = page;
        }
    }

    pub fn render(&self, canvas: &mut Canvas, x: i32, y: i32, _w: u32, h: u32) {
        let ux = x as usize;
        let uy = y as usize;
        let btn_w = 28usize;
        let btn_h = h as usize;
        let gap = 4usize;

        // Prev button
        let bg = self.theme.button_bg;
        canvas.fill_rect(ux, uy, btn_w, btn_h, bg);
        canvas.draw_rect_outline(ux, uy, btn_w, btn_h, self.theme.border);
        canvas.draw_text(
            ux + btn_w / 2 - 4,
            uy + (btn_h - 8) / 2,
            "<",
            self.theme.button_text,
            None,
        );

        // Page buttons
        let start_page = if self.current_page < self.visible_page_buttons / 2 {
            0
        } else {
            (self.current_page - self.visible_page_buttons / 2)
                .min(self.total_pages.saturating_sub(self.visible_page_buttons))
        };
        let end_page = (start_page + self.visible_page_buttons).min(self.total_pages);

        let mut bx = ux + btn_w + gap;
        for p in start_page..end_page {
            let is_current = p == self.current_page;
            let page_bg = if is_current {
                self.theme.button_hot
            } else {
                self.theme.button_bg
            };
            canvas.fill_rect(bx, uy, btn_w, btn_h, page_bg);
            canvas.draw_rect_outline(bx, uy, btn_w, btn_h, self.theme.border);
            let num = p + 1;
            let mut nbuf = [0u8; 10];
            let ns = u32_to_decimal(num as u32, &mut nbuf);
            let tx = bx + (btn_w - ns.len() * 8) / 2;
            canvas.draw_text(tx, uy + (btn_h - 8) / 2, ns, self.theme.button_text, None);
            bx += btn_w + gap;
        }

        // Next button
        canvas.fill_rect(bx, uy, btn_w, btn_h, bg);
        canvas.draw_rect_outline(bx, uy, btn_w, btn_h, self.theme.border);
        canvas.draw_text(
            bx + btn_w / 2 - 4,
            uy + (btn_h - 8) / 2,
            ">",
            self.theme.button_text,
            None,
        );

        if self.focused {
            let total_w = btn_w + gap + (end_page - start_page) * (btn_w + gap) + btn_w;
            canvas.draw_rect_outline(ux, uy, total_w, btn_h, self.theme.button_hot);
        }
    }

    pub fn handle_event(&mut self, event: &Event, x: i32, y: i32, _w: u32, h: u32) {
        let btn_w = 28i32;
        let gap = 4i32;
        match event {
            Event::MouseClick { x: mx, y: my } => {
                // Prev
                if hit(*mx, *my, x, y, btn_w as u32, h) {
                    if self.current_page > 0 {
                        self.current_page -= 1;
                    }
                    self.focused = true;
                    return;
                }
                // Page buttons
                let start_page = if self.current_page < self.visible_page_buttons / 2 {
                    0
                } else {
                    (self.current_page - self.visible_page_buttons / 2)
                        .min(self.total_pages.saturating_sub(self.visible_page_buttons))
                };
                let end_page = (start_page + self.visible_page_buttons).min(self.total_pages);
                let mut bx = x + btn_w + gap;
                for p in start_page..end_page {
                    if hit(*mx, *my, bx, y, btn_w as u32, h) {
                        self.current_page = p;
                        self.focused = true;
                        return;
                    }
                    bx += btn_w + gap;
                }
                // Next
                if hit(*mx, *my, bx, y, btn_w as u32, h) {
                    if self.current_page + 1 < self.total_pages {
                        self.current_page += 1;
                    }
                    self.focused = true;
                    return;
                }
                self.focused = false;
            }
            Event::KeyPress(k) if self.focused => match *k {
                75 => {
                    if self.current_page > 0 {
                        self.current_page -= 1;
                    }
                }
                77 => {
                    if self.current_page + 1 < self.total_pages {
                        self.current_page += 1;
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

// ── SegmentedControl ────────────────────────────────────────────────────

pub struct SegmentedControl {
    pub segments: Vec<String>,
    pub selected: usize,
    pub focused: bool,
    pub theme: Theme,
}

impl SegmentedControl {
    pub fn new(segments: &[&str]) -> Self {
        let mut v = Vec::new();
        for s in segments {
            v.push(String::from(*s));
        }
        Self {
            segments: v,
            selected: 0,
            focused: false,
            theme: default_theme(),
        }
    }

    pub fn render(&self, canvas: &mut Canvas, x: i32, y: i32, w: u32, h: u32) {
        let ux = x as usize;
        let uy = y as usize;
        let count = self.segments.len();
        if count == 0 {
            return;
        }
        let seg_w = w as usize / count;

        // Background pill
        canvas.fill_rect(ux, uy, w as usize, h as usize, self.theme.chrome_bg);
        canvas.draw_rect_outline(ux, uy, w as usize, h as usize, self.theme.border);

        for (i, seg) in self.segments.iter().enumerate() {
            let sx = ux + i * seg_w;
            let is_selected = i == self.selected;
            if is_selected {
                canvas.fill_rect(sx, uy, seg_w, h as usize, self.theme.button_hot);
            }
            let fg = if is_selected {
                self.theme.button_text
            } else {
                self.theme.text_fg
            };
            let tw = seg.len() * 8;
            let tx = sx + (seg_w - tw) / 2;
            canvas.draw_text(tx, uy + (h as usize - 8) / 2, seg, fg, None);
            // Separator
            if i > 0 && !is_selected && i != self.selected {
                for py in uy + 4..uy + h as usize - 4 {
                    canvas.draw_pixel(sx, py, self.theme.border);
                }
            }
        }

        if self.focused {
            canvas.draw_rect_outline(ux, uy, w as usize, h as usize, self.theme.button_hot);
        }
    }

    pub fn handle_event(&mut self, event: &Event, x: i32, y: i32, w: u32, h: u32) {
        let count = self.segments.len();
        if count == 0 {
            return;
        }
        let seg_w = w / count as u32;
        match event {
            Event::MouseClick { x: mx, y: my } => {
                if hit(*mx, *my, x, y, w, h) {
                    let idx = ((*mx - x) as u32 / seg_w) as usize;
                    if idx < count {
                        self.selected = idx;
                    }
                    self.focused = true;
                } else {
                    self.focused = false;
                }
            }
            Event::KeyPress(k) if self.focused => match *k {
                75 => {
                    if self.selected > 0 {
                        self.selected -= 1;
                    }
                }
                77 => {
                    if self.selected + 1 < count {
                        self.selected += 1;
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }

    pub fn accessibility_role(&self) -> AccessibilityRole {
        AccessibilityRole::TabBar
    }
}

// ── CommandPalette ──────────────────────────────────────────────────────

pub struct CommandEntry {
    pub label: String,
    pub shortcut: Option<String>,
    pub category: Option<String>,
}

impl CommandEntry {
    pub fn new(label: &str) -> Self {
        Self {
            label: String::from(label),
            shortcut: None,
            category: None,
        }
    }

    pub fn with_shortcut(mut self, sc: &str) -> Self {
        self.shortcut = Some(String::from(sc));
        self
    }

    pub fn with_category(mut self, cat: &str) -> Self {
        self.category = Some(String::from(cat));
        self
    }
}

pub struct CommandPalette {
    pub entries: Vec<CommandEntry>,
    pub query: String,
    pub cursor_pos: usize,
    pub selected: usize,
    pub visible: bool,
    pub max_visible: usize,
    pub scroll_offset: usize,
    pub theme: Theme,
}

impl CommandPalette {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            query: String::new(),
            cursor_pos: 0,
            selected: 0,
            visible: false,
            max_visible: 8,
            scroll_offset: 0,
            theme: default_theme(),
        }
    }

    pub fn show(&mut self) {
        self.visible = true;
        self.query.clear();
        self.cursor_pos = 0;
        self.selected = 0;
        self.scroll_offset = 0;
    }

    pub fn dismiss(&mut self) {
        self.visible = false;
    }

    fn filtered_indices(&self) -> Vec<usize> {
        if self.query.is_empty() {
            return (0..self.entries.len()).collect();
        }
        let q_lower: Vec<u8> = self.query.bytes().map(|b| b.to_ascii_lowercase()).collect();
        let mut out = Vec::new();
        for (i, entry) in self.entries.iter().enumerate() {
            if fuzzy_match(&entry.label, &q_lower) {
                out.push(i);
            }
        }
        out
    }

    pub fn render(&self, canvas: &mut Canvas, x: i32, y: i32, w: u32, _h: u32) {
        if !self.visible {
            return;
        }
        let ux = x as usize;
        let uy = y as usize;
        let input_h = 32usize;
        let item_h = 28usize;

        // Backdrop overlay (semi-transparent)
        let backdrop = blend_colors(0xFF_00_00_00, self.theme.window_bg, 160);
        canvas.fill_rect(0, 0, canvas.width(), canvas.height(), backdrop);

        // Palette container — centered at top
        let pw = (w as usize).min(500);
        let px = ux + (w as usize - pw) / 2;
        let py = uy + 60;

        // Input field
        canvas.fill_rect(px, py, pw, input_h, self.theme.chrome_bg);
        canvas.draw_rect_outline(px, py, pw, input_h, self.theme.border);
        canvas.draw_text(px + 8, py + (input_h - 8) / 2, ">", self.theme.border, None);
        if self.query.is_empty() {
            canvas.draw_text(
                px + 24,
                py + (input_h - 8) / 2,
                "Type a command...",
                0xFF_66_66_88,
                None,
            );
        } else {
            canvas.draw_text(
                px + 24,
                py + (input_h - 8) / 2,
                &self.query,
                self.theme.text_fg,
                None,
            );
        }
        // Cursor
        let cx = px + 24 + self.cursor_pos * 8;
        for dy in 0..16 {
            if cx < px + pw {
                canvas.draw_pixel(cx, py + 8 + dy, self.theme.border);
            }
        }

        // Results list
        let filtered = self.filtered_indices();
        let show = filtered.len().min(self.max_visible);
        let list_y = py + input_h;
        let list_h = show * item_h;
        canvas.fill_rect(px, list_y, pw, list_h, self.theme.window_bg);
        canvas.draw_rect_outline(px, list_y, pw, list_h, self.theme.border);

        for (vi, &idx) in filtered
            .iter()
            .skip(self.scroll_offset)
            .take(show)
            .enumerate()
        {
            let iy = list_y + vi * item_h;
            let is_sel = vi + self.scroll_offset == self.selected;
            if is_sel {
                canvas.fill_rect(px + 1, iy, pw - 2, item_h, self.theme.button_bg);
            }
            let entry = &self.entries[idx];
            let fg = if is_sel {
                self.theme.title_fg
            } else {
                self.theme.text_fg
            };
            // Category prefix
            let mut tx = px + 8;
            if let Some(ref cat) = entry.category {
                canvas.draw_text(tx, iy + (item_h - 8) / 2, cat, self.theme.border, None);
                tx += cat.len() * 8 + 8;
                canvas.draw_text(tx - 4, iy + (item_h - 8) / 2, ":", self.theme.text_fg, None);
            }
            canvas.draw_text(tx, iy + (item_h - 8) / 2, &entry.label, fg, None);
            // Shortcut right-aligned
            if let Some(ref sc) = entry.shortcut {
                let sx = px + pw - sc.len() * 8 - 8;
                canvas.draw_text(sx, iy + (item_h - 8) / 2, sc, 0xFF_66_66_88, None);
            }
        }
    }

    /// Returns index into `entries` of the selected command, or None.
    pub fn handle_event(
        &mut self,
        event: &Event,
        _x: i32,
        _y: i32,
        _w: u32,
        _h: u32,
    ) -> Option<usize> {
        if !self.visible {
            return None;
        }
        match event {
            Event::KeyPress(k) => match *k {
                27 => {
                    self.dismiss();
                    return None;
                }
                8 => {
                    if self.cursor_pos > 0 {
                        self.cursor_pos -= 1;
                        self.query.remove(self.cursor_pos);
                        self.selected = 0;
                        self.scroll_offset = 0;
                    }
                }
                13 => {
                    let filtered = self.filtered_indices();
                    if self.selected < filtered.len() {
                        let idx = filtered[self.selected];
                        self.dismiss();
                        return Some(idx);
                    }
                }
                72 => {
                    if self.selected > 0 {
                        self.selected -= 1;
                        if self.selected < self.scroll_offset {
                            self.scroll_offset = self.selected;
                        }
                    }
                }
                80 => {
                    let filtered = self.filtered_indices();
                    if self.selected + 1 < filtered.len() {
                        self.selected += 1;
                        if self.selected >= self.scroll_offset + self.max_visible {
                            self.scroll_offset = self.selected - self.max_visible + 1;
                        }
                    }
                }
                c if c >= 32 => {
                    self.query.insert(self.cursor_pos, c as char);
                    self.cursor_pos += 1;
                    self.selected = 0;
                    self.scroll_offset = 0;
                }
                _ => {}
            },
            _ => {}
        }
        None
    }

    pub fn accessibility_role(&self) -> AccessibilityRole {
        AccessibilityRole::Dialog
    }
}

/// Simple fuzzy match: all query chars must appear in order in the target.
fn fuzzy_match(target: &str, query: &[u8]) -> bool {
    if query.is_empty() {
        return true;
    }
    let mut qi = 0;
    for b in target.bytes() {
        if b.to_ascii_lowercase() == query[qi] {
            qi += 1;
            if qi == query.len() {
                return true;
            }
        }
    }
    false
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
