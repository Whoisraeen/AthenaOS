//! AthenaOS Text Editor (Notepad-equivalent).
//!
//! Standalone userspace ELF launched from the start menu
//! (`exec_path = "text_editor"`). Implements a fixed-line line buffer with
//! insertion, navigation, and a status line. File I/O is now implemented
//! for "todo.txt" (validated end-to-end with a mounted AthFS).
//!
//! FIND & REPLACE (parity with Notes / Win11 Notepad / macOS TextEdit): Ctrl+F
//! opens a find bar with live match recompute, a case toggle (default
//! case-insensitive, Ctrl+I / [Aa]), Enter/Down for next, Shift+Enter/Up for
//! previous (both wrap), an "N of M" counter, caret/scroll jump to the current
//! match, and match highlighting. Ctrl+H reveals a Replace field with Replace
//! (one) and Replace All; replacements update the buffer + mark it dirty so a
//! subsequent Ctrl+S persists them. Esc closes the bar. The bar is mouse-
//! clickable via the same draw-rects-are-hit-rects pattern (keyboard is the
//! must). NEVER PANICS: the search/replace core (`find_matches` /
//! `replace_all` / `replace_one_at`) is char-boundary-safe, so a multibyte UTF-8
//! char in the buffer can never be split mid-codepoint, and an empty needle is a
//! no-op (no zero-width loop).
//!
//! PROOF: this ELF can't run `cargo test`, so `design_proof()` (a fail-able
//! runtime gate at `_start`) asserts both the shared-token wiring AND the
//! find/replace core logic — case-insensitive vs case-sensitive match
//! counts/offsets, `replace_all` count + result, `replace_one_at` scoping, the
//! no-match / empty-needle no-ops, and a UTF-8 ("café") char-boundary case —
//! exit(3) on any drift. (The exact assertions mirror Notes' proven proof.)

#![no_std]
#![no_main]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

#[allow(unused_imports)]
use athkit;

use ath_regex::Regex;
use ath_tokens::{DARK, RAEBLUE};
use ath_toml::Toml;
use athgfx::text::FontFamily;
use athgfx::Canvas;

// ── Window geometry ─────────────────────────────────────────────────────

const WIN_W: usize = 700;
const WIN_H: usize = 460;
const SURFACE_VIRT: u64 = 0x0000_7C00_0000;

const TITLE_H: usize = 28;
const TOOLBAR_H: usize = 32;
const STATUS_H: usize = 22;
const GUTTER_W: usize = 56;
const GLYPH_W: usize = 8;
const GLYPH_H: usize = 8;
const LINE_H: usize = 14;

/// The on-screen origin we present this window at. Absolute cursor coordinates
/// from `cursor_pos()` are converted to surface-local space by subtracting this
/// (falling back here if the live surface origin can't be read).
const PRESENT_X: i32 = 280;
const PRESENT_Y: i32 = 120;

// ── Palette (ath_tokens, docs/design/design-language.md) ──────────────────
//
// Generic chrome pulled onto `ath_tokens::DARK` + the RaeBlue accent ramp for
// whole-OS cohesion. Accent shades (cursor, selection) are derived (non-const)
// so they live in helpers. The "unsaved" dirty dot maps to `state_warn` (a real
// attention token), so no app-specific colors remain. Live Vibe accent =
// NEEDS-INTERFACE (see report).

const BG: u32 = DARK.bg_raised;
const TITLE_BG: u32 = DARK.bg_base;
const TOOLBAR_BG: u32 = DARK.bg_overlay;
const GUTTER_BG: u32 = DARK.bg_base;
const TEXT_FG: u32 = DARK.text_primary;
const TEXT_DIM: u32 = DARK.text_secondary;
const LINE_NUM_FG: u32 = DARK.text_tertiary;
const STATUS_BG: u32 = DARK.bg_base;
const DIRTY_DOT: u32 = DARK.state_warn; // unsaved-changes attention indicator

/// The live desktop accent seed (Vibe Mode) via `SYS_THEME_GET`, or RaeBlue when
/// the theme syscall is unavailable. Read at launch so the editor re-skins to
/// the active theme (Concept §Customization Engine).
fn theme_seed() -> u32 {
    athkit::sys::theme_accent()
}
/// Caret color: the accent base (live ramp). Matches the desktop accent.
fn cursor() -> u32 {
    ath_tokens::derive_accent(theme_seed(), &DARK).base
}
/// Selection wash: the accent's active (pressed) shade.
fn sel_bg() -> u32 {
    ath_tokens::derive_accent(theme_seed(), &DARK).active
}

// ── Buffer ──────────────────────────────────────────────────────────────

const MAX_LINES: usize = 256;
const LINE_CAP: usize = 128;

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
    shift: bool,
    ctrl: bool,
    /// Path the buffer is bound to (save target / persisted "last file").
    last_file: String,
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
            shift: false,
            ctrl: false,
            last_file: String::from(DEFAULT_FILE),
        }
    }

    fn seed_welcome(&mut self) {
        let welcome = [
            "# Welcome to AthenaOS Text Editor",
            "",
            "Tab/Esc to dismiss. Type freely.",
            "Backspace deletes; Enter starts a new line.",
            "Arrow keys move the cursor.",
            "",
            "Ctrl+S saves to \"todo.txt\".",
            "Ctrl+F to find; Ctrl+H to replace.",
        ];
        for (i, line) in welcome.iter().enumerate() {
            for &b in line.as_bytes() {
                self.lines[i].insert(self.lines[i].len, b);
            }
        }
        self.nlines = welcome.len();
        self.dirty = false;
    }

    fn save(&mut self) {
        // O_WRONLY | O_CREAT | O_TRUNC = 0x0241. Save to the bound file (defaults
        // to "todo.txt"), keeping the persisted "last file" and target in sync.
        let target = if self.last_file.is_empty() {
            String::from(DEFAULT_FILE)
        } else {
            self.last_file.clone()
        };
        let fd = athkit::sys::open(&target, 0x0241);
        if fd == u64::MAX {
            return;
        }

        for i in 0..self.nlines {
            let line = &self.lines[i];
            athkit::sys::write(fd, &line.chars[..line.len]);
            athkit::sys::write(fd, b"\n");
        }
        athkit::sys::close(fd);
        self.dirty = false;
    }

    /// Materialise the whole buffer as one string (lines joined by '\n'), for the
    /// find/replace core. The newline join matches the on-disk save format, so a
    /// match offset maps back to a `(row, col)` via `byte_off_to_rowcol`.
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

    /// Replace the buffer contents with `text` (split on '\n'). Never panics:
    /// over-long lines / over-many lines are truncated to the buffer caps. Used
    /// to re-load the buffer after a Replace (one/all) edits the materialized
    /// string.
    fn load_text(&mut self, text: &str) {
        for i in 0..self.nlines {
            self.lines[i].len = 0;
        }
        let mut row = 0usize;
        for raw in text.split('\n') {
            if row >= MAX_LINES {
                break;
            }
            let line = raw.strip_suffix('\r').unwrap_or(raw);
            let bytes = line.as_bytes();
            let n = bytes.len().min(LINE_CAP);
            self.lines[row].chars[..n].copy_from_slice(&bytes[..n]);
            self.lines[row].len = n;
            row += 1;
        }
        self.nlines = row.max(1);
        if self.cursor_row >= self.nlines {
            self.cursor_row = self.nlines - 1;
        }
        let cur_len = self.lines[self.cursor_row].len;
        if self.cursor_col > cur_len {
            self.cursor_col = cur_len;
        }
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

    fn home(&mut self) {
        self.cursor_col = 0;
    }
    fn end(&mut self) {
        self.cursor_col = self.lines[self.cursor_row].len;
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
// These three functions are the load-bearing search/replace logic, copied
// IDENTICALLY from the proven Notes implementation (kept self-contained per the
// "no shared crate this slice" directive). Pure (string-in / ranges-or-string-
// out) so they're reused by both the find UI and `design_proof`. CHAR-BOUNDARY
// SAFETY: every returned range is a real substring boundary of a `find`/scanned
// hit, so slicing the haystack at any returned `start`/`end` lands on a UTF-8
// char boundary — a multibyte char can never be split mid-codepoint. An empty
// needle returns no matches (no zero-width infinite loop); replace with "" is
// allowed (deletion).

/// All match ranges (byte `(start, end)`) of `needle` in `haystack`, non-
/// overlapping, left-to-right. Case-insensitive folds both sides to ASCII
/// lowercase (the editor's text domain); a case-sensitive request matches bytes
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
        // ASCII-case-insensitive: compare the haystack window to the needle under
        // `eq_ignore_ascii_case`. We only ever START a window at a char boundary
        // (guarded) and the window length equals the needle's byte length; an
        // equal-length ASCII-folded match preserves char boundaries (a multibyte
        // UTF-8 char never byte-equals an ASCII fold of a different length).
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
/// pattern → unchanged buffer, 0 count.
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
/// the regex-expanded replacement for that one match (so `$1` group refs expand
/// against the match), then splice the result back. The range MUST be a real
/// `regex_find_matches` span. A bad pattern or degenerate range leaves the buffer
/// unchanged (never panics).
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

// ── Persistent preferences (ath_toml) ─────────────────────────────────────────
//
// LEGACY_GAMING_CONCEPT.md §"The user owns the machine": "remember my settings" must be
// real. The Text Editor persists its find-bar toggles (case-sensitive + regex,
// mirroring Notes) AND the last-opened file PATH to
// `<home>/.config/texteditor.toml`, restoring them on launch. The buffer CONTENT
// is NOT persisted (it is saved as files); only WHICH file + how to search it. On
// launch the last file is re-opened if it still exists; on a miss the buffer falls
// back to the empty/welcome state. Every load is hostile-input-tolerant: a
// missing, corrupt, or out-of-range config falls back to TYPED DEFAULTS and NEVER
// panics. This is the per-app prefs pattern the consumer apps follow.

/// The default file the editor reads/writes (matches the existing save target).
const DEFAULT_FILE: &str = "todo.txt";
/// Cap on a persisted file path (a pathological length can't blow anything up;
/// it's only ever fed to `open`).
const FILE_PATH_CAP: usize = 256;

/// The decoded, defaulted preferences restored at launch. Pure data: load builds
/// it from a parsed (or absent) TOML; save serializes the live state.
#[derive(Clone)]
struct Prefs {
    /// Find-bar case-sensitivity toggle (default off = case-insensitive).
    case_sensitive: bool,
    /// Find-bar regex-mode toggle (default off = literal).
    regex_mode: bool,
    /// Last-opened file path (re-opened on launch if it still exists). Empty =
    /// none → welcome buffer.
    last_file: String,
}

impl Prefs {
    /// The typed defaults used on first run or any config error.
    fn defaults() -> Self {
        Self {
            case_sensitive: false,
            regex_mode: false,
            last_file: String::new(),
        }
    }

    /// Build `Prefs` from a parsed TOML table, validating every field and
    /// substituting the typed default for any missing / wrong-typed value. Never
    /// panics; an unrelated shape (e.g. a non-table root) yields full defaults.
    fn from_toml(t: &Toml) -> Self {
        let mut p = Self::defaults();
        if let Some(b) = t.get("case_sensitive").and_then(Toml::as_bool) {
            p.case_sensitive = b;
        }
        if let Some(b) = t.get("regex_mode").and_then(Toml::as_bool) {
            p.regex_mode = b;
        }
        if let Some(s) = t.get("last_file").and_then(Toml::as_str) {
            p.last_file = String::from(truncate_on_char_boundary(s, FILE_PATH_CAP));
        }
        p
    }

    /// Serialize the live preferences into an order-stable `Toml::Table` ready for
    /// `ath_toml::to_string`. The schema is flat (no headers), human-editable.
    fn to_toml(&self) -> Toml {
        let mut table: Vec<(String, Toml)> = Vec::new();
        table.push((
            String::from("case_sensitive"),
            Toml::Boolean(self.case_sensitive),
        ));
        table.push((String::from("regex_mode"), Toml::Boolean(self.regex_mode)));
        table.push((
            String::from("last_file"),
            Toml::String(self.last_file.clone()),
        ));
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

/// A fixed-capacity path builder for the config file (`<home>/.config/...`),
/// allocation-free and char-boundary-safe (we only ever push known ASCII names).
const CFG_PATH_CAP: usize = 256;
struct CfgPath {
    bytes: [u8; CFG_PATH_CAP],
    len: usize,
}

impl CfgPath {
    fn new() -> Self {
        Self {
            bytes: [0; CFG_PATH_CAP],
            len: 0,
        }
    }
    fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.len]).unwrap_or("/")
    }
    fn set(&mut self, s: &str) {
        let n = s.as_bytes().len().min(CFG_PATH_CAP);
        self.bytes[..n].copy_from_slice(&s.as_bytes()[..n]);
        self.len = n;
    }
    fn push_component(&mut self, name: &str) {
        if self.len > 0 && self.bytes[self.len - 1] != b'/' && self.len < CFG_PATH_CAP {
            self.bytes[self.len] = b'/';
            self.len += 1;
        }
        for &b in name.as_bytes() {
            if self.len >= CFG_PATH_CAP {
                break;
            }
            self.bytes[self.len] = b;
            self.len += 1;
        }
    }
}

/// The per-app config DIRECTORY: `<session home>/.config`. Falls back to
/// `/home/user/.config` when no session is present. The `.config` directory is
/// created (idempotent) before any write.
fn prefs_dir() -> CfgPath {
    let mut p = CfgPath::new();
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

/// Load preferences from `<home>/.config/texteditor.toml`. On ANY failure — file
/// absent, unreadable, not UTF-8, or a `ath_toml::parse` error — returns the typed
/// defaults. Never panics, never blocks the app from launching.
fn load_prefs() -> Prefs {
    let mut path = prefs_dir();
    path.push_component("texteditor.toml");
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

/// Persist `prefs` to `<home>/.config/texteditor.toml` (best effort). Creates the
/// `.config` directory if missing, serializes via `ath_toml::to_string`, and
/// writes O_CREAT|O_TRUNC. A failure is silent — the app keeps running.
fn save_prefs(prefs: &Prefs) {
    let dir = prefs_dir();
    let _ = athkit::sys::mkdir(dir.as_str());
    let mut path = dir;
    path.push_component("texteditor.toml");
    let text = ath_toml::to_string(&prefs.to_toml());
    // O_WRONLY | O_CREAT | O_TRUNC = 0x0241.
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

/// Read the whole file at `path` into a `String`, or `None` if it can't be opened
/// / read / decoded. Used to re-open the last file on launch. Never panics.
fn read_file_to_string(path: &str) -> Option<String> {
    let fd = athkit::sys::open(path, 0);
    if fd == u64::MAX {
        return None;
    }
    let mut data: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        // Bound the slurp: the line buffer caps at MAX_LINES*LINE_CAP anyway.
        if data.len() > 4 * 1024 * 1024 {
            break;
        }
        let n = athkit::sys::read(fd, &mut chunk) as usize;
        if n == 0 || n > chunk.len() {
            break;
        }
        data.extend_from_slice(&chunk[..n]);
    }
    let _ = athkit::sys::close(fd);
    match core::str::from_utf8(&data) {
        Ok(s) => Some(String::from(s)),
        Err(_) => None,
    }
}

// ── Open / Save-As dialogs (browse + save ANY file) ───────────────────────
//
// A real editor opens and saves arbitrary files, not just its bound `last_file`.
// The Open dialog (Ctrl+O) is a modal overlay listing the current directory via
// the SAME `readdir_at` mechanism the Files app uses (decoding
// `[name_len:u16][size:u32][name…]` records, folder heuristic = `size==0 &&
// !name.contains('.')`). Dirs render first, then files (sorted); Up/Down select,
// Enter on a dir enters it (a synthetic ".." entry goes up), Enter on a file
// loads it + rebinds `last_file`, Esc cancels. Save-As (Ctrl+Shift+S) types a
// new filename in the current dir and rebinds the buffer to it.
//
// DIRTY GUARD: opening a new file while the buffer is `dirty` AUTO-SAVES the
// current file first (to its bound path), so edits are never lost silently —
// the simplest correct choice (mirrors a "save on switch" editor). A persisted
// path with no bound target falls back to the default save file.
//
// NEVER PANICS: every directory read / file read / write error surfaces as a
// transient toast and the app stays alive; the decode/join/sort core is pure +
// char-boundary-safe (proven by `dialog_proof`).

/// Cap on a browsed path (matches the Files app's `PATH_CAP`).
const DLG_PATH_CAP: usize = 256;
/// Max directory entries shown in the Open dialog (dir reads are bounded).
const DLG_MAX_ENTRIES: usize = 64;
/// Cap on a Save-As filename field.
const SAVEAS_CAP: usize = 96;

/// A fixed-capacity, char-boundary-safe path buffer for the dialogs (only ever
/// fed ASCII '/' separators + entry names; identical model to the Files app).
#[derive(Clone, Copy)]
struct DlgPath {
    bytes: [u8; DLG_PATH_CAP],
    len: usize,
}

impl DlgPath {
    fn new() -> Self {
        Self {
            bytes: [0; DLG_PATH_CAP],
            len: 0,
        }
    }
    fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.len]).unwrap_or("/")
    }
    fn set(&mut self, s: &str) {
        let n = s.as_bytes().len().min(DLG_PATH_CAP);
        self.bytes[..n].copy_from_slice(&s.as_bytes()[..n]);
        self.len = n;
        // Snap the truncation point back to a char boundary so `as_str` is valid.
        while self.len > 0 && core::str::from_utf8(&self.bytes[..self.len]).is_err() {
            self.len -= 1;
        }
        if self.len == 0 {
            self.bytes[0] = b'/';
            self.len = 1;
        }
    }
    /// Append `name` as a path component, inserting a single '/' separator when
    /// needed. Only appends bytes + an ASCII '/', so the join is char-boundary
    /// safe (a multibyte name is copied whole or not at all per the cap guard).
    fn push_component(&mut self, name: &str) {
        if self.len > 0 && self.bytes[self.len - 1] != b'/' && self.len < DLG_PATH_CAP {
            self.bytes[self.len] = b'/';
            self.len += 1;
        }
        for &b in name.as_bytes() {
            if self.len >= DLG_PATH_CAP {
                break;
            }
            self.bytes[self.len] = b;
            self.len += 1;
        }
    }
    /// Drop the last path component (go to the parent dir). Leaves "/" at root.
    fn pop_component(&mut self) {
        if self.len <= 1 {
            return;
        }
        if self.bytes[self.len - 1] == b'/' {
            self.len -= 1;
        }
        while self.len > 1 && self.bytes[self.len - 1] != b'/' {
            self.len -= 1;
        }
        if self.len > 1 && self.bytes[self.len - 1] == b'/' {
            self.len -= 1;
        }
        if self.len == 0 {
            self.bytes[0] = b'/';
            self.len = 1;
        }
    }
}

/// The session home dir (`<home>`), or `/home/user` when no session is present.
/// Used as the Open dialog's start dir when there's no bound file.
fn home_dir() -> DlgPath {
    let mut p = DlgPath::new();
    let mut info = [0u8; 96];
    if athkit::sys::session_info(&mut info).is_some() {
        if let Some(home) = athkit::sys::session_home_from(&info) {
            p.set(home);
            return p;
        }
    }
    p.set("/home/user");
    p
}

/// The directory component of `path` (everything up to the last '/'), or `<home>`
/// when `path` has no separator (a bare filename like "todo.txt"). Char-boundary
/// safe: it only splits at an ASCII '/' byte, never inside a multibyte char.
fn dir_of(path: &str) -> DlgPath {
    match path.rfind('/') {
        Some(0) => {
            // "/file" → root.
            let mut p = DlgPath::new();
            p.set("/");
            p
        }
        Some(i) => {
            let mut p = DlgPath::new();
            p.set(&path[..i]);
            p
        }
        None => home_dir(),
    }
}

/// One decoded directory entry: leaf name + whether it's a directory. Mirrors the
/// Files app's `DynamicEntry` shape (fixed-capacity, copyable).
#[derive(Clone, Copy)]
struct DirEntry {
    name: [u8; 48],
    name_len: usize,
    is_dir: bool,
}

impl DirEntry {
    fn name_str(&self) -> &str {
        core::str::from_utf8(&self.name[..self.name_len]).unwrap_or("?")
    }
}

/// Decode a `readdir_at`-format buffer (`[name_len:u16][size:u32][name…]`
/// records, `count` of them) into `out`, applying the Files-app folder heuristic
/// (`size==0 && !name.contains('.')`) and skipping self/parent/empty links. Pure
/// + never panics (every slice is bounds-checked); used by `read_dir` AND
/// `dialog_proof`. Returns the number of entries decoded.
fn decode_readdir(buf: &[u8], count: usize, out: &mut [DirEntry; DLG_MAX_ENTRIES]) -> usize {
    let mut off = 0usize;
    let mut n_out = 0usize;
    for _ in 0..count {
        if off + 6 > buf.len() || n_out >= DLG_MAX_ENTRIES {
            break;
        }
        let name_len = u16::from_le_bytes([buf[off], buf[off + 1]]) as usize;
        let size = u32::from_le_bytes([buf[off + 2], buf[off + 3], buf[off + 4], buf[off + 5]]);
        off += 6;
        if off + name_len > buf.len() {
            break;
        }
        let name = &buf[off..off + name_len];
        off += name_len;
        // Skip "." / ".." / empty (the dialog injects its own ".." entry).
        if name.is_empty() || name == b"." || name == b".." {
            continue;
        }
        let take = name.len().min(48);
        // Don't accept a name truncated mid-UTF-8 char: shrink to a boundary.
        let mut take = take;
        while take > 0 && core::str::from_utf8(&name[..take]).is_err() {
            take -= 1;
        }
        if take == 0 {
            continue;
        }
        let is_dir = size == 0 && !name[..take].contains(&b'.');
        let slot = &mut out[n_out];
        slot.name[..take].copy_from_slice(&name[..take]);
        slot.name_len = take;
        slot.is_dir = is_dir;
        n_out += 1;
    }
    sort_entries(out, n_out);
    n_out
}

/// Stable-ish sort of the first `n` entries: directories first, then files, each
/// group alphabetical by ASCII-lowercased name. Pure (insertion sort over the
/// fixed array → no alloc); used by `decode_readdir` and asserted by the proof.
fn sort_entries(entries: &mut [DirEntry; DLG_MAX_ENTRIES], n: usize) {
    for i in 1..n {
        let mut j = i;
        while j > 0 && entry_less(&entries[j], &entries[j - 1]) {
            entries.swap(j, j - 1);
            j -= 1;
        }
    }
}

/// Ordering predicate: dirs sort before files; within a group, case-insensitive
/// ASCII name order. Pure / total / never panics.
fn entry_less(a: &DirEntry, b: &DirEntry) -> bool {
    if a.is_dir != b.is_dir {
        return a.is_dir; // a dir comes before a file
    }
    let an = a.name_str().as_bytes();
    let bn = b.name_str().as_bytes();
    let mut i = 0;
    while i < an.len() && i < bn.len() {
        let ca = an[i].to_ascii_lowercase();
        let cb = bn[i].to_ascii_lowercase();
        if ca != cb {
            return ca < cb;
        }
        i += 1;
    }
    an.len() < bn.len()
}

/// Read the directory at `path` via `readdir_at`, returning the decoded + sorted
/// entries (dirs first). On a read error or empty dir, returns 0 entries (the
/// dialog still shows its ".." row). Never panics.
fn read_dir(path: &str) -> ([DirEntry; DLG_MAX_ENTRIES], usize) {
    let mut buf = [0u8; 4096];
    let count = athkit::sys::readdir_at(path, &mut buf) as usize;
    let empty = DirEntry {
        name: [0; 48],
        name_len: 0,
        is_dir: false,
    };
    let mut entries = [empty; DLG_MAX_ENTRIES];
    // A bogus/huge count (error sentinel) decodes to nothing — defensive.
    let count = count.min(DLG_MAX_ENTRIES * 4);
    let n = decode_readdir(&buf, count, &mut entries);
    (entries, n)
}

/// Open-file dialog state: a modal overlay browsing `dir`, with a ".." parent row
/// at index 0 followed by the directory's entries. `sel` is the highlighted row
/// (0 = ".."). Mouse-clickable rows mirror the keyboard selection 1:1.
struct OpenDialog {
    active: bool,
    dir: DlgPath,
    entries: [DirEntry; DLG_MAX_ENTRIES],
    n: usize,
    sel: usize,
    scroll: usize,
}

impl OpenDialog {
    fn new() -> Self {
        let empty = DirEntry {
            name: [0; 48],
            name_len: 0,
            is_dir: false,
        };
        Self {
            active: false,
            dir: DlgPath::new(),
            entries: [empty; DLG_MAX_ENTRIES],
            n: 0,
            sel: 0,
            scroll: 0,
        }
    }
    /// Total rows = 1 (the ".." parent) + the directory entries.
    fn row_count(&self) -> usize {
        self.n + 1
    }
    /// Open the dialog in `dir`, reading its contents.
    fn open_in(&mut self, dir: &str) {
        self.dir.set(dir);
        self.reload();
        self.active = true;
    }
    /// Re-read the current directory (after entering / going up).
    fn reload(&mut self) {
        let (e, n) = read_dir(self.dir.as_str());
        self.entries = e;
        self.n = n;
        self.sel = 0;
        self.scroll = 0;
    }
    fn move_sel(&mut self, dir: i32, visible: usize) {
        let total = self.row_count() as i32;
        if total == 0 {
            return;
        }
        let next = (self.sel as i32 + dir).rem_euclid(total) as usize;
        self.sel = next;
        if self.sel < self.scroll {
            self.scroll = self.sel;
        } else if visible > 0 && self.sel >= self.scroll + visible {
            self.scroll = self.sel + 1 - visible;
        }
    }
    /// Go to the parent directory.
    fn go_up(&mut self) {
        self.dir.pop_component();
        self.reload();
    }
    /// Enter the directory entry at row `sel` (1-based into `entries`). No-op if
    /// the row isn't a directory.
    fn enter_dir(&mut self, idx: usize) {
        if idx >= self.n {
            return;
        }
        let mut name = [0u8; 48];
        let len = self.entries[idx].name_len.min(48);
        name[..len].copy_from_slice(&self.entries[idx].name[..len]);
        if let Ok(s) = core::str::from_utf8(&name[..len]) {
            self.dir.push_component(s);
            self.reload();
        }
    }
}

/// Save-As dialog state: a single filename text field (saved into the buffer's
/// current directory). Reuses the editor's scancode→ASCII input handling.
struct SaveAsDialog {
    active: bool,
    /// The directory the new file lands in (the buffer's current dir).
    dir: DlgPath,
    name: [u8; SAVEAS_CAP],
    name_len: usize,
}

impl SaveAsDialog {
    fn new() -> Self {
        Self {
            active: false,
            dir: DlgPath::new(),
            name: [0; SAVEAS_CAP],
            name_len: 0,
        }
    }
    fn name_str(&self) -> &str {
        core::str::from_utf8(&self.name[..self.name_len]).unwrap_or("")
    }
    fn push_char(&mut self, ch: u8) {
        // Only printable ASCII enters the filename (no '/' so it stays a leaf).
        if ch == b'/' || ch < 0x20 || ch >= 0x7F {
            return;
        }
        if self.name_len < SAVEAS_CAP {
            self.name[self.name_len] = ch;
            self.name_len += 1;
        }
    }
    fn backspace(&mut self) {
        if self.name_len > 0 {
            self.name_len -= 1;
        }
    }
    /// The full absolute path the file will be saved to (`dir/name`).
    fn full_path(&self) -> DlgPath {
        let mut p = self.dir;
        p.push_component(self.name_str());
        p
    }
}

/// A transient one-line status message (errors / results) shown briefly in the
/// status bar. Cleared after `ttl` render passes so it doesn't linger forever.
struct Toast {
    text: [u8; 80],
    len: usize,
    ttl: u32,
}
impl Toast {
    fn empty() -> Self {
        Self {
            text: [0; 80],
            len: 0,
            ttl: 0,
        }
    }
    fn set(&mut self, s: &str) {
        let n = s.as_bytes().len().min(80);
        // Cut on a char boundary so `as_str` is always valid.
        let mut n = n;
        while n > 0 && !s.is_char_boundary(n) {
            n -= 1;
        }
        self.text[..n].copy_from_slice(&s.as_bytes()[..n]);
        self.len = n;
        self.ttl = 240; // ~ a few seconds of idle frames
    }
    fn as_str(&self) -> &str {
        core::str::from_utf8(&self.text[..self.len]).unwrap_or("")
    }
    fn tick(&mut self) {
        if self.ttl > 0 {
            self.ttl -= 1;
            if self.ttl == 0 {
                self.len = 0;
            }
        }
    }
}

// ── Find-bar state & geometry ─────────────────────────────────────────────
//
// The find bar is a horizontal strip at the top of the editor pane (just below
// the toolbar), pushing the text body down. Its rects are computed from the SAME
// constants `render_find_bar` draws with, so clicks can't drift from the pixels.
// Layout (left→right): [Find input] [Aa] [<] [>] [N/M] [x]; the replace row
// (when shown) adds [Replace input] [Replace] [All]. Keyboard is the must; the
// mouse is mirrored 1:1 onto those actions.

const FIND_CAP: usize = 128;
const FIND_ROW_H: usize = 26; // each input/button row
const FIND_PAD: usize = 8;
const FIND_INPUT_W: usize = 220;
const FIND_BTN_W: usize = 24; // square icon button
const FIND_GAP: usize = 6;
const FIND_RBTN_W: usize = 64; // "Replace"/"All" text buttons

/// Which field of the find bar receives typed characters.
#[derive(Clone, Copy, PartialEq, Eq)]
enum FindField {
    Find,
    Replace,
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

/// Y of the find bar's top edge (just below the toolbar).
fn find_bar_y() -> usize {
    TITLE_H + TOOLBAR_H
}

/// Left x of the find bar (inset from the window edge).
fn find_bar_x() -> usize {
    FIND_PAD
}

/// The Find input rect.
fn find_input_rect() -> Rect {
    Rect {
        x: find_bar_x(),
        y: find_bar_y() + 4,
        w: FIND_INPUT_W,
        h: FIND_ROW_H,
    }
}

/// The icon button at `slot` (0=Aa, 1=`.*` regex toggle, 2=prev, 3=next,
/// 4=close) right of the input.
fn find_btn_rect(slot: usize) -> Rect {
    let x = find_input_rect().x + FIND_INPUT_W + FIND_GAP + slot * (FIND_BTN_W + FIND_GAP);
    Rect {
        x,
        y: find_bar_y() + 4,
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
        y: find_bar_y() + 4 + FIND_ROW_H + 4,
        w: FIND_INPUT_W,
        h: FIND_ROW_H,
    }
}

/// The Replace / Replace-All text button at `slot` (0=Replace, 1=All).
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
        FIND_ROW_H * 2 + 16
    } else {
        FIND_ROW_H + 12
    }
}

/// The Find/Replace bar overlay state. When `active`, the find bar is drawn at
/// the top of the editor pane and captures keyboard input; matches are
/// recomputed from the live buffer whenever the query or buffer changes.
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

// ── Find integration on the buffer (the find UI ⇄ buffer glue) ─────────────

impl Buf {
    /// Recompute the find match set from the live buffer + query, clamping
    /// `current` and jumping the caret to it.
    fn recompute_matches(&mut self, find: &mut FindState) {
        let hay = self.to_string();
        let query = String::from(find.query_str());
        if find.regex_mode {
            match regex_find_matches(&hay, &query) {
                Some(m) => {
                    find.matches = m;
                    find.bad_regex = false;
                }
                None => {
                    find.matches = Vec::new();
                    find.bad_regex = true;
                }
            }
        } else {
            find.bad_regex = false;
            find.matches = find_matches(&hay, &query, find.case_sensitive);
        }
        if find.matches.is_empty() || find.current >= find.matches.len() {
            find.current = 0;
        }
        self.move_caret_to_current(find);
    }

    /// Step to the next (`+1`) or previous (`-1`) match, wrapping around.
    fn step_match(&mut self, find: &mut FindState, dir: i32) {
        let n = find.matches.len();
        if n == 0 {
            return;
        }
        let cur = find.current as i32;
        find.current = (cur + dir).rem_euclid(n as i32) as usize;
        self.move_caret_to_current(find);
    }

    /// Move the caret + scroll to the start of the current match (if any).
    fn move_caret_to_current(&mut self, find: &FindState) {
        let Some(&(start, _end)) = find.matches.get(find.current) else {
            return;
        };
        let (row, col) = self.byte_off_to_rowcol(start);
        self.cursor_row = row;
        self.cursor_col = col;
        let visible = (WIN_H - TITLE_H - TOOLBAR_H - STATUS_H) / LINE_H;
        self.update_scroll(visible.max(1));
    }

    /// Map a byte offset in the materialized buffer string to `(row, col)` (BYTE
    /// column within the line — the grid is byte-pitch for ASCII). Offsets come
    /// from `find_matches` over the SAME `to_string()`, so they land on a line.
    fn byte_off_to_rowcol(&self, off: usize) -> (usize, usize) {
        let mut remaining = off;
        for row in 0..self.nlines {
            let line_len = self.lines[row].len;
            if remaining <= line_len {
                return (row, remaining);
            }
            remaining -= line_len + 1; // +1 for the '\n' to_string() inserts
        }
        let last = self.nlines.saturating_sub(1);
        (last, self.lines[last].len)
    }

    /// Replace the current match with the Replace field text, then keep position.
    /// No-op if there are no matches. Rebuilds the buffer + marks dirty.
    fn replace_current(&mut self, find: &mut FindState) {
        let Some(&range) = find.matches.get(find.current) else {
            return;
        };
        let hay = self.to_string();
        let repl = String::from(find.repl_str());
        let new = if find.regex_mode {
            // Regex-expand `$1` group refs against this one match's slice.
            let query = String::from(find.query_str());
            regex_replace_one_at(&hay, range, &query, &repl)
        } else {
            replace_one_at(&hay, range, &repl)
        };
        let keep = find.current;
        self.load_text(&new);
        self.dirty = true;
        self.recompute_matches(find);
        if !find.matches.is_empty() {
            find.current = keep.min(find.matches.len() - 1);
            self.move_caret_to_current(find);
        }
    }

    /// Replace ALL matches in the buffer. Returns the replacement count (0 = none).
    fn replace_all_matches(&mut self, find: &mut FindState) -> usize {
        let hay = self.to_string();
        let needle = String::from(find.query_str());
        let repl = String::from(find.repl_str());
        let (new, count) = if find.regex_mode {
            regex_replace_all(&hay, &needle, &repl)
        } else {
            replace_all_cs(&hay, &needle, &repl, find.case_sensitive)
        };
        if count == 0 {
            return 0;
        }
        self.load_text(&new);
        self.dirty = true;
        self.recompute_matches(find);
        count
    }

    /// The per-row find-highlight spans on row `row`, as `(col_start, col_end,
    /// is_current)` byte columns. A match spanning newlines is clamped to this
    /// row (never out of range → never panics).
    fn row_match_spans(&self, find: &FindState, row: usize) -> Vec<(usize, usize, bool)> {
        let mut spans = Vec::new();
        if find.matches.is_empty() {
            return spans;
        }
        let mut row_start = 0usize;
        for r in 0..row {
            row_start += self.lines[r].len + 1; // +1 for '\n'
        }
        let row_len = self.lines[row].len;
        let row_end = row_start + row_len;
        for (mi, &(ms, me)) in find.matches.iter().enumerate() {
            let s = ms.max(row_start);
            let e = me.min(row_end);
            if s < e {
                spans.push((s - row_start, e - row_start, mi == find.current));
            }
        }
        spans
    }
}

// ── Mouse hit-testing (draw-rects == hit-rects) ────────────────────────────

/// What a left-click on the find bar maps to (each mirrors a keyboard action).
#[derive(Clone, Copy, PartialEq, Eq)]
enum FindAction {
    FocusFind,
    FocusReplace,
    FindNext,
    FindPrev,
    ToggleCase,
    ToggleRegex,
    ReplaceCurrent,
    ReplaceAll,
    CloseFind,
    None,
}

/// Hit-test a surface-local click against the (active) find bar. Returns the
/// matching action, or `None` for empty space. Builds the SAME rects the bar
/// draws, so a click can never drift from the pixel. Pure / never panics.
fn hit_find_bar(find: &FindState, px: i32, py: i32) -> FindAction {
    if !find.active {
        return FindAction::None;
    }
    if find_input_rect().contains(px, py) {
        return FindAction::FocusFind;
    }
    if find_btn_rect(0).contains(px, py) {
        return FindAction::ToggleCase;
    }
    if find_btn_rect(1).contains(px, py) {
        return FindAction::ToggleRegex;
    }
    if find_btn_rect(2).contains(px, py) {
        return FindAction::FindPrev;
    }
    if find_btn_rect(3).contains(px, py) {
        return FindAction::FindNext;
    }
    if find_btn_rect(4).contains(px, py) {
        return FindAction::CloseFind;
    }
    if find.replace_mode {
        if replace_input_rect().contains(px, py) {
            return FindAction::FocusReplace;
        }
        if replace_btn_rect(0).contains(px, py) {
            return FindAction::ReplaceCurrent;
        }
        if replace_btn_rect(1).contains(px, py) {
            return FindAction::ReplaceAll;
        }
    }
    FindAction::None
}

// ── Scancode → ASCII ────────────────────────────────────────────────────

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

// ── Rendering ───────────────────────────────────────────────────────────

fn render(buf: &Buf, find: &FindState, toast: &Toast, canvas: &mut Canvas) {
    canvas.fill_rect(0, 0, WIN_W, WIN_H, BG);

    // Title bar
    canvas.fill_rect(0, 0, WIN_W, TITLE_H, TITLE_BG);
    let title_w = canvas.draw_text_aa(
        12,
        ((TITLE_H - ath_tokens::TYPE_SUBTITLE.line_height as usize) / 2) as i32,
        "Text Editor — todo.txt",
        ath_tokens::TYPE_SUBTITLE,
        TEXT_FG,
        FontFamily::Sans,
    );
    if buf.dirty {
        canvas.fill_rect(12 + title_w as usize + 6, 12, 5, 5, DIRTY_DOT);
    }
    canvas.fill_rect(WIN_W - 28, 4, 20, 20, DARK.state_danger);
    let x_w = canvas.measure_text_aa("X", ath_tokens::TYPE_LABEL, FontFamily::Sans);
    canvas.draw_text_aa(
        (WIN_W - 18) as i32 - x_w / 2,
        (4 + (20 - ath_tokens::TYPE_LABEL.line_height as usize) / 2) as i32,
        "X",
        ath_tokens::TYPE_LABEL,
        0xFF_FF_FF_FF,
        FontFamily::Sans,
    );

    // Toolbar
    let tb_y = TITLE_H;
    canvas.fill_rect(0, tb_y, WIN_W, TOOLBAR_H, TOOLBAR_BG);
    draw_btn(canvas, 8, tb_y + 4, 56, 24, "New");
    draw_btn(canvas, 72, tb_y + 4, 56, 24, "Open");
    draw_btn(canvas, 136, tb_y + 4, 56, 24, "Save");
    draw_btn(canvas, 200, tb_y + 4, 64, 24, "Reload");

    // Editor area — pushed down by the find bar when it's active.
    let mut ed_y = tb_y + TOOLBAR_H;
    if find.active {
        let fh = find_bar_total_h(find.replace_mode);
        render_find_bar(find, canvas);
        ed_y += fh;
    }
    let ed_h = (WIN_H - STATUS_H).saturating_sub(ed_y);
    canvas.fill_rect(0, ed_y, GUTTER_W, ed_h, GUTTER_BG);
    canvas.fill_rect(GUTTER_W, ed_y, WIN_W - GUTTER_W, ed_h, BG);

    // The editor body is a fixed-pitch 8px monospace grid: the caret lands at
    // `cursor_col * GLYPH_W`, so line text + gutter numbers stay on the 8x8
    // bitmap font (matching the live athshell terminal grid). Only the chrome
    // (title / toolbar / status) is crisp AA. A proportional AA body would
    // desync the caret from the glyphs — out of scope for a text-only migration.
    let visible_rows = ed_h / LINE_H;

    for i in 0..visible_rows {
        let row = buf.scroll + i;
        if row >= buf.nlines {
            break;
        }

        let py = ed_y + i * LINE_H + 2;

        // Find-match highlights (drawn under the glyphs).
        if find.active {
            for (cs, ce, is_current) in buf.row_match_spans(find, row) {
                let hx = GUTTER_W + 6 + cs * GLYPH_W;
                let hw = (ce - cs) * GLYPH_W;
                let fill = if is_current {
                    sel_bg()
                } else {
                    DARK.bg_elevated
                };
                canvas.fill_rect(hx, py, hw.max(1), GLYPH_H + 2, fill);
            }
        }

        let mut num_buf = [0u8; 6];
        let n = fmt_u64((row + 1) as u64, &mut num_buf);
        if let Ok(s) = core::str::from_utf8(&num_buf[..n]) {
            let nx = GUTTER_W - 8 - s.len() * GLYPH_W;
            canvas.draw_text(nx, py, s, LINE_NUM_FG, None);
        }

        let s = buf.lines[row].as_str();
        canvas.draw_text(GUTTER_W + 6, py, s, TEXT_FG, None);

        if row == buf.cursor_row {
            let cx = GUTTER_W + 6 + buf.cursor_col * GLYPH_W;
            canvas.fill_rect(cx, py, 2, GLYPH_H, cursor());
        }
    }

    // Status bar
    let st_y = WIN_H - STATUS_H;
    canvas.fill_rect(0, st_y, WIN_W, STATUS_H, STATUS_BG);
    let st_ty = (st_y + (STATUS_H - ath_tokens::TYPE_CAPTION.line_height as usize) / 2) as i32;
    let mut pos_buf = [0u8; 32];
    let n = fmt_pos(buf.cursor_row + 1, buf.cursor_col + 1, &mut pos_buf);
    if let Ok(s) = core::str::from_utf8(&pos_buf[..n]) {
        canvas.draw_text_aa(
            12,
            st_ty,
            s,
            ath_tokens::TYPE_CAPTION,
            TEXT_DIM,
            FontFamily::Sans,
        );
    }
    let modified = if buf.dirty { "modified" } else { "saved" };
    canvas.draw_text_aa(
        140,
        st_ty,
        modified,
        ath_tokens::TYPE_CAPTION,
        TEXT_DIM,
        FontFamily::Sans,
    );
    // A live toast (error / op result) replaces the key-hint on the right.
    if toast.len > 0 {
        let tw = canvas.measure_text_aa(toast.as_str(), ath_tokens::TYPE_CAPTION, FontFamily::Sans);
        canvas.draw_text_aa(
            (WIN_W - 12) as i32 - tw,
            st_ty,
            toast.as_str(),
            ath_tokens::TYPE_CAPTION,
            DARK.state_warn,
            FontFamily::Sans,
        );
    } else {
        let hint = "Ctrl+O:open  Ctrl+F:find  Ctrl+S:save  Ctrl+Shift+S:save as";
        let hint_w = canvas.measure_text_aa(hint, ath_tokens::TYPE_CAPTION, FontFamily::Sans);
        canvas.draw_text_aa(
            (WIN_W - 12) as i32 - hint_w,
            st_ty,
            hint,
            ath_tokens::TYPE_CAPTION,
            TEXT_DIM,
            FontFamily::Sans,
        );
    }
}

/// Draw the Find/Replace overlay bar: a Find input, [Aa]/[<]/[>]/[x] buttons, an
/// "N of M" counter, and (in replace mode) a Replace input + Replace/All
/// buttons. Geometry mirrors the `find_*_rect` helpers `hit_find_bar` tests.
fn render_find_bar(find: &FindState, canvas: &mut Canvas) {
    let bar_h = find_bar_total_h(find.replace_mode);
    canvas.fill_rect(0, find_bar_y(), WIN_W, bar_h, TOOLBAR_BG);
    canvas.fill_rect(0, find_bar_y() + bar_h, WIN_W, 1, DARK.stroke_strong);

    // Find input box.
    let fr = find_input_rect();
    draw_find_input(
        canvas,
        fr,
        find.query_str(),
        find.field == FindField::Find,
        "Find",
    );

    // [Aa] case toggle (active-shown only when not in regex mode, where it's
    // ignored), [.*] regex toggle, [<] prev, [>] next, [x] close.
    draw_find_button(
        canvas,
        find_btn_rect(0),
        "Aa",
        find.case_sensitive && !find.regex_mode,
    );
    draw_find_button(canvas, find_btn_rect(1), ".*", find.regex_mode);
    draw_find_button(canvas, find_btn_rect(2), "<", false);
    draw_find_button(canvas, find_btn_rect(3), ">", false);
    draw_find_button(canvas, find_btn_rect(4), "x", false);

    // "N of M" counter — or "Bad regex" when a malformed pattern is typed.
    let ty = (find_btn_rect(4).y
        + (FIND_ROW_H.saturating_sub(ath_tokens::TYPE_CAPTION.line_height as usize)) / 2)
        as i32;
    if find.regex_mode && find.bad_regex {
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
            if find.matches.is_empty() {
                0
            } else {
                find.current + 1
            },
            find.matches.len(),
            &mut cbuf,
        );
        if let Ok(s) = core::str::from_utf8(&cbuf[..cn]) {
            let fg = if find.matches.is_empty() {
                LINE_NUM_FG
            } else {
                TEXT_DIM
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
    if find.replace_mode {
        let rr = replace_input_rect();
        draw_find_input(
            canvas,
            rr,
            find.repl_str(),
            find.field == FindField::Replace,
            "Replace with",
        );
        draw_find_button(canvas, replace_btn_rect(0), "Replace", false);
        draw_find_button(canvas, replace_btn_rect(1), "All", false);
    }
}

/// Draw one find/replace input box (fill, focus accent ring, text or dimmed
/// placeholder, and a caret when focused).
fn draw_find_input(canvas: &mut Canvas, r: Rect, text: &str, focused: bool, placeholder: &str) {
    canvas.fill_rect(r.x, r.y, r.w, r.h, DARK.bg_base);
    if focused {
        let ring = cursor();
        canvas.fill_rect(r.x, r.y, r.w, 1, ring);
        canvas.fill_rect(r.x, r.y + r.h - 1, r.w, 1, ring);
        canvas.fill_rect(r.x, r.y, 1, r.h, ring);
        canvas.fill_rect(r.x + r.w - 1, r.y, 1, r.h, ring);
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
            LINE_NUM_FG,
            FontFamily::Sans,
        );
    } else {
        let tw = canvas.measure_text_aa(text, ath_tokens::TYPE_LABEL, FontFamily::Sans);
        canvas.draw_text_aa(
            (r.x + 8) as i32,
            ty,
            text,
            ath_tokens::TYPE_LABEL,
            TEXT_FG,
            FontFamily::Sans,
        );
        if focused {
            let cx = (r.x + 8 + (tw as usize).min(r.w.saturating_sub(16))) as i32;
            canvas.fill_rect(
                cx.max(0) as usize,
                r.y + 5,
                2,
                r.h.saturating_sub(10),
                cursor(),
            );
        }
    }
}

/// Draw a find-bar button (icon/label). `active` highlights it (case toggle on).
fn draw_find_button(canvas: &mut Canvas, r: Rect, label: &str, active: bool) {
    let fill = if active { sel_bg() } else { DARK.bg_elevated };
    canvas.fill_rect(r.x, r.y, r.w, r.h, fill);
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
        if active { TEXT_FG } else { TEXT_DIM },
        FontFamily::Sans,
    );
}

fn draw_btn(canvas: &mut Canvas, x: usize, y: usize, w: usize, h: usize, label: &str) {
    canvas.fill_rect(x, y, w, h, DARK.bg_elevated);
    let label_w = canvas.measure_text_aa(label, ath_tokens::TYPE_LABEL, FontFamily::Sans);
    let tx = x as i32 + (w as i32 - label_w) / 2;
    let ty = (y + (h.saturating_sub(ath_tokens::TYPE_LABEL.line_height as usize)) / 2) as i32;
    canvas.draw_text_aa(
        tx,
        ty,
        label,
        ath_tokens::TYPE_LABEL,
        TEXT_FG,
        FontFamily::Sans,
    );
}

// ── Open / Save-As dialog geometry, hit-test, render ───────────────────────
//
// The dialogs are centered modal cards drawn OVER the editor (the editor render
// runs first, then the dialog on top). Rects are computed from the SAME
// constants the draw fns use, so a click can't drift from the pixel (the find-bar
// pattern). Keyboard is the must; the mouse mirrors it.

const DLG_W: usize = 420;
const DLG_ROW_H: usize = 18;
const DLG_HEADER_H: usize = 56; // title + path header
const DLG_FOOTER_H: usize = 26; // hint line
const DLG_VISIBLE_ROWS: usize = 12;

/// The toolbar "Open" button rect (mirrors the `draw_btn` call in `render`).
fn toolbar_open_rect() -> Rect {
    Rect {
        x: 72,
        y: TITLE_H + 4,
        w: 56,
        h: 24,
    }
}
/// The toolbar "Save" button rect.
fn toolbar_save_rect() -> Rect {
    Rect {
        x: 136,
        y: TITLE_H + 4,
        w: 56,
        h: 24,
    }
}
/// True iff a surface-local click hits the toolbar "Open" button.
fn hit_toolbar_open(px: i32, py: i32) -> bool {
    toolbar_open_rect().contains(px, py)
}
/// True iff a surface-local click hits the toolbar "Save" button.
fn hit_toolbar_save(px: i32, py: i32) -> bool {
    toolbar_save_rect().contains(px, py)
}

/// The Open dialog card rect (centered in the window).
fn dlg_card() -> Rect {
    let h = DLG_HEADER_H + DLG_VISIBLE_ROWS * DLG_ROW_H + DLG_FOOTER_H + 8;
    Rect {
        x: (WIN_W - DLG_W) / 2,
        y: (WIN_H.saturating_sub(h)) / 2,
        w: DLG_W,
        h,
    }
}

/// The rect of the list row at visible index `i` (0 = first visible row).
fn dlg_row_rect(i: usize) -> Rect {
    let card = dlg_card();
    Rect {
        x: card.x + 8,
        y: card.y + DLG_HEADER_H + i * DLG_ROW_H,
        w: DLG_W - 16,
        h: DLG_ROW_H,
    }
}

/// What an Open-dialog click maps to: select+activate a row, or close.
enum OpenClick {
    Row(usize), // absolute row index (0 = "..")
    Close,
    None,
}

/// Hit-test a surface-local click against the (active) Open dialog. A click on a
/// visible list row returns that absolute row index; outside the card → Close.
fn hit_open_dialog(dlg: &OpenDialog, px: i32, py: i32) -> OpenClick {
    if !dlg.active {
        return OpenClick::None;
    }
    if !dlg_card().contains(px, py) {
        return OpenClick::Close;
    }
    for i in 0..DLG_VISIBLE_ROWS {
        let abs = dlg.scroll + i;
        if abs >= dlg.row_count() {
            break;
        }
        if dlg_row_rect(i).contains(px, py) {
            return OpenClick::Row(abs);
        }
    }
    OpenClick::None
}

/// Render the Open dialog card (dim wash + card + path header + scrolled rows).
fn render_open_dialog(dlg: &OpenDialog, canvas: &mut Canvas) {
    // Dim the editor behind the modal (deepest layer as a solid scrim).
    canvas.fill_rect(0, 0, WIN_W, WIN_H, DARK.bg_base);
    let card = dlg_card();
    canvas.fill_rect(card.x, card.y, card.w, card.h, DARK.bg_overlay);
    canvas.fill_rect(card.x, card.y, card.w, 1, DARK.stroke_strong);

    // Title + current path header.
    canvas.draw_text_aa(
        (card.x + 12) as i32,
        (card.y + 8) as i32,
        "Open file",
        ath_tokens::TYPE_SUBTITLE,
        TEXT_FG,
        FontFamily::Sans,
    );
    canvas.draw_text_aa(
        (card.x + 12) as i32,
        (card.y + 30) as i32,
        dlg.dir.as_str(),
        ath_tokens::TYPE_CAPTION,
        TEXT_DIM,
        FontFamily::Sans,
    );

    for i in 0..DLG_VISIBLE_ROWS {
        let abs = dlg.scroll + i;
        if abs >= dlg.row_count() {
            break;
        }
        let r = dlg_row_rect(i);
        if abs == dlg.sel {
            canvas.fill_rect(r.x, r.y, r.w, r.h, sel_bg());
        }
        // Row 0 is the synthetic ".." parent; the rest index `entries`.
        let (label, is_dir) = if abs == 0 {
            ("..", true)
        } else {
            let e = &dlg.entries[abs - 1];
            (e.name_str(), e.is_dir)
        };
        let icon = if is_dir { "[D]" } else { "   " };
        let fg = if is_dir { TEXT_FG } else { TEXT_DIM };
        canvas.draw_text(
            (r.x + 6) as usize,
            (r.y + 5) as usize,
            icon,
            LINE_NUM_FG,
            None,
        );
        canvas.draw_text((r.x + 34) as usize, (r.y + 5) as usize, label, fg, None);
    }

    let hint = "Up/Down  Enter:open  Esc:cancel";
    canvas.draw_text_aa(
        (card.x + 12) as i32,
        (card.y + card.h - DLG_FOOTER_H + 4) as i32,
        hint,
        ath_tokens::TYPE_CAPTION,
        LINE_NUM_FG,
        FontFamily::Sans,
    );
}

/// The Save-As card rect (a smaller centered card with one filename field).
fn saveas_card() -> Rect {
    Rect {
        x: (WIN_W - DLG_W) / 2,
        y: (WIN_H.saturating_sub(120)) / 2,
        w: DLG_W,
        h: 120,
    }
}

/// The Save-As filename input rect.
fn saveas_input_rect() -> Rect {
    let card = saveas_card();
    Rect {
        x: card.x + 12,
        y: card.y + 52,
        w: card.w - 24,
        h: FIND_ROW_H,
    }
}

/// Render the Save-As card (dim wash + card + dir header + filename field).
fn render_saveas_dialog(dlg: &SaveAsDialog, canvas: &mut Canvas) {
    canvas.fill_rect(0, 0, WIN_W, WIN_H, DARK.bg_base);
    let card = saveas_card();
    canvas.fill_rect(card.x, card.y, card.w, card.h, DARK.bg_overlay);
    canvas.fill_rect(card.x, card.y, card.w, 1, DARK.stroke_strong);

    canvas.draw_text_aa(
        (card.x + 12) as i32,
        (card.y + 8) as i32,
        "Save As",
        ath_tokens::TYPE_SUBTITLE,
        TEXT_FG,
        FontFamily::Sans,
    );
    canvas.draw_text_aa(
        (card.x + 12) as i32,
        (card.y + 30) as i32,
        dlg.dir.as_str(),
        ath_tokens::TYPE_CAPTION,
        TEXT_DIM,
        FontFamily::Sans,
    );
    draw_find_input(
        canvas,
        saveas_input_rect(),
        dlg.name_str(),
        true,
        "filename",
    );
    canvas.draw_text_aa(
        (card.x + 12) as i32,
        (card.y + card.h - 22) as i32,
        "Enter:save  Esc:cancel",
        ath_tokens::TYPE_CAPTION,
        LINE_NUM_FG,
        FontFamily::Sans,
    );
}

fn fmt_pos(row: usize, col: usize, out: &mut [u8]) -> usize {
    let label = b"Ln ";
    let mut n = 0;
    for &b in label {
        out[n] = b;
        n += 1;
    }
    n += fmt_u64(row as u64, &mut out[n..]);
    let mid = b", Col ";
    for &b in mid {
        out[n] = b;
        n += 1;
    }
    n += fmt_u64(col as u64, &mut out[n..]);
    n
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
        out[n] = tmp[i];
        n += 1;
    }
    n
}

// ── Design proof (R10: a fail-able check the token wiring is correct) ─────
//
// `cargo test` can't run a libtest harness in this `#![no_main]` bin (athkit's
// `#[panic_handler]` + std's = duplicate lang item). This pure proof is the
// fail-able authority for BOTH the `ath_tokens` wiring (the ramp is host-KAT'd
// by `cargo test -p ath_tokens`) AND the find/replace core logic.

/// True iff the Text Editor's generic chrome is wired to the shared tokens AND
/// the find/replace core logic holds.
#[must_use]
pub fn design_proof() -> bool {
    let ramp = ath_tokens::derive_accent(theme_seed(), &DARK);
    let tokens_ok = cursor() == ramp.base
        && sel_bg() == ramp.active
        && BG == DARK.bg_raised
        && TITLE_BG == DARK.bg_base
        && TOOLBAR_BG == DARK.bg_overlay
        && TEXT_FG == DARK.text_primary
        && TEXT_DIM == DARK.text_secondary
        && LINE_NUM_FG == DARK.text_tertiary
        && DIRTY_DOT == DARK.state_warn
        && athkit::sys::THEME_DEFAULT_ACCENT == RAEBLUE;

    tokens_ok && find_replace_proof() && prefs_round_trip_ok() && dialog_proof()
}

/// Prove the Open/Save-As dialog CORE logic (the load-bearing decode/join/sort,
/// the parts the live `readdir_at`/QEMU path can't fail-ably assert at boot).
/// Fail-able (→ exit(3)): a synthetic `readdir`-format buffer decodes to the
/// expected (name, is_dir) entries with dirs sorted first; `DlgPath` path-join +
/// parent handling is correct and char-boundary-safe; `dir_of` splits at the
/// right '/'.
#[must_use]
fn dialog_proof() -> bool {
    // (a) Decode a synthetic readdir buffer: a dir "docs" (size 0, no dot), a file
    //     "a.txt" (has a dot), and a file "bin" (size>0). Record =
    //     [name_len:u16 LE][size:u32 LE][name bytes].
    fn rec(out: &mut Vec<u8>, name: &str, size: u32) {
        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(name.as_bytes());
    }
    let mut raw = Vec::new();
    rec(&mut raw, "zeta", 0); // dir (size 0, no dot)
    rec(&mut raw, "a.txt", 12); // file
    rec(&mut raw, "docs", 0); // dir
    rec(&mut raw, "bin", 999); // file (size>0 → file even with no dot)
    rec(&mut raw, ".", 0); // self-link, must be skipped
    rec(&mut raw, "..", 0); // parent-link, must be skipped
    let empty = DirEntry {
        name: [0; 48],
        name_len: 0,
        is_dir: false,
    };
    let mut out = [empty; DLG_MAX_ENTRIES];
    let n = decode_readdir(&raw, 6, &mut out);
    if n != 4 {
        return false;
    }
    // After sort: dirs first (alpha: "docs","zeta"), then files (alpha:
    // "a.txt","bin").
    if !out[0].is_dir || out[0].name_str() != "docs" {
        return false;
    }
    if !out[1].is_dir || out[1].name_str() != "zeta" {
        return false;
    }
    if out[2].is_dir || out[2].name_str() != "a.txt" {
        return false;
    }
    if out[3].is_dir || out[3].name_str() != "bin" {
        return false;
    }

    // (b) Path-join: "/home/u" + "notes.txt" == "/home/u/notes.txt", and joining
    //     onto a dir that already ends in '/' doesn't double the separator.
    let mut p = DlgPath::new();
    p.set("/home/u");
    p.push_component("notes.txt");
    if p.as_str() != "/home/u/notes.txt" {
        return false;
    }
    let mut p2 = DlgPath::new();
    p2.set("/home/u/");
    p2.push_component("notes.txt");
    if p2.as_str() != "/home/u/notes.txt" {
        return false;
    }

    // (c) Parent-of handling: pop_component drops the leaf; from root stays "/".
    let mut p3 = DlgPath::new();
    p3.set("/home/u/notes.txt");
    p3.pop_component();
    if p3.as_str() != "/home/u" {
        return false;
    }
    p3.pop_component();
    if p3.as_str() != "/home" {
        return false;
    }
    let mut root = DlgPath::new();
    root.set("/");
    root.pop_component();
    if root.as_str() != "/" {
        return false;
    }

    // (d) dir_of: the directory of a path is everything before the last '/'; a
    //     bare filename has no '/' (falls back to home — just assert it's
    //     non-empty + absolute); a "/file" at root yields "/".
    if dir_of("/home/u/notes.txt").as_str() != "/home/u" {
        return false;
    }
    if dir_of("/file.txt").as_str() != "/" {
        return false;
    }
    let bare = dir_of("todo.txt");
    if !bare.as_str().starts_with('/') {
        return false;
    }

    // (e) Char-boundary safety: a multibyte filename joins whole and `as_str`
    //     never splits a codepoint.
    let mut pu = DlgPath::new();
    pu.set("/home");
    pu.push_component("café.txt");
    if pu.as_str() != "/home/café.txt" {
        return false;
    }

    // (f) entry_less ordering: dir < file regardless of name.
    let mut da = empty;
    da.is_dir = true;
    da.name[..3].copy_from_slice(b"zzz");
    da.name_len = 3;
    let mut fa = empty;
    fa.is_dir = false;
    fa.name[..3].copy_from_slice(b"aaa");
    fa.name_len = 3;
    if !entry_less(&da, &fa) || entry_less(&fa, &da) {
        return false;
    }

    true
}

/// Prove the Text Editor PREFS SCHEMA: a known non-default `Prefs` serialized via
/// `ath_toml` then re-parsed restores every field exactly (find-bar case + regex
/// toggles, last-opened file path), AND a corrupt / missing-key document resolves
/// to the typed defaults (NOT a panic, NOT a wrong value). Returns `false` on any
/// drift (→ exit(3) at startup).
#[must_use]
fn prefs_round_trip_ok() -> bool {
    // (a) Full round-trip of a non-default Prefs.
    let p = Prefs {
        case_sensitive: true,
        regex_mode: true,
        last_file: String::from("notes.txt"),
    };
    let text = ath_toml::to_string(&p.to_toml());
    let parsed = match ath_toml::parse(&text) {
        Ok(t) => t,
        Err(_) => return false,
    };
    let back = Prefs::from_toml(&parsed);
    if !back.case_sensitive || !back.regex_mode || back.last_file != "notes.txt" {
        return false;
    }

    // (b) A corrupt document → typed defaults (parse FAILS, we don't panic).
    let corrupt = "regex_mode = = oops\n[unterminated\n";
    let d = match ath_toml::parse(corrupt) {
        Ok(t) => Prefs::from_toml(&t), // shouldn't reach here for this input
        Err(_) => Prefs::defaults(),
    };
    if d.case_sensitive || d.regex_mode || !d.last_file.is_empty() {
        return false;
    }

    // (c) A well-formed doc MISSING every prefs key → typed defaults per field.
    let empty = match ath_toml::parse("unrelated = 1\n") {
        Ok(t) => t,
        Err(_) => return false,
    };
    let e = Prefs::from_toml(&empty);
    if e.case_sensitive || e.regex_mode || !e.last_file.is_empty() {
        return false;
    }

    // (d) A wrong-TYPED field (regex_mode as a string) is ignored → default, not
    // a crash; an unrelated valid key still parses.
    let wrong = match ath_toml::parse("regex_mode = \"yes\"\nlast_file = \"x.txt\"\n") {
        Ok(t) => t,
        Err(_) => return false,
    };
    let w = Prefs::from_toml(&wrong);
    if w.regex_mode || w.last_file != "x.txt" {
        return false;
    }

    true
}

/// Prove the Find/Replace core logic (the load-bearing search/replace fns), the
/// exact assertions Notes uses. Fail-able (→ exit(3)): case-insensitive vs
/// case-sensitive match counts/offsets, `replace_all` count + result,
/// `replace_one_at` scoping, the no-match / empty-needle no-ops, and the UTF-8
/// char-boundary safety case.
#[must_use]
fn find_replace_proof() -> bool {
    // (1) Case-insensitive: "the" matches twice in a mixed-case haystack.
    let hay = "the cat sat on the mat";
    let ci = find_matches(hay, "the", false);
    if ci.len() != 2 || ci[0] != (0, 3) || ci[1] != (15, 18) {
        return false;
    }
    if &hay[ci[0].0..ci[0].1] != "the" || &hay[ci[1].0..ci[1].1] != "the" {
        return false;
    }

    // (2) Case-sensitive differs: in "The cat sat on the mat", "The" matches once.
    let hay2 = "The cat sat on the mat";
    let cs = find_matches(hay2, "The", true);
    if cs.len() != 1 || cs[0] != (0, 3) {
        return false;
    }
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

    // (8) UTF-8 char-boundary safety: a multibyte char is never split mid-
    // codepoint. "café au lait" (é = 2 bytes at 3..5).
    let uni = "café au lait";
    let m = find_matches(uni, "caf", false);
    if m.len() != 1 || m[0] != (0, 3) || &uni[m[0].0..m[0].1] != "caf" {
        return false;
    }
    let me = find_matches(uni, "é", false);
    if me.len() != 1 || &uni[me[0].0..me[0].1] != "é" {
        return false;
    }
    let (uo, un2) = replace_all(uni, "é", "e");
    if un2 != 1 || uo != "cafe au lait" {
        return false;
    }
    // A 1-byte ASCII needle must not false-match inside a multibyte char.
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
    if &"ab12cd345"[m[0].0..m[0].1] != "12" || &"ab12cd345"[m[1].0..m[1].1] != "345" {
        return false;
    }

    // (b) Regex replace with a group swap: "(\w)(\w)" on "ab" with "$2$1" → "ba".
    let (out, n) = regex_replace_all("ab", "(\\w)(\\w)", "$2$1");
    if n != 1 || out != "ba" {
        return false;
    }

    // (c) Regex replace-one bounded to the current match: replace just (2,4) of
    // "ab12cd" (the "12") with a literal "#" → "ab#cd".
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

/// Render the full frame: the editor body + find bar, then (when active) the
/// Open or Save-As modal card ON TOP. One entry point so every present is a
/// complete, consistent frame.
fn render_all(
    buf: &Buf,
    find: &FindState,
    open_dlg: &OpenDialog,
    saveas_dlg: &SaveAsDialog,
    toast: &Toast,
    canvas: &mut Canvas,
) {
    render(buf, find, toast, canvas);
    if open_dlg.active {
        render_open_dialog(open_dlg, canvas);
    } else if saveas_dlg.active {
        render_saveas_dialog(saveas_dlg, canvas);
    }
}

/// Load the file at absolute `path` into the buffer, rebinding `last_file` and
/// persisting it, with a DIRTY GUARD: if the buffer has unsaved edits, the
/// current file is auto-saved FIRST so nothing is lost. Reports the outcome via
/// `toast`. Never panics (a read failure leaves the buffer untouched).
fn do_open_file(buf: &mut Buf, find: &FindState, path: &str, toast: &mut Toast) {
    // Dirty guard: persist the current buffer to its bound file before switching.
    if buf.dirty {
        buf.save();
    }
    match read_file_to_string(path) {
        Some(text) => {
            buf.load_text(&text);
            buf.last_file = String::from(path);
            buf.dirty = false;
            buf.cursor_row = 0;
            buf.cursor_col = 0;
            buf.scroll = 0;
            persist_prefs(buf, find);
            toast.set("Opened");
        }
        None => {
            toast.set("Open failed");
        }
    }
}

/// Act on the currently-selected Open-dialog row: row 0 ("..") goes to the
/// parent dir; a directory row enters it; a file row loads it (with the dirty
/// guard) and closes the dialog. Never panics.
fn activate_open_row(buf: &mut Buf, find: &FindState, dlg: &mut OpenDialog, toast: &mut Toast) {
    if dlg.sel == 0 {
        dlg.go_up();
        return;
    }
    let idx = dlg.sel - 1;
    if idx >= dlg.n {
        return;
    }
    if dlg.entries[idx].is_dir {
        dlg.enter_dir(idx);
    } else {
        // Build the absolute file path, then load it.
        let mut path = dlg.dir;
        let mut name = [0u8; 48];
        let len = dlg.entries[idx].name_len.min(48);
        name[..len].copy_from_slice(&dlg.entries[idx].name[..len]);
        if let Ok(s) = core::str::from_utf8(&name[..len]) {
            path.push_component(s);
            // Copy out of the borrow before mutating `buf`/persisting.
            let mut full = [0u8; DLG_PATH_CAP];
            let pn = path.as_str().as_bytes().len().min(DLG_PATH_CAP);
            full[..pn].copy_from_slice(&path.as_str().as_bytes()[..pn]);
            if let Ok(ps) = core::str::from_utf8(&full[..pn]) {
                do_open_file(buf, find, ps, toast);
            }
        }
        dlg.active = false;
    }
}

/// Save the buffer to absolute `path` (Save-As), rebinding `last_file` so future
/// Ctrl+S writes there. Reports via `toast`. Never panics.
fn do_save_as(buf: &mut Buf, find: &FindState, path: &str, toast: &mut Toast) {
    buf.last_file = String::from(path);
    buf.save();
    if buf.dirty {
        toast.set("Save failed");
    } else {
        persist_prefs(buf, find);
        toast.set("Saved");
    }
}

// ── Entry point ─────────────────────────────────────────────────────────

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

    // Restore saved preferences (typed defaults on first run / any error).
    let prefs = load_prefs();
    let mut buf = Buf::new();
    let mut find = FindState::new();
    find.case_sensitive = prefs.case_sensitive;
    find.regex_mode = prefs.regex_mode;

    // Re-open the last file if it was persisted AND still exists; on a miss fall
    // back to the welcome buffer (never panics, always lands on a usable buffer).
    let mut opened = false;
    if !prefs.last_file.is_empty() {
        if let Some(text) = read_file_to_string(&prefs.last_file) {
            buf.load_text(&text);
            buf.last_file = String::from(prefs.last_file.as_str());
            buf.dirty = false;
            opened = true;
        }
    }
    if !opened {
        buf.seed_welcome();
    }

    let mut open_dlg = OpenDialog::new();
    let mut saveas_dlg = SaveAsDialog::new();
    let mut toast = Toast::empty();

    render_all(&buf, &find, &open_dlg, &saveas_dlg, &toast, &mut canvas);
    athkit::sys::surface_present(sid, PRESENT_X as u64, PRESENT_Y as u64);

    let mut extended = false;
    let mut left_was_down = false;
    let visible_rows = (WIN_H - TITLE_H - TOOLBAR_H - STATUS_H) / LINE_H;

    loop {
        // ── Mouse: drain button events; hit-test the cursor on a click edge ──
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
                let (ox, oy) = athkit::sys::surface_origin(sid)
                    .unwrap_or((PRESENT_X as u32, PRESENT_Y as u32));
                let lx = (cx as i32).saturating_sub(ox as i32);
                let ly = (cy as i32).saturating_sub(oy as i32);
                let mut changed = false;
                if open_dlg.active {
                    // Open-dialog clicks: a row activates it (enter dir / open
                    // file); outside the card closes the dialog.
                    match hit_open_dialog(&open_dlg, lx, ly) {
                        OpenClick::Row(abs) => {
                            open_dlg.sel = abs;
                            activate_open_row(&mut buf, &find, &mut open_dlg, &mut toast);
                            changed = true;
                        }
                        OpenClick::Close => {
                            open_dlg.active = false;
                            changed = true;
                        }
                        OpenClick::None => {}
                    }
                } else if saveas_dlg.active {
                    // A click outside the Save-As card cancels it.
                    if !saveas_card().contains(lx, ly) {
                        saveas_dlg.active = false;
                        changed = true;
                    }
                } else if find.active {
                    let action = hit_find_bar(&find, lx, ly);
                    changed = dispatch_find_click(&mut buf, &mut find, action);
                } else if hit_toolbar_open(lx, ly) {
                    // Toolbar "Open" button → launch the Open dialog.
                    let start = if buf.last_file.is_empty() {
                        home_dir()
                    } else {
                        dir_of(&buf.last_file)
                    };
                    open_dlg.open_in(start.as_str());
                    changed = true;
                } else if hit_toolbar_save(lx, ly) {
                    // Toolbar "Save" button → write the bound file.
                    buf.save();
                    persist_prefs(&buf, &find);
                    toast.set("Saved");
                    changed = true;
                }
                if changed {
                    render_all(&buf, &find, &open_dlg, &saveas_dlg, &toast, &mut canvas);
                    athkit::sys::surface_present(sid, PRESENT_X as u64, PRESENT_Y as u64);
                }
            }
            left_was_down = left_down;
        }

        let key = athkit::sys::read_key();
        if key == 0 {
            // Age out a transient toast; re-render once when it clears.
            if toast.len > 0 {
                toast.tick();
                if toast.len == 0 {
                    render_all(&buf, &find, &open_dlg, &saveas_dlg, &toast, &mut canvas);
                    athkit::sys::surface_present(sid, PRESENT_X as u64, PRESENT_Y as u64);
                }
            }
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

        if code == 0x2A || code == 0x36 {
            buf.shift = !release;
            continue;
        }
        if code == 0x1D {
            buf.ctrl = !release;
            continue;
        }
        if release {
            continue;
        }

        let mut dirty = false;

        // ── Open dialog: when active, it captures ALL keyboard input ─────────
        if open_dlg.active {
            match (ext, code) {
                (false, 0x01) => open_dlg.active = false, // Esc cancels
                (true, 0x48) | (false, 0x48) => open_dlg.move_sel(-1, DLG_VISIBLE_ROWS), // Up
                (true, 0x50) | (false, 0x50) => open_dlg.move_sel(1, DLG_VISIBLE_ROWS), // Down
                (_, 0x1C) => {
                    // Enter: activate the selected row.
                    activate_open_row(&mut buf, &find, &mut open_dlg, &mut toast);
                }
                _ => {}
            }
            render_all(&buf, &find, &open_dlg, &saveas_dlg, &toast, &mut canvas);
            athkit::sys::surface_present(sid, PRESENT_X as u64, PRESENT_Y as u64);
            continue;
        }

        // ── Save-As dialog: a filename text field captures keyboard input ────
        if saveas_dlg.active {
            match (ext, code) {
                (false, 0x01) => saveas_dlg.active = false, // Esc cancels
                (false, 0x1C) => {
                    // Enter: save (only if a non-empty filename was typed).
                    if saveas_dlg.name_len > 0 {
                        let path = saveas_dlg.full_path();
                        let mut full = [0u8; DLG_PATH_CAP];
                        let pn = path.as_str().as_bytes().len().min(DLG_PATH_CAP);
                        full[..pn].copy_from_slice(&path.as_str().as_bytes()[..pn]);
                        if let Ok(ps) = core::str::from_utf8(&full[..pn]) {
                            do_save_as(&mut buf, &find, ps, &mut toast);
                        }
                        saveas_dlg.active = false;
                    }
                }
                _ => {
                    if let Some(ch) = scancode_to_ascii(code, buf.shift) {
                        match ch {
                            0x08 => saveas_dlg.backspace(),
                            c if c >= 0x20 && c < 0x7F => saveas_dlg.push_char(c),
                            _ => {}
                        }
                    }
                }
            }
            render_all(&buf, &find, &open_dlg, &saveas_dlg, &toast, &mut canvas);
            athkit::sys::surface_present(sid, PRESENT_X as u64, PRESENT_Y as u64);
            continue;
        }

        // ── Find bar: when active, it captures keyboard input first ──────────
        if find.active {
            if buf.ctrl {
                match code {
                    0x21 => {
                        // Ctrl+F (again): focus the Find field.
                        find.field = FindField::Find;
                        dirty = true;
                    }
                    0x23 => {
                        // Ctrl+H: reveal replace row, focus it.
                        find.replace_mode = true;
                        find.field = FindField::Replace;
                        dirty = true;
                    }
                    0x17 => {
                        // Ctrl+I: toggle case-sensitivity.
                        find.case_sensitive = !find.case_sensitive;
                        buf.recompute_matches(&mut find);
                        persist_prefs(&buf, &find);
                        dirty = true;
                    }
                    0x13 => {
                        // Ctrl+R: toggle regex mode.
                        find.regex_mode = !find.regex_mode;
                        buf.recompute_matches(&mut find);
                        persist_prefs(&buf, &find);
                        dirty = true;
                    }
                    _ => {}
                }
                if dirty {
                    render_all(&buf, &find, &open_dlg, &saveas_dlg, &toast, &mut canvas);
                    athkit::sys::surface_present(sid, PRESENT_X as u64, PRESENT_Y as u64);
                }
                continue;
            }
            match (ext, code) {
                (false, 0x01) => {
                    // Esc closes the find bar.
                    find.active = false;
                    find.matches.clear();
                    find.current = 0;
                    dirty = true;
                }
                (false, 0x0F) => {
                    // Tab switches focus between Find and Replace (if shown).
                    if find.replace_mode {
                        find.field = match find.field {
                            FindField::Find => FindField::Replace,
                            FindField::Replace => FindField::Find,
                        };
                        dirty = true;
                    }
                }
                (false, 0x1C) => {
                    // Enter = next match (Shift+Enter = previous).
                    if buf.shift {
                        buf.step_match(&mut find, -1);
                    } else {
                        buf.step_match(&mut find, 1);
                    }
                    dirty = true;
                }
                (true, 0x48) => {
                    // Up = previous match.
                    buf.step_match(&mut find, -1);
                    dirty = true;
                }
                (true, 0x50) => {
                    // Down = next match.
                    buf.step_match(&mut find, 1);
                    dirty = true;
                }
                _ => {
                    if let Some(ch) = scancode_to_ascii(code, buf.shift) {
                        match ch {
                            0x08 => {
                                find.backspace_field();
                                buf.recompute_matches(&mut find);
                                dirty = true;
                            }
                            c if c >= 0x20 && c < 0x7F => {
                                find.push_char(c);
                                buf.recompute_matches(&mut find);
                                dirty = true;
                            }
                            _ => {}
                        }
                    }
                }
            }
            if dirty {
                render_all(&buf, &find, &open_dlg, &saveas_dlg, &toast, &mut canvas);
                athkit::sys::surface_present(sid, PRESENT_X as u64, PRESENT_Y as u64);
            }
            continue;
        }

        match (ext, code) {
            (false, 0x21) if buf.ctrl => {
                // Ctrl+F = open Find bar.
                find.active = true;
                find.replace_mode = false;
                find.field = FindField::Find;
                buf.recompute_matches(&mut find);
                dirty = true;
            }
            (false, 0x23) if buf.ctrl => {
                // Ctrl+H = open Find+Replace bar.
                find.active = true;
                find.replace_mode = true;
                find.field = FindField::Replace;
                buf.recompute_matches(&mut find);
                dirty = true;
            }
            (false, 0x18) if buf.ctrl => {
                // Ctrl+O = open the Open-file dialog, starting in the bound file's
                // directory (or <home> when there's no bound file / it's bare).
                let start = if buf.last_file.is_empty() {
                    home_dir()
                } else {
                    dir_of(&buf.last_file)
                };
                open_dlg.open_in(start.as_str());
                dirty = true;
            }
            (true, 0x4B) => {
                buf.move_left();
                dirty = true;
            }
            (true, 0x4D) => {
                buf.move_right();
                dirty = true;
            }
            (true, 0x48) => {
                buf.move_up();
                dirty = true;
            }
            (true, 0x50) => {
                buf.move_down();
                dirty = true;
            }
            (true, 0x47) => {
                buf.home();
                dirty = true;
            }
            (true, 0x4F) => {
                buf.end();
                dirty = true;
            }
            (false, 0x01) => athkit::sys::exit(0), // Esc
            (false, 0x1F) if buf.ctrl && buf.shift => {
                // Ctrl+Shift+S = Save As: type a new filename in the current dir.
                // (Must precede the plain Ctrl+S arm so Shift isn't swallowed.)
                saveas_dlg.dir = if buf.last_file.is_empty() {
                    home_dir()
                } else {
                    dir_of(&buf.last_file)
                };
                saveas_dlg.name_len = 0;
                saveas_dlg.active = true;
                dirty = true;
            }
            (false, 0x1F) if buf.ctrl => {
                buf.save();
                // Remember the file we just wrote (so it re-opens next launch).
                persist_prefs(&buf, &find);
                dirty = true;
            } // Ctrl+S
            _ => {
                if let Some(ch) = scancode_to_ascii(code, buf.shift) {
                    match ch {
                        b'\n' => {
                            buf.newline();
                            dirty = true;
                        }
                        0x08 => {
                            buf.backspace();
                            dirty = true;
                        }
                        c if c >= 0x20 && c < 0x7F => {
                            buf.insert_char(c);
                            dirty = true;
                        }
                        _ => {}
                    }
                }
            }
        }

        if dirty {
            buf.update_scroll(visible_rows);
            render_all(&buf, &find, &open_dlg, &saveas_dlg, &toast, &mut canvas);
            athkit::sys::surface_present(sid, PRESENT_X as u64, PRESENT_Y as u64);
        }
    }
}

/// Snapshot the live find-bar toggles + bound file into a `Prefs` and write it to
/// disk (best effort, silent on failure). Called on every preference-affecting
/// change (save, case toggle, regex toggle) so settings survive a relaunch.
fn persist_prefs(buf: &Buf, find: &FindState) {
    let prefs = Prefs {
        case_sensitive: find.case_sensitive,
        regex_mode: find.regex_mode,
        last_file: buf.last_file.clone(),
    };
    save_prefs(&prefs);
}

/// Apply a find-bar click action (mirrors the matching key 1:1). Returns true if
/// anything changed (caller re-renders). Editing the body stays keyboard-driven.
fn dispatch_find_click(buf: &mut Buf, find: &mut FindState, action: FindAction) -> bool {
    match action {
        FindAction::FocusFind => {
            find.field = FindField::Find;
            true
        }
        FindAction::FocusReplace => {
            find.replace_mode = true;
            find.field = FindField::Replace;
            true
        }
        FindAction::FindNext => {
            buf.step_match(find, 1);
            true
        }
        FindAction::FindPrev => {
            buf.step_match(find, -1);
            true
        }
        FindAction::ToggleCase => {
            find.case_sensitive = !find.case_sensitive;
            buf.recompute_matches(find);
            persist_prefs(buf, find);
            true
        }
        FindAction::ToggleRegex => {
            find.regex_mode = !find.regex_mode;
            buf.recompute_matches(find);
            persist_prefs(buf, find);
            true
        }
        FindAction::ReplaceCurrent => {
            buf.replace_current(find);
            true
        }
        FindAction::ReplaceAll => {
            buf.replace_all_matches(find);
            true
        }
        FindAction::CloseFind => {
            find.active = false;
            find.matches.clear();
            find.current = 0;
            true
        }
        FindAction::None => false,
    }
}
