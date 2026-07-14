#![allow(dead_code)]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

// ─── Connector Types ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectorType {
    Vga,
    DviI,
    DviD,
    DviA,
    Hdmi,
    DisplayPort,
    Lvds,
    EDP,
    Dsi,
    Virtual,
    Unknown,
}

impl ConnectorType {
    pub fn name(&self) -> &'static str {
        match self {
            ConnectorType::Vga => "VGA",
            ConnectorType::DviI => "DVI-I",
            ConnectorType::DviD => "DVI-D",
            ConnectorType::DviA => "DVI-A",
            ConnectorType::Hdmi => "HDMI",
            ConnectorType::DisplayPort => "DisplayPort",
            ConnectorType::Lvds => "LVDS",
            ConnectorType::EDP => "eDP",
            ConnectorType::Dsi => "DSI",
            ConnectorType::Virtual => "Virtual",
            ConnectorType::Unknown => "Unknown",
        }
    }

    pub fn is_digital(&self) -> bool {
        !matches!(self, ConnectorType::Vga | ConnectorType::DviA)
    }

    pub fn supports_audio(&self) -> bool {
        matches!(
            self,
            ConnectorType::Hdmi | ConnectorType::DisplayPort | ConnectorType::EDP
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectorState {
    Connected,
    Disconnected,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubpixelOrder {
    Unknown,
    HorizontalRgb,
    HorizontalBgr,
    VerticalRgb,
    VerticalBgr,
    None,
}

// ─── Display Mode ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub struct ModeFlags {
    pub preferred: bool,
    pub vrr_capable: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct DisplayMode {
    pub width: u32,
    pub height: u32,
    pub refresh_hz: u32,
    pub clock_khz: u32,
    pub hsync_start: u32,
    pub hsync_end: u32,
    pub htotal: u32,
    pub vsync_start: u32,
    pub vsync_end: u32,
    pub vtotal: u32,
    pub flags: ModeFlags,
    pub interlaced: bool,
}

impl DisplayMode {
    pub fn pixel_clock_mhz(&self) -> f32 {
        self.clock_khz as f32 / 1000.0
    }

    pub fn total_pixels(&self) -> u64 {
        self.htotal as u64 * self.vtotal as u64
    }

    pub fn active_pixels(&self) -> u64 {
        self.width as u64 * self.height as u64
    }

    pub fn aspect_ratio(&self) -> (u32, u32) {
        let g = gcd(self.width, self.height);
        if g == 0 {
            return (0, 0);
        }
        (self.width / g, self.height / g)
    }

    pub fn bandwidth_gbps(&self) -> f32 {
        let bpp = 24;
        (self.width as f32 * self.height as f32 * self.refresh_hz as f32 * bpp as f32)
            / 1_000_000_000.0
    }

    pub fn is_4k(&self) -> bool {
        self.width >= 3840 && self.height >= 2160
    }

    pub fn is_ultrawide(&self) -> bool {
        if self.height == 0 {
            return false;
        }
        let ratio = self.width as f32 / self.height as f32;
        ratio > 2.0
    }
}

fn gcd(a: u32, b: u32) -> u32 {
    if b == 0 {
        a
    } else {
        gcd(b, a % b)
    }
}

pub fn mode_1080p60() -> DisplayMode {
    DisplayMode {
        width: 1920,
        height: 1080,
        refresh_hz: 60,
        clock_khz: 148500,
        hsync_start: 2008,
        hsync_end: 2052,
        htotal: 2200,
        vsync_start: 1084,
        vsync_end: 1089,
        vtotal: 1125,
        flags: ModeFlags {
            preferred: true,
            vrr_capable: false,
        },
        interlaced: false,
    }
}

pub fn mode_4k60() -> DisplayMode {
    DisplayMode {
        width: 3840,
        height: 2160,
        refresh_hz: 60,
        clock_khz: 594000,
        hsync_start: 4016,
        hsync_end: 4104,
        htotal: 4400,
        vsync_start: 2168,
        vsync_end: 2178,
        vtotal: 2250,
        flags: ModeFlags {
            preferred: false,
            vrr_capable: true,
        },
        interlaced: false,
    }
}

pub fn mode_1440p144() -> DisplayMode {
    DisplayMode {
        width: 2560,
        height: 1440,
        refresh_hz: 144,
        clock_khz: 586589,
        hsync_start: 2608,
        hsync_end: 2640,
        htotal: 2720,
        vsync_start: 1443,
        vsync_end: 1448,
        vtotal: 1497,
        flags: ModeFlags {
            preferred: false,
            vrr_capable: true,
        },
        interlaced: false,
    }
}

// ─── EDID ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HdrEotf {
    Sdr,
    HdrPq,
    HdrHlg,
    Traditional,
}

#[derive(Debug, Clone)]
pub struct HdrMetadata {
    pub eotf: HdrEotf,
    pub max_luminance: u16,
    pub min_luminance: u16,
    pub max_content_light: u16,
    pub max_frame_avg_light: u16,
    pub primaries: [[u16; 2]; 3],
    pub white_point: [u16; 2],
}

impl HdrMetadata {
    pub fn bt2020_pq() -> Self {
        Self {
            eotf: HdrEotf::HdrPq,
            max_luminance: 1000,
            min_luminance: 1,
            max_content_light: 1000,
            max_frame_avg_light: 400,
            primaries: [[34000, 16000], [13250, 34500], [7500, 3000]],
            white_point: [15635, 16450],
        }
    }
}

#[derive(Debug, Clone)]
pub struct EdidInfo {
    pub manufacturer: [u8; 3],
    pub product_code: u16,
    pub serial_number: u32,
    pub manufacture_year: u16,
    pub manufacture_week: u8,
    pub version: (u8, u8),
    pub digital: bool,
    pub width_cm: u8,
    pub height_cm: u8,
    pub preferred_mode: DisplayMode,
    pub monitor_name: String,
    pub color_depth: u8,
    pub hdr_metadata: Option<HdrMetadata>,
    pub vrr_range: Option<(u32, u32)>,
}

impl EdidInfo {
    pub fn diagonal_inches(&self) -> f32 {
        let w = self.width_cm as f32;
        let h = self.height_cm as f32;
        sqrt_approx(w * w + h * h) / 2.54
    }

    pub fn ppi(&self) -> f32 {
        let w = self.preferred_mode.width as f32;
        let h = self.preferred_mode.height as f32;
        let diag_px = sqrt_approx(w * w + h * h);
        let diag_in = self.diagonal_inches();
        if diag_in < 0.1 {
            return 0.0;
        }
        diag_px / diag_in
    }

    pub fn supports_hdr(&self) -> bool {
        self.hdr_metadata.is_some()
    }

    pub fn supports_vrr(&self) -> bool {
        self.vrr_range.is_some()
    }

    pub fn manufacturer_id(&self) -> [char; 3] {
        [
            (self.manufacturer[0] + b'A' - 1) as char,
            (self.manufacturer[1] + b'A' - 1) as char,
            (self.manufacturer[2] + b'A' - 1) as char,
        ]
    }
}

fn sqrt_approx(x: f32) -> f32 {
    if x <= 0.0 {
        return 0.0;
    }
    let bits = x.to_bits();
    let approx = f32::from_bits((bits >> 1) + 0x1FC0_0000);
    0.5 * (approx + x / approx)
}

fn pow2_approx(x: f32) -> f32 {
    x * x
}

// ─── KMS Pipeline ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncoderType {
    DacAnalog,
    TmdsDvi,
    LvdsPanel,
    DpMst,
    Virtual,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaneType {
    Primary,
    Overlay,
    Cursor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rotation {
    None,
    Rotate90,
    Rotate180,
    Rotate270,
    FlipH,
    FlipV,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HdrDisplayMode {
    Sdr,
    Hdr10,
    DolbyVision,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ColorProfile {
    Srgb,
    AdobeRgb,
    DciP3,
    Bt2020,
    Custom(String),
}

#[derive(Debug)]
pub struct Crtc {
    pub id: u32,
    pub active: bool,
    pub mode: Option<DisplayMode>,
    pub gamma_size: u32,
}

impl Crtc {
    pub fn new(id: u32) -> Self {
        Self {
            id,
            active: false,
            mode: None,
            gamma_size: 256,
        }
    }

    pub fn set_mode(&mut self, mode: DisplayMode) {
        self.mode = Some(mode);
        self.active = true;
    }

    pub fn disable(&mut self) {
        self.active = false;
        self.mode = None;
    }
}

#[derive(Debug)]
pub struct Encoder {
    pub id: u32,
    pub encoder_type: EncoderType,
    pub possible_crtcs: u32,
}

impl Encoder {
    pub fn can_use_crtc(&self, crtc_index: u8) -> bool {
        (self.possible_crtcs >> crtc_index) & 1 == 1
    }
}

#[derive(Debug)]
pub struct Plane {
    pub id: u32,
    pub plane_type: PlaneType,
    pub formats: Vec<u32>,
    pub crtc_id: Option<u32>,
}

impl Plane {
    pub fn supports_format(&self, fourcc: u32) -> bool {
        self.formats.contains(&fourcc)
    }
}

#[derive(Debug)]
pub struct Framebuffer {
    pub id: u32,
    pub width: u32,
    pub height: u32,
    pub format: u32,
    pub handle: u64,
}

impl Framebuffer {
    pub fn stride(&self) -> u32 {
        self.width * self.bpp() / 8
    }

    pub fn bpp(&self) -> u32 {
        match self.format {
            0x34325258 => 32, // XRGB8888
            0x34324258 => 32, // XBGR8888
            0x34325241 => 32, // ARGB8888
            0x36314752 => 16, // RGB565
            _ => 32,
        }
    }

    pub fn size_bytes(&self) -> u64 {
        self.stride() as u64 * self.height as u64
    }
}

// ─── Display Connector ─────────────────────────────────────────────────────

pub struct DisplayConnector {
    pub id: u32,
    pub connector_type: ConnectorType,
    pub state: ConnectorState,
    pub edid: Option<EdidInfo>,
    pub modes: Vec<DisplayMode>,
    pub current_mode: Option<usize>,
    pub physical_width_mm: u32,
    pub physical_height_mm: u32,
    pub subpixel: SubpixelOrder,
}

impl DisplayConnector {
    pub fn new(id: u32, connector_type: ConnectorType) -> Self {
        Self {
            id,
            connector_type,
            state: ConnectorState::Disconnected,
            edid: None,
            modes: Vec::new(),
            current_mode: None,
            physical_width_mm: 0,
            physical_height_mm: 0,
            subpixel: SubpixelOrder::Unknown,
        }
    }

    pub fn is_connected(&self) -> bool {
        self.state == ConnectorState::Connected
    }

    pub fn preferred_mode(&self) -> Option<&DisplayMode> {
        self.modes
            .iter()
            .find(|m| m.flags.preferred)
            .or_else(|| self.modes.first())
    }

    pub fn active_mode(&self) -> Option<&DisplayMode> {
        self.current_mode.and_then(|idx| self.modes.get(idx))
    }

    pub fn find_mode(&self, width: u32, height: u32, refresh: u32) -> Option<usize> {
        self.modes
            .iter()
            .position(|m| m.width == width && m.height == height && m.refresh_hz == refresh)
    }

    pub fn best_mode(&self) -> Option<usize> {
        if self.modes.is_empty() {
            return None;
        }
        let mut best = 0usize;
        for (i, mode) in self.modes.iter().enumerate() {
            let cur = &self.modes[best];
            let score = (mode.width as u64 * mode.height as u64) * mode.refresh_hz as u64;
            let best_score = (cur.width as u64 * cur.height as u64) * cur.refresh_hz as u64;
            if score > best_score {
                best = i;
            }
        }
        Some(best)
    }

    pub fn connect(&mut self, edid: EdidInfo, modes: Vec<DisplayMode>) {
        self.state = ConnectorState::Connected;
        self.physical_width_mm = edid.width_cm as u32 * 10;
        self.physical_height_mm = edid.height_cm as u32 * 10;
        self.edid = Some(edid);
        self.modes = modes;
        self.subpixel = SubpixelOrder::HorizontalRgb;
    }

    pub fn disconnect(&mut self) {
        self.state = ConnectorState::Disconnected;
        self.edid = None;
        self.modes.clear();
        self.current_mode = None;
    }

    pub fn vrr_capable(&self) -> bool {
        self.edid.as_ref().map_or(false, |e| e.supports_vrr())
    }

    pub fn hdr_capable(&self) -> bool {
        self.edid.as_ref().map_or(false, |e| e.supports_hdr())
    }
}

// ─── Display Config ─────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct DisplayConfig {
    pub connector_id: u32,
    pub crtc_id: u32,
    pub mode: DisplayMode,
    pub position: (i32, i32),
    pub rotation: Rotation,
    pub scale: f32,
    pub hdr_mode: HdrDisplayMode,
    pub vrr_enabled: bool,
    pub color_profile: ColorProfile,
}

impl DisplayConfig {
    pub fn new(connector_id: u32, crtc_id: u32, mode: DisplayMode) -> Self {
        Self {
            connector_id,
            crtc_id,
            mode,
            position: (0, 0),
            rotation: Rotation::None,
            scale: 1.0,
            hdr_mode: HdrDisplayMode::Sdr,
            vrr_enabled: false,
            color_profile: ColorProfile::Srgb,
        }
    }

    pub fn scaled_width(&self) -> u32 {
        (self.mode.width as f32 / self.scale) as u32
    }

    pub fn scaled_height(&self) -> u32 {
        (self.mode.height as f32 / self.scale) as u32
    }

    pub fn effective_rect(&self) -> (i32, i32, u32, u32) {
        match self.rotation {
            Rotation::Rotate90 | Rotation::Rotate270 => (
                self.position.0,
                self.position.1,
                self.scaled_height(),
                self.scaled_width(),
            ),
            _ => (
                self.position.0,
                self.position.1,
                self.scaled_width(),
                self.scaled_height(),
            ),
        }
    }
}

// ─── Hotplug Event ──────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct HotplugEvent {
    pub connector_id: u32,
    pub connected: bool,
    pub timestamp: u64,
}

// ─── Display Manager ────────────────────────────────────────────────────────

pub struct DisplayManager {
    pub connectors: Vec<DisplayConnector>,
    pub crtcs: Vec<Crtc>,
    pub encoders: Vec<Encoder>,
    pub planes: Vec<Plane>,
    pub framebuffers: Vec<Framebuffer>,
    pub active_config: Vec<DisplayConfig>,
    pub hotplug_events: Vec<HotplugEvent>,
    next_fb_id: u32,
    tick_counter: u64,
}

impl DisplayManager {
    pub fn new() -> Self {
        Self {
            connectors: Vec::new(),
            crtcs: Vec::new(),
            encoders: Vec::new(),
            planes: Vec::new(),
            framebuffers: Vec::new(),
            active_config: Vec::new(),
            hotplug_events: Vec::new(),
            next_fb_id: 1,
            tick_counter: 0,
        }
    }

    pub fn init(&mut self) {
        // Set up two CRTCs (dual-head capable).
        self.crtcs.push(Crtc::new(0));
        self.crtcs.push(Crtc::new(1));

        // Encoders bridging CRTCs to connectors.
        self.encoders.push(Encoder {
            id: 0,
            encoder_type: EncoderType::TmdsDvi,
            possible_crtcs: 0b11,
        });
        self.encoders.push(Encoder {
            id: 1,
            encoder_type: EncoderType::DpMst,
            possible_crtcs: 0b11,
        });

        let xrgb8888: u32 = 0x34325258;
        let argb8888: u32 = 0x34325241;

        // Primary plane per CRTC.
        self.planes.push(Plane {
            id: 0,
            plane_type: PlaneType::Primary,
            formats: alloc::vec![xrgb8888, argb8888],
            crtc_id: Some(0),
        });
        self.planes.push(Plane {
            id: 1,
            plane_type: PlaneType::Primary,
            formats: alloc::vec![xrgb8888, argb8888],
            crtc_id: Some(1),
        });

        // Overlay + cursor planes on CRTC 0.
        self.planes.push(Plane {
            id: 2,
            plane_type: PlaneType::Overlay,
            formats: alloc::vec![xrgb8888, argb8888],
            crtc_id: Some(0),
        });
        self.planes.push(Plane {
            id: 3,
            plane_type: PlaneType::Cursor,
            formats: alloc::vec![argb8888],
            crtc_id: Some(0),
        });

        // Pre-populate connectors.
        self.connectors
            .push(DisplayConnector::new(0, ConnectorType::Hdmi));
        self.connectors
            .push(DisplayConnector::new(1, ConnectorType::DisplayPort));
        self.connectors
            .push(DisplayConnector::new(2, ConnectorType::Virtual));
    }

    pub fn detect_displays(&mut self) {
        // Simulate an HDMI monitor connected on connector 0.
        let edid = EdidInfo {
            manufacturer: [4, 3, 13], // "DCM"
            product_code: 0x27D8,
            serial_number: 0x0001_0001,
            manufacture_year: 2024,
            manufacture_week: 42,
            version: (1, 4),
            digital: true,
            width_cm: 60,
            height_cm: 34,
            preferred_mode: mode_1080p60(),
            monitor_name: String::from("AthenaOS Virtual Display"),
            color_depth: 8,
            hdr_metadata: Some(HdrMetadata::bt2020_pq()),
            vrr_range: Some((48, 144)),
        };

        let modes = alloc::vec![
            mode_1080p60(),
            mode_4k60(),
            mode_1440p144(),
            DisplayMode {
                width: 1280,
                height: 720,
                refresh_hz: 60,
                clock_khz: 74250,
                hsync_start: 1390,
                hsync_end: 1430,
                htotal: 1650,
                vsync_start: 725,
                vsync_end: 730,
                vtotal: 750,
                flags: ModeFlags {
                    preferred: false,
                    vrr_capable: false
                },
                interlaced: false,
            },
            DisplayMode {
                width: 1920,
                height: 1080,
                refresh_hz: 144,
                clock_khz: 346500,
                hsync_start: 2008,
                hsync_end: 2052,
                htotal: 2200,
                vsync_start: 1084,
                vsync_end: 1089,
                vtotal: 1100,
                flags: ModeFlags {
                    preferred: false,
                    vrr_capable: true
                },
                interlaced: false,
            },
        ];

        if let Some(conn) = self.connectors.get_mut(0) {
            conn.connect(edid, modes);
        }

        // Virtual connector is always connected.
        if let Some(conn) = self.connectors.get_mut(2) {
            let vedid = EdidInfo {
                manufacturer: [18, 1, 5], // "RAE"
                product_code: 0x0001,
                serial_number: 0,
                manufacture_year: 2026,
                manufacture_week: 1,
                version: (1, 4),
                digital: true,
                width_cm: 0,
                height_cm: 0,
                preferred_mode: mode_1080p60(),
                monitor_name: String::from("AthenaOS Virtual Framebuffer"),
                color_depth: 8,
                hdr_metadata: None,
                vrr_range: None,
            };
            conn.connect(vedid, alloc::vec![mode_1080p60()]);
        }
    }

    pub fn parse_edid(&self, raw: &[u8]) -> Option<EdidInfo> {
        if raw.len() < 128 {
            return None;
        }
        // EDID header validation: 00 FF FF FF FF FF FF 00
        if raw[0] != 0x00 || raw[1] != 0xFF || raw[6] != 0xFF || raw[7] != 0x00 {
            return None;
        }

        let mfg_bytes = ((raw[8] as u16) << 8) | raw[9] as u16;
        let manufacturer = [
            ((mfg_bytes >> 10) & 0x1F) as u8,
            ((mfg_bytes >> 5) & 0x1F) as u8,
            (mfg_bytes & 0x1F) as u8,
        ];

        let product_code = (raw[10] as u16) | ((raw[11] as u16) << 8);
        let serial_number = u32::from_le_bytes([raw[12], raw[13], raw[14], raw[15]]);
        let manufacture_week = raw[16];
        let manufacture_year = raw[17] as u16 + 1990;
        let version = (raw[18], raw[19]);
        let digital = (raw[20] & 0x80) != 0;
        let width_cm = raw[21];
        let height_cm = raw[22];

        let mut monitor_name = String::from("Unknown Monitor");
        for block_start in (54..126).step_by(18) {
            if raw[block_start] == 0
                && raw[block_start + 1] == 0
                && raw[block_start + 2] == 0
                && raw[block_start + 3] == 0xFC
            {
                let name_bytes = &raw[block_start + 5..block_start + 18];
                let end = name_bytes.iter().position(|&b| b == 0x0A).unwrap_or(13);
                let mut name = String::new();
                for &b in &name_bytes[..end] {
                    if b >= 0x20 && b < 0x7F {
                        name.push(b as char);
                    }
                }
                if !name.is_empty() {
                    monitor_name = name;
                }
                break;
            }
        }

        Some(EdidInfo {
            manufacturer,
            product_code,
            serial_number,
            manufacture_year,
            manufacture_week,
            version,
            digital,
            width_cm,
            height_cm,
            preferred_mode: mode_1080p60(),
            monitor_name,
            color_depth: if digital { 8 } else { 6 },
            hdr_metadata: None,
            vrr_range: None,
        })
    }

    pub fn apply_config(&mut self, config: DisplayConfig) -> Result<(), &'static str> {
        let crtc = self
            .crtcs
            .get_mut(config.crtc_id as usize)
            .ok_or("invalid CRTC id")?;
        let _ = self
            .connectors
            .get(config.connector_id as usize)
            .ok_or("invalid connector id")?;

        crtc.set_mode(config.mode);
        self.active_config
            .retain(|c| c.connector_id != config.connector_id);
        self.active_config.push(config);
        Ok(())
    }

    pub fn set_mode(&mut self, connector_id: u32, mode_index: usize) -> Result<(), &'static str> {
        let connector = self
            .connectors
            .get_mut(connector_id as usize)
            .ok_or("invalid connector id")?;

        if !connector.is_connected() {
            return Err("connector not connected");
        }

        let mode = *connector
            .modes
            .get(mode_index)
            .ok_or("invalid mode index")?;
        connector.current_mode = Some(mode_index);

        if let Some(crtc) = self.crtcs.first_mut() {
            crtc.set_mode(mode);
        }

        Ok(())
    }

    pub fn handle_hotplug(&mut self, connector_id: u32, connected: bool) {
        self.tick_counter += 1;
        let event = HotplugEvent {
            connector_id,
            connected,
            timestamp: self.tick_counter,
        };
        self.hotplug_events.push(event);

        if !connected {
            if let Some(conn) = self.connectors.get_mut(connector_id as usize) {
                conn.disconnect();
            }
            self.active_config
                .retain(|c| c.connector_id != connector_id);
        }
    }

    pub fn get_preferred_mode(&self, connector_id: u32) -> Option<&DisplayMode> {
        self.connectors
            .get(connector_id as usize)
            .and_then(|c| c.preferred_mode())
    }

    pub fn list_modes(&self, connector_id: u32) -> &[DisplayMode] {
        self.connectors
            .get(connector_id as usize)
            .map(|c| c.modes.as_slice())
            .unwrap_or(&[])
    }

    pub fn enable_vrr(&mut self, connector_id: u32) -> Result<(), &'static str> {
        let conn = self
            .connectors
            .get(connector_id as usize)
            .ok_or("invalid connector id")?;

        if !conn.vrr_capable() {
            return Err("connector does not support VRR");
        }

        if let Some(cfg) = self
            .active_config
            .iter_mut()
            .find(|c| c.connector_id == connector_id)
        {
            cfg.vrr_enabled = true;
        }
        Ok(())
    }

    pub fn set_hdr_metadata(
        &mut self,
        connector_id: u32,
        mode: HdrDisplayMode,
    ) -> Result<(), &'static str> {
        let conn = self
            .connectors
            .get(connector_id as usize)
            .ok_or("invalid connector id")?;

        if mode != HdrDisplayMode::Sdr && !conn.hdr_capable() {
            return Err("connector does not support HDR");
        }

        if let Some(cfg) = self
            .active_config
            .iter_mut()
            .find(|c| c.connector_id == connector_id)
        {
            cfg.hdr_mode = mode;
        }
        Ok(())
    }

    pub fn create_framebuffer(&mut self, width: u32, height: u32, format: u32, handle: u64) -> u32 {
        let id = self.next_fb_id;
        self.next_fb_id += 1;
        self.framebuffers.push(Framebuffer {
            id,
            width,
            height,
            format,
            handle,
        });
        id
    }

    pub fn destroy_framebuffer(&mut self, fb_id: u32) {
        self.framebuffers.retain(|fb| fb.id != fb_id);
    }

    pub fn page_flip(&mut self, crtc_id: u32, fb_id: u32) -> Result<(), &'static str> {
        let _ = self.crtcs.get(crtc_id as usize).ok_or("invalid CRTC id")?;
        let _ = self
            .framebuffers
            .iter()
            .find(|fb| fb.id == fb_id)
            .ok_or("invalid framebuffer id")?;
        Ok(())
    }

    pub fn set_gamma(
        &mut self,
        crtc_id: u32,
        _red: &[u16],
        _green: &[u16],
        _blue: &[u16],
    ) -> Result<(), &'static str> {
        let crtc = self.crtcs.get(crtc_id as usize).ok_or("invalid CRTC id")?;

        if !crtc.active {
            return Err("CRTC is not active");
        }

        Ok(())
    }

    pub fn get_connector_info(&self, connector_id: u32) -> Option<ConnectorInfo> {
        let conn = self.connectors.get(connector_id as usize)?;
        Some(ConnectorInfo {
            id: conn.id,
            connector_type: conn.connector_type,
            state: conn.state,
            mode_count: conn.modes.len(),
            physical_width_mm: conn.physical_width_mm,
            physical_height_mm: conn.physical_height_mm,
            subpixel: conn.subpixel,
            monitor_name: conn.edid.as_ref().map(|e| e.monitor_name.clone()),
            vrr_capable: conn.vrr_capable(),
            hdr_capable: conn.hdr_capable(),
        })
    }

    pub fn connected_count(&self) -> usize {
        self.connectors.iter().filter(|c| c.is_connected()).count()
    }

    pub fn total_desktop_size(&self) -> (i32, i32, u32, u32) {
        if self.active_config.is_empty() {
            return (0, 0, 0, 0);
        }
        let mut min_x = i32::MAX;
        let mut min_y = i32::MAX;
        let mut max_x = i32::MIN;
        let mut max_y = i32::MIN;

        for cfg in &self.active_config {
            let (x, y, w, h) = cfg.effective_rect();
            if x < min_x {
                min_x = x;
            }
            if y < min_y {
                min_y = y;
            }
            if x + w as i32 > max_x {
                max_x = x + w as i32;
            }
            if y + h as i32 > max_y {
                max_y = y + h as i32;
            }
        }

        (min_x, min_y, (max_x - min_x) as u32, (max_y - min_y) as u32)
    }
}

#[derive(Debug, Clone)]
pub struct ConnectorInfo {
    pub id: u32,
    pub connector_type: ConnectorType,
    pub state: ConnectorState,
    pub mode_count: usize,
    pub physical_width_mm: u32,
    pub physical_height_mm: u32,
    pub subpixel: SubpixelOrder,
    pub monitor_name: Option<String>,
    pub vrr_capable: bool,
    pub hdr_capable: bool,
}

// ─── Global State ───────────────────────────────────────────────────────────

pub static DISPLAY_MANAGER: Mutex<Option<DisplayManager>> = Mutex::new(None);

pub fn init() {
    let mut mgr = DisplayManager::new();
    mgr.init();
    mgr.detect_displays();

    // Auto-configure the first connected display with its preferred mode.
    for i in 0..mgr.connectors.len() {
        if mgr.connectors[i].is_connected() {
            if let Some(mode) = mgr.connectors[i].preferred_mode().copied() {
                let crtc_id = if i < mgr.crtcs.len() { i as u32 } else { 0 };
                let config = DisplayConfig::new(i as u32, crtc_id, mode);
                let _ = mgr.apply_config(config);
            }
        }
    }

    *DISPLAY_MANAGER.lock() = Some(mgr);
}

/// Deterministic proof of EDID parsing with ZERO hardware access: a synthesized
/// 128-byte EDID block is decoded and its manufacturer ID, product code,
/// manufacture year, digital flag and monitor name are checked, plus rejection
/// of a corrupt header and a short buffer. MasterChecklist Phase 2.3 — "EDID +
/// display modes". Concept §display.
pub fn run_boot_smoketest() {
    let mgr = DisplayManager::new();
    let mut pass = 0u32;
    let mut total = 0u32;
    let mut check = |c: bool, n: &str| {
        total += 1;
        if c {
            pass += 1;
        } else {
            crate::serial_println!("[edid-selftest] FAIL {}", n);
        }
    };

    // Synthesize a valid EDID 1.4 block: mfg "RAE", product 0x1234, year 2024,
    // digital input, monitor name "RaeMon".
    let mut edid = [0u8; 128];
    edid[0..8].copy_from_slice(&[0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00]);
    edid[8] = 0x48; // mfg id hi  ─┐ "RAE" = (18<<10)|(1<<5)|5 = 0x4825
    edid[9] = 0x25; // mfg id lo  ─┘
    edid[10] = 0x34; // product code 0x1234 (little-endian)
    edid[11] = 0x12;
    edid[12] = 0x01; // serial number (LE) = 1
    edid[16] = 10; // manufacture week
    edid[17] = 34; // manufacture year = 1990 + 34 = 2024
    edid[18] = 1; // EDID version 1.4
    edid[19] = 4;
    edid[20] = 0x80; // digital input flag
    edid[21] = 60; // screen width cm
    edid[22] = 34; // screen height cm
    edid[57] = 0xFC; // descriptor block 1 = monitor name
    edid[59..65].copy_from_slice(b"RaeMon");
    edid[65] = 0x0A; // name terminator

    match mgr.parse_edid(&edid) {
        Some(info) => {
            check(info.manufacturer == [18, 1, 5], "edid-manufacturer");
            check(info.product_code == 0x1234, "edid-product");
            check(info.manufacture_year == 2024, "edid-year");
            check(info.digital, "edid-digital");
            check(info.monitor_name == "RaeMon", "edid-name");
        }
        None => {
            for n in [
                "edid-manufacturer",
                "edid-product",
                "edid-year",
                "edid-digital",
                "edid-name",
            ] {
                check(false, n);
            }
        }
    }

    // Negative cases: a corrupt header byte and a too-short buffer are rejected.
    let mut bad = edid;
    bad[0] = 0xFF;
    check(mgr.parse_edid(&bad).is_none(), "edid-reject-bad-header");
    check(mgr.parse_edid(&edid[..64]).is_none(), "edid-reject-short");

    drop(check);
    crate::serial_println!(
        "[ OK ] EDID parse selftest: {}/{} checks passed (header + manufacturer + name, no hardware)",
        pass,
        total
    );
    if pass != total {
        crate::serial_println!(
            "[FAIL] EDID parse selftest: {} check(s) failed",
            total - pass
        );
    }
}
