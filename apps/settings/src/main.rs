//! RaeenOS Settings.
//!
//! Standalone userspace ELF launched from the start menu (`exec_path = "settings"`).
//! Sectioned settings panel: System / Display / Sound / Network / Personalization
//! / Power / Privacy / About.
//!
//! Settings read and write the kernel config registry via `SYS_CONFIG_GET/SET`.

#![no_std]
#![no_main]

extern crate alloc;

#[allow(unused_imports)]
use raekit;

use alloc::vec;
use alloc::vec::Vec;

use rae_tokens::{DARK, RAEBLUE};
use raegfx::text::FontFamily;
use raegfx::Canvas;

trait StrSlice {
    fn as_str(&self) -> &str;
}
impl StrSlice for Vec<u8> {
    fn as_str(&self) -> &str {
        core::str::from_utf8(self).unwrap_or("?")
    }
}

// ── Window geometry ─────────────────────────────────────────────────────

const WIN_W: usize = 820;
const WIN_H: usize = 520;
const SURFACE_VIRT: u64 = 0x0000_7B00_0000;

const TITLE_H: usize = 28;
const SIDEBAR_W: usize = 200;
const HEADER_H: usize = 56;
const ROW_H: usize = 56;
const STATUS_H: usize = 22;

// ── Palette (rae_tokens, docs/design/design-language.md) ──────────────────
//
// Generic chrome pulled onto `rae_tokens::DARK` + the RaeBlue accent ramp for
// whole-OS cohesion. Accent shades are derived (non-const) so they live in
// helpers. The toggle-ON green maps to `state_ok` (a real token); no
// app-specific colors remain. Live Vibe accent = NEEDS-INTERFACE (see report).

const BG: u32 = DARK.bg_raised;
const TITLE_BG: u32 = DARK.bg_base;
const SIDEBAR_BG: u32 = DARK.bg_base;
const SECTION_BG: u32 = DARK.bg_overlay;
const ROW_BG: u32 = DARK.bg_elevated;
const ROW_ALT: u32 = DARK.bg_raised;
const TEXT_FG: u32 = DARK.text_primary;
const TEXT_DIM: u32 = DARK.text_secondary;
const STATUS_BG: u32 = DARK.bg_base;
const TOGGLE_ON: u32 = DARK.state_ok; // success/on track
const TOGGLE_OFF: u32 = DARK.bg_elevated; // neutral off track

/// The live desktop accent seed (Vibe Mode's active accent) via `SYS_THEME_GET`,
/// or RaeBlue when the theme syscall is unavailable. Read at launch so Settings
/// re-skins to match the rest of the desktop (Concept §Customization Engine).
fn theme_seed() -> u32 {
    raekit::sys::theme_accent()
}
/// Accent base (live ramp) — labels, sliders, chevrons.
fn accent() -> u32 {
    rae_tokens::derive_accent(theme_seed(), &DARK).base
}
/// Selected sidebar row / selection wash: the accent's active (pressed) shade.
fn row_sel_bg() -> u32 {
    rae_tokens::derive_accent(theme_seed(), &DARK).active
}
/// Lower-emphasis accent fill (active section pill, dropdown well). Reuses the
/// active shade for an opaque accent tint over the solid panel.
fn accent_dim() -> u32 {
    rae_tokens::derive_accent(theme_seed(), &DARK).active
}

// ── Section model ───────────────────────────────────────────────────────

#[derive(Clone, Copy)]
struct Section {
    name: &'static str,
    glyph: char,
}

const SECTIONS: &[Section] = &[
    Section {
        name: "System",
        glyph: 'S',
    },
    Section {
        name: "Display",
        glyph: 'D',
    },
    Section {
        name: "Sound",
        glyph: 'A',
    },
    Section {
        name: "Network",
        glyph: 'N',
    },
    Section {
        name: "Personalization",
        glyph: 'P',
    },
    Section {
        name: "Power",
        glyph: 'B',
    },
    Section {
        name: "Privacy",
        glyph: 'L',
    },
    Section {
        name: "About",
        glyph: '?',
    },
];

#[derive(Clone, Copy)]
enum Control {
    Toggle(bool),
    Slider {
        value: u8,
        max: u8,
    },
    Choice {
        selected: u8,
        options: &'static [&'static str],
    },
    Action(&'static str),
    Label(&'static str),
    /// Like Label but holds a runtime string (up to 63 bytes).
    DynLabel {
        buf: [u8; 64],
        len: u8,
    },
}

impl Control {
    fn dyn_label(s: &[u8]) -> Self {
        let len = s.len().min(63);
        let mut buf = [0u8; 64];
        buf[..len].copy_from_slice(&s[..len]);
        Control::DynLabel {
            buf,
            len: len as u8,
        }
    }
}

struct Item {
    title: &'static str,
    detail: &'static str,
    control: Control,
}

// ── Static layout ───────────────────────────────────────────────────────
//
// Two-level: index by section ordinal, returns a slice of items.

fn items_for(section: usize, app: &App) -> Vec<Item> {
    match section {
        0 => vec![
            Item {
                title: "Game Mode",
                detail: "Enable SCHED_GAME priority class by default",
                control: Control::Toggle(app.game_mode),
            },
            Item {
                title: "Update channel",
                detail: "dev / beta / stable",
                control: Control::Choice {
                    selected: app.channel,
                    options: &["dev", "beta", "stable"],
                },
            },
            Item {
                title: "Telemetry",
                detail: "Anonymous crash reports",
                control: Control::Toggle(app.telemetry),
            },
            Item {
                title: "Fast boot",
                detail: "Skip POST-equivalent checks on warm reboot",
                control: Control::Toggle(app.fast_boot),
            },
            Item {
                title: "Kernel log",
                detail: "Verbose serial output (COM1)",
                control: Control::Toggle(app.klog),
            },
        ],
        1 => vec![
            Item {
                title: "Resolution",
                detail: "Output resolution (width × height)",
                control: Control::Choice {
                    selected: app.resolution,
                    options: &["1280×720", "1920×1080", "2560×1440", "3840×2160"],
                },
            },
            Item {
                title: "Refresh rate",
                detail: "Hz (requires VRR-capable display)",
                control: Control::Choice {
                    selected: app.refresh,
                    options: &["60", "120", "144", "165", "240"],
                },
            },
            Item {
                title: "Scale",
                detail: "UI scale percentage",
                control: Control::Slider {
                    value: app.scale_pct.clamp(50, 200) as u8,
                    max: 200,
                },
            },
            Item {
                title: "HDR",
                detail: "Enable if monitor supports it",
                control: Control::Toggle(app.hdr),
            },
        ],
        2 => vec![
            Item {
                title: "Master volume",
                detail: "Adjust system volume",
                control: Control::Slider {
                    value: app.volume.clamp(0, 100) as u8,
                    max: 100,
                },
            },
            Item {
                title: "Mute",
                detail: "Silence all output",
                control: Control::Toggle(app.mute),
            },
            Item {
                title: "Spatial audio",
                detail: "Headphone surround",
                control: Control::Toggle(app.spatial),
            },
        ],
        3 => vec![
            Item {
                title: "Wi-Fi radio",
                detail: "Enable wireless networking",
                control: Control::Toggle(app.wifi),
            },
            Item {
                title: "Firewall",
                detail: "Default deny / gaming / open",
                control: Control::Choice {
                    selected: app.firewall,
                    options: &["DefaultDeny", "Gaming", "Open"],
                },
            },
        ],
        4 => vec![
            Item {
                title: "Theme",
                detail: "Light / Dark / Auto",
                control: Control::Choice {
                    selected: app.theme,
                    options: &["Light", "Dark", "Auto"],
                },
            },
            Item {
                title: "Vibe Mode",
                detail: "Cyberpunk / Studio Ghibli",
                control: Control::Choice {
                    selected: app.vibe,
                    options: &["Default", "Cyberpunk", "Ghibli"],
                },
            },
            Item {
                title: "Glassmorphism",
                detail: "Compositor blur effects",
                control: Control::Toggle(app.glass),
            },
            Item {
                title: "Animations",
                detail: "Window open/close motion",
                control: Control::Toggle(app.animations),
            },
        ],
        5 => vec![
            Item {
                title: "Power profile",
                detail: "Performance / balanced / battery",
                control: Control::Choice {
                    selected: app.power_profile,
                    options: &["Performance", "Balanced", "Battery"],
                },
            },
            Item {
                title: "Sleep after idle",
                detail: "Minutes before suspend",
                control: Control::Slider {
                    value: app.sleep_minutes.clamp(1, 120) as u8,
                    max: 120,
                },
            },
        ],
        6 => vec![
            Item {
                title: "Telemetry",
                detail: "Anonymous crash reports",
                control: Control::Toggle(app.telemetry),
            },
            Item {
                title: "Location",
                detail: "Allow apps to request location",
                control: Control::Toggle(app.location),
            },
            Item {
                title: "Camera",
                detail: "Allow apps to request camera",
                control: Control::Toggle(app.camera),
            },
            Item {
                title: "Microphone",
                detail: "Allow apps to request microphone",
                control: Control::Toggle(app.mic),
            },
            Item {
                title: "Sign out",
                detail: "Return to login screen",
                control: Control::Action("Sign out"),
            },
        ],
        7 => vec![
            Item {
                title: "OS",
                detail: "Operating system name",
                control: Control::dyn_label(&app.sys_name),
            },
            Item {
                title: "Version",
                detail: "Build version + channel",
                control: Control::dyn_label(&app.sys_version),
            },
            Item {
                title: "Kernel",
                detail: "Architecture",
                control: Control::Label("RaeKernel x86_64 hybrid"),
            },
            Item {
                title: "SMP",
                detail: "Symmetric multi-processing",
                control: Control::Label("up to 8 cores"),
            },
            Item {
                title: "Config",
                detail: "Versioned registry generation",
                control: Control::Label("live"),
            },
            Item {
                title: "License",
                detail: "Source availability",
                control: Control::Label("Proprietary"),
            },
        ],
        _ => vec![],
    }
}

// ── Config bridge ───────────────────────────────────────────────────────

fn cfg_bool(key: &str, default: bool) -> bool {
    let mut buf = [0u8; 16];
    match raekit::sys::config_get(key, &mut buf) {
        Some(n) if n > 0 => buf[0] != 0 || buf.starts_with(b"true"),
        _ => default,
    }
}

fn cfg_int(key: &str, default: i64) -> i64 {
    let mut buf = [0u8; 16];
    if let Some(n) = raekit::sys::config_get(key, &mut buf) {
        if n == 8 {
            let mut b = [0u8; 8];
            b.copy_from_slice(&buf[..8]);
            return i64::from_le_bytes(b);
        }
        if let Ok(s) = core::str::from_utf8(&buf[..n]) {
            return parse_i64(s).unwrap_or(default);
        }
    }
    default
}

fn cfg_text_buf(key: &str, default: &str) -> Vec<u8> {
    let mut buf = [0u8; 64];
    if let Some(n) = raekit::sys::config_get(key, &mut buf) {
        if n > 0 {
            return buf[..n].to_vec();
        }
    }
    default.as_bytes().to_vec()
}

fn cfg_text_choice(key: &str, options: &[&str], default: u8) -> u8 {
    let mut buf = [0u8; 32];
    if let Some(n) = raekit::sys::config_get(key, &mut buf) {
        if let Ok(s) = core::str::from_utf8(&buf[..n]) {
            if let Some(i) = options.iter().position(|o| *o == s) {
                return i as u8;
            }
        }
    }
    default
}

fn parse_i64(s: &str) -> Option<i64> {
    let mut neg = false;
    let mut val: i64 = 0;
    for (i, b) in s.bytes().enumerate() {
        if i == 0 && b == b'-' {
            neg = true;
            continue;
        }
        if b < b'0' || b > b'9' {
            return None;
        }
        val = val.saturating_mul(10).saturating_add((b - b'0') as i64);
    }
    Some(if neg { -val } else { val })
}

// ── App state ───────────────────────────────────────────────────────────

struct App {
    section: usize,
    item: usize,
    focus_sidebar: bool,
    // System
    game_mode: bool,
    channel: u8,
    fast_boot: bool,
    klog: bool,
    // Display
    resolution: u8,
    refresh: u8,
    scale_pct: i64,
    hdr: bool,
    // Sound
    volume: i64,
    mute: bool,
    spatial: bool,
    // Network
    wifi: bool,
    firewall: u8,
    // Personalization
    theme: u8,
    vibe: u8,
    glass: bool,
    animations: bool,
    // Power
    power_profile: u8,
    sleep_minutes: i64,
    // Privacy
    telemetry: bool,
    location: bool,
    camera: bool,
    mic: bool,
    // About (live from registry)
    sys_name: Vec<u8>,
    sys_version: Vec<u8>,
}

impl App {
    fn new() -> Self {
        Self {
            section: 0,
            item: 0,
            focus_sidebar: true,
            // System
            game_mode: cfg_bool("/system/game_mode_default", true),
            channel: cfg_text_choice("/system/channel", &["dev", "beta", "stable"], 0),
            fast_boot: cfg_bool("/system/fast_boot", false),
            klog: cfg_bool("/system/kernel_log", true),
            // Display
            resolution: cfg_text_choice(
                "/display/resolution",
                &["1280×720", "1920×1080", "2560×1440", "3840×2160"],
                1,
            ),
            refresh: cfg_text_choice(
                "/display/refresh_hz_str",
                &["60", "120", "144", "165", "240"],
                0,
            ),
            scale_pct: cfg_int("/display/scale_pct", 100),
            hdr: cfg_bool("/display/hdr_enabled", false),
            // Sound
            volume: cfg_int("/audio/master_volume", 70),
            mute: cfg_bool("/audio/mute", false),
            spatial: cfg_bool("/audio/spatial_audio", false),
            // Network
            wifi: cfg_bool("/network/wifi_radio", true),
            firewall: cfg_text_choice(
                "/network/firewall_profile",
                &["DefaultDeny", "Gaming", "Open"],
                0,
            ),
            // Personalization
            theme: cfg_text_choice("/personalization/theme", &["Light", "Dark", "Auto"], 1),
            vibe: cfg_text_choice(
                "/personalization/vibe_mode",
                &["Default", "Cyberpunk", "Ghibli"],
                0,
            ),
            glass: cfg_bool("/personalization/glassmorphism", true),
            animations: cfg_bool("/personalization/animations", true),
            // Power
            power_profile: cfg_text_choice(
                "/power/profile",
                &["Performance", "Balanced", "Battery"],
                0,
            ),
            sleep_minutes: cfg_int("/power/sleep_idle_minutes", 15),
            // Privacy
            telemetry: cfg_bool("/system/telemetry_enabled", false),
            location: cfg_bool("/privacy/location_enabled", false),
            camera: cfg_bool("/privacy/camera_enabled", false),
            mic: cfg_bool("/privacy/microphone_enabled", false),
            // About
            sys_name: cfg_text_buf("/system/name", "RaeenOS"),
            sys_version: {
                let v = cfg_text_buf("/system/version", "0.0.1");
                let ch = cfg_text_buf("/system/channel", "dev");
                let mut out = v;
                out.push(b' ');
                out.push(b'(');
                for b in ch {
                    out.push(b);
                }
                out.push(b')');
                out
            },
        }
    }

    fn current_items(&self) -> Vec<Item> {
        items_for(self.section, self)
    }

    fn activate_item(&mut self) {
        if self.focus_sidebar {
            return;
        }
        let items = self.current_items();
        let Some(it) = items.get(self.item) else {
            return;
        };
        match it.control {
            Control::Toggle(_) => self.toggle_bound(it.title),
            Control::Choice { selected, options } => {
                let next = ((selected as usize + 1) % options.len()) as u8;
                self.set_choice(it.title, next, options);
            }
            Control::Slider { value, max } => {
                let step = (max / 10).max(1);
                let next = value.saturating_add(step).min(max);
                self.set_slider(it.title, next);
            }
            Control::Action("Sign out") => {
                raekit::sys::session_logout();
                raekit::sys::exit(0);
            }
            _ => {}
        }
    }

    fn toggle_bound(&mut self, title: &str) {
        match title {
            "HDR" => {
                self.hdr = !self.hdr;
                let _ = raekit::sys::config_set_bool("/display/hdr_enabled", self.hdr);
            }
            "Mute" => {
                self.mute = !self.mute;
                let _ = raekit::sys::config_set_bool("/audio/mute", self.mute);
            }
            "Spatial audio" => {
                self.spatial = !self.spatial;
                let _ = raekit::sys::config_set_bool("/audio/spatial_audio", self.spatial);
            }
            "Glassmorphism" => {
                self.glass = !self.glass;
                let _ = raekit::sys::config_set_bool("/personalization/glassmorphism", self.glass);
            }
            "Animations" => {
                self.animations = !self.animations;
                let _ =
                    raekit::sys::config_set_bool("/personalization/animations", self.animations);
            }
            "Telemetry" => {
                self.telemetry = !self.telemetry;
                let _ = raekit::sys::config_set_bool("/system/telemetry_enabled", self.telemetry);
            }
            "Location" => {
                self.location = !self.location;
                let _ = raekit::sys::config_set_bool("/privacy/location_enabled", self.location);
            }
            "Camera" => {
                self.camera = !self.camera;
                let _ = raekit::sys::config_set_bool("/privacy/camera_enabled", self.camera);
            }
            "Microphone" => {
                self.mic = !self.mic;
                let _ = raekit::sys::config_set_bool("/privacy/microphone_enabled", self.mic);
            }
            "Wi-Fi radio" => {
                self.wifi = !self.wifi;
                let _ = raekit::sys::config_set_bool("/network/wifi_radio", self.wifi);
            }
            "Game Mode" => {
                self.game_mode = !self.game_mode;
                let _ = raekit::sys::config_set_bool("/system/game_mode_default", self.game_mode);
            }
            "Fast boot" => {
                self.fast_boot = !self.fast_boot;
                let _ = raekit::sys::config_set_bool("/system/fast_boot", self.fast_boot);
            }
            "Kernel log" => {
                self.klog = !self.klog;
                let _ = raekit::sys::config_set_bool("/system/kernel_log", self.klog);
            }
            _ => {}
        }
    }

    fn set_choice(&mut self, title: &str, idx: u8, options: &[&str]) {
        let label = options.get(idx as usize).copied().unwrap_or("");
        match title {
            "Theme" => {
                self.theme = idx;
                let _ = raekit::sys::config_set_text("/personalization/theme", label);
            }
            "Vibe Mode" => {
                self.vibe = idx;
                let _ = raekit::sys::config_set_text("/personalization/vibe_mode", label);
            }
            "Firewall" => {
                self.firewall = idx;
                let _ = raekit::sys::config_set_text("/network/firewall_profile", label);
            }
            "Power profile" => {
                self.power_profile = idx;
                let _ = raekit::sys::config_set_text("/power/profile", label);
            }
            "Update channel" => {
                self.channel = idx;
                let _ = raekit::sys::config_set_text("/system/channel", label);
            }
            "Resolution" => {
                self.resolution = idx;
                let _ = raekit::sys::config_set_text("/display/resolution", label);
            }
            "Refresh rate" => {
                self.refresh = idx;
                let _ = raekit::sys::config_set_text("/display/refresh_hz_str", label);
            }
            _ => {}
        }
    }

    fn set_slider(&mut self, title: &str, value: u8) {
        match title {
            "Master volume" => {
                self.volume = value as i64;
                let _ = raekit::sys::config_set_int("/audio/master_volume", self.volume);
            }
            "Sleep after idle" => {
                self.sleep_minutes = value as i64;
                let _ =
                    raekit::sys::config_set_int("/power/sleep_idle_minutes", self.sleep_minutes);
            }
            "Scale" => {
                self.scale_pct = value as i64;
                let _ = raekit::sys::config_set_int("/display/scale_pct", self.scale_pct);
            }
            _ => {}
        }
    }

    fn move_section(&mut self, delta: i32) {
        let n = SECTIONS.len() as i32;
        self.section = ((self.section as i32 + delta).rem_euclid(n)) as usize;
        self.item = 0;
    }

    fn move_item(&mut self, delta: i32) {
        let n = self.current_items().len() as i32;
        if n == 0 {
            return;
        }
        self.item = ((self.item as i32 + delta).rem_euclid(n)) as usize;
    }
}

// ── Rendering ───────────────────────────────────────────────────────────

fn render(app: &App, canvas: &mut Canvas) {
    canvas.fill_rect(0, 0, WIN_W, WIN_H, BG);

    // Title bar
    canvas.fill_rect(0, 0, WIN_W, TITLE_H, TITLE_BG);
    canvas.draw_text_aa(
        12,
        ((TITLE_H - rae_tokens::TYPE_SUBTITLE.line_height as usize) / 2) as i32,
        "Settings",
        rae_tokens::TYPE_SUBTITLE,
        TEXT_FG,
        FontFamily::Sans,
    );
    canvas.fill_rect(WIN_W - 28, 4, 20, 20, DARK.state_danger);
    let x_w = canvas.measure_text_aa("X", rae_tokens::TYPE_LABEL, FontFamily::Sans);
    canvas.draw_text_aa(
        (WIN_W - 18) as i32 - x_w / 2,
        (4 + (20 - rae_tokens::TYPE_LABEL.line_height as usize) / 2) as i32,
        "X",
        rae_tokens::TYPE_LABEL,
        0xFF_FF_FF_FF,
        FontFamily::Sans,
    );

    // Sidebar
    let sb_y = TITLE_H;
    let sb_h = WIN_H - sb_y - STATUS_H;
    canvas.fill_rect(0, sb_y, SIDEBAR_W, sb_h, SIDEBAR_BG);

    canvas.draw_text_aa(
        16,
        (sb_y + 12) as i32,
        "Settings",
        rae_tokens::TYPE_LABEL,
        accent(),
        FontFamily::Sans,
    );
    canvas.draw_text_aa(
        16,
        (sb_y + 30) as i32,
        "Find:  /  to search",
        rae_tokens::TYPE_CAPTION,
        TEXT_DIM,
        FontFamily::Sans,
    );

    for (i, sec) in SECTIONS.iter().enumerate() {
        let y = sb_y + 56 + i * 38;
        let selected = i == app.section;
        if selected {
            canvas.fill_rect(8, y, SIDEBAR_W - 16, 30, row_sel_bg());
        } else if app.focus_sidebar && i == app.section {
            canvas.fill_rect(8, y, SIDEBAR_W - 16, 30, accent_dim());
        }
        let row_ty = (y + (30 - rae_tokens::TYPE_LABEL.line_height as usize) / 2) as i32;
        let mut g = [0u8; 1];
        g[0] = sec.glyph as u8;
        if let Ok(gs) = core::str::from_utf8(&g) {
            canvas.draw_text_aa(
                20,
                row_ty,
                gs,
                rae_tokens::TYPE_LABEL,
                accent(),
                FontFamily::Sans,
            );
        }
        canvas.draw_text_aa(
            40,
            row_ty,
            sec.name,
            rae_tokens::TYPE_LABEL,
            TEXT_FG,
            FontFamily::Sans,
        );
    }

    // Header for the selected section
    let panel_x = SIDEBAR_W;
    let panel_w = WIN_W - SIDEBAR_W;

    canvas.fill_rect(panel_x, sb_y, panel_w, HEADER_H, SECTION_BG);
    let title = SECTIONS[app.section].name;
    canvas.draw_text_aa(
        (panel_x + 16) as i32,
        (sb_y + 10) as i32,
        title,
        rae_tokens::TYPE_TITLE,
        TEXT_FG,
        FontFamily::Sans,
    );
    canvas.draw_text_aa(
        (panel_x + 16) as i32,
        (sb_y + 36) as i32,
        panel_subtitle(app.section),
        rae_tokens::TYPE_CAPTION,
        TEXT_DIM,
        FontFamily::Sans,
    );

    // Items panel
    let items = app.current_items();
    let items_y = sb_y + HEADER_H;
    let items_h = sb_h - HEADER_H;
    canvas.fill_rect(panel_x, items_y, panel_w, items_h, BG);

    let max_rows = items_h / ROW_H;
    for (i, item) in items.iter().take(max_rows).enumerate() {
        let row_y = items_y + i * ROW_H;
        let selected = !app.focus_sidebar && i == app.item;
        let bg = if selected {
            row_sel_bg()
        } else if i % 2 == 0 {
            ROW_BG
        } else {
            ROW_ALT
        };
        canvas.fill_rect(panel_x + 8, row_y + 4, panel_w - 16, ROW_H - 8, bg);

        canvas.draw_text_aa(
            (panel_x + 20) as i32,
            (row_y + 10) as i32,
            item.title,
            rae_tokens::TYPE_BODY,
            TEXT_FG,
            FontFamily::Sans,
        );
        canvas.draw_text_aa(
            (panel_x + 20) as i32,
            (row_y + 30) as i32,
            item.detail,
            rae_tokens::TYPE_CAPTION,
            TEXT_DIM,
            FontFamily::Sans,
        );

        draw_control(
            canvas,
            panel_x + panel_w - 200,
            row_y + 14,
            180,
            ROW_H - 24,
            &item.control,
        );
    }

    // Status bar
    let st_y = WIN_H - STATUS_H;
    canvas.fill_rect(0, st_y, WIN_W, STATUS_H, STATUS_BG);
    canvas.draw_text_aa(
        12,
        (st_y + (STATUS_H - rae_tokens::TYPE_CAPTION.line_height as usize) / 2) as i32,
        "Tab: switch pane   Up/Down: move   Enter: toggle   Esc: close",
        rae_tokens::TYPE_CAPTION,
        TEXT_DIM,
        FontFamily::Sans,
    );
}

fn panel_subtitle(section: usize) -> &'static str {
    match section {
        0 => "Core OS preferences",
        1 => "Resolution, scale, refresh rate, HDR",
        2 => "Audio devices, volume, latency",
        3 => "Wi-Fi, firewall, RaeNet shaping",
        4 => "Themes, accent color, Vibe Mode, animations",
        5 => "Power profile, sleep timer, battery",
        6 => "Telemetry, permissions, attestation",
        7 => "System version and hardware summary",
        _ => "",
    }
}

fn draw_control(canvas: &mut Canvas, x: usize, y: usize, w: usize, h: usize, ctrl: &Control) {
    match *ctrl {
        Control::Toggle(on) => {
            let pill_w = 40;
            let pill_x = x + w - pill_w - 8;
            let pill_y = y + 4;
            let bg = if on { TOGGLE_ON } else { TOGGLE_OFF };
            canvas.fill_rect(pill_x, pill_y, pill_w, 20, bg);
            let knob_x = if on { pill_x + pill_w - 18 } else { pill_x + 2 };
            canvas.fill_rect(knob_x, pill_y + 2, 16, 16, DARK.text_primary);
        }
        Control::Slider { value, max } => {
            let bar_y = y + h / 2 - 2;
            canvas.fill_rect(x, bar_y, w - 8, 4, TOGGLE_OFF);
            let fill_w = ((value as usize * (w - 8)) / max as usize).max(2);
            canvas.fill_rect(x, bar_y, fill_w, 4, accent());
            let knob_x = x + fill_w - 4;
            canvas.fill_rect(knob_x, bar_y - 5, 8, 14, accent());
        }
        Control::Choice { selected, options } => {
            if let Some(label) = options.get(selected as usize) {
                let text_w = canvas.measure_text_aa(label, rae_tokens::TYPE_BODY, FontFamily::Sans);
                let lw = text_w as usize + 32;
                let bx = x + w - lw - 8;
                canvas.fill_rect(bx, y + 2, lw, h - 8, ROW_BG);
                let cty =
                    (y + (h.saturating_sub(rae_tokens::TYPE_BODY.line_height as usize)) / 2) as i32;
                canvas.draw_text_aa(
                    (bx + 8) as i32,
                    cty,
                    label,
                    rae_tokens::TYPE_BODY,
                    TEXT_FG,
                    FontFamily::Sans,
                );
                canvas.draw_text_aa(
                    (bx + lw - 12) as i32,
                    cty,
                    "v",
                    rae_tokens::TYPE_BODY,
                    accent(),
                    FontFamily::Sans,
                );
            }
        }
        Control::Action(label) => {
            let text_w = canvas.measure_text_aa(label, rae_tokens::TYPE_BODY, FontFamily::Sans);
            let lw = text_w as usize + 24;
            let bx = x + w - lw - 8;
            canvas.fill_rect(bx, y + 2, lw, h - 8, accent_dim());
            canvas.draw_text_aa(
                (bx + 12) as i32,
                (y + (h.saturating_sub(rae_tokens::TYPE_BODY.line_height as usize)) / 2) as i32,
                label,
                rae_tokens::TYPE_BODY,
                TEXT_FG,
                FontFamily::Sans,
            );
        }
        Control::Label(text) => {
            let lw = canvas.measure_text_aa(text, rae_tokens::TYPE_BODY, FontFamily::Sans);
            canvas.draw_text_aa(
                (x + w) as i32 - lw - 16,
                (y + (h.saturating_sub(rae_tokens::TYPE_BODY.line_height as usize)) / 2) as i32,
                text,
                rae_tokens::TYPE_BODY,
                TEXT_DIM,
                FontFamily::Sans,
            );
        }
        Control::DynLabel { ref buf, len } => {
            if let Ok(text) = core::str::from_utf8(&buf[..len as usize]) {
                let lw = canvas.measure_text_aa(text, rae_tokens::TYPE_BODY, FontFamily::Sans);
                canvas.draw_text_aa(
                    (x + w) as i32 - lw - 16,
                    (y + (h.saturating_sub(rae_tokens::TYPE_BODY.line_height as usize)) / 2) as i32,
                    text,
                    rae_tokens::TYPE_BODY,
                    TEXT_DIM,
                    FontFamily::Sans,
                );
            }
        }
    }
}

// ── Design proof (R10: a fail-able check the token wiring is correct) ─────
//
// `cargo test` can't run a libtest harness inside this `#![no_main]` bin
// (raekit's `#[panic_handler]` + std's = duplicate lang item). This pure
// `rae_tokens` proof is the fail-able authority instead; the same
// `derive_accent` ramp is host-KAT'd by `cargo test -p rae_tokens`.

/// True iff Settings' generic chrome is wired to the shared design tokens.
#[must_use]
pub fn design_proof() -> bool {
    // Chrome derives from the LIVE theme seed (Vibe Mode), not a hardcoded
    // accent; the fallback when no theme is active is RaeBlue.
    let ramp = rae_tokens::derive_accent(theme_seed(), &DARK);
    accent() == ramp.base
        && row_sel_bg() == ramp.active
        && accent_dim() == ramp.active
        && BG == DARK.bg_raised
        && TITLE_BG == DARK.bg_base
        && SECTION_BG == DARK.bg_overlay
        && ROW_BG == DARK.bg_elevated
        && TEXT_FG == DARK.text_primary
        && TEXT_DIM == DARK.text_secondary
        && TOGGLE_ON == DARK.state_ok
        && TOGGLE_OFF == DARK.bg_elevated
        && raekit::sys::THEME_DEFAULT_ACCENT == RAEBLUE
}

// ── Entry point ─────────────────────────────────────────────────────────

#[no_mangle]
pub extern "C" fn _start() -> ! {
    if !design_proof() {
        raekit::sys::exit(3);
    }
    let sid = raekit::sys::surface_create(WIN_W as u64, WIN_H as u64, SURFACE_VIRT);
    if sid == u64::MAX {
        raekit::sys::exit(1);
    }

    let mut canvas = unsafe { Canvas::new(SURFACE_VIRT as *mut u8, WIN_W, WIN_H, 4) };

    let mut app = App::new();
    render(&app, &mut canvas);
    raekit::sys::surface_present(sid, 220, 80);

    let mut extended = false;
    loop {
        let key = raekit::sys::read_key();
        if key == 0 {
            raekit::sys::yield_now();
            continue;
        }

        let sc = key as u8;
        if sc == 0xE0 {
            extended = true;
            continue;
        }
        let ext = core::mem::replace(&mut extended, false);

        let release = sc & 0x80 != 0;
        let code = sc & 0x7F;
        if release {
            continue;
        }

        let mut dirty = false;
        match (ext, code) {
            (true, 0x48) => {
                // Up
                if app.focus_sidebar {
                    app.move_section(-1);
                } else {
                    app.move_item(-1);
                }
                dirty = true;
            }
            (true, 0x50) => {
                // Down
                if app.focus_sidebar {
                    app.move_section(1);
                } else {
                    app.move_item(1);
                }
                dirty = true;
            }
            (true, 0x4B) => {
                app.focus_sidebar = true;
                dirty = true;
            } // Left
            (true, 0x4D) => {
                app.focus_sidebar = false;
                dirty = true;
            } // Right
            (false, 0x0F) => {
                app.focus_sidebar = !app.focus_sidebar;
                dirty = true;
            } // Tab
            (false, 0x1C) => {
                app.focus_sidebar = false;
                app.activate_item();
                dirty = true;
            } // Enter
            (false, 0x01) => {
                raekit::sys::exit(0);
            } // Esc
            _ => {}
        }

        if dirty {
            render(&app, &mut canvas);
            raekit::sys::surface_present(sid, 220, 80);
        }
    }
}
