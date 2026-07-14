//! Clipboard-history panel — the Win+V-class glass flyout for RaeShell.
//!
//! A keystroke (`Super+C`) summons a glass panel of everything recently copied;
//! pin the keepers, delete the rest, clear-all (keeping pins), and **paste-on-
//! select** — clicking or Enter on a row promotes it to the active clipboard so
//! the focused app's paste reads it, then the panel closes. This is the
//! Windows 11 Win+V model rendered with macOS-grade materiality (`material.glass`
//! + `elev.3`) and the live shell accent — per `docs/design/clipboard-history.md`.
//!
//! Concept: *"The user owns the machine — no forced telemetry."*
//! (RaeenOS_Concept.md §The user owns the machine) → the history is local-only,
//! RAM-only, owned by the user.
//!
//! ## Architecture (why this is a snapshot widget, not a syscall caller)
//!
//! This crate is linked into the kernel (`#![no_std]`, no syscalls of its own),
//! so the panel cannot call `raekit::sys::clip_hist_*` directly. Instead it is a
//! pure **view + selection** widget: the kernel's `shell_runner` reads the live
//! history from `crate::clipboard::history_*` and pushes a `Vec<ClipRow>`
//! snapshot in via [`ClipboardPanel::set_rows`] when the panel opens or changes;
//! the panel renders that snapshot and reports the user's intent
//! ([`ClipPanelAction`]) which the kernel executes through the same history API
//! (the userspace `raekit::sys::clip_hist_*` wrappers are the equivalent surface
//! for separate-process apps). The ordering authority is
//! [`crate::clipboard::ClipboardManager`] itself: the snapshot is loaded into a
//! manager (newest-first via `copy` + `pin`), and the display order — **pinned
//! above recent, recent newest-first, pinned never evicted** — is read back from
//! `ClipboardManager::history()`, so this panel and the rich manager share one
//! ordering model rather than re-deriving it.

use crate::clipboard::{ClipboardContent, ClipboardManager};
use alloc::string::String;
use alloc::vec::Vec;

/// One history row as the kernel snapshots it for the panel. Mirrors the
/// `raekit::sys::ClipEntry` fields the panel needs (format + pinned flag +
/// preview text), pre-trimmed to a single preview line by the caller.
#[derive(Debug, Clone)]
pub struct ClipRow {
    /// `CLIP_FMT_*` tag (0 = text today; the rest reserved). Drives the badge.
    pub format: u32,
    /// True if the entry is pinned (exempt from eviction + clear).
    pub pinned: bool,
    /// The preview line (already a single line, kernel-capped). Empty -> "(empty)".
    pub preview: String,
}

/// The format-badge label for a `CLIP_FMT_*` tag (design-language §3 badge).
/// Mirrors `raekit::sys::CLIP_FMT_*` so a future image/url/files entry reads
/// correctly without a code change here.
fn format_badge(format: u32) -> &'static str {
    match format {
        0 => "TXT",  // CLIP_FMT_TEXT
        1 => "HTML", // CLIP_FMT_RICH_TEXT
        2 => "IMG",  // CLIP_FMT_IMAGE
        3 => "FILE", // CLIP_FMT_FILES
        4 => "URL",  // CLIP_FMT_URL
        5 => "COL",  // CLIP_FMT_COLOR
        _ => "BIN",
    }
}

/// What the user asked the host (kernel `shell_runner`) to do with the panel.
/// The panel owns no syscalls, so each interaction returns an intent the kernel
/// executes against `crate::clipboard::history_*` (the same surface the
/// `raekit::sys::clip_hist_*` userspace wrappers reach).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipPanelAction {
    /// Nothing to do (no rows, or a no-op key).
    None,
    /// Promote the history entry at this **history index** to the active
    /// clipboard (paste-on-select), then close the panel.
    Promote(usize),
    /// Toggle the pin on the history entry at this index.
    TogglePin(usize),
    /// Delete the history entry at this index (refused kernel-side if pinned).
    Delete(usize),
    /// Clear all unpinned entries (keep pinned) — the top-bar "Clear all".
    ClearAll,
    /// Close the panel with no other effect.
    Close,
}

/// The clipboard-history panel surface — a single global flyout instance.
///
/// Holds a snapshot of the live history plus the selection cursor. The list is
/// ordered **pinned-first then recent**, but each row carries its real
/// `history_index` so an action maps back to the kernel ring unambiguously even
/// though the panel reorders for display.
pub struct ClipboardPanel {
    pub visible: bool,
    /// Display-ordered rows: pinned block first, then recent (newest-first).
    /// Each is `(history_index, ClipRow)` — the index is the kernel ring index.
    rows: Vec<(usize, ClipRow)>,
    /// Count of pinned rows (always the leading block of `rows`).
    pinned_count: usize,
    /// Selected display row (0-based into `rows`); the first Recent (or first
    /// Pinned if there is no Recent) is selected on open for blind Super+C→Enter.
    pub selected: usize,
    /// Incognito posture (privacy strip, design-language §6). View-only here;
    /// the kernel honours it on the copy path.
    pub incognito: bool,
    pub screen_width: usize,
    pub screen_height: usize,
}

/// Panel width — design-language §2: 360px (quick-settings flyout family).
const PANEL_W: usize = 360;
/// Panel max height — design-language §2: 480px, the list scrolls beyond.
const PANEL_MAX_H: usize = 480;
/// Text/link/files/color row height — design-language §3: 44px (≥32 floor).
const ROW_H: usize = 44;
/// Header bar height (title + clear-all + incognito).
const HEADER_H: usize = 32;
/// Section header height ("Pinned" / "Recent").
const SECTION_H: usize = 20;
/// Max rows rendered before the list virtualizes (design-language §6).
const MAX_VISIBLE_ROWS: usize = 8;

impl ClipboardPanel {
    #[must_use]
    pub fn new(screen_width: usize, screen_height: usize) -> Self {
        Self {
            visible: false,
            rows: Vec::new(),
            pinned_count: 0,
            selected: 0,
            incognito: false,
            screen_width,
            screen_height,
        }
    }

    /// Replace the panel's snapshot. `rows` is the raw history ring newest-first
    /// (history index 0 = newest), exactly as `crate::clipboard::history_*`
    /// yields it. The newest-first ordering is run through a
    /// [`ClipboardManager`] (the shared ordering authority) and then partitioned
    /// to **pinned-first, recent-newest-first** for display, while each row keeps
    /// its real kernel history index for action mapping.
    pub fn set_rows(&mut self, rows: Vec<ClipRow>) {
        // Load the snapshot into the rich manager to establish ordering. The
        // manager prepends on `copy` (newest at index 0), so feeding it the ring
        // oldest-first reproduces the kernel's newest-first order in `history()`.
        let mut manager = ClipboardManager::new(rows.len().max(1));
        // Remember each preview's kernel index (newest-first input order).
        let kernel_index: Vec<(u32, bool, String)> = rows
            .iter()
            .map(|r| (r.format, r.pinned, r.preview.clone()))
            .collect();
        for r in rows.iter().rev() {
            manager.copy(ClipboardContent::Text(r.preview.clone()), None);
        }
        // Apply pins on the manager so its newest-first `history()` carries the
        // pin flags through the shared model (manager index == newest-first pos).
        for (i, r) in kernel_index.iter().enumerate() {
            if r.1 {
                manager.pin(i);
            }
        }

        // Read the ordered history back and partition pinned-above-recent for the
        // view, mapping each manager entry to its kernel history index by
        // newest-first position (same order in and out).
        let mut pinned: Vec<(usize, ClipRow)> = Vec::new();
        let mut recent: Vec<(usize, ClipRow)> = Vec::new();
        for (pos, entry) in manager.history().iter().enumerate() {
            let (format, _, preview) =
                kernel_index
                    .get(pos)
                    .cloned()
                    .unwrap_or((0, false, String::new()));
            let row = ClipRow {
                format,
                pinned: entry.pinned,
                preview,
            };
            if entry.pinned {
                pinned.push((pos, row));
            } else {
                recent.push((pos, row));
            }
        }
        self.pinned_count = pinned.len();
        let mut combined = pinned;
        combined.extend(recent);
        self.rows = combined;
        // Keep the selection in range; on a fresh snapshot prefer the first
        // Recent entry (blind Super+C→Enter pastes the last copy) — that is the
        // row just after the pinned block, clamped to the list.
        let default_sel = self.pinned_count.min(self.rows.len().saturating_sub(1));
        if self.selected >= self.rows.len() {
            self.selected = default_sel;
        }
    }

    /// Open the panel: select the first Recent entry (the 90% case — paste the
    /// last copy) and show it. The caller pushes a fresh snapshot first.
    pub fn open(&mut self) {
        self.visible = true;
        self.selected = self.pinned_count.min(self.rows.len().saturating_sub(1));
    }

    pub fn close(&mut self) {
        self.visible = false;
        self.selected = 0;
    }

    /// Toggle open/close (the `Super+C` hotkey semantics).
    pub fn toggle(&mut self) {
        if self.visible {
            self.close();
        } else {
            self.open();
        }
    }

    /// Number of display rows (pinned + recent).
    #[must_use]
    pub fn row_count(&self) -> usize {
        self.rows.len()
    }

    /// Number of pinned rows (the leading block).
    #[must_use]
    pub fn pinned_count(&self) -> usize {
        self.pinned_count
    }

    /// True if there is a Pinned section to render (≥1 pinned entry).
    #[must_use]
    pub fn has_pinned_section(&self) -> bool {
        self.pinned_count > 0
    }

    /// The history index of the selected display row, if any.
    #[must_use]
    pub fn selected_history_index(&self) -> Option<usize> {
        self.rows.get(self.selected).map(|(idx, _)| *idx)
    }

    /// The preview of the selected row (for the smoketest / a11y).
    #[must_use]
    pub fn selected_preview(&self) -> Option<&str> {
        self.rows
            .get(self.selected)
            .map(|(_, r)| r.preview.as_str())
    }

    pub fn select_next(&mut self) {
        let n = self.rows.len();
        if n > 0 {
            self.selected = (self.selected + 1) % n;
        }
    }

    pub fn select_prev(&mut self) {
        let n = self.rows.len();
        if n > 0 {
            self.selected = self.selected.checked_sub(1).unwrap_or(n - 1);
        }
    }

    /// Promote the selected row (paste-on-select). Returns the intent the kernel
    /// runs: `Promote(history_index)` then close, or `Close`/`None`.
    #[must_use]
    pub fn activate_selected(&self) -> ClipPanelAction {
        match self.selected_history_index() {
            Some(idx) => ClipPanelAction::Promote(idx),
            None => ClipPanelAction::None,
        }
    }

    /// Toggle pin on the selected row (`P` / the pin button).
    #[must_use]
    pub fn toggle_pin_selected(&self) -> ClipPanelAction {
        match self.selected_history_index() {
            Some(idx) => ClipPanelAction::TogglePin(idx),
            None => ClipPanelAction::None,
        }
    }

    /// Delete the selected row (`Delete`). The kernel refuses a pinned entry.
    #[must_use]
    pub fn delete_selected(&self) -> ClipPanelAction {
        match self.selected_history_index() {
            Some(idx) => ClipPanelAction::Delete(idx),
            None => ClipPanelAction::None,
        }
    }

    /// Map a quick-paste digit `1..=9` to a `Promote` of the Nth visible row
    /// (Maccy-style number paste, design-language §5).
    #[must_use]
    pub fn quick_paste(&self, digit: usize) -> ClipPanelAction {
        if digit == 0 {
            return ClipPanelAction::None;
        }
        match self.rows.get(digit - 1) {
            Some((idx, _)) => ClipPanelAction::Promote(*idx),
            None => ClipPanelAction::None,
        }
    }

    /// The panel's on-screen rect (x, y, w, h), bottom-centered above the
    /// taskbar (design-language §2 — no caret anchoring in the kernel shell yet).
    fn panel_rect(&self) -> (usize, usize, usize, usize) {
        use rae_tokens::SPACE_4;
        let pad = SPACE_4 as usize;
        let visible = self.rows.len().min(MAX_VISIBLE_ROWS);
        let sections = (if self.pinned_count > 0 { SECTION_H } else { 0 })
            + (if visible > self.pinned_count.min(visible) {
                SECTION_H
            } else {
                0
            });
        let list_h = visible * ROW_H + sections;
        let inner_h = HEADER_H + list_h + if self.incognito { SECTION_H } else { 0 };
        let panel_h = (pad + inner_h + pad).min(PANEL_MAX_H);
        let panel_w = PANEL_W.min(self.screen_width.saturating_sub(2 * pad));
        let panel_x = self.screen_width.saturating_sub(panel_w) / 2;
        // Above the 44px taskbar with a space.4 gap.
        let taskbar = 44usize;
        let panel_y = self
            .screen_height
            .saturating_sub(taskbar)
            .saturating_sub(pad)
            .saturating_sub(panel_h);
        (panel_x, panel_y, panel_w, panel_h)
    }

    /// Render the glass flyout (design-language §2/§3). Same transient-glass
    /// family as the command palette / Start (material.glass, radius.lg,
    /// elev.3, accent from `derive_accent`).
    pub fn render(&self, canvas: &mut raegfx::Canvas) {
        if !self.visible {
            return;
        }
        use rae_tokens::{RADIUS_LG, RADIUS_XS, SPACE_2, SPACE_3, SPACE_4};
        let accent = crate::accent();
        let p = crate::PALETTE;
        let pad = SPACE_4 as usize;

        let (panel_x, panel_y, panel_w, panel_h) = self.panel_rect();

        // ── Glass panel: tint + radius.lg + top-edge highlight + hairline.
        //    (material.glass + elev.3 — the transient-flyout material.)
        canvas.fill_rounded_rect(
            panel_x,
            panel_y,
            panel_w,
            panel_h,
            RADIUS_LG as usize,
            rae_tokens::GLASS_TINT_DARK,
        );
        canvas.draw_rounded_rect_outline(
            panel_x,
            panel_y,
            panel_w,
            panel_h,
            RADIUS_LG as usize,
            p.stroke_subtle,
        );
        for xx in panel_x + RADIUS_LG as usize..panel_x + panel_w - RADIUS_LG as usize {
            canvas.blend_pixel(xx, panel_y + 1, p.stroke_strong);
        }

        let content_x = panel_x + pad;
        let content_w = panel_w - 2 * pad;

        // ── Header bar: "Clipboard" title (type.subtitle) + right-aligned
        //    Clear-all (state.danger family) and an incognito hint.
        let header_y = panel_y + pad;
        canvas.draw_text_aa(
            content_x as i32,
            (header_y
                + (HEADER_H.saturating_sub(rae_tokens::TYPE_SUBTITLE.line_height as usize)) / 2)
                as i32,
            "Clipboard",
            rae_tokens::TYPE_SUBTITLE,
            p.text_primary,
            raegfx::text::FontFamily::Sans,
        );
        // Clear-all label, right-aligned in the header (type.caption).
        let clear_label = "Clear all";
        let clear_w = canvas.measure_text_aa(
            clear_label,
            rae_tokens::TYPE_CAPTION,
            raegfx::text::FontFamily::Sans,
        );
        let clear_x = (content_x + content_w) as i32 - clear_w;
        canvas.draw_text_aa(
            clear_x,
            (header_y
                + (HEADER_H.saturating_sub(rae_tokens::TYPE_CAPTION.line_height as usize)) / 2)
                as i32,
            clear_label,
            rae_tokens::TYPE_CAPTION,
            p.state_danger,
            raegfx::text::FontFamily::Sans,
        );

        let mut cursor_y = header_y + HEADER_H;

        // ── Incognito strip (design-language §6) ────────────────────────────
        if self.incognito {
            canvas.fill_rounded_rect(
                content_x,
                cursor_y,
                content_w,
                SECTION_H,
                RADIUS_XS as usize,
                accent.subtle,
            );
            canvas.draw_text_aa(
                (content_x + SPACE_2 as usize) as i32,
                (cursor_y
                    + (SECTION_H.saturating_sub(rae_tokens::TYPE_CAPTION.line_height as usize)) / 2)
                    as i32,
                "Incognito — not saving",
                rae_tokens::TYPE_CAPTION,
                accent.text,
                raegfx::text::FontFamily::Sans,
            );
            cursor_y += SECTION_H;
        }

        // Empty state.
        if self.rows.is_empty() {
            canvas.draw_text_aa(
                content_x as i32,
                (cursor_y + SPACE_2 as usize) as i32,
                "Nothing copied yet",
                rae_tokens::TYPE_BODY,
                p.text_tertiary,
                raegfx::text::FontFamily::Sans,
            );
            return;
        }

        // ── Sections: Pinned (if any) then Recent. Rows render in display
        //    order; the first MAX_VISIBLE_ROWS are shown (the list virtualizes).
        let mut shown = 0usize;
        let mut printed_recent_header = false;
        let mut printed_pinned_header = false;

        for (i, (_hist_idx, row)) in self.rows.iter().enumerate() {
            if shown >= MAX_VISIBLE_ROWS {
                break;
            }
            let is_pinned_block = i < self.pinned_count;

            // Section header before the first row of each section.
            if is_pinned_block && !printed_pinned_header {
                self.draw_section_header(canvas, content_x, cursor_y, "Pinned", p);
                cursor_y += SECTION_H;
                printed_pinned_header = true;
            }
            if !is_pinned_block && !printed_recent_header {
                self.draw_section_header(canvas, content_x, cursor_y, "Recent", p);
                cursor_y += SECTION_H;
                printed_recent_header = true;
            }

            let ry = cursor_y;
            let selected = i == self.selected;

            // Selected = accent.subtle fill + 2px accent.base left bar (focus is
            // never colour-only — design-language §7). Pinned rows additionally
            // carry a faint accent.subtle left edge so they read as "kept".
            if selected {
                canvas.fill_rounded_rect(
                    content_x,
                    ry,
                    content_w,
                    ROW_H - 2,
                    RADIUS_XS as usize,
                    accent.subtle,
                );
                canvas.fill_rect(content_x, ry, 2, ROW_H - 2, accent.base);
            } else if is_pinned_block {
                canvas.fill_rect(content_x, ry, 2, ROW_H - 2, accent.subtle);
            }

            // Format badge chip (left inset, radius.xs, bg.elevated).
            let badge = format_badge(row.format);
            let badge_w = canvas.measure_text_aa(
                badge,
                rae_tokens::TYPE_CAPTION,
                raegfx::text::FontFamily::Sans,
            );
            let badge_pad = SPACE_2 as usize;
            let badge_x = content_x + SPACE_3 as usize;
            let chip_w = badge_w.max(0) as usize + 2 * badge_pad;
            let chip_h = rae_tokens::TYPE_CAPTION.line_height as usize + 4;
            let chip_y = ry + (ROW_H.saturating_sub(chip_h)) / 2;
            canvas.fill_rounded_rect(
                badge_x,
                chip_y,
                chip_w,
                chip_h,
                RADIUS_XS as usize,
                p.bg_elevated,
            );
            canvas.draw_text_aa(
                (badge_x + badge_pad) as i32,
                (chip_y + 2) as i32,
                badge,
                rae_tokens::TYPE_CAPTION,
                p.text_secondary,
                raegfx::text::FontFamily::Sans,
            );

            // Preview line (type.body, text.primary), truncated to the row width.
            let preview_x = (badge_x + chip_w + SPACE_2 as usize) as i32;
            let pin_glyph_w = 12i32; // reserve the right cluster
            let avail = (content_x + content_w) as i32 - preview_x - pin_glyph_w - SPACE_2 as i32;
            let preview = truncate_to_width(canvas, &row.preview, avail.max(0) as usize);
            let preview_text = if preview.is_empty() {
                "(empty)"
            } else {
                preview.as_str()
            };
            canvas.draw_text_aa(
                preview_x,
                (ry + (ROW_H.saturating_sub(rae_tokens::TYPE_BODY.line_height as usize)) / 2)
                    as i32,
                preview_text,
                rae_tokens::TYPE_BODY,
                p.text_primary,
                raegfx::text::FontFamily::Sans,
            );

            // Pin glyph (right) — filled accent.text when pinned, outline '+'
            // hint when unpinned and selected (the per-row affordance reveal).
            let pin_x = (content_x + content_w) as i32 - pin_glyph_w;
            let pin_y = ry + (ROW_H.saturating_sub(8)) / 2;
            if row.pinned {
                canvas.draw_glyph(pin_x as usize, pin_y, '*', accent.text, None);
            } else if selected {
                canvas.draw_glyph(pin_x as usize, pin_y, '+', p.text_tertiary, None);
            }

            cursor_y += ROW_H;
            shown += 1;
        }

        // Footer item count (type.caption, text.tertiary) — the bounded-memory
        // posture (design-language §6 "N items").
        let footer = count_footer(self.rows.len(), self.pinned_count);
        canvas.draw_text_aa(
            content_x as i32,
            (panel_y + panel_h - pad) as i32,
            &footer,
            rae_tokens::TYPE_CAPTION,
            p.text_tertiary,
            raegfx::text::FontFamily::Sans,
        );
    }

    fn draw_section_header(
        &self,
        canvas: &mut raegfx::Canvas,
        x: usize,
        y: usize,
        label: &str,
        p: &rae_tokens::Palette,
    ) {
        canvas.draw_text_aa(
            x as i32,
            (y + (SECTION_H.saturating_sub(rae_tokens::TYPE_CAPTION.line_height as usize)) / 2)
                as i32,
            label,
            rae_tokens::TYPE_CAPTION,
            p.text_secondary,
            raegfx::text::FontFamily::Sans,
        );
    }

    /// FAIL-able design proof of the panel surface (R10): the panel reads the
    /// live accent, the pinned section sits above recent, and a populated
    /// snapshot renders ≥1 row. Returned to the kernel smoketest.
    #[must_use]
    pub fn design_proof() -> ClipPanelProof {
        let mut panel = ClipboardPanel::new(1280, 720);
        // A representative ring: newest copy (index 0) is unpinned, a pinned
        // keeper is older (index 1), plus an older URL (index 2). Newest-first,
        // exactly as the kernel snapshots it.
        panel.set_rows(alloc::vec![
            ClipRow {
                format: 0,
                pinned: false,
                preview: String::from("newest copy"),
            },
            ClipRow {
                format: 0,
                pinned: true,
                preview: String::from("pinned snippet"),
            },
            ClipRow {
                format: 4,
                pinned: false,
                preview: String::from("https://raeen.os"),
            },
        ]);
        panel.open();

        // Pinned block leads the display order, recent follows.
        let pinned_first =
            panel.rows.first().map(|(_, r)| r.pinned).unwrap_or(false) && panel.pinned_count == 1;
        // On open, the first Recent entry is selected (blind Super+C→Enter pastes
        // the last copy): display row == pinned_count (1), kernel history index 0.
        let selects_newest_recent = panel.selected == 1
            && panel.selected_history_index() == Some(0)
            && panel.selected_preview() == Some("newest copy");
        // Promote returns the selected entry's kernel history index (0 = newest).
        let promote_ok = panel.activate_selected() == ClipPanelAction::Promote(0);
        // Accent cohesion: the panel derives from the SAME live seed as the shell.
        let accent_base = crate::accent().base;
        let want_accent = rae_tokens::derive_accent(crate::active_accent(), crate::PALETTE).base;
        let accent_ok = accent_base == want_accent;

        let pass = pinned_first && selects_newest_recent && promote_ok && accent_ok;
        ClipPanelProof {
            rows: panel.rows.len(),
            pinned: panel.pinned_count,
            has_pinned_section: panel.has_pinned_section(),
            promote_ok,
            accent_base,
            pass,
        }
    }
}

/// The result of [`ClipboardPanel::design_proof`] — the kernel logs it.
#[derive(Clone, Copy, Debug)]
pub struct ClipPanelProof {
    pub rows: usize,
    pub pinned: usize,
    pub has_pinned_section: bool,
    pub promote_ok: bool,
    pub accent_base: u32,
    pub pass: bool,
}

/// Truncate `text` so its rendered width fits `max_w` px, appending an ellipsis
/// when clipped. Uses the canvas's AA metrics (proportional RaeSans).
fn truncate_to_width(canvas: &raegfx::Canvas, text: &str, max_w: usize) -> String {
    if max_w == 0 {
        return String::new();
    }
    let full = canvas.measure_text_aa(text, rae_tokens::TYPE_BODY, raegfx::text::FontFamily::Sans);
    if full.max(0) as usize <= max_w {
        return String::from(text);
    }
    // Trim char-by-char until it fits with a trailing ellipsis.
    let mut out = String::new();
    for ch in text.chars() {
        let mut probe = out.clone();
        probe.push(ch);
        probe.push_str("...");
        let w = canvas.measure_text_aa(
            &probe,
            rae_tokens::TYPE_BODY,
            raegfx::text::FontFamily::Sans,
        );
        if w.max(0) as usize > max_w {
            break;
        }
        out.push(ch);
    }
    out.push_str("...");
    out
}

/// "N items - M pinned" footer (design-language §6 bounded-memory posture).
fn count_footer(total: usize, pinned: usize) -> String {
    if pinned > 0 {
        alloc::format!("{} items - {} pinned", total, pinned)
    } else {
        alloc::format!("{} items", total)
    }
}

// ── Host KATs (R10: a smoketest must be able to print FAIL) ────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> ClipboardPanel {
        let mut panel = ClipboardPanel::new(1280, 720);
        panel.set_rows(alloc::vec![
            // history index 0 = newest (unpinned)
            ClipRow {
                format: 0,
                pinned: false,
                preview: String::from("third")
            },
            // index 1 (pinned)
            ClipRow {
                format: 0,
                pinned: true,
                preview: String::from("kept")
            },
            // index 2 (unpinned)
            ClipRow {
                format: 4,
                pinned: false,
                preview: String::from("https://x")
            },
        ]);
        panel
    }

    #[test]
    fn pinned_section_above_recent() {
        let panel = sample();
        assert_eq!(panel.pinned_count(), 1);
        assert!(panel.has_pinned_section());
        // Display order: pinned block first.
        assert_eq!(panel.rows[0].1.preview, "kept");
        assert!(panel.rows[0].1.pinned);
        // Then recent, newest-first (history order preserved within recent).
        assert_eq!(panel.rows[1].1.preview, "third");
        assert_eq!(panel.rows[2].1.preview, "https://x");
    }

    #[test]
    fn history_index_preserved_through_reorder() {
        let panel = sample();
        // The pinned row at display 0 is history index 1.
        assert_eq!(panel.rows[0].0, 1);
        // The first recent row at display 1 is history index 0 (newest).
        assert_eq!(panel.rows[1].0, 0);
    }

    #[test]
    fn open_selects_first_recent() {
        let mut panel = sample();
        panel.open();
        // First Recent = display row 1 (just after the single pinned), history 0.
        assert_eq!(panel.selected, 1);
        assert_eq!(panel.selected_history_index(), Some(0));
        assert_eq!(panel.selected_preview(), Some("third"));
    }

    #[test]
    fn activate_promotes_history_index() {
        let mut panel = sample();
        panel.open();
        assert_eq!(panel.activate_selected(), ClipPanelAction::Promote(0));
    }

    #[test]
    fn quick_paste_maps_visible_rows() {
        let panel = sample();
        // 1 = first visible (pinned "kept", history 1).
        assert_eq!(panel.quick_paste(1), ClipPanelAction::Promote(1));
        // 2 = second visible (recent "third", history 0).
        assert_eq!(panel.quick_paste(2), ClipPanelAction::Promote(0));
        // 0 and out-of-range are no-ops.
        assert_eq!(panel.quick_paste(0), ClipPanelAction::None);
        assert_eq!(panel.quick_paste(9), ClipPanelAction::None);
    }

    #[test]
    fn toggle_and_delete_target_selection() {
        let mut panel = sample();
        panel.open();
        // Selected is history 0 (recent "third").
        assert_eq!(panel.toggle_pin_selected(), ClipPanelAction::TogglePin(0));
        assert_eq!(panel.delete_selected(), ClipPanelAction::Delete(0));
    }

    #[test]
    fn selection_wraps() {
        let mut panel = sample();
        panel.open();
        let n = panel.row_count();
        assert!(n == 3);
        panel.selected = 0;
        panel.select_prev();
        assert_eq!(panel.selected, n - 1, "prev from 0 wraps to last");
        panel.select_next();
        assert_eq!(panel.selected, 0, "next from last wraps to first");
    }

    #[test]
    fn empty_snapshot_is_safe() {
        let mut panel = ClipboardPanel::new(800, 600);
        panel.set_rows(alloc::vec![]);
        panel.open();
        assert_eq!(panel.row_count(), 0);
        assert!(!panel.has_pinned_section());
        assert_eq!(panel.activate_selected(), ClipPanelAction::None);
        assert_eq!(panel.selected_history_index(), None);
    }

    #[test]
    fn format_badges_cover_tags() {
        assert_eq!(format_badge(0), "TXT");
        assert_eq!(format_badge(4), "URL");
        assert_eq!(format_badge(99), "BIN");
    }

    #[test]
    fn design_proof_passes() {
        let proof = ClipboardPanel::design_proof();
        assert!(
            proof.pass,
            "clipboard panel design proof must pass: {proof:?}"
        );
        assert_eq!(proof.rows, 3);
        assert_eq!(proof.pinned, 1);
        assert!(proof.has_pinned_section);
        assert!(proof.promote_ok);
    }
}
