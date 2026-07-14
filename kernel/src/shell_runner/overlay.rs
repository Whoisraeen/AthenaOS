//! `shell_runner` overlay rendering — the Alt-Tab switcher + the Overview
//! chrome and their shared ARGB draw primitives, carved out of the
//! `shell_runner` god-file (behaviour-identical move). The parent calls
//! `overlay::{render_overview_chrome, cycle_alt_tab}`; everything else here is
//! private to the overlay-rendering concern.
//!
//! `use super::*` inherits the parent module namespace (ShellRunnerState,
//! SHELL_STATE, the compositor/raeshell imports, and the sibling helpers
//! like `overview_grid_ids`) so the move needed no import churn.

#![allow(clippy::too_many_arguments)]
use super::*;

/// Render the overview chrome ON TOP of the compositor's thumbnails: a top
/// spaces strip + each cell's app title + the selected cell's accent ring.
/// Cell geometry mirrors `wm_policy::compute_layout(Tile,…)` so labels land on
/// the same grid the compositor composites the thumbnails into.
pub(super) fn render_overview_chrome(state: &mut ShellRunnerState) {
    let ids = overview_grid_ids(state);
    let w = state.width as i32;
    let h = state.height as i32;
    let stride = state.width as usize;
    let pixels = state.width as usize * state.height as usize;
    let buf = unsafe { core::slice::from_raw_parts_mut(state.surface_ptr as *mut u32, pixels) };

    let seed = raeshell::active_accent();
    let ring = raeshell::spaces::selection_ring(seed);
    let glow = raeshell::spaces::selection_glow(seed);

    // ── Spaces strip (top): one chip per space; current = accent ring ───────
    let chip_w = 96i32;
    let chip_gap = 8i32; // space.2
    let count = state.spaces.count() as i32;
    let strip_total = count * chip_w + (count - 1) * chip_gap;
    let mut chip_x = (w - strip_total) / 2;
    let chip_y = 8i32;
    let chip_h = SPACES_STRIP_H - 12;
    for i in 0..state.spaces.count() {
        fill_overlay_rect(buf, stride, chip_x, chip_y, chip_w, chip_h, 0xC0_1A_1D_28);
        if i == state.spaces.current_index() {
            draw_overlay_ring(buf, stride, chip_x, chip_y, chip_w, chip_h, ring);
        }
        if let Some(name) = state.spaces.space_name(i) {
            let lx = chip_x + (chip_w - name.len() as i32 * 8) / 2;
            let ly = chip_y + (chip_h - 8) / 2;
            let fg = if i == state.spaces.current_index() {
                ring
            } else {
                0xFF_C8_CC_D8
            };
            draw_overlay_label(buf, stride, lx.max(chip_x + 4), ly, name, fg);
        }
        chip_x += chip_w + chip_gap;
    }

    // ── Per-cell title + selection ring ─────────────────────────────────────
    // Grid area sits below the spaces strip; cell origins match the compositor's
    // near-square Tile layout over the (strip-offset) screen.
    let n = ids.len() as u32;
    if n == 0 {
        let _ = crate::compositor::present_surface(state.surface_id, 0, 0);
        return;
    }
    let mut cols = 1u32;
    while cols * cols < n {
        cols += 1;
    }
    let rows = (n + cols - 1) / cols;
    let grid_y0 = SPACES_STRIP_H;
    let grid_h = h - grid_y0;
    let cell_w = w / cols as i32;
    let cell_h = grid_h / rows as i32;

    for (i, &sid) in ids.iter().enumerate() {
        let col = (i as u32 % cols) as i32;
        let row = (i as u32 / cols) as i32;
        let cell_x = col * cell_w;
        let cell_y = grid_y0 + row * cell_h;
        // Selected cell: accent ring around the cell (the thumbnail fills it
        // aspect-fit; the ring at the cell edge reads as the selection box).
        if i == state.overview_sel {
            fill_overlay_rect(
                buf,
                stride,
                cell_x + OVERVIEW_GUTTER / 2,
                cell_y + OVERVIEW_GUTTER / 2,
                cell_w - OVERVIEW_GUTTER,
                cell_h - OVERVIEW_GUTTER,
                glow,
            );
            draw_overlay_ring(
                buf,
                stride,
                cell_x + OVERVIEW_GUTTER / 2,
                cell_y + OVERVIEW_GUTTER / 2,
                cell_w - OVERVIEW_GUTTER,
                cell_h - OVERVIEW_GUTTER,
                ring,
            );
        }
        // Title at the cell bottom (in the aspect-fit gutter, above thumbnails).
        let label = crate::compositor::surface_title(sid);
        let lx = cell_x + OVERVIEW_GUTTER;
        let ly = cell_y + cell_h - 20;
        let fg = if i == state.overview_sel {
            ring
        } else {
            0xFF_E8_EA_F0
        };
        draw_overlay_label(buf, stride, lx, ly, &label, fg);
    }

    let _ = crate::compositor::present_surface(state.surface_id, 0, 0);
}

/// Apply a space switch: flip each surface's compositor visibility (via
/// `set_surface_minimized` — the available "drop out of the active composite"
/// path) per the membership model, then ramp the wallpaper cross-fade.
pub(super) fn apply_space_switch(
    state: &mut ShellRunnerState,
    flips: alloc::vec::Vec<(u64, bool)>,
) {
    let mut hidden = 0u32;
    let mut shown = 0u32;
    for (id, should_show) in &flips {
        // visible == not minimized; a non-member is hidden by minimizing it.
        let _ = crate::compositor::set_surface_minimized(*id, !*should_show);
        if *should_show {
            shown += 1;
        } else {
            hidden += 1;
        }
    }
    // Wallpaper cross-fade (window-management.md §2): reduced-motion = hard cut.
    if reduced_motion() {
        crate::compositor::set_wallpaper_alpha(255);
    } else {
        // Drive a few ramp steps inline (the compositor recomposites each set);
        // the switch is brief and CPU0-cheap. 0 -> 255 over ~5 steps.
        for a in [0u8, 64, 128, 192, 255] {
            crate::compositor::set_wallpaper_alpha(a);
        }
    }
    state.spaces.advance_fade(255);
    if state.overview_open {
        render_overview_chrome(state);
    } else {
        repaint_desktop(state);
    }
    crate::serial_println!(
        "[shell_runner] space -> {}/{} (hidden={} shown={})",
        state.spaces.current_index() + 1,
        state.spaces.count(),
        hidden,
        shown
    );
}

/// Move the focused window to space `target` (creating intermediate spaces as
/// needed), then flip its visibility for the active space (window-management.md
/// §2 move-window-to-space, the keyboard path Super+Shift+N).
pub(super) fn move_focused_to_space(state: &mut ShellRunnerState, target: usize) {
    let focused = match crate::compositor::focused_surface_id() {
        Some(id) => id,
        None => {
            // Fall back to the topmost current-space surface.
            let mut surfaces = crate::compositor::list_userspace_surfaces();
            surfaces.sort_by_key(|(_, z)| *z);
            match surfaces.last().map(|(id, _)| *id) {
                Some(id) => id,
                None => return,
            }
        }
    };
    // Grow the spaces list so `target` exists.
    while state.spaces.count() <= target {
        let _ = state.spaces.add_space();
    }
    let (removed, added) = state.spaces.move_window(focused, target);
    if added {
        // The window left the current space, so hide it now.
        if !state.spaces.is_visible(focused) {
            let _ = crate::compositor::set_surface_minimized(focused, true);
        }
        if state.overview_open {
            state.overview_sel = 0;
            render_overview_chrome(state);
        } else {
            repaint_desktop(state);
        }
    }
    crate::serial_println!(
        "[shell_runner] move window {} -> space {} (removed={} added={})",
        focused,
        target + 1,
        removed,
        added
    );
}

pub(super) fn cycle_alt_tab(state: &mut ShellRunnerState) {
    let mut surfaces = crate::compositor::list_userspace_surfaces();
    if surfaces.is_empty() {
        return;
    }
    surfaces.sort_by_key(|(_, z)| *z);
    if !state.alt_tab_open {
        state.alt_tab_open = true;
        state.alt_tab_index = 0;
    } else {
        state.alt_tab_index = (state.alt_tab_index + 1) % surfaces.len();
    }
    let (sid, _) = surfaces[state.alt_tab_index];
    let _ = crate::compositor::focus_surface(sid);
    if let Some(ref mut shell) = state.shell {
        let banner = alloc::format!("Welcome, {}", crate::session::display_name());
        render_shell(shell, state.surface_ptr, state.width, state.height, &banner);
    }
    if state.shell.is_some() {
        render_alt_tab_overlay(state, &surfaces);
        let _ = crate::compositor::present_surface(state.surface_id, 0, 0);
    }
}

/// Thumbnail dimensions for an Alt+Tab preview tile (window-management.md §4:
/// a small live preview from `snapshot_surface`, ~16:9, ≥48px tall for couch).
const SWITCHER_THUMB_W: u32 = 96;
const SWITCHER_THUMB_H: u32 = 54;

/// Live-preview Alt+Tab switcher (window-management.md §4). Each row is a glass
/// strip carrying a `snapshot_surface` thumbnail + the app title; the selected
/// tile gets a 2px `derive_accent(seed).base` ring (the retired `0xFF_4E_9C_FF`
/// hardcode is gone) and an `elev.focus` glow wash — focus is never color-only.
fn render_alt_tab_overlay(state: &ShellRunnerState, surfaces: &[(u64, u32)]) {
    let pixels = state.width as usize * state.height as usize;
    let stride = state.width as usize;
    let buf = unsafe { core::slice::from_raw_parts_mut(state.surface_ptr as *mut u32, pixels) };

    // Tokenized accent (re-skins with Vibe Mode — the cohesion proof).
    let seed = raeshell::active_accent();
    let ring = raeshell::spaces::selection_ring(seed);
    let glow = raeshell::spaces::selection_glow(seed);

    let row_h = (SWITCHER_THUMB_H as i32 + 16).max(48); // ≥48px couch floor
    let overlay_w = 420i32;
    let overlay_h = (surfaces.len() as i32 * row_h + 56).min(state.height as i32 - 80);
    let ox = ((state.width as i32) - overlay_w) / 2;
    let oy = ((state.height as i32) - overlay_h) / 2;

    // material.glass panel tint (bg.overlay-ish dim) over the live desktop.
    for row in 0..overlay_h {
        for col in 0..overlay_w {
            let x = ox + col;
            let y = oy + row;
            if x < 0 || y < 0 {
                continue;
            }
            let idx = y as usize * stride + x as usize;
            if idx >= buf.len() {
                continue;
            }
            buf[idx] = blend_px(buf[idx], 0xE0_1A_1D_28u32);
        }
    }
    let title_y = oy + 14;
    draw_overlay_label(
        buf,
        stride,
        ox + 18,
        title_y,
        "Switch window (Alt+Tab)",
        0xFF_E8_EA_F0,
    );

    // One reusable thumbnail scratch buffer (heap; the switcher is short-lived,
    // and we refresh on index advance, not per frame).
    let mut thumb = alloc::vec![0u32; (SWITCHER_THUMB_W * SWITCHER_THUMB_H) as usize];

    for (i, (sid, _)) in surfaces.iter().enumerate() {
        let row_y = oy + 44 + (i as i32 * row_h);
        let selected = i == state.alt_tab_index;

        // Selected-tile background wash (accent.glow) + 2px accent ring — the
        // hover-independent focus signal (design-language §8).
        if selected {
            fill_overlay_rect(buf, stride, ox + 8, row_y - 4, overlay_w - 16, row_h, glow);
            draw_overlay_ring(buf, stride, ox + 8, row_y - 4, overlay_w - 16, row_h, ring);
        }

        // Live thumbnail (snapshot_surface box-downscale) at the row's left.
        let tx = ox + 16;
        let ty = row_y;
        let ok = unsafe {
            crate::compositor::snapshot_surface(
                *sid,
                thumb.as_mut_ptr() as *mut u8,
                SWITCHER_THUMB_W,
                SWITCHER_THUMB_H,
            )
        };
        if ok {
            blit_overlay_thumb(
                buf,
                stride,
                tx,
                ty,
                &thumb,
                SWITCHER_THUMB_W as i32,
                SWITCHER_THUMB_H as i32,
            );
        } else {
            // No last frame yet — a placeholder card so the row still reads.
            fill_overlay_rect(
                buf,
                stride,
                tx,
                ty,
                SWITCHER_THUMB_W as i32,
                SWITCHER_THUMB_H as i32,
                0xFF_28_2C_44,
            );
        }

        // Title to the right of the thumbnail.
        let label = crate::compositor::surface_title(*sid);
        let label_x = tx + SWITCHER_THUMB_W as i32 + 14;
        let label_y = ty + (SWITCHER_THUMB_H as i32 - 8) / 2;
        let fg = if selected { ring } else { 0xFF_C8_CC_D8 };
        draw_overlay_label(buf, stride, label_x, label_y, &label, fg);
    }
}

/// Opaque-fill a rect into the desktop surface buffer (clipped), alpha-blended
/// if `color` carries alpha < 255.
fn fill_overlay_rect(buf: &mut [u32], stride: usize, x: i32, y: i32, w: i32, h: i32, color: u32) {
    for row in 0..h {
        for col in 0..w {
            let px = x + col;
            let py = y + row;
            if px < 0 || py < 0 {
                continue;
            }
            let idx = py as usize * stride + px as usize;
            if idx >= buf.len() {
                continue;
            }
            let a = (color >> 24) & 0xFF;
            buf[idx] = if a >= 255 {
                color
            } else {
                blend_px(buf[idx], color)
            };
        }
    }
}

/// Draw a 2px rectangle outline (the accent selection ring) into the buffer.
fn draw_overlay_ring(buf: &mut [u32], stride: usize, x: i32, y: i32, w: i32, h: i32, color: u32) {
    for t in 0..2i32 {
        for col in 0..w {
            for &py in &[y + t, y + h - 1 - t] {
                let px = x + col;
                if px < 0 || py < 0 {
                    continue;
                }
                let idx = py as usize * stride + px as usize;
                if idx < buf.len() {
                    buf[idx] = color;
                }
            }
        }
        for row in 0..h {
            for &px in &[x + t, x + w - 1 - t] {
                let py = y + row;
                if px < 0 || py < 0 {
                    continue;
                }
                let idx = py as usize * stride + px as usize;
                if idx < buf.len() {
                    buf[idx] = color;
                }
            }
        }
    }
}

/// Blit a downscaled ARGB thumbnail buffer into the desktop surface (1:1, the
/// thumbnail is already the target size from `snapshot_surface`).
fn blit_overlay_thumb(
    buf: &mut [u32],
    stride: usize,
    x: i32,
    y: i32,
    thumb: &[u32],
    tw: i32,
    th: i32,
) {
    for row in 0..th {
        for col in 0..tw {
            let px = x + col;
            let py = y + row;
            if px < 0 || py < 0 {
                continue;
            }
            let didx = py as usize * stride + px as usize;
            let sidx = (row * tw + col) as usize;
            if didx >= buf.len() || sidx >= thumb.len() {
                continue;
            }
            let p = thumb[sidx];
            let a = (p >> 24) & 0xFF;
            buf[didx] = if a == 0 {
                0xFF_1A_1D_28
            } else {
                (p & 0x00FF_FFFF) | 0xFF00_0000
            };
        }
    }
}

fn blend_px(dst: u32, src: u32) -> u32 {
    let sa = ((src >> 24) & 0xFF) as u32;
    let sr = (src >> 16) & 0xFF;
    let sg = (src >> 8) & 0xFF;
    let sb = src & 0xFF;
    let dr = (dst >> 16) & 0xFF;
    let dg = (dst >> 8) & 0xFF;
    let db = dst & 0xFF;
    let inv = 256 - sa;
    let r = (sr * sa + dr * inv) / 256;
    let g = (sg * sa + dg * inv) / 256;
    let b = (sb * sa + db * inv) / 256;
    0xFF_00_00_00 | (r << 16) | (g << 8) | b
}

fn draw_overlay_label(buf: &mut [u32], stride: usize, x: i32, y: i32, text: &str, color: u32) {
    let mut tx = x;
    for _ch in text.chars().take(32) {
        for row in 0..8 {
            for col in 0..8 {
                let px = tx + col;
                let py = y + row;
                if px < 0 || py < 0 {
                    continue;
                }
                let idx = py as usize * stride + px as usize;
                if idx < buf.len() && (row + col) % 3 != 0 {
                    buf[idx] = color;
                }
            }
        }
        tx += 8;
    }
}
