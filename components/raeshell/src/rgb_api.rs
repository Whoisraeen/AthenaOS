//! RGB Unified API — one API for all RGB hardware.
//!
//! Every motherboard, every fan, every keyboard, one API, one config.
//! RGB hell is a Windows problem; RaeenOS solves it.
//!
//! The `RgbDevice` trait abstracts over USB HID RGB devices.
//! `RgbManager` discovers devices and provides unified control.
//! Effects, profiles, and game integration are built in.

#![allow(dead_code)]

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

// ── Colour ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RgbColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl RgbColor {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    pub const fn from_hex(hex: u32) -> Self {
        Self {
            r: ((hex >> 16) & 0xFF) as u8,
            g: ((hex >> 8) & 0xFF) as u8,
            b: (hex & 0xFF) as u8,
        }
    }

    pub const fn to_hex(self) -> u32 {
        ((self.r as u32) << 16) | ((self.g as u32) << 8) | (self.b as u32)
    }

    pub fn lerp(self, other: Self, t_256: u16) -> Self {
        let inv = 256u16.saturating_sub(t_256);
        Self {
            r: ((self.r as u16 * inv + other.r as u16 * t_256) >> 8) as u8,
            g: ((self.g as u16 * inv + other.g as u16 * t_256) >> 8) as u8,
            b: ((self.b as u16 * inv + other.b as u16 * t_256) >> 8) as u8,
        }
    }

    pub const BLACK: Self = Self::new(0, 0, 0);
    pub const WHITE: Self = Self::new(255, 255, 255);
    pub const RED: Self = Self::new(255, 0, 0);
    pub const GREEN: Self = Self::new(0, 255, 0);
    pub const BLUE: Self = Self::new(0, 0, 255);

    pub fn hsv(h: u16, s: u8, v: u8) -> Self {
        if s == 0 {
            return Self::new(v, v, v);
        }
        let h = (h % 360) as u32;
        let s = s as u32;
        let v = v as u32;
        let region = h / 60;
        let remainder = (h - region * 60) * 255 / 60;
        let p = (v * (255 - s)) / 255;
        let q = (v * (255 - (s * remainder) / 255)) / 255;
        let t = (v * (255 - (s * (255 - remainder)) / 255)) / 255;

        match region {
            0 => Self::new(v as u8, t as u8, p as u8),
            1 => Self::new(q as u8, v as u8, p as u8),
            2 => Self::new(p as u8, v as u8, t as u8),
            3 => Self::new(p as u8, q as u8, v as u8),
            4 => Self::new(t as u8, p as u8, v as u8),
            _ => Self::new(v as u8, p as u8, q as u8),
        }
    }
}

// ── Zone ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZoneType {
    Single,
    Strip,
    Matrix,
    Logo,
    Fan,
    Ram,
    Gpu,
    Motherboard,
    Peripheral,
    Custom,
}

#[derive(Debug, Clone)]
pub struct Zone {
    pub id: u16,
    pub name: String,
    pub zone_type: ZoneType,
    pub led_count: u16,
    pub row_count: u8,
    pub col_count: u8,
}

impl Zone {
    pub fn single(id: u16, name: &str) -> Self {
        Self {
            id,
            name: String::from(name),
            zone_type: ZoneType::Single,
            led_count: 1,
            row_count: 1,
            col_count: 1,
        }
    }

    pub fn strip(id: u16, name: &str, count: u16) -> Self {
        Self {
            id,
            name: String::from(name),
            zone_type: ZoneType::Strip,
            led_count: count,
            row_count: 1,
            col_count: count as u8,
        }
    }

    pub fn matrix(id: u16, name: &str, rows: u8, cols: u8) -> Self {
        Self {
            id,
            name: String::from(name),
            zone_type: ZoneType::Matrix,
            led_count: rows as u16 * cols as u16,
            row_count: rows,
            col_count: cols,
        }
    }
}

// ── Effects ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RgbEffect {
    Static,
    Breathing,
    Rainbow,
    Wave,
    Reactive,
    AudioReactive,
    ColorCycle,
    Starlight,
    Ripple,
    Fire,
    Off,
}

#[derive(Debug, Clone)]
pub struct EffectConfig {
    pub effect: RgbEffect,
    pub speed: u8,
    pub brightness: u8,
    pub direction: EffectDirection,
    pub color1: RgbColor,
    pub color2: RgbColor,
    pub random_colors: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectDirection {
    Forward,
    Backward,
    Inward,
    Outward,
    Alternating,
}

impl EffectConfig {
    pub fn static_color(color: RgbColor) -> Self {
        Self {
            effect: RgbEffect::Static,
            speed: 0,
            brightness: 255,
            direction: EffectDirection::Forward,
            color1: color,
            color2: RgbColor::BLACK,
            random_colors: false,
        }
    }

    pub fn breathing(color: RgbColor, speed: u8) -> Self {
        Self {
            effect: RgbEffect::Breathing,
            speed,
            brightness: 255,
            direction: EffectDirection::Forward,
            color1: color,
            color2: RgbColor::BLACK,
            random_colors: false,
        }
    }

    pub fn rainbow(speed: u8) -> Self {
        Self {
            effect: RgbEffect::Rainbow,
            speed,
            brightness: 255,
            direction: EffectDirection::Forward,
            color1: RgbColor::RED,
            color2: RgbColor::BLUE,
            random_colors: false,
        }
    }

    pub fn wave(color1: RgbColor, color2: RgbColor, speed: u8) -> Self {
        Self {
            effect: RgbEffect::Wave,
            speed,
            brightness: 255,
            direction: EffectDirection::Forward,
            color1,
            color2,
            random_colors: false,
        }
    }

    pub fn off() -> Self {
        Self {
            effect: RgbEffect::Off,
            speed: 0,
            brightness: 0,
            direction: EffectDirection::Forward,
            color1: RgbColor::BLACK,
            color2: RgbColor::BLACK,
            random_colors: false,
        }
    }
}

// ── Effect state machine ─────────────────────────────────────────────────

pub struct EffectEngine {
    phase: u32,
    tick_count: u64,
}

impl EffectEngine {
    pub fn new() -> Self {
        Self {
            phase: 0,
            tick_count: 0,
        }
    }

    pub fn tick(&mut self, delta_ms: u32) {
        self.tick_count += 1;
        self.phase = self.phase.wrapping_add(delta_ms);
    }

    pub fn compute_color(&self, cfg: &EffectConfig, led_index: u16, led_count: u16) -> RgbColor {
        let speed = cfg.speed.max(1) as u32;
        let phase = self.phase / (256 / speed);
        let pos_frac = if led_count > 0 {
            (led_index as u32 * 256) / led_count as u32
        } else {
            0
        };

        match cfg.effect {
            RgbEffect::Static => cfg.color1,
            RgbEffect::Off => RgbColor::BLACK,

            RgbEffect::Breathing => {
                let breath = breath_curve(phase);
                let scale = (breath as u16 * cfg.brightness as u16) >> 8;
                RgbColor::new(
                    ((cfg.color1.r as u16 * scale) >> 8) as u8,
                    ((cfg.color1.g as u16 * scale) >> 8) as u8,
                    ((cfg.color1.b as u16 * scale) >> 8) as u8,
                )
            }

            RgbEffect::Rainbow => {
                let hue = ((phase + pos_frac) % 360) as u16;
                RgbColor::hsv(hue, 255, cfg.brightness)
            }

            RgbEffect::Wave => {
                let wave_pos = phase.wrapping_add(pos_frac);
                let t = (wave_pos % 512) as u16;
                let t = if t >= 256 { 512 - t } else { t };
                cfg.color1.lerp(cfg.color2, t)
            }

            RgbEffect::ColorCycle => {
                let hue = (phase % 360) as u16;
                RgbColor::hsv(hue, 255, cfg.brightness)
            }

            RgbEffect::Starlight => {
                let hash = simple_hash(led_index as u32, self.tick_count as u32);
                if hash % 20 == 0 {
                    cfg.color1
                } else {
                    let dim = (cfg.brightness as u16 * 30) / 255;
                    RgbColor::new(dim as u8, dim as u8, dim as u8)
                }
            }

            RgbEffect::Fire => {
                let heat = fire_heat(led_index, led_count, phase);
                heat_to_color(heat, cfg.brightness)
            }

            RgbEffect::Reactive | RgbEffect::AudioReactive | RgbEffect::Ripple => cfg.color1,
        }
    }
}

fn breath_curve(phase: u32) -> u8 {
    let p = (phase % 512) as u16;
    if p < 256 {
        p as u8
    } else {
        (511 - p) as u8
    }
}

fn simple_hash(a: u32, b: u32) -> u32 {
    let mut h = a.wrapping_mul(2654435761);
    h ^= b.wrapping_mul(2246822519);
    h ^= h >> 16;
    h
}

fn fire_heat(index: u16, count: u16, phase: u32) -> u8 {
    let pos_pct = if count > 0 {
        (index as u32 * 100) / count as u32
    } else {
        50
    };
    let base = 255u32.saturating_sub(pos_pct * 2);
    let wobble = simple_hash(index as u32 + phase / 50, phase / 100) % 60;
    base.saturating_sub(wobble).min(255) as u8
}

fn heat_to_color(heat: u8, brightness: u8) -> RgbColor {
    let h = heat as u16;
    let b = brightness as u16;
    let r = ((h.min(85) * 3 * b) >> 8).min(255) as u8;
    let g = if heat > 85 {
        (((h - 85).min(85) * 3 * b) >> 8).min(255) as u8
    } else {
        0
    };
    let bl = if heat > 170 {
        (((h - 170) * 3 * b) >> 8).min(255) as u8
    } else {
        0
    };
    RgbColor::new(r, g, bl)
}

// ── RgbDevice trait ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceType {
    Keyboard,
    Mouse,
    Mousepad,
    Headset,
    Fan,
    Strip,
    Ram,
    Gpu,
    Motherboard,
    Case,
    Other,
}

#[derive(Debug, Clone)]
pub struct DeviceInfo {
    pub name: String,
    pub vendor: String,
    pub device_type: DeviceType,
    pub vendor_id: u16,
    pub product_id: u16,
    pub serial: String,
    pub firmware: String,
}

pub trait RgbDevice {
    fn info(&self) -> &DeviceInfo;
    fn zones(&self) -> &[Zone];
    fn set_color(&mut self, zone_id: u16, color: RgbColor) -> bool;
    fn set_effect(&mut self, zone_id: u16, effect: &EffectConfig) -> bool;
    fn get_color(&self, zone_id: u16) -> Option<RgbColor>;
    fn set_brightness(&mut self, brightness: u8);
    fn brightness(&self) -> u8;
    fn apply(&mut self);
}

// ── Stub USB HID device ──────────────────────────────────────────────────

pub struct StubRgbDevice {
    pub info: DeviceInfo,
    pub zones: Vec<Zone>,
    pub colors: Vec<RgbColor>,
    pub effects: Vec<Option<EffectConfig>>,
    pub brightness: u8,
}

impl StubRgbDevice {
    pub fn new(info: DeviceInfo, zones: Vec<Zone>) -> Self {
        let n = zones.len();
        Self {
            info,
            zones,
            colors: alloc::vec![RgbColor::BLACK; n],
            effects: alloc::vec![None; n],
            brightness: 255,
        }
    }
}

impl RgbDevice for StubRgbDevice {
    fn info(&self) -> &DeviceInfo {
        &self.info
    }
    fn zones(&self) -> &[Zone] {
        &self.zones
    }

    fn set_color(&mut self, zone_id: u16, color: RgbColor) -> bool {
        if let Some(idx) = self.zones.iter().position(|z| z.id == zone_id) {
            self.colors[idx] = color;
            self.effects[idx] = None;
            true
        } else {
            false
        }
    }

    fn set_effect(&mut self, zone_id: u16, effect: &EffectConfig) -> bool {
        if let Some(idx) = self.zones.iter().position(|z| z.id == zone_id) {
            self.effects[idx] = Some(effect.clone());
            true
        } else {
            false
        }
    }

    fn get_color(&self, zone_id: u16) -> Option<RgbColor> {
        self.zones
            .iter()
            .position(|z| z.id == zone_id)
            .map(|idx| self.colors[idx])
    }

    fn set_brightness(&mut self, b: u8) {
        self.brightness = b;
    }
    fn brightness(&self) -> u8 {
        self.brightness
    }
    fn apply(&mut self) {}
}

// ── RGB Profile system ───────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ZoneConfig {
    pub zone_id: u16,
    pub effect: EffectConfig,
}

#[derive(Debug, Clone)]
pub struct RgbProfile {
    pub name: String,
    pub zones: Vec<ZoneConfig>,
    pub brightness: u8,
}

impl RgbProfile {
    pub fn new(name: &str) -> Self {
        Self {
            name: String::from(name),
            zones: Vec::new(),
            brightness: 255,
        }
    }

    pub fn add_zone(&mut self, zone_id: u16, effect: EffectConfig) {
        self.zones.push(ZoneConfig { zone_id, effect });
    }
}

// ── Game integration events ──────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GameEvent {
    HealthChanged { pct: u8 },
    AmmoChanged { pct: u8 },
    Damage,
    Kill,
    Death,
    Victory,
    Defeat,
    Ability { id: u8 },
    EnvironmentChange { r: u8, g: u8, b: u8 },
    Custom { id: u16, value: u16 },
}

#[derive(Debug, Clone)]
pub struct GameRgbMapping {
    pub event: GameEvent,
    pub zone_id: u16,
    pub effect: EffectConfig,
    pub duration_ms: u32,
}

pub struct GameRgbIntegration {
    pub game_name: String,
    pub mappings: Vec<GameRgbMapping>,
    pub active: bool,
    pub active_flash: Option<(u16, RgbColor, u32)>,
}

impl GameRgbIntegration {
    pub fn new(game_name: &str) -> Self {
        Self {
            game_name: String::from(game_name),
            mappings: Vec::new(),
            active: false,
            active_flash: None,
        }
    }

    pub fn add_mapping(&mut self, mapping: GameRgbMapping) {
        self.mappings.push(mapping);
    }

    pub fn on_event(&mut self, event: GameEvent) -> Vec<(u16, EffectConfig, u32)> {
        if !self.active {
            return Vec::new();
        }
        let mut results = Vec::new();
        for m in &self.mappings {
            if core::mem::discriminant(&m.event) == core::mem::discriminant(&event) {
                results.push((m.zone_id, m.effect.clone(), m.duration_ms));
            }
        }
        results
    }

    pub fn health_flash(pct: u8) -> RgbColor {
        if pct > 66 {
            RgbColor::GREEN
        } else if pct > 33 {
            RgbColor::new(255, 165, 0)
        } else {
            RgbColor::RED
        }
    }
}

// ── RGB Manager — discovers and controls all devices ─────────────────────

pub struct RgbManager {
    devices: Vec<StubRgbDevice>,
    profiles: Vec<RgbProfile>,
    active_profile: Option<String>,
    engine: EffectEngine,
    game_integration: Option<GameRgbIntegration>,
    global_brightness: u8,
    enabled: bool,
}

impl RgbManager {
    pub fn new() -> Self {
        Self {
            devices: Vec::new(),
            profiles: Vec::new(),
            active_profile: None,
            engine: EffectEngine::new(),
            game_integration: None,
            global_brightness: 255,
            enabled: true,
        }
    }

    pub fn discover_devices(&mut self) {
        // In a real system, USB HID enumeration would happen here.
        // For now, stub devices represent common RGB hardware.
    }

    pub fn add_device(&mut self, device: StubRgbDevice) {
        self.devices.push(device);
    }

    pub fn device_count(&self) -> usize {
        self.devices.len()
    }

    pub fn device_names(&self) -> Vec<&str> {
        self.devices.iter().map(|d| d.info.name.as_str()).collect()
    }

    pub fn all_zones(&self) -> Vec<(usize, &Zone)> {
        let mut result = Vec::new();
        for (dev_idx, dev) in self.devices.iter().enumerate() {
            for zone in &dev.zones {
                result.push((dev_idx, zone));
            }
        }
        result
    }

    pub fn set_all_color(&mut self, color: RgbColor) {
        for dev in &mut self.devices {
            for zone in dev.zones.iter() {
                let _ = dev
                    .colors
                    .get_mut(dev.zones.iter().position(|z| z.id == zone.id).unwrap_or(0))
                    .map(|c| *c = color);
            }
        }
    }

    pub fn set_all_effect(&mut self, effect: &EffectConfig) {
        for dev in &mut self.devices {
            let zone_ids: Vec<u16> = dev.zones.iter().map(|z| z.id).collect();
            for zid in zone_ids {
                dev.set_effect(zid, effect);
            }
        }
    }

    pub fn set_device_color(&mut self, dev_idx: usize, color: RgbColor) {
        if let Some(dev) = self.devices.get_mut(dev_idx) {
            let zone_ids: Vec<u16> = dev.zones.iter().map(|z| z.id).collect();
            for zid in zone_ids {
                dev.set_color(zid, color);
            }
        }
    }

    pub fn set_zone_color(&mut self, dev_idx: usize, zone_id: u16, color: RgbColor) {
        if let Some(dev) = self.devices.get_mut(dev_idx) {
            dev.set_color(zone_id, color);
        }
    }

    pub fn set_zone_effect(&mut self, dev_idx: usize, zone_id: u16, effect: &EffectConfig) {
        if let Some(dev) = self.devices.get_mut(dev_idx) {
            dev.set_effect(zone_id, effect);
        }
    }

    pub fn set_global_brightness(&mut self, brightness: u8) {
        self.global_brightness = brightness;
        for dev in &mut self.devices {
            dev.set_brightness(brightness);
        }
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        if !enabled {
            self.set_all_color(RgbColor::BLACK);
        }
    }

    pub fn tick(&mut self, delta_ms: u32) {
        if !self.enabled {
            return;
        }
        self.engine.tick(delta_ms);

        for dev in &mut self.devices {
            for (idx, zone) in dev.zones.iter().enumerate() {
                if let Some(ref cfg) = dev.effects[idx] {
                    let color = self.engine.compute_color(cfg, 0, zone.led_count);
                    dev.colors[idx] = color;
                }
            }
            dev.apply();
        }
    }

    // ── Profile management ───────────────────────────────────────────

    pub fn save_profile(&mut self, name: &str) {
        let mut profile = RgbProfile::new(name);
        profile.brightness = self.global_brightness;
        for dev in &self.devices {
            for (idx, zone) in dev.zones.iter().enumerate() {
                let effect = dev.effects[idx]
                    .clone()
                    .unwrap_or_else(|| EffectConfig::static_color(dev.colors[idx]));
                profile.add_zone(zone.id, effect);
            }
        }
        if let Some(existing) = self.profiles.iter_mut().find(|p| p.name == name) {
            *existing = profile;
        } else {
            self.profiles.push(profile);
        }
    }

    pub fn load_profile(&mut self, name: &str) -> bool {
        let profile = match self.profiles.iter().find(|p| p.name == name) {
            Some(p) => p.clone(),
            None => return false,
        };
        self.global_brightness = profile.brightness;
        for dev in &mut self.devices {
            dev.set_brightness(profile.brightness);
        }
        for zc in &profile.zones {
            for dev in &mut self.devices {
                dev.set_effect(zc.zone_id, &zc.effect);
            }
        }
        self.active_profile = Some(String::from(name));
        true
    }

    pub fn delete_profile(&mut self, name: &str) {
        self.profiles.retain(|p| p.name != name);
        if self.active_profile.as_deref() == Some(name) {
            self.active_profile = None;
        }
    }

    pub fn profile_names(&self) -> Vec<&str> {
        self.profiles.iter().map(|p| p.name.as_str()).collect()
    }

    pub fn active_profile_name(&self) -> Option<&str> {
        self.active_profile.as_deref()
    }

    // ── Game integration ─────────────────────────────────────────────

    pub fn set_game_integration(&mut self, integration: GameRgbIntegration) {
        self.game_integration = Some(integration);
    }

    pub fn clear_game_integration(&mut self) {
        self.game_integration = None;
    }

    pub fn on_game_event(&mut self, event: GameEvent) {
        if let Some(ref mut gi) = self.game_integration {
            let actions = gi.on_event(event);
            for (zone_id, effect, _duration) in actions {
                for dev in &mut self.devices {
                    dev.set_effect(zone_id, &effect);
                }
            }
        }
    }
}

// ── Default profiles ─────────────────────────────────────────────────────

pub fn create_default_profiles() -> Vec<RgbProfile> {
    let mut profiles = Vec::new();

    let mut gaming = RgbProfile::new("Gaming");
    gaming.brightness = 255;
    gaming.add_zone(0, EffectConfig::rainbow(128));
    profiles.push(gaming);

    let mut stealth = RgbProfile::new("Stealth");
    stealth.brightness = 50;
    stealth.add_zone(0, EffectConfig::static_color(RgbColor::new(30, 0, 50)));
    profiles.push(stealth);

    let mut wave = RgbProfile::new("Ocean Wave");
    wave.brightness = 200;
    wave.add_zone(
        0,
        EffectConfig::wave(RgbColor::new(0, 80, 200), RgbColor::new(0, 200, 180), 80),
    );
    profiles.push(wave);

    let mut breath = RgbProfile::new("Chill Breathing");
    breath.brightness = 180;
    breath.add_zone(0, EffectConfig::breathing(RgbColor::new(78, 156, 255), 60));
    profiles.push(breath);

    let mut off = RgbProfile::new("Off");
    off.brightness = 0;
    off.add_zone(0, EffectConfig::off());
    profiles.push(off);

    profiles
}

// ── Game-foreground RGB binding (MasterChecklist L1610) ──────────────────
// "RGB strip changes color when a game enters foreground" — the gaming hook of
// the customization pillar. Holds the user's normal effect plus a game effect;
// when the kernel marks a game foreground (`game_session`), the active effect
// switches to the game one and reverts on exit. Pure logic: the per-device
// `EffectEngine` renders whatever `active()` returns, and the kernel's
// game-foreground/exit event drives `set_game_foreground`.
#[derive(Debug, Clone)]
pub struct GameRgbProfile {
    base: EffectConfig,
    game: EffectConfig,
    in_game: bool,
}

impl GameRgbProfile {
    /// `base` runs on the desktop; `game` runs while a game is foreground.
    pub fn new(base: EffectConfig, game: EffectConfig) -> Self {
        Self {
            base,
            game,
            in_game: false,
        }
    }

    /// Update the game-foreground state. Returns `true` if the active effect
    /// CHANGED (so the caller re-applies it to the devices), `false` if no-op —
    /// so the kernel can call it on every game event without redundant re-applies.
    pub fn set_game_foreground(&mut self, in_game: bool) -> bool {
        let changed = self.in_game != in_game;
        self.in_game = in_game;
        changed
    }

    /// The effect to render right now (the game effect while a game is foreground).
    pub fn active(&self) -> &EffectConfig {
        if self.in_game {
            &self.game
        } else {
            &self.base
        }
    }

    pub fn is_game_active(&self) -> bool {
        self.in_game
    }

    /// Replace the desktop (non-game) effect without disturbing the
    /// game-foreground state — e.g. the user picks a new desktop effect while a
    /// game is running; it takes effect when the game exits.
    pub fn set_base(&mut self, base: EffectConfig) {
        self.base = base;
    }
}

// ── Host KATs (dev box, `cargo test -p raeshell`) ────────────────────────
#[cfg(test)]
mod game_rgb_kat {
    use super::*;

    #[test]
    fn game_foreground_switches_effect_and_reverts() {
        let base = EffectConfig::breathing(RgbColor::BLUE, 4);
        let game = EffectConfig::static_color(RgbColor::RED);
        let mut p = GameRgbProfile::new(base, game);

        // Desktop: the user's base effect is active.
        assert_eq!(p.active().effect, RgbEffect::Breathing);
        assert_eq!(p.active().color1, RgbColor::BLUE);
        assert!(!p.is_game_active());

        // Game enters foreground -> game effect active, reports a change.
        assert!(p.set_game_foreground(true));
        assert_eq!(p.active().effect, RgbEffect::Static);
        assert_eq!(p.active().color1, RgbColor::RED);
        assert!(p.is_game_active());

        // Idempotent: re-asserting foreground is a no-op (no redundant re-apply).
        assert!(!p.set_game_foreground(true));

        // Game exits -> reverts to the base effect, reports a change.
        assert!(p.set_game_foreground(false));
        assert_eq!(p.active().effect, RgbEffect::Breathing);
        assert_eq!(p.active().color1, RgbColor::BLUE);
        assert!(!p.set_game_foreground(false));
    }

    #[test]
    fn set_base_during_game_takes_effect_on_exit() {
        let mut p = GameRgbProfile::new(EffectConfig::off(), EffectConfig::rainbow(6));
        assert!(p.set_game_foreground(true));
        // User changes their desktop effect while a game is foreground.
        p.set_base(EffectConfig::static_color(RgbColor::GREEN));
        // Game effect still active (rainbow), game state intact.
        assert_eq!(p.active().effect, RgbEffect::Rainbow);
        assert!(p.is_game_active());
        // On exit, the NEW base applies.
        p.set_game_foreground(false);
        assert_eq!(p.active().effect, RgbEffect::Static);
        assert_eq!(p.active().color1, RgbColor::GREEN);
    }
}

// ── Per-device profile persistence (MasterChecklist L1602) ───────────────
// Serialize an RgbProfile to a compact, versioned byte blob so each device's
// profile survives a reboot (the customization engine stores the blob in
// versioned config). Round-trippable; from_bytes is fully bounds-checked and
// returns None on a truncated/malformed/hostile blob (never panics).
impl RgbEffect {
    fn to_u8(self) -> u8 {
        match self {
            RgbEffect::Static => 0,
            RgbEffect::Breathing => 1,
            RgbEffect::Rainbow => 2,
            RgbEffect::Wave => 3,
            RgbEffect::Reactive => 4,
            RgbEffect::AudioReactive => 5,
            RgbEffect::ColorCycle => 6,
            RgbEffect::Starlight => 7,
            RgbEffect::Ripple => 8,
            RgbEffect::Fire => 9,
            RgbEffect::Off => 10,
        }
    }
    fn from_u8(v: u8) -> Option<Self> {
        Some(match v {
            0 => RgbEffect::Static,
            1 => RgbEffect::Breathing,
            2 => RgbEffect::Rainbow,
            3 => RgbEffect::Wave,
            4 => RgbEffect::Reactive,
            5 => RgbEffect::AudioReactive,
            6 => RgbEffect::ColorCycle,
            7 => RgbEffect::Starlight,
            8 => RgbEffect::Ripple,
            9 => RgbEffect::Fire,
            10 => RgbEffect::Off,
            _ => return None,
        })
    }
}

impl EffectDirection {
    fn to_u8(self) -> u8 {
        match self {
            EffectDirection::Forward => 0,
            EffectDirection::Backward => 1,
            EffectDirection::Inward => 2,
            EffectDirection::Outward => 3,
            EffectDirection::Alternating => 4,
        }
    }
    fn from_u8(v: u8) -> Option<Self> {
        Some(match v {
            0 => EffectDirection::Forward,
            1 => EffectDirection::Backward,
            2 => EffectDirection::Inward,
            3 => EffectDirection::Outward,
            4 => EffectDirection::Alternating,
            _ => return None,
        })
    }
}

const RGB_PROFILE_MAGIC: [u8; 2] = [b'R', b'P'];
const RGB_PROFILE_VERSION: u8 = 1;

/// Bounds-checked read of `n` bytes at `*p`, advancing `p`. None if short.
fn rgb_read_n<'a>(buf: &'a [u8], p: &mut usize, n: usize) -> Option<&'a [u8]> {
    let s = buf.get(*p..p.checked_add(n)?)?;
    *p += n;
    Some(s)
}

impl RgbProfile {
    /// Serialize to a compact versioned blob (magic "RP", v1) for versioned
    /// config storage. Name + zone count are capped at 255 (a u8 length each).
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&RGB_PROFILE_MAGIC);
        out.push(RGB_PROFILE_VERSION);
        out.push(self.brightness);
        let name = self.name.as_bytes();
        let nlen = name.len().min(255);
        out.push(nlen as u8);
        out.extend_from_slice(&name[..nlen]);
        let zcount = self.zones.len().min(255);
        out.push(zcount as u8);
        for z in self.zones.iter().take(zcount) {
            out.extend_from_slice(&z.zone_id.to_le_bytes());
            let e = &z.effect;
            out.push(e.effect.to_u8());
            out.push(e.speed);
            out.push(e.brightness);
            out.push(e.direction.to_u8());
            out.push(e.color1.r);
            out.push(e.color1.g);
            out.push(e.color1.b);
            out.push(e.color2.r);
            out.push(e.color2.g);
            out.push(e.color2.b);
            out.push(e.random_colors as u8);
        }
        out
    }

    /// Reload a profile from a blob. None on a truncated/malformed blob (bad
    /// magic/version, invalid effect/direction id, bad UTF-8 name, short read).
    pub fn from_bytes(buf: &[u8]) -> Option<RgbProfile> {
        let mut p = 0usize;
        if rgb_read_n(buf, &mut p, 2)? != RGB_PROFILE_MAGIC {
            return None;
        }
        if rgb_read_n(buf, &mut p, 1)?[0] != RGB_PROFILE_VERSION {
            return None;
        }
        let brightness = rgb_read_n(buf, &mut p, 1)?[0];
        let nlen = rgb_read_n(buf, &mut p, 1)?[0] as usize;
        let name = core::str::from_utf8(rgb_read_n(buf, &mut p, nlen)?).ok()?;
        let mut prof = RgbProfile::new(name);
        prof.brightness = brightness;
        let zcount = rgb_read_n(buf, &mut p, 1)?[0] as usize;
        for _ in 0..zcount {
            let zid = u16::from_le_bytes([
                rgb_read_n(buf, &mut p, 1)?[0],
                rgb_read_n(buf, &mut p, 1)?[0],
            ]);
            let effect = RgbEffect::from_u8(rgb_read_n(buf, &mut p, 1)?[0])?;
            let speed = rgb_read_n(buf, &mut p, 1)?[0];
            let ebr = rgb_read_n(buf, &mut p, 1)?[0];
            let dir = EffectDirection::from_u8(rgb_read_n(buf, &mut p, 1)?[0])?;
            let c1 = RgbColor::new(
                rgb_read_n(buf, &mut p, 1)?[0],
                rgb_read_n(buf, &mut p, 1)?[0],
                rgb_read_n(buf, &mut p, 1)?[0],
            );
            let c2 = RgbColor::new(
                rgb_read_n(buf, &mut p, 1)?[0],
                rgb_read_n(buf, &mut p, 1)?[0],
                rgb_read_n(buf, &mut p, 1)?[0],
            );
            let rnd = rgb_read_n(buf, &mut p, 1)?[0] != 0;
            prof.add_zone(
                zid,
                EffectConfig {
                    effect,
                    speed,
                    brightness: ebr,
                    direction: dir,
                    color1: c1,
                    color2: c2,
                    random_colors: rnd,
                },
            );
        }
        Some(prof)
    }
}

#[cfg(test)]
mod rgb_persist_kat {
    use super::*;

    #[test]
    fn rgb_profile_round_trips() {
        let mut prof = RgbProfile::new("Cyberpunk");
        prof.brightness = 180;
        prof.add_zone(0, EffectConfig::breathing(RgbColor::new(78, 156, 255), 60));
        prof.add_zone(7, EffectConfig::wave(RgbColor::RED, RgbColor::BLUE, 30));

        let bytes = prof.to_bytes();
        let back = RgbProfile::from_bytes(&bytes).expect("round-trip");
        assert_eq!(back.name, "Cyberpunk");
        assert_eq!(back.brightness, 180);
        assert_eq!(back.zones.len(), 2);
        assert_eq!(back.zones[0].zone_id, 0);
        assert_eq!(back.zones[0].effect.effect, RgbEffect::Breathing);
        assert_eq!(back.zones[0].effect.color1, RgbColor::new(78, 156, 255));
        assert_eq!(back.zones[0].effect.speed, 60);
        assert_eq!(back.zones[1].zone_id, 7);
        assert_eq!(back.zones[1].effect.effect, RgbEffect::Wave);
        assert_eq!(back.zones[1].effect.color2, RgbColor::BLUE);
        // Re-serializing the parsed profile is byte-identical (stable encoding).
        assert_eq!(back.to_bytes(), bytes);
    }

    #[test]
    fn from_bytes_rejects_malformed_without_panic() {
        assert!(RgbProfile::from_bytes(&[]).is_none()); // empty
        assert!(RgbProfile::from_bytes(b"XX\x01").is_none()); // wrong magic
        assert!(RgbProfile::from_bytes(b"RP\x02").is_none()); // wrong version

        let mut ok = RgbProfile::new("x");
        ok.add_zone(0, EffectConfig::static_color(RgbColor::RED));
        // Truncated mid-zone -> None.
        let mut trunc = ok.to_bytes();
        trunc.truncate(trunc.len() - 3);
        assert!(RgbProfile::from_bytes(&trunc).is_none());
        // Invalid effect id -> None (not a panic). Effect byte sits after
        // magic(2)+ver(1)+bri(1)+nlen(1)+name(1)+zcount(1)+zid(2) = 9.
        let mut bad = ok.to_bytes();
        bad[9] = 200;
        assert!(RgbProfile::from_bytes(&bad).is_none());
    }
}
