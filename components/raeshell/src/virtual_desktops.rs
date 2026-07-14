#![no_std]

extern crate alloc;

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

// ── Desktop errors ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DesktopError {
    InvalidIndex,
    MaxDesktopsReached,
    CannotRemoveLast,
    WindowNotFound,
    DesktopNotFound,
}

// ── Wallpaper ────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum WallpaperSource {
    SolidColor(u32),
    Gradient(u32, u32, GradientDirection),
    Image(String),
    Slideshow(Vec<String>),
    Live(String),
    Dynamic(DynamicWallpaper),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WallpaperMode {
    Fill,
    Fit,
    Stretch,
    Tile,
    Center,
    Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GradientDirection {
    Horizontal,
    Vertical,
    Diagonal,
    Radial,
}

#[derive(Debug, Clone)]
pub struct DynamicWallpaper {
    pub time_entries: Vec<(u32, String)>,
    pub transition_duration_s: u32,
}

#[derive(Debug, Clone)]
pub struct WallpaperConfig {
    pub source: WallpaperSource,
    pub mode: WallpaperMode,
    pub color: u32,
    pub slideshow_interval_s: Option<u32>,
    pub blur_enabled: bool,
    pub blur_radius: u32,
    pub tint_color: Option<u32>,
    pub tint_opacity: f32,
}

impl WallpaperConfig {
    pub fn solid(color: u32) -> Self {
        Self {
            source: WallpaperSource::SolidColor(color),
            mode: WallpaperMode::Fill,
            color,
            slideshow_interval_s: None,
            blur_enabled: false,
            blur_radius: 0,
            tint_color: None,
            tint_opacity: 0.0,
        }
    }
}

// ── Window layout & desktop window ───────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowLayout {
    Free,
    TileHorizontal,
    TileVertical,
    Grid,
    Cascade,
    Monocle,
}

#[derive(Debug, Clone)]
pub struct DesktopWindow {
    pub surface_id: u64,
    pub title: String,
    pub app_id: String,
    pub rect: (i32, i32, u32, u32),
    pub minimized: bool,
    pub maximized: bool,
    pub pinned: bool,
    pub z_order: u32,
    pub thumbnail: Option<Vec<u32>>,
    pub icon: Option<String>,
}

// ── Virtual desktop ──────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct VirtualDesktop {
    pub id: u64,
    pub name: String,
    pub windows: Vec<DesktopWindow>,
    pub wallpaper: WallpaperConfig,
    pub icon: Option<String>,
    pub color: u32,
    pub pinned_apps: Vec<String>,
    pub layout: WindowLayout,
    pub created_at: u64,
}

impl VirtualDesktop {
    fn new(id: u64, name: &str, color: u32) -> Self {
        Self {
            id,
            name: String::from(name),
            windows: Vec::new(),
            wallpaper: WallpaperConfig::solid(0xFF_14_16_22),
            icon: None,
            color,
            pinned_apps: Vec::new(),
            layout: WindowLayout::Free,
            created_at: 0,
        }
    }

    fn window_index(&self, window_id: u64) -> Option<usize> {
        self.windows.iter().position(|w| w.surface_id == window_id)
    }
}

// ── Transitions & easing ─────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TransitionType {
    SlideHorizontal,
    SlideVertical,
    Fade,
    Zoom,
    Cube,
    Flip,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EasingFunction {
    Linear,
    EaseIn,
    EaseOut,
    EaseInOut,
    CubicBezier(f32, f32, f32, f32),
    Spring(f32, f32),
}

pub struct DesktopTransition {
    pub transition_type: TransitionType,
    pub progress: f32,
    pub duration_ms: u32,
    pub from_desktop: usize,
    pub to_desktop: usize,
    pub active: bool,
    pub easing: EasingFunction,
}

impl DesktopTransition {
    fn new() -> Self {
        Self {
            transition_type: TransitionType::SlideHorizontal,
            progress: 0.0,
            duration_ms: 300,
            from_desktop: 0,
            to_desktop: 0,
            active: false,
            easing: EasingFunction::EaseInOut,
        }
    }

    fn start(&mut self, from: usize, to: usize, transition: TransitionType, duration_ms: u32) {
        self.from_desktop = from;
        self.to_desktop = to;
        self.transition_type = transition;
        self.duration_ms = duration_ms;
        self.progress = 0.0;
        self.active = true;
    }

    fn reset(&mut self) {
        self.active = false;
        self.progress = 0.0;
    }
}

// ── Layout ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutType {
    Linear,
    Grid,
}

pub struct DesktopLayout {
    pub layout_type: LayoutType,
    pub rows: u32,
    pub columns: u32,
    pub wrap_around: bool,
}

impl DesktopLayout {
    fn new() -> Self {
        Self {
            layout_type: LayoutType::Linear,
            rows: 1,
            columns: 4,
            wrap_around: true,
        }
    }

    fn index_left(&self, current: usize, count: usize) -> usize {
        if count == 0 {
            return 0;
        }
        match self.layout_type {
            LayoutType::Linear => {
                if current == 0 {
                    if self.wrap_around {
                        count - 1
                    } else {
                        0
                    }
                } else {
                    current - 1
                }
            }
            LayoutType::Grid => {
                let col = current % self.columns as usize;
                if col == 0 {
                    if self.wrap_around {
                        let row_start = (current / self.columns as usize) * self.columns as usize;
                        (row_start + self.columns as usize - 1).min(count - 1)
                    } else {
                        current
                    }
                } else {
                    current - 1
                }
            }
        }
    }

    fn index_right(&self, current: usize, count: usize) -> usize {
        if count == 0 {
            return 0;
        }
        match self.layout_type {
            LayoutType::Linear => {
                if current + 1 >= count {
                    if self.wrap_around {
                        0
                    } else {
                        current
                    }
                } else {
                    current + 1
                }
            }
            LayoutType::Grid => {
                let col = current % self.columns as usize;
                if col + 1 >= self.columns as usize || current + 1 >= count {
                    if self.wrap_around {
                        let row_start = (current / self.columns as usize) * self.columns as usize;
                        row_start
                    } else {
                        current
                    }
                } else {
                    current + 1
                }
            }
        }
    }

    fn index_up(&self, current: usize, count: usize) -> usize {
        if count == 0 {
            return 0;
        }
        match self.layout_type {
            LayoutType::Linear => current,
            LayoutType::Grid => {
                if current < self.columns as usize {
                    if self.wrap_around {
                        let last_row = (count - 1) / self.columns as usize;
                        let target =
                            last_row * self.columns as usize + (current % self.columns as usize);
                        target.min(count - 1)
                    } else {
                        current
                    }
                } else {
                    current - self.columns as usize
                }
            }
        }
    }

    fn index_down(&self, current: usize, count: usize) -> usize {
        if count == 0 {
            return 0;
        }
        match self.layout_type {
            LayoutType::Linear => current,
            LayoutType::Grid => {
                let next = current + self.columns as usize;
                if next >= count {
                    if self.wrap_around {
                        current % self.columns as usize
                    } else {
                        current
                    }
                } else {
                    next
                }
            }
        }
    }
}

// ── Hot corners & settings ───────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum HotCornerAction {
    ShowOverview,
    ShowDesktop,
    LaunchApp(String),
    ShowNotifications,
    LockScreen,
    None,
}

pub struct DesktopSettings {
    pub max_desktops: usize,
    pub dynamic_desktops: bool,
    pub show_desktop_number: bool,
    pub show_desktop_name: bool,
    pub close_empty: bool,
    pub transition_type: TransitionType,
    pub transition_duration_ms: u32,
    pub overview_scale: f32,
    pub gesture_enabled: bool,
    pub hotcorners: [Option<HotCornerAction>; 4],
}

impl DesktopSettings {
    fn new() -> Self {
        Self {
            max_desktops: 16,
            dynamic_desktops: true,
            show_desktop_number: true,
            show_desktop_name: true,
            close_empty: false,
            transition_type: TransitionType::SlideHorizontal,
            transition_duration_ms: 300,
            overview_scale: 0.2,
            gesture_enabled: true,
            hotcorners: [None, None, None, None],
        }
    }
}

// ── Drag state ───────────────────────────────────────────────────────────

pub struct DragState {
    pub window_id: u64,
    pub from_desktop: usize,
    pub to_desktop: Option<usize>,
    pub position: (i32, i32),
}

// ── Gestures ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DesktopGesture {
    SwipeLeft,
    SwipeRight,
    SwipeUp,
    SwipeDown,
    PinchIn,
    PinchOut,
    ThreeFingerSwipeLeft,
    ThreeFingerSwipeRight,
}

// ── Rendering constants ──────────────────────────────────────────────────

const OVERVIEW_BG: u32 = 0xFF_0A_0E_1A;
const OVERVIEW_DESKTOP_BG: u32 = 0xFF_1A_1E_2E;
const OVERVIEW_ACTIVE_BORDER: u32 = 0xFF_4E_9C_FF;
const OVERVIEW_INACTIVE_BORDER: u32 = 0xFF_44_44_55;
const OVERVIEW_LABEL_FG: u32 = 0xFF_C0_C0_D0;
const OVERVIEW_DIM_FG: u32 = 0xFF_70_70_80;
const INDICATOR_BG: u32 = 0xFF_0A_0E_1A;
const INDICATOR_ACTIVE: u32 = 0xFF_4E_9C_FF;
const INDICATOR_INACTIVE: u32 = 0xFF_44_44_55;
const GLYPH_W: usize = 8;
const GLYPH_H: usize = 8;

const DEFAULT_DESKTOP_COLORS: [u32; 8] = [
    0xFF_4E_9C_FF,
    0xFF_FF_6B_6B,
    0xFF_51_CF_66,
    0xFF_FF_D4_3B,
    0xFF_CC_5D_E8,
    0xFF_20_C9_97,
    0xFF_FF_92_2B,
    0xFF_74_5C_FF,
];

// ── Virtual Desktop Manager ──────────────────────────────────────────────

pub struct VirtualDesktopManager {
    desktops: Vec<VirtualDesktop>,
    active: usize,
    history: Vec<usize>,
    animation: DesktopTransition,
    layout: DesktopLayout,
    settings: DesktopSettings,
    overview_active: bool,
    drag_state: Option<DragState>,
    next_id: u64,
}

impl VirtualDesktopManager {
    pub fn new() -> Self {
        let mut mgr = Self {
            desktops: Vec::new(),
            active: 0,
            history: Vec::new(),
            animation: DesktopTransition::new(),
            layout: DesktopLayout::new(),
            settings: DesktopSettings::new(),
            overview_active: false,
            drag_state: None,
            next_id: 1,
        };
        mgr.desktops.push(VirtualDesktop::new(
            0,
            "Desktop 1",
            DEFAULT_DESKTOP_COLORS[0],
        ));
        mgr
    }

    pub fn create_desktop(&mut self, name: &str) -> u64 {
        if self.desktops.len() >= self.settings.max_desktops {
            return 0;
        }
        let id = self.next_id;
        self.next_id += 1;
        let color_idx = self.desktops.len() % DEFAULT_DESKTOP_COLORS.len();
        let desktop = VirtualDesktop::new(id, name, DEFAULT_DESKTOP_COLORS[color_idx]);
        self.desktops.push(desktop);
        id
    }

    pub fn remove_desktop(&mut self, index: usize) -> Result<(), DesktopError> {
        if self.desktops.len() <= 1 {
            return Err(DesktopError::CannotRemoveLast);
        }
        if index >= self.desktops.len() {
            return Err(DesktopError::InvalidIndex);
        }

        let removed = self.desktops.remove(index);

        // Move orphaned windows to the neighboring desktop
        let target = if index >= self.desktops.len() {
            self.desktops.len() - 1
        } else {
            index
        };
        for win in removed.windows {
            if !win.pinned {
                self.desktops[target].windows.push(win);
            }
        }

        if self.active >= self.desktops.len() {
            self.active = self.desktops.len() - 1;
        } else if self.active > index {
            self.active -= 1;
        }

        self.history.retain(|&i| i != index);
        for idx in self.history.iter_mut() {
            if *idx > index {
                *idx -= 1;
            }
        }

        Ok(())
    }

    pub fn switch_to(&mut self, index: usize) -> Result<(), DesktopError> {
        if index >= self.desktops.len() {
            return Err(DesktopError::InvalidIndex);
        }
        if index == self.active && !self.animation.active {
            return Ok(());
        }

        self.history.push(self.active);
        if self.history.len() > 32 {
            self.history.remove(0);
        }

        self.animation.start(
            self.active,
            index,
            self.settings.transition_type,
            self.settings.transition_duration_ms,
        );
        self.active = index;
        Ok(())
    }

    pub fn switch_left(&mut self) {
        let target = self.layout.index_left(self.active, self.desktops.len());
        let _ = self.switch_to(target);
    }

    pub fn switch_right(&mut self) {
        let target = self.layout.index_right(self.active, self.desktops.len());
        let _ = self.switch_to(target);
    }

    pub fn switch_up(&mut self) {
        let target = self.layout.index_up(self.active, self.desktops.len());
        let _ = self.switch_to(target);
    }

    pub fn switch_down(&mut self) {
        let target = self.layout.index_down(self.active, self.desktops.len());
        let _ = self.switch_to(target);
    }

    pub fn move_window_to_desktop(
        &mut self,
        window_id: u64,
        desktop: usize,
    ) -> Result<(), DesktopError> {
        if desktop >= self.desktops.len() {
            return Err(DesktopError::InvalidIndex);
        }

        let mut found_window: Option<DesktopWindow> = None;
        for d in &mut self.desktops {
            if let Some(pos) = d.window_index(window_id) {
                found_window = Some(d.windows.remove(pos));
                break;
            }
        }

        match found_window {
            Some(win) => {
                self.desktops[desktop].windows.push(win);
                Ok(())
            }
            None => Err(DesktopError::WindowNotFound),
        }
    }

    pub fn pin_window(&mut self, window_id: u64) {
        for desktop in &mut self.desktops {
            for win in &mut desktop.windows {
                if win.surface_id == window_id {
                    win.pinned = true;
                    return;
                }
            }
        }
    }

    pub fn unpin_window(&mut self, window_id: u64) {
        for desktop in &mut self.desktops {
            for win in &mut desktop.windows {
                if win.surface_id == window_id {
                    win.pinned = false;
                    return;
                }
            }
        }
    }

    pub fn rename_desktop(&mut self, index: usize, name: &str) -> Result<(), DesktopError> {
        let d = self
            .desktops
            .get_mut(index)
            .ok_or(DesktopError::InvalidIndex)?;
        d.name.clear();
        d.name.push_str(name);
        Ok(())
    }

    pub fn reorder_desktops(&mut self, from: usize, to: usize) -> Result<(), DesktopError> {
        if from >= self.desktops.len() || to >= self.desktops.len() {
            return Err(DesktopError::InvalidIndex);
        }
        if from == to {
            return Ok(());
        }

        let desktop = self.desktops.remove(from);
        self.desktops.insert(to, desktop);

        if self.active == from {
            self.active = to;
        } else if from < to && self.active > from && self.active <= to {
            self.active -= 1;
        } else if from > to && self.active >= to && self.active < from {
            self.active += 1;
        }
        Ok(())
    }

    pub fn show_overview(&mut self) {
        self.overview_active = true;
    }

    pub fn hide_overview(&mut self) {
        self.overview_active = false;
        self.drag_state = None;
    }

    pub fn toggle_overview(&mut self) {
        if self.overview_active {
            self.hide_overview();
        } else {
            self.show_overview();
        }
    }

    pub fn active_desktop(&self) -> &VirtualDesktop {
        &self.desktops[self.active]
    }

    pub fn active_index(&self) -> usize {
        self.active
    }

    pub fn desktop_count(&self) -> usize {
        self.desktops.len()
    }

    pub fn tick(&mut self, delta_ms: u64) -> bool {
        if self.animation.active {
            self.update_transition(delta_ms)
        } else {
            false
        }
    }

    pub fn handle_gesture(&mut self, gesture: DesktopGesture) {
        if !self.settings.gesture_enabled {
            return;
        }
        match gesture {
            DesktopGesture::SwipeLeft | DesktopGesture::ThreeFingerSwipeLeft => {
                self.switch_left();
            }
            DesktopGesture::SwipeRight | DesktopGesture::ThreeFingerSwipeRight => {
                self.switch_right();
            }
            DesktopGesture::SwipeUp => {
                self.show_overview();
            }
            DesktopGesture::SwipeDown => {
                self.hide_overview();
            }
            DesktopGesture::PinchIn => {
                self.show_overview();
            }
            DesktopGesture::PinchOut => {
                self.hide_overview();
            }
        }
    }

    pub fn render_overview(&self, canvas: &mut [u32], width: u32, height: u32) {
        if !self.overview_active {
            return;
        }

        let w = width as usize;
        let h = height as usize;

        // Dim background
        for pixel in canvas.iter_mut().take(w * h) {
            let r = ((*pixel >> 16) & 0xFF) / 3;
            let g = ((*pixel >> 8) & 0xFF) / 3;
            let b = (*pixel & 0xFF) / 3;
            *pixel = 0xFF_00_00_00 | (r << 16) | (g << 8) | b;
        }

        let count = self.desktops.len();
        if count == 0 {
            return;
        }

        let cols = match self.layout.layout_type {
            LayoutType::Linear => count.min(6),
            LayoutType::Grid => self.layout.columns as usize,
        };
        let rows = (count + cols - 1) / cols;

        let padding = 24usize;
        let thumb_w = (w.saturating_sub(padding * (cols + 1))) / cols;
        let thumb_h = (h.saturating_sub(padding * (rows + 1) + 40)) / rows;
        let label_h = 28usize;

        let total_w = cols * thumb_w + (cols - 1) * padding;
        let total_h = rows * (thumb_h + label_h) + (rows - 1) * padding;
        let start_x = (w.saturating_sub(total_w)) / 2;
        let start_y = (h.saturating_sub(total_h)) / 2;

        for (i, desktop) in self.desktops.iter().enumerate() {
            let col = i % cols;
            let row = i / cols;
            let dx = start_x + col * (thumb_w + padding);
            let dy = start_y + row * (thumb_h + label_h + padding);

            let border_color = if i == self.active {
                OVERVIEW_ACTIVE_BORDER
            } else {
                OVERVIEW_INACTIVE_BORDER
            };

            fill_rect_in_buffer(canvas, w, h, dx, dy, thumb_w, thumb_h, OVERVIEW_DESKTOP_BG);
            draw_rect_outline_in_buffer(canvas, w, h, dx, dy, thumb_w, thumb_h, border_color);

            if i == self.active {
                draw_rect_outline_in_buffer(
                    canvas,
                    w,
                    h,
                    dx.saturating_sub(1),
                    dy.saturating_sub(1),
                    thumb_w + 2,
                    thumb_h + 2,
                    border_color,
                );
            }

            // Render mini window thumbnails inside the desktop preview
            let scale = self.settings.overview_scale;
            for win in &desktop.windows {
                if win.minimized {
                    continue;
                }
                let (wx, wy, ww, wh) = win.rect;
                let mini_x = dx + (wx as f32 * scale) as usize;
                let mini_y = dy + (wy as f32 * scale) as usize;
                let mini_w = (ww as f32 * scale).max(4.0) as usize;
                let mini_h = (wh as f32 * scale).max(4.0) as usize;

                if mini_x < dx + thumb_w && mini_y < dy + thumb_h {
                    let clamped_w = mini_w.min(dx + thumb_w - mini_x);
                    let clamped_h = mini_h.min(dy + thumb_h - mini_y);
                    fill_rect_in_buffer(
                        canvas,
                        w,
                        h,
                        mini_x,
                        mini_y,
                        clamped_w,
                        clamped_h,
                        0xFF_28_2C_44,
                    );
                    draw_rect_outline_in_buffer(
                        canvas,
                        w,
                        h,
                        mini_x,
                        mini_y,
                        clamped_w,
                        clamped_h,
                        0xFF_4E_9C_FF,
                    );
                }
            }

            // Desktop name label
            let label_y = dy + thumb_h + 4;
            let name_bytes = desktop.name.as_bytes();
            let max_chars = thumb_w / GLYPH_W;
            let chars_to_draw = name_bytes.len().min(max_chars);
            let text_start_x = dx + (thumb_w.saturating_sub(chars_to_draw * GLYPH_W)) / 2;

            for (ci, &byte) in name_bytes.iter().take(chars_to_draw).enumerate() {
                let cx = text_start_x + ci * GLYPH_W;
                let fg = if i == self.active {
                    OVERVIEW_LABEL_FG
                } else {
                    OVERVIEW_DIM_FG
                };
                put_glyph_in_buffer(canvas, w, h, cx, label_y, byte, fg);
            }

            // Desktop number indicator
            if self.settings.show_desktop_number {
                let num = (i + 1) as u8;
                let digit = if num < 10 { b'0' + num } else { b'?' };
                let num_x = dx + thumb_w - GLYPH_W - 4;
                let num_y = dy + 4;
                put_glyph_in_buffer(canvas, w, h, num_x, num_y, digit, OVERVIEW_DIM_FG);
            }
        }

        // "New Desktop +" button
        if self.desktops.len() < self.settings.max_desktops {
            let btn_w = 120usize;
            let btn_h = 28usize;
            let btn_x = (w.saturating_sub(btn_w)) / 2;
            let btn_y = start_y + total_h + padding;

            if btn_y + btn_h < h {
                fill_rect_in_buffer(
                    canvas,
                    w,
                    h,
                    btn_x,
                    btn_y,
                    btn_w,
                    btn_h,
                    OVERVIEW_DESKTOP_BG,
                );
                draw_rect_outline_in_buffer(
                    canvas,
                    w,
                    h,
                    btn_x,
                    btn_y,
                    btn_w,
                    btn_h,
                    OVERVIEW_INACTIVE_BORDER,
                );

                let label = b"+ New Desktop";
                let lx = btn_x + (btn_w.saturating_sub(label.len() * GLYPH_W)) / 2;
                let ly = btn_y + (btn_h.saturating_sub(GLYPH_H)) / 2;
                for (ci, &byte) in label.iter().enumerate() {
                    put_glyph_in_buffer(
                        canvas,
                        w,
                        h,
                        lx + ci * GLYPH_W,
                        ly,
                        byte,
                        OVERVIEW_LABEL_FG,
                    );
                }
            }
        }
    }

    pub fn render_indicator(&self, canvas: &mut [u32], width: u32, height: u32, x: i32, y: i32) {
        let count = self.desktops.len();
        if count == 0 {
            return;
        }

        let dot_size = 8usize;
        let dot_gap = 6usize;
        let total_w = count * dot_size + (count - 1) * dot_gap;
        let padding = 8usize;
        let bar_w = total_w + padding * 2;
        let bar_h = dot_size + padding * 2;

        let bx = if x < 0 {
            ((width as usize).saturating_sub(bar_w)) / 2
        } else {
            x as usize
        };
        let by = y as usize;
        let w = width as usize;
        let h = height as usize;

        fill_rect_in_buffer(canvas, w, h, bx, by, bar_w, bar_h, INDICATOR_BG);
        draw_rect_outline_in_buffer(canvas, w, h, bx, by, bar_w, bar_h, INDICATOR_INACTIVE);

        for i in 0..count {
            let dx = bx + padding + i * (dot_size + dot_gap);
            let dy = by + padding;
            let color = if i == self.active {
                INDICATOR_ACTIVE
            } else {
                INDICATOR_INACTIVE
            };
            fill_rect_in_buffer(canvas, w, h, dx, dy, dot_size, dot_size, color);
        }
    }

    pub fn render_transition(&self, canvas: &mut [u32], width: u32, height: u32) {
        if !self.animation.active {
            return;
        }

        let w = width as usize;
        let h = height as usize;
        let t = self.animation.progress;

        match self.animation.transition_type {
            TransitionType::Fade => {
                let alpha = (t * 255.0) as u32;
                let inv_alpha = 255 - alpha;
                for pixel in canvas.iter_mut().take(w * h) {
                    let r = ((*pixel >> 16) & 0xFF) * inv_alpha / 255;
                    let g = ((*pixel >> 8) & 0xFF) * inv_alpha / 255;
                    let b = (*pixel & 0xFF) * inv_alpha / 255;
                    *pixel = 0xFF_00_00_00 | (r << 16) | (g << 8) | b;
                }
            }
            TransitionType::SlideHorizontal => {
                let offset = (t * w as f32) as usize;
                let going_right = self.animation.to_desktop > self.animation.from_desktop;
                if going_right {
                    shift_buffer_horizontal(canvas, w, h, offset, true);
                } else {
                    shift_buffer_horizontal(canvas, w, h, offset, false);
                }
            }
            TransitionType::SlideVertical => {
                let offset = (t * h as f32) as usize;
                let going_down = self.animation.to_desktop > self.animation.from_desktop;
                if going_down {
                    shift_buffer_vertical(canvas, w, h, offset, true);
                } else {
                    shift_buffer_vertical(canvas, w, h, offset, false);
                }
            }
            TransitionType::Zoom => {
                let scale = 1.0 - t * 0.3;
                let cx = w / 2;
                let cy = h / 2;
                let new_w = (w as f32 * scale) as usize;
                let new_h = (h as f32 * scale) as usize;
                let ox = cx.saturating_sub(new_w / 2);
                let oy = cy.saturating_sub(new_h / 2);

                for y_pos in 0..h {
                    for x_pos in 0..w {
                        let inside =
                            x_pos >= ox && x_pos < ox + new_w && y_pos >= oy && y_pos < oy + new_h;
                        if !inside {
                            let idx = y_pos * w + x_pos;
                            if idx < canvas.len() {
                                canvas[idx] = OVERVIEW_BG;
                            }
                        }
                    }
                }
            }
            TransitionType::Cube | TransitionType::Flip => {
                let alpha = ((1.0 - t) * 200.0) as u32;
                for pixel in canvas.iter_mut().take(w * h) {
                    let r = ((*pixel >> 16) & 0xFF) * alpha / 255;
                    let g = ((*pixel >> 8) & 0xFF) * alpha / 255;
                    let b = (*pixel & 0xFF) * alpha / 255;
                    *pixel = 0xFF_00_00_00 | (r << 16) | (g << 8) | b;
                }
            }
            TransitionType::None => {}
        }
    }

    fn update_transition(&mut self, delta_ms: u64) -> bool {
        if !self.animation.active {
            return false;
        }

        let step = delta_ms as f32 / self.animation.duration_ms as f32;
        self.animation.progress += step;

        if self.animation.progress >= 1.0 {
            self.animation.progress = 1.0;
            self.animation.reset();
            return true;
        }

        true
    }

    fn apply_easing(&self, t: f32) -> f32 {
        match self.animation.easing {
            EasingFunction::Linear => t,
            EasingFunction::EaseIn => t * t,
            EasingFunction::EaseOut => t * (2.0 - t),
            EasingFunction::EaseInOut => {
                if t < 0.5 {
                    2.0 * t * t
                } else {
                    -1.0 + (4.0 - 2.0 * t) * t
                }
            }
            EasingFunction::CubicBezier(p1x, p1y, p2x, p2y) => {
                cubic_bezier_approx(t, p1x, p1y, p2x, p2y)
            }
            EasingFunction::Spring(stiffness, damping) => spring_value(t, stiffness, damping),
        }
    }

    fn generate_thumbnails(&mut self) {
        for desktop in &mut self.desktops {
            for win in &mut desktop.windows {
                if win.thumbnail.is_none() {
                    let (_, _, ww, wh) = win.rect;
                    let tw = (ww / 8).max(1) as usize;
                    let th = (wh / 8).max(1) as usize;
                    let mut thumb = vec![0xFF_28_2C_44u32; tw * th];
                    // Top bar
                    for x in 0..tw {
                        if x < thumb.len() {
                            thumb[x] = 0xFF_4E_9C_FF;
                        }
                    }
                    win.thumbnail = Some(thumb);
                }
            }
        }
    }

    pub fn desktops(&self) -> &[VirtualDesktop] {
        &self.desktops
    }

    pub fn settings(&self) -> &DesktopSettings {
        &self.settings
    }

    pub fn settings_mut(&mut self) -> &mut DesktopSettings {
        &mut self.settings
    }

    pub fn layout_mut(&mut self) -> &mut DesktopLayout {
        &mut self.layout
    }

    pub fn is_overview_active(&self) -> bool {
        self.overview_active
    }

    pub fn is_transitioning(&self) -> bool {
        self.animation.active
    }

    pub fn transition_progress(&self) -> f32 {
        if self.animation.active {
            self.apply_easing(self.animation.progress)
        } else {
            0.0
        }
    }

    pub fn previous_desktop(&self) -> Option<usize> {
        self.history.last().copied()
    }

    pub fn switch_to_previous(&mut self) {
        if let Some(prev) = self.history.last().copied() {
            let _ = self.switch_to(prev);
        }
    }

    pub fn find_window(&self, window_id: u64) -> Option<(usize, usize)> {
        for (di, desktop) in self.desktops.iter().enumerate() {
            if let Some(wi) = desktop.window_index(window_id) {
                return Some((di, wi));
            }
        }
        None
    }

    pub fn add_window_to_active(
        &mut self,
        surface_id: u64,
        title: &str,
        app_id: &str,
        rect: (i32, i32, u32, u32),
    ) {
        let win = DesktopWindow {
            surface_id,
            title: String::from(title),
            app_id: String::from(app_id),
            rect,
            minimized: false,
            maximized: false,
            pinned: false,
            z_order: 0,
            thumbnail: None,
            icon: None,
        };
        self.desktops[self.active].windows.push(win);
    }

    pub fn remove_window_globally(&mut self, window_id: u64) -> bool {
        for desktop in &mut self.desktops {
            if let Some(pos) = desktop.window_index(window_id) {
                desktop.windows.remove(pos);
                return true;
            }
        }
        false
    }
}

// ── Buffer drawing helpers ───────────────────────────────────────────────

fn fill_rect_in_buffer(
    buf: &mut [u32],
    buf_w: usize,
    buf_h: usize,
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    color: u32,
) {
    for row in y..y + h {
        if row >= buf_h {
            break;
        }
        for col in x..x + w {
            if col >= buf_w {
                break;
            }
            let idx = row * buf_w + col;
            if idx < buf.len() {
                buf[idx] = color;
            }
        }
    }
}

fn draw_rect_outline_in_buffer(
    buf: &mut [u32],
    buf_w: usize,
    buf_h: usize,
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    color: u32,
) {
    for col in x..x + w {
        if col >= buf_w {
            continue;
        }
        if y < buf_h {
            buf[y * buf_w + col] = color;
        }
        let bot = y + h.saturating_sub(1);
        if bot < buf_h {
            buf[bot * buf_w + col] = color;
        }
    }
    for row in y..y + h {
        if row >= buf_h {
            continue;
        }
        if x < buf_w {
            buf[row * buf_w + x] = color;
        }
        let right = x + w.saturating_sub(1);
        if right < buf_w {
            buf[row * buf_w + right] = color;
        }
    }
}

fn put_glyph_in_buffer(
    buf: &mut [u32],
    buf_w: usize,
    buf_h: usize,
    x: usize,
    y: usize,
    _ch: u8,
    color: u32,
) {
    for dy in 0..GLYPH_H {
        for dx in 0..GLYPH_W {
            let px = x + dx;
            let py = y + dy;
            if px < buf_w && py < buf_h {
                if (dy == 0 || dy == GLYPH_H - 1) || (dx == 0 || dx == GLYPH_W - 1) {
                    buf[py * buf_w + px] = color;
                }
            }
        }
    }
}

fn shift_buffer_horizontal(buf: &mut [u32], w: usize, h: usize, offset: usize, left: bool) {
    let clamped = offset.min(w);
    for row in 0..h {
        let base = row * w;
        if left {
            for col in 0..w {
                let src = col + clamped;
                let idx = base + col;
                if src < w && idx < buf.len() {
                    buf[idx] = buf[base + src];
                } else if idx < buf.len() {
                    buf[idx] = OVERVIEW_BG;
                }
            }
        } else {
            for col in (0..w).rev() {
                let idx = base + col;
                if col >= clamped {
                    let src = base + col - clamped;
                    if idx < buf.len() && src < buf.len() {
                        buf[idx] = buf[src];
                    }
                } else if idx < buf.len() {
                    buf[idx] = OVERVIEW_BG;
                }
            }
        }
    }
}

fn shift_buffer_vertical(buf: &mut [u32], w: usize, h: usize, offset: usize, up: bool) {
    let clamped = offset.min(h);
    if up {
        for row in 0..h {
            let src_row = row + clamped;
            for col in 0..w {
                let idx = row * w + col;
                if src_row < h {
                    let src_idx = src_row * w + col;
                    if idx < buf.len() && src_idx < buf.len() {
                        buf[idx] = buf[src_idx];
                    }
                } else if idx < buf.len() {
                    buf[idx] = OVERVIEW_BG;
                }
            }
        }
    } else {
        for row in (0..h).rev() {
            for col in 0..w {
                let idx = row * w + col;
                if row >= clamped {
                    let src_idx = (row - clamped) * w + col;
                    if idx < buf.len() && src_idx < buf.len() {
                        buf[idx] = buf[src_idx];
                    }
                } else if idx < buf.len() {
                    buf[idx] = OVERVIEW_BG;
                }
            }
        }
    }
}

fn cubic_bezier_approx(t: f32, p1x: f32, p1y: f32, p2x: f32, p2y: f32) -> f32 {
    let cx = 3.0 * p1x;
    let bx = 3.0 * (p2x - p1x) - cx;
    let ax = 1.0 - cx - bx;

    let cy = 3.0 * p1y;
    let by = 3.0 * (p2y - p1y) - cy;
    let ay = 1.0 - cy - by;

    let mut guess = t;
    for _ in 0..8 {
        let x = ((ax * guess + bx) * guess + cx) * guess;
        let dx = (3.0 * ax * guess + 2.0 * bx) * guess + cx;
        if dx.abs() < 1e-6 {
            break;
        }
        guess -= (x - t) / dx;
        guess = guess.clamp(0.0, 1.0);
    }

    ((ay * guess + by) * guess + cy) * guess
}

fn f32_sqrt(x: f32) -> f32 {
    if x <= 0.0 {
        return 0.0;
    }
    let mut g = x / 2.0;
    for _ in 0..15 {
        g = (g + x / g) * 0.5;
    }
    g
}

fn f32_exp(x: f32) -> f32 {
    let mut sum: f32 = 1.0;
    let mut term: f32 = 1.0;
    for i in 1..20 {
        term *= x / i as f32;
        sum += term;
    }
    sum
}

fn f32_cos(x: f32) -> f32 {
    let pi = 3.14159265;
    let mut x = x % (2.0 * pi);
    if x < 0.0 {
        x += 2.0 * pi;
    }
    let x2 = x * x;
    1.0 - x2 / 2.0 + x2 * x2 / 24.0 - x2 * x2 * x2 / 720.0 + x2 * x2 * x2 * x2 / 40320.0
}

fn spring_value(t: f32, stiffness: f32, damping: f32) -> f32 {
    let omega = f32_sqrt(stiffness);
    let decay = f32_exp(-damping * t);
    1.0 - decay * f32_cos(omega * t)
}
