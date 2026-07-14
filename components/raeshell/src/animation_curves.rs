//! Window Animation Curve Editor — user-editable cubic bezier curves
//! for window open/close/minimize/maximize transitions.
//!
//! Curves are pure data: four control points defining a cubic bezier
//! in normalised [0,1] space.  Preset curves are provided (Linear,
//! EaseIn, EaseOut, Bounce, Spring).  Users can edit curves visually
//! and assign them per-action.

#![allow(dead_code)]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

// ── Cubic Bezier Curve ───────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CubicBezier {
    pub x1: f32,
    pub y1: f32,
    pub x2: f32,
    pub y2: f32,
}

impl CubicBezier {
    pub const fn new(x1: f32, y1: f32, x2: f32, y2: f32) -> Self {
        Self { x1, y1, x2, y2 }
    }

    pub fn sample_y(&self, t: f32) -> f32 {
        let t = clamp01(t);
        let t2 = t * t;
        let t3 = t2 * t;
        let mt = 1.0 - t;
        let mt2 = mt * mt;
        let mt3 = mt2 * mt;

        // P0=(0,0), P1=(x1,y1), P2=(x2,y2), P3=(1,1)
        // y = 3*mt2*t*y1 + 3*mt*t2*y2 + t3
        3.0 * mt2 * t * self.y1 + 3.0 * mt * t2 * self.y2 + t3
    }

    pub fn sample_x(&self, t: f32) -> f32 {
        let t = clamp01(t);
        let t2 = t * t;
        let t3 = t2 * t;
        let mt = 1.0 - t;
        let mt2 = mt * mt;

        3.0 * mt2 * t * self.x1 + 3.0 * mt * t2 * self.x2 + t3
    }

    pub fn evaluate(&self, x: f32) -> f32 {
        let x = clamp01(x);
        if x <= 0.0 {
            return 0.0;
        }
        if x >= 1.0 {
            return 1.0;
        }

        // Newton's method to find t for given x
        let mut t = x;
        for _ in 0..8 {
            let current_x = self.sample_x(t);
            let dx = current_x - x;
            if dx > -0.0001 && dx < 0.0001 {
                break;
            }

            let derivative = self.dx_dt(t);
            if derivative > -0.00001 && derivative < 0.00001 {
                break;
            }
            t -= dx / derivative;
            t = clamp01(t);
        }

        self.sample_y(t)
    }

    fn dx_dt(&self, t: f32) -> f32 {
        let mt = 1.0 - t;
        // derivative of bezier x(t)
        3.0 * mt * mt * self.x1 + 6.0 * mt * t * (self.x2 - self.x1) + 3.0 * t * t * (1.0 - self.x2)
    }

    pub fn is_valid(&self) -> bool {
        self.x1 >= 0.0 && self.x1 <= 1.0 && self.x2 >= 0.0 && self.x2 <= 1.0
    }
}

fn clamp01(x: f32) -> f32 {
    if x < 0.0 {
        0.0
    } else if x > 1.0 {
        1.0
    } else {
        x
    }
}

// ── AnimationCurve — wraps a bezier with metadata ────────────────────────

#[derive(Debug, Clone)]
pub struct AnimationCurve {
    pub name: String,
    pub bezier: CubicBezier,
}

impl AnimationCurve {
    pub fn new(name: &str, bezier: CubicBezier) -> Self {
        Self {
            name: String::from(name),
            bezier,
        }
    }

    pub fn evaluate(&self, t: f32) -> f32 {
        self.bezier.evaluate(t)
    }
}

// ── Preset curves ────────────────────────────────────────────────────────

pub const LINEAR: CubicBezier = CubicBezier::new(0.0, 0.0, 1.0, 1.0);
pub const EASE_IN: CubicBezier = CubicBezier::new(0.42, 0.0, 1.0, 1.0);
pub const EASE_OUT: CubicBezier = CubicBezier::new(0.0, 0.0, 0.58, 1.0);
pub const EASE_IN_OUT: CubicBezier = CubicBezier::new(0.42, 0.0, 0.58, 1.0);
pub const EASE_OUT_BACK: CubicBezier = CubicBezier::new(0.34, 1.56, 0.64, 1.0);
pub const EASE_IN_BACK: CubicBezier = CubicBezier::new(0.36, 0.0, 0.66, -0.56);
pub const SNAPPY: CubicBezier = CubicBezier::new(0.1, 0.9, 0.2, 1.0);

pub fn preset_linear() -> AnimationCurve {
    AnimationCurve::new("Linear", LINEAR)
}

pub fn preset_ease_in() -> AnimationCurve {
    AnimationCurve::new("Ease In", EASE_IN)
}

pub fn preset_ease_out() -> AnimationCurve {
    AnimationCurve::new("Ease Out", EASE_OUT)
}

pub fn preset_ease_in_out() -> AnimationCurve {
    AnimationCurve::new("Ease In Out", EASE_IN_OUT)
}

pub fn preset_bounce() -> AnimationCurve {
    AnimationCurve::new("Bounce", CubicBezier::new(0.34, 1.56, 0.64, 1.0))
}

pub fn preset_spring() -> AnimationCurve {
    AnimationCurve::new("Spring", CubicBezier::new(0.175, 0.885, 0.32, 1.275))
}

pub fn preset_snappy() -> AnimationCurve {
    AnimationCurve::new("Snappy", SNAPPY)
}

pub fn all_presets() -> Vec<AnimationCurve> {
    alloc::vec![
        preset_linear(),
        preset_ease_in(),
        preset_ease_out(),
        preset_ease_in_out(),
        preset_bounce(),
        preset_spring(),
        preset_snappy(),
    ]
}

// ── Window action types ──────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Ord, PartialOrd)]
pub enum WindowAction {
    Open,
    Close,
    Minimize,
    Maximize,
    Restore,
    Move,
    Resize,
    FocusIn,
    FocusOut,
    WorkspaceSwitch,
}

pub const ALL_ACTIONS: &[WindowAction] = &[
    WindowAction::Open,
    WindowAction::Close,
    WindowAction::Minimize,
    WindowAction::Maximize,
    WindowAction::Restore,
    WindowAction::Move,
    WindowAction::Resize,
    WindowAction::FocusIn,
    WindowAction::FocusOut,
    WindowAction::WorkspaceSwitch,
];

// ── Per-action curve assignment ──────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ActionCurveBinding {
    pub action: WindowAction,
    pub curve: AnimationCurve,
    pub duration_ms: u32,
    pub enabled: bool,
}

impl ActionCurveBinding {
    pub fn new(action: WindowAction, curve: AnimationCurve, duration_ms: u32) -> Self {
        Self {
            action,
            curve,
            duration_ms,
            enabled: true,
        }
    }
}

pub struct CurveAssignmentTable {
    pub bindings: Vec<ActionCurveBinding>,
}

impl CurveAssignmentTable {
    pub fn new() -> Self {
        Self {
            bindings: Vec::new(),
        }
    }

    pub fn defaults() -> Self {
        let mut table = Self::new();
        table.bindings.push(ActionCurveBinding::new(
            WindowAction::Open,
            preset_ease_out(),
            250,
        ));
        table.bindings.push(ActionCurveBinding::new(
            WindowAction::Close,
            preset_ease_in(),
            200,
        ));
        table.bindings.push(ActionCurveBinding::new(
            WindowAction::Minimize,
            preset_snappy(),
            180,
        ));
        table.bindings.push(ActionCurveBinding::new(
            WindowAction::Maximize,
            preset_ease_in_out(),
            220,
        ));
        table.bindings.push(ActionCurveBinding::new(
            WindowAction::Restore,
            preset_ease_out(),
            220,
        ));
        table.bindings.push(ActionCurveBinding::new(
            WindowAction::Move,
            preset_linear(),
            0,
        ));
        table.bindings.push(ActionCurveBinding::new(
            WindowAction::Resize,
            preset_linear(),
            0,
        ));
        table.bindings.push(ActionCurveBinding::new(
            WindowAction::FocusIn,
            preset_ease_out(),
            150,
        ));
        table.bindings.push(ActionCurveBinding::new(
            WindowAction::FocusOut,
            preset_ease_in(),
            150,
        ));
        table.bindings.push(ActionCurveBinding::new(
            WindowAction::WorkspaceSwitch,
            preset_snappy(),
            300,
        ));
        table
    }

    pub fn get(&self, action: WindowAction) -> Option<&ActionCurveBinding> {
        self.bindings.iter().find(|b| b.action == action)
    }

    pub fn set_curve(&mut self, action: WindowAction, curve: AnimationCurve) {
        if let Some(b) = self.bindings.iter_mut().find(|b| b.action == action) {
            b.curve = curve;
        }
    }

    pub fn set_duration(&mut self, action: WindowAction, ms: u32) {
        if let Some(b) = self.bindings.iter_mut().find(|b| b.action == action) {
            b.duration_ms = ms;
        }
    }

    pub fn set_enabled(&mut self, action: WindowAction, enabled: bool) {
        if let Some(b) = self.bindings.iter_mut().find(|b| b.action == action) {
            b.enabled = enabled;
        }
    }

    pub fn evaluate(&self, action: WindowAction, progress: f32) -> f32 {
        match self.get(action) {
            Some(b) if b.enabled => b.curve.evaluate(progress),
            _ => progress,
        }
    }
}

// ── Active animation tracker ─────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ActiveAnimation {
    pub window_id: u64,
    pub action: WindowAction,
    pub elapsed_ms: u32,
    pub duration_ms: u32,
    pub start_x: f32,
    pub start_y: f32,
    pub start_w: f32,
    pub start_h: f32,
    pub start_alpha: f32,
    pub end_x: f32,
    pub end_y: f32,
    pub end_w: f32,
    pub end_h: f32,
    pub end_alpha: f32,
}

impl ActiveAnimation {
    pub fn new(window_id: u64, action: WindowAction, duration_ms: u32) -> Self {
        Self {
            window_id,
            action,
            elapsed_ms: 0,
            duration_ms,
            start_x: 0.0,
            start_y: 0.0,
            start_w: 0.0,
            start_h: 0.0,
            start_alpha: 0.0,
            end_x: 0.0,
            end_y: 0.0,
            end_w: 0.0,
            end_h: 0.0,
            end_alpha: 1.0,
        }
    }

    pub fn tick(&mut self, delta_ms: u32) {
        self.elapsed_ms = self.elapsed_ms.saturating_add(delta_ms);
    }

    pub fn is_finished(&self) -> bool {
        self.elapsed_ms >= self.duration_ms
    }

    pub fn linear_progress(&self) -> f32 {
        if self.duration_ms == 0 {
            return 1.0;
        }
        let t = self.elapsed_ms as f32 / self.duration_ms as f32;
        clamp01(t)
    }

    pub fn interpolate(&self, table: &CurveAssignmentTable) -> AnimationFrame {
        let t = self.linear_progress();
        let eased = table.evaluate(self.action, t);

        AnimationFrame {
            x: self.start_x + (self.end_x - self.start_x) * eased,
            y: self.start_y + (self.end_y - self.start_y) * eased,
            w: self.start_w + (self.end_w - self.start_w) * eased,
            h: self.start_h + (self.end_h - self.start_h) * eased,
            alpha: self.start_alpha + (self.end_alpha - self.start_alpha) * eased,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct AnimationFrame {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub alpha: f32,
}

// ── Animation Manager ────────────────────────────────────────────────────

pub struct AnimationManager {
    pub active: Vec<ActiveAnimation>,
    pub table: CurveAssignmentTable,
    pub enabled: bool,
    pub speed_multiplier: u8,
}

impl AnimationManager {
    pub fn new() -> Self {
        Self {
            active: Vec::new(),
            table: CurveAssignmentTable::defaults(),
            enabled: true,
            speed_multiplier: 100,
        }
    }

    pub fn start_animation(&mut self, anim: ActiveAnimation) {
        if !self.enabled {
            return;
        }
        self.active
            .retain(|a| a.window_id != anim.window_id || a.action != anim.action);
        self.active.push(anim);
    }

    pub fn tick(&mut self, delta_ms: u32) {
        let scaled = (delta_ms as u64 * self.speed_multiplier as u64 / 100) as u32;
        for anim in &mut self.active {
            anim.tick(scaled);
        }
        self.active.retain(|a| !a.is_finished());
    }

    pub fn frame_for(&self, window_id: u64) -> Option<AnimationFrame> {
        self.active
            .iter()
            .find(|a| a.window_id == window_id)
            .map(|a| a.interpolate(&self.table))
    }

    pub fn is_animating(&self, window_id: u64) -> bool {
        self.active.iter().any(|a| a.window_id == window_id)
    }

    pub fn cancel(&mut self, window_id: u64) {
        self.active.retain(|a| a.window_id != window_id);
    }

    pub fn cancel_all(&mut self) {
        self.active.clear();
    }

    pub fn set_speed(&mut self, pct: u8) {
        self.speed_multiplier = pct.max(10);
    }

    pub fn active_count(&self) -> usize {
        self.active.len()
    }
}

// ── User-saved curve library ─────────────────────────────────────────────

pub struct CurveLibrary {
    pub curves: Vec<AnimationCurve>,
}

impl CurveLibrary {
    pub fn new() -> Self {
        Self {
            curves: all_presets(),
        }
    }

    pub fn add(&mut self, curve: AnimationCurve) {
        if !self.curves.iter().any(|c| c.name == curve.name) {
            self.curves.push(curve);
        }
    }

    pub fn remove(&mut self, name: &str) {
        self.curves.retain(|c| c.name != name);
    }

    pub fn find(&self, name: &str) -> Option<&AnimationCurve> {
        self.curves.iter().find(|c| c.name == name)
    }

    pub fn names(&self) -> Vec<&str> {
        self.curves.iter().map(|c| c.name.as_str()).collect()
    }
}
