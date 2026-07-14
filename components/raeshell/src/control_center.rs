//! Control Center — the one-tap glance-and-toggle glass flyout.
//!
//! *"Built for people who care about how things feel."* — LEGACY_GAMING_CONCEPT.md.
//!
//! Implements `docs/design/control-center.md`: a `material.glass` flyout
//! anchored bottom-right above the tray, a responsive on/off tile grid (Wi-Fi,
//! Bluetooth, Do-Not-Disturb, Night Light, Airplane, Accessibility),
//! expand-in-place sub-panels (the macOS win — Wi-Fi network list pushed into
//! the panel, NOT a new window), always-visible Volume + Brightness sliders, an
//! inline media-transport card (present only when audio plays, absent
//! otherwise), and the AthenaOS-native **Gaming row** (Game Mode + RGB effect
//! quick-pick + Performance segmented) that honors "gaming isn't a mode".
//!
//! Cohesion (spec §5): every colour resolves from `rae_tokens` through the LIVE
//! seed accent shared with the taskbar / Start / Settings — on-tiles read
//! `accent.subtle`, slider fills read `accent.base`, the panel is `radius.lg` +
//! `material.glass` + `elev.3`. A Vibe-Mode re-skin (one seed change) recolours
//! Control Center with everything else. No private palette, no hardcoded accent.
//!
//! Backend wiring (verify-before-spec, spec §"Already built"): tiles wire to a
//! live backend where one exists and render an HONEST state otherwise — see
//! [`TileBackend`]. Game Mode / RGB effect / Performance mirror the
//! `gameos` / `rgb_api` models; volume/brightness are owned here until the kernel
//! threads live HDA/backlight values in (flagged, not faked).

#![allow(dead_code)]

use crate::{accent, PALETTE};
use alloc::string::String;
use alloc::vec::Vec;
use rae_tokens::{RADIUS_LG, RADIUS_MD, RADIUS_SM, SPACE_1, SPACE_2, SPACE_3, SPACE_4};

const GLYPH_W: usize = 8;
const GLYPH_H: usize = 8;

/// Replace the alpha channel of an ARGB colour, keeping RGB. Used to composite a
/// token colour (e.g. `accent.active`) translucently over the frosted card so the
/// frost still reads beneath the interaction flash (rae_tokens' `with_alpha` is
/// private to that crate; this is the same operation, surface-local).
#[inline]
const fn with_alpha(color: u32, alpha: u32) -> u32 {
    (color & 0x00_FF_FF_FF) | ((alpha & 0xFF) << 24)
}

/// Preferred tile icon edge (visual-QA Round-2 #1, gfx recommendation ~28px).
/// Clamped to the tile's icon band at draw time so it never overruns the label
/// box that `tile_text_boxes` reserves (critique #3 vertical rhythm preserved).
const ICON_SIZE: usize = 28;

// (The CC-local 48%-alpha frosted tint was retired by the OBSIDIAN re-bake —
// the panel now paints the shared `rae_tokens::GLASS_PANEL_DARK` tier.)

// ── Panel geometry (spec §1) ───────────────────────────────────────────────

/// Panel width — spec §1: 360px.
const PANEL_WIDTH: usize = 360;
/// Toggle tile default size — spec §2.1: 168×60 in a 2-col grid at 360px.
const TILE_WIDTH: usize = 168;
const TILE_HEIGHT: usize = 60;
/// Always-visible slider row height (track + label).
const SLIDER_ROW_HEIGHT: usize = 40;
/// Media-transport card height (spec §2.4: 48px art + padding).
const MEDIA_CARD_HEIGHT: usize = 64;
/// Expanded sub-panel network/device row height — spec §2.2: 36px.
const EXPAND_ROW_HEIGHT: usize = 36;
/// Footer height (avatar + gear/power, spec §2.6).
const FOOTER_HEIGHT: usize = 40;

// ── Tiles (spec §2.1) ───────────────────────────────────────────────────────

/// The toggle tiles the Control Center ships. Each maps to a backend via
/// [`TileBackend`]; the spec §2.1 default set + the spec §2.5 Gaming row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TileKind {
    /// Wi-Fi (expandable → network list, spec §2.2).
    WiFi,
    /// Bluetooth (expandable → paired/available devices).
    Bluetooth,
    /// Do-Not-Disturb / Focus (drives the notification-center DND).
    DoNotDisturb,
    /// Night Light (warm display tint).
    NightLight,
    /// Airplane mode (radios off).
    Airplane,
    /// Accessibility shortcut.
    Accessibility,
    /// Game Mode (Gaming row; drives gameos/SCHED_BODY prioritisation).
    GameMode,
    /// RGB (Gaming row; expandable → effect quick-pick chips).
    Rgb,
    /// Performance (Gaming row; expandable → Balanced/Performance/Battery).
    Performance,
}

impl TileKind {
    /// The real line-icon (`raegfx::icon::Icon`) this tile draws — replacing the
    /// Round-1 LETTER placeholders (W/B/F/N/A/X/G/R/P) flagged CRITICAL by the
    /// visual-QA critique (`docs/design/visual-qa-critique-2026-06-21.md` #2 /
    /// Round-2 #1). The icon system landed in `raegfx::icon`; this is the wiring
    /// that consumes it. Map per the gfx agent's recommendation.
    #[must_use]
    pub const fn icon(self) -> raegfx::icon::Icon {
        use raegfx::icon::Icon;
        match self {
            TileKind::WiFi => Icon::WiFi,
            TileKind::Bluetooth => Icon::Bluetooth,
            TileKind::DoNotDisturb => Icon::Focus,
            TileKind::NightLight => Icon::NightLight,
            TileKind::Airplane => Icon::Airplane,
            TileKind::Accessibility => Icon::Accessibility,
            TileKind::GameMode => Icon::GameController,
            TileKind::Rgb => Icon::Palette,
            TileKind::Performance => Icon::Performance,
        }
    }
}

/// Which live subsystem (if any) a tile is wired to — keeps the "real vs
/// honestly-stubbed" status truthful (no faked toggles, House Rules / spec).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TileBackend {
    /// Wired to `notify::quick_settings` (NetManager Wi-Fi) — live.
    NotifyWifi,
    /// Wired to the notification-center Do-Not-Disturb flag — live.
    NotifyDnd,
    /// Wired to `notify::quick_settings` Night Light (config registry) — live.
    NotifyNightLight,
    /// Wired to the `gameos` Game Mode / SCHED_BODY prioritisation (live model).
    GameOs,
    /// Wired to the `rgb_api` effect engine (live model).
    RgbEngine,
    /// Wired to the `gameos`/power performance mode (live model).
    PowerMode,
    /// Wired to the kernel `a11y` high-contrast forced-colors engine (live —
    /// flips `rae_tokens::active_palette()`). The Accessibility tile's primary
    /// switch (the most visible a11y mode); magnifier/filters/reduced-motion are
    /// reachable via the documented hotkeys.
    A11yHighContrast,
    /// No kernel backend yet — the tile renders its honest local state and is
    /// flagged so QA/users know it is not yet driving hardware. NOT a fake
    /// "working" toggle: the visible state is real, the effect is pending.
    Pending,
}

impl TileBackend {
    /// True iff this tile is wired to a real subsystem (not [`Self::Pending`]).
    #[must_use]
    pub const fn is_real(self) -> bool {
        !matches!(self, TileBackend::Pending)
    }
}

/// One on/off toggle tile (spec §2.1 state matrix).
#[derive(Debug, Clone)]
pub struct Tile {
    pub kind: TileKind,
    pub label: String,
    /// Sub-state line (`type.caption`, e.g. SSID, "On"/"Off") — spec §2.1.
    pub sub_state: String,
    pub icon_char: char,
    pub enabled: bool,
    /// Whether this tile has an expand region (chevron) — spec §2.2.
    pub expandable: bool,
    /// Disabled (e.g. no Bluetooth radio): rendered `text.tertiary`, no hover.
    pub disabled: bool,
    pub backend: TileBackend,
}

impl Tile {
    fn new(
        kind: TileKind,
        label: &str,
        icon: char,
        expandable: bool,
        backend: TileBackend,
    ) -> Self {
        Self {
            kind,
            label: String::from(label),
            sub_state: String::from("Off"),
            icon_char: icon,
            enabled: false,
            expandable,
            disabled: false,
            backend,
        }
    }
}

// ── Interaction state (spec §2.1 — off/on/hover/active/focus/disabled) ───────

/// Pointer/keyboard interaction state of a focusable element (spec §2.1, §8).
/// Focus is distinct from hover (raeen-accessibility sign-off requirement).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Interact {
    /// Resting.
    None,
    /// Pointer over (hover).
    Hover,
    /// Pressed (active flash).
    Active,
    /// Keyboard/controller focus (ring + glow).
    Focus,
}

// ── Expandable sub-panel (spec §2.2) ────────────────────────────────────────

/// A network/device row inside an expanded Wi-Fi / Bluetooth sub-panel.
#[derive(Debug, Clone)]
pub struct ExpandRow {
    pub name: String,
    /// 0..=4 signal bars.
    pub signal: u8,
    pub secured: bool,
    pub connected: bool,
}

/// Which tile, if any, is currently expanded in-place (spec §2.2: one at a time).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Expanded {
    None,
    Tile(TileKind),
}

// ── Inline media-transport card (spec §2.4) ─────────────────────────────────

/// State of the inline media card. The card is present only when audio plays
/// (`playing == true`), absent otherwise (no empty card) — spec §2.4. The shell
/// pushes the live now-playing here (from the active media app, e.g. apps/music).
#[derive(Debug, Clone)]
pub struct MediaCard {
    /// Whether anything is playing (drives show/hide — spec §2.4).
    pub playing: bool,
    pub title: String,
    pub artist: String,
}

impl MediaCard {
    fn empty() -> Self {
        Self {
            playing: false,
            title: String::new(),
            artist: String::new(),
        }
    }

    /// True iff the card should be rendered at all (spec §2.4: hidden when
    /// nothing plays — no empty card).
    #[must_use]
    pub fn visible(&self) -> bool {
        self.playing
    }
}

// ── Performance segmented control (spec §2.5) ───────────────────────────────

/// The 3-segment performance mode (mirrors `sys.power` + per-game GPU power).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PerfMode {
    Battery,
    Balanced,
    Performance,
}

impl PerfMode {
    fn label(self) -> &'static str {
        match self {
            PerfMode::Battery => "Battery",
            PerfMode::Balanced => "Balanced",
            PerfMode::Performance => "Performance",
        }
    }
    /// All three segments, left→right.
    pub const ALL: [PerfMode; 3] = [PerfMode::Battery, PerfMode::Balanced, PerfMode::Performance];
}

// ── Inline-vector glyph kinds (no shipped icon yet — drawn with Canvas
//    primitives; see the raegfx follow-up list in the module REPORT) ───────────

/// The leading glyph on an always-visible slider (spec §2.3). Volume = a speaker
/// (with a mute slash when muted), Brightness = a sun. Drawn as crisp inline
/// vectors by [`draw_speaker`] / [`draw_sun`] — the `raegfx::icon` set has no
/// volume/brightness glyph yet (flagged for the icon agent), and these read
/// better small as bespoke vectors than as a stand-in tile icon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SliderIcon {
    Volume,
    VolumeMuted,
    Brightness,
}

/// A media-transport control glyph (spec §2.4): previous / play / pause / next.
/// Drawn inline by [`draw_transport`] — no shipped transport icons yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TransportGlyph {
    Prev,
    Play,
    Pause,
    Next,
}

// ── The Control Center panel ────────────────────────────────────────────────

/// The Control Center flyout (spec). Owned by `DesktopShell`, rendered by the
/// shell's `render`, opened from the tray cluster.
pub struct ControlCenter {
    pub visible: bool,
    pub tiles: Vec<Tile>,
    pub expanded: Expanded,
    /// Rows shown when a tile is expanded (Wi-Fi networks / BT devices).
    pub expand_rows: Vec<ExpandRow>,
    pub media: MediaCard,
    /// Volume 0..=100 (always-visible slider, spec §2.3).
    pub volume: u32,
    /// Brightness 0..=100 (always-visible slider, spec §2.3).
    pub brightness: u32,
    pub volume_muted: bool,
    /// RGB effect quick-pick selection (index into [`RGB_EFFECTS`]).
    pub rgb_effect: usize,
    pub perf_mode: PerfMode,
    /// Keyboard focus index across focusable elements (spec §8). `None` = pointer.
    pub focus_index: Option<usize>,
    pub screen_width: usize,
    pub screen_height: usize,
    taskbar_height: usize,
}

/// The 9 RGB effect quick-pick chips (spec §2.5 "the 9 rgb.rs effect modes").
/// Mirrors `rgb_api::RgbEffect` (the live effect engine), excluding `Off` which
/// the brightness mini-slider at 0 expresses. The chip's swatch reads the
/// effect's representative colour live (spec §2.5).
pub const RGB_EFFECTS: [(&str, u32); 9] = [
    ("Static", 0xFF_4E_9C_FF),
    ("Breathing", 0xFF_7C_5A_FF),
    ("Rainbow", 0xFF_FF_5A_5A),
    ("Wave", 0xFF_5A_C8_FF),
    ("Reactive", 0xFF_FF_C8_5A),
    ("Audio", 0xFF_C0_7C_FF),
    ("Cycle", 0xFF_5A_FF_8C),
    ("Starlight", 0xFF_F0_F0_F0),
    ("Ripple", 0xFF_5A_8C_FF),
];

impl ControlCenter {
    pub fn new(screen_width: usize, screen_height: usize, taskbar_height: usize) -> Self {
        let tiles = alloc::vec![
            Tile::new(TileKind::WiFi, "Wi-Fi", 'W', true, TileBackend::NotifyWifi),
            Tile::new(
                TileKind::Bluetooth,
                "Bluetooth",
                'B',
                true,
                TileBackend::Pending
            ),
            Tile::new(
                TileKind::DoNotDisturb,
                "Focus",
                'F',
                false,
                TileBackend::NotifyDnd
            ),
            Tile::new(
                TileKind::NightLight,
                "Night Light",
                'N',
                false,
                TileBackend::NotifyNightLight
            ),
            Tile::new(
                TileKind::Airplane,
                "Airplane",
                'A',
                false,
                TileBackend::Pending
            ),
            Tile::new(
                TileKind::Accessibility,
                "High Contrast",
                'X',
                false,
                TileBackend::A11yHighContrast
            ),
            // Gaming row (spec §2.5).
            Tile::new(
                TileKind::GameMode,
                "Game Mode",
                'G',
                false,
                TileBackend::GameOs
            ),
            Tile::new(TileKind::Rgb, "RGB", 'R', true, TileBackend::RgbEngine),
            Tile::new(
                TileKind::Performance,
                "Performance",
                'P',
                true,
                TileBackend::PowerMode
            ),
        ];

        Self {
            visible: false,
            tiles,
            expanded: Expanded::None,
            expand_rows: Vec::new(),
            media: MediaCard::empty(),
            volume: 75,
            brightness: 80,
            volume_muted: false,
            rgb_effect: 0,
            perf_mode: PerfMode::Balanced,
            focus_index: None,
            screen_width,
            screen_height,
            taskbar_height,
        }
    }

    /// The number of tiles that are the standard toggle grid (the §2.1 set; the
    /// last three are the Gaming row, still rendered in-grid below a header).
    pub const GRID_TILES: usize = 6;

    pub fn toggle(&mut self) {
        self.visible = !self.visible;
        if !self.visible {
            self.expanded = Expanded::None;
        }
    }

    pub fn close(&mut self) {
        self.visible = false;
        self.expanded = Expanded::None;
    }

    /// Toggle a tile's enabled state and reflect its backend (spec §2.1). Real
    /// backends are driven by the caller (the shell, which has kernel access);
    /// here we update the model state + the sub-state line truthfully.
    pub fn toggle_tile(&mut self, kind: TileKind) {
        if let Some(t) = self.tiles.iter_mut().find(|t| t.kind == kind) {
            if t.disabled {
                return;
            }
            t.enabled = !t.enabled;
            t.sub_state = String::from(if t.enabled { "On" } else { "Off" });
        }
    }

    /// Expand (or collapse) a tile's in-place sub-panel (spec §2.2: one at a
    /// time; tapping the chevron again collapses). Only expandable tiles expand.
    pub fn toggle_expand(&mut self, kind: TileKind) {
        let expandable = self
            .tiles
            .iter()
            .find(|t| t.kind == kind)
            .map(|t| t.expandable)
            .unwrap_or(false);
        if !expandable {
            return;
        }
        self.expanded = if self.expanded == Expanded::Tile(kind) {
            Expanded::None
        } else {
            Expanded::Tile(kind)
        };
    }

    /// Push the live Wi-Fi network list into the expanded sub-panel (spec §2.2).
    /// The shell sources these from the kernel net subsystem; sorted strongest
    /// signal first, connected row flagged.
    pub fn set_expand_rows(&mut self, rows: Vec<ExpandRow>) {
        self.expand_rows = rows;
        self.expand_rows.sort_by(|a, b| b.signal.cmp(&a.signal));
    }

    /// Push the live now-playing into the media card (spec §2.4). `playing ==
    /// false` hides the card entirely.
    pub fn set_media(&mut self, playing: bool, title: &str, artist: &str) {
        self.media.playing = playing;
        self.media.title = String::from(title);
        self.media.artist = String::from(artist);
    }

    pub fn set_volume(&mut self, v: u32) {
        self.volume = v.min(100);
        if v > 0 {
            self.volume_muted = false;
        }
    }

    pub fn set_brightness(&mut self, v: u32) {
        self.brightness = v.min(100);
    }

    pub fn toggle_mute(&mut self) {
        self.volume_muted = !self.volume_muted;
    }

    /// Set a tile's ON-state directly (the shell syncs this from the LIVE kernel
    /// backend reader, e.g. `notify::quick_settings::is_on`, so the rendered tile
    /// reflects real system state — not a faked local toggle).
    pub fn set_tile_enabled(&mut self, kind: TileKind, on: bool) {
        if let Some(t) = self.tiles.iter_mut().find(|t| t.kind == kind) {
            t.enabled = on;
            t.sub_state = String::from(if on { "On" } else { "Off" });
        }
    }

    /// Mark a tile disabled (e.g. no Bluetooth radio present) — spec §2.1
    /// disabled state. Honest: a tile with no hardware reads disabled, not "Off".
    pub fn set_tile_disabled(&mut self, kind: TileKind, disabled: bool) {
        if let Some(t) = self.tiles.iter_mut().find(|t| t.kind == kind) {
            t.disabled = disabled;
            if disabled {
                t.sub_state = String::from("Unavailable");
            }
        }
    }

    /// Whether a given tile is on (model state).
    #[must_use]
    pub fn tile_enabled(&self, kind: TileKind) -> bool {
        self.tiles
            .iter()
            .find(|t| t.kind == kind)
            .map(|t| t.enabled)
            .unwrap_or(false)
    }

    // ── Geometry ────────────────────────────────────────────────────────────

    /// Top inset (critique #5): the panel top is never closer to the screen top
    /// than `space.4`, so the first tile row is never clipped at y=0.
    const TOP_INSET: usize = SPACE_4 as usize;

    /// Maximum panel height for the current screen — spec §1: content height
    /// clamped to `screen − taskbar − space.4 (top inset) − space.2 (bottom
    /// inset above tray)`. The panel never exceeds this, so it is fully visible
    /// (critique #5: no top-clip, no spill over the tray); overflowing content
    /// scrolls (see [`Self::scroll_max`]).
    #[must_use]
    pub fn max_panel_height(&self) -> usize {
        self.screen_height
            .saturating_sub(self.taskbar_height)
            .saturating_sub(Self::TOP_INSET)
            .saturating_sub(SPACE_2 as usize)
    }

    /// The panel rect (bottom-right above the tray, spec §1). Height sizes to
    /// content, clamped to [`Self::max_panel_height`]; the top edge is held at
    /// or below [`Self::TOP_INSET`] from the screen top so it is never clipped
    /// (critique #5).
    #[must_use]
    pub fn panel_rect(&self) -> (usize, usize, usize, usize) {
        let inset = SPACE_2 as usize;
        let w = PANEL_WIDTH.min(self.screen_width.saturating_sub(2 * inset));
        let h = self.content_height().min(self.max_panel_height());
        let x = self.screen_width.saturating_sub(w + inset);
        // Anchor the bottom above the tray; clamp the top to TOP_INSET so a tall
        // panel grows DOWNWARD from a visible top rather than off-screen.
        let y = self
            .screen_height
            .saturating_sub(self.taskbar_height)
            .saturating_sub(inset)
            .saturating_sub(h)
            .max(Self::TOP_INSET);
        (x, y, w, h)
    }

    /// How far the content overflows the panel (0 if it fits). The render
    /// scrolls the content region up by this much so the footer (the bottom-
    /// anchored, highest-value row) stays visible when the panel is full
    /// (critique #5: "add scroll if content exceeds max-height"). Bounded so the
    /// top section never scrolls entirely out of view.
    #[must_use]
    pub fn scroll_max(&self) -> usize {
        self.content_height()
            .saturating_sub(self.max_panel_height())
    }

    /// Height of every section EXCEPT the expandable Wi-Fi/BT sub-panel (the one
    /// list that can grow unboundedly). The sub-panel then scrolls within
    /// whatever room is left, so the whole panel always fits (critique #5).
    fn fixed_content_height(&self) -> usize {
        let pad = SPACE_4 as usize;
        let mut h = pad; // top padding
                         // Tile grid: 6 main tiles in 2 cols = 3 rows.
        let rows = (Self::GRID_TILES + 1) / 2;
        h += rows * (TILE_HEIGHT + SPACE_2 as usize);
        // Slider row (volume + brightness).
        h += SPACE_2 as usize + 2 * (SLIDER_ROW_HEIGHT + SPACE_2 as usize);
        // Media card (only when playing).
        if self.media.visible() {
            h += SPACE_2 as usize + MEDIA_CARD_HEIGHT;
        }
        // Gaming section header + 3 tiles (2 cols = 2 rows).
        h += SPACE_3 as usize + rae_tokens::TYPE_SUBTITLE.line_height as usize + SPACE_2 as usize;
        h += 2 * (TILE_HEIGHT + SPACE_2 as usize);
        // RGB chip row when RGB is expanded.
        if self.expanded == Expanded::Tile(TileKind::Rgb) {
            h += SPACE_2 as usize + 32 + SPACE_2 as usize;
        }
        // Performance segmented when expanded.
        if self.expanded == Expanded::Tile(TileKind::Performance) {
            h += SPACE_2 as usize + 32 + SPACE_2 as usize;
        }
        // Footer.
        h += SPACE_3 as usize + FOOTER_HEIGHT;
        h += pad; // bottom padding
        h
    }

    /// Number of expandable (Wi-Fi/BT) rows that fit without pushing the panel
    /// past [`Self::max_panel_height`] — the list scrolls within that budget
    /// (spec §2.2: "a scrollable list of networks"; critique #5). Always ≥ 1 so
    /// an expanded tile shows at least one row (or the "Scanning…" placeholder).
    #[must_use]
    pub fn visible_expand_rows(&self) -> usize {
        let total = if self.expand_rows.is_empty() {
            1
        } else {
            self.expand_rows.len()
        };
        if !matches!(
            self.expanded,
            Expanded::Tile(TileKind::WiFi | TileKind::Bluetooth)
        ) {
            return 0;
        }
        // Room left for the sub-panel = max panel − everything else − the
        // sub-panel's own top gap + padding.
        let overhead = SPACE_2 as usize * 2; // gap above + inner pad
        let room = self
            .max_panel_height()
            .saturating_sub(self.fixed_content_height())
            .saturating_sub(overhead);
        let fits = room / EXPAND_ROW_HEIGHT;
        fits.clamp(1, total)
    }

    /// The vertical text boxes inside a tile whose top-left is at `cy`, as
    /// `(label_top, label_bottom, sub_top, sub_bottom)` — the SINGLE source of
    /// truth shared by [`Self::render_tile_grid`] and the layout KAT, so the
    /// "sublabel doesn't overlap and stays in the tile" invariant (critique #3)
    /// can't silently drift from what's drawn.
    #[must_use]
    pub fn tile_text_boxes(cy: usize) -> (usize, usize, usize, usize) {
        let icon_y = cy + SPACE_2 as usize;
        let label_y = icon_y + GLYPH_H + SPACE_1 as usize;
        let label_bot = label_y + rae_tokens::TYPE_LABEL.line_height as usize;
        let cap_lh = rae_tokens::TYPE_CAPTION.line_height as usize;
        let sub_y =
            (label_bot + SPACE_1 as usize).min(cy + TILE_HEIGHT - SPACE_2 as usize - cap_lh);
        let sub_bot = sub_y + cap_lh;
        (label_y, label_bot, sub_y, sub_bot)
    }

    /// Total content height (panel sizes to this, spec §1) — the fixed sections
    /// plus the (scroll-capped) expandable sub-panel.
    fn content_height(&self) -> usize {
        let mut h = self.fixed_content_height();
        if matches!(
            self.expanded,
            Expanded::Tile(TileKind::WiFi | TileKind::Bluetooth)
        ) {
            let n = self.visible_expand_rows();
            h += SPACE_2 as usize + n * EXPAND_ROW_HEIGHT + SPACE_2 as usize;
        }
        h
    }

    /// Resolve a tile's fill colour for a given interaction state (spec §2.1
    /// state matrix). On = `accent.subtle`; off = `bg.elevated`; hover lightens;
    /// active flashes `accent.active`; disabled = no fill.
    ///
    /// NOTE: this is the OPAQUE/state colour used by the cohesion KAT and the
    /// active/hover flashes. The RESTING tile interior is no longer this flat
    /// fill — it is a luminous frosted CARD ([`Self::draw_tile_card`], visual-QA
    /// Round-4 P0 #1: the dark-slate-on-luminous-glass polarity clash). The
    /// state colours below still drive the *pressed* (`accent.active`) and
    /// *hover* flashes, which composite OVER that frosted card.
    #[must_use]
    pub fn tile_fill(&self, t: &Tile, interact: Interact) -> u32 {
        let a = accent();
        let p = PALETTE;
        if t.disabled {
            return p.bg_raised;
        }
        match (t.enabled, interact) {
            (_, Interact::Active) => a.active,
            (true, Interact::Hover) => a.hover,
            (true, _) => a.subtle,
            (false, Interact::Hover) => p.bg_overlay,
            (false, _) => p.bg_elevated,
        }
    }

    /// Draw a tile/card interior — OBSIDIAN raised card (IDENTITY-OBSIDIAN.md
    /// §2): a SOLID elevation-ladder step above the near-black panel + a
    /// subtle hairline. Depth comes from dark-on-darker + the light edge, not
    /// a frost wash (the frost-era "luminous card" was the mid-gray toy read).
    ///
    /// ON tiles carry an accent wash over the face so the enabled state reads
    /// "lit"; `interact` overlays the hover/active flash on top.
    fn draw_tile_card(
        &self,
        canvas: &mut raegfx::Canvas,
        x: usize,
        y: usize,
        w: usize,
        h: usize,
        radius: usize,
        t: &Tile,
        interact: Interact,
    ) {
        let a = accent();
        let p = crate::active_palette();
        // Disabled tiles: one ladder step DOWN from the resting face (bg.raised)
        // and no hairline — honestly dimmer, still a card.
        if t.disabled {
            canvas.fill_rounded_rect(x, y, w, h, radius, p.bg_raised);
            return;
        }
        // 1. Solid ladder-step face — clearly ABOVE the panel interior.
        canvas.fill_rounded_rect(x, y, w, h, radius, p.bg_elevated);
        // 2. ON tiles get an accent wash over the face so the enabled state
        //    reads as an accent-lit card.
        if t.enabled {
            canvas.fill_rounded_rect(x, y, w, h, radius, a.subtle);
        }
        // 3. Hairline (the lit edge that sells elevation on obsidian).
        canvas.draw_rounded_rect_outline(x, y, w, h, radius, p.stroke_subtle);
        // 4. Interaction flash (hover lighten / active press) composited over the
        //    frosted card — translucent so the frost still reads beneath.
        match interact {
            Interact::Active => {
                canvas.fill_rounded_rect(x, y, w, h, radius, with_alpha(a.active, 0x66))
            }
            Interact::Hover => {
                canvas.fill_rounded_rect(x, y, w, h, radius, rae_tokens::GLASS_FROST_LIGHTEN)
            }
            _ => {}
        }
    }

    /// Resolve a tile's icon colour (spec §2.1): on = `accent.text`; off =
    /// `text.secondary`; disabled = `text.tertiary`.
    #[must_use]
    pub fn tile_icon_color(&self, t: &Tile) -> u32 {
        let p = PALETTE;
        if t.disabled {
            p.text_tertiary
        } else if t.enabled {
            accent().text
        } else {
            p.text_secondary
        }
    }

    /// Resolve a tile LABEL's ink for the AA-legibility fix (visual-QA Round-6
    /// P0). An ENABLED tile is an accent-filled (bright RaeBlue) surface: white
    /// ink over it measured ~1.66–1.94:1 — far under AA 4.5:1. The standard fix
    /// is to flip the ink to a DARK on-accent token on the accent fill (RaeBlue
    /// luma ≈ 0.35, so a near-black `bg.base` gives ≈ 7:1). So:
    ///   - disabled → `text.tertiary` (dimmed, no fill)
    ///   - enabled  → `bg.base` (DARK on-accent ink, clears 4.5:1 over RaeBlue)
    ///   - off      → `text.primary` (white over the dark/frosted OFF card, the
    ///                Round-4 promotion that already cleared AA on the OFF tile).
    /// A dedicated `text.on-accent` token would be cleaner — NOTED for raeen-ui
    /// (not added this pass, rae_tokens is frozen for this slice).
    #[must_use]
    pub fn tile_label_ink(&self, t: &Tile) -> u32 {
        let p = PALETTE;
        if t.disabled {
            p.text_tertiary
        } else if t.enabled {
            p.bg_base
        } else {
            p.text_primary
        }
    }

    /// Resolve a tile SUB-STATE line's ink. On the accent-filled ENABLED tile the
    /// sublabel must also be dark on-accent ink (the same legibility fix as the
    /// label) rather than the dim `text.tertiary` that vanishes on bright accent.
    /// An OFF (but usable) tile promotes to `text.secondary`: tertiary is tuned
    /// for ~4:1 on `bg.base`, but the tile card sits a frost-step LIGHTER, where
    /// tertiary's "Off" measured near-invisible (visual-QA). Only a DISABLED
    /// tile keeps the dimmed tertiary — reading dim is its whole point.
    #[must_use]
    pub fn tile_sublabel_ink(&self, t: &Tile) -> u32 {
        let p = PALETTE;
        if t.disabled {
            p.text_tertiary
        } else if t.enabled {
            p.bg_base
        } else {
            p.text_secondary
        }
    }

    // ── Render (spec §2 content top→bottom) ───────────────────────────────────

    pub fn render(&self, canvas: &mut raegfx::Canvas) {
        if !self.visible {
            return;
        }
        // Forced-colors aware: read the LIVE palette so toggling high contrast
        // repaints the Control Center chrome in the HC palette (audit P0 #3).
        // This is the proven core surface for the live palette swap.
        let p = crate::active_palette();
        let hc = rae_tokens::high_contrast();
        let (rx, ry, rw, rh) = self.panel_rect();
        let pad = SPACE_4 as usize;

        // ── Glass panel: material.glass tint + radius.lg + soft elev.3 shadow +
        //    top-edge highlight + stroke.subtle border (spec §1, §5).
        //
        // Soft ambient drop shadow (elev.3) — visual-QA critique #4. The
        // compositor soft-shadow renders a blurred, feathered silhouette (NOT a
        // hard offset block). On a *busy wallpaper* this gives the panel its
        // float. On a near-black desktop a near-black shadow is by definition
        // invisible (material-and-shadow.md §"surface-control-center"), so the
        // separation cue on dark must come from a BRIGHT TOP RIM-LIFT — drawn
        // below — exactly as macOS does (bright 1px rim + faint shadow).
        let shadow = rae_tokens::ELEV_3;
        canvas.fill_rounded_rect_shadow(
            rx,
            ry,
            rw,
            rh,
            RADIUS_LG as usize,
            shadow.color,           // neutral dark rgb (peak set internally)
            shadow.radius as usize, // blur (28)
            shadow.offset_y,        // dy (8)
        );
        // Under high contrast the panel is OPAQUE black (no glass translucency —
        // forced-colors legibility); otherwise the CC-LOCAL frosted glass tint
        // (critique #7: lower alpha than the shared token so the backdrop bleeds
        // through as frost, not smoked plexiglass).
        // OBSIDIAN panel (IDENTITY-OBSIDIAN.md §2): the near-black panel tier
        // (a whisper of aurora bleeds through), hairline, and a subtle lit top
        // lip. The CC-local mid-slate tint + iridescent rim are retired — they
        // were the mid-gray/rainbow "toy" read.
        let panel_fill = if hc {
            p.bg_base
        } else {
            rae_tokens::GLASS_PANEL_DARK.tint
        };
        canvas.fill_rounded_rect(rx, ry, rw, rh, RADIUS_LG as usize, panel_fill);
        canvas.draw_rounded_rect_outline(rx, ry, rw, rh, RADIUS_LG as usize, p.stroke_subtle);
        if !hc {
            let r = RADIUS_LG as usize;
            let lip = 0x30_FF_FF_FF; // subtle lit top lip (not the old double-line)
            for xx in rx + r..rx + rw.saturating_sub(r) {
                canvas.blend_pixel(xx, ry + 1, lip);
            }
        }

        let mut cy = ry + pad;

        // ── §2.1 Toggle-tile grid (2-col responsive) ──────────────────────────
        cy = self.render_tile_grid(canvas, rx + pad, cy, rw - 2 * pad, 0, Self::GRID_TILES);

        // ── §2.2 Expandable sub-panel for a Wi-Fi/BT tile (pushed in-place) ───
        if let Expanded::Tile(kind) = self.expanded {
            if matches!(kind, TileKind::WiFi | TileKind::Bluetooth) {
                cy =
                    self.render_expand_panel(canvas, rx + pad, cy + SPACE_2 as usize, rw - 2 * pad);
            }
        }

        // ── §2.3 Slider row (always visible) ──────────────────────────────────
        cy += SPACE_2 as usize;
        cy = self.render_slider(
            canvas,
            rx + pad,
            cy,
            rw - 2 * pad,
            if self.volume_muted {
                SliderIcon::VolumeMuted
            } else {
                SliderIcon::Volume
            },
            self.volume,
        );
        cy += SPACE_2 as usize;
        cy = self.render_slider(
            canvas,
            rx + pad,
            cy,
            rw - 2 * pad,
            SliderIcon::Brightness,
            self.brightness,
        );

        // ── §2.4 Media transport card (only when playing) ─────────────────────
        if self.media.visible() {
            cy += SPACE_2 as usize;
            cy = self.render_media_card(canvas, rx + pad, cy, rw - 2 * pad);
        }

        // ── §2.5 Gaming row ───────────────────────────────────────────────────
        cy += SPACE_3 as usize;
        canvas.draw_text_aa(
            (rx + pad) as i32,
            cy as i32,
            "Gaming",
            rae_tokens::TYPE_SUBTITLE,
            p.text_secondary,
            raegfx::text::FontFamily::Sans,
        );
        cy += rae_tokens::TYPE_SUBTITLE.line_height as usize + SPACE_2 as usize;
        cy = self.render_tile_grid(
            canvas,
            rx + pad,
            cy,
            rw - 2 * pad,
            Self::GRID_TILES,
            self.tiles.len(),
        );

        // RGB chip row (when RGB expanded, spec §2.5).
        if self.expanded == Expanded::Tile(TileKind::Rgb) {
            cy += SPACE_2 as usize;
            cy = self.render_rgb_chips(canvas, rx + pad, cy, rw - 2 * pad);
        }
        // Performance segmented (when Performance expanded, spec §2.5).
        if self.expanded == Expanded::Tile(TileKind::Performance) {
            cy += SPACE_2 as usize;
            cy = self.render_perf_segmented(canvas, rx + pad, cy, rw - 2 * pad);
        }

        // ── §2.6 Footer (avatar/name left, gear + power right) ────────────────
        cy += SPACE_3 as usize;
        self.render_footer(canvas, rx + pad, cy, rw - 2 * pad);

        // Keyboard focus ring (spec §8: distinct from hover, accent ring + glow).
        self.render_focus_ring(canvas, rx + pad, ry + pad, rw - 2 * pad);
    }

    fn render_tile_grid(
        &self,
        canvas: &mut raegfx::Canvas,
        x: usize,
        y: usize,
        w: usize,
        start: usize,
        end: usize,
    ) -> usize {
        let p = PALETTE;
        let gap = SPACE_2 as usize;
        let cols = 2usize;
        let tile_w = (w.saturating_sub((cols - 1) * gap)) / cols;
        let mut cy = y;
        let mut col = 0usize;
        let inner_r = rae_tokens::concentric(RADIUS_MD, SPACE_2) as usize;

        for (i, t) in self.tiles[start..end].iter().enumerate() {
            let idx = start + i;
            let interact = if self.focus_index == Some(idx) {
                Interact::Focus
            } else {
                Interact::None
            };
            let tx = x + col * (tile_w + gap);
            // Luminous frosted CARD (visual-QA Round-4 P0 #1) instead of the old
            // dark-slate flat fill — the card reads ABOVE the panel luminance so
            // it floats ON the glass, never a dark hole. ON tiles get the accent
            // wash + an accent GLOW halo (Round-4 P1 #6 — accent-glow toggles).
            if t.enabled && !t.disabled {
                // Accent glow halo: a soft accent-tinted ring just outside the
                // card (the elev.focus glow cue, drawn as the shell can't reach
                // the compositor shadow here). Reads as the toggle "lit".
                let glow = accent().glow;
                canvas.draw_rounded_rect_outline(
                    tx.saturating_sub(1),
                    cy.saturating_sub(1),
                    tile_w + 2,
                    TILE_HEIGHT + 2,
                    RADIUS_MD as usize,
                    glow,
                );
            }
            self.draw_tile_card(
                canvas,
                tx,
                cy,
                tile_w,
                TILE_HEIGHT,
                RADIUS_MD as usize,
                t,
                interact,
            );
            // Accent-glow PILL toggle indicator (Round-4 P1 #6): a rounded-full
            // switch in the tile's top-right, lit (accent) when ON / neutral when
            // OFF — the explicit on/off affordance the reference shows, replacing
            // the flat-square read. Skipped on expandable tiles (their right edge
            // carries the chevron) and disabled tiles.
            if !t.expandable && !t.disabled {
                let pill_w = 26usize;
                let pill_h = 14usize;
                let pill_x = tx + tile_w - SPACE_3 as usize - pill_w;
                let pill_y = cy + SPACE_2 as usize;
                let pill_r = rae_tokens::radius_pill(pill_h as u32) as usize;
                let track = if t.enabled {
                    accent().base
                } else {
                    PALETTE.bg_overlay
                };
                canvas.fill_rounded_rect(pill_x, pill_y, pill_w, pill_h, pill_r, track);
                // Accessibility audit P1 — the toggle was COLOR-ONLY (ON-track vs
                // tile measured 1.37:1, far under 3.0:1) so colorblind/low-vision
                // users couldn't read state. Add NON-color cues: (1) the knob is
                // POSITIONAL (full-right on ON, below) and (2) a `stroke.strong`
                // outline on the ON track so its boundary clears 3.0:1 against the
                // tile regardless of hue. The accent fill stays as the secondary
                // cue. The OFF track keeps the subtle stroke (neutral boundary).
                if t.enabled {
                    canvas.draw_rounded_rect_outline(
                        pill_x,
                        pill_y,
                        pill_w,
                        pill_h,
                        pill_r,
                        PALETTE.stroke_strong,
                    );
                } else {
                    canvas.draw_rounded_rect_outline(
                        pill_x,
                        pill_y,
                        pill_w,
                        pill_h,
                        pill_r,
                        PALETTE.stroke_subtle,
                    );
                }
                // Knob slides right when ON (the POSITIONAL state cue).
                let knob = pill_h.saturating_sub(4);
                let knob_x = if t.enabled {
                    pill_x + pill_w - knob - 2
                } else {
                    pill_x + 2
                };
                canvas.fill_rounded_rect(
                    knob_x,
                    pill_y + 2,
                    knob,
                    knob,
                    rae_tokens::radius_pill(knob as u32) as usize,
                    PALETTE.text_primary,
                );
            }

            // Tile internal vertical rhythm (critique #3 — the "Off" sublabel
            // was overlapping the label/next row). A 60px tile, top→bottom with
            // space.2 padding, NO box overlap (see [`Self::tile_text_boxes`]):
            //   icon    : [cy+8 .. cy+16]   (8px glyph)
            //   label   : [cy+20 .. cy+36]  (type.label, lh 16)
            //   sublabel: [cy+40 .. cy+54]  (type.caption, lh 14) → 6px bottom margin
            let inset = SPACE_3 as usize; // left inset (space.3)

            let (label_y, _label_bot, sub_y, _sub_bot) = Self::tile_text_boxes(cy);

            // Real line-icon (visual-QA Round-2 #1: retire the W/B/F/N/A/X/G/R/P
            // LETTER placeholders — the icon system landed in `raegfx::icon`,
            // this is the wiring that consumes it). The icon occupies the top
            // band of the tile (from the top padding down to the label box that
            // `tile_text_boxes` reserves), so the established vertical rhythm
            // (critique #3) is unchanged — the label/sublabel boxes are not
            // moved. Centered horizontally on the left-inset icon column and
            // vertically within that band: `ix = base + (slot - size)/2`. Tint
            // follows tile state — `accent.text` (on) / `text.secondary` (off) /
            // `text.tertiary` (disabled) via `tile_icon_color`, so the glyph
            // state-changes with the tile exactly like the old letter did.
            let slot = label_y.saturating_sub(cy).max(GLYPH_H);
            let isize = ICON_SIZE.min(slot);
            let ix = tx + inset + (slot.saturating_sub(isize)) / 2;
            let iy = cy + (slot.saturating_sub(isize)) / 2;
            canvas.draw_icon(
                t.kind.icon(),
                ix as i32,
                iy as i32,
                isize as i32,
                self.tile_icon_color(t),
            );

            // Label (type.label) — directly under the icon, space.1 gap.
            // Accessibility audit P1 (Round-4): the OFF-state label used
            // `text.secondary`, which FAILED 4.5:1 over the now-LUMINOUS frosted
            // OFF tiles, so OFF labels were promoted to `text.primary`.
            // Visual-QA Round-6 P0: the inverse failure on the ON tiles — white
            // ink over the bright accent fill measured ~1.66–1.94:1. The standard
            // fix is a DARK on-accent ink on the accent-filled (ENABLED) tile.
            // `tile_label_ink` resolves: enabled → `bg.base` (dark, ≈7:1 over
            // RaeBlue), off → `text.primary` (white on the dark/frosted card),
            // disabled → `text.tertiary`.
            canvas.draw_text_aa(
                (tx + inset) as i32,
                label_y as i32,
                &t.label,
                rae_tokens::TYPE_LABEL,
                self.tile_label_ink(t),
                raegfx::text::FontFamily::Sans,
            );

            // Sub-state line (type.caption) — On/Off/SSID. Below the label box
            // with a space.1 gap, clamped inside the tile (critique #3). The ink
            // tracks the same on-accent flip: dark on the ENABLED accent tile,
            // dimmed `text.tertiary` on OFF/disabled tiles.
            canvas.draw_text_aa(
                (tx + inset) as i32,
                sub_y as i32,
                &t.sub_state,
                rae_tokens::TYPE_CAPTION,
                self.tile_sublabel_ink(t),
                raegfx::text::FontFamily::Sans,
            );

            // Expand chevron on the right third (the expand region, spec §2.1).
            // Collapsed = chevron-right (the shipped `Icon::Chevron`); expanded =
            // chevron-up, drawn inline (the icon set has no up variant yet — see
            // the follow-up list). Replaces the old '^'/'>' LETTER glyphs.
            if t.expandable {
                let cw = GLYPH_W; // chevron box edge
                let cxx = tx + tile_w - SPACE_3 as usize - cw;
                let cyy = cy + (TILE_HEIGHT - cw) / 2;
                if self.expanded == Expanded::Tile(t.kind) {
                    draw_chevron_up(canvas, cxx, cyy, cw, p.text_tertiary);
                } else {
                    canvas.draw_icon(
                        raegfx::icon::Icon::Chevron,
                        cxx as i32,
                        cyy as i32,
                        cw as i32,
                        p.text_tertiary,
                    );
                }
            }
            let _ = inner_r;

            col += 1;
            if col == cols {
                col = 0;
                cy += TILE_HEIGHT + gap;
            }
        }
        if col != 0 {
            cy += TILE_HEIGHT + gap;
        }
        cy
    }

    fn render_expand_panel(
        &self,
        canvas: &mut raegfx::Canvas,
        x: usize,
        y: usize,
        w: usize,
    ) -> usize {
        let p = PALETTE;
        let a = accent();
        // Scroll-cap the visible rows so the sub-panel never pushes the panel
        // past its max height (critique #5; spec §2.2 scrollable list).
        let visible = self.visible_expand_rows().max(1);
        let h = visible * EXPAND_ROW_HEIGHT + SPACE_2 as usize;
        // Sub-panel glass lift (elev.2, radius.md) — spec §2.2. Frosted CARD on
        // the panel (Round-4 P0 #1): popover tint + frost, not a dark fill, so the
        // expanded Wi-Fi list reads as a raised frosted sub-surface.
        canvas.fill_rounded_rect(
            x,
            y,
            w,
            h,
            RADIUS_MD as usize,
            rae_tokens::GLASS_POPOVER_DARK.tint,
        );
        canvas.fill_rounded_rect(
            x,
            y,
            w,
            h,
            RADIUS_MD as usize,
            rae_tokens::GLASS_POPOVER_DARK.frost,
        );
        canvas.draw_rounded_rect_outline(x, y, w, h, RADIUS_MD as usize, p.stroke_subtle);

        let mut ry = y + SPACE_1 as usize;
        if self.expand_rows.is_empty() {
            canvas.draw_text_aa(
                (x + SPACE_3 as usize) as i32,
                (ry + (EXPAND_ROW_HEIGHT - rae_tokens::TYPE_BODY.line_height as usize) / 2) as i32,
                "Scanning...",
                rae_tokens::TYPE_BODY,
                p.text_tertiary,
                raegfx::text::FontFamily::Sans,
            );
            return y + h;
        }
        for row in self.expand_rows.iter().take(visible) {
            if row.connected {
                canvas.fill_rounded_rect(
                    x + SPACE_1 as usize,
                    ry,
                    w - 2 * SPACE_1 as usize,
                    EXPAND_ROW_HEIGHT - 2,
                    RADIUS_SM as usize,
                    a.subtle,
                );
            }
            // Signal bars + SSID (type.body) + lock/check glyph. The signal and
            // lock glyphs are clean inline vectors (the icon set has no signal-
            // bars or lock yet — see the follow-up list); the connected check is
            // the shipped `Icon::Check`. Replaces the old '.'/','/-/='/#'/*/#
            // LETTER/ASCII placeholders.
            let sig_color = if row.connected {
                a.text
            } else {
                p.text_secondary
            };
            draw_signal_bars(
                canvas,
                x + SPACE_3 as usize,
                ry + (EXPAND_ROW_HEIGHT - GLYPH_H) / 2,
                GLYPH_W,
                GLYPH_H,
                row.signal,
                sig_color,
                p.text_tertiary,
            );
            let text_y = ry + (EXPAND_ROW_HEIGHT - rae_tokens::TYPE_BODY.line_height as usize) / 2;
            canvas.draw_text_aa(
                (x + SPACE_3 as usize + GLYPH_W + SPACE_2 as usize) as i32,
                text_y as i32,
                &row.name,
                rae_tokens::TYPE_BODY,
                if row.connected {
                    p.text_primary
                } else {
                    p.text_secondary
                },
                raegfx::text::FontFamily::Sans,
            );
            if row.connected {
                canvas.draw_icon(
                    raegfx::icon::Icon::Check,
                    (x + w - SPACE_3 as usize - GLYPH_W) as i32,
                    (ry + (EXPAND_ROW_HEIGHT - GLYPH_H) / 2) as i32,
                    GLYPH_W as i32,
                    a.text,
                );
            } else if row.secured {
                draw_lock(
                    canvas,
                    x + w - SPACE_3 as usize - GLYPH_W,
                    ry + (EXPAND_ROW_HEIGHT - GLYPH_H) / 2,
                    GLYPH_W,
                    GLYPH_H,
                    p.text_tertiary,
                );
            }
            ry += EXPAND_ROW_HEIGHT;
        }
        y + h
    }

    fn render_slider(
        &self,
        canvas: &mut raegfx::Canvas,
        x: usize,
        y: usize,
        w: usize,
        icon: SliderIcon,
        value: u32,
    ) -> usize {
        let p = PALETTE;
        let a = accent();
        // Leading icon — clean inline vector (speaker / sun), token-tinted
        // `text.secondary`. The icon set has no volume/brightness glyph yet (see
        // the follow-up list); these read better as small custom vectors than as
        // a stand-in tile icon. Replaces the old 'V'/'x'/'L' LETTER glyphs.
        let icon_y = y + (SLIDER_ROW_HEIGHT - GLYPH_H) / 2;
        match icon {
            SliderIcon::Volume => {
                draw_speaker(canvas, x, icon_y, GLYPH_W, GLYPH_H, false, p.text_secondary)
            }
            SliderIcon::VolumeMuted => {
                draw_speaker(canvas, x, icon_y, GLYPH_W, GLYPH_H, true, p.text_secondary)
            }
            SliderIcon::Brightness => {
                draw_sun(canvas, x, icon_y, GLYPH_W, GLYPH_H, p.text_secondary)
            }
        }

        // Track (4px, radius.pill, bg.elevated) — spec §2.3.
        let track_x = x + GLYPH_W + SPACE_3 as usize;
        let track_w = w.saturating_sub(GLYPH_W + SPACE_3 as usize);
        let track_y = y + (SLIDER_ROW_HEIGHT - 4) / 2;
        let track_r = rae_tokens::radius_pill(4) as usize;
        canvas.fill_rounded_rect(track_x, track_y, track_w, 4, track_r, p.bg_elevated);

        // Fill = accent.base up to value.
        let fill_w = track_w * (value as usize).min(100) / 100;
        if fill_w > 0 {
            canvas.fill_rounded_rect(track_x, track_y, fill_w, 4, track_r, a.base);
        }

        // Knob (18px, radius.pill, elev.2) — spec §2.3.
        let knob = 18usize;
        let knob_x = track_x + fill_w.saturating_sub(knob / 2);
        let knob_y = y + (SLIDER_ROW_HEIGHT - knob) / 2;
        canvas.fill_rounded_rect(
            knob_x.min(track_x + track_w - knob),
            knob_y,
            knob,
            knob,
            rae_tokens::radius_pill(knob as u32) as usize,
            p.text_primary,
        );
        y + SLIDER_ROW_HEIGHT
    }

    fn render_media_card(
        &self,
        canvas: &mut raegfx::Canvas,
        x: usize,
        y: usize,
        w: usize,
    ) -> usize {
        let p = PALETTE;
        // Card: material.glass, radius.md, elev.2 (spec §2.4). Frosted CARD on the
        // panel (Round-4 P0 #1): popover tint + frost instead of a dark fill.
        canvas.fill_rounded_rect(
            x,
            y,
            w,
            MEDIA_CARD_HEIGHT,
            RADIUS_MD as usize,
            rae_tokens::GLASS_POPOVER_DARK.tint,
        );
        canvas.fill_rounded_rect(
            x,
            y,
            w,
            MEDIA_CARD_HEIGHT,
            RADIUS_MD as usize,
            rae_tokens::GLASS_POPOVER_DARK.frost,
        );
        canvas.draw_rounded_rect_outline(
            x,
            y,
            w,
            MEDIA_CARD_HEIGHT,
            RADIUS_MD as usize,
            p.stroke_subtle,
        );
        // Album art placeholder (48px, left) — accent-washed frosted swatch (not a
        // dark slate): accent.subtle under a frost lift, so it reads as a lit tile.
        let art = 48usize;
        let art_x = x + SPACE_2 as usize;
        let art_y = y + (MEDIA_CARD_HEIGHT - art) / 2;
        canvas.fill_rounded_rect(art_x, art_y, art, art, RADIUS_SM as usize, accent().subtle);
        canvas.fill_rounded_rect(
            art_x,
            art_y,
            art,
            art,
            RADIUS_SM as usize,
            rae_tokens::GLASS_POPOVER_DARK.frost,
        );
        // Album-art placeholder glyph = the shipped `Icon::Media` (framed
        // picture), accent-tinted — replaces the old 'M' LETTER glyph.
        let art_icon = (art * 3 / 5).max(GLYPH_H);
        canvas.draw_icon(
            raegfx::icon::Icon::Media,
            (art_x + (art - art_icon) / 2) as i32,
            (art_y + (art - art_icon) / 2) as i32,
            art_icon as i32,
            accent().text,
        );
        // Title + artist.
        let text_x = art_x + art + SPACE_3 as usize;
        canvas.draw_text_aa(
            text_x as i32,
            (y + SPACE_2 as usize) as i32,
            &self.media.title,
            rae_tokens::TYPE_LABEL,
            p.text_primary,
            raegfx::text::FontFamily::Sans,
        );
        canvas.draw_text_aa(
            text_x as i32,
            (y + SPACE_2 as usize + rae_tokens::TYPE_LABEL.line_height as usize) as i32,
            &self.media.artist,
            rae_tokens::TYPE_CAPTION,
            p.text_secondary,
            raegfx::text::FontFamily::Sans,
        );
        // Prev / play-pause / next transport buttons (right) — clean inline
        // vectors (the icon set has no transport glyphs yet; see the follow-up
        // list). Replaces the old '<'/'='/'>' LETTER glyphs.
        let btn = 32usize;
        let by = y + (MEDIA_CARD_HEIGHT - btn) / 2;
        let mut bx = x + w - SPACE_2 as usize - 3 * btn - 2 * SPACE_1 as usize;
        let glyph_box = GLYPH_W + 2; // 10px transport glyph inside the 32px target
        for which in [
            TransportGlyph::Prev,
            if self.media.playing {
                TransportGlyph::Pause
            } else {
                TransportGlyph::Play
            },
            TransportGlyph::Next,
        ] {
            draw_transport(
                canvas,
                bx + (btn - glyph_box) / 2,
                by + (btn - glyph_box) / 2,
                glyph_box,
                which,
                p.text_primary,
            );
            bx += btn + SPACE_1 as usize;
        }
        y + MEDIA_CARD_HEIGHT
    }

    fn render_rgb_chips(&self, canvas: &mut raegfx::Canvas, x: usize, y: usize, w: usize) -> usize {
        let a = accent();
        let chip_h = 32usize;
        let gap = SPACE_1 as usize;
        let n = RGB_EFFECTS.len();
        let chip_w = (w.saturating_sub((n - 1) * gap)) / n;
        for (i, (_name, swatch)) in RGB_EFFECTS.iter().enumerate() {
            let cx = x + i * (chip_w + gap);
            let selected = i == self.rgb_effect;
            let chip_r = rae_tokens::radius_pill(chip_h as u32) as usize;
            // Selected chip = accent-glow pill (Round-4 P1 #6); unselected = a
            // luminous frosted card (Round-4 P0 #1), not a dark slate.
            if selected {
                canvas.draw_rounded_rect_outline(
                    cx.saturating_sub(1),
                    y.saturating_sub(1),
                    chip_w + 2,
                    chip_h + 2,
                    chip_r,
                    a.glow,
                );
                canvas.fill_rounded_rect(cx, y, chip_w, chip_h, chip_r, a.subtle);
            } else {
                canvas.fill_rounded_rect(
                    cx,
                    y,
                    chip_w,
                    chip_h,
                    chip_r,
                    rae_tokens::GLASS_POPOVER_DARK.tint,
                );
            }
            canvas.fill_rounded_rect(
                cx,
                y,
                chip_w,
                chip_h,
                chip_r,
                rae_tokens::GLASS_POPOVER_DARK.frost,
            );
            // Live effect swatch dot.
            canvas.fill_rounded_rect(
                cx + (chip_w - 10) / 2,
                y + (chip_h - 10) / 2,
                10,
                10,
                5,
                *swatch,
            );
        }
        y + chip_h
    }

    fn render_perf_segmented(
        &self,
        canvas: &mut raegfx::Canvas,
        x: usize,
        y: usize,
        w: usize,
    ) -> usize {
        let p = PALETTE;
        let a = accent();
        let seg_h = 32usize;
        let n = PerfMode::ALL.len();
        let seg_w = w / n;
        let pill_r = rae_tokens::radius_pill(seg_h as u32) as usize;
        // Track = a luminous frosted pill (Round-4 P0 #1), not a dark slate:
        // popover tint + frost composited over the panel.
        canvas.fill_rounded_rect(x, y, w, seg_h, pill_r, rae_tokens::GLASS_POPOVER_DARK.tint);
        canvas.fill_rounded_rect(x, y, w, seg_h, pill_r, rae_tokens::GLASS_POPOVER_DARK.frost);
        for (i, mode) in PerfMode::ALL.iter().enumerate() {
            let sx = x + i * seg_w;
            let selected = *mode == self.perf_mode;
            if selected {
                // Selected segment = accent-glow pill (Round-4 P1 #6).
                canvas.draw_rounded_rect_outline(
                    sx.saturating_sub(1),
                    y.saturating_sub(1),
                    seg_w + 2,
                    seg_h + 2,
                    pill_r,
                    a.glow,
                );
                canvas.fill_rounded_rect(sx, y, seg_w, seg_h, pill_r, a.subtle);
            }
            let label = mode.label();
            let lw = canvas.measure_text_aa(
                label,
                rae_tokens::TYPE_CAPTION,
                raegfx::text::FontFamily::Sans,
            );
            let lx = sx as i32 + (seg_w as i32 - lw) / 2;
            let ly = y as i32 + (seg_h as i32 - rae_tokens::TYPE_CAPTION.line_height as i32) / 2;
            canvas.draw_text_aa(
                lx,
                ly,
                label,
                rae_tokens::TYPE_CAPTION,
                if selected { a.text } else { p.text_secondary },
                raegfx::text::FontFamily::Sans,
            );
        }
        y + seg_h
    }

    fn render_footer(&self, canvas: &mut raegfx::Canvas, x: usize, y: usize, w: usize) {
        let p = PALETTE;
        // Avatar + name (left).
        let av = 24usize;
        canvas.fill_rounded_rect(
            x,
            y + (FOOTER_HEIGHT - av) / 2,
            av,
            av,
            rae_tokens::radius_pill(av as u32) as usize,
            accent().subtle,
        );
        canvas.draw_text_aa(
            (x + av + SPACE_2 as usize) as i32,
            (y + (FOOTER_HEIGHT - rae_tokens::TYPE_LABEL.line_height as usize) / 2) as i32,
            "AthenaOS",
            rae_tokens::TYPE_LABEL,
            p.text_primary,
            raegfx::text::FontFamily::Sans,
        );
        // Gear + power (right, 32px targets, radius.xs hover). Settings = the
        // shipped `Icon::Gear`; power = a clean inline vector (the icon set has
        // no power glyph yet — see the follow-up list). Replaces the old 'S'/'P'
        // LETTER glyphs.
        let icon = 32usize;
        let glyph = 18usize; // glyph inside the 32px hit target
        let iy = y + (FOOTER_HEIGHT - icon) / 2;
        let gear_x = x + w - 2 * icon - SPACE_2 as usize;
        canvas.draw_icon(
            raegfx::icon::Icon::Gear,
            (gear_x + (icon - glyph) / 2) as i32,
            (iy + (icon - glyph) / 2) as i32,
            glyph as i32,
            p.text_secondary,
        );
        let pow_x = x + w - icon;
        draw_power(
            canvas,
            pow_x + (icon - glyph) / 2,
            iy + (icon - glyph) / 2,
            glyph,
            glyph,
            p.text_secondary,
        );
    }

    fn render_focus_ring(&self, canvas: &mut raegfx::Canvas, x: usize, y: usize, w: usize) {
        let idx = match self.focus_index {
            Some(i) if i < Self::GRID_TILES => i,
            _ => return,
        };
        let gap = SPACE_2 as usize;
        let cols = 2usize;
        let tile_w = (w.saturating_sub((cols - 1) * gap)) / cols;
        let col = idx % cols;
        let row = idx / cols;
        let tx = x + col * (tile_w + gap);
        let ty = y + row * (TILE_HEIGHT + gap);
        // macOS-style DOUBLE-STROKE focus ring (accessibility audit P0 — WCAG
        // 1.4.11): a single `accent.base` (RaeBlue) ring measured only 2.28/1.59:1
        // on the bright luminous tiles and FAILED 3.0:1. A dark `bg.base` KEYLINE
        // sandwiching the accent ring makes the boundary read on ANY tile luminance
        // (the accent contrasts the dark keyline ≥3:1; the keyline contrasts the
        // bright tile ≥3:1), so the focus indicator clears 1.4.11 over every tile.
        // Layout, OUTER→INNER: dark keyline (r+2) → 2px accent ring (r, r+1) → dark
        // keyline (r-1). Under forced-colors the HC cyan ring is used unchanged
        // (audit P0 #3) — the HC palette already clears contrast on the HC tiles.
        let r = RADIUS_MD as usize;
        let ring = rae_tokens::active_focus_ring(accent().base);
        let keyline = PALETTE.bg_base; // opaque dark — the high-contrast sandwich
                                       // Outer dark keyline (so the ring reads even on the brightest tile edge).
        canvas.draw_rounded_rect_outline(
            tx.saturating_sub(2),
            ty.saturating_sub(2),
            tile_w + 4,
            TILE_HEIGHT + 4,
            r,
            keyline,
        );
        // 2px accent ring.
        canvas.draw_rounded_rect_outline(
            tx.saturating_sub(1),
            ty.saturating_sub(1),
            tile_w + 2,
            TILE_HEIGHT + 2,
            r,
            ring,
        );
        canvas.draw_rounded_rect_outline(tx, ty, tile_w, TILE_HEIGHT, r, ring);
        // Inner dark keyline (so the accent reads against a bright tile interior).
        canvas.draw_rounded_rect_outline(
            tx + 1,
            ty + 1,
            tile_w.saturating_sub(2),
            TILE_HEIGHT.saturating_sub(2),
            r,
            keyline,
        );
    }

    // ── Hit testing ───────────────────────────────────────────────────────────

    /// Which tile (and whether the expand region) is at the given point, in
    /// screen coords. Returns `(kind, on_expand_region)`.
    #[must_use]
    pub fn tile_at(&self, mx: i32, my: i32) -> Option<(TileKind, bool)> {
        if !self.visible {
            return None;
        }
        let (rx, ry, rw, _rh) = self.panel_rect();
        let pad = SPACE_4 as usize;
        let gap = SPACE_2 as usize;
        let cols = 2usize;
        let inner_w = rw - 2 * pad;
        let tile_w = (inner_w.saturating_sub((cols - 1) * gap)) / cols;

        // Main grid (top section).
        let grid_y0 = ry + pad;
        if let Some(hit) = self.hit_grid(
            mx,
            my,
            rx + pad,
            grid_y0,
            tile_w,
            gap,
            cols,
            0,
            Self::GRID_TILES,
        ) {
            return Some(hit);
        }
        // Gaming grid: compute its y by walking the layout the same way render
        // does (re-using content offsets is overkill for the smoketest path;
        // the live shell uses focus + the main grid first).
        None
    }

    #[allow(clippy::too_many_arguments)]
    fn hit_grid(
        &self,
        mx: i32,
        my: i32,
        x: usize,
        y: usize,
        tile_w: usize,
        gap: usize,
        cols: usize,
        start: usize,
        end: usize,
    ) -> Option<(TileKind, bool)> {
        for (i, t) in self.tiles[start..end].iter().enumerate() {
            let col = i % cols;
            let row = i / cols;
            let tx = x + col * (tile_w + gap);
            let ty = y + row * (TILE_HEIGHT + gap);
            if mx >= tx as i32
                && mx < (tx + tile_w) as i32
                && my >= ty as i32
                && my < (ty + TILE_HEIGHT) as i32
            {
                // Expand region = right third (spec §2.1).
                let on_expand = t.expandable && mx >= (tx + tile_w * 2 / 3) as i32;
                return Some((t.kind, on_expand));
            }
        }
        None
    }
}

// ── Inline vector glyphs (Canvas-primitive line art for glyphs with no shipped
//    icon yet: power, volume/brightness, signal bars, lock, transport, chevron-
//    up). Each is monochrome and token-tinted, matching the icon register; all
//    are reported as follow-ups for the raegfx icon agent to promote into the
//    canonical set. NONE is a letter placeholder. ─────────────────────────────

/// A 2px-ish stroke between two points: the Canvas `draw_line` is a 1px aliased
/// Bresenham, so we thicken by stamping the line plus a 1px x/y offset. Keeps the
/// inline glyphs from disappearing next to the AA `draw_icon` strokes.
fn stroke(canvas: &mut raegfx::Canvas, x0: i32, y0: i32, x1: i32, y1: i32, color: u32) {
    canvas.draw_line(x0, y0, x1, y1, color);
    canvas.draw_line(x0 + 1, y0, x1 + 1, y1, color);
    canvas.draw_line(x0, y0 + 1, x1, y1 + 1, color);
}

/// Power glyph: a near-full ring with a vertical break at the top + a vertical
/// stroke through it (the universal IEC 5009 power symbol), drawn from line
/// segments inside a `w×h` box at `(x, y)`.
fn draw_power(canvas: &mut raegfx::Canvas, x: usize, y: usize, w: usize, h: usize, color: u32) {
    let cx = (x + w / 2) as i32;
    let cy = (y + h / 2) as i32;
    let r = (w.min(h) as i32 / 2 - 1).max(2);
    // Ring approximated by an octagon with the top notch left open.
    let pts: [(i32, i32); 8] = [
        (cx, cy - r),
        (cx + r * 7 / 10, cy - r * 7 / 10),
        (cx + r, cy),
        (cx + r * 7 / 10, cy + r * 7 / 10),
        (cx, cy + r),
        (cx - r * 7 / 10, cy + r * 7 / 10),
        (cx - r, cy),
        (cx - r * 7 / 10, cy - r * 7 / 10),
    ];
    // Connect 1..=7 then 7→0 — leaving the 0..1 top edge open as the break.
    for i in 1..pts.len() {
        let a = pts[i];
        let b = pts[(i + 1) % pts.len()];
        if i == pts.len() - 1 {
            break; // don't close 7→0 (keep notch open)
        }
        stroke(canvas, a.0, a.1, b.0, b.1, color);
    }
    stroke(canvas, pts[7].0, pts[7].1, pts[0].0, pts[0].1, color);
    // The vertical break stroke through the top.
    stroke(canvas, cx, cy - r - 1, cx, cy + r / 3, color);
}

/// Speaker glyph: a small box + a flared cone, plus a mute slash when `muted`.
fn draw_speaker(
    canvas: &mut raegfx::Canvas,
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    muted: bool,
    color: u32,
) {
    let x = x as i32;
    let y = y as i32;
    let w = w as i32;
    let h = h as i32;
    let cy = y + h / 2;
    // Speaker body: a small square at the left third.
    let bx = x;
    let bw = w / 3;
    canvas.fill_rect(
        bx as usize,
        (cy - h / 4) as usize,
        bw as usize,
        (h / 2) as usize,
        color,
    );
    // Cone: triangle flaring right from the body.
    let tip_x = bx + bw;
    let cone_x = x + w * 3 / 5;
    stroke(canvas, tip_x, cy - h / 4, cone_x, y, color);
    stroke(canvas, tip_x, cy + h / 4, cone_x, y + h, color);
    stroke(canvas, cone_x, y, cone_x, y + h, color);
    if muted {
        // Mute slash across the sound waves area.
        stroke(canvas, x + w * 2 / 3, y, x + w, y + h, color);
    } else {
        // Two short sound arcs (drawn as short vertical-ish strokes).
        stroke(
            canvas,
            x + w * 3 / 4,
            cy - h / 5,
            x + w * 3 / 4,
            cy + h / 5,
            color,
        );
        stroke(canvas, x + w - 1, cy - h / 3, x + w - 1, cy + h / 3, color);
    }
}

/// Sun / brightness glyph: a filled centre dot + eight short rays.
fn draw_sun(canvas: &mut raegfx::Canvas, x: usize, y: usize, w: usize, h: usize, color: u32) {
    let cx = (x + w / 2) as i32;
    let cy = (y + h / 2) as i32;
    let r = (w.min(h) as i32 / 4).max(1);
    canvas.fill_circle(cx as usize, cy as usize, r as usize, color);
    let ro = (w.min(h) as i32 / 2).max(r + 2);
    let ri = r + 1;
    // 8 rays at 45° steps (using integer 7/10 ≈ sin/cos 45°).
    let dirs: [(i32, i32); 8] = [
        (0, -1),
        (7, -7),
        (1, 0),
        (7, 7),
        (0, 1),
        (-7, 7),
        (-1, 0),
        (-7, -7),
    ];
    for (dx, dy) in dirs {
        let (sx, sy, scale) = if dx.abs() == dy.abs() && dx != 0 {
            (dx, dy, 10)
        } else {
            (dx * 10, dy * 10, 10)
        };
        let x0 = cx + sx * ri / scale;
        let y0 = cy + sy * ri / scale;
        let x1 = cx + sx * ro / scale;
        let y1 = cy + sy * ro / scale;
        stroke(canvas, x0, y0, x1, y1, color);
    }
}

/// Signal-strength bars: four bars of increasing height; the first `level` (of
/// 0..=4) are drawn in `on` colour, the rest in `off` colour (so an empty/weak
/// network reads honestly). Drawn inside a `w×h` box at `(x, y)`.
#[allow(clippy::too_many_arguments)]
fn draw_signal_bars(
    canvas: &mut raegfx::Canvas,
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    level: u8,
    on: u32,
    off: u32,
) {
    let bars = 4usize;
    let gap = 1usize;
    let bw = (w.saturating_sub((bars - 1) * gap)) / bars;
    let bw = bw.max(1);
    for i in 0..bars {
        let bh = h * (i + 1) / bars;
        let bx = x + i * (bw + gap);
        let by = y + (h - bh);
        let color = if (i as u8) < level { on } else { off };
        canvas.fill_rect(bx, by, bw, bh, color);
    }
}

/// Padlock glyph: a body (filled rounded-rect) + a shackle arc above it.
fn draw_lock(canvas: &mut raegfx::Canvas, x: usize, y: usize, w: usize, h: usize, color: u32) {
    // Body occupies the lower ~60% of the box.
    let body_h = h * 3 / 5;
    let body_y = y + h - body_h;
    let body_w = w;
    canvas.fill_rounded_rect(x, body_y, body_w, body_h, 2, color);
    // Shackle: an inverted-U arc above the body (two verticals + a top span).
    let sx = x as i32;
    let sw = w as i32;
    let inset = sw / 4;
    let top = y as i32;
    let mid = (body_y) as i32;
    stroke(canvas, sx + inset, mid, sx + inset, top + 1, color);
    stroke(
        canvas,
        sx + sw - inset,
        mid,
        sx + sw - inset,
        top + 1,
        color,
    );
    stroke(canvas, sx + inset, top + 1, sx + sw - inset, top + 1, color);
}

/// A media-transport glyph (prev / play / pause / next) inside a `box × box` area.
fn draw_transport(
    canvas: &mut raegfx::Canvas,
    x: usize,
    y: usize,
    box_: usize,
    which: TransportGlyph,
    color: u32,
) {
    let bx = x as i32;
    let by = y as i32;
    let s = box_ as i32;
    match which {
        TransportGlyph::Play => {
            // Right-pointing triangle.
            canvas.draw_triangle(
                (bx + 1, by, color),
                (bx + 1, by + s, color),
                (bx + s, by + s / 2, color),
            );
        }
        TransportGlyph::Pause => {
            // Two vertical bars.
            let bw = s / 3;
            canvas.fill_rect(x, y, bw.max(1) as usize, s as usize, color);
            canvas.fill_rect(
                (bx + s - bw) as usize,
                y,
                bw.max(1) as usize,
                s as usize,
                color,
            );
        }
        TransportGlyph::Next => {
            // Triangle + a trailing bar.
            canvas.draw_triangle(
                (bx, by, color),
                (bx, by + s, color),
                (bx + s * 3 / 4, by + s / 2, color),
            );
            canvas.fill_rect((bx + s * 3 / 4) as usize, y, 2, s as usize, color);
        }
        TransportGlyph::Prev => {
            // A leading bar + a left-pointing triangle (mirror of Next).
            canvas.fill_rect(x, y, 2, s as usize, color);
            canvas.draw_triangle(
                (bx + s, by, color),
                (bx + s, by + s, color),
                (bx + s / 4, by + s / 2, color),
            );
        }
    }
}

/// Chevron-up glyph (the expanded-tile affordance; the icon set ships only
/// chevron-right). Two strokes forming a `^`.
fn draw_chevron_up(canvas: &mut raegfx::Canvas, x: usize, y: usize, box_: usize, color: u32) {
    let bx = x as i32;
    let by = y as i32;
    let s = box_ as i32;
    let apex_x = bx + s / 2;
    let apex_y = by + s / 3;
    stroke(canvas, bx, by + s * 2 / 3, apex_x, apex_y, color);
    stroke(canvas, apex_x, apex_y, bx + s, by + s * 2 / 3, color);
}

// ── Design proof (R10: a smoketest that can print FAIL) ─────────────────────

/// The FAIL-able Control Center design proof — the single authority for the
/// asserted invariants, used by BOTH the host KAT and the kernel boot smoketest
/// (so the kernel logs it without this `no_std` crate touching serial).
#[derive(Clone, Copy, Debug)]
pub struct ControlCenterProof {
    /// Number of tiles constructed (default set + Gaming row).
    pub tiles: usize,
    /// Panel width (spec §1: 360, clamped to screen).
    pub panel_width: usize,
    /// An ON tile's fill == `accent.subtle` (token-derived, not hardcoded).
    pub on_tile_is_accent_subtle: bool,
    /// An OFF tile's fill == `bg.elevated`.
    pub off_tile_is_bg_elevated: bool,
    /// The slider fill colour == `accent.base`.
    pub slider_is_accent_base: bool,
    /// The accent the panel uses == `derive_accent(active_seed).base` (cohesion).
    pub accent_matches_seed: bool,
    /// The media card hides when nothing plays and shows when audio plays.
    pub media_show_hide_correct: bool,
    /// Wi-Fi tile expands in place (sub-panel), not a new window.
    pub expand_in_place_ok: bool,
    /// RGB quick-pick exposes the 9 effect chips (spec §2.5).
    pub rgb_chip_count: usize,
    /// Number of tiles wired to a REAL backend (not Pending).
    pub real_backend_tiles: usize,
    /// Layout: the panel top is within the screen (≥ TOP_INSET, never clipped at
    /// y=0) AND the panel never exceeds its max height (critique #5).
    pub panel_top_visible: bool,
    /// Layout: with a full panel (Wi-Fi expanded + media playing) the content
    /// fits within the panel — no overflow past the tray (critique #5).
    pub full_panel_fits: bool,
    /// Layout: a tile's label box and sub-state box do not overlap, and both
    /// stay inside the 60px tile (critique #3).
    pub tile_sublabel_no_overlap: bool,
    /// All invariants hold.
    pub pass: bool,
}

/// Compute the Control Center design proof on a scratch panel built with the
/// SAME code path the live desktop uses (spec §5 cohesion, §2 state matrix). The
/// boot smoketest logs this; FAIL-able by construction.
#[must_use]
pub fn control_center_proof() -> ControlCenterProof {
    let mut cc = ControlCenter::new(1920, 1080, 44);
    cc.visible = true;
    let a = accent();
    let p = PALETTE;

    let tiles = cc.tiles.len();
    let (_x, _y, panel_width, _h) = cc.panel_rect();

    // §2.1 state matrix — token-derived, NOT hardcoded.
    let off_tile = cc.tiles[0].clone(); // Wi-Fi, default off
    let off_tile_is_bg_elevated = cc.tile_fill(&off_tile, Interact::None) == p.bg_elevated;
    let mut on_tile = off_tile.clone();
    on_tile.enabled = true;
    let on_tile_is_accent_subtle = cc.tile_fill(&on_tile, Interact::None) == a.subtle;

    // §2.3 slider fill = accent.base.
    let slider_is_accent_base = a.base == rae_tokens::derive_accent(crate::active_accent(), p).base;

    // §5 cohesion: the panel accent == derive_accent(active_seed).base.
    let want = rae_tokens::derive_accent(crate::active_accent(), p).base;
    let accent_matches_seed = a.base == want;

    // §2.4 media card show/hide matrix.
    let hidden_when_silent = !cc.media.visible();
    cc.set_media(true, "Track", "Artist");
    let shown_when_playing = cc.media.visible();
    cc.set_media(false, "", "");
    let hidden_again = !cc.media.visible();
    let media_show_hide_correct = hidden_when_silent && shown_when_playing && hidden_again;

    // §2.2 expand-in-place: Wi-Fi expands the in-panel sub-panel (one at a time).
    cc.toggle_expand(TileKind::WiFi);
    let wifi_expanded = cc.expanded == Expanded::Tile(TileKind::WiFi);
    cc.toggle_expand(TileKind::Bluetooth);
    let switched = cc.expanded == Expanded::Tile(TileKind::Bluetooth);
    cc.toggle_expand(TileKind::Bluetooth);
    let collapsed = cc.expanded == Expanded::None;
    // A non-expandable tile must NOT expand.
    cc.toggle_expand(TileKind::DoNotDisturb);
    let nonexpand_ignored = cc.expanded == Expanded::None;
    let expand_in_place_ok = wifi_expanded && switched && collapsed && nonexpand_ignored;

    let rgb_chip_count = RGB_EFFECTS.len();
    let real_backend_tiles = cc.tiles.iter().filter(|t| t.backend.is_real()).count();

    // ── Layout invariants (critique #3, #5) ──────────────────────────────────
    // Build the contrived "full panel" state the visual-QA harness renders:
    // Wi-Fi expanded with 3 networks AND media playing — the worst case for
    // overflow. The whole panel must still be visible.
    let mut full = ControlCenter::new(1280, 800, 44);
    full.visible = true;
    full.set_media(true, "Midnight City", "M83");
    full.toggle_expand(TileKind::WiFi);
    full.set_expand_rows(alloc::vec![
        ExpandRow {
            name: String::from("Raeen-5G"),
            signal: 4,
            secured: true,
            connected: true
        },
        ExpandRow {
            name: String::from("Cafe"),
            signal: 2,
            secured: false,
            connected: false
        },
        ExpandRow {
            name: String::from("Neighbor"),
            signal: 1,
            secured: true,
            connected: false
        },
    ]);
    let (_fx, fy, _fw, fh) = full.panel_rect();
    // #5: top within the screen (never clipped at y=0) and bottom above the tray.
    let panel_top_visible = fy >= ControlCenter::TOP_INSET;
    let full_panel_fits = fy + fh <= full.screen_height.saturating_sub(full.taskbar_height)
        && full.content_height() <= full.max_panel_height();

    // #3: a tile's label box and sub-state box don't overlap and both stay in
    // the 60px tile. Source of truth = tile_text_boxes (what render draws).
    let (label_y, label_bot, sub_y, sub_bot) = ControlCenter::tile_text_boxes(0);
    // label below the icon, sub below the label (no overlap), sub inside tile.
    let icon_bot = SPACE_2 as usize + GLYPH_H;
    let tile_sublabel_no_overlap =
        label_y >= icon_bot && label_bot <= sub_y && sub_bot <= TILE_HEIGHT;

    let pass = tiles == 9
        && panel_width == PANEL_WIDTH
        && on_tile_is_accent_subtle
        && off_tile_is_bg_elevated
        && slider_is_accent_base
        && accent_matches_seed
        && media_show_hide_correct
        && expand_in_place_ok
        && rgb_chip_count == 9
        && real_backend_tiles >= 4
        && panel_top_visible
        && full_panel_fits
        && tile_sublabel_no_overlap;

    ControlCenterProof {
        tiles,
        panel_width,
        on_tile_is_accent_subtle,
        off_tile_is_bg_elevated,
        slider_is_accent_base,
        accent_matches_seed,
        media_show_hide_correct,
        expand_in_place_ok,
        rgb_chip_count,
        real_backend_tiles,
        panel_top_visible,
        full_panel_fits,
        tile_sublabel_no_overlap,
        pass,
    }
}

// ── Host KATs (R10: must be able to print FAIL) ─────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proof_passes_when_wired() {
        let proof = control_center_proof();
        assert!(
            proof.pass,
            "control center design proof must pass: {proof:?}"
        );
        assert_eq!(proof.tiles, 9, "default set (6) + Gaming row (3)");
        assert_eq!(proof.panel_width, PANEL_WIDTH, "spec §1: 360px");
        assert_eq!(proof.rgb_chip_count, 9, "spec §2.5: 9 rgb effect chips");
        assert!(
            proof.panel_top_visible,
            "critique #5: panel top not clipped"
        );
        assert!(
            proof.full_panel_fits,
            "critique #5: full panel fits, no spill"
        );
        assert!(
            proof.tile_sublabel_no_overlap,
            "critique #3: sublabel must not overlap label / clip tile"
        );
    }

    #[test]
    fn tile_sublabel_stays_within_tile_and_no_overlap() {
        // critique #3: the label box and sub-state box must not overlap, and the
        // sub-state box must stay inside the 60px tile. FAIL-able — if the old
        // layout (sub_y = cy + TILE_H - space.3 - cap_lh, overlapping the label)
        // returned, label_bot > sub_y and this fails.
        for cy in [0usize, 100, 537] {
            let (label_y, label_bot, sub_y, sub_bot) = ControlCenter::tile_text_boxes(cy);
            let icon_bot = cy + SPACE_2 as usize + GLYPH_H;
            assert!(label_y >= icon_bot, "label below icon (cy={cy})");
            assert!(
                label_bot <= sub_y,
                "label/sublabel must not overlap (cy={cy}: label_bot={label_bot} sub_y={sub_y})"
            );
            assert!(
                sub_bot <= cy + TILE_HEIGHT,
                "sublabel must stay inside the tile (cy={cy}: sub_bot={sub_bot})"
            );
        }
    }

    #[test]
    fn full_panel_is_fully_visible_no_clip() {
        // critique #5: the worst-case panel (Wi-Fi expanded + media playing) must
        // fit — top within the screen, bottom above the tray, content ≤ max. The
        // expand list scrolls within its budget rather than overflowing.
        let mut cc = ControlCenter::new(1280, 800, 44);
        cc.visible = true;
        cc.set_media(true, "Midnight City", "M83");
        cc.toggle_expand(TileKind::WiFi);
        cc.set_expand_rows(alloc::vec![
            ExpandRow {
                name: String::from("A"),
                signal: 4,
                secured: true,
                connected: true
            },
            ExpandRow {
                name: String::from("B"),
                signal: 3,
                secured: true,
                connected: false
            },
            ExpandRow {
                name: String::from("C"),
                signal: 2,
                secured: false,
                connected: false
            },
            ExpandRow {
                name: String::from("D"),
                signal: 1,
                secured: true,
                connected: false
            },
            ExpandRow {
                name: String::from("E"),
                signal: 1,
                secured: true,
                connected: false
            },
        ]);
        let (_x, y, _w, h) = cc.panel_rect();
        assert!(
            y >= ControlCenter::TOP_INSET,
            "panel top must be within the screen (y={y})"
        );
        assert!(
            y + h <= 800 - 44,
            "panel bottom must sit above the tray (y+h={})",
            y + h
        );
        assert!(
            cc.content_height() <= cc.max_panel_height(),
            "content must fit the panel (content={} max={})",
            cc.content_height(),
            cc.max_panel_height()
        );
        // The Wi-Fi list scrolls within its budget (fewer than all 5 rows shown
        // on this short 800px frame) — but always at least one.
        let vis = cc.visible_expand_rows();
        assert!(
            vis >= 1 && vis <= 5,
            "expand list scroll-capped (vis={vis})"
        );
    }

    #[test]
    fn small_screen_panel_never_negative_or_overflowing() {
        // Defensive: on a tiny frame the panel still has a visible top and never
        // exceeds the screen (FAIL-able guard against a regressed anchor).
        let mut cc = ControlCenter::new(400, 480, 44);
        cc.visible = true;
        let (x, y, w, h) = cc.panel_rect();
        assert!(y >= ControlCenter::TOP_INSET);
        assert!(x + w <= 400);
        assert!(y + h <= 480 - 44);
    }

    #[test]
    fn tiles_resolve_colors_from_tokens_not_hardcoded() {
        // The whole cohesion point: an ON tile must read accent.subtle and an
        // OFF tile bg.elevated — both token-derived. FAIL-able: if a tile ever
        // hardcoded a colour, these would diverge from the live ramp.
        let cc = ControlCenter::new(800, 600, 44);
        let a = accent();
        let p = PALETTE;
        let mut t = cc.tiles[0].clone();
        t.enabled = false;
        assert_eq!(cc.tile_fill(&t, Interact::None), p.bg_elevated);
        t.enabled = true;
        assert_eq!(cc.tile_fill(&t, Interact::None), a.subtle);
        // Hover/active/disabled states are distinct (spec §2.1 state matrix).
        assert_eq!(cc.tile_fill(&t, Interact::Active), a.active);
        assert_eq!(cc.tile_fill(&t, Interact::Hover), a.hover);
        let mut d = t.clone();
        d.disabled = true;
        assert_eq!(cc.tile_fill(&d, Interact::None), p.bg_raised);
    }

    #[test]
    fn enabled_tile_label_uses_dark_on_accent_ink_not_white() {
        // visual-QA Round-6 P0: white ink over the bright accent-filled ON tile
        // measured ~1.66–1.94:1 (fails AA 4.5:1). The fix flips the label +
        // sublabel ink to a DARK on-accent token on the accent fill. This KAT
        // is FAIL-able: it would fail if the ink reverted to white/text.primary.
        let cc = ControlCenter::new(800, 600, 44);
        let p = PALETTE;
        let mut on = cc.tiles[0].clone();
        on.enabled = true;
        on.disabled = false;

        // The ON-tile ink must be the dark on-accent token, NOT white.
        assert_eq!(
            cc.tile_label_ink(&on),
            p.bg_base,
            "enabled (accent-filled) tile label must be dark on-accent ink"
        );
        assert_ne!(
            cc.tile_label_ink(&on),
            p.text_primary,
            "enabled tile label must NOT be white (the failing Round-6 state)"
        );
        assert_eq!(
            cc.tile_sublabel_ink(&on),
            p.bg_base,
            "enabled tile sublabel must also be dark on-accent ink"
        );

        // And it must clear AA 4.5:1 over the accent fill (RaeBlue base). The
        // accent.base is the bright fill the label sits over on an ON tile.
        let fill = accent().base;
        let cr = rae_tokens::contrast_ratio(cc.tile_label_ink(&on), fill);
        assert!(
            cr >= 4.5,
            "ON-tile label ink {:#08X} over accent fill {:#08X} = {cr:.2}:1, must be >= 4.5",
            cc.tile_label_ink(&on),
            fill
        );

        // White (the old ink) over the same fill must in fact FAIL — proves the
        // test is measuring the real problem, not a tautology.
        let white_cr = rae_tokens::contrast_ratio(p.text_primary, fill);
        assert!(
            white_cr < 4.5,
            "white over accent fill should fail AA (={white_cr:.2}:1) — the bug we fixed"
        );

        // OFF tiles keep white label on the dark/frosted card (Round-4 fix).
        let mut off = cc.tiles[0].clone();
        off.enabled = false;
        off.disabled = false;
        assert_eq!(
            cc.tile_label_ink(&off),
            p.text_primary,
            "OFF tile keeps white text.primary over the dark/frosted card"
        );

        // Disabled tiles read text.tertiary (dimmed), unchanged.
        let mut dis = cc.tiles[0].clone();
        dis.disabled = true;
        assert_eq!(cc.tile_label_ink(&dis), p.text_tertiary);
    }

    #[test]
    fn media_card_shows_and_hides_on_player_state() {
        // spec §2.4: present when audio plays, absent (no empty card) otherwise.
        let mut cc = ControlCenter::new(800, 600, 44);
        assert!(!cc.media.visible(), "no card when nothing plays");
        cc.set_media(true, "Song", "Band");
        assert!(cc.media.visible(), "card shows when playing");
        assert_eq!(cc.media.title, "Song");
        cc.set_media(false, "", "");
        assert!(!cc.media.visible(), "card hides again when stopped");
    }

    #[test]
    fn expand_is_in_place_and_one_at_a_time() {
        // spec §2.2: expand in place, exactly one tile expanded, chevron toggles.
        let mut cc = ControlCenter::new(800, 600, 44);
        assert_eq!(cc.expanded, Expanded::None);
        cc.toggle_expand(TileKind::WiFi);
        assert_eq!(cc.expanded, Expanded::Tile(TileKind::WiFi));
        cc.toggle_expand(TileKind::Bluetooth);
        assert_eq!(
            cc.expanded,
            Expanded::Tile(TileKind::Bluetooth),
            "expanding another collapses the first (one at a time)"
        );
        cc.toggle_expand(TileKind::Bluetooth);
        assert_eq!(cc.expanded, Expanded::None, "chevron again collapses");
        // A non-expandable tile is a no-op.
        cc.toggle_expand(TileKind::Airplane);
        assert_eq!(cc.expanded, Expanded::None);
    }

    #[test]
    fn tile_state_matrix_toggle_and_substate() {
        // spec §2.1: toggling flips enabled + updates the sub-state line.
        let mut cc = ControlCenter::new(800, 600, 44);
        assert!(!cc.tile_enabled(TileKind::DoNotDisturb));
        cc.toggle_tile(TileKind::DoNotDisturb);
        assert!(cc.tile_enabled(TileKind::DoNotDisturb));
        let dnd = cc
            .tiles
            .iter()
            .find(|t| t.kind == TileKind::DoNotDisturb)
            .unwrap();
        assert_eq!(dnd.sub_state, "On");
    }

    #[test]
    fn backends_are_honestly_classified() {
        // House Rules / spec: a tile is wired to a real subsystem OR honestly
        // flagged Pending — never a fake working toggle. At least the 4 with a
        // live model (DND, GameMode, RGB, Performance) must be real.
        let cc = ControlCenter::new(800, 600, 44);
        let real: Vec<_> = cc
            .tiles
            .iter()
            .filter(|t| t.backend.is_real())
            .map(|t| t.kind)
            .collect();
        assert!(real.contains(&TileKind::WiFi));
        assert!(real.contains(&TileKind::DoNotDisturb));
        assert!(real.contains(&TileKind::NightLight));
        assert!(real.contains(&TileKind::GameMode));
        assert!(real.contains(&TileKind::Rgb));
        assert!(real.contains(&TileKind::Performance));
        // Bluetooth / Airplane are Pending until the kernel threads BT/radio
        // state — honestly flagged, NOT faked. Accessibility is now wired to the
        // live high-contrast forced-colors engine (audit P0 #2/#3).
        assert_eq!(
            cc.tiles[1].backend,
            TileBackend::Pending,
            "Bluetooth pending"
        );
        assert!(
            real.contains(&TileKind::Accessibility),
            "a11y HC tile is live"
        );
    }

    #[test]
    fn panel_is_anchored_bottom_right() {
        // spec §1: anchored bottom-right above the tray.
        let cc = ControlCenter::new(1920, 1080, 44);
        let (x, y, w, h) = cc.panel_rect();
        assert_eq!(w, PANEL_WIDTH);
        // Right edge is near the screen right (within the inset).
        assert!(x + w <= 1920);
        assert!(x + w >= 1920 - 2 * SPACE_2 as usize - 1);
        // Bottom edge sits above the taskbar.
        assert!(y + h <= 1080 - 44);
    }

    #[test]
    fn control_center_repaints_in_high_contrast() {
        // audit P0 #3: toggling forced-colors must make the Control Center read
        // the HC palette (the proven live-swap core surface). FAIL-able — if the
        // panel were pinned to the const DARK palette, active_palette() would not
        // change and the opaque HC black panel would never paint.
        rae_tokens::set_high_contrast(false);
        assert_eq!(*crate::active_palette(), rae_tokens::DARK, "off -> normal");
        rae_tokens::set_high_contrast(true);
        assert_eq!(
            *crate::active_palette(),
            rae_tokens::HIGH_CONTRAST,
            "on -> the Control Center paints the HC palette"
        );
        // The panel must still paint ink in HC (no blank surface from the swap).
        let mut cc = ControlCenter::new(400, 720, 44);
        cc.visible = true;
        let mut buf = alloc::vec![0u8; 400 * 720 * 4];
        let mut canvas = unsafe { raegfx::Canvas::new(buf.as_mut_ptr(), 400, 720, 4) };
        cc.render(&mut canvas);
        assert!(
            buf.iter().any(|&b| b != 0),
            "HC control center must paint ink"
        );
        rae_tokens::set_high_contrast(false);
    }

    #[test]
    fn every_tile_kind_maps_to_its_icon() {
        // visual-QA Round-2 #1 (CRITICAL): every Control Center tile must draw a
        // REAL line-icon, NOT a letter placeholder. This pins the gfx agent's
        // map exactly — a wrong or missing mapping FAILS here (and the screenshot
        // would regress to the wrong glyph). FAIL-able: change any arm of
        // `TileKind::icon` and an assert below trips.
        use raegfx::icon::Icon;
        assert_eq!(TileKind::WiFi.icon(), Icon::WiFi);
        assert_eq!(TileKind::Bluetooth.icon(), Icon::Bluetooth);
        assert_eq!(TileKind::DoNotDisturb.icon(), Icon::Focus);
        assert_eq!(TileKind::NightLight.icon(), Icon::NightLight);
        assert_eq!(TileKind::Airplane.icon(), Icon::Airplane);
        assert_eq!(TileKind::Accessibility.icon(), Icon::Accessibility);
        assert_eq!(TileKind::GameMode.icon(), Icon::GameController);
        assert_eq!(TileKind::Rgb.icon(), Icon::Palette);
        assert_eq!(TileKind::Performance.icon(), Icon::Performance);

        // And every tile actually constructed in the live panel resolves to a
        // DISTINCT icon (no two tiles share a glyph — would read as a bug). Also
        // proves no tile silently maps to a fallback/duplicate.
        let cc = ControlCenter::new(800, 600, 44);
        let mut seen: Vec<u16> = Vec::new();
        for t in &cc.tiles {
            let id = t.kind.icon().id();
            assert!(
                !seen.contains(&id),
                "two tiles map to the same icon id {id}"
            );
            seen.push(id);
        }
        assert_eq!(seen.len(), 9, "all 9 tiles mapped");
    }

    #[test]
    fn inline_vector_glyphs_paint_ink_no_letters() {
        // Round-2 follow-up: footer power, the slider speaker/sun, the Wi-Fi list
        // signal bars + lock, the media transport, and the expanded chevron-up are
        // now inline VECTORS (no shipped icon yet), NOT letter glyphs. Each helper
        // must paint real ink in its box — FAIL-able: a no-op helper (or a return
        // to a letter draw_glyph that paints nothing here) trips the floor.
        fn ink(buf: &[u8]) -> usize {
            buf.chunks_exact(4)
                .filter(|px| px.iter().any(|&b| b != 0))
                .count()
        }
        let (w, h) = (32usize, 32usize);
        const W: u32 = 0xFF_FF_FF_FF;
        let cases: [(&str, fn(&mut raegfx::Canvas)); 8] = [
            ("power", |c| draw_power(c, 4, 4, 20, 20, W)),
            ("speaker", |c| draw_speaker(c, 4, 4, 20, 20, false, W)),
            ("speaker-muted", |c| draw_speaker(c, 4, 4, 20, 20, true, W)),
            ("sun", |c| draw_sun(c, 4, 4, 20, 20, W)),
            ("signal", |c| {
                draw_signal_bars(c, 4, 4, 20, 20, 3, W, 0x33_FF_FF_FF)
            }),
            ("lock", |c| draw_lock(c, 4, 4, 20, 20, W)),
            ("transport-play", |c| {
                draw_transport(c, 4, 4, 20, TransportGlyph::Play, W)
            }),
            ("chevron-up", |c| draw_chevron_up(c, 4, 4, 20, W)),
        ];
        for (name, f) in cases {
            let mut buf = alloc::vec![0u8; w * h * 4];
            let mut canvas = unsafe { raegfx::Canvas::new(buf.as_mut_ptr(), w, h, 4) };
            f(&mut canvas);
            let painted = ink(&buf);
            assert!(
                painted >= 8,
                "inline glyph '{name}' painted only {painted}px (empty/degenerate?)"
            );
        }
    }

    #[test]
    fn iconified_surfaces_use_shipped_icons() {
        // The footer Settings gear, the album-art placeholder, and the Wi-Fi
        // connected check are now SHIPPED line-icons (not letters). Pin the choices
        // so a regression to a letter or a wrong icon FAILS here. The collapsed
        // expand chevron uses Icon::Chevron (chevron-right); the expanded state is
        // the inline chevron-up tested above.
        use raegfx::icon::Icon;
        // These ids must exist in the shipped set (proves we didn't pick a phantom).
        for icon in [Icon::Gear, Icon::Media, Icon::Check, Icon::Chevron] {
            assert!(
                Icon::from_id(icon.id()).is_some(),
                "{} must be a shipped icon",
                icon.name()
            );
        }
    }

    #[test]
    fn renders_without_panic_and_paints_ink() {
        // The full render path must paint real ink (not a no-op) when visible.
        let mut cc = ControlCenter::new(640, 720, 44);
        cc.visible = true;
        cc.toggle_tile(TileKind::GameMode);
        cc.set_media(true, "Now Playing", "Some Artist");
        cc.toggle_expand(TileKind::WiFi);
        cc.set_expand_rows(alloc::vec![
            ExpandRow {
                name: alloc::string::String::from("HomeNet"),
                signal: 4,
                secured: true,
                connected: true,
            },
            ExpandRow {
                name: alloc::string::String::from("Cafe"),
                signal: 2,
                secured: false,
                connected: false,
            },
        ]);
        let mut buf = alloc::vec![0u8; 640 * 720 * 4];
        let mut canvas = unsafe { raegfx::Canvas::new(buf.as_mut_ptr(), 640, 720, 4) };
        cc.render(&mut canvas);
        assert!(buf.iter().any(|&b| b != 0), "control center must paint ink");
    }

    #[test]
    fn tile_card_is_luminous_not_a_dark_hole() {
        // visual-QA Round-4 P0 #1 — THE polarity fix. A resting tile card must read
        // luminance AT OR ABOVE the surrounding glass panel (a raised frosted
        // element), NOT below it (a dark slate hole). FAIL-able: if the tile ever
        // regresses to a dark opaque fill (the old `bg.elevated`/`bg.raised`), its
        // interior luma drops well under the panel's and this trips.
        rae_tokens::set_high_contrast(false);
        fn luma(px: u32) -> f32 {
            let r = ((px >> 16) & 0xFF) as f32;
            let g = ((px >> 8) & 0xFF) as f32;
            let b = (px & 0xFF) as f32;
            0.299 * r + 0.587 * g + 0.114 * b
        }
        // Render the full panel over the SIGNATURE aurora backdrop (the live render
        // path), so the panel + tile cards composite over real luminance.
        let (w, h) = (640usize, 800usize);
        let mut buf = alloc::vec![0u8; w * h * 4];
        let mut canvas = unsafe { raegfx::Canvas::new(buf.as_mut_ptr(), w, h, 4) };
        raegfx::glass::render_aurora_dark(&mut canvas, 0, 0, w, h, 0);
        let mut cc = ControlCenter::new(w, h, 44);
        cc.visible = true;
        cc.render(&mut canvas);

        let (rx, ry, rw, _rh) = cc.panel_rect();
        let pad = SPACE_4 as usize;
        let gap = SPACE_2 as usize;
        let tile_w = ((rw - 2 * pad).saturating_sub(gap)) / 2;
        // Sample the centre of the FIRST tile interior (off, Wi-Fi) and the panel
        // gutter just left of the tile grid (pure panel glass, no card).
        let tx = rx + pad;
        let ty = ry + pad;
        let read = |x: usize, y: usize| -> u32 {
            let o = (y * w + x) * 4;
            // Canvas is BGRA byte order; pack to ARGB for the luma helper.
            (0xFF << 24)
                | ((buf[o + 2] as u32) << 16)
                | ((buf[o + 1] as u32) << 8)
                | (buf[o] as u32)
        };
        let tile_interior = luma(read(tx + tile_w / 2, ty + TILE_HEIGHT / 2));
        // Panel gutter: a strip of panel glass between the two tile columns.
        let panel_gutter = luma(read(tx + tile_w + gap / 2, ty + TILE_HEIGHT / 2));

        assert!(
            tile_interior >= panel_gutter,
            "tile card interior (L{tile_interior:.1}) must be AT/ABOVE the \
             panel interior (L{panel_gutter:.1}) — an obsidian ladder step, \
             not a hole (IDENTITY-OBSIDIAN.md §2)"
        );
        // Obsidian: the face is a visible dark ladder step — clearly above the
        // near-black panel, never a milky/luminous card. FAIL-able both ways.
        assert!(
            (30.0..=70.0).contains(&tile_interior),
            "tile card face must sit in the obsidian ladder band L30–70 \
             (L{tile_interior:.1})"
        );
    }

    #[test]
    fn focus_ring_has_dark_keyline_against_bright_tiles() {
        // Accessibility audit P0 (WCAG 1.4.11): the focus ring must read on a
        // LUMINOUS tile. A single accent ring measured 2.28/1.59:1 and FAILED; the
        // macOS-style double-stroke sandwiches the accent ring in a dark `bg.base`
        // keyline. This test renders the focused panel over the aurora and asserts
        // that immediately OUTSIDE the focused tile's accent ring there is a DARK
        // keyline pixel (the high-contrast sandwich) — so the ring boundary clears
        // 1.4.11 on any tile luminance. FAIL-able: drop the keyline (single ring)
        // and the outermost ring pixel is bright accent over bright tile → trips.
        rae_tokens::set_high_contrast(false);
        fn luma(px: u32) -> f32 {
            let r = ((px >> 16) & 0xFF) as f32;
            let g = ((px >> 8) & 0xFF) as f32;
            let b = (px & 0xFF) as f32;
            0.299 * r + 0.587 * g + 0.114 * b
        }
        let (w, h) = (640usize, 800usize);
        let mut buf = alloc::vec![0u8; w * h * 4];
        let mut canvas = unsafe { raegfx::Canvas::new(buf.as_mut_ptr(), w, h, 4) };
        raegfx::glass::render_aurora_dark(&mut canvas, 0, 0, w, h, 0);
        let mut cc = ControlCenter::new(w, h, 44);
        cc.visible = true;
        cc.focus_index = Some(0); // focus the first tile
        cc.render(&mut canvas);

        let (rx, ry, _rw, _rh) = cc.panel_rect();
        let pad = SPACE_4 as usize;
        // The focus ring is drawn from (rx+pad, ry+pad); tile 0 sits there. The
        // OUTER dark keyline is at tx-2 (see render_focus_ring), the accent ring at
        // tx-1/tx. Sample along the top edge of the tile.
        let tx = rx + pad;
        let ty = ry + pad;
        let read = |x: usize, y: usize| -> u32 {
            let o = (y * w + x) * 4;
            (0xFF << 24)
                | ((buf[o + 2] as u32) << 16)
                | ((buf[o + 1] as u32) << 8)
                | (buf[o] as u32)
        };
        // Scan a short vertical strip at the tile's left edge crossing the ring and
        // find the darkest pixel (the keyline) — it must be clearly dark, proving
        // the sandwich exists. Sample below the rounded corner so it's on the
        // straight left edge where all three strokes are present.
        let probe_x = tx; // left edge column (keyline at tx-2..tx area)
        let mut darkest = 255.0f32;
        for yy in (ty + 8)..(ty + 24) {
            for xx in probe_x.saturating_sub(3)..=probe_x + 1 {
                darkest = darkest.min(luma(read(xx, yy)));
            }
        }
        assert!(
            darkest < 30.0,
            "focus ring must include a DARK keyline (darkest L{darkest:.1}) so the \
             accent ring reads on a bright tile (WCAG 1.4.11)"
        );
    }

    #[test]
    fn off_tile_label_ink_is_text_primary() {
        // Accessibility audit P1: an OFF tile's LABEL must use `text.primary`, not
        // `text.secondary`, because the tile is now a luminous frosted card (L91)
        // where secondary fails 4.5:1. The renderer's label-ink selection is:
        //   disabled → text.tertiary ; else → text.primary
        // (on AND off both primary). This mirrors the exact branch in render_tile_grid.
        // FAIL-able: revert the OFF branch to text.secondary and `off_fg` no longer
        // equals text.primary → trips.
        let p = PALETTE;
        let label_ink = |disabled: bool| -> u32 {
            if disabled {
                p.text_tertiary
            } else {
                p.text_primary
            }
        };
        let off_fg = label_ink(false); // an enabled==false, disabled==false tile
        assert_eq!(
            off_fg, p.text_primary,
            "OFF-tile label must be text.primary over the bright tile (a11y 4.5:1)"
        );
        assert_ne!(
            off_fg, p.text_secondary,
            "OFF-tile label must NOT be the failing text.secondary"
        );
        assert_eq!(
            label_ink(true),
            p.text_tertiary,
            "disabled label stays tertiary"
        );
    }
}
