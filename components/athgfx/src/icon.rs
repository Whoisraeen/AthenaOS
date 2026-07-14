//! AthGFX line-icon system — data-driven, crisp, scalable vector icons.
//!
//! *"Built for people who care about how things feel."* — `LEGACY_GAMING_CONCEPT.md`
//! §AthUI. The visual-QA pixel critique (`docs/design/visual-qa-critique-2026-06-21.md`,
//! findings #1/#2) flagged the single worst "looks basic" defect: Control Center
//! tiles and the Files window draw single LETTER placeholders (W/B/F/N/A/X/G/R/P,
//! H/D/L/M) where icons belong. That reads "1995 terminal," not Sequoia.
//!
//! This module replaces those letters with real **line icons** in the
//! Feather/Lucide register the design language asks for (`design-language.md`:
//! clean / minimal): a consistent stroke on a normalized 24-unit grid, rounded
//! caps and joins, **monochrome** so each icon tints with a single token color.
//!
//! Design properties:
//!   * **Data-driven** — every icon is a `&'static [IconCmd]` (lines, polylines,
//!     circles, arcs, rounded-rects) on a `[0,24]×[0,24]` grid. Adding an icon is
//!     adding a const slice + an enum arm; no new rendering code.
//!   * **Crisp + scalable** — strokes are rasterized with a coverage-based
//!     anti-aliased distance field at *any* target size, so an icon is sharp at a
//!     22px tile and at a 96px hero with no bitmap blur and no letter glyph.
//!   * **`no_std`** — pure integer fixed-point (Q8) + an integer sqrt; the Canvas
//!     is `no_std` and the kernel composites with this exact code.
//!
//! Consumed by `draw_icon` on [`crate::Canvas`]; the shell (`athshell`
//! control_center / file_manager) and apps wire the `Icon` ids into their tiles.

use crate::Canvas;

/// Normalized design grid: every icon is authored on a `GRID × GRID` square
/// (matching Feather/Lucide's 24-unit viewBox) and scaled to the requested size.
pub const GRID: i32 = 24;

/// A single drawing command in an icon's primitive list. Coordinates are in
/// grid units (`0..=GRID`) and scaled to the target size at draw time. Every
/// command is a *stroked* outline (monochrome line art) except `Dot`, the one
/// filled primitive (status pips, the center of a record/target glyph).
#[derive(Clone, Copy, Debug)]
pub enum IconCmd {
    /// Open polyline through the given grid points (rounded caps + joins).
    Poly(&'static [(i32, i32)]),
    /// Closed polygon: like `Poly` but the last point connects back to the first.
    Closed(&'static [(i32, i32)]),
    /// Stroked circle outline: center `(cx, cy)`, radius `r` (grid units).
    Circle { cx: i32, cy: i32, r: i32 },
    /// Stroked circular arc: center, radius, start/end angle in degrees
    /// (0° = +x / right, 90° = +y / down, growing clockwise on screen).
    Arc {
        cx: i32,
        cy: i32,
        r: i32,
        a0: i32,
        a1: i32,
    },
    /// Stroked rounded-rect outline: top-left `(x, y)`, size `(w, h)`, corner `r`.
    RRect {
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        r: i32,
    },
    /// A filled dot: center `(cx, cy)`, radius `r` grid units.
    Dot { cx: i32, cy: i32, r: i32 },
    /// A FILLED rounded-rect (IDENTITY-OBSIDIAN.md §4 "color where it counts"):
    /// solid content-icon silhouettes (the macOS-style solid blue folder) built
    /// from filled shapes instead of line strokes. Rendered through the
    /// Canvas's AA `fill_rounded_rect` before the stroke batch, so strokes can
    /// detail on top.
    FilledRRect {
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        r: i32,
    },
}

/// The shipped icon set. Covers every surface flagged by the visual-QA critique:
///   * Control Center: WiFi, Bluetooth, Focus (moon), NightLight (sun),
///     Airplane, Accessibility (person), GameController, Palette (RGB),
///     Performance (gauge).
///   * Files: Folder, File (generic) + the file-type set (Code, Media, Doc,
///     Archive, Exec).
///   * System chrome: Bell, Gear, Search, Close, Chevron, Plus, Check.
///   * Control Center / media (canonicalized from the CC inline vectors,
///     commit 68321f5): Power, Volume / VolumeMuted, Brightness, SignalBars,
///     Lock, the transport set (Play / Pause / SkipPrev / SkipNext), and the
///     full chevron family (Up / Down / Left, joining the original right).
///
/// `from_id` / `id` give a stable u16 mapping so the shell can carry an icon in
/// a tile struct without importing the enum's variant names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u16)]
pub enum Icon {
    // ── Control Center ──
    WiFi = 0,
    Bluetooth = 1,
    Focus = 2,      // do-not-disturb moon
    NightLight = 3, // sun / warm light
    Airplane = 4,
    Accessibility = 5, // person in a ring
    GameController = 6,
    Palette = 7,     // RGB / theming
    Performance = 8, // speed gauge
    // ── Files ──
    Folder = 9,
    File = 10,    // generic document w/ dog-ear
    Code = 11,    // </>
    Media = 12,   // image (mountains in a frame)
    Doc = 13,     // document w/ text lines
    Archive = 14, // box / zip
    Exec = 15,    // terminal / runnable
    // ── System chrome ──
    Bell = 16,
    Gear = 17,
    Search = 18,
    Close = 19,
    Chevron = 20, // chevron-right (rotate by drawing variants later)
    Plus = 21,
    Check = 22,
    // ── Control Center / media (canonicalized from CC inline vectors,
    //    commit 68321f5) — append-only, do NOT renumber 0..=22. ──
    Power = 23,       // IEC 5009 power symbol (ring + top break + stroke)
    Volume = 24,      // speaker cone + sound waves
    VolumeMuted = 25, // speaker cone + mute slash
    Brightness = 26,  // sun: center circle + 8 rays (cooler than NightLight)
    SignalBars = 27,  // 4 ascending strength bars
    Lock = 28,        // padlock: body rrect + shackle arc
    Play = 29,        // transport: right triangle
    Pause = 30,       // transport: two bars
    SkipPrev = 31,    // transport: |◀
    SkipNext = 32,    // transport: ▶|
    ChevronUp = 33,
    ChevronDown = 34,
    ChevronLeft = 35,
    /// The Rae mark (taskbar Start / Rae-key glyph): diamond outline + orb dot.
    RaeLogo = 36,
    /// SOLID folder (IDENTITY-OBSIDIAN.md §4 "color where it counts") — the
    /// macOS-register filled silhouette for CONTENT surfaces (Files rows/
    /// sidebar, Start tiles); the stroked `Folder` stays for chrome.
    FolderSolid = 37,
}

impl Icon {
    /// Stable numeric id (so a shell tile struct can store `u16`, not the enum).
    pub fn id(self) -> u16 {
        self as u16
    }

    /// Inverse of [`Icon::id`]. Returns `None` for an unknown id.
    pub fn from_id(id: u16) -> Option<Icon> {
        Some(match id {
            0 => Icon::WiFi,
            1 => Icon::Bluetooth,
            2 => Icon::Focus,
            3 => Icon::NightLight,
            4 => Icon::Airplane,
            5 => Icon::Accessibility,
            6 => Icon::GameController,
            7 => Icon::Palette,
            8 => Icon::Performance,
            9 => Icon::Folder,
            10 => Icon::File,
            11 => Icon::Code,
            12 => Icon::Media,
            13 => Icon::Doc,
            14 => Icon::Archive,
            15 => Icon::Exec,
            16 => Icon::Bell,
            17 => Icon::Gear,
            18 => Icon::Search,
            19 => Icon::Close,
            20 => Icon::Chevron,
            21 => Icon::Plus,
            22 => Icon::Check,
            23 => Icon::Power,
            24 => Icon::Volume,
            25 => Icon::VolumeMuted,
            26 => Icon::Brightness,
            27 => Icon::SignalBars,
            28 => Icon::Lock,
            29 => Icon::Play,
            30 => Icon::Pause,
            31 => Icon::SkipPrev,
            32 => Icon::SkipNext,
            33 => Icon::ChevronUp,
            34 => Icon::ChevronDown,
            35 => Icon::ChevronLeft,
            36 => Icon::RaeLogo,
            37 => Icon::FolderSolid,
            _ => return None,
        })
    }

    /// Every icon id, in `id` order — for the icon-sheet harness and KATs.
    pub const ALL: [Icon; 38] = [
        Icon::WiFi,
        Icon::Bluetooth,
        Icon::Focus,
        Icon::NightLight,
        Icon::Airplane,
        Icon::Accessibility,
        Icon::GameController,
        Icon::Palette,
        Icon::Performance,
        Icon::Folder,
        Icon::File,
        Icon::Code,
        Icon::Media,
        Icon::Doc,
        Icon::Archive,
        Icon::Exec,
        Icon::Bell,
        Icon::Gear,
        Icon::Search,
        Icon::Close,
        Icon::Chevron,
        Icon::Plus,
        Icon::Check,
        Icon::Power,
        Icon::Volume,
        Icon::VolumeMuted,
        Icon::Brightness,
        Icon::SignalBars,
        Icon::Lock,
        Icon::Play,
        Icon::Pause,
        Icon::SkipPrev,
        Icon::SkipNext,
        Icon::ChevronUp,
        Icon::ChevronDown,
        Icon::ChevronLeft,
        Icon::RaeLogo,
        Icon::FolderSolid,
    ];

    /// Short stable name (for sheet labels / debugging).
    pub fn name(self) -> &'static str {
        match self {
            Icon::WiFi => "wifi",
            Icon::Bluetooth => "bluetooth",
            Icon::Focus => "focus",
            Icon::NightLight => "night-light",
            Icon::Airplane => "airplane",
            Icon::Accessibility => "accessibility",
            Icon::GameController => "game",
            Icon::Palette => "palette",
            Icon::Performance => "performance",
            Icon::Folder => "folder",
            Icon::File => "file",
            Icon::Code => "code",
            Icon::Media => "media",
            Icon::Doc => "doc",
            Icon::Archive => "archive",
            Icon::Exec => "exec",
            Icon::Bell => "bell",
            Icon::Gear => "gear",
            Icon::Search => "search",
            Icon::Close => "close",
            Icon::Chevron => "chevron",
            Icon::Plus => "plus",
            Icon::Check => "check",
            Icon::Power => "power",
            Icon::Volume => "volume",
            Icon::VolumeMuted => "volume-muted",
            Icon::Brightness => "brightness",
            Icon::SignalBars => "signal-bars",
            Icon::Lock => "lock",
            Icon::Play => "play",
            Icon::Pause => "pause",
            Icon::SkipPrev => "skip-prev",
            Icon::SkipNext => "skip-next",
            Icon::ChevronUp => "chevron-up",
            Icon::ChevronDown => "chevron-down",
            Icon::ChevronLeft => "chevron-left",
            Icon::RaeLogo => "rae-logo",
            Icon::FolderSolid => "folder-solid",
        }
    }

    /// The icon's vector definition: a list of stroked/filled primitives on the
    /// `[0,GRID]` grid. This is the entire icon "font" — data, not code.
    pub fn commands(self) -> &'static [IconCmd] {
        match self {
            Icon::WiFi => WIFI,
            Icon::Bluetooth => BLUETOOTH,
            Icon::Focus => FOCUS,
            Icon::NightLight => NIGHT_LIGHT,
            Icon::Airplane => AIRPLANE,
            Icon::Accessibility => ACCESSIBILITY,
            Icon::GameController => GAME_CONTROLLER,
            Icon::Palette => PALETTE,
            Icon::Performance => PERFORMANCE,
            Icon::Folder => FOLDER,
            Icon::File => FILE,
            Icon::Code => CODE,
            Icon::Media => MEDIA,
            Icon::Doc => DOC,
            Icon::Archive => ARCHIVE,
            Icon::Exec => EXEC,
            Icon::Bell => BELL,
            Icon::Gear => GEAR,
            Icon::Search => SEARCH,
            Icon::Close => CLOSE,
            Icon::Chevron => CHEVRON,
            Icon::Plus => PLUS,
            Icon::Check => CHECK,
            Icon::Power => POWER,
            Icon::Volume => VOLUME,
            Icon::VolumeMuted => VOLUME_MUTED,
            Icon::Brightness => BRIGHTNESS,
            Icon::SignalBars => SIGNAL_BARS,
            Icon::Lock => LOCK,
            Icon::Play => PLAY,
            Icon::Pause => PAUSE,
            Icon::SkipPrev => SKIP_PREV,
            Icon::SkipNext => SKIP_NEXT,
            Icon::ChevronUp => CHEVRON_UP,
            Icon::ChevronDown => CHEVRON_DOWN,
            Icon::ChevronLeft => CHEVRON_LEFT,
            Icon::RaeLogo => RAE_LOGO,
            Icon::FolderSolid => FOLDER_SOLID,
        }
    }
}

// ════════════════════════════════════════════════════════════════════════════
// Icon definitions — each a primitive list on the 24-unit grid.
// Authored to read at small tile sizes: generous, simple strokes (no fiddly
// detail that mushes below ~20px). Geometry mirrors the Feather/Lucide silhouette
// so they're instantly recognizable.
// ════════════════════════════════════════════════════════════════════════════

// WiFi: three concentric arcs (signal fan) + a dot. Arcs open upward.
static WIFI: &[IconCmd] = &[
    IconCmd::Arc {
        cx: 12,
        cy: 19,
        r: 11,
        a0: 215,
        a1: 325,
    },
    IconCmd::Arc {
        cx: 12,
        cy: 19,
        r: 7,
        a0: 210,
        a1: 330,
    },
    IconCmd::Arc {
        cx: 12,
        cy: 19,
        r: 3,
        a0: 205,
        a1: 335,
    },
    IconCmd::Dot {
        cx: 12,
        cy: 19,
        r: 1,
    },
];

// Bluetooth: the runic "B" — a vertical spine with two crossed diagonals.
static BLUETOOTH: &[IconCmd] = &[IconCmd::Poly(&[
    (7, 8),
    (17, 16),
    (12, 21),
    (12, 3),
    (17, 8),
    (7, 16),
])];

// Focus / DND: crescent moon (two offset arcs forming a crescent).
static FOCUS: &[IconCmd] = &[
    IconCmd::Arc {
        cx: 12,
        cy: 12,
        r: 9,
        a0: 60,
        a1: 300,
    },
    IconCmd::Arc {
        cx: 16,
        cy: 12,
        r: 9,
        a0: 120,
        a1: 240,
    },
];

// Night light / warm sun: a circle with eight short rays.
static NIGHT_LIGHT: &[IconCmd] = &[
    IconCmd::Circle {
        cx: 12,
        cy: 12,
        r: 4,
    },
    IconCmd::Poly(&[(12, 1), (12, 4)]),
    IconCmd::Poly(&[(12, 20), (12, 23)]),
    IconCmd::Poly(&[(1, 12), (4, 12)]),
    IconCmd::Poly(&[(20, 12), (23, 12)]),
    IconCmd::Poly(&[(4, 4), (6, 6)]),
    IconCmd::Poly(&[(18, 18), (20, 20)]),
    IconCmd::Poly(&[(20, 4), (18, 6)]),
    IconCmd::Poly(&[(6, 18), (4, 20)]),
];

// Airplane: paper-plane / send silhouette.
static AIRPLANE: &[IconCmd] = &[
    IconCmd::Closed(&[(2, 12), (22, 3), (15, 21), (11, 14), (2, 12)]),
    IconCmd::Poly(&[(22, 3), (11, 14)]),
];

// Accessibility: head dot + outstretched arms + legs (the universal access glyph).
static ACCESSIBILITY: &[IconCmd] = &[
    IconCmd::Circle {
        cx: 12,
        cy: 12,
        r: 10,
    },
    IconCmd::Dot {
        cx: 12,
        cy: 7,
        r: 1,
    },
    IconCmd::Poly(&[(6, 11), (18, 11)]),
    IconCmd::Poly(&[(12, 9), (12, 15)]),
    IconCmd::Poly(&[(12, 15), (9, 19)]),
    IconCmd::Poly(&[(12, 15), (15, 19)]),
];

// Game controller: rounded body, d-pad, two buttons, grips.
static GAME_CONTROLLER: &[IconCmd] = &[
    IconCmd::RRect {
        x: 2,
        y: 8,
        w: 20,
        h: 10,
        r: 5,
    },
    // d-pad (left)
    IconCmd::Poly(&[(5, 13), (9, 13)]),
    IconCmd::Poly(&[(7, 11), (7, 15)]),
    // buttons (right)
    IconCmd::Dot {
        cx: 16,
        cy: 12,
        r: 1,
    },
    IconCmd::Dot {
        cx: 19,
        cy: 14,
        r: 1,
    },
];

// Palette / RGB: artist palette — an arc body with a thumb notch + paint dots.
static PALETTE: &[IconCmd] = &[
    IconCmd::Arc {
        cx: 12,
        cy: 12,
        r: 10,
        a0: 0,
        a1: 300,
    },
    IconCmd::Poly(&[(12, 22), (16, 18)]),
    IconCmd::Dot { cx: 7, cy: 9, r: 1 },
    IconCmd::Dot {
        cx: 11,
        cy: 6,
        r: 1,
    },
    IconCmd::Dot {
        cx: 16,
        cy: 8,
        r: 1,
    },
    IconCmd::Dot {
        cx: 17,
        cy: 13,
        r: 1,
    },
];

// Performance: a speed gauge — semicircular dial + a needle.
static PERFORMANCE: &[IconCmd] = &[
    IconCmd::Arc {
        cx: 12,
        cy: 16,
        r: 10,
        a0: 180,
        a1: 360,
    },
    IconCmd::Poly(&[(12, 16), (17, 9)]),
    IconCmd::Dot {
        cx: 12,
        cy: 16,
        r: 1,
    },
];

// Folder: classic tab folder outline.
static FOLDER: &[IconCmd] = &[IconCmd::Closed(&[
    (3, 7),
    (3, 19),
    (21, 19),
    (21, 9),
    (12, 9),
    (10, 6),
    (4, 6),
    (3, 7),
])];

// File (generic): page with a folded dog-ear corner.
static FILE: &[IconCmd] = &[
    IconCmd::Closed(&[(6, 3), (15, 3), (19, 7), (19, 21), (6, 21), (6, 3)]),
    IconCmd::Poly(&[(15, 3), (15, 7), (19, 7)]),
];

// Code: page + "</>" angle brackets.
static CODE: &[IconCmd] = &[
    IconCmd::Closed(&[(6, 3), (15, 3), (19, 7), (19, 21), (6, 21), (6, 3)]),
    IconCmd::Poly(&[(15, 3), (15, 7), (19, 7)]),
    IconCmd::Poly(&[(11, 12), (9, 15), (11, 18)]),
    IconCmd::Poly(&[(14, 12), (16, 15), (14, 18)]),
];

// Media / image: framed picture with a sun and a mountain.
static MEDIA: &[IconCmd] = &[
    IconCmd::RRect {
        x: 3,
        y: 5,
        w: 18,
        h: 14,
        r: 2,
    },
    IconCmd::Circle {
        cx: 8,
        cy: 10,
        r: 2,
    },
    IconCmd::Poly(&[(4, 18), (10, 12), (14, 16), (17, 13), (20, 16)]),
];

// Doc: page + horizontal text lines.
static DOC: &[IconCmd] = &[
    IconCmd::Closed(&[(6, 3), (15, 3), (19, 7), (19, 21), (6, 21), (6, 3)]),
    IconCmd::Poly(&[(15, 3), (15, 7), (19, 7)]),
    IconCmd::Poly(&[(9, 12), (16, 12)]),
    IconCmd::Poly(&[(9, 15), (16, 15)]),
    IconCmd::Poly(&[(9, 18), (13, 18)]),
];

// Archive: box with a lid seam and a latch (zip/tar).
static ARCHIVE: &[IconCmd] = &[
    IconCmd::RRect {
        x: 3,
        y: 4,
        w: 18,
        h: 5,
        r: 1,
    },
    IconCmd::Closed(&[(5, 9), (5, 20), (19, 20), (19, 9)]),
    IconCmd::Poly(&[(10, 13), (14, 13)]),
];

// Exec: terminal window with a prompt ">" and cursor.
static EXEC: &[IconCmd] = &[
    IconCmd::RRect {
        x: 3,
        y: 4,
        w: 18,
        h: 16,
        r: 2,
    },
    IconCmd::Poly(&[(7, 10), (10, 13), (7, 16)]),
    IconCmd::Poly(&[(12, 16), (17, 16)]),
];

// Bell: notification bell + clapper.
static BELL: &[IconCmd] = &[
    IconCmd::Poly(&[
        (5, 18),
        (5, 16),
        (7, 14),
        (7, 10),
        (12, 5),
        (17, 10),
        (17, 14),
        (19, 16),
        (19, 18),
        (5, 18),
    ]),
    IconCmd::Arc {
        cx: 12,
        cy: 19,
        r: 2,
        a0: 20,
        a1: 160,
    },
];

// Gear / settings: hex cog + center hub. (Tooth bumps approximated by a notched
// dodecagon so it reads as a gear at small sizes without sub-pixel teeth.)
static GEAR: &[IconCmd] = &[
    IconCmd::Closed(&[
        (12, 2),
        (14, 4),
        (17, 4),
        (18, 7),
        (21, 9),
        (20, 12),
        (21, 15),
        (18, 17),
        (17, 20),
        (14, 20),
        (12, 22),
        (10, 20),
        (7, 20),
        (6, 17),
        (3, 15),
        (4, 12),
        (3, 9),
        (6, 7),
        (7, 4),
        (10, 4),
    ]),
    IconCmd::Circle {
        cx: 12,
        cy: 12,
        r: 4,
    },
];

// Search: magnifier loupe + handle.
static SEARCH: &[IconCmd] = &[
    IconCmd::Circle {
        cx: 10,
        cy: 10,
        r: 7,
    },
    IconCmd::Poly(&[(15, 15), (21, 21)]),
];

// Close: X.
static CLOSE: &[IconCmd] = &[
    IconCmd::Poly(&[(5, 5), (19, 19)]),
    IconCmd::Poly(&[(19, 5), (5, 19)]),
];

// Chevron (right).
static CHEVRON: &[IconCmd] = &[IconCmd::Poly(&[(9, 5), (16, 12), (9, 19)])];

// Plus.
static PLUS: &[IconCmd] = &[
    IconCmd::Poly(&[(12, 5), (12, 19)]),
    IconCmd::Poly(&[(5, 12), (19, 12)]),
];

// Check.
static CHECK: &[IconCmd] = &[IconCmd::Poly(&[(5, 13), (10, 18), (19, 6)])];

// ── Control Center / media canonical icons (from CC inline vectors) ──

// Power (IEC 5009): a ring with a break at the top + a vertical stroke through
// it. Arc angles are screen-CW (0°=right, 90°=down, 270°=up); we run from just
// past the top (300°) clockwise the long way back to just before the top (240°),
// leaving a gap centered on 270° (up). The vertical line pierces that gap.
static POWER: &[IconCmd] = &[
    IconCmd::Arc {
        cx: 12,
        cy: 13,
        r: 8,
        a0: 300,
        a1: 600, // 600 mod 360 = 240; wraps through 0/90/180 → gap at top
    },
    IconCmd::Poly(&[(12, 3), (12, 12)]),
];

// Volume / speaker: a cone (small box + flared triangle) + two sound-wave arcs
// opening to the right.
static VOLUME: &[IconCmd] = &[
    IconCmd::Closed(&[(3, 10), (7, 10), (12, 5), (12, 19), (7, 14), (3, 14)]),
    IconCmd::Arc {
        cx: 12,
        cy: 12,
        r: 5,
        a0: 305,
        a1: 415, // 415 mod 360 = 55 → right-opening arc
    },
    IconCmd::Arc {
        cx: 12,
        cy: 12,
        r: 9,
        a0: 315,
        a1: 405, // → wider right-opening arc
    },
];

// Volume muted: the same cone + an X where the waves were.
static VOLUME_MUTED: &[IconCmd] = &[
    IconCmd::Closed(&[(3, 10), (7, 10), (12, 5), (12, 19), (7, 14), (3, 14)]),
    IconCmd::Poly(&[(16, 9), (22, 15)]),
    IconCmd::Poly(&[(22, 9), (16, 15)]),
];

// Brightness / sun: a center circle + 8 rays. Distinct from NightLight (which
// uses a smaller circle + open-ended Poly rays); this is a balanced cool sun.
static BRIGHTNESS: &[IconCmd] = &[
    IconCmd::Circle {
        cx: 12,
        cy: 12,
        r: 5,
    },
    IconCmd::Poly(&[(12, 1), (12, 3)]),
    IconCmd::Poly(&[(12, 21), (12, 23)]),
    IconCmd::Poly(&[(1, 12), (3, 12)]),
    IconCmd::Poly(&[(21, 12), (23, 12)]),
    IconCmd::Poly(&[(4, 4), (5, 5)]),
    IconCmd::Poly(&[(19, 19), (20, 20)]),
    IconCmd::Poly(&[(20, 4), (19, 5)]),
    IconCmd::Poly(&[(5, 19), (4, 20)]),
];

// Signal bars: four ascending bars (a full-strength meter). Each bar is a short
// vertical stroke; heights step up left→right.
static SIGNAL_BARS: &[IconCmd] = &[
    IconCmd::Poly(&[(5, 19), (5, 16)]),
    IconCmd::Poly(&[(10, 19), (10, 12)]),
    IconCmd::Poly(&[(15, 19), (15, 8)]),
    IconCmd::Poly(&[(20, 19), (20, 4)]),
];

// Lock (padlock): body rounded-rect + a shackle arc above. Arc opens downward
// (top half of a circle): 180°→360° spans left→top→right (the inverted-U).
static LOCK: &[IconCmd] = &[
    IconCmd::RRect {
        x: 5,
        y: 11,
        w: 14,
        h: 10,
        r: 2,
    },
    IconCmd::Arc {
        cx: 12,
        cy: 11,
        r: 4,
        a0: 180,
        a1: 360, // upper semicircle → shackle
    },
];

// Play: right-pointing filled triangle (drawn as a closed stroked triangle so it
// matches the line-art set; reads as Play at tile size).
static PLAY: &[IconCmd] = &[IconCmd::Closed(&[(7, 5), (7, 19), (19, 12)])];

// Pause: two vertical bars.
static PAUSE: &[IconCmd] = &[
    IconCmd::Poly(&[(9, 5), (9, 19)]),
    IconCmd::Poly(&[(15, 5), (15, 19)]),
];

// Skip-prev: a leading bar + a left-pointing triangle (|◀).
static SKIP_PREV: &[IconCmd] = &[
    IconCmd::Poly(&[(6, 6), (6, 18)]),
    IconCmd::Closed(&[(18, 6), (18, 18), (9, 12)]),
];

// Skip-next: a right-pointing triangle + a trailing bar (▶|).
static SKIP_NEXT: &[IconCmd] = &[
    IconCmd::Closed(&[(6, 6), (6, 18), (15, 12)]),
    IconCmd::Poly(&[(18, 6), (18, 18)]),
];

// Chevron-up (^).
static CHEVRON_UP: &[IconCmd] = &[IconCmd::Poly(&[(5, 16), (12, 9), (19, 16)])];

// Chevron-down (v).
static CHEVRON_DOWN: &[IconCmd] = &[IconCmd::Poly(&[(5, 9), (12, 16), (19, 9)])];

// Chevron-left (<).
static CHEVRON_LEFT: &[IconCmd] = &[IconCmd::Poly(&[(15, 5), (8, 12), (15, 19)])];

/// The Rae mark — the OS identity glyph (Start button, Rae-key keycap, boot
/// splash): a diamond ("cut-gem" silhouette) holding a centered orb. Reads at
/// 16px (taskbar) and scales to keycap/splash sizes.
static RAE_LOGO: &[IconCmd] = &[
    IconCmd::Closed(&[(12, 2), (22, 12), (12, 22), (2, 12)]),
    IconCmd::Dot {
        cx: 12,
        cy: 12,
        r: 3,
    },
];

/// SOLID folder (macOS register, IDENTITY-OBSIDIAN.md §4): a filled back tab
/// peeking above a filled front body — one tint, reads as a solid colored
/// folder at any size.
static FOLDER_SOLID: &[IconCmd] = &[
    IconCmd::FilledRRect {
        x: 2,
        y: 5,
        w: 10,
        h: 6,
        r: 2,
    },
    IconCmd::FilledRRect {
        x: 2,
        y: 8,
        w: 20,
        h: 12,
        r: 2,
    },
];

// ════════════════════════════════════════════════════════════════════════════
// Rasterizer — anti-aliased stroked vectors at any scale.
// Pure integer (Q8 fixed point) + integer sqrt; no floats, no_std-safe.
// ════════════════════════════════════════════════════════════════════════════

const Q: i64 = 256; // fixed-point scale (8 fractional bits)

/// Integer square root of a non-negative `i64` (floor). Newton's method.
#[inline]
fn isqrt(n: i64) -> i64 {
    if n <= 0 {
        return 0;
    }
    let mut x = n;
    let mut y = (x + 1) / 2;
    while y < x {
        x = y;
        y = (x + n / x) / 2;
    }
    x
}

/// 256-entry quarter sine table in Q8 (sin(θ)·256 for θ = 0..90°). Built once;
/// used by `arc`/`circle` so we never need a float trig call. Index = degrees.
fn sin_q8(deg: i32) -> i64 {
    // Reduce to [0,360).
    let d = ((deg % 360) + 360) % 360;
    // sin via quarter-symmetry off a small 0..=90 table (integer-built).
    let q = |a: i32| -> i64 {
        // sin(a) for a in 0..=90, Q8, via the isqrt-free CORDIC-ish poly is
        // overkill; use a tiny fixed table at 0,15,30,...,90 with linear interp.
        // Values are round(sin(a°)*256).
        const T: [i64; 7] = [0, 66, 128, 181, 222, 247, 256]; // 0,15,30,45,60,75,90
        let a = a.clamp(0, 90) as i64;
        let seg = a / 15; // 0..6
        let frac = a % 15; // 0..14
        if seg >= 6 {
            return T[6];
        }
        let lo = T[seg as usize];
        let hi = T[seg as usize + 1];
        lo + (hi - lo) * frac / 15
    };
    if d <= 90 {
        q(d)
    } else if d <= 180 {
        q(180 - d)
    } else if d <= 270 {
        -q(d - 180)
    } else {
        -q(360 - d)
    }
}

#[inline]
fn cos_q8(deg: i32) -> i64 {
    sin_q8(deg + 90)
}

/// AA half-width of the feathered stroke edge, in Q8 (≈ 0.7px each side).
const AA: i64 = 180;

/// Squared distance (Q8²-scaled fixed point already cancels) from point `p`
/// to the segment `a—b`, returned as a *true* distance in Q8 units.
#[inline]
fn dist_to_seg_q8(px: i64, py: i64, ax: i64, ay: i64, bx: i64, by: i64) -> i64 {
    let vx = bx - ax;
    let vy = by - ay;
    let wx = px - ax;
    let wy = py - ay;
    let c1 = vx * wx + vy * wy;
    if c1 <= 0 {
        let dx = px - ax;
        let dy = py - ay;
        return isqrt(dx * dx + dy * dy);
    }
    let c2 = vx * vx + vy * vy;
    if c2 <= c1 {
        let dx = px - bx;
        let dy = py - by;
        return isqrt(dx * dx + dy * dy);
    }
    // projection t = c1/c2 in [0,1]; closest point = a + t·v.
    let projx = ax + vx * c1 / c2;
    let projy = ay + vy * c1 / c2;
    let dx = px - projx;
    let dy = py - projy;
    isqrt(dx * dx + dy * dy)
}

/// A flattened stroke is a list of segments (in Q8 screen-space) sharing one
/// half-width; we rasterize a single tight bounding box once per icon so
/// overlapping joins anti-alias cleanly (no double-blended seams).
struct StrokeBatch {
    segs: alloc::vec::Vec<(i64, i64, i64, i64)>, // (ax,ay,bx,by) Q8
    dots: alloc::vec::Vec<(i64, i64, i64)>,      // (cx,cy,r) Q8 filled
    hw: i64,                                     // stroke half-width, Q8
}

impl StrokeBatch {
    fn new(hw: i64) -> Self {
        StrokeBatch {
            segs: alloc::vec::Vec::new(),
            dots: alloc::vec::Vec::new(),
            hw,
        }
    }

    fn line(&mut self, ax: i64, ay: i64, bx: i64, by: i64) {
        self.segs.push((ax, ay, bx, by));
    }

    /// Flatten an arc (degrees, screen CW) into segments.
    fn arc(&mut self, cx: i64, cy: i64, r: i64, a0: i32, a1: i32) {
        let (lo, hi) = if a1 >= a0 { (a0, a1) } else { (a1, a0) };
        let steps = ((hi - lo) / 8).max(2); // ~8° per chord
        let mut prev: Option<(i64, i64)> = None;
        for i in 0..=steps {
            let ang = lo + (hi - lo) * i / steps;
            let x = cx + r * cos_q8(ang) / Q;
            let y = cy + r * sin_q8(ang) / Q;
            if let Some((px, py)) = prev {
                self.line(px, py, x, y);
            }
            prev = Some((x, y));
        }
    }

    fn circle(&mut self, cx: i64, cy: i64, r: i64) {
        self.arc(cx, cy, r, 0, 360);
    }

    /// Rasterize the whole batch into `canvas`, tinted `color` (alpha respected
    /// as the icon's overall opacity). One pass over a clipped bounding box.
    fn rasterize(&self, canvas: &mut Canvas, color: u32) {
        if self.segs.is_empty() && self.dots.is_empty() {
            return;
        }
        let base_a = {
            let a = (color >> 24) & 0xFF;
            if a == 0 {
                255
            } else {
                a
            }
        } as i64;
        let rgb = color & 0x00FF_FFFF;

        // Bounding box (Q8 -> px), padded by half-width + AA.
        let pad = self.hw + AA + Q;
        let mut minx = i64::MAX;
        let mut miny = i64::MAX;
        let mut maxx = i64::MIN;
        let mut maxy = i64::MIN;
        let mut bump = |x: i64, y: i64, r: i64| {
            minx = minx.min(x - r);
            miny = miny.min(y - r);
            maxx = maxx.max(x + r);
            maxy = maxy.max(y + r);
        };
        for &(ax, ay, bx, by) in &self.segs {
            bump(ax, ay, pad);
            bump(bx, by, pad);
        }
        for &(cx, cy, r) in &self.dots {
            bump(cx, cy, r + AA + Q);
        }
        let x0 = (minx / Q).max(0);
        let y0 = (miny / Q).max(0);
        let x1 = ((maxx / Q) + 1).min(canvas.width() as i64);
        let y1 = ((maxy / Q) + 1).min(canvas.height() as i64);
        if x0 >= x1 || y0 >= y1 {
            return;
        }

        for py in y0..y1 {
            // Pixel center in Q8.
            let qy = py * Q + Q / 2;
            for px in x0..x1 {
                let qx = px * Q + Q / 2;
                // Coverage = max over all strokes (so overlaps don't darken).
                let mut cov: i64 = 0;
                for &(ax, ay, bx, by) in &self.segs {
                    let d = dist_to_seg_q8(qx, qy, ax, ay, bx, by) - self.hw;
                    let c = if d <= 0 {
                        256
                    } else if d >= AA {
                        0
                    } else {
                        256 - d * 256 / AA
                    };
                    if c > cov {
                        cov = c;
                    }
                    if cov >= 256 {
                        break;
                    }
                }
                if cov < 256 {
                    for &(cx, cy, r) in &self.dots {
                        let dx = qx - cx;
                        let dy = qy - cy;
                        let d = isqrt(dx * dx + dy * dy) - r;
                        let c = if d <= 0 {
                            256
                        } else if d >= AA {
                            0
                        } else {
                            256 - d * 256 / AA
                        };
                        if c > cov {
                            cov = c;
                        }
                    }
                }
                if cov <= 0 {
                    continue;
                }
                let cov = cov.min(256);
                let a = (base_a * cov / 256) as u32;
                if a == 0 {
                    continue;
                }
                canvas.blend_pixel(px as usize, py as usize, (a << 24) | rgb);
            }
        }
    }
}

/// Build the Q8 stroke batch for `icon` at the given pixel size, offset to
/// `(ox, oy)`. Stroke width scales with size (Feather's ~2/24 ratio) with a 1px
/// floor so it never disappears at tiny tile sizes.
fn build_batch(icon: Icon, ox: i32, oy: i32, size: i32) -> StrokeBatch {
    let s = size.max(1) as i64;
    // grid unit -> Q8 screen: (g * size / GRID + offset), kept in Q8.
    let map = |g: i32| -> i64 { g as i64 * s * Q / GRID as i64 };
    let mx = |g: i32| -> i64 { ox as i64 * Q + map(g) };
    let my = |g: i32| -> i64 { oy as i64 * Q + map(g) };
    // stroke half-width: ~2px on a 24px icon, scaled, ≥0.9px.
    let stroke = (s * Q * 2 / GRID as i64).max(Q * 9 / 10);
    let mut b = StrokeBatch::new(stroke / 2);

    for cmd in icon.commands() {
        match *cmd {
            IconCmd::Poly(pts) => {
                for w in pts.windows(2) {
                    b.line(mx(w[0].0), my(w[0].1), mx(w[1].0), my(w[1].1));
                }
            }
            IconCmd::Closed(pts) => {
                for w in pts.windows(2) {
                    b.line(mx(w[0].0), my(w[0].1), mx(w[1].0), my(w[1].1));
                }
                if pts.len() >= 2 {
                    let f = pts[0];
                    let l = pts[pts.len() - 1];
                    if f != l {
                        b.line(mx(l.0), my(l.1), mx(f.0), my(f.1));
                    }
                }
            }
            IconCmd::Circle { cx, cy, r } => {
                b.circle(mx(cx), my(cy), map(r));
            }
            IconCmd::Arc { cx, cy, r, a0, a1 } => {
                b.arc(mx(cx), my(cy), map(r), a0, a1);
            }
            IconCmd::RRect { x, y, w, h, r } => {
                let (x0, y0) = (x, y);
                let (x1, y1) = (x + w, y + h);
                // straight edges (inset by r at the corners)
                b.line(mx(x0 + r), my(y0), mx(x1 - r), my(y0)); // top
                b.line(mx(x1), my(y0 + r), mx(x1), my(y1 - r)); // right
                b.line(mx(x1 - r), my(y1), mx(x0 + r), my(y1)); // bottom
                b.line(mx(x0), my(y1 - r), mx(x0), my(y0 + r)); // left
                                                                // corner arcs
                b.arc(mx(x0 + r), my(y0 + r), map(r), 180, 270); // TL
                b.arc(mx(x1 - r), my(y0 + r), map(r), 270, 360); // TR
                b.arc(mx(x1 - r), my(y1 - r), map(r), 0, 90); // BR
                b.arc(mx(x0 + r), my(y1 - r), map(r), 90, 180); // BL
            }
            IconCmd::Dot { cx, cy, r } => {
                b.dots.push((mx(cx), my(cy), map(r).max(Q / 2)));
            }
            // Filled shapes are rendered by `draw_icon` directly (via the
            // Canvas AA fill), not by the stroke batch.
            IconCmd::FilledRRect { .. } => {}
        }
    }
    b
}

/// Draw `icon` at pixel `(ox, oy)`, occupying a `size × size` box, tinted
/// `color` (ARGB; alpha = overall opacity). Crisp anti-aliased line art at any
/// size — NOT a bitmap, NOT a letter glyph.
pub fn draw_icon(canvas: &mut Canvas, icon: Icon, ox: i32, oy: i32, size: i32, color: u32) {
    // Filled shapes first (solid silhouettes), then the stroke batch details
    // on top — the two share the tint so a single-color caller still works.
    let s = size.max(1) as i64;
    for cmd in icon.commands() {
        if let IconCmd::FilledRRect { x, y, w, h, r } = *cmd {
            // grid → pixels, rounded to nearest (fills are ≥3 grid units, so
            // sub-pixel rounding is invisible at ≥14px icon sizes).
            let px = ox as i64 + (x as i64 * s + GRID as i64 / 2) / GRID as i64;
            let py = oy as i64 + (y as i64 * s + GRID as i64 / 2) / GRID as i64;
            let pw = (w as i64 * s + GRID as i64 / 2) / GRID as i64;
            let ph = (h as i64 * s + GRID as i64 / 2) / GRID as i64;
            let pr = (r as i64 * s / GRID as i64).max(1);
            if pw > 0 && ph > 0 && px >= 0 && py >= 0 {
                canvas.fill_rounded_rect(
                    px as usize,
                    py as usize,
                    pw as usize,
                    ph as usize,
                    pr as usize,
                    color,
                );
            }
        }
    }
    let batch = build_batch(icon, ox, oy, size);
    batch.rasterize(canvas, color);
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn make(buf: &mut alloc::vec::Vec<u8>, w: usize, h: usize) -> Canvas {
        *buf = vec![0u8; w * h * 4];
        unsafe { Canvas::new(buf.as_mut_ptr(), w, h, 4) }
    }

    /// Count painted (non-zero) pixels in the buffer.
    fn ink(buf: &[u8], w: usize, h: usize) -> usize {
        let px = unsafe { core::slice::from_raw_parts(buf.as_ptr() as *const u32, w * h) };
        px.iter().filter(|&&p| p != 0).count()
    }

    /// Every icon must paint NON-TRIVIAL ink — proving it is a real glyph, not
    /// empty and not the 8x8 letter fallback. FAIL-able: an icon whose command
    /// list produced nothing (or a degenerate dot) trips the floor. Also asserts
    /// the ink is spread out (a real shape), not a single blob.
    #[test]
    fn every_icon_paints_real_ink() {
        let (w, h) = (64usize, 64usize);
        for icon in Icon::ALL {
            let mut buf = alloc::vec::Vec::new();
            let mut c = make(&mut buf, w, h);
            draw_icon(&mut c, icon, 4, 4, 56, 0xFF_FF_FF_FF);
            let painted = ink(&buf, w, h);
            // A 56px line icon has at least a few dozen px of stroke; an empty
            // command list or a no-op primitive would fail here.
            assert!(
                painted >= 40,
                "icon {} painted only {} px (empty/degenerate?)",
                icon.name(),
                painted
            );
            // Spread check: ink must span > 1/4 of the box in both axes (a real
            // shape), ruling out a stray single dot masquerading as an icon.
            let px = unsafe { core::slice::from_raw_parts(buf.as_ptr() as *const u32, w * h) };
            let (mut minx, mut maxx, mut miny, mut maxy) = (w, 0usize, h, 0usize);
            for y in 0..h {
                for x in 0..w {
                    if px[y * w + x] != 0 {
                        minx = minx.min(x);
                        maxx = maxx.max(x);
                        miny = miny.min(y);
                        maxy = maxy.max(y);
                    }
                }
            }
            assert!(
                maxx - minx >= 14 && maxy - miny >= 14,
                "icon {} ink not spread (bbox {}x{})",
                icon.name(),
                maxx - minx,
                maxy - miny
            );
        }
    }

    /// Icons must be ANTI-ALIASED, not 1px aliased lines: a stroked icon at a
    /// real size produces intermediate-alpha edge pixels (the crisp-not-blocky
    /// property). FAIL-able against a hypothetical aliased rasterizer.
    #[test]
    fn icons_are_antialiased() {
        let (w, h) = (64usize, 64usize);
        let mut buf = alloc::vec::Vec::new();
        let mut c = make(&mut buf, w, h);
        // Close (an X) — long diagonals guarantee AA edge grays.
        draw_icon(&mut c, Icon::Close, 4, 4, 56, 0xFF_FF_FF_FF);
        let px = unsafe { core::slice::from_raw_parts(buf.as_ptr() as *const u32, w * h) };
        let mut partials = 0usize;
        for &p in px {
            let r = (p >> 16) & 0xFF;
            if r > 0 && r < 255 {
                partials += 1;
            }
        }
        assert!(partials > 10, "expected AA edge grays, got {}", partials);
    }

    /// Tint is honored: the painted ink takes the requested hue, not white.
    #[test]
    fn icon_tints_with_color() {
        let (w, h) = (48usize, 48usize);
        let mut buf = alloc::vec::Vec::new();
        let mut c = make(&mut buf, w, h);
        draw_icon(&mut c, Icon::Check, 4, 4, 40, 0xFF_4E_9C_FF); // RaeBlue
        let px = unsafe { core::slice::from_raw_parts(buf.as_ptr() as *const u32, w * h) };
        // Find the most opaque pixel; it must read blue-dominant (b > r).
        let mut best = 0u32;
        let mut best_a = 0u32;
        for &p in px {
            let a = (p >> 24) & 0xFF;
            if a > best_a {
                best_a = a;
                best = p;
            }
        }
        let r = (best >> 16) & 0xFF;
        let b = best & 0xFF;
        assert!(b > r, "tint not applied: pixel {:08X}", best);
    }

    /// Scales crisply: the same icon at 16px and 96px both paint, and the large
    /// one has proportionally more ink — proving vector scaling (not a fixed
    /// bitmap stamped twice).
    #[test]
    fn scales_to_size() {
        let small = {
            let mut buf = alloc::vec::Vec::new();
            let mut c = make(&mut buf, 24, 24);
            draw_icon(&mut c, Icon::Gear, 2, 2, 20, 0xFF_FF_FF_FF);
            ink(&buf, 24, 24)
        };
        let large = {
            let mut buf = alloc::vec::Vec::new();
            let mut c = make(&mut buf, 112, 112);
            draw_icon(&mut c, Icon::Gear, 8, 8, 96, 0xFF_FF_FF_FF);
            ink(&buf, 112, 112)
        };
        assert!(small >= 20, "small gear too sparse: {}", small);
        assert!(
            large > small * 3,
            "large ({}) should dwarf small ({}) — vector scaling",
            large,
            small
        );
    }

    /// id <-> enum round-trips for the whole set (the shell's u16 carrier).
    #[test]
    fn id_roundtrip() {
        for icon in Icon::ALL {
            assert_eq!(Icon::from_id(icon.id()), Some(icon), "{}", icon.name());
        }
        assert_eq!(Icon::from_id(9999), None);
    }
}
