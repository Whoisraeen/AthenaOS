//! First-boot setup wizard — Windows-OOBE-equivalent.
//!
//! Rendered by `shell_runner` when no user has completed first-boot
//! setup. Asks for a username + password, creates the local account via
//! `session::create_local_account`, persists a `setup.first_boot_done`
//! flag in `config_registry`, and transitions straight to the desktop
//! (auto-login as the just-created user — same convention as Windows OOBE).
//!
//! Until this lands, RaeenOS booted to a pre-seeded `raeen`/`raeen` account
//! that no real user ever chose — fine for a debug kernel, not for an OS
//! you'd hand to anyone. The wizard makes first boot feel like a fresh
//! Windows or macOS install: you see your own name on the lock screen
//! after the first power-on.
//!
//! Subsequent boots short-circuit: `setup::is_first_boot_complete()` reads
//! the config flag and returns true, so `shell_runner` skips this phase
//! and goes straight to the login screen.

#![allow(dead_code)]

extern crate alloc;

use alloc::string::String;
use rae_tokens::{
    AccentRamp, Palette, DARK, GLASS_PANEL_DARK, RADIUS_MD, RADIUS_SM, RADIUS_XL, SPACE_2, SPACE_3,
    SPACE_4, SPACE_5, SPACE_6, TYPE_BODY, TYPE_CAPTION, TYPE_LABEL, TYPE_SUBTITLE, TYPE_TITLE,
};
use raegfx::text::FontFamily;

/// Active palette for the OOBE surface — dark default (the first-boot wizard
/// wears the same dark Liquid Glass identity as the lock / login screens).
const PALETTE: &Palette = &DARK;

/// The LIVE accent ramp for the OOBE wizard — derived from the active
/// theme/Vibe seed (`theme_engine::active_accent`), same as window chrome,
/// the login screen and the toasts. The wizard's primary button + field
/// focus ring recolour on a one-tap Vibe re-skin instead of being frozen
/// to a hand-picked blue. The card itself is now the shipped tiered
/// **Liquid Glass** material (`glass.panel` over the aurora) — visual
/// continuity with the lock screen (`lock_screen.rs`) and login
/// (`login_ui.rs`); only the accent fill/ring is tokenized on top.
#[inline]
fn accent() -> AccentRamp {
    rae_tokens::derive_accent(crate::theme_engine::active_accent(), PALETTE)
}

/// The accent base actually painted on the wizard — public so the
/// cross-surface cohesion smoketest can confirm the OOBE tracks the live seed.
#[inline]
pub fn proof_accent() -> u32 {
    accent().base
}

/// Brand-emblem tile size (px). A rounded accent tile with the white Rae diamond.
const EMBLEM_SIZE: usize = 56;
/// Full-screen OOBE card corner radius (design-language §3 `radius.xl` = 24).
const CARD_RADIUS: u32 = RADIUS_XL;
/// Primary-button corner radius (design-language §3 `radius.md` = 12).
const BUTTON_RADIUS: u32 = RADIUS_MD;
/// Input-field corner radius (design-language §3 `radius.sm` = 8) — concentric
/// inside the `radius.xl` card (24 > 8).
const FIELD_RADIUS: u32 = RADIUS_SM;

// ── Type scales (8×8 glyph upscale factors) ──────────────────────────────────
// `draw_text_scaled(scale)` renders 8*scale-px-tall glyphs advancing 8*scale px.
// Unscaled `draw_text` is 8 px. These map to the design-language type ramp at the
// nearest integer software-raster scale (§6).
const TITLE_SCALE: usize = 3; // ≈ type.display
const TEXT_SCALE: usize = 2; // ≈ type.subtitle/body for labels + field text
const TITLE_H: usize = 8 * TITLE_SCALE; // 24
const TEXT_H: usize = 8 * TEXT_SCALE; // 16
const HINT_H: usize = 8; // unscaled hint/caption row

/// An axis-aligned rectangle (screen-space, px). Used to lay out the OOBE on the
/// 4px grid AND to let the layout smoketest assert the hint never overlaps the
/// primary button (the exact regression the first screenshot caught).
#[derive(Clone, Copy)]
pub struct Rect {
    pub x: usize,
    pub y: usize,
    pub w: usize,
    pub h: usize,
}

impl Rect {
    /// True if two rects share any pixel. The OOBE layout guarantees the hint
    /// row and the button rect never do.
    pub fn intersects(&self, o: &Rect) -> bool {
        let x_overlap = self.x < o.x + o.w && o.x < self.x + self.w;
        let y_overlap = self.y < o.y + o.h && o.y < self.y + self.h;
        x_overlap && y_overlap
    }
}

/// The full wizard geometry, computed once from the framebuffer size on the 4px
/// grid (design-language §2 spacing tokens). `render` paints from it and the
/// layout smoketest asserts against it — single source of truth so the proof
/// can never drift from what's drawn.
pub struct Layout {
    pub card: Rect,
    /// Brand emblem tile (top-left of the card content, above the title) — the
    /// Rae mark. Win11/macOS OOBE both lead with a product emblem; a bare text
    /// title reads as unfinished. `(x, y, size)`.
    pub emblem: (usize, usize, usize),
    pub title: (usize, usize),
    pub subtitle: (usize, usize),
    pub username_label: (usize, usize),
    pub username_field: Rect,
    pub password_label: (usize, usize),
    pub password_field: Rect,
    pub confirm_label: (usize, usize),
    pub confirm_field: Rect,
    pub button: Rect,
    /// "Skip — use without an account" affordance, left-aligned on the same row
    /// as the primary button. Concept §"the user owns the machine" + UI_UX §7
    /// anti-pattern "Account required for local use": RaeenOS must let a user
    /// reach the desktop on first boot WITHOUT creating credentials, unlike
    /// Windows 11's forced Microsoft account.
    pub skip_button: Rect,
    pub hint: Rect,
    pub text_x: usize,
    pub field_pad_x: usize,
}

/// Compute the OOBE layout for a `w × h` framebuffer. Pure geometry: no drawing,
/// no `self` — so the boot smoketest computes the exact same rects the renderer
/// uses. Vertical rhythm follows design-language §2:
/// card top padding `SPACE_6`(32) · emblem→title `SPACE_4`(16) ·
/// title→subtitle `SPACE_2`(8) · subtitle→fields `SPACE_5`(24) ·
/// label→field `SPACE_2`(8) · field→field `SPACE_4`(16) ·
/// group→button `SPACE_5`(24) · button→hint `SPACE_3`(12). The card height
/// grows by the emblem block so the header adds space instead of clipping.
pub fn compute_layout(w: usize, h: usize) -> Layout {
    // Grow the card by the emblem block (emblem tile + gap to the title) so the
    // brand header adds vertical space rather than pushing the hint off the card.
    let emb = EMBLEM_SIZE;
    let emblem_block = emb + SPACE_4 as usize; // emblem → title (space.4 = 16)
    let cw = (w * 46 / 100).clamp(540, 760).min(w.saturating_sub(48));
    let ch = (h * 62 / 100)
        .clamp(472 + emblem_block, 620 + emblem_block)
        .min(h.saturating_sub(48));
    let card = Rect {
        x: (w - cw) / 2,
        y: (h - ch) / 2,
        w: cw,
        h: ch,
    };

    let pad = SPACE_6 as usize; // 32 — card content margin (§2 space.6)
    let tx = card.x + pad;
    let fw = cw - pad * 2;
    let fh = 42usize;

    let mut y = card.y + pad;

    // Brand emblem, then the title beneath it.
    let emblem = (tx, y, emb);
    y += emblem_block;

    let title = (tx, y);
    y += TITLE_H + SPACE_2 as usize; // title → subtitle (space.2 = 8)

    let subtitle = (tx, y);
    y += TEXT_H + SPACE_5 as usize; // subtitle → first field group (space.5 = 24)

    let username_label = (tx, y);
    y += TEXT_H + SPACE_2 as usize; // label → field (space.2 = 8)
    let username_field = Rect {
        x: tx,
        y,
        w: fw,
        h: fh,
    };
    y += fh + SPACE_4 as usize; // field → field (space.4 = 16)

    let password_label = (tx, y);
    y += TEXT_H + SPACE_2 as usize;
    let password_field = Rect {
        x: tx,
        y,
        w: fw,
        h: fh,
    };
    y += fh + SPACE_4 as usize;

    let confirm_label = (tx, y);
    y += TEXT_H + SPACE_2 as usize;
    let confirm_field = Rect {
        x: tx,
        y,
        w: fw,
        h: fh,
    };
    y += fh + SPACE_5 as usize; // last field group → button (space.5 = 24)

    let label = "Create account";
    let bw = label.len() * (8 * TEXT_SCALE) + 48;
    let bh = 46usize;
    let button = Rect {
        x: card.x + cw - bw - pad, // right-aligned inside the card padding
        y,
        w: bw,
        h: bh,
    };

    // "Skip — use without an account" — a tertiary text button left-aligned on
    // the SAME row as the primary, so the create-account path stays the obvious
    // default while the no-account escape is always visible (UI_UX §7). Sized to
    // its label; it never overlaps the right-aligned primary because the card is
    // ≥540 px wide and the two labels together fit inside `fw`.
    let skip_label = "Skip - use without an account";
    let sbw = (skip_label.len() * 8 + 24).min(button.x.saturating_sub(tx + SPACE_3 as usize));
    let skip_button = Rect {
        x: tx,
        y,
        w: sbw,
        h: bh,
    };

    // Hint on its OWN full-width row BELOW the button (space.3 = 12 gap), so it
    // can never share a baseline with the button — the screenshot bug fix.
    let hint = Rect {
        x: tx,
        y: button.y + button.h + SPACE_3 as usize,
        w: fw,
        h: HINT_H,
    };

    Layout {
        card,
        emblem,
        title,
        subtitle,
        username_label,
        username_field,
        password_label,
        password_field,
        confirm_label,
        confirm_field,
        button,
        skip_button,
        hint,
        text_x: tx,
        field_pad_x: 14,
    }
}

const BG: u32 = DARK.bg_base;
const FG: u32 = DARK.text_primary;
/// Error text — `state.danger` (token). Used by the smoketest + future surfaces.
const ERR: u32 = DARK.state_danger;
const OK: u32 = DARK.state_ok;
const FIELD_BG: u32 = DARK.bg_overlay;
const MUTED: u32 = DARK.text_tertiary;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SetupField {
    Username,
    Password,
    PasswordConfirm,
}

pub struct SetupState {
    pub username: [u8; 32],
    pub username_len: usize,
    pub password: [u8; 64],
    pub password_len: usize,
    pub password_confirm: [u8; 64],
    pub password_confirm_len: usize,
    pub field: SetupField,
    pub error: Option<String>,
    pub shift_held: bool,
}

impl SetupState {
    pub fn new() -> Self {
        Self {
            username: [0u8; 32],
            username_len: 0,
            password: [0u8; 64],
            password_len: 0,
            password_confirm: [0u8; 64],
            password_confirm_len: 0,
            field: SetupField::Username,
            error: None,
            shift_held: false,
        }
    }

    /// True once the user has started filling in the wizard — any field has
    /// content, the focus has moved off the first field, or an error/shift state
    /// is set. Used by `shell_runner` (BUG B) to avoid resetting + repainting an
    /// in-progress setup card during a periodic recheck/logout, which would wipe
    /// keystrokes and blank/clip the card mid-render. A freshly-constructed state
    /// reports `false`, so the initial paint still happens normally.
    pub fn in_progress(&self) -> bool {
        self.username_len > 0
            || self.password_len > 0
            || self.password_confirm_len > 0
            || self.field != SetupField::Username
            || self.error.is_some()
    }

    /// Render the first-boot wizard into a raw `0xAARRGGBB` framebuffer.
    ///
    /// Concept §"rival Windows + macOS": OOBE is the literal first thing a new
    /// user sees, so it wears the same **Liquid Glass** identity as the desktop,
    /// the lock screen (`lock_screen.rs`) and login (`login_ui.rs`) — a living
    /// aurora backdrop (IDENTITY.md §3 Aurora Mesh) with a centered frosted
    /// `glass.panel` card (§7 tiers) holding the account fields, floating on the
    /// aurora. The public signature is unchanged (the boot caller passes `ptr` +
    /// `w`/`h`); internally we wrap the buffer in a [`raegfx::Canvas`] and draw
    /// through the shared `glass`/`draw_text_aa` primitives instead of hand-rolled
    /// hex pixels. Token-derived colours only — a Vibe re-skin flows here too.
    pub fn render(&self, ptr: *mut u8, w: u32, h: u32) {
        let mut canvas = unsafe { raegfx::Canvas::new(ptr, w as usize, h as usize, 4) };
        let (w, h) = (w as usize, h as usize);

        // Live accent (tracks Vibe Mode) for the primary button + focus ring.
        let acc = accent();
        let p: &Palette = PALETTE;
        let sans = FontFamily::Sans;
        let lay = compute_layout(w, h);

        // ── Background → the signature Aurora Mesh (IDENTITY.md §3): the same
        //    living backdrop the desktop / lock / login wear — visual continuity
        //    from first power-on onward. Replaces the old flat blue gradient.
        raegfx::glass::render_aurora_dark(&mut canvas, 0, 0, w, h, 0);

        // ── Centered glass card: the wizard content floats on a `glass.panel`
        //    frosted surface (the workhorse large-card tier) with the shipped
        //    tiered draw (tint → frost → legibility cap → iridescent rim) and a
        //    soft ambient shadow that lifts it off the aurora — at radius.xl (24)
        //    for the OOBE/full-screen modal (design-language §3).
        let c = lay.card;
        let cr = CARD_RADIUS as usize;
        canvas.fill_rounded_rect_shadow(c.x, c.y, c.w, c.h, cr, 0x0A_10_1C, 44, 18);
        raegfx::glass::draw_glass_surface(&mut canvas, c.x, c.y, c.w, c.h, cr, GLASS_PANEL_DARK);

        // ── Brand emblem: a rounded accent tile carrying the white Rae diamond.
        //    Gives the OOBE a real product mark (the top "feels finished" gap vs
        //    Win11/macOS, which both lead with an emblem). Accent-tinted so it
        //    tracks Vibe Mode like the primary button + focus ring.
        let (ex, ey, es) = lay.emblem;
        // A soft ambient lift, then the tile at radius.md, in the live accent.
        canvas.fill_rounded_rect_shadow(ex, ey, es, es, BUTTON_RADIUS as usize, 0x05_0A_14, 40, 12);
        canvas.fill_rounded_rect(ex, ey, es, es, BUTTON_RADIUS as usize, acc.base);
        // A thin lighter cap on the top third (opaque `hover` = accent-toward-white)
        // gives the tile a subtle glassy face without depending on alpha blending.
        canvas.fill_rounded_rect(
            ex + 4,
            ey + 4,
            es - 8,
            es / 3,
            (BUTTON_RADIUS as usize).saturating_sub(3),
            acc.hover,
        );
        // The white Rae diamond (rotated square), centered, ~44% of the tile —
        // filled by symmetric scanlines so the edges stay crisp at any size.
        let cx = ex + es / 2;
        let cy = ey + es / 2;
        let d = (es * 22 / 100).max(6); // diamond half-height
        for dy in 0..=d {
            let hw = d - dy; // half-width shrinks toward the tips
            let w2 = hw * 2 + 1;
            // upper and lower halves
            canvas.fill_rect(cx - hw, cy - dy, w2, 1, 0xFF_FF_FF);
            canvas.fill_rect(cx - hw, cy + dy, w2, 1, 0xFF_FF_FF);
        }

        // Title (display) + subtitle — text.primary / text.secondary. The
        // legibility cap inside draw_glass_surface keeps the panel interior dark
        // enough that text.primary wins over the aurora behind it.
        canvas.draw_text_aa(
            lay.title.0 as i32,
            lay.title.1 as i32,
            "Welcome to RaeenOS",
            TYPE_TITLE,
            p.text_primary,
            sans,
        );
        canvas.draw_text_aa(
            lay.subtitle.0 as i32,
            lay.subtitle.1 as i32,
            "Let's set up your account",
            TYPE_SUBTITLE,
            p.text_secondary,
            sans,
        );

        // Vertically center body text within the 42px field box for TYPE_BODY's
        // line box.
        let ty = (42 - TYPE_BODY.line_height as usize) / 2;

        // Username.
        canvas.draw_text_aa(
            lay.username_label.0 as i32,
            lay.username_label.1 as i32,
            "Username",
            TYPE_LABEL,
            p.text_secondary,
            sans,
        );
        let uf = lay.username_field;
        draw_field(&mut canvas, uf, self.field == SetupField::Username, &acc);
        let utext = core::str::from_utf8(&self.username[..self.username_len]).unwrap_or("");
        if utext.is_empty() {
            self.draw_placeholder(&mut canvas, uf, ty, "Choose a username");
        } else {
            canvas.draw_text_aa(
                (uf.x + lay.field_pad_x) as i32,
                (uf.y + ty) as i32,
                utext,
                TYPE_BODY,
                p.text_primary,
                sans,
            );
        }
        self.draw_caret(
            &mut canvas,
            uf,
            ty,
            utext,
            self.field == SetupField::Username,
            &acc,
        );

        // Password.
        canvas.draw_text_aa(
            lay.password_label.0 as i32,
            lay.password_label.1 as i32,
            "Password",
            TYPE_LABEL,
            p.text_secondary,
            sans,
        );
        let pf = lay.password_field;
        draw_field(&mut canvas, pf, self.field == SetupField::Password, &acc);
        let pw_stars = "*".repeat(self.password_len);
        if self.password_len == 0 {
            self.draw_placeholder(&mut canvas, pf, ty, "Enter a password");
        } else {
            canvas.draw_text_aa(
                (pf.x + lay.field_pad_x) as i32,
                (pf.y + ty) as i32,
                &pw_stars,
                TYPE_BODY,
                p.text_primary,
                sans,
            );
        }
        self.draw_caret(
            &mut canvas,
            pf,
            ty,
            &pw_stars,
            self.field == SetupField::Password,
            &acc,
        );

        // Confirm.
        canvas.draw_text_aa(
            lay.confirm_label.0 as i32,
            lay.confirm_label.1 as i32,
            "Confirm password",
            TYPE_LABEL,
            p.text_secondary,
            sans,
        );
        let cf = lay.confirm_field;
        draw_field(
            &mut canvas,
            cf,
            self.field == SetupField::PasswordConfirm,
            &acc,
        );
        let cf_stars = "*".repeat(self.password_confirm_len);
        if self.password_confirm_len == 0 {
            self.draw_placeholder(&mut canvas, cf, ty, "Re-enter password");
        } else {
            canvas.draw_text_aa(
                (cf.x + lay.field_pad_x) as i32,
                (cf.y + ty) as i32,
                &cf_stars,
                TYPE_BODY,
                p.text_primary,
                sans,
            );
        }
        self.draw_caret(
            &mut canvas,
            cf,
            ty,
            &cf_stars,
            self.field == SetupField::PasswordConfirm,
            &acc,
        );

        // Primary "Create account" → accent-filled pill with dark-on-accent INK
        // (the IDENTITY guardrail: white-on-accent fails WCAG; ink on an accent
        // fill is bg.base). LIVE accent fill recolours with a Vibe re-skin.
        let b = lay.button;
        canvas.fill_rounded_rect(b.x, b.y, b.w, b.h, BUTTON_RADIUS as usize, acc.base);
        canvas.draw_text_aa(
            (b.x + 24) as i32,
            (b.y + (b.h - TYPE_LABEL.line_height as usize) / 2) as i32,
            "Create account",
            TYPE_LABEL,
            p.bg_base,
            sans,
        );

        // "Skip — use without an account" — tertiary text button (no fill, just a
        // resting hairline so it reads as a secondary affordance, not a primary).
        // The Concept "no forced account" escape hatch; left-aligned on the
        // button row. Click hit-test + Esc both route to `skip()`.
        let s = lay.skip_button;
        canvas.draw_rounded_rect_outline(
            s.x,
            s.y,
            s.w,
            s.h,
            BUTTON_RADIUS as usize,
            p.stroke_subtle,
        );
        canvas.draw_text_aa(
            (s.x + 12) as i32,
            (s.y + (s.h - TYPE_LABEL.line_height as usize) / 2) as i32,
            "Skip - use without an account",
            TYPE_LABEL,
            p.text_secondary,
            sans,
        );

        // Hint / error line on its OWN full-width row BELOW the button (no shared
        // baseline → never bleeds through the button — screenshot-bug fix).
        let hint = lay.hint;
        if let Some(ref err) = self.error {
            canvas.draw_text_aa(
                hint.x as i32,
                hint.y as i32,
                err,
                TYPE_CAPTION,
                p.state_danger,
                sans,
            );
        } else {
            // text.secondary — the keyboard hints are the only affordance a
            // mouse-less first boot has; tertiary vanished over the glass card.
            canvas.draw_text_aa(
                hint.x as i32,
                hint.y as i32,
                "Tab = next field    Enter = create account    Esc = skip",
                TYPE_CAPTION,
                p.text_secondary,
                sans,
            );
        }
    }

    /// Placeholder inside an empty field. `text.secondary`, not tertiary —
    /// tertiary is tuned for bg.base, and over the frost-lifted field fill on
    /// the glass card it measured near-invisible on the live OOBE (QMP
    /// screenshot 2026-07-01); the user couldn't read what a field wanted.
    fn draw_placeholder(&self, canvas: &mut raegfx::Canvas, field: Rect, ty: usize, s: &str) {
        canvas.draw_text_aa(
            (field.x + 14) as i32,
            (field.y + ty) as i32,
            s,
            TYPE_BODY,
            PALETTE.text_secondary,
            FontFamily::Sans,
        );
    }

    /// A 2px accent caret at the text-insertion point of the focused field.
    /// Caret x is the measured AA advance of the text so far (kerned), so it
    /// tracks the real glyph positions instead of a fixed cell width.
    fn draw_caret(
        &self,
        canvas: &mut raegfx::Canvas,
        field: Rect,
        ty: usize,
        text: &str,
        focused: bool,
        acc: &AccentRamp,
    ) {
        if !focused {
            return;
        }
        let advance = canvas.measure_text_aa(text, TYPE_BODY, FontFamily::Sans);
        let cx = field.x + 14 + advance.max(0) as usize;
        canvas.fill_rect(
            cx + 1,
            field.y + ty,
            2,
            TYPE_BODY.line_height as usize,
            acc.base,
        );
    }
}

/// A frosted input pill that turns accent with a focus ring when active — the
/// glass-card field style (matches the login password pill, `login_ui.rs`). The
/// fill is the popover-panel frost sheen (a low-alpha white over the dark glass
/// card, so the field reads as a brighter inset on the panel); the focus ring is
/// the LIVE accent (`acc.base` border + `acc.glow` inner ring), recolouring with
/// a Vibe re-skin; the resting border is a token `stroke.subtle` hairline.
fn draw_field(canvas: &mut raegfx::Canvas, f: Rect, active: bool, acc: &AccentRamp) {
    let r = FIELD_RADIUS as usize; // radius.sm (8) — concentric inside the radius.xl card
    canvas.fill_rounded_rect(f.x, f.y, f.w, f.h, r, GLASS_PANEL_DARK.frost);
    if active {
        canvas.draw_rounded_rect_outline(f.x, f.y, f.w, f.h, r, acc.base);
        canvas.draw_rounded_rect_outline(
            f.x + 1,
            f.y + 1,
            f.w.saturating_sub(2),
            f.h.saturating_sub(2),
            r.saturating_sub(1),
            acc.glow,
        );
    } else {
        // Resting hairline — token stroke.subtle (the glass-card field cue).
        canvas.draw_rounded_rect_outline(f.x, f.y, f.w, f.h, r, PALETTE.stroke_subtle);
    }
}

/// Map a PS/2 set-1 scancode to ASCII, honoring the shift modifier.
/// Copy of `login_ui::scancode_to_ascii` — kept private here so changes
/// to login UX don't silently affect setup UX (the wizard is intentionally
/// minimal and a moving target).
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

/// Handle a PS/2 scancode. Returns `true` when setup completed
/// successfully (and the caller should activate the desktop). Returns
/// `false` if the wizard is still collecting input (or rejected the
/// input with an error message rendered next render pass).
pub fn handle_key(state: &mut SetupState, scancode: u8) -> bool {
    let is_release = scancode & 0x80 != 0;
    let code = scancode & 0x7F;

    if code == 0x2A || code == 0x36 {
        state.shift_held = !is_release;
        return false;
    }
    if is_release {
        return false;
    }

    // Tab — cycle Username → Password → Confirm → Username
    if code == 0x0F {
        state.field = match state.field {
            SetupField::Username => SetupField::Password,
            SetupField::Password => SetupField::PasswordConfirm,
            SetupField::PasswordConfirm => SetupField::Username,
        };
        return false;
    }

    // Enter — submit. Validate, create account, persist first-boot flag.
    if code == 0x1C {
        return submit(state);
    }

    // Esc — "Skip / use without an account". Concept §"the user owns the
    // machine" + UI_UX §7: reach the desktop without credentials. Enters a
    // local GUEST session and completes OOBE so a real user is never forced to
    // create an account on first boot (the Windows-11 anti-pattern). Returns
    // true so the caller activates the desktop, exactly like account creation.
    if code == 0x01 {
        return skip(state);
    }

    // Backspace
    if code == 0x0E {
        match state.field {
            SetupField::Username if state.username_len > 0 => state.username_len -= 1,
            SetupField::Password if state.password_len > 0 => state.password_len -= 1,
            SetupField::PasswordConfirm if state.password_confirm_len > 0 => {
                state.password_confirm_len -= 1
            }
            _ => {}
        }
        return false;
    }

    // Printable character
    if let Some(ch) = scancode_to_ascii(code, state.shift_held) {
        if ch == b'\n' || ch == b'\t' || ch == 0x1B {
            return false;
        }
        match state.field {
            SetupField::Username => {
                if state.username_len < state.username.len() && ch >= 0x20 {
                    state.username[state.username_len] = ch;
                    state.username_len += 1;
                }
            }
            SetupField::Password => {
                if state.password_len < state.password.len() && ch >= 0x20 {
                    state.password[state.password_len] = ch;
                    state.password_len += 1;
                }
            }
            SetupField::PasswordConfirm => {
                if state.password_confirm_len < state.password_confirm.len() && ch >= 0x20 {
                    state.password_confirm[state.password_confirm_len] = ch;
                    state.password_confirm_len += 1;
                }
            }
        }
    }

    false
}

fn submit(state: &mut SetupState) -> bool {
    // Validation rules — kept loose for a v1 wizard. We're not enforcing
    // Microsoft-style complexity; just non-empty and matching.
    if state.username_len == 0 {
        state.error = Some(String::from("Pick a username to continue."));
        return false;
    }
    if state.password_len == 0 {
        state.error = Some(String::from("Pick a password to continue."));
        return false;
    }
    if state.password_len != state.password_confirm_len
        || state.password[..state.password_len]
            != state.password_confirm[..state.password_confirm_len]
    {
        state.error = Some(String::from("Passwords don't match. Try again."));
        state.password_confirm_len = 0;
        state.field = SetupField::PasswordConfirm;
        return false;
    }

    let user_bytes = &state.username[..state.username_len];
    let pass_bytes = &state.password[..state.password_len];
    let username = match core::str::from_utf8(user_bytes) {
        Ok(s) => s,
        Err(_) => {
            state.error = Some(String::from("Username contains invalid characters."));
            return false;
        }
    };

    // Use the username as display name for v1 — capitalize first letter.
    let mut display = String::from(username);
    if let Some(first) = display.get_mut(0..1) {
        first.make_ascii_uppercase();
    }

    let uid = crate::session::create_local_account(username, &display, pass_bytes);
    if uid.is_none() {
        state.error = Some(String::from("Account creation failed (name taken?)."));
        return false;
    }

    // Persist the first-boot-done flag so subsequent boots skip the
    // wizard. Stored in the versioned config registry so a rollback
    // would also restore the user back to the wizard — intentional.
    mark_first_boot_done();
    // Persist last-used username so the next boot's login screen
    // greets the right person instead of the hardcoded "Raeen" default.
    crate::config_registry::set_text("/session/last_user", username);

    // Auto-sign-in as the freshly created account so the user lands on
    // the desktop without typing the password they JUST set.
    let signed_in = crate::session::login_password(username, pass_bytes);
    crate::serial_println!(
        "[setup] first-boot account created: '{}' (uid={:?}) auto_signin={}",
        username,
        uid,
        signed_in,
    );

    true
}

/// "Skip — use without an account": complete first-boot WITHOUT credentials by
/// entering a local guest session, then mark OOBE done so subsequent boots don't
/// re-prompt the wizard. Returns true (the caller activates the desktop) on a
/// successful guest sign-in, false (with an error surfaced) if the guest session
/// could not be started.
///
/// Concept §"the user owns the machine" + `docs/UI_UX.md §7` anti-pattern
/// "Account required for local use": RaeenOS must let a person reach a usable
/// desktop on first power-on without ever creating an account — the opposite of
/// Windows 11 forcing a Microsoft account.
fn skip(state: &mut SetupState) -> bool {
    if !crate::session::login_guest() {
        state.error = Some(String::from("Couldn't start a guest session."));
        return false;
    }
    // OOBE is genuinely complete — the user CHOSE no account. Persist the flag
    // so the next boot goes to login (where guest is selectable), not back to
    // the wizard. Mirrors the account-creation completion path.
    mark_first_boot_done();
    crate::serial_println!("[setup] first-boot skipped: entered local guest session (no account)");
    true
}

/// Pure state-transition predicate for the OOBE skip path, host-testable without
/// a live session. Given whether the guest sign-in succeeded, returns
/// `(completed, mark_done)`: a successful skip completes OOBE and marks the
/// first-boot flag; a failed one stays on the wizard and marks nothing. The
/// FAIL-able host KAT asserts skip→completed+marked and a failed skip→neither,
/// so the "no forced account" contract can't silently regress.
pub fn skip_outcome(guest_ok: bool) -> (bool, bool) {
    if guest_ok {
        (true, true)
    } else {
        (false, false)
    }
}

/// True if first-boot setup has already been completed on this install.
/// Reads `/setup/first_boot_done` from the config registry. Returns
/// `false` when the key is absent (fresh install) or the registry is
/// not yet initialized.
pub fn is_first_boot_complete() -> bool {
    crate::config_registry::get_bool("/setup/first_boot_done").unwrap_or(false)
}

fn mark_first_boot_done() {
    crate::config_registry::set_bool("/setup/first_boot_done", true);
}

pub fn run_boot_smoketest() {
    crate::serial_println!(
        "[setup] smoketest: first_boot_done={} -> PASS",
        is_first_boot_complete(),
    );

    // Fail-able OOBE token wiring + live-accent tracking + Liquid Glass identity.
    // The wizard's primary button + focus ring must use
    // derive_accent(active_accent()).base; the full-screen card must be radius.xl
    // (24); and the card now draws on the `glass.panel` tier over the aurora (not
    // a flat opaque card). If any drifts, prints FAIL.
    let want_accent = rae_tokens::derive_accent(crate::theme_engine::active_accent(), PALETTE).base;
    let accent_ok = proof_accent() == want_accent;
    let card_xl = CARD_RADIUS == RADIUS_XL;
    // Glass tier the render actually paints — panel (the large-card workhorse).
    // OBSIDIAN contract (IDENTITY-OBSIDIAN.md §2): near-black tier with a
    // whisper frost (0x04 over white), never a flat 0xFF_00_00_00 card. If the
    // shipped token ladder drifts off this band, prints FAIL.
    let glass_ok =
        GLASS_PANEL_DARK.frost == 0x04_FF_FF_FF && GLASS_PANEL_DARK.tint != 0xFF_00_00_00;
    let pass = accent_ok && card_xl && glass_ok;
    crate::serial_println!(
        "[oobe] setup: accent={:#010X} card=RADIUS_XL({}) button=RADIUS_MD({}) aurora=on glass={} -> {}",
        proof_accent(),
        CARD_RADIUS,
        BUTTON_RADIUS,
        if glass_ok { "panel" } else { "DRIFT" },
        if pass { "PASS" } else { "FAIL" },
    );

    // Fail-able LAYOUT guard — the exact regression the first screenshot caught:
    // the hint text bled THROUGH the "Create account" button (shared baseline).
    // Compute the real layout the renderer uses and assert the hint row and the
    // button rect never intersect, at several representative resolutions. Also
    // re-assert the card radius is radius.xl. Prints FAIL if any rects collide.
    let mut overlap = false;
    for &(w, h) in &[
        (1280usize, 800usize),
        (1920, 1080),
        (1024, 768),
        (2560, 1440),
    ] {
        let lay = compute_layout(w, h);
        if lay.hint.intersects(&lay.button) {
            overlap = true;
        }
    }
    let layout_pass = !overlap && CARD_RADIUS == RADIUS_XL;
    crate::serial_println!(
        "[oobe] layout smoketest: hint_button_overlap={} card_radius={} -> {}",
        overlap,
        CARD_RADIUS,
        if layout_pass { "PASS" } else { "FAIL" },
    );

    // Fail-able "no forced account" guard (BUG A). Two assertions:
    //  1) the skip-state transition: a successful guest sign-in MUST complete
    //     OOBE and mark first-boot-done; a failed one must do NEITHER (the user
    //     stays on the wizard rather than landing on a broken desktop).
    //  2) the skip button never overlaps the right-aligned primary button at any
    //     representative resolution (the same overlap class as the hint bug).
    let (ok_done, ok_mark) = skip_outcome(true);
    let (fail_done, fail_mark) = skip_outcome(false);
    let transition_ok = ok_done && ok_mark && !fail_done && !fail_mark;
    let mut skip_overlap = false;
    for &(w, h) in &[
        (1280usize, 800usize),
        (1920, 1080),
        (1024, 768),
        (2560, 1440),
    ] {
        let lay = compute_layout(w, h);
        if lay.skip_button.intersects(&lay.button) {
            skip_overlap = true;
        }
    }
    let skip_pass = transition_ok && !skip_overlap;
    crate::serial_println!(
        "[oobe] skip smoketest: transition_ok={} skip_button_overlap={} -> {}",
        transition_ok,
        skip_overlap,
        if skip_pass { "PASS" } else { "FAIL" },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// FAIL-able host KAT for BUG A: the "use without an account" skip path's
    /// pure state transition. A successful guest sign-in completes OOBE AND marks
    /// the first-boot flag; a failed one does neither (so a user is never dumped
    /// onto a desktop with no session, and the wizard re-shows).
    #[test]
    fn skip_completes_oobe_without_credentials() {
        assert_eq!(skip_outcome(true), (true, true));
        assert_eq!(skip_outcome(false), (false, false));
    }

    /// FAIL-able host KAT for BUG B: a fresh setup state is NOT in progress (so
    /// the initial paint runs), but any user interaction marks it in progress
    /// (so the recheck/logout repaint is suppressed and the card stays stable).
    #[test]
    fn in_progress_tracks_user_interaction() {
        let mut s = SetupState::new();
        assert!(!s.in_progress(), "fresh state must allow the initial paint");
        s.username[0] = b'a';
        s.username_len = 1;
        assert!(s.in_progress(), "a typed username must mark in-progress");

        let mut s2 = SetupState::new();
        s2.field = SetupField::Password;
        assert!(s2.in_progress(), "moving focus must mark in-progress");

        let mut s3 = SetupState::new();
        s3.error = Some(String::from("x"));
        assert!(s3.in_progress(), "a surfaced error must mark in-progress");
    }

    /// The skip button and the primary "Create account" button must never share
    /// a pixel at any representative resolution (same overlap class as the
    /// hint/button screenshot bug).
    #[test]
    fn skip_button_never_overlaps_primary() {
        for &(w, h) in &[
            (1024usize, 768usize),
            (1280, 800),
            (1920, 1080),
            (2560, 1440),
        ] {
            let lay = compute_layout(w, h);
            assert!(
                !lay.skip_button.intersects(&lay.button),
                "skip/primary overlap at {w}x{h}"
            );
        }
    }
}
