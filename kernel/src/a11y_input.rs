//! Accessibility keyboard filters — Concept §"Security & accessibility by
//! default, not friction" (the OS must be usable by people who cannot hold two
//! keys at once or who have a hand tremor). MasterChecklist Phase 19.3 —
//! "Sticky keys / slow keys / key-repeat tuning (`input.rs` filters)".
//!
//! Windows (Ease of Access) and macOS (Accessibility → Keyboard) both ship this
//! exact set; AthenaOS makes it a kernel input-pipeline stage so it works in
//! EVERY surface (login, shell, games, AthBridge guests) without per-app opt-in.
//!
//! The filter sits between the HID driver and the input event queue. Every
//! physical key flows through [`A11yKeyFilter::filter_key`], which returns the
//! LOGICAL events to actually enqueue. With all features off (the default) it is
//! an exact identity passthrough — zero behavior change — so it is always live
//! but invisible until a user enables a feature from Settings.
//!
//! Features:
//!   * **Sticky keys** — a modifier (Shift/Ctrl/Alt/Meta) latches for the NEXT
//!     key instead of having to be held (one-handed Ctrl+C). Double-tap LOCKS it
//!     held; tapping again unlocks. The filter decouples physical modifier
//!     up/down from the logical modifier state and synthesizes the logical
//!     events the shell sees.
//!   * **Slow keys** — a key must be held for a dwell time before it registers,
//!     filtering accidental brushes. A key released early is dropped; the
//!     surviving key-down is emitted from [`A11yKeyFilter::poll`] once the dwell
//!     elapses.
//!   * **Bounce keys** — a repeat of the SAME key within a debounce window is
//!     ignored (hand tremor / chattery switch).
//!   * **Key-repeat tuning** — configurable initial delay + repeat rate, queried
//!     by the auto-repeat generator via [`A11yKeyFilter::repeat_due`].
//!
//! R10: `init()` from `kernel_main`, `run_boot_smoketest()` (FAIL-able, drives
//! the pure state machine with synthetic timestamps), `/proc/raeen/a11y_keys`
//! via `vfs.rs`, and this docstring.

#![allow(dead_code)]

extern crate alloc;

use crate::input::KeyCode;
use alloc::vec::Vec;
use spin::Mutex;

// ── Modifier bitmask ────────────────────────────────────────────────────────

pub const MOD_SHIFT: u8 = 1 << 0;
pub const MOD_CTRL: u8 = 1 << 1;
pub const MOD_ALT: u8 = 1 << 2;
pub const MOD_GUI: u8 = 1 << 3;

/// Canonical synthetic key for each modifier bit (the left variant), used when
/// the filter injects a logical modifier the user did not physically hold.
const MOD_KEYS: [(u8, KeyCode); 4] = [
    (MOD_SHIFT, KeyCode::LeftShift),
    (MOD_CTRL, KeyCode::LeftCtrl),
    (MOD_ALT, KeyCode::LeftAlt),
    (MOD_GUI, KeyCode::LeftMeta),
];

/// Map a physical modifier key (either side) to its bit, or `None` for a
/// non-modifier key.
fn modifier_bit(code: KeyCode) -> Option<u8> {
    match code {
        KeyCode::LeftShift | KeyCode::RightShift => Some(MOD_SHIFT),
        KeyCode::LeftCtrl | KeyCode::RightCtrl => Some(MOD_CTRL),
        KeyCode::LeftAlt | KeyCode::RightAlt => Some(MOD_ALT),
        KeyCode::LeftMeta | KeyCode::RightMeta => Some(MOD_GUI),
        _ => None,
    }
}

/// Maximum gap between two taps of the same modifier to count as a double-tap
/// LOCK (matches the Windows/macOS sticky-keys default feel).
const DOUBLE_TAP_MS: u64 = 400;

// ── Config ──────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct A11yKeyConfig {
    pub sticky_keys: bool,
    /// Dwell a key must be held to register, in ms. `0` = off.
    pub slow_keys_ms: u32,
    /// Debounce window for a same-key repeat, in ms. `0` = off.
    pub bounce_keys_ms: u32,
    /// Initial delay before auto-repeat starts, in ms.
    pub repeat_delay_ms: u32,
    /// Interval between auto-repeats once started, in ms.
    pub repeat_rate_ms: u32,
}

impl Default for A11yKeyConfig {
    fn default() -> Self {
        // All assistive filters OFF; standard PC repeat cadence (250 ms / ~30 Hz).
        Self {
            sticky_keys: false,
            slow_keys_ms: 0,
            bounce_keys_ms: 0,
            repeat_delay_ms: 250,
            repeat_rate_ms: 33,
        }
    }
}

// ── Filter state machine (pure; no I/O, host-shaped for the smoketest) ───────

pub struct A11yKeyFilter {
    cfg: A11yKeyConfig,
    /// One-shot sticky modifiers active for the next non-modifier key.
    latched: u8,
    /// Locked (double-tapped) sticky modifiers, held until tapped again.
    locked: u8,
    /// (modifier bit, time) of the last modifier tap, for double-tap detection.
    last_tap: Option<(u8, u64)>,
    /// Slow-keys dwell in progress: (key, press time).
    pending_slow: Option<(KeyCode, u64)>,
    /// Last accepted key + time, for bounce debounce.
    last_accept: Option<(KeyCode, u64)>,
    // Diagnostics (procfs / smoketest visibility).
    pub bounced: u64,
    pub slow_rejected: u64,
    pub sticky_applied: u64,
}

impl A11yKeyFilter {
    pub const fn new(cfg: A11yKeyConfig) -> Self {
        Self {
            cfg,
            latched: 0,
            locked: 0,
            last_tap: None,
            pending_slow: None,
            last_accept: None,
            bounced: 0,
            slow_rejected: 0,
            sticky_applied: 0,
        }
    }

    pub fn config(&self) -> A11yKeyConfig {
        self.cfg
    }

    pub fn set_config(&mut self, cfg: A11yKeyConfig) {
        self.cfg = cfg;
        // Switching features off must not leave a modifier stuck logically down.
        if !cfg.sticky_keys {
            self.latched = 0;
            self.locked = 0;
            self.last_tap = None;
        }
        if cfg.slow_keys_ms == 0 {
            self.pending_slow = None;
        }
    }

    /// True when every assistive filter is off, so `filter_key` is a passthrough.
    fn all_off(&self) -> bool {
        !self.cfg.sticky_keys && self.cfg.slow_keys_ms == 0 && self.cfg.bounce_keys_ms == 0
    }

    /// Push `[latched-mod downs, (code,true), latched-mod ups]` and consume the
    /// one-shot latch. Locked modifiers are already logically held (emitted at
    /// lock time), so they are NOT re-synthesized here — only latched ones are.
    fn wrap_key_down(&mut self, code: KeyCode, out: &mut Vec<(KeyCode, bool)>) {
        let mods = self.latched;
        for (bit, kc) in MOD_KEYS.iter() {
            if mods & bit != 0 {
                out.push((*kc, true));
            }
        }
        out.push((code, true));
        for (bit, kc) in MOD_KEYS.iter().rev() {
            if mods & bit != 0 {
                out.push((*kc, false));
            }
        }
        if mods != 0 {
            self.sticky_applied += 1;
        }
        self.latched = 0;
        self.last_tap = None;
    }

    /// Filter one physical key transition; return the logical events to enqueue.
    pub fn filter_key(&mut self, code: KeyCode, down: bool, now_ms: u64) -> Vec<(KeyCode, bool)> {
        if self.all_off() {
            let mut out = Vec::with_capacity(1);
            out.push((code, down));
            return out;
        }
        if down {
            self.on_down(code, now_ms)
        } else {
            self.on_up(code, now_ms)
        }
    }

    fn on_down(&mut self, code: KeyCode, now: u64) -> Vec<(KeyCode, bool)> {
        let mut out = Vec::new();

        // Bounce: drop a same-key re-press inside the debounce window.
        if self.cfg.bounce_keys_ms > 0 {
            if let Some((last, t)) = self.last_accept {
                if last == code && now.saturating_sub(t) < self.cfg.bounce_keys_ms as u64 {
                    self.bounced += 1;
                    return out;
                }
            }
        }

        // Sticky modifier handling — physical modifier events are swallowed and
        // logical state is synthesized.
        if self.cfg.sticky_keys {
            if let Some(bit) = modifier_bit(code) {
                if self.locked & bit != 0 {
                    // Third tap → unlock; the modifier was logically held.
                    self.locked &= !bit;
                    self.last_tap = None;
                    for (b, kc) in MOD_KEYS.iter() {
                        if *b == bit {
                            out.push((*kc, false));
                        }
                    }
                    return out;
                }
                if let Some((lb, lt)) = self.last_tap {
                    if lb == bit && now.saturating_sub(lt) <= DOUBLE_TAP_MS {
                        // Second tap → lock held; emit the logical down.
                        self.locked |= bit;
                        self.latched &= !bit;
                        self.last_tap = None;
                        for (b, kc) in MOD_KEYS.iter() {
                            if *b == bit {
                                out.push((*kc, true));
                            }
                        }
                        return out;
                    }
                }
                // First tap → latch one-shot for the next key.
                self.latched |= bit;
                self.last_tap = Some((bit, now));
                return out;
            }
        }

        // Non-modifier key.
        if self.cfg.slow_keys_ms > 0 {
            // Defer: emitted from poll() once the dwell elapses (if still held).
            self.pending_slow = Some((code, now));
            return out;
        }

        self.wrap_key_down(code, &mut out);
        self.last_accept = Some((code, now));
        out
    }

    fn on_up(&mut self, code: KeyCode, now: u64) -> Vec<(KeyCode, bool)> {
        // Slow keys: a key released during its dwell is cancelled; if the dwell
        // had already elapsed (poll not yet run), honor it as a full down+up.
        if let Some((pc, pt)) = self.pending_slow {
            if pc == code {
                self.pending_slow = None;
                if now.saturating_sub(pt) >= self.cfg.slow_keys_ms as u64 {
                    let mut out = Vec::new();
                    self.wrap_key_down(code, &mut out);
                    out.push((code, false));
                    self.last_accept = Some((code, now));
                    return out;
                }
                self.slow_rejected += 1;
                return Vec::new();
            }
        }

        // Sticky: physical modifier releases are swallowed (logical state owns it).
        if self.cfg.sticky_keys && modifier_bit(code).is_some() {
            return Vec::new();
        }

        let mut out = Vec::with_capacity(1);
        out.push((code, false));
        out
    }

    /// Emit any slow-key whose dwell has now elapsed. The input poll loop calls
    /// this each tick with the current time; returns the logical key-down (and
    /// its sticky modifiers). The real key-up passes through later normally.
    pub fn poll(&mut self, now_ms: u64) -> Vec<(KeyCode, bool)> {
        if let Some((pc, pt)) = self.pending_slow {
            if self.cfg.slow_keys_ms > 0
                && now_ms.saturating_sub(pt) >= self.cfg.slow_keys_ms as u64
            {
                self.pending_slow = None;
                let mut out = Vec::new();
                self.wrap_key_down(pc, &mut out);
                self.last_accept = Some((pc, now_ms));
                return out;
            }
        }
        Vec::new()
    }

    /// Whether the `(emitted + 1)`-th auto-repeat is due for a key held for
    /// `held_ms`. First repeat fires at `repeat_delay_ms`, each subsequent one
    /// `repeat_rate_ms` later. The auto-repeat generator consults this so the
    /// user-tuned cadence (Settings → Accessibility) actually takes effect.
    pub fn repeat_due(&self, held_ms: u64, emitted: u32) -> bool {
        let delay = self.cfg.repeat_delay_ms as u64;
        let rate = self.cfg.repeat_rate_ms.max(1) as u64;
        if emitted == 0 {
            held_ms >= delay
        } else {
            held_ms >= delay + (emitted as u64) * rate
        }
    }

    /// Logical sticky-modifier state `(latched_one_shot, locked)` for procfs.
    pub fn sticky_state(&self) -> (u8, u8) {
        (self.latched, self.locked)
    }
}

// ── Global instance ──────────────────────────────────────────────────────────

static FILTER: Mutex<A11yKeyFilter> = Mutex::new(A11yKeyFilter::new(A11yKeyConfig {
    sticky_keys: false,
    slow_keys_ms: 0,
    bounce_keys_ms: 0,
    repeat_delay_ms: 250,
    repeat_rate_ms: 33,
}));

/// Filter one physical key transition through the live global filter. The HID
/// driver calls this instead of pushing `KeyDown`/`KeyUp` directly; it returns
/// the logical events to enqueue (identity passthrough when all filters are off).
pub fn filter_key(code: KeyCode, down: bool, now_ms: u64) -> Vec<(KeyCode, bool)> {
    FILTER.lock().filter_key(code, down, now_ms)
}

/// Drain any slow-key whose dwell elapsed (call from the input poll tick).
pub fn poll(now_ms: u64) -> Vec<(KeyCode, bool)> {
    FILTER.lock().poll(now_ms)
}

pub fn config() -> A11yKeyConfig {
    FILTER.lock().config()
}

pub fn set_config(cfg: A11yKeyConfig) {
    FILTER.lock().set_config(cfg);
    crate::serial_println!(
        "[a11y] key filters: sticky={} slow={}ms bounce={}ms repeat={}ms/{}ms",
        cfg.sticky_keys,
        cfg.slow_keys_ms,
        cfg.bounce_keys_ms,
        cfg.repeat_delay_ms,
        cfg.repeat_rate_ms,
    );
}

/// Config-registry key persisting the sticky-keys toggle (same mechanism as
/// `/a11y/reduced_motion` / high-contrast — a Settings panel or the global
/// Super+Alt+K shortcut writes it, `apply_from_config` reads it at boot).
const STICKY_KEYS_CFG: &str = "/a11y/sticky_keys";

pub fn set_sticky_keys(on: bool) {
    let mut cfg = config();
    cfg.sticky_keys = on;
    set_config(cfg);
    crate::config_registry::set_bool(STICKY_KEYS_CFG, on);
}

/// Toggle sticky keys and return the new state (the Super+Alt+K shortcut).
pub fn toggle_sticky_keys() -> bool {
    let on = !config().sticky_keys;
    set_sticky_keys(on);
    on
}

/// Re-apply persisted accessibility key settings from `config_registry` at boot
/// so a user who turned sticky keys on keeps it across a session. A no-op when
/// the key is absent (registry not up yet / never set) — the default stays off.
pub fn apply_from_config() {
    if let Some(on) = crate::config_registry::get_bool(STICKY_KEYS_CFG) {
        if on != config().sticky_keys {
            let mut cfg = config();
            cfg.sticky_keys = on;
            set_config(cfg);
        }
    }
}

pub fn set_slow_keys_ms(ms: u32) {
    let mut cfg = config();
    cfg.slow_keys_ms = ms;
    set_config(cfg);
}

pub fn set_bounce_keys_ms(ms: u32) {
    let mut cfg = config();
    cfg.bounce_keys_ms = ms;
    set_config(cfg);
}

pub fn set_key_repeat(delay_ms: u32, rate_ms: u32) {
    let mut cfg = config();
    cfg.repeat_delay_ms = delay_ms;
    cfg.repeat_rate_ms = rate_ms;
    set_config(cfg);
}

pub fn init() {
    apply_from_config();
    crate::serial_println!(
        "[ OK ] Accessibility key filters ready (sticky/slow/bounce/repeat, default off)"
    );
}

/// `/proc/raeen/a11y_keys` — current accessibility key-filter config + counters.
pub fn dump_text() -> alloc::string::String {
    let f = FILTER.lock();
    let cfg = f.config();
    let (latched, locked) = f.sticky_state();
    alloc::format!(
        "# accessibility keyboard filters\n\
         sticky_keys: {}\n\
         slow_keys_ms: {}\n\
         bounce_keys_ms: {}\n\
         repeat_delay_ms: {}\n\
         repeat_rate_ms: {}\n\
         latched_mods: 0x{:x}\n\
         locked_mods: 0x{:x}\n\
         bounced: {}\n\
         slow_rejected: {}\n\
         sticky_applied: {}\n",
        cfg.sticky_keys,
        cfg.slow_keys_ms,
        cfg.bounce_keys_ms,
        cfg.repeat_delay_ms,
        cfg.repeat_rate_ms,
        latched,
        locked,
        f.bounced,
        f.slow_rejected,
        f.sticky_applied,
    )
}

/// FAIL-able proof of every filter on a fresh, isolated `A11yKeyFilter` driven
/// with synthetic timestamps (no live keyboard, no clock dependency):
///   1. passthrough when all features are off,
///   2. sticky one-shot (Ctrl latches → wraps the next key → clears),
///   3. sticky double-tap LOCK then UNLOCK,
///   4. slow keys: brief press dropped, held-past-dwell key emitted via poll,
///   5. bounce: a same-key re-press inside the window is dropped,
///   6. key-repeat schedule honors the configured delay + rate.
pub fn run_boot_smoketest() {
    use KeyCode::*;

    // 1. Passthrough (all off).
    let mut f = A11yKeyFilter::new(A11yKeyConfig::default());
    let passthrough = f.filter_key(C, true, 0) == alloc::vec![(C, true)]
        && f.filter_key(C, false, 1) == alloc::vec![(C, false)];

    // 2. Sticky one-shot: Ctrl latches, C wraps with Ctrl, V gets nothing.
    let mut f = A11yKeyFilter::new(A11yKeyConfig {
        sticky_keys: true,
        ..A11yKeyConfig::default()
    });
    let ctrl_swallowed = f.filter_key(LeftCtrl, true, 0).is_empty();
    let c_wrapped =
        f.filter_key(C, true, 10) == alloc::vec![(LeftCtrl, true), (C, true), (LeftCtrl, false)];
    let v_plain = f.filter_key(V, true, 20) == alloc::vec![(V, true)];
    let sticky_oneshot = ctrl_swallowed && c_wrapped && v_plain;

    // 3. Double-tap LOCK then UNLOCK.
    let mut f = A11yKeyFilter::new(A11yKeyConfig {
        sticky_keys: true,
        ..A11yKeyConfig::default()
    });
    let _ = f.filter_key(LeftShift, true, 100); // first tap (latch)
    let _ = f.filter_key(LeftShift, false, 110);
    let lock_down = f.filter_key(LeftShift, true, 150) == alloc::vec![(LeftShift, true)]; // 2nd tap → lock
    let a_while_locked = f.filter_key(A, true, 160) == alloc::vec![(A, true)]; // mod already held
    let unlock_up = f.filter_key(LeftShift, true, 200) == alloc::vec![(LeftShift, false)]; // 3rd tap → unlock
    let sticky_lock = lock_down && a_while_locked && unlock_up;

    // 4. Slow keys (dwell 200 ms): brief press dropped; a held key emerges via poll.
    let mut f = A11yKeyFilter::new(A11yKeyConfig {
        slow_keys_ms: 200,
        ..A11yKeyConfig::default()
    });
    let _ = f.filter_key(A, true, 1000); // defer
    let brief_drop = f.filter_key(A, false, 1050).is_empty() // released at 50 ms < dwell
        && f.poll(1300).is_empty(); // nothing pending after cancel
    let _ = f.filter_key(B, true, 2000); // defer
    let no_early = f.poll(2100).is_empty(); // only 100 ms elapsed
    let held_emerges = f.poll(2200) == alloc::vec![(B, true)]; // dwell satisfied
    let slow_keys = brief_drop && no_early && held_emerges;

    // 5. Bounce (debounce 50 ms): a same-key re-press inside the window drops.
    let mut f = A11yKeyFilter::new(A11yKeyConfig {
        bounce_keys_ms: 50,
        ..A11yKeyConfig::default()
    });
    let first = f.filter_key(A, true, 3000) == alloc::vec![(A, true)];
    let chatter = f.filter_key(A, true, 3020).is_empty(); // 20 ms < 50 ms → dropped
    let after_window = f.filter_key(A, true, 3100) == alloc::vec![(A, true)]; // 100 ms ok
    let bounce_keys = first && chatter && after_window;

    // 6. Key-repeat schedule (delay 250, rate 33).
    let f = A11yKeyFilter::new(A11yKeyConfig {
        repeat_delay_ms: 250,
        repeat_rate_ms: 33,
        ..A11yKeyConfig::default()
    });
    let repeat_tuning = !f.repeat_due(200, 0) // before initial delay
        && f.repeat_due(250, 0) // first repeat at delay
        && f.repeat_due(283, 1) // second at delay + rate
        && !f.repeat_due(282, 1);

    // 7. Live toggle round-trip on the GLOBAL filter (the Super+Alt+K path),
    // restoring the real state after so the boot keyboard is never left sticky.
    let saved = config().sticky_keys;
    let toggled = toggle_sticky_keys();
    set_sticky_keys(saved);
    let toggle_ok = toggled == !saved && config().sticky_keys == saved;

    let pass = passthrough
        && sticky_oneshot
        && sticky_lock
        && slow_keys
        && bounce_keys
        && repeat_tuning
        && toggle_ok;
    crate::serial_println!(
        "[a11y] key-filter smoketest: passthrough={} sticky_oneshot={} sticky_lock={} slow_keys={} bounce={} repeat={} toggle={} -> {}",
        passthrough,
        sticky_oneshot,
        sticky_lock,
        slow_keys,
        bounce_keys,
        repeat_tuning,
        toggle_ok,
        if pass { "PASS" } else { "FAIL" },
    );
}
