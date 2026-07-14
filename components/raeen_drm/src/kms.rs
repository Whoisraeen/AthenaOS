//! KMS — Kernel Mode Setting: CRTC, connector, encoder, plane, framebuffer, mode.
//!
//! amdgpu's `amdgpu_dm` (Display Manager) drives these to light up a display.
//! The atomic-modeset commit path (`drm_atomic_commit`) is the choke point: the
//! driver builds a `drm_atomic_state` describing the desired CRTC/plane config,
//! and on commit we forward the final scanout buffer + mode to the AthenaOS
//! compositor.

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

/// `struct drm_display_mode` — one timing (resolution + refresh).
#[derive(Debug, Clone, Copy)]
pub struct DrmDisplayMode {
    pub hdisplay: u16,
    pub vdisplay: u16,
    pub clock_khz: u32,
    pub vrefresh: u16,
    pub flags: u32,
}

impl DrmDisplayMode {
    pub const fn new(w: u16, h: u16, hz: u16) -> Self {
        // Approximate pixel clock: w*h*hz with ~20% blanking overhead.
        let clock = ((w as u64) * (h as u64) * (hz as u64) * 12 / 10 / 1000) as u32;
        Self {
            hdisplay: w,
            vdisplay: h,
            clock_khz: clock,
            vrefresh: hz,
            flags: 0,
        }
    }
}

/// Connector status (`drm_connector_status`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectorStatus {
    Connected,
    Disconnected,
    Unknown,
}

/// Connector type (`DRM_MODE_CONNECTOR_*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectorType {
    Hdmi,
    DisplayPort,
    Edp,
    Dvi,
    Vga,
    Unknown,
}

/// `struct drm_framebuffer` — a scanout buffer (a GEM/TTM bo + format).
#[derive(Debug, Clone, Copy)]
pub struct DrmFramebuffer {
    pub fb_id: u64,
    pub width: u32,
    pub height: u32,
    pub pitch: u32,
    pub format_fourcc: u32, // e.g. DRM_FORMAT_XRGB8888
    /// DMA/GPU address of the scanout buffer (from TTM/GEM).
    pub gpu_addr: u64,
}

/// DRM_FORMAT_XRGB8888.
pub const DRM_FORMAT_XRGB8888: u32 = 0x34325258;
/// DRM_FORMAT_ARGB8888.
pub const DRM_FORMAT_ARGB8888: u32 = 0x34325241;

/// `struct drm_plane` — a scanout layer (primary/cursor/overlay).
pub struct DrmPlane {
    pub plane_id: u64,
    pub plane_type: PlaneType,
    pub fb: Option<DrmFramebuffer>,
    pub crtc_x: i32,
    pub crtc_y: i32,
    pub crtc_w: u32,
    pub crtc_h: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaneType {
    Primary,
    Cursor,
    Overlay,
}

/// `struct drm_crtc` — a display controller (scanout engine → one output).
pub struct DrmCrtc {
    pub crtc_id: u64,
    pub enabled: bool,
    pub active_mode: Option<DrmDisplayMode>,
    pub primary_plane: u64,
    pub cursor_plane: u64,
}

/// `struct drm_encoder` — converts CRTC output to a wire signal.
pub struct DrmEncoder {
    pub encoder_id: u64,
    pub crtc_id: u64,
}

/// `struct drm_connector` — a physical port + the monitor attached to it.
pub struct DrmConnector {
    pub connector_id: u64,
    pub conn_type: ConnectorType,
    pub status: ConnectorStatus,
    pub encoder_id: u64,
    /// EDID-derived mode list.
    pub modes: Vec<DrmDisplayMode>,
    pub name: String,
}

impl DrmConnector {
    /// `drm_connector_init` + `->detect()` — probe whether a monitor is attached.
    pub fn detect(&mut self) -> ConnectorStatus {
        // On real hardware this reads HPD (hot-plug detect) + DDC/EDID.
        // The amdgpu_dm path calls dc_link_detect(). Reported by the daemon.
        self.status
    }

    /// `drm_add_edid_modes` — populate the mode list from an EDID block.
    pub fn add_edid_modes(&mut self, edid: &[u8]) -> usize {
        // Minimal EDID detailed-timing parse: byte 54+ is the first DTD.
        // Real amdgpu uses drm_edid_to_eld + the full CEA extension walk.
        if edid.len() >= 128 && edid[0] == 0x00 && edid[1] == 0xFF {
            // Pull native resolution from the first detailed timing descriptor.
            let dtd = &edid[54..72];
            let hactive = ((dtd[4] as u16 & 0xF0) << 4) | dtd[2] as u16;
            let vactive = ((dtd[7] as u16 & 0xF0) << 4) | dtd[5] as u16;
            if hactive > 0 && vactive > 0 {
                self.modes.push(DrmDisplayMode::new(hactive, vactive, 60));
                return 1;
            }
        }
        // Fallback: offer a standard mode set.
        self.modes.push(DrmDisplayMode::new(1920, 1080, 60));
        self.modes.push(DrmDisplayMode::new(2560, 1440, 60));
        self.modes.push(DrmDisplayMode::new(3840, 2160, 60));
        3
    }

    pub fn preferred_mode(&self) -> Option<DrmDisplayMode> {
        self.modes.first().copied()
    }
}

/// `struct drm_atomic_state` — the desired display configuration for a commit.
pub struct DrmAtomicState {
    pub crtc_id: u64,
    pub mode: DrmDisplayMode,
    pub fb: DrmFramebuffer,
}

/// `drm_atomic_commit` — apply a display configuration. This is the modeset
/// choke point. We forward the final mode + scanout buffer to the compositor.
pub fn atomic_commit(state: &DrmAtomicState) -> i32 {
    super::log(&alloc::format!(
        "[drm] atomic_commit: crtc={} mode={}x{}@{}Hz fb={}x{} gpu_addr={:#x} -> compositor scanout",
        state.crtc_id, state.mode.hdisplay, state.mode.vdisplay, state.mode.vrefresh,
        state.fb.width, state.fb.height, state.fb.gpu_addr,
    ));
    // MasterChecklist Phase 6: forward to compositor::attach_gpu_scanout via IPC.
    // The daemon sends a SURFACE/scanout message with (gpu_addr, mode) here.
    0
}
