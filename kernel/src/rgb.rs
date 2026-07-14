//! Unified RGB control API — Concept §Customization Engine:
//!
//! > "RGB unified — every motherboard, every fan, every keyboard, one API,
//! >  one config. RGB hell is a Windows problem; RaeenOS solves it."
//!
//! Windows users run NVIDIA Aurora, ASUS Aura Sync, MSI Mystic Light,
//! Gigabyte Fusion, Corsair iCUE, Razer Synapse, Logitech G Hub, ASRock
//! Polychrome, Cooler Master MasterPlus, NZXT CAM, and OpenRGB to control
//! lights — sometimes simultaneously, fighting each other for the same I²C
//! bus. RaeenOS ships one API.
//!
//! Each light-emitting peripheral is a `Device` with a stable id, a `Kind`,
//! and one or more `Zone`s. A zone is the smallest unit you can address
//! (a single LED on a strip, a keyboard key, a fan ring, the AIO pump cap).
//! Userspace either sets per-zone colors or applies a named effect; the
//! kernel keeps the canonical state so the on-disk config, the Settings
//! → Personalization panel, and any Vibe Mode theme all read from the
//! same place.
//!
//! ## Syscalls (62-65)
//!
//! | nr | name        | rdi/rsi/rdx/r10                                        | rax |
//! |----|-------------|--------------------------------------------------------|----|
//! | 62 | RGB_LIST    | rdi=out_ptr, rsi=out_cap (16 B per device)             | count |
//! | 63 | RGB_QUERY   | rdi=device_id, rsi=out_ptr, rdx=cap (struct DeviceAbi) | bytes or u64::MAX |
//! | 64 | RGB_SET     | rdi=device_id, rsi=zone, rdx=ARGB color, r10=brightness| 0/err |
//! | 65 | RGB_EFFECT  | rdi=device_id, rsi=effect_id, rdx=speed, r10=color     | 0/err |

#![allow(dead_code)]

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

// ── Device taxonomy ────────────────────────────────────────────────────

#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Motherboard = 1,
    Keyboard = 2,
    Mouse = 3,
    Headset = 4,
    Ram = 5,
    Gpu = 6,
    Fan = 7,
    AioPump = 8,
    LedStrip = 9,
    Case = 10,
    Other = 99,
}

#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    Static = 0,
    Breathing = 1,
    Rainbow = 2,
    ColorCycle = 3,
    Pulse = 4,
    Wave = 5,
    ReactiveType = 6, // keyboard: light up keys as user types
    AudioReact = 7,   // tied to RaeAudio waveform peak
    GameLink = 8,     // driven by per-game profile / game state
}

impl Kind {
    fn label(self) -> &'static str {
        match self {
            Kind::Motherboard => "motherboard",
            Kind::Keyboard => "keyboard",
            Kind::Mouse => "mouse",
            Kind::Headset => "headset",
            Kind::Ram => "ram",
            Kind::Gpu => "gpu",
            Kind::Fan => "fan",
            Kind::AioPump => "aio_pump",
            Kind::LedStrip => "led_strip",
            Kind::Case => "case",
            Kind::Other => "other",
        }
    }
}

#[derive(Debug, Clone)]
struct Device {
    id: u64,
    kind: Kind,
    name: String,
    /// One ARGB color per zone (0xAARRGGBB).
    zone_colors: Vec<u32>,
    brightness: u8, // 0..=100
    effect: Effect,
    effect_speed: u8, // 0..=100
    effect_color: u32,
}

impl Device {
    fn new(id: u64, kind: Kind, name: &str, zones: usize) -> Self {
        Self {
            id,
            kind,
            name: String::from(name),
            zone_colors: alloc::vec![0xFF_00_00_00; zones],
            brightness: 100,
            effect: Effect::Static,
            effect_speed: 50,
            effect_color: 0xFF_4E_9C_FF, // RaeBlue
        }
    }
}

// ── Effect animation math ───────────────────────────────────────────────────
//
// Pure integer functions (no FPU, no_std) that turn an effect + time into the
// ARGB a zone should display. The registry stores the effect; an animation tick
// (or a userspace RGB daemon) calls `Effect::frame_color` per zone per frame to
// drive real LEDs. Kept FPU-free so it computes identically on host and kernel
// (host-pattern-tested via run_effect_smoketest).

/// Scale a colour's RGB channels by `level`/255 (alpha preserved).
fn scale_rgb(argb: u32, level: u32) -> u32 {
    let a = argb & 0xFF00_0000;
    let r = ((argb >> 16) & 0xFF) * level / 255;
    let g = ((argb >> 8) & 0xFF) * level / 255;
    let b = (argb & 0xFF) * level / 255;
    a | (r << 16) | (g << 8) | b
}

/// A 0..=255 triangle wave over `[0, period)` — ramps up then back down.
fn triangle(x: u64, period: u64) -> u32 {
    if period == 0 {
        return 255;
    }
    let half = period / 2;
    let pos = x % period;
    let up = if pos < half {
        pos * 255 / half.max(1)
    } else {
        255 - (pos - half) * 255 / half.max(1)
    };
    up as u32
}

/// Integer HSV→ARGB. `h` in degrees 0..359, `s`/`v` 0..=255. Full alpha.
fn hsv_to_argb(h: u16, s: u32, v: u32) -> u32 {
    let h = (h % 360) as u32;
    let region = h / 60;
    let rem = h % 60;
    let p = v * (255 - s) / 255;
    let q = v * (255 - s * rem / 60) / 255;
    let t = v * (255 - s * (60 - rem) / 60) / 255;
    let (r, g, b) = match region {
        0 => (v, t, p),
        1 => (q, v, p),
        2 => (p, v, t),
        3 => (p, q, v),
        4 => (t, p, v),
        _ => (v, p, q),
    };
    0xFF00_0000 | (r << 16) | (g << 8) | b
}

/// Milliseconds per hue-degree for cycling effects (faster `speed` → smaller).
fn hue_step_ms(speed: u8) -> u64 {
    // speed 0 → ~30ms/deg (~11s/cycle); speed 100 → ~3ms/deg (~1s/cycle).
    (30 - (speed as u64 * 27 / 100)).max(1)
}

/// Full breathing-cycle period in ms (faster `speed` → shorter).
fn breathing_period_ms(speed: u8) -> u64 {
    // speed 0 → 4000ms; speed 100 → 600ms.
    (4000 - (speed as u64 * 3400 / 100)).max(200)
}

impl Effect {
    /// The ARGB a zone should display for this effect at time `t_ms`. `base` is
    /// the configured effect colour, `zone`/`zones` locate the LED, `speed`
    /// (0..=100) sets the rate, `brightness` (0..=100) scales the output.
    pub fn frame_color(
        self,
        base: u32,
        t_ms: u64,
        zone: usize,
        zones: usize,
        speed: u8,
        brightness: u8,
    ) -> u32 {
        let bl = brightness.min(100) as u32 * 255 / 100;
        let dim = |c: u32| scale_rgb(c, bl);
        match self {
            Effect::Static => dim(base),
            Effect::Breathing | Effect::Pulse => {
                let level = triangle(t_ms, breathing_period_ms(speed));
                dim(scale_rgb(base, level))
            }
            Effect::Rainbow | Effect::ColorCycle => {
                let hue = ((t_ms / hue_step_ms(speed)) % 360) as u16;
                dim(hsv_to_argb(hue, 255, 255))
            }
            Effect::Wave => {
                // Rainbow with a per-zone phase offset so colour sweeps across
                // the device's zones.
                let phase = (zone as u64) * 360 / zones.max(1) as u64;
                let hue = (((t_ms / hue_step_ms(speed)) + phase) % 360) as u16;
                dim(hsv_to_argb(hue, 255, 255))
            }
            // Live-input-driven effects (typing/audio/game): no event here, so
            // hold the base colour. The input path overrides per event.
            Effect::ReactiveType | Effect::AudioReact | Effect::GameLink => dim(base),
        }
    }
}

/// 64-byte fixed ABI struct returned by RGB_QUERY.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct DeviceAbi {
    pub version: u32, // = 1
    pub id: u64,
    pub kind: u32,
    pub zones: u32,
    pub brightness: u32,
    pub effect: u32,
    pub effect_speed: u32,
    pub effect_color: u32,
    pub name: [u8; 24],
}

// ── Registry ───────────────────────────────────────────────────────────

struct Registry {
    devices: BTreeMap<u64, Device>,
    next_id: u64,
    /// Aggregate command counter for /proc/raeen/rgb.
    commands_total: u64,
}

impl Registry {
    fn new() -> Self {
        let mut r = Self {
            devices: BTreeMap::new(),
            next_id: 1,
            commands_total: 0,
        };
        r.seed();
        r
    }

    /// Seed with a representative gaming-PC RGB inventory so the Settings
    /// → Personalization → RGB panel has something to show before real
    /// hardware enumeration lands. Matches the device set OpenRGB would
    /// typically find on a midrange enthusiast build.
    fn seed(&mut self) {
        let inventory: &[(Kind, &str, usize)] = &[
            (Kind::Motherboard, "ASUS ROG Strix B650-E", 12),
            (Kind::Keyboard, "Keychron Q1 Pro", 87), // per-key
            (Kind::Mouse, "Logitech G Pro X Superlight", 1),
            (Kind::Headset, "SteelSeries Arctis Nova Pro", 2),
            (Kind::Ram, "G.Skill Trident Z5 (2x16)", 16),
            (Kind::Gpu, "MSI RTX 4080 SUPRIM", 8),
            (Kind::Fan, "Noctua NF-A12x25 LCS #1", 8),
            (Kind::Fan, "Noctua NF-A12x25 LCS #2", 8),
            (Kind::Fan, "Noctua NF-A12x25 LCS #3", 8),
            (Kind::AioPump, "Corsair iCUE H150i", 1),
            (Kind::LedStrip, "Phanteks NEON-LED 1m", 30),
            (Kind::Case, "Lian Li O11 Dynamic", 4),
        ];
        for (kind, name, zones) in inventory {
            let id = self.next_id;
            self.next_id += 1;
            self.devices
                .insert(id, Device::new(id, *kind, name, *zones));
        }
    }

    fn set_color(&mut self, dev_id: u64, zone: u32, color: u32, brightness: u8) -> u64 {
        let d = match self.devices.get_mut(&dev_id) {
            Some(d) => d,
            None => return ERR_NO_DEVICE,
        };
        if zone as usize == u32::MAX as usize {
            // Sentinel: set all zones.
            for c in d.zone_colors.iter_mut() {
                *c = color;
            }
        } else if (zone as usize) >= d.zone_colors.len() {
            return ERR_BAD_ZONE;
        } else {
            d.zone_colors[zone as usize] = color;
        }
        d.brightness = brightness.min(100);
        // Direct color set implies static effect.
        d.effect = Effect::Static;
        self.commands_total += 1;
        0
    }

    fn set_effect(&mut self, dev_id: u64, effect_id: u32, speed: u8, color: u32) -> u64 {
        let d = match self.devices.get_mut(&dev_id) {
            Some(d) => d,
            None => return ERR_NO_DEVICE,
        };
        let effect = match effect_id {
            0 => Effect::Static,
            1 => Effect::Breathing,
            2 => Effect::Rainbow,
            3 => Effect::ColorCycle,
            4 => Effect::Pulse,
            5 => Effect::Wave,
            6 => Effect::ReactiveType,
            7 => Effect::AudioReact,
            8 => Effect::GameLink,
            _ => return ERR_BAD_EFFECT,
        };
        d.effect = effect;
        d.effect_speed = speed.min(100);
        d.effect_color = color;
        self.commands_total += 1;
        0
    }
}

static REGISTRY: Mutex<Option<Registry>> = Mutex::new(None);

// ── Error codes ────────────────────────────────────────────────────────

pub const ERR_NOT_INIT: u64 = 0xFFFF_FFFF_FFFF_FD01;
pub const ERR_NO_DEVICE: u64 = 0xFFFF_FFFF_FFFF_FD02;
pub const ERR_BAD_ZONE: u64 = 0xFFFF_FFFF_FFFF_FD03;
pub const ERR_BAD_EFFECT: u64 = 0xFFFF_FFFF_FFFF_FD04;
pub const ERR_BAD_USER: u64 = 0xFFFF_FFFF_FFFF_FD05;

// ── Boot init ──────────────────────────────────────────────────────────

pub fn init() {
    let reg = Registry::new();
    let n = reg.devices.len();
    let total_zones: usize = reg.devices.values().map(|d| d.zone_colors.len()).sum();
    *REGISTRY.lock() = Some(reg);
    crate::serial_println!(
        "[ OK ] Unified RGB: {} device(s), {} zone(s) under one kernel API",
        n,
        total_zones,
    );
}

// ── Syscall handlers ───────────────────────────────────────────────────

pub const SYS_RGB_LIST: u64 = 62;
pub const SYS_RGB_QUERY: u64 = 63;
pub const SYS_RGB_SET: u64 = 64;
pub const SYS_RGB_EFFECT: u64 = 65;

/// Output: 16 bytes per device (u64 id, u32 kind, u32 zones).
pub fn sys_list(out_ptr: u64, out_cap: u64, validate_w: impl Fn(u64, u64, bool) -> bool) -> u64 {
    if out_cap > 0 && !validate_w(out_ptr, out_cap, true) {
        return 0;
    }
    let g = REGISTRY.lock();
    let reg = match g.as_ref() {
        Some(r) => r,
        None => return 0,
    };
    let max = (out_cap / 16) as usize;
    let n = reg.devices.len().min(max);
    // SMAP-safe: assemble kernel-side, one validated extable copy-out.
    let mut out: alloc::vec::Vec<u8> = alloc::vec::Vec::with_capacity(n * 16);
    for (id, dev) in reg.devices.iter().take(n) {
        out.extend_from_slice(&id.to_le_bytes());
        out.extend_from_slice(&(dev.kind as u32).to_le_bytes());
        out.extend_from_slice(&(dev.zone_colors.len() as u32).to_le_bytes());
    }
    if crate::uaccess::copy_to_user(out_ptr, &out).is_err() {
        return 0;
    }
    n as u64
}

pub fn sys_query(
    dev_id: u64,
    out_ptr: u64,
    out_cap: u64,
    validate_w: impl Fn(u64, u64, bool) -> bool,
) -> u64 {
    let size = core::mem::size_of::<DeviceAbi>() as u64;
    if out_cap < size {
        return u64::MAX;
    }
    if !validate_w(out_ptr, size, true) {
        return u64::MAX;
    }
    let g = REGISTRY.lock();
    let reg = match g.as_ref() {
        Some(r) => r,
        None => return u64::MAX,
    };
    let dev = match reg.devices.get(&dev_id) {
        Some(d) => d,
        None => return u64::MAX,
    };
    let mut name_buf = [0u8; 24];
    let nb = dev.name.as_bytes();
    let n = nb.len().min(24);
    name_buf[..n].copy_from_slice(&nb[..n]);
    // SMAP-safe: serialize the repr(C) DeviceAbi into a 64-byte buffer matching
    // its exact layout (version, 4B pad, id@8, then the u32 tail + name@40) and
    // one validated extable copy-out. Manual packing avoids exposing the struct
    // padding byte and any raw user-ptr deref.
    debug_assert_eq!(size as usize, 64);
    let mut buf = [0u8; 64];
    buf[0..4].copy_from_slice(&1u32.to_le_bytes()); // version
    buf[8..16].copy_from_slice(&dev.id.to_le_bytes());
    buf[16..20].copy_from_slice(&(dev.kind as u32).to_le_bytes());
    buf[20..24].copy_from_slice(&(dev.zone_colors.len() as u32).to_le_bytes());
    buf[24..28].copy_from_slice(&(dev.brightness as u32).to_le_bytes());
    buf[28..32].copy_from_slice(&(dev.effect as u32).to_le_bytes());
    buf[32..36].copy_from_slice(&(dev.effect_speed as u32).to_le_bytes());
    buf[36..40].copy_from_slice(&dev.effect_color.to_le_bytes());
    buf[40..64].copy_from_slice(&name_buf);
    if crate::uaccess::copy_to_user(out_ptr, &buf).is_err() {
        return u64::MAX;
    }
    size
}

pub fn sys_set(dev_id: u64, zone: u64, color: u64, brightness: u64) -> u64 {
    let mut g = REGISTRY.lock();
    match g.as_mut() {
        Some(r) => r.set_color(dev_id, zone as u32, color as u32, brightness as u8),
        None => ERR_NOT_INIT,
    }
}

pub fn sys_effect(dev_id: u64, effect_id: u64, speed: u64, color: u64) -> u64 {
    let mut g = REGISTRY.lock();
    match g.as_mut() {
        Some(r) => r.set_effect(dev_id, effect_id as u32, speed as u8, color as u32),
        None => ERR_NOT_INIT,
    }
}

// ── /proc/raeen/rgb ────────────────────────────────────────────────────

pub fn dump_text() -> String {
    let g = REGISTRY.lock();
    let reg = match g.as_ref() {
        Some(r) => r,
        None => return String::from("# rgb registry not initialized\n"),
    };
    let mut out = String::new();
    let total_zones: usize = reg.devices.values().map(|d| d.zone_colors.len()).sum();
    out.push_str(&alloc::format!(
        "# RaeenOS unified RGB ({} devices, {} zones, {} commands since boot)\n",
        reg.devices.len(),
        total_zones,
        reg.commands_total,
    ));
    for (id, d) in &reg.devices {
        out.push_str(&alloc::format!(
            "\ndev{} kind={} zones={} brightness={} effect={:?} effect_speed={} effect_color=0x{:08x}\n  name = \"{}\"\n  zone_colors = [",
            id, d.kind.label(), d.zone_colors.len(),
            d.brightness, d.effect, d.effect_speed, d.effect_color,
            d.name,
        ));
        let mut first = true;
        for (i, c) in d.zone_colors.iter().enumerate() {
            if !first {
                out.push_str(", ");
            }
            first = false;
            if i > 0 && i % 8 == 0 {
                out.push_str("\n                ");
            }
            out.push_str(&alloc::format!("0x{:08x}", c));
        }
        out.push_str("]\n");
    }
    out
}

// ── Boot smoketest ─────────────────────────────────────────────────────

pub fn run_boot_smoketest() {
    // Paint everything RaeBlue, then kick the keyboard into per-key
    // reactive-type effect so the boot log shows the API works.
    let device_ids: Vec<u64> = {
        let g = REGISTRY.lock();
        g.as_ref()
            .map(|r| r.devices.keys().copied().collect())
            .unwrap_or_default()
    };
    let raeblue = 0xFF_4E_9C_FF;
    let mut painted = 0;
    for id in &device_ids {
        if sys_set(*id, u32::MAX as u64, raeblue, 80) == 0 {
            painted += 1;
        }
    }
    // Pick the keyboard (kind == 2) and put it on reactive-type.
    let kbd_id = {
        let g = REGISTRY.lock();
        g.as_ref().and_then(|r| {
            r.devices
                .iter()
                .find(|(_, d)| d.kind == Kind::Keyboard)
                .map(|(id, _)| *id)
        })
    };
    if let Some(kid) = kbd_id {
        sys_effect(kid, Effect::ReactiveType as u64, 70, raeblue);
    }
    crate::serial_println!(
        "[rgb] smoketest: painted {} device(s) RaeBlue, keyboard set to reactive-type",
        painted,
    );

    run_effect_smoketest();
}

/// Verify the effect animation math (Phase 13 — breathing/wave/rainbow). Pure
/// integer functions, so this proves the per-frame colour computation works
/// without any LED hardware.
pub fn run_effect_smoketest() {
    let base = 0xFF_4E_9C_FF; // RaeBlue

    // HSV sanity: hue 0 full sat/val is pure red.
    let red = hsv_to_argb(0, 255, 255);
    let hsv_ok = (red & 0x00FF_FFFF) == 0x00FF_0000;

    // Static: full brightness keeps the colour; zero brightness is black.
    let static_ok = Effect::Static.frame_color(base, 0, 0, 1, 50, 100) == base
        && (Effect::Static.frame_color(base, 0, 0, 1, 50, 0) & 0x00FF_FFFF) == 0;

    // Breathing animates: the trough (t=0) is darker than the crest (t=period/2).
    let period = breathing_period_ms(50);
    let trough = Effect::Breathing.frame_color(base, 0, 0, 1, 50, 100);
    let crest = Effect::Breathing.frame_color(base, period / 2, 0, 1, 50, 100);
    let breathing_ok = (trough & 0x00FF_FFFF) < (crest & 0x00FF_FFFF);

    // Wave: at the same instant, different zones show different colours (phase).
    let z0 = Effect::Wave.frame_color(base, 1000, 0, 8, 50, 100);
    let z4 = Effect::Wave.frame_color(base, 1000, 4, 8, 50, 100);
    let wave_ok = z0 != z4;

    // Rainbow cycles over time.
    let r_t0 = Effect::Rainbow.frame_color(base, 0, 0, 1, 80, 100);
    let r_t1 = Effect::Rainbow.frame_color(base, 2000, 0, 1, 80, 100);
    let rainbow_ok = r_t0 != r_t1;

    let pass = hsv_ok && static_ok && breathing_ok && wave_ok && rainbow_ok;
    crate::serial_println!(
        "[rgb] effect smoketest: hsv={} static={} breathing={} wave={} rainbow={} -> {}",
        hsv_ok,
        static_ok,
        breathing_ok,
        wave_ok,
        rainbow_ok,
        if pass { "PASS" } else { "FAIL" },
    );
}
