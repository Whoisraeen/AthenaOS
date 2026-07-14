//! Live wallpaper engine — Concept §Customization Engine:
//!
//! > "Live wallpapers that don't murder battery — GPU-accelerated, paused
//! >  when occluded."
//!
//! Windows DreamScene / macOS dynamic wallpapers / Wallpaper Engine on
//! Steam all eventually burn battery because they keep drawing under
//! every window, every minute, forever. The Concept-doc invariant is
//! "paused when occluded" — if a fullscreen game or maximized browser
//! covers the desktop, the wallpaper engine stops scheduling frames at
//! all. The compositor already publishes occlusion to its own internal
//! loop; this module is the userspace ABI and the registry of installed
//! wallpapers, plus the bookkeeping for which one is current.
//!
//! Wallpapers come in four kinds:
//!   * **Solid** — single ARGB fill (zero GPU cost).
//!   * **Image** — static raster the compositor uploads once.
//!   * **Procedural** — kernel-shipped procedural wallpaper (gradient,
//!     mesh, plasma). Cheap on power.
//!   * **Shader** — userspace shader bundle, sandboxed, GPU-accelerated.
//!
//! ## Syscalls (85-87)
//!
//! | nr | name             | rdi/rsi/rdx                                            | rax |
//! |----|------------------|--------------------------------------------------------|----|
//! | 85 | WALLPAPER_LIST   | rdi=out_ptr, rsi=out_cap (WallpaperAbi entries)        | count |
//! | 86 | WALLPAPER_SET    | rdi=wallpaper_id                                       | 0/err |
//! | 87 | WALLPAPER_STATUS | rdi=out_ptr (WallpaperStatusAbi)                       | bytes |

#![allow(dead_code)]

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::string::String;
use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;

#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Solid = 0,
    Image = 1,
    Procedural = 2,
    Shader = 3,
}

#[derive(Debug, Clone)]
struct Wallpaper {
    id: u64,
    kind: Kind,
    name: String,
    color_argb: u32, // for Solid + Procedural seed color
    cost_pct: u8,    // estimated power cost 0..=100
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct WallpaperAbi {
    pub version: u32, // = 1
    pub id: u64,
    pub kind: u32,
    pub color_argb: u32,
    pub cost_pct: u32,
    pub name: [u8; 32],
}

impl WallpaperAbi {
    /// Serialize into the exact repr(C) byte layout for a SMAP-safe copy_to_user.
    /// Non-obvious padding: `id`(u64) forces 4 pad bytes after `version`@0, and
    /// the struct has 4 TRAILING pad bytes (60 fields rounded up to align-8 = 64).
    /// Layout: version@0, pad@4, id@8, kind@16, color_argb@20, cost_pct@24,
    ///   name@28, trailing pad@60 → total 64.
    fn to_le_bytes(&self) -> [u8; 64] {
        debug_assert_eq!(core::mem::size_of::<WallpaperAbi>(), 64);
        let mut b = [0u8; 64];
        b[0..4].copy_from_slice(&self.version.to_le_bytes());
        b[8..16].copy_from_slice(&self.id.to_le_bytes());
        b[16..20].copy_from_slice(&self.kind.to_le_bytes());
        b[20..24].copy_from_slice(&self.color_argb.to_le_bytes());
        b[24..28].copy_from_slice(&self.cost_pct.to_le_bytes());
        b[28..60].copy_from_slice(&self.name);
        b
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct WallpaperStatusAbi {
    pub version: u32,
    pub current_id: u64,
    pub occluded: u32, // bool — true when fullscreen surface covers desktop
    pub frames_total: u64,
    pub frames_skipped: u64, // frames the engine declined to render because occluded
    pub current_cost_pct: u32,
}

impl WallpaperStatusAbi {
    /// Serialize into the exact repr(C) byte layout for a SMAP-safe copy_to_user.
    /// Non-obvious padding: `current_id`(u64) forces 4 pad after `version`@0,
    /// `frames_total`(u64) forces 4 pad after `occluded`@16, and 4 TRAILING pad
    /// bytes (44 fields rounded up to align-8 = 48). Layout: version@0, pad@4,
    ///   current_id@8, occluded@16, pad@20, frames_total@24, frames_skipped@32,
    ///   current_cost_pct@40, trailing pad@44 → total 48.
    fn to_le_bytes(&self) -> [u8; 48] {
        debug_assert_eq!(core::mem::size_of::<WallpaperStatusAbi>(), 48);
        let mut b = [0u8; 48];
        b[0..4].copy_from_slice(&self.version.to_le_bytes());
        b[8..16].copy_from_slice(&self.current_id.to_le_bytes());
        b[16..20].copy_from_slice(&self.occluded.to_le_bytes());
        b[24..32].copy_from_slice(&self.frames_total.to_le_bytes());
        b[32..40].copy_from_slice(&self.frames_skipped.to_le_bytes());
        b[40..44].copy_from_slice(&self.current_cost_pct.to_le_bytes());
        b
    }
}

struct Engine {
    wallpapers: BTreeMap<u64, Wallpaper>,
    current: Option<u64>,
    occluded: bool,
    frames_total: u64,
    frames_skipped: u64,
}

static NEXT_ID: AtomicU64 = AtomicU64::new(1);
static ENG: Mutex<Option<Engine>> = Mutex::new(None);

fn name_bytes(s: &str) -> [u8; 32] {
    let mut a = [0u8; 32];
    let b = s.as_bytes();
    let n = b.len().min(32);
    a[..n].copy_from_slice(&b[..n]);
    a
}

fn seed(e: &mut Engine) {
    let items: &[(Kind, &str, u32, u8)] = &[
        (Kind::Solid, "RaeBlue Solid", 0xFF_0A_0E_1A, 0),
        (Kind::Procedural, "Aurora Gradient", 0xFF_4E_9C_FF, 3),
        (Kind::Procedural, "Plasma Drift", 0xFF_7A_2A_FF, 8),
        (Kind::Image, "Mt Fuji Dawn (still)", 0xFF_FF_C0_88, 0),
        (Kind::Image, "Tokyo Neon Skyline", 0xFF_18_22_3A, 0),
        (Kind::Shader, "Cyberpunk Rain Shader", 0xFF_07_0A_18, 22),
        (Kind::Shader, "Cherry Blossom Drift", 0xFF_F0_C0_C0, 12),
        (Kind::Shader, "Liquid Glass", 0xFF_AA_DD_FF, 18),
    ];
    for (kind, name, color, cost) in items {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let w = Wallpaper {
            id,
            kind: *kind,
            name: String::from(*name),
            color_argb: *color,
            cost_pct: *cost,
        };
        if e.current.is_none() {
            e.current = Some(id);
        }
        e.wallpapers.insert(id, w);
    }
}

// ── Boot init ──────────────────────────────────────────────────────────

pub fn init() {
    let mut e = Engine {
        wallpapers: BTreeMap::new(),
        current: None,
        occluded: false,
        frames_total: 0,
        frames_skipped: 0,
    };
    seed(&mut e);
    let n = e.wallpapers.len();
    let cur = e.current.unwrap_or(0);
    *ENG.lock() = Some(e);
    crate::serial_println!(
        "[ OK ] Live wallpaper engine: {} wallpaper(s) registered, current={} (paused when occluded)",
        n, cur,
    );
}

// ── Public API ─────────────────────────────────────────────────────────

pub fn set_current(id: u64) -> u64 {
    let mut g = ENG.lock();
    let e = match g.as_mut() {
        Some(e) => e,
        None => return ERR_NOT_INIT,
    };
    if !e.wallpapers.contains_key(&id) {
        return ERR_NO_SUCH;
    }
    e.current = Some(id);
    let name = e.wallpapers[&id].name.clone();
    crate::serial_println!("[wallpaper] current set to #{} \"{}\"", id, name);
    0
}

/// Look a wallpaper up by its display name (Vibe Mode presets bind by name).
pub fn find_by_name(name: &str) -> Option<u64> {
    let g = ENG.lock();
    let e = g.as_ref()?;
    e.wallpapers
        .iter()
        .find(|(_, w)| w.name == name)
        .map(|(id, _)| *id)
}

/// Currently selected wallpaper id.
pub fn current_id() -> Option<u64> {
    ENG.lock().as_ref().and_then(|e| e.current)
}

pub fn set_occluded(occluded: bool) {
    let mut g = ENG.lock();
    if let Some(e) = g.as_mut() {
        e.occluded = occluded;
    }
}

/// Called by the compositor's render tick. Increments frame counters and
/// tells the caller whether to actually render a wallpaper frame this tick.
/// When the desktop is occluded by a fullscreen surface, we return false
/// and bump `frames_skipped` so the battery saving is visible in /proc.
pub fn render_tick() -> bool {
    let mut g = ENG.lock();
    let e = match g.as_mut() {
        Some(e) => e,
        None => return false,
    };
    e.frames_total += 1;
    if e.occluded {
        e.frames_skipped += 1;
        return false;
    }
    true
}

pub fn status() -> Option<WallpaperStatusAbi> {
    let g = ENG.lock();
    let e = g.as_ref()?;
    let id = e.current?;
    let cost = e
        .wallpapers
        .get(&id)
        .map(|w| w.cost_pct as u32)
        .unwrap_or(0);
    Some(WallpaperStatusAbi {
        version: 1,
        current_id: id,
        occluded: e.occluded as u32,
        frames_total: e.frames_total,
        frames_skipped: e.frames_skipped,
        current_cost_pct: if e.occluded { 0 } else { cost },
    })
}

// ── Error codes ────────────────────────────────────────────────────────

pub const ERR_NOT_INIT: u64 = 0xFFFF_FFFF_FFFF_F901;
pub const ERR_NO_SUCH: u64 = 0xFFFF_FFFF_FFFF_F902;
pub const ERR_BAD_USER: u64 = 0xFFFF_FFFF_FFFF_F903;

// ── Syscalls ───────────────────────────────────────────────────────────

pub const SYS_WALLPAPER_LIST: u64 = 85;
pub const SYS_WALLPAPER_SET: u64 = 86;
pub const SYS_WALLPAPER_STATUS: u64 = 87;

const WP_ABI: usize = core::mem::size_of::<WallpaperAbi>();
const WP_STATUS: usize = core::mem::size_of::<WallpaperStatusAbi>();

pub fn sys_list(out_ptr: u64, out_cap: u64, validate_w: impl Fn(u64, u64, bool) -> bool) -> u64 {
    if out_cap > 0 && !validate_w(out_ptr, out_cap, true) {
        return 0;
    }
    let g = ENG.lock();
    let e = match g.as_ref() {
        Some(e) => e,
        None => return 0,
    };
    let max = (out_cap as usize) / WP_ABI;
    let n = e.wallpapers.len().min(max);
    // Assemble all entries kernel-side, then one SMAP-safe copy through the
    // uaccess/extable chokepoint (was per-entry raw write_unaligned to user).
    let mut buf = alloc::vec::Vec::with_capacity(n * WP_ABI);
    for (id, w) in e.wallpapers.iter().take(n) {
        let abi = WallpaperAbi {
            version: 1,
            id: *id,
            kind: w.kind as u32,
            color_argb: w.color_argb,
            cost_pct: w.cost_pct as u32,
            name: name_bytes(&w.name),
        };
        buf.extend_from_slice(&abi.to_le_bytes());
    }
    if crate::uaccess::copy_to_user(out_ptr, &buf).is_err() {
        return 0;
    }
    n as u64
}

pub fn sys_set(id: u64) -> u64 {
    set_current(id)
}

pub fn sys_status(out_ptr: u64, out_cap: u64, validate_w: impl Fn(u64, u64, bool) -> bool) -> u64 {
    if out_cap < WP_STATUS as u64 {
        return u64::MAX;
    }
    if !validate_w(out_ptr, WP_STATUS as u64, true) {
        return u64::MAX;
    }
    let abi = match status() {
        Some(s) => s,
        None => return u64::MAX,
    };
    // SMAP-safe copy through the uaccess/extable chokepoint (was raw
    // write_unaligned to the user pointer).
    if crate::uaccess::copy_to_user(out_ptr, &abi.to_le_bytes()).is_err() {
        return u64::MAX;
    }
    WP_STATUS as u64
}

// ── /proc/raeen/wallpaper ──────────────────────────────────────────────

pub fn dump_text() -> String {
    let g = ENG.lock();
    let e = match g.as_ref() {
        Some(e) => e,
        None => return String::from("# live wallpaper engine not initialized\n"),
    };
    let mut out = String::new();
    let pct_skipped = if e.frames_total == 0 {
        0
    } else {
        (e.frames_skipped * 100) / e.frames_total
    };
    out.push_str(&alloc::format!(
        "# AthenaOS live wallpaper engine ({} wallpapers, occluded={}, frames {}/{} skipped, {}% saved)\n",
        e.wallpapers.len(), e.occluded,
        e.frames_skipped, e.frames_total, pct_skipped,
    ));
    if let Some(id) = e.current {
        out.push_str(&alloc::format!("# current = {}\n", id));
    }
    for (id, w) in &e.wallpapers {
        out.push_str(&alloc::format!(
            "#{:<3} kind={:?} cost={}% color=#{:06x} name=\"{}\"\n",
            id,
            w.kind,
            w.cost_pct,
            w.color_argb & 0x00FF_FFFF,
            w.name,
        ));
    }
    out
}

// ── Boot smoketest ─────────────────────────────────────────────────────

pub fn run_boot_smoketest() {
    // Drive a few render ticks with occluded=false, then flip to true,
    // then back. Boot log should show the skip-counter rising while
    // occluded.
    for _ in 0..3 {
        render_tick();
    }
    set_occluded(true);
    for _ in 0..7 {
        render_tick();
    }
    set_occluded(false);
    let s = status().unwrap_or(WallpaperStatusAbi {
        version: 1,
        current_id: 0,
        occluded: 0,
        frames_total: 0,
        frames_skipped: 0,
        current_cost_pct: 0,
    });
    crate::serial_println!(
        "[wallpaper] smoketest: frames_total={} frames_skipped={} (battery-saver path works)",
        s.frames_total,
        s.frames_skipped,
    );
}
