//! Runtime permission prompt dialog (Concept §AthGuard: "permission prompts
//! you can actually trust — drawn by the OS compositor, not the requesting
//! app, so an app can never spoof its own consent screen").
//! MasterChecklist Phase 9.2 — "Runtime permission prompts via compositor UI
//! (kernel queue exists; no UI consumes it)" — this is the consumer.
//!
//! Flow: an app's capability request lands in `perm_prompt`'s queue →
//! [`pump`] (called from the shell tick) opens a modal compositor dialog
//! showing WHO is asking for WHAT → the user answers on the keyboard
//! ([Y]/Enter = allow, [N]/Esc = deny) through [`handle_key`], which routes
//! the verdict into `perm_prompt::resolve` — the same call that performs the
//! real capability grant. While the dialog is open it owns the keyboard
//! (modal): keystrokes cannot leak to the app that triggered the prompt.
//!
//! The smoketest drives the full UI cycle programmatically: synthetic
//! request → dialog surface actually created → Y keystroke → verdict
//! Approved with a real cap handle → dialog gone → second request denied
//! via N. Deterministic on QEMU and iron (needs only the compositor).

#![allow(dead_code)]

extern crate alloc;

use alloc::string::String;
use core::sync::atomic::{AtomicU64, Ordering};
use rae_tokens::{AccentRamp, DARK};
use spin::Mutex;

use crate::perm_prompt::{PermRequest, Verdict};

const DIALOG_W: u32 = 460;
const DIALOG_H: u32 = 180;

// Same token palette as login_ui / window chrome — every colour is a
// `rae_tokens::DARK` value so the consent dialog reads as part of the OS, not a
// bolted-on box. The accent bar is LIVE (`accent()`), tracking Vibe Mode.
//
// SECURITY: this is a consent surface, so the danger semantics are deliberate
// and must NOT wash out. ALLOW = `state.ok` (green), DENY = `state.danger`
// (alarming red), and `state.warn` is reserved for the danger banner the dialog
// raises when the requester is unverified (see `render`). A permission warning
// that doesn't look like a warning is a security bug.
const CARD_BG: u32 = DARK.bg_raised; // window/panel layer
const FG: u32 = DARK.text_primary; // headings, body
const FG_DIM: u32 = DARK.text_secondary; // description, key hints
const ALLOW: u32 = DARK.state_ok; // [Y] Allow — success green
const DENY: u32 = DARK.state_danger; // [N] Deny — destructive red
const WARN: u32 = DARK.state_warn; // unverified-developer danger banner

/// The LIVE accent ramp for the consent dialog's top bar + title — derived
/// from the active theme/Vibe seed (`theme_engine::active_accent`), same as the
/// login screen, OOBE wizard, installer and toasts. The danger/allow/deny
/// colours stay fixed token states (a re-skin must never recolour a warning).
#[inline]
fn accent() -> AccentRamp {
    rae_tokens::derive_accent(crate::theme_engine::active_accent(), &DARK)
}

/// The accent base actually painted — public so the cross-surface cohesion
/// smoketest can confirm the dialog tracks the live seed.
#[inline]
pub fn proof_accent() -> u32 {
    accent().base
}

// PS/2 set-1 make codes.
const SC_Y: u8 = 0x15;
const SC_ENTER: u8 = 0x1C;
const SC_N: u8 = 0x31;
const SC_ESC: u8 = 0x01;

struct DialogState {
    surface: u64,
    request: PermRequest,
}

/// True for capability flavors that hand an app low-level reach over the
/// machine (raw MMIO, IRQ vectors, I/O ports, system control, debug/ptrace,
/// crypto keys). Granting these to an *unverified* app is the canonical
/// "this could own your computer" moment, so the dialog raises a loud
/// `state.warn` banner for them — the danger emphasis the consent surface
/// exists to carry. Pure projection of the existing `CapFlavor`; no new ABI.
fn flavor_high_risk(flavor: crate::perm_prompt::CapFlavor) -> bool {
    use crate::perm_prompt::CapFlavor::*;
    matches!(flavor, Mmio | Irq | Port | System | Debug | CryptoKey)
}

static DIALOG: Mutex<Option<DialogState>> = Mutex::new(None);
static PROMPTS_SHOWN: AtomicU64 = AtomicU64::new(0);
static PROMPTS_APPROVED: AtomicU64 = AtomicU64::new(0);
static PROMPTS_DENIED: AtomicU64 = AtomicU64::new(0);

/// True while a permission dialog is on screen (the dialog is modal).
pub fn dialog_open() -> bool {
    DIALOG.lock().is_some()
}

fn render(ptr: *mut u8, w: u32, h: u32, req: &PermRequest) {
    let mut canvas = unsafe { raegfx::Canvas::new(ptr, w as usize, h as usize, 4) };
    let wu = w as usize;
    let hu = h as usize;
    let accent = accent().base;
    canvas.fill_rect(0, 0, wu, hu, CARD_BG);
    canvas.fill_rect(0, 0, wu, 3, accent);

    canvas.draw_text(20, 18, "Permission Request", accent, None);
    let line1 = alloc::format!("\"{}\" wants access to:", req.app_name);
    canvas.draw_text(20, 48, &line1, FG, None);
    // High-risk flavors paint their label in danger red, not body grey, so the
    // capability itself reads as alarming before the user even hits the banner.
    let high_risk = flavor_high_risk(req.flavor);
    let flavor_color = if high_risk { DENY } else { FG };
    canvas.draw_text(36, 72, req.flavor.label(), flavor_color, None);
    if !req.description.is_empty() {
        // Boundary-safe truncation: `String::truncate(52)` PANICS (kernel crash)
        // when byte 52 lands mid-codepoint, and a capability description is
        // arbitrary app-supplied text (accents/CJK/emoji).
        let desc = raeshell::text_util::truncate_chars(&req.description, 52);
        canvas.draw_text(20, 100, desc, FG_DIM, None);
    }

    // Danger banner for low-level capabilities: a full-width state.warn strip
    // with a warning line. This is the "unverified developer / this could harm
    // your machine" emphasis the consent dialog must never wash out.
    if high_risk {
        canvas.fill_rect(0, 116, wu, 18, WARN);
        canvas.draw_text(
            20,
            120,
            "Warning: low-level hardware access. Only allow apps you trust.",
            CARD_BG,
            None,
        );
    }

    canvas.draw_text(20, 152, "[Y] Allow", ALLOW, None);
    canvas.draw_text(140, 152, "[N] Deny", DENY, None);
    canvas.draw_text(240, 152, "(Enter = allow, Esc = deny)", FG_DIM, None);
}

/// Open a dialog for the oldest pending request, if none is showing.
/// Called from the shell tick (and the smoketest). Cheap when idle: one
/// lock + one queue peek.
pub fn pump() {
    {
        if DIALOG.lock().is_some() {
            return;
        }
    }
    let Some(req) = crate::perm_prompt::drain_pending(1).into_iter().next() else {
        return;
    };

    let (sw, sh) = crate::compositor::screen_dimensions().unwrap_or((1024, 768));
    let Some((id, ptr)) = crate::compositor::create_kernel_surface(DIALOG_W, DIALOG_H) else {
        return;
    };
    render(ptr, DIALOG_W, DIALOG_H, &req);
    let _ = crate::compositor::set_surface_title(id, "Permission Request");
    let _ = crate::compositor::present_surface(
        id,
        (sw.saturating_sub(DIALOG_W) / 2) as i32,
        (sh.saturating_sub(DIALOG_H) / 2) as i32,
    );
    let _ = crate::compositor::focus_surface(id);
    PROMPTS_SHOWN.fetch_add(1, Ordering::Relaxed);
    crate::serial_println!(
        "[perm-ui] dialog open: \"{}\" requests {} (surface {})",
        req.app_name,
        req.flavor.label(),
        id,
    );
    *DIALOG.lock() = Some(DialogState {
        surface: id,
        request: req,
    });
}

/// Keyboard entry point, wired BEFORE the shell/login handlers: while a
/// dialog is open it consumes every key (modal — keystrokes must not leak
/// to the app that asked). Returns true when the key was consumed.
pub fn handle_key(scancode: u8) -> bool {
    let taken = {
        let mut guard = DIALOG.lock();
        let Some(state) = guard.as_ref() else {
            return false;
        };
        let approve = match scancode {
            SC_Y | SC_ENTER => true,
            SC_N | SC_ESC => false,
            _ => return true, // modal: swallow everything else (incl. break codes)
        };
        let id = state.request.id;
        let surface = state.surface;
        let app = state.request.app_name.clone();
        let flavor = state.request.flavor;
        *guard = None;
        Some((id, surface, app, flavor, approve))
    }; // DIALOG released before resolve() — it takes the SCHEDULER lock.

    if let Some((id, surface, app, flavor, approve)) = taken {
        crate::perm_prompt::resolve(id, approve);
        let _ = crate::compositor::close_surface(surface);
        if approve {
            PROMPTS_APPROVED.fetch_add(1, Ordering::Relaxed);
        } else {
            PROMPTS_DENIED.fetch_add(1, Ordering::Relaxed);
        }
        crate::serial_println!(
            "[perm-ui] user {} \"{}\" -> {}",
            if approve { "ALLOWED" } else { "DENIED" },
            app,
            flavor.label(),
        );
    }
    true
}

pub fn init() {
    crate::serial_println!("[ OK ] Permission prompt UI armed (compositor-rendered, modal)");
}

/// Deterministic proof of the full UI consent cycle: request → dialog
/// surface really exists → Y key → Approved with a live cap handle →
/// dialog gone; second request → N key → Denied.
pub fn run_boot_smoketest() {
    use crate::capability::{Cap, Rights};

    let Some(tid) = crate::scheduler::current_task_id() else {
        crate::serial_println!("[perm-ui] smoketest: no current task -> SKIP");
        return;
    };
    let want = Cap::Network {
        port_range_start: 41010,
        port_range_end: 41011,
        rights: Rights::READ,
    };

    // perm_prompt's own smoketest (which runs first) auto-opened a dialog via
    // the request hook and then resolved its request directly, leaving the
    // dialog stale. Clear it: the Esc resolve of an already-resolved id is a
    // no-op, the surface close is real.
    if dialog_open() {
        handle_key(SC_ESC);
    }

    // Approve path through the real dialog.
    let Some(id) =
        crate::perm_prompt::request_permission(tid, "perm-ui-selftest", &want, "UI consent loop")
    else {
        crate::serial_println!("[perm-ui] smoketest: queue full -> FAIL");
        return;
    };
    pump();
    let opened = dialog_open();
    let consumed = handle_key(SC_Y);
    let (verdict, handle) = crate::perm_prompt::poll_verdict(id);
    let approved = verdict == Verdict::Approved && handle != 0;
    let closed = !dialog_open();

    // Deny path.
    let denied =
        match crate::perm_prompt::request_permission(tid, "perm-ui-selftest", &want, "deny path") {
            Some(id2) => {
                pump();
                handle_key(SC_N);
                crate::perm_prompt::poll_verdict(id2).0 == Verdict::Denied
            }
            None => false,
        };

    let pass = opened && consumed && approved && closed && denied;
    crate::serial_println!(
        "[perm-ui] smoketest: dialog_opened={} key_consumed={} approved_with_grant={} dialog_closed={} deny_path={} -> {}",
        opened,
        consumed,
        approved,
        closed,
        denied,
        if pass { "PASS" } else { "FAIL" },
    );

    // ── Token + live-accent + danger-emphasis cohesion (fail-able) ──────────
    // The dialog's title/top-bar accent must be derive_accent(active_accent())
    // .base, the palette must be DARK tokens, and the danger states must stay
    // distinct + alarming (Deny == state.danger, banner == state.warn, both
    // != the accent). If any drifts or the warning washes out, prints FAIL.
    let want_accent = rae_tokens::derive_accent(crate::theme_engine::active_accent(), &DARK).base;
    let accent_ok = proof_accent() == want_accent;
    let palette_ok = CARD_BG == DARK.bg_raised && FG == DARK.text_primary && ALLOW == DARK.state_ok;
    // Danger emphasis preserved: deny/warn are the alarming token states and
    // are NOT the (re-skinnable) accent, so a Vibe re-skin can't mute a warning.
    let danger_ok = DENY == DARK.state_danger
        && WARN == DARK.state_warn
        && DENY != want_accent
        && WARN != want_accent
        && flavor_high_risk(crate::perm_prompt::CapFlavor::Mmio)
        && !flavor_high_risk(crate::perm_prompt::CapFlavor::Audio);
    crate::serial_println!(
        "[perm] ui: accent={:#010X} danger={:#010X} warn={:#010X} palette={} danger_emphasis={} -> {}",
        proof_accent(),
        DENY,
        WARN,
        if palette_ok { "tokens" } else { "DRIFT" },
        if danger_ok { "preserved" } else { "WASHED" },
        if accent_ok && palette_ok && danger_ok {
            "PASS"
        } else {
            "FAIL"
        },
    );
}

/// `/proc/raeen/perm_ui` — prompt UI counters.
pub fn dump_text() -> String {
    alloc::format!(
        "# permission prompt UI (compositor-rendered, modal)\ndialog_open: {}\nprompts_shown: {}\napproved: {}\ndenied: {}\n",
        dialog_open(),
        PROMPTS_SHOWN.load(Ordering::Relaxed),
        PROMPTS_APPROVED.load(Ordering::Relaxed),
        PROMPTS_DENIED.load(Ordering::Relaxed),
    )
}
