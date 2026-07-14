//! RaeUI Rich Text Editor
//!
//! A structured document editor with styled text runs, paragraph formatting,
//! cursor/selection management, undo/redo, clipboard operations, and find/replace.

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

// ── Color (ARGB8888) ────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Color(pub u32);

impl Color {
    pub const WHITE: Color = Color(0xFF_FF_FF_FF);
    pub const BLACK: Color = Color(0xFF_00_00_00);
    pub const TRANSPARENT: Color = Color(0x00_00_00_00);
}

// ── Text Run Style ──────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq)]
pub struct TextRunStyle {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strikethrough: bool,
    pub font_size: f32,
    pub color: Color,
    pub background: Option<Color>,
}

impl Default for TextRunStyle {
    fn default() -> Self {
        Self {
            bold: false,
            italic: false,
            underline: false,
            strikethrough: false,
            font_size: 14.0,
            color: Color(0xFF_E0_E0_FF),
            background: None,
        }
    }
}

// ── Text Run ────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct TextRun {
    pub text: String,
    pub style: TextRunStyle,
}

impl TextRun {
    pub fn new(text: &str, style: TextRunStyle) -> Self {
        Self {
            text: String::from(text),
            style,
        }
    }

    pub fn plain(text: &str) -> Self {
        Self {
            text: String::from(text),
            style: TextRunStyle::default(),
        }
    }

    pub fn len(&self) -> usize {
        self.text.len()
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }
}

// ── Text Alignment ──────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextAlignment {
    Left,
    Center,
    Right,
    Justify,
}

// ── Paragraph ───────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct Paragraph {
    pub runs: Vec<TextRun>,
    pub alignment: TextAlignment,
    pub indent: f32,
    pub spacing_before: f32,
    pub spacing_after: f32,
}

impl Paragraph {
    pub fn new() -> Self {
        Self {
            runs: Vec::new(),
            alignment: TextAlignment::Left,
            indent: 0.0,
            spacing_before: 0.0,
            spacing_after: 4.0,
        }
    }

    pub fn with_text(text: &str) -> Self {
        let mut p = Self::new();
        p.runs.push(TextRun::plain(text));
        p
    }

    pub fn total_len(&self) -> usize {
        self.runs.iter().map(|r| r.len()).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.runs.is_empty() || self.runs.iter().all(|r| r.is_empty())
    }

    pub fn plain_text(&self) -> String {
        let mut s = String::new();
        for run in &self.runs {
            s.push_str(&run.text);
        }
        s
    }

    /// Get the character at a given offset within this paragraph.
    pub fn char_at(&self, offset: usize) -> Option<char> {
        let mut pos = 0;
        for run in &self.runs {
            if offset < pos + run.len() {
                return run.text[offset - pos..].chars().next();
            }
            pos += run.len();
        }
        None
    }

    /// Insert text at an offset, inheriting style from the run at that position.
    pub fn insert_text(&mut self, offset: usize, text: &str) {
        if self.runs.is_empty() {
            self.runs.push(TextRun::plain(text));
            return;
        }
        let (run_idx, run_offset) = self.find_run(offset);
        let run = &mut self.runs[run_idx];
        run.text.insert_str(run_offset, text);
    }

    /// Delete a range of characters within this paragraph.
    pub fn delete_range(&mut self, start: usize, end: usize) {
        if start >= end {
            return;
        }
        let mut pos = 0;
        let mut i = 0;
        while i < self.runs.len() {
            let run_start = pos;
            let run_end = pos + self.runs[i].len();

            if run_end <= start || run_start >= end {
                pos = run_end;
                i += 1;
                continue;
            }

            let del_start = start.saturating_sub(run_start);
            let del_end = (end - run_start).min(self.runs[i].len());

            if del_start == 0 && del_end >= self.runs[i].len() {
                self.runs.remove(i);
            } else {
                self.runs[i].text.drain(del_start..del_end);
                if self.runs[i].is_empty() {
                    self.runs.remove(i);
                } else {
                    i += 1;
                }
            }
            pos = run_start;
            // Re-measure after mutation
            if i < self.runs.len() {
                pos += self.runs[i].len();
                i += 1;
            }
        }
        if self.runs.is_empty() {
            self.runs.push(TextRun::plain(""));
        }
    }

    /// Find which run and local offset corresponds to a paragraph offset.
    fn find_run(&self, offset: usize) -> (usize, usize) {
        let mut pos = 0;
        for (i, run) in self.runs.iter().enumerate() {
            if offset <= pos + run.len() {
                return (i, offset - pos);
            }
            pos += run.len();
        }
        let last = self.runs.len().saturating_sub(1);
        (last, self.runs.get(last).map(|r| r.len()).unwrap_or(0))
    }

    /// Apply a style to a range within this paragraph by splitting runs.
    pub fn apply_style(&mut self, start: usize, end: usize, style_fn: fn(&mut TextRunStyle)) {
        if start >= end || self.runs.is_empty() {
            return;
        }
        // Split runs at boundaries, then apply style
        self.split_at(start);
        self.split_at(end);
        let mut pos = 0;
        for run in &mut self.runs {
            let run_end = pos + run.len();
            if pos >= start && run_end <= end {
                style_fn(&mut run.style);
            }
            pos = run_end;
        }
    }

    pub fn split_at(&mut self, offset: usize) {
        let (run_idx, run_offset) = self.find_run(offset);
        if run_offset == 0 || run_offset >= self.runs[run_idx].len() {
            return;
        }
        let right_text = String::from(&self.runs[run_idx].text[run_offset..]);
        let right_style = self.runs[run_idx].style.clone();
        self.runs[run_idx].text.truncate(run_offset);
        self.runs
            .insert(run_idx + 1, TextRun::new(&right_text, right_style));
    }
}

// ── Rich Document ───────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct RichDocument {
    pub paragraphs: Vec<Paragraph>,
}

impl RichDocument {
    pub fn new() -> Self {
        Self {
            paragraphs: alloc::vec![Paragraph::new()],
        }
    }

    pub fn from_plain_text(text: &str) -> Self {
        let paragraphs: Vec<Paragraph> = text.split('\n').map(Paragraph::with_text).collect();
        if paragraphs.is_empty() {
            Self::new()
        } else {
            Self { paragraphs }
        }
    }

    pub fn plain_text(&self) -> String {
        let mut s = String::new();
        for (i, para) in self.paragraphs.iter().enumerate() {
            if i > 0 {
                s.push('\n');
            }
            s.push_str(&para.plain_text());
        }
        s
    }

    pub fn total_len(&self) -> usize {
        let para_len: usize = self.paragraphs.iter().map(|p| p.total_len()).sum();
        // Add newlines between paragraphs
        para_len + self.paragraphs.len().saturating_sub(1)
    }
}

// ── Text Cursor ─────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextCursor {
    pub paragraph: usize,
    pub offset: usize,
}

impl TextCursor {
    pub fn start() -> Self {
        Self {
            paragraph: 0,
            offset: 0,
        }
    }
}

// ── Text Selection ──────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextSelection {
    pub start: TextCursor,
    pub end: TextCursor,
}

impl TextSelection {
    pub fn new(start: TextCursor, end: TextCursor) -> Self {
        Self { start, end }
    }

    pub fn ordered(&self) -> (TextCursor, TextCursor) {
        if self.start.paragraph < self.end.paragraph
            || (self.start.paragraph == self.end.paragraph && self.start.offset <= self.end.offset)
        {
            (self.start, self.end)
        } else {
            (self.end, self.start)
        }
    }

    pub fn is_empty(&self) -> bool {
        self.start == self.end
    }
}

// ── Edit Operations (for undo/redo) ─────────────────────────────────────

#[derive(Clone, Debug)]
pub enum EditOperation {
    InsertText {
        cursor: TextCursor,
        text: String,
    },
    DeleteText {
        start: TextCursor,
        end: TextCursor,
        deleted: String,
    },
    SplitParagraph {
        cursor: TextCursor,
    },
    MergeParagraphs {
        paragraph: usize,
    },
    ApplyStyle {
        start: TextCursor,
        end: TextCursor,
        style_change: StyleChange,
    },
}

#[derive(Clone, Debug)]
pub enum StyleChange {
    Bold(bool),
    Italic(bool),
    Underline(bool),
    Strikethrough(bool),
    FontSize(f32),
    TextColor(Color),
}

// ── Find/Replace ────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct FindResult {
    pub paragraph: usize,
    pub start: usize,
    pub end: usize,
}

// ── Rich Text Editor ────────────────────────────────────────────────────

pub struct RichTextEditor {
    pub document: RichDocument,
    pub cursor: TextCursor,
    pub selection: Option<TextSelection>,
    pub undo_stack: Vec<EditOperation>,
    pub redo_stack: Vec<EditOperation>,
    max_undo: usize,
}

impl RichTextEditor {
    pub fn new() -> Self {
        Self {
            document: RichDocument::new(),
            cursor: TextCursor::start(),
            selection: None,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            max_undo: 100,
        }
    }

    pub fn from_text(text: &str) -> Self {
        Self {
            document: RichDocument::from_plain_text(text),
            cursor: TextCursor::start(),
            selection: None,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            max_undo: 100,
        }
    }

    // ── Text insertion ──────────────────────────────────────────────────

    pub fn insert_text(&mut self, text: &str) {
        self.delete_selection_content();
        self.redo_stack.clear();

        let op = EditOperation::InsertText {
            cursor: self.cursor,
            text: String::from(text),
        };

        for ch in text.chars() {
            if ch == '\n' {
                self.split_paragraph();
            } else {
                let para = &mut self.document.paragraphs[self.cursor.paragraph];
                let mut buf = [0u8; 4];
                let s = ch.encode_utf8(&mut buf);
                para.insert_text(self.cursor.offset, s);
                self.cursor.offset += ch.len_utf8();
            }
        }

        self.push_undo(op);
    }

    pub fn insert_char(&mut self, ch: char) {
        self.delete_selection_content();
        self.redo_stack.clear();

        if ch == '\n' {
            let op = EditOperation::SplitParagraph {
                cursor: self.cursor,
            };
            self.split_paragraph();
            self.push_undo(op);
        } else {
            let mut buf = [0u8; 4];
            let s = ch.encode_utf8(&mut buf);
            let text = String::from(&*s);
            let para = &mut self.document.paragraphs[self.cursor.paragraph];
            para.insert_text(self.cursor.offset, &text);
            let op = EditOperation::InsertText {
                cursor: self.cursor,
                text,
            };
            self.cursor.offset += ch.len_utf8();
            self.push_undo(op);
        }
    }

    // ── Deletion ────────────────────────────────────────────────────────

    pub fn delete_backward(&mut self) {
        if self.delete_selection_content() {
            return;
        }
        self.redo_stack.clear();

        if self.cursor.offset > 0 {
            let para = &self.document.paragraphs[self.cursor.paragraph];
            // Find the previous char boundary
            let text = para.plain_text();
            let prev_boundary = text[..self.cursor.offset]
                .char_indices()
                .last()
                .map(|(i, _)| i)
                .unwrap_or(0);
            let deleted = String::from(&text[prev_boundary..self.cursor.offset]);
            let start = TextCursor {
                paragraph: self.cursor.paragraph,
                offset: prev_boundary,
            };
            let end = self.cursor;

            let para = &mut self.document.paragraphs[self.cursor.paragraph];
            para.delete_range(prev_boundary, self.cursor.offset);
            self.cursor.offset = prev_boundary;

            self.push_undo(EditOperation::DeleteText {
                start,
                end,
                deleted,
            });
        } else if self.cursor.paragraph > 0 {
            // Merge with previous paragraph
            let prev_para = self.cursor.paragraph - 1;
            let prev_len = self.document.paragraphs[prev_para].total_len();
            let op = EditOperation::MergeParagraphs {
                paragraph: prev_para,
            };
            self.merge_paragraphs(prev_para);
            self.cursor.paragraph = prev_para;
            self.cursor.offset = prev_len;
            self.push_undo(op);
        }
    }

    pub fn delete_forward(&mut self) {
        if self.delete_selection_content() {
            return;
        }
        self.redo_stack.clear();

        let para = &self.document.paragraphs[self.cursor.paragraph];
        if self.cursor.offset < para.total_len() {
            let text = para.plain_text();
            let next_boundary = text[self.cursor.offset..]
                .chars()
                .next()
                .map(|c| self.cursor.offset + c.len_utf8())
                .unwrap_or(self.cursor.offset);
            let deleted = String::from(&text[self.cursor.offset..next_boundary]);
            let start = self.cursor;
            let end = TextCursor {
                paragraph: self.cursor.paragraph,
                offset: next_boundary,
            };

            let para = &mut self.document.paragraphs[self.cursor.paragraph];
            para.delete_range(self.cursor.offset, next_boundary);

            self.push_undo(EditOperation::DeleteText {
                start,
                end,
                deleted,
            });
        } else if self.cursor.paragraph < self.document.paragraphs.len() - 1 {
            let op = EditOperation::MergeParagraphs {
                paragraph: self.cursor.paragraph,
            };
            self.merge_paragraphs(self.cursor.paragraph);
            self.push_undo(op);
        }
    }

    // ── Selection ───────────────────────────────────────────────────────

    pub fn select_all(&mut self) {
        let last_para = self.document.paragraphs.len() - 1;
        let last_offset = self.document.paragraphs[last_para].total_len();
        self.selection = Some(TextSelection {
            start: TextCursor {
                paragraph: 0,
                offset: 0,
            },
            end: TextCursor {
                paragraph: last_para,
                offset: last_offset,
            },
        });
        self.cursor = TextCursor {
            paragraph: last_para,
            offset: last_offset,
        };
    }

    pub fn selected_text(&self) -> String {
        let sel = match &self.selection {
            Some(s) if !s.is_empty() => s,
            _ => return String::new(),
        };
        let (start, end) = sel.ordered();
        let mut result = String::new();

        for pi in start.paragraph..=end.paragraph {
            if pi >= self.document.paragraphs.len() {
                break;
            }
            let para = &self.document.paragraphs[pi];
            let text = para.plain_text();
            let s = if pi == start.paragraph {
                start.offset
            } else {
                0
            };
            let e = if pi == end.paragraph {
                end.offset
            } else {
                text.len()
            };
            let slice = &text[s.min(text.len())..e.min(text.len())];
            result.push_str(slice);
            if pi < end.paragraph {
                result.push('\n');
            }
        }
        result
    }

    pub fn clear_selection(&mut self) {
        self.selection = None;
    }

    // ── Cursor Movement ─────────────────────────────────────────────────

    pub fn move_left(&mut self) {
        self.selection = None;
        if self.cursor.offset > 0 {
            let text = self.document.paragraphs[self.cursor.paragraph].plain_text();
            let prev = text[..self.cursor.offset]
                .char_indices()
                .last()
                .map(|(i, _)| i)
                .unwrap_or(0);
            self.cursor.offset = prev;
        } else if self.cursor.paragraph > 0 {
            self.cursor.paragraph -= 1;
            self.cursor.offset = self.document.paragraphs[self.cursor.paragraph].total_len();
        }
    }

    pub fn move_right(&mut self) {
        self.selection = None;
        let para = &self.document.paragraphs[self.cursor.paragraph];
        if self.cursor.offset < para.total_len() {
            let text = para.plain_text();
            let next = text[self.cursor.offset..]
                .chars()
                .next()
                .map(|c| self.cursor.offset + c.len_utf8())
                .unwrap_or(self.cursor.offset);
            self.cursor.offset = next;
        } else if self.cursor.paragraph < self.document.paragraphs.len() - 1 {
            self.cursor.paragraph += 1;
            self.cursor.offset = 0;
        }
    }

    pub fn move_up(&mut self) {
        self.selection = None;
        if self.cursor.paragraph > 0 {
            self.cursor.paragraph -= 1;
            let para_len = self.document.paragraphs[self.cursor.paragraph].total_len();
            self.cursor.offset = self.cursor.offset.min(para_len);
        } else {
            self.cursor.offset = 0;
        }
    }

    pub fn move_down(&mut self) {
        self.selection = None;
        if self.cursor.paragraph < self.document.paragraphs.len() - 1 {
            self.cursor.paragraph += 1;
            let para_len = self.document.paragraphs[self.cursor.paragraph].total_len();
            self.cursor.offset = self.cursor.offset.min(para_len);
        } else {
            self.cursor.offset = self.document.paragraphs[self.cursor.paragraph].total_len();
        }
    }

    pub fn move_to_line_start(&mut self) {
        self.selection = None;
        self.cursor.offset = 0;
    }

    pub fn move_to_line_end(&mut self) {
        self.selection = None;
        self.cursor.offset = self.document.paragraphs[self.cursor.paragraph].total_len();
    }

    // ── Undo / Redo ─────────────────────────────────────────────────────

    pub fn undo(&mut self) {
        if let Some(op) = self.undo_stack.pop() {
            self.apply_inverse(&op);
            self.redo_stack.push(op);
        }
    }

    pub fn redo(&mut self) {
        if let Some(op) = self.redo_stack.pop() {
            self.apply_forward(&op);
            self.undo_stack.push(op);
        }
    }

    // ── Cut / Copy / Paste ──────────────────────────────────────────────

    pub fn cut(&mut self) -> String {
        let text = self.selected_text();
        self.delete_selection_content();
        text
    }

    pub fn paste(&mut self, text: &str) {
        self.insert_text(text);
    }

    // ── Find / Replace ──────────────────────────────────────────────────

    pub fn find(&self, query: &str) -> Vec<FindResult> {
        let mut results = Vec::new();
        if query.is_empty() {
            return results;
        }

        for (pi, para) in self.document.paragraphs.iter().enumerate() {
            let text = para.plain_text();
            let mut start = 0;
            while let Some(pos) = find_substr(&text[start..], query) {
                let abs_pos = start + pos;
                results.push(FindResult {
                    paragraph: pi,
                    start: abs_pos,
                    end: abs_pos + query.len(),
                });
                start = abs_pos + 1;
                if start >= text.len() {
                    break;
                }
            }
        }
        results
    }

    pub fn replace_all(&mut self, query: &str, replacement: &str) -> usize {
        let matches = self.find(query);
        let count = matches.len();
        // Apply from back to front to preserve offsets
        for result in matches.into_iter().rev() {
            let para = &mut self.document.paragraphs[result.paragraph];
            para.delete_range(result.start, result.end);
            para.insert_text(result.start, replacement);
        }
        count
    }

    // ── Style Application ───────────────────────────────────────────────

    pub fn toggle_bold(&mut self) {
        self.apply_style_to_selection(|s| s.bold = !s.bold);
    }

    pub fn toggle_italic(&mut self) {
        self.apply_style_to_selection(|s| s.italic = !s.italic);
    }

    pub fn toggle_underline(&mut self) {
        self.apply_style_to_selection(|s| s.underline = !s.underline);
    }

    pub fn toggle_strikethrough(&mut self) {
        self.apply_style_to_selection(|s| s.strikethrough = !s.strikethrough);
    }

    pub fn set_font_size(&mut self, size: f32) {
        let sel = match &self.selection {
            Some(s) if !s.is_empty() => *s,
            _ => return,
        };
        let (start, end) = sel.ordered();
        for pi in start.paragraph..=end.paragraph {
            if pi >= self.document.paragraphs.len() {
                break;
            }
            let s = if pi == start.paragraph {
                start.offset
            } else {
                0
            };
            let e = if pi == end.paragraph {
                end.offset
            } else {
                self.document.paragraphs[pi].total_len()
            };
            self.document.paragraphs[pi].split_at(s);
            self.document.paragraphs[pi].split_at(e);
            let mut pos = 0;
            for run in &mut self.document.paragraphs[pi].runs {
                let run_end = pos + run.len();
                if pos >= s && run_end <= e {
                    run.style.font_size = size;
                }
                pos = run_end;
            }
        }
    }

    // ── Internal helpers ────────────────────────────────────────────────

    fn apply_style_to_selection(&mut self, style_fn: fn(&mut TextRunStyle)) {
        let sel = match &self.selection {
            Some(s) if !s.is_empty() => *s,
            _ => return,
        };
        let (start, end) = sel.ordered();
        for pi in start.paragraph..=end.paragraph {
            if pi >= self.document.paragraphs.len() {
                break;
            }
            let s = if pi == start.paragraph {
                start.offset
            } else {
                0
            };
            let e = if pi == end.paragraph {
                end.offset
            } else {
                self.document.paragraphs[pi].total_len()
            };
            self.document.paragraphs[pi].apply_style(s, e, style_fn);
        }
    }

    fn delete_selection_content(&mut self) -> bool {
        let sel = match self.selection.take() {
            Some(s) if !s.is_empty() => s,
            _ => return false,
        };
        let (start, end) = sel.ordered();
        self.redo_stack.clear();

        if start.paragraph == end.paragraph {
            let deleted = {
                let text = self.document.paragraphs[start.paragraph].plain_text();
                String::from(&text[start.offset..end.offset.min(text.len())])
            };
            self.document.paragraphs[start.paragraph].delete_range(start.offset, end.offset);
            self.push_undo(EditOperation::DeleteText {
                start,
                end,
                deleted,
            });
        } else {
            let deleted = self.text_between(start, end);
            // Keep text before selection in first paragraph
            let first_text = {
                let text = self.document.paragraphs[start.paragraph].plain_text();
                String::from(&text[..start.offset.min(text.len())])
            };
            // Keep text after selection in last paragraph
            let last_text = {
                let text = self.document.paragraphs[end.paragraph].plain_text();
                String::from(&text[end.offset.min(text.len())..])
            };
            // Remove intermediate paragraphs
            let remove_count = end.paragraph - start.paragraph;
            for _ in 0..remove_count {
                if start.paragraph + 1 < self.document.paragraphs.len() {
                    self.document.paragraphs.remove(start.paragraph + 1);
                }
            }
            // Rebuild first paragraph
            self.document.paragraphs[start.paragraph] = Paragraph::with_text(&{
                let mut combined = first_text;
                combined.push_str(&last_text);
                combined
            });
            self.push_undo(EditOperation::DeleteText {
                start,
                end,
                deleted,
            });
        }
        self.cursor = start;
        true
    }

    fn text_between(&self, start: TextCursor, end: TextCursor) -> String {
        let mut result = String::new();
        for pi in start.paragraph..=end.paragraph {
            if pi >= self.document.paragraphs.len() {
                break;
            }
            let text = self.document.paragraphs[pi].plain_text();
            let s = if pi == start.paragraph {
                start.offset
            } else {
                0
            };
            let e = if pi == end.paragraph {
                end.offset.min(text.len())
            } else {
                text.len()
            };
            result.push_str(&text[s.min(text.len())..e]);
            if pi < end.paragraph {
                result.push('\n');
            }
        }
        result
    }

    fn split_paragraph(&mut self) {
        let para = &self.document.paragraphs[self.cursor.paragraph];
        let text = para.plain_text();
        let after = String::from(&text[self.cursor.offset.min(text.len())..]);

        self.document.paragraphs[self.cursor.paragraph]
            .delete_range(self.cursor.offset, text.len());

        let new_para = Paragraph::with_text(&after);
        self.document
            .paragraphs
            .insert(self.cursor.paragraph + 1, new_para);
        self.cursor.paragraph += 1;
        self.cursor.offset = 0;
    }

    fn merge_paragraphs(&mut self, idx: usize) {
        if idx + 1 >= self.document.paragraphs.len() {
            return;
        }
        let next_text = self.document.paragraphs[idx + 1].plain_text();
        let current_len = self.document.paragraphs[idx].total_len();
        self.document.paragraphs[idx].insert_text(current_len, &next_text);
        self.document.paragraphs.remove(idx + 1);
    }

    fn push_undo(&mut self, op: EditOperation) {
        self.undo_stack.push(op);
        if self.undo_stack.len() > self.max_undo {
            self.undo_stack.remove(0);
        }
    }

    fn apply_inverse(&mut self, op: &EditOperation) {
        match op {
            EditOperation::InsertText { cursor, text } => {
                let end_cursor = self.advance_cursor(*cursor, text.len());
                self.cursor = *cursor;
                let para = &mut self.document.paragraphs[cursor.paragraph];
                para.delete_range(cursor.offset, cursor.offset + text.len());
                let _ = end_cursor;
            }
            EditOperation::DeleteText { start, deleted, .. } => {
                self.cursor = *start;
                let para = &mut self.document.paragraphs[start.paragraph];
                para.insert_text(start.offset, deleted);
            }
            EditOperation::SplitParagraph { cursor } => {
                self.merge_paragraphs(cursor.paragraph);
                self.cursor = *cursor;
            }
            EditOperation::MergeParagraphs { paragraph } => {
                // Re-split at the old boundary (approximate)
                self.cursor = TextCursor {
                    paragraph: *paragraph,
                    offset: self.document.paragraphs[*paragraph].total_len(),
                };
                self.split_paragraph();
                self.cursor.paragraph -= 1;
                self.cursor.offset = self.document.paragraphs[self.cursor.paragraph].total_len();
            }
            EditOperation::ApplyStyle { .. } => {
                // Style undo would require storing the old styles; simplified here
            }
        }
    }

    fn apply_forward(&mut self, op: &EditOperation) {
        match op {
            EditOperation::InsertText { cursor, text } => {
                self.cursor = *cursor;
                let para = &mut self.document.paragraphs[cursor.paragraph];
                para.insert_text(cursor.offset, text);
                self.cursor.offset += text.len();
            }
            EditOperation::DeleteText { start, end, .. } => {
                self.cursor = *start;
                if start.paragraph == end.paragraph {
                    let para = &mut self.document.paragraphs[start.paragraph];
                    para.delete_range(start.offset, end.offset);
                }
            }
            EditOperation::SplitParagraph { cursor } => {
                self.cursor = *cursor;
                self.split_paragraph();
            }
            EditOperation::MergeParagraphs { paragraph } => {
                self.merge_paragraphs(*paragraph);
                self.cursor = TextCursor {
                    paragraph: *paragraph,
                    offset: self.document.paragraphs[*paragraph].total_len(),
                };
            }
            EditOperation::ApplyStyle { .. } => {}
        }
    }

    fn advance_cursor(&self, cursor: TextCursor, len: usize) -> TextCursor {
        TextCursor {
            paragraph: cursor.paragraph,
            offset: cursor.offset + len,
        }
    }
}

// ── String search helper (no regex, simple substring) ───────────────────

fn find_substr(haystack: &str, needle: &str) -> Option<usize> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return None;
    }
    let needle_bytes = needle.as_bytes();
    let hay_bytes = haystack.as_bytes();
    for i in 0..=(hay_bytes.len() - needle_bytes.len()) {
        if &hay_bytes[i..i + needle_bytes.len()] == needle_bytes {
            return Some(i);
        }
    }
    None
}
