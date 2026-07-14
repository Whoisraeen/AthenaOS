//! AthUI Animation Engine — curve-editable animations for the AthenaOS UI.
//!
//! Provides timeline-based animations with extensive easing (cubic bezier,
//! spring physics, elastic, bounce), keyframe sequences, and an engine that
//! ticks all active animations each frame.

extern crate alloc;

use alloc::vec::Vec;
use libm::{cosf, expf, powf, sinf, sqrtf};

// ── Easing Functions ────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug)]
pub enum EasingFunction {
    Linear,
    EaseIn,
    EaseOut,
    EaseInOut,
    CubicBezier(f32, f32, f32, f32),
    Spring {
        stiffness: f32,
        damping: f32,
        mass: f32,
    },
    Bounce,
    Elastic,
}

fn cubic_bezier_sample(x1: f32, y1: f32, x2: f32, y2: f32, t: f32) -> f32 {
    // Attempt to find the t parameter for the given x, then evaluate y.
    // Use Newton-Raphson to solve for the parametric t that gives our x.
    let mut guess = t;
    for _ in 0..8 {
        let cx = 3.0 * x1;
        let bx = 3.0 * (x2 - x1) - cx;
        let ax = 1.0 - cx - bx;
        let x_at = ((ax * guess + bx) * guess + cx) * guess;
        let dx = (3.0 * ax * guess + 2.0 * bx) * guess + cx;
        if dx.abs() < 1e-6 {
            break;
        }
        guess -= (x_at - t) / dx;
        if guess < 0.0 {
            guess = 0.0;
        }
        if guess > 1.0 {
            guess = 1.0;
        }
    }
    let cy = 3.0 * y1;
    let by = 3.0 * (y2 - y1) - cy;
    let ay = 1.0 - cy - by;
    ((ay * guess + by) * guess + cy) * guess
}

fn bounce_out(t: f32) -> f32 {
    if t < 1.0 / 2.75 {
        7.5625 * t * t
    } else if t < 2.0 / 2.75 {
        let t2 = t - 1.5 / 2.75;
        7.5625 * t2 * t2 + 0.75
    } else if t < 2.5 / 2.75 {
        let t2 = t - 2.25 / 2.75;
        7.5625 * t2 * t2 + 0.9375
    } else {
        let t2 = t - 2.625 / 2.75;
        7.5625 * t2 * t2 + 0.984375
    }
}

fn elastic_out(t: f32) -> f32 {
    if t <= 0.0 {
        return 0.0;
    }
    if t >= 1.0 {
        return 1.0;
    }
    let p = 0.3;
    let s = p / 4.0;
    let val = powf(2.0, -10.0 * t);
    val * sinf((t - s) * (2.0 * core::f32::consts::PI) / p) + 1.0
}

/// Evaluate the easing function at normalized time t ∈ [0, 1].
/// For Spring easing, `t` represents the fraction of duration elapsed;
/// spring dynamics are approximated analytically.
pub fn evaluate_easing(easing: EasingFunction, t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    match easing {
        EasingFunction::Linear => t,
        EasingFunction::EaseIn => t * t * t,
        EasingFunction::EaseOut => {
            let inv = 1.0 - t;
            1.0 - inv * inv * inv
        }
        EasingFunction::EaseInOut => {
            if t < 0.5 {
                4.0 * t * t * t
            } else {
                let p = -2.0 * t + 2.0;
                1.0 - p * p * p / 2.0
            }
        }
        EasingFunction::CubicBezier(x1, y1, x2, y2) => cubic_bezier_sample(x1, y1, x2, y2, t),
        EasingFunction::Spring {
            stiffness,
            damping,
            mass,
        } => {
            // Damped harmonic oscillator: x(t) = 1 - e^(-ζωt)(cos(ωd*t) + ...)
            let omega = sqrtf(stiffness / mass);
            let zeta = damping / (2.0 * sqrtf(stiffness * mass));
            let damped_freq = omega * sqrtf((1.0 - zeta * zeta).max(0.0));
            // Map t to a time scale (use ~4x period for full settle)
            let time = t * 4.0 / (zeta * omega + 0.001);
            let decay = expf(-zeta * omega * time);
            if zeta < 1.0 {
                1.0 - decay
                    * (cosf(damped_freq * time)
                        + (zeta * omega / (damped_freq + 0.001)) * sinf(damped_freq * time))
            } else {
                1.0 - decay * (1.0 + (zeta * omega - omega) * time)
            }
        }
        EasingFunction::Bounce => bounce_out(t),
        EasingFunction::Elastic => elastic_out(t),
    }
}

// ── Animated Properties ─────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnimatedProperty {
    X,
    Y,
    Width,
    Height,
    Opacity,
    Rotation,
    Scale,
    CornerRadius,
    BorderWidth,
    ColorR,
    ColorG,
    ColorB,
    ColorA,
}

// ── Animation State ─────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnimationState {
    Running,
    Paused,
    Completed,
    Cancelled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RepeatMode {
    Once,
    Loop,
    PingPong,
    Count(u32),
}

// ── Animation ───────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct Animation {
    pub id: u32,
    pub widget_id: u32,
    pub property: AnimatedProperty,
    pub from: f32,
    pub to: f32,
    pub duration_ms: u32,
    pub elapsed_ms: u32,
    pub easing: EasingFunction,
    pub state: AnimationState,
    pub repeat: RepeatMode,
    pub on_complete: Option<u32>,
    pub delay_ms: u32,
    repeat_count: u32,
    ping_pong_forward: bool,
}

impl Animation {
    pub fn new(
        id: u32,
        widget_id: u32,
        property: AnimatedProperty,
        from: f32,
        to: f32,
        duration_ms: u32,
        easing: EasingFunction,
    ) -> Self {
        Self {
            id,
            widget_id,
            property,
            from,
            to,
            duration_ms,
            elapsed_ms: 0,
            easing,
            state: AnimationState::Running,
            repeat: RepeatMode::Once,
            on_complete: None,
            delay_ms: 0,
            repeat_count: 0,
            ping_pong_forward: true,
        }
    }

    pub fn current_value(&self) -> f32 {
        if self.state == AnimationState::Completed {
            return self.to;
        }
        if self.elapsed_ms < self.delay_ms {
            return self.from;
        }
        let active_elapsed = self.elapsed_ms - self.delay_ms;
        if self.duration_ms == 0 {
            return self.to;
        }
        let raw_t = active_elapsed as f32 / self.duration_ms as f32;
        let t = raw_t.clamp(0.0, 1.0);
        let eased = evaluate_easing(self.easing, t);

        let (a, b) = if !self.ping_pong_forward {
            (self.to, self.from)
        } else {
            (self.from, self.to)
        };
        a + (b - a) * eased
    }

    fn tick(&mut self, delta_ms: u32) {
        if self.state != AnimationState::Running {
            return;
        }
        self.elapsed_ms += delta_ms;
        let active_elapsed = self.elapsed_ms.saturating_sub(self.delay_ms);

        if active_elapsed >= self.duration_ms {
            match self.repeat {
                RepeatMode::Once => {
                    self.state = AnimationState::Completed;
                }
                RepeatMode::Loop => {
                    self.elapsed_ms = self.delay_ms + (active_elapsed % self.duration_ms);
                }
                RepeatMode::PingPong => {
                    self.ping_pong_forward = !self.ping_pong_forward;
                    self.elapsed_ms = self.delay_ms + (active_elapsed % self.duration_ms);
                }
                RepeatMode::Count(max) => {
                    self.repeat_count += 1;
                    if self.repeat_count >= max {
                        self.state = AnimationState::Completed;
                    } else {
                        self.elapsed_ms = self.delay_ms + (active_elapsed % self.duration_ms);
                    }
                }
            }
        }
    }
}

// ── Keyframe Sequence ───────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct Keyframe {
    pub value: f32,
    pub duration_ms: u32,
    pub easing: EasingFunction,
}

#[derive(Clone, Debug)]
pub struct KeyframeSequence {
    pub id: u32,
    pub widget_id: u32,
    pub property: AnimatedProperty,
    pub keyframes: Vec<Keyframe>,
    pub current_frame: usize,
    pub elapsed_in_frame: u32,
    pub state: AnimationState,
    pub on_complete: Option<u32>,
}

impl KeyframeSequence {
    pub fn new(
        id: u32,
        widget_id: u32,
        property: AnimatedProperty,
        keyframes: Vec<Keyframe>,
    ) -> Self {
        Self {
            id,
            widget_id,
            property,
            keyframes,
            current_frame: 0,
            elapsed_in_frame: 0,
            state: AnimationState::Running,
            on_complete: None,
        }
    }

    pub fn current_value(&self) -> f32 {
        if self.keyframes.is_empty() {
            return 0.0;
        }
        if self.state == AnimationState::Completed || self.current_frame >= self.keyframes.len() {
            return self.keyframes.last().map(|k| k.value).unwrap_or(0.0);
        }

        let from = if self.current_frame == 0 {
            self.keyframes[0].value
        } else {
            self.keyframes[self.current_frame - 1].value
        };
        let kf = &self.keyframes[self.current_frame];
        if kf.duration_ms == 0 {
            return kf.value;
        }
        let t = (self.elapsed_in_frame as f32 / kf.duration_ms as f32).clamp(0.0, 1.0);
        let eased = evaluate_easing(kf.easing, t);
        from + (kf.value - from) * eased
    }

    fn tick(&mut self, delta_ms: u32) {
        if self.state != AnimationState::Running {
            return;
        }
        if self.current_frame >= self.keyframes.len() {
            self.state = AnimationState::Completed;
            return;
        }
        self.elapsed_in_frame += delta_ms;
        while self.current_frame < self.keyframes.len() {
            let kf_dur = self.keyframes[self.current_frame].duration_ms;
            if self.elapsed_in_frame >= kf_dur {
                self.elapsed_in_frame -= kf_dur;
                self.current_frame += 1;
            } else {
                break;
            }
        }
        if self.current_frame >= self.keyframes.len() {
            self.state = AnimationState::Completed;
        }
    }
}

// ── Transition (automatic state change animation) ───────────────────────

#[derive(Clone, Debug)]
pub struct Transition {
    pub property: AnimatedProperty,
    pub duration_ms: u32,
    pub easing: EasingFunction,
    pub delay_ms: u32,
}

impl Transition {
    pub fn new(property: AnimatedProperty, duration_ms: u32, easing: EasingFunction) -> Self {
        Self {
            property,
            duration_ms,
            easing,
            delay_ms: 0,
        }
    }
}

// ── Property Update (output from the engine) ────────────────────────────

#[derive(Clone, Copy, Debug)]
pub struct PropertyUpdate {
    pub widget_id: u32,
    pub property: AnimatedProperty,
    pub value: f32,
}

// ── Animation Engine ────────────────────────────────────────────────────

pub struct AnimationEngine {
    animations: Vec<Animation>,
    sequences: Vec<KeyframeSequence>,
    next_id: u32,
    pub reduced_motion: bool,
    pending_callbacks: Vec<u32>,
}

impl AnimationEngine {
    pub fn new() -> Self {
        Self {
            animations: Vec::new(),
            sequences: Vec::new(),
            next_id: 1,
            reduced_motion: false,
            pending_callbacks: Vec::new(),
        }
    }

    pub fn animate(
        &mut self,
        widget_id: u32,
        property: AnimatedProperty,
        from: f32,
        to: f32,
        duration_ms: u32,
        easing: EasingFunction,
    ) -> u32 {
        if self.reduced_motion {
            // Skip animation, return a completed animation
            let id = self.alloc_id();
            let mut anim =
                Animation::new(id, widget_id, property, from, to, 0, EasingFunction::Linear);
            anim.state = AnimationState::Completed;
            self.animations.push(anim);
            return id;
        }
        let id = self.alloc_id();
        let anim = Animation::new(id, widget_id, property, from, to, duration_ms, easing);
        self.animations.push(anim);
        id
    }

    pub fn animate_with_delay(
        &mut self,
        widget_id: u32,
        property: AnimatedProperty,
        from: f32,
        to: f32,
        duration_ms: u32,
        delay_ms: u32,
        easing: EasingFunction,
    ) -> u32 {
        let id = self.animate(widget_id, property, from, to, duration_ms, easing);
        if let Some(anim) = self.animations.iter_mut().find(|a| a.id == id) {
            anim.delay_ms = delay_ms;
        }
        id
    }

    pub fn animate_sequence(
        &mut self,
        widget_id: u32,
        property: AnimatedProperty,
        keyframes: Vec<Keyframe>,
    ) -> u32 {
        let id = self.alloc_id();
        let seq = KeyframeSequence::new(id, widget_id, property, keyframes);
        self.sequences.push(seq);
        id
    }

    pub fn transition(
        &mut self,
        widget_id: u32,
        property: AnimatedProperty,
        current: f32,
        target: f32,
        trans: &Transition,
    ) -> u32 {
        // Cancel any running animation on this widget+property
        self.cancel_property(widget_id, property);
        self.animate_with_delay(
            widget_id,
            property,
            current,
            target,
            trans.duration_ms,
            trans.delay_ms,
            trans.easing,
        )
    }

    pub fn cancel(&mut self, animation_id: u32) {
        if let Some(anim) = self.animations.iter_mut().find(|a| a.id == animation_id) {
            anim.state = AnimationState::Cancelled;
        }
        if let Some(seq) = self.sequences.iter_mut().find(|s| s.id == animation_id) {
            seq.state = AnimationState::Cancelled;
        }
    }

    pub fn cancel_property(&mut self, widget_id: u32, property: AnimatedProperty) {
        for anim in &mut self.animations {
            if anim.widget_id == widget_id
                && anim.property == property
                && anim.state == AnimationState::Running
            {
                anim.state = AnimationState::Cancelled;
            }
        }
    }

    pub fn pause(&mut self, animation_id: u32) {
        if let Some(anim) = self.animations.iter_mut().find(|a| a.id == animation_id) {
            if anim.state == AnimationState::Running {
                anim.state = AnimationState::Paused;
            }
        }
        if let Some(seq) = self.sequences.iter_mut().find(|s| s.id == animation_id) {
            if seq.state == AnimationState::Running {
                seq.state = AnimationState::Paused;
            }
        }
    }

    pub fn resume(&mut self, animation_id: u32) {
        if let Some(anim) = self.animations.iter_mut().find(|a| a.id == animation_id) {
            if anim.state == AnimationState::Paused {
                anim.state = AnimationState::Running;
            }
        }
        if let Some(seq) = self.sequences.iter_mut().find(|s| s.id == animation_id) {
            if seq.state == AnimationState::Paused {
                seq.state = AnimationState::Running;
            }
        }
    }

    pub fn set_repeat(&mut self, animation_id: u32, repeat: RepeatMode) {
        if let Some(anim) = self.animations.iter_mut().find(|a| a.id == animation_id) {
            anim.repeat = repeat;
        }
    }

    pub fn set_on_complete(&mut self, animation_id: u32, callback_id: u32) {
        if let Some(anim) = self.animations.iter_mut().find(|a| a.id == animation_id) {
            anim.on_complete = Some(callback_id);
        }
        if let Some(seq) = self.sequences.iter_mut().find(|s| s.id == animation_id) {
            seq.on_complete = Some(callback_id);
        }
    }

    /// Tick all animations forward by `delta_ms`. Returns property updates to apply.
    pub fn tick(&mut self, delta_ms: u32) -> Vec<PropertyUpdate> {
        let mut updates = Vec::new();
        self.pending_callbacks.clear();

        for anim in &mut self.animations {
            let was_running = anim.state == AnimationState::Running;
            anim.tick(delta_ms);
            if anim.state == AnimationState::Running
                || (was_running && anim.state == AnimationState::Completed)
            {
                updates.push(PropertyUpdate {
                    widget_id: anim.widget_id,
                    property: anim.property,
                    value: anim.current_value(),
                });
            }
            if was_running && anim.state == AnimationState::Completed {
                if let Some(cb) = anim.on_complete {
                    self.pending_callbacks.push(cb);
                }
            }
        }

        for seq in &mut self.sequences {
            let was_running = seq.state == AnimationState::Running;
            seq.tick(delta_ms);
            if seq.state == AnimationState::Running
                || (was_running && seq.state == AnimationState::Completed)
            {
                updates.push(PropertyUpdate {
                    widget_id: seq.widget_id,
                    property: seq.property,
                    value: seq.current_value(),
                });
            }
            if was_running && seq.state == AnimationState::Completed {
                if let Some(cb) = seq.on_complete {
                    self.pending_callbacks.push(cb);
                }
            }
        }

        // Garbage collect completed/cancelled animations
        self.animations
            .retain(|a| a.state == AnimationState::Running || a.state == AnimationState::Paused);
        self.sequences
            .retain(|s| s.state == AnimationState::Running || s.state == AnimationState::Paused);

        updates
    }

    pub fn drain_callbacks(&mut self) -> Vec<u32> {
        core::mem::take(&mut self.pending_callbacks)
    }

    pub fn active_count(&self) -> usize {
        self.animations
            .iter()
            .filter(|a| a.state == AnimationState::Running)
            .count()
            + self
                .sequences
                .iter()
                .filter(|s| s.state == AnimationState::Running)
                .count()
    }

    pub fn is_animating(&self, widget_id: u32) -> bool {
        self.animations
            .iter()
            .any(|a| a.widget_id == widget_id && a.state == AnimationState::Running)
            || self
                .sequences
                .iter()
                .any(|s| s.widget_id == widget_id && s.state == AnimationState::Running)
    }

    fn alloc_id(&mut self) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }
}

// ── Host KATs (dev box, `cargo test -p athui`) ──────────────────────────
// MasterChecklist Phase 8: "Window animations curve-editable". These FAIL-ably
// prove the editable easing-curve primitive (the user-tunable part) and the
// engine tick that drives window/widget animations. Pure logic, no GPU.
#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f32, b: f32, eps: f32) -> bool {
        (a - b).abs() <= eps
    }

    #[test]
    fn easing_curves_anchor_at_endpoints() {
        // Every editable curve MUST pin (0->0) and (1->1) or animations would
        // visibly jump at start/end. A broken curve fails here, not on screen.
        let curves = [
            EasingFunction::Linear,
            EasingFunction::EaseIn,
            EasingFunction::EaseOut,
            EasingFunction::EaseInOut,
            EasingFunction::CubicBezier(0.25, 0.1, 0.25, 1.0), // CSS "ease"
            EasingFunction::Bounce,
            EasingFunction::Elastic,
        ];
        for c in curves {
            assert!(
                approx(evaluate_easing(c, 0.0), 0.0, 0.02),
                "{:?}(0) != 0",
                c
            );
            assert!(
                approx(evaluate_easing(c, 1.0), 1.0, 0.02),
                "{:?}(1) != 1",
                c
            );
        }
        // Spring settles toward 1 from 0 (approximate analytic model); just
        // assert the start anchor and that the model never returns NaN/inf.
        let spring = EasingFunction::Spring {
            stiffness: 100.0,
            damping: 10.0,
            mass: 1.0,
        };
        assert!(approx(evaluate_easing(spring, 0.0), 0.0, 0.02));
        assert!(evaluate_easing(spring, 1.0).is_finite());
        // Out-of-range t is clamped, never NaN/inf (a slow frame can overshoot).
        assert_eq!(evaluate_easing(EasingFunction::Linear, -1.0), 0.0);
        assert_eq!(evaluate_easing(EasingFunction::Linear, 2.0), 1.0);
    }

    #[test]
    fn easing_basic_curves_monotonic_and_shaped() {
        // Linear is the identity.
        for i in 0..=10 {
            let t = i as f32 / 10.0;
            assert!(approx(evaluate_easing(EasingFunction::Linear, t), t, 1e-4));
        }
        // EaseInOut is symmetric about its midpoint.
        assert!(approx(
            evaluate_easing(EasingFunction::EaseInOut, 0.5),
            0.5,
            0.001
        ));
        // The four standard curves never backtrack (non-decreasing).
        for c in [
            EasingFunction::Linear,
            EasingFunction::EaseIn,
            EasingFunction::EaseOut,
            EasingFunction::EaseInOut,
        ] {
            let mut prev = -1.0;
            for i in 0..=20 {
                let v = evaluate_easing(c, i as f32 / 20.0);
                assert!(v >= prev - 1e-4, "{:?} not monotonic at step {}", c, i);
                prev = v;
            }
        }
        // The defining shape: EaseIn starts slow, EaseOut starts fast.
        assert!(evaluate_easing(EasingFunction::EaseIn, 0.25) < 0.25);
        assert!(evaluate_easing(EasingFunction::EaseOut, 0.25) > 0.25);
    }

    #[test]
    fn animation_ticks_to_target_and_completes() {
        let mut a = Animation::new(
            1,
            9,
            AnimatedProperty::Opacity,
            0.0,
            100.0,
            100,
            EasingFunction::Linear,
        );
        assert_eq!(a.current_value(), 0.0);
        a.tick(50);
        let mid = a.current_value();
        assert!(mid > 40.0 && mid < 60.0, "linear midpoint was {}", mid);
        a.tick(50);
        assert_eq!(a.state, AnimationState::Completed);
        assert_eq!(a.current_value(), 100.0);
    }

    #[test]
    fn engine_reduced_motion_makes_animation_instantly_complete() {
        let mut e = AnimationEngine::new();
        e.reduced_motion = true;
        let _ = e.animate(
            3,
            AnimatedProperty::X,
            0.0,
            200.0,
            300,
            EasingFunction::EaseInOut,
        );
        // Reduced-motion (a11y): born Completed, never counts as active motion.
        assert_eq!(e.active_count(), 0);
        let _ = e.tick(16); // GCs the completed no-op
        assert_eq!(e.active_count(), 0);
    }

    #[test]
    fn engine_tick_emits_updates_then_gcs_completed() {
        let mut e = AnimationEngine::new();
        e.animate(
            5,
            AnimatedProperty::Y,
            0.0,
            10.0,
            100,
            EasingFunction::Linear,
        );
        assert_eq!(e.active_count(), 1);
        let u1 = e.tick(50);
        assert!(
            u1.iter().any(|u| u.widget_id == 5),
            "no update for widget 5"
        );
        assert_eq!(e.active_count(), 1); // still running mid-flight
        let _ = e.tick(60); // crosses the 100ms duration -> Completed -> GC'd
        assert_eq!(e.active_count(), 0);
    }

    #[test]
    fn keyframe_sequence_interpolates_through_frames() {
        let kfs = alloc::vec![
            Keyframe {
                value: 100.0,
                duration_ms: 100,
                easing: EasingFunction::Linear
            },
            Keyframe {
                value: 0.0,
                duration_ms: 100,
                easing: EasingFunction::Linear
            },
        ];
        let mut seq = KeyframeSequence::new(1, 7, AnimatedProperty::Height, kfs);
        // Frame 0 holds the first keyframe's value.
        assert!(approx(seq.current_value(), 100.0, 0.01));
        seq.tick(100); // advance into frame 1 (interpolates kf0 -> kf1)
        assert!(approx(seq.current_value(), 100.0, 0.01));
        seq.tick(50); // halfway through frame 1: 100 -> 0
        assert!(approx(seq.current_value(), 50.0, 1.0));
        seq.tick(50); // frame 1 done -> sequence Completed at the last value
        assert_eq!(seq.state, AnimationState::Completed);
        assert!(approx(seq.current_value(), 0.0, 0.01));
    }
}

// ── Curve editor model (MasterChecklist L1585) ───────────────────────────
// The data model behind the user-facing "edit window animation curves" UI:
// a list of selectable presets + a custom cubic-bezier the user tweaks, which
// resolves to the EasingFunction the compositor/AthUI feeds the AnimationEngine.
// Pure logic (clamping + preset->curve mapping); the rendered editor sits on top.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CurvePreset {
    Linear,
    EaseIn,
    EaseOut,
    EaseInOut,
    Bounce,
    Elastic,
    Custom,
}

pub struct CurveEditor {
    selected: CurvePreset,
    bezier: (f32, f32, f32, f32),
}

impl Default for CurveEditor {
    fn default() -> Self {
        Self::new()
    }
}

impl CurveEditor {
    /// Defaults to the CSS "ease" curve (EaseInOut), with the standard
    /// `cubic-bezier(0.25, 0.1, 0.25, 1.0)` staged if the user switches to Custom.
    pub fn new() -> Self {
        Self {
            selected: CurvePreset::EaseInOut,
            bezier: (0.25, 0.1, 0.25, 1.0),
        }
    }

    /// The presets the UI offers, in display order (includes Custom last).
    pub fn presets() -> &'static [CurvePreset] {
        &[
            CurvePreset::Linear,
            CurvePreset::EaseIn,
            CurvePreset::EaseOut,
            CurvePreset::EaseInOut,
            CurvePreset::Bounce,
            CurvePreset::Elastic,
            CurvePreset::Custom,
        ]
    }

    pub fn selected(&self) -> CurvePreset {
        self.selected
    }

    pub fn select(&mut self, preset: CurvePreset) {
        self.selected = preset;
    }

    pub fn bezier(&self) -> (f32, f32, f32, f32) {
        self.bezier
    }

    /// Set the custom cubic-bezier control points and switch to Custom. The X
    /// coordinates are clamped to `[0,1]` (an easing bezier must be a function
    /// of time — X outside `[0,1]` is non-monotonic/invalid); Y is left free so
    /// curves can overshoot (anticipation / overshoot easing).
    pub fn set_bezier(&mut self, x1: f32, y1: f32, x2: f32, y2: f32) {
        self.bezier = (x1.clamp(0.0, 1.0), y1, x2.clamp(0.0, 1.0), y2);
        self.selected = CurvePreset::Custom;
    }

    /// Resolve the current selection to the EasingFunction the engine consumes.
    pub fn easing(&self) -> EasingFunction {
        match self.selected {
            CurvePreset::Linear => EasingFunction::Linear,
            CurvePreset::EaseIn => EasingFunction::EaseIn,
            CurvePreset::EaseOut => EasingFunction::EaseOut,
            CurvePreset::EaseInOut => EasingFunction::EaseInOut,
            CurvePreset::Bounce => EasingFunction::Bounce,
            CurvePreset::Elastic => EasingFunction::Elastic,
            CurvePreset::Custom => {
                let (x1, y1, x2, y2) = self.bezier;
                EasingFunction::CubicBezier(x1, y1, x2, y2)
            }
        }
    }
}

#[cfg(test)]
mod curve_editor_kat {
    use super::*;

    #[test]
    fn preset_selection_maps_to_easing() {
        let mut ce = CurveEditor::new();
        assert_eq!(ce.selected(), CurvePreset::EaseInOut); // CSS "ease" default
        assert!(matches!(ce.easing(), EasingFunction::EaseInOut));
        ce.select(CurvePreset::Bounce);
        assert!(matches!(ce.easing(), EasingFunction::Bounce));
        ce.select(CurvePreset::Linear);
        assert!(matches!(ce.easing(), EasingFunction::Linear));
        assert_eq!(CurveEditor::presets().len(), 7);
        assert!(CurveEditor::presets().contains(&CurvePreset::Custom));
    }

    #[test]
    fn custom_bezier_clamps_x_keeps_y_and_still_anchors() {
        let mut ce = CurveEditor::new();
        // Out-of-range X clamps to [0,1]; Y (overshoot) is preserved.
        ce.set_bezier(2.0, 1.4, -0.5, -0.3);
        assert_eq!(ce.selected(), CurvePreset::Custom);
        assert_eq!(ce.bezier(), (1.0, 1.4, 0.0, -0.3));
        match ce.easing() {
            EasingFunction::CubicBezier(x1, y1, x2, y2) => {
                assert_eq!((x1, y1, x2, y2), (1.0, 1.4, 0.0, -0.3));
                // The resulting editable curve still pins (0->0, 1->1).
                assert!(evaluate_easing(ce.easing(), 0.0).abs() < 0.02);
                assert!((evaluate_easing(ce.easing(), 1.0) - 1.0).abs() < 0.02);
            }
            other => panic!("expected CubicBezier, got {:?}", other),
        }
    }
}
