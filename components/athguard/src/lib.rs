//! AthGuard — safety and capability face of AthenaOS.
//!
//! Dominates AthBody / AthMind / AthVoice. Policy cannot be silently
//! rewritten by the cognitive loop. Inherited AthGuard (`athshield`)
//! remains the deep capability engine until the rename pass merges them.
#![cfg_attr(not(test), no_std)]

extern crate alloc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GuardDecision {
    Allow,
    Deny,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActuatorClass {
    JointMotor,
    Gripper,
    Locomotion,
    Speech,
    NetworkTool,
}

#[derive(Clone, Debug)]
pub struct GuardRequest {
    pub actuator: ActuatorClass,
    pub estop_latched: bool,
    pub has_capability: bool,
}

pub struct AthGuard {
    pub policy_generation: u64,
}

impl AthGuard {
    pub const fn new() -> Self {
        Self {
            policy_generation: 1,
        }
    }

    pub fn decide(&self, req: &GuardRequest) -> GuardDecision {
        if req.estop_latched {
            return GuardDecision::Deny;
        }
        if !req.has_capability {
            return GuardDecision::Deny;
        }
        GuardDecision::Allow
    }

    /// Owner-attested policy bump only — AthMind must not call this casually.
    pub fn bump_policy_attested(&mut self) {
        self.policy_generation = self.policy_generation.saturating_add(1);
    }
}

impl Default for AthGuard {
    fn default() -> Self {
        Self::new()
    }
}

pub fn mission() -> &'static str {
    "AthGuard: E-stop, capabilities, consent, attestation — safety dominates goals."
}
