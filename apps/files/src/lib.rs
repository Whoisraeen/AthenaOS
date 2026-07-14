//! RaeenOS File Manager — *"the modern file manager"* (RaeenOS_Concept.md
//! §Windows Pain Points).
//!
//! Standalone userspace ELF launched from the start menu (`exec_path = "files"`).
//! A windowed file browser that rivals Windows 11 Explorer (TABS with
//! back/forward history) and macOS Finder (QUICK LOOK preview, BATCH RENAME) on
//! day one, on top of an **undoable Trash** (delete = a CoW move into a
//! session-home `.Trash` bucket via `SYS_RENAME`; Restore + Empty Trash via
//! `SYS_RENAME`/`SYS_UNLINK`).
//!
//! All decision logic (tab/history model, trash-path arithmetic, batch-rename
//! pattern expansion) lives in the host-KAT'd `rae_files` crate
//! (`cargo test -p rae_files`); this bin is the thin syscall + render shell.
//!
//! `/system/apps` and `/bundled` and the session home list real files via
//! `SYS_READDIR_AT`. Other paths use a lightweight virtual tree until the
//! hierarchical VFS grows those subtrees.

// no_std for the real userspace ELF; std under `cargo test` so any future host
// KAT can link. The host screenshot harness links this crate as a LIBRARY and
// calls `render_preview` + `preview_state_demo` (the syscall-free draw seam) —
// the live ELF entry point lives in the thin `src/main.rs` bin, which calls
// `run()` below.
#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

use rae_diff::unified_diff;

#[allow(unused_imports)]
use raekit;

use rae_files::{batch_rename_target, restore_target, trash_dir_for_home, trash_target, TabSet};
// File-association resolver (rae_mime, committed cf80811) + its persistence
// format (rae_toml). This is the wiring of the "what is this file / what opens
// it" infra into the live Files app — RaeenOS_Concept.md §Windows Pain Points
// "the modern file manager" / the #1 daily-driver parity gap: double-clicking a
// file opens it in the right app, with an "Open With" submenu and a persistent
// "Set as default" override. `resolve` ties content-sniff + extension + the
// `Registry` together; overrides round-trip to `<home>/.config/file_assoc.toml`.
use rae_mime::{resolve, MimeType, Registry, Resolution};
use rae_tokens::{DARK, RAEBLUE};
use rae_zip::{is_safe_path, Archive};
// TAR / gzip extraction (.tar / .tar.gz / .tgz) — the POSIX-world counterpart to
// the rae_zip path. `rae_tar::is_safe_path` is the tar-side zip-slip gate (kept
// fully-qualified to avoid clashing with the rae_zip `is_safe_path` imported
// above); `TarKind` classifies each entry so symlinks/hardlinks are skipped.
use rae_tar::{read_tar, read_tar_gz, TarKind};
use raegfx::text::FontFamily;
use raegfx::Canvas;
use raemedia::png::{decode_png, DecodedImage};
// JPEG decode + EXIF-orientation path (the same Concept §creators/media "show my
// photos" surface as PNG). `decode_jpeg_oriented` returns `jpeg::DecodedImage`,
// a structurally-identical sibling of `png::DecodedImage`; `jpeg_to_canvas_image`
// below bridges it so the EXISTING `blit_image_fit` scale/letterbox/blit path is
// reused unchanged for both formats.
use raemedia::exif::decode_jpeg_oriented;
// Animated-GIF decode (Concept §creators/media: "show my photos" extends to the
// animated images that fill the web/messaging). Quick Look shows a STATIC
// first-frame preview — `decode_gif` returns a fully-composited ARGB8888 buffer
// per frame, structurally identical to a decoded PNG, so frame 0's pixels reuse
// the EXACT `blit_image_fit` scale/letterbox/blit path. `decode_gif` is
// hostile-input safe (returns `Err`, never panics); on failure Quick Look falls
// back to the existing dims/hex summary.
use rae_gif::decode_gif;
// Document/preview open path (the WS4 "open documents" deliverable —
// RaeenOS_Concept.md §Windows Pain Points "the modern file manager"). Dispatch is
// by MAGIC BYTES via `rae_formats::detect` (never the extension): a PDF renders
// its extracted text paginated, a DOCX its paragraphs/headings/tables as text, an
// XLSX as a CSV-style grid (reusing the existing CSV table view), and any image
// (PNG/JPEG/BMP/GIF/WebP) through the unified `rae_image` decoder into the same
// ARGB8888 `blit_image_fit` path photos already use. Every engine is
// hostile-input safe (returns `Err`, never panics) — on any failure the open
// dispatch yields `DocPreview::None` and Quick Look falls back to the existing
// text/hex summary.
use rae_formats::{detect, FileKind};
// Windows-app double-click launch (RaeenOS_Concept.md §Compatibility Strategy:
// "RaeBridge runs Windows apps on day one… apps run naturally"). Activating a
// `.exe` in Files writes its VFS path to the proven RaeBridge handoff channel and
// spawns `raebridge_run`, which loads the PE as its OWN RaeenOS process (the
// per-process isolation proven in commit d5db628). We consume the handoff codec
// directly — `Target::Pe { path }` + `encode_record` — rather than replicating it.
use raebridge::handoff::{self, Target};

// ── Window geometry ─────────────────────────────────────────────────────

const WIN_W: usize = 760;
const WIN_H: usize = 480;
const SURFACE_VIRT: u64 = 0x0000_7A00_0000;

const TITLE_H: usize = 28;
const TABBAR_H: usize = 26;
const TOOLBAR_H: usize = 36;
const BREADCRUMB_H: usize = 24;
const SIDEBAR_W: usize = 160;
const ROW_H: usize = 26;
const STATUS_H: usize = 22;

/// The on-screen origin this window is presented at (`surface_present(sid, 240,
/// 90)` in `_start`). Absolute cursor coordinates from `cursor_pos()` are
/// converted to surface-local space by subtracting this — the compositor honors a
/// non-zero present offset.
const PRESENT_X: i32 = 240;
const PRESENT_Y: i32 = 90;

/// A double-click is two left-button press EDGES on the same row within this
/// window. Wall-clock based (`time_ns`), so it is independent of the main loop's
/// yield cadence. 400 ms matches the desktop double-click default.
const DOUBLE_CLICK_NS: u64 = 400_000_000;

// ── Click hit-testing (single source of truth: draw-rects == hit-rects) ───
//
// The same `geom_*` helpers below compute the rectangle for each interactive
// element; `render` draws at those rects and `build_layout` hit-tests against
// them, so the two can never drift (the invariant `design_proof` enforces).

/// An axis-aligned rectangle in SURFACE-LOCAL coordinates.
#[derive(Clone, Copy)]
struct Rect {
    x: usize,
    y: usize,
    w: usize,
    h: usize,
}

impl Rect {
    /// True iff the surface-local point `(px, py)` lies inside this rect.
    fn contains(&self, px: i32, py: i32) -> bool {
        px >= self.x as i32
            && px < (self.x + self.w) as i32
            && py >= self.y as i32
            && py < (self.y + self.h) as i32
    }
    fn center(&self) -> (i32, i32) {
        ((self.x + self.w / 2) as i32, (self.y + self.h / 2) as i32)
    }
}

/// What a click on an interactive element does — each variant maps to the EXACT
/// same `App` method the corresponding key fires, so mouse and keyboard share
/// one behavior. Row clicks carry the ACTUAL entry index (not the visible-row
/// offset). All payloads are `Copy`, never a borrowed label.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Action {
    /// Single-click a list row → select that entry.
    SelectRow(usize),
    /// A row's intent under a DOUBLE-click → open (dir = navigate, file = Quick
    /// Look), mirroring Enter/Space.
    OpenRow(usize),
    /// Click a tab → make it active (mirrors 't').
    SwitchTab(usize),
    /// Click "+" → open a new tab (mirrors Shift+t).
    NewTab,
    /// Click the title-bar close glyph → exit (mirrors Esc).
    CloseApp,
    GoBack,
    GoForward,
    GoUp,
    NewFolder,
    OpenRename,
    TrashSelected,
    /// Click a sidebar Quick-Access row → navigate there (mirrors keys 1–7).
    QuickAccess(usize),
}

/// One interactive element: its surface-local rect and the action a click on it
/// dispatches. Built once per frame into a fixed array (no allocation), then
/// hit-tested top-to-bottom.
#[derive(Clone, Copy)]
struct Element {
    rect: Rect,
    action: Action,
}

/// Max interactive elements: close + "+" + 8 toolbar/tab area + up to 8 tabs +
/// 8 quick-access + MAX_ENTRIES rows. Sized generously and never overrun.
const MAX_ELEMENTS: usize = 2 + 8 + 16 + 8 + MAX_ENTRIES;

/// A fixed-capacity, allocation-free element list: the single source of truth
/// for a frame's interactive layout — filled once, hit-tested against.
struct Layout {
    items: [Element; MAX_ELEMENTS],
    count: usize,
}

impl Layout {
    fn new() -> Self {
        Self {
            items: [Element {
                rect: Rect {
                    x: 0,
                    y: 0,
                    w: 0,
                    h: 0,
                },
                action: Action::GoUp,
            }; MAX_ELEMENTS],
            count: 0,
        }
    }
    fn push(&mut self, rect: Rect, action: Action) {
        if self.count < MAX_ELEMENTS {
            self.items[self.count] = Element { rect, action };
            self.count += 1;
        }
    }
    fn as_slice(&self) -> &[Element] {
        &self.items[..self.count]
    }
    /// Hit-test a surface-local point; returns the action of the FIRST element
    /// whose rect contains it (elements are non-overlapping), or `None` when the
    /// click missed empty space (a no-op, never panics).
    fn hit(&self, px: i32, py: i32) -> Option<Action> {
        self.as_slice()
            .iter()
            .find(|e| e.rect.contains(px, py))
            .map(|e| e.action)
    }
}

// ── Geometry helpers (used by BOTH render and build_layout) ───────────────

/// Title-bar close glyph rect.
const fn geom_close() -> Rect {
    Rect {
        x: WIN_W - 28,
        y: 4,
        w: 20,
        h: 20,
    }
}

/// The "+" new-tab affordance rect — docked right after the LAST tab (the
/// Win11 tab-strip register). Floating alone at the window's far edge it read
/// as a detached mystery control (visual-QA).
fn geom_new_tab(count: usize) -> Rect {
    let tab_w = 120usize;
    let tx = (4 + count.max(1) * (tab_w + 2) + 2).min(WIN_W - 26);
    Rect {
        x: tx,
        y: TITLE_H + 4,
        w: 20,
        h: TABBAR_H - 6,
    }
}

/// Tab `i`'s clickable rect (same geometry `render_tabs_preview` draws). Returns `None`
/// when the tab would overflow the strip (matches the render break condition).
fn geom_tab(i: usize) -> Option<Rect> {
    let tab_w = 120usize;
    let tx = 4 + i * (tab_w + 2);
    if tx + tab_w > WIN_W - 30 {
        return None;
    }
    Some(Rect {
        x: tx,
        y: TITLE_H + 3,
        w: tab_w,
        h: TABBAR_H - 4,
    })
}

/// Y of the toolbar row.
const fn toolbar_y() -> usize {
    TITLE_H + TABBAR_H
}

/// Toolbar button rects, in the exact order `render` draws them:
/// back, forward, Up, New Folder, Rename, Trash.
fn geom_toolbar() -> [(Rect, Action); 6] {
    // MUST mirror `render_preview`'s toolbar layout exactly (26px controls
    // vertically centred in the 36px toolbar; icon nav cluster then labeled
    // action pills).
    let tb_y = toolbar_y();
    let h = 26;
    let y = tb_y + (TOOLBAR_H - h) / 2;
    [
        (Rect { x: 8, y, w: 26, h }, Action::GoBack),
        (Rect { x: 38, y, w: 26, h }, Action::GoForward),
        (Rect { x: 68, y, w: 26, h }, Action::GoUp),
        (
            Rect {
                x: 110,
                y,
                w: 92,
                h,
            },
            Action::NewFolder,
        ),
        (
            Rect {
                x: 208,
                y,
                w: 68,
                h,
            },
            Action::OpenRename,
        ),
        (
            Rect {
                x: 282,
                y,
                w: 58,
                h,
            },
            Action::TrashSelected,
        ),
    ]
}

/// Y where the sidebar / list view begins.
const fn content_y() -> usize {
    toolbar_y() + TOOLBAR_H + BREADCRUMB_H
}

/// Sidebar Quick-Access row `i`'s rect (same geometry the render loop steps).
const fn geom_quick_access(i: usize) -> Rect {
    // MUST mirror the render's Finder-scale rail: 24px rows on a 28px pitch.
    let sy = content_y() + 28 + i * 28;
    Rect {
        x: 4,
        y: sy,
        w: SIDEBAR_W - 8,
        h: 24,
    }
}

/// Number of list rows that fit in the list view.
fn list_rows_visible() -> usize {
    let sb_y = content_y();
    let sb_h = WIN_H - sb_y - STATUS_H;
    sb_h.saturating_sub(22) / ROW_H
}

/// The full-width clickable rect for the `vis`-th VISIBLE row (0 = first row
/// under the column header). Same geometry `render`'s list loop draws.
fn geom_row(vis: usize) -> Rect {
    let lv_x = SIDEBAR_W;
    let lv_y = content_y();
    let lv_w = WIN_W - SIDEBAR_W;
    Rect {
        x: lv_x,
        y: lv_y + 22 + vis * ROW_H,
        w: lv_w,
        h: ROW_H,
    }
}

// ── Palette (rae_tokens, docs/design/design-language.md) ──────────────────
//
// Generic chrome (bg / text / accent / selection / strokes) is pulled onto the
// shared `rae_tokens::DARK` palette + the RaeBlue accent ramp so File Manager
// matches the desktop default 1:1 (whole-OS cohesion, the rae_tokens raison
// d'être). The accent ramp is derived (not const), so accent fills are computed
// in helpers below. File-TYPE icon tints are NOT chrome — they come from the
// fixed `rae_tokens::FTYPE_*` semantic palette (design-language.md §4.4), defined
// in the file-type section further down (replacing the old private folder-yellow).
//
// Live Vibe-Mode tracking: `theme_seed()` reads the desktop's *active* accent
// via `SYS_THEME_GET` (raekit::sys::theme_accent) at launch.

// ── Liquid Glass window chrome (IDENTITY.md §7 — Files = glass.panel) ────────
//
// visual-QA Round 5 §2 (the headline cohesion break): the live Files window was a
// flat dark opaque box (titlebar L20 / toolbar L51 / sidebar L42 / content L45)
// sitting DARKER than the aurora (L72) outside it — the only identity surface that
// received no glass treatment. The fix per IDENTITY §7: frosted-glass chrome
// (titlebar+toolbar = `glass.chrome`, sidebar = `glass.panel`, both composited
// tint→frost over the aurora so the backdrop reads through), a SOLID-but-de-tinted
// content list (lifted off the bluish near-black to a neutral field so ftype icons
// pop, kept opaque for row legibility — Finder/Explorer don't glassify the list),
// and the iridescent perimeter rim around the window edge (`draw_iridescent_rim`,
// the RaeenOS fingerprint). The chrome composites the SAME tint→frost order the
// Control Center uses, so over the aurora the chrome lands at/above the backdrop
// luminance instead of punching a dark hole. No new tokens — every value is a
// `rae_tokens::GLASS_*` tier or palette entry.

/// Window corner radius (radius.lg) — rounds the outer window so the rim + frost
/// read as a floating glass sheet, matching the CC panel.
const WIN_RADIUS: usize = rae_tokens::RADIUS_LG as usize;

/// `glass.chrome` tier — titlebar + toolbar (the most see-through tier; the aurora
/// floats through always-on chrome, IDENTITY §2.1).
const CHROME_TIER: rae_tokens::GlassTier = rae_tokens::GLASS_CHROME_DARK;
/// `glass.panel` tier — the sidebar (the classic Finder/Explorer translucent rail;
/// backdrop bleeds through, IDENTITY §7).
const PANEL_TIER: rae_tokens::GlassTier = rae_tokens::GLASS_PANEL_DARK;

/// Solid, DE-TINTED content field. The old `bg.raised` (#121624, L≈22, strongly
/// BLUE) read as "2015 dark mode" near-black navy; this is a lifted NEUTRAL slate
/// (de-saturated, brighter) so the file list stays solid + legible while no longer
/// reading as a dark hole, and the §4.4 ftype icon tints pop against it. Kept
/// OPAQUE (not glass) on purpose — a dense list over a moving aurora is a
/// legibility nightmare neither Finder nor Explorer accepts (visual-QA §2).
/// OBSIDIAN re-bake (IDENTITY-OBSIDIAN.md §2): the L≈44 neutral slate read as
/// the mid-gray "toy" register against the near-black chrome. The field drops
/// to a deep NEUTRAL (still de-tinted, still a step above `bg.raised`, still
/// opaque — dense lists never sit on a moving backdrop).
const CONTENT_BG: u32 = 0xFF_18_19_1E;
/// Column-header band over the content field — one neutral step above `CONTENT_BG`.
const CONTENT_HDR_BG: u32 = 0xFF_1F_21_26;

/// Rec.601 luma ×1000 (0..=255000) of an opaque ARGB color — the de-tint proof's
/// "lifted off near-black" check. PURE.
const fn content_luma(c: u32) -> u32 {
    let r = (c >> 16) & 0xFF;
    let g = (c >> 8) & 0xFF;
    let b = c & 0xFF;
    299 * r + 587 * g + 114 * b
}

/// True iff `c` is a NEUTRAL field, not a strongly-blue navy: the blue channel
/// must not dominate the red+green average by more than a small margin (the old
/// `bg.raised` #121624 has B=0x24 ≫ R=0x12 — a clear blue cast; the de-tinted
/// `CONTENT_BG` keeps B within ~8 of the R/G mean). FAIL-able: push the content
/// field back toward the bluish navy and B re-dominates → false. PURE.
const fn content_is_detinted(c: u32) -> bool {
    let r = (c >> 16) & 0xFF;
    let g = (c >> 8) & 0xFF;
    let b = c & 0xFF;
    let rg_mean = (r + g) / 2;
    b <= rg_mean + 8
}

const BG: u32 = CONTENT_BG; // window client (solid de-tinted content field)
const TEXT_FG: u32 = DARK.text_primary;
const TEXT_MUTED: u32 = DARK.text_secondary;
const STROKE_HL: u32 = DARK.stroke_strong; // glass top-edge / control border
const OVERLAY_SCRIM: u32 = 0xCC_0A_0B_10; // Quick Look / dialog dimming scrim

/// Alt/hover row tint over the de-tinted content field — a faint neutral lift.
const ROW_HOVER: u32 = 0x0C_FF_FF_FF;

fn theme_seed() -> u32 {
    raekit::sys::theme_accent()
}

/// Accent base, derived through the shared ramp from the live theme seed.
fn accent() -> u32 {
    accent_s(theme_seed())
}

/// Opaque selection fill: the accent's pressed/active shade.
fn row_sel() -> u32 {
    row_sel_s(theme_seed())
}

/// Accent base for an EXPLICIT seed — the pure counterpart of [`accent`]. The
/// live path reads the seed once (via the syscall) and threads it through the
/// draw so the same render is reproducible host-side from a [`FilesViewState`]
/// with no syscall. `accent()` == `accent_s(theme_seed())` by construction.
fn accent_s(seed: u32) -> u32 {
    rae_tokens::derive_accent(seed, &DARK).base
}

/// Opaque selection fill for an EXPLICIT seed — pure counterpart of [`row_sel`].
fn row_sel_s(seed: u32) -> u32 {
    rae_tokens::derive_accent(seed, &DARK).active
}

// ── File-type semantic tint (design-language.md §4.4 `ftype.*`) ───────────
//
// File-type icon color is a FIXED semantic palette (a directory must look like a
// directory in any Vibe preset), so it is sourced from `rae_tokens::FTYPE_*`,
// NOT a private `FOLDER_FG`/`FILE_FG`. Two types intentionally TRACK the accent
// (`dir`/`code` read as "primary") via `ftype_dir`/`ftype_code` — that is the one
// way these tints re-skin with Vibe Mode. The classifier maps an entry to a §4.4
// token; the resolver turns the token into a live ARGB.

/// The §4.4 file-type semantic classes (icon tinting only — not chrome).
#[derive(Clone, Copy, PartialEq, Eq)]
enum FType {
    /// `ftype.dir` — directories (tracks accent).
    Dir,
    /// `ftype.code` — source code (tracks accent).
    Code,
    /// `ftype.exec` — executables.
    Exec,
    /// `ftype.media` — image / video / audio.
    Media,
    /// `ftype.doc` — documents / pdf.
    Doc,
    /// `ftype.archive` — archives.
    Archive,
    /// `ftype.neutral` — plain / unknown.
    Neutral,
}

/// Classify an entry into its §4.4 file-type class by kind + extension
/// (case-insensitive). Directories are always `Dir`; files are routed by their
/// extension into the calmer collapsed media/doc/archive/exec/code buckets, with
/// everything unmatched falling to `Neutral`.
fn classify(e: &DynamicEntry) -> FType {
    if e.kind == Kind::Folder {
        return FType::Dir;
    }
    classify_name(App::entry_name(e))
}

/// Classify a file leaf NAME (no folder check — callers handle directories) into
/// its §4.4 file-type class by extension (case-insensitive). Shared by the file
/// list (`classify`) and the global-search rows (`ftype_for_resolved`) so both
/// surfaces draw the same icon palette. PURE.
fn classify_name(name: &str) -> FType {
    let ext_is = |suf: &str| name_ends_with_ci(name, suf);
    // Media (image / video / audio) — collapsed to one hue per §4.4.
    if ext_is(".png")
        || ext_is(".jpg")
        || ext_is(".jpeg")
        || ext_is(".gif")
        || ext_is(".bmp")
        || ext_is(".webp")
        || ext_is(".svg")
        || ext_is(".mp4")
        || ext_is(".mkv")
        || ext_is(".mov")
        || ext_is(".webm")
        || ext_is(".mp3")
        || ext_is(".wav")
        || ext_is(".flac")
        || ext_is(".ogg")
    {
        return FType::Media;
    }
    // Documents / pdf.
    if ext_is(".pdf")
        || ext_is(".txt")
        || ext_is(".md")
        || ext_is(".doc")
        || ext_is(".docx")
        || ext_is(".odt")
        || ext_is(".rtf")
        || ext_is(".csv")
        || ext_is(".tsv")
    {
        return FType::Doc;
    }
    // Archives.
    if ext_is(".zip")
        || ext_is(".tar")
        || ext_is(".gz")
        || ext_is(".tgz")
        || ext_is(".tar.gz")
        || ext_is(".xz")
        || ext_is(".7z")
        || ext_is(".rar")
    {
        return FType::Archive;
    }
    // Executables.
    if ext_is(".elf") || ext_is(".exe") || ext_is(".app") || ext_is(".sh") {
        return FType::Exec;
    }
    // Source code.
    if ext_is(".rs")
        || ext_is(".c")
        || ext_is(".h")
        || ext_is(".cpp")
        || ext_is(".py")
        || ext_is(".js")
        || ext_is(".ts")
        || ext_is(".toml")
        || ext_is(".json")
        || ext_is(".html")
        || ext_is(".css")
    {
        return FType::Code;
    }
    FType::Neutral
}

// ── Windows .exe double-click → RaeBridge launch ──────────────────────────────

/// The bundled launcher ELF that loads a PE as its own RaeenOS process. Spawned
/// via the SAME app-launch syscall the start menu / other apps use
/// (`raekit::sys::spawn`); it reads [`handoff::HANDOFF_PATH`] at startup to learn
/// which PE to load. Lives in `/bundled` like the other bundled app crates.
const RAEBRIDGE_RUN: &str = "raebridge_run";

/// The fully-decided launch for activating a `.exe`: the exact handoff record the
/// parent must write to [`handoff::HANDOFF_PATH`] and the launcher app-id to spawn
/// afterwards. PURE (no syscalls) so the Files double-click → launch decision is
/// host-KAT'd and FAIL-able off-target — the syscall wrapper
/// ([`App::launch_windows_exe`]) just executes this plan (unlink → write → spawn).
struct ExeLaunch {
    /// Fixed-width handoff record (`handoff::encode_record`) — the parent unlinks
    /// then writes this to [`handoff::HANDOFF_PATH`].
    record: Vec<u8>,
    /// The launcher app-id to spawn after the handoff write ([`RAEBRIDGE_RUN`]).
    spawn: &'static str,
}

/// Decide the RaeBridge launch for activating `path`, IFF it names a `.exe`
/// (case-insensitive). Returns `None` for any non-`.exe` path (so the caller
/// keeps the normal document/image preview-open route) or when the path is too
/// long for the interim handoff record (production length uses `SYS_SPAWN_ARGS`).
/// PURE: builds the `Target::Pe { path }` handoff record via the proven RaeBridge
/// codec; performs no I/O. This is the host-testable core of the double-click flow.
fn exe_launch_plan(path: &str) -> Option<ExeLaunch> {
    if !name_ends_with_ci(path, ".exe") {
        return None;
    }
    let record = handoff::encode_record(&Target::Pe {
        path: path.as_bytes().to_vec(),
    })?;
    Some(ExeLaunch {
        record,
        spawn: RAEBRIDGE_RUN,
    })
}

/// Resolve a §4.4 file-type class to a live ARGB tint from `rae_tokens`. `Dir`
/// and `Code` track the live theme seed (Vibe Mode re-skin); the rest are the
/// fixed semantic hues.
fn ftype_color(ft: FType) -> u32 {
    ftype_color_s(ft, theme_seed())
}

/// Resolve a §4.4 file-type class to a live ARGB tint for an EXPLICIT seed — the
/// pure counterpart of [`ftype_color`]. The accent-tracking classes (`Dir`,
/// `Code`) read from this seed; the fixed semantic hues ignore it.
/// `ftype_color(ft)` == `ftype_color_s(ft, theme_seed())` by construction.
fn ftype_color_s(ft: FType, seed: u32) -> u32 {
    match ft {
        FType::Dir => rae_tokens::ftype_dir(seed, &DARK),
        FType::Code => rae_tokens::ftype_code(seed, &DARK),
        FType::Exec => rae_tokens::FTYPE_EXEC,
        FType::Media => rae_tokens::FTYPE_MEDIA,
        FType::Doc => rae_tokens::FTYPE_DOC,
        FType::Archive => rae_tokens::FTYPE_ARCHIVE,
        FType::Neutral => rae_tokens::ftype_neutral(&DARK),
    }
}

/// The real line-icon for a §4.4 file-type class (`raegfx::icon`). Replaces the
/// old letter/block `ftype_glyph` placeholders the visual-QA Round-2 critique
/// flagged as the worst shipped surface: folder→Folder, code→Code, exec→Exec,
/// media (image/video/audio)→Media, doc/pdf→Doc, archive→Archive, and the
/// generic/unknown leaf→File. Drawn tinted by `ftype_color` so the icon carries
/// the §4.4 palette (dir/code track the live accent). PURE.
fn ftype_icon(ft: FType) -> raegfx::icon::Icon {
    use raegfx::icon::Icon;
    match ft {
        // SOLID blue folder (IDENTITY-OBSIDIAN.md §4 — the macOS content
        // register; the stroked Folder stays for chrome affordances).
        FType::Dir => Icon::FolderSolid,
        FType::Code => Icon::Code,
        FType::Exec => Icon::Exec,
        FType::Media => Icon::Media,
        FType::Doc => Icon::Doc,
        FType::Archive => Icon::Archive,
        FType::Neutral => Icon::File,
    }
}

// ── Static mock VFS (non-home paths only) ────────────────────────────────
//
// Path → entries. The session home + /system/apps + /bundled are REAL
// (SYS_READDIR_AT). These mock subtrees back the system paths the hierarchical
// VFS does not enumerate yet.

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Folder,
    File,
}

#[derive(Clone, Copy)]
struct Entry {
    name: &'static str,
    kind: Kind,
    bytes: u64,
}

/// Sidebar quick-access list: `(label, path, glyph, home_relative)`.
const QUICK_ACCESS: &[(&str, &str, char, bool)] = &[
    ("Home", "", 'H', true),
    ("Documents", "Documents", 'D', true),
    ("Downloads", "Downloads", 'L', true),
    ("Pictures", "Pictures", 'P', true),
    ("Music", "Music", 'M', true),
    ("Videos", "Videos", 'V', true),
    ("Trash", ".Trash", 'T', true),
    ("Apps", "/system/apps", 'A', false),
];

fn list_path(path: &str) -> &'static [Entry] {
    match path {
        "/" => &[
            Entry {
                name: "home",
                kind: Kind::Folder,
                bytes: 0,
            },
            Entry {
                name: "system",
                kind: Kind::Folder,
                bytes: 0,
            },
            Entry {
                name: "tmp",
                kind: Kind::Folder,
                bytes: 0,
            },
            Entry {
                name: "dev",
                kind: Kind::Folder,
                bytes: 0,
            },
            Entry {
                name: "proc",
                kind: Kind::Folder,
                bytes: 0,
            },
            Entry {
                name: "sys",
                kind: Kind::Folder,
                bytes: 0,
            },
        ],
        "/system" => &[
            Entry {
                name: "apps",
                kind: Kind::Folder,
                bytes: 0,
            },
            Entry {
                name: "bundled",
                kind: Kind::Folder,
                bytes: 0,
            },
            Entry {
                name: "kernel",
                kind: Kind::Folder,
                bytes: 0,
            },
            Entry {
                name: "drivers",
                kind: Kind::Folder,
                bytes: 0,
            },
            Entry {
                name: "fonts",
                kind: Kind::Folder,
                bytes: 0,
            },
            Entry {
                name: "themes",
                kind: Kind::Folder,
                bytes: 0,
            },
        ],
        "/system/apps" => &[Entry {
            name: "bundled",
            kind: Kind::Folder,
            bytes: 0,
        }],
        _ => &[],
    }
}

// ── App state ───────────────────────────────────────────────────────────

const PATH_CAP: usize = 256;

#[derive(Clone, Copy)]
struct PathBuf {
    bytes: [u8; PATH_CAP],
    len: usize,
}

impl PathBuf {
    fn new() -> Self {
        Self {
            bytes: [0; PATH_CAP],
            len: 0,
        }
    }
    fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.len]).unwrap_or("/")
    }
    fn set(&mut self, s: &str) {
        let n = s.as_bytes().len().min(PATH_CAP);
        self.bytes[..n].copy_from_slice(&s.as_bytes()[..n]);
        self.len = n;
    }
    fn push_component(&mut self, name: &str) {
        if self.len > 0 && self.bytes[self.len - 1] != b'/' && self.len < PATH_CAP {
            self.bytes[self.len] = b'/';
            self.len += 1;
        }
        for &b in name.as_bytes() {
            if self.len >= PATH_CAP {
                break;
            }
            self.bytes[self.len] = b;
            self.len += 1;
        }
    }
    fn pop_component(&mut self) {
        if self.len <= 1 {
            return;
        }
        if self.bytes[self.len - 1] == b'/' {
            self.len -= 1;
        }
        while self.len > 1 && self.bytes[self.len - 1] != b'/' {
            self.len -= 1;
        }
        if self.len > 1 && self.bytes[self.len - 1] == b'/' {
            self.len -= 1;
        }
        if self.len == 0 {
            self.bytes[0] = b'/';
            self.len = 1;
        }
    }
}

const MAX_ENTRIES: usize = 64;

#[derive(Clone, Copy)]
struct DynamicEntry {
    name: [u8; 48],
    name_len: usize,
    kind: Kind,
    bytes: u64,
    marked: bool, // batch-select flag (space-bar multi-select for rename)
}

/// Transient one-line status message (errors, op results) shown in the status bar.
#[derive(Clone, Copy)]
struct Toast {
    text: [u8; 64],
    len: usize,
}
impl Toast {
    fn empty() -> Self {
        Self {
            text: [0; 64],
            len: 0,
        }
    }
    fn set(&mut self, s: &str) {
        let n = s.as_bytes().len().min(64);
        self.text[..n].copy_from_slice(&s.as_bytes()[..n]);
        self.len = n;
    }
    fn as_str(&self) -> &str {
        core::str::from_utf8(&self.text[..self.len]).unwrap_or("")
    }
    /// Format the extract result into the toast buffer:
    /// "Extracted N files (M skipped: K unsafe, J unsupported)".
    /// When nothing was skipped, the parenthetical is omitted.
    fn set_extract_summary(&mut self, extracted: u32, unsafe_skipped: u32, other_skipped: u32) {
        let mut buf = [0u8; 64];
        let mut n = 0usize;
        let mut put = |s: &[u8], n: &mut usize| {
            for &b in s {
                if *n < buf.len() {
                    buf[*n] = b;
                    *n += 1;
                }
            }
        };
        let mut num = [0u8; 20];
        put(b"Extracted ", &mut n);
        let k = fmt_u64(extracted as u64, &mut num);
        put(&num[..k], &mut n);
        put(b" files", &mut n);
        let skipped = unsafe_skipped + other_skipped;
        if skipped > 0 {
            put(b" (", &mut n);
            let k = fmt_u64(skipped as u64, &mut num);
            put(&num[..k], &mut n);
            put(b" skipped: ", &mut n);
            let k = fmt_u64(unsafe_skipped as u64, &mut num);
            put(&num[..k], &mut n);
            put(b" unsafe, ", &mut n);
            let k = fmt_u64(other_skipped as u64, &mut num);
            put(&num[..k], &mut n);
            put(b" unsupported)", &mut n);
        }
        self.text[..n].copy_from_slice(&buf[..n]);
        self.len = n;
    }

    /// Format the gzip-compress result: "Compressed <name> -> <name>.gz".
    fn set_compress_gz_summary(&mut self, name: &str) {
        let mut buf = [0u8; 64];
        let mut n = 0usize;
        let mut put = |s: &[u8], n: &mut usize| {
            for &b in s {
                if *n < buf.len() {
                    buf[*n] = b;
                    *n += 1;
                }
            }
        };
        put(b"Compressed ", &mut n);
        put(name.as_bytes(), &mut n);
        put(b" -> ", &mut n);
        put(name.as_bytes(), &mut n);
        put(b".gz", &mut n);
        self.text[..n].copy_from_slice(&buf[..n]);
        self.len = n;
    }

    /// Format the zip-compress result: "Compressed N files -> <archive>.zip".
    fn set_compress_zip_summary(&mut self, count: u32, zip_name: &str) {
        let mut buf = [0u8; 64];
        let mut n = 0usize;
        let mut put = |s: &[u8], n: &mut usize| {
            for &b in s {
                if *n < buf.len() {
                    buf[*n] = b;
                    *n += 1;
                }
            }
        };
        let mut num = [0u8; 20];
        put(b"Compressed ", &mut n);
        let k = fmt_u64(count as u64, &mut num);
        put(&num[..k], &mut n);
        put(b" files -> ", &mut n);
        put(zip_name.as_bytes(), &mut n);
        self.text[..n].copy_from_slice(&buf[..n]);
        self.len = n;
    }
}

/// Which modal overlay (if any) is up.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Overlay {
    None,
    QuickLook,
    BatchRename,
    /// Compare two marked text files as a color-coded unified diff.
    Compare,
    /// Compute a file's SHA-256/SHA-1/MD5/CRC32 + verify against a pasted hash.
    Checksum,
    /// "Open With" — pick which app id opens the selected file (rae_mime
    /// candidate list, default bolded/first), and optionally set it as default.
    OpenWith,
    /// Global indexed search — type a query, Enter queries the kernel index via
    /// `raekit::search::query`, and the panel shows the per-kind hit tally.
    Search,
}

const PREVIEW_CAP: usize = 4096;

// ── Host-renderable view model (the syscall/draw seam) ────────────────────────
//
// `FilesViewState` is the COMPLETE input to the main-window draw — everything the
// renderer reads that, on the real OS, comes from a syscall (the live theme
// accent via `SYS_THEME_GET`, the directory listing via `SYS_READDIR_AT`, the
// tab/history model) is captured here as plain owned data. The live app fills it
// from `App` (see `App::view_state`) exactly as before — zero behavior change —
// and a host tool can build one with `preview_state_demo()` and call
// `render_preview` to produce a representative Files window with NO syscalls.
// This is what lets the screenshot harness render the LIVE Files path instead of
// the dead quarantined `raeshell::file_manager` twin.

/// One row in the file list, as the renderer needs it (the syscall-read fields
/// of a `DynamicEntry`, decoupled from the fixed-capacity on-stack layout).
pub struct PreviewEntry {
    /// Display leaf name.
    pub name: String,
    /// `true` for a directory (drawn as the Folder icon, no size column).
    pub is_folder: bool,
    /// File size in bytes (ignored for folders).
    pub bytes: u64,
    /// Batch-select flag (the mark dot + elevated row fill).
    pub marked: bool,
}

/// The complete, syscall-free input to the main-window render. Built from `App`
/// on the live path (`App::view_state`) and from a static mock by
/// [`preview_state_demo`] for host rendering.
pub struct FilesViewState {
    /// Live theme accent seed (live: `SYS_THEME_GET`). Threaded through the draw
    /// so accent/selection/dir+code tints are reproducible host-side.
    pub theme_seed: u32,
    /// Session home, used to resolve the sidebar quick-access active highlight.
    pub home: String,
    /// Current directory (breadcrumb + sidebar active match).
    pub cwd: String,
    /// Back/forward affordance state (toolbar arrow enablement).
    pub can_back: bool,
    pub can_forward: bool,
    /// One full cwd path per open tab; the label is the leaf, computed at draw.
    pub tabs: Vec<String>,
    /// Index of the active tab within `tabs`.
    pub active_tab: usize,
    /// The directory listing, in display order.
    pub entries: Vec<PreviewEntry>,
    /// Highlighted (selected) row index into `entries`.
    pub selected: usize,
    /// First visible row index (windowed scroll).
    pub scroll: usize,
    /// Transient status-bar message; empty = show the item count instead.
    pub toast: String,
    /// Count of marked rows (status-bar tally).
    pub marked_count: usize,
}

/// A realistic mock Files window for host rendering: a Home folder with the
/// canonical user directories (Desktop/Documents/Downloads/Pictures/Music/Videos)
/// plus a few representative files spanning the §4.4 file-type palette (doc /
/// media / code / archive), with plausible sizes. Single tab, default RaeBlue
/// accent, no toast. PURE — no syscall — so the screenshot harness gets a
/// representative, deterministic render of the LIVE Files draw path.
pub fn preview_state_demo() -> FilesViewState {
    fn dir(name: &str) -> PreviewEntry {
        PreviewEntry {
            name: String::from(name),
            is_folder: true,
            bytes: 0,
            marked: false,
        }
    }
    fn file(name: &str, bytes: u64) -> PreviewEntry {
        PreviewEntry {
            name: String::from(name),
            is_folder: false,
            bytes,
            marked: false,
        }
    }
    let mut entries = Vec::new();
    entries.push(dir("Desktop"));
    entries.push(dir("Documents"));
    entries.push(dir("Downloads"));
    entries.push(dir("Pictures"));
    entries.push(dir("Music"));
    entries.push(dir("Videos"));
    entries.push(file("Notes.md", 4_213));
    entries.push(file("vacation.jpg", 2_481_104));
    entries.push(file("main.rs", 18_944));
    entries.push(file("archive.zip", 9_220_096));
    entries.push(file("budget.csv", 1_337));

    let mut tabs = Vec::new();
    tabs.push(String::from("/home/rae"));

    FilesViewState {
        theme_seed: raekit::sys::THEME_DEFAULT_ACCENT,
        home: String::from("/home/rae"),
        cwd: String::from("/home/rae"),
        can_back: false,
        can_forward: false,
        tabs,
        active_tab: 0,
        entries,
        selected: 1,
        scroll: 0,
        toast: String::new(),
        marked_count: 0,
    }
}

struct App {
    tabs: TabSet,
    home: PathBuf,
    selected: usize,
    scroll: usize,
    shift: bool,
    entries: [DynamicEntry; MAX_ENTRIES],
    entry_count: usize,
    overlay: Overlay,
    toast: Toast,
    // Quick Look state
    preview: [u8; PREVIEW_CAP],
    preview_len: usize,
    preview_is_text: bool,
    preview_total: u64,
    // Decoded image for the current Quick Look target (PNG or JPEG → ARGB8888
    // pixels; JPEG is EXIF-oriented so portraits show upright). `None` whenever
    // the selection isn't a decodable image; the overlay then falls back to the
    // dims/hex summary. Heap-backed (raekit allocator), so a large photo doesn't
    // blow the stack-resident `App`.
    preview_image: Option<DecodedImage>,
    // Quick Look CSV/TSV table state. When the selection is a `.csv`/`.tsv` that
    // `rae_csv::parse[_with]` accepted, `preview_csv` holds the parsed (ragged)
    // grid and the overlay renders an aligned table instead of raw text. `None`
    // for everything else (image/text/hex paths are untouched). `preview_csv_tsv`
    // records which delimiter parsed it (for the header label); `preview_csv_scroll`
    // is the first visible DATA row (the header row is pinned).
    preview_csv: Option<rae_csv::Csv>,
    preview_csv_tsv: bool,
    preview_csv_scroll: usize,
    // Quick Look DOCUMENT text. When the selection is a PDF or DOCX that the
    // engine extracted text from, this holds the extracted plain text and the
    // overlay renders it (paginated/scrollable) via the existing text-preview
    // path. `None` for everything else (image/CSV/text paths are untouched).
    // `preview_doc_scroll` is the first visible line; `preview_doc_label` names
    // the source format for the overlay header (e.g. "PDF", "DOCX").
    preview_doc: Option<String>,
    preview_doc_scroll: usize,
    preview_doc_label: &'static str,
    // Batch-rename pattern entry (live-edited buffer).
    pattern: [u8; 48],
    pattern_len: usize,
    // Compare (diff) state. `compare_diff` holds the rendered unified-diff TEXT
    // (heap, so a large diff doesn't grow the stack-resident `App`); it is empty
    // unless the Compare overlay is up. `compare_scroll` is the first visible diff
    // line. The two header buffers carry the leaf names of the compared files.
    compare_diff: Vec<u8>,
    compare_scroll: usize,
    compare_name_a: [u8; 48],
    compare_name_a_len: usize,
    compare_name_b: [u8; 48],
    compare_name_b_len: usize,
    // Checksum / Verify state. When the Checksum overlay is up, these hold the
    // hex digests of the SELECTED file (computed by streaming it in 64 KiB blocks,
    // never loading a multi-GB file whole). `checksum_name` is the leaf name; the
    // four digest strings are heap-backed (a SHA-256 hex is 64 chars). `verify_*`
    // is the user-pasted expected-hash field; on Enter it is matched (algo picked
    // by the pasted hex LENGTH) against the file → `verify_result`.
    checksum_name: [u8; 48],
    checksum_name_len: usize,
    checksum_size: u64,
    checksum_sha256: String,
    checksum_sha1: String,
    checksum_md5: String,
    checksum_crc32: String,
    // Pasted expected hash (the verify field). 64 chars holds a SHA-256 hex; a few
    // bytes of slack for surrounding whitespace the user might paste.
    verify_buf: [u8; 72],
    verify_len: usize,
    verify_result: VerifyResult,
    // ── File-association state (rae_mime) ────────────────────────────────────
    // The default-app registry: the built-in `Registry::with_defaults()` overlaid
    // with the user's persisted overrides from `<home>/.config/file_assoc.toml`
    // (loaded once at launch; a corrupt/missing config silently falls back to the
    // built-ins, never panics). "Set as default" mutates this AND re-persists.
    registry: Registry,
    // "Open With" overlay state, populated by `open_open_with`. `openwith_mime` is
    // the resolved MIME of the target file (the key "Set as default" writes under);
    // `openwith_candidates` is the rae_mime candidate app-id list (default first);
    // `openwith_selected` is the highlighted row. `openwith_name` is the target
    // file's leaf name, shown in the overlay header.
    openwith_mime: MimeType,
    openwith_candidates: Vec<String>,
    openwith_selected: usize,
    openwith_name: [u8; 48],
    openwith_name_len: usize,
    // ── Global indexed search (raekit::search → kernel search_index) ──────────
    // The Files search box is a *global* find (Finder/Explorer "search this PC"),
    // NOT just an in-folder filter: it queries the SAME kernel index the command
    // palette uses via `raekit::search::query_resolved` (SYS_SEARCH_QUERY_RESOLVED,
    // 281), which returns each hit's NAME + PATH + is_folder — so the overlay can
    // render named, clickable result rows (no longer a count-only tally). A row is
    // openable: a file routes to the SAME default-open path Enter uses; a folder
    // navigates Files to that path. The per-kind tally is kept as a header summary
    // (derived from the resolved hits) since it's still useful at a glance.
    // `search_buf`/`search_len` is the live-edited query field; `search_results`
    // is the decoded resolved-hit list (kernel-ranked order preserved);
    // `search_selected` is the highlighted row (Up/Down + Enter, mouse click);
    // `search_summary` is the per-kind tally header; `search_ran` distinguishes
    // "haven't queried yet" from "queried, zero results" (→ "No results").
    search_buf: [u8; 64],
    search_len: usize,
    search_results: Vec<raekit::syscalls::search::ResolvedHit>,
    search_selected: usize,
    search_summary: SearchSummary,
    search_ran: bool,
}

/// A truthful tally of one indexed-search result set — the ONLY thing userspace
/// can render today, since `SYS_SEARCH_QUERY` returns just `(id, kind)` (no
/// name/path). `total` is the hit count; the per-kind fields break it down for
/// the result panel. Produced by the pure, host-tested [`summarize_hits`] so the
/// proof can feed a synthetic decoded set and assert the exact rendered tally.
#[derive(Clone, Copy, PartialEq, Eq)]
struct SearchSummary {
    total: usize,
    files: usize,
    apps: usize,
    settings: usize,
    documents: usize,
    other: usize,
}

impl SearchSummary {
    const fn empty() -> Self {
        Self {
            total: 0,
            files: 0,
            apps: 0,
            settings: 0,
            documents: 0,
            other: 0,
        }
    }
}

/// Tally a decoded indexed-search result set into a [`SearchSummary`] by kind.
/// PURE — no syscall — so the FAIL-able proof feeds a synthetic `&[SearchHit]`
/// and asserts the exact per-kind counts the result panel renders. `Contact`
/// folds into `other` (the Files panel does not have a contacts row). A wrong
/// bucket mapping or a miscount flips the proof to `false`.
fn summarize_hits(hits: &[raekit::syscalls::search::SearchHit]) -> SearchSummary {
    use raekit::syscalls::search::Kind as K;
    let mut s = SearchSummary::empty();
    s.total = hits.len();
    for h in hits {
        match h.kind {
            K::File => s.files += 1,
            K::App => s.apps += 1,
            K::Setting => s.settings += 1,
            K::Document => s.documents += 1,
            K::Contact | K::Other => s.other += 1,
        }
    }
    s
}

/// Tally a RESOLVED indexed-search result set into a [`SearchSummary`] by kind —
/// the header summary above the named rows. A folder hit always counts as a file
/// (Finder/Explorer treat directories as files in the result count), regardless
/// of its index kind tag. PURE — no syscall — so the proof asserts the exact
/// per-kind counts the header renders.
fn summarize_resolved(hits: &[raekit::syscalls::search::ResolvedHit]) -> SearchSummary {
    use raekit::syscalls::search::Kind as K;
    let mut s = SearchSummary::empty();
    s.total = hits.len();
    for h in hits {
        if h.is_folder {
            s.files += 1;
            continue;
        }
        match h.kind {
            K::File => s.files += 1,
            K::App => s.apps += 1,
            K::Setting => s.settings += 1,
            K::Document => s.documents += 1,
            K::Contact | K::Other => s.other += 1,
        }
    }
    s
}

/// What activating a search result row does. PURE routing decision — host-tested
/// against synthetic [`ResolvedHit`]s — so the open wiring is provable without a
/// syscall.
#[derive(Clone, PartialEq, Eq, Debug)]
enum SearchOpenRoute {
    /// Navigate Files to this folder path (a folder hit with a usable path).
    NavigateFolder(String),
    /// Open this file via the existing path-based default-open (Quick Look /
    /// association launch). Carries the absolute path.
    OpenFile(String),
    /// Nothing to do — an item with no filesystem path (an app/setting hit, or a
    /// resolver row that carried no path). The caller leaves a graceful no-op.
    NoTarget,
}

/// Decide how to activate a resolved search hit. A folder with a non-empty path
/// navigates; any other hit with a non-empty path opens as a file; an empty path
/// (apps/settings, or a path-less row) is [`SearchOpenRoute::NoTarget`] (never
/// panics, never fabricates a path). PURE — the host proof feeds a file row, a
/// folder row, and a path-less row and asserts the exact route.
fn search_open_route(hit: &raekit::syscalls::search::ResolvedHit) -> SearchOpenRoute {
    if hit.path.is_empty() {
        return SearchOpenRoute::NoTarget;
    }
    if hit.is_folder {
        SearchOpenRoute::NavigateFolder(hit.path.clone())
    } else {
        SearchOpenRoute::OpenFile(hit.path.clone())
    }
}

/// The §4.4 file-type icon class for a resolved search row: a folder is always
/// `Dir`; a file is classified by its path's leaf extension (the SAME `classify`
/// taxonomy the file list uses, so the search rows and the list rows share one
/// icon palette). PURE.
fn ftype_for_resolved(hit: &raekit::syscalls::search::ResolvedHit) -> FType {
    if hit.is_folder {
        return FType::Dir;
    }
    let name = leaf_of(&hit.path);
    classify_name(name)
}

/// The leaf component of a path (`/a/b/c.txt` → `c.txt`). Returns the whole
/// string when there is no separator. PURE.
fn leaf_of(path: &str) -> &str {
    match path.rfind('/') {
        Some(i) => &path[i + 1..],
        None => path,
    }
}

/// Outcome of a paste-to-verify comparison.
#[derive(Clone, Copy, PartialEq, Eq)]
enum VerifyResult {
    /// No expected hash submitted yet (field empty or untested).
    None,
    /// The pasted hash matched the file's digest (algo auto-detected by length).
    Match,
    /// The pasted hash did not match (wrong digest, or an unrecognized length).
    NoMatch,
}

impl App {
    fn default_home() -> PathBuf {
        let mut info = [0u8; 96];
        if raekit::sys::session_info(&mut info).is_some() {
            if let Some(home) = raekit::sys::session_home_from(&info) {
                let mut p = PathBuf::new();
                p.set(home);
                return p;
            }
        }
        let mut p = PathBuf::new();
        p.set("/home/user");
        p
    }

    fn new() -> Self {
        let home = Self::default_home();
        // Best-effort: ensure the Trash bucket exists (idempotent; E_VFS_EXISTS ok).
        if let Some(td) = trash_dir_for_home(home.as_str()) {
            let _ = raekit::sys::mkdir(td.as_str());
        }
        let tabs = TabSet::new(home.as_str()).unwrap_or_else(|_| {
            // PATH_CAP guarantees this never fails for a real home, but never panic.
            TabSet::new("/").expect("root always fits")
        });
        let default_pattern = b"Renamed_###";
        let mut pattern = [0u8; 48];
        pattern[..default_pattern.len()].copy_from_slice(default_pattern);
        let mut app = Self {
            tabs,
            home,
            selected: 0,
            scroll: 0,
            shift: false,
            entries: [DynamicEntry {
                name: [0; 48],
                name_len: 0,
                kind: Kind::File,
                bytes: 0,
                marked: false,
            }; MAX_ENTRIES],
            entry_count: 0,
            overlay: Overlay::None,
            toast: Toast::empty(),
            preview: [0; PREVIEW_CAP],
            preview_len: 0,
            preview_is_text: false,
            preview_total: 0,
            preview_image: None,
            preview_csv: None,
            preview_csv_tsv: false,
            preview_csv_scroll: 0,
            preview_doc: None,
            preview_doc_scroll: 0,
            preview_doc_label: "",
            pattern,
            pattern_len: default_pattern.len(),
            compare_diff: Vec::new(),
            compare_scroll: 0,
            compare_name_a: [0; 48],
            compare_name_a_len: 0,
            compare_name_b: [0; 48],
            compare_name_b_len: 0,
            checksum_name: [0; 48],
            checksum_name_len: 0,
            checksum_size: 0,
            checksum_sha256: String::new(),
            checksum_sha1: String::new(),
            checksum_md5: String::new(),
            checksum_crc32: String::new(),
            verify_buf: [0; 72],
            verify_len: 0,
            verify_result: VerifyResult::None,
            registry: load_registry(home.as_str()),
            openwith_mime: rae_mime::OCTET_STREAM,
            openwith_candidates: Vec::new(),
            openwith_selected: 0,
            openwith_name: [0; 48],
            openwith_name_len: 0,
            search_buf: [0; 64],
            search_len: 0,
            search_results: Vec::new(),
            search_selected: 0,
            search_summary: SearchSummary::empty(),
            search_ran: false,
        };
        app.refresh_entries();
        app
    }

    fn cwd(&self) -> &str {
        self.tabs.active().cwd()
    }

    fn uses_kernel_dir(path: &str) -> bool {
        path == "/"
            || path == "/system/apps"
            || path == "/bundled"
            || path.starts_with("/home/")
            || path == "/data/apps/self"
            || path.starts_with("/data/apps/self/")
    }

    fn refresh_entries(&mut self) {
        self.entry_count = 0;
        // copy cwd to a local buffer to avoid borrow conflicts
        let mut path_buf = [0u8; PATH_CAP];
        let cwd = self.cwd();
        let pn = cwd.as_bytes().len().min(PATH_CAP);
        path_buf[..pn].copy_from_slice(&cwd.as_bytes()[..pn]);
        let path = core::str::from_utf8(&path_buf[..pn]).unwrap_or("/");

        if Self::uses_kernel_dir(path) {
            self.load_readdir_at(path);
        } else {
            for e in list_path(path) {
                if self.entry_count >= MAX_ENTRIES {
                    break;
                }
                let slot = &mut self.entries[self.entry_count];
                let n = e.name.as_bytes().len().min(48);
                slot.name[..n].copy_from_slice(&e.name.as_bytes()[..n]);
                slot.name_len = n;
                slot.kind = e.kind;
                slot.bytes = e.bytes;
                slot.marked = false;
                self.entry_count += 1;
            }
        }
        if self.selected >= self.entry_count {
            self.selected = self.entry_count.saturating_sub(1);
        }
    }

    fn load_readdir_at(&mut self, path: &str) {
        let mut buf = [0u8; 4096];
        let count = raekit::sys::readdir_at(path, &mut buf) as usize;
        let mut off = 0usize;
        for _ in 0..count {
            if off + 6 > buf.len() || self.entry_count >= MAX_ENTRIES {
                break;
            }
            let name_len = u16::from_le_bytes([buf[off], buf[off + 1]]) as usize;
            let size =
                u32::from_le_bytes([buf[off + 2], buf[off + 3], buf[off + 4], buf[off + 5]]) as u64;
            off += 6;
            if off + name_len > buf.len() {
                break;
            }
            let name = &buf[off..off + name_len];
            off += name_len;
            let slot = &mut self.entries[self.entry_count];
            let n = name.len().min(48);
            slot.name[..n].copy_from_slice(&name[..n]);
            slot.name_len = n;
            let is_folder = size == 0 && !name.contains(&b'.');
            slot.kind = if is_folder { Kind::Folder } else { Kind::File };
            slot.bytes = size;
            slot.marked = false;
            self.entry_count += 1;
        }
    }

    fn entries(&self) -> &[DynamicEntry] {
        &self.entries[..self.entry_count]
    }

    /// Snapshot the live state the main-window renderer needs into the owned,
    /// syscall-free [`FilesViewState`]. The theme seed is read ONCE here (the
    /// only syscall on the draw path) and threaded through; everything else is
    /// copied out of the already-populated `App` (entries came from
    /// `SYS_READDIR_AT`, tabs/cwd from the in-memory model). Building this and
    /// handing it to [`render_preview`] is behavior-identical to the old inline
    /// draw — it only moves the syscall read to this one edge.
    fn view_state(&self) -> FilesViewState {
        let mut tabs = Vec::new();
        for i in 0..self.tabs.count() {
            let cwd = self.tabs.get(i).map(|t| t.cwd()).unwrap_or("/");
            tabs.push(String::from(cwd));
        }
        let mut entries = Vec::new();
        for e in self.entries() {
            entries.push(PreviewEntry {
                name: String::from(App::entry_name(e)),
                is_folder: e.kind == Kind::Folder,
                bytes: e.bytes,
                marked: e.marked,
            });
        }
        FilesViewState {
            theme_seed: theme_seed(),
            home: String::from(self.home.as_str()),
            cwd: String::from(self.cwd()),
            can_back: self.tabs.active().can_back(),
            can_forward: self.tabs.active().can_forward(),
            tabs,
            active_tab: self.tabs.active_index(),
            entries,
            selected: self.selected,
            scroll: self.scroll,
            toast: String::from(self.toast.as_str()),
            marked_count: self.marked_count(),
        }
    }

    fn entry_name(e: &DynamicEntry) -> &str {
        core::str::from_utf8(&e.name[..e.name_len]).unwrap_or("?")
    }

    fn marked_count(&self) -> usize {
        self.entries().iter().filter(|e| e.marked).count()
    }

    /// Build the absolute path of an entry in the current dir into `out`.
    fn entry_path(&self, e: &DynamicEntry, out: &mut PathBuf) {
        out.set(self.cwd());
        out.push_component(Self::entry_name(e));
    }

    fn navigate(&mut self, path: &str) {
        let _ = self.tabs.active_mut().navigate(path);
        self.selected = 0;
        self.scroll = 0;
        self.refresh_entries();
    }

    fn navigate_quick_access(&mut self, idx: usize) {
        let Some(&(_, path, _, home_rel)) = QUICK_ACCESS.get(idx) else {
            return;
        };
        let mut p = PathBuf::new();
        if home_rel {
            p = self.home;
            if !path.is_empty() {
                p.push_component(path);
            }
        } else {
            p.set(path);
        }
        self.navigate(p.as_str());
    }

    fn enter_selected(&mut self) {
        let (kind, name_buf, name_len) = {
            let entries = self.entries();
            let Some(e) = entries.get(self.selected) else {
                return;
            };
            let mut buf = [0u8; 48];
            let n = e.name_len.min(48);
            buf[..n].copy_from_slice(&e.name[..n]);
            (e.kind, buf, n)
        };
        if kind == Kind::Folder {
            if let Ok(name) = core::str::from_utf8(&name_buf[..name_len]) {
                let mut p = PathBuf::new();
                p.set(self.cwd());
                p.push_component(name);
                self.navigate(p.as_str());
            }
        } else {
            // Association-driven open: resolve the file (content sniff + extension
            // + the registry) and launch its default app id via the SAME spawn path
            // the start menu uses. Replaces the old "spawn the filename" behavior
            // (which only worked for files that happened to be app names).
            self.open_default_selected();
        }
    }

    // ── File associations (rae_mime) ─────────────────────────────────────────

    /// Resolve the selected file against the registry, sniffing the leading bytes
    /// (so a mislabeled file — a PNG saved as `notes.txt` — still resolves by
    /// content). Returns the full [`Resolution`] (mime + default app + candidate
    /// list). Reads at most the first ~512 bytes; never panics, never hangs on a
    /// directory (callers guard `Kind::File`).
    fn resolve_selected(&self) -> Option<Resolution> {
        let entry = *self.entries().get(self.selected)?;
        if entry.kind == Kind::Folder {
            return None;
        }
        let name = Self::entry_name(&entry);
        // Read the leading bytes for magic sniffing (content wins over the name).
        let mut p = PathBuf::new();
        self.entry_path(&entry, &mut p);
        let mut head = [0u8; 512];
        let mut n = 0usize;
        let fd = raekit::sys::open(p.as_str(), 0);
        if fd != u64::MAX {
            n = raekit::sys::read(fd, &mut head) as usize;
            let _ = raekit::sys::close(fd);
        }
        let magic: Option<&[u8]> = if n > 0 {
            Some(&head[..n.min(512)])
        } else {
            None
        };
        Some(resolve(name, magic, &self.registry))
    }

    /// Open the selected file in its DEFAULT app (the double-click / Enter path).
    /// Falls back to a text Quick Look for unknown/plain content so the user is
    /// never left with a dead double-click. Never panics.
    fn open_default_selected(&mut self) {
        // Windows app double-click: a `.exe` LAUNCHES via RaeBridge (its own
        // RaeenOS process) instead of routing to the doc/image preview path. Only
        // `.exe` is intercepted here; every other type keeps the preview-open flow
        // below. RaeenOS_Concept.md §Compatibility Strategy — "apps run naturally".
        {
            let entry = match self.entries().get(self.selected) {
                Some(e) if e.kind == Kind::File => *e,
                _ => return,
            };
            let mut p = PathBuf::new();
            self.entry_path(&entry, &mut p);
            if let Some(plan) = exe_launch_plan(p.as_str()) {
                self.launch_windows_exe(&plan);
                return;
            }
        }
        match self.resolve_selected() {
            Some(res) => {
                // Unknown content (octet-stream) → keep the old sensible fallback:
                // show it in Quick Look rather than spawning the Files app on top
                // of itself.
                if res.mime == rae_mime::OCTET_STREAM && res.default_app == rae_mime::FALLBACK_APP {
                    self.open_quick_look();
                    return;
                }
                if res.default_app == rae_mime::FALLBACK_APP {
                    // The registry routes this type to Files itself (e.g. an
                    // archive). Show Quick Look instead of recursively spawning.
                    self.open_quick_look();
                    return;
                }
                let _ = raekit::sys::spawn(&res.default_app);
                self.toast.set("Opening…");
            }
            None => self.open_quick_look(),
        }
    }

    /// Execute an [`ExeLaunch`] plan: serialise (UNLINK → WRITE → SPAWN) so the
    /// well-known handoff path is never raced. The unlink is mandatory — the
    /// kernel RAM-FS returns a read-only snapshot for an existing home file, so a
    /// fresh writable inode must be created each launch (`handoff::HANDOFF_PATH`
    /// docstring). After the handoff is in place we spawn the launcher via the
    /// SAME app-launch syscall the start menu uses; `raebridge_run` then loads the
    /// PE as its own RaeenOS process and the session reaps its exit. Never panics;
    /// reports any failure via the status toast.
    fn launch_windows_exe(&mut self, plan: &ExeLaunch) {
        let path = match core::str::from_utf8(handoff::HANDOFF_PATH) {
            Ok(p) => p,
            Err(_) => {
                self.toast.set("Run failed");
                return;
            }
        };
        // RAM-FS quirk: remove any prior handoff so the next open re-creates a
        // writable inode (E_VFS_NOT_FOUND on the first launch is expected/benign).
        let _ = raekit::sys::unlink(path);
        if !write_whole_file(path, &plan.record) {
            self.toast.set("Run failed");
            return;
        }
        let pid = raekit::sys::spawn(plan.spawn);
        if pid == u64::MAX {
            self.toast.set("Run failed");
        } else {
            self.toast.set("Running…");
        }
    }

    /// Open the "Open With" overlay for the selected file: resolve its candidate
    /// app-id list (default first/bolded) and let the user pick one to launch, or
    /// set the highlighted app as the new default. No-op on a folder / no
    /// selection. Never panics.
    fn open_open_with(&mut self) {
        let entry = match self.entries().get(self.selected) {
            Some(e) if e.kind == Kind::File => *e,
            _ => {
                self.toast.set("Open With: select a file");
                return;
            }
        };
        let res = match self.resolve_selected() {
            Some(r) => r,
            None => return,
        };
        let name = Self::entry_name(&entry);
        let nlen = name.as_bytes().len().min(48);
        self.openwith_name[..nlen].copy_from_slice(&name.as_bytes()[..nlen]);
        self.openwith_name_len = nlen;
        self.openwith_mime = res.mime;
        self.openwith_candidates = res.candidates;
        self.openwith_selected = 0;
        self.overlay = Overlay::OpenWith;
    }

    /// Launch the currently-highlighted "Open With" candidate via the SAME spawn
    /// path the default-open uses, then dismiss the overlay. Never panics on an
    /// empty candidate list.
    fn open_with_launch_selected(&mut self) {
        if let Some(app_id) = self.openwith_candidates.get(self.openwith_selected) {
            let _ = raekit::sys::spawn(app_id);
            self.toast.set("Opening…");
        }
        self.close_open_with();
    }

    /// Set the highlighted "Open With" candidate as the default for this file's
    /// MIME type, persisting the registry to `<home>/.config/file_assoc.toml` so
    /// the choice survives relaunch. A failed write leaves the in-memory override
    /// active and reports it (never panics).
    fn open_with_set_default(&mut self) {
        let chosen = match self.openwith_candidates.get(self.openwith_selected) {
            Some(s) => s.clone(),
            None => return,
        };
        // Re-order so the chosen app is the new default AND first candidate, the
        // rest preserved (so the "Open With" list still offers the alternatives).
        let mut cand: Vec<&str> = Vec::with_capacity(self.openwith_candidates.len());
        cand.push(chosen.as_str());
        for c in &self.openwith_candidates {
            if c.as_str() != chosen.as_str() {
                cand.push(c.as_str());
            }
        }
        self.registry
            .set(self.openwith_mime.as_str(), chosen.as_str(), &cand);
        // Reflect the new order in the live overlay + persist.
        self.openwith_candidates = cand.iter().map(|s| String::from(*s)).collect();
        self.openwith_selected = 0;
        if save_registry(self.home.as_str(), &self.registry) {
            self.toast.set("Set as default");
        } else {
            self.toast.set("Default set (not saved)");
        }
    }

    /// Dismiss the "Open With" overlay and release its candidate list.
    fn close_open_with(&mut self) {
        self.overlay = Overlay::None;
        self.openwith_candidates = Vec::new();
        self.openwith_selected = 0;
    }

    // ── Global indexed search (raekit::search → kernel search_index) ──────────

    /// Open the global-search overlay with a fresh, empty query field. This is
    /// the Files counterpart of the command palette: an OS-wide find (not just an
    /// in-folder filter) that hits the same kernel index. No syscall yet — typing
    /// + Enter drives [`run_indexed_search`].
    fn open_search(&mut self) {
        self.overlay = Overlay::Search;
        self.search_len = 0;
        self.search_results = Vec::new();
        self.search_selected = 0;
        self.search_summary = SearchSummary::empty();
        self.search_ran = false;
    }

    /// Dismiss the search overlay and clear its transient state.
    fn close_search(&mut self) {
        self.overlay = Overlay::None;
        self.search_len = 0;
        self.search_results = Vec::new();
        self.search_selected = 0;
        self.search_summary = SearchSummary::empty();
        self.search_ran = false;
    }

    /// The live search query as a `&str` (lossy on the off chance of a non-UTF-8
    /// byte, which `scancode_to_ascii` never produces).
    fn search_query(&self) -> &str {
        core::str::from_utf8(&self.search_buf[..self.search_len]).unwrap_or("")
    }

    /// Append a typed character to the search query (bounded to the buffer).
    fn search_push(&mut self, c: u8) {
        if self.search_len < self.search_buf.len() {
            self.search_buf[self.search_len] = c;
            self.search_len += 1;
        }
    }

    /// Delete the last character of the search query.
    fn search_backspace(&mut self) {
        if self.search_len > 0 {
            self.search_len -= 1;
        }
    }

    /// Run the typed query against the kernel search index via
    /// `raekit::search::query_resolved` (SYS_SEARCH_QUERY_RESOLVED, 281) and store
    /// the NAMED hits (name + path + is_folder) as the live result rows, in the
    /// kernel's ranked order. An empty query clears the result. A pre-crawl /
    /// no-match index returns 0 hits gracefully (`search_ran = true`, empty list →
    /// the panel shows "No results", never an error). Caps the request at 64 hits
    /// (one screenful). The per-kind tally header is derived from the same hits.
    fn run_indexed_search(&mut self) {
        let q = self.search_query();
        if q.is_empty() {
            self.search_results = Vec::new();
            self.search_selected = 0;
            self.search_summary = SearchSummary::empty();
            self.search_ran = false;
            return;
        }
        let hits = raekit::syscalls::search::query_resolved(q, 64);
        self.search_summary = summarize_resolved(&hits);
        self.search_results = hits;
        self.search_selected = 0;
        self.search_ran = true;
    }

    /// Move the search-result highlight by `delta`, clamped to the result list.
    /// No-op when there are no results.
    fn search_move(&mut self, delta: i32) {
        let len = self.search_results.len();
        if len == 0 {
            return;
        }
        let cur = self.search_selected as i32;
        let next = (cur + delta).clamp(0, len as i32 - 1);
        self.search_selected = next as usize;
    }

    /// Open the highlighted search result: a folder navigates Files to that path
    /// (and dismisses the overlay); a file routes to the SAME default-open path
    /// Enter uses on the list (Quick Look / association launch). A path-less hit
    /// (app/setting) is a graceful no-op with a hint. Never panics; reuses the
    /// existing navigate/open code — no new syscall.
    fn search_open_selected(&mut self) {
        let Some(hit) = self.search_results.get(self.search_selected) else {
            return;
        };
        match search_open_route(hit) {
            SearchOpenRoute::NavigateFolder(path) => {
                self.close_search();
                self.navigate(&path);
            }
            SearchOpenRoute::OpenFile(path) => {
                self.close_search();
                self.open_path_default(&path);
            }
            SearchOpenRoute::NoTarget => {
                self.toast.set("No file path for this result");
            }
        }
    }

    /// Open an absolute file PATH in its default app — the path-based counterpart
    /// of [`open_default_selected`] (which works off the selected list entry). The
    /// search rows carry a full path, not a list index, so this resolves the file
    /// by content sniff + extension via the registry and launches the default app
    /// id, falling back to a text Quick Look for unknown/plain content. Reuses the
    /// SAME `resolve` + spawn path the list double-click uses. Never panics.
    fn open_path_default(&mut self, path: &str) {
        let name = leaf_of(path);
        let mut head = [0u8; 512];
        let mut n = 0usize;
        let fd = raekit::sys::open(path, 0);
        if fd != u64::MAX {
            n = raekit::sys::read(fd, &mut head) as usize;
            let _ = raekit::sys::close(fd);
        }
        let magic: Option<&[u8]> = if n > 0 {
            Some(&head[..n.min(512)])
        } else {
            None
        };
        let res = resolve(name, magic, &self.registry);
        // Unknown/plain content, or a type the registry routes back to Files
        // itself → Quick Look the path rather than spawning Files on top of
        // itself. (The list path opens its in-window Quick Look; here we have no
        // list selection, so launch the default text viewer if one is set, else
        // just report — never recurse into Files.)
        if res.default_app == rae_mime::FALLBACK_APP {
            self.toast.set("Opening…");
            return;
        }
        let _ = raekit::sys::spawn(&res.default_app);
        self.toast.set("Opening…");
    }

    /// Move the "Open With" highlight by `delta`, clamped to the candidate list.
    fn open_with_move(&mut self, delta: i32) {
        let len = self.openwith_candidates.len();
        if len == 0 {
            return;
        }
        let cur = self.openwith_selected as i32;
        let next = (cur + delta).clamp(0, len as i32 - 1);
        self.openwith_selected = next as usize;
    }

    fn go_up(&mut self) {
        let mut p = PathBuf::new();
        p.set(self.cwd());
        p.pop_component();
        self.navigate(p.as_str());
    }

    fn go_back(&mut self) {
        let moved = self.tabs.active_mut().back().is_some();
        if moved {
            self.selected = 0;
            self.scroll = 0;
            self.refresh_entries();
        }
    }

    fn go_forward(&mut self) {
        let moved = self.tabs.active_mut().forward().is_some();
        if moved {
            self.selected = 0;
            self.scroll = 0;
            self.refresh_entries();
        }
    }

    fn new_tab(&mut self) {
        let mut home_buf = [0u8; PATH_CAP];
        let h = self.home.as_str();
        let n = h.as_bytes().len().min(PATH_CAP);
        home_buf[..n].copy_from_slice(&h.as_bytes()[..n]);
        let hp = core::str::from_utf8(&home_buf[..n]).unwrap_or("/");
        match self.tabs.open(hp) {
            Ok(_) => {
                self.selected = 0;
                self.scroll = 0;
                self.refresh_entries();
            }
            Err(_) => self.toast.set("Max tabs open"),
        }
    }

    fn close_tab(&mut self) {
        let active = self.tabs.active_index();
        match self.tabs.close(active) {
            Ok(()) => {
                self.selected = 0;
                self.scroll = 0;
                self.refresh_entries();
            }
            Err(_) => self.toast.set("Cannot close last tab"),
        }
    }

    fn next_tab(&mut self) {
        self.tabs.next();
        self.selected = 0;
        self.scroll = 0;
        self.refresh_entries();
    }

    /// Move the selected entry into Trash (a CoW move). Undoable via Restore.
    fn trash_selected(&mut self) {
        let entry = match self.entries().get(self.selected) {
            Some(e) => *e,
            None => return,
        };
        // Compute src + trash target through the host-KAT'd logic.
        let mut src = PathBuf::new();
        self.entry_path(&entry, &mut src);
        match trash_target(src.as_str(), self.home.as_str()) {
            Ok(dst) => match raekit::sys::rename(src.as_str(), dst.as_str()) {
                Ok(()) => {
                    self.toast.set("Moved to Trash");
                    self.refresh_entries();
                }
                Err(raekit::sys::E_VFS_EXISTS) => self.toast.set("Already in Trash (name taken)"),
                Err(_) => self.toast.set("Delete failed"),
            },
            Err(_) => self.toast.set("Cannot trash this item"),
        }
    }

    /// Restore the selected trashed entry back to the home root (best default).
    fn restore_selected(&mut self) {
        let entry = match self.entries().get(self.selected) {
            Some(e) => *e,
            None => return,
        };
        let mut src = PathBuf::new();
        self.entry_path(&entry, &mut src);
        match restore_target(src.as_str(), self.home.as_str(), self.home.as_str()) {
            Ok(dst) => match raekit::sys::rename(src.as_str(), dst.as_str()) {
                Ok(()) => {
                    self.toast.set("Restored to Home");
                    self.refresh_entries();
                }
                Err(raekit::sys::E_VFS_EXISTS) => self.toast.set("Restore: name taken at Home"),
                Err(_) => self.toast.set("Restore failed"),
            },
            Err(_) => self.toast.set("Not in Trash"),
        }
    }

    /// Permanently delete the selected entry (Empty-Trash / hard delete).
    fn delete_selected_forever(&mut self) {
        let entry = match self.entries().get(self.selected) {
            Some(e) => *e,
            None => return,
        };
        let mut src = PathBuf::new();
        self.entry_path(&entry, &mut src);
        match raekit::sys::unlink(src.as_str()) {
            Ok(()) => {
                self.toast.set("Deleted permanently");
                self.refresh_entries();
            }
            Err(raekit::sys::E_VFS_NOT_EMPTY) => self.toast.set("Folder not empty"),
            Err(_) => self.toast.set("Delete failed"),
        }
    }

    fn new_folder(&mut self) {
        // Create a uniquely-named folder in the current dir.
        let mut p = PathBuf::new();
        p.set(self.cwd());
        p.push_component("New Folder");
        match raekit::sys::mkdir(p.as_str()) {
            Ok(()) => {
                self.toast.set("Created New Folder");
                self.refresh_entries();
            }
            Err(raekit::sys::E_VFS_EXISTS) => self.toast.set("New Folder already exists"),
            Err(raekit::sys::E_VFS_READONLY) => self.toast.set("Read-only location"),
            Err(_) => self.toast.set("mkdir failed"),
        }
    }

    // ── Extract here (.zip / .tar / .tar.gz / .tgz) ─────────────────────────
    //
    // RaeenOS_Concept.md §"The user owns the machine": a daily driver lets you
    // double-click a downloaded archive and get its contents without installing
    // a third-party tool (Windows Explorer and macOS Finder both extract natively).
    // `.zip` is the Windows-world shape; `.tar.gz`/`.tgz` is the POSIX-world shape
    // (source releases, toolchains, container layers). The archive logic (parse /
    // inflate / CRC / bomb bounds / zip-slip gate) is the host-KAT'd `rae_zip` and
    // `rae_tar`; this method is the thin syscall shell that drives them, with the
    // per-format `is_safe_path` gate consulted before every write.

    /// True when the selected entry looks like a supported archive (zip or
    /// tar/tar.gz), by magic sniff first then extension — so the `x` key and the
    /// toolbar both know whether Extract here applies.
    fn selected_is_archive(&self) -> bool {
        self.selected_archive_format().is_some()
    }

    /// Detect the selected entry's archive format: gzip-tar (`.tar.gz`/`.tgz`,
    /// gzip magic `1F 8B`), plain tar (ustar magic at offset 257, or `.tar`), or
    /// zip (PK magic, or `.zip`). Magic is the primary signal; the extension is the
    /// fallback for a short/unreadable read. `None` for folders / non-archives.
    fn selected_archive_format(&self) -> Option<ArchiveFormat> {
        let entry = match self.entries().get(self.selected) {
            Some(e) => *e,
            None => return None,
        };
        if entry.kind == Kind::Folder {
            return None;
        }
        let name = Self::entry_name(&entry);

        // Extension fallbacks (case-insensitive), checked compound-first so a
        // `.tar.gz` name routes to the gzip path, not the plain-tar path.
        let ext_gz = name_ends_with_ci(name, ".tar.gz") || name_ends_with_ci(name, ".tgz");
        let ext_tar = name_ends_with_ci(name, ".tar");
        let ext_zip = name_ends_with_ci(name, ".zip");

        // Magic sniff: read enough of the header to see the gzip (offset 0), zip
        // (offset 0), or ustar (offset 257) magic. 264 bytes covers all three.
        let mut p = PathBuf::new();
        self.entry_path(&entry, &mut p);
        let fd = raekit::sys::open(p.as_str(), 0);
        if fd != u64::MAX {
            let mut hdr = [0u8; 264];
            let n = raekit::sys::read(fd, &mut hdr) as usize;
            let _ = raekit::sys::close(fd);
            let hdr = &hdr[..n.min(264)];
            if hdr.len() >= 2 && hdr[0] == 0x1F && hdr[1] == 0x8B {
                return Some(ArchiveFormat::TarGz);
            }
            if hdr.len() >= 4 && hdr[..4] == [b'P', b'K', 0x03, 0x04] {
                return Some(ArchiveFormat::Zip);
            }
            // ustar magic lives at offset 257 ("ustar" then NUL or "  ").
            if hdr.len() >= 262 && &hdr[257..262] == b"ustar" {
                return Some(ArchiveFormat::Tar);
            }
        }

        // No decisive magic → fall back to the extension.
        if ext_gz {
            Some(ArchiveFormat::TarGz)
        } else if ext_tar {
            Some(ArchiveFormat::Tar)
        } else if ext_zip {
            Some(ArchiveFormat::Zip)
        } else {
            None
        }
    }

    /// Extract the selected archive (`.zip` / `.tar` / `.tar.gz` / `.tgz`) into a
    /// sibling `<archive-stem>/` directory, routing by detected format.
    ///
    /// Never panics: a malformed archive shows an error toast and leaves the app
    /// running; per-entry failures (unsafe path / unsupported method / bomb /
    /// bad CRC / symlink) are SKIPPED and tallied, not fatal. The end-of-op toast
    /// reports the counts.
    fn extract_selected(&mut self) {
        let entry = match self.entries().get(self.selected) {
            Some(e) => *e,
            None => return,
        };
        if entry.kind == Kind::Folder {
            self.toast.set("Extract: select an archive");
            return;
        }

        let format = match self.selected_archive_format() {
            Some(f) => f,
            None => {
                self.toast.set("Extract: not an archive");
                return;
            }
        };

        // Read the whole archive (same size cap as the image-decode path).
        let mut src = PathBuf::new();
        self.entry_path(&entry, &mut src);
        let fd = raekit::sys::open(src.as_str(), 0);
        if fd == u64::MAX {
            self.toast.set("Extract: cannot open archive");
            return;
        }
        let bytes = read_whole_file(fd, &[]);
        let _ = raekit::sys::close(fd);
        let bytes = match bytes {
            Some(b) => b,
            None => {
                self.toast.set("Extract: archive too large or empty");
                return;
            }
        };

        // Destination dir = <cwd>/<archive-stem>/ (stem drops BOTH extensions for
        // `foo.tar.gz` → `foo/`). Create it (E_VFS_EXISTS ok).
        let stem = archive_stem(Self::entry_name(&entry));
        let mut dest = PathBuf::new();
        dest.set(self.cwd());
        dest.push_component(stem);
        match raekit::sys::mkdir(dest.as_str()) {
            Ok(()) | Err(raekit::sys::E_VFS_EXISTS) => {}
            Err(raekit::sys::E_VFS_READONLY) => {
                self.toast.set("Extract: read-only location");
                return;
            }
            Err(_) => {
                self.toast.set("Extract: cannot create folder");
                return;
            }
        }

        let result = match format {
            ArchiveFormat::Zip => extract_zip_bytes(&bytes, dest.as_str()),
            ArchiveFormat::Tar => extract_tar_bytes(&bytes, dest.as_str(), false),
            ArchiveFormat::TarGz => extract_tar_bytes(&bytes, dest.as_str(), true),
        };
        match result {
            Some((extracted, unsafe_skipped, other_skipped)) => {
                self.toast
                    .set_extract_summary(extracted, unsafe_skipped, other_skipped);
            }
            None => self.toast.set("Can't extract (corrupt archive)"),
        }
        self.refresh_entries();
    }

    // ── Compress here (.zip / .gz) ──────────────────────────────────────────
    //
    // RaeenOS_Concept.md §"The user owns the machine": the inverse of Extract — a
    // daily driver lets you CREATE an archive (zip a project to email, gzip a log)
    // without a third-party tool (Windows Explorer "Send to > Compressed folder",
    // macOS Finder "Compress"). The compression core (LZ77 + fixed-Huffman DEFLATE,
    // gzip framing, CRC-32) is the host-KAT'd `rae_deflate`; this method is the thin
    // syscall + container shell. The container choice mirrors the platforms:
    //   • exactly ONE marked FILE (no folders) → `<name>.gz` (gzip single stream),
    //   • anything else (2+ items, OR any folder) → `<base>.zip` (multi-member ZIP).
    // FOLDERS are walked RECURSIVELY: compressing `docs/` yields zip entries
    // `docs/readme.md`, `docs/sub/a.txt`, … (each child under the folder name, with
    // explicit `dir/` entries for structural directories, like real zips). Never
    // panics: a read/write failure shows an error toast and leaves the app running;
    // a child that can't be read is skipped + tallied, not fatal.

    /// Compress the marked item(s) into the current directory. If nothing is
    /// marked, the current selection is marked first (single-item convenience,
    /// mirroring batch-rename). Exactly one marked file (no folder) → gzip (`.gz`);
    /// any folder, or two-or-more items → a ZIP. Folders are walked recursively,
    /// their tree rooted under the folder name with paths RELATIVE to the archive
    /// root.
    ///
    /// Never panics: each read goes through the size-capped `read_whole_file`, each
    /// write reports failure as a toast (not a crash), an unreadable child is
    /// skipped + tallied, and an empty selection is a no-op with a message.
    fn compress_marked(&mut self) {
        // Mark the current selection if nothing is marked yet (single-item path).
        if self.marked_count() == 0 {
            if let Some(e) = self.entries.get_mut(self.selected) {
                e.marked = true;
            }
        }

        // Collect ALL marked entries (files AND folders). Snapshot indices into the
        // entries array, mirroring batch-rename.
        let mut targets: [usize; MAX_ENTRIES] = [0; MAX_ENTRIES];
        let mut tcount = 0usize;
        let mut folder_marked = false;
        for (i, e) in self.entries().iter().enumerate() {
            if e.marked {
                if tcount < MAX_ENTRIES {
                    targets[tcount] = i;
                    tcount += 1;
                }
                if e.kind == Kind::Folder {
                    folder_marked = true;
                }
            }
        }
        if tcount == 0 {
            self.toast.set("Compress: select an item");
            // Clear any auto-mark so the UI doesn't leave a stray selection marked.
            for e in self.entries.iter_mut() {
                e.marked = false;
            }
            return;
        }

        let cwd_owned = {
            let mut p = PathBuf::new();
            p.set(self.cwd());
            p
        };

        // Single FILE (no folder) → gzip `<name>.gz` (the single-stream shape).
        if tcount == 1 && !folder_marked {
            let ei = targets[0];
            let mut name_buf = [0u8; 48];
            let nl = self.entries[ei].name_len;
            name_buf[..nl].copy_from_slice(&self.entries[ei].name[..nl]);
            let name = core::str::from_utf8(&name_buf[..nl]).unwrap_or("file");

            let mut src = PathBuf::new();
            src.set(cwd_owned.as_str());
            src.push_component(name);
            let bytes = match read_file_bytes(src.as_str()) {
                Some(b) => b,
                None => {
                    self.toast.set("Compress failed (cannot read)");
                    self.clear_marks_refresh();
                    return;
                }
            };
            let gz = rae_deflate::gzip_compress(&bytes);

            // Destination: `<name>.gz` in the current dir.
            let mut dst = PathBuf::new();
            dst.set(cwd_owned.as_str());
            let mut gz_name = [0u8; 56];
            let gn = build_dot_suffix_name(name, ".gz", &mut gz_name);
            dst.push_component(core::str::from_utf8(&gz_name[..gn]).unwrap_or("file.gz"));

            if write_whole_file(dst.as_str(), &gz) {
                self.toast.set_compress_gz_summary(name);
            } else {
                self.toast.set("Compress failed (write)");
            }
            self.clear_marks_refresh();
            return;
        }

        // Otherwise → build a ZIP `<base>.zip`. Loose marked files are added by
        // basename; marked folders are walked recursively under the folder name.
        // A single shared budget (`Walk`) bounds the whole archive so a pathological
        // tree (or many huge files) can never run away.
        let mut writer = ZipWriter::new();
        let mut walk = Walk::new();
        let mut added = 0u32; // files actually placed in the zip
        for t in 0..tcount {
            if walk.exhausted() {
                break;
            }
            let ei = targets[t];
            let kind = self.entries[ei].kind;
            let mut name_buf = [0u8; 48];
            let nl = self.entries[ei].name_len;
            name_buf[..nl].copy_from_slice(&self.entries[ei].name[..nl]);
            let name = core::str::from_utf8(&name_buf[..nl]).unwrap_or("file");

            let mut src = PathBuf::new();
            src.set(cwd_owned.as_str());
            src.push_component(name);

            if kind == Kind::Folder {
                // Recurse: archive-relative prefix is the folder's own name. The
                // walker adds files as `name/child…` and explicit `name/sub/` dir
                // entries, tallying each placed file into `added`.
                let mut rel = PathBuf::new();
                rel.set(name);
                zip_add_dir_recursive(&mut writer, &src, &rel, 0, &mut walk, &mut added);
            } else if let Some(b) = read_file_bytes(src.as_str()) {
                if walk.charge_file(b.len()) {
                    writer.add_file(name, &b);
                    added += 1;
                }
            }
        }
        if added == 0 {
            self.toast.set("Compress failed (nothing readable)");
            self.clear_marks_refresh();
            return;
        }
        let zip = writer.finish();

        // Destination name from the first marked item's stem.
        let first_ei = targets[0];
        let mut name_buf = [0u8; 48];
        let nl = self.entries[first_ei].name_len;
        name_buf[..nl].copy_from_slice(&self.entries[first_ei].name[..nl]);
        let first_name = core::str::from_utf8(&name_buf[..nl]).unwrap_or("archive");
        let mut zip_name = [0u8; 56];
        let zn = build_zip_basename(first_name, &mut zip_name);
        let zip_name_str = core::str::from_utf8(&zip_name[..zn]).unwrap_or("archive.zip");

        let mut dst = PathBuf::new();
        dst.set(cwd_owned.as_str());
        dst.push_component(zip_name_str);

        if write_whole_file(dst.as_str(), &zip) {
            self.toast.set_compress_zip_summary(added, zip_name_str);
        } else {
            self.toast.set("Compress failed (write)");
        }
        self.clear_marks_refresh();
    }

    /// Clear every mark and reload the directory (so the new archive shows).
    fn clear_marks_refresh(&mut self) {
        for e in self.entries.iter_mut() {
            e.marked = false;
        }
        self.refresh_entries();
    }

    // ── Quick Look ───────────────────────────────────────────────────────

    fn open_quick_look(&mut self) {
        let entry = match self.entries().get(self.selected) {
            Some(e) => *e,
            None => return,
        };
        if entry.kind == Kind::Folder {
            self.toast.set("Quick Look: select a file");
            return;
        }
        let mut p = PathBuf::new();
        self.entry_path(&entry, &mut p);
        self.preview_len = 0;
        self.preview_total = entry.bytes;
        self.preview_is_text = false;
        self.preview_image = None;
        self.preview_csv = None;
        self.preview_csv_tsv = false;
        self.preview_csv_scroll = 0;
        self.preview_doc = None;
        self.preview_doc_scroll = 0;
        self.preview_doc_label = "";

        // CSV/TSV detection by extension (safe + sufficient): a `.csv`/`.tsv` is
        // parsed into an aligned table. The parser is hostile-input safe (returns
        // Err, never panics) and memory-bounded; on ANY Err we leave `preview_csv`
        // None and fall through to the existing plain-text view. We read the whole
        // file (bounded by CSV_READ_CAP) because the table needs all rows, not just
        // the 4 KiB preview chunk.
        let name = App::entry_name(&entry);
        let is_csv = name_ends_with_ci(name, ".csv");
        let is_tsv = name_ends_with_ci(name, ".tsv");
        if is_csv || is_tsv {
            if let Some(text) = read_file_text_capped(p.as_str(), CSV_READ_CAP) {
                let parsed = if is_tsv {
                    rae_csv::parse_with(&text, '\t')
                } else {
                    rae_csv::parse(&text)
                };
                if let Ok(csv) = parsed {
                    if !csv.is_empty() {
                        self.preview_csv = Some(csv);
                        self.preview_csv_tsv = is_tsv;
                    }
                }
            }
        }

        // Document / preview open dispatch (PDF · DOCX · XLSX · image), by MAGIC
        // BYTES not extension. We slurp the file (capped) and run the syscall-free
        // `build_doc_preview` core. A hit routes to the matching render surface:
        // PDF/DOCX → paginated text, XLSX → the CSV table view, an image → the
        // ARGB8888 blit. On a miss (`None`) we fall through to the existing
        // text/image/hex path below — so a `.csv` already set above, or a plain
        // text file, is unaffected. The image branch also satisfies the
        // PNG/JPEG/GIF cases the legacy block below would handle, but via the
        // unified decoder (adds BMP/WebP); a decode failure there still falls
        // through to the legacy attempt + hex summary.
        if self.preview_csv.is_none() {
            if let Some(bytes) = read_whole_file_path(p.as_str(), DOC_DECODE_CAP) {
                match build_doc_preview(&bytes) {
                    DocPreview::Text { label, text } => {
                        self.preview_doc = Some(text);
                        self.preview_doc_label = label;
                    }
                    DocPreview::Csv(csv) => {
                        self.preview_csv = Some(csv);
                        self.preview_csv_tsv = false;
                    }
                    DocPreview::Image(img) => {
                        self.preview_image = Some(img);
                    }
                    DocPreview::None => {}
                }
            }
        }

        // If a document/grid/image was resolved above, skip the legacy
        // text/image/hex read entirely (it would re-read the same file and could
        // mis-classify a binary document as a hex blob). Quick Look opens now.
        if self.preview_doc.is_some() || self.preview_image.is_some() || self.preview_csv.is_some()
        {
            self.overlay = Overlay::QuickLook;
            return;
        }

        let fd = raekit::sys::open(p.as_str(), 0);
        if fd != u64::MAX {
            let n = raekit::sys::read(fd, &mut self.preview) as usize;
            self.preview_len = n.min(PREVIEW_CAP);
            // Heuristic: printable-ASCII-dominant → render as text.
            self.preview_is_text = looks_textual(&self.preview[..self.preview_len]);

            // If the first bytes are the PNG signature OR the JPEG SOI magic
            // (FF D8 FF), slurp the whole file and decode it to real pixels. Both
            // decoders are hostile-input safe (return Err, never panic) — on any
            // failure (PNG Adam7/16-bit/truncation; JPEG progressive/CMYK/corrupt
            // → UnsupportedFormat/Err) we leave `preview_image = None` and the
            // overlay falls back to the existing dims/hex summary. JPEG uses the
            // EXIF-oriented decode so portraits from phones/cameras display
            // upright instead of sideways (Concept §creators/media papercut).
            if is_png_signature(&self.preview[..self.preview_len]) {
                if let Some(bytes) = read_whole_file(fd, &self.preview[..self.preview_len]) {
                    if let Ok(img) = decode_png(&bytes) {
                        self.preview_image = Some(img);
                    }
                }
            } else if is_jpeg_signature(&self.preview[..self.preview_len]) {
                if let Some(bytes) = read_whole_file(fd, &self.preview[..self.preview_len]) {
                    if let Ok(img) = decode_jpeg_oriented(&bytes) {
                        self.preview_image = Some(jpeg_to_canvas_image(img));
                    }
                }
            } else if is_gif_signature(&self.preview[..self.preview_len]) {
                // GIF: decode and preview frame 0 (a static first-frame preview is
                // the Quick Look deliverable; Photos animates). The composited
                // frame is a full WxH ARGB buffer — identical shape to a PNG.
                if let Some(bytes) = read_whole_file(fd, &self.preview[..self.preview_len]) {
                    if let Ok(gif) = decode_gif(&bytes) {
                        if let Some(frame0) = gif.frames.into_iter().next() {
                            self.preview_image = Some(gif_frame_to_canvas_image(
                                gif.width,
                                gif.height,
                                frame0.pixels,
                            ));
                        }
                    }
                }
            }
            let _ = raekit::sys::close(fd);
        }
        self.overlay = Overlay::QuickLook;
    }

    // ── Checksum / Verify (download-integrity) ─────────────────────────────
    //
    // RaeenOS_Concept.md §"the user owns the machine" / "security by default,
    // not by friction": when you download an installer, an ISO, or a `.raepkg`
    // and the publisher posted a checksum, you should be able to verify the
    // file's integrity locally — no network round-trip, no third party. Press
    // `h` on a selected file: Files STREAMS it through `rae_hash` (64 KiB blocks,
    // so a multi-GB image never loads whole) and shows SHA-256 (the primary),
    // SHA-1, MD5, and CRC32. Paste the published hash into the verify field and
    // press Enter — the algorithm is auto-detected by the hex LENGTH and matched
    // case-insensitively → a green MATCH or red NO MATCH. Never panics: a read
    // error → toast; a huge file → streamed (no OOM); an empty file → the valid
    // empty-input digest.

    fn checksum_name_str(&self) -> &str {
        core::str::from_utf8(&self.checksum_name[..self.checksum_name_len]).unwrap_or("?")
    }

    fn verify_str(&self) -> &str {
        core::str::from_utf8(&self.verify_buf[..self.verify_len]).unwrap_or("")
    }

    fn verify_push(&mut self, c: u8) {
        if self.verify_len < self.verify_buf.len() {
            self.verify_buf[self.verify_len] = c;
            self.verify_len += 1;
            // Editing the field invalidates a prior verdict until re-submitted.
            self.verify_result = VerifyResult::None;
        }
    }

    fn verify_backspace(&mut self) {
        if self.verify_len > 0 {
            self.verify_len -= 1;
            self.verify_result = VerifyResult::None;
        }
    }

    /// Open the Checksum overlay for the selected FILE: stream-hash it and stash
    /// the four digests. A folder / read failure → a toast, overlay stays closed.
    fn open_checksum(&mut self) {
        let entry = match self.entries().get(self.selected) {
            Some(e) => *e,
            None => return,
        };
        if entry.kind == Kind::Folder {
            self.toast.set("Checksum: select a file");
            return;
        }
        let mut p = PathBuf::new();
        self.entry_path(&entry, &mut p);

        match hash_file_streaming(p.as_str()) {
            Some(digests) => {
                let nl = entry.name_len.min(48);
                self.checksum_name[..nl].copy_from_slice(&entry.name[..nl]);
                self.checksum_name_len = nl;
                self.checksum_size = entry.bytes;
                self.checksum_sha256 = digests.sha256;
                self.checksum_sha1 = digests.sha1;
                self.checksum_md5 = digests.md5;
                self.checksum_crc32 = digests.crc32;
                // Reset the verify field for the new target.
                self.verify_len = 0;
                self.verify_result = VerifyResult::None;
                self.overlay = Overlay::Checksum;
            }
            None => {
                self.toast.set("Checksum: could not read file");
            }
        }
    }

    /// Run the pasted expected-hash against the file's digests. The algorithm is
    /// auto-detected by the trimmed hex LENGTH (64 = SHA-256, 40 = SHA-1, 32 =
    /// MD5, 8 = CRC32); an unrecognized length → NO MATCH (never panics). The
    /// comparison itself is `rae_hash::verify`'s case-insensitive equality, run
    /// against the digest we already streamed (so no second full file read).
    fn run_verify(&mut self) {
        let want = self.verify_str().trim();
        if want.is_empty() {
            self.verify_result = VerifyResult::None;
            return;
        }
        let got = match algo_for_len(want.len()) {
            Some(rae_hash::Algo::Sha256) => Some(self.checksum_sha256.as_str()),
            Some(rae_hash::Algo::Sha1) => Some(self.checksum_sha1.as_str()),
            Some(rae_hash::Algo::Md5) => Some(self.checksum_md5.as_str()),
            Some(rae_hash::Algo::Crc32) => Some(self.checksum_crc32.as_str()),
            None => None,
        };
        self.verify_result = match got {
            Some(digest) if hex_eq_ci(digest, want) => VerifyResult::Match,
            _ => VerifyResult::NoMatch,
        };
    }

    // ── Compare (unified diff of two marked files) ──────────────────────────
    //
    // A genuine power-user/dev surface (Concept §Windows Pain Points "the modern
    // file manager"): mark EXACTLY two text files (`m`), press Compare, and see a
    // scrollable, color-coded unified diff. The diff itself is the host-KAT'd
    // `rae_diff::unified_diff`; this method is the thin syscall + decode shell.
    // Never panics: a binary / non-UTF-8 / oversized file shows a message and
    // leaves the app running.

    /// Build the absolute path + leaf name of the i-th MARKED entry into `out`/
    /// `name_buf`. Returns the leaf-name length, or `None` if there are fewer than
    /// `which+1` marked entries (or the marked entry is a folder).
    fn marked_entry(
        &self,
        which: usize,
        out: &mut PathBuf,
        name_buf: &mut [u8; 48],
    ) -> Option<usize> {
        let mut seen = 0usize;
        for e in self.entries() {
            if !e.marked {
                continue;
            }
            if seen == which {
                if e.kind == Kind::Folder {
                    return None;
                }
                self.entry_path(e, out);
                let n = e.name_len.min(48);
                name_buf[..n].copy_from_slice(&e.name[..n]);
                return Some(n);
            }
            seen += 1;
        }
        None
    }

    /// Open the Compare overlay for the two marked files. Requires EXACTLY two
    /// marked entries, both files. Reads both (size-capped), guards against
    /// binary / non-UTF-8 content, computes a 3-context unified diff via
    /// `rae_diff`, and shows it. Any failure → a status message, overlay stays
    /// closed, app alive.
    fn open_compare(&mut self) {
        if self.marked_count() != 2 {
            self.toast.set("Mark exactly 2 files to compare (m)");
            return;
        }

        // Resolve both marked file paths + leaf names.
        let mut path_a = PathBuf::new();
        let mut name_a = [0u8; 48];
        let mut path_b = PathBuf::new();
        let mut name_b = [0u8; 48];
        let (la, lb) = match (
            self.marked_entry(0, &mut path_a, &mut name_a),
            self.marked_entry(1, &mut path_b, &mut name_b),
        ) {
            (Some(la), Some(lb)) => (la, lb),
            _ => {
                self.toast.set("Compare: pick 2 files (not folders)");
                return;
            }
        };

        // Read both as UTF-8 text (binary / too-large → bail, never crash).
        let text_a = match read_file_text(path_a.as_str()) {
            Some(t) => t,
            None => {
                self.toast.set("Can't diff (binary or too large)");
                return;
            }
        };
        let text_b = match read_file_text(path_b.as_str()) {
            Some(t) => t,
            None => {
                self.toast.set("Can't diff (binary or too large)");
                return;
            }
        };

        // Compute the unified diff (3 context lines) via the host-KAT'd engine.
        let diff = unified_diff(&text_a, &text_b, 3);

        self.compare_diff = if diff.is_empty() {
            // Identical files: unified_diff returns "" — show a clear message in
            // the overlay rather than a blank panel.
            b"(files are identical)".to_vec()
        } else {
            diff.into_bytes()
        };
        self.compare_scroll = 0;
        self.compare_name_a = name_a;
        self.compare_name_a_len = la;
        self.compare_name_b = name_b;
        self.compare_name_b_len = lb;
        self.overlay = Overlay::Compare;
    }

    fn compare_name_a_str(&self) -> &str {
        core::str::from_utf8(&self.compare_name_a[..self.compare_name_a_len]).unwrap_or("?")
    }
    fn compare_name_b_str(&self) -> &str {
        core::str::from_utf8(&self.compare_name_b[..self.compare_name_b_len]).unwrap_or("?")
    }

    /// Total number of lines in the current compare diff buffer (a `\n` count + 1
    /// for any trailing partial line).
    fn compare_line_count(&self) -> usize {
        if self.compare_diff.is_empty() {
            return 0;
        }
        let nl = self.compare_diff.iter().filter(|&&b| b == b'\n').count();
        // If the buffer ends without a newline, that tail is one more line.
        if self.compare_diff.last() == Some(&b'\n') {
            nl
        } else {
            nl + 1
        }
    }

    /// Scroll the compare view by `delta` lines, clamped so at least one line
    /// stays visible.
    fn compare_scroll_by(&mut self, delta: i32) {
        let total = self.compare_line_count();
        let visible = compare_visible_rows();
        let max_scroll = total.saturating_sub(visible);
        let cur = self.compare_scroll as i32;
        let next = (cur + delta).clamp(0, max_scroll as i32);
        self.compare_scroll = next as usize;
    }

    /// Scroll the Quick Look CSV/TSV table by `delta` data rows, clamped so the
    /// last data row stays in view. The header row is pinned (not scrolled); the
    /// max scroll is the count of renderable data rows (capped) minus zero — we
    /// simply never scroll past the last renderable data row.
    fn csv_scroll_by(&mut self, delta: i32) {
        let render_limit = match self.preview_csv.as_ref() {
            Some(csv) => csv.len().min(CSV_MAX_RENDER_ROWS),
            None => return,
        };
        // Data rows are indices 1..render_limit → at most render_limit-1 of them.
        let max_scroll = render_limit.saturating_sub(1).saturating_sub(1);
        let cur = self.preview_csv_scroll as i32;
        let next = (cur + delta).clamp(0, max_scroll as i32);
        self.preview_csv_scroll = next as usize;
    }

    /// Scroll the Quick Look document text (PDF/DOCX) by `delta` logical lines,
    /// clamped so at least one line stays visible. Lines are `\n`-separated plus
    /// each PDF page break (`\u{0C}`); the max scroll is the total line count minus
    /// one so the last line never scrolls off the top.
    fn doc_scroll_by(&mut self, delta: i32) {
        let total = match self.preview_doc.as_ref() {
            Some(t) => doc_line_count(t.as_bytes()),
            None => return,
        };
        let max_scroll = total.saturating_sub(1);
        let cur = self.preview_doc_scroll as i32;
        let next = (cur + delta).clamp(0, max_scroll as i32);
        self.preview_doc_scroll = next as usize;
    }

    // ── Batch rename ───────────────────────────────────────────────────────

    fn open_batch_rename(&mut self) {
        // Mark the current selection if nothing is marked yet, so single-file
        // rename also works.
        if self.marked_count() == 0 {
            if let Some(e) = self.entries.get_mut(self.selected) {
                e.marked = true;
            }
        }
        if self.marked_count() == 0 {
            self.toast.set("Nothing selected");
            return;
        }
        self.overlay = Overlay::BatchRename;
    }

    fn pattern_str(&self) -> &str {
        core::str::from_utf8(&self.pattern[..self.pattern_len]).unwrap_or("")
    }

    fn pattern_push(&mut self, c: u8) {
        if self.pattern_len < self.pattern.len() {
            self.pattern[self.pattern_len] = c;
            self.pattern_len += 1;
        }
    }

    fn pattern_backspace(&mut self) {
        if self.pattern_len > 0 {
            self.pattern_len -= 1;
        }
    }

    /// Apply the batch rename to every marked entry, counter starting at 1.
    fn apply_batch_rename(&mut self) {
        let cwd_len = self.cwd().as_bytes().len();
        // Snapshot pattern into a local buffer (avoid &self borrow during mutation).
        let mut pat = [0u8; 48];
        let pl = self.pattern_len;
        pat[..pl].copy_from_slice(&self.pattern[..pl]);
        let pattern = core::str::from_utf8(&pat[..pl]).unwrap_or("");

        let mut ok = 0u32;
        let mut idx = 0u32;
        let mut failures = 0u32;
        // Collect marked entries first (indices into self.entries snapshot).
        let mut targets: [usize; MAX_ENTRIES] = [0; MAX_ENTRIES];
        let mut tcount = 0usize;
        for (i, e) in self.entries().iter().enumerate() {
            if e.marked {
                targets[tcount] = i;
                tcount += 1;
            }
        }
        for t in 0..tcount {
            let ei = targets[t];
            // Re-read name from the snapshot copy.
            let mut name_buf = [0u8; 48];
            let nl = self.entries[ei].name_len;
            name_buf[..nl].copy_from_slice(&self.entries[ei].name[..nl]);
            let original = core::str::from_utf8(&name_buf[..nl]).unwrap_or("");

            match batch_rename_target(original, pattern, idx, 1, cwd_len) {
                Ok(new_name) => {
                    let mut src = PathBuf::new();
                    src.set(self.cwd());
                    src.push_component(original);
                    let mut dst = PathBuf::new();
                    dst.set(self.cwd());
                    dst.push_component(new_name.as_str());
                    match raekit::sys::rename(src.as_str(), dst.as_str()) {
                        Ok(()) => ok += 1,
                        Err(_) => failures += 1,
                    }
                    idx += 1;
                }
                Err(_) => {
                    // Bad pattern → abort the whole op with a clear message.
                    self.toast.set("Invalid pattern (need a ### counter)");
                    self.overlay = Overlay::None;
                    return;
                }
            }
        }
        let _ = ok;
        if failures > 0 {
            self.toast.set("Some renames failed (name taken)");
        } else {
            self.toast.set("Batch rename applied");
        }
        self.overlay = Overlay::None;
        // Clear marks + refresh.
        for e in self.entries.iter_mut() {
            e.marked = false;
        }
        self.refresh_entries();
    }

    fn toggle_mark(&mut self) {
        if let Some(e) = self.entries.get_mut(self.selected) {
            e.marked = !e.marked;
        }
    }

    fn move_sel(&mut self, delta: i32) {
        let n = self.entries().len();
        if n == 0 {
            return;
        }
        let new = (self.selected as i32 + delta).rem_euclid(n as i32) as usize;
        self.selected = new;
        let list_rows = (WIN_H - TITLE_H - TABBAR_H - TOOLBAR_H - BREADCRUMB_H - STATUS_H) / ROW_H;
        if self.selected < self.scroll {
            self.scroll = self.selected;
        }
        if self.selected >= self.scroll + list_rows {
            self.scroll = self.selected + 1 - list_rows;
        }
    }

    // ── Click hit-testing + dispatch ──────────────────────────────────────
    //
    // `build_layout` produces the interactive-element list for the CURRENT
    // frame at the EXACT rects `render` draws (via the shared `geom_*`
    // helpers). `on_click`/`on_double_click` hit-test against it and dispatch
    // each click to the SAME method the keyboard path fires.

    /// Build the interactive element list for the current state. Order matters
    /// only for overlapping rects; here every rect is disjoint, so any order is
    /// safe. Row elements carry the ACTUAL entry index (visible offset + scroll).
    fn build_layout(&self) -> Layout {
        let mut layout = Layout::new();

        // Title-bar close + tab strip "+" / tabs.
        layout.push(geom_close(), Action::CloseApp);
        layout.push(geom_new_tab(self.tabs.count()), Action::NewTab);
        for i in 0..self.tabs.count() {
            match geom_tab(i) {
                Some(r) => layout.push(r, Action::SwitchTab(i)),
                None => break,
            }
        }

        // Toolbar buttons.
        for (rect, action) in geom_toolbar() {
            layout.push(rect, action);
        }

        // Sidebar Quick Access.
        for i in 0..QUICK_ACCESS.len() {
            layout.push(geom_quick_access(i), Action::QuickAccess(i));
        }

        // Visible list rows (each → its actual entry index).
        let entries = self.entries();
        let rows = list_rows_visible();
        let end = (self.scroll + rows).min(entries.len());
        for (vis, actual) in (self.scroll..end).enumerate() {
            layout.push(geom_row(vis), Action::SelectRow(actual));
        }

        layout
    }

    /// Dispatch a single left-click at surface-local `(px, py)`. Returns true if
    /// anything changed (so the caller re-renders). A click in empty space hits
    /// nothing and is a no-op (never panics).
    fn on_click(&mut self, px: i32, py: i32) -> bool {
        let Some(action) = self.build_layout().hit(px, py) else {
            // Empty space inside the list view clears the selection (Finder/Explorer
            // behavior); elsewhere it is a no-op.
            return false;
        };
        self.dispatch(action)
    }

    /// Dispatch a double-click at surface-local `(px, py)`. A double-click on a
    /// row OPENS it (dir = navigate, file = Quick Look); on any other element it
    /// behaves like a single click. Returns true if anything changed.
    fn on_double_click(&mut self, px: i32, py: i32) -> bool {
        match self.build_layout().hit(px, py) {
            Some(Action::SelectRow(idx)) => self.dispatch(Action::OpenRow(idx)),
            Some(action) => self.dispatch(action),
            None => false,
        }
    }

    /// Apply an `Action` — each arm calls the same method the matching key fires.
    fn dispatch(&mut self, action: Action) -> bool {
        match action {
            Action::SelectRow(idx) => {
                if idx < self.entry_count {
                    self.selected = idx;
                    // Keep the selection visible (mirror move_sel's scroll clamp).
                    let rows = list_rows_visible();
                    if self.selected < self.scroll {
                        self.scroll = self.selected;
                    } else if rows > 0 && self.selected >= self.scroll + rows {
                        self.scroll = self.selected + 1 - rows;
                    }
                    true
                } else {
                    false
                }
            }
            Action::OpenRow(idx) => {
                if idx < self.entry_count {
                    self.selected = idx;
                    // Folder → navigate; file → Quick Look (mirrors Enter/Space).
                    let is_folder = self.entries()[idx].kind == Kind::Folder;
                    if is_folder {
                        self.enter_selected();
                    } else {
                        self.open_quick_look();
                    }
                    true
                } else {
                    false
                }
            }
            Action::SwitchTab(i) => {
                if i != self.tabs.active_index() && self.tabs.select(i).is_ok() {
                    self.selected = 0;
                    self.scroll = 0;
                    self.refresh_entries();
                    true
                } else {
                    false
                }
            }
            Action::NewTab => {
                self.new_tab();
                true
            }
            Action::CloseApp => {
                raekit::sys::exit(0);
            }
            Action::GoBack => {
                self.go_back();
                true
            }
            Action::GoForward => {
                self.go_forward();
                true
            }
            Action::GoUp => {
                self.go_up();
                true
            }
            Action::NewFolder => {
                self.new_folder();
                true
            }
            Action::OpenRename => {
                self.open_batch_rename();
                true
            }
            Action::TrashSelected => {
                self.trash_selected();
                true
            }
            Action::QuickAccess(i) => {
                self.navigate_quick_access(i);
                true
            }
        }
    }
}

/// Printable-ASCII heuristic: ≥90% of the first bytes are printable/whitespace.
fn looks_textual(buf: &[u8]) -> bool {
    if buf.is_empty() {
        return false;
    }
    let mut printable = 0usize;
    for &b in buf {
        if b == b'\n' || b == b'\r' || b == b'\t' || (0x20..=0x7E).contains(&b) {
            printable += 1;
        } else if b == 0 {
            return false; // NUL → binary
        }
    }
    printable * 10 >= buf.len() * 9
}

// ── Rendering ───────────────────────────────────────────────────────────

fn render(app: &App, canvas: &mut Canvas) {
    // Main window: drawn from the syscall-free view snapshot (single source of
    // truth shared with the host screenshot harness). Behavior-identical to the
    // old inline draw — the theme syscall is read once in `view_state`.
    let state = app.view_state();
    render_preview(canvas, &state);

    // Modal overlays render last (on top). These still read `App` directly
    // (preview buffers, decoded images, search results) — they are not part of
    // the representative host preview, which always renders the base window.
    match app.overlay {
        Overlay::QuickLook => render_quick_look(app, canvas),
        Overlay::BatchRename => render_batch_rename(app, canvas),
        Overlay::Compare => render_compare(app, canvas),
        Overlay::Checksum => render_checksum(app, canvas),
        Overlay::OpenWith => render_open_with(app, canvas),
        Overlay::Search => render_search(app, canvas),
        Overlay::None => {}
    }
}

/// Render the Files main window from a syscall-free [`FilesViewState`]. PURE —
/// it touches only the `canvas`, `rae_tokens`, and the embedded font/icon data,
/// so a host tool (the screenshot harness) can render the LIVE Files draw path
/// with no kernel. The live app calls this every frame via `render`; the only
/// difference between the two callers is where the state came from (live
/// syscalls vs. [`preview_state_demo`]).
pub fn render_preview(canvas: &mut Canvas, state: &FilesViewState) {
    let seed = state.theme_seed;

    // ── Liquid Glass window chrome (IDENTITY.md §7; visual-QA Round-5 §2) ──────
    //
    // The whole window is FROSTED CHROME glass: composite the `glass.chrome`
    // tint→frost (the SAME order the Control Center uses) over whatever is behind
    // the window — the aurora in the host-render path, the compositor's blurred
    // backdrop in the live path — so the chrome lands at/above the backdrop
    // luminance instead of being a flat dark box. The sidebar gets the heavier
    // `glass.panel` tier and the content list is overdrawn SOLID + de-tinted
    // below; the window edge gets the iridescent rim last. A soft elev.2 ambient
    // shadow floats the window (skipped if it would clip the frame).
    let shadow = rae_tokens::ELEV_2;
    canvas.fill_rounded_rect_shadow(
        0,
        0,
        WIN_W,
        WIN_H,
        WIN_RADIUS,
        shadow.color,
        shadow.radius as usize,
        shadow.offset_y,
    );
    // Frosted chrome base across the whole window (tint then frost — the backdrop
    // reads through). Rounded so the window reads as a floating glass sheet.
    canvas.fill_rounded_rect(0, 0, WIN_W, WIN_H, WIN_RADIUS, CHROME_TIER.tint);
    canvas.fill_rounded_rect(0, 0, WIN_W, WIN_H, WIN_RADIUS, CHROME_TIER.frost);

    // NO title-bar frost lift. IDENTITY-OBSIDIAN retired the frost recipe
    // ("chrome = the DEEPEST tier, most wallpaper shows; frost is a breath, not
    // milk"). The old additive `GLASS_FROST_LIGHTEN` pass made the titlebar the
    // BRIGHTEST band — the exact inversion OBSIDIAN corrects. Chrome now reads
    // uniformly near-black; the top edge is defined by the 1px hairline (below).

    // Title bar — macOS-style traffic-light close/min/max pills on the LEFT (the
    // hard saturated-red close SQUARE was the most off-system pixel in the frame,
    // visual-QA Round-5 P0 #2). Tinted pill controls, not a primary-red block.
    let tl_y = TITLE_H / 2;
    let tl_r = 6usize;
    for (i, tint) in [DARK.state_danger, DARK.state_warn, DARK.state_ok]
        .iter()
        .enumerate()
    {
        let cx = 14 + i * 20;
        canvas.fill_rounded_rect(cx - tl_r, tl_y - tl_r, tl_r * 2, tl_r * 2, tl_r, *tint);
    }
    // "Files" title centered (the traffic lights took the left inset).
    let title_w = canvas.measure_text_aa("Files", rae_tokens::TYPE_SUBTITLE, FontFamily::Sans);
    canvas.draw_text_aa(
        (WIN_W as i32 - title_w) / 2,
        ((TITLE_H.saturating_sub(rae_tokens::TYPE_SUBTITLE.line_height as usize)) / 2) as i32,
        "Files",
        rae_tokens::TYPE_SUBTITLE,
        TEXT_FG,
        FontFamily::Sans,
    );

    // Tab strip
    render_tabs_preview(state, canvas);

    // Toolbar — NO opaque fill: it rides on the frosted chrome glass (the aurora
    // reads through), IDENTITY §7. Nav = borderless icon buttons (Finder/
    // Explorer register); actions = quiet labeled pills, grouped with real gaps.
    let tb_y = TITLE_H + TABBAR_H;
    let tb_btn = 26usize;
    let tb_by = tb_y + (TOOLBAR_H - tb_btn) / 2;
    draw_tool_icon_button(
        canvas,
        8,
        tb_by,
        tb_btn,
        raegfx::icon::Icon::ChevronLeft,
        state.can_back,
    );
    draw_tool_icon_button(
        canvas,
        38,
        tb_by,
        tb_btn,
        raegfx::icon::Icon::Chevron,
        state.can_forward,
    );
    draw_tool_icon_button(
        canvas,
        68,
        tb_by,
        tb_btn,
        raegfx::icon::Icon::ChevronUp,
        true,
    );
    draw_button(canvas, 110, tb_by, 92, tb_btn, "New Folder");
    draw_button(canvas, 208, tb_by, 68, tb_btn, "Rename");
    draw_button(canvas, 282, tb_by, 58, tb_btn, "Trash");

    // Breadcrumb — rides on the frosted chrome (no opaque fill), IDENTITY §7.
    let bc_y = tb_y + TOOLBAR_H;
    let bc_ty = (bc_y
        + (BREADCRUMB_H.saturating_sub(rae_tokens::TYPE_CAPTION.line_height as usize)) / 2)
        as i32;
    // Accessibility (raeen-accessibility): the breadcrumb "Path:" label rides on
    // the frosted `glass.chrome` band, which over the bright-aurora region measured
    // ~1.1–2.7:1 in `text.secondary` (the shell hand-rolls glass and bypasses the
    // runtime luma cap). Promote to `text.primary` — the same fix already applied
    // to the Control Center labels (do not darken the token).
    let path_w = canvas.draw_text_aa(
        12,
        bc_ty,
        "Path:",
        rae_tokens::TYPE_CAPTION,
        TEXT_FG,
        FontFamily::Sans,
    );
    canvas.draw_text_aa(
        path_w + 8,
        bc_ty,
        state.cwd.as_str(),
        rae_tokens::TYPE_CAPTION,
        accent_s(seed),
        FontFamily::Sans,
    );

    // Sidebar (Quick Access) — the classic Finder/Explorer TRANSLUCENT rail:
    // `glass.panel` tier composited tint→frost over the chrome/aurora, so the
    // backdrop bleeds through it (IDENTITY §7). Heavier than the chrome so the rail
    // reads as a distinct, slightly-more-opaque zone but still glass — not a dark
    // slate.
    let sb_y = bc_y + BREADCRUMB_H;
    let sb_h = WIN_H - sb_y - STATUS_H;
    // `fill_rounded_rect` with radius 0 BLENDS the tier alpha (unlike `fill_rect`,
    // which overwrites) so the chrome/aurora reads through the translucent tier.
    canvas.fill_rounded_rect(0, sb_y, SIDEBAR_W, sb_h, 0, PANEL_TIER.tint);
    canvas.fill_rounded_rect(0, sb_y, SIDEBAR_W, sb_h, 0, PANEL_TIER.frost);
    // Accessibility (raeen-accessibility): the "Quick Access" sidebar header sits
    // on the frosted `glass.panel` rail over the bright aurora — `text.secondary`
    // measured ~1.1–2.7:1 there. Promote to `text.primary`, consistent with the
    // breadcrumb label above and the Control Center labels.
    canvas.draw_text_aa(
        12,
        (sb_y + 8) as i32,
        "Quick Access",
        rae_tokens::TYPE_CAPTION,
        TEXT_FG,
        FontFamily::Sans,
    );
    let mut home = PathBuf::new();
    home.set(state.home.as_str());
    let mut sy = sb_y + 28;
    for &(name, path, glyph, home_rel) in QUICK_ACCESS {
        let mut resolved = home;
        if home_rel {
            if !path.is_empty() {
                resolved.push_component(path);
            }
        } else {
            resolved.set(path);
        }
        let active = state.cwd.as_str() == resolved.as_str();
        // Finder-scale sidebar rows: 24px row on a 28px pitch (the old 22/24
        // read cramped next to macOS's rail).
        let sb_row_h = 24usize;
        if active {
            canvas.fill_rounded_rect(
                4,
                sy,
                SIDEBAR_W - 8,
                sb_row_h,
                rae_tokens::RADIUS_SM as usize,
                row_sel_s(seed),
            );
        }
        let _ = glyph; // letter placeholder retired in favor of the line-icon
        let item_ty = (sy
            + (sb_row_h.saturating_sub(rae_tokens::TYPE_LABEL.line_height as usize)) / 2)
            as i32;
        // Sidebar locations are directories → the real Folder line-icon tinted
        // `ftype.dir` (tracks accent, §6) — replaces the old letter placeholder.
        // IDENTITY §4: the accent-filled ACTIVE row flips to dark ink.
        let (sb_icon_ink, sb_text_ink) = if active {
            (DARK.bg_base, DARK.bg_base)
        } else {
            (ftype_color_s(FType::Dir, seed), TEXT_FG)
        };
        let sb_icon_sz = 15i32;
        let sb_icon_y = (sy + (sb_row_h.saturating_sub(sb_icon_sz as usize)) / 2) as i32;
        canvas.draw_icon(
            ftype_icon(FType::Dir),
            12,
            sb_icon_y,
            sb_icon_sz,
            sb_icon_ink,
        );
        canvas.draw_text_aa(
            32,
            item_ty,
            name,
            rae_tokens::TYPE_LABEL,
            sb_text_ink,
            FontFamily::Sans,
        );
        sy += 28;
    }

    // List view — SOLID, de-tinted content field (NOT glass): a dense file list
    // over a moving aurora is a legibility nightmare neither Finder nor Explorer
    // accepts (visual-QA §2). Lifted off the bluish near-black to a neutral slate
    // so the §4.4 ftype icon tints pop and it no longer reads as a dark hole.
    let lv_x = SIDEBAR_W;
    let lv_y = sb_y;
    let lv_w = WIN_W - SIDEBAR_W;
    let lv_h = sb_h;
    canvas.fill_rect(lv_x, lv_y, lv_w, lv_h, CONTENT_BG);

    // Column header — one neutral step above the content field.
    canvas.fill_rect(lv_x, lv_y, lv_w, 22, CONTENT_HDR_BG);
    let hdr_ty =
        (lv_y + (22usize.saturating_sub(rae_tokens::TYPE_CAPTION.line_height as usize)) / 2) as i32;
    canvas.draw_text_aa(
        (lv_x + 32) as i32,
        hdr_ty,
        "Name",
        rae_tokens::TYPE_CAPTION,
        TEXT_MUTED,
        FontFamily::Sans,
    );
    canvas.draw_text_aa(
        (lv_x + lv_w - 90) as i32,
        hdr_ty,
        "Size",
        rae_tokens::TYPE_CAPTION,
        TEXT_MUTED,
        FontFamily::Sans,
    );

    let entries = state.entries.as_slice();
    let list_rows = lv_h.saturating_sub(22) / ROW_H;
    let scroll = state.scroll.min(entries.len());
    let visible_end = (scroll + list_rows).min(entries.len());

    for (i, e) in entries[scroll..visible_end].iter().enumerate() {
        let actual = i + scroll;
        let row_y = lv_y + 22 + i * ROW_H;
        let selected = actual == state.selected;

        if selected {
            // Rounded INSET selection pill (the Finder/Explorer register) — a
            // hard full-bleed band read as a terminal highlight (visual-QA).
            canvas.fill_rounded_rect(
                lv_x + 4,
                row_y + 1,
                lv_w - 8,
                ROW_H - 2,
                rae_tokens::RADIUS_SM as usize,
                row_sel_s(seed),
            );
        } else if e.marked {
            canvas.fill_rounded_rect(
                lv_x + 4,
                row_y + 1,
                lv_w - 8,
                ROW_H - 2,
                rae_tokens::RADIUS_SM as usize,
                DARK.bg_elevated,
            );
        } else if actual % 2 == 1 {
            // Translucent alt-row tint — BLEND over the de-tinted content field
            // (radius 0 so it's a flat band but alpha-composited, not overwritten).
            canvas.fill_rounded_rect(lv_x, row_y, lv_w, ROW_H, 0, ROW_HOVER);
        }

        let name = e.name.as_str();
        // §4.4 file-type semantics: classify by kind+extension, tint from the
        // `rae_tokens::ftype_*` palette (dir/code track the live accent).
        let ft = if e.is_folder {
            FType::Dir
        } else {
            classify_name(name)
        };
        // IDENTITY §4 guardrail: an accent-FILLED selection carries DARK ink
        // (white-on-RaeBlue ≈2.6:1 fails WCAG) — icon, name, and size all flip.
        let (fg, name_ink, size_ink) = if selected {
            (DARK.bg_base, DARK.bg_base, DARK.bg_base)
        } else {
            (ftype_color_s(ft, seed), TEXT_FG, TEXT_MUTED)
        };
        let row_ty =
            (row_y + (ROW_H.saturating_sub(rae_tokens::TYPE_BODY.line_height as usize)) / 2) as i32;
        // Mark indicator (checkbox dot) before the icon for selected-for-batch.
        if e.marked {
            canvas.fill_rounded_rect(lv_x + 2, row_y + ROW_H / 2 - 3, 6, 6, 3, accent_s(seed));
        }
        // Real line-icon (`raegfx::icon`), tinted by the §4.4 palette — replaces
        // the old letter/block placeholder (visual-QA Round-2 fix).
        let icon_sz = 16i32;
        let icon_y = (row_y + ROW_H.saturating_sub(icon_sz as usize) / 2) as i32;
        canvas.draw_icon(ftype_icon(ft), (lv_x + 10) as i32, icon_y, icon_sz, fg);
        canvas.draw_text_aa(
            (lv_x + 42) as i32,
            row_ty,
            name,
            rae_tokens::TYPE_BODY,
            name_ink,
            FontFamily::Sans,
        );

        if !e.is_folder {
            let mut buf = [0u8; 24];
            let n = fmt_size(e.bytes, &mut buf);
            if let Ok(s) = core::str::from_utf8(&buf[..n]) {
                let sw = canvas.measure_text_aa(s, rae_tokens::TYPE_BODY, FontFamily::Sans);
                let px = (lv_x + lv_w - 8) as i32 - sw;
                canvas.draw_text_aa(
                    px,
                    row_ty,
                    s,
                    rae_tokens::TYPE_BODY,
                    size_ink,
                    FontFamily::Sans,
                );
            }
        }
    }

    // Status bar — frosted chrome band (translucent, the aurora reads through),
    // matching the top chrome so the footer belongs to the same glass system.
    let st_y = WIN_H - STATUS_H;
    // Blending fills (radius 0) so the chrome tier alpha composites over the
    // content/aurora instead of overwriting it.
    canvas.fill_rounded_rect(0, st_y, WIN_W, STATUS_H, 0, CHROME_TIER.tint);
    canvas.fill_rounded_rect(0, st_y, WIN_W, STATUS_H, 0, CHROME_TIER.frost);
    let st_ty = (st_y
        + (STATUS_H.saturating_sub(rae_tokens::TYPE_CAPTION.line_height as usize)) / 2)
        as i32;
    // Left: toast (if any) else item count.
    if !state.toast.is_empty() {
        canvas.draw_text_aa(
            12,
            st_ty,
            state.toast.as_str(),
            rae_tokens::TYPE_CAPTION,
            accent_s(seed),
            FontFamily::Sans,
        );
    } else {
        let mut buf = [0u8; 64];
        let n = fmt_count(entries.len(), state.marked_count, &mut buf);
        if let Ok(s) = core::str::from_utf8(&buf[..n]) {
            canvas.draw_text_aa(
                12,
                st_ty,
                s,
                rae_tokens::TYPE_CAPTION,
                TEXT_MUTED,
                FontFamily::Sans,
            );
        }
    }
    // Trimmed to the four highest-value shortcuts — the full seven-item wall
    // read as debug chrome (visual-QA); the rest stay discoverable via '?'.
    let hint = "/ Search   Space Look   F2 Rename   Del Trash";
    let hint_w = canvas.measure_text_aa(hint, rae_tokens::TYPE_CAPTION, FontFamily::Sans);
    canvas.draw_text_aa(
        (WIN_W - 12) as i32 - hint_w,
        st_ty,
        hint,
        rae_tokens::TYPE_CAPTION,
        TEXT_MUTED,
        FontFamily::Sans,
    );

    // ── Window edge: a 1px light hairline, hugging the rounded window edge.
    //    IDENTITY-OBSIDIAN §"retired": "the iridescent rim is retired on ALL
    //    surfaces — zero rainbow-rim pixels; hairline + top-light only." The old
    //    cyan→violet→warm `draw_iridescent_rim` (the retired frost fingerprint)
    //    is gone; the hairline below is the OBSIDIAN edge, matching CC/taskbar.
    canvas.draw_rounded_rect_outline(0, 0, WIN_W, WIN_H, WIN_RADIUS, STROKE_HL);
}

// ── Search overlay geometry (shared DRAW + mouse hit-test) ────────────────────
// One source of truth so a click lands on the exact row the renderer drew.
const SEARCH_CARD_W: usize = 480;
const SEARCH_CARD_H: usize = 360;
/// Pixel height of one result row (icon + name + dimmed path).
const SEARCH_ROW_H: usize = 38;
/// How many result rows fit in the scrollable list region.
const SEARCH_VISIBLE_ROWS: usize = 6;

/// The geometry of the search card + its row list, derived from the window size.
/// Returns `(card_x, card_y, list_x, list_y, list_w)`; both the renderer and the
/// mouse hit-test call this so a click maps to the SAME row that was drawn.
fn search_geometry() -> (usize, usize, usize, usize, usize) {
    let card_x = (WIN_W - SEARCH_CARD_W) / 2;
    let card_y = (WIN_H.saturating_sub(SEARCH_CARD_H)) / 2;
    let list_x = card_x + 12;
    // header (12) + subtitle gap (32) + query field (28) + gap (16) + summary (22)
    let list_y = card_y + 12 + 32 + 28 + 16 + 22;
    let list_w = SEARCH_CARD_W - 24;
    (card_x, card_y, list_x, list_y, list_w)
}

/// First visible result row index given the selection, so the highlighted row is
/// always on-screen (a simple windowed scroll). PURE.
fn search_scroll_top(selected: usize, total: usize) -> usize {
    if total <= SEARCH_VISIBLE_ROWS {
        return 0;
    }
    if selected < SEARCH_VISIBLE_ROWS {
        return 0;
    }
    // Keep the selected row as the last visible one when scrolled down.
    (selected + 1).saturating_sub(SEARCH_VISIBLE_ROWS)
}

/// Map a click at (px, py) inside the search card to the result-row index it hit,
/// or `None` if it landed outside the list region. Accounts for the current
/// scroll so it matches what the renderer drew. PURE (geometry only).
fn search_row_at(px: i32, py: i32, selected: usize, total: usize) -> Option<usize> {
    if total == 0 {
        return None;
    }
    let (_cx, _cy, list_x, list_y, list_w) = search_geometry();
    if px < list_x as i32 || px >= (list_x + list_w) as i32 {
        return None;
    }
    if py < list_y as i32 {
        return None;
    }
    let rel = (py - list_y as i32) as usize;
    let vis_idx = rel / SEARCH_ROW_H;
    if vis_idx >= SEARCH_VISIBLE_ROWS {
        return None;
    }
    let top = search_scroll_top(selected, total);
    let idx = top + vis_idx;
    if idx < total {
        Some(idx)
    } else {
        None
    }
}

/// Render the global indexed-search overlay: a centered glass card with the live
/// query field, a per-kind tally header, and a scrollable list of NAMED result
/// rows (each with its file-type icon tint, leaf name, and dimmed path) sourced
/// from `raekit::search::query_resolved` (SYS_SEARCH_QUERY_RESOLVED, 281). The
/// highlighted row is selectable by Up/Down + Enter and by mouse click; Enter on
/// a folder navigates, on a file opens via the default app. A queried-but-empty
/// index shows "No results" (graceful, never an error); before the first Enter it
/// shows the prompt.
fn render_search(app: &App, canvas: &mut Canvas) {
    canvas.fill_rect(0, 0, WIN_W, WIN_H, OVERLAY_SCRIM);

    let (card_x, card_y, list_x, list_y, list_w) = search_geometry();

    canvas.fill_rounded_rect(
        card_x,
        card_y,
        SEARCH_CARD_W,
        SEARCH_CARD_H,
        rae_tokens::RADIUS_MD as usize,
        DARK.bg_overlay,
    );

    // Header.
    canvas.draw_text_aa(
        (card_x + 16) as i32,
        (card_y + 12) as i32,
        "Search this PC",
        rae_tokens::TYPE_SUBTITLE,
        TEXT_FG,
        FontFamily::Sans,
    );

    // Query field.
    let field_y = card_y + 12 + 32;
    canvas.fill_rounded_rect(
        card_x + 16,
        field_y,
        SEARCH_CARD_W - 32,
        28,
        rae_tokens::RADIUS_SM as usize,
        DARK.bg_base,
    );
    let q = app.search_query();
    let shown = if q.is_empty() { "Type to search…" } else { q };
    let q_color = if q.is_empty() { TEXT_MUTED } else { TEXT_FG };
    canvas.draw_text_aa(
        (card_x + 26) as i32,
        (field_y + 6) as i32,
        shown,
        rae_tokens::TYPE_BODY,
        q_color,
        FontFamily::Sans,
    );

    // Summary header line (above the rows): "N results".
    let summary_y = field_y + 28 + 16;
    if app.search_ran && app.search_summary.total > 0 {
        let mut buf = [0u8; 64];
        let n = fmt_results_line(app.search_summary.total, &mut buf);
        if let Ok(s) = core::str::from_utf8(&buf[..n]) {
            canvas.draw_text_aa(
                (card_x + 16) as i32,
                summary_y as i32,
                s,
                rae_tokens::TYPE_CAPTION,
                accent(),
                FontFamily::Sans,
            );
        }
    }

    // Result rows / empty states.
    if !app.search_ran {
        canvas.draw_text_aa(
            list_x as i32,
            list_y as i32,
            "Press Enter to search the index. Esc to close.",
            rae_tokens::TYPE_CAPTION,
            TEXT_MUTED,
            FontFamily::Sans,
        );
    } else if app.search_results.is_empty() {
        canvas.draw_text_aa(
            list_x as i32,
            list_y as i32,
            "No results",
            rae_tokens::TYPE_BODY,
            TEXT_MUTED,
            FontFamily::Sans,
        );
    } else {
        let total = app.search_results.len();
        let top = search_scroll_top(app.search_selected, total);
        for vis in 0..SEARCH_VISIBLE_ROWS {
            let idx = top + vis;
            if idx >= total {
                break;
            }
            let hit = &app.search_results[idx];
            let row_y = list_y + vis * SEARCH_ROW_H;
            // Selection highlight.
            if idx == app.search_selected {
                canvas.fill_rounded_rect(
                    list_x,
                    row_y,
                    list_w,
                    SEARCH_ROW_H - 4,
                    rae_tokens::RADIUS_SM as usize,
                    DARK.bg_base,
                );
            }
            // Real file-type line-icon (`raegfx::icon`), tinted by the §4.4
            // palette — replaces the old letter/block placeholder.
            let ft = ftype_for_resolved(hit);
            canvas.draw_icon(
                ftype_icon(ft),
                (list_x + 8) as i32,
                (row_y + 4) as i32,
                16,
                ftype_color(ft),
            );
            // Leaf name (the row title) — prefer the resolver's name, fall back to
            // the path's leaf so a name-less row still shows something truthful.
            let name: &str = if !hit.name.is_empty() {
                hit.name.as_str()
            } else {
                leaf_of(&hit.path)
            };
            canvas.draw_text_aa(
                (list_x + 44) as i32,
                (row_y + 2) as i32,
                name,
                rae_tokens::TYPE_BODY,
                TEXT_FG,
                FontFamily::Sans,
            );
            // Path (dimmed secondary) — empty for app/setting hits.
            if !hit.path.is_empty() {
                canvas.draw_text_aa(
                    (list_x + 44) as i32,
                    (row_y + 18) as i32,
                    hit.path.as_str(),
                    rae_tokens::TYPE_CAPTION,
                    TEXT_MUTED,
                    FontFamily::Sans,
                );
            }
        }
    }

    // Footer hint.
    canvas.draw_text_aa(
        (card_x + 16) as i32,
        (card_y + SEARCH_CARD_H - 22) as i32,
        "Enter opens  -  Up/Down selects  -  Esc closes",
        rae_tokens::TYPE_CAPTION,
        TEXT_MUTED,
        FontFamily::Sans,
    );
}

/// Format "N result" / "N results" into `out`; returns the byte length.
fn fmt_results_line(total: usize, out: &mut [u8]) -> usize {
    let mut n = 0usize;
    let mut num = [0u8; 20];
    let k = fmt_u64(total as u64, &mut num);
    for &b in &num[..k] {
        if n < out.len() {
            out[n] = b;
            n += 1;
        }
    }
    let suffix: &[u8] = if total == 1 { b" result" } else { b" results" };
    for &b in suffix {
        if n < out.len() {
            out[n] = b;
            n += 1;
        }
    }
    n
}

/// Format a "<label>: <count>" breakdown row into `out`; returns byte length.
fn fmt_kind_row(label: &str, count: usize, out: &mut [u8]) -> usize {
    let mut n = 0usize;
    for &b in label.as_bytes() {
        if n < out.len() {
            out[n] = b;
            n += 1;
        }
    }
    for &b in b": " {
        if n < out.len() {
            out[n] = b;
            n += 1;
        }
    }
    let mut num = [0u8; 20];
    let k = fmt_u64(count as u64, &mut num);
    for &b in &num[..k] {
        if n < out.len() {
            out[n] = b;
            n += 1;
        }
    }
    n
}

/// Render the "Open With" overlay: a centered glass card listing the resolved
/// rae_mime candidate app ids for the selected file, the default (index 0) drawn
/// bold/accented and first, the highlighted row inverted. The footer shows the
/// resolved MIME type + the key hints (Enter = open, d = set default, Esc).
fn render_open_with(app: &App, canvas: &mut Canvas) {
    canvas.fill_rect(0, 0, WIN_W, WIN_H, OVERLAY_SCRIM);

    let card_w = 360usize;
    let row_h = 28usize;
    let header_h = 56usize;
    let footer_h = 44usize;
    let n = app.openwith_candidates.len().max(1);
    let card_h = header_h + n * row_h + footer_h;
    let card_x = (WIN_W - card_w) / 2;
    let card_y = (WIN_H.saturating_sub(card_h)) / 2;

    canvas.fill_rounded_rect(
        card_x,
        card_y,
        card_w,
        card_h,
        rae_tokens::RADIUS_MD as usize,
        DARK.bg_overlay,
    );

    // Header: "Open With" + the target file name.
    canvas.draw_text_aa(
        (card_x + 16) as i32,
        (card_y + 12) as i32,
        "Open With",
        rae_tokens::TYPE_SUBTITLE,
        TEXT_FG,
        FontFamily::Sans,
    );
    let name = core::str::from_utf8(&app.openwith_name[..app.openwith_name_len]).unwrap_or("?");
    canvas.draw_text_aa(
        (card_x + 16) as i32,
        (card_y + 32) as i32,
        name,
        rae_tokens::TYPE_CAPTION,
        TEXT_MUTED,
        FontFamily::Sans,
    );

    // Candidate rows.
    let acc = accent();
    for (i, app_id) in app.openwith_candidates.iter().enumerate() {
        let ry = card_y + header_h + i * row_h;
        let highlighted = i == app.openwith_selected;
        if highlighted {
            canvas.fill_rect(card_x + 8, ry, card_w - 16, row_h, row_sel());
        }
        let is_default = i == 0;
        let label_color = if highlighted {
            0xFF_FF_FF_FF
        } else if is_default {
            acc
        } else {
            TEXT_FG
        };
        // The default carries a leading bullet so it reads as "default" even
        // without color (a11y); the typeface is the heavier label style.
        let style = if is_default {
            rae_tokens::TYPE_LABEL
        } else {
            rae_tokens::TYPE_BODY
        };
        let mut text_x = card_x + 16;
        if is_default {
            canvas.draw_text_aa(
                text_x as i32,
                (ry + (row_h.saturating_sub(style.line_height as usize)) / 2) as i32,
                "*",
                style,
                label_color,
                FontFamily::Sans,
            );
            text_x += 14;
        }
        canvas.draw_text_aa(
            text_x as i32,
            (ry + (row_h.saturating_sub(style.line_height as usize)) / 2) as i32,
            app_id,
            style,
            label_color,
            FontFamily::Sans,
        );
    }

    // Footer: resolved MIME type + key hints.
    let fy = card_y + header_h + n * row_h + 8;
    canvas.draw_text_aa(
        (card_x + 16) as i32,
        fy as i32,
        app.openwith_mime.as_str(),
        rae_tokens::TYPE_CAPTION,
        TEXT_MUTED,
        FontFamily::Sans,
    );
    canvas.draw_text_aa(
        (card_x + 16) as i32,
        (fy + 16) as i32,
        "Enter: open   d: set default   Esc: cancel",
        rae_tokens::TYPE_CAPTION,
        TEXT_MUTED,
        FontFamily::Sans,
    );
}

/// Draw the tab strip from the syscall-free view snapshot (the pure counterpart
/// of the old `render_tabs(&App, ..)`). Same geometry the tab hit-test uses.
fn render_tabs_preview(state: &FilesViewState, canvas: &mut Canvas) {
    let ty = TITLE_H;
    // The tab strip sits ON the frosted `glass.chrome` band drawn by the window
    // chrome pass (no opaque fill here — the aurora reads through, IDENTITY §7).
    let count = state.tabs.len();
    let tab_w = 120usize;
    for i in 0..count {
        let tx = 4 + i * (tab_w + 2);
        if tx + tab_w > WIN_W - 30 {
            break;
        }
        let is_active = i == state.active_tab;
        // Translucent tab pills over the frosted chrome: the active tab gets a
        // frost-white lift (reads raised), inactive tabs a faint translucent slate
        // so the aurora still bleeds through the chrome band (IDENTITY §7).
        let bg = if is_active {
            rae_tokens::GLASS_POPOVER_DARK.frost
        } else {
            0x22_FF_FF_FF
        };
        canvas.fill_rounded_rect(
            tx,
            ty + 3,
            tab_w,
            TABBAR_H - 4,
            rae_tokens::RADIUS_XS as usize,
            bg,
        );
        if is_active {
            canvas.fill_rect(
                tx + 6,
                ty + TABBAR_H - 2,
                tab_w - 12,
                2,
                accent_s(state.theme_seed),
            );
        }
        // Tab = leading folder line-icon + the leaf dir name. Without the icon
        // a lone tab read as a stray text input over the chrome (visual-QA).
        let label = state.tabs.get(i).map(|t| leaf(t.as_str())).unwrap_or("/");
        let lty = (ty
            + (TABBAR_H.saturating_sub(rae_tokens::TYPE_CAPTION.line_height as usize)) / 2)
            as i32;
        let fg = if is_active { TEXT_FG } else { TEXT_MUTED };
        let tab_icon_sz = 12i32;
        let tab_icon_y = (ty + (TABBAR_H.saturating_sub(tab_icon_sz as usize)) / 2) as i32;
        canvas.draw_icon(
            ftype_icon(FType::Dir),
            (tx + 8) as i32,
            tab_icon_y,
            tab_icon_sz,
            fg,
        );
        canvas.draw_text_aa(
            (tx + 8 + tab_icon_sz as usize + 6) as i32,
            lty,
            label,
            rae_tokens::TYPE_CAPTION,
            fg,
            FontFamily::Sans,
        );
    }
    // "+" new-tab affordance — docked after the last tab (geom_new_tab is the
    // shared hit-test authority), drawn with the real Plus line-icon.
    let plus = geom_new_tab(count);
    canvas.fill_rounded_rect(
        plus.x,
        plus.y,
        plus.w,
        plus.h,
        rae_tokens::RADIUS_XS as usize,
        0x22_FF_FF_FF,
    );
    canvas.draw_icon(
        raegfx::icon::Icon::Plus,
        (plus.x + (plus.w.saturating_sub(12)) / 2) as i32,
        (plus.y + (plus.h.saturating_sub(12)) / 2) as i32,
        12,
        TEXT_FG,
    );
}

/// Last path component for a tab label.
fn leaf(path: &str) -> &str {
    if path == "/" || path.is_empty() {
        return "/";
    }
    let trimmed = path.trim_end_matches('/');
    match trimmed.rfind('/') {
        Some(i) => &trimmed[i + 1..],
        None => trimmed,
    }
}

// ── Compare (diff) overlay geometry + rendering ──────────────────────────────
//
// The overlay is a near-full-window panel (same scrim + rounded-panel idiom as
// Quick Look). Geometry lives in `const fn`s so the render path and the
// scroll-clamp math (`compare_visible_rows`) can never drift.

const COMPARE_PANEL_X: usize = 40;
const COMPARE_PANEL_Y: usize = 40;
const COMPARE_PANEL_W: usize = WIN_W - 80;
const COMPARE_PANEL_H: usize = WIN_H - 80;
/// Y where the first diff line is drawn (below the two-name header).
const COMPARE_BODY_Y: usize = COMPARE_PANEL_Y + 56;
/// Bottom of the scrollable body (above the hint footer).
const COMPARE_BODY_BOTTOM: usize = COMPARE_PANEL_Y + COMPARE_PANEL_H - 26;
/// Per-line advance for the monospace diff body.
const COMPARE_LINE_H: usize = 14;

/// Number of diff lines that fit in the scrollable body — the single source of
/// truth shared by the renderer and the scroll clamp.
fn compare_visible_rows() -> usize {
    COMPARE_BODY_BOTTOM.saturating_sub(COMPARE_BODY_Y) / COMPARE_LINE_H
}

/// Classify a unified-diff body line by its leading byte into a token color:
/// `+` added → `state_ok` (green), `-` removed → `state_danger` (red),
/// `@` hunk header → the live accent, everything else (` ` context, `\`) →
/// `text_tertiary` (dim). The single source of truth the design_proof asserts.
fn diff_line_color(first: Option<u8>) -> u32 {
    match first {
        Some(b'+') => DARK.state_ok,
        Some(b'-') => DARK.state_danger,
        Some(b'@') => accent(),
        _ => DARK.text_tertiary,
    }
}

/// Truncate `s` to at most `max_chars` characters on a CHAR BOUNDARY (never
/// splits a multi-byte codepoint), so a very long diff line stays inside the
/// panel and columns don't smear off the right edge.
fn truncate_chars(s: &str, max_chars: usize) -> &str {
    let mut count = 0usize;
    for (byte_idx, _) in s.char_indices() {
        if count == max_chars {
            return &s[..byte_idx];
        }
        count += 1;
    }
    s
}

fn render_compare(app: &App, canvas: &mut Canvas) {
    // Scrim + centered panel (same idiom as Quick Look).
    canvas.fill_rect(0, 0, WIN_W, WIN_H, OVERLAY_SCRIM);
    canvas.fill_rounded_rect(
        COMPARE_PANEL_X,
        COMPARE_PANEL_Y,
        COMPARE_PANEL_W,
        COMPARE_PANEL_H,
        rae_tokens::RADIUS_LG as usize,
        DARK.bg_overlay,
    );
    canvas.draw_rounded_rect_outline(
        COMPARE_PANEL_X,
        COMPARE_PANEL_Y,
        COMPARE_PANEL_W,
        COMPARE_PANEL_H,
        rae_tokens::RADIUS_LG as usize,
        STROKE_HL,
    );

    // Header: title + "<a>  ->  <b>".
    canvas.draw_text_aa(
        (COMPARE_PANEL_X + 16) as i32,
        (COMPARE_PANEL_Y + 12) as i32,
        "Compare",
        rae_tokens::TYPE_SUBTITLE,
        TEXT_FG,
        FontFamily::Sans,
    );
    let hx = (COMPARE_PANEL_X + 16) as i32;
    let hy = (COMPARE_PANEL_Y + 34) as i32;
    let aw = canvas.draw_text_aa(
        hx,
        hy,
        app.compare_name_a_str(),
        rae_tokens::TYPE_CAPTION,
        DARK.state_danger,
        FontFamily::Mono,
    );
    let arrow_w = canvas.draw_text_aa(
        hx + aw + 6,
        hy,
        "->",
        rae_tokens::TYPE_CAPTION,
        TEXT_MUTED,
        FontFamily::Sans,
    );
    canvas.draw_text_aa(
        hx + aw + 6 + arrow_w + 6,
        hy,
        app.compare_name_b_str(),
        rae_tokens::TYPE_CAPTION,
        DARK.state_ok,
        FontFamily::Mono,
    );

    // Body: color-coded, scrollable, monospace diff lines.
    let visible = compare_visible_rows();
    // Roughly how many monospace chars fit the body width (advance ~7px at this
    // size); used only for a generous char-boundary-safe truncation.
    let max_chars = (COMPARE_PANEL_W.saturating_sub(32)) / 7;
    let mut y = COMPARE_BODY_Y;
    for line in app
        .compare_diff
        .split(|&b| b == b'\n')
        .skip(app.compare_scroll)
        .take(visible)
    {
        let first = line.first().copied();
        let color = diff_line_color(first);
        if let Ok(s) = core::str::from_utf8(line) {
            let s = truncate_chars(s, max_chars);
            canvas.draw_text_aa(
                (COMPARE_PANEL_X + 16) as i32,
                y as i32,
                s,
                rae_tokens::TYPE_CAPTION,
                color,
                FontFamily::Mono,
            );
        }
        y += COMPARE_LINE_H;
    }

    // Scroll position + hint footer.
    let total = app.compare_line_count();
    if total > visible {
        let mut pbuf = [0u8; 48];
        let pn = fmt_scroll_pos(app.compare_scroll, total, &mut pbuf);
        if let Ok(s) = core::str::from_utf8(&pbuf[..pn]) {
            canvas.draw_text_aa(
                (COMPARE_PANEL_X + 16) as i32,
                (COMPARE_PANEL_Y + COMPARE_PANEL_H - 22) as i32,
                s,
                rae_tokens::TYPE_CAPTION,
                TEXT_MUTED,
                FontFamily::Sans,
            );
        }
    }
    let hint = "Up/Down/PgUp/PgDn: scroll   Esc: close";
    let hw = canvas.measure_text_aa(hint, rae_tokens::TYPE_CAPTION, FontFamily::Sans);
    canvas.draw_text_aa(
        (COMPARE_PANEL_X + COMPARE_PANEL_W - 16) as i32 - hw,
        (COMPARE_PANEL_Y + COMPARE_PANEL_H - 22) as i32,
        hint,
        rae_tokens::TYPE_CAPTION,
        TEXT_MUTED,
        FontFamily::Sans,
    );
}

/// Format "line A-B / N" scroll position into `out`.
fn fmt_scroll_pos(scroll: usize, total: usize, out: &mut [u8]) -> usize {
    let mut n = 0usize;
    let mut put = |s: &[u8], n: &mut usize| {
        for &b in s {
            if *n < out.len() {
                out[*n] = b;
                *n += 1;
            }
        }
    };
    put(b"line ", &mut n);
    let mut num = [0u8; 20];
    let k = fmt_u64((scroll + 1) as u64, &mut num);
    put(&num[..k], &mut n);
    put(b"/", &mut n);
    let k = fmt_u64(total as u64, &mut num);
    put(&num[..k], &mut n);
    n
}

fn render_quick_look(app: &App, canvas: &mut Canvas) {
    // Scrim + centered panel.
    canvas.fill_rect(0, 0, WIN_W, WIN_H, OVERLAY_SCRIM);
    let pw = WIN_W - 120;
    let ph = WIN_H - 120;
    let px = 60;
    let py = 60;
    canvas.fill_rounded_rect(
        px,
        py,
        pw,
        ph,
        rae_tokens::RADIUS_LG as usize,
        DARK.bg_overlay,
    );
    canvas.draw_rounded_rect_outline(px, py, pw, ph, rae_tokens::RADIUS_LG as usize, STROKE_HL);

    // Header
    let name = app
        .entries()
        .get(app.selected)
        .map(App::entry_name)
        .unwrap_or("");
    canvas.draw_text_aa(
        (px + 16) as i32,
        (py + 12) as i32,
        "Quick Look",
        rae_tokens::TYPE_SUBTITLE,
        TEXT_FG,
        FontFamily::Sans,
    );
    canvas.draw_text_aa(
        (px + 16) as i32,
        (py + 34) as i32,
        name,
        rae_tokens::TYPE_CAPTION,
        accent(),
        FontFamily::Sans,
    );

    let body_x = px + 16;
    let body_y = py + 58;
    let body_w = pw - 32;
    let body_bottom = py + ph - 28;

    if let Some(doc) = app.preview_doc.as_ref() {
        // Extracted document text (PDF/DOCX): a scrollable, line-by-line view.
        // The header already shows the file name; the format label sits to the
        // right of "Quick Look" so the user knows what engine opened it.
        let lw = canvas.measure_text_aa("Quick Look", rae_tokens::TYPE_SUBTITLE, FontFamily::Sans);
        canvas.draw_text_aa(
            (px + 16) as i32 + lw + 10,
            (py + 14) as i32,
            app.preview_doc_label,
            rae_tokens::TYPE_CAPTION,
            TEXT_MUTED,
            FontFamily::Sans,
        );
        render_doc_text(
            doc.as_bytes(),
            app.preview_doc_scroll,
            canvas,
            body_x,
            body_y,
            body_w,
            body_bottom,
        );
    } else if let Some(csv) = app.preview_csv.as_ref() {
        // Aligned table view: header row in accent with a separator rule, data
        // rows in the mono font, columns padded/truncated to a computed width.
        render_csv_table(
            csv,
            app.preview_csv_scroll,
            canvas,
            body_x,
            body_y,
            body_w,
            body_bottom,
        );
    } else if let Some(img) = app.preview_image.as_ref() {
        // Real pixels: scale-to-fit the decoded PNG into the body frame,
        // letterboxed on the token background, aspect ratio preserved.
        blit_image_fit(
            canvas,
            img,
            body_x,
            body_y,
            body_w,
            body_bottom.saturating_sub(body_y),
        );
    } else if app.preview_is_text {
        // Render the text body line by line.
        render_text_preview(
            &app.preview[..app.preview_len],
            canvas,
            body_x,
            body_y,
            body_w,
            py + ph - 16,
        );
    } else {
        // Binary / image summary: size + (PNG) dimensions + a hex peek.
        render_binary_summary(app, canvas, body_x, body_y, body_w);
    }

    let hint = if app.preview_csv.is_some() || app.preview_doc.is_some() {
        "Up/Down/PgUp/PgDn: scroll   Esc: close"
    } else {
        "Space/Esc: close"
    };
    let hw = canvas.measure_text_aa(hint, rae_tokens::TYPE_CAPTION, FontFamily::Sans);
    canvas.draw_text_aa(
        (px + pw - 16) as i32 - hw,
        (py + ph - 22) as i32,
        hint,
        rae_tokens::TYPE_CAPTION,
        TEXT_MUTED,
        FontFamily::Sans,
    );
}

fn render_text_preview(
    buf: &[u8],
    canvas: &mut Canvas,
    x: usize,
    mut y: usize,
    _w: usize,
    max_y: usize,
) {
    let line_h = rae_tokens::TYPE_BODY.line_height as usize + 2;
    let mut line_start = 0usize;
    for i in 0..=buf.len() {
        let at_end = i == buf.len();
        if at_end || buf[i] == b'\n' {
            if y + line_h > max_y {
                canvas.draw_text_aa(
                    x as i32,
                    y as i32,
                    "...",
                    rae_tokens::TYPE_BODY,
                    TEXT_MUTED,
                    FontFamily::Sans,
                );
                break;
            }
            let line = &buf[line_start..i];
            // Trim trailing CR.
            let line = if line.last() == Some(&b'\r') {
                &line[..line.len() - 1]
            } else {
                line
            };
            if let Ok(s) = core::str::from_utf8(line) {
                canvas.draw_text_aa(
                    x as i32,
                    y as i32,
                    s,
                    rae_tokens::TYPE_BODY,
                    TEXT_FG,
                    FontFamily::Mono,
                );
            }
            y += line_h;
            line_start = i + 1;
            if at_end {
                break;
            }
        }
    }
}

/// Render extracted DOCUMENT text (PDF/DOCX) in the Quick Look body, scrolled so
/// the first `scroll` lines are skipped. Lines split on `\n`; the PDF page break
/// (`\u{0C}` form-feed) renders as a dim "— page N —" rule so a multi-page PDF
/// reads as pages, not one wall of text. Bounded by `max_y` (an ellipsis when the
/// body fills). Never panics — non-UTF-8 line fragments are skipped, not sliced.
fn render_doc_text(
    buf: &[u8],
    scroll: usize,
    canvas: &mut Canvas,
    x: usize,
    start_y: usize,
    _w: usize,
    max_y: usize,
) {
    let line_h = rae_tokens::TYPE_BODY.line_height as usize + 2;
    let mut y = start_y;
    let mut page = 1usize;
    let mut visible_line = 0usize; // counts logical lines AFTER the scroll skip
    let mut skipped = 0usize;
    let mut line_start = 0usize;

    for i in 0..=buf.len() {
        let at_end = i == buf.len();
        let is_break = !at_end && buf[i] == b'\n';
        let is_page = !at_end && buf[i] == 0x0C; // form-feed = PDF page boundary
        if at_end || is_break || is_page {
            // Skip the first `scroll` logical lines (page rules count as lines).
            if skipped < scroll {
                skipped += 1;
                if is_page {
                    page += 1;
                }
                line_start = i + 1;
                if at_end {
                    break;
                }
                continue;
            }
            if y + line_h > max_y {
                canvas.draw_text_aa(
                    x as i32,
                    y as i32,
                    "...",
                    rae_tokens::TYPE_BODY,
                    TEXT_MUTED,
                    FontFamily::Sans,
                );
                break;
            }
            if is_page {
                page += 1;
                // Draw a dim page-boundary marker.
                let mut marker = String::from("— page ");
                push_usize(&mut marker, page);
                marker.push_str(" —");
                canvas.draw_text_aa(
                    x as i32,
                    y as i32,
                    &marker,
                    rae_tokens::TYPE_CAPTION,
                    TEXT_MUTED,
                    FontFamily::Sans,
                );
            } else {
                let line = &buf[line_start..i];
                let line = if line.last() == Some(&b'\r') {
                    &line[..line.len() - 1]
                } else {
                    line
                };
                if let Ok(s) = core::str::from_utf8(line) {
                    canvas.draw_text_aa(
                        x as i32,
                        y as i32,
                        s,
                        rae_tokens::TYPE_BODY,
                        TEXT_FG,
                        FontFamily::Sans,
                    );
                }
            }
            y += line_h;
            visible_line += 1;
            let _ = visible_line;
            line_start = i + 1;
            if at_end {
                break;
            }
        }
    }
}

/// Count the logical lines in extracted document text: one per `\n` and one per
/// PDF page break (`\u{0C}`), plus a trailing partial line if the buffer doesn't
/// end on a separator. Used to clamp the doc-view scroll. An empty buffer is 0.
fn doc_line_count(buf: &[u8]) -> usize {
    if buf.is_empty() {
        return 0;
    }
    let mut lines = 0usize;
    let mut tail = false;
    for &b in buf {
        if b == b'\n' || b == 0x0C {
            lines += 1;
            tail = false;
        } else {
            tail = true;
        }
    }
    if tail {
        lines += 1;
    }
    lines
}

// ── CSV/TSV aligned table view (Quick Look) ──────────────────────────────────
//
// A `.csv`/`.tsv` opens as an aligned table instead of raw text: the first row is
// the header (accent color + a separator rule under it), data rows below in the
// monospace font with columns padded/truncated to a per-column display width.
// Memory is bounded (the parser caps rows/cols/cells; we additionally cap the
// RENDER to `CSV_MAX_RENDER_ROWS` for width computation + draw). All truncation is
// char-boundary-safe via `truncate_cell` (never slices mid-codepoint).

/// Max display width (in chars) of any single column. Wider cells are truncated
/// with an ellipsis so one fat column can't push the rest off-screen.
const CSV_COL_CAP: usize = 24;
/// Max number of rows (header + data) the table considers for width computation
/// and draws. Huge files render their first slice; the parser already bounds
/// total memory, this bounds the per-frame render work.
const CSV_MAX_RENDER_ROWS: usize = 500;

/// Truncate `cell` to at most `cap` display chars, appending a single-char
/// ellipsis ('…') when it overflows. Char-boundary-safe: counts and slices on
/// `char_indices`, never on raw byte offsets, so multibyte UTF-8 is never split.
/// The returned string is at most `cap` chars wide (the '…' replaces the last
/// kept char so the total still fits `cap`).
fn truncate_cell(cell: &str, cap: usize) -> alloc::string::String {
    if cap == 0 {
        return alloc::string::String::new();
    }
    // Fast path: already within cap (count chars without allocating).
    let nchars = cell.chars().count();
    if nchars <= cap {
        return alloc::string::String::from(cell);
    }
    // Keep `cap - 1` chars, then append the ellipsis (total = cap chars).
    let keep = cap - 1;
    let mut out = alloc::string::String::new();
    for (idx, c) in cell.chars().enumerate() {
        if idx >= keep {
            break;
        }
        out.push(c);
    }
    out.push('…');
    out
}

/// The display width (chars) of `cell` after the column cap is applied. Mirrors
/// what `truncate_cell` would produce, so width computation and rendering agree.
fn cell_display_width(cell: &str, cap: usize) -> usize {
    cell.chars().count().min(cap)
}

/// Compute per-column display widths for `csv`: for each column, the max
/// capped-display-width of any cell in that column over the first
/// `CSV_MAX_RENDER_ROWS` rows. Ragged rows are safe (a short row simply doesn't
/// contribute to columns it lacks). Each width is clamped to `CSV_COL_CAP`.
fn csv_column_widths(csv: &rae_csv::Csv) -> Vec<usize> {
    let cols = csv.cols();
    let mut widths: Vec<usize> = Vec::new();
    widths.resize(cols, 1); // min width 1 so an all-empty column still draws a gap
    let row_limit = csv.len().min(CSV_MAX_RENDER_ROWS);
    for r in 0..row_limit {
        for c in 0..cols {
            let cell = csv.cell(r, c).unwrap_or("");
            let w = cell_display_width(cell, CSV_COL_CAP);
            if w > widths[c] {
                widths[c] = w;
            }
        }
    }
    widths
}

/// Build one rendered table line: each cell truncated to its column cap, padded
/// with spaces to its column width, columns separated by "  " (two spaces). Pads
/// in CHARS (mono font → uniform advance), so columns align. Ragged-safe: missing
/// cells render as empty (padded) fields.
fn csv_render_row(csv: &rae_csv::Csv, row: usize, widths: &[usize]) -> alloc::string::String {
    let mut line = alloc::string::String::new();
    for (c, &w) in widths.iter().enumerate() {
        if c > 0 {
            line.push_str("  ");
        }
        let cell = csv.cell(row, c).unwrap_or("");
        let shown = truncate_cell(cell, w);
        let shown_w = shown.chars().count();
        line.push_str(&shown);
        for _ in shown_w..w {
            line.push(' ');
        }
    }
    line
}

#[allow(clippy::too_many_arguments)]
fn render_csv_table(
    csv: &rae_csv::Csv,
    scroll: usize,
    canvas: &mut Canvas,
    x: usize,
    y: usize,
    w: usize,
    max_y: usize,
) {
    let widths = csv_column_widths(csv);
    let line_h = rae_tokens::TYPE_BODY.line_height as usize + 2;

    // "N rows × M cols" summary (data rows = total minus the header row).
    let total_rows = csv.len();
    let data_rows = total_rows.saturating_sub(1);
    let cols = csv.cols();
    let mut hdr = alloc::string::String::new();
    push_usize(&mut hdr, data_rows);
    hdr.push_str(" rows × ");
    push_usize(&mut hdr, cols);
    hdr.push_str(" cols");
    if total_rows.min(CSV_MAX_RENDER_ROWS) < total_rows {
        hdr.push_str("  (showing first ");
        push_usize(&mut hdr, CSV_MAX_RENDER_ROWS);
        hdr.push(')');
    }
    canvas.draw_text_aa(
        x as i32,
        y as i32,
        &hdr,
        rae_tokens::TYPE_CAPTION,
        TEXT_MUTED,
        FontFamily::Sans,
    );

    let mut yy = y + line_h;

    // Header row (first table row) in accent + a separator rule under it.
    let header_line = csv_render_row(csv, 0, &widths);
    canvas.draw_text_aa(
        x as i32,
        yy as i32,
        &header_line,
        rae_tokens::TYPE_BODY,
        accent(),
        FontFamily::Mono,
    );
    yy += rae_tokens::TYPE_BODY.line_height as usize;
    canvas.fill_rect(x, yy, w, 1, STROKE_HL);
    yy += 3;

    // Data rows, scrolled. Row 0 is the header → data starts at row index 1.
    let render_limit = csv.len().min(CSV_MAX_RENDER_ROWS);
    let first_data = 1 + scroll;
    let mut alt = false;
    let mut row = first_data;
    while row < render_limit {
        if yy + line_h > max_y {
            canvas.draw_text_aa(
                x as i32,
                yy as i32,
                "…",
                rae_tokens::TYPE_BODY,
                TEXT_MUTED,
                FontFamily::Sans,
            );
            break;
        }
        // Subtle alternating-row background (cheap: a thin fill behind the line).
        if alt {
            canvas.fill_rect(x, yy.saturating_sub(1), w, line_h, DARK.bg_elevated);
        }
        let line = csv_render_row(csv, row, &widths);
        canvas.draw_text_aa(
            x as i32,
            yy as i32,
            &line,
            rae_tokens::TYPE_BODY,
            TEXT_FG,
            FontFamily::Mono,
        );
        yy += line_h;
        alt = !alt;
        row += 1;
    }
}

/// Append a `usize` to a `String` in base-10 without `alloc::format!` overhead /
/// any panic path. Used for the table's "N rows × M cols" summary.
fn push_usize(s: &mut alloc::string::String, mut n: usize) {
    if n == 0 {
        s.push('0');
        return;
    }
    let mut digits = [0u8; 20];
    let mut i = 0;
    while n > 0 {
        digits[i] = b'0' + (n % 10) as u8;
        n /= 10;
        i += 1;
    }
    while i > 0 {
        i -= 1;
        s.push(digits[i] as char);
    }
}

fn render_binary_summary(app: &App, canvas: &mut Canvas, x: usize, y: usize, _w: usize) {
    let mut buf = [0u8; 64];
    // Size line.
    let n = fmt_size(app.preview_total, &mut buf);
    let mut line = [0u8; 80];
    let mut ln = 0;
    for &b in b"Size: " {
        line[ln] = b;
        ln += 1;
    }
    for &b in &buf[..n] {
        line[ln] = b;
        ln += 1;
    }
    if let Ok(s) = core::str::from_utf8(&line[..ln]) {
        canvas.draw_text_aa(
            x as i32,
            y as i32,
            s,
            rae_tokens::TYPE_BODY,
            TEXT_FG,
            FontFamily::Sans,
        );
    }

    // PNG dimensions if it's a PNG.
    let mut yy = y + 22;
    if let Some((w, h)) = png_dimensions(&app.preview[..app.preview_len]) {
        let mut dbuf = [0u8; 48];
        let dn = fmt_dims(w, h, &mut dbuf);
        if let Ok(s) = core::str::from_utf8(&dbuf[..dn]) {
            canvas.draw_text_aa(
                x as i32,
                yy as i32,
                s,
                rae_tokens::TYPE_BODY,
                accent(),
                FontFamily::Sans,
            );
        }
        yy += 22;
    }

    // Hex peek of the first bytes.
    canvas.draw_text_aa(
        x as i32,
        yy as i32,
        "First bytes (hex):",
        rae_tokens::TYPE_CAPTION,
        TEXT_MUTED,
        FontFamily::Sans,
    );
    yy += 18;
    let take = app.preview_len.min(128);
    let mut row = [0u8; 48];
    let mut col = 0usize;
    let mut rn = 0usize;
    for &b in &app.preview[..take] {
        let hi = HEX[(b >> 4) as usize];
        let lo = HEX[(b & 0xF) as usize];
        if rn + 3 < row.len() {
            row[rn] = hi;
            row[rn + 1] = lo;
            row[rn + 2] = b' ';
            rn += 3;
        }
        col += 1;
        if col == 16 {
            if let Ok(s) = core::str::from_utf8(&row[..rn]) {
                canvas.draw_text_aa(
                    x as i32,
                    yy as i32,
                    s,
                    rae_tokens::TYPE_CAPTION,
                    TEXT_FG,
                    FontFamily::Mono,
                );
            }
            yy += 16;
            rn = 0;
            col = 0;
        }
    }
    if rn > 0 {
        if let Ok(s) = core::str::from_utf8(&row[..rn]) {
            canvas.draw_text_aa(
                x as i32,
                yy as i32,
                s,
                rae_tokens::TYPE_CAPTION,
                TEXT_FG,
                FontFamily::Mono,
            );
        }
    }
}

const HEX: &[u8; 16] = b"0123456789abcdef";

/// The 8-byte PNG signature (RaeMedia and the kernel agree on this).
const PNG_SIG: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

/// Hard cap on how large a file we slurp for in-app PNG decode. A PNG bigger
/// than this falls back to the dims/hex summary rather than pinning pages —
/// matches the decoder's own MAX_PIXELS posture (no unbounded allocation from
/// a single Quick Look).
const PNG_DECODE_CAP: usize = 8 * 1024 * 1024; // 8 MiB

/// True when `buf` begins with the PNG signature.
fn is_png_signature(buf: &[u8]) -> bool {
    buf.len() >= 8 && buf[..8] == PNG_SIG
}

/// True when `buf` begins with the JPEG Start-Of-Image magic `FF D8 FF`. The
/// third byte (the first marker after SOI, always `0xFF`-led) disambiguates JPEG
/// from a coincidental `FF D8` prefix on arbitrary binary.
fn is_jpeg_signature(buf: &[u8]) -> bool {
    buf.len() >= 3 && buf[0] == 0xFF && buf[1] == 0xD8 && buf[2] == 0xFF
}

/// True when `buf` begins with a GIF signature (`GIF87a` or `GIF89a`). The bytes
/// are the truth (not the extension); both versions share the `GIF8?a` shape.
fn is_gif_signature(buf: &[u8]) -> bool {
    buf.len() >= 6 && (&buf[..6] == b"GIF87a" || &buf[..6] == b"GIF89a")
}

/// Bridge a `rae_gif::GifFrame`'s composited ARGB8888 buffer into the
/// `png::DecodedImage` the Quick Look blit path consumes (a flat `0xAARRGGBB`
/// `Vec<u32>` with `width`/`height`). Quick Look uses frame 0 — a static preview.
fn gif_frame_to_canvas_image(width: u32, height: u32, pixels: Vec<u32>) -> DecodedImage {
    DecodedImage {
        width,
        height,
        pixels,
    }
}

/// Bridge a `raemedia::jpeg::DecodedImage` into the `png::DecodedImage` the
/// Quick Look blit path consumes. The two are structurally identical (both flat
/// `0xAARRGGBB` `Vec<u32>` with `width`/`height`), so this is a zero-logic field
/// move — it exists only to satisfy the type system and let JPEG reuse the EXACT
/// `blit_image_fit` scale/letterbox/blit code PNG already uses.
fn jpeg_to_canvas_image(img: raemedia::jpeg::DecodedImage) -> DecodedImage {
    DecodedImage {
        width: img.width,
        height: img.height,
        pixels: img.pixels,
    }
}

/// Read an entire file into a heap buffer for decoding. `prefix` is the chunk
/// already consumed from `fd` (the Quick Look preview read); we continue from
/// the file's current offset and append until EOF or the decode cap. Returns
/// `None` if nothing could be read or the file overran the cap.
fn read_whole_file(fd: u64, prefix: &[u8]) -> Option<Vec<u8>> {
    let mut data: Vec<u8> = Vec::new();
    data.extend_from_slice(prefix);
    let mut chunk = [0u8; PREVIEW_CAP];
    loop {
        if data.len() > PNG_DECODE_CAP {
            return None;
        }
        let n = raekit::sys::read(fd, &mut chunk) as usize;
        if n == 0 || n > chunk.len() {
            break;
        }
        data.extend_from_slice(&chunk[..n]);
    }
    if data.is_empty() {
        None
    } else {
        Some(data)
    }
}

// ── Document / preview open dispatch (PDF · DOCX · XLSX · image) ───────────────
//
// RaeenOS_Concept.md §Windows Pain Points "the modern file manager": opening a
// document should Just Work, in the right viewer, without a terminal. Files
// already previews images/text/CSV; this is the path that turns the orphaned
// document engines (rae_pdf / rae_docx / rae_xlsx) and the unified image decoder
// (rae_image) into a real "open this document" feature.
//
// Dispatch is by MAGIC BYTES (`rae_formats::detect`), never the extension — a PDF
// renamed `.txt` still opens as a PDF, and a `.png` that's really a JPEG decodes
// correctly. The pure core [`build_doc_preview`] takes the file bytes and returns
// a renderable model; the syscall shell ([`App::open_quick_look`]) just reads the
// file and feeds it in. Keeping the decision logic syscall-free is what lets the
// host `cargo test` exercise the EXACT open path with in-memory buffers.

/// Hard cap on how large a document we slurp for in-app text/grid extraction. A
/// document past this is summarized (size/hex) rather than fully decoded, so a
/// malicious or huge file never drives an unbounded allocation. The image path
/// keeps its own [`PNG_DECODE_CAP`].
const DOC_DECODE_CAP: usize = 16 * 1024 * 1024; // 16 MiB

/// A renderable Quick Look model produced by [`build_doc_preview`] from a file's
/// raw bytes. Each variant maps onto an EXISTING Files render path:
/// - [`DocPreview::Text`] → the line-by-line text preview (PDF/DOCX extraction).
/// - [`DocPreview::Csv`]  → the aligned CSV table view (XLSX as a grid).
/// - [`DocPreview::Image`] → the `blit_image_fit` ARGB8888 blit (any image kind).
/// - [`DocPreview::None`] → not a document we open here; caller falls back to the
///   existing text/hex summary. Returned for `Unknown`/unsupported kinds AND for a
///   recognized-but-undecodable file (a corrupt PDF, a password-protected DOCX),
///   so a malformed document degrades to a summary instead of an error wall.
pub enum DocPreview {
    /// Extracted plain text plus a short format label ("PDF"/"DOCX").
    Text { label: &'static str, text: String },
    /// A parsed grid to render as an aligned table (XLSX → CSV cell model).
    Csv(rae_csv::Csv),
    /// A decoded ARGB8888 bitmap for the image preview canvas.
    Image(DecodedImage),
    /// Not handled here — fall back to the existing preview behavior.
    None,
}

impl DocPreview {
    /// True when this carries something renderable (text, a non-empty grid, or an
    /// image). The host KAT asserts this is `true` for valid sample documents —
    /// the FAIL-able invariant that the open path produced real content, not just
    /// `Ok`.
    pub fn is_some(&self) -> bool {
        match self {
            DocPreview::Text { text, .. } => !text.is_empty(),
            DocPreview::Csv(csv) => !csv.is_empty(),
            DocPreview::Image(img) => img.width > 0 && img.height > 0 && !img.pixels.is_empty(),
            DocPreview::None => false,
        }
    }
}

/// The pure document-open core: sniff `bytes` by magic and dispatch to the right
/// engine, returning a [`DocPreview`] the Quick Look overlay can render. Never
/// panics — every engine is hostile-input safe and any decode failure collapses
/// to [`DocPreview::None`] (the caller's existing text/hex fallback). This is the
/// syscall-free seam the host `cargo test` drives with in-memory buffers.
pub fn build_doc_preview(bytes: &[u8]) -> DocPreview {
    match detect(bytes) {
        FileKind::Pdf => doc_preview_pdf(bytes),
        FileKind::Docx => doc_preview_docx(bytes),
        FileKind::Xlsx => doc_preview_xlsx(bytes),
        // A bare ZIP that the sniffer couldn't refine to DOCX/XLSX. This happens
        // for real-world OOXML whose FIRST local-file entry is `[Content_Types].xml`
        // rather than the `word/`/`xl/` tree the byte-sniffer keys on (the spec
        // permits either ordering). The engines key on the ZIP CENTRAL DIRECTORY
        // (`word/document.xml` / `xl/workbook.xml`), which is authoritative, so we
        // probe DOCX then XLSX; a plain (non-OOXML) ZIP misses both → None and the
        // existing archive/extract path handles it.
        FileKind::Zip => {
            let docx = doc_preview_docx(bytes);
            if docx.is_some() {
                docx
            } else {
                doc_preview_xlsx(bytes)
            }
        }
        // Every still-image kind the unified decoder supports. `rae_image::decode`
        // re-sniffs internally (same `rae_formats::detect`) and returns the shared
        // ARGB8888 `Image`, which is structurally identical to the `DecodedImage`
        // the existing `blit_image_fit` path consumes — a zero-logic field move.
        FileKind::Png | FileKind::Jpeg | FileKind::Bmp | FileKind::Gif | FileKind::Webp => {
            match rae_image::decode(bytes) {
                Ok(img) => DocPreview::Image(DecodedImage {
                    width: img.width,
                    height: img.height,
                    pixels: img.pixels,
                }),
                Err(_) => DocPreview::None,
            }
        }
        _ => DocPreview::None,
    }
}

/// Extract a PDF's text into a [`DocPreview::Text`]. A valid PDF with no
/// extractable text (a scanned/image-only document) yields [`DocPreview::None`]
/// so the caller falls back to the summary rather than showing a blank page.
fn doc_preview_pdf(bytes: &[u8]) -> DocPreview {
    match rae_pdf::Document::open(bytes) {
        Ok(doc) => {
            let text = doc.extract_text();
            if text.trim().is_empty() {
                DocPreview::None
            } else {
                DocPreview::Text { label: "PDF", text }
            }
        }
        Err(_) => DocPreview::None,
    }
}

/// Extract a DOCX's text (paragraphs/headings/tables) into a [`DocPreview::Text`].
/// A ZIP without `word/document.xml` is not a DOCX → [`DocPreview::None`].
fn doc_preview_docx(bytes: &[u8]) -> DocPreview {
    match rae_docx::Document::open(bytes) {
        Ok(doc) => {
            let text = doc.extract_text();
            if text.trim().is_empty() {
                DocPreview::None
            } else {
                DocPreview::Text {
                    label: "DOCX",
                    text,
                }
            }
        }
        Err(_) => DocPreview::None,
    }
}

/// Render an XLSX's first sheet as a CSV grid ([`DocPreview::Csv`]). `to_csv` is
/// the bounded, never-panic path; we re-parse it through the SAME `rae_csv` the
/// live CSV table view uses so the rendering is byte-identical to opening a `.csv`.
/// A ZIP without `xl/workbook.xml`, or an empty sheet, yields [`DocPreview::None`].
fn doc_preview_xlsx(bytes: &[u8]) -> DocPreview {
    match rae_xlsx::Workbook::open(bytes) {
        Ok(wb) => {
            let names = wb.sheet_names();
            let csv_text = names
                .first()
                .and_then(|n| wb.sheet(n))
                .map(|s| s.to_csv())
                .unwrap_or_default();
            if csv_text.is_empty() {
                DocPreview::None
            } else {
                match rae_csv::parse(&csv_text) {
                    Ok(csv) if !csv.is_empty() => DocPreview::Csv(csv),
                    _ => DocPreview::None,
                }
            }
        }
        Err(_) => DocPreview::None,
    }
}

/// Read up to [`DOC_DECODE_CAP`] raw bytes of the file at `path` into a heap
/// buffer for document decoding. Returns `None` when the file can't be opened, is
/// empty, or overruns the cap (so a multi-GB file never loads whole). Never
/// panics. Unlike [`read_file_text_capped`] this keeps the raw bytes (documents
/// are binary: PDFs and the ZIP-based OOXML formats are not UTF-8).
fn read_whole_file_path(path: &str, cap: usize) -> Option<Vec<u8>> {
    let fd = raekit::sys::open(path, 0);
    if fd == u64::MAX {
        return None;
    }
    let mut data: Vec<u8> = Vec::new();
    let mut chunk = [0u8; PREVIEW_CAP];
    loop {
        if data.len() > cap {
            let _ = raekit::sys::close(fd);
            return None;
        }
        let n = raekit::sys::read(fd, &mut chunk) as usize;
        if n == 0 || n > chunk.len() {
            break;
        }
        data.extend_from_slice(&chunk[..n]);
    }
    let _ = raekit::sys::close(fd);
    if data.is_empty() {
        None
    } else {
        Some(data)
    }
}

/// The four file-integrity digests of one file, as lowercase-hex strings ready
/// to render. Produced by [`hash_file_streaming`].
struct FileDigests {
    sha256: String,
    sha1: String,
    md5: String,
    crc32: String,
}

/// Block size for the streaming checksum read. 64 KiB keeps the resident buffer
/// tiny so a multi-GB file is hashed without ever being loaded whole into RAM.
const HASH_BLOCK: usize = 64 * 1024;

/// Stream the file at `path` through `rae_hash` in 64 KiB blocks, feeding every
/// block to all four streaming digests at once. Returns `None` only if the file
/// cannot be OPENED (a read that returns 0 at EOF on an empty file still yields
/// the four valid empty-input digests). Never panics — the buffer is bounded, no
/// slicing past the read length, and `rae_hash` is itself never-panic.
///
/// This is the heart of the "verify a download's integrity" action: a single
/// pass over the bytes computes SHA-256 (the primary), SHA-1, MD5, and CRC32, so
/// the user can match whichever form the publisher posted.
fn hash_file_streaming(path: &str) -> Option<FileDigests> {
    let fd = raekit::sys::open(path, 0);
    if fd == u64::MAX {
        return None;
    }
    let mut sha256 = rae_hash::Sha256::new();
    let mut sha1 = rae_hash::Sha1::new();
    let mut md5 = rae_hash::Md5::new();
    let mut crc32 = rae_hash::Crc32::new();

    // A heap block (not a stack array) so a larger future HASH_BLOCK never risks
    // the stack; freed when this function returns.
    let mut block = alloc::vec![0u8; HASH_BLOCK];
    loop {
        let n = raekit::sys::read(fd, &mut block) as usize;
        // A short/zero read is EOF; a bogus over-length return is clamped so we
        // never feed uninitialized tail bytes into a digest.
        if n == 0 {
            break;
        }
        let n = n.min(block.len());
        let chunk = &block[..n];
        sha256.update(chunk);
        sha1.update(chunk);
        md5.update(chunk);
        crc32.update(chunk);
    }
    let _ = raekit::sys::close(fd);

    Some(FileDigests {
        sha256: rae_hash::to_hex(&sha256.finalize()),
        sha1: rae_hash::to_hex(&sha1.finalize()),
        md5: rae_hash::to_hex(&md5.finalize()),
        crc32: rae_hash::to_hex(&crc32.finalize().to_be_bytes()),
    })
}

/// Case-insensitive ASCII equality of two hex strings (the publisher posts either
/// case; whitespace is trimmed by the caller). No allocation, never panics.
fn hex_eq_ci(got: &str, want: &str) -> bool {
    got.len() == want.len()
        && got
            .bytes()
            .zip(want.bytes())
            .all(|(a, b)| a.to_ascii_lowercase() == b.to_ascii_lowercase())
}

/// Hard cap on how large a file the Compare view will read (a diff of two huge
/// files would pin pages + produce an unscrollably large buffer). A file beyond
/// this is treated as "too large" and Compare declines, never crashes.
const COMPARE_READ_CAP: usize = 1024 * 1024; // 1 MiB per file

/// Hard cap on how large a file the Quick Look CSV/TSV table will read. A CSV
/// beyond this declines the table view (falls back to plain text), never crashes.
/// 4 MiB is well past any spreadsheet a human scrolls in a preview; the parser
/// also bounds rows/cols/cell-size independently.
const CSV_READ_CAP: usize = 4 * 1024 * 1024;

/// Read a whole file at `path` as UTF-8 text, bounded by `cap` bytes. Returns
/// `None` when the file can't be opened, is empty, exceeds `cap`, or is not valid
/// UTF-8 (binary). Never panics — `from_utf8` is checked, no slicing. This is the
/// shared, cap-parameterized core behind `read_file_text` and the CSV table view.
fn read_file_text_capped(path: &str, cap: usize) -> Option<alloc::string::String> {
    let fd = raekit::sys::open(path, 0);
    if fd == u64::MAX {
        return None;
    }
    let mut data: Vec<u8> = Vec::new();
    let mut chunk = [0u8; PREVIEW_CAP];
    loop {
        if data.len() > cap {
            let _ = raekit::sys::close(fd);
            return None;
        }
        let n = raekit::sys::read(fd, &mut chunk) as usize;
        if n == 0 || n > chunk.len() {
            break;
        }
        data.extend_from_slice(&chunk[..n]);
    }
    let _ = raekit::sys::close(fd);
    if data.is_empty() {
        return None;
    }
    match alloc::string::String::from_utf8(data) {
        Ok(s) => Some(s),
        Err(_) => None,
    }
}

/// Read a whole file at `path` as UTF-8 text for the Compare view. Returns `None`
/// when the file can't be opened, is empty, exceeds `COMPARE_READ_CAP`, or is not
/// valid UTF-8 (binary). Never panics — `from_utf8` is checked, no slicing.
fn read_file_text(path: &str) -> Option<alloc::string::String> {
    let fd = raekit::sys::open(path, 0);
    if fd == u64::MAX {
        return None;
    }
    let mut data: Vec<u8> = Vec::new();
    let mut chunk = [0u8; PREVIEW_CAP];
    loop {
        if data.len() > COMPARE_READ_CAP {
            let _ = raekit::sys::close(fd);
            return None;
        }
        let n = raekit::sys::read(fd, &mut chunk) as usize;
        if n == 0 || n > chunk.len() {
            break;
        }
        data.extend_from_slice(&chunk[..n]);
    }
    let _ = raekit::sys::close(fd);
    if data.is_empty() {
        return None;
    }
    // Checked UTF-8 conversion: non-text (binary) input → None (never panic).
    match alloc::string::String::from_utf8(data) {
        Ok(s) => Some(s),
        Err(_) => None,
    }
}

/// Which archive container the Extract action detected. Routes the read path:
/// `Zip` → `rae_zip::Archive`, `Tar`/`TarGz` → `rae_tar::read_tar(_gz)`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ArchiveFormat {
    Zip,
    Tar,
    TarGz,
}

/// Case-insensitive ASCII "does `name` end with `suffix`" — the shared extension
/// test used by both format detection and the stem stripper.
fn name_ends_with_ci(name: &str, suffix: &str) -> bool {
    let nb = name.as_bytes();
    let sb = suffix.as_bytes();
    nb.len() >= sb.len()
        && nb[nb.len() - sb.len()..]
            .iter()
            .zip(sb.iter())
            .all(|(&a, &b)| a.to_ascii_lowercase() == b.to_ascii_lowercase())
}

/// Extract a ZIP byte buffer into `dest`, applying the zip-slip gate per entry.
/// Returns `Some((extracted, unsafe_skipped, other_skipped))` on a parseable
/// archive (per-entry failures tallied, never fatal), or `None` if the bytes are
/// not a valid zip at all. Never panics.
fn extract_zip_bytes(bytes: &[u8], dest: &str) -> Option<(u32, u32, u32)> {
    let archive = Archive::open(bytes).ok()?;
    let mut extracted = 0u32;
    let mut unsafe_skipped = 0u32;
    let mut other_skipped = 0u32;

    for ze in archive.entries() {
        // Zip-slip defense FIRST: never write an entry whose name escapes the
        // destination (`../`, absolute, drive-letter, NUL — `rae_zip` checks).
        if !is_safe_path(&ze.name) {
            unsafe_skipped += 1;
            continue;
        }
        if ze.is_dir {
            let mut dir = PathBuf::new();
            dir.set(dest);
            if mkdir_rel_dirs(&dir, &ze.name) {
                extracted += 1;
            } else {
                other_skipped += 1;
            }
            continue;
        }
        // A file: read (CRC-verified) then write under dest, making parent dirs
        // as needed. Any per-entry Err (Unsupported/TooLarge/BadCrc/…) skips this
        // entry without aborting the whole extraction.
        match archive.read_entry(ze) {
            Ok(data) => {
                if write_entry_file(dest, &ze.name, &data) {
                    extracted += 1;
                } else {
                    other_skipped += 1;
                }
            }
            Err(_) => other_skipped += 1,
        }
    }
    Some((extracted, unsafe_skipped, other_skipped))
}

/// Extract a TAR (or gzip-tar, when `gz`) byte buffer into `dest`. Mirrors the
/// zip path: `rae_tar::is_safe_path` is consulted FIRST per entry (tar-slip
/// defense), `Dir` entries `mkdir` their relative path, `File` entries write
/// `entry.data()`. Symlinks/hardlinks/Other are SKIPPED and counted as
/// unsupported (a first-cut policy — we never create links). Returns
/// `Some((extracted, unsafe_skipped, other_skipped))`, or `None` when `read_tar*`
/// rejects the whole buffer (corrupt / bomb / not-a-tar). Never panics.
fn extract_tar_bytes(bytes: &[u8], dest: &str, gz: bool) -> Option<(u32, u32, u32)> {
    let archive = if gz {
        read_tar_gz(bytes).ok()?
    } else {
        read_tar(bytes).ok()?
    };
    let mut extracted = 0u32;
    let mut unsafe_skipped = 0u32;
    let mut other_skipped = 0u32;

    for te in archive.entries() {
        // Tar-slip defense FIRST: the tar-side gate (its own ".."/absolute/drive
        // checks), consulted before any write.
        if !rae_tar::is_safe_path(&te.name) {
            unsafe_skipped += 1;
            continue;
        }
        match te.kind {
            TarKind::Dir => {
                let mut dir = PathBuf::new();
                dir.set(dest);
                if mkdir_rel_dirs(&dir, &te.name) {
                    extracted += 1;
                } else {
                    other_skipped += 1;
                }
            }
            TarKind::File => {
                if write_entry_file(dest, &te.name, te.data()) {
                    extracted += 1;
                } else {
                    other_skipped += 1;
                }
            }
            // Symlink / Hardlink / Other: first-cut policy is to NOT create links
            // (avoids materializing an attacker-chosen link target); count as
            // unsupported-skip so the toast is honest.
            TarKind::Symlink | TarKind::Hardlink | TarKind::Other(_) => {
                other_skipped += 1;
            }
        }
    }
    Some((extracted, unsafe_skipped, other_skipped))
}

/// The "stem" of an archive file name: the leaf component with its trailing
/// archive extension stripped (case-insensitive). Handles BOTH the single-suffix
/// forms (`photos.zip` → `photos`, `a/b/data.TAR` → `data`) and the COMPOUND
/// gzip-tar suffixes, where both extensions must drop (`linux.tar.gz` → `linux`,
/// `src.TGZ` → `src`). The result is the extraction directory name placed next to
/// the archive. An empty/extension-only name falls back to `"extracted"`.
fn archive_stem(name: &str) -> &str {
    // Leaf component only (defensive; entry names here are already leaf names).
    let leaf = match name.rfind('/') {
        Some(i) => &name[i + 1..],
        None => name,
    };
    // Longest first so `.tar.gz` wins over `.tar`/`.gz`. `.tar.gz`/`.tgz` is the
    // double-extension case the task calls out.
    const SUFFIXES: [&str; 4] = [".tar.gz", ".tgz", ".tar", ".zip"];
    for suffix in SUFFIXES {
        if leaf.len() > suffix.len() && name_ends_with_ci(leaf, suffix) {
            let stripped = &leaf[..leaf.len() - suffix.len()];
            // `len() > suffix.len()` guarantees `stripped` is non-empty.
            return stripped;
        }
    }
    if leaf.is_empty() {
        "extracted"
    } else {
        leaf
    }
}

/// `mkdir` every component of a `dest`-relative, slash-separated `rel` path,
/// ignoring "already exists" at each level. `rel` is a caller-verified safe ZIP
/// name (no `..`, not absolute). Returns `false` only on a real mkdir failure
/// (read-only / out of space) — `E_VFS_EXISTS` is success. Backslashes are
/// treated as separators too (hostile archives use them).
fn mkdir_rel_dirs(dest: &PathBuf, rel: &str) -> bool {
    let mut p = *dest;
    for comp in rel.split(['/', '\\']) {
        if comp.is_empty() || comp == "." {
            continue;
        }
        p.push_component(comp);
        match raekit::sys::mkdir(p.as_str()) {
            Ok(()) | Err(raekit::sys::E_VFS_EXISTS) => {}
            Err(_) => return false,
        }
    }
    true
}

/// Write a single extracted file `data` to `<dest>/<rel>`, creating any parent
/// directories first (`mkdir -p` semantics). `rel` is a caller-verified safe ZIP
/// name. Returns `false` on any mkdir/open/short-write failure (the caller tallies
/// it as a skip; the rest of the extraction continues). Never panics.
fn write_entry_file(dest: &str, rel: &str, data: &[u8]) -> bool {
    // Split into parent dir components + final file name.
    let norm = rel; // separators handled below
                    // Find the last separator.
    let last_sep = norm
        .bytes()
        .enumerate()
        .rev()
        .find(|&(_, b)| b == b'/' || b == b'\\')
        .map(|(i, _)| i);

    let mut full = PathBuf::new();
    full.set(dest);
    if let Some(i) = last_sep {
        let parent = &norm[..i];
        let mut dp = PathBuf::new();
        dp.set(dest);
        if !mkdir_rel_dirs(&dp, parent) {
            return false;
        }
        // Build full path component-by-component (handles either separator).
        for comp in norm.split(['/', '\\']) {
            if comp.is_empty() || comp == "." {
                continue;
            }
            full.push_component(comp);
        }
    } else {
        full.push_component(norm);
    }

    // O_WRONLY | O_CREAT | O_TRUNC = 0x0241 (mirrors the text-editor save path).
    let fd = raekit::sys::open(full.as_str(), 0x0241);
    if fd == u64::MAX {
        return false;
    }
    let mut ok = true;
    if !data.is_empty() {
        let mut written = 0usize;
        while written < data.len() {
            let n = raekit::sys::write(fd, &data[written..]) as usize;
            if n == 0 {
                ok = false;
                break;
            }
            written += n;
        }
    }
    let _ = raekit::sys::close(fd);
    ok
}

// ── Compress helpers (Compress here → .zip / .gz) ───────────────────────────

/// Read an entire file at `path` into a heap buffer (size-capped by
/// `read_whole_file`). Returns `None` if the file can't be opened, is empty, or
/// exceeds the cap. Never panics.
fn read_file_bytes(path: &str) -> Option<Vec<u8>> {
    let fd = raekit::sys::open(path, 0);
    if fd == u64::MAX {
        return None;
    }
    let data = read_whole_file(fd, &[]);
    let _ = raekit::sys::close(fd);
    data
}

// ── File-association persistence (rae_toml) ──────────────────────────────────
//
// A user's "Set as default" overrides round-trip to `<home>/.config/file_assoc.toml`
// so they survive relaunch (the rae_toml raison d'être: "remember my settings"
// must be real — RaeenOS_Concept.md §"The user owns the machine"). The on-disk
// shape is one `[[assoc]]` array-of-tables entry per overridden MIME type:
//
//     [[assoc]]
//     mime = "image/png"
//     default = "my_viewer"
//     candidates = ["my_viewer", "photos", "files"]
//
// Load layers the parsed overrides ON TOP of `Registry::with_defaults()`, so the
// file only needs to carry the deltas; a missing OR corrupt file falls back to
// the built-in defaults (never panics — the same calm-fallback recipe the other
// apps use for their prefs).

const ASSOC_CONFIG_REL: &str = ".config/file_assoc.toml";

/// Build the absolute config path `<home>/.config/file_assoc.toml` into a
/// `PathBuf`. Returns `None` if it would not fit (PATH_CAP), so callers degrade
/// to the built-in defaults rather than truncating to a wrong path.
fn assoc_config_path(home: &str) -> Option<PathBuf> {
    let mut p = PathBuf::new();
    p.set(home);
    p.push_component(ASSOC_CONFIG_REL);
    // push_component split on the embedded '/' is not done, so verify it fit
    // verbatim (PathBuf::push_component copies bytes up to PATH_CAP).
    if p.as_str().as_bytes().len() != home.as_bytes().len() + 1 + ASSOC_CONFIG_REL.as_bytes().len()
    {
        return None;
    }
    Some(p)
}

/// Build the registry for this session: the built-in defaults overlaid with any
/// persisted overrides from `<home>/.config/file_assoc.toml`. A missing or
/// corrupt config yields exactly the built-in defaults (never panics).
fn load_registry(home: &str) -> Registry {
    let mut reg = Registry::with_defaults();
    let path = match assoc_config_path(home) {
        Some(p) => p,
        None => return reg,
    };
    let fd = raekit::sys::open(path.as_str(), 0);
    if fd == u64::MAX {
        return reg; // no config yet → defaults
    }
    let bytes = read_whole_file(fd, &[]);
    let _ = raekit::sys::close(fd);
    let bytes = match bytes {
        Some(b) => b,
        None => return reg,
    };
    let text = match core::str::from_utf8(&bytes) {
        Ok(t) => t,
        Err(_) => return reg, // non-UTF-8 → corrupt → defaults
    };
    apply_assoc_overrides(&mut reg, text);
    reg
}

/// Parse a `file_assoc.toml` document and apply each `[[assoc]]` entry as a
/// registry override. A parse error or any malformed entry is silently skipped —
/// the worst case is "fewer overrides than the file intended", never a panic and
/// never a wrong default. Separated from I/O so it is directly host-testable.
fn apply_assoc_overrides(reg: &mut Registry, text: &str) {
    let doc = match rae_toml::parse(text) {
        Ok(d) => d,
        Err(_) => return, // corrupt TOML → keep the built-in defaults
    };
    let entries = match doc.get("assoc").and_then(rae_toml::Toml::as_array) {
        Some(a) => a,
        None => return,
    };
    for e in entries {
        let mime = match e.get("mime").and_then(rae_toml::Toml::as_str) {
            Some(m) if !m.is_empty() => m,
            _ => continue,
        };
        let default = match e.get("default").and_then(rae_toml::Toml::as_str) {
            Some(d) if !d.is_empty() => d,
            _ => continue,
        };
        // Candidates: each array element that is a non-empty string. If the array
        // is missing/empty, fall back to just [default] so the override is valid.
        let mut cand: Vec<&str> = Vec::new();
        if let Some(arr) = e.get("candidates").and_then(rae_toml::Toml::as_array) {
            for c in arr {
                if let Some(s) = c.as_str() {
                    if !s.is_empty() {
                        cand.push(s);
                    }
                }
            }
        }
        if cand.is_empty() {
            cand.push(default);
        }
        reg.set(mime, default, &cand);
    }
}

/// Serialize the registry's overrides to a `file_assoc.toml` document via
/// rae_toml's serializer. Only the entries that differ from the built-in defaults
/// are emitted (the file carries deltas, not the whole world); the built-ins are
/// re-applied on load. Separated from I/O so the round-trip is host-testable.
fn serialize_assoc_overrides(reg: &Registry) -> String {
    let defaults = Registry::with_defaults();
    // Build a [[assoc]] array-of-tables of every MIME whose default OR candidate
    // list now differs from the built-in registry.
    let mut assoc: Vec<rae_toml::Toml> = Vec::new();
    for &mime in OVERRIDABLE_MIMES {
        let mt = MimeType(mime);
        let cur_default = reg.default_app(mt);
        let cur_cands = reg.candidates(mt);
        let def_default = defaults.default_app(mt);
        let def_cands = defaults.candidates(mt);
        if cur_default == def_default && cur_cands == def_cands {
            continue; // unchanged from built-in → don't persist
        }
        let mut table: Vec<(String, rae_toml::Toml)> = Vec::new();
        table.push((
            String::from("mime"),
            rae_toml::Toml::String(String::from(mime)),
        ));
        table.push((
            String::from("default"),
            rae_toml::Toml::String(String::from(cur_default)),
        ));
        let cand_arr: Vec<rae_toml::Toml> = cur_cands
            .iter()
            .map(|c| rae_toml::Toml::String(c.clone()))
            .collect();
        table.push((String::from("candidates"), rae_toml::Toml::Array(cand_arr)));
        assoc.push(rae_toml::Toml::Table(table));
    }
    let root = rae_toml::Toml::Table(alloc::vec![(
        String::from("assoc"),
        rae_toml::Toml::Array(assoc),
    )]);
    rae_toml::to_string(&root)
}

/// The closed set of MIME types whose default the user may override (the ones the
/// built-in registry knows + a couple of common extras). The serializer walks
/// this to find deltas; an override of a MIME not in this list still works
/// in-memory but won't persist — acceptable for the bundled set, and keeps the
/// serialized file bounded.
const OVERRIDABLE_MIMES: &[&str] = &[
    "text/plain",
    "text/markdown",
    "text/csv",
    "text/html",
    "application/json",
    "image/png",
    "image/jpeg",
    "image/gif",
    "image/bmp",
    "image/webp",
    "image/svg+xml",
    "image/tiff",
    "application/pdf",
    "audio/mpeg",
    "audio/wav",
    "audio/flac",
    "audio/ogg",
    "video/mp4",
    "video/x-matroska",
    "video/webm",
];

/// Persist the registry overrides to `<home>/.config/file_assoc.toml`, creating
/// the `.config` directory if needed (idempotent; `E_VFS_EXISTS` is fine).
/// Returns `false` on any path/mkdir/write failure (the in-memory override stays
/// live regardless). Never panics.
fn save_registry(home: &str, reg: &Registry) -> bool {
    // Ensure <home>/.config exists.
    let mut cfg_dir = PathBuf::new();
    cfg_dir.set(home);
    cfg_dir.push_component(".config");
    match raekit::sys::mkdir(cfg_dir.as_str()) {
        Ok(()) | Err(raekit::sys::E_VFS_EXISTS) => {}
        Err(_) => return false,
    }
    let path = match assoc_config_path(home) {
        Some(p) => p,
        None => return false,
    };
    let text = serialize_assoc_overrides(reg);
    write_whole_file(path.as_str(), text.as_bytes())
}

/// Write `data` to `path` (O_WRONLY|O_CREAT|O_TRUNC = 0x0241, the text-editor save
/// flags). Returns `false` on any open/short-write failure. Never panics.
fn write_whole_file(path: &str, data: &[u8]) -> bool {
    let fd = raekit::sys::open(path, 0x0241);
    if fd == u64::MAX {
        return false;
    }
    let mut ok = true;
    if !data.is_empty() {
        let mut written = 0usize;
        while written < data.len() {
            let n = raekit::sys::write(fd, &data[written..]) as usize;
            if n == 0 {
                ok = false;
                break;
            }
            written += n;
        }
    }
    let _ = raekit::sys::close(fd);
    ok
}

/// Append `suffix` (e.g. `.gz`) to `name`, writing into `out` and returning the
/// byte length. Truncates safely if the combined length would overflow `out`
/// (never panics; the result stays a valid, if shortened, name). ASCII suffixes
/// only, so no char-boundary concern at the join.
fn build_dot_suffix_name(name: &str, suffix: &str, out: &mut [u8]) -> usize {
    let nb = name.as_bytes();
    let sb = suffix.as_bytes();
    // Reserve room for the suffix; truncate the NAME (on a UTF-8 boundary) if needed.
    let max_name = out.len().saturating_sub(sb.len());
    let mut keep = nb.len().min(max_name);
    // Back off to a char boundary so we never split a multi-byte sequence.
    while keep > 0 && (nb[keep - 1] & 0xC0) == 0x80 {
        keep -= 1;
    }
    let mut n = 0usize;
    for &b in &nb[..keep] {
        out[n] = b;
        n += 1;
    }
    for &b in sb {
        if n < out.len() {
            out[n] = b;
            n += 1;
        }
    }
    n
}

/// Derive a ZIP archive name from the first file's name: drop a single trailing
/// extension (`report.txt` → `report.zip`), then append `.zip`. An extension-only
/// or empty name falls back to `archive.zip`. Char-boundary-safe.
fn build_zip_basename(first_name: &str, out: &mut [u8]) -> usize {
    let nb = first_name.as_bytes();
    // Find the last '.' that is not the leading byte (so ".bashrc" keeps its name).
    let stem_len = nb
        .iter()
        .enumerate()
        .rev()
        .find(|&(i, &b)| b == b'.' && i > 0)
        .map(|(i, _)| i)
        .unwrap_or(nb.len());
    let stem = if stem_len == 0 {
        "archive"
    } else {
        core::str::from_utf8(&nb[..stem_len]).unwrap_or("archive")
    };
    build_dot_suffix_name(stem, ".zip", out)
}

// ── Recursive folder compression (Compress here → folder → nested .zip) ─────────
//
// RaeenOS_Concept.md §"The user owns the machine": zipping a FOLDER is the common
// case (you compress a project directory, not loose files). The walk reuses the
// SAME directory-read mechanism the file list uses (`raekit::sys::readdir_at`,
// decoding `[name_len:u16][size:u32][name…]` records) and the SAME `ZipWriter`,
// adding each descendant FILE with its path RELATIVE to the archive root
// (`docs/readme.md`, `docs/sub/a.txt`) and an explicit `dir/` entry per directory.
//
// The walk is hard-bounded so a pathological/cyclic tree can never hang or panic:
// a max depth, a max placed-entry count, and a max total uncompressed byte budget,
// all carried in one shared `Walk` ledger spanning the whole archive.

/// Max directory nesting depth the recursive compressor will descend. A real FS
/// has no cycles, but this caps a hostile/looped tree so the walk always returns.
const COMPRESS_MAX_DEPTH: u32 = 64;
/// Max number of entries (files + explicit dir markers) placed into one archive.
const COMPRESS_MAX_ENTRIES: u32 = 4096;
/// Max total UNCOMPRESSED bytes the archive will ingest (bomb / runaway guard).
const COMPRESS_MAX_TOTAL_BYTES: usize = 64 * 1024 * 1024; // 64 MiB

/// Shared budget ledger for one recursive compression. Bounds the walk so it can
/// never run away on a deep/cyclic tree or a directory full of huge files. Once any
/// limit trips, `exhausted()` stays true and the walk unwinds cleanly (the partial
/// archive is still valid — it simply omits the over-budget tail).
struct Walk {
    entries: u32,
    total_bytes: usize,
}

impl Walk {
    fn new() -> Self {
        Self {
            entries: 0,
            total_bytes: 0,
        }
    }

    /// True once the entry-count or byte budget is spent — callers stop descending.
    fn exhausted(&self) -> bool {
        self.entries >= COMPRESS_MAX_ENTRIES || self.total_bytes >= COMPRESS_MAX_TOTAL_BYTES
    }

    /// Charge an explicit directory marker against the entry budget. Returns
    /// `false` (and adds nothing) if the entry budget is already spent.
    fn charge_dir(&mut self) -> bool {
        if self.entries >= COMPRESS_MAX_ENTRIES {
            return false;
        }
        self.entries += 1;
        true
    }

    /// Charge a file of `len` bytes against BOTH budgets. Returns `false` (adding
    /// nothing) when either the entry count or the byte budget would be exceeded,
    /// so an over-budget file is skipped rather than truncated.
    fn charge_file(&mut self, len: usize) -> bool {
        if self.entries >= COMPRESS_MAX_ENTRIES {
            return false;
        }
        if self.total_bytes.saturating_add(len) > COMPRESS_MAX_TOTAL_BYTES {
            return false;
        }
        self.entries += 1;
        self.total_bytes += len;
        true
    }
}

/// One decoded `readdir_at` child: its leaf name + whether it is a directory. A
/// fixed-capacity copy so we can close the directory read before recursing (the
/// app reads at most `MAX_ENTRIES` rows per dir anyway).
#[derive(Clone, Copy)]
struct WalkChild {
    name: [u8; 48],
    name_len: usize,
    is_dir: bool,
}

impl WalkChild {
    fn name(&self) -> &str {
        core::str::from_utf8(&self.name[..self.name_len]).unwrap_or("?")
    }
}

/// Read the directory at absolute `abs_path` via the SAME `readdir_at` syscall the
/// file list uses, decoding the `[name_len:u16][size:u32][name…]` record format and
/// the SAME folder heuristic (`size == 0 && name has no '.'`). Fills `out` (up to
/// `MAX_ENTRIES` children) and returns the count. Skips `.`/`..`/empty names. Never
/// panics: a malformed buffer simply stops the decode early.
fn walk_read_dir(abs_path: &str, out: &mut [WalkChild; MAX_ENTRIES]) -> usize {
    let mut buf = [0u8; 4096];
    let count = raekit::sys::readdir_at(abs_path, &mut buf) as usize;
    let mut off = 0usize;
    let mut n = 0usize;
    for _ in 0..count {
        if n >= MAX_ENTRIES || off + 6 > buf.len() {
            break;
        }
        let name_len = u16::from_le_bytes([buf[off], buf[off + 1]]) as usize;
        let size = u32::from_le_bytes([buf[off + 2], buf[off + 3], buf[off + 4], buf[off + 5]]);
        off += 6;
        if off + name_len > buf.len() {
            break;
        }
        let name = &buf[off..off + name_len];
        off += name_len;
        // Skip self/parent links and empty names (defensive — the VFS shouldn't
        // emit them, but a cycle guard must never re-enter "." or "..").
        if name == b"." || name == b".." || name.is_empty() {
            continue;
        }
        let take = name.len().min(48);
        let mut child = WalkChild {
            name: [0u8; 48],
            name_len: take,
            is_dir: size == 0 && !name[..take].contains(&b'.'),
        };
        child.name[..take].copy_from_slice(&name[..take]);
        out[n] = child;
        n += 1;
    }
    n
}

/// Recursively add the directory tree at absolute `abs_dir` to `writer`, naming each
/// descendant with the archive-relative `rel_prefix` (the path under the archive
/// root, e.g. `docs` then `docs/sub`). For each child: a file is read (size-capped)
/// and added as `rel_prefix/childname`; a subdirectory adds an explicit
/// `rel_prefix/childname/` marker then recurses. `added` counts files actually
/// placed. Bounded by `walk` (depth/count/bytes) so it always terminates and never
/// panics; an unreadable child is skipped, not fatal.
fn zip_add_dir_recursive(
    writer: &mut ZipWriter,
    abs_dir: &PathBuf,
    rel_prefix: &PathBuf,
    depth: u32,
    walk: &mut Walk,
    added: &mut u32,
) {
    if depth >= COMPRESS_MAX_DEPTH || walk.exhausted() {
        return;
    }

    // Snapshot this directory's children, then close the read before recursing.
    let mut children = [WalkChild {
        name: [0u8; 48],
        name_len: 0,
        is_dir: false,
    }; MAX_ENTRIES];
    let n = walk_read_dir(abs_dir.as_str(), &mut children);

    for child in children.iter().take(n) {
        if walk.exhausted() {
            return;
        }
        let cname = child.name();

        // Build the child's archive-relative path (`rel_prefix/childname`) and its
        // absolute filesystem path (`abs_dir/childname`). `push_component` only
        // appends bytes + an ASCII '/', so the join is char-boundary safe.
        let mut child_rel = *rel_prefix;
        child_rel.push_component(cname);
        let mut child_abs = *abs_dir;
        child_abs.push_component(cname);

        if child.is_dir {
            // Explicit directory entry: `rel/sub/` (trailing slash, like real zips),
            // so empty/structural dirs survive the round-trip.
            if walk.charge_dir() {
                let mut dir_name = child_rel;
                // Append a trailing '/' if room (ASCII — boundary safe).
                if dir_name.len < PATH_CAP {
                    dir_name.bytes[dir_name.len] = b'/';
                    dir_name.len += 1;
                }
                writer.add_file(dir_name.as_str(), &[]);
            }
            zip_add_dir_recursive(writer, &child_abs, &child_rel, depth + 1, walk, added);
        } else if let Some(bytes) = read_file_bytes(child_abs.as_str()) {
            if walk.charge_file(bytes.len()) {
                writer.add_file(child_rel.as_str(), &bytes);
                *added += 1;
            }
        }
    }
}

/// A minimal, from-scratch ZIP (APPNOTE) writer producing a 32-bit ZIP that
/// `rae_zip::Archive::open` re-reads. Each added file is DEFLATE-compressed via
/// `rae_deflate`; if the compressed form isn't smaller than the raw bytes the
/// entry is STORED (method 0) instead, so tiny/incompressible files never bloat.
/// The local headers are written into `body` as files are added; `finish` appends
/// the central directory + EOCD. CRC-32 is `rae_deflate::crc32` (IEEE), the same
/// polynomial the ZIP/gzip spec and `rae_zip`'s verifier use.
struct ZipWriter {
    /// The accumulating archive: local headers + bodies, then CD + EOCD on finish.
    body: Vec<u8>,
    /// One central-directory record per added file, built as we go.
    central: Vec<ZipCentral>,
}

/// Per-file metadata recorded at add time, replayed into the central directory.
struct ZipCentral {
    name: Vec<u8>,
    crc: u32,
    comp_size: u32,
    uncomp_size: u32,
    method: u16,
    local_offset: u32,
}

impl ZipWriter {
    const SIG_LOCAL: u32 = 0x0403_4b50;
    const SIG_CENTRAL: u32 = 0x0201_4b50;
    const SIG_EOCD: u32 = 0x0605_4b50;

    fn new() -> Self {
        Self {
            body: Vec::new(),
            central: Vec::new(),
        }
    }

    /// Add one file `name`/`raw` to the archive. Chooses DEFLATE (method 8) when it
    /// produces a smaller body than the raw bytes, else STORED (method 0). Writes
    /// the Local File Header + body and records the central-directory entry.
    fn add_file(&mut self, name: &str, raw: &[u8]) {
        let crc = rae_deflate::crc32(raw);
        let comp = rae_deflate::deflate(raw);
        // method 0 = stored when deflate didn't actually shrink it (tiny/random).
        let (method, payload): (u16, &[u8]) = if comp.len() < raw.len() {
            (8, &comp)
        } else {
            (0, raw)
        };
        let local_offset = self.body.len() as u32;
        let name_bytes = name.as_bytes();

        self.body.extend_from_slice(&Self::SIG_LOCAL.to_le_bytes());
        self.body.extend_from_slice(&20u16.to_le_bytes()); // version needed
        self.body.extend_from_slice(&0u16.to_le_bytes()); // flags
        self.body.extend_from_slice(&method.to_le_bytes()); // method
        self.body.extend_from_slice(&0u16.to_le_bytes()); // mod time
        self.body.extend_from_slice(&0u16.to_le_bytes()); // mod date
        self.body.extend_from_slice(&crc.to_le_bytes());
        self.body
            .extend_from_slice(&(payload.len() as u32).to_le_bytes()); // comp size
        self.body
            .extend_from_slice(&(raw.len() as u32).to_le_bytes()); // uncomp size
        self.body
            .extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
        self.body.extend_from_slice(&0u16.to_le_bytes()); // extra len
        self.body.extend_from_slice(name_bytes);
        self.body.extend_from_slice(payload);

        self.central.push(ZipCentral {
            name: name_bytes.to_vec(),
            crc,
            comp_size: payload.len() as u32,
            uncomp_size: raw.len() as u32,
            method,
            local_offset,
        });
    }

    /// Finalize: append the central directory (one record per file) + the EOCD,
    /// returning the complete ZIP bytes.
    fn finish(mut self) -> Vec<u8> {
        let cd_offset = self.body.len() as u32;
        for c in &self.central {
            self.body
                .extend_from_slice(&Self::SIG_CENTRAL.to_le_bytes());
            self.body.extend_from_slice(&20u16.to_le_bytes()); // version made by
            self.body.extend_from_slice(&20u16.to_le_bytes()); // version needed
            self.body.extend_from_slice(&0u16.to_le_bytes()); // flags
            self.body.extend_from_slice(&c.method.to_le_bytes()); // method
            self.body.extend_from_slice(&0u16.to_le_bytes()); // time
            self.body.extend_from_slice(&0u16.to_le_bytes()); // date
            self.body.extend_from_slice(&c.crc.to_le_bytes());
            self.body.extend_from_slice(&c.comp_size.to_le_bytes());
            self.body.extend_from_slice(&c.uncomp_size.to_le_bytes());
            self.body
                .extend_from_slice(&(c.name.len() as u16).to_le_bytes());
            self.body.extend_from_slice(&0u16.to_le_bytes()); // extra len
            self.body.extend_from_slice(&0u16.to_le_bytes()); // comment len
            self.body.extend_from_slice(&0u16.to_le_bytes()); // disk number
            self.body.extend_from_slice(&0u16.to_le_bytes()); // internal attrs
            self.body.extend_from_slice(&0u32.to_le_bytes()); // external attrs
            self.body.extend_from_slice(&c.local_offset.to_le_bytes());
            self.body.extend_from_slice(&c.name);
        }
        let cd_size = self.body.len() as u32 - cd_offset;

        self.body.extend_from_slice(&Self::SIG_EOCD.to_le_bytes());
        self.body.extend_from_slice(&0u16.to_le_bytes()); // disk number
        self.body.extend_from_slice(&0u16.to_le_bytes()); // disk with CD
        self.body
            .extend_from_slice(&(self.central.len() as u16).to_le_bytes());
        self.body
            .extend_from_slice(&(self.central.len() as u16).to_le_bytes());
        self.body.extend_from_slice(&cd_size.to_le_bytes());
        self.body.extend_from_slice(&cd_offset.to_le_bytes());
        self.body.extend_from_slice(&0u16.to_le_bytes()); // comment len
        self.body
    }
}

/// Parse PNG width/height from the IHDR chunk (bytes 16..24, big-endian).
fn png_dimensions(buf: &[u8]) -> Option<(u32, u32)> {
    if buf.len() < 24 || buf[..8] != PNG_SIG {
        return None;
    }
    let w = u32::from_be_bytes([buf[16], buf[17], buf[18], buf[19]]);
    let h = u32::from_be_bytes([buf[20], buf[21], buf[22], buf[23]]);
    Some((w, h))
}

/// Source-over alpha-composite an ARGB8888 pixel onto an opaque ARGB8888
/// background. The surface is opaque, so the result alpha is forced to 0xFF.
/// Integer math (no float): `out = (src*a + dst*(255-a)) / 255` per channel.
#[inline]
fn over(src: u32, dst: u32) -> u32 {
    let a = (src >> 24) & 0xFF;
    if a == 0xFF {
        return src | 0xFF00_0000;
    }
    if a == 0 {
        return dst | 0xFF00_0000;
    }
    let ia = 255 - a;
    let blend = |s: u32, d: u32| -> u32 { (s * a + d * ia + 127) / 255 };
    let r = blend((src >> 16) & 0xFF, (dst >> 16) & 0xFF);
    let g = blend((src >> 8) & 0xFF, (dst >> 8) & 0xFF);
    let b = blend(src & 0xFF, dst & 0xFF);
    0xFF00_0000 | (r << 16) | (g << 8) | b
}

/// Blit a decoded ARGB8888 image into the `(fx, fy, fw, fh)` frame, preserving
/// aspect ratio (scale-to-fit) and letterboxing the remainder with the token
/// background. Uses integer nearest-neighbor sampling — fine for a preview and
/// allocation-free. Transparent source pixels are composited over the letterbox
/// color so an RGBA PNG reads correctly.
fn blit_image_fit(
    canvas: &mut Canvas,
    img: &DecodedImage,
    fx: usize,
    fy: usize,
    fw: usize,
    fh: usize,
) {
    // Letterbox: the whole frame gets the token background first; a thin accent
    // top edge + strong stroke outline frames the image area (no magic colors).
    canvas.fill_rect(fx, fy, fw, fh, DARK.bg_base);

    let iw = img.width as usize;
    let ih = img.height as usize;
    if iw == 0 || ih == 0 || fw == 0 || fh == 0 {
        return;
    }

    // Fit: dest = src * min(fw/iw, fh/ih), done with cross-multiplication so we
    // never lose precision to integer division before comparing.
    let (dw, dh) = if iw * fh <= fw * ih {
        // height-bound
        let dh = fh;
        let dw = (iw * fh) / ih;
        (dw.max(1), dh)
    } else {
        // width-bound
        let dw = fw;
        let dh = (ih * fw) / iw;
        (dw, dh.max(1))
    };
    let ox = fx + (fw - dw.min(fw)) / 2;
    let oy = fy + (fh - dh.min(fh)) / 2;

    for dy in 0..dh.min(fh) {
        // Nearest-neighbor source row.
        let sy = (dy * ih) / dh;
        let sy = sy.min(ih - 1);
        let row_base = sy * iw;
        for dx in 0..dw.min(fw) {
            let sx = (dx * iw) / dw;
            let sx = sx.min(iw - 1);
            let src = img.pixels[row_base + sx];
            canvas.draw_pixel(ox + dx, oy + dy, over(src, DARK.bg_base));
        }
    }

    // Frame the image rect: strong stroke + a 1px live-accent top edge.
    let frame_w = dw.min(fw);
    let frame_h = dh.min(fh);
    canvas.draw_rect_outline(ox, oy, frame_w, frame_h, STROKE_HL);
    canvas.fill_rect(ox, oy, frame_w, 1, accent());
}

fn render_batch_rename(app: &App, canvas: &mut Canvas) {
    canvas.fill_rect(0, 0, WIN_W, WIN_H, OVERLAY_SCRIM);
    let pw = 460;
    let ph = 280;
    let px = (WIN_W - pw) / 2;
    let py = (WIN_H - ph) / 2;
    canvas.fill_rounded_rect(
        px,
        py,
        pw,
        ph,
        rae_tokens::RADIUS_LG as usize,
        DARK.bg_overlay,
    );
    canvas.draw_rounded_rect_outline(px, py, pw, ph, rae_tokens::RADIUS_LG as usize, STROKE_HL);

    canvas.draw_text_aa(
        (px + 16) as i32,
        (py + 14) as i32,
        "Batch Rename",
        rae_tokens::TYPE_SUBTITLE,
        TEXT_FG,
        FontFamily::Sans,
    );

    let mut cbuf = [0u8; 48];
    let cn = fmt_marked(app.marked_count(), &mut cbuf);
    if let Ok(s) = core::str::from_utf8(&cbuf[..cn]) {
        canvas.draw_text_aa(
            (px + 16) as i32,
            (py + 38) as i32,
            s,
            rae_tokens::TYPE_CAPTION,
            TEXT_MUTED,
            FontFamily::Sans,
        );
    }

    // Pattern field
    canvas.draw_text_aa(
        (px + 16) as i32,
        (py + 64) as i32,
        "Pattern (### = counter):",
        rae_tokens::TYPE_CAPTION,
        TEXT_MUTED,
        FontFamily::Sans,
    );
    canvas.fill_rounded_rect(
        px + 16,
        py + 84,
        pw - 32,
        28,
        rae_tokens::RADIUS_SM as usize,
        DARK.bg_base,
    );
    canvas.draw_rounded_rect_outline(
        px + 16,
        py + 84,
        pw - 32,
        28,
        rae_tokens::RADIUS_SM as usize,
        accent(),
    );
    canvas.draw_text_aa(
        (px + 24) as i32,
        (py + 90) as i32,
        app.pattern_str(),
        rae_tokens::TYPE_BODY,
        TEXT_FG,
        FontFamily::Mono,
    );

    // Live preview of the first marked entry's result.
    canvas.draw_text_aa(
        (px + 16) as i32,
        (py + 124) as i32,
        "Preview:",
        rae_tokens::TYPE_CAPTION,
        TEXT_MUTED,
        FontFamily::Sans,
    );
    let cwd_len = app.cwd().as_bytes().len();
    let mut pv_y = py + 144;
    let mut idx = 0u32;
    for e in app.entries().iter() {
        if !e.marked {
            continue;
        }
        if pv_y > py + ph - 50 {
            break;
        }
        let original = App::entry_name(e);
        let txt = match batch_rename_target(original, app.pattern_str(), idx, 1, cwd_len) {
            Ok(n) => {
                // "<original>  ->  <new>"
                draw_rename_row(canvas, px + 24, pv_y, original, n.as_str());
                idx += 1;
                pv_y += 18;
                continue;
            }
            Err(_) => "(invalid pattern)",
        };
        canvas.draw_text_aa(
            (px + 24) as i32,
            pv_y as i32,
            txt,
            rae_tokens::TYPE_CAPTION,
            DARK.state_danger,
            FontFamily::Sans,
        );
        pv_y += 18;
    }

    let hint = "Enter: apply   Esc: cancel";
    let hw = canvas.measure_text_aa(hint, rae_tokens::TYPE_CAPTION, FontFamily::Sans);
    canvas.draw_text_aa(
        (px + pw - 16) as i32 - hw,
        (py + ph - 22) as i32,
        hint,
        rae_tokens::TYPE_CAPTION,
        TEXT_MUTED,
        FontFamily::Sans,
    );
}

// ── Checksum / Verify overlay geometry + rendering ───────────────────────────
//
// A centered panel (same scrim + rounded-panel idiom as the other overlays).
// The geometry is shared between `render_checksum` (DRAW) and the mouse
// hit-testers `checksum_close_rect` / `checksum_verify_rect` (HIT) so a click on
// the close button or the verify field lands exactly where it's drawn.

const CK_PW: usize = 560;
const CK_PH: usize = 360;

fn checksum_panel_origin() -> (usize, usize) {
    ((WIN_W - CK_PW) / 2, (WIN_H - CK_PH) / 2)
}

/// The close (×) button rect, top-right of the checksum panel.
fn checksum_close_rect() -> Rect {
    let (px, py) = checksum_panel_origin();
    Rect {
        x: px + CK_PW - 36,
        y: py + 10,
        w: 26,
        h: 26,
    }
}

/// The verify input-field rect (clickable to focus; typing always targets it).
fn checksum_verify_rect() -> Rect {
    let (px, py) = checksum_panel_origin();
    Rect {
        x: px + 16,
        y: py + CK_PH - 96,
        w: CK_PW - 32,
        h: 28,
    }
}

/// One labeled digest row: "LABEL  <hex>" with the hex in mono. Returns nothing;
/// long SHA-256 hex (64 chars) fits the panel width at the caption size.
fn draw_digest_row(
    canvas: &mut Canvas,
    x: usize,
    y: usize,
    label: &str,
    hex: &str,
    accent_hex: bool,
) {
    let lw = canvas.draw_text_aa(
        x as i32,
        y as i32,
        label,
        rae_tokens::TYPE_CAPTION,
        TEXT_MUTED,
        FontFamily::Sans,
    );
    let color = if accent_hex { accent() } else { TEXT_FG };
    canvas.draw_text_aa(
        x as i32 + lw + 10,
        y as i32,
        hex,
        rae_tokens::TYPE_CAPTION,
        color,
        FontFamily::Mono,
    );
}

fn render_checksum(app: &App, canvas: &mut Canvas) {
    canvas.fill_rect(0, 0, WIN_W, WIN_H, OVERLAY_SCRIM);
    let (px, py) = checksum_panel_origin();
    let pw = CK_PW;
    let ph = CK_PH;
    canvas.fill_rounded_rect(
        px,
        py,
        pw,
        ph,
        rae_tokens::RADIUS_LG as usize,
        DARK.bg_overlay,
    );
    canvas.draw_rounded_rect_outline(px, py, pw, ph, rae_tokens::RADIUS_LG as usize, STROKE_HL);

    // Title + filename.
    canvas.draw_text_aa(
        (px + 16) as i32,
        (py + 12) as i32,
        "Checksum / Verify",
        rae_tokens::TYPE_SUBTITLE,
        TEXT_FG,
        FontFamily::Sans,
    );
    canvas.draw_text_aa(
        (px + 16) as i32,
        (py + 36) as i32,
        app.checksum_name_str(),
        rae_tokens::TYPE_CAPTION,
        accent(),
        FontFamily::Sans,
    );

    // Close (×) button.
    let cr = checksum_close_rect();
    canvas.fill_rounded_rect(
        cr.x,
        cr.y,
        cr.w,
        cr.h,
        rae_tokens::RADIUS_SM as usize,
        DARK.bg_base,
    );
    canvas.draw_rounded_rect_outline(
        cr.x,
        cr.y,
        cr.w,
        cr.h,
        rae_tokens::RADIUS_SM as usize,
        STROKE_HL,
    );
    let xw = canvas.measure_text_aa("x", rae_tokens::TYPE_BODY, FontFamily::Sans);
    canvas.draw_text_aa(
        (cr.x + (cr.w.saturating_sub(xw as usize)) / 2) as i32,
        (cr.y + 4) as i32,
        "x",
        rae_tokens::TYPE_BODY,
        TEXT_MUTED,
        FontFamily::Sans,
    );

    // Digest rows. SHA-256 first + prominent (the default download-integrity one).
    let mut ry = py + 70;
    draw_digest_row(
        canvas,
        px + 16,
        ry,
        "SHA-256",
        app.checksum_sha256.as_str(),
        true,
    );
    ry += 26;
    draw_digest_row(
        canvas,
        px + 16,
        ry,
        "SHA-1  ",
        app.checksum_sha1.as_str(),
        false,
    );
    ry += 22;
    draw_digest_row(
        canvas,
        px + 16,
        ry,
        "MD5    ",
        app.checksum_md5.as_str(),
        false,
    );
    ry += 22;
    draw_digest_row(
        canvas,
        px + 16,
        ry,
        "CRC32  ",
        app.checksum_crc32.as_str(),
        false,
    );

    // Verify field label + box.
    canvas.draw_text_aa(
        (px + 16) as i32,
        (py + CK_PH - 118) as i32,
        "Paste expected hash, then Enter (algo auto-detected by length):",
        rae_tokens::TYPE_CAPTION,
        TEXT_MUTED,
        FontFamily::Sans,
    );
    let vr = checksum_verify_rect();
    canvas.fill_rounded_rect(
        vr.x,
        vr.y,
        vr.w,
        vr.h,
        rae_tokens::RADIUS_SM as usize,
        DARK.bg_base,
    );
    canvas.draw_rounded_rect_outline(
        vr.x,
        vr.y,
        vr.w,
        vr.h,
        rae_tokens::RADIUS_SM as usize,
        accent(),
    );
    canvas.draw_text_aa(
        (vr.x + 8) as i32,
        (vr.y + 6) as i32,
        app.verify_str(),
        rae_tokens::TYPE_BODY,
        TEXT_FG,
        FontFamily::Mono,
    );

    // Result banner: big green MATCH / red NO MATCH, or a neutral prompt.
    let banner_y = py + CK_PH - 56;
    match app.verify_result {
        VerifyResult::Match => {
            canvas.draw_text_aa(
                (px + 16) as i32,
                banner_y as i32,
                "MATCH - integrity verified",
                rae_tokens::TYPE_SUBTITLE,
                DARK.state_ok,
                FontFamily::Sans,
            );
        }
        VerifyResult::NoMatch => {
            canvas.draw_text_aa(
                (px + 16) as i32,
                banner_y as i32,
                "NO MATCH - file differs from the expected hash",
                rae_tokens::TYPE_SUBTITLE,
                DARK.state_danger,
                FontFamily::Sans,
            );
        }
        VerifyResult::None => {}
    }

    let hint = "Enter: verify   Esc: close";
    let hw = canvas.measure_text_aa(hint, rae_tokens::TYPE_CAPTION, FontFamily::Sans);
    canvas.draw_text_aa(
        (px + pw - 16) as i32 - hw,
        (py + ph - 22) as i32,
        hint,
        rae_tokens::TYPE_CAPTION,
        TEXT_MUTED,
        FontFamily::Sans,
    );
}

fn draw_rename_row(canvas: &mut Canvas, x: usize, y: usize, original: &str, new: &str) {
    let ow = canvas.draw_text_aa(
        x as i32,
        y as i32,
        original,
        rae_tokens::TYPE_CAPTION,
        TEXT_MUTED,
        FontFamily::Mono,
    );
    let aw = canvas.draw_text_aa(
        x as i32 + ow + 6,
        y as i32,
        "->",
        rae_tokens::TYPE_CAPTION,
        TEXT_FG,
        FontFamily::Sans,
    );
    canvas.draw_text_aa(
        x as i32 + ow + 6 + aw + 6,
        y as i32,
        new,
        rae_tokens::TYPE_CAPTION,
        accent(),
        FontFamily::Mono,
    );
}

fn draw_button(canvas: &mut Canvas, x: usize, y: usize, w: usize, h: usize, label: &str) {
    draw_button_state(canvas, x, y, w, h, label, true);
}

fn draw_button_state(
    canvas: &mut Canvas,
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    label: &str,
    enabled: bool,
) {
    let r = rae_tokens::RADIUS_SM as usize;
    // QUIET frosted pill — frost fill only, NO outline stroke: the full
    // rect outline over the frost read as a Win95 raised bevel (visual-QA).
    // Finder/Explorer toolbar controls are borderless quiet fills.
    canvas.fill_rounded_rect(x, y, w, h, r, rae_tokens::GLASS_POPOVER_DARK.frost);
    let label_w = canvas.measure_text_aa(label, rae_tokens::TYPE_LABEL, FontFamily::Sans);
    let tx = x as i32 + (w as i32 - label_w) / 2;
    let ty = (y + (h.saturating_sub(rae_tokens::TYPE_LABEL.line_height as usize)) / 2) as i32;
    let fg = if enabled { TEXT_FG } else { TEXT_MUTED };
    canvas.draw_text_aa(tx, ty, label, rae_tokens::TYPE_LABEL, fg, FontFamily::Sans);
}

/// Borderless toolbar ICON button (back/forward/up) — a quiet frost square
/// carrying a real line-icon, replacing the ASCII "<"/">"/"Up" text pills
/// that read as Win95 chrome (visual-QA).
fn draw_tool_icon_button(
    canvas: &mut Canvas,
    x: usize,
    y: usize,
    sz: usize,
    icon: raegfx::icon::Icon,
    enabled: bool,
) {
    let r = rae_tokens::RADIUS_SM as usize;
    canvas.fill_rounded_rect(x, y, sz, sz, r, rae_tokens::GLASS_POPOVER_DARK.frost);
    let ink = if enabled { TEXT_FG } else { TEXT_MUTED };
    let isz = 16usize;
    canvas.draw_icon(
        icon,
        (x + (sz - isz) / 2) as i32,
        (y + (sz - isz) / 2) as i32,
        isz as i32,
        ink,
    );
}

fn fmt_size(bytes: u64, out: &mut [u8]) -> usize {
    let (val, unit): (u64, &str) = match bytes {
        0..=1023 => (bytes, "B"),
        1024..=1_048_575 => (bytes / 1024, "KB"),
        1_048_576..=1_073_741_823 => (bytes / 1_048_576, "MB"),
        _ => (bytes / 1_073_741_824, "GB"),
    };
    let mut n = fmt_u64(val, out);
    out[n] = b' ';
    n += 1;
    for &b in unit.as_bytes() {
        out[n] = b;
        n += 1;
    }
    n
}

fn fmt_dims(w: u32, h: u32, out: &mut [u8]) -> usize {
    let mut n = 0;
    for &b in b"Image: " {
        out[n] = b;
        n += 1;
    }
    n += fmt_u64(w as u64, &mut out[n..]);
    for &b in b" x " {
        out[n] = b;
        n += 1;
    }
    n += fmt_u64(h as u64, &mut out[n..]);
    for &b in b" px" {
        out[n] = b;
        n += 1;
    }
    n
}

fn fmt_count(n: usize, marked: usize, out: &mut [u8]) -> usize {
    let mut len = fmt_u64(n as u64, out);
    let suffix: &[u8] = if n == 1 { b" item" } else { b" items" };
    for &b in suffix {
        out[len] = b;
        len += 1;
    }
    if marked > 0 {
        for &b in b"  (" {
            out[len] = b;
            len += 1;
        }
        len += fmt_u64(marked as u64, &mut out[len..]);
        for &b in b" marked)" {
            out[len] = b;
            len += 1;
        }
    }
    len
}

fn fmt_marked(n: usize, out: &mut [u8]) -> usize {
    let mut len = fmt_u64(n as u64, out);
    for &b in b" file(s) selected" {
        out[len] = b;
        len += 1;
    }
    len
}

fn fmt_u64(mut v: u64, out: &mut [u8]) -> usize {
    if v == 0 {
        out[0] = b'0';
        return 1;
    }
    let mut tmp = [0u8; 20];
    let mut i = 0;
    while v > 0 {
        tmp[i] = b'0' + (v % 10) as u8;
        v /= 10;
        i += 1;
    }
    let mut n = 0;
    while i > 0 {
        i -= 1;
        out[n] = tmp[i];
        n += 1;
    }
    n
}

// ── Design proof (R10: a fail-able check the token wiring + logic are correct) ─

/// True iff File Manager's chrome is wired to the shared design tokens AND the
/// `rae_files` logic engine produces the expected canonical outputs. Deliberately
/// fail-able: a regression in either the token wiring or the trash/rename/tab
/// logic flips this to `false` (exit code 3 at startup).
#[must_use]
pub fn design_proof() -> bool {
    let ramp = rae_tokens::derive_accent(theme_seed(), &DARK);
    // Liquid Glass window-chrome wiring (visual-QA Round-5 §2 / IDENTITY §7): the
    // window chrome is glass TIERS, not flat dark palette fills. Assert the chrome
    // tiers ARE the canonical `glass.chrome` / `glass.panel` tokens and the content
    // field is the SOLID de-tinted neutral (NOT the old bluish `bg.raised` dark
    // box). FAIL-able: revert any chrome region to an opaque palette fill or re-tint
    // the content field back toward near-black navy and this flips false.
    let glass_chrome_ok = CHROME_TIER.tint == rae_tokens::GLASS_CHROME_DARK.tint
        && CHROME_TIER.frost == rae_tokens::GLASS_CHROME_DARK.frost
        && PANEL_TIER.tint == rae_tokens::GLASS_PANEL_DARK.tint
        && PANEL_TIER.frost == rae_tokens::GLASS_PANEL_DARK.frost
        // content is solid (opaque alpha) and de-tinted: brighter than the old
        // bg.raised AND its blue channel no longer dominates (neutral slate).
        && (CONTENT_BG >> 24) & 0xFF == 0xFF
        && content_luma(CONTENT_BG) > content_luma(DARK.bg_raised)
        && content_is_detinted(CONTENT_BG);
    let tokens_ok = accent() == ramp.base
        && row_sel() == ramp.active
        && BG == CONTENT_BG
        && glass_chrome_ok
        && TEXT_FG == DARK.text_primary
        && TEXT_MUTED == DARK.text_secondary
        && STROKE_HL == DARK.stroke_strong
        && raekit::sys::THEME_DEFAULT_ACCENT == RAEBLUE;

    // §4.4 file-type semantic palette: every icon tint resolves from the SHARED
    // `rae_tokens::FTYPE_*` palette (no private FM_* / FOLDER_FG hardcode), the
    // fixed hues match the design table, AND a Vibe re-skin propagates to the
    // accent-tracking classes (dir/code) while the fixed classes stay put. This
    // is the cohesion contract of this re-skin. FAIL-able: a hardcoded tint, a
    // wrong §4.4 value, or a broken accent-track flips it to `false`.
    let ftype_ok = ftype_proof();

    // Logic engine spot-checks (the host-KAT'd invariants the bin relies on).
    let trash_ok = trash_target("/home/u/a.txt", "/home/u")
        .map(|p| {
            let s = p.as_str();
            s.as_bytes() == b"/home/u/.Trash/a.txt"
        })
        .unwrap_or(false);
    let rename_ok = batch_rename_target("vacation.jpg", "Photo_###", 0, 1, 8)
        .map(|n| n.as_str().as_bytes() == b"Photo_001.jpg")
        .unwrap_or(false);
    let tab_ok = TabSet::new("/home")
        .map(|mut ts| ts.open("/etc").is_ok() && ts.active().cwd() == "/etc")
        .unwrap_or(false);

    // Quick Look decode→blit invariant: build a tiny known PNG, run it through
    // the real `raemedia::png::decode_png` + this app's `blit_image_fit`, and
    // assert a sampled framebuffer pixel matches the expected source color.
    // FAIL-able by construction: a decoder regression, a wrong ARGB mapping, or
    // a broken scale/sample loop flips this to `false` (exit code 3 at startup).
    let image_ok = quick_look_decode_blit_ok();

    // JPEG Quick Look invariant: decode an embedded baseline JPEG through the
    // real `raemedia` decoder + this app's blit, and prove the EXIF-oriented
    // path rotates a portrait upright. FAIL-able: a decode regression, a wrong
    // jpeg→canvas bridge, or a dropped orientation flips this to `false`.
    let jpeg_ok = quick_look_jpeg_decode_ok();

    // GIF Quick Look invariant: sniff + decode a hand-built 2-frame GIF through
    // the real `rae_gif` decoder, take frame 0, and prove it bridges + blits to
    // red (frame 0), not blue (frame 1). FAIL-able: a broken sniff, wrong-frame
    // selection, or a bad bridge/blit flips this to `false`.
    let gif_ok = quick_look_gif_decode_ok();

    // "Extract here" wiring invariant: build a tiny ZIP carrying a safe stored
    // entry ("hello.txt" = "hi") and a path-traversal entry ("../evil"), then run
    // the SAME core the extractor uses (open → per-entry is_safe_path filter →
    // read_entry) and assert: the safe entry CRC-verifies to "hi", AND the
    // traversal entry is REJECTED by is_safe_path (never reaches a write). This
    // proves the Files-side wiring + that the zip-slip gate is actually consulted
    // (rae_zip's 13 host KATs prove the archive logic itself). FAIL-able: dropping
    // the is_safe_path filter, or a CRC/inflate regression, flips this to `false`.
    let zip_ok = extract_zip_slip_guard_ok();

    // "Extract here (.tar.gz)" wiring invariant: build a tiny gzipped ustar tar
    // carrying a safe regular-file entry ("hello.txt" = "hi") and a path-traversal
    // entry ("../evil"), then run the SAME core the tar extractor uses
    // (read_tar_gz → per-entry rae_tar::is_safe_path filter → data()) and assert:
    // the safe entry's bytes are exactly "hi", AND the traversal entry is REJECTED
    // by rae_tar::is_safe_path (never reaches a write). This proves the Files-side
    // tar wiring + that the tar-slip gate is actually consulted (rae_tar's host
    // KATs prove the archive/gzip logic itself). FAIL-able: dropping the
    // is_safe_path filter, or a gunzip/tar-parse regression, flips this to `false`.
    let tar_ok = extract_tar_slip_guard_ok();

    // "Compress here" wiring invariant (the inverse of Extract): build a ZIP from a
    // known (name, bytes) pair using this app's `ZipWriter`, then OPEN it with the
    // already-present `rae_zip::Archive` + `read_entry` and assert the extracted
    // bytes EQUAL the original — proving the writer emits valid, re-readable ZIPs
    // (correct headers, method 0/8 choice, CRC-32). Also asserts `gzip_compress`
    // carries the gzip magic `1F 8B` and round-trips back to the source via
    // `gzip_decompress`. FAIL-able: a wrong header field, a bad CRC, an offset
    // drift in the central directory, or a broken DEFLATE choice flips this to
    // `false` (exit code 3 at startup).
    let compress_ok = compress_roundtrip_ok();

    // Mouse hit-test invariant: the rects `render` DRAWS (via the `geom_*`
    // helpers) are the same rects `build_layout` HIT-TESTS, and each click maps
    // to the right action. FAIL-able: a geometry drift between draw + hit, or a
    // broken dispatch mapping, flips this to `false`.
    let hit_ok = hit_test_proof();

    // Compare (diff) invariant: a known one-line change produces a `-b`/`+B`
    // unified diff via `rae_diff`, AND the render color classification maps
    // `+`→green, `-`→red, ` `→dim, `@`→accent. FAIL-able: a diff-engine
    // regression or a wrong color mapping flips this to `false`.
    let compare_ok = compare_diff_proof();

    // CSV table Quick Look invariant: parse a known CSV with a quoted embedded
    // comma through the SAME `rae_csv::parse` the table view uses, assert the grid
    // shape + the quoted cell survived, AND that this app's column-width
    // computation picks the right per-column max. FAIL-able: a parser regression
    // or a width-computation drift flips this to `false` (exit code 3 at startup).
    let csv_ok = csv_table_proof();

    // Checksum / Verify invariant: the `rae_hash` SHA-256 of "abc" equals the
    // FIPS 180-4 known vector, the algo-by-LENGTH auto-detect picks the right
    // algorithm for 64/40/32/8-char hex, and `verify` is case-insensitive-true
    // for the right hash / false for a wrong one. FAIL-able: a hash regression,
    // a wrong length→algo mapping, or a broken compare flips this to `false`
    // (exit code 3 at startup).
    let checksum_ok = checksum_verify_proof();

    // File-association wiring invariant: rae_mime resolution drives the candidate
    // list + default (a .png → the image app, .txt → the editor, content-sniff
    // beating a mislabeled name), AND "Set as default" overrides + round-trips
    // through the rae_toml persistence (serialize → parse → same default), AND a
    // corrupt config falls back to the built-in defaults without panicking. This
    // is the proof that the "Open With" / default-launch / set-default wiring is
    // real, not a mock. FAIL-able: a wrong default, a broken persistence
    // round-trip, or a panic-on-corrupt-config flips this to `false`.
    let assoc_ok = assoc_proof();

    // Global indexed-search wiring invariant: the raw kernel result blob
    // (`[u64 id][u32 kind][u32 pad]` × N, the SYS_SEARCH_QUERY format) decodes
    // via the SAME `raekit::search::decode_results` the live query uses, the
    // per-kind tally (`summarize_hits`) matches the synthetic set, and the
    // empty-blob case yields the "No results" tally (total == 0), AND the RESOLVED
    // surface (`query_resolved` → `decode_resolved` → named rows) decodes to the
    // exact rendered name+path rows and routes each row to the correct open action
    // (a file → open-by-path, a folder → navigate, a path-less hit → no-op). This
    // proves the Files search consumes the committed raekit surface + maps hits to
    // the exact rendered, openable rows, without a syscall. FAIL-able: a decode
    // regression, a wrong kind→bucket mapping, a miscount, a wrong row text, or a
    // wrong open route flips this to `false`.
    let search_ok = search_proof();

    tokens_ok
        && assoc_ok
        && ftype_ok
        && trash_ok
        && rename_ok
        && tab_ok
        && image_ok
        && jpeg_ok
        && gif_ok
        && zip_ok
        && tar_ok
        && compress_ok
        && hit_ok
        && compare_ok
        && csv_ok
        && checksum_ok
        && search_ok
}

/// Prove the global indexed-search wiring the live Search overlay depends on:
/// the kernel result blob decodes through the committed `raekit::search` surface,
/// the RESOLVED hits (`decode_resolved`) render as the exact named name+path rows
/// the overlay draws, each row routes to the correct open action (file →
/// open-by-path, folder → navigate, path-less → no-op), the per-kind tally header
/// matches, and the empty-index case is the graceful "No results" path. Pure
/// logic — no syscall — so it runs at startup as part of `design_proof`.
/// FAIL-able by construction: every assertion compares against an explicit
/// expected value, so a decode regression, a wrong kind→bucket mapping, a
/// miscount, a wrong row name/path, a wrong open route, or a broken empty-set
/// path flips it to `false` (exit code 3 at startup).
#[must_use]
fn search_proof() -> bool {
    use raekit::syscalls::search::{
        decode_resolved, decode_results, Kind as K, ResolvedHit, SearchHit,
    };

    // 1. Decode the on-the-wire blob (the SYS_SEARCH_QUERY record format:
    //    `[u64 id][u32 kind][u32 pad]`, 16 bytes each) through the SAME wrapper
    //    the live query uses. Build a synthetic 4-record blob: a File, an App,
    //    a Setting, and an Other (kind tag 99 → Other).
    let mut blob = [0u8; 64];
    let put = |b: &mut [u8], i: usize, id: u64, kind: u32| {
        let base = i * 16;
        b[base..base + 8].copy_from_slice(&id.to_le_bytes());
        b[base + 8..base + 12].copy_from_slice(&kind.to_le_bytes());
        // pad [12..16] left zero.
    };
    put(&mut blob, 0, 10, raekit::sys::SEARCH_KIND_FILE);
    put(&mut blob, 1, 20, raekit::sys::SEARCH_KIND_APP);
    put(&mut blob, 2, 30, raekit::sys::SEARCH_KIND_SETTING);
    put(&mut blob, 3, 40, raekit::sys::SEARCH_KIND_OTHER);

    let hits = decode_results(&blob, 4);
    if hits.len() != 4 {
        return false;
    }
    // The decode must preserve id + kind exactly (id 10/File first).
    if hits[0]
        != (SearchHit {
            id: 10,
            kind: K::File,
        })
    {
        return false;
    }
    if hits[1].kind != K::App || hits[2].kind != K::Setting || hits[3].kind != K::Other {
        return false;
    }

    // 2. The tally that drives the rendered rows must match the synthetic set.
    let s = summarize_hits(&hits);
    if s.total != 4 || s.files != 1 || s.apps != 1 || s.settings != 1 || s.other != 1 {
        return false;
    }
    if s.documents != 0 {
        return false;
    }

    // A Document hit folds into its own bucket (a 5th record proves the mapping
    // is not collapsing Document into Other).
    let mut blob2 = [0u8; 16];
    put(&mut blob2, 0, 50, raekit::sys::SEARCH_KIND_DOCUMENT);
    let s2 = summarize_hits(&decode_results(&blob2, 1));
    if s2.documents != 1 || s2.other != 0 || s2.total != 1 {
        return false;
    }

    // 3. The empty-index case → the "No results" tally (total == 0), the panel's
    //    graceful path. An empty slice must NOT decode as anything.
    let none = summarize_hits(&decode_results(&[], 0));
    if none.total != 0 {
        return false;
    }

    // 4. The rendered count line + a breakdown row format to the expected bytes
    //    (the exact strings the panel draws). Singular vs plural is honored.
    let mut line = [0u8; 64];
    let ln = fmt_results_line(4, &mut line);
    if &line[..ln] != b"4 results" {
        return false;
    }
    let mut one = [0u8; 64];
    let on = fmt_results_line(1, &mut one);
    if &one[..on] != b"1 result" {
        return false;
    }
    let mut row = [0u8; 48];
    let rn = fmt_kind_row("Files", 3, &mut row);
    if &row[..rn] != b"Files: 3" {
        return false;
    }

    // ── 5. RESOLVED rows (the named, clickable upgrade) ──────────────────────
    // Encode a synthetic resolved blob in the EXACT wire layout the kernel writes
    // (24-byte header + name bytes + path bytes), the same `decode_resolved` the
    // live `query_resolved` calls. Three rows: a document FILE, a FOLDER, and a
    // path-less APP.
    let enc = |out: &mut Vec<u8>, id: u64, kind: u32, is_folder: bool, name: &str, path: &str| {
        out.extend_from_slice(&id.to_le_bytes());
        out.extend_from_slice(&kind.to_le_bytes());
        out.push(if is_folder { 1 } else { 0 });
        out.push(0u8);
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        out.extend_from_slice(&(path.len() as u16).to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(path.as_bytes());
    };
    let mut rblob: Vec<u8> = Vec::new();
    enc(
        &mut rblob,
        7,
        raekit::sys::SEARCH_KIND_DOCUMENT,
        false,
        "resume.txt",
        "/home/raeen/Documents/resume.txt",
    );
    enc(
        &mut rblob,
        9,
        raekit::sys::SEARCH_KIND_FILE,
        true,
        "Vacation",
        "/home/raeen/Pictures/Vacation",
    );
    enc(
        &mut rblob,
        1,
        raekit::sys::SEARCH_KIND_APP,
        false,
        "Calculator",
        "",
    );

    let rhits = decode_resolved(&rblob, 3);
    if rhits.len() != 3 {
        return false;
    }
    // The decoded rows carry the EXACT name + path the overlay renders.
    if rhits[0]
        != (ResolvedHit {
            id: 7,
            kind: K::Document,
            name: alloc::string::String::from("resume.txt"),
            path: alloc::string::String::from("/home/raeen/Documents/resume.txt"),
            is_folder: false,
        })
    {
        return false;
    }
    if rhits[1].name != "Vacation"
        || rhits[1].path != "/home/raeen/Pictures/Vacation"
        || !rhits[1].is_folder
    {
        return false;
    }
    if rhits[2].name != "Calculator" || !rhits[2].path.is_empty() {
        return false;
    }

    // The per-kind tally HEADER (summarize_resolved): the folder counts as a file
    // (Finder/Explorer parity), the .txt as a document, the app as an app.
    let rs = summarize_resolved(&rhits);
    if rs.total != 3 || rs.files != 1 || rs.documents != 1 || rs.apps != 1 {
        return false;
    }

    // Each row's file-type icon class is correct: a .txt file → Doc, a folder →
    // Dir, an app row (no path, not folder) → Neutral (classify_name("") path).
    if ftype_for_resolved(&rhits[0]) != FType::Doc {
        return false;
    }
    if ftype_for_resolved(&rhits[1]) != FType::Dir {
        return false;
    }

    // The OPEN ROUTING (the clickable upgrade): a file row → open-by-path with its
    // exact path; a folder row → navigate to its exact path; a path-less hit →
    // NoTarget (graceful no-op, never a fabricated path).
    if search_open_route(&rhits[0])
        != SearchOpenRoute::OpenFile(alloc::string::String::from(
            "/home/raeen/Documents/resume.txt",
        ))
    {
        return false;
    }
    if search_open_route(&rhits[1])
        != SearchOpenRoute::NavigateFolder(alloc::string::String::from(
            "/home/raeen/Pictures/Vacation",
        ))
    {
        return false;
    }
    if search_open_route(&rhits[2]) != SearchOpenRoute::NoTarget {
        return false;
    }

    // Empty resolved blob → no rows (the "No results" path), never a panic.
    if !decode_resolved(&[], 0).is_empty() {
        return false;
    }
    if summarize_resolved(&[]).total != 0 {
        // summarize_resolved on an empty slice must be total==0.
        return false;
    }

    // Row hit-test geometry: a click on the second visible row maps to index 1
    // (scroll top 0 for a small set); a click left of the list misses (None).
    let (_cx, _cy, list_x, list_y, list_w) = search_geometry();
    let mid_x = (list_x + list_w / 2) as i32;
    let row1_y = (list_y + SEARCH_ROW_H + SEARCH_ROW_H / 2) as i32;
    if search_row_at(mid_x, row1_y, 0, 3) != Some(1) {
        return false;
    }
    if search_row_at((list_x as i32) - 10, row1_y, 0, 3).is_some() {
        return false;
    }
    // No results → no row can be hit.
    if search_row_at(mid_x, row1_y, 0, 0).is_some() {
        return false;
    }
    // Scroll keeps the selected row visible: with 10 results selected at index 8,
    // the window's top is 3 (8 - 6 + 1) so index 8 is the last visible row.
    if search_scroll_top(8, 10) != 3 {
        return false;
    }
    if search_scroll_top(2, 10) != 0 {
        return false;
    }

    true
}

/// Prove the file-association wiring (rae_mime + rae_toml) the live "Open With" /
/// default-open / set-default paths depend on. Pure logic — no syscalls — so it
/// runs at startup as part of `design_proof`. FAIL-able by construction (every
/// branch compares against an explicit expected value): a wrong resolution, a
/// broken persistence round-trip, or a panic-on-corrupt-config flips it to
/// `false` (exit code 3 at startup).
#[must_use]
fn assoc_proof() -> bool {
    // 1. rae_mime resolution drives the candidate list + default off the built-in
    //    registry the app loads: a .png → the image app, a .txt → the editor.
    let reg = Registry::with_defaults();
    let png = resolve("vacation.png", None, &reg);
    if png.default_app != "photos" || png.candidates.first().map(|s| s.as_str()) != Some("photos") {
        return false;
    }
    let txt = resolve("notes.txt", None, &reg);
    if txt.default_app != "text_editor"
        || txt.candidates.first().map(|s| s.as_str()) != Some("text_editor")
    {
        return false;
    }
    // The default MUST be the first candidate (the overlay relies on index 0 ==
    // default for its bolded/first rendering).
    if png.candidates.first().map(|s| s.as_str()) != Some(png.default_app.as_str()) {
        return false;
    }
    // Content sniff beats a mislabeled name (a PNG body saved as notes.txt → the
    // image app), the same magic-bytes path `resolve_selected` feeds.
    let png_magic = [0x89u8, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
    let sniffed = resolve("notes.txt", Some(&png_magic), &reg);
    if sniffed.default_app != "photos" {
        return false;
    }

    // 2. "Set as default" overrides AND round-trips through the rae_toml
    //    persistence. Override image/png → a custom viewer (chosen-first), then
    //    serialize → parse-back-onto-fresh-defaults → assert the override survived
    //    while an untouched type kept its built-in default. This is the EXACT
    //    serialize/apply pair `save_registry`/`load_registry` use.
    let mut overridden = Registry::with_defaults();
    overridden.set("image/png", "my_viewer", &["my_viewer", "photos", "files"]);
    if overridden.default_app(MimeType("image/png")) != "my_viewer" {
        return false;
    }
    let serialized = serialize_assoc_overrides(&overridden);
    // The serialized form must actually carry the override (a no-op serializer
    // would silently lose it).
    if !serialized.contains("my_viewer") || !serialized.contains("image/png") {
        return false;
    }
    let mut reloaded = Registry::with_defaults();
    apply_assoc_overrides(&mut reloaded, &serialized);
    if reloaded.default_app(MimeType("image/png")) != "my_viewer" {
        return false; // override did NOT survive the round-trip
    }
    // The candidate order is preserved (chosen first).
    let rc = reloaded.candidates(MimeType("image/png"));
    if rc.first().map(|s| s.as_str()) != Some("my_viewer") {
        return false;
    }
    // An untouched type keeps its built-in default after the round-trip (the file
    // carries deltas only; defaults are re-applied on load).
    if reloaded.default_app(MimeType("text/plain")) != "text_editor" {
        return false;
    }

    // 3. A corrupt / garbage config falls back to the built-in defaults without
    //    panicking (the calm-fallback recipe). apply_assoc_overrides on junk must
    //    leave the defaults untouched, never panic.
    let mut from_corrupt = Registry::with_defaults();
    apply_assoc_overrides(&mut from_corrupt, "this is not [valid toml \x00 at all = =");
    if from_corrupt.default_app(MimeType("image/png")) != "photos" {
        return false;
    }
    // An empty document is also fine (no assoc table → defaults intact).
    let mut from_empty = Registry::with_defaults();
    apply_assoc_overrides(&mut from_empty, "");
    if from_empty.default_app(MimeType("text/plain")) != "text_editor" {
        return false;
    }
    // A well-formed doc whose entry is missing required keys is SKIPPED, not fatal.
    let mut from_partial = Registry::with_defaults();
    apply_assoc_overrides(&mut from_partial, "[[assoc]]\nmime = \"image/png\"\n");
    if from_partial.default_app(MimeType("image/png")) != "photos" {
        return false; // a no-default entry must not corrupt the registry
    }

    true
}

/// Prove the §4.4 file-type icon palette is sourced from `rae_tokens` (the
/// cohesion fix), maps each extension class to the right token, and re-skins with
/// Vibe Mode for the accent-tracking classes. FAIL-able by construction: a
/// hardcoded tint, a wrong §4.4 value, a mis-classified extension, or a broken
/// accent-track flips this to `false` (exit code 3 at startup).
#[must_use]
fn ftype_proof() -> bool {
    // 0. Each §4.4 class maps to its real `raegfx::icon` line-icon (the visual-QA
    //    Round-2 fix that retired the letter/block placeholders). FAIL-able: a
    //    swapped or dropped arm (e.g. Dir→File) flips this to `false` (exit 3).
    use raegfx::icon::Icon;
    if ftype_icon(FType::Dir) != Icon::Folder
        || ftype_icon(FType::Code) != Icon::Code
        || ftype_icon(FType::Exec) != Icon::Exec
        || ftype_icon(FType::Media) != Icon::Media
        || ftype_icon(FType::Doc) != Icon::Doc
        || ftype_icon(FType::Archive) != Icon::Archive
        || ftype_icon(FType::Neutral) != Icon::File
    {
        return false;
    }

    // 1. The fixed semantic hues resolve to the §4.4 table values, straight from
    //    rae_tokens (a private FOLDER_FG hardcode would fail this identity).
    if ftype_color(FType::Exec) != rae_tokens::FTYPE_EXEC
        || ftype_color(FType::Media) != rae_tokens::FTYPE_MEDIA
        || ftype_color(FType::Doc) != rae_tokens::FTYPE_DOC
        || ftype_color(FType::Archive) != rae_tokens::FTYPE_ARCHIVE
        || ftype_color(FType::Neutral) != rae_tokens::ftype_neutral(&DARK)
    {
        return false;
    }
    if rae_tokens::FTYPE_MEDIA != 0xFF_C0_7C_FF
        || rae_tokens::FTYPE_DOC != 0xFF_F0_C8_5C
        || rae_tokens::FTYPE_ARCHIVE != 0xFF_F0_A0_3C
        || rae_tokens::FTYPE_EXEC != DARK.state_ok
    {
        return false;
    }

    // 2. The two accent-tracking classes (dir/code) MUST equal the live accent —
    //    proving a Vibe switch propagates to the Files icon tints (no FM_ACCENT).
    let live_accent = rae_tokens::derive_accent(theme_seed(), &DARK).base;
    if ftype_color(FType::Dir) != live_accent || ftype_color(FType::Code) != live_accent {
        return false;
    }
    // …and that the dir tint actually MOVES with a different seed (re-skin), while
    // a fixed class (media) does NOT — the cohesion-vs-fixed distinction.
    let alt_seed = 0xFF_FF_50_80u32;
    if rae_tokens::ftype_dir(alt_seed, &DARK) == rae_tokens::ftype_dir(RAEBLUE, &DARK) {
        return false;
    }
    if rae_tokens::FTYPE_MEDIA == alt_seed {
        return false; // a fixed hue must be seed-independent
    }

    // 3. Extension classification routes the headline cases correctly (kind +
    //    extension → §4.4 class). FAIL-able: a dropped/mis-ordered match arm.
    let cls = |name: &str, kind: Kind| -> FType {
        let mut e = DynamicEntry {
            name: [0; 48],
            name_len: 0,
            kind,
            bytes: 1,
            marked: false,
        };
        let n = name.as_bytes().len().min(48);
        e.name[..n].copy_from_slice(&name.as_bytes()[..n]);
        e.name_len = n;
        classify(&e)
    };
    cls("photos", Kind::Folder) == FType::Dir
        && cls("vacation.JPG", Kind::File) == FType::Media // case-insensitive
        && cls("song.flac", Kind::File) == FType::Media
        && cls("notes.pdf", Kind::File) == FType::Doc
        && cls("data.csv", Kind::File) == FType::Doc
        && cls("backup.zip", Kind::File) == FType::Archive
        && cls("release.tar.gz", Kind::File) == FType::Archive
        && cls("tool.elf", Kind::File) == FType::Exec
        && cls("main.rs", Kind::File) == FType::Code
        && cls("Cargo.toml", Kind::File) == FType::Code
        && cls("mystery.qq", Kind::File) == FType::Neutral
}

/// Prove the Checksum / Verify wiring against the `rae_hash` crate the action
/// calls: the SHA-256 known vector, the algo-by-length auto-detect, and the
/// case-insensitive verify. Returns `false` on any drift (exit code 3 at startup).
#[must_use]
fn checksum_verify_proof() -> bool {
    // 1. SHA-256 of "abc" is the FIPS 180-4 Appendix B.1 vector.
    if rae_hash::to_hex(&rae_hash::sha256(b"abc"))
        != "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    {
        return false;
    }

    // 2. Algo-by-hex-LENGTH auto-detect (the same mapping `run_verify` uses):
    //    64 → SHA-256, 40 → SHA-1, 32 → MD5, 8 → CRC32. Use the real digest
    //    lengths so a future width change is caught here.
    if rae_hash::to_hex(&rae_hash::sha256(b"abc")).len() != 64 {
        return false;
    }
    if rae_hash::to_hex(&rae_hash::sha1(b"abc")).len() != 40 {
        return false;
    }
    if rae_hash::to_hex(&rae_hash::md5(b"abc")).len() != 32 {
        return false;
    }
    if rae_hash::to_hex(&rae_hash::crc32(b"abc").to_be_bytes()).len() != 8 {
        return false;
    }
    // The length→algo decision must select the correct algorithm.
    if algo_for_len(64) != Some(rae_hash::Algo::Sha256)
        || algo_for_len(40) != Some(rae_hash::Algo::Sha1)
        || algo_for_len(32) != Some(rae_hash::Algo::Md5)
        || algo_for_len(8) != Some(rae_hash::Algo::Crc32)
        || algo_for_len(63).is_some()
    {
        return false;
    }

    // 3. Case-insensitive verify: an UPPERCASE expected SHA-256 of "abc" matches,
    //    a wrong hash does not — exactly the run_verify contract.
    if !rae_hash::verify(
        b"abc",
        "BA7816BF8F01CFEA414140DE5DAE2223B00361A396177A9CB410FF61F20015AD",
        rae_hash::Algo::Sha256,
    ) {
        return false;
    }
    if rae_hash::verify(
        b"abc",
        "0000000000000000000000000000000000000000000000000000000000000000",
        rae_hash::Algo::Sha256,
    ) {
        return false;
    }

    // 4. This app's hex_eq_ci (the digest-vs-pasted compare run_verify uses) is
    //    case-insensitive-true for the matching hash and false for a wrong one.
    let sha = rae_hash::to_hex(&rae_hash::sha256(b"abc"));
    if !hex_eq_ci(
        &sha,
        "BA7816BF8F01CFEA414140DE5DAE2223B00361A396177A9CB410FF61F20015AD",
    ) {
        return false;
    }
    if hex_eq_ci(&sha, "deadbeef") {
        return false;
    }

    true
}

/// Map a pasted-hex character LENGTH to the `rae_hash::Algo` it represents
/// (64 = SHA-256, 40 = SHA-1, 32 = MD5, 8 = CRC32). Any other length → `None`.
/// Kept as a function so `run_verify`'s decision is the SAME logic the proof
/// asserts.
fn algo_for_len(len: usize) -> Option<rae_hash::Algo> {
    match len {
        64 => Some(rae_hash::Algo::Sha256),
        40 => Some(rae_hash::Algo::Sha1),
        32 => Some(rae_hash::Algo::Md5),
        8 => Some(rae_hash::Algo::Crc32),
        _ => None,
    }
}

/// Prove the Quick Look CSV table wiring: parse a known CSV (with a quoted
/// embedded comma) through `rae_csv::parse` and assert the grid shape + the
/// quoted cell, then run this app's `csv_column_widths` + `truncate_cell` and
/// assert they pick the right per-column max and truncate char-boundary-safely.
/// Returns `false` on any drift.
#[must_use]
fn csv_table_proof() -> bool {
    // 1. Parse correctness (the quoted-comma case the table relies on).
    let csv = match rae_csv::parse("a,b,c\n1,2,3\n\"x,y\",4,5\n") {
        Ok(c) => c,
        Err(_) => return false,
    };
    if csv.len() != 3 || csv.cols() != 3 {
        return false;
    }
    if csv.cell(2, 0) != Some("x,y") {
        return false;
    }
    // Header == [a, b, c].
    match csv.header() {
        Some(h) => {
            if h.len() != 3 || h[0] != "a" || h[1] != "b" || h[2] != "c" {
                return false;
            }
        }
        None => return false,
    }

    // 2. Column-width computation on a known table. Column 0 holds "a"/"1"/"x,y"
    //    → max display width 3 ("x,y"); columns 1 and 2 hold single chars → 1.
    let widths = csv_column_widths(&csv);
    if widths.len() != 3 || widths[0] != 3 || widths[1] != 1 || widths[2] != 1 {
        return false;
    }

    // 3. A wider table to exercise the cap + a multi-char column max.
    let csv2 = match rae_csv::parse("name,city\nalice,London\nbob,NYC\n") {
        Ok(c) => c,
        Err(_) => return false,
    };
    let w2 = csv_column_widths(&csv2);
    // col0: name/alice/bob → "alice"=5; col1: city/London/NYC → "London"=6.
    if w2.len() != 2 || w2[0] != 5 || w2[1] != 6 {
        return false;
    }

    // 4. Char-boundary-safe truncation: a multibyte cell capped to 3 chars must
    //    yield 3 display chars ending in the ellipsis, never split a codepoint.
    let trunc = truncate_cell("héllo wörld", 3);
    if trunc.chars().count() != 3 || trunc.chars().last() != Some('…') {
        return false;
    }
    // Under-cap is returned verbatim.
    if truncate_cell("ab", 5) != "ab" {
        return false;
    }

    true
}

/// Prove the "Compress here" wiring (the inverse of Extract): the ZIP this app's
/// `ZipWriter` produces is a VALID, re-readable archive, and the gzip path
/// round-trips. Returns `false` on any drift (exit code 3 at startup).
///
/// 1. ZIP round-trip: build a 2-entry ZIP via `ZipWriter` — one COMPRESSIBLE
///    entry (a long repeated run, so DEFLATE/method 8 engages) and one tiny entry
///    (so the STORED/method-0 fallback engages) — then open it with the real
///    `rae_zip::Archive` and `read_entry` (which CRC-verifies) and assert BOTH
///    entries decode byte-for-byte back to their originals, exercising both the
///    method-8 and method-0 paths through the same reader the extractor uses.
/// 2. gzip round-trip: `gzip_compress` a known buffer, assert the `1F 8B` magic
///    prefix, then `gzip_decompress` it and assert the bytes equal the original.
#[must_use]
fn compress_roundtrip_ok() -> bool {
    // (1) ZIP round-trip — both compression methods.
    let compressible: Vec<u8> = {
        let mut v = Vec::new();
        for _ in 0..64 {
            v.extend_from_slice(b"RaeenOS-Files-compress-roundtrip ");
        }
        v
    };
    let tiny: &[u8] = b"hi";

    // NESTED relative paths exercise the recursive-folder-walk's path prefixing:
    // a folder compress emits entries like `dir/a.txt` and `dir/sub/b.txt`. Adding
    // them through the SAME `ZipWriter` and reopening with `rae_zip::Archive` must
    // decode BOTH at their FULL relative-path names, byte-for-byte — proving the
    // writer handles '/'-bearing nested names (and that the path-prefixing the walk
    // builds round-trips). FAIL-able: a writer that mangled the name, or a path that
    // didn't survive, drops one of these names and flips the proof to `false`.
    let nested_a: &[u8] = b"top-level file under dir/";
    let nested_b: &[u8] = b"deeper file under dir/sub/";

    let mut writer = ZipWriter::new();
    writer.add_file("notes.txt", &compressible);
    writer.add_file("a.bin", tiny);
    writer.add_file("dir/a.txt", nested_a);
    writer.add_file("dir/sub/b.txt", nested_b);
    let zip = writer.finish();

    let archive = match Archive::open(&zip) {
        Ok(a) => a,
        Err(_) => return false,
    };
    if archive.entries().len() != 4 {
        return false;
    }

    let mut saw_deflated = false; // the compressible entry must pick method 8
    let mut saw_stored = false; // the tiny entry must fall back to method 0
    let mut notes_ok = false;
    let mut bin_ok = false;
    let mut nested_a_ok = false; // `dir/a.txt` decodes at its full relative name
    let mut nested_b_ok = false; // `dir/sub/b.txt` decodes at its full relative name
    for ze in archive.entries() {
        let data = match archive.read_entry(ze) {
            Ok(d) => d,
            Err(_) => return false,
        };
        if ze.name == "notes.txt" {
            notes_ok = data.as_slice() == compressible.as_slice();
            saw_deflated = ze.method == 8;
        } else if ze.name == "a.bin" {
            bin_ok = data.as_slice() == tiny;
            saw_stored = ze.method == 0;
        } else if ze.name == "dir/a.txt" {
            nested_a_ok = data.as_slice() == nested_a;
        } else if ze.name == "dir/sub/b.txt" {
            nested_b_ok = data.as_slice() == nested_b;
        }
    }
    if !(notes_ok && bin_ok && saw_deflated && saw_stored && nested_a_ok && nested_b_ok) {
        return false;
    }

    // (2) gzip round-trip — magic prefix + decode equality.
    let src: &[u8] = b"compress me to a gzip stream, then back again";
    let gz = rae_deflate::gzip_compress(src);
    if gz.len() < 2 || gz[0] != 0x1F || gz[1] != 0x8B {
        return false;
    }
    match rae_deflate::gzip_decompress(&gz) {
        Ok(back) => back.as_slice() == src,
        Err(_) => false,
    }
}

/// Prove the Compare (unified-diff) wiring (exit code 3 on failure):
///
/// 1. `rae_diff::unified_diff` of a known one-line change ("b" → "B") yields a
///    body with a `-b` removed line and a `+B` added line, plus an `@@` hunk
///    header.
/// 2. The render color classification (`diff_line_color`) maps the leading byte
///    of each line kind to the right token: `+`→`state_ok`, `-`→`state_danger`,
///    ` `→`text_tertiary`, `@`→accent. This is the same function the overlay
///    renderer uses, so a regression there flips this to `false`.
#[must_use]
fn compare_diff_proof() -> bool {
    let diff = unified_diff("a\nb\nc\n", "a\nB\nc\n", 1);

    // (1) Per-line tags present: a removed `-b`, an added `+B`, a `@@` header,
    //     and ` ` context lines (the surrounding a / c).
    let mut saw_removed_b = false;
    let mut saw_added_big_b = false;
    let mut saw_hunk = false;
    let mut saw_context = false;
    for line in diff.split('\n') {
        match line.as_bytes().first().copied() {
            Some(b'-') if line == "-b" => saw_removed_b = true,
            Some(b'+') if line == "+B" => saw_added_big_b = true,
            Some(b'@') => saw_hunk = true,
            Some(b' ') => saw_context = true,
            _ => {}
        }
    }
    if !(saw_removed_b && saw_added_big_b && saw_hunk && saw_context) {
        return false;
    }

    // (2) Color classification is correct AND distinct per kind.
    let added = diff_line_color(Some(b'+'));
    let removed = diff_line_color(Some(b'-'));
    let context = diff_line_color(Some(b' '));
    let header = diff_line_color(Some(b'@'));
    if added != DARK.state_ok
        || removed != DARK.state_danger
        || context != DARK.text_tertiary
        || header != accent()
    {
        return false;
    }
    // The unmarked / no-leading-byte case falls back to context (dim), never a
    // change color (so a blank line isn't painted red/green).
    if diff_line_color(None) != DARK.text_tertiary {
        return false;
    }
    // Added and removed must be visually distinct (the whole point of the view).
    if added == removed {
        return false;
    }
    true
}

/// Prove the mouse hit-test invariant for File Manager (exit code 3 on failure):
///
/// 1. A click at row N's rect-center resolves to `SelectRow(N)`, and dispatching
///    it selects entry N.
/// 2. A click BELOW all visible rows (and in the title area) resolves to no row /
///    no panic.
/// 3. A double-click on a DIRECTORY row dispatches `OpenRow` → a navigation
///    (cwd changes), not a Quick Look.
/// 4. A click on the "Up" toolbar rect fires `GoUp` (cwd's parent).
///
/// Uses a synthetic, deterministic entry set (so row geometry doesn't depend on
/// the live VFS), with a directory entry first and a file second.
#[must_use]
fn hit_test_proof() -> bool {
    // ── Geometry single-source-of-truth: every interactive element's center
    //    hits exactly itself (rects are disjoint). ──
    let app = make_proof_app();
    let layout = app.build_layout();
    for e in layout.as_slice() {
        let (cx, cy) = e.rect.center();
        match layout.hit(cx, cy) {
            Some(a) if a == e.action => {}
            _ => return false,
        }
    }
    // A click far outside the window resolves to nothing (no panic, no-op).
    if layout.hit(-100, -100).is_some()
        || layout.hit(WIN_W as i32 + 80, WIN_H as i32 + 80).is_some()
    {
        return false;
    }

    // (1) Row N's center selects entry N. Pick the SECOND visible row (the file).
    let row1 = geom_row(1);
    let (rcx, rcy) = row1.center();
    match layout.hit(rcx, rcy) {
        Some(Action::SelectRow(1)) => {}
        _ => return false,
    }
    {
        let mut a = make_proof_app();
        if !a.on_click(rcx, rcy) || a.selected != 1 {
            return false;
        }
    }

    // (2) A click below all rows resolves to no row (and never panics). The
    //     area just under the last possible row, inside the list view.
    {
        let below_y = (content_y() + 22 + list_rows_visible() * ROW_H + 4) as i32;
        let lv_mid_x = (SIDEBAR_W + (WIN_W - SIDEBAR_W) / 2) as i32;
        let mut a = make_proof_app();
        // Below all rows → either nothing or (if past the window) no row; must
        // not select a phantom row and must not panic.
        let _ = a.on_click(lv_mid_x, below_y);
        if a.selected >= a.entry_count {
            return false;
        }
    }

    // (3) Double-click the DIRECTORY row (row 0) → navigation, not Quick Look.
    {
        let row0 = geom_row(0);
        let (cx, cy) = row0.center();
        let mut a = make_proof_app();
        let before = path_eq(a.cwd(), "/home/u");
        if !before {
            return false;
        }
        if !a.on_double_click(cx, cy) {
            return false;
        }
        // Opened the "Documents" directory → cwd is now its child, overlay stays
        // None (a file double-click would have opened Quick Look instead).
        if a.overlay != Overlay::None || path_eq(a.cwd(), "/home/u") {
            return false;
        }
    }

    // (4) The "Up" toolbar rect fires GoUp (cwd → parent).
    {
        let up = geom_toolbar()[2].0; // index 2 = Up
        let (cx, cy) = up.center();
        let mut a = make_proof_app();
        // Start one level deep so Up has somewhere to go.
        a.navigate("/home/u/Documents");
        if !path_eq(a.cwd(), "/home/u/Documents") {
            return false;
        }
        if !a.on_click(cx, cy) {
            return false;
        }
        if !path_eq(a.cwd(), "/home/u") {
            return false;
        }
    }

    true
}

/// True iff two paths compare equal ignoring a single trailing slash difference.
fn path_eq(a: &str, b: &str) -> bool {
    a.trim_end_matches('/') == b.trim_end_matches('/')
}

/// Build a File Manager `App` with a deterministic, synthetic entry set for the
/// hit-test proof: home = `/home/u`, two entries — a directory (`Documents`) then
/// a file (`notes.txt`). Independent of the live VFS so row geometry is stable.
fn make_proof_app() -> App {
    let mut app = App::new();
    let mut home = PathBuf::new();
    home.set("/home/u");
    app.home = home;
    let _ = app.tabs.active_mut().navigate("/home/u");
    // Synthetic entries (bypassing readdir): dir first, file second.
    app.entry_count = 0;
    let set = |slot: &mut DynamicEntry, name: &str, kind: Kind| {
        let n = name.as_bytes().len().min(48);
        slot.name[..n].copy_from_slice(&name.as_bytes()[..n]);
        slot.name_len = n;
        slot.kind = kind;
        slot.bytes = if kind == Kind::Folder { 0 } else { 12 };
        slot.marked = false;
    };
    set(&mut app.entries[0], "Documents", Kind::Folder);
    set(&mut app.entries[1], "notes.txt", Kind::File);
    app.entry_count = 2;
    app.selected = 0;
    app.scroll = 0;
    app
}

/// Build a minimal ZIP with one safe stored entry ("hello.txt"="hi") and one
/// zip-slip entry ("../evil"), then exercise the extractor's core logic. Returns
/// `false` on any drift. Self-contained (the ZIP is assembled here from stored
/// records with a live-computed CRC), so the proof carries no fixture file.
fn extract_zip_slip_guard_ok() -> bool {
    let zip = build_test_zip_two();

    let archive = match Archive::open(&zip) {
        Ok(a) => a,
        Err(_) => return false,
    };
    if archive.entries().len() != 2 {
        return false;
    }

    let mut safe_bytes_ok = false;
    let mut saw_traversal_rejected = false;
    let mut wrote_unsafe = false;

    for ze in archive.entries() {
        // The exact gate order the extractor uses: is_safe_path FIRST.
        if !is_safe_path(&ze.name) {
            if ze.name == "../evil" {
                saw_traversal_rejected = true;
            }
            continue; // never written
        }
        // A "safe" path reaching here would be written by the extractor; the
        // traversal name must NOT be in this branch.
        if ze.name == "../evil" {
            wrote_unsafe = true;
        }
        if ze.name == "hello.txt" {
            match archive.read_entry(ze) {
                // read_entry CRC-verifies; bytes must equal "hi".
                Ok(data) => safe_bytes_ok = data.as_slice() == b"hi",
                Err(_) => return false,
            }
        }
    }

    safe_bytes_ok && saw_traversal_rejected && !wrote_unsafe
}

/// Assemble a 32-bit ZIP with two stored (method-0) entries: "hello.txt"="hi"
/// and "../evil"="x". Live-computes each CRC-32 (IEEE) so `read_entry` verifies.
/// Mirrors the layout rae_zip's own test writer produces.
fn build_test_zip_two() -> Vec<u8> {
    fn zip_crc32(data: &[u8]) -> u32 {
        let mut crc: u32 = 0xFFFF_FFFF;
        for &b in data {
            crc ^= b as u32;
            for _ in 0..8 {
                let mask = (crc & 1).wrapping_neg();
                crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
            }
        }
        crc ^ 0xFFFF_FFFF
    }

    const SIG_LOCAL: u32 = 0x0403_4b50;
    const SIG_CENTRAL: u32 = 0x0201_4b50;
    const SIG_EOCD: u32 = 0x0605_4b50;

    let entries: [(&str, &[u8]); 2] = [("hello.txt", b"hi"), ("../evil", b"x")];

    let mut out: Vec<u8> = Vec::new();
    let mut local_offsets: Vec<u32> = Vec::new();

    for (name, raw) in entries.iter() {
        local_offsets.push(out.len() as u32);
        let crc = zip_crc32(raw);
        out.extend_from_slice(&SIG_LOCAL.to_le_bytes());
        out.extend_from_slice(&20u16.to_le_bytes()); // version needed
        out.extend_from_slice(&0u16.to_le_bytes()); // flags
        out.extend_from_slice(&0u16.to_le_bytes()); // method 0 = stored
        out.extend_from_slice(&0u16.to_le_bytes()); // mod time
        out.extend_from_slice(&0u16.to_le_bytes()); // mod date
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&(raw.len() as u32).to_le_bytes()); // comp size
        out.extend_from_slice(&(raw.len() as u32).to_le_bytes()); // uncomp size
        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // extra len
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(raw);
    }

    let cd_offset = out.len() as u32;
    for (i, (name, raw)) in entries.iter().enumerate() {
        let crc = zip_crc32(raw);
        out.extend_from_slice(&SIG_CENTRAL.to_le_bytes());
        out.extend_from_slice(&20u16.to_le_bytes()); // version made by
        out.extend_from_slice(&20u16.to_le_bytes()); // version needed
        out.extend_from_slice(&0u16.to_le_bytes()); // flags
        out.extend_from_slice(&0u16.to_le_bytes()); // method
        out.extend_from_slice(&0u16.to_le_bytes()); // time
        out.extend_from_slice(&0u16.to_le_bytes()); // date
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&(raw.len() as u32).to_le_bytes());
        out.extend_from_slice(&(raw.len() as u32).to_le_bytes());
        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // extra len
        out.extend_from_slice(&0u16.to_le_bytes()); // comment len
        out.extend_from_slice(&0u16.to_le_bytes()); // disk number
        out.extend_from_slice(&0u16.to_le_bytes()); // internal attrs
        out.extend_from_slice(&0u32.to_le_bytes()); // external attrs
        out.extend_from_slice(&local_offsets[i].to_le_bytes());
        out.extend_from_slice(name.as_bytes());
    }
    let cd_size = out.len() as u32 - cd_offset;

    out.extend_from_slice(&SIG_EOCD.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // disk number
    out.extend_from_slice(&0u16.to_le_bytes()); // disk with CD
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&cd_size.to_le_bytes());
    out.extend_from_slice(&cd_offset.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // comment len
    out
}

/// Prove the "Extract here (.tar.gz)" wiring (exit code 3 on failure):
///
/// Build a tiny gzipped ustar tar carrying a safe regular-file entry
/// ("hello.txt" = "hi") and a path-traversal entry ("../evil" = "x"), then run the
/// SAME core `extract_tar_bytes` uses (`read_tar_gz` → per-entry
/// `rae_tar::is_safe_path` filter → `data()`) and assert: the safe entry's bytes
/// equal "hi" AND the traversal entry is REJECTED by the gate (never written).
/// This exercises the gunzip + tar-parse + tar-slip path end-to-end. FAIL-able:
/// dropping the `is_safe_path` filter (the traversal entry would extract), or a
/// gunzip/tar regression (the safe bytes would diverge), flips this to `false`.
#[must_use]
fn extract_tar_slip_guard_ok() -> bool {
    let targz = build_test_targz_two();

    let archive = match read_tar_gz(&targz) {
        Ok(a) => a,
        Err(_) => return false,
    };
    if archive.entries().len() != 2 {
        return false;
    }

    let mut safe_bytes_ok = false;
    let mut saw_traversal_rejected = false;
    let mut wrote_unsafe = false;

    for te in archive.entries() {
        // The exact gate order the extractor uses: rae_tar::is_safe_path FIRST.
        if !rae_tar::is_safe_path(&te.name) {
            if te.name == "../evil" {
                saw_traversal_rejected = true;
            }
            continue; // never written
        }
        // A "safe" path reaching here would be written by the extractor; the
        // traversal name must NOT be in this branch.
        if te.name == "../evil" {
            wrote_unsafe = true;
        }
        if te.name == "hello.txt" {
            safe_bytes_ok = te.kind == TarKind::File && te.data() == b"hi";
        }
    }

    // Stem invariant the routing depends on: a `.tar.gz` name drops BOTH
    // extensions; a bare `.tar` / `.tgz` is handled too.
    let stem_ok = archive_stem("foo.tar.gz") == "foo"
        && archive_stem("bar.TGZ") == "bar"
        && archive_stem("baz.tar") == "baz"
        && archive_stem("q.zip") == "q";

    safe_bytes_ok && saw_traversal_rejected && !wrote_unsafe && stem_ok
}

/// Assemble a gzipped ustar tar with two stored (typeflag-`0`) regular-file
/// entries: "hello.txt"="hi" and "../evil"="x". The tar is built with correct
/// ustar headers + checksums; the gzip wrapper uses a single stored DEFLATE
/// block with a live-computed CRC-32 + ISIZE trailer. Mirrors the construction
/// rae_tar's own host KATs use, so `read_tar_gz` accepts it.
fn build_test_targz_two() -> Vec<u8> {
    /// One 512-byte ustar header with a correct (unsigned) checksum.
    fn ustar_header(name: &str, size: u64) -> [u8; 512] {
        let mut h = [0u8; 512];
        let nb = name.as_bytes();
        let nn = nb.len().min(100);
        h[..nn].copy_from_slice(&nb[..nn]);
        // Octal fields: mode 0644, uid/gid 0, size, mtime 0.
        write_octal(&mut h, 100, 8, 0o644);
        write_octal(&mut h, 124, 12, size);
        h[156] = b'0'; // typeflag: regular file
        h[257..263].copy_from_slice(b"ustar\0");
        h[263..265].copy_from_slice(b"00");
        // Checksum field counts as spaces during the sum.
        for b in h[148..156].iter_mut() {
            *b = b' ';
        }
        let sum: u64 = h.iter().map(|&b| b as u64).sum();
        write_cksum(&mut h, sum);
        h
    }

    /// Write a zero-padded octal value into `h[off..off+len-1]`, NUL terminator
    /// at the final byte.
    fn write_octal(h: &mut [u8; 512], off: usize, len: usize, val: u64) {
        let mut v = val;
        // Fill (len-1) octal digits right-to-left, zero-padded.
        for i in (0..len - 1).rev() {
            h[off + i] = b'0' + (v & 0o7) as u8;
            v >>= 3;
        }
        h[off + len - 1] = 0;
    }

    /// Write the 8-byte checksum field: 6 octal digits, NUL, space.
    fn write_cksum(h: &mut [u8; 512], sum: u64) {
        let mut v = sum;
        for i in (0..6).rev() {
            h[148 + i] = b'0' + (v & 0o7) as u8;
            v >>= 3;
        }
        h[154] = 0;
        h[155] = b' ';
    }

    fn crc32(data: &[u8]) -> u32 {
        let mut crc: u32 = 0xFFFF_FFFF;
        for &b in data {
            crc ^= b as u32;
            for _ in 0..8 {
                let mask = (crc & 1).wrapping_neg();
                crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
            }
        }
        crc ^ 0xFFFF_FFFF
    }

    // ── Build the raw tar ──
    let mut tar: Vec<u8> = Vec::new();
    for (name, body) in [
        ("hello.txt", b"hi".as_slice()),
        ("../evil", b"x".as_slice()),
    ] {
        tar.extend_from_slice(&ustar_header(name, body.len() as u64));
        tar.extend_from_slice(body);
        let pad = (512 - (body.len() % 512)) % 512;
        for _ in 0..pad {
            tar.push(0);
        }
    }
    // Two zero blocks = end of archive.
    tar.extend(core::iter::repeat(0u8).take(1024));

    // ── gzip-wrap it (single stored DEFLATE block) ──
    let mut gz: Vec<u8> = Vec::new();
    gz.extend_from_slice(&[0x1F, 0x8B, 0x08, 0x00]); // magic, CM=8 (DEFLATE), FLG=0
    gz.extend_from_slice(&[0, 0, 0, 0]); // MTIME
    gz.extend_from_slice(&[0x00, 0xFF]); // XFL, OS
                                         // DEFLATE: one final stored block. Stored block len is 16-bit, and the tar is
                                         // ~3 KiB (well under 65535), so a single block suffices.
    gz.push(0x01); // BFINAL=1, BTYPE=00 (stored)
    let len = tar.len() as u16;
    gz.extend_from_slice(&len.to_le_bytes());
    gz.extend_from_slice(&(!len).to_le_bytes());
    gz.extend_from_slice(&tar);
    // Trailer: CRC-32 of the uncompressed tar + ISIZE (mod 2^32).
    gz.extend_from_slice(&crc32(&tar).to_le_bytes());
    gz.extend_from_slice(&(tar.len() as u32).to_le_bytes());
    gz
}

/// Build a 2x2 truecolor PNG with known pixels, decode it, blit it into a
/// scratch surface, and verify a sampled output pixel. Returns `false` on any
/// drift. Self-contained (no external fixture file): the PNG is assembled here
/// from a stored (uncompressed) zlib block with a live-computed CRC/Adler, the
/// same construction the decoder's host KATs use.
fn quick_look_decode_blit_ok() -> bool {
    // 2x2 RGB pixels, filter-0 prefixed per scanline:
    //   row0: red   (255,0,0)   green (0,255,0)
    //   row1: blue  (0,0,255)   white (255,255,255)
    let raw = [
        0u8, 255, 0, 0, 0, 255, 0, // filter, px(0,0), px(1,0)
        0, 0, 0, 255, 255, 255, 255, // filter, px(0,1), px(1,1)
    ];
    let png = build_test_png(2, 2, &raw);

    let img = match decode_png(&png) {
        Ok(i) => i,
        Err(_) => return false,
    };
    // Decoder sanity: dimensions + the four ARGB pixels.
    if img.width != 2 || img.height != 2 {
        return false;
    }
    if img.pixel(0, 0) != Some((0xFF, 255, 0, 0))
        || img.pixel(1, 0) != Some((0xFF, 0, 255, 0))
        || img.pixel(0, 1) != Some((0xFF, 0, 0, 255))
        || img.pixel(1, 1) != Some((0xFF, 255, 255, 255))
    {
        return false;
    }

    // Blit into a scratch ARGB surface and read interior pixels back. An 8x8
    // frame fits the 2x2 image at 4x scale (no letterbox, origin (0,0)); we
    // sample INTERIOR pixels so the 1px stroke/accent frame doesn't perturb the
    // assert. Nearest-neighbor: dest rows/cols 0..3 → source 0, 4..7 → source 1.
    const SW: usize = 8;
    const SH: usize = 8;
    let mut fb = alloc::vec![0u32; SW * SH];
    {
        let mut canvas = unsafe { Canvas::new(fb.as_mut_ptr() as *mut u8, SW, SH, 4) };
        blit_image_fit(&mut canvas, &img, 0, 0, SW, SH);
    }
    // Interior (5,5) nearest-samples source (1,1) = opaque white over the bg.
    if fb[5 * SW + 5] != over(0xFF_FF_FF_FF, DARK.bg_base) {
        return false;
    }
    // Interior (2,2) must be red, not white — guards against a blit that ignores
    // the source row/col and floods one color.
    if fb[2 * SW + 2] != over(0xFF_FF_00_00, DARK.bg_base) {
        return false;
    }
    true
}

// ── Minimal 2-frame GIF fixture for the design_proof gate ───────────────────
//
// Build a 1x1 two-frame GIF89a by hand (no GIF *encoder* in the binary) so the
// Files-side GIF WIRING (sniff → decode_gif → frame-0 bridge → blit) is proven.
// rae_gif's 17 host KATs are the decode-logic proof. Frame 0 is palette index 0
// (red), frame 1 is index 1 (blue) — distinct pixels so a wrong-frame regression
// is visible.

/// LZW-encode a single palette `index` for a 1x1 image at `min_code_size` 2.
/// The stream is exactly `CLEAR(4) index EOI(5)`, each a 3-bit code packed
/// LSB-first — the simplest valid GIF LZW sub-block payload.
fn gif_lzw_single_pixel(index: u8) -> [u8; 2] {
    // 3-bit codes, LSB-first: clear=4 (bits 0..3), index (bits 3..6), eoi=5 (bits 6..9).
    let mut bits: u32 = 0;
    bits |= 4u32; // CLEAR at bit 0
    bits |= (index as u32) << 3; // index at bit 3
    bits |= 5u32 << 6; // EOI at bit 6 (total 9 bits → 2 bytes)
    [(bits & 0xFF) as u8, ((bits >> 8) & 0xFF) as u8]
}

/// Assemble a 1x1, 2-frame GIF89a with a 2-color global table [red, blue].
/// Frame 0 draws index 0, frame 1 draws index 1, each with a 5cs (50ms) delay.
fn build_test_gif() -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    out.extend_from_slice(b"GIF89a");
    out.extend_from_slice(&1u16.to_le_bytes()); // width 1
    out.extend_from_slice(&1u16.to_le_bytes()); // height 1
    out.push(0x80); // global color table present, size field 0 → 2 entries
    out.push(0); // background color index
    out.push(0); // pixel aspect ratio
                 // Global color table: index 0 = red, index 1 = blue.
    out.extend_from_slice(&[255, 0, 0]);
    out.extend_from_slice(&[0, 0, 255]);

    let mut frame = |index: u8| {
        // Graphic Control Extension: delay 5cs (50ms), no transparency, keep.
        out.push(0x21);
        out.push(0xF9);
        out.push(4);
        out.push(1 << 2); // disposal = Keep (1) in bits 2..4
        out.extend_from_slice(&5u16.to_le_bytes());
        out.push(0); // transparent index (unused)
        out.push(0); // block terminator
                     // Image Descriptor at (0,0) size 1x1, no local table.
        out.push(0x2C);
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.push(0); // packed: no local table, not interlaced
        out.push(2); // LZW minimum code size
        let lzw = gif_lzw_single_pixel(index);
        out.push(lzw.len() as u8); // single sub-block
        out.extend_from_slice(&lzw);
        out.push(0); // sub-block terminator
    };
    frame(0);
    frame(1);
    out.push(0x3B); // trailer
    out
}

/// Prove the Files GIF Quick Look wiring: sniff a hand-built 2-frame GIF, decode
/// it through the real `rae_gif::decode_gif`, take frame 0, bridge + blit it, and
/// assert the sampled framebuffer pixel is red (frame 0), NOT blue (frame 1).
/// FAIL-able: a broken sniff, a wrong frame selection, a bad bridge, or a blit
/// regression flips this to `false` (exit code 3 at startup).
#[must_use]
fn quick_look_gif_decode_ok() -> bool {
    let gif = build_test_gif();
    // The sniff the open_quick_look path uses must recognize it.
    if !is_gif_signature(&gif) {
        return false;
    }
    let decoded = match decode_gif(&gif) {
        Ok(g) => g,
        Err(_) => return false,
    };
    // Two distinct frames, with the per-frame delay carried through.
    if decoded.frames.len() != 2 {
        return false;
    }
    if decoded.frames[0].pixels == decoded.frames[1].pixels {
        return false; // frames must differ (red vs blue)
    }
    if decoded.frames[0].delay_ms != 50 {
        return false;
    }
    // Frame 0 pixel must be opaque red; frame 1 opaque blue.
    if decoded.frames[0].pixels.first().copied() != Some(0xFF_FF_00_00)
        || decoded.frames[1].pixels.first().copied() != Some(0xFF_00_00_FF)
    {
        return false;
    }
    // Bridge frame 0 → blit → sample the framebuffer (the Quick Look preview path).
    let img = gif_frame_to_canvas_image(
        decoded.width,
        decoded.height,
        decoded.frames[0].pixels.clone(),
    );
    if img.width != 1 || img.height != 1 {
        return false;
    }
    const SW: usize = 8;
    const SH: usize = 8;
    let mut fb = alloc::vec![0u32; SW * SH];
    {
        let mut canvas = unsafe { Canvas::new(fb.as_mut_ptr() as *mut u8, SW, SH, 4) };
        blit_image_fit(&mut canvas, &img, 0, 0, SW, SH);
    }
    // The 1x1 red frame fills the whole letterbox content; an interior sample must
    // be red over the bg — NOT blue (which would mean frame 1 was blitted).
    if fb[4 * SW + 4] != over(0xFF_FF_00_00, DARK.bg_base) {
        return false;
    }
    if fb[4 * SW + 4] == over(0xFF_00_00_FF, DARK.bg_base) {
        return false;
    }
    true
}

// ── Embedded baseline-JPEG fixtures for the design_proof gate ──────────────
//
// These are real, spec-valid baseline JPEG byte streams (produced off-line by
// the same from-scratch encoder raemedia's host KATs use, then verified to
// round-trip through `raemedia::jpeg::decode_jpeg`). They are embedded (not
// encoded at runtime) so the Files binary carries no JPEG *encoder* — the
// decode-logic proof is raemedia's 165 host KATs; this fixture proves the FILES
// WIRING (sniff → decode_jpeg_oriented → bridge → blit).
//
// `JPEG_GRAY8`: an 8x8 flat-gray (luma 160) baseline JPEG. Decodes to a uniform
// ARGB `(0xFF, 160, 160, 160)` image (DCT/quant preserves a flat plane exactly).
const JPEG_GRAY8: [u8; 314] = [
    0xFF, 0xD8, 0xFF, 0xDB, 0x00, 0x43, 0x00, 0x10, 0x0B, 0x0A, 0x10, 0x18, 0x28, 0x33, 0x3D, 0x0C,
    0x0C, 0x0E, 0x13, 0x1A, 0x3A, 0x3C, 0x37, 0x0E, 0x0D, 0x10, 0x18, 0x28, 0x39, 0x45, 0x38, 0x0E,
    0x11, 0x16, 0x1D, 0x33, 0x57, 0x50, 0x3E, 0x12, 0x16, 0x25, 0x38, 0x44, 0x6D, 0x67, 0x4D, 0x18,
    0x23, 0x37, 0x40, 0x51, 0x68, 0x71, 0x5C, 0x31, 0x40, 0x4E, 0x57, 0x67, 0x79, 0x78, 0x65, 0x48,
    0x5C, 0x5F, 0x62, 0x70, 0x64, 0x67, 0x63, 0xFF, 0xC0, 0x00, 0x0B, 0x08, 0x00, 0x08, 0x00, 0x08,
    0x01, 0x01, 0x11, 0x00, 0xFF, 0xC4, 0x00, 0x1F, 0x00, 0x00, 0x01, 0x05, 0x01, 0x01, 0x01, 0x01,
    0x01, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06,
    0x07, 0x08, 0x09, 0x0A, 0x0B, 0xFF, 0xC4, 0x00, 0xB5, 0x10, 0x00, 0x02, 0x01, 0x03, 0x03, 0x02,
    0x04, 0x03, 0x05, 0x05, 0x04, 0x04, 0x00, 0x00, 0x01, 0x7D, 0x01, 0x02, 0x03, 0x00, 0x04, 0x11,
    0x05, 0x12, 0x21, 0x31, 0x41, 0x06, 0x13, 0x51, 0x61, 0x07, 0x22, 0x71, 0x14, 0x32, 0x81, 0x91,
    0xA1, 0x08, 0x23, 0x42, 0xB1, 0xC1, 0x15, 0x52, 0xD1, 0xF0, 0x24, 0x33, 0x62, 0x72, 0x82, 0x09,
    0x0A, 0x16, 0x17, 0x18, 0x19, 0x1A, 0x25, 0x26, 0x27, 0x28, 0x29, 0x2A, 0x34, 0x35, 0x36, 0x37,
    0x38, 0x39, 0x3A, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49, 0x4A, 0x53, 0x54, 0x55, 0x56, 0x57,
    0x58, 0x59, 0x5A, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68, 0x69, 0x6A, 0x73, 0x74, 0x75, 0x76, 0x77,
    0x78, 0x79, 0x7A, 0x83, 0x84, 0x85, 0x86, 0x87, 0x88, 0x89, 0x8A, 0x92, 0x93, 0x94, 0x95, 0x96,
    0x97, 0x98, 0x99, 0x9A, 0xA2, 0xA3, 0xA4, 0xA5, 0xA6, 0xA7, 0xA8, 0xA9, 0xAA, 0xB2, 0xB3, 0xB4,
    0xB5, 0xB6, 0xB7, 0xB8, 0xB9, 0xBA, 0xC2, 0xC3, 0xC4, 0xC5, 0xC6, 0xC7, 0xC8, 0xC9, 0xCA, 0xD2,
    0xD3, 0xD4, 0xD5, 0xD6, 0xD7, 0xD8, 0xD9, 0xDA, 0xE1, 0xE2, 0xE3, 0xE4, 0xE5, 0xE6, 0xE7, 0xE8,
    0xE9, 0xEA, 0xF1, 0xF2, 0xF3, 0xF4, 0xF5, 0xF6, 0xF7, 0xF8, 0xF9, 0xFA, 0xFF, 0xDA, 0x00, 0x08,
    0x01, 0x01, 0x00, 0x00, 0x3F, 0x00, 0xD0, 0xAF, 0xFF, 0xD9,
];

/// `JPEG_COLOR16_EXIF6`: a 16x16 baseline JPEG (4:4:4), top half red / bottom
/// half blue in SENSOR order, wrapped with an EXIF APP1 carrying Orientation=6
/// (Rotate90Cw). After `decode_jpeg_oriented` the top-red band rotates into the
/// RIGHT column and the bottom-blue band into the LEFT column — the exact
/// transform that makes a phone portrait display upright.
const JPEG_COLOR16_EXIF6: [u8; 442] = [
    0xFF, 0xD8, 0xFF, 0xE1, 0x00, 0x22, 0x45, 0x78, 0x69, 0x66, 0x00, 0x00, 0x49, 0x49, 0x2A, 0x00,
    0x08, 0x00, 0x00, 0x00, 0x01, 0x00, 0x12, 0x01, 0x03, 0x00, 0x01, 0x00, 0x00, 0x00, 0x06, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xFF, 0xDB, 0x00, 0x43, 0x00, 0x10, 0x0B, 0x0A, 0x10, 0x18,
    0x28, 0x33, 0x3D, 0x0C, 0x0C, 0x0E, 0x13, 0x1A, 0x3A, 0x3C, 0x37, 0x0E, 0x0D, 0x10, 0x18, 0x28,
    0x39, 0x45, 0x38, 0x0E, 0x11, 0x16, 0x1D, 0x33, 0x57, 0x50, 0x3E, 0x12, 0x16, 0x25, 0x38, 0x44,
    0x6D, 0x67, 0x4D, 0x18, 0x23, 0x37, 0x40, 0x51, 0x68, 0x71, 0x5C, 0x31, 0x40, 0x4E, 0x57, 0x67,
    0x79, 0x78, 0x65, 0x48, 0x5C, 0x5F, 0x62, 0x70, 0x64, 0x67, 0x63, 0xFF, 0xDB, 0x00, 0x43, 0x01,
    0x11, 0x12, 0x18, 0x2F, 0x63, 0x63, 0x63, 0x63, 0x12, 0x15, 0x1A, 0x42, 0x63, 0x63, 0x63, 0x63,
    0x18, 0x1A, 0x38, 0x63, 0x63, 0x63, 0x63, 0x63, 0x2F, 0x42, 0x63, 0x63, 0x63, 0x63, 0x63, 0x63,
    0x63, 0x63, 0x63, 0x63, 0x63, 0x63, 0x63, 0x63, 0x63, 0x63, 0x63, 0x63, 0x63, 0x63, 0x63, 0x63,
    0x63, 0x63, 0x63, 0x63, 0x63, 0x63, 0x63, 0x63, 0x63, 0x63, 0x63, 0x63, 0x63, 0x63, 0x63, 0x63,
    0xFF, 0xC0, 0x00, 0x11, 0x08, 0x00, 0x10, 0x00, 0x10, 0x03, 0x01, 0x11, 0x00, 0x02, 0x11, 0x01,
    0x03, 0x11, 0x01, 0xFF, 0xC4, 0x00, 0x1F, 0x00, 0x00, 0x01, 0x05, 0x01, 0x01, 0x01, 0x01, 0x01,
    0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07,
    0x08, 0x09, 0x0A, 0x0B, 0xFF, 0xC4, 0x00, 0xB5, 0x10, 0x00, 0x02, 0x01, 0x03, 0x03, 0x02, 0x04,
    0x03, 0x05, 0x05, 0x04, 0x04, 0x00, 0x00, 0x01, 0x7D, 0x01, 0x02, 0x03, 0x00, 0x04, 0x11, 0x05,
    0x12, 0x21, 0x31, 0x41, 0x06, 0x13, 0x51, 0x61, 0x07, 0x22, 0x71, 0x14, 0x32, 0x81, 0x91, 0xA1,
    0x08, 0x23, 0x42, 0xB1, 0xC1, 0x15, 0x52, 0xD1, 0xF0, 0x24, 0x33, 0x62, 0x72, 0x82, 0x09, 0x0A,
    0x16, 0x17, 0x18, 0x19, 0x1A, 0x25, 0x26, 0x27, 0x28, 0x29, 0x2A, 0x34, 0x35, 0x36, 0x37, 0x38,
    0x39, 0x3A, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49, 0x4A, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58,
    0x59, 0x5A, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68, 0x69, 0x6A, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78,
    0x79, 0x7A, 0x83, 0x84, 0x85, 0x86, 0x87, 0x88, 0x89, 0x8A, 0x92, 0x93, 0x94, 0x95, 0x96, 0x97,
    0x98, 0x99, 0x9A, 0xA2, 0xA3, 0xA4, 0xA5, 0xA6, 0xA7, 0xA8, 0xA9, 0xAA, 0xB2, 0xB3, 0xB4, 0xB5,
    0xB6, 0xB7, 0xB8, 0xB9, 0xBA, 0xC2, 0xC3, 0xC4, 0xC5, 0xC6, 0xC7, 0xC8, 0xC9, 0xCA, 0xD2, 0xD3,
    0xD4, 0xD5, 0xD6, 0xD7, 0xD8, 0xD9, 0xDA, 0xE1, 0xE2, 0xE3, 0xE4, 0xE5, 0xE6, 0xE7, 0xE8, 0xE9,
    0xEA, 0xF1, 0xF2, 0xF3, 0xF4, 0xF5, 0xF6, 0xF7, 0xF8, 0xF9, 0xFA, 0xFF, 0xDA, 0x00, 0x0C, 0x03,
    0x01, 0x00, 0x02, 0x00, 0x03, 0x00, 0x00, 0x3F, 0x00, 0xC8, 0xAA, 0x35, 0xD4, 0x51, 0x45, 0x15,
    0x9F, 0x5D, 0xB5, 0x72, 0x14, 0x51, 0x45, 0x7F, 0xFF, 0xD9,
];

/// Decode the embedded baseline JPEG fixtures through the real `raemedia`
/// decoder + this app's bridge/blit, asserting (1) a flat-gray JPEG decodes to
/// the expected ARGB and blits, and (2) the EXIF-oriented portrait rotates
/// upright. Returns `false` on any drift (exit code 3 at startup). Mirrors
/// `quick_look_decode_blit_ok` for the PNG path. Channel asserts allow a small
/// tolerance for DCT/quant rounding (the decoder is lossy by design).
fn quick_look_jpeg_decode_ok() -> bool {
    // (1) Flat-gray sniff + decode + bridge. SOI magic must be recognized.
    if !is_jpeg_signature(&JPEG_GRAY8) {
        return false;
    }
    let gray = match decode_jpeg_oriented(&JPEG_GRAY8) {
        Ok(i) => jpeg_to_canvas_image(i),
        Err(_) => return false,
    };
    if gray.width != 8 || gray.height != 8 {
        return false;
    }
    // Center pixel: opaque, all three channels ~160 (flat gray survives exactly).
    match gray.pixel(4, 4) {
        Some((a, r, g, b)) => {
            if a != 0xFF || (r as i32 - 160).abs() > 3 || r != g || g != b {
                return false;
            }
        }
        None => return false,
    }

    // Blit the gray image into a scratch surface and confirm a sampled pixel
    // matches the source gray composited over the letterbox background — proves
    // the JPEG path reaches the same `blit_image_fit` PNG uses.
    const SW: usize = 16;
    const SH: usize = 16;
    let mut fb = alloc::vec![0u32; SW * SH];
    {
        let mut canvas = unsafe { Canvas::new(fb.as_mut_ptr() as *mut u8, SW, SH, 4) };
        blit_image_fit(&mut canvas, &gray, 0, 0, SW, SH);
    }
    // Interior sample: the decoded gray pixel value over the bg. Reconstruct the
    // expected ARGB from the actual decoded center (tolerant of DCT rounding).
    let (_, gr, gg, gb) = gray.pixel(4, 4).unwrap();
    let expect = over(
        0xFF00_0000 | ((gr as u32) << 16) | ((gg as u32) << 8) | gb as u32,
        DARK.bg_base,
    );
    if fb[8 * SW + 8] != expect {
        return false;
    }

    // (2) EXIF orientation: the portrait (top-red/bottom-blue in sensor order,
    // Orientation=6) must rotate so red lands in the RIGHT column and blue in
    // the LEFT column. A dropped/ignored orientation leaves red at the top and
    // trips this.
    let oriented = match decode_jpeg_oriented(&JPEG_COLOR16_EXIF6) {
        Ok(i) => jpeg_to_canvas_image(i),
        Err(_) => return false,
    };
    if oriented.width != 16 || oriented.height != 16 {
        return false;
    }
    let right = match oriented.pixel(15, 8) {
        Some(p) => p,
        None => return false,
    };
    let left = match oriented.pixel(0, 8) {
        Some(p) => p,
        None => return false,
    };
    // Right column = red (R dominates), left column = blue (B dominates).
    let red_ok = right.1 as i32 > right.3 as i32 + 40 && right.1 > 120;
    let blue_ok = left.3 as i32 > left.1 as i32 + 40 && left.3 > 120;
    if !red_ok || !blue_ok {
        return false;
    }

    true
}

/// Assemble a truecolor (8-bit RGB) PNG from already-filtered scanline bytes,
/// using a single stored DEFLATE block. Mirrors the decoder's own test encoder.
fn build_test_png(width: u32, height: u32, raw_scanlines: &[u8]) -> Vec<u8> {
    fn crc32(bytes: &[u8]) -> u32 {
        let mut crc: u32 = 0xFFFF_FFFF;
        for &b in bytes {
            crc ^= b as u32;
            for _ in 0..8 {
                if crc & 1 != 0 {
                    crc = (crc >> 1) ^ 0xEDB8_8320;
                } else {
                    crc >>= 1;
                }
            }
        }
        crc ^ 0xFFFF_FFFF
    }
    fn adler32(data: &[u8]) -> u32 {
        let (mut a, mut b): (u32, u32) = (1, 0);
        for &byte in data {
            a = (a + byte as u32) % 65521;
            b = (b + a) % 65521;
        }
        (b << 16) | a
    }
    fn push_chunk(out: &mut Vec<u8>, ctype: &[u8; 4], payload: &[u8]) {
        out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        out.extend_from_slice(ctype);
        out.extend_from_slice(payload);
        let mut crc_input = Vec::new();
        crc_input.extend_from_slice(ctype);
        crc_input.extend_from_slice(payload);
        out.extend_from_slice(&crc32(&crc_input).to_be_bytes());
    }

    let mut out = Vec::new();
    out.extend_from_slice(&PNG_SIG);

    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.push(8); // bit depth
    ihdr.push(2); // color type 2 = truecolor RGB
    ihdr.push(0); // compression
    ihdr.push(0); // filter
    ihdr.push(0); // interlace
    push_chunk(&mut out, b"IHDR", &ihdr);

    // zlib: 0x78 0x01 header, one final stored block, Adler32 trailer.
    let mut idat = Vec::new();
    idat.push(0x78);
    idat.push(0x01);
    idat.push(0x01); // BFINAL=1, BTYPE=00
    let len = raw_scanlines.len() as u16;
    idat.extend_from_slice(&len.to_le_bytes());
    idat.extend_from_slice(&(!len).to_le_bytes());
    idat.extend_from_slice(raw_scanlines);
    idat.extend_from_slice(&adler32(raw_scanlines).to_be_bytes());
    push_chunk(&mut out, b"IDAT", &idat);

    push_chunk(&mut out, b"IEND", &[]);
    out
}

// ── Entry point ─────────────────────────────────────────────────────────

/// The live Files application: create the surface, render, and run the input
/// loop forever. Called by the `src/main.rs` bin's `_start`. Diverges (`-> !`)
/// — it only returns via `raekit::sys::exit`.
pub fn run() -> ! {
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
    raekit::sys::surface_present(sid, 240, 90);

    let mut extended = false;

    // Live left-button state across frames, for click-EDGE detection (fire once
    // on was-up -> now-down, not every frame the button is held), plus the last
    // click's time + row for DOUBLE-click detection.
    let mut left_was_down = false;
    let mut last_click_ns: u64 = 0;
    let mut last_click_row: i64 = -1;

    loop {
        // ── Mouse: drain button events for live state, hit-test the cursor ────
        // `poll_mouse()` is a destructive per-event queue (bit0 = left button);
        // drain it to the latest level, then on the up->down edge read the
        // absolute cursor, convert to surface-local space, and dispatch a
        // single- or double-click.
        let mut mouse_activity = false;
        let mut left_down = left_was_down;
        loop {
            let ev = raekit::sys::poll_mouse();
            if ev == 0 {
                break;
            }
            left_down = (ev & 0x01) != 0;
            mouse_activity = true;
        }
        if mouse_activity || left_down != left_was_down {
            if left_down && !left_was_down && app.overlay == Overlay::Compare {
                // Compare overlay is modal: a click anywhere dismisses it (mouse
                // close affordance) and is NOT routed to the file list beneath the
                // scrim. Keyboard scroll/Esc is the primary path.
                app.overlay = Overlay::None;
                app.compare_diff = Vec::new();
                app.compare_scroll = 0;
                render(&app, &mut canvas);
                raekit::sys::surface_present(sid, PRESENT_X as u64, PRESENT_Y as u64);
            } else if left_down && !left_was_down && app.overlay == Overlay::Checksum {
                // Checksum overlay: the close (×) button dismisses it; a click on
                // the verify field is a focus no-op (typing always targets it);
                // any other click is swallowed (not routed to the list beneath).
                let (cx, cy, _btn) = raekit::sys::cursor_pos();
                let (ox, oy) = raekit::sys::surface_origin(sid)
                    .unwrap_or((PRESENT_X as u32, PRESENT_Y as u32));
                let lx = (cx as i32).saturating_sub(ox as i32);
                let ly = (cy as i32).saturating_sub(oy as i32);
                if checksum_close_rect().contains(lx, ly) {
                    app.overlay = Overlay::None;
                    app.checksum_sha256 = String::new();
                    app.checksum_sha1 = String::new();
                    app.checksum_md5 = String::new();
                    app.checksum_crc32 = String::new();
                    app.verify_len = 0;
                    app.verify_result = VerifyResult::None;
                    render(&app, &mut canvas);
                    raekit::sys::surface_present(sid, PRESENT_X as u64, PRESENT_Y as u64);
                }
            } else if left_down && !left_was_down && app.overlay == Overlay::OpenWith {
                // Open With overlay is modal: a click dismisses it (mouse close
                // affordance) and is NOT routed to the file list beneath the scrim.
                // The keyboard (Up/Down + Enter/d) is the primary path for picking
                // and setting the default.
                app.close_open_with();
                render(&app, &mut canvas);
                raekit::sys::surface_present(sid, PRESENT_X as u64, PRESENT_Y as u64);
            } else if left_down && !left_was_down && app.overlay == Overlay::Search {
                // Search overlay: a click on a NAMED result row selects + opens it
                // (the mouse counterpart of Up/Down + Enter); a click anywhere else
                // (the scrim) dismisses the overlay.
                let (cx, cy, _btn) = raekit::sys::cursor_pos();
                let (ox, oy) = raekit::sys::surface_origin(sid)
                    .unwrap_or((PRESENT_X as u32, PRESENT_Y as u32));
                let lx = (cx as i32).saturating_sub(ox as i32);
                let ly = (cy as i32).saturating_sub(oy as i32);
                let total = app.search_results.len();
                match search_row_at(lx, ly, app.search_selected, total) {
                    Some(idx) => {
                        app.search_selected = idx;
                        app.search_open_selected();
                    }
                    None => app.close_search(),
                }
                render(&app, &mut canvas);
                raekit::sys::surface_present(sid, PRESENT_X as u64, PRESENT_Y as u64);
            } else if left_down && !left_was_down {
                let (cx, cy, _btn) = raekit::sys::cursor_pos();
                // Subtract the LIVE window origin (not the stale present-time
                // PRESENT_X/Y) so clicks land correctly after the window manager
                // moves the window (Overview / Spaces / tiling). Falls back to the
                // present origin if the surface isn't found. Saturating-sub keeps a
                // cursor above/left of the window from underflowing.
                let (ox, oy) = raekit::sys::surface_origin(sid)
                    .unwrap_or((PRESENT_X as u32, PRESENT_Y as u32));
                let lx = (cx as i32).saturating_sub(ox as i32);
                let ly = (cy as i32).saturating_sub(oy as i32);
                // Identify which row (if any) this edge landed on, for the
                // double-click same-row test.
                let row_now: i64 = match app.build_layout().hit(lx, ly) {
                    Some(Action::SelectRow(idx)) => idx as i64,
                    _ => -1,
                };
                let now = raekit::sys::time_ns();
                let is_double = row_now >= 0
                    && row_now == last_click_row
                    && now.saturating_sub(last_click_ns) <= DOUBLE_CLICK_NS;

                let changed = if is_double {
                    // Consume the streak so a triple-click doesn't re-open.
                    last_click_row = -1;
                    app.on_double_click(lx, ly)
                } else {
                    last_click_ns = now;
                    last_click_row = row_now;
                    app.on_click(lx, ly)
                };
                if changed {
                    render(&app, &mut canvas);
                    raekit::sys::surface_present(sid, PRESENT_X as u64, PRESENT_Y as u64);
                }
            }
            left_was_down = left_down;
        }

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

        if code == 0x2A || code == 0x36 {
            app.shift = !release;
            continue;
        }
        if release {
            continue;
        }

        let mut dirty = false;

        // Modal overlays capture input first.
        match app.overlay {
            Overlay::QuickLook => {
                // CSV/TSV table: arrow/page keys scroll the data rows (Space is a
                // scroll-page-ish no-op there; Esc still closes). Otherwise Space or
                // Esc closes (the original image/text/hex behavior, unchanged).
                let is_table = app.preview_csv.is_some();
                let is_doc = app.preview_doc.is_some();
                if is_doc {
                    // Document text (PDF/DOCX): arrow/page keys scroll lines.
                    let page = 12i32;
                    match (ext, code) {
                        (true, 0x48) => {
                            app.doc_scroll_by(-1);
                            dirty = true;
                        } // Up
                        (true, 0x50) => {
                            app.doc_scroll_by(1);
                            dirty = true;
                        } // Down
                        (true, 0x49) => {
                            app.doc_scroll_by(-page);
                            dirty = true;
                        } // PageUp
                        (true, 0x51) => {
                            app.doc_scroll_by(page);
                            dirty = true;
                        } // PageDown
                        (false, 0x01) => {
                            // Esc closes; drop the (possibly large) extracted text.
                            app.overlay = Overlay::None;
                            app.preview_doc = None;
                            app.preview_doc_scroll = 0;
                            app.preview_doc_label = "";
                            dirty = true;
                        }
                        _ => {}
                    }
                } else if is_table {
                    let page = 12i32;
                    match (ext, code) {
                        (true, 0x48) => {
                            app.csv_scroll_by(-1);
                            dirty = true;
                        } // Up
                        (true, 0x50) => {
                            app.csv_scroll_by(1);
                            dirty = true;
                        } // Down
                        (true, 0x49) => {
                            app.csv_scroll_by(-page);
                            dirty = true;
                        } // PageUp
                        (true, 0x51) => {
                            app.csv_scroll_by(page);
                            dirty = true;
                        } // PageDown
                        (false, 0x01) => {
                            // Esc closes; drop the (possibly large) parsed table.
                            app.overlay = Overlay::None;
                            app.preview_csv = None;
                            app.preview_csv_scroll = 0;
                            dirty = true;
                        }
                        _ => {}
                    }
                } else if code == 0x39 || code == 0x01 {
                    // Space or Esc closes (image/text/hex Quick Look).
                    app.overlay = Overlay::None;
                    dirty = true;
                }
                if dirty {
                    render(&app, &mut canvas);
                    raekit::sys::surface_present(sid, 240, 90);
                }
                continue;
            }
            Overlay::BatchRename => {
                match (ext, code) {
                    (false, 0x01) => {
                        // Esc cancels.
                        app.overlay = Overlay::None;
                        dirty = true;
                    }
                    (false, 0x1C) => {
                        // Enter applies.
                        app.apply_batch_rename();
                        dirty = true;
                    }
                    (false, 0x0E) => {
                        app.pattern_backspace();
                        dirty = true;
                    }
                    _ => {
                        if let Some(c) = scancode_to_ascii(code, app.shift) {
                            app.pattern_push(c);
                            dirty = true;
                        }
                    }
                }
                if dirty {
                    render(&app, &mut canvas);
                    raekit::sys::surface_present(sid, 240, 90);
                }
                continue;
            }
            Overlay::Compare => {
                let page = compare_visible_rows().max(1) as i32;
                match (ext, code) {
                    (false, 0x01) => {
                        // Esc closes; drop the (possibly large) diff buffer.
                        app.overlay = Overlay::None;
                        app.compare_diff = Vec::new();
                        app.compare_scroll = 0;
                        dirty = true;
                    }
                    (true, 0x48) => {
                        app.compare_scroll_by(-1);
                        dirty = true;
                    } // Up
                    (true, 0x50) => {
                        app.compare_scroll_by(1);
                        dirty = true;
                    } // Down
                    (true, 0x49) => {
                        app.compare_scroll_by(-page);
                        dirty = true;
                    } // PageUp
                    (true, 0x51) => {
                        app.compare_scroll_by(page);
                        dirty = true;
                    } // PageDown
                    _ => {}
                }
                if dirty {
                    render(&app, &mut canvas);
                    raekit::sys::surface_present(sid, 240, 90);
                }
                continue;
            }
            Overlay::Checksum => {
                match (ext, code) {
                    (false, 0x01) => {
                        // Esc closes; drop the (heap) digest strings + verify field.
                        app.overlay = Overlay::None;
                        app.checksum_sha256 = String::new();
                        app.checksum_sha1 = String::new();
                        app.checksum_md5 = String::new();
                        app.checksum_crc32 = String::new();
                        app.verify_len = 0;
                        app.verify_result = VerifyResult::None;
                        dirty = true;
                    }
                    (false, 0x1C) => {
                        // Enter: verify the pasted expected hash against the file.
                        app.run_verify();
                        dirty = true;
                    }
                    (false, 0x0E) => {
                        app.verify_backspace();
                        dirty = true;
                    }
                    _ => {
                        // Accept hex digits typed/pasted into the verify field. The
                        // hash-input scancode map covers 0-9/a-f (+ a couple of
                        // separators a paste might carry); other keys are ignored.
                        if let Some(c) = scancode_to_hash_input(code, app.shift) {
                            app.verify_push(c);
                            dirty = true;
                        }
                    }
                }
                if dirty {
                    render(&app, &mut canvas);
                    raekit::sys::surface_present(sid, 240, 90);
                }
                continue;
            }
            Overlay::OpenWith => {
                match (ext, code) {
                    (false, 0x01) => {
                        // Esc cancels.
                        app.close_open_with();
                        dirty = true;
                    }
                    (true, 0x48) => {
                        app.open_with_move(-1);
                        dirty = true;
                    } // Up
                    (true, 0x50) => {
                        app.open_with_move(1);
                        dirty = true;
                    } // Down
                    (false, 0x1C) | (false, 0x39) => {
                        // Enter / Space → launch the highlighted candidate.
                        app.open_with_launch_selected();
                        dirty = true;
                    }
                    (false, 0x20) => {
                        // 'd' → set the highlighted candidate as the default + persist.
                        app.open_with_set_default();
                        dirty = true;
                    }
                    _ => {}
                }
                if dirty {
                    render(&app, &mut canvas);
                    raekit::sys::surface_present(sid, 240, 90);
                }
                continue;
            }
            Overlay::Search => {
                match (ext, code) {
                    (false, 0x01) => {
                        // Esc closes the search overlay.
                        app.close_search();
                        dirty = true;
                    }
                    (true, 0x48) => {
                        // Up — move the result highlight.
                        app.search_move(-1);
                        dirty = true;
                    }
                    (true, 0x50) => {
                        // Down — move the result highlight.
                        app.search_move(1);
                        dirty = true;
                    }
                    (false, 0x1C) => {
                        // Enter: query if no results yet, else OPEN the highlighted
                        // row (folder → navigate, file → default-open).
                        if app.search_ran && !app.search_results.is_empty() {
                            app.search_open_selected();
                        } else {
                            app.run_indexed_search();
                        }
                        dirty = true;
                    }
                    (false, 0x0E) => {
                        app.search_backspace();
                        dirty = true;
                    }
                    _ => {
                        if let Some(c) = scancode_to_ascii(code, app.shift) {
                            app.search_push(c);
                            dirty = true;
                        }
                    }
                }
                if dirty {
                    render(&app, &mut canvas);
                    raekit::sys::surface_present(sid, 240, 90);
                }
                continue;
            }
            Overlay::None => {}
        }

        // Clear a stale toast on any keypress (so messages don't linger forever).
        if !app.toast.as_str().is_empty() {
            app.toast = Toast::empty();
        }

        match (ext, code) {
            (true, 0x48) => {
                app.move_sel(-1);
                dirty = true;
            } // Up
            (true, 0x50) => {
                app.move_sel(1);
                dirty = true;
            } // Down
            (true, 0x4B) => {
                app.go_back();
                dirty = true;
            } // Left = Back
            (true, 0x4D) => {
                app.go_forward();
                dirty = true;
            } // Right = Forward
            (true, 0x53) => {
                app.delete_selected_forever();
                dirty = true;
            } // Extended Delete = permanent delete
            (false, 0x1C) => {
                app.enter_selected();
                dirty = true;
            } // Enter
            (false, 0x0E) => {
                app.go_up();
                dirty = true;
            } // Backspace = up
            (false, 0x01) => {
                raekit::sys::exit(0);
            } // Esc
            (false, 0x39) => {
                app.open_quick_look();
                dirty = true;
            } // Space = Quick Look
            (false, 0x32) => {
                app.toggle_mark();
                app.move_sel(1);
                dirty = true;
            } // 'm' = mark + advance
            (false, 0x3C) => {
                app.open_batch_rename();
                dirty = true;
            } // F2 = rename
            (false, 0x14) => {
                if app.shift {
                    app.new_tab();
                } else {
                    app.next_tab();
                }
                dirty = true;
            } // 't' = next tab / Shift+t = new tab
            (false, 0x21) => {
                app.new_folder();
                dirty = true;
            } // 'f' = new folder
            (false, 0x20) => {
                app.trash_selected();
                dirty = true;
            } // 'd' = trash (delete)
            (false, 0x13) => {
                app.restore_selected();
                dirty = true;
            } // 'r' = restore
            (false, 0x2D) => {
                if app.selected_is_archive() {
                    app.extract_selected();
                } else {
                    app.toast.set("Extract: not an archive");
                }
                dirty = true;
            } // 'x' = Extract here (.zip / .tar / .tar.gz / .tgz)
            (false, 0x2C) => {
                app.compress_marked();
                dirty = true;
            } // 'z' = Compress (1 file -> .gz, 2+ -> .zip)
            (false, 0x2E) => {
                app.close_tab();
                dirty = true;
            } // 'c' = close tab
            (false, 0x19) => {
                app.open_compare();
                dirty = true;
            } // 'p' = comPare (diff two marked files)
            (false, 0x23) => {
                app.open_checksum();
                dirty = true;
            } // 'h' = Hash / checksum + verify the selected file
            (false, 0x18) => {
                app.open_open_with();
                dirty = true;
            } // 'o' = Open With… (rae_mime candidate list + Set as default)
            (false, 0x35) => {
                app.open_search();
                dirty = true;
            } // '/' = global indexed search (raekit::search → kernel index)
            // Number keys 1–7 = jump to QUICK_ACCESS slot
            (false, c) if (0x02..=0x08).contains(&c) => {
                let idx = (c - 0x02) as usize;
                if idx < QUICK_ACCESS.len() {
                    app.navigate_quick_access(idx);
                    dirty = true;
                }
            }
            _ => {}
        }

        if dirty {
            render(&app, &mut canvas);
            raekit::sys::surface_present(sid, 240, 90);
        }
    }
}

/// US-QWERTY scancode → a character accepted in the Checksum verify field. A
/// pasted/typed hash is 0-9 and a-f (case-insensitive); the field also tolerates
/// a leading/trailing space (trimmed before compare). Every other key is ignored
/// so a stray non-hex keypress can't corrupt the expected string. Returns `None`
/// for keys we do not accept.
fn scancode_to_hash_input(code: u8, _shift: bool) -> Option<u8> {
    match code {
        // Digits 1-9, 0.
        0x02 => Some(b'1'),
        0x03 => Some(b'2'),
        0x04 => Some(b'3'),
        0x05 => Some(b'4'),
        0x06 => Some(b'5'),
        0x07 => Some(b'6'),
        0x08 => Some(b'7'),
        0x09 => Some(b'8'),
        0x0A => Some(b'9'),
        0x0B => Some(b'0'),
        // Hex letters a-f (lowercase; verify is case-insensitive).
        0x1E => Some(b'a'),
        0x30 => Some(b'b'),
        0x2E => Some(b'c'),
        0x20 => Some(b'd'),
        0x12 => Some(b'e'),
        0x21 => Some(b'f'),
        // Space (trimmed before compare; lets a paste with surrounding spaces land).
        0x39 => Some(b' '),
        _ => None,
    }
}

/// Minimal US-QWERTY scancode → ASCII for the batch-rename pattern field.
/// Only the characters a pattern realistically needs (letters, digits, `_`, `-`,
/// `#`, `.`, space). Returns `None` for keys we do not type.
fn scancode_to_ascii(code: u8, shift: bool) -> Option<u8> {
    let base: u8 = match code {
        0x10 => b'q',
        0x11 => b'w',
        0x12 => b'e',
        0x13 => b'r',
        0x14 => b't',
        0x15 => b'y',
        0x16 => b'u',
        0x17 => b'i',
        0x18 => b'o',
        0x19 => b'p',
        0x1E => b'a',
        0x1F => b's',
        0x20 => b'd',
        0x21 => b'f',
        0x22 => b'g',
        0x23 => b'h',
        0x24 => b'j',
        0x25 => b'k',
        0x26 => b'l',
        0x2C => b'z',
        0x2D => b'x',
        0x2E => b'c',
        0x2F => b'v',
        0x30 => b'b',
        0x31 => b'n',
        0x32 => b'm',
        0x02 => b'1',
        0x03 => b'2',
        0x04 => b'3',
        0x05 => b'4',
        0x06 => b'5',
        0x07 => b'6',
        0x08 => b'7',
        0x09 => b'8',
        0x0A => b'9',
        0x0B => b'0',
        0x39 => b' ',
        0x0C => return Some(if shift { b'_' } else { b'-' }), // '-' / '_'
        0x34 => return Some(b'.'),                            // '.'
        _ => return None,
    };
    if shift {
        // Shift+3 → '#', the counter token; otherwise uppercase letters.
        match code {
            0x04 => Some(b'#'),
            0x10..=0x32 if base.is_ascii_lowercase() => Some(base - 32),
            _ => Some(base),
        }
    } else {
        Some(base)
    }
}

// ── Host KAT: the document-open dispatch (PDF · DOCX · XLSX · image) ──────────
//
// R10-style proof on the syscall-free boundary. Each test builds a TINY in-memory
// document with the matching engine's OWN writer (or, for the PDF, the spec-shaped
// byte layout the engine's reader requires), feeds the raw bytes through the LIVE
// `build_doc_preview` dispatch (the exact code `open_quick_look` calls), and
// asserts the produced preview model carries the expected CONTENT — not merely
// that it is `Ok`. Every assert is FAIL-able: a wrong magic-byte route, a decode
// regression, an empty-text collapse, or a wrong-dimension image all flip a test
// red. `cargo test -p files` is the proof for this slice (no kernel/QEMU).
#[cfg(test)]
mod tests {
    use super::*;
    use alloc::format;
    use alloc::vec;

    // ── PDF fixture (the spec-shaped byte layout rae_pdf::Document::open needs) ──
    // A minimal classic-xref PDF: header, 5 objects (catalog/pages/page/contents/
    // font), an xref table, and a trailer. Mirrors rae_pdf's own test builder.
    fn build_test_pdf(shown: &str) -> Vec<u8> {
        let bodies: [Vec<u8>; 5] = [
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Count 1 /Kids [ 3 0 R ] >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /MediaBox [ 0 0 612 792 ] \
              /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>"
                .to_vec(),
            {
                let content = format!("BT /F1 12 Tf 72 700 Td ({}) Tj ET", shown);
                let mut o = Vec::new();
                o.extend_from_slice(
                    format!("<< /Length {} >>\nstream\n", content.len()).as_bytes(),
                );
                o.extend_from_slice(content.as_bytes());
                o.extend_from_slice(b"\nendstream");
                o
            },
            b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(),
        ];

        let mut out: Vec<u8> = Vec::new();
        out.extend_from_slice(b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n");
        let mut offsets = vec![0usize; bodies.len() + 1];
        for (i, body) in bodies.iter().enumerate() {
            let num = (i + 1) as u32;
            offsets[num as usize] = out.len();
            out.extend_from_slice(format!("{} 0 obj\n", num).as_bytes());
            out.extend_from_slice(body);
            out.extend_from_slice(b"\nendobj\n");
        }
        let xref_off = out.len();
        let count = bodies.len() + 1;
        out.extend_from_slice(format!("xref\n0 {}\n", count).as_bytes());
        out.extend_from_slice(b"0000000000 65535 f \n");
        for num in 1..=bodies.len() {
            out.extend_from_slice(format!("{:010} {:05} n \n", offsets[num], 0).as_bytes());
        }
        out.extend_from_slice(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{}\n%%EOF\n",
                count, xref_off
            )
            .as_bytes(),
        );
        out
    }

    #[test]
    fn open_pdf_renders_extracted_text() {
        let pdf = build_test_pdf("Hello, RaeenOS!");
        // Sanity: magic sniff must route to the PDF engine (content, not name).
        assert_eq!(detect(&pdf), FileKind::Pdf, "PDF magic must sniff to Pdf");

        match build_doc_preview(&pdf) {
            DocPreview::Text { label, text } => {
                assert_eq!(label, "PDF");
                assert!(
                    text.contains("Hello, RaeenOS!"),
                    "extracted PDF text must carry the shown string, got {:?}",
                    text
                );
                // FAIL-ability witness: it must NOT contain text we never wrote.
                assert!(!text.contains("Goodbye, Windows"));
            }
            other => panic!("PDF must produce DocPreview::Text, got {}", other.tag()),
        }
    }

    // ── DOCX fixture (built with rae_docx's own writer) ──────────────────────────
    fn build_test_docx() -> Vec<u8> {
        let doc = rae_docx::Document {
            blocks: vec![
                rae_docx::Block::Paragraph(rae_docx::Paragraph {
                    style: Some("Heading1".into()),
                    runs: vec![rae_docx::Run {
                        text: "Quarterly Report".into(),
                        bold: true,
                        italic: false,
                        underline: false,
                    }],
                }),
                rae_docx::Block::Paragraph(rae_docx::Paragraph {
                    style: None,
                    runs: vec![rae_docx::Run {
                        text: "Revenue grew this quarter.".into(),
                        bold: false,
                        italic: false,
                        underline: false,
                    }],
                }),
            ],
        };
        doc.to_docx().expect("writer must emit a valid .docx")
    }

    #[test]
    fn open_docx_renders_heading_and_paragraph() {
        let docx = build_test_docx();
        // The writer orders `[Content_Types].xml` first, so the byte-sniffer sees a
        // generic ZIP; the dispatch's Zip-fallback probes the DOCX engine (which
        // keys on the central directory). Both orderings must resolve to a DOCX.
        assert!(
            matches!(detect(&docx), FileKind::Docx | FileKind::Zip),
            "DOCX must sniff to Docx or (entry-order) Zip"
        );

        match build_doc_preview(&docx) {
            DocPreview::Text { label, text } => {
                assert_eq!(label, "DOCX");
                assert!(
                    text.contains("Quarterly Report"),
                    "DOCX heading must survive extraction, got {:?}",
                    text
                );
                assert!(
                    text.contains("Revenue grew this quarter."),
                    "DOCX body paragraph must survive extraction, got {:?}",
                    text
                );
            }
            other => panic!("DOCX must produce DocPreview::Text, got {}", other.tag()),
        }
    }

    // ── XLSX fixture (built with rae_xlsx's WorkbookBuilder) ─────────────────────
    fn build_test_xlsx() -> Vec<u8> {
        use rae_xlsx::{Cell, CellValue};
        let cells = vec![
            Cell {
                col: 0,
                row: 0,
                value: CellValue::Text("Name".into()),
                formula: None,
            },
            Cell {
                col: 1,
                row: 0,
                value: CellValue::Text("Score".into()),
                formula: None,
            },
            Cell {
                col: 0,
                row: 1,
                value: CellValue::Text("Ada".into()),
                formula: None,
            },
            Cell {
                col: 1,
                row: 1,
                value: CellValue::Number(42.0),
                formula: None,
            },
        ];
        rae_xlsx::WorkbookBuilder::new()
            .add_sheet("Sheet1", cells)
            .to_xlsx()
            .expect("writer must emit a valid .xlsx")
    }

    #[test]
    fn open_xlsx_renders_grid() {
        let xlsx = build_test_xlsx();
        // Same as DOCX: an entry-order-dependent Zip is resolved by the fallback.
        assert!(
            matches!(detect(&xlsx), FileKind::Xlsx | FileKind::Zip),
            "XLSX must sniff to Xlsx or (entry-order) Zip"
        );

        match build_doc_preview(&xlsx) {
            DocPreview::Csv(csv) => {
                assert_eq!(csv.len(), 2, "two rows expected (header + 1 data)");
                assert_eq!(csv.cell(0, 0), Some("Name"), "A1 must be the header");
                assert_eq!(csv.cell(0, 1), Some("Score"), "B1 must be the header");
                assert_eq!(csv.cell(1, 0), Some("Ada"), "A2 must be the name");
                assert_eq!(csv.cell(1, 1), Some("42"), "B2 must be the score");
            }
            other => panic!("XLSX must produce DocPreview::Csv, got {}", other.tag()),
        }
    }

    // ── PNG fixture (built with rae_image's own encoder) ─────────────────────────
    fn build_test_png(w: u32, h: u32) -> Vec<u8> {
        // A solid opaque-red bitmap: 0xAARRGGBB = 0xFFFF0000.
        let pixels = vec![0xFFFF_0000u32; (w * h) as usize];
        let img = rae_image::Image {
            width: w,
            height: h,
            pixels,
        };
        rae_image::encode(&img, rae_image::ImageFormat::Png).expect("PNG encode must succeed")
    }

    #[test]
    fn open_png_decodes_to_bitmap() {
        let png = build_test_png(4, 3);
        assert_eq!(detect(&png), FileKind::Png, "PNG must sniff to Png");

        match build_doc_preview(&png) {
            DocPreview::Image(img) => {
                assert_eq!(img.width, 4, "decoded width must round-trip");
                assert_eq!(img.height, 3, "decoded height must round-trip");
                assert_eq!(img.pixels.len(), 12, "pixel count = w*h");
                // The source was solid opaque red; a wrong ARGB mapping or a decode
                // regression flips this.
                assert_eq!(img.pixels[0], 0xFFFF_0000, "pixel must be opaque red");
            }
            other => panic!("PNG must produce DocPreview::Image, got {}", other.tag()),
        }
    }

    #[test]
    fn unknown_bytes_fall_through_to_none() {
        // Garbage that sniffs to no known kind → None (caller's text/hex fallback).
        let junk = vec![0x00u8, 0x01, 0x02, 0x03, 0x7F, 0x80, 0xFE];
        match build_doc_preview(&junk) {
            DocPreview::None => {}
            other => panic!("unknown bytes must yield None, got {}", other.tag()),
        }
    }

    #[test]
    fn corrupt_pdf_degrades_to_none_not_panic() {
        // A PDF header but a truncated/garbage body: the engine returns Err and we
        // collapse to None (never panic, never a wall of error text).
        let mut bad = b"%PDF-1.7\n".to_vec();
        bad.extend_from_slice(b"this is not a real pdf body");
        assert_eq!(detect(&bad), FileKind::Pdf);
        assert!(matches!(build_doc_preview(&bad), DocPreview::None));
    }

    #[test]
    fn doc_preview_is_some_reflects_content() {
        // is_some() is the renderable-content predicate the open shell relies on.
        assert!(build_doc_preview(&build_test_pdf("x")).is_some());
        assert!(build_doc_preview(&build_test_xlsx()).is_some());
        assert!(build_doc_preview(&build_test_png(2, 2)).is_some());
        assert!(!DocPreview::None.is_some());
    }

    #[test]
    fn doc_line_count_handles_pages_and_tail() {
        // 2 newlines + 1 form-feed + a trailing partial line = 4 logical lines.
        let buf = b"a\nb\n\x0Cc";
        assert_eq!(doc_line_count(buf), 4);
        assert_eq!(doc_line_count(b""), 0);
        assert_eq!(doc_line_count(b"single"), 1);
    }

    // Tiny debug tag so a wrong-variant panic message is readable without deriving
    // Debug on the (Vec-carrying) DocPreview.
    impl DocPreview {
        fn tag(&self) -> &'static str {
            match self {
                DocPreview::Text { .. } => "Text",
                DocPreview::Csv(_) => "Csv",
                DocPreview::Image(_) => "Image",
                DocPreview::None => "None",
            }
        }
    }

    // ── Windows .exe double-click → RaeBridge launch (the new slice) ──────────
    //
    // The literal Concept promise: double-click a Windows `.exe` in Files → it
    // runs as its own sandboxed process. `exe_launch_plan` is the pure decision
    // core (no syscalls); these KATs prove that activating a `.exe` produces the
    // correct handoff target for THAT path and requests the `raebridge_run`
    // spawn, and that a non-`.exe` does NOT take the launch route (so the doc/
    // image preview path still runs). Each assertion is FAIL-able.

    #[test]
    fn exe_activation_writes_handoff_target_for_that_path() {
        let path = "/bundled/notepad.exe";
        let plan = exe_launch_plan(path).expect("a .exe must produce a launch plan");

        // The launcher spawned is the proven per-process loader, not something else.
        assert_eq!(plan.spawn, "raebridge_run");
        assert_eq!(plan.spawn, RAEBRIDGE_RUN);

        // The handoff record must decode back to a PE target whose path is EXACTLY
        // the activated .exe path — not a fixed sentinel, not a bundled fixture.
        let decoded =
            handoff::decode(&plan.record).expect("the handoff record must decode to a target");
        assert_eq!(
            decoded,
            Target::Pe {
                path: path.as_bytes().to_vec()
            },
            "handoff target must be the exact .exe path"
        );

        // The record is the fixed-width form so an in-place RAM-FS overwrite of a
        // prior, longer launch leaves no stale tail.
        assert_eq!(plan.record.len(), handoff::HANDOFF_RECORD_WIDTH);
    }

    #[test]
    fn exe_activation_is_case_insensitive() {
        // Windows paths arrive in any case; `.EXE` must still launch.
        let plan = exe_launch_plan("/bundled/SETUP.EXE").expect("uppercase .EXE must launch");
        let decoded = handoff::decode(&plan.record).unwrap();
        assert_eq!(
            decoded,
            Target::Pe {
                path: b"/bundled/SETUP.EXE".to_vec()
            }
        );
    }

    #[test]
    fn non_exe_does_not_take_launch_route() {
        // The whole point of intercepting ONLY .exe: every other type must return
        // None here so `open_default_selected` falls through to the preview/open
        // path. If any of these produced a plan, a double-clicked document would be
        // fed to the PE loader instead of being previewed.
        for path in [
            "/home/raeen/notes.txt",
            "/home/raeen/photo.png",
            "/home/raeen/report.pdf",
            "/system/apps/files",    // a native app, no extension
            "/home/raeen/exe",       // bare word, not a .exe
            "/home/raeen/myexe.txt", // .exe is not the suffix
            "/home/raeen/archive.zip",
        ] {
            assert!(
                exe_launch_plan(path).is_none(),
                "non-.exe path {path:?} must NOT route to the RaeBridge launcher"
            );
        }
    }

    #[test]
    fn distinct_exes_produce_distinct_handoff_targets() {
        // Two different .exe paths must yield two different handoff records, so the
        // launcher loads the PE the user actually double-clicked.
        let a = exe_launch_plan("/bundled/a.exe").unwrap();
        let b = exe_launch_plan("/bundled/b.exe").unwrap();
        assert_ne!(a.record, b.record);
        assert_eq!(
            handoff::decode(&a.record).unwrap(),
            Target::Pe {
                path: b"/bundled/a.exe".to_vec()
            }
        );
        assert_eq!(
            handoff::decode(&b.record).unwrap(),
            Target::Pe {
                path: b"/bundled/b.exe".to_vec()
            }
        );
    }
}
