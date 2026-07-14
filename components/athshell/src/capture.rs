//! Capture & stream at the compositor — zero-cost recording, no OBS overhead.
//!
//! The `CaptureEngine` copies framebuffer data directly from the compositor's
//! surface, encodes it (placeholder for hardware-accelerated encode), and
//! writes to disk or pushes to a live-stream endpoint.  Screenshots are a
//! single-frame capture written as raw pixel data.

#![allow(unused)]

extern crate alloc;

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

// ── Colour constants for the on-screen indicator ─────────────────────────

const REC_RED: u32 = 0xFF_FF_22_22;
const TEXT_FG: u32 = 0xFF_F0_F0_F8;
const TEXT_DIM: u32 = 0xFF_88_8C_A0;
const OVERLAY_BG: u32 = 0xCC_0C_0E_18;
const GREEN: u32 = 0xFF_44_CC_66;
const ACCENT: u32 = 0xFF_4E_9C_FF;
const GLYPH_W: usize = 8;

// ── Public types ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureRegion {
    FullScreen,
    Window(u64),
    Region { x: u32, y: u32, w: u32, h: u32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoCodec {
    H264,
    H265,
    Av1,
    Vp9,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioCodec {
    Aac,
    Opus,
    Flac,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContainerFormat {
    Mp4,
    Mkv,
    Webm,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncoderQuality {
    Fast,
    Balanced,
    Quality,
    Lossless,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamProtocol {
    Rtmp,
    Srt,
    Whip,
}

pub struct EncoderConfig {
    pub codec: VideoCodec,
    pub bitrate_kbps: u32,
    pub fps: u32,
    pub quality: EncoderQuality,
    pub hardware_encode: bool,
    pub audio_codec: AudioCodec,
    pub audio_bitrate_kbps: u32,
    pub container: ContainerFormat,
}

impl Default for EncoderConfig {
    fn default() -> Self {
        Self {
            codec: VideoCodec::H265,
            bitrate_kbps: 20_000,
            fps: 60,
            quality: EncoderQuality::Balanced,
            hardware_encode: true,
            audio_codec: AudioCodec::Opus,
            audio_bitrate_kbps: 160,
            container: ContainerFormat::Mkv,
        }
    }
}

pub struct StreamConfig {
    pub url: String,
    pub stream_key: String,
    pub protocol: StreamProtocol,
    pub bitrate_kbps: u32,
    pub resolution: (u32, u32),
    pub fps: u32,
}

pub struct CaptureStats {
    pub frames_total: u64,
    pub frames_dropped: u64,
    pub bytes_written: u64,
    pub avg_encode_time_us: u64,
    pub avg_capture_time_us: u64,
    pub duration_secs: u64,
}

impl CaptureStats {
    fn new() -> Self {
        Self {
            frames_total: 0,
            frames_dropped: 0,
            bytes_written: 0,
            avg_encode_time_us: 0,
            avg_capture_time_us: 0,
            duration_secs: 0,
        }
    }

    pub fn drop_rate(&self) -> f32 {
        if self.frames_total == 0 {
            return 0.0;
        }
        self.frames_dropped as f32 / self.frames_total as f32
    }

    pub fn avg_bitrate_kbps(&self) -> u64 {
        if self.duration_secs == 0 {
            return 0;
        }
        (self.bytes_written * 8) / (self.duration_secs * 1000)
    }
}

// ── CaptureEngine ────────────────────────────────────────────────────────

pub struct CaptureEngine {
    pub recording: bool,
    pub record_start: u64,
    pub frames_captured: u64,
    pub output_path: String,
    pub encoder: EncoderConfig,
    pub framebuffer_copy: Vec<u8>,
    pub screenshot_buffer: Vec<u8>,
    pub capture_region: CaptureRegion,
    pub audio_capture: bool,
    pub microphone_capture: bool,
    pub streaming: bool,
    pub stream_config: Option<StreamConfig>,
    pub stats: CaptureStats,
    screen_width: u32,
    screen_height: u32,
    bpp: usize,
    encode_accumulator_us: u64,
    capture_accumulator_us: u64,
    sample_count: u64,
}

impl CaptureEngine {
    pub fn new(screen_width: u32, screen_height: u32, bpp: usize) -> Self {
        let fb_size = (screen_width as usize) * (screen_height as usize) * bpp;
        Self {
            recording: false,
            record_start: 0,
            frames_captured: 0,
            output_path: String::from("/recordings/capture.mkv"),
            encoder: EncoderConfig::default(),
            framebuffer_copy: vec![0u8; fb_size],
            screenshot_buffer: vec![0u8; fb_size],
            capture_region: CaptureRegion::FullScreen,
            audio_capture: true,
            microphone_capture: false,
            streaming: false,
            stream_config: None,
            stats: CaptureStats::new(),
            screen_width,
            screen_height,
            bpp,
            encode_accumulator_us: 0,
            capture_accumulator_us: 0,
            sample_count: 0,
        }
    }

    // ── Recording ────────────────────────────────────────────────────────

    pub fn start_recording(&mut self, timestamp: u64) -> bool {
        if self.recording {
            return false;
        }
        self.recording = true;
        self.record_start = timestamp;
        self.frames_captured = 0;
        self.stats = CaptureStats::new();
        true
    }

    pub fn stop_recording(&mut self, timestamp: u64) -> CaptureStats {
        self.recording = false;
        self.stats.duration_secs = timestamp.saturating_sub(self.record_start);

        if self.sample_count > 0 {
            self.stats.avg_encode_time_us = self.encode_accumulator_us / self.sample_count;
            self.stats.avg_capture_time_us = self.capture_accumulator_us / self.sample_count;
        }

        let result = CaptureStats {
            frames_total: self.stats.frames_total,
            frames_dropped: self.stats.frames_dropped,
            bytes_written: self.stats.bytes_written,
            avg_encode_time_us: self.stats.avg_encode_time_us,
            avg_capture_time_us: self.stats.avg_capture_time_us,
            duration_secs: self.stats.duration_secs,
        };

        self.encode_accumulator_us = 0;
        self.capture_accumulator_us = 0;
        self.sample_count = 0;

        result
    }

    // ── Frame capture ────────────────────────────────────────────────────

    pub fn capture_frame(
        &mut self,
        framebuffer: &[u8],
        capture_time_us: u64,
        encode_time_us: u64,
    ) -> bool {
        if !self.recording && !self.streaming {
            return false;
        }

        let region = self.resolve_region();
        let (rx, ry, rw, rh) = region;

        let row_bytes = rw as usize * self.bpp;
        let stride = self.screen_width as usize * self.bpp;

        let needed = rw as usize * rh as usize * self.bpp;
        if self.framebuffer_copy.len() < needed {
            self.framebuffer_copy.resize(needed, 0);
        }

        // Copy the capture region from the source framebuffer
        let mut dst_offset = 0;
        for row in 0..rh as usize {
            let src_y = ry as usize + row;
            let src_start = src_y * stride + rx as usize * self.bpp;
            let src_end = src_start + row_bytes;

            if src_end <= framebuffer.len() && dst_offset + row_bytes <= self.framebuffer_copy.len()
            {
                self.framebuffer_copy[dst_offset..dst_offset + row_bytes]
                    .copy_from_slice(&framebuffer[src_start..src_end]);
            }
            dst_offset += row_bytes;
        }

        self.frames_captured += 1;
        self.stats.frames_total += 1;

        // Simulated encode size: bitrate-based estimate
        let frame_bytes =
            (self.encoder.bitrate_kbps as u64 * 1000 / 8) / self.encoder.fps.max(1) as u64;
        self.stats.bytes_written += frame_bytes;

        self.capture_accumulator_us += capture_time_us;
        self.encode_accumulator_us += encode_time_us;
        self.sample_count += 1;

        true
    }

    fn resolve_region(&self) -> (u32, u32, u32, u32) {
        match self.capture_region {
            CaptureRegion::FullScreen => (0, 0, self.screen_width, self.screen_height),
            CaptureRegion::Window(_wid) => {
                // In a real implementation, this would query the compositor
                // for the window's position and size.
                (0, 0, self.screen_width, self.screen_height)
            }
            CaptureRegion::Region { x, y, w, h } => (
                x.min(self.screen_width),
                y.min(self.screen_height),
                w.min(self.screen_width - x.min(self.screen_width)),
                h.min(self.screen_height - y.min(self.screen_height)),
            ),
        }
    }

    // ── Screenshots ──────────────────────────────────────────────────────

    pub fn take_screenshot(&mut self, framebuffer: &[u8]) -> bool {
        let (rx, ry, rw, rh) = self.resolve_region();
        let row_bytes = rw as usize * self.bpp;
        let stride = self.screen_width as usize * self.bpp;
        let needed = rw as usize * rh as usize * self.bpp;

        if self.screenshot_buffer.len() < needed {
            self.screenshot_buffer.resize(needed, 0);
        }

        let mut dst_offset = 0;
        for row in 0..rh as usize {
            let src_y = ry as usize + row;
            let src_start = src_y * stride + rx as usize * self.bpp;
            let src_end = src_start + row_bytes;

            if src_end <= framebuffer.len()
                && dst_offset + row_bytes <= self.screenshot_buffer.len()
            {
                self.screenshot_buffer[dst_offset..dst_offset + row_bytes]
                    .copy_from_slice(&framebuffer[src_start..src_end]);
            }
            dst_offset += row_bytes;
        }

        true
    }

    pub fn save_screenshot(&self) -> Option<(&[u8], u32, u32)> {
        let (_, _, rw, rh) = self.resolve_region();
        let expected = rw as usize * rh as usize * self.bpp;
        if self.screenshot_buffer.len() >= expected && expected > 0 {
            Some((&self.screenshot_buffer[..expected], rw, rh))
        } else {
            None
        }
    }

    // ── Streaming ────────────────────────────────────────────────────────

    pub fn start_streaming(&mut self, config: StreamConfig) -> bool {
        if self.streaming {
            return false;
        }
        self.streaming = true;
        self.stream_config = Some(config);
        true
    }

    pub fn stop_streaming(&mut self) {
        self.streaming = false;
        self.stream_config = None;
    }

    // ── Stats ────────────────────────────────────────────────────────────

    pub fn get_stats(&self) -> &CaptureStats {
        &self.stats
    }

    pub fn is_active(&self) -> bool {
        self.recording || self.streaming
    }

    // ── On-screen indicator ──────────────────────────────────────────────

    pub fn render_indicator(&self, canvas: &mut athgfx::Canvas, screen_width: usize) {
        if !self.recording && !self.streaming {
            return;
        }

        let iw = 160usize;
        let ih = 28usize;
        let ix = screen_width - iw - 12;
        let iy = 12;

        canvas.fill_rect(ix, iy, iw, ih, OVERLAY_BG);

        if self.recording {
            // Red dot + REC
            canvas.fill_rect(ix + 6, iy + 10, 8, 8, REC_RED);
            canvas.draw_text(ix + 18, iy + 10, "REC", REC_RED, None);

            // Frame counter
            let mut buf = [0u8; 12];
            let frames_str = fmt_usize(self.frames_captured as usize, &mut buf);
            canvas.draw_text(ix + 50, iy + 10, frames_str, TEXT_DIM, None);
            let after = ix + 50 + frames_str.len() * GLYPH_W;
            canvas.draw_text(after, iy + 10, "f", TEXT_DIM, None);
        }

        if self.streaming {
            let stream_x = if self.recording { ix + 100 } else { ix + 6 };
            canvas.draw_text(stream_x, iy + 10, "LIVE", GREEN, None);

            // Show protocol
            if let Some(ref cfg) = self.stream_config {
                let proto = match cfg.protocol {
                    StreamProtocol::Rtmp => "RTMP",
                    StreamProtocol::Srt => "SRT",
                    StreamProtocol::Whip => "WHIP",
                };
                canvas.draw_text(stream_x + 5 * GLYPH_W, iy + 10, proto, TEXT_DIM, None);
            }
        }

        // Drop rate warning
        let drop_pct = self.stats.drop_rate() * 100.0;
        if drop_pct > 1.0 {
            let drop_i = drop_pct as usize;
            let mut buf = [0u8; 12];
            let drop_str = fmt_usize(drop_i, &mut buf);
            canvas.draw_text(ix + 6, iy + ih as usize + 4, "Drop:", REC_RED, None);
            canvas.draw_text(
                ix + 6 + 5 * GLYPH_W,
                iy + ih as usize + 4,
                drop_str,
                REC_RED,
                None,
            );
            let after = ix + 6 + 5 * GLYPH_W + drop_str.len() * GLYPH_W;
            canvas.draw_text(after, iy + ih as usize + 4, "%", REC_RED, None);
        }

        // Bitrate
        let kbps = self.stats.avg_bitrate_kbps();
        if kbps > 0 {
            let mbps = kbps / 1000;
            let mut buf = [0u8; 12];
            let br_str = fmt_usize(mbps as usize, &mut buf);
            let br_x = ix + iw - (br_str.len() + 5) * GLYPH_W;
            canvas.draw_text(br_x, iy + 10, br_str, ACCENT, None);
            let after = br_x + br_str.len() * GLYPH_W;
            canvas.draw_text(after, iy + 10, "Mbps", TEXT_DIM, None);
        }
    }
}

// ── Formatting helper ────────────────────────────────────────────────────

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
