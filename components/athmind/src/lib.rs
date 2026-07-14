//! AthMind — cognitive loop for AthenaOS.
//!
//! Implements the sense → update self → choose → act → remember tick.
//! LLM/tool proposals (via inherited `athai`) are optional and never
//! bypass AthGuard on the path to AthBody / AthVoice.
#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod affect;

use affect::{AffectEvent, AffectPacket, AffectState};
use alloc::string::String;
use alloc::vec::Vec;
use athbody::{AthBody, BodyCommandBatch};
use athsense::{AthSense, FusedPercept, SensorFrame};

#[derive(Clone, Debug, Default)]
pub struct SelfModel {
    pub identity: String,
    pub role: String,
}

#[derive(Clone, Debug)]
pub struct Goal {
    pub id: u64,
    pub description: String,
    pub priority: i32,
}

#[derive(Clone, Debug, Default)]
pub struct WorkingMemory {
    pub last_percept: Option<FusedPercept>,
    pub goals: Vec<Goal>,
}

/// One deliberative / control tick outcome.
#[derive(Clone, Debug, Default)]
pub struct TickResult {
    pub body: BodyCommandBatch,
    pub note: String,
}

pub struct AthMind {
    pub self_model: SelfModel,
    pub memory: WorkingMemory,
    pub sense: AthSense,
    pub body: AthBody,
    pub affect_state: AffectState,
    pending_affect: Vec<AffectEvent>,
}

impl AthMind {
    pub fn new(identity: String, role: String) -> Self {
        Self {
            self_model: SelfModel { identity, role },
            memory: WorkingMemory::default(),
            sense: AthSense::new(),
            body: AthBody::new(),
            affect_state: AffectState::default(),
            pending_affect: Vec::new(),
        }
    }

    /// Queue an affect event; it lands on the next tick's update-self stage.
    pub fn apply_affect_event(&mut self, ev: AffectEvent) {
        self.pending_affect.push(ev);
    }

    pub fn affect(&self) -> &AffectState {
        &self.affect_state
    }

    pub fn affect_packet_for_llm(&self) -> AffectPacket {
        self.affect_state.packet()
    }

    pub fn observe(&mut self, frame: SensorFrame) {
        let percept = self.sense.ingest(frame);
        self.memory.last_percept = Some(percept);
    }

    /// Stub planner: freezes body; real planner will emit guarded cmds.
    pub fn tick(&mut self) -> TickResult {
        // update self: decay + apply this tick's queued affect events.
        self.affect_state.tick(&self.pending_affect);
        self.pending_affect.clear();

        let note = match &self.memory.last_percept {
            Some(p) => alloc::format!("tick ok; {}", p.summary),
            None => String::from("tick ok; no percept yet"),
        };
        let batch = BodyCommandBatch::default();
        let _ = self.body.submit(&batch);
        TickResult {
            body: batch,
            note,
        }
    }
}

pub fn mission() -> &'static str {
    "AthMind: persistent self, memory, goals, and the Athena cognitive tick."
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::affect::{AffectEvent, AffectEventKind, AffectSource, Channel};

    fn mind() -> AthMind {
        AthMind::new(String::from("athena"), String::from("companion"))
    }

    #[test]
    fn events_apply_on_tick_not_on_queue() {
        let mut m = mind();
        m.apply_affect_event(AffectEvent {
            kind: AffectEventKind::GuardNearDeny,
            magnitude: 1.0,
            source: AffectSource::Guard,
        });
        assert_eq!(m.affect().get(Channel::Shame), 0.0, "queued, not yet applied");
        m.tick();
        assert!(m.affect().get(Channel::Shame) > 0.2, "applied during update-self");
    }

    #[test]
    fn queue_drains_once() {
        let mut m = mind();
        m.apply_affect_event(AffectEvent {
            kind: AffectEventKind::Novelty,
            magnitude: 1.0,
            source: AffectSource::Sense,
        });
        m.tick();
        let after_first = m.affect().get(Channel::Curiosity);
        m.tick();
        assert!(
            m.affect().get(Channel::Curiosity) < after_first,
            "second tick must only decay — event must not re-apply"
        );
    }

    #[test]
    fn packet_reflects_current_state() {
        let mut m = mind();
        m.apply_affect_event(AffectEvent {
            kind: AffectEventKind::OwnerPraise,
            magnitude: 1.0,
            source: AffectSource::Social,
        });
        m.tick();
        let p = m.affect_packet_for_llm();
        assert_eq!(p.warmth, m.affect().get(Channel::Warmth));
    }
}
