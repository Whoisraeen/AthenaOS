//! AthenaOS Photos — *"show my photos"* (LEGACY_GAMING_CONCEPT.md §creators/media).
//!
//! The daily-driver photo viewer that rivals Windows Photos and macOS Preview on
//! day one: a **thumbnail grid** of a Pictures directory plus a **single-image
//! view** (full decode, aspect-fit, EXIF-oriented) with next/prev navigation and
//! keyboard controls. Still images open through the unified `ath_image` "open any
//! image" dispatcher — the SAME seam the Files app uses — so Photos now decodes
//! **PNG / JPEG / BMP / WebP** (content-sniffed by magic bytes, never the
//! extension) plus the `athmedia::exif` orientation transforms for JPEG. Animated
//! GIFs route to `ath_gif` directly for the multi-frame animation loop.
//!
//! Standalone userspace ELF (`exec_path = "photos"`). Chrome is on the shared
//! `ath_tokens` design language; the live desktop accent comes through
//! `SYS_THEME_GET` (athkit::sys::theme_accent) at launch, so Photos matches the
//! desktop 1:1 (whole-OS cohesion).
//!
//! HOSTILE-INPUT POSTURE: a photo is untrusted data. Every decode goes through
//! the host-KAT'd `ath_image` / `ath_gif` decoders which return `Err` (never
//! panic) on malformed input; a decode failure renders a "can't display"
//! placeholder tile and the app stays alive. The whole image pipeline never
//! panics.
//!
//! PROOF (two layers): (1) `design_proof()` is a fail-able runtime gate the live
//! ELF runs at `_start` (decode → thumbnail downscale → EXIF rotation → exact
//! pixels; exit(3) on drift). (2) The decode dispatch ([`decode_oriented`]) is
//! syscall-free, so the host KAT (`cargo test -p photos --features host`, in the
//! `tests` module below) feeds an in-memory BMP, WebP, and PNG through it and
//! asserts exact dimensions/pixels — proving the NEW formats open in Photos.
//! `ath_image`/`ath_gif`/`athmedia`'s own host KATs prove the decode logic.
//!
//! This is the LIBRARY target; the freestanding `_start` lives in the thin
//! `src/main.rs` bin and just calls [`run`].

// no_std for the real userspace ELF; std under `cargo test` (or the `host`
// feature) so the host KAT can link without athkit's bare-ELF lang items
// colliding with std.
#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::vec::Vec;

#[allow(unused_imports)]
use athkit;

use alloc::string::String;
use ath_tokens::{DARK, RAEBLUE};
use ath_toml::Toml;
use athgfx::text::FontFamily;
use athgfx::Canvas;
use athmedia::exif::{apply_orientation, parse_orientation, Orientation};
// `DecodedImage` is the app's working ARGB8888 image type (the shape the
// downscale/letterbox/blit path consumes). `decode_png` is retained for the
// built-in `design_proof()` fixture decode (the live-ELF runtime gate); the live
// open path now goes through `ath_image::decode` (see `decode_oriented`).
use athmedia::jpeg::DecodedImage;
use athmedia::png::decode_png;
// The unified "open any image" dispatcher (the cohesion seam shared with Files):
// one `decode` call that sniffs the format from MAGIC BYTES (never the
// extension) and routes PNG/JPEG/BMP/GIF/WebP to the matching from-scratch
// decoder, returning the identical ARGB8888 `Vec<u32>` model `DecodedImage`
// already uses. Adopting it makes Photos open BMP + WebP in addition to the
// existing PNG/JPEG, through ONE path. Hostile-input safe (returns `Err`, never
// panics).
use ath_image::{decode as decode_image, FileKind};
// Animated-GIF decode (Concept §creators/media: "show my photos" extends to the
// animated images that fill the web/messaging). `ath_image` collapses a GIF to
// its first frame (the right still/thumbnail behavior), so animated GIFs route
// to `ath_gif::decode_gif` directly to keep ALL frames + per-frame `delay_ms` for
// the single-view animation loop. Each frame is a fully-composited ARGB8888
// `Vec<u32>`, structurally identical to a decoded still, so it reuses the EXACT
// aspect-fit/letterbox/blit path. `decode_gif` is hostile-input safe.
use ath_gif::{decode_gif, GifImage};

// ── Window geometry ─────────────────────────────────────────────────────

const WIN_W: usize = 820;
const WIN_H: usize = 560;
const SURFACE_VIRT: u64 = 0x0000_7B00_0000;

const TITLE_H: usize = 28;
const TOOLBAR_H: usize = 34;
const STATUS_H: usize = 22;

// Thumbnail grid metrics.
const THUMB_W: usize = 150;
const THUMB_H: usize = 120;
const THUMB_PAD: usize = 14;
const LABEL_H: usize = 18;
const CELL_W: usize = THUMB_W + THUMB_PAD;
const CELL_H: usize = THUMB_H + LABEL_H + THUMB_PAD;

/// The on-screen origin we present this window at. Absolute cursor coordinates
/// from `cursor_pos()` are converted to surface-local space by subtracting this.
const PRESENT_X: i32 = 200;
const PRESENT_Y: i32 = 80;

// ── Mouse hit-testing (single source of truth: draw-rects == hit-rects) ───
//
// Grid-thumbnail + single-view nav geometry computed from the SAME constants
// the renderer uses, so a click can never drift from the visual. A click
// dispatches to the EXACT action the matching key fires; empty space resolves
// to `Action::None` (no-op, never panics).

/// What a left-click maps to — each mirrors a keyboard action 1:1.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Action {
    /// Select thumbnail `i` and open it in single-view (Enter on the grid).
    OpenThumb(usize),
    /// Single-view next/prev (Right/Left arrows).
    NavNext,
    NavPrev,
    /// Single-view back to the grid (Esc).
    BackToGrid,
    Close,
    None,
}

#[derive(Clone, Copy)]
struct Rect {
    x: usize,
    y: usize,
    w: usize,
    h: usize,
}

impl Rect {
    fn contains(&self, px: i32, py: i32) -> bool {
        px >= self.x as i32
            && px < (self.x + self.w) as i32
            && py >= self.y as i32
            && py < (self.y + self.h) as i32
    }
}

/// The window-close (X) rect in the title bar.
fn close_rect() -> Rect {
    Rect {
        x: WIN_W - 28,
        y: 4,
        w: 20,
        h: 20,
    }
}

// Single-view nav-affordance metrics — shared by draw + hit.
const NAV_W: usize = 48;
const NAV_BACK_W: usize = 64;
const NAV_BACK_H: usize = 28;

/// The single-view "previous" hot-zone (left edge of the image area).
fn single_prev_rect() -> Rect {
    let area_y = TITLE_H + TOOLBAR_H;
    let area_h = WIN_H - area_y - STATUS_H;
    Rect {
        x: 0,
        y: area_y,
        w: NAV_W,
        h: area_h,
    }
}

/// The single-view "next" hot-zone (right edge of the image area).
fn single_next_rect() -> Rect {
    let area_y = TITLE_H + TOOLBAR_H;
    let area_h = WIN_H - area_y - STATUS_H;
    Rect {
        x: WIN_W - NAV_W,
        y: area_y,
        w: NAV_W,
        h: area_h,
    }
}

/// The single-view "back to grid" button (top-left of the image area).
fn single_back_rect() -> Rect {
    let area_y = TITLE_H + TOOLBAR_H;
    Rect {
        x: 12,
        y: area_y + 10,
        w: NAV_BACK_W,
        h: NAV_BACK_H,
    }
}

// ── Palette (ath_tokens, docs/design/design-language.md) ──────────────────
//
// Generic chrome pulls onto the shared `ath_tokens::DARK` palette + the RaeBlue
// accent ramp so Photos matches the desktop default 1:1. The accent ramp is
// derived (not const), so accent fills are computed in helpers below.

const BG: u32 = DARK.bg_raised; // window client
const TITLE_BG: u32 = DARK.bg_base; // deepest chrome
const TOOLBAR_BG: u32 = DARK.bg_overlay; // panel
const GRID_BG: u32 = DARK.bg_raised; // thumbnail surface
const TILE_BG: u32 = DARK.bg_base; // thumbnail letterbox
const TEXT_FG: u32 = DARK.text_primary;
const TEXT_MUTED: u32 = DARK.text_secondary;
const STATUS_BG: u32 = DARK.bg_base;
const STROKE_HL: u32 = DARK.stroke_strong; // glass top-edge / tile border
const VIEW_SCRIM: u32 = 0xEE_06_07_0C; // single-image view dimming

fn theme_seed() -> u32 {
    athkit::sys::theme_accent()
}

/// Accent base, derived through the shared ramp from the live theme seed.
fn accent() -> u32 {
    ath_tokens::derive_accent(theme_seed(), &DARK).base
}

/// Opaque selection fill: the accent's pressed/active shade.
fn sel_fill() -> u32 {
    ath_tokens::derive_accent(theme_seed(), &DARK).active
}

// ── Image format sniffing ────────────────────────────────────────────────

/// The 8-byte PNG signature (used by the built-in `build_test_png` proof fixture
/// encoder; the live open path sniffs via `ath_image::detect`).
const PNG_SIG: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

/// Whether `bytes` is an (animated) GIF — the one format Photos routes off the
/// unified `ath_image` still path to `ath_gif` directly, so the single-view can
/// loop all frames. Sniffed by MAGIC BYTES (via `ath_image::detect`, the same
/// content-not-extension sniffer the dispatcher uses), never the file name.
fn is_gif(buf: &[u8]) -> bool {
    ath_image::detect(buf) == FileKind::Gif
}

/// Whether `bytes` is a JPEG — used to gate EXIF-orientation handling (only JPEG
/// carries the orientation tag Photos honors). Content-sniffed via `ath_image`.
fn is_jpeg(buf: &[u8]) -> bool {
    ath_image::detect(buf) == FileKind::Jpeg
}

/// Re-home a unified [`ath_image::Image`] into the app's [`DecodedImage`]. Both
/// are the identical ARGB8888 `Vec<u32>` model, so this is a field move, never a
/// pixel convert.
fn image_to_decoded(img: ath_image::Image) -> DecodedImage {
    DecodedImage {
        width: img.width,
        height: img.height,
        pixels: img.pixels,
    }
}

/// Decode a whole image buffer to ARGB8888, auto-applying EXIF orientation for
/// JPEGs. Returns `None` on any unsupported/corrupt input (never panics).
///
/// Still images (PNG/JPEG/BMP/WebP — and a GIF's first frame) go through the
/// unified [`ath_image::decode`] dispatcher (one path, magic-byte routing); only
/// JPEG then gets the EXIF-orientation transform applied (`ath_image` surfaces no
/// EXIF, so orientation is parsed from the original bytes and applied to the
/// decoded buffer exactly as before). Animated GIFs keep all frames via
/// [`App::open_single`]'s direct `ath_gif` path — here a GIF still resolves to its
/// first frame (the thumbnail/Quick-Look representation), which `ath_image`
/// already returns.
fn decode_oriented(bytes: &[u8]) -> Option<DecodedImage> {
    let img = image_to_decoded(decode_image(bytes).ok()?);
    // EXIF orientation is JPEG-only; apply it to the decoded buffer (ath_image
    // does not rotate, so this preserves the prior behavior precisely).
    if is_jpeg(bytes) {
        let o = parse_orientation(bytes);
        if o != Orientation::Normal {
            return Some(apply_orientation(&img, o));
        }
    }
    Some(img)
}

/// Extract frame `idx` of a decoded GIF as a `DecodedImage` (the structurally
/// identical ARGB shape the PNG/JPEG blit path consumes). `None` if the frame
/// index is out of range. Each GIF frame is already composited full-canvas, so
/// this is a field move + clone — no per-frame compositing here.
fn gif_frame_image(gif: &GifImage, idx: usize) -> Option<DecodedImage> {
    let frame = gif.frames.get(idx)?;
    Some(DecodedImage {
        width: gif.width,
        height: gif.height,
        pixels: frame.pixels.clone(),
    })
}

// ── Pictures directory enumeration ────────────────────────────────────────

const PATH_CAP: usize = 256;
const NAME_CAP: usize = 64;
const MAX_PHOTOS: usize = 64;

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
}

/// One photo entry: a file name + a lazily-decoded thumbnail. The thumbnail is a
/// small ARGB buffer (THUMB_W x THUMB_H, fit + letterboxed) so the whole grid is
/// resident without holding full-resolution decodes. `decoded_ok=false` means the
/// file is present but not a displayable image → placeholder tile.
struct Photo {
    name: [u8; NAME_CAP],
    name_len: usize,
    thumb: Option<DecodedImage>, // THUMB_W x THUMB_H ARGB, letterboxed
    decoded_ok: bool,
    attempted: bool, // thumbnail decode attempted (lazy)
}

impl Photo {
    fn name(&self) -> &str {
        core::str::from_utf8(&self.name[..self.name_len]).unwrap_or("?")
    }
}

/// True if a file name ends with a known image extension (case-insensitive).
/// The decoder sniffs the bytes regardless; this is only the directory filter so
/// the grid doesn't list .txt files as broken tiles.
fn is_image_name(name: &str) -> bool {
    let lower_ends = |suf: &str| -> bool {
        let nb = name.as_bytes();
        let sb = suf.as_bytes();
        if nb.len() < sb.len() {
            return false;
        }
        let tail = &nb[nb.len() - sb.len()..];
        tail.iter()
            .zip(sb.iter())
            .all(|(a, b)| a.to_ascii_lowercase() == *b)
    };
    lower_ends(".png") || lower_ends(".jpg") || lower_ends(".jpeg") || lower_ends(".jpe")
}

// ── App state ─────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
enum View {
    Grid,
    Single,
}

impl View {
    /// Stable token persisted in the prefs file.
    fn as_token(self) -> &'static str {
        match self {
            View::Grid => "grid",
            View::Single => "single",
        }
    }
    /// Parse the persisted token; unknown / missing → the typed default (`Grid`).
    fn from_token(s: &str) -> Self {
        match s {
            "single" => View::Single,
            _ => View::Grid,
        }
    }
}

/// Hard cap on a single full-resolution slurp (matches the decoders' own
/// MAX_PIXELS posture — no unbounded allocation from one open).
const DECODE_CAP: usize = 24 * 1024 * 1024; // 24 MiB

// ── Persistent preferences (ath_toml) ─────────────────────────────────────────
//
// LEGACY_GAMING_CONCEPT.md §"The user owns the machine": "remember my settings" must be
// real. Photos persists its last library FOLDER, the view MODE (grid vs single),
// and the currently-selected photo FILE NAME to `<home>/.config/photos.toml`,
// restoring them on launch. (There is no sort/zoom control in this build, so the
// selected item is the meaningful "where was I" state.) Every load is hostile-
// input-tolerant: a missing, corrupt, or out-of-range config falls back to TYPED
// DEFAULTS and NEVER panics — the app always starts. This is the per-app prefs
// pattern the consumer apps follow (the proven Music recipe).

/// The decoded, defaulted preferences restored at launch. Pure data: load builds
/// it from a parsed (or absent) TOML, save serializes the live App state.
#[derive(Clone)]
struct Prefs {
    /// Last scanned library folder (absolute path). Empty = the default
    /// `<home>/Pictures`.
    last_folder: String,
    /// View mode: false = Grid (the default), true = Single.
    single: bool,
    /// Last-selected photo FILE NAME (re-resolved against the live scan; a renamed
    /// / deleted file simply fails to match → selection 0). Empty = none.
    last_photo: String,
}

impl Prefs {
    /// The typed defaults used on first run or any config error.
    fn defaults() -> Self {
        Self {
            last_folder: String::new(),
            single: false,
            last_photo: String::new(),
        }
    }

    /// Build `Prefs` from a parsed TOML table, validating every field and
    /// substituting the typed default for any missing / wrong-typed value. Never
    /// panics; an unrelated shape (e.g. a non-table root) yields full defaults.
    fn from_toml(t: &Toml) -> Self {
        let mut p = Self::defaults();
        if let Some(s) = t.get("last_folder").and_then(Toml::as_str) {
            p.last_folder = String::from(truncate_on_char_boundary(s, PATH_CAP));
        }
        if let Some(s) = t.get("view").and_then(Toml::as_str) {
            p.single = View::from_token(s) == View::Single;
        }
        if let Some(s) = t.get("last_photo").and_then(Toml::as_str) {
            p.last_photo = String::from(truncate_on_char_boundary(s, PATH_CAP));
        }
        p
    }

    /// Serialize the live preferences into an order-stable `Toml::Table` ready for
    /// `ath_toml::to_string`. The schema is flat (no headers).
    fn to_toml(&self) -> Toml {
        let mut table: Vec<(String, Toml)> = Vec::new();
        table.push((
            String::from("last_folder"),
            Toml::String(self.last_folder.clone()),
        ));
        let view = if self.single {
            View::Single
        } else {
            View::Grid
        };
        table.push((
            String::from("view"),
            Toml::String(String::from(view.as_token())),
        ));
        table.push((
            String::from("last_photo"),
            Toml::String(self.last_photo.clone()),
        ));
        Toml::Table(table)
    }
}

/// Return a prefix of `s` no longer than `max` bytes, cut on a UTF-8 char
/// boundary so the result is always valid (never panics on a multi-byte split).
fn truncate_on_char_boundary(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// The per-app config DIRECTORY: `<session home>/.config`. Falls back to the same
/// `/home/user` default the Pictures directory uses when no session is present.
/// Created (idempotent) before any write.
fn prefs_dir() -> PathBuf {
    let mut p = PathBuf::new();
    let mut info = [0u8; 96];
    if athkit::sys::session_info(&mut info).is_some() {
        if let Some(home) = athkit::sys::session_home_from(&info) {
            p.set(home);
            p.push_component(".config");
            return p;
        }
    }
    p.set("/home/user/.config");
    p
}

/// Load preferences from `<home>/.config/photos.toml`. On ANY failure — file
/// absent, unreadable, not UTF-8, or a `ath_toml::parse` error — returns the typed
/// defaults. Never panics, never blocks the app from launching.
fn load_prefs() -> Prefs {
    let mut path = prefs_dir();
    path.push_component("photos.toml");
    let fd = athkit::sys::open(path.as_str(), 0);
    if fd == u64::MAX {
        return Prefs::defaults();
    }
    let mut data: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        // Hard cap: a config file should be tiny; refuse to slurp a giant blob.
        if data.len() > 64 * 1024 {
            break;
        }
        let n = athkit::sys::read(fd, &mut chunk) as usize;
        if n == 0 || n > chunk.len() {
            break;
        }
        data.extend_from_slice(&chunk[..n]);
    }
    let _ = athkit::sys::close(fd);
    let text = match core::str::from_utf8(&data) {
        Ok(s) => s,
        Err(_) => return Prefs::defaults(),
    };
    match ath_toml::parse(text) {
        Ok(t) => Prefs::from_toml(&t),
        Err(_) => Prefs::defaults(),
    }
}

/// Persist `prefs` to `<home>/.config/photos.toml` (best effort). Creates the
/// `.config` directory if missing, serializes via `ath_toml::to_string`, and
/// writes O_CREAT|O_TRUNC. A failure is silent — the app keeps running.
fn save_prefs(prefs: &Prefs) {
    let dir = prefs_dir();
    let _ = athkit::sys::mkdir(dir.as_str());
    let mut path = dir;
    path.push_component("photos.toml");
    let text = ath_toml::to_string(&prefs.to_toml());
    // O_WRONLY | O_CREAT | O_TRUNC = 0x0241.
    let fd = athkit::sys::open(path.as_str(), 0x0241);
    if fd == u64::MAX {
        return;
    }
    let bytes = text.as_bytes();
    let mut off = 0usize;
    while off < bytes.len() {
        let end = (off + 4096).min(bytes.len());
        let n = athkit::sys::write(fd, &bytes[off..end]) as usize;
        if n == 0 {
            break;
        }
        off += n;
    }
    let _ = athkit::sys::close(fd);
}

struct App {
    dir: PathBuf,
    photos: Vec<Photo>,
    selected: usize,
    scroll_row: usize,
    view: View,
    // Full-resolution decode of the current single-view photo (EXIF-oriented).
    // For a still image (PNG/JPEG, or a single-frame GIF) this holds the only
    // frame; for an animated GIF it mirrors the current animation frame so the
    // existing still-render path stays identical.
    full: Option<DecodedImage>,
    // The animated GIF currently open in single-view (all frames), or `None` for
    // a still image. When set, the event loop advances `current_frame` by each
    // frame's `delay_ms` and refreshes `full` from the new frame.
    anim: Option<GifImage>,
    current_frame: usize,
    last_advance_ns: u64,
    toast: [u8; 64],
    toast_len: usize,
}

/// Floor on a GIF frame delay (ms). A 0 or tiny delay (common in real GIFs) would
/// otherwise spin the CPU repainting every loop tick; clamp it to a sane cadence.
const GIF_MIN_DELAY_MS: u64 = 100;

impl App {
    fn pictures_dir() -> PathBuf {
        // Prefer <session home>/Pictures; fall back to a system bucket.
        let mut info = [0u8; 96];
        if athkit::sys::session_info(&mut info).is_some() {
            if let Some(home) = athkit::sys::session_home_from(&info) {
                let mut p = PathBuf::new();
                p.set(home);
                p.push_component("Pictures");
                return p;
            }
        }
        let mut p = PathBuf::new();
        p.set("/home/user/Pictures");
        p
    }

    fn new() -> Self {
        // Restore saved preferences (typed defaults on first run / any error).
        let prefs = load_prefs();
        // Last folder wins if persisted and non-empty, else the default
        // `<home>/Pictures`. A stale/deleted folder simply scans to zero photos.
        let dir = if prefs.last_folder.is_empty() {
            Self::pictures_dir()
        } else {
            let mut p = PathBuf::new();
            p.set(&prefs.last_folder);
            p
        };
        // Best-effort: ensure the bucket exists (idempotent).
        let _ = athkit::sys::mkdir(dir.as_str());
        let mut app = Self {
            dir,
            photos: Vec::new(),
            selected: 0,
            scroll_row: 0,
            view: View::Grid,
            full: None,
            anim: None,
            current_frame: 0,
            last_advance_ns: 0,
            toast: [0; 64],
            toast_len: 0,
        };
        app.scan();
        // Re-resolve the last-selected photo against the freshly scanned list.
        if !prefs.last_photo.is_empty() {
            for (i, p) in app.photos.iter().enumerate() {
                if p.name() == prefs.last_photo.as_str() {
                    app.selected = i;
                    break;
                }
            }
            // Keep the restored selection scrolled into view.
            let sel_row = app.selected / app.cols();
            let vis = app.visible_rows();
            if sel_row >= app.scroll_row + vis {
                app.scroll_row = sel_row + 1 - vis;
            }
        }
        // Restore the single-image view if that's where the user left off and the
        // selection resolved to a real photo (decodes the current image).
        if prefs.single && !app.photos.is_empty() {
            app.open_single();
        }
        app
    }

    /// Snapshot the live persistable state into a `Prefs` and write it to disk.
    /// Called on every preference-affecting change (folder/selection/view). Best
    /// effort + silent on failure (the app never blocks on the config write).
    fn persist(&self) {
        let last_photo = if self.selected < self.photos.len() {
            String::from(self.photos[self.selected].name())
        } else {
            String::new()
        };
        let prefs = Prefs {
            last_folder: String::from(self.dir.as_str()),
            single: self.view == View::Single,
            last_photo,
        };
        save_prefs(&prefs);
    }

    fn set_toast(&mut self, s: &str) {
        let n = s.as_bytes().len().min(64);
        self.toast[..n].copy_from_slice(&s.as_bytes()[..n]);
        self.toast_len = n;
    }

    fn toast_str(&self) -> &str {
        core::str::from_utf8(&self.toast[..self.toast_len]).unwrap_or("")
    }

    /// Enumerate the Pictures directory and build the (un-decoded) photo list.
    fn scan(&mut self) {
        self.photos.clear();
        let mut buf = [0u8; 4096];
        // Copy the dir path locally to avoid a borrow conflict with the read.
        let mut dirbuf = [0u8; PATH_CAP];
        let dn = self.dir.as_str().as_bytes().len().min(PATH_CAP);
        dirbuf[..dn].copy_from_slice(&self.dir.as_str().as_bytes()[..dn]);
        let dir = core::str::from_utf8(&dirbuf[..dn]).unwrap_or("/");

        let count = athkit::sys::readdir_at(dir, &mut buf) as usize;
        let mut off = 0usize;
        for _ in 0..count {
            if off + 6 > buf.len() || self.photos.len() >= MAX_PHOTOS {
                break;
            }
            let name_len = u16::from_le_bytes([buf[off], buf[off + 1]]) as usize;
            let _size =
                u32::from_le_bytes([buf[off + 2], buf[off + 3], buf[off + 4], buf[off + 5]]);
            off += 6;
            if off + name_len > buf.len() {
                break;
            }
            let raw = &buf[off..off + name_len];
            off += name_len;
            let name = core::str::from_utf8(raw).unwrap_or("");
            if !is_image_name(name) {
                continue;
            }
            let mut nbuf = [0u8; NAME_CAP];
            let n = raw.len().min(NAME_CAP);
            nbuf[..n].copy_from_slice(&raw[..n]);
            self.photos.push(Photo {
                name: nbuf,
                name_len: n,
                thumb: None,
                decoded_ok: false,
                attempted: false,
            });
        }
        if self.selected >= self.photos.len() {
            self.selected = self.photos.len().saturating_sub(1);
        }
    }

    /// Read a whole file under the Pictures dir into a heap buffer (capped).
    fn read_photo_bytes(&self, idx: usize) -> Option<Vec<u8>> {
        let p = self.photos.get(idx)?;
        let mut path = PathBuf::new();
        path.set(self.dir.as_str());
        path.push_component(p.name());
        let fd = athkit::sys::open(path.as_str(), 0);
        if fd == u64::MAX {
            return None;
        }
        let mut data: Vec<u8> = Vec::new();
        let mut chunk = [0u8; 4096];
        loop {
            if data.len() > DECODE_CAP {
                let _ = athkit::sys::close(fd);
                return None;
            }
            let n = athkit::sys::read(fd, &mut chunk) as usize;
            if n == 0 || n > chunk.len() {
                break;
            }
            data.extend_from_slice(&chunk[..n]);
        }
        let _ = athkit::sys::close(fd);
        if data.is_empty() {
            None
        } else {
            Some(data)
        }
    }

    /// Lazily build the thumbnail for one photo (decode → downscale to THUMB).
    /// Records `decoded_ok` so the grid shows a placeholder for non-images. Never
    /// panics: a decode failure leaves `thumb=None, decoded_ok=false`.
    fn ensure_thumb(&mut self, idx: usize) {
        let already = self.photos.get(idx).map(|p| p.attempted).unwrap_or(true);
        if already {
            return;
        }
        let bytes = self.read_photo_bytes(idx);
        let thumb = bytes
            .as_ref()
            .and_then(|b| decode_oriented(b))
            .map(|img| downscale_fit(&img, THUMB_W, THUMB_H));
        if let Some(p) = self.photos.get_mut(idx) {
            p.attempted = true;
            p.decoded_ok = thumb.is_some();
            p.thumb = thumb;
        }
    }

    /// Decode the selected photo at full resolution (EXIF-oriented) for the
    /// single-image view.
    fn open_single(&mut self) {
        if self.photos.is_empty() {
            return;
        }
        let idx = self.selected;
        // Reset any prior animation before decoding the new selection.
        self.anim = None;
        self.current_frame = 0;
        self.last_advance_ns = athkit::sys::time_ns();
        let bytes = self.read_photo_bytes(idx);

        // An animated GIF keeps all its frames so the single-view can loop them;
        // every other format (and a 1-frame GIF) decodes to a single still image.
        if let Some(b) = bytes.as_ref() {
            if is_gif(b) {
                if let Ok(gif) = decode_gif(b) {
                    self.full = gif_frame_image(&gif, 0);
                    // Only retain the animation when there is more than one frame
                    // — a single-frame GIF is just a still (no timer churn).
                    if gif.frames.len() > 1 {
                        self.anim = Some(gif);
                    }
                }
            }
        }
        // Non-GIF (or a GIF whose decode failed above leaves `full` unchanged):
        // fall back to the still decode path.
        if self.full.is_none() {
            self.full = bytes.as_ref().and_then(|b| decode_oriented(b));
        }

        if self.full.is_none() {
            self.set_toast("Can't display this image");
        } else {
            self.toast_len = 0;
        }
        self.view = View::Single;
        // Remember the open photo + single-view across launches.
        self.persist();
    }

    /// Advance the GIF animation if enough wall-clock time has elapsed for the
    /// current frame's delay. Returns `true` if the frame changed (caller
    /// re-renders). A no-op for still images. Polled each event-loop tick — it
    /// never blocks; it only compares `time_ns()` against the per-frame delay.
    fn tick_animation(&mut self) -> bool {
        let gif = match self.anim.as_ref() {
            Some(g) => g,
            None => return false,
        };
        let n = gif.frames.len();
        if n <= 1 {
            return false;
        }
        let delay_ms = (gif.frames[self.current_frame].delay_ms as u64).max(GIF_MIN_DELAY_MS);
        let now = athkit::sys::time_ns();
        if now.saturating_sub(self.last_advance_ns) < delay_ms * 1_000_000 {
            return false;
        }
        self.current_frame = (self.current_frame + 1) % n;
        self.last_advance_ns = now;
        // Mirror the new frame into `full` so the existing still-render path draws it.
        self.full = gif_frame_image(gif, self.current_frame);
        true
    }

    fn close_single(&mut self) {
        self.full = None;
        self.anim = None;
        self.current_frame = 0;
        self.view = View::Grid;
        // Remember the return to grid view across launches.
        self.persist();
    }

    fn cols(&self) -> usize {
        let grid_w = WIN_W;
        (grid_w / CELL_W).max(1)
    }

    fn visible_rows(&self) -> usize {
        let grid_h = WIN_H - TITLE_H - TOOLBAR_H - STATUS_H;
        (grid_h / CELL_H).max(1)
    }

    fn move_sel(&mut self, dcol: i32, drow: i32) {
        if self.photos.is_empty() {
            return;
        }
        let cols = self.cols() as i32;
        let n = self.photos.len() as i32;
        let cur = self.selected as i32;
        let mut row = cur / cols;
        let mut col = cur % cols;
        col += dcol;
        row += drow;
        if col < 0 {
            col = 0;
        }
        if col >= cols {
            col = cols - 1;
        }
        if row < 0 {
            row = 0;
        }
        let mut idx = row * cols + col;
        if idx >= n {
            idx = n - 1;
        }
        if idx < 0 {
            idx = 0;
        }
        self.selected = idx as usize;
        // Keep selection in the scrolled viewport.
        let sel_row = self.selected / self.cols();
        let vis = self.visible_rows();
        if sel_row < self.scroll_row {
            self.scroll_row = sel_row;
        }
        if sel_row >= self.scroll_row + vis {
            self.scroll_row = sel_row + 1 - vis;
        }
        // Remember the highlighted photo across launches.
        self.persist();
    }

    /// Single-view next/prev: re-decode the new selection at full res.
    fn nav_single(&mut self, delta: i32) {
        if self.photos.is_empty() {
            return;
        }
        let n = self.photos.len() as i32;
        let mut idx = self.selected as i32 + delta;
        if idx < 0 {
            idx = n - 1;
        }
        if idx >= n {
            idx = 0;
        }
        self.selected = idx as usize;
        self.open_single();
    }

    /// The surface-local rect of visible grid thumbnail `i`, or `None` if `i` is
    /// not currently scrolled into view. Uses the SAME metrics `render_grid` draws
    /// (tile = THUMB_W wide, THUMB_H + LABEL_H tall, so the label is clickable too).
    fn thumb_rect(&self, i: usize) -> Option<Rect> {
        let cols = self.cols();
        let vis_rows = self.visible_rows();
        let start = self.scroll_row * cols;
        let end = (start + vis_rows * cols).min(self.photos.len());
        if i < start || i >= end {
            return None;
        }
        let grid_y = TITLE_H + TOOLBAR_H;
        let rel = i - start;
        let col = rel % cols;
        let row = rel / cols;
        Some(Rect {
            x: THUMB_PAD / 2 + col * CELL_W,
            y: grid_y + THUMB_PAD / 2 + row * CELL_H,
            w: THUMB_W,
            h: THUMB_H + LABEL_H,
        })
    }

    /// Hit-test a surface-local click. In Grid view: close button, then a
    /// thumbnail. In Single view: close button, back button, then the prev/next
    /// edge zones. Returns `Action::None` for empty space. Pure: builds the SAME
    /// rects the renderer draws.
    fn hit(&self, px: i32, py: i32) -> Action {
        if close_rect().contains(px, py) {
            return Action::Close;
        }
        match self.view {
            View::Grid => {
                for i in 0..self.photos.len() {
                    if let Some(r) = self.thumb_rect(i) {
                        if r.contains(px, py) {
                            return Action::OpenThumb(i);
                        }
                    }
                }
                Action::None
            }
            View::Single => {
                if single_back_rect().contains(px, py) {
                    return Action::BackToGrid;
                }
                if single_prev_rect().contains(px, py) {
                    return Action::NavPrev;
                }
                if single_next_rect().contains(px, py) {
                    return Action::NavNext;
                }
                Action::None
            }
        }
    }

    /// Apply an `Action` (shared by click dispatch + the hit-test proof). Returns
    /// true if anything changed (caller re-renders). `Close` exits. Each branch
    /// mirrors the matching key exactly.
    fn dispatch(&mut self, action: Action) -> bool {
        match action {
            Action::OpenThumb(i) => {
                if i >= self.photos.len() {
                    return false;
                }
                self.selected = i;
                self.open_single();
                true
            }
            Action::NavNext => {
                self.nav_single(1);
                true
            }
            Action::NavPrev => {
                self.nav_single(-1);
                true
            }
            Action::BackToGrid => {
                self.close_single();
                true
            }
            Action::Close => athkit::sys::exit(0),
            Action::None => false,
        }
    }
}

// ── Thumbnail downscale (the proven fit/blit math from Files Quick Look) ──────

/// Source-over alpha-composite an ARGB8888 pixel onto an opaque ARGB8888
/// background. Integer math (no float). Identical to the Files Quick Look blend.
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

/// Produce a `(tw x th)` ARGB image that contains `img` scaled-to-fit and
/// letterboxed on the tile background (aspect ratio preserved). Nearest-neighbor
/// integer sampling, allocation = one output buffer. Used for the grid thumbs so
/// the full-res decode can be dropped immediately. Never panics.
fn downscale_fit(img: &DecodedImage, tw: usize, th: usize) -> DecodedImage {
    let mut out = alloc::vec![TILE_BG | 0xFF00_0000; tw * th];
    let iw = img.width as usize;
    let ih = img.height as usize;
    if iw == 0 || ih == 0 || img.pixels.len() != iw * ih {
        return DecodedImage {
            width: tw as u32,
            height: th as u32,
            pixels: out,
        };
    }
    // Fit via cross-multiplication (no precision loss before comparing).
    let (dw, dh) = if iw * th <= tw * ih {
        (((iw * th) / ih).max(1), th)
    } else {
        (tw, ((ih * tw) / iw).max(1))
    };
    let ox = (tw - dw.min(tw)) / 2;
    let oy = (th - dh.min(th)) / 2;
    for dy in 0..dh.min(th) {
        let sy = ((dy * ih) / dh).min(ih - 1);
        let row_base = sy * iw;
        for dx in 0..dw.min(tw) {
            let sx = ((dx * iw) / dw).min(iw - 1);
            let src = img.pixels[row_base + sx];
            let o = (oy + dy) * tw + (ox + dx);
            out[o] = over(src, TILE_BG);
        }
    }
    DecodedImage {
        width: tw as u32,
        height: th as u32,
        pixels: out,
    }
}

/// Blit a pre-rendered ARGB image 1:1 at `(x, y)` onto the canvas.
fn blit_argb(canvas: &mut Canvas, img: &DecodedImage, x: usize, y: usize) {
    let w = img.width as usize;
    let h = img.height as usize;
    if img.pixels.len() != w * h {
        return;
    }
    for dy in 0..h {
        let base = dy * w;
        for dx in 0..w {
            canvas.draw_pixel(x + dx, y + dy, img.pixels[base + dx]);
        }
    }
}

/// Blit a decoded image scale-to-fit into the `(fx, fy, fw, fh)` frame,
/// letterboxed on the tile background. The single-image view path. Never panics.
fn blit_image_fit(
    canvas: &mut Canvas,
    img: &DecodedImage,
    fx: usize,
    fy: usize,
    fw: usize,
    fh: usize,
) {
    canvas.fill_rect(fx, fy, fw, fh, TILE_BG);
    let iw = img.width as usize;
    let ih = img.height as usize;
    if iw == 0 || ih == 0 || fw == 0 || fh == 0 || img.pixels.len() != iw * ih {
        return;
    }
    let (dw, dh) = if iw * fh <= fw * ih {
        (((iw * fh) / ih).max(1), fh)
    } else {
        (fw, ((ih * fw) / iw).max(1))
    };
    let ox = fx + (fw - dw.min(fw)) / 2;
    let oy = fy + (fh - dh.min(fh)) / 2;
    for dy in 0..dh.min(fh) {
        let sy = ((dy * ih) / dh).min(ih - 1);
        let row_base = sy * iw;
        for dx in 0..dw.min(fw) {
            let sx = ((dx * iw) / dw).min(iw - 1);
            let src = img.pixels[row_base + sx];
            canvas.draw_pixel(ox + dx, oy + dy, over(src, TILE_BG));
        }
    }
    let frame_w = dw.min(fw);
    let frame_h = dh.min(fh);
    canvas.draw_rect_outline(ox, oy, frame_w, frame_h, STROKE_HL);
    canvas.fill_rect(ox, oy, frame_w, 1, accent());
}

// ── Rendering ─────────────────────────────────────────────────────────────

fn render(app: &mut App, canvas: &mut Canvas) {
    canvas.fill_rect(0, 0, WIN_W, WIN_H, BG);

    // Title bar
    canvas.fill_rect_gradient(0, 0, WIN_W, TITLE_H, DARK.bg_elevated, TITLE_BG);
    canvas.draw_text_aa(
        12,
        ((TITLE_H.saturating_sub(ath_tokens::TYPE_SUBTITLE.line_height as usize)) / 2) as i32,
        "Photos",
        ath_tokens::TYPE_SUBTITLE,
        TEXT_FG,
        FontFamily::Sans,
    );
    canvas.fill_rounded_rect(
        WIN_W - 28,
        4,
        20,
        20,
        ath_tokens::RADIUS_XS as usize,
        DARK.state_danger,
    );
    let x_w = canvas.measure_text_aa("X", ath_tokens::TYPE_LABEL, FontFamily::Sans);
    canvas.draw_text_aa(
        (WIN_W - 18) as i32 - x_w / 2,
        (4 + (20 - ath_tokens::TYPE_LABEL.line_height as usize) / 2) as i32,
        "X",
        ath_tokens::TYPE_LABEL,
        0xFF_FF_FF_FF,
        FontFamily::Sans,
    );

    // Toolbar (path + count)
    let tb_y = TITLE_H;
    canvas.fill_rect(0, tb_y, WIN_W, TOOLBAR_H, TOOLBAR_BG);
    let tb_ty = (tb_y
        + (TOOLBAR_H.saturating_sub(ath_tokens::TYPE_CAPTION.line_height as usize)) / 2)
        as i32;
    let lbl_w = canvas.draw_text_aa(
        12,
        tb_ty,
        "Library:",
        ath_tokens::TYPE_CAPTION,
        TEXT_MUTED,
        FontFamily::Sans,
    );
    canvas.draw_text_aa(
        lbl_w + 18,
        tb_ty,
        app.dir.as_str(),
        ath_tokens::TYPE_CAPTION,
        accent(),
        FontFamily::Sans,
    );

    match app.view {
        View::Grid => render_grid(app, canvas),
        View::Single => render_single(app, canvas),
    }

    // Status bar
    let st_y = WIN_H - STATUS_H;
    canvas.fill_rect(0, st_y, WIN_W, STATUS_H, STATUS_BG);
    let st_ty = (st_y
        + (STATUS_H.saturating_sub(ath_tokens::TYPE_CAPTION.line_height as usize)) / 2)
        as i32;
    if !app.toast_str().is_empty() {
        canvas.draw_text_aa(
            12,
            st_ty,
            app.toast_str(),
            ath_tokens::TYPE_CAPTION,
            DARK.state_danger,
            FontFamily::Sans,
        );
    } else {
        let mut buf = [0u8; 48];
        let n = fmt_count(app.photos.len(), &mut buf);
        if let Ok(s) = core::str::from_utf8(&buf[..n]) {
            canvas.draw_text_aa(
                12,
                st_ty,
                s,
                ath_tokens::TYPE_CAPTION,
                TEXT_MUTED,
                FontFamily::Sans,
            );
        }
    }
    let hint = match app.view {
        View::Grid => "Enter:open  Arrows:move  Esc:quit",
        View::Single => "Left/Right:prev/next  Esc:grid",
    };
    let hw = canvas.measure_text_aa(hint, ath_tokens::TYPE_CAPTION, FontFamily::Sans);
    canvas.draw_text_aa(
        (WIN_W - 12) as i32 - hw,
        st_ty,
        hint,
        ath_tokens::TYPE_CAPTION,
        TEXT_MUTED,
        FontFamily::Sans,
    );
}

fn render_grid(app: &mut App, canvas: &mut Canvas) {
    let grid_y = TITLE_H + TOOLBAR_H;
    let grid_h = WIN_H - grid_y - STATUS_H;
    canvas.fill_rect(0, grid_y, WIN_W, grid_h, GRID_BG);

    if app.photos.is_empty() {
        canvas.draw_text_aa(
            24,
            (grid_y + 24) as i32,
            "No photos in your Pictures library yet.",
            ath_tokens::TYPE_BODY,
            TEXT_MUTED,
            FontFamily::Sans,
        );
        return;
    }

    let cols = app.cols();
    let vis_rows = app.visible_rows();
    let start = app.scroll_row * cols;
    let end = (start + vis_rows * cols).min(app.photos.len());

    for i in start..end {
        // Lazily decode the thumbnail when it first scrolls into view.
        app.ensure_thumb(i);
        let rel = i - start;
        let col = rel % cols;
        let row = rel / cols;
        let cx = THUMB_PAD / 2 + col * CELL_W;
        let cy = grid_y + THUMB_PAD / 2 + row * CELL_H;
        let selected = i == app.selected;

        // Tile background + selection frame.
        if selected {
            canvas.fill_rounded_rect(
                cx.saturating_sub(4),
                cy.saturating_sub(4),
                THUMB_W + 8,
                THUMB_H + LABEL_H + 8,
                ath_tokens::RADIUS_SM as usize,
                sel_fill(),
            );
        }
        canvas.fill_rect(cx, cy, THUMB_W, THUMB_H, TILE_BG);

        let p = &app.photos[i];
        if let Some(thumb) = p.thumb.as_ref() {
            blit_argb(canvas, thumb, cx, cy);
        } else {
            // Placeholder: a broken-image glyph centered in the tile.
            let glyph = "[ x ]";
            let gw = canvas.measure_text_aa(glyph, ath_tokens::TYPE_BODY, FontFamily::Sans);
            canvas.draw_text_aa(
                (cx + (THUMB_W - gw as usize) / 2) as i32,
                (cy + THUMB_H / 2 - 8) as i32,
                glyph,
                ath_tokens::TYPE_BODY,
                if p.attempted {
                    DARK.state_danger
                } else {
                    TEXT_MUTED
                },
                FontFamily::Sans,
            );
        }
        canvas.draw_rect_outline(cx, cy, THUMB_W, THUMB_H, STROKE_HL);

        // Label (truncated file name) under the tile.
        let name = p.name();
        let label_y = cy + THUMB_H + 2;
        let fg = if selected { TEXT_FG } else { TEXT_MUTED };
        draw_label_clipped(canvas, name, cx, label_y, THUMB_W, fg);
    }
}

fn render_single(app: &App, canvas: &mut Canvas) {
    let area_y = TITLE_H + TOOLBAR_H;
    let area_h = WIN_H - area_y - STATUS_H;
    canvas.fill_rect(0, area_y, WIN_W, area_h, VIEW_SCRIM);

    // Nav affordances (clickable; same rects as `App::hit`): a "< Grid" back
    // button plus left/right chevrons centered in the edge hot-zones.
    let back = single_back_rect();
    canvas.fill_rounded_rect(
        back.x,
        back.y,
        back.w,
        back.h,
        ath_tokens::RADIUS_XS as usize,
        DARK.bg_elevated,
    );
    let bl = "Grid";
    let blw = canvas.measure_text_aa(bl, ath_tokens::TYPE_LABEL, FontFamily::Sans);
    canvas.draw_text_aa(
        back.x as i32 + (back.w as i32 - blw) / 2,
        (back.y + (back.h - ath_tokens::TYPE_LABEL.line_height as usize) / 2) as i32,
        bl,
        ath_tokens::TYPE_LABEL,
        TEXT_FG,
        FontFamily::Sans,
    );
    if app.photos.len() > 1 {
        let prev = single_prev_rect();
        let next = single_next_rect();
        let chev_y = (prev.y + prev.h / 2 - ath_tokens::TYPE_TITLE.line_height as usize / 2) as i32;
        let lw = canvas.measure_text_aa("<", ath_tokens::TYPE_TITLE, FontFamily::Sans);
        canvas.draw_text_aa(
            (prev.x + (prev.w - lw as usize) / 2) as i32,
            chev_y,
            "<",
            ath_tokens::TYPE_TITLE,
            accent(),
            FontFamily::Sans,
        );
        let rw = canvas.measure_text_aa(">", ath_tokens::TYPE_TITLE, FontFamily::Sans);
        canvas.draw_text_aa(
            (next.x + (next.w - rw as usize) / 2) as i32,
            chev_y,
            ">",
            ath_tokens::TYPE_TITLE,
            accent(),
            FontFamily::Sans,
        );
    }

    if let Some(img) = app.full.as_ref() {
        let pad = 16;
        blit_image_fit(
            canvas,
            img,
            pad,
            area_y + pad,
            WIN_W - pad * 2,
            area_h.saturating_sub(pad * 2 + 22),
        );
        // Caption: name + dimensions.
        let name = app.photos.get(app.selected).map(Photo::name).unwrap_or("");
        let mut dbuf = [0u8; 32];
        let dn = fmt_dims(img.width, img.height, &mut dbuf);
        let cap_y = (area_y + area_h - 18) as i32;
        let nw = canvas.draw_text_aa(
            16,
            cap_y,
            name,
            ath_tokens::TYPE_CAPTION,
            TEXT_FG,
            FontFamily::Sans,
        );
        if let Ok(s) = core::str::from_utf8(&dbuf[..dn]) {
            canvas.draw_text_aa(
                16 + nw + 12,
                cap_y,
                s,
                ath_tokens::TYPE_CAPTION,
                accent(),
                FontFamily::Sans,
            );
        }
    } else {
        // Decode failed: a centered "can't display" message, app stays alive.
        let msg = "Can't display this image";
        let mw = canvas.measure_text_aa(msg, ath_tokens::TYPE_BODY, FontFamily::Sans);
        canvas.draw_text_aa(
            ((WIN_W - mw as usize) / 2) as i32,
            (area_y + area_h / 2) as i32,
            msg,
            ath_tokens::TYPE_BODY,
            DARK.state_danger,
            FontFamily::Sans,
        );
    }
}

/// Draw a file name clipped to `max_w` pixels (ellipsis when it overflows).
fn draw_label_clipped(canvas: &mut Canvas, name: &str, x: usize, y: usize, max_w: usize, fg: u32) {
    let full_w = canvas.measure_text_aa(name, ath_tokens::TYPE_CAPTION, FontFamily::Sans);
    if (full_w as usize) <= max_w {
        canvas.draw_text_aa(
            x as i32,
            y as i32,
            name,
            ath_tokens::TYPE_CAPTION,
            fg,
            FontFamily::Sans,
        );
        return;
    }
    // Trim characters until name + ".." fits.
    let bytes = name.as_bytes();
    let mut take = bytes.len();
    let mut clipped = [0u8; NAME_CAP + 2];
    while take > 0 {
        let mut buf = [0u8; NAME_CAP + 2];
        let t = take.min(NAME_CAP);
        buf[..t].copy_from_slice(&bytes[..t]);
        buf[t] = b'.';
        buf[t + 1] = b'.';
        if let Ok(s) = core::str::from_utf8(&buf[..t + 2]) {
            let w = canvas.measure_text_aa(s, ath_tokens::TYPE_CAPTION, FontFamily::Sans);
            if (w as usize) <= max_w {
                clipped[..t + 2].copy_from_slice(&buf[..t + 2]);
                if let Ok(cs) = core::str::from_utf8(&clipped[..t + 2]) {
                    canvas.draw_text_aa(
                        x as i32,
                        y as i32,
                        cs,
                        ath_tokens::TYPE_CAPTION,
                        fg,
                        FontFamily::Sans,
                    );
                }
                return;
            }
        }
        take -= 1;
    }
}

fn fmt_count(n: usize, out: &mut [u8]) -> usize {
    let mut len = fmt_u64(n as u64, out);
    let suffix: &[u8] = if n == 1 { b" photo" } else { b" photos" };
    for &b in suffix {
        out[len] = b;
        len += 1;
    }
    len
}

fn fmt_dims(w: u32, h: u32, out: &mut [u8]) -> usize {
    let mut n = fmt_u64(w as u64, out);
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

// ── Design proof (R10: a fail-able check the token wiring + pipeline are right) ─

/// True iff Photos' chrome is wired to the shared design tokens AND the image
/// pipeline (decode → thumbnail downscale → EXIF rotation) produces the expected
/// pixels. Deliberately fail-able: a regression in token wiring, the decoder, the
/// downscale math, or the EXIF transform flips this to `false` (exit code 3 at
/// startup). This is the proof an ELF bin can carry (no `cargo test` here);
/// `athmedia`'s own host KATs prove the decode/EXIF logic in isolation.
#[must_use]
pub fn design_proof() -> bool {
    let ramp = ath_tokens::derive_accent(theme_seed(), &DARK);
    let tokens_ok = accent() == ramp.base
        && sel_fill() == ramp.active
        && BG == DARK.bg_raised
        && TITLE_BG == DARK.bg_base
        && TOOLBAR_BG == DARK.bg_overlay
        && TEXT_FG == DARK.text_primary
        && TEXT_MUTED == DARK.text_secondary
        && STROKE_HL == DARK.stroke_strong
        && athkit::sys::THEME_DEFAULT_ACCENT == RAEBLUE;

    let pipeline_ok = pipeline_decode_downscale_exif_ok();

    tokens_ok && pipeline_ok && hit_test_proof() && gif_animation_proof() && prefs_round_trip_ok()
}

/// Prove the Photos PREFS SCHEMA: a known non-default `Prefs` serialized via
/// `ath_toml` then re-parsed restores every field exactly (last folder, view mode,
/// last-selected photo), AND a corrupt / missing-key document resolves to the
/// typed defaults (NOT a panic, NOT a wrong value). This proves the per-app prefs
/// contract on top of `ath_toml`'s own parser KATs. Returns `false` on any drift
/// (→ exit(3) at startup).
#[must_use]
fn prefs_round_trip_ok() -> bool {
    // (a) Full round-trip of a non-default Prefs.
    let p = Prefs {
        last_folder: String::from("/home/rae/Vacation"),
        single: true,
        last_photo: String::from("sunset 2.jpg"),
    };
    let text = ath_toml::to_string(&p.to_toml());
    let parsed = match ath_toml::parse(&text) {
        Ok(t) => t,
        Err(_) => return false,
    };
    let back = Prefs::from_toml(&parsed);
    if back.last_folder != "/home/rae/Vacation" || !back.single || back.last_photo != "sunset 2.jpg"
    {
        return false;
    }

    // (b) View token round-trips through its stable string form.
    for v in [View::Grid, View::Single] {
        if View::from_token(v.as_token()) != v {
            return false;
        }
    }

    // (c) A corrupt document → typed defaults (parse FAILS, we don't panic).
    let corrupt = "view = = oops\n[unterminated\n";
    let d = match ath_toml::parse(corrupt) {
        Ok(t) => Prefs::from_toml(&t), // shouldn't reach here for this input
        Err(_) => Prefs::defaults(),
    };
    if !d.last_folder.is_empty() || d.single || !d.last_photo.is_empty() {
        return false;
    }

    // (d) A well-formed doc MISSING every prefs key → typed defaults per field.
    let empty = match ath_toml::parse("unrelated = 1\n") {
        Ok(t) => t,
        Err(_) => return false,
    };
    let e = Prefs::from_toml(&empty);
    if !e.last_folder.is_empty() || e.single || !e.last_photo.is_empty() {
        return false;
    }

    // (e) An unknown view token → the default Grid (single == false), not a crash.
    let bad = match ath_toml::parse("view = \"bogus\"\nlast_photo = \"a.png\"\n") {
        Ok(t) => t,
        Err(_) => return false,
    };
    let c = Prefs::from_toml(&bad);
    if c.single || c.last_photo != "a.png" {
        return false;
    }

    true
}

/// Prove the animated-GIF wiring (exit code 3 on failure): build a hand-rolled
/// 2-frame GIF fixture, decode it through the real `ath_gif::decode_gif`, and
/// assert: (1) two frames, (2) the two frames' pixels DIFFER, (3) frame 0's
/// thumbnail/still image is frame 0's color, and (4) the animation timer advances
/// to frame 1 once frame 0's `delay_ms` has elapsed (but NOT before). `ath_gif`'s
/// 17 host KATs prove the decode; this proves THIS app's wiring + the
/// frame-advance timer logic. FAIL-able by construction.
#[must_use]
fn gif_animation_proof() -> bool {
    let gif_bytes = build_test_gif();
    // The sniff the open path uses must classify it as a GIF.
    if !is_gif(&gif_bytes) {
        return false;
    }
    let gif = match decode_gif(&gif_bytes) {
        Ok(g) => g,
        Err(_) => return false,
    };
    // (1) Two frames.
    if gif.frames.len() != 2 {
        return false;
    }
    // (2) The two frames' pixels differ (red vs blue).
    if gif.frames[0].pixels == gif.frames[1].pixels {
        return false;
    }
    if gif.frames[0].delay_ms != 50 {
        return false;
    }
    // (3) Frame 0's still image is opaque red; frame 1's is opaque blue.
    let f0 = match gif_frame_image(&gif, 0) {
        Some(i) => i,
        None => return false,
    };
    if f0.pixel(0, 0) != Some((0xFF, 255, 0, 0)) {
        return false;
    }
    if gif_frame_image(&gif, 1).and_then(|i| i.pixel(0, 0)) != Some((0xFF, 0, 0, 255)) {
        return false;
    }

    // (4) Frame-advance timer logic. Build an App holding this animation at frame
    //     0. `tick_animation` compares `time_ns()` against the per-frame delay.
    let mut app = App {
        dir: PathBuf::new(),
        photos: Vec::new(),
        selected: 0,
        scroll_row: 0,
        view: View::Single,
        full: gif_frame_image(&gif, 0),
        anim: Some(gif),
        current_frame: 0,
        last_advance_ns: 0,
        toast: [0; 64],
        toast_len: 0,
    };

    // (4a) With `last_advance_ns` freshly stamped to NOW, no time has elapsed, so
    //      the timer must NOT advance (guards against an always-advance spin).
    app.last_advance_ns = athkit::sys::time_ns();
    app.current_frame = 0;
    if app.tick_animation() {
        return false;
    }
    if app.current_frame != 0 {
        return false;
    }

    // (4b) With `last_advance_ns = 0` (far in the past), more than frame 0's 50ms
    //      delay has elapsed by any real wall clock, so the timer MUST advance to
    //      frame 1 and mirror it into `full`.
    app.last_advance_ns = 0;
    app.current_frame = 0;
    if !app.tick_animation() {
        return false;
    }
    if app.current_frame != 1 {
        return false;
    }
    // The mirrored still must now be frame 1 (blue), not frame 0 (red).
    if app.full.as_ref().and_then(|i| i.pixel(0, 0)) != Some((0xFF, 0, 0, 255)) {
        return false;
    }

    // (4c) Looping: a second elapsed advance wraps back to frame 0 (red).
    app.last_advance_ns = 0;
    if !app.tick_animation() {
        return false;
    }
    if app.current_frame != 0 {
        return false;
    }
    app.full.as_ref().and_then(|i| i.pixel(0, 0)) == Some((0xFF, 255, 0, 0))
}

/// LZW-encode a single palette `index` for a 1x1 image at `min_code_size` 2: the
/// stream is exactly `CLEAR(4) index EOI(5)`, each a 3-bit code packed LSB-first.
fn gif_lzw_single_pixel(index: u8) -> [u8; 2] {
    let mut bits: u32 = 0;
    bits |= 4u32; // CLEAR at bit 0
    bits |= (index as u32) << 3; // index at bit 3
    bits |= 5u32 << 6; // EOI at bit 6 (9 bits total → 2 bytes)
    [(bits & 0xFF) as u8, ((bits >> 8) & 0xFF) as u8]
}

/// Assemble a 1x1, 2-frame GIF89a with a 2-color global table [red, blue]: frame
/// 0 draws index 0 (red), frame 1 draws index 1 (blue), each with a 5cs (50ms)
/// delay. Hand-built so the Photos binary carries no GIF *encoder*.
fn build_test_gif() -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    out.extend_from_slice(b"GIF89a");
    out.extend_from_slice(&1u16.to_le_bytes()); // width 1
    out.extend_from_slice(&1u16.to_le_bytes()); // height 1
    out.push(0x80); // global color table present, size field 0 → 2 entries
    out.push(0); // background color index
    out.push(0); // pixel aspect ratio
    out.extend_from_slice(&[255, 0, 0]); // index 0 = red
    out.extend_from_slice(&[0, 0, 255]); // index 1 = blue

    let mut frame = |index: u8| {
        out.push(0x21); // extension introducer
        out.push(0xF9); // Graphic Control Extension
        out.push(4); // block size
        out.push(1 << 2); // disposal = Keep (1) in bits 2..4, no transparency
        out.extend_from_slice(&5u16.to_le_bytes()); // delay 5cs → 50ms
        out.push(0); // transparent index (unused)
        out.push(0); // block terminator
        out.push(0x2C); // Image Descriptor
        out.extend_from_slice(&0u16.to_le_bytes()); // left
        out.extend_from_slice(&0u16.to_le_bytes()); // top
        out.extend_from_slice(&1u16.to_le_bytes()); // width
        out.extend_from_slice(&1u16.to_le_bytes()); // height
        out.push(0); // packed: no local table, not interlaced
        out.push(2); // LZW minimum code size
        let lzw = gif_lzw_single_pixel(index);
        out.push(lzw.len() as u8); // single sub-block length
        out.extend_from_slice(&lzw);
        out.push(0); // sub-block terminator
    };
    frame(0);
    frame(1);
    out.push(0x3B); // trailer
    out
}

/// Prove the mouse hit-test invariant: a click on thumbnail 0's rect-center
/// opens single-view; the single-view nav zones map to the right actions; an
/// out-of-bounds click resolves to `Action::None`. Returns `false` on any drift
/// (→ exit(3) at startup). Builds a synthetic 3-photo app so grid geometry exists
/// without touching the real Pictures directory.
#[must_use]
fn hit_test_proof() -> bool {
    let mk_app = || {
        let mut app = App {
            dir: PathBuf::new(),
            photos: Vec::new(),
            selected: 0,
            scroll_row: 0,
            view: View::Grid,
            full: None,
            anim: None,
            current_frame: 0,
            last_advance_ns: 0,
            toast: [0; 64],
            toast_len: 0,
        };
        for label in [b"a.png".as_slice(), b"b.png", b"c.png"] {
            let mut name = [0u8; NAME_CAP];
            name[..label.len()].copy_from_slice(label);
            app.photos.push(Photo {
                name,
                name_len: label.len(),
                thumb: None,
                decoded_ok: false,
                attempted: false,
            });
        }
        app
    };

    // (1) Grid view: a click at thumbnail 0's center hits OpenThumb(0).
    let app = mk_app();
    let r0 = match app.thumb_rect(0) {
        Some(r) => r,
        None => return false,
    };
    if app.hit((r0.x + r0.w / 2) as i32, (r0.y + r0.h / 2) as i32) != Action::OpenThumb(0) {
        return false;
    }
    // Thumbnail 1 is a distinct tile (guards against all-rects-overlap).
    if let Some(r1) = app.thumb_rect(1) {
        if app.hit((r1.x + r1.w / 2) as i32, (r1.y + r1.h / 2) as i32) != Action::OpenThumb(1) {
            return false;
        }
    } else {
        return false;
    }

    // (2) Out-of-bounds (below the grid) resolves to None.
    if app.hit(-100, -100) != Action::None {
        return false;
    }

    // (3) Dispatching OpenThumb(0) flips to Single view (decode of the absent
    // file fails, but the view transition is unconditional — open_single sets it).
    let mut app_s = mk_app();
    let _ = app_s.dispatch(Action::OpenThumb(0));
    if app_s.view != View::Single {
        return false;
    }

    // (4) Single view: the prev/next edge zones + back button map correctly.
    let prev = single_prev_rect();
    let next = single_next_rect();
    let back = single_back_rect();
    if app_s.hit((prev.x + prev.w / 2) as i32, (prev.y + prev.h / 2) as i32) != Action::NavPrev {
        return false;
    }
    if app_s.hit((next.x + next.w / 2) as i32, (next.y + next.h / 2) as i32) != Action::NavNext {
        return false;
    }
    if app_s.hit((back.x + back.w / 2) as i32, (back.y + back.h / 2) as i32) != Action::BackToGrid {
        return false;
    }

    // (5) BackToGrid dispatch returns to the grid.
    let _ = app_s.dispatch(Action::BackToGrid);
    app_s.view == View::Grid
}

/// Build a 2x2 known-pixel PNG, decode it through `athmedia::png`, then assert:
///   (1) the four decoded ARGB pixels are exact,
///   (2) a thumbnail downscale to 8x8 nearest-samples the right source pixels,
///   (3) an EXIF 90°-CW rotation moves a known corner correctly.
/// Returns `false` on any drift. Self-contained (no fixture file): the PNG is
/// assembled from a stored zlib block with a live CRC/Adler, the construction the
/// decoder's host KATs use.
fn pipeline_decode_downscale_exif_ok() -> bool {
    // 2x2 RGB, filter-0 per scanline:
    //   row0: red (255,0,0)  green (0,255,0)
    //   row1: blue (0,0,255) white (255,255,255)
    let raw = [
        0u8, 255, 0, 0, 0, 255, 0, // filter, px(0,0), px(1,0)
        0, 0, 0, 255, 255, 255, 255, // filter, px(0,1), px(1,1)
    ];
    let png = build_test_png(2, 2, &raw);

    let pimg = match decode_png(&png) {
        Ok(i) => i,
        Err(_) => return false,
    };
    if pimg.width != 2 || pimg.height != 2 {
        return false;
    }
    let img = DecodedImage {
        width: pimg.width,
        height: pimg.height,
        pixels: pimg.pixels,
    };
    if img.pixel(0, 0) != Some((0xFF, 255, 0, 0))
        || img.pixel(1, 0) != Some((0xFF, 0, 255, 0))
        || img.pixel(0, 1) != Some((0xFF, 0, 0, 255))
        || img.pixel(1, 1) != Some((0xFF, 255, 255, 255))
    {
        return false;
    }

    // (2) Downscale to 8x8 (4x scale, no letterbox at this aspect): nearest
    // sampling maps dest 0..3 → src 0, 4..7 → src 1 on each axis.
    let thumb = downscale_fit(&img, 8, 8);
    if thumb.width != 8 || thumb.height != 8 {
        return false;
    }
    // Interior (5,5) → source (1,1) opaque white over the tile bg.
    if thumb.pixels[5 * 8 + 5] != over(0xFF_FF_FF_FF, TILE_BG) {
        return false;
    }
    // Interior (2,2) → source (0,0) red — guards against a flood/ignored-source bug.
    if thumb.pixels[2 * 8 + 2] != over(0xFF_FF_00_00, TILE_BG) {
        return false;
    }

    // (3) EXIF 90°-CW rotation of the 2x2: source top-left (0,0)=red must land
    // at the destination top-right. For a 2x2, Rotate90Cw maps (sx,sy)->(h-1-sy,sx):
    //   (0,0)red -> (1,0); (1,0)green -> (1,1); (0,1)blue -> (0,0); (1,1)white -> (0,1)
    let rot = apply_orientation(&img, Orientation::Rotate90Cw);
    if rot.width != 2 || rot.height != 2 {
        return false;
    }
    if rot.pixel(1, 0).map(|p| (p.1, p.2, p.3)) != Some((255, 0, 0)) {
        return false;
    }
    if rot.pixel(0, 0).map(|p| (p.1, p.2, p.3)) != Some((0, 0, 255)) {
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

// ── Entry point ───────────────────────────────────────────────────────────

/// The live Photos app: run the fail-able `design_proof()` gate, create the
/// window surface, then drive the mouse/keyboard/animation event loop forever.
/// The thin `src/main.rs` bin's `_start` calls this. Never returns (exits via
/// `SYS_EXIT`).
pub fn run() -> ! {
    if !design_proof() {
        athkit::sys::exit(3);
    }
    let sid = athkit::sys::surface_create(WIN_W as u64, WIN_H as u64, SURFACE_VIRT);
    if sid == u64::MAX {
        athkit::sys::exit(1);
    }

    let mut canvas = unsafe { Canvas::new(SURFACE_VIRT as *mut u8, WIN_W, WIN_H, 4) };

    let mut app = App::new();
    render(&mut app, &mut canvas);
    athkit::sys::surface_present(sid, PRESENT_X as u64, PRESENT_Y as u64);

    let mut extended = false;
    let mut left_was_down = false;

    loop {
        // ── Mouse: drain button events, hit-test the cursor on a click edge ──
        let mut mouse_activity = false;
        let mut left_down = left_was_down;
        loop {
            let ev = athkit::sys::poll_mouse();
            if ev == 0 {
                break;
            }
            left_down = (ev & 0x01) != 0;
            mouse_activity = true;
        }
        if mouse_activity || left_down != left_was_down {
            if left_down && !left_was_down {
                let (cx, cy, _btn) = athkit::sys::cursor_pos();
                // Subtract the LIVE window origin (not the stale present-time
                // PRESENT_X/Y) so clicks land correctly after the window manager
                // moves the window (Overview / Spaces / tiling). Falls back to the
                // present origin if the surface isn't found. Saturating-sub keeps a
                // cursor above/left of the window from underflowing.
                let (ox, oy) = athkit::sys::surface_origin(sid)
                    .unwrap_or((PRESENT_X as u32, PRESENT_Y as u32));
                let lx = (cx as i32).saturating_sub(ox as i32);
                let ly = (cy as i32).saturating_sub(oy as i32);
                let action = app.hit(lx, ly);
                if app.dispatch(action) {
                    render(&mut app, &mut canvas);
                    athkit::sys::surface_present(sid, PRESENT_X as u64, PRESENT_Y as u64);
                }
            }
            left_was_down = left_down;
        }

        // ── GIF animation: advance the current frame on its own wall-clock
        //    cadence, independent of input. Polled every tick (never blocks on
        //    the delay); only re-presents when the frame actually changes. ──
        if app.view == View::Single && app.tick_animation() {
            render(&mut app, &mut canvas);
            athkit::sys::surface_present(sid, PRESENT_X as u64, PRESENT_Y as u64);
        }

        let key = athkit::sys::read_key();
        if key == 0 {
            athkit::sys::yield_now();
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

        match app.view {
            View::Grid => match (ext, code) {
                (true, 0x48) => {
                    app.move_sel(0, -1);
                    dirty = true;
                } // Up
                (true, 0x50) => {
                    app.move_sel(0, 1);
                    dirty = true;
                } // Down
                (true, 0x4B) => {
                    app.move_sel(-1, 0);
                    dirty = true;
                } // Left
                (true, 0x4D) => {
                    app.move_sel(1, 0);
                    dirty = true;
                } // Right
                (false, 0x1C) => {
                    app.open_single();
                    dirty = true;
                } // Enter = open
                (false, 0x39) => {
                    app.open_single();
                    dirty = true;
                } // Space = open
                (false, 0x13) => {
                    app.scan();
                    dirty = true;
                } // 'r' = rescan library
                (false, 0x01) => {
                    athkit::sys::exit(0);
                } // Esc = quit
                _ => {}
            },
            View::Single => match (ext, code) {
                (true, 0x4B) => {
                    app.nav_single(-1);
                    dirty = true;
                } // Left = prev
                (true, 0x4D) => {
                    app.nav_single(1);
                    dirty = true;
                } // Right = next
                (false, 0x01) => {
                    app.close_single();
                    dirty = true;
                } // Esc = back to grid
                (false, 0x39) => {
                    app.close_single();
                    dirty = true;
                } // Space = back to grid
                _ => {}
            },
        }

        if dirty {
            render(&mut app, &mut canvas);
            athkit::sys::surface_present(sid, PRESENT_X as u64, PRESENT_Y as u64);
        }
    }
}

// ════════════════════════════════════════════════════════════════════════════
// Host KAT — `cargo test -p photos --features host`. FAIL-able by construction.
//
// The slice this proves: Photos' still-image open path ([`decode_oriented`]) now
// goes through the unified `ath_image` dispatcher, so it opens BMP and WebP IN
// ADDITION to the existing PNG/JPEG/GIF. The test feeds an in-memory BMP and an
// in-memory WebP (the NEW formats) plus a PNG control (no regression) through the
// EXACT function the live grid/single-view call, and asserts a non-empty bitmap
// with the right dimensions AND exact pixels — so a wrong dispatch, a dropped
// format, an R/B swap, or a dimension bug fails the test.
//
// Fixtures are hand-built minimal real headers (BMP 24-bpp bottom-up; WebP VP8L
// single-symbol Huffman) mirroring `ath_image`'s own proven KAT corpus, and a PNG
// control encoded via the in-tree `ath_image::encode`. Under #[cfg(test)] the
// crate builds as std, so `Vec`/`vec!` come from the imported `alloc`.
// ════════════════════════════════════════════════════════════════════════════
#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    /// A tiny LSB-first bit writer for hand-building the VP8L bitstream (the same
    /// construction `ath_image`'s WebP KAT uses).
    struct BitWriter {
        bytes: Vec<u8>,
        cur: u32,
        nbits: u32,
    }
    impl BitWriter {
        fn new() -> Self {
            BitWriter {
                bytes: Vec::new(),
                cur: 0,
                nbits: 0,
            }
        }
        fn put(&mut self, val: u32, bits: u32) {
            for i in 0..bits {
                let b = (val >> i) & 1;
                self.cur |= b << self.nbits;
                self.nbits += 1;
                if self.nbits == 8 {
                    self.bytes.push(self.cur as u8);
                    self.cur = 0;
                    self.nbits = 0;
                }
            }
        }
        fn into_bytes(mut self) -> Vec<u8> {
            if self.nbits > 0 {
                self.bytes.push(self.cur as u8);
            }
            self.bytes
        }
    }

    /// Hand-build a minimal 24-bpp bottom-up BMP of a 2x2 image:
    ///   (0,0)=red (1,0)=green / (0,1)=blue (1,1)=white.
    /// BMP rows are bottom-up and BGR, padded to a 4-byte boundary.
    fn bmp_2x2() -> Vec<u8> {
        let row_stride = 8usize; // 2px*3 = 6, padded to 8
        let pixel_data_size = row_stride * 2;
        let offset = 14 + 40usize;
        let total = offset + pixel_data_size;
        let mut v = Vec::new();
        v.extend_from_slice(b"BM");
        v.extend_from_slice(&(total as u32).to_le_bytes());
        v.extend_from_slice(&[0, 0, 0, 0]);
        v.extend_from_slice(&(offset as u32).to_le_bytes());
        v.extend_from_slice(&40u32.to_le_bytes());
        v.extend_from_slice(&2i32.to_le_bytes()); // width
        v.extend_from_slice(&2i32.to_le_bytes()); // height (+ = bottom-up)
        v.extend_from_slice(&1u16.to_le_bytes());
        v.extend_from_slice(&24u16.to_le_bytes());
        v.extend_from_slice(&0u32.to_le_bytes()); // BI_RGB
        v.extend_from_slice(&(pixel_data_size as u32).to_le_bytes());
        v.extend_from_slice(&0i32.to_le_bytes());
        v.extend_from_slice(&0i32.to_le_bytes());
        v.extend_from_slice(&0u32.to_le_bytes());
        v.extend_from_slice(&0u32.to_le_bytes());
        // Bottom-up: image row y=1 first (blue, white), then y=0 (red, green). BGR.
        v.extend_from_slice(&[0xFF, 0x00, 0x00]); // blue
        v.extend_from_slice(&[0xFF, 0xFF, 0xFF]); // white
        v.extend_from_slice(&[0x00, 0x00]); // pad
        v.extend_from_slice(&[0x00, 0x00, 0xFF]); // red
        v.extend_from_slice(&[0x00, 0xFF, 0x00]); // green
        v.extend_from_slice(&[0x00, 0x00]); // pad
        v
    }

    /// Hand-build a minimal lossless VP8L WebP carrying a solid-color image (no
    /// transforms, no color cache, no meta-Huffman, five single-symbol "simple"
    /// Huffman code groups) — the exact bit layout `ath_image`'s WebP KAT uses.
    fn webp_solid(width: u32, height: u32, a: u8, r: u8, g: u8, b: u8) -> Vec<u8> {
        let mut w = BitWriter::new();
        w.put(0x2F, 8); // VP8L signature
        w.put(width - 1, 14);
        w.put(height - 1, 14);
        w.put(0, 1); // alpha_used
        w.put(0, 3); // version
        w.put(0, 1); // no transform
        w.put(0, 1); // no color cache
        w.put(0, 1); // no meta-huffman
        let simple = |w: &mut BitWriter, sym: u32| {
            w.put(1, 1);
            w.put(0, 1);
            w.put(1, 1);
            w.put(sym, 8);
        };
        simple(&mut w, g as u32);
        simple(&mut w, r as u32);
        simple(&mut w, b as u32);
        simple(&mut w, a as u32);
        simple(&mut w, 0); // distance (unused)
        let payload = w.into_bytes();
        let mut body = Vec::new();
        body.extend_from_slice(b"VP8L");
        body.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        body.extend_from_slice(&payload);
        if payload.len() & 1 == 1 {
            body.push(0);
        }
        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&((4 + body.len()) as u32).to_le_bytes());
        out.extend_from_slice(b"WEBP");
        out.extend_from_slice(&body);
        out
    }

    /// A real 2x2 PNG via the in-tree encoder (the no-regression control):
    ///   (0,0)=red (1,0)=green / (0,1)=blue (1,1)=white.
    fn png_2x2() -> Vec<u8> {
        let img = ath_image::Image {
            width: 2,
            height: 2,
            pixels: vec![0xFFFF0000, 0xFF00FF00, 0xFF0000FF, 0xFFFFFFFF],
        };
        ath_image::encode(&img, ath_image::ImageFormat::Png).expect("encode png control")
    }

    // ── 1. NEW: Photos now opens a BMP through its real decode dispatch ───────
    #[test]
    fn photos_opens_bmp() {
        let bytes = bmp_2x2();
        assert_eq!(ath_image::detect(&bytes), FileKind::Bmp);
        let img = decode_oriented(&bytes).expect("photos decodes BMP via ath_image");
        assert_eq!((img.width, img.height), (2, 2));
        assert!(!img.pixels.is_empty());
        assert_eq!(img.pixel(0, 0), Some((0xFF, 255, 0, 0))); // red
        assert_eq!(img.pixel(1, 0), Some((0xFF, 0, 255, 0))); // green
        assert_eq!(img.pixel(0, 1), Some((0xFF, 0, 0, 255))); // blue
        assert_eq!(img.pixel(1, 1), Some((0xFF, 255, 255, 255))); // white
                                                                  // FAIL-ability: an R/B swap or wrong dispatch makes (0,0) not pure red.
        assert_ne!(img.pixel(0, 0), Some((0xFF, 0, 0, 255)));
    }

    // ── 2. NEW: Photos now opens a WebP (VP8L) through its decode dispatch ────
    #[test]
    fn photos_opens_webp() {
        // 2x1 solid green (A=255, R=0, G=255, B=0).
        let bytes = webp_solid(2, 1, 255, 0, 255, 0);
        assert_eq!(ath_image::detect(&bytes), FileKind::Webp);
        let img = decode_oriented(&bytes).expect("photos decodes WebP via ath_image");
        assert_eq!((img.width, img.height), (2, 1));
        assert!(!img.pixels.is_empty());
        assert_eq!(img.pixel(0, 0), Some((0xFF, 0, 255, 0))); // green, opaque
        assert_eq!(img.pixel(1, 0), Some((0xFF, 0, 255, 0)));
        // FAIL-ability: a channel mix-up would not be pure green.
        assert_ne!(img.pixel(0, 0), Some((0xFF, 255, 0, 0)));
    }

    // ── 3. No regression: a PNG still opens, exact pixels ────────────────────
    #[test]
    fn photos_opens_png_no_regression() {
        let bytes = png_2x2();
        assert_eq!(ath_image::detect(&bytes), FileKind::Png);
        let img = decode_oriented(&bytes).expect("photos decodes PNG via ath_image");
        assert_eq!((img.width, img.height), (2, 2));
        assert_eq!(img.pixel(0, 0), Some((0xFF, 255, 0, 0))); // red
        assert_eq!(img.pixel(1, 0), Some((0xFF, 0, 255, 0))); // green
        assert_eq!(img.pixel(0, 1), Some((0xFF, 0, 0, 255))); // blue
        assert_eq!(img.pixel(1, 1), Some((0xFF, 255, 255, 255))); // white
    }

    // ── 4. GIF still routes off the still path (animated-GIF preserved) ───────
    //
    // `is_gif` is the branch `open_single` uses to keep all frames for the
    // animation loop; the built-in 2-frame fixture must classify as a GIF and
    // its first frame must decode through `decode_oriented` (the thumbnail/still).
    #[test]
    fn gif_routes_to_animation_path() {
        let bytes = build_test_gif();
        assert!(
            is_gif(&bytes),
            "GIF must be recognized for the animation path"
        );
        assert!(!is_jpeg(&bytes));
        let img = decode_oriented(&bytes).expect("GIF first frame as still");
        assert_eq!((img.width, img.height), (1, 1));
        assert_eq!(img.pixel(0, 0), Some((0xFF, 255, 0, 0))); // frame 0 = red
    }

    // ── 5. Hostile input never panics, returns None ──────────────────────────
    #[test]
    fn corrupt_and_empty_no_panic() {
        assert!(decode_oriented(&[]).is_none());
        assert!(decode_oriented(b"not an image at all").is_none());
        // Valid PNG magic but junk body → decode error → None (no panic).
        let mut bad = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
        bad.extend_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF]);
        assert!(decode_oriented(&bad).is_none());
    }

    // NOTE: the runtime `design_proof()` / `gif_animation_proof()` gates are NOT
    // re-run here — they call raw syscalls (`theme_accent`, `time_ns`) that, on a
    // host `cargo test`, would execute a real `syscall` instruction and fault
    // (the known Windows host-harness crash). The decode dispatch above is
    // syscall-free, so it is the part safely host-provable; the runtime gates
    // stay live in the ELF (`run()` exits 3 on drift) + are covered by the
    // underlying crates' own host KATs.
}
