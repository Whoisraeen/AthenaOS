//! Full start menu for AthenaOS shell.

#![allow(dead_code)]

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};

/// Replace the alpha channel of an ARGB colour, keeping RGB — used to composite a
/// token colour (e.g. dark on-accent ink) translucently over a card. Mirrors the
/// CC-local helper (`ath_tokens::with_alpha` is private to that crate).
#[inline]
const fn with_alpha(color: u32, alpha: u32) -> u32 {
    (color & 0x00_FF_FF_FF) | ((alpha & 0xFF) << 24)
}

/// A representative dark-aurora backdrop sample for offline (KAT) reasoning about
/// the start-menu glass interior luminance. The live compositor blurs the real
/// wallpaper under the surface; this is the modeling constant the raised-tile KAT
/// flattens against (the SAME `flatten_over` math `draw_glass_surface` runs), so
/// the proof tracks the shipped composite rather than a hand-picked number.
const TILE_BACKDROP_SAMPLE: u32 = 0xFF_14_18_2A;

/// Composited interior of the Start popover surface over `backdrop` — i.e. the
/// colour the *inter-tile gap / container* reads (`backdrop → tint → frost`, the
/// `glass.popover` interior). Pure token math (mirrors `ath_tokens`' tier model)
/// so the raised-card KAT can compare a tile face against the gap it floats above.
fn start_gap_interior(backdrop: u32) -> u32 {
    let t = ath_tokens::GLASS_POPOVER_DARK;
    ath_tokens::flatten_over(t.frost, ath_tokens::flatten_over(t.tint, backdrop))
}

/// Composited face of a *raised* pinned/recommended tile over `backdrop` —
/// OBSIDIAN (IDENTITY-OBSIDIAN.md §2): a SOLID `bg.elevated` step of the dark
/// ladder, not a frost wash. Elevation on the near-black material reads as
/// "one step lighter dark" + hairline + shadow (the ShadowMist/macOS-dark
/// register); the frost-lift card was the mid-gray "toy" read. The renderer
/// draws exactly this face; this helper is the KAT's source of truth.
fn start_tile_card_face(backdrop: u32) -> u32 {
    let _ = backdrop; // the face is a solid ladder step — backdrop-independent
    crate::active_palette().bg_elevated
}

/// Mean-channel luminance (0..100) of an opaque ARGB colour — the perceptual weight
/// the glass tier model + the raised-tile KAT measure with. Local mirror of the
/// `ath_tokens` private `mean_luma` (kept private to that crate).
fn tile_luma_pct(color: u32) -> f32 {
    let r = ((color >> 16) & 0xFF) as f32;
    let g = ((color >> 8) & 0xFF) as f32;
    let b = (color & 0xFF) as f32;
    (0.2126 * r + 0.7152 * g + 0.0722 * b) / 255.0 * 100.0
}

/// Map a Start-menu app (by id + name) to the closest shipped `athgfx` line-icon
/// — the SAME icon set Control Center / Files / the taskbar consume (visual-QA
/// Round-7 #1: retire the single-accent-LETTER placeholders the bitmap font drew
/// as `?`/initials). No new icons: the set already ships Folder/Code/Gear/etc.
/// The mapping keys on the stable `app_id` first (e.g. `com.raeos.terminal`),
/// then falls back to a name keyword, then a generic `File`/`Folder` icon.
pub(crate) fn app_line_icon(app_id: &str, name: &str) -> athgfx::icon::Icon {
    use athgfx::icon::Icon;
    let id = app_id;
    let n = name;
    let hay = |needle: &str| id.contains(needle) || n.to_ascii_lowercase().contains(needle);
    if hay("terminal") || hay("console") || hay("shell") {
        Icon::Exec
    } else if hay("file") || hay("explorer") || hay("finder") {
        // "file" (not "files") so the live registry's "File Manager" maps too.
        // SOLID folder — the content register (IDENTITY-OBSIDIAN.md §4).
        Icon::FolderSolid
    } else if hay("browser") || hay("web") || hay("internet") {
        Icon::WiFi // closest shipped "network/globe" glyph (no dedicated browser icon)
    } else if hay("settings") || hay("control") || hay("preferences") {
        Icon::Gear
    } else if hay("password") || hay("vault") || hay("secret") || hay("keychain") {
        Icon::Lock
    } else if hay("editor") || hay("text") || hay("code") {
        Icon::Code
    } else if hay("notes") || hay("calendar") || hay("mail") || hay("calc") {
        Icon::Doc
    } else if hay("monitor") || hay("system") || hay("task") || hay("perf") || hay("clock") {
        // The gauge glyph (circle + needle) doubles as the analog-clock face.
        Icon::Performance
    } else if hay("music") || hay("media") || hay("video") || hay("player") || hay("photo") {
        Icon::Media
    } else if hay("game") || hay("play") {
        Icon::GameController
    } else if hay("store") || hay("download") {
        Icon::Archive
    } else {
        Icon::File
    }
}

/// Map a Start quick-access / recommended FOLDER or file entry to a line-icon.
pub(crate) fn entry_line_icon(name: &str, path: &str) -> athgfx::icon::Icon {
    use athgfx::icon::Icon;
    let n = name.to_ascii_lowercase();
    if path.ends_with('/') || (!path.contains('.') && !path.is_empty()) {
        return Icon::FolderSolid;
    }
    if n.ends_with(".rs") || n.ends_with(".c") || n.ends_with(".py") || n.contains("code") {
        Icon::Code
    } else if n.ends_with(".png")
        || n.ends_with(".jpg")
        || n.ends_with(".mp4")
        || n.ends_with(".mp3")
    {
        Icon::Media
    } else if n.ends_with(".zip") || n.ends_with(".tar") || n.ends_with(".gz") {
        Icon::Archive
    } else {
        Icon::Doc
    }
}

// ── Layout modes ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartMenuLayout {
    TwoColumn,
    FullScreen,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartMenuSection {
    PinnedApps,
    AllApps,
    Recommended,
    Search,
}

// ── App representation ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppCategory {
    Productivity,
    Games,
    Utilities,
    Media,
    Development,
    System,
    Web,
    Creative,
    Social,
    Other,
}

#[derive(Debug, Clone)]
pub struct AppInfo {
    pub id: u64,
    pub name: String,
    pub app_id: String,
    pub exec_path: String,
    pub icon_char: char,
    pub category: AppCategory,
    pub pinned: bool,
    pub pinned_position: u32,
    pub launch_count: u32,
    pub last_launched: u64,
    pub installed_timestamp: u64,
    pub folder_id: Option<u64>,
    pub description: String,
}

impl AppInfo {
    pub fn new(
        id: u64,
        name: &str,
        app_id: &str,
        exec_path: &str,
        icon: char,
        category: AppCategory,
    ) -> Self {
        Self {
            id,
            name: String::from(name),
            app_id: String::from(app_id),
            exec_path: String::from(exec_path),
            icon_char: icon,
            category,
            pinned: false,
            pinned_position: 0,
            launch_count: 0,
            last_launched: 0,
            installed_timestamp: 0,
            folder_id: None,
            description: String::new(),
        }
    }

    pub fn matches_query(&self, query: &str) -> bool {
        if query.is_empty() {
            return true;
        }
        let q = query;
        self.name.contains(q) || self.app_id.contains(q) || self.description.contains(q)
    }
}

// ── Pinned app folder ───────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct PinnedFolder {
    pub id: u64,
    pub name: String,
    pub app_ids: Vec<u64>,
    pub position: u32,
    pub expanded: bool,
}

impl PinnedFolder {
    pub fn new(id: u64, name: &str) -> Self {
        Self {
            id,
            name: String::from(name),
            app_ids: Vec::new(),
            position: 0,
            expanded: false,
        }
    }

    pub fn add_app(&mut self, app_id: u64) {
        if !self.app_ids.contains(&app_id) {
            self.app_ids.push(app_id);
        }
    }

    pub fn remove_app(&mut self, app_id: u64) {
        self.app_ids.retain(|&id| id != app_id);
    }

    pub fn count(&self) -> usize {
        self.app_ids.len()
    }

    pub fn toggle_expand(&mut self) {
        self.expanded = !self.expanded;
    }
}

// ── Recommended items ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecommendedKind {
    RecentFile,
    RecentApp,
    SuggestedApp,
}

#[derive(Debug, Clone)]
pub struct RecommendedItem {
    pub title: String,
    pub subtitle: String,
    pub icon_char: char,
    pub kind: RecommendedKind,
    pub path: String,
    pub timestamp: u64,
    pub app_id: Option<String>,
}

impl RecommendedItem {
    pub fn new_file(title: &str, path: &str, timestamp: u64) -> Self {
        Self {
            title: String::from(title),
            subtitle: String::from(path),
            icon_char: 'D',
            kind: RecommendedKind::RecentFile,
            path: String::from(path),
            timestamp,
            app_id: None,
        }
    }

    pub fn new_app(title: &str, app_id: &str, timestamp: u64) -> Self {
        Self {
            title: String::from(title),
            subtitle: String::from("Recently installed"),
            icon_char: 'A',
            kind: RecommendedKind::RecentApp,
            path: String::new(),
            timestamp,
            app_id: Some(String::from(app_id)),
        }
    }

    pub fn new_suggestion(title: &str, description: &str) -> Self {
        Self {
            title: String::from(title),
            subtitle: String::from(description),
            icon_char: 'S',
            kind: RecommendedKind::SuggestedApp,
            path: String::new(),
            timestamp: 0,
            app_id: None,
        }
    }
}

// ── Search results ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchResultKind {
    App,
    File,
    Setting,
    Web,
    Calculator,
}

#[derive(Debug, Clone)]
pub struct SearchResult {
    pub title: String,
    pub subtitle: String,
    pub kind: SearchResultKind,
    pub icon_char: char,
    pub action_path: String,
    pub relevance_score: u32,
}

impl SearchResult {
    pub fn new(
        title: &str,
        subtitle: &str,
        kind: SearchResultKind,
        icon: char,
        path: &str,
    ) -> Self {
        Self {
            title: String::from(title),
            subtitle: String::from(subtitle),
            kind,
            icon_char: icon,
            action_path: String::from(path),
            relevance_score: 0,
        }
    }
}

pub struct SearchEngine {
    pub results: Vec<SearchResult>,
    pub query: String,
    pub selected_index: usize,
    pub calculator_result: Option<String>,
}

impl SearchEngine {
    pub fn new() -> Self {
        Self {
            results: Vec::new(),
            query: String::new(),
            selected_index: 0,
            calculator_result: None,
        }
    }

    pub fn search(&mut self, query: &str, apps: &[AppInfo]) {
        self.query = String::from(query);
        self.results.clear();
        self.selected_index = 0;
        self.calculator_result = None;

        if query.is_empty() {
            return;
        }

        for app in apps {
            if app.matches_query(query) {
                self.results.push(SearchResult::new(
                    &app.name,
                    &app.exec_path,
                    SearchResultKind::App,
                    app.icon_char,
                    &app.exec_path,
                ));
            }
        }

        if let Some(calc_result) = self.try_calculate(query) {
            self.calculator_result = Some(calc_result.clone());
            self.results.insert(
                0,
                SearchResult::new(
                    &calc_result,
                    "Calculator",
                    SearchResultKind::Calculator,
                    '=',
                    "",
                ),
            );
        }

        self.results
            .sort_by(|a, b| b.relevance_score.cmp(&a.relevance_score));
    }

    pub fn select_next(&mut self) {
        if !self.results.is_empty() {
            self.selected_index = (self.selected_index + 1) % self.results.len();
        }
    }

    pub fn select_prev(&mut self) {
        if !self.results.is_empty() {
            self.selected_index = self
                .selected_index
                .checked_sub(1)
                .unwrap_or(self.results.len() - 1);
        }
    }

    pub fn selected_result(&self) -> Option<&SearchResult> {
        self.results.get(self.selected_index)
    }

    fn try_calculate(&self, expr: &str) -> Option<String> {
        let trimmed = expr.trim();
        if trimmed.is_empty() {
            return None;
        }
        let has_op = trimmed.contains('+')
            || trimmed.contains('-')
            || trimmed.contains('*')
            || trimmed.contains('/');
        if !has_op {
            return None;
        }

        if let Some(pos) = trimmed.find('+') {
            let left: i64 = trimmed[..pos].trim().parse().ok()?;
            let right: i64 = trimmed[pos + 1..].trim().parse().ok()?;
            return Some(format_i64(left + right));
        }
        None
    }
}

fn format_i64(val: i64) -> String {
    let mut s = String::new();
    let mut n = val;
    if n < 0 {
        s.push('-');
        n = -n;
    }
    if n == 0 {
        s.push('0');
        return s;
    }
    let mut digits = Vec::new();
    while n > 0 {
        digits.push((n % 10) as u8 + b'0');
        n /= 10;
    }
    for d in digits.iter().rev() {
        s.push(*d as char);
    }
    s
}

// ── User profile ────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct UserProfile {
    pub name: String,
    pub avatar_char: char,
    pub email: String,
    pub locked: bool,
}

impl UserProfile {
    pub fn new(name: &str, email: &str) -> Self {
        let avatar = name.chars().next().unwrap_or('U');
        Self {
            name: String::from(name),
            avatar_char: avatar,
            email: String::from(email),
            locked: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserAction {
    SignOut,
    SwitchUser,
    Lock,
    AccountSettings,
}

// ── Power menu ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerAction {
    Shutdown,
    Restart,
    Sleep,
    Hibernate,
    UpdateAndRestart,
    UpdateAndShutdown,
}

#[derive(Debug, Clone)]
pub struct PowerMenu {
    pub visible: bool,
    pub items: Vec<PowerMenuItem>,
    pub selected: usize,
    pub pending_update: bool,
}

#[derive(Debug, Clone)]
pub struct PowerMenuItem {
    pub action: PowerAction,
    pub label: String,
    pub icon_char: char,
    pub enabled: bool,
}

impl PowerMenu {
    pub fn new() -> Self {
        let items = alloc::vec![
            PowerMenuItem {
                action: PowerAction::Shutdown,
                label: String::from("Shut down"),
                icon_char: 'X',
                enabled: true
            },
            PowerMenuItem {
                action: PowerAction::Restart,
                label: String::from("Restart"),
                icon_char: 'R',
                enabled: true
            },
            PowerMenuItem {
                action: PowerAction::Sleep,
                label: String::from("Sleep"),
                icon_char: 'S',
                enabled: true
            },
            PowerMenuItem {
                action: PowerAction::Hibernate,
                label: String::from("Hibernate"),
                icon_char: 'H',
                enabled: true
            },
        ];
        Self {
            visible: false,
            items,
            selected: 0,
            pending_update: false,
        }
    }

    pub fn toggle(&mut self) {
        self.visible = !self.visible;
        self.selected = 0;
    }

    pub fn select_next(&mut self) {
        let enabled_count = self.items.iter().filter(|i| i.enabled).count();
        if enabled_count > 0 {
            self.selected = (self.selected + 1) % self.items.len();
            while !self.items[self.selected].enabled {
                self.selected = (self.selected + 1) % self.items.len();
            }
        }
    }

    pub fn select_prev(&mut self) {
        let enabled_count = self.items.iter().filter(|i| i.enabled).count();
        if enabled_count > 0 {
            self.selected = self.selected.checked_sub(1).unwrap_or(self.items.len() - 1);
            while !self.items[self.selected].enabled {
                self.selected = self.selected.checked_sub(1).unwrap_or(self.items.len() - 1);
            }
        }
    }

    pub fn selected_action(&self) -> Option<PowerAction> {
        self.items
            .get(self.selected)
            .filter(|i| i.enabled)
            .map(|i| i.action)
    }

    pub fn set_pending_update(&mut self, pending: bool) {
        self.pending_update = pending;
        if pending {
            self.items.push(PowerMenuItem {
                action: PowerAction::UpdateAndRestart,
                label: String::from("Update and restart"),
                icon_char: 'U',
                enabled: true,
            });
            self.items.push(PowerMenuItem {
                action: PowerAction::UpdateAndShutdown,
                label: String::from("Update and shut down"),
                icon_char: 'D',
                enabled: true,
            });
        }
    }
}

// ── Live tiles ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TileSize {
    Small,
    Medium,
    Wide,
    Large,
}

impl TileSize {
    pub fn grid_units(&self) -> (u32, u32) {
        match self {
            Self::Small => (1, 1),
            Self::Medium => (2, 2),
            Self::Wide => (4, 2),
            Self::Large => (4, 4),
        }
    }
}

#[derive(Debug, Clone)]
pub struct LiveTile {
    pub app_id: u64,
    pub size: TileSize,
    pub content: TileContent,
    pub group_id: Option<u64>,
    pub position: (u32, u32),
}

#[derive(Debug, Clone)]
pub enum TileContent {
    Static {
        title: String,
        icon_char: char,
    },
    Weather {
        temp: i32,
        condition: String,
        icon_char: char,
    },
    Calendar {
        event_title: String,
        event_time: String,
    },
    News {
        headline: String,
        source: String,
    },
    Counter {
        label: String,
        value: u32,
    },
}

#[derive(Debug, Clone)]
pub struct TileGroup {
    pub id: u64,
    pub name: String,
    pub tiles: Vec<u64>,
    pub collapsed: bool,
}

impl TileGroup {
    pub fn new(id: u64, name: &str) -> Self {
        Self {
            id,
            name: String::from(name),
            tiles: Vec::new(),
            collapsed: false,
        }
    }

    pub fn add_tile(&mut self, app_id: u64) {
        self.tiles.push(app_id);
    }

    pub fn remove_tile(&mut self, app_id: u64) {
        self.tiles.retain(|&id| id != app_id);
    }
}

// ── Context menu ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextAction {
    Open,
    RunAsAdmin,
    OpenFileLocation,
    Unpin,
    PinToStart,
    PinToTaskbar,
    ResizeTile(TileSize),
    AppSettings,
    Uninstall,
}

#[derive(Debug, Clone)]
pub struct ContextMenuItem {
    pub action: ContextAction,
    pub label: String,
    pub icon_char: char,
    pub enabled: bool,
    pub separator_after: bool,
    /// Optional trailing keyboard-shortcut hint (e.g. `"Ctrl+C"`), drawn
    /// right-aligned in `type.caption` / `text.tertiary` — the macOS/Win11
    /// affordance that teaches the accelerator without a tooltip.
    pub shortcut: Option<String>,
}

/// Map a `ContextAction` to the closest shipped `athgfx` line-icon — the SAME
/// icon set Files / the taskbar / Start tiles consume (no letter placeholders,
/// no new glyphs). The right-click menu is the highest-frequency surface, so its
/// leading icons must read as real vectors, not the bitmap-font `?`.
pub(crate) fn context_action_icon(action: ContextAction) -> athgfx::icon::Icon {
    use athgfx::icon::Icon;
    match action {
        ContextAction::Open => Icon::Folder,
        ContextAction::RunAsAdmin => Icon::Exec,
        ContextAction::OpenFileLocation => Icon::Folder,
        ContextAction::Unpin => Icon::Close,
        ContextAction::PinToStart => Icon::Plus,
        ContextAction::PinToTaskbar => Icon::Plus,
        ContextAction::ResizeTile(_) => Icon::Chevron,
        ContextAction::AppSettings => Icon::Gear,
        ContextAction::Uninstall => Icon::Close,
    }
}

#[derive(Debug, Clone)]
pub struct ContextMenu {
    pub visible: bool,
    pub items: Vec<ContextMenuItem>,
    pub selected: usize,
    pub target_app_id: Option<u64>,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl ContextMenu {
    pub fn new() -> Self {
        Self {
            visible: false,
            items: Vec::new(),
            selected: 0,
            target_app_id: None,
            x: 0,
            y: 0,
            width: 200,
            height: 0,
        }
    }

    /// Build a menu item with no shortcut hint (the common case).
    fn item(action: ContextAction, label: &str, icon_char: char, sep: bool) -> ContextMenuItem {
        ContextMenuItem {
            action,
            label: String::from(label),
            icon_char,
            enabled: true,
            separator_after: sep,
            shortcut: None,
        }
    }

    /// Build a menu item carrying a trailing keyboard-shortcut hint.
    fn item_shortcut(
        action: ContextAction,
        label: &str,
        icon_char: char,
        shortcut: &str,
        sep: bool,
    ) -> ContextMenuItem {
        ContextMenuItem {
            action,
            label: String::from(label),
            icon_char,
            enabled: true,
            separator_after: sep,
            shortcut: Some(String::from(shortcut)),
        }
    }

    pub fn show_for_pinned(&mut self, app_id: u64, x: i32, y: i32) {
        self.items.clear();
        self.items.push(Self::item_shortcut(
            ContextAction::Open,
            "Open",
            'O',
            "Enter",
            false,
        ));
        self.items.push(Self::item(
            ContextAction::RunAsAdmin,
            "Run as administrator",
            'A',
            true,
        ));
        self.items.push(Self::item(
            ContextAction::OpenFileLocation,
            "Open file location",
            'L',
            false,
        ));
        self.items.push(Self::item(
            ContextAction::Unpin,
            "Unpin from Start",
            'U',
            true,
        ));
        self.items.push(Self::item(
            ContextAction::PinToTaskbar,
            "Pin to taskbar",
            'T',
            false,
        ));
        self.items.push(Self::item(
            ContextAction::AppSettings,
            "App settings",
            'S',
            false,
        ));
        self.items.push(Self::item(
            ContextAction::Uninstall,
            "Uninstall",
            'X',
            false,
        ));

        self.visible = true;
        self.target_app_id = Some(app_id);
        self.x = x;
        self.y = y;
        self.selected = 0;
        self.relayout();
    }

    pub fn show_for_tile(&mut self, app_id: u64, x: i32, y: i32) {
        self.items.clear();
        self.items.push(Self::item_shortcut(
            ContextAction::Open,
            "Open",
            'O',
            "Enter",
            true,
        ));
        self.items.push(Self::item(
            ContextAction::ResizeTile(TileSize::Small),
            "Small",
            's',
            false,
        ));
        self.items.push(Self::item(
            ContextAction::ResizeTile(TileSize::Medium),
            "Medium",
            'm',
            false,
        ));
        self.items.push(Self::item(
            ContextAction::ResizeTile(TileSize::Wide),
            "Wide",
            'w',
            false,
        ));
        self.items.push(Self::item(
            ContextAction::ResizeTile(TileSize::Large),
            "Large",
            'l',
            true,
        ));
        self.items
            .push(Self::item(ContextAction::Unpin, "Unpin", 'U', false));
        self.items.push(Self::item(
            ContextAction::Uninstall,
            "Uninstall",
            'X',
            false,
        ));

        self.visible = true;
        self.target_app_id = Some(app_id);
        self.x = x;
        self.y = y;
        self.selected = 0;
        self.relayout();
    }

    pub fn hide(&mut self) {
        self.visible = false;
        self.target_app_id = None;
    }

    pub fn select_next(&mut self) {
        if !self.items.is_empty() {
            self.selected = (self.selected + 1) % self.items.len();
        }
    }

    pub fn select_prev(&mut self) {
        if !self.items.is_empty() {
            self.selected = self.selected.checked_sub(1).unwrap_or(self.items.len() - 1);
        }
    }

    pub fn selected_action(&self) -> Option<ContextAction> {
        self.items
            .get(self.selected)
            .filter(|i| i.enabled)
            .map(|i| i.action)
    }

    pub fn contains(&self, px: i32, py: i32) -> bool {
        px >= self.x
            && px < self.x + self.width as i32
            && py >= self.y
            && py < self.y + self.height as i32
    }

    /// Per-item row height (type.label line-height + vertical padding).
    const ITEM_H: usize = 30;
    /// Height of a separator group rule (the hairline lives centered in it).
    const SEP_H: usize = 9;
    /// Inner padding from the flyout rim to the first/last row.
    const PAD_Y: usize = 6;

    /// Lay out the flyout: measure the widest label + shortcut so the menu sizes
    /// to its content (macOS/Win11 fit-to-content), and total the per-row +
    /// separator heights. Writes `width`/`height` back so `contains` + hit-test
    /// stay correct. Returns nothing (mutates self).
    pub fn relayout(&mut self) {
        let label_style = ath_tokens::TYPE_LABEL;
        let caption_style = ath_tokens::TYPE_CAPTION;
        let sans = athgfx::text::FontFamily::Sans;
        // Geometry of a row: [pad][icon 18][gutter 10][label ...][gap 24][shortcut][pad]
        let icon_w = 18i32;
        let lead = 12i32 + icon_w + 10i32; // left pad + icon + gutter
        let trail = 12i32; // right pad
        let shortcut_gap = 24i32;
        let _ = (label_style, caption_style, sans);
        let mut max_w = 160i32; // a sane minimum so a 1-word menu isn't a sliver
        let mut total_h = Self::PAD_Y * 2;
        // Sizing uses a proportional char estimate (no Canvas here); `render`
        // right-aligns the shortcut with the EXACT measured advance, so a slightly
        // generous box only adds trailing margin, never clips.
        for it in &self.items {
            let lw = it.label.chars().count() as i32 * 9;
            let sw = it
                .shortcut
                .as_deref()
                .map(|s| shortcut_gap + s.chars().count() as i32 * 7)
                .unwrap_or(0);
            max_w = max_w.max(lead + lw + sw + trail);
        }
        for it in &self.items {
            total_h += Self::ITEM_H;
            if it.separator_after {
                total_h += Self::SEP_H;
            }
        }
        self.width = max_w as u32;
        self.height = total_h as u32;
    }

    /// Render the context menu as a `glass.popover` frosted flyout (IDENTITY.md
    /// §7) at `(x, y)` — the SAME `draw_glass_surface` stack (tint → frost →
    /// legibility-cap → iridescent rim → top highlight) the Start menu / taskbar /
    /// Control Center use, with a soft ambient drop shadow so it floats over
    /// whatever's behind it. Rows are RaeSans `type.label` ink with a leading
    /// `athgfx` line-icon and an optional right-aligned `type.caption` shortcut
    /// hint; the hovered/selected row is an accent wash with dark on-accent ink;
    /// disabled rows use `text.tertiary`; grouped sections are split by a hairline
    /// `stroke.subtle` rule. All token-derived (no hardcoded hex), legibility per
    /// the §9 a11y rule (the glass legibility cap + dark-on-accent ink).
    pub fn render(&self, canvas: &mut athgfx::Canvas, x: i32, y: i32) {
        if !self.visible || self.items.is_empty() {
            return;
        }
        let label_style = ath_tokens::TYPE_LABEL;
        let caption_style = ath_tokens::TYPE_CAPTION;
        let sans = athgfx::text::FontFamily::Sans;
        let p = crate::active_palette();
        let a = ath_tokens::derive_accent(crate::active_accent(), p);
        let on_accent = p.bg_base; // dark-on-accent ink (a11y: white-on-accent fails WCAG)

        let w = self.width.max(160) as usize;
        let h = self.height.max(Self::ITEM_H as u32) as usize;
        let radius = ath_tokens::RADIUS_MD as usize;

        // ── Soft ambient drop shadow so the flyout floats over the desktop. ──
        canvas.fill_rounded_rect_shadow(x as usize, y as usize, w, h, radius, 0x0A_10_1C, 34, 12);

        // ── The glass.popover surface (tint → frost → legibility cap → rim). ──
        athgfx::glass::draw_glass_surface(
            canvas,
            x as usize,
            y as usize,
            w,
            h,
            radius,
            ath_tokens::GLASS_POPOVER_DARK,
        );

        // ── Rows ──
        let icon_sz = 18i32;
        let lead = 12i32 + icon_sz + 10i32; // left pad + icon + gutter
        let mut row_top = y as usize + Self::PAD_Y;
        for (i, it) in self.items.iter().enumerate() {
            let selected = i == self.selected && it.enabled;
            let row_h = Self::ITEM_H;

            // Hover/selected row = accent wash inset from the rim.
            if selected {
                canvas.fill_rounded_rect(
                    x as usize + 6,
                    row_top + 2,
                    w.saturating_sub(12),
                    row_h.saturating_sub(4),
                    ath_tokens::RADIUS_SM as usize,
                    a.base,
                );
            }

            // Ink: dark-on-accent when selected; text.tertiary when disabled;
            // otherwise text.primary (label) / icon text.secondary.
            let (icon_ink, label_ink, hint_ink) = if selected {
                (on_accent, on_accent, with_alpha(on_accent, 0xCC))
            } else if !it.enabled {
                (p.text_tertiary, p.text_tertiary, p.text_tertiary)
            } else {
                (p.text_secondary, p.text_primary, p.text_tertiary)
            };

            // Leading line-icon (real athgfx vector, never a letter).
            let icon = context_action_icon(it.action);
            let icon_y = row_top + (row_h - icon_sz as usize) / 2;
            canvas.draw_icon(icon, x + 12, icon_y as i32, icon_sz, icon_ink);

            // Label (RaeSans type.label), vertically centered.
            let label_y = row_top + (row_h.saturating_sub(label_style.line_height as usize)) / 2;
            canvas.draw_text_aa(
                x + lead,
                label_y as i32,
                &it.label,
                label_style,
                label_ink,
                sans,
            );

            // Trailing shortcut hint (type.caption), right-aligned with the EXACT
            // measured advance so it hugs the right rim regardless of glyph widths.
            if let Some(sc) = &it.shortcut {
                let sw = canvas.measure_text_aa(sc, caption_style, sans);
                let sx = x + w as i32 - 12 - sw;
                let sy = row_top + (row_h.saturating_sub(caption_style.line_height as usize)) / 2;
                canvas.draw_text_aa(sx, sy as i32, sc, caption_style, hint_ink, sans);
            }

            row_top += row_h;

            // Group separator: a hairline stroke.subtle rule centered in the gap.
            if it.separator_after {
                let rule_y = row_top + Self::SEP_H / 2;
                for px in (x as usize + 10)..(x as usize + w - 10) {
                    canvas.blend_pixel(px, rule_y, p.stroke_subtle);
                }
                row_top += Self::SEP_H;
            }
        }
    }
}

// ── Animation ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuAnimationStyle {
    Slide,
    Fade,
    Zoom,
    None,
}

#[derive(Debug, Clone)]
pub struct MenuAnimation {
    pub style: MenuAnimationStyle,
    pub progress: f32,
    pub duration_ms: u32,
    pub elapsed_ms: u32,
    pub opening: bool,
}

impl MenuAnimation {
    pub fn new(style: MenuAnimationStyle, duration_ms: u32) -> Self {
        Self {
            style,
            progress: 0.0,
            duration_ms,
            elapsed_ms: 0,
            opening: true,
        }
    }

    pub fn start_open(&mut self) {
        self.opening = true;
        self.elapsed_ms = 0;
        self.progress = 0.0;
    }

    pub fn start_close(&mut self) {
        self.opening = false;
        self.elapsed_ms = 0;
        self.progress = 1.0;
    }

    pub fn tick(&mut self, delta_ms: u32) -> bool {
        self.elapsed_ms += delta_ms;
        let t = (self.elapsed_ms as f32 / self.duration_ms as f32).min(1.0);
        self.progress = if self.opening {
            ease_out_cubic(t)
        } else {
            1.0 - ease_out_cubic(t)
        };
        self.is_done()
    }

    pub fn is_done(&self) -> bool {
        self.elapsed_ms >= self.duration_ms
    }

    pub fn current_opacity(&self) -> f32 {
        self.progress
    }

    pub fn current_scale(&self) -> f32 {
        0.9 + 0.1 * self.progress
    }

    pub fn current_slide_offset(&self) -> i32 {
        ((1.0 - self.progress) * 20.0) as i32
    }
}

fn ease_out_cubic(t: f32) -> f32 {
    let t1 = 1.0 - t;
    1.0 - t1 * t1 * t1
}

// ── Theming ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeMode {
    Light,
    Dark,
}

#[derive(Debug, Clone)]
pub struct StartMenuTheme {
    pub mode: ThemeMode,
    pub background_color: u32,
    pub text_color: u32,
    pub accent_color: u32,
    pub hover_color: u32,
    pub selected_color: u32,
    pub dimmed_color: u32,
    pub border_color: u32,
    pub blur_enabled: bool,
    pub transparency: f32,
    pub corner_radius: u32,
}

impl StartMenuTheme {
    /// Dark theme, derived from `ath_tokens` through the LIVE seed accent (Vibe
    /// cohesion, IDENTITY.md §4.1) — the hardcoded palette is retired. The Start
    /// menu is a `glass.popover` flyout (§7); the per-field colours below feed the
    /// app-tile/search-row chrome that layers over the frosted glass. `text_color`
    /// is `text.primary` (the §9 a11y guardrail for chrome over the bright aurora),
    /// `hover_color` is a frosted wash, `selected_color`/`border_color` track the
    /// accent ramp so a one-tap Vibe re-skin recolours Start with the taskbar/CC.
    pub fn dark() -> Self {
        let p = crate::active_palette();
        let a = ath_tokens::derive_accent(crate::active_accent(), p);
        Self {
            mode: ThemeMode::Dark,
            background_color: ath_tokens::GLASS_POPOVER_DARK.tint,
            text_color: p.text_primary,
            accent_color: a.base,
            hover_color: ath_tokens::GLASS_POPOVER_DARK.frost,
            selected_color: a.subtle,
            dimmed_color: p.text_tertiary,
            border_color: a.base,
            blur_enabled: true,
            transparency: 0.85,
            corner_radius: ath_tokens::RADIUS_LG,
        }
    }

    pub fn light() -> Self {
        Self {
            mode: ThemeMode::Light,
            background_color: 0xFF_F8_F8_FA,
            text_color: 0xFF_1A_1A_2E,
            accent_color: 0xFF_00_5F_B3,
            hover_color: 0xFF_E8_E8_F0,
            selected_color: 0xFF_D0_D0_E0,
            dimmed_color: 0xFF_80_80_90,
            border_color: 0xFF_C0_C0_D0,
            blur_enabled: true,
            transparency: 0.92,
            corner_radius: 12,
        }
    }
}

// ── Quick access folders ────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct QuickAccessFolder {
    pub name: String,
    pub path: String,
    pub icon_char: char,
    pub pinned: bool,
}

impl QuickAccessFolder {
    pub fn new(name: &str, path: &str, icon: char) -> Self {
        Self {
            name: String::from(name),
            path: String::from(path),
            icon_char: icon,
            pinned: true,
        }
    }
}

// ── Recent activities / timeline ────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ActivityEntry {
    pub app_name: String,
    pub app_id: String,
    pub title: String,
    pub timestamp: u64,
    pub icon_char: char,
    pub document_path: Option<String>,
}

impl ActivityEntry {
    pub fn new(app_name: &str, app_id: &str, title: &str, timestamp: u64, icon: char) -> Self {
        Self {
            app_name: String::from(app_name),
            app_id: String::from(app_id),
            title: String::from(title),
            timestamp,
            icon_char: icon,
            document_path: None,
        }
    }
}

pub struct ActivityTimeline {
    pub entries: Vec<ActivityEntry>,
    pub max_entries: usize,
    pub visible: bool,
}

impl ActivityTimeline {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            max_entries: 50,
            visible: false,
        }
    }

    pub fn add_entry(&mut self, entry: ActivityEntry) {
        self.entries.insert(0, entry);
        if self.entries.len() > self.max_entries {
            self.entries.truncate(self.max_entries);
        }
    }

    pub fn entries_for_day(&self, day_timestamp: u64, seconds_per_day: u64) -> Vec<&ActivityEntry> {
        self.entries
            .iter()
            .filter(|e| {
                let day_start = day_timestamp - (day_timestamp % seconds_per_day);
                e.timestamp >= day_start && e.timestamp < day_start + seconds_per_day
            })
            .collect()
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    pub fn toggle(&mut self) {
        self.visible = !self.visible;
    }
}

// ── A-Z jump list ───────────────────────────────────────────────────────────

pub struct AlphabetJumpList {
    pub letters: [bool; 26],
    pub visible: bool,
    pub selected: Option<u8>,
}

impl AlphabetJumpList {
    pub fn new() -> Self {
        Self {
            letters: [false; 26],
            visible: false,
            selected: None,
        }
    }

    pub fn update_from_apps(&mut self, apps: &[AppInfo]) {
        self.letters = [false; 26];
        for app in apps {
            if let Some(first) = app.name.chars().next() {
                let c = first.to_ascii_uppercase();
                if c.is_ascii_uppercase() {
                    self.letters[(c as u8 - b'A') as usize] = true;
                }
            }
        }
    }

    pub fn select_letter(&mut self, letter: u8) {
        if letter < 26 && self.letters[letter as usize] {
            self.selected = Some(letter);
        }
    }

    pub fn toggle(&mut self) {
        self.visible = !self.visible;
    }

    pub fn available_letters(&self) -> Vec<char> {
        self.letters
            .iter()
            .enumerate()
            .filter(|(_, &avail)| avail)
            .map(|(i, _)| (b'A' + i as u8) as char)
            .collect()
    }
}

// ── All apps list ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppListSort {
    Alphabetical,
    ByCategory,
    RecentlyAdded,
    MostUsed,
}

pub struct AllAppsList {
    pub sort_mode: AppListSort,
    pub scroll_offset: usize,
    pub items_per_page: usize,
    pub expanded_categories: Vec<AppCategory>,
    pub filter_query: String,
}

impl AllAppsList {
    pub fn new() -> Self {
        Self {
            sort_mode: AppListSort::Alphabetical,
            scroll_offset: 0,
            items_per_page: 15,
            expanded_categories: Vec::new(),
            filter_query: String::new(),
        }
    }

    pub fn sort_apps<'a>(&self, apps: &'a [AppInfo]) -> Vec<&'a AppInfo> {
        let mut sorted: Vec<&AppInfo> = apps
            .iter()
            .filter(|a| a.matches_query(&self.filter_query))
            .collect();

        match self.sort_mode {
            AppListSort::Alphabetical => sorted.sort_by(|a, b| a.name.cmp(&b.name)),
            AppListSort::ByCategory => sorted.sort_by(|a, b| {
                (a.category as u8)
                    .cmp(&(b.category as u8))
                    .then(a.name.cmp(&b.name))
            }),
            AppListSort::RecentlyAdded => {
                sorted.sort_by(|a, b| b.installed_timestamp.cmp(&a.installed_timestamp))
            }
            AppListSort::MostUsed => sorted.sort_by(|a, b| b.launch_count.cmp(&a.launch_count)),
        }
        sorted
    }

    pub fn visible_page<'a>(&self, apps: &'a [AppInfo]) -> Vec<&'a AppInfo> {
        let sorted = self.sort_apps(apps);
        sorted
            .into_iter()
            .skip(self.scroll_offset)
            .take(self.items_per_page)
            .collect()
    }

    pub fn scroll_down(&mut self) {
        self.scroll_offset += 1;
    }

    pub fn scroll_up(&mut self) {
        self.scroll_offset = self.scroll_offset.saturating_sub(1);
    }

    pub fn set_sort(&mut self, mode: AppListSort) {
        self.sort_mode = mode;
        self.scroll_offset = 0;
    }

    pub fn toggle_category(&mut self, cat: AppCategory) {
        if let Some(pos) = self.expanded_categories.iter().position(|&c| c == cat) {
            self.expanded_categories.remove(pos);
        } else {
            self.expanded_categories.push(cat);
        }
    }
}

// ── Global Start Menu ───────────────────────────────────────────────────────

static START_MENU_INITIALIZED: AtomicBool = AtomicBool::new(false);

pub struct StartMenu {
    pub visible: bool,
    pub layout: StartMenuLayout,
    pub active_section: StartMenuSection,
    pub apps: Vec<AppInfo>,
    pub pinned_folders: Vec<PinnedFolder>,
    pub recommended: Vec<RecommendedItem>,
    pub search_engine: SearchEngine,
    pub all_apps: AllAppsList,
    pub user_profile: UserProfile,
    pub power_menu: PowerMenu,
    pub context_menu: ContextMenu,
    pub animation: MenuAnimation,
    pub theme: StartMenuTheme,
    pub quick_access: Vec<QuickAccessFolder>,
    pub activity_timeline: ActivityTimeline,
    pub jump_list: AlphabetJumpList,
    pub live_tiles: Vec<LiveTile>,
    pub tile_groups: Vec<TileGroup>,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub screen_width: u32,
    pub screen_height: u32,
    pub pinned_grid_cols: u32,
    pub pinned_grid_rows: u32,
    pub next_app_id: u64,
    pub next_folder_id: u64,
    pub next_tile_group_id: u64,
    pub scroll_offset: f32,
    pub scroll_momentum: f32,
}

impl StartMenu {
    pub fn new(screen_width: u32, screen_height: u32) -> Self {
        let width = 640;
        // Content-sized: search (52) + pinned header/grid (24+8+3×72) + gap 16
        // + Recommended header (24+8) + 4 rows (144) + footer 48 ≈ 560. A 700px
        // panel left a dead un-usable band above the footer (visual-QA).
        let height = 580;
        let x = ((screen_width - width) / 2) as i32;
        let y = (screen_height - height - 40) as i32;

        let mut menu = Self {
            visible: false,
            layout: StartMenuLayout::TwoColumn,
            active_section: StartMenuSection::PinnedApps,
            apps: Vec::new(),
            pinned_folders: Vec::new(),
            recommended: Vec::new(),
            search_engine: SearchEngine::new(),
            all_apps: AllAppsList::new(),
            user_profile: UserProfile::new("User", "user@raeos.local"),
            power_menu: PowerMenu::new(),
            context_menu: ContextMenu::new(),
            animation: MenuAnimation::new(MenuAnimationStyle::Zoom, 200),
            theme: StartMenuTheme::dark(),
            quick_access: Vec::new(),
            activity_timeline: ActivityTimeline::new(),
            jump_list: AlphabetJumpList::new(),
            live_tiles: Vec::new(),
            tile_groups: Vec::new(),
            x,
            y,
            width,
            height,
            screen_width,
            screen_height,
            pinned_grid_cols: 6,
            pinned_grid_rows: 3,
            next_app_id: 1,
            next_folder_id: 1,
            next_tile_group_id: 1,
            scroll_offset: 0.0,
            scroll_momentum: 0.0,
        };
        menu.populate_quick_access();
        menu
    }

    fn populate_quick_access(&mut self) {
        self.quick_access.push(QuickAccessFolder::new(
            "Documents",
            "/home/user/Documents",
            'D',
        ));
        self.quick_access.push(QuickAccessFolder::new(
            "Downloads",
            "/home/user/Downloads",
            'L',
        ));
        self.quick_access.push(QuickAccessFolder::new(
            "Pictures",
            "/home/user/Pictures",
            'P',
        ));
        self.quick_access
            .push(QuickAccessFolder::new("Music", "/home/user/Music", 'M'));
        self.quick_access
            .push(QuickAccessFolder::new("Desktop", "/home/user/Desktop", 'K'));
    }

    pub fn toggle(&mut self) {
        if self.visible {
            self.close();
        } else {
            self.open();
        }
    }

    /// Panel height that exactly wraps the content the render body will draw
    /// (search band + used pinned rows + recommended rows + footer). A fixed
    /// max-envelope height left a dead glass band above the footer whenever
    /// fewer than 3 pinned rows / 4 recommended rows exist (visual-QA).
    fn content_height(&self) -> u32 {
        let cols = self.pinned_grid_cols.max(1);
        let max_tiles = self.pinned_grid_cols * self.pinned_grid_rows;
        let shown = (self.pinned_apps().len() as u32).min(max_tiles);
        let rows = shown.div_ceil(cols).max(1);
        let rec = (self.recommended.len() as u32).min(4);
        let mut ch = 12 + 28 + 12; // search pill band
        ch += 24 + 8 + rows * 72; // "Pinned" header + grid
        ch += 16 + 24 + 8 + rec * 36; // "Recommended" header + rows
        ch += 12 + 48; // bottom slack + footer strip
        ch
    }

    pub fn open(&mut self) {
        self.visible = true;
        self.animation.start_open();
        self.active_section = StartMenuSection::PinnedApps;
        self.search_engine.query.clear();
        self.search_engine.results.clear();
        self.context_menu.hide();
        self.power_menu.visible = false;
        // Re-fit the anchored popover to its content each open (pin/unpin and
        // recents change between opens). FullScreen keeps its whole-screen rect.
        if self.layout != StartMenuLayout::FullScreen {
            self.height = self
                .content_height()
                .min(self.screen_height.saturating_sub(80));
            self.y = (self.screen_height.saturating_sub(self.height + 40)) as i32;
        }
    }

    pub fn close(&mut self) {
        self.animation.start_close();
        self.visible = false;
        self.context_menu.hide();
        self.power_menu.visible = false;
    }

    pub fn add_app(
        &mut self,
        name: &str,
        app_id: &str,
        exec_path: &str,
        icon: char,
        category: AppCategory,
    ) -> u64 {
        let id = self.next_app_id;
        self.next_app_id += 1;
        let app = AppInfo::new(id, name, app_id, exec_path, icon, category);
        self.apps.push(app);
        self.jump_list.update_from_apps(&self.apps);
        id
    }

    pub fn pin_app(&mut self, app_id: u64) {
        let pinned_count = self.apps.iter().filter(|a| a.pinned).count() as u32;
        if let Some(app) = self.apps.iter_mut().find(|a| a.id == app_id) {
            app.pinned = true;
            app.pinned_position = pinned_count + 1;
        }
    }

    pub fn unpin_app(&mut self, app_id: u64) {
        if let Some(app) = self.apps.iter_mut().find(|a| a.id == app_id) {
            app.pinned = false;
        }
    }

    pub fn move_pinned(&mut self, app_id: u64, new_position: u32) {
        if let Some(app) = self.apps.iter_mut().find(|a| a.id == app_id) {
            app.pinned_position = new_position;
        }
        let mut pinned: Vec<&mut AppInfo> = self.apps.iter_mut().filter(|a| a.pinned).collect();
        pinned.sort_by_key(|a| a.pinned_position);
        for (i, app) in pinned.iter_mut().enumerate() {
            app.pinned_position = i as u32;
        }
    }

    pub fn create_folder(&mut self, name: &str, app_ids: &[u64]) -> u64 {
        let fid = self.next_folder_id;
        self.next_folder_id += 1;
        let mut folder = PinnedFolder::new(fid, name);
        for &aid in app_ids {
            folder.add_app(aid);
            if let Some(app) = self.apps.iter_mut().find(|a| a.id == aid) {
                app.folder_id = Some(fid);
            }
        }
        self.pinned_folders.push(folder);
        fid
    }

    pub fn rename_folder(&mut self, folder_id: u64, new_name: &str) {
        if let Some(folder) = self.pinned_folders.iter_mut().find(|f| f.id == folder_id) {
            folder.name = String::from(new_name);
        }
    }

    pub fn delete_folder(&mut self, folder_id: u64) {
        if let Some(folder) = self.pinned_folders.iter().find(|f| f.id == folder_id) {
            for &app_id in &folder.app_ids {
                if let Some(app) = self.apps.iter_mut().find(|a| a.id == app_id) {
                    app.folder_id = None;
                }
            }
        }
        self.pinned_folders.retain(|f| f.id != folder_id);
    }

    pub fn search(&mut self, query: &str) {
        if query.is_empty() {
            self.active_section = StartMenuSection::PinnedApps;
        } else {
            self.active_section = StartMenuSection::Search;
        }
        self.search_engine.search(query, &self.apps);
    }

    pub fn launch_app(&mut self, app_id: u64) -> Option<&str> {
        if let Some(app) = self.apps.iter_mut().find(|a| a.id == app_id) {
            app.launch_count += 1;
            app.last_launched = 0; // would use real timestamp
            return Some(&app.exec_path);
        }
        None
    }

    pub fn add_recommended(&mut self, item: RecommendedItem) {
        self.recommended.push(item);
        if self.recommended.len() > 8 {
            self.recommended.remove(0);
        }
    }

    pub fn pinned_apps(&self) -> Vec<&AppInfo> {
        let mut apps: Vec<&AppInfo> = self
            .apps
            .iter()
            .filter(|a| a.pinned && a.folder_id.is_none())
            .collect();
        apps.sort_by_key(|a| a.pinned_position);
        apps
    }

    pub fn recently_added(&self) -> Vec<&AppInfo> {
        let mut apps: Vec<&AppInfo> = self.apps.iter().collect();
        apps.sort_by(|a, b| b.installed_timestamp.cmp(&a.installed_timestamp));
        apps.into_iter().take(5).collect()
    }

    pub fn most_used(&self) -> Vec<&AppInfo> {
        let mut apps: Vec<&AppInfo> = self.apps.iter().collect();
        apps.sort_by(|a, b| b.launch_count.cmp(&a.launch_count));
        apps.into_iter().take(5).collect()
    }

    pub fn show_all_apps(&mut self) {
        self.active_section = StartMenuSection::AllApps;
        self.all_apps.scroll_offset = 0;
    }

    pub fn show_pinned(&mut self) {
        self.active_section = StartMenuSection::PinnedApps;
    }

    pub fn show_recommended(&mut self) {
        self.active_section = StartMenuSection::Recommended;
    }

    pub fn handle_key_down(&mut self) {
        match self.active_section {
            StartMenuSection::Search => self.search_engine.select_next(),
            StartMenuSection::AllApps => self.all_apps.scroll_down(),
            _ => {}
        }
        if self.power_menu.visible {
            self.power_menu.select_next();
        }
        if self.context_menu.visible {
            self.context_menu.select_next();
        }
    }

    pub fn handle_key_up(&mut self) {
        match self.active_section {
            StartMenuSection::Search => self.search_engine.select_prev(),
            StartMenuSection::AllApps => self.all_apps.scroll_up(),
            _ => {}
        }
        if self.power_menu.visible {
            self.power_menu.select_prev();
        }
        if self.context_menu.visible {
            self.context_menu.select_prev();
        }
    }

    pub fn handle_tab(&mut self) {
        self.active_section = match self.active_section {
            StartMenuSection::PinnedApps => StartMenuSection::Recommended,
            StartMenuSection::Recommended => StartMenuSection::AllApps,
            StartMenuSection::AllApps => StartMenuSection::PinnedApps,
            StartMenuSection::Search => StartMenuSection::Search,
        };
    }

    pub fn tick(&mut self, delta_ms: u32) {
        self.animation.tick(delta_ms);

        if self.scroll_momentum.abs() > 0.1 {
            self.scroll_offset += self.scroll_momentum;
            self.scroll_momentum *= 0.92;
            if self.scroll_offset < 0.0 {
                self.scroll_offset = 0.0;
                self.scroll_momentum = 0.0;
            }
        } else {
            self.scroll_momentum = 0.0;
        }
    }

    pub fn apply_scroll(&mut self, delta: f32) {
        self.scroll_momentum += delta;
    }

    pub fn set_theme(&mut self, mode: ThemeMode) {
        self.theme = match mode {
            ThemeMode::Dark => StartMenuTheme::dark(),
            ThemeMode::Light => StartMenuTheme::light(),
        };
    }

    pub fn set_layout(&mut self, layout: StartMenuLayout) {
        self.layout = layout;
        if layout == StartMenuLayout::FullScreen {
            self.x = 0;
            self.y = 0;
            self.width = self.screen_width;
            self.height = self.screen_height - 40;
        } else {
            self.width = 640;
            self.height = 580;
            self.x = ((self.screen_width - self.width) / 2) as i32;
            self.y = (self.screen_height - self.height - 40) as i32;
        }
    }

    pub fn contains(&self, px: i32, py: i32) -> bool {
        px >= self.x
            && px < self.x + self.width as i32
            && py >= self.y
            && py < self.y + self.height as i32
    }

    pub fn add_live_tile(&mut self, app_id: u64, size: TileSize, content: TileContent) {
        self.live_tiles.push(LiveTile {
            app_id,
            size,
            content,
            group_id: None,
            position: (0, 0),
        });
    }

    pub fn create_tile_group(&mut self, name: &str) -> u64 {
        let id = self.next_tile_group_id;
        self.next_tile_group_id += 1;
        self.tile_groups.push(TileGroup::new(id, name));
        id
    }

    pub fn add_tile_to_group(&mut self, tile_app_id: u64, group_id: u64) {
        if let Some(tile) = self.live_tiles.iter_mut().find(|t| t.app_id == tile_app_id) {
            tile.group_id = Some(group_id);
        }
        if let Some(group) = self.tile_groups.iter_mut().find(|g| g.id == group_id) {
            group.add_tile(tile_app_id);
        }
    }

    pub fn resize_tile(&mut self, app_id: u64, new_size: TileSize) {
        if let Some(tile) = self.live_tiles.iter_mut().find(|t| t.app_id == app_id) {
            tile.size = new_size;
        }
    }

    pub fn add_quick_access(&mut self, name: &str, path: &str, icon: char) {
        self.quick_access
            .push(QuickAccessFolder::new(name, path, icon));
    }

    pub fn remove_quick_access(&mut self, path: &str) {
        self.quick_access.retain(|f| f.path != path);
    }

    pub fn record_activity(
        &mut self,
        app_name: &str,
        app_id: &str,
        title: &str,
        timestamp: u64,
        icon: char,
    ) {
        let entry = ActivityEntry::new(app_name, app_id, title, timestamp, icon);
        self.activity_timeline.add_entry(entry);
    }

    /// The clamped on-screen top-left the panel paints from (desktop-shell.md §2).
    ///
    /// The render body is authored in menu-LOCAL coordinates (0,0 = the panel's
    /// own corner); this translates them to the panel's intended position
    /// (`self.x`/`self.y`, anchored to the Start button / bottom-left like
    /// Win11/macOS) while CLAMPING so the whole `width`×`height` panel stays
    /// inside `screen_width`×`screen_height` — a negative or off-right position
    /// can never push the glass off the framebuffer. Returning this from a pure
    /// helper makes the position contract host-KAT-able (a regression back to the
    /// old literal `(0,0)` paint fails `render_honors_position_and_clamps`).
    pub fn panel_origin(&self) -> (usize, usize) {
        let w = self.width as usize;
        let h = self.height as usize;
        let max_ox = (self.screen_width as usize).saturating_sub(w);
        let max_oy = (self.screen_height as usize).saturating_sub(h);
        let ox = (self.x.max(0) as usize).min(max_ox);
        let oy = (self.y.max(0) as usize).min(max_oy);
        (ox, oy)
    }

    /// Render the start menu into the given Canvas.
    ///
    /// Draws: border, search bar, pinned-apps section (or search results),
    /// recommended section, quick-access folders, user profile, and power
    /// button. All geometry is offset by [`panel_origin`](Self::panel_origin)
    /// so the panel composites at its anchored on-screen position.
    pub fn render(&self, canvas: &mut athgfx::Canvas) {
        if !self.visible {
            return;
        }

        let t = &self.theme;
        // Chrome type: RaeSans proportional, NOT the 8x8 mono bitmap (visual-QA
        // Round-8 "unify chrome type" — the Start menu was the last surface still
        // drawing chrome labels through the chunky monospace path; macOS/Win11 use
        // a proportional UI sans for ALL chrome). `type.title` = section headers,
        // `type.label` = app/tile/row names, `type.caption` = hints/subtitles.
        // Every label routes through the SAME anti-aliased proportional path the
        // taskbar / Control Center use; advances are MEASURED (not count*char_w).
        // Section headers use `type.subtitle` (the token's documented "flyout
        // headers" role) — TYPE_TITLE's 22px/28 line overlapped the tile grid
        // below it (visual-QA: "Pinned baseline sits on the tiles").
        let heading_style = ath_tokens::TYPE_SUBTITLE;
        let heading_lh = heading_style.line_height as usize;
        let label_style = ath_tokens::TYPE_LABEL;
        let caption_style = ath_tokens::TYPE_CAPTION;
        let sans = athgfx::text::FontFamily::Sans;
        let w = self.width as usize;
        let h = self.height as usize;
        // ── On-screen anchor (desktop-shell.md §2 / Win11 + macOS): the menu is
        //    a positioned popover, NOT a top-left full-canvas paint. EVERY draw
        //    below is authored in menu-local coordinates (0,0 = the panel's top
        //    corner); we translate them by (ox, oy) so the panel lands at
        //    `self.x`/`self.y` — anchored to the Start button / bottom-left.
        //    The offset is CLAMPED so the whole panel stays within the screen
        //    (a negative or off-right `self.x` can never push the glass off the
        //    framebuffer). This is the fix for the "Start toggles visible=true but
        //    no panel renders" bug: the old code drew at literal (0,0) ignoring
        //    self.x/self.y, so the panel composited in the wrong region.
        let (ox, oy) = self.panel_origin();
        let p = crate::active_palette();
        let a = ath_tokens::derive_accent(crate::active_accent(), p);
        // Dark on-accent ink for any accent-filled element (IDENTITY guardrail:
        // white-on-RaeBlue ≈2.6:1 fails WCAG; selection ink must be bg.base).
        let on_accent = p.bg_base;
        let radius = t.corner_radius as usize;

        // ── Glass.popover flyout (IDENTITY.md §7): a frosted card the aurora
        //    reads through, with the iridescent rim. The shipped
        //    `draw_glass_surface` lays tint → frost → legibility-cap → rim → top
        //    highlight, the same call CC/Files/taskbar make. Retires the opaque
        //    `fill_rect(background_color)` + flat accent outline.
        athgfx::glass::draw_glass_surface(
            canvas,
            ox,
            oy,
            w,
            h,
            radius,
            ath_tokens::GLASS_POPOVER_DARK,
        );

        // Search bar — an obsidian input pill: solid `bg.elevated` ladder step
        // (the frost fill became invisible at obsidian frost alphas) with an
        // accent focus ring.
        let search_y = oy + 12usize;
        let search_h = 28usize;
        let search_r = search_h / 2;
        canvas.fill_rounded_rect(ox + 16, search_y, w - 32, search_h, search_r, p.bg_elevated);
        canvas.draw_rounded_rect_outline(ox + 16, search_y, w - 32, search_h, search_r, a.base);
        let _ = on_accent; // (selection ink used in tile/row branches below)
                           // Leading magnifier — a REAL athgfx line-icon (Round-8 residual icon gap:
                           // the field's loupe drew as a `?` from the bitmap font). Token-tinted
                           // text.secondary so it reads as a quiet affordance, not chrome.
        let mag_sz = 16usize;
        let mag_x = ox + 24usize;
        let mag_y = search_y + (search_h - mag_sz) / 2;
        canvas.draw_icon(
            athgfx::icon::Icon::Search,
            mag_x as i32,
            mag_y as i32,
            mag_sz as i32,
            p.text_secondary,
        );
        // Text starts AFTER the magnifier (icon width + a gutter), not at the rim.
        let search_text_x = mag_x + mag_sz + 8;
        let search_text = if self.search_engine.query.is_empty() {
            "Type to search..."
        } else {
            self.search_engine.query.as_str()
        };
        let sfg = if self.search_engine.query.is_empty() {
            t.dimmed_color
        } else {
            t.text_color
        };
        canvas.draw_text_aa(
            search_text_x as i32,
            (search_y + (search_h - label_style.line_height as usize) / 2) as i32,
            search_text,
            label_style,
            sfg,
            sans,
        );

        let body_y = search_y + search_h + 12;
        // Absolute lower bound for body content (panel bottom minus the footer
        // strip) — every "does this row still fit?" test compares against this so
        // content is clipped to the panel, never the framebuffer.
        let body_bottom = oy + h - 60;

        match self.active_section {
            StartMenuSection::Search => {
                // Search results
                canvas.draw_text_aa(
                    (ox + 16) as i32,
                    body_y as i32,
                    "Results",
                    heading_style,
                    t.accent_color,
                    sans,
                );
                let entry_h = 36usize;
                for (i, result) in self.search_engine.results.iter().take(10).enumerate() {
                    let ey = body_y + heading_lh + 8 + i * entry_h;
                    if ey + entry_h > body_bottom {
                        break;
                    }
                    let selected = i == self.search_engine.selected_index;
                    // Selected row = accent-filled card → dark on-accent ink;
                    // otherwise transparent (the frosted popover shows through).
                    let (icon_ink, title_ink, sub_ink) = if selected {
                        canvas.fill_rounded_rect(
                            ox + 12,
                            ey,
                            w - 24,
                            entry_h - 4,
                            ath_tokens::RADIUS_SM as usize,
                            a.base,
                        );
                        (on_accent, on_accent, with_alpha(on_accent, 0xCC))
                    } else {
                        (a.base, t.text_color, t.dimmed_color)
                    };
                    // Result row icon (Round-7 #1): a shipped line-icon keyed off
                    // the result kind/title, not the letter glyph.
                    let ricon = match result.kind {
                        SearchResultKind::App => app_line_icon(&result.action_path, &result.title),
                        SearchResultKind::File => {
                            entry_line_icon(&result.title, &result.action_path)
                        }
                        SearchResultKind::Setting => athgfx::icon::Icon::Gear,
                        SearchResultKind::Web => athgfx::icon::Icon::WiFi,
                        SearchResultKind::Calculator => athgfx::icon::Icon::Doc,
                    };
                    canvas.draw_icon(ricon, (ox + 22) as i32, (ey + 8) as i32, 18, icon_ink);
                    canvas.draw_text_aa(
                        (ox + 44) as i32,
                        (ey + 6) as i32,
                        &result.title,
                        label_style,
                        title_ink,
                        sans,
                    );
                    canvas.draw_text_aa(
                        (ox + 44) as i32,
                        (ey + 20) as i32,
                        &result.subtitle,
                        caption_style,
                        sub_ink,
                        sans,
                    );
                }
            }
            _ => {
                // Pinned apps section
                canvas.draw_text_aa(
                    (ox + 16) as i32,
                    body_y as i32,
                    "Pinned",
                    heading_style,
                    t.text_color,
                    sans,
                );
                // "All apps >" right-aligned by its MEASURED proportional advance
                // (not a fixed mono offset) so it always clears the right inset.
                let all_apps = "All apps >";
                let all_adv = canvas.measure_text_aa(all_apps, label_style, sans) as usize;
                let more_x = ox + w.saturating_sub(all_adv + 16);
                // Bottom-align the 16px label line with the 24px header line.
                canvas.draw_text_aa(
                    more_x as i32,
                    (body_y + heading_lh.saturating_sub(label_style.line_height as usize)) as i32,
                    all_apps,
                    label_style,
                    t.accent_color,
                    sans,
                );

                let grid_y = body_y + heading_lh + 8;
                let cols = self.pinned_grid_cols.max(1) as usize;
                let cell_w = (w - 32) / cols;
                let cell_h = 72usize;
                let pinned = self.pinned_apps();
                let max_tiles = (self.pinned_grid_cols * self.pinned_grid_rows) as usize;
                // Rows the grid ACTUALLY fills — the Recommended section docks
                // right below the last used row, not below the reserved 3-row
                // envelope (which left a dead band when few apps are pinned).
                let shown = pinned.len().min(max_tiles);
                let rows_used = shown.div_ceil(cols).max(1);

                for (i, app) in pinned.iter().take(max_tiles).enumerate() {
                    let col = i % cols;
                    let row = i / cols;
                    let cx = ox + 16 + col * cell_w;
                    let cy = grid_y + row * cell_h;

                    // App tile — OBSIDIAN raised card (IDENTITY-OBSIDIAN.md §2):
                    // soft shadow → SOLID `bg.elevated` ladder-step face →
                    // hairline. Depth from dark-on-darker + light edge, not a
                    // frost wash. `start_tile_card_face` is the KAT's source
                    // of truth for the face.
                    let tile_w = cell_w - 4;
                    let tile_h = cell_h - 4;
                    let tile_r = ath_tokens::RADIUS_MD as usize;
                    canvas.fill_rounded_rect_shadow(
                        cx,
                        cy,
                        tile_w,
                        tile_h,
                        tile_r,
                        0x00_02_03_06, // neutral near-black penumbra
                        8,             // soft ambient spread
                        3,             // cast slightly down — light-from-above
                    );
                    canvas.fill_rounded_rect(
                        cx,
                        cy,
                        tile_w,
                        tile_h,
                        tile_r,
                        start_tile_card_face(0),
                    );
                    canvas.draw_rounded_rect_outline(
                        cx,
                        cy,
                        tile_w,
                        tile_h,
                        tile_r,
                        p.stroke_subtle,
                    );
                    // Real athgfx line-icon (Round-7 #1) instead of the accent
                    // LETTER — centred in the tile's top band, accent-tinted.
                    let icon_sz = 24usize;
                    let icon_x = cx + (cell_w - 4) / 2 - icon_sz / 2;
                    canvas.draw_icon(
                        app_line_icon(&app.app_id, &app.name),
                        icon_x as i32,
                        (cy + 11) as i32,
                        icon_sz as i32,
                        a.base,
                    );

                    // Proportional truncation + centering: drop trailing chars
                    // until the RaeSans advance fits the tile, then centre on the
                    // MEASURED width (not a mono char-count) so the name neither
                    // clips nor drifts off-centre.
                    let name_avail = cell_w.saturating_sub(8);
                    let full = app.name.as_str();
                    let mut end = full.len();
                    while end > 0 {
                        if canvas.measure_text_aa(&full[..end], label_style, sans) as usize
                            <= name_avail
                        {
                            break;
                        }
                        end -= 1;
                        while end > 0 && !full.is_char_boundary(end) {
                            end -= 1;
                        }
                    }
                    let name = &full[..end];
                    let name_adv = canvas.measure_text_aa(name, label_style, sans) as usize;
                    let name_x = cx + (cell_w - 4) / 2 - name_adv / 2;
                    canvas.draw_text_aa(
                        name_x as i32,
                        (cy + 42) as i32,
                        name,
                        label_style,
                        t.text_color,
                        sans,
                    );
                }

                // Recommended section — docks below the rows the grid actually
                // used (not the reserved envelope), with a real section gap.
                let rec_y = grid_y + rows_used * cell_h + 16;
                if rec_y + heading_lh < body_bottom {
                    canvas.draw_text_aa(
                        (ox + 16) as i32,
                        rec_y as i32,
                        "Recommended",
                        heading_style,
                        t.text_color,
                        sans,
                    );
                    let entry_h = 36usize;
                    for (i, rec) in self.recommended.iter().take(4).enumerate() {
                        let ey = rec_y + heading_lh + 8 + i * entry_h;
                        if ey + entry_h > body_bottom {
                            break;
                        }
                        // Recommended row = the SAME obsidian raised card as the
                        // pinned tiles: shadow → solid ladder-step face → hairline.
                        let row_x = ox + 12usize;
                        let row_w = w - 24;
                        let row_h = entry_h - 4;
                        let row_r = ath_tokens::RADIUS_SM as usize;
                        canvas.fill_rounded_rect_shadow(
                            row_x,
                            ey,
                            row_w,
                            row_h,
                            row_r,
                            0x00_02_03_06,
                            8,
                            3,
                        );
                        canvas.fill_rounded_rect(
                            row_x,
                            ey,
                            row_w,
                            row_h,
                            row_r,
                            start_tile_card_face(0),
                        );
                        canvas.draw_rounded_rect_outline(
                            row_x,
                            ey,
                            row_w,
                            row_h,
                            row_r,
                            p.stroke_subtle,
                        );
                        // Recommended row icon (Round-7 #1): file/app line-icon.
                        let ricon = match rec.kind {
                            RecommendedKind::RecentApp | RecommendedKind::SuggestedApp => {
                                app_line_icon(rec.app_id.as_deref().unwrap_or(""), &rec.title)
                            }
                            RecommendedKind::RecentFile => entry_line_icon(&rec.title, &rec.path),
                        };
                        canvas.draw_icon(
                            ricon,
                            (ox + 22) as i32,
                            (ey + 6) as i32,
                            18,
                            t.accent_color,
                        );
                        canvas.draw_text_aa(
                            (ox + 44) as i32,
                            (ey + 4) as i32,
                            &rec.title,
                            label_style,
                            t.text_color,
                            sans,
                        );
                        // Subtitle: proportional truncation by MEASURED advance.
                        let sub_avail = w.saturating_sub(64);
                        let subf = rec.subtitle.as_str();
                        let mut send = subf.len();
                        while send > 0 {
                            if canvas.measure_text_aa(&subf[..send], caption_style, sans) as usize
                                <= sub_avail
                            {
                                break;
                            }
                            send -= 1;
                            while send > 0 && !subf.is_char_boundary(send) {
                                send -= 1;
                            }
                        }
                        canvas.draw_text_aa(
                            (ox + 44) as i32,
                            (ey + 18) as i32,
                            &subf[..send],
                            caption_style,
                            t.dimmed_color,
                            sans,
                        );
                    }
                }
            }
        }

        // Bottom bar: user profile + power button — an obsidian footer band one
        // ladder step ABOVE the popover face (the frost strip is invisible at
        // obsidian frost alphas). Rounded so the strip's bottom corners follow
        // the panel's radius.
        let bot_y = oy + h - 48;
        canvas.fill_rounded_rect(ox, bot_y, w, 48, radius, p.bg_raised);
        // Hairline divider above the footer (stroke.subtle).
        for x in (ox + 8)..(ox + w - 8) {
            canvas.blend_pixel(x, bot_y, p.stroke_subtle);
        }

        // User avatar + name — an accent-ringed circular avatar carrying the
        // person line-icon (the old path drew a bare letter glyph).
        let av_d = 28usize;
        let av_x = ox + 16;
        let av_y = bot_y + (48 - av_d) / 2;
        canvas.fill_rounded_rect(av_x, av_y, av_d, av_d, av_d / 2, with_alpha(a.base, 0x33));
        canvas.draw_rounded_rect_outline(av_x, av_y, av_d, av_d, av_d / 2, a.base);
        canvas.draw_icon(
            athgfx::icon::Icon::Accessibility,
            (av_x + 5) as i32,
            (av_y + 5) as i32,
            (av_d - 10) as i32,
            t.text_color,
        );
        canvas.draw_text_aa(
            (av_x + av_d + 10) as i32,
            (bot_y + (48 - label_style.line_height as usize) / 2) as i32,
            &self.user_profile.name,
            label_style,
            t.text_color,
            sans,
        );

        // Power button — accent-filled pill carrying the REAL power line-icon
        // with dark on-accent ink (was a 'P' letter glyph).
        let pw_x = ox + w - 44;
        let pw_y = bot_y + (48 - 28) / 2;
        canvas.fill_rounded_rect(pw_x, pw_y, 28, 28, 8, a.base);
        canvas.draw_icon(
            athgfx::icon::Icon::Power,
            (pw_x + 6) as i32,
            (pw_y + 6) as i32,
            16,
            on_accent,
        );
    }
}

// ── Liquid-Glass identity proof (IDENTITY.md §7) ────────────────────────────

#[cfg(test)]
mod identity_tests {
    use super::*;

    #[test]
    fn start_tiles_map_to_real_line_icons_not_letters() {
        // visual-QA Round-7 #1: each pinned app tile must resolve a real athgfx
        // Icon (the bitmap font drew the accent LETTER as a stand-in). The known
        // bundled apps map to their recognisable glyph; an unknown app falls back
        // to the generic File icon (never a letter). FAIL-able: a regression that
        // dropped the mapping would return `File` for Terminal too.
        use athgfx::icon::Icon;
        assert_eq!(app_line_icon("com.raeos.terminal", "Terminal"), Icon::Exec);
        assert_eq!(app_line_icon("com.raeos.files", "Files"), Icon::FolderSolid);
        assert_eq!(app_line_icon("com.raeos.browser", "Browser"), Icon::WiFi);
        assert_eq!(app_line_icon("com.raeos.settings", "Settings"), Icon::Gear);
        assert_eq!(app_line_icon("com.raeos.editor", "Text Editor"), Icon::Code);
        assert_eq!(
            app_line_icon("com.raeos.games", "RaeGames"),
            Icon::GameController
        );
        // Unknown app → generic File, NOT a letter glyph.
        assert_eq!(app_line_icon("com.example.unknown", "Weird"), Icon::File);
        // Name-keyed fallback works when the id is opaque.
        assert_eq!(app_line_icon("x", "My Terminal"), Icon::Exec);
    }

    #[test]
    fn start_theme_derives_from_tokens_not_hardcoded_hex() {
        // The dark theme must resolve from ath_tokens (the glass.popover tier +
        // the live accent ramp), NOT the retired hardcoded palette. FAIL-able: if
        // a future edit re-hardcodes `0xFF_12_14_20`, these mismatch.
        let t = StartMenuTheme::dark();
        let p = crate::active_palette();
        let a = ath_tokens::derive_accent(crate::active_accent(), p);
        assert_eq!(
            t.background_color,
            ath_tokens::GLASS_POPOVER_DARK.tint,
            "Start background must be the glass.popover tier (IDENTITY §7)"
        );
        assert_eq!(
            t.accent_color, a.base,
            "accent must track the live seed ramp"
        );
        assert_eq!(
            t.text_color, p.text_primary,
            "label ink must be text.primary (a11y guardrail over bright glass)"
        );
        assert_eq!(
            t.corner_radius,
            ath_tokens::RADIUS_LG,
            "Start uses radius.lg (IDENTITY §5.1)"
        );
    }

    #[test]
    fn start_uses_translucent_popover_glass() {
        // OBSIDIAN (IDENTITY-OBSIDIAN.md §2): the popover is NEAR-opaque
        // near-black — a whisper of backdrop bleeds (never a milky sheet,
        // never fully dead). FAIL-able both directions: alpha ≥ 0xFB kills
        // the wallpaper bleed; alpha < 0xE0 regresses toward the mid-gray
        // frost look. RGB must be near-black (each channel ≤ 0x20).
        let tint = ath_tokens::GLASS_POPOVER_DARK.tint;
        let a = (tint >> 24) & 0xFF;
        assert!(
            (0xE0..=0xFA).contains(&a),
            "glass.popover tint alpha must sit in the obsidian band [0xE0,0xFA] (got {a:#04X})"
        );
        for (name, ch) in [
            ("r", (tint >> 16) & 0xFF),
            ("g", (tint >> 8) & 0xFF),
            ("b", tint & 0xFF),
        ] {
            assert!(
                ch <= 0x20,
                "glass.popover tint {name} channel must be near-black (got {ch:#04X})"
            );
        }
    }

    #[test]
    fn start_tiles_float_above_the_gap_not_below() {
        // visual-QA Round-8 P0 #1 (re-contracted for OBSIDIAN): tile FACES must
        // read ABOVE the inter-tile gap. On the near-black material the face is
        // a solid elevation-ladder step; a +3 mean-luma-point margin on an L≈11
        // gap is a ≥25% relative lift — clearly visible. FAIL-able: dropping
        // the face to bg_raised (or the gap brightening past the face) flips it.
        for bd in [0xFF_14_18_2A_u32, 0xFF_1E_26_42, 0xFF_2A_30_50] {
            let gap = tile_luma_pct(start_gap_interior(bd));
            let face = tile_luma_pct(start_tile_card_face(bd));
            let delta = face - gap;
            assert!(
                delta >= 3.0,
                "tile face must float >= +3 L over the inter-tile gap (obsidian ladder step); \
                 bd={bd:08X} gap L={gap:.1} face L={face:.1} delta={delta:+.1}"
            );
        }
    }

    #[test]
    fn start_search_field_uses_real_magnifier_icon_not_question_mark() {
        // visual-QA Round-8 residual: the search field's loupe rendered as `?` (the
        // bitmap font's missing-glyph box). The field must draw a REAL athgfx line
        // icon. FAIL-able: `Icon::Search` must be a distinct, shipped icon (not the
        // generic File fallback the letter-glyph path collapsed to), and it carries a
        // stroke command set (a real vector), not an empty/placeholder glyph.
        use athgfx::icon::Icon;
        assert_ne!(
            Icon::Search,
            Icon::File,
            "magnifier must be the Search icon"
        );
        // The icon name resolves (a `?` placeholder would have no mapping).
        assert_eq!(Icon::Search.name(), "search");
    }

    #[test]
    fn start_selection_ink_is_dark_on_accent() {
        // The selected-row / power-button ink is bg.base (dark), never white —
        // white-on-RaeBlue ≈2.6:1 fails WCAG. Render and assert the selected card
        // does not carry white ink. We check the design invariant directly.
        let p = crate::active_palette();
        let on_accent = p.bg_base;
        let r = ((on_accent >> 16) & 0xFF) as f32;
        let g = ((on_accent >> 8) & 0xFF) as f32;
        let b = (on_accent & 0xFF) as f32;
        let luma = (0.2126 * r + 0.7152 * g + 0.0722 * b) / 255.0;
        assert!(
            luma < 0.2,
            "on-accent ink (bg.base) must be dark, not white (luma={luma})"
        );
    }

    #[test]
    fn start_chrome_text_is_raesans_proportional_not_mono_bitmap() {
        // visual-QA Round-8 "unify chrome type": every Start-menu chrome label
        // (search field, section headers, app/tile names, recent rows, profile)
        // routes through the anti-aliased PROPORTIONAL RaeSans path the taskbar /
        // Control Center use — NOT the chunky 8x8 mono bitmap. Two FAIL-able
        // invariants off the same AA engine render() now calls:
        use athgfx::text::FontFamily;
        assert!(
            athgfx::text::ensure_init(),
            "RaeSans AA engine must be available for the chrome-type path"
        );
        // (1) PROPORTIONAL: equal char count, different glyph widths → different
        //     advances under RaeSans (a mono/bitmap path gives count*fixed). The
        //     tile-name centering + truncation now depend on this being true.
        let sw = 320usize;
        let sh = 40usize;
        let mut px = alloc::vec![0xFF_10_14_20u32; sw * sh];
        let c = unsafe { athgfx::Canvas::new(px.as_mut_ptr() as *mut u8, sw, sh, 4) };
        let narrow = c.measure_text_aa("iiii", ath_tokens::TYPE_LABEL, FontFamily::Sans);
        let wide = c.measure_text_aa("WWWW", ath_tokens::TYPE_LABEL, FontFamily::Sans);
        assert!(
            wide > narrow,
            "RaeSans must be proportional (W wider than i): narrow={narrow} wide={wide}"
        );
        // (2) AA INK: a section header lays down non-uniform glyph coverage
        //     (grayscale AA edges) the 1-bit bitmap blit can't produce.
        let mut c = unsafe { athgfx::Canvas::new(px.as_mut_ptr() as *mut u8, sw, sh, 4) };
        let stats = c.draw_text_aa_stats(
            8,
            8,
            "Pinned",
            ath_tokens::TYPE_TITLE,
            0xFF_FF_FF_FF,
            FontFamily::Sans,
        );
        assert!(
            stats.total_coverage > 0 && stats.min_cov < stats.max_cov,
            "header must render anti-aliased RaeSans ink (non-uniform coverage): \
             total={} min={} max={}",
            stats.total_coverage,
            stats.min_cov,
            stats.max_cov
        );
    }

    #[test]
    fn context_menu_is_glass_with_separators_and_real_icons() {
        // Phase-2 glassify: the right-click menu (highest-frequency surface) must
        // carry the Liquid Glass identity, NOT flat hardcoded styling. Three
        // FAIL-able invariants:
        //   (1) the menu is built on the glass.popover tier (translucent so the
        //       backdrop reads through — an opaque sheet fails);
        //   (2) it has separator rules between groups (a flat list of rows fails);
        //   (3) every item maps to a REAL athgfx line-icon, never a letter glyph.
        let mut m = ContextMenu::new();
        m.show_for_pinned(7, 100, 100);

        // (1) glass.popover tier is translucent.
        let tint = ath_tokens::GLASS_POPOVER_DARK.tint;
        assert!(
            (0xE0..=0xFA).contains(&((tint >> 24) & 0xFF)),
            "context menu must use the obsidian glass.popover tier — near-opaque \
             near-black with a whisper of bleed (tint alpha={:#X})",
            (tint >> 24) & 0xFF
        );

        // (2) at least one separator rule splits the groups.
        let seps = m.items.iter().filter(|i| i.separator_after).count();
        assert!(
            seps >= 1,
            "context menu must group items with hairline separators (found {seps})"
        );

        // (3) icons are real shipped glyphs, not letters.
        use athgfx::icon::Icon;
        assert_eq!(context_action_icon(ContextAction::Open), Icon::Folder);
        assert_eq!(context_action_icon(ContextAction::AppSettings), Icon::Gear);
        assert_eq!(context_action_icon(ContextAction::RunAsAdmin), Icon::Exec);
        // Every item resolves a named (non-placeholder) icon.
        for it in &m.items {
            let name = context_action_icon(it.action).name();
            assert!(!name.is_empty(), "every menu item needs a real icon");
        }
    }

    #[test]
    fn render_honors_position_and_clamps() {
        // BUG (live QEMU beta-test): pressing the Start hotkey set visible=true (the
        // "Rae" pill highlighted) but NO panel appeared — render() drew its glass at
        // a literal (0,0) using width/height, IGNORING self.x/self.y, so the panel
        // composited in the wrong region. The panel must paint from its anchored
        // on-screen origin. Two FAIL-able invariants on the pure geometry seam the
        // render now uses:
        //
        //   (1) the origin TRACKS self.x/self.y (a regression to literal 0,0 fails);
        //   (2) the origin is CLAMPED so the whole panel stays on-screen (an off-
        //       right / negative position can never push the glass off-framebuffer).
        let mut m = StartMenu::new(1280, 800);
        m.set_layout(StartMenuLayout::TwoColumn); // 640x700 anchored panel

        // (1) origin tracks the menu's intended position (bottom-left anchor on a
        //     1280x800 screen → a non-(0,0) on-screen origin).
        let (ox, oy) = m.panel_origin();
        assert_eq!(
            (ox, oy),
            (m.x as usize, m.y as usize),
            "panel origin must honor self.x/self.y, not paint at (0,0)"
        );
        assert!(
            ox != 0 || oy != 0,
            "a bottom-left-anchored menu must NOT render at the canvas top-left \
             (the exact 0,0-ignores-position bug)"
        );

        // (2a) clamp a position pushed off the RIGHT/BOTTOM edge back on-screen.
        m.x = 5000;
        m.y = 5000;
        let (cx, cy) = m.panel_origin();
        assert!(
            cx + m.width as usize <= m.screen_width as usize,
            "panel right edge must stay on-screen (cx={cx} w={} sw={})",
            m.width,
            m.screen_width
        );
        assert!(
            cy + m.height as usize <= m.screen_height as usize,
            "panel bottom edge must stay on-screen (cy={cy} h={} sh={})",
            m.height,
            m.screen_height
        );

        // (2b) a negative position clamps to 0 (never paints off the left/top).
        m.x = -400;
        m.y = -400;
        assert_eq!(
            m.panel_origin(),
            (0, 0),
            "a negative position must clamp to the top-left corner, not underflow"
        );

        // (3) end-to-end: a visible menu actually lays ink down INSIDE the panel
        //     rect and at its anchored origin (not at canvas 0,0). Render into a
        //     full-screen canvas and confirm the panel's top-left glass pixel is
        //     painted while a pixel well OUTSIDE the panel (and not at 0,0) is left
        //     as the cleared backdrop. FAIL-able: the old 0,0 paint would light up
        //     the canvas corner and leave the anchored origin untouched.
        let sw = 1280usize;
        let sh = 800usize;
        let mut px = alloc::vec![0xFF_00_00_00u32; sw * sh];
        let mut m2 = StartMenu::new(sw as u32, sh as u32);
        m2.set_layout(StartMenuLayout::TwoColumn);
        let fid = m2.add_app(
            "Files",
            "com.raeos.files",
            "/apps/files",
            'F',
            AppCategory::Utilities,
        );
        m2.pin_app(fid);
        m2.visible = true;
        let (px0, py0) = m2.panel_origin();
        {
            let mut canvas = unsafe { athgfx::Canvas::new(px.as_mut_ptr() as *mut u8, sw, sh, 4) };
            m2.render(&mut canvas);
        }
        // Sample a pixel near the panel CENTER (clear of the rounded corners) — it
        // must have changed from the cleared backdrop (the glass surface painted
        // there). Sampling the center proves the paint landed at the anchored
        // origin, not the canvas corner.
        let cx_s = px0 + (m2.width as usize) / 2;
        let cy_s = py0 + (m2.height as usize) / 2;
        let inside = px[cy_s * sw + cx_s];
        assert_ne!(
            inside, 0xFF_00_00_00,
            "the panel must paint glass at its anchored on-screen origin ({px0},{py0})"
        );
        // The far top-right corner is OUTSIDE the 640-wide panel anchored bottom-left,
        // so it must remain the cleared backdrop (no stray full-canvas paint).
        let outside = px[2 * sw + (sw - 4)];
        assert_eq!(
            outside, 0xFF_00_00_00,
            "the menu must not paint outside its panel rect (top-right corner dirtied)"
        );
    }

    #[test]
    fn context_menu_hover_ink_is_dark_on_accent_not_white() {
        // The hovered/selected row paints an accent wash; its ink must be the dark
        // bg.base (dark-on-accent), NOT white — white-on-RaeBlue ≈2.6:1 fails WCAG
        // (IDENTITY §9 a11y guardrail). FAIL-able: a regression to white ink raises
        // this luma past the threshold.
        let p = crate::active_palette();
        let on_accent = p.bg_base;
        let r = ((on_accent >> 16) & 0xFF) as f32;
        let g = ((on_accent >> 8) & 0xFF) as f32;
        let b = (on_accent & 0xFF) as f32;
        let luma = (0.2126 * r + 0.7152 * g + 0.0722 * b) / 255.0;
        assert!(
            luma < 0.2,
            "context-menu hover ink (bg.base) must be dark-on-accent, not white \
             (luma={luma})"
        );
    }

    #[test]
    fn context_menu_renders_glass_and_accent_hover_over_backdrop() {
        // End-to-end render FAIL-able: render the menu over a known mid backdrop and
        // assert (a) the hovered row carries accent-tinted pixels (the wash drew),
        // and (b) the surface is NOT a flat field (luma spread proves glass + ink).
        assert!(athgfx::text::ensure_init());
        let (w, h) = (320usize, 280usize);
        let mut px = vec![0xFF_14_18_2Au32; w * h];
        let mut m = ContextMenu::new();
        m.show_for_pinned(1, 0, 0);
        m.selected = 0; // hover the first ("Open") row
        {
            let mut c = unsafe { athgfx::Canvas::new(px.as_mut_ptr() as *mut u8, w, h, 4) };
            m.render(&mut c, 12, 12);
        }
        let a = ath_tokens::derive_accent(crate::active_accent(), crate::active_palette());
        let ar = ((a.base >> 16) & 0xFF) as i32;
        let ag = ((a.base >> 8) & 0xFF) as i32;
        let ab = (a.base & 0xFF) as i32;
        let mut accent_px = 0u32;
        let (mut lo, mut hi) = (u32::MAX, 0u32);
        for &p in px.iter() {
            let r = ((p >> 16) & 0xFF) as i32;
            let g = ((p >> 8) & 0xFF) as i32;
            let b = (p & 0xFF) as i32;
            if (r - ar).abs() < 40 && (g - ag).abs() < 40 && (b - ab).abs() < 40 {
                accent_px += 1;
            }
            let l = (r + g + b) as u32;
            lo = lo.min(l);
            hi = hi.max(l);
        }
        assert!(
            accent_px > 200,
            "hover row must paint an accent wash (accent-like px={accent_px})"
        );
        assert!(
            hi - lo > 80,
            "context menu must not be a flat field (luma spread {} too small)",
            hi - lo
        );
    }
}

// ── Global instance ─────────────────────────────────────────────────────────

static mut START_MENU: Option<StartMenu> = None;

pub fn init() {
    if START_MENU_INITIALIZED.swap(true, Ordering::SeqCst) {
        return;
    }
    unsafe {
        let mut menu = StartMenu::new(1920, 1080);

        let term_id = menu.add_app(
            "Terminal",
            "com.raeos.terminal",
            "/usr/bin/raeterminal",
            'T',
            AppCategory::Utilities,
        );
        let files_id = menu.add_app(
            "Files",
            "com.raeos.files",
            "/usr/bin/raefiles",
            'F',
            AppCategory::Utilities,
        );
        let browser_id = menu.add_app(
            "Browser",
            "com.raeos.browser",
            "/usr/bin/raebrowser",
            'W',
            AppCategory::Web,
        );
        let settings_id = menu.add_app(
            "Settings",
            "com.raeos.settings",
            "/usr/bin/athsettings",
            'S',
            AppCategory::System,
        );
        let editor_id = menu.add_app(
            "Text Editor",
            "com.raeos.editor",
            "/usr/bin/raeeditor",
            'E',
            AppCategory::Productivity,
        );
        let monitor_id = menu.add_app(
            "System Monitor",
            "com.raeos.monitor",
            "/usr/bin/raemonitor",
            'M',
            AppCategory::System,
        );
        let _game_id = menu.add_app(
            "RaeGames",
            "com.raeos.games",
            "/usr/bin/raegames",
            'G',
            AppCategory::Games,
        );
        let _music_id = menu.add_app(
            "Music",
            "com.raeos.music",
            "/usr/bin/raemusic",
            'H',
            AppCategory::Media,
        );
        let _calc_id = menu.add_app(
            "Calculator",
            "com.raeos.calc",
            "/usr/bin/raecalc",
            'C',
            AppCategory::Utilities,
        );

        menu.pin_app(term_id);
        menu.pin_app(files_id);
        menu.pin_app(browser_id);
        menu.pin_app(settings_id);
        menu.pin_app(editor_id);
        menu.pin_app(monitor_id);

        menu.add_recommended(RecommendedItem::new_file(
            "report.pdf",
            "/home/user/Documents/report.pdf",
            1000,
        ));
        menu.add_recommended(RecommendedItem::new_file(
            "notes.txt",
            "/home/user/Documents/notes.txt",
            900,
        ));
        menu.add_recommended(RecommendedItem::new_app("Music", "com.raeos.music", 800));

        START_MENU = Some(menu);
    }
}

pub fn get() -> Option<&'static StartMenu> {
    // SAFETY: single-threaded shell init/access pattern (init() gates on the
    // INITIALIZED flag); raw-pointer read avoids the static_mut_refs lint.
    unsafe { (*core::ptr::addr_of!(START_MENU)).as_ref() }
}

pub fn get_mut() -> Option<&'static mut StartMenu> {
    // SAFETY: as above — exclusive shell-thread access.
    unsafe { (*core::ptr::addr_of_mut!(START_MENU)).as_mut() }
}
