//! AthPlay — PARKED game launcher (not an AthenaOS product surface).
//!
//! Bootstrap residue: Steam/Epic/GOG library aggregation. Do not expand for
//! Athena; see `docs/PARKED_GAMING.md`. Types remain so the workspace builds.
#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

// ---------------------------------------------------------------------------
// Core identifiers
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GameId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum StoreType {
    Steam,
    Epic,
    Gog,
    AthStore,
    Manual,
}

impl fmt::Display for StoreType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StoreType::Steam => f.write_str("Steam"),
            StoreType::Epic => f.write_str("Epic Games"),
            StoreType::Gog => f.write_str("GOG"),
            StoreType::AthStore => f.write_str("AthStore"),
            StoreType::Manual => f.write_str("Manual"),
        }
    }
}

// ---------------------------------------------------------------------------
// Game state machine
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GameState {
    NotInstalled,
    Queued,
    Installing { progress_pct: u8 },
    Installed,
    Updating { progress_pct: u8 },
    Running { pid: u64 },
    UpdateAvailable,
    Corrupted,
}

impl GameState {
    pub fn is_playable(&self) -> bool {
        matches!(self, GameState::Installed | GameState::UpdateAvailable)
    }

    pub fn is_busy(&self) -> bool {
        matches!(
            self,
            GameState::Installing { .. } | GameState::Updating { .. } | GameState::Running { .. }
        )
    }
}

// ---------------------------------------------------------------------------
// Display / fullscreen / HDR enums
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FullscreenMode {
    Windowed,
    Borderless,
    ExclusiveFullscreen,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HdrMode {
    Off,
    Auto,
    ForceSdr,
    ForceHdr10,
    ForceDolbyVision,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VsyncMode {
    Off,
    On,
    Adaptive,
}

// ---------------------------------------------------------------------------
// GPU profile types
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct FanCurvePoint {
    pub temp_c: u8,
    pub fan_pct: u8,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FanCurve {
    pub points: Vec<FanCurvePoint>,
}

impl FanCurve {
    pub fn new() -> Self {
        Self { points: Vec::new() }
    }

    pub fn add_point(&mut self, temp_c: u8, fan_pct: u8) {
        self.points.push(FanCurvePoint { temp_c, fan_pct });
        self.points.sort_by_key(|p| p.temp_c);
    }

    pub fn fan_pct_at(&self, temp_c: u8) -> u8 {
        if self.points.is_empty() {
            return 50;
        }
        if temp_c <= self.points[0].temp_c {
            return self.points[0].fan_pct;
        }
        let last = self.points.len() - 1;
        if temp_c >= self.points[last].temp_c {
            return self.points[last].fan_pct;
        }
        for i in 0..last {
            let lo = &self.points[i];
            let hi = &self.points[i + 1];
            if temp_c >= lo.temp_c && temp_c <= hi.temp_c {
                let range_t = (hi.temp_c - lo.temp_c) as u32;
                let range_f = hi.fan_pct as i32 - lo.fan_pct as i32;
                let dt = (temp_c - lo.temp_c) as u32;
                return (lo.fan_pct as i32 + (range_f * dt as i32 / range_t as i32)) as u8;
            }
        }
        50
    }
}

// ---------------------------------------------------------------------------
// DirectX translation layer
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DxTranslation {
    None,
    Dx9ToVulkan,
    Dx11ToVulkan,
    Dx12ToVulkan,
    Dx11ToRaeGfx,
    Dx12ToRaeGfx,
}

// ---------------------------------------------------------------------------
// Per-game profile: Display
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct DisplayProfile {
    pub resolution: (u32, u32),
    pub refresh_rate: u32,
    pub hdr_mode: HdrMode,
    pub vrr_enabled: bool,
    pub vsync: VsyncMode,
    pub fullscreen_mode: FullscreenMode,
    pub scaling_filter: ScalingFilter,
    pub custom_dpi: Option<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScalingFilter {
    None,
    Bilinear,
    Fsr,
    NearestNeighbor,
}

impl DisplayProfile {
    pub fn default_1080p() -> Self {
        Self {
            resolution: (1920, 1080),
            refresh_rate: 60,
            hdr_mode: HdrMode::Auto,
            vrr_enabled: true,
            vsync: VsyncMode::Off,
            fullscreen_mode: FullscreenMode::ExclusiveFullscreen,
            scaling_filter: ScalingFilter::None,
            custom_dpi: None,
        }
    }

    pub fn default_1440p() -> Self {
        Self {
            resolution: (2560, 1440),
            refresh_rate: 144,
            hdr_mode: HdrMode::Auto,
            vrr_enabled: true,
            vsync: VsyncMode::Off,
            fullscreen_mode: FullscreenMode::ExclusiveFullscreen,
            scaling_filter: ScalingFilter::None,
            custom_dpi: None,
        }
    }

    pub fn default_4k() -> Self {
        Self {
            resolution: (3840, 2160),
            refresh_rate: 60,
            hdr_mode: HdrMode::ForceHdr10,
            vrr_enabled: true,
            vsync: VsyncMode::On,
            fullscreen_mode: FullscreenMode::ExclusiveFullscreen,
            scaling_filter: ScalingFilter::None,
            custom_dpi: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Per-game profile: GPU
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct GpuProfile {
    pub power_limit_percent: u8,
    pub fan_curve: Option<FanCurve>,
    pub shader_cache_enabled: bool,
    pub max_frame_rate: Option<u32>,
    pub force_low_latency: bool,
}

impl GpuProfile {
    pub fn default_profile() -> Self {
        Self {
            power_limit_percent: 100,
            fan_curve: None,
            shader_cache_enabled: true,
            max_frame_rate: None,
            force_low_latency: false,
        }
    }

    pub fn power_saver() -> Self {
        Self {
            power_limit_percent: 60,
            fan_curve: None,
            shader_cache_enabled: true,
            max_frame_rate: Some(30),
            force_low_latency: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Per-game profile: Audio
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct AudioProfile {
    pub output_device: Option<String>,
    pub volume_percent: u8,
    pub spatial_audio: SpatialAudioMode,
    pub voice_chat_device: Option<String>,
    pub voice_chat_ducking: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpatialAudioMode {
    Off,
    Stereo,
    Surround51,
    Surround71,
    Hrtf,
}

impl AudioProfile {
    pub fn default_profile() -> Self {
        Self {
            output_device: None,
            volume_percent: 100,
            spatial_audio: SpatialAudioMode::Stereo,
            voice_chat_device: None,
            voice_chat_ducking: true,
        }
    }
}

// ---------------------------------------------------------------------------
// Per-game profile: Input
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct InputProfile {
    pub controller_layout: ControllerLayout,
    pub mouse_sensitivity: u16,
    pub mouse_accel: bool,
    pub controller_vibration: bool,
    pub gyro_enabled: bool,
    pub adaptive_triggers: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControllerLayout {
    Default,
    Xbox,
    DualSense,
    SteamDeck,
    Custom,
}

impl InputProfile {
    pub fn default_profile() -> Self {
        Self {
            controller_layout: ControllerLayout::Default,
            mouse_sensitivity: 500,
            mouse_accel: false,
            controller_vibration: true,
            gyro_enabled: false,
            adaptive_triggers: true,
        }
    }
}

// ---------------------------------------------------------------------------
// Per-game profile: Scheduler (SCHED_BODY integration)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct SchedulerProfile {
    pub use_sched_game: bool,
    pub core_affinity: Option<u64>,
    pub null_latency: bool,
    pub background_throttle: bool,
    pub priority_boost: bool,
    pub render_thread_pinning: bool,
}

impl SchedulerProfile {
    pub fn default_profile() -> Self {
        Self {
            use_sched_game: true,
            core_affinity: None,
            null_latency: false,
            background_throttle: true,
            priority_boost: false,
            render_thread_pinning: false,
        }
    }

    pub fn competitive() -> Self {
        Self {
            use_sched_game: true,
            core_affinity: None,
            null_latency: true,
            background_throttle: true,
            priority_boost: true,
            render_thread_pinning: true,
        }
    }
}

// ---------------------------------------------------------------------------
// Per-game profile: Compatibility (AthBridge integration)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct CompatProfile {
    pub use_athbridge: bool,
    pub wine_prefix: Option<String>,
    pub directx_translation: DxTranslation,
    pub env_vars: Vec<(String, String)>,
    pub force_proton_version: Option<String>,
    pub esync: bool,
    pub fsync: bool,
    pub mangohud: bool,
    pub gamemode: bool,
}

impl CompatProfile {
    pub fn native() -> Self {
        Self {
            use_athbridge: false,
            wine_prefix: None,
            directx_translation: DxTranslation::None,
            env_vars: Vec::new(),
            force_proton_version: None,
            esync: false,
            fsync: false,
            mangohud: false,
            gamemode: false,
        }
    }

    pub fn windows_compat() -> Self {
        Self {
            use_athbridge: true,
            wine_prefix: None,
            directx_translation: DxTranslation::Dx11ToVulkan,
            env_vars: Vec::new(),
            force_proton_version: None,
            esync: true,
            fsync: true,
            mangohud: false,
            gamemode: true,
        }
    }
}

// ---------------------------------------------------------------------------
// Composite per-game profile
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct GameProfile {
    pub game_id: GameId,
    pub name: String,
    pub display: DisplayProfile,
    pub gpu: GpuProfile,
    pub audio: AudioProfile,
    pub input: InputProfile,
    pub scheduler: SchedulerProfile,
    pub compat: CompatProfile,
}

impl GameProfile {
    pub fn default_for(game_id: GameId) -> Self {
        Self {
            game_id,
            name: String::from("Default"),
            display: DisplayProfile::default_1080p(),
            gpu: GpuProfile::default_profile(),
            audio: AudioProfile::default_profile(),
            input: InputProfile::default_profile(),
            scheduler: SchedulerProfile::default_profile(),
            compat: CompatProfile::native(),
        }
    }

    pub fn competitive_for(game_id: GameId) -> Self {
        Self {
            game_id,
            name: String::from("Competitive"),
            display: DisplayProfile {
                resolution: (1920, 1080),
                refresh_rate: 240,
                hdr_mode: HdrMode::Off,
                vrr_enabled: false,
                vsync: VsyncMode::Off,
                fullscreen_mode: FullscreenMode::ExclusiveFullscreen,
                scaling_filter: ScalingFilter::None,
                custom_dpi: None,
            },
            gpu: GpuProfile {
                power_limit_percent: 100,
                fan_curve: None,
                shader_cache_enabled: true,
                max_frame_rate: None,
                force_low_latency: true,
            },
            audio: AudioProfile::default_profile(),
            input: InputProfile::default_profile(),
            scheduler: SchedulerProfile::competitive(),
            compat: CompatProfile::native(),
        }
    }
}

// ---------------------------------------------------------------------------
// Achievements
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct Achievement {
    pub id: String,
    pub name: String,
    pub description: String,
    pub unlocked: bool,
    pub unlock_time: Option<u64>,
    pub icon_hash: [u8; 32],
    pub rarity: u16,
}

impl Achievement {
    pub fn rarity_percent(&self) -> f32 {
        self.rarity as f32 / 100.0
    }

    pub fn is_rare(&self) -> bool {
        self.rarity < 1000
    }
}

// ---------------------------------------------------------------------------
// Game entry — a single game in the unified library
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct GameEntry {
    pub id: GameId,
    pub title: String,
    pub store: StoreType,
    pub store_app_id: String,
    pub install_path: Option<String>,
    pub install_size_bytes: u64,
    pub last_played: u64,
    pub total_playtime_secs: u64,
    pub cover_art_hash: [u8; 32],
    pub executable: Option<String>,
    pub launch_args: Vec<String>,
    pub state: GameState,
    pub tags: Vec<String>,
    pub achievements: Vec<Achievement>,
    pub version: Option<String>,
    pub developer: Option<String>,
    pub publisher: Option<String>,
    pub release_year: Option<u16>,
    pub rating: Option<u8>,
    pub notes: Option<String>,
}

impl GameEntry {
    pub fn new(id: u64, title: String, store: StoreType, store_app_id: String) -> Self {
        Self {
            id: GameId(id),
            title,
            store,
            store_app_id,
            install_path: None,
            install_size_bytes: 0,
            last_played: 0,
            total_playtime_secs: 0,
            cover_art_hash: [0u8; 32],
            executable: None,
            launch_args: Vec::new(),
            state: GameState::NotInstalled,
            tags: Vec::new(),
            achievements: Vec::new(),
            version: None,
            developer: None,
            publisher: None,
            release_year: None,
            rating: None,
            notes: None,
        }
    }

    pub fn achievement_progress(&self) -> (usize, usize) {
        let total = self.achievements.len();
        let unlocked = self.achievements.iter().filter(|a| a.unlocked).count();
        (unlocked, total)
    }

    pub fn playtime_hours(&self) -> u32 {
        (self.total_playtime_secs / 3600) as u32
    }
}

// ---------------------------------------------------------------------------
// Store connection — tracks credentials/paths for each store
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct StoreConnection {
    pub store: StoreType,
    pub enabled: bool,
    pub library_paths: Vec<String>,
    pub username: Option<String>,
    pub last_sync: u64,
    pub game_count: usize,
}

impl StoreConnection {
    pub fn new(store: StoreType) -> Self {
        Self {
            store,
            enabled: true,
            library_paths: Vec::new(),
            username: None,
            last_sync: 0,
            game_count: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StoreError {
    NotConnected,
    AppNotFound,
    AlreadyInstalled,
    NotInstalled,
    InsufficientDiskSpace,
    NetworkError,
    AuthenticationRequired,
    PermissionDenied,
    CorruptManifest,
    ParseError(String),
    LaunchFailed(String),
    UpdateFailed(String),
    SyncFailed(String),
    Unknown(String),
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StoreError::NotConnected => f.write_str("store not connected"),
            StoreError::AppNotFound => f.write_str("app not found"),
            StoreError::AlreadyInstalled => f.write_str("already installed"),
            StoreError::NotInstalled => f.write_str("not installed"),
            StoreError::InsufficientDiskSpace => f.write_str("insufficient disk space"),
            StoreError::NetworkError => f.write_str("network error"),
            StoreError::AuthenticationRequired => f.write_str("authentication required"),
            StoreError::PermissionDenied => f.write_str("permission denied"),
            StoreError::CorruptManifest => f.write_str("corrupt manifest"),
            StoreError::ParseError(msg) => write!(f, "parse error: {}", msg),
            StoreError::LaunchFailed(msg) => write!(f, "launch failed: {}", msg),
            StoreError::UpdateFailed(msg) => write!(f, "update failed: {}", msg),
            StoreError::SyncFailed(msg) => write!(f, "sync failed: {}", msg),
            StoreError::Unknown(msg) => write!(f, "unknown error: {}", msg),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LaunchError {
    GameNotFound,
    GameNotInstalled,
    GameAlreadyRunning,
    ProfileNotFound,
    ExecutableNotSet,
    SpawnFailed(String),
    PreHookFailed(String),
    CompatLayerFailed(String),
}

impl fmt::Display for LaunchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LaunchError::GameNotFound => f.write_str("game not found"),
            LaunchError::GameNotInstalled => f.write_str("game not installed"),
            LaunchError::GameAlreadyRunning => f.write_str("game already running"),
            LaunchError::ProfileNotFound => f.write_str("profile not found"),
            LaunchError::ExecutableNotSet => f.write_str("executable not set"),
            LaunchError::SpawnFailed(msg) => write!(f, "spawn failed: {}", msg),
            LaunchError::PreHookFailed(msg) => write!(f, "pre-hook failed: {}", msg),
            LaunchError::CompatLayerFailed(msg) => write!(f, "compat layer failed: {}", msg),
        }
    }
}

// ---------------------------------------------------------------------------
// Playtime session tracking
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct GameSession {
    pub game_id: GameId,
    pub start_time: u64,
    pub duration_secs: u64,
    pub avg_fps: Option<u32>,
    pub peak_fps: Option<u32>,
    pub min_fps: Option<u32>,
    pub crashes: u32,
}

#[derive(Clone, Debug)]
pub struct PlaytimeStats {
    pub total_sessions: u64,
    pub total_secs: u64,
    pub avg_session_secs: u64,
    pub longest_session_secs: u64,
    pub avg_fps_overall: Option<u32>,
    pub last_7_days_secs: u64,
    pub last_30_days_secs: u64,
    pub last_365_days_secs: u64,
}

// ---------------------------------------------------------------------------
// VDF parser — Valve Data Format (Steam appmanifest_*.acf files)
// ---------------------------------------------------------------------------

/// Key-value pair from a VDF section.
#[derive(Clone, Debug)]
pub struct VdfKeyValue {
    pub key: String,
    pub value: String,
}

/// A parsed VDF section (nested key-value dictionary).
#[derive(Clone, Debug)]
pub struct VdfSection {
    pub name: String,
    pub values: Vec<VdfKeyValue>,
    pub children: Vec<VdfSection>,
}

impl VdfSection {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.values
            .iter()
            .find(|kv| kv.key == key)
            .map(|kv| kv.value.as_str())
    }

    pub fn get_u64(&self, key: &str) -> Option<u64> {
        self.get(key).and_then(|v| parse_u64_decimal(v.as_bytes()))
    }

    pub fn child(&self, name: &str) -> Option<&VdfSection> {
        self.children.iter().find(|c| c.name == name)
    }
}

fn parse_u64_decimal(bytes: &[u8]) -> Option<u64> {
    if bytes.is_empty() {
        return None;
    }
    let mut result: u64 = 0;
    for &b in bytes {
        if b < b'0' || b > b'9' {
            return None;
        }
        result = result.checked_mul(10)?.checked_add((b - b'0') as u64)?;
    }
    Some(result)
}

/// Parse Valve Data Format from raw bytes.
///
/// VDF looks like:
/// ```text
/// "AppState"
/// {
///     "appid"    "440"
///     "name"     "Team Fortress 2"
///     "UserConfig"
///     {
///         "language"  "english"
///     }
/// }
/// ```
pub fn parse_vdf(data: &[u8]) -> Result<VdfSection, StoreError> {
    let mut pos = 0;
    let mut root_name = String::new();

    skip_whitespace(data, &mut pos);
    if pos < data.len() && data[pos] == b'"' {
        root_name = read_quoted_string(data, &mut pos)?;
    }

    skip_whitespace(data, &mut pos);
    if pos >= data.len() || data[pos] != b'{' {
        return Err(StoreError::ParseError(String::from("expected '{'")));
    }
    pos += 1;

    parse_vdf_section(data, &mut pos, root_name)
}

fn parse_vdf_section(data: &[u8], pos: &mut usize, name: String) -> Result<VdfSection, StoreError> {
    let mut section = VdfSection {
        name,
        values: Vec::new(),
        children: Vec::new(),
    };

    loop {
        skip_whitespace(data, pos);
        if *pos >= data.len() {
            break;
        }
        if data[*pos] == b'}' {
            *pos += 1;
            break;
        }
        if data[*pos] == b'"' {
            let key = read_quoted_string(data, pos)?;
            skip_whitespace(data, pos);

            if *pos < data.len() && data[*pos] == b'{' {
                *pos += 1;
                let child = parse_vdf_section(data, pos, key)?;
                section.children.push(child);
            } else if *pos < data.len() && data[*pos] == b'"' {
                let value = read_quoted_string(data, pos)?;
                section.values.push(VdfKeyValue { key, value });
            } else {
                return Err(StoreError::ParseError(String::from(
                    "expected value or section after key",
                )));
            }
        } else {
            *pos += 1;
        }
    }

    Ok(section)
}

fn skip_whitespace(data: &[u8], pos: &mut usize) {
    while *pos < data.len() {
        match data[*pos] {
            b' ' | b'\t' | b'\n' | b'\r' => *pos += 1,
            b'/' if *pos + 1 < data.len() && data[*pos + 1] == b'/' => {
                while *pos < data.len() && data[*pos] != b'\n' {
                    *pos += 1;
                }
            }
            _ => break,
        }
    }
}

fn read_quoted_string(data: &[u8], pos: &mut usize) -> Result<String, StoreError> {
    if *pos >= data.len() || data[*pos] != b'"' {
        return Err(StoreError::ParseError(String::from("expected '\"'")));
    }
    *pos += 1;
    let start = *pos;

    while *pos < data.len() && data[*pos] != b'"' {
        if data[*pos] == b'\\' && *pos + 1 < data.len() {
            *pos += 2;
        } else {
            *pos += 1;
        }
    }

    let end = *pos;
    if *pos < data.len() {
        *pos += 1; // skip closing quote
    }

    let slice = &data[start..end];
    let mut s = String::with_capacity(slice.len());
    let mut i = 0;
    while i < slice.len() {
        if slice[i] == b'\\' && i + 1 < slice.len() {
            match slice[i + 1] {
                b'n' => s.push('\n'),
                b't' => s.push('\t'),
                b'\\' => s.push('\\'),
                b'"' => s.push('"'),
                other => {
                    s.push('\\');
                    s.push(other as char);
                }
            }
            i += 2;
        } else {
            s.push(slice[i] as char);
            i += 1;
        }
    }
    Ok(s)
}

// ---------------------------------------------------------------------------
// Epic manifest parser — simplified JSON-like item manifest
// ---------------------------------------------------------------------------

/// Parsed fields from an Epic Games `.item` manifest (JSON subset).
#[derive(Clone, Debug)]
pub struct EpicManifest {
    pub display_name: String,
    pub install_location: String,
    pub launch_executable: String,
    pub app_name: String,
    pub app_version: String,
    pub install_size: u64,
    pub is_managed: bool,
}

/// Minimal JSON value parser for Epic manifests — `no_std` compatible.
///
/// Only handles string and number values at the top level of a flat JSON
/// object (no nesting). This is sufficient for the `.item` format.
pub fn parse_epic_manifest(data: &[u8]) -> Result<EpicManifest, StoreError> {
    let kvs = parse_flat_json(data)?;

    let get = |key: &str| -> Result<String, StoreError> {
        kvs.iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.clone())
            .ok_or_else(|| StoreError::ParseError(String::from(key)))
    };

    let install_size = kvs
        .iter()
        .find(|(k, _)| k == "InstallSize")
        .and_then(|(_, v)| parse_u64_decimal(v.as_bytes()))
        .unwrap_or(0);

    let is_managed = kvs
        .iter()
        .find(|(k, _)| k == "bIsManaged")
        .map(|(_, v)| v == "true")
        .unwrap_or(false);

    Ok(EpicManifest {
        display_name: get("DisplayName").unwrap_or_default(),
        install_location: get("InstallLocation").unwrap_or_default(),
        launch_executable: get("LaunchExecutable").unwrap_or_default(),
        app_name: get("AppName").unwrap_or_default(),
        app_version: get("AppVersionString").unwrap_or_default(),
        install_size,
        is_managed,
    })
}

fn parse_flat_json(data: &[u8]) -> Result<Vec<(String, String)>, StoreError> {
    let mut pos: usize = 0;
    let mut kvs = Vec::new();

    skip_ws(data, &mut pos);
    if pos >= data.len() || data[pos] != b'{' {
        return Err(StoreError::ParseError(String::from("expected '{'")));
    }
    pos += 1;

    loop {
        skip_ws(data, &mut pos);
        if pos >= data.len() {
            break;
        }
        if data[pos] == b'}' {
            break;
        }
        if data[pos] == b',' {
            pos += 1;
            continue;
        }

        if data[pos] != b'"' {
            pos += 1;
            continue;
        }

        let key = read_json_string(data, &mut pos)?;
        skip_ws(data, &mut pos);

        if pos >= data.len() || data[pos] != b':' {
            return Err(StoreError::ParseError(String::from("expected ':'")));
        }
        pos += 1;
        skip_ws(data, &mut pos);

        if pos >= data.len() {
            break;
        }

        let value = if data[pos] == b'"' {
            read_json_string(data, &mut pos)?
        } else if data[pos] == b'{' || data[pos] == b'[' {
            skip_nested(data, &mut pos);
            String::new()
        } else {
            read_json_literal(data, &mut pos)
        };

        kvs.push((key, value));
    }

    Ok(kvs)
}

fn skip_ws(data: &[u8], pos: &mut usize) {
    while *pos < data.len() && matches!(data[*pos], b' ' | b'\t' | b'\n' | b'\r') {
        *pos += 1;
    }
}

fn read_json_string(data: &[u8], pos: &mut usize) -> Result<String, StoreError> {
    if *pos >= data.len() || data[*pos] != b'"' {
        return Err(StoreError::ParseError(String::from("expected '\"'")));
    }
    *pos += 1;
    let mut s = String::new();
    while *pos < data.len() && data[*pos] != b'"' {
        if data[*pos] == b'\\' && *pos + 1 < data.len() {
            match data[*pos + 1] {
                b'"' => s.push('"'),
                b'\\' => s.push('\\'),
                b'/' => s.push('/'),
                b'n' => s.push('\n'),
                b't' => s.push('\t'),
                b'r' => s.push('\r'),
                other => {
                    s.push('\\');
                    s.push(other as char);
                }
            }
            *pos += 2;
        } else {
            s.push(data[*pos] as char);
            *pos += 1;
        }
    }
    if *pos < data.len() {
        *pos += 1;
    }
    Ok(s)
}

fn read_json_literal(data: &[u8], pos: &mut usize) -> String {
    let start = *pos;
    while *pos < data.len()
        && !matches!(
            data[*pos],
            b',' | b'}' | b']' | b' ' | b'\n' | b'\r' | b'\t'
        )
    {
        *pos += 1;
    }
    let slice = &data[start..*pos];
    let mut s = String::with_capacity(slice.len());
    for &b in slice {
        s.push(b as char);
    }
    s
}

fn skip_nested(data: &[u8], pos: &mut usize) {
    let open = data[*pos];
    let close = if open == b'{' { b'}' } else { b']' };
    let mut depth = 1u32;
    *pos += 1;
    while *pos < data.len() && depth > 0 {
        if data[*pos] == open {
            depth += 1;
        } else if data[*pos] == close {
            depth -= 1;
        } else if data[*pos] == b'"' {
            *pos += 1;
            while *pos < data.len() && data[*pos] != b'"' {
                if data[*pos] == b'\\' {
                    *pos += 1;
                }
                *pos += 1;
            }
        }
        *pos += 1;
    }
}

// ---------------------------------------------------------------------------
// Store connector trait
// ---------------------------------------------------------------------------

pub trait StoreConnector {
    fn name(&self) -> &str;
    fn store_type(&self) -> StoreType;
    fn scan_library(&mut self) -> Vec<GameEntry>;
    fn install_game(&mut self, app_id: &str) -> Result<(), StoreError>;
    fn uninstall_game(&mut self, app_id: &str) -> Result<(), StoreError>;
    fn update_game(&mut self, app_id: &str) -> Result<(), StoreError>;
    fn launch_game(&mut self, app_id: &str, profile: &GameProfile) -> Result<u64, StoreError>;
    fn sync_saves(&mut self, app_id: &str) -> Result<(), StoreError>;
    fn check_updates(&mut self) -> Vec<(String, String)>;
    fn is_connected(&self) -> bool;
}

// ---------------------------------------------------------------------------
// Steam connector
// ---------------------------------------------------------------------------

pub struct SteamConnector {
    library_paths: Vec<String>,
    manifests: Vec<(String, VdfSection)>,
    games: Vec<GameEntry>,
    connected: bool,
    next_id: u64,
}

impl SteamConnector {
    pub fn new() -> Self {
        Self {
            library_paths: Vec::new(),
            manifests: Vec::new(),
            games: Vec::new(),
            connected: false,
            next_id: 1_000_000,
        }
    }

    pub fn add_library_path(&mut self, path: String) {
        self.library_paths.push(path);
    }

    /// Feed raw `appmanifest_*.acf` data. Call once per manifest file found
    /// in each Steam library folder.
    pub fn add_manifest_data(&mut self, filename: &str, data: &[u8]) -> Result<(), StoreError> {
        let section = parse_vdf(data)?;
        self.manifests.push((String::from(filename), section));
        Ok(())
    }

    fn manifest_to_entry(&mut self, section: &VdfSection) -> Option<GameEntry> {
        let app_id_str = section.get("appid")?;
        let name = section.get("name")?;
        let install_dir = section.get("installdir");
        let size = section.get_u64("SizeOnDisk").unwrap_or(0);
        let last_updated = section.get_u64("LastUpdated").unwrap_or(0);
        let state_flags = section.get_u64("StateFlags").unwrap_or(0);

        let id = self.next_id;
        self.next_id += 1;

        let game_state = match state_flags {
            4 => GameState::Installed,
            6 => GameState::UpdateAvailable,
            1026 | 1030 => GameState::Updating { progress_pct: 0 },
            _ => GameState::Installed,
        };

        let install_path = install_dir.map(|d| {
            let mut path = String::new();
            if let Some(first_lib) = self.library_paths.first() {
                path.push_str(first_lib.as_str());
                path.push_str("/steamapps/common/");
            }
            path.push_str(d);
            path
        });

        let mut entry = GameEntry::new(
            id,
            String::from(name),
            StoreType::Steam,
            String::from(app_id_str),
        );
        entry.install_path = install_path;
        entry.install_size_bytes = size;
        entry.last_played = last_updated;
        entry.state = game_state;

        if let Some(uc) = section.child("UserConfig") {
            if let Some(launch) = uc.get("LaunchOptions") {
                for arg in launch.split_ascii_whitespace() {
                    entry.launch_args.push(String::from(arg));
                }
            }
        }

        Some(entry)
    }
}

impl StoreConnector for SteamConnector {
    fn name(&self) -> &str {
        "Steam"
    }

    fn store_type(&self) -> StoreType {
        StoreType::Steam
    }

    fn scan_library(&mut self) -> Vec<GameEntry> {
        self.connected = true;
        self.games.clear();

        let manifests: Vec<VdfSection> = self.manifests.iter().map(|(_, s)| s.clone()).collect();
        for section in &manifests {
            if let Some(entry) = self.manifest_to_entry(section) {
                self.games.push(entry);
            }
        }

        self.games.clone()
    }

    fn install_game(&mut self, _app_id: &str) -> Result<(), StoreError> {
        Err(StoreError::Unknown(String::from(
            "Steam install requires Steam client IPC",
        )))
    }

    fn uninstall_game(&mut self, app_id: &str) -> Result<(), StoreError> {
        if let Some(g) = self.games.iter_mut().find(|g| g.store_app_id == app_id) {
            g.state = GameState::NotInstalled;
            g.install_path = None;
            Ok(())
        } else {
            Err(StoreError::AppNotFound)
        }
    }

    fn update_game(&mut self, _app_id: &str) -> Result<(), StoreError> {
        Err(StoreError::Unknown(String::from(
            "Steam update requires Steam client IPC",
        )))
    }

    fn launch_game(&mut self, app_id: &str, _profile: &GameProfile) -> Result<u64, StoreError> {
        if let Some(g) = self.games.iter_mut().find(|g| g.store_app_id == app_id) {
            if !g.state.is_playable() {
                return Err(StoreError::LaunchFailed(String::from(
                    "game not in playable state",
                )));
            }
            let pid = 0u64;
            g.state = GameState::Running { pid };
            Ok(pid)
        } else {
            Err(StoreError::AppNotFound)
        }
    }

    fn sync_saves(&mut self, _app_id: &str) -> Result<(), StoreError> {
        Ok(())
    }

    fn check_updates(&mut self) -> Vec<(String, String)> {
        self.games
            .iter()
            .filter_map(|g| {
                if matches!(g.state, GameState::UpdateAvailable) {
                    Some((g.store_app_id.clone(), String::from("update available")))
                } else {
                    None
                }
            })
            .collect()
    }

    fn is_connected(&self) -> bool {
        self.connected
    }
}

// ---------------------------------------------------------------------------
// Epic Games connector
// ---------------------------------------------------------------------------

pub struct EpicConnector {
    manifest_dir: Option<String>,
    raw_manifests: Vec<EpicManifest>,
    games: Vec<GameEntry>,
    connected: bool,
    next_id: u64,
}

impl EpicConnector {
    pub fn new() -> Self {
        Self {
            manifest_dir: None,
            raw_manifests: Vec::new(),
            games: Vec::new(),
            connected: false,
            next_id: 2_000_000,
        }
    }

    pub fn set_manifest_dir(&mut self, path: String) {
        self.manifest_dir = Some(path);
    }

    /// Feed raw `.item` manifest JSON from the Epic manifests directory.
    pub fn add_manifest_data(&mut self, data: &[u8]) -> Result<(), StoreError> {
        let manifest = parse_epic_manifest(data)?;
        self.raw_manifests.push(manifest);
        Ok(())
    }
}

impl StoreConnector for EpicConnector {
    fn name(&self) -> &str {
        "Epic Games"
    }

    fn store_type(&self) -> StoreType {
        StoreType::Epic
    }

    fn scan_library(&mut self) -> Vec<GameEntry> {
        self.connected = true;
        self.games.clear();

        for manifest in &self.raw_manifests {
            let id = self.next_id;
            self.next_id += 1;

            let mut entry = GameEntry::new(
                id,
                manifest.display_name.clone(),
                StoreType::Epic,
                manifest.app_name.clone(),
            );

            if !manifest.install_location.is_empty() {
                entry.install_path = Some(manifest.install_location.clone());
                entry.state = GameState::Installed;
            }

            if !manifest.launch_executable.is_empty() {
                entry.executable = Some(manifest.launch_executable.clone());
            }

            entry.install_size_bytes = manifest.install_size;
            entry.version = if manifest.app_version.is_empty() {
                None
            } else {
                Some(manifest.app_version.clone())
            };

            self.games.push(entry);
        }

        self.games.clone()
    }

    fn install_game(&mut self, _app_id: &str) -> Result<(), StoreError> {
        Err(StoreError::Unknown(String::from(
            "Epic install requires Epic Launcher IPC",
        )))
    }

    fn uninstall_game(&mut self, app_id: &str) -> Result<(), StoreError> {
        if let Some(g) = self.games.iter_mut().find(|g| g.store_app_id == app_id) {
            g.state = GameState::NotInstalled;
            g.install_path = None;
            Ok(())
        } else {
            Err(StoreError::AppNotFound)
        }
    }

    fn update_game(&mut self, _app_id: &str) -> Result<(), StoreError> {
        Err(StoreError::Unknown(String::from(
            "Epic update requires Epic Launcher IPC",
        )))
    }

    fn launch_game(&mut self, app_id: &str, _profile: &GameProfile) -> Result<u64, StoreError> {
        let game = self
            .games
            .iter_mut()
            .find(|g| g.store_app_id == app_id)
            .ok_or(StoreError::AppNotFound)?;

        if !game.state.is_playable() {
            return Err(StoreError::LaunchFailed(String::from(
                "game not in playable state",
            )));
        }

        if game.executable.is_none() {
            return Err(StoreError::LaunchFailed(String::from("no executable set")));
        }

        let pid = 0;
        game.state = GameState::Running { pid };
        Ok(pid)
    }

    fn sync_saves(&mut self, _app_id: &str) -> Result<(), StoreError> {
        Ok(())
    }

    fn check_updates(&mut self) -> Vec<(String, String)> {
        Vec::new()
    }

    fn is_connected(&self) -> bool {
        self.connected
    }
}

// ---------------------------------------------------------------------------
// GOG connector
// ---------------------------------------------------------------------------

pub struct GogConnector {
    install_paths: Vec<String>,
    game_infos: Vec<GogGameInfo>,
    games: Vec<GameEntry>,
    connected: bool,
    next_id: u64,
}

#[derive(Clone, Debug)]
pub struct GogGameInfo {
    pub game_id: String,
    pub name: String,
    pub install_path: String,
    pub executable: String,
    pub version: String,
    pub size_bytes: u64,
}

impl GogConnector {
    pub fn new() -> Self {
        Self {
            install_paths: Vec::new(),
            game_infos: Vec::new(),
            games: Vec::new(),
            connected: false,
            next_id: 3_000_000,
        }
    }

    pub fn add_install_path(&mut self, path: String) {
        self.install_paths.push(path);
    }

    /// Register a GOG game discovered by scanning the GOG Galaxy database
    /// or install directories.
    pub fn add_game_info(&mut self, info: GogGameInfo) {
        self.game_infos.push(info);
    }

    /// Parse a GOG `gameinfo` file (simple line-based format).
    /// Lines: name, game_id, version, language.
    pub fn parse_gameinfo(data: &[u8]) -> Option<(String, String, String)> {
        let mut lines = Vec::new();
        let mut start = 0;
        for i in 0..data.len() {
            if data[i] == b'\n' {
                let end = if i > 0 && data[i - 1] == b'\r' {
                    i - 1
                } else {
                    i
                };
                let mut s = String::new();
                for &b in &data[start..end] {
                    s.push(b as char);
                }
                lines.push(s);
                start = i + 1;
            }
        }
        if start < data.len() {
            let mut s = String::new();
            for &b in &data[start..] {
                s.push(b as char);
            }
            lines.push(s);
        }

        if lines.len() >= 3 {
            Some((lines[0].clone(), lines[1].clone(), lines[2].clone()))
        } else {
            None
        }
    }
}

impl StoreConnector for GogConnector {
    fn name(&self) -> &str {
        "GOG"
    }

    fn store_type(&self) -> StoreType {
        StoreType::Gog
    }

    fn scan_library(&mut self) -> Vec<GameEntry> {
        self.connected = true;
        self.games.clear();

        for info in &self.game_infos {
            let id = self.next_id;
            self.next_id += 1;

            let mut entry =
                GameEntry::new(id, info.name.clone(), StoreType::Gog, info.game_id.clone());
            entry.install_path = Some(info.install_path.clone());
            entry.executable = Some(info.executable.clone());
            entry.install_size_bytes = info.size_bytes;
            entry.state = GameState::Installed;
            entry.version = if info.version.is_empty() {
                None
            } else {
                Some(info.version.clone())
            };

            self.games.push(entry);
        }

        self.games.clone()
    }

    fn install_game(&mut self, _app_id: &str) -> Result<(), StoreError> {
        Err(StoreError::Unknown(String::from(
            "GOG install requires GOG Galaxy or manual installer",
        )))
    }

    fn uninstall_game(&mut self, app_id: &str) -> Result<(), StoreError> {
        if let Some(g) = self.games.iter_mut().find(|g| g.store_app_id == app_id) {
            g.state = GameState::NotInstalled;
            g.install_path = None;
            Ok(())
        } else {
            Err(StoreError::AppNotFound)
        }
    }

    fn update_game(&mut self, _app_id: &str) -> Result<(), StoreError> {
        Err(StoreError::Unknown(String::from(
            "GOG update requires GOG Galaxy",
        )))
    }

    fn launch_game(&mut self, app_id: &str, _profile: &GameProfile) -> Result<u64, StoreError> {
        let game = self
            .games
            .iter_mut()
            .find(|g| g.store_app_id == app_id)
            .ok_or(StoreError::AppNotFound)?;

        if !game.state.is_playable() {
            return Err(StoreError::LaunchFailed(String::from("not playable")));
        }

        if game.executable.is_none() {
            return Err(StoreError::LaunchFailed(String::from("no executable")));
        }

        let pid = 0;
        game.state = GameState::Running { pid };
        Ok(pid)
    }

    fn sync_saves(&mut self, _app_id: &str) -> Result<(), StoreError> {
        Ok(())
    }

    fn check_updates(&mut self) -> Vec<(String, String)> {
        Vec::new()
    }

    fn is_connected(&self) -> bool {
        self.connected
    }
}

// ---------------------------------------------------------------------------
// AthStore connector — native AthenaOS store
// ---------------------------------------------------------------------------

pub struct AthStoreConnector {
    games: Vec<GameEntry>,
    connected: bool,
    next_id: u64,
    api_endpoint: Option<String>,
}

/// A package listing from AthStore's catalog API.
#[derive(Clone, Debug)]
pub struct AthStorePackage {
    pub package_id: String,
    pub name: String,
    pub version: String,
    pub size_bytes: u64,
    pub developer: String,
    pub description: String,
    pub executable: String,
    pub tags: Vec<String>,
}

impl AthStoreConnector {
    pub fn new() -> Self {
        Self {
            games: Vec::new(),
            connected: false,
            next_id: 4_000_000,
            api_endpoint: None,
        }
    }

    pub fn set_api_endpoint(&mut self, endpoint: String) {
        self.api_endpoint = Some(endpoint);
    }

    /// Register a game from the AthStore catalog.
    pub fn add_package(
        &mut self,
        pkg: AthStorePackage,
        installed: bool,
        install_path: Option<String>,
    ) {
        let id = self.next_id;
        self.next_id += 1;

        let mut entry = GameEntry::new(id, pkg.name, StoreType::AthStore, pkg.package_id);
        entry.executable = Some(pkg.executable);
        entry.install_size_bytes = pkg.size_bytes;
        entry.developer = Some(pkg.developer);
        entry.version = Some(pkg.version);
        entry.tags = pkg.tags;

        if installed {
            entry.state = GameState::Installed;
            entry.install_path = install_path;
        }

        self.games.push(entry);
    }
}

impl StoreConnector for AthStoreConnector {
    fn name(&self) -> &str {
        "AthStore"
    }

    fn store_type(&self) -> StoreType {
        StoreType::AthStore
    }

    fn scan_library(&mut self) -> Vec<GameEntry> {
        self.connected = true;
        self.games.clone()
    }

    fn install_game(&mut self, app_id: &str) -> Result<(), StoreError> {
        if let Some(g) = self.games.iter_mut().find(|g| g.store_app_id == app_id) {
            if matches!(g.state, GameState::Installed) {
                return Err(StoreError::AlreadyInstalled);
            }
            g.state = GameState::Installing { progress_pct: 0 };
            Ok(())
        } else {
            Err(StoreError::AppNotFound)
        }
    }

    fn uninstall_game(&mut self, app_id: &str) -> Result<(), StoreError> {
        if let Some(g) = self.games.iter_mut().find(|g| g.store_app_id == app_id) {
            g.state = GameState::NotInstalled;
            g.install_path = None;
            Ok(())
        } else {
            Err(StoreError::AppNotFound)
        }
    }

    fn update_game(&mut self, app_id: &str) -> Result<(), StoreError> {
        if let Some(g) = self.games.iter_mut().find(|g| g.store_app_id == app_id) {
            if !matches!(g.state, GameState::UpdateAvailable) {
                return Err(StoreError::Unknown(String::from("no update available")));
            }
            g.state = GameState::Updating { progress_pct: 0 };
            Ok(())
        } else {
            Err(StoreError::AppNotFound)
        }
    }

    fn launch_game(&mut self, app_id: &str, _profile: &GameProfile) -> Result<u64, StoreError> {
        let game = self
            .games
            .iter_mut()
            .find(|g| g.store_app_id == app_id)
            .ok_or(StoreError::AppNotFound)?;

        if !game.state.is_playable() {
            return Err(StoreError::LaunchFailed(String::from("not playable")));
        }

        let pid = 0;
        game.state = GameState::Running { pid };
        Ok(pid)
    }

    fn sync_saves(&mut self, _app_id: &str) -> Result<(), StoreError> {
        Ok(())
    }

    fn check_updates(&mut self) -> Vec<(String, String)> {
        self.games
            .iter()
            .filter_map(|g| {
                if matches!(g.state, GameState::UpdateAvailable) {
                    Some((g.store_app_id.clone(), String::from("update available")))
                } else {
                    None
                }
            })
            .collect()
    }

    fn is_connected(&self) -> bool {
        self.connected
    }
}

// ---------------------------------------------------------------------------
// Manual connector — user-added games with custom launch configs
// ---------------------------------------------------------------------------

pub struct ManualConnector {
    games: Vec<GameEntry>,
    next_id: u64,
}

/// Configuration for a manually-added game.
#[derive(Clone, Debug)]
pub struct ManualGameConfig {
    pub name: String,
    pub executable: String,
    pub install_path: String,
    pub args: Vec<String>,
    pub env_vars: Vec<(String, String)>,
    pub tags: Vec<String>,
}

impl ManualConnector {
    pub fn new() -> Self {
        Self {
            games: Vec::new(),
            next_id: 5_000_000,
        }
    }

    pub fn add_game(&mut self, config: ManualGameConfig) -> GameId {
        let id = self.next_id;
        self.next_id += 1;

        let mut entry = GameEntry::new(id, config.name, StoreType::Manual, String::new());
        entry.executable = Some(config.executable);
        entry.install_path = Some(config.install_path);
        entry.launch_args = config.args;
        entry.tags = config.tags;
        entry.state = GameState::Installed;
        entry.store_app_id = alloc::format!("manual_{}", id);

        let game_id = entry.id;
        self.games.push(entry);
        game_id
    }

    pub fn remove_game(&mut self, id: GameId) -> bool {
        let before = self.games.len();
        self.games.retain(|g| g.id != id);
        self.games.len() != before
    }
}

impl StoreConnector for ManualConnector {
    fn name(&self) -> &str {
        "Manual"
    }

    fn store_type(&self) -> StoreType {
        StoreType::Manual
    }

    fn scan_library(&mut self) -> Vec<GameEntry> {
        self.games.clone()
    }

    fn install_game(&mut self, _app_id: &str) -> Result<(), StoreError> {
        Err(StoreError::Unknown(String::from(
            "manual games are managed by the user",
        )))
    }

    fn uninstall_game(&mut self, app_id: &str) -> Result<(), StoreError> {
        let before = self.games.len();
        self.games.retain(|g| g.store_app_id != app_id);
        if self.games.len() != before {
            Ok(())
        } else {
            Err(StoreError::AppNotFound)
        }
    }

    fn update_game(&mut self, _app_id: &str) -> Result<(), StoreError> {
        Err(StoreError::Unknown(String::from(
            "manual games don't have updates",
        )))
    }

    fn launch_game(&mut self, app_id: &str, _profile: &GameProfile) -> Result<u64, StoreError> {
        let game = self
            .games
            .iter_mut()
            .find(|g| g.store_app_id == app_id)
            .ok_or(StoreError::AppNotFound)?;

        if game.executable.is_none() {
            return Err(StoreError::LaunchFailed(String::from("no executable")));
        }

        let pid = 0;
        game.state = GameState::Running { pid };
        Ok(pid)
    }

    fn sync_saves(&mut self, _app_id: &str) -> Result<(), StoreError> {
        Err(StoreError::Unknown(String::from(
            "no cloud saves for manual games",
        )))
    }

    fn check_updates(&mut self) -> Vec<(String, String)> {
        Vec::new()
    }

    fn is_connected(&self) -> bool {
        true
    }
}

// ---------------------------------------------------------------------------
// Launch manager — orchestrates game launches with profile application
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct LaunchRequest {
    pub game_id: GameId,
    pub profile: GameProfile,
    pub environment_vars: Vec<(String, String)>,
    pub pre_launch_script: Option<String>,
    pub post_exit_script: Option<String>,
}

#[derive(Clone, Debug)]
pub struct RunningGame {
    pub game_id: GameId,
    pub pid: u64,
    pub start_time: u64,
    pub profile_applied: GameProfile,
    pub previous_display: Option<DisplayProfile>,
}

#[derive(Clone, Debug)]
pub struct ExitReport {
    pub game_id: GameId,
    pub pid: u64,
    pub exit_code: i32,
    pub session_duration_secs: u64,
    pub crashed: bool,
    pub avg_fps: Option<u32>,
}

/// Callbacks the OS layer provides to `LaunchManager` for privileged operations.
pub trait LaunchCallbacks {
    fn set_display_mode(&mut self, profile: &DisplayProfile) -> Result<(), LaunchError>;
    fn restore_display_mode(&mut self, previous: &DisplayProfile) -> Result<(), LaunchError>;
    fn activate_sched_game(&mut self, pid: u64) -> Result<(), LaunchError>;
    fn deactivate_sched_game(&mut self, pid: u64) -> Result<(), LaunchError>;
    fn apply_gpu_profile(&mut self, profile: &GpuProfile) -> Result<(), LaunchError>;
    fn restore_gpu_profile(&mut self) -> Result<(), LaunchError>;
    fn spawn_process(
        &mut self,
        executable: &str,
        args: &[String],
        env: &[(String, String)],
        cwd: Option<&str>,
    ) -> Result<u64, LaunchError>;
    fn start_overlay(&mut self, pid: u64) -> Result<(), LaunchError>;
    fn stop_overlay(&mut self, pid: u64) -> Result<(), LaunchError>;
    fn get_current_display(&self) -> DisplayProfile;
    fn current_timestamp(&self) -> u64;
}

pub struct LaunchManager {
    running: Vec<RunningGame>,
    exit_history: Vec<ExitReport>,
    max_history: usize,
}

impl LaunchManager {
    pub fn new() -> Self {
        Self {
            running: Vec::new(),
            exit_history: Vec::new(),
            max_history: 1000,
        }
    }

    pub fn running_games(&self) -> &[RunningGame] {
        &self.running
    }

    pub fn is_running(&self, id: GameId) -> bool {
        self.running.iter().any(|r| r.game_id == id)
    }

    pub fn exit_history(&self) -> &[ExitReport] {
        &self.exit_history
    }

    /// Full launch sequence:
    /// 1. Save current display state
    /// 2. Apply display profile
    /// 3. Apply GPU profile
    /// 4. Activate SCHED_BODY if requested
    /// 5. Spawn process
    /// 6. Start overlay
    pub fn launch(
        &mut self,
        entry: &GameEntry,
        request: &LaunchRequest,
        cb: &mut dyn LaunchCallbacks,
    ) -> Result<u64, LaunchError> {
        if self.is_running(entry.id) {
            return Err(LaunchError::GameAlreadyRunning);
        }

        if !entry.state.is_playable() {
            return Err(LaunchError::GameNotInstalled);
        }

        let executable = entry
            .executable
            .as_deref()
            .ok_or(LaunchError::ExecutableNotSet)?;

        let previous_display = cb.get_current_display();

        cb.set_display_mode(&request.profile.display)?;
        cb.apply_gpu_profile(&request.profile.gpu)?;

        let mut env = request.environment_vars.clone();
        for (k, v) in &request.profile.compat.env_vars {
            env.push((k.clone(), v.clone()));
        }

        if request.profile.scheduler.null_latency {
            env.push((String::from("ATHENAOS_NULL_LATENCY"), String::from("1")));
        }

        if request.profile.compat.mangohud {
            env.push((String::from("MANGOHUD"), String::from("1")));
        }

        let cwd = entry.install_path.as_deref();
        let pid = cb.spawn_process(executable, &entry.launch_args, &env, cwd)?;

        if request.profile.scheduler.use_sched_game {
            let _ = cb.activate_sched_game(pid);
        }

        let _ = cb.start_overlay(pid);

        let now = cb.current_timestamp();
        self.running.push(RunningGame {
            game_id: entry.id,
            pid,
            start_time: now,
            profile_applied: request.profile.clone(),
            previous_display: Some(previous_display),
        });

        Ok(pid)
    }

    /// Called when the OS detects that a game process has exited.
    /// Restores display/GPU/scheduler state and records the session.
    pub fn on_game_exit(
        &mut self,
        pid: u64,
        exit_code: i32,
        cb: &mut dyn LaunchCallbacks,
    ) -> Option<ExitReport> {
        let idx = self.running.iter().position(|r| r.pid == pid)?;
        let running = self.running.remove(idx);

        let _ = cb.stop_overlay(pid);

        if running.profile_applied.scheduler.use_sched_game {
            let _ = cb.deactivate_sched_game(pid);
        }

        let _ = cb.restore_gpu_profile();

        if let Some(prev) = &running.previous_display {
            let _ = cb.restore_display_mode(prev);
        }

        let now = cb.current_timestamp();
        let duration = now.saturating_sub(running.start_time);
        let crashed = exit_code != 0;

        let report = ExitReport {
            game_id: running.game_id,
            pid,
            exit_code,
            session_duration_secs: duration,
            crashed,
            avg_fps: None,
        };

        if self.exit_history.len() >= self.max_history {
            self.exit_history.remove(0);
        }
        self.exit_history.push(report.clone());

        Some(report)
    }

    pub fn crash_count_for(&self, id: GameId) -> usize {
        self.exit_history
            .iter()
            .filter(|r| r.game_id == id && r.crashed)
            .count()
    }
}

// ---------------------------------------------------------------------------
// Playtime tracker
// ---------------------------------------------------------------------------

pub struct PlaytimeTracker {
    sessions: BTreeMap<GameId, Vec<GameSession>>,
}

impl PlaytimeTracker {
    pub fn new() -> Self {
        Self {
            sessions: BTreeMap::new(),
        }
    }

    pub fn record_session(&mut self, session: GameSession) {
        self.sessions
            .entry(session.game_id)
            .or_insert_with(Vec::new)
            .push(session);
    }

    pub fn record_from_exit(&mut self, report: &ExitReport, start_time: u64) {
        let session = GameSession {
            game_id: report.game_id,
            start_time,
            duration_secs: report.session_duration_secs,
            avg_fps: report.avg_fps,
            peak_fps: None,
            min_fps: None,
            crashes: if report.crashed { 1 } else { 0 },
        };
        self.record_session(session);
    }

    pub fn sessions_for(&self, id: GameId) -> &[GameSession] {
        self.sessions.get(&id).map(|v| v.as_slice()).unwrap_or(&[])
    }

    pub fn total_playtime(&self, id: GameId) -> u64 {
        self.sessions_for(id).iter().map(|s| s.duration_secs).sum()
    }

    pub fn session_count(&self, id: GameId) -> usize {
        self.sessions_for(id).len()
    }

    pub fn stats_for(&self, id: GameId, now: u64) -> PlaytimeStats {
        let sessions = self.sessions_for(id);
        let total_secs: u64 = sessions.iter().map(|s| s.duration_secs).sum();
        let total_sessions = sessions.len() as u64;
        let avg_session = if total_sessions > 0 {
            total_secs / total_sessions
        } else {
            0
        };
        let longest = sessions.iter().map(|s| s.duration_secs).max().unwrap_or(0);

        let fps_sessions: Vec<u32> = sessions.iter().filter_map(|s| s.avg_fps).collect();
        let avg_fps = if fps_sessions.is_empty() {
            None
        } else {
            let sum: u64 = fps_sessions.iter().map(|&f| f as u64).sum();
            Some((sum / fps_sessions.len() as u64) as u32)
        };

        let secs_7d = 7 * 24 * 3600;
        let secs_30d = 30 * 24 * 3600;
        let secs_365d = 365 * 24 * 3600;

        let last_7 = sessions
            .iter()
            .filter(|s| now.saturating_sub(s.start_time) < secs_7d)
            .map(|s| s.duration_secs)
            .sum();
        let last_30 = sessions
            .iter()
            .filter(|s| now.saturating_sub(s.start_time) < secs_30d)
            .map(|s| s.duration_secs)
            .sum();
        let last_365 = sessions
            .iter()
            .filter(|s| now.saturating_sub(s.start_time) < secs_365d)
            .map(|s| s.duration_secs)
            .sum();

        PlaytimeStats {
            total_sessions,
            total_secs,
            avg_session_secs: avg_session,
            longest_session_secs: longest,
            avg_fps_overall: avg_fps,
            last_7_days_secs: last_7,
            last_30_days_secs: last_30,
            last_365_days_secs: last_365,
        }
    }

    pub fn recently_played(&self, limit: usize) -> Vec<(GameId, u64)> {
        let mut latest: Vec<(GameId, u64)> = self
            .sessions
            .iter()
            .filter_map(|(id, sessions)| {
                sessions
                    .iter()
                    .map(|s| s.start_time + s.duration_secs)
                    .max()
                    .map(|t| (*id, t))
            })
            .collect();
        latest.sort_by(|a, b| b.1.cmp(&a.1));
        latest.truncate(limit);
        latest
    }

    pub fn most_played(&self, limit: usize) -> Vec<(GameId, u64)> {
        let mut totals: Vec<(GameId, u64)> = self
            .sessions
            .iter()
            .map(|(id, sessions)| {
                let total: u64 = sessions.iter().map(|s| s.duration_secs).sum();
                (*id, total)
            })
            .collect();
        totals.sort_by(|a, b| b.1.cmp(&a.1));
        totals.truncate(limit);
        totals
    }
}

// ---------------------------------------------------------------------------
// Library sort / filter / search — UI model
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortField {
    Name,
    LastPlayed,
    Playtime,
    InstallSize,
    Store,
    ReleaseYear,
    Rating,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortDirection {
    Ascending,
    Descending,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewMode {
    Grid,
    List,
    Compact,
}

#[derive(Clone, Debug)]
pub struct LibraryFilter {
    pub store: Option<StoreType>,
    pub installed_only: bool,
    pub tags: Vec<String>,
    pub min_playtime_secs: Option<u64>,
    pub search_query: Option<String>,
    pub hide_uninstalled: bool,
}

impl LibraryFilter {
    pub fn none() -> Self {
        Self {
            store: None,
            installed_only: false,
            tags: Vec::new(),
            min_playtime_secs: None,
            search_query: None,
            hide_uninstalled: false,
        }
    }

    pub fn installed() -> Self {
        Self {
            installed_only: true,
            ..Self::none()
        }
    }

    pub fn for_store(store: StoreType) -> Self {
        Self {
            store: Some(store),
            ..Self::none()
        }
    }

    pub fn matches(&self, game: &GameEntry) -> bool {
        if let Some(store) = self.store {
            if game.store != store {
                return false;
            }
        }

        if self.installed_only || self.hide_uninstalled {
            if !game.state.is_playable() && !matches!(game.state, GameState::Running { .. }) {
                return false;
            }
        }

        if !self.tags.is_empty() {
            let has_tag = self.tags.iter().any(|t| game.tags.contains(t));
            if !has_tag {
                return false;
            }
        }

        if let Some(min_pt) = self.min_playtime_secs {
            if game.total_playtime_secs < min_pt {
                return false;
            }
        }

        if let Some(query) = &self.search_query {
            if !fuzzy_match(&game.title, query) {
                return false;
            }
        }

        true
    }
}

/// Case-insensitive fuzzy substring match.
///
/// Returns `true` if all characters in `query` appear in `haystack` in
/// order (not necessarily contiguous).
pub fn fuzzy_match(haystack: &str, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }

    let lower_h: Vec<u8> = haystack.bytes().map(|b| to_ascii_lower(b)).collect();
    let lower_q: Vec<u8> = query.bytes().map(|b| to_ascii_lower(b)).collect();

    let mut qi = 0;
    for &hb in &lower_h {
        if qi < lower_q.len() && hb == lower_q[qi] {
            qi += 1;
        }
    }
    qi == lower_q.len()
}

/// Fuzzy match score — higher is better. Returns 0 if no match.
pub fn fuzzy_score(haystack: &str, query: &str) -> u32 {
    if query.is_empty() {
        return 1;
    }

    let lower_h: Vec<u8> = haystack.bytes().map(|b| to_ascii_lower(b)).collect();
    let lower_q: Vec<u8> = query.bytes().map(|b| to_ascii_lower(b)).collect();

    let mut qi = 0;
    let mut score: u32 = 0;
    let mut prev_match = false;

    for (i, &hb) in lower_h.iter().enumerate() {
        if qi < lower_q.len() && hb == lower_q[qi] {
            score += 10;
            if prev_match {
                score += 5; // consecutive match bonus
            }
            if i == 0 || (i > 0 && matches!(lower_h[i - 1], b' ' | b'_' | b'-' | b'.')) {
                score += 15; // word boundary bonus
            }
            qi += 1;
            prev_match = true;
        } else {
            prev_match = false;
        }
    }

    if qi == lower_q.len() {
        score
    } else {
        0
    }
}

fn to_ascii_lower(b: u8) -> u8 {
    if b >= b'A' && b <= b'Z' {
        b + 32
    } else {
        b
    }
}

// ---------------------------------------------------------------------------
// GameLibrary — the unified library
// ---------------------------------------------------------------------------

pub struct GameLibrary {
    pub games: Vec<GameEntry>,
    pub stores: Vec<StoreConnection>,
    pub profiles: BTreeMap<GameId, Vec<GameProfile>>,
    pub active_profile: BTreeMap<GameId, usize>,
    pub view_mode: ViewMode,
    pub sort_field: SortField,
    pub sort_direction: SortDirection,
    pub filter: LibraryFilter,
}

impl GameLibrary {
    pub fn new() -> Self {
        Self {
            games: Vec::new(),
            stores: Vec::new(),
            profiles: BTreeMap::new(),
            active_profile: BTreeMap::new(),
            view_mode: ViewMode::Grid,
            sort_field: SortField::Name,
            sort_direction: SortDirection::Ascending,
            filter: LibraryFilter::none(),
        }
    }

    pub fn add_game(&mut self, game: GameEntry) {
        let id = game.id;
        self.games.push(game);
        if !self.profiles.contains_key(&id) {
            let default = GameProfile::default_for(id);
            self.profiles.insert(id, alloc::vec![default]);
            self.active_profile.insert(id, 0);
        }
    }

    pub fn merge_from_connector(&mut self, entries: Vec<GameEntry>) {
        for entry in entries {
            let already = self
                .games
                .iter()
                .any(|g| g.store == entry.store && g.store_app_id == entry.store_app_id);
            if !already {
                self.add_game(entry);
            }
        }
    }

    pub fn remove_game(&mut self, id: GameId) -> bool {
        let before = self.games.len();
        self.games.retain(|g| g.id != id);
        self.profiles.remove(&id);
        self.active_profile.remove(&id);
        self.games.len() != before
    }

    pub fn find_game(&self, id: GameId) -> Option<&GameEntry> {
        self.games.iter().find(|g| g.id == id)
    }

    pub fn find_game_mut(&mut self, id: GameId) -> Option<&mut GameEntry> {
        self.games.iter_mut().find(|g| g.id == id)
    }

    pub fn find_by_store_app_id(&self, store: StoreType, app_id: &str) -> Option<&GameEntry> {
        self.games
            .iter()
            .find(|g| g.store == store && g.store_app_id == app_id)
    }

    pub fn game_count(&self) -> usize {
        self.games.len()
    }

    pub fn installed_count(&self) -> usize {
        self.games.iter().filter(|g| g.state.is_playable()).count()
    }

    pub fn total_install_size(&self) -> u64 {
        self.games
            .iter()
            .filter(|g| g.state.is_playable())
            .map(|g| g.install_size_bytes)
            .sum()
    }

    // --- Profile management ---

    pub fn add_profile(&mut self, profile: GameProfile) {
        self.profiles
            .entry(profile.game_id)
            .or_insert_with(Vec::new)
            .push(profile);
    }

    pub fn profiles_for(&self, id: GameId) -> &[GameProfile] {
        self.profiles.get(&id).map(|v| v.as_slice()).unwrap_or(&[])
    }

    pub fn active_profile_for(&self, id: GameId) -> Option<&GameProfile> {
        let idx = self.active_profile.get(&id)?;
        self.profiles.get(&id)?.get(*idx)
    }

    pub fn set_active_profile(&mut self, id: GameId, index: usize) -> bool {
        if let Some(profiles) = self.profiles.get(&id) {
            if index < profiles.len() {
                self.active_profile.insert(id, index);
                return true;
            }
        }
        false
    }

    // --- Playtime ---

    pub fn record_playtime(&mut self, id: GameId, seconds: u64, timestamp: u64) {
        if let Some(game) = self.games.iter_mut().find(|g| g.id == id) {
            game.total_playtime_secs += seconds;
            game.last_played = timestamp;
        }
    }

    // --- Sorting ---

    pub fn sorted_games(&self) -> Vec<&GameEntry> {
        let mut filtered: Vec<&GameEntry> = self
            .games
            .iter()
            .filter(|g| self.filter.matches(g))
            .collect();

        let dir = self.sort_direction;
        match self.sort_field {
            SortField::Name => {
                filtered.sort_by(|a, b| {
                    let cmp = a.title.cmp(&b.title);
                    if dir == SortDirection::Descending {
                        cmp.reverse()
                    } else {
                        cmp
                    }
                });
            }
            SortField::LastPlayed => {
                filtered.sort_by(|a, b| {
                    let cmp = a.last_played.cmp(&b.last_played);
                    if dir == SortDirection::Ascending {
                        cmp.reverse()
                    } else {
                        cmp
                    }
                });
            }
            SortField::Playtime => {
                filtered.sort_by(|a, b| {
                    let cmp = a.total_playtime_secs.cmp(&b.total_playtime_secs);
                    if dir == SortDirection::Ascending {
                        cmp.reverse()
                    } else {
                        cmp
                    }
                });
            }
            SortField::InstallSize => {
                filtered.sort_by(|a, b| {
                    let cmp = a.install_size_bytes.cmp(&b.install_size_bytes);
                    if dir == SortDirection::Ascending {
                        cmp.reverse()
                    } else {
                        cmp
                    }
                });
            }
            SortField::Store => {
                filtered.sort_by(|a, b| {
                    let cmp = a.store.cmp(&b.store);
                    if dir == SortDirection::Descending {
                        cmp.reverse()
                    } else {
                        cmp
                    }
                });
            }
            SortField::ReleaseYear => {
                filtered.sort_by(|a, b| {
                    let cmp = a.release_year.cmp(&b.release_year);
                    if dir == SortDirection::Descending {
                        cmp.reverse()
                    } else {
                        cmp
                    }
                });
            }
            SortField::Rating => {
                filtered.sort_by(|a, b| {
                    let cmp = a.rating.cmp(&b.rating);
                    if dir == SortDirection::Ascending {
                        cmp.reverse()
                    } else {
                        cmp
                    }
                });
            }
        }

        filtered
    }

    pub fn games_by_store(&self) -> BTreeMap<StoreType, Vec<&GameEntry>> {
        let mut by_store: BTreeMap<StoreType, Vec<&GameEntry>> = BTreeMap::new();
        for game in &self.games {
            by_store
                .entry(game.store)
                .or_insert_with(Vec::new)
                .push(game);
        }
        by_store
    }

    // --- Search ---

    pub fn search(&self, query: &str) -> Vec<&GameEntry> {
        if query.is_empty() {
            return self.games.iter().collect();
        }

        let mut scored: Vec<(&GameEntry, u32)> = self
            .games
            .iter()
            .filter_map(|g| {
                let s = fuzzy_score(&g.title, query);
                if s > 0 {
                    Some((g, s))
                } else {
                    None
                }
            })
            .collect();

        scored.sort_by(|a, b| b.1.cmp(&a.1));
        scored.into_iter().map(|(g, _)| g).collect()
    }

    /// Quick filter: recently played games.
    pub fn recently_played(&self, limit: usize) -> Vec<&GameEntry> {
        let mut sorted: Vec<&GameEntry> = self.games.iter().filter(|g| g.last_played > 0).collect();
        sorted.sort_by(|a, b| b.last_played.cmp(&a.last_played));
        sorted.truncate(limit);
        sorted
    }

    /// Quick filter: most played games.
    pub fn most_played(&self, limit: usize) -> Vec<&GameEntry> {
        let mut sorted: Vec<&GameEntry> = self
            .games
            .iter()
            .filter(|g| g.total_playtime_secs > 0)
            .collect();
        sorted.sort_by(|a, b| b.total_playtime_secs.cmp(&a.total_playtime_secs));
        sorted.truncate(limit);
        sorted
    }

    /// Quick filter: games with available updates.
    pub fn pending_updates(&self) -> Vec<&GameEntry> {
        self.games
            .iter()
            .filter(|g| matches!(g.state, GameState::UpdateAvailable))
            .collect()
    }

    /// Quick filter: currently running games.
    pub fn running_games(&self) -> Vec<&GameEntry> {
        self.games
            .iter()
            .filter(|g| matches!(g.state, GameState::Running { .. }))
            .collect()
    }

    // --- Store management ---

    pub fn add_store_connection(&mut self, conn: StoreConnection) {
        self.stores.push(conn);
    }

    pub fn store_connection(&self, store: StoreType) -> Option<&StoreConnection> {
        self.stores.iter().find(|c| c.store == store)
    }

    pub fn update_store_sync(&mut self, store: StoreType, timestamp: u64, count: usize) {
        if let Some(conn) = self.stores.iter_mut().find(|c| c.store == store) {
            conn.last_sync = timestamp;
            conn.game_count = count;
        }
    }

    // --- Tags ---

    pub fn all_tags(&self) -> Vec<&str> {
        let mut tags: Vec<&str> = Vec::new();
        for game in &self.games {
            for tag in &game.tags {
                if !tags.contains(&tag.as_str()) {
                    tags.push(tag.as_str());
                }
            }
        }
        tags.sort();
        tags
    }

    pub fn add_tag(&mut self, id: GameId, tag: String) {
        if let Some(game) = self.games.iter_mut().find(|g| g.id == id) {
            if !game.tags.contains(&tag) {
                game.tags.push(tag);
            }
        }
    }

    pub fn remove_tag(&mut self, id: GameId, tag: &str) {
        if let Some(game) = self.games.iter_mut().find(|g| g.id == id) {
            game.tags.retain(|t| t != tag);
        }
    }

    // --- Achievement summary ---

    pub fn total_achievements(&self) -> (usize, usize) {
        let mut unlocked = 0;
        let mut total = 0;
        for game in &self.games {
            total += game.achievements.len();
            unlocked += game.achievements.iter().filter(|a| a.unlocked).count();
        }
        (unlocked, total)
    }

    pub fn rarest_achievements(&self, limit: usize) -> Vec<(&GameEntry, &Achievement)> {
        let mut rare: Vec<(&GameEntry, &Achievement)> = self
            .games
            .iter()
            .flat_map(|g| {
                g.achievements
                    .iter()
                    .filter(|a| a.unlocked)
                    .map(move |a| (g, a))
            })
            .collect();
        rare.sort_by_key(|(_, a)| a.rarity);
        rare.truncate(limit);
        rare
    }
}

// ---------------------------------------------------------------------------
// Library view descriptor — for the UI layer
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct LibraryViewState {
    pub view_mode: ViewMode,
    pub sort_field: SortField,
    pub sort_direction: SortDirection,
    pub filter: LibraryFilter,
    pub selected_game: Option<GameId>,
    pub scroll_offset: u32,
    pub search_active: bool,
    pub search_text: String,
}

impl LibraryViewState {
    pub fn new() -> Self {
        Self {
            view_mode: ViewMode::Grid,
            sort_field: SortField::Name,
            sort_direction: SortDirection::Ascending,
            filter: LibraryFilter::none(),
            selected_game: None,
            scroll_offset: 0,
            search_active: false,
            search_text: String::new(),
        }
    }

    pub fn toggle_view(&mut self) {
        self.view_mode = match self.view_mode {
            ViewMode::Grid => ViewMode::List,
            ViewMode::List => ViewMode::Compact,
            ViewMode::Compact => ViewMode::Grid,
        };
    }

    pub fn toggle_sort_direction(&mut self) {
        self.sort_direction = match self.sort_direction {
            SortDirection::Ascending => SortDirection::Descending,
            SortDirection::Descending => SortDirection::Ascending,
        };
    }

    pub fn set_sort(&mut self, field: SortField) {
        if self.sort_field == field {
            self.toggle_sort_direction();
        } else {
            self.sort_field = field;
            self.sort_direction = SortDirection::Ascending;
        }
    }

    pub fn set_search(&mut self, query: String) {
        self.search_active = !query.is_empty();
        self.filter.search_query = if query.is_empty() {
            None
        } else {
            Some(query.clone())
        };
        self.search_text = query;
        self.scroll_offset = 0;
    }

    pub fn clear_search(&mut self) {
        self.search_active = false;
        self.search_text.clear();
        self.filter.search_query = None;
    }
}

// ---------------------------------------------------------------------------
// Game detail view descriptor
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct GameDetailView {
    pub game_id: GameId,
    pub active_tab: DetailTab,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DetailTab {
    Overview,
    Profiles,
    Achievements,
    PlaytimeStats,
    Settings,
}

impl GameDetailView {
    pub fn new(game_id: GameId) -> Self {
        Self {
            game_id,
            active_tab: DetailTab::Overview,
        }
    }
}

// ---------------------------------------------------------------------------
// Initialization helper
// ---------------------------------------------------------------------------

pub struct AthPlay {
    pub library: GameLibrary,
    pub launch_manager: LaunchManager,
    pub playtime: PlaytimeTracker,
    pub view: LibraryViewState,
}

impl AthPlay {
    pub fn new() -> Self {
        Self {
            library: GameLibrary::new(),
            launch_manager: LaunchManager::new(),
            playtime: PlaytimeTracker::new(),
            view: LibraryViewState::new(),
        }
    }

    pub fn sync_store(&mut self, connector: &mut dyn StoreConnector, timestamp: u64) {
        let store_type = connector.store_type();
        let entries = connector.scan_library();
        let count = entries.len();
        self.library.merge_from_connector(entries);
        self.library.update_store_sync(store_type, timestamp, count);
    }

    pub fn launch(
        &mut self,
        game_id: GameId,
        cb: &mut dyn LaunchCallbacks,
    ) -> Result<u64, LaunchError> {
        let entry = self
            .library
            .find_game(game_id)
            .ok_or(LaunchError::GameNotFound)?
            .clone();

        let profile = self
            .library
            .active_profile_for(game_id)
            .ok_or(LaunchError::ProfileNotFound)?
            .clone();

        let request = LaunchRequest {
            game_id,
            profile,
            environment_vars: Vec::new(),
            pre_launch_script: None,
            post_exit_script: None,
        };

        let pid = self.launch_manager.launch(&entry, &request, cb)?;

        if let Some(game) = self.library.find_game_mut(game_id) {
            game.state = GameState::Running { pid };
        }

        Ok(pid)
    }

    pub fn on_exit(
        &mut self,
        pid: u64,
        exit_code: i32,
        cb: &mut dyn LaunchCallbacks,
    ) -> Option<ExitReport> {
        let report = self.launch_manager.on_game_exit(pid, exit_code, cb)?;
        let start = cb
            .current_timestamp()
            .saturating_sub(report.session_duration_secs);
        self.playtime.record_from_exit(&report, start);
        self.library.record_playtime(
            report.game_id,
            report.session_duration_secs,
            cb.current_timestamp(),
        );

        if let Some(game) = self.library.find_game_mut(report.game_id) {
            game.state = GameState::Installed;
        }

        Some(report)
    }

    pub fn search(&self, query: &str) -> Vec<&GameEntry> {
        self.library.search(query)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TF2_VDF: &[u8] = br#""AppState"
{
    "appid"    "440"
    "name"     "Team Fortress 2"
    "StateFlags"  "4"
    "UserConfig"
    {
        "language"  "english"
    }
}"#;

    #[test]
    fn vdf_parses_a_steam_manifest() {
        let root = parse_vdf(TF2_VDF).expect("valid VDF");
        assert_eq!(root.name, "AppState");
        assert_eq!(root.get("appid"), Some("440"));
        assert_eq!(root.get_u64("appid"), Some(440));
        assert_eq!(root.get("name"), Some("Team Fortress 2"));
        assert_eq!(root.get("missing"), None);
        // Nested section.
        let cfg = root.child("UserConfig").expect("UserConfig child");
        assert_eq!(cfg.get("language"), Some("english"));
        assert!(root.child("Nope").is_none());
    }

    #[test]
    fn vdf_get_u64_rejects_non_numeric() {
        let root = parse_vdf(TF2_VDF).unwrap();
        assert_eq!(root.get_u64("name"), None); // "Team Fortress 2" is not a number
    }

    #[test]
    fn vdf_rejects_hostile_input_without_panicking() {
        assert!(parse_vdf(b"").is_err()); // empty
        assert!(parse_vdf(b"\"AppState\"").is_err()); // no opening brace
        assert!(parse_vdf(b"\"unterminated").is_err()); // unterminated quote
        assert!(parse_vdf(b"garbage no quotes").is_err());
    }

    #[test]
    fn fan_curve_clamps_and_interpolates() {
        let mut c = FanCurve::new();
        assert_eq!(c.fan_pct_at(50), 50); // empty → safe default
        c.add_point(60, 60);
        c.add_point(40, 20); // inserted out of order; add_point keeps it sorted
        c.add_point(80, 100);
        assert_eq!(c.fan_pct_at(30), 20); // below first point → floor
        assert_eq!(c.fan_pct_at(40), 20); // at first point
        assert_eq!(c.fan_pct_at(50), 40); // midway 40..60 between 20..60 → 40
        assert_eq!(c.fan_pct_at(60), 60); // at a point
        assert_eq!(c.fan_pct_at(90), 100); // above last point → ceiling
    }
}
