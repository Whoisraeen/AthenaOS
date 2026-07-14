//! Affect engine (Layer A) — durable emotional state for AthMind.
//!
//! Spec: docs/superpowers/specs/2026-07-14-athena-affect-arc-design.md §4.
//! Update law each tick: `affect[c] = clamp01(affect[c] * decay[c] + Σ events[c] * gain[c])`.
//! Affect biases priorities and presence only; it never raises actuator,
//! network, or tool capabilities (spec §3) — nothing here touches AthGuard.

use alloc::format;
use alloc::string::String;

/// Channel order is the array layout AND the dump-line order. Keep in sync
/// with `DECAY`, `ALL_CHANNELS`, and `dump_line`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Channel {
    Stress,
    Trust,
    Attachment,
    Warmth,
    Resolve,
    Shame,
    Curiosity,
    Fatigue,
}

pub const CHANNEL_COUNT: usize = 8;

const ALL_CHANNELS: [Channel; CHANNEL_COUNT] = [
    Channel::Stress,
    Channel::Trust,
    Channel::Attachment,
    Channel::Warmth,
    Channel::Resolve,
    Channel::Shame,
    Channel::Curiosity,
    Channel::Fatigue,
];

/// Per-tick retention multipliers (spec §4.2 "per-channel half-life").
/// v1 tuning defaults — spec §11 marks exact values as open; the invariant
/// tests rely on is only ORDER: stress decays fastest, attachment/fatigue
/// slowest.
const DECAY: [f32; CHANNEL_COUNT] = [
    0.98,   // stress    — fast: acute threat fades in ~35 ticks to half
    0.999,  // trust     — slow: confidence erodes only over long horizons
    0.9995, // attachment— slowest social channel
    0.995,  // warmth
    0.995,  // resolve
    0.99,   // shame
    0.99,   // curiosity
    0.9999, // fatigue   — near-integrator: relieved by Rest events, not time
];

/// Anti-oscillation cap (spec §4.2 "saturation"): the summed event delta a
/// single tick may apply to one channel. Keeps one loud event (or a burst)
/// from slamming a channel 0 → 1 with no cooldown.
const MAX_TICK_DELTA: f32 = 0.5;

/// Where an event originated (spec §4.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AffectSource {
    Sense,
    Social,
    Guard,
    Arc,
    Homeostasis,
}

/// v1 event vocabulary, one per "typical driver" row in spec §4.1.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AffectEventKind {
    LoudConflict,
    TaskFailure,
    TaskSuccess,
    GuardNearDeny,
    OwnerPraise,
    OwnerReassurance,
    Novelty,
    Rest,
    UptimeLoad,
}

impl AffectEventKind {
    /// Base per-channel gains for this event at magnitude 1.0. Negative
    /// gains lower a channel (e.g. reassurance discharges stress).
    fn gains(self) -> &'static [(Channel, f32)] {
        use AffectEventKind::*;
        use Channel::*;
        match self {
            LoudConflict => &[(Stress, 0.30), (Warmth, -0.10)],
            TaskFailure => &[(Stress, 0.20), (Resolve, -0.15)],
            TaskSuccess => &[(Resolve, 0.20), (Warmth, 0.05), (Stress, -0.05)],
            GuardNearDeny => &[(Shame, 0.35), (Stress, 0.20)],
            OwnerPraise => &[(Warmth, 0.25), (Trust, 0.10), (Attachment, 0.05)],
            OwnerReassurance => &[(Stress, -0.25), (Trust, 0.15)],
            Novelty => &[(Curiosity, 0.30)],
            Rest => &[(Fatigue, -0.40), (Stress, -0.10)],
            UptimeLoad => &[(Fatigue, 0.05)],
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct AffectEvent {
    pub kind: AffectEventKind,
    /// Scales the kind's base gains. Clamped to [0, 1] on apply.
    pub magnitude: f32,
    pub source: AffectSource,
}

/// Durable affect vector. All channels in [0, 1]. Default = calm zeros.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AffectState {
    channels: [f32; CHANNEL_COUNT],
}

impl AffectState {
    pub fn get(&self, c: Channel) -> f32 {
        self.channels[c as usize]
    }

    /// One update-law step (spec §4.2): decay every channel, then apply the
    /// tick's summed event deltas, capped per channel and clamped to [0, 1].
    pub fn tick(&mut self, events: &[AffectEvent]) {
        let mut delta = [0.0f32; CHANNEL_COUNT];
        for ev in events {
            let m = ev.magnitude.clamp(0.0, 1.0);
            for &(c, gain) in ev.kind.gains() {
                delta[c as usize] += gain * m;
            }
        }
        for i in 0..CHANNEL_COUNT {
            let d = delta[i].clamp(-MAX_TICK_DELTA, MAX_TICK_DELTA);
            self.channels[i] = (self.channels[i] * DECAY[i] + d).clamp(0.0, 1.0);
        }
    }

    /// Derived signed mood for UI (spec §4.1) — not independently
    /// authoritative; never read it back into the update law.
    pub fn valence(&self) -> f32 {
        use Channel::*;
        let pos = (self.get(Warmth) + self.get(Trust) + self.get(Resolve)) / 3.0;
        let neg = (self.get(Stress) + self.get(Shame) + self.get(Fatigue)) / 3.0;
        (pos - neg).clamp(-1.0, 1.0)
    }

    /// Canonical serial/proc dump (spec §4.4, §5.3). Grep-stable prefix:
    /// `[affect] stress=`.
    pub fn dump_line(&self) -> String {
        use Channel::*;
        format!(
            "[affect] stress={:.2} trust={:.2} attachment={:.2} warmth={:.2} resolve={:.2} shame={:.2} curiosity={:.2} fatigue={:.2} valence={:+.2}",
            self.get(Stress),
            self.get(Trust),
            self.get(Attachment),
            self.get(Warmth),
            self.get(Resolve),
            self.get(Shame),
            self.get(Curiosity),
            self.get(Fatigue),
            self.valence(),
        )
    }
}

/// Serializable snapshot handed to the LLM preamble and presence layer
/// (spec §4.4, §5.2). Plain named fields so consumers never depend on
/// channel array layout.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AffectPacket {
    pub stress: f32,
    pub trust: f32,
    pub attachment: f32,
    pub warmth: f32,
    pub resolve: f32,
    pub shame: f32,
    pub curiosity: f32,
    pub fatigue: f32,
    pub valence: f32,
}

impl AffectState {
    pub fn packet(&self) -> AffectPacket {
        use Channel::*;
        AffectPacket {
            stress: self.get(Stress),
            trust: self.get(Trust),
            attachment: self.get(Attachment),
            warmth: self.get(Warmth),
            resolve: self.get(Resolve),
            shame: self.get(Shame),
            curiosity: self.get(Curiosity),
            fatigue: self.get(Fatigue),
            valence: self.valence(),
        }
    }
}

/// Pure-compute P1 proof (spec §8): spike affect with a Guard near-deny,
/// verify the update law's rise / decay / bounds invariants, and return the
/// canonical `[affect]` dump line. Safe for the kernel's deferred boot
/// self-test sweep (ADR 0006): no I/O, no init, allocates only the line.
pub fn run_smoketest() -> (bool, String) {
    let mut a = AffectState::default();
    a.tick(&[AffectEvent {
        kind: AffectEventKind::GuardNearDeny,
        magnitude: 1.0,
        source: AffectSource::Guard,
    }]);
    let peak_stress = a.get(Channel::Stress);
    let peak_shame = a.get(Channel::Shame);
    for _ in 0..50 {
        a.tick(&[]);
    }
    let mut in_bounds = true;
    for c in ALL_CHANNELS {
        let v = a.get(c);
        if !(0.0..=1.0).contains(&v) {
            in_bounds = false;
        }
    }
    let pass = peak_stress > 0.1
        && peak_shame > 0.2
        && a.get(Channel::Stress) < peak_stress
        && a.get(Channel::Shame) < peak_shame
        && in_bounds;
    (pass, a.dump_line())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(kind: AffectEventKind, magnitude: f32) -> AffectEvent {
        AffectEvent {
            kind,
            magnitude,
            source: AffectSource::Social,
        }
    }

    #[test]
    fn default_is_calm_zero() {
        let a = AffectState::default();
        for c in ALL_CHANNELS {
            assert_eq!(a.get(c), 0.0, "{c:?} should start at 0");
        }
        assert_eq!(a.valence(), 0.0);
    }

    #[test]
    fn guard_near_deny_raises_shame_and_stress() {
        let mut a = AffectState::default();
        a.tick(&[ev(AffectEventKind::GuardNearDeny, 1.0)]);
        assert!(a.get(Channel::Shame) > 0.2);
        assert!(a.get(Channel::Stress) > 0.1);
        assert_eq!(
            a.get(Channel::Curiosity),
            0.0,
            "unrelated channel untouched"
        );
    }

    #[test]
    fn stress_decays_faster_than_attachment() {
        let mut a = AffectState::default();
        // Drive both channels to comparable levels.
        a.tick(&[
            ev(AffectEventKind::LoudConflict, 1.0),
            ev(AffectEventKind::OwnerPraise, 1.0),
        ]);
        let s0 = a.get(Channel::Stress);
        let at0 = a.get(Channel::Attachment);
        assert!(s0 > 0.0 && at0 > 0.0);
        for _ in 0..200 {
            a.tick(&[]);
        }
        let stress_kept = a.get(Channel::Stress) / s0;
        let attach_kept = a.get(Channel::Attachment) / at0;
        assert!(
            stress_kept < attach_kept,
            "stress must decay faster: kept {stress_kept} vs attachment {attach_kept}"
        );
    }

    #[test]
    fn channels_never_leave_unit_interval() {
        let mut a = AffectState::default();
        for _ in 0..100 {
            a.tick(&[
                ev(AffectEventKind::LoudConflict, 1.0),
                ev(AffectEventKind::GuardNearDeny, 1.0),
                ev(AffectEventKind::Rest, 1.0),
            ]);
            for c in ALL_CHANNELS {
                let v = a.get(c);
                assert!((0.0..=1.0).contains(&v), "{c:?}={v} out of range");
            }
        }
    }

    #[test]
    fn magnitude_is_clamped_to_unit() {
        let mut huge = AffectState::default();
        huge.tick(&[ev(AffectEventKind::Novelty, 50.0)]);
        let mut unit = AffectState::default();
        unit.tick(&[ev(AffectEventKind::Novelty, 1.0)]);
        assert_eq!(huge.get(Channel::Curiosity), unit.get(Channel::Curiosity));
    }

    #[test]
    fn single_tick_delta_is_capped() {
        let mut a = AffectState::default();
        // Five max-magnitude conflicts in one tick: raw stress delta 1.5.
        let burst = [ev(AffectEventKind::LoudConflict, 1.0); 5];
        a.tick(&burst);
        assert!(
            a.get(Channel::Stress) <= MAX_TICK_DELTA,
            "anti-oscillation cap breached: {}",
            a.get(Channel::Stress)
        );
    }

    #[test]
    fn valence_negative_under_stress_and_bounded() {
        let mut a = AffectState::default();
        for _ in 0..10 {
            a.tick(&[ev(AffectEventKind::GuardNearDeny, 1.0)]);
        }
        assert!(a.valence() < 0.0);
        assert!((-1.0..=1.0).contains(&a.valence()));
    }

    #[test]
    fn dump_line_has_grep_stable_prefix() {
        let a = AffectState::default();
        let line = a.dump_line();
        assert!(line.starts_with("[affect] stress="), "got: {line}");
        assert!(line.contains(" trust="));
        assert!(line.contains(" fatigue="));
    }

    #[test]
    fn packet_snapshots_channels_and_valence() {
        let mut a = AffectState::default();
        a.tick(&[ev(AffectEventKind::OwnerPraise, 1.0)]);
        let p = a.packet();
        assert_eq!(p.warmth, a.get(Channel::Warmth));
        assert_eq!(p.trust, a.get(Channel::Trust));
        assert_eq!(p.valence, a.valence());
    }

    #[test]
    fn smoketest_passes_and_emits_affect_line() {
        let (pass, line) = run_smoketest();
        assert!(pass, "smoketest failed: {line}");
        assert!(line.starts_with("[affect] stress="), "got: {line}");
    }
}
