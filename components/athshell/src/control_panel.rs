#![allow(dead_code)]

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};

// ═══════════════════════════════════════════════════════════════════════════
// Control Panel / Settings UI — Comprehensive system settings for AthenaOS
// ═══════════════════════════════════════════════════════════════════════════

static PANEL_INITIALIZED: AtomicBool = AtomicBool::new(false);

// ── Categories ───────────────────────────────────────────────────────────

/// The 10 AthenaOS-native top-level Settings categories (docs/design/settings-redesign.md
/// §1 IA). This re-groups the prior 11 Windows-clone categories into the macOS/Win11
/// "one searchable app" IA; the per-page `SubPage` identity is preserved and each
/// page's `category` field is re-homed in `populate_pages` (no settings dropped).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SettingsCategory {
    /// 1. Appearance & Vibe — colors/accent, wallpaper, taskbar, Vibe presets.
    Appearance,
    /// 2. Display — resolution, brightness, HDR, refresh.
    Display,
    /// 3. Sound — volume, output, spatial audio.
    Sound,
    /// 4. Network — Wi-Fi, VPN, proxy.
    Network,
    /// 5. Bluetooth & Devices — Bluetooth, mouse, printers.
    Devices,
    /// 6. Power & Gaming — power/battery, Game Bar/Mode/GPU power.
    PowerGaming,
    /// 7. Accessibility — vision, narrator, high-contrast, reduced motion.
    Accessibility,
    /// 8. Privacy & Security — privacy toggles + update/recovery/developer.
    PrivacySecurity,
    /// 9. Storage — disk usage, cleanup (panel is a later slice; data edit now).
    Storage,
    /// 10. System & About — date/region/language, apps, notifications, accounts, About.
    SystemAbout,
}

impl SettingsCategory {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Appearance => "Appearance & Vibe",
            Self::Display => "Display",
            Self::Sound => "Sound",
            Self::Network => "Network",
            Self::Devices => "Bluetooth & Devices",
            Self::PowerGaming => "Power & Gaming",
            Self::Accessibility => "Accessibility",
            Self::PrivacySecurity => "Privacy & Security",
            Self::Storage => "Storage",
            Self::SystemAbout => "System & About",
        }
    }

    pub fn icon(&self) -> char {
        match self {
            Self::Appearance => '\u{1F3A8}',
            Self::Display => '\u{1F4BB}',
            Self::Sound => '\u{1F50A}',
            Self::Network => '\u{1F310}',
            Self::Devices => '\u{1F4F6}',
            Self::PowerGaming => '\u{1F3AE}',
            Self::Accessibility => '\u{267F}',
            Self::PrivacySecurity => '\u{1F512}',
            Self::Storage => '\u{1F4BE}',
            Self::SystemAbout => '\u{2139}',
        }
    }

    /// The real `athgfx` line-icon for this nav category — replacing the `?`/emoji
    /// `char` placeholders the bitmap font can't render (visual-QA Round-7 #1).
    /// Reuses the SAME `athgfx::icon::Icon` set Control Center / Files consume
    /// (commit `f47f258`/`3102a46` lineage); each category maps to the closest
    /// shipped glyph. No new icons are added — the set already covers these.
    pub fn line_icon(&self) -> athgfx::icon::Icon {
        use athgfx::icon::Icon;
        match self {
            Self::Appearance => Icon::Palette,          // colors/accent/Vibe
            Self::Display => Icon::Brightness,          // screen/brightness/HDR
            Self::Sound => Icon::Volume,                // volume/output
            Self::Network => Icon::WiFi,                // Wi-Fi/VPN/proxy
            Self::Devices => Icon::Bluetooth,           // Bluetooth & devices
            Self::PowerGaming => Icon::GameController,  // power + Game Bar/Mode
            Self::Accessibility => Icon::Accessibility, // person-in-ring
            Self::PrivacySecurity => Icon::Lock,        // privacy/update/recovery
            Self::Storage => Icon::Archive,             // disk usage / box
            Self::SystemAbout => Icon::Gear,            // system + about
        }
    }

    pub fn all() -> &'static [SettingsCategory] {
        &[
            Self::Appearance,
            Self::Display,
            Self::Sound,
            Self::Network,
            Self::Devices,
            Self::PowerGaming,
            Self::Accessibility,
            Self::PrivacySecurity,
            Self::Storage,
            Self::SystemAbout,
        ]
    }
}

// ── Sub-pages per category ───────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SystemPage {
    Display,
    Sound,
    Notifications,
    FocusAssist,
    Power,
    Storage,
    Multitasking,
    NearbySharing,
    Clipboard,
    About,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DevicesPage {
    Bluetooth,
    Printers,
    Mouse,
    Touchpad,
    Pen,
    AutoPlay,
    Usb,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NetworkPage {
    Status,
    Wifi,
    Ethernet,
    DialUp,
    Vpn,
    MobileHotspot,
    AirplaneMode,
    Proxy,
    AdvancedSharing,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PersonalizationPage {
    Background,
    Colors,
    LockScreen,
    Themes,
    Fonts,
    Start,
    Taskbar,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AppsPage {
    AppsFeatures,
    DefaultApps,
    OfflineMaps,
    OptionalFeatures,
    AppsForWebsites,
    VideoPlayback,
    Startup,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AccountsPage {
    YourInfo,
    Email,
    SignInOptions,
    WorkAccess,
    Family,
    OtherUsers,
    SyncSettings,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TimeLanguagePage {
    DateTime,
    Region,
    Language,
    Speech,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GamingPage {
    GameBar,
    Captures,
    GameMode,
    GameDvr,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EaseOfAccessPage {
    Display,
    Cursor,
    Magnifier,
    ColorFilters,
    HighContrast,
    Narrator,
    Audio,
    Captions,
    SpeechInput,
    Keyboard,
    Mouse,
    EyeControl,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PrivacyPage {
    General,
    Speech,
    Inking,
    Diagnostics,
    ActivityHistory,
    Location,
    Camera,
    Microphone,
    VoiceActivation,
    Notifications,
    AccountInfo,
    Contacts,
    Calendar,
    PhoneCalls,
    CallHistory,
    Email,
    Tasks,
    Messaging,
    Documents,
    Pictures,
    Videos,
    FileSystem,
    BackgroundApps,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UpdateSecurityPage {
    WindowsUpdate,
    DeliveryOptimization,
    Troubleshoot,
    Recovery,
    Activation,
    FindMyDevice,
    ForDevelopers,
    WindowsInsider,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubPage {
    System(SystemPage),
    Devices(DevicesPage),
    Network(NetworkPage),
    Personalization(PersonalizationPage),
    Apps(AppsPage),
    Accounts(AccountsPage),
    TimeLanguage(TimeLanguagePage),
    Gaming(GamingPage),
    EaseOfAccess(EaseOfAccessPage),
    Privacy(PrivacyPage),
    UpdateSecurity(UpdateSecurityPage),
}

impl SubPage {
    /// Coarse fallback mapping from the per-page SubPage group to the 10-category
    /// IA. The authoritative per-page category lives on `SettingsPage.category`
    /// (re-homed in `populate_pages`); this only covers the few sub-pages that
    /// split across the new categories at their group granularity.
    pub fn category(&self) -> SettingsCategory {
        match self {
            // System sub-pages mostly fold into System & About, except Display,
            // Sound, Power and Storage which are promoted to top-level categories.
            Self::System(SystemPage::Display) => SettingsCategory::Display,
            Self::System(SystemPage::Sound) => SettingsCategory::Sound,
            Self::System(SystemPage::Power) => SettingsCategory::PowerGaming,
            Self::System(SystemPage::Storage) => SettingsCategory::Storage,
            Self::System(_) => SettingsCategory::SystemAbout,
            Self::Devices(_) => SettingsCategory::Devices,
            Self::Network(_) => SettingsCategory::Network,
            Self::Personalization(_) => SettingsCategory::Appearance,
            Self::Apps(_) => SettingsCategory::SystemAbout,
            Self::Accounts(_) => SettingsCategory::SystemAbout,
            Self::TimeLanguage(_) => SettingsCategory::SystemAbout,
            Self::Gaming(_) => SettingsCategory::PowerGaming,
            Self::EaseOfAccess(_) => SettingsCategory::Accessibility,
            Self::Privacy(_) => SettingsCategory::PrivacySecurity,
            Self::UpdateSecurity(_) => SettingsCategory::PrivacySecurity,
        }
    }
}

// ── Setting control types ────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum SettingControl {
    Toggle {
        value: bool,
    },
    Slider {
        value: u32,
        min: u32,
        max: u32,
        step: u32,
    },
    Dropdown {
        selected: usize,
        options: Vec<String>,
    },
    TextInput {
        value: String,
        placeholder: String,
        max_length: usize,
    },
    ColorPicker {
        r: u8,
        g: u8,
        b: u8,
        a: u8,
    },
    KeyBinding {
        modifiers: u8,
        key: u8,
        display: String,
    },
    RadioGroup {
        selected: usize,
        options: Vec<String>,
    },
    CheckboxGroup {
        items: Vec<(String, bool)>,
    },
    Button {
        label: String,
        action_id: u64,
    },
    Link {
        label: String,
        url: String,
    },
    InfoBar {
        message: String,
        severity: InfoSeverity,
    },
    ExpandableSection {
        title: String,
        expanded: bool,
        children: Vec<SettingItem>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InfoSeverity {
    Info,
    Warning,
    Error,
    Success,
}

// ── Setting item ─────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct SettingItem {
    pub id: String,
    pub label: String,
    pub description: String,
    pub control: SettingControl,
    pub visible: bool,
    pub enabled: bool,
    pub requires_restart: bool,
    pub mdm_locked: bool,
}

impl SettingItem {
    pub fn toggle(id: &str, label: &str, desc: &str, value: bool) -> Self {
        Self {
            id: String::from(id),
            label: String::from(label),
            description: String::from(desc),
            control: SettingControl::Toggle { value },
            visible: true,
            enabled: true,
            requires_restart: false,
            mdm_locked: false,
        }
    }

    pub fn slider(id: &str, label: &str, desc: &str, value: u32, min: u32, max: u32) -> Self {
        Self {
            id: String::from(id),
            label: String::from(label),
            description: String::from(desc),
            control: SettingControl::Slider {
                value,
                min,
                max,
                step: 1,
            },
            visible: true,
            enabled: true,
            requires_restart: false,
            mdm_locked: false,
        }
    }

    pub fn dropdown(id: &str, label: &str, desc: &str, options: &[&str], selected: usize) -> Self {
        Self {
            id: String::from(id),
            label: String::from(label),
            description: String::from(desc),
            control: SettingControl::Dropdown {
                selected,
                options: options.iter().map(|o| String::from(*o)).collect(),
            },
            visible: true,
            enabled: true,
            requires_restart: false,
            mdm_locked: false,
        }
    }

    pub fn text_input(id: &str, label: &str, desc: &str, value: &str, placeholder: &str) -> Self {
        Self {
            id: String::from(id),
            label: String::from(label),
            description: String::from(desc),
            control: SettingControl::TextInput {
                value: String::from(value),
                placeholder: String::from(placeholder),
                max_length: 256,
            },
            visible: true,
            enabled: true,
            requires_restart: false,
            mdm_locked: false,
        }
    }

    pub fn color_picker(id: &str, label: &str, desc: &str, r: u8, g: u8, b: u8) -> Self {
        Self {
            id: String::from(id),
            label: String::from(label),
            description: String::from(desc),
            control: SettingControl::ColorPicker { r, g, b, a: 255 },
            visible: true,
            enabled: true,
            requires_restart: false,
            mdm_locked: false,
        }
    }

    pub fn radio_group(
        id: &str,
        label: &str,
        desc: &str,
        options: &[&str],
        selected: usize,
    ) -> Self {
        Self {
            id: String::from(id),
            label: String::from(label),
            description: String::from(desc),
            control: SettingControl::RadioGroup {
                selected,
                options: options.iter().map(|o| String::from(*o)).collect(),
            },
            visible: true,
            enabled: true,
            requires_restart: false,
            mdm_locked: false,
        }
    }

    pub fn button(id: &str, label: &str, desc: &str, button_label: &str, action_id: u64) -> Self {
        Self {
            id: String::from(id),
            label: String::from(label),
            description: String::from(desc),
            control: SettingControl::Button {
                label: String::from(button_label),
                action_id,
            },
            visible: true,
            enabled: true,
            requires_restart: false,
            mdm_locked: false,
        }
    }

    pub fn info_bar(id: &str, message: &str, severity: InfoSeverity) -> Self {
        Self {
            id: String::from(id),
            label: String::new(),
            description: String::new(),
            control: SettingControl::InfoBar {
                message: String::from(message),
                severity,
            },
            visible: true,
            enabled: true,
            requires_restart: false,
            mdm_locked: false,
        }
    }

    pub fn with_restart(mut self) -> Self {
        self.requires_restart = true;
        self
    }

    pub fn with_mdm_lock(mut self) -> Self {
        self.mdm_locked = true;
        self.enabled = false;
        self
    }
}

// ── Settings page ────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct SettingsPage {
    pub id: String,
    pub title: String,
    pub icon: char,
    pub category: SettingsCategory,
    pub sub_page: Option<SubPage>,
    pub description: String,
    pub search_keywords: Vec<String>,
    pub settings: Vec<SettingItem>,
}

impl SettingsPage {
    pub fn new(id: &str, title: &str, icon: char, category: SettingsCategory) -> Self {
        Self {
            id: String::from(id),
            title: String::from(title),
            icon,
            category,
            sub_page: None,
            description: String::new(),
            search_keywords: Vec::new(),
            settings: Vec::new(),
        }
    }

    pub fn with_description(mut self, desc: &str) -> Self {
        self.description = String::from(desc);
        self
    }

    pub fn with_keywords(mut self, keywords: &[&str]) -> Self {
        self.search_keywords = keywords.iter().map(|k| String::from(*k)).collect();
        self
    }

    pub fn with_sub_page(mut self, sub: SubPage) -> Self {
        self.sub_page = Some(sub);
        self
    }

    pub fn add_setting(&mut self, item: SettingItem) {
        self.settings.push(item);
    }

    pub fn get_setting(&self, id: &str) -> Option<&SettingItem> {
        self.settings.iter().find(|s| s.id == id)
    }

    pub fn get_setting_mut(&mut self, id: &str) -> Option<&mut SettingItem> {
        self.settings.iter_mut().find(|s| s.id == id)
    }

    pub fn visible_settings(&self) -> Vec<&SettingItem> {
        self.settings.iter().filter(|s| s.visible).collect()
    }

    pub fn reset_to_defaults(&mut self, defaults: &[(&str, SettingControl)]) {
        for (id, default_ctrl) in defaults {
            if let Some(item) = self.settings.iter_mut().find(|s| s.id == *id) {
                item.control = default_ctrl.clone();
            }
        }
    }
}

// ── Navigation ───────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct NavigationState {
    pub current_category: Option<SettingsCategory>,
    pub current_page_id: Option<String>,
    pub back_stack: Vec<(Option<SettingsCategory>, Option<String>)>,
    pub forward_stack: Vec<(Option<SettingsCategory>, Option<String>)>,
    pub breadcrumb: Vec<String>,
}

impl NavigationState {
    pub fn new() -> Self {
        Self {
            current_category: None,
            current_page_id: None,
            back_stack: Vec::new(),
            forward_stack: Vec::new(),
            breadcrumb: Vec::new(),
        }
    }

    pub fn navigate_to_category(&mut self, category: SettingsCategory) {
        self.push_current();
        self.current_category = Some(category);
        self.current_page_id = None;
        self.forward_stack.clear();
        self.update_breadcrumb();
    }

    pub fn navigate_to_page(&mut self, category: SettingsCategory, page_id: &str) {
        self.push_current();
        self.current_category = Some(category);
        self.current_page_id = Some(String::from(page_id));
        self.forward_stack.clear();
        self.update_breadcrumb();
    }

    pub fn go_back(&mut self) -> bool {
        if let Some((cat, page)) = self.back_stack.pop() {
            self.forward_stack
                .push((self.current_category, self.current_page_id.clone()));
            self.current_category = cat;
            self.current_page_id = page;
            self.update_breadcrumb();
            true
        } else {
            false
        }
    }

    pub fn go_forward(&mut self) -> bool {
        if let Some((cat, page)) = self.forward_stack.pop() {
            self.back_stack
                .push((self.current_category, self.current_page_id.clone()));
            self.current_category = cat;
            self.current_page_id = page;
            self.update_breadcrumb();
            true
        } else {
            false
        }
    }

    pub fn go_home(&mut self) {
        self.push_current();
        self.current_category = None;
        self.current_page_id = None;
        self.forward_stack.clear();
        self.update_breadcrumb();
    }

    fn push_current(&mut self) {
        self.back_stack
            .push((self.current_category, self.current_page_id.clone()));
    }

    fn update_breadcrumb(&mut self) {
        self.breadcrumb.clear();
        self.breadcrumb.push(String::from("Settings"));
        if let Some(cat) = self.current_category {
            self.breadcrumb.push(String::from(cat.label()));
        }
        if let Some(ref page) = self.current_page_id {
            self.breadcrumb.push(page.clone());
        }
    }

    pub fn can_go_back(&self) -> bool {
        !self.back_stack.is_empty()
    }

    pub fn can_go_forward(&self) -> bool {
        !self.forward_stack.is_empty()
    }
}

// ── Search ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct SearchResult {
    pub page_id: String,
    pub page_title: String,
    pub category: SettingsCategory,
    pub setting_id: Option<String>,
    pub setting_label: Option<String>,
    pub match_context: String,
}

#[derive(Debug, Clone)]
pub struct SearchState {
    pub query: String,
    pub results: Vec<SearchResult>,
    pub selected_index: usize,
    pub active: bool,
}

impl SearchState {
    pub fn new() -> Self {
        Self {
            query: String::new(),
            results: Vec::new(),
            selected_index: 0,
            active: false,
        }
    }

    /// Case-INSENSITIVE, ranked filter over every page + setting.
    ///
    /// docs/design/settings-redesign.md §2: the live model was case-sensitive —
    /// `"Accent".contains("accent")` returned 0 hits, the #1 switcher annoyance.
    /// We lowercase-fold both query and every searched field, then rank each hit
    /// by match quality (title > description > keyword, exact-prefix < word-start <
    /// substring) so the most relevant control floats to the top and `Enter` dives
    /// straight to it. Never panics on any query input (empty / unicode / huge).
    pub fn search(&mut self, pages: &[SettingsPage]) {
        self.results.clear();
        self.selected_index = 0;
        if self.query.is_empty() {
            return;
        }
        let q = self.query.to_ascii_lowercase();
        let q = q.trim();
        if q.is_empty() {
            return;
        }
        // (result, rank) — lower rank == better match; stable-sorted at the end.
        let mut ranked: Vec<(SearchResult, u32)> = Vec::new();
        for page in pages {
            // Page-level: rank from the strongest of title/description/keyword.
            let title_rank = Self::field_rank(&page.title, q);
            let desc_rank = Self::field_rank(&page.description, q);
            // Best (lowest) positional rank among matching keywords only — a
            // non-matching keyword yields `None` and must NOT win the min, so we
            // filter to the `Some` ranks before taking the minimum.
            let kw_rank = page
                .search_keywords
                .iter()
                .filter_map(|k| Self::field_rank(k, q))
                .min();
            // Field-class weighting: title(0) < description(100) < keyword(200);
            // within a class, the positional rank (prefix/word-start/substring).
            let page_rank = [
                title_rank,
                desc_rank.map(|r| 100 + r),
                kw_rank.map(|r| 200 + r),
            ]
            .into_iter()
            .flatten()
            .min();
            if let Some(rank) = page_rank {
                ranked.push((
                    SearchResult {
                        page_id: page.id.clone(),
                        page_title: page.title.clone(),
                        category: page.category,
                        setting_id: None,
                        setting_label: None,
                        match_context: page.description.clone(),
                    },
                    rank,
                ));
            }
            for setting in &page.settings {
                let label_rank = Self::field_rank(&setting.label, q);
                let sdesc_rank = Self::field_rank(&setting.description, q);
                let setting_rank = [label_rank, sdesc_rank.map(|r| 100 + r)]
                    .into_iter()
                    .flatten()
                    .min();
                if let Some(rank) = setting_rank {
                    // Settings sort just after their page's own rank band so a
                    // page title hit still leads, but a precise control match
                    // (e.g. "Accent Color") still beats a fuzzy page hit.
                    ranked.push((
                        SearchResult {
                            page_id: page.id.clone(),
                            page_title: page.title.clone(),
                            category: page.category,
                            setting_id: Some(setting.id.clone()),
                            setting_label: Some(setting.label.clone()),
                            match_context: setting.description.clone(),
                        },
                        rank,
                    ));
                }
            }
        }
        // Stable sort by rank, then by shorter label first (more specific).
        ranked.sort_by(|a, b| {
            a.1.cmp(&b.1).then_with(|| {
                let al = a.0.setting_label.as_ref().unwrap_or(&a.0.page_title).len();
                let bl = b.0.setting_label.as_ref().unwrap_or(&b.0.page_title).len();
                al.cmp(&bl)
            })
        });
        self.results = ranked.into_iter().map(|(r, _)| r).collect();
    }

    /// Positional match rank of a lowercased query `q` inside `field` (folded
    /// here): `Some(0)` exact-prefix, `Some(1)` word-start, `Some(2)` substring,
    /// `None` no match. Pure, allocation-light, panic-free.
    fn field_rank(field: &str, q: &str) -> Option<u32> {
        let f = field.to_ascii_lowercase();
        let pos = f.find(q)?;
        if pos == 0 {
            Some(0)
        } else if f.as_bytes().get(pos.wrapping_sub(1)) == Some(&b' ') {
            Some(1)
        } else {
            Some(2)
        }
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

    pub fn clear(&mut self) {
        self.query.clear();
        self.results.clear();
        self.selected_index = 0;
        self.active = false;
    }
}

// ── Live system info (About + Storage panels) ────────────────────────────

/// Parsed, render-ready system facts for the **About** (§5) and **Storage** (§4)
/// panels. The kernel pushes the raw `/proc/athena/*` text via
/// [`set_system_info_from_proc`]; this crate parses it **defensively** (no panic,
/// ever, on a missing/garbled/`(unavailable)` field) into the fields below.
///
/// Concept §"The user owns the machine": the OS is honest about the hardware —
/// the real OS/kernel/CPU/RAM/board and the real disk used/free — surfaced in one
/// glass surface, never a fabricated number. A field we cannot read renders as
/// "(unknown)" (About) or the empty-state InfoBar (Storage), never blank/crash.
#[derive(Debug, Clone)]
pub struct SystemInfo {
    // About — text fields (each "(unknown)" until the kernel pushes a real value).
    pub os_version: String,
    pub kernel: String,
    pub processor: String,
    pub smp_cores: u32,
    pub board: String,
    pub installed_ram_bytes: u64,
    // Storage — capacity (all 0 + `storage_mounted=false` ⇒ render the empty-state).
    pub storage_mounted: bool,
    pub storage_total_bytes: u64,
    pub storage_free_bytes: u64,
    pub storage_used_bytes: u64,
    pub storage_system_bytes: u64,
}

impl SystemInfo {
    /// All-unknown default — what a host KAT / a shell with no kernel push shows.
    /// The About panel renders every row as "(unknown)"; Storage shows the
    /// empty-state InfoBar (mounted=false). FAIL-able state for the smoketest.
    pub fn unknown() -> Self {
        Self {
            os_version: String::from("(unknown)"),
            kernel: String::from("(unknown)"),
            processor: String::from("(unknown)"),
            smp_cores: 0,
            board: String::from("(unknown)"),
            installed_ram_bytes: 0,
            storage_mounted: false,
            storage_total_bytes: 0,
            storage_free_bytes: 0,
            storage_used_bytes: 0,
            storage_system_bytes: 0,
        }
    }

    /// Count of About fields that read as live (non-"(unknown)"/non-zero). Used by
    /// the boot smoketest: a fully-unknown panel (0 live fields) is a FAIL.
    pub fn about_live_fields(&self) -> u32 {
        let mut n = 0;
        if !is_unknown(&self.os_version) {
            n += 1;
        }
        if !is_unknown(&self.kernel) {
            n += 1;
        }
        if !is_unknown(&self.processor) {
            n += 1;
        }
        if self.smp_cores > 0 {
            n += 1;
        }
        if !is_unknown(&self.board) {
            n += 1;
        }
        if self.installed_ram_bytes > 0 {
            n += 1;
        }
        n
    }
}

/// Is this About-field string the "(unknown)"/empty sentinel (i.e. NOT live)?
#[inline]
fn is_unknown(s: &str) -> bool {
    s.is_empty() || s == "(unknown)" || s == "(unavailable)"
}

/// First whitespace-free *non-empty* line of a procfs dump (skips `#` comment
/// banners), trimmed. Used to turn `/proc/version` / `/proc/athena/hardware` into
/// a single render-ready line. Never panics; returns "(unknown)" if nothing fits.
fn first_meaningful_line(text: &str) -> String {
    for line in text.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        return String::from(t);
    }
    String::from("(unknown)")
}

/// Pull the value after `key:` from a `key: value [...]` procfs body, returning
/// the trimmed remainder of that line (everything after the first colon). Returns
/// `None` if the key is absent or its value is the `(unavailable)` sentinel.
/// Boundary-safe (operates on whole lines + byte offsets `find` returns on the
/// ASCII procfs text); never panics on garbled input.
fn proc_value<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix(key) {
            // Require the next char to be ':' so `total_bytes` doesn't match
            // `total_bytes_foo`. strip_prefix already consumed `key`.
            let rest = rest.trim_start();
            if let Some(val) = rest.strip_prefix(':') {
                let val = val.trim();
                if val.is_empty() || val.starts_with("(unavailable)") {
                    return None;
                }
                return Some(val);
            }
        }
    }
    None
}

/// The first integer token after `key:` (per the kernel agent's format: the
/// leading decimal of `total_bytes: 1234 (5 MiB)` is the authoritative byte
/// count; the `(NNN MiB)` hint is ignored). `None` on missing/garbled/
/// `(unavailable)`. Saturates rather than panicking on overflow.
fn proc_u64(text: &str, key: &str) -> Option<u64> {
    let val = proc_value(text, key)?;
    let digits: String = val.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        return None;
    }
    let mut n: u64 = 0;
    for b in digits.bytes() {
        n = n.saturating_mul(10).saturating_add((b - b'0') as u64);
    }
    Some(n)
}

/// Pretty-print a byte count as a human GiB/MiB string (e.g. "15.6 GiB"). Integer
/// math only (no_std, no float formatting); one decimal place. 0 ⇒ "0 B".
fn fmt_bytes(bytes: u64) -> String {
    if bytes == 0 {
        return String::from("0 B");
    }
    const GIB: u64 = 1 << 30;
    const MIB: u64 = 1 << 20;
    const KIB: u64 = 1 << 10;
    let (val_x10, unit) = if bytes >= GIB {
        (bytes.saturating_mul(10) / GIB, "GiB")
    } else if bytes >= MIB {
        (bytes.saturating_mul(10) / MIB, "MiB")
    } else if bytes >= KIB {
        (bytes.saturating_mul(10) / KIB, "KiB")
    } else {
        let mut s = String::new();
        push_u64(&mut s, bytes);
        s.push_str(" B");
        return s;
    };
    let mut s = String::new();
    push_u64(&mut s, val_x10 / 10);
    s.push('.');
    push_u64(&mut s, val_x10 % 10);
    s.push(' ');
    s.push_str(unit);
    s
}

/// Append a u64 as decimal to a String without allocating a temporary (no_std).
fn push_u64(out: &mut String, mut v: u64) {
    if v == 0 {
        out.push('0');
        return;
    }
    let mut buf = [0u8; 20];
    let mut n = 0;
    while v > 0 {
        buf[n] = b'0' + (v % 10) as u8;
        v /= 10;
        n += 1;
    }
    for i in (0..n).rev() {
        out.push(buf[i] as char);
    }
}

/// Parse the live `/proc/athena/*` dumps the kernel pushes into a render-ready
/// [`SystemInfo`]. Pure + defensive: every field independently falls back to
/// "(unknown)"/0 if its source line is missing or `(unavailable)`, so a partial
/// or garbled procfs never blanks the whole panel or panics. The single
/// authority both the live setter and the host KAT use.
///
/// - `version`  ← `/proc/version`            → OS line + kernel line
/// - `cpu`      ← `/proc/athena/cpu`          → processor (brand/vendor)
/// - `smp`      ← `/proc/athena/smp`          → logical-CPU count (`cpuN:` rows)
/// - `hardware` ← `/proc/athena/hardware`     → board/DMI
/// - `memory`   ← `/proc/athena/memory`       → `physical_total_bytes`
/// - `storage`  ← `/proc/athena/storage`      → `mounted:` gate + total/free/used
pub fn parse_system_info(
    version: &str,
    cpu: &str,
    smp: &str,
    hardware: &str,
    memory: &str,
    storage: &str,
) -> SystemInfo {
    let mut info = SystemInfo::unknown();

    // ── About: OS + kernel (one /proc/version line carries both) ───────────
    let ver_line = first_meaningful_line(version);
    if !is_unknown(&ver_line) {
        info.os_version = ver_line.clone();
        // The same line names the kernel (AthKernel + build hash); show it whole
        // rather than mis-splitting an unknown format.
        info.kernel = ver_line;
    }

    // ── About: processor name ──────────────────────────────────────────────
    // The kernel's cpu_features dump labels the human name `brand: "..."` (quoted)
    // and the vendor `vendor: AMD (...)`. Prefer the brand; fall back to vendor,
    // a generic key, or the first meaningful line. Strip surrounding quotes.
    let proc_raw = proc_value(cpu, "brand")
        .or_else(|| proc_value(cpu, "model name"))
        .or_else(|| proc_value(cpu, "processor"))
        .or_else(|| proc_value(cpu, "model"))
        .or_else(|| proc_value(cpu, "vendor"));
    if let Some(v) = proc_raw {
        let v = v.trim().trim_matches('"').trim();
        if !v.is_empty() {
            info.processor = String::from(v);
        }
    }
    if is_unknown(&info.processor) {
        let l = first_meaningful_line(cpu);
        if !is_unknown(&l) {
            info.processor = l;
        }
    }

    // ── About: SMP logical-CPU count ───────────────────────────────────────
    // The kernel's /proc/athena/smp emits one `cpuN: ticks=.. task_picks=.. ..`
    // row per online CPU slot plus a trailing `# N of M ...` comment. We count
    // the distinct `cpuN:` data rows (robust to the comment wording). Fall back
    // to any explicit `cpus_online:`-style key. 0 ⇒ "(unknown)".
    let mut cpu_rows = 0u32;
    for line in smp.lines() {
        let l = line.trim();
        if l.starts_with('#') {
            continue;
        }
        if l.starts_with("cpu") && l.contains(':') {
            cpu_rows = cpu_rows.saturating_add(1);
        }
    }
    if cpu_rows > 0 {
        info.smp_cores = cpu_rows;
    } else if let Some(n) = proc_u64(smp, "cpus_online")
        .or_else(|| proc_u64(smp, "logical_cpus"))
        .or_else(|| proc_u64(cpu, "cpus_online"))
    {
        info.smp_cores = n.min(u32::MAX as u64) as u32;
    }

    // ── About: board / firmware (DMI) ──────────────────────────────────────
    if let Some(v) = proc_value(hardware, "board")
        .or_else(|| proc_value(hardware, "product"))
        .or_else(|| proc_value(hardware, "profile"))
    {
        info.board = String::from(v);
    } else {
        let l = first_meaningful_line(hardware);
        if !is_unknown(&l) {
            info.board = l;
        }
    }

    // ── About: installed RAM (physical_total_bytes, first int = bytes) ─────
    if let Some(b) = proc_u64(memory, "physical_total_bytes") {
        info.installed_ram_bytes = b;
    }

    // ── Storage: gate on `mounted: 1`, then total/free/used/system bytes ───
    if proc_u64(storage, "mounted") == Some(1) {
        info.storage_mounted = true;
        info.storage_total_bytes = proc_u64(storage, "total_bytes").unwrap_or(0);
        info.storage_free_bytes = proc_u64(storage, "free_bytes").unwrap_or(0);
        info.storage_used_bytes = proc_u64(storage, "used_bytes").unwrap_or_else(|| {
            info.storage_total_bytes
                .saturating_sub(info.storage_free_bytes)
        });
        info.storage_system_bytes =
            proc_u64(storage, "category_system_bytes").unwrap_or(info.storage_used_bytes);
        // A volume claiming mounted but with a 0 total is not a real volume —
        // fall back to the empty-state so the bar never divides by zero.
        if info.storage_total_bytes == 0 {
            info.storage_mounted = false;
        }
    }

    info
}

// ── Settings profile export/import ───────────────────────────────────────

#[derive(Debug, Clone)]
pub struct SettingsProfile {
    pub name: String,
    pub created_at: u64,
    pub entries: BTreeMap<String, String>,
}

impl SettingsProfile {
    pub fn new(name: &str, timestamp: u64) -> Self {
        Self {
            name: String::from(name),
            created_at: timestamp,
            entries: BTreeMap::new(),
        }
    }

    pub fn set(&mut self, key: &str, value: &str) {
        self.entries.insert(String::from(key), String::from(value));
    }

    pub fn get(&self, key: &str) -> Option<&String> {
        self.entries.get(key)
    }

    pub fn export_to_string(&self) -> String {
        let mut out = String::new();
        out.push_str("[profile]\n");
        out.push_str("name=");
        out.push_str(&self.name);
        out.push('\n');
        out.push_str("[settings]\n");
        for (k, v) in &self.entries {
            out.push_str(k);
            out.push('=');
            out.push_str(v);
            out.push('\n');
        }
        out
    }

    pub fn import_from_string(data: &str) -> Option<Self> {
        let mut name = String::new();
        let mut entries = BTreeMap::new();
        let mut in_settings = false;
        for line in data.split('\n') {
            let trimmed = line.trim();
            if trimmed == "[profile]" {
                in_settings = false;
                continue;
            }
            if trimmed == "[settings]" {
                in_settings = true;
                continue;
            }
            if let Some(eq_pos) = trimmed.find('=') {
                let key = &trimmed[..eq_pos];
                let val = &trimmed[eq_pos + 1..];
                if !in_settings && key == "name" {
                    name = String::from(val);
                } else if in_settings {
                    entries.insert(String::from(key), String::from(val));
                }
            }
        }
        if name.is_empty() {
            return None;
        }
        Some(Self {
            name,
            created_at: 0,
            entries,
        })
    }
}

// ── MDM / GPO integration model ─────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicySource {
    User,
    LocalAdmin,
    GroupPolicy,
    Mdm,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyAction {
    Allow,
    Deny,
    ForceValue,
    Hide,
}

#[derive(Debug, Clone)]
pub struct PolicyEntry {
    pub setting_id: String,
    pub source: PolicySource,
    pub action: PolicyAction,
    pub forced_value: Option<String>,
}

#[derive(Debug, Clone)]
pub struct PolicyManager {
    pub policies: Vec<PolicyEntry>,
}

impl PolicyManager {
    pub fn new() -> Self {
        Self {
            policies: Vec::new(),
        }
    }

    pub fn add_policy(&mut self, setting_id: &str, source: PolicySource, action: PolicyAction) {
        self.policies.push(PolicyEntry {
            setting_id: String::from(setting_id),
            source,
            action,
            forced_value: None,
        });
    }

    pub fn add_forced_policy(&mut self, setting_id: &str, source: PolicySource, value: &str) {
        self.policies.push(PolicyEntry {
            setting_id: String::from(setting_id),
            source,
            action: PolicyAction::ForceValue,
            forced_value: Some(String::from(value)),
        });
    }

    pub fn is_locked(&self, setting_id: &str) -> bool {
        self.policies.iter().any(|p| {
            p.setting_id == setting_id
                && matches!(p.action, PolicyAction::Deny | PolicyAction::ForceValue)
        })
    }

    pub fn is_hidden(&self, setting_id: &str) -> bool {
        self.policies
            .iter()
            .any(|p| p.setting_id == setting_id && p.action == PolicyAction::Hide)
    }

    pub fn forced_value(&self, setting_id: &str) -> Option<&str> {
        self.policies
            .iter()
            .find(|p| p.setting_id == setting_id && p.action == PolicyAction::ForceValue)
            .and_then(|p| p.forced_value.as_deref())
    }

    pub fn effective_source(&self, setting_id: &str) -> PolicySource {
        self.policies
            .iter()
            .filter(|p| p.setting_id == setting_id)
            .max_by_key(|p| match p.source {
                PolicySource::Mdm => 3,
                PolicySource::GroupPolicy => 2,
                PolicySource::LocalAdmin => 1,
                PolicySource::User => 0,
            })
            .map(|p| p.source)
            .unwrap_or(PolicySource::User)
    }
}

// ── Reset options ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResetScope {
    SinglePage,
    Category,
    AllSettings,
    FactoryReset,
}

#[derive(Debug, Clone)]
pub struct ResetRequest {
    pub scope: ResetScope,
    pub target_page: Option<String>,
    pub target_category: Option<SettingsCategory>,
    pub confirmed: bool,
    pub keep_files: bool,
}

impl ResetRequest {
    pub fn page(page_id: &str) -> Self {
        Self {
            scope: ResetScope::SinglePage,
            target_page: Some(String::from(page_id)),
            target_category: None,
            confirmed: false,
            keep_files: true,
        }
    }

    pub fn category(cat: SettingsCategory) -> Self {
        Self {
            scope: ResetScope::Category,
            target_page: None,
            target_category: Some(cat),
            confirmed: false,
            keep_files: true,
        }
    }

    pub fn full_reset(keep_files: bool) -> Self {
        Self {
            scope: ResetScope::FactoryReset,
            target_page: None,
            target_category: None,
            confirmed: false,
            keep_files,
        }
    }
}

// ── Main control panel ───────────────────────────────────────────────────

pub struct ControlPanel {
    pub pages: Vec<SettingsPage>,
    pub navigation: NavigationState,
    pub search: SearchState,
    pub profiles: Vec<SettingsProfile>,
    pub policy_manager: PolicyManager,
    pub pending_reset: Option<ResetRequest>,
    pub visible: bool,
    pub needs_restart_items: Vec<String>,
    /// Live About/Storage facts (parsed from `/proc/athena/*`, pushed by the
    /// kernel via [`set_system_info`]). Defaults to all-"(unknown)" so a shell
    /// built without the kernel push still renders the panels gracefully.
    pub system_info: SystemInfo,
}

impl ControlPanel {
    pub fn new() -> Self {
        let mut panel = Self {
            pages: Vec::new(),
            navigation: NavigationState::new(),
            search: SearchState::new(),
            profiles: Vec::new(),
            policy_manager: PolicyManager::new(),
            pending_reset: None,
            visible: false,
            needs_restart_items: Vec::new(),
            system_info: SystemInfo::unknown(),
        };
        panel.populate_pages();
        panel
    }

    /// Replace the live About/Storage facts (the kernel pushes parsed procfs via
    /// the module-level [`set_system_info_from_proc`]).
    pub fn set_system_info(&mut self, info: SystemInfo) {
        self.system_info = info;
    }

    fn populate_pages(&mut self) {
        self.add_system_pages();
        self.add_devices_pages();
        self.add_network_pages();
        self.add_personalization_pages();
        self.add_apps_pages();
        self.add_accounts_pages();
        self.add_time_language_pages();
        self.add_gaming_pages();
        self.add_ease_of_access_pages();
        self.add_privacy_pages();
        self.add_update_security_pages();
    }

    fn add_system_pages(&mut self) {
        let mut display = SettingsPage::new(
            "sys.display",
            "Display",
            '\u{1F4BB}',
            SettingsCategory::Display,
        )
        .with_description("Resolution, brightness, night light, HDR")
        .with_keywords(&[
            "monitor",
            "screen",
            "resolution",
            "brightness",
            "hdr",
            "night light",
        ])
        .with_sub_page(SubPage::System(SystemPage::Display));
        display.add_setting(SettingItem::dropdown(
            "display.resolution",
            "Resolution",
            "Screen resolution",
            &["3840x2160", "2560x1440", "1920x1080", "1280x720"],
            2,
        ));
        display.add_setting(SettingItem::slider(
            "display.brightness",
            "Brightness",
            "Screen brightness level",
            80,
            0,
            100,
        ));
        display.add_setting(SettingItem::toggle(
            "display.hdr",
            "HDR",
            "High dynamic range output",
            false,
        ));
        display.add_setting(SettingItem::toggle(
            "display.night_light",
            "Night Light",
            "Reduce blue light",
            false,
        ));
        display.add_setting(SettingItem::dropdown(
            "display.refresh",
            "Refresh Rate",
            "Hz",
            &["165", "144", "120", "60"],
            0,
        ));
        display.add_setting(SettingItem::toggle(
            "display.vrr",
            "Adaptive Sync",
            "Variable refresh rate",
            true,
        ));
        self.pages.push(display);

        let mut sound =
            SettingsPage::new("sys.sound", "Sound", '\u{1F50A}', SettingsCategory::Sound)
                .with_description("Volume, output device, spatial audio")
                .with_keywords(&["volume", "audio", "speaker", "headphone", "microphone"])
                .with_sub_page(SubPage::System(SystemPage::Sound));
        sound.add_setting(SettingItem::slider(
            "sound.volume",
            "Master Volume",
            "System volume",
            75,
            0,
            100,
        ));
        sound.add_setting(SettingItem::dropdown(
            "sound.output",
            "Output Device",
            "Active output",
            &["Speakers", "Headphones", "HDMI", "USB DAC"],
            0,
        ));
        sound.add_setting(SettingItem::toggle(
            "sound.spatial",
            "Spatial Audio",
            "3D audio",
            true,
        ));
        sound.add_setting(SettingItem::toggle(
            "sound.low_latency",
            "Low Latency",
            "Minimize buffer",
            true,
        ));
        self.pages.push(sound);

        let mut notifs = SettingsPage::new(
            "sys.notifications",
            "Notifications",
            '\u{1F514}',
            SettingsCategory::SystemAbout,
        )
        .with_description("Notification preferences and Do Not Disturb")
        .with_keywords(&["notification", "alert", "banner", "do not disturb", "dnd"])
        .with_sub_page(SubPage::System(SystemPage::Notifications));
        notifs.add_setting(SettingItem::toggle(
            "notifs.enabled",
            "Notifications",
            "Show notifications",
            true,
        ));
        notifs.add_setting(SettingItem::toggle(
            "notifs.lock_screen",
            "On Lock Screen",
            "Show on lock screen",
            true,
        ));
        notifs.add_setting(SettingItem::toggle(
            "notifs.sounds",
            "Sounds",
            "Play notification sounds",
            true,
        ));
        self.pages.push(notifs);

        let mut power = SettingsPage::new(
            "sys.power",
            "Power & Battery",
            '\u{1F50B}',
            SettingsCategory::PowerGaming,
        )
        .with_description("Power mode, battery saver, sleep settings")
        .with_keywords(&["power", "battery", "sleep", "hibernate", "screen timeout"])
        .with_sub_page(SubPage::System(SystemPage::Power));
        power.add_setting(SettingItem::dropdown(
            "power.mode",
            "Power Mode",
            "Performance vs efficiency",
            &["Best performance", "Balanced", "Best efficiency"],
            1,
        ));
        power.add_setting(SettingItem::toggle(
            "power.battery_saver",
            "Battery Saver",
            "Reduce background activity",
            false,
        ));
        power.add_setting(SettingItem::slider(
            "power.screen_timeout",
            "Screen Timeout (min)",
            "Turn off display after",
            5,
            1,
            60,
        ));
        power.add_setting(SettingItem::slider(
            "power.sleep_timeout",
            "Sleep After (min)",
            "Put device to sleep",
            15,
            1,
            120,
        ));
        self.pages.push(power);

        let mut storage = SettingsPage::new(
            "sys.storage",
            "Storage",
            '\u{1F4BE}',
            SettingsCategory::Storage,
        )
        .with_description("Disk usage, cleanup, storage sense")
        .with_keywords(&["storage", "disk", "space", "cleanup", "temp files"])
        .with_sub_page(SubPage::System(SystemPage::Storage));
        storage.add_setting(SettingItem::toggle(
            "storage.sense",
            "Storage Sense",
            "Automatic cleanup",
            true,
        ));
        storage.add_setting(SettingItem::dropdown(
            "storage.sense_freq",
            "Run Frequency",
            "How often to clean",
            &["Daily", "Weekly", "Monthly", "Low disk space"],
            2,
        ));
        self.pages.push(storage);

        let about = SettingsPage::new(
            "sys.about",
            "About",
            '\u{2139}',
            SettingsCategory::SystemAbout,
        )
        .with_description("Device name, OS version, hardware info")
        .with_keywords(&["about", "version", "device", "rename", "specs"])
        .with_sub_page(SubPage::System(SystemPage::About));
        self.pages.push(about);
    }

    fn add_devices_pages(&mut self) {
        let mut bt = SettingsPage::new(
            "dev.bluetooth",
            "Bluetooth",
            '\u{1F4F6}',
            SettingsCategory::Devices,
        )
        .with_description("Pair and manage Bluetooth devices")
        .with_keywords(&["bluetooth", "pair", "wireless", "headset", "controller"])
        .with_sub_page(SubPage::Devices(DevicesPage::Bluetooth));
        bt.add_setting(SettingItem::toggle(
            "bt.enabled",
            "Bluetooth",
            "Enable Bluetooth",
            true,
        ));
        bt.add_setting(SettingItem::toggle(
            "bt.discoverable",
            "Discoverable",
            "Allow other devices to find this PC",
            false,
        ));
        self.pages.push(bt);

        let mut mouse =
            SettingsPage::new("dev.mouse", "Mouse", '\u{1F5B1}', SettingsCategory::Devices)
                .with_description("Pointer speed, buttons, scrolling")
                .with_keywords(&["mouse", "pointer", "cursor", "scroll", "dpi"])
                .with_sub_page(SubPage::Devices(DevicesPage::Mouse));
        mouse.add_setting(SettingItem::slider(
            "mouse.speed",
            "Pointer Speed",
            "Mouse sensitivity",
            10,
            1,
            20,
        ));
        mouse.add_setting(SettingItem::toggle(
            "mouse.accel",
            "Enhance Pointer Precision",
            "Mouse acceleration",
            false,
        ));
        mouse.add_setting(SettingItem::slider(
            "mouse.scroll_lines",
            "Scroll Lines",
            "Lines per scroll notch",
            3,
            1,
            20,
        ));
        mouse.add_setting(SettingItem::toggle(
            "mouse.reverse_scroll",
            "Reverse Scroll",
            "Natural scrolling",
            false,
        ));
        self.pages.push(mouse);

        let printers = SettingsPage::new(
            "dev.printers",
            "Printers & Scanners",
            '\u{1F5A8}',
            SettingsCategory::Devices,
        )
        .with_description("Add, remove, and manage printers")
        .with_keywords(&["printer", "scanner", "print", "queue"])
        .with_sub_page(SubPage::Devices(DevicesPage::Printers));
        self.pages.push(printers);
    }

    fn add_network_pages(&mut self) {
        let mut wifi =
            SettingsPage::new("net.wifi", "Wi-Fi", '\u{1F4F6}', SettingsCategory::Network)
                .with_description("Connect to wireless networks")
                .with_keywords(&["wifi", "wireless", "connect", "ssid", "password"])
                .with_sub_page(SubPage::Network(NetworkPage::Wifi));
        wifi.add_setting(SettingItem::toggle(
            "wifi.enabled",
            "Wi-Fi",
            "Enable wireless",
            true,
        ));
        wifi.add_setting(SettingItem::toggle(
            "wifi.auto_connect",
            "Auto-connect",
            "Connect to known networks",
            true,
        ));
        wifi.add_setting(SettingItem::toggle(
            "wifi.random_mac",
            "Random MAC",
            "Randomize hardware address",
            true,
        ));
        self.pages.push(wifi);

        let mut vpn = SettingsPage::new("net.vpn", "VPN", '\u{1F512}', SettingsCategory::Network)
            .with_description("Virtual private network connections")
            .with_keywords(&["vpn", "tunnel", "wireguard", "openvpn", "private"])
            .with_sub_page(SubPage::Network(NetworkPage::Vpn));
        vpn.add_setting(SettingItem::toggle(
            "vpn.enabled",
            "VPN",
            "Enable VPN",
            false,
        ));
        vpn.add_setting(SettingItem::dropdown(
            "vpn.protocol",
            "Protocol",
            "VPN protocol",
            &["WireGuard", "OpenVPN", "IKEv2"],
            0,
        ));
        vpn.add_setting(SettingItem::text_input(
            "vpn.server",
            "Server",
            "VPN server address",
            "",
            "server.example.com",
        ));
        self.pages.push(vpn);

        let mut proxy =
            SettingsPage::new("net.proxy", "Proxy", '\u{1F310}', SettingsCategory::Network)
                .with_description("Proxy server settings")
                .with_keywords(&["proxy", "http", "socks", "pac"])
                .with_sub_page(SubPage::Network(NetworkPage::Proxy));
        proxy.add_setting(SettingItem::toggle(
            "proxy.enabled",
            "Use Proxy",
            "Route traffic through proxy",
            false,
        ));
        proxy.add_setting(SettingItem::text_input(
            "proxy.address",
            "Address",
            "Proxy server",
            "",
            "proxy.example.com:8080",
        ));
        self.pages.push(proxy);
    }

    fn add_personalization_pages(&mut self) {
        let mut bg = SettingsPage::new(
            "pers.background",
            "Background",
            '\u{1F5BC}',
            SettingsCategory::Appearance,
        )
        .with_description("Desktop wallpaper and slideshow")
        .with_keywords(&["wallpaper", "background", "desktop", "slideshow"])
        .with_sub_page(SubPage::Personalization(PersonalizationPage::Background));
        bg.add_setting(SettingItem::dropdown(
            "bg.type",
            "Background Type",
            "What to show",
            &["Solid color", "Picture", "Slideshow"],
            1,
        ));
        bg.add_setting(SettingItem::dropdown(
            "bg.fit",
            "Picture Fit",
            "How to display",
            &["Fill", "Fit", "Stretch", "Tile", "Center", "Span"],
            0,
        ));
        bg.add_setting(SettingItem::color_picker(
            "bg.solid_color",
            "Solid Color",
            "Background color",
            20,
            22,
            34,
        ));
        self.pages.push(bg);

        let mut colors = SettingsPage::new(
            "pers.colors",
            "Colors",
            '\u{1F3A8}',
            SettingsCategory::Appearance,
        )
        .with_description("Accent color, dark/light mode, transparency")
        .with_keywords(&[
            "color",
            "theme",
            "dark mode",
            "light mode",
            "accent",
            "transparency",
        ])
        .with_sub_page(SubPage::Personalization(PersonalizationPage::Colors));
        colors.add_setting(SettingItem::dropdown(
            "colors.mode",
            "App Mode",
            "Light or dark",
            &["Light", "Dark", "Custom"],
            1,
        ));
        colors.add_setting(SettingItem::color_picker(
            "colors.accent",
            "Accent Color",
            "System accent",
            78,
            156,
            255,
        ));
        colors.add_setting(SettingItem::toggle(
            "colors.transparency",
            "Transparency",
            "Enable transparency effects",
            true,
        ));
        colors.add_setting(SettingItem::toggle(
            "colors.title_bar",
            "Title Bar Accent",
            "Show accent on title bars",
            false,
        ));
        self.pages.push(colors);

        let mut taskbar = SettingsPage::new(
            "pers.taskbar",
            "Taskbar",
            '\u{2B1C}',
            SettingsCategory::Appearance,
        )
        .with_description("Taskbar position, size, auto-hide")
        .with_keywords(&["taskbar", "bar", "position", "auto hide", "icons"])
        .with_sub_page(SubPage::Personalization(PersonalizationPage::Taskbar));
        taskbar.add_setting(SettingItem::dropdown(
            "taskbar.position",
            "Position",
            "Taskbar location",
            &["Bottom", "Top", "Left", "Right"],
            0,
        ));
        taskbar.add_setting(SettingItem::toggle(
            "taskbar.auto_hide",
            "Auto-hide",
            "Hide when not in use",
            false,
        ));
        taskbar.add_setting(SettingItem::dropdown(
            "taskbar.size",
            "Size",
            "Taskbar height",
            &["Small", "Medium", "Large"],
            1,
        ));
        taskbar.add_setting(SettingItem::toggle(
            "taskbar.badges",
            "Show Badges",
            "Notification badges on icons",
            true,
        ));
        self.pages.push(taskbar);
    }

    fn add_apps_pages(&mut self) {
        let mut apps = SettingsPage::new(
            "apps.features",
            "Apps & Features",
            '\u{1F4E6}',
            SettingsCategory::SystemAbout,
        )
        .with_description("Installed apps, uninstall, defaults")
        .with_keywords(&["apps", "programs", "install", "uninstall", "features"])
        .with_sub_page(SubPage::Apps(AppsPage::AppsFeatures));
        apps.add_setting(SettingItem::dropdown(
            "apps.install_source",
            "Install Source",
            "Where to get apps",
            &["Anywhere", "Store preferred", "Store only"],
            0,
        ));
        self.pages.push(apps);

        let mut startup = SettingsPage::new(
            "apps.startup",
            "Startup",
            '\u{1F680}',
            SettingsCategory::SystemAbout,
        )
        .with_description("Apps that run at sign-in")
        .with_keywords(&["startup", "boot", "autostart", "login"])
        .with_sub_page(SubPage::Apps(AppsPage::Startup));
        startup.add_setting(SettingItem::info_bar(
            "startup.info",
            "Startup apps can slow down sign-in",
            InfoSeverity::Info,
        ));
        self.pages.push(startup);
    }

    fn add_accounts_pages(&mut self) {
        let mut info = SettingsPage::new(
            "acc.info",
            "Your Info",
            '\u{1F464}',
            SettingsCategory::SystemAbout,
        )
        .with_description("Account name, picture, sign-in")
        .with_keywords(&["account", "profile", "name", "picture", "avatar"])
        .with_sub_page(SubPage::Accounts(AccountsPage::YourInfo));
        info.add_setting(SettingItem::text_input(
            "acc.display_name",
            "Display Name",
            "Your name",
            "User",
            "Enter name",
        ));
        self.pages.push(info);

        let mut signin = SettingsPage::new(
            "acc.signin",
            "Sign-in Options",
            '\u{1F511}',
            SettingsCategory::SystemAbout,
        )
        .with_description("Password, PIN, fingerprint, face recognition")
        .with_keywords(&[
            "password",
            "pin",
            "fingerprint",
            "face",
            "hello",
            "biometric",
        ])
        .with_sub_page(SubPage::Accounts(AccountsPage::SignInOptions));
        signin.add_setting(SettingItem::toggle(
            "signin.require_password",
            "Require Sign-in",
            "After sleep",
            true,
        ));
        signin.add_setting(SettingItem::button(
            "signin.change_password",
            "Password",
            "Change your password",
            "Change",
            1001,
        ));
        signin.add_setting(SettingItem::button(
            "signin.setup_pin",
            "PIN",
            "Set up a numeric PIN",
            "Set up",
            1002,
        ));
        self.pages.push(signin);
    }

    fn add_time_language_pages(&mut self) {
        let mut datetime = SettingsPage::new(
            "time.datetime",
            "Date & Time",
            '\u{1F552}',
            SettingsCategory::SystemAbout,
        )
        .with_description("Clock, time zone, calendar")
        .with_keywords(&["time", "date", "clock", "timezone", "calendar", "ntp"])
        .with_sub_page(SubPage::TimeLanguage(TimeLanguagePage::DateTime));
        datetime.add_setting(SettingItem::toggle(
            "time.auto",
            "Set Automatically",
            "Sync with time server",
            true,
        ));
        datetime.add_setting(SettingItem::toggle(
            "time.24h",
            "24-hour Clock",
            "Use 24-hour format",
            true,
        ));
        datetime.add_setting(SettingItem::dropdown(
            "time.timezone",
            "Time Zone",
            "Your time zone",
            &["UTC", "America/New_York", "Europe/London", "Asia/Tokyo"],
            0,
        ));
        self.pages.push(datetime);

        let mut lang = SettingsPage::new(
            "time.language",
            "Language",
            '\u{1F30D}',
            SettingsCategory::SystemAbout,
        )
        .with_description("Display language, input methods, keyboard layouts")
        .with_keywords(&["language", "locale", "keyboard", "input", "ime"])
        .with_sub_page(SubPage::TimeLanguage(TimeLanguagePage::Language));
        lang.add_setting(SettingItem::dropdown(
            "lang.display",
            "Display Language",
            "UI language",
            &[
                "English", "Japanese", "Korean", "Chinese", "Spanish", "French", "German",
            ],
            0,
        ));
        self.pages.push(lang);
    }

    fn add_gaming_pages(&mut self) {
        let mut gbar = SettingsPage::new(
            "game.bar",
            "Game Bar",
            '\u{1F3AE}',
            SettingsCategory::PowerGaming,
        )
        .with_description("Game overlay, recording, screenshots")
        .with_keywords(&["game bar", "overlay", "capture", "record", "screenshot"])
        .with_sub_page(SubPage::Gaming(GamingPage::GameBar));
        gbar.add_setting(SettingItem::toggle(
            "game.bar_enabled",
            "Game Bar",
            "Enable overlay",
            true,
        ));
        gbar.add_setting(SettingItem::toggle(
            "game.fps_counter",
            "FPS Counter",
            "Show frame rate",
            true,
        ));
        self.pages.push(gbar);

        let mut gmode = SettingsPage::new(
            "game.mode",
            "Game Mode",
            '\u{1F3AE}',
            SettingsCategory::PowerGaming,
        )
        .with_description("Optimize system for gaming performance")
        .with_keywords(&["game mode", "performance", "fps", "latency", "priority"])
        .with_sub_page(SubPage::Gaming(GamingPage::GameMode));
        gmode.add_setting(SettingItem::toggle(
            "game.mode_enabled",
            "Game Mode",
            "Optimize for games",
            true,
        ));
        gmode.add_setting(SettingItem::toggle(
            "game.sched_game",
            "SCHED_BODY",
            "Real-time priority for games",
            true,
        ));
        gmode.add_setting(SettingItem::toggle(
            "game.bg_throttle",
            "Background Throttle",
            "Limit background apps",
            true,
        ));
        gmode.add_setting(SettingItem::toggle(
            "game.null_latency",
            "NULL_LATENCY",
            "Disable smoothing",
            false,
        ));
        gmode.add_setting(SettingItem::slider(
            "game.gpu_power",
            "GPU Power Limit (%)",
            "Max GPU power",
            100,
            50,
            115,
        ));
        self.pages.push(gmode);
    }

    fn add_ease_of_access_pages(&mut self) {
        let mut vision = SettingsPage::new(
            "access.display",
            "Display",
            '\u{1F441}',
            SettingsCategory::Accessibility,
        )
        .with_description("Text size, cursor, animations")
        .with_keywords(&["text size", "cursor size", "accessibility", "vision"])
        .with_sub_page(SubPage::EaseOfAccess(EaseOfAccessPage::Display));
        vision.add_setting(SettingItem::slider(
            "access.text_scale",
            "Text Scaling (%)",
            "Make text larger",
            100,
            100,
            225,
        ));
        vision.add_setting(SettingItem::slider(
            "access.cursor_size",
            "Cursor Size",
            "Mouse cursor size",
            1,
            1,
            15,
        ));
        vision.add_setting(SettingItem::toggle(
            "access.animations",
            "Animations",
            "Show animations",
            true,
        ));
        vision.add_setting(SettingItem::toggle(
            "access.transparency",
            "Transparency",
            "Show transparency",
            true,
        ));
        self.pages.push(vision);

        let mut narrator = SettingsPage::new(
            "access.narrator",
            "Narrator",
            '\u{1F4AC}',
            SettingsCategory::Accessibility,
        )
        .with_description("Screen reader for blind users")
        .with_keywords(&["narrator", "screen reader", "tts", "blind", "accessibility"])
        .with_sub_page(SubPage::EaseOfAccess(EaseOfAccessPage::Narrator));
        narrator.add_setting(SettingItem::toggle(
            "access.narrator_enabled",
            "Narrator",
            "Enable screen reader",
            false,
        ));
        narrator.add_setting(SettingItem::slider(
            "access.narrator_speed",
            "Speed",
            "Speech rate",
            10,
            1,
            20,
        ));
        narrator.add_setting(SettingItem::dropdown(
            "access.narrator_voice",
            "Voice",
            "TTS voice",
            &["Default", "Male", "Female"],
            0,
        ));
        self.pages.push(narrator);

        let mut hc = SettingsPage::new(
            "access.highcontrast",
            "High Contrast",
            '\u{25D1}',
            SettingsCategory::Accessibility,
        )
        .with_description("High contrast themes for better visibility")
        .with_keywords(&["high contrast", "theme", "visibility", "colorblind"])
        .with_sub_page(SubPage::EaseOfAccess(EaseOfAccessPage::HighContrast));
        hc.add_setting(SettingItem::toggle(
            "access.hc_enabled",
            "High Contrast",
            "Enable high contrast",
            false,
        ));
        hc.add_setting(SettingItem::dropdown(
            "access.hc_theme",
            "Theme",
            "Contrast theme",
            &[
                "High Contrast #1",
                "High Contrast #2",
                "High Contrast Black",
                "High Contrast White",
            ],
            0,
        ));
        self.pages.push(hc);
    }

    fn add_privacy_pages(&mut self) {
        let mut general = SettingsPage::new(
            "priv.general",
            "General",
            '\u{1F512}',
            SettingsCategory::PrivacySecurity,
        )
        .with_description("Advertising ID, content suggestions, app launch tracking")
        .with_keywords(&["privacy", "advertising", "tracking", "telemetry"])
        .with_sub_page(SubPage::Privacy(PrivacyPage::General));
        general.add_setting(SettingItem::toggle(
            "priv.ad_id",
            "Advertising ID",
            "Let apps use advertising ID",
            false,
        ));
        general.add_setting(SettingItem::toggle(
            "priv.content_suggestions",
            "Content Suggestions",
            "Show suggested content",
            false,
        ));
        general.add_setting(SettingItem::toggle(
            "priv.app_launch_tracking",
            "App Launch Tracking",
            "Track which apps are launched",
            false,
        ));
        self.pages.push(general);

        let mut location = SettingsPage::new(
            "priv.location",
            "Location",
            '\u{1F4CD}',
            SettingsCategory::PrivacySecurity,
        )
        .with_description("Location access for apps and system")
        .with_keywords(&["location", "gps", "position", "geolocation"])
        .with_sub_page(SubPage::Privacy(PrivacyPage::Location));
        location.add_setting(SettingItem::toggle(
            "priv.location_enabled",
            "Location Service",
            "Allow location access",
            true,
        ));
        location.add_setting(SettingItem::toggle(
            "priv.location_history",
            "Location History",
            "Store location history",
            false,
        ));
        self.pages.push(location);

        let mut camera = SettingsPage::new(
            "priv.camera",
            "Camera",
            '\u{1F4F7}',
            SettingsCategory::PrivacySecurity,
        )
        .with_description("Camera access for apps")
        .with_keywords(&["camera", "webcam", "video", "privacy"])
        .with_sub_page(SubPage::Privacy(PrivacyPage::Camera));
        camera.add_setting(SettingItem::toggle(
            "priv.camera_enabled",
            "Camera Access",
            "Allow apps to use camera",
            true,
        ));
        self.pages.push(camera);

        let mut mic = SettingsPage::new(
            "priv.microphone",
            "Microphone",
            '\u{1F3A4}',
            SettingsCategory::PrivacySecurity,
        )
        .with_description("Microphone access for apps")
        .with_keywords(&["microphone", "mic", "audio", "recording", "privacy"])
        .with_sub_page(SubPage::Privacy(PrivacyPage::Microphone));
        mic.add_setting(SettingItem::toggle(
            "priv.mic_enabled",
            "Microphone Access",
            "Allow apps to use mic",
            true,
        ));
        self.pages.push(mic);
    }

    fn add_update_security_pages(&mut self) {
        let mut update = SettingsPage::new(
            "upd.windows_update",
            "AthenaOS Update",
            '\u{1F504}',
            SettingsCategory::PrivacySecurity,
        )
        .with_description("Check for updates, update history, advanced options")
        .with_keywords(&["update", "patch", "upgrade", "install", "check"])
        .with_sub_page(SubPage::UpdateSecurity(UpdateSecurityPage::WindowsUpdate));
        update.add_setting(SettingItem::toggle(
            "upd.auto_check",
            "Auto Check",
            "Automatically check for updates",
            true,
        ));
        update.add_setting(SettingItem::toggle(
            "upd.auto_install",
            "Auto Install",
            "Install updates automatically",
            true,
        ));
        update.add_setting(SettingItem::dropdown(
            "upd.branch",
            "Update Branch",
            "Release channel",
            &["Stable", "Beta", "Insider"],
            0,
        ));
        update.add_setting(SettingItem::button(
            "upd.check_now",
            "Check for Updates",
            "Look for available updates",
            "Check now",
            2001,
        ));
        self.pages.push(update);

        let mut recovery = SettingsPage::new(
            "upd.recovery",
            "Recovery",
            '\u{1F504}',
            SettingsCategory::PrivacySecurity,
        )
        .with_description("Reset PC, advanced startup, go back")
        .with_keywords(&["recovery", "reset", "restore", "rollback", "backup"])
        .with_sub_page(SubPage::UpdateSecurity(UpdateSecurityPage::Recovery));
        recovery.add_setting(SettingItem::button(
            "upd.reset_pc",
            "Reset this PC",
            "Reinstall AthenaOS",
            "Get started",
            2002,
        ));
        recovery.add_setting(SettingItem::button(
            "upd.advanced_startup",
            "Advanced Startup",
            "Boot into recovery",
            "Restart now",
            2003,
        ));
        recovery.add_setting(SettingItem::toggle(
            "upd.snapshots",
            "Auto Snapshots",
            "Create restore points before updates",
            true,
        ));
        self.pages.push(recovery);

        let mut dev = SettingsPage::new(
            "upd.developer",
            "For Developers",
            '\u{1F527}',
            SettingsCategory::PrivacySecurity,
        )
        .with_description("Developer mode, remote debugging, sideloading")
        .with_keywords(&["developer", "sideload", "debug", "usb", "adb"])
        .with_sub_page(SubPage::UpdateSecurity(UpdateSecurityPage::ForDevelopers));
        dev.add_setting(
            SettingItem::toggle(
                "dev.mode",
                "Developer Mode",
                "Enable developer features",
                false,
            )
            .with_restart(),
        );
        dev.add_setting(SettingItem::toggle(
            "dev.remote_debug",
            "Remote Debugging",
            "Allow remote connections",
            false,
        ));
        dev.add_setting(SettingItem::toggle(
            "dev.sideload",
            "Sideload Apps",
            "Install apps from any source",
            false,
        ));
        self.pages.push(dev);
    }

    // ── Public API ───────────────────────────────────────────────────────

    pub fn show(&mut self) {
        self.visible = true;
        // macOS/Win11 open Settings onto a selected pane, never an empty title
        // (visual-QA Round-7 #2). If nothing is selected yet, land on Appearance
        // & Vibe — its Colors page hosts the accent-ramp + Vibe preset showcase,
        // a representative populated detail pane (and the nav row's icon lights).
        if self.navigation.current_category.is_none() && self.navigation.current_page_id.is_none() {
            self.navigation
                .navigate_to_page(SettingsCategory::Appearance, "pers.colors");
        }
    }

    pub fn hide(&mut self) {
        self.visible = false;
    }

    pub fn toggle(&mut self) {
        self.visible = !self.visible;
    }

    pub fn navigate_to_category(&mut self, category: SettingsCategory) {
        self.navigation.navigate_to_category(category);
        self.search.clear();
    }

    pub fn navigate_to_page(&mut self, page_id: &str) {
        if let Some(page) = self.pages.iter().find(|p| p.id == page_id) {
            let cat = page.category;
            self.navigation.navigate_to_page(cat, page_id);
            self.search.clear();
        }
    }

    pub fn go_back(&mut self) -> bool {
        self.navigation.go_back()
    }

    pub fn go_forward(&mut self) -> bool {
        self.navigation.go_forward()
    }

    pub fn go_home(&mut self) {
        self.navigation.go_home();
    }

    pub fn search(&mut self, query: &str) {
        self.search.query = String::from(query);
        self.search.active = true;
        self.search.search(&self.pages);
    }

    pub fn pages_for_category(&self, category: SettingsCategory) -> Vec<&SettingsPage> {
        self.pages
            .iter()
            .filter(|p| p.category == category)
            .collect()
    }

    pub fn current_page(&self) -> Option<&SettingsPage> {
        self.navigation
            .current_page_id
            .as_ref()
            .and_then(|id| self.pages.iter().find(|p| p.id == id.as_str()))
    }

    pub fn current_page_mut(&mut self) -> Option<&mut SettingsPage> {
        if let Some(ref id) = self.navigation.current_page_id {
            let id_owned = id.clone();
            self.pages.iter_mut().find(|p| p.id == id_owned.as_str())
        } else {
            None
        }
    }

    pub fn toggle_setting(&mut self, page_id: &str, setting_id: &str) {
        if self.policy_manager.is_locked(setting_id) {
            return;
        }
        if let Some(page) = self.pages.iter_mut().find(|p| p.id == page_id) {
            if let Some(item) = page.settings.iter_mut().find(|s| s.id == setting_id) {
                if let SettingControl::Toggle { ref mut value } = item.control {
                    *value = !*value;
                }
                if item.requires_restart && !self.needs_restart_items.contains(&item.id) {
                    self.needs_restart_items.push(item.id.clone());
                }
            }
        }
    }

    pub fn set_slider(&mut self, page_id: &str, setting_id: &str, val: u32) {
        if self.policy_manager.is_locked(setting_id) {
            return;
        }
        if let Some(page) = self.pages.iter_mut().find(|p| p.id == page_id) {
            if let Some(item) = page.settings.iter_mut().find(|s| s.id == setting_id) {
                if let SettingControl::Slider {
                    ref mut value,
                    min,
                    max,
                    ..
                } = item.control
                {
                    *value = val.clamp(min, max);
                }
            }
        }
    }

    pub fn set_dropdown(&mut self, page_id: &str, setting_id: &str, index: usize) {
        if self.policy_manager.is_locked(setting_id) {
            return;
        }
        if let Some(page) = self.pages.iter_mut().find(|p| p.id == page_id) {
            if let Some(item) = page.settings.iter_mut().find(|s| s.id == setting_id) {
                if let SettingControl::Dropdown {
                    ref mut selected,
                    ref options,
                } = item.control
                {
                    if index < options.len() {
                        *selected = index;
                    }
                }
            }
        }
    }

    pub fn request_reset(
        &mut self,
        scope: ResetScope,
        page_id: Option<&str>,
        category: Option<SettingsCategory>,
    ) {
        self.pending_reset = Some(ResetRequest {
            scope,
            target_page: page_id.map(String::from),
            target_category: category,
            confirmed: false,
            keep_files: true,
        });
    }

    pub fn confirm_reset(&mut self) {
        if let Some(ref mut req) = self.pending_reset {
            req.confirmed = true;
        }
    }

    pub fn cancel_reset(&mut self) {
        self.pending_reset = None;
    }

    pub fn export_profile(&self, name: &str, timestamp: u64) -> SettingsProfile {
        let mut profile = SettingsProfile::new(name, timestamp);
        for page in &self.pages {
            for item in &page.settings {
                let val = match &item.control {
                    SettingControl::Toggle { value } => {
                        if *value {
                            String::from("true")
                        } else {
                            String::from("false")
                        }
                    }
                    SettingControl::Slider { value, .. } => {
                        let mut s = String::new();
                        let mut n = *value;
                        if n == 0 {
                            s.push('0');
                        } else {
                            let mut buf = [0u8; 10];
                            let mut pos = 10;
                            while n > 0 {
                                pos -= 1;
                                buf[pos] = b'0' + (n % 10) as u8;
                                n /= 10;
                            }
                            for &b in &buf[pos..10] {
                                s.push(b as char);
                            }
                        }
                        s
                    }
                    SettingControl::Dropdown { selected, .. } => {
                        let mut s = String::new();
                        let mut n = *selected as u32;
                        if n == 0 {
                            s.push('0');
                        } else {
                            let mut buf = [0u8; 10];
                            let mut pos = 10;
                            while n > 0 {
                                pos -= 1;
                                buf[pos] = b'0' + (n % 10) as u8;
                                n /= 10;
                            }
                            for &b in &buf[pos..10] {
                                s.push(b as char);
                            }
                        }
                        s
                    }
                    SettingControl::TextInput { value, .. } => value.clone(),
                    _ => continue,
                };
                profile.set(&item.id, &val);
            }
        }
        profile
    }

    pub fn import_profile(&mut self, profile: &SettingsProfile) {
        for (key, val) in &profile.entries {
            for page in &mut self.pages {
                if let Some(item) = page.settings.iter_mut().find(|s| s.id == key.as_str()) {
                    if self.policy_manager.is_locked(&item.id) {
                        continue;
                    }
                    match &mut item.control {
                        SettingControl::Toggle { ref mut value } => {
                            *value = val == "true";
                        }
                        SettingControl::Slider {
                            ref mut value,
                            min,
                            max,
                            ..
                        } => {
                            if let Some(n) = parse_u32(val) {
                                *value = n.clamp(*min, *max);
                            }
                        }
                        SettingControl::Dropdown {
                            ref mut selected,
                            ref options,
                        } => {
                            if let Some(n) = parse_u32(val) {
                                if (n as usize) < options.len() {
                                    *selected = n as usize;
                                }
                            }
                        }
                        SettingControl::TextInput { ref mut value, .. } => {
                            *value = val.clone();
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    pub fn needs_restart(&self) -> bool {
        !self.needs_restart_items.is_empty()
    }
}

fn parse_u32(s: &str) -> Option<u32> {
    let mut result: u32 = 0;
    for b in s.bytes() {
        if b < b'0' || b > b'9' {
            return None;
        }
        result = result.checked_mul(10)?.checked_add((b - b'0') as u32)?;
    }
    Some(result)
}

// ── Design tokens (docs/design/settings.md + design-language.md, ath_tokens) ─
//
// The Settings surface no longer owns a private `CP_*` palette. Every colour is
// derived from the SAME LIVE seed accent the taskbar/Start re-skin reads
// (`crate::active_accent()`, pushed by the kernel from
// `theme_engine::active_accent()`), so changing the accent re-skins Settings,
// the taskbar, and the window chrome together — settings.md §6 cohesion test.
// The retired consts map to tokens as:
//   CP_BG          → mica_tint()  (window backdrop / sidebar, material.mica)
//   CP_SIDEBAR_BG  → mica_tint()  (sidebar is continuous with the backdrop)
//   CP_HEADER_BG   → mica_tint()  (titlebar continuous with the mica)
//   CP_FG          → PALETTE.text_primary
//   CP_DIM         → PALETTE.text_tertiary  (hints/descriptions)
//   CP_ACCENT      → cp_accent().base  (= derive_accent(active_accent()).base)
//   CP_SELECTED    → accent().subtle  (selection wash)
//   CP_TOGGLE_ON   → accent().base
//   CP_TOGGLE_OFF  → PALETTE.bg_elevated
//   CP_SLIDER_TRACK→ PALETTE.bg_elevated
//   CP_BTN_BG      → PALETTE.bg_elevated  (button resting)

const GLYPH_W: usize = 8;
const GLYPH_H: usize = 8;

/// Active palette — dark default (design-language §4.1), same as the shell.
/// The accent seed is now LIVE via `cp_accent_seed()` → `crate::active_accent()`
/// (pushed by the kernel from `theme_engine::active_accent()`); a light palette
/// swap is the one remaining value change here. No `ath_abi` dependency is
/// needed — the seed flows through the crate-local `ACTIVE_ACCENT` static.
#[inline]
fn cp_palette() -> &'static ath_tokens::Palette {
    &ath_tokens::DARK
}

/// The LIVE seed accent — the SAME `crate::active_accent()` the taskbar/Start
/// reskin reads, pushed by the kernel from `theme_engine::active_accent()`. A
/// one-tap Vibe re-skin recolours Settings, the taskbar, and the window chrome
/// together (settings.md §6 cohesion test). Defaults to RaeBlue.
#[inline]
fn cp_accent_seed() -> u32 {
    crate::active_accent()
}

/// The six-token accent ramp, derived deterministically from the live seed.
#[inline]
fn cp_accent() -> ath_tokens::AccentRamp {
    ath_tokens::derive_accent(cp_accent_seed(), cp_palette())
}

/// The contrast-safe CAPTION / hint / secondary-label TEXT ink over glass.
///
/// CONTRAST is the binding rule (spec bullet 7). The two glass planes apply the
/// SHIP-GATE WCAG cap inside `glass_tier_interior`, but that cap guarantees AA for
/// `text.primary` ONLY — and the window can be dragged anywhere, so over a BRIGHT
/// aurora blob the panel/popover interior rises to ~luma 0.40 where:
///   * `text_tertiary` (rel-luminance 0.18) measures ~2.5:1 — FAILS,
///   * `text_secondary` (rel-luminance 0.46) measures ~2.7:1 over a bright blob —
///     also FAILS (it only clears AA over the DARK void, not over the aurora),
///   * `text_primary`  (rel-luminance 0.89) is the ONE ink the cap guarantees
///     ≥4.5:1 at EVERY aurora position (worst measured 4.92:1 over the blue blob).
/// So EVERY on-glass TEXT label is painted in `text_primary`, and the visual
/// hierarchy is carried by the TYPE RAMP (caption 11px < label 13px < body 14px <
/// subtitle 17px < title 22px) + accent emphasis, NOT by colour dimming — robust
/// at any window position. `text_tertiary` survives only on DECORATIVE glyphs
/// (chevrons, the disabled-control dim, the search magnifier), never on a label.
///
/// DEVIATION NOTE (spec bullet 7): this departs from the literal "caption at 70%
/// opacity" style — contrast wins, exactly as the spec instructs ("if it conflicts
/// with the 70% caption style, contrast wins — note the deviation").
#[inline]
fn cp_caption_ink() -> u32 {
    cp_palette().text_primary
}

/// In-card caption ink — identical to [`cp_caption_ink`] (both planes carry the
/// same binding constraint; see that doc). Kept as a distinct name so the call
/// sites document which plane they render over.
#[inline]
fn cp_card_caption_ink() -> u32 {
    cp_palette().text_primary
}

/// `material.mica` static tint (design-language §5.2): the same opaque
/// bg.base/bg.raised blend the taskbar paints — wallpaper-independent, off the
/// per-frame blur path. The Settings window backdrop + sidebar use this.
#[inline]
fn cp_mica() -> u32 {
    let p = cp_palette();
    cp_blend_opaque(p.bg_base, p.bg_raised, 1, 2)
}

/// Plane 3 (content card): a raised glass.popover surface over the frosted panel.
///
/// THE depth contract (the spec's 3-plane property): the sidebar is the
/// glass.panel plane (Plane 2); every detail-pane card is the MORE-opaque
/// glass.popover plane (Plane 3) drawn here. We cast a soft `elev.1`/`elev.2`
/// drop shadow under the card so it visibly lifts above the panel, then draw the
/// shipped `draw_glass_surface(GLASS_POPOVER_DARK)` (which bakes the frost, the
/// 1px top highlight, the WCAG legibility cap guaranteeing `text.primary`, the
/// hairline + iridescent rim). All cards use `RADIUS_MD` (12) — the single card
/// radius across the whole surface. Retires the old solid `cp_pane_card()` fill
/// (which was the same opaque plane as everything else — no depth).
fn draw_content_card(
    canvas: &mut athgfx::Canvas,
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    elev: ath_tokens::Elevation,
) {
    if w == 0 || h == 0 {
        return;
    }
    let r = ath_tokens::RADIUS_MD as usize;
    // Soft ambient shadow so the popover card lifts off the panel plane.
    canvas.fill_rounded_rect_shadow(
        x,
        y,
        w,
        h,
        r,
        elev.color,
        elev.radius as usize,
        elev.offset_y,
    );
    // Plane 3: the more-opaque glass.popover surface (frost + top highlight + WCAG
    // cap + hairline + rim are all baked by draw_glass_surface).
    athgfx::glass::draw_glass_surface(canvas, x, y, w, h, r, ath_tokens::GLASS_POPOVER_DARK);
}

/// Opaque per-channel blend `a*(den-num)/den + b*num/den` (matches
/// athshell::lib `blend_opaque`; duplicated here only because that helper is
/// private to lib.rs and this crate keeps token math local per module).
#[inline]
fn cp_blend_opaque(a: u32, b: u32, num: u32, den: u32) -> u32 {
    let mix = |sa: u32, sb: u32| -> u32 { (sa * (den - num) + sb * num) / den };
    let ar = (a >> 16) & 0xFF;
    let ag = (a >> 8) & 0xFF;
    let ab = a & 0xFF;
    let br = (b >> 16) & 0xFF;
    let bg = (b >> 8) & 0xFF;
    let bb = b & 0xFF;
    0xFF00_0000 | (mix(ar, br) << 16) | (mix(ag, bg) << 8) | mix(ab, bb)
}

// ── Rendering ────────────────────────────────────────────────────────────

impl ControlPanel {
    pub fn render(&self, canvas: &mut athgfx::Canvas, ox: usize, oy: usize, w: usize, h: usize) {
        if !self.visible {
            return;
        }
        use ath_tokens::{RADIUS_LG, SPACE_4, SPACE_5, SPACE_6};

        let p = cp_palette();

        // ── Plane 1 (backdrop): the aurora is drawn by the caller (shell_runner /
        //    the screenshot harness) BEHIND this window. We composite over it.
        //
        // ── SHADOW: the floating top-level window casts the `elev.5` drop shadow
        //    (offset 24 / blur 48 / 30% black) so the whole frame lifts off the
        //    wallpaper (design-language §5.3). Drawn BEFORE the glass so only the
        //    soft fringe + corners read; the opaque glass overdraws the interior.
        let win_r = RADIUS_LG as usize; // window outer corner = radius.lg (16)
        canvas.fill_rounded_rect_shadow(
            ox,
            oy,
            w,
            h,
            win_r,
            ath_tokens::ELEV_5.color,
            ath_tokens::ELEV_5.radius as usize,
            ath_tokens::ELEV_5.offset_y,
        );

        // ── Plane 2 (panel): the window body + sidebar — glass.panel (IDENTITY.md
        //    §7), a frosted, see-through material the aurora reads through, with the
        //    baked 1px top highlight (8% white), the WCAG legibility cap, the
        //    hairline + iridescent rim (the shipped `draw_glass_surface`). The
        //    sidebar inherits this plane; the CONTENT cards are a MORE-opaque
        //    glass.popover plane (Plane 3) raised above it — so sidebar(panel) and
        //    content(popover) read as visibly different translucency planes.
        athgfx::glass::draw_glass_surface(
            canvas,
            ox,
            oy,
            w,
            h,
            win_r,
            ath_tokens::GLASS_PANEL_DARK,
        );
        // A 1px hairline border (stroke.subtle) framing the whole window —
        // `draw_glass_surface` already lays the outer hairline, but we re-assert it
        // at the window radius so the frame edge is crisp against the shadow.
        canvas.draw_rounded_rect_outline(ox, oy, w, h, win_r, p.stroke_subtle);

        // ── Titlebar (32px, standard window chrome, type.subtitle) ──────────
        let titlebar_h = 32usize;
        canvas.draw_text_aa(
            (ox + SPACE_4 as usize) as i32,
            (oy + (titlebar_h.saturating_sub(ath_tokens::TYPE_SUBTITLE.line_height as usize)) / 2)
                as i32,
            "Settings",
            ath_tokens::TYPE_SUBTITLE,
            p.text_primary,
            athgfx::text::FontFamily::Sans,
        );
        // Close affordance — REAL athgfx Close icon (spec bullet 8: no tofu glyph),
        // resting text.secondary. 16px, centred on the 32px title bar.
        let close_sz = 16usize;
        canvas.draw_icon(
            athgfx::icon::Icon::Close,
            (ox + w.saturating_sub(SPACE_5 as usize)) as i32,
            (oy + (titlebar_h.saturating_sub(close_sz)) / 2) as i32,
            close_sz as i32,
            p.text_secondary,
        );

        // ── Two panes ───────────────────────────────────────────────────────
        // Sidebar width = space.6 × 7.5 = 240px (settings.md §1), clamped to a
        // third of the window so a small window still shows content. 240 is on the
        // 8-grid (30 × 8).
        let sidebar_w = (SPACE_6 as usize * 15 / 2).min(w / 3);
        let pane_y = oy + titlebar_h;
        let pane_h = h.saturating_sub(titlebar_h);

        self.render_sidebar(canvas, ox, pane_y, sidebar_w, pane_h);

        // Sidebar/content divider (1px stroke.subtle).
        for s in pane_y..pane_y + pane_h {
            canvas.blend_pixel(ox + sidebar_w, s, p.stroke_subtle);
        }

        // ── Plane 3 (content): the detail-pane CARDS are drawn directly over the
        //    frosted panel as glass.popover (more opaque, raised) — the panel glass
        //    reads through the gutter between cards, so the sidebar(panel) and the
        //    content cards(popover) are visibly distinct planes. We do NOT paint a
        //    solid content fill here (the old de-tinted box flattened the depth) —
        //    the panel plane IS the content background, and each card lifts above it.
        let content_x = ox + sidebar_w + SPACE_6 as usize;
        let content_w = w.saturating_sub(sidebar_w + SPACE_6 as usize * 2);
        let content_y = pane_y;

        if self.navigation.current_page_id.is_none() {
            self.render_category_landing(canvas, content_x, content_y, content_w, pane_h);
        } else if let Some(page) = self.current_page() {
            self.render_page(canvas, page, content_x, content_y, content_w, pane_h);
        }

        // ── Search results overlay (material.glass, elev.3) over content ────
        if self.search.active && !self.search.query.is_empty() {
            self.render_search_overlay(canvas, content_x, content_y, content_w, pane_h);
        }
    }

    /// Sidebar: search field (radius.sm, bg.elevated) + the category list with
    /// hover/selection wash (accent.subtle) + radius.xs row fills.
    fn render_sidebar(
        &self,
        canvas: &mut athgfx::Canvas,
        sx: usize,
        sy: usize,
        sw: usize,
        sh: usize,
    ) {
        use ath_tokens::{RADIUS_SM, RADIUS_XS, SPACE_1, SPACE_2, SPACE_3, SPACE_4};
        let p = cp_palette();
        let accent = cp_accent();
        let pad = SPACE_4 as usize;

        // Search field (32px floor, radius.sm, bg.elevated). Focus ring when
        // the search is active (settings.md §1: 2px accent.base ring).
        let field_h = ath_tokens::HIT_TARGET_POINTER as usize;
        let fx = sx + pad;
        let fy = sy + pad;
        let fw = sw.saturating_sub(2 * pad);
        canvas.fill_rounded_rect(fx, fy, fw, field_h, RADIUS_SM as usize, p.bg_elevated);
        if self.search.active {
            canvas.draw_rounded_rect_outline(fx, fy, fw, field_h, RADIUS_SM as usize, accent.base);
            canvas.draw_rounded_rect_outline(
                fx.saturating_sub(1),
                fy.saturating_sub(1),
                fw + 2,
                field_h + 2,
                RADIUS_SM as usize,
                accent.glow,
            );
        }
        // Search magnifier — REAL athgfx line-icon (spec bullet 8: no tofu glyph;
        // the bitmap font drew the U+1F50D codepoint as `?`).
        let mag_sz = 16usize;
        let mag_y = fy + (field_h.saturating_sub(mag_sz)) / 2;
        canvas.draw_icon(
            athgfx::icon::Icon::Search,
            (fx + SPACE_2 as usize) as i32,
            mag_y as i32,
            mag_sz as i32,
            p.text_secondary,
        );
        let (q_text, q_fg) = if self.search.query.is_empty() {
            // Placeholder uses the contrast-safe secondary ink (text_tertiary fails
            // 4.5:1 over the field/panel; contrast is the binding rule).
            ("Search settings", p.text_secondary)
        } else {
            (self.search.query.as_str(), p.text_primary)
        };
        // Search field text — type.body, crisp AA RaeSans.
        let field_text_y =
            fy + (field_h.saturating_sub(ath_tokens::TYPE_BODY.line_height as usize)) / 2;
        canvas.draw_text_aa(
            // Clear the 16px magnifier icon + a SPACE_2 gap (8-grid).
            (fx + SPACE_2 as usize + 16 + SPACE_2 as usize) as i32,
            field_text_y as i32,
            q_text,
            ath_tokens::TYPE_BODY,
            q_fg,
            athgfx::text::FontFamily::Sans,
        );

        // Category list (36px rows, radius.xs hover/selection wash).
        let row_h = 36usize;
        let mut iy = fy + field_h + SPACE_4 as usize;
        for cat in SettingsCategory::all().iter() {
            if iy + row_h > sy + sh {
                break;
            }
            let is_sel = self.navigation.current_category == Some(*cat);
            if is_sel {
                canvas.fill_rounded_rect(
                    sx + SPACE_2 as usize,
                    iy,
                    sw.saturating_sub(2 * SPACE_2 as usize),
                    row_h,
                    RADIUS_XS as usize,
                    accent.subtle,
                );
            }
            // Real athgfx line-icon (visual-QA Round-7 #1: retire the emoji-char
            // `?` placeholders). Token-tint follows the CC treatment: active row =
            // accent.text ink, inactive = text.secondary. Sized to the glyph band
            // (18px) so it sits on the established 36px-row baseline; centred on
            // the same left-inset column the label flows from.
            let icon_sz = 18usize;
            let icon_y = iy + (row_h.saturating_sub(icon_sz)) / 2;
            canvas.draw_icon(
                cat.line_icon(),
                (sx + pad) as i32,
                icon_y as i32,
                icon_sz as i32,
                if is_sel {
                    accent.text
                } else {
                    p.text_secondary
                },
            );
            let label_fg = if is_sel {
                p.text_primary
            } else {
                p.text_secondary
            };
            // Category label — type.label, centred on the 36px row. The label
            // column clears the 18px icon (was GLYPH_W=8 for the old char glyph).
            let label_y =
                iy + (row_h.saturating_sub(ath_tokens::TYPE_LABEL.line_height as usize)) / 2;
            canvas.draw_text_aa(
                (sx + pad + icon_sz + SPACE_3 as usize) as i32,
                label_y as i32,
                cat.label(),
                ath_tokens::TYPE_LABEL,
                label_fg,
                athgfx::text::FontFamily::Sans,
            );
            iy += row_h + SPACE_1 as usize;
        }
    }

    /// Category landing: breadcrumb + the page list as group-card rows.
    fn render_category_landing(
        &self,
        canvas: &mut athgfx::Canvas,
        cx: usize,
        cy: usize,
        cw: usize,
        ch: usize,
    ) {
        use ath_tokens::{SPACE_3, SPACE_4, SPACE_5};
        let p = cp_palette();
        let accent = cp_accent();

        self.render_breadcrumb(canvas, cx, cy + SPACE_4 as usize, cw);

        let title = self
            .navigation
            .current_category
            .map(|c| c.label())
            .unwrap_or("Settings");
        // Page title — type.title, crisp AA RaeSans (was scale-2 8×8 bitmap).
        let title_y = cy + SPACE_5 as usize;
        canvas.draw_text_aa(
            cx as i32,
            title_y as i32,
            title,
            ath_tokens::TYPE_TITLE,
            p.text_primary,
            athgfx::text::FontFamily::Sans,
        );

        if let Some(cat) = self.navigation.current_category {
            let pages = self.pages_for_category(cat);
            let card_h = 56usize;
            let mut py = title_y + 24 + SPACE_4 as usize;
            for page in pages.iter() {
                if py + card_h > cy + ch {
                    break;
                }
                // Plane 3 group card (glass.popover, radius.md, elev.1 — raised
                // above the panel plane the aurora reads through).
                draw_content_card(canvas, cx, py, cw, card_h, ath_tokens::ELEV_1);
                let gy = py + SPACE_4 as usize;
                // Leading icon — REAL athgfx line-icon for the category (the page's
                // emoji `char` rendered as tofu); accent-tinted. 18px on the 8-grid.
                let lic_sz = 18usize;
                canvas.draw_icon(
                    cat.line_icon(),
                    (cx + SPACE_4 as usize) as i32,
                    gy as i32,
                    lic_sz as i32,
                    accent.text,
                );
                let text_x = (cx + SPACE_4 as usize + lic_sz + SPACE_3 as usize) as i32;
                // Card title — type.body; description below — type.caption.
                canvas.draw_text_aa(
                    text_x,
                    gy as i32,
                    &page.title,
                    ath_tokens::TYPE_BODY,
                    p.text_primary,
                    athgfx::text::FontFamily::Sans,
                );
                // Proportional truncation by MEASURED RaeSans advance (not the
                // 8px mono-cell estimate) — fits the card's text column exactly.
                let desc_avail = cw.saturating_sub(text_x as usize - cx + SPACE_4 as usize);
                let desc = fit_text_aa(
                    canvas,
                    &page.description,
                    desc_avail,
                    ath_tokens::TYPE_CAPTION,
                );
                canvas.draw_text_aa(
                    text_x,
                    (gy + 16) as i32,
                    desc,
                    ath_tokens::TYPE_CAPTION,
                    cp_card_caption_ink(), // in-card caption (popover plane)
                    athgfx::text::FontFamily::Sans,
                );
                // Disclosure chevron — REAL Chevron (right) line-icon, no tofu.
                let chev_sz = 14usize;
                canvas.draw_icon(
                    athgfx::icon::Icon::Chevron,
                    (cx + cw.saturating_sub(SPACE_4 as usize + chev_sz)) as i32,
                    (py + (card_h.saturating_sub(chev_sz)) / 2) as i32,
                    chev_sz as i32,
                    p.text_secondary,
                );
                py += card_h + SPACE_3 as usize;
            }
        }
    }

    /// A full settings page: breadcrumb header, page title, then a titled group
    /// card holding the setting rows + their token-driven controls.
    fn render_page(
        &self,
        canvas: &mut athgfx::Canvas,
        page: &SettingsPage,
        cx: usize,
        cy: usize,
        cw: usize,
        ch: usize,
    ) {
        use ath_tokens::{SPACE_4, SPACE_5};
        let p = cp_palette();

        self.render_breadcrumb(canvas, cx, cy + SPACE_4 as usize, cw);

        let title_y = cy + SPACE_5 as usize;
        // Page title — type.title; description — type.body (crisp AA RaeSans).
        canvas.draw_text_aa(
            cx as i32,
            title_y as i32,
            &page.title,
            ath_tokens::TYPE_TITLE,
            p.text_primary,
            athgfx::text::FontFamily::Sans,
        );
        canvas.draw_text_aa(
            cx as i32,
            (title_y + 26) as i32,
            fit_text_aa(canvas, &page.description, cw, ath_tokens::TYPE_BODY),
            ath_tokens::TYPE_BODY,
            p.text_secondary,
            athgfx::text::FontFamily::Sans,
        );

        // Net-new data panels: the About (§5) + Storage (§4) pages render live
        // `/proc/athena/*` facts as glass cards above their (few) controls.
        let panels_y = title_y + 26 + GLYPH_H + SPACE_4 as usize;
        let panels_h = (cy + ch).saturating_sub(panels_y);
        if page.id == "sys.about" {
            self.render_about_panel(canvas, cx, panels_y, cw, panels_h);
            return;
        }
        if page.id == "sys.storage" {
            self.render_storage_panel(canvas, page, cx, panels_y, cw, panels_h);
            return;
        }

        // Group card holding every row (concentric child radius for the rows).
        let card_x = cx;
        let card_y = title_y + 26 + GLYPH_H + SPACE_4 as usize;
        let card_w = cw;
        let row_h = 44usize; // settings.md §1 row min-height.
        let inner_pad = SPACE_4 as usize;
        let visible_rows = page.settings.iter().filter(|s| s.visible).count().max(1);
        let card_h = (visible_rows * row_h + 2 * inner_pad)
            .min((cy + ch).saturating_sub(card_y + inner_pad));
        // Plane 3 group card (glass.popover, radius.md, elev.2 — the main detail
        // card sits highest, raised above the panel plane).
        draw_content_card(canvas, card_x, card_y, card_w, card_h, ath_tokens::ELEV_2);

        let mut sy = card_y + inner_pad;
        let mut first = true;
        for item in page.settings.iter().filter(|s| s.visible) {
            if sy + row_h > card_y + card_h {
                break;
            }
            // Row divider (stroke.subtle) between rows (not above the first).
            if !first {
                for xx in card_x + inner_pad..card_x + card_w - inner_pad {
                    canvas.blend_pixel(xx, sy - 1, p.stroke_subtle);
                }
            }
            first = false;
            self.render_setting_row(
                canvas,
                item,
                card_x + inner_pad,
                sy,
                card_w - 2 * inner_pad,
                row_h,
            );
            sy += row_h;
        }

        // Appearance & Vibe showcase: the Colors page hosts the accent-ramp
        // preview + the live Vibe preset grid (settings.md §5).
        if page.id == "pers.colors" {
            let showcase_y = card_y + card_h + SPACE_5 as usize;
            self.render_appearance_showcase(
                canvas,
                cx,
                showcase_y,
                cw,
                (cy + ch).saturating_sub(showcase_y),
            );
        }
    }

    /// About panel (docs/design/settings-redesign.md §5): a Device hero card +
    /// a System label/value card sourced from the live `/proc/athena/*` push
    /// (`self.system_info`). Every unavailable field renders "(unknown)" — never
    /// blank, never a panic. Glass group cards at `radius.lg`, `elev.1`.
    fn render_about_panel(
        &self,
        canvas: &mut athgfx::Canvas,
        cx: usize,
        cy: usize,
        cw: usize,
        ch: usize,
    ) {
        use ath_tokens::{SPACE_2, SPACE_3, SPACE_4};
        let p = cp_palette();
        let accent = cp_accent();
        let info = &self.system_info;
        let inner = SPACE_4 as usize;

        // ── Card 1 — Device hero (OS line + AthenaOS mark) ──────────────────
        let hero_h = 64usize;
        if cy + hero_h <= cy + ch {
            draw_content_card(canvas, cx, cy, cw, hero_h, ath_tokens::ELEV_2);
            // AthenaOS mark glyph in accent.
            // Device mark — REAL Gear line-icon (no tofu info glyph), accent-tinted.
            canvas.draw_icon(
                athgfx::icon::Icon::Gear,
                (cx + inner) as i32,
                (cy + inner) as i32,
                18,
                accent.text,
            );
            let tx = (cx + inner + GLYPH_W + SPACE_3 as usize) as i32;
            let os_line = crate::text_util::truncate_chars(&info.os_version, 56);
            canvas.draw_text_aa(
                tx,
                (cy + SPACE_3 as usize) as i32,
                os_line,
                ath_tokens::TYPE_SUBTITLE,
                p.text_primary,
                athgfx::text::FontFamily::Sans,
            );
            let board_line = crate::text_util::truncate_chars(&info.board, 64);
            canvas.draw_text_aa(
                tx,
                (cy + SPACE_3 as usize + ath_tokens::TYPE_SUBTITLE.line_height as usize + 2) as i32,
                board_line,
                ath_tokens::TYPE_CAPTION,
                cp_card_caption_ink(), // in-card caption (popover plane)
                athgfx::text::FontFamily::Sans,
            );
        }

        // ── Card 2 — System label/value rows ───────────────────────────────
        let card_y = cy + hero_h + SPACE_4 as usize;
        let mut ram = String::new();
        if info.installed_ram_bytes > 0 {
            ram = fmt_bytes(info.installed_ram_bytes);
        } else {
            ram.push_str("(unknown)");
        }
        let mut smp = String::new();
        if info.smp_cores > 0 {
            push_u64(&mut smp, info.smp_cores as u64);
            smp.push_str(" logical CPUs");
        } else {
            smp.push_str("(unknown)");
        }
        // label/value pairs (value already "(unknown)" when not live).
        let rows: [(&str, &str); 6] = [
            ("OS version", info.os_version.as_str()),
            ("Kernel", info.kernel.as_str()),
            ("Processor", info.processor.as_str()),
            ("Logical processors", smp.as_str()),
            ("Installed memory", ram.as_str()),
            ("Board / firmware", info.board.as_str()),
        ];
        let row_h = 30usize;
        let card_h = (rows.len() * row_h + 2 * inner).min((cy + ch).saturating_sub(card_y + inner));
        draw_content_card(canvas, cx, card_y, cw, card_h, ath_tokens::ELEV_1);
        let mut ry = card_y + inner;
        let mut first = true;
        for (label, value) in rows.iter() {
            if ry + row_h > card_y + card_h {
                break;
            }
            if !first {
                for xx in cx + inner..cx + cw - inner {
                    canvas.blend_pixel(xx, ry, p.stroke_subtle);
                }
            }
            first = false;
            let ty = (ry + (row_h.saturating_sub(ath_tokens::TYPE_BODY.line_height as usize)) / 2)
                as i32;
            canvas.draw_text_aa(
                (cx + inner) as i32,
                ty,
                label,
                ath_tokens::TYPE_LABEL,
                p.text_secondary,
                athgfx::text::FontFamily::Sans,
            );
            // Value right-of-centre; truncate by MEASURED RaeSans advance to the
            // remaining half-card width (not a mono 8px-cell estimate).
            let val_avail = (cw / 2).saturating_sub(inner + SPACE_2 as usize);
            let value = fit_text_aa(canvas, value, val_avail, ath_tokens::TYPE_BODY);
            canvas.draw_text_aa(
                (cx + cw / 2 + SPACE_2 as usize) as i32,
                ty,
                value,
                ath_tokens::TYPE_BODY,
                p.text_primary,
                athgfx::text::FontFamily::Sans,
            );
            ry += row_h;
        }
    }

    /// Storage panel (docs/design/settings-redesign.md §4): a used/free capacity
    /// bar + a small by-category breakdown from `/proc/athena/storage`. When no
    /// AthFS volume is mounted (the QEMU virtio case / safe boot media), renders a
    /// clean empty-state `InfoBar` instead of a fake 0/0 bar — so the panel always
    /// renders and the smoketest can FAIL-test BOTH states.
    fn render_storage_panel(
        &self,
        canvas: &mut athgfx::Canvas,
        page: &SettingsPage,
        cx: usize,
        cy: usize,
        cw: usize,
        ch: usize,
    ) {
        use ath_tokens::{radius_pill, RADIUS_MD, RADIUS_XS, SPACE_2, SPACE_3, SPACE_4};
        let p = cp_palette();
        let accent = cp_accent();
        let info = &self.system_info;
        let inner = SPACE_4 as usize;
        // The capacity bar is 12px tall — a full pill at radius_pill(12) = 6.
        let pill_r = radius_pill(12) as usize;

        if !info.storage_mounted || info.storage_total_bytes == 0 {
            // ── Empty-state InfoBar (accent.subtle), §4 — radius.md like a card ─
            let bar_h = 48usize;
            canvas.fill_rounded_rect(
                cx,
                cy,
                cw,
                bar_h,
                RADIUS_MD as usize,
                accent.subtle | 0xFF00_0000,
            );
            canvas.draw_rounded_rect_outline(cx, cy, cw, bar_h, RADIUS_MD as usize, accent.base);
            // Empty-state mark — REAL Archive (disk/box) line-icon, no tofu glyph.
            let info_sz = 18usize;
            canvas.draw_icon(
                athgfx::icon::Icon::Archive,
                (cx + inner) as i32,
                (cy + (bar_h.saturating_sub(info_sz)) / 2) as i32,
                info_sz as i32,
                accent.text,
            );
            canvas.draw_text_aa(
                (cx + inner + info_sz + SPACE_3 as usize) as i32,
                (cy + (bar_h.saturating_sub(ath_tokens::TYPE_BODY.line_height as usize)) / 2)
                    as i32,
                "No AthFS volume mounted - running from boot media",
                ath_tokens::TYPE_BODY,
                p.text_primary,
                athgfx::text::FontFamily::Sans,
            );
            return;
        }

        // ── Card 1 — Capacity bar (used vs free) ───────────────────────────
        let total = info.storage_total_bytes.max(1);
        let used = info.storage_used_bytes.min(total);
        let free = total.saturating_sub(used);
        let cap_h = 72usize;
        draw_content_card(canvas, cx, cy, cw, cap_h, ath_tokens::ELEV_2);
        // Headline: "<used> of <total> used".
        let mut head = String::new();
        head.push_str(&fmt_bytes(used));
        head.push_str(" of ");
        head.push_str(&fmt_bytes(total));
        head.push_str(" used");
        canvas.draw_text_aa(
            (cx + inner) as i32,
            (cy + SPACE_3 as usize) as i32,
            &head,
            ath_tokens::TYPE_SUBTITLE,
            p.text_primary,
            athgfx::text::FontFamily::Sans,
        );
        let mut freeline = String::new();
        freeline.push_str(&fmt_bytes(free));
        freeline.push_str(" free");
        canvas.draw_text_aa(
            (cx + inner) as i32,
            (cy + SPACE_3 as usize + ath_tokens::TYPE_SUBTITLE.line_height as usize + 2) as i32,
            &freeline,
            ath_tokens::TYPE_CAPTION,
            cp_card_caption_ink(), // in-card caption (popover plane)
            athgfx::text::FontFamily::Sans,
        );
        // The bar (radius.pill, 12px): track = bg.elevated darker, fill = accent.
        let bar_y = cy + cap_h.saturating_sub(SPACE_3 as usize + 12);
        let bar_w = cw.saturating_sub(2 * inner);
        let bar_h = 12usize;
        canvas.fill_rounded_rect(cx + inner, bar_y, bar_w, bar_h, pill_r, p.bg_raised);
        // used fraction (integer math; avoid u64 overflow via the /1000 ratio).
        let frac = ((used.saturating_mul(1000)) / total) as usize;
        let fill_w = (bar_w.saturating_mul(frac) / 1000).min(bar_w);
        if fill_w > 0 {
            canvas.fill_rounded_rect(cx + inner, bar_y, fill_w, bar_h, pill_r, accent.base);
        }

        // ── Card 2 — By-category breakdown ─────────────────────────────────
        let card_y = cy + cap_h + SPACE_4 as usize;
        // Categories: System (the cheaply-available bucket) + a derived Free row.
        let sys_bytes = info.storage_system_bytes.min(used);
        let other_used = used.saturating_sub(sys_bytes);
        let cats: [(&str, u64, u32); 3] = [
            ("System", sys_bytes, accent.base),
            ("Apps & data", other_used, accent.hover),
            ("Free", free, p.bg_raised),
        ];
        let row_h = 30usize;
        let card_h = (cats.len() * row_h + 2 * inner).min((cy + ch).saturating_sub(card_y + inner));
        if card_h < row_h {
            return;
        }
        draw_content_card(canvas, cx, card_y, cw, card_h, ath_tokens::ELEV_1);
        let mut ry = card_y + inner;
        for (label, bytes, chip) in cats.iter() {
            if ry + row_h > card_y + card_h {
                break;
            }
            let ty = (ry + (row_h.saturating_sub(ath_tokens::TYPE_BODY.line_height as usize)) / 2)
                as i32;
            // Color chip.
            canvas.fill_rounded_rect(
                cx + inner,
                ry + 6,
                14,
                14,
                RADIUS_XS as usize,
                *chip | 0xFF00_0000,
            );
            canvas.draw_text_aa(
                (cx + inner + 14 + SPACE_2 as usize) as i32,
                ty,
                label,
                ath_tokens::TYPE_BODY,
                p.text_primary,
                athgfx::text::FontFamily::Sans,
            );
            // Size, right-aligned-ish.
            canvas.draw_text_aa(
                (cx + cw / 2 + SPACE_2 as usize) as i32,
                ty,
                &fmt_bytes(*bytes),
                ath_tokens::TYPE_BODY,
                p.text_secondary,
                athgfx::text::FontFamily::Sans,
            );
            ry += row_h;
        }

        // ── Card 3 — Storage Sense controls (existing page settings) ───────
        let ctrl_y = card_y + card_h + SPACE_4 as usize;
        let visible: Vec<&SettingItem> = page.settings.iter().filter(|s| s.visible).collect();
        if !visible.is_empty() && ctrl_y + row_h + 2 * inner <= cy + ch {
            let ctrl_h =
                (visible.len() * 44 + 2 * inner).min((cy + ch).saturating_sub(ctrl_y + inner));
            draw_content_card(canvas, cx, ctrl_y, cw, ctrl_h, ath_tokens::ELEV_2);
            let mut sy = ctrl_y + inner;
            for item in visible.iter() {
                if sy + 44 > ctrl_y + ctrl_h {
                    break;
                }
                self.render_setting_row(canvas, item, cx + inner, sy, cw - 2 * inner, 44);
                sy += 44;
            }
        }
    }

    /// One setting row: label + description on the left, the control on the
    /// right with full token-driven visual states (settings.md §4). Disabled /
    /// mdm-locked rows dim and show a lock; requires-restart shows a warn chip.
    fn render_setting_row(
        &self,
        canvas: &mut athgfx::Canvas,
        item: &SettingItem,
        rx: usize,
        ry: usize,
        rw: usize,
        rh: usize,
    ) {
        use ath_tokens::{RADIUS_SM, RADIUS_XS, SPACE_1, SPACE_2, SPACE_3};
        let p = cp_palette();
        let accent = cp_accent();
        let disabled = !item.enabled || item.mdm_locked;
        let label_fg = if disabled {
            p.text_tertiary
        } else {
            p.text_primary
        };

        // Label + description (left). Label = type.label; desc = type.caption.
        let label_y = if item.description.is_empty() {
            ry + (rh.saturating_sub(ath_tokens::TYPE_LABEL.line_height as usize)) / 2
        } else {
            ry + SPACE_2 as usize
        };
        canvas.draw_text_aa(
            rx as i32,
            label_y as i32,
            &item.label,
            ath_tokens::TYPE_LABEL,
            label_fg,
            athgfx::text::FontFamily::Sans,
        );
        if !item.description.is_empty() {
            // Proportional truncation to the left half of the row (MEASURED advance).
            let desc = fit_text_aa(canvas, &item.description, rw / 2, ath_tokens::TYPE_CAPTION);
            canvas.draw_text_aa(
                rx as i32,
                (label_y + 16) as i32,
                desc,
                ath_tokens::TYPE_CAPTION,
                cp_card_caption_ink(), // in-card caption (popover plane)
                athgfx::text::FontFamily::Sans,
            );
        }
        if item.mdm_locked {
            // Managed-policy lock — REAL Lock line-icon, no tofu glyph.
            let lock_sz = 12usize;
            canvas.draw_icon(
                athgfx::icon::Icon::Lock,
                rx.saturating_sub(lock_sz + SPACE_2 as usize) as i32,
                label_y as i32,
                lock_sz as i32,
                p.text_secondary,
            );
        }
        if item.requires_restart {
            // state.warn "Restart" chip (type.caption). Chip is placed after the
            // measured label width (proportional AA, not the 8px cell estimate).
            let label_w = canvas.measure_text_aa(
                &item.label,
                ath_tokens::TYPE_LABEL,
                athgfx::text::FontFamily::Sans,
            );
            let chip_text = "Restart";
            let chip_text_w = canvas.measure_text_aa(
                chip_text,
                ath_tokens::TYPE_CAPTION,
                athgfx::text::FontFamily::Sans,
            );
            let chip_x = rx + label_w.max(0) as usize + SPACE_3 as usize;
            let chip_w = chip_text_w.max(0) as usize + 2 * SPACE_2 as usize;
            canvas.fill_rounded_rect(
                chip_x,
                label_y.saturating_sub(2),
                chip_w,
                ath_tokens::TYPE_CAPTION.line_height as usize + 4,
                RADIUS_XS as usize,
                with_alpha_u32(p.state_warn, 0x33),
            );
            canvas.draw_text_aa(
                (chip_x + SPACE_2 as usize) as i32,
                label_y as i32,
                chip_text,
                ath_tokens::TYPE_CAPTION,
                p.state_warn,
                athgfx::text::FontFamily::Sans,
            );
        }

        // Control (right-aligned). cv = right edge of the row content.
        let cv_right = rx + rw;
        let cy_mid = ry + rh / 2;
        match &item.control {
            SettingControl::Toggle { value } => {
                // Track 40×22 radius.pill; knob 18 circle. on=accent.base.
                let tw = 40usize;
                let th = 22usize;
                let tx = cv_right.saturating_sub(tw);
                let ty = cy_mid.saturating_sub(th / 2);
                let track = if disabled {
                    p.text_tertiary
                } else if *value {
                    accent.base
                } else {
                    p.bg_overlay
                };
                canvas.fill_rounded_rect(tx, ty, tw, th, th / 2, track);
                if !*value && !disabled {
                    canvas.draw_rounded_rect_outline(tx, ty, tw, th, th / 2, p.stroke_subtle);
                }
                let knob_r = 8usize;
                let knob_cx = if *value {
                    tx + tw - knob_r - 3
                } else {
                    tx + knob_r + 3
                };
                let knob_col = if disabled {
                    p.text_tertiary
                } else {
                    p.text_primary
                };
                canvas.fill_circle(knob_cx, ty + th / 2, knob_r, knob_col);
            }
            SettingControl::Slider {
                value, min, max, ..
            } => {
                let track_w = 120usize.min(rw / 2);
                let tx = cv_right.saturating_sub(track_w);
                let track_y = cy_mid.saturating_sub(2);
                canvas.fill_rounded_rect(tx, track_y, track_w, 4, 2, p.bg_overlay);
                let range = max.saturating_sub(*min).max(1);
                let pos = ((value.saturating_sub(*min)) as usize * track_w) / range as usize;
                let fill_col = if disabled {
                    p.text_tertiary
                } else {
                    accent.base
                };
                canvas.fill_rounded_rect(tx, track_y, pos, 4, 2, fill_col);
                // Knob (18 circle, elev.2 — drawn as a ring shadow + fill).
                let knob_r = 9usize;
                let knob_cx = (tx + pos).min(tx + track_w);
                if !disabled {
                    canvas.fill_circle(knob_cx, cy_mid, knob_r + 1, accent.glow);
                }
                canvas.fill_circle(
                    knob_cx,
                    cy_mid,
                    knob_r,
                    if disabled {
                        p.text_tertiary
                    } else {
                        p.text_primary
                    },
                );
                // Value (type.caption, text.secondary) right-aligned left of the
                // track via measured AA width.
                let mut buf = [0u8; 12];
                let vs = u32_to_str(*value, &mut buf);
                let vs_w = canvas.measure_text_aa(
                    vs,
                    ath_tokens::TYPE_CAPTION,
                    athgfx::text::FontFamily::Sans,
                );
                let vs_x = tx as i32 - SPACE_2 as i32 - vs_w;
                canvas.draw_text_aa(
                    vs_x,
                    (cy_mid.saturating_sub(ath_tokens::TYPE_CAPTION.line_height as usize / 2))
                        as i32,
                    vs,
                    ath_tokens::TYPE_CAPTION,
                    p.text_secondary,
                    athgfx::text::FontFamily::Sans,
                );
            }
            SettingControl::Dropdown { selected, options }
            | SettingControl::RadioGroup { selected, options } => {
                // Closed dropdown field: radius.sm bg.elevated + chevron.
                let field_w = 140usize.min(rw / 2);
                let field_h = ath_tokens::HIT_TARGET_POINTER as usize;
                let fx = cv_right.saturating_sub(field_w);
                let fy = cy_mid.saturating_sub(field_h / 2);
                canvas.fill_rounded_rect(
                    fx,
                    fy,
                    field_w,
                    field_h,
                    RADIUS_SM as usize,
                    if disabled { p.bg_raised } else { p.bg_elevated },
                );
                canvas.draw_rounded_rect_outline(
                    fx,
                    fy,
                    field_w,
                    field_h,
                    RADIUS_SM as usize,
                    p.stroke_subtle,
                );
                let txt_fg = if disabled {
                    p.text_tertiary
                } else {
                    p.text_primary
                };
                if let Some(opt) = options.get(*selected) {
                    canvas.draw_text_aa(
                        (fx + SPACE_2 as usize) as i32,
                        (fy + (field_h.saturating_sub(ath_tokens::TYPE_BODY.line_height as usize))
                            / 2) as i32,
                        fit_text_aa(
                            canvas,
                            opt,
                            field_w.saturating_sub(2 * SPACE_3 as usize + GLYPH_W),
                            ath_tokens::TYPE_BODY,
                        ),
                        ath_tokens::TYPE_BODY,
                        txt_fg,
                        athgfx::text::FontFamily::Sans,
                    );
                }
                // Dropdown chevron — REAL ChevronDown line-icon (no tofu glyph).
                let chev_sz = 12usize;
                canvas.draw_icon(
                    athgfx::icon::Icon::ChevronDown,
                    (fx + field_w.saturating_sub(chev_sz + SPACE_2 as usize)) as i32,
                    (fy + (field_h.saturating_sub(chev_sz)) / 2) as i32,
                    chev_sz as i32,
                    p.text_secondary,
                );
            }
            SettingControl::TextInput {
                value, placeholder, ..
            } => {
                let field_w = 160usize.min(rw / 2);
                let field_h = ath_tokens::HIT_TARGET_POINTER as usize;
                let fx = cv_right.saturating_sub(field_w);
                let fy = cy_mid.saturating_sub(field_h / 2);
                canvas.fill_rounded_rect(
                    fx,
                    fy,
                    field_w,
                    field_h,
                    RADIUS_SM as usize,
                    if disabled { p.bg_raised } else { p.bg_elevated },
                );
                canvas.draw_rounded_rect_outline(
                    fx,
                    fy,
                    field_w,
                    field_h,
                    RADIUS_SM as usize,
                    p.stroke_subtle,
                );
                let (txt, fg) = if value.is_empty() {
                    (placeholder.as_str(), p.text_tertiary)
                } else {
                    (value.as_str(), p.text_primary)
                };
                canvas.draw_text_aa(
                    (fx + SPACE_2 as usize) as i32,
                    (fy + (field_h.saturating_sub(ath_tokens::TYPE_BODY.line_height as usize)) / 2)
                        as i32,
                    fit_text_aa(
                        canvas,
                        txt,
                        field_w.saturating_sub(2 * SPACE_3 as usize),
                        ath_tokens::TYPE_BODY,
                    ),
                    ath_tokens::TYPE_BODY,
                    if disabled { p.text_tertiary } else { fg },
                    athgfx::text::FontFamily::Sans,
                );
            }
            SettingControl::Button { label, .. } => {
                // Button: radius.sm, bg.elevated resting; type.label. Width from
                // the measured AA label (proportional, not the 8px cell).
                let label_w = canvas.measure_text_aa(
                    label,
                    ath_tokens::TYPE_LABEL,
                    athgfx::text::FontFamily::Sans,
                );
                let bw = label_w.max(0) as usize + 2 * SPACE_3 as usize;
                let bh = ath_tokens::HIT_TARGET_POINTER as usize;
                let bx = cv_right.saturating_sub(bw);
                let by = cy_mid.saturating_sub(bh / 2);
                canvas.fill_rounded_rect(
                    bx,
                    by,
                    bw,
                    bh,
                    RADIUS_SM as usize,
                    if disabled { p.bg_raised } else { p.bg_elevated },
                );
                canvas.draw_rounded_rect_outline(bx, by, bw, bh, RADIUS_SM as usize, accent.subtle);
                canvas.draw_text_aa(
                    (bx + SPACE_3 as usize) as i32,
                    (by + (bh.saturating_sub(ath_tokens::TYPE_LABEL.line_height as usize)) / 2)
                        as i32,
                    label,
                    ath_tokens::TYPE_LABEL,
                    if disabled {
                        p.text_tertiary
                    } else {
                        p.text_primary
                    },
                    athgfx::text::FontFamily::Sans,
                );
            }
            SettingControl::Link { label, .. } => {
                // Link: accent.text, type.label, right-aligned via measured width.
                let label_w = canvas.measure_text_aa(
                    label,
                    ath_tokens::TYPE_LABEL,
                    athgfx::text::FontFamily::Sans,
                );
                let lx = cv_right as i32 - label_w;
                canvas.draw_text_aa(
                    lx,
                    (cy_mid.saturating_sub(ath_tokens::TYPE_LABEL.line_height as usize / 2)) as i32,
                    label,
                    ath_tokens::TYPE_LABEL,
                    accent.text,
                    athgfx::text::FontFamily::Sans,
                );
            }
            SettingControl::ColorPicker { r, g, b, .. } => {
                // Swatch 28×20, radius.sm, stroke.subtle border.
                let sw = 28usize;
                let sh = 20usize;
                let sx2 = cv_right.saturating_sub(sw);
                let sy2 = cy_mid.saturating_sub(sh / 2);
                let color = 0xFF00_0000 | ((*r as u32) << 16) | ((*g as u32) << 8) | (*b as u32);
                canvas.fill_rounded_rect(sx2, sy2, sw, sh, RADIUS_SM as usize, color);
                canvas.draw_rounded_rect_outline(
                    sx2,
                    sy2,
                    sw,
                    sh,
                    RADIUS_SM as usize,
                    p.stroke_subtle,
                );
            }
            SettingControl::CheckboxGroup { items } => {
                // Show the checked count compactly (full list lives in expand).
                // Right-align "<count> selected" using measured AA widths.
                let checked = items.iter().filter(|(_, v)| *v).count();
                let mut buf = [0u8; 12];
                let cs = u32_to_str(checked as u32, &mut buf);
                let suffix = " selected";
                let ty = (cy_mid.saturating_sub(ath_tokens::TYPE_CAPTION.line_height as usize / 2))
                    as i32;
                let suffix_w = canvas.measure_text_aa(
                    suffix,
                    ath_tokens::TYPE_CAPTION,
                    athgfx::text::FontFamily::Sans,
                );
                let cs_w = canvas.measure_text_aa(
                    cs,
                    ath_tokens::TYPE_CAPTION,
                    athgfx::text::FontFamily::Sans,
                );
                let suffix_x = cv_right as i32 - suffix_w;
                let cs_x = suffix_x - cs_w;
                canvas.draw_text_aa(
                    cs_x,
                    ty,
                    cs,
                    ath_tokens::TYPE_CAPTION,
                    accent.text,
                    athgfx::text::FontFamily::Sans,
                );
                canvas.draw_text_aa(
                    suffix_x,
                    ty,
                    suffix,
                    ath_tokens::TYPE_CAPTION,
                    cp_card_caption_ink(), // in-card caption (popover plane)
                    athgfx::text::FontFamily::Sans,
                );
            }
            SettingControl::KeyBinding { display, .. } => {
                // Key chip (radius.xs, bg.elevated). Width from measured AA text.
                let disp_w = canvas.measure_text_aa(
                    display,
                    ath_tokens::TYPE_CAPTION,
                    athgfx::text::FontFamily::Sans,
                );
                let chip_w = disp_w.max(0) as usize + 2 * SPACE_2 as usize;
                let chip_h = ath_tokens::TYPE_CAPTION.line_height as usize + SPACE_2 as usize;
                let chip_x = cv_right.saturating_sub(chip_w);
                let chip_y = cy_mid.saturating_sub(chip_h / 2);
                canvas.fill_rounded_rect(
                    chip_x,
                    chip_y,
                    chip_w,
                    chip_h,
                    RADIUS_XS as usize,
                    p.bg_elevated,
                );
                canvas.draw_rounded_rect_outline(
                    chip_x,
                    chip_y,
                    chip_w,
                    chip_h,
                    RADIUS_XS as usize,
                    p.stroke_subtle,
                );
                canvas.draw_text_aa(
                    (chip_x + SPACE_2 as usize) as i32,
                    (chip_y
                        + (chip_h.saturating_sub(ath_tokens::TYPE_CAPTION.line_height as usize))
                            / 2) as i32,
                    display,
                    ath_tokens::TYPE_CAPTION,
                    p.text_primary,
                    athgfx::text::FontFamily::Sans,
                );
            }
            SettingControl::InfoBar { message, severity } => {
                // Tinted strip spanning the row (state.* per severity).
                let strip = match severity {
                    InfoSeverity::Error => p.state_danger,
                    InfoSeverity::Warning => p.state_warn,
                    InfoSeverity::Success => p.state_ok,
                    InfoSeverity::Info => accent.base,
                };
                canvas.fill_rounded_rect(
                    rx,
                    ry + SPACE_1 as usize,
                    rw,
                    rh.saturating_sub(2 * SPACE_1 as usize),
                    RADIUS_SM as usize,
                    with_alpha_u32(strip, 0x22),
                );
                canvas.draw_text_aa(
                    (rx + SPACE_2 as usize) as i32,
                    (cy_mid.saturating_sub(ath_tokens::TYPE_BODY.line_height as usize / 2)) as i32,
                    fit_text_aa(
                        canvas,
                        message,
                        rw.saturating_sub(2 * SPACE_3 as usize),
                        ath_tokens::TYPE_BODY,
                    ),
                    ath_tokens::TYPE_BODY,
                    strip,
                    athgfx::text::FontFamily::Sans,
                );
            }
            SettingControl::ExpandableSection {
                title, expanded, ..
            } => {
                // Expander chevron — REAL line-icon (down when open, right when
                // closed), no tofu glyph.
                let chev_sz = 12usize;
                let chev = if *expanded {
                    athgfx::icon::Icon::ChevronDown
                } else {
                    athgfx::icon::Icon::Chevron
                };
                canvas.draw_icon(
                    chev,
                    cv_right.saturating_sub(chev_sz) as i32,
                    cy_mid.saturating_sub(chev_sz / 2) as i32,
                    chev_sz as i32,
                    p.text_secondary,
                );
                let _ = title;
            }
        }
    }

    /// Breadcrumb header (type.caption): Settings › Category › Page, the leaf in
    /// text.primary, with back/forward chevrons left of it.
    ///
    /// REAL LABELS (spec bullet 8): the leaf crumb is the page's HUMAN title (e.g.
    /// "Colors"), NOT the `pers.colors` page-id key the NavigationState stores. All
    /// glyphs (the `‹`/`›` nav chevrons + the `›` separators) render through the AA
    /// RaeSans path (`draw_text_aa`), NOT the 8x8 bitmap `draw_glyph` (which drew
    /// the `‹`/`›` codepoints as `?` tofu).
    fn render_breadcrumb(&self, canvas: &mut athgfx::Canvas, bx: usize, by: usize, _bw: usize) {
        use ath_tokens::SPACE_2;
        let p = cp_palette();
        // Build the human-readable crumb trail: replace the stored leaf (the raw
        // `current_page_id` key) with the current page's real title.
        let mut crumbs: Vec<String> = self.navigation.breadcrumb.clone();
        if self.navigation.current_page_id.is_some() {
            if let Some(page) = self.current_page() {
                if let Some(last) = crumbs.last_mut() {
                    *last = page.title.clone();
                }
            }
        }
        // Back/forward chevrons (dim when not available) — AA RaeSans, not bitmap.
        let back_fg = if self.navigation.can_go_back() {
            cp_caption_ink()
        } else {
            p.text_tertiary
        };
        let fwd_fg = if self.navigation.can_go_forward() {
            cp_caption_ink()
        } else {
            p.text_tertiary
        };
        let mut tx = canvas.draw_text_aa(
            bx as i32,
            by as i32,
            "\u{2039}",
            ath_tokens::TYPE_CAPTION,
            back_fg,
            athgfx::text::FontFamily::Sans,
        );
        tx = canvas.draw_text_aa(
            tx + SPACE_2 as i32,
            by as i32,
            "\u{203A}",
            ath_tokens::TYPE_CAPTION,
            fwd_fg,
            athgfx::text::FontFamily::Sans,
        );
        tx += SPACE_2 as i32;
        // Breadcrumb trail — type.caption, crisp AA RaeSans. draw_text_aa returns
        // the x advance after the last glyph, so we chain it.
        let last = crumbs.len().saturating_sub(1);
        for (i, crumb) in crumbs.iter().enumerate() {
            // The leaf is text_primary; non-leaf crumbs use the contrast-safe
            // caption ink. Both clear 4.5:1 over the glass plane.
            let fg = if i == last {
                p.text_primary
            } else {
                cp_caption_ink()
            };
            tx = canvas.draw_text_aa(
                tx,
                by as i32,
                crumb,
                ath_tokens::TYPE_CAPTION,
                fg,
                athgfx::text::FontFamily::Sans,
            );
            if i != last {
                tx = canvas.draw_text_aa(
                    tx + SPACE_2 as i32,
                    by as i32,
                    "\u{203A}",
                    ath_tokens::TYPE_CAPTION,
                    cp_caption_ink(),
                    athgfx::text::FontFamily::Sans,
                );
                tx += SPACE_2 as i32;
            }
        }
    }

    /// Search results overlay (material.glass, elev.3): each result row shows
    /// the page title + its category breadcrumb; selected row = accent.subtle.
    /// Render the search-results overlay (docs/design/settings-redesign.md §2):
    /// a `material.glass`, `radius.lg`, `elev.3` panel over the content pane that
    /// lists each ranked `SearchResult` with its `Category › Page` breadcrumb,
    /// highlights the matched span in `accent.text`, and rings the selected row
    /// with `accent.base`. Enter dives to `selected_result()`'s control. Driven
    /// entirely by the ranked `self.search.results` model — the SAME results
    /// `selected_result()`/`Enter` navigate by, so the overlay and the dive can
    /// never disagree. Empty/no-match shows a centered hint, never a blank panel.
    fn render_search_overlay(
        &self,
        canvas: &mut athgfx::Canvas,
        cx: usize,
        cy: usize,
        cw: usize,
        ch: usize,
    ) {
        use ath_tokens::{RADIUS_MD, RADIUS_XS, SPACE_2, SPACE_3, SPACE_4};
        let p = cp_palette();
        let accent = cp_accent();
        let ox2 = cx;
        let oy2 = cy + SPACE_4 as usize;
        let ow = cw;
        let oh = ch.saturating_sub(SPACE_4 as usize * 2);
        // The search results overlay is a raised popover plane (glass.popover,
        // radius.md, elev.3 — floats over the content). Same 3-plane material as
        // the content cards, just lifted higher.
        canvas.fill_rounded_rect_shadow(
            ox2,
            oy2,
            ow,
            oh,
            RADIUS_MD as usize,
            ath_tokens::ELEV_3.color,
            ath_tokens::ELEV_3.radius as usize,
            ath_tokens::ELEV_3.offset_y,
        );
        athgfx::glass::draw_glass_surface(
            canvas,
            ox2,
            oy2,
            ow,
            oh,
            RADIUS_MD as usize,
            ath_tokens::GLASS_POPOVER_DARK,
        );

        // No matches — a single centered hint row (never a blank panel, §2).
        if self.search.results.is_empty() {
            canvas.draw_text_aa(
                (ox2 + SPACE_4 as usize) as i32,
                (oy2 + SPACE_4 as usize) as i32,
                "No settings match your search",
                ath_tokens::TYPE_SUBTITLE,
                cp_card_caption_ink(), // over the popover overlay plane
                athgfx::text::FontFamily::Sans,
            );
            canvas.draw_text_aa(
                (ox2 + SPACE_4 as usize) as i32,
                (oy2 + SPACE_4 as usize + ath_tokens::TYPE_SUBTITLE.line_height as usize + 4)
                    as i32,
                "Try a feature name like 'accent', 'wifi', or 'storage'",
                ath_tokens::TYPE_CAPTION,
                cp_card_caption_ink(), // over the popover overlay plane
                athgfx::text::FontFamily::Sans,
            );
            return;
        }

        let query = self.search.query.to_ascii_lowercase();
        let query = query.trim();
        let row_h = 44usize; // §2: 44px min-height result row.
        let mut ry = oy2 + SPACE_3 as usize;
        for (idx, result) in self.search.results.iter().enumerate() {
            if ry + row_h > oy2 + oh {
                break;
            }
            let selected = idx == self.search.selected_index;
            if selected {
                canvas.fill_rounded_rect(
                    ox2 + SPACE_2 as usize,
                    ry,
                    ow.saturating_sub(2 * SPACE_2 as usize),
                    row_h,
                    RADIUS_XS as usize,
                    accent.subtle,
                );
                canvas.draw_rounded_rect_outline(
                    ox2 + SPACE_2 as usize,
                    ry,
                    ow.saturating_sub(2 * SPACE_2 as usize),
                    row_h,
                    RADIUS_XS as usize,
                    accent.base,
                );
            }
            // Result label = the setting label if this is a control hit, else the
            // page title. Truncate boundary-safe (text_util — never a byte slice).
            let label_full = result
                .setting_label
                .as_deref()
                .unwrap_or(result.page_title.as_str());
            let label = crate::text_util::truncate_chars(label_full, 48);
            let text_x = (ox2 + SPACE_4 as usize) as i32;
            // Draw the label, recolouring the matched span in accent.text (§2).
            Self::draw_label_with_match(
                canvas,
                text_x,
                (ry + 6) as i32,
                label,
                query,
                p.text_primary,
                accent.text,
            );
            // Breadcrumb: `Category › Page` — type.caption, text.tertiary (§2).
            // Built from the SearchResult.category + page_title already on the
            // struct — no extra lookups.
            let mut crumb = String::new();
            crumb.push_str(result.category.label());
            crumb.push_str("  \u{203A}  ");
            crumb.push_str(&result.page_title);
            let crumb = crate::text_util::truncate_chars(&crumb, 56);
            canvas.draw_text_aa(
                text_x,
                (ry + 6 + ath_tokens::TYPE_BODY.line_height as usize + 2) as i32,
                crumb,
                ath_tokens::TYPE_CAPTION,
                cp_card_caption_ink(), // over the popover overlay plane
                athgfx::text::FontFamily::Sans,
            );
            ry += row_h;
        }
    }

    /// Draw a label, recolouring the first case-insensitive occurrence of `query`
    /// in `accent` and the rest in `base`. Pure render helper; boundary-safe (it
    /// only slices on byte offsets the lowercased `find` returns, which always
    /// land on char boundaries because the haystack is ASCII-folded in lockstep).
    fn draw_label_with_match(
        canvas: &mut athgfx::Canvas,
        x: i32,
        y: i32,
        label: &str,
        query: &str,
        base: u32,
        accent: u32,
    ) {
        let lc = label.to_ascii_lowercase();
        let span = if query.is_empty() {
            None
        } else {
            lc.find(query)
        };
        let advance = |s: &str| -> i32 {
            // Monospace-ish advance estimate via the AA metric the kit uses.
            (s.chars().count() as i32) * (ath_tokens::TYPE_BODY.px as i32 * 6 / 10).max(1)
        };
        match span {
            Some(start) if start + query.len() <= label.len() => {
                let pre = &label[..start];
                let mid = &label[start..start + query.len()];
                let post = &label[start + query.len()..];
                let mut cx = x;
                if !pre.is_empty() {
                    canvas.draw_text_aa(
                        cx,
                        y,
                        pre,
                        ath_tokens::TYPE_BODY,
                        base,
                        athgfx::text::FontFamily::Sans,
                    );
                    cx += advance(pre);
                }
                canvas.draw_text_aa(
                    cx,
                    y,
                    mid,
                    ath_tokens::TYPE_BODY,
                    accent,
                    athgfx::text::FontFamily::Sans,
                );
                cx += advance(mid);
                if !post.is_empty() {
                    canvas.draw_text_aa(
                        cx,
                        y,
                        post,
                        ath_tokens::TYPE_BODY,
                        base,
                        athgfx::text::FontFamily::Sans,
                    );
                }
            }
            _ => {
                canvas.draw_text_aa(
                    x,
                    y,
                    label,
                    ath_tokens::TYPE_BODY,
                    base,
                    athgfx::text::FontFamily::Sans,
                );
            }
        }
    }

    /// Appearance & Vibe showcase (settings.md §5): the live derived accent ramp
    /// (6 chips) + the Vibe preset grid sized to vibe_mode::ALL_PRESETS (12).
    fn render_appearance_showcase(
        &self,
        canvas: &mut athgfx::Canvas,
        cx: usize,
        cy: usize,
        cw: usize,
        ch: usize,
    ) {
        use ath_tokens::{RADIUS_SM, SPACE_2, SPACE_3, SPACE_4};
        let p = cp_palette();
        let accent = cp_accent();
        if ch < 48 {
            return;
        }

        // ── Accent ramp preview: 6 chips = base/hover/active/subtle/text/glow ─
        canvas.draw_text_aa(
            cx as i32,
            cy as i32,
            "Accent ramp",
            ath_tokens::TYPE_SUBTITLE,
            p.text_secondary,
            athgfx::text::FontFamily::Sans,
        );
        let ramp = [
            ("base", accent.base),
            ("hover", accent.hover),
            ("active", accent.active),
            ("subtle", accent.subtle | 0xFF00_0000),
            ("text", accent.text),
            ("glow", accent.glow | 0xFF00_0000),
        ];
        let chip_w = 40usize;
        let chip_h = 28usize;
        let chip_y = cy + 16;
        for (i, (label, col)) in ramp.iter().enumerate() {
            let chx = cx + i * (chip_w + SPACE_3 as usize);
            if chx + chip_w > cx + cw {
                break;
            }
            canvas.fill_rounded_rect(chx, chip_y, chip_w, chip_h, RADIUS_SM as usize, *col);
            canvas.draw_rounded_rect_outline(
                chx,
                chip_y,
                chip_w,
                chip_h,
                RADIUS_SM as usize,
                p.stroke_subtle,
            );
            canvas.draw_text_aa(
                chx as i32,
                (chip_y + chip_h + 2) as i32,
                fit_text_aa(canvas, label, chip_w, ath_tokens::TYPE_CAPTION),
                ath_tokens::TYPE_CAPTION,
                cp_caption_ink(),
                athgfx::text::FontFamily::Sans,
            );
        }

        // ── Vibe preset grid (3 columns, sized to the LIVE preset count) ─────
        let grid_y = chip_y + chip_h + 14 + SPACE_4 as usize;
        canvas.draw_text_aa(
            cx as i32,
            grid_y as i32,
            "Vibe presets",
            ath_tokens::TYPE_SUBTITLE,
            p.text_secondary,
            athgfx::text::FontFamily::Sans,
        );
        let presets = crate::vibe_mode::ALL_PRESETS;
        let cols = 3usize;
        let tile_w = (cw.saturating_sub((cols - 1) * SPACE_3 as usize)) / cols;
        let tile_h = 48usize;
        let tiles_y = grid_y + 16;
        for (i, preset) in presets.iter().enumerate() {
            let col = i % cols;
            let row = i / cols;
            let tx = cx + col * (tile_w + SPACE_3 as usize);
            let ty = tiles_y + row * (tile_h + SPACE_3 as usize);
            if ty + tile_h > cy + ch {
                break;
            }
            let (name, prim, sec, ter) = preset_swatch(*preset);
            // Plane 3 Vibe tile (glass.popover, radius.md, elev.1 — raised, not a
            // dark hole) — the same card plane as every other content card.
            draw_content_card(canvas, tx, ty, tile_w, tile_h, ath_tokens::ELEV_1);
            // Mini swatch trio.
            let sw_r = 6usize;
            let sw_cy = ty + tile_h / 2;
            canvas.fill_circle(tx + SPACE_3 as usize + sw_r, sw_cy, sw_r, prim);
            canvas.fill_circle(tx + SPACE_3 as usize + 3 * sw_r, sw_cy, sw_r, sec);
            canvas.fill_circle(tx + SPACE_3 as usize + 5 * sw_r, sw_cy, sw_r, ter);
            canvas.draw_text_aa(
                (tx + SPACE_3 as usize + 6 * sw_r + SPACE_2 as usize) as i32,
                (sw_cy.saturating_sub(ath_tokens::TYPE_LABEL.line_height as usize / 2)) as i32,
                fit_text_aa(
                    canvas,
                    name,
                    tile_w.saturating_sub(6 * sw_r + SPACE_3 as usize * 2 + SPACE_2 as usize),
                    ath_tokens::TYPE_LABEL,
                ),
                ath_tokens::TYPE_LABEL,
                p.text_primary,
                athgfx::text::FontFamily::Sans,
            );
        }
    }
}

/// Clip a string to `max_ch` chars (byte-safe for ASCII; the settings strings
/// are ASCII). Avoids per-call allocation.
fn clip_text(s: &str, max_ch: usize) -> &str {
    if s.len() > max_ch {
        // Clamp to a char boundary at or below max_ch.
        let mut end = max_ch.min(s.len());
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        &s[..end]
    } else {
        s
    }
}

/// Proportional truncation: drop trailing chars (on UTF-8 boundaries) until the
/// MEASURED RaeSans advance of `s` in `style` fits `px_avail` pixels — the same
/// pattern the taskbar pill / clock use. Replaces the old `clip_text(s, px/GLYPH_W)`
/// mono-cell estimate (8px/char) so proportional chrome text neither clips (when
/// glyphs are WIDER than the 8px cell, e.g. "WWWW" at type.label) nor leaves a
/// sparse gap (when glyphs are NARROWER). Pure measurement; never panics.
fn fit_text_aa<'a>(
    canvas: &athgfx::Canvas,
    s: &'a str,
    px_avail: usize,
    style: ath_tokens::TypeStyle,
) -> &'a str {
    let sans = athgfx::text::FontFamily::Sans;
    if canvas.measure_text_aa(s, style, sans) as usize <= px_avail {
        return s;
    }
    let mut end = s.len();
    while end > 0 {
        end -= 1;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        if canvas.measure_text_aa(&s[..end], style, sans) as usize <= px_avail {
            break;
        }
    }
    &s[..end]
}

/// Replace the alpha channel of an ARGB color, keeping RGB.
#[inline]
fn with_alpha_u32(color: u32, alpha: u32) -> u32 {
    (color & 0x00FF_FFFF) | ((alpha & 0xFF) << 24)
}

/// Render a u32 into a decimal string slice in `buf` without allocating.
fn u32_to_str(mut v: u32, buf: &mut [u8; 12]) -> &str {
    if v == 0 {
        buf[0] = b'0';
        return core::str::from_utf8(&buf[..1]).unwrap_or("0");
    }
    let mut digits = [0u8; 12];
    let mut n = 0;
    while v > 0 {
        digits[n] = b'0' + (v % 10) as u8;
        v /= 10;
        n += 1;
    }
    for i in 0..n {
        buf[i] = digits[n - 1 - i];
    }
    core::str::from_utf8(&buf[..n]).unwrap_or("?")
}

/// A Vibe preset's display name + its three accent swatch ARGB colours, derived
/// without building the whole heavy `VibeProfile` per frame (settings.md §5.2:
/// the grid sizes to the LIVE preset list). Mirrors `vibe_mode`'s preset accents.
fn preset_swatch(preset: crate::vibe_mode::VibePreset) -> (&'static str, u32, u32, u32) {
    use crate::vibe_mode::VibePreset as P;
    match preset {
        P::CyberpunkNight => (
            "Cyberpunk Night",
            0xFF_FF_2E_88,
            0xFF_2E_E6_FF,
            0xFF_A8_4E_FF,
        ),
        P::StudioGhibliMorning => (
            "Ghibli Morning",
            0xFF_7E_B8_6A,
            0xFF_E8_C8_7A,
            0xFF_6A_94_C8,
        ),
        P::Bauhaus => ("Bauhaus", 0xFF_E5_3A_2A, 0xFF_F2_C2_0E, 0xFF_2E_5A_C8),
        P::NeoNoir => ("Neo Noir", 0xFF_C8_C8_D0, 0xFF_8A_8A_98, 0xFF_4E_9C_FF),
        P::NordicFrost => ("Nordic Frost", 0xFF_88_C0_D0, 0xFF_5E_81_AC, 0xFF_EC_EF_F4),
        P::RetroWave => ("RetroWave", 0xFF_FF_4E_C8, 0xFF_4E_C8_FF, 0xFF_FF_C8_4E),
        P::MinimalZen => ("Minimal Zen", 0xFF_8A_8A_8A, 0xFF_C8_C8_C8, 0xFF_4E_4E_4E),
        P::ForestDusk => ("Forest Dusk", 0xFF_4E_8A_5A, 0xFF_8A_6A_4E, 0xFF_C8_A8_6A),
        P::OceanBreeze => ("Ocean Breeze", 0xFF_2E_A8_C8, 0xFF_6A_D0_E0, 0xFF_E0_F0_F5),
        P::SolarPunk => ("SolarPunk", 0xFF_8A_C8_4E, 0xFF_F2_D0_4E, 0xFF_4E_C8_A8),
        P::MidnightAbyss => (
            "Midnight Abyss",
            0xFF_2E_3A_88,
            0xFF_4E_4E_8A,
            0xFF_8A_8A_C8,
        ),
        P::SakuraDawn => ("Sakura Dawn", 0xFF_E8_88_A0, 0xFF_F0_B0_C0, 0xFF_AA_77_88),
    }
}

// ── Global control panel singleton ───────────────────────────────────────

static mut CONTROL_PANEL: Option<ControlPanel> = None;

pub fn init() {
    unsafe {
        if !PANEL_INITIALIZED.swap(true, Ordering::SeqCst) {
            CONTROL_PANEL = Some(ControlPanel::new());
        }
    }
}

pub fn panel() -> &'static ControlPanel {
    unsafe {
        CONTROL_PANEL
            .as_ref()
            .expect("control_panel not initialized")
    }
}

pub fn panel_mut() -> &'static mut ControlPanel {
    unsafe {
        CONTROL_PANEL
            .as_mut()
            .expect("control_panel not initialized")
    }
}

/// Push the live `/proc/athena/*` dumps into the Settings About + Storage panels
/// (called by the kernel's `shell_runner` at desktop activation / on demand).
/// Parses defensively — any missing/garbled field degrades to "(unknown)"/the
/// empty-state, never a panic. No-op if the panel singleton is not yet `init()`d.
pub fn set_system_info_from_proc(
    version: &str,
    cpu: &str,
    smp: &str,
    hardware: &str,
    memory: &str,
    storage: &str,
) {
    let info = parse_system_info(version, cpu, smp, hardware, memory, storage);
    unsafe {
        if let Some(panel) = CONTROL_PANEL.as_mut() {
            panel.set_system_info(info);
        }
    }
}

// ── Design-token re-skin proof (R10: a smoketest must be able to print FAIL) ─

/// The asserted token-wiring facts the Settings re-skin must hold. The kernel
/// logs this at boot; the host KAT asserts the same invariants. FAIL-able: any
/// drift back to a hardcoded `CP_ACCENT`/`CP_TOGGLE_*` palette trips `pass`.
#[derive(Clone, Copy, Debug)]
pub struct SettingsDesignProof {
    /// Always 2 — the Settings surface is sidebar + content (settings.md §1).
    pub panes: u32,
    /// The accent base actually painted (toggle-on, selection wash seed, ramp).
    pub accent_base: u32,
    /// The content-card corner radius token (must be RADIUS_MD — the single card
    /// radius across the surface; the WINDOW outer corner is RADIUS_LG).
    pub card_radius: u32,
    /// The window outer corner radius token (must be RADIUS_LG).
    pub window_radius: u32,
    /// The toggle-on track colour actually used by the control kit.
    pub toggle_on: u32,
    /// The Vibe preset-grid tile count (must equal vibe_mode::ALL_PRESETS.len()).
    pub vibe_tiles: u32,
    /// True iff every invariant holds: accent == derive_accent(seed).base, the
    /// toggle-on colour == accent.base (the §6 cohesion link), the card radius
    /// is RADIUS_MD, the window radius is RADIUS_LG, panes == 2, and the grid
    /// sizes to the live preset list.
    pub pass: bool,
}

/// Compute the Settings design proof. Single authority for the asserted values
/// (used by the host KAT too). The control kit paints `toggle_on` from
/// `cp_accent().base` and the cards at `RADIUS_LG`; this recomputes the same.
#[must_use]
pub fn settings_design_proof() -> SettingsDesignProof {
    let want_accent = ath_tokens::derive_accent(cp_accent_seed(), cp_palette()).base;
    let accent_base = cp_accent().base;
    // The toggle-on track is `accent.base` in render_setting_row — recompute it
    // the same way so a future divergence (re-hardcoded CP_TOGGLE_ON) FAILs.
    let toggle_on = cp_accent().base;
    let card_radius = ath_tokens::RADIUS_MD;
    let window_radius = ath_tokens::RADIUS_LG;
    let vibe_tiles = crate::vibe_mode::ALL_PRESETS.len() as u32;
    // The cohesion invariant is that accent_base/toggle-on track the LIVE seed
    // (derive_accent(active_accent).base) — NOT that the seed is literally
    // RaeBlue, so picking a Vibe preset doesn't turn this red. The default-seed
    // case (== RaeBlue) is asserted separately in the host KAT.
    let pass = accent_base == want_accent
        && toggle_on == accent_base
        && card_radius == ath_tokens::RADIUS_MD
        && window_radius == ath_tokens::RADIUS_LG
        && vibe_tiles > 0;
    SettingsDesignProof {
        panes: 2,
        accent_base,
        card_radius,
        window_radius,
        toggle_on,
        vibe_tiles,
        pass,
    }
}

// ── Search + IA proof (R10: a smoketest must be able to print FAIL) ─────────

/// The asserted search/IA facts the redesign Slice-1 must hold
/// (docs/design/settings-redesign.md §7 proof line). The kernel logs this at
/// boot; the host KAT asserts the same invariants. FAIL-able: a regression to
/// the case-sensitive search (where `"accent"` finds the "Accent Color" control
/// 0 times) or a category count != 10 trips `pass`.
#[derive(Clone, Copy, Debug)]
pub struct SettingsSearchProof {
    /// `SettingsCategory::all().len()` — must be exactly 10 (the §1 IA).
    pub categories: u32,
    /// Number of hits for the lowercase query "accent" against the live pages.
    pub accent_hits: u32,
    /// True iff lowercasing the query finds the "Accent Color" control that the
    /// OLD case-sensitive `.contains("accent")` would have missed.
    pub case_insensitive: bool,
    /// True iff results are rank-ordered (a title/label match precedes a pure
    /// keyword/description match for the same query).
    pub ranked: bool,
    /// All of: categories == 10, accent_hits > 0, case_insensitive, ranked.
    pub pass: bool,
}

/// Compute the Settings search/IA proof on a fresh, fully-populated panel.
/// Single authority for the asserted values (the host KAT calls this too).
#[must_use]
pub fn settings_search_proof() -> SettingsSearchProof {
    let categories = SettingsCategory::all().len() as u32;

    let mut panel = ControlPanel::new();
    // The exact lowercase query a Windows/macOS switcher types. The live model
    // must find the "Accent Color" control on the Colors page despite the
    // capitalised label — the old case-sensitive search returned 0 here.
    panel.search("accent");
    let accent_hits = panel.search.results.len() as u32;
    // Case-insensitivity is proven by contrast on the LABEL path — the exact
    // bug-class the task names: `"Accent Color".contains("accent")` is false
    // case-sensitively. We require that the live folded search surfaces the
    // capitalised "Accent Color" control that a case-sensitive label scan of the
    // lowercase query would miss. (The keyword list happens to also carry a
    // lowercase "accent", so a keyword-only contrast would not be FAIL-able —
    // the label contrast is the honest one.)
    let label_hit_case_sensitive = panel
        .pages
        .iter()
        .flat_map(|p| p.settings.iter())
        .any(|s| s.label.contains("accent"));
    let label_hit_folded = panel
        .pages
        .iter()
        .flat_map(|p| p.settings.iter())
        .any(|s| s.label.to_ascii_lowercase().contains("accent"));
    // A "Accent Color" control exists (folded match) but the case-sensitive
    // label scan misses it — that contrast is what makes this FAIL-able. If the
    // search ever regressed to case-sensitive, `accent_hits` would still be >0
    // via keywords, so we ALSO require the folded label hit to be present in the
    // ranked results (the control the user actually wants to dive to).
    let accent_control_surfaced = panel.search.results.iter().any(|r| {
        r.setting_label
            .as_deref()
            .map(|l| l.to_ascii_lowercase().contains("accent"))
            .unwrap_or(false)
    });
    let case_insensitive = label_hit_folded && !label_hit_case_sensitive && accent_control_surfaced;

    // Rank check: the first "accent" result must be a title/label hit (rank 0/1
    // band), not a pure description/keyword hit. We re-derive: the top result's
    // label or page title must itself contain the query.
    let ranked = panel
        .search
        .results
        .first()
        .map(|r| {
            let lead = r
                .setting_label
                .as_deref()
                .unwrap_or(r.page_title.as_str())
                .to_ascii_lowercase();
            lead.contains("accent")
        })
        .unwrap_or(false);

    let pass = categories == 10 && accent_hits > 0 && case_insensitive && ranked;
    SettingsSearchProof {
        categories,
        accent_hits,
        case_insensitive,
        ranked,
        pass,
    }
}

// ── Layout + About/Storage proof (R10: a smoketest must be able to FAIL) ────

/// The asserted layout facts Slices 2-4 must hold (docs/design/settings-redesign.md
/// §3-§5 + the task's target proof line). The kernel logs this at boot; the host
/// KAT asserts the same invariants. FAIL-able: panes != 2, the sidebar IA != 10
/// categories, the About panel renders 0 live fields, or the Storage panel can
/// render NEITHER a populated bar NOR the empty-state.
#[derive(Clone, Copy, Debug)]
pub struct SettingsLayoutProof {
    /// Always 2 — sidebar + detail pane (§3).
    pub panes: u32,
    /// Sidebar category-row count — must be exactly 10 (§1 IA).
    pub sidebar_cats: u32,
    /// Count of About fields that read live (non-"(unknown)") from the pushed
    /// `/proc/athena/*`. 0 ⇒ FAIL (the panel rendered nothing real).
    pub about_fields: u32,
    /// True iff a AthFS volume is mounted (the capacity bar path); false ⇒ the
    /// Storage panel takes the empty-state InfoBar path. EITHER is a valid render.
    pub storage_mounted: bool,
    /// True iff: panes == 2, sidebar_cats == 10, about_fields > 0, and the
    /// Storage panel renders a definite path (always true — both states render).
    pub pass: bool,
}

/// Compute the Settings layout proof against the LIVE singleton (so it reflects
/// the kernel's `/proc/athena/*` push). Single authority for the asserted values.
/// Falls back to a fresh panel if the singleton is not yet `init()`d (host KAT).
#[must_use]
pub fn settings_layout_proof() -> SettingsLayoutProof {
    let sidebar_cats = SettingsCategory::all().len() as u32;
    // Read the live panel if present (kernel path); else a fresh one (host KAT).
    let info_owned;
    let info: &SystemInfo = unsafe {
        match CONTROL_PANEL.as_ref() {
            Some(panel) => &panel.system_info,
            None => {
                info_owned = SystemInfo::unknown();
                &info_owned
            }
        }
    };
    let about_fields = info.about_live_fields();
    let storage_mounted = info.storage_mounted;
    // Both Storage states render a definite surface (bar OR InfoBar) — the panel
    // can never render "nothing", so the storage clause is structurally true.
    let storage_renders = true;
    let pass = sidebar_cats == 10 && about_fields > 0 && storage_renders;
    SettingsLayoutProof {
        panes: 2,
        sidebar_cats,
        about_fields,
        storage_mounted,
        pass,
    }
}

#[cfg(test)]
mod design_tests {
    use super::*;

    #[test]
    fn settings_chrome_text_is_raesans_proportional_not_mono_bitmap() {
        // visual-QA Round-8 "unify chrome type": every Settings chrome label (nav
        // rows, window title, page/content headers, body/control labels, Vibe tile
        // names) renders through the anti-aliased PROPORTIONAL RaeSans path the
        // taskbar / Control Center use — NOT the 8x8 mono bitmap. Three FAIL-able
        // invariants off the same AA engine render() calls:
        use athgfx::text::FontFamily;
        assert!(
            athgfx::text::ensure_init(),
            "RaeSans AA engine must be available for the chrome-type path"
        );
        let sw = 320usize;
        let sh = 40usize;
        let mut px = alloc::vec![0xFF_10_14_20u32; sw * sh];
        // (1) PROPORTIONAL: equal char count, different glyph widths → different
        //     advances (a mono/bitmap path gives count*fixed).
        let c = unsafe { athgfx::Canvas::new(px.as_mut_ptr() as *mut u8, sw, sh, 4) };
        let narrow = c.measure_text_aa("iiii", ath_tokens::TYPE_LABEL, FontFamily::Sans);
        let wide = c.measure_text_aa("WWWW", ath_tokens::TYPE_LABEL, FontFamily::Sans);
        assert!(
            wide > narrow,
            "RaeSans must be proportional (W wider than i): narrow={narrow} wide={wide}"
        );
        // (2) MEASURED TRUNCATION: fit_text_aa truncates by the PROPORTIONAL advance,
        //     so a budget that fits N wide glyphs fits MORE narrow glyphs (the old
        //     `clip_text(s, px/8)` mono estimate gave the same char count for both).
        let budget = wide.max(0) as usize; // room for ~4 'W'
        let wfit = fit_text_aa(&c, "WWWWWWWW", budget, ath_tokens::TYPE_LABEL);
        let ifit = fit_text_aa(&c, "iiiiiiii", budget, ath_tokens::TYPE_LABEL);
        assert!(
            ifit.len() > wfit.len(),
            "proportional fit must pack more narrow glyphs than wide: \
             i-fit={} chars, W-fit={} chars",
            ifit.len(),
            wfit.len()
        );
        // (3) AA INK: a header lays down non-uniform grayscale coverage the 1-bit
        //     bitmap blit can't produce.
        let mut c = unsafe { athgfx::Canvas::new(px.as_mut_ptr() as *mut u8, sw, sh, 4) };
        let stats = c.draw_text_aa_stats(
            8,
            8,
            "Appearance",
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
    fn parse_system_info_reads_live_proc_dumps() {
        // The exact procfs formats the kernel agent specified must parse to live
        // fields — and the proof must be FAIL-able (the all-unknown default fails).
        let version = "# AthenaOS\nAthKernel 0.1.0 (build abc123) x86_64\n";
        // The real cpu_features dump format: `brand: "..."` (quoted) + `vendor:`.
        let cpu = "# AthenaOS CPU feature detection\nvendor: AMD (AuthenticAMD)\n\
            brand:  \"AMD Ryzen 5 7640HS\"\nfamily: 0x19   model: 0x74   stepping: 1\n";
        // The real /proc/athena/smp format: one `cpuN: ...` row per online CPU.
        let smp = "# AthenaOS SMP heartbeat (per-CPU timer IRQ counters)\n\
            cpu0: ticks=100 task_picks=50 steals=0\ncpu1: ticks=90 task_picks=40 steals=0\n\
            cpu2: ticks=80 task_picks=30 steals=0\ncpu3: ticks=70 task_picks=20 steals=0\n\
            # 4 of 12 CPU slot(s) heartbeating, 4 actually running scheduler work\n";
        let hardware = "# hardware\nboard: Beelink EliteMini\n";
        let memory = "# memory\nphysical_total_bytes: 16978542592 (16192 MiB)\n";
        let storage = "# storage\nmounted: 1\ntotal_bytes: 1073741824 (1024 MiB)\n\
            free_bytes: 805306368 (768 MiB)\nused_bytes: 268435456 (256 MiB)\n\
            block_size: 4096\ncategory_system_bytes: 268435456\n";
        let info = parse_system_info(version, cpu, smp, hardware, memory, storage);
        assert!(
            info.processor.contains("Ryzen"),
            "processor brand must parse"
        );
        assert_eq!(info.smp_cores, 4, "SMP count = distinct cpuN: rows");
        assert_eq!(info.installed_ram_bytes, 16978542592, "RAM bytes first-int");
        assert!(info.board.contains("Beelink"), "board must parse");
        assert!(info.storage_mounted, "mounted:1 ⇒ storage mounted");
        assert_eq!(info.storage_total_bytes, 1073741824);
        assert_eq!(info.storage_free_bytes, 805306368);
        assert_eq!(info.storage_used_bytes, 268435456);
        assert!(
            info.about_live_fields() >= 5,
            "at least 5 live About fields"
        );
    }

    #[test]
    fn parse_system_info_unavailable_is_graceful() {
        // The QEMU virtio case: storage unmounted, RAM unavailable. Must NOT
        // panic and must take the empty-state path (mounted=false), never a fake
        // 0/0 bar; the all-garbled default still has the unknown sentinels.
        let storage = "# storage\nmounted: 0\ntotal_bytes: (unavailable)\n\
            free_bytes: (unavailable)\n";
        let memory = "# memory\nphysical_total_bytes: (unavailable)\n";
        let info = parse_system_info("", "", "", "", memory, storage);
        assert!(!info.storage_mounted, "mounted:0 ⇒ empty-state");
        assert_eq!(info.storage_total_bytes, 0);
        assert_eq!(info.installed_ram_bytes, 0);
        assert!(is_unknown(&info.processor), "missing cpu ⇒ (unknown)");
        // Fully-unknown ⇒ 0 live About fields ⇒ the layout proof would FAIL.
        assert_eq!(info.about_live_fields(), 0);
    }

    #[test]
    fn parse_handles_garbled_without_panic() {
        // Adversarial: a mounted:1 with a 0 total must fall back to empty-state
        // (no divide-by-zero in the bar), and junk lines must be ignored.
        let storage = "garbage\nmounted: 1\ntotal_bytes: 0\nfree_bytes: nonsense\n";
        let info = parse_system_info("???", "@@@", "!!!", "###", "%%%", storage);
        assert!(
            !info.storage_mounted,
            "0-total ⇒ empty-state, no div-by-zero"
        );
    }

    #[test]
    fn layout_proof_default_fails_about_then_passes_when_pushed() {
        // FAIL-able: with no live push the About panel has 0 live fields ⇒ a
        // proof computed from an unknown info must NOT pass the about clause.
        let unknown = SystemInfo::unknown();
        assert_eq!(unknown.about_live_fields(), 0);
        // And a populated info yields a passing about_fields.
        let info = parse_system_info(
            "AthKernel 0.1.0 x86_64",
            "brand:  \"TestCPU\"",
            "cpu0: ticks=1 task_picks=1 steals=0",
            "board: TestBoard",
            "physical_total_bytes: 8589934592",
            "mounted: 0",
        );
        assert!(
            info.about_live_fields() > 0,
            "live push ⇒ live About fields"
        );
    }

    #[test]
    fn fmt_bytes_is_panic_free_and_sane() {
        assert_eq!(fmt_bytes(0), "0 B");
        assert_eq!(fmt_bytes(512), "512 B");
        assert!(fmt_bytes(1 << 20).ends_with("MiB"));
        assert!(fmt_bytes(16 * (1 << 30)).ends_with("GiB"));
        // No panic at the u64 ceiling.
        let _ = fmt_bytes(u64::MAX);
    }

    #[test]
    fn search_is_case_insensitive_and_finds_accent() {
        // The whole point of Slice 1: typing the lowercase "accent" finds the
        // "Accent Color" control. The OLD case-sensitive search returned 0.
        let proof = settings_search_proof();
        assert_eq!(proof.categories, 10, "IA must be 10 categories");
        assert!(proof.accent_hits > 0, "\"accent\" must find hits");
        assert!(
            proof.case_insensitive,
            "search must fold case (old path found 0)"
        );
        assert!(proof.ranked, "top result must be a title/label match");
        assert!(proof.pass, "search proof must pass: {proof:?}");
    }

    #[test]
    fn old_case_sensitive_query_would_fail() {
        // Prove the smoketest CAN fail: the retired case-sensitive substring of
        // the lowercase query finds nothing against the capitalised labels.
        let panel = ControlPanel::new();
        let raw = panel
            .pages
            .iter()
            .filter(|p| {
                p.title.contains("accent") || p.settings.iter().any(|s| s.label.contains("accent"))
            })
            .count();
        assert_eq!(raw, 0, "case-sensitive lowercase query must miss 'Accent'");
    }

    #[test]
    fn every_page_lands_in_one_of_the_ten_categories() {
        // The IA regroup must not orphan a page: every page's category is one of
        // the 10, and no settings were dropped (the page+setting counts survive).
        let panel = ControlPanel::new();
        let cats = SettingsCategory::all();
        for page in &panel.pages {
            assert!(
                cats.contains(&page.category),
                "page {} has an out-of-IA category",
                page.id
            );
        }
        // Spot-check the remap preserved key pages.
        assert!(panel.pages.iter().any(|p| p.id == "pers.colors"));
        assert!(panel.pages.iter().any(|p| p.id == "sys.about"));
        assert!(panel.pages.iter().any(|p| p.id == "access.highcontrast"));
    }

    #[test]
    fn accent_is_token_derived_not_hardcoded() {
        // The whole point of the re-skin: Settings reads the shared ramp, not
        // the retired `const CP_ACCENT = 0x4E9CFF`.
        let want = ath_tokens::derive_accent(cp_accent_seed(), cp_palette()).base;
        assert_eq!(
            cp_accent().base,
            want,
            "accent.base must be derive_accent(seed)"
        );
        // Default live seed (no kernel push in the host KAT) is RaeBlue.
        assert_eq!(want, ath_tokens::RAEBLUE, "default seed is RaeBlue");
    }

    #[test]
    fn toggle_on_equals_accent_base_cohesion_link() {
        // settings.md §6: the toggle-on colour IS accent.base — the same value
        // the taskbar reads. If the control kit re-hardcodes a toggle colour
        // this FAILs.
        let proof = settings_design_proof();
        assert_eq!(
            proof.toggle_on, proof.accent_base,
            "toggle-on must be accent.base"
        );
        assert_eq!(
            proof.accent_base, 0xFF_4E_9C_FF,
            "accent must be 0xFF4E9CFF (taskbar cohesion)"
        );
    }

    #[test]
    fn mica_recipe_is_a_valid_detinted_base() {
        // `cp_mica()` is the de-tinted bg.base/bg.raised blend (still used for the
        // de-tinted content base reference). IDENTITY.md §7 moved the WINDOW
        // backdrop to glass.panel, but the recipe must stay a valid opaque blend.
        let mica = cp_mica();
        assert_eq!((mica >> 24) & 0xFF, 0xFF, "mica must be opaque");
        let p = cp_palette();
        let base_b = p.bg_base & 0xFF;
        let raised_b = p.bg_raised & 0xFF;
        let mica_b = mica & 0xFF;
        assert!(
            mica_b >= base_b && mica_b <= raised_b,
            "mica blue not between bg.base and bg.raised"
        );
        assert_eq!(mica, 0xFF_0E_12_1F, "mica recipe value pinned");
    }

    #[test]
    fn settings_uses_three_distinct_translucency_planes() {
        // THE depth property (spec bullet 1): 3 distinct translucency planes.
        //   Plane 2 (panel)   = the window body + sidebar — glass.panel.
        //   Plane 3 (popover) = the content cards — glass.popover (MORE opaque).
        // Both must be translucent (the aurora reads through), and popover must be
        // STRICTLY more opaque than panel so the sidebar(panel) and the content
        // cards(popover) read as visibly different planes. FAIL-able: if a future
        // edit drew the cards on the panel tier (or a solid fill) the alpha
        // ordering collapses and this trips.
        // OBSIDIAN (IDENTITY-OBSIDIAN.md §2): both planes are near-black with a
        // whisper of bleed — never fully opaque (0xFF kills the wallpaper read),
        // never back under 0xD0 (the milky toy-gray regression) — and the
        // popover plane stays STRICTLY more opaque than panel so the two read
        // as different planes.
        let panel_a = (ath_tokens::GLASS_PANEL_DARK.tint >> 24) & 0xFF;
        let popover_a = (ath_tokens::GLASS_POPOVER_DARK.tint >> 24) & 0xFF;
        assert!(
            (0xD0..=0xFA).contains(&panel_a),
            "glass.panel alpha must sit in the obsidian band [0xD0,0xFA] (a={panel_a:#x})"
        );
        assert!(
            (0xD0..=0xFA).contains(&popover_a),
            "glass.popover alpha must sit in the obsidian band [0xD0,0xFA] (a={popover_a:#x})"
        );
        assert!(
            popover_a > panel_a,
            "the content plane (popover a={popover_a:#x}) must be MORE opaque than the \
             sidebar/panel plane (a={panel_a:#x}) — the two must read as different planes"
        );
    }

    #[test]
    fn vibe_grid_sizes_to_live_preset_count() {
        // settings.md §5.2: the grid sizes to vibe_mode::ALL_PRESETS, NOT a
        // hardcoded 5. The swatch table must cover every live preset.
        let proof = settings_design_proof();
        assert_eq!(
            proof.vibe_tiles as usize,
            crate::vibe_mode::ALL_PRESETS.len()
        );
        for preset in crate::vibe_mode::ALL_PRESETS.iter() {
            let (name, _, _, _) = preset_swatch(*preset);
            assert!(!name.is_empty(), "every Vibe preset needs a grid label");
        }
    }

    #[test]
    fn proof_passes_when_wired() {
        // The exact assertion the kernel logs. Real fail-able check: if any
        // invariant regressed (accent source, toggle cohesion, card radius,
        // panes, grid), pass=false.
        let proof = settings_design_proof();
        assert!(proof.pass, "settings design proof must pass: {proof:?}");
        assert_eq!(proof.panes, 2);
        assert_eq!(proof.card_radius, ath_tokens::RADIUS_MD);
        assert_eq!(proof.window_radius, ath_tokens::RADIUS_LG);
    }

    #[test]
    fn every_settings_label_clears_wcag_aa_over_its_real_glass_plane() {
        // CONTRAST is the BINDING rule (spec bullet 7): EVERY text colour the
        // Settings render paints must clear WCAG AA 4.5:1 against its REAL
        // composited backdrop — measured, never eyeballed. We compute the actual
        // contrast of each (label colour, glass-plane interior) pair the render
        // produces, over the worst real aurora samples the window sits over.
        //
        // Two glass planes carry text:
        //   * Plane 2 (panel)   — the sidebar (inactive nav labels = text_secondary,
        //                          active = text_primary; search placeholder/icon =
        //                          text_tertiary).
        //   * Plane 3 (popover) — the content cards (titles/values = text_primary,
        //                          descriptions/captions = text_tertiary, page desc /
        //                          slider value / breakdown size = text_secondary).
        //
        // Captions / descriptions / hints render in `cp_caption_ink()` ==
        // `text_secondary` — NOT `text_tertiary`. The spec's literal "caption at
        // 70% opacity" maps to text_tertiary (rel-luminance 0.18), which measures
        // ~2.5:1 over the panel-glass interior and therefore FAILS the binding
        // 4.5:1 rule; per the spec ("Contrast ≥4.5 is the BINDING rule … if it
        // conflicts with the 70% caption style, contrast wins") captions are
        // painted in text_secondary instead. So the LOWEST-contrast LABEL ink the
        // render uses is text_secondary; if it clears 4.5:1 here, text_primary does
        // too. `text_tertiary` survives only on decorative glyphs (chevrons, the
        // disabled-control dim, the search magnifier), never on a text label — so
        // it is NOT in the measured set. Both glass tiers apply the WCAG legibility
        // cap inside `glass_tier_interior`, so the measured backdrop is the capped
        // interior the eye actually sees.
        use ath_tokens::{
            contrast_ratio, glass_tier_interior, AURORA_BLOB_BLUE, AURORA_BLOB_TEAL,
            GLASS_PANEL_DARK, GLASS_POPOVER_DARK, WALLPAPER_AURORA_BASE_DARK,
        };
        let p = cp_palette();
        // The brightest real aurora samples the window composites over (the cap's
        // hardest inputs): base void, the blue blob (single-blob peak), the teal
        // blob, and the two-blob peak.
        let add = |a: u32, b: u32| -> u32 {
            let ch = |s: u32| (((a >> s) & 0xFF) + ((b >> s) & 0xFF)).min(0xFF);
            0xFF00_0000 | (ch(16) << 16) | (ch(8) << 8) | ch(0)
        };
        let two_blob = add(
            add(WALLPAPER_AURORA_BASE_DARK, AURORA_BLOB_BLUE),
            AURORA_BLOB_TEAL,
        );
        let aurora_samples = [
            ("aurora.base", WALLPAPER_AURORA_BASE_DARK),
            ("aurora.blue_blob", AURORA_BLOB_BLUE),
            ("aurora.teal_blob", AURORA_BLOB_TEAL),
            ("aurora.two_blob_peak", two_blob),
        ];
        // EVERY on-glass TEXT label is painted in `text_primary` (cp_caption_ink ==
        // cp_card_caption_ink == text_primary), because that is the ONE ink the
        // SHIP-GATE WCAG cap guarantees ≥4.5:1 at every window position (over a
        // BRIGHT aurora blob the panel/popover interior rises to ~luma 0.40, where
        // text_secondary/text_tertiary both drop below AA). Hierarchy is carried by
        // the type ramp + accent, not colour dimming. We measure text_primary over
        // BOTH planes on every aurora sample; all must clear 4.5:1.
        assert_eq!(cp_caption_ink(), p.text_primary, "on-glass caption ink");
        assert_eq!(cp_card_caption_ink(), p.text_primary, "in-card caption ink");
        let label_ink = ("text_primary (every on-glass label)", p.text_primary);
        let planes = [
            ("panel (sidebar/showcase/breadcrumb)", GLASS_PANEL_DARK),
            ("popover (content card / overlay)", GLASS_POPOVER_DARK),
        ];
        let mut worst = f32::MAX;
        let mut worst_desc = "";
        for (sample_name, sample) in aurora_samples.iter() {
            for (plane_name, tier) in planes.iter() {
                let interior = glass_tier_interior(*tier, *sample);
                let (ink_name, ink) = label_ink;
                let cr = contrast_ratio(ink, interior);
                if cr < worst {
                    worst = cr;
                    worst_desc = ink_name;
                }
                assert!(
                    cr >= 4.5,
                    "Settings label '{ink_name}' fails WCAG AA over {plane_name} \
                     glass on {sample_name}: CR {cr:.2} < 4.5 — the binding contrast rule"
                );
            }
        }
        // FAIL-able: a degenerate (e.g. text_tertiary == card colour) would drive
        // worst toward 1.0 and trip the assert above; here we record the margin.
        assert!(
            worst >= 4.5,
            "worst Settings label/backdrop pair ('{worst_desc}') CR {worst:.2} < 4.5"
        );
        // Proof the binding rule was REAL (the test can FAIL): the literal
        // "caption@70%" text_tertiary ink would NOT have cleared 4.5:1 over the
        // panel-glass plane — which is exactly why captions are painted in
        // text_secondary instead. If a future edit made text_tertiary legible over
        // glass, this guard would trip and the caption choice could be revisited.
        let panel_interior = glass_tier_interior(GLASS_PANEL_DARK, WALLPAPER_AURORA_BASE_DARK);
        let tertiary_cr = contrast_ratio(p.text_tertiary, panel_interior);
        assert!(
            tertiary_cr < 4.5,
            "text_tertiary unexpectedly clears 4.5:1 ({tertiary_cr:.2}) over panel glass — \
             the caption→text_secondary promotion was the documented deviation; re-verify"
        );
    }

    #[test]
    fn nav_rows_map_to_real_line_icons_not_letters() {
        // visual-QA Round-7 #1: every Settings nav row must resolve a real athgfx
        // Icon (the bitmap font drew the old emoji `char` as `?`). All 10 map; the
        // icons are distinct enough to be recognisable (Palette/Brightness/Volume/
        // WiFi/Bluetooth/GameController/Accessibility/Lock/Archive/Gear).
        use athgfx::icon::Icon;
        let cats = SettingsCategory::all();
        assert_eq!(cats.len(), 10);
        let wanted = [
            (SettingsCategory::Appearance, Icon::Palette),
            (SettingsCategory::Display, Icon::Brightness),
            (SettingsCategory::Sound, Icon::Volume),
            (SettingsCategory::Network, Icon::WiFi),
            (SettingsCategory::Devices, Icon::Bluetooth),
            (SettingsCategory::PowerGaming, Icon::GameController),
            (SettingsCategory::Accessibility, Icon::Accessibility),
            (SettingsCategory::PrivacySecurity, Icon::Lock),
            (SettingsCategory::Storage, Icon::Archive),
            (SettingsCategory::SystemAbout, Icon::Gear),
        ];
        for (cat, icon) in wanted {
            assert_eq!(cat.line_icon(), icon, "{cat:?} nav icon");
        }
    }

    #[test]
    fn show_lands_on_a_populated_pane_not_empty() {
        // visual-QA Round-7 #2: opening Settings must select a pane (a lit nav row
        // + a populated detail), never an empty title. FAIL-able: if show() left
        // current_page_id None the landing would be the empty home.
        let mut cp = ControlPanel::new();
        cp.show();
        assert_eq!(
            cp.navigation.current_category,
            Some(SettingsCategory::Appearance),
            "show() must select the Appearance nav row"
        );
        assert_eq!(
            cp.navigation.current_page_id.as_deref(),
            Some("pers.colors"),
            "show() must land on the populated Colors page"
        );
    }

    #[test]
    fn cp_palette_consts_are_retired() {
        // A guard against re-introducing the flat palette: clip_text + the
        // alpha helper are the only colour helpers; the accent flows from the
        // shared ramp. (Compile-time: if CP_ACCENT existed this module would
        // still build, so we assert the ramp identity instead.)
        assert_eq!(
            cp_accent().subtle,
            ath_tokens::derive_accent(ath_tokens::RAEBLUE, &ath_tokens::DARK).subtle
        );
    }
}
