//! AthBody — embodiment and actuation for AthenaOS.
//!
//! Owns kinematics, motor command sketches, balance hooks, and the
//! software side of E-stop / safe-pose. Every command that reaches
//! hardware must pass AthGuard; this crate never bypasses caps.
#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::vec::Vec;

/// Joint identifier in the body schema (stable across sessions).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct JointId(pub u16);

/// Proposed joint command — units are implementation-defined until drivers land.
#[derive(Clone, Copy, Debug)]
pub struct JointCommand {
    pub joint: JointId,
    pub position: f32,
    pub velocity: f32,
    pub effort: f32,
}

/// Body-level command batch from AthMind (still subject to AthGuard).
#[derive(Clone, Debug, Default)]
pub struct BodyCommandBatch {
    pub joints: Vec<JointCommand>,
    pub estop_clear_requested: bool,
}

/// Software reflection of E-stop / safe state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BodySafetyState {
    Running,
    Frozen,
    EstopAsserted,
}

/// AthBody service surface (stub).
pub struct AthBody {
    pub state: BodySafetyState,
}

impl AthBody {
    pub const fn new() -> Self {
        Self {
            state: BodySafetyState::Frozen,
        }
    }

    pub fn assert_estop(&mut self) {
        self.state = BodySafetyState::EstopAsserted;
    }

    /// Accept a batch only when not E-stopped. Real impl will call drivers.
    pub fn submit(&mut self, batch: &BodyCommandBatch) -> Result<(), BodyError> {
        if self.state == BodySafetyState::EstopAsserted && !batch.estop_clear_requested {
            return Err(BodyError::EstopActive);
        }
        if batch.estop_clear_requested {
            self.state = BodySafetyState::Frozen;
        }
        let _ = &batch.joints;
        Ok(())
    }
}

impl Default for AthBody {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BodyError {
    EstopActive,
    GuardDenied,
    InvalidJoint,
}

pub fn mission() -> &'static str {
    "AthBody: kinematics, motors, balance, E-stop — actuation under AthGuard."
}
