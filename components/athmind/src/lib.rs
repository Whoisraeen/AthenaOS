//! AthMind — cognitive loop for AthenaOS.
//!
//! Implements the sense → update self → choose → act → remember tick.
//! LLM/tool proposals (via inherited `athai`) are optional and never
//! bypass AthGuard on the path to AthBody / AthVoice.
#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod affect;

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
}

impl AthMind {
    pub fn new(identity: String, role: String) -> Self {
        Self {
            self_model: SelfModel { identity, role },
            memory: WorkingMemory::default(),
            sense: AthSense::new(),
            body: AthBody::new(),
        }
    }

    pub fn observe(&mut self, frame: SensorFrame) {
        let percept = self.sense.ingest(frame);
        self.memory.last_percept = Some(percept);
    }

    /// Stub planner: freezes body; real planner will emit guarded cmds.
    pub fn tick(&mut self) -> TickResult {
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
