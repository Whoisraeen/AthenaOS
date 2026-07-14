//! Visual notifications / caption stream (Concept §"accessibility by default,
//! not friction"). MasterChecklist Phase 19.2 — "Captions hooks (system audio →
//! caption surface)".
//!
//! Deaf and hard-of-hearing users cannot rely on system SOUNDS (alert chimes,
//! alarm tones, the connect/disconnect beep). Windows ships this as "SoundSentry
//! / visual notifications" and macOS as "screen flash on alert"; AthenaOS makes
//! it a kernel hook so EVERY sound-bearing system event also produces a readable
//! visual record.
//!
//! Two surfaces:
//!   * A **caption stream** — a ring of the most recent system announcements
//!     (source + text + time). Any sound-bearing event calls [`caption`]; the
//!     notification system mirrors every notification into it via
//!     [`on_notification`]. A future speech-to-text or media-subtitle source
//!     feeds the SAME hook.
//!   * A **visual-alert flag** — set when a Critical (alarm/security) event
//!     fires while visual alerts are on, so the compositor can flash the screen
//!     in place of the sound the user can't hear. [`take_visual_alert`] is the
//!     consume-once poll the compositor reads.
//!
//! All off by default (an exact no-op until a user enables it from Settings or
//! the Super+Alt+V shortcut), persisted via `config_registry` like the rest of
//! the a11y toggles. R10: `init` + `run_boot_smoketest` (FAIL-able) +
//! `/proc/athena/captions` + this docstring.

#![allow(dead_code)]

extern crate alloc;

use alloc::collections::VecDeque;
use alloc::string::{String, ToString};
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use spin::Mutex;

/// Config-registry key persisting the visual-alerts toggle (same mechanism as
/// `/a11y/reduced_motion` / `/a11y/sticky_keys`).
const VISUAL_ALERTS_CFG: &str = "/a11y/visual_alerts";

/// Most recent captions kept for the on-screen stream / `/proc/athena/captions`.
const RING_CAP: usize = 32;

/// One captioned system announcement.
#[derive(Clone, Debug)]
pub struct Caption {
    pub source: String,
    pub text: String,
    pub time_ms: u64,
    pub critical: bool,
}

struct CaptionState {
    enabled: bool,
    ring: VecDeque<Caption>,
}

static STATE: Mutex<CaptionState> = Mutex::new(CaptionState {
    enabled: false,
    ring: VecDeque::new(),
});

/// Total captions posted (diagnostics / procfs), independent of the bounded ring.
static POSTED: AtomicU64 = AtomicU64::new(0);
/// Set when a Critical event fires with visual alerts on; consumed by the
/// compositor's screen-flash. A plain bool is enough — flashes coalesce.
static VISUAL_ALERT_PENDING: AtomicBool = AtomicBool::new(false);

pub fn enabled() -> bool {
    STATE.lock().enabled
}

pub fn set_enabled(on: bool) {
    STATE.lock().enabled = on;
    crate::config_registry::set_bool(VISUAL_ALERTS_CFG, on);
    crate::serial_println!("[captions] visual alerts -> {}", on);
}

/// Toggle visual alerts and return the new state (the Super+Alt+V shortcut).
pub fn toggle() -> bool {
    let on = !enabled();
    set_enabled(on);
    on
}

/// Post a caption for a sound-bearing system event. A no-op (records nothing,
/// raises no flag) unless visual alerts are on, so callers can hook this
/// unconditionally with zero cost when the feature is off. `critical` requests a
/// screen flash (an alarm/security sound the user can't hear).
pub fn caption(source: &str, text: &str, critical: bool) {
    let now = crate::aurora::aurora_now_ms();
    let mut st = STATE.lock();
    if !st.enabled {
        return;
    }
    if st.ring.len() >= RING_CAP {
        st.ring.pop_front();
    }
    st.ring.push_back(Caption {
        source: source.to_string(),
        text: text.to_string(),
        time_ms: now,
        critical,
    });
    drop(st);
    POSTED.fetch_add(1, Ordering::Relaxed);
    if critical {
        VISUAL_ALERT_PENDING.store(true, Ordering::Relaxed);
    }
}

/// The notification system's hook: mirror every notification into the caption
/// stream so a deaf user reads what an alert sound would have signalled. Called
/// from `notify::post_at_with_actions`. `critical` is true for
/// `NotificationUrgency::Critical` (alarms/security → screen flash).
pub fn on_notification(source: &str, title: &str, critical: bool) {
    caption(source, title, critical);
}

/// Consume-once poll for the compositor's screen flash: returns `true` at most
/// once per Critical event, then clears. No-op churn when nothing is pending.
pub fn take_visual_alert() -> bool {
    VISUAL_ALERT_PENDING.swap(false, Ordering::Relaxed)
}

/// The most recent caption text (procfs / smoketest visibility), or empty.
pub fn last_caption() -> String {
    STATE
        .lock()
        .ring
        .back()
        .map(|c| c.text.clone())
        .unwrap_or_default()
}

pub fn caption_count() -> usize {
    STATE.lock().ring.len()
}

/// Re-apply the persisted visual-alerts toggle at boot.
pub fn apply_from_config() {
    if let Some(on) = crate::config_registry::get_bool(VISUAL_ALERTS_CFG) {
        STATE.lock().enabled = on;
    }
}

pub fn init() {
    apply_from_config();
    crate::serial_println!("[ OK ] Captions / visual alerts ready (default off)");
}

/// `/proc/athena/captions` — visual-alert state + the recent caption stream.
pub fn dump_text() -> String {
    let st = STATE.lock();
    let mut s = alloc::format!(
        "# captions / visual alerts\nenabled: {}\nposted: {}\nring_len: {}\nvisual_alert_pending: {}\n",
        st.enabled,
        POSTED.load(Ordering::Relaxed),
        st.ring.len(),
        VISUAL_ALERT_PENDING.load(Ordering::Relaxed),
    );
    for c in st.ring.iter().rev().take(8) {
        s.push_str(&alloc::format!(
            "  [{}ms] {}{}: {}\n",
            c.time_ms,
            if c.critical { "(!) " } else { "" },
            c.source,
            c.text,
        ));
    }
    s
}

/// FAIL-able proof on an isolated view of the state (saves + restores the live
/// enabled flag and ring so it never leaks a test caption onto a user's stream):
///   1. off = no-op: a caption while disabled records nothing,
///   2. on: a caption lands in the ring and is retrievable as `last_caption`,
///   3. a Critical caption raises the visual-alert flag, which `take_visual_alert`
///      consumes exactly once,
///   4. the ring is bounded (never grows past RING_CAP).
pub fn run_boot_smoketest() {
    // Snapshot live state.
    let (saved_enabled, saved_ring) = {
        let st = STATE.lock();
        (st.enabled, st.ring.clone())
    };
    let saved_alert = VISUAL_ALERT_PENDING.swap(false, Ordering::Relaxed);

    // Reset to a clean test view.
    {
        let mut st = STATE.lock();
        st.enabled = false;
        st.ring.clear();
    }

    // 1. Off = no-op.
    caption("test", "ignored while off", false);
    let off_noop = caption_count() == 0;

    // 2. On: a caption lands and is retrievable.
    set_enabled(true);
    caption("Wi-Fi", "Connected", false);
    let on_records = caption_count() == 1 && last_caption() == "Connected";

    // 3. Critical raises + consumes the visual-alert flag once.
    let _ = take_visual_alert(); // clear any
    caption("Alarm", "Timer done", true);
    let critical_flags = take_visual_alert() && !take_visual_alert();

    // 4. Ring is bounded.
    for i in 0..(RING_CAP + 10) {
        caption("flood", "x", false);
        let _ = i;
    }
    let bounded = caption_count() <= RING_CAP;

    // Restore live state.
    {
        let mut st = STATE.lock();
        st.enabled = saved_enabled;
        st.ring = saved_ring;
    }
    VISUAL_ALERT_PENDING.store(saved_alert, Ordering::Relaxed);

    let pass = off_noop && on_records && critical_flags && bounded;
    crate::serial_println!(
        "[captions] smoketest: off_noop={} on_records={} critical_flag={} bounded={} -> {}",
        off_noop,
        on_records,
        critical_flags,
        bounded,
        if pass { "PASS" } else { "FAIL" },
    );
}
