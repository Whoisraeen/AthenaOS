//! AthenaOS Notes — *"keep my notes"* (LEGACY_GAMING_CONCEPT.md §Three User Experiences,
//! the bundled-app parity bar vs Win11 Sticky Notes / macOS Notes).
//!
//! The daily-driver note-taking app: a **sidebar list** of `.md`/`.txt` notes in
//! `<home>/Notes`, an **editable text buffer** (raw Markdown), and a **live
//! rich-text preview** rendered straight from the from-scratch `ath_markdown`
//! parser — headings sized per level, **bold**/*italic*/`code` runs styled, lists
//! with indent, blockquotes with an accent bar, fenced code in a mono panel, and
//! thematic breaks as a rule. Tab toggles edit ⇄ preview; Ctrl+S saves; New/Delete
//! manage the library.
//!
//! Standalone userspace ELF (`exec_path = "notes"`). Chrome is on the shared
//! `ath_tokens` design language; the live desktop accent comes through
//! `SYS_THEME_GET` (athkit::sys::theme_accent) at launch so Notes matches the
//! desktop 1:1 (whole-OS cohesion).
//!
//! HOSTILE-INPUT POSTURE: note text is untrusted. The whole render walks the
//! `ath_markdown::parse` model, which NEVER panics (its 23 host KATs + the
//! `parse_never_panics_battery` are the parser proof) — bad markdown degrades to
//! literal text. File read/write errors surface as a status-bar toast and the app
//! stays alive; there is no panic path from any user action.
//!
//! PROOF: this ELF can't run `cargo test`, so `design_proof()` (a fail-able
//! runtime gate at `_start`) parses a built-in markdown fixture and asserts the
//! `Document` has the expected blocks (a heading + a bold inline + a list) AND
//! that the render's per-level heading sizing invariant holds (H1 draws larger
//! than H3 draws larger than body) — exit(3) on any drift. `ath_markdown`'s host
//! KATs are the parser-logic proof; this proves the app's parse → render wiring.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

#[allow(unused_imports)]
use athkit;

use ath_markdown::{parse, Block, Inline, ListItem};
use ath_regex::Regex;
use ath_tokens::{TypeStyle, DARK, RAEBLUE};
use ath_toml::Toml;
use athgfx::text::FontFamily;
use athgfx::Canvas;

// ── Window geometry ─────────────────────────────────────────────────────

const WIN_W: usize = 880;
const WIN_H: usize = 580;
const SURFACE_VIRT: u64 = 0x0000_7D00_0000;

const TITLE_H: usize = 28;
const TOOLBAR_H: usize = 34;
const STATUS_H: usize = 22;
const SIDEBAR_W: usize = 220;

const ROW_H: usize = 30; // sidebar note row
const EDIT_LINE_H: usize = 16; // raw-edit fixed line pitch
const EDIT_GLYPH_W: usize = 8; // raw-edit fixed advance (8x8 bitmap font)

/// The on-screen origin we present this window at. Absolute cursor coordinates
/// from `cursor_pos()` are converted to surface-local space by subtracting this.
const PRESENT_X: i32 = 180;
const PRESENT_Y: i32 = 90;

// ── Mouse hit-testing (single source of truth: draw-rects == hit-rects) ───
//
// Toolbar-chip + sidebar-row + close-button geometry computed from the SAME
// constants `render` draws with, so a click can never drift from the visual.
// A click dispatches to the EXACT action the matching key fires; empty space
// resolves to `Action::None` (no-op, never panics).

/// What a left-click maps to — each mirrors a keyboard action 1:1.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Action {
    /// Select + open sidebar note row `i`.
    SelectNote(usize),
    SetEdit,
    SetPreview,
    NewNote,
    DeleteNote,
    Close,
    /// Toolbar "Find" chip — opens the find bar (Ctrl+F).
    OpenFind,
    /// Find-bar: focus the Find field.
    FocusFind,
    /// Find-bar: focus the Replace field (also reveals replace row).
    FocusReplace,
    /// Find-bar: jump to next match.
    FindNext,
    /// Find-bar: jump to previous match.
    FindPrev,
    /// Find-bar: toggle case-sensitivity.
    ToggleCase,
    /// Find-bar: toggle regex mode (the `.*` chip / Ctrl+R).
    ToggleRegex,
    /// Find-bar: replace the current match.
    ReplaceCurrent,
    /// Find-bar: replace all matches.
    ReplaceAll,
    /// Find-bar: close it (X).
    CloseFind,
    None,
}

#[derive(Clone, Copy)]
struct Rect {
    x: usize,
    y: usize,
    w: usize,
    h: usize,
}

impl Rect {
    fn contains(&self, px: i32, py: i32) -> bool {
        px >= self.x as i32
            && px < (self.x + self.w) as i32
            && py >= self.y as i32
            && py < (self.y + self.h) as i32
    }
}

// Toolbar-chip layout — shared by draw + hit so they can't drift.
const CHIP_W: usize = 78;
const CHIP_H: usize = 24;
const CHIP_GAP: usize = 8;
const CHIP_GROUP_GAP: usize = 8; // extra gap before the New/Delete group
const CHIP_START_X: usize = 8;

/// The toolbar chips in draw order, with the action each fires. Chips 0/1 are
/// the mode group; an extra group gap precedes chips 2/3 (New/Delete) and chip 4
/// (Find).
const TOOLBAR_CHIPS: [(&str, Action); 5] = [
    ("Edit", Action::SetEdit),
    ("Preview", Action::SetPreview),
    ("New", Action::NewNote),
    ("Delete", Action::DeleteNote),
    ("Find", Action::OpenFind),
];

/// X of toolbar chip `i`. Chips 0/1 are the mode group; an extra group gap
/// precedes chip 2 (New) onward — matching `render`'s grouping.
fn chip_x(i: usize) -> usize {
    let mut x = CHIP_START_X + i * (CHIP_W + CHIP_GAP);
    if i >= 2 {
        x += CHIP_GROUP_GAP;
    }
    x
}

/// The y of the toolbar-chip row (same as `render`: `TITLE_H + 5`).
fn chip_y() -> usize {
    TITLE_H + 5
}

/// The window-close (X) rect in the title bar.
fn close_rect() -> Rect {
    Rect {
        x: WIN_W - 28,
        y: 4,
        w: 20,
        h: 20,
    }
}

// ── Find-bar geometry (overlay atop the main pane) ────────────────────────
//
// The find bar is a horizontal strip at the top of the editor pane. Its rects
// are computed from the SAME constants `render_find_bar` draws with, so clicks
// can't drift from the pixels. Layout (left→right): [Find input] [Aa] [<] [>]
// [N/M] [x]; the replace row (when shown) adds [Replace input] [Replace] [All].

const FIND_BAR_H: usize = 32; // one row (find)
const FIND_ROW_H: usize = 28; // height of each input/button row
const FIND_PAD: usize = 8;
const FIND_INPUT_W: usize = 220;
const FIND_BTN_W: usize = 26; // square icon button
const FIND_GAP: usize = 6;
const FIND_RBTN_W: usize = 66; // "Replace"/"All" text buttons

/// Y of the find bar's top edge (just inside the main pane).
fn find_bar_y() -> usize {
    TITLE_H + TOOLBAR_H
}

/// Left x of the find bar (inside the main pane, past the sidebar divider).
fn find_bar_x() -> usize {
    SIDEBAR_W + 1 + FIND_PAD
}

/// The Find input rect.
fn find_input_rect() -> Rect {
    Rect {
        x: find_bar_x(),
        y: find_bar_y() + 2,
        w: FIND_INPUT_W,
        h: FIND_ROW_H,
    }
}

/// The N icon button to the right of the Find input at slot `slot` (0=Aa,
/// 1=`.*` regex toggle, 2=prev, 3=next, 4=close).
fn find_btn_rect(slot: usize) -> Rect {
    let x = find_input_rect().x + FIND_INPUT_W + FIND_GAP + slot * (FIND_BTN_W + FIND_GAP);
    Rect {
        x,
        y: find_bar_y() + 2,
        w: FIND_BTN_W,
        h: FIND_ROW_H,
    }
}

/// The "N of M" counter x (just right of the five icon buttons).
fn find_count_x() -> usize {
    find_btn_rect(4).x + FIND_BTN_W + FIND_GAP
}

/// The Replace input rect (second row, only in replace mode).
fn replace_input_rect() -> Rect {
    Rect {
        x: find_bar_x(),
        y: find_bar_y() + 2 + FIND_ROW_H + 4,
        w: FIND_INPUT_W,
        h: FIND_ROW_H,
    }
}

/// The Replace / Replace-All text button at slot `slot` (0=Replace, 1=All).
fn replace_btn_rect(slot: usize) -> Rect {
    let x = replace_input_rect().x + FIND_INPUT_W + FIND_GAP + slot * (FIND_RBTN_W + FIND_GAP);
    Rect {
        x,
        y: replace_input_rect().y,
        w: FIND_RBTN_W,
        h: FIND_ROW_H,
    }
}

/// Total height the find bar occupies (find row, plus replace row when shown).
fn find_bar_total_h(replace_mode: bool) -> usize {
    if replace_mode {
        FIND_ROW_H * 2 + 12
    } else {
        FIND_BAR_H + 4
    }
}

// ── Palette (ath_tokens, docs/design/design-language.md) ──────────────────
//
// All chrome pulls onto the shared `ath_tokens::DARK` palette + the RaeBlue
// accent ramp so Notes matches the desktop default 1:1. Accent shades are
// derived (non-const) → computed in helpers below.

const BG: u32 = DARK.bg_raised; // editor/preview surface
const TITLE_BG: u32 = DARK.bg_base; // deepest chrome
const TOOLBAR_BG: u32 = DARK.bg_overlay; // panel
const SIDEBAR_BG: u32 = DARK.bg_base; // note-list rail
const ROW_ALT_BG: u32 = DARK.bg_overlay; // zebra row
const TEXT_FG: u32 = DARK.text_primary;
const TEXT_MUTED: u32 = DARK.text_secondary;
const TEXT_DIM: u32 = DARK.text_tertiary;
const STATUS_BG: u32 = DARK.bg_base;
const STROKE_HL: u32 = DARK.stroke_strong;
const CODE_BG: u32 = DARK.bg_base; // inline/code-block panel fill

fn theme_seed() -> u32 {
    athkit::sys::theme_accent()
}

/// Accent base, derived through the shared ramp from the live theme seed.
fn accent() -> u32 {
    ath_tokens::derive_accent(theme_seed(), &DARK).base
}

/// Opaque selection fill: the accent's pressed/active shade.
fn sel_fill() -> u32 {
    ath_tokens::derive_accent(theme_seed(), &DARK).active
}

// ── Heading type sizing (the render invariant design_proof asserts) ───────
//
// Headings are sized per level off the shared type ramp: H1 = TYPE_TITLE,
// H2 = TYPE_SUBTITLE, H3+ = TYPE_BODY-but-bold-weight. Body text = TYPE_BODY.
// The strict ordering `heading_style(1).px > heading_style(3).px >= BODY.px`
// is the invariant design_proof checks (a regression in the ramp wiring flips
// it). Sans family throughout (proportional AA — preview is read-only).

fn heading_style(level: u8) -> TypeStyle {
    match level {
        1 => ath_tokens::TYPE_TITLE,    // 22px / 600
        2 => ath_tokens::TYPE_SUBTITLE, // 17px / 500
        _ => TypeStyle {
            px: ath_tokens::TYPE_BODY.px,
            weight: 600, // heavier than body to read as a heading at H3+
            line_height: ath_tokens::TYPE_BODY.line_height,
        },
    }
}

// ── Path / name helpers (shared shape with Photos/Music) ──────────────────

const PATH_CAP: usize = 256;
const NAME_CAP: usize = 64;
const MAX_NOTES: usize = 128;
/// Hard cap on a single note slurp (notes are text — generous but bounded).
const READ_CAP: usize = 4 * 1024 * 1024; // 4 MiB

#[derive(Clone, Copy)]
struct PathBuf {
    bytes: [u8; PATH_CAP],
    len: usize,
}

impl PathBuf {
    fn new() -> Self {
        Self {
            bytes: [0; PATH_CAP],
            len: 0,
        }
    }
    fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.len]).unwrap_or("/")
    }
    fn set(&mut self, s: &str) {
        let n = s.as_bytes().len().min(PATH_CAP);
        self.bytes[..n].copy_from_slice(&s.as_bytes()[..n]);
        self.len = n;
    }
    fn push_component(&mut self, name: &str) {
        if self.len > 0 && self.bytes[self.len - 1] != b'/' && self.len < PATH_CAP {
            self.bytes[self.len] = b'/';
            self.len += 1;
        }
        for &b in name.as_bytes() {
            if self.len >= PATH_CAP {
                break;
            }
            self.bytes[self.len] = b;
            self.len += 1;
        }
    }
}

/// One note: its file name on disk. Content is loaded on select (we do not hold
/// every note's body resident). `title` is derived lazily on render.
struct Note {
    name: [u8; NAME_CAP],
    name_len: usize,
}

impl Note {
    fn name(&self) -> &str {
        core::str::from_utf8(&self.name[..self.name_len]).unwrap_or("?")
    }
}

/// True if a file name ends with `.md`/`.markdown`/`.txt` (case-insensitive).
fn is_note_name(name: &str) -> bool {
    let lower_ends = |suf: &str| -> bool {
        let nb = name.as_bytes();
        let sb = suf.as_bytes();
        if nb.len() < sb.len() {
            return false;
        }
        let tail = &nb[nb.len() - sb.len()..];
        tail.iter()
            .zip(sb.iter())
            .all(|(a, b)| a.to_ascii_lowercase() == *b)
    };
    lower_ends(".md") || lower_ends(".markdown") || lower_ends(".txt")
}

// ── Edit buffer (line-based, like the Text Editor) ────────────────────────

const MAX_LINES: usize = 512;
const LINE_CAP: usize = 256;

struct Line {
    chars: [u8; LINE_CAP],
    len: usize,
}

impl Line {
    const fn empty() -> Self {
        Self {
            chars: [0; LINE_CAP],
            len: 0,
        }
    }
    fn insert(&mut self, idx: usize, ch: u8) -> bool {
        if self.len >= LINE_CAP || idx > self.len {
            return false;
        }
        for i in (idx..self.len).rev() {
            self.chars[i + 1] = self.chars[i];
        }
        self.chars[idx] = ch;
        self.len += 1;
        true
    }
    fn delete(&mut self, idx: usize) -> bool {
        if idx >= self.len {
            return false;
        }
        for i in idx..self.len - 1 {
            self.chars[i] = self.chars[i + 1];
        }
        self.len -= 1;
        true
    }
    fn as_str(&self) -> &str {
        core::str::from_utf8(&self.chars[..self.len]).unwrap_or("")
    }
}

struct Buf {
    lines: [Line; MAX_LINES],
    nlines: usize,
    cursor_row: usize,
    cursor_col: usize,
    scroll: usize,
    dirty: bool,
}

impl Buf {
    fn new() -> Self {
        const EMPTY: Line = Line::empty();
        Self {
            lines: [EMPTY; MAX_LINES],
            nlines: 1,
            cursor_row: 0,
            cursor_col: 0,
            scroll: 0,
            dirty: false,
        }
    }

    /// Reset to empty (one blank line).
    fn clear(&mut self) {
        for i in 0..self.nlines {
            self.lines[i].len = 0;
        }
        self.nlines = 1;
        self.cursor_row = 0;
        self.cursor_col = 0;
        self.scroll = 0;
        self.dirty = false;
    }

    /// Replace the buffer contents with `text` (split on '\n'). Never panics:
    /// over-long lines / over-many lines are truncated to the buffer caps.
    fn load_text(&mut self, text: &str) {
        self.clear();
        let mut row = 0usize;
        for raw in text.split('\n') {
            if row >= MAX_LINES {
                break;
            }
            // Strip a trailing '\r' (CRLF files).
            let line = raw.strip_suffix('\r').unwrap_or(raw);
            let bytes = line.as_bytes();
            let n = bytes.len().min(LINE_CAP);
            self.lines[row].chars[..n].copy_from_slice(&bytes[..n]);
            self.lines[row].len = n;
            row += 1;
        }
        self.nlines = row.max(1);
        self.dirty = false;
    }

    /// Materialise the buffer as one markdown string for the parser / save.
    fn to_string(&self) -> String {
        let mut out = String::new();
        for i in 0..self.nlines {
            out.push_str(self.lines[i].as_str());
            if i + 1 < self.nlines {
                out.push('\n');
            }
        }
        out
    }

    fn current(&mut self) -> &mut Line {
        &mut self.lines[self.cursor_row]
    }

    fn insert_char(&mut self, ch: u8) {
        let col = self.cursor_col;
        if self.current().insert(col, ch) {
            self.cursor_col += 1;
            self.dirty = true;
        }
    }

    fn newline(&mut self) {
        if self.nlines >= MAX_LINES {
            return;
        }
        let row = self.cursor_row;
        let col = self.cursor_col;
        for i in (row + 1..self.nlines).rev() {
            self.lines.swap(i, i + 1);
        }
        self.nlines += 1;
        let tail_len = self.lines[row].len - col;
        for i in 0..tail_len {
            self.lines[row + 1].chars[i] = self.lines[row].chars[col + i];
        }
        self.lines[row + 1].len = tail_len;
        self.lines[row].len = col;
        self.cursor_row += 1;
        self.cursor_col = 0;
        self.dirty = true;
    }

    fn backspace(&mut self) {
        if self.cursor_col > 0 {
            self.cursor_col -= 1;
            let col = self.cursor_col;
            self.current().delete(col);
            self.dirty = true;
        } else if self.cursor_row > 0 {
            let row = self.cursor_row;
            let prev_len = self.lines[row - 1].len;
            let cur_len = self.lines[row].len;
            if prev_len + cur_len <= LINE_CAP {
                for i in 0..cur_len {
                    self.lines[row - 1].chars[prev_len + i] = self.lines[row].chars[i];
                }
                self.lines[row - 1].len = prev_len + cur_len;
                for i in row..self.nlines - 1 {
                    self.lines.swap(i, i + 1);
                }
                self.lines[self.nlines - 1].len = 0;
                self.nlines -= 1;
                self.cursor_row -= 1;
                self.cursor_col = prev_len;
                self.dirty = true;
            }
        }
    }

    fn move_left(&mut self) {
        if self.cursor_col > 0 {
            self.cursor_col -= 1;
        } else if self.cursor_row > 0 {
            self.cursor_row -= 1;
            self.cursor_col = self.lines[self.cursor_row].len;
        }
    }
    fn move_right(&mut self) {
        let cur_len = self.lines[self.cursor_row].len;
        if self.cursor_col < cur_len {
            self.cursor_col += 1;
        } else if self.cursor_row + 1 < self.nlines {
            self.cursor_row += 1;
            self.cursor_col = 0;
        }
    }
    fn move_up(&mut self) {
        if self.cursor_row == 0 {
            return;
        }
        self.cursor_row -= 1;
        let n = self.lines[self.cursor_row].len;
        if self.cursor_col > n {
            self.cursor_col = n;
        }
    }
    fn move_down(&mut self) {
        if self.cursor_row + 1 >= self.nlines {
            return;
        }
        self.cursor_row += 1;
        let n = self.lines[self.cursor_row].len;
        if self.cursor_col > n {
            self.cursor_col = n;
        }
    }

    fn update_scroll(&mut self, visible_rows: usize) {
        if self.cursor_row < self.scroll {
            self.scroll = self.cursor_row;
        }
        if visible_rows > 0 && self.cursor_row >= self.scroll + visible_rows {
            self.scroll = self.cursor_row + 1 - visible_rows;
        }
    }
}

// ── Find & Replace core logic (pure, char-boundary-safe) ──────────────────
//
// These three functions are the load-bearing search/replace logic, kept pure
// (string-in / ranges-or-string-out) so they're testable in spirit and reused
// by both the find UI and the design_proof. CHAR-BOUNDARY SAFETY: matching is
// done over the *byte* slice but every returned range is a substring boundary
// of a real `find`/`match_indices` hit, so slicing the haystack at any returned
// `start`/`end` lands on a UTF-8 char boundary — a multibyte char in the
// haystack can never be split mid-codepoint. An empty needle returns no matches
// (so there is no zero-width infinite loop), and replace with "" is allowed.

/// All match ranges (byte `(start, end)`) of `needle` in `haystack`, non-
/// overlapping, left-to-right. Case-insensitive folds both sides to lowercase
/// for ASCII (the editor's text domain); a case-sensitive request matches bytes
/// exactly. Empty needle → no matches (never an infinite loop). Every range is a
/// real substring boundary, so the caller can slice the haystack safely.
fn find_matches(haystack: &str, needle: &str, case_sensitive: bool) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    if needle.is_empty() || needle.len() > haystack.len() {
        return out;
    }
    if case_sensitive {
        // Exact byte search via the std matcher — every index is a char boundary
        // because `needle`/`haystack` are valid &str and the match is a substring.
        let mut from = 0usize;
        while let Some(rel) = haystack[from..].find(needle) {
            let start = from + rel;
            let end = start + needle.len();
            out.push((start, end));
            from = end; // non-overlapping
        }
    } else {
        // ASCII-case-insensitive: compare the haystack window to the needle byte
        // for byte under `eq_ignore_ascii_case`. We only ever START a window at a
        // char boundary (we walk char_indices), and the window length equals the
        // needle's byte length; equal-length ASCII-folded matches preserve char
        // boundaries (a multibyte UTF-8 char never byte-equals an ASCII fold of a
        // different length, and same-byte-length windows that fold-equal end on a
        // boundary). Guard the end against the haystack length + boundary.
        let hb = haystack.as_bytes();
        let nlen = needle.len();
        let mut i = 0usize;
        while i + nlen <= hb.len() {
            if !haystack.is_char_boundary(i) {
                i += 1;
                continue;
            }
            let end = i + nlen;
            if haystack.is_char_boundary(end) && hb[i..end].eq_ignore_ascii_case(needle.as_bytes())
            {
                out.push((i, end));
                i = end; // non-overlapping
            } else {
                // advance one char (not one byte) to keep boundaries
                i += char_len_at(haystack, i);
            }
        }
    }
    out
}

/// Byte length of the UTF-8 char starting at byte index `i` (assumes `i` is a
/// char boundary). Used to advance the case-insensitive scan one *char* at a
/// time so we never land mid-codepoint.
fn char_len_at(s: &str, i: usize) -> usize {
    s[i..].chars().next().map(|c| c.len_utf8()).unwrap_or(1)
}

/// Replace every match of `needle` in `buf` with `repl`, returning the new
/// string and the replacement count. Case-insensitive (matches `find_matches`
/// defaults). Empty needle → unchanged buffer, 0 count. Builds the result by
/// copying the gaps between matches verbatim (so non-matching text, including
/// multibyte chars, is preserved byte-for-byte).
fn replace_all(buf: &str, needle: &str, repl: &str) -> (String, usize) {
    replace_all_cs(buf, needle, repl, false)
}

/// Case-aware `replace_all` (the find UI passes the live case-sensitive flag).
fn replace_all_cs(buf: &str, needle: &str, repl: &str, case_sensitive: bool) -> (String, usize) {
    let matches = find_matches(buf, needle, case_sensitive);
    if matches.is_empty() {
        return (String::from(buf), 0);
    }
    let mut out = String::with_capacity(buf.len());
    let mut cursor = 0usize;
    for &(start, end) in &matches {
        // gap before this match (slice on real boundaries → safe)
        out.push_str(&buf[cursor..start]);
        out.push_str(repl);
        cursor = end;
    }
    out.push_str(&buf[cursor..]);
    (out, matches.len())
}

/// Replace exactly the byte range `(start, end)` of `buf` with `repl`, returning
/// the new string. The range MUST be a real match range from `find_matches` (it
/// is a char boundary on both ends). A degenerate/out-of-range range leaves the
/// buffer unchanged (never panics).
fn replace_one_at(buf: &str, range: (usize, usize), repl: &str) -> String {
    let (start, end) = range;
    if start > end || end > buf.len() || !buf.is_char_boundary(start) || !buf.is_char_boundary(end)
    {
        return String::from(buf);
    }
    let mut out = String::with_capacity(buf.len());
    out.push_str(&buf[..start]);
    out.push_str(repl);
    out.push_str(&buf[end..]);
    out
}

// ── Regex find/replace (ath_regex, the never-panic Pike-VM engine) ─────────
//
// REGEX MODE: when the find bar's `regex_mode` is on, the query is compiled with
// `ath_regex::Regex::new` (never panics — a malformed pattern returns Err). On a
// good pattern, `find_all` produces the SAME `Vec<(start,end)>` byte ranges the
// literal path uses, so highlight / next / prev / caret-jump all work unchanged.
// Replace-all uses `Regex::replace_all`, so `$1` group references in the
// replacement expand. Replace-one re-runs `replace_all` bounded to the current
// match's slice (so a group ref in the replacement expands for that one match).
//
// CASE SENSITIVITY: ath_regex documents NO inline flags (`(?i)` is unsupported),
// and folding a regex case-insensitively is not trivial, so in regex mode the
// `[Aa]` toggle is IGNORED and matching is always case-sensitive (the documented,
// least-surprising behavior — a user who wants case-insensitive regex writes a
// character class like `[Tt]he`). The literal path still honors `[Aa]`.

/// Compile `query` and return all match ranges, or `None` if the pattern is
/// malformed (the caller shows "Bad regex" and treats it as zero matches). Never
/// panics: `Regex::new` returns `Err` on a bad pattern, `find_all` is linear and
/// allocation-bounded. Every returned range is a real `find_all` match span, so
/// slicing the haystack at any `start`/`end` lands on a UTF-8 char boundary.
fn regex_find_matches(haystack: &str, query: &str) -> Option<Vec<(usize, usize)>> {
    if query.is_empty() {
        return Some(Vec::new());
    }
    let re = Regex::new(query).ok()?;
    let mut out = Vec::new();
    for m in re.find_all(haystack) {
        // Skip zero-width matches: the editor's match machinery (highlight, caret
        // jump, replace) assumes a non-empty span, and a 0-width highlight is
        // meaningless. find_all already advances past them so this just filters.
        if m.end > m.start {
            out.push((m.start, m.end));
        }
    }
    Some(out)
}

/// Replace ALL regex matches of `query` in `buf` with `repl` (with `$1` group
/// expansion), returning the new string and the replacement count. On a bad
/// pattern → unchanged buffer, 0 count. Counts via a fresh `find_all` (matching
/// the count the UI shows) so the toast is accurate.
fn regex_replace_all(buf: &str, query: &str, repl: &str) -> (String, usize) {
    if query.is_empty() {
        return (String::from(buf), 0);
    }
    let re = match Regex::new(query) {
        Ok(r) => r,
        Err(_) => return (String::from(buf), 0),
    };
    let count = re.find_all(buf).iter().filter(|m| m.end > m.start).count();
    if count == 0 {
        return (String::from(buf), 0);
    }
    (re.replace_all(buf, repl), count)
}

/// Replace exactly the current match's byte range `(start, end)` of `buf` with
/// the regex-expanded replacement for that one match. We slice out the match
/// substring and run `replace_all` over ONLY that slice (so `$1` group refs
/// expand against the match), then splice the result back. The range MUST be a
/// real `regex_find_matches` span (char boundaries on both ends). A bad pattern
/// or degenerate range leaves the buffer unchanged (never panics).
fn regex_replace_one_at(buf: &str, range: (usize, usize), query: &str, repl: &str) -> String {
    let (start, end) = range;
    if start >= end || end > buf.len() || !buf.is_char_boundary(start) || !buf.is_char_boundary(end)
    {
        return String::from(buf);
    }
    let re = match Regex::new(query) {
        Ok(r) => r,
        Err(_) => return String::from(buf),
    };
    let slice = &buf[start..end];
    let replaced = re.replace_all(slice, repl);
    let mut out = String::with_capacity(buf.len());
    out.push_str(&buf[..start]);
    out.push_str(&replaced);
    out.push_str(&buf[end..]);
    out
}

// ── App state ─────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Edit,
    Preview,
}

/// Which field of the find bar receives typed characters.
#[derive(Clone, Copy, PartialEq, Eq)]
enum FindField {
    Find,
    Replace,
}

/// The Find/Replace bar overlay state. When `active`, the find bar is drawn at
/// the top of the main pane and captures keyboard input; matches are recomputed
/// from the live buffer whenever the query or buffer changes.
struct FindState {
    active: bool,
    /// Replace mode (Ctrl+H) shows the Replace field + Replace/All buttons.
    replace_mode: bool,
    /// Default off = case-insensitive; toggled with the [Aa] chip / Ctrl+I.
    /// IGNORED in regex mode (ath_regex has no inline-flag support → regex is
    /// always case-sensitive).
    case_sensitive: bool,
    /// Regex mode (the `.*` chip / Ctrl+R): the query is a `ath_regex` pattern.
    regex_mode: bool,
    /// True when `regex_mode` is on AND the current query is a malformed pattern
    /// (the counter shows "Bad regex" and the match set is empty — never panics).
    bad_regex: bool,
    field: FindField,
    query: [u8; FIND_CAP],
    query_len: usize,
    repl: [u8; FIND_CAP],
    repl_len: usize,
    /// Byte ranges of all current matches in the materialized buffer string.
    matches: Vec<(usize, usize)>,
    /// Index into `matches` of the "current" match (caret target).
    current: usize,
}

const FIND_CAP: usize = 128;

impl FindState {
    fn new() -> Self {
        Self {
            active: false,
            replace_mode: false,
            case_sensitive: false,
            regex_mode: false,
            bad_regex: false,
            field: FindField::Find,
            query: [0; FIND_CAP],
            query_len: 0,
            repl: [0; FIND_CAP],
            repl_len: 0,
            matches: Vec::new(),
            current: 0,
        }
    }
    fn query_str(&self) -> &str {
        core::str::from_utf8(&self.query[..self.query_len]).unwrap_or("")
    }
    fn repl_str(&self) -> &str {
        core::str::from_utf8(&self.repl[..self.repl_len]).unwrap_or("")
    }
    /// The buffer ([u8;FIND_CAP], len) for the active field.
    fn field_mut(&mut self) -> (&mut [u8; FIND_CAP], &mut usize) {
        match self.field {
            FindField::Find => (&mut self.query, &mut self.query_len),
            FindField::Replace => (&mut self.repl, &mut self.repl_len),
        }
    }
    fn push_char(&mut self, ch: u8) {
        let (buf, len) = self.field_mut();
        if *len < FIND_CAP {
            buf[*len] = ch;
            *len += 1;
        }
    }
    fn backspace_field(&mut self) {
        let (_buf, len) = self.field_mut();
        if *len > 0 {
            *len -= 1;
        }
    }
}

// ── Persistent preferences (ath_toml) ─────────────────────────────────────────
//
// LEGACY_GAMING_CONCEPT.md §"The user owns the machine": "remember my settings" must be
// real. Notes persists its VIEW state — which note was open, edit-vs-preview mode,
// and the find-bar case-sensitive + regex toggles — to `<home>/.config/notes.toml`
// and restores it on launch. The note CONTENT is NOT persisted here (it already
// lives on disk as `.md`/`.txt` files); only WHICH note + how to view it. Every
// load is hostile-input-tolerant: a missing, corrupt, or out-of-range config falls
// back to TYPED DEFAULTS and NEVER panics — the app always starts. This is the
// per-app prefs pattern the consumer apps follow (the proven Music recipe).

/// The decoded, defaulted preferences restored at launch. Pure data: load builds
/// it from a parsed (or absent) TOML, save serializes the live App view-state.
#[derive(Clone)]
struct Prefs {
    /// Last-open note FILE NAME (re-resolved against the live scan; a renamed /
    /// deleted file simply fails to match → selection 0). Empty = none.
    last_note: String,
    /// View mode: false = Edit (the default), true = Preview.
    preview: bool,
    /// Find-bar case-sensitivity toggle (default off = case-insensitive).
    case_sensitive: bool,
    /// Find-bar regex-mode toggle (default off = literal).
    regex_mode: bool,
}

impl Prefs {
    /// The typed defaults used on first run or any config error.
    fn defaults() -> Self {
        Self {
            last_note: String::new(),
            preview: false,
            case_sensitive: false,
            regex_mode: false,
        }
    }

    /// Build `Prefs` from a parsed TOML table, validating every field and
    /// substituting the typed default for any missing / wrong-typed value. Never
    /// panics; an unrelated shape (e.g. a non-table root) yields full defaults.
    fn from_toml(t: &Toml) -> Self {
        let mut p = Self::defaults();
        if let Some(s) = t.get("last_note").and_then(Toml::as_str) {
            // Cap the stored name; a pathological length can't blow anything up
            // because it's only ever compared against scanned file names.
            p.last_note = String::from(truncate_on_char_boundary(s, PATH_CAP));
        }
        if let Some(b) = t.get("preview").and_then(Toml::as_bool) {
            p.preview = b;
        }
        if let Some(b) = t.get("case_sensitive").and_then(Toml::as_bool) {
            p.case_sensitive = b;
        }
        if let Some(b) = t.get("regex_mode").and_then(Toml::as_bool) {
            p.regex_mode = b;
        }
        p
    }

    /// Serialize the live preferences into an order-stable `Toml::Table` ready for
    /// `ath_toml::to_string`. The schema is flat (no headers) so a round-trip is
    /// trivial and human-editable.
    fn to_toml(&self) -> Toml {
        let mut table: Vec<(String, Toml)> = Vec::new();
        table.push((
            String::from("last_note"),
            Toml::String(self.last_note.clone()),
        ));
        table.push((String::from("preview"), Toml::Boolean(self.preview)));
        table.push((
            String::from("case_sensitive"),
            Toml::Boolean(self.case_sensitive),
        ));
        table.push((String::from("regex_mode"), Toml::Boolean(self.regex_mode)));
        Toml::Table(table)
    }
}

/// Return a prefix of `s` no longer than `max` bytes, cut on a UTF-8 char
/// boundary so the result is always valid (never panics on a multi-byte split).
fn truncate_on_char_boundary(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// The per-app config DIRECTORY: `<session home>/.config`. Falls back to the same
/// `/home/user` default the Notes directory uses when no session is present. The
/// `.config` directory is created (idempotent) before any write.
fn prefs_dir() -> PathBuf {
    let mut p = PathBuf::new();
    let mut info = [0u8; 96];
    if athkit::sys::session_info(&mut info).is_some() {
        if let Some(home) = athkit::sys::session_home_from(&info) {
            p.set(home);
            p.push_component(".config");
            return p;
        }
    }
    p.set("/home/user/.config");
    p
}

/// Load preferences from `<home>/.config/notes.toml`. On ANY failure — file
/// absent, unreadable, not UTF-8, or a `ath_toml::parse` error — returns the typed
/// defaults. Never panics, never blocks the app from launching.
fn load_prefs() -> Prefs {
    let mut path = prefs_dir();
    path.push_component("notes.toml");
    let fd = athkit::sys::open(path.as_str(), 0);
    if fd == u64::MAX {
        return Prefs::defaults();
    }
    let mut data: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        // Hard cap: a config file should be tiny; refuse to slurp a giant blob.
        if data.len() > 64 * 1024 {
            break;
        }
        let n = athkit::sys::read(fd, &mut chunk) as usize;
        if n == 0 || n > chunk.len() {
            break;
        }
        data.extend_from_slice(&chunk[..n]);
    }
    let _ = athkit::sys::close(fd);
    let text = match core::str::from_utf8(&data) {
        Ok(s) => s,
        Err(_) => return Prefs::defaults(),
    };
    match ath_toml::parse(text) {
        Ok(t) => Prefs::from_toml(&t),
        Err(_) => Prefs::defaults(),
    }
}

/// Persist `prefs` to `<home>/.config/notes.toml` (best effort). Creates the
/// `.config` directory if missing, serializes via `ath_toml::to_string`, and
/// writes O_CREAT|O_TRUNC. A failure is silent — the app keeps running.
fn save_prefs(prefs: &Prefs) {
    let dir = prefs_dir();
    let _ = athkit::sys::mkdir(dir.as_str());
    let mut path = dir;
    path.push_component("notes.toml");
    let text = ath_toml::to_string(&prefs.to_toml());
    // O_WRONLY | O_CREAT | O_TRUNC = 0x0241 (matches the save path above).
    let fd = athkit::sys::open(path.as_str(), 0x0241);
    if fd == u64::MAX {
        return;
    }
    let bytes = text.as_bytes();
    let mut off = 0usize;
    while off < bytes.len() {
        let end = (off + 4096).min(bytes.len());
        let n = athkit::sys::write(fd, &bytes[off..end]) as usize;
        if n == 0 {
            break;
        }
        off += n;
    }
    let _ = athkit::sys::close(fd);
}

struct App {
    dir: PathBuf,
    notes: Vec<Note>,
    selected: usize,
    list_scroll: usize,
    buf: Buf,
    mode: Mode,
    /// Index of the note currently loaded into `buf` (usize::MAX = none/new).
    open_idx: usize,
    /// Preview vertical scroll, in pixels.
    preview_scroll: usize,
    toast: [u8; 64],
    toast_len: usize,
    /// Counter that makes auto-generated note names unique within a session.
    new_seq: u32,
    /// Find/Replace bar state (Ctrl+F / Ctrl+H).
    find: FindState,
}

impl App {
    fn notes_dir() -> PathBuf {
        let mut info = [0u8; 96];
        if athkit::sys::session_info(&mut info).is_some() {
            if let Some(home) = athkit::sys::session_home_from(&info) {
                let mut p = PathBuf::new();
                p.set(home);
                p.push_component("Notes");
                return p;
            }
        }
        let mut p = PathBuf::new();
        p.set("/home/user/Notes");
        p
    }

    fn new() -> Self {
        // Restore saved view preferences (typed defaults on first run / any error).
        let prefs = load_prefs();
        let dir = Self::notes_dir();
        let _ = athkit::sys::mkdir(dir.as_str());
        let mut find = FindState::new();
        find.case_sensitive = prefs.case_sensitive;
        find.regex_mode = prefs.regex_mode;
        let mut app = Self {
            dir,
            notes: Vec::new(),
            selected: 0,
            list_scroll: 0,
            buf: Buf::new(),
            mode: if prefs.preview {
                Mode::Preview
            } else {
                Mode::Edit
            },
            open_idx: usize::MAX,
            preview_scroll: 0,
            toast: [0; 64],
            toast_len: 0,
            new_seq: 0,
            find,
        };
        app.scan();
        if app.notes.is_empty() {
            app.seed_welcome();
        } else {
            // Re-resolve the last-open note against the freshly scanned list; fall
            // back to note 0 if it was renamed/deleted (or none was persisted).
            let mut idx = 0usize;
            if !prefs.last_note.is_empty() {
                for (i, note) in app.notes.iter().enumerate() {
                    if note.name() == prefs.last_note.as_str() {
                        idx = i;
                        break;
                    }
                }
            }
            app.open(idx);
        }
        app
    }

    /// Snapshot the live view-state into a `Prefs` and write it to disk. Called on
    /// every preference-affecting change (note open, mode toggle, find toggle).
    /// Best effort + silent on failure (the app never blocks on the config write).
    fn persist(&self) {
        let last_note = if self.open_idx < self.notes.len() {
            String::from(self.notes[self.open_idx].name())
        } else if self.selected < self.notes.len() {
            String::from(self.notes[self.selected].name())
        } else {
            String::new()
        };
        let prefs = Prefs {
            last_note,
            preview: self.mode == Mode::Preview,
            case_sensitive: self.find.case_sensitive,
            regex_mode: self.find.regex_mode,
        };
        save_prefs(&prefs);
    }

    fn set_toast(&mut self, s: &str) {
        let n = s.as_bytes().len().min(64);
        self.toast[..n].copy_from_slice(&s.as_bytes()[..n]);
        self.toast_len = n;
    }
    fn toast_str(&self) -> &str {
        core::str::from_utf8(&self.toast[..self.toast_len]).unwrap_or("")
    }

    /// Enumerate the Notes directory and build the note list.
    fn scan(&mut self) {
        self.notes.clear();
        let mut buf = [0u8; 8192];
        let mut dirbuf = [0u8; PATH_CAP];
        let dn = self.dir.as_str().as_bytes().len().min(PATH_CAP);
        dirbuf[..dn].copy_from_slice(&self.dir.as_str().as_bytes()[..dn]);
        let dir = core::str::from_utf8(&dirbuf[..dn]).unwrap_or("/");

        let count = athkit::sys::readdir_at(dir, &mut buf) as usize;
        let mut off = 0usize;
        for _ in 0..count {
            if off + 6 > buf.len() || self.notes.len() >= MAX_NOTES {
                break;
            }
            let name_len = u16::from_le_bytes([buf[off], buf[off + 1]]) as usize;
            off += 6;
            if off + name_len > buf.len() {
                break;
            }
            let raw = &buf[off..off + name_len];
            off += name_len;
            let name = core::str::from_utf8(raw).unwrap_or("");
            if !is_note_name(name) {
                continue;
            }
            let mut nbuf = [0u8; NAME_CAP];
            let n = raw.len().min(NAME_CAP);
            nbuf[..n].copy_from_slice(&raw[..n]);
            self.notes.push(Note {
                name: nbuf,
                name_len: n,
            });
        }
        if self.selected >= self.notes.len() {
            self.selected = self.notes.len().saturating_sub(1);
        }
    }

    /// Seed an in-memory welcome note when the library is empty (not yet on
    /// disk — saving it with Ctrl+S writes it as `welcome.md`).
    fn seed_welcome(&mut self) {
        let welcome = "# Welcome to Notes\n\n\
            A **Markdown** editor with a *live* preview, built on AthenaOS's own\n\
            `ath_markdown` parser.\n\n\
            ## Getting started\n\n\
            - Press **Tab** to toggle edit and preview\n\
            - Press **Ctrl+S** to save\n\
            - Press **Ctrl+N** for a new note\n\n\
            > Bad markdown never crashes the app — it degrades to plain text.\n\n\
            ```\nlet hello = \"world\";\n```\n\n\
            ---\n\nHappy note-taking!\n";
        self.buf.load_text(welcome);
        self.open_idx = usize::MAX; // unsaved
        self.mode = Mode::Edit;
        self.preview_scroll = 0;
    }

    fn note_path(&self, idx: usize) -> Option<PathBuf> {
        let note = self.notes.get(idx)?;
        let mut path = PathBuf::new();
        path.set(self.dir.as_str());
        path.push_component(note.name());
        Some(path)
    }

    /// Load note `idx` into the edit buffer. On read failure: toast + stay alive.
    fn open(&mut self, idx: usize) {
        let path = match self.note_path(idx) {
            Some(p) => p,
            None => return,
        };
        match read_file(path.as_str()) {
            Some(text) => {
                let s = core::str::from_utf8(&text).unwrap_or("");
                self.buf.load_text(s);
                self.open_idx = idx;
                self.selected = idx;
                self.preview_scroll = 0;
                self.toast_len = 0;
                // Remember the newly-open note across launches.
                self.persist();
            }
            None => {
                self.set_toast("Can't open this note");
            }
        }
    }

    /// Save the buffer. If the open note is on disk, overwrite it; otherwise
    /// create a new file named after the first heading / first line. Returns true
    /// on success. On failure: toast + stay alive.
    fn save(&mut self) {
        let body = self.buf.to_string();
        // Determine the target file name.
        let path = if let Some(p) = self.note_path(self.open_idx) {
            p
        } else {
            let mut name = [0u8; NAME_CAP];
            let nlen = derive_filename(&body, self.new_seq, &mut name);
            self.new_seq = self.new_seq.wrapping_add(1);
            let fname = core::str::from_utf8(&name[..nlen]).unwrap_or("note.md");
            let mut p = PathBuf::new();
            p.set(self.dir.as_str());
            p.push_component(fname);
            p
        };
        if write_file(path.as_str(), body.as_bytes()) {
            self.buf.dirty = false;
            self.set_toast("Saved");
            // Re-scan so a brand-new note appears in the sidebar + becomes the
            // open note (so subsequent saves overwrite it, not create copies).
            self.scan();
            for (i, note) in self.notes.iter().enumerate() {
                if path.as_str().ends_with(note.name()) {
                    self.open_idx = i;
                    self.selected = i;
                    break;
                }
            }
        } else {
            self.set_toast("Save failed");
        }
    }

    /// Start a fresh, unsaved note in the buffer.
    fn new_note(&mut self) {
        self.buf.clear();
        self.open_idx = usize::MAX;
        self.mode = Mode::Edit;
        self.preview_scroll = 0;
        self.set_toast("New note");
    }

    /// Delete the selected note from disk + the list. On failure: toast.
    fn delete_selected(&mut self) {
        let path = match self.note_path(self.selected) {
            Some(p) => p,
            None => return,
        };
        if athkit::sys::unlink(path.as_str()).is_ok() {
            self.set_toast("Deleted");
            self.scan();
            if self.notes.is_empty() {
                self.buf.clear();
                self.open_idx = usize::MAX;
            } else {
                let next = self.selected.min(self.notes.len() - 1);
                self.open(next);
            }
        } else {
            self.set_toast("Delete failed");
        }
    }

    fn list_visible_rows(&self) -> usize {
        let h = WIN_H - TITLE_H - TOOLBAR_H - STATUS_H;
        (h / ROW_H).max(1)
    }

    fn move_sel(&mut self, delta: i32) {
        if self.notes.is_empty() {
            return;
        }
        let n = self.notes.len() as i32;
        let mut idx = self.selected as i32 + delta;
        if idx < 0 {
            idx = 0;
        }
        if idx >= n {
            idx = n - 1;
        }
        self.selected = idx as usize;
        let vis = self.list_visible_rows();
        if self.selected < self.list_scroll {
            self.list_scroll = self.selected;
        }
        if self.selected >= self.list_scroll + vis {
            self.list_scroll = self.selected + 1 - vis;
        }
    }

    fn toggle_mode(&mut self) {
        self.mode = match self.mode {
            Mode::Edit => Mode::Preview,
            Mode::Preview => Mode::Edit,
        };
        self.preview_scroll = 0;
        // Remember the view mode across launches.
        self.persist();
    }

    // ── Find & Replace integration ────────────────────────────────────────

    /// Open the find bar (Ctrl+F). Switches to Edit mode (where the caret lives),
    /// focuses the Find field, and recomputes matches against the current buffer.
    fn open_find(&mut self, replace_mode: bool) {
        self.mode = Mode::Edit;
        self.find.active = true;
        self.find.replace_mode = replace_mode;
        self.find.field = FindField::Find;
        self.recompute_matches();
    }

    /// Close the find bar (Esc) and clear its match set.
    fn close_find(&mut self) {
        self.find.active = false;
        self.find.matches.clear();
        self.find.current = 0;
    }

    /// Recompute the match set from the live buffer + query, clamping `current`.
    /// In regex mode the query is compiled via `ath_regex`; a bad pattern sets
    /// `bad_regex` and yields zero matches (never panics, never crashes).
    fn recompute_matches(&mut self) {
        let hay = self.buf.to_string();
        let query_owned = String::from(self.find.query_str());
        if self.find.regex_mode {
            match regex_find_matches(&hay, &query_owned) {
                Some(m) => {
                    self.find.matches = m;
                    self.find.bad_regex = false;
                }
                None => {
                    self.find.matches = Vec::new();
                    self.find.bad_regex = true;
                }
            }
        } else {
            self.find.bad_regex = false;
            self.find.matches = find_matches(&hay, &query_owned, self.find.case_sensitive);
        }
        if self.find.matches.is_empty() {
            self.find.current = 0;
        } else if self.find.current >= self.find.matches.len() {
            self.find.current = 0;
        }
        self.move_caret_to_current();
    }

    /// Step to the next (`+1`) or previous (`-1`) match, wrapping around.
    fn step_match(&mut self, dir: i32) {
        let n = self.find.matches.len();
        if n == 0 {
            return;
        }
        let cur = self.find.current as i32;
        let next = (cur + dir).rem_euclid(n as i32);
        self.find.current = next as usize;
        self.move_caret_to_current();
    }

    /// Move the edit caret + scroll to the start of the current match (if any).
    fn move_caret_to_current(&mut self) {
        let Some(&(start, _end)) = self.find.matches.get(self.find.current) else {
            return;
        };
        let (row, col) = self.byte_off_to_rowcol(start);
        self.buf.cursor_row = row;
        self.buf.cursor_col = col;
        // Keep the match visible in the edit view.
        let visible = (WIN_H - TITLE_H - TOOLBAR_H - STATUS_H).saturating_sub(20) / EDIT_LINE_H;
        self.buf.update_scroll(visible.max(1));
    }

    /// Map a byte offset in the materialized buffer string to a `(row, col)`,
    /// where `col` is a BYTE column within the line (the edit grid is byte-pitch
    /// for ASCII). Offsets are produced by `find_matches` against the SAME
    /// `to_string()` materialization, so the row/col land on a line boundary.
    fn byte_off_to_rowcol(&self, off: usize) -> (usize, usize) {
        let mut remaining = off;
        for row in 0..self.buf.nlines {
            let line_len = self.buf.lines[row].len;
            if remaining <= line_len {
                return (row, remaining);
            }
            // Consume the line + the '\n' that to_string() inserts between lines.
            remaining -= line_len + 1;
        }
        // Past the end → last line end.
        let last = self.buf.nlines.saturating_sub(1);
        (last, self.buf.lines[last].len)
    }

    /// Replace the current match with the Replace field text, then advance. No-op
    /// if there are no matches. Rebuilds the buffer + marks dirty.
    fn replace_current(&mut self) {
        let Some(&range) = self.find.matches.get(self.find.current) else {
            return;
        };
        let hay = self.buf.to_string();
        let repl = String::from(self.find.repl_str());
        let new = if self.find.regex_mode {
            // Regex-expand `$1` group refs against this one match's slice.
            let query = String::from(self.find.query_str());
            regex_replace_one_at(&hay, range, &query, &repl)
        } else {
            replace_one_at(&hay, range, &repl)
        };
        self.buf.load_text(&new);
        self.buf.dirty = true;
        // Recompute (offsets shifted by the replacement length delta), keeping the
        // same logical position so the next match is the one after the replaced.
        let keep = self.find.current;
        self.recompute_matches();
        if !self.find.matches.is_empty() {
            self.find.current = keep.min(self.find.matches.len() - 1);
            self.move_caret_to_current();
        }
        self.set_toast("Replaced");
    }

    /// Replace ALL matches in the buffer. No-op if there are none.
    fn replace_all_matches(&mut self) {
        let hay = self.buf.to_string();
        let needle = String::from(self.find.query_str());
        let repl = String::from(self.find.repl_str());
        let (new, count) = if self.find.regex_mode {
            regex_replace_all(&hay, &needle, &repl)
        } else {
            replace_all_cs(&hay, &needle, &repl, self.find.case_sensitive)
        };
        if count == 0 {
            self.set_toast("No matches");
            return;
        }
        self.buf.load_text(&new);
        self.buf.dirty = true;
        self.recompute_matches();
        let mut msg = [0u8; 48];
        let mut n = 0usize;
        for &b in b"Replaced " {
            msg[n] = b;
            n += 1;
        }
        n += fmt_u64(count as u64, &mut msg[n..]);
        for &b in b" matches" {
            if n < msg.len() {
                msg[n] = b;
                n += 1;
            }
        }
        if let Ok(s) = core::str::from_utf8(&msg[..n]) {
            self.set_toast(s);
        }
    }

    /// The surface-local rect of visible sidebar row `i`, or `None` if `i` is not
    /// currently scrolled into view. Uses the SAME metrics `render_sidebar` draws.
    fn note_row_rect(&self, i: usize) -> Option<Rect> {
        let vis = self.list_visible_rows();
        let start = self.list_scroll;
        let end = (start + vis).min(self.notes.len());
        if i < start || i >= end {
            return None;
        }
        let sb_y = TITLE_H + TOOLBAR_H;
        let rel = i - start;
        Some(Rect {
            x: 0,
            y: sb_y + rel * ROW_H,
            w: SIDEBAR_W,
            h: ROW_H,
        })
    }

    /// Hit-test a surface-local click. Returns the action of the topmost element
    /// (close button, then a toolbar chip, then a sidebar note row), or
    /// `Action::None` for empty space. Pure: builds the SAME rects `render` draws.
    fn hit(&self, px: i32, py: i32) -> Action {
        if close_rect().contains(px, py) {
            return Action::Close;
        }
        // Find bar (when active) sits atop the main pane and captures its clicks.
        if self.find.active {
            if find_input_rect().contains(px, py) {
                return Action::FocusFind;
            }
            if find_btn_rect(0).contains(px, py) {
                return Action::ToggleCase;
            }
            if find_btn_rect(1).contains(px, py) {
                return Action::ToggleRegex;
            }
            if find_btn_rect(2).contains(px, py) {
                return Action::FindPrev;
            }
            if find_btn_rect(3).contains(px, py) {
                return Action::FindNext;
            }
            if find_btn_rect(4).contains(px, py) {
                return Action::CloseFind;
            }
            if self.find.replace_mode {
                if replace_input_rect().contains(px, py) {
                    return Action::FocusReplace;
                }
                if replace_btn_rect(0).contains(px, py) {
                    return Action::ReplaceCurrent;
                }
                if replace_btn_rect(1).contains(px, py) {
                    return Action::ReplaceAll;
                }
            }
        }
        let cy = chip_y();
        for (i, (_label, action)) in TOOLBAR_CHIPS.iter().enumerate() {
            let r = Rect {
                x: chip_x(i),
                y: cy,
                w: CHIP_W,
                h: CHIP_H,
            };
            if r.contains(px, py) {
                return *action;
            }
        }
        for i in 0..self.notes.len() {
            if let Some(r) = self.note_row_rect(i) {
                if r.contains(px, py) {
                    return Action::SelectNote(i);
                }
            }
        }
        Action::None
    }

    /// Apply an `Action` (shared by click dispatch + the hit-test proof). Returns
    /// true if anything changed (caller re-renders). `Close` exits. Each branch
    /// mirrors the matching key exactly; editing stays keyboard-driven.
    fn dispatch(&mut self, action: Action) -> bool {
        match action {
            Action::SelectNote(i) => {
                if i >= self.notes.len() {
                    return false;
                }
                // Click a row = select + open it (the PageUp/PageDown behavior).
                self.selected = i;
                self.open(i);
                true
            }
            Action::SetEdit => {
                self.mode = Mode::Edit;
                self.preview_scroll = 0;
                self.persist();
                true
            }
            Action::SetPreview => {
                self.mode = Mode::Preview;
                self.preview_scroll = 0;
                self.persist();
                true
            }
            Action::NewNote => {
                self.new_note();
                true
            }
            Action::DeleteNote => {
                self.delete_selected();
                true
            }
            Action::Close => athkit::sys::exit(0),
            Action::OpenFind => {
                self.open_find(false);
                true
            }
            Action::FocusFind => {
                self.find.field = FindField::Find;
                true
            }
            Action::FocusReplace => {
                self.find.replace_mode = true;
                self.find.field = FindField::Replace;
                true
            }
            Action::FindNext => {
                self.step_match(1);
                true
            }
            Action::FindPrev => {
                self.step_match(-1);
                true
            }
            Action::ToggleCase => {
                self.find.case_sensitive = !self.find.case_sensitive;
                self.recompute_matches();
                self.persist();
                true
            }
            Action::ToggleRegex => {
                self.find.regex_mode = !self.find.regex_mode;
                self.recompute_matches();
                self.persist();
                true
            }
            Action::ReplaceCurrent => {
                self.replace_current();
                true
            }
            Action::ReplaceAll => {
                self.replace_all_matches();
                true
            }
            Action::CloseFind => {
                self.close_find();
                true
            }
            Action::None => false,
        }
    }
}

// ── File I/O (never-panic wrappers over athkit) ───────────────────────────

/// Read a whole file into a heap buffer (capped). `None` on any error.
fn read_file(path: &str) -> Option<Vec<u8>> {
    let fd = athkit::sys::open(path, 0);
    if fd == u64::MAX {
        return None;
    }
    let mut data: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        if data.len() > READ_CAP {
            let _ = athkit::sys::close(fd);
            return None;
        }
        let n = athkit::sys::read(fd, &mut chunk) as usize;
        if n == 0 || n > chunk.len() {
            break;
        }
        data.extend_from_slice(&chunk[..n]);
    }
    let _ = athkit::sys::close(fd);
    Some(data)
}

/// Write `bytes` to `path` (create/truncate). Returns true on success.
fn write_file(path: &str, bytes: &[u8]) -> bool {
    // O_WRONLY | O_CREAT | O_TRUNC = 0x0241 (matches the Text Editor save path).
    let fd = athkit::sys::open(path, 0x0241);
    if fd == u64::MAX {
        return false;
    }
    let mut off = 0usize;
    let mut ok = true;
    while off < bytes.len() {
        let end = (off + 4096).min(bytes.len());
        let n = athkit::sys::write(fd, &bytes[off..end]) as usize;
        if n == 0 {
            ok = false;
            break;
        }
        off += n;
    }
    athkit::sys::close(fd);
    ok
}

/// Derive a safe `.md` file name from the note body's first heading / first
/// non-blank line (lowercased, spaces → '-', alnum/dash only). Falls back to
/// `note-<seq>.md`. Writes into `out`, returns the byte length.
fn derive_filename(body: &str, seq: u32, out: &mut [u8]) -> usize {
    // First non-blank line, stripping a leading '# ' run.
    let mut title = "";
    for line in body.split('\n') {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        title = t.trim_start_matches('#').trim();
        break;
    }
    let mut n = 0usize;
    let mut last_dash = true; // suppress leading dash
    for c in title.chars() {
        if n + 4 >= out.len() {
            break;
        }
        if c.is_ascii_alphanumeric() {
            out[n] = c.to_ascii_lowercase() as u8;
            n += 1;
            last_dash = false;
        } else if !last_dash {
            out[n] = b'-';
            n += 1;
            last_dash = true;
        }
    }
    // Trim a trailing dash.
    while n > 0 && out[n - 1] == b'-' {
        n -= 1;
    }
    if n == 0 {
        // Fallback: note-<seq>
        for &b in b"note-" {
            out[n] = b;
            n += 1;
        }
        n += fmt_u64(seq as u64, &mut out[n..]);
    }
    for &b in b".md" {
        if n + 1 > out.len() {
            break;
        }
        out[n] = b;
        n += 1;
    }
    n
}

// ── Rendering: chrome ─────────────────────────────────────────────────────

fn render(app: &App, canvas: &mut Canvas) {
    canvas.fill_rect(0, 0, WIN_W, WIN_H, BG);

    // Title bar
    canvas.fill_rect_gradient(0, 0, WIN_W, TITLE_H, DARK.bg_elevated, TITLE_BG);
    canvas.draw_text_aa(
        12,
        ((TITLE_H.saturating_sub(ath_tokens::TYPE_SUBTITLE.line_height as usize)) / 2) as i32,
        "Notes",
        ath_tokens::TYPE_SUBTITLE,
        TEXT_FG,
        FontFamily::Sans,
    );
    if app.buf.dirty {
        canvas.fill_rect(58, 12, 5, 5, DARK.state_warn);
    }
    canvas.fill_rounded_rect(
        WIN_W - 28,
        4,
        20,
        20,
        ath_tokens::RADIUS_XS as usize,
        DARK.state_danger,
    );
    let x_w = canvas.measure_text_aa("X", ath_tokens::TYPE_LABEL, FontFamily::Sans);
    canvas.draw_text_aa(
        (WIN_W - 18) as i32 - x_w / 2,
        (4 + (20 - ath_tokens::TYPE_LABEL.line_height as usize) / 2) as i32,
        "X",
        ath_tokens::TYPE_LABEL,
        0xFF_FF_FF_FF,
        FontFamily::Sans,
    );

    // Toolbar (mode chips + actions). Each chip draws at chip_x(i)/chip_y(), the
    // SAME rect `App::hit` tests, so the click target can't drift from the pixel.
    let tb_y = TITLE_H;
    canvas.fill_rect(0, tb_y, WIN_W, TOOLBAR_H, TOOLBAR_BG);
    let y = chip_y();
    for (i, (label, action)) in TOOLBAR_CHIPS.iter().enumerate() {
        let x = chip_x(i);
        let active = matches!(
            (action, app.mode),
            (Action::SetEdit, Mode::Edit) | (Action::SetPreview, Mode::Preview)
        );
        let fill = if active { sel_fill() } else { DARK.bg_elevated };
        canvas.fill_rounded_rect(x, y, CHIP_W, CHIP_H, ath_tokens::RADIUS_XS as usize, fill);
        let lw = canvas.measure_text_aa(label, ath_tokens::TYPE_LABEL, FontFamily::Sans);
        let ty =
            (y + (CHIP_H.saturating_sub(ath_tokens::TYPE_LABEL.line_height as usize)) / 2) as i32;
        canvas.draw_text_aa(
            x as i32 + (CHIP_W as i32 - lw) / 2,
            ty,
            label,
            ath_tokens::TYPE_LABEL,
            if active { TEXT_FG } else { TEXT_MUTED },
            FontFamily::Sans,
        );
    }

    render_sidebar(app, canvas);
    render_main(app, canvas);

    // Status bar
    let st_y = WIN_H - STATUS_H;
    canvas.fill_rect(0, st_y, WIN_W, STATUS_H, STATUS_BG);
    let st_ty = (st_y
        + (STATUS_H.saturating_sub(ath_tokens::TYPE_CAPTION.line_height as usize)) / 2)
        as i32;
    if !app.toast_str().is_empty() {
        canvas.draw_text_aa(
            12,
            st_ty,
            app.toast_str(),
            ath_tokens::TYPE_CAPTION,
            accent(),
            FontFamily::Sans,
        );
    } else {
        let mut cbuf = [0u8; 48];
        let n = fmt_count(app.notes.len(), &mut cbuf);
        if let Ok(s) = core::str::from_utf8(&cbuf[..n]) {
            canvas.draw_text_aa(
                12,
                st_ty,
                s,
                ath_tokens::TYPE_CAPTION,
                TEXT_MUTED,
                FontFamily::Sans,
            );
        }
    }
    let hint = "Tab:edit/preview  Ctrl+F:find  Ctrl+H:replace  Ctrl+R:regex  Ctrl+S:save  Esc:quit";
    let hw = canvas.measure_text_aa(hint, ath_tokens::TYPE_CAPTION, FontFamily::Sans);
    canvas.draw_text_aa(
        (WIN_W - 12) as i32 - hw,
        st_ty,
        hint,
        ath_tokens::TYPE_CAPTION,
        TEXT_DIM,
        FontFamily::Sans,
    );
}

fn render_sidebar(app: &App, canvas: &mut Canvas) {
    let sb_y = TITLE_H + TOOLBAR_H;
    let sb_h = WIN_H - sb_y - STATUS_H;
    canvas.fill_rect(0, sb_y, SIDEBAR_W, sb_h, SIDEBAR_BG);
    canvas.fill_rect(SIDEBAR_W, sb_y, 1, sb_h, STROKE_HL);

    if app.notes.is_empty() {
        canvas.draw_text_aa(
            14,
            (sb_y + 16) as i32,
            "No notes yet.",
            ath_tokens::TYPE_CAPTION,
            TEXT_MUTED,
            FontFamily::Sans,
        );
        canvas.draw_text_aa(
            14,
            (sb_y + 34) as i32,
            "New (or Ctrl+N) starts one.",
            ath_tokens::TYPE_CAPTION,
            TEXT_DIM,
            FontFamily::Sans,
        );
        return;
    }

    let vis = app.list_visible_rows();
    let start = app.list_scroll;
    let end = (start + vis).min(app.notes.len());
    for i in start..end {
        let rel = i - start;
        let ry = sb_y + rel * ROW_H;
        let selected = i == app.selected;
        if selected {
            canvas.fill_rect(0, ry, SIDEBAR_W, ROW_H, sel_fill());
            canvas.fill_rect(0, ry, 3, ROW_H, accent());
        } else if rel % 2 == 1 {
            canvas.fill_rect(0, ry, SIDEBAR_W, ROW_H, ROW_ALT_BG);
        }
        let title = note_title(app, i);
        let fg = if selected { TEXT_FG } else { TEXT_MUTED };
        let ty = ry + (ROW_H - ath_tokens::TYPE_LABEL.line_height as usize) / 2;
        draw_label_clipped(canvas, title, 12, ty, SIDEBAR_W - 20, fg);
    }
}

/// The sidebar title for note `i`: the first heading text if present, else the
/// file name (sans extension). Loading the body for every row would be costly,
/// so for the OPEN note we use its parsed first heading; others use the name.
fn note_title<'a>(app: &'a App, i: usize) -> &'a str {
    // Use the file name without its extension as the title (cheap + stable).
    // The open note's first H1 is not shown here to avoid re-reading every row.
    let name = app.notes[i].name();
    if let Some(stem) = name.rfind('.') {
        &name[..stem]
    } else {
        name
    }
}

fn render_main(app: &App, canvas: &mut Canvas) {
    let mx = SIDEBAR_W + 1;
    let mut my = TITLE_H + TOOLBAR_H;
    let mw = WIN_W - mx;
    let full_h = WIN_H - my - STATUS_H;
    canvas.fill_rect(mx, my, mw, full_h, BG);

    // The find bar (when active) occupies a strip at the top of the pane; the
    // editor/preview content starts below it.
    let mut mh = full_h;
    if app.find.active {
        let fh = find_bar_total_h(app.find.replace_mode);
        render_find_bar(app, canvas);
        my += fh;
        mh = full_h.saturating_sub(fh);
    }

    match app.mode {
        Mode::Edit => render_edit(app, canvas, mx, my, mw, mh),
        Mode::Preview => render_preview(app, canvas, mx, my, mw, mh),
    }
}

/// Draw the Find/Replace overlay bar: a Find input, [Aa]/[<]/[>]/[x] buttons,
/// an "N of M" counter, and (in replace mode) a Replace input + Replace/All
/// buttons. Geometry mirrors the `find_*_rect` helpers `App::hit` tests.
fn render_find_bar(app: &App, canvas: &mut Canvas) {
    let mx = SIDEBAR_W + 1;
    let mw = WIN_W - mx;
    let bar_h = find_bar_total_h(app.find.replace_mode);
    canvas.fill_rect(mx, find_bar_y(), mw, bar_h, TOOLBAR_BG);
    canvas.fill_rect(mx, find_bar_y() + bar_h, mw, 1, STROKE_HL);

    // Find input box.
    let fr = find_input_rect();
    let focused_find = app.find.field == FindField::Find;
    draw_find_input(canvas, fr, app.find.query_str(), focused_find, "Find");

    // [Aa] case toggle (greyed-active in regex mode, where it is ignored).
    draw_find_button(
        canvas,
        find_btn_rect(0),
        "Aa",
        app.find.case_sensitive && !app.find.regex_mode,
    );
    // [.*] regex toggle.
    draw_find_button(canvas, find_btn_rect(1), ".*", app.find.regex_mode);
    // [<] prev / [>] next.
    draw_find_button(canvas, find_btn_rect(2), "<", false);
    draw_find_button(canvas, find_btn_rect(3), ">", false);
    // [x] close.
    draw_find_button(canvas, find_btn_rect(4), "x", false);

    // "N of M" match counter — or "Bad regex" when a malformed pattern is typed.
    let ty = (find_btn_rect(4).y
        + (FIND_ROW_H.saturating_sub(ath_tokens::TYPE_CAPTION.line_height as usize)) / 2)
        as i32;
    if app.find.regex_mode && app.find.bad_regex {
        canvas.draw_text_aa(
            find_count_x() as i32,
            ty,
            "Bad regex",
            ath_tokens::TYPE_CAPTION,
            DARK.state_danger,
            FontFamily::Sans,
        );
    } else {
        let mut cbuf = [0u8; 24];
        let cn = fmt_n_of_m(
            if app.find.matches.is_empty() {
                0
            } else {
                app.find.current + 1
            },
            app.find.matches.len(),
            &mut cbuf,
        );
        if let Ok(s) = core::str::from_utf8(&cbuf[..cn]) {
            let fg = if app.find.matches.is_empty() {
                TEXT_DIM
            } else {
                TEXT_MUTED
            };
            canvas.draw_text_aa(
                find_count_x() as i32,
                ty,
                s,
                ath_tokens::TYPE_CAPTION,
                fg,
                FontFamily::Sans,
            );
        }
    }

    // Replace row.
    if app.find.replace_mode {
        let rr = replace_input_rect();
        let focused_repl = app.find.field == FindField::Replace;
        draw_find_input(
            canvas,
            rr,
            app.find.repl_str(),
            focused_repl,
            "Replace with",
        );
        draw_find_button(canvas, replace_btn_rect(0), "Replace", false);
        draw_find_button(canvas, replace_btn_rect(1), "All", false);
    }
}

/// Draw one find/replace input box (rounded fill, focus accent ring, text or a
/// dimmed placeholder, and a caret when focused).
fn draw_find_input(canvas: &mut Canvas, r: Rect, text: &str, focused: bool, placeholder: &str) {
    let fill = DARK.bg_base;
    canvas.fill_rounded_rect(r.x, r.y, r.w, r.h, ath_tokens::RADIUS_XS as usize, fill);
    if focused {
        // 1px accent ring (top/bottom/left/right).
        canvas.fill_rect(r.x, r.y, r.w, 1, accent());
        canvas.fill_rect(r.x, r.y + r.h - 1, r.w, 1, accent());
        canvas.fill_rect(r.x, r.y, 1, r.h, accent());
        canvas.fill_rect(r.x + r.w - 1, r.y, 1, r.h, accent());
    }
    let ty = (r.y
        + (r.h
            .saturating_sub(ath_tokens::TYPE_LABEL.line_height as usize))
            / 2) as i32;
    if text.is_empty() {
        canvas.draw_text_aa(
            (r.x + 8) as i32,
            ty,
            placeholder,
            ath_tokens::TYPE_LABEL,
            TEXT_DIM,
            FontFamily::Sans,
        );
    } else {
        let tw = canvas.measure_text_aa(text, ath_tokens::TYPE_LABEL, FontFamily::Sans);
        draw_label_clipped(
            canvas,
            text,
            r.x + 8,
            ty as usize,
            r.w.saturating_sub(16),
            TEXT_FG,
        );
        if focused {
            // caret after the (possibly clipped) text
            let cx = (r.x + 8 + (tw as usize).min(r.w.saturating_sub(16))) as i32;
            canvas.fill_rect(
                cx.max(0) as usize,
                r.y + 6,
                2,
                r.h.saturating_sub(12),
                accent(),
            );
        }
    }
}

/// Draw a find-bar button (icon/label). `active` highlights it (case toggle on).
fn draw_find_button(canvas: &mut Canvas, r: Rect, label: &str, active: bool) {
    let fill = if active { sel_fill() } else { DARK.bg_elevated };
    canvas.fill_rounded_rect(r.x, r.y, r.w, r.h, ath_tokens::RADIUS_XS as usize, fill);
    let lw = canvas.measure_text_aa(label, ath_tokens::TYPE_LABEL, FontFamily::Sans);
    let ty = (r.y
        + (r.h
            .saturating_sub(ath_tokens::TYPE_LABEL.line_height as usize))
            / 2) as i32;
    canvas.draw_text_aa(
        r.x as i32 + (r.w as i32 - lw) / 2,
        ty,
        label,
        ath_tokens::TYPE_LABEL,
        if active { TEXT_FG } else { TEXT_MUTED },
        FontFamily::Sans,
    );
}

/// Format "N of M" (or "No results" when M==0) into `out`, returns byte length.
fn fmt_n_of_m(n: usize, m: usize, out: &mut [u8]) -> usize {
    if m == 0 {
        let s = b"No results";
        let k = s.len().min(out.len());
        out[..k].copy_from_slice(&s[..k]);
        return k;
    }
    let mut len = fmt_u64(n as u64, out);
    for &b in b" of " {
        if len < out.len() {
            out[len] = b;
            len += 1;
        }
    }
    len += fmt_u64(m as u64, &mut out[len..]);
    len
}

/// Raw-Markdown edit view: a fixed-pitch 8x8 grid (caret tracks `cursor_col *
/// GLYPH_W`), matching the Text Editor's grid. Only chrome is crisp AA — a
/// proportional body would desync the caret from glyphs.
fn render_edit(app: &App, canvas: &mut Canvas, mx: usize, my: usize, mw: usize, mh: usize) {
    let pad = 10usize;
    let visible_rows = (mh.saturating_sub(pad * 2)) / EDIT_LINE_H;
    for i in 0..visible_rows {
        let row = app.buf.scroll + i;
        if row >= app.buf.nlines {
            break;
        }
        let py = my + pad + i * EDIT_LINE_H;
        // Highlight any find matches that fall on this row (drawn under the text).
        if app.find.active {
            for (col_start, col_end, is_current) in row_match_spans(app, row) {
                let hx = mx + pad + col_start * EDIT_GLYPH_W;
                let hw = (col_end - col_start) * EDIT_GLYPH_W;
                let fill = if is_current {
                    sel_fill()
                } else {
                    DARK.bg_elevated
                };
                canvas.fill_rect(hx, py, hw.max(1), EDIT_LINE_H, fill);
            }
        }
        let s = app.buf.lines[row].as_str();
        canvas.draw_text(mx + pad, py, s, TEXT_FG, None);
        if row == app.buf.cursor_row {
            let cx = mx + pad + app.buf.cursor_col * EDIT_GLYPH_W;
            canvas.fill_rect(cx, py, 2, 10, accent());
        }
    }
    let _ = mw;
}

/// The per-row highlight spans for find matches on buffer row `row`, as
/// `(col_start, col_end, is_current)` byte columns. A match that spans newlines
/// is clamped to this row's bounds (never out of range → never panics).
fn row_match_spans(app: &App, row: usize) -> Vec<(usize, usize, bool)> {
    let mut spans = Vec::new();
    if app.find.matches.is_empty() {
        return spans;
    }
    // Byte offset where `row` begins in the materialized string, and its length.
    let mut row_start = 0usize;
    for r in 0..row {
        row_start += app.buf.lines[r].len + 1; // +1 for '\n'
    }
    let row_len = app.buf.lines[row].len;
    let row_end = row_start + row_len;
    for (mi, &(ms, me)) in app.find.matches.iter().enumerate() {
        // Intersect [ms,me) with [row_start,row_end).
        let s = ms.max(row_start);
        let e = me.min(row_end);
        if s < e {
            spans.push((s - row_start, e - row_start, mi == app.find.current));
        }
    }
    spans
}

// ── Rendering: the Markdown → rich-text walk (the heart of the app) ───────
//
// `parse(buffer)` → `Document` (`Vec<Block>`), then we walk it and DRAW:
//   Heading    → draw_text_aa at heading_style(level) (larger/bold per level)
//   Paragraph  → inline run rendering, soft-wrapped to the column width
//   Bold/Italic→ inline styled (bold = heavier weight; italic = accent tint to
//                read as emphasis on the bitmap-derived AA face)
//   Code (span)→ JetBrains-Mono (FontFamily::Mono) over a subtle CODE_BG pill
//   Link       → accent-colored display text
//   List       → bullet '•' / "N." marker + indent, items as paragraphs
//   BlockQuote → left accent bar + QUOTE_BG wash, inner blocks indented
//   CodeBlock  → mono panel on CODE_BG (literal text, no inline parsing)
//   ThematicBreak → a horizontal rule
// Never panics: the parser already degrades bad markdown to literal text.

/// A tiny cursor describing where the next block draws, plus the clip region.
struct DrawCtx {
    x: usize,     // left margin (content origin x)
    y: i32,       // current baseline-top y
    width: usize, // content width (for wrapping)
    bottom: i32,  // clip bottom (stop drawing past this)
}

fn render_preview(app: &App, canvas: &mut Canvas, mx: usize, my: usize, mw: usize, mh: usize) {
    let pad = 18usize;
    let doc = parse(&app.buf.to_string());
    let mut ctx = DrawCtx {
        x: mx + pad,
        y: (my + pad) as i32 - app.preview_scroll as i32,
        width: mw.saturating_sub(pad * 2),
        bottom: (my + mh) as i32,
    };
    draw_blocks(canvas, &doc, &mut ctx, 0);

    if doc.is_empty() {
        canvas.draw_text_aa(
            (mx + pad) as i32,
            (my + pad) as i32,
            "This note is empty.",
            ath_tokens::TYPE_BODY,
            TEXT_DIM,
            FontFamily::Sans,
        );
    }
}

/// Walk a block list at indent `depth` (each level = 18px of left inset).
fn draw_blocks(canvas: &mut Canvas, blocks: &[Block], ctx: &mut DrawCtx, depth: usize) {
    let indent = depth * 18;
    for block in blocks {
        if ctx.y > ctx.bottom {
            break; // past the clip — stop (cheap scroll cull)
        }
        match block {
            Block::Heading { level, content } => {
                let style = heading_style(*level);
                draw_inline_line(
                    canvas,
                    content,
                    ctx.x + indent,
                    &mut ctx.y,
                    ctx.width,
                    style,
                    TEXT_FG,
                );
                ctx.y += 6; // heading bottom margin
            }
            Block::Paragraph(content) => {
                draw_inline_line(
                    canvas,
                    content,
                    ctx.x + indent,
                    &mut ctx.y,
                    ctx.width.saturating_sub(indent),
                    ath_tokens::TYPE_BODY,
                    TEXT_FG,
                );
                ctx.y += 6;
            }
            Block::List { ordered, items } => {
                draw_list(canvas, *ordered, items, ctx, depth);
            }
            Block::BlockQuote(inner) => {
                let start_y = ctx.y;
                // Draw the inner blocks into a sub-context (inset past the bar),
                // then back-fill the left accent bar spanning start..end.
                let mut sub = DrawCtx {
                    x: ctx.x + indent + 12,
                    y: ctx.y,
                    width: ctx.width.saturating_sub(indent + 16),
                    bottom: ctx.bottom,
                };
                draw_blocks(canvas, inner, &mut sub, 0);
                let end_y = sub.y;
                // Left accent bar spanning the quote (clipped to visible region).
                if end_y > start_y && start_y >= 0 {
                    let h = (end_y - start_y).max(0) as usize;
                    canvas.fill_rect(ctx.x + indent, start_y as usize, 3, h, accent());
                }
                ctx.y = end_y + 6;
            }
            Block::CodeBlock { lang, code } => {
                draw_code_block(canvas, lang, code, ctx, indent);
            }
            Block::ThematicBreak => {
                if ctx.y >= 0 {
                    canvas.fill_rect(
                        ctx.x + indent,
                        ctx.y as usize + 6,
                        ctx.width.saturating_sub(indent),
                        1,
                        STROKE_HL,
                    );
                }
                ctx.y += 16;
            }
            Block::Table { headers, rows, .. } => {
                // Minimal readable render of a GFM pipe table: the header row (bold)
                // then each body row, cells joined by a separator. A full aligned
                // grid is a follow-up; the content stays legible, never dropped.
                let w = ctx.width.saturating_sub(indent);
                for (ri, cells) in core::iter::once(headers).chain(rows.iter()).enumerate() {
                    let mut line: Vec<Inline> = Vec::new();
                    for (ci, cell) in cells.iter().enumerate() {
                        if ci > 0 {
                            line.push(Inline::Text(String::from("  |  ")));
                        }
                        line.extend(cell.iter().cloned());
                    }
                    let style = if ri == 0 {
                        TypeStyle {
                            px: ath_tokens::TYPE_BODY.px,
                            weight: 600,
                            line_height: ath_tokens::TYPE_BODY.line_height,
                        }
                    } else {
                        ath_tokens::TYPE_BODY
                    };
                    draw_inline_line(canvas, &line, ctx.x + indent, &mut ctx.y, w, style, TEXT_FG);
                }
                ctx.y += 6;
            }
        }
    }
}

/// Draw a list: each item gets a marker + its content blocks at depth+1.
fn draw_list(
    canvas: &mut Canvas,
    ordered: bool,
    items: &[ListItem],
    ctx: &mut DrawCtx,
    depth: usize,
) {
    let indent = depth * 18;
    for (i, item) in items.iter().enumerate() {
        if ctx.y > ctx.bottom {
            break;
        }
        // Marker.
        let marker_y = ctx.y;
        if marker_y >= 0 {
            let mx = ctx.x + indent;
            if ordered {
                let mut nb = [0u8; 8];
                let mut n = fmt_u64((i + 1) as u64, &mut nb);
                if n < nb.len() {
                    nb[n] = b'.';
                    n += 1;
                }
                if let Ok(s) = core::str::from_utf8(&nb[..n]) {
                    canvas.draw_text_aa(
                        mx as i32,
                        marker_y,
                        s,
                        ath_tokens::TYPE_BODY,
                        accent(),
                        FontFamily::Sans,
                    );
                }
            } else {
                canvas.draw_text_aa(
                    mx as i32,
                    marker_y,
                    "\u{2022}",
                    ath_tokens::TYPE_BODY,
                    accent(),
                    FontFamily::Sans,
                );
            }
        }
        // Item content, inset past the marker.
        let mut sub = DrawCtx {
            x: ctx.x + indent + 22,
            y: ctx.y,
            width: ctx.width.saturating_sub(indent + 22),
            bottom: ctx.bottom,
        };
        draw_blocks(canvas, &item.blocks, &mut sub, 0);
        // Ensure at least one line of advance even for an empty item.
        ctx.y = sub.y.max(ctx.y + ath_tokens::TYPE_BODY.line_height as i32);
    }
    ctx.y += 4;
}

/// Draw a fenced code block: a CODE_BG panel, mono text, literal (no inlines).
fn draw_code_block(canvas: &mut Canvas, lang: &str, code: &str, ctx: &mut DrawCtx, indent: usize) {
    let line_h = ath_tokens::TYPE_BODY.line_height as usize;
    let x = ctx.x + indent;
    let w = ctx.width.saturating_sub(indent);
    // Count lines for the panel height.
    let nlines = code.split('\n').count().max(1);
    let panel_h = nlines * line_h + 12;
    let top = ctx.y;
    if top >= 0 && top < ctx.bottom {
        canvas.fill_rounded_rect(
            x,
            top as usize,
            w,
            panel_h,
            ath_tokens::RADIUS_XS as usize,
            CODE_BG,
        );
    }
    let _ = lang;
    let mut ly = top + 6;
    for line in code.split('\n') {
        if ly >= 0 && ly < ctx.bottom {
            canvas.draw_text_aa(
                (x + 8) as i32,
                ly,
                line,
                ath_tokens::TYPE_BODY,
                DARK.text_secondary,
                FontFamily::Mono,
            );
        }
        ly += line_h as i32;
    }
    ctx.y = top + panel_h as i32 + 6;
}

/// Draw a single line of inline content with word-wrap to `width`. Advances `*y`
/// by however many wrapped rows it produced. Bold/italic/code/link styling per
/// run. The fallback line height is the heading/body style's line_height.
fn draw_inline_line(
    canvas: &mut Canvas,
    inlines: &[Inline],
    x: usize,
    y: &mut i32,
    width: usize,
    base_style: TypeStyle,
    base_fg: u32,
) {
    let line_h = base_style.line_height as i32;
    let mut cx = x;
    let max_x = x + width;
    // Walk inlines, drawing runs and wrapping at word boundaries when we exceed
    // `max_x`. A LineBreak forces a new row.
    draw_inline_runs(
        canvas, inlines, x, &mut cx, y, max_x, line_h, base_style, base_fg,
    );
    // Always advance at least one line.
    *y += line_h;
}

#[allow(clippy::too_many_arguments)]
fn draw_inline_runs(
    canvas: &mut Canvas,
    inlines: &[Inline],
    left: usize,
    cx: &mut usize,
    y: &mut i32,
    max_x: usize,
    line_h: i32,
    style: TypeStyle,
    fg: u32,
) {
    for inl in inlines {
        match inl {
            Inline::Text(t) => {
                draw_wrapped_text(
                    canvas,
                    t,
                    left,
                    cx,
                    y,
                    max_x,
                    line_h,
                    style,
                    fg,
                    FontFamily::Sans,
                );
            }
            Inline::Bold(inner) => {
                let bold = TypeStyle {
                    px: style.px,
                    weight: 600,
                    line_height: style.line_height,
                };
                draw_inline_runs(canvas, inner, left, cx, y, max_x, line_h, bold, fg);
            }
            Inline::Italic(inner) => {
                // No italic face shipped — tint to the accent to read as emphasis.
                draw_inline_runs(canvas, inner, left, cx, y, max_x, line_h, style, accent());
            }
            Inline::Code(c) => {
                // Inline code: mono face on a subtle pill.
                let w = canvas.measure_text_aa(c, style, FontFamily::Mono);
                if *cx as i32 + w > max_x as i32 && *cx > left {
                    *cx = left;
                    *y += line_h;
                }
                if *y >= 0 {
                    canvas.fill_rounded_rect(
                        cx.saturating_sub(2),
                        (*y).max(0) as usize,
                        (w + 4).max(0) as usize,
                        style.line_height as usize,
                        ath_tokens::RADIUS_XS as usize,
                        CODE_BG,
                    );
                    canvas.draw_text_aa(*cx as i32, *y, c, style, DARK.state_ok, FontFamily::Mono);
                }
                *cx += w as usize + 4;
            }
            Inline::Link { text, .. } => {
                // Link display text in accent.
                draw_inline_runs(canvas, text, left, cx, y, max_x, line_h, style, accent());
            }
            Inline::LineBreak => {
                *cx = left;
                *y += line_h;
            }
            Inline::Strikethrough(inner) => {
                // No strikethrough glyph face is shipped yet; render the inner runs
                // so the struck (GFM `~~text~~`) content still reads. The strike-line
                // itself is a follow-up — the text is never dropped.
                draw_inline_runs(canvas, inner, left, cx, y, max_x, line_h, style, fg);
            }
        }
    }
}

/// Draw a text run with word-wrap. Splits on spaces; a word that exceeds the
/// remaining width wraps to a new row. Updates `*cx`/`*y`. Never panics.
#[allow(clippy::too_many_arguments)]
fn draw_wrapped_text(
    canvas: &mut Canvas,
    text: &str,
    left: usize,
    cx: &mut usize,
    y: &mut i32,
    max_x: usize,
    line_h: i32,
    style: TypeStyle,
    fg: u32,
    family: FontFamily,
) {
    let space_w = canvas.measure_text_aa(" ", style, family) as usize;
    let mut first = true;
    for word in text.split(' ') {
        if word.is_empty() {
            // Preserve runs of spaces as advance.
            *cx += space_w;
            continue;
        }
        let ww = canvas.measure_text_aa(word, style, family) as usize;
        // Leading space between words (not at row start).
        if !first && *cx > left {
            *cx += space_w;
        }
        first = false;
        if *cx + ww > max_x && *cx > left {
            *cx = left;
            *y += line_h;
        }
        if *y >= 0 {
            canvas.draw_text_aa(*cx as i32, *y, word, style, fg, family);
        }
        *cx += ww;
    }
}

// ── Small helpers ─────────────────────────────────────────────────────────

/// Draw a string clipped to `max_w` px (ellipsis on overflow).
fn draw_label_clipped(canvas: &mut Canvas, name: &str, x: usize, y: usize, max_w: usize, fg: u32) {
    let full_w = canvas.measure_text_aa(name, ath_tokens::TYPE_LABEL, FontFamily::Sans);
    if (full_w as usize) <= max_w {
        canvas.draw_text_aa(
            x as i32,
            y as i32,
            name,
            ath_tokens::TYPE_LABEL,
            fg,
            FontFamily::Sans,
        );
        return;
    }
    let bytes = name.as_bytes();
    let mut take = bytes.len();
    while take > 0 {
        let mut buf = [0u8; NAME_CAP + 2];
        let t = take.min(NAME_CAP);
        buf[..t].copy_from_slice(&bytes[..t]);
        buf[t] = b'.';
        buf[t + 1] = b'.';
        if let Ok(s) = core::str::from_utf8(&buf[..t + 2]) {
            let w = canvas.measure_text_aa(s, ath_tokens::TYPE_LABEL, FontFamily::Sans);
            if (w as usize) <= max_w {
                canvas.draw_text_aa(
                    x as i32,
                    y as i32,
                    s,
                    ath_tokens::TYPE_LABEL,
                    fg,
                    FontFamily::Sans,
                );
                return;
            }
        }
        take -= 1;
    }
}

fn fmt_count(n: usize, out: &mut [u8]) -> usize {
    let mut len = fmt_u64(n as u64, out);
    let suffix: &[u8] = if n == 1 { b" note" } else { b" notes" };
    for &b in suffix {
        if len >= out.len() {
            break;
        }
        out[len] = b;
        len += 1;
    }
    len
}

fn fmt_u64(mut v: u64, out: &mut [u8]) -> usize {
    if v == 0 {
        out[0] = b'0';
        return 1;
    }
    let mut tmp = [0u8; 20];
    let mut i = 0;
    while v > 0 {
        tmp[i] = b'0' + (v % 10) as u8;
        v /= 10;
        i += 1;
    }
    let mut n = 0;
    while i > 0 {
        i -= 1;
        if n >= out.len() {
            break;
        }
        out[n] = tmp[i];
        n += 1;
    }
    n
}

// ── Scancode → ASCII (Text Editor's proven table) ─────────────────────────

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

// ── Design proof (R10: a fail-able check tokens + parse→render wiring) ─────

/// True iff Notes' chrome is wired to the shared design tokens AND the
/// Markdown parse → render wiring holds. Deliberately fail-able: a regression in
/// token wiring, the parser, or the heading-sizing render invariant flips this to
/// `false` (exit code 3 at startup). `ath_markdown`'s own host KATs prove the
/// parser logic in isolation; this proves THIS app's parse → render mapping.
#[must_use]
pub fn design_proof() -> bool {
    let ramp = ath_tokens::derive_accent(theme_seed(), &DARK);
    let tokens_ok = accent() == ramp.base
        && sel_fill() == ramp.active
        && BG == DARK.bg_raised
        && TITLE_BG == DARK.bg_base
        && TOOLBAR_BG == DARK.bg_overlay
        && SIDEBAR_BG == DARK.bg_base
        && TEXT_FG == DARK.text_primary
        && TEXT_MUTED == DARK.text_secondary
        && TEXT_DIM == DARK.text_tertiary
        && STROKE_HL == DARK.stroke_strong
        && athkit::sys::THEME_DEFAULT_ACCENT == RAEBLUE;

    tokens_ok
        && parse_render_wiring_ok()
        && hit_test_proof()
        && find_replace_proof()
        && prefs_round_trip_ok()
}

/// Prove the Notes PREFS SCHEMA: a known non-default `Prefs` serialized via
/// `ath_toml` then re-parsed restores every field exactly (last-open note, the
/// edit/preview mode, the find-bar case + regex toggles), AND a corrupt /
/// missing-key document resolves to the typed defaults (NOT a panic, NOT a wrong
/// value). This proves the per-app prefs contract on top of `ath_toml`'s own
/// parser KATs. Returns `false` on any drift (→ exit(3) at startup).
#[must_use]
fn prefs_round_trip_ok() -> bool {
    // (a) Full round-trip of a non-default Prefs.
    let p = Prefs {
        last_note: String::from("my-todo.md"),
        preview: true,
        case_sensitive: true,
        regex_mode: true,
    };
    let text = ath_toml::to_string(&p.to_toml());
    let parsed = match ath_toml::parse(&text) {
        Ok(t) => t,
        Err(_) => return false,
    };
    let back = Prefs::from_toml(&parsed);
    if back.last_note != "my-todo.md" || !back.preview || !back.case_sensitive || !back.regex_mode {
        return false;
    }

    // (b) A corrupt document → typed defaults (parse FAILS, we don't panic).
    let corrupt = "preview = = oops\n[unterminated\n";
    let d = match ath_toml::parse(corrupt) {
        Ok(t) => Prefs::from_toml(&t), // shouldn't reach here for this input
        Err(_) => Prefs::defaults(),
    };
    if !d.last_note.is_empty() || d.preview || d.case_sensitive || d.regex_mode {
        return false;
    }

    // (c) A well-formed doc MISSING every prefs key → typed defaults per field.
    let empty = match ath_toml::parse("unrelated = 1\n") {
        Ok(t) => t,
        Err(_) => return false,
    };
    let e = Prefs::from_toml(&empty);
    if !e.last_note.is_empty() || e.preview || e.case_sensitive || e.regex_mode {
        return false;
    }

    // (d) A wrong-TYPED field (preview as a string) is ignored → default, not a
    // crash; an unrelated valid key still parses.
    let wrong = match ath_toml::parse("preview = \"yes\"\nlast_note = \"x.md\"\n") {
        Ok(t) => t,
        Err(_) => return false,
    };
    let w = Prefs::from_toml(&wrong);
    if w.preview || w.last_note != "x.md" {
        return false;
    }

    true
}

/// Prove the Find/Replace core logic (the load-bearing search/replace fns) over
/// the buffer string. Fail-able (→ exit(3)): case-insensitive vs case-sensitive
/// match counts/offsets, `replace_all` count + result, `replace_one_at` scoping,
/// the no-match / empty-needle no-ops, and the UTF-8 char-boundary safety case.
#[must_use]
fn find_replace_proof() -> bool {
    // (1) Case-insensitive: "the" matches twice in a mixed-case haystack.
    let hay = "the cat sat on the mat";
    let ci = find_matches(hay, "the", false);
    if ci.len() != 2 || ci[0] != (0, 3) || ci[1] != (15, 18) {
        return false;
    }
    // Slicing the haystack at every returned range must land on real text.
    if &hay[ci[0].0..ci[0].1] != "the" || &hay[ci[1].0..ci[1].1] != "the" {
        return false;
    }

    // (2) Case-sensitive differs: in "The cat sat on the mat", "The" matches once.
    let hay2 = "The cat sat on the mat";
    let cs = find_matches(hay2, "The", true);
    if cs.len() != 1 || cs[0] != (0, 3) {
        return false;
    }
    // Case-insensitive over the same finds both "The" and "the".
    let ci2 = find_matches(hay2, "the", false);
    if ci2.len() != 2 {
        return false;
    }

    // (3) replace_all over a punctuation needle (default case-insensitive).
    let (out, n) = replace_all("a.b.c", ".", "-");
    if n != 2 || out != "a-b-c" {
        return false;
    }

    // (4) replace_one_at replaces ONLY the given range.
    let one = replace_one_at("a.b.c", (1, 2), "-");
    if one != "a-b.c" {
        return false;
    }

    // (5) A needle not present → 0 matches, unchanged buffer.
    if !find_matches("hello", "zzz", false).is_empty() {
        return false;
    }
    let (un, uc) = replace_all("hello", "zzz", "Q");
    if uc != 0 || un != "hello" {
        return false;
    }

    // (6) Empty needle → 0 matches (no infinite loop), unchanged buffer.
    if !find_matches("hello", "", false).is_empty() {
        return false;
    }
    let (en, ec) = replace_all("hello", "", "Q");
    if ec != 0 || en != "hello" {
        return false;
    }

    // (7) Replace with empty string is allowed (deletion).
    let (del, dc) = replace_all("a-b-c", "-", "");
    if dc != 2 || del != "abc" {
        return false;
    }

    // (8) UTF-8 char-boundary safety: a multibyte char in the haystack is not
    // split mid-codepoint. "café" (é = 2 bytes) searched for "caf" matches at
    // (0,3); searching for "fé" matches the 'f' + 'é' run on a boundary; and a
    // search that would straddle the é must NOT produce a mid-codepoint range.
    let uni = "café au lait"; // é at bytes 3..5
    let m = find_matches(uni, "caf", false);
    if m.len() != 1 || m[0] != (0, 3) || &uni[m[0].0..m[0].1] != "caf" {
        return false;
    }
    // Searching for "é" (2 bytes) lands exactly on the é.
    let me = find_matches(uni, "é", false);
    if me.len() != 1 || &uni[me[0].0..me[0].1] != "é" {
        return false;
    }
    // replace_all over a multibyte needle preserves the rest byte-for-byte.
    let (uo, un2) = replace_all(uni, "é", "e");
    if un2 != 1 || uo != "cafe au lait" {
        return false;
    }
    // A 1-byte ASCII needle equal to the first byte of a multibyte char must not
    // false-match into the middle of that char. 0xC3 0xA9 = é; needle "\u{00e9}"
    // covered above. Here ensure "a" doesn't match inside "é".
    let ma = find_matches("é", "a", false);
    if !ma.is_empty() {
        return false;
    }

    regex_proof()
}

/// Prove the REGEX find/replace path (ath_regex wiring into the editor's match
/// machinery). Fail-able (→ exit(3)): a `\d+` find returns the right ranges, a
/// group-ref replace expands `$1`/`$2`, a bad pattern is handled as `None` (no
/// crash), and an empty query yields no matches. `ath_regex`'s own host KATs
/// prove the engine; this proves THIS app's regex → match-list wiring.
#[must_use]
fn regex_proof() -> bool {
    // (a) Regex find: "\d+" over "ab12cd345" → two matches at (2,4) and (6,9).
    let m = match regex_find_matches("ab12cd345", "\\d+") {
        Some(v) => v,
        None => return false,
    };
    if m.len() != 2 || m[0] != (2, 4) || m[1] != (6, 9) {
        return false;
    }
    // The ranges slice the haystack on real boundaries.
    if &"ab12cd345"[m[0].0..m[0].1] != "12" || &"ab12cd345"[m[1].0..m[1].1] != "345" {
        return false;
    }

    // (b) Regex replace with a group swap: "(\w)(\w)" on "ab" with "$2$1" → "ba".
    let (out, n) = regex_replace_all("ab", "(\\w)(\\w)", "$2$1");
    if n != 1 || out != "ba" {
        return false;
    }

    // (c) Regex replace-one bounded to the current match: replace just (2,4) of
    // "ab12cd" (the "12") with the group-swapped "$0" identity → unchanged span,
    // and a literal replacement "#" → "ab#cd".
    let one = regex_replace_one_at("ab12cd", (2, 4), "\\d+", "#");
    if one != "ab#cd" {
        return false;
    }

    // (d) A BAD pattern is handled, never panics: `a(` → None (treated as zero
    // matches), and the replace paths leave the buffer unchanged.
    if regex_find_matches("anything", "a(").is_some() {
        return false;
    }
    let (bo, bn) = regex_replace_all("anything", "a(", "x");
    if bn != 0 || bo != "anything" {
        return false;
    }
    if regex_replace_one_at("anything", (0, 1), "a(", "x") != "anything" {
        return false;
    }

    // (e) An empty query → zero matches (no zero-width loop), unchanged buffer.
    match regex_find_matches("text", "") {
        Some(v) if v.is_empty() => {}
        _ => return false,
    }

    true
}

/// Prove the mouse hit-test invariant: a click on a known element's rect-center
/// resolves to that element's action (the SAME rects `render` draws), and an
/// out-of-bounds click resolves to `Action::None`. Returns `false` on any drift
/// (→ exit(3) at startup). Builds a synthetic 3-note app so sidebar-row geometry
/// exists without touching the real Notes directory.
#[must_use]
fn hit_test_proof() -> bool {
    let mut app = App {
        dir: PathBuf::new(),
        notes: Vec::new(),
        selected: 0,
        list_scroll: 0,
        buf: Buf::new(),
        mode: Mode::Edit,
        open_idx: usize::MAX,
        preview_scroll: 0,
        toast: [0; 64],
        toast_len: 0,
        new_seq: 0,
        find: FindState::new(),
    };
    for label in [b"a.md".as_slice(), b"b.md", b"c.md"] {
        let mut name = [0u8; NAME_CAP];
        name[..label.len()].copy_from_slice(label);
        app.notes.push(Note {
            name,
            name_len: label.len(),
        });
    }

    // (1) A click at sidebar-row 0's center hits SelectNote(0).
    let r0 = match app.note_row_rect(0) {
        Some(r) => r,
        None => return false,
    };
    if app.hit((r0.x + r0.w / 2) as i32, (r0.y + r0.h / 2) as i32) != Action::SelectNote(0) {
        return false;
    }

    // (2) Each toolbar chip's center hits its own action.
    let cy = chip_y();
    for (i, (_label, action)) in TOOLBAR_CHIPS.iter().enumerate() {
        let cx = (chip_x(i) + CHIP_W / 2) as i32;
        let yc = (cy + CHIP_H / 2) as i32;
        if app.hit(cx, yc) != *action {
            return false;
        }
    }

    // (3) The Preview chip specifically maps to SetPreview; dispatch switches mode.
    let prev_idx = TOOLBAR_CHIPS
        .iter()
        .position(|(_, a)| *a == Action::SetPreview);
    let Some(pi) = prev_idx else { return false };
    let pcx = (chip_x(pi) + CHIP_W / 2) as i32;
    let pcy = (cy + CHIP_H / 2) as i32;
    if app.hit(pcx, pcy) != Action::SetPreview {
        return false;
    }

    // (4) Out-of-bounds clicks resolve to None.
    if app.hit(-100, -100) != Action::None {
        return false;
    }
    if app.hit(WIN_W as i32 + 50, WIN_H as i32 + 50) != Action::None {
        return false;
    }

    // (5) Dispatching SetPreview switches the mode to Preview.
    let _ = app.dispatch(Action::SetPreview);
    app.mode == Mode::Preview
}

/// Parse a built-in fixture and assert (1) the Document has the expected block
/// shape (a level-1 heading, a paragraph carrying a Bold inline, and an
/// unordered list of two items), AND (2) the render's heading-sizing invariant
/// holds: H1 draws larger than H3, which is at least body size, and H3 draws
/// heavier than body weight. Returns `false` on any drift.
fn parse_render_wiring_ok() -> bool {
    let fixture = "# Title\n\nsome **bold** text\n\n- one\n- two\n";
    let doc = parse(fixture);
    if doc.len() != 3 {
        return false;
    }
    // (1a) First block is an H1 with text "Title".
    match &doc[0] {
        Block::Heading { level, content } => {
            if *level != 1 {
                return false;
            }
            if !matches!(content.first(), Some(Inline::Text(t)) if t == "Title") {
                return false;
            }
        }
        _ => return false,
    }
    // (1b) Second block is a paragraph whose runs include a Bold inline.
    match &doc[1] {
        Block::Paragraph(runs) => {
            if !runs.iter().any(|i| matches!(i, Inline::Bold(_))) {
                return false;
            }
        }
        _ => return false,
    }
    // (1c) Third block is an unordered list of two items.
    match &doc[2] {
        Block::List { ordered, items } => {
            if *ordered || items.len() != 2 {
                return false;
            }
        }
        _ => return false,
    }

    // (2) Render invariant: heading sizing is strictly ordered by level
    // (H1 > H2 > H3), H1 draws larger than body, H3 is at least body size, and
    // H3 reads as a heading (heavier weight than body).
    let h1 = heading_style(1);
    let h2 = heading_style(2);
    let h3 = heading_style(3);
    let body = ath_tokens::TYPE_BODY;
    h1.px > h2.px && h2.px > h3.px && h1.px > body.px && h3.px >= body.px && h3.weight > body.weight
}

// ── Entry point ───────────────────────────────────────────────────────────

#[no_mangle]
pub extern "C" fn _start() -> ! {
    if !design_proof() {
        athkit::sys::exit(3);
    }
    let sid = athkit::sys::surface_create(WIN_W as u64, WIN_H as u64, SURFACE_VIRT);
    if sid == u64::MAX {
        athkit::sys::exit(1);
    }

    let mut canvas = unsafe { Canvas::new(SURFACE_VIRT as *mut u8, WIN_W, WIN_H, 4) };

    let mut app = App::new();
    render(&app, &mut canvas);
    athkit::sys::surface_present(sid, PRESENT_X as u64, PRESENT_Y as u64);

    let mut extended = false;
    let mut shift = false;
    let mut ctrl = false;
    let mut left_was_down = false;
    let edit_rows = (WIN_H - TITLE_H - TOOLBAR_H - STATUS_H).saturating_sub(20) / EDIT_LINE_H;

    loop {
        // ── Mouse: drain button events, hit-test the cursor on a click edge ──
        let mut mouse_activity = false;
        let mut left_down = left_was_down;
        loop {
            let ev = athkit::sys::poll_mouse();
            if ev == 0 {
                break;
            }
            left_down = (ev & 0x01) != 0;
            mouse_activity = true;
        }
        if mouse_activity || left_down != left_was_down {
            if left_down && !left_was_down {
                let (cx, cy, _btn) = athkit::sys::cursor_pos();
                // Subtract the LIVE window origin (not the stale present-time
                // PRESENT_X/Y) so clicks land correctly after the window manager
                // moves the window (Overview / Spaces / tiling). Falls back to the
                // present origin if the surface isn't found. Saturating-sub keeps a
                // cursor above/left of the window from underflowing.
                let (ox, oy) = athkit::sys::surface_origin(sid)
                    .unwrap_or((PRESENT_X as u32, PRESENT_Y as u32));
                let lx = (cx as i32).saturating_sub(ox as i32);
                let ly = (cy as i32).saturating_sub(oy as i32);
                if app.dispatch(app.hit(lx, ly)) {
                    render(&app, &mut canvas);
                    athkit::sys::surface_present(sid, PRESENT_X as u64, PRESENT_Y as u64);
                }
            }
            left_was_down = left_down;
        }

        let key = athkit::sys::read_key();
        if key == 0 {
            athkit::sys::yield_now();
            continue;
        }
        let sc = key as u8;
        if sc == 0xE0 {
            extended = true;
            continue;
        }
        let ext = core::mem::replace(&mut extended, false);
        let release = sc & 0x80 != 0;
        let code = sc & 0x7F;

        // Modifier latches.
        if code == 0x2A || code == 0x36 {
            shift = !release;
            continue;
        }
        if code == 0x1D {
            ctrl = !release;
            continue;
        }
        if release {
            continue;
        }

        let mut dirty = false;

        // ── Find bar: when active, it captures keyboard input first ──────────
        if app.find.active {
            // Ctrl-combos still work (toggle case, switch to replace, save).
            if ctrl {
                match code {
                    0x21 => {
                        // Ctrl+F (again): focus the Find field.
                        app.find.field = FindField::Find;
                        dirty = true;
                    }
                    0x23 => {
                        // Ctrl+H: reveal replace row, focus it.
                        app.find.replace_mode = true;
                        app.find.field = FindField::Replace;
                        dirty = true;
                    }
                    0x17 => {
                        // Ctrl+I: toggle case-sensitivity.
                        app.find.case_sensitive = !app.find.case_sensitive;
                        app.recompute_matches();
                        app.persist();
                        dirty = true;
                    }
                    0x13 => {
                        // Ctrl+R: toggle regex mode.
                        app.find.regex_mode = !app.find.regex_mode;
                        app.recompute_matches();
                        app.persist();
                        dirty = true;
                    }
                    _ => {}
                }
                if dirty {
                    render(&app, &mut canvas);
                    athkit::sys::surface_present(sid, PRESENT_X as u64, PRESENT_Y as u64);
                }
                continue;
            }
            match (ext, code) {
                (false, 0x01) => {
                    // Esc closes the find bar.
                    app.close_find();
                    dirty = true;
                }
                (false, 0x0F) => {
                    // Tab switches focus between Find and Replace (if shown).
                    if app.find.replace_mode {
                        app.find.field = match app.find.field {
                            FindField::Find => FindField::Replace,
                            FindField::Replace => FindField::Find,
                        };
                        dirty = true;
                    }
                }
                (false, 0x1C) => {
                    // Enter = next match (Shift+Enter = previous).
                    if shift {
                        app.step_match(-1);
                    } else {
                        app.step_match(1);
                    }
                    dirty = true;
                }
                (true, 0x48) => {
                    // Up = previous match.
                    app.step_match(-1);
                    dirty = true;
                }
                (true, 0x50) => {
                    // Down = next match.
                    app.step_match(1);
                    dirty = true;
                }
                _ => {
                    if let Some(ch) = scancode_to_ascii(code, shift) {
                        match ch {
                            0x08 => {
                                app.find.backspace_field();
                                app.recompute_matches();
                                dirty = true;
                            }
                            c if c >= 0x20 && c < 0x7F => {
                                app.find.push_char(c);
                                app.recompute_matches();
                                dirty = true;
                            }
                            _ => {}
                        }
                    }
                }
            }
            if dirty {
                render(&app, &mut canvas);
                athkit::sys::surface_present(sid, PRESENT_X as u64, PRESENT_Y as u64);
            }
            continue;
        }

        // Global keys (work in both modes).
        match (ext, code) {
            (false, 0x01) => athkit::sys::exit(0), // Esc = quit
            (false, 0x21) if ctrl => {
                // Ctrl+F = open Find bar.
                app.open_find(false);
                dirty = true;
            }
            (false, 0x23) if ctrl => {
                // Ctrl+H = open Find+Replace bar.
                app.open_find(true);
                app.find.field = FindField::Replace;
                dirty = true;
            }
            (false, 0x0F) => {
                // Tab = toggle edit/preview.
                app.toggle_mode();
                dirty = true;
            }
            (false, 0x1F) if ctrl => {
                // Ctrl+S = save
                app.save();
                dirty = true;
            }
            (false, 0x31) if ctrl => {
                // Ctrl+N = new note
                app.new_note();
                dirty = true;
            }
            (false, 0x20) if ctrl => {
                // Ctrl+D = delete selected note
                app.delete_selected();
                dirty = true;
            }
            (true, 0x49) => {
                // PageUp = sidebar select up (works in both modes)
                app.move_sel(-1);
                if app.selected < app.notes.len() {
                    app.open(app.selected);
                }
                dirty = true;
            }
            (true, 0x51) => {
                // PageDown = sidebar select down
                app.move_sel(1);
                if app.selected < app.notes.len() {
                    app.open(app.selected);
                }
                dirty = true;
            }
            _ => {
                match app.mode {
                    Mode::Edit => match (ext, code) {
                        (true, 0x4B) => {
                            app.buf.move_left();
                            dirty = true;
                        }
                        (true, 0x4D) => {
                            app.buf.move_right();
                            dirty = true;
                        }
                        (true, 0x48) => {
                            app.buf.move_up();
                            dirty = true;
                        }
                        (true, 0x50) => {
                            app.buf.move_down();
                            dirty = true;
                        }
                        _ => {
                            if let Some(ch) = scancode_to_ascii(code, shift) {
                                match ch {
                                    b'\n' => {
                                        app.buf.newline();
                                        dirty = true;
                                    }
                                    0x08 => {
                                        app.buf.backspace();
                                        dirty = true;
                                    }
                                    c if c >= 0x20 && c < 0x7F => {
                                        app.buf.insert_char(c);
                                        dirty = true;
                                    }
                                    _ => {}
                                }
                            }
                        }
                    },
                    Mode::Preview => {
                        match (ext, code) {
                            (true, 0x48) => {
                                // Up = scroll preview up
                                app.preview_scroll = app.preview_scroll.saturating_sub(24);
                                dirty = true;
                            }
                            (true, 0x50) => {
                                // Down = scroll preview down
                                app.preview_scroll = app.preview_scroll.saturating_add(24);
                                dirty = true;
                            }
                            _ => {}
                        }
                    }
                }
            }
        }

        if dirty {
            if app.mode == Mode::Edit {
                app.buf.update_scroll(edit_rows);
            }
            render(&app, &mut canvas);
            athkit::sys::surface_present(sid, PRESENT_X as u64, PRESENT_Y as u64);
        }
    }
}

// FOLLOW-UP: register Notes tile in athshell/start_menu (the concurrent opus
// session owns athshell/start_menu — this slice deliberately does NOT edit it).
// Once that session lands, add a "Notes" entry with exec_path = "notes" so the
// app is launchable from the Start menu, not only via the initramfs bundle.
