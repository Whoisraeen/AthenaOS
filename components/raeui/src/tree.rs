//! RaeUI Widget Tree
//!
//! A tree structure that holds widgets, runs layout, renders them at computed
//! positions, and performs hit-testing for input routing.

extern crate alloc;
use alloc::boxed::Box;
use alloc::vec::Vec;
use raegfx::Canvas;

use crate::layout::{compute_layout, ComputedLayout, Dimension, Edges, LayoutNode, LayoutStyle};
use crate::Event;

// ── Widget2 Trait ───────────────────────────────────────────────────────
// New layout-aware widget trait. Widgets report their intrinsic size and
// render at the position determined by the layout engine.

pub trait Widget2 {
    fn intrinsic_size(&self) -> (f32, f32);
    fn layout_style(&self) -> LayoutStyle {
        LayoutStyle::default()
    }
    fn render_at(&self, canvas: &mut Canvas, x: f32, y: f32, w: f32, h: f32);
    fn on_event(&mut self, event: &Event, x: f32, y: f32, w: f32, h: f32) -> bool;
    fn children_mut(&mut self) -> &mut [WidgetNode] {
        &mut []
    }
    fn children(&self) -> &[WidgetNode] {
        &[]
    }
}

// ── Widget Node ─────────────────────────────────────────────────────────

pub struct WidgetNode {
    pub widget: Box<dyn Widget2>,
    pub children: Vec<WidgetNode>,
    pub computed: ComputedLayout,
    pub id: u32,
}

static mut NEXT_ID: u32 = 1;

fn alloc_id() -> u32 {
    unsafe {
        let id = NEXT_ID;
        NEXT_ID += 1;
        id
    }
}

impl WidgetNode {
    pub fn new(widget: Box<dyn Widget2>) -> Self {
        Self {
            widget,
            children: Vec::new(),
            computed: ComputedLayout::default(),
            id: alloc_id(),
        }
    }

    pub fn with_children(widget: Box<dyn Widget2>, children: Vec<WidgetNode>) -> Self {
        Self {
            widget,
            children,
            computed: ComputedLayout::default(),
            id: alloc_id(),
        }
    }

    pub fn add_child(&mut self, child: WidgetNode) {
        self.children.push(child);
    }
}

// ── Widget Tree ─────────────────────────────────────────────────────────

pub struct WidgetTree {
    pub root: WidgetNode,
}

impl WidgetTree {
    pub fn new(root: WidgetNode) -> Self {
        Self { root }
    }

    pub fn layout(&mut self, width: f32, height: f32) {
        let mut layout_tree = build_layout_tree(&self.root);
        compute_layout(&mut layout_tree, width, height);
        apply_layout(&mut self.root, &layout_tree);
    }

    pub fn render(&self, canvas: &mut Canvas) {
        render_node(&self.root, canvas, 0.0, 0.0);
    }

    pub fn hit_test(&self, x: f32, y: f32) -> Option<u32> {
        hit_test_node(&self.root, x, y, 0.0, 0.0)
    }

    pub fn dispatch_event(&mut self, event: &Event) {
        dispatch_node(&mut self.root, event, 0.0, 0.0);
    }

    pub fn find_mut(&mut self, id: u32) -> Option<&mut WidgetNode> {
        find_node_mut(&mut self.root, id)
    }
}

// ── Tree Traversal Helpers ──────────────────────────────────────────────

fn build_layout_tree(node: &WidgetNode) -> LayoutNode {
    let style = node.widget.layout_style();
    let (iw, ih) = node.widget.intrinsic_size();
    let mut ln = LayoutNode {
        style,
        children: Vec::with_capacity(node.children.len()),
        computed: ComputedLayout::default(),
        intrinsic_width: if iw > 0.0 { Some(iw) } else { None },
        intrinsic_height: if ih > 0.0 { Some(ih) } else { None },
    };
    for child in &node.children {
        ln.children.push(build_layout_tree(child));
    }
    ln
}

fn apply_layout(node: &mut WidgetNode, layout: &LayoutNode) {
    node.computed = layout.computed;
    for (child, child_layout) in node.children.iter_mut().zip(layout.children.iter()) {
        apply_layout(child, child_layout);
    }
}

fn render_node(node: &WidgetNode, canvas: &mut Canvas, parent_x: f32, parent_y: f32) {
    let abs_x = parent_x + node.computed.x;
    let abs_y = parent_y + node.computed.y;
    node.widget.render_at(
        canvas,
        abs_x,
        abs_y,
        node.computed.width,
        node.computed.height,
    );
    for child in &node.children {
        render_node(child, canvas, abs_x, abs_y);
    }
}

fn hit_test_node(node: &WidgetNode, x: f32, y: f32, parent_x: f32, parent_y: f32) -> Option<u32> {
    let abs_x = parent_x + node.computed.x;
    let abs_y = parent_y + node.computed.y;

    // Check children first (front-to-back, last child renders on top)
    for child in node.children.iter().rev() {
        if let Some(id) = hit_test_node(child, x, y, abs_x, abs_y) {
            return Some(id);
        }
    }

    // Check self
    if x >= abs_x
        && x < abs_x + node.computed.width
        && y >= abs_y
        && y < abs_y + node.computed.height
    {
        return Some(node.id);
    }
    None
}

fn dispatch_node(node: &mut WidgetNode, event: &Event, parent_x: f32, parent_y: f32) {
    let abs_x = parent_x + node.computed.x;
    let abs_y = parent_y + node.computed.y;

    let w = node.computed.width;
    let h = node.computed.height;
    node.widget.on_event(event, abs_x, abs_y, w, h);

    for child in &mut node.children {
        dispatch_node(child, event, abs_x, abs_y);
    }
}

fn find_node_mut(node: &mut WidgetNode, id: u32) -> Option<&mut WidgetNode> {
    if node.id == id {
        return Some(node);
    }
    for child in &mut node.children {
        if let Some(found) = find_node_mut(child, id) {
            return Some(found);
        }
    }
    None
}

// ── Layout-Aware Widget Implementations ─────────────────────────────────

use crate::theme;
use alloc::string::String;

/// A label widget that uses the layout system.
pub struct LayoutLabel {
    pub text: String,
    pub fg: u32,
    pub font_size: u16,
}

impl LayoutLabel {
    pub fn new(text: &str) -> Self {
        Self {
            text: String::from(text),
            fg: theme::TEXT_FG,
            font_size: 14,
        }
    }
}

impl Widget2 for LayoutLabel {
    fn intrinsic_size(&self) -> (f32, f32) {
        let char_w = self.font_size as f32 * 0.6;
        let w = self.text.chars().count() as f32 * char_w;
        let h = self.font_size as f32 * 1.2;
        (w, h)
    }

    fn render_at(&self, canvas: &mut Canvas, x: f32, y: f32, _w: f32, _h: f32) {
        canvas.draw_text(x as usize, y as usize, &self.text, self.fg, None);
    }

    fn on_event(&mut self, _event: &Event, _x: f32, _y: f32, _w: f32, _h: f32) -> bool {
        false
    }
}

/// A button widget that uses the layout system.
pub struct LayoutButton {
    pub text: String,
    pub pressed: bool,
    pub hovered: bool,
}

impl LayoutButton {
    pub fn new(text: &str) -> Self {
        Self {
            text: String::from(text),
            pressed: false,
            hovered: false,
        }
    }
}

impl Widget2 for LayoutButton {
    fn intrinsic_size(&self) -> (f32, f32) {
        let text_w = self.text.chars().count() as f32 * 8.0;
        (text_w + 24.0, 28.0)
    }

    fn layout_style(&self) -> LayoutStyle {
        LayoutStyle {
            padding: Edges::symmetric(6.0, 12.0),
            ..Default::default()
        }
    }

    fn render_at(&self, canvas: &mut Canvas, x: f32, y: f32, w: f32, h: f32) {
        let bg = if self.pressed {
            theme::BUTTON_HOT
        } else if self.hovered {
            0xFF_44_44_66
        } else {
            theme::BUTTON_BG
        };
        let ix = x as usize;
        let iy = y as usize;
        let iw = w as usize;
        let ih = h as usize;
        canvas.fill_rect(ix, iy, iw, ih, bg);
        canvas.draw_rect_outline(ix, iy, iw, ih, theme::BORDER);

        let text_w = self.text.chars().count() * 8;
        let tx = ix + iw.saturating_sub(text_w) / 2;
        let ty = iy + ih.saturating_sub(8) / 2;
        canvas.draw_text(tx, ty, &self.text, theme::BUTTON_TEXT, None);
    }

    fn on_event(&mut self, event: &Event, x: f32, y: f32, w: f32, h: f32) -> bool {
        match event {
            Event::MouseClick { x: mx, y: my } => {
                let in_bounds = *mx as f32 >= x
                    && (*mx as f32) < x + w
                    && *my as f32 >= y
                    && (*my as f32) < y + h;
                if in_bounds {
                    self.pressed = !self.pressed;
                    return true;
                }
            }
            _ => {}
        }
        false
    }
}

/// A container (frame) that holds children and applies flex layout.
pub struct LayoutFrame {
    pub bg_color: u32,
}

impl LayoutFrame {
    pub fn new() -> Self {
        Self { bg_color: 0 }
    }

    pub fn with_bg(bg: u32) -> Self {
        Self { bg_color: bg }
    }
}

impl Widget2 for LayoutFrame {
    fn intrinsic_size(&self) -> (f32, f32) {
        (0.0, 0.0)
    }

    fn render_at(&self, canvas: &mut Canvas, x: f32, y: f32, w: f32, h: f32) {
        if self.bg_color != 0 {
            canvas.fill_rect(
                x as usize,
                y as usize,
                w as usize,
                h as usize,
                self.bg_color,
            );
        }
    }

    fn on_event(&mut self, _event: &Event, _x: f32, _y: f32, _w: f32, _h: f32) -> bool {
        false
    }
}

/// A single-line text input with cursor and basic editing.
pub struct LayoutTextInput {
    pub text: String,
    pub cursor_pos: usize,
    pub focused: bool,
    pub placeholder: String,
    pub selection_start: Option<usize>,
}

impl LayoutTextInput {
    pub fn new(placeholder: &str) -> Self {
        Self {
            text: String::new(),
            cursor_pos: 0,
            focused: false,
            placeholder: String::from(placeholder),
            selection_start: None,
        }
    }

    pub fn insert_char(&mut self, ch: char) {
        self.delete_selection();
        if self.cursor_pos <= self.text.len() {
            self.text.insert(self.cursor_pos, ch);
            self.cursor_pos += 1;
        }
    }

    pub fn delete_back(&mut self) {
        if self.selection_start.is_some() {
            self.delete_selection();
            return;
        }
        if self.cursor_pos > 0 {
            self.cursor_pos -= 1;
            self.text.remove(self.cursor_pos);
        }
    }

    pub fn delete_forward(&mut self) {
        if self.selection_start.is_some() {
            self.delete_selection();
            return;
        }
        if self.cursor_pos < self.text.len() {
            self.text.remove(self.cursor_pos);
        }
    }

    pub fn move_left(&mut self) {
        self.selection_start = None;
        if self.cursor_pos > 0 {
            self.cursor_pos -= 1;
        }
    }

    pub fn move_right(&mut self) {
        self.selection_start = None;
        if self.cursor_pos < self.text.len() {
            self.cursor_pos += 1;
        }
    }

    pub fn select_all(&mut self) {
        self.selection_start = Some(0);
        self.cursor_pos = self.text.len();
    }

    pub fn selected_text(&self) -> &str {
        if let Some(start) = self.selection_start {
            let s = start.min(self.cursor_pos);
            let e = start.max(self.cursor_pos);
            &self.text[s..e]
        } else {
            ""
        }
    }

    fn delete_selection(&mut self) {
        if let Some(start) = self.selection_start.take() {
            let s = start.min(self.cursor_pos);
            let e = start.max(self.cursor_pos);
            self.text.drain(s..e);
            self.cursor_pos = s;
        }
    }

    pub fn home(&mut self) {
        self.cursor_pos = 0;
        self.selection_start = None;
    }
    pub fn end(&mut self) {
        self.cursor_pos = self.text.len();
        self.selection_start = None;
    }
}

impl Widget2 for LayoutTextInput {
    fn intrinsic_size(&self) -> (f32, f32) {
        (160.0, 24.0)
    }

    fn layout_style(&self) -> LayoutStyle {
        LayoutStyle {
            flex_grow: 1.0,
            min_width: Dimension::Points(80.0),
            ..Default::default()
        }
    }

    fn render_at(&self, canvas: &mut Canvas, x: f32, y: f32, w: f32, h: f32) {
        let ix = x as usize;
        let iy = y as usize;
        let iw = w as usize;
        let ih = h as usize;

        let bg = if self.focused {
            0xFF_2A_2A_3A
        } else {
            0xFF_1E_1E_2E
        };
        canvas.fill_rect(ix, iy, iw, ih, bg);
        canvas.draw_rect_outline(ix, iy, iw, ih, theme::BORDER);

        let text_x = ix + 4;
        let text_y = iy + ih.saturating_sub(10) / 2;

        // Selection highlight
        if let Some(sel_start) = self.selection_start {
            let s = sel_start.min(self.cursor_pos);
            let e = sel_start.max(self.cursor_pos);
            let sel_x = text_x + s * 8;
            let sel_w = (e - s) * 8;
            canvas.fill_rect(sel_x, iy + 2, sel_w, ih - 4, 0xFF_33_55_88);
        }

        if self.text.is_empty() && !self.focused {
            canvas.draw_text(text_x, text_y, &self.placeholder, 0xFF_66_66_88, None);
        } else {
            canvas.draw_text(text_x, text_y, &self.text, theme::TEXT_FG, None);
        }

        if self.focused {
            let cursor_x = text_x + self.cursor_pos * 8;
            for dy in 0..(ih.saturating_sub(4)) {
                canvas.draw_pixel(cursor_x, iy + 2 + dy, theme::BORDER);
            }
        }
    }

    fn on_event(&mut self, event: &Event, x: f32, y: f32, w: f32, h: f32) -> bool {
        match event {
            Event::MouseClick { x: mx, y: my } => {
                let in_bounds = *mx as f32 >= x
                    && (*mx as f32) < x + w
                    && *my as f32 >= y
                    && (*my as f32) < y + h;
                self.focused = in_bounds;
                if in_bounds {
                    let rel_x = (*mx as f32 - x - 4.0).max(0.0);
                    self.cursor_pos = ((rel_x / 8.0) as usize).min(self.text.len());
                    self.selection_start = None;
                    return true;
                }
            }
            Event::KeyPress(ch) => {
                if self.focused {
                    match *ch {
                        8 => self.delete_back(),
                        127 => self.delete_forward(),
                        0 => {}
                        c => self.insert_char(c as char),
                    }
                    return true;
                }
            }
            _ => {}
        }
        false
    }
}
