//! AthSense — perception bus for AthenaOS.
//!
//! Ingests camera, microphone, IMU, and tactile samples; publishes fused
//! percepts to AthMind. Privacy-sensitive streams are capability-gated
//! by AthGuard (mic/camera caps).
#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::vec::Vec;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SensorKind {
    Camera,
    Microphone,
    Imu,
    Tactile,
}

#[derive(Clone, Debug)]
pub struct SensorFrame {
    pub kind: SensorKind,
    pub timestamp_us: u64,
    pub payload: Vec<u8>,
}

#[derive(Clone, Debug, Default)]
pub struct FusedPercept {
    pub timestamp_us: u64,
    /// Placeholder pose / attention summary until real fusion lands.
    pub summary: alloc::string::String,
}

pub struct AthSense {
    pub frames_seen: u64,
}

impl AthSense {
    pub const fn new() -> Self {
        Self { frames_seen: 0 }
    }

    pub fn ingest(&mut self, frame: SensorFrame) -> FusedPercept {
        self.frames_seen = self.frames_seen.saturating_add(1);
        FusedPercept {
            timestamp_us: frame.timestamp_us,
            summary: alloc::format!("{:?} bytes={}", frame.kind, frame.payload.len()),
        }
    }
}

impl Default for AthSense {
    fn default() -> Self {
        Self::new()
    }
}

pub fn mission() -> &'static str {
    "AthSense: sensor ingest + fusion for the Athena perception bus."
}
