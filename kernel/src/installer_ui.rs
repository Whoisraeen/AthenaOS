//! installer_ui — AthenaOS graphical install wizard (MasterChecklist Phase 3 / 16.1).
//!
//! Concept: *"Built for people who care about how things feel."* — the very
//! first thing a new user touches is the installer, so it must feel like a
//! finished product, not a debug console. This is the Windows-Setup-equivalent:
//! a multi-step wizard (Welcome -> pick disk -> choose layout -> account ->
//! review -> install -> done) rendered on the compositor framebuffer.
//!
//! It is the *presentation layer* over the already-proven install pipeline:
//!   * disk enumeration   -> `block_io::BLOCK_LAYER::list_devices`
//!   * keep-data planning  -> `installer::plan_layout` (full-disk vs dual-boot)
//!   * partition+format+boot-tree+AthFS -> `installer::run_install`
//!   * account creation    -> `session::create_local_account`
//!
//! No new block-write path is introduced here — every byte still flows through
//! `installer::run_install`, which routes through `block_io::safe_mode_guard_write`.
//! On a `--safe` image the wizard therefore runs as a *dry run*: it walks the
//! whole UX and the pipeline, every write is refused, and the Done screen says
//! so honestly. That makes the wizard fully exercisable on the safe iron image
//! before a single real install is ever attempted.
//!
//! Today it renders on the kernel's software framebuffer (same path as
//! `login_ui`/`setup_ui`). When the AthGFX GPU-submit path lands, the renderer
//! upgrades to GPU-composited surfaces with the same state machine underneath —
//! exactly how Windows Setup degrades to a basic display without a GPU driver.
//!
//! Visually the wizard wears the same **Liquid Glass** identity as the lock
//! screen (`lock_screen.rs`), login (`login_ui.rs`) and OOBE (`setup_ui.rs`):
//! a living **Aurora Mesh** backdrop (IDENTITY.md §3) with the wizard content
//! floating on a centered frosted `glass.panel` card (§7 tiers) — tint → frost →
//! legibility cap → iridescent rim, lifted off the aurora by a soft ambient
//! shadow. Every colour is token-derived (`rae_tokens::DARK` + the LIVE
//! `derive_accent`), so a one-tap Vibe re-skin flows through the installer too;
//! text is crisp AA RaeSans. The reskin is render-only — the install state
//! machine, disk-write pipeline and `safe_mode_guard_write` guards are untouched.

#![allow(dead_code)]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use rae_tokens::{
    AccentRamp, Palette, DARK, GLASS_PANEL_DARK, RADIUS_LG, RADIUS_MD, RADIUS_SM, RADIUS_XL,
    TYPE_BODY, TYPE_CAPTION, TYPE_LABEL, TYPE_SUBTITLE, TYPE_TITLE,
};
use raegfx::text::FontFamily;

use crate::installer::LayoutPlan;

// ── Palette (shared visual language with setup_ui / login_ui) ───────────────
// Every static colour is a `rae_tokens::DARK` value so the installer reads the
// same palette as the desktop/OOBE/login; the accent is LIVE (see `accent()`).
const BG: u32 = DARK.bg_base; // desktop void
const RAIL_BG: u32 = DARK.bg_raised; // left-rail panel surface
const FG: u32 = DARK.text_primary; // body / active labels
const MUTED: u32 = DARK.text_tertiary; // hints, inactive rail rows
const ERR: u32 = DARK.state_danger; // destructive / failure
const OK: u32 = DARK.state_ok; // success, completed stages
const WARN: u32 = DARK.state_warn; // erase/removable warnings
const FIELD_BG: u32 = DARK.bg_overlay; // text-field fill
const SEL_BG: u32 = DARK.bg_elevated; // selected list row
/// Header underline / rail hairline — `stroke.strong` (token glass edge).
const RULE: u32 = DARK.stroke_strong;

/// Active palette for the installer surface — dark default (the install wizard
/// wears the same dark Liquid Glass identity as the lock / login / OOBE screens).
const PALETTE: &Palette = &DARK;

/// Full-screen wizard-card corner radius (design-language §3 `radius.xl` = 24).
const CARD_RADIUS: usize = RADIUS_XL as usize;
/// Left-rail card corner radius (design-language §3 `radius.lg` = 16).
const RAIL_RADIUS: usize = RADIUS_LG as usize;
/// Primary / pill-button corner radius (design-language §3 `radius.md` = 12).
const BUTTON_RADIUS: usize = RADIUS_MD as usize;
/// List-row / input-field corner radius (design-language §3 `radius.sm` = 8),
/// concentric inside the larger cards.
const ROW_RADIUS: usize = RADIUS_SM as usize;

/// An axis-aligned rectangle (screen-space, px) — the glass rail/card geometry
/// the renderer lays out on. Pure layout helper for the reskin.
#[derive(Clone, Copy)]
struct Rect {
    x: usize,
    y: usize,
    w: usize,
    h: usize,
}

/// The LIVE accent ramp for the install wizard — derived from the active
/// theme/Vibe seed (`theme_engine::active_accent`), same as window chrome, the
/// login screen, the OOBE wizard and the toasts. The header title, rail
/// wordmark, active-step marker and account-field focus colour recolour on a
/// one-tap Vibe re-skin instead of being frozen to a hand-picked blue.
#[inline]
fn accent() -> AccentRamp {
    rae_tokens::derive_accent(crate::theme_engine::active_accent(), &DARK)
}

/// The accent base actually painted — public so the cross-surface cohesion
/// smoketest can confirm the installer tracks the live seed.
#[inline]
pub fn proof_accent() -> u32 {
    accent().base
}

/// The seven screens of the wizard, in order.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum InstallStep {
    Welcome,
    DiskSelect,
    Layout,
    Account,
    Review,
    Installing,
    Done,
}

impl InstallStep {
    /// Index into the left-rail step list (Installing+Done both show "Install").
    fn rail_index(self) -> usize {
        match self {
            InstallStep::Welcome => 0,
            InstallStep::DiskSelect => 1,
            InstallStep::Layout => 2,
            InstallStep::Account => 3,
            InstallStep::Review => 4,
            InstallStep::Installing | InstallStep::Done => 5,
        }
    }
}

const RAIL_STEPS: [&str; 6] = ["Welcome", "Disk", "Layout", "Account", "Review", "Install"];

/// Names of the five backend install stages, in `installer::STAGE_*` bit order.
const STAGE_LABELS: [&str; 5] = [
    "Partition table",
    "Format ESP",
    "Boot files",
    "Format AthFS",
    "Verify",
];

/// A disk the user can pick as the install target. Mirrors the metadata in
/// `block_io::BlockDeviceInfo` so the picker can be honest about size and
/// whether a disk is removable (i.e. likely the install USB itself).
#[derive(Clone)]
pub struct DiskChoice {
    pub name: String,
    pub model: String,
    pub capacity_mb: u64,
    pub removable: bool,
    pub read_only: bool,
}

impl DiskChoice {
    fn capacity_label(&self) -> String {
        if self.capacity_mb >= 1024 {
            alloc::format!("{} GB", self.capacity_mb / 1024)
        } else {
            alloc::format!("{} MB", self.capacity_mb)
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum AccountField {
    Username,
    Password,
    Confirm,
}

/// Signal returned to the shell-runner host after a keystroke is processed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum InstallSignal {
    /// Nothing changed — caller may ignore.
    Ignored,
    /// State changed — caller should re-render + present the surface.
    Repaint,
    /// User confirmed the install — caller spawns the worker thread that runs
    /// the heavy block I/O off the keyboard-IRQ path.
    BeginInstall,
    /// User backed out of the wizard — caller returns to the normal boot flow.
    Cancel,
    /// User chose "Restart" on the Done screen — caller reboots the machine.
    Reboot,
}

pub struct InstallState {
    pub step: InstallStep,
    pub disks: Vec<DiskChoice>,
    pub selected_disk: usize,
    /// What `plan_layout` detected on the target disk (drives the Layout
    /// choice screen). `DualBoot` = an existing OS was found; `FullDisk` =
    /// empty disk; `Refuse` = can't keep data, erase is the only option.
    pub detected: Option<LayoutPlan>,
    /// Layout choice when an existing OS is detected: 0 = install alongside
    /// (keep it), 1 = erase the whole disk.
    pub layout_sel: usize,
    /// The EFFECTIVE plan the user chose, applied at install time.
    pub plan: Option<LayoutPlan>,
    pub username: [u8; 32],
    pub username_len: usize,
    pub password: [u8; 64],
    pub password_len: usize,
    pub confirm: [u8; 64],
    pub confirm_len: usize,
    pub field: AccountField,
    /// Stage bitmask from the last `installer::run_install` (set by the worker).
    pub stage_result: u64,
    pub account_uid: Option<u64>,
    pub safe_mode: bool,
    /// True when the wizard was opened from a live desktop (F9), so Cancel
    /// returns to the desktop rather than to the login screen.
    pub from_desktop: bool,
    pub error: Option<String>,
    pub shift_held: bool,
}

impl InstallState {
    /// Build a wizard rooted at the Welcome screen, enumerating the real
    /// block devices currently registered.
    pub fn new() -> Self {
        Self::with_disks(enumerate_disks())
    }

    /// Construct with an explicit disk list — used by the boot smoketest so it
    /// can drive the state machine deterministically without real hardware.
    pub fn with_disks(disks: Vec<DiskChoice>) -> Self {
        Self {
            step: InstallStep::Welcome,
            disks,
            selected_disk: 0,
            detected: None,
            layout_sel: 0,
            plan: None,
            username: [0u8; 32],
            username_len: 0,
            password: [0u8; 64],
            password_len: 0,
            confirm: [0u8; 64],
            confirm_len: 0,
            field: AccountField::Username,
            stage_result: 0,
            account_uid: None,
            safe_mode: crate::block_io::safe_mode_enabled(),
            from_desktop: false,
            error: None,
            shift_held: false,
        }
    }

    fn username_str(&self) -> &str {
        core::str::from_utf8(&self.username[..self.username_len]).unwrap_or("")
    }

    /// Test/host helper: set a text field by value (real input goes byte-wise).
    pub fn set_field_text(&mut self, field: AccountField, text: &str) {
        let bytes = text.as_bytes();
        match field {
            AccountField::Username => {
                let n = bytes.len().min(self.username.len());
                self.username[..n].copy_from_slice(&bytes[..n]);
                self.username_len = n;
            }
            AccountField::Password => {
                let n = bytes.len().min(self.password.len());
                self.password[..n].copy_from_slice(&bytes[..n]);
                self.password_len = n;
            }
            AccountField::Confirm => {
                let n = bytes.len().min(self.confirm.len());
                self.confirm[..n].copy_from_slice(&bytes[..n]);
                self.confirm_len = n;
            }
        }
    }

    /// Compute the keep-data plan for the active install target. Reads the
    /// partition table only (never writes). Falls back to `Refuse` when no
    /// target device is bound, so the UI always has something honest to show.
    /// Inspect the target disk's partition table (read-only) and record what we
    /// found. `DualBoot` => an existing OS is present and we CAN install
    /// alongside; `FullDisk` => the disk is empty, so a full install is the only
    /// sensible choice; `Refuse` => partitions exist but we can't keep them, so
    /// erasing is the only path forward. Resets the layout selection.
    fn detect_layout(&mut self) {
        let guard = crate::block_io::ACTIVE_BLOCK_DEVICE.lock();
        self.detected = Some(match guard.as_ref() {
            Some(dev) => crate::installer::plan_layout(dev.as_ref()),
            None => LayoutPlan::Refuse("no install target device is bound"),
        });
        self.layout_sel = 0;
    }

    /// True when an existing OS was detected and the user is offered a choice
    /// (install alongside vs. erase the whole disk).
    fn has_dual_boot_choice(&self) -> bool {
        matches!(self.detected, Some(LayoutPlan::DualBoot { .. }))
    }

    /// Validate the account fields. `Ok(())` when ready to proceed.
    fn validate_account(&self) -> Result<(), &'static str> {
        if self.username_len == 0 {
            return Err("Pick a username to continue.");
        }
        if self.password_len == 0 {
            return Err("Pick a password to continue.");
        }
        if self.password_len != self.confirm_len
            || self.password[..self.password_len] != self.confirm[..self.confirm_len]
        {
            return Err("Passwords don't match. Try again.");
        }
        if core::str::from_utf8(&self.username[..self.username_len]).is_err() {
            return Err("Username contains invalid characters.");
        }
        Ok(())
    }
}

/// Read the registered block devices into picker choices. The list is what the
/// user sees; the actual install target is the kernel's `ACTIVE_BLOCK_DEVICE`
/// (the only writable surface the kernel exposes), so the chosen entry is
/// informational for v1 — multi-target selection needs per-disk trait objects
/// (a Phase 3 follow-up). Removable disks are flagged so the user doesn't pick
/// the install stick by mistake.
fn enumerate_disks() -> Vec<DiskChoice> {
    let layer = crate::block_io::BLOCK_LAYER.lock();
    let Some(bl) = layer.as_ref() else {
        return Vec::new();
    };
    bl.list_devices()
        .iter()
        .map(|d| DiskChoice {
            name: d.name.clone(),
            model: if d.model.is_empty() {
                String::from("Unknown")
            } else {
                d.model.clone()
            },
            capacity_mb: d.capacity_mb(),
            removable: d.removable,
            read_only: d.read_only,
        })
        .collect()
}

// ── Rendering ───────────────────────────────────────────────────────────────

impl InstallState {
    /// Render the install wizard into a raw `0xAARRGGBB` framebuffer.
    ///
    /// Concept §"Built for people who care about how things feel.": the installer
    /// is the literal first thing a new user touches, so it wears the shipped
    /// **Liquid Glass** identity — a living Aurora Mesh backdrop (IDENTITY.md §3),
    /// a frosted `glass.panel` step rail and content card (§7 tiers, with the
    /// iridescent rim + soft ambient shadow) and crisp AA RaeSans text. The
    /// public signature is unchanged (the boot caller passes `ptr` + `w`/`h`);
    /// internally we wrap the buffer in a [`raegfx::Canvas`] and draw through the
    /// shared `glass`/`draw_text_aa` primitives. Token-derived colours only — a
    /// Vibe re-skin flows here too. RENDER-ONLY: no install/disk logic touched.
    pub fn render(&self, ptr: *mut u8, w: u32, h: u32) {
        let mut canvas = unsafe { raegfx::Canvas::new(ptr, w as usize, h as usize, 4) };
        let (w, h) = (w as usize, h as usize);

        // Live accent (tracks Vibe Mode) + active palette, computed once.
        let acc = accent();
        let p: &Palette = PALETTE;
        let sans = FontFamily::Sans;

        // ── Background → the signature Aurora Mesh (IDENTITY.md §3): the same
        //    living backdrop the desktop / lock / login / OOBE wear — visual
        //    continuity from the very first install onward. Replaces the flat fill.
        raegfx::glass::render_aurora_dark(&mut canvas, 0, 0, w, h, 0);

        // ── Left step rail → a frosted glass card down the left edge, lifted off
        //    the aurora by a soft ambient shadow. radius.lg (16) for the tall rail.
        let rail_pad = 24usize;
        let rail_w = 240usize.min(w / 4).max(180);
        let rail = Rect {
            x: rail_pad,
            y: rail_pad,
            w: rail_w,
            h: h.saturating_sub(rail_pad * 2),
        };
        canvas.fill_rounded_rect_shadow(
            rail.x,
            rail.y,
            rail.w,
            rail.h,
            RAIL_RADIUS,
            0x0A_10_1C,
            40,
            16,
        );
        raegfx::glass::draw_glass_surface(
            &mut canvas,
            rail.x,
            rail.y,
            rail.w,
            rail.h,
            RAIL_RADIUS,
            GLASS_PANEL_DARK,
        );
        self.render_rail(&mut canvas, rail, &acc);

        // ── Content card → the wizard body floats on its own frosted glass.panel
        //    card to the right of the rail, at radius.xl (24) with the ambient
        //    shadow + iridescent rim.
        let gap = 24usize;
        let card = Rect {
            x: rail.x + rail.w + gap,
            y: rail_pad,
            w: w.saturating_sub(rail.x + rail.w + gap + rail_pad),
            h: h.saturating_sub(rail_pad * 2),
        };
        canvas.fill_rounded_rect_shadow(
            card.x,
            card.y,
            card.w,
            card.h,
            CARD_RADIUS,
            0x0A_10_1C,
            44,
            18,
        );
        raegfx::glass::draw_glass_surface(
            &mut canvas,
            card.x,
            card.y,
            card.w,
            card.h,
            CARD_RADIUS,
            GLASS_PANEL_DARK,
        );

        // Card content origin (padded inside the radius.xl card).
        let pad = 32usize;
        let cx = card.x + pad;
        let cw = card.w.saturating_sub(pad * 2);
        let header_y = card.y + pad;

        // Header (display) + accent underline rule.
        canvas.draw_text_aa(
            cx as i32,
            header_y as i32,
            self.header_title(),
            TYPE_TITLE,
            p.text_primary,
            sans,
        );
        let rule_y = header_y + TYPE_TITLE.line_height as usize + 8;
        canvas.fill_rounded_rect(cx, rule_y, cw.min(540), 3, 1, acc.base);

        let content_y = rule_y + 28;
        match self.step {
            InstallStep::Welcome => self.render_welcome(&mut canvas, cx, content_y, cw),
            InstallStep::DiskSelect => self.render_disks(&mut canvas, cx, content_y, cw, &acc),
            InstallStep::Layout => self.render_layout(&mut canvas, cx, content_y, cw, &acc),
            InstallStep::Account => self.render_account(&mut canvas, cx, content_y, cw, &acc),
            InstallStep::Review => self.render_review(&mut canvas, cx, content_y, cw, &acc),
            InstallStep::Installing => self.render_installing(&mut canvas, cx, content_y, cw, &acc),
            InstallStep::Done => self.render_done(&mut canvas, cx, content_y, &acc),
        }

        // Error line (rendered for every step that can set one), inside the card.
        if let Some(ref e) = self.error {
            canvas.draw_text_aa(
                cx as i32,
                (card.y + card.h - 96) as i32,
                e,
                TYPE_BODY,
                p.state_danger,
                sans,
            );
        }

        // Footer hint — caption text along the bottom of the content card.
        canvas.draw_text_aa(
            cx as i32,
            (card.y + card.h - pad - TYPE_CAPTION.line_height as usize) as i32,
            self.footer_hint(),
            TYPE_CAPTION,
            p.text_tertiary,
            sans,
        );
    }

    fn header_title(&self) -> &'static str {
        match self.step {
            InstallStep::Welcome => "Install AthenaOS",
            InstallStep::DiskSelect => "Where do you want to install?",
            InstallStep::Layout => "Choose how to use this disk",
            InstallStep::Account => "Create your account",
            InstallStep::Review => "Review and install",
            InstallStep::Installing => "Installing AthenaOS",
            InstallStep::Done => "Setup complete",
        }
    }

    fn footer_hint(&self) -> &'static str {
        match self.step {
            InstallStep::Welcome => "Enter = Begin    Esc = Cancel",
            InstallStep::DiskSelect => "Up/Down = Select    Enter = Next    Esc = Back",
            InstallStep::Layout => "Enter = Next    Esc = Back",
            InstallStep::Account => "Tab = Next field    Enter = Next    Esc = Back",
            InstallStep::Review => "Enter = Install    Esc = Back",
            InstallStep::Installing => "Please wait — do not power off.",
            InstallStep::Done => "R = Restart    Esc = Continue",
        }
    }

    fn render_rail(&self, canvas: &mut raegfx::Canvas, rail: Rect, acc: &AccentRamp) {
        let p: &Palette = PALETTE;
        let sans = FontFamily::Sans;
        let lx = rail.x + 20;

        // Wordmark — accent "AthenaOS" + secondary "Setup" subtitle.
        canvas.draw_text_aa(
            lx as i32,
            (rail.y + 24) as i32,
            "AthenaOS",
            TYPE_SUBTITLE,
            acc.base,
            sans,
        );
        canvas.draw_text_aa(
            lx as i32,
            (rail.y + 24 + TYPE_SUBTITLE.line_height as usize + 2) as i32,
            "Setup",
            TYPE_LABEL,
            p.text_secondary,
            sans,
        );

        let active = self.step.rail_index();
        let mut y = rail.y + 96;
        let row_h = 40usize;
        let dot_r = 5usize;
        for (i, label) in RAIL_STEPS.iter().enumerate() {
            // Active step gets a frosted accent pill behind the row; the dot
            // marker is accent (active), ok-green (done) or muted (pending).
            if i == active {
                canvas.fill_rounded_rect(
                    rail.x + 8,
                    y - 6,
                    rail.w.saturating_sub(16),
                    row_h - 8,
                    ROW_RADIUS,
                    GLASS_PANEL_DARK.frost,
                );
            }
            let (dot, label_color) = if i < active {
                (p.state_ok, p.text_secondary)
            } else if i == active {
                (acc.base, p.text_primary)
            } else {
                (p.text_tertiary, p.text_tertiary)
            };
            let cy = y + (TYPE_LABEL.line_height as usize) / 2;
            canvas.fill_rounded_rect(
                lx,
                cy.saturating_sub(dot_r),
                dot_r * 2,
                dot_r * 2,
                dot_r,
                dot,
            );
            canvas.draw_text_aa(
                (lx + 24) as i32,
                y as i32,
                label,
                TYPE_LABEL,
                label_color,
                sans,
            );
            y += row_h;
        }

        if self.safe_mode {
            canvas.draw_text_aa(
                lx as i32,
                (rail.y + rail.h - 48) as i32,
                "SAFE MODE",
                TYPE_LABEL,
                p.state_warn,
                sans,
            );
            canvas.draw_text_aa(
                lx as i32,
                (rail.y + rail.h - 48 + TYPE_LABEL.line_height as usize + 2) as i32,
                "writes blocked",
                TYPE_CAPTION,
                p.text_tertiary,
                sans,
            );
        }
    }

    fn render_welcome(&self, canvas: &mut raegfx::Canvas, x: usize, y: usize, _w: usize) {
        let p: &Palette = PALETTE;
        let sans = FontFamily::Sans;
        let lines = [
            "Welcome. This wizard will install AthenaOS on this computer.",
            "",
            "You'll choose a disk, decide whether to keep an existing OS,",
            "and create your account. The install itself takes a minute.",
        ];
        let row = TYPE_BODY.line_height as usize + 8;
        let mut yy = y;
        for l in lines {
            canvas.draw_text_aa(x as i32, yy as i32, l, TYPE_BODY, p.text_primary, sans);
            yy += row;
        }
        let (msg, color) = if self.safe_mode {
            (
                "Safe mode: this is a DRY RUN. No disk will be written.",
                p.state_warn,
            )
        } else {
            (
                "WARNING: installing can erase data on the chosen disk.",
                p.state_warn,
            )
        };
        canvas.draw_text_aa(x as i32, (yy + 16) as i32, msg, TYPE_BODY, color, sans);
    }

    fn render_disks(
        &self,
        canvas: &mut raegfx::Canvas,
        x: usize,
        y: usize,
        w: usize,
        acc: &AccentRamp,
    ) {
        let p: &Palette = PALETTE;
        let sans = FontFamily::Sans;
        if self.disks.is_empty() {
            canvas.draw_text_aa(
                x as i32,
                y as i32,
                "No disks detected.",
                TYPE_BODY,
                p.state_danger,
                sans,
            );
            return;
        }
        let row_h = 64usize;
        let rw = w.min(560);
        for (i, d) in self.disks.iter().enumerate() {
            let ry = y + i * row_h;
            let sel = i == self.selected_disk;
            // Selected row → frosted glass row with an accent rim; resting rows
            // get a subtle stroke hairline.
            canvas.fill_rounded_rect(x, ry, rw, row_h - 10, ROW_RADIUS, GLASS_PANEL_DARK.frost);
            if sel {
                canvas.draw_rounded_rect_outline(x, ry, rw, row_h - 10, ROW_RADIUS, acc.base);
            } else {
                canvas.draw_rounded_rect_outline(
                    x,
                    ry,
                    rw,
                    row_h - 10,
                    ROW_RADIUS,
                    p.stroke_subtle,
                );
            }
            let tx = x + 16;
            canvas.draw_text_aa(
                tx as i32,
                (ry + 8) as i32,
                &alloc::format!("{}  ({})", d.name, d.capacity_label()),
                TYPE_LABEL,
                if sel {
                    p.text_primary
                } else {
                    p.text_secondary
                },
                sans,
            );
            let kind = if d.removable {
                "removable - likely your install USB"
            } else {
                "internal disk"
            };
            let kind_color = if d.removable {
                p.state_warn
            } else {
                p.text_tertiary
            };
            canvas.draw_text_aa(
                tx as i32,
                (ry + 8 + TYPE_LABEL.line_height as usize + 4) as i32,
                &alloc::format!("{}  -  {}", d.model, kind),
                TYPE_CAPTION,
                kind_color,
                sans,
            );
        }
    }

    fn render_layout(
        &self,
        canvas: &mut raegfx::Canvas,
        x: usize,
        y: usize,
        w: usize,
        acc: &AccentRamp,
    ) {
        let p: &Palette = PALETTE;
        let sans = FontFamily::Sans;
        let rw = w.min(540);
        // A selectable option row: frosted glass card, accent rim + accent title
        // when chosen, stroke.subtle hairline otherwise.
        let option = |canvas: &mut raegfx::Canvas, oy: usize, sel: bool, title: &str, sub: &str| {
            let rh = 52usize;
            canvas.fill_rounded_rect(x, oy, rw, rh, ROW_RADIUS, GLASS_PANEL_DARK.frost);
            if sel {
                canvas.draw_rounded_rect_outline(x, oy, rw, rh, ROW_RADIUS, acc.base);
            } else {
                canvas.draw_rounded_rect_outline(x, oy, rw, rh, ROW_RADIUS, p.stroke_subtle);
            }
            canvas.draw_text_aa(
                (x + 16) as i32,
                (oy + 8) as i32,
                title,
                TYPE_LABEL,
                if sel { acc.base } else { p.text_secondary },
                sans,
            );
            canvas.draw_text_aa(
                (x + 16) as i32,
                (oy + 8 + TYPE_LABEL.line_height as usize + 4) as i32,
                sub,
                TYPE_CAPTION,
                p.text_tertiary,
                sans,
            );
        };

        match &self.detected {
            Some(LayoutPlan::DualBoot { raefs_sectors, .. }) => {
                let gb = (raefs_sectors * 512) / (1024 * 1024 * 1024);
                canvas.draw_text_aa(
                    x as i32,
                    y as i32,
                    "An existing operating system was found.",
                    TYPE_BODY,
                    p.state_warn,
                    sans,
                );
                option(
                    canvas,
                    y + 36,
                    self.layout_sel == 0,
                    "Install alongside it  (recommended)",
                    &alloc::format!(
                        "Keeps your OS and files. AthenaOS uses ~{} GB of free space.",
                        gb
                    ),
                );
                option(
                    canvas,
                    y + 102,
                    self.layout_sel == 1,
                    "Erase the entire disk and install AthenaOS",
                    if self.safe_mode {
                        "Everything on the disk is removed (dry run in safe mode)."
                    } else {
                        "WARNING: every partition, including the existing OS, is erased."
                    },
                );
            }
            Some(LayoutPlan::FullDisk) => {
                canvas.draw_text_aa(
                    x as i32,
                    y as i32,
                    "This disk is empty.",
                    TYPE_BODY,
                    p.text_primary,
                    sans,
                );
                option(
                    canvas,
                    y + 36,
                    true,
                    "Install AthenaOS (uses the whole disk)",
                    "Creates a GPT, an EFI partition, and a AthFS root.",
                );
            }
            Some(LayoutPlan::Refuse(why)) => {
                canvas.draw_text_aa(
                    x as i32,
                    y as i32,
                    "Can't install alongside the existing data.",
                    TYPE_BODY,
                    p.state_danger,
                    sans,
                );
                canvas.draw_text_aa(
                    x as i32,
                    (y + TYPE_BODY.line_height as usize + 6) as i32,
                    why,
                    TYPE_CAPTION,
                    p.text_tertiary,
                    sans,
                );
                option(
                    canvas,
                    y + 64,
                    true,
                    "Erase the entire disk and install AthenaOS",
                    if self.safe_mode {
                        "Everything on the disk is removed (dry run in safe mode)."
                    } else {
                        "WARNING: every partition on this disk is erased."
                    },
                );
            }
            None => {
                canvas.draw_text_aa(
                    x as i32,
                    y as i32,
                    "Analyzing disk...",
                    TYPE_BODY,
                    p.text_tertiary,
                    sans,
                );
            }
        }
    }

    fn render_account(
        &self,
        canvas: &mut raegfx::Canvas,
        x: usize,
        y: usize,
        w: usize,
        acc: &AccentRamp,
    ) {
        let p: &Palette = PALETTE;
        let sans = FontFamily::Sans;
        let fw = w.min(360);
        let fh = 40usize;
        // A glass-card input pill: label above, frosted field with a LIVE-accent
        // focus ring when active (matches login_ui/setup_ui), placeholder/text AA.
        let field = |canvas: &mut raegfx::Canvas,
                     fy: usize,
                     label: &str,
                     text: &str,
                     placeholder: &str,
                     sel: bool| {
            canvas.draw_text_aa(
                x as i32,
                fy as i32,
                label,
                TYPE_LABEL,
                p.text_secondary,
                sans,
            );
            let by = fy + TYPE_LABEL.line_height as usize + 6;
            canvas.fill_rounded_rect(x, by, fw, fh, ROW_RADIUS, GLASS_PANEL_DARK.frost);
            if sel {
                canvas.draw_rounded_rect_outline(x, by, fw, fh, ROW_RADIUS, acc.base);
                canvas.draw_rounded_rect_outline(
                    x + 1,
                    by + 1,
                    fw.saturating_sub(2),
                    fh.saturating_sub(2),
                    ROW_RADIUS.saturating_sub(1),
                    acc.glow,
                );
            } else {
                canvas.draw_rounded_rect_outline(x, by, fw, fh, ROW_RADIUS, p.stroke_subtle);
            }
            let ty = by + (fh - TYPE_BODY.line_height as usize) / 2;
            if text.is_empty() {
                canvas.draw_text_aa(
                    (x + 14) as i32,
                    ty as i32,
                    placeholder,
                    TYPE_BODY,
                    p.text_tertiary,
                    sans,
                );
            } else {
                canvas.draw_text_aa(
                    (x + 14) as i32,
                    ty as i32,
                    text,
                    TYPE_BODY,
                    p.text_primary,
                    sans,
                );
            }
        };
        let group = TYPE_LABEL.line_height as usize + 6 + fh + 24;
        field(
            canvas,
            y,
            "Username",
            self.username_str(),
            "Choose a username",
            self.field == AccountField::Username,
        );
        let stars: String = core::iter::repeat('*').take(self.password_len).collect();
        field(
            canvas,
            y + group,
            "Password",
            &stars,
            "Enter a password",
            self.field == AccountField::Password,
        );
        let stars2: String = core::iter::repeat('*').take(self.confirm_len).collect();
        field(
            canvas,
            y + group * 2,
            "Confirm password",
            &stars2,
            "Re-enter password",
            self.field == AccountField::Confirm,
        );
    }

    fn render_review(
        &self,
        canvas: &mut raegfx::Canvas,
        x: usize,
        y: usize,
        w: usize,
        acc: &AccentRamp,
    ) {
        let p: &Palette = PALETTE;
        let sans = FontFamily::Sans;
        let disk = self
            .disks
            .get(self.selected_disk)
            .map(|d| alloc::format!("{} ({})", d.name, d.capacity_label()))
            .unwrap_or_else(|| String::from("active device"));
        let layout = match &self.plan {
            Some(LayoutPlan::FullDisk) => "Erase disk, use entire disk",
            Some(LayoutPlan::DualBoot { .. }) => "Keep existing OS, install alongside",
            Some(LayoutPlan::Refuse(_)) => "Unavailable",
            None => "Unknown",
        };
        let rows = [
            ("Disk", disk),
            ("Layout", String::from(layout)),
            ("Account", String::from(self.username_str())),
        ];
        let row = TYPE_BODY.line_height as usize + 12;
        let mut yy = y;
        for (label, val) in rows {
            canvas.draw_text_aa(
                x as i32,
                yy as i32,
                label,
                TYPE_LABEL,
                p.text_tertiary,
                sans,
            );
            canvas.draw_text_aa(
                (x + 120) as i32,
                yy as i32,
                &val,
                TYPE_BODY,
                p.text_primary,
                sans,
            );
            yy += row;
        }
        let msg = if self.safe_mode {
            "This is a dry run — no disk writes."
        } else {
            "The chosen disk will be modified."
        };
        canvas.draw_text_aa(
            x as i32,
            (yy + 12) as i32,
            msg,
            TYPE_BODY,
            if self.safe_mode {
                p.text_tertiary
            } else {
                p.state_warn
            },
            sans,
        );

        // Primary "Install" → accent-filled pill with dark-on-accent INK (the
        // IDENTITY guardrail: ink on an accent fill is bg.base). LIVE accent fill.
        let label = if self.safe_mode {
            "Run dry install"
        } else {
            "Install"
        };
        let bw = (label.len() * 11 + 48).min(w);
        let bh = 44usize;
        let by = yy + 12 + TYPE_BODY.line_height as usize + 24;
        canvas.fill_rounded_rect(x, by, bw, bh, BUTTON_RADIUS, acc.base);
        canvas.draw_text_aa(
            (x + 24) as i32,
            (by + (bh - TYPE_LABEL.line_height as usize) / 2) as i32,
            label,
            TYPE_LABEL,
            p.bg_base,
            sans,
        );
    }

    fn render_installing(
        &self,
        canvas: &mut raegfx::Canvas,
        x: usize,
        y: usize,
        w: usize,
        acc: &AccentRamp,
    ) {
        let p: &Palette = PALETTE;
        let sans = FontFamily::Sans;
        // Progress bar → token accent fill on a frosted track, proportion of the
        // five backend stages completed.
        let done_count = (0..STAGE_LABELS.len())
            .filter(|i| self.stage_result & (1 << i) != 0)
            .count();
        let bar_w = w.min(540);
        let bar_h = 10usize;
        canvas.fill_rounded_rect(x, y, bar_w, bar_h, bar_h / 2, GLASS_PANEL_DARK.frost);
        let fill = bar_w * done_count / STAGE_LABELS.len();
        if fill > 0 {
            canvas.fill_rounded_rect(x, y, fill.max(bar_h), bar_h, bar_h / 2, acc.base);
        }

        let list_y = y + bar_h + 24;
        let row = TYPE_BODY.line_height as usize + 12;
        for (i, label) in STAGE_LABELS.iter().enumerate() {
            let done = self.stage_result & (1 << i) != 0;
            let (dot, color) = if done {
                (p.state_ok, p.text_primary)
            } else {
                (p.text_tertiary, p.text_tertiary)
            };
            let ry = list_y + i * row;
            let cy = ry + (TYPE_BODY.line_height as usize) / 2;
            canvas.fill_rounded_rect(x, cy.saturating_sub(5), 10, 10, 5, dot);
            canvas.draw_text_aa((x + 24) as i32, ry as i32, label, TYPE_BODY, color, sans);
        }
    }

    fn render_done(&self, canvas: &mut raegfx::Canvas, x: usize, y: usize, _acc: &AccentRamp) {
        let p: &Palette = PALETTE;
        let sans = FontFamily::Sans;
        let all = crate::installer::STAGE_GPT
            | crate::installer::STAGE_ESP_FORMAT
            | crate::installer::STAGE_BOOT_TREE
            | crate::installer::STAGE_RAEFS_FORMAT
            | crate::installer::STAGE_VERIFY;
        let got = (self.stage_result & all).count_ones();
        let body_row = TYPE_BODY.line_height as usize + 8;
        if self.safe_mode {
            canvas.draw_text_aa(
                x as i32,
                y as i32,
                "Dry run complete (safe mode).",
                TYPE_SUBTITLE,
                p.state_warn,
                sans,
            );
            canvas.draw_text_aa(
                x as i32,
                (y + 40) as i32,
                "All disk writes were blocked, as expected on a safe image.",
                TYPE_BODY,
                p.text_secondary,
                sans,
            );
            canvas.draw_text_aa(
                x as i32,
                (y + 40 + body_row) as i32,
                "Flash a non-safe image to perform a real install.",
                TYPE_BODY,
                p.text_tertiary,
                sans,
            );
        } else if got == 5 {
            canvas.draw_text_aa(
                x as i32,
                y as i32,
                "AthenaOS is installed.",
                TYPE_SUBTITLE,
                p.state_ok,
                sans,
            );
            canvas.draw_text_aa(
                x as i32,
                (y + 40) as i32,
                "Remove the install media and restart.",
                TYPE_BODY,
                p.text_primary,
                sans,
            );
        } else {
            canvas.draw_text_aa(
                x as i32,
                y as i32,
                &alloc::format!("Install incomplete: {}/5 stages succeeded.", got),
                TYPE_SUBTITLE,
                p.state_danger,
                sans,
            );
            canvas.draw_text_aa(
                x as i32,
                (y + 40) as i32,
                "See the serial/bootlog for [install] lines.",
                TYPE_BODY,
                p.text_tertiary,
                sans,
            );
        }
        // Per-stage recap — accent dot per stage.
        let recap_y = y + 96;
        let row = TYPE_BODY.line_height as usize + 10;
        for (i, label) in STAGE_LABELS.iter().enumerate() {
            let done = self.stage_result & (1 << i) != 0;
            let (dot, color) = if done {
                (p.state_ok, p.text_secondary)
            } else {
                (p.state_danger, p.text_tertiary)
            };
            let ry = recap_y + i * row;
            let cy = ry + (TYPE_BODY.line_height as usize) / 2;
            canvas.fill_rounded_rect(x, cy.saturating_sub(5), 10, 10, 5, dot);
            canvas.draw_text_aa((x + 24) as i32, ry as i32, label, TYPE_BODY, color, sans);
        }
    }
}

// ── Input ───────────────────────────────────────────────────────────────────

/// Handle one PS/2 set-1 make code (`extended` = preceded by the 0xE0 prefix).
/// Returns an `InstallSignal` telling the shell-runner host what to do next.
pub fn handle_key(state: &mut InstallState, extended: bool, scancode: u8) -> InstallSignal {
    let code = scancode & 0x7F;

    // Shift latch (mirrors setup_ui — releases are filtered upstream, so this
    // tracks the make code only; sufficient for the keyboard-light bring-up).
    if code == 0x2A || code == 0x36 {
        state.shift_held = true;
        return InstallSignal::Ignored;
    }

    LAST_STEP.store(step_index(state.step), Ordering::Relaxed);

    match state.step {
        InstallStep::Welcome => {
            if code == 0x1C {
                state.step = InstallStep::DiskSelect;
                state.error = None;
                InstallSignal::Repaint
            } else if code == 0x01 {
                InstallSignal::Cancel
            } else {
                InstallSignal::Ignored
            }
        }
        InstallStep::DiskSelect => match (extended, code) {
            (true, 0x48) => {
                // Up
                if state.selected_disk > 0 {
                    state.selected_disk -= 1;
                }
                InstallSignal::Repaint
            }
            (true, 0x50) => {
                // Down
                if state.selected_disk + 1 < state.disks.len() {
                    state.selected_disk += 1;
                }
                InstallSignal::Repaint
            }
            (_, 0x1C) => {
                state.detect_layout();
                state.step = InstallStep::Layout;
                state.error = None;
                InstallSignal::Repaint
            }
            (_, 0x01) => {
                state.step = InstallStep::Welcome;
                InstallSignal::Repaint
            }
            _ => InstallSignal::Ignored,
        },
        InstallStep::Layout => match (extended, code) {
            // Up/Down toggle the alongside-vs-erase choice (only when an
            // existing OS was detected, i.e. there are two options).
            (true, 0x48) if state.has_dual_boot_choice() => {
                state.layout_sel = 0;
                InstallSignal::Repaint
            }
            (true, 0x50) if state.has_dual_boot_choice() => {
                state.layout_sel = 1;
                InstallSignal::Repaint
            }
            (_, 0x1C) => {
                // Resolve the effective plan from what was detected + the choice.
                let effective = match &state.detected {
                    Some(LayoutPlan::DualBoot { .. }) => {
                        if state.layout_sel == 0 {
                            state.detected.clone() // install alongside
                        } else {
                            Some(LayoutPlan::FullDisk) // user chose to erase
                        }
                    }
                    // Empty disk, or can't-keep-data: full install is the path.
                    Some(LayoutPlan::FullDisk) | Some(LayoutPlan::Refuse(_)) => {
                        Some(LayoutPlan::FullDisk)
                    }
                    None => None,
                };
                match effective {
                    Some(p) => {
                        state.plan = Some(p);
                        state.step = InstallStep::Account;
                        state.error = None;
                        InstallSignal::Repaint
                    }
                    None => {
                        state.error = Some(String::from("No target disk — go back (Esc)."));
                        InstallSignal::Repaint
                    }
                }
            }
            (_, 0x01) => {
                state.step = InstallStep::DiskSelect;
                state.error = None;
                InstallSignal::Repaint
            }
            _ => InstallSignal::Ignored,
        },
        InstallStep::Account => handle_account_key(state, code),
        InstallStep::Review => {
            if code == 0x1C {
                InstallSignal::BeginInstall
            } else if code == 0x01 {
                state.step = InstallStep::Account;
                InstallSignal::Repaint
            } else {
                InstallSignal::Ignored
            }
        }
        InstallStep::Installing => InstallSignal::Ignored,
        InstallStep::Done => {
            if code == 0x13 {
                // 'R' — restart
                InstallSignal::Reboot
            } else if code == 0x01 || code == 0x1C {
                InstallSignal::Cancel
            } else {
                InstallSignal::Ignored
            }
        }
    }
}

fn handle_account_key(state: &mut InstallState, code: u8) -> InstallSignal {
    // Tab — cycle fields.
    if code == 0x0F {
        state.field = match state.field {
            AccountField::Username => AccountField::Password,
            AccountField::Password => AccountField::Confirm,
            AccountField::Confirm => AccountField::Username,
        };
        return InstallSignal::Repaint;
    }
    // Esc — back.
    if code == 0x01 {
        state.step = InstallStep::Layout;
        state.error = None;
        return InstallSignal::Repaint;
    }
    // Enter — validate + advance.
    if code == 0x1C {
        match state.validate_account() {
            Ok(()) => {
                state.step = InstallStep::Review;
                state.error = None;
            }
            Err(e) => {
                state.error = Some(String::from(e));
                if e.starts_with("Passwords") {
                    state.confirm_len = 0;
                    state.field = AccountField::Confirm;
                }
            }
        }
        return InstallSignal::Repaint;
    }
    // Backspace.
    if code == 0x0E {
        match state.field {
            AccountField::Username if state.username_len > 0 => state.username_len -= 1,
            AccountField::Password if state.password_len > 0 => state.password_len -= 1,
            AccountField::Confirm if state.confirm_len > 0 => state.confirm_len -= 1,
            _ => {}
        }
        return InstallSignal::Repaint;
    }
    // Printable.
    if let Some(ch) = scancode_to_ascii(code, state.shift_held) {
        if ch >= 0x20 {
            match state.field {
                AccountField::Username => {
                    if state.username_len < state.username.len() {
                        state.username[state.username_len] = ch;
                        state.username_len += 1;
                    }
                }
                AccountField::Password => {
                    if state.password_len < state.password.len() {
                        state.password[state.password_len] = ch;
                        state.password_len += 1;
                    }
                }
                AccountField::Confirm => {
                    if state.confirm_len < state.confirm.len() {
                        state.confirm[state.confirm_len] = ch;
                        state.confirm_len += 1;
                    }
                }
            }
            return InstallSignal::Repaint;
        }
    }
    InstallSignal::Ignored
}

/// PS/2 set-1 scancode -> ASCII (private copy; the installer's text entry is
/// intentionally decoupled from login/setup so UX tweaks don't cross-wire).
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
    (ch != 0).then_some(ch)
}

/// Reboot the machine — used by the Done screen's "Restart" action. Tries the
/// chipset reset-control register (0xCF9) first, then pulses the 8042 reset
/// line as a fallback. Never returns.
pub fn reboot() -> ! {
    use x86_64::instructions::port::Port;
    crate::serial_println!("[install] restart requested — resetting machine");
    crate::bootlog_persist::flush();
    unsafe {
        // 0xCF9: set RST_CPU (bit 1), then full reset (bit 2 | bit 1).
        let mut rst: Port<u8> = Port::new(0xCF9);
        rst.write(0x02u8);
        rst.write(0x06u8);
        // 8042 pulse reset fallback.
        let mut kbd: Port<u8> = Port::new(0x64);
        kbd.write(0xFEu8);
    }
    // TRIPLE-FAULT FALLBACK — the Beelink Athena ignores the 0xCF9 + 8042 resets
    // (bare-metal boot 2026-06-27 sat in an hlt loop after reboot(), never reset).
    // Loading a null IDT and faulting forces a double→triple fault, which ALWAYS
    // resets an x86 CPU. This is the universal reset and the reliable path for the
    // SSH-driven bare-metal test loop (so the machine returns to Linux on its own).
    unsafe {
        let null_idt = x86_64::structures::DescriptorTablePointer {
            limit: 0,
            base: x86_64::VirtAddr::new(0),
        };
        x86_64::instructions::tables::lidt(&null_idt);
        core::arch::asm!("int3", options(noreturn));
    }
}

/// Reset the machine with the GUARANTEED triple-fault path ONLY — no bootlog flush.
/// The SAFE-MODE bare-metal auto-return safety net uses this: it must NEVER depend on the
/// NVMe BOOTLOG.TXT write, because a live NVMe controller can block that write bare-metal
/// (the live-controller-blocks pattern), and that exact stall stranded the Athena
/// 2026-06-28 (AthenaOS running + ping-able, but no auto-reboot, no SSH). The netlog (UDP)
/// carries the capture instead; this only guarantees the box returns to Linux.
pub fn reboot_no_flush() -> ! {
    use x86_64::instructions::tables::lidt;
    crate::serial_println!(
        "[reboot] no-flush triple-fault reset (SAFE-MODE bare-metal auto-return)"
    );
    unsafe {
        let null_idt = x86_64::structures::DescriptorTablePointer {
            limit: 0,
            base: x86_64::VirtAddr::new(0),
        };
        lidt(&null_idt);
        core::arch::asm!("int3", options(noreturn));
    }
}

/// IRQ-safe variant for the safe-mode LAPIC deadline. This deliberately emits
/// no log line: a timer interrupt may have preempted code holding SERIAL1, and
/// acquiring that lock here would defeat the scheduler-independent reset.
pub fn reboot_no_flush_irq() -> ! {
    unsafe {
        let null_idt = x86_64::structures::DescriptorTablePointer {
            limit: 0,
            base: x86_64::VirtAddr::new(0),
        };
        x86_64::instructions::tables::lidt(&null_idt);
        core::arch::asm!("int3", options(noreturn));
    }
}

// ── Status / procfs (R10) ─────────────────────────────────────────────────

fn step_index(step: InstallStep) -> u8 {
    match step {
        InstallStep::Welcome => 0,
        InstallStep::DiskSelect => 1,
        InstallStep::Layout => 2,
        InstallStep::Account => 3,
        InstallStep::Review => 4,
        InstallStep::Installing => 5,
        InstallStep::Done => 6,
    }
}

static SMOKE_PASS: AtomicU8 = AtomicU8::new(0); // 0 not run, 1 pass, 2 fail
static LAST_STEP: AtomicU8 = AtomicU8::new(0);
static LAST_RESULT: AtomicU64 = AtomicU64::new(0);

/// Record the outcome of a real (worker-driven) install run for /proc.
pub fn record_install_result(result: u64) {
    LAST_RESULT.store(result, Ordering::Relaxed);
    LAST_STEP.store(step_index(InstallStep::Done), Ordering::Relaxed);
}

pub fn dump_text() -> String {
    let step_names = [
        "Welcome",
        "DiskSelect",
        "Layout",
        "Account",
        "Review",
        "Installing",
        "Done",
    ];
    alloc::format!(
        "# AthenaOS install wizard (UI)\nsmoketest: {}\nlast_step: {}\nlast_install_result: {:#07b}\nsafe_mode: {}\n",
        match SMOKE_PASS.load(Ordering::Relaxed) {
            1 => "PASS",
            2 => "FAIL",
            _ => "not run",
        },
        step_names
            .get(LAST_STEP.load(Ordering::Relaxed) as usize)
            .copied()
            .unwrap_or("?"),
        LAST_RESULT.load(Ordering::Relaxed),
        crate::block_io::safe_mode_enabled(),
    )
}

/// Boot smoketest (R10): drive the wizard state machine deterministically over
/// two synthetic disks and assert every transition, including that bad input
/// is *rejected* (a test that can print FAIL).
pub fn run_boot_smoketest() {
    let disks = alloc::vec![
        DiskChoice {
            name: String::from("nvme0"),
            model: String::from("Test NVMe"),
            capacity_mb: 256 * 1024,
            removable: false,
            read_only: false,
        },
        DiskChoice {
            name: String::from("usb0"),
            model: String::from("Install Stick"),
            capacity_mb: 16 * 1024,
            removable: true,
            read_only: false,
        },
    ];
    let mut s = InstallState::with_disks(disks);
    let mut ok = true;

    // Welcome -> Enter -> DiskSelect.
    ok &= handle_key(&mut s, false, 0x1C) == InstallSignal::Repaint;
    ok &= s.step == InstallStep::DiskSelect;

    // Down selects disk 1, Up returns to 0, clamped at the ends.
    handle_key(&mut s, true, 0x50);
    ok &= s.selected_disk == 1;
    handle_key(&mut s, true, 0x50); // already last — clamp
    ok &= s.selected_disk == 1;
    handle_key(&mut s, true, 0x48);
    ok &= s.selected_disk == 0;

    // Enter -> Layout (detection ran; Some even with no active device).
    handle_key(&mut s, false, 0x1C);
    ok &= s.step == InstallStep::Layout && s.detected.is_some();

    // ── Layout choice logic (the new dual-boot detection) ───────────────────
    // (a) Existing OS detected + "install alongside" (sel 0) -> effective plan
    //     is DualBoot.
    s.detected = Some(LayoutPlan::DualBoot {
        esp_lba: 34,
        esp_sectors: 200,
        raefs_start: 2240,
        raefs_sectors: 40_000_000,
    });
    s.layout_sel = 0;
    handle_key(&mut s, false, 0x1C);
    ok &= s.step == InstallStep::Account && matches!(s.plan, Some(LayoutPlan::DualBoot { .. }));

    // (b) Same disk, but the user picks "erase" (Down -> sel 1) -> FullDisk.
    s.step = InstallStep::Layout;
    s.detected = Some(LayoutPlan::DualBoot {
        esp_lba: 34,
        esp_sectors: 200,
        raefs_start: 2240,
        raefs_sectors: 40_000_000,
    });
    s.layout_sel = 0;
    handle_key(&mut s, true, 0x50); // Down -> erase
    ok &= s.layout_sel == 1;
    handle_key(&mut s, false, 0x1C);
    ok &= matches!(s.plan, Some(LayoutPlan::FullDisk));

    // (c) Empty disk detected -> FullDisk (no choice needed).
    s.step = InstallStep::Layout;
    s.detected = Some(LayoutPlan::FullDisk);
    handle_key(&mut s, false, 0x1C);
    ok &= matches!(s.plan, Some(LayoutPlan::FullDisk));

    // (d) Can't-keep-data (Refuse) -> erase fallback is FullDisk.
    s.step = InstallStep::Layout;
    s.detected = Some(LayoutPlan::Refuse("no free space"));
    handle_key(&mut s, false, 0x1C);
    ok &= s.step == InstallStep::Account && matches!(s.plan, Some(LayoutPlan::FullDisk));

    // Account: mismatched passwords must be REJECTED (the falsifiable check).
    s.set_field_text(AccountField::Username, "tester");
    s.set_field_text(AccountField::Password, "alpha1");
    s.set_field_text(AccountField::Confirm, "beta22");
    handle_key(&mut s, false, 0x1C);
    ok &= s.step == InstallStep::Account && s.error.is_some();

    // Fix confirmation -> advance to Review.
    s.set_field_text(AccountField::Confirm, "alpha1");
    handle_key(&mut s, false, 0x1C);
    ok &= s.step == InstallStep::Review && s.error.is_none();

    // Review -> Enter -> BeginInstall signal.
    ok &= handle_key(&mut s, false, 0x1C) == InstallSignal::BeginInstall;

    // Typed input lands in the right field.
    let mut t = InstallState::with_disks(Vec::new());
    t.step = InstallStep::Account;
    t.field = AccountField::Username;
    for c in [0x14u8, 0x12, 0x1F, 0x14] {
        // t e s t
        handle_key(&mut t, false, c);
    }
    ok &= &t.username[..t.username_len] == b"test";

    // ── Token + live-accent cohesion (fail-able) ────────────────────────────
    // The wizard's header/rail/active-step accent must be
    // derive_accent(active_accent()).base, and the static palette must be the
    // DARK tokens (not re-hardcoded hex). If either drifts, prints FAIL.
    let want_accent = rae_tokens::derive_accent(crate::theme_engine::active_accent(), &DARK).base;
    let accent_ok = proof_accent() == want_accent;
    let palette_ok = BG == DARK.bg_base
        && RAIL_BG == DARK.bg_raised
        && FG == DARK.text_primary
        && FIELD_BG == DARK.bg_overlay
        && SEL_BG == DARK.bg_elevated
        && WARN == DARK.state_warn
        && ERR == DARK.state_danger;
    ok &= accent_ok && palette_ok;

    SMOKE_PASS.store(if ok { 1 } else { 2 }, Ordering::Relaxed);
    crate::serial_println!(
        "[install] ui smoketest: welcome->disk->layout(dualboot:alongside/erase, empty->full, refuse->erase)->account(reject mismatch)->review->install -> {}",
        if ok { "PASS" } else { "FAIL" },
    );
    crate::serial_println!(
        "[installer] ui: accent={:#010X} palette={} -> {}",
        proof_accent(),
        if palette_ok { "tokens" } else { "DRIFT" },
        if accent_ok && palette_ok {
            "PASS"
        } else {
            "FAIL"
        },
    );
}
