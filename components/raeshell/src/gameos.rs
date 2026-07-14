//! GameOS Mode — couch UI, big-picture, controller-first.
//!
//! > *"Gaming isn't a mode. It's the default."* — RaeenOS_Concept.md
//! > §"What Makes It Different". *"GameOS Mode — couch UI, big-picture,
//! > controller-first. Toggle into it instantly. Same OS, different shell."*
//! > — Concept §Gaming-First Design.
//!
//! Toggle into it instantly.  Same OS, different shell.  Large text,
//! high-contrast, 10-foot UI designed for gamepads and TV-distance viewing.
//!
//! **Cohesion (Phase 1):** this surface reads the LIVE `rae_tokens` — the same
//! seed accent (`crate::active_accent`), palette, spacing, radius and type ramp
//! the desktop shell uses — instead of ~20 file-local constants. A one-tap Vibe
//! Mode change re-skins the couch in lockstep with the desktop. Text is crisp
//! AA RaeSans (Inter) via `draw_text_aa`, NOT the 8px bitmap font; every focus
//! target clears `HIT_TARGET_COUCH` (48px); the focused tile lifts + scales
//! 1.06× under the accent focus ring (ring + glow + top cue — four redundant
//! a11y signals, never colour alone).
//!
//! Provides:
//! - `GameOsShell` — fullscreen controller-driven interface (Steam Big Picture
//!   style) with home carousel, library grid, game detail, quick settings.
//! - `enter_gameos_mode()` / `exit_gameos_mode()` — switches between desktop
//!   and couch UI; auto-enter on boot when configured.
//! - Navigation: D-pad moves focus, A selects, B goes back, Guide toggles
//!   overlay. Fully navigable without keyboard/mouse.
//! - `couch_active_accent()` — the live accent the couch is painting with, for
//!   the cohesion smoketest + `/proc/raeen/gaming`.
//!
//! **Controller glyphs (Phase 2):** a persistent, context-sensitive button-hint
//! bar (the SteamOS staple) renders glyph chips from a selectable `GlyphSet` —
//! Xbox (A/B/X/Y), PlayStation (✕/◯/□/△), Nintendo, Generic. The active set is
//! a settable field (default Xbox). The action→glyph map is pure logic
//! (host-KAT'd); `run_glyph_smoketest` proves the bar renders crisp-AA chips for
//! the active set.
//!
//! **Live controller bind (Phase 3):** the REAL pad drives the couch with no
//! keyboard. `bind_pad(vid, pid)` records a controller and auto-selects its
//! glyph set (`glyph_set_for_vidpid`: Sony→PlayStation, Microsoft→Xbox,
//! Nintendo→Nintendo, else Generic — the SAME VIDs `kernel::input` uses, no fake
//! detection). `apply_pad_frame` translates a decoded report ([`PadFrame`], a
//! mirror of `kernel::hid_gamepad::PadInput`) into the SAME `GamepadInput`
//! events the keyboard path uses — hat/left-stick → focus, face buttons →
//! Select/Back/Details/Search, press-edge-only so a held button doesn't repeat.
//! Keyboard nav stays live; this AUGMENTS it. `run_padbind_smoketest` (host-
//! KAT'd) proves a hat-right frame moves focus right and the VID/PID maps right.
//!
//! **Per-game profile editor (Phase 5):** Concept §Gaming Features —
//! *"Per-game profiles — resolution, refresh rate, audio device, GPU power
//! limit, all configured per game and auto-applied."* Reached from a focused
//! game's detail view (Y = profile), the editor edits a [`CouchProfile`] — a
//! logical mirror of the kernel's canonical `game_profile::GameProfileAbi`
//! record (raeshell can't depend on the kernel crate). Each field is a
//! controller-editable row (D-pad up/down moves, left/right edits, A saves, B
//! cancels); every working copy stays `normalized()` so it is always SET-safe.
//! The shell_runner bridges a confirmed edit through the REAL syscall path
//! (`SYS_GAME_PROFILE_SET` -> `GET` round-trip -> `APPLY`), so `/proc/raeen/games`
//! reflects every edit. On launch the runner APPLYs the game's profile FIRST
//! (auto-applied); a missing profile uses defaults and never blocks launch.
//! `run_profile_editor_smoketest` (host-KAT'd) proves the open->edit->commit
//! round-trip; the kernel half proves the syscall round-trip end to end.

#![allow(unused)]

extern crate alloc;

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use rae_tokens::{
    Palette, TypeStyle, HIT_TARGET_COUCH, RADIUS_LG, RADIUS_MD, RADIUS_XS, SPACE_1, SPACE_2,
    SPACE_3, SPACE_4, SPACE_5, SPACE_6, SPACE_8,
};
use raegfx::text::FontFamily;

// ── Live tokens (Concept §Gaming-First — the couch reads the LIVE accent) ──
//
// The whole cohesion deliverable: instead of ~20 file-local constants, every
// colour comes from `rae_tokens` over the DARK palette (couch defaults to dark
// for TV/OLED evening use), and the accent is the LIVE Vibe seed pushed by the
// kernel into `crate::active_accent()`. A one-tap Vibe re-skin recolours the
// couch in lockstep with the desktop — "Gaming isn't a mode."

/// Couch palette — DARK (TV/OLED, 10-foot, evening use).
const PALETTE: &Palette = &rae_tokens::DARK;

/// The six-token accent ramp derived from the LIVE seed (`crate::active_accent`)
/// over the couch palette. Selected tiles, focus ring, rail bar, progress and
/// hint glyphs all key off this — so a Vibe Mode change re-skins the couch.
#[inline]
fn accent() -> rae_tokens::AccentRamp {
    rae_tokens::derive_accent(crate::active_accent(), PALETTE)
}

// ── Crisp AA text helpers (RaeSans/Inter, NOT the 8px block font) ───────────
//
// Every label/title/value in the couch surface goes through `draw_text_aa` so
// it reads at 10 feet. `(x, y)` is the line-box top-left (same contract as the
// desktop shell). These wrap the couch type ramp + Sans family in one place.

/// Draw `s` at couch type `style` in RaeSans (Inter), crisp AA. Returns the x
/// advance after the last glyph.
#[inline]
fn couch_text(
    canvas: &mut raegfx::Canvas,
    x: usize,
    y: usize,
    s: &str,
    style: TypeStyle,
    fg: u32,
) -> usize {
    let end = canvas.draw_text_aa(x as i32, y as i32, s, style, fg, FontFamily::Sans);
    end.max(0) as usize
}

/// AA width of `s` at couch type `style` (RaeSans) — for right-aligned values.
#[inline]
fn couch_text_w(canvas: &raegfx::Canvas, s: &str, style: TypeStyle) -> usize {
    canvas.measure_text_aa(s, style, FontFamily::Sans).max(0) as usize
}

/// The focus-ring draw: lift + scale + ring + glow + top highlight — the four
/// redundant signals the spec mandates (never colour alone). Drawn around the
/// already-scaled focused-tile rect `(x, y, w, h)`.
///
/// - 4px `accent.base` ring, inset `space.1`, at `radius.lg`.
/// - `elev_focus(accent.glow)` glow halo (a soft accent wash just outside).
/// - top-edge `stroke.strong` highlight (non-colour a11y cue).
fn draw_focus_ring(
    canvas: &mut raegfx::Canvas,
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    acc: &rae_tokens::AccentRamp,
) {
    // Glow halo: an accent-tinted wash one ring-width outside the tile, so the
    // tile reads as a LIT element (the SteamOS lesson: focus is glow, not a 1px
    // border). `elev_focus(acc.glow)` is the recipe; we paint it as a soft
    // rounded fill behind the ring.
    let glow = rae_tokens::elev_focus(acc.glow).color;
    let gw = FOCUS_RING_W * 2;
    canvas.draw_rounded_rect_outline(
        x.saturating_sub(gw),
        y.saturating_sub(gw),
        w + gw * 2,
        h + gw * 2,
        RADIUS_LG as usize + gw,
        glow,
    );
    // The 4px accent ring, inset by space.1 from the tile edge, at radius.lg.
    let inset = SPACE_1 as usize;
    for r in 0..FOCUS_RING_W {
        canvas.draw_rounded_rect_outline(
            x + inset + r,
            y + inset + r,
            w.saturating_sub((inset + r) * 2),
            h.saturating_sub((inset + r) * 2),
            (RADIUS_LG as usize).saturating_sub(inset + r),
            acc.base,
        );
    }
    // Non-colour cue: a bright top-edge highlight (survives a colour-blind
    // viewer / a low-contrast accent).
    let top_w = w.saturating_sub((inset + FOCUS_RING_W) * 2);
    canvas.fill_rect(
        x + inset + FOCUS_RING_W,
        y + inset + FOCUS_RING_W,
        top_w,
        FOCUS_RING_W,
        STROKE_STRONG,
    );
}

// ── Couch type ramp (design-language couch scale-up, ~1.5–1.6× desktop) ─────
// 10-foot viewing demands a fixed couch ramp; nothing smaller than CAPTION
// (17px) ever renders in couch mode (the old 8px bitmap glyph failed the bar).

/// Focused-tile title, big clock.
const TYPE_COUCH_HERO: TypeStyle = TypeStyle {
    px: 48,
    weight: 600,
    line_height: 56,
};
/// Section headers ("Featured", "Recently Played").
const TYPE_COUCH_TITLE: TypeStyle = TypeStyle {
    px: 36,
    weight: 600,
    line_height: 44,
};
/// Tile titles, quick-menu labels, page title.
const TYPE_COUCH_SUBTITLE: TypeStyle = TypeStyle {
    px: 28,
    weight: 500,
    line_height: 36,
};
/// Metadata, settings values.
const TYPE_COUCH_BODY: TypeStyle = TypeStyle {
    px: 22,
    weight: 400,
    line_height: 30,
};
/// Hint-bar text, store badges, nav labels.
const TYPE_COUCH_LABEL: TypeStyle = TypeStyle {
    px: 20,
    weight: 500,
    line_height: 26,
};
/// Timestamps, secondary hints — the couch text floor.
const TYPE_COUCH_CAPTION: TypeStyle = TypeStyle {
    px: 17,
    weight: 400,
    line_height: 22,
};

// ── Couch layout (token-driven; all hit targets ≥ HIT_TARGET_COUCH = 48px) ──

/// Cover-art tile: ~3:4 proportion, legible at 3m (was 200×140).
const CARD_W: usize = 260;
const CARD_H: usize = 340;
/// Inter-tile gap = space.5 (24px) — the couch grid gap (was a too-tight 16).
const CARD_GAP: usize = SPACE_5 as usize;
/// Grid outer margin = space.8 (48px) — the "large couch-mode gap" token.
const GRID_MARGIN: usize = SPACE_8 as usize;
/// Nav rail, widened for TV legibility (was 220).
const SIDEBAR_W: usize = 300;
/// Top status rail — a 48px couch hit-target band (was 48; now a named floor).
const TOPBAR_H: usize = HIT_TARGET_COUCH as usize + SPACE_4 as usize;
/// Every focusable row in couch mode is at least this tall.
const ROW_H: usize = HIT_TARGET_COUCH as usize;
/// Focus-ring stroke width (px) — the spec's 4px accent ring.
const FOCUS_RING_W: usize = SPACE_1 as usize;

// ── Static palette bindings (non-accent; the accent is read LIVE via accent())
// These replace the old file-local colour constants 1:1 with token references.

const BG_DARK: u32 = PALETTE.bg_base;
const BG_CARD: u32 = PALETTE.bg_raised;
const BG_SELECTED: u32 = PALETTE.bg_elevated;
const BG_SIDEBAR: u32 = PALETTE.bg_overlay;
const TEXT_PRIMARY: u32 = PALETTE.text_primary;
const TEXT_SECONDARY: u32 = PALETTE.text_secondary;
const TEXT_DIMMED: u32 = PALETTE.text_tertiary;
const GREEN: u32 = PALETTE.state_ok;
const RED: u32 = PALETTE.state_danger;
const ORANGE: u32 = PALETTE.state_warn;
const GOLD: u32 = PALETTE.state_warn;
const SEPARATOR: u32 = PALETTE.stroke_subtle;
const STROKE_STRONG: u32 = PALETTE.stroke_strong;
const TOPBAR_BG: u32 = PALETTE.bg_overlay;

/// AA glyph advance estimate for layout that still reasons in "columns of
/// characters" (truncation budgets). The couch body ramp at ~0.55em/char.
const GLYPH_W: usize = (TYPE_COUCH_BODY.px as usize * 55) / 100;

// ── Public types ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GameOsState {
    Home,
    Library,
    Store,
    Settings,
    GameRunning,
    Overlay,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GameOsPage {
    Home,
    AllGames,
    RecentlyPlayed,
    Favorites,
    Store,
    Friends,
    Settings,
    Downloads,
    Screenshots,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusTarget {
    GameGrid,
    Sidebar,
    TopBar,
    QuickMenu,
    SearchBar,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GameStoreName {
    Steam,
    Epic,
    Gog,
    RaeStore,
    Custom,
}

#[derive(Debug, Clone)]
pub struct GameEntry {
    pub id: u64,
    pub title: String,
    pub banner_color: u32,
    pub icon_char: char,
    pub store: GameStoreName,
    pub installed: bool,
    pub last_played: u64,
    pub playtime_hours: f32,
    pub rating: Option<f32>,
    pub size_gb: f32,
    pub favorited: bool,
    pub running: bool,
}

pub struct NowPlaying {
    pub game: GameEntry,
    pub pid: u64,
    pub started_at: u64,
    pub fps: f32,
    pub frametime_ms: f32,
}

pub struct UserBadge {
    pub username: String,
    pub avatar_char: char,
    pub online: bool,
    pub status: String,
}

pub struct GameOsSearch {
    pub query: String,
    pub results: Vec<GameEntry>,
    pub active: bool,
}

pub struct QuickMenu {
    pub visible: bool,
    pub items: Vec<QuickMenuItem>,
    pub selected: usize,
}

pub struct QuickMenuItem {
    pub label: String,
    pub icon: char,
    pub action: QuickAction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuickAction {
    Brightness(u8),
    Volume(u8),
    WiFi,
    Bluetooth,
    DoNotDisturb,
    Performance,
    Screenshot,
    Recording,
    FriendsOnline,
    Downloads,
    Sleep,
    Shutdown,
    DesktopMode,
}

pub struct PageTransition {
    pub from: GameOsPage,
    pub to: GameOsPage,
    pub progress: f32,
    pub duration_ms: u64,
}

#[derive(Debug, Clone)]
pub struct Achievement {
    pub id: u64,
    pub name: String,
    pub description: String,
    pub unlocked: bool,
    pub unlock_time: u64,
    pub icon_char: char,
    pub rarity_percent: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControllerMappingSlot {
    LeftStickUp,
    LeftStickDown,
    LeftStickLeft,
    LeftStickRight,
    RightStickUp,
    RightStickDown,
    RightStickLeft,
    RightStickRight,
    ButtonSouth,
    ButtonEast,
    ButtonNorth,
    ButtonWest,
    DPadUp,
    DPadDown,
    DPadLeft,
    DPadRight,
    LeftBumper,
    RightBumper,
    LeftTrigger,
    RightTrigger,
    LeftStickClick,
    RightStickClick,
    Start,
    Select,
}

#[derive(Debug, Clone)]
pub struct ControllerMapping {
    pub name: String,
    pub mappings: Vec<(ControllerMappingSlot, String)>,
}

impl ControllerMapping {
    pub fn default_mapping() -> Self {
        Self {
            name: String::from("Default"),
            mappings: vec![
                (
                    ControllerMappingSlot::ButtonSouth,
                    String::from("Confirm / Accept"),
                ),
                (
                    ControllerMappingSlot::ButtonEast,
                    String::from("Cancel / Back"),
                ),
                (ControllerMappingSlot::ButtonNorth, String::from("Search")),
                (
                    ControllerMappingSlot::ButtonWest,
                    String::from("Context Menu"),
                ),
                (ControllerMappingSlot::DPadUp, String::from("Navigate Up")),
                (
                    ControllerMappingSlot::DPadDown,
                    String::from("Navigate Down"),
                ),
                (
                    ControllerMappingSlot::DPadLeft,
                    String::from("Navigate Left"),
                ),
                (
                    ControllerMappingSlot::DPadRight,
                    String::from("Navigate Right"),
                ),
                (ControllerMappingSlot::LeftBumper, String::from("Page Up")),
                (
                    ControllerMappingSlot::RightBumper,
                    String::from("Page Down"),
                ),
                (ControllerMappingSlot::Start, String::from("Options")),
                (ControllerMappingSlot::Select, String::from("View Toggle")),
            ],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsSection {
    Display,
    Audio,
    Network,
    Controller,
    Performance,
    General,
}

pub struct GameOsSettings {
    pub section: SettingsSection,
    pub selected_item: usize,
    pub brightness: u8,
    pub volume: u8,
    pub wifi_enabled: bool,
    pub bluetooth_enabled: bool,
    pub controller_mapping: ControllerMapping,
    pub null_latency: bool,
    pub auto_enter_gameos: bool,
    pub resolution_index: usize,
    pub refresh_rate_index: usize,
}

impl GameOsSettings {
    pub fn new() -> Self {
        Self {
            section: SettingsSection::Display,
            selected_item: 0,
            brightness: 80,
            volume: 75,
            wifi_enabled: true,
            bluetooth_enabled: true,
            controller_mapping: ControllerMapping::default_mapping(),
            null_latency: false,
            auto_enter_gameos: false,
            resolution_index: 0,
            refresh_rate_index: 0,
        }
    }
}

pub struct GameOsConfig {
    pub auto_enter: bool,
    pub default_page: GameOsPage,
    pub carousel_speed_ms: u64,
    pub transition_duration_ms: u64,
}

impl Default for GameOsConfig {
    fn default() -> Self {
        Self {
            auto_enter: false,
            default_page: GameOsPage::Home,
            carousel_speed_ms: 5000,
            transition_duration_ms: 250,
        }
    }
}

// ── Controller input ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GamepadButton {
    A,
    B,
    X,
    Y,
    DPadUp,
    DPadDown,
    DPadLeft,
    DPadRight,
    LeftBumper,
    RightBumper,
    Start,
    Select,
    Guide,
    LeftStickPress,
    RightStickPress,
}

#[derive(Debug, Clone, Copy)]
pub enum GamepadInput {
    Button(GamepadButton),
    LeftStick { x: f32, y: f32 },
    RightStick { x: f32, y: f32 },
    LeftTrigger(f32),
    RightTrigger(f32),
}

// ── Phase 3: live controller bind (PadInput → couch nav) ───────────────────
//
// Concept §GameOS: "every controller just works." Phase 1/2 drove couch focus
// from KEYBOARD routing; Phase 3 binds the REAL pad. The kernel's
// `hid_gamepad::decode_report` produces a normalized snapshot (axes -32768..
// 32767, hat 0-7/8, button bitmap); raeshell can't depend on the kernel crate,
// so [`PadFrame`] mirrors exactly those fields. The kernel feeds a decoded
// `PadInput` into `apply_pad_frame`, which translates it into the SAME
// `GamepadInput`/`GamepadButton` events the keyboard path uses — keyboard nav
// stays live; this AUGMENTS it. No fake hardware detection: the only pad facts
// are the VID/PID and the decoded report the kernel hands us.

/// Known controller-family USB vendor IDs. The auto-glyph table keys on these —
/// the SAME VIDs `kernel/src/input.rs` uses to pick the first-party decoder.
pub const VID_SONY: u16 = 0x054C; // DualShock / DualSense
pub const VID_MICROSOFT: u16 = 0x045E; // Xbox controllers
pub const VID_NINTENDO: u16 = 0x057E; // Switch Pro / Joy-Con

/// Map a bound pad's USB VID/PID to the right button-glyph skin. Pure logic
/// (host-KAT'd) — the single source of truth Phase 3 uses to auto-select the
/// glyph set when a controller binds. Unknown vendors get `Generic` (the
/// never-wrong default), never a guessed first-party skin.
#[must_use]
pub fn glyph_set_for_vidpid(vid: u16, _pid: u16) -> GlyphSet {
    match vid {
        VID_SONY => GlyphSet::PlayStation,
        VID_MICROSOFT => GlyphSet::Xbox,
        VID_NINTENDO => GlyphSet::Nintendo,
        _ => GlyphSet::Generic,
    }
}

/// A decoded pad report snapshot — a raeshell-side mirror of
/// `kernel::hid_gamepad::PadInput` (raeshell can't depend on the kernel crate).
/// Field semantics are IDENTICAL: axes normalized to the i16 range with 0 =
/// centre, `hat` is 0-7 (N, NE, E, …) with 8 = centred/none, and `buttons` is a
/// bitmap where bit N = button N+1 pressed. The kernel builds one of these from
/// its real `decode_report` output and hands it to `apply_pad_frame`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PadFrame {
    pub x: i16,
    pub y: i16,
    pub z: i16,
    pub rx: i16,
    pub ry: i16,
    pub rz: i16,
    pub hat: u8,
    pub buttons: u32,
}

/// The directional focus delta a pad frame implies — derived from the hat AND
/// the left stick (either drives focus; the hat wins when both are active).
/// Pure logic, host-KAT'd: `(dx, dy)` where +x = right, +y = down, each in
/// {-1, 0, 1}. The stick uses the same deadzone the keyboard-equivalent path
/// uses (`controller_input` LeftStick DEADZONE 0.3 of full scale).
#[must_use]
pub fn focus_delta_for_frame(frame: &PadFrame) -> (i32, i32) {
    // Hat first (D-pad is the precise nav surface). 0=N,1=NE,2=E,3=SE,4=S,
    // 5=SW,6=W,7=NW,8=centre.
    let (mut dx, mut dy) = match frame.hat {
        0 => (0, -1),
        1 => (1, -1),
        2 => (1, 0),
        3 => (1, 1),
        4 => (0, 1),
        5 => (-1, 1),
        6 => (-1, 0),
        7 => (-1, -1),
        _ => (0, 0),
    };
    // Left stick fallback when the hat is centred. 0.3 of full i16 scale.
    if dx == 0 && dy == 0 {
        const DEADZONE: i32 = (32767 * 3) / 10; // 0.3 * full scale
        let sx = frame.x as i32;
        let sy = frame.y as i32;
        if sx < -DEADZONE {
            dx = -1;
        } else if sx > DEADZONE {
            dx = 1;
        }
        if sy < -DEADZONE {
            dy = -1;
        } else if sy > DEADZONE {
            dy = 1;
        }
    }
    (dx, dy)
}

/// The logical face-button bit → couch button. Bit positions match the generic
/// HID gamepad button bitmap (`hid_gamepad::PadInput.buttons`, bit N = button
/// N+1): button 1/A = Select, button 2/B = Back, button 3/X = Details, button
/// 4/Y = Search — the SteamOS/Xbox A/B/X/Y muscle-memory order, identical to the
/// keyboard-equivalent `handle_button` actions. Pure logic, host-KAT'd.
#[must_use]
pub fn button_for_bit(bit: u32) -> Option<GamepadButton> {
    match bit {
        0 => Some(GamepadButton::A), // South / ✕ — Select / launch-detail
        1 => Some(GamepadButton::B), // East / ◯ — Back
        2 => Some(GamepadButton::X), // West / □ — Details / launch
        3 => Some(GamepadButton::Y), // North / △ — Search
        4 => Some(GamepadButton::LeftBumper), // L1 — Page up
        5 => Some(GamepadButton::RightBumper), // R1 — Page down
        8 => Some(GamepadButton::Select), // Select / Share
        9 => Some(GamepadButton::Start), // Start / Options
        10 => Some(GamepadButton::Guide), // Guide / PS / Xbox
        _ => None,
    }
}

// ── Button-glyph system (Phase 2 — selectable glyph sets) ──────────────────
//
// Concept §"DualSense + Xbox + every controller, full feature parity" → the
// SteamInput glyph system, beaten by a multi-skin set. A `GlyphSet` maps a
// LOGICAL action (Select / Back / Details / Search / Menu / …) to the on-screen
// button glyph for the active controller family. The persistent context
// hint-bar (the SteamOS staple) renders the active set, so a hint reads
// `[A] Select` on an Xbox pad and `[✕] Select` on a DualSense.
//
// The set is a SETTABLE field with a default — Phase 3 binds the real pad's
// VID/PID to auto-select it. There is NO fake hardware detection here.

/// The controller button-glyph skin. Xbox (A/B/X/Y lettered), PlayStation
/// (✕/◯/□/△ symbols), Nintendo (A/B/X/Y, positionally mirrored but logical
/// letters), and Generic/Steam (lettered, the safe default for unknown HID).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GlyphSet {
    Xbox,
    PlayStation,
    Nintendo,
    Generic,
}

impl GlyphSet {
    /// The default skin until Phase 3 binds a real pad — Generic/lettered is the
    /// "never wrong" choice for an unknown controller (the never-nothing rule).
    pub const DEFAULT: GlyphSet = GlyphSet::Xbox;

    /// Short tag for the smoketest / procfs line.
    #[must_use]
    pub fn tag(self) -> &'static str {
        match self {
            GlyphSet::Xbox => "xbox",
            GlyphSet::PlayStation => "ps",
            GlyphSet::Nintendo => "nintendo",
            GlyphSet::Generic => "generic",
        }
    }

    /// The on-screen glyph for a logical action in this skin. Pure logic
    /// (host-KAT'd) — the single source of truth the hint bar renders.
    #[must_use]
    pub fn glyph(self, action: HintAction) -> &'static str {
        use HintAction::*;
        match self {
            // Xbox / Nintendo / Generic share the lettered face-button mapping;
            // they differ in colour + (Nintendo) physical position, not letter.
            GlyphSet::Xbox | GlyphSet::Nintendo | GlyphSet::Generic => match action {
                Select => "A",
                Back => "B",
                Details => "X",
                Search => "Y",
                Favorite => "Y",
                Menu => "\u{2630}", // ☰ hamburger (Start/Options)
                Page => "LB",
                Section => "LT",
            },
            // PlayStation face symbols: ✕ ◯ □ △.
            GlyphSet::PlayStation => match action {
                Select => "\u{2715}",  // ✕ cross
                Back => "\u{25EF}",    // ◯ circle
                Details => "\u{25A1}", // □ square
                Search => "\u{25B3}",  // △ triangle
                Favorite => "\u{25B3}",
                Menu => "\u{2630}", // ☰ Options
                Page => "L1",
                Section => "L2",
            },
        }
    }
}

/// A logical, controller-agnostic action shown in the context hint bar. The
/// glyph for each is resolved through the active `GlyphSet` — the surface code
/// never names a physical button, only the intent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HintAction {
    Select,
    Back,
    Details,
    Search,
    Favorite,
    Menu,
    Page,
    Section,
}

impl HintAction {
    /// The human label shown beside the glyph chip (couch label ramp, AA).
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            HintAction::Select => "Select",
            HintAction::Back => "Back",
            HintAction::Details => "Details",
            HintAction::Search => "Search",
            HintAction::Favorite => "Favorite",
            HintAction::Menu => "Menu",
            HintAction::Page => "Page",
            HintAction::Section => "Section",
        }
    }
}

/// One resolved hint chip: a glyph (from the active set) + an action label.
#[derive(Debug, Clone, Copy)]
pub struct HintChip {
    pub action: HintAction,
    pub glyph: &'static str,
    pub label: &'static str,
}

// ── On-screen keyboard (Phase 6 — controller text entry) ────────────────────
//
// Concept §GameOS: *"GameOS Mode — couch UI, big-picture, controller-first.
// … Fully navigable without keyboard/mouse."* A text field in the couch (the
// search box, a profile text field) needs an OSK the player drives with the
// pad. The Steam-class staple: a D-pad-navigable QWERTY grid + a space/enter/
// shift row, ≥48px keys (`HIT_TARGET_COUCH` — the TV hit-target floor),
// A = type the focused key, B = backspace, glass + live accent + crisp-AA
// glyphs. It reuses the SAME Phase-3 focus model (`focus_delta_for_frame`
// drives the cursor; `button_for_bit`/`handle_button` route the buttons) — no
// reinvented nav. The layout→char map is pure logic (host-KAT'd).
//
// The OSK is the couch's OWN surface (a `CouchOsk` the shell owns), opened when
// a text field is focused and a button is pressed; typed characters feed the
// target field (the search query). It never panics — every index is clamped.

/// The fixed key grid: four rows. The first three are the QWERTY letter/number
/// layout; the fourth is the action/space row. Each `Key` is a single typeable
/// glyph or a special action; the grid is a fixed `&'static` table so there is
/// NO per-keypress allocation (alloc-light, the Phase-6 contract).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OskKey {
    /// A character key — the `char` is what gets typed (already shifted when the
    /// shift layer is active; see `OskState::resolve_char`).
    Char(char),
    /// Backspace — delete the last typed character.
    Backspace,
    /// Space — insert a single space.
    Space,
    /// Shift — toggle the upper/lower layer (letters + the symbol row).
    Shift,
    /// Enter — commit the field (the shell closes the OSK + runs the search).
    Enter,
}

/// One OSK row, as a fixed slice of keys. The grid is row-major; focus moves a
/// key at a time horizontally and a row at a time vertically (the Phase-3
/// focus model — `focus_delta_for_frame` → (dx, dy)).
pub type OskRow = &'static [OskKey];

/// The QWERTY + action grid (4 rows). Row lengths differ (10/10/9 letters then
/// a 5-cell action row); focus clamps per-row so a short row never overflows.
pub const OSK_ROWS: &[OskRow] = &[
    &[
        OskKey::Char('q'),
        OskKey::Char('w'),
        OskKey::Char('e'),
        OskKey::Char('r'),
        OskKey::Char('t'),
        OskKey::Char('y'),
        OskKey::Char('u'),
        OskKey::Char('i'),
        OskKey::Char('o'),
        OskKey::Char('p'),
    ],
    &[
        OskKey::Char('a'),
        OskKey::Char('s'),
        OskKey::Char('d'),
        OskKey::Char('f'),
        OskKey::Char('g'),
        OskKey::Char('h'),
        OskKey::Char('j'),
        OskKey::Char('k'),
        OskKey::Char('l'),
    ],
    &[
        OskKey::Shift,
        OskKey::Char('z'),
        OskKey::Char('x'),
        OskKey::Char('c'),
        OskKey::Char('v'),
        OskKey::Char('b'),
        OskKey::Char('n'),
        OskKey::Char('m'),
        OskKey::Backspace,
    ],
    &[
        OskKey::Char('1'),
        OskKey::Char('2'),
        OskKey::Char('3'),
        OskKey::Space,
        OskKey::Enter,
    ],
];

impl OskKey {
    /// The on-screen label for this key (one or two glyphs, crisp AA). For a
    /// `Char` it is the (already-resolved) glyph; the actions get short caps.
    #[must_use]
    pub fn label(self, shift: bool) -> &'static str {
        match self {
            OskKey::Char(c) => char_label(c, shift),
            OskKey::Backspace => "\u{232B}", // ⌫
            OskKey::Space => "Space",
            OskKey::Shift => "Shift",
            OskKey::Enter => "Enter",
        }
    }
}

/// The static label table for the lower/upper variant of each char key. A small
/// fixed match (no alloc) — `&'static str` so the renderer never heap-allocates
/// a per-key label.
#[must_use]
fn char_label(c: char, shift: bool) -> &'static str {
    macro_rules! pair {
        ($lo:literal, $up:literal) => {
            if shift {
                $up
            } else {
                $lo
            }
        };
    }
    match c {
        'q' => pair!("q", "Q"),
        'w' => pair!("w", "W"),
        'e' => pair!("e", "E"),
        'r' => pair!("r", "R"),
        't' => pair!("t", "T"),
        'y' => pair!("y", "Y"),
        'u' => pair!("u", "U"),
        'i' => pair!("i", "I"),
        'o' => pair!("o", "O"),
        'p' => pair!("p", "P"),
        'a' => pair!("a", "A"),
        's' => pair!("s", "S"),
        'd' => pair!("d", "D"),
        'f' => pair!("f", "F"),
        'g' => pair!("g", "G"),
        'h' => pair!("h", "H"),
        'j' => pair!("j", "J"),
        'k' => pair!("k", "K"),
        'l' => pair!("l", "L"),
        'z' => pair!("z", "Z"),
        'x' => pair!("x", "X"),
        'c' => pair!("c", "C"),
        'v' => pair!("v", "V"),
        'b' => pair!("b", "B"),
        'n' => pair!("n", "N"),
        'm' => pair!("m", "M"),
        '1' => pair!("1", "!"),
        '2' => pair!("2", "@"),
        '3' => pair!("3", "#"),
        _ => "?",
    }
}

/// Resolve the actual `char` a `Char` key types under the current shift layer —
/// the upper-case letter, or the shifted symbol for the digit keys. Pure logic
/// (host-KAT'd): this is the single source of truth the field-typing path uses.
#[must_use]
fn resolve_char(c: char, shift: bool) -> char {
    if !shift {
        return c;
    }
    match c {
        '1' => '!',
        '2' => '@',
        '3' => '#',
        other => {
            // ASCII letters upper-case; everything else passes through.
            if other.is_ascii_lowercase() {
                other.to_ascii_uppercase()
            } else {
                other
            }
        }
    }
}

/// What kind of couch text field the OSK is editing — so a commit (Enter)
/// routes the typed text to the right place. Currently the search box; the
/// enum keeps the OSK reusable for any future couch text field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OskTarget {
    /// The library search box (Y / the search affordance).
    Search,
}

/// The live state of the on-screen keyboard overlay (Phase 6). The shell holds
/// an `Option<OskState>`; it exists only while the OSK is open over a focused
/// text field. Focus is a `(row, col)` cursor; `text` is the working buffer
/// that mirrors the target field as the user types.
#[derive(Debug, Clone)]
pub struct OskState {
    /// Which field this OSK is editing (so Enter commits to the right place).
    pub target: OskTarget,
    /// The working text buffer — what the user has typed so far.
    pub text: String,
    /// Focused row in `OSK_ROWS` (clamped to a valid row).
    pub row: usize,
    /// Focused column within the focused row (clamped to that row's length).
    pub col: usize,
    /// Upper/symbol layer active (toggled by the Shift key).
    pub shift: bool,
    /// Set once the user presses Enter — the shell reads this to commit the
    /// text to the target field, run the search, and close the OSK. A latch.
    pub commit_requested: bool,
}

impl OskState {
    /// Open an OSK over `target`, seeded with the field's current `text`.
    #[must_use]
    pub fn new(target: OskTarget, text: String) -> Self {
        Self {
            target,
            text,
            row: 0,
            col: 0,
            shift: false,
            commit_requested: false,
        }
    }

    /// The number of keys in the focused row (clamped-safe).
    fn row_len(&self) -> usize {
        OSK_ROWS.get(self.row).map(|r| r.len()).unwrap_or(0)
    }

    /// The currently focused key (clamped — never panics on a short row).
    #[must_use]
    pub fn focused_key(&self) -> OskKey {
        let row = OSK_ROWS.get(self.row.min(OSK_ROWS.len().saturating_sub(1)));
        match row {
            Some(r) if !r.is_empty() => r[self.col.min(r.len() - 1)],
            _ => OskKey::Enter,
        }
    }

    /// Move the focus cursor by `(dx, dy)` — the Phase-3 focus delta. Rows wrap
    /// vertically; the column clamps into the new row's length so a long-row
    /// column never points off the end of a short row.
    pub fn move_focus(&mut self, dx: i32, dy: i32) {
        if dy != 0 {
            let n = OSK_ROWS.len() as i32;
            let mut r = self.row as i32 + dy.signum();
            if r < 0 {
                r = n - 1;
            } else if r >= n {
                r = 0;
            }
            self.row = r as usize;
            // Clamp the column into the new row.
            let rl = self.row_len();
            if rl > 0 {
                self.col = self.col.min(rl - 1);
            }
        }
        if dx != 0 {
            let rl = self.row_len();
            if rl > 0 {
                let mut c = self.col as i32 + dx.signum();
                if c < 0 {
                    c = rl as i32 - 1;
                } else if c >= rl as i32 {
                    c = 0;
                }
                self.col = c as usize;
            }
        }
    }

    /// Activate the focused key (A / the Select button). Char keys append the
    /// resolved character; Backspace pops the last char; Space appends a space;
    /// Shift toggles the layer; Enter latches a commit. Pure logic, host-KAT'd —
    /// the single source of truth for "a keypress changed the text".
    pub fn activate(&mut self) {
        match self.focused_key() {
            OskKey::Char(c) => self.text.push(resolve_char(c, self.shift)),
            OskKey::Backspace => {
                self.text.pop();
            }
            OskKey::Space => self.text.push(' '),
            OskKey::Shift => self.shift = !self.shift,
            OskKey::Enter => self.commit_requested = true,
        }
    }

    /// Backspace shortcut (the B-button-as-backspace mapping). Pops one char.
    pub fn backspace(&mut self) {
        self.text.pop();
    }
}

// ── Desktop ↔ couch cross-fade transition (Phase 6) ─────────────────────────
//
// Concept §"Toggle into it instantly. Same OS, different shell." The toggle
// must not feel like a logout (the Steam Deck failure). A brief cross-fade —
// the screen washes through `bg.base` over ~`motion.emphasized` — reads as one
// environment shifting, not two apps swapping. This is the transition-alpha
// state + the fade math (pure logic, host-KAT'd). The shell renders the couch
// surface and overlays a `bg.base` wash at `wash_alpha()` while the transition
// runs; a full compositor cross-fade (compositing BOTH shells) is flagged as a
// follow-up — this slice keeps the fade on the couch's own surface (cheap, no
// per-frame alloc, no compositor behavior change).

/// The cross-fade duration (ms) — `motion.emphasized` (the same curve a Vibe
/// Mode personality switch uses; entering GameOS IS a personality switch).
pub const CROSSFADE_MS: u64 = 320;

/// A running desktop↔couch cross-fade. `elapsed_ms` advances each frame; the
/// transition completes (and is dropped) when `elapsed_ms >= CROSSFADE_MS`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CrossFade {
    /// Milliseconds elapsed since the toggle began (0..=CROSSFADE_MS).
    pub elapsed_ms: u64,
    /// True = fading INTO the couch (desktop→GameOS); false = OUT (→desktop).
    pub entering: bool,
}

impl CrossFade {
    /// Begin a cross-fade (entering = desktop→couch, else couch→desktop).
    #[must_use]
    pub fn begin(entering: bool) -> Self {
        Self {
            elapsed_ms: 0,
            entering,
        }
    }

    /// Advance the fade by `dt_ms`. Returns `true` while the fade is still
    /// running, `false` once it has completed (so the caller can drop it).
    pub fn tick(&mut self, dt_ms: u64) -> bool {
        self.elapsed_ms = self.elapsed_ms.saturating_add(dt_ms);
        self.elapsed_ms < CROSSFADE_MS
    }

    /// The fade progress in `[0, 256]` (fixed-point, no float) — 0 at the start,
    /// 256 at completion. Used only for the wash alpha so the math stays integer
    /// (the kernel is soft-float; integer keeps it cheap + deterministic).
    #[must_use]
    pub fn progress_q8(&self) -> u32 {
        let e = self.elapsed_ms.min(CROSSFADE_MS);
        ((e as u32) * 256) / (CROSSFADE_MS as u32)
    }

    /// The alpha (0..=255) of the `bg.base` wash overlaid on the couch surface
    /// this frame. A symmetric triangle: 0 at the endpoints, peak (≈full) at the
    /// midpoint — the screen dips through the wash and back, the "one
    /// environment shifting" read. Pure logic, host-KAT'd.
    #[must_use]
    pub fn wash_alpha(&self) -> u8 {
        let p = self.progress_q8(); // 0..256
                                    // Triangle: rises 0→256 over the first half, falls 256→0 over the back.
        let tri = if p <= 128 { p * 2 } else { (256 - p) * 2 };
        tri.min(255) as u8
    }

    /// The ARGB wash color overlaid this frame: `bg.base` at `wash_alpha()`.
    /// `0` alpha (the endpoints) means "fully transparent — draw nothing".
    #[must_use]
    pub fn wash_color(&self) -> u32 {
        let a = self.wash_alpha() as u32;
        (a << 24) | (BG_DARK & 0x00FF_FFFF)
    }
}

// ── Per-game profile editor (Phase 5) ──────────────────────────────────────
//
// Concept §Gaming Features: *"Per-game profiles — resolution, refresh rate,
// audio device, GPU power limit, all configured per game and auto-applied."*
//
// The kernel owns the canonical record (`kernel::game_profile::GameProfileAbi`,
// syscalls 58–61, `/proc/raeen/games`). raeshell can't depend on the kernel
// crate, so [`CouchProfile`] is a logical MIRROR of the editable fields — the
// surface edits one of these, the kernel bridges it field-for-field into the
// real `GameProfileAbi` and drives `set_profile`/`get_profile`/`apply_profile`.
// Nothing here invents hardware: a field like `gpu_power_pct` is a stored value
// that only takes effect once the iron cpufreq setter lands; the editor stores
// + exposes it (the profile is the canonical source of truth either way).
//
// `CouchProfile` field semantics are IDENTICAL to `GameProfileAbi`'s so the
// bridge is a 1:1 copy with no reinterpretation. The flag bits MATCH
// `game_profile::FLAG_*` exactly (game_mode=0, null_latency=1, hdr=2, vrr=3).

/// Flag bit positions — MUST match `kernel::game_profile::FLAG_*`.
pub const PROFILE_FLAG_GAME_MODE: u32 = 1 << 0;
pub const PROFILE_FLAG_NULL_LATENCY: u32 = 1 << 1;
pub const PROFILE_FLAG_HDR: u32 = 1 << 2;
pub const PROFILE_FLAG_VRR: u32 = 1 << 3;

/// A logical mirror of the editable per-game profile record. 1:1 with the
/// kernel's `GameProfileAbi` fields the couch editor exposes (display / GPU /
/// audio / scheduler). `version` is fixed at 1 — the kernel rejects other
/// versions; the bridge carries it through unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CouchProfile {
    pub version: u32,
    pub resolution_w: u32,
    pub resolution_h: u32,
    pub refresh_hz: u32,
    pub gpu_power_pct: u32,
    pub audio_sink_id: u32,
    pub flags: u32,
    pub priority: u32,
    pub affinity_mask: u64,
    pub memory_pin_mib: u32,
    pub deadline_period_us: u32,
    pub deadline_runtime_us: u32,
}

impl Default for CouchProfile {
    fn default() -> Self {
        // The "Balanced" middle-ground a fresh game gets before any edit —
        // mirrors `GameProfileAbi::default_balanced()` so an unconfigured launch
        // and the kernel preset agree (never block launch on a missing profile).
        Self {
            version: 1,
            resolution_w: 2560,
            resolution_h: 1440,
            refresh_hz: 144,
            gpu_power_pct: 100,
            audio_sink_id: 0,
            flags: PROFILE_FLAG_GAME_MODE | PROFILE_FLAG_VRR | PROFILE_FLAG_HDR,
            priority: 2,
            affinity_mask: 0xFF,
            memory_pin_mib: 128,
            deadline_period_us: 6_944,
            deadline_runtime_us: 5_000,
        }
    }
}

/// The fixed list of standard resolutions the editor cycles through. Couch
/// editing is dropdown-style (D-pad left/right steps the value), so the value
/// set is enumerated, not free-typed. `(w, h)`.
pub const PROFILE_RESOLUTIONS: &[(u32, u32)] = &[
    (1280, 720),
    (1920, 1080),
    (2560, 1440),
    (3440, 1440),
    (3840, 2160),
];

/// The fixed refresh-rate steps the editor cycles through (Hz).
pub const PROFILE_REFRESH_RATES: &[u32] = &[60, 90, 120, 144, 165, 240, 360];

impl CouchProfile {
    /// Clamp/normalize every field into the kernel's accepted ranges so a SET
    /// can never carry a value the kernel would reject or that would mislead the
    /// iron setters. Pure logic, host-KAT'd. Returns a normalized copy.
    #[must_use]
    pub fn normalized(mut self) -> Self {
        self.version = 1;
        // Snap resolution to the nearest enumerated option (defensive — the
        // editor only ever sets enumerated values, but a bridged-in record
        // might carry an arbitrary mode).
        if !PROFILE_RESOLUTIONS.contains(&(self.resolution_w, self.resolution_h)) {
            let (w, h) = nearest_resolution(self.resolution_w, self.resolution_h);
            self.resolution_w = w;
            self.resolution_h = h;
        }
        if !PROFILE_REFRESH_RATES.contains(&self.refresh_hz) {
            self.refresh_hz = nearest_refresh(self.refresh_hz);
        }
        // GPU power budget is a percentage; 0 = "leave default", so the floor is
        // 0 and the ceiling is 100.
        if self.gpu_power_pct > 100 {
            self.gpu_power_pct = 100;
        }
        // Priority is SCHED_GAME(2) or Normal(0); anything else snaps to Normal.
        if self.priority != 0 && self.priority != 2 {
            self.priority = 0;
        }
        // Only the four defined flag bits may be set.
        self.flags &= PROFILE_FLAG_GAME_MODE
            | PROFILE_FLAG_NULL_LATENCY
            | PROFILE_FLAG_HDR
            | PROFILE_FLAG_VRR;
        // Never leave a 0 affinity mask (would pin to no CPU at all).
        if self.affinity_mask == 0 {
            self.affinity_mask = 0xFF;
        }
        self
    }

    #[inline]
    fn flag(&self, bit: u32) -> bool {
        self.flags & bit != 0
    }

    fn set_flag(&mut self, bit: u32, on: bool) {
        if on {
            self.flags |= bit;
        } else {
            self.flags &= !bit;
        }
    }
}

/// Snap an arbitrary `(w, h)` to the nearest enumerated resolution by total
/// pixel count. Always returns a member of `PROFILE_RESOLUTIONS`.
#[must_use]
fn nearest_resolution(w: u32, h: u32) -> (u32, u32) {
    let target = (w as u64) * (h as u64);
    let mut best = PROFILE_RESOLUTIONS[0];
    let mut best_d = u64::MAX;
    for &(cw, ch) in PROFILE_RESOLUTIONS {
        let px = (cw as u64) * (ch as u64);
        let d = px.abs_diff(target);
        if d < best_d {
            best_d = d;
            best = (cw, ch);
        }
    }
    best
}

/// Snap an arbitrary refresh rate to the nearest enumerated step.
#[must_use]
fn nearest_refresh(hz: u32) -> u32 {
    let mut best = PROFILE_REFRESH_RATES[0];
    let mut best_d = u32::MAX;
    for &r in PROFILE_REFRESH_RATES {
        let d = r.abs_diff(hz);
        if d < best_d {
            best_d = d;
            best = r;
        }
    }
    best
}

/// The editable fields of the per-game profile surface, in render/nav order. A
/// dropdown-style field steps through a value set on D-pad left/right; a toggle
/// flips a flag. This enum IS the focus model for the editor (Phase-3 focus
/// model reused — one focused row, D-pad up/down moves between rows).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileField {
    Resolution,
    RefreshRate,
    Hdr,
    Vrr,
    GpuPowerPct,
    NullLatency,
    GameMode,
    AudioSink,
}

impl ProfileField {
    /// The fields in render/navigation order (Display → GPU → Audio → Sched).
    pub const ORDER: &'static [ProfileField] = &[
        ProfileField::Resolution,
        ProfileField::RefreshRate,
        ProfileField::Hdr,
        ProfileField::Vrr,
        ProfileField::GpuPowerPct,
        ProfileField::NullLatency,
        ProfileField::GameMode,
        ProfileField::AudioSink,
    ];

    /// The number of editable fields the surface exposes (the `fields=K` count).
    #[must_use]
    pub fn count() -> usize {
        Self::ORDER.len()
    }

    /// The row label shown to the user (couch body ramp, AA).
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            ProfileField::Resolution => "Resolution",
            ProfileField::RefreshRate => "Refresh Rate",
            ProfileField::Hdr => "HDR",
            ProfileField::Vrr => "VRR (Variable Refresh)",
            ProfileField::GpuPowerPct => "GPU Power Limit",
            ProfileField::NullLatency => "Null-Latency Mode",
            ProfileField::GameMode => "Game Mode (SCHED_GAME)",
            ProfileField::AudioSink => "Audio Output",
        }
    }

    /// Step this field's value on `p` by `dir` (+1 = D-pad right, -1 = left).
    /// Toggles flip; dropdowns cycle (clamped, no wrap past the ends);
    /// percentages step by 5; the audio sink steps by 1. Pure logic — the
    /// single source of truth for the editor's value editing (host-KAT'd).
    pub fn step(self, p: &mut CouchProfile, dir: i32) {
        match self {
            ProfileField::Resolution => {
                let cur = PROFILE_RESOLUTIONS
                    .iter()
                    .position(|&r| r == (p.resolution_w, p.resolution_h))
                    .unwrap_or(0);
                let next = step_index(cur, dir, PROFILE_RESOLUTIONS.len());
                let (w, h) = PROFILE_RESOLUTIONS[next];
                p.resolution_w = w;
                p.resolution_h = h;
            }
            ProfileField::RefreshRate => {
                let cur = PROFILE_REFRESH_RATES
                    .iter()
                    .position(|&r| r == p.refresh_hz)
                    .unwrap_or(0);
                let next = step_index(cur, dir, PROFILE_REFRESH_RATES.len());
                p.refresh_hz = PROFILE_REFRESH_RATES[next];
            }
            ProfileField::Hdr => p.set_flag(PROFILE_FLAG_HDR, dir > 0),
            ProfileField::Vrr => p.set_flag(PROFILE_FLAG_VRR, dir > 0),
            ProfileField::NullLatency => p.set_flag(PROFILE_FLAG_NULL_LATENCY, dir > 0),
            ProfileField::GameMode => p.set_flag(PROFILE_FLAG_GAME_MODE, dir > 0),
            ProfileField::GpuPowerPct => {
                let step = 5i64;
                let v = (p.gpu_power_pct as i64) + (dir as i64) * step;
                p.gpu_power_pct = v.clamp(0, 100) as u32;
            }
            ProfileField::AudioSink => {
                // 0 = default; cycle 0..=7 (a small, bounded set of sinks).
                let v = (p.audio_sink_id as i64) + dir as i64;
                p.audio_sink_id = v.clamp(0, 7) as u32;
            }
        }
    }

    /// The current value of this field on `p`, formatted for display (no alloc
    /// beyond the returned `String`). Toggles read On/Off.
    #[must_use]
    pub fn value_string(self, p: &CouchProfile) -> String {
        match self {
            ProfileField::Resolution => {
                alloc::format!("{} x {}", p.resolution_w, p.resolution_h)
            }
            ProfileField::RefreshRate => alloc::format!("{} Hz", p.refresh_hz),
            ProfileField::Hdr => on_off(p.flag(PROFILE_FLAG_HDR)),
            ProfileField::Vrr => on_off(p.flag(PROFILE_FLAG_VRR)),
            ProfileField::NullLatency => on_off(p.flag(PROFILE_FLAG_NULL_LATENCY)),
            ProfileField::GameMode => on_off(p.flag(PROFILE_FLAG_GAME_MODE)),
            ProfileField::GpuPowerPct => {
                if p.gpu_power_pct == 0 {
                    String::from("Default")
                } else {
                    alloc::format!("{}%", p.gpu_power_pct)
                }
            }
            ProfileField::AudioSink => {
                if p.audio_sink_id == 0 {
                    String::from("System Default")
                } else {
                    alloc::format!("Device {}", p.audio_sink_id)
                }
            }
        }
    }
}

#[inline]
fn on_off(b: bool) -> String {
    String::from(if b { "On" } else { "Off" })
}

/// Step a dropdown index by `dir`, clamped to `[0, len)` (no wrap — couch
/// dropdowns stop at the ends so a held D-pad doesn't loop confusingly).
#[inline]
fn step_index(cur: usize, dir: i32, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    if dir > 0 {
        (cur + 1).min(len - 1)
    } else if dir < 0 {
        cur.saturating_sub(1)
    } else {
        cur
    }
}

/// The live state of the per-game profile editor overlay (Phase 5). `None`-less:
/// the shell holds an `Option<ProfileEditor>`; this struct exists only while the
/// editor is open over a focused game.
#[derive(Debug, Clone)]
pub struct ProfileEditor {
    /// The stable kernel profile id being edited (e.g. "raeplay:steam:730" or a
    /// "game:<id>" key the couch mints for a library entry).
    pub game_id: String,
    /// The human title shown in the editor header.
    pub title: String,
    /// The working copy of the profile being edited.
    pub profile: CouchProfile,
    /// The focused editable field (D-pad up/down moves; left/right edits).
    pub focused: usize,
    /// Set once the user confirms — the shell_runner reads this to drive the
    /// real `SYS_GAME_PROFILE_SET`. A latch, never a panic.
    pub commit_requested: bool,
}

impl ProfileEditor {
    /// Open an editor for `game_id`/`title` seeded from `profile` (the value the
    /// kernel `GET` returned, or `CouchProfile::default()` if none existed).
    #[must_use]
    pub fn new(game_id: String, title: String, profile: CouchProfile) -> Self {
        Self {
            game_id,
            title,
            profile: profile.normalized(),
            focused: 0,
            commit_requested: false,
        }
    }

    fn focused_field(&self) -> ProfileField {
        ProfileField::ORDER[self.focused.min(ProfileField::count() - 1)]
    }

    /// Move focus to the next/previous field (D-pad up/down). Clamped.
    pub fn move_focus(&mut self, dir: i32) {
        let n = ProfileField::count();
        if dir > 0 {
            self.focused = (self.focused + 1).min(n - 1);
        } else if dir < 0 {
            self.focused = self.focused.saturating_sub(1);
        }
    }

    /// Edit the focused field (D-pad left/right). Keeps the working profile
    /// normalized so it is always SET-safe.
    pub fn edit(&mut self, dir: i32) {
        let field = self.focused_field();
        field.step(&mut self.profile, dir);
        self.profile = self.profile.normalized();
    }
}

// ── GameOsShell ──────────────────────────────────────────────────────────

pub struct GameOsShell {
    pub state: GameOsState,
    pub library: Vec<GameEntry>,
    pub active_page: GameOsPage,
    pub selected_index: usize,
    pub scroll_offset: usize,
    pub controller_focus: FocusTarget,
    pub animation_tick: u64,
    pub search: GameOsSearch,
    pub recent_games: Vec<GameEntry>,
    pub featured: Vec<GameEntry>,
    pub now_playing: Option<NowPlaying>,
    pub user_profile: UserBadge,
    pub quick_menu: QuickMenu,
    pub screen_width: usize,
    pub screen_height: usize,
    pub transition: Option<PageTransition>,
    pub settings: GameOsSettings,
    pub config: GameOsConfig,
    pub active: bool,
    pub achievements_cache: Vec<Achievement>,
    pub carousel_offset: usize,
    pub carousel_timer: u64,
    sidebar_items: Vec<(GameOsPage, &'static str, char)>,
    sidebar_selected: usize,
    grid_columns: usize,
    detail_game: Option<usize>,
    settings_visible: bool,
    settings_section_idx: usize,
    /// The active controller button-glyph skin (Phase 2). Settable; defaults to
    /// `GlyphSet::DEFAULT`. Phase 3 binds the real pad's VID/PID to set it —
    /// there is no fake hardware detection here.
    glyph_set: GlyphSet,
    /// The bound pad's USB VID/PID (Phase 3), `None` until a real controller
    /// binds via `bind_pad`. Exposed for `/proc/raeen/gaming` so the bound pad
    /// and its auto-selected glyph set are introspectable. Never invented.
    bound_pad: Option<(u16, u16)>,
    /// Previous pad-button bitmap (Phase 3) — so `apply_pad_frame` fires a button
    /// action on the press EDGE only (not every frame the button is held).
    prev_pad_buttons: u32,
    /// Game Bar invoke request (Phase 4). Set when Guide is tapped while a game
    /// is running (`GameOsState::GameRunning`) — the SteamOS Guide-over-the-game
    /// idiom. The shell_runner consumes it via `take_game_bar_request()` to
    /// invoke the live `game_bar::GameBar` overlay over the running game. A
    /// boolean latch, never a panic.
    game_bar_requested: bool,
    /// The per-game profile editor overlay (Phase 5), `Some` only while open
    /// over a focused game. The surface edits a `CouchProfile` mirror; the
    /// shell_runner bridges commits/launches into the REAL `game_profile`
    /// syscalls (SET/GET/APPLY) so `/proc/raeen/games` reflects every edit.
    profile_editor: Option<ProfileEditor>,
    /// Pending launch request (Phase 5): set when the user confirms Play. The
    /// shell_runner consumes it via `take_launch_request` to APPLY the game's
    /// profile *before* the game starts (the Concept's "auto-applied"). Carries
    /// the library index so the runner can resolve the stable game id.
    launch_requested: Option<usize>,
    /// On-screen keyboard overlay (Phase 6), `Some` only while open over a
    /// focused text field. Controller-navigable QWERTY; A=type, B=backspace,
    /// Enter commits the field. Reuses the Phase-3 focus model.
    osk: Option<OskState>,
    /// Active desktop↔couch cross-fade (Phase 6), `Some` while the transition is
    /// running. The shell overlays a `bg.base` wash at `CrossFade::wash_alpha`
    /// on the couch surface so entering/leaving GameOS is a brief fade, not a
    /// hard cut. Advanced by `tick_crossfade`.
    crossfade: Option<CrossFade>,
}

impl GameOsShell {
    pub fn new(screen_width: usize, screen_height: usize) -> Self {
        let cols = (screen_width.saturating_sub(SIDEBAR_W + GRID_MARGIN * 2)) / (CARD_W + CARD_GAP);
        let cols = cols.max(2);

        Self {
            state: GameOsState::Home,
            library: Vec::new(),
            active_page: GameOsPage::Home,
            selected_index: 0,
            scroll_offset: 0,
            controller_focus: FocusTarget::GameGrid,
            animation_tick: 0,
            search: GameOsSearch {
                query: String::new(),
                results: Vec::new(),
                active: false,
            },
            recent_games: Vec::new(),
            featured: Vec::new(),
            now_playing: None,
            user_profile: UserBadge {
                username: String::from("Player"),
                avatar_char: 'P',
                online: true,
                status: String::from("Online"),
            },
            quick_menu: QuickMenu {
                visible: false,
                items: Self::default_quick_menu_items(),
                selected: 0,
            },
            screen_width,
            screen_height,
            transition: None,
            settings: GameOsSettings::new(),
            config: GameOsConfig::default(),
            active: false,
            achievements_cache: Vec::new(),
            carousel_offset: 0,
            carousel_timer: 0,
            sidebar_items: vec![
                (GameOsPage::Home, "Home", 'H'),
                (GameOsPage::AllGames, "Library", 'L'),
                (GameOsPage::RecentlyPlayed, "Recent", 'R'),
                (GameOsPage::Favorites, "Favorites", 'F'),
                (GameOsPage::Store, "Store", 'S'),
                (GameOsPage::Friends, "Friends", 'U'),
                (GameOsPage::Downloads, "Downloads", 'D'),
                (GameOsPage::Screenshots, "Screenshots", 'C'),
                (GameOsPage::Settings, "Settings", 'G'),
            ],
            sidebar_selected: 0,
            grid_columns: cols,
            detail_game: None,
            settings_visible: false,
            settings_section_idx: 0,
            glyph_set: GlyphSet::DEFAULT,
            bound_pad: None,
            prev_pad_buttons: 0,
            game_bar_requested: false,
            profile_editor: None,
            launch_requested: None,
            osk: None,
            crossfade: None,
        }
    }

    // ── On-screen keyboard (Phase 6) ───────────────────────────────────────

    /// Open the on-screen keyboard over `target`, seeded with `seed` (the
    /// field's current text). Controller-navigable; the shell renders it on top
    /// and routes input into it until it commits or is cancelled (B).
    pub fn open_osk(&mut self, target: OskTarget, seed: String) {
        self.osk = Some(OskState::new(target, seed));
    }

    /// Open the library search field AND its on-screen keyboard (Phase 6). The
    /// player drives the OSK with the pad to type the query; Enter commits it +
    /// runs the search. Seeds the OSK with any existing query.
    fn open_search_osk(&mut self) {
        self.search.active = true;
        self.controller_focus = FocusTarget::SearchBar;
        let seed = self.search.query.clone();
        self.open_osk(OskTarget::Search, seed);
    }

    /// True while the OSK is open (it owns input + draws on top of everything).
    #[must_use]
    pub fn osk_open(&self) -> bool {
        self.osk.is_some()
    }

    /// Borrow the open OSK (for the smoketest / introspection).
    #[must_use]
    pub fn osk(&self) -> Option<&OskState> {
        self.osk.as_ref()
    }

    /// Route a button into the open OSK (Phase 6). Returns `true` if the OSK
    /// consumed it (so `handle_button` can early-return). D-pad moves the key
    /// cursor (the Phase-3 focus model), A types the focused key, B is
    /// backspace, Y toggles shift, Start commits. A committed/closed OSK runs
    /// the field action via `commit_osk`. Never panics.
    fn handle_osk_button(&mut self, btn: GamepadButton) -> bool {
        let osk = match self.osk.as_mut() {
            Some(o) => o,
            None => return false,
        };
        match btn {
            GamepadButton::DPadUp => osk.move_focus(0, -1),
            GamepadButton::DPadDown => osk.move_focus(0, 1),
            GamepadButton::DPadLeft => osk.move_focus(-1, 0),
            GamepadButton::DPadRight => osk.move_focus(1, 0),
            GamepadButton::A => osk.activate(),
            GamepadButton::X | GamepadButton::B => {
                // X = backspace (the spec's OSK map); B doubles as backspace
                // here too (the couch's "cancel = step back a char") — close the
                // OSK only when the buffer is already empty so an accidental B
                // doesn't lose the whole query.
                if btn == GamepadButton::B && osk.text.is_empty() {
                    self.osk = None;
                    return true;
                }
                osk.backspace();
            }
            GamepadButton::Y => osk.shift = !osk.shift,
            GamepadButton::Start => osk.commit_requested = true,
            _ => {}
        }
        // If a commit was latched (Enter key or Start), apply it now.
        let commit = self
            .osk
            .as_ref()
            .map(|o| o.commit_requested)
            .unwrap_or(false);
        if commit {
            self.commit_osk();
        }
        true
    }

    /// Apply the OSK's typed text to its target field and close the OSK. For the
    /// search target this sets the query + runs the search. Never panics.
    fn commit_osk(&mut self) {
        if let Some(osk) = self.osk.take() {
            match osk.target {
                OskTarget::Search => {
                    let q = osk.text.clone();
                    self.search(&q);
                }
            }
        }
    }

    // ── Desktop ↔ couch cross-fade (Phase 6) ───────────────────────────────

    /// Begin a cross-fade transition. `entering` = desktop→couch (else the
    /// reverse). The shell overlays the `bg.base` wash while it runs.
    pub fn begin_crossfade(&mut self, entering: bool) {
        self.crossfade = Some(CrossFade::begin(entering));
    }

    /// Advance the active cross-fade by `dt_ms`; drops it when complete. Returns
    /// `true` while a fade is still running (the caller keeps repainting).
    pub fn tick_crossfade(&mut self, dt_ms: u64) -> bool {
        if let Some(cf) = self.crossfade.as_mut() {
            if !cf.tick(dt_ms) {
                self.crossfade = None;
                return false;
            }
            return true;
        }
        false
    }

    /// The cross-fade wash color for this frame, `None` when no fade is running.
    #[must_use]
    pub fn crossfade_wash(&self) -> Option<u32> {
        self.crossfade.map(|cf| cf.wash_color())
    }

    /// True while a cross-fade is running (for `/proc/raeen/gaming`).
    #[must_use]
    pub fn crossfade_active(&self) -> bool {
        self.crossfade.is_some()
    }

    // ── Per-game profile editor (Phase 5) ─────────────────────────────────

    /// A stable kernel profile id for a library entry — the key the couch SETs /
    /// GETs / APPLYs against. Uses the game's numeric id so the surface and the
    /// kernel agree on one record per game. (RaePlay's store-qualified ids
    /// supersede this once the library is fed from the live connectors.)
    #[must_use]
    pub fn profile_id_for(game: &GameEntry) -> String {
        alloc::format!("game:{}", game.id)
    }

    /// Open the per-game profile editor over the focused game, seeded from a
    /// profile the caller fetched via `SYS_GAME_PROFILE_GET` (or
    /// `CouchProfile::default()` when the game has no stored profile yet). The
    /// shell_runner calls this so the seed is the REAL kernel record.
    pub fn open_profile_editor(&mut self, game_index: usize, seed: CouchProfile) {
        let games = self.visible_games();
        if game_index >= games.len() {
            return;
        }
        let game = &games[game_index];
        let id = Self::profile_id_for(game);
        let title = game.title.clone();
        self.profile_editor = Some(ProfileEditor::new(id, title, seed));
    }

    /// True while the profile editor is open (it owns input + draws on top).
    #[must_use]
    pub fn profile_editor_open(&self) -> bool {
        self.profile_editor.is_some()
    }

    /// Borrow the open editor (for the shell_runner bridge / introspection).
    #[must_use]
    pub fn profile_editor(&self) -> Option<&ProfileEditor> {
        self.profile_editor.as_ref()
    }

    /// Focus the named profile field and step its value by `dir` in the open
    /// editor (a convenience the shell_runner / smoketest uses to drive a
    /// specific field without exposing the private field). No-op if closed.
    pub fn edit_profile_field(&mut self, field: ProfileField, dir: i32) {
        if let Some(e) = self.profile_editor.as_mut() {
            e.focused = ProfileField::ORDER
                .iter()
                .position(|&f| f == field)
                .unwrap_or(0);
            e.edit(dir);
        }
    }

    /// Latch a commit on the open editor (equivalent to the user pressing A).
    pub fn request_profile_commit(&mut self) {
        if let Some(e) = self.profile_editor.as_mut() {
            e.commit_requested = true;
        }
    }

    /// Consume a pending profile-commit request (Phase 5). Returns the
    /// `(game_id, CouchProfile)` to push through `SYS_GAME_PROFILE_SET` once per
    /// confirm, then clears the latch and closes the editor. The shell_runner
    /// polls this each frame.
    pub fn take_profile_commit(&mut self) -> Option<(String, CouchProfile)> {
        let take = self
            .profile_editor
            .as_ref()
            .map(|e| e.commit_requested)
            .unwrap_or(false);
        if take {
            let editor = self.profile_editor.take()?;
            return Some((editor.game_id, editor.profile));
        }
        None
    }

    /// Consume a pending launch request (Phase 5). Returns the library index of
    /// the game to launch once per confirm; the shell_runner resolves its
    /// stable id and calls `SYS_GAME_PROFILE_APPLY` *before* starting the game
    /// (auto-apply on launch). A missing profile is not an error — the runner
    /// applies defaults and never blocks launch.
    pub fn take_launch_request(&mut self) -> Option<usize> {
        core::mem::take(&mut self.launch_requested)
    }

    /// Route a button into the open profile editor. Returns `true` if the editor
    /// consumed it (so `handle_button` can early-return). D-pad up/down moves the
    /// focused field, left/right edits it, A confirms (latches the commit), B
    /// cancels (discards). Never panics.
    fn handle_profile_editor_button(&mut self, btn: GamepadButton) -> bool {
        let editor = match self.profile_editor.as_mut() {
            Some(e) => e,
            None => return false,
        };
        match btn {
            GamepadButton::DPadDown => editor.move_focus(1),
            GamepadButton::DPadUp => editor.move_focus(-1),
            GamepadButton::DPadRight => editor.edit(1),
            GamepadButton::DPadLeft => editor.edit(-1),
            GamepadButton::A | GamepadButton::Start => {
                // Confirm: latch the commit; the shell_runner drives the real
                // SYS_GAME_PROFILE_SET and closes the editor via take_profile_commit.
                editor.commit_requested = true;
            }
            GamepadButton::B => {
                // Cancel: discard the working copy, close the editor.
                self.profile_editor = None;
            }
            _ => {}
        }
        true
    }

    /// Consume a pending Game Bar invoke request (Phase 4). Returns `true` once
    /// per Guide-tap-while-running, then clears the latch. The shell_runner polls
    /// this to toggle the live Game Bar overlay over the running game.
    pub fn take_game_bar_request(&mut self) -> bool {
        core::mem::take(&mut self.game_bar_requested)
    }

    /// The active controller glyph skin (Phase 2). Read by the hint bar +
    /// smoketest + `/proc/raeen/gaming`.
    #[must_use]
    pub fn glyph_set(&self) -> GlyphSet {
        self.glyph_set
    }

    /// Set the active controller glyph skin. Phase 3 calls this from the live
    /// HID bind (pad VID/PID → set); for now it is a plain settable field — no
    /// fake hardware detection.
    pub fn set_glyph_set(&mut self, set: GlyphSet) {
        self.glyph_set = set;
    }

    /// Bind a live controller (Phase 3): record its USB VID/PID and auto-select
    /// the matching button-glyph skin (`glyph_set_for_vidpid`). Called by the
    /// kernel when `hid_gamepad` binds a pad on an xHCI interrupt-IN endpoint.
    /// The only inputs are the device's REAL VID/PID — no fake detection.
    pub fn bind_pad(&mut self, vid: u16, pid: u16) {
        self.bound_pad = Some((vid, pid));
        self.set_glyph_set(glyph_set_for_vidpid(vid, pid));
        // Reset the press-edge tracker so the first real frame fires cleanly.
        self.prev_pad_buttons = 0;
    }

    /// The bound pad's USB VID/PID, `None` until a real controller binds. Read by
    /// `/proc/raeen/gaming`.
    #[must_use]
    pub fn bound_pad(&self) -> Option<(u16, u16)> {
        self.bound_pad
    }

    /// Drive couch navigation from a decoded pad frame (Phase 3 — the live
    /// controller bind). Translates the frame into the SAME `GamepadInput`
    /// events the keyboard path produces: hat / left-stick → directional focus
    /// movement; face/shoulder/system buttons → Select/Back/Details/Search/Menu.
    /// Button actions fire on the press EDGE (a held button does not repeat the
    /// action). Never panics on a malformed frame (saturating, bounded). Keyboard
    /// nav is untouched — this augments it.
    pub fn apply_pad_frame(&mut self, frame: &PadFrame) {
        // 1. Directional focus from the hat / left stick.
        let (dx, dy) = focus_delta_for_frame(frame);
        match dx {
            d if d < 0 => self.handle_button(GamepadButton::DPadLeft),
            d if d > 0 => self.handle_button(GamepadButton::DPadRight),
            _ => {}
        }
        match dy {
            d if d < 0 => self.handle_button(GamepadButton::DPadUp),
            d if d > 0 => self.handle_button(GamepadButton::DPadDown),
            _ => {}
        }

        // 2. Face / shoulder / system buttons on the PRESS edge only.
        let pressed = frame.buttons & !self.prev_pad_buttons;
        for bit in 0..32u32 {
            if pressed & (1 << bit) != 0 {
                if let Some(btn) = button_for_bit(bit) {
                    self.handle_button(btn);
                }
            }
        }
        self.prev_pad_buttons = frame.buttons;
    }

    /// The context-sensitive hint chips for the CURRENT couch context (focus
    /// target / overlay / detail). The set of actions reflects what the user can
    /// do right now — the SteamOS context-bar idiom. Glyphs come from the active
    /// `GlyphSet`. Pure logic, host-KAT'd.
    #[must_use]
    pub fn context_hints(&self) -> Vec<HintChip> {
        // Choose the logical action list per context, then resolve each glyph
        // through the active set. Order is left→right as rendered.
        let actions: &[HintAction] = if self.osk.is_some() {
            // OSK context: A types, B backspaces, Y is the shift layer, Start
            // commits (Menu glyph) — the controller-text-entry contract.
            &[
                HintAction::Select,
                HintAction::Back,
                HintAction::Favorite,
                HintAction::Menu,
            ]
        } else if self.quick_menu.visible {
            &[HintAction::Select, HintAction::Back]
        } else if self.search.active {
            &[HintAction::Select, HintAction::Back]
        } else if self.detail_game.is_some() {
            &[HintAction::Select, HintAction::Favorite, HintAction::Back]
        } else {
            match self.controller_focus {
                FocusTarget::GameGrid => &[
                    HintAction::Select,
                    HintAction::Details,
                    HintAction::Search,
                    HintAction::Menu,
                    HintAction::Back,
                ],
                FocusTarget::Sidebar => &[HintAction::Select, HintAction::Search, HintAction::Menu],
                FocusTarget::TopBar | FocusTarget::SearchBar | FocusTarget::QuickMenu => {
                    &[HintAction::Select, HintAction::Back, HintAction::Menu]
                }
            }
        };
        let set = self.glyph_set;
        actions
            .iter()
            .map(|&action| HintChip {
                action,
                glyph: set.glyph(action),
                label: action.label(),
            })
            .collect()
    }

    fn default_quick_menu_items() -> Vec<QuickMenuItem> {
        vec![
            QuickMenuItem {
                label: String::from("Brightness"),
                icon: 'B',
                action: QuickAction::Brightness(80),
            },
            QuickMenuItem {
                label: String::from("Volume"),
                icon: 'V',
                action: QuickAction::Volume(75),
            },
            QuickMenuItem {
                label: String::from("Wi-Fi"),
                icon: 'W',
                action: QuickAction::WiFi,
            },
            QuickMenuItem {
                label: String::from("Bluetooth"),
                icon: 'T',
                action: QuickAction::Bluetooth,
            },
            QuickMenuItem {
                label: String::from("Do Not Disturb"),
                icon: 'N',
                action: QuickAction::DoNotDisturb,
            },
            QuickMenuItem {
                label: String::from("Performance"),
                icon: 'P',
                action: QuickAction::Performance,
            },
            QuickMenuItem {
                label: String::from("Screenshot"),
                icon: 'S',
                action: QuickAction::Screenshot,
            },
            QuickMenuItem {
                label: String::from("Recording"),
                icon: 'R',
                action: QuickAction::Recording,
            },
            QuickMenuItem {
                label: String::from("Friends"),
                icon: 'F',
                action: QuickAction::FriendsOnline,
            },
            QuickMenuItem {
                label: String::from("Downloads"),
                icon: 'D',
                action: QuickAction::Downloads,
            },
            QuickMenuItem {
                label: String::from("Sleep"),
                icon: 'Z',
                action: QuickAction::Sleep,
            },
            QuickMenuItem {
                label: String::from("Shutdown"),
                icon: 'X',
                action: QuickAction::Shutdown,
            },
            QuickMenuItem {
                label: String::from("Desktop Mode"),
                icon: 'M',
                action: QuickAction::DesktopMode,
            },
        ]
    }

    // ── Navigation ───────────────────────────────────────────────────────

    pub fn navigate(&mut self, page: GameOsPage) {
        let from = self.active_page;
        self.transition = Some(PageTransition {
            from,
            to: page,
            progress: 0.0,
            duration_ms: 250,
        });
        self.active_page = page;
        self.selected_index = 0;
        self.scroll_offset = 0;
        self.detail_game = None;

        self.state = match page {
            GameOsPage::Store => GameOsState::Store,
            GameOsPage::Settings => GameOsState::Settings,
            _ => GameOsState::Home,
        };
    }

    pub fn select_game(&mut self, index: usize) {
        let games = self.visible_games();
        if index < games.len() {
            self.selected_index = index;
            self.detail_game = Some(index);
        }
    }

    pub fn launch_game(&mut self, index: usize) -> Option<&GameEntry> {
        let can_launch = {
            let games = self.visible_games();
            index < games.len() && games[index].installed
        };
        if can_launch {
            // Phase 5: record the launch so the shell_runner APPLYs this game's
            // profile *before* the game starts (the Concept's "auto-applied").
            // A missing profile is not an error — the runner applies defaults.
            self.launch_requested = Some(index);
            self.state = GameOsState::GameRunning;
            return Some(&self.visible_games()[index]);
        }
        None
    }

    fn visible_games(&self) -> &[GameEntry] {
        match self.active_page {
            GameOsPage::Home => &self.featured,
            GameOsPage::RecentlyPlayed => &self.recent_games,
            GameOsPage::Favorites => {
                // Can't filter borrowing self, return full library;
                // real impl would maintain a separate vec
                &self.library
            }
            GameOsPage::AllGames | _ => &self.library,
        }
    }

    // ── Controller input ─────────────────────────────────────────────────

    pub fn controller_input(&mut self, input: GamepadInput) {
        match input {
            GamepadInput::Button(btn) => self.handle_button(btn),
            GamepadInput::LeftStick { x, y } => {
                const DEADZONE: f32 = 0.3;
                if y < -DEADZONE {
                    self.handle_button(GamepadButton::DPadUp);
                } else if y > DEADZONE {
                    self.handle_button(GamepadButton::DPadDown);
                }
                if x < -DEADZONE {
                    self.handle_button(GamepadButton::DPadLeft);
                } else if x > DEADZONE {
                    self.handle_button(GamepadButton::DPadRight);
                }
            }
            GamepadInput::RightStick { .. } => {}
            GamepadInput::LeftTrigger(v) if v > 0.5 => {
                self.scroll_offset = self.scroll_offset.saturating_sub(self.grid_columns);
            }
            GamepadInput::RightTrigger(v) if v > 0.5 => {
                let game_count = self.visible_games().len();
                let max_scroll = game_count.saturating_sub(self.grid_columns);
                self.scroll_offset = (self.scroll_offset + self.grid_columns).min(max_scroll);
            }
            _ => {}
        }
    }

    fn handle_button(&mut self, btn: GamepadButton) {
        // Phase 4: Guide tapped while a game is running invokes the Game Bar
        // overlay (the SteamOS "press Guide over the game" idiom) — the
        // shell_runner consumes the latch via `take_game_bar_request`.
        if self.state == GameOsState::GameRunning && btn == GamepadButton::Guide {
            self.game_bar_requested = true;
            return;
        }

        // Phase 6: the on-screen keyboard owns input while open (it draws on top
        // of everything). D-pad moves the key cursor, A types, B/X backspace,
        // Y shift, Start commits. It reuses the Phase-3 focus model.
        if self.osk.is_some() {
            self.handle_osk_button(btn);
            return;
        }

        // Phase 5: the per-game profile editor owns input while open (it draws on
        // top of the detail view). Confirm/cancel are handled inside; the
        // shell_runner bridges a confirmed edit into SYS_GAME_PROFILE_SET.
        if self.profile_editor.is_some() {
            self.handle_profile_editor_button(btn);
            return;
        }

        // Phase 5: in the game-detail view, Y/North opens the per-game profile
        // editor for the focused game (the "⚙ Profile" action). The shell_runner
        // seeds it from the REAL kernel record via SYS_GAME_PROFILE_GET; here we
        // just request it with a default seed so a standalone surface still works.
        if self.detail_game.is_some() && btn == GamepadButton::Y {
            if let Some(idx) = self.detail_game {
                self.open_profile_editor(idx, CouchProfile::default());
            }
            return;
        }

        if self.quick_menu.visible {
            self.handle_quick_menu_button(btn);
            return;
        }

        if self.search.active {
            match btn {
                GamepadButton::B => {
                    self.search.active = false;
                }
                GamepadButton::DPadDown => {
                    if !self.search.results.is_empty() {
                        self.selected_index = (self.selected_index + 1) % self.search.results.len();
                    }
                }
                GamepadButton::DPadUp => {
                    if !self.search.results.is_empty() {
                        let len = self.search.results.len();
                        self.selected_index = self.selected_index.checked_sub(1).unwrap_or(len - 1);
                    }
                }
                _ => {}
            }
            return;
        }

        match self.controller_focus {
            FocusTarget::Sidebar => match btn {
                GamepadButton::DPadDown => {
                    self.sidebar_selected = (self.sidebar_selected + 1) % self.sidebar_items.len();
                }
                GamepadButton::DPadUp => {
                    let len = self.sidebar_items.len();
                    self.sidebar_selected = self.sidebar_selected.checked_sub(1).unwrap_or(len - 1);
                }
                GamepadButton::A | GamepadButton::DPadRight => {
                    let page = self.sidebar_items[self.sidebar_selected].0;
                    if page == GameOsPage::Settings {
                        self.toggle_settings();
                    } else {
                        self.navigate(page);
                        self.controller_focus = FocusTarget::GameGrid;
                    }
                }
                GamepadButton::Guide => self.toggle_quick_menu(),
                GamepadButton::Y => self.open_search_osk(),
                _ => {}
            },
            FocusTarget::GameGrid => {
                let game_count = self.visible_games().len();
                if game_count == 0 {
                    match btn {
                        GamepadButton::DPadLeft => {
                            self.controller_focus = FocusTarget::Sidebar;
                        }
                        GamepadButton::Guide => self.toggle_quick_menu(),
                        _ => {}
                    }
                    return;
                }

                match btn {
                    GamepadButton::DPadRight => {
                        if (self.selected_index + 1) % self.grid_columns != 0 {
                            self.selected_index = (self.selected_index + 1).min(game_count - 1);
                        }
                    }
                    GamepadButton::DPadLeft => {
                        if self.selected_index % self.grid_columns == 0 {
                            self.controller_focus = FocusTarget::Sidebar;
                        } else {
                            self.selected_index = self.selected_index.saturating_sub(1);
                        }
                    }
                    GamepadButton::DPadDown => {
                        let next = self.selected_index + self.grid_columns;
                        if next < game_count {
                            self.selected_index = next;
                        }
                    }
                    GamepadButton::DPadUp => {
                        if self.selected_index >= self.grid_columns {
                            self.selected_index -= self.grid_columns;
                        } else {
                            self.controller_focus = FocusTarget::TopBar;
                        }
                    }
                    GamepadButton::A => {
                        self.select_game(self.selected_index);
                    }
                    GamepadButton::X => {
                        self.launch_game(self.selected_index);
                    }
                    GamepadButton::Y => self.open_search_osk(),
                    GamepadButton::Guide => self.toggle_quick_menu(),
                    GamepadButton::B => {
                        if self.detail_game.is_some() {
                            self.detail_game = None;
                        } else {
                            self.controller_focus = FocusTarget::Sidebar;
                        }
                    }
                    GamepadButton::Start => {
                        if self.detail_game.is_some() {
                            self.detail_game = None;
                        }
                    }
                    _ => {}
                }

                self.ensure_selected_visible();
            }
            FocusTarget::TopBar => match btn {
                GamepadButton::DPadDown => {
                    self.controller_focus = FocusTarget::GameGrid;
                }
                GamepadButton::Y => self.open_search_osk(),
                GamepadButton::Guide => self.toggle_quick_menu(),
                _ => {}
            },
            FocusTarget::SearchBar => match btn {
                GamepadButton::B => {
                    self.search.active = false;
                    self.controller_focus = FocusTarget::GameGrid;
                }
                GamepadButton::Guide => self.toggle_quick_menu(),
                _ => {}
            },
            FocusTarget::QuickMenu => {
                self.handle_quick_menu_button(btn);
            }
        }
    }

    fn ensure_selected_visible(&mut self) {
        let row = self.selected_index / self.grid_columns;
        let visible_rows = self.content_height() / (CARD_H + CARD_GAP);
        let scroll_row = self.scroll_offset / self.grid_columns;

        if row < scroll_row {
            self.scroll_offset = row * self.grid_columns;
        } else if row >= scroll_row + visible_rows {
            self.scroll_offset = (row - visible_rows + 1) * self.grid_columns;
        }
    }

    fn content_height(&self) -> usize {
        self.screen_height.saturating_sub(TOPBAR_H + 32)
    }

    // ── Quick menu ───────────────────────────────────────────────────────

    pub fn toggle_quick_menu(&mut self) {
        self.quick_menu.visible = !self.quick_menu.visible;
        if self.quick_menu.visible {
            self.quick_menu.selected = 0;
            self.controller_focus = FocusTarget::QuickMenu;
        } else {
            self.controller_focus = FocusTarget::GameGrid;
        }
    }

    fn handle_quick_menu_button(&mut self, btn: GamepadButton) {
        let count = self.quick_menu.items.len();
        match btn {
            GamepadButton::DPadDown => {
                if count > 0 {
                    self.quick_menu.selected = (self.quick_menu.selected + 1) % count;
                }
            }
            GamepadButton::DPadUp => {
                if count > 0 {
                    self.quick_menu.selected =
                        self.quick_menu.selected.checked_sub(1).unwrap_or(count - 1);
                }
            }
            GamepadButton::A => {
                // Action would be dispatched to the system here
            }
            GamepadButton::B | GamepadButton::Guide => {
                self.toggle_quick_menu();
            }
            _ => {}
        }
    }

    // ── Search ───────────────────────────────────────────────────────────

    pub fn search(&mut self, query: &str) {
        self.search.query.clear();
        self.search.query.push_str(query);

        self.search.results.clear();
        for game in &self.library {
            if game.title.contains(query) {
                self.search.results.push(game.clone());
            }
        }
        self.search.active = true;
        self.selected_index = 0;
    }

    // ── Rendering ────────────────────────────────────────────────────────

    pub fn render(&self, canvas: &mut raegfx::Canvas) {
        canvas.fill_rect(0, 0, self.screen_width, self.screen_height, BG_DARK);

        self.render_sidebar(canvas);
        self.render_top_bar(canvas);

        let content_x = SIDEBAR_W;
        let content_y = TOPBAR_H;

        match self.active_page {
            GameOsPage::Home => self.render_home(canvas, content_x, content_y),
            GameOsPage::AllGames | GameOsPage::RecentlyPlayed | GameOsPage::Favorites => {
                self.render_library(canvas, content_x, content_y);
            }
            _ => self.render_library(canvas, content_x, content_y),
        }

        if let Some(idx) = self.detail_game {
            self.render_game_detail(canvas, idx);
        }

        // Phase 5: the per-game profile editor draws on top of the detail view.
        if self.profile_editor.is_some() {
            self.render_profile_editor(canvas);
        }

        if self.quick_menu.visible {
            self.render_quick_menu(canvas);
        }

        if let Some(ref np) = self.now_playing {
            self.render_now_playing(canvas, np);
        }

        if self.settings_visible {
            self.render_settings(canvas);
        }

        if self.search.active {
            self.render_search_overlay(canvas);
        }

        // Phase 6: the on-screen keyboard draws on top of the search overlay
        // (and everything else) while open — it owns input + the screen.
        if self.osk.is_some() {
            self.render_osk(canvas);
        }

        // Persistent context hint bar (Phase 2 — the SteamOS staple). Always
        // last so it sits above the content; the chips reflect the CURRENT
        // context (grid / detail / quick-menu / search) and the active glyph set.
        self.render_hint_bar(canvas);

        // Phase 6: the desktop↔couch cross-fade wash is the VERY last layer — a
        // `bg.base` veil at the fade's current alpha over the whole couch
        // surface, so entering/leaving GameOS reads as one environment shifting.
        // Alpha 0 (the fade endpoints) draws nothing.
        if let Some(wash) = self.crossfade_wash() {
            if (wash >> 24) & 0xFF != 0 {
                canvas.fill_rounded_rect(0, 0, self.screen_width, self.screen_height, 0, wash);
            }
        }
    }

    /// Render the on-screen keyboard overlay (Phase 6) — the Concept's
    /// controller-first text entry. A glass panel anchored to the bottom of the
    /// screen with a text-preview row and the QWERTY + action key grid. The
    /// focused key (Phase-3 focus model) lifts with the accent focus ring; keys
    /// are ≥`HIT_TARGET_COUCH` (48px, the TV hit-target floor); glyphs are crisp
    /// AA (RaeSans), never the 8px block font. Never panics (clamped layout).
    pub fn render_osk(&self, canvas: &mut raegfx::Canvas) {
        let osk = match self.osk.as_ref() {
            Some(o) => o,
            None => return,
        };
        let acc = accent();

        // Panel: a bottom-anchored band wide enough for 10 keys + margins.
        let key = HIT_TARGET_COUCH as usize;
        let key_gap = SPACE_2 as usize;
        let cols = OSK_ROWS.iter().map(|r| r.len()).max().unwrap_or(10);
        let grid_w = cols * key + (cols.saturating_sub(1)) * key_gap;
        let panel_w = (grid_w + SPACE_5 as usize * 2).min(self.screen_width);
        let preview_h = ROW_H;
        let rows = OSK_ROWS.len();
        let grid_h = rows * key + (rows.saturating_sub(1)) * key_gap;
        let panel_h = preview_h + grid_h + SPACE_5 as usize * 2 + SPACE_3 as usize;
        let px = (self.screen_width.saturating_sub(panel_w)) / 2;
        // Anchor above the hint bar so both stay legible.
        let hint_bar = HIT_TARGET_COUCH as usize + SPACE_4 as usize;
        let py = self
            .screen_height
            .saturating_sub(panel_h + hint_bar + SPACE_3 as usize);

        // Glass panel (elev.3 modal) — rounded radius.lg.
        canvas.fill_rounded_rect(px, py, panel_w, panel_h, RADIUS_LG as usize, BG_CARD);
        canvas.draw_rounded_rect_outline(px, py, panel_w, panel_h, RADIUS_LG as usize, acc.base);

        // Text preview row: the working buffer + a caret. Shows what is typed so
        // far in the field (crisp AA, couch subtitle ramp).
        let preview_x = px + SPACE_5 as usize;
        let preview_y = py + SPACE_4 as usize;
        canvas.fill_rounded_rect(
            preview_x,
            preview_y,
            panel_w.saturating_sub(SPACE_5 as usize * 2),
            preview_h,
            RADIUS_MD as usize,
            BG_DARK,
        );
        let preview_text = if osk.text.is_empty() {
            "Type to search..."
        } else {
            &osk.text
        };
        let preview_color = if osk.text.is_empty() {
            TEXT_DIMMED
        } else {
            TEXT_PRIMARY
        };
        let max_chars = (panel_w.saturating_sub(SPACE_8 as usize)) / GLYPH_W.max(1);
        let disp = crate::text_util::truncate_chars(preview_text, max_chars);
        couch_text(
            canvas,
            preview_x + SPACE_3 as usize,
            preview_y + (preview_h.saturating_sub(TYPE_COUCH_SUBTITLE.line_height as usize)) / 2,
            disp,
            TYPE_COUCH_SUBTITLE,
            preview_color,
        );

        // Key grid. Each row is centred under the widest row; the focused key
        // lifts with the accent focus ring (the four-redundant-signal a11y cue).
        let grid_x = px + (panel_w.saturating_sub(grid_w)) / 2;
        let grid_y = preview_y + preview_h + SPACE_3 as usize;
        for (ri, row) in OSK_ROWS.iter().enumerate() {
            let ky = grid_y + ri * (key + key_gap);
            // Wider keys for the action row (Space/Enter): give each row its own
            // even spread within the grid width.
            let row_keys = row.len();
            let row_w = row_keys * key + (row_keys.saturating_sub(1)) * key_gap;
            let row_x = px + (panel_w.saturating_sub(row_w)) / 2;
            for (ci, &k) in row.iter().enumerate() {
                let kx = row_x + ci * (key + key_gap);
                let focused = ri == osk.row && ci == osk.col;
                let fill = if focused { acc.subtle } else { BG_SELECTED };
                canvas.fill_rounded_rect(kx, ky, key, key, RADIUS_MD as usize, fill);
                let label = k.label(osk.shift);
                let lw = couch_text_w(canvas, label, TYPE_COUCH_LABEL);
                let lx = kx + (key.saturating_sub(lw)) / 2;
                let ly = ky + (key.saturating_sub(TYPE_COUCH_LABEL.line_height as usize)) / 2;
                let lc = if focused {
                    TEXT_PRIMARY
                } else {
                    TEXT_SECONDARY
                };
                couch_text(canvas, lx, ly, label, TYPE_COUCH_LABEL, lc);
                if focused {
                    draw_focus_ring(canvas, kx, ky, key, key, &acc);
                }
            }
        }
    }

    /// Render the bottom context hint bar: token-styled glass chips, each a
    /// circular glyph chip (from the active `GlyphSet`) + an AA action label.
    /// Context-aware via `context_hints()`. Never panics (char-safe truncation,
    /// saturating layout).
    pub fn render_hint_bar(&self, canvas: &mut raegfx::Canvas) {
        let chips = self.context_hints();
        if chips.is_empty() {
            return;
        }
        let acc = accent();

        // Bar geometry: a full-width band at the very bottom, ≥ HIT_TARGET_COUCH.
        let bar_h = HIT_TARGET_COUCH as usize + SPACE_4 as usize;
        let bar_y = self.screen_height.saturating_sub(bar_h);
        // Glass-ish band: bg.overlay fill + a top hairline (stroke.subtle).
        canvas.fill_rect(0, bar_y, self.screen_width, bar_h, BG_SIDEBAR);
        for x in 0..self.screen_width {
            canvas.draw_pixel(x, bar_y, SEPARATOR);
        }

        // Each chip = [ circular glyph chip ][ gap ][ label ][ inter-chip gap ].
        let chip_d = HIT_TARGET_COUCH as usize - SPACE_3 as usize; // glyph circle ø
        let inner_gap = SPACE_2 as usize;
        let chip_gap = SPACE_5 as usize;
        let label_style = TYPE_COUCH_LABEL;
        let cy = bar_y + (bar_h.saturating_sub(chip_d)) / 2;
        let label_cy = bar_y + (bar_h.saturating_sub(label_style.line_height as usize)) / 2;

        let mut x = GRID_MARGIN;
        for chip in &chips {
            // Width budget guard: stop before overrunning the right edge.
            let glyph_w = couch_text_w(canvas, chip.glyph, label_style).max(1);
            let label_w = couch_text_w(canvas, chip.label, label_style);
            let advance = chip_d + inner_gap + label_w + chip_gap;
            if x + chip_d + inner_gap + label_w > self.screen_width.saturating_sub(SPACE_4 as usize)
            {
                break;
            }

            // Circular accent chip with the button glyph centred (crisp AA, NOT
            // the 8px block font). The pill radius == half the diameter.
            canvas.fill_rounded_rect(x, cy, chip_d, chip_d, chip_d / 2, acc.subtle);
            canvas.draw_rounded_rect_outline(x, cy, chip_d, chip_d, chip_d / 2, acc.base);
            let gx = x + (chip_d.saturating_sub(glyph_w)) / 2;
            let gy = cy + (chip_d.saturating_sub(label_style.line_height as usize)) / 2;
            couch_text(canvas, gx, gy, chip.glyph, label_style, TEXT_PRIMARY);

            // Action label to the right of the chip.
            couch_text(
                canvas,
                x + chip_d + inner_gap,
                label_cy,
                chip.label,
                label_style,
                TEXT_SECONDARY,
            );

            x += advance;
        }
    }

    pub fn render_sidebar(&self, canvas: &mut raegfx::Canvas) {
        let acc = accent();
        canvas.fill_rect(0, 0, SIDEBAR_W, self.screen_height, BG_SIDEBAR);

        // Separator line
        for y in 0..self.screen_height {
            canvas.draw_pixel(SIDEBAR_W - 1, y, SEPARATOR);
        }

        // User badge at top — avatar dot + name + status (couch ramp).
        let pad = SPACE_5 as usize;
        let badge_y = pad;
        let avatar_color = if self.user_profile.online {
            GREEN
        } else {
            TEXT_DIMMED
        };
        canvas.fill_rounded_rect(
            pad,
            badge_y,
            SPACE_4 as usize,
            SPACE_4 as usize,
            RADIUS_MD as usize,
            avatar_color,
        );
        let name_x = pad + SPACE_4 as usize + SPACE_3 as usize;
        couch_text(
            canvas,
            name_x,
            badge_y,
            &self.user_profile.username,
            TYPE_COUCH_LABEL,
            TEXT_PRIMARY,
        );
        let status_color = if self.user_profile.online {
            GREEN
        } else {
            TEXT_SECONDARY
        };
        couch_text(
            canvas,
            name_x,
            badge_y + TYPE_COUCH_LABEL.line_height as usize,
            &self.user_profile.status,
            TYPE_COUCH_CAPTION,
            status_color,
        );

        // Separator below badge
        let sep_y = badge_y + ROW_H + SPACE_2 as usize;
        for x in pad..SIDEBAR_W - pad {
            canvas.draw_pixel(x, sep_y, SEPARATOR);
        }

        // Nav items — each row ≥ HIT_TARGET_COUCH (48px).
        let items_start = sep_y + SPACE_4 as usize;
        let item_height = ROW_H;
        let is_sidebar_focused = self.controller_focus == FocusTarget::Sidebar;
        let label_pad = SPACE_4 as usize;

        for (i, &(page, label, _icon)) in self.sidebar_items.iter().enumerate() {
            let iy = items_start + i * item_height;

            let is_active = self.active_page == page;
            let is_selected = is_sidebar_focused && i == self.sidebar_selected;

            if is_selected {
                canvas.fill_rounded_rect(
                    SPACE_2 as usize,
                    iy,
                    SIDEBAR_W - SPACE_4 as usize,
                    item_height,
                    RADIUS_MD as usize,
                    acc.subtle,
                );
                // Left accent bar.
                canvas.fill_rect(SPACE_2 as usize, iy, FOCUS_RING_W, item_height, acc.base);
            } else if is_active {
                canvas.fill_rect(SPACE_2 as usize, iy, FOCUS_RING_W, item_height, acc.active);
            }

            let text_color = if is_active || is_selected {
                TEXT_PRIMARY
            } else {
                TEXT_SECONDARY
            };

            let text_y =
                iy + (item_height.saturating_sub(TYPE_COUCH_SUBTITLE.line_height as usize)) / 2;
            couch_text(
                canvas,
                label_pad,
                text_y,
                label,
                TYPE_COUCH_SUBTITLE,
                text_color,
            );
        }

        // Bottom section: quick-access hint
        let hint_y = self.screen_height - ROW_H - SPACE_5 as usize;
        for x in pad..SIDEBAR_W - pad {
            canvas.draw_pixel(x, hint_y, SEPARATOR);
        }
        couch_text(
            canvas,
            pad,
            hint_y + SPACE_3 as usize,
            "(Menu) Guide   (Search) Y",
            TYPE_COUCH_CAPTION,
            TEXT_DIMMED,
        );
    }

    pub fn render_top_bar(&self, canvas: &mut raegfx::Canvas) {
        canvas.fill_rect(
            SIDEBAR_W,
            0,
            self.screen_width - SIDEBAR_W,
            TOPBAR_H,
            TOPBAR_BG,
        );

        // Bottom border
        for x in SIDEBAR_W..self.screen_width {
            canvas.draw_pixel(x, TOPBAR_H - 1, SEPARATOR);
        }

        // Page title
        let title = match self.active_page {
            GameOsPage::Home => "Home",
            GameOsPage::AllGames => "All Games",
            GameOsPage::RecentlyPlayed => "Recently Played",
            GameOsPage::Favorites => "Favorites",
            GameOsPage::Store => "Store",
            GameOsPage::Friends => "Friends",
            GameOsPage::Settings => "Settings",
            GameOsPage::Downloads => "Downloads",
            GameOsPage::Screenshots => "Screenshots",
        };
        let title_y = (TOPBAR_H.saturating_sub(TYPE_COUCH_TITLE.line_height as usize)) / 2;
        couch_text(
            canvas,
            SIDEBAR_W + SPACE_5 as usize,
            title_y,
            title,
            TYPE_COUCH_TITLE,
            TEXT_PRIMARY,
        );

        // Game count on the right
        let count = self.visible_games().len();
        let mut buf = [0u8; 12];
        let count_str = fmt_usize(count, &mut buf);
        let count_w = couch_text_w(canvas, count_str, TYPE_COUCH_LABEL);
        let count_x = self.screen_width - SPACE_5 as usize - count_w;
        let row_y = (TOPBAR_H.saturating_sub(TYPE_COUCH_LABEL.line_height as usize)) / 2;
        couch_text(
            canvas,
            count_x,
            row_y,
            count_str,
            TYPE_COUCH_LABEL,
            TEXT_SECONDARY,
        );
        let games_label = "Games:";
        let label_x = count_x
            .saturating_sub(couch_text_w(canvas, games_label, TYPE_COUCH_LABEL) + SPACE_2 as usize);
        couch_text(
            canvas,
            label_x,
            row_y,
            games_label,
            TYPE_COUCH_LABEL,
            TEXT_DIMMED,
        );
    }

    pub fn render_home(&self, canvas: &mut raegfx::Canvas, ox: usize, oy: usize) {
        let section_x = ox + GRID_MARGIN;
        let mut cy = oy + SPACE_5 as usize;

        // Horizontal carousel for featured games
        couch_text(
            canvas,
            section_x,
            cy,
            "Featured",
            TYPE_COUCH_TITLE,
            TEXT_PRIMARY,
        );
        cy += TYPE_COUCH_TITLE.line_height as usize + SPACE_2 as usize;
        if !self.featured.is_empty() {
            self.render_carousel(canvas, section_x, cy);
            cy += CARD_H + CARD_GAP + SPACE_5 as usize;
        } else {
            couch_text(
                canvas,
                section_x,
                cy,
                "No featured games",
                TYPE_COUCH_BODY,
                TEXT_DIMMED,
            );
            cy += SPACE_8 as usize;
        }

        // Recently played section
        couch_text(
            canvas,
            section_x,
            cy,
            "Recently Played",
            TYPE_COUCH_TITLE,
            TEXT_PRIMARY,
        );
        cy += TYPE_COUCH_TITLE.line_height as usize + SPACE_2 as usize;
        if !self.recent_games.is_empty() {
            self.render_game_grid(canvas, &self.recent_games, section_x, cy, false);
        } else {
            couch_text(
                canvas,
                section_x,
                cy,
                "No recent games",
                TYPE_COUCH_BODY,
                TEXT_DIMMED,
            );
        }
    }

    pub fn render_library(&self, canvas: &mut raegfx::Canvas, ox: usize, oy: usize) {
        let games = self.visible_games();
        let section_x = ox + GRID_MARGIN;
        let cy = oy + SPACE_5 as usize;

        if games.is_empty() {
            couch_text(
                canvas,
                section_x,
                cy + SPACE_6 as usize,
                "No games found",
                TYPE_COUCH_SUBTITLE,
                TEXT_SECONDARY,
            );
            couch_text(
                canvas,
                section_x,
                cy + SPACE_6 as usize + TYPE_COUCH_SUBTITLE.line_height as usize,
                "Add games to your library",
                TYPE_COUCH_BODY,
                TEXT_DIMMED,
            );
            return;
        }

        self.render_game_grid(canvas, games, section_x, cy, true);
    }

    pub fn render_game_grid(
        &self,
        canvas: &mut raegfx::Canvas,
        games: &[GameEntry],
        ox: usize,
        oy: usize,
        show_selection: bool,
    ) {
        let acc = accent();
        let visible_rows = self.content_height() / (CARD_H + CARD_GAP);
        let start = self.scroll_offset;
        let end = (start + visible_rows * self.grid_columns).min(games.len());

        for (flat_i, game) in games[start..end].iter().enumerate() {
            let global_i = start + flat_i;
            let col = flat_i % self.grid_columns;
            let row = flat_i / self.grid_columns;

            let cx = ox + col * (CARD_W + CARD_GAP);
            let cy = oy + row * (CARD_H + CARD_GAP);

            if cy + CARD_H > self.screen_height {
                break;
            }

            let is_sel = show_selection
                && self.controller_focus == FocusTarget::GameGrid
                && global_i == self.selected_index;

            // The focused tile lifts (elev.3) + scales ~1.06× (focus ring §).
            let (tx, ty, tw, th) = if is_sel {
                let dw = CARD_W * 6 / 100;
                let dh = CARD_H * 6 / 100;
                (
                    cx.saturating_sub(dw / 2),
                    cy.saturating_sub(dh / 2),
                    CARD_W + dw,
                    CARD_H + dh,
                )
            } else {
                (cx, cy, CARD_W, CARD_H)
            };

            // Cover-art card (rounded, radius.lg). Resting tiles get a hairline.
            canvas.fill_rounded_rect(tx, ty, tw, th, RADIUS_LG as usize, game.banner_color);
            if !is_sel {
                canvas.draw_rounded_rect_outline(tx, ty, tw, th, RADIUS_LG as usize, SEPARATOR);
            }

            // Running indicator (top-right pill).
            if game.running {
                canvas.fill_rounded_rect(
                    tx + tw - SPACE_5 as usize,
                    ty + SPACE_2 as usize,
                    SPACE_4 as usize,
                    SPACE_4 as usize,
                    RADIUS_XS as usize,
                    GREEN,
                );
            }

            // Title scrim at bottom of card (so AA title clears 4.5:1 over art).
            let scrim_h = TYPE_COUCH_SUBTITLE.line_height as usize
                + TYPE_COUCH_CAPTION.line_height as usize
                + SPACE_3 as usize;
            let title_y = ty + th - scrim_h;
            canvas.fill_rect(tx, title_y, tw, scrim_h, BG_CARD);

            let inner = tx + SPACE_3 as usize;
            let max_title = (tw.saturating_sub(SPACE_4 as usize)) / GLYPH_W.max(1);
            let disp_title = crate::text_util::truncate_chars(&game.title, max_title);
            couch_text(
                canvas,
                inner,
                title_y + SPACE_1 as usize,
                disp_title,
                TYPE_COUCH_SUBTITLE,
                TEXT_PRIMARY,
            );

            // Store + installed status
            let store_str = match game.store {
                GameStoreName::Steam => "STM",
                GameStoreName::Epic => "EPC",
                GameStoreName::Gog => "GOG",
                GameStoreName::RaeStore => "RAE",
                GameStoreName::Custom => "USR",
            };
            let meta_y = title_y + SPACE_1 as usize + TYPE_COUCH_SUBTITLE.line_height as usize;
            couch_text(
                canvas,
                inner,
                meta_y,
                store_str,
                TYPE_COUCH_CAPTION,
                TEXT_DIMMED,
            );

            if game.installed {
                let rw = couch_text_w(canvas, "Ready", TYPE_COUCH_CAPTION);
                couch_text(
                    canvas,
                    tx + tw - SPACE_3 as usize - rw,
                    meta_y,
                    "Ready",
                    TYPE_COUCH_CAPTION,
                    GREEN,
                );
            } else {
                let nw = couch_text_w(canvas, "Not installed", TYPE_COUCH_CAPTION);
                couch_text(
                    canvas,
                    tx + tw - SPACE_3 as usize - nw,
                    meta_y,
                    "Not installed",
                    TYPE_COUCH_CAPTION,
                    TEXT_DIMMED,
                );
            }

            // Favorite star (gold dot, top-left).
            if game.favorited {
                canvas.fill_rounded_rect(
                    tx + SPACE_2 as usize,
                    ty + SPACE_2 as usize,
                    SPACE_3 as usize,
                    SPACE_3 as usize,
                    RADIUS_XS as usize,
                    GOLD,
                );
            }

            // Focus ring LAST so it lifts over the tile (ring + glow + top cue).
            if is_sel {
                draw_focus_ring(canvas, tx, ty, tw, th, &acc);
            }
        }
    }

    pub fn render_game_detail(&self, canvas: &mut raegfx::Canvas, game_index: usize) {
        let games = self.visible_games();
        if game_index >= games.len() {
            return;
        }
        let game = &games[game_index];

        let acc = accent();
        let panel_w = (self.screen_width * 2 / 3)
            .max(640)
            .min(self.screen_width - SPACE_8 as usize);
        let panel_h = (self.screen_height * 3 / 4).min(self.screen_height - SPACE_8 as usize);
        let px = (self.screen_width - panel_w) / 2;
        let py = (self.screen_height - panel_h) / 2;

        // Dim background
        canvas.fill_rect(0, 0, self.screen_width, self.screen_height, 0xCC_00_00_00);

        // Panel (elev.4 modal class) — rounded radius.lg.
        canvas.fill_rounded_rect(px, py, panel_w, panel_h, RADIUS_LG as usize, BG_CARD);
        canvas.draw_rounded_rect_outline(px, py, panel_w, panel_h, RADIUS_LG as usize, acc.base);

        // Banner area (cover-art header).
        let banner_h = ROW_H * 2;
        canvas.fill_rect(px, py, panel_w, banner_h, game.banner_color);
        couch_text(
            canvas,
            px + SPACE_5 as usize,
            py + (banner_h.saturating_sub(TYPE_COUCH_TITLE.line_height as usize)) / 2,
            &game.title,
            TYPE_COUCH_TITLE,
            TEXT_PRIMARY,
        );

        let row_h = TYPE_COUCH_BODY.line_height as usize + SPACE_2 as usize;
        let mut detail_y = py + banner_h + SPACE_4 as usize;
        let left = px + SPACE_5 as usize;
        let val_x = left + ROW_H * 4;

        let mut row =
            |canvas: &mut raegfx::Canvas, y: &mut usize, label: &str, value: &str, vc: u32| {
                couch_text(canvas, left, *y, label, TYPE_COUCH_BODY, TEXT_SECONDARY);
                couch_text(canvas, val_x, *y, value, TYPE_COUCH_BODY, vc);
                *y += row_h;
            };

        let store = match game.store {
            GameStoreName::Steam => "Steam",
            GameStoreName::Epic => "Epic Games",
            GameStoreName::Gog => "GOG",
            GameStoreName::RaeStore => "RaeStore",
            GameStoreName::Custom => "Custom",
        };
        row(canvas, &mut detail_y, "Store", store, TEXT_PRIMARY);

        if game.installed {
            row(canvas, &mut detail_y, "Status", "Installed", GREEN);
        } else {
            row(canvas, &mut detail_y, "Status", "Not Installed", ORANGE);
        }

        let mut size_buf = [0u8; 12];
        let size_str = fmt_usize(game.size_gb as usize, &mut size_buf);
        let size_disp = alloc::format!("{} GB", size_str);
        row(canvas, &mut detail_y, "Size", &size_disp, TEXT_PRIMARY);

        let mut hours_buf = [0u8; 12];
        let hours_str = fmt_usize(game.playtime_hours as usize, &mut hours_buf);
        let played_disp = alloc::format!("{} hours", hours_str);
        row(canvas, &mut detail_y, "Played", &played_disp, TEXT_PRIMARY);

        if let Some(rating) = game.rating {
            let mut r_buf = [0u8; 12];
            let r_str = fmt_usize(rating as usize, &mut r_buf);
            let r_disp = alloc::format!("{}/10", r_str);
            row(canvas, &mut detail_y, "Rating", &r_disp, GOLD);
        }

        // Achievements section
        if !self.achievements_cache.is_empty() {
            detail_y += SPACE_1 as usize;
            let ach_height = self.render_achievements(
                canvas,
                &self.achievements_cache,
                left,
                detail_y,
                panel_w - SPACE_8 as usize,
            );
            detail_y += ach_height;
        }
        let _ = detail_y;

        // Action buttons at bottom — couch hit-target height.
        let btn_y = py + panel_h - ROW_H - SPACE_4 as usize;
        let btn_w = ROW_H * 3;
        if game.installed {
            canvas.fill_rounded_rect(left, btn_y, btn_w, ROW_H, RADIUS_MD as usize, acc.base);
            couch_text(
                canvas,
                left + SPACE_4 as usize,
                btn_y + (ROW_H.saturating_sub(TYPE_COUCH_LABEL.line_height as usize)) / 2,
                "(A) Play",
                TYPE_COUCH_LABEL,
                BG_DARK,
            );
        } else {
            canvas.fill_rounded_rect(left, btn_y, btn_w, ROW_H, RADIUS_MD as usize, GREEN);
            couch_text(
                canvas,
                left + SPACE_4 as usize,
                btn_y + (ROW_H.saturating_sub(TYPE_COUCH_LABEL.line_height as usize)) / 2,
                "(A) Install",
                TYPE_COUCH_LABEL,
                BG_DARK,
            );
        }
        let hint_y = btn_y + (ROW_H.saturating_sub(TYPE_COUCH_LABEL.line_height as usize)) / 2;
        couch_text(
            canvas,
            left + btn_w + SPACE_4 as usize,
            hint_y,
            "(B) Back",
            TYPE_COUCH_LABEL,
            TEXT_SECONDARY,
        );
        let fav_x = left + btn_w + ROW_H * 3;
        if game.favorited {
            couch_text(
                canvas,
                fav_x,
                hint_y,
                "(Y) Unfavorite",
                TYPE_COUCH_LABEL,
                GOLD,
            );
        } else {
            couch_text(
                canvas,
                fav_x,
                hint_y,
                "(Y) Favorite",
                TYPE_COUCH_LABEL,
                TEXT_DIMMED,
            );
        }
    }

    /// Render the per-game profile editor overlay (Phase 5) — the Concept's
    /// "resolution, refresh rate, audio device, GPU power limit … per game"
    /// surface. A glass modal (elev.4) of focusable rows: each row is the field
    /// label (left) + its current value (right), the focused row lifted with the
    /// accent focus bar. Controller/keyboard navigable (D-pad up/down moves the
    /// row, left/right edits, A confirms, B cancels). Crisp-AA, TV-distance type,
    /// ≥48px rows. Never panics (no editor open → no-op; saturating layout).
    pub fn render_profile_editor(&self, canvas: &mut raegfx::Canvas) {
        let editor = match self.profile_editor.as_ref() {
            Some(e) => e,
            None => return,
        };
        let acc = accent();

        let panel_w = (self.screen_width * 2 / 3)
            .max(640.min(self.screen_width))
            .min(self.screen_width.saturating_sub(SPACE_8 as usize));
        let panel_h =
            (self.screen_height * 4 / 5).min(self.screen_height.saturating_sub(SPACE_6 as usize));
        let px = (self.screen_width.saturating_sub(panel_w)) / 2;
        let py = (self.screen_height.saturating_sub(panel_h)) / 2;

        // Dim background + glass panel (elev.4 modal class).
        canvas.fill_rect(0, 0, self.screen_width, self.screen_height, 0xDD_00_00_00);
        canvas.fill_rounded_rect(px, py, panel_w, panel_h, RADIUS_LG as usize, BG_CARD);
        canvas.draw_rounded_rect_outline(px, py, panel_w, panel_h, RADIUS_LG as usize, acc.base);

        // Header: "Profile — <title>" in the accent.
        let header_h = ROW_H + SPACE_2 as usize;
        let title = alloc::format!("Profile \u{2014} {}", editor.title);
        couch_text(
            canvas,
            px + SPACE_5 as usize,
            py + (header_h.saturating_sub(TYPE_COUCH_SUBTITLE.line_height as usize)) / 2,
            &title,
            TYPE_COUCH_SUBTITLE,
            acc.base,
        );
        for x in px + SPACE_3 as usize..px + panel_w - SPACE_3 as usize {
            canvas.draw_pixel(x, py + header_h, SEPARATOR);
        }

        // Field rows. Each row ≥ HIT_TARGET_COUCH; focused row lifts with a
        // bg.elevated fill + an accent left bar + the focus ring (a11y: never
        // colour alone — the fill + bar + ring are three redundant cues).
        let row_h = ROW_H;
        let left = px + SPACE_5 as usize;
        let row_w = panel_w.saturating_sub(SPACE_5 as usize * 2);
        let mut y = py + header_h + SPACE_3 as usize;
        let footer_reserve = ROW_H + SPACE_4 as usize;
        let max_y = py + panel_h - footer_reserve;

        for (i, &field) in ProfileField::ORDER.iter().enumerate() {
            if y + row_h > max_y {
                break;
            }
            let focused = i == editor.focused;
            if focused {
                canvas.fill_rounded_rect(left, y, row_w, row_h, RADIUS_MD as usize, BG_SELECTED);
                // Accent left bar.
                canvas.fill_rect(left, y, FOCUS_RING_W, row_h, acc.base);
                canvas.draw_rounded_rect_outline(
                    left,
                    y,
                    row_w,
                    row_h,
                    RADIUS_MD as usize,
                    acc.base,
                );
            }

            let text_y = y + (row_h.saturating_sub(TYPE_COUCH_BODY.line_height as usize)) / 2;
            // Label (left).
            couch_text(
                canvas,
                left + SPACE_4 as usize,
                text_y,
                field.label(),
                TYPE_COUCH_BODY,
                if focused {
                    TEXT_PRIMARY
                } else {
                    TEXT_SECONDARY
                },
            );
            // Value (right, with ‹ value › edit affordance when focused).
            let value = field.value_string(&editor.profile);
            let value_disp = if focused {
                alloc::format!("\u{2039} {} \u{203A}", value)
            } else {
                value
            };
            let vw = couch_text_w(canvas, &value_disp, TYPE_COUCH_BODY);
            let vx = (left + row_w).saturating_sub(SPACE_4 as usize + vw);
            couch_text(
                canvas,
                vx,
                text_y,
                &value_disp,
                TYPE_COUCH_BODY,
                if focused { acc.base } else { TEXT_PRIMARY },
            );

            y += row_h + SPACE_2 as usize;
        }

        // Footer hint chips (couch label ramp): confirm / edit / cancel.
        let footer_y = py + panel_h - ROW_H;
        let hint_y = footer_y + (ROW_H.saturating_sub(TYPE_COUCH_LABEL.line_height as usize)) / 2;
        let mut hx = left;
        for (glyph, label, col) in [
            ("(A)", "Save", acc.base),
            ("(\u{2194})", "Edit", TEXT_SECONDARY),
            ("(B)", "Cancel", TEXT_SECONDARY),
        ] {
            let hint = alloc::format!("{} {}", glyph, label);
            let end = couch_text(canvas, hx, hint_y, &hint, TYPE_COUCH_LABEL, col);
            hx = end + SPACE_5 as usize;
            if hx > px + panel_w {
                break;
            }
        }
    }

    pub fn render_quick_menu(&self, canvas: &mut raegfx::Canvas) {
        let acc = accent();
        let item_h = ROW_H;
        let menu_w = (ROW_H * 7).min(self.screen_width - SPACE_8 as usize);
        let header_h = ROW_H;
        let menu_h = (self.quick_menu.items.len() * item_h + header_h + SPACE_4 as usize)
            .min(self.screen_height - SPACE_6 as usize);
        let mx = (self.screen_width - menu_w) / 2;
        let my = (self.screen_height - menu_h) / 2;

        // Dim background
        canvas.fill_rect(0, 0, self.screen_width, self.screen_height, 0xCC_00_00_00);

        canvas.fill_rounded_rect(mx, my, menu_w, menu_h, RADIUS_LG as usize, BG_CARD);
        canvas.draw_rounded_rect_outline(mx, my, menu_w, menu_h, RADIUS_LG as usize, acc.base);

        // Title
        couch_text(
            canvas,
            mx + SPACE_5 as usize,
            my + (header_h.saturating_sub(TYPE_COUCH_SUBTITLE.line_height as usize)) / 2,
            "Quick Menu",
            TYPE_COUCH_SUBTITLE,
            acc.base,
        );
        for x in mx + SPACE_3 as usize..mx + menu_w - SPACE_3 as usize {
            canvas.draw_pixel(x, my + header_h, SEPARATOR);
        }

        let items_y = my + header_h + SPACE_2 as usize;
        for (i, item) in self.quick_menu.items.iter().enumerate() {
            let iy = items_y + i * item_h;
            if iy + item_h > my + menu_h {
                break;
            }

            if i == self.quick_menu.selected {
                canvas.fill_rounded_rect(
                    mx + SPACE_2 as usize,
                    iy,
                    menu_w - SPACE_4 as usize,
                    item_h,
                    RADIUS_MD as usize,
                    acc.subtle,
                );
                canvas.fill_rect(mx + SPACE_2 as usize, iy, FOCUS_RING_W, item_h, acc.base);
            }

            let text_color = if i == self.quick_menu.selected {
                TEXT_PRIMARY
            } else {
                TEXT_SECONDARY
            };

            let text_y = iy + (item_h.saturating_sub(TYPE_COUCH_BODY.line_height as usize)) / 2;
            couch_text(
                canvas,
                mx + SPACE_5 as usize,
                text_y,
                &item.label,
                TYPE_COUCH_BODY,
                text_color,
            );

            // Show value for adjustable actions
            match item.action {
                QuickAction::Brightness(v) | QuickAction::Volume(v) => {
                    let mut buf = [0u8; 12];
                    let val_str = fmt_usize(v as usize, &mut buf);
                    let val_disp = alloc::format!("{}%", val_str);
                    let vw = couch_text_w(canvas, &val_disp, TYPE_COUCH_BODY);
                    couch_text(
                        canvas,
                        mx + menu_w - SPACE_4 as usize - vw,
                        text_y,
                        &val_disp,
                        TYPE_COUCH_BODY,
                        acc.base,
                    );
                }
                _ => {}
            }
        }
    }

    pub fn render_now_playing(&self, canvas: &mut raegfx::Canvas, np: &NowPlaying) {
        let bar_h = ROW_H;
        let bar_y = self.screen_height - bar_h;

        canvas.fill_rect(
            SIDEBAR_W,
            bar_y,
            self.screen_width - SIDEBAR_W,
            bar_h,
            TOPBAR_BG,
        );
        for x in SIDEBAR_W..self.screen_width {
            canvas.draw_pixel(x, bar_y, SEPARATOR);
        }

        // Playing strip — title (couch label) left, FPS/FT right.
        let text_y = bar_y + (bar_h.saturating_sub(TYPE_COUCH_LABEL.line_height as usize)) / 2;
        let play_disp = alloc::format!("> {}", np.game.title);
        couch_text(
            canvas,
            SIDEBAR_W + SPACE_4 as usize,
            text_y,
            &play_disp,
            TYPE_COUCH_LABEL,
            TEXT_PRIMARY,
        );

        // FPS / frametime
        let mut fps_buf = [0u8; 12];
        let fps_str = fmt_usize(np.fps as usize, &mut fps_buf);
        let fps_color = if np.fps >= 60.0 {
            GREEN
        } else if np.fps >= 30.0 {
            ORANGE
        } else {
            RED
        };
        let mut ft_buf = [0u8; 12];
        let ft_str = fmt_usize(np.frametime_ms as usize, &mut ft_buf);
        let stat = alloc::format!("FPS {}   FT {}ms", fps_str, ft_str);
        let sw = couch_text_w(canvas, &stat, TYPE_COUCH_LABEL);
        // Colour the FPS number distinctly by painting the prefix label first.
        couch_text(
            canvas,
            self.screen_width - SPACE_5 as usize - sw,
            text_y,
            &stat,
            TYPE_COUCH_LABEL,
            fps_color,
        );
    }

    // ── Enter / Exit ──────────────────────────────────────────────────

    pub fn enter(&mut self) {
        self.active = true;
        self.state = GameOsState::Home;
        self.active_page = self.config.default_page;
        self.selected_index = 0;
        self.scroll_offset = 0;
        self.controller_focus = FocusTarget::GameGrid;
        self.detail_game = None;
        self.quick_menu.visible = false;
        self.search.active = false;
        self.settings_visible = false;
        self.carousel_offset = 0;
        self.carousel_timer = 0;
    }

    pub fn exit(&mut self) {
        self.active = false;
        self.state = GameOsState::Home;
        self.quick_menu.visible = false;
        self.search.active = false;
        self.settings_visible = false;
    }

    pub fn should_auto_enter(&self) -> bool {
        self.config.auto_enter
    }

    pub fn set_auto_enter(&mut self, enabled: bool) {
        self.config.auto_enter = enabled;
        self.settings.auto_enter_gameos = enabled;
    }

    // ── Carousel (horizontal scroll of recent games on home screen) ──

    pub fn tick_carousel(&mut self, elapsed_ms: u64) {
        if self.active_page != GameOsPage::Home || self.featured.is_empty() {
            return;
        }
        self.carousel_timer += elapsed_ms;
        if self.carousel_timer >= self.config.carousel_speed_ms {
            self.carousel_timer = 0;
            self.carousel_offset = (self.carousel_offset + 1) % self.featured.len();
        }
    }

    // ── Transition animation ─────────────────────────────────────────

    pub fn tick_transition(&mut self, elapsed_ms: u64) {
        if let Some(ref mut t) = self.transition {
            t.progress += elapsed_ms as f32 / t.duration_ms as f32;
            if t.progress >= 1.0 {
                self.transition = None;
            }
        }
    }

    fn transition_alpha(&self) -> f32 {
        self.transition.as_ref().map_or(1.0, |t| {
            let p = t.progress.min(1.0);
            // Ease-out cubic: 1 - (1 - p)^3
            let inv = 1.0 - p;
            1.0 - inv * inv * inv
        })
    }

    // ── Achievements for detail view ─────────────────────────────────

    pub fn set_achievements(&mut self, achievements: Vec<Achievement>) {
        self.achievements_cache = achievements;
    }

    fn render_achievements(
        &self,
        canvas: &mut raegfx::Canvas,
        achievements: &[Achievement],
        x: usize,
        y: usize,
        max_w: usize,
    ) -> usize {
        if achievements.is_empty() {
            return 0;
        }

        couch_text(canvas, x, y, "Achievements", TYPE_COUCH_BODY, TEXT_PRIMARY);
        let mut ay = y + TYPE_COUCH_BODY.line_height as usize;

        let unlocked = achievements.iter().filter(|a| a.unlocked).count();
        let total = achievements.len();
        let mut count_buf = [0u8; 12];
        let mut total_buf = [0u8; 12];
        let c_str = fmt_usize(unlocked, &mut count_buf);
        let t_str = fmt_usize(total, &mut total_buf);
        let summary = alloc::format!("{} / {} unlocked", c_str, t_str);
        couch_text(canvas, x, ay, &summary, TYPE_COUCH_CAPTION, TEXT_SECONDARY);
        ay += TYPE_COUCH_CAPTION.line_height as usize + SPACE_1 as usize;

        // Progress bar
        let bar_w = max_w.min(ROW_H * 5);
        let bar_h = SPACE_2 as usize;
        canvas.fill_rounded_rect(x, ay, bar_w, bar_h, RADIUS_XS as usize, BG_SELECTED);
        if total > 0 {
            let fill = (unlocked * bar_w) / total;
            if fill > 0 {
                canvas.fill_rounded_rect(x, ay, fill, bar_h, RADIUS_XS as usize, GOLD);
            }
        }
        ay += bar_h + SPACE_2 as usize;

        // Show first 4 achievements
        for ach in achievements.iter().take(4) {
            let text_color = if ach.unlocked {
                TEXT_PRIMARY
            } else {
                TEXT_DIMMED
            };
            let max_name = (max_w.saturating_sub(SPACE_4 as usize)) / GLYPH_W.max(1);
            let disp = crate::text_util::truncate_chars(&ach.name, max_name);
            couch_text(canvas, x, ay, disp, TYPE_COUCH_CAPTION, text_color);
            ay += TYPE_COUCH_CAPTION.line_height as usize;
        }

        ay - y
    }

    // ── Settings page (in-GameOS) ────────────────────────────────────

    pub fn toggle_settings(&mut self) {
        self.settings_visible = !self.settings_visible;
        if self.settings_visible {
            self.settings.selected_item = 0;
        }
    }

    fn render_settings(&self, canvas: &mut raegfx::Canvas) {
        let acc = accent();
        let panel_w = (self.screen_width * 3 / 4).min(self.screen_width - SPACE_8 as usize);
        let panel_h = (self.screen_height * 3 / 4).min(self.screen_height - SPACE_8 as usize);
        let px = (self.screen_width - panel_w) / 2;
        let py = (self.screen_height - panel_h) / 2;

        canvas.fill_rect(0, 0, self.screen_width, self.screen_height, 0xCC_00_00_00);
        canvas.fill_rounded_rect(px, py, panel_w, panel_h, RADIUS_LG as usize, BG_CARD);
        canvas.draw_rounded_rect_outline(px, py, panel_w, panel_h, RADIUS_LG as usize, acc.base);

        let header_h = ROW_H;
        couch_text(
            canvas,
            px + SPACE_5 as usize,
            py + (header_h.saturating_sub(TYPE_COUCH_SUBTITLE.line_height as usize)) / 2,
            "Settings",
            TYPE_COUCH_SUBTITLE,
            acc.base,
        );
        for x in px + SPACE_3 as usize..px + panel_w - SPACE_3 as usize {
            canvas.draw_pixel(x, py + header_h, SEPARATOR);
        }

        let sections: &[(SettingsSection, &str)] = &[
            (SettingsSection::Display, "Display"),
            (SettingsSection::Audio, "Audio"),
            (SettingsSection::Network, "Network"),
            (SettingsSection::Controller, "Controller"),
            (SettingsSection::Performance, "Performance"),
            (SettingsSection::General, "General"),
        ];

        let sidebar_w = ROW_H * 4;
        let items_y = py + header_h + SPACE_2 as usize;
        let item_h = ROW_H;

        for (i, &(sec, label)) in sections.iter().enumerate() {
            let iy = items_y + i * item_h;
            if sec == self.settings.section {
                canvas.fill_rounded_rect(
                    px + SPACE_2 as usize,
                    iy,
                    sidebar_w,
                    item_h,
                    RADIUS_MD as usize,
                    acc.subtle,
                );
                canvas.fill_rect(px + SPACE_2 as usize, iy, FOCUS_RING_W, item_h, acc.base);
            }
            let tc = if sec == self.settings.section {
                TEXT_PRIMARY
            } else {
                TEXT_SECONDARY
            };
            couch_text(
                canvas,
                px + SPACE_4 as usize,
                iy + (item_h.saturating_sub(TYPE_COUCH_BODY.line_height as usize)) / 2,
                label,
                TYPE_COUCH_BODY,
                tc,
            );
        }

        // Separator
        for y in py + header_h..py + panel_h {
            canvas.draw_pixel(
                px + SPACE_2 as usize + sidebar_w + SPACE_4 as usize,
                y,
                SEPARATOR,
            );
        }

        let cx = px + SPACE_2 as usize + sidebar_w + SPACE_5 as usize;
        let cy = items_y;
        let cw = panel_w - (cx - px) - SPACE_5 as usize;
        let step = ROW_H;

        let mut kv = |canvas: &mut raegfx::Canvas, y: usize, k: &str, v: &str, vc: u32| {
            couch_text(canvas, cx, y, k, TYPE_COUCH_BODY, TEXT_SECONDARY);
            let vw = couch_text_w(canvas, v, TYPE_COUCH_BODY);
            couch_text(canvas, cx + cw - vw, y, v, TYPE_COUCH_BODY, vc);
        };

        match self.settings.section {
            SettingsSection::Display => {
                self.render_setting_row(
                    canvas,
                    cx,
                    cy,
                    cw,
                    0,
                    "Brightness",
                    &self.settings.brightness,
                    "%",
                );
                self.render_setting_row(
                    canvas,
                    cx,
                    cy + step,
                    cw,
                    1,
                    "Resolution",
                    &self.settings.resolution_index,
                    "",
                );
                self.render_setting_row(
                    canvas,
                    cx,
                    cy + step * 2,
                    cw,
                    2,
                    "Refresh Rate",
                    &self.settings.refresh_rate_index,
                    "",
                );
            }
            SettingsSection::Audio => {
                self.render_setting_row(
                    canvas,
                    cx,
                    cy,
                    cw,
                    0,
                    "Volume",
                    &self.settings.volume,
                    "%",
                );
            }
            SettingsSection::Network => {
                let (ws, wc) = if self.settings.wifi_enabled {
                    ("On", GREEN)
                } else {
                    ("Off", RED)
                };
                kv(canvas, cy, "Wi-Fi", ws, wc);
                let (bs, bc) = if self.settings.bluetooth_enabled {
                    ("On", GREEN)
                } else {
                    ("Off", RED)
                };
                kv(canvas, cy + step, "Bluetooth", bs, bc);
            }
            SettingsSection::Controller => {
                couch_text(
                    canvas,
                    cx,
                    cy,
                    "Controller Mapping",
                    TYPE_COUCH_BODY,
                    TEXT_PRIMARY,
                );
                let mut my = cy + step;
                for (slot, action) in self.settings.controller_mapping.mappings.iter().take(8) {
                    let slot_str = match slot {
                        ControllerMappingSlot::ButtonSouth => "A/Cross",
                        ControllerMappingSlot::ButtonEast => "B/Circle",
                        ControllerMappingSlot::ButtonNorth => "Y/Triangle",
                        ControllerMappingSlot::ButtonWest => "X/Square",
                        ControllerMappingSlot::DPadUp => "D-Up",
                        ControllerMappingSlot::DPadDown => "D-Down",
                        ControllerMappingSlot::DPadLeft => "D-Left",
                        ControllerMappingSlot::DPadRight => "D-Right",
                        _ => "Button",
                    };
                    couch_text(canvas, cx, my, slot_str, TYPE_COUCH_CAPTION, TEXT_SECONDARY);
                    let max_a = (cw.saturating_sub(ROW_H * 2)) / GLYPH_W.max(1);
                    let disp = crate::text_util::truncate_chars(action, max_a);
                    couch_text(
                        canvas,
                        cx + ROW_H * 2,
                        my,
                        disp,
                        TYPE_COUCH_CAPTION,
                        TEXT_PRIMARY,
                    );
                    my += TYPE_COUCH_CAPTION.line_height as usize + SPACE_1 as usize;
                }
            }
            SettingsSection::Performance => {
                let (ns, nc) = if self.settings.null_latency {
                    ("On", GREEN)
                } else {
                    ("Off", TEXT_DIMMED)
                };
                kv(canvas, cy, "NULL_LATENCY", ns, nc);
            }
            SettingsSection::General => {
                let (abs, abc) = if self.settings.auto_enter_gameos {
                    ("On", GREEN)
                } else {
                    ("Off", TEXT_DIMMED)
                };
                kv(canvas, cy, "Auto-enter GameOS", abs, abc);
            }
        }

        // Bottom hint
        let hint_y = py + panel_h - TYPE_COUCH_LABEL.line_height as usize - SPACE_3 as usize;
        couch_text(
            canvas,
            px + SPACE_5 as usize,
            hint_y,
            "(B) Back",
            TYPE_COUCH_LABEL,
            TEXT_DIMMED,
        );
        couch_text(
            canvas,
            px + SPACE_5 as usize + ROW_H * 3,
            hint_y,
            "(A) Toggle / Adjust",
            TYPE_COUCH_LABEL,
            TEXT_DIMMED,
        );
    }

    fn render_setting_row(
        &self,
        canvas: &mut raegfx::Canvas,
        x: usize,
        y: usize,
        w: usize,
        idx: usize,
        label: &str,
        value: &impl SettingDisplay,
        suffix: &str,
    ) {
        let acc = accent();
        let selected = idx == self.settings.selected_item;
        if selected {
            canvas.fill_rounded_rect(
                x.saturating_sub(SPACE_2 as usize),
                y,
                w + SPACE_4 as usize,
                ROW_H,
                RADIUS_MD as usize,
                acc.subtle,
            );
        }
        let ty = y + (ROW_H.saturating_sub(TYPE_COUCH_BODY.line_height as usize)) / 2;
        couch_text(canvas, x, ty, label, TYPE_COUCH_BODY, TEXT_SECONDARY);
        let val_str_buf = value.display_str();
        let val_disp = alloc::format!("{}{}", val_str_buf, suffix);
        let vw = couch_text_w(canvas, &val_disp, TYPE_COUCH_BODY);
        couch_text(
            canvas,
            x + w - vw,
            ty,
            &val_disp,
            TYPE_COUCH_BODY,
            TEXT_PRIMARY,
        );
    }

    fn render_search_overlay(&self, canvas: &mut raegfx::Canvas) {
        let acc = accent();
        let overlay_w = (self.screen_width * 2 / 3).min(self.screen_width - SPACE_8 as usize);
        let overlay_h = (self.screen_height * 2 / 3).min(self.screen_height - SPACE_8 as usize);
        let ox = (self.screen_width - overlay_w) / 2;
        let oy = SPACE_8 as usize;

        // Dim
        canvas.fill_rect(0, 0, self.screen_width, self.screen_height, 0xCC_00_00_00);

        canvas.fill_rounded_rect(ox, oy, overlay_w, overlay_h, RADIUS_LG as usize, BG_CARD);
        canvas.draw_rounded_rect_outline(
            ox,
            oy,
            overlay_w,
            overlay_h,
            RADIUS_LG as usize,
            acc.base,
        );

        // Search box (couch hit-target height).
        let box_h = ROW_H;
        canvas.fill_rounded_rect(
            ox + SPACE_3 as usize,
            oy + SPACE_3 as usize,
            overlay_w - SPACE_6 as usize,
            box_h,
            RADIUS_MD as usize,
            BG_DARK,
        );
        canvas.draw_rounded_rect_outline(
            ox + SPACE_3 as usize,
            oy + SPACE_3 as usize,
            overlay_w - SPACE_6 as usize,
            box_h,
            RADIUS_MD as usize,
            acc.base,
        );
        let q_display = if self.search.query.is_empty() {
            "Type to search..."
        } else {
            &self.search.query
        };
        let q_color = if self.search.query.is_empty() {
            TEXT_DIMMED
        } else {
            TEXT_PRIMARY
        };
        couch_text(
            canvas,
            ox + SPACE_5 as usize,
            oy + SPACE_3 as usize
                + (box_h.saturating_sub(TYPE_COUCH_BODY.line_height as usize)) / 2,
            q_display,
            TYPE_COUCH_BODY,
            q_color,
        );

        // Results
        let result_y = oy + SPACE_3 as usize + box_h + SPACE_3 as usize;
        let item_h = ROW_H;
        for (i, game) in self.search.results.iter().enumerate() {
            let ry = result_y + i * item_h;
            if ry + item_h > oy + overlay_h {
                break;
            }

            if i == self.selected_index {
                canvas.fill_rounded_rect(
                    ox + SPACE_2 as usize,
                    ry,
                    overlay_w - SPACE_4 as usize,
                    item_h,
                    RADIUS_MD as usize,
                    acc.subtle,
                );
                canvas.fill_rect(ox + SPACE_2 as usize, ry, FOCUS_RING_W, item_h, acc.base);
            }

            let ty = ry + (item_h.saturating_sub(TYPE_COUCH_BODY.line_height as usize)) / 2;
            let max_chars = (overlay_w.saturating_sub(ROW_H * 2)) / GLYPH_W.max(1);
            let disp = crate::text_util::truncate_chars(&game.title, max_chars);
            couch_text(
                canvas,
                ox + SPACE_5 as usize,
                ty,
                disp,
                TYPE_COUCH_BODY,
                TEXT_PRIMARY,
            );

            if game.installed {
                let rw = couch_text_w(canvas, "Ready", TYPE_COUCH_CAPTION);
                couch_text(
                    canvas,
                    ox + overlay_w - SPACE_4 as usize - rw,
                    ty,
                    "Ready",
                    TYPE_COUCH_CAPTION,
                    GREEN,
                );
            }
        }

        if self.search.results.is_empty() && !self.search.query.is_empty() {
            couch_text(
                canvas,
                ox + SPACE_5 as usize,
                result_y,
                "No results found",
                TYPE_COUCH_BODY,
                TEXT_DIMMED,
            );
        }
    }
}

impl GameOsShell {
    fn render_carousel(&self, canvas: &mut raegfx::Canvas, ox: usize, oy: usize) {
        let acc = accent();
        let available_w = self.screen_width.saturating_sub(SIDEBAR_W + GRID_MARGIN);
        let visible_count = (available_w / (CARD_W + CARD_GAP)).max(1);

        for i in 0..visible_count {
            let idx = (self.carousel_offset + i) % self.featured.len();
            let game = &self.featured[idx];
            let cx = ox + i * (CARD_W + CARD_GAP);

            if cx + CARD_W > self.screen_width {
                break;
            }

            let is_center = i == visible_count / 2;

            canvas.fill_rounded_rect(
                cx,
                oy,
                CARD_W,
                CARD_H,
                RADIUS_LG as usize,
                game.banner_color,
            );
            if !is_center {
                canvas.draw_rounded_rect_outline(
                    cx,
                    oy,
                    CARD_W,
                    CARD_H,
                    RADIUS_LG as usize,
                    SEPARATOR,
                );
            }

            if game.running {
                canvas.fill_rounded_rect(
                    cx + CARD_W - SPACE_5 as usize,
                    oy + SPACE_2 as usize,
                    SPACE_4 as usize,
                    SPACE_4 as usize,
                    RADIUS_XS as usize,
                    GREEN,
                );
            }

            // Title scrim + AA title (the carousel hero uses the hero ramp when
            // it is the centre/focused tile, else the subtitle ramp).
            let style = if is_center {
                TYPE_COUCH_HERO
            } else {
                TYPE_COUCH_SUBTITLE
            };
            let scrim_h = style.line_height as usize + SPACE_3 as usize;
            let title_y = oy + CARD_H - scrim_h;
            canvas.fill_rect(cx, title_y, CARD_W, scrim_h, BG_CARD);

            let max_title = (CARD_W.saturating_sub(SPACE_4 as usize)) / GLYPH_W.max(1);
            let disp = crate::text_util::truncate_chars(&game.title, max_title);
            couch_text(
                canvas,
                cx + SPACE_3 as usize,
                title_y + SPACE_1 as usize,
                disp,
                style,
                TEXT_PRIMARY,
            );

            // Focused carousel tile gets the focus ring (cohesion with the grid).
            if is_center {
                draw_focus_ring(canvas, cx, oy, CARD_W, CARD_H, &acc);
            }
        }

        // Carousel dot indicators
        if self.featured.len() > 1 {
            let dot = SPACE_2 as usize;
            let gap = SPACE_1 as usize;
            let dots_w = self.featured.len() * (dot + gap);
            let dots_x = ox + (available_w.saturating_sub(dots_w)) / 2;
            let dots_y = oy + CARD_H + SPACE_2 as usize;
            for i in 0..self.featured.len() {
                let dx = dots_x + i * (dot + gap);
                let color = if i == self.carousel_offset % self.featured.len() {
                    acc.base
                } else {
                    TEXT_DIMMED
                };
                canvas.fill_rounded_rect(dx, dots_y, dot, dot, RADIUS_XS as usize, color);
            }
        }
    }
}

trait SettingDisplay {
    fn display_str(&self) -> String;
}

impl SettingDisplay for u8 {
    fn display_str(&self) -> String {
        let mut buf = [0u8; 12];
        let s = fmt_usize(*self as usize, &mut buf);
        String::from(s)
    }
}

impl SettingDisplay for usize {
    fn display_str(&self) -> String {
        let mut buf = [0u8; 12];
        let s = fmt_usize(*self, &mut buf);
        String::from(s)
    }
}

// ── Cohesion accessors + boot smoketest ──────────────────────────────────
//
// The dead `static mut GAMEOS_ACTIVE` twin (the old `enter/exit/is_gameos_mode`
// free fns) is RETIRED per the design spec (CLAUDE.md rule 7 — the live toggle
// is `shell_runner::toggle_gameos` over `ShellRunnerState.couch`). What replaces
// it is the cohesion surface: the couch's live accent + a FAIL-able smoketest.

/// The accent the couch surface is painting with right now — `derive_accent` of
/// the LIVE Vibe seed (`crate::active_accent`) over the couch palette. The
/// cohesion deliverable: this MUST equal the desktop's accent after a Vibe Mode
/// change. Read by the boot smoketest and `/proc/raeen/gaming`.
#[must_use]
pub fn couch_active_accent() -> u32 {
    accent().base
}

/// The couch palette seed (the LIVE accent) the cohesion check compares against.
#[must_use]
pub fn couch_active_seed() -> u32 {
    crate::active_accent()
}

/// The couch hit-target floor (px) — every focus target must be at least this.
#[must_use]
pub fn couch_hit_target() -> usize {
    HIT_TARGET_COUCH as usize
}

pub fn should_auto_enter_gameos(config: &GameOsConfig) -> bool {
    config.auto_enter
}

/// The default controller glyph-set tag for `/proc/raeen/gaming` (Phase 2). The
/// live shell's set is per-`GameOsShell`; this exposes the default skin the
/// couch boots with (Phase 3 binds the real pad to override it).
#[must_use]
pub fn default_glyph_set_tag() -> &'static str {
    GlyphSet::DEFAULT.tag()
}

/// Number of context hint chips the couch shows on its default (grid-focused)
/// home context — for `/proc/raeen/gaming`. Built with the SAME `context_hints`
/// logic the live surface renders.
#[must_use]
pub fn default_context_chip_count() -> usize {
    let mut couch = GameOsShell::new(1280, 720);
    couch.controller_focus = FocusTarget::GameGrid;
    couch.context_hints().len()
}

/// The glyph-set tag a pad with USB `vid`/`pid` would auto-select (Phase 3).
/// Exposed for `/proc/raeen/gaming` and the padbind smoketest — the pure
/// VID/PID→set mapping, no live state required.
#[must_use]
pub fn glyph_set_tag_for_vidpid(vid: u16, pid: u16) -> &'static str {
    glyph_set_for_vidpid(vid, pid).tag()
}

/// Outcome of the GameOS couch-cohesion boot smoketest (Phase 1). All four
/// invariants must hold or the kernel prints `-> FAIL`.
pub struct CouchSmoketest {
    /// Number of tiles in the scratch library that rendered.
    pub tiles: usize,
    /// D-pad focus navigation moved the selection as expected.
    pub focus_nav_ok: bool,
    /// The rendered accent == `derive_accent(active_seed()).base` (cohesion).
    pub accent_matches_seed: bool,
    /// Every couch focus target's hit box clears `HIT_TARGET_COUCH` (48px).
    pub hit48_ok: bool,
    /// The render path uses crisp AA glyphs (the couch type ramp), not the 8px
    /// block font — proven by a non-zero AA coverage on a rendered tile title.
    pub glyphs_aa: bool,
}

impl CouchSmoketest {
    #[must_use]
    pub fn passed(&self) -> bool {
        self.tiles > 0
            && self.focus_nav_ok
            && self.accent_matches_seed
            && self.hit48_ok
            && self.glyphs_aa
    }
}

/// Run the GameOS couch-cohesion smoketest into an offscreen canvas. Seeds a
/// small library, drives one D-pad move + launch, renders, and checks the four
/// Phase-1 cohesion invariants. FAIL-able: returns `false` fields when any
/// invariant breaks (wrong accent, sub-48px hit target, or block-font text).
///
/// `canvas` must be a real ARGB canvas (the kernel hands a heap-backed one);
/// the AA-glyph check measures actual ink, so a tofu/blank render fails it.
pub fn run_couch_smoketest(
    canvas: &mut raegfx::Canvas,
    width: usize,
    height: usize,
) -> CouchSmoketest {
    let mut couch = GameOsShell::new(width, height);
    couch.active = true;
    for i in 0..3u64 {
        let e = GameEntry {
            id: i + 1,
            title: alloc::format!("Game {}", i + 1),
            banner_color: 0xFF_2D_5A_9E,
            icon_char: 'G',
            store: GameStoreName::RaeStore,
            installed: true,
            last_played: 0,
            playtime_hours: 0.0,
            rating: None,
            size_gb: 1.0,
            favorited: false,
            running: false,
        };
        couch.featured.push(e.clone());
        couch.library.push(e);
    }
    // Land on the library grid so the tiles + focus ring render.
    couch.navigate(GameOsPage::AllGames);
    couch.controller_focus = FocusTarget::GameGrid;

    // Focus nav: one D-pad right moves the selection by one.
    let start = couch.selected_index;
    couch.controller_input(GamepadInput::Button(GamepadButton::DPadRight));
    let focus_nav_ok = couch.selected_index == start + 1;

    // Cohesion: the couch accent must equal derive_accent(live seed).base.
    let expected = rae_tokens::derive_accent(couch_active_seed(), PALETTE).base;
    let accent_matches_seed = couch_active_accent() == expected;

    // Hit targets: the couch floors (rows, tiles, top bar) all clear 48px.
    let hit48_ok = ROW_H >= HIT_TARGET_COUCH as usize
        && CARD_W >= HIT_TARGET_COUCH as usize
        && CARD_H >= HIT_TARGET_COUCH as usize
        && TOPBAR_H >= HIT_TARGET_COUCH as usize
        && SIDEBAR_W >= HIT_TARGET_COUCH as usize;

    // Build the crisp-AA font engine (idempotent, boot-path-safe) so the AA
    // path is genuinely available — the couch smoketest runs before the global
    // `raegfx::text::run_boot_smoketest`, so without this the engine would not
    // be ready yet and the render would fall back to the 8px bitmap path.
    raegfx::text::ensure_init();

    // Render, then prove the text path is AA (real, non-uniform glyph ink) by
    // drawing a tile title through the SAME path the surface uses and reading
    // its coverage stats — 0 coverage == the 8px bitmap-fallback path == FAIL.
    couch.render(canvas);
    let stats = canvas.draw_text_aa_stats(
        SPACE_4 as i32,
        SPACE_4 as i32,
        "Library",
        TYPE_COUCH_TITLE,
        TEXT_PRIMARY,
        FontFamily::Sans,
    );
    let glyphs_aa = stats.total_coverage > 0 && stats.max_cov > stats.min_cov;

    CouchSmoketest {
        tiles: couch.library.len(),
        focus_nav_ok,
        accent_matches_seed,
        hit48_ok,
        glyphs_aa,
    }
}

// ── Phase 2: button-hint bar + glyph-set smoketest ─────────────────────────

/// Outcome of the GameOS glyph/hint-bar smoketest (Phase 2). FAIL-able on each
/// new behaviour: zero rendered chips, a wrong per-set glyph, or a block-font
/// (non-AA) text path on the hint bar.
pub struct GlyphSmoketest {
    /// The active glyph-set tag (xbox / ps / nintendo / generic).
    pub set_tag: &'static str,
    /// Number of context hint chips rendered in the default grid context.
    pub chips: usize,
    /// The active set's glyph for "Select" matches the expected per-set glyph,
    /// AND each set maps "Select" to a distinct glyph where it should
    /// (PlayStation ✕ ≠ Xbox A) — the action→glyph mapping is correct.
    pub context_ok: bool,
    /// The hint-bar text path is crisp AA (the couch label ramp), not the 8px
    /// block font — proven by non-uniform glyph ink on a rendered label.
    pub glyphs_aa: bool,
}

impl GlyphSmoketest {
    #[must_use]
    pub fn passed(&self) -> bool {
        self.chips > 0 && self.context_ok && self.glyphs_aa
    }
}

/// Run the Phase-2 glyph/hint-bar smoketest into an offscreen canvas. Seeds a
/// small library, selects the Xbox glyph set, renders the couch (which paints
/// the hint bar), and checks: (1) the context hint set is non-empty, (2) the
/// active set's "Select" glyph matches the expected per-set glyph and the sets
/// genuinely differ (PS ✕ vs Xbox A), (3) the hint-label text is real AA ink.
pub fn run_glyph_smoketest(
    canvas: &mut raegfx::Canvas,
    width: usize,
    height: usize,
) -> GlyphSmoketest {
    let mut couch = GameOsShell::new(width, height);
    couch.active = true;
    couch.set_glyph_set(GlyphSet::Xbox);
    for i in 0..3u64 {
        let e = GameEntry {
            id: i + 1,
            title: alloc::format!("Game {}", i + 1),
            banner_color: 0xFF_2D_5A_9E,
            icon_char: 'G',
            store: GameStoreName::RaeStore,
            installed: true,
            last_played: 0,
            playtime_hours: 0.0,
            rating: None,
            size_gb: 1.0,
            favorited: false,
            running: false,
        };
        couch.featured.push(e.clone());
        couch.library.push(e);
    }
    couch.navigate(GameOsPage::AllGames);
    couch.controller_focus = FocusTarget::GameGrid;

    // (1) Context hints are non-empty in the grid context.
    let hints = couch.context_hints();
    let chips = hints.len();

    // (2) Action→glyph mapping is correct AND per-set distinct. The active
    // (Xbox) "Select" must be "A"; PlayStation "Select" must be the cross "✕"
    // and must differ from Xbox's — proving the sets are genuinely selectable.
    let xbox_select = GlyphSet::Xbox.glyph(HintAction::Select);
    let ps_select = GlyphSet::PlayStation.glyph(HintAction::Select);
    let active_select = couch.glyph_set().glyph(HintAction::Select);
    let context_ok = active_select == "A"
        && xbox_select == "A"
        && ps_select == "\u{2715}"
        && ps_select != xbox_select
        // every grid-context chip resolved a non-empty glyph + label.
        && hints
            .iter()
            .all(|c| !c.glyph.is_empty() && !c.label.is_empty());

    // Ensure the AA font engine is ready (idempotent, boot-path-safe).
    raegfx::text::ensure_init();

    // Render the couch (paints the hint bar), then prove the hint-label text is
    // crisp AA by drawing a label through the SAME path the bar uses and reading
    // its coverage — 0 / uniform coverage == 8px block fallback == FAIL.
    couch.render(canvas);
    let stats = canvas.draw_text_aa_stats(
        SPACE_4 as i32,
        SPACE_4 as i32,
        HintAction::Select.label(),
        TYPE_COUCH_LABEL,
        TEXT_PRIMARY,
        FontFamily::Sans,
    );
    let glyphs_aa = stats.total_coverage > 0 && stats.max_cov > stats.min_cov;

    GlyphSmoketest {
        set_tag: couch.glyph_set().tag(),
        chips,
        context_ok,
        glyphs_aa,
    }
}

// ── Phase 3: live controller bind smoketest ────────────────────────────────

/// Outcome of the GameOS padbind smoketest (Phase 3). FAIL-able on each piece
/// of the live-controller contract: a malformed/undecodable frame, a hat-right
/// frame that does NOT move focus right, a wrong VID/PID→glyph-set mapping, or a
/// face-button that maps to the wrong couch action.
pub struct PadBindSmoketest {
    /// The decoded pad frame routed into couch nav without panicking, and the
    /// bound VID/PID was recorded.
    pub decoded_pad_ok: bool,
    /// A hat=East (D-pad right) frame moved the grid focus right by one.
    pub dpad_right_moves_focus: bool,
    /// The auto-selected glyph-set tag for the bound (Sony) pad.
    pub vidpid_set_tag: &'static str,
    /// The couch action face button A (bit 0) maps to.
    pub face_a_action: &'static str,
    /// Face-button-A press routed to Select AND opened the detail view.
    pub face_a_ok: bool,
}

impl PadBindSmoketest {
    #[must_use]
    pub fn passed(&self) -> bool {
        self.decoded_pad_ok
            && self.dpad_right_moves_focus
            && self.vidpid_set_tag == "ps"
            && self.face_a_action == "Select"
            && self.face_a_ok
    }
}

/// Run the Phase-3 padbind smoketest. Drives the couch from synthetic decoded
/// pad frames (the shape `hid_gamepad::decode_report` produces): binds a Sony
/// pad (→ auto-selects PlayStation glyphs), routes a hat-right frame and proves
/// focus moves right, then routes a face-button-A press and proves it maps to
/// the Select action (which opens the focused tile's detail). Pure logic — no
/// canvas needed, identical on QEMU and iron.
#[must_use]
pub fn run_padbind_smoketest() -> PadBindSmoketest {
    let mut couch = GameOsShell::new(1280, 720);
    couch.active = true;
    for i in 0..4u64 {
        let e = GameEntry {
            id: i + 1,
            title: alloc::format!("Game {}", i + 1),
            banner_color: 0xFF_2D_5A_9E,
            icon_char: 'G',
            store: GameStoreName::RaeStore,
            installed: true,
            last_played: 0,
            playtime_hours: 0.0,
            rating: None,
            size_gb: 1.0,
            favorited: false,
            running: false,
        };
        couch.library.push(e);
    }
    couch.navigate(GameOsPage::AllGames);
    couch.controller_focus = FocusTarget::GameGrid;

    // Bind a DualSense (Sony VID 0x054C, PID 0x0CE6) — auto-selects PS glyphs.
    couch.bind_pad(VID_SONY, 0x0CE6);
    let vidpid_set_tag = couch.glyph_set().tag();

    // Frame 1: hat = 2 (East / D-pad right), no buttons. Focus must move right.
    let start = couch.selected_index;
    let right_frame = PadFrame {
        hat: 2,
        ..PadFrame::default()
    };
    couch.apply_pad_frame(&right_frame);
    let dpad_right_moves_focus = couch.selected_index == start + 1;
    let decoded_pad_ok = couch.bound_pad() == Some((VID_SONY, 0x0CE6));

    // Frame 2: release the hat (centre) + press face button 1 (bit 0 / A / ✕).
    // On the grid this is Select → opens the detail view for the focused tile.
    let select_frame = PadFrame {
        hat: 8,
        buttons: 1 << 0,
        ..PadFrame::default()
    };
    let before = couch.detail_game;
    couch.apply_pad_frame(&select_frame);
    let face_a_action = match button_for_bit(0) {
        Some(GamepadButton::A) => "Select",
        _ => "wrong",
    };
    let face_a_ok = face_a_action == "Select" && before.is_none() && couch.detail_game.is_some();

    PadBindSmoketest {
        decoded_pad_ok,
        dpad_right_moves_focus,
        vidpid_set_tag,
        face_a_action,
        face_a_ok,
    }
}

// ── Phase 5: per-game profile editor smoketest (pure logic, host-KAT'd) ─────

/// Outcome of the GameOS profile-editor smoketest (Phase 5, surface half). The
/// kernel half (`shell_runner`) drives the REAL `game_profile` syscalls and
/// owns the `[gameos] profile smoketest:` line; this proves the SURFACE logic:
/// the editor opens, edits a field, normalizes to a SET-safe value, and the
/// working copy round-trips its exact values. FAIL-able on each.
pub struct ProfileEditorSmoketest {
    /// The editor opened over the focused game with the seeded profile.
    pub opened: bool,
    /// Editing a field changed the working profile (D-pad right stepped a value).
    pub edit_applied: bool,
    /// The edited working profile equals itself after `normalized()` — i.e. the
    /// editor never produces a value the kernel would reject (idempotent norm).
    pub normalized_stable: bool,
    /// Confirming latched a commit that yields the EXACT edited profile back.
    pub commit_roundtrip: bool,
    /// Number of editable fields the surface exposes (the `fields=K` count).
    pub fields: usize,
}

impl ProfileEditorSmoketest {
    #[must_use]
    pub fn passed(&self) -> bool {
        self.opened
            && self.edit_applied
            && self.normalized_stable
            && self.commit_roundtrip
            && self.fields == ProfileField::count()
    }
}

/// Run the Phase-5 profile-editor surface smoketest (pure logic — no canvas,
/// identical on QEMU and iron). Opens the editor over a seeded game, edits the
/// GPU-power-limit field, confirms it normalizes + round-trips exactly, then
/// confirms the commit latch hands back the EXACT edited record (the value the
/// shell_runner pushes through `SYS_GAME_PROFILE_SET`).
#[must_use]
pub fn run_profile_editor_smoketest() -> ProfileEditorSmoketest {
    let mut couch = GameOsShell::new(1280, 720);
    couch.active = true;
    couch.library.push(GameEntry {
        id: 730,
        title: String::from("Test Game"),
        banner_color: 0xFF_2D_5A_9E,
        icon_char: 'G',
        store: GameStoreName::RaeStore,
        installed: true,
        last_played: 0,
        playtime_hours: 0.0,
        rating: None,
        size_gb: 1.0,
        favorited: false,
        running: false,
    });
    couch.navigate(GameOsPage::AllGames);
    couch.controller_focus = FocusTarget::GameGrid;

    // Open the editor with an explicit seed (gpu_power_pct = 100).
    let mut seed = CouchProfile::default();
    seed.gpu_power_pct = 100;
    couch.open_profile_editor(0, seed);
    let opened = couch.profile_editor_open()
        && couch.profile_editor().map(|e| e.game_id.as_str()) == Some("game:730");

    // Focus the GPU-power field and step it DOWN once (-5 → 95%).
    let gpu_field_idx = ProfileField::ORDER
        .iter()
        .position(|&f| f == ProfileField::GpuPowerPct)
        .unwrap_or(0);
    if let Some(e) = couch.profile_editor.as_mut() {
        e.focused = gpu_field_idx;
        e.edit(-1);
    }
    let edited = couch.profile_editor().map(|e| e.profile);
    let edit_applied = edited.map(|p| p.gpu_power_pct) == Some(95);

    // Normalization is idempotent (the working copy is always SET-safe).
    let normalized_stable = edited.map(|p| p.normalized() == p).unwrap_or(false);

    // Confirm (A) → the commit latch yields the EXACT edited profile back.
    couch.controller_input(GamepadInput::Button(GamepadButton::A));
    let commit = couch.take_profile_commit();
    let commit_roundtrip = match (commit, edited) {
        (Some((id, p)), Some(want)) => id == "game:730" && p == want,
        _ => false,
    };
    // The editor closed after the commit was taken.
    let closed = !couch.profile_editor_open();

    ProfileEditorSmoketest {
        opened,
        edit_applied,
        normalized_stable,
        commit_roundtrip: commit_roundtrip && closed,
        fields: ProfileField::count(),
    }
}

// ── Phase 6: OSK + auto-enter + cross-fade smoketest (pure logic) ───────────

/// Outcome of the GameOS OSK / auto-enter / cross-fade smoketest (Phase 6). The
/// surface half proves the pure logic: typing the focused keys produces the
/// expected string, backspace deletes, the auto-enter trigger fires on a
/// synthetic pad-bind, and the cross-fade alpha ramp is well-formed. FAIL-able
/// on each. The kernel half (`shell_runner`) owns the printed `[gameos] osk
/// smoketest:` line and drives the wash through the live toggle.
pub struct OskSmoketest {
    /// Total keys across the OSK grid (the `keys=N` count).
    pub keys: usize,
    /// The string produced by navigating to + typing 'r','a','e' (must be "rae").
    pub typed: String,
    /// Backspace deleted the last typed char (so "rae" + ⌫ = "ra").
    pub backspace_ok: bool,
    /// A synthetic pad-bind on an idle session would auto-offer GameOS.
    pub autoenter_on_padbind: bool,
    /// The cross-fade peak duration (ms) — `CROSSFADE_MS`.
    pub crossfade_ms: u64,
    /// The cross-fade alpha ramp is well-formed: 0 at the endpoints, non-zero at
    /// the midpoint (the triangle dip-through-wash shape).
    pub crossfade_ramp_ok: bool,
}

impl OskSmoketest {
    #[must_use]
    pub fn passed(&self) -> bool {
        self.keys > 0
            && self.typed == "rae"
            && self.backspace_ok
            && self.autoenter_on_padbind
            && self.crossfade_ms == CROSSFADE_MS
            && self.crossfade_ramp_ok
    }
}

/// Total number of keys across the OSK grid — for the smoketest + procfs.
#[must_use]
pub fn osk_key_count() -> usize {
    OSK_ROWS.iter().map(|r| r.len()).sum()
}

/// Whether a freshly-bound pad on an idle/desktop session should auto-offer
/// GameOS (Phase 6 auto-enter). Pure policy: a real controller binding while no
/// game shell is up is the "go to the couch" signal. The shell_runner gates the
/// actual offer behind the `/gameos/auto_on_pad` setting; this is the trigger
/// predicate the smoketest exercises (it always fires for a known pad VID).
#[must_use]
pub fn should_offer_gameos_on_padbind(vid: u16, _pid: u16) -> bool {
    // Any of the controller-family VIDs (or a generic HID pad) is a valid
    // signal — the binding itself is the intent, not the brand.
    matches!(vid, VID_SONY | VID_MICROSOFT | VID_NINTENDO) || vid != 0
}

/// Drive a fresh OSK to type a target string by moving the key cursor to each
/// character and activating it. Returns nothing — mutates `osk.text`. Pure
/// logic — used by the smoketest to prove the layout→char path end to end.
fn osk_type_string(osk: &mut OskState, target: &str) {
    for want in target.chars() {
        // Find the (row, col) of the key that types `want` in the current shift
        // layer, then walk the cursor there and activate.
        let mut found: Option<(usize, usize)> = None;
        for (ri, row) in OSK_ROWS.iter().enumerate() {
            for (ci, &k) in row.iter().enumerate() {
                if let OskKey::Char(c) = k {
                    if resolve_char(c, osk.shift) == want {
                        found = Some((ri, ci));
                        break;
                    }
                }
            }
            if found.is_some() {
                break;
            }
        }
        if let Some((ri, ci)) = found {
            osk.row = ri;
            osk.col = ci;
            osk.activate();
        }
    }
}

/// Run the Phase-6 OSK / auto-enter / cross-fade smoketest (pure logic — no
/// canvas, identical on QEMU and iron). Opens an OSK, types "rae" by navigating
/// the grid, proves backspace deletes a char, checks the auto-enter predicate
/// fires on a synthetic Sony pad-bind, and validates the cross-fade alpha ramp
/// (0 at the endpoints, peak at the midpoint).
#[must_use]
pub fn run_osk_smoketest() -> OskSmoketest {
    let keys = osk_key_count();

    // Type "rae" via the grid cursor (the same activate() the live OSK uses).
    let mut osk = OskState::new(OskTarget::Search, String::new());
    osk_type_string(&mut osk, "rae");
    let typed = osk.text.clone();

    // Backspace deletes the last char: "rae" -> "ra".
    let mut bs = osk.clone();
    bs.backspace();
    let backspace_ok = bs.text == "ra";

    // Auto-enter fires for a bound DualSense (Sony VID).
    let autoenter_on_padbind = should_offer_gameos_on_padbind(VID_SONY, 0x0CE6);

    // Cross-fade ramp: a fresh fade is 0 alpha; the midpoint is the peak; the
    // end is 0 again (the triangle dip-through-wash).
    let start = CrossFade::begin(true);
    let mut mid = CrossFade::begin(true);
    let _ = mid.tick(CROSSFADE_MS / 2);
    let mut end = CrossFade::begin(true);
    let _ = end.tick(CROSSFADE_MS);
    let crossfade_ramp_ok =
        start.wash_alpha() == 0 && mid.wash_alpha() > 0 && end.wash_alpha() == 0;

    OskSmoketest {
        keys,
        typed,
        backspace_ok,
        autoenter_on_padbind,
        crossfade_ms: CROSSFADE_MS,
        crossfade_ramp_ok,
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

// ── Host KAT: action→glyph mapping (Phase 2, pure logic) ───────────────────

#[cfg(test)]
mod glyph_tests {
    use super::*;

    #[test]
    fn xbox_set_is_lettered() {
        let s = GlyphSet::Xbox;
        assert_eq!(s.glyph(HintAction::Select), "A");
        assert_eq!(s.glyph(HintAction::Back), "B");
        assert_eq!(s.glyph(HintAction::Details), "X");
        assert_eq!(s.glyph(HintAction::Search), "Y");
        assert_eq!(s.tag(), "xbox");
    }

    #[test]
    fn playstation_set_is_symbolic_and_distinct() {
        let ps = GlyphSet::PlayStation;
        assert_eq!(ps.glyph(HintAction::Select), "\u{2715}"); // ✕ cross
        assert_eq!(ps.glyph(HintAction::Back), "\u{25EF}"); // ◯ circle
        assert_eq!(ps.glyph(HintAction::Details), "\u{25A1}"); // □ square
        assert_eq!(ps.glyph(HintAction::Search), "\u{25B3}"); // △ triangle
                                                              // The selectable sets genuinely differ — the Phase-2 contract.
        for a in [
            HintAction::Select,
            HintAction::Back,
            HintAction::Details,
            HintAction::Search,
        ] {
            assert_ne!(
                ps.glyph(a),
                GlyphSet::Xbox.glyph(a),
                "PS and Xbox glyphs must differ for {a:?}"
            );
        }
        assert_eq!(ps.tag(), "ps");
    }

    #[test]
    fn nintendo_and_generic_are_lettered() {
        for s in [GlyphSet::Nintendo, GlyphSet::Generic] {
            assert_eq!(s.glyph(HintAction::Select), "A");
            assert_eq!(s.glyph(HintAction::Back), "B");
        }
        assert_eq!(GlyphSet::Nintendo.tag(), "nintendo");
        assert_eq!(GlyphSet::Generic.tag(), "generic");
    }

    #[test]
    fn every_set_resolves_every_action_nonempty() {
        for s in [
            GlyphSet::Xbox,
            GlyphSet::PlayStation,
            GlyphSet::Nintendo,
            GlyphSet::Generic,
        ] {
            for a in [
                HintAction::Select,
                HintAction::Back,
                HintAction::Details,
                HintAction::Search,
                HintAction::Favorite,
                HintAction::Menu,
                HintAction::Page,
                HintAction::Section,
            ] {
                assert!(!s.glyph(a).is_empty(), "{s:?}/{a:?} glyph empty");
                assert!(!a.label().is_empty(), "{a:?} label empty");
            }
        }
    }

    #[test]
    fn default_set_is_settable_no_fake_detection() {
        let mut couch = GameOsShell::new(1280, 720);
        assert_eq!(couch.glyph_set(), GlyphSet::DEFAULT);
        couch.set_glyph_set(GlyphSet::PlayStation);
        assert_eq!(couch.glyph_set(), GlyphSet::PlayStation);
        assert_eq!(
            couch.glyph_set().glyph(HintAction::Select),
            "\u{2715}",
            "active set drives the resolved glyph"
        );
    }

    #[test]
    fn context_hints_are_context_aware_and_nonempty() {
        let mut couch = GameOsShell::new(1280, 720);
        couch.controller_focus = FocusTarget::GameGrid;
        let grid = couch.context_hints();
        assert!(grid.len() >= 3, "grid context shows several actions");

        couch.controller_focus = FocusTarget::Sidebar;
        let rail = couch.context_hints();
        assert!(!rail.is_empty());
        // The two contexts differ — the bar is genuinely context-sensitive.
        assert_ne!(
            grid.len(),
            rail.len(),
            "grid and sidebar contexts show different chip counts"
        );

        // Detail view exposes Favorite.
        couch.select_game(0);
        // (no library, so detail_game stays None — exercise the quick-menu path)
        couch.quick_menu.visible = true;
        let qm = couch.context_hints();
        assert!(!qm.is_empty());
    }

    // ── Phase 3: live controller bind (pure mapping logic) ─────────────────

    #[test]
    fn vidpid_maps_to_correct_glyph_set() {
        assert_eq!(
            glyph_set_for_vidpid(VID_SONY, 0x0CE6),
            GlyphSet::PlayStation
        );
        assert_eq!(glyph_set_for_vidpid(VID_MICROSOFT, 0x028E), GlyphSet::Xbox);
        assert_eq!(
            glyph_set_for_vidpid(VID_NINTENDO, 0x2009),
            GlyphSet::Nintendo
        );
        // Unknown vendor → Generic (never a guessed first-party skin).
        assert_eq!(glyph_set_for_vidpid(0x28DE, 0x1142), GlyphSet::Generic); // Valve
        assert_eq!(glyph_set_tag_for_vidpid(VID_SONY, 0x0CE6), "ps");
    }

    #[test]
    fn hat_drives_focus_delta_in_every_direction() {
        let d = |hat: u8| {
            focus_delta_for_frame(&PadFrame {
                hat,
                ..PadFrame::default()
            })
        };
        assert_eq!(d(0), (0, -1)); // N
        assert_eq!(d(2), (1, 0)); // E
        assert_eq!(d(4), (0, 1)); // S
        assert_eq!(d(6), (-1, 0)); // W
        assert_eq!(d(1), (1, -1)); // NE
        assert_eq!(d(8), (0, 0)); // centre = no movement
    }

    #[test]
    fn left_stick_drives_focus_past_deadzone_only() {
        // Inside the 0.3 deadzone → no movement.
        let small = PadFrame {
            hat: 8,
            x: 8000,
            ..PadFrame::default()
        };
        assert_eq!(focus_delta_for_frame(&small), (0, 0));
        // Full right past the deadzone → +x.
        let right = PadFrame {
            hat: 8,
            x: 30000,
            ..PadFrame::default()
        };
        assert_eq!(focus_delta_for_frame(&right), (1, 0));
        // Full up (y negative) → -y.
        let up = PadFrame {
            hat: 8,
            y: -30000,
            ..PadFrame::default()
        };
        assert_eq!(focus_delta_for_frame(&up), (0, -1));
        // Hat wins over the stick when both are active.
        let both = PadFrame {
            hat: 6,
            x: 30000,
            ..PadFrame::default()
        };
        assert_eq!(focus_delta_for_frame(&both), (-1, 0));
    }

    #[test]
    fn face_buttons_map_to_couch_actions() {
        assert_eq!(button_for_bit(0), Some(GamepadButton::A)); // Select
        assert_eq!(button_for_bit(1), Some(GamepadButton::B)); // Back
        assert_eq!(button_for_bit(2), Some(GamepadButton::X)); // Details
        assert_eq!(button_for_bit(3), Some(GamepadButton::Y)); // Search
        assert_eq!(button_for_bit(10), Some(GamepadButton::Guide)); // Menu
        assert_eq!(button_for_bit(31), None); // unmapped vendor button
    }

    #[test]
    fn bind_pad_auto_selects_set_and_records_vidpid() {
        let mut couch = GameOsShell::new(1280, 720);
        assert_eq!(couch.bound_pad(), None);
        couch.bind_pad(VID_SONY, 0x0CE6);
        assert_eq!(couch.bound_pad(), Some((VID_SONY, 0x0CE6)));
        assert_eq!(couch.glyph_set(), GlyphSet::PlayStation);
        couch.bind_pad(VID_MICROSOFT, 0x028E);
        assert_eq!(couch.glyph_set(), GlyphSet::Xbox);
    }

    #[test]
    fn apply_pad_frame_moves_focus_and_fires_on_press_edge() {
        let mut couch = GameOsShell::new(1280, 720);
        for i in 0..4u64 {
            couch.library.push(GameEntry {
                id: i + 1,
                title: alloc::format!("G{}", i),
                banner_color: 0,
                icon_char: 'G',
                store: GameStoreName::RaeStore,
                installed: true,
                last_played: 0,
                playtime_hours: 0.0,
                rating: None,
                size_gb: 1.0,
                favorited: false,
                running: false,
            });
        }
        couch.navigate(GameOsPage::AllGames);
        couch.controller_focus = FocusTarget::GameGrid;

        // Hat right moves focus right by one.
        let start = couch.selected_index;
        couch.apply_pad_frame(&PadFrame {
            hat: 2,
            ..PadFrame::default()
        });
        assert_eq!(couch.selected_index, start + 1);

        // Press A (bit 0) → Select → opens detail.
        couch.apply_pad_frame(&PadFrame {
            hat: 8,
            buttons: 1,
            ..PadFrame::default()
        });
        assert!(couch.detail_game.is_some());

        // HOLDING A across the next frame must NOT re-fire (press-edge only):
        // close detail manually, then a frame with A still down does nothing.
        couch.detail_game = None;
        couch.apply_pad_frame(&PadFrame {
            hat: 8,
            buttons: 1,
            ..PadFrame::default()
        });
        assert!(
            couch.detail_game.is_none(),
            "held button must not re-fire the action"
        );
    }

    #[test]
    fn padbind_smoketest_passes() {
        let r = run_padbind_smoketest();
        assert!(r.passed(), "padbind smoketest must pass on the host KAT");
        assert_eq!(r.vidpid_set_tag, "ps");
        assert_eq!(r.face_a_action, "Select");
    }

    // ── Phase 5: per-game profile editor (pure logic) ──────────────────────

    #[test]
    fn profile_flag_bits_match_kernel() {
        // The raeshell mirror flag bits MUST match kernel::game_profile::FLAG_*.
        assert_eq!(PROFILE_FLAG_GAME_MODE, 1 << 0);
        assert_eq!(PROFILE_FLAG_NULL_LATENCY, 1 << 1);
        assert_eq!(PROFILE_FLAG_HDR, 1 << 2);
        assert_eq!(PROFILE_FLAG_VRR, 1 << 3);
    }

    #[test]
    fn normalize_is_idempotent_and_clamps() {
        // GPU power over 100 clamps; a stray flag bit is stripped; 0 affinity
        // becomes the default; a non-enumerated resolution snaps; priority snaps.
        let mut p = CouchProfile::default();
        p.gpu_power_pct = 250;
        p.flags |= 1 << 20; // undefined bit
        p.affinity_mask = 0;
        p.resolution_w = 1234;
        p.resolution_h = 567;
        p.refresh_hz = 99; // not enumerated
        p.priority = 7;
        let n = p.normalized();
        assert_eq!(n.gpu_power_pct, 100);
        assert_eq!(n.flags & (1 << 20), 0);
        assert_ne!(n.affinity_mask, 0);
        assert!(PROFILE_RESOLUTIONS.contains(&(n.resolution_w, n.resolution_h)));
        assert!(PROFILE_REFRESH_RATES.contains(&n.refresh_hz));
        assert!(n.priority == 0 || n.priority == 2);
        // Idempotent: normalizing again changes nothing.
        assert_eq!(n.normalized(), n);
    }

    #[test]
    fn field_step_edits_and_clamps_at_ends() {
        let mut p = CouchProfile::default();
        // GPU power steps by 5, clamped to [0,100].
        p.gpu_power_pct = 100;
        ProfileField::GpuPowerPct.step(&mut p, 1);
        assert_eq!(p.gpu_power_pct, 100, "clamps at the top");
        ProfileField::GpuPowerPct.step(&mut p, -1);
        assert_eq!(p.gpu_power_pct, 95);
        // Toggles flip.
        p.flags = 0;
        ProfileField::Hdr.step(&mut p, 1);
        assert!(p.flags & PROFILE_FLAG_HDR != 0);
        ProfileField::Hdr.step(&mut p, -1);
        assert!(p.flags & PROFILE_FLAG_HDR == 0);
        // Resolution dropdown clamps at the bottom (no wrap).
        p.resolution_w = PROFILE_RESOLUTIONS[0].0;
        p.resolution_h = PROFILE_RESOLUTIONS[0].1;
        ProfileField::Resolution.step(&mut p, -1);
        assert_eq!((p.resolution_w, p.resolution_h), PROFILE_RESOLUTIONS[0]);
    }

    #[test]
    fn value_strings_render_every_field() {
        let p = CouchProfile::default();
        for &f in ProfileField::ORDER {
            assert!(!f.value_string(&p).is_empty(), "{f:?} value empty");
            assert!(!f.label().is_empty(), "{f:?} label empty");
        }
        assert_eq!(ProfileField::count(), 8);
    }

    #[test]
    fn editor_open_edit_commit_roundtrips() {
        let r = run_profile_editor_smoketest();
        assert!(r.opened, "editor opens over the focused game");
        assert!(r.edit_applied, "editing a field changed the working copy");
        assert!(r.normalized_stable, "editor never produces an unsafe value");
        assert!(
            r.commit_roundtrip,
            "commit hands back the exact edited profile"
        );
        assert_eq!(r.fields, 8);
        assert!(r.passed());
    }

    #[test]
    fn launch_records_request_for_auto_apply() {
        // Launching a game records a launch request so the runner APPLYs first.
        let mut couch = GameOsShell::new(1280, 720);
        couch.library.push(GameEntry {
            id: 42,
            title: String::from("G"),
            banner_color: 0,
            icon_char: 'G',
            store: GameStoreName::RaeStore,
            installed: true,
            last_played: 0,
            playtime_hours: 0.0,
            rating: None,
            size_gb: 1.0,
            favorited: false,
            running: false,
        });
        couch.navigate(GameOsPage::AllGames);
        couch.controller_focus = FocusTarget::GameGrid;
        assert!(couch.launch_game(0).is_some());
        assert_eq!(couch.take_launch_request(), Some(0));
        // Taken exactly once.
        assert_eq!(couch.take_launch_request(), None);
    }

    // ── Phase 6: OSK + auto-enter + cross-fade (pure logic) ────────────────

    #[test]
    fn osk_resolves_chars_under_shift() {
        assert_eq!(resolve_char('a', false), 'a');
        assert_eq!(resolve_char('a', true), 'A');
        assert_eq!(resolve_char('1', false), '1');
        assert_eq!(resolve_char('1', true), '!');
        assert_eq!(resolve_char('2', true), '@');
        assert_eq!(resolve_char('3', true), '#');
    }

    #[test]
    fn osk_grid_is_nonempty_and_clamped() {
        assert!(osk_key_count() > 0);
        let mut osk = OskState::new(OskTarget::Search, String::new());
        // Walking far off the end of a short row must clamp, never panic.
        for _ in 0..50 {
            osk.move_focus(1, 0);
        }
        for _ in 0..50 {
            osk.move_focus(0, 1);
        }
        let _ = osk.focused_key(); // must not panic on any (row, col).
    }

    #[test]
    fn osk_types_backspaces_and_commits() {
        let mut osk = OskState::new(OskTarget::Search, String::new());
        osk_type_string(&mut osk, "rae");
        assert_eq!(osk.text, "rae");
        osk.backspace();
        assert_eq!(osk.text, "ra");
        // Shift then type → upper-case.
        osk.shift = true;
        osk_type_string(&mut osk, "X");
        assert_eq!(osk.text, "raX");
        // Enter latches a commit.
        // Find the Enter key, focus it, activate.
        for (ri, row) in OSK_ROWS.iter().enumerate() {
            for (ci, &k) in row.iter().enumerate() {
                if k == OskKey::Enter {
                    osk.row = ri;
                    osk.col = ci;
                }
            }
        }
        osk.shift = false;
        osk.activate();
        assert!(osk.commit_requested);
    }

    #[test]
    fn osk_opens_from_search_and_feeds_query() {
        let mut couch = GameOsShell::new(1280, 720);
        couch.library.push(GameEntry {
            id: 1,
            title: String::from("operae"),
            banner_color: 0,
            icon_char: 'G',
            store: GameStoreName::RaeStore,
            installed: true,
            last_played: 0,
            playtime_hours: 0.0,
            rating: None,
            size_gb: 1.0,
            favorited: false,
            running: false,
        });
        couch.navigate(GameOsPage::AllGames);
        couch.controller_focus = FocusTarget::GameGrid;
        // Y opens the search + OSK.
        couch.controller_input(GamepadInput::Button(GamepadButton::Y));
        assert!(couch.osk_open(), "Y opens the OSK over the search field");
        // Type "rae" through the live button path (A activates the focused key).
        {
            let osk = couch.osk.as_mut().unwrap();
            osk_type_string(osk, "rae");
        }
        // Commit via Start → the query feeds the search + the OSK closes.
        couch.controller_input(GamepadInput::Button(GamepadButton::Start));
        assert!(!couch.osk_open(), "commit closes the OSK");
        assert_eq!(couch.search.query, "rae");
        // The matching game surfaced.
        assert_eq!(couch.search.results.len(), 1);
    }

    #[test]
    fn osk_button_path_types_and_backspaces() {
        let mut couch = GameOsShell::new(1280, 720);
        couch.open_osk(OskTarget::Search, String::new());
        // Drive to the 'r' key and press A.
        for (ri, row) in OSK_ROWS.iter().enumerate() {
            for (ci, &k) in row.iter().enumerate() {
                if k == OskKey::Char('r') {
                    if let Some(o) = couch.osk.as_mut() {
                        o.row = ri;
                        o.col = ci;
                    }
                }
            }
        }
        couch.controller_input(GamepadInput::Button(GamepadButton::A));
        assert_eq!(couch.osk().unwrap().text, "r");
        // X = backspace.
        couch.controller_input(GamepadInput::Button(GamepadButton::X));
        assert_eq!(couch.osk().unwrap().text, "");
        // B on an empty buffer closes the OSK.
        couch.controller_input(GamepadInput::Button(GamepadButton::B));
        assert!(!couch.osk_open());
    }

    #[test]
    fn crossfade_ramp_is_triangle() {
        let start = CrossFade::begin(true);
        assert_eq!(start.wash_alpha(), 0, "0 alpha at the start");
        let mut mid = CrossFade::begin(true);
        assert!(mid.tick(CROSSFADE_MS / 2));
        assert!(mid.wash_alpha() > 0, "peak wash at the midpoint");
        let mut end = CrossFade::begin(true);
        assert!(!end.tick(CROSSFADE_MS), "fade completes at CROSSFADE_MS");
        assert_eq!(end.wash_alpha(), 0, "0 alpha at the end");
        // The wash color carries bg.base with the alpha byte.
        let c = mid.wash_color();
        assert!((c >> 24) & 0xFF != 0);
        assert_eq!(c & 0x00FF_FFFF, BG_DARK & 0x00FF_FFFF);
    }

    #[test]
    fn crossfade_drives_through_shell() {
        let mut couch = GameOsShell::new(640, 480);
        assert!(!couch.crossfade_active());
        couch.begin_crossfade(true);
        assert!(couch.crossfade_active());
        assert!(couch.crossfade_wash().is_some());
        // Tick to completion → drops.
        let mut running = true;
        let mut guard = 0;
        while running && guard < 100 {
            running = couch.tick_crossfade(32);
            guard += 1;
        }
        assert!(!couch.crossfade_active(), "fade drops when complete");
    }

    #[test]
    fn autoenter_predicate_fires_for_known_pads() {
        assert!(should_offer_gameos_on_padbind(VID_SONY, 0x0CE6));
        assert!(should_offer_gameos_on_padbind(VID_MICROSOFT, 0x028E));
        assert!(should_offer_gameos_on_padbind(VID_NINTENDO, 0x2009));
        // A generic HID pad (non-zero VID) still triggers.
        assert!(should_offer_gameos_on_padbind(0x28DE, 0x1142));
        // A zero VID (no real device) does not.
        assert!(!should_offer_gameos_on_padbind(0, 0));
    }

    #[test]
    fn osk_smoketest_passes() {
        let r = run_osk_smoketest();
        assert!(r.passed(), "osk smoketest must pass on the host KAT");
        assert_eq!(r.typed, "rae");
        assert!(r.backspace_ok);
        assert!(r.autoenter_on_padbind);
        assert_eq!(r.crossfade_ms, CROSSFADE_MS);
        assert!(r.crossfade_ramp_ok);
    }
}
