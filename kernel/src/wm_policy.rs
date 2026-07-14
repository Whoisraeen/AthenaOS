//! Swappable window-manager policy (Concept §RaeUI: "your desktop, your
//! rules — tiling, stacking, floating are POLICIES over the compositor, not
//! forks of it; switching is one call, not a different OS").
//! MasterChecklist Phase 13.2 — "Swappable window manager API (tile, stack,
//! float, hybrid)".
//!
//! The compositor owns surfaces; this module owns PLACEMENT. A
//! [`WmMode`] is a pure function from (screen, window list) to origins —
//! [`compute_layout`] — and [`apply`] feeds the live userspace window list
//! through it via `compositor::set_surface_origin`. Because the policy is
//! pure and the mechanism is the existing compositor API, a third-party WM
//! is just another `WmMode` implementation (the "swappable API" of the
//! checklist item).
//!
//! Modes today: Float (windows stay where the user put them), Tile (grid,
//! row-major, near-square), Stack (cascade). Tile is TRUE tiling: it does not
//! just place a window at a cell origin, it RESIZES the client to FILL the cell
//! (i3/sway tiling; Win11 Snap Layouts). A kernel-owned surface reflows
//! immediately (the compositor owns its buffer); a userspace client honors the
//! resize via the `SYS_SURFACE_RESIZE_REQ` (291) / `SYS_SURFACE_RESIZE` (292)
//! handshake — `compositor::request_surface_resize` records the cell size and
//! the client reallocates its framebuffer to match. MasterChecklist Phase 13.2.

#![allow(dead_code)]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, AtomicU8, Ordering};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum WmMode {
    Float = 0,
    Tile = 1,
    Stack = 2,
}

impl WmMode {
    pub fn name(self) -> &'static str {
        match self {
            WmMode::Float => "float",
            WmMode::Tile => "tile",
            WmMode::Stack => "stack",
        }
    }
    fn from_u8(v: u8) -> Self {
        match v {
            1 => WmMode::Tile,
            2 => WmMode::Stack,
            _ => WmMode::Float,
        }
    }
}

static MODE: AtomicU8 = AtomicU8::new(WmMode::Float as u8);
static APPLIES: AtomicU64 = AtomicU64::new(0);

pub fn current_mode() -> WmMode {
    WmMode::from_u8(MODE.load(Ordering::Relaxed))
}

pub fn set_mode(mode: WmMode) {
    MODE.store(mode as u8, Ordering::Relaxed);
    crate::serial_println!("[wm] mode -> {}", mode.name());
    apply();
}

/// The tiling grid for `n` windows on a `screen_w × screen_h` screen:
/// `(cols, rows, cell_w, cell_h)`. Near-square, row-major: `cols = ceil(sqrt(n))`.
/// The single source of truth for BOTH the cell origins (`compute_layout`) and
/// the cell SIZES (`apply_to`'s resize requests), so a window's position and its
/// reflowed size always agree. `n == 0` yields a 1×1 full-screen cell (callers
/// guard `n == 0` before using it).
pub fn tile_grid(screen_w: u32, screen_h: u32, n: u32) -> (u32, u32, u32, u32) {
    let n = n.max(1);
    let mut cols = 1u32;
    while cols * cols < n {
        cols += 1;
    }
    let rows = (n + cols - 1) / cols;
    let cell_w = screen_w / cols.max(1);
    let cell_h = screen_h / rows.max(1);
    (cols, rows, cell_w, cell_h)
}

/// Pure placement policy: given the screen and the windows (id, w, h) in
/// z-order (bottom first), return each window's new origin. No compositor
/// access — this is the swappable part, and what the smoketest pins down.
pub fn compute_layout(
    mode: WmMode,
    screen_w: u32,
    screen_h: u32,
    windows: &[(u64, u32, u32)],
) -> Vec<(u64, i32, i32)> {
    match mode {
        WmMode::Float => Vec::new(), // float never moves anything
        WmMode::Tile => {
            let n = windows.len() as u32;
            if n == 0 {
                return Vec::new();
            }
            let (cols, _rows, cell_w, cell_h) = tile_grid(screen_w, screen_h, n);
            windows
                .iter()
                .enumerate()
                .map(|(i, &(id, _w, _h))| {
                    let col = (i as u32) % cols;
                    let row = (i as u32) / cols;
                    ((id), (col * cell_w) as i32, (row * cell_h) as i32)
                })
                .collect()
        }
        WmMode::Stack => windows
            .iter()
            .enumerate()
            .map(|(i, &(id, _w, _h))| (id, 32 * i as i32 + 16, 28 * i as i32 + 16))
            .collect(),
    }
}

/// Apply the current mode to an explicit surface set (the smoketest path,
/// and what a future per-workspace policy calls). Returns how many windows
/// were repositioned.
pub fn apply_to(ids: &[u64]) -> usize {
    let (sw, sh) = match crate::compositor::screen_dimensions() {
        Some(d) => d,
        None => return 0,
    };
    let windows: Vec<(u64, u32, u32)> = ids
        .iter()
        .filter_map(|&id| {
            crate::compositor::surface_frame(id)
                .filter(|f| !f.4) // skip minimized
                .map(|f| (id, f.2 as u32, f.3 as u32))
        })
        .collect();
    let mode = current_mode();
    let placements = compute_layout(mode, sw, sh, &windows);
    let mut moved = 0;
    for (id, x, y) in placements {
        if crate::compositor::set_surface_origin(id, x, y).is_ok() {
            moved += 1;
        }
    }
    // True tiling: don't just move windows into cells — RESIZE them to FILL the
    // cell (i3/sway tiling; Win11 Snap Layouts). For a kernel-owned surface the
    // compositor owns the buffer and reflows it immediately; for a user surface
    // it records a request the client honors via SYS_SURFACE_RESIZE_REQ (291) /
    // SYS_SURFACE_RESIZE (292). Float/Stack leave window sizes alone.
    if mode == WmMode::Tile && !windows.is_empty() {
        let (_cols, _rows, cell_w, cell_h) = tile_grid(sw, sh, windows.len() as u32);
        if cell_w > 0 && cell_h > 0 {
            for &(id, _w, _h) in &windows {
                let _ = crate::compositor::request_surface_resize(id, cell_w, cell_h);
            }
        }
    }
    if moved > 0 {
        APPLIES.fetch_add(1, Ordering::Relaxed);
    }
    moved
}

/// Apply the current mode to all live userspace windows (app surfaces).
pub fn apply() -> usize {
    let ids: Vec<u64> = crate::compositor::list_userspace_surfaces()
        .into_iter()
        .map(|(id, _z)| id)
        .collect();
    apply_to(&ids)
}

pub fn init() {
    crate::serial_println!(
        "[wm] window-manager policy ready (float/tile/stack swappable, mode={})",
        current_mode().name(),
    );
}

/// Deterministic proof: (1) the pure tile policy puts 3 windows on a
/// 2-column grid and stack cascades them; (2) the LIVE apply path really
/// moves compositor surfaces AND reflows them to fill their cell (true tiling
/// resize — 3 kernel test surfaces created at 64x48, tiled, then verified via
/// surface_frame to sit at their cell origin AND match their cell size, then
/// closed).
pub fn run_boot_smoketest() {
    // (1) Pure policy.
    let wins = [(10u64, 400u32, 300u32), (11, 400, 300), (12, 400, 300)];
    let tiled = compute_layout(WmMode::Tile, 1024, 768, &wins);
    // 3 windows -> cols=2, rows=2, cells 512x384; row-major origins.
    let tile_ok = tiled == alloc::vec![(10u64, 0i32, 0i32), (11, 512, 0), (12, 0, 384)];
    let stacked = compute_layout(WmMode::Stack, 1024, 768, &wins);
    let stack_ok = stacked == alloc::vec![(10u64, 16i32, 16i32), (11, 48, 44), (12, 80, 72)];
    let float_ok = compute_layout(WmMode::Float, 1024, 768, &wins).is_empty();

    // (2) Live apply over real compositor surfaces.
    let mut ids: Vec<u64> = Vec::new();
    for _ in 0..3 {
        if let Some((id, _ptr)) = crate::compositor::create_kernel_surface(64, 48) {
            let _ = crate::compositor::present_surface(id, 500, 500);
            ids.push(id);
        }
    }
    // The cell the WM will compute for these 3 surfaces, so we can assert the
    // windows REFLOWED to fill it (not merely got positioned at its origin).
    let (live_ok, resize_ok) = if ids.len() == 3 {
        let (sw, sh) = crate::compositor::screen_dimensions().unwrap_or((0, 0));
        let (_c, _r, cell_w, cell_h) = tile_grid(sw, sh, 3);
        let prev = current_mode();
        MODE.store(WmMode::Tile as u8, Ordering::Relaxed);
        let moved = apply_to(&ids);
        let f0 = crate::compositor::surface_frame(ids[0]);
        let first = f0.map(|f| (f.0, f.1));
        let second = crate::compositor::surface_frame(ids[1]).map(|f| (f.0, f.1));
        // True-tiling resize: kernel-owned surfaces reflow immediately, so every
        // tiled window's width/height must now equal the computed cell.
        let resize_ok = cell_w > 0
            && cell_h > 0
            && ids.iter().all(|&id| {
                crate::compositor::surface_frame(id)
                    .map(|f| f.2 as u32 == cell_w && f.3 as u32 == cell_h)
                    .unwrap_or(false)
            });
        MODE.store(prev as u8, Ordering::Relaxed);
        for id in &ids {
            let _ = crate::compositor::close_surface(*id);
        }
        let live_ok =
            moved == 3 && first == Some((0, 0)) && second.map(|(x, _)| x > 0).unwrap_or(false);
        (live_ok, resize_ok)
    } else {
        for id in &ids {
            let _ = crate::compositor::close_surface(*id);
        }
        (false, false)
    };

    let pass = tile_ok && stack_ok && float_ok && live_ok && resize_ok;
    crate::serial_println!(
        "[wm] smoketest: tile_grid={} stack_cascade={} float_noop={} live_apply={} tile_resize={} -> {}",
        tile_ok,
        stack_ok,
        float_ok,
        live_ok,
        resize_ok,
        if pass { "PASS" } else { "FAIL" },
    );
}

/// `/proc/raeen/wm` — window-manager policy state.
pub fn dump_text() -> String {
    alloc::format!(
        "# window-manager policy (swappable: float/tile/stack)\nmode: {}\napplies: {}\nmanaged_windows: {}\n",
        current_mode().name(),
        APPLIES.load(Ordering::Relaxed),
        crate::compositor::list_userspace_surfaces().len(),
    )
}
