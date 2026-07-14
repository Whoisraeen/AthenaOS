//! AthAudio — low-latency audio engine.
//!
//! Sub-3 ms round-trip on certified hardware.
//! No ASIO mess, no PulseAudio mess.
//!
//! See `docs/components/raeaudio.md` for the design.
// no_std for real builds; std under `cargo test` so the dsp host KATs can link.
#![cfg_attr(not(test), no_std)]

pub mod dsp;
/// HDA pin config-default decoding — find the mic input pin (L1375), host-KAT'd
/// against the real Realtek ALC269VC on the Athena.
pub mod hda_pin;
pub mod mixer;
pub mod routing;

extern crate alloc;

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Audio format types
// ---------------------------------------------------------------------------

/// PCM sample encoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleFormat {
    F32,
    I16,
    I24,
    I32,
}

impl SampleFormat {
    /// Bytes occupied by a single sample.
    pub const fn byte_size(self) -> usize {
        match self {
            Self::F32 | Self::I32 => 4,
            Self::I24 => 3,
            Self::I16 => 2,
        }
    }
}

/// Speaker / channel layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelLayout {
    Mono,
    Stereo,
    Surround51,
    Surround71,
}

impl ChannelLayout {
    pub const fn channel_count(self) -> u16 {
        match self {
            Self::Mono => 1,
            Self::Stereo => 2,
            Self::Surround51 => 6,
            Self::Surround71 => 8,
        }
    }
}

/// Fully describes a PCM stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioFormat {
    pub sample_rate: u32,
    pub channels: u16,
    pub sample_format: SampleFormat,
}

impl AudioFormat {
    pub const fn new(sample_rate: u32, channels: u16, sample_format: SampleFormat) -> Self {
        Self {
            sample_rate,
            channels,
            sample_format,
        }
    }

    /// Bytes per interleaved frame (all channels for one sample instant).
    pub const fn frame_byte_size(&self) -> usize {
        self.sample_format.byte_size() * self.channels as usize
    }
}

// ---------------------------------------------------------------------------
// Sample trait — abstracts over concrete PCM types
// ---------------------------------------------------------------------------

/// Marker + arithmetic requirements for a type that can live in an `AudioBuffer`.
pub trait Sample:
    Copy + Default + core::ops::Add<Output = Self> + core::ops::Mul<Output = Self>
{
    /// Neutral (silent) value.
    const ZERO: Self;

    /// Sum two samples *without clip-wrap*.
    ///
    /// For integer PCM, `a + b` wraps on overflow — two full-scale signals
    /// summed with `+` produce a near-silent negative value, an audible click
    /// (the exact "no artifacts" violation the Concept forbids). This sums with
    /// saturation so full-scale + full-scale clamps to full-scale. For f32 the
    /// natural range is unbounded headroom, so `+` is already correct and a
    /// downstream limiter handles the ceiling.
    fn saturating_mix(self, other: Self) -> Self;
}

impl Sample for f32 {
    const ZERO: Self = 0.0;
    #[inline]
    fn saturating_mix(self, other: Self) -> Self {
        self + other
    }
}

impl Sample for i16 {
    const ZERO: Self = 0;
    #[inline]
    fn saturating_mix(self, other: Self) -> Self {
        self.saturating_add(other)
    }
}

impl Sample for i32 {
    const ZERO: Self = 0;
    #[inline]
    fn saturating_mix(self, other: Self) -> Self {
        self.saturating_add(other)
    }
}

// ---------------------------------------------------------------------------
// AudioBuffer<T>
// ---------------------------------------------------------------------------

/// Interleaved PCM buffer generic over sample type.
///
/// Layout: `[ch0_f0, ch1_f0, …, ch0_f1, ch1_f1, …]`
pub struct AudioBuffer<T: Sample> {
    data: Vec<T>,
    channels: u16,
    frames: usize,
}

impl<T: Sample> AudioBuffer<T> {
    /// Allocate a silent buffer.
    pub fn new(channels: u16, frames: usize) -> Self {
        let len = channels as usize * frames;
        Self {
            data: vec![T::ZERO; len],
            channels,
            frames,
        }
    }

    #[inline]
    pub fn frames(&self) -> usize {
        self.frames
    }

    #[inline]
    pub fn channels(&self) -> u16 {
        self.channels
    }

    #[inline]
    pub fn as_slice(&self) -> &[T] {
        &self.data
    }

    #[inline]
    pub fn as_mut_slice(&mut self) -> &mut [T] {
        &mut self.data
    }

    /// Immutable access to a single sample.
    #[inline]
    pub fn sample(&self, frame: usize, channel: u16) -> T {
        self.data[frame * self.channels as usize + channel as usize]
    }

    /// Mutable access to a single sample.
    #[inline]
    pub fn sample_mut(&mut self, frame: usize, channel: u16) -> &mut T {
        let idx = frame * self.channels as usize + channel as usize;
        &mut self.data[idx]
    }

    /// Additive mix of `other` into `self`, handling basic channel up/down-mixing.
    ///
    /// Summing is saturating (see [`Sample::saturating_mix`]) so integer PCM
    /// cannot clip-wrap into an audible click.
    pub fn mix_into(&mut self, other: &Self) {
        if self.channels == other.channels {
            let frames = self.frames.min(other.frames);
            let len = frames * self.channels as usize;
            for i in 0..len {
                self.data[i] = self.data[i].saturating_mix(other.data[i]);
            }
        } else if self.channels == 2 && other.channels == 1 {
            // Mono to Stereo upmix
            let frames = self.frames.min(other.frames);
            for f in 0..frames {
                let sample = other.sample(f, 0);
                *self.sample_mut(f, 0) = self.sample(f, 0).saturating_mix(sample);
                *self.sample_mut(f, 1) = self.sample(f, 1).saturating_mix(sample);
            }
        } else if self.channels == 6 && other.channels == 2 {
            // Stereo to 5.1 upmix (L, R, C, LFE, Ls, Rs)
            let frames = self.frames.min(other.frames);
            for f in 0..frames {
                let left = other.sample(f, 0);
                let right = other.sample(f, 1);
                *self.sample_mut(f, 0) = self.sample(f, 0).saturating_mix(left); // L
                *self.sample_mut(f, 1) = self.sample(f, 1).saturating_mix(right);
                // R
                // Optional: synthesize center or LFE. We just do L/R for now.
            }
        } else if self.channels == 2 && other.channels == 6 {
            // 5.1 to Stereo downmix (L+R only for now)
            let frames = self.frames.min(other.frames);
            for f in 0..frames {
                let left = other.sample(f, 0);
                let right = other.sample(f, 1);
                *self.sample_mut(f, 0) = self.sample(f, 0).saturating_mix(left);
                *self.sample_mut(f, 1) = self.sample(f, 1).saturating_mix(right);
            }
        } else {
            // Fallback: mix identical channels up to min_channels
            let frames = self.frames.min(other.frames);
            let min_ch = self.channels.min(other.channels);
            for f in 0..frames {
                for c in 0..min_ch {
                    *self.sample_mut(f, c) = self.sample(f, c).saturating_mix(other.sample(f, c));
                }
            }
        }
    }

    /// Set every sample to `ZERO`.
    pub fn clear(&mut self) {
        for s in self.data.iter_mut() {
            *s = T::ZERO;
        }
    }
}

/// f32-specific helpers.
impl AudioBuffer<f32> {
    /// Scale every sample by `gain`.
    pub fn apply_gain(&mut self, gain: f32) {
        for s in self.data.iter_mut() {
            *s *= gain;
        }
    }
}

// ---------------------------------------------------------------------------
// Audio graph – Node trait
// ---------------------------------------------------------------------------

/// A single processing stage in the audio graph.
pub trait Node {
    /// Process one buffer period.  Reads from `input`, writes to `output`.
    fn process(&mut self, input: &AudioBuffer<f32>, output: &mut AudioBuffer<f32>);

    /// Human-readable name for debugging / profiling.
    fn name(&self) -> &str;
}

// ---------------------------------------------------------------------------
// Concrete nodes
// ---------------------------------------------------------------------------

/// Volume control with per-frame linear smoothing to avoid zipper noise.
pub struct GainNode {
    current_gain: f32,
    target_gain: f32,
    /// Reciprocal of the number of frames over which to smooth.  Pre-computed
    /// for the common buffer size so we avoid a division per frame.
    smoothing_coeff: f32,
}

impl GainNode {
    pub fn new(initial_gain: f32) -> Self {
        Self {
            current_gain: initial_gain,
            target_gain: initial_gain,
            smoothing_coeff: 1.0 / 128.0,
        }
    }

    pub fn set_gain(&mut self, gain: f32) {
        self.target_gain = gain;
    }

    pub fn set_smoothing_frames(&mut self, frames: usize) {
        self.smoothing_coeff = if frames == 0 {
            1.0
        } else {
            1.0 / frames as f32
        };
    }
}

impl Node for GainNode {
    fn process(&mut self, input: &AudioBuffer<f32>, output: &mut AudioBuffer<f32>) {
        let total = input.data.len().min(output.data.len());
        let frames = input.frames();
        let ch = input.channels() as usize;

        for f in 0..frames {
            // one-pole smoothing toward target
            self.current_gain += (self.target_gain - self.current_gain) * self.smoothing_coeff;
            let g = self.current_gain;
            let base = f * ch;
            for c in 0..ch {
                let idx = base + c;
                if idx < total {
                    output.data[idx] = input.data[idx] * g;
                }
            }
        }
    }

    fn name(&self) -> &str {
        "GainNode"
    }
}

/// Sums N input buffers into one output.
pub struct MixerNode {
    input_buffers: Vec<AudioBuffer<f32>>,
}

impl MixerNode {
    pub fn new() -> Self {
        Self {
            input_buffers: Vec::new(),
        }
    }

    /// Register an input slot; returns its index.
    pub fn add_input(&mut self, channels: u16, frames: usize) -> usize {
        let idx = self.input_buffers.len();
        self.input_buffers.push(AudioBuffer::new(channels, frames));
        idx
    }

    pub fn input_buffer_mut(&mut self, index: usize) -> &mut AudioBuffer<f32> {
        &mut self.input_buffers[index]
    }
}

impl Node for MixerNode {
    fn process(&mut self, _input: &AudioBuffer<f32>, output: &mut AudioBuffer<f32>) {
        output.clear();
        for buf in &self.input_buffers {
            output.mix_into(buf);
        }
    }

    fn name(&self) -> &str {
        "MixerNode"
    }
}

/// Stereo panner using the constant-power (equal-power) law.
///
/// `pan` ranges from -1.0 (full left) to 1.0 (full right).
pub struct PanNode {
    pan: f32,
}

impl PanNode {
    pub fn new(pan: f32) -> Self {
        Self {
            pan: clamp(pan, -1.0, 1.0),
        }
    }

    pub fn set_pan(&mut self, pan: f32) {
        self.pan = clamp(pan, -1.0, 1.0);
    }
}

impl Node for PanNode {
    fn process(&mut self, input: &AudioBuffer<f32>, output: &mut AudioBuffer<f32>) {
        // constant-power pan: map [-1,1] to [0, pi/2], then cos/sin
        let angle = (self.pan + 1.0) * 0.25 * core::f32::consts::PI;
        let (gain_r, gain_l) = sin_cos_approx(angle);

        let frames = input.frames().min(output.frames());
        let in_ch = input.channels() as usize;
        let out_ch = output.channels() as usize;

        for f in 0..frames {
            let mono = if in_ch == 1 {
                input.data[f]
            } else {
                // downmix to mono first
                let mut sum = 0.0f32;
                for c in 0..in_ch {
                    sum += input.data[f * in_ch + c];
                }
                sum / in_ch as f32
            };

            if out_ch >= 2 {
                output.data[f * out_ch] = mono * gain_l;
                output.data[f * out_ch + 1] = mono * gain_r;
            } else {
                output.data[f] = mono;
            }
        }
    }

    fn name(&self) -> &str {
        "PanNode"
    }
}

/// Hard limiter / clipper — clamps samples to [-threshold, threshold].
pub struct ClipNode {
    threshold: f32,
}

impl ClipNode {
    pub fn new(threshold: f32) -> Self {
        Self {
            threshold: if threshold < 0.0 { 1.0 } else { threshold },
        }
    }
}

impl Node for ClipNode {
    fn process(&mut self, input: &AudioBuffer<f32>, output: &mut AudioBuffer<f32>) {
        let len = input.data.len().min(output.data.len());
        let t = self.threshold;
        for i in 0..len {
            output.data[i] = clamp(input.data[i], -t, t);
        }
    }

    fn name(&self) -> &str {
        "ClipNode"
    }
}

// ---------------------------------------------------------------------------
// Device abstraction
// ---------------------------------------------------------------------------

/// Opaque handle to a hardware (or virtual) audio endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DeviceId(pub u64);

/// Static metadata about an audio device.
#[derive(Debug, Clone)]
pub struct DeviceInfo {
    pub id: DeviceId,
    pub name: String,
    pub format: AudioFormat,
    pub buffer_size_frames: usize,
    /// Measured or estimated one-way latency in microseconds.
    pub latency_us: u32,
}

/// Errors that device operations may return.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceError {
    NotFound,
    AlreadyOpen,
    NotOpen,
    Underrun,
    Overrun,
    UnsupportedFormat,
    HardwareError,
}

/// Platform-agnostic output device.
pub trait AudioDevice {
    fn open(&mut self) -> Result<(), DeviceError>;
    fn close(&mut self) -> Result<(), DeviceError>;

    /// Submit one buffer period worth of interleaved f32 samples.
    fn write(&mut self, buf: &AudioBuffer<f32>) -> Result<(), DeviceError>;

    fn info(&self) -> &DeviceInfo;
}

// ---------------------------------------------------------------------------
// Audio engine — owns the graph + device
// ---------------------------------------------------------------------------

/// Index into the engine's node table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NodeId(pub usize);

/// A directed edge in the audio graph.
#[derive(Debug, Clone, Copy)]
struct Connection {
    from: NodeId,
    to: NodeId,
}

/// Central audio engine.
///
/// Owns processing nodes, the connection graph, and an optional output device.
/// Call [`process_tick`](AudioEngine::process_tick) once per buffer period
/// (driven by the device callback or a dedicated audio thread).
pub struct AudioEngine {
    nodes: Vec<Box<dyn Node>>,
    connections: Vec<Connection>,
    master_gain: GainNode,
    buffers: Vec<AudioBuffer<f32>>,
    output_buffer: AudioBuffer<f32>,
    channels: u16,
    buffer_frames: usize,
}

impl AudioEngine {
    pub fn new(channels: u16, buffer_frames: usize) -> Self {
        Self {
            nodes: Vec::new(),
            connections: Vec::new(),
            master_gain: GainNode::new(1.0),
            buffers: Vec::new(),
            output_buffer: AudioBuffer::new(channels, buffer_frames),
            channels,
            buffer_frames,
        }
    }

    /// Insert a processing node; returns its handle.
    pub fn add_node(&mut self, node: Box<dyn Node>) -> NodeId {
        let id = NodeId(self.nodes.len());
        self.nodes.push(node);
        self.buffers
            .push(AudioBuffer::new(self.channels, self.buffer_frames));
        id
    }

    /// Create a directed edge from one node's output to another's input.
    pub fn connect(&mut self, from: NodeId, to: NodeId) {
        self.connections.push(Connection { from, to });
    }

    pub fn set_master_volume(&mut self, volume: f32) {
        self.master_gain.set_gain(volume);
    }

    /// Copy a buffer into the graph's scratch space for a given node so it
    /// will be picked up on the next `process_tick`.
    pub fn play_buffer(&mut self, node: NodeId, buf: &AudioBuffer<f32>) {
        if let Some(b) = self.buffers.get_mut(node.0) {
            b.mix_into(buf);
        }
    }

    /// Run one buffer period through the graph.
    ///
    /// Simplified single-pass topology: iterate nodes in insertion order,
    /// summing connected inputs.  A production engine would topologically
    /// sort, but insertion order is correct for simple chains and good enough
    /// for early bring-up.
    pub fn process_tick(&mut self) -> &AudioBuffer<f32> {
        let node_count = self.nodes.len();

        // For each node, gather connected inputs and process.
        // We need to work around the borrow checker: process each node by
        // temporarily swapping out its output buffer.
        for idx in 0..node_count {
            // Build a summed input from all connections targeting this node.
            let mut input = AudioBuffer::<f32>::new(self.channels, self.buffer_frames);
            for conn in &self.connections {
                if conn.to.0 == idx {
                    if let Some(src) = self.buffers.get(conn.from.0) {
                        input.mix_into(src);
                    }
                }
            }

            // Swap out the output buffer, process, swap back.
            let mut out_buf = core::mem::replace(
                &mut self.buffers[idx],
                AudioBuffer::new(self.channels, self.buffer_frames),
            );
            self.nodes[idx].process(&input, &mut out_buf);
            self.buffers[idx] = out_buf;
        }

        // Sum all terminal nodes (nodes that are not a source for any
        // connection) into the master output, then apply master gain.
        self.output_buffer.clear();
        for idx in 0..node_count {
            let is_source = self.connections.iter().any(|c| c.from.0 == idx);
            if !is_source {
                self.output_buffer.mix_into(&self.buffers[idx]);
            }
        }

        // Apply master gain in-place.
        let mut tmp = AudioBuffer::new(self.channels, self.buffer_frames);
        self.master_gain.process(&self.output_buffer, &mut tmp);
        core::mem::swap(&mut self.output_buffer, &mut tmp);

        // Clear per-node scratch buffers for next tick.
        for b in &mut self.buffers {
            b.clear();
        }

        &self.output_buffer
    }

    /// Elevate the current thread to SCHED_BODY priority to guarantee sub-3ms latency.
    pub fn elevate_to_game_priority() {
        // We use SYS_NULL_LATENCY_ENTER (44) to explicitly request real-time pinned execution
        #[cfg(target_arch = "x86_64")]
        unsafe {
            core::arch::asm!(
                "syscall",
                in("rax") 44, // SYS_NULL_LATENCY_ENTER
                in("rdi") 0,  // tid = 0 (self)
                out("rcx") _,
                out("r11") _,
                options(nostack, preserves_flags)
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Audio Effects Chain
// ---------------------------------------------------------------------------

/// Trait for real-time audio effects. All effects process in-place on f32
/// sample buffers. Implementations must be allocation-free in `process()`.
pub trait AudioEffect {
    fn process(&mut self, samples: &mut [f32]);
    fn name(&self) -> &str;
    fn bypass(&self) -> bool;
    fn set_bypass(&mut self, bypass: bool);
}

/// Simple gain effect.
pub struct GainEffect {
    pub gain: f32,
    bypassed: bool,
}

impl GainEffect {
    pub fn new(gain: f32) -> Self {
        Self {
            gain,
            bypassed: false,
        }
    }
}

impl AudioEffect for GainEffect {
    fn process(&mut self, samples: &mut [f32]) {
        if self.bypassed {
            return;
        }
        let g = self.gain;
        for s in samples.iter_mut() {
            *s *= g;
        }
    }
    fn name(&self) -> &str {
        "Gain"
    }
    fn bypass(&self) -> bool {
        self.bypassed
    }
    fn set_bypass(&mut self, bypass: bool) {
        self.bypassed = bypass;
    }
}

/// 3-band parametric EQ. Each band: frequency, gain_db, Q factor.
/// Uses biquad coefficients computed from analog prototype.
pub struct ParametricEq {
    pub bands: [EqBand; 3],
    states: [[BiquadState; 2]; 3], // per-band, stereo
    bypassed: bool,
}

#[derive(Clone, Copy)]
pub struct EqBand {
    pub frequency: f32,
    pub gain_db: f32,
    pub q: f32,
    coeffs: BiquadCoeffs,
}

impl EqBand {
    pub fn new(frequency: f32, gain_db: f32, q: f32, sample_rate: f32) -> Self {
        let mut band = Self {
            frequency,
            gain_db,
            q,
            coeffs: BiquadCoeffs::default(),
        };
        band.compute_coeffs(sample_rate);
        band
    }

    fn compute_coeffs(&mut self, sample_rate: f32) {
        let w0 = 2.0 * core::f32::consts::PI * self.frequency / sample_rate;
        let (sin_w0, cos_w0) = sin_cos_approx(w0);
        let alpha = sin_w0 / (2.0 * self.q);
        let a_lin = db_to_linear(self.gain_db);

        let b0 = 1.0 + alpha * a_lin;
        let b1 = -2.0 * cos_w0;
        let b2 = 1.0 - alpha * a_lin;
        let a0 = 1.0 + alpha / a_lin;
        let a1 = -2.0 * cos_w0;
        let a2 = 1.0 - alpha / a_lin;

        let inv_a0 = 1.0 / a0;
        self.coeffs = BiquadCoeffs {
            b0: b0 * inv_a0,
            b1: b1 * inv_a0,
            b2: b2 * inv_a0,
            a1: a1 * inv_a0,
            a2: a2 * inv_a0,
        };
    }
}

#[derive(Clone, Copy, Default)]
struct BiquadCoeffs {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
}

#[derive(Clone, Copy, Default)]
struct BiquadState {
    x1: f32,
    x2: f32,
    y1: f32,
    y2: f32,
}

impl BiquadState {
    #[inline]
    fn process(&mut self, input: f32, c: &BiquadCoeffs) -> f32 {
        let out = c.b0 * input + c.b1 * self.x1 + c.b2 * self.x2 - c.a1 * self.y1 - c.a2 * self.y2;
        self.x2 = self.x1;
        self.x1 = input;
        self.y2 = self.y1;
        self.y1 = out;
        out
    }
}

impl ParametricEq {
    pub fn new(low: EqBand, mid: EqBand, high: EqBand) -> Self {
        Self {
            bands: [low, mid, high],
            states: [[BiquadState::default(); 2]; 3],
            bypassed: false,
        }
    }

    pub fn default_3band(sample_rate: f32) -> Self {
        Self::new(
            EqBand::new(100.0, 0.0, 0.707, sample_rate),
            EqBand::new(1000.0, 0.0, 0.707, sample_rate),
            EqBand::new(8000.0, 0.0, 0.707, sample_rate),
        )
    }

    pub fn set_band(&mut self, index: usize, gain_db: f32, sample_rate: f32) {
        if index < 3 {
            self.bands[index].gain_db = gain_db;
            self.bands[index].compute_coeffs(sample_rate);
        }
    }
}

impl AudioEffect for ParametricEq {
    fn process(&mut self, samples: &mut [f32]) {
        if self.bypassed {
            return;
        }
        let len = samples.len();
        for band_idx in 0..3 {
            let coeffs = self.bands[band_idx].coeffs;
            let states = &mut self.states[band_idx];
            // Process interleaved stereo
            let mut i = 0;
            while i + 1 < len {
                samples[i] = states[0].process(samples[i], &coeffs);
                samples[i + 1] = states[1].process(samples[i + 1], &coeffs);
                i += 2;
            }
        }
    }
    fn name(&self) -> &str {
        "ParametricEQ"
    }
    fn bypass(&self) -> bool {
        self.bypassed
    }
    fn set_bypass(&mut self, bypass: bool) {
        self.bypassed = bypass;
    }
}

/// Dynamic range compressor with attack/release envelope follower.
pub struct Compressor {
    pub threshold_db: f32,
    pub ratio: f32,
    pub attack_ms: f32,
    pub release_ms: f32,
    pub makeup_gain_db: f32,
    envelope: f32,
    attack_coeff: f32,
    release_coeff: f32,
    bypassed: bool,
}

impl Compressor {
    pub fn new(
        threshold_db: f32,
        ratio: f32,
        attack_ms: f32,
        release_ms: f32,
        sample_rate: f32,
    ) -> Self {
        Self {
            threshold_db,
            ratio,
            attack_ms,
            release_ms,
            makeup_gain_db: 0.0,
            envelope: 0.0,
            attack_coeff: exp_coeff(attack_ms, sample_rate),
            release_coeff: exp_coeff(release_ms, sample_rate),
            bypassed: false,
        }
    }
}

impl AudioEffect for Compressor {
    fn process(&mut self, samples: &mut [f32]) {
        if self.bypassed {
            return;
        }
        let threshold_lin = db_to_linear(self.threshold_db);
        let makeup_lin = db_to_linear(self.makeup_gain_db);

        for s in samples.iter_mut() {
            let abs_s = if *s < 0.0 { -*s } else { *s };
            let coeff = if abs_s > self.envelope {
                self.attack_coeff
            } else {
                self.release_coeff
            };
            self.envelope = self.envelope * coeff + abs_s * (1.0 - coeff);

            if self.envelope > threshold_lin {
                let over = self.envelope / threshold_lin;
                let gain_reduction = fast_pow(over, 1.0 / self.ratio - 1.0);
                *s *= gain_reduction * makeup_lin;
            } else {
                *s *= makeup_lin;
            }
        }
    }
    fn name(&self) -> &str {
        "Compressor"
    }
    fn bypass(&self) -> bool {
        self.bypassed
    }
    fn set_bypass(&mut self, bypass: bool) {
        self.bypassed = bypass;
    }
}

/// Hard/soft limiter — prevents signal from exceeding ceiling.
pub struct Limiter {
    pub ceiling_db: f32,
    ceiling_lin: f32,
    bypassed: bool,
}

impl Limiter {
    pub fn new(ceiling_db: f32) -> Self {
        Self {
            ceiling_db,
            ceiling_lin: db_to_linear(ceiling_db),
            bypassed: false,
        }
    }
}

impl AudioEffect for Limiter {
    fn process(&mut self, samples: &mut [f32]) {
        if self.bypassed {
            return;
        }
        let ceil = self.ceiling_lin;
        for s in samples.iter_mut() {
            *s = clamp(*s, -ceil, ceil);
        }
    }
    fn name(&self) -> &str {
        "Limiter"
    }
    fn bypass(&self) -> bool {
        self.bypassed
    }
    fn set_bypass(&mut self, bypass: bool) {
        self.bypassed = bypass;
    }
}

/// Noise gate — silences signal below threshold.
pub struct Gate {
    pub threshold_db: f32,
    pub attack_ms: f32,
    pub release_ms: f32,
    envelope: f32,
    gate_gain: f32,
    attack_coeff: f32,
    release_coeff: f32,
    bypassed: bool,
}

impl Gate {
    pub fn new(threshold_db: f32, attack_ms: f32, release_ms: f32, sample_rate: f32) -> Self {
        Self {
            threshold_db,
            attack_ms,
            release_ms,
            envelope: 0.0,
            gate_gain: 0.0,
            attack_coeff: exp_coeff(attack_ms, sample_rate),
            release_coeff: exp_coeff(release_ms, sample_rate),
            bypassed: false,
        }
    }
}

impl AudioEffect for Gate {
    fn process(&mut self, samples: &mut [f32]) {
        if self.bypassed {
            return;
        }
        let threshold_lin = db_to_linear(self.threshold_db);

        for s in samples.iter_mut() {
            let abs_s = if *s < 0.0 { -*s } else { *s };
            self.envelope = self.envelope * self.release_coeff + abs_s * (1.0 - self.release_coeff);

            let target = if self.envelope > threshold_lin {
                1.0
            } else {
                0.0
            };
            let coeff = if target > self.gate_gain {
                self.attack_coeff
            } else {
                self.release_coeff
            };
            self.gate_gain = self.gate_gain * coeff + target * (1.0 - coeff);
            *s *= self.gate_gain;
        }
    }
    fn name(&self) -> &str {
        "Gate"
    }
    fn bypass(&self) -> bool {
        self.bypassed
    }
    fn set_bypass(&mut self, bypass: bool) {
        self.bypassed = bypass;
    }
}

/// Schroeder reverb — 4 parallel comb filters + 2 series allpass filters.
/// Compact and efficient for real-time use.
pub struct SchroederReverb {
    pub room_size: f32,
    pub damping: f32,
    pub wet: f32,
    pub dry: f32,
    comb_buffers: [Vec<f32>; 4],
    comb_indices: [usize; 4],
    comb_feedback: [f32; 4],
    comb_damp_state: [f32; 4],
    allpass_buffers: [Vec<f32>; 2],
    allpass_indices: [usize; 2],
    bypassed: bool,
}

const COMB_LENGTHS: [usize; 4] = [1116, 1188, 1277, 1356];
const ALLPASS_LENGTHS: [usize; 2] = [556, 441];

impl SchroederReverb {
    pub fn new(room_size: f32, damping: f32, wet: f32, dry: f32) -> Self {
        Self {
            room_size,
            damping,
            wet,
            dry,
            comb_buffers: [
                vec![0.0; COMB_LENGTHS[0]],
                vec![0.0; COMB_LENGTHS[1]],
                vec![0.0; COMB_LENGTHS[2]],
                vec![0.0; COMB_LENGTHS[3]],
            ],
            comb_indices: [0; 4],
            comb_feedback: [
                room_size * 0.84,
                room_size * 0.82,
                room_size * 0.80,
                room_size * 0.78,
            ],
            comb_damp_state: [0.0; 4],
            allpass_buffers: [vec![0.0; ALLPASS_LENGTHS[0]], vec![0.0; ALLPASS_LENGTHS[1]]],
            allpass_indices: [0; 2],
            bypassed: false,
        }
    }

    pub fn small_room() -> Self {
        Self::new(0.3, 0.5, 0.2, 0.8)
    }
    pub fn large_hall() -> Self {
        Self::new(0.9, 0.3, 0.4, 0.6)
    }

    fn process_comb(&mut self, comb_idx: usize, input: f32) -> f32 {
        let buf = &mut self.comb_buffers[comb_idx];
        let idx = self.comb_indices[comb_idx];
        let output = buf[idx];

        let damp = self.damping;
        self.comb_damp_state[comb_idx] =
            output * (1.0 - damp) + self.comb_damp_state[comb_idx] * damp;

        buf[idx] = input + self.comb_damp_state[comb_idx] * self.comb_feedback[comb_idx];
        self.comb_indices[comb_idx] = (idx + 1) % buf.len();
        output
    }

    fn process_allpass(&mut self, ap_idx: usize, input: f32) -> f32 {
        let buf = &mut self.allpass_buffers[ap_idx];
        let idx = self.allpass_indices[ap_idx];
        let buffered = buf[idx];
        let output = buffered - input;
        buf[idx] = input + buffered * 0.5;
        self.allpass_indices[ap_idx] = (idx + 1) % buf.len();
        output
    }
}

impl AudioEffect for SchroederReverb {
    fn process(&mut self, samples: &mut [f32]) {
        if self.bypassed {
            return;
        }
        let wet = self.wet;
        let dry = self.dry;

        for s in samples.iter_mut() {
            let input = *s;

            let mut comb_sum = 0.0f32;
            for c in 0..4 {
                comb_sum += self.process_comb(c, input);
            }
            comb_sum *= 0.25;

            let mut ap_out = comb_sum;
            for a in 0..2 {
                ap_out = self.process_allpass(a, ap_out);
            }

            *s = input * dry + ap_out * wet;
        }
    }
    fn name(&self) -> &str {
        "Reverb"
    }
    fn bypass(&self) -> bool {
        self.bypassed
    }
    fn set_bypass(&mut self, bypass: bool) {
        self.bypassed = bypass;
    }
}

/// Simple delay effect with feedback.
pub struct DelayEffect {
    pub delay_samples: usize,
    pub feedback: f32,
    pub wet: f32,
    buffer: Vec<f32>,
    write_pos: usize,
    bypassed: bool,
}

impl DelayEffect {
    pub fn new(delay_ms: f32, feedback: f32, wet: f32, sample_rate: f32) -> Self {
        let delay_samples = (delay_ms * sample_rate / 1000.0) as usize;
        Self {
            delay_samples,
            feedback: clamp(feedback, 0.0, 0.95),
            wet,
            buffer: vec![0.0; delay_samples.max(1)],
            write_pos: 0,
            bypassed: false,
        }
    }
}

impl AudioEffect for DelayEffect {
    fn process(&mut self, samples: &mut [f32]) {
        if self.bypassed {
            return;
        }
        let len = self.buffer.len();
        for s in samples.iter_mut() {
            let read_pos = (self.write_pos + len - self.delay_samples) % len;
            let delayed = self.buffer[read_pos];
            self.buffer[self.write_pos] = *s + delayed * self.feedback;
            *s = *s * (1.0 - self.wet) + delayed * self.wet;
            self.write_pos = (self.write_pos + 1) % len;
        }
    }
    fn name(&self) -> &str {
        "Delay"
    }
    fn bypass(&self) -> bool {
        self.bypassed
    }
    fn set_bypass(&mut self, bypass: bool) {
        self.bypassed = bypass;
    }
}

/// Ordered chain of effects applied to a single channel.
pub struct EffectsChain {
    effects: Vec<Box<dyn AudioEffect>>,
}

impl EffectsChain {
    pub fn new() -> Self {
        Self {
            effects: Vec::new(),
        }
    }

    pub fn add(&mut self, effect: Box<dyn AudioEffect>) {
        self.effects.push(effect);
    }

    pub fn remove(&mut self, index: usize) {
        if index < self.effects.len() {
            self.effects.remove(index);
        }
    }

    pub fn set_bypass(&mut self, index: usize, bypass: bool) {
        if let Some(e) = self.effects.get_mut(index) {
            e.set_bypass(bypass);
        }
    }

    pub fn process(&mut self, samples: &mut [f32]) {
        for effect in self.effects.iter_mut() {
            effect.process(samples);
        }
    }

    pub fn effect_count(&self) -> usize {
        self.effects.len()
    }
}

// ---------------------------------------------------------------------------
// Audio Router — VoiceMeeter-class routing
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct InputId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct OutputId(pub u32);

/// Type of virtual input source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VirtualInputKind {
    AppAudio,
    Microphone,
    LoopbackCapture,
}

/// Type of virtual output destination.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VirtualOutputKind {
    Speakers,
    Headphones,
    StreamOutput,
    Recording,
    Monitor,
}

pub struct VirtualInput {
    pub id: InputId,
    pub name: String,
    pub kind: VirtualInputKind,
    pub channels: u16,
    pub buffer: AudioBuffer<f32>,
    pub muted: bool,
    pub gain: f32,
}

impl VirtualInput {
    pub fn new(
        id: InputId,
        name: &str,
        kind: VirtualInputKind,
        channels: u16,
        frames: usize,
    ) -> Self {
        Self {
            id,
            name: String::from(name),
            kind,
            channels,
            buffer: AudioBuffer::new(channels, frames),
            muted: false,
            gain: 1.0,
        }
    }
}

pub struct VirtualOutput {
    pub id: OutputId,
    pub name: String,
    pub kind: VirtualOutputKind,
    pub channels: u16,
    pub buffer: AudioBuffer<f32>,
    pub muted: bool,
    pub gain: f32,
}

impl VirtualOutput {
    pub fn new(
        id: OutputId,
        name: &str,
        kind: VirtualOutputKind,
        channels: u16,
        frames: usize,
    ) -> Self {
        Self {
            id,
            name: String::from(name),
            kind,
            channels,
            buffer: AudioBuffer::new(channels, frames),
            muted: false,
            gain: 1.0,
        }
    }
}

/// A single route from one input to one output with gain, mute, EQ, and compressor.
pub struct Route {
    pub from: InputId,
    pub to: OutputId,
    pub gain: f32,
    pub mute: bool,
    pub eq: ParametricEq,
    pub compressor: Option<Compressor>,
    pub effects_chain: EffectsChain,
}

impl Route {
    pub fn new(from: InputId, to: OutputId, sample_rate: f32) -> Self {
        Self {
            from,
            to,
            gain: 1.0,
            mute: false,
            eq: ParametricEq::default_3band(sample_rate),
            compressor: None,
            effects_chain: EffectsChain::new(),
        }
    }

    pub fn effective_gain(&self) -> f32 {
        if self.mute {
            0.0
        } else {
            self.gain
        }
    }

    pub fn process(&mut self, samples: &mut [f32]) {
        if self.mute {
            return;
        }
        let g = self.gain;
        for s in samples.iter_mut() {
            *s *= g;
        }
        self.eq.process(samples);
        if let Some(ref mut comp) = self.compressor {
            comp.process(samples);
        }
        self.effects_chain.process(samples);
    }
}

/// VoiceMeeter-class audio router with arbitrary input→output routing.
pub struct AudioRouter {
    inputs: Vec<VirtualInput>,
    outputs: Vec<VirtualOutput>,
    routes: Vec<Route>,
    next_input_id: u32,
    next_output_id: u32,
    sample_rate: f32,
    buffer_frames: usize,
    route_buffer: Vec<f32>,
}

impl AudioRouter {
    pub fn new(sample_rate: f32, buffer_frames: usize) -> Self {
        Self {
            inputs: Vec::new(),
            outputs: Vec::new(),
            routes: Vec::new(),
            next_input_id: 1,
            next_output_id: 1,
            sample_rate,
            buffer_frames,
            route_buffer: vec![0.0; buffer_frames * 2],
        }
    }

    pub fn add_input(&mut self, name: &str, kind: VirtualInputKind, channels: u16) -> InputId {
        let id = InputId(self.next_input_id);
        self.next_input_id += 1;
        self.inputs.push(VirtualInput::new(
            id,
            name,
            kind,
            channels,
            self.buffer_frames,
        ));
        id
    }

    pub fn add_output(&mut self, name: &str, kind: VirtualOutputKind, channels: u16) -> OutputId {
        let id = OutputId(self.next_output_id);
        self.next_output_id += 1;
        self.outputs.push(VirtualOutput::new(
            id,
            name,
            kind,
            channels,
            self.buffer_frames,
        ));
        id
    }

    pub fn add_route(&mut self, from: InputId, to: OutputId) -> usize {
        let idx = self.routes.len();
        self.routes.push(Route::new(from, to, self.sample_rate));
        idx
    }

    pub fn remove_route(&mut self, index: usize) {
        if index < self.routes.len() {
            self.routes.remove(index);
        }
    }

    pub fn set_route_gain(&mut self, index: usize, gain: f32) {
        if let Some(r) = self.routes.get_mut(index) {
            r.gain = clamp(gain, 0.0, 4.0);
        }
    }

    pub fn set_route_mute(&mut self, index: usize, mute: bool) {
        if let Some(r) = self.routes.get_mut(index) {
            r.mute = mute;
        }
    }

    pub fn set_route_eq_band(&mut self, route_idx: usize, band_idx: usize, gain_db: f32) {
        if let Some(r) = self.routes.get_mut(route_idx) {
            r.eq.set_band(band_idx, gain_db, self.sample_rate);
        }
    }

    pub fn enable_route_compressor(
        &mut self,
        route_idx: usize,
        threshold_db: f32,
        ratio: f32,
        attack_ms: f32,
        release_ms: f32,
    ) {
        if let Some(r) = self.routes.get_mut(route_idx) {
            r.compressor = Some(Compressor::new(
                threshold_db,
                ratio,
                attack_ms,
                release_ms,
                self.sample_rate,
            ));
        }
    }

    pub fn set_input_gain(&mut self, id: InputId, gain: f32) {
        if let Some(inp) = self.inputs.iter_mut().find(|i| i.id == id) {
            inp.gain = clamp(gain, 0.0, 4.0);
        }
    }

    pub fn set_output_gain(&mut self, id: OutputId, gain: f32) {
        if let Some(out) = self.outputs.iter_mut().find(|o| o.id == id) {
            out.gain = clamp(gain, 0.0, 4.0);
        }
    }

    pub fn set_input_mute(&mut self, id: InputId, muted: bool) {
        if let Some(inp) = self.inputs.iter_mut().find(|i| i.id == id) {
            inp.muted = muted;
        }
    }

    pub fn set_output_mute(&mut self, id: OutputId, muted: bool) {
        if let Some(out) = self.outputs.iter_mut().find(|o| o.id == id) {
            out.muted = muted;
        }
    }

    /// Write audio data into a virtual input's buffer.
    pub fn write_input(&mut self, id: InputId, data: &AudioBuffer<f32>) {
        if let Some(inp) = self.inputs.iter_mut().find(|i| i.id == id) {
            inp.buffer.clear();
            inp.buffer.mix_into(data);
            if !inp.muted {
                inp.buffer.apply_gain(inp.gain);
            }
        }
    }

    /// Process all routes: for each route, copy input → apply route processing → mix into output.
    /// Real-time safe: no allocations; uses pre-allocated route_buffer.
    pub fn process(&mut self) {
        for out in self.outputs.iter_mut() {
            out.buffer.clear();
        }

        for route_idx in 0..self.routes.len() {
            // A muted route contributes nothing — skip before touching anything.
            if self.routes[route_idx].mute {
                continue;
            }
            let from_id = self.routes[route_idx].from;
            let to_id = self.routes[route_idx].to;

            // Copy input data into route scratch buffer
            let buf_len = self.route_buffer.len();
            for s in self.route_buffer.iter_mut() {
                *s = 0.0;
            }

            if let Some(inp) = self.inputs.iter().find(|i| i.id == from_id) {
                if inp.muted {
                    continue;
                }
                let src = inp.buffer.as_slice();
                let n = src.len().min(buf_len);
                self.route_buffer[..n].copy_from_slice(&src[..n]);
            } else {
                continue;
            }

            self.routes[route_idx].process(&mut self.route_buffer);

            if let Some(out) = self.outputs.iter_mut().find(|o| o.id == to_id) {
                if out.muted {
                    continue;
                }
                let dst = out.buffer.as_mut_slice();
                let n = dst.len().min(self.route_buffer.len());
                for i in 0..n {
                    dst[i] += self.route_buffer[i];
                }
            }
        }

        // Apply each output's bus gain ONCE after all routes have summed in
        // (applying it per-route would compound the gain by the route count).
        for out in self.outputs.iter_mut() {
            if out.gain != 1.0 {
                out.buffer.apply_gain(out.gain);
            }
        }
    }

    /// Get the processed output buffer for a given output.
    pub fn output_buffer(&self, id: OutputId) -> Option<&AudioBuffer<f32>> {
        self.outputs.iter().find(|o| o.id == id).map(|o| &o.buffer)
    }

    pub fn input_count(&self) -> usize {
        self.inputs.len()
    }
    pub fn output_count(&self) -> usize {
        self.outputs.len()
    }
    pub fn route_count(&self) -> usize {
        self.routes.len()
    }
}

// ---------------------------------------------------------------------------
// Audio Graph — Node-based processing with topological sort
// ---------------------------------------------------------------------------

/// Trait for nodes in the audio graph. Process reads from `inputs` and writes
/// to `outputs`. Must be allocation-free in the process path.
pub trait AudioNode {
    fn process(
        &mut self,
        inputs: &[&AudioBuffer<f32>],
        outputs: &mut [AudioBuffer<f32>],
        frames: usize,
    );
    fn name(&self) -> &str;
    fn input_count(&self) -> usize;
    fn output_count(&self) -> usize;
}

/// Source node — generates audio (e.g. from a decoded buffer).
pub struct SourceNode {
    pub data: AudioBuffer<f32>,
    pub position: usize,
    pub looping: bool,
}

impl SourceNode {
    pub fn new(channels: u16, frames: usize) -> Self {
        Self {
            data: AudioBuffer::new(channels, frames),
            position: 0,
            looping: false,
        }
    }

    pub fn load(&mut self, samples: &[f32]) {
        let dst = self.data.as_mut_slice();
        let n = samples.len().min(dst.len());
        dst[..n].copy_from_slice(&samples[..n]);
        self.position = 0;
    }
}

impl AudioNode for SourceNode {
    fn process(
        &mut self,
        _inputs: &[&AudioBuffer<f32>],
        outputs: &mut [AudioBuffer<f32>],
        frames: usize,
    ) {
        if outputs.is_empty() {
            return;
        }
        let ch = self.data.channels() as usize;
        let src = self.data.as_slice();
        let dst = outputs[0].as_mut_slice();
        let total = src.len();

        for f in 0..frames {
            for c in 0..ch {
                let src_idx = self.position * ch + c;
                let dst_idx = f * ch + c;
                if src_idx < total && dst_idx < dst.len() {
                    dst[dst_idx] = src[src_idx];
                }
            }
            self.position += 1;
            if self.position * ch >= total {
                if self.looping {
                    self.position = 0;
                } else {
                    break;
                }
            }
        }
    }
    fn name(&self) -> &str {
        "SourceNode"
    }
    fn input_count(&self) -> usize {
        0
    }
    fn output_count(&self) -> usize {
        1
    }
}

/// Gain node for the graph (wraps existing GainNode behavior).
pub struct GraphGainNode {
    gain: f32,
    target_gain: f32,
    smoothing: f32,
}

impl GraphGainNode {
    pub fn new(gain: f32) -> Self {
        Self {
            gain,
            target_gain: gain,
            smoothing: 1.0 / 128.0,
        }
    }

    pub fn set_gain(&mut self, gain: f32) {
        self.target_gain = gain;
    }
}

impl AudioNode for GraphGainNode {
    fn process(
        &mut self,
        inputs: &[&AudioBuffer<f32>],
        outputs: &mut [AudioBuffer<f32>],
        frames: usize,
    ) {
        if inputs.is_empty() || outputs.is_empty() {
            return;
        }
        let src = inputs[0].as_slice();
        let dst = outputs[0].as_mut_slice();
        let ch = inputs[0].channels() as usize;

        for f in 0..frames {
            self.gain += (self.target_gain - self.gain) * self.smoothing;
            let g = self.gain;
            for c in 0..ch {
                let idx = f * ch + c;
                if idx < src.len() && idx < dst.len() {
                    dst[idx] = src[idx] * g;
                }
            }
        }
    }
    fn name(&self) -> &str {
        "GraphGainNode"
    }
    fn input_count(&self) -> usize {
        1
    }
    fn output_count(&self) -> usize {
        1
    }
}

/// Mixer node — sums all inputs into a single output.
pub struct GraphMixerNode {
    input_slots: usize,
}

impl GraphMixerNode {
    pub fn new(input_slots: usize) -> Self {
        Self { input_slots }
    }
}

impl AudioNode for GraphMixerNode {
    fn process(
        &mut self,
        inputs: &[&AudioBuffer<f32>],
        outputs: &mut [AudioBuffer<f32>],
        _frames: usize,
    ) {
        if outputs.is_empty() {
            return;
        }
        outputs[0].clear();
        for inp in inputs {
            outputs[0].mix_into(inp);
        }
    }
    fn name(&self) -> &str {
        "GraphMixerNode"
    }
    fn input_count(&self) -> usize {
        self.input_slots
    }
    fn output_count(&self) -> usize {
        1
    }
}

/// Splitter — copies one input to N outputs.
pub struct SplitterNode {
    output_slots: usize,
}

impl SplitterNode {
    pub fn new(output_slots: usize) -> Self {
        Self { output_slots }
    }
}

impl AudioNode for SplitterNode {
    fn process(
        &mut self,
        inputs: &[&AudioBuffer<f32>],
        outputs: &mut [AudioBuffer<f32>],
        _frames: usize,
    ) {
        if inputs.is_empty() {
            return;
        }
        let src = inputs[0].as_slice();
        for out in outputs.iter_mut() {
            let dst = out.as_mut_slice();
            let n = src.len().min(dst.len());
            dst[..n].copy_from_slice(&src[..n]);
        }
    }
    fn name(&self) -> &str {
        "SplitterNode"
    }
    fn input_count(&self) -> usize {
        1
    }
    fn output_count(&self) -> usize {
        self.output_slots
    }
}

/// Wraps an AudioEffect into an AudioNode for use in the graph.
pub struct EffectNode {
    effect: Box<dyn AudioEffect>,
}

impl EffectNode {
    pub fn new(effect: Box<dyn AudioEffect>) -> Self {
        Self { effect }
    }
}

impl AudioNode for EffectNode {
    fn process(
        &mut self,
        inputs: &[&AudioBuffer<f32>],
        outputs: &mut [AudioBuffer<f32>],
        _frames: usize,
    ) {
        if inputs.is_empty() || outputs.is_empty() {
            return;
        }
        let src = inputs[0].as_slice();
        let dst = outputs[0].as_mut_slice();
        let n = src.len().min(dst.len());
        dst[..n].copy_from_slice(&src[..n]);
        self.effect.process(&mut outputs[0].as_mut_slice()[..n]);
    }
    fn name(&self) -> &str {
        "EffectNode"
    }
    fn input_count(&self) -> usize {
        1
    }
    fn output_count(&self) -> usize {
        1
    }
}

/// Output node — terminal sink in the graph.
pub struct OutputNode {
    pub name_str: String,
}

impl OutputNode {
    pub fn new(name: &str) -> Self {
        Self {
            name_str: String::from(name),
        }
    }
}

impl AudioNode for OutputNode {
    fn process(
        &mut self,
        inputs: &[&AudioBuffer<f32>],
        outputs: &mut [AudioBuffer<f32>],
        _frames: usize,
    ) {
        if inputs.is_empty() || outputs.is_empty() {
            return;
        }
        let src = inputs[0].as_slice();
        let dst = outputs[0].as_mut_slice();
        let n = src.len().min(dst.len());
        dst[..n].copy_from_slice(&src[..n]);
    }
    fn name(&self) -> &str {
        &self.name_str
    }
    fn input_count(&self) -> usize {
        1
    }
    fn output_count(&self) -> usize {
        1
    }
}

/// Graph node handle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GraphNodeId(pub usize);

/// Edge connecting one node's output to another's input.
#[derive(Debug, Clone, Copy)]
struct GraphEdge {
    from: GraphNodeId,
    from_port: usize,
    to: GraphNodeId,
    to_port: usize,
}

/// Node-based audio processing graph with topological sort.
pub struct AudioGraph {
    nodes: Vec<Box<dyn AudioNode>>,
    edges: Vec<GraphEdge>,
    sorted_order: Vec<usize>,
    node_buffers: Vec<Vec<AudioBuffer<f32>>>,
    channels: u16,
    buffer_frames: usize,
    dirty: bool,
}

impl AudioGraph {
    pub fn new(channels: u16, buffer_frames: usize) -> Self {
        Self {
            nodes: Vec::new(),
            edges: Vec::new(),
            sorted_order: Vec::new(),
            node_buffers: Vec::new(),
            channels,
            buffer_frames,
            dirty: true,
        }
    }

    pub fn add_node(&mut self, node: Box<dyn AudioNode>) -> GraphNodeId {
        let id = GraphNodeId(self.nodes.len());
        let output_count = node.output_count().max(1);
        let mut bufs = Vec::with_capacity(output_count);
        for _ in 0..output_count {
            bufs.push(AudioBuffer::new(self.channels, self.buffer_frames));
        }
        self.node_buffers.push(bufs);
        self.nodes.push(node);
        self.dirty = true;
        id
    }

    pub fn connect(
        &mut self,
        from: GraphNodeId,
        from_port: usize,
        to: GraphNodeId,
        to_port: usize,
    ) {
        self.edges.push(GraphEdge {
            from,
            from_port,
            to,
            to_port,
        });
        self.dirty = true;
    }

    /// Kahn's algorithm topological sort.
    fn topo_sort(&mut self) {
        let n = self.nodes.len();
        let mut in_degree = vec![0u32; n];
        for edge in &self.edges {
            in_degree[edge.to.0] += 1;
        }

        let mut queue: Vec<usize> = Vec::new();
        for i in 0..n {
            if in_degree[i] == 0 {
                queue.push(i);
            }
        }

        self.sorted_order.clear();
        while let Some(node_idx) = queue.pop() {
            self.sorted_order.push(node_idx);
            for edge in &self.edges {
                if edge.from.0 == node_idx {
                    in_degree[edge.to.0] -= 1;
                    if in_degree[edge.to.0] == 0 {
                        queue.push(edge.to.0);
                    }
                }
            }
        }

        self.dirty = false;
    }

    /// Process the entire graph for one buffer period.
    /// Uses copied input buffers to satisfy the borrow checker while keeping
    /// the processing loop allocation-light (Vec reuse across ticks is a future
    /// optimization; the copies are small — 128 frames × 2 ch × 4 bytes).
    pub fn process(&mut self) {
        if self.dirty {
            self.topo_sort();
        }

        for bufs in self.node_buffers.iter_mut() {
            for buf in bufs.iter_mut() {
                buf.clear();
            }
        }

        let order = self.sorted_order.clone();
        for &node_idx in &order {
            // Gather incoming edges, ordered by destination input port so a
            // multi-input node receives its inputs in a deterministic, correct
            // slot order (port 0 first, then 1, ...).
            let mut incoming: Vec<(usize, AudioBuffer<f32>)> = Vec::new();
            for edge in &self.edges {
                if edge.to.0 == node_idx {
                    if let Some(bufs) = self.node_buffers.get(edge.from.0) {
                        if let Some(buf) = bufs.get(edge.from_port) {
                            let mut copy = AudioBuffer::new(buf.channels(), buf.frames());
                            copy.as_mut_slice().copy_from_slice(buf.as_slice());
                            incoming.push((edge.to_port, copy));
                        }
                    }
                }
            }
            incoming.sort_by_key(|(port, _)| *port);
            let input_copies: Vec<AudioBuffer<f32>> =
                incoming.into_iter().map(|(_, buf)| buf).collect();

            let input_refs: Vec<&AudioBuffer<f32>> = input_copies.iter().collect();
            let mut outputs = core::mem::take(&mut self.node_buffers[node_idx]);

            self.nodes[node_idx].process(&input_refs, &mut outputs, self.buffer_frames);

            self.node_buffers[node_idx] = outputs;
        }
    }

    /// Get the output buffer of a specific node/port.
    pub fn node_output(&self, id: GraphNodeId, port: usize) -> Option<&AudioBuffer<f32>> {
        self.node_buffers.get(id.0).and_then(|bufs| bufs.get(port))
    }

    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }
    pub fn edge_count(&self) -> usize {
        self.edges.len()
    }
}

// ---------------------------------------------------------------------------
// Device Manager
// ---------------------------------------------------------------------------

/// Physical audio device type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioDeviceType {
    Output,
    Input,
    Loopback,
}

/// Describes a discovered audio device.
#[derive(Debug, Clone)]
pub struct AudioDeviceDescriptor {
    pub id: DeviceId,
    pub name: String,
    pub device_type: AudioDeviceType,
    pub sample_rates: Vec<u32>,
    pub channel_count: u16,
    pub format: AudioFormat,
    pub is_default: bool,
    pub is_connected: bool,
}

impl AudioDeviceDescriptor {
    pub fn new(
        id: DeviceId,
        name: &str,
        device_type: AudioDeviceType,
        channel_count: u16,
        format: AudioFormat,
    ) -> Self {
        Self {
            id,
            name: String::from(name),
            device_type,
            sample_rates: vec![44100, 48000, 96000, 192000],
            channel_count,
            format,
            is_default: false,
            is_connected: true,
        }
    }

    pub fn supports_rate(&self, rate: u32) -> bool {
        self.sample_rates.contains(&rate)
    }
}

/// Per-app device routing assignment.
#[derive(Debug, Clone)]
pub struct AppDeviceBinding {
    pub app_name: String,
    pub output_device: DeviceId,
    pub input_device: Option<DeviceId>,
}

/// Manages audio device enumeration, default selection, hot-plug, and per-app routing.
pub struct DeviceManager {
    devices: Vec<AudioDeviceDescriptor>,
    default_output: Option<DeviceId>,
    default_input: Option<DeviceId>,
    app_bindings: Vec<AppDeviceBinding>,
    next_device_id: u64,
    hotplug_generation: u64,
}

impl DeviceManager {
    pub fn new() -> Self {
        Self {
            devices: Vec::new(),
            default_output: None,
            default_input: None,
            app_bindings: Vec::new(),
            next_device_id: 1,
            hotplug_generation: 0,
        }
    }

    pub fn add_device(
        &mut self,
        name: &str,
        device_type: AudioDeviceType,
        channel_count: u16,
        format: AudioFormat,
    ) -> DeviceId {
        let id = DeviceId(self.next_device_id);
        self.next_device_id += 1;
        let mut desc = AudioDeviceDescriptor::new(id, name, device_type, channel_count, format);

        if self.default_output.is_none() && device_type == AudioDeviceType::Output {
            desc.is_default = true;
            self.default_output = Some(id);
        }
        if self.default_input.is_none() && device_type == AudioDeviceType::Input {
            desc.is_default = true;
            self.default_input = Some(id);
        }

        self.devices.push(desc);
        id
    }

    pub fn remove_device(&mut self, id: DeviceId) {
        self.devices.retain(|d| d.id != id);
        if self.default_output == Some(id) {
            self.default_output = self
                .devices
                .iter()
                .find(|d| d.device_type == AudioDeviceType::Output && d.is_connected)
                .map(|d| d.id);
        }
        if self.default_input == Some(id) {
            self.default_input = self
                .devices
                .iter()
                .find(|d| d.device_type == AudioDeviceType::Input && d.is_connected)
                .map(|d| d.id);
        }
        self.hotplug_generation += 1;
    }

    /// Simulate USB audio hot-plug: mark device as connected/disconnected.
    pub fn set_connected(&mut self, id: DeviceId, connected: bool) {
        if let Some(dev) = self.devices.iter_mut().find(|d| d.id == id) {
            dev.is_connected = connected;
            self.hotplug_generation += 1;

            if !connected {
                if self.default_output == Some(id) {
                    self.default_output = self
                        .devices
                        .iter()
                        .find(|d| {
                            d.device_type == AudioDeviceType::Output && d.is_connected && d.id != id
                        })
                        .map(|d| d.id);
                }
                if self.default_input == Some(id) {
                    self.default_input = self
                        .devices
                        .iter()
                        .find(|d| {
                            d.device_type == AudioDeviceType::Input && d.is_connected && d.id != id
                        })
                        .map(|d| d.id);
                }
            }
        }
    }

    pub fn set_default_output(&mut self, id: DeviceId) {
        for dev in self.devices.iter_mut() {
            dev.is_default = dev.id == id && dev.device_type == AudioDeviceType::Output;
        }
        self.default_output = Some(id);
    }

    pub fn set_default_input(&mut self, id: DeviceId) {
        for dev in self.devices.iter_mut() {
            if dev.device_type == AudioDeviceType::Input {
                dev.is_default = dev.id == id;
            }
        }
        self.default_input = Some(id);
    }

    pub fn default_output(&self) -> Option<&AudioDeviceDescriptor> {
        self.default_output
            .and_then(|id| self.devices.iter().find(|d| d.id == id))
    }

    pub fn default_input(&self) -> Option<&AudioDeviceDescriptor> {
        self.default_input
            .and_then(|id| self.devices.iter().find(|d| d.id == id))
    }

    pub fn list_devices(&self) -> &[AudioDeviceDescriptor] {
        &self.devices
    }

    pub fn output_devices(&self) -> Vec<&AudioDeviceDescriptor> {
        self.devices
            .iter()
            .filter(|d| d.device_type == AudioDeviceType::Output && d.is_connected)
            .collect()
    }

    pub fn input_devices(&self) -> Vec<&AudioDeviceDescriptor> {
        self.devices
            .iter()
            .filter(|d| d.device_type == AudioDeviceType::Input && d.is_connected)
            .collect()
    }

    /// Bind an app to specific output/input devices (e.g. game → headphones, Discord → speakers).
    pub fn bind_app(&mut self, app_name: &str, output: DeviceId, input: Option<DeviceId>) {
        if let Some(binding) = self
            .app_bindings
            .iter_mut()
            .find(|b| b.app_name == app_name)
        {
            binding.output_device = output;
            binding.input_device = input;
        } else {
            self.app_bindings.push(AppDeviceBinding {
                app_name: String::from(app_name),
                output_device: output,
                input_device: input,
            });
        }
    }

    pub fn unbind_app(&mut self, app_name: &str) {
        self.app_bindings.retain(|b| b.app_name != app_name);
    }

    /// Resolve which output device an app should use.
    pub fn resolve_output(&self, app_name: &str) -> Option<DeviceId> {
        self.app_bindings
            .iter()
            .find(|b| b.app_name == app_name)
            .map(|b| b.output_device)
            .or(self.default_output)
    }

    /// Resolve which input device an app should use.
    pub fn resolve_input(&self, app_name: &str) -> Option<DeviceId> {
        self.app_bindings
            .iter()
            .find(|b| b.app_name == app_name)
            .and_then(|b| b.input_device)
            .or(self.default_input)
    }

    pub fn hotplug_generation(&self) -> u64 {
        self.hotplug_generation
    }
    pub fn device_count(&self) -> usize {
        self.devices.len()
    }
}

// ---------------------------------------------------------------------------
// Game audio helpers
// ---------------------------------------------------------------------------

/// A short, fully-decoded sample intended for one-shot playback (UI clicks,
/// gunshots, footsteps, …).
pub struct SoundEffect {
    pub buffer: AudioBuffer<f32>,
    pub sample_rate: u32,
    pub looping: bool,
}

impl SoundEffect {
    pub fn from_buffer(buffer: AudioBuffer<f32>, sample_rate: u32) -> Self {
        Self {
            buffer,
            sample_rate,
            looping: false,
        }
    }
}

/// Handle to a longer piece of audio that is decoded / streamed in chunks
/// rather than loaded entirely into memory.
pub struct MusicTrack {
    pub name: String,
    pub sample_rate: u32,
    pub channels: u16,
    pub position: usize,
    pub total_frames: usize,
    pub playing: bool,
    pub volume: f32,
}

impl MusicTrack {
    pub fn new(name: String, sample_rate: u32, channels: u16, total_frames: usize) -> Self {
        Self {
            name,
            sample_rate,
            channels,
            position: 0,
            total_frames,
            playing: false,
            volume: 1.0,
        }
    }

    pub fn play(&mut self) {
        self.playing = true;
    }

    pub fn pause(&mut self) {
        self.playing = false;
    }

    pub fn seek(&mut self, frame: usize) {
        self.position = frame;
    }
}

/// 3-D listener used for spatial / positional audio.
///
/// Coordinates follow a right-handed Y-up convention consistent with common
/// game-engine conventions.
pub struct AudioListener {
    pub position: [f32; 3],
    pub forward: [f32; 3],
    pub up: [f32; 3],
}

impl AudioListener {
    pub fn new() -> Self {
        Self {
            position: [0.0, 0.0, 0.0],
            forward: [0.0, 0.0, -1.0],
            up: [0.0, 1.0, 0.0],
        }
    }

    /// Compute the squared distance to a sound source (avoids a sqrt).
    pub fn distance_sq(&self, source: &[f32; 3]) -> f32 {
        let dx = self.position[0] - source[0];
        let dy = self.position[1] - source[1];
        let dz = self.position[2] - source[2];
        dx * dx + dy * dy + dz * dz
    }
}

impl Default for AudioListener {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Utility helpers (no_std friendly)
// ---------------------------------------------------------------------------

#[inline]
fn clamp(v: f32, lo: f32, hi: f32) -> f32 {
    if v < lo {
        lo
    } else if v > hi {
        hi
    } else {
        v
    }
}

/// Convert dB to linear gain.
#[inline]
fn db_to_linear(db: f32) -> f32 {
    // 10^(db/20) ≈ e^(db * ln(10)/20)
    fast_exp(db * 0.11512925)
}

/// Fast exponential approximation (Schraudolph's method).
#[inline]
fn fast_exp(x: f32) -> f32 {
    let clipped = clamp(x, -87.0, 88.0);
    let v = (12102203.0 * clipped + 1065353216.0) as i32;
    f32::from_bits(v as u32)
}

/// Fast power approximation: x^p ≈ exp(p * ln(x)).
#[inline]
fn fast_pow(x: f32, p: f32) -> f32 {
    if x <= 0.0 {
        return 0.0;
    }
    fast_exp(p * fast_ln(x))
}

/// Fast natural log approximation.
#[inline]
fn fast_ln(x: f32) -> f32 {
    let bits = x.to_bits() as f32;
    (bits - 1065353216.0) / 12102203.0
}

/// Compute exponential smoothing coefficient from time constant in ms.
#[inline]
fn exp_coeff(time_ms: f32, sample_rate: f32) -> f32 {
    if time_ms <= 0.0 {
        return 0.0;
    }
    let samples = time_ms * sample_rate / 1000.0;
    1.0 - 1.0 / samples
}

/// Fast sin / cos approximation good enough for audio panning (< 0.001 error
/// over [0, pi/2]).
#[inline]
fn sin_cos_approx(x: f32) -> (f32, f32) {
    (sin_approx(x), sin_approx(core::f32::consts::FRAC_PI_2 - x))
}

#[inline]
fn sin_approx(x: f32) -> f32 {
    let x2 = x * x;
    x * (1.0 - x2 / 6.0 * (1.0 - x2 / 20.0))
}

// ---------------------------------------------------------------------------
// R10 Artifacts
// ---------------------------------------------------------------------------

static mut ENGINE: Option<AudioEngine> = None;

/// Initialize the global audio engine.
///
/// MasterChecklist Phase 7.2: "In-kernel audio mixer (priority over background
/// apps in game mode)". This engine provides the core mixing logic.
pub fn init() {
    unsafe {
        ENGINE = Some(AudioEngine::new(2, 128));
    }
}

pub fn engine() -> &'static mut AudioEngine {
    // SAFETY: single-threaded boot init sets ENGINE before any caller; the
    // raw-pointer form avoids the `static_mut_refs` UB-shaped pattern while
    // preserving the existing &'static mut accessor contract.
    unsafe {
        (*core::ptr::addr_of_mut!(ENGINE))
            .as_mut()
            .expect("AthAudio not initialized")
    }
}

/// Prove behavioral correctness of the mixer and effects chain.
pub fn run_boot_smoketest() -> bool {
    let mut mixer = MixerNode::new();

    // 1. Test Mixer
    let idx1 = mixer.add_input(2, 64);
    let idx2 = mixer.add_input(2, 64);

    {
        let buf1 = mixer.input_buffer_mut(idx1);
        for s in buf1.as_mut_slice() {
            *s = 0.5;
        }
        let buf2 = mixer.input_buffer_mut(idx2);
        for s in buf2.as_mut_slice() {
            *s = 0.2;
        }
    }

    let mut out = AudioBuffer::new(2, 64);
    mixer.process(&AudioBuffer::new(2, 64), &mut out);

    let mixer_ok = out.as_slice().iter().all(|&s| (s - 0.7).abs() < 0.001);

    // 2. Test Effects
    let mut chain = EffectsChain::new();
    chain.add(Box::new(GainEffect::new(0.5)));

    let mut samples = [1.0f32; 10];
    chain.process(&mut samples);
    let effects_ok = samples.iter().all(|&s| (s - 0.5).abs() < 0.001);

    // 3. Test Graph
    let mut graph = AudioGraph::new(2, 64);
    let src_id = graph.add_node(Box::new(SourceNode::new(2, 64)));
    let out_id = graph.add_node(Box::new(OutputNode::new("Main Out")));
    graph.connect(src_id, 0, out_id, 0);

    graph.process();
    let graph_ok = graph.node_output(out_id, 0).is_some();

    // 4. Test the game-priority mixer hot path: two voices sum, and game-mode
    //    ducking attenuates a background voice while the game voice is intact.
    let mut gm = mixer::GameMixer::new(1, 16);
    gm.set_limit_master(false);
    gm.set_duck(0.5, 0.25);
    let game = gm.add_stream("game", 1, mixer::StreamPriority::Game);
    let bg = gm.add_stream("chat", 1, mixer::StreamPriority::Background);
    if let Some(b) = gm.stream_buffer_mut(game) {
        for s in b {
            *s = 0.4;
        }
    }
    if let Some(b) = gm.stream_buffer_mut(bg) {
        for s in b {
            *s = 0.4;
        }
    }
    gm.set_game_mode(true);
    let out = gm.mix();
    // game 0.4 + ducked bg 0.4*0.25=0.1 => 0.5
    let game_mixer_ok = out.iter().all(|&v| (v - 0.5).abs() < 1e-3);

    mixer_ok && effects_ok && graph_ok && game_mixer_ok
}

#[cfg(test)]
mod lib_tests {
    use super::*;

    /// REGRESSION: integer PCM mix must saturate, not clip-wrap. Two full-scale
    /// i16 voices summed with plain `+` wrap to -2 (a loud click); the saturating
    /// path must hold at i16::MAX.
    #[test]
    fn i16_mix_saturates_not_wraps() {
        let mut dst: AudioBuffer<i16> = AudioBuffer::new(1, 4);
        let mut src: AudioBuffer<i16> = AudioBuffer::new(1, 4);
        for s in dst.as_mut_slice() {
            *s = i16::MAX;
        }
        for s in src.as_mut_slice() {
            *s = i16::MAX;
        }
        dst.mix_into(&src);
        for &v in dst.as_slice() {
            assert_eq!(v, i16::MAX, "i16 mix wrapped to {v} instead of saturating");
        }

        // Negative side too.
        let mut dn: AudioBuffer<i16> = AudioBuffer::new(1, 2);
        let mut sn: AudioBuffer<i16> = AudioBuffer::new(1, 2);
        for s in dn.as_mut_slice() {
            *s = i16::MIN;
        }
        for s in sn.as_mut_slice() {
            *s = i16::MIN;
        }
        dn.mix_into(&sn);
        for &v in dn.as_slice() {
            assert_eq!(v, i16::MIN, "i16 negative mix wrapped to {v}");
        }
    }

    /// i32 PCM also saturates instead of wrapping.
    #[test]
    fn i32_mix_saturates() {
        let mut dst: AudioBuffer<i32> = AudioBuffer::new(2, 2);
        let mut src: AudioBuffer<i32> = AudioBuffer::new(2, 2);
        for s in dst.as_mut_slice() {
            *s = i32::MAX;
        }
        for s in src.as_mut_slice() {
            *s = 1000;
        }
        dst.mix_into(&src);
        for &v in dst.as_slice() {
            assert_eq!(v, i32::MAX, "i32 mix overflowed to {v}");
        }
    }

    /// f32 mix sums normally (headroom is unbounded; a limiter handles ceiling).
    #[test]
    fn f32_mix_sums_with_headroom() {
        let mut dst: AudioBuffer<f32> = AudioBuffer::new(2, 2);
        let mut src: AudioBuffer<f32> = AudioBuffer::new(2, 2);
        for s in dst.as_mut_slice() {
            *s = 0.8;
        }
        for s in src.as_mut_slice() {
            *s = 0.5;
        }
        dst.mix_into(&src);
        for &v in dst.as_slice() {
            assert!(
                (v - 1.3).abs() < 1e-6,
                "f32 mix = {v}, expected 1.3 headroom"
            );
        }
    }

    /// Mono→stereo upmix duplicates the mono sample to both channels.
    #[test]
    fn mono_to_stereo_upmix_in_buffer() {
        let mut stereo: AudioBuffer<f32> = AudioBuffer::new(2, 3);
        let mut mono: AudioBuffer<f32> = AudioBuffer::new(1, 3);
        for (f, s) in mono.as_mut_slice().iter_mut().enumerate() {
            *s = 0.1 * (f as f32 + 1.0);
        }
        stereo.mix_into(&mono);
        for f in 0..3 {
            let expect = 0.1 * (f as f32 + 1.0);
            assert!((stereo.sample(f, 0) - expect).abs() < 1e-6);
            assert!((stereo.sample(f, 1) - expect).abs() < 1e-6);
        }
    }

    /// MixerNode sums two equal-format f32 inputs.
    #[test]
    fn mixer_node_sums_inputs() {
        let mut mixer = MixerNode::new();
        let a = mixer.add_input(2, 8);
        let b = mixer.add_input(2, 8);
        for s in mixer.input_buffer_mut(a).as_mut_slice() {
            *s = 0.3;
        }
        for s in mixer.input_buffer_mut(b).as_mut_slice() {
            *s = 0.2;
        }
        let mut out = AudioBuffer::new(2, 8);
        mixer.process(&AudioBuffer::new(2, 8), &mut out);
        for &v in out.as_slice() {
            assert!((v - 0.5).abs() < 1e-6, "got {v}");
        }
    }

    /// GainNode converges to its target gain after smoothing.
    #[test]
    fn gain_node_converges_to_target() {
        let mut g = GainNode::new(0.0);
        g.set_smoothing_frames(8);
        g.set_gain(1.0);
        let mut input: AudioBuffer<f32> = AudioBuffer::new(1, 256);
        for s in input.as_mut_slice() {
            *s = 1.0;
        }
        let mut out = AudioBuffer::new(1, 256);
        g.process(&input, &mut out);
        // First frame should be well below target (still ramping)...
        assert!(out.sample(0, 0) < 0.5, "no ramp: {}", out.sample(0, 0));
        // ...and the tail should have converged very close to 1.0.
        assert!(
            (out.sample(255, 0) - 1.0).abs() < 1e-3,
            "did not converge: {}",
            out.sample(255, 0)
        );
    }

    /// ClipNode hard-clamps f32 to the threshold.
    #[test]
    fn clip_node_clamps() {
        let mut c = ClipNode::new(0.5);
        let mut input: AudioBuffer<f32> = AudioBuffer::new(1, 4);
        let vals = [2.0f32, -2.0, 0.25, -0.25];
        for (i, s) in input.as_mut_slice().iter_mut().enumerate() {
            *s = vals[i];
        }
        let mut out = AudioBuffer::new(1, 4);
        c.process(&input, &mut out);
        assert_eq!(out.sample(0, 0), 0.5);
        assert_eq!(out.sample(1, 0), -0.5);
        assert!((out.sample(2, 0) - 0.25).abs() < 1e-6);
        assert!((out.sample(3, 0) + 0.25).abs() < 1e-6);
    }

    /// AudioRouter routes one input to one output and applies route gain.
    #[test]
    fn router_routes_input_to_output_with_gain() {
        let mut r = AudioRouter::new(48_000.0, 4);
        let mic = r.add_input("mic", VirtualInputKind::Microphone, 2);
        let spk = r.add_output("spk", VirtualOutputKind::Speakers, 2);
        let route = r.add_route(mic, spk);
        r.set_route_gain(route, 0.5);

        let mut data: AudioBuffer<f32> = AudioBuffer::new(2, 4);
        for s in data.as_mut_slice() {
            *s = 1.0;
        }
        r.write_input(mic, &data);
        r.process();

        let out = r.output_buffer(spk).expect("output exists");
        // route gain 0.5 (EQ flat at 0 dB is unity, output gain unity)
        for &v in out.as_slice() {
            assert!((v - 0.5).abs() < 1e-3, "router out = {v}");
        }
    }

    /// AudioRouter sums two inputs routed to the same output (matrix mixing).
    #[test]
    fn router_sums_two_inputs_to_one_output() {
        let mut r = AudioRouter::new(48_000.0, 2);
        let a = r.add_input("a", VirtualInputKind::AppAudio, 2);
        let b = r.add_input("b", VirtualInputKind::AppAudio, 2);
        let spk = r.add_output("spk", VirtualOutputKind::Speakers, 2);
        r.add_route(a, spk);
        r.add_route(b, spk);

        let mut da: AudioBuffer<f32> = AudioBuffer::new(2, 2);
        for s in da.as_mut_slice() {
            *s = 0.3;
        }
        let mut db: AudioBuffer<f32> = AudioBuffer::new(2, 2);
        for s in db.as_mut_slice() {
            *s = 0.2;
        }
        r.write_input(a, &da);
        r.write_input(b, &db);
        r.process();

        let out = r.output_buffer(spk).unwrap();
        for &v in out.as_slice() {
            assert!((v - 0.5).abs() < 1e-3, "summed router out = {v}");
        }
    }

    /// A muted route contributes nothing to its output.
    #[test]
    fn router_muted_route_is_silent() {
        let mut r = AudioRouter::new(48_000.0, 2);
        let a = r.add_input("a", VirtualInputKind::AppAudio, 2);
        let spk = r.add_output("spk", VirtualOutputKind::Speakers, 2);
        let route = r.add_route(a, spk);
        r.set_route_mute(route, true);

        let mut da: AudioBuffer<f32> = AudioBuffer::new(2, 2);
        for s in da.as_mut_slice() {
            *s = 1.0;
        }
        r.write_input(a, &da);
        r.process();
        let out = r.output_buffer(spk).unwrap();
        for &v in out.as_slice() {
            assert_eq!(v, 0.0, "muted route leaked {v}");
        }
    }

    /// REGRESSION: output bus gain must be applied once, not once per route.
    /// Two routes into one output at gain 0.5 must scale the SUM by 0.5, not
    /// 0.5^2 (which the old per-route application produced).
    #[test]
    fn router_output_gain_applied_once() {
        let mut r = AudioRouter::new(48_000.0, 2);
        let a = r.add_input("a", VirtualInputKind::AppAudio, 2);
        let b = r.add_input("b", VirtualInputKind::AppAudio, 2);
        let spk = r.add_output("spk", VirtualOutputKind::Speakers, 2);
        r.add_route(a, spk);
        r.add_route(b, spk);
        r.set_output_gain(spk, 0.5);

        let mut da: AudioBuffer<f32> = AudioBuffer::new(2, 2);
        for s in da.as_mut_slice() {
            *s = 0.4;
        }
        let mut db: AudioBuffer<f32> = AudioBuffer::new(2, 2);
        for s in db.as_mut_slice() {
            *s = 0.4;
        }
        r.write_input(a, &da);
        r.write_input(b, &db);
        r.process();

        let out = r.output_buffer(spk).unwrap();
        // (0.4 + 0.4) * 0.5 = 0.4 ; the bug would give ~0.2 (0.8 * 0.5 * 0.5).
        for &v in out.as_slice() {
            assert!((v - 0.4).abs() < 1e-3, "output gain compounded: {v}");
        }
    }

    /// The boot smoketest passes (and is wired through real summing logic).
    #[test]
    fn boot_smoketest_passes() {
        assert!(run_boot_smoketest());
    }
}
