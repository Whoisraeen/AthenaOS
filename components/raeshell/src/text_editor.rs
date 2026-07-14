//! Built-in text editor for AthenaOS — modal editing, syntax highlighting,
//! search/replace, undo/redo, and line numbers.

#![allow(dead_code)]

extern crate alloc;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

// ── Colour palette ───────────────────────────────────────────────────────

const ED_BG: u32 = 0xFF_0E_10_1A;
const ED_GUTTER_BG: u32 = 0xFF_12_14_1E;
const ED_FG: u32 = 0xFF_D0_D0_E0;
const ED_DIM: u32 = 0xFF_55_55_66;
const ED_ACCENT: u32 = 0xFF_4E_9C_FF;
const ED_CURSOR_BG: u32 = 0xFF_4E_9C_FF;
const ED_LINE_HL: u32 = 0xFF_1A_1C_2A;
const ED_SELECTION: u32 = 0xFF_33_55_88;
const ED_MATCH_BG: u32 = 0xFF_55_44_00;
const ED_STATUS_BG: u32 = 0xFF_18_1A_28;
const ED_CMD_BG: u32 = 0xFF_14_16_22;
const ED_KEYWORD: u32 = 0xFF_CC_77_FF;
const ED_STRING: u32 = 0xFF_88_CC_44;
const ED_COMMENT: u32 = 0xFF_55_66_77;
const ED_NUMBER: u32 = 0xFF_FF_99_44;
const ED_MODIFIED: u32 = 0xFF_FF_AA_33;
const GLYPH_W: usize = 8;
const GLYPH_H: usize = 8;

// ── Types ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditorMode {
    Normal,
    Insert,
    Command,
    Search,
}

#[derive(Debug, Clone)]
pub enum EditAction {
    Insert {
        row: usize,
        col: usize,
        text: String,
    },
    Delete {
        row: usize,
        col: usize,
        text: String,
    },
    InsertLine {
        row: usize,
    },
    DeleteLine {
        row: usize,
        content: String,
    },
}

#[derive(Debug, Clone)]
pub struct SyntaxRule {
    pub pattern: String,
    pub color: u32,
    pub bold: bool,
}

#[derive(Debug, Clone)]
pub struct SyntaxHighlight {
    pub language: String,
    pub rules: Vec<SyntaxRule>,
}

impl SyntaxHighlight {
    pub fn for_rust() -> Self {
        let keywords = [
            "fn", "let", "mut", "pub", "struct", "enum", "impl", "use", "mod", "self", "Self",
            "return", "if", "else", "match", "for", "while", "loop", "break", "continue", "true",
            "false", "const", "static", "type", "where", "trait", "as", "ref", "unsafe", "extern",
            "crate", "super",
        ];
        let rules: Vec<SyntaxRule> = keywords
            .iter()
            .map(|kw| SyntaxRule {
                pattern: String::from(*kw),
                color: ED_KEYWORD,
                bold: true,
            })
            .collect();
        Self {
            language: String::from("rust"),
            rules,
        }
    }
}

// ── Data structures ──────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct TextBuffer {
    pub lines: Vec<String>,
}

impl TextBuffer {
    pub fn new() -> Self {
        Self {
            lines: alloc::vec![String::new()],
        }
    }

    pub fn from_text(text: &str) -> Self {
        let lines: Vec<String> = text.split('\n').map(String::from).collect();
        if lines.is_empty() {
            Self::new()
        } else {
            Self { lines }
        }
    }

    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    pub fn line(&self, row: usize) -> &str {
        self.lines.get(row).map(|s| s.as_str()).unwrap_or("")
    }

    pub fn to_string(&self) -> String {
        let mut result = String::new();
        for (i, line) in self.lines.iter().enumerate() {
            result.push_str(line);
            if i + 1 < self.lines.len() {
                result.push('\n');
            }
        }
        result
    }
}

#[derive(Debug, Clone, Copy)]
pub struct EditorCursor {
    pub row: usize,
    pub col: usize,
    pub desired_col: usize,
}

#[derive(Debug, Clone, Copy)]
pub struct ScrollState {
    pub top_line: usize,
    pub left_col: usize,
}

#[derive(Debug, Clone)]
pub struct EditorSearch {
    pub query: String,
    pub results: Vec<(usize, usize)>,
    pub current: usize,
    pub active: bool,
}

impl EditorSearch {
    pub fn new() -> Self {
        Self {
            query: String::new(),
            results: Vec::new(),
            current: 0,
            active: false,
        }
    }
}

// ── Text editor ──────────────────────────────────────────────────────────

pub struct TextEditor {
    pub buffer: TextBuffer,
    pub cursor: EditorCursor,
    pub scroll: ScrollState,
    pub mode: EditorMode,
    pub file_path: Option<String>,
    pub modified: bool,
    pub syntax: Option<SyntaxHighlight>,
    pub line_numbers: bool,
    pub word_wrap: bool,
    pub tab_size: usize,
    pub search: EditorSearch,
    pub clipboard: String,
    pub undo_stack: Vec<EditAction>,
    pub redo_stack: Vec<EditAction>,
    pub status_message: String,
    pub width: usize,
    pub height: usize,
}

impl TextEditor {
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            buffer: TextBuffer::new(),
            cursor: EditorCursor {
                row: 0,
                col: 0,
                desired_col: 0,
            },
            scroll: ScrollState {
                top_line: 0,
                left_col: 0,
            },
            mode: EditorMode::Insert,
            file_path: None,
            modified: false,
            syntax: None,
            line_numbers: true,
            word_wrap: false,
            tab_size: 4,
            search: EditorSearch::new(),
            clipboard: String::new(),
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            status_message: String::new(),
            width,
            height,
        }
    }

    pub fn open_file(&mut self, path: &str, content: &str) {
        self.buffer = TextBuffer::from_text(content);
        self.file_path = Some(String::from(path));
        self.cursor = EditorCursor {
            row: 0,
            col: 0,
            desired_col: 0,
        };
        self.scroll = ScrollState {
            top_line: 0,
            left_col: 0,
        };
        self.modified = false;
        self.undo_stack.clear();
        self.redo_stack.clear();

        if path.ends_with(".rs") {
            self.syntax = Some(SyntaxHighlight::for_rust());
        } else {
            self.syntax = None;
        }

        self.status_message = format!("Opened: {}", path);
    }

    pub fn save_file(&mut self) -> Option<(String, String)> {
        if let Some(ref path) = self.file_path {
            let content = self.buffer.to_string();
            self.modified = false;
            self.status_message = format!("Saved: {}", path);
            Some((path.clone(), content))
        } else {
            self.status_message = String::from("No file path set");
            None
        }
    }

    // ── Text insertion / deletion ────────────────────────────────────

    pub fn insert_char(&mut self, ch: char) {
        let row = self.cursor.row;
        let col = self.cursor.col;
        if row >= self.buffer.lines.len() {
            return;
        }

        let col = col.min(self.buffer.lines[row].len());
        self.undo_stack.push(EditAction::Insert {
            row,
            col,
            text: String::from(ch.to_string().as_str()),
        });
        self.redo_stack.clear();
        self.buffer.lines[row].insert(col, ch);
        self.cursor.col = col + 1;
        self.cursor.desired_col = self.cursor.col;
        self.modified = true;
    }

    pub fn delete_char(&mut self) {
        let row = self.cursor.row;
        let col = self.cursor.col;
        if row >= self.buffer.lines.len() {
            return;
        }

        if col < self.buffer.lines[row].len() {
            let ch = self.buffer.lines[row].remove(col);
            self.undo_stack.push(EditAction::Delete {
                row,
                col,
                text: String::from(ch.to_string().as_str()),
            });
            self.redo_stack.clear();
            self.modified = true;
        } else if row + 1 < self.buffer.lines.len() {
            let next_line = self.buffer.lines.remove(row + 1);
            self.buffer.lines[row].push_str(&next_line);
            self.undo_stack.push(EditAction::DeleteLine {
                row: row + 1,
                content: next_line,
            });
            self.redo_stack.clear();
            self.modified = true;
        }
    }

    pub fn new_line(&mut self) {
        let row = self.cursor.row;
        let col = self.cursor.col.min(self.buffer.lines[row].len());

        let remainder = self.buffer.lines[row][col..].to_string();
        self.buffer.lines[row].truncate(col);
        self.buffer
            .lines
            .insert(row + 1, String::from(remainder.as_str()));

        self.undo_stack
            .push(EditAction::InsertLine { row: row + 1 });
        self.redo_stack.clear();
        self.cursor.row += 1;
        self.cursor.col = 0;
        self.cursor.desired_col = 0;
        self.modified = true;
    }

    pub fn delete_line(&mut self) {
        let row = self.cursor.row;
        if self.buffer.lines.len() <= 1 {
            let content = self.buffer.lines[0].clone();
            self.buffer.lines[0].clear();
            self.undo_stack.push(EditAction::Delete {
                row: 0,
                col: 0,
                text: content,
            });
        } else {
            let content = self.buffer.lines.remove(row);
            self.undo_stack
                .push(EditAction::DeleteLine { row, content });
            if self.cursor.row >= self.buffer.lines.len() {
                self.cursor.row = self.buffer.lines.len() - 1;
            }
        }
        self.redo_stack.clear();
        self.clamp_cursor();
        self.modified = true;
    }

    // ── Cursor movement ──────────────────────────────────────────────

    pub fn move_cursor(&mut self, dr: i32, dc: i32) {
        if dr != 0 {
            let new_row = (self.cursor.row as i64 + dr as i64)
                .max(0)
                .min(self.buffer.line_count().saturating_sub(1) as i64)
                as usize;
            self.cursor.row = new_row;
            self.cursor.col = self
                .cursor
                .desired_col
                .min(self.buffer.line(self.cursor.row).len());
        }
        if dc != 0 {
            let line_len = self.buffer.line(self.cursor.row).len();
            let new_col = (self.cursor.col as i64 + dc as i64)
                .max(0)
                .min(line_len as i64) as usize;
            self.cursor.col = new_col;
            self.cursor.desired_col = self.cursor.col;
        }
        self.ensure_cursor_visible();
    }

    pub fn page_up(&mut self) {
        let visible_lines = self.visible_line_count();
        self.cursor.row = self.cursor.row.saturating_sub(visible_lines);
        self.scroll.top_line = self.scroll.top_line.saturating_sub(visible_lines);
        self.clamp_cursor();
    }

    pub fn page_down(&mut self) {
        let visible_lines = self.visible_line_count();
        self.cursor.row =
            (self.cursor.row + visible_lines).min(self.buffer.line_count().saturating_sub(1));
        self.scroll.top_line = (self.scroll.top_line + visible_lines)
            .min(self.buffer.line_count().saturating_sub(visible_lines));
        self.clamp_cursor();
    }

    pub fn home(&mut self) {
        self.cursor.col = 0;
        self.cursor.desired_col = 0;
    }

    pub fn end(&mut self) {
        self.cursor.col = self.buffer.line(self.cursor.row).len();
        self.cursor.desired_col = self.cursor.col;
    }

    pub fn select_word(&mut self) -> Option<String> {
        let line = self.buffer.line(self.cursor.row);
        if self.cursor.col >= line.len() {
            return None;
        }

        let bytes = line.as_bytes();
        let mut start = self.cursor.col;
        let mut end = self.cursor.col;
        while start > 0 && (bytes[start - 1] as char).is_alphanumeric() {
            start -= 1;
        }
        while end < bytes.len() && (bytes[end] as char).is_alphanumeric() {
            end += 1;
        }

        if start < end {
            Some(String::from(&line[start..end]))
        } else {
            None
        }
    }

    // ── Clipboard ────────────────────────────────────────────────────

    pub fn copy(&mut self) {
        self.clipboard = String::from(self.buffer.line(self.cursor.row));
        self.status_message = String::from("Line copied");
    }

    pub fn cut(&mut self) {
        self.clipboard = String::from(self.buffer.line(self.cursor.row));
        self.delete_line();
        self.status_message = String::from("Line cut");
    }

    pub fn paste(&mut self) {
        if self.clipboard.is_empty() {
            return;
        }
        let row = self.cursor.row;
        let col = self.cursor.col.min(self.buffer.lines[row].len());
        let text = self.clipboard.clone();
        self.undo_stack.push(EditAction::Insert {
            row,
            col,
            text: text.clone(),
        });
        self.redo_stack.clear();
        self.buffer.lines[row].insert_str(col, &text);
        self.cursor.col = col + text.len();
        self.cursor.desired_col = self.cursor.col;
        self.modified = true;
    }

    // ── Undo / redo ──────────────────────────────────────────────────

    pub fn undo(&mut self) {
        if let Some(action) = self.undo_stack.pop() {
            match &action {
                EditAction::Insert { row, col, text } => {
                    let end = col + text.len();
                    if *row < self.buffer.lines.len() && end <= self.buffer.lines[*row].len() {
                        self.buffer.lines[*row].replace_range(*col..end, "");
                    }
                    self.cursor.row = *row;
                    self.cursor.col = *col;
                }
                EditAction::Delete { row, col, text } => {
                    if *row < self.buffer.lines.len() {
                        self.buffer.lines[*row].insert_str(*col, text);
                    }
                    self.cursor.row = *row;
                    self.cursor.col = col + text.len();
                }
                EditAction::InsertLine { row } => {
                    if *row < self.buffer.lines.len() && *row > 0 {
                        let removed = self.buffer.lines.remove(*row);
                        self.buffer.lines[*row - 1].push_str(&removed);
                    }
                    self.cursor.row = row.saturating_sub(1);
                }
                EditAction::DeleteLine { row, content } => {
                    self.buffer.lines.insert(*row, content.clone());
                    self.cursor.row = *row;
                }
            }
            self.redo_stack.push(action);
            self.clamp_cursor();
            self.modified = true;
            self.status_message = String::from("Undo");
        }
    }

    pub fn redo(&mut self) {
        if let Some(action) = self.redo_stack.pop() {
            match &action {
                EditAction::Insert { row, col, text } => {
                    if *row < self.buffer.lines.len() {
                        let c = (*col).min(self.buffer.lines[*row].len());
                        self.buffer.lines[*row].insert_str(c, text);
                        self.cursor.col = c + text.len();
                    }
                    self.cursor.row = *row;
                }
                EditAction::Delete { row, col, text } => {
                    let end = col + text.len();
                    if *row < self.buffer.lines.len() && end <= self.buffer.lines[*row].len() {
                        self.buffer.lines[*row].replace_range(*col..end, "");
                    }
                    self.cursor.row = *row;
                    self.cursor.col = *col;
                }
                EditAction::InsertLine { row } => {
                    if *row <= self.buffer.lines.len() {
                        self.buffer.lines.insert(*row, String::new());
                    }
                    self.cursor.row = *row;
                    self.cursor.col = 0;
                }
                EditAction::DeleteLine { row, .. } => {
                    if *row < self.buffer.lines.len() {
                        self.buffer.lines.remove(*row);
                    }
                    if self.cursor.row >= self.buffer.lines.len() {
                        self.cursor.row = self.buffer.lines.len().saturating_sub(1);
                    }
                }
            }
            self.undo_stack.push(action);
            self.clamp_cursor();
            self.modified = true;
            self.status_message = String::from("Redo");
        }
    }

    // ── Search ───────────────────────────────────────────────────────

    pub fn find(&mut self, query: &str) {
        self.search.query = String::from(query);
        self.search.results.clear();
        self.search.active = true;

        for (row, line) in self.buffer.lines.iter().enumerate() {
            let mut start = 0;
            while let Some(pos) = line[start..].find(query) {
                self.search.results.push((row, start + pos));
                start += pos + 1;
                if start >= line.len() {
                    break;
                }
            }
        }
        self.search.current = 0;
        if !self.search.results.is_empty() {
            let (row, col) = self.search.results[0];
            self.cursor.row = row;
            self.cursor.col = col;
            self.ensure_cursor_visible();
        }
        self.status_message = format!("{} matches", self.search.results.len());
    }

    pub fn find_next(&mut self) {
        if self.search.results.is_empty() {
            return;
        }
        self.search.current = (self.search.current + 1) % self.search.results.len();
        let (row, col) = self.search.results[self.search.current];
        self.cursor.row = row;
        self.cursor.col = col;
        self.ensure_cursor_visible();
        self.status_message = format!(
            "Match {}/{}",
            self.search.current + 1,
            self.search.results.len()
        );
    }

    pub fn find_replace(&mut self, find: &str, replace: &str) -> usize {
        let mut count = 0;
        for line in &mut self.buffer.lines {
            while let Some(pos) = line.find(find) {
                line.replace_range(pos..pos + find.len(), replace);
                count += 1;
            }
        }
        if count > 0 {
            self.modified = true;
        }
        self.status_message = format!("Replaced {} occurrences", count);
        count
    }

    pub fn go_to_line(&mut self, line: usize) {
        let target = line
            .saturating_sub(1)
            .min(self.buffer.line_count().saturating_sub(1));
        self.cursor.row = target;
        self.cursor.col = 0;
        self.cursor.desired_col = 0;
        self.ensure_cursor_visible();
    }

    // ── Internal helpers ─────────────────────────────────────────────

    fn visible_line_count(&self) -> usize {
        (self.height / GLYPH_H).saturating_sub(2) // minus status bar + command line
    }

    fn gutter_width(&self) -> usize {
        if !self.line_numbers {
            return 0;
        }
        let digits = format!("{}", self.buffer.line_count()).len();
        (digits + 2) * GLYPH_W
    }

    fn clamp_cursor(&mut self) {
        if self.cursor.row >= self.buffer.line_count() {
            self.cursor.row = self.buffer.line_count().saturating_sub(1);
        }
        let line_len = self.buffer.line(self.cursor.row).len();
        if self.cursor.col > line_len {
            self.cursor.col = line_len;
        }
    }

    fn ensure_cursor_visible(&mut self) {
        let visible = self.visible_line_count();
        if self.cursor.row < self.scroll.top_line {
            self.scroll.top_line = self.cursor.row;
        }
        if self.cursor.row >= self.scroll.top_line + visible {
            self.scroll.top_line = self.cursor.row.saturating_sub(visible) + 1;
        }
    }

    fn is_keyword(&self, word: &str) -> Option<u32> {
        if let Some(ref syn) = self.syntax {
            for rule in &syn.rules {
                if rule.pattern == word {
                    return Some(rule.color);
                }
            }
        }
        None
    }

    // ── Render ───────────────────────────────────────────────────────

    pub fn render(&self, canvas: &mut raegfx::Canvas, x: usize, y: usize, w: usize, h: usize) {
        canvas.fill_rect(x, y, w, h, ED_BG);

        let gutter_w = self.gutter_width();
        let status_h = GLYPH_H + 4;
        let text_x = x + gutter_w;
        let text_w = w.saturating_sub(gutter_w);
        let visible_lines = self.visible_line_count();
        let visible_cols = text_w / GLYPH_W;

        // Line numbers and text
        for vi in 0..visible_lines {
            let line_idx = self.scroll.top_line + vi;
            if line_idx >= self.buffer.line_count() {
                break;
            }
            let py = y + vi * GLYPH_H;

            // Current line highlight
            if line_idx == self.cursor.row {
                canvas.fill_rect(x, py, w, GLYPH_H, ED_LINE_HL);
            }

            // Gutter
            if self.line_numbers {
                let num_str = format!("{}", line_idx + 1);
                let num_x = x + gutter_w - (num_str.len() + 1) * GLYPH_W;
                let gutter_fg = if line_idx == self.cursor.row {
                    ED_FG
                } else {
                    ED_DIM
                };
                canvas.draw_text(num_x, py, &num_str, gutter_fg, None);
            }

            // Line text with basic highlighting
            let line = self.buffer.line(line_idx);
            let display_start = self.scroll.left_col;
            let display = if display_start < line.len() {
                &line[display_start..]
            } else {
                ""
            };
            let chars_to_draw = display.len().min(visible_cols);

            if self.syntax.is_some() {
                self.render_highlighted_line(canvas, text_x, py, display, chars_to_draw);
            } else {
                canvas.draw_text(
                    text_x,
                    py,
                    crate::text_util::truncate_chars(display, chars_to_draw),
                    ED_FG,
                    None,
                );
            }

            // Search match highlights
            if self.search.active {
                for &(mr, mc) in &self.search.results {
                    if mr == line_idx && mc >= display_start && mc < display_start + visible_cols {
                        let hx = text_x + (mc - display_start) * GLYPH_W;
                        let hw = self.search.query.len() * GLYPH_W;
                        canvas.fill_rect(hx, py, hw, GLYPH_H, ED_MATCH_BG);
                    }
                }
            }
        }

        // Gutter border
        if self.line_numbers {
            let bx = x + gutter_w - GLYPH_W / 2;
            for gy in y..y + visible_lines * GLYPH_H {
                canvas.draw_pixel(bx, gy, ED_DIM);
            }
        }

        // Cursor
        if self.cursor.row >= self.scroll.top_line
            && self.cursor.row < self.scroll.top_line + visible_lines
            && self.cursor.col >= self.scroll.left_col
        {
            let cx = text_x + (self.cursor.col - self.scroll.left_col) * GLYPH_W;
            let cy = y + (self.cursor.row - self.scroll.top_line) * GLYPH_H;
            match self.mode {
                EditorMode::Normal => {
                    canvas.fill_rect(cx, cy, GLYPH_W, GLYPH_H, ED_CURSOR_BG);
                    let ch = self
                        .buffer
                        .line(self.cursor.row)
                        .chars()
                        .nth(self.cursor.col)
                        .unwrap_or(' ');
                    if ch != ' ' {
                        canvas.draw_glyph(cx, cy, ch, ED_BG, None);
                    }
                }
                EditorMode::Insert => {
                    for uy in cy..cy + GLYPH_H {
                        canvas.draw_pixel(cx, uy, ED_CURSOR_BG);
                        canvas.draw_pixel(cx + 1, uy, ED_CURSOR_BG);
                    }
                }
                _ => {
                    canvas.fill_rect(cx, cy + GLYPH_H - 2, GLYPH_W, 2, ED_CURSOR_BG);
                }
            }
        }

        // Status bar
        let status_y = y + h - status_h * 2;
        canvas.fill_rect(x, status_y, w, status_h, ED_STATUS_BG);

        let mode_str = match self.mode {
            EditorMode::Normal => "NORMAL",
            EditorMode::Insert => "INSERT",
            EditorMode::Command => "COMMAND",
            EditorMode::Search => "SEARCH",
        };
        canvas.draw_text(x + 4, status_y + 2, mode_str, ED_ACCENT, None);

        if let Some(ref path) = self.file_path {
            let file_display = if path.len() > 30 {
                &path[path.len() - 30..]
            } else {
                path.as_str()
            };
            let fx = x + (mode_str.len() + 2) * GLYPH_W;
            canvas.draw_text(fx, status_y + 2, file_display, ED_FG, None);
            if self.modified {
                canvas.draw_text(
                    fx + file_display.len() * GLYPH_W + GLYPH_W,
                    status_y + 2,
                    "[+]",
                    ED_MODIFIED,
                    None,
                );
            }
        }

        let pos_str = format!("Ln {}, Col {}", self.cursor.row + 1, self.cursor.col + 1);
        let pos_x = x + w - pos_str.len() * GLYPH_W - 8;
        canvas.draw_text(pos_x, status_y + 2, &pos_str, ED_DIM, None);

        // Message / command line
        let cmd_y = status_y + status_h;
        canvas.fill_rect(x, cmd_y, w, status_h, ED_CMD_BG);
        if !self.status_message.is_empty() {
            canvas.draw_text(x + 4, cmd_y + 2, &self.status_message, ED_DIM, None);
        }
        if self.search.active {
            let sq = format!("/{}", self.search.query);
            canvas.draw_text(x + 4, cmd_y + 2, &sq, ED_ACCENT, None);
        }
    }

    fn render_highlighted_line(
        &self,
        canvas: &mut raegfx::Canvas,
        x: usize,
        y: usize,
        line: &str,
        max_chars: usize,
    ) {
        let bytes = line.as_bytes();
        let len = bytes.len().min(max_chars);
        let mut i = 0;
        let mut px = x;

        while i < len {
            let ch = bytes[i] as char;

            // Comments (Rust-style //)
            if i + 1 < len && bytes[i] == b'/' && bytes[i + 1] == b'/' {
                canvas.draw_text(px, y, &line[i..len], ED_COMMENT, None);
                return;
            }

            // Strings
            if ch == '"' || ch == '\'' {
                let quote = ch;
                let start = i;
                i += 1;
                while i < len && bytes[i] as char != quote {
                    if bytes[i] == b'\\' {
                        i += 1;
                    }
                    i += 1;
                }
                if i < len {
                    i += 1;
                }
                let s = &line[start..i.min(len)];
                canvas.draw_text(px, y, s, ED_STRING, None);
                px += s.len() * GLYPH_W;
                continue;
            }

            // Numbers
            if ch.is_ascii_digit() {
                let start = i;
                while i < len && (bytes[i] as char).is_ascii_alphanumeric() || bytes[i] == b'.' {
                    i += 1;
                }
                let s = &line[start..i];
                canvas.draw_text(px, y, s, ED_NUMBER, None);
                px += s.len() * GLYPH_W;
                continue;
            }

            // Identifiers / keywords
            if ch.is_ascii_alphabetic() || ch == '_' {
                let start = i;
                while i < len && (bytes[i] as char).is_ascii_alphanumeric() || bytes[i] == b'_' {
                    i += 1;
                }
                let word = &line[start..i];
                let color = self.is_keyword(word).unwrap_or(ED_FG);
                canvas.draw_text(px, y, word, color, None);
                px += word.len() * GLYPH_W;
                continue;
            }

            canvas.draw_glyph(px, y, ch, ED_FG, None);
            px += GLYPH_W;
            i += 1;
        }
    }
}
