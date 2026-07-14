//! VoiceMeeter-class audio routing matrix (Concept §RaeAudio — "VoiceMeeter-class
//! audio routing").
//!
//! Routes N virtual input strips to M output buses — the A1/A2/B1/B2 model where a
//! single strip can feed several buses at once (e.g. "game audio → speakers AND the
//! stream-mix bus"), each route carrying its own linear gain. Layered ON TOP of the
//! per-stream [`crate::mixer::GameMixer`] (which collapses voices to one master bus):
//! the GameMixer is the game/SCHED_GAME fast path; this matrix is the broader desktop
//! routing surface a creator/streamer expects.
//!
//! Pure logic — interleaved f32 with a shared `channels`/`frames` block layout on
//! every input and bus — so the whole matrix is `cargo test`-provable on the host with
//! no audio hardware (the live engine drives it from the HDA / USB-audio output side).

extern crate alloc;
use alloc::vec;
use alloc::vec::Vec;

/// A strip→bus routing matrix. Inputs and buses share the same `channels` count and
/// `frames` per processing block. Any input may feed any subset of buses; each
/// `(input, bus)` route carries its own linear gain (0.0 = not routed). Inputs have a
/// pre-gain + mute; buses have a master gain + mute.
pub struct RoutingMatrix {
    channels: u16,
    frames: usize,
    n_inputs: usize,
    n_buses: usize,
    /// Row-major `[input * n_buses + bus]` linear route gain; 0.0 means "not routed".
    route_gain: Vec<f32>,
    input_gain: Vec<f32>,
    input_mute: Vec<bool>,
    bus_gain: Vec<f32>,
    bus_mute: Vec<bool>,
}

impl RoutingMatrix {
    /// A matrix with `n_inputs` strips and `n_buses` buses, no routes set (silent
    /// until [`route`](Self::route) wires strips to buses), unity input/bus gains.
    pub fn new(channels: u16, frames: usize, n_inputs: usize, n_buses: usize) -> Self {
        Self {
            channels,
            frames,
            n_inputs,
            n_buses,
            route_gain: vec![0.0; n_inputs * n_buses],
            input_gain: vec![1.0; n_inputs],
            input_mute: vec![false; n_inputs],
            bus_gain: vec![1.0; n_buses],
            bus_mute: vec![false; n_buses],
        }
    }

    fn samples(&self) -> usize {
        self.frames * self.channels as usize
    }

    /// Route input strip `input` to output bus `bus` at linear `gain` (0.0 unroutes).
    /// Out-of-range indices are ignored (no panic on a stale UI request).
    pub fn route(&mut self, input: usize, bus: usize, gain: f32) {
        if input < self.n_inputs && bus < self.n_buses {
            self.route_gain[input * self.n_buses + bus] = gain.max(0.0);
        }
    }

    /// Whether strip `input` currently feeds bus `bus` (gain > 0).
    pub fn is_routed(&self, input: usize, bus: usize) -> bool {
        input < self.n_inputs
            && bus < self.n_buses
            && self.route_gain[input * self.n_buses + bus] > 0.0
    }

    pub fn set_input_gain(&mut self, input: usize, gain: f32) {
        if let Some(g) = self.input_gain.get_mut(input) {
            *g = gain.max(0.0);
        }
    }
    pub fn set_input_mute(&mut self, input: usize, mute: bool) {
        if let Some(m) = self.input_mute.get_mut(input) {
            *m = mute;
        }
    }
    pub fn set_bus_gain(&mut self, bus: usize, gain: f32) {
        if let Some(g) = self.bus_gain.get_mut(bus) {
            *g = gain.max(0.0);
        }
    }
    pub fn set_bus_mute(&mut self, bus: usize, mute: bool) {
        if let Some(m) = self.bus_mute.get_mut(bus) {
            *m = mute;
        }
    }

    pub fn channels(&self) -> u16 {
        self.channels
    }
    pub fn frames(&self) -> usize {
        self.frames
    }
    pub fn input_count(&self) -> usize {
        self.n_inputs
    }
    pub fn bus_count(&self) -> usize {
        self.n_buses
    }

    /// Mix `inputs` (one interleaved block per strip) into `buses` (one per bus).
    /// Each bus = `bus_gain * Σ_i( !input_mute · input_gain[i] · route_gain[i][b] · input[i] )`.
    /// A muted bus (or one with no live routes) is zeroed. Buffers shorter than a full
    /// `frames*channels` block are processed up to their length and the rest left as
    /// written (defensive — a partial block never reads/writes out of bounds). Returns
    /// the number of buses written (`min(buses.len(), bus_count)`).
    pub fn mix(&self, inputs: &[&[f32]], buses: &mut [&mut [f32]]) -> usize {
        let block = self.samples();
        let nb = buses.len().min(self.n_buses);
        let n_in = self.n_inputs.min(inputs.len());
        for b in 0..nb {
            let out = &mut buses[b];
            let span = out.len().min(block);
            for s in out.iter_mut().take(span) {
                *s = 0.0;
            }
            if self.bus_mute[b] {
                continue;
            }
            let bg = self.bus_gain[b];
            for i in 0..n_in {
                if self.input_mute[i] {
                    continue;
                }
                let rg = self.route_gain[i * self.n_buses + b];
                if rg <= 0.0 {
                    continue;
                }
                let g = bg * self.input_gain[i] * rg;
                let inp = inputs[i];
                let lim = span.min(inp.len());
                for s in 0..lim {
                    out[s] += inp[s] * g;
                }
            }
        }
        nb
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    // Helpers: a 2ch×2frame interleaved block filled with `v`, and the bus refs dance.
    fn block(v: f32) -> Vec<f32> {
        vec![v; 4] // 2 channels * 2 frames
    }
    fn mix_into(m: &RoutingMatrix, inputs: &[Vec<f32>], n_buses: usize) -> Vec<Vec<f32>> {
        let mut buses: Vec<Vec<f32>> = (0..n_buses).map(|_| vec![0.0f32; 4]).collect();
        let in_refs: Vec<&[f32]> = inputs.iter().map(|v| v.as_slice()).collect();
        let mut bus_refs: Vec<&mut [f32]> = buses.iter_mut().map(|v| v.as_mut_slice()).collect();
        m.mix(&in_refs, &mut bus_refs);
        buses
    }

    #[test]
    fn diagonal_routes_keep_strips_separate() {
        // input 0 -> bus 0 @1.0, input 1 -> bus 1 @0.5; no cross-routes.
        let mut m = RoutingMatrix::new(2, 2, 2, 2);
        m.route(0, 0, 1.0);
        m.route(1, 1, 0.5);
        let out = mix_into(&m, &[block(1.0), block(2.0)], 2);
        assert!(
            out[0].iter().all(|&x| (x - 1.0).abs() < 1e-6),
            "bus0 = input0: {:?}",
            out[0]
        );
        assert!(
            out[1].iter().all(|&x| (x - 1.0).abs() < 1e-6),
            "bus1 = 0.5*input1=1.0: {:?}",
            out[1]
        );
        // Cross-routes are silent (the bug this guards: a flat sum that ignores the matrix).
        assert!(!m.is_routed(0, 1) && !m.is_routed(1, 0));
    }

    #[test]
    fn one_strip_feeds_two_buses_voicemeeter_a_plus_b() {
        // The defining VoiceMeeter feature: input 0 -> bus 0 (speakers) AND bus 1 (stream).
        let mut m = RoutingMatrix::new(2, 2, 1, 2);
        m.route(0, 0, 1.0);
        m.route(0, 1, 1.0);
        let out = mix_into(&m, &[block(0.5)], 2);
        assert!(
            out[0].iter().all(|&x| (x - 0.5).abs() < 1e-6),
            "bus0 got the strip"
        );
        assert!(
            out[1].iter().all(|&x| (x - 0.5).abs() < 1e-6),
            "bus1 ALSO got the strip"
        );
    }

    #[test]
    fn two_strips_sum_into_one_bus() {
        let mut m = RoutingMatrix::new(2, 2, 2, 1);
        m.route(0, 0, 1.0);
        m.route(1, 0, 1.0);
        let out = mix_into(&m, &[block(0.3), block(0.4)], 1);
        assert!(
            out[0].iter().all(|&x| (x - 0.7).abs() < 1e-6),
            "summed: {:?}",
            out[0]
        );
    }

    #[test]
    fn input_mute_and_pre_gain_apply() {
        let mut m = RoutingMatrix::new(2, 2, 2, 1);
        m.route(0, 0, 1.0);
        m.route(1, 0, 1.0);
        m.set_input_mute(0, true); // strip 0 muted -> only strip 1
        m.set_input_gain(1, 0.5); // strip 1 pre-gain
        let out = mix_into(&m, &[block(1.0), block(1.0)], 1);
        assert!(
            out[0].iter().all(|&x| (x - 0.5).abs() < 1e-6),
            "muted+gain: {:?}",
            out[0]
        );
    }

    #[test]
    fn bus_master_gain_and_mute() {
        let mut m = RoutingMatrix::new(2, 2, 1, 2);
        m.route(0, 0, 1.0);
        m.route(0, 1, 1.0);
        m.set_bus_gain(0, 0.25); // bus 0 master quarter
        m.set_bus_mute(1, true); // bus 1 fully muted
        let out = mix_into(&m, &[block(1.0)], 2);
        assert!(
            out[0].iter().all(|&x| (x - 0.25).abs() < 1e-6),
            "bus master gain: {:?}",
            out[0]
        );
        assert!(
            out[1].iter().all(|&x| x == 0.0),
            "muted bus silent: {:?}",
            out[1]
        );
    }

    #[test]
    fn route_clamps_negative_and_out_of_range_is_noop() {
        let mut m = RoutingMatrix::new(2, 2, 1, 1);
        m.route(0, 0, -3.0); // clamps to 0 (unrouted)
        assert!(!m.is_routed(0, 0));
        m.route(9, 9, 1.0); // out of range -> ignored, no panic
        let out = mix_into(&m, &[block(1.0)], 1);
        assert!(
            out[0].iter().all(|&x| x == 0.0),
            "no route -> silent: {:?}",
            out[0]
        );
    }
}
