//! Vibe Mode — one-tap aesthetic presets (Concept §Customization Engine:
//! "Vibe Mode presets — Cyberpunk Night, Studio Ghibli Morning, Bauhaus —
//! wallpaper, accent colors, sound design, fonts, cursor, animations, all
//! switched together; the desktop becomes a different place in one tap").
//! MasterChecklist Phase 13.1 — "Vibe Mode presets" + "Vibe Mode includes:
//! wallpaper, accent colors, sound design, fonts, cursor, animations".
//!
//! A preset is a BUNDLE binding the subsystems that already exist: a theme
//! (`theme_engine` carries accent/fonts/cursor/animation curves/scanlines/
//! particles), a live wallpaper (`live_wallpaper`), a named sound pack
//! (AthAudio asset — id recorded now, audible on iron when Phase 7 PCM
//! lands), and the RGB accent the peripheral sync mirrors. [`apply_preset`]
//! switches everything in one call; presets bind by NAME so id reshuffles
//! across boots can't mismatch them.
//!
//! The smoketest applies two presets through the LIVE engines and verifies
//! the theme + wallpaper really switched (and restores the user's choice).

#![allow(dead_code)]

extern crate alloc;

use alloc::string::String;
use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;

/// One Vibe Mode preset: everything that switches together.
pub struct VibePreset {
    pub name: &'static str,
    pub theme: &'static str,
    pub wallpaper: &'static str,
    pub sound_pack: &'static str,
    pub rgb_accent_argb: u32,
}

/// The shipped presets. Themes/wallpapers reference the signed builtins in
/// `theme_engine`/`live_wallpaper` by display name.
pub static PRESETS: [VibePreset; 5] = [
    VibePreset {
        name: "Cyberpunk Night",
        theme: "Cyberpunk Night",
        wallpaper: "Cyberpunk Rain Shader",
        sound_pack: "synthwave-rain",
        rgb_accent_argb: 0xFF_FF_2A_88,
    },
    VibePreset {
        name: "Studio Ghibli Morning",
        theme: "Studio Ghibli Morning",
        wallpaper: "Cherry Blossom Drift",
        sound_pack: "forest-morning",
        rgb_accent_argb: 0xFF_C8_E0_94,
    },
    VibePreset {
        name: "Bauhaus",
        theme: "Bauhaus",
        wallpaper: "RaeBlue Solid",
        sound_pack: "minimal-click",
        rgb_accent_argb: 0xFF_E5_3A_2C,
    },
    VibePreset {
        name: "Neo-noir",
        theme: "Neo-noir",
        wallpaper: "Tokyo Neon Skyline",
        sound_pack: "rain-on-glass",
        rgb_accent_argb: 0xFF_E8_B5_4B,
    },
    VibePreset {
        name: "Holographic",
        theme: "Holographic",
        wallpaper: "Liquid Glass",
        sound_pack: "glass-chimes",
        rgb_accent_argb: 0xFF_88_FF_FF,
    },
];

/// Name of the active preset (None until the user picks one — individual
/// theme/wallpaper choices outside Vibe Mode stay untouched).
static ACTIVE: Mutex<Option<&'static VibePreset>> = Mutex::new(None);
static APPLIES: AtomicU64 = AtomicU64::new(0);

/// Apply a preset by name: theme + wallpaper switch through their engines,
/// the sound pack + RGB accent are recorded for AthAudio / peripheral sync.
/// Returns false (and changes nothing) for an unknown preset or when a
/// bound theme/wallpaper name doesn't resolve — fail-closed, no half-vibe.
pub fn apply_preset(name: &str) -> bool {
    let Some(preset) = PRESETS.iter().find(|p| p.name == name) else {
        return false;
    };
    // Resolve BOTH targets before touching either — no partial switch.
    let Some(theme_id) = crate::theme_engine::find_by_name(preset.theme) else {
        crate::serial_println!("[vibe] preset \"{}\": theme not found", name);
        return false;
    };
    let Some(wp_id) = crate::live_wallpaper::find_by_name(preset.wallpaper) else {
        crate::serial_println!("[vibe] preset \"{}\": wallpaper not found", name);
        return false;
    };
    if crate::theme_engine::apply(theme_id) != 0 {
        return false;
    }
    if crate::live_wallpaper::set_current(wp_id) != 0 {
        return false;
    }
    *ACTIVE.lock() = Some(preset);
    APPLIES.fetch_add(1, Ordering::Relaxed);
    crate::serial_println!(
        "[vibe] \"{}\" applied: theme+wallpaper switched, sound_pack={}, rgb_accent={:#010x}",
        preset.name,
        preset.sound_pack,
        preset.rgb_accent_argb,
    );
    true
}

/// The active preset's RGB accent for peripheral sync (None outside Vibe Mode).
pub fn active_rgb_accent() -> Option<u32> {
    ACTIVE.lock().map(|p| p.rgb_accent_argb)
}

pub fn init() {
    crate::serial_println!(
        "[ OK ] Vibe Mode: {} preset(s) (theme+wallpaper+sound+RGB switch as one)",
        PRESETS.len(),
    );
}

/// Deterministic proof: two presets applied through the LIVE theme +
/// wallpaper engines, each switch verified against both engines' current
/// state, fail-closed on an unknown preset, user's choices restored.
pub fn run_boot_smoketest() {
    let saved_theme = crate::theme_engine::current_id();
    let saved_wp = crate::live_wallpaper::current_id();

    let mut check = |preset_name: &str, theme_name: &str, wp_name: &str| -> bool {
        if !apply_preset(preset_name) {
            return false;
        }
        let theme_ok =
            crate::theme_engine::current_id() == crate::theme_engine::find_by_name(theme_name);
        let wp_ok =
            crate::live_wallpaper::current_id() == crate::live_wallpaper::find_by_name(wp_name);
        theme_ok && wp_ok
    };

    let cyberpunk = check(
        "Cyberpunk Night",
        "Cyberpunk Night",
        "Cyberpunk Rain Shader",
    );
    let ghibli = check(
        "Studio Ghibli Morning",
        "Studio Ghibli Morning",
        "Cherry Blossom Drift",
    );
    let reject = !apply_preset("Vaporwave Basement"); // not shipped

    // Restore the user's pre-test state.
    if let Some(id) = saved_theme {
        let _ = crate::theme_engine::apply(id);
    }
    if let Some(id) = saved_wp {
        let _ = crate::live_wallpaper::set_current(id);
    }
    *ACTIVE.lock() = None;

    let pass = cyberpunk && ghibli && reject;
    crate::serial_println!(
        "[vibe] smoketest: cyberpunk_bundle={} ghibli_bundle={} reject_unknown={} -> {}",
        cyberpunk,
        ghibli,
        reject,
        if pass { "PASS" } else { "FAIL" },
    );
}

/// `/proc/athena/vibe` — Vibe Mode state.
pub fn dump_text() -> String {
    let active = ACTIVE
        .lock()
        .map(|p| p.name)
        .unwrap_or("(none — custom theme/wallpaper)");
    let mut out = alloc::format!(
        "# Vibe Mode (one-tap aesthetic bundles)\nactive: {}\napplies: {}\n",
        active,
        APPLIES.load(Ordering::Relaxed),
    );
    for p in PRESETS.iter() {
        out.push_str(&alloc::format!(
            "preset: {} (theme={}, wallpaper={}, sound={}, rgb={:#010x})\n",
            p.name,
            p.theme,
            p.wallpaper,
            p.sound_pack,
            p.rgb_accent_argb,
        ));
    }
    out
}
