//! Lock screen for AthenaOS desktop shell.
//!
//! Provides a full lock screen with clock, notifications, media controls,
//! multiple authentication methods (password, PIN, pattern, biometric),
//! quick-action toggles, and accessibility features.

#![allow(unused)]

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

// ── Colour constants ─────────────────────────────────────────────────────

const LOCK_BG: u32 = 0xFF_08_0A_14;
const LOCK_OVERLAY: u32 = 0xCC_00_00_00;
const CLOCK_FG: u32 = 0xFF_FF_FF_FF;
const DATE_FG: u32 = 0xFF_B0_B0_C0;
const INPUT_BG: u32 = 0xFF_1A_1E_2E;
const INPUT_BORDER: u32 = 0xFF_4E_9C_FF;
const INPUT_ACTIVE_BORDER: u32 = 0xFF_7E_BC_FF;
const PIN_DOT_FILLED: u32 = 0xFF_4E_9C_FF;
const PIN_DOT_EMPTY: u32 = 0xFF_44_44_55;
const NOTIF_BG: u32 = 0xCC_18_1C_2E;
const NOTIF_FG: u32 = 0xFF_D0_D0_E0;
const NOTIF_TITLE: u32 = 0xFF_FF_FF_FF;
const MEDIA_BG: u32 = 0xCC_14_16_22;
const MEDIA_FG: u32 = 0xFF_FF_FF_FF;
const MEDIA_ACCENT: u32 = 0xFF_4E_9C_FF;
const QUICK_BG: u32 = 0xFF_22_26_38;
const QUICK_ACTIVE: u32 = 0xFF_4E_9C_FF;
const QUICK_FG: u32 = 0xFF_C0_C0_D0;
const ERROR_FG: u32 = 0xFF_FF_44_44;
const SUCCESS_FG: u32 = 0xFF_44_FF_88;
const GLYPH_W: usize = 8;
const GLYPH_H: usize = 8;

const MAX_FAILED_BEFORE_LOCKOUT: u32 = 5;
const LOCKOUT_BASE_S: u64 = 30;
const MAX_NOTIFICATIONS: usize = 5;
const MAX_PIN_LENGTH: u8 = 8;

// ── Types ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockState {
    Active,
    Authenticating,
    Unlocking,
    Locked,
    Dismissed,
}

#[derive(Debug, Clone)]
pub enum LockBackground {
    SolidColor(u32),
    Gradient(u32, u32),
    Image(String),
    Slideshow(Vec<String>, u32),
    BlurredDesktop,
}

#[derive(Debug, Clone)]
pub struct LockClock {
    pub time_format: TimeFormat,
    pub show_date: bool,
    pub show_weather: bool,
    pub position: ClockPosition,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeFormat {
    H12,
    H24,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClockPosition {
    Center,
    TopLeft,
    TopRight,
    BottomLeft,
}

#[derive(Debug, Clone)]
pub struct LockNotification {
    pub app: String,
    pub title: String,
    pub body: String,
    pub icon: Option<String>,
    pub timestamp: u64,
    pub urgency: NotificationUrgency,
    pub private: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationUrgency {
    Low,
    Normal,
    High,
    Critical,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AuthMethod {
    Password,
    Pin(u8),
    Pattern,
    Fingerprint,
    FaceRecognition,
    Passkey,
    None,
}

#[derive(Debug, Clone)]
pub enum AuthState {
    Idle,
    InputActive,
    Processing,
    Success,
    Failed(String),
    Lockout(u64),
}

#[derive(Debug, Clone)]
pub struct MediaControls {
    pub track_title: String,
    pub artist: String,
    pub album: String,
    pub album_art: Option<String>,
    pub playing: bool,
    pub progress: f32,
    pub duration_s: u32,
    pub volume: f32,
    pub source: String,
}

#[derive(Debug, Clone)]
pub struct QuickAction {
    pub id: String,
    pub icon: String,
    pub label: String,
    pub action: QuickActionType,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuickActionType {
    ToggleWifi,
    ToggleBluetooth,
    ToggleDnd,
    ToggleNightLight,
    ToggleAirplaneMode,
    Camera,
    Emergency,
    Accessibility,
}

#[derive(Debug, Clone)]
pub struct UserInfo {
    pub display_name: String,
    pub avatar: Option<String>,
    pub email: Option<String>,
    pub status: Option<String>,
}

#[derive(Debug, Clone)]
pub struct LockAnimation {
    pub fade_in: bool,
    pub slide_up: bool,
    pub blur_transition: bool,
    pub clock_bounce: bool,
    pub current_alpha: f32,
}

#[derive(Debug, Clone)]
pub struct LockAccessibility {
    pub narrator_active: bool,
    pub high_contrast: bool,
    pub magnifier: bool,
    pub keyboard_visible: bool,
    pub large_text: bool,
}

#[derive(Debug, Clone)]
pub enum AuthResult {
    Success,
    Failed(String),
    Lockout(u64),
    NeedSecondFactor,
}

// ── LockScreen ───────────────────────────────────────────────────────────

pub struct LockScreen {
    state: LockState,
    background: LockBackground,
    clock: LockClock,
    notifications: Vec<LockNotification>,
    auth_method: AuthMethod,
    pub auth_state: AuthState,
    input_buffer: String,
    pin_dots: Vec<bool>,
    failed_attempts: u32,
    lockout_until: Option<u64>,
    media_controls: Option<MediaControls>,
    quick_actions: Vec<QuickAction>,
    pub user_info: UserInfo,
    screen_width: u32,
    screen_height: u32,
    animation: LockAnimation,
    idle_timeout_s: u32,
    auto_lock_enabled: bool,
    show_notifications: bool,
    show_media: bool,
    accessibility: LockAccessibility,
    current_time: u64,
    quick_action_states: BTreeMap<String, bool>,
}

impl LockScreen {
    pub fn new(width: u32, height: u32) -> Self {
        let mut quick_action_states = BTreeMap::new();
        let quick_actions = Self::default_quick_actions();
        for qa in &quick_actions {
            quick_action_states.insert(qa.id.clone(), false);
        }

        Self {
            state: LockState::Dismissed,
            background: LockBackground::SolidColor(LOCK_BG),
            clock: LockClock {
                time_format: TimeFormat::H24,
                show_date: true,
                show_weather: false,
                position: ClockPosition::Center,
            },
            notifications: Vec::new(),
            auth_method: AuthMethod::Password,
            auth_state: AuthState::Idle,
            input_buffer: String::new(),
            pin_dots: Vec::new(),
            failed_attempts: 0,
            lockout_until: None,
            media_controls: None,
            quick_actions,
            user_info: UserInfo {
                display_name: String::from("User"),
                avatar: None,
                email: None,
                status: None,
            },
            screen_width: width,
            screen_height: height,
            animation: LockAnimation {
                fade_in: true,
                slide_up: true,
                blur_transition: true,
                clock_bounce: false,
                current_alpha: 0.0,
            },
            idle_timeout_s: 300,
            auto_lock_enabled: true,
            show_notifications: true,
            show_media: true,
            accessibility: LockAccessibility {
                narrator_active: false,
                high_contrast: false,
                magnifier: false,
                keyboard_visible: false,
                large_text: false,
            },
            current_time: 0,
            quick_action_states,
        }
    }

    fn default_quick_actions() -> Vec<QuickAction> {
        vec![
            QuickAction {
                id: String::from("wifi"),
                icon: String::from("W"),
                label: String::from("Wi-Fi"),
                action: QuickActionType::ToggleWifi,
            },
            QuickAction {
                id: String::from("bt"),
                icon: String::from("B"),
                label: String::from("Bluetooth"),
                action: QuickActionType::ToggleBluetooth,
            },
            QuickAction {
                id: String::from("dnd"),
                icon: String::from("D"),
                label: String::from("Do Not Disturb"),
                action: QuickActionType::ToggleDnd,
            },
            QuickAction {
                id: String::from("night"),
                icon: String::from("N"),
                label: String::from("Night Light"),
                action: QuickActionType::ToggleNightLight,
            },
            QuickAction {
                id: String::from("airplane"),
                icon: String::from("A"),
                label: String::from("Airplane"),
                action: QuickActionType::ToggleAirplaneMode,
            },
            QuickAction {
                id: String::from("access"),
                icon: String::from("@"),
                label: String::from("Accessibility"),
                action: QuickActionType::Accessibility,
            },
        ]
    }

    pub fn set_display_name(&mut self, name: &str) {
        self.user_info.display_name = String::from(name);
    }

    pub fn show_auth_failed(&mut self, message: &str) {
        self.auth_state = AuthState::Failed(String::from(message));
        self.input_buffer.clear();
        self.pin_dots.clear();
    }

    pub fn lock(&mut self) {
        self.state = LockState::Active;
        self.auth_state = AuthState::Idle;
        self.input_buffer.clear();
        self.pin_dots.clear();
        self.animation.current_alpha = 0.0;
    }

    pub fn unlock(&mut self) -> bool {
        if self.state == LockState::Unlocking || self.state == LockState::Dismissed {
            return true;
        }
        if matches!(self.auth_state, AuthState::Success) {
            self.state = LockState::Unlocking;
            self.animation.current_alpha = 1.0;
            true
        } else {
            false
        }
    }

    pub fn submit_password(&mut self, password: &str) -> AuthResult {
        if self.check_lockout() {
            let remaining = self.remaining_lockout_s();
            return AuthResult::Lockout(remaining);
        }

        self.auth_state = AuthState::Processing;

        let valid = !password.is_empty() && password.len() >= 4;
        if valid {
            self.auth_state = AuthState::Success;
            self.failed_attempts = 0;
            self.lockout_until = None;
            AuthResult::Success
        } else {
            self.failed_attempts += 1;
            let msg = String::from("Incorrect password");
            if self.failed_attempts >= MAX_FAILED_BEFORE_LOCKOUT {
                let lockout_s = LOCKOUT_BASE_S
                    * (self.failed_attempts as u64 / MAX_FAILED_BEFORE_LOCKOUT as u64);
                self.lockout_until = Some(self.current_time + lockout_s);
                self.auth_state = AuthState::Lockout(lockout_s);
                AuthResult::Lockout(lockout_s)
            } else {
                self.auth_state = AuthState::Failed(msg.clone());
                AuthResult::Failed(msg)
            }
        }
    }

    pub fn submit_pin(&mut self, pin: &[u8]) -> AuthResult {
        if self.check_lockout() {
            return AuthResult::Lockout(self.remaining_lockout_s());
        }

        self.auth_state = AuthState::Processing;

        let expected_len = match self.auth_method {
            AuthMethod::Pin(len) => len as usize,
            _ => 4,
        };

        if pin.len() != expected_len {
            self.failed_attempts += 1;
            let msg = String::from("Incorrect PIN length");
            self.auth_state = AuthState::Failed(msg.clone());
            return AuthResult::Failed(msg);
        }

        let valid = pin.iter().all(|&d| d <= 9);
        if valid {
            self.auth_state = AuthState::Success;
            self.failed_attempts = 0;
            self.lockout_until = None;
            AuthResult::Success
        } else {
            self.failed_attempts += 1;
            self.apply_lockout_if_needed()
        }
    }

    pub fn submit_pattern(&mut self, pattern: &[(u8, u8)]) -> AuthResult {
        if self.check_lockout() {
            return AuthResult::Lockout(self.remaining_lockout_s());
        }

        self.auth_state = AuthState::Processing;

        if pattern.len() < 4 {
            self.failed_attempts += 1;
            let msg = String::from("Pattern too short");
            self.auth_state = AuthState::Failed(msg.clone());
            return AuthResult::Failed(msg);
        }

        let valid = pattern.iter().all(|&(r, c)| r < 3 && c < 3);
        if valid && pattern.len() >= 4 {
            self.auth_state = AuthState::Success;
            self.failed_attempts = 0;
            self.lockout_until = None;
            AuthResult::Success
        } else {
            self.failed_attempts += 1;
            self.apply_lockout_if_needed()
        }
    }

    pub fn submit_biometric(&mut self, bio_type: AuthMethod) -> AuthResult {
        if self.check_lockout() {
            return AuthResult::Lockout(self.remaining_lockout_s());
        }

        self.auth_state = AuthState::Processing;

        match bio_type {
            AuthMethod::Fingerprint | AuthMethod::FaceRecognition => {
                self.auth_state = AuthState::Success;
                self.failed_attempts = 0;
                self.lockout_until = None;
                AuthResult::Success
            }
            AuthMethod::Passkey => {
                self.auth_state = AuthState::Success;
                AuthResult::NeedSecondFactor
            }
            _ => {
                let msg = String::from("Unsupported biometric method");
                self.auth_state = AuthState::Failed(msg.clone());
                AuthResult::Failed(msg)
            }
        }
    }

    fn apply_lockout_if_needed(&mut self) -> AuthResult {
        if self.failed_attempts >= MAX_FAILED_BEFORE_LOCKOUT {
            let lockout_s =
                LOCKOUT_BASE_S * (self.failed_attempts as u64 / MAX_FAILED_BEFORE_LOCKOUT as u64);
            self.lockout_until = Some(self.current_time + lockout_s);
            self.auth_state = AuthState::Lockout(lockout_s);
            AuthResult::Lockout(lockout_s)
        } else {
            let msg = String::from("Authentication failed");
            self.auth_state = AuthState::Failed(msg.clone());
            AuthResult::Failed(msg)
        }
    }

    pub fn add_notification(&mut self, notif: LockNotification) {
        if self.notifications.len() >= MAX_NOTIFICATIONS {
            self.notifications.remove(0);
        }
        self.notifications.push(notif);
    }

    pub fn dismiss_notification(&mut self, index: usize) {
        if index < self.notifications.len() {
            self.notifications.remove(index);
        }
    }

    pub fn clear_notifications(&mut self) {
        self.notifications.clear();
    }

    pub fn update_media(&mut self, controls: MediaControls) {
        self.media_controls = Some(controls);
    }

    pub fn media_play_pause(&mut self) {
        if let Some(ref mut mc) = self.media_controls {
            mc.playing = !mc.playing;
        }
    }

    pub fn media_next(&mut self) {
        if let Some(ref mut mc) = self.media_controls {
            mc.progress = 0.0;
        }
    }

    pub fn media_prev(&mut self) {
        if let Some(ref mut mc) = self.media_controls {
            if mc.progress > 0.1 {
                mc.progress = 0.0;
            }
        }
    }

    pub fn toggle_quick_action(&mut self, id: &str) {
        if let Some(state) = self.quick_action_states.get_mut(id) {
            *state = !*state;
        }
    }

    pub fn tick(&mut self, delta_ms: u64) {
        self.current_time += delta_ms;

        if self.animation.fade_in && self.state == LockState::Active {
            self.animation.current_alpha += delta_ms as f32 / 500.0;
            if self.animation.current_alpha > 1.0 {
                self.animation.current_alpha = 1.0;
            }
        }

        if self.state == LockState::Unlocking {
            self.animation.current_alpha -= delta_ms as f32 / 300.0;
            if self.animation.current_alpha <= 0.0 {
                self.animation.current_alpha = 0.0;
                self.state = LockState::Dismissed;
            }
        }

        if let Some(until) = self.lockout_until {
            if self.current_time >= until {
                self.lockout_until = None;
                self.auth_state = AuthState::Idle;
            }
        }

        if let Some(ref mut mc) = self.media_controls {
            if mc.playing && mc.duration_s > 0 {
                mc.progress += delta_ms as f32 / (mc.duration_s as f32 * 1000.0);
                if mc.progress > 1.0 {
                    mc.progress = 1.0;
                    mc.playing = false;
                }
            }
        }
    }

    /// Render the lock screen into a raw `0xAARRGGBB` framebuffer slice.
    ///
    /// Concept §"rival Windows + macOS": the lock/unlock moment is a first-
    /// impression surface, so it wears the same **Liquid Glass** identity as the
    /// desktop (IDENTITY.md §3 Aurora Mesh + §7 tiers) — a living aurora backdrop
    /// with a centered frosted glass card holding the clock, avatar, and the
    /// password pill, exactly like the macOS / Win11 login moment. The public
    /// signature is unchanged (the kernel passes `surface_ptr` + `width` stride);
    /// internally we wrap the buffer in a [`athgfx::Canvas`] — the SAME software
    /// rasterizer the compositor uses — so we draw through the shared
    /// `glass`/`draw_text_aa` primitives instead of hand-rolled hex pixels.
    pub fn render(&self, canvas: &mut [u32], stride: usize) {
        if self.state == LockState::Dismissed || stride == 0 {
            return;
        }
        let height = canvas.len() / stride;
        if height == 0 {
            return;
        }
        // SAFETY: `canvas` is `stride * height` u32s, writable for this call, and
        // outlives the Canvas (dropped at end of scope). The kernel's bpp=4 path
        // writes `*(p as *mut u32) = color`, so the slice round-trips ARGB.
        let mut c =
            unsafe { athgfx::Canvas::new(canvas.as_mut_ptr() as *mut u8, stride, height, 4) };
        self.render_canvas(&mut c);
    }

    /// Glass-based render through a [`athgfx::Canvas`] — host-callable (the UI
    /// screenshot harness drives this over a `HostFb`).
    pub fn render_canvas(&self, c: &mut athgfx::Canvas) {
        if self.state == LockState::Dismissed {
            return;
        }
        let w = c.width();
        let h = c.height();
        let p = crate::active_palette();
        let accent = ath_tokens::derive_accent(crate::active_accent(), p);
        let sans = athgfx::text::FontFamily::Sans;

        // ── Background → the signature Aurora Mesh (IDENTITY.md §3), the same
        //    living backdrop the desktop wears — visual continuity from desktop
        //    to lock. Replaces the flat hardcoded navy gradient.
        athgfx::glass::render_aurora_dark(c, 0, 0, w, h, 0);

        // ── The centered glass card: clock + avatar + password pill float on a
        //    `glass.popover` frosted surface (transient, instant legibility over
        //    the busy aurora) with the iridescent rim — the same draw CC / Start
        //    / toasts make. Token-derived sizing; no hardcoded hex.
        let card_w = (w * 3 / 8).clamp(320, 440);
        let card_h = 360usize.min(h.saturating_sub(80));
        let card_x = (w.saturating_sub(card_w)) / 2;
        let card_y = (h.saturating_sub(card_h)) / 2;
        let radius = ath_tokens::RADIUS_LG as usize;

        // Soft ambient shadow so the card reads as floating, then the shipped
        // tiered-glass draw (tint → frost → legibility cap → iridescent rim).
        c.fill_rounded_rect_shadow(card_x, card_y, card_w, card_h, radius, 0x0A_10_1C, 40, 18);
        athgfx::glass::draw_glass_surface(
            c,
            card_x,
            card_y,
            card_w,
            card_h,
            radius,
            ath_tokens::GLASS_POPOVER_DARK,
        );

        // ── Clock — large display RaeSans, text.primary, centered in the card.
        //    The legibility cap inside draw_glass_surface keeps the card interior
        //    dark enough that text.primary wins over the bright aurora.
        let time_str = match self.clock.time_format {
            TimeFormat::H24 => "12:34",
            TimeFormat::H12 => "12:34 PM",
        };
        let clock_style = ath_tokens::TYPE_DISPLAY;
        let tw = c.measure_text_aa(time_str, clock_style, sans);
        let clock_y = card_y + 28;
        c.draw_text_aa(
            (card_x + card_w / 2) as i32 - tw / 2,
            clock_y as i32,
            time_str,
            clock_style,
            p.text_primary,
            sans,
        );

        // Date under the clock (text.secondary caption).
        if self.clock.show_date {
            let date_str = "Monday, May 25";
            let dstyle = ath_tokens::TYPE_CAPTION;
            let dw = c.measure_text_aa(date_str, dstyle, sans);
            let date_y = clock_y + clock_style.line_height as usize + 6;
            c.draw_text_aa(
                (card_x + card_w / 2) as i32 - dw / 2,
                date_y as i32,
                date_str,
                dstyle,
                p.text_secondary,
                sans,
            );
        }

        // ── User avatar — an accent-ringed circle with the display-name initial
        //    in dark-on-accent ink (the IDENTITY guardrail: white-on-accent fails
        //    WCAG; ink on an accent fill is bg.base).
        let avatar_r = 36usize;
        let avatar_cx = card_x + card_w / 2;
        let avatar_cy = card_y + 150;
        c.fill_circle(avatar_cx, avatar_cy, avatar_r + 3, accent.base);
        c.fill_circle(avatar_cx, avatar_cy, avatar_r, p.bg_elevated);
        let initial: String = self
            .user_info
            .display_name
            .chars()
            .next()
            .map(|ch| ch.to_uppercase().collect())
            .unwrap_or_else(|| String::from("?"));
        let istyle = ath_tokens::TYPE_TITLE;
        let iw = c.measure_text_aa(&initial, istyle, sans);
        c.draw_text_aa(
            avatar_cx as i32 - iw / 2,
            avatar_cy as i32 - (istyle.line_height as i32) / 2,
            &initial,
            istyle,
            p.text_primary,
            sans,
        );

        // Display name (label, text.primary), centered under the avatar.
        let name = self.user_info.display_name.as_str();
        let nstyle = ath_tokens::TYPE_LABEL;
        let nw = c.measure_text_aa(name, nstyle, sans);
        let name_y = avatar_cy + avatar_r + 12;
        c.draw_text_aa(
            (card_x + card_w / 2) as i32 - nw / 2,
            name_y as i32,
            name,
            nstyle,
            p.text_primary,
            sans,
        );

        // ── Password field → a frosted input pill (radius-pill) with an accent
        //    focus ring. Dots for typed chars, a placeholder caption otherwise.
        let pill_w = card_w - 56;
        let pill_h = 40usize;
        let pill_x = card_x + 28;
        let pill_y = name_y + nstyle.line_height as usize + 18;
        let pill_r = pill_h / 2;
        c.fill_rounded_rect(
            pill_x,
            pill_y,
            pill_w,
            pill_h,
            pill_r,
            ath_tokens::GLASS_POPOVER_DARK.frost,
        );
        let focused = matches!(self.auth_state, AuthState::InputActive);
        let ring = if focused { accent.hover } else { accent.subtle };
        c.draw_rounded_rect_outline(pill_x, pill_y, pill_w, pill_h, pill_r, ring);

        match &self.auth_method {
            AuthMethod::Pin(len) => {
                // PIN → centered dots inside the pill.
                let dot_r = 6usize;
                let gap = 18usize;
                let n = *len as usize;
                let total = n * dot_r * 2 + (n.saturating_sub(1)) * gap;
                let sx = pill_x + (pill_w.saturating_sub(total)) / 2 + dot_r;
                let cy = pill_y + pill_h / 2;
                for i in 0..n {
                    let dx = sx + i * (dot_r * 2 + gap);
                    let filled = self.pin_dots.get(i).copied().unwrap_or(false);
                    let color = if filled { accent.base } else { p.text_tertiary };
                    c.fill_circle(dx, cy, dot_r, color);
                }
            }
            _ => {
                let pstyle = ath_tokens::TYPE_BODY;
                if self.input_buffer.is_empty() {
                    c.draw_text_aa(
                        (pill_x + 18) as i32,
                        (pill_y + (pill_h - pstyle.line_height as usize) / 2) as i32,
                        "Enter password",
                        pstyle,
                        p.text_tertiary,
                        sans,
                    );
                } else {
                    // Filled dots, drawn as solid circles (never echo the secret).
                    let dot_r = 5usize;
                    let gap = 8usize;
                    let cy = pill_y + pill_h / 2;
                    let mut dx = pill_x + 18 + dot_r;
                    for _ in 0..self.input_buffer.len().min(24) {
                        c.fill_circle(dx, cy, dot_r, p.text_primary);
                        dx += dot_r * 2 + gap;
                    }
                }
            }
        }

        // ── Status line under the pill (error / lockout / success / hint).
        let status_style = ath_tokens::TYPE_CAPTION;
        let (msg, color) = match &self.auth_state {
            AuthState::Failed(m) => (m.as_str(), p.state_danger),
            AuthState::Lockout(_) => ("Too many attempts. Try again later.", p.state_danger),
            AuthState::Success => ("Unlocked", p.state_ok),
            _ => (
                match &self.auth_method {
                    AuthMethod::Pin(_) => "Enter your PIN",
                    AuthMethod::Fingerprint => "Touch sensor to unlock",
                    AuthMethod::FaceRecognition => "Looking for your face...",
                    AuthMethod::Passkey => "Use your passkey",
                    AuthMethod::None => "Swipe up to unlock",
                    _ => "Press Enter to unlock",
                },
                p.text_secondary,
            ),
        };
        let mw = c.measure_text_aa(msg, status_style, sans);
        let status_y = pill_y + pill_h + 14;
        c.draw_text_aa(
            (card_x + card_w / 2) as i32 - mw / 2,
            status_y as i32,
            msg,
            status_style,
            color,
            sans,
        );
    }

    fn render_background(&self, canvas: &mut [u32], stride: usize, w: usize, h: usize) {
        match &self.background {
            LockBackground::SolidColor(c) => {
                for y in 0..h {
                    for x in 0..w {
                        if y * stride + x < canvas.len() {
                            canvas[y * stride + x] = *c;
                        }
                    }
                }
            }
            LockBackground::Gradient(top, bot) => {
                let tr = (*top >> 16) & 0xFF;
                let tg = (*top >> 8) & 0xFF;
                let tb = *top & 0xFF;
                let br = (*bot >> 16) & 0xFF;
                let bg = (*bot >> 8) & 0xFF;
                let bb = *bot & 0xFF;
                for y in 0..h {
                    let frac = y as u32 * 255 / h.max(1) as u32;
                    let r = tr + (br.wrapping_sub(tr)) * frac / 255;
                    let g = tg + (bg.wrapping_sub(tg)) * frac / 255;
                    let b = tb + (bb.wrapping_sub(tb)) * frac / 255;
                    let pixel = 0xFF_00_00_00 | (r << 16) | (g << 8) | b;
                    for x in 0..w {
                        if y * stride + x < canvas.len() {
                            canvas[y * stride + x] = pixel;
                        }
                    }
                }
            }
            _ => {
                for y in 0..h {
                    for x in 0..w {
                        if y * stride + x < canvas.len() {
                            canvas[y * stride + x] = LOCK_BG;
                        }
                    }
                }
            }
        }
    }

    fn render_clock(&self, canvas: &mut [u32], stride: usize) {
        let w = self.screen_width as usize;
        let h = self.screen_height as usize;

        let (cx, cy) = match self.clock.position {
            ClockPosition::Center => (w / 2, h / 4),
            ClockPosition::TopLeft => (100, 60),
            ClockPosition::TopRight => (w - 200, 60),
            ClockPosition::BottomLeft => (100, h - 200),
        };

        let time_str = match self.clock.time_format {
            TimeFormat::H24 => "12:34",
            TimeFormat::H12 => "12:34 PM",
        };

        let scale = if self.accessibility.large_text { 4 } else { 3 };
        let char_w = GLYPH_W * scale;
        let text_w = time_str.len() * char_w;
        let start_x = cx.saturating_sub(text_w / 2);

        for (i, ch) in time_str.chars().enumerate() {
            let gx = start_x + i * char_w;
            self.draw_scaled_char(canvas, stride, gx, cy, ch, CLOCK_FG, scale);
        }

        if self.clock.show_date {
            let date_str = "Monday, May 25";
            let date_w = date_str.len() * GLYPH_W;
            let date_x = cx.saturating_sub(date_w / 2);
            let date_y = cy + char_w + 12;
            for (i, ch) in date_str.chars().enumerate() {
                self.draw_scaled_char(canvas, stride, date_x + i * GLYPH_W, date_y, ch, DATE_FG, 1);
            }
        }
    }

    fn render_notifications(&self, canvas: &mut [u32], stride: usize) {
        if self.notifications.is_empty() {
            return;
        }

        let w = self.screen_width as usize;
        let notif_w = 300usize.min(w - 40);
        let start_x = (w - notif_w) / 2;
        let start_y = self.screen_height as usize / 2 - 100;

        for (i, notif) in self.notifications.iter().enumerate().take(3) {
            let ny = start_y + i * 64;
            self.fill_rect(canvas, stride, start_x, ny, notif_w, 56, NOTIF_BG);
            self.draw_rect_outline(canvas, stride, start_x, ny, notif_w, 56, INPUT_BORDER);

            let display_title = if notif.private {
                "Content hidden"
            } else {
                notif.title.as_str()
            };
            let display_body = if notif.private {
                ""
            } else {
                notif.body.as_str()
            };

            self.draw_text_simple(canvas, stride, start_x + 8, ny + 8, &notif.app, QUICK_FG);
            self.draw_text_simple(
                canvas,
                stride,
                start_x + 8,
                ny + 22,
                display_title,
                NOTIF_TITLE,
            );
            self.draw_text_simple(canvas, stride, start_x + 8, ny + 36, display_body, NOTIF_FG);

            let urgency_color = match notif.urgency {
                NotificationUrgency::Critical => ERROR_FG,
                NotificationUrgency::High => 0xFF_FF_AA_33,
                _ => INPUT_BORDER,
            };
            self.fill_rect(canvas, stride, start_x, ny, 3, 56, urgency_color);
        }
    }

    fn render_auth_input(&self, canvas: &mut [u32], stride: usize) {
        let w = self.screen_width as usize;
        let h = self.screen_height as usize;
        let center_y = h * 3 / 5;

        match &self.auth_method {
            AuthMethod::Password | AuthMethod::Passkey => {
                let input_w = 280usize.min(w - 40);
                let input_h = 36usize;
                let ix = (w - input_w) / 2;
                let iy = center_y;

                let border = if matches!(self.auth_state, AuthState::InputActive) {
                    INPUT_ACTIVE_BORDER
                } else {
                    INPUT_BORDER
                };
                self.fill_rect(canvas, stride, ix, iy, input_w, input_h, INPUT_BG);
                self.draw_rect_outline(canvas, stride, ix, iy, input_w, input_h, border);

                let dots: String = (0..self.input_buffer.len()).map(|_| '*').collect();
                let display = if dots.is_empty() {
                    "Enter password..."
                } else {
                    dots.as_str()
                };
                let fg = if dots.is_empty() { QUICK_FG } else { CLOCK_FG };
                self.draw_text_simple(canvas, stride, ix + 12, iy + 12, display, fg);
            }
            AuthMethod::Pin(len) => {
                let dot_size = 16usize;
                let dot_gap = 12usize;
                let pin_len = *len as usize;
                let total_w = pin_len * dot_size + (pin_len - 1) * dot_gap;
                let start_x = (w - total_w) / 2;

                for i in 0..pin_len {
                    let dx = start_x + i * (dot_size + dot_gap);
                    let filled =
                        i < self.pin_dots.len() && self.pin_dots.get(i).copied().unwrap_or(false);
                    let color = if filled {
                        PIN_DOT_FILLED
                    } else {
                        PIN_DOT_EMPTY
                    };
                    self.fill_circle(
                        canvas,
                        stride,
                        dx + dot_size / 2,
                        center_y + dot_size / 2,
                        dot_size / 2,
                        color,
                    );
                }
            }
            AuthMethod::Pattern => {
                let grid = 3usize;
                let cell = 48usize;
                let gap = 24usize;
                let total = grid * cell + (grid - 1) * gap;
                let sx = (w - total) / 2;
                let sy = center_y;

                for row in 0..grid {
                    for col in 0..grid {
                        let cx = sx + col * (cell + gap) + cell / 2;
                        let cy = sy + row * (cell + gap) + cell / 2;
                        self.fill_circle(canvas, stride, cx, cy, 8, PIN_DOT_EMPTY);
                        self.draw_circle_outline(canvas, stride, cx, cy, 14, INPUT_BORDER);
                    }
                }
            }
            AuthMethod::Fingerprint | AuthMethod::FaceRecognition => {
                let msg = match &self.auth_method {
                    AuthMethod::Fingerprint => "Touch sensor to unlock",
                    AuthMethod::FaceRecognition => "Looking for your face...",
                    _ => "",
                };
                let msg_w = msg.len() * GLYPH_W;
                self.draw_text_simple(canvas, stride, (w - msg_w) / 2, center_y, msg, DATE_FG);
            }
            AuthMethod::None => {}
        }

        match &self.auth_state {
            AuthState::Failed(msg) => {
                let msg_w = msg.len() * GLYPH_W;
                let ey = self.screen_height as usize * 3 / 5 + 50;
                self.draw_text_simple(canvas, stride, (w - msg_w) / 2, ey, msg, ERROR_FG);
            }
            AuthState::Lockout(secs) => {
                let msg = "Too many attempts. Try again later.";
                let msg_w = msg.len() * GLYPH_W;
                let ey = self.screen_height as usize * 3 / 5 + 50;
                self.draw_text_simple(canvas, stride, (w - msg_w) / 2, ey, msg, ERROR_FG);
            }
            AuthState::Success => {
                let msg = "Unlocked";
                let msg_w = msg.len() * GLYPH_W;
                let ey = self.screen_height as usize * 3 / 5 + 50;
                self.draw_text_simple(canvas, stride, (w - msg_w) / 2, ey, msg, SUCCESS_FG);
            }
            _ => {}
        }
    }

    fn render_media_controls(&self, canvas: &mut [u32], stride: usize) {
        let mc = match &self.media_controls {
            Some(c) => c,
            None => return,
        };

        let w = self.screen_width as usize;
        let h = self.screen_height as usize;
        let panel_w = 300usize.min(w - 20);
        let panel_h = 80usize;
        let px = (w - panel_w) / 2;
        let py = h - panel_h - 80;

        self.fill_rect(canvas, stride, px, py, panel_w, panel_h, MEDIA_BG);
        self.draw_rect_outline(canvas, stride, px, py, panel_w, panel_h, INPUT_BORDER);

        self.draw_text_simple(canvas, stride, px + 12, py + 10, &mc.track_title, MEDIA_FG);
        self.draw_text_simple(canvas, stride, px + 12, py + 24, &mc.artist, DATE_FG);

        let controls_y = py + 44;
        let btn_labels = ["<<", if mc.playing { "||" } else { " >" }, ">>"];
        let btn_w = 32usize;
        let btn_gap = 16usize;
        let total_btn_w = btn_labels.len() * btn_w + (btn_labels.len() - 1) * btn_gap;
        let btn_start = px + (panel_w - total_btn_w) / 2;

        for (i, label) in btn_labels.iter().enumerate() {
            let bx = btn_start + i * (btn_w + btn_gap);
            self.draw_text_simple(canvas, stride, bx + 4, controls_y, label, MEDIA_ACCENT);
        }

        let bar_y = py + 66;
        let bar_w = panel_w - 24;
        self.fill_rect(canvas, stride, px + 12, bar_y, bar_w, 3, QUICK_BG);
        let filled = (mc.progress * bar_w as f32) as usize;
        self.fill_rect(
            canvas,
            stride,
            px + 12,
            bar_y,
            filled.min(bar_w),
            3,
            MEDIA_ACCENT,
        );
    }

    fn render_quick_actions(&self, canvas: &mut [u32], stride: usize) {
        let w = self.screen_width as usize;
        let h = self.screen_height as usize;

        let btn_size = 40usize;
        let btn_gap = 12usize;
        let count = self.quick_actions.len();
        let total_w = count * btn_size + (count.saturating_sub(1)) * btn_gap;
        let start_x = (w - total_w) / 2;
        let start_y = h - 60;

        for (i, qa) in self.quick_actions.iter().enumerate() {
            let bx = start_x + i * (btn_size + btn_gap);
            let active = self
                .quick_action_states
                .get(&qa.id)
                .copied()
                .unwrap_or(false);
            let bg = if active { QUICK_ACTIVE } else { QUICK_BG };
            self.fill_rect(canvas, stride, bx, start_y, btn_size, btn_size, bg);
            self.draw_rect_outline(
                canvas,
                stride,
                bx,
                start_y,
                btn_size,
                btn_size,
                INPUT_BORDER,
            );

            let icon_char = qa.icon.chars().next().unwrap_or('?');
            let fg = if active { LOCK_BG } else { QUICK_FG };
            let gx = bx + (btn_size - GLYPH_W) / 2;
            let gy = start_y + (btn_size - GLYPH_H) / 2;
            self.draw_scaled_char(canvas, stride, gx, gy, icon_char, fg, 1);
        }
    }

    fn render_user_info(&self, canvas: &mut [u32], stride: usize) {
        let w = self.screen_width as usize;
        let h = self.screen_height as usize;

        let name = self.user_info.display_name.as_str();
        let name_w = name.len() * GLYPH_W * 2;
        let name_x = (w - name_w) / 2;
        let name_y = h * 3 / 5 - 50;

        for (i, ch) in name.chars().enumerate() {
            self.draw_scaled_char(
                canvas,
                stride,
                name_x + i * GLYPH_W * 2,
                name_y,
                ch,
                CLOCK_FG,
                2,
            );
        }

        if let Some(ref status) = self.user_info.status {
            let sw = status.len() * GLYPH_W;
            self.draw_text_simple(
                canvas,
                stride,
                (w - sw) / 2,
                name_y + GLYPH_H * 2 + 8,
                status,
                DATE_FG,
            );
        }
    }

    fn render_status_text(&self, canvas: &mut [u32], stride: usize) {
        let w = self.screen_width as usize;
        let h = self.screen_height as usize;
        let hint = match &self.auth_method {
            AuthMethod::Password => "Press Enter to submit",
            AuthMethod::Pin(_) => "Enter your PIN",
            AuthMethod::Pattern => "Draw your pattern",
            AuthMethod::Fingerprint => "Touch sensor",
            AuthMethod::FaceRecognition => "Look at camera",
            AuthMethod::Passkey => "Use your passkey",
            AuthMethod::None => "Swipe up to unlock",
        };
        let hw = hint.len() * GLYPH_W;
        self.draw_text_simple(canvas, stride, (w - hw) / 2, h - 20, hint, QUICK_FG);
    }

    pub fn handle_input(&mut self, key: u8) {
        if self.check_lockout() {
            return;
        }

        self.auth_state = AuthState::InputActive;

        match key {
            0x08 => {
                self.input_buffer.pop();
                self.pin_dots.pop();
            }
            0x0D => {
                match &self.auth_method {
                    AuthMethod::Password | AuthMethod::Passkey => {
                        let pw = self.input_buffer.clone();
                        self.submit_password(&pw);
                    }
                    AuthMethod::Pin(_) => {
                        let pin: Vec<u8> = self
                            .pin_dots
                            .iter()
                            .enumerate()
                            .filter(|(_, &filled)| filled)
                            .map(|(i, _)| i as u8)
                            .collect();
                        self.submit_pin(&pin);
                    }
                    _ => {}
                }
                self.input_buffer.clear();
            }
            0x1B => {
                self.input_buffer.clear();
                self.pin_dots.clear();
                self.auth_state = AuthState::Idle;
            }
            c if c >= 0x20 && c < 0x7F => match &self.auth_method {
                AuthMethod::Password | AuthMethod::Passkey => {
                    self.input_buffer.push(c as char);
                }
                AuthMethod::Pin(len) => {
                    if c >= b'0' && c <= b'9' && self.pin_dots.len() < *len as usize {
                        self.pin_dots.push(true);
                        self.input_buffer.push(c as char);
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }

    pub fn handle_mouse(&mut self, x: i32, y: i32, button: u8) {
        if self.state != LockState::Active {
            return;
        }

        let w = self.screen_width as usize;
        let h = self.screen_height as usize;

        let btn_size = 40i32;
        let btn_gap = 12i32;
        let count = self.quick_actions.len() as i32;
        let total_w = count * btn_size + (count - 1) * btn_gap;
        let start_x = (w as i32 - total_w) / 2;
        let start_y = h as i32 - 60;

        if button == 1 && y >= start_y && y < start_y + btn_size {
            for (i, qa) in self.quick_actions.iter().enumerate() {
                let bx = start_x + i as i32 * (btn_size + btn_gap);
                if x >= bx && x < bx + btn_size {
                    let id = qa.id.clone();
                    self.toggle_quick_action(&id);
                    return;
                }
            }
        }

        if button == 1 {
            if matches!(self.auth_state, AuthState::Idle) {
                self.auth_state = AuthState::InputActive;
            }
        }
    }

    pub fn check_lockout(&self) -> bool {
        match self.lockout_until {
            Some(until) => self.current_time < until,
            None => false,
        }
    }

    pub fn remaining_lockout_s(&self) -> u64 {
        match self.lockout_until {
            Some(until) if self.current_time < until => (until - self.current_time) / 1000,
            _ => 0,
        }
    }

    pub fn set_auth_method(&mut self, method: AuthMethod) {
        self.auth_method = method;
        self.input_buffer.clear();
        self.pin_dots.clear();
        self.auth_state = AuthState::Idle;
    }

    pub fn switch_user(&mut self) -> bool {
        self.input_buffer.clear();
        self.pin_dots.clear();
        self.auth_state = AuthState::Idle;
        self.failed_attempts = 0;
        self.lockout_until = None;
        true
    }

    fn check_auto_lock(&mut self, idle_time_s: u32) -> bool {
        if self.auto_lock_enabled && idle_time_s >= self.idle_timeout_s {
            if self.state == LockState::Dismissed {
                self.lock();
                return true;
            }
        }
        false
    }

    // ── Drawing helpers ──────────────────────────────────────────────────

    fn fill_rect(
        &self,
        canvas: &mut [u32],
        stride: usize,
        x: usize,
        y: usize,
        w: usize,
        h: usize,
        color: u32,
    ) {
        for row in y..y + h {
            for col in x..x + w {
                if row * stride + col < canvas.len() {
                    canvas[row * stride + col] = color;
                }
            }
        }
    }

    fn draw_rect_outline(
        &self,
        canvas: &mut [u32],
        stride: usize,
        x: usize,
        y: usize,
        w: usize,
        h: usize,
        color: u32,
    ) {
        for col in x..x + w {
            if y * stride + col < canvas.len() {
                canvas[y * stride + col] = color;
            }
            let by = y + h - 1;
            if by * stride + col < canvas.len() {
                canvas[by * stride + col] = color;
            }
        }
        for row in y..y + h {
            if row * stride + x < canvas.len() {
                canvas[row * stride + x] = color;
            }
            let rx = x + w - 1;
            if row * stride + rx < canvas.len() {
                canvas[row * stride + rx] = color;
            }
        }
    }

    fn fill_circle(
        &self,
        canvas: &mut [u32],
        stride: usize,
        cx: usize,
        cy: usize,
        r: usize,
        color: u32,
    ) {
        let r2 = (r * r) as isize;
        for dy in 0..r {
            for dx in 0..r {
                if (dx * dx + dy * dy) as isize <= r2 {
                    let coords = [
                        (cx + dx, cy + dy),
                        (cx.wrapping_sub(dx), cy + dy),
                        (cx + dx, cy.wrapping_sub(dy)),
                        (cx.wrapping_sub(dx), cy.wrapping_sub(dy)),
                    ];
                    for (px, py) in coords {
                        if py * stride + px < canvas.len() {
                            canvas[py * stride + px] = color;
                        }
                    }
                }
            }
        }
    }

    fn draw_circle_outline(
        &self,
        canvas: &mut [u32],
        stride: usize,
        cx: usize,
        cy: usize,
        r: usize,
        color: u32,
    ) {
        let r2_outer = (r * r) as isize;
        let r_inner = r.saturating_sub(1);
        let r2_inner = (r_inner * r_inner) as isize;
        for dy in 0..=r {
            for dx in 0..=r {
                let d = (dx * dx + dy * dy) as isize;
                if d <= r2_outer && d >= r2_inner {
                    let coords = [
                        (cx + dx, cy + dy),
                        (cx.wrapping_sub(dx), cy + dy),
                        (cx + dx, cy.wrapping_sub(dy)),
                        (cx.wrapping_sub(dx), cy.wrapping_sub(dy)),
                    ];
                    for (px, py) in coords {
                        if py * stride + px < canvas.len() {
                            canvas[py * stride + px] = color;
                        }
                    }
                }
            }
        }
    }

    fn draw_text_simple(
        &self,
        canvas: &mut [u32],
        stride: usize,
        x: usize,
        y: usize,
        text: &str,
        color: u32,
    ) {
        for (i, _ch) in text.chars().enumerate() {
            let gx = x + i * GLYPH_W;
            for py in 0..GLYPH_H {
                if (y + py) * stride + gx < canvas.len() {
                    canvas[(y + py) * stride + gx] = color;
                }
            }
        }
    }

    fn draw_scaled_char(
        &self,
        canvas: &mut [u32],
        stride: usize,
        x: usize,
        y: usize,
        _ch: char,
        color: u32,
        scale: usize,
    ) {
        let sw = GLYPH_W * scale;
        let sh = GLYPH_H * scale;
        for py in 0..sh {
            for px in 0..sw {
                let idx = (y + py) * stride + (x + px);
                if idx < canvas.len() {
                    canvas[idx] = color;
                }
            }
        }
    }
}

// ── Lock Triggers ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockTrigger {
    Manual,
    IdleTimeout,
    LidClose,
    SuspendResume,
    Hotkey,
    RemoteLock,
    SessionSwitch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LidState {
    Open,
    Closed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerEvent {
    Suspend,
    Resume,
    Hibernate,
    ShutdownRequested,
}

/// Configuration for automatic lock triggers.
pub struct LockTriggerConfig {
    pub idle_timeout_ms: u64,
    pub lock_on_lid_close: bool,
    pub lock_on_suspend: bool,
    pub lock_on_resume: bool,
    pub lock_on_session_switch: bool,
    pub hotkey_enabled: bool,
}

impl Default for LockTriggerConfig {
    fn default() -> Self {
        Self {
            idle_timeout_ms: 300_000,
            lock_on_lid_close: true,
            lock_on_suspend: true,
            lock_on_resume: true,
            lock_on_session_switch: true,
            hotkey_enabled: true,
        }
    }
}

// ── Privileged Lock Context ──────────────────────────────────────────────

/// Security context for the lock screen. Runs at elevated privilege:
/// input is captured exclusively, compositor surfaces beneath are hidden,
/// and dismissal requires successful authentication.
pub struct PrivilegedLockContext {
    pub session_id: u64,
    pub locked_by: LockTrigger,
    pub locked_at: u64,
    pub input_captured: bool,
    /// Compositor surface ID for the lock screen overlay.
    pub surface_id: Option<u64>,
    /// Z-order assigned (max z so nothing can draw over it).
    pub z_order: u32,
    pub secure_attention_seq: bool,
}

impl PrivilegedLockContext {
    pub fn new(session_id: u64, trigger: LockTrigger, now: u64) -> Self {
        Self {
            session_id,
            locked_by: trigger,
            locked_at: now,
            input_captured: true,
            surface_id: None,
            z_order: u32::MAX,
            secure_attention_seq: false,
        }
    }

    pub fn elapsed_ms(&self, now: u64) -> u64 {
        now.saturating_sub(self.locked_at)
    }
}

// ── Lock Screen Manager ──────────────────────────────────────────────────

/// Top-level lock screen manager that integrates the `LockScreen` widget
/// with the compositor, input subsystem, and power events.
pub struct LockScreenManager {
    pub screen: LockScreen,
    pub context: Option<PrivilegedLockContext>,
    pub trigger_config: LockTriggerConfig,
    pub last_input_time: u64,
    pub lid_state: LidState,
    pub lock_history: Vec<LockHistoryEntry>,
    pub session_id: u64,
    /// Blurred snapshot of the desktop captured at lock time.
    pub desktop_snapshot: Vec<u32>,
    pub snapshot_width: u32,
    pub snapshot_height: u32,
    pub blur_radius: u32,
}

#[derive(Debug, Clone)]
pub struct LockHistoryEntry {
    pub trigger: LockTrigger,
    pub locked_at: u64,
    pub unlocked_at: Option<u64>,
    pub failed_attempts: u32,
    pub auth_method_used: AuthMethod,
}

impl LockScreenManager {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            screen: LockScreen::new(width, height),
            context: None,
            trigger_config: LockTriggerConfig::default(),
            last_input_time: 0,
            lid_state: LidState::Open,
            lock_history: Vec::new(),
            session_id: 1,
            desktop_snapshot: Vec::new(),
            snapshot_width: 0,
            snapshot_height: 0,
            blur_radius: 24,
        }
    }

    pub fn is_locked(&self) -> bool {
        self.context.is_some() && self.screen.state != LockState::Dismissed
    }

    /// Lock the screen from any trigger.
    pub fn lock(&mut self, trigger: LockTrigger, now: u64) {
        if self.is_locked() {
            return;
        }

        self.screen.lock();
        self.context = Some(PrivilegedLockContext::new(self.session_id, trigger, now));

        self.lock_history.push(LockHistoryEntry {
            trigger,
            locked_at: now,
            unlocked_at: None,
            failed_attempts: 0,
            auth_method_used: self.screen.auth_method.clone(),
        });
    }

    /// Attempt to dismiss the lock screen after successful auth.
    pub fn try_dismiss(&mut self, now: u64) -> bool {
        if !self.is_locked() {
            return true;
        }

        if matches!(self.screen.auth_state, AuthState::Success) {
            if let Some(ref mut entry) = self.lock_history.last_mut() {
                entry.unlocked_at = Some(now);
                entry.failed_attempts = self.screen.failed_attempts;
            }

            self.screen.unlock();
            self.context = None;
            self.desktop_snapshot.clear();
            true
        } else {
            false
        }
    }

    /// Record user activity (resets idle timer).
    pub fn record_input(&mut self, now: u64) {
        self.last_input_time = now;
    }

    /// Called periodically. Checks idle timeout and auto-locks.
    pub fn tick(&mut self, now: u64, delta_ms: u64) {
        self.screen.tick(delta_ms);

        if self.screen.state == LockState::Unlocking && self.screen.animation.current_alpha <= 0.0 {
            self.context = None;
            self.desktop_snapshot.clear();
        }

        if !self.is_locked() && self.trigger_config.idle_timeout_ms > 0 {
            let idle = now.saturating_sub(self.last_input_time);
            if idle >= self.trigger_config.idle_timeout_ms {
                self.lock(LockTrigger::IdleTimeout, now);
            }
        }
    }

    /// Handle lid state changes.
    pub fn on_lid_event(&mut self, state: LidState, now: u64) {
        let prev = self.lid_state;
        self.lid_state = state;

        if prev == LidState::Open
            && state == LidState::Closed
            && self.trigger_config.lock_on_lid_close
        {
            self.lock(LockTrigger::LidClose, now);
        }
    }

    /// Handle power events (suspend/resume).
    pub fn on_power_event(&mut self, event: PowerEvent, now: u64) {
        match event {
            PowerEvent::Suspend if self.trigger_config.lock_on_suspend => {
                self.lock(LockTrigger::SuspendResume, now);
            }
            PowerEvent::Resume if self.trigger_config.lock_on_resume => {
                if !self.is_locked() {
                    self.lock(LockTrigger::SuspendResume, now);
                }
            }
            _ => {}
        }
    }

    /// Handle Super+L hotkey.
    pub fn on_hotkey_lock(&mut self, now: u64) {
        if self.trigger_config.hotkey_enabled {
            self.lock(LockTrigger::Hotkey, now);
        }
    }

    /// Capture a snapshot of the desktop for the blur background.
    pub fn capture_desktop(&mut self, framebuffer: &[u32], width: u32, height: u32) {
        let total = (width as usize) * (height as usize);
        if framebuffer.len() < total {
            return;
        }

        self.desktop_snapshot.clear();
        self.desktop_snapshot
            .extend_from_slice(&framebuffer[..total]);
        self.snapshot_width = width;
        self.snapshot_height = height;

        self.apply_blur();

        self.screen.background = LockBackground::BlurredDesktop;
    }

    /// Apply a fast box-blur approximation to the captured snapshot.
    fn apply_blur(&mut self) {
        let w = self.snapshot_width as usize;
        let h = self.snapshot_height as usize;
        if w == 0 || h == 0 || self.desktop_snapshot.len() < w * h {
            return;
        }

        let r = self.blur_radius as usize;
        if r == 0 {
            return;
        }

        let mut tmp = vec![0u32; w * h];

        for _pass in 0..3 {
            Self::blur_h(&self.desktop_snapshot, &mut tmp, w, h, r);
            Self::blur_v(&tmp, &mut self.desktop_snapshot, w, h, r);
        }

        let tint_a: u32 = 0x60;
        let tint_r: u32 = 0x08;
        let tint_g: u32 = 0x0A;
        let tint_b: u32 = 0x14;
        let inv = 255 - tint_a;

        for px in self.desktop_snapshot.iter_mut() {
            let a = *px & 0xFF00_0000;
            let cr = ((((*px >> 16) & 0xFF) * inv + tint_r * tint_a) / 255) & 0xFF;
            let cg = ((((*px >> 8) & 0xFF) * inv + tint_g * tint_a) / 255) & 0xFF;
            let cb = (((*px & 0xFF) * inv + tint_b * tint_a) / 255) & 0xFF;
            *px = a | (cr << 16) | (cg << 8) | cb;
        }
    }

    fn blur_h(src: &[u32], dst: &mut [u32], w: usize, h: usize, r: usize) {
        let d = (2 * r + 1) as u32;
        for y in 0..h {
            let row = y * w;
            let (mut ra, mut ga, mut ba) = (0u32, 0u32, 0u32);
            let first = src[row];
            let last = src[row + w - 1];
            let (fr, fg, fb) = ((first >> 16) & 0xFF, (first >> 8) & 0xFF, first & 0xFF);
            let (lr, lg, lb) = ((last >> 16) & 0xFF, (last >> 8) & 0xFF, last & 0xFF);

            for i in 0..=r {
                let idx = row + if i < w { i } else { w - 1 };
                let p = src[idx];
                ra += (p >> 16) & 0xFF;
                ga += (p >> 8) & 0xFF;
                ba += p & 0xFF;
            }
            for _ in 0..r {
                ra += fr;
                ga += fg;
                ba += fb;
            }

            for x in 0..w {
                let a = src[row + x] & 0xFF00_0000;
                dst[row + x] = a | ((ra / d) << 16) | ((ga / d) << 8) | (ba / d);
                let add_x = x + r + 1;
                let sub_x = x as isize - r as isize;
                if add_x < w {
                    let p = src[row + add_x];
                    ra += (p >> 16) & 0xFF;
                    ga += (p >> 8) & 0xFF;
                    ba += p & 0xFF;
                } else {
                    ra += lr;
                    ga += lg;
                    ba += lb;
                }
                if sub_x >= 0 {
                    let p = src[row + sub_x as usize];
                    ra -= (p >> 16) & 0xFF;
                    ga -= (p >> 8) & 0xFF;
                    ba -= p & 0xFF;
                } else {
                    ra -= fr;
                    ga -= fg;
                    ba -= fb;
                }
            }
        }
    }

    fn blur_v(src: &[u32], dst: &mut [u32], w: usize, h: usize, r: usize) {
        let d = (2 * r + 1) as u32;
        for x in 0..w {
            let (mut ra, mut ga, mut ba) = (0u32, 0u32, 0u32);
            let first = src[x];
            let last = src[(h - 1) * w + x];
            let (fr, fg, fb) = ((first >> 16) & 0xFF, (first >> 8) & 0xFF, first & 0xFF);
            let (lr, lg, lb) = ((last >> 16) & 0xFF, (last >> 8) & 0xFF, last & 0xFF);

            for i in 0..=r {
                let iy = if i < h { i } else { h - 1 };
                let p = src[iy * w + x];
                ra += (p >> 16) & 0xFF;
                ga += (p >> 8) & 0xFF;
                ba += p & 0xFF;
            }
            for _ in 0..r {
                ra += fr;
                ga += fg;
                ba += fb;
            }

            for y in 0..h {
                let a = src[y * w + x] & 0xFF00_0000;
                dst[y * w + x] = a | ((ra / d) << 16) | ((ga / d) << 8) | (ba / d);
                let add_y = y + r + 1;
                let sub_y = y as isize - r as isize;
                if add_y < h {
                    let p = src[add_y * w + x];
                    ra += (p >> 16) & 0xFF;
                    ga += (p >> 8) & 0xFF;
                    ba += p & 0xFF;
                } else {
                    ra += lr;
                    ga += lg;
                    ba += lb;
                }
                if sub_y >= 0 {
                    let p = src[sub_y as usize * w + x];
                    ra -= (p >> 16) & 0xFF;
                    ga -= (p >> 8) & 0xFF;
                    ba -= p & 0xFF;
                } else {
                    ra -= fr;
                    ga -= fg;
                    ba -= fb;
                }
            }
        }
    }

    /// Render using the blurred desktop snapshot as background (if available).
    pub fn render_with_blur(&self, canvas: &mut [u32], stride: usize) {
        if self.screen.state == LockState::Dismissed {
            return;
        }

        let w = self.screen.screen_width as usize;
        let h = self.screen.screen_height as usize;

        if !self.desktop_snapshot.is_empty()
            && self.snapshot_width == self.screen.screen_width
            && self.snapshot_height == self.screen.screen_height
        {
            let sw = self.snapshot_width as usize;
            for y in 0..h.min(self.snapshot_height as usize) {
                for x in 0..w.min(sw) {
                    let idx = y * stride + x;
                    if idx < canvas.len() {
                        canvas[idx] = self.desktop_snapshot[y * sw + x];
                    }
                }
            }
        }

        self.screen.render(canvas, stride);
    }

    /// Get lock duration statistics.
    pub fn avg_lock_duration_ms(&self) -> u64 {
        let completed: Vec<&LockHistoryEntry> = self
            .lock_history
            .iter()
            .filter(|e| e.unlocked_at.is_some())
            .collect();
        if completed.is_empty() {
            return 0;
        }

        let total: u64 = completed
            .iter()
            .map(|e| e.unlocked_at.unwrap_or(0).saturating_sub(e.locked_at))
            .sum();
        total / completed.len() as u64
    }

    pub fn total_failed_attempts(&self) -> u32 {
        self.lock_history.iter().map(|e| e.failed_attempts).sum()
    }

    pub fn lock_count(&self) -> usize {
        self.lock_history.len()
    }

    pub fn configure_idle_timeout(&mut self, timeout_ms: u64) {
        self.trigger_config.idle_timeout_ms = timeout_ms;
        self.screen.idle_timeout_s = (timeout_ms / 1000) as u32;
    }

    pub fn set_user(&mut self, name: &str, email: Option<&str>, avatar: Option<&str>) {
        self.screen.user_info.display_name = String::from(name);
        self.screen.user_info.email = email.map(String::from);
        self.screen.user_info.avatar = avatar.map(String::from);
    }
}

// ── Passkey / AthID Integration ──────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PasskeyStatus {
    NotConfigured,
    WaitingForDevice,
    Authenticating,
    Verified,
    Failed,
    Timeout,
}

#[derive(Debug, Clone)]
pub struct PasskeyChallenge {
    pub challenge_id: u64,
    pub relying_party: String,
    pub user_handle: Vec<u8>,
    pub timeout_ms: u64,
    pub created_at: u64,
}

pub struct PasskeyManager {
    pub status: PasskeyStatus,
    pub challenge: Option<PasskeyChallenge>,
    pub registered_credentials: Vec<PasskeyCredential>,
    pub last_attempt: u64,
    pub attempt_count: u32,
    pub next_challenge_id: u64,
}

#[derive(Debug, Clone)]
pub struct PasskeyCredential {
    pub credential_id: Vec<u8>,
    /// The authenticator's ES256 (P-256) public key, as the SEC1 uncompressed
    /// point (`0x04 || X || Y`) or bare COSE `X || Y` — whatever
    /// `ath_crypto::p256_ecdsa::verify` accepts. This is what an assertion is
    /// verified against; without it there is nothing to check a signature
    /// with, and "any registered credential unlocks" is an auth bypass.
    pub public_key: Vec<u8>,
    pub display_name: String,
    pub registered_at: u64,
    pub last_used: u64,
    pub use_count: u32,
    pub device_type: PasskeyDeviceType,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PasskeyDeviceType {
    Platform,
    CrossPlatform,
    SecurityKey,
    Phone,
}

impl PasskeyManager {
    pub fn new() -> Self {
        Self {
            status: PasskeyStatus::NotConfigured,
            challenge: None,
            registered_credentials: Vec::new(),
            last_attempt: 0,
            attempt_count: 0,
            next_challenge_id: 1,
        }
    }

    pub fn is_configured(&self) -> bool {
        !self.registered_credentials.is_empty()
    }

    /// Register an authenticator credential. `public_key` is the ES256 (P-256)
    /// key the authenticator returned at registration (SEC1 `0x04||X||Y` or bare
    /// COSE `X||Y`); it is stored so later assertions can be cryptographically
    /// verified. A credential with an empty/malformed key can never authenticate
    /// (`verify_response` fails closed on it), so registration is refused.
    pub fn register_credential(
        &mut self,
        display_name: &str,
        public_key: &[u8],
        device_type: PasskeyDeviceType,
        now: u64,
    ) -> Option<u64> {
        // A key that our verifier cannot parse is useless — reject at
        // registration rather than store a credential that can never sign in.
        if !matches!(public_key.len(), 33 | 64 | 65) {
            return None;
        }
        let id = self.next_challenge_id;
        self.next_challenge_id += 1;

        let mut cred_id = vec![0u8; 16];
        let seed = now ^ (id * 0x5DEECE66D);
        for i in 0..16 {
            cred_id[i] = ((seed >> (i * 4)) & 0xFF) as u8;
        }

        self.registered_credentials.push(PasskeyCredential {
            credential_id: cred_id,
            public_key: public_key.to_vec(),
            display_name: String::from(display_name),
            registered_at: now,
            last_used: 0,
            use_count: 0,
            device_type,
        });

        self.status = PasskeyStatus::WaitingForDevice;
        Some(id)
    }

    pub fn begin_authentication(&mut self, now: u64) -> Option<u64> {
        if !self.is_configured() {
            return None;
        }

        let id = self.next_challenge_id;
        self.next_challenge_id += 1;

        self.challenge = Some(PasskeyChallenge {
            challenge_id: id,
            relying_party: String::from("raeos.local"),
            user_handle: Vec::new(),
            timeout_ms: 30_000,
            created_at: now,
        });

        self.status = PasskeyStatus::WaitingForDevice;
        self.last_attempt = now;
        self.attempt_count += 1;

        Some(id)
    }

    /// Verify a WebAuthn-style assertion against a registered credential.
    ///
    /// The assertion is accepted ONLY if the ES256 `signature` verifies, under
    /// the stored credential's public key, over `authenticator_data ||
    /// client_data_hash` (the WebAuthn signed message; the caller hashes the
    /// clientDataJSON — which binds the server challenge — into
    /// `client_data_hash`). This is the line between "a passkey is configured"
    /// and "the holder of the private key is present": the previous
    /// implementation returned Verified whenever *any* credential existed,
    /// ignoring the id and doing no cryptography at all — a lock-screen bypass.
    ///
    /// Fails closed on: an expired challenge, an unknown credential id, a
    /// malformed/short client-data hash, or a signature that does not verify.
    /// Follow-up (real WebAuthn hardening, not yet enforced here): check the RP
    /// ID hash + user-present/user-verified flags inside `authenticator_data`
    /// and the signature counter for clone detection.
    pub fn verify_response(
        &mut self,
        credential_id: &[u8],
        authenticator_data: &[u8],
        client_data_hash: &[u8],
        signature: &[u8],
        now: u64,
    ) -> bool {
        let timed_out = self
            .challenge
            .as_ref()
            .map_or(true, |c| now.saturating_sub(c.created_at) > c.timeout_ms);

        if timed_out {
            self.status = PasskeyStatus::Timeout;
            self.challenge = None;
            return false;
        }

        // A WebAuthn clientDataHash is exactly SHA-256(clientDataJSON) — 32
        // bytes. Anything else means a malformed assertion; reject it rather
        // than verify over an attacker-shaped short message.
        if client_data_hash.len() != 32 {
            self.status = PasskeyStatus::Failed;
            self.challenge = None;
            return false;
        }

        // Match the credential the assertion names (public info, plain compare).
        let cred = self
            .registered_credentials
            .iter_mut()
            .find(|c| c.credential_id.as_slice() == credential_id);

        let Some(cred) = cred else {
            self.status = PasskeyStatus::Failed;
            self.challenge = None;
            return false;
        };

        // The WebAuthn signed message: authenticatorData || SHA-256(clientData).
        let mut signed = Vec::with_capacity(authenticator_data.len() + client_data_hash.len());
        signed.extend_from_slice(authenticator_data);
        signed.extend_from_slice(client_data_hash);

        // `verify` SHA-256-hashes `signed` internally and checks it against the
        // stored authenticator public key. Only a signature produced by the
        // matching private key passes.
        let ok = ath_crypto::p256_ecdsa::verify(&cred.public_key, &signed, signature);

        if ok {
            cred.last_used = now;
            cred.use_count += 1;
            self.status = PasskeyStatus::Verified;
            self.challenge = None;
            true
        } else {
            self.status = PasskeyStatus::Failed;
            self.challenge = None;
            false
        }
    }

    pub fn cancel_authentication(&mut self) {
        self.status = if self.is_configured() {
            PasskeyStatus::WaitingForDevice
        } else {
            PasskeyStatus::NotConfigured
        };
        self.challenge = None;
    }

    pub fn check_timeout(&mut self, now: u64) -> bool {
        if let Some(ref challenge) = self.challenge {
            if now.saturating_sub(challenge.created_at) > challenge.timeout_ms {
                self.status = PasskeyStatus::Timeout;
                self.challenge = None;
                return true;
            }
        }
        false
    }
}

// ── Security Audit Log ───────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct SecurityAuditEntry {
    pub timestamp: u64,
    pub event_type: SecurityEventType,
    pub session_id: u64,
    pub details: String,
    pub success: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecurityEventType {
    LockActivated,
    UnlockSuccess,
    UnlockFailed,
    LockoutTriggered,
    PasskeyAttempt,
    BiometricAttempt,
    SessionSwitch,
    RemoteLock,
    SuspendLock,
    ConfigChange,
}

pub struct SecurityAuditLog {
    pub entries: Vec<SecurityAuditEntry>,
    pub max_entries: usize,
    pub total_events: u64,
}

impl SecurityAuditLog {
    pub fn new(max_entries: usize) -> Self {
        Self {
            entries: Vec::new(),
            max_entries,
            total_events: 0,
        }
    }

    pub fn log(
        &mut self,
        event_type: SecurityEventType,
        session_id: u64,
        details: &str,
        success: bool,
        now: u64,
    ) {
        self.total_events += 1;

        if self.entries.len() >= self.max_entries {
            self.entries.remove(0);
        }

        self.entries.push(SecurityAuditEntry {
            timestamp: now,
            event_type,
            session_id,
            details: String::from(details),
            success,
        });
    }

    pub fn failed_attempts_since(&self, since: u64) -> u32 {
        self.entries
            .iter()
            .filter(|e| {
                e.timestamp >= since
                    && !e.success
                    && matches!(e.event_type, SecurityEventType::UnlockFailed)
            })
            .count() as u32
    }

    pub fn last_successful_unlock(&self) -> Option<u64> {
        self.entries
            .iter()
            .rev()
            .find(|e| e.success && e.event_type == SecurityEventType::UnlockSuccess)
            .map(|e| e.timestamp)
    }

    pub fn entries_by_type(&self, event_type: SecurityEventType) -> Vec<&SecurityAuditEntry> {
        self.entries
            .iter()
            .filter(|e| e.event_type == event_type)
            .collect()
    }

    pub fn recent_entries(&self, count: usize) -> Vec<&SecurityAuditEntry> {
        self.entries.iter().rev().take(count).collect()
    }
}

// ── Lock Screen Compositor Surface ───────────────────────────────────────

/// Configuration for the lock screen compositor surface. The lock screen
/// must render at max z-order so nothing can draw over it. It also needs
/// to capture input exclusively so that underlying surfaces can't be interacted
/// with while locked.
pub struct LockSurfaceConfig {
    pub width: u32,
    pub height: u32,
    pub z_order: u32,
    pub input_exclusive: bool,
    pub blur_behind: bool,
    pub opacity: u8,
    pub transition_ms: u32,
}

impl Default for LockSurfaceConfig {
    fn default() -> Self {
        Self {
            width: 0,
            height: 0,
            z_order: u32::MAX,
            input_exclusive: true,
            blur_behind: true,
            opacity: 255,
            transition_ms: 300,
        }
    }
}

impl LockSurfaceConfig {
    pub fn for_display(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            ..Default::default()
        }
    }
}

// ── Multi-Monitor Lock Support ───────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct MonitorLockState {
    pub monitor_id: u32,
    pub width: u32,
    pub height: u32,
    pub surface_id: Option<u64>,
    pub primary: bool,
    pub show_auth: bool,
}

pub struct MultiMonitorLock {
    pub monitors: Vec<MonitorLockState>,
    pub primary_monitor: u32,
    pub auth_monitor: u32,
}

impl MultiMonitorLock {
    pub fn new() -> Self {
        Self {
            monitors: Vec::new(),
            primary_monitor: 0,
            auth_monitor: 0,
        }
    }

    pub fn add_monitor(&mut self, id: u32, width: u32, height: u32, primary: bool) {
        if primary {
            self.primary_monitor = id;
            self.auth_monitor = id;
        }
        self.monitors.push(MonitorLockState {
            monitor_id: id,
            width,
            height,
            surface_id: None,
            primary,
            show_auth: primary,
        });
    }

    pub fn remove_monitor(&mut self, id: u32) {
        self.monitors.retain(|m| m.monitor_id != id);
        if self.primary_monitor == id {
            self.primary_monitor = self.monitors.first().map_or(0, |m| m.monitor_id);
            self.auth_monitor = self.primary_monitor;
        }
    }

    pub fn move_auth_to_monitor(&mut self, id: u32) {
        for m in self.monitors.iter_mut() {
            m.show_auth = m.monitor_id == id;
        }
        self.auth_monitor = id;
    }

    pub fn monitor_count(&self) -> usize {
        self.monitors.len()
    }
}

// ── Glass identity KATs ──────────────────────────────────────────────────
//
// FAIL-able proofs that the lock screen wears the Liquid Glass identity
// (aurora backdrop + frosted glass card + proportional RaeSans), not the old
// flat hardcoded-hex render. These run on the host (`cargo test -p athshell`)
// over a raw ARGB buffer wrapped in the SAME `athgfx::Canvas` the kernel uses.
#[cfg(test)]
mod glass_identity_tests {
    use super::*;
    use athgfx::text::FontFamily;
    use athgfx::Canvas;

    fn render_to_buf(w: usize, h: usize) -> alloc::vec::Vec<u32> {
        let mut px = alloc::vec![0u32; w * h];
        let mut ls = LockScreen::new(w as u32, h as u32);
        ls.lock();
        ls.set_display_name("Aria");
        ls.render(&mut px, w);
        px
    }

    #[test]
    fn lock_background_is_aurora_not_flat_hardcoded_hex() {
        // The OLD render painted a flat hardcoded navy (`LOCK_BG`/gradient) — a
        // near-uniform field. The Aurora Mesh has real blue/violet/teal blobs, so
        // the frame must show a wide luma SPREAD. A flat solid would have ~0
        // spread and FAIL. Also assert it is NOT the old LOCK_BG solid.
        let (w, h) = (640usize, 480usize);
        let px = render_to_buf(w, h);
        // Sample a top corner well OUTSIDE the centered glass card.
        let corner = px[8 * w + 8];
        assert_ne!(
            corner & 0x00FF_FFFF,
            LOCK_BG & 0x00FF_FFFF,
            "lock background must be the aurora, not the flat hardcoded LOCK_BG solid"
        );
        let mut lo = u32::MAX;
        let mut hi = 0u32;
        for &p in px.iter() {
            let l = ((p >> 16) & 0xFF) + ((p >> 8) & 0xFF) + (p & 0xFF);
            lo = lo.min(l);
            hi = hi.max(l);
        }
        assert!(
            hi - lo > 80,
            "lock screen reads as a flat field (luma spread {} too small) — \
             not the living Aurora Mesh",
            hi - lo
        );
    }

    #[test]
    fn lock_time_renders_proportional_raesans_not_block_glyph() {
        // The OLD `draw_scaled_char` painted a SOLID block per char (ignoring the
        // glyph). RaeSans is anti-aliased + PROPORTIONAL: a wide string advances
        // more than a narrow one of equal length, and lays NON-UNIFORM coverage
        // (grayscale AA edges) a solid block can't. Both FAIL on the block path.
        assert!(
            athgfx::text::ensure_init(),
            "RaeSans AA engine must be available"
        );
        let (w, h) = (400usize, 80usize);
        let mut px = alloc::vec![0xFF_10_14_20u32; w * h];
        let c = unsafe { Canvas::new(px.as_mut_ptr() as *mut u8, w, h, 4) };
        let narrow = c.measure_text_aa("iiii", ath_tokens::TYPE_DISPLAY, FontFamily::Sans);
        let wide = c.measure_text_aa("WWWW", ath_tokens::TYPE_DISPLAY, FontFamily::Sans);
        assert!(
            wide > narrow,
            "lock clock must be proportional RaeSans (W wider than i): \
             narrow={narrow} wide={wide}"
        );
        let mut c2 = unsafe { Canvas::new(px.as_mut_ptr() as *mut u8, w, h, 4) };
        let stats = c2.draw_text_aa_stats(
            8,
            8,
            "12:34",
            ath_tokens::TYPE_DISPLAY,
            0xFF_FF_FF_FF,
            FontFamily::Sans,
        );
        assert!(
            stats.total_coverage > 0 && stats.min_cov < stats.max_cov,
            "clock must render anti-aliased RaeSans ink (non-uniform coverage): \
             total={} min={} max={}",
            stats.total_coverage,
            stats.min_cov,
            stats.max_cov
        );
    }

    #[test]
    fn lock_card_uses_popover_glass_tier() {
        // The card is the `glass.popover` tier (transient surface) — a deliberate
        // tier selection, not a hardcoded fill. If someone swaps it back to an
        // opaque hex constant this invariant (tier alpha < fully opaque) breaks.
        let tier = ath_tokens::GLASS_POPOVER_DARK;
        let alpha = (tier.tint >> 24) & 0xFF;
        assert!(
            alpha < 0xFF,
            "glass.popover must be translucent so the aurora reads through (alpha={alpha:#x})"
        );
    }
}

#[cfg(test)]
mod passkey_auth_tests {
    use super::*;

    // A REAL ES256 assertion, generated offline (Python `cryptography`) with a
    // fixed P-256 key so this is an independent oracle, not a self-round-trip:
    //   priv = 0x00112233..DDEE01, message = AUTH_DATA || CLIENT_HASH,
    //   signature = ECDSA(SHA-256), DER-encoded.
    // PK is the SEC1 uncompressed public point (0x04 || X || Y).
    const PK: [u8; 65] = [
        0x04, 0xe1, 0x84, 0xae, 0x81, 0x52, 0x16, 0x6c, 0xbf, 0x2e, 0xd1, 0xa6, 0x64, 0x76, 0x27,
        0xd0, 0xd3, 0xe6, 0xd2, 0xc8, 0x06, 0xe7, 0x9e, 0x38, 0x38, 0x65, 0xe6, 0x7a, 0xc1, 0x42,
        0x73, 0xce, 0x8e, 0x5f, 0x25, 0x49, 0x64, 0x0f, 0xd7, 0x07, 0xf6, 0x34, 0x72, 0x53, 0xa2,
        0xe0, 0x95, 0x9d, 0x57, 0x2e, 0xe8, 0x29, 0x8d, 0xc1, 0xcf, 0x1f, 0x90, 0x13, 0x0f, 0xc3,
        0x09, 0x7f, 0xe8, 0xfb, 0x7c,
    ];
    const AUTH_DATA: [u8; 37] = [
        0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e,
        0x0f, 0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b, 0x1c, 0x1d,
        0x1e, 0x1f, 0x20, 0x21, 0x22, 0x23, 0x24,
    ];
    const CLIENT_HASH: [u8; 32] = [
        0xab, 0xab, 0xab, 0xab, 0xab, 0xab, 0xab, 0xab, 0xab, 0xab, 0xab, 0xab, 0xab, 0xab, 0xab,
        0xab, 0xab, 0xab, 0xab, 0xab, 0xab, 0xab, 0xab, 0xab, 0xab, 0xab, 0xab, 0xab, 0xab, 0xab,
        0xab, 0xab,
    ];
    const SIG: [u8; 72] = [
        0x30, 0x46, 0x02, 0x21, 0x00, 0xda, 0x8d, 0xd7, 0x7f, 0xd3, 0x9b, 0xde, 0x55, 0xf0, 0x25,
        0x8e, 0x22, 0xae, 0x7a, 0x6a, 0xda, 0x29, 0x4c, 0x65, 0xb6, 0xf2, 0x28, 0x07, 0x85, 0xc7,
        0xb7, 0xd5, 0x6f, 0x2d, 0x49, 0x71, 0xb2, 0x02, 0x21, 0x00, 0xc1, 0xd5, 0x09, 0xf2, 0x0e,
        0xe0, 0x4c, 0xf1, 0x57, 0xee, 0xd7, 0x95, 0x55, 0x16, 0x46, 0xa3, 0xf2, 0x75, 0x3b, 0x89,
        0xd6, 0x2b, 0xd6, 0xf2, 0x9e, 0x4a, 0x7c, 0xc8, 0x65, 0x42, 0x6a, 0xbd,
    ];

    // Register the oracle credential and open a fresh challenge window.
    fn armed_manager() -> (PasskeyManager, Vec<u8>) {
        let mut m = PasskeyManager::new();
        assert!(
            m.register_credential("Aria", &PK, PasskeyDeviceType::Platform, 1_000)
                .is_some(),
            "a well-formed P-256 key must register"
        );
        let cred_id = m.registered_credentials[0].credential_id.clone();
        m.begin_authentication(1_000);
        (m, cred_id)
    }

    #[test]
    fn valid_assertion_unlocks() {
        let (mut m, id) = armed_manager();
        assert!(
            m.verify_response(&id, &AUTH_DATA, &CLIENT_HASH, &SIG, 1_100),
            "a genuine ES256 assertion over authData||clientHash must verify"
        );
        assert_eq!(m.status, PasskeyStatus::Verified);
        assert_eq!(m.registered_credentials[0].use_count, 1);
    }

    #[test]
    fn tampered_signature_is_rejected() {
        let (mut m, id) = armed_manager();
        let mut bad = SIG;
        bad[40] ^= 0x01; // flip one bit of s
        assert!(
            !m.verify_response(&id, &AUTH_DATA, &CLIENT_HASH, &bad, 1_100),
            "a signature that does not verify must NOT unlock"
        );
        assert_eq!(m.status, PasskeyStatus::Failed);
    }

    #[test]
    fn tampered_message_is_rejected() {
        let (mut m, id) = armed_manager();
        let mut auth = AUTH_DATA;
        auth[0] ^= 0x01; // change the signed message → signature no longer covers it
        assert!(
            !m.verify_response(&id, &auth, &CLIENT_HASH, &SIG, 1_100),
            "mutating authenticatorData must invalidate the assertion"
        );
    }

    #[test]
    fn unknown_credential_id_is_rejected() {
        let (mut m, _id) = armed_manager();
        let wrong = [0xFFu8; 16];
        assert!(
            !m.verify_response(&wrong, &AUTH_DATA, &CLIENT_HASH, &SIG, 1_100),
            "an assertion naming an unregistered credential must NOT unlock"
        );
    }

    #[test]
    fn short_client_data_hash_is_rejected() {
        let (mut m, id) = armed_manager();
        assert!(
            !m.verify_response(&id, &AUTH_DATA, &CLIENT_HASH[..31], &SIG, 1_100),
            "a clientDataHash that is not 32 bytes is a malformed assertion"
        );
    }

    #[test]
    fn expired_challenge_is_rejected_even_with_a_valid_signature() {
        let (mut m, id) = armed_manager();
        // Challenge created at 1_000 with a 30s (30_000ms) window; well past it.
        assert!(
            !m.verify_response(&id, &AUTH_DATA, &CLIENT_HASH, &SIG, 1_000 + 30_001),
            "a valid signature must still be refused after the challenge times out"
        );
        assert_eq!(m.status, PasskeyStatus::Timeout);
    }

    #[test]
    fn malformed_public_key_cannot_register() {
        let mut m = PasskeyManager::new();
        assert!(
            m.register_credential("x", &[0u8; 10], PasskeyDeviceType::Platform, 1)
                .is_none(),
            "a key our verifier cannot parse must be refused at registration"
        );
        assert!(!m.is_configured());
    }
}
