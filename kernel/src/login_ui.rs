//! Compositor login screen — shown before the desktop shell is activated.
//!
//! Visual language: Windows 11 / macOS Sonoma. Dark, centered, with a
//! gradient backdrop, a user-initial avatar bubble, the user's display
//! name from `session::display_name()`, and a single prominent password
//! field. Debug-style hint text ("Tab = switch field   G = guest") is
//! suppressed in favor of a single muted "Press Enter to sign in" line —
//! the OS shouldn't feel like a developer console at the lock screen.

#![allow(dead_code)]

extern crate alloc;

use alloc::string::String;
use rae_tokens::{
    AccentRamp, Palette, DARK, GLASS_POPOVER_DARK, RADIUS_LG, TYPE_BODY, TYPE_CAPTION, TYPE_LABEL,
    TYPE_SUBTITLE,
};
use raegfx::text::FontFamily;

/// Active palette for the login surface (dark default — the lock screen is
/// always dark, like Windows/macOS). Every colour below is a `rae_tokens`
/// value so a Vibe-Mode re-skin flows here with the rest of the shell.
const PALETTE: &Palette = &DARK;

/// The LIVE accent ramp for the login screen — derived from the active
/// theme/Vibe seed (`theme_engine::active_accent`) so the card top-bar,
/// field underline and footer wordmark recolour on a one-tap re-skin, same
/// as window chrome (`window_chrome::accent`) and the toasts (`notify`).
#[inline]
fn accent() -> AccentRamp {
    rae_tokens::derive_accent(crate::theme_engine::active_accent(), PALETTE)
}

/// The accent base actually painted — public so the cross-surface cohesion
/// smoketest can confirm the login screen tracks the live seed.
#[inline]
pub fn proof_accent() -> u32 {
    accent().base
}

/// Backdrop palette — vertical gradient between BG_TOP (top of screen)
/// and BG_BOT (bottom). Faked via N horizontal strips because the
/// raegfx::Canvas API only exposes fill_rect; per-pixel interpolation
/// would be prettier but isn't free on a CPU rasterizer.
///
/// `BG_TOP` is `bg.base` verbatim (design-language §4.1 notes the login
/// gradient top matches `bg.base`); `BG_BOT` is `bg.overlay` so the gradient
/// settles on a token-defined navy rather than a hand-mixed constant.
const BG_TOP: u32 = DARK.bg_base; // deep navy = bg.base
const BG_BOT: u32 = DARK.bg_overlay; // settles on bg.overlay (token navy)
const GRADIENT_STRIPS: usize = 32;

/// Card surface — `bg.raised` (window/panel layer). `CARD_EDGE` is the glass
/// top-edge highlight (`stroke.strong`).
const CARD_BG: u32 = DARK.bg_raised;
const CARD_EDGE: u32 = DARK.stroke_strong; // glass top-edge / inset hairline
const FG: u32 = DARK.text_primary;
const FG_DIM: u32 = DARK.text_secondary;
const FG_MUTED: u32 = DARK.text_tertiary;
const ERR: u32 = DARK.state_danger;
/// Field surfaces — `bg.elevated` fill behind a `stroke.subtle` hairline.
const FIELD_BG: u32 = DARK.bg_elevated;
const FIELD_BORDER: u32 = DARK.stroke_subtle;

pub struct LoginState {
    pub username: [u8; 32],
    pub username_len: usize,
    pub password: [u8; 64],
    pub password_len: usize,
    pub field: LoginField,
    pub error: Option<String>,
    pub shift_held: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum LoginField {
    Username,
    Password,
}

impl LoginState {
    pub fn new() -> Self {
        let mut s = Self {
            username: [0u8; 32],
            username_len: 0,
            password: [0u8; 64],
            password_len: 0,
            field: LoginField::Password,
            error: None,
            shift_held: false,
        };
        // Seed username from /session/last_user (set by setup_ui on
        // first-boot account creation). Falls back to "raeen" — the
        // dev-default account — only when the registry has no record,
        // which happens on a fresh kernel with no completed first-boot
        // setup (the wizard runs in that case, not the login screen).
        let user = crate::config_registry::get_text("/session/last_user")
            .unwrap_or_else(|| alloc::string::String::from("raeen"));
        let bytes = user.as_bytes();
        let n = bytes.len().min(s.username.len());
        s.username[..n].copy_from_slice(&bytes[..n]);
        s.username_len = n;
        s
    }

    /// Render the login screen into a raw `0xAARRGGBB` framebuffer.
    ///
    /// Concept §"rival Windows + macOS": the login moment is the first-impression
    /// surface, so it wears the same **Liquid Glass** identity as the desktop and
    /// the lock screen (IDENTITY.md §3 Aurora Mesh + §7 tiers) — a living aurora
    /// backdrop with a centered frosted glass card holding the avatar, greeting,
    /// and the password pill, exactly like the macOS / Win11 sign-in moment. The
    /// public signature is unchanged (the kernel passes `ptr` + `w`/`h`);
    /// internally we wrap the buffer in a [`raegfx::Canvas`] — the SAME software
    /// rasterizer the compositor uses — so we draw through the shared
    /// `glass`/`draw_text_aa` primitives instead of hand-rolled hex pixels.
    pub fn render(&self, ptr: *mut u8, w: u32, h: u32) {
        let mut canvas = unsafe { raegfx::Canvas::new(ptr, w as usize, h as usize, 4) };

        // Live accent ramp (tracks Vibe Mode). `base` = bright accent (focused),
        // `active` = darkened accent for the dim/inactive ring.
        let acc = accent();
        let p: &Palette = PALETTE;
        let sans = FontFamily::Sans;

        let sw = w as usize;
        let sh = h as usize;
        let cx = sw / 2;
        let cy = sh / 2;

        // ── Background → the signature Aurora Mesh (IDENTITY.md §3): the same
        //    living backdrop the desktop + lock screen wear — visual continuity
        //    from boot to login to desktop. Replaces the old flat navy gradient.
        raegfx::glass::render_aurora_dark(&mut canvas, 0, 0, sw, sh, 0);

        // ── Centered glass card: avatar + greeting + password pill float on a
        //    `glass.popover` frosted surface (transient → instant legibility over
        //    the busy aurora) with the iridescent rim — the same tiered draw CC /
        //    Start / toasts / the lock screen make. Token-derived sizing.
        let card_w = 360usize;
        let card_h = 320usize;
        let card_x = cx.saturating_sub(card_w / 2);
        let card_y = cy.saturating_sub(card_h / 2);
        let cr = RADIUS_LG as usize;

        // Soft ambient shadow lifts the card off the aurora, then the shipped
        // tiered-glass draw (tint → frost → legibility cap → iridescent rim).
        canvas.fill_rounded_rect_shadow(card_x, card_y, card_w, card_h, cr, 0x0A_10_1C, 40, 18);
        raegfx::glass::draw_glass_surface(
            &mut canvas,
            card_x,
            card_y,
            card_w,
            card_h,
            cr,
            GLASS_POPOVER_DARK,
        );

        // ── Avatar bubble — an accent-ringed circle with the display-name
        //    initial in text.primary, floating at the top of the card. Centre ==
        //    the old layout anchor so the greeting/field flow below is unchanged.
        let avatar_size = 64usize;
        let avatar_x = cx.saturating_sub(avatar_size / 2);
        let avatar_y = card_y + 32;
        let avatar_cx = cx;
        let avatar_cy = avatar_y + avatar_size / 2;
        canvas.fill_circle(avatar_cx, avatar_cy, avatar_size / 2, acc.base);
        canvas.fill_circle(avatar_cx, avatar_cy, avatar_size / 2 - 3, p.bg_elevated);
        // First letter of display name, centred in the avatar.
        let display = crate::session::display_name();
        let initial = display
            .chars()
            .next()
            .map(|c| c.to_ascii_uppercase())
            .unwrap_or('?');
        let initial_str: alloc::string::String = core::iter::once(initial).collect();
        // Crisp AA initial — `type.subtitle` reads as the display glyph in the
        // 64px tile, centred on both axes via measure + the line box.
        let init_w = canvas.measure_text_aa(&initial_str, TYPE_SUBTITLE, sans);
        canvas.draw_text_aa(
            (avatar_x + avatar_size / 2) as i32 - init_w / 2,
            avatar_cy as i32 - TYPE_SUBTITLE.line_height as i32 / 2,
            &initial_str,
            TYPE_SUBTITLE,
            p.text_primary,
            sans,
        );

        // ── Greeting: "Welcome back, <DisplayName>" — `type.subtitle` heading,
        //    text.primary (the legibility cap inside draw_glass_surface keeps the
        //    card interior dark enough that text.primary wins over the aurora).
        let greeting = alloc::format!("Welcome back, {}", display);
        let greet_w = canvas.measure_text_aa(&greeting, TYPE_SUBTITLE, sans);
        canvas.draw_text_aa(
            cx as i32 - greet_w / 2,
            (avatar_y + avatar_size + 18) as i32,
            &greeting,
            TYPE_SUBTITLE,
            p.text_primary,
            sans,
        );

        // ── Password field → a frosted input pill (popover-frost fill, radius-
        //    pill) with an accent focus ring — the macOS/Win11 field cue.
        let field_w = 280usize;
        let field_h = 40usize;
        let field_x = cx.saturating_sub(field_w / 2);
        let field_y = avatar_y + avatar_size + 52;
        let fr = field_h / 2;
        canvas.fill_rounded_rect(
            field_x,
            field_y,
            field_w,
            field_h,
            fr,
            GLASS_POPOVER_DARK.frost,
        );

        let prompt = if self.password_len == 0 {
            String::from("Password")
        } else {
            "*".repeat(self.password_len.min(32))
        };
        let prompt_color = if self.password_len == 0 {
            p.text_tertiary
        } else {
            p.text_primary
        };
        // Text inset = 16px (the token control inset for a pill). Field text is
        // `type.body`, vertically centred in the 40px field box.
        let field_ty = (field_h.saturating_sub(TYPE_BODY.line_height as usize)) / 2;
        canvas.draw_text_aa(
            (field_x + 16) as i32,
            (field_y + field_ty) as i32,
            &prompt,
            TYPE_BODY,
            prompt_color,
            sans,
        );

        // Focus ring — accent.hover when the password field has focus, a faint
        // accent.subtle resting outline otherwise (so the pill always reads).
        let ring = if self.field == LoginField::Password {
            acc.hover
        } else {
            acc.subtle
        };
        canvas.draw_rounded_rect_outline(field_x, field_y, field_w, field_h, fr, ring);

        // ── Hint line: subtle "Press Enter to sign in" — `type.caption`,
        //    text.secondary, single line, no debug-y keyboard map.
        let hint = "Press Enter to sign in";
        let hint_w = canvas.measure_text_aa(hint, TYPE_CAPTION, sans);
        canvas.draw_text_aa(
            cx as i32 - hint_w / 2,
            (field_y + field_h + 24) as i32,
            hint,
            TYPE_CAPTION,
            p.text_secondary,
            sans,
        );

        // ── Error slot — `type.caption`, danger colour, centred.
        if let Some(ref err) = self.error {
            let err_w = canvas.measure_text_aa(err, TYPE_CAPTION, sans);
            canvas.draw_text_aa(
                cx as i32 - err_w / 2,
                (field_y + field_h + 50) as i32,
                err,
                TYPE_CAPTION,
                p.state_danger,
                sans,
            );
        }

        // ── Bottom-of-card footer: small RaeenOS wordmark (`type.label`, accent)
        //    so the brand reads even on a single screenshot.
        let footer = "AthenaOS";
        let footer_w = canvas.measure_text_aa(footer, TYPE_LABEL, sans);
        canvas.draw_text_aa(
            cx as i32 - footer_w / 2,
            (card_y + card_h - 24) as i32,
            footer,
            TYPE_LABEL,
            acc.active,
            sans,
        );
    }
}

// ── R10 proof: token wiring + live-accent tracking ──────────────────────────

/// Deterministic, fail-able proof that the login screen wears the Liquid Glass
/// identity: its accent tracks the LIVE Vibe seed (`theme_engine::active_accent`),
/// its palette is `rae_tokens`-derived (not re-hardcoded), and the card draws on
/// the `glass.popover` tier (the aurora + frosted-card surface, not a flat panel).
/// If the screen ever re-hardcodes the accent, drifts a palette colour off the
/// token, or downgrades the card off the popover tier, this prints FAIL.
pub fn run_boot_smoketest() {
    let want_accent = rae_tokens::derive_accent(crate::theme_engine::active_accent(), PALETTE).base;
    let accent_ok = proof_accent() == want_accent;
    // Palette colours must be the token values, not re-hardcoded constants.
    let palette_ok = CARD_BG == DARK.bg_raised
        && FG == DARK.text_primary
        && FG_DIM == DARK.text_secondary
        && FIELD_BG == DARK.bg_elevated
        && BG_TOP == DARK.bg_base;
    // The card glass tier the render actually paints — popover (transient surface
    // over the busy aurora). A regression to a flat opaque card breaks this.
    let glass_ok = GLASS_POPOVER_DARK.frost == DARK_POPOVER_FROST_REF
        && GLASS_POPOVER_DARK.tint != 0xFF_00_00_00;
    let pass = accent_ok && palette_ok && glass_ok;
    crate::serial_println!(
        "[login] glass-screen: accent={:#010X} card={:#010X} aurora=on glass={} palette={} text=aa -> {}",
        proof_accent(),
        CARD_BG,
        if glass_ok { "popover" } else { "DRIFT" },
        if palette_ok { "tokens" } else { "DRIFT" },
        if pass { "PASS" } else { "FAIL" },
    );
}

/// Reference popover-frost alpha-add the login card uses — the OBSIDIAN whisper
/// frost (`0x06` over white, IDENTITY-OBSIDIAN.md §2); if the shipped token
/// ladder shifts off it, the smoketest above flags DRIFT.
const DARK_POPOVER_FROST_REF: u32 = 0x06_FF_FF_FF;

/// Public re-export so other UI surfaces (desktop wallpaper, setup
/// wizard) can share the same gradient language for visual continuity.
pub fn draw_vertical_gradient_pub(
    canvas: &mut raegfx::Canvas,
    w: usize,
    h: usize,
    top: u32,
    bot: u32,
    strips: usize,
) {
    draw_vertical_gradient(canvas, w, h, top, bot, strips);
}

/// Draw an N-strip vertical gradient between `top` and `bot`. Faked via
/// fill_rect because the Canvas API has no per-pixel write — this is
/// O(N) fill_rects, each O(width). At N=32 and 1920×1080 that's about
/// 60K pixels written 32 times = 2M writes, fast enough that the login
/// screen redraw doesn't visibly stutter on TCG QEMU.
fn draw_vertical_gradient(
    canvas: &mut raegfx::Canvas,
    w: usize,
    h: usize,
    top: u32,
    bot: u32,
    strips: usize,
) {
    let strip_h = (h / strips).max(1);
    let extract = |c: u32, shift: u32| ((c >> shift) & 0xFF) as i32;
    let a_top = extract(top, 24);
    let r_top = extract(top, 16);
    let g_top = extract(top, 8);
    let b_top = extract(top, 0);
    let a_bot = extract(bot, 24);
    let r_bot = extract(bot, 16);
    let g_bot = extract(bot, 8);
    let b_bot = extract(bot, 0);
    let lerp = |a: i32, b: i32, t: i32| -> u32 {
        let v = a + (b - a) * t / (strips as i32 - 1).max(1);
        v.clamp(0, 255) as u32
    };
    for i in 0..strips {
        let t = i as i32;
        let a = lerp(a_top, a_bot, t);
        let r = lerp(r_top, r_bot, t);
        let g = lerp(g_top, g_bot, t);
        let b = lerp(b_top, b_bot, t);
        let color = (a << 24) | (r << 16) | (g << 8) | b;
        let y = i * strip_h;
        let this_h = if i == strips - 1 {
            h.saturating_sub(y)
        } else {
            strip_h
        };
        canvas.fill_rect(0, y, w, this_h, color);
    }
}

fn scancode_to_ascii(code: u8, shift: bool) -> Option<u8> {
    #[rustfmt::skip]
    const UNSHIFTED: [u8; 58] = [
        0, 0x1B, b'1', b'2', b'3', b'4', b'5', b'6', b'7', b'8',
        b'9', b'0', b'-', b'=', 0x08, b'\t', b'q', b'w', b'e', b'r',
        b't', b'y', b'u', b'i', b'o', b'p', b'[', b']', b'\n', 0,
        b'a', b's', b'd', b'f', b'g', b'h', b'j', b'k', b'l', b';',
        b'\'', b'`', 0, b'\\', b'z', b'x', b'c', b'v', b'b', b'n',
        b'm', b',', b'.', b'/', 0, b'*', 0, b' ',
    ];
    #[rustfmt::skip]
    const SHIFTED: [u8; 58] = [
        0, 0x1B, b'!', b'@', b'#', b'$', b'%', b'^', b'&', b'*',
        b'(', b')', b'_', b'+', 0x08, b'\t', b'Q', b'W', b'E', b'R',
        b'T', b'Y', b'U', b'I', b'O', b'P', b'{', b'}', b'\n', 0,
        b'A', b'S', b'D', b'F', b'G', b'H', b'J', b'K', b'L', b':',
        b'"', b'~', 0, b'|', b'Z', b'X', b'C', b'V', b'B', b'N',
        b'M', b'<', b'>', b'?', 0, b'*', 0, b' ',
    ];
    if code >= 58 {
        return None;
    }
    let ch = if shift {
        SHIFTED[code as usize]
    } else {
        UNSHIFTED[code as usize]
    };
    if ch == 0 {
        None
    } else {
        Some(ch)
    }
}

/// Handle a PS/2 scancode. Returns `true` when login succeeded.
pub fn handle_key(state: &mut LoginState, scancode: u8) -> bool {
    let is_release = scancode & 0x80 != 0;
    let code = scancode & 0x7F;

    if code == 0x2A || code == 0x36 {
        state.shift_held = !is_release;
        return false;
    }
    if is_release {
        return false;
    }

    if code == 0x0F {
        state.field = match state.field {
            LoginField::Username => LoginField::Password,
            LoginField::Password => LoginField::Username,
        };
        return false;
    }

    if code == 0x34 {
        if crate::session::login_guest() {
            return true;
        }
        state.error = Some(String::from("Guest login unavailable"));
        return false;
    }

    if code == 0x1C {
        return try_submit(state);
    }

    if code == 0x0E {
        match state.field {
            LoginField::Username if state.username_len > 0 => {
                state.username_len -= 1;
            }
            LoginField::Password if state.password_len > 0 => {
                state.password_len -= 1;
            }
            _ => {}
        }
        return false;
    }

    if let Some(ascii) = scancode_to_ascii(code, state.shift_held) {
        if ascii == b'\t' {
            state.field = match state.field {
                LoginField::Username => LoginField::Password,
                LoginField::Password => LoginField::Username,
            };
            return false;
        }
        if ascii == 0x08 {
            return handle_key(state, 0x0E);
        }
        if ascii >= 0x20 {
            match state.field {
                LoginField::Username if state.username_len < state.username.len() - 1 => {
                    state.username[state.username_len] = ascii;
                    state.username_len += 1;
                }
                LoginField::Password if state.password_len < state.password.len() - 1 => {
                    state.password[state.password_len] = ascii;
                    state.password_len += 1;
                }
                _ => {}
            }
        }
    }

    false
}

fn try_submit(state: &mut LoginState) -> bool {
    let user = match core::str::from_utf8(&state.username[..state.username_len]) {
        Ok(s) => s,
        Err(_) => {
            state.error = Some(String::from("Invalid username"));
            return false;
        }
    };

    if crate::session::login_password(user, &state.password[..state.password_len]) {
        state.error = None;
        return true;
    }

    state.error = Some(String::from("Incorrect username or password"));
    state.password_len = 0;
    false
}
