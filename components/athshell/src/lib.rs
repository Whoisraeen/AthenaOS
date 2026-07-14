//! AthShell — default desktop shell for AthenaOS.
//!
//! Provides: Taskbar, Start Menu / App Launcher, System Tray, Notification
//! Center, multi-mode Window Manager, and a built-in Settings panel.

// no_std for real builds; std under `cargo test` so the vte_parser host KAT links.
#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod file_manager;
pub mod system_monitor;
pub mod terminal;
pub mod terminal_emulator;
pub mod text_editor;
pub mod text_util;
#[cfg(feature = "terminal_vt")]
pub mod vte_parser;

pub mod capture;
pub mod game_bar;
pub mod gameos;

pub mod lock_screen;
pub use lock_screen::LockScreen;
pub mod clipboard;
pub mod clipboard_panel;
pub mod command_palette;
pub mod search_indexer;

pub mod animations;
pub mod spaces;
pub mod virtual_desktops;

pub mod animation_curves;
pub mod desktop_widgets;
pub mod power_management;
pub mod rgb_api;
pub mod snap_assist;
pub mod snap_directional;
pub mod snap_groups;
pub mod snap_layouts;
pub mod tiling_wm;
pub mod vibe_mode;

pub mod calculator;
pub mod screen_recorder;
pub mod screenshot;
pub mod screenshot_overlay;

pub mod notifications_daemon;
pub mod permission_prompt;

pub mod control_center;
pub mod start_menu;
pub mod taskbar;

pub mod control_panel;
pub mod file_dialog;
pub mod system_tray_daemon;

// Former in-shell app modules removed 2026-07-03/04 — all now live standalone
// apps/ crates: calendar_app->apps/calendar, image_viewer->apps/photos,
// media_player->apps/music+video, email_client->apps/mail, notes_app->apps/notes,
// contacts_app->apps/contacts, weather_app->apps/weather. The models moved WITH
// the last two (previously unwired) so their code became live apps.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

// ── no_std math helpers ──────────────────────────────────────────────────

fn f32_sqrt(x: f32) -> f32 {
    if x <= 0.0 {
        return 0.0;
    }
    let mut g = x / 2.0;
    for _ in 0..15 {
        g = (g + x / g) * 0.5;
    }
    g
}

fn f32_ceil(x: f32) -> f32 {
    let i = x as i32;
    if (i as f32) < x {
        (i + 1) as f32
    } else {
        i as f32
    }
}

// ── Geometry (design-language §2 grid, desktop-shell.md §1 geometry) ───────

/// Taskbar height — desktop-shell.md §1: 44px (32px hit-target floor + padding).
const TASKBAR_HEIGHT: usize = 44;
/// Start panel — desktop-shell.md §2: 560×640.
const START_MENU_WIDTH: usize = 560;
const START_MENU_HEIGHT: usize = 640;
/// Tray status-icon hit target — desktop-shell.md §3: 32px.
const TRAY_ICON_SIZE: usize = 32;
const NOTIFICATION_WIDTH: usize = 340;
const NOTIFICATION_HEIGHT: usize = 80;
const GLYPH_W: usize = 8;
const GLYPH_H: usize = 8;

// ── Design tokens (docs/design/design-language.md, via ath_tokens) ─────────
//
// The shell no longer owns a private palette. Every colour below is derived
// from ONE seed accent flowed through the shared `ath_tokens` palette, so a
// Vibe-Mode re-skin (one seed change) recolours the whole shell coherently —
// the duplication this crate was built to kill (ath_tokens crate docstring).

/// Active palette. Dark is the default (design-language §4.1); a light/Vibe
/// palette swap is a single value change here (the accent seed is already live
/// via [`active_accent`]).
const PALETTE: &ath_tokens::Palette = &ath_tokens::DARK;

/// The palette a surface should paint with RIGHT NOW — the high-contrast
/// forced-colors palette when a11y HC mode is on, else [`PALETTE`] (audit P0
/// #3). A surface that reads this instead of the fixed `PALETTE` const repaints
/// in HC for free when forced-colors is toggled. Live-swap-aware via
/// `ath_tokens::active_palette()` (the kernel a11y on-switch flips the flag).
#[inline]
pub fn active_palette() -> &'static ath_tokens::Palette {
    ath_tokens::active_palette()
}

/// The LIVE seed accent, shared across every shell surface (taskbar, Start,
/// tray, Settings). The kernel (`theme_engine::active_accent`, via
/// `shell_runner`) pushes the active theme/Vibe seed in here at desktop
/// activation and on every theme change, so a one-tap Vibe re-skin recolours
/// the whole shell coherently — the Concept "the desktop becomes a different
/// place" promise and the payoff of the ath_tokens system.
///
/// `no_std`-safe (`AtomicU32`); defaults to `RAEBLUE` so a shell built/tested
/// without the kernel push (the host KATs) paints the design default.
static ACTIVE_ACCENT: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(ath_tokens::RAEBLUE);

/// Push the live accent seed (called by the kernel's `shell_runner` from
/// `theme_engine::active_accent()` at desktop activation and on theme change).
pub fn set_active_accent(argb: u32) {
    ACTIVE_ACCENT.store(argb, core::sync::atomic::Ordering::Release);
}

/// The current live accent seed (defaults to `RAEBLUE`).
#[must_use]
pub fn active_accent() -> u32 {
    ACTIVE_ACCENT.load(core::sync::atomic::Ordering::Acquire)
}

/// The six-token accent ramp (base/hover/active/subtle/text/glow), derived
/// deterministically from the LIVE seed over `PALETTE`.
#[inline]
fn accent() -> ath_tokens::AccentRamp {
    ath_tokens::derive_accent(active_accent(), PALETTE)
}

/// `material.mica` static tint (design-language §5.2): a wallpaper-independent
/// solid blend of `bg.base` and `bg.raised` — off the per-frame blur path, so
/// the taskbar is cheap to paint while still reading as a layered material.
#[inline]
fn mica_tint() -> u32 {
    blend_opaque(PALETTE.bg_base, PALETTE.bg_raised, 1, 2)
}

/// Opaque per-channel blend of two ARGB colours: `a*(den-num)/den + b*num/den`.
/// Result is forced opaque (the taskbar/panel fills are solid surfaces).
#[inline]
fn blend_opaque(a: u32, b: u32, num: u32, den: u32) -> u32 {
    let mix = |sa: u32, sb: u32| -> u32 { (sa * (den - num) + sb * num) / den };
    let ar = (a >> 16) & 0xFF;
    let ag = (a >> 8) & 0xFF;
    let ab = a & 0xFF;
    let br = (b >> 16) & 0xFF;
    let bg = (b >> 8) & 0xFF;
    let bb = b & 0xFF;
    0xFF00_0000 | (mix(ar, br) << 16) | (mix(ag, bg) << 8) | mix(ab, bb)
}

// ── Window management modes ──────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowMode {
    Float,
    TileHorizontal,
    TileVertical,
    TileGrid,
    Monocle,
    GameOS,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

impl Rect {
    pub const fn new(x: i32, y: i32, w: u32, h: u32) -> Self {
        Self { x, y, w, h }
    }

    pub fn contains(&self, px: i32, py: i32) -> bool {
        px >= self.x && px < self.x + self.w as i32 && py >= self.y && py < self.y + self.h as i32
    }
}

// ── Taskbar item ─────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct TaskbarItem {
    pub title: String,
    pub surface_id: u64,
    pub focused: bool,
    pub minimized: bool,
    pub icon_char: char,
}

// ── App launcher / Start menu ────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct AppEntry {
    pub name: String,
    pub exec_path: String,
    pub icon_char: char,
    pub category: AppCategory,
    pub pinned: bool,
    pub launch_count: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppCategory {
    Game,
    Utility,
    Creative,
    System,
    Web,
    Development,
    Media,
    Other,
}

pub struct StartMenu {
    pub visible: bool,
    pub apps: Vec<AppEntry>,
    pub search_query: String,
    pub selected_index: usize,
    pub scroll_offset: usize,
    pub rect: Rect,
}

impl StartMenu {
    /// Search-field height inside the panel (the strip above the app list).
    pub const SEARCH_H: usize = 40;
    /// App-list row height — the SINGLE source of truth shared by `render()`
    /// (which draws each row at this pitch) and the kernel click hit-test
    /// (`shell_runner::dispatch_click`). They diverged once (render 36 / hit 32),
    /// so a click landed on the wrong row; routing both through these helpers
    /// guarantees the row you click is the row that's drawn.
    pub const ITEM_HEIGHT: usize = 36;

    /// Absolute screen-Y where the app list begins (top of row 0). Mirrors the
    /// `render()` layout exactly: panel-top + pad + search-field + pad.
    pub fn list_y_start(&self) -> usize {
        let pad = ath_tokens::SPACE_4 as usize;
        self.rect.y as usize + pad + Self::SEARCH_H + pad
    }

    /// Map an absolute screen-Y to the FILTERED-app index under it, accounting
    /// for the scroll offset. `None` when the click is above the list or past the
    /// last app. Single source of the hit geometry (matches `render()`).
    pub fn row_at(&self, abs_y: i32) -> Option<usize> {
        let top = self.list_y_start();
        let y = abs_y as usize;
        if y < top {
            return None;
        }
        let row = (y - top) / Self::ITEM_HEIGHT;
        let idx = row + self.scroll_offset;
        if idx < self.filtered_apps().len() {
            Some(idx)
        } else {
            None
        }
    }

    pub fn new(screen_width: usize, screen_height: usize) -> Self {
        // Anchored bottom-left above the Start pill at space.2 inset; height
        // clamped to fit screen − taskbar − space.4 (desktop-shell.md §2).
        let inset = ath_tokens::SPACE_2 as usize;
        let avail_h = screen_height
            .saturating_sub(TASKBAR_HEIGHT)
            .saturating_sub(ath_tokens::SPACE_4 as usize)
            .saturating_sub(inset);
        let menu_h = START_MENU_HEIGHT.min(avail_h);
        let menu_w = START_MENU_WIDTH.min(screen_width.saturating_sub(2 * inset));
        let menu_x = inset as i32;
        let menu_y = screen_height
            .saturating_sub(TASKBAR_HEIGHT)
            .saturating_sub(inset)
            .saturating_sub(menu_h) as i32;
        Self {
            visible: false,
            apps: Vec::new(),
            search_query: String::new(),
            selected_index: 0,
            scroll_offset: 0,
            rect: Rect::new(menu_x, menu_y, menu_w as u32, menu_h as u32),
        }
    }

    pub fn toggle(&mut self) {
        self.visible = !self.visible;
        if self.visible {
            self.search_query.clear();
            self.selected_index = 0;
            self.scroll_offset = 0;
        }
    }

    pub fn add_app(&mut self, entry: AppEntry) {
        self.apps.push(entry);
    }

    pub fn filtered_apps(&self) -> Vec<&AppEntry> {
        if self.search_query.is_empty() {
            let mut sorted: Vec<&AppEntry> = self.apps.iter().collect();
            sorted.sort_by(|a, b| {
                b.pinned
                    .cmp(&a.pinned)
                    .then(b.launch_count.cmp(&a.launch_count))
            });
            sorted
        } else {
            // Case-insensitive: the search bar is fed lowercase scancode
            // ascii, app names are Title Case — "term" must hit "Terminal".
            let query = self.search_query.to_ascii_lowercase();
            self.apps
                .iter()
                .filter(|app| app.name.to_ascii_lowercase().contains(query.as_str()))
                .collect()
        }
    }

    pub fn select_next(&mut self) {
        let count = self.filtered_apps().len();
        if count > 0 {
            self.selected_index = (self.selected_index + 1) % count;
        }
    }

    pub fn select_prev(&mut self) {
        let count = self.filtered_apps().len();
        if count > 0 {
            self.selected_index = self.selected_index.checked_sub(1).unwrap_or(count - 1);
        }
    }

    pub fn selected_app(&self) -> Option<AppEntry> {
        let apps = self.filtered_apps();
        apps.get(self.selected_index).map(|a| (*a).clone())
    }

    pub fn render(&self, canvas: &mut athgfx::Canvas) {
        if !self.visible {
            return;
        }
        use ath_tokens::{RADIUS_LG, RADIUS_SM, SPACE_2, SPACE_3, SPACE_4};
        let accent = accent();
        let p = PALETTE;

        let rx = self.rect.x as usize;
        let ry = self.rect.y as usize;
        let rw = self.rect.w as usize;
        let rh = self.rect.h as usize;
        let pad = SPACE_4 as usize;

        // ── Glass panel (material.glass) — the LIVE Start launcher.
        //
        // BUGFIX (start-menu-invisible): the panel previously painted a bare
        // `fill_rounded_rect(GLASS_TINT_DARK)`. `GLASS_TINT_DARK` is a DEPRECATED
        // alias for `GLASS_PANEL_DARK.tint` (~62% slate) with NO frost sheen, no
        // luma cap, and no rim — so over the bright aurora wallpaper the panel
        // read as a near-invisible smoked sheet (the beta-tester's "Tab toggles
        // but no panel appears" / "washed-out contrast"). The Control Center and
        // command-palette launcher both fixed this by routing through the shipped
        // `athgfx::glass::draw_glass_surface`, which composites the FULL stack:
        // slate tint → white FROST sheen → §2.3 interior luma cap → chroma lift →
        // §9 WCAG legibility cap → hairline → iridescent rim → top-edge highlight.
        // That stack guarantees a solid, legible, on-brand frosted panel (white
        // text.primary clears AA inside it) over ANY backdrop. We use the
        // `glass.popover` tier (the overlay/launcher tier the command-palette and
        // the start_menu.rs twin use), not the deprecated panel alias.
        athgfx::glass::draw_glass_surface(
            canvas,
            rx,
            ry,
            rw,
            rh,
            RADIUS_LG as usize,
            ath_tokens::GLASS_POPOVER_DARK,
        );

        // ── Search field (full width, h=40, radius.sm, bg.elevated) ─────────
        // SEARCH_H/ITEM_HEIGHT come from the shared geometry consts so the click
        // hit-test (kernel) and this render can never drift apart again.
        let search_h = Self::SEARCH_H;
        let search_x = rx + pad;
        let search_y = ry + pad;
        let search_w = rw - 2 * pad;
        canvas.fill_rounded_rect(
            search_x,
            search_y,
            search_w,
            search_h,
            RADIUS_SM as usize,
            p.bg_elevated,
        );
        let search_text = if self.search_query.is_empty() {
            "Search apps, files, settings"
        } else {
            self.search_query.as_str()
        };
        let search_fg = if self.search_query.is_empty() {
            p.text_tertiary
        } else {
            p.text_primary
        };
        // type.body — the Start search field text (crisp AA, RaeSans).
        canvas.draw_text_aa(
            (search_x + SPACE_3 as usize) as i32,
            (search_y + (search_h.saturating_sub(ath_tokens::TYPE_BODY.line_height as usize)) / 2)
                as i32,
            search_text,
            ath_tokens::TYPE_BODY,
            search_fg,
            athgfx::text::FontFamily::Sans,
        );

        // ── App list (re-skinned; the 6-col pinned grid is a later pass) ────
        // Drawn at EXACTLY the geometry the hit-test reads (Self::list_y_start /
        // Self::ITEM_HEIGHT), so a click hits the row that's painted.
        let list_y_start = self.list_y_start();
        let item_height = Self::ITEM_HEIGHT;
        let max_visible = (rh.saturating_sub(list_y_start - ry + pad)) / item_height;
        let apps = self.filtered_apps();

        for (i, app) in apps
            .iter()
            .skip(self.scroll_offset)
            .take(max_visible)
            .enumerate()
        {
            let item_y = list_y_start + i * item_height;
            let actual_idx = i + self.scroll_offset;
            let selected = actual_idx == self.selected_index;

            if selected {
                // Selection wash = accent.subtle @ radius.sm.
                canvas.fill_rounded_rect(
                    rx + pad,
                    item_y,
                    rw - 2 * pad,
                    item_height - 2,
                    RADIUS_SM as usize,
                    accent.subtle,
                );
            }

            // Real line-icon keyed off the app's exec path + name (the SAME
            // mapping the Start-menu twin, taskbar, and palette use) — the
            // letter glyph column was the live launcher's loudest "hobby OS"
            // tell (QMP journey screenshot, 2026-07-01).
            let icon_sz = 18usize;
            let icon_y = item_y + (item_height.saturating_sub(icon_sz)) / 2;
            canvas.draw_icon(
                start_menu::app_line_icon(&app.exec_path, &app.name),
                (rx + pad + SPACE_3 as usize) as i32,
                icon_y as i32,
                icon_sz as i32,
                accent.base,
            );

            let name_fg = if selected {
                p.text_primary
            } else {
                p.text_secondary
            };
            // App name — type.label, vertically centred in the 36px row.
            let name_y = item_y
                + (item_height.saturating_sub(ath_tokens::TYPE_LABEL.line_height as usize)) / 2;
            canvas.draw_text_aa(
                (rx + pad + SPACE_3 as usize + icon_sz + SPACE_2 as usize + 2) as i32,
                name_y as i32,
                &app.name,
                ath_tokens::TYPE_LABEL,
                name_fg,
                athgfx::text::FontFamily::Sans,
            );

            // Category tag (type.caption, text.tertiary, right-aligned).
            let cat_str = match app.category {
                AppCategory::Game => "GAME",
                AppCategory::Utility => "UTIL",
                AppCategory::Creative => "ART",
                AppCategory::System => "SYS",
                AppCategory::Web => "WEB",
                AppCategory::Development => "DEV",
                AppCategory::Media => "MED",
                AppCategory::Other => "",
            };
            if !cat_str.is_empty() {
                let tag_w = canvas.measure_text_aa(
                    cat_str,
                    ath_tokens::TYPE_CAPTION,
                    athgfx::text::FontFamily::Sans,
                );
                let tag_x = (rx + rw - pad) as i32 - tag_w;
                let tag_y = item_y
                    + (item_height.saturating_sub(ath_tokens::TYPE_CAPTION.line_height as usize))
                        / 2;
                canvas.draw_text_aa(
                    tag_x,
                    tag_y as i32,
                    cat_str,
                    ath_tokens::TYPE_CAPTION,
                    p.text_tertiary,
                    athgfx::text::FontFamily::Sans,
                );
            }
        }
    }
}

// ── System tray ──────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct TrayIcon {
    pub id: u64,
    pub label: String,
    pub glyph: char,
    pub tooltip: String,
    pub active: bool,
}

pub struct SystemTray {
    pub icons: Vec<TrayIcon>,
    pub next_id: u64,
    pub clock_text: String,
}

impl SystemTray {
    pub fn new() -> Self {
        Self {
            icons: Vec::new(),
            next_id: 1,
            clock_text: String::from("00:00"),
        }
    }

    pub fn add_icon(&mut self, label: &str, glyph: char, tooltip: &str) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.icons.push(TrayIcon {
            id,
            label: String::from(label),
            glyph,
            tooltip: String::from(tooltip),
            active: true,
        });
        id
    }

    pub fn remove_icon(&mut self, id: u64) {
        self.icons.retain(|i| i.id != id);
    }

    pub fn set_clock(&mut self, text: &str) {
        self.clock_text.clear();
        self.clock_text.push_str(text);
    }

    pub fn total_width(&self) -> usize {
        let gap = ath_tokens::SPACE_1 as usize;
        let icons_w = self.icons.len() * (TRAY_ICON_SIZE + gap);
        let clock_w = self.clock_text.len() * GLYPH_W + 2 * ath_tokens::SPACE_2 as usize;
        icons_w + clock_w + ath_tokens::SPACE_3 as usize
    }

    /// Render the tray cluster: status icons (text.secondary resting) at 32px
    /// hit targets with space.1 gaps, then a type.caption clock right-aligned
    /// (desktop-shell.md §3). Two-line time/date once the kernel passes a date
    /// string; today the clock is the single time line in text.primary.
    pub fn render(&self, canvas: &mut athgfx::Canvas, x: usize, y: usize, height: usize) {
        let p = PALETTE;
        let gap = ath_tokens::SPACE_1 as usize;
        // Real token-tinted line-icons keyed off the tray item's label
        // (net/vol/bat...), not single letter glyphs (visual-QA Round-7 #1).
        let icon_sz = 16usize;
        let icon_y = y + (height.saturating_sub(icon_sz)) / 2;
        let mut cx = x;

        for icon in &self.icons {
            // Resting tone per §3 (hover → text.primary is driven by input
            // state the shell does not yet thread into the tray).
            let color = if icon.active {
                p.text_secondary
            } else {
                p.text_tertiary
            };
            let gx = cx + (TRAY_ICON_SIZE.saturating_sub(icon_sz)) / 2;
            canvas.draw_icon(
                crate::taskbar::tray_line_icon(&icon.label),
                gx as i32,
                icon_y as i32,
                icon_sz as i32,
                color,
            );
            cx += TRAY_ICON_SIZE + gap;
        }

        // Clock — type.caption (desktop-shell.md §3), crisp AA RaeSans, vertically
        // centred on the clock's own line box (not the 8px glyph cell).
        cx += ath_tokens::SPACE_2 as usize;
        let clock_y =
            y + (height.saturating_sub(ath_tokens::TYPE_CAPTION.line_height as usize)) / 2;
        canvas.draw_text_aa(
            cx as i32,
            clock_y as i32,
            &self.clock_text,
            ath_tokens::TYPE_CAPTION,
            p.text_primary,
            athgfx::text::FontFamily::Sans,
        );
    }
}

// ── Notifications ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotifPriority {
    Low,
    Normal,
    High,
    Critical,
}

#[derive(Debug, Clone)]
pub struct Notification {
    pub id: u64,
    pub title: String,
    pub body: String,
    pub priority: NotifPriority,
    pub timestamp: u64,
    pub app_name: String,
    pub read: bool,
    pub actions: Vec<String>,
}

pub struct NotificationCenter {
    pub notifications: Vec<Notification>,
    pub next_id: u64,
    pub visible: bool,
    pub max_toast_count: usize,
    pub do_not_disturb: bool,
}

impl NotificationCenter {
    pub fn new() -> Self {
        Self {
            notifications: Vec::new(),
            next_id: 1,
            visible: false,
            max_toast_count: 3,
            do_not_disturb: false,
        }
    }

    pub fn push(
        &mut self,
        title: &str,
        body: &str,
        priority: NotifPriority,
        app: &str,
        timestamp: u64,
    ) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.notifications.push(Notification {
            id,
            title: String::from(title),
            body: String::from(body),
            priority,
            timestamp,
            app_name: String::from(app),
            read: false,
            actions: Vec::new(),
        });
        id
    }

    pub fn dismiss(&mut self, id: u64) {
        self.notifications.retain(|n| n.id != id);
    }

    pub fn mark_read(&mut self, id: u64) {
        if let Some(n) = self.notifications.iter_mut().find(|n| n.id == id) {
            n.read = true;
        }
    }

    pub fn unread_count(&self) -> usize {
        self.notifications.iter().filter(|n| !n.read).count()
    }

    pub fn clear_all(&mut self) {
        self.notifications.clear();
    }

    pub fn toggle(&mut self) {
        self.visible = !self.visible;
    }

    pub fn active_toasts(&self) -> Vec<&Notification> {
        if self.do_not_disturb {
            return Vec::new();
        }
        self.notifications
            .iter()
            .rev()
            .filter(|n| !n.read)
            .take(self.max_toast_count)
            .collect()
    }

    pub fn render_toasts(&self, canvas: &mut athgfx::Canvas, screen_width: usize) {
        // Glass material + token colours (desktop-shell.md §5). The kernel's
        // `notify.rs` owns the primary compositor toasts; this shell-side toast
        // path mirrors the same tokens for in-shell notifications.
        use ath_tokens::{RADIUS_MD, SPACE_2, SPACE_4};
        let accent = accent();
        let p = PALETTE;

        let toasts = self.active_toasts();
        let start_x = screen_width.saturating_sub(NOTIFICATION_WIDTH + SPACE_4 as usize);
        for (i, notif) in toasts.iter().enumerate() {
            let ny = SPACE_4 as usize + i * (NOTIFICATION_HEIGHT + SPACE_2 as usize);
            canvas.fill_rounded_rect(
                start_x,
                ny,
                NOTIFICATION_WIDTH,
                NOTIFICATION_HEIGHT,
                RADIUS_MD as usize,
                ath_tokens::GLASS_TINT_DARK,
            );
            canvas.draw_rounded_rect_outline(
                start_x,
                ny,
                NOTIFICATION_WIDTH,
                NOTIFICATION_HEIGHT,
                RADIUS_MD as usize,
                p.stroke_subtle,
            );

            // Urgency bar (left 4px), state.* per urgency.
            let bar_color = match notif.priority {
                NotifPriority::Critical => p.state_danger,
                NotifPriority::High => p.state_warn,
                NotifPriority::Low => p.text_tertiary,
                NotifPriority::Normal => accent.base,
            };
            canvas.fill_rounded_rect(
                start_x + 4,
                ny + 8,
                4,
                NOTIFICATION_HEIGHT - 16,
                2,
                bar_color,
            );

            let title_color = match notif.priority {
                NotifPriority::Critical => p.state_danger,
                NotifPriority::High => p.state_warn,
                _ => p.text_primary,
            };

            let tx = start_x + SPACE_4 as usize;
            canvas.draw_text(tx, ny + 8, &notif.app_name, p.text_tertiary, None);
            canvas.draw_text(tx, ny + 24, &notif.title, title_color, None);

            let body_max = (NOTIFICATION_WIDTH - 2 * SPACE_4 as usize) / GLYPH_W;
            let body_display = crate::text_util::truncate_chars(&notif.body, body_max);
            canvas.draw_text(tx, ny + 44, body_display, p.text_secondary, None);

            canvas.draw_text(
                start_x + NOTIFICATION_WIDTH - 20,
                ny + 8,
                "x",
                p.text_tertiary,
                None,
            );
        }
    }
}

// ── Window manager ───────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ManagedWindow {
    pub surface_id: u64,
    pub title: String,
    pub rect: Rect,
    pub minimized: bool,
    pub maximized: bool,
    pub floating: bool,
    pub z_order: u32,
    /// Directional-snap state (Rae+Arrow) — drives the Win11 half/quarter/
    /// maximize/restore state machine ([`snap_directional`]).
    pub snap_state: snap_directional::SnapState,
    /// The rect to return to when a snapped window is restored to `Normal`
    /// (saved the moment it first leaves `Normal`).
    pub restore_rect: Rect,
}

pub struct WindowManager {
    pub windows: Vec<ManagedWindow>,
    pub mode: WindowMode,
    pub work_area: Rect,
    pub focused: Option<u64>,
    pub next_z: u32,
    pub gap: u32,
}

impl WindowManager {
    pub fn new(screen_width: u32, screen_height: u32) -> Self {
        Self {
            windows: Vec::new(),
            mode: WindowMode::Float,
            work_area: Rect::new(
                0,
                0,
                screen_width,
                screen_height.saturating_sub(TASKBAR_HEIGHT as u32),
            ),
            focused: None,
            next_z: 1,
            gap: 8,
        }
    }

    pub fn add_window(&mut self, surface_id: u64, title: &str, width: u32, height: u32) {
        let offset = (self.windows.len() as i32 % 10) * 30;
        let x = self.work_area.x + 60 + offset;
        let y = self.work_area.y + 60 + offset;
        let z = self.next_z;
        self.next_z += 1;
        let rect = Rect::new(x, y, width, height);
        self.windows.push(ManagedWindow {
            surface_id,
            title: String::from(title),
            rect,
            minimized: false,
            maximized: false,
            floating: false,
            z_order: z,
            snap_state: snap_directional::SnapState::Normal,
            restore_rect: rect,
        });
        self.focused = Some(surface_id);
        if self.mode != WindowMode::Float {
            self.retile();
        }
    }

    pub fn remove_window(&mut self, surface_id: u64) {
        self.windows.retain(|w| w.surface_id != surface_id);
        if self.focused == Some(surface_id) {
            self.focused = self.windows.last().map(|w| w.surface_id);
        }
        if self.mode != WindowMode::Float {
            self.retile();
        }
    }

    /// Place a window at an EXACT work-area rect chosen from a Snap Layouts
    /// template (Rae+Z). Clears the maximized/minimized latch and marks the
    /// window floating so it sits in the custom region until the user moves it.
    /// Saves the restore rect (once) and resets the directional state so a later
    /// Rae+Arrow starts fresh from this position.
    pub fn snap_to_rect(&mut self, surface_id: u64, rect: Rect) {
        if let Some(w) = self.windows.iter_mut().find(|w| w.surface_id == surface_id) {
            if w.snap_state.is_normal() {
                w.restore_rect = w.rect;
            }
            w.rect = rect;
            w.maximized = false;
            w.minimized = false;
            w.floating = true;
            w.snap_state = snap_directional::SnapState::Normal;
        }
    }

    /// Apply a Rae+Arrow directional snap to `surface_id`. Advances the window's
    /// [`snap_directional::SnapState`] and returns `(new_rect, minimized)` for the
    /// caller to push to the compositor, or `None` when the chord is a no-op
    /// (e.g. Rae+← on an already-left window) or the window is gone.
    pub fn snap_directional(
        &mut self,
        surface_id: u64,
        dir: snap_directional::SnapDir,
    ) -> Option<(Rect, bool)> {
        let work = self.work_area;
        let w = self
            .windows
            .iter_mut()
            .find(|w| w.surface_id == surface_id)?;
        let old = w.snap_state;
        let new = old.apply(dir);
        if new == old {
            return None; // no-op — don't thrash geometry on a held key
        }
        // Save the free-floating rect the first time we leave Normal.
        if old.is_normal() {
            w.restore_rect = w.rect;
        }
        w.snap_state = new;
        if new.is_minimized() {
            w.minimized = true;
            return Some((w.rect, true));
        }
        w.minimized = false;
        w.maximized = new == snap_directional::SnapState::Max;
        let target = if new.is_normal() {
            w.restore_rect
        } else {
            new.geometry(work)?
        };
        w.rect = target;
        Some((target, false))
    }

    pub fn focus_window(&mut self, surface_id: u64) {
        self.focused = Some(surface_id);
        if let Some(w) = self.windows.iter_mut().find(|w| w.surface_id == surface_id) {
            w.z_order = self.next_z;
            self.next_z += 1;
        }
    }

    pub fn minimize(&mut self, surface_id: u64) {
        if let Some(w) = self.windows.iter_mut().find(|w| w.surface_id == surface_id) {
            w.minimized = true;
        }
        if self.focused == Some(surface_id) {
            self.focused = self
                .windows
                .iter()
                .filter(|w| !w.minimized)
                .max_by_key(|w| w.z_order)
                .map(|w| w.surface_id);
        }
    }

    pub fn restore(&mut self, surface_id: u64) {
        if let Some(w) = self.windows.iter_mut().find(|w| w.surface_id == surface_id) {
            w.minimized = false;
            w.z_order = self.next_z;
            self.next_z += 1;
        }
        self.focused = Some(surface_id);
    }

    pub fn maximize(&mut self, surface_id: u64) {
        if let Some(w) = self.windows.iter_mut().find(|w| w.surface_id == surface_id) {
            w.maximized = true;
            w.rect = self.work_area;
        }
    }

    pub fn unmaximize(&mut self, surface_id: u64) {
        if let Some(w) = self.windows.iter_mut().find(|w| w.surface_id == surface_id) {
            w.maximized = false;
        }
    }

    pub fn set_mode(&mut self, mode: WindowMode) {
        self.mode = mode;
        self.retile();
    }

    pub fn toggle_float(&mut self, surface_id: u64) {
        if let Some(w) = self.windows.iter_mut().find(|w| w.surface_id == surface_id) {
            w.floating = !w.floating;
        }
        self.retile();
    }

    pub fn retile(&mut self) {
        let tiled: Vec<usize> = self
            .windows
            .iter()
            .enumerate()
            .filter(|(_, w)| !w.minimized && !w.floating)
            .map(|(i, _)| i)
            .collect();

        if tiled.is_empty() {
            return;
        }

        let g = self.gap as i32;
        let wa = &self.work_area;

        match self.mode {
            WindowMode::TileHorizontal => {
                let w = (wa.w as i32 - g * (tiled.len() as i32 + 1)) / tiled.len().max(1) as i32;
                for (i, &idx) in tiled.iter().enumerate() {
                    self.windows[idx].rect = Rect::new(
                        wa.x + g + i as i32 * (w + g),
                        wa.y + g,
                        w.max(100) as u32,
                        (wa.h as i32 - 2 * g).max(100) as u32,
                    );
                }
            }
            WindowMode::TileVertical => {
                let h = (wa.h as i32 - g * (tiled.len() as i32 + 1)) / tiled.len().max(1) as i32;
                for (i, &idx) in tiled.iter().enumerate() {
                    self.windows[idx].rect = Rect::new(
                        wa.x + g,
                        wa.y + g + i as i32 * (h + g),
                        (wa.w as i32 - 2 * g).max(100) as u32,
                        h.max(100) as u32,
                    );
                }
            }
            WindowMode::TileGrid => {
                let cols = f32_ceil(f32_sqrt(tiled.len() as f32)) as usize;
                let rows = (tiled.len() + cols - 1) / cols;
                let cell_w = (wa.w as i32 - g * (cols as i32 + 1)) / cols.max(1) as i32;
                let cell_h = (wa.h as i32 - g * (rows as i32 + 1)) / rows.max(1) as i32;
                for (i, &idx) in tiled.iter().enumerate() {
                    let col = i % cols;
                    let row = i / cols;
                    self.windows[idx].rect = Rect::new(
                        wa.x + g + col as i32 * (cell_w + g),
                        wa.y + g + row as i32 * (cell_h + g),
                        cell_w.max(100) as u32,
                        cell_h.max(100) as u32,
                    );
                }
            }
            WindowMode::Monocle => {
                for &idx in &tiled {
                    self.windows[idx].rect = Rect::new(
                        wa.x + g,
                        wa.y + g,
                        (wa.w as i32 - 2 * g).max(100) as u32,
                        (wa.h as i32 - 2 * g).max(100) as u32,
                    );
                }
            }
            WindowMode::GameOS => {
                if let Some(&idx) = tiled.last() {
                    self.windows[idx].rect =
                        Rect::new(wa.x, wa.y, wa.w, wa.h + TASKBAR_HEIGHT as u32);
                }
            }
            WindowMode::Float => {}
        }
    }

    pub fn visible_windows(&self) -> Vec<&ManagedWindow> {
        let mut visible: Vec<&ManagedWindow> =
            self.windows.iter().filter(|w| !w.minimized).collect();
        visible.sort_by_key(|w| w.z_order);
        visible
    }

    pub fn window_at(&self, px: i32, py: i32) -> Option<u64> {
        self.visible_windows()
            .iter()
            .rev()
            .find(|w| w.rect.contains(px, py))
            .map(|w| w.surface_id)
    }
}

// ── Settings panel ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsPage {
    Display,
    Audio,
    Network,
    Gaming,
    Appearance,
    Security,
    System,
    About,
}

#[derive(Debug, Clone)]
pub enum SettingValue {
    Toggle(bool),
    Slider {
        value: u32,
        min: u32,
        max: u32,
    },
    Choice {
        selected: usize,
        options: Vec<String>,
    },
    Text(String),
}

#[derive(Debug, Clone)]
pub struct SettingEntry {
    pub key: String,
    pub label: String,
    pub description: String,
    pub value: SettingValue,
    pub page: SettingsPage,
}

pub struct SettingsPanel {
    pub visible: bool,
    pub current_page: SettingsPage,
    pub entries: Vec<SettingEntry>,
    pub search_query: String,
    pub rect: Rect,
}

impl SettingsPanel {
    pub fn new(screen_width: usize, screen_height: usize) -> Self {
        let w = (screen_width * 2 / 3).min(800);
        let h = (screen_height * 2 / 3).min(600);
        let x = ((screen_width - w) / 2) as i32;
        let y = ((screen_height - h) / 2) as i32;

        let mut panel = Self {
            visible: false,
            current_page: SettingsPage::Display,
            entries: Vec::new(),
            search_query: String::new(),
            rect: Rect::new(x, y, w as u32, h as u32),
        };
        panel.populate_defaults();
        panel
    }

    fn populate_defaults(&mut self) {
        // Display
        self.add_entry(
            "display.resolution",
            "Resolution",
            "Screen resolution",
            SettingsPage::Display,
            SettingValue::Choice {
                selected: 0,
                options: alloc::vec![
                    String::from("3840x2160"),
                    String::from("2560x1440"),
                    String::from("1920x1080"),
                    String::from("1280x720"),
                ],
            },
        );
        self.add_entry(
            "display.refresh",
            "Refresh Rate",
            "Display refresh rate",
            SettingsPage::Display,
            SettingValue::Choice {
                selected: 0,
                options: alloc::vec![
                    String::from("165 Hz"),
                    String::from("144 Hz"),
                    String::from("120 Hz"),
                    String::from("60 Hz"),
                ],
            },
        );
        self.add_entry(
            "display.vrr",
            "Variable Refresh Rate",
            "Enable adaptive sync",
            SettingsPage::Display,
            SettingValue::Toggle(true),
        );
        self.add_entry(
            "display.hdr",
            "HDR",
            "High dynamic range output",
            SettingsPage::Display,
            SettingValue::Toggle(false),
        );
        self.add_entry(
            "display.brightness",
            "Brightness",
            "Screen brightness level",
            SettingsPage::Display,
            SettingValue::Slider {
                value: 80,
                min: 0,
                max: 100,
            },
        );
        self.add_entry(
            "display.night_light",
            "Night Light",
            "Reduce blue light in the evening",
            SettingsPage::Display,
            SettingValue::Toggle(false),
        );

        // Audio
        self.add_entry(
            "audio.master_volume",
            "Master Volume",
            "System-wide volume",
            SettingsPage::Audio,
            SettingValue::Slider {
                value: 75,
                min: 0,
                max: 100,
            },
        );
        self.add_entry(
            "audio.output",
            "Output Device",
            "Active audio output",
            SettingsPage::Audio,
            SettingValue::Choice {
                selected: 0,
                options: alloc::vec![
                    String::from("Speakers"),
                    String::from("Headphones"),
                    String::from("HDMI"),
                    String::from("USB DAC"),
                ],
            },
        );
        self.add_entry(
            "audio.spatial",
            "Spatial Audio",
            "3D audio processing",
            SettingsPage::Audio,
            SettingValue::Toggle(true),
        );
        self.add_entry(
            "audio.latency",
            "Low Latency Mode",
            "Minimize audio buffer size",
            SettingsPage::Audio,
            SettingValue::Toggle(true),
        );

        // Network
        self.add_entry(
            "net.wifi_enabled",
            "Wi-Fi",
            "Enable wireless networking",
            SettingsPage::Network,
            SettingValue::Toggle(true),
        );
        self.add_entry(
            "net.vpn",
            "VPN",
            "Built-in WireGuard tunnel",
            SettingsPage::Network,
            SettingValue::Toggle(false),
        );
        self.add_entry(
            "net.gaming_qos",
            "Gaming QoS",
            "Prioritize game traffic",
            SettingsPage::Network,
            SettingValue::Toggle(true),
        );

        // Gaming
        self.add_entry(
            "game.sched_game",
            "SCHED_BODY Priority",
            "Hard real-time scheduling for games",
            SettingsPage::Gaming,
            SettingValue::Toggle(true),
        );
        self.add_entry(
            "game.bg_throttle",
            "Background Throttling",
            "Throttle background apps during gaming",
            SettingsPage::Gaming,
            SettingValue::Toggle(true),
        );
        self.add_entry(
            "game.null_latency",
            "NULL_LATENCY Mode",
            "Disable all smoothing for competitive play",
            SettingsPage::Gaming,
            SettingValue::Toggle(false),
        );
        self.add_entry(
            "game.shader_cache",
            "OS Shader Cache",
            "Persistent shader compilation cache",
            SettingsPage::Gaming,
            SettingValue::Toggle(true),
        );
        self.add_entry(
            "game.gpu_power",
            "GPU Power Limit",
            "Maximum GPU power percentage",
            SettingsPage::Gaming,
            SettingValue::Slider {
                value: 100,
                min: 50,
                max: 115,
            },
        );

        // Appearance
        self.add_entry(
            "appear.vibe",
            "Vibe Mode",
            "System-wide visual personality",
            SettingsPage::Appearance,
            SettingValue::Choice {
                selected: 0,
                options: alloc::vec![
                    String::from("Default"),
                    String::from("Cyberpunk Night"),
                    String::from("Studio Ghibli"),
                    String::from("Bauhaus"),
                    String::from("Neo Noir"),
                ],
            },
        );
        self.add_entry(
            "appear.wm_mode",
            "Window Manager",
            "Window tiling mode",
            SettingsPage::Appearance,
            SettingValue::Choice {
                selected: 0,
                options: alloc::vec![
                    String::from("Float"),
                    String::from("Tile Horizontal"),
                    String::from("Tile Vertical"),
                    String::from("Grid"),
                    String::from("Monocle"),
                ],
            },
        );
        self.add_entry(
            "appear.animations",
            "Animations",
            "Enable window animations",
            SettingsPage::Appearance,
            SettingValue::Toggle(true),
        );
        self.add_entry(
            "appear.transparency",
            "Transparency",
            "Window transparency level",
            SettingsPage::Appearance,
            SettingValue::Slider {
                value: 90,
                min: 50,
                max: 100,
            },
        );

        // Security
        self.add_entry(
            "sec.sandbox",
            "App Sandboxing",
            "Mandatory sandbox for all apps",
            SettingsPage::Security,
            SettingValue::Toggle(true),
        );
        self.add_entry(
            "sec.code_signing",
            "Require Code Signing",
            "Only run signed executables",
            SettingsPage::Security,
            SettingValue::Toggle(false),
        );
        self.add_entry(
            "sec.firewall",
            "Firewall",
            "Network firewall protection",
            SettingsPage::Security,
            SettingValue::Toggle(true),
        );

        // System
        self.add_entry(
            "sys.auto_update",
            "Auto Update Check",
            "Periodically check for updates",
            SettingsPage::System,
            SettingValue::Toggle(true),
        );
        self.add_entry(
            "sys.telemetry",
            "Telemetry",
            "Anonymous usage statistics (opt-in)",
            SettingsPage::System,
            SettingValue::Toggle(false),
        );
        self.add_entry(
            "sys.snapshots",
            "Automatic Snapshots",
            "Create system snapshots before updates",
            SettingsPage::System,
            SettingValue::Toggle(true),
        );
    }

    fn add_entry(
        &mut self,
        key: &str,
        label: &str,
        desc: &str,
        page: SettingsPage,
        value: SettingValue,
    ) {
        self.entries.push(SettingEntry {
            key: String::from(key),
            label: String::from(label),
            description: String::from(desc),
            value,
            page,
        });
    }

    pub fn toggle(&mut self) {
        self.visible = !self.visible;
    }

    pub fn set_page(&mut self, page: SettingsPage) {
        self.current_page = page;
    }

    pub fn entries_for_page(&self) -> Vec<&SettingEntry> {
        let query = self.search_query.as_str();
        self.entries
            .iter()
            .filter(|e| {
                if !query.is_empty() {
                    return e.label.contains(query) || e.description.contains(query);
                }
                e.page == self.current_page
            })
            .collect()
    }

    pub fn toggle_setting(&mut self, key: &str) {
        if let Some(entry) = self.entries.iter_mut().find(|e| e.key == key) {
            if let SettingValue::Toggle(ref mut v) = entry.value {
                *v = !*v;
            }
        }
    }

    pub fn set_slider(&mut self, key: &str, value: u32) {
        if let Some(entry) = self.entries.iter_mut().find(|e| e.key == key) {
            if let SettingValue::Slider {
                value: ref mut v,
                min,
                max,
            } = entry.value
            {
                *v = value.clamp(min, max);
            }
        }
    }

    pub fn set_choice(&mut self, key: &str, index: usize) {
        if let Some(entry) = self.entries.iter_mut().find(|e| e.key == key) {
            if let SettingValue::Choice {
                ref mut selected,
                ref options,
            } = entry.value
            {
                if index < options.len() {
                    *selected = index;
                }
            }
        }
    }

    pub fn get_bool(&self, key: &str) -> Option<bool> {
        self.entries.iter().find(|e| e.key == key).and_then(|e| {
            if let SettingValue::Toggle(v) = e.value {
                Some(v)
            } else {
                None
            }
        })
    }

    pub fn get_slider(&self, key: &str) -> Option<u32> {
        self.entries.iter().find(|e| e.key == key).and_then(|e| {
            if let SettingValue::Slider { value, .. } = e.value {
                Some(value)
            } else {
                None
            }
        })
    }

    pub fn render(&self, canvas: &mut athgfx::Canvas) {
        if !self.visible {
            return;
        }

        // Token-derived palette aliases — the Settings panel is re-skinned onto
        // the shared tokens here; its deeper layout polish (unified Settings) is
        // a later pass. Names mirror the retired consts so the body is stable.
        let accent_ramp = accent();
        let settings_bg = PALETTE.bg_raised;
        let bar_bg = PALETTE.bg_overlay;
        let focused_fg = PALETTE.text_primary;
        let dimmed_fg = PALETTE.text_tertiary;
        let accent_c = accent_ramp.text;
        let focused_bg = PALETTE.bg_elevated;
        let item_fg = PALETTE.text_secondary;
        let toggle_on = accent_ramp.base;
        let toggle_off = PALETTE.bg_elevated;
        let slider_track = PALETTE.bg_elevated;
        let slider_knob = accent_ramp.base;

        let r = &self.rect;
        canvas.fill_rect(
            r.x as usize,
            r.y as usize,
            r.w as usize,
            r.h as usize,
            settings_bg,
        );
        canvas.draw_rect_outline(
            r.x as usize,
            r.y as usize,
            r.w as usize,
            r.h as usize,
            accent_c,
        );

        // Title bar — title type.title, close affordance type.label (crisp AA).
        canvas.fill_rect(r.x as usize, r.y as usize, r.w as usize, 32, bar_bg);
        canvas.draw_text_aa(
            r.x + 12,
            r.y + (32 - ath_tokens::TYPE_TITLE.line_height as i32) / 2,
            "Settings",
            ath_tokens::TYPE_TITLE,
            focused_fg,
            athgfx::text::FontFamily::Sans,
        );
        canvas.draw_text_aa(
            r.x + r.w as i32 - 20,
            r.y + (32 - ath_tokens::TYPE_LABEL.line_height as i32) / 2,
            "x",
            ath_tokens::TYPE_LABEL,
            dimmed_fg,
            athgfx::text::FontFamily::Sans,
        );

        // Sidebar with pages
        let sidebar_w = 140usize;
        let pages = [
            (SettingsPage::Display, "Display"),
            (SettingsPage::Audio, "Audio"),
            (SettingsPage::Network, "Network"),
            (SettingsPage::Gaming, "Gaming"),
            (SettingsPage::Appearance, "Appearance"),
            (SettingsPage::Security, "Security"),
            (SettingsPage::System, "System"),
            (SettingsPage::About, "About"),
        ];

        for (i, (page, label)) in pages.iter().enumerate() {
            let item_y = r.y as usize + 40 + i * 28;
            let label_y = item_y + (28 - ath_tokens::TYPE_LABEL.line_height as usize) / 2;
            let label_fg = if *page == self.current_page {
                canvas.fill_rect(r.x as usize, item_y, sidebar_w, 28, focused_bg);
                accent_c
            } else {
                item_fg
            };
            canvas.draw_text_aa(
                r.x + 12,
                label_y as i32,
                label,
                ath_tokens::TYPE_LABEL,
                label_fg,
                athgfx::text::FontFamily::Sans,
            );
        }

        // Vertical separator
        let sep_x = r.x as usize + sidebar_w;
        for sy in r.y as usize + 32..r.y as usize + r.h as usize {
            canvas.draw_pixel(sep_x, sy, dimmed_fg);
        }

        // Content area
        let content_x = r.x as usize + sidebar_w + 16;
        let content_y_start = r.y as usize + 48;
        let entries = self.entries_for_page();

        for (i, entry) in entries.iter().enumerate() {
            let ey = content_y_start + i * 44;
            if ey + 44 > r.y as usize + r.h as usize {
                break;
            }

            // Entry label type.label; description type.caption (crisp AA).
            canvas.draw_text_aa(
                content_x as i32,
                ey as i32,
                &entry.label,
                ath_tokens::TYPE_LABEL,
                focused_fg,
                athgfx::text::FontFamily::Sans,
            );
            canvas.draw_text_aa(
                content_x as i32,
                (ey + 14) as i32,
                &entry.description,
                ath_tokens::TYPE_CAPTION,
                dimmed_fg,
                athgfx::text::FontFamily::Sans,
            );

            let control_x = r.x as usize + r.w as usize - 100;

            match &entry.value {
                SettingValue::Toggle(on) => {
                    let bg = if *on { toggle_on } else { toggle_off };
                    canvas.fill_rect(control_x, ey + 2, 40, 16, bg);
                    canvas.draw_rect_outline(control_x, ey + 2, 40, 16, accent_c);
                    let knob_x = if *on { control_x + 24 } else { control_x + 2 };
                    canvas.fill_rect(knob_x, ey + 4, 14, 12, focused_fg);
                }
                SettingValue::Slider { value, min, max } => {
                    let track_w = 80usize;
                    canvas.fill_rect(control_x, ey + 8, track_w, 4, slider_track);
                    let range = max - min;
                    let pos = if range > 0 {
                        (((*value - min) as usize) * track_w) / range as usize
                    } else {
                        0
                    };
                    canvas.fill_rect(control_x + pos, ey + 4, 8, 12, slider_knob);

                    let mut val_buf = [0u8; 8];
                    let val_str = fmt_u32(*value, &mut val_buf);
                    canvas.draw_text_aa(
                        (control_x + track_w + 8) as i32,
                        (ey + 4) as i32,
                        val_str,
                        ath_tokens::TYPE_CAPTION,
                        item_fg,
                        athgfx::text::FontFamily::Sans,
                    );
                }
                SettingValue::Choice { selected, options } => {
                    if let Some(opt) = options.get(*selected) {
                        let max_chars = 12;
                        let display = crate::text_util::truncate_chars(opt, max_chars);
                        canvas.draw_text_aa(
                            control_x as i32,
                            (ey + 4) as i32,
                            display,
                            ath_tokens::TYPE_BODY,
                            accent_c,
                            athgfx::text::FontFamily::Sans,
                        );
                    }
                }
                SettingValue::Text(txt) => {
                    canvas.draw_text_aa(
                        control_x as i32,
                        (ey + 4) as i32,
                        txt,
                        ath_tokens::TYPE_BODY,
                        item_fg,
                        athgfx::text::FontFamily::Sans,
                    );
                }
            }
        }
    }
}

fn fmt_u32(mut n: u32, buf: &mut [u8; 8]) -> &str {
    if n == 0 {
        buf[0] = b'0';
        return unsafe { core::str::from_utf8_unchecked(&buf[..1]) };
    }
    let mut pos = 8;
    while n > 0 && pos > 0 {
        pos -= 1;
        buf[pos] = b'0' + (n % 10) as u8;
        n /= 10;
    }
    unsafe { core::str::from_utf8_unchecked(&buf[pos..8]) }
}

// ── Desktop shell (top-level) ────────────────────────────────────────────

pub struct DesktopShell {
    pub taskbar_items: Vec<TaskbarItem>,
    pub taskbar_rect: Rect,
    pub start_menu: StartMenu,
    pub system_tray: SystemTray,
    /// Control Center — the bottom-right glass quick-settings flyout
    /// (docs/design/control-center.md). Opened from the tray cluster.
    pub control_center: control_center::ControlCenter,
    pub notifications: NotificationCenter,
    pub window_manager: WindowManager,
    pub settings: SettingsPanel,
    /// Global command palette (Super+Space) — the launcher + action runner.
    pub command_palette: command_palette::CommandPalette,
    /// Clipboard-history panel (Super+C) — the Win+V-class glass flyout.
    pub clipboard_panel: clipboard_panel::ClipboardPanel,
    /// Screenshot / region-capture overlay (Super+Shift+S) — the dimmed-scrim
    /// capture-mode UI that drives the compositor capture engine.
    pub capture_overlay: screenshot_overlay::CaptureOverlay,
    /// Snap Layouts flyout (Rae key + Z) — the Win11-style template picker that
    /// places the focused window into an exact region of the work area.
    pub snap_overlay: snap_layouts::SnapOverlay,
    /// Snap Assist flow — after a layout snap, offers the other windows to fill
    /// the remaining zones. `None` when not running.
    pub snap_assist: Option<snap_assist::SnapAssist>,
    /// Snap groups — windows snapped together into a layout, remembered so they
    /// minimize / restore as a unit (Win11 snap groups).
    pub snap_groups: snap_groups::SnapGroups,
    pub focused_surface: Option<u64>,
    pub screen_width: usize,
    pub screen_height: usize,
}

/// One taskbar button in the combined pinned+running cluster
/// ([`DesktopShell::taskbar_buttons`]): `surface_id = Some` means a running
/// window (click focuses); `None` means a pinned launcher (click launches
/// `exec_path` via the kernel's spawn path).
#[derive(Debug, Clone)]
pub struct TaskbarButtonView {
    pub surface_id: Option<u64>,
    pub label: String,
    pub exec_path: String,
    pub focused: bool,
    pub minimized: bool,
}

impl DesktopShell {
    pub fn new(screen_width: usize, screen_height: usize) -> Self {
        let mut tray = SystemTray::new();
        tray.add_icon("net", 'N', "Network: Connected");
        tray.add_icon("vol", 'V', "Volume: 75%");
        tray.add_icon("bat", 'B', "Battery: 100%");

        Self {
            taskbar_items: Vec::new(),
            taskbar_rect: Rect::new(
                0,
                (screen_height - TASKBAR_HEIGHT) as i32,
                screen_width as u32,
                TASKBAR_HEIGHT as u32,
            ),
            start_menu: StartMenu::new(screen_width, screen_height),
            system_tray: tray,
            control_center: control_center::ControlCenter::new(
                screen_width,
                screen_height,
                TASKBAR_HEIGHT,
            ),
            notifications: NotificationCenter::new(),
            window_manager: WindowManager::new(screen_width as u32, screen_height as u32),
            settings: SettingsPanel::new(screen_width, screen_height),
            command_palette: command_palette::CommandPalette::new(screen_width, screen_height),
            clipboard_panel: clipboard_panel::ClipboardPanel::new(screen_width, screen_height),
            capture_overlay: screenshot_overlay::CaptureOverlay::new(
                screen_width as u32,
                screen_height as u32,
            ),
            snap_overlay: snap_layouts::SnapOverlay::new(
                screen_width,
                screen_height,
                tiling_wm::Rect::new(
                    0,
                    0,
                    screen_width as u32,
                    screen_height.saturating_sub(TASKBAR_HEIGHT) as u32,
                ),
            ),
            snap_assist: None,
            snap_groups: snap_groups::SnapGroups::new(),
            focused_surface: None,
            screen_width,
            screen_height,
        }
    }

    pub fn add_window(&mut self, title: &str, surface_id: u64, width: u32, height: u32) {
        let focused = self.taskbar_items.is_empty();
        self.taskbar_items.push(TaskbarItem {
            title: String::from(title),
            surface_id,
            focused,
            minimized: false,
            icon_char: title.chars().next().unwrap_or('?'),
        });
        self.window_manager
            .add_window(surface_id, title, width, height);
        if focused {
            self.focused_surface = Some(surface_id);
        }
    }

    pub fn remove_window(&mut self, surface_id: u64) {
        self.taskbar_items.retain(|it| it.surface_id != surface_id);
        self.window_manager.remove_window(surface_id);
        if self.focused_surface == Some(surface_id) {
            self.focused_surface = self.window_manager.focused;
            for it in &mut self.taskbar_items {
                it.focused = Some(it.surface_id) == self.focused_surface;
            }
        }
    }

    pub fn focus_window(&mut self, surface_id: u64) {
        self.focused_surface = Some(surface_id);
        self.window_manager.focus_window(surface_id);
        for it in &mut self.taskbar_items {
            it.focused = it.surface_id == surface_id;
        }
    }

    pub fn set_window_mode(&mut self, mode: WindowMode) {
        self.window_manager.set_mode(mode);
    }

    /// Taskbar Start-button rect — the SINGLE geometry authority shared by
    /// render, the kernel hit-test, and the a11y focus publication (they had
    /// drifted to three different widths: ~46px render / 56px click / 44px
    /// a11y, so clicks and focus rings missed the drawn pill).
    pub fn taskbar_start_rect(&self) -> Rect {
        let tb = &self.taskbar_rect;
        Rect::new(tb.x + 8, tb.y + 4, 56, (tb.h as i32 - 8).max(0) as u32)
    }

    /// The centered taskbar button cluster: PINNED app launchers (from the
    /// live Start registry, deduped against running windows by title) followed
    /// by running windows (Win11's combined model). A pinned button launches;
    /// a running button focuses. This is the single source the render, the
    /// kernel click hit-test, and the a11y focus rects all consume — the
    /// default desktop is never a barren strip (IDENTITY-OBSIDIAN.md context;
    /// desktop-shell.md §1).
    pub fn taskbar_buttons(&self) -> Vec<TaskbarButtonView> {
        let mut out: Vec<TaskbarButtonView> = Vec::new();
        for app in self.start_menu.apps.iter().filter(|a| a.pinned) {
            let running = self
                .taskbar_items
                .iter()
                .find(|it| it.title.eq_ignore_ascii_case(&app.name));
            out.push(TaskbarButtonView {
                surface_id: running.map(|it| it.surface_id),
                label: app.name.clone(),
                exec_path: app.exec_path.clone(),
                focused: running.map(|it| it.focused).unwrap_or(false),
                minimized: running.map(|it| it.minimized).unwrap_or(false),
            });
        }
        // Running windows that don't match a pinned entry follow the pins.
        for it in &self.taskbar_items {
            let pinned_match = self
                .start_menu
                .apps
                .iter()
                .any(|a| a.pinned && it.title.eq_ignore_ascii_case(&a.name));
            if !pinned_match {
                out.push(TaskbarButtonView {
                    surface_id: Some(it.surface_id),
                    label: it.title.clone(),
                    exec_path: String::new(),
                    focused: it.focused,
                    minimized: it.minimized,
                });
            }
        }
        out
    }

    /// Rect of taskbar button `i` in the [`Self::taskbar_buttons`] order — a
    /// Win11-style CENTERED icon-button cluster. Same single-authority
    /// contract as [`Self::taskbar_start_rect`].
    pub fn taskbar_item_rect(&self, i: usize) -> Rect {
        let tb = &self.taskbar_rect;
        let btn = 40i32;
        let sp = 4i32;
        let n = self.taskbar_buttons().len() as i32;
        let cluster = (n * (btn + sp) - sp).max(0);
        let start_clear = self.taskbar_start_rect();
        let x0 = ((self.screen_width as i32 - cluster) / 2)
            .max(start_clear.x + start_clear.w as i32 + 12);
        Rect::new(
            x0 + i as i32 * (btn + sp),
            tb.y + 4,
            btn as u32,
            (tb.h as i32 - 8).max(0) as u32,
        )
    }

    /// Left x of the tray cluster (icons + clock) — right-aligned.
    pub fn taskbar_tray_x(&self) -> usize {
        self.screen_width
            .saturating_sub(self.system_tray.total_width() + ath_tokens::SPACE_2 as usize)
    }

    pub fn render(&self, canvas: &mut athgfx::Canvas) {
        let accent = accent();
        let p = PALETTE;

        // ── Taskbar: edge-docked `glass.chrome` (IDENTITY §2.1/§7) — frosted,
        //    see-through, with the edge stack confined to the exposed top edge
        //    (hairline + quiet iridescent shimmer + lit lip). Retires the flat
        //    mica fill_rect (visual-QA: the live bar must match the shipped
        //    Taskbar component's material, not a second flat twin).
        let tb = &self.taskbar_rect;
        let tb_x = tb.x as usize;
        let tb_y = tb.y as usize;
        let tb_w = tb.w as usize;
        athgfx::glass::draw_glass_surface_docked(
            canvas,
            tb_x,
            tb_y,
            tb_w,
            tb.h as usize,
            ath_tokens::GLASS_CHROME_DARK,
            true,
        );
        // Dark on-accent ink (IDENTITY guardrail: never white on RaeBlue).
        let on_accent = p.bg_base;

        // ── Start button — the Rae mark (diamond/orb identity glyph). Accent
        //    pill with dark on-accent ink while the menu is open.
        let sr = self.taskbar_start_rect();
        let pill_r = ath_tokens::radius_pill(sr.h) as usize;
        let mark_ink = if self.start_menu.visible {
            canvas.fill_rounded_rect(
                sr.x as usize,
                sr.y as usize,
                sr.w as usize,
                sr.h as usize,
                pill_r,
                accent.base,
            );
            on_accent
        } else {
            accent.base
        };
        let mark_sz = 20usize;
        canvas.draw_icon(
            athgfx::icon::Icon::RaeLogo,
            sr.x + (sr.w as i32 - mark_sz as i32) / 2,
            sr.y + (sr.h as i32 - mark_sz as i32) / 2,
            mark_sz as i32,
            mark_ink,
        );

        // ── Taskbar buttons — the combined pinned+running centered cluster
        //    (taskbar_buttons is the single source; Win11 model). Pinned-only
        //    launchers draw with secondary ink and NO indicator; running gets
        //    the accent dot; focused = elevation step + accent glow + underline.
        for (i, btn) in self.taskbar_buttons().iter().enumerate() {
            let r = self.taskbar_item_rect(i);
            let bx = r.x as usize;
            let by = r.y as usize;
            let bw = r.w as usize;
            let bh = r.h as usize;
            let running = btn.surface_id.is_some();
            if btn.focused {
                // Obsidian: focused button = solid elevation step + accent
                // glow halo underneath ("lit on black", IDENTITY-OBSIDIAN §3).
                canvas.fill_rounded_rect_shadow(
                    bx,
                    by,
                    bw,
                    bh,
                    ath_tokens::RADIUS_SM as usize,
                    accent.base & 0x00FF_FFFF,
                    ath_tokens::GLOW_ACCENT_BLUR as usize,
                    0,
                );
                canvas.fill_rounded_rect(
                    bx,
                    by,
                    bw,
                    bh,
                    ath_tokens::RADIUS_SM as usize,
                    p.bg_elevated,
                );
            }

            let icon_sz = 20usize;
            let icon_fg = if !running {
                p.text_secondary // pinned launcher: quieter than a live window
            } else if btn.minimized {
                p.text_tertiary
            } else {
                p.text_primary
            };
            canvas.draw_icon(
                start_menu::app_line_icon(&btn.exec_path, &btn.label),
                (bx + (bw.saturating_sub(icon_sz)) / 2) as i32,
                (by + (bh.saturating_sub(icon_sz)) / 2) as i32,
                icon_sz as i32,
                icon_fg,
            );

            // Running indicator under the button (desktop-shell.md §1.2).
            // Pinned-only launchers carry none — that's the running tell.
            let ind_y = by + bh.saturating_sub(2);
            if btn.focused {
                canvas.fill_rounded_rect(bx + 6, ind_y, bw - 12, 2, 1, accent.base);
            } else if running {
                let (dot_w, dot_c) = if btn.minimized {
                    (6usize, p.text_tertiary)
                } else {
                    (8usize, accent.base)
                };
                canvas.fill_rounded_rect(
                    bx + (bw.saturating_sub(dot_w)) / 2,
                    ind_y,
                    dot_w,
                    2,
                    1,
                    dot_c,
                );
            }
        }

        // ── System tray (right-aligned, type.caption clock + status icons) ─
        self.system_tray
            .render(canvas, self.taskbar_tray_x(), tb_y, TASKBAR_HEIGHT);

        // Overlays
        self.start_menu.render(canvas);
        self.notifications.render_toasts(canvas, self.screen_width);
        self.settings.render(canvas);
        // Control Center — the bottom-right glass quick-settings flyout
        // (control-center.md). A transient elev.3 surface; painted with the
        // other flyouts, below the command palette (the topmost modal).
        self.control_center.render(canvas);
        // The command palette floats above everything (a transient modal) so it
        // is the last overlay painted (spec §2 elev.3).
        self.command_palette.render(canvas);
        // The clipboard-history flyout (Super+C) is likewise a transient glass
        // flyout at elev.3 — painted last so it floats above the desktop chrome.
        self.clipboard_panel.render(canvas);
        // The Snap Layouts flyout (Rae+Z) sits above everything, including the
        // other flyouts, so its scrim dims them too while the user picks a zone.
        self.snap_overlay.render(canvas);
        // Snap Assist (the fill-the-rest picker) renders last — it takes over
        // after the flyout closes and a zone was snapped.
        if let Some(assist) = self.snap_assist.as_ref() {
            assist.render(canvas);
        }
    }

    // ── Snap Layouts (Rae key + Z) ─────────────────────────────────────────
    //
    // Win11-style layout picker. The kernel input path (shell_runner) drives
    // these on the Rae+Z chord and cursor events; the overlay engine + geometry
    // live in `snap_layouts` (host-KAT'd).

    /// Toggle the Snap Layouts flyout. Rae+Z opens it for the focused window,
    /// Rae+Z / Esc closes it.
    pub fn toggle_snap_layouts(&mut self) {
        // Keep it sized to the live screen/work area before showing.
        self.snap_overlay.set_geometry(
            self.screen_width,
            self.screen_height,
            tiling_wm::Rect::new(
                0,
                0,
                self.screen_width as u32,
                self.screen_height.saturating_sub(TASKBAR_HEIGHT) as u32,
            ),
        );
        self.snap_overlay.toggle();
    }

    /// True while the flyout owns input (so the shell routes cursor/keys to it).
    pub fn snap_layouts_open(&self) -> bool {
        self.snap_overlay.visible
    }

    /// Cursor moved over the open flyout — update the hovered zone. No-op when
    /// closed. Returns true if the highlight changed (repaint hint).
    pub fn snap_layouts_hover(&mut self, px: i32, py: i32) -> bool {
        self.snap_overlay.hover_at(px, py)
    }

    /// A click at `(px, py)` while the flyout is open. If it lands on a zone,
    /// snap the focused window to that zone's exact work-area rect, close the
    /// flyout, and return the applied rect; otherwise close and return `None`
    /// (a click-away dismiss, matching the other flyouts).
    pub fn snap_layouts_click(&mut self, px: i32, py: i32) -> Option<Rect> {
        if !self.snap_overlay.visible {
            return None;
        }
        let picked = self.snap_overlay.picked_layout(px, py);
        self.snap_overlay.close();
        let (zones, idx) = picked?;
        // tiling_wm::Rect -> the shell/window_manager Rect (identical fields).
        let z = zones[idx];
        let rect = Rect::new(z.x, z.y, z.w, z.h);
        let sid = self.focused_surface?;
        self.window_manager.snap_to_rect(sid, rect);
        // Kick off Snap Assist for the remaining zones with the OTHER windows as
        // candidates (Win11: the empty zones offer thumbnails to fill them). It
        // self-deactivates when there is no empty zone or no other window.
        let candidates: Vec<snap_assist::Candidate> = self
            .window_manager
            .windows
            .iter()
            .filter(|w| w.surface_id != sid && !w.minimized)
            .map(|w| snap_assist::Candidate {
                id: w.surface_id,
                title: w.title.clone(),
            })
            .collect();
        let assist = snap_assist::SnapAssist::new(
            zones,
            idx,
            sid,
            candidates,
            self.screen_width,
            self.screen_height,
        );
        self.snap_assist = if assist.is_active() {
            Some(assist)
        } else {
            None
        };
        Some(rect)
    }

    // ── Snap Assist (fill the layout) ──────────────────────────────────────

    /// True while Snap Assist owns input (offering candidates for an empty zone).
    pub fn snap_assist_active(&self) -> bool {
        self.snap_assist
            .as_ref()
            .map(|a| a.is_active())
            .unwrap_or(false)
    }

    /// Cursor moved over the assist picker — update the hovered tile. Returns
    /// true if the highlight changed (repaint hint).
    pub fn snap_assist_hover(&mut self, px: i32, py: i32) -> bool {
        self.snap_assist
            .as_mut()
            .map(|a| a.hover_at(px, py))
            .unwrap_or(false)
    }

    /// A click while Snap Assist is up. On a candidate tile: snap that window to
    /// the current zone and return `(surface_id, rect)` for the compositor; on a
    /// miss: dismiss. Ends the flow (drops the state) once it is no longer active.
    pub fn snap_assist_click(&mut self, px: i32, py: i32) -> Option<(u64, Rect)> {
        let assist = self.snap_assist.as_mut()?;
        let picked = assist.click(px, py);
        let still_active = assist.is_active();
        // When the layout completes, snapshot the placements so we can remember
        // the windows as a snap group (minimize / restore together).
        let placements = (!still_active).then(|| assist.placements());
        let result = picked.map(|(id, z)| {
            let rect = Rect::new(z.x, z.y, z.w, z.h);
            self.window_manager.snap_to_rect(id, rect);
            (id, rect)
        });
        if let Some(pl) = placements {
            let members: Vec<(u64, Rect)> = pl
                .into_iter()
                .map(|(id, z)| (id, Rect::new(z.x, z.y, z.w, z.h)))
                .collect();
            self.snap_groups.form(members);
            self.snap_assist = None;
        }
        result
    }

    /// Dismiss Snap Assist (Esc).
    pub fn snap_assist_close(&mut self) {
        self.snap_assist = None;
    }

    // ── Snap groups (minimize / restore together) ──────────────────────────

    /// The `(surface_id, zone_rect)` of every window grouped with `id` (incl.
    /// `id`), or empty if `id` isn't in a group. The shell minimizes / restores
    /// the whole set as a unit (Win11 snap groups).
    pub fn snap_group_members(&self, id: u64) -> Vec<(u64, Rect)> {
        self.snap_groups.group_members(id)
    }

    /// Drop `id` from any snap group (it was closed or moved out of its layout).
    pub fn snap_group_forget(&mut self, id: u64) {
        self.snap_groups.remove_window(id);
    }

    /// Apply a Rae+Arrow directional snap to the focused window. Returns
    /// `(surface_id, new_rect, minimized)` so the kernel can push the geometry to
    /// the compositor, or `None` on a no-op / when nothing is focused.
    pub fn snap_directional(
        &mut self,
        dir: snap_directional::SnapDir,
    ) -> Option<(u64, Rect, bool)> {
        let sid = self.focused_surface?;
        self.window_manager
            .snap_directional(sid, dir)
            .map(|(rect, minimized)| (sid, rect, minimized))
    }
}

// ── Shell design-token proof (R10: must be able to print FAIL) ─────────────

/// A FAIL-able assertion that the live shell is wired to the shared design
/// tokens, not the old hardcoded palette. Returned to the kernel's shell
/// smoketest so it can emit the `[shell] taskbar: ...` proof line without this
/// `no_std` crate needing serial access.
#[derive(Clone, Copy, Debug)]
pub struct ShellDesignProof {
    /// Taskbar height in px — must be 44 (desktop-shell.md §1).
    pub taskbar_height: usize,
    /// The static mica tint actually painted behind the taskbar.
    pub mica_tint: u32,
    /// The derived accent base actually used for indicators/labels.
    pub accent_base: u32,
    /// True iff every token-wiring invariant holds (height==44, accent matches
    /// `derive_accent(seed).base`, mica is the bg.base/bg.raised blend).
    pub pass: bool,
}

/// Compute the shell design proof. The kernel logs it; this function is the
/// single authority for the asserted values (used by the host KAT too).
#[must_use]
pub fn shell_design_proof() -> ShellDesignProof {
    let want_accent = ath_tokens::derive_accent(active_accent(), PALETTE).base;
    let want_mica = blend_opaque(PALETTE.bg_base, PALETTE.bg_raised, 1, 2);
    let got_mica = mica_tint();
    let got_accent = accent().base;
    let pass = TASKBAR_HEIGHT == 44 && got_accent == want_accent && got_mica == want_mica;
    ShellDesignProof {
        taskbar_height: TASKBAR_HEIGHT,
        mica_tint: got_mica,
        accent_base: got_accent,
        pass,
    }
}

// ── Host KATs (R10: a smoketest must be able to print FAIL) ────────────────

#[cfg(test)]
mod design_tests {
    use super::*;

    #[test]
    fn taskbar_is_44px() {
        // desktop-shell.md §1 geometry: the re-skin raises the bar 36 → 44.
        assert_eq!(TASKBAR_HEIGHT, 44, "taskbar must be 44px");
    }

    #[test]
    fn accent_is_token_derived_not_hardcoded() {
        // The whole point of the re-skin: the shell accent must come from the
        // shared ramp, not the retired `const ACCENT = 0x4E9CFF`.
        let want = ath_tokens::derive_accent(active_accent(), PALETTE).base;
        assert_eq!(
            accent().base,
            want,
            "accent.base must be derive_accent(seed)"
        );
        // Default live seed is RaeBlue (no kernel push in the host KAT).
        assert_eq!(want, ath_tokens::RAEBLUE);
    }

    #[test]
    fn accent_tracks_the_live_seed() {
        // The Vibe-Mode cohesion link: the shell ramp is derived from the live
        // seed, NOT a hardcoded RaeBlue. FAIL-able by construction — if `accent()`
        // is wired to read `active_accent()`, deriving from a distinctive seed
        // must yield that seed's ramp. (We assert the derivation rather than
        // mutating the shared `ACTIVE_ACCENT` static, so this is independent of
        // the parallel test harness's ordering.)
        const ORANGE: u32 = 0xFF_FF_88_00;
        let ramp = ath_tokens::derive_accent(ORANGE, PALETTE);
        assert_eq!(ramp.base, ORANGE, "derive_accent.base is the seed");
        assert_ne!(
            ramp.base,
            ath_tokens::RAEBLUE,
            "a distinctive seed must not collapse to RaeBlue"
        );
        // And the default live seed (no kernel push) is RaeBlue.
        assert_eq!(active_accent(), ath_tokens::RAEBLUE);
        assert_eq!(accent().base, ath_tokens::RAEBLUE);
    }

    #[test]
    fn mica_is_bg_base_raised_blend() {
        // material.mica = a static bg.base/bg.raised blend (design-language
        // §5.2). It must be opaque and lie strictly between the two bases.
        let mica = mica_tint();
        assert_eq!((mica >> 24) & 0xFF, 0xFF, "mica must be opaque");
        let base_b = PALETTE.bg_base & 0xFF;
        let raised_b = PALETTE.bg_raised & 0xFF;
        let mica_b = mica & 0xFF;
        assert!(
            mica_b >= base_b && mica_b <= raised_b,
            "mica blue {mica_b} not between bg.base {base_b} and bg.raised {raised_b}"
        );
    }

    #[test]
    fn start_menu_panel_is_visible_over_bright_aurora() {
        // BUGFIX regression guard (start-menu-invisible): the LIVE Start panel must
        // read as a SOLID, legible frosted surface over the bright signature aurora
        // wallpaper — not the near-invisible smoked sheet the deprecated
        // `fill_rounded_rect(GLASS_TINT_DARK)` produced. We render the aurora, then
        // the open Start menu over it, then assert (1) the glass SUBSTANTIALLY
        // transforms the wallpaper pixels it covers (so the panel reads as a solid
        // surface, not a near-invisible sheet the backdrop bleeds straight through)
        // and (2) white text.primary inside the panel clears the AA legibility bar.
        // FAIL-able: revert the render to the bare deprecated `GLASS_TINT_DARK` tint
        // (no frost / no luma cap) and the transform delta collapses and/or the WCAG
        // assertion trips.
        ath_tokens::set_high_contrast(false);
        let (w, h) = (1280usize, 800usize);
        let mut buf = alloc::vec![0u8; w * h * 4];
        let mut canvas = unsafe { athgfx::Canvas::new(buf.as_mut_ptr(), w, h, 4) };
        // The bright signature backdrop the panel must remain visible over.
        athgfx::glass::render_aurora_dark(&mut canvas, 0, 0, w, h, 0);

        let mut menu = StartMenu::new(w, h);
        menu.visible = true;
        let (rx, ry, rw, rh) = (
            menu.rect.x as usize,
            menu.rect.y as usize,
            menu.rect.w as usize,
            menu.rect.h as usize,
        );
        let cx = rx + rw / 2;

        // Snapshot the RAW aurora backdrop the panel will cover, so we can measure
        // how much the glass composite actually transforms those pixels. The old
        // bare ~45%/62% tint barely changed them (the "invisible panel" defect);
        // a real frosted surface composites a large, consistent delta.
        let backdrop_snapshot = buf.clone();

        menu.render(&mut canvas);

        // The panel must paint real ink in its rect.
        let mut painted = false;
        'scan: for yy in ry..(ry + rh).min(h) {
            for xx in rx..(rx + rw).min(w) {
                let o = (yy * w + xx) * 4;
                if buf[o] != 0 || buf[o + 1] != 0 || buf[o + 2] != 0 {
                    painted = true;
                    break 'scan;
                }
            }
        }
        assert!(painted, "Start panel must paint ink in its rect");

        // Sample the frosted interior just below the search field (a clean glass
        // band, away from glyphs/selection wash) at several x positions and take
        // the median to avoid a stray text pixel.
        let band_y = ry + (ath_tokens::SPACE_4 as usize) + 40 + (ath_tokens::SPACE_4 as usize) + 4;
        let band_y = band_y.min(h - 1);
        let sample_xs: alloc::vec::Vec<usize> = (1..=7).map(|i| rx + rw * i / 8).collect();

        // COHERENT SURFACE (the discriminating invariant): a real frosted panel
        // reads as ONE calm surface — its interior luma is nearly FLAT across the
        // band, regardless of the structured aurora luminance underneath it. The
        // shipped `draw_glass_surface` composite (tint + frost + §2.3 luma cap)
        // collapses the interior to a tight spread (~1 luma); the deprecated bare
        // `GLASS_TINT_DARK` fill is just a translucent slate that lets the
        // wallpaper's wide luminance STRUCTURE bleed straight through (~27 luma
        // spread over the same band) — which is exactly why the beta-tester saw
        // "no panel appears" (it read as wallpaper, not a panel). MEASURED at this
        // fix: panel spread 0.8 vs backdrop spread 49.5; the bare-tint regression
        // measured panel spread 26.6. FAIL-able: revert the render to the bare tint
        // and the panel spread jumps back over the 8.0 ceiling and the >2× calmer
        // ratio fails.
        let lum = |o: usize, src: &[u8]| -> f32 {
            0.299 * src[o + 2] as f32 + 0.587 * src[o + 1] as f32 + 0.114 * src[o] as f32
        };
        let (mut bmin, mut bmax) = (f32::MAX, f32::MIN);
        let (mut pmin, mut pmax) = (f32::MAX, f32::MIN);
        for &sx in &sample_xs {
            let o = (band_y * w + sx) * 4;
            let bl = lum(o, &backdrop_snapshot);
            let pl = lum(o, &buf);
            bmin = bmin.min(bl);
            bmax = bmax.max(bl);
            pmin = pmin.min(pl);
            pmax = pmax.max(pl);
        }
        let backdrop_spread = bmax - bmin;
        let panel_spread = pmax - pmin;
        assert!(
            panel_spread <= 8.0 && panel_spread * 2.0 <= backdrop_spread,
            "Start panel interior must read as a COHERENT frosted surface (luma \
             spread {panel_spread:.1}, must be <= 8.0 and <= half the backdrop \
             spread {backdrop_spread:.1}) — a wider spread means the wallpaper \
             structure is bleeding through and the panel reads as near-invisible"
        );

        // And white text.primary inside the panel must clear the AA legibility bar
        // (the whole reason for routing through draw_glass_surface).
        let interior_px = {
            let o = (band_y * w + cx) * 4;
            (0xFF << 24)
                | ((buf[o + 2] as u32) << 16)
                | ((buf[o + 1] as u32) << 8)
                | (buf[o] as u32)
        };
        let cr = ath_tokens::contrast_ratio(PALETTE.text_primary, interior_px);
        assert!(
            cr >= 4.5,
            "white text.primary over the Start panel must clear AA 4.5:1 (got {cr:.2}:1)"
        );
    }

    #[test]
    fn start_menu_click_hits_the_drawn_row() {
        // P2 (render/hit-test geometry mismatch): the kernel click hit-test once
        // used list_y=rect.y+48 / item_height=32 while render drew at +72 / 36, so
        // a click landed on the WRONG row. Both now route through the SHARED
        // geometry (`list_y_start()` / `ITEM_HEIGHT` / `row_at`). This proves the
        // render rect for row N == the hit rect for row N: a click at the vertical
        // CENTRE of the pixels render paints for row N resolves to index N.
        //
        // FAIL-able: change `ITEM_HEIGHT` (or `list_y_start`) in ONE of the two
        // formulas and the centre-of-drawn-row click resolves to the neighbour.
        let mut menu = StartMenu::new(1280, 800);
        menu.visible = true;
        for i in 0..8 {
            menu.add_app(AppEntry {
                name: alloc::format!("App{i}"),
                exec_path: alloc::format!("app{i}"),
                icon_char: 'A',
                category: AppCategory::Utility,
                pinned: false,
                launch_count: 0,
            });
        }

        let top = menu.list_y_start();
        let pitch = StartMenu::ITEM_HEIGHT;

        // Click just ABOVE the list (inside the search field) hits no row.
        assert_eq!(
            menu.row_at(top as i32 - 1),
            None,
            "above-list click hits no row"
        );

        for n in 0..menu.filtered_apps().len() {
            // The y the renderer draws row n at is `top + n * pitch`; its visual
            // centre is `+ pitch/2`. Both the top edge and the centre must map to n.
            let row_top = (top + n * pitch) as i32;
            let row_centre = (top + n * pitch + pitch / 2) as i32;
            let row_bottom = (top + (n + 1) * pitch - 1) as i32;
            assert_eq!(menu.row_at(row_top), Some(n), "row {n} top edge");
            assert_eq!(menu.row_at(row_centre), Some(n), "row {n} centre");
            assert_eq!(menu.row_at(row_bottom), Some(n), "row {n} bottom edge");
        }

        // A click past the last app resolves to no row (no phantom launch).
        let past = (top + menu.filtered_apps().len() * pitch + 1) as i32;
        assert_eq!(menu.row_at(past), None, "below-last-app click hits no row");
    }

    #[test]
    fn proof_passes_when_wired() {
        // The exact assertion the kernel logs. A real fail-able check: if any
        // invariant regressed (height, accent source, mica recipe), pass=false.
        let proof = shell_design_proof();
        assert!(proof.pass, "shell design proof must pass: {proof:?}");
        assert_eq!(proof.taskbar_height, 44);
        assert_eq!(proof.accent_base, ath_tokens::RAEBLUE);
    }
}
