//! Theme engine — Concept §Customization Engine:
//!
//! > "Theme engine at the compositor level — themes change the actual
//! >  rendering, not just colors. Frosted glass, holographic, CRT
//! >  scanlines, neo-noir, brutalist, whatever. Themes ship as small
//! >  declarative bundles, signed and sandboxed."
//!
//! Most desktop "themes" are colour-only swaps (Windows accent colour,
//! macOS Dark Mode, GTK CSS). AthenaOS treats a theme as a record the
//! compositor can act on at draw time — colour palette, font, animation
//! curves, blur radius, scanline overlay, glassmorphism amount, cursor
//! style, system sound pack, even per-event particle behaviour. A theme
//! bundle is a signed manifest plus optional shader+image assets, but the
//! kernel-side surface is a small fixed-ABI struct so the compositor and
//! Settings app can speak it without parsing.
//!
//! ## Syscalls (74-77)
//!
//! | nr | name           | rdi/rsi/rdx                                           | rax |
//! |----|----------------|-------------------------------------------------------|----|
//! | 74 | THEME_LIST     | rdi=out_ptr, rsi=out_cap (32 B per theme: name+id)    | count |
//! | 75 | THEME_QUERY    | rdi=theme_id, rsi=out_ptr (ThemeAbi)                  | bytes |
//! | 76 | THEME_APPLY    | rdi=theme_id                                          | 0/err |
//! | 77 | THEME_REGISTER | rdi=bundle_ptr, rsi=bundle_len (signed manifest)      | new id / err |

#![allow(dead_code)]

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use spin::Mutex;

/// The default seed accent — "RaeBlue" (matches `ath_tokens::RAEBLUE` and the
/// "RaeBlue Default" builtin's `accent_argb`). Single source of truth for the
/// fall-back when no theme is current.
pub const RAEBLUE: u32 = 0xFF_4E_9C_FF;

/// Live accent override. `0` (sentinel) = "no override, use the current
/// theme's `accent_argb`". A non-zero value is the live seed every re-skinned
/// surface reads via [`active_accent`] — used by the Settings colour picker /
/// custom-accent path and by the cohesion smoketest to inject a distinctive
/// seed. [`apply`] clears it so picking a signed theme wins.
static ACCENT_OVERRIDE: AtomicU32 = AtomicU32::new(0);

// ── Theme schema ───────────────────────────────────────────────────────

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct ThemeAbi {
    pub version: u32, // = 1
    pub id: u64,
    pub flags: u32, // bit 0 signed, 1 sandboxed, 2 builtin, 3 dynamic
    pub accent_argb: u32,
    pub bg_argb: u32,
    pub fg_argb: u32,
    pub muted_argb: u32,
    pub glass_amount: u32,  // 0..=100 — Concept "glassmorphism by default"
    pub blur_radius: u32,   // pixels
    pub corner_radius: u32, // pixels
    pub anim_curve: u32,    // 0=linear, 1=ease-out, 2=spring, 3=cubic-bezier
    pub anim_ms: u32,
    pub scanline_amt: u32,  // 0..=100, "CRT scanlines"
    pub particle_kind: u32, // 0=none, 1=sparkle, 2=neon-trail, 3=cherry-blossom
    pub font_family: [u8; 24],
    pub cursor_style: [u8; 24],
    pub name: [u8; 32],
}

impl ThemeAbi {
    /// Serialize into the exact repr(C) byte layout for a SMAP-safe copy_to_user.
    /// Non-obvious padding: `id`(u64) forces 4 pad bytes after `version`@0; the
    /// rest packs with no gaps. Layout: version@0, pad@4, id@8, flags@16,
    ///   accent@20, bg@24, fg@28, muted@32, glass@36, blur@40, corner@44,
    ///   anim_curve@48, anim_ms@52, scanline@56, particle@60, font_family@64,
    ///   cursor_style@88, name@112 → total 144.
    fn to_le_bytes(&self) -> [u8; 144] {
        debug_assert_eq!(core::mem::size_of::<ThemeAbi>(), 144);
        let mut b = [0u8; 144];
        b[0..4].copy_from_slice(&self.version.to_le_bytes());
        b[8..16].copy_from_slice(&self.id.to_le_bytes());
        b[16..20].copy_from_slice(&self.flags.to_le_bytes());
        b[20..24].copy_from_slice(&self.accent_argb.to_le_bytes());
        b[24..28].copy_from_slice(&self.bg_argb.to_le_bytes());
        b[28..32].copy_from_slice(&self.fg_argb.to_le_bytes());
        b[32..36].copy_from_slice(&self.muted_argb.to_le_bytes());
        b[36..40].copy_from_slice(&self.glass_amount.to_le_bytes());
        b[40..44].copy_from_slice(&self.blur_radius.to_le_bytes());
        b[44..48].copy_from_slice(&self.corner_radius.to_le_bytes());
        b[48..52].copy_from_slice(&self.anim_curve.to_le_bytes());
        b[52..56].copy_from_slice(&self.anim_ms.to_le_bytes());
        b[56..60].copy_from_slice(&self.scanline_amt.to_le_bytes());
        b[60..64].copy_from_slice(&self.particle_kind.to_le_bytes());
        b[64..88].copy_from_slice(&self.font_family);
        b[88..112].copy_from_slice(&self.cursor_style);
        b[112..144].copy_from_slice(&self.name);
        b
    }
}

pub const FLAG_SIGNED: u32 = 1 << 0;
pub const FLAG_SANDBOXED: u32 = 1 << 1;
pub const FLAG_BUILTIN: u32 = 1 << 2;
pub const FLAG_DYNAMIC: u32 = 1 << 3;

#[derive(Debug, Clone)]
struct Theme {
    abi: ThemeAbi,
}

// ── Registry ───────────────────────────────────────────────────────────

struct Registry {
    themes: BTreeMap<u64, Theme>,
    current: Option<u64>,
    apply_count: u64,
}

static NEXT_ID: AtomicU64 = AtomicU64::new(1);
static REGISTRY: Mutex<Option<Registry>> = Mutex::new(None);

fn name_bytes(s: &str) -> [u8; 32] {
    let mut a = [0u8; 32];
    let b = s.as_bytes();
    let n = b.len().min(32);
    a[..n].copy_from_slice(&b[..n]);
    a
}
fn label24(s: &str) -> [u8; 24] {
    let mut a = [0u8; 24];
    let b = s.as_bytes();
    let n = b.len().min(24);
    a[..n].copy_from_slice(&b[..n]);
    a
}

fn seed(reg: &mut Registry) {
    let builtin: &[(&str, ThemeAbi)] = &[
        (
            "RaeBlue Default",
            ThemeAbi {
                version: 1,
                id: 0,
                flags: FLAG_BUILTIN | FLAG_SIGNED,
                accent_argb: 0xFF_4E_9C_FF,
                bg_argb: 0xFF_0A_0E_1A,
                fg_argb: 0xFF_E0_E4_F0,
                muted_argb: 0xFF_70_78_88,
                glass_amount: 60,
                blur_radius: 16,
                corner_radius: 8,
                anim_curve: 1,
                anim_ms: 180,
                scanline_amt: 0,
                particle_kind: 0,
                font_family: label24("Inter"),
                cursor_style: label24("Default"),
                name: name_bytes("RaeBlue Default"),
            },
        ),
        (
            "Cyberpunk Night",
            ThemeAbi {
                version: 1,
                id: 0,
                flags: FLAG_BUILTIN | FLAG_SIGNED | FLAG_DYNAMIC,
                accent_argb: 0xFF_FF_2A_88,
                bg_argb: 0xFF_03_05_0A,
                fg_argb: 0xFF_F0_FF_FF,
                muted_argb: 0xFF_44_88_AA,
                glass_amount: 80,
                blur_radius: 24,
                corner_radius: 2,
                anim_curve: 2,
                anim_ms: 120,
                scanline_amt: 25,
                particle_kind: 2,
                font_family: label24("JetBrains Mono"),
                cursor_style: label24("Crosshair Neon"),
                name: name_bytes("Cyberpunk Night"),
            },
        ),
        (
            "Studio Ghibli Morning",
            ThemeAbi {
                version: 1,
                id: 0,
                flags: FLAG_BUILTIN | FLAG_SIGNED | FLAG_DYNAMIC,
                accent_argb: 0xFF_C8_E0_94,
                bg_argb: 0xFF_FA_F6_E8,
                fg_argb: 0xFF_3D_4A_2F,
                muted_argb: 0xFF_7A_8F_60,
                glass_amount: 30,
                blur_radius: 6,
                corner_radius: 12,
                anim_curve: 2,
                anim_ms: 320,
                scanline_amt: 0,
                particle_kind: 3,
                font_family: label24("Source Han Serif"),
                cursor_style: label24("Soft"),
                name: name_bytes("Studio Ghibli Morning"),
            },
        ),
        (
            "Bauhaus",
            ThemeAbi {
                version: 1,
                id: 0,
                flags: FLAG_BUILTIN | FLAG_SIGNED,
                accent_argb: 0xFF_E5_3A_2C,
                bg_argb: 0xFF_F4_EF_DC,
                fg_argb: 0xFF_14_12_0C,
                muted_argb: 0xFF_8A_82_70,
                glass_amount: 0,
                blur_radius: 0,
                corner_radius: 0,
                anim_curve: 0,
                anim_ms: 100,
                scanline_amt: 0,
                particle_kind: 0,
                font_family: label24("Futura"),
                cursor_style: label24("Block"),
                name: name_bytes("Bauhaus"),
            },
        ),
        (
            "Neo-noir",
            ThemeAbi {
                version: 1,
                id: 0,
                flags: FLAG_BUILTIN | FLAG_SIGNED,
                accent_argb: 0xFF_E8_B5_4B,
                bg_argb: 0xFF_0E_0B_0A,
                fg_argb: 0xFF_D8_D0_C8,
                muted_argb: 0xFF_58_4C_40,
                glass_amount: 40,
                blur_radius: 12,
                corner_radius: 4,
                anim_curve: 1,
                anim_ms: 240,
                scanline_amt: 0,
                particle_kind: 0,
                font_family: label24("EB Garamond"),
                cursor_style: label24("Pointer"),
                name: name_bytes("Neo-noir"),
            },
        ),
        (
            "CRT Scanlines",
            ThemeAbi {
                version: 1,
                id: 0,
                flags: FLAG_BUILTIN | FLAG_SIGNED,
                accent_argb: 0xFF_44_FF_44,
                bg_argb: 0xFF_00_10_05,
                fg_argb: 0xFF_88_FF_88,
                muted_argb: 0xFF_22_88_44,
                glass_amount: 0,
                blur_radius: 0,
                corner_radius: 0,
                anim_curve: 0,
                anim_ms: 60,
                scanline_amt: 80,
                particle_kind: 0,
                font_family: label24("VT323"),
                cursor_style: label24("Underline"),
                name: name_bytes("CRT Scanlines"),
            },
        ),
        (
            "Brutalist",
            ThemeAbi {
                version: 1,
                id: 0,
                flags: FLAG_BUILTIN | FLAG_SIGNED,
                accent_argb: 0xFF_FF_FF_00,
                bg_argb: 0xFF_2A_2A_2A,
                fg_argb: 0xFF_FF_FF_FF,
                muted_argb: 0xFF_88_88_88,
                glass_amount: 0,
                blur_radius: 0,
                corner_radius: 0,
                anim_curve: 0,
                anim_ms: 0,
                scanline_amt: 0,
                particle_kind: 0,
                font_family: label24("Helvetica Bold"),
                cursor_style: label24("Block"),
                name: name_bytes("Brutalist"),
            },
        ),
        (
            "Holographic",
            ThemeAbi {
                version: 1,
                id: 0,
                flags: FLAG_BUILTIN | FLAG_SIGNED | FLAG_DYNAMIC,
                accent_argb: 0xFF_88_FF_FF,
                bg_argb: 0xFF_06_0A_18,
                fg_argb: 0xFF_E8_E8_FF,
                muted_argb: 0xFF_60_80_C0,
                glass_amount: 95,
                blur_radius: 28,
                corner_radius: 16,
                anim_curve: 2,
                anim_ms: 240,
                scanline_amt: 8,
                particle_kind: 1,
                font_family: label24("SF Pro Rounded"),
                cursor_style: label24("Iridescent"),
                name: name_bytes("Holographic"),
            },
        ),
    ];
    for (_, abi) in builtin {
        let mut a = *abi;
        a.id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        if reg.current.is_none() {
            reg.current = Some(a.id);
        }
        reg.themes.insert(a.id, Theme { abi: a });
    }
}

// ── Boot init ──────────────────────────────────────────────────────────

pub fn init() {
    let mut reg = Registry {
        themes: BTreeMap::new(),
        current: None,
        apply_count: 0,
    };
    seed(&mut reg);
    let n = reg.themes.len();
    let cur = reg.current.unwrap_or(0);
    *REGISTRY.lock() = Some(reg);
    crate::serial_println!(
        "[ OK ] Theme engine: {} signed built-in theme(s), current={} (compositor-level rendering)",
        n,
        cur,
    );
}

// ── Public APIs ────────────────────────────────────────────────────────

pub fn apply(theme_id: u64) -> u64 {
    let mut g = REGISTRY.lock();
    let reg = match g.as_mut() {
        Some(r) => r,
        None => return ERR_NOT_INIT,
    };
    if !reg.themes.contains_key(&theme_id) {
        return ERR_NO_SUCH;
    }
    reg.current = Some(theme_id);
    reg.apply_count += 1;
    // Picking a signed theme is an explicit accent choice — it wins over any
    // prior raw-accent override so `active_accent()` reflects this theme.
    ACCENT_OVERRIDE.store(0, Ordering::Release);
    let name_bytes = reg.themes[&theme_id].abi.name;
    let name = core::str::from_utf8(&name_bytes)
        .unwrap_or("(invalid)")
        .trim_end_matches('\0');
    drop(g);
    // Re-skin the userspace shell with the new theme's accent in the SAME call,
    // so a Vibe preset (which goes through apply()) recolours the taskbar/Start/
    // tray/Settings in lock-step with the kernel surfaces.
    athshell::set_active_accent(active_accent());
    crate::serial_println!("[theme] applied #{} \"{}\"", theme_id, name);
    0
}

pub fn current_id() -> Option<u64> {
    REGISTRY.lock().as_ref().and_then(|r| r.current)
}

/// Look a theme up by its display name (Vibe Mode presets bind by name so
/// they survive id reshuffles across boots).
pub fn find_by_name(name: &str) -> Option<u64> {
    let g = REGISTRY.lock();
    let r = g.as_ref()?;
    r.themes
        .iter()
        .find(|(_, t)| {
            core::str::from_utf8(&t.abi.name)
                .map(|n| n.trim_end_matches('\0') == name)
                .unwrap_or(false)
        })
        .map(|(id, _)| *id)
}

pub fn current_abi() -> Option<ThemeAbi> {
    let g = REGISTRY.lock();
    let r = g.as_ref()?;
    let id = r.current?;
    r.themes.get(&id).map(|t| t.abi)
}

/// The LIVE accent seed — the single source of truth every re-skinned surface
/// (window chrome, notification bars, the shell taskbar/Start, the Settings
/// control kit) feeds into `ath_tokens::derive_accent`. This is what makes
/// Vibe Mode's "one tap re-skins the whole desktop" real (Concept §Customization
/// Engine): a single value flows to every surface from one home.
///
/// Resolution order: an explicit raw-accent override (the colour picker / a
/// custom accent), else the current theme's `accent_argb`, else [`RAEBLUE`].
#[must_use]
pub fn active_accent() -> u32 {
    let over = ACCENT_OVERRIDE.load(Ordering::Acquire);
    if over != 0 {
        return over;
    }
    current_abi().map(|a| a.accent_argb).unwrap_or(RAEBLUE)
}

/// Set a raw live accent seed independent of the signed-theme list (the custom
/// accent / colour-picker path, and the cohesion smoketest's distinctive seed).
/// Passing [`RAEBLUE`] is treated as "no override" so the default reverts to the
/// current theme cleanly. After this, [`active_accent`] returns `argb`; the
/// kernel re-propagates it to the userspace shell via
/// `athshell::set_active_accent` (see `shell_runner`).
pub fn set_active_accent(argb: u32) {
    // `0` is our "no override" sentinel; an opaque accent always has a non-zero
    // alpha, so a real accent can never collide with it. Treat RAEBLUE as
    // "clear" so the default path stays the theme's value.
    if argb == RAEBLUE {
        ACCENT_OVERRIDE.store(0, Ordering::Release);
    } else {
        ACCENT_OVERRIDE.store(argb, Ordering::Release);
    }
    // Keep the userspace shell's live seed in lock-step with the kernel surfaces.
    athshell::set_active_accent(active_accent());
}

/// Clear any raw-accent override; [`active_accent`] reverts to the current
/// theme's `accent_argb` (or [`RAEBLUE`]).
pub fn clear_active_accent() {
    ACCENT_OVERRIDE.store(0, Ordering::Release);
}

/// Live theme snapshot for `SYS_THEME_GET` (syscall 266), as the field tuple
/// `(accent_argb, bg_argb, fg_argb, is_dark, blur_radius, palette_id)`. The
/// accent honours the live override (so it equals [`active_accent`]); palette /
/// blur / id come from the current [`ThemeAbi`] (defaults when none is current).
/// `is_dark` is `1` when the background luminance is below mid-grey. This is the
/// single home the kernel hands to separate-process apps so Vibe Mode reaches
/// them — see `kernel/src/syscall.rs` arm 266 and `ath_abi::ThemeInfo`.
#[must_use]
pub fn theme_info() -> (u32, u32, u32, u32, u32, u32) {
    let accent = active_accent();
    match current_abi() {
        Some(a) => {
            let is_dark = if bg_is_dark(a.bg_argb) { 1 } else { 0 };
            (
                accent,
                a.bg_argb,
                a.fg_argb,
                is_dark,
                a.blur_radius,
                a.id as u32,
            )
        }
        // No current theme: mirror the "RaeBlue Default" builtin's chrome so an
        // app gets a coherent (dark) palette even on an unthemed early boot.
        None => (accent, 0xFF_0A_0E_1A, 0xFF_E0_E4_F0, 1, 16, 0),
    }
}

/// True when an ARGB background reads as "dark" (Rec.601 luma below mid-grey).
/// Lets `SYS_THEME_GET` report `is_dark` without an extra ThemeAbi field.
fn bg_is_dark(argb: u32) -> bool {
    let r = ((argb >> 16) & 0xFF) as u32;
    let g = ((argb >> 8) & 0xFF) as u32;
    let b = (argb & 0xFF) as u32;
    // Integer Rec.601: (299*R + 587*G + 114*B) / 1000, dark if < 128.
    (299 * r + 587 * g + 114 * b) / 1000 < 128
}

// ── Error codes ────────────────────────────────────────────────────────

pub const ERR_NOT_INIT: u64 = 0xFFFF_FFFF_FFFF_FC01;
pub const ERR_NO_SUCH: u64 = 0xFFFF_FFFF_FFFF_FC02;
pub const ERR_BAD_USER: u64 = 0xFFFF_FFFF_FFFF_FC03;
pub const ERR_NOT_SIGNED: u64 = 0xFFFF_FFFF_FFFF_FC04;

// ── Syscalls ───────────────────────────────────────────────────────────

pub const SYS_THEME_LIST: u64 = 74;
pub const SYS_THEME_QUERY: u64 = 75;
pub const SYS_THEME_APPLY: u64 = 76;
pub const SYS_THEME_REGISTER: u64 = 77;

/// 32-byte entries: u64 id, u32 flags, [u8;20] name.
pub fn sys_list(out_ptr: u64, out_cap: u64, validate_w: impl Fn(u64, u64, bool) -> bool) -> u64 {
    if out_cap > 0 && !validate_w(out_ptr, out_cap, true) {
        return 0;
    }
    let g = REGISTRY.lock();
    let reg = match g.as_ref() {
        Some(r) => r,
        None => return 0,
    };
    let max = (out_cap / 32) as usize;
    let n = reg.themes.len().min(max);
    // Assemble the 32-byte entries (u64 id, u32 flags, [u8;20] name) kernel-side,
    // then one SMAP-safe copy through the uaccess/extable chokepoint (was
    // per-entry raw writes to the user pointer).
    let mut buf = alloc::vec::Vec::with_capacity(n * 32);
    for (id, t) in reg.themes.iter().take(n) {
        buf.extend_from_slice(&id.to_le_bytes()); // @0
        buf.extend_from_slice(&t.abi.flags.to_le_bytes()); // @8
        buf.extend_from_slice(&t.abi.name[..20]); // @12..32
    }
    if crate::uaccess::copy_to_user(out_ptr, &buf).is_err() {
        return 0;
    }
    n as u64
}

pub fn sys_query(
    theme_id: u64,
    out_ptr: u64,
    out_cap: u64,
    validate_w: impl Fn(u64, u64, bool) -> bool,
) -> u64 {
    let size = core::mem::size_of::<ThemeAbi>() as u64;
    if out_cap < size {
        return u64::MAX;
    }
    if !validate_w(out_ptr, size, true) {
        return u64::MAX;
    }
    let g = REGISTRY.lock();
    let reg = match g.as_ref() {
        Some(r) => r,
        None => return u64::MAX,
    };
    let theme = match reg.themes.get(&theme_id) {
        Some(t) => t.abi,
        None => return u64::MAX,
    };
    // SMAP-safe copy through the uaccess/extable chokepoint (was raw
    // write_unaligned to the user pointer).
    if crate::uaccess::copy_to_user(out_ptr, &theme.to_le_bytes()).is_err() {
        return u64::MAX;
    }
    size
}

pub fn sys_apply(theme_id: u64) -> u64 {
    apply(theme_id)
}

// ── Signed theme-bundle install (Concept §"tweakable everything", signed +
//    sandboxed) ───────────────────────────────────────────────────────────
//
// A theme bundle is a canonical 76-byte payload (theme parameters) followed by a
// 64-byte Ed25519 signature over that payload. The publisher signs offline; the
// kernel holds only the public key (the platform trust anchor, `secure_boot`), so
// installing an UNTRUSTED bundle cannot forge a theme — a tampered or wrong-key
// bundle is refused (`ERR_NOT_SIGNED`). Installed themes are flagged
// `SIGNED | SANDBOXED | DYNAMIC`; they join the registry and are immediately
// applyable like a built-in.

/// Canonical signed payload length (theme parameters, little-endian).
pub const THEME_PAYLOAD_LEN: usize = 76;
/// Full bundle = payload + a 64-byte Ed25519 signature.
pub const THEME_BUNDLE_LEN: usize = THEME_PAYLOAD_LEN + 64;

/// Parse a canonical theme payload into a `ThemeAbi` (id assigned at register).
/// All numeric fields are clamped to safe ranges so a hostile bundle can't push an
/// out-of-range animation/blur/etc. into the compositor.
fn parse_theme_payload(p: &[u8]) -> Option<ThemeAbi> {
    if p.len() != THEME_PAYLOAD_LEN {
        return None;
    }
    let rd = |o: usize| u32::from_le_bytes([p[o], p[o + 1], p[o + 2], p[o + 3]]);
    let mut name = [0u8; 32];
    name.copy_from_slice(&p[0..32]);
    Some(ThemeAbi {
        version: 1,
        id: 0,
        flags: FLAG_SIGNED | FLAG_SANDBOXED | FLAG_DYNAMIC,
        accent_argb: rd(32),
        bg_argb: rd(36),
        fg_argb: rd(40),
        muted_argb: rd(44),
        glass_amount: rd(48).min(100),
        blur_radius: rd(52).min(64),
        corner_radius: rd(56).min(64),
        anim_curve: rd(60).min(3),
        anim_ms: rd(64).min(10_000),
        scanline_amt: rd(68).min(100),
        particle_kind: rd(72).min(3),
        font_family: label24("System"),
        cursor_style: label24("Default"),
        name,
    })
}

/// Add a parsed theme to the registry under a fresh id. Returns the new id, or
/// `ERR_NOT_INIT` if the engine has not booted.
fn register_theme(mut abi: ThemeAbi) -> u64 {
    let mut g = REGISTRY.lock();
    let reg = match g.as_mut() {
        Some(r) => r,
        None => return ERR_NOT_INIT,
    };
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    abi.id = id;
    reg.themes.insert(id, Theme { abi });
    id
}

/// Verify a theme bundle against `publisher_pubkey`, parse it, and register the
/// theme. Returns the new theme id, or an `ERR_*` code (fail-closed: a bad length /
/// bad signature / unparsable payload never registers anything).
pub fn install_theme_bundle(bundle: &[u8], publisher_pubkey: &[u8; 32]) -> u64 {
    if bundle.len() != THEME_BUNDLE_LEN {
        return ERR_BAD_USER;
    }
    let payload = &bundle[..THEME_PAYLOAD_LEN];
    let mut sig = [0u8; 64];
    sig.copy_from_slice(&bundle[THEME_PAYLOAD_LEN..]);
    if !ath_crypto::ed25519::verify(publisher_pubkey, payload, &sig) {
        return ERR_NOT_SIGNED;
    }
    match parse_theme_payload(payload) {
        Some(abi) => register_theme(abi),
        None => ERR_BAD_USER,
    }
}

/// `SYS_THEME_REGISTER`: install a user-supplied signed theme bundle. The bundle is
/// verified against the platform trust anchor (`secure_boot`) — the same offline key
/// that signs the boot manifest — so a theme cannot ship code-or-config the user did
/// not get from a trusted publisher.
pub fn sys_register(
    bundle_ptr: u64,
    bundle_len: u64,
    validate_r: impl Fn(u64, u64, bool) -> bool,
) -> u64 {
    if bundle_len as usize != THEME_BUNDLE_LEN {
        return ERR_BAD_USER;
    }
    if !validate_r(bundle_ptr, bundle_len, false) {
        return ERR_BAD_USER;
    }
    // SMAP-safe read through the uaccess/extable chokepoint (was raw
    // copy_nonoverlapping from the user pointer).
    let theme_data = match crate::uaccess::copy_from_user(bundle_ptr, bundle_len as usize) {
        Ok(b) => b,
        Err(()) => return ERR_BAD_USER,
    };
    install_theme_bundle(&theme_data, &crate::secure_boot::anchor_public_key())
}

/// R10 smoketest: prove the signed-bundle install path end to end, fail-closed.
/// Uses a TEST keypair (so it can sign in-kernel without the production anchor's
/// private key): builds a theme payload, signs it, installs + applies it, then
/// confirms a tampered signature and a wrong publisher key are both refused.
pub fn run_register_smoketest() {
    use ath_crypto::ed25519;
    let test_seed = [0x42u8; 32];
    let test_pub = ed25519::derive_public_key(&test_seed);

    let mut payload = [0u8; THEME_PAYLOAD_LEN];
    let nm = b"Installed Test Theme";
    payload[..nm.len()].copy_from_slice(nm);
    payload[32..36].copy_from_slice(&0xFF00_FF00u32.to_le_bytes()); // accent
    let sig = ed25519::sign(&test_seed, &payload);
    let mut bundle = [0u8; THEME_BUNDLE_LEN];
    bundle[..THEME_PAYLOAD_LEN].copy_from_slice(&payload);
    bundle[THEME_PAYLOAD_LEN..].copy_from_slice(&sig);

    // Remember the ACTIVE theme so the test theme never leaks into the live
    // session (this smoketest previously left its 0xFF00FF00 pure-green test
    // accent APPLIED — the whole desktop booted green; QMP screenshot
    // 2026-07-01. A smoketest must restore every piece of state it touches).
    let prior_theme: Option<u64> = {
        let g = REGISTRY.lock();
        g.as_ref().and_then(|r| r.current)
    };

    let id = install_theme_bundle(&bundle, &test_pub);
    let installed = id != 0 && id < ERR_NOT_INIT; // a real id, not an ERR_* code
    let apply_ok = installed && apply(id) == 0; // registered + applyable
    let accent_ok = apply_ok && active_accent() == 0xFF00_FF00; // theme data took effect

    // Tampered signature is refused.
    let mut tampered = bundle;
    tampered[THEME_PAYLOAD_LEN] ^= 0x01;
    let tamper_rejected = install_theme_bundle(&tampered, &test_pub) == ERR_NOT_SIGNED;
    // A different publisher key is refused (anti-forgery).
    let other_pub = ed25519::derive_public_key(&[0x99u8; 32]);
    let wrongkey_rejected = install_theme_bundle(&bundle, &other_pub) == ERR_NOT_SIGNED;

    // Restore the pre-test theme and UNREGISTER the test bundle — the install
    // proof is the serial line, not a stray "Installed Test Theme" entry in the
    // user's Settings theme list.
    let restored = {
        let restore_ok = match prior_theme {
            Some(prev) => apply(prev) == 0,
            None => true,
        };
        {
            let mut g = REGISTRY.lock();
            if let Some(reg) = g.as_mut() {
                if installed {
                    reg.themes.remove(&id);
                }
                if prior_theme.is_none() {
                    // No theme was applied pre-test (accent came from the RAEBLUE
                    // default) — clear `current` so active_accent() falls back.
                    reg.current = None;
                }
            }
            // The guard MUST drop here: active_accent() -> current_abi() takes
            // REGISTRY.lock() itself. Calling it with `g` still live was a spin
            // self-deadlock with IRQs off on CPU0 — it hard-wedged EVERY 2026-07-01
            // iron boot containing this smoketest (fabric-dead, power-button-only;
            // KVM repro: 8/8 frozen at this exact line, all APs in hlt, CPU0
            // spinning in run_deferred with IF=0).
        }
        restore_ok && active_accent() != 0xFF00_FF00
    };

    let pass =
        installed && apply_ok && accent_ok && tamper_rejected && wrongkey_rejected && restored;
    crate::serial_println!(
        "[theme-install] smoketest: installed={} apply={} accent_ok={} tamper_rejected={} wrongkey_rejected={} restored={} -> {}",
        installed,
        apply_ok,
        accent_ok,
        tamper_rejected,
        wrongkey_rejected,
        restored,
        if pass { "PASS" } else { "FAIL" },
    );
}

// ── /proc/athena/themes ─────────────────────────────────────────────────

pub fn dump_text() -> String {
    let g = REGISTRY.lock();
    let reg = match g.as_ref() {
        Some(r) => r,
        None => return String::from("# theme engine not initialized\n"),
    };
    let mut out = String::new();
    out.push_str(&alloc::format!(
        "# AthenaOS theme engine ({} themes, {} applies since boot)\n",
        reg.themes.len(),
        reg.apply_count,
    ));
    if let Some(id) = reg.current {
        out.push_str(&alloc::format!("# current = {}\n", id));
    }
    for (id, t) in &reg.themes {
        let name = core::str::from_utf8(&t.abi.name)
            .unwrap_or("?")
            .trim_end_matches('\0');
        let font = core::str::from_utf8(&t.abi.font_family)
            .unwrap_or("?")
            .trim_end_matches('\0');
        out.push_str(&alloc::format!(
            "\n#{:<3} flags=0x{:x}  accent=#{:06x}  glass={}  blur={}  scanline={}  font=\"{}\"  name=\"{}\"\n",
            id, t.abi.flags, t.abi.accent_argb & 0x00FF_FFFF,
            t.abi.glass_amount, t.abi.blur_radius, t.abi.scanline_amt,
            font, name,
        ));
    }
    out
}

// ── Boot smoketest ─────────────────────────────────────────────────────

pub fn run_boot_smoketest() {
    // Apply each preset once so the log proves the pipeline works.
    let ids: Vec<u64> = {
        let g = REGISTRY.lock();
        g.as_ref()
            .map(|r| r.themes.keys().copied().collect())
            .unwrap_or_default()
    };
    for id in &ids {
        let _ = apply(*id);
    }
    // Restore to the first theme so we're back on RaeBlue. Compute the
    // first id with the lock scoped tight so apply() can re-lock cleanly.
    let first_id: Option<u64> = ids.first().copied();
    if let Some(id) = first_id {
        let _ = apply(id);
    }
}

/// The dynamic-cohesion proof (design-language §6, made FAIL-able with no
/// screenshot): Concept §Customization Engine — "Vibe Mode presets … the
/// desktop becomes a different place in one tap". This proves that single tap
/// is real: ONE live seed flows to EVERY re-skinned surface.
///
/// It reads the default accent (all surfaces agree), sets a distinctive
/// non-default accent through the live `set_active_accent` path (also pushed to
/// the userspace shell via `athshell::set_active_accent`), re-reads every
/// surface's proof accent, asserts they ALL now derive from the new seed, then
/// restores RaeBlue and asserts they all revert. Prints PASS only if every
/// surface tracked the change in lock-step.
///
/// Run from `kernel_main` AFTER `theme_engine`, `vibe_mode`, the window-chrome
/// + notify surfaces, and `athshell` are initialized.
pub fn run_accent_cohesion_smoketest() {
    // A distinctive seed that is NOT any of the 8 signed builtins, so a stale
    // hardcoded surface can't accidentally match it.
    const ORANGE: u32 = 0xFF_FF_88_00;
    let palette = &ath_tokens::DARK;
    let want_orange = ath_tokens::derive_accent(ORANGE, palette).base;

    // Snapshot every surface's proof accent for the current live seed.
    let sample = || -> (u32, u32, u32, u32) {
        (
            crate::window_chrome::proof_accent(),
            crate::notify::proof_accent(),
            athshell::shell_design_proof().accent_base,
            athshell::control_panel::settings_design_proof().accent_base,
        )
    };

    // set_active_accent updates the kernel override AND pushes to the userspace
    // shell's live seed in one call — exactly the runtime Vibe-Mode path.
    let push = set_active_accent;

    // Establish a known default (RaeBlue) regardless of which theme is current,
    // and remember it to restore at the end.
    push(RAEBLUE);
    let want_blue = active_accent(); // the live default seed, normally RaeBlue
    let want_blue_base = ath_tokens::derive_accent(want_blue, palette).base;

    // ── Default: all four surfaces agree on the default-derived base. ──
    let (c0, n0, t0, s0) = sample();
    let default_ok = c0 == want_blue_base
        && n0 == want_blue_base
        && t0 == want_blue_base
        && s0 == want_blue_base;
    let default_accent = c0;

    // ── Set the distinctive seed: every surface must re-skin to it. ──
    push(ORANGE);
    let (c1, n1, t1, s1) = sample();
    let set_ok = c1 == want_orange && n1 == want_orange && t1 == want_orange && s1 == want_orange;

    // ── Restore the default: every surface must revert. ──
    push(want_blue);
    let (c2, n2, t2, s2) = sample();
    let restore_ok = c2 == want_blue_base
        && n2 == want_blue_base
        && t2 == want_blue_base
        && s2 == want_blue_base;

    let pass = default_ok && set_ok && restore_ok;
    crate::serial_println!(
        "[theme] accent-cohesion: default={:#010X} -> set {:#010X} -> all{{chrome={:#010X},notify={:#010X},taskbar={:#010X},settings={:#010X}}}={:#010X} -> restored={:#010X} -> {}",
        default_accent,
        ORANGE,
        c1,
        n1,
        t1,
        s1,
        want_orange,
        c2,
        if pass { "PASS" } else { "FAIL" },
    );
}

/// FAIL-able boot smoketest for `SYS_THEME_GET` (syscall 266): the value the
/// kernel hands separate-process apps must equal the live `active_accent()` —
/// both at the default seed and after a Vibe-Mode change — so a re-skinned app
/// matches the in-kernel surfaces. Exercises the syscall's value-producing path
/// (`theme_info()`, exactly what the arm `copy_to_user`s out); the user-pointer
/// copy itself is covered by the live app render. Restores the seed at the end.
pub fn run_theme_get_smoketest() {
    const ORANGE: u32 = 0xFF_FF_88_00;

    // ── Default seed: ThemeInfo.accent must equal the live active_accent(). ──
    set_active_accent(RAEBLUE);
    let want_default = active_accent();
    let (acc0, bg0, fg0, dark0, _blur0, _id0) = theme_info();
    // accent agreement + a coherent palette (non-zero bg/fg, dark default).
    let default_ok = acc0 == want_default && bg0 != 0 && fg0 != 0 && dark0 == 1;

    // ── Vibe-Mode change: ThemeInfo.accent tracks the new live seed. ──
    set_active_accent(ORANGE);
    let want_new = active_accent(); // ORANGE (opaque, != RAEBLUE so not cleared)
    let (acc1, _, _, _, _, _) = theme_info();
    let set_ok = acc1 == want_new && acc1 == ORANGE;

    // ── Restore: ThemeInfo.accent reverts to the default. ──
    set_active_accent(RAEBLUE);
    let (acc2, _, _, _, _, _) = theme_info();
    let restore_ok = acc2 == want_default;

    let pass = default_ok && set_ok && restore_ok;
    crate::serial_println!(
        "[theme] theme-get smoketest: accent={:#010X} matches active_accent={:#010X} -> set {:#010X}={:#010X} -> restored={:#010X} -> {}",
        acc0,
        want_default,
        ORANGE,
        acc1,
        acc2,
        if pass { "PASS" } else { "FAIL" },
    );
}
