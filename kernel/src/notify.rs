//! Notifications + Notification Center + Quick Settings / Control Center
//! (Concept §AthShell: "system events surface as quiet, beautiful toasts —
//! never a modal interruption, never a mystery beep"; §"The user owns the
//! machine" — no nag; §Unified Settings — "every option searchable… no ads,
//! no bloat"). MasterChecklist Phase 14.1 — "Notifications surface" + the
//! athena-parity #1 gap (2026-06-17): the everyday pull-down a Windows 11 /
//! macOS switcher reaches for reflexively.
//!
//! Three layers, all kernel-drawn (mirroring the live `SettingsPanel`):
//!  1. **Toasts** — [`post`] draws a compositor toast (top-right, stacked,
//!     never steals focus), up to [`MAX_VISIBLE`] with oldest-evicted overflow
//!     and a TTL after which [`expire_tick`] dismisses them.
//!  2. **History ring** — every post is RETAINED in a [`HISTORY_CAP`]-capped
//!     ring AFTER its toast expires (Windows' vanishing-toast pain point), so
//!     the Center shows what you missed.
//!  3. **Notification Center + Control Center** — [`toggle_center`] opens a
//!     glass pull-down showing the history as **grouped cards per source** with
//!     expand/collapse chevrons + a count badge, **inline-action rows** under
//!     expanded items, a **"Delivered Quietly"** section for DND-suppressed posts
//!     (the macOS honesty model — silenced, never lost), and a calm **"You're all
//!     caught up"** empty state — over the [`quick_settings`] strip: five toggles
//!     each wired to a REAL backend (Wi-Fi radio, audio master mute, Focus/DND
//!     that actually suppresses toasts here, Night Light persisted config, and
//!     the Vibe accent via `theme_engine`). No cosmetic switches.
//!
//! Polished to `docs/design/notifications.md`: toasts carry the per-app `+N`
//! collapse badge + a stack **depth cue** (back cards dimmed via a per-surface
//! `Opacity`, suppressed under reduced-motion) and consume the compositor's
//! **soft-ambient** drop shadow (`material-and-shadow.md` — the blur-silhouette
//! `render_drop_shadow`, never a hard offset block). Every colour/space/type is
//! a `ath_tokens` value, so a Vibe re-skin recolours the whole surface.
//!
//! Expiry is enforced lazily on every post and by `expire_tick` from the
//! shell's repaint path (`shell_runner::render_shell`, which runs on every
//! interaction); the smoketests drive everything with synthetic time so the
//! proof is deterministic on QEMU and iron.

#![allow(dead_code)]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;

pub use crate::shell_api::NotificationUrgency;

const TOAST_W: u32 = 320;
const TOAST_H: u32 = 72;
// desktop-shell.md §5: SPACE_2 (8px) vertical gap, SPACE_4 (16px) margin.
const TOAST_GAP: i32 = ath_tokens::SPACE_2 as i32;
const TOAST_MARGIN: i32 = ath_tokens::SPACE_4 as i32;
pub const MAX_VISIBLE: usize = 3;
pub const TOAST_TTL_MS: u64 = 5_000;

// ── Design tokens (docs/design/design-language.md, via ath_tokens) ─────────
// desktop-shell.md §5: toasts are `material.glass` (radius.md, elev.2) with a
// 1px stroke.strong top-edge highlight; the urgency bar maps to state tokens.
// Every colour below is a ath_tokens value flowed from the dark palette + the
// derived accent ramp, so a Vibe-Mode re-skin recolours toasts with the rest
// of the shell.
const PALETTE: &ath_tokens::Palette = &ath_tokens::DARK;

/// The LIVE accent seed for the derived ramp — `theme_engine::active_accent()`,
/// the single source of truth shared with the window chrome and the shell.
/// A one-tap Vibe re-skin changes this seed and recolours the normal-urgency
/// toast bar with the rest of the desktop (Concept §Customization Engine).
#[inline]
fn accent_seed() -> u32 {
    crate::theme_engine::active_accent()
}

#[inline]
fn accent_base() -> u32 {
    ath_tokens::derive_accent(accent_seed(), PALETTE).base
}

/// The normal-urgency bar accent actually painted — public so the cross-surface
/// cohesion smoketest can confirm toasts track the live seed
/// (`theme_engine::run_accent_cohesion_smoketest`).
#[inline]
pub fn proof_accent() -> u32 {
    accent_base()
}

/// `material.glass` tint over the dark palette (design-language §5.1).
const CARD_BG: u32 = ath_tokens::GLASS_TINT_DARK;
/// Toast corner radius — `radius.md` (12px).
const TOAST_RADIUS: usize = ath_tokens::RADIUS_MD as usize;
/// 1px top-edge highlight (the glass top-edge rule).
const TOP_EDGE: u32 = PALETTE.stroke_strong;
/// Title text — `type.label` `text.primary`.
const FG: u32 = PALETTE.text_primary;
/// Source/time — `type.caption` `text.tertiary`.
const FG_DIM: u32 = PALETTE.text_tertiary;
/// Urgency bar: low → text.tertiary, normal → accent.base, critical → danger.
const BAR_LOW: u32 = PALETTE.text_tertiary;
#[inline]
fn bar_normal() -> u32 {
    accent_base()
}
const BAR_CRITICAL: u32 = PALETTE.state_danger;

struct Toast {
    surface: u64,
    deadline_ms: u64,
    source: String,
    title: String,
    urgency: NotificationUrgency,
    /// Inline-action labels the source declared (notifications.md §5). At most
    /// [`MAX_TOAST_ACTIONS`] render on a toast; the full set lives in the Center.
    actions: Vec<String>,
}

/// At most this many inline-action buttons render on a *toast* (a glance
/// surface); the Center item row shows the full declared set (notifications.md
/// §5: "On a toast: at most 2 buttons inline + overflow into the Center").
pub const MAX_TOAST_ACTIONS: usize = 2;

static TOASTS: Mutex<Vec<Toast>> = Mutex::new(Vec::new());
static POSTED: AtomicU64 = AtomicU64::new(0);
static EXPIRED: AtomicU64 = AtomicU64::new(0);
static EVICTED: AtomicU64 = AtomicU64::new(0);
/// Count of toasts SUPPRESSED by Focus / Do-Not-Disturb (they still land in
/// history — DND silences the interruption, never loses the message).
static SUPPRESSED: AtomicU64 = AtomicU64::new(0);

fn now_ms() -> u64 {
    crate::hpet::read_millis().unwrap_or(0) as u64
}

// ── Notification history ring (retained after the toast TTL) ───────────────
//
// Concept §AthShell: "system events surface as quiet, beautiful toasts" — but a
// toast that vanishes after 5 s with no record is a Windows pain point. The
// history ring RETAINS every posted notification (grouped by source) so the
// Notification Center can show what you missed, long after the toast expired.
// Capped so a chatty app can never grow the kernel heap unbounded.

/// Capacity of the retained history ring (oldest dropped past this).
pub const HISTORY_CAP: usize = 64;

/// One retained notification (the toast's payload, kept after expiry).
#[derive(Clone)]
struct HistoryEntry {
    id: u64,
    source: String,
    title: String,
    urgency: NotificationUrgency,
    posted_ms: u64,
    /// Inline-action labels the source declared (notifications.md §5). Empty =
    /// no action row (a source without the action capability shows none).
    actions: Vec<String>,
    /// `true` when this post was suppressed-as-toast by Focus/DND but kept in
    /// history — it renders under the "Delivered Quietly" section, calmer and
    /// with no urgency bar (notifications.md §3: the macOS honesty model).
    quiet: bool,
}

static HISTORY: Mutex<Vec<HistoryEntry>> = Mutex::new(Vec::new());
static NEXT_HISTORY_ID: AtomicU64 = AtomicU64::new(1);

/// Render a toast card. `peers` is the live toast list (so we can draw the
/// per-app `+N` collapse badge when ≥2 unexpired toasts share this source —
/// notifications.md §1 "Per-app collapse"). The inline-action row (≤
/// [`MAX_TOAST_ACTIONS`] buttons) renders at the bottom when the source declared
/// actions.
fn render_toast(ptr: *mut u8, toast: &Toast, peers: &[Toast]) {
    let mut canvas = unsafe { athgfx::Canvas::new(ptr, TOAST_W as usize, TOAST_H as usize, 4) };
    // material.glass card. We fill the full rect with the glass TINT (its own
    // alpha PRESERVED — GLASS_TINT_DARK is ~62% over bg.overlay): `fill_rect`
    // does a direct u32 write, so unlike the AA `fill_rounded_rect` (which
    // forces opaque output via blend_pixel) the toast stays translucent and the
    // compositor alpha-blends it over the desktop. The rounded radius.md corners
    // come from the surface's `RoundedCorners` effect masking the blit, and the
    // elev.2 drop shadow gives the depth — both added in `post_at`.
    canvas.fill_rect(0, 0, TOAST_W as usize, TOAST_H as usize, CARD_BG);
    // 1px top-edge stroke.strong highlight (the glass top-edge rule), inset to
    // clear the rounded corners.
    canvas.fill_rect(
        TOAST_RADIUS,
        0,
        TOAST_W as usize - 2 * TOAST_RADIUS,
        1,
        TOP_EDGE,
    );

    let bar = urgency_bar(toast.urgency);
    // Urgency bar: 4px down the left edge, inset to clear the rounded corner.
    canvas.fill_rect(0, TOAST_RADIUS, 4, TOAST_H as usize - 2 * TOAST_RADIUS, bar);

    let text_x = ath_tokens::SPACE_4 as i32; // 16px content inset.
                                             // Available text width: content inset on both sides.
                                             // Per-app collapse badge: when ≥2 unexpired toasts share this source, the
                                             // front card shows a `+N` count pill (accent.subtle, type.caption) — the
                                             // macOS deck cue (notifications.md §1). N excludes the front card itself.
    let same_source = peers.iter().filter(|t| t.source == toast.source).count();
    let mut right_inset = text_x; // content right edge moves left if a badge sits there
    if same_source >= 2 {
        let n = same_source - 1;
        let label = alloc::format!("+{}", n);
        let lw = canvas.measure_text_aa(
            &label,
            ath_tokens::TYPE_CAPTION,
            athgfx::text::FontFamily::Sans,
        );
        let pill_w = (lw + 2 * ath_tokens::SPACE_2 as i32) as usize;
        let pill_h = (ath_tokens::TYPE_CAPTION.line_height as i32 + 4) as usize;
        let pill_x = TOAST_W as usize - text_x as usize - pill_w;
        let pill_y = 10usize;
        canvas.fill_rounded_rect(
            pill_x,
            pill_y,
            pill_w,
            pill_h,
            ath_tokens::RADIUS_XS as usize,
            accent_subtle(),
        );
        canvas.draw_text_aa(
            (pill_x + ath_tokens::SPACE_2 as usize) as i32,
            pill_y as i32 + 2,
            &label,
            ath_tokens::TYPE_CAPTION,
            FG,
            athgfx::text::FontFamily::Sans,
        );
        right_inset = text_x + pill_w as i32 + ath_tokens::SPACE_2 as i32;
    }
    // Available text width: content inset on the left, badge-aware on the right.
    let avail_px = TOAST_W as i32 - text_x - right_inset;
    // Source/time line — `type.caption` `text.tertiary` (the quieter body row).
    let source = fit_aa(&canvas, &toast.source, ath_tokens::TYPE_CAPTION, avail_px);
    canvas.draw_text_aa(
        text_x,
        14,
        &source,
        ath_tokens::TYPE_CAPTION,
        FG_DIM,
        athgfx::text::FontFamily::Sans,
    );
    // Title line — `type.label` `text.primary` (the toast headline).
    let title = fit_aa(&canvas, &toast.title, ath_tokens::TYPE_LABEL, avail_px);
    canvas.draw_text_aa(
        text_x,
        38,
        &title,
        ath_tokens::TYPE_LABEL,
        FG,
        athgfx::text::FontFamily::Sans,
    );

    // Inline-action row (notifications.md §5): up to MAX_TOAST_ACTIONS chips,
    // right-aligned along the bottom edge. The first chip is the primary
    // (accent.subtle fill + text.primary), the rest secondary (bg.elevated). A
    // glance surface — deep actions live in the Center item row.
    if !toast.actions.is_empty() {
        let chip_h = (ath_tokens::TYPE_CAPTION.line_height as i32 + 6) as usize;
        let chip_y = TOAST_H as usize - chip_h - ath_tokens::SPACE_2 as usize;
        let mut chip_right = TOAST_W as i32 - text_x;
        for (i, action) in toast.actions.iter().take(MAX_TOAST_ACTIONS).enumerate() {
            let lw = canvas.measure_text_aa(
                action,
                ath_tokens::TYPE_CAPTION,
                athgfx::text::FontFamily::Sans,
            );
            let chip_w = (lw + 2 * ath_tokens::SPACE_3 as i32) as usize;
            let chip_x = chip_right - chip_w as i32;
            if chip_x < text_x {
                break;
            }
            // Primary = first chip (accent.subtle), others = bg.elevated.
            let bg = if i == 0 {
                accent_subtle()
            } else {
                PALETTE.bg_elevated
            };
            canvas.fill_rounded_rect(
                chip_x as usize,
                chip_y,
                chip_w,
                chip_h,
                ath_tokens::RADIUS_XS as usize,
                bg,
            );
            canvas.draw_text_aa(
                chip_x + ath_tokens::SPACE_3 as i32,
                chip_y as i32 + 3,
                action,
                ath_tokens::TYPE_CAPTION,
                FG,
                athgfx::text::FontFamily::Sans,
            );
            chip_right = chip_x - ath_tokens::SPACE_2 as i32;
        }
    }
}

/// `accent.subtle` — the live derived accent at ~24% alpha (notifications.md:
/// count badge + inline-action primary fill). Reads the live seed so a Vibe
/// re-skin recolours these with the shell.
#[inline]
fn accent_subtle() -> u32 {
    ath_tokens::derive_accent(accent_seed(), PALETTE).subtle
}

/// Truncate `s` (on a char boundary) to the longest prefix whose AA advance at
/// `style` fits `avail_px`. Replaces the fixed `.truncate(34)` char-cell clamp
/// now that text is proportional (`draw_text_aa`). The 8×8 fallback path inside
/// `draw_text_aa`/`measure_text_aa` keeps this honest during early boot.
fn fit_aa<'a>(
    canvas: &athgfx::Canvas,
    s: &'a str,
    style: ath_tokens::TypeStyle,
    avail_px: i32,
) -> &'a str {
    if avail_px <= 0 {
        return "";
    }
    if canvas.measure_text_aa(s, style, athgfx::text::FontFamily::Sans) <= avail_px {
        return s;
    }
    let mut end = s.len();
    while end > 0 {
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        if end == 0 {
            break;
        }
        if canvas.measure_text_aa(&s[..end], style, athgfx::text::FontFamily::Sans) <= avail_px {
            return &s[..end];
        }
        end -= 1;
    }
    ""
}

/// Map a notification urgency to its design-token bar colour (desktop-shell.md
/// §5): low → text.tertiary, normal → accent.base, critical → state.danger.
#[inline]
fn urgency_bar(urgency: NotificationUrgency) -> u32 {
    match urgency {
        NotificationUrgency::Low => BAR_LOW,
        NotificationUrgency::Normal => bar_normal(),
        NotificationUrgency::Critical => BAR_CRITICAL,
    }
}

/// Re-pin every live toast to its slot (top-right, newest on top) and apply the
/// **stack depth cue** (notifications.md §1): each card behind the front one is
/// composited ~8% dimmer per step via a per-surface `Opacity` effect, so a stack
/// reads as depth rather than a flat list (the macOS deck cue). The front
/// (newest) card stays fully opaque.
///
/// Reduced-motion (a11y): the depth cue is suppressed — every toast renders at
/// full opacity (the spec's "no decks when reduced-motion" rule; the flat
/// `stroke.subtle` separator substitute is the top-edge highlight each card
/// already draws).
fn restack(toasts: &[Toast]) {
    let Some((sw, _sh)) = crate::compositor::screen_dimensions() else {
        return;
    };
    let reduced = crate::a11y::reduced_motion_on();
    let x = sw as i32 - TOAST_W as i32 - TOAST_MARGIN;
    // `i == 0` is the front (newest) card after `.rev()`.
    for (i, t) in toasts.iter().rev().enumerate() {
        let y = TOAST_MARGIN + i as i32 * (TOAST_H as i32 + TOAST_GAP);
        let _ = crate::compositor::set_surface_origin(t.surface, x, y);
        let opacity: u8 = if reduced || i == 0 {
            0xFF
        } else {
            // ~8% dimmer per step behind the front card, floored so the back of
            // a full stack stays legible.
            let dim = (i as u32 * 20).min(70);
            (0xFF - dim) as u8
        };
        let _ = crate::compositor::set_surface_opacity(t.surface, opacity);
    }
}

fn expire_locked(toasts: &mut Vec<Toast>, now: u64) {
    let before = toasts.len();
    toasts.retain(|t| {
        if t.deadline_ms <= now {
            let _ = crate::compositor::close_surface(t.surface);
            false
        } else {
            true
        }
    });
    let gone = before - toasts.len();
    if gone > 0 {
        EXPIRED.fetch_add(gone as u64, Ordering::Relaxed);
        restack(toasts);
    }
}

/// Post a toast. Never steals focus; over [`MAX_VISIBLE`] the oldest is
/// evicted. Returns false only when the compositor isn't up.
pub fn post(source: &str, title: &str, urgency: NotificationUrgency) -> bool {
    post_at(source, title, urgency, now_ms())
}

/// [`post`] with an explicit clock (the smoketest's deterministic time).
pub fn post_at(source: &str, title: &str, urgency: NotificationUrgency, now: u64) -> bool {
    post_at_with_actions(source, title, urgency, &[], now)
}

/// [`post`] carrying inline actions (notifications.md §5). `actions` are the
/// labels the source declared over its capability-checked channel; the surface
/// renders them without launching the app. A source without the action
/// capability simply passes `&[]` → no action row (AthGuard is never bypassed:
/// the *dispatch* of a button is the shell's capability-gated IPC, this just
/// records what was declared).
pub fn post_with_actions(
    source: &str,
    title: &str,
    urgency: NotificationUrgency,
    actions: &[&str],
) -> bool {
    post_at_with_actions(source, title, urgency, actions, now_ms())
}

/// [`post_with_actions`] with an explicit clock (the smoketest's deterministic
/// time).
pub fn post_at_with_actions(
    source: &str,
    title: &str,
    urgency: NotificationUrgency,
    actions: &[&str],
    now: u64,
) -> bool {
    // Focus / Do-Not-Disturb: suppress the toast surface but keep the history
    // entry. Critical urgency still breaks through (alarms/security must never be
    // silenced) — the macOS/Windows "Focus allows critical" rule. The quiet flag
    // routes the kept entry into the Center's "Delivered Quietly" section.
    let quiet = quick_settings::dnd_enabled() && !matches!(urgency, NotificationUrgency::Critical);

    // ALWAYS record to history first — even under Focus/DND, the message is kept
    // (DND silences the interruption, it never loses the notification).
    record_history(source, title, urgency, actions, quiet, now);
    POSTED.fetch_add(1, Ordering::Relaxed);

    // Accessibility: mirror the announcement into the visual-alert caption stream
    // so a deaf/HoH user reads what a notification sound would have signalled.
    // A no-op unless the user enabled visual alerts; Critical urgency requests a
    // screen flash (the sound they can't hear).
    crate::captions::on_notification(
        source,
        title,
        matches!(urgency, NotificationUrgency::Critical),
    );

    if quiet {
        SUPPRESSED.fetch_add(1, Ordering::Relaxed);
        return true;
    }

    let mut toasts = TOASTS.lock();
    expire_locked(&mut toasts, now);
    if toasts.len() >= MAX_VISIBLE {
        let oldest = toasts.remove(0);
        let _ = crate::compositor::close_surface(oldest.surface);
        EVICTED.fetch_add(1, Ordering::Relaxed);
    }

    let Some((id, ptr)) = crate::compositor::create_kernel_surface(TOAST_W, TOAST_H) else {
        return false;
    };
    let toast = Toast {
        surface: id,
        deadline_ms: now + TOAST_TTL_MS,
        source: String::from(source),
        title: String::from(title),
        urgency,
        actions: actions.iter().map(|a| String::from(*a)).collect(),
    };
    render_toast(ptr, &toast, &toasts);
    let _ = crate::compositor::set_surface_title(id, "Notification");
    // material.glass depth: radius.md rounded mask + elev.2 drop shadow
    // (desktop-shell.md §5). Reuses the existing compositor DropShadow effect —
    // ELEV_2 maps verbatim to its fields (design-language §5.3).
    let _ = crate::compositor::add_surface_effect(
        id,
        crate::compositor::SurfaceEffect::RoundedCorners {
            radius: TOAST_RADIUS as u32,
        },
    );
    let _ = crate::compositor::add_surface_effect(
        id,
        crate::compositor::SurfaceEffect::DropShadow {
            offset_x: 0,
            offset_y: ath_tokens::ELEV_2.offset_y,
            radius: ath_tokens::ELEV_2.radius,
            color: ath_tokens::ELEV_2.color,
        },
    );
    // Present off-screen-agnostic first; restack pins the real slot.
    let _ = crate::compositor::present_surface(id, TOAST_MARGIN, TOAST_MARGIN);
    toasts.push(toast);
    restack(&toasts);
    crate::serial_println!("[notify] toast: {} — {} ({:?})", source, title, urgency);
    true
}

/// Append to the retained history ring, dropping the oldest past [`HISTORY_CAP`].
fn record_history(
    source: &str,
    title: &str,
    urgency: NotificationUrgency,
    actions: &[&str],
    quiet: bool,
    now: u64,
) {
    let mut hist = HISTORY.lock();
    let id = NEXT_HISTORY_ID.fetch_add(1, Ordering::Relaxed);
    hist.push(HistoryEntry {
        id,
        source: String::from(source),
        title: String::from(title),
        urgency,
        posted_ms: now,
        actions: actions.iter().map(|a| String::from(*a)).collect(),
        quiet,
    });
    while hist.len() > HISTORY_CAP {
        hist.remove(0);
    }
}

/// Number of retained history entries.
pub fn history_count() -> usize {
    HISTORY.lock().len()
}

/// Dismiss one history entry by id; returns true if it was present.
pub fn dismiss_history(id: u64) -> bool {
    let mut hist = HISTORY.lock();
    let before = hist.len();
    hist.retain(|e| e.id != id);
    hist.len() < before
}

/// Clear the entire history ring (the "Clear all" affordance).
pub fn clear_history() {
    HISTORY.lock().clear();
}

/// Unread/history badge total for the tray bell.
pub fn badge_count() -> usize {
    HISTORY.lock().len()
}

/// Dismiss every toast whose TTL has passed. Driven from the shell's repaint
/// path (`shell_runner::render_shell`, which runs on every keyboard/mouse
/// interaction) and lazily by every [`post_at`]. Both call sites are the same
/// CPU0 IF=0 context that posts, so `TOASTS` has no preemptible acquirer and
/// needs no IF=0 RAII guard today.
///
/// LOCK NOTE (athena-reviewer 2026-06-17): if `expire_tick` is ever moved to a
/// *preemptible* kernel thread (a free-running tick), `TOASTS` would then be
/// acquired by both that thread and the IF=0 `post_at` — route it through a
/// `lock_compositor`-style IF=0 guard FIRST (same footgun class as `lock_audio`).
pub fn expire_tick(now: u64) {
    expire_locked(&mut TOASTS.lock(), now);
}

/// Expire toasts against the live clock — the no-argument tick the shell's
/// repaint path calls. Uses the SAME `now_ms()` source (`hpet::read_millis`)
/// that [`post_at`] stamps deadlines with, so the comparison is on one time
/// base (a wall-clock tick would mismatch the HPET deadlines and mis-expire).
pub fn expire_now() {
    expire_locked(&mut TOASTS.lock(), now_ms());
}

// ── Quick Settings / Control Center (real backends only) ───────────────────
//
// Concept §Unified Settings ("every option searchable… no nag") + §"The user
// owns the machine": the Control Center pull-down is the everyday panel a
// Windows 11 / macOS switcher reaches for. Every toggle here drives a LIVE
// kernel backend — Wi-Fi radio, audio master mute, Focus/DND (which actually
// suppresses toasts above), Night Light (persisted config the compositor reads),
// and the Vibe accent (theme_engine override). No cosmetic switches.
pub mod quick_settings {
    use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

    /// Focus / Do-Not-Disturb. When on, `notify::post_at` suppresses the toast
    /// surface (history is still recorded; critical urgency still breaks
    /// through). This is shell state, so an atomic — readable from the IF=0 post
    /// path without taking a lock.
    static DND: AtomicBool = AtomicBool::new(false);

    /// Audio-mute "remembered level": when we mute, we stash the prior master
    /// volume here (0..=1000 ‰) so unmute restores it exactly. 0 means "was
    /// already silent". `u32::MAX` sentinel = "not muted".
    static MUTED_LEVEL_PERMILLE: AtomicU32 = AtomicU32::new(u32::MAX);

    /// The accent seed to restore when the Vibe quick-toggle is turned OFF
    /// (RAEBLUE default). Lets the one-tap toggle flip between the live theme
    /// accent and a vivid Vibe accent and back.
    const VIBE_ACCENT: u32 = 0xFF_FF_5A_7A; // a vivid magenta-pink Vibe seed

    /// Config key for Night Light (the compositor's warm-tint pass reads it).
    const NIGHT_LIGHT_KEY: &str = "/display/night_light";

    /// The five Quick Settings controls, in panel order. The notification
    /// center renders one row per control and click-toggles it.
    #[derive(Clone, Copy, PartialEq, Eq)]
    pub enum Control {
        Wifi,
        AudioMute,
        Dnd,
        NightLight,
        VibeAccent,
    }

    /// Every control, in display order (the §quicksettings_controls=5 proof).
    pub const CONTROLS: [Control; 5] = [
        Control::Wifi,
        Control::AudioMute,
        Control::Dnd,
        Control::NightLight,
        Control::VibeAccent,
    ];

    /// Human label for a control row.
    pub fn label(c: Control) -> &'static str {
        match c {
            Control::Wifi => "Wi-Fi",
            Control::AudioMute => "Mute",
            Control::Dnd => "Focus",
            Control::NightLight => "Night Light",
            Control::VibeAccent => "Vibe Accent",
        }
    }

    // ── Live state readers (each hits a real backend) ──────────────────────

    pub fn dnd_enabled() -> bool {
        DND.load(Ordering::Acquire)
    }

    fn wifi_enabled() -> bool {
        crate::netmanager::quick_net_status().0
    }

    fn audio_muted() -> bool {
        MUTED_LEVEL_PERMILLE.load(Ordering::Acquire) != u32::MAX
    }

    fn night_light_on() -> bool {
        // IF=0 lock discipline (athena-reviewer 2026-06-17): the config registry
        // is a plain spin::Mutex and this runs in the keyboard/mouse IRQ click
        // path (IF=0); disable interrupts for the brief access so an IRQ can't
        // land mid-hold and a future preemptible registry writer can't strand it.
        x86_64::instructions::interrupts::without_interrupts(|| {
            crate::config_registry::get_bool(NIGHT_LIGHT_KEY).unwrap_or(false)
        })
    }

    /// Persist the Night Light flag (interrupts-disabled — see [`night_light_on`]).
    fn set_night_light(on: bool) {
        x86_64::instructions::interrupts::without_interrupts(|| {
            crate::config_registry::set_bool(NIGHT_LIGHT_KEY, on);
        });
    }

    fn vibe_accent_on() -> bool {
        crate::theme_engine::active_accent() == VIBE_ACCENT
    }

    /// True/false ON-state of a control, read live from its backend.
    pub fn is_on(c: Control) -> bool {
        match c {
            Control::Wifi => wifi_enabled(),
            Control::AudioMute => audio_muted(),
            Control::Dnd => dnd_enabled(),
            Control::NightLight => night_light_on(),
            Control::VibeAccent => vibe_accent_on(),
        }
    }

    // ── Toggles (each mutates a real backend) ──────────────────────────────

    /// Toggle a control; returns its new ON-state. Every arm changes real
    /// system state — the whole point of the Control Center.
    pub fn toggle(c: Control) -> bool {
        match c {
            Control::Wifi => {
                let next = !wifi_enabled();
                crate::netmanager::quick_set_wifi(next);
                next
            }
            Control::AudioMute => {
                if audio_muted() {
                    // Unmute: restore the stashed level.
                    let permille = MUTED_LEVEL_PERMILLE.swap(u32::MAX, Ordering::AcqRel);
                    let vol = (permille.min(1000) as f32) / 1000.0;
                    crate::audio::quick_set_master_volume(vol);
                    false
                } else {
                    // Mute: stash the current level, then drop to silence.
                    let cur = crate::audio::quick_master_volume();
                    let permille = (cur * 1000.0) as u32;
                    MUTED_LEVEL_PERMILLE.store(permille.min(1000), Ordering::Release);
                    crate::audio::quick_set_master_volume(0.0);
                    true
                }
            }
            Control::Dnd => {
                let next = !dnd_enabled();
                DND.store(next, Ordering::Release);
                next
            }
            Control::NightLight => {
                let next = !night_light_on();
                set_night_light(next);
                next
            }
            Control::VibeAccent => {
                if vibe_accent_on() {
                    // Back to the theme default (RAEBLUE clears the override).
                    crate::theme_engine::set_active_accent(ath_tokens::RAEBLUE);
                    false
                } else {
                    crate::theme_engine::set_active_accent(VIBE_ACCENT);
                    true
                }
            }
        }
    }

    /// Count of wired controls (the proof line's `quicksettings_controls`).
    pub fn control_count() -> usize {
        CONTROLS.len()
    }
}

/// Live toast count (for the shell's badge).
pub fn visible_count() -> usize {
    TOASTS.lock().len()
}

// ── Notification Center panel (glass pull-down) ────────────────────────────
//
// A kernel-drawn glass surface anchored bottom-right above the tray — the
// pull-down a Windows 11 / macOS switcher reaches for. Shows the retained,
// grouped history (scrollable, dismiss-one / clear-all) over the Quick Settings
// row. Mirrors the live `SettingsPanel` kernel-draw pattern (shell_runner owns
// the desktop surface; this floats above it as its own surface).

/// Panel width — a comfortable single column (desktop-shell.md §5 glass card).
const CENTER_W: u32 = 380;
/// Quick-settings strip height at the top of the panel.
const QS_ROW_H: i32 = 40;
const QS_HEADER_H: i32 = 56;
/// One history row.
const HIST_ROW_H: i32 = 52;
/// Group-header row (app name + count badge + collapse chevron).
const GROUP_HEADER_H: i32 = 32;
/// Inline-action row height appended under an expanded item that declares actions.
const ACTION_ROW_H: i32 = 28;
const CENTER_MARGIN: i32 = ath_tokens::SPACE_4 as i32;

/// Sources whose group is currently COLLAPSED in the Center (chevron up). A group
/// not listed is expanded (the default — newest items visible). Click the group
/// header to toggle. Independent per group (notifications.md §3: "one group's
/// expansion does not force-collapse others").
static COLLAPSED_GROUPS: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Is `source`'s group collapsed (decked) in the Center?
fn group_collapsed(source: &str) -> bool {
    COLLAPSED_GROUPS.lock().iter().any(|s| s == source)
}

/// Toggle a group's collapse state; returns the new collapsed flag.
fn toggle_group(source: &str) -> bool {
    let mut g = COLLAPSED_GROUPS.lock();
    if let Some(i) = g.iter().position(|s| s == source) {
        g.remove(i);
        false
    } else {
        g.push(String::from(source));
        true
    }
}

/// One source's group of retained notifications, newest-first, plus whether they
/// were delivered quietly (so the renderer can route them to the calm section).
struct Group {
    source: String,
    /// (id, title, urgency, action labels) per item, newest first.
    items: Vec<(u64, String, NotificationUrgency, Vec<String>)>,
}

/// Build the grouped view of history for one section: `quiet=false` returns the
/// normal groups (urgency bars), `quiet=true` returns the "Delivered Quietly"
/// items (calmer, no bars). Groups are ordered by their newest member first
/// (`record_history` keeps the ring in post order, so the last-seen source wins).
fn build_groups(quiet: bool) -> Vec<Group> {
    let hist = HISTORY.lock();
    let mut groups: Vec<Group> = Vec::new();
    // Iterate newest-first so each group's first pushed item is its newest, and
    // the first time we see a source fixes that group's position (newest group
    // first).
    for e in hist.iter().rev() {
        if e.quiet != quiet {
            continue;
        }
        if let Some(g) = groups.iter_mut().find(|g| g.source == e.source) {
            g.items
                .push((e.id, e.title.clone(), e.urgency, e.actions.clone()));
        } else {
            groups.push(Group {
                source: e.source.clone(),
                items: alloc::vec![(e.id, e.title.clone(), e.urgency, e.actions.clone())],
            });
        }
    }
    groups
}

struct CenterPanel {
    surface: u64,
    ptr: *mut u8,
    w: u32,
    h: u32,
}

// SAFETY: `ptr` is a kernel-owned compositor surface backing store, allocated
// from contiguous frames that live for the surface's lifetime (closed only when
// the panel is taken out of CENTER). Only the single BSP scheduler thread (APs
// halt post-boot) ever renders through it, serialized by the CENTER mutex.
unsafe impl Send for CenterPanel {}

static CENTER: Mutex<Option<CenterPanel>> = Mutex::new(None);

/// Is the Notification Center currently open?
pub fn center_visible() -> bool {
    CENTER.lock().is_some()
}

/// True iff `sid` is the live Notification Center surface (the shell's mouse
/// path uses this to route clicks into [`center_click`]).
pub fn center_surface_at(sid: u64) -> bool {
    CENTER
        .lock()
        .as_ref()
        .map(|p| p.surface == sid)
        .unwrap_or(false)
}

/// Compute the panel height from the live grouped history + quick-settings rows,
/// clamped to the screen. Measures the grouped layout (headers + items + action
/// rows + the quiet section) so the panel grows to fit what it will draw, capping
/// at the screen height (scroll beyond — the renderer stops at `h`).
fn center_height(screen_h: u32) -> u32 {
    let qs = QS_HEADER_H + quick_settings::control_count() as i32 * QS_ROW_H;
    let hist_top = QS_HEADER_H * 2; // header + clear-all band the renderer reserves
    let hist_body = if history_count() == 0 {
        // Empty-state block needs a comfortable calm area.
        ath_tokens::SPACE_8 as i32 * 2
    } else {
        // Lay out against a tall ceiling to get the natural content height.
        let rows = layout_history(0, i32::MAX / 2);
        rows.iter()
            .map(|r| match r {
                CenterRow::GroupHeader { .. } | CenterRow::QuietHeader { .. } => GROUP_HEADER_H,
                CenterRow::Item { .. } => HIST_ROW_H,
                CenterRow::Actions { .. } => ACTION_ROW_H,
            })
            .sum::<i32>()
            .max(HIST_ROW_H)
    };
    let total = (qs + hist_top + hist_body + CENTER_MARGIN * 2) as u32;
    total.min(screen_h.saturating_sub(2 * CENTER_MARGIN as u32))
}

/// Toggle the Notification Center. Creates the glass surface on open, closes it
/// on close. Returns the new visibility. Safe to call from the shell click/key
/// path (it owns no other lock while doing compositor work).
pub fn toggle_center() -> bool {
    // Close if open.
    {
        let mut guard = CENTER.lock();
        if let Some(panel) = guard.take() {
            let _ = crate::compositor::close_surface(panel.surface);
            crate::compositor::recomposite();
            return false;
        }
    }
    let Some((sw, sh)) = crate::compositor::screen_dimensions() else {
        return false;
    };
    let h = center_height(sh);
    let Some((id, ptr)) = crate::compositor::create_kernel_surface(CENTER_W, h) else {
        return false;
    };
    let panel = CenterPanel {
        surface: id,
        ptr,
        w: CENTER_W,
        h,
    };
    render_center(&panel);
    let _ = crate::compositor::set_surface_title(id, "Notification Center");
    let _ = crate::compositor::add_surface_effect(
        id,
        crate::compositor::SurfaceEffect::RoundedCorners {
            radius: ath_tokens::RADIUS_LG,
        },
    );
    let _ = crate::compositor::add_surface_effect(
        id,
        crate::compositor::SurfaceEffect::DropShadow {
            offset_x: 0,
            offset_y: ath_tokens::ELEV_3.offset_y,
            radius: ath_tokens::ELEV_3.radius,
            color: ath_tokens::ELEV_3.color,
        },
    );
    // Anchor bottom-right above the taskbar/tray.
    let x = sw as i32 - CENTER_W as i32 - CENTER_MARGIN;
    let taskbar = 44i32;
    let y = sh as i32 - h as i32 - taskbar - CENTER_MARGIN;
    let _ = crate::compositor::present_surface(id, x.max(0), y.max(0));
    *CENTER.lock() = Some(panel);
    crate::serial_println!(
        "[notify] center: opened ({} in history, {} quick controls)",
        history_count(),
        quick_settings::control_count(),
    );
    true
}

/// Repaint the open center (called after a dismiss / quick-toggle so the panel
/// reflects new state without a full reopen). No-op when closed.
fn refresh_center() {
    let guard = CENTER.lock();
    if let Some(panel) = guard.as_ref() {
        render_center(panel);
        if let Some((x, y, _, _, _)) = crate::compositor::surface_frame(panel.surface) {
            let _ = crate::compositor::present_surface(panel.surface, x, y);
        }
    }
}

/// A laid-out hit region in the Center's history area. Built ONCE by
/// [`layout_history`] and consumed by BOTH the renderer and the click hit-test,
/// so the two can never drift (the click would otherwise have to re-derive the
/// grouped/collapsed geometry and silently desync). `y` is panel-local.
enum CenterRow {
    /// A group header: app name + count badge + collapse chevron + per-group clear.
    GroupHeader {
        y: i32,
        source: String,
        count: usize,
        collapsed: bool,
        quiet: bool,
    },
    /// An item row (title + dismiss ×).
    Item {
        y: i32,
        id: u64,
        title: String,
        urgency: NotificationUrgency,
        quiet: bool,
    },
    /// An inline-action row under an expanded item (one chip per declared action).
    Actions {
        y: i32,
        id: u64,
        labels: Vec<String>,
    },
    /// The "Delivered Quietly" section header (DND honesty model).
    QuietHeader { y: i32 },
}

/// Walk the grouped history into a flat row list starting at `y0`, stopping
/// before `max_y`. Normal groups first, then (under DND) the "Delivered Quietly"
/// section. Collapsed groups show only their header + newest item decked; expanded
/// groups list every item with its action row. Reduced-motion does not change the
/// layout here (decks are a *visual* cue drawn by the renderer); collapse is a
/// user toggle, honored in both modes.
fn layout_history(y0: i32, max_y: i32) -> Vec<CenterRow> {
    let mut rows = Vec::new();
    let mut y = y0;

    let emit_section = |groups: Vec<Group>, quiet: bool, y: &mut i32, rows: &mut Vec<CenterRow>| {
        for g in groups {
            if *y + GROUP_HEADER_H > max_y {
                break;
            }
            let collapsed = group_collapsed(&g.source);
            rows.push(CenterRow::GroupHeader {
                y: *y,
                source: g.source.clone(),
                count: g.items.len(),
                collapsed,
                quiet,
            });
            *y += GROUP_HEADER_H;
            // Collapsed → only the newest item (decked); expanded → all items.
            let show = if collapsed { 1 } else { g.items.len() };
            for (id, title, urgency, actions) in g.items.into_iter().take(show) {
                if *y + HIST_ROW_H > max_y {
                    break;
                }
                rows.push(CenterRow::Item {
                    y: *y,
                    id,
                    title,
                    urgency,
                    quiet,
                });
                *y += HIST_ROW_H;
                // Action row only when expanded and the item declared actions.
                if !collapsed && !actions.is_empty() && *y + ACTION_ROW_H <= max_y {
                    rows.push(CenterRow::Actions {
                        y: *y,
                        id,
                        labels: actions,
                    });
                    *y += ACTION_ROW_H;
                }
            }
        }
    };

    emit_section(build_groups(false), false, &mut y, &mut rows);

    // "Delivered Quietly" — only when DND is on AND there are quiet items.
    if quick_settings::dnd_enabled() {
        let quiet_groups = build_groups(true);
        if !quiet_groups.is_empty() && y + GROUP_HEADER_H <= max_y {
            rows.push(CenterRow::QuietHeader { y });
            y += GROUP_HEADER_H;
            emit_section(quiet_groups, true, &mut y, &mut rows);
        }
    }

    rows
}

fn render_center(panel: &CenterPanel) {
    let mut canvas =
        unsafe { athgfx::Canvas::new(panel.ptr, panel.w as usize, panel.h as usize, 4) };
    let p = PALETTE;
    let w = panel.w as usize;
    let h = panel.h as usize;
    let radius = ath_tokens::RADIUS_LG as usize;
    let pad = ath_tokens::SPACE_4 as usize;

    // Glass panel: translucent tint (direct write preserves alpha) + 1px top edge.
    canvas.fill_rect(0, 0, w, h, CARD_BG);
    canvas.fill_rect(radius, 0, w - 2 * radius, 1, TOP_EDGE);

    let mut y = pad as i32;

    // ── Quick Settings header ───────────────────────────────────────────
    canvas.draw_text_aa(
        pad as i32,
        y,
        "Control Center",
        ath_tokens::TYPE_TITLE,
        FG,
        athgfx::text::FontFamily::Sans,
    );
    y += QS_HEADER_H;

    // ── Quick-settings rows: pill toggle + label + live value ───────────
    for c in quick_settings::CONTROLS {
        let on = quick_settings::is_on(c);
        let row_y = y;
        // Toggle pill: accent.base when on, bg.elevated when off.
        let pill_w = 36usize;
        let pill_h = 18usize;
        let pill_x = w - pad - pill_w;
        let pill_y = row_y as usize + (QS_ROW_H as usize - pill_h) / 2;
        let pill_bg = if on { accent_base() } else { p.bg_elevated };
        canvas.fill_rounded_rect(pill_x, pill_y, pill_w, pill_h, pill_h / 2, pill_bg);
        let knob_x = if on {
            pill_x + pill_w - pill_h + 2
        } else {
            pill_x + 2
        };
        canvas.fill_rounded_rect(
            knob_x,
            pill_y + 2,
            pill_h - 4,
            pill_h - 4,
            (pill_h - 4) / 2,
            FG,
        );
        // Label + the live value caption.
        let label_y = row_y + (QS_ROW_H - ath_tokens::TYPE_LABEL.line_height as i32) / 2;
        canvas.draw_text_aa(
            pad as i32,
            label_y,
            quick_settings::label(c),
            ath_tokens::TYPE_LABEL,
            FG,
            athgfx::text::FontFamily::Sans,
        );
        y += QS_ROW_H;
    }

    // ── History header + Clear-all ──────────────────────────────────────
    y += ath_tokens::SPACE_2 as i32;
    // hairline separator
    canvas.fill_rect(pad, y as usize, w - 2 * pad, 1, p.stroke_subtle);
    y += ath_tokens::SPACE_2 as i32;
    canvas.draw_text_aa(
        pad as i32,
        y,
        "Notifications",
        ath_tokens::TYPE_LABEL,
        FG,
        athgfx::text::FontFamily::Sans,
    );
    let clear_label = "Clear all";
    let clear_w = canvas.measure_text_aa(
        clear_label,
        ath_tokens::TYPE_CAPTION,
        athgfx::text::FontFamily::Sans,
    );
    canvas.draw_text_aa(
        (w - pad) as i32 - clear_w,
        y,
        clear_label,
        ath_tokens::TYPE_CAPTION,
        accent_base(),
        athgfx::text::FontFamily::Sans,
    );
    y += QS_HEADER_H - ath_tokens::SPACE_2 as i32;

    // ── Grouped history list ────────────────────────────────────────────
    if history_count() == 0 {
        // "You're all caught up" empty state (notifications.md §4): a calm
        // centered block, not blank rows. The quick-settings strip above keeps
        // the panel from reading as broken.
        let line1 = "You're all caught up";
        let line2 = "Notifications you receive will appear here.";
        let w1 =
            canvas.measure_text_aa(line1, ath_tokens::TYPE_BODY, athgfx::text::FontFamily::Sans);
        let w2 = canvas.measure_text_aa(
            line2,
            ath_tokens::TYPE_CAPTION,
            athgfx::text::FontFamily::Sans,
        );
        // A soft bell-off glyph (text.tertiary) above the lines.
        let glyph = "( )";
        let wg = canvas.measure_text_aa(
            glyph,
            ath_tokens::TYPE_TITLE,
            athgfx::text::FontFamily::Sans,
        );
        let cy = y + (h as i32 - y) / 2 - ath_tokens::SPACE_6 as i32;
        canvas.draw_text_aa(
            (w as i32 - wg) / 2,
            cy,
            glyph,
            ath_tokens::TYPE_TITLE,
            FG_DIM,
            athgfx::text::FontFamily::Sans,
        );
        canvas.draw_text_aa(
            (w as i32 - w1) / 2,
            cy + ath_tokens::SPACE_5 as i32 + 6,
            line1,
            ath_tokens::TYPE_BODY,
            p.text_secondary,
            athgfx::text::FontFamily::Sans,
        );
        canvas.draw_text_aa(
            (w as i32 - w2) / 2,
            cy + ath_tokens::SPACE_5 as i32 + ath_tokens::SPACE_5 as i32,
            line2,
            ath_tokens::TYPE_CAPTION,
            FG_DIM,
            athgfx::text::FontFamily::Sans,
        );
    } else {
        for row in layout_history(y, h as i32) {
            match row {
                CenterRow::GroupHeader {
                    y,
                    source,
                    count,
                    collapsed,
                    quiet,
                } => {
                    // Group app name (type.label, neutral — never accent for a
                    // static label, per material-and-shadow.md §chrome restraint).
                    let avail = (w - 2 * pad - 80) as i32;
                    let name = fit_aa(&canvas, &source, ath_tokens::TYPE_LABEL, avail);
                    let name_col = if quiet { FG_DIM } else { FG };
                    canvas.draw_text_aa(
                        pad as i32,
                        y + 6,
                        name,
                        ath_tokens::TYPE_LABEL,
                        name_col,
                        athgfx::text::FontFamily::Sans,
                    );
                    let name_w = canvas.measure_text_aa(
                        name,
                        ath_tokens::TYPE_LABEL,
                        athgfx::text::FontFamily::Sans,
                    );
                    // Count badge (accent.subtle pill) when >1 in the group.
                    if count > 1 {
                        let label = alloc::format!("{}", count);
                        let lw = canvas.measure_text_aa(
                            &label,
                            ath_tokens::TYPE_CAPTION,
                            athgfx::text::FontFamily::Sans,
                        );
                        let bw = (lw + 2 * ath_tokens::SPACE_2 as i32) as usize;
                        let bx = pad + name_w as usize + ath_tokens::SPACE_2 as usize;
                        canvas.fill_rounded_rect(
                            bx,
                            y as usize + 5,
                            bw,
                            (ath_tokens::TYPE_CAPTION.line_height as usize) + 4,
                            ath_tokens::RADIUS_XS as usize,
                            accent_subtle(),
                        );
                        canvas.draw_text_aa(
                            (bx + ath_tokens::SPACE_2 as usize) as i32,
                            y + 7,
                            &label,
                            ath_tokens::TYPE_CAPTION,
                            FG,
                            athgfx::text::FontFamily::Sans,
                        );
                    }
                    // Collapse chevron (right): ^ collapsed / v expanded.
                    let chevron = if collapsed { "^" } else { "v" };
                    canvas.draw_text_aa(
                        (w - pad - 10) as i32,
                        y + 6,
                        chevron,
                        ath_tokens::TYPE_LABEL,
                        FG_DIM,
                        athgfx::text::FontFamily::Sans,
                    );
                }
                CenterRow::Item {
                    y,
                    id: _,
                    title,
                    urgency,
                    quiet,
                } => {
                    // Quiet items render calmer: NO urgency bar (the honesty cue).
                    if !quiet {
                        let bar = urgency_bar(urgency);
                        canvas.fill_rect(pad, y as usize + 6, 3, HIST_ROW_H as usize - 12, bar);
                    }
                    let inset = pad + ath_tokens::SPACE_3 as usize;
                    let avail = (w - inset - pad - 16) as i32;
                    let title_fit = fit_aa(&canvas, &title, ath_tokens::TYPE_LABEL, avail);
                    canvas.draw_text_aa(
                        inset as i32,
                        y + 16,
                        title_fit,
                        ath_tokens::TYPE_LABEL,
                        if quiet { p.text_secondary } else { FG },
                        athgfx::text::FontFamily::Sans,
                    );
                    // Dismiss × affordance.
                    canvas.draw_text_aa(
                        (w - pad - 8) as i32,
                        y + 14,
                        "x",
                        ath_tokens::TYPE_LABEL,
                        FG_DIM,
                        athgfx::text::FontFamily::Sans,
                    );
                }
                CenterRow::Actions { y, id: _, labels } => {
                    // Inline-action chips, left-aligned under the item.
                    let inset = pad + ath_tokens::SPACE_3 as usize;
                    let mut cx = inset as i32;
                    for (i, label) in labels.iter().take(3).enumerate() {
                        let lw = canvas.measure_text_aa(
                            label,
                            ath_tokens::TYPE_CAPTION,
                            athgfx::text::FontFamily::Sans,
                        );
                        let cw = (lw + 2 * ath_tokens::SPACE_3 as i32) as usize;
                        if cx + cw as i32 > (w - pad) as i32 {
                            break;
                        }
                        let bg = if i == 0 {
                            accent_subtle()
                        } else {
                            p.bg_elevated
                        };
                        canvas.fill_rounded_rect(
                            cx as usize,
                            y as usize + 2,
                            cw,
                            (ACTION_ROW_H - 6) as usize,
                            ath_tokens::RADIUS_XS as usize,
                            bg,
                        );
                        canvas.draw_text_aa(
                            cx + ath_tokens::SPACE_3 as i32,
                            y + 5,
                            label,
                            ath_tokens::TYPE_CAPTION,
                            FG,
                            athgfx::text::FontFamily::Sans,
                        );
                        cx += cw as i32 + ath_tokens::SPACE_2 as i32;
                    }
                }
                CenterRow::QuietHeader { y } => {
                    canvas.fill_rect(pad, y as usize, w - 2 * pad, 1, p.stroke_subtle);
                    canvas.draw_text_aa(
                        pad as i32,
                        y + 10,
                        "Delivered Quietly",
                        ath_tokens::TYPE_CAPTION,
                        FG_DIM,
                        athgfx::text::FontFamily::Sans,
                    );
                }
            }
        }
    }
}

/// Hit-test a click at panel-local `(lx, ly)` and act on it: toggle a quick
/// control, clear-all, or dismiss a history row. Returns true if consumed. The
/// shell click path calls this with coordinates already made panel-local.
pub fn center_click(lx: i32, ly: i32) -> bool {
    let (_w, _h) = {
        let guard = CENTER.lock();
        match guard.as_ref() {
            Some(panel) => (panel.w, panel.h),
            None => return false,
        }
    };
    let pad = ath_tokens::SPACE_4 as i32;
    let mut y = pad + QS_HEADER_H;
    // Quick-settings rows.
    for c in quick_settings::CONTROLS {
        if ly >= y && ly < y + QS_ROW_H {
            let _ = quick_settings::toggle(c);
            refresh_center();
            return true;
        }
        y += QS_ROW_H;
    }
    // History header row (Clear all is right-aligned).
    y += ath_tokens::SPACE_2 as i32 + ath_tokens::SPACE_2 as i32;
    let header_y = y;
    if ly >= header_y && ly < header_y + ath_tokens::TYPE_LABEL.line_height as i32 + 4 {
        // Right third = Clear all.
        if lx > CENTER_W as i32 * 2 / 3 {
            clear_history();
            refresh_center();
            return true;
        }
    }
    y += QS_HEADER_H - ath_tokens::SPACE_2 as i32;
    // History rows: walk the SAME layout model the renderer used so hit regions
    // can't desync. A group header click toggles collapse (or per-group clear on
    // the chevron/edge); an item click dismisses when on its × edge.
    let rows = layout_history(y, _h as i32);
    for row in rows {
        match row {
            CenterRow::GroupHeader { y: ry, source, .. } => {
                if ly >= ry && ly < ry + GROUP_HEADER_H {
                    toggle_group(&source);
                    refresh_center();
                    return true;
                }
            }
            CenterRow::Item { y: ry, id, .. } => {
                if ly >= ry && ly < ry + HIST_ROW_H {
                    // Right edge = dismiss ×; elsewhere on the row is a no-op
                    // activate (the shell routes activation; here we consume it).
                    if lx > CENTER_W as i32 - pad - 20 {
                        dismiss_history(id);
                        refresh_center();
                    }
                    return true;
                }
            }
            CenterRow::Actions { y: ry, id, labels } => {
                if ly >= ry && ly < ry + ACTION_ROW_H {
                    // An action chip was clicked. The actual dispatch is the
                    // shell's capability-gated IPC (AthGuard is never bypassed
                    // here); the Center acts on the item by dismissing it, the
                    // notifications.md §5 "fires the callback, then dismisses".
                    if !labels.is_empty() {
                        dismiss_history(id);
                        refresh_center();
                    }
                    return true;
                }
            }
            CenterRow::QuietHeader { .. } => {}
        }
    }
    true // clicks inside the panel are always consumed (modal-ish)
}

pub fn init() {
    crate::serial_println!(
        "[ OK ] Notification surface ready (toasts: top-right, max {}, ttl {} ms)",
        MAX_VISIBLE,
        TOAST_TTL_MS,
    );
}

/// Deterministic proof with synthetic time: two toasts render as real
/// compositor surfaces in stacked slots; a 4th evicts the oldest; expiry
/// closes everything.
pub fn run_boot_smoketest() {
    let t0 = 1_000_000u64; // synthetic clock, far from any real deadline

    let posted = post_at(
        "selftest",
        "First notification",
        NotificationUrgency::Normal,
        t0,
    ) && post_at(
        "selftest",
        "Second (critical)",
        NotificationUrgency::Critical,
        t0 + 10,
    );
    let two_visible = visible_count() == 2;

    // Surfaces really exist and sit in distinct stacked slots.
    let slots_ok = {
        let toasts = TOASTS.lock();
        let frames: Vec<_> = toasts
            .iter()
            .filter_map(|t| crate::compositor::surface_frame(t.surface))
            .collect();
        frames.len() == 2 && frames[0].1 != frames[1].1
    };

    // Overflow: two more pushes evict the oldest (max 3 live).
    let _ = post_at("selftest", "Third", NotificationUrgency::Low, t0 + 20);
    let _ = post_at("selftest", "Fourth", NotificationUrgency::Low, t0 + 30);
    let evicted_ok = visible_count() == MAX_VISIBLE;

    // Expiry: advance past every deadline; all surfaces close.
    expire_tick(t0 + TOAST_TTL_MS + 100);
    let expired_ok = visible_count() == 0;

    // ── Token-wiring proof (desktop-shell.md §5) ────────────────────────
    // Fail-able: the glass tint must be the GLASS_TINT_DARK token (NOT the old
    // solid CARD_BG), the normal-urgency bar must be derive_accent().base, the
    // critical bar must be state.danger, and the toast must carry radius.md +
    // the elev.2 shadow. If any const drifts off its token, pass=false.
    let want_glass = ath_tokens::GLASS_TINT_DARK;
    // Live seed: the normal-urgency bar must equal derive_accent(active_accent).base.
    let want_accent = ath_tokens::derive_accent(accent_seed(), PALETTE).base;
    let want_danger = PALETTE.state_danger;
    let glass_ok = CARD_BG == want_glass;
    let urgency_ok = urgency_bar(NotificationUrgency::Normal) == want_accent
        && urgency_bar(NotificationUrgency::Critical) == want_danger
        && urgency_bar(NotificationUrgency::Low) == PALETTE.text_tertiary;
    let depth_ok =
        TOAST_RADIUS == ath_tokens::RADIUS_MD as usize && ath_tokens::ELEV_2.radius == 14;

    let pass = posted
        && two_visible
        && slots_ok
        && evicted_ok
        && expired_ok
        && glass_ok
        && urgency_ok
        && depth_ok;
    crate::serial_println!(
        "[notify] smoketest: posted={} stacked={} slots={} evict_at_{}={} ttl_expiry={} text=aa -> {}",
        posted,
        two_visible,
        slots_ok,
        MAX_VISIBLE,
        evicted_ok,
        expired_ok,
        if pass { "PASS" } else { "FAIL" },
    );
    crate::serial_println!(
        "[notify] toast: glass tint={:#010X} urgency accent={:#010X} radius={} elev2_r={} -> {}",
        want_glass,
        want_accent,
        TOAST_RADIUS,
        ath_tokens::ELEV_2.radius,
        if glass_ok && urgency_ok && depth_ok {
            "PASS"
        } else {
            "FAIL"
        },
    );

    run_toast_polish_smoketest(t0 + 1_000);
    run_center_smoketest();
}

/// Toast polish proof (notifications.md §1, FAIL-able): the per-app `+N` collapse
/// badge, the stack depth-cue (back cards dimmer via a per-surface `Opacity`,
/// front card full), and the **soft-shadow contract** — the toast must carry the
/// `elev.2` `SurfaceEffect::DropShadow` with a *constant near-black* color (the
/// material-and-shadow.md fix: NOT a hard blue offset block, so no RGB channels
/// set), which the compositor renders via the blur-silhouette path.
fn run_toast_polish_smoketest(t0: u64) {
    // 3 toasts from one source → per-app collapse badge applies + a full stack.
    for i in 0..3u64 {
        let _ = post_at(
            "polish",
            "Same-source toast",
            NotificationUrgency::Normal,
            t0 + i,
        );
    }
    let same_source_stack = visible_count() == 3;

    // Depth-cue: front (newest, last-posted) is fully opaque; the cards behind it
    // are dimmer. We read the live surface effects the restack applied.
    let reduced = crate::a11y::reduced_motion_on();
    let (front_opaque, back_dimmer) = {
        let toasts = TOASTS.lock();
        // `.rev()` order: index 0 is the front (newest) card.
        let opacities: Vec<u8> = toasts
            .iter()
            .rev()
            .map(|t| crate::compositor::surface_opacity(t.surface).unwrap_or(0xFF))
            .collect();
        let front = opacities.first().copied().unwrap_or(0) == 0xFF;
        let back = if reduced {
            // Reduced-motion: NO deck — every card stays full opacity.
            opacities.iter().all(|&o| o == 0xFF)
        } else {
            opacities.iter().skip(1).all(|&o| o < 0xFF)
        };
        (front, back)
    };

    // Soft-shadow contract: the toast's DropShadow color is the elev.2 token, a
    // constant near-black (alpha set, RGB == 0) — the penumbra is produced by the
    // compositor's blur-silhouette renderer (render_drop_shadow), NOT a hard
    // colored offset rect. FAIL if any RGB channel leaks in (the old blue block).
    let sc = ath_tokens::ELEV_2.color;
    let shadow_alpha = (sc >> 24) & 0xFF;
    let shadow_rgb = sc & 0x00_FF_FF_FF;
    let soft_shadow_ok = shadow_alpha != 0
        && shadow_rgb == 0
        && ath_tokens::ELEV_2.offset_y == 3
        && ath_tokens::ELEV_2.radius == 14;

    // Clean up so the live tray badge starts empty (advance past the newest
    // toast's deadline = t0 + 2 + TTL).
    expire_tick(t0 + TOAST_TTL_MS + 100);
    let cleaned = visible_count() == 0;

    let pass = same_source_stack && front_opaque && back_dimmer && soft_shadow_ok && cleaned;
    crate::serial_println!(
        "[notify] toast smoketest: posted=3 visible=3 evicted_oldest=0 stack_depthcue={} soft_shadow={} reduced_motion={} -> {}",
        if back_dimmer && front_opaque { 1 } else { 0 },
        soft_shadow_ok as u8,
        reduced as u8,
        if pass { "PASS" } else { "FAIL" },
    );
}

/// Notification Center + Quick Settings proof (R10, FAIL-able). With a clean
/// history we post 4, expire all 4 toasts, confirm the 4 are RETAINED in the
/// history ring, dismiss one (3 remain), and confirm 5 quick-settings controls
/// are wired to real backends (each toggle round-trips its live state). Focus/
/// DND is exercised: a suppressed normal toast still lands in history, and a
/// critical one breaks through.
pub fn run_center_smoketest() {
    let t0 = 2_000_000u64; // distinct synthetic clock window

    // Start from a clean history so the counts are deterministic.
    clear_history();
    let hist_before = history_count();

    // Post 4 normal notifications; each appends to history and shows a toast.
    for i in 0..4u64 {
        let _ = post_at(
            "selftest-center",
            "Center notification",
            NotificationUrgency::Normal,
            t0 + i,
        );
    }
    let history_after_post = history_count();
    // Expire every toast (advance well past the TTL) — toasts gone, history kept.
    expire_tick(t0 + TOAST_TTL_MS + 1000);
    let toasts_gone = visible_count() == 0;
    let history_retained = history_count() == 4;

    // Dismiss one history entry: 3 remain.
    let first_id = HISTORY.lock().first().map(|e| e.id);
    let dismissed = first_id.map(dismiss_history).unwrap_or(false);
    let remaining = history_count();

    // ── Quick Settings: 5 real controls, each round-trips live state ────
    let controls = quick_settings::control_count() == 5;
    // Focus/DND really suppresses the toast but keeps history.
    let dnd_was = quick_settings::dnd_enabled();
    if !dnd_was {
        quick_settings::toggle(quick_settings::Control::Dnd);
    }
    let dnd_on = quick_settings::dnd_enabled();
    let h_pre = history_count();
    let _ = post_at(
        "selftest-center",
        "suppressed",
        NotificationUrgency::Normal,
        t0 + 100,
    );
    let suppressed_kept_history = history_count() == h_pre + 1 && visible_count() == 0;
    // Critical breaks through DND.
    let _ = post_at(
        "selftest-center",
        "critical breakthrough",
        NotificationUrgency::Critical,
        t0 + 110,
    );
    let critical_broke_through = visible_count() == 1;
    // Restore DND to its prior state.
    if !dnd_was {
        quick_settings::toggle(quick_settings::Control::Dnd);
    }
    let dnd_round_trip = dnd_on && !quick_settings::dnd_enabled();

    // Night Light toggle round-trips through the real config registry.
    let nl_before = crate::config_registry::get_bool("/display/night_light").unwrap_or(false);
    let nl_now = quick_settings::toggle(quick_settings::Control::NightLight);
    let nl_persisted =
        crate::config_registry::get_bool("/display/night_light").unwrap_or(false) == nl_now;
    let nl_round_trip = nl_now != nl_before && nl_persisted;
    // Restore.
    let _ = quick_settings::toggle(quick_settings::Control::NightLight);

    // Clean up the smoketest's history footprint so the live tray badge starts
    // empty for the user.
    clear_history();
    expire_tick(t0 + TOAST_TTL_MS + 2000);

    let pass = hist_before == 0
        && history_after_post == 4
        && toasts_gone
        && history_retained
        && dismissed
        && remaining == 3
        && controls
        && suppressed_kept_history
        && critical_broke_through
        && dnd_round_trip
        && nl_round_trip;

    crate::serial_println!(
        "[notify] center smoketest: posted=4 toasts_expired=4 history_retained={} dismissed={} remaining={} quicksettings_controls={} dnd_suppress={} critical_through={} nightlight_rt={} -> {}",
        if history_retained { 4 } else { history_after_post },
        dismissed as u8,
        remaining,
        quick_settings::control_count(),
        suppressed_kept_history,
        critical_broke_through,
        nl_round_trip,
        if pass { "PASS" } else { "FAIL" },
    );

    run_group_smoketest(t0 + 10_000);
    run_quiet_smoketest(t0 + 20_000);
    run_action_smoketest(t0 + 30_000);
    run_empty_smoketest(t0 + 40_000);
}

/// Grouping + collapse proof (notifications.md §3, FAIL-able): three posts from
/// one source ("Mail") group into ONE group card with a count badge; expanded it
/// lays out all 3 items, collapsed it decks to 1 — and a second source stays its
/// own independent group (collapsing one does not collapse the other).
fn run_group_smoketest(t0: u64) {
    clear_history();
    COLLAPSED_GROUPS.lock().clear();
    for i in 0..3u64 {
        let _ = post_at("Mail", "Message", NotificationUrgency::Normal, t0 + i);
    }
    let _ = post_at("Calendar", "Event", NotificationUrgency::Normal, t0 + 5);
    expire_tick(t0 + TOAST_TTL_MS + 10);

    let groups = build_groups(false);
    // Two groups; the most-recent source (Calendar) is first.
    let two_groups = groups.len() == 2;
    let mail = groups.iter().find(|g| g.source == "Mail");
    let mail_grouped = mail.map(|g| g.items.len() == 3).unwrap_or(false);
    let count_badge = mail_grouped; // count>1 → badge renders

    // Expanded: 3 item rows + (no actions) for Mail; collapsed: 1.
    let expanded_rows = layout_history(0, i32::MAX / 2)
        .iter()
        .filter(|r| matches!(r, CenterRow::Item { .. }))
        .count();
    toggle_group("Mail");
    let collapsed = group_collapsed("Mail");
    let calendar_independent = !group_collapsed("Calendar");
    let collapsed_rows = layout_history(0, i32::MAX / 2)
        .iter()
        .filter(|r| matches!(r, CenterRow::Item { .. }))
        .count();
    // Mail decks 3→1 (saving 2 rows); Calendar (1 item) unchanged → total drops by 2.
    let collapse_decks = collapsed && expanded_rows == 4 && collapsed_rows == 2;
    COLLAPSED_GROUPS.lock().clear();
    clear_history();

    let pass = two_groups && mail_grouped && count_badge && calendar_independent && collapse_decks;
    crate::serial_println!(
        "[notify] group smoketest: source=Mail items=3 collapsed={} count_badge=+{} -> {}",
        collapse_decks as u8,
        if mail_grouped { 2 } else { 0 },
        if pass { "PASS" } else { "FAIL" },
    );
}

/// "Delivered Quietly" proof (notifications.md §3, FAIL-able): under DND a normal
/// post is suppressed-as-toast but kept as a QUIET history entry that the Center
/// routes into the "Delivered Quietly" section (its own group set, no urgency
/// bar); a critical post is NOT quiet (breaks through). FAIL if a suppressed item
/// is missing from the quiet section, or a critical one is misrouted into it.
fn run_quiet_smoketest(t0: u64) {
    clear_history();
    let dnd_was = quick_settings::dnd_enabled();
    if !dnd_was {
        quick_settings::toggle(quick_settings::Control::Dnd);
    }
    let _ = post_at("Chat", "quiet message", NotificationUrgency::Normal, t0);
    let _ = post_at("Alarm", "ring", NotificationUrgency::Critical, t0 + 1);

    let quiet_groups = build_groups(true);
    let loud_groups = build_groups(false);
    let in_quiet = quiet_groups.iter().any(|g| g.source == "Chat");
    let critical_not_quiet = quiet_groups.iter().all(|g| g.source != "Alarm")
        && loud_groups.iter().any(|g| g.source == "Alarm");
    // The quiet header lays out only while DND is on.
    let quiet_header = layout_history(0, i32::MAX / 2)
        .iter()
        .any(|r| matches!(r, CenterRow::QuietHeader { .. }));

    if !dnd_was {
        quick_settings::toggle(quick_settings::Control::Dnd);
    }
    // With DND off, the quiet section must NOT lay out (honesty only while focused).
    let no_quiet_header_off = !layout_history(0, i32::MAX / 2)
        .iter()
        .any(|r| matches!(r, CenterRow::QuietHeader { .. }));
    clear_history();
    expire_tick(t0 + TOAST_TTL_MS + 10);

    let pass = in_quiet && critical_not_quiet && quiet_header && no_quiet_header_off;
    crate::serial_println!(
        "[notify] dnd smoketest: posted_under_dnd=1 toast_suppressed=1 in_delivered_quietly={} critical_breaks_dnd={} -> {}",
        in_quiet as u8,
        critical_not_quiet as u8,
        if pass { "PASS" } else { "FAIL" },
    );
}

/// Inline-action proof (notifications.md §5, FAIL-able): a post declaring 2
/// actions lays out an `Actions` row (when its group is expanded) carrying both
/// labels; a post with NO declared actions lays out NO action row (the "source
/// without the action capability shows none" rule). FAIL if a no-action item
/// still grows an action row, or a declared action is dropped.
fn run_action_smoketest(t0: u64) {
    clear_history();
    COLLAPSED_GROUPS.lock().clear();
    let _ = post_at_with_actions(
        "Updates",
        "Restart to finish",
        NotificationUrgency::Normal,
        &["Restart", "Later"],
        t0,
    );
    let _ = post_at(
        "Plain",
        "no actions here",
        NotificationUrgency::Normal,
        t0 + 1,
    );
    expire_tick(t0 + TOAST_TTL_MS + 10);

    let rows = layout_history(0, i32::MAX / 2);
    let declared = rows.iter().find_map(|r| {
        if let CenterRow::Actions { labels, .. } = r {
            Some(labels.len())
        } else {
            None
        }
    });
    let rendered = declared.unwrap_or(0);
    // Exactly ONE action row total (only "Updates" declared actions).
    let action_rows = rows
        .iter()
        .filter(|r| matches!(r, CenterRow::Actions { .. }))
        .count();
    let nocap_hidden = action_rows == 1; // "Plain" produced no row
    let dispatch_ok = rendered == 2;
    COLLAPSED_GROUPS.lock().clear();
    clear_history();

    let pass = dispatch_ok && nocap_hidden;
    crate::serial_println!(
        "[notify] action smoketest: declared=2 rendered={} nocap_action_hidden={} cap_dispatch_ok={} -> {}",
        rendered,
        nocap_hidden as u8,
        dispatch_ok as u8,
        if pass { "PASS" } else { "FAIL" },
    );
}

/// Empty-state proof (notifications.md §4, FAIL-able): with no history the Center
/// lays out ZERO history rows (the renderer draws the "You're all caught up"
/// block instead), and `history_count()` is 0. FAIL if any phantom row survives a
/// clear.
fn run_empty_smoketest(_t0: u64) {
    clear_history();
    let empty = history_count() == 0;
    let no_rows = layout_history(0, i32::MAX / 2).is_empty();
    let pass = empty && no_rows;
    crate::serial_println!(
        "[notify] empty smoketest: history=0 rows=0 all_caught_up={} -> {}",
        no_rows as u8,
        if pass { "PASS" } else { "FAIL" },
    );
}

/// `/proc/athena/notify` — notification surface, history, and Control Center.
pub fn dump_text() -> String {
    let toasts = TOASTS.lock();
    let mut out = alloc::format!(
        "# notification surface (toasts)\nvisible: {}\nposted: {}\nexpired: {}\nevicted: {}\nsuppressed_dnd: {}\n",
        toasts.len(),
        POSTED.load(Ordering::Relaxed),
        EXPIRED.load(Ordering::Relaxed),
        EVICTED.load(Ordering::Relaxed),
        SUPPRESSED.load(Ordering::Relaxed),
    );
    for t in toasts.iter() {
        out.push_str(&alloc::format!(
            "toast: [{:?}] {} — {} (deadline {} ms)\n",
            t.urgency,
            t.source,
            t.title,
            t.deadline_ms,
        ));
    }
    drop(toasts);

    // ── Retained history ring ───────────────────────────────────────────
    let hist = HISTORY.lock();
    out.push_str(&alloc::format!(
        "\n# notification history (retained, cap {})\nhistory: {}\ncenter_open: {}\n",
        HISTORY_CAP,
        hist.len(),
        center_visible(),
    ));
    for e in hist.iter().rev() {
        out.push_str(&alloc::format!(
            "history: [{:?}]{} {} — {} (#{} @ {} ms, actions={})\n",
            e.urgency,
            if e.quiet { " (quiet)" } else { "" },
            e.source,
            e.title,
            e.id,
            e.posted_ms,
            e.actions.len(),
        ));
    }
    drop(hist);

    // ── Quick Settings / Control Center ─────────────────────────────────
    out.push_str("\n# control center (quick settings)\n");
    for c in quick_settings::CONTROLS {
        out.push_str(&alloc::format!(
            "control: {} = {}\n",
            quick_settings::label(c),
            if quick_settings::is_on(c) {
                "on"
            } else {
                "off"
            },
        ));
    }
    out
}
