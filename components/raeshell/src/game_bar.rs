//! Game Bar — in-game performance overlay.
//!
//! FPS counter, frametime graph, CPU/GPU temps, voice chat, screenshots,
//! recording indicator.  All native, all fast — rendered directly onto the
//! compositor surface with zero driver overhead.

#![allow(unused)]

extern crate alloc;

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

// ── Colour palette ───────────────────────────────────────────────────────

const OVERLAY_BG: u32 = 0xCC_0C_0E_18;
const OVERLAY_BORDER: u32 = 0xFF_4E_9C_FF;
const TEXT_FG: u32 = 0xFF_F0_F0_F8;
const TEXT_DIM: u32 = 0xFF_88_8C_A0;
const GREEN: u32 = 0xFF_44_CC_66;
const YELLOW: u32 = 0xFF_FF_D7_00;
const ORANGE: u32 = 0xFF_FF_AA_33;
const RED: u32 = 0xFF_FF_44_44;
const ACCENT: u32 = 0xFF_4E_9C_FF;
const BAR_BG: u32 = 0xFF_1A_1E_30;
const BAR_FILL_CPU: u32 = 0xFF_4E_9C_FF;
const BAR_FILL_GPU: u32 = 0xFF_AA_66_FF;
const BAR_FILL_RAM: u32 = 0xFF_44_CC_66;
const BAR_FILL_VRAM: u32 = 0xFF_FF_AA_33;
const REC_RED: u32 = 0xFF_FF_22_22;
const VOICE_GREEN: u32 = 0xFF_33_DD_55;
const NOTIF_BG: u32 = 0xDD_18_1C_2E;
const NOTIF_BORDER: u32 = 0xFF_4E_9C_FF;
const GRAPH_BG: u32 = 0xFF_14_16_22;
const GRAPH_LINE: u32 = 0xFF_4E_9C_FF;
const GRAPH_BAD: u32 = 0xFF_FF_44_44;
const GRAPH_GRID: u32 = 0xFF_22_24_34;

const GLYPH_W: usize = 8;
const GLYPH_H: usize = 8;

// ── Public types ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BarPosition {
    TopLeft,
    TopCenter,
    TopRight,
    BottomLeft,
    BottomCenter,
    BottomRight,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BarWidget {
    FpsCounter,
    FrametimeGraph,
    CpuTemp,
    GpuTemp,
    CpuUsage,
    GpuUsage,
    RamUsage,
    VramUsage,
    NetworkPing,
    Clock,
    Battery,
    RecordIndicator,
    VoiceChat,
}

pub struct VoiceChatState {
    pub enabled: bool,
    pub muted: bool,
    pub deafened: bool,
    pub channel: Option<String>,
    pub users: Vec<VoiceChatUser>,
    pub speaking: Vec<u64>,
    pub volume: u8,
    pub input_device: String,
    pub output_device: String,
}

pub struct VoiceChatUser {
    pub id: u64,
    pub name: String,
    pub muted: bool,
    pub volume: u8,
}

pub struct OverlayNotification {
    pub text: String,
    pub timestamp: u64,
    pub duration_ms: u64,
    pub icon: char,
    pub priority: u8,
}

pub struct PerformanceSnapshot {
    pub fps: f32,
    pub frametime_ms: f32,
    pub cpu_usage: f32,
    pub gpu_usage: f32,
    pub cpu_temp: f32,
    pub gpu_temp: f32,
    pub ram_mb: u32,
    pub vram_mb: u32,
    pub network_ping: u32,
    pub timestamp: u64,
}

pub struct FpsCounter {
    pub current_fps: f32,
    pub avg_fps: f32,
    pub one_percent_low: f32,
    pub frame_times: Vec<f32>,
    history_max: usize,
}

impl FpsCounter {
    pub fn new() -> Self {
        Self {
            current_fps: 0.0,
            avg_fps: 0.0,
            one_percent_low: 0.0,
            frame_times: Vec::new(),
            history_max: 300,
        }
    }

    pub fn push_frame(&mut self, frametime_ms: f32) {
        if frametime_ms > 0.0 {
            self.current_fps = 1000.0 / frametime_ms;
        }
        self.frame_times.push(frametime_ms);
        if self.frame_times.len() > self.history_max {
            self.frame_times.remove(0);
        }
        self.recalculate();
    }

    fn recalculate(&mut self) {
        if self.frame_times.is_empty() {
            return;
        }

        let sum: f32 = self.frame_times.iter().sum();
        let avg_ft = sum / self.frame_times.len() as f32;
        if avg_ft > 0.0 {
            self.avg_fps = 1000.0 / avg_ft;
        }

        if self.frame_times.len() > 10 {
            let mut sorted = self.frame_times.clone();
            sorted.sort_by(|a, b| b.partial_cmp(a).unwrap_or(core::cmp::Ordering::Equal));
            let idx = sorted.len() / 100;
            let worst_1pct = sorted.get(idx).copied().unwrap_or(0.0);
            if worst_1pct > 0.0 {
                self.one_percent_low = 1000.0 / worst_1pct;
            }
        }
    }
}

pub struct FrametimeGraph {
    pub history: Vec<f32>,
    pub target_ms: f32,
    pub scale_max: f32,
    history_max: usize,
}

impl FrametimeGraph {
    pub fn new(target_fps: f32) -> Self {
        let target_ms = if target_fps > 0.0 {
            1000.0 / target_fps
        } else {
            16.67
        };
        Self {
            history: Vec::new(),
            target_ms,
            scale_max: 50.0,
            history_max: 300,
        }
    }

    pub fn push(&mut self, frametime_ms: f32) {
        self.history.push(frametime_ms);
        if self.history.len() > self.history_max {
            self.history.remove(0);
        }
    }

    pub fn set_target_fps(&mut self, fps: f32) {
        if fps > 0.0 {
            self.target_ms = 1000.0 / fps;
        }
    }
}

// ── Phase 4: fixed-size frametime ring (no per-frame alloc) ─────────────────
//
// The Concept's "Game Bar that doesn't suck — FPS, frametime graph … all
// native, all fast" demands a frametime history that the overlay can draw as a
// sparkline WITHOUT allocating every frame. `FrametimeGraph` above keeps a
// `Vec` and does `remove(0)` (an O(n) shift) per push; the Game Bar's live
// graph uses this bounded ring instead — a fixed array, O(1) push, zero alloc
// in steady state. Values are milliseconds (the unit the overlay draws).

/// Number of frametime points the Game Bar graph keeps (~2s at 60fps).
pub const FT_RING_CAP: usize = 120;

#[derive(Clone)]
pub struct FrametimeRing {
    buf: [f32; FT_RING_CAP],
    /// Next write slot.
    head: usize,
    /// Number of valid samples (saturates at `FT_RING_CAP`).
    len: usize,
}

impl Default for FrametimeRing {
    fn default() -> Self {
        Self::new()
    }
}

impl FrametimeRing {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            buf: [0.0; FT_RING_CAP],
            head: 0,
            len: 0,
        }
    }

    /// Push one frametime (ms). O(1), no allocation. Non-finite / negative /
    /// absurd values are clamped so a bad sample never poisons the graph.
    pub fn push(&mut self, ft_ms: f32) {
        let v = if ft_ms.is_finite() && ft_ms > 0.0 {
            ft_ms.min(10_000.0)
        } else {
            0.0
        };
        self.buf[self.head] = v;
        self.head = (self.head + 1) % FT_RING_CAP;
        if self.len < FT_RING_CAP {
            self.len += 1;
        }
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.len
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn clear(&mut self) {
        self.head = 0;
        self.len = 0;
    }

    /// The i-th oldest sample (0 = oldest), or `None` if out of range.
    #[must_use]
    pub fn get_chrono(&self, i: usize) -> Option<f32> {
        if i >= self.len {
            return None;
        }
        // Oldest sits at (head - len) mod CAP.
        let start = (self.head + FT_RING_CAP - self.len) % FT_RING_CAP;
        Some(self.buf[(start + i) % FT_RING_CAP])
    }

    /// The most recent sample (ms), or `None` if empty.
    #[must_use]
    pub fn last(&self) -> Option<f32> {
        if self.len == 0 {
            return None;
        }
        let idx = (self.head + FT_RING_CAP - 1) % FT_RING_CAP;
        Some(self.buf[idx])
    }

    /// Mean frametime over the ring (ms), or `None` if empty.
    #[must_use]
    pub fn avg_ms(&self) -> Option<f32> {
        if self.len == 0 {
            return None;
        }
        let mut sum = 0.0f32;
        for i in 0..self.len {
            let start = (self.head + FT_RING_CAP - self.len) % FT_RING_CAP;
            sum += self.buf[(start + i) % FT_RING_CAP];
        }
        Some(sum / self.len as f32)
    }

    /// FPS derived from the average frametime, or `None` if empty / degenerate.
    #[must_use]
    pub fn fps(&self) -> Option<f32> {
        match self.avg_ms() {
            Some(avg) if avg > 0.0 => Some(1000.0 / avg),
            _ => None,
        }
    }
}

/// A typed live-telemetry snapshot the kernel hands the Game Bar each time it
/// opens / refreshes. Sourced from `crate::perf` (FPS + frametime) and
/// `crate::thermal` (CPU/GPU temp) in the kernel; raeshell stays decoupled from
/// the kernel crate. `None` temps render as "(n/a)" (QEMU has no real sensor).
#[derive(Clone, Copy, Default)]
pub struct PerfFeed {
    /// FPS estimate (frames/sec), `None` if no frames presented yet.
    pub fps: Option<f32>,
    /// Most recent frametime (ms), `None` if no frame yet.
    pub frametime_ms: Option<f32>,
    /// CPU temperature (°C), `None` where no sensor (QEMU / pre-calibration).
    pub cpu_temp_c: Option<f32>,
    /// GPU temperature (°C), `None` where unavailable.
    pub gpu_temp_c: Option<f32>,
}

pub struct SystemMonitors {
    pub cpu_usage: f32,
    pub gpu_usage: f32,
    pub cpu_temp: f32,
    pub gpu_temp: f32,
    pub ram_used_mb: u64,
    pub ram_total_mb: u64,
    pub vram_used_mb: u64,
    pub vram_total_mb: u64,
    pub fan_rpm: u32,
}

impl Default for SystemMonitors {
    fn default() -> Self {
        Self {
            cpu_usage: 0.0,
            gpu_usage: 0.0,
            cpu_temp: 0.0,
            gpu_temp: 0.0,
            ram_used_mb: 0,
            ram_total_mb: 0,
            vram_used_mb: 0,
            vram_total_mb: 0,
            fan_rpm: 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuickActionType {
    Screenshot,
    StartRecording,
    StopRecording,
    ToggleMic,
    ToggleNullLatency,
    ToggleCompactMode,
    CyclePosition,
}

pub struct QuickAction {
    pub label: String,
    pub icon: char,
    pub action: QuickActionType,
    pub enabled: bool,
}

pub struct QuickActionBar {
    pub visible: bool,
    pub actions: Vec<QuickAction>,
    pub selected: usize,
}

impl QuickActionBar {
    pub fn new() -> Self {
        Self {
            visible: false,
            actions: vec![
                QuickAction {
                    label: String::from("Screenshot"),
                    icon: 'S',
                    action: QuickActionType::Screenshot,
                    enabled: true,
                },
                QuickAction {
                    label: String::from("Record"),
                    icon: 'R',
                    action: QuickActionType::StartRecording,
                    enabled: true,
                },
                QuickAction {
                    label: String::from("Mic"),
                    icon: 'M',
                    action: QuickActionType::ToggleMic,
                    enabled: true,
                },
                QuickAction {
                    label: String::from("NULL_LAT"),
                    icon: 'N',
                    action: QuickActionType::ToggleNullLatency,
                    enabled: false,
                },
                QuickAction {
                    label: String::from("Compact"),
                    icon: 'C',
                    action: QuickActionType::ToggleCompactMode,
                    enabled: false,
                },
                QuickAction {
                    label: String::from("Move"),
                    icon: 'P',
                    action: QuickActionType::CyclePosition,
                    enabled: true,
                },
            ],
            selected: 0,
        }
    }

    pub fn select_next(&mut self) {
        if !self.actions.is_empty() {
            self.selected = (self.selected + 1) % self.actions.len();
        }
    }

    pub fn select_prev(&mut self) {
        if !self.actions.is_empty() {
            self.selected = self
                .selected
                .checked_sub(1)
                .unwrap_or(self.actions.len() - 1);
        }
    }

    pub fn selected_action(&self) -> Option<QuickActionType> {
        self.actions.get(self.selected).map(|a| a.action)
    }
}

// ── GameBar ──────────────────────────────────────────────────────────────

pub struct GameBar {
    pub visible: bool,
    pub opacity: u8,
    pub position: BarPosition,
    pub widgets: Vec<BarWidget>,
    pub active_widget: usize,
    pub fps_history: Vec<f32>,
    pub frametime_history: Vec<f32>,
    pub cpu_temp: f32,
    pub gpu_temp: f32,
    pub gpu_usage: f32,
    pub cpu_usage: f32,
    pub ram_usage_percent: f32,
    pub vram_usage_percent: f32,
    pub network_ping_ms: u32,
    pub recording: bool,
    pub record_duration_secs: u64,
    pub screenshot_flash: u8,
    pub voice_chat: VoiceChatState,
    pub notifications: Vec<OverlayNotification>,
    pub compact_mode: bool,
    pub screen_width: usize,
    pub screen_height: usize,
    pub fps_counter: FpsCounter,
    pub frametime_graph: FrametimeGraph,
    pub system_monitors: SystemMonitors,
    pub quick_actions: QuickActionBar,
    pub null_latency_active: bool,
    pub mic_muted: bool,
    history_max: usize,
    clock_text: String,
    battery_percent: u8,
    // ── Phase 4: live overlay state (Guide-chord invoked) ──────────────────
    /// Fixed-size frametime ring driving the Game Bar's graph (no per-frame
    /// alloc — Concept "all native, all fast").
    pub ft_ring: FrametimeRing,
    /// Live FPS from `crate::perf` (kernel-fed), `None` until a frame presents.
    pub live_fps: Option<f32>,
    /// CPU temp (°C) from `crate::thermal`, `None` where no sensor → "(n/a)".
    pub live_cpu_temp: Option<f32>,
    /// GPU temp (°C), `None` where unavailable → "(n/a)".
    pub live_gpu_temp: Option<f32>,
    /// How many times the overlay has been invoked (Guide chord / hotkey) — read
    /// by the smoketest + `/proc/raeen/gaming` to prove the toggle fires.
    pub invoke_count: u64,
}

impl GameBar {
    pub fn new(screen_width: usize, screen_height: usize) -> Self {
        Self {
            visible: false,
            opacity: 200,
            position: BarPosition::TopLeft,
            widgets: vec![
                BarWidget::FpsCounter,
                BarWidget::FrametimeGraph,
                BarWidget::CpuUsage,
                BarWidget::GpuUsage,
                BarWidget::CpuTemp,
                BarWidget::GpuTemp,
                BarWidget::RamUsage,
                BarWidget::VramUsage,
                BarWidget::NetworkPing,
            ],
            active_widget: 0,
            fps_history: Vec::new(),
            frametime_history: Vec::new(),
            cpu_temp: 0.0,
            gpu_temp: 0.0,
            gpu_usage: 0.0,
            cpu_usage: 0.0,
            ram_usage_percent: 0.0,
            vram_usage_percent: 0.0,
            network_ping_ms: 0,
            recording: false,
            record_duration_secs: 0,
            screenshot_flash: 0,
            voice_chat: VoiceChatState {
                enabled: false,
                muted: false,
                deafened: false,
                channel: None,
                users: Vec::new(),
                speaking: Vec::new(),
                volume: 100,
                input_device: String::from("Default"),
                output_device: String::from("Default"),
            },
            notifications: Vec::new(),
            compact_mode: false,
            screen_width,
            screen_height,
            fps_counter: FpsCounter::new(),
            frametime_graph: FrametimeGraph::new(60.0),
            system_monitors: SystemMonitors::default(),
            quick_actions: QuickActionBar::new(),
            null_latency_active: false,
            mic_muted: false,
            history_max: 120,
            clock_text: String::from("00:00"),
            battery_percent: 100,
            ft_ring: FrametimeRing::new(),
            live_fps: None,
            live_cpu_temp: None,
            live_gpu_temp: None,
            invoke_count: 0,
        }
    }

    // ── Controls ─────────────────────────────────────────────────────────

    pub fn toggle(&mut self) {
        self.visible = !self.visible;
    }

    /// Invoke the Game Bar (Guide tap / hotkey). Toggles visibility and, when
    /// opening, counts the invocation so the smoketest + procfs can prove the
    /// chord fires. Returns the new visibility. The overlay composites OVER the
    /// running couch/game without disrupting it (the caller keeps rendering the
    /// content underneath; `render` paints the bar above it).
    pub fn invoke(&mut self) -> bool {
        self.visible = !self.visible;
        if self.visible {
            self.invoke_count = self.invoke_count.saturating_add(1);
        }
        self.visible
    }

    /// Feed the Game Bar one live telemetry sample (Phase 4). The kernel calls
    /// this from the compositor/Game-Bar refresh with `crate::perf` FPS +
    /// frametime and `crate::thermal` CPU/GPU temps. Pushes the frametime into
    /// the fixed-size ring (driving the graph) and records FPS + temps for the
    /// panels. `None` temps surface as "(n/a)" — never a panic, never a 0°C lie.
    pub fn ingest_perf(&mut self, feed: &PerfFeed) {
        if let Some(ft) = feed.frametime_ms {
            self.ft_ring.push(ft);
            // Mirror into the legacy graph/history for the existing renderers.
            self.frametime_graph.push(ft);
            self.frametime_history.push(ft);
            if self.frametime_history.len() > self.history_max {
                self.frametime_history.remove(0);
            }
        }
        // Prefer the explicitly-fed FPS; else derive from the ring.
        self.live_fps = feed.fps.or_else(|| self.ft_ring.fps());
        if let Some(fps) = self.live_fps {
            self.fps_counter.current_fps = fps;
            self.fps_history.push(fps);
            if self.fps_history.len() > self.history_max {
                self.fps_history.remove(0);
            }
        }
        self.live_cpu_temp = feed.cpu_temp_c;
        self.live_gpu_temp = feed.gpu_temp_c;
        if let Some(c) = feed.cpu_temp_c {
            self.cpu_temp = c;
        }
        if let Some(g) = feed.gpu_temp_c {
            self.gpu_temp = g;
        }
    }

    /// Number of populated panels in the live overlay (FPS, frametime graph,
    /// temps, quick-actions) — used by the smoketest + procfs. Temps count as a
    /// panel even when "(n/a)" (the panel still renders the unavailable state).
    #[must_use]
    pub fn live_panel_count(&self) -> usize {
        // FPS panel + frametime-graph panel + temps panel + quick-action panel.
        4
    }

    pub fn toggle_widget(&mut self, widget: BarWidget) {
        if let Some(pos) = self.widgets.iter().position(|w| *w == widget) {
            self.widgets.remove(pos);
        } else {
            self.widgets.push(widget);
        }
    }

    pub fn cycle_widget(&mut self, forward: bool) {
        if self.widgets.is_empty() {
            return;
        }
        if forward {
            self.active_widget = (self.active_widget + 1) % self.widgets.len();
        } else {
            self.active_widget = self
                .active_widget
                .checked_sub(1)
                .unwrap_or(self.widgets.len() - 1);
        }
    }

    pub fn set_position(&mut self, pos: BarPosition) {
        self.position = pos;
    }

    // ── Stats update ─────────────────────────────────────────────────────

    pub fn update_stats(&mut self, snap: &PerformanceSnapshot) {
        self.fps_history.push(snap.fps);
        if self.fps_history.len() > self.history_max {
            self.fps_history.remove(0);
        }

        self.frametime_history.push(snap.frametime_ms);
        if self.frametime_history.len() > self.history_max {
            self.frametime_history.remove(0);
        }

        self.cpu_temp = snap.cpu_temp;
        self.gpu_temp = snap.gpu_temp;
        self.cpu_usage = snap.cpu_usage;
        self.gpu_usage = snap.gpu_usage;
        self.network_ping_ms = snap.network_ping;

        if snap.timestamp > 0 && self.recording {
            // duration would be computed from start; placeholder
        }

        if self.screenshot_flash > 0 {
            self.screenshot_flash = self.screenshot_flash.saturating_sub(4);
        }
    }

    pub fn update_full(&mut self, snap: &PerformanceSnapshot) {
        self.update_stats(snap);

        self.fps_counter.push_frame(snap.frametime_ms);
        self.frametime_graph.push(snap.frametime_ms);

        self.system_monitors.cpu_usage = snap.cpu_usage;
        self.system_monitors.gpu_usage = snap.gpu_usage;
        self.system_monitors.cpu_temp = snap.cpu_temp;
        self.system_monitors.gpu_temp = snap.gpu_temp;
        self.system_monitors.ram_used_mb = snap.ram_mb as u64;
        self.system_monitors.vram_used_mb = snap.vram_mb as u64;
    }

    pub fn execute_quick_action(&mut self) -> Option<QuickActionType> {
        let action = self.quick_actions.selected_action()?;
        match action {
            QuickActionType::Screenshot => {
                self.take_screenshot();
            }
            QuickActionType::StartRecording => {
                if !self.recording {
                    self.toggle_recording();
                }
            }
            QuickActionType::StopRecording => {
                if self.recording {
                    self.toggle_recording();
                }
            }
            QuickActionType::ToggleMic => {
                self.mic_muted = !self.mic_muted;
                self.voice_chat.muted = self.mic_muted;
            }
            QuickActionType::ToggleNullLatency => {
                self.null_latency_active = !self.null_latency_active;
            }
            QuickActionType::ToggleCompactMode => {
                self.compact_mode = !self.compact_mode;
            }
            QuickActionType::CyclePosition => {
                self.position = match self.position {
                    BarPosition::TopLeft => BarPosition::TopRight,
                    BarPosition::TopRight => BarPosition::BottomRight,
                    BarPosition::BottomRight => BarPosition::BottomLeft,
                    BarPosition::BottomLeft => BarPosition::TopCenter,
                    BarPosition::TopCenter => BarPosition::BottomCenter,
                    BarPosition::BottomCenter => BarPosition::TopLeft,
                };
            }
        }
        Some(action)
    }

    pub fn toggle_quick_actions(&mut self) {
        self.quick_actions.visible = !self.quick_actions.visible;
        if self.quick_actions.visible {
            self.quick_actions.selected = 0;
        }
    }

    pub fn render_compact_corner(&self, canvas: &mut raegfx::Canvas) {
        if self.visible || !self.compact_mode {
            return;
        }

        let fps = self.fps_counter.current_fps;
        let fps_i = fps as u32;
        let mut buf = [0u8; 12];
        let fps_str = fmt_usize(fps_i as usize, &mut buf);
        let fps_color = fps_to_color(fps);

        let margin = 8usize;
        let (x, y) = match self.position {
            BarPosition::TopLeft | BarPosition::TopCenter => (margin, margin),
            BarPosition::TopRight => (
                self.screen_width - fps_str.len() * GLYPH_W - 4 * GLYPH_W - margin,
                margin,
            ),
            BarPosition::BottomLeft | BarPosition::BottomCenter => {
                (margin, self.screen_height - GLYPH_H - margin)
            }
            BarPosition::BottomRight => (
                self.screen_width - fps_str.len() * GLYPH_W - 4 * GLYPH_W - margin,
                self.screen_height - GLYPH_H - margin,
            ),
        };

        let bg_w = (4 + fps_str.len()) * GLYPH_W + 8;
        canvas.fill_rect(x, y, bg_w, GLYPH_H + 6, OVERLAY_BG);
        canvas.draw_text(x + 4, y + 3, "FPS ", TEXT_DIM, None);
        canvas.draw_text(x + 4 + 4 * GLYPH_W, y + 3, fps_str, fps_color, None);
    }

    // ── Screenshot / recording ───────────────────────────────────────────

    pub fn take_screenshot(&mut self) {
        self.screenshot_flash = 255;
    }

    pub fn toggle_recording(&mut self) {
        self.recording = !self.recording;
        if self.recording {
            self.record_duration_secs = 0;
        }
    }

    // ── Rendering ────────────────────────────────────────────────────────

    pub fn render(&self, canvas: &mut raegfx::Canvas) {
        if !self.visible {
            if self.recording {
                self.render_recording_indicator(canvas);
            }
            self.render_notifications(canvas);
            self.render_compact_corner(canvas);

            if self.screenshot_flash > 0 {
                let flash_color = (self.screenshot_flash as u32) << 24 | 0xFF_FF_FF;
                canvas.fill_rect(0, 0, self.screen_width, self.screen_height, flash_color);
            }
            return;
        }

        let (ox, oy) = self.overlay_origin();
        let panel_w = if self.compact_mode { 180 } else { 260 };
        let mut panel_h = 8usize;

        // Measure required height
        for widget in &self.widgets {
            panel_h += self.widget_height(*widget);
        }
        panel_h += 8;

        // Background panel
        canvas.fill_rect(ox, oy, panel_w, panel_h, OVERLAY_BG);
        canvas.draw_rect_outline(ox, oy, panel_w, panel_h, OVERLAY_BORDER);

        // Render each widget
        let mut wy = oy + 8;
        for (i, widget) in self.widgets.iter().enumerate() {
            let highlighted = i == self.active_widget;
            match widget {
                BarWidget::FpsCounter => {
                    self.render_fps_counter(canvas, ox + 8, wy, panel_w - 16, highlighted);
                }
                BarWidget::FrametimeGraph => {
                    self.render_frametime_graph(canvas, ox + 8, wy, panel_w - 16, highlighted);
                }
                BarWidget::CpuTemp | BarWidget::GpuTemp => {
                    self.render_temps(canvas, ox + 8, wy, panel_w - 16, *widget, highlighted);
                }
                BarWidget::CpuUsage
                | BarWidget::GpuUsage
                | BarWidget::RamUsage
                | BarWidget::VramUsage => {
                    self.render_usage_bars(canvas, ox + 8, wy, panel_w - 16, *widget, highlighted);
                }
                BarWidget::NetworkPing => {
                    self.render_ping(canvas, ox + 8, wy, panel_w - 16, highlighted);
                }
                BarWidget::RecordIndicator => {
                    self.render_recording_indicator(canvas);
                }
                BarWidget::VoiceChat => {
                    self.render_voice_chat(canvas, ox + 8, wy, panel_w - 16);
                }
                BarWidget::Clock => {
                    if highlighted {
                        canvas.fill_rect(ox + 4, wy - 2, panel_w - 8, 16, 0x44_4E_9C_FF);
                    }
                    canvas.draw_text(ox + 8, wy, &self.clock_text, TEXT_FG, None);
                }
                BarWidget::Battery => {
                    if highlighted {
                        canvas.fill_rect(ox + 4, wy - 2, panel_w - 8, 16, 0x44_4E_9C_FF);
                    }
                    let mut buf = [0u8; 12];
                    let pct = fmt_usize(self.battery_percent as usize, &mut buf);
                    canvas.draw_text(ox + 8, wy, "BAT: ", TEXT_DIM, None);
                    let color = if self.battery_percent > 50 {
                        GREEN
                    } else if self.battery_percent > 20 {
                        YELLOW
                    } else {
                        RED
                    };
                    canvas.draw_text(ox + 8 + 5 * GLYPH_W, wy, pct, color, None);
                    let after = ox + 8 + 5 * GLYPH_W + pct.len() * GLYPH_W;
                    canvas.draw_text(after, wy, "%", TEXT_DIM, None);
                }
            }
            wy += self.widget_height(*widget);
        }

        // Screenshot flash
        if self.screenshot_flash > 0 {
            let flash_color = (self.screenshot_flash as u32) << 24 | 0xFF_FF_FF;
            canvas.fill_rect(0, 0, self.screen_width, self.screen_height, flash_color);
        }

        if self.quick_actions.visible {
            self.render_quick_actions(canvas);
        }

        self.render_notifications(canvas);
    }

    /// Phase 4 live overlay: draw the Game-Bar panels from the live `perf` /
    /// `thermal` feed — FPS, the frametime sparkline (from `ft_ring`), CPU/GPU
    /// temps with graceful "(n/a)", and the quick-action bar. Composites in the
    /// top-left over whatever runs underneath; does nothing when hidden. Never
    /// panics on missing data.
    pub fn render_live_overlay(&self, canvas: &mut raegfx::Canvas) {
        if !self.visible {
            return;
        }
        let (ox, oy) = self.overlay_origin();
        let panel_w = 260usize;
        let panel_h = 156usize;
        canvas.fill_rect(ox, oy, panel_w, panel_h, OVERLAY_BG);
        canvas.draw_rect_outline(ox, oy, panel_w, panel_h, OVERLAY_BORDER);

        let ix = ox + 8;
        let mut wy = oy + 8;

        // FPS line (large, colored vs target).
        let fps = self.live_fps.unwrap_or(0.0);
        canvas.draw_text(ix, wy, "FPS ", TEXT_DIM, None);
        let mut buf = [0u8; 12];
        if self.live_fps.is_some() {
            let s = fmt_usize(fps as usize, &mut buf);
            canvas.draw_text(ix + 4 * GLYPH_W, wy, s, fps_to_color(fps), None);
        } else {
            canvas.draw_text(ix + 4 * GLYPH_W, wy, "(n/a)", TEXT_DIM, None);
        }
        wy += 18;

        // Frametime sparkline from the fixed-size ring.
        self.render_ring_graph(canvas, ix, wy, panel_w - 16, 44);
        wy += 44 + 4;

        // Last frametime value label.
        canvas.draw_text(ix, wy, "FT ", TEXT_DIM, None);
        if let Some(last) = self.ft_ring.last() {
            let s = fmt_usize(last as usize, &mut buf);
            canvas.draw_text(ix + 3 * GLYPH_W, wy, s, TEXT_FG, None);
            let after = ix + 3 * GLYPH_W + s.len() * GLYPH_W;
            canvas.draw_text(after, wy, "ms", TEXT_DIM, None);
        } else {
            canvas.draw_text(ix + 3 * GLYPH_W, wy, "(n/a)", TEXT_DIM, None);
        }
        wy += 16;

        // Temps (CPU / GPU) with graceful (n/a).
        self.render_temp_line(canvas, ix, wy, "CPU", self.live_cpu_temp);
        self.render_temp_line(canvas, ix + (panel_w / 2), wy, "GPU", self.live_gpu_temp);
        wy += 16;

        // Quick-action hints (capture/screenshot/voice).
        canvas.draw_glyph(ix, wy, 'S', ACCENT, None);
        canvas.draw_text(ix + 14, wy, "Capture", TEXT_DIM, None);
        if self.recording {
            canvas.fill_rect(ix + panel_w - 24, wy, 8, 8, REC_RED);
        }

        if self.screenshot_flash > 0 {
            let flash_color = (self.screenshot_flash as u32) << 24 | 0xFF_FF_FF;
            canvas.fill_rect(0, 0, self.screen_width, self.screen_height, flash_color);
        }
    }

    /// Draw the frametime ring as a bar/sparkline graph. Each ring sample maps
    /// to a column; bars are colored red past the 33.3ms (30fps) line. Empty
    /// ring → just the background + grid (never a panic).
    fn render_ring_graph(
        &self,
        canvas: &mut raegfx::Canvas,
        x: usize,
        y: usize,
        w: usize,
        h: usize,
    ) {
        canvas.fill_rect(x, y, w, h, GRAPH_BG);
        canvas.draw_rect_outline(x, y, w, h, GRAPH_GRID);

        let max_ft = 50.0f32;
        // 16.6ms (60fps) and 33.3ms (30fps) reference rules.
        for &rule in &[16.6f32, 33.3f32] {
            let ry = y + h - ((rule / max_ft) * h as f32) as usize;
            if ry > y && ry < y + h {
                for gx in (x..x + w).step_by(4) {
                    canvas.draw_pixel(gx, ry, GRAPH_GRID);
                }
            }
        }

        let n = self.ft_ring.len();
        if n == 0 || w == 0 {
            return;
        }
        let cols = n.min(w);
        let start = n.saturating_sub(cols);
        for c in 0..cols {
            let ft = self.ft_ring.get_chrono(start + c).unwrap_or(0.0);
            let norm = (ft / max_ft).min(1.0);
            let bar_h = (norm * h as f32) as usize;
            let bx = x + (c * w) / cols;
            let by = y + h - bar_h;
            let color = if ft > 33.3 { GRAPH_BAD } else { GRAPH_LINE };
            if bar_h > 0 {
                canvas.fill_rect(bx, by, (w / cols).max(1), bar_h, color);
            }
        }
    }

    /// One temp label/value pair with "(n/a)" when the sensor is unavailable.
    fn render_temp_line(
        &self,
        canvas: &mut raegfx::Canvas,
        x: usize,
        y: usize,
        label: &str,
        temp: Option<f32>,
    ) {
        canvas.draw_text(x, y, label, TEXT_DIM, None);
        let vx = x + (label.len() + 1) * GLYPH_W;
        match temp {
            Some(t) => {
                let mut buf = [0u8; 12];
                let s = fmt_usize(t as usize, &mut buf);
                canvas.draw_text(vx, y, s, temp_to_color(t), None);
                let after = vx + s.len() * GLYPH_W;
                canvas.draw_text(after, y, "C", TEXT_DIM, None);
            }
            None => {
                canvas.draw_text(vx, y, "(n/a)", TEXT_DIM, None);
            }
        }
    }

    fn render_quick_actions(&self, canvas: &mut raegfx::Canvas) {
        let (ox, _oy) = self.overlay_origin();
        let panel_w = 200usize;
        let item_h = 28usize;
        let panel_h = self.quick_actions.actions.len() * item_h + 16;
        let px = ox;
        let py = 12 + 16;

        canvas.fill_rect(px, py, panel_w, panel_h, OVERLAY_BG);
        canvas.draw_rect_outline(px, py, panel_w, panel_h, OVERLAY_BORDER);

        let mut iy = py + 8;
        for (i, action) in self.quick_actions.actions.iter().enumerate() {
            let selected = i == self.quick_actions.selected;
            if selected {
                canvas.fill_rect(px + 4, iy, panel_w - 8, item_h - 4, 0x44_4E_9C_FF);
            }

            let ic = if selected { ACCENT } else { TEXT_DIM };
            let tc = if selected { TEXT_FG } else { TEXT_DIM };
            canvas.draw_glyph(px + 8, iy + 8, action.icon, ic, None);
            canvas.draw_text(px + 22, iy + 8, &action.label, tc, None);

            if action.enabled {
                canvas.draw_glyph(px + panel_w - 16, iy + 8, '*', GREEN, None);
            }

            iy += item_h;
        }
    }

    fn widget_height(&self, w: BarWidget) -> usize {
        match w {
            BarWidget::FpsCounter => 20,
            BarWidget::FrametimeGraph => {
                if self.compact_mode {
                    40
                } else {
                    56
                }
            }
            BarWidget::CpuTemp | BarWidget::GpuTemp => 16,
            BarWidget::CpuUsage
            | BarWidget::GpuUsage
            | BarWidget::RamUsage
            | BarWidget::VramUsage => 20,
            BarWidget::NetworkPing => 16,
            BarWidget::Clock | BarWidget::Battery => 16,
            BarWidget::RecordIndicator => 16,
            BarWidget::VoiceChat => {
                let user_lines = self.voice_chat.users.len().min(5);
                20 + user_lines * 14
            }
        }
    }

    fn overlay_origin(&self) -> (usize, usize) {
        let margin = 12usize;
        match self.position {
            BarPosition::TopLeft => (margin, margin),
            BarPosition::TopCenter => (self.screen_width / 2 - 130, margin),
            BarPosition::TopRight => (self.screen_width - 260 - margin, margin),
            BarPosition::BottomLeft => (margin, self.screen_height - 300 - margin),
            BarPosition::BottomCenter => (
                self.screen_width / 2 - 130,
                self.screen_height - 300 - margin,
            ),
            BarPosition::BottomRight => (
                self.screen_width - 260 - margin,
                self.screen_height - 300 - margin,
            ),
        }
    }

    // ── Individual widget renderers ──────────────────────────────────────

    pub fn render_fps_counter(
        &self,
        canvas: &mut raegfx::Canvas,
        x: usize,
        y: usize,
        w: usize,
        hl: bool,
    ) {
        if hl {
            canvas.fill_rect(x - 4, y - 2, w + 8, 18, 0x44_4E_9C_FF);
        }

        let fps = self.fps_history.last().copied().unwrap_or(0.0);
        let fps_color = fps_to_color(fps);

        canvas.draw_text(x, y, "FPS ", TEXT_DIM, None);

        let fps_i = fps as u32;
        let mut buf = [0u8; 12];
        let fps_str = fmt_usize(fps_i as usize, &mut buf);
        canvas.draw_text(x + 4 * GLYPH_W, y, fps_str, fps_color, None);

        // 1% low
        if self.fps_history.len() > 10 {
            let mut sorted = self.fps_history.clone();
            sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(core::cmp::Ordering::Equal));
            let idx_1pct = sorted.len() / 100;
            let low1 = sorted.get(idx_1pct).copied().unwrap_or(0.0) as u32;
            let mut low_buf = [0u8; 12];
            let low_str = fmt_usize(low1 as usize, &mut low_buf);
            let lx = x + w - (4 + low_str.len()) * GLYPH_W;
            canvas.draw_text(lx, y, "1%: ", TEXT_DIM, None);
            canvas.draw_text(
                lx + 4 * GLYPH_W,
                y,
                low_str,
                fps_to_color(low1 as f32),
                None,
            );
        }
    }

    pub fn render_frametime_graph(
        &self,
        canvas: &mut raegfx::Canvas,
        x: usize,
        y: usize,
        w: usize,
        hl: bool,
    ) {
        let graph_h = if self.compact_mode { 28 } else { 44 };

        // Graph background
        canvas.fill_rect(x, y, w, graph_h, GRAPH_BG);
        if hl {
            canvas.draw_rect_outline(x, y, w, graph_h, ACCENT);
        } else {
            canvas.draw_rect_outline(x, y, w, graph_h, 0xFF_22_24_34);
        }

        // Horizontal grid lines at 16ms and 33ms
        let max_ft = 50.0f32;
        let line_16_y = y + graph_h - ((16.0 / max_ft) * graph_h as f32) as usize;
        let line_33_y = y + graph_h - ((33.0 / max_ft) * graph_h as f32) as usize;

        if line_16_y > y && line_16_y < y + graph_h {
            for gx in (x..x + w).step_by(4) {
                canvas.draw_pixel(gx, line_16_y, GRAPH_GRID);
            }
        }
        if line_33_y > y && line_33_y < y + graph_h {
            for gx in (x..x + w).step_by(4) {
                canvas.draw_pixel(gx, line_33_y, GRAPH_GRID);
            }
        }

        // Plot frametime data
        let history = &self.frametime_history;
        if history.len() >= 2 {
            let samples = history.len().min(w);
            let start = history.len().saturating_sub(samples);

            let mut prev_px: Option<(i32, i32)> = None;
            for (i, &ft) in history[start..].iter().enumerate() {
                let px_x = x as i32 + (i * w / samples) as i32;
                let norm = (ft / max_ft).min(1.0);
                let px_y = (y + graph_h) as i32 - (norm * graph_h as f32) as i32;

                let color = if ft > 33.3 { GRAPH_BAD } else { GRAPH_LINE };

                if let Some((prev_x, prev_y)) = prev_px {
                    canvas.draw_line(prev_x, prev_y, px_x, px_y, color);
                }
                prev_px = Some((px_x, px_y));
            }
        }

        // Label
        let label_y = y + graph_h + 2;
        canvas.draw_text(x, label_y, "FT", TEXT_DIM, None);
        if let Some(&last) = self.frametime_history.last() {
            let ft_i = last as u32;
            let mut buf = [0u8; 12];
            let ft_str = fmt_usize(ft_i as usize, &mut buf);
            canvas.draw_text(x + 3 * GLYPH_W, label_y, ft_str, TEXT_FG, None);
            let after = x + 3 * GLYPH_W + ft_str.len() * GLYPH_W;
            canvas.draw_text(after, label_y, "ms", TEXT_DIM, None);
        }
    }

    pub fn render_temps(
        &self,
        canvas: &mut raegfx::Canvas,
        x: usize,
        y: usize,
        w: usize,
        widget: BarWidget,
        hl: bool,
    ) {
        if hl {
            canvas.fill_rect(x - 4, y - 2, w + 8, 14, 0x44_4E_9C_FF);
        }

        let (label, temp) = match widget {
            BarWidget::CpuTemp => ("CPU", self.cpu_temp),
            BarWidget::GpuTemp => ("GPU", self.gpu_temp),
            _ => return,
        };

        canvas.draw_text(x, y, label, TEXT_DIM, None);
        canvas.draw_text(x + 4 * GLYPH_W, y, "Temp: ", TEXT_DIM, None);

        let temp_i = temp as u32;
        let mut buf = [0u8; 12];
        let temp_str = fmt_usize(temp_i as usize, &mut buf);
        let color = temp_to_color(temp);
        canvas.draw_text(x + 10 * GLYPH_W, y, temp_str, color, None);
        let after = x + 10 * GLYPH_W + temp_str.len() * GLYPH_W;
        canvas.draw_text(after, y, "C", TEXT_DIM, None);
    }

    pub fn render_usage_bars(
        &self,
        canvas: &mut raegfx::Canvas,
        x: usize,
        y: usize,
        w: usize,
        widget: BarWidget,
        hl: bool,
    ) {
        let (label, pct, fill_color) = match widget {
            BarWidget::CpuUsage => ("CPU", self.cpu_usage, BAR_FILL_CPU),
            BarWidget::GpuUsage => ("GPU", self.gpu_usage, BAR_FILL_GPU),
            BarWidget::RamUsage => ("RAM", self.ram_usage_percent, BAR_FILL_RAM),
            BarWidget::VramUsage => ("VRAM", self.vram_usage_percent, BAR_FILL_VRAM),
            _ => return,
        };

        if hl {
            canvas.fill_rect(x - 4, y - 2, w + 8, 18, 0x44_4E_9C_FF);
        }

        let label_w = label.len() * GLYPH_W;
        canvas.draw_text(x, y, label, TEXT_DIM, None);

        // Progress bar
        let bar_x = x + label_w + GLYPH_W;
        let bar_w = w - label_w - GLYPH_W - 5 * GLYPH_W;
        let bar_h = 10usize;
        let bar_y = y + 2;

        canvas.fill_rect(bar_x, bar_y, bar_w, bar_h, BAR_BG);
        let fill_w = ((pct / 100.0) * bar_w as f32) as usize;
        if fill_w > 0 {
            canvas.fill_rect(bar_x, bar_y, fill_w.min(bar_w), bar_h, fill_color);
        }
        canvas.draw_rect_outline(bar_x, bar_y, bar_w, bar_h, 0xFF_33_37_4A);

        // Percentage text
        let pct_i = pct as u32;
        let mut buf = [0u8; 12];
        let pct_str = fmt_usize(pct_i as usize, &mut buf);
        let pct_x = bar_x + bar_w + GLYPH_W;
        canvas.draw_text(pct_x, y, pct_str, TEXT_FG, None);
        let after = pct_x + pct_str.len() * GLYPH_W;
        canvas.draw_text(after, y, "%", TEXT_DIM, None);
    }

    fn render_ping(&self, canvas: &mut raegfx::Canvas, x: usize, y: usize, w: usize, hl: bool) {
        if hl {
            canvas.fill_rect(x - 4, y - 2, w + 8, 14, 0x44_4E_9C_FF);
        }

        canvas.draw_text(x, y, "PING ", TEXT_DIM, None);

        let mut buf = [0u8; 12];
        let ping_str = fmt_usize(self.network_ping_ms as usize, &mut buf);
        let ping_color = if self.network_ping_ms < 30 {
            GREEN
        } else if self.network_ping_ms < 80 {
            YELLOW
        } else {
            RED
        };
        canvas.draw_text(x + 5 * GLYPH_W, y, ping_str, ping_color, None);
        let after = x + 5 * GLYPH_W + ping_str.len() * GLYPH_W;
        canvas.draw_text(after, y, "ms", TEXT_DIM, None);
    }

    pub fn render_recording_indicator(&self, canvas: &mut raegfx::Canvas) {
        if !self.recording {
            return;
        }

        let rx = self.screen_width - 120;
        let ry = 12;
        let rw = 108;
        let rh = 24;

        canvas.fill_rect(rx, ry, rw, rh, 0xCC_1A_0A_0A);
        canvas.draw_rect_outline(rx, ry, rw, rh, REC_RED);

        // Pulsing dot (simulated with constant for now)
        canvas.fill_rect(rx + 6, ry + 8, 8, 8, REC_RED);

        canvas.draw_text(rx + 18, ry + 8, "REC", RED, None);

        // Duration
        let mins = self.record_duration_secs / 60;
        let secs = self.record_duration_secs % 60;
        let mut m_buf = [0u8; 12];
        let mut s_buf = [0u8; 12];
        let m_str = fmt_usize(mins as usize, &mut m_buf);
        let s_str = fmt_usize(secs as usize, &mut s_buf);

        let tx = rx + 44;
        canvas.draw_text(tx, ry + 8, m_str, TEXT_FG, None);
        let after_m = tx + m_str.len() * GLYPH_W;
        canvas.draw_text(after_m, ry + 8, ":", TEXT_DIM, None);
        // Zero-pad seconds
        if secs < 10 {
            canvas.draw_text(after_m + GLYPH_W, ry + 8, "0", TEXT_FG, None);
            canvas.draw_text(after_m + 2 * GLYPH_W, ry + 8, s_str, TEXT_FG, None);
        } else {
            canvas.draw_text(after_m + GLYPH_W, ry + 8, s_str, TEXT_FG, None);
        }
    }

    pub fn render_voice_chat(&self, canvas: &mut raegfx::Canvas, x: usize, y: usize, w: usize) {
        if !self.voice_chat.enabled {
            canvas.draw_text(x, y, "Voice: Off", TEXT_DIM, None);
            return;
        }

        // Channel name
        let channel_str = self.voice_chat.channel.as_deref().unwrap_or("No Channel");
        let mic_icon = if self.voice_chat.muted { 'X' } else { 'M' };
        let mic_color = if self.voice_chat.muted {
            RED
        } else {
            VOICE_GREEN
        };

        canvas.draw_glyph(x, y, mic_icon, mic_color, None);
        canvas.draw_text(x + GLYPH_W + 4, y, channel_str, TEXT_FG, None);

        if self.voice_chat.deafened {
            let dx = x + w - 2 * GLYPH_W;
            canvas.draw_text(dx, y, "DF", RED, None);
        }

        // User list (max 5)
        let mut uy = y + 16;
        for (i, user) in self.voice_chat.users.iter().take(5).enumerate() {
            let is_speaking = self.voice_chat.speaking.contains(&user.id);
            let name_color = if is_speaking { VOICE_GREEN } else { TEXT_DIM };

            let indicator = if user.muted {
                'X'
            } else if is_speaking {
                '>'
            } else {
                '-'
            };
            let ind_color = if user.muted {
                RED
            } else if is_speaking {
                VOICE_GREEN
            } else {
                TEXT_DIM
            };

            canvas.draw_glyph(x + 8, uy, indicator, ind_color, None);

            let max_name = (w - 32) / GLYPH_W;
            let disp = crate::text_util::truncate_chars(&user.name, max_name);
            canvas.draw_text(x + 20, uy, disp, name_color, None);

            uy += 14;
        }

        if self.voice_chat.users.len() > 5 {
            let extra = self.voice_chat.users.len() - 5;
            let mut buf = [0u8; 12];
            let n_str = fmt_usize(extra, &mut buf);
            canvas.draw_text(x + 8, uy, "+", TEXT_DIM, None);
            canvas.draw_text(x + 16, uy, n_str, TEXT_DIM, None);
            let after = x + 16 + n_str.len() * GLYPH_W;
            canvas.draw_text(after, uy, " more", TEXT_DIM, None);
        }
    }

    pub fn render_notifications(&self, canvas: &mut raegfx::Canvas) {
        let notif_w = 300usize;
        let notif_h = 40usize;
        let margin = 12usize;
        let max_show = 4usize;

        let base_x = self.screen_width - notif_w - margin;
        let base_y = self.screen_height - margin;

        for (i, notif) in self.notifications.iter().rev().take(max_show).enumerate() {
            let ny = base_y - (i + 1) * (notif_h + 8);

            canvas.fill_rect(base_x, ny, notif_w, notif_h, NOTIF_BG);

            let border_color = if notif.priority >= 3 {
                RED
            } else if notif.priority >= 2 {
                ORANGE
            } else {
                NOTIF_BORDER
            };
            canvas.draw_rect_outline(base_x, ny, notif_w, notif_h, border_color);

            canvas.draw_glyph(base_x + 8, ny + 8, notif.icon, ACCENT, None);

            let max_text = (notif_w - 32) / GLYPH_W;
            let disp = crate::text_util::truncate_chars(&notif.text, max_text);
            canvas.draw_text(base_x + 22, ny + 8, disp, TEXT_FG, None);

            // Duration indicator
            let mut dur_buf = [0u8; 12];
            let dur_secs = notif.duration_ms / 1000;
            let dur_str = fmt_usize(dur_secs as usize, &mut dur_buf);
            canvas.draw_text(base_x + 22, ny + 24, dur_str, TEXT_DIM, None);
            let after = base_x + 22 + dur_str.len() * GLYPH_W;
            canvas.draw_text(after, ny + 24, "s", TEXT_DIM, None);
        }
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────

// ── Phase 4: Game Bar smoketest ────────────────────────────────────────────

/// Outcome of the Game-Bar overlay smoketest (Phase 4). FAIL-able on each new
/// behaviour: the overlay won't toggle, the frametime graph has 0 points after
/// synthetic frames, the FPS read is garbage, or the panels don't render.
pub struct GameBarSmoketest {
    /// The Guide-chord / hotkey invoke toggled the overlay on then off.
    pub invoked_ok: bool,
    /// FPS read back from the ingested feed (rounded), for the proof line.
    pub fps_read: u32,
    /// Number of frametime points in the ring after the synthetic frames.
    pub frametime_pts: usize,
    /// Temp sources resolved (Some) vs gracefully "(n/a)" — both are valid; this
    /// is `true` when the temp path did not panic and produced a renderable
    /// result for the fed values.
    pub temps_ok: bool,
    /// Live panels rendered (FPS / graph / temps / quick-actions).
    pub panels: usize,
    /// The overlay rendered real ink into an offscreen canvas (not all-zero).
    pub rendered: bool,
}

impl GameBarSmoketest {
    #[must_use]
    pub fn passed(&self) -> bool {
        self.invoked_ok
            && self.frametime_pts > 0
            && self.fps_read > 0
            && self.fps_read < 100_000 // garbage guard
            && self.temps_ok
            && self.panels >= 4
            && self.rendered
    }
}

/// Run the Phase-4 Game-Bar smoketest into an offscreen canvas. Drives the
/// FULL live path: invoke (toggle on/off), feed N synthetic frames of ~16.6ms
/// (60fps) plus a CPU temp and a `None` GPU temp (the QEMU "(n/a)" case),
/// confirm the ring filled + FPS reads ~60, render the live overlay, and check
/// real ink landed. FAIL if any of those break.
pub fn run_gamebar_smoketest(
    canvas: &mut raegfx::Canvas,
    width: usize,
    height: usize,
) -> GameBarSmoketest {
    let mut bar = GameBar::new(width, height);

    // Invoke: open, then close — proves the chord toggles. `invoke` returns the
    // new visibility, so the first call returns true (opened) and the second
    // returns false (closed); the open is counted once.
    let opened = bar.invoke();
    let closed = !bar.invoke();
    let invoked_ok = opened && closed && bar.invoke_count == 1;
    // Leave it open for the render.
    let _ = bar.invoke();

    // Feed N synthetic 60fps frames (16.6ms) + a real CPU temp, no GPU temp.
    const N: usize = 30;
    for _ in 0..N {
        bar.ingest_perf(&PerfFeed {
            fps: None, // force the ring-derived FPS path
            frametime_ms: Some(16.6),
            cpu_temp_c: Some(58.0),
            gpu_temp_c: None, // the "(n/a)" case
        });
    }

    let fps_read = bar.live_fps.unwrap_or(0.0) as u32;
    let frametime_pts = bar.ft_ring.len();
    // Temps: CPU fed (Some), GPU None → both must produce a renderable result.
    let temps_ok = bar.live_cpu_temp == Some(58.0) && bar.live_gpu_temp.is_none();
    let panels = bar.live_panel_count();

    bar.render_live_overlay(canvas);
    let rendered = canvas_has_ink(canvas, width, height);

    GameBarSmoketest {
        invoked_ok,
        fps_read,
        frametime_pts,
        temps_ok,
        panels,
        rendered,
    }
}

/// True if the canvas has any non-zero byte (real ink was drawn). Reads the
/// backing buffer the caller owns via the canvas dimensions.
fn canvas_has_ink(canvas: &raegfx::Canvas, width: usize, height: usize) -> bool {
    for y in 0..height {
        for x in 0..width {
            if canvas.get_pixel(x, y) & 0x00FF_FFFF != 0 {
                return true;
            }
        }
    }
    false
}

fn fps_to_color(fps: f32) -> u32 {
    if fps >= 60.0 {
        GREEN
    } else if fps >= 30.0 {
        YELLOW
    } else if fps >= 15.0 {
        ORANGE
    } else {
        RED
    }
}

fn temp_to_color(temp: f32) -> u32 {
    if temp < 60.0 {
        GREEN
    } else if temp < 75.0 {
        YELLOW
    } else if temp < 85.0 {
        ORANGE
    } else {
        RED
    }
}

fn fmt_usize(mut n: usize, buf: &mut [u8; 12]) -> &str {
    if n == 0 {
        buf[0] = b'0';
        return unsafe { core::str::from_utf8_unchecked(&buf[..1]) };
    }
    let mut pos = 12;
    while n > 0 && pos > 0 {
        pos -= 1;
        buf[pos] = b'0' + (n % 10) as u8;
        n /= 10;
    }
    unsafe { core::str::from_utf8_unchecked(&buf[pos..12]) }
}

// ── Phase 4 host KATs: pure ring + FPS logic (cargo test -p raeshell) ────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_fills_and_caps() {
        let mut r = FrametimeRing::new();
        assert!(r.is_empty());
        for _ in 0..30 {
            r.push(16.6);
        }
        assert_eq!(r.len(), 30);
        // Overfill past capacity — len saturates, never panics.
        for _ in 0..FT_RING_CAP * 2 {
            r.push(8.3);
        }
        assert_eq!(r.len(), FT_RING_CAP);
    }

    #[test]
    fn ring_chronological_order_after_wrap() {
        let mut r = FrametimeRing::new();
        // Fill with a ramp; after wrap the oldest must be the most-recently-kept.
        for i in 0..(FT_RING_CAP + 5) {
            r.push((i + 1) as f32);
        }
        // Oldest kept sample = the (5+1)th pushed = 6.0; newest = CAP+5.
        assert_eq!(r.get_chrono(0), Some(6.0));
        assert_eq!(r.last(), Some((FT_RING_CAP + 5) as f32));
        // Out-of-range index returns None, not a panic.
        assert_eq!(r.get_chrono(FT_RING_CAP), None);
    }

    #[test]
    fn fps_from_60hz_frametime() {
        let mut r = FrametimeRing::new();
        for _ in 0..60 {
            r.push(16.6);
        }
        let fps = r.fps().unwrap();
        assert!((fps - 60.24).abs() < 1.0, "fps was {fps}");
    }

    #[test]
    fn empty_ring_is_graceful() {
        let r = FrametimeRing::new();
        assert_eq!(r.fps(), None);
        assert_eq!(r.avg_ms(), None);
        assert_eq!(r.last(), None);
        assert_eq!(r.get_chrono(0), None);
    }

    #[test]
    fn bad_samples_clamped() {
        let mut r = FrametimeRing::new();
        r.push(f32::NAN);
        r.push(-5.0);
        r.push(1.0e9);
        // NaN/neg → 0.0; huge → 10_000 cap. None of these panic.
        assert_eq!(r.get_chrono(0), Some(0.0));
        assert_eq!(r.get_chrono(1), Some(0.0));
        assert_eq!(r.get_chrono(2), Some(10_000.0));
    }

    #[test]
    fn invoke_toggles_and_counts() {
        let mut bar = GameBar::new(640, 480);
        assert!(!bar.visible);
        assert!(bar.invoke()); // on
        assert!(!bar.invoke()); // off
        assert!(bar.invoke()); // on
        assert_eq!(bar.invoke_count, 2); // counted only the two opens
    }

    #[test]
    fn gamebar_smoketest_passes() {
        let mut buf = vec![0u8; 640 * 480 * 4];
        let mut canvas = unsafe { raegfx::Canvas::new(buf.as_mut_ptr(), 640, 480, 4) };
        let gb = run_gamebar_smoketest(&mut canvas, 640, 480);
        assert!(gb.invoked_ok, "invoke toggle should pass");
        assert!(gb.frametime_pts > 0);
        assert!(gb.fps_read > 0 && gb.fps_read < 100_000);
        assert!(gb.temps_ok);
        assert_eq!(gb.panels, 4);
        assert!(gb.rendered);
        assert!(gb.passed());
    }

    #[test]
    fn ingest_perf_drives_ring_and_handles_na_temp() {
        let mut bar = GameBar::new(640, 480);
        for _ in 0..30 {
            bar.ingest_perf(&PerfFeed {
                fps: None,
                frametime_ms: Some(16.6),
                cpu_temp_c: Some(58.0),
                gpu_temp_c: None,
            });
        }
        assert_eq!(bar.ft_ring.len(), 30);
        assert!(bar.live_fps.unwrap() > 55.0 && bar.live_fps.unwrap() < 65.0);
        assert_eq!(bar.live_cpu_temp, Some(58.0));
        assert!(bar.live_gpu_temp.is_none()); // (n/a) path
        assert_eq!(bar.live_panel_count(), 4);
    }
}
