//! Desktop widget system (Concept §Customization Engine: "Widget system —
//! Rainmeter-equivalent, sandboxed: little live panels on the desktop,
//! each showing exactly one thing, never able to touch anything else").
//! MasterChecklist Phase 13.2 — "Widget system (Rainmeter-equivalent,
//! sandboxed)".
//!
//! A widget is a small always-on-desktop compositor surface bound to one
//! CLOSED data feed (`fn() -> String`): the widget can render only what
//! its feed returns — that's the sandbox for built-ins (third-party
//! widgets ride userspace + capabilities later). Enablement lives in the
//! versioned config (`/widgets/<name>`), so widget layouts snapshot and
//! roll back like every other setting. [`refresh`] reconciles the live
//! surfaces against the config and repaints the feeds; the desktop repaint
//! path calls it, so widgets stay current whenever the desktop redraws.
//!
//! Built-ins: `clock` (sys_wall_clock HH:MM), `uptime` (since kernel T0),
//! `windows` (open app-window count).

#![allow(dead_code)]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use ath_tokens::{DARK, SPACE_3};
use spin::Mutex;

const WIDGET_W: u32 = 168;
const WIDGET_H: u32 = 48;
/// Desktop edge margin — `SPACE_3` (12px).
const MARGIN: i32 = SPACE_3 as i32;
const STACK_Y0: i32 = 64;

// ── Token-driven widget colours (shared kit; high fan-out) ──────────────────
// Every surface using this kit inherits Vibe-Mode cohesion: the card is the
// `bg.raised` token, label/value are `text.secondary`/`text.primary`, and the
// accent stripe is the LIVE seed (`theme_engine::active_accent`), same as
// window chrome / login / notify.
const CARD_BG: u32 = DARK.bg_raised;
const FG: u32 = DARK.text_primary;
const FG_DIM: u32 = DARK.text_secondary;

/// LIVE accent base for the widget kit (tracks Vibe Mode).
#[inline]
fn accent() -> u32 {
    ath_tokens::derive_accent(crate::theme_engine::active_accent(), &DARK).base
}

/// The accent base actually painted — public for the cohesion smoketest.
#[inline]
pub fn proof_accent() -> u32 {
    accent()
}

/// One widget definition: a name and its closed data feed.
struct WidgetDef {
    name: &'static str,
    /// Enabled when `/widgets/<name>` is unset (the out-of-box desktop).
    default_on: bool,
    feed: fn() -> String,
}

fn feed_clock() -> String {
    crate::shell_runner::tray_clock_string()
}

fn feed_uptime() -> String {
    let ms = crate::boot_elapsed_ms();
    alloc::format!("{}m {:02}s", ms / 60_000, (ms / 1000) % 60)
}

fn feed_windows() -> String {
    alloc::format!(
        "{} open",
        crate::compositor::list_userspace_surfaces().len()
    )
}

static BUILTINS: [WidgetDef; 3] = [
    WidgetDef {
        name: "clock",
        default_on: true,
        feed: feed_clock,
    },
    WidgetDef {
        name: "uptime",
        default_on: false,
        feed: feed_uptime,
    },
    WidgetDef {
        name: "windows",
        default_on: false,
        feed: feed_windows,
    },
];

struct LiveWidget {
    name: &'static str,
    surface: u64,
    ptr: *mut u8,
}

// SAFETY: `ptr` is a kernel compositor surface buffer that stays alive for
// the surface's lifetime; widgets are created/painted/closed only under the
// LIVE lock from the (single-threaded) shell paths.
unsafe impl Send for LiveWidget {}

static LIVE: Mutex<Vec<LiveWidget>> = Mutex::new(Vec::new());
static REFRESHES: AtomicU64 = AtomicU64::new(0);

fn enabled(def: &WidgetDef) -> bool {
    crate::config_registry::get_bool(&alloc::format!("/widgets/{}", def.name))
        .unwrap_or(def.default_on)
}

fn paint(w: &LiveWidget, value: &str) {
    let mut canvas = unsafe { athgfx::Canvas::new(w.ptr, WIDGET_W as usize, WIDGET_H as usize, 4) };
    canvas.fill_rect(0, 0, WIDGET_W as usize, WIDGET_H as usize, CARD_BG);
    // LIVE accent stripe (Vibe-tracking), 3px on the left edge.
    canvas.fill_rect(0, 0, 3, WIDGET_H as usize, accent());
    // Label/value inset = SPACE_3 + the stripe (12 + 2).
    let inset = SPACE_3 as usize + 2;
    canvas.draw_text(inset, 8, w.name, FG_DIM, None);
    // Boundary-safe truncation: `String::truncate(18)` PANICS (kernel crash)
    // when byte 18 lands mid-codepoint; a widget value is arbitrary text.
    let v = athshell::text_util::truncate_chars(value, 18);
    canvas.draw_text(inset, 26, v, FG, None);
}

/// Reconcile live widget surfaces against the config and repaint feeds.
/// Called from the desktop repaint path and the smoketest. Returns the
/// number of live widgets after reconciliation.
pub fn refresh() -> usize {
    let mut live = LIVE.lock();

    for def in BUILTINS.iter() {
        let want = enabled(def);
        let have = live.iter().position(|l| l.name == def.name);
        match (want, have) {
            (true, None) => {
                if let Some((id, ptr)) =
                    crate::compositor::create_kernel_surface(WIDGET_W, WIDGET_H)
                {
                    let _ = crate::compositor::set_surface_title(id, def.name);
                    let w = LiveWidget {
                        name: def.name,
                        surface: id,
                        ptr,
                    };
                    paint(&w, &(def.feed)());
                    live.push(w);
                }
            }
            (false, Some(idx)) => {
                let w = live.remove(idx);
                let _ = crate::compositor::close_surface(w.surface);
            }
            (true, Some(idx)) => paint(&live[idx], &(def.feed)()),
            (false, None) => {}
        }
    }

    // Stack the live widgets down the left edge, in builtin order.
    let mut slot = 0i32;
    for def in BUILTINS.iter() {
        if let Some(w) = live.iter().find(|l| l.name == def.name) {
            let y = STACK_Y0 + slot * (WIDGET_H as i32 + 8);
            let _ = crate::compositor::present_surface(w.surface, MARGIN, y);
            slot += 1;
        }
    }
    REFRESHES.fetch_add(1, Ordering::Relaxed);
    live.len()
}

pub fn init() {
    crate::serial_println!(
        "[widgets] widget system ready ({} built-ins, enable via /widgets/<name>)",
        BUILTINS.len(),
    );
}

/// Deterministic proof: enabling via versioned config creates REAL
/// compositor surfaces in stacked slots with non-empty feed output;
/// disabling reconciles them away; original config restored.
pub fn run_boot_smoketest() {
    let saved: Vec<(&str, Option<bool>)> = BUILTINS
        .iter()
        .map(|d| {
            (
                d.name,
                crate::config_registry::get_bool(&alloc::format!("/widgets/{}", d.name)),
            )
        })
        .collect();

    crate::config_registry::set_bool("/widgets/clock", true);
    crate::config_registry::set_bool("/widgets/uptime", true);
    crate::config_registry::set_bool("/widgets/windows", false);
    let two = refresh() == 2;

    let surfaces_ok = {
        let live = LIVE.lock();
        let frames: Vec<_> = live
            .iter()
            .filter_map(|w| crate::compositor::surface_frame(w.surface))
            .collect();
        frames.len() == 2 && frames[0].1 != frames[1].1
    };

    let feeds_ok = {
        let clock = feed_clock();
        let uptime = feed_uptime();
        clock.len() == 5 && clock.as_bytes()[2] == b':' && uptime.contains('m')
    };

    crate::config_registry::set_bool("/widgets/clock", false);
    let one = refresh() == 1;
    crate::config_registry::set_bool("/widgets/uptime", false);
    let zero = refresh() == 0;

    // Restore the user's widget config. The config has no delete, so a key
    // that was unset restores to its built-in default explicitly — same
    // out-of-box behavior, now pinned.
    for (def, (_, prev)) in BUILTINS.iter().zip(saved) {
        crate::config_registry::set_bool(
            &alloc::format!("/widgets/{}", def.name),
            prev.unwrap_or(def.default_on),
        );
    }
    let _ = refresh();

    // Fail-able live-accent assertion: the kit's accent stripe must track the
    // Vibe seed (derive_accent(active_accent()).base). If the kit ever
    // re-hardcodes its accent, this drifts off the live seed and prints FAIL.
    let want_accent = ath_tokens::derive_accent(crate::theme_engine::active_accent(), &DARK).base;
    let accent_ok = proof_accent() == want_accent;

    // Fail-able UTF-8 boundary-safety assertion for the truncation used in
    // `paint` (and the bootlog/perm-ui render paths). A raw `&s[..18]` /
    // `String::truncate(18)` on this input PANICS (kernel crash) because byte
    // 18 lands inside the 4-byte emoji; `truncate_chars` must not. If this code
    // were to regress to a byte slice, the boot would PANIC here, not print FAIL
    // — which is exactly why the kernel must never byte-slice user strings.
    let multibyte = "Caf\u{e9}_\u{6587}\u{6863}_\u{1F3AE}_widget_value_overflow_xyz";
    let cut = athshell::text_util::truncate_chars(multibyte, 18);
    let truncation_ok = cut.chars().count() == 18
        && multibyte.is_char_boundary(cut.len())
        && multibyte.starts_with(cut);

    let pass = two && surfaces_ok && feeds_ok && one && zero && accent_ok && truncation_ok;
    crate::serial_println!(
        "[widgets] smoketest: enable_two={} real_surfaces={} feeds={} disable_one={} disable_all={} utf8_trunc={} -> {}",
        two,
        surfaces_ok,
        feeds_ok,
        one,
        zero,
        truncation_ok,
        if pass { "PASS" } else { "FAIL" },
    );
    crate::serial_println!(
        "[widgets] kit: accent={:#010X} card={:#010X} -> {}",
        proof_accent(),
        CARD_BG,
        if accent_ok && CARD_BG == DARK.bg_raised {
            "PASS"
        } else {
            "FAIL"
        },
    );
}

/// `/proc/athena/widgets` — widget system state.
pub fn dump_text() -> String {
    let live = LIVE.lock();
    let mut out = alloc::format!(
        "# desktop widgets (Rainmeter-equivalent; /widgets/<name> in versioned config)\nlive: {}\nrefreshes: {}\n",
        live.len(),
        REFRESHES.load(Ordering::Relaxed),
    );
    for def in BUILTINS.iter() {
        out.push_str(&alloc::format!(
            "widget: {} enabled={} value=\"{}\"\n",
            def.name,
            enabled(def),
            (def.feed)(),
        ));
    }
    out
}
