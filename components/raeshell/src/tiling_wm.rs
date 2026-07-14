//! Swappable Window Managers for RaeenOS.
//!
//! Provides three first-class window management strategies behind a
//! unified `WmStrategy` trait:
//!
//! - **TilingWm** — automatic tiling with master/stack layout, configurable
//!   gaps, resize handles, and directional navigation.
//! - **FloatingWm** — traditional drag-and-drop windows with snap zones.
//! - **HybridWm** — floating by default, with on-demand tile groups.
//!
//! The active strategy is per-workspace.  Users switch via hotkeys or the
//! settings panel.  The compositor just asks the active WmStrategy for
//! window rects each frame.

#![allow(dead_code)]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

// ── Geometry ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

impl Rect {
    pub const fn new(x: i32, y: i32, w: u32, h: u32) -> Self {
        Self { x, y, w, h }
    }

    pub fn contains(&self, px: i32, py: i32) -> bool {
        px >= self.x && px < self.x + self.w as i32 && py >= self.y && py < self.y + self.h as i32
    }

    pub fn right(&self) -> i32 {
        self.x + self.w as i32
    }
    pub fn bottom(&self) -> i32 {
        self.y + self.h as i32
    }

    pub fn center_x(&self) -> i32 {
        self.x + self.w as i32 / 2
    }
    pub fn center_y(&self) -> i32 {
        self.y + self.h as i32 / 2
    }
}

// ── Window Manager Mode enum ─────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowManagerMode {
    Float,
    TileHorizontal,
    TileVertical,
    TileGrid,
    Monocle,
    Hybrid,
    MasterStack,
}

// ── Window state ─────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct WmWindow {
    pub id: u64,
    pub title: String,
    pub rect: Rect,
    pub saved_rect: Rect,
    pub minimized: bool,
    pub maximized: bool,
    pub floating: bool,
    pub z_order: u32,
    pub tile_group: Option<u32>,
    pub min_width: u32,
    pub min_height: u32,
    pub fixed_size: bool,
}

impl WmWindow {
    pub fn new(id: u64, title: &str, rect: Rect) -> Self {
        Self {
            id,
            title: String::from(title),
            rect,
            saved_rect: rect,
            minimized: false,
            maximized: false,
            floating: false,
            z_order: 0,
            tile_group: None,
            min_width: 100,
            min_height: 80,
            fixed_size: false,
        }
    }
}

// ── Snap zones for floating WM ───────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapZone {
    None,
    Left,
    Right,
    Top,
    Bottom,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
    Maximize,
}

impl SnapZone {
    pub fn rect_for_zone(self, work: &Rect) -> Rect {
        let hw = work.w / 2;
        let hh = work.h / 2;
        match self {
            SnapZone::None => Rect::new(0, 0, 0, 0),
            SnapZone::Left => Rect::new(work.x, work.y, hw, work.h),
            SnapZone::Right => Rect::new(work.x + hw as i32, work.y, hw, work.h),
            SnapZone::Top => Rect::new(work.x, work.y, work.w, hh),
            SnapZone::Bottom => Rect::new(work.x, work.y + hh as i32, work.w, hh),
            SnapZone::TopLeft => Rect::new(work.x, work.y, hw, hh),
            SnapZone::TopRight => Rect::new(work.x + hw as i32, work.y, hw, hh),
            SnapZone::BottomLeft => Rect::new(work.x, work.y + hh as i32, hw, hh),
            SnapZone::BottomRight => Rect::new(work.x + hw as i32, work.y + hh as i32, hw, hh),
            SnapZone::Maximize => *work,
        }
    }

    pub fn detect(px: i32, py: i32, work: &Rect, margin: i32) -> Self {
        let near_left = px - work.x < margin;
        let near_right = work.right() - px < margin;
        let near_top = py - work.y < margin;
        let near_bottom = work.bottom() - py < margin;

        match (near_left, near_right, near_top, near_bottom) {
            (true, false, true, false) => SnapZone::TopLeft,
            (true, false, false, true) => SnapZone::BottomLeft,
            (false, true, true, false) => SnapZone::TopRight,
            (false, true, false, true) => SnapZone::BottomRight,
            (true, false, false, false) => SnapZone::Left,
            (false, true, false, false) => SnapZone::Right,
            (false, false, true, false) => SnapZone::Top,
            (false, false, false, true) => SnapZone::Bottom,
            _ => SnapZone::None,
        }
    }
}

// ── Resize handle detection ──────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResizeEdge {
    None,
    Top,
    Bottom,
    Left,
    Right,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

impl ResizeEdge {
    pub fn detect(px: i32, py: i32, rect: &Rect, grip: i32) -> Self {
        let near_l = px - rect.x < grip && px >= rect.x;
        let near_r = rect.right() - px <= grip && px < rect.right();
        let near_t = py - rect.y < grip && py >= rect.y;
        let near_b = rect.bottom() - py <= grip && py < rect.bottom();

        match (near_l, near_r, near_t, near_b) {
            (true, false, true, false) => ResizeEdge::TopLeft,
            (true, false, false, true) => ResizeEdge::BottomLeft,
            (false, true, true, false) => ResizeEdge::TopRight,
            (false, true, false, true) => ResizeEdge::BottomRight,
            (true, false, false, false) => ResizeEdge::Left,
            (false, true, false, false) => ResizeEdge::Right,
            (false, false, true, false) => ResizeEdge::Top,
            (false, false, false, true) => ResizeEdge::Bottom,
            _ => ResizeEdge::None,
        }
    }
}

// ── Tiling configuration ─────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub struct TilingConfig {
    pub gap: u32,
    pub outer_gap: u32,
    pub master_ratio: u8,
    pub max_masters: u8,
    pub respect_min_size: bool,
}

impl TilingConfig {
    pub const fn default_config() -> Self {
        Self {
            gap: 8,
            outer_gap: 8,
            master_ratio: 55,
            max_masters: 1,
            respect_min_size: true,
        }
    }
}

// ── Tiling Window Manager ────────────────────────────────────────────────

pub struct TilingWm {
    pub windows: Vec<WmWindow>,
    pub work_area: Rect,
    pub config: TilingConfig,
    pub mode: WindowManagerMode,
    pub focused: Option<u64>,
    pub next_z: u32,
}

impl TilingWm {
    pub fn new(work_area: Rect) -> Self {
        Self {
            windows: Vec::new(),
            work_area,
            config: TilingConfig::default_config(),
            mode: WindowManagerMode::MasterStack,
            focused: None,
            next_z: 1,
        }
    }

    pub fn set_mode(&mut self, mode: WindowManagerMode) {
        self.mode = mode;
        self.retile();
    }

    pub fn add_window(&mut self, id: u64, title: &str, _w: u32, _h: u32) {
        let z = self.next_z;
        self.next_z += 1;
        self.windows.push(WmWindow {
            id,
            title: String::from(title),
            rect: self.work_area,
            saved_rect: self.work_area,
            minimized: false,
            maximized: false,
            floating: false,
            z_order: z,
            tile_group: None,
            min_width: 100,
            min_height: 80,
            fixed_size: false,
        });
        self.focused = Some(id);
        self.retile();
    }

    pub fn remove_window(&mut self, id: u64) {
        self.windows.retain(|w| w.id != id);
        if self.focused == Some(id) {
            self.focused = self.windows.last().map(|w| w.id);
        }
        self.retile();
    }

    pub fn focus(&mut self, id: u64) {
        self.focused = Some(id);
        if let Some(w) = self.windows.iter_mut().find(|w| w.id == id) {
            w.z_order = self.next_z;
            self.next_z += 1;
        }
    }

    pub fn toggle_float(&mut self, id: u64) {
        if let Some(w) = self.windows.iter_mut().find(|w| w.id == id) {
            w.floating = !w.floating;
            if !w.floating {
                w.saved_rect = w.rect;
            } else {
                w.rect = w.saved_rect;
            }
        }
        self.retile();
    }

    pub fn swap_master(&mut self) {
        let focused_id = match self.focused {
            Some(id) => id,
            None => return,
        };
        let focused_idx = self.windows.iter().position(|w| w.id == focused_id);
        if let Some(idx) = focused_idx {
            if idx > 0 {
                self.windows.swap(0, idx);
                self.retile();
            }
        }
    }

    pub fn focus_next(&mut self) {
        if self.windows.is_empty() {
            return;
        }
        let visible: Vec<usize> = self
            .windows
            .iter()
            .enumerate()
            .filter(|(_, w)| !w.minimized)
            .map(|(i, _)| i)
            .collect();
        if visible.is_empty() {
            return;
        }

        let current = self
            .focused
            .and_then(|id| visible.iter().position(|&i| self.windows[i].id == id))
            .unwrap_or(0);
        let next = (current + 1) % visible.len();
        self.focused = Some(self.windows[visible[next]].id);
    }

    pub fn focus_prev(&mut self) {
        if self.windows.is_empty() {
            return;
        }
        let visible: Vec<usize> = self
            .windows
            .iter()
            .enumerate()
            .filter(|(_, w)| !w.minimized)
            .map(|(i, _)| i)
            .collect();
        if visible.is_empty() {
            return;
        }

        let current = self
            .focused
            .and_then(|id| visible.iter().position(|&i| self.windows[i].id == id))
            .unwrap_or(0);
        let prev = if current == 0 {
            visible.len() - 1
        } else {
            current - 1
        };
        self.focused = Some(self.windows[visible[prev]].id);
    }

    pub fn adjust_master_ratio(&mut self, delta: i8) {
        let new = self.config.master_ratio as i16 + delta as i16;
        self.config.master_ratio = (new.clamp(20, 80)) as u8;
        self.retile();
    }

    pub fn set_gap(&mut self, gap: u32) {
        self.config.gap = gap;
        self.retile();
    }

    pub fn retile(&mut self) {
        let tiled: Vec<usize> = self
            .windows
            .iter()
            .enumerate()
            .filter(|(_, w)| !w.minimized && !w.floating)
            .map(|(i, _)| i)
            .collect();

        if tiled.is_empty() {
            return;
        }

        let g = self.config.gap as i32;
        let og = self.config.outer_gap as i32;
        let wa = &self.work_area;

        let inner_x = wa.x + og;
        let inner_y = wa.y + og;
        let inner_w = (wa.w as i32 - 2 * og).max(1);
        let inner_h = (wa.h as i32 - 2 * og).max(1);

        match self.mode {
            WindowManagerMode::MasterStack => {
                self.tile_master_stack(&tiled, inner_x, inner_y, inner_w, inner_h, g);
            }
            WindowManagerMode::TileHorizontal => {
                self.tile_horizontal(&tiled, inner_x, inner_y, inner_w, inner_h, g);
            }
            WindowManagerMode::TileVertical => {
                self.tile_vertical(&tiled, inner_x, inner_y, inner_w, inner_h, g);
            }
            WindowManagerMode::TileGrid => {
                self.tile_grid(&tiled, inner_x, inner_y, inner_w, inner_h, g);
            }
            WindowManagerMode::Monocle => {
                for &idx in &tiled {
                    self.windows[idx].rect =
                        Rect::new(inner_x, inner_y, inner_w as u32, inner_h as u32);
                }
            }
            _ => {}
        }
    }

    fn tile_master_stack(&mut self, tiled: &[usize], ix: i32, iy: i32, iw: i32, ih: i32, g: i32) {
        let n_masters = (self.config.max_masters as usize).min(tiled.len());
        let n_stack = tiled.len() - n_masters;

        if n_stack == 0 {
            let master_h = (ih - g * (n_masters as i32 - 1).max(0)) / n_masters.max(1) as i32;
            for (i, &idx) in tiled.iter().enumerate() {
                self.windows[idx].rect = Rect::new(
                    ix,
                    iy + i as i32 * (master_h + g),
                    iw.max(1) as u32,
                    master_h.max(1) as u32,
                );
            }
            return;
        }

        let master_w = (iw * self.config.master_ratio as i32) / 100;
        let stack_w = iw - master_w - g;

        let master_h = (ih - g * (n_masters as i32 - 1).max(0)) / n_masters.max(1) as i32;
        for i in 0..n_masters {
            let idx = tiled[i];
            self.windows[idx].rect = Rect::new(
                ix,
                iy + i as i32 * (master_h + g),
                master_w.max(1) as u32,
                master_h.max(1) as u32,
            );
        }

        let stack_h = (ih - g * (n_stack as i32 - 1).max(0)) / n_stack.max(1) as i32;
        for i in 0..n_stack {
            let idx = tiled[n_masters + i];
            self.windows[idx].rect = Rect::new(
                ix + master_w + g,
                iy + i as i32 * (stack_h + g),
                stack_w.max(1) as u32,
                stack_h.max(1) as u32,
            );
        }
    }

    fn tile_horizontal(&mut self, tiled: &[usize], ix: i32, iy: i32, iw: i32, ih: i32, g: i32) {
        let n = tiled.len() as i32;
        let w = (iw - g * (n - 1).max(0)) / n.max(1);
        for (i, &idx) in tiled.iter().enumerate() {
            self.windows[idx].rect = Rect::new(
                ix + i as i32 * (w + g),
                iy,
                w.max(1) as u32,
                ih.max(1) as u32,
            );
        }
    }

    fn tile_vertical(&mut self, tiled: &[usize], ix: i32, iy: i32, iw: i32, ih: i32, g: i32) {
        let n = tiled.len() as i32;
        let h = (ih - g * (n - 1).max(0)) / n.max(1);
        for (i, &idx) in tiled.iter().enumerate() {
            self.windows[idx].rect = Rect::new(
                ix,
                iy + i as i32 * (h + g),
                iw.max(1) as u32,
                h.max(1) as u32,
            );
        }
    }

    fn tile_grid(&mut self, tiled: &[usize], ix: i32, iy: i32, iw: i32, ih: i32, g: i32) {
        let n = tiled.len();
        let cols = isqrt_ceil(n as u32) as usize;
        let rows = (n + cols - 1) / cols.max(1);
        let cw = (iw - g * (cols as i32 - 1).max(0)) / cols.max(1) as i32;
        let ch = (ih - g * (rows as i32 - 1).max(0)) / rows.max(1) as i32;
        for (i, &idx) in tiled.iter().enumerate() {
            let col = i % cols;
            let row = i / cols;
            self.windows[idx].rect = Rect::new(
                ix + col as i32 * (cw + g),
                iy + row as i32 * (ch + g),
                cw.max(1) as u32,
                ch.max(1) as u32,
            );
        }
    }

    pub fn visible_windows(&self) -> Vec<&WmWindow> {
        let mut vis: Vec<&WmWindow> = self.windows.iter().filter(|w| !w.minimized).collect();
        vis.sort_by_key(|w| w.z_order);
        vis
    }
}

// ── Floating Window Manager ──────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DragState {
    None,
    Moving {
        win_id: u64,
        start_x: i32,
        start_y: i32,
        win_start_x: i32,
        win_start_y: i32,
    },
    Resizing {
        win_id: u64,
        edge: ResizeEdge,
        start_x: i32,
        start_y: i32,
        start_rect: Rect,
    },
}

pub struct FloatingWm {
    pub windows: Vec<WmWindow>,
    pub work_area: Rect,
    pub focused: Option<u64>,
    pub next_z: u32,
    pub drag: DragState,
    pub snap_margin: i32,
    pub cascade_offset: i32,
}

impl FloatingWm {
    pub fn new(work_area: Rect) -> Self {
        Self {
            windows: Vec::new(),
            work_area,
            focused: None,
            next_z: 1,
            drag: DragState::None,
            snap_margin: 20,
            cascade_offset: 30,
        }
    }

    pub fn add_window(&mut self, id: u64, title: &str, w: u32, h: u32) {
        let offset = (self.windows.len() as i32 % 10) * self.cascade_offset;
        let x = self.work_area.x + 60 + offset;
        let y = self.work_area.y + 60 + offset;
        let z = self.next_z;
        self.next_z += 1;

        let rect = Rect::new(x, y, w, h);
        self.windows.push(WmWindow::new(id, title, rect));
        if let Some(w) = self.windows.last_mut() {
            w.z_order = z;
        }
        self.focused = Some(id);
    }

    pub fn remove_window(&mut self, id: u64) {
        self.windows.retain(|w| w.id != id);
        if self.focused == Some(id) {
            self.focused = self
                .windows
                .iter()
                .filter(|w| !w.minimized)
                .max_by_key(|w| w.z_order)
                .map(|w| w.id);
        }
    }

    pub fn focus(&mut self, id: u64) {
        self.focused = Some(id);
        if let Some(w) = self.windows.iter_mut().find(|w| w.id == id) {
            w.z_order = self.next_z;
            self.next_z += 1;
        }
    }

    pub fn begin_move(&mut self, id: u64, mouse_x: i32, mouse_y: i32) {
        if let Some(w) = self.windows.iter().find(|w| w.id == id) {
            self.drag = DragState::Moving {
                win_id: id,
                start_x: mouse_x,
                start_y: mouse_y,
                win_start_x: w.rect.x,
                win_start_y: w.rect.y,
            };
            self.focus(id);
        }
    }

    pub fn begin_resize(&mut self, id: u64, edge: ResizeEdge, mouse_x: i32, mouse_y: i32) {
        if let Some(w) = self.windows.iter().find(|w| w.id == id) {
            self.drag = DragState::Resizing {
                win_id: id,
                edge,
                start_x: mouse_x,
                start_y: mouse_y,
                start_rect: w.rect,
            };
            self.focus(id);
        }
    }

    pub fn on_mouse_move(&mut self, mx: i32, my: i32) {
        match self.drag {
            DragState::Moving {
                win_id,
                start_x,
                start_y,
                win_start_x,
                win_start_y,
            } => {
                let dx = mx - start_x;
                let dy = my - start_y;
                if let Some(w) = self.windows.iter_mut().find(|w| w.id == win_id) {
                    w.rect.x = win_start_x + dx;
                    w.rect.y = win_start_y + dy;
                }
            }
            DragState::Resizing {
                win_id,
                edge,
                start_x,
                start_y,
                start_rect,
            } => {
                let dx = mx - start_x;
                let dy = my - start_y;
                if let Some(w) = self.windows.iter_mut().find(|w| w.id == win_id) {
                    apply_resize(
                        &mut w.rect,
                        &start_rect,
                        edge,
                        dx,
                        dy,
                        w.min_width,
                        w.min_height,
                    );
                }
            }
            DragState::None => {}
        }
    }

    pub fn end_drag(&mut self) -> SnapZone {
        let snap = match self.drag {
            DragState::Moving { win_id, .. } => {
                if let Some(w) = self.windows.iter().find(|w| w.id == win_id) {
                    let cx = w.rect.center_x();
                    let cy = w.rect.center_y();
                    let zone = SnapZone::detect(cx, cy, &self.work_area, self.snap_margin);
                    if zone != SnapZone::None {
                        if let Some(w) = self.windows.iter_mut().find(|w| w.id == win_id) {
                            w.saved_rect = w.rect;
                            w.rect = zone.rect_for_zone(&self.work_area);
                        }
                    }
                    zone
                } else {
                    SnapZone::None
                }
            }
            _ => SnapZone::None,
        };
        self.drag = DragState::None;
        snap
    }

    pub fn minimize(&mut self, id: u64) {
        if let Some(w) = self.windows.iter_mut().find(|w| w.id == id) {
            w.minimized = true;
        }
        if self.focused == Some(id) {
            self.focused = self
                .windows
                .iter()
                .filter(|w| !w.minimized)
                .max_by_key(|w| w.z_order)
                .map(|w| w.id);
        }
    }

    pub fn restore(&mut self, id: u64) {
        if let Some(w) = self.windows.iter_mut().find(|w| w.id == id) {
            w.minimized = false;
            w.z_order = self.next_z;
            self.next_z += 1;
        }
        self.focused = Some(id);
    }

    pub fn maximize(&mut self, id: u64) {
        if let Some(w) = self.windows.iter_mut().find(|w| w.id == id) {
            if !w.maximized {
                w.saved_rect = w.rect;
                w.rect = self.work_area;
                w.maximized = true;
            }
        }
    }

    pub fn unmaximize(&mut self, id: u64) {
        if let Some(w) = self.windows.iter_mut().find(|w| w.id == id) {
            if w.maximized {
                w.rect = w.saved_rect;
                w.maximized = false;
            }
        }
    }

    pub fn toggle_maximize(&mut self, id: u64) {
        let is_max = self
            .windows
            .iter()
            .find(|w| w.id == id)
            .map(|w| w.maximized)
            .unwrap_or(false);
        if is_max {
            self.unmaximize(id);
        } else {
            self.maximize(id);
        }
    }

    pub fn window_at(&self, px: i32, py: i32) -> Option<u64> {
        self.visible_windows()
            .iter()
            .rev()
            .find(|w| w.rect.contains(px, py))
            .map(|w| w.id)
    }

    pub fn visible_windows(&self) -> Vec<&WmWindow> {
        let mut vis: Vec<&WmWindow> = self.windows.iter().filter(|w| !w.minimized).collect();
        vis.sort_by_key(|w| w.z_order);
        vis
    }

    pub fn cascade_all(&mut self) {
        for (i, w) in self.windows.iter_mut().enumerate() {
            if !w.minimized {
                let offset = (i as i32) * self.cascade_offset;
                w.rect.x = self.work_area.x + 40 + offset;
                w.rect.y = self.work_area.y + 40 + offset;
                w.maximized = false;
            }
        }
    }

    pub fn tile_side_by_side(&mut self) {
        let visible: Vec<usize> = self
            .windows
            .iter()
            .enumerate()
            .filter(|(_, w)| !w.minimized)
            .map(|(i, _)| i)
            .collect();
        if visible.is_empty() {
            return;
        }

        let n = visible.len() as i32;
        let w = self.work_area.w as i32 / n;
        for (i, &idx) in visible.iter().enumerate() {
            self.windows[idx].rect = Rect::new(
                self.work_area.x + i as i32 * w,
                self.work_area.y,
                w as u32,
                self.work_area.h,
            );
            self.windows[idx].maximized = false;
        }
    }
}

// ── Hybrid Window Manager ────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TileGroup {
    pub id: u32,
    pub rect: Rect,
    pub mode: WindowManagerMode,
}

pub struct HybridWm {
    pub floating: FloatingWm,
    pub tile_groups: Vec<TileGroup>,
    pub next_group: u32,
    pub gap: u32,
}

impl HybridWm {
    pub fn new(work_area: Rect) -> Self {
        Self {
            floating: FloatingWm::new(work_area),
            tile_groups: Vec::new(),
            next_group: 1,
            gap: 8,
        }
    }

    pub fn add_window(&mut self, id: u64, title: &str, w: u32, h: u32) {
        self.floating.add_window(id, title, w, h);
    }

    pub fn remove_window(&mut self, id: u64) {
        self.floating.remove_window(id);
        self.retile_all_groups();
    }

    pub fn create_tile_group(&mut self, rect: Rect, mode: WindowManagerMode) -> u32 {
        let gid = self.next_group;
        self.next_group += 1;
        self.tile_groups.push(TileGroup {
            id: gid,
            rect,
            mode,
        });
        gid
    }

    pub fn assign_to_group(&mut self, win_id: u64, group_id: u32) {
        if let Some(w) = self.floating.windows.iter_mut().find(|w| w.id == win_id) {
            w.tile_group = Some(group_id);
            w.floating = false;
        }
        self.retile_group(group_id);
    }

    pub fn unassign_from_group(&mut self, win_id: u64) {
        let old_group = self
            .floating
            .windows
            .iter()
            .find(|w| w.id == win_id)
            .and_then(|w| w.tile_group);

        if let Some(w) = self.floating.windows.iter_mut().find(|w| w.id == win_id) {
            w.tile_group = None;
            w.floating = true;
            w.rect = w.saved_rect;
        }
        if let Some(gid) = old_group {
            self.retile_group(gid);
        }
    }

    pub fn remove_tile_group(&mut self, group_id: u32) {
        for w in &mut self.floating.windows {
            if w.tile_group == Some(group_id) {
                w.tile_group = None;
                w.floating = true;
                w.rect = w.saved_rect;
            }
        }
        self.tile_groups.retain(|g| g.id != group_id);
    }

    pub fn quick_tile_focused(&mut self, zone: SnapZone) {
        let focused_id = match self.floating.focused {
            Some(id) => id,
            None => return,
        };
        let rect = zone.rect_for_zone(&self.floating.work_area);
        if rect.w == 0 {
            return;
        }

        let gid = self.create_tile_group(rect, WindowManagerMode::TileVertical);
        self.assign_to_group(focused_id, gid);
    }

    fn retile_group(&mut self, group_id: u32) {
        let group = match self.tile_groups.iter().find(|g| g.id == group_id) {
            Some(g) => *g,
            None => return,
        };

        let members: Vec<usize> = self
            .floating
            .windows
            .iter()
            .enumerate()
            .filter(|(_, w)| w.tile_group == Some(group_id) && !w.minimized)
            .map(|(i, _)| i)
            .collect();

        if members.is_empty() {
            return;
        }

        let g = self.gap as i32;
        let n = members.len() as i32;

        match group.mode {
            WindowManagerMode::TileHorizontal => {
                let w = (group.rect.w as i32 - g * (n - 1).max(0)) / n.max(1);
                for (i, &idx) in members.iter().enumerate() {
                    self.floating.windows[idx].rect = Rect::new(
                        group.rect.x + i as i32 * (w + g),
                        group.rect.y,
                        w.max(1) as u32,
                        group.rect.h,
                    );
                }
            }
            WindowManagerMode::TileVertical => {
                let h = (group.rect.h as i32 - g * (n - 1).max(0)) / n.max(1);
                for (i, &idx) in members.iter().enumerate() {
                    self.floating.windows[idx].rect = Rect::new(
                        group.rect.x,
                        group.rect.y + i as i32 * (h + g),
                        group.rect.w,
                        h.max(1) as u32,
                    );
                }
            }
            _ => {
                for &idx in &members {
                    self.floating.windows[idx].rect = group.rect;
                }
            }
        }
    }

    fn retile_all_groups(&mut self) {
        let group_ids: Vec<u32> = self.tile_groups.iter().map(|g| g.id).collect();
        for gid in group_ids {
            self.retile_group(gid);
        }
    }

    pub fn visible_windows(&self) -> Vec<&WmWindow> {
        self.floating.visible_windows()
    }
}

// ── Per-workspace WM mode ────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct WorkspaceWmConfig {
    pub workspace_id: u32,
    pub workspace_name: String,
    pub mode: WindowManagerMode,
    pub tiling_config: TilingConfig,
}

impl WorkspaceWmConfig {
    pub fn new(id: u32, name: &str, mode: WindowManagerMode) -> Self {
        Self {
            workspace_id: id,
            workspace_name: String::from(name),
            mode,
            tiling_config: TilingConfig::default_config(),
        }
    }
}

pub struct WmModeManager {
    pub workspace_configs: Vec<WorkspaceWmConfig>,
    pub default_mode: WindowManagerMode,
}

impl WmModeManager {
    pub fn new() -> Self {
        Self {
            workspace_configs: Vec::new(),
            default_mode: WindowManagerMode::Float,
        }
    }

    pub fn set_workspace_mode(&mut self, workspace_id: u32, mode: WindowManagerMode) {
        if let Some(cfg) = self
            .workspace_configs
            .iter_mut()
            .find(|c| c.workspace_id == workspace_id)
        {
            cfg.mode = mode;
        }
    }

    pub fn mode_for_workspace(&self, workspace_id: u32) -> WindowManagerMode {
        self.workspace_configs
            .iter()
            .find(|c| c.workspace_id == workspace_id)
            .map(|c| c.mode)
            .unwrap_or(self.default_mode)
    }

    pub fn add_workspace(&mut self, id: u32, name: &str) {
        self.workspace_configs
            .push(WorkspaceWmConfig::new(id, name, self.default_mode));
    }

    pub fn cycle_mode(&mut self, workspace_id: u32) {
        let current = self.mode_for_workspace(workspace_id);
        let next = match current {
            WindowManagerMode::Float => WindowManagerMode::TileHorizontal,
            WindowManagerMode::TileHorizontal => WindowManagerMode::TileVertical,
            WindowManagerMode::TileVertical => WindowManagerMode::TileGrid,
            WindowManagerMode::TileGrid => WindowManagerMode::MasterStack,
            WindowManagerMode::MasterStack => WindowManagerMode::Monocle,
            WindowManagerMode::Monocle => WindowManagerMode::Hybrid,
            WindowManagerMode::Hybrid => WindowManagerMode::Float,
        };
        self.set_workspace_mode(workspace_id, next);
    }
}

// ── Hotkey binding table ─────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WmAction {
    CycleMode,
    SetMode(WindowManagerMode),
    FocusNext,
    FocusPrev,
    SwapMaster,
    ToggleFloat,
    IncMasterRatio,
    DecMasterRatio,
    IncGap,
    DecGap,
    Maximize,
    Minimize,
    Close,
    CascadeAll,
    TileSideBySide,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hotkey {
    pub modifiers: u8,
    pub key: u8,
}

pub const MOD_SUPER: u8 = 0x01;
pub const MOD_SHIFT: u8 = 0x02;
pub const MOD_CTRL: u8 = 0x04;
pub const MOD_ALT: u8 = 0x08;

impl Hotkey {
    pub const fn new(modifiers: u8, key: u8) -> Self {
        Self { modifiers, key }
    }
}

pub struct HotkeyBinding {
    pub hotkey: Hotkey,
    pub action: WmAction,
}

pub struct WmHotkeyTable {
    pub bindings: Vec<HotkeyBinding>,
}

impl WmHotkeyTable {
    pub fn new() -> Self {
        Self {
            bindings: Vec::new(),
        }
    }

    pub fn bind(&mut self, hotkey: Hotkey, action: WmAction) {
        self.bindings.push(HotkeyBinding { hotkey, action });
    }

    pub fn lookup(&self, modifiers: u8, key: u8) -> Option<WmAction> {
        self.bindings
            .iter()
            .find(|b| b.hotkey.modifiers == modifiers && b.hotkey.key == key)
            .map(|b| b.action)
    }

    pub fn default_bindings() -> Self {
        let mut table = Self::new();
        table.bind(Hotkey::new(MOD_SUPER, b'M'), WmAction::CycleMode);
        table.bind(Hotkey::new(MOD_SUPER, b'J'), WmAction::FocusNext);
        table.bind(Hotkey::new(MOD_SUPER, b'K'), WmAction::FocusPrev);
        table.bind(
            Hotkey::new(MOD_SUPER | MOD_SHIFT, b'J'),
            WmAction::SwapMaster,
        );
        table.bind(Hotkey::new(MOD_SUPER, b'F'), WmAction::ToggleFloat);
        table.bind(Hotkey::new(MOD_SUPER, b'L'), WmAction::IncMasterRatio);
        table.bind(Hotkey::new(MOD_SUPER, b'H'), WmAction::DecMasterRatio);
        table.bind(Hotkey::new(MOD_SUPER | MOD_SHIFT, b'='), WmAction::IncGap);
        table.bind(Hotkey::new(MOD_SUPER | MOD_SHIFT, b'-'), WmAction::DecGap);
        table.bind(Hotkey::new(MOD_SUPER, b'X'), WmAction::Maximize);
        table.bind(Hotkey::new(MOD_SUPER, b'N'), WmAction::Minimize);
        table.bind(Hotkey::new(MOD_SUPER | MOD_SHIFT, b'C'), WmAction::Close);
        table
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────

fn apply_resize(
    rect: &mut Rect,
    start: &Rect,
    edge: ResizeEdge,
    dx: i32,
    dy: i32,
    min_w: u32,
    min_h: u32,
) {
    match edge {
        ResizeEdge::Right => {
            rect.w = ((start.w as i32 + dx).max(min_w as i32)) as u32;
        }
        ResizeEdge::Bottom => {
            rect.h = ((start.h as i32 + dy).max(min_h as i32)) as u32;
        }
        ResizeEdge::Left => {
            let new_w = (start.w as i32 - dx).max(min_w as i32);
            rect.x = start.x + start.w as i32 - new_w;
            rect.w = new_w as u32;
        }
        ResizeEdge::Top => {
            let new_h = (start.h as i32 - dy).max(min_h as i32);
            rect.y = start.y + start.h as i32 - new_h;
            rect.h = new_h as u32;
        }
        ResizeEdge::TopLeft => {
            let new_w = (start.w as i32 - dx).max(min_w as i32);
            let new_h = (start.h as i32 - dy).max(min_h as i32);
            rect.x = start.x + start.w as i32 - new_w;
            rect.y = start.y + start.h as i32 - new_h;
            rect.w = new_w as u32;
            rect.h = new_h as u32;
        }
        ResizeEdge::TopRight => {
            let new_h = (start.h as i32 - dy).max(min_h as i32);
            rect.y = start.y + start.h as i32 - new_h;
            rect.w = ((start.w as i32 + dx).max(min_w as i32)) as u32;
            rect.h = new_h as u32;
        }
        ResizeEdge::BottomLeft => {
            let new_w = (start.w as i32 - dx).max(min_w as i32);
            rect.x = start.x + start.w as i32 - new_w;
            rect.w = new_w as u32;
            rect.h = ((start.h as i32 + dy).max(min_h as i32)) as u32;
        }
        ResizeEdge::BottomRight => {
            rect.w = ((start.w as i32 + dx).max(min_w as i32)) as u32;
            rect.h = ((start.h as i32 + dy).max(min_h as i32)) as u32;
        }
        ResizeEdge::None => {}
    }
}

fn isqrt_ceil(n: u32) -> u32 {
    if n == 0 {
        return 0;
    }
    let mut x = 1u32;
    while x * x < n {
        x += 1;
    }
    x
}
