#![no_std]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

// ── no_std math helpers ──────────────────────────────────────────────────

fn f32_powi(base: f32, exp: i32) -> f32 {
    let mut result = 1.0f32;
    let mut b = base;
    let mut e = if exp < 0 { -exp } else { exp } as u32;
    while e > 0 {
        if e & 1 == 1 {
            result *= b;
        }
        b *= b;
        e >>= 1;
    }
    if exp < 0 {
        1.0 / result
    } else {
        result
    }
}

fn f32_powf(base: f32, exp: f32) -> f32 {
    if base <= 0.0 {
        return 0.0;
    }
    f32_exp(exp * f32_ln(base))
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

fn f32_ln(x: f32) -> f32 {
    if x <= 0.0 {
        return -1e30;
    }
    let mut val = x as f64;
    let mut result: f64 = 0.0;
    while val > 2.0 {
        val /= 2.718281828;
        result += 1.0;
    }
    while val < 0.5 {
        val *= 2.718281828;
        result -= 1.0;
    }
    let y = (val - 1.0) / (val + 1.0);
    let y2 = y * y;
    let mut term = y;
    for i in 0..10 {
        result += 2.0 * term / (2 * i + 1) as f64;
        term *= y2;
    }
    result as f32
}

fn f32_sin(x: f32) -> f32 {
    let pi = 3.14159265f32;
    let mut x = x % (2.0 * pi);
    if x < -pi {
        x += 2.0 * pi;
    }
    if x > pi {
        x -= 2.0 * pi;
    }
    let x2 = x * x;
    x * (1.0 - x2 / 6.0 + x2 * x2 / 120.0 - x2 * x2 * x2 / 5040.0 + x2 * x2 * x2 * x2 / 362880.0)
}

fn f32_cos(x: f32) -> f32 {
    let pi = 3.14159265f32;
    let mut x = x % (2.0 * pi);
    if x < -pi {
        x += 2.0 * pi;
    }
    if x > pi {
        x -= 2.0 * pi;
    }
    let x2 = x * x;
    1.0 - x2 / 2.0 + x2 * x2 / 24.0 - x2 * x2 * x2 / 720.0 + x2 * x2 * x2 * x2 / 40320.0
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

fn f32_ceil(x: f32) -> f32 {
    let i = x as i32;
    if (i as f32) < x {
        (i + 1) as f32
    } else {
        i as f32
    }
}

fn f32_floor(x: f32) -> f32 {
    let i = x as i32;
    if (i as f32) > x {
        (i - 1) as f32
    } else {
        i as f32
    }
}

// ── Direction ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

// ── Easing functions ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EasingFunction {
    Linear,
    EaseIn,
    EaseOut,
    EaseInOut,
    EaseInQuad,
    EaseOutQuad,
    EaseInOutQuad,
    EaseInCubic,
    EaseOutCubic,
    EaseInOutCubic,
    EaseInQuart,
    EaseOutQuart,
    EaseInOutQuart,
    EaseInExpo,
    EaseOutExpo,
    EaseInOutExpo,
    EaseInBack,
    EaseOutBack,
    EaseInOutBack,
    EaseInElastic,
    EaseOutElastic,
    EaseInOutElastic,
    EaseInBounce,
    EaseOutBounce,
    EaseInOutBounce,
    CubicBezier(f32, f32, f32, f32),
    Spring { stiffness: f32, damping: f32 },
    Steps(u32, StepPosition),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepPosition {
    Start,
    End,
}

// ── Animation target & type ──────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum AnimationTarget {
    Window(u64),
    Widget(u64),
    Desktop(usize),
    Global,
    Taskbar,
    Menu,
    Notification(u64),
}

#[derive(Debug, Clone, PartialEq)]
pub enum AnimationType {
    WindowOpen,
    WindowClose,
    WindowMinimize,
    WindowMaximize,
    WindowRestore,
    WindowMove,
    WindowResize,
    WindowSnap,
    WindowShake,
    WindowBounce,
    DesktopSwitch,
    DesktopOverview,
    MenuOpen,
    MenuClose,
    PopupOpen,
    PopupClose,
    NotificationSlideIn,
    NotificationSlideOut,
    NotificationFade,
    FadeIn,
    FadeOut,
    ScaleIn,
    ScaleOut,
    SlideIn(Direction),
    SlideOut(Direction),
    Ripple,
    Pulse,
    Glow,
    Shake,
    Bounce,
    Spin,
    Custom { property: String },
}

// ── Animation value & transform ──────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum AnimationValue {
    Float(f32),
    Vec2(f32, f32),
    Vec4(f32, f32, f32, f32),
    Color(u32),
    Rect(f32, f32, f32, f32),
    Transform(Transform2D),
}

impl AnimationValue {
    pub fn as_float(&self) -> f32 {
        match self {
            AnimationValue::Float(v) => *v,
            _ => 0.0,
        }
    }

    pub fn zero() -> Self {
        AnimationValue::Float(0.0)
    }

    pub fn one() -> Self {
        AnimationValue::Float(1.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transform2D {
    pub translate_x: f32,
    pub translate_y: f32,
    pub scale_x: f32,
    pub scale_y: f32,
    pub rotation: f32,
    pub origin_x: f32,
    pub origin_y: f32,
    pub opacity: f32,
}

impl Transform2D {
    pub fn identity() -> Self {
        Self {
            translate_x: 0.0,
            translate_y: 0.0,
            scale_x: 1.0,
            scale_y: 1.0,
            rotation: 0.0,
            origin_x: 0.5,
            origin_y: 0.5,
            opacity: 1.0,
        }
    }
}

// ── Repeat, state, callback ──────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepeatMode {
    Once,
    Loop,
    PingPong,
    Count(u32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnimationState {
    Pending,
    Running,
    Paused,
    Completed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnimationCallback {
    Remove(u64),
    Show(u64),
    Hide(u64),
    Focus(u64),
    Custom(u64),
}

// ── Animation ────────────────────────────────────────────────────────────

pub struct Animation {
    pub id: u64,
    pub target: AnimationTarget,
    pub animation_type: AnimationType,
    pub easing: EasingFunction,
    pub duration_ms: u32,
    pub elapsed_ms: u32,
    pub delay_ms: u32,
    pub repeat: RepeatMode,
    pub state: AnimationState,
    pub on_complete: Option<AnimationCallback>,
    pub group: Option<u64>,
    pub from: AnimationValue,
    pub to: AnimationValue,
    pub current: AnimationValue,
}

impl Animation {
    pub fn new(
        target: AnimationTarget,
        animation_type: AnimationType,
        duration_ms: u32,
        easing: EasingFunction,
    ) -> Self {
        Self {
            id: 0,
            target,
            animation_type,
            easing,
            duration_ms,
            elapsed_ms: 0,
            delay_ms: 0,
            repeat: RepeatMode::Once,
            state: AnimationState::Pending,
            on_complete: None,
            group: None,
            from: AnimationValue::Float(0.0),
            to: AnimationValue::Float(1.0),
            current: AnimationValue::Float(0.0),
        }
    }

    pub fn with_delay(mut self, delay_ms: u32) -> Self {
        self.delay_ms = delay_ms;
        self
    }

    pub fn with_repeat(mut self, repeat: RepeatMode) -> Self {
        self.repeat = repeat;
        self
    }

    pub fn with_callback(mut self, cb: AnimationCallback) -> Self {
        self.on_complete = Some(cb);
        self
    }

    pub fn with_group(mut self, group: u64) -> Self {
        self.group = Some(group);
        self
    }

    pub fn with_values(mut self, from: AnimationValue, to: AnimationValue) -> Self {
        self.current = from.clone();
        self.from = from;
        self.to = to;
        self
    }

    fn progress(&self) -> f32 {
        if self.duration_ms == 0 {
            return 1.0;
        }
        let effective = self.elapsed_ms.saturating_sub(self.delay_ms);
        (effective as f32 / self.duration_ms as f32).clamp(0.0, 1.0)
    }

    fn is_delayed(&self) -> bool {
        self.elapsed_ms < self.delay_ms
    }
}

// ── Animation update ─────────────────────────────────────────────────────

pub struct AnimationUpdate {
    pub id: u64,
    pub target: AnimationTarget,
    pub value: AnimationValue,
    pub completed: bool,
}

// ── Animation engine ─────────────────────────────────────────────────────

pub struct AnimationEngine {
    active_animations: Vec<Animation>,
    completed: Vec<u64>,
    next_id: u64,
    global_speed: f32,
    reduce_motion: bool,
    fps_target: u32,
}

impl AnimationEngine {
    pub fn new() -> Self {
        Self {
            active_animations: Vec::new(),
            completed: Vec::new(),
            next_id: 1,
            global_speed: 1.0,
            reduce_motion: false,
            fps_target: 60,
        }
    }

    pub fn start(
        &mut self,
        target: AnimationTarget,
        anim_type: AnimationType,
        duration_ms: u32,
        easing: EasingFunction,
    ) -> u64 {
        let effective_duration = if self.reduce_motion {
            (duration_ms / 4).max(1)
        } else {
            duration_ms
        };

        let mut anim = Animation::new(target, anim_type, effective_duration, easing);
        anim.id = self.next_id;
        self.next_id += 1;
        anim.state = AnimationState::Running;

        let id = anim.id;
        self.active_animations.push(anim);
        id
    }

    pub fn start_with(&mut self, mut animation: Animation) -> u64 {
        animation.id = self.next_id;
        self.next_id += 1;
        animation.state = AnimationState::Running;

        if self.reduce_motion {
            animation.duration_ms = (animation.duration_ms / 4).max(1);
            animation.delay_ms = animation.delay_ms / 4;
        }

        let id = animation.id;
        self.active_animations.push(animation);
        id
    }

    pub fn cancel(&mut self, id: u64) {
        if let Some(anim) = self.active_animations.iter_mut().find(|a| a.id == id) {
            anim.state = AnimationState::Cancelled;
        }
    }

    pub fn cancel_for_target(&mut self, target: &AnimationTarget) {
        for anim in &mut self.active_animations {
            if anim.target == *target {
                anim.state = AnimationState::Cancelled;
            }
        }
    }

    pub fn pause(&mut self, id: u64) {
        if let Some(anim) = self.active_animations.iter_mut().find(|a| a.id == id) {
            if anim.state == AnimationState::Running {
                anim.state = AnimationState::Paused;
            }
        }
    }

    pub fn resume(&mut self, id: u64) {
        if let Some(anim) = self.active_animations.iter_mut().find(|a| a.id == id) {
            if anim.state == AnimationState::Paused {
                anim.state = AnimationState::Running;
            }
        }
    }

    pub fn tick(&mut self, delta_ms: u64) -> Vec<AnimationUpdate> {
        let mut updates = Vec::new();
        let scaled_delta = (delta_ms as f32 * self.global_speed) as u32;

        for anim in &mut self.active_animations {
            if anim.state != AnimationState::Running {
                continue;
            }

            anim.elapsed_ms += scaled_delta;

            if anim.is_delayed() {
                continue;
            }

            let raw_t = anim.progress();
            let t = Self::apply_easing_static(&anim.easing, raw_t);
            anim.current = Self::interpolate_static(&anim.from, &anim.to, t);

            let completed = raw_t >= 1.0;

            updates.push(AnimationUpdate {
                id: anim.id,
                target: anim.target.clone(),
                value: anim.current.clone(),
                completed,
            });

            if completed {
                match anim.repeat {
                    RepeatMode::Once => {
                        anim.state = AnimationState::Completed;
                    }
                    RepeatMode::Loop => {
                        anim.elapsed_ms = anim.delay_ms;
                    }
                    RepeatMode::PingPong => {
                        let tmp = anim.from.clone();
                        anim.from = anim.to.clone();
                        anim.to = tmp;
                        anim.elapsed_ms = anim.delay_ms;
                    }
                    RepeatMode::Count(n) => {
                        if n > 1 {
                            anim.repeat = RepeatMode::Count(n - 1);
                            anim.elapsed_ms = anim.delay_ms;
                        } else {
                            anim.state = AnimationState::Completed;
                        }
                    }
                }
            }
        }

        // Collect completed/cancelled IDs
        let mut to_remove = Vec::new();
        for anim in &self.active_animations {
            if anim.state == AnimationState::Completed || anim.state == AnimationState::Cancelled {
                to_remove.push(anim.id);
                self.completed.push(anim.id);
            }
        }
        self.active_animations.retain(|a| {
            a.state != AnimationState::Completed && a.state != AnimationState::Cancelled
        });

        if self.completed.len() > 256 {
            let drain_count = self.completed.len() - 128;
            self.completed.drain(0..drain_count);
        }

        updates
    }

    pub fn is_animating(&self, target: &AnimationTarget) -> bool {
        self.active_animations
            .iter()
            .any(|a| a.target == *target && a.state == AnimationState::Running)
    }

    pub fn active_count(&self) -> usize {
        self.active_animations
            .iter()
            .filter(|a| a.state == AnimationState::Running)
            .count()
    }

    pub fn set_global_speed(&mut self, speed: f32) {
        self.global_speed = speed.clamp(0.1, 10.0);
    }

    pub fn set_reduce_motion(&mut self, reduce: bool) {
        self.reduce_motion = reduce;
    }

    fn interpolate(&self, from: &AnimationValue, to: &AnimationValue, t: f32) -> AnimationValue {
        Self::interpolate_static(from, to, t)
    }

    fn interpolate_static(from: &AnimationValue, to: &AnimationValue, t: f32) -> AnimationValue {
        match (from, to) {
            (AnimationValue::Float(a), AnimationValue::Float(b)) => {
                AnimationValue::Float(a + (b - a) * t)
            }
            (AnimationValue::Vec2(ax, ay), AnimationValue::Vec2(bx, by)) => {
                AnimationValue::Vec2(ax + (bx - ax) * t, ay + (by - ay) * t)
            }
            (AnimationValue::Vec4(ax, ay, az, aw), AnimationValue::Vec4(bx, by, bz, bw)) => {
                AnimationValue::Vec4(
                    ax + (bx - ax) * t,
                    ay + (by - ay) * t,
                    az + (bz - az) * t,
                    aw + (bw - aw) * t,
                )
            }
            (AnimationValue::Color(a), AnimationValue::Color(b)) => {
                AnimationValue::Color(Self::lerp_color(*a, *b, t))
            }
            (AnimationValue::Rect(ax, ay, aw, ah), AnimationValue::Rect(bx, by, bw, bh)) => {
                AnimationValue::Rect(
                    ax + (bx - ax) * t,
                    ay + (by - ay) * t,
                    aw + (bw - aw) * t,
                    ah + (bh - ah) * t,
                )
            }
            (AnimationValue::Transform(a), AnimationValue::Transform(b)) => {
                AnimationValue::Transform(Transform2D {
                    translate_x: a.translate_x + (b.translate_x - a.translate_x) * t,
                    translate_y: a.translate_y + (b.translate_y - a.translate_y) * t,
                    scale_x: a.scale_x + (b.scale_x - a.scale_x) * t,
                    scale_y: a.scale_y + (b.scale_y - a.scale_y) * t,
                    rotation: a.rotation + (b.rotation - a.rotation) * t,
                    origin_x: a.origin_x + (b.origin_x - a.origin_x) * t,
                    origin_y: a.origin_y + (b.origin_y - a.origin_y) * t,
                    opacity: a.opacity + (b.opacity - a.opacity) * t,
                })
            }
            _ => from.clone(),
        }
    }

    fn apply_easing(&self, easing: &EasingFunction, t: f32) -> f32 {
        Self::apply_easing_static(easing, t)
    }

    fn apply_easing_static(easing: &EasingFunction, t: f32) -> f32 {
        match easing {
            EasingFunction::Linear => t,
            EasingFunction::EaseIn => t * t * t,
            EasingFunction::EaseOut => 1.0 - f32_powi(1.0 - t, 3),
            EasingFunction::EaseInOut => {
                if t < 0.5 {
                    4.0 * t * t * t
                } else {
                    1.0 - f32_powi(-2.0 * t + 2.0, 3) / 2.0
                }
            }
            EasingFunction::EaseInQuad => t * t,
            EasingFunction::EaseOutQuad => t * (2.0 - t),
            EasingFunction::EaseInOutQuad => {
                if t < 0.5 {
                    2.0 * t * t
                } else {
                    -1.0 + (4.0 - 2.0 * t) * t
                }
            }
            EasingFunction::EaseInCubic => t * t * t,
            EasingFunction::EaseOutCubic => {
                let u = t - 1.0;
                u * u * u + 1.0
            }
            EasingFunction::EaseInOutCubic => {
                if t < 0.5 {
                    4.0 * t * t * t
                } else {
                    (t - 1.0) * (2.0 * t - 2.0) * (2.0 * t - 2.0) + 1.0
                }
            }
            EasingFunction::EaseInQuart => t * t * t * t,
            EasingFunction::EaseOutQuart => {
                let u = t - 1.0;
                1.0 - u * u * u * u
            }
            EasingFunction::EaseInOutQuart => {
                if t < 0.5 {
                    8.0 * t * t * t * t
                } else {
                    let u = t - 1.0;
                    1.0 - 8.0 * u * u * u * u
                }
            }
            EasingFunction::EaseInExpo => {
                if t <= 0.0 {
                    0.0
                } else {
                    f32_powf(2.0, 10.0 * (t - 1.0))
                }
            }
            EasingFunction::EaseOutExpo => {
                if t >= 1.0 {
                    1.0
                } else {
                    1.0 - f32_powf(2.0, -10.0 * t)
                }
            }
            EasingFunction::EaseInOutExpo => {
                if t <= 0.0 {
                    return 0.0;
                }
                if t >= 1.0 {
                    return 1.0;
                }
                if t < 0.5 {
                    f32_powf(2.0, 20.0 * t - 10.0) / 2.0
                } else {
                    (2.0 - f32_powf(2.0, -20.0 * t + 10.0)) / 2.0
                }
            }
            EasingFunction::EaseInBack => {
                let c1 = 1.70158;
                let c3 = c1 + 1.0;
                c3 * t * t * t - c1 * t * t
            }
            EasingFunction::EaseOutBack => Self::ease_out_back(t),
            EasingFunction::EaseInOutBack => {
                let c1 = 1.70158;
                let c2 = c1 * 1.525;
                if t < 0.5 {
                    f32_powi(2.0 * t, 2) * ((c2 + 1.0) * 2.0 * t - c2) / 2.0
                } else {
                    (f32_powi(2.0 * t - 2.0, 2) * ((c2 + 1.0) * (2.0 * t - 2.0) + c2) + 2.0) / 2.0
                }
            }
            EasingFunction::EaseInElastic => {
                if t <= 0.0 {
                    return 0.0;
                }
                if t >= 1.0 {
                    return 1.0;
                }
                let c4 = core::f32::consts::TAU / 3.0;
                -(f32_powf(2.0, 10.0 * t - 10.0) * f32_sin((10.0 * t - 10.75) * c4))
            }
            EasingFunction::EaseOutElastic => Self::ease_out_elastic(t),
            EasingFunction::EaseInOutElastic => {
                if t <= 0.0 {
                    return 0.0;
                }
                if t >= 1.0 {
                    return 1.0;
                }
                let c5 = core::f32::consts::TAU / 4.5;
                if t < 0.5 {
                    -(f32_powf(2.0, 20.0 * t - 10.0) * f32_sin((20.0 * t - 11.125) * c5)) / 2.0
                } else {
                    f32_powf(2.0, -20.0 * t + 10.0) * f32_sin((20.0 * t - 11.125) * c5) / 2.0 + 1.0
                }
            }
            EasingFunction::EaseInBounce => 1.0 - Self::ease_out_bounce(1.0 - t),
            EasingFunction::EaseOutBounce => Self::ease_out_bounce(t),
            EasingFunction::EaseInOutBounce => {
                if t < 0.5 {
                    (1.0 - Self::ease_out_bounce(1.0 - 2.0 * t)) / 2.0
                } else {
                    (1.0 + Self::ease_out_bounce(2.0 * t - 1.0)) / 2.0
                }
            }
            EasingFunction::CubicBezier(p1x, p1y, p2x, p2y) => {
                Self::cubic_bezier(t, *p1x, *p1y, *p2x, *p2y)
            }
            EasingFunction::Spring { stiffness, damping } => Self::spring(t, *stiffness, *damping),
            EasingFunction::Steps(steps, position) => {
                let n = *steps as f32;
                match position {
                    StepPosition::Start => (f32_ceil(t * n) / n).clamp(0.0, 1.0),
                    StepPosition::End => (f32_floor(t * n) / n).clamp(0.0, 1.0),
                }
            }
        }
    }

    fn ease_out_bounce(t: f32) -> f32 {
        let n1 = 7.5625;
        let d1 = 2.75;
        if t < 1.0 / d1 {
            n1 * t * t
        } else if t < 2.0 / d1 {
            let t2 = t - 1.5 / d1;
            n1 * t2 * t2 + 0.75
        } else if t < 2.5 / d1 {
            let t2 = t - 2.25 / d1;
            n1 * t2 * t2 + 0.9375
        } else {
            let t2 = t - 2.625 / d1;
            n1 * t2 * t2 + 0.984375
        }
    }

    fn ease_out_elastic(t: f32) -> f32 {
        if t <= 0.0 {
            return 0.0;
        }
        if t >= 1.0 {
            return 1.0;
        }
        let c4 = core::f32::consts::TAU / 3.0;
        f32_powf(2.0, -10.0 * t) * f32_sin((10.0 * t - 0.75) * c4) + 1.0
    }

    fn ease_out_back(t: f32) -> f32 {
        let c1 = 1.70158;
        let c3 = c1 + 1.0;
        let u = t - 1.0;
        1.0 + c3 * u * u * u + c1 * u * u
    }

    fn cubic_bezier(t: f32, p1x: f32, p1y: f32, p2x: f32, p2y: f32) -> f32 {
        let cx = 3.0 * p1x;
        let bx = 3.0 * (p2x - p1x) - cx;
        let ax = 1.0 - cx - bx;

        let cy = 3.0 * p1y;
        let by = 3.0 * (p2y - p1y) - cy;
        let ay = 1.0 - cy - by;

        // Newton-Raphson to solve for parameter given x = t
        let mut guess = t;
        for _ in 0..8 {
            let x_val = ((ax * guess + bx) * guess + cx) * guess;
            let dx = (3.0 * ax * guess + 2.0 * bx) * guess + cx;
            if dx.abs() < 1e-7 {
                break;
            }
            guess -= (x_val - t) / dx;
            guess = guess.clamp(0.0, 1.0);
        }

        ((ay * guess + by) * guess + cy) * guess
    }

    fn spring(t: f32, stiffness: f32, damping: f32) -> f32 {
        if t <= 0.0 {
            return 0.0;
        }
        if t >= 1.0 {
            return 1.0;
        }
        let omega = f32_sqrt(stiffness);
        let decay = f32_exp(-damping * t);
        1.0 - decay * f32_cos(omega * t)
    }

    fn lerp_color(from: u32, to: u32, t: f32) -> u32 {
        let fa = ((from >> 24) & 0xFF) as f32;
        let fr = ((from >> 16) & 0xFF) as f32;
        let fg = ((from >> 8) & 0xFF) as f32;
        let fb = (from & 0xFF) as f32;

        let ta = ((to >> 24) & 0xFF) as f32;
        let tr = ((to >> 16) & 0xFF) as f32;
        let tg = ((to >> 8) & 0xFF) as f32;
        let tb = (to & 0xFF) as f32;

        let a = (fa + (ta - fa) * t) as u32;
        let r = (fr + (tr - fr) * t) as u32;
        let g = (fg + (tg - fg) * t) as u32;
        let b = (fb + (tb - fb) * t) as u32;

        (a.min(255) << 24) | (r.min(255) << 16) | (g.min(255) << 8) | b.min(255)
    }
}

// ── Predefined animation presets ─────────────────────────────────────────

pub fn window_open_animation() -> Animation {
    Animation::new(
        AnimationTarget::Global,
        AnimationType::WindowOpen,
        250,
        EasingFunction::EaseOutBack,
    )
    .with_values(
        AnimationValue::Transform(Transform2D {
            scale_x: 0.8,
            scale_y: 0.8,
            opacity: 0.0,
            ..Transform2D::identity()
        }),
        AnimationValue::Transform(Transform2D::identity()),
    )
}

pub fn window_close_animation() -> Animation {
    Animation::new(
        AnimationTarget::Global,
        AnimationType::WindowClose,
        200,
        EasingFunction::EaseInCubic,
    )
    .with_values(
        AnimationValue::Transform(Transform2D::identity()),
        AnimationValue::Transform(Transform2D {
            scale_x: 0.9,
            scale_y: 0.9,
            opacity: 0.0,
            ..Transform2D::identity()
        }),
    )
}

pub fn window_minimize_animation() -> Animation {
    Animation::new(
        AnimationTarget::Global,
        AnimationType::WindowMinimize,
        300,
        EasingFunction::EaseInOutCubic,
    )
    .with_values(
        AnimationValue::Transform(Transform2D::identity()),
        AnimationValue::Transform(Transform2D {
            translate_y: 600.0,
            scale_x: 0.3,
            scale_y: 0.3,
            opacity: 0.0,
            ..Transform2D::identity()
        }),
    )
}

pub fn window_maximize_animation() -> Animation {
    Animation::new(
        AnimationTarget::Global,
        AnimationType::WindowMaximize,
        250,
        EasingFunction::EaseOutCubic,
    )
    .with_values(
        AnimationValue::Rect(100.0, 100.0, 800.0, 600.0),
        AnimationValue::Rect(0.0, 0.0, 1920.0, 1080.0),
    )
}

pub fn notification_enter_animation() -> Animation {
    Animation::new(
        AnimationTarget::Global,
        AnimationType::NotificationSlideIn,
        350,
        EasingFunction::EaseOutBack,
    )
    .with_values(
        AnimationValue::Transform(Transform2D {
            translate_x: 400.0,
            opacity: 0.0,
            ..Transform2D::identity()
        }),
        AnimationValue::Transform(Transform2D::identity()),
    )
}

pub fn notification_exit_animation() -> Animation {
    Animation::new(
        AnimationTarget::Global,
        AnimationType::NotificationSlideOut,
        250,
        EasingFunction::EaseInCubic,
    )
    .with_values(
        AnimationValue::Transform(Transform2D::identity()),
        AnimationValue::Transform(Transform2D {
            translate_x: 400.0,
            opacity: 0.0,
            ..Transform2D::identity()
        }),
    )
}

pub fn menu_open_animation() -> Animation {
    Animation::new(
        AnimationTarget::Menu,
        AnimationType::MenuOpen,
        200,
        EasingFunction::EaseOutQuart,
    )
    .with_values(
        AnimationValue::Transform(Transform2D {
            scale_y: 0.0,
            opacity: 0.0,
            origin_y: 0.0,
            ..Transform2D::identity()
        }),
        AnimationValue::Transform(Transform2D::identity()),
    )
}

pub fn desktop_switch_animation(direction: Direction) -> Animation {
    let (from_tx, to_tx, from_ty, to_ty) = match direction {
        Direction::Left => (0.0f32, 1920.0f32, 0.0f32, 0.0f32),
        Direction::Right => (0.0, -1920.0, 0.0, 0.0),
        Direction::Up => (0.0, 0.0, 0.0, 1080.0),
        Direction::Down => (0.0, 0.0, 0.0, -1080.0),
    };

    Animation::new(
        AnimationTarget::Global,
        AnimationType::DesktopSwitch,
        400,
        EasingFunction::EaseInOutCubic,
    )
    .with_values(
        AnimationValue::Transform(Transform2D {
            translate_x: from_tx,
            translate_y: from_ty,
            ..Transform2D::identity()
        }),
        AnimationValue::Transform(Transform2D {
            translate_x: to_tx,
            translate_y: to_ty,
            ..Transform2D::identity()
        }),
    )
}
