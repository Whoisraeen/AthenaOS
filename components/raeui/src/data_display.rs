//! Data display widgets for AthUI.
//!
//! Table (virtual-scrolled), TreeView, ListView (virtual-scrolled), GridView,
//! Badge, Tag, Tooltip, Avatar, Breadcrumb, Chart, Sparkline, StatusIndicator.
//! All theme-aware and keyboard-navigable.

extern crate alloc;

use crate::accessibility::AccessibilityRole;
use crate::{default_theme, Event, Theme};
use alloc::string::String;
use alloc::vec::Vec;
use raegfx::Canvas;

fn hit(mx: i32, my: i32, x: i32, y: i32, w: u32, h: u32) -> bool {
    mx >= x && mx < x + w as i32 && my >= y && my < y + h as i32
}

// ── Table (virtual scrolling) ───────────────────────────────────────────

pub struct TableColumn {
    pub label: String,
    pub width: u32,
    pub sortable: bool,
}

impl TableColumn {
    pub fn new(label: &str, width: u32) -> Self {
        Self {
            label: String::from(label),
            width,
            sortable: true,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortDir {
    Asc,
    Desc,
}

/// Virtual-scrolled table: only renders visible rows. The caller supplies
/// a `cell_fn` callback to provide cell text on demand rather than storing
/// all rows in memory.
pub struct Table {
    pub columns: Vec<TableColumn>,
    pub total_rows: usize,
    pub row_height: u32,
    pub header_height: u32,
    pub scroll_offset: usize,
    pub selected_row: Option<usize>,
    pub sort_column: Option<usize>,
    pub sort_dir: SortDir,
    pub focused: bool,
    pub theme: Theme,
}

impl Table {
    pub fn new(columns: Vec<TableColumn>, total_rows: usize) -> Self {
        Self {
            columns,
            total_rows,
            row_height: 20,
            header_height: 24,
            scroll_offset: 0,
            selected_row: None,
            sort_column: None,
            sort_dir: SortDir::Asc,
            focused: false,
            theme: default_theme(),
        }
    }

    pub fn visible_rows(&self, h: u32) -> usize {
        if self.row_height == 0 {
            return 0;
        }
        (h.saturating_sub(self.header_height) / self.row_height) as usize
    }

    /// Render with a callback that provides cell text: `cell_fn(row, col) -> &str`.
    pub fn render_with<F>(&self, canvas: &mut Canvas, x: i32, y: i32, w: u32, h: u32, cell_fn: F)
    where
        F: Fn(usize, usize) -> String,
    {
        let ux = x as usize;
        let uy = y as usize;
        canvas.fill_rect(ux, uy, w as usize, h as usize, self.theme.window_bg);

        // Header
        let mut col_x = ux;
        for (ci, col) in self.columns.iter().enumerate() {
            let cw = col.width as usize;
            canvas.fill_rect(
                col_x,
                uy,
                cw,
                self.header_height as usize,
                self.theme.chrome_bg,
            );
            canvas.draw_rect_outline(
                col_x,
                uy,
                cw,
                self.header_height as usize,
                self.theme.border,
            );
            canvas.draw_text(
                col_x + 4,
                uy + (self.header_height as usize - 8) / 2,
                &col.label,
                self.theme.title_fg,
                None,
            );
            if self.sort_column == Some(ci) {
                let arrow = match self.sort_dir {
                    SortDir::Asc => "^",
                    SortDir::Desc => "v",
                };
                canvas.draw_text(
                    col_x + cw - 12,
                    uy + (self.header_height as usize - 8) / 2,
                    arrow,
                    self.theme.border,
                    None,
                );
            }
            col_x += cw;
        }

        // Rows (virtual scrolling — only render visible)
        let vis = self.visible_rows(h);
        let end = (self.scroll_offset + vis).min(self.total_rows);
        let body_y = uy + self.header_height as usize;

        for ri in self.scroll_offset..end {
            let row_y = body_y + (ri - self.scroll_offset) * self.row_height as usize;
            let is_sel = self.selected_row == Some(ri);
            if is_sel {
                let total_w: usize = self.columns.iter().map(|c| c.width as usize).sum();
                canvas.fill_rect(
                    ux,
                    row_y,
                    total_w.min(w as usize),
                    self.row_height as usize,
                    self.theme.button_bg,
                );
            }
            let mut cx = ux;
            for ci in 0..self.columns.len() {
                let cw = self.columns[ci].width as usize;
                let text = cell_fn(ri, ci);
                let fg = if is_sel {
                    self.theme.title_fg
                } else {
                    self.theme.text_fg
                };
                canvas.draw_text(
                    cx + 4,
                    row_y + (self.row_height as usize - 8) / 2,
                    &text,
                    fg,
                    None,
                );
                cx += cw;
            }
        }

        // Vertical scrollbar
        if self.total_rows > vis && vis > 0 {
            let bar_w = 4usize;
            let bar_x = ux + w as usize - bar_w - 1;
            let bar_area_h = h as usize - self.header_height as usize;
            canvas.fill_rect(bar_x, body_y, bar_w, bar_area_h, 0xFF_22_22_33);
            let ratio = vis as f32 / self.total_rows as f32;
            let thumb_h = ((bar_area_h as f32) * ratio) as usize;
            let thumb_h = thumb_h.max(8);
            let scroll_frac = self.scroll_offset as f32 / (self.total_rows - vis) as f32;
            let thumb_y = body_y + ((bar_area_h - thumb_h) as f32 * scroll_frac) as usize;
            canvas.fill_rect(bar_x, thumb_y, bar_w, thumb_h, self.theme.border);
        }

        canvas.draw_rect_outline(ux, uy, w as usize, h as usize, self.theme.border);
        if self.focused {
            canvas.draw_rect_outline(ux, uy, w as usize, h as usize, self.theme.button_hot);
        }
    }

    pub fn render(&self, canvas: &mut Canvas, x: i32, y: i32, w: u32, h: u32) {
        self.render_with(canvas, x, y, w, h, |_r, _c| String::new());
    }

    pub fn handle_event(&mut self, event: &Event, x: i32, y: i32, w: u32, h: u32) {
        let vis = self.visible_rows(h);
        match event {
            Event::MouseClick { x: mx, y: my } => {
                if !hit(*mx, *my, x, y, w, h) {
                    self.focused = false;
                    return;
                }
                self.focused = true;
                let header_bottom = y + self.header_height as i32;
                if *my < header_bottom {
                    // Header click — sort
                    let mut cx = x;
                    for (ci, col) in self.columns.iter().enumerate() {
                        if *mx >= cx && *mx < cx + col.width as i32 && col.sortable {
                            if self.sort_column == Some(ci) {
                                self.sort_dir = match self.sort_dir {
                                    SortDir::Asc => SortDir::Desc,
                                    SortDir::Desc => SortDir::Asc,
                                };
                            } else {
                                self.sort_column = Some(ci);
                                self.sort_dir = SortDir::Asc;
                            }
                            break;
                        }
                        cx += col.width as i32;
                    }
                } else {
                    let rel_y = (*my - header_bottom) as usize;
                    let row = self.scroll_offset + rel_y / self.row_height as usize;
                    if row < self.total_rows {
                        self.selected_row = Some(row);
                    }
                }
            }
            Event::KeyPress(k) if self.focused => match *k {
                72 => {
                    if let Some(r) = self.selected_row {
                        if r > 0 {
                            self.selected_row = Some(r - 1);
                        }
                        if let Some(sr) = self.selected_row {
                            if sr < self.scroll_offset {
                                self.scroll_offset = sr;
                            }
                        }
                    }
                }
                80 => {
                    let next = match self.selected_row {
                        Some(r) => r + 1,
                        None => 0,
                    };
                    if next < self.total_rows {
                        self.selected_row = Some(next);
                        if next >= self.scroll_offset + vis {
                            self.scroll_offset = next - vis + 1;
                        }
                    }
                }
                73 => {
                    self.scroll_offset = self.scroll_offset.saturating_sub(vis);
                }
                81 => {
                    self.scroll_offset =
                        (self.scroll_offset + vis).min(self.total_rows.saturating_sub(vis));
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

// ── TreeView ────────────────────────────────────────────────────────────

pub struct TreeNode {
    pub label: String,
    pub expanded: bool,
    pub children: Vec<TreeNode>,
    pub icon: Option<char>,
}

impl TreeNode {
    pub fn new(label: &str) -> Self {
        Self {
            label: String::from(label),
            expanded: false,
            children: Vec::new(),
            icon: None,
        }
    }

    pub fn with_children(mut self, children: Vec<TreeNode>) -> Self {
        self.children = children;
        self
    }
}

pub struct TreeView {
    pub roots: Vec<TreeNode>,
    pub selected_path: Vec<usize>,
    pub focused: bool,
    pub indent: u32,
    pub item_height: u32,
    pub theme: Theme,
}

impl TreeView {
    pub fn new() -> Self {
        Self {
            roots: Vec::new(),
            selected_path: Vec::new(),
            focused: false,
            indent: 16,
            item_height: 20,
            theme: default_theme(),
        }
    }

    fn render_node(
        &self,
        canvas: &mut Canvas,
        node: &TreeNode,
        x: usize,
        y: &mut usize,
        depth: usize,
        w: usize,
        path: &mut Vec<usize>,
        idx: usize,
    ) {
        let indent = depth * self.indent as usize;
        let nx = x + indent;
        let ny = *y;
        path.push(idx);
        let is_selected = *path == self.selected_path;
        if is_selected {
            canvas.fill_rect(x, ny, w, self.item_height as usize, self.theme.button_bg);
        }
        // Expand/collapse arrow for nodes with children
        if !node.children.is_empty() {
            let arrow = if node.expanded { "v" } else { ">" };
            canvas.draw_text(
                nx,
                ny + (self.item_height as usize - 8) / 2,
                arrow,
                self.theme.border,
                None,
            );
        }
        let tx = nx + 12;
        if let Some(icon) = node.icon {
            let mut ibuf = [0u8; 4];
            let s = icon.encode_utf8(&mut ibuf);
            canvas.draw_text(
                tx,
                ny + (self.item_height as usize - 8) / 2,
                s,
                self.theme.border,
                None,
            );
        }
        let text_x = tx + if node.icon.is_some() { 12 } else { 0 };
        let fg = if is_selected {
            self.theme.title_fg
        } else {
            self.theme.text_fg
        };
        canvas.draw_text(
            text_x,
            ny + (self.item_height as usize - 8) / 2,
            &node.label,
            fg,
            None,
        );

        *y += self.item_height as usize;
        if node.expanded {
            for (ci, child) in node.children.iter().enumerate() {
                self.render_node(canvas, child, x, y, depth + 1, w, path, ci);
            }
        }
        path.pop();
    }

    pub fn render(&self, canvas: &mut Canvas, x: i32, y: i32, w: u32, h: u32) {
        let ux = x as usize;
        let uy = y as usize;
        canvas.fill_rect(ux, uy, w as usize, h as usize, self.theme.window_bg);
        canvas.draw_rect_outline(ux, uy, w as usize, h as usize, self.theme.border);
        let mut cy = uy;
        let mut path = Vec::new();
        for (i, root) in self.roots.iter().enumerate() {
            if cy >= uy + h as usize {
                break;
            }
            self.render_node(canvas, root, ux, &mut cy, 0, w as usize, &mut path, i);
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
                    let idx = rel_y / self.item_height as usize;
                    let path = self.flat_path_at(idx);
                    if let Some(p) = path {
                        if let Some(node) = self.get_node_mut(&p) {
                            if !node.children.is_empty() {
                                node.expanded = !node.expanded;
                            }
                        }
                        self.selected_path = p;
                    }
                }
            }
            Event::KeyPress(k) if self.focused => match *k {
                13 | 32 => {
                    let p = self.selected_path.clone();
                    if let Some(node) = self.get_node_mut(&p) {
                        if !node.children.is_empty() {
                            node.expanded = !node.expanded;
                        }
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }

    fn flat_path_at(&self, target: usize) -> Option<Vec<usize>> {
        let mut counter = 0usize;
        let mut path = Vec::new();
        for (i, root) in self.roots.iter().enumerate() {
            if let Some(p) = Self::find_in_node(root, target, &mut counter, &mut path, i) {
                return Some(p);
            }
        }
        None
    }

    fn find_in_node(
        node: &TreeNode,
        target: usize,
        counter: &mut usize,
        path: &mut Vec<usize>,
        idx: usize,
    ) -> Option<Vec<usize>> {
        path.push(idx);
        if *counter == target {
            return Some(path.clone());
        }
        *counter += 1;
        if node.expanded {
            for (ci, child) in node.children.iter().enumerate() {
                if let Some(p) = Self::find_in_node(child, target, counter, path, ci) {
                    return Some(p);
                }
            }
        }
        path.pop();
        None
    }

    fn get_node_mut(&mut self, path: &[usize]) -> Option<&mut TreeNode> {
        if path.is_empty() {
            return None;
        }
        let mut node = self.roots.get_mut(path[0])?;
        for &idx in &path[1..] {
            node = node.children.get_mut(idx)?;
        }
        Some(node)
    }

    pub fn accessibility_role(&self) -> AccessibilityRole {
        AccessibilityRole::List
    }
}

// ── VirtualListView ─────────────────────────────────────────────────────

/// High-performance virtual-scrolled list. Stores no per-item data; the
/// caller provides a `render_item` callback. Handles 100K+ items.
pub struct VirtualListView {
    pub total_items: usize,
    pub item_height: u32,
    pub scroll_offset: usize,
    pub selected: Option<usize>,
    pub focused: bool,
    pub theme: Theme,
}

impl VirtualListView {
    pub fn new(total_items: usize, item_height: u32) -> Self {
        Self {
            total_items,
            item_height,
            scroll_offset: 0,
            selected: None,
            focused: false,
            theme: default_theme(),
        }
    }

    pub fn visible_count(&self, h: u32) -> usize {
        if self.item_height == 0 {
            return 0;
        }
        h as usize / self.item_height as usize
    }

    pub fn render_with<F>(
        &self,
        canvas: &mut Canvas,
        x: i32,
        y: i32,
        w: u32,
        h: u32,
        render_item: F,
    ) where
        F: Fn(&mut Canvas, usize, usize, usize, usize, usize, bool),
    {
        let ux = x as usize;
        let uy = y as usize;
        canvas.fill_rect(ux, uy, w as usize, h as usize, self.theme.window_bg);
        canvas.draw_rect_outline(ux, uy, w as usize, h as usize, self.theme.border);

        let vis = self.visible_count(h);
        let end = (self.scroll_offset + vis).min(self.total_items);
        for i in self.scroll_offset..end {
            let iy = uy + (i - self.scroll_offset) * self.item_height as usize;
            let is_sel = self.selected == Some(i);
            if is_sel {
                canvas.fill_rect(
                    ux + 1,
                    iy,
                    w as usize - 2,
                    self.item_height as usize,
                    self.theme.button_bg,
                );
            }
            render_item(
                canvas,
                i,
                ux + 4,
                iy,
                w as usize - 8,
                self.item_height as usize,
                is_sel,
            );
        }

        // Scrollbar
        if self.total_items > vis && vis > 0 {
            let bar_w = 4usize;
            let bar_x = ux + w as usize - bar_w - 1;
            canvas.fill_rect(bar_x, uy, bar_w, h as usize, 0xFF_22_22_33);
            let ratio = vis as f32 / self.total_items as f32;
            let thumb_h = ((h as f32) * ratio) as usize;
            let thumb_h = thumb_h.max(8);
            let scroll_frac = self.scroll_offset as f32 / (self.total_items - vis).max(1) as f32;
            let thumb_y = uy + ((h as usize - thumb_h) as f32 * scroll_frac) as usize;
            canvas.fill_rect(bar_x, thumb_y, bar_w, thumb_h, self.theme.border);
        }

        if self.focused {
            canvas.draw_rect_outline(ux, uy, w as usize, h as usize, self.theme.button_hot);
        }
    }

    pub fn render(&self, canvas: &mut Canvas, x: i32, y: i32, w: u32, h: u32) {
        self.render_with(canvas, x, y, w, h, |_, _, _, _, _, _, _| {});
    }

    pub fn handle_event(&mut self, event: &Event, x: i32, y: i32, w: u32, h: u32) {
        let vis = self.visible_count(h);
        match event {
            Event::MouseClick { x: mx, y: my } => {
                if !hit(*mx, *my, x, y, w, h) {
                    self.focused = false;
                    return;
                }
                self.focused = true;
                let rel_y = (*my - y) as usize;
                let idx = self.scroll_offset + rel_y / self.item_height as usize;
                if idx < self.total_items {
                    self.selected = Some(idx);
                }
            }
            Event::KeyPress(k) if self.focused => match *k {
                72 => {
                    if let Some(s) = self.selected {
                        if s > 0 {
                            self.selected = Some(s - 1);
                            if s - 1 < self.scroll_offset {
                                self.scroll_offset = s - 1;
                            }
                        }
                    }
                }
                80 => {
                    let next = match self.selected {
                        Some(s) => s + 1,
                        None => 0,
                    };
                    if next < self.total_items {
                        self.selected = Some(next);
                        if next >= self.scroll_offset + vis {
                            self.scroll_offset = next - vis + 1;
                        }
                    }
                }
                73 => {
                    self.scroll_offset = self.scroll_offset.saturating_sub(vis);
                }
                81 => {
                    self.scroll_offset =
                        (self.scroll_offset + vis).min(self.total_items.saturating_sub(vis));
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

// ── GridView ────────────────────────────────────────────────────────────

pub struct GridView {
    pub total_items: usize,
    pub cell_width: u32,
    pub cell_height: u32,
    pub gap: u32,
    pub scroll_offset_rows: usize,
    pub selected: Option<usize>,
    pub focused: bool,
    pub theme: Theme,
}

impl GridView {
    pub fn new(total_items: usize, cell_width: u32, cell_height: u32) -> Self {
        Self {
            total_items,
            cell_width,
            cell_height,
            gap: 4,
            scroll_offset_rows: 0,
            selected: None,
            focused: false,
            theme: default_theme(),
        }
    }

    fn cols(&self, w: u32) -> usize {
        let cw = self.cell_width + self.gap;
        if cw == 0 {
            return 1;
        }
        (w as usize / cw as usize).max(1)
    }

    fn total_rows(&self, w: u32) -> usize {
        let c = self.cols(w);
        (self.total_items + c - 1) / c
    }

    fn visible_rows(&self, w: u32, h: u32) -> usize {
        let ch = self.cell_height + self.gap;
        if ch == 0 {
            return 1;
        }
        (h as usize / ch as usize).max(1)
    }

    pub fn render_with<F>(
        &self,
        canvas: &mut Canvas,
        x: i32,
        y: i32,
        w: u32,
        h: u32,
        render_cell: F,
    ) where
        F: Fn(&mut Canvas, usize, usize, usize, usize, usize, bool),
    {
        let ux = x as usize;
        let uy = y as usize;
        canvas.fill_rect(ux, uy, w as usize, h as usize, self.theme.window_bg);
        let cols = self.cols(w);
        let vis_rows = self.visible_rows(w, h);
        let end_row = (self.scroll_offset_rows + vis_rows).min(self.total_rows(w));

        for row in self.scroll_offset_rows..end_row {
            for col in 0..cols {
                let item_idx = row * cols + col;
                if item_idx >= self.total_items {
                    break;
                }
                let cx = ux + col * (self.cell_width + self.gap) as usize;
                let cy =
                    uy + (row - self.scroll_offset_rows) * (self.cell_height + self.gap) as usize;
                let is_sel = self.selected == Some(item_idx);
                if is_sel {
                    canvas.draw_rect_outline(
                        cx,
                        cy,
                        self.cell_width as usize,
                        self.cell_height as usize,
                        self.theme.button_hot,
                    );
                }
                render_cell(
                    canvas,
                    item_idx,
                    cx,
                    cy,
                    self.cell_width as usize,
                    self.cell_height as usize,
                    is_sel,
                );
            }
        }
        canvas.draw_rect_outline(ux, uy, w as usize, h as usize, self.theme.border);
        if self.focused {
            canvas.draw_rect_outline(ux, uy, w as usize, h as usize, self.theme.button_hot);
        }
    }

    pub fn render(&self, canvas: &mut Canvas, x: i32, y: i32, w: u32, h: u32) {
        self.render_with(canvas, x, y, w, h, |_, _, _, _, _, _, _| {});
    }

    pub fn handle_event(&mut self, event: &Event, x: i32, y: i32, w: u32, h: u32) {
        let cols = self.cols(w);
        match event {
            Event::MouseClick { x: mx, y: my } => {
                if !hit(*mx, *my, x, y, w, h) {
                    self.focused = false;
                    return;
                }
                self.focused = true;
                let rx = (*mx - x) as usize;
                let ry = (*my - y) as usize;
                let col = rx / (self.cell_width + self.gap) as usize;
                let row = self.scroll_offset_rows + ry / (self.cell_height + self.gap) as usize;
                let idx = row * cols + col;
                if idx < self.total_items {
                    self.selected = Some(idx);
                }
            }
            Event::KeyPress(k) if self.focused => match *k {
                75 => {
                    if let Some(s) = self.selected {
                        if s > 0 {
                            self.selected = Some(s - 1);
                        }
                    }
                }
                77 => {
                    if let Some(s) = self.selected {
                        if s + 1 < self.total_items {
                            self.selected = Some(s + 1);
                        }
                    }
                }
                72 => {
                    if let Some(s) = self.selected {
                        if s >= cols {
                            self.selected = Some(s - cols);
                        }
                    }
                }
                80 => {
                    if let Some(s) = self.selected {
                        if s + cols < self.total_items {
                            self.selected = Some(s + cols);
                        }
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

// ── Badge ───────────────────────────────────────────────────────────────

pub struct Badge {
    pub count: u32,
    pub max_display: u32,
    pub color: u32,
    pub theme: Theme,
}

impl Badge {
    pub fn new(count: u32) -> Self {
        Self {
            count,
            max_display: 99,
            color: 0xFF_FF_2E_88,
            theme: default_theme(),
        }
    }

    pub fn render(&self, canvas: &mut Canvas, x: i32, y: i32, w: u32, h: u32) {
        if self.count == 0 {
            return;
        }
        let ux = x as usize;
        let uy = y as usize;
        canvas.fill_rect(ux, uy, w as usize, h as usize, self.color);
        let mut buf = [0u8; 10];
        let text = if self.count > self.max_display {
            let mut nbuf = [0u8; 10];
            let s = u32_to_decimal(self.max_display, &mut nbuf);
            let mut out = String::from(s);
            out.push('+');
            out
        } else {
            String::from(u32_to_decimal(self.count, &mut buf))
        };
        let tx = ux + (w as usize - text.len() * 8) / 2;
        canvas.draw_text(tx, uy + (h as usize - 8) / 2, &text, 0xFF_FF_FF_FF, None);
    }

    pub fn accessibility_role(&self) -> AccessibilityRole {
        AccessibilityRole::Label
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

// ── Tag ─────────────────────────────────────────────────────────────────

pub struct Tag {
    pub label: String,
    pub color: u32,
    pub removable: bool,
    pub theme: Theme,
}

impl Tag {
    pub fn new(label: &str, color: u32) -> Self {
        Self {
            label: String::from(label),
            color,
            removable: false,
            theme: default_theme(),
        }
    }

    pub fn render(&self, canvas: &mut Canvas, x: i32, y: i32, w: u32, h: u32) {
        let ux = x as usize;
        let uy = y as usize;
        canvas.fill_rect(ux, uy, w as usize, h as usize, self.color);
        canvas.draw_text(
            ux + 4,
            uy + (h as usize - 8) / 2,
            &self.label,
            0xFF_FF_FF_FF,
            None,
        );
        if self.removable {
            canvas.draw_text(
                ux + w as usize - 12,
                uy + (h as usize - 8) / 2,
                "x",
                0xFF_FF_FF_FF,
                None,
            );
        }
    }

    pub fn accessibility_role(&self) -> AccessibilityRole {
        AccessibilityRole::Label
    }
}

// ── Tooltip ─────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TooltipPosition {
    Top,
    Bottom,
    Left,
    Right,
}

pub struct Tooltip {
    pub text: String,
    pub position: TooltipPosition,
    pub visible: bool,
    pub delay_ms: u32,
    pub elapsed_ms: u32,
    pub theme: Theme,
}

impl Tooltip {
    pub fn new(text: &str, position: TooltipPosition) -> Self {
        Self {
            text: String::from(text),
            position,
            visible: false,
            delay_ms: 500,
            elapsed_ms: 0,
            theme: default_theme(),
        }
    }

    pub fn tick_hover(&mut self, hovering: bool, dt_ms: u32) {
        if hovering {
            self.elapsed_ms += dt_ms;
            if self.elapsed_ms >= self.delay_ms {
                self.visible = true;
            }
        } else {
            self.elapsed_ms = 0;
            self.visible = false;
        }
    }

    pub fn render(&self, canvas: &mut Canvas, x: i32, y: i32, _w: u32, _h: u32) {
        if !self.visible {
            return;
        }
        let ux = x as usize;
        let uy = y as usize;
        let tw = self.text.len() * 8 + 12;
        let th = 20usize;
        canvas.fill_rect(ux, uy, tw, th, 0xFF_22_22_33);
        canvas.draw_rect_outline(ux, uy, tw, th, self.theme.border);
        canvas.draw_text(
            ux + 6,
            uy + (th - 8) / 2,
            &self.text,
            self.theme.text_fg,
            None,
        );
    }

    pub fn accessibility_role(&self) -> AccessibilityRole {
        AccessibilityRole::Label
    }
}

// ── Avatar ──────────────────────────────────────────────────────────────

pub struct Avatar {
    pub initials: String,
    pub bg_color: u32,
    pub theme: Theme,
}

impl Avatar {
    pub fn new(initials: &str, bg_color: u32) -> Self {
        Self {
            initials: String::from(initials),
            bg_color,
            theme: default_theme(),
        }
    }

    pub fn render(&self, canvas: &mut Canvas, x: i32, y: i32, w: u32, h: u32) {
        let ux = x as usize;
        let uy = y as usize;
        let r = (w.min(h) / 2) as usize;
        let cx = ux + w as usize / 2;
        let cy = uy + h as usize / 2;
        // Circle fill
        for dy in 0..r * 2 {
            for dx in 0..r * 2 {
                let rx = dx as i32 - r as i32;
                let ry = dy as i32 - r as i32;
                if rx * rx + ry * ry <= (r as i32) * (r as i32) {
                    let px = (cx as i32 + rx) as usize;
                    let py = (cy as i32 + ry) as usize;
                    canvas.draw_pixel(px, py, self.bg_color);
                }
            }
        }
        // Centered initials
        let tw = self.initials.len() * 8;
        let tx = cx.saturating_sub(tw / 2);
        let ty = cy.saturating_sub(4);
        canvas.draw_text(tx, ty, &self.initials, 0xFF_FF_FF_FF, None);
    }

    pub fn accessibility_role(&self) -> AccessibilityRole {
        AccessibilityRole::Image
    }
}

// ── Breadcrumb ──────────────────────────────────────────────────────────

pub struct Breadcrumb {
    pub segments: Vec<String>,
    pub focused: bool,
    pub theme: Theme,
}

impl Breadcrumb {
    pub fn new(segments: &[&str]) -> Self {
        let mut v = Vec::new();
        for s in segments {
            v.push(String::from(*s));
        }
        Self {
            segments: v,
            focused: false,
            theme: default_theme(),
        }
    }

    pub fn render(&self, canvas: &mut Canvas, x: i32, y: i32, _w: u32, h: u32) {
        let ux = x as usize;
        let uy = y as usize;
        let mut tx = ux;
        let ty = uy + (h as usize - 8) / 2;
        for (i, seg) in self.segments.iter().enumerate() {
            let is_last = i == self.segments.len() - 1;
            let fg = if is_last {
                self.theme.title_fg
            } else {
                self.theme.border
            };
            canvas.draw_text(tx, ty, seg, fg, None);
            tx += seg.len() * 8;
            if !is_last {
                canvas.draw_text(tx + 4, ty, "/", self.theme.text_fg, None);
                tx += 16;
            }
        }
    }

    /// Returns index of clicked segment, if any.
    pub fn handle_event(
        &mut self,
        event: &Event,
        x: i32,
        y: i32,
        _w: u32,
        h: u32,
    ) -> Option<usize> {
        if let Event::MouseClick { x: mx, y: my } = event {
            if *my < y || *my >= y + h as i32 {
                return None;
            }
            let mut sx = x;
            for (i, seg) in self.segments.iter().enumerate() {
                let seg_w = seg.len() as i32 * 8;
                if *mx >= sx && *mx < sx + seg_w {
                    return Some(i);
                }
                sx += seg_w + 16;
            }
        }
        None
    }

    pub fn accessibility_role(&self) -> AccessibilityRole {
        AccessibilityRole::Group
    }
}

// ── Chart (line / bar / pie) ────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChartKind {
    Line,
    Bar,
    Pie,
}

pub struct ChartDataPoint {
    pub value: f32,
    pub label: String,
}

pub struct Chart {
    pub kind: ChartKind,
    pub data: Vec<ChartDataPoint>,
    pub title: String,
    pub theme: Theme,
}

impl Chart {
    pub fn new(kind: ChartKind, title: &str) -> Self {
        Self {
            kind,
            data: Vec::new(),
            title: String::from(title),
            theme: default_theme(),
        }
    }

    pub fn add_point(&mut self, value: f32, label: &str) {
        self.data.push(ChartDataPoint {
            value,
            label: String::from(label),
        });
    }

    pub fn render(&self, canvas: &mut Canvas, x: i32, y: i32, w: u32, h: u32) {
        let ux = x as usize;
        let uy = y as usize;
        canvas.fill_rect(ux, uy, w as usize, h as usize, self.theme.window_bg);
        canvas.draw_rect_outline(ux, uy, w as usize, h as usize, self.theme.border);
        canvas.draw_text(ux + 4, uy + 4, &self.title, self.theme.title_fg, None);

        if self.data.is_empty() {
            return;
        }

        let chart_x = ux + 30;
        let chart_y = uy + 20;
        let chart_w = (w as usize).saturating_sub(40);
        let chart_h = (h as usize).saturating_sub(40);
        if chart_w == 0 || chart_h == 0 {
            return;
        }

        let max_val = self
            .data
            .iter()
            .map(|d| d.value)
            .fold(0.0f32, f32::max)
            .max(1.0);

        match self.kind {
            ChartKind::Line => {
                let step = if self.data.len() > 1 {
                    chart_w / (self.data.len() - 1)
                } else {
                    chart_w
                };
                for i in 0..self.data.len() {
                    let px = chart_x + i * step;
                    let frac = self.data[i].value / max_val;
                    let py = chart_y + chart_h - (chart_h as f32 * frac) as usize;
                    // Data point
                    for dy in 0..3usize {
                        for dx in 0..3usize {
                            canvas.draw_pixel(px + dx, py + dy, self.theme.button_hot);
                        }
                    }
                    // Line to next
                    if i + 1 < self.data.len() {
                        let nx = chart_x + (i + 1) * step;
                        let nfrac = self.data[i + 1].value / max_val;
                        let ny = chart_y + chart_h - (chart_h as f32 * nfrac) as usize;
                        draw_line(canvas, px + 1, py + 1, nx + 1, ny + 1, self.theme.border);
                    }
                }
            }
            ChartKind::Bar => {
                if self.data.is_empty() {
                    return;
                }
                let bar_w = chart_w / self.data.len();
                let gap = 2usize;
                for (i, dp) in self.data.iter().enumerate() {
                    let bx = chart_x + i * bar_w + gap;
                    let bw = bar_w - gap * 2;
                    let frac = dp.value / max_val;
                    let bh = (chart_h as f32 * frac) as usize;
                    let by = chart_y + chart_h - bh;
                    canvas.fill_rect(bx, by, bw, bh, self.theme.border);
                }
            }
            ChartKind::Pie => {
                let total: f32 = self.data.iter().map(|d| d.value).sum();
                if total <= 0.0 {
                    return;
                }
                let cr = chart_h.min(chart_w) / 2;
                let ccx = chart_x + chart_w / 2;
                let ccy = chart_y + chart_h / 2;
                let colors = [
                    self.theme.border,
                    self.theme.button_hot,
                    self.theme.button_bg,
                    0xFF_8B_6F_47,
                    0xFF_6B_8F_71,
                ];
                let mut start_angle: f32 = 0.0;
                for (i, dp) in self.data.iter().enumerate() {
                    let sweep = dp.value / total * 360.0;
                    let col = colors[i % colors.len()];
                    fill_arc(canvas, ccx, ccy, cr, start_angle, start_angle + sweep, col);
                    start_angle += sweep;
                }
            }
        }
        // Axis labels
        for px in chart_x..chart_x + chart_w {
            canvas.draw_pixel(px, chart_y + chart_h, self.theme.text_fg);
        }
        for py in chart_y..chart_y + chart_h {
            canvas.draw_pixel(chart_x, py, self.theme.text_fg);
        }
    }

    pub fn accessibility_role(&self) -> AccessibilityRole {
        AccessibilityRole::Image
    }
}

fn draw_line(canvas: &mut Canvas, x0: usize, y0: usize, x1: usize, y1: usize, color: u32) {
    let dx = if x1 > x0 { x1 - x0 } else { x0 - x1 };
    let dy = if y1 > y0 { y1 - y0 } else { y0 - y1 };
    let steps = dx.max(dy).max(1);
    for s in 0..=steps {
        let t = s as f32 / steps as f32;
        let px = x0 as f32 + (x1 as f32 - x0 as f32) * t;
        let py = y0 as f32 + (y1 as f32 - y0 as f32) * t;
        canvas.draw_pixel(px as usize, py as usize, color);
    }
}

fn fill_arc(
    canvas: &mut Canvas,
    cx: usize,
    cy: usize,
    r: usize,
    start_deg: f32,
    end_deg: f32,
    color: u32,
) {
    for dy in 0..r * 2 {
        for dx in 0..r * 2 {
            let rx = dx as i32 - r as i32;
            let ry = dy as i32 - r as i32;
            if rx * rx + ry * ry > (r as i32) * (r as i32) {
                continue;
            }
            let angle = libm::atan2f(ry as f32, rx as f32) * 180.0 / core::f32::consts::PI;
            let angle = if angle < 0.0 { angle + 360.0 } else { angle };
            let in_arc = if start_deg <= end_deg {
                angle >= start_deg && angle < end_deg
            } else {
                angle >= start_deg || angle < end_deg
            };
            if in_arc {
                canvas.draw_pixel((cx as i32 + rx) as usize, (cy as i32 + ry) as usize, color);
            }
        }
    }
}

// ── Sparkline ───────────────────────────────────────────────────────────

pub struct Sparkline {
    pub values: Vec<f32>,
    pub color: u32,
    pub theme: Theme,
}

impl Sparkline {
    pub fn new(values: &[f32]) -> Self {
        let mut v = Vec::new();
        for &val in values {
            v.push(val);
        }
        Self {
            values: v,
            color: 0xFF_4E_9C_FF,
            theme: default_theme(),
        }
    }

    pub fn render(&self, canvas: &mut Canvas, x: i32, y: i32, w: u32, h: u32) {
        if self.values.is_empty() {
            return;
        }
        let ux = x as usize;
        let uy = y as usize;
        let max_val = self.values.iter().copied().fold(0.0f32, f32::max).max(1.0);
        let count = self.values.len();
        let step = if count > 1 {
            w as usize / (count - 1)
        } else {
            w as usize
        };
        for i in 0..count {
            let px = ux + i * step;
            let frac = self.values[i] / max_val;
            let py = uy + h as usize - (h as f32 * frac) as usize;
            canvas.draw_pixel(px, py, self.color);
            if i + 1 < count {
                let nx = ux + (i + 1) * step;
                let nfrac = self.values[i + 1] / max_val;
                let ny = uy + h as usize - (h as f32 * nfrac) as usize;
                draw_line(canvas, px, py, nx, ny, self.color);
            }
        }
    }

    pub fn accessibility_role(&self) -> AccessibilityRole {
        AccessibilityRole::Image
    }
}

// ── StatusIndicator ─────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusLevel {
    Green,
    Yellow,
    Red,
    Grey,
}

pub struct StatusIndicator {
    pub level: StatusLevel,
    pub label: String,
    pub theme: Theme,
}

impl StatusIndicator {
    pub fn new(level: StatusLevel, label: &str) -> Self {
        Self {
            level,
            label: String::from(label),
            theme: default_theme(),
        }
    }

    fn dot_color(&self) -> u32 {
        match self.level {
            StatusLevel::Green => 0xFF_00_CC_66,
            StatusLevel::Yellow => 0xFF_FF_CC_00,
            StatusLevel::Red => 0xFF_FF_33_33,
            StatusLevel::Grey => 0xFF_88_88_88,
        }
    }

    pub fn render(&self, canvas: &mut Canvas, x: i32, y: i32, _w: u32, h: u32) {
        let ux = x as usize;
        let uy = y as usize;
        let r = 4usize;
        let cx = ux + r;
        let cy = uy + h as usize / 2;
        for dy in 0..r * 2 {
            for dx in 0..r * 2 {
                let rx = dx as i32 - r as i32;
                let ry = dy as i32 - r as i32;
                if rx * rx + ry * ry <= (r as i32) * (r as i32) {
                    canvas.draw_pixel(
                        (cx as i32 + rx) as usize,
                        (cy as i32 + ry) as usize,
                        self.dot_color(),
                    );
                }
            }
        }
        canvas.draw_text(
            ux + r * 2 + 6,
            uy + (h as usize - 8) / 2,
            &self.label,
            self.theme.text_fg,
            None,
        );
    }

    pub fn accessibility_role(&self) -> AccessibilityRole {
        AccessibilityRole::Label
    }
}
