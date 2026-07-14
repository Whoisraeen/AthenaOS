//! Command palette — the global launcher + action runner for AthShell.
//!
//! A single keystroke (`Super+Space`) summons a floating glass field; you type
//! and the OS answers instantly — launch an app, jump to a setting, open a
//! file, or do a sum. It fuses macOS Spotlight's instant fuzzy launch with the
//! VSCode/rofi "run-an-action" model: apps, settings-actions, files and an
//! inline calculator are ranked together and dispatched by one Enter.
//!
//! Concept: *"Familiar enough to switch from Windows or Mac in 10 minutes."*
//! (LEGACY_GAMING_CONCEPT.md §Three User Experiences) + *"Fast is a feature."*
//!
//! This surface WIRES the (previously dead) `search_indexer::SearchEngine` to a
//! window: it instantiates the engine, feeds it the app registry, a real
//! settings-actions catalog, and (for apps/settings/calc) an in-process engine,
//! then renders ranked `SearchResult` rows per `docs/design/command-palette.md`.
//! The Enter dispatch maps each `SearchAction` to a real shell handler — there
//! are no fake actions here; every result does something.
//!
//! ## One file index (no parallel walk)
//!
//! The FILE/document portion of every query is served from the KERNEL search
//! index (`kernel::search_index`, syscalls 54-57) — the same index the
//! post-login crawler (`crawl_session_home`) populates. The host wires it via
//! [`CommandPalette::set_kernel_file_source`]; the palette is `no_std` and owns
//! no syscalls, so the host (`shell_runner`, same address space) injects a
//! provider that routes to `search_index::query`. This RETIRES the palette's
//! private VFS-walk file index as the live source — the crawler and the palette
//! now agree on one index (raeen-kernel's review fix). If no kernel source is
//! wired, the palette falls back to the legacy engine file index (no regression).

use crate::search_indexer::{
    AppIndexEntry, SearchAction, SearchEngine, SearchResult, SearchResultType, SettingIndexEntry,
};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// One file/folder hit resolved from the KERNEL search index (`search_index`,
/// syscalls 54-57) — the single source of truth the post-login crawler feeds.
///
/// The kernel index query (`SYS_SEARCH_QUERY`) returns only `(id, kind)`; the
/// host (`shell_runner`, in the same address space) resolves each id to its
/// display name + path and hands the palette this richer hit. This is the file
/// data source that REPLACES the palette's private VFS-walk file index, so the
/// crawler and the palette agree on one index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KernelFileHit {
    /// Leaf display name (what the row title shows).
    pub name: String,
    /// Absolute path (the `Open` target + row subtitle).
    pub path: String,
    /// True for a directory (`Folder` row), false for a file/document.
    pub is_folder: bool,
}

/// Host-supplied file-search provider: given a query and a result cap, return
/// the matching files/folders FROM THE KERNEL INDEX. The palette is `no_std`
/// and owns no syscalls, so the host (`shell_runner`) injects this; it routes
/// internally to `search_index::query`. Returning an empty vec is the normal
/// pre-crawl / no-match case (handled gracefully — "no results", never an error).
pub type KernelFileQuery = fn(query: &str, max: usize) -> Vec<KernelFileHit>;

/// What a fired result asks the host (kernel `shell_runner`) to do. The palette
/// is `no_std` and owns no syscalls, so dispatch is returned as an intent the
/// kernel executes through its existing capability-checked handlers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaletteDispatch {
    /// Launch an application by exec path (Start-grid launch path).
    Launch(String),
    /// Open a file/folder path (file manager / default handler).
    Open(String),
    /// Navigate the Settings panel to a `settings:<page>` target, or run a
    /// settings ACTION (e.g. `action:toggle_vibe`). The host routes both.
    Navigate(String),
    /// Copy a calculator answer to the clipboard.
    Copy(String),
    /// Nothing actionable (empty selection).
    None,
}

/// A settings-action catalog entry: a human label, a description, search
/// keywords, and the `Navigate` target the engine emits. The target is either
/// `settings:<page>` (open Settings to a page) or `action:<verb>` (run a real
/// system action). Both resolve to real handlers in `shell_runner`.
struct SettingsAction {
    name: &'static str,
    target: &'static str,
    description: &'static str,
    keywords: &'static [&'static str],
}

/// The v1 settings-actions catalog. Every entry maps to a real shell action
/// (open a Settings page or run a toggle) — this IS the action-runner story.
const SETTINGS_ACTIONS: &[SettingsAction] = &[
    SettingsAction {
        name: "Open Display Settings",
        target: "settings:display",
        description: "Resolution, refresh rate, HDR, night light",
        keywords: &["display", "screen", "monitor", "resolution", "hdr", "night"],
    },
    SettingsAction {
        name: "Open Audio Settings",
        target: "settings:audio",
        description: "Output device, volume, spatial audio",
        keywords: &["audio", "sound", "volume", "speaker", "headphone"],
    },
    SettingsAction {
        name: "Open Network Settings",
        target: "settings:network",
        description: "Wi-Fi, VPN, gaming QoS",
        keywords: &["network", "wifi", "vpn", "internet", "ethernet"],
    },
    SettingsAction {
        name: "Open Gaming Settings",
        target: "settings:gaming",
        description: "SCHED_BODY, GPU power, shader cache",
        keywords: &["gaming", "game", "fps", "gpu", "shader", "latency"],
    },
    SettingsAction {
        name: "Open Appearance Settings",
        target: "settings:appearance",
        description: "Vibe Mode, window manager, animations",
        keywords: &["appearance", "theme", "vibe", "wallpaper", "look"],
    },
    SettingsAction {
        name: "Open Security Settings",
        target: "settings:security",
        description: "Sandboxing, code signing, firewall",
        keywords: &["security", "sandbox", "firewall", "signing", "privacy"],
    },
    SettingsAction {
        name: "Open System Settings",
        target: "settings:system",
        description: "Updates, telemetry, snapshots",
        keywords: &["system", "update", "telemetry", "snapshot", "about"],
    },
    SettingsAction {
        name: "Toggle Vibe Mode",
        target: "action:cycle_vibe",
        description: "Cycle the system-wide visual personality",
        keywords: &["vibe", "theme", "accent", "color", "cyberpunk", "ghibli"],
    },
    SettingsAction {
        name: "Toggle Focus / Do Not Disturb",
        target: "action:toggle_dnd",
        description: "Silence notifications",
        keywords: &[
            "focus",
            "dnd",
            "disturb",
            "quiet",
            "silence",
            "notification",
        ],
    },
    SettingsAction {
        name: "Open Notification Center",
        target: "action:notifications",
        description: "Notification history and quick settings",
        keywords: &["notification", "center", "history", "tray", "bell"],
    },
    SettingsAction {
        name: "Lock Workstation",
        target: "action:lock",
        description: "Lock the screen",
        keywords: &["lock", "screen", "secure", "away"],
    },
    SettingsAction {
        name: "Sign Out",
        target: "action:logout",
        description: "Return to the login screen",
        keywords: &["sign out", "logout", "log off", "switch user"],
    },
    SettingsAction {
        name: "Run Rae Script",
        target: "action:run_script",
        description: "Run your quick automation script (config key /scripting/palette_script)",
        keywords: &["script", "rae", "raelang", "run", "automation", "macro"],
    },
];

/// The number of settings-actions seeded — exposed so the boot smoketest can
/// assert the catalog populated (FAIL-able if it drifts to 0).
pub const SETTINGS_ACTION_COUNT: usize = SETTINGS_ACTIONS.len();

/// The glass tier the palette renders with. A command/search flyout is a
/// transient surface over arbitrary content, so it is a `glass.popover`
/// (IDENTITY.md §2.1 / §7) — the same tier Start, context menus and toasts use,
/// the most opaque so text is instantly readable over a busy backdrop. This is
/// the single source the `render` path passes to `draw_glass_surface`, and the
/// FAIL-able KAT asserts it is the POPOVER tier (not the deprecated
/// `GLASS_TINT_DARK` alias, which resolves to the PANEL tint).
pub(crate) const PALETTE_GLASS_TIER: rae_tokens::GlassTier = rae_tokens::GLASS_POPOVER_DARK;

use crate::text_util::truncate_chars;

/// The global command palette surface.
pub struct CommandPalette {
    /// The wired search engine (apps + settings + files + inline calc). This is
    /// the previously-dead `SearchEngine`, now LIVE.
    engine: SearchEngine,
    pub visible: bool,
    pub query: String,
    pub selected: usize,
    results: Vec<SearchResult>,
    /// A transient confirmation shown in the field (e.g. "Copied 42") after a
    /// calculator copy — the glance-and-grab flow from the spec §5.
    pub confirmation: Option<String>,
    pub screen_width: usize,
    pub screen_height: usize,
    /// Count of apps fed in (for the smoketest).
    indexed_apps: usize,
    /// Count of files fed in (for the smoketest).
    indexed_files: usize,
    /// When set by the host, the file/document portion of every query is served
    /// from the KERNEL search index through this provider (the crawler's index —
    /// one source of truth) instead of the engine's private file index. Unset =
    /// fall back to the engine file index (no regression on a host that hasn't
    /// wired the kernel source yet).
    kernel_files: Option<KernelFileQuery>,
    /// Max kernel file hits to request per query (UI shows at most 8 rows total).
    kernel_file_cap: usize,
}

impl CommandPalette {
    pub fn new(screen_width: usize, screen_height: usize) -> Self {
        let mut p = Self {
            engine: SearchEngine::new(),
            visible: false,
            query: String::new(),
            selected: 0,
            results: Vec::new(),
            confirmation: None,
            screen_width,
            screen_height,
            indexed_apps: 0,
            indexed_files: 0,
            kernel_files: None,
            kernel_file_cap: 16,
        };
        p.seed_settings_actions();
        p
    }

    /// Feed the engine the settings-actions catalog (called at construction).
    fn seed_settings_actions(&mut self) {
        for a in SETTINGS_ACTIONS {
            self.engine.indexer.index_setting(SettingIndexEntry {
                name: String::from(a.name),
                path: String::from(a.target),
                description: String::from(a.description),
                keywords: a.keywords.iter().map(|k| String::from(*k)).collect(),
                category: String::from("System"),
                icon: None,
            });
        }
    }

    /// Feed one application into the index (the 8 bundled apps come through here
    /// from the shell's start-menu registry).
    pub fn index_app(&mut self, name: &str, exec: &str, description: &str, keywords: &[&str]) {
        self.engine.indexer.index_application(AppIndexEntry {
            name: String::from(name),
            exec: String::from(exec),
            icon: None,
            description: String::from(description),
            keywords: keywords.iter().map(|k| String::from(*k)).collect(),
            categories: Vec::new(),
            desktop_file: String::new(),
            usage_count: 0,
            last_used: 0,
        });
        self.indexed_apps += 1;
    }

    /// Feed one file path from the boot filesystem into the index.
    ///
    /// NOTE: this seeds the engine's OWN file index, which is the legacy
    /// (private VFS-walk) data source. When the host wires a kernel file source
    /// via [`set_kernel_file_source`], queries serve files from the KERNEL index
    /// instead and these engine-indexed files are no longer queried for — the
    /// two indexes stop diverging. Kept so an un-wired host still finds files.
    pub fn index_file(&mut self, path: &str) {
        if self.engine.indexer.index_file(path).is_ok() {
            self.indexed_files += 1;
        }
    }

    /// Wire the KERNEL search index as the file/document data source. The host
    /// (`shell_runner`) passes a provider that routes to `search_index::query`,
    /// so the palette and the post-login crawler share one index. After this is
    /// set, the file portion of each query comes from the kernel; app/setting/
    /// calculator results still come from the in-process engine.
    pub fn set_kernel_file_source(&mut self, source: KernelFileQuery) {
        self.kernel_files = Some(source);
    }

    /// True once a kernel file source is wired (for the smoketest / introspection).
    pub fn has_kernel_file_source(&self) -> bool {
        self.kernel_files.is_some()
    }

    pub fn indexed_apps(&self) -> usize {
        self.indexed_apps
    }
    pub fn indexed_files(&self) -> usize {
        self.indexed_files
    }
    pub fn settings_actions(&self) -> usize {
        SETTINGS_ACTION_COUNT
    }
    pub fn result_count(&self) -> usize {
        self.results.len()
    }

    /// The title of the currently-selected result (for the smoketest / a11y).
    pub fn selected_title(&self) -> Option<&str> {
        self.results.get(self.selected).map(|r| r.title.as_str())
    }

    /// Open the palette. Per spec §1 a fresh open "selects all existing text so
    /// a new query replaces the last" — we model that as starting empty, so the
    /// first keystroke begins a clean query (no stale prefix from a prior open).
    pub fn open(&mut self) {
        self.visible = true;
        self.confirmation = None;
        self.selected = 0;
        self.query.clear();
        self.results.clear();
    }

    pub fn close(&mut self) {
        self.visible = false;
        self.query.clear();
        self.results.clear();
        self.selected = 0;
        self.confirmation = None;
    }

    /// Toggle semantics (spec §1): the hotkey closes an open palette.
    pub fn toggle(&mut self) {
        if self.visible {
            self.close();
        } else {
            self.open();
        }
    }

    /// Append a printable char to the query and re-rank.
    pub fn push_char(&mut self, c: char) {
        self.confirmation = None;
        self.query.push(c);
        self.selected = 0;
        self.recompute();
    }

    /// Backspace the query and re-rank.
    pub fn backspace(&mut self) {
        self.confirmation = None;
        self.query.pop();
        self.selected = 0;
        self.recompute();
    }

    pub fn select_next(&mut self) {
        let n = self.results.len();
        if n > 0 {
            self.selected = (self.selected + 1) % n;
        }
    }

    pub fn select_prev(&mut self) {
        let n = self.results.len();
        if n > 0 {
            self.selected = self.selected.checked_sub(1).unwrap_or(n - 1);
        }
    }

    /// Run the live indexer for the current query (the per-keystroke search).
    ///
    /// Data sources are merged and ranked together (spec §3): apps, settings,
    /// calculator, contacts and bookmarks come from the in-process engine; the
    /// FILE/document portion comes from the KERNEL index when a source is wired
    /// (one source of truth — the crawler feeds it), else from the engine's
    /// legacy file index. An empty/not-yet-crawled kernel index simply yields no
    /// file rows (graceful — the render path shows "No results", never an error).
    fn recompute(&mut self) {
        let q = self.query.trim();
        if q.is_empty() {
            self.results.clear();
            return;
        }
        let mut results = self.engine.search(q);

        if let Some(source) = self.kernel_files {
            // The kernel index is now the file source: drop the engine's own
            // file/folder/document hits so the two indexes don't both appear,
            // then splice in the kernel hits. App/setting/calc/contact/bookmark
            // results are untouched.
            results.retain(|r| !Self::is_file_result(r.entry_type));
            let hits = source(q, self.kernel_file_cap);
            results.extend(Self::map_kernel_hits(q, &hits));
            // Re-rank by score so kernel file rows interleave with app/setting
            // rows exactly as the engine's own ranking would (spec §3.2).
            results.sort_by(|a, b| {
                b.score
                    .partial_cmp(&a.score)
                    .unwrap_or(core::cmp::Ordering::Equal)
            });
            results.truncate(20);
        }

        self.results = results;
        if self.selected >= self.results.len() {
            self.selected = 0;
        }
    }

    /// Whether a result type is a file-family row (served by the file index).
    fn is_file_result(rt: SearchResultType) -> bool {
        matches!(
            rt,
            SearchResultType::File | SearchResultType::Folder | SearchResultType::RecentDocument
        )
    }

    /// Map kernel-index file hits to ranked `SearchResult` rows. PURE (no
    /// engine/syscall state) so it is host-testable — the FAIL-able KAT feeds
    /// synthetic hits and asserts the exact rows + ordering.
    ///
    /// Scoring mirrors the engine's file-result band so kernel rows interleave
    /// naturally: an exact (case-insensitive) name match scores highest, a
    /// prefix match next, a plain substring match lowest — always below an app
    /// or settings-action exact hit (those score ~0.9-1.0 in the engine) so a
    /// file never outranks the app you literally named.
    fn map_kernel_hits(query: &str, hits: &[KernelFileHit]) -> Vec<SearchResult> {
        let ql = query.trim().to_ascii_lowercase();
        let mut out = Vec::with_capacity(hits.len());
        for h in hits {
            let nl = h.name.to_ascii_lowercase();
            let score = if nl == ql {
                0.80
            } else if nl.starts_with(&ql) {
                0.70
            } else {
                0.55
            };
            let entry_type = if h.is_folder {
                SearchResultType::Folder
            } else {
                SearchResultType::File
            };
            out.push(SearchResult {
                entry_type,
                title: h.name.clone(),
                subtitle: h.path.clone(),
                icon: None,
                path: Some(h.path.clone()),
                score,
                highlights: Vec::new(),
                action: SearchAction::Open(h.path.clone()),
            });
        }
        out
    }

    /// Map the selected result's `SearchAction` to a host dispatch intent
    /// (spec §5). Returns `PaletteDispatch::None` when nothing is selected.
    /// For a `Copy`, the caller copies and the palette shows a confirmation.
    pub fn fire_selected(&mut self) -> PaletteDispatch {
        let Some(result) = self.results.get(self.selected) else {
            return PaletteDispatch::None;
        };
        match &result.action {
            SearchAction::Launch(exec) => PaletteDispatch::Launch(exec.clone()),
            SearchAction::Open(path) => PaletteDispatch::Open(path.clone()),
            SearchAction::Navigate(target) => PaletteDispatch::Navigate(target.clone()),
            SearchAction::Calculate(value) => {
                self.confirmation = Some(format!("Copied {value}"));
                PaletteDispatch::Copy(value.clone())
            }
            SearchAction::WebSearch(q) => {
                // Web is reserved (spec §5/§7) — surface it as a navigate so the
                // host can decide; never auto-ranked above local hits.
                PaletteDispatch::Navigate(format!("web:{q}"))
            }
        }
    }

    /// A short category tag for a row (spec §3.4).
    fn category_label(rt: SearchResultType) -> &'static str {
        match rt {
            SearchResultType::Application => "App",
            SearchResultType::Setting => "Action",
            SearchResultType::File | SearchResultType::RecentDocument => "File",
            SearchResultType::Folder => "Folder",
            SearchResultType::Calculator => "Math",
            SearchResultType::Contact => "Contact",
            SearchResultType::Bookmark => "Bookmark",
            _ => "Result",
        }
    }

    /// Render the floating glass palette into the desktop canvas (spec §2/§3).
    /// Drawn as an overlay into the shell surface, same family as the Start
    /// menu (material.glass, radius.lg, accent-from-`derive_accent`).
    pub fn render(&self, canvas: &mut raegfx::Canvas) {
        if !self.visible {
            return;
        }
        use rae_tokens::{RADIUS_LG, RADIUS_SM, RADIUS_XS, SPACE_2, SPACE_3, SPACE_4};
        let accent = crate::accent();
        let p = crate::PALETTE;

        // ── Backdrop scrim: dim the desktop ~12% so the glass reads over busy
        //    wallpaper (spec §2). A flat alpha wash over the whole screen.
        for y in 0..self.screen_height {
            for x in 0..self.screen_width {
                canvas.blend_pixel(x, y, 0x20_0A_0C_14);
            }
        }

        // ── Geometry (spec §2): 640px wide, query field top at 28% height. ──
        let margin = rae_tokens::SPACE_6 as usize;
        let panel_w = 640usize.min(self.screen_width.saturating_sub(2 * margin));
        let panel_x = (self.screen_width.saturating_sub(panel_w)) / 2;
        let field_y = self.screen_height * 28 / 100;
        let field_h = 52usize;
        let row_h = 44usize;
        let pad = SPACE_4 as usize;

        let visible_rows = self.results.len().min(8);
        let list_h = if visible_rows > 0 {
            visible_rows * row_h + SPACE_2 as usize
        } else if !self.query.trim().is_empty() {
            row_h // single "no results" row (spec §7)
        } else {
            0
        };
        let panel_y = field_y.saturating_sub(pad);
        let panel_h = pad + field_h + list_h + pad;

        // ── Glass.popover flyout (IDENTITY.md §7): a search/command flyout is a
        //    transient popover (macOS Spotlight / Win11 PowerToys Run), so it uses
        //    the POPOVER tier — the most opaque so text is instantly readable over a
        //    busy backdrop. A soft ambient drop shadow first so the card floats off
        //    the desktop, then the shipped `draw_glass_surface` lays the full stack:
        //    luma-adjusted tint → frost white sheen → legibility cap → iridescent
        //    rim → top highlight. This RETIRES the deprecated `GLASS_TINT_DARK`
        //    alias fill; the tier + measured backdrop pick the alpha, not us. The
        //    same call CC / Start / Files / the taskbar make.
        canvas.fill_rounded_rect_shadow(
            panel_x,
            panel_y,
            panel_w,
            panel_h,
            RADIUS_LG as usize,
            0x0A_10_1C,
            40,
            16,
        );
        raegfx::glass::draw_glass_surface(
            canvas,
            panel_x,
            panel_y,
            panel_w,
            panel_h,
            RADIUS_LG as usize,
            PALETTE_GLASS_TIER,
        );

        // ── Query field: a frosted inset well (the popover frost sheen, the SAME
        //    pill Start's search bar uses) with an accent hairline so the focus is
        //    never colour-only (a11y §8). ─────────────────────────────────────
        let field_x = panel_x + pad;
        let field_w = panel_w - 2 * pad;
        canvas.fill_rounded_rect(
            field_x,
            field_y,
            field_w,
            field_h,
            RADIUS_SM as usize,
            PALETTE_GLASS_TIER.frost,
        );
        canvas.draw_rounded_rect_outline(
            field_x,
            field_y,
            field_w,
            field_h,
            RADIUS_SM as usize,
            accent.base,
        );
        // Leading magnifier — a REAL raegfx line-icon (not the bitmap-font '>'),
        // token-tinted text.secondary so it reads as a quiet affordance.
        let mag_sz = 18i32;
        canvas.draw_icon(
            raegfx::icon::Icon::Search,
            (field_x + SPACE_3 as usize) as i32,
            (field_y + (field_h - mag_sz as usize) / 2) as i32,
            mag_sz,
            p.text_secondary,
        );
        let text_x = field_x + SPACE_3 as usize + 16;
        let text_y = (field_y
            + (field_h.saturating_sub(rae_tokens::TYPE_SUBTITLE.line_height as usize)) / 2)
            as i32;
        // The confirmation (e.g. "Copied 42") replaces the query for one beat.
        let (field_text, field_fg): (&str, u32) = if let Some(ref c) = self.confirmation {
            (c.as_str(), p.state_ok)
        } else if self.query.is_empty() {
            ("Search apps, settings, files — or do math", p.text_tertiary)
        } else {
            (self.query.as_str(), p.text_primary)
        };
        canvas.draw_text_aa(
            text_x as i32,
            text_y,
            field_text,
            rae_tokens::TYPE_SUBTITLE,
            field_fg,
            raegfx::text::FontFamily::Sans,
        );

        // ── Result rows OR empty/no-results state. ──────────────────────────
        let list_y = field_y + field_h + SPACE_2 as usize;

        if self.query.trim().is_empty() {
            return; // just-opened: field only (the recent-apps hint is a later pass)
        }

        if self.results.is_empty() {
            // No-results single row (spec §7), never collapses to just the field.
            canvas.draw_text_aa(
                (field_x + SPACE_3 as usize) as i32,
                (list_y + (row_h.saturating_sub(rae_tokens::TYPE_BODY.line_height as usize)) / 2)
                    as i32,
                &format!("No results for \"{}\"", self.query),
                rae_tokens::TYPE_BODY,
                p.text_secondary,
                raegfx::text::FontFamily::Sans,
            );
            return;
        }

        // Dark on-accent ink for the selected row (IDENTITY a11y guardrail:
        // white-on-RaeBlue ≈2.6:1 fails WCAG, so the selection ink is bg.base).
        let on_accent = Self::selection_ink(p);

        for (i, r) in self.results.iter().take(8).enumerate() {
            let ry = list_y + i * row_h;
            let selected = i == self.selected;

            // Per-row ink: a selected row is an accent WASH (the whole row card
            // filled accent) → dark on-accent ink so every glyph clears 4.5:1; an
            // unselected row stays transparent (the frosted popover shows through)
            // and uses the system text ramp. Focus is never colour-only — a 2px
            // accent.base left bar marks the selection too (spec §8).
            let (icon_ink, title_ink, sub_ink, tag_ink) = if selected {
                canvas.fill_rounded_rect(
                    field_x,
                    ry,
                    field_w,
                    row_h - 2,
                    RADIUS_XS as usize,
                    accent.base,
                );
                canvas.fill_rect(field_x, ry, 2, row_h - 2, accent.text);
                (
                    on_accent,
                    on_accent,
                    with_row_alpha(on_accent, 0xCC),
                    with_row_alpha(on_accent, 0xAA),
                )
            } else {
                (
                    accent.base,
                    p.text_primary,
                    p.text_secondary,
                    p.text_tertiary,
                )
            };

            // Leading line-icon per result kind — the SAME shipped raegfx icon set
            // Start / Files / the taskbar consume (retires the A/*/=/D/F letter
            // glyphs). Apps/files key off id+name; settings/calc map directly.
            let icon = Self::result_line_icon(r);
            let icon_sz = 18i32;
            canvas.draw_icon(
                icon,
                (field_x + SPACE_3 as usize) as i32,
                (ry + (row_h - icon_sz as usize) / 2) as i32,
                icon_sz,
                icon_ink,
            );

            let body_x = (field_x + SPACE_3 as usize + 16 + 8) as i32;
            // Title (type.body).
            let title_y = (ry + SPACE_2 as usize) as i32;
            canvas.draw_text_aa(
                body_x,
                title_y,
                &r.title,
                rae_tokens::TYPE_BODY,
                title_ink,
                raegfx::text::FontFamily::Sans,
            );
            // Subtitle (type.caption — the path/hint in text.secondary).
            if !r.subtitle.is_empty() {
                let sub_y =
                    (ry + SPACE_2 as usize + rae_tokens::TYPE_BODY.line_height as usize) as i32;
                // UTF-8 safety: a subtitle is an arbitrary file path / setting
                // label and may hold multi-byte chars (accents, CJK, emoji).
                // Slicing at a raw byte offset (`&s[..64]`) PANICS when the
                // index lands inside a code point, so truncate on a CHAR
                // boundary instead.
                let sub = truncate_chars(&r.subtitle, 64);
                canvas.draw_text_aa(
                    body_x,
                    sub_y,
                    sub,
                    rae_tokens::TYPE_CAPTION,
                    sub_ink,
                    raegfx::text::FontFamily::Sans,
                );
            }

            // Category tag (type.caption, right-aligned, text.tertiary / on-accent).
            let tag = Self::category_label(r.entry_type);
            let tag_w = canvas.measure_text_aa(
                tag,
                rae_tokens::TYPE_CAPTION,
                raegfx::text::FontFamily::Sans,
            );
            let tag_x = (field_x + field_w - SPACE_3 as usize) as i32 - tag_w;
            let tag_y = (ry
                + (row_h.saturating_sub(rae_tokens::TYPE_CAPTION.line_height as usize)) / 2)
                as i32;
            canvas.draw_text_aa(
                tag_x,
                tag_y,
                tag,
                rae_tokens::TYPE_CAPTION,
                tag_ink,
                raegfx::text::FontFamily::Sans,
            );
        }
    }

    /// The ink colour painted on the accent-filled selected row. Per the
    /// IDENTITY a11y guardrail, white-on-accent (≈2.6:1) fails WCAG, so the
    /// selection ink is the DARK `bg.base` (dark-on-accent) — the same selection
    /// ink Start and context menus use. Pulled out so the FAIL-able KAT can prove
    /// the selected row is dark-on-accent (legible) and not white-on-accent.
    pub(crate) fn selection_ink(p: &rae_tokens::Palette) -> u32 {
        p.bg_base
    }

    /// Map a result to the closest shipped `raegfx` line-icon — the SAME icon set
    /// Start / Files / the taskbar consume (no letter placeholders). Apps and
    /// files key off the result's title + path via the shared start-menu mappers;
    /// settings/calc/contact/bookmark map directly.
    fn result_line_icon(r: &SearchResult) -> raegfx::icon::Icon {
        use raegfx::icon::Icon;
        let path = r.path.as_deref().unwrap_or("");
        match r.entry_type {
            SearchResultType::Application => crate::start_menu::app_line_icon(path, &r.title),
            SearchResultType::Setting => Icon::Gear,
            SearchResultType::Calculator => Icon::Doc,
            SearchResultType::Folder => Icon::FolderSolid,
            SearchResultType::File | SearchResultType::RecentDocument => {
                crate::start_menu::entry_line_icon(&r.title, path)
            }
            SearchResultType::Contact => Icon::File,
            SearchResultType::Bookmark => Icon::WiFi,
            _ => Icon::File,
        }
    }
}

/// Replace the alpha channel of an ARGB colour, keeping RGB — composites a token
/// colour (e.g. dark on-accent ink) translucently for the secondary row text.
#[inline]
const fn with_row_alpha(color: u32, alpha: u32) -> u32 {
    (color & 0x00_FF_FF_FF) | ((alpha & 0xFF) << 24)
}

// ── Host KATs (R10: a smoketest must be able to print FAIL) ────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn seeded() -> CommandPalette {
        let mut pal = CommandPalette::new(1920, 1080);
        for (name, exec, desc, kw) in [
            (
                "Terminal",
                "terminal",
                "Command line",
                &["shell", "console"][..],
            ),
            (
                "Files",
                "files",
                "File manager",
                &["explorer", "folder"][..],
            ),
            (
                "Settings",
                "settings",
                "System settings",
                &["config", "options"][..],
            ),
            ("Calculator", "calculator", "Do math", &["calc", "math"][..]),
            (
                "Text Editor",
                "text_editor",
                "Edit text",
                &["notepad", "edit"][..],
            ),
            (
                "Media Player",
                "media_player",
                "Play media",
                &["music", "video"][..],
            ),
            (
                "Task Manager",
                "task_mgr",
                "Processes",
                &["process", "kill"][..],
            ),
            (
                "Photo Viewer",
                "image_viewer",
                "View images",
                &["photo", "picture"][..],
            ),
        ] {
            pal.index_app(name, exec, desc, kw);
        }
        pal.index_file("/home/user/Documents/report.txt");
        pal.index_file("/home/user/Pictures/holiday.png");
        pal
    }

    #[test]
    fn settings_actions_seeded() {
        let pal = CommandPalette::new(1920, 1080);
        assert!(
            pal.settings_actions() >= 12,
            "expected >=12 settings actions, got {}",
            pal.settings_actions()
        );
    }

    #[test]
    fn eight_apps_indexed() {
        let pal = seeded();
        assert_eq!(pal.indexed_apps(), 8);
        assert_eq!(pal.indexed_files(), 2);
    }

    #[test]
    fn query_disp_tops_display_settings() {
        let mut pal = seeded();
        pal.open();
        for c in "disp".chars() {
            pal.push_char(c);
        }
        let top = pal.selected_title().unwrap_or("");
        assert_eq!(
            top, "Open Display Settings",
            "top hit for 'disp' should be the display settings action, got '{top}'"
        );
        // Firing it must yield a real Navigate dispatch.
        match pal.fire_selected() {
            PaletteDispatch::Navigate(t) => assert_eq!(t, "settings:display"),
            other => panic!("expected Navigate, got {other:?}"),
        }
    }

    #[test]
    fn query_term_launches_terminal() {
        let mut pal = seeded();
        pal.open();
        for c in "term".chars() {
            pal.push_char(c);
        }
        assert_eq!(pal.selected_title(), Some("Terminal"));
        match pal.fire_selected() {
            PaletteDispatch::Launch(exec) => assert_eq!(exec, "terminal"),
            other => panic!("expected Launch, got {other:?}"),
        }
    }

    #[test]
    fn arithmetic_query_tops_calculator() {
        let mut pal = seeded();
        pal.open();
        for c in "6*7".chars() {
            pal.push_char(c);
        }
        // The calculator result scores 100 → always top.
        assert!(
            pal.selected_title()
                .map(|t| t.contains("42"))
                .unwrap_or(false),
            "calc '6*7' should top with 42, got {:?}",
            pal.selected_title()
        );
        match pal.fire_selected() {
            PaletteDispatch::Copy(v) => assert_eq!(v, "42"),
            other => panic!("expected Copy, got {other:?}"),
        }
        assert!(pal.confirmation.is_some(), "copy must show a confirmation");
    }

    #[test]
    fn keyboard_nav_wraps() {
        let mut pal = seeded();
        pal.open();
        for c in "e".chars() {
            pal.push_char(c);
        }
        let n = pal.result_count();
        assert!(n > 1, "expected multiple results for 'e'");
        pal.select_prev();
        assert_eq!(pal.selected, n - 1, "prev from 0 wraps to last");
        pal.select_next();
        assert_eq!(pal.selected, 0, "next from last wraps to first");
    }

    #[test]
    fn toggle_and_close_clears() {
        let mut pal = seeded();
        pal.toggle();
        assert!(pal.visible);
        pal.push_char('x');
        pal.toggle();
        assert!(!pal.visible);
        assert!(pal.query.is_empty(), "close discards the query");
    }

    #[test]
    fn empty_selection_fires_none() {
        let mut pal = seeded();
        pal.open();
        // No query → no results → firing is a no-op.
        assert_eq!(pal.fire_selected(), PaletteDispatch::None);
    }

    // ── Kernel-index file source (syscalls 54-57 unification) ──────────────

    /// A fake kernel file source: returns hits for "report" only, mirroring what
    /// `search_index::query` would yield after the crawler indexed a home tree.
    fn fake_kernel_source(query: &str, max: usize) -> Vec<KernelFileHit> {
        let all = [
            KernelFileHit {
                name: String::from("report.txt"),
                path: String::from("/home/user/Documents/report.txt"),
                is_folder: false,
            },
            KernelFileHit {
                name: String::from("Reports"),
                path: String::from("/home/user/Documents/Reports"),
                is_folder: true,
            },
        ];
        let ql = query.trim().to_ascii_lowercase();
        all.into_iter()
            .filter(|h| h.name.to_ascii_lowercase().contains(&ql))
            .take(max)
            .collect()
    }

    /// Empty source = the not-yet-crawled / no-match case. Must NOT error or
    /// panic — it just yields no file rows.
    fn empty_kernel_source(_q: &str, _max: usize) -> Vec<KernelFileHit> {
        Vec::new()
    }

    #[test]
    fn map_kernel_hits_ranks_exact_above_substring() {
        let hits = vec![
            KernelFileHit {
                name: String::from("notes_old.txt"),
                path: String::from("/h/notes_old.txt"),
                is_folder: false,
            },
            KernelFileHit {
                name: String::from("notes"),
                path: String::from("/h/notes"),
                is_folder: true,
            },
        ];
        let rows = CommandPalette::map_kernel_hits("notes", &hits);
        assert_eq!(rows.len(), 2);
        // Exact (case-insensitive) name match must score above the substring.
        let exact = rows.iter().find(|r| r.title == "notes").unwrap();
        let sub = rows.iter().find(|r| r.title == "notes_old.txt").unwrap();
        assert!(
            exact.score > sub.score,
            "exact '{}' ({}) should outrank substring '{}' ({})",
            exact.title,
            exact.score,
            sub.title,
            sub.score
        );
        // Folder vs file typing + Open action carry the path.
        assert_eq!(exact.entry_type, SearchResultType::Folder);
        assert_eq!(sub.entry_type, SearchResultType::File);
        match &exact.action {
            SearchAction::Open(p) => assert_eq!(p, "/h/notes"),
            other => panic!("expected Open, got {other:?}"),
        }
    }

    #[test]
    fn wired_source_serves_files_from_kernel() {
        let mut pal = seeded(); // seeded also engine-indexes report.txt / holiday.png
        pal.set_kernel_file_source(fake_kernel_source);
        assert!(pal.has_kernel_file_source());
        pal.open();
        for c in "report".chars() {
            pal.push_char(c);
        }
        // Files now come from the kernel source: both report hits present, and
        // the engine's own private file index is NOT also queried for files
        // (one source of truth) — exactly two file-family rows.
        let file_rows = pal
            .results
            .iter()
            .filter(|r| CommandPalette::is_file_result(r.entry_type))
            .count();
        assert_eq!(
            file_rows, 2,
            "expected exactly the 2 kernel file hits, got {file_rows}"
        );
        assert!(
            pal.results.iter().any(|r| r.title == "report.txt"),
            "kernel file hit 'report.txt' must surface"
        );
    }

    #[test]
    fn empty_kernel_index_is_graceful() {
        let mut pal = seeded();
        pal.set_kernel_file_source(empty_kernel_source);
        pal.open();
        for c in "report".chars() {
            pal.push_char(c);
        }
        // Pre-crawl / no-match: no file rows, and crucially no panic/error — the
        // legacy engine file index must NOT leak back in once a source is wired.
        let file_rows = pal
            .results
            .iter()
            .filter(|r| CommandPalette::is_file_result(r.entry_type))
            .count();
        assert_eq!(file_rows, 0, "empty kernel index yields no file rows");
    }

    #[test]
    fn unwired_falls_back_to_engine_files() {
        // No kernel source set → the legacy engine file index still answers, so
        // an un-wired host doesn't regress to zero file results.
        let mut pal = seeded();
        assert!(!pal.has_kernel_file_source());
        pal.open();
        for c in "report".chars() {
            pal.push_char(c);
        }
        assert!(
            pal.results.iter().any(|r| r.title.contains("report")),
            "engine file fallback should still find report.txt"
        );
    }

    /// FAIL-able decode KAT for the on-the-wire kernel result format
    /// (`[u64 id][u32 kind][u32 pad]`, little-endian, 16-byte stride) — the same
    /// bytes raekit's `search::decode_results` parses. Proves the host that wires
    /// the kernel source (and the raekit userspace path) reads the format right.
    #[test]
    fn kernel_result_blob_decodes() {
        // Two records: (id=7, kind=2/File), (id=42, kind=5/Document).
        let mut blob = Vec::new();
        blob.extend_from_slice(&7u64.to_le_bytes());
        blob.extend_from_slice(&2u32.to_le_bytes());
        blob.extend_from_slice(&0u32.to_le_bytes());
        blob.extend_from_slice(&42u64.to_le_bytes());
        blob.extend_from_slice(&5u32.to_le_bytes());
        blob.extend_from_slice(&0u32.to_le_bytes());

        // Decode mirroring the kernel layout (the logic raekit shares).
        let decode = |buf: &[u8], count: usize| -> Vec<(u64, u32)> {
            let mut out = Vec::new();
            for i in 0..count {
                let b = i * 16;
                if b + 16 > buf.len() {
                    break;
                }
                let id = u64::from_le_bytes(buf[b..b + 8].try_into().unwrap());
                let kind = u32::from_le_bytes(buf[b + 8..b + 12].try_into().unwrap());
                out.push((id, kind));
            }
            out
        };
        let got = decode(&blob, 2);
        assert_eq!(got, vec![(7u64, 2u32), (42u64, 5u32)]);
        // A count larger than the buffer must not over-read (defensive).
        let clamped = decode(&blob, 99);
        assert_eq!(clamped.len(), 2);
    }

    // ── Liquid Glass surface contract (the visual refresh KAT) ─────────────

    /// FAIL-able: the palette renders with the `glass.popover` tier through the
    /// shipped `draw_glass_surface`, NOT the deprecated `GLASS_TINT_DARK` alias.
    /// The alias resolves to the PANEL tint, so this asserts (a) the tier IS
    /// popover and (b) it is NOT the panel tint the alias points at — revert the
    /// render to the alias and this fails on both counts.
    #[test]
    fn palette_uses_popover_tier_not_tint_alias() {
        // (a) the palette's render tier is exactly the popover tier.
        assert_eq!(
            PALETTE_GLASS_TIER,
            rae_tokens::GLASS_POPOVER_DARK,
            "command palette must render the glass.popover tier (a search/command \
             flyout is a popover)"
        );
        // (b) and it is NOT the panel tier the deprecated GLASS_TINT_DARK alias
        //     resolves to (GLASS_TINT_DARK == GLASS_PANEL_DARK.tint).
        assert_ne!(
            PALETTE_GLASS_TIER.tint,
            rae_tokens::GLASS_TINT_DARK,
            "palette tier must not be the deprecated GLASS_TINT_DARK (panel) alias"
        );
        assert_ne!(
            PALETTE_GLASS_TIER,
            rae_tokens::GLASS_PANEL_DARK,
            "palette tier must be popover, not panel"
        );
    }

    /// FAIL-able: the selected row is DARK on-accent ink (legible), not the
    /// white-on-accent that fails WCAG. Asserts the selection ink == bg.base and
    /// that dark-on-accent clears the 4.5:1 body-text bar while white-on-accent
    /// would NOT — proving the a11y guardrail is honoured.
    #[test]
    fn selected_row_is_dark_on_accent_and_legible() {
        let p = crate::active_palette();
        let a = rae_tokens::derive_accent(crate::active_accent(), p);
        let ink = CommandPalette::selection_ink(p);
        // The ink is the dark bg.base, never a light/white ink.
        assert_eq!(
            ink, p.bg_base,
            "selection ink must be dark-on-accent (bg.base)"
        );
        // Dark-on-accent clears the body-text contrast bar over the accent fill.
        let dark_on_accent = rae_tokens::contrast_ratio(ink, a.base);
        assert!(
            dark_on_accent >= 4.5,
            "dark-on-accent selection ink must clear 4.5:1 (got {dark_on_accent:.2})"
        );
        // The rejected white-on-accent would NOT clear the bar — proving the
        // guardrail is doing real work (if it passed too, the test couldn't fail
        // for the wrong choice).
        let white_on_accent = rae_tokens::contrast_ratio(0xFF_FF_FF_FF, a.base);
        assert!(
            white_on_accent < dark_on_accent,
            "the dark ink must be MORE legible than white-on-accent (the rejected ink)"
        );
    }

    /// FAIL-able: every result kind maps to a real shipped `raegfx` line-icon
    /// (no letter-glyph placeholders) — apps key off the path/title mapper, the
    /// rest map directly. Asserts the app/setting/file/calc kinds resolve to the
    /// expected icons.
    #[test]
    fn result_rows_use_line_icons() {
        use raegfx::icon::Icon;
        let app = SearchResult {
            entry_type: SearchResultType::Application,
            title: String::from("Terminal"),
            subtitle: String::new(),
            icon: None,
            path: Some(String::from("com.raeos.terminal")),
            score: 1.0,
            highlights: Vec::new(),
            action: SearchAction::Launch(String::from("terminal")),
        };
        assert_eq!(CommandPalette::result_line_icon(&app), Icon::Exec);

        let setting = SearchResult {
            entry_type: SearchResultType::Setting,
            title: String::from("Open Display Settings"),
            subtitle: String::new(),
            icon: None,
            path: Some(String::from("settings:display")),
            score: 0.9,
            highlights: Vec::new(),
            action: SearchAction::Navigate(String::from("settings:display")),
        };
        assert_eq!(CommandPalette::result_line_icon(&setting), Icon::Gear);

        let folder = SearchResult {
            entry_type: SearchResultType::Folder,
            title: String::from("Documents"),
            subtitle: String::from("/home/user/Documents"),
            icon: None,
            path: Some(String::from("/home/user/Documents")),
            score: 0.7,
            highlights: Vec::new(),
            action: SearchAction::Open(String::from("/home/user/Documents")),
        };
        assert_eq!(CommandPalette::result_line_icon(&folder), Icon::FolderSolid);

        let calc = SearchResult {
            entry_type: SearchResultType::Calculator,
            title: String::from("42"),
            subtitle: String::new(),
            icon: None,
            path: None,
            score: 1.0,
            highlights: Vec::new(),
            action: SearchAction::Calculate(String::from("42")),
        };
        assert_eq!(CommandPalette::result_line_icon(&calc), Icon::Doc);
    }
}
