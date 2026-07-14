//! Runtime Permission Prompts — modal dialog for capability grant requests.
//!
//! When an app requests a new capability, a permission prompt appears:
//! - Shows app identity (icon + name) and a human-readable description
//! - "Allow Once" / "Allow Always" / "Deny" buttons
//! - Rate limiting (max 3 requests per minute per app)
//! - Persistent permission store for user decisions
//! - Integration with RaeShield's SandboxPolicy

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

// ── Permission Types ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u16)]
pub enum PermissionKind {
    Filesystem = 0,
    Network = 1,
    Camera = 2,
    Microphone = 3,
    Location = 4,
    Notifications = 5,
    Clipboard = 6,
    Gpu = 7,
    Bluetooth = 8,
    Usb = 9,
    SystemSettings = 10,
    ProcessSpawn = 11,
    ScreenCapture = 12,
    Audio = 13,
}

impl PermissionKind {
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Filesystem => "File System Access",
            Self::Network => "Network Access",
            Self::Camera => "Camera",
            Self::Microphone => "Microphone",
            Self::Location => "Location",
            Self::Notifications => "Notifications",
            Self::Clipboard => "Clipboard",
            Self::Gpu => "GPU Acceleration",
            Self::Bluetooth => "Bluetooth",
            Self::Usb => "USB Devices",
            Self::SystemSettings => "System Settings",
            Self::ProcessSpawn => "Run Programs",
            Self::ScreenCapture => "Screen Capture",
            Self::Audio => "Audio Playback",
        }
    }

    pub fn description(&self) -> &'static str {
        match self {
            Self::Filesystem => "Read and write files on your device",
            Self::Network => "Connect to the internet and local networks",
            Self::Camera => "Access your camera to take photos or video",
            Self::Microphone => "Record audio from your microphone",
            Self::Location => "Determine your physical location",
            Self::Notifications => "Show notifications on your desktop",
            Self::Clipboard => "Read and write your clipboard contents",
            Self::Gpu => "Use hardware-accelerated graphics",
            Self::Bluetooth => "Discover and connect to Bluetooth devices",
            Self::Usb => "Access connected USB devices",
            Self::SystemSettings => "Modify system configuration",
            Self::ProcessSpawn => "Launch other programs on your behalf",
            Self::ScreenCapture => "Capture your screen contents",
            Self::Audio => "Play audio through your speakers",
        }
    }

    pub fn icon_glyph(&self) -> char {
        match self {
            Self::Filesystem => 'F',
            Self::Network => 'N',
            Self::Camera => 'C',
            Self::Microphone => 'M',
            Self::Location => 'L',
            Self::Notifications => '!',
            Self::Clipboard => 'P',
            Self::Gpu => 'G',
            Self::Bluetooth => 'B',
            Self::Usb => 'U',
            Self::SystemSettings => 'S',
            Self::ProcessSpawn => 'R',
            Self::ScreenCapture => 'D',
            Self::Audio => 'A',
        }
    }

    pub fn is_sensitive(&self) -> bool {
        matches!(
            self,
            Self::Camera
                | Self::Microphone
                | Self::Location
                | Self::Filesystem
                | Self::ScreenCapture
        )
    }
}

// ── Permission Scope ──────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionScope {
    /// Full access (no restrictions within the permission kind).
    Full,
    /// Filesystem: limited to a specific path prefix.
    Path(String),
    /// Network: limited to specific host/port.
    HostPort { host: String, port: u16 },
    /// Network: limited to host only (any port).
    Host(String),
}

impl PermissionScope {
    pub fn display(&self) -> String {
        match self {
            Self::Full => String::from("(unrestricted)"),
            Self::Path(p) => p.clone(),
            Self::HostPort { host, port } => {
                let mut s = host.clone();
                s.push(':');
                // Manual port to string without format!
                let mut buf = [0u8; 5];
                let port_str = u16_to_str(*port, &mut buf);
                s.push_str(port_str);
                s
            }
            Self::Host(h) => h.clone(),
        }
    }
}

fn u16_to_str(mut n: u16, buf: &mut [u8; 5]) -> &str {
    if n == 0 {
        buf[0] = b'0';
        // Safety: '0' is valid UTF-8.
        return unsafe { core::str::from_utf8_unchecked(&buf[..1]) };
    }
    let mut i = 5;
    while n > 0 {
        i -= 1;
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
    }
    unsafe { core::str::from_utf8_unchecked(&buf[i..]) }
}

// ── Permission Request ────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct PermissionRequest {
    pub id: u64,
    pub app_id: u64,
    pub app_name: String,
    pub kind: PermissionKind,
    pub scope: PermissionScope,
    pub timestamp: u64,
    pub grouped_with: Vec<PermissionKind>,
}

impl PermissionRequest {
    pub fn new(
        id: u64,
        app_id: u64,
        app_name: &str,
        kind: PermissionKind,
        scope: PermissionScope,
        ts: u64,
    ) -> Self {
        Self {
            id,
            app_id,
            app_name: String::from(app_name),
            kind,
            scope,
            timestamp: ts,
            grouped_with: Vec::new(),
        }
    }

    pub fn add_grouped(&mut self, kind: PermissionKind) {
        if !self.grouped_with.contains(&kind) {
            self.grouped_with.push(kind);
        }
    }

    pub fn is_grouped(&self) -> bool {
        !self.grouped_with.is_empty()
    }

    pub fn total_permissions(&self) -> usize {
        1 + self.grouped_with.len()
    }
}

// ── User Decision ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionDecision {
    AllowOnce,
    AllowAlways,
    Deny,
    DenyAlways,
}

impl PermissionDecision {
    pub fn is_allow(&self) -> bool {
        matches!(self, Self::AllowOnce | Self::AllowAlways)
    }

    pub fn is_persistent(&self) -> bool {
        matches!(self, Self::AllowAlways | Self::DenyAlways)
    }
}

// ── Permission Store ──────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct StoredPermission {
    pub app_id: u64,
    pub kind: PermissionKind,
    pub scope: PermissionScope,
    pub decision: PermissionDecision,
    pub granted_at: u64,
    pub last_used: u64,
    pub use_count: u64,
}

/// Persistent storage of user permission decisions (per-app, per-capability).
pub struct PermissionStore {
    entries: Vec<StoredPermission>,
    max_entries: usize,
}

impl PermissionStore {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            max_entries: 4096,
        }
    }

    pub fn store(&mut self, entry: StoredPermission) {
        // Replace existing entry for same app+kind+scope if present.
        if let Some(existing) = self
            .entries
            .iter_mut()
            .find(|e| e.app_id == entry.app_id && e.kind == entry.kind && e.scope == entry.scope)
        {
            *existing = entry;
            return;
        }

        if self.entries.len() >= self.max_entries {
            // Evict least-recently-used.
            if let Some(idx) = self
                .entries
                .iter()
                .enumerate()
                .min_by_key(|(_, e)| e.last_used)
                .map(|(i, _)| i)
            {
                self.entries.remove(idx);
            }
        }
        self.entries.push(entry);
    }

    pub fn lookup(&self, app_id: u64, kind: PermissionKind) -> Option<&StoredPermission> {
        self.entries
            .iter()
            .find(|e| e.app_id == app_id && e.kind == kind && e.decision.is_persistent())
    }

    pub fn lookup_with_scope(
        &self,
        app_id: u64,
        kind: PermissionKind,
        scope: &PermissionScope,
    ) -> Option<&StoredPermission> {
        self.entries.iter().find(|e| {
            e.app_id == app_id && e.kind == kind && e.scope == *scope && e.decision.is_persistent()
        })
    }

    pub fn permissions_for_app(&self, app_id: u64) -> Vec<&StoredPermission> {
        self.entries.iter().filter(|e| e.app_id == app_id).collect()
    }

    pub fn revoke(&mut self, app_id: u64, kind: PermissionKind) -> bool {
        let before = self.entries.len();
        self.entries
            .retain(|e| !(e.app_id == app_id && e.kind == kind));
        self.entries.len() < before
    }

    pub fn revoke_all_for_app(&mut self, app_id: u64) -> usize {
        let before = self.entries.len();
        self.entries.retain(|e| e.app_id != app_id);
        before - self.entries.len()
    }

    pub fn record_use(&mut self, app_id: u64, kind: PermissionKind, timestamp: u64) {
        if let Some(entry) = self
            .entries
            .iter_mut()
            .find(|e| e.app_id == app_id && e.kind == kind)
        {
            entry.last_used = timestamp;
            entry.use_count += 1;
        }
    }

    pub fn total_grants(&self) -> usize {
        self.entries
            .iter()
            .filter(|e| e.decision.is_allow())
            .count()
    }

    pub fn total_denials(&self) -> usize {
        self.entries
            .iter()
            .filter(|e| !e.decision.is_allow())
            .count()
    }
}

// ── Rate Limiter ──────────────────────────────────────────────────────────

const MAX_REQUESTS_PER_WINDOW: usize = 3;
const RATE_WINDOW_MS: u64 = 60_000;

struct RateEntry {
    timestamps: Vec<u64>,
}

impl RateEntry {
    fn new() -> Self {
        Self {
            timestamps: Vec::new(),
        }
    }

    fn record(&mut self, ts: u64) {
        self.timestamps.push(ts);
        // Keep only timestamps within the window.
        let cutoff = ts.saturating_sub(RATE_WINDOW_MS);
        self.timestamps.retain(|&t| t >= cutoff);
    }

    fn is_rate_limited(&self, now: u64) -> bool {
        let cutoff = now.saturating_sub(RATE_WINDOW_MS);
        let recent = self.timestamps.iter().filter(|&&t| t >= cutoff).count();
        recent >= MAX_REQUESTS_PER_WINDOW
    }
}

struct RateLimiter {
    entries: BTreeMap<u64, RateEntry>,
}

impl RateLimiter {
    fn new() -> Self {
        Self {
            entries: BTreeMap::new(),
        }
    }

    fn check_and_record(&mut self, app_id: u64, now: u64) -> bool {
        let entry = self.entries.entry(app_id).or_insert_with(RateEntry::new);
        if entry.is_rate_limited(now) {
            return false;
        }
        entry.record(now);
        true
    }

    #[allow(dead_code)]
    fn is_limited(&self, app_id: u64, now: u64) -> bool {
        self.entries
            .get(&app_id)
            .map_or(false, |e| e.is_rate_limited(now))
    }
}

// ── Prompt State ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptState {
    Pending,
    Shown,
    Answered,
    RateLimited,
    AutoGranted,
    AutoDenied,
}

#[derive(Debug, Clone)]
pub struct PromptResult {
    pub request_id: u64,
    pub decision: PermissionDecision,
    pub remember: bool,
}

// ── Permission Prompt UI ──────────────────────────────────────────────────

const PROMPT_WIDTH: usize = 400;
const PROMPT_HEIGHT: usize = 260;
const PROMPT_BG: u32 = 0xFF_18_1C_2E;
const PROMPT_BORDER: u32 = 0xFF_4E_9C_FF;
const PROMPT_TITLE_FG: u32 = 0xFF_FF_FF_FF;
const PROMPT_BODY_FG: u32 = 0xFF_C0_C0_D0;
const BUTTON_ALLOW_BG: u32 = 0xFF_22_8B_22;
const BUTTON_DENY_BG: u32 = 0xFF_CC_33_33;
const BUTTON_ONCE_BG: u32 = 0xFF_33_66_99;
const BUTTON_FG: u32 = 0xFF_FF_FF_FF;
const CHECKBOX_BG: u32 = 0xFF_28_2C_44;
const CHECKBOX_CHECK: u32 = 0xFF_4E_9C_FF;

pub struct PermissionPrompt {
    pub request: PermissionRequest,
    pub state: PromptState,
    pub remember_checked: bool,
    pub selected_button: u8,
    pub x: usize,
    pub y: usize,
}

impl PermissionPrompt {
    pub fn new(request: PermissionRequest, screen_w: usize, screen_h: usize) -> Self {
        let x = (screen_w.saturating_sub(PROMPT_WIDTH)) / 2;
        let y = (screen_h.saturating_sub(PROMPT_HEIGHT)) / 2;
        Self {
            request,
            state: PromptState::Shown,
            remember_checked: false,
            selected_button: 0,
            x,
            y,
        }
    }

    pub fn render(&self, fb: &mut [u32], stride: usize) {
        // Background
        fill_rect(
            fb,
            stride,
            self.x,
            self.y,
            PROMPT_WIDTH,
            PROMPT_HEIGHT,
            PROMPT_BG,
        );
        // Border
        draw_border(
            fb,
            stride,
            self.x,
            self.y,
            PROMPT_WIDTH,
            PROMPT_HEIGHT,
            PROMPT_BORDER,
        );

        // Title bar
        let title_y = self.y + 12;
        draw_text_simple(
            fb,
            stride,
            self.x + 16,
            title_y,
            "Permission Request",
            PROMPT_TITLE_FG,
        );

        // App name and permission
        let app_y = self.y + 40;
        draw_text_simple(
            fb,
            stride,
            self.x + 16,
            app_y,
            &self.request.app_name,
            PROMPT_BODY_FG,
        );

        let desc_y = self.y + 64;
        draw_text_simple(
            fb,
            stride,
            self.x + 16,
            desc_y,
            self.request.kind.display_name(),
            PROMPT_TITLE_FG,
        );

        let detail_y = self.y + 88;
        draw_text_simple(
            fb,
            stride,
            self.x + 16,
            detail_y,
            self.request.kind.description(),
            PROMPT_BODY_FG,
        );

        // Scope line
        let scope_y = self.y + 112;
        let scope_display = self.request.scope.display();
        draw_text_simple(
            fb,
            stride,
            self.x + 16,
            scope_y,
            &scope_display,
            PROMPT_BODY_FG,
        );

        // Grouped permissions (if any)
        if self.request.is_grouped() {
            let group_y = self.y + 136;
            draw_text_simple(
                fb,
                stride,
                self.x + 16,
                group_y,
                "Also requesting:",
                PROMPT_BODY_FG,
            );
            for (i, kind) in self.request.grouped_with.iter().enumerate() {
                let gy = group_y + 16 + i * 14;
                if gy + 14 < self.y + PROMPT_HEIGHT - 60 {
                    draw_text_simple(
                        fb,
                        stride,
                        self.x + 32,
                        gy,
                        kind.display_name(),
                        PROMPT_BODY_FG,
                    );
                }
            }
        }

        // "Remember" checkbox
        let checkbox_y = self.y + PROMPT_HEIGHT - 56;
        let cb_x = self.x + 16;
        fill_rect(fb, stride, cb_x, checkbox_y, 12, 12, CHECKBOX_BG);
        if self.remember_checked {
            fill_rect(fb, stride, cb_x + 2, checkbox_y + 2, 8, 8, CHECKBOX_CHECK);
        }
        draw_text_simple(
            fb,
            stride,
            cb_x + 18,
            checkbox_y + 2,
            "Remember my choice",
            PROMPT_BODY_FG,
        );

        // Buttons
        let btn_y = self.y + PROMPT_HEIGHT - 36;
        let btn_w = 100;
        let btn_h = 24;

        // Allow Always
        let b0_x = self.x + 16;
        let b0_bg = if self.selected_button == 0 {
            BUTTON_ALLOW_BG
        } else {
            darken(BUTTON_ALLOW_BG)
        };
        fill_rect(fb, stride, b0_x, btn_y, btn_w, btn_h, b0_bg);
        draw_text_simple(fb, stride, b0_x + 8, btn_y + 6, "Allow Always", BUTTON_FG);

        // Allow Once
        let b1_x = self.x + 130;
        let b1_bg = if self.selected_button == 1 {
            BUTTON_ONCE_BG
        } else {
            darken(BUTTON_ONCE_BG)
        };
        fill_rect(fb, stride, b1_x, btn_y, btn_w, btn_h, b1_bg);
        draw_text_simple(fb, stride, b1_x + 12, btn_y + 6, "Allow Once", BUTTON_FG);

        // Deny
        let b2_x = self.x + 260;
        let b2_bg = if self.selected_button == 2 {
            BUTTON_DENY_BG
        } else {
            darken(BUTTON_DENY_BG)
        };
        fill_rect(fb, stride, b2_x, btn_y, btn_w, btn_h, b2_bg);
        draw_text_simple(fb, stride, b2_x + 28, btn_y + 6, "Deny", BUTTON_FG);
    }

    pub fn handle_click(&mut self, x: usize, y: usize) -> Option<PromptResult> {
        // Checkbox hit test
        let checkbox_y = self.y + PROMPT_HEIGHT - 56;
        let cb_x = self.x + 16;
        if x >= cb_x && x < cb_x + 150 && y >= checkbox_y && y < checkbox_y + 14 {
            self.remember_checked = !self.remember_checked;
            return None;
        }

        // Button hit tests
        let btn_y = self.y + PROMPT_HEIGHT - 36;
        let btn_h = 24;
        let btn_w = 100;

        if y < btn_y || y >= btn_y + btn_h {
            return None;
        }

        let decision = if x >= self.x + 16 && x < self.x + 16 + btn_w {
            self.selected_button = 0;
            PermissionDecision::AllowAlways
        } else if x >= self.x + 130 && x < self.x + 130 + btn_w {
            self.selected_button = 1;
            PermissionDecision::AllowOnce
        } else if x >= self.x + 260 && x < self.x + 260 + btn_w {
            self.selected_button = 2;
            PermissionDecision::Deny
        } else {
            return None;
        };

        self.state = PromptState::Answered;
        Some(PromptResult {
            request_id: self.request.id,
            decision,
            remember: self.remember_checked,
        })
    }

    pub fn contains_point(&self, x: usize, y: usize) -> bool {
        x >= self.x && x < self.x + PROMPT_WIDTH && y >= self.y && y < self.y + PROMPT_HEIGHT
    }
}

// ── First-Run Wizard ──────────────────────────────────────────────────────

pub struct FirstRunWizard {
    pub app_id: u64,
    pub app_name: String,
    pub requested_permissions: Vec<(PermissionKind, PermissionScope)>,
    pub decisions: Vec<Option<PermissionDecision>>,
    pub current_index: usize,
    pub completed: bool,
}

impl FirstRunWizard {
    pub fn new(
        app_id: u64,
        app_name: &str,
        permissions: Vec<(PermissionKind, PermissionScope)>,
    ) -> Self {
        let count = permissions.len();
        Self {
            app_id,
            app_name: String::from(app_name),
            requested_permissions: permissions,
            decisions: (0..count).map(|_| None).collect(),
            current_index: 0,
            completed: false,
        }
    }

    pub fn current_permission(&self) -> Option<&(PermissionKind, PermissionScope)> {
        self.requested_permissions.get(self.current_index)
    }

    pub fn decide_current(&mut self, decision: PermissionDecision) -> bool {
        if self.current_index < self.decisions.len() {
            self.decisions[self.current_index] = Some(decision);
            self.current_index += 1;
            if self.current_index >= self.requested_permissions.len() {
                self.completed = true;
            }
            true
        } else {
            false
        }
    }

    pub fn is_complete(&self) -> bool {
        self.completed
    }

    pub fn results(&self) -> Vec<(PermissionKind, PermissionDecision)> {
        self.requested_permissions
            .iter()
            .zip(self.decisions.iter())
            .filter_map(|((kind, _), dec)| dec.map(|d| (*kind, d)))
            .collect()
    }

    pub fn remaining_count(&self) -> usize {
        self.requested_permissions
            .len()
            .saturating_sub(self.current_index)
    }
}

// ── Permission Manager ────────────────────────────────────────────────────

pub struct PermissionManager {
    store: PermissionStore,
    rate_limiter: RateLimiter,
    pending_prompts: Vec<PermissionRequest>,
    active_prompt: Option<PermissionPrompt>,
    active_wizard: Option<FirstRunWizard>,
    next_request_id: u64,
    screen_width: usize,
    screen_height: usize,
    timestamp: u64,
}

impl PermissionManager {
    pub fn new(screen_width: usize, screen_height: usize) -> Self {
        Self {
            store: PermissionStore::new(),
            rate_limiter: RateLimiter::new(),
            pending_prompts: Vec::new(),
            active_prompt: None,
            active_wizard: None,
            next_request_id: 1,
            screen_width,
            screen_height,
            timestamp: 0,
        }
    }

    pub fn tick(&mut self, ts: u64) {
        self.timestamp = ts;
    }

    /// Request a permission for an app. Returns immediately if already granted.
    /// Returns None if rate-limited, Some(id) if prompt was enqueued.
    pub fn request_permission(
        &mut self,
        app_id: u64,
        app_name: &str,
        kind: PermissionKind,
        scope: PermissionScope,
    ) -> PermissionRequestResult {
        // Check existing decision.
        if let Some(stored) = self.store.lookup_with_scope(app_id, kind, &scope) {
            if stored.decision.is_allow() {
                self.store.record_use(app_id, kind, self.timestamp);
                return PermissionRequestResult::AlreadyGranted;
            } else {
                return PermissionRequestResult::PreviouslyDenied;
            }
        }

        // Rate limit check.
        if !self.rate_limiter.check_and_record(app_id, self.timestamp) {
            return PermissionRequestResult::RateLimited;
        }

        let id = self.next_request_id;
        self.next_request_id += 1;

        let request = PermissionRequest::new(id, app_id, app_name, kind, scope, self.timestamp);
        self.pending_prompts.push(request);
        self.try_show_next_prompt();
        PermissionRequestResult::PromptShown(id)
    }

    /// Request multiple permissions at once (grouped UI).
    pub fn request_multiple(
        &mut self,
        app_id: u64,
        app_name: &str,
        permissions: Vec<(PermissionKind, PermissionScope)>,
    ) -> PermissionRequestResult {
        if permissions.is_empty() {
            return PermissionRequestResult::AlreadyGranted;
        }

        // Filter out already-granted ones.
        let needed: Vec<_> = permissions
            .into_iter()
            .filter(|(kind, scope)| self.store.lookup_with_scope(app_id, *kind, scope).is_none())
            .collect();

        if needed.is_empty() {
            return PermissionRequestResult::AlreadyGranted;
        }

        if !self.rate_limiter.check_and_record(app_id, self.timestamp) {
            return PermissionRequestResult::RateLimited;
        }

        let id = self.next_request_id;
        self.next_request_id += 1;

        let (first_kind, first_scope) = needed[0].clone();
        let mut request = PermissionRequest::new(
            id,
            app_id,
            app_name,
            first_kind,
            first_scope,
            self.timestamp,
        );
        for (kind, _) in needed.iter().skip(1) {
            request.add_grouped(*kind);
        }

        self.pending_prompts.push(request);
        self.try_show_next_prompt();
        PermissionRequestResult::PromptShown(id)
    }

    /// Start a first-run wizard for a new app.
    pub fn start_first_run(
        &mut self,
        app_id: u64,
        app_name: &str,
        permissions: Vec<(PermissionKind, PermissionScope)>,
    ) {
        self.active_wizard = Some(FirstRunWizard::new(app_id, app_name, permissions));
    }

    /// Process a user decision on the active prompt.
    pub fn handle_decision(&mut self, result: PromptResult) {
        let decision = result.decision;
        let remember = result.remember || decision.is_persistent();

        if let Some(prompt) = &self.active_prompt {
            let app_id = prompt.request.app_id;
            let kind = prompt.request.kind;
            let scope = prompt.request.scope.clone();

            if remember {
                self.store.store(StoredPermission {
                    app_id,
                    kind,
                    scope,
                    decision,
                    granted_at: self.timestamp,
                    last_used: self.timestamp,
                    use_count: 0,
                });
            }
        }

        self.active_prompt = None;
        self.try_show_next_prompt();
    }

    /// Render the active prompt to framebuffer.
    pub fn render(&self, fb: &mut [u32], stride: usize) {
        if let Some(prompt) = &self.active_prompt {
            prompt.render(fb, stride);
        }
    }

    /// Handle a click event on the prompt.
    pub fn handle_click(&mut self, x: usize, y: usize) -> Option<PromptResult> {
        if let Some(prompt) = &mut self.active_prompt {
            if prompt.contains_point(x, y) {
                return prompt.handle_click(x, y);
            }
        }
        None
    }

    /// Is a prompt currently shown?
    pub fn is_prompt_active(&self) -> bool {
        self.active_prompt.is_some()
    }

    /// Revoke a previously granted permission.
    pub fn revoke(&mut self, app_id: u64, kind: PermissionKind) -> bool {
        self.store.revoke(app_id, kind)
    }

    /// Revoke all permissions for an app.
    pub fn revoke_all(&mut self, app_id: u64) -> usize {
        self.store.revoke_all_for_app(app_id)
    }

    /// List all permissions for an app.
    pub fn permissions_for_app(&self, app_id: u64) -> Vec<&StoredPermission> {
        self.store.permissions_for_app(app_id)
    }

    /// Check if app has permission (without prompting).
    pub fn has_permission(&self, app_id: u64, kind: PermissionKind) -> bool {
        self.store
            .lookup(app_id, kind)
            .map_or(false, |s| s.decision.is_allow())
    }

    /// Pending prompts in queue.
    pub fn pending_count(&self) -> usize {
        self.pending_prompts.len()
    }

    fn try_show_next_prompt(&mut self) {
        if self.active_prompt.is_some() || self.pending_prompts.is_empty() {
            return;
        }
        let request = self.pending_prompts.remove(0);
        self.active_prompt = Some(PermissionPrompt::new(
            request,
            self.screen_width,
            self.screen_height,
        ));
    }
}

// ── Result type ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionRequestResult {
    AlreadyGranted,
    PreviouslyDenied,
    RateLimited,
    PromptShown(u64),
}

// ── Framebuffer drawing helpers ───────────────────────────────────────────

fn fill_rect(fb: &mut [u32], stride: usize, x: usize, y: usize, w: usize, h: usize, color: u32) {
    for row in y..y + h {
        let start = row * stride + x;
        let end = start + w;
        if end <= fb.len() {
            for px in &mut fb[start..end] {
                *px = color;
            }
        }
    }
}

fn draw_border(fb: &mut [u32], stride: usize, x: usize, y: usize, w: usize, h: usize, color: u32) {
    // Top
    for col in x..x + w {
        let idx = y * stride + col;
        if idx < fb.len() {
            fb[idx] = color;
        }
    }
    // Bottom
    for col in x..x + w {
        let idx = (y + h - 1) * stride + col;
        if idx < fb.len() {
            fb[idx] = color;
        }
    }
    // Left
    for row in y..y + h {
        let idx = row * stride + x;
        if idx < fb.len() {
            fb[idx] = color;
        }
    }
    // Right
    for row in y..y + h {
        let idx = row * stride + x + w - 1;
        if idx < fb.len() {
            fb[idx] = color;
        }
    }
}

fn draw_text_simple(fb: &mut [u32], stride: usize, x: usize, y: usize, text: &str, color: u32) {
    let glyph_w = 8;
    let glyph_h = 8;
    for (i, _ch) in text.chars().enumerate() {
        let cx = x + i * glyph_w;
        // Simplified glyph: draw a small filled rect as placeholder per char.
        for row in 0..glyph_h {
            for col in 0..glyph_w - 2 {
                let px = (y + row) * stride + cx + col;
                if px < fb.len() && row > 0 && row < glyph_h - 1 {
                    fb[px] = color;
                }
            }
        }
    }
}

fn darken(color: u32) -> u32 {
    let a = color & 0xFF_00_00_00;
    let r = ((color >> 16) & 0xFF) * 3 / 4;
    let g = ((color >> 8) & 0xFF) * 3 / 4;
    let b = (color & 0xFF) * 3 / 4;
    a | (r << 16) | (g << 8) | b
}
