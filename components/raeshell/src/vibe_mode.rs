//! Vibe Mode — system-wide visual personalities for AthenaOS.
//!
//! A `VibeProfile` is a declarative data bundle describing wallpaper,
//! accent colours, sound design, fonts, cursor theme, animation curves,
//! and icon style.  Vibes are pure data — no code, fully serialisable,
//! sharable, and sandboxed.
//!
//! The `VibeEngine` manages the active profile, built-in presets,
//! user-created vibes, and time-based auto-switching.

#![allow(dead_code)]

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

// ── Colour type ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 0xFF }
    }

    pub const fn from_argb(argb: u32) -> Self {
        Self {
            a: ((argb >> 24) & 0xFF) as u8,
            r: ((argb >> 16) & 0xFF) as u8,
            g: ((argb >> 8) & 0xFF) as u8,
            b: (argb & 0xFF) as u8,
        }
    }

    pub const fn to_argb(self) -> u32 {
        ((self.a as u32) << 24) | ((self.r as u32) << 16) | ((self.g as u32) << 8) | (self.b as u32)
    }

    pub fn lerp(self, other: Self, t_256: u16) -> Self {
        let inv = 256u16.saturating_sub(t_256);
        Self {
            r: ((self.r as u16 * inv + other.r as u16 * t_256) >> 8) as u8,
            g: ((self.g as u16 * inv + other.g as u16 * t_256) >> 8) as u8,
            b: ((self.b as u16 * inv + other.b as u16 * t_256) >> 8) as u8,
            a: ((self.a as u16 * inv + other.a as u16 * t_256) >> 8) as u8,
        }
    }
}

// ── Accent colour palette ────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AccentPalette {
    pub primary: Color,
    pub secondary: Color,
    pub tertiary: Color,
    pub surface: Color,
    pub on_surface: Color,
    pub error: Color,
    pub warning: Color,
    pub success: Color,
}

// ── Sound design ─────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct SoundDesign {
    pub click_sound: String,
    pub notify_sound: String,
    pub startup_sound: String,
    pub error_sound: String,
    pub volume_scale: u8,
    pub ambient_enabled: bool,
    pub ambient_track: String,
}

impl SoundDesign {
    pub fn silent() -> Self {
        Self {
            click_sound: String::new(),
            notify_sound: String::new(),
            startup_sound: String::new(),
            error_sound: String::new(),
            volume_scale: 0,
            ambient_enabled: false,
            ambient_track: String::new(),
        }
    }
}

// ── Cursor theme ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorStyle {
    Default,
    Minimal,
    Neon,
    Retro,
    Geometric,
    Organic,
    Custom,
}

#[derive(Debug, Clone)]
pub struct CursorTheme {
    pub style: CursorStyle,
    pub color: Color,
    pub trail: bool,
    pub size_scale: u8,
}

// ── Icon style ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconStyle {
    Outlined,
    Filled,
    Rounded,
    Flat,
    Glassmorphic,
    Pixel,
    Duotone,
}

// ── Window animation style ───────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnimationStyle {
    Smooth,
    Snappy,
    Bouncy,
    Slide,
    Fade,
    Scale,
    Glitch,
    None,
}

#[derive(Debug, Clone, Copy)]
pub struct AnimationConfig {
    pub open_style: AnimationStyle,
    pub close_style: AnimationStyle,
    pub minimize_style: AnimationStyle,
    pub maximize_style: AnimationStyle,
    pub duration_ms: u32,
    pub overshoot: u8,
}

impl AnimationConfig {
    pub const fn default_config() -> Self {
        Self {
            open_style: AnimationStyle::Smooth,
            close_style: AnimationStyle::Fade,
            minimize_style: AnimationStyle::Scale,
            maximize_style: AnimationStyle::Smooth,
            duration_ms: 250,
            overshoot: 0,
        }
    }
}

// ── Font config ──────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct FontConfig {
    pub system_font: String,
    pub monospace_font: String,
    pub heading_font: String,
    pub base_size_pt: u8,
    pub weight: u16,
}

impl FontConfig {
    pub fn default_fonts() -> Self {
        Self {
            system_font: String::from("RaeenSans"),
            monospace_font: String::from("RaeenMono"),
            heading_font: String::from("RaeenSans"),
            base_size_pt: 14,
            weight: 400,
        }
    }
}

// ── Wallpaper config ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WallpaperMode {
    Static,
    LiveShader,
    SlideShow,
    Gradient,
    SolidColor,
}

#[derive(Debug, Clone)]
pub struct WallpaperConfig {
    pub mode: WallpaperMode,
    pub path: String,
    pub shader_name: String,
    pub gradient_start: Color,
    pub gradient_end: Color,
    pub solid_color: Color,
    pub slideshow_interval_secs: u32,
}

impl WallpaperConfig {
    pub fn solid(color: Color) -> Self {
        Self {
            mode: WallpaperMode::SolidColor,
            path: String::new(),
            shader_name: String::new(),
            gradient_start: color,
            gradient_end: color,
            solid_color: color,
            slideshow_interval_secs: 0,
        }
    }

    pub fn gradient(start: Color, end: Color) -> Self {
        Self {
            mode: WallpaperMode::Gradient,
            path: String::new(),
            shader_name: String::new(),
            gradient_start: start,
            gradient_end: end,
            solid_color: start,
            slideshow_interval_secs: 0,
        }
    }
}

// ── Compositor theme overrides ───────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub struct CompositorTheme {
    pub window_bg: u32,
    pub chrome_bg: u32,
    pub border_color: u32,
    pub text_fg: u32,
    pub title_fg: u32,
    pub button_bg: u32,
    pub button_hot: u32,
    pub taskbar_bg: u32,
    pub taskbar_border: u32,
    pub menu_bg: u32,
    pub menu_hover: u32,
    pub blur_radius: u8,
    pub corner_radius: u8,
    pub shadow_opacity: u8,
}

// ── The complete Vibe Profile ────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Ord, PartialOrd)]
pub enum VibePreset {
    CyberpunkNight,
    StudioGhibliMorning,
    Bauhaus,
    NeoNoir,
    NordicFrost,
    RetroWave,
    MinimalZen,
    ForestDusk,
    OceanBreeze,
    SolarPunk,
    MidnightAbyss,
    SakuraDawn,
}

#[derive(Debug, Clone)]
pub struct VibeProfile {
    pub name: String,
    pub description: String,
    pub author: String,
    pub version: u32,

    pub accent: AccentPalette,
    pub wallpaper: WallpaperConfig,
    pub sounds: SoundDesign,
    pub cursor: CursorTheme,
    pub icon_style: IconStyle,
    pub animations: AnimationConfig,
    pub fonts: FontConfig,
    pub compositor: CompositorTheme,

    pub metadata: BTreeMap<String, String>,
}

impl VibeProfile {
    fn new_named(name: &str, desc: &str) -> Self {
        Self {
            name: String::from(name),
            description: String::from(desc),
            author: String::from("AthenaOS"),
            version: 1,
            accent: default_accent(),
            wallpaper: WallpaperConfig::solid(Color::rgb(0x0A, 0x0E, 0x1A)),
            sounds: SoundDesign::silent(),
            cursor: CursorTheme {
                style: CursorStyle::Default,
                color: Color::rgb(0xFF, 0xFF, 0xFF),
                trail: false,
                size_scale: 100,
            },
            icon_style: IconStyle::Outlined,
            animations: AnimationConfig::default_config(),
            fonts: FontConfig::default_fonts(),
            compositor: default_compositor_theme(),
            metadata: BTreeMap::new(),
        }
    }
}

fn default_accent() -> AccentPalette {
    AccentPalette {
        primary: Color::rgb(0x4E, 0x9C, 0xFF),
        secondary: Color::rgb(0xFF, 0x2E, 0x88),
        tertiary: Color::rgb(0x00, 0xD4, 0xAA),
        surface: Color::rgb(0x1A, 0x1A, 0x22),
        on_surface: Color::rgb(0xE0, 0xE0, 0xFF),
        error: Color::rgb(0xFF, 0x44, 0x44),
        warning: Color::rgb(0xFF, 0xAA, 0x33),
        success: Color::rgb(0x44, 0xFF, 0x88),
    }
}

fn default_compositor_theme() -> CompositorTheme {
    CompositorTheme {
        window_bg: 0xFF_1A_1A_22,
        chrome_bg: 0xFF_0A_0E_1A,
        border_color: 0xFF_4E_9C_FF,
        text_fg: 0xFF_E0_E0_FF,
        title_fg: 0xFF_FF_FF_FF,
        button_bg: 0xFF_33_33_55,
        button_hot: 0xFF_FF_2E_88,
        taskbar_bg: 0xFF_0A_0E_1A,
        taskbar_border: 0xFF_4E_9C_FF,
        menu_bg: 0xFF_12_14_20,
        menu_hover: 0xFF_28_2C_44,
        blur_radius: 12,
        corner_radius: 8,
        shadow_opacity: 40,
    }
}

// ── Built-in preset constructors ─────────────────────────────────────────

pub fn preset_cyberpunk_night() -> VibeProfile {
    let mut v = VibeProfile::new_named("Cyberpunk Night", "Neon-soaked, dark, electric");
    v.accent = AccentPalette {
        primary: Color::rgb(0x00, 0xFF, 0xE5),
        secondary: Color::rgb(0xFF, 0x00, 0x7F),
        tertiary: Color::rgb(0xBB, 0x00, 0xFF),
        surface: Color::rgb(0x0D, 0x0D, 0x1A),
        on_surface: Color::rgb(0xC0, 0xFF, 0xF0),
        error: Color::rgb(0xFF, 0x22, 0x22),
        warning: Color::rgb(0xFF, 0x88, 0x00),
        success: Color::rgb(0x00, 0xFF, 0x66),
    };
    v.wallpaper =
        WallpaperConfig::gradient(Color::rgb(0x05, 0x05, 0x10), Color::rgb(0x1A, 0x00, 0x33));
    v.cursor = CursorTheme {
        style: CursorStyle::Neon,
        color: Color::rgb(0x00, 0xFF, 0xE5),
        trail: true,
        size_scale: 100,
    };
    v.icon_style = IconStyle::Glassmorphic;
    v.animations = AnimationConfig {
        open_style: AnimationStyle::Glitch,
        close_style: AnimationStyle::Fade,
        minimize_style: AnimationStyle::Scale,
        maximize_style: AnimationStyle::Snappy,
        duration_ms: 200,
        overshoot: 10,
    };
    v.compositor = CompositorTheme {
        window_bg: 0xFF_0D_0D_1A,
        chrome_bg: 0xFF_05_05_10,
        border_color: 0xFF_00_FF_E5,
        text_fg: 0xFF_C0_FF_F0,
        title_fg: 0xFF_00_FF_E5,
        button_bg: 0xFF_1A_00_33,
        button_hot: 0xFF_FF_00_7F,
        taskbar_bg: 0xFF_05_05_10,
        taskbar_border: 0xFF_00_FF_E5,
        menu_bg: 0xFF_0A_0A_15,
        menu_hover: 0xFF_1A_00_33,
        blur_radius: 16,
        corner_radius: 4,
        shadow_opacity: 60,
    };
    v
}

pub fn preset_studio_ghibli_morning() -> VibeProfile {
    let mut v = VibeProfile::new_named("Studio Ghibli Morning", "Warm, whimsical, hand-painted");
    v.accent = AccentPalette {
        primary: Color::rgb(0x6B, 0x8F, 0x71),
        secondary: Color::rgb(0xE8, 0x6B, 0x5A),
        tertiary: Color::rgb(0xF5, 0xC6, 0x4F),
        surface: Color::rgb(0xF5, 0xF0, 0xE1),
        on_surface: Color::rgb(0x3A, 0x3A, 0x2E),
        error: Color::rgb(0xCC, 0x44, 0x33),
        warning: Color::rgb(0xDD, 0x99, 0x22),
        success: Color::rgb(0x55, 0x99, 0x55),
    };
    v.wallpaper =
        WallpaperConfig::gradient(Color::rgb(0xE8, 0xDD, 0xCC), Color::rgb(0x87, 0xCE, 0xAA));
    v.cursor = CursorTheme {
        style: CursorStyle::Organic,
        color: Color::rgb(0x6B, 0x8F, 0x71),
        trail: false,
        size_scale: 110,
    };
    v.icon_style = IconStyle::Rounded;
    v.animations = AnimationConfig {
        open_style: AnimationStyle::Bouncy,
        close_style: AnimationStyle::Fade,
        minimize_style: AnimationStyle::Slide,
        maximize_style: AnimationStyle::Smooth,
        duration_ms: 350,
        overshoot: 15,
    };
    v.compositor = CompositorTheme {
        window_bg: 0xFF_F5_F0_E1,
        chrome_bg: 0xFF_6B_8F_71,
        border_color: 0xFF_8B_6F_47,
        text_fg: 0xFF_3A_3A_2E,
        title_fg: 0xFF_FF_FF_F0,
        button_bg: 0xFF_A8_C6_8F,
        button_hot: 0xFF_E8_6B_5A,
        taskbar_bg: 0xFF_6B_8F_71,
        taskbar_border: 0xFF_8B_6F_47,
        menu_bg: 0xFF_E8_E0_D0,
        menu_hover: 0xFF_D5_CC_BB,
        blur_radius: 8,
        corner_radius: 12,
        shadow_opacity: 20,
    };
    v.fonts = FontConfig {
        system_font: String::from("RaeenRound"),
        monospace_font: String::from("RaeenMono"),
        heading_font: String::from("RaeenRound"),
        base_size_pt: 15,
        weight: 400,
    };
    v
}

pub fn preset_bauhaus() -> VibeProfile {
    let mut v = VibeProfile::new_named("Bauhaus", "Primary colours, geometric, functional");
    v.accent = AccentPalette {
        primary: Color::rgb(0xE3, 0x1B, 0x23),
        secondary: Color::rgb(0x00, 0x56, 0xA4),
        tertiary: Color::rgb(0xF7, 0xC6, 0x00),
        surface: Color::rgb(0xF2, 0xF2, 0xF2),
        on_surface: Color::rgb(0x1A, 0x1A, 0x1A),
        error: Color::rgb(0xCC, 0x00, 0x00),
        warning: Color::rgb(0xF7, 0xC6, 0x00),
        success: Color::rgb(0x00, 0x88, 0x44),
    };
    v.wallpaper = WallpaperConfig::solid(Color::rgb(0xF2, 0xF2, 0xF2));
    v.cursor = CursorTheme {
        style: CursorStyle::Geometric,
        color: Color::rgb(0x1A, 0x1A, 0x1A),
        trail: false,
        size_scale: 100,
    };
    v.icon_style = IconStyle::Flat;
    v.animations = AnimationConfig {
        open_style: AnimationStyle::Snappy,
        close_style: AnimationStyle::Snappy,
        minimize_style: AnimationStyle::Scale,
        maximize_style: AnimationStyle::Snappy,
        duration_ms: 150,
        overshoot: 0,
    };
    v.compositor = CompositorTheme {
        window_bg: 0xFF_F2_F2_F2,
        chrome_bg: 0xFF_1A_1A_1A,
        border_color: 0xFF_E3_1B_23,
        text_fg: 0xFF_1A_1A_1A,
        title_fg: 0xFF_FF_FF_FF,
        button_bg: 0xFF_00_56_A4,
        button_hot: 0xFF_F7_C6_00,
        taskbar_bg: 0xFF_1A_1A_1A,
        taskbar_border: 0xFF_E3_1B_23,
        menu_bg: 0xFF_E8_E8_E8,
        menu_hover: 0xFF_D0_D0_D0,
        blur_radius: 0,
        corner_radius: 0,
        shadow_opacity: 15,
    };
    v.fonts = FontConfig {
        system_font: String::from("RaeenGeo"),
        monospace_font: String::from("RaeenMono"),
        heading_font: String::from("RaeenGeo"),
        base_size_pt: 14,
        weight: 500,
    };
    v
}

pub fn preset_neo_noir() -> VibeProfile {
    let mut v = VibeProfile::new_named("Neo Noir", "Dark, moody, cinematic shadows");
    v.accent = AccentPalette {
        primary: Color::rgb(0xCC, 0x00, 0x00),
        secondary: Color::rgb(0x80, 0x80, 0x80),
        tertiary: Color::rgb(0x44, 0x44, 0x44),
        surface: Color::rgb(0x10, 0x10, 0x10),
        on_surface: Color::rgb(0xB0, 0xB0, 0xB0),
        error: Color::rgb(0xFF, 0x22, 0x22),
        warning: Color::rgb(0xCC, 0x88, 0x00),
        success: Color::rgb(0x44, 0xAA, 0x44),
    };
    v.wallpaper = WallpaperConfig::solid(Color::rgb(0x08, 0x08, 0x08));
    v.cursor = CursorTheme {
        style: CursorStyle::Minimal,
        color: Color::rgb(0xCC, 0xCC, 0xCC),
        trail: false,
        size_scale: 90,
    };
    v.icon_style = IconStyle::Outlined;
    v.animations = AnimationConfig {
        open_style: AnimationStyle::Fade,
        close_style: AnimationStyle::Fade,
        minimize_style: AnimationStyle::Fade,
        maximize_style: AnimationStyle::Smooth,
        duration_ms: 300,
        overshoot: 0,
    };
    v.compositor = CompositorTheme {
        window_bg: 0xFF_10_10_10,
        chrome_bg: 0xFF_08_08_08,
        border_color: 0xFF_44_44_44,
        text_fg: 0xFF_B0_B0_B0,
        title_fg: 0xFF_D0_D0_D0,
        button_bg: 0xFF_22_22_22,
        button_hot: 0xFF_CC_00_00,
        taskbar_bg: 0xFF_08_08_08,
        taskbar_border: 0xFF_33_33_33,
        menu_bg: 0xFF_0C_0C_0C,
        menu_hover: 0xFF_1A_1A_1A,
        blur_radius: 4,
        corner_radius: 2,
        shadow_opacity: 70,
    };
    v
}

pub fn preset_nordic_frost() -> VibeProfile {
    let mut v = VibeProfile::new_named("Nordic Frost", "Cool blues, icy whites, calm and clean");
    v.accent = AccentPalette {
        primary: Color::rgb(0x5E, 0x81, 0xAC),
        secondary: Color::rgb(0x88, 0xC0, 0xD0),
        tertiary: Color::rgb(0x81, 0xA1, 0xC1),
        surface: Color::rgb(0xEC, 0xEF, 0xF4),
        on_surface: Color::rgb(0x2E, 0x34, 0x40),
        error: Color::rgb(0xBF, 0x61, 0x6A),
        warning: Color::rgb(0xEB, 0xCB, 0x8B),
        success: Color::rgb(0xA3, 0xBE, 0x8C),
    };
    v.wallpaper =
        WallpaperConfig::gradient(Color::rgb(0xD8, 0xDE, 0xE9), Color::rgb(0xEC, 0xEF, 0xF4));
    v.cursor = CursorTheme {
        style: CursorStyle::Minimal,
        color: Color::rgb(0x2E, 0x34, 0x40),
        trail: false,
        size_scale: 100,
    };
    v.icon_style = IconStyle::Rounded;
    v.animations = AnimationConfig {
        open_style: AnimationStyle::Smooth,
        close_style: AnimationStyle::Fade,
        minimize_style: AnimationStyle::Slide,
        maximize_style: AnimationStyle::Smooth,
        duration_ms: 280,
        overshoot: 5,
    };
    v.compositor = CompositorTheme {
        window_bg: 0xFF_EC_EF_F4,
        chrome_bg: 0xFF_D8_DE_E9,
        border_color: 0xFF_5E_81_AC,
        text_fg: 0xFF_2E_34_40,
        title_fg: 0xFF_2E_34_40,
        button_bg: 0xFF_81_A1_C1,
        button_hot: 0xFF_88_C0_D0,
        taskbar_bg: 0xFF_D8_DE_E9,
        taskbar_border: 0xFF_5E_81_AC,
        menu_bg: 0xFF_E5_E9_F0,
        menu_hover: 0xFF_D8_DE_E9,
        blur_radius: 10,
        corner_radius: 10,
        shadow_opacity: 15,
    };
    v
}

pub fn preset_retrowave() -> VibeProfile {
    let mut v = VibeProfile::new_named("RetroWave", "80s synthwave, hot pinks and chrome grids");
    v.accent = AccentPalette {
        primary: Color::rgb(0xFF, 0x6E, 0xC7),
        secondary: Color::rgb(0x7B, 0x2F, 0xFF),
        tertiary: Color::rgb(0x00, 0xE5, 0xFF),
        surface: Color::rgb(0x1A, 0x00, 0x2E),
        on_surface: Color::rgb(0xFF, 0xCC, 0xEE),
        error: Color::rgb(0xFF, 0x00, 0x44),
        warning: Color::rgb(0xFF, 0xAA, 0x00),
        success: Color::rgb(0x00, 0xFF, 0x88),
    };
    v.wallpaper =
        WallpaperConfig::gradient(Color::rgb(0x0A, 0x00, 0x1A), Color::rgb(0x2A, 0x00, 0x4A));
    v.cursor = CursorTheme {
        style: CursorStyle::Neon,
        color: Color::rgb(0xFF, 0x6E, 0xC7),
        trail: true,
        size_scale: 105,
    };
    v.icon_style = IconStyle::Glassmorphic;
    v.animations = AnimationConfig {
        open_style: AnimationStyle::Glitch,
        close_style: AnimationStyle::Slide,
        minimize_style: AnimationStyle::Scale,
        maximize_style: AnimationStyle::Bouncy,
        duration_ms: 220,
        overshoot: 20,
    };
    v.compositor = CompositorTheme {
        window_bg: 0xFF_1A_00_2E,
        chrome_bg: 0xFF_0A_00_1A,
        border_color: 0xFF_FF_6E_C7,
        text_fg: 0xFF_FF_CC_EE,
        title_fg: 0xFF_FF_6E_C7,
        button_bg: 0xFF_2A_00_4A,
        button_hot: 0xFF_00_E5_FF,
        taskbar_bg: 0xFF_0A_00_1A,
        taskbar_border: 0xFF_7B_2F_FF,
        menu_bg: 0xFF_14_00_28,
        menu_hover: 0xFF_2A_00_4A,
        blur_radius: 14,
        corner_radius: 6,
        shadow_opacity: 50,
    };
    v
}

pub fn preset_minimal_zen() -> VibeProfile {
    let mut v = VibeProfile::new_named("Minimal Zen", "Near-monochrome, breath of space");
    v.accent = AccentPalette {
        primary: Color::rgb(0x33, 0x33, 0x33),
        secondary: Color::rgb(0x88, 0x88, 0x88),
        tertiary: Color::rgb(0xCC, 0xCC, 0xCC),
        surface: Color::rgb(0xFB, 0xFB, 0xFB),
        on_surface: Color::rgb(0x22, 0x22, 0x22),
        error: Color::rgb(0xDD, 0x44, 0x44),
        warning: Color::rgb(0xCC, 0x99, 0x33),
        success: Color::rgb(0x33, 0x99, 0x55),
    };
    v.wallpaper = WallpaperConfig::solid(Color::rgb(0xFB, 0xFB, 0xFB));
    v.cursor = CursorTheme {
        style: CursorStyle::Minimal,
        color: Color::rgb(0x22, 0x22, 0x22),
        trail: false,
        size_scale: 90,
    };
    v.icon_style = IconStyle::Outlined;
    v.animations = AnimationConfig {
        open_style: AnimationStyle::Fade,
        close_style: AnimationStyle::Fade,
        minimize_style: AnimationStyle::Fade,
        maximize_style: AnimationStyle::Fade,
        duration_ms: 400,
        overshoot: 0,
    };
    v.compositor = CompositorTheme {
        window_bg: 0xFF_FB_FB_FB,
        chrome_bg: 0xFF_F0_F0_F0,
        border_color: 0xFF_DD_DD_DD,
        text_fg: 0xFF_22_22_22,
        title_fg: 0xFF_33_33_33,
        button_bg: 0xFF_E8_E8_E8,
        button_hot: 0xFF_33_33_33,
        taskbar_bg: 0xFF_F0_F0_F0,
        taskbar_border: 0xFF_DD_DD_DD,
        menu_bg: 0xFF_F5_F5_F5,
        menu_hover: 0xFF_E8_E8_E8,
        blur_radius: 0,
        corner_radius: 4,
        shadow_opacity: 8,
    };
    v.fonts = FontConfig {
        system_font: String::from("RaeenSans"),
        monospace_font: String::from("RaeenMono"),
        heading_font: String::from("RaeenSans"),
        base_size_pt: 13,
        weight: 300,
    };
    v
}

pub fn preset_forest_dusk() -> VibeProfile {
    let mut v = VibeProfile::new_named("Forest Dusk", "Deep greens and warm amber twilight");
    v.accent = AccentPalette {
        primary: Color::rgb(0x4C, 0x72, 0x4C),
        secondary: Color::rgb(0xD4, 0x8B, 0x3C),
        tertiary: Color::rgb(0x8B, 0x6F, 0x47),
        surface: Color::rgb(0x1A, 0x1E, 0x16),
        on_surface: Color::rgb(0xD4, 0xCC, 0xBB),
        error: Color::rgb(0xCC, 0x55, 0x33),
        warning: Color::rgb(0xD4, 0x8B, 0x3C),
        success: Color::rgb(0x6B, 0x99, 0x55),
    };
    v.wallpaper =
        WallpaperConfig::gradient(Color::rgb(0x0E, 0x12, 0x0A), Color::rgb(0x2A, 0x33, 0x22));
    v.cursor = CursorTheme {
        style: CursorStyle::Organic,
        color: Color::rgb(0xD4, 0x8B, 0x3C),
        trail: false,
        size_scale: 100,
    };
    v.icon_style = IconStyle::Duotone;
    v.animations = AnimationConfig {
        open_style: AnimationStyle::Smooth,
        close_style: AnimationStyle::Fade,
        minimize_style: AnimationStyle::Slide,
        maximize_style: AnimationStyle::Smooth,
        duration_ms: 320,
        overshoot: 8,
    };
    v.compositor = CompositorTheme {
        window_bg: 0xFF_1A_1E_16,
        chrome_bg: 0xFF_12_15_0E,
        border_color: 0xFF_4C_72_4C,
        text_fg: 0xFF_D4_CC_BB,
        title_fg: 0xFF_E0_D8_CC,
        button_bg: 0xFF_2A_33_22,
        button_hot: 0xFF_D4_8B_3C,
        taskbar_bg: 0xFF_12_15_0E,
        taskbar_border: 0xFF_4C_72_4C,
        menu_bg: 0xFF_16_1A_12,
        menu_hover: 0xFF_2A_33_22,
        blur_radius: 8,
        corner_radius: 6,
        shadow_opacity: 35,
    };
    v
}

pub fn preset_ocean_breeze() -> VibeProfile {
    let mut v = VibeProfile::new_named("Ocean Breeze", "Teal waters, sandy shores, coastal calm");
    v.accent = AccentPalette {
        primary: Color::rgb(0x00, 0x99, 0xAA),
        secondary: Color::rgb(0x00, 0xCC, 0xBB),
        tertiary: Color::rgb(0xDD, 0xBB, 0x88),
        surface: Color::rgb(0xF0, 0xF5, 0xF5),
        on_surface: Color::rgb(0x1A, 0x33, 0x33),
        error: Color::rgb(0xCC, 0x44, 0x44),
        warning: Color::rgb(0xDD, 0xAA, 0x44),
        success: Color::rgb(0x44, 0xAA, 0x77),
    };
    v.wallpaper =
        WallpaperConfig::gradient(Color::rgb(0xC8, 0xE6, 0xE8), Color::rgb(0xF0, 0xF5, 0xF0));
    v.cursor = CursorTheme {
        style: CursorStyle::Default,
        color: Color::rgb(0x00, 0x99, 0xAA),
        trail: false,
        size_scale: 100,
    };
    v.icon_style = IconStyle::Rounded;
    v.animations = AnimationConfig {
        open_style: AnimationStyle::Slide,
        close_style: AnimationStyle::Fade,
        minimize_style: AnimationStyle::Scale,
        maximize_style: AnimationStyle::Smooth,
        duration_ms: 300,
        overshoot: 10,
    };
    v.compositor = CompositorTheme {
        window_bg: 0xFF_F0_F5_F5,
        chrome_bg: 0xFF_C8_E0_E2,
        border_color: 0xFF_00_99_AA,
        text_fg: 0xFF_1A_33_33,
        title_fg: 0xFF_00_66_77,
        button_bg: 0xFF_B0_D8_DD,
        button_hot: 0xFF_00_CC_BB,
        taskbar_bg: 0xFF_C8_E0_E2,
        taskbar_border: 0xFF_00_99_AA,
        menu_bg: 0xFF_E0_EE_EE,
        menu_hover: 0xFF_C8_E0_E2,
        blur_radius: 10,
        corner_radius: 12,
        shadow_opacity: 15,
    };
    v
}

pub fn preset_solarpunk() -> VibeProfile {
    let mut v = VibeProfile::new_named("SolarPunk", "Optimistic green tech, warm sun, lush growth");
    v.accent = AccentPalette {
        primary: Color::rgb(0x55, 0xAA, 0x33),
        secondary: Color::rgb(0xFF, 0xBB, 0x33),
        tertiary: Color::rgb(0x33, 0x88, 0x66),
        surface: Color::rgb(0xF0, 0xF5, 0xE5),
        on_surface: Color::rgb(0x22, 0x33, 0x11),
        error: Color::rgb(0xCC, 0x44, 0x22),
        warning: Color::rgb(0xFF, 0xBB, 0x33),
        success: Color::rgb(0x55, 0xAA, 0x33),
    };
    v.wallpaper =
        WallpaperConfig::gradient(Color::rgb(0xE5, 0xF0, 0xD0), Color::rgb(0xF5, 0xF0, 0xD5));
    v.cursor = CursorTheme {
        style: CursorStyle::Organic,
        color: Color::rgb(0x55, 0xAA, 0x33),
        trail: false,
        size_scale: 100,
    };
    v.icon_style = IconStyle::Rounded;
    v.animations = AnimationConfig {
        open_style: AnimationStyle::Bouncy,
        close_style: AnimationStyle::Fade,
        minimize_style: AnimationStyle::Slide,
        maximize_style: AnimationStyle::Smooth,
        duration_ms: 280,
        overshoot: 12,
    };
    v.compositor = CompositorTheme {
        window_bg: 0xFF_F0_F5_E5,
        chrome_bg: 0xFF_D5_E8_C0,
        border_color: 0xFF_55_AA_33,
        text_fg: 0xFF_22_33_11,
        title_fg: 0xFF_33_55_11,
        button_bg: 0xFF_C0_DD_AA,
        button_hot: 0xFF_FF_BB_33,
        taskbar_bg: 0xFF_D5_E8_C0,
        taskbar_border: 0xFF_55_AA_33,
        menu_bg: 0xFF_E5_F0_D5,
        menu_hover: 0xFF_D5_E8_C0,
        blur_radius: 8,
        corner_radius: 10,
        shadow_opacity: 12,
    };
    v
}

pub fn preset_midnight_abyss() -> VibeProfile {
    let mut v = VibeProfile::new_named("Midnight Abyss", "Deep space, bioluminescent accents");
    v.accent = AccentPalette {
        primary: Color::rgb(0x44, 0x66, 0xFF),
        secondary: Color::rgb(0x00, 0xCC, 0xFF),
        tertiary: Color::rgb(0x88, 0x44, 0xFF),
        surface: Color::rgb(0x06, 0x06, 0x10),
        on_surface: Color::rgb(0xA0, 0xB0, 0xDD),
        error: Color::rgb(0xFF, 0x33, 0x55),
        warning: Color::rgb(0xFF, 0x99, 0x22),
        success: Color::rgb(0x22, 0xDD, 0x88),
    };
    v.wallpaper = WallpaperConfig::solid(Color::rgb(0x02, 0x02, 0x08));
    v.cursor = CursorTheme {
        style: CursorStyle::Neon,
        color: Color::rgb(0x44, 0x66, 0xFF),
        trail: true,
        size_scale: 100,
    };
    v.icon_style = IconStyle::Glassmorphic;
    v.animations = AnimationConfig {
        open_style: AnimationStyle::Scale,
        close_style: AnimationStyle::Fade,
        minimize_style: AnimationStyle::Scale,
        maximize_style: AnimationStyle::Smooth,
        duration_ms: 260,
        overshoot: 5,
    };
    v.compositor = CompositorTheme {
        window_bg: 0xFF_06_06_10,
        chrome_bg: 0xFF_02_02_08,
        border_color: 0xFF_44_66_FF,
        text_fg: 0xFF_A0_B0_DD,
        title_fg: 0xFF_CC_DD_FF,
        button_bg: 0xFF_0A_0A_1A,
        button_hot: 0xFF_00_CC_FF,
        taskbar_bg: 0xFF_02_02_08,
        taskbar_border: 0xFF_44_66_FF,
        menu_bg: 0xFF_04_04_0C,
        menu_hover: 0xFF_0A_0A_1A,
        blur_radius: 16,
        corner_radius: 6,
        shadow_opacity: 60,
    };
    v
}

pub fn preset_sakura_dawn() -> VibeProfile {
    let mut v = VibeProfile::new_named("Sakura Dawn", "Cherry blossoms, soft pinks, morning light");
    v.accent = AccentPalette {
        primary: Color::rgb(0xE8, 0x88, 0xA0),
        secondary: Color::rgb(0xF0, 0xB0, 0xC0),
        tertiary: Color::rgb(0xAA, 0x77, 0x88),
        surface: Color::rgb(0xFC, 0xF5, 0xF5),
        on_surface: Color::rgb(0x44, 0x22, 0x33),
        error: Color::rgb(0xCC, 0x44, 0x55),
        warning: Color::rgb(0xDD, 0xAA, 0x55),
        success: Color::rgb(0x77, 0xAA, 0x77),
    };
    v.wallpaper =
        WallpaperConfig::gradient(Color::rgb(0xFC, 0xEE, 0xEE), Color::rgb(0xF5, 0xF0, 0xF5));
    v.cursor = CursorTheme {
        style: CursorStyle::Default,
        color: Color::rgb(0xE8, 0x88, 0xA0),
        trail: false,
        size_scale: 100,
    };
    v.icon_style = IconStyle::Rounded;
    v.animations = AnimationConfig {
        open_style: AnimationStyle::Bouncy,
        close_style: AnimationStyle::Fade,
        minimize_style: AnimationStyle::Scale,
        maximize_style: AnimationStyle::Smooth,
        duration_ms: 320,
        overshoot: 18,
    };
    v.compositor = CompositorTheme {
        window_bg: 0xFF_FC_F5_F5,
        chrome_bg: 0xFF_F0_DD_E5,
        border_color: 0xFF_E8_88_A0,
        text_fg: 0xFF_44_22_33,
        title_fg: 0xFF_55_33_44,
        button_bg: 0xFF_F0_CC_DD,
        button_hot: 0xFF_F0_B0_C0,
        taskbar_bg: 0xFF_F0_DD_E5,
        taskbar_border: 0xFF_E8_88_A0,
        menu_bg: 0xFF_F5_EE_F0,
        menu_hover: 0xFF_F0_DD_E5,
        blur_radius: 8,
        corner_radius: 14,
        shadow_opacity: 10,
    };
    v
}

// ── Preset registry ──────────────────────────────────────────────────────

pub fn build_preset(preset: VibePreset) -> VibeProfile {
    match preset {
        VibePreset::CyberpunkNight => preset_cyberpunk_night(),
        VibePreset::StudioGhibliMorning => preset_studio_ghibli_morning(),
        VibePreset::Bauhaus => preset_bauhaus(),
        VibePreset::NeoNoir => preset_neo_noir(),
        VibePreset::NordicFrost => preset_nordic_frost(),
        VibePreset::RetroWave => preset_retrowave(),
        VibePreset::MinimalZen => preset_minimal_zen(),
        VibePreset::ForestDusk => preset_forest_dusk(),
        VibePreset::OceanBreeze => preset_ocean_breeze(),
        VibePreset::SolarPunk => preset_solarpunk(),
        VibePreset::MidnightAbyss => preset_midnight_abyss(),
        VibePreset::SakuraDawn => preset_sakura_dawn(),
    }
}

pub const ALL_PRESETS: &[VibePreset] = &[
    VibePreset::CyberpunkNight,
    VibePreset::StudioGhibliMorning,
    VibePreset::Bauhaus,
    VibePreset::NeoNoir,
    VibePreset::NordicFrost,
    VibePreset::RetroWave,
    VibePreset::MinimalZen,
    VibePreset::ForestDusk,
    VibePreset::OceanBreeze,
    VibePreset::SolarPunk,
    VibePreset::MidnightAbyss,
    VibePreset::SakuraDawn,
];

// ── Time-based auto-switch ───────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimeSlot {
    pub hour_start: u8,
    pub hour_end: u8,
}

impl TimeSlot {
    pub const fn new(start: u8, end: u8) -> Self {
        Self {
            hour_start: start,
            hour_end: end,
        }
    }

    pub fn contains_hour(&self, hour: u8) -> bool {
        if self.hour_start <= self.hour_end {
            hour >= self.hour_start && hour < self.hour_end
        } else {
            hour >= self.hour_start || hour < self.hour_end
        }
    }
}

#[derive(Debug, Clone)]
pub struct TimeScheduleEntry {
    pub slot: TimeSlot,
    pub profile: String,
}

// ── Vibe Engine — the runtime manager ────────────────────────────────────

pub struct VibeEngine {
    active_profile: VibeProfile,
    user_profiles: Vec<VibeProfile>,
    schedule: Vec<TimeScheduleEntry>,
    schedule_enabled: bool,
    transition_progress: u16,
    transition_from: Option<CompositorTheme>,
}

impl VibeEngine {
    pub fn new() -> Self {
        Self {
            active_profile: build_preset(VibePreset::CyberpunkNight),
            user_profiles: Vec::new(),
            schedule: Vec::new(),
            schedule_enabled: false,
            transition_progress: 256,
            transition_from: None,
        }
    }

    pub fn active(&self) -> &VibeProfile {
        &self.active_profile
    }

    pub fn apply_preset(&mut self, preset: VibePreset) {
        self.begin_transition();
        self.active_profile = build_preset(preset);
    }

    pub fn apply_profile(&mut self, profile: VibeProfile) {
        self.begin_transition();
        self.active_profile = profile;
    }

    fn begin_transition(&mut self) {
        self.transition_from = Some(self.active_profile.compositor);
        self.transition_progress = 0;
    }

    pub fn tick_transition(&mut self, delta_ms: u16) {
        if self.transition_progress < 256 {
            self.transition_progress = self.transition_progress.saturating_add(delta_ms);
            if self.transition_progress > 256 {
                self.transition_progress = 256;
                self.transition_from = None;
            }
        }
    }

    pub fn is_transitioning(&self) -> bool {
        self.transition_progress < 256
    }

    pub fn effective_compositor_theme(&self) -> CompositorTheme {
        match self.transition_from {
            Some(from) if self.transition_progress < 256 => {
                let to = &self.active_profile.compositor;
                let t = self.transition_progress;
                CompositorTheme {
                    window_bg: lerp_argb(from.window_bg, to.window_bg, t),
                    chrome_bg: lerp_argb(from.chrome_bg, to.chrome_bg, t),
                    border_color: lerp_argb(from.border_color, to.border_color, t),
                    text_fg: lerp_argb(from.text_fg, to.text_fg, t),
                    title_fg: lerp_argb(from.title_fg, to.title_fg, t),
                    button_bg: lerp_argb(from.button_bg, to.button_bg, t),
                    button_hot: lerp_argb(from.button_hot, to.button_hot, t),
                    taskbar_bg: lerp_argb(from.taskbar_bg, to.taskbar_bg, t),
                    taskbar_border: lerp_argb(from.taskbar_border, to.taskbar_border, t),
                    menu_bg: lerp_argb(from.menu_bg, to.menu_bg, t),
                    menu_hover: lerp_argb(from.menu_hover, to.menu_hover, t),
                    blur_radius: lerp_u8(from.blur_radius, to.blur_radius, t),
                    corner_radius: lerp_u8(from.corner_radius, to.corner_radius, t),
                    shadow_opacity: lerp_u8(from.shadow_opacity, to.shadow_opacity, t),
                }
            }
            _ => self.active_profile.compositor,
        }
    }

    // ── User profiles ────────────────────────────────────────────────

    pub fn save_user_profile(&mut self, profile: VibeProfile) {
        if let Some(existing) = self
            .user_profiles
            .iter_mut()
            .find(|p| p.name == profile.name)
        {
            *existing = profile;
        } else {
            self.user_profiles.push(profile);
        }
    }

    pub fn load_user_profile(&mut self, name: &str) -> bool {
        if let Some(profile) = self.user_profiles.iter().find(|p| p.name == name) {
            let p = profile.clone();
            self.apply_profile(p);
            true
        } else {
            false
        }
    }

    pub fn remove_user_profile(&mut self, name: &str) {
        self.user_profiles.retain(|p| p.name != name);
    }

    pub fn user_profile_names(&self) -> Vec<&str> {
        self.user_profiles.iter().map(|p| p.name.as_str()).collect()
    }

    pub fn snapshot_current_as_user_profile(&mut self, name: &str) {
        let mut snapshot = self.active_profile.clone();
        snapshot.name = String::from(name);
        snapshot.author = String::from("User");
        self.save_user_profile(snapshot);
    }

    // ── Schedule ─────────────────────────────────────────────────────

    pub fn set_schedule_enabled(&mut self, enabled: bool) {
        self.schedule_enabled = enabled;
    }

    pub fn add_schedule_entry(&mut self, slot: TimeSlot, profile_name: &str) {
        self.schedule.push(TimeScheduleEntry {
            slot,
            profile: String::from(profile_name),
        });
    }

    pub fn clear_schedule(&mut self) {
        self.schedule.clear();
    }

    pub fn check_schedule(&mut self, current_hour: u8) {
        if !self.schedule_enabled {
            return;
        }
        for entry in &self.schedule {
            if entry.slot.contains_hour(current_hour) && self.active_profile.name != entry.profile {
                for preset in ALL_PRESETS {
                    let built = build_preset(*preset);
                    if built.name == entry.profile {
                        self.begin_transition();
                        self.active_profile = built;
                        return;
                    }
                }
                if let Some(profile) = self.user_profiles.iter().find(|p| p.name == entry.profile) {
                    let p = profile.clone();
                    self.apply_profile(p);
                    return;
                }
            }
        }
    }
}

// ── ARGB interpolation helpers ───────────────────────────────────────────

fn lerp_u8(a: u8, b: u8, t_256: u16) -> u8 {
    let inv = 256u16.saturating_sub(t_256);
    ((a as u16 * inv + b as u16 * t_256) >> 8) as u8
}

fn lerp_argb(a: u32, b: u32, t_256: u16) -> u32 {
    let ca = Color::from_argb(a);
    let cb = Color::from_argb(b);
    ca.lerp(cb, t_256).to_argb()
}
