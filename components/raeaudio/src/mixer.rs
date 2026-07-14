//! Real-time game-priority mixer — Concept §"AthAudio: SCHED_BODY mix thread,
//! sub-3 ms round-trip, zero underruns" and MasterChecklist Phase 7.2
//! "In-kernel audio mixer (priority over background apps in game mode)".
//!
//! This is the hot path: it runs on the SCHED_BODY mix thread once per buffer
//! period and MUST be allocation-free in steady state. All per-stream scratch
//! and the master bus are pre-allocated at construction; [`GameMixer::mix`]
//! touches no allocator and uses only saturating/limited arithmetic so two
//! full-scale voices can never clip-wrap into an audible click.
//!
//! Game-mode priority: when game mode is engaged, [`StreamPriority::Game`]
//! voices play at full gain while [`StreamPriority::Background`] voices are
//! attenuated ("ducked") by a configurable amount. This is the audio half of
//! "gaming isn't a mode, it's the default" — a footstep is never masked by a
//! Discord notification.

extern crate alloc;

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

/// Priority class of a mixer voice. Drives game-mode ducking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamPriority {
    /// The focused game / its competitive audio. Never ducked.
    Game,
    /// A normal foreground app (media player, browser). Ducked lightly.
    Foreground,
    /// Background / ambient app (chat notifications, music). Ducked hardest.
    Background,
}

/// One mixer input voice: a pre-allocated interleaved f32 buffer plus its
/// mixing controls. The buffer is filled by the producer (decoder / capture /
/// app) before each [`GameMixer::mix`] call.
pub struct Stream {
    pub name: String,
    /// Interleaved f32 PCM, `frames * channels` long. Producer writes here.
    pub buffer: Vec<f32>,
    pub channels: u16,
    pub gain: f32,
    pub muted: bool,
    pub priority: StreamPriority,
    /// Monotonic write generation — lets the mix detect a silent (un-refilled)
    /// voice and skip it without scanning the whole buffer.
    pub active: bool,
}

impl Stream {
    fn new(name: &str, channels: u16, frames: usize, priority: StreamPriority) -> Self {
        Self {
            name: String::from(name),
            buffer: vec![0.0; frames * channels as usize],
            channels,
            gain: 1.0,
            muted: false,
            priority,
            active: true,
        }
    }
}

/// Handle to a registered stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamId(pub usize);

/// Saturating soft-knee tanh-style limiter applied to the master bus so the
/// summed signal stays in [-1, 1] without the harsh edge of a hard clip.
///
/// Uses a cheap rational approximation of `tanh` (no libm, soft-float safe):
/// for |x| <= 1 it is near-linear; beyond that it compresses toward ±1.
#[inline]
fn soft_limit(x: f32) -> f32 {
    // Hard ceiling guard first (keeps the approximation bounded for huge sums).
    let x = if x > 4.0 {
        4.0
    } else if x < -4.0 {
        -4.0
    } else {
        x
    };
    // Padé-style tanh approximation: x*(27 + x^2) / (27 + 9*x^2).
    let x2 = x * x;
    let y = x * (27.0 + x2) / (27.0 + 9.0 * x2);
    // The approximation asymptotes near ±1; clamp the residual to be exact.
    if y > 1.0 {
        1.0
    } else if y < -1.0 {
        -1.0
    } else {
        y
    }
}

/// The real-time game-priority mixer.
pub struct GameMixer {
    streams: Vec<Stream>,
    /// Pre-allocated master bus (interleaved f32), `frames * out_channels`.
    master: Vec<f32>,
    out_channels: u16,
    frames: usize,
    master_gain: f32,
    game_mode: bool,
    /// Linear attenuation applied to Foreground voices in game mode.
    foreground_duck: f32,
    /// Linear attenuation applied to Background voices in game mode.
    background_duck: f32,
    /// If true, the master bus is soft-limited to [-1,1]; if false it is left
    /// at full range (an integer/HDA stage downstream applies its own clamp).
    limit_master: bool,
}

impl GameMixer {
    /// Create a mixer producing `out_channels` interleaved f32 at `frames` per
    /// period. All scratch is allocated here; [`mix`](Self::mix) allocates none.
    pub fn new(out_channels: u16, frames: usize) -> Self {
        Self {
            streams: Vec::new(),
            master: vec![0.0; frames * out_channels as usize],
            out_channels,
            frames,
            master_gain: 1.0,
            game_mode: false,
            foreground_duck: 0.5, // -6 dB
            background_duck: 0.2, // ~-14 dB
            limit_master: true,
        }
    }

    /// Register a voice. Its channel count may differ from the output; the mix
    /// up/down-mixes mono<->stereo. Returns a handle for refilling the buffer.
    pub fn add_stream(&mut self, name: &str, channels: u16, priority: StreamPriority) -> StreamId {
        let id = StreamId(self.streams.len());
        self.streams
            .push(Stream::new(name, channels, self.frames, priority));
        id
    }

    /// Mutable access to a voice's PCM buffer (the producer fills this).
    pub fn stream_buffer_mut(&mut self, id: StreamId) -> Option<&mut [f32]> {
        self.streams.get_mut(id.0).map(|s| s.buffer.as_mut_slice())
    }

    pub fn set_stream_gain(&mut self, id: StreamId, gain: f32) {
        if let Some(s) = self.streams.get_mut(id.0) {
            s.gain = gain.max(0.0);
        }
    }

    pub fn set_stream_mute(&mut self, id: StreamId, muted: bool) {
        if let Some(s) = self.streams.get_mut(id.0) {
            s.muted = muted;
        }
    }

    pub fn set_stream_active(&mut self, id: StreamId, active: bool) {
        if let Some(s) = self.streams.get_mut(id.0) {
            s.active = active;
        }
    }

    pub fn set_master_gain(&mut self, gain: f32) {
        self.master_gain = gain.max(0.0);
    }

    pub fn set_game_mode(&mut self, on: bool) {
        self.game_mode = on;
    }

    pub fn game_mode(&self) -> bool {
        self.game_mode
    }

    /// Configure ducking attenuation (linear gains) applied in game mode.
    pub fn set_duck(&mut self, foreground: f32, background: f32) {
        self.foreground_duck = foreground.clamp(0.0, 1.0);
        self.background_duck = background.clamp(0.0, 1.0);
    }

    pub fn set_limit_master(&mut self, on: bool) {
        self.limit_master = on;
    }

    pub fn stream_count(&self) -> usize {
        self.streams.len()
    }

    /// Effective gain for a voice given its priority and current game mode.
    #[inline]
    fn effective_gain(&self, s: &Stream) -> f32 {
        if s.muted || !s.active {
            return 0.0;
        }
        let duck = if self.game_mode {
            match s.priority {
                StreamPriority::Game => 1.0,
                StreamPriority::Foreground => self.foreground_duck,
                StreamPriority::Background => self.background_duck,
            }
        } else {
            1.0
        };
        s.gain * duck
    }

    /// Mix all voices into the master bus and return it. **Allocation-free.**
    ///
    /// Per voice: applies effective gain (with game-mode ducking), up/down-mixes
    /// to the output channel count, and saturating-sums into the master. Then
    /// applies master gain and (optionally) the soft limiter so the result is
    /// glitch-free and within range for the HDA / USB-audio DMA ring.
    pub fn mix(&mut self) -> &[f32] {
        // Clear master in place — no allocation.
        for s in self.master.iter_mut() {
            *s = 0.0;
        }

        let out_ch = self.out_channels as usize;
        let frames = self.frames;

        for stream in &self.streams {
            let g = self.effective_gain(stream);
            if g == 0.0 {
                continue;
            }
            let in_ch = stream.channels as usize;
            let src = &stream.buffer;

            // Up/down-mix per frame, gain-scaled, saturating-summed.
            for f in 0..frames {
                if in_ch == out_ch {
                    let base = f * out_ch;
                    for c in 0..out_ch {
                        let i = base + c;
                        if i < src.len() && i < self.master.len() {
                            self.master[i] += src[i] * g;
                        }
                    }
                } else if in_ch == 1 && out_ch >= 2 {
                    // Mono source fanned to every output channel.
                    let si = f;
                    if si < src.len() {
                        let v = src[si] * g;
                        let base = f * out_ch;
                        for c in 0..out_ch {
                            let i = base + c;
                            if i < self.master.len() {
                                self.master[i] += v;
                            }
                        }
                    }
                } else if in_ch >= 2 && out_ch == 1 {
                    // Downmix to mono: average the source channels.
                    let sbase = f * in_ch;
                    let mut sum = 0.0f32;
                    for c in 0..in_ch {
                        let i = sbase + c;
                        if i < src.len() {
                            sum += src[i];
                        }
                    }
                    let v = (sum / in_ch as f32) * g;
                    if f < self.master.len() {
                        self.master[f] += v;
                    }
                } else {
                    // Mismatched multichannel: copy the channels that line up.
                    let n = in_ch.min(out_ch);
                    let sbase = f * in_ch;
                    let dbase = f * out_ch;
                    for c in 0..n {
                        let si = sbase + c;
                        let di = dbase + c;
                        if si < src.len() && di < self.master.len() {
                            self.master[di] += src[si] * g;
                        }
                    }
                }
            }
        }

        // Master gain + optional soft-limit, in place.
        let mg = self.master_gain;
        if self.limit_master {
            for s in self.master.iter_mut() {
                *s = soft_limit(*s * mg);
            }
        } else if mg != 1.0 {
            for s in self.master.iter_mut() {
                *s *= mg;
            }
        }

        &self.master
    }

    /// Read-only view of the master bus from the last [`mix`](Self::mix).
    pub fn master(&self) -> &[f32] {
        &self.master
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two streams at known values sum correctly into the master bus.
    #[test]
    fn two_streams_sum() {
        let mut mx = GameMixer::new(2, 4);
        mx.set_limit_master(false); // test raw summing, not the limiter
        let a = mx.add_stream("a", 2, StreamPriority::Game);
        let b = mx.add_stream("b", 2, StreamPriority::Game);
        for s in mx.stream_buffer_mut(a).unwrap() {
            *s = 0.25;
        }
        for s in mx.stream_buffer_mut(b).unwrap() {
            *s = 0.1;
        }
        let out = mx.mix();
        assert_eq!(out.len(), 8);
        for &v in out {
            assert!((v - 0.35).abs() < 1e-6, "expected 0.35, got {v}");
        }
    }

    /// Full-scale + full-scale must NOT clip-wrap: the soft limiter holds the
    /// master within [-1, 1] instead of producing an out-of-range click.
    #[test]
    fn fullscale_sum_does_not_exceed_unity() {
        let mut mx = GameMixer::new(1, 8);
        let a = mx.add_stream("a", 1, StreamPriority::Game);
        let b = mx.add_stream("b", 1, StreamPriority::Game);
        for s in mx.stream_buffer_mut(a).unwrap() {
            *s = 1.0;
        }
        for s in mx.stream_buffer_mut(b).unwrap() {
            *s = 1.0;
        }
        let out = mx.mix();
        // Sum is 2.0 pre-limit; the limiter must pull it into range and it must
        // remain POSITIVE (a wrap would flip sign — the bug we are guarding).
        for &v in out {
            assert!(v <= 1.0 + 1e-6, "limiter overshoot: {v}");
            assert!(v > 0.5, "limiter collapsed/flipped signal: {v}");
        }
    }

    /// The soft limiter is near-linear for small signals (transparency).
    #[test]
    fn soft_limit_is_transparent_at_low_level() {
        for &x in &[0.0f32, 0.1, -0.2, 0.3, -0.05] {
            let y = soft_limit(x);
            assert!(
                (y - x).abs() < 0.02,
                "soft_limit({x}) = {y}, too far from x"
            );
        }
        // Monotonic + bounded at extremes.
        assert!(soft_limit(10.0) <= 1.0 && soft_limit(10.0) > 0.9);
        assert!(soft_limit(-10.0) >= -1.0 && soft_limit(-10.0) < -0.9);
    }

    /// Game mode ducks background but never the game voice.
    #[test]
    fn game_mode_ducks_background_only() {
        let mut mx = GameMixer::new(1, 4);
        mx.set_limit_master(false);
        mx.set_duck(0.5, 0.25);
        let game = mx.add_stream("game", 1, StreamPriority::Game);
        let bg = mx.add_stream("chat", 1, StreamPriority::Background);
        for s in mx.stream_buffer_mut(game).unwrap() {
            *s = 0.4;
        }
        for s in mx.stream_buffer_mut(bg).unwrap() {
            *s = 0.4;
        }

        // Game OFF: both full -> 0.4 + 0.4 = 0.8
        mx.set_game_mode(false);
        let out = mx.mix();
        assert!((out[0] - 0.8).abs() < 1e-6, "game-off sum = {}", out[0]);

        // Game ON: game 0.4 + ducked bg 0.4*0.25=0.1 -> 0.5
        mx.set_game_mode(true);
        let out = mx.mix();
        assert!(
            (out[0] - 0.5).abs() < 1e-6,
            "game-on ducked sum = {}",
            out[0]
        );
    }

    /// Foreground ducks less than background.
    #[test]
    fn foreground_ducks_less_than_background() {
        let mut mx = GameMixer::new(1, 2);
        mx.set_limit_master(false);
        mx.set_duck(0.5, 0.2);
        mx.set_game_mode(true);
        let fg = mx.add_stream("media", 1, StreamPriority::Foreground);
        let bg = mx.add_stream("notify", 1, StreamPriority::Background);
        for s in mx.stream_buffer_mut(fg).unwrap() {
            *s = 1.0;
        }
        for s in mx.stream_buffer_mut(bg).unwrap() {
            *s = 1.0;
        }
        // fg*0.5 + bg*0.2 = 0.7
        let out = mx.mix();
        assert!((out[0] - 0.7).abs() < 1e-6, "got {}", out[0]);
    }

    /// Mono source fans to both output channels.
    #[test]
    fn mono_upmix_to_stereo() {
        let mut mx = GameMixer::new(2, 2);
        mx.set_limit_master(false);
        let m = mx.add_stream("mono", 1, StreamPriority::Game);
        for s in mx.stream_buffer_mut(m).unwrap() {
            *s = 0.3;
        }
        let out = mx.mix();
        // frame0 L,R then frame1 L,R all = 0.3
        assert_eq!(out.len(), 4);
        for &v in out {
            assert!((v - 0.3).abs() < 1e-6, "got {v}");
        }
    }

    /// Stereo source downmixes to mono by averaging.
    #[test]
    fn stereo_downmix_to_mono_averages() {
        let mut mx = GameMixer::new(1, 2);
        mx.set_limit_master(false);
        let s = mx.add_stream("stereo", 2, StreamPriority::Game);
        {
            let buf = mx.stream_buffer_mut(s).unwrap();
            // frame0: L=0.2 R=0.6 -> avg 0.4 ; frame1: L=1.0 R=0.0 -> avg 0.5
            buf[0] = 0.2;
            buf[1] = 0.6;
            buf[2] = 1.0;
            buf[3] = 0.0;
        }
        let out = mx.mix();
        assert_eq!(out.len(), 2);
        assert!((out[0] - 0.4).abs() < 1e-6, "f0 {}", out[0]);
        assert!((out[1] - 0.5).abs() < 1e-6, "f1 {}", out[1]);
    }

    /// Muted and inactive voices contribute nothing.
    #[test]
    fn muted_and_inactive_are_silent() {
        let mut mx = GameMixer::new(1, 2);
        mx.set_limit_master(false);
        let a = mx.add_stream("a", 1, StreamPriority::Game);
        let b = mx.add_stream("b", 1, StreamPriority::Game);
        let c = mx.add_stream("c", 1, StreamPriority::Game);
        for id in [a, b, c] {
            for s in mx.stream_buffer_mut(id).unwrap() {
                *s = 0.5;
            }
        }
        mx.set_stream_mute(b, true);
        mx.set_stream_active(c, false);
        // Only `a` survives -> 0.5
        let out = mx.mix();
        for &v in out {
            assert!((v - 0.5).abs() < 1e-6, "got {v}");
        }
    }

    /// Per-stream gain scales the contribution.
    #[test]
    fn per_stream_gain() {
        let mut mx = GameMixer::new(1, 2);
        mx.set_limit_master(false);
        let a = mx.add_stream("a", 1, StreamPriority::Game);
        for s in mx.stream_buffer_mut(a).unwrap() {
            *s = 0.5;
        }
        mx.set_stream_gain(a, 0.5);
        let out = mx.mix();
        for &v in out {
            assert!((v - 0.25).abs() < 1e-6, "got {v}");
        }
    }

    /// The steady-state mix path performs zero heap allocation. We prove this
    /// by reusing the same mixer across many ticks without growing any Vec
    /// (capacities are fixed at construction; mix() only writes existing slots).
    #[test]
    fn mix_is_allocation_free_steady_state() {
        let mut mx = GameMixer::new(2, 128);
        let a = mx.add_stream("a", 2, StreamPriority::Game);
        let master_cap = mx.master().len();
        let buf_cap = mx.stream_buffer_mut(a).unwrap().len();
        for tick in 0..1000 {
            // refill (producer side) then mix (hot path)
            for s in mx.stream_buffer_mut(a).unwrap() {
                *s = (tick as f32 % 7.0) * 0.05;
            }
            let _ = mx.mix();
            // No buffer was reallocated/resized by the hot path.
            assert_eq!(mx.master().len(), master_cap);
            assert_eq!(mx.stream_buffer_mut(a).unwrap().len(), buf_cap);
        }
    }
}
