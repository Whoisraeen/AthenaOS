//! AthVoice — speech and social presence for AthenaOS.
//!
//! Text-to-speech, speech-to-text hooks, and presence cues. Recording
//! and cloud offload require AthGuard consent/capability flags.
#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::string::String;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VoiceConsent {
    Denied,
    LocalOnly,
    CloudAllowed,
}

pub struct AthVoice {
    pub consent: VoiceConsent,
    pub last_utterance: Option<String>,
}

impl AthVoice {
    pub const fn new() -> Self {
        Self {
            consent: VoiceConsent::Denied,
            last_utterance: None,
        }
    }

    pub fn say(&mut self, text: String) -> Result<(), VoiceError> {
        if self.consent == VoiceConsent::Denied {
            return Err(VoiceError::ConsentDenied);
        }
        self.last_utterance = Some(text);
        Ok(())
    }
}

impl Default for AthVoice {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VoiceError {
    ConsentDenied,
    GuardDenied,
}

pub fn mission() -> &'static str {
    "AthVoice: speech I/O and social presence under consent + AthGuard."
}
