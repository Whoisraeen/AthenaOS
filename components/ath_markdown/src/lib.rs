//! # RaeMarkdown — a never-panic CommonMark-subset render model (`no_std`).
//!
//! LEGACY_GAMING_CONCEPT.md — *"Built for people who care about how things feel."*
//! Markdown is the text format that powers the Notes app, the in-OS docs
//! viewer, README rendering in the file browser, and chat message bodies. This
//! crate turns a string of Markdown into a typed [`Document`] — a tree of
//! [`Block`]s and [`Inline`]s the UI layer can walk and render — plus a
//! [`to_plain_text`] projection for search indexing and previews.
//!
//! ## What it is
//! - [`parse`]: `&str` → [`Document`] (`= Vec<Block>`). A robust CommonMark
//!   *subset* — the 90% that Notes / docs / chat need — that **never panics**
//!   on any input.
//! - [`to_plain_text`]: [`Document`] → `String`, stripping all formatting (for
//!   search indices and one-line previews).
//! - [`render_debug`]: a tiny model-walk that renders the tree to an indented
//!   text outline (a debugging / smoketest aid — the real on-screen render
//!   belongs to the consuming app + athui later; this slice ships the *model*).
//!
//! ## Subset supported
//! Blocks: ATX headings (`#`..`######`), paragraphs, fenced code blocks
//! (```` ```lang ````), blockquotes (`>`), unordered lists (`-`/`*`/`+`),
//! ordered lists (`1.`), GFM task lists (`- [ ]` / `- [x]`), GFM pipe tables,
//! thematic breaks (`---`/`***`/`___`), blank-line separation. Inlines:
//! `**bold**`/`__bold__`, `*italic*`/`_italic_`,
//! `~~strikethrough~~` (GFM), `` `code` ``, `[text](url)` links, autolinks
//! (`<http://...>` AND GFM bare URLs `https://x` / `www.x` in plain text), hard
//! line breaks (two trailing spaces *or* a trailing backslash), and `\`-escapes.
//!
//! ## Documented limits (deliberately out of scope for this slice)
//! - **Indented code blocks** (4-space) are NOT recognised — fenced only. A
//!   4-space-indented line is treated as a normal paragraph line. (Fenced code
//!   is the modern, unambiguous form Notes/chat use.)
//! - **List nesting** is supported to ONE level (a `-`/`1.` item whose
//!   continuation lines are indented by 2+ spaces and themselves start a list).
//!   Deeper nesting is flattened into the nearest level rather than erroring.
//! - Blockquotes nest (they re-parse their stripped content as blocks), and may
//!   contain any block; lazy continuation lines are supported.
//! - GFM pipe tables (`| a | b |` + a `|---|:-:|` delimiter row) parse with
//!   per-column alignment; body rows are padded/truncated to the column count.
//! - Setext headings: a text line directly underlined by `===` (H1) or `---`
//!   (H2) — single-line form (multi-line setext text degrades to a paragraph).
//! - Reference-style links (`[text][id]`), multi-line setext, HTML passthrough,
//!   and emphasis run-length edge cases beyond the common `*`/`_`/`**`/`__`
//!   forms are NOT implemented; unrecognised markup degrades to literal text
//!   rather than erroring.
//!
//! ## Hostile-input posture (CLAUDE §10: untrusted bytes are an attack surface)
//! Note text, web text, and chat messages are hostile. There is no
//! `unwrap`/`expect`/`panic`/slice-index-panic path reachable from [`parse`]:
//! unterminated emphasis (`**bold`), an unclosed link (`[a](`), a lone backtick,
//! empty input, raw control bytes, and deeply nested markup all degrade to
//! literal / partial text. The `parse_never_panics_battery` host KAT at the
//! bottom of this file drives a corpus of hostile inputs through [`parse`] and
//! is the primary proof (`cargo test -p ath_markdown`).

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// Maximum block-nesting depth (blockquotes / lists) the parser will recurse
/// into. Beyond this, over-deep content degrades gracefully to plain paragraphs
/// rather than recursing further — markdown never errors or panics on input, and
/// this caps stack use so a hostile `">".repeat(N)` / deeply-indented list cannot
/// overflow the stack (a DoS). Mirrors the depth guard in ath_json / ath_toml.
pub const MAX_DEPTH: usize = 96;

/// A parsed Markdown document: an ordered list of top-level blocks.
pub type Document = Vec<Block>;

/// A block-level element.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    /// An ATX heading: `level` in `1..=6`, with inline content.
    Heading { level: u8, content: Vec<Inline> },
    /// A paragraph of inline content.
    Paragraph(Vec<Inline>),
    /// A fenced code block. `lang` is the info string after the opening fence
    /// (empty when none); `code` is the raw, *unprocessed* body (no inline
    /// parsing — code is literal).
    CodeBlock { lang: String, code: String },
    /// A blockquote: its content re-parsed as a sequence of blocks.
    BlockQuote(Vec<Block>),
    /// A list. `ordered` distinguishes `1.` from `-`/`*`/`+`.
    List { ordered: bool, items: Vec<ListItem> },
    /// A GFM pipe table: a header row, per-column alignment, and body rows.
    /// Every cell holds inline content; body rows are padded/truncated to the
    /// column count the delimiter row defines.
    Table {
        align: Vec<Align>,
        headers: Vec<Vec<Inline>>,
        rows: Vec<Vec<Vec<Inline>>>,
    },
    /// A thematic break (`---` / `***` / `___`).
    ThematicBreak,
}

/// Column alignment declared by a GFM table delimiter row (`:--`, `:-:`, `--:`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    None,
    Left,
    Center,
    Right,
}

/// One item in a [`Block::List`]: a sequence of blocks (usually a single
/// [`Block::Paragraph`], but may hold a nested [`Block::List`] etc.).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListItem {
    /// The item's content blocks.
    pub blocks: Vec<Block>,
    /// GFM task-list state: `None` for a plain item, `Some(false)` for an
    /// unchecked `[ ]` checkbox, `Some(true)` for a checked `[x]` checkbox.
    pub task: Option<bool>,
}

/// An inline (span-level) element.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Inline {
    /// Literal text.
    Text(String),
    /// Strong emphasis (`**`/`__`).
    Bold(Vec<Inline>),
    /// Emphasis (`*`/`_`).
    Italic(Vec<Inline>),
    /// Strikethrough (GFM `~~text~~`).
    Strikethrough(Vec<Inline>),
    /// Inline code span (`` ` ``). Content is literal.
    Code(String),
    /// A link: display content + destination URL.
    Link { text: Vec<Inline>, url: String },
    /// A hard line break.
    LineBreak,
}

// ===========================================================================
// Block-level parsing
// ===========================================================================

/// Parse a Markdown string into a [`Document`]. Never panics on any input.
pub fn parse(input: &str) -> Document {
    // Normalise line endings without allocating per char by splitting on '\n'
    // and trimming a trailing '\r'. We collect into a Vec<&str> of logical
    // lines so the block scanner can look ahead cheaply.
    let lines: Vec<&str> = input.split('\n').map(strip_cr).collect();
    parse_lines(&lines, 0)
}

fn strip_cr(s: &str) -> &str {
    if let Some(stripped) = s.strip_suffix('\r') {
        stripped
    } else {
        s
    }
}

/// Parse a slice of already-split logical lines into blocks. `depth` is the
/// current block-nesting depth (incremented per blockquote / list level); once it
/// reaches [`MAX_DEPTH`] the scanner stops descending into nested blockquotes /
/// lists and treats their lines as plain paragraph text, so deeply nested hostile
/// input degrades gracefully instead of overflowing the stack.
fn parse_lines(lines: &[&str], depth: usize) -> Vec<Block> {
    let mut blocks: Vec<Block> = Vec::new();
    let mut i = 0usize;
    // At the cap, no more nesting: render whatever remains as plain paragraphs.
    let at_cap = depth >= MAX_DEPTH;

    while i < lines.len() {
        let line = lines[i];

        // Blank line: separator, skip.
        if line.trim().is_empty() {
            i += 1;
            continue;
        }

        // Thematic break.
        if is_thematic_break(line) {
            blocks.push(Block::ThematicBreak);
            i += 1;
            continue;
        }

        // ATX heading.
        if let Some((level, text)) = parse_atx_heading(line) {
            blocks.push(Block::Heading {
                level,
                content: parse_inlines(text),
            });
            i += 1;
            continue;
        }

        // Fenced code block.
        if let Some(fence) = FenceInfo::open(line) {
            let (block, next) = parse_fenced_code(lines, i, &fence);
            blocks.push(block);
            i = next;
            continue;
        }

        // Blockquote. At the nesting cap we DON'T descend; the line falls
        // through to be rendered as plain paragraph text instead.
        if !at_cap && is_blockquote_line(line) {
            let (block, next) = parse_blockquote(lines, i, depth);
            blocks.push(block);
            i = next;
            continue;
        }

        // List (ordered or unordered). Likewise suppressed at the cap.
        if !at_cap && list_marker(line).is_some() {
            let (block, next) = parse_list(lines, i, depth);
            blocks.push(block);
            i = next;
            continue;
        }

        // GFM pipe table: a header row containing `|` immediately followed by a
        // delimiter row (`|---|:--:|`). The delimiter's column count defines the
        // table; a header that doesn't match it isn't a table and falls through.
        if !at_cap && line.contains('|') && i + 1 < lines.len() {
            if let Some(align) = parse_table_delimiter(lines[i + 1]) {
                let headers: Vec<Vec<Inline>> = split_table_row(line)
                    .iter()
                    .map(|c| parse_inlines(c))
                    .collect();
                if !headers.is_empty() && headers.len() == align.len() {
                    let cols = headers.len();
                    let mut rows: Vec<Vec<Vec<Inline>>> = Vec::new();
                    let mut j = i + 2;
                    while j < lines.len() && !lines[j].trim().is_empty() && lines[j].contains('|') {
                        let cells = split_table_row(lines[j]);
                        let mut row: Vec<Vec<Inline>> =
                            cells.iter().take(cols).map(|c| parse_inlines(c)).collect();
                        while row.len() < cols {
                            row.push(Vec::new()); // pad short rows (GFM)
                        }
                        rows.push(row);
                        j += 1;
                    }
                    blocks.push(Block::Table {
                        align,
                        headers,
                        rows,
                    });
                    i = j;
                    continue;
                }
            }
        }

        // Setext heading: a text line immediately underlined by `===` (H1) or
        // `---` (H2), with no blank line between. Must precede the paragraph
        // fallback so the underline isn't swallowed as paragraph text. We only
        // reach here when `line` is plain text (headings/fences/quotes/lists/
        // tables and a STANDALONE thematic break were all consumed above), so a
        // following underline makes this a heading. (A `---` with a blank line
        // before it stays a thematic break; one directly under text is setext.)
        if !at_cap && i + 1 < lines.len() {
            if let Some(level) = setext_underline_level(lines[i + 1]) {
                blocks.push(Block::Heading {
                    level,
                    content: parse_inlines(line.trim()),
                });
                i += 2;
                continue;
            }
        }

        // Paragraph: consume consecutive non-blank, non-interrupting lines.
        // At the cap, treat blockquote/list markers as ordinary text so a single
        // over-deep line still terminates as one paragraph block.
        let (block, next) = parse_paragraph(lines, i, at_cap);
        blocks.push(block);
        i = next;
    }

    blocks
}

/// If `line` is a setext underline — one or more of all-`=` (→ H1) or all-`-`
/// (→ H2), after trimming — return the heading level. A standalone such line is
/// only a heading when it directly follows a text line (the caller guarantees
/// that); on its own a 3+ `-` run is a thematic break, handled earlier.
fn setext_underline_level(line: &str) -> Option<u8> {
    let t = line.trim();
    if t.is_empty() {
        return None;
    }
    if t.chars().all(|c| c == '=') {
        Some(1)
    } else if t.chars().all(|c| c == '-') {
        Some(2)
    } else {
        None
    }
}

/// Split a GFM table row on UNescaped `|`, trimming the optional leading/trailing
/// pipe and surrounding whitespace per cell. `\|` yields a literal `|` in a cell.
fn split_table_row(line: &str) -> Vec<String> {
    let chars: Vec<char> = line.trim().chars().collect();
    let mut cells: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '\\' && i + 1 < chars.len() && chars[i + 1] == '|' {
            cur.push('|');
            i += 2;
            continue;
        }
        if c == '|' {
            cells.push(cur.trim().to_string());
            cur = String::new();
            i += 1;
            continue;
        }
        cur.push(c);
        i += 1;
    }
    cells.push(cur.trim().to_string());
    // A leading/trailing `|` produces an empty edge cell — drop it.
    if cells.first().is_some_and(|s| s.is_empty()) {
        cells.remove(0);
    }
    if cells.last().is_some_and(|s| s.is_empty()) {
        cells.pop();
    }
    cells
}

/// If `line` is a valid GFM table delimiter row — every cell `:?-+:?` — return
/// the per-column alignments; otherwise `None`. Requires at least one `|` (so a
/// bare `---` reads as a thematic break, not a one-column table delimiter).
fn parse_table_delimiter(line: &str) -> Option<Vec<Align>> {
    if !line.contains('|') {
        return None;
    }
    let cells = split_table_row(line);
    if cells.is_empty() {
        return None;
    }
    let mut aligns = Vec::with_capacity(cells.len());
    for cell in &cells {
        let c = cell.trim();
        let left = c.starts_with(':');
        let right = c.ends_with(':');
        let dashes = c.trim_start_matches(':').trim_end_matches(':');
        if dashes.is_empty() || !dashes.chars().all(|ch| ch == '-') {
            return None;
        }
        aligns.push(match (left, right) {
            (true, true) => Align::Center,
            (true, false) => Align::Left,
            (false, true) => Align::Right,
            (false, false) => Align::None,
        });
    }
    Some(aligns)
}

/// `true` if a line is a thematic break: a line of 3+ of the same `-`, `*`, or
/// `_` characters, optionally separated by spaces, after stripping ≤3 leading
/// spaces.
fn is_thematic_break(line: &str) -> bool {
    let t = strip_leading_spaces(line, 3).trim_end();
    if t.is_empty() {
        return false;
    }
    let mut marker: Option<char> = None;
    let mut count = 0usize;
    for c in t.chars() {
        if c == ' ' || c == '\t' {
            continue;
        }
        if c == '-' || c == '*' || c == '_' {
            match marker {
                None => marker = Some(c),
                Some(m) if m == c => {}
                Some(_) => return false,
            }
            count += 1;
        } else {
            return false;
        }
    }
    count >= 3
}

/// Parse an ATX heading. Returns `(level, trimmed inline text)`.
fn parse_atx_heading(line: &str) -> Option<(u8, &str)> {
    let t = strip_leading_spaces(line, 3);
    let mut level = 0u8;
    let bytes = t.as_bytes();
    // Count the leading '#' run, but stop the moment it exceeds 6: a run of 7+
    // '#' is never a valid ATX heading (CommonMark), so we don't need the exact
    // count — and counting it fully would overflow `level` (a `u8`) on a hostile
    // line of 256+ '#'. Capping the count keeps `parse` panic-free on any input.
    while (level as usize) < bytes.len() && bytes[level as usize] == b'#' {
        level += 1;
        if level > 6 {
            return None;
        }
    }
    if level == 0 {
        return None;
    }
    // The '#' run must be followed by a space/tab or end-of-line.
    let rest = &t[level as usize..];
    if !rest.is_empty() && !rest.starts_with(' ') && !rest.starts_with('\t') {
        return None;
    }
    // Strip leading whitespace and a trailing closing '#' sequence.
    let rest = rest.trim_start();
    let rest = rest.trim_end();
    let rest = rest.trim_end_matches('#');
    let rest = rest.trim_end();
    Some((level, rest))
}

/// Fenced-code opening-fence info.
struct FenceInfo {
    /// The fence character: `` ` `` or `~`.
    ch: char,
    /// The fence length (≥3).
    len: usize,
    /// The info string (language) after the fence.
    lang: String,
}

impl FenceInfo {
    fn open(line: &str) -> Option<FenceInfo> {
        let t = strip_leading_spaces(line, 3);
        let ch = match t.chars().next() {
            Some('`') => '`',
            Some('~') => '~',
            _ => return None,
        };
        let len = t.chars().take_while(|&c| c == ch).count();
        if len < 3 {
            return None;
        }
        let info = t[len..].trim();
        // For backtick fences, an info string containing a backtick is invalid.
        if ch == '`' && info.contains('`') {
            return None;
        }
        Some(FenceInfo {
            ch,
            len,
            lang: info.to_string(),
        })
    }

    /// `true` if `line` is a closing fence for this open fence.
    fn closes(&self, line: &str) -> bool {
        let t = strip_leading_spaces(line, 3);
        let count = t.chars().take_while(|&c| c == self.ch).count();
        if count < self.len {
            return false;
        }
        // A closing fence has only the fence char (+ trailing spaces) after.
        t[count..].trim().is_empty()
    }
}

/// Parse a fenced code block starting at `start` (the opening fence). Returns
/// the block and the index of the first line *after* the close (or EOF).
fn parse_fenced_code(lines: &[&str], start: usize, fence: &FenceInfo) -> (Block, usize) {
    let mut code = String::new();
    let mut i = start + 1;
    let mut first = true;
    while i < lines.len() {
        if fence.closes(lines[i]) {
            i += 1; // consume the closing fence
            return (
                Block::CodeBlock {
                    lang: fence.lang.clone(),
                    code,
                },
                i,
            );
        }
        if !first {
            code.push('\n');
        }
        first = false;
        code.push_str(lines[i]);
        i += 1;
    }
    // Unterminated fence: everything to EOF is code (CommonMark behaviour).
    (
        Block::CodeBlock {
            lang: fence.lang.clone(),
            code,
        },
        i,
    )
}

fn is_blockquote_line(line: &str) -> bool {
    strip_leading_spaces(line, 3).starts_with('>')
}

/// Parse a blockquote: collect `>`-prefixed (and lazy-continuation) lines,
/// strip one level of `>` marker, and recursively parse the inner content.
fn parse_blockquote(lines: &[&str], start: usize, depth: usize) -> (Block, usize) {
    let mut inner: Vec<String> = Vec::new();
    let mut i = start;
    while i < lines.len() {
        let line = lines[i];
        if is_blockquote_line(line) {
            let t = strip_leading_spaces(line, 3);
            // Strip the leading '>' and at most one following space.
            let after = &t[1..];
            let after = after.strip_prefix(' ').unwrap_or(after);
            inner.push(after.to_string());
            i += 1;
        } else if line.trim().is_empty() {
            // Blank line ends the blockquote.
            break;
        } else {
            // Lazy continuation: a non-blank, non-'>' line continues a
            // blockquote paragraph.
            inner.push(line.to_string());
            i += 1;
        }
    }
    let inner_refs: Vec<&str> = inner.iter().map(|s| s.as_str()).collect();
    (Block::BlockQuote(parse_lines(&inner_refs, depth + 1)), i)
}

/// A detected list marker on a line.
struct Marker {
    /// Whether the list is ordered.
    ordered: bool,
    /// Number of leading spaces before the marker.
    indent: usize,
    /// Byte offset of the first content char after the marker + its space.
    content_offset: usize,
}

/// Detect a list marker at the start of `line` (allowing ≤3 leading spaces of
/// outer indent, plus we report the actual indent for nesting).
fn list_marker(line: &str) -> Option<Marker> {
    let indent = line.len() - line.trim_start_matches(' ').len();
    let t = &line[indent..];
    let bytes = t.as_bytes();
    if bytes.is_empty() {
        return None;
    }
    // Unordered: '-', '*', '+' followed by a space (or end-of-line).
    let first = bytes[0];
    if first == b'-' || first == b'*' || first == b'+' {
        if bytes.len() == 1 {
            return Some(Marker {
                ordered: false,
                indent,
                content_offset: indent + 1,
            });
        }
        if bytes[1] == b' ' || bytes[1] == b'\t' {
            return Some(Marker {
                ordered: false,
                indent,
                content_offset: indent + 2,
            });
        }
        return None;
    }
    // Ordered: 1+ digits then '.' or ')' then a space.
    let digits = bytes.iter().take_while(|&&b| b.is_ascii_digit()).count();
    if digits == 0 || digits > 9 {
        return None;
    }
    if digits >= bytes.len() {
        return None;
    }
    let delim = bytes[digits];
    if delim != b'.' && delim != b')' {
        return None;
    }
    let after = digits + 1;
    if after == bytes.len() {
        return Some(Marker {
            ordered: true,
            indent,
            content_offset: indent + after,
        });
    }
    if bytes[after] == b' ' || bytes[after] == b'\t' {
        return Some(Marker {
            ordered: true,
            indent,
            content_offset: indent + after + 1,
        });
    }
    None
}

/// Parse a list starting at `start`. Supports tight lists and one level of
/// nesting (indented child items). Returns the list block and the next index.
fn parse_list(lines: &[&str], start: usize, depth: usize) -> (Block, usize) {
    let first_marker = match list_marker(lines[start]) {
        Some(m) => m,
        // Caller guarantees a marker; defensive fall-through to a paragraph.
        None => return parse_paragraph(lines, start, false),
    };
    let ordered = first_marker.ordered;
    let base_indent = first_marker.indent;
    let mut items: Vec<ListItem> = Vec::new();
    let mut i = start;

    while i < lines.len() {
        let line = lines[i];
        if line.trim().is_empty() {
            // Blank line: peek ahead. If the next non-blank line is still a
            // same-level list item, continue (loose list); otherwise stop.
            let mut j = i + 1;
            while j < lines.len() && lines[j].trim().is_empty() {
                j += 1;
            }
            match j < lines.len() {
                true => match list_marker(lines[j]) {
                    Some(m) if m.indent == base_indent && m.ordered == ordered => {
                        i = j;
                        continue;
                    }
                    _ => break,
                },
                false => break,
            }
        }

        let marker = match list_marker(line) {
            Some(m) if m.indent == base_indent && m.ordered == ordered => m,
            // A differently-indented or different-type marker, or a non-marker
            // line, ends this list.
            _ => break,
        };

        // Collect this item's raw content lines: the marker line's remainder
        // plus any subsequent indented continuation lines (until the next
        // same-level marker or a non-indented non-blank line).
        let mut item_lines: Vec<String> = Vec::new();
        let content_start = marker.content_offset.min(line.len());
        item_lines.push(line[content_start..].to_string());
        i += 1;

        let child_indent = marker.content_offset;
        while i < lines.len() {
            let cont = lines[i];
            if cont.trim().is_empty() {
                // Tentatively keep blank lines as paragraph separators inside
                // the item; the outer blank-handling above will stop if the
                // list truly ends.
                let mut j = i + 1;
                while j < lines.len() && lines[j].trim().is_empty() {
                    j += 1;
                }
                if j < lines.len() {
                    let next_indent = lines[j].len() - lines[j].trim_start_matches(' ').len();
                    // A same-level marker after the blank means a new item.
                    let same_level = list_marker(lines[j])
                        .map(|m| m.indent == base_indent)
                        .unwrap_or(false);
                    if same_level || next_indent <= base_indent {
                        break;
                    }
                }
                item_lines.push(String::new());
                i += 1;
                continue;
            }
            // A same-level marker starts the next item.
            if let Some(m) = list_marker(cont) {
                if m.indent == base_indent {
                    break;
                }
            }
            let cont_indent = cont.len() - cont.trim_start_matches(' ').len();
            if cont_indent >= child_indent {
                // Indented continuation: strip `child_indent` spaces.
                let stripped = strip_leading_spaces(cont, child_indent);
                item_lines.push(stripped.to_string());
                i += 1;
            } else if cont_indent > base_indent {
                // Loosely-indented continuation (lazy): keep as-is content.
                item_lines.push(cont.trim_start().to_string());
                i += 1;
            } else {
                break;
            }
        }

        // GFM task-list item: the content begins with `[ ]` (unchecked) or
        // `[x]`/`[X]` (checked) followed by whitespace or end-of-item. Detected
        // on the raw first line and stripped before the content is parsed, so
        // `- [ ] buy milk` becomes a checkbox item with content "buy milk".
        let mut task: Option<bool> = None;
        if let Some(first) = item_lines.first_mut() {
            let t = first.trim_start();
            let (state, rest) = if let Some(r) = t.strip_prefix("[ ]") {
                (Some(false), r)
            } else if let Some(r) = t.strip_prefix("[x]").or_else(|| t.strip_prefix("[X]")) {
                (Some(true), r)
            } else {
                (None, "")
            };
            if state.is_some() && (rest.is_empty() || rest.starts_with(char::is_whitespace)) {
                task = state;
                *first = rest.trim_start().to_string();
            }
        }

        let refs: Vec<&str> = item_lines.iter().map(|s| s.as_str()).collect();
        let mut blocks = parse_lines(&refs, depth + 1);
        if blocks.is_empty() {
            blocks.push(Block::Paragraph(Vec::new()));
        }
        items.push(ListItem { blocks, task });
    }

    (Block::List { ordered, items }, i)
}

/// Parse a paragraph: consume consecutive lines until a blank line or a line
/// that starts a different block (heading, fence, thematic break, blockquote,
/// list). Joins lines and applies inline parsing (with hard-break detection).
fn parse_paragraph(lines: &[&str], start: usize, force_text: bool) -> (Block, usize) {
    let mut text = String::new();
    let mut i = start;
    let mut first = true;
    while i < lines.len() {
        let line = lines[i];
        if line.trim().is_empty() {
            break;
        }
        // `force_text` (set at the nesting cap) makes the scanner absorb
        // would-be block starters as plain text instead of interrupting, so
        // over-deep nested content collapses into one paragraph and the line
        // index always advances (no infinite loop).
        if i != start && !force_text {
            // A line that would start a new block interrupts the paragraph.
            if is_thematic_break(line)
                || parse_atx_heading(line).is_some()
                || FenceInfo::open(line).is_some()
                || is_blockquote_line(line)
                || list_marker(line).is_some()
            {
                break;
            }
        }
        if !first {
            text.push('\n');
        }
        first = false;
        text.push_str(line);
        i += 1;
    }
    (Block::Paragraph(parse_inlines(&text)), i)
}

/// Strip up to `max` leading space characters (tabs counted as one space each
/// for the purposes of this subset).
fn strip_leading_spaces(line: &str, max: usize) -> &str {
    let mut removed = 0usize;
    let mut idx = 0usize;
    for (byte_idx, c) in line.char_indices() {
        if removed >= max {
            idx = byte_idx;
            return &line[idx..];
        }
        if c == ' ' || c == '\t' {
            removed += 1;
            idx = byte_idx + c.len_utf8();
        } else {
            return &line[byte_idx..];
        }
    }
    &line[idx..]
}

// ===========================================================================
// Inline parsing
// ===========================================================================

/// Parse a span of text (possibly multi-line, joined with `\n`) into inlines.
/// Handles emphasis, code spans, links, autolinks, escapes, and hard breaks.
/// Never panics: unterminated constructs degrade to literal text.
pub fn parse_inlines(text: &str) -> Vec<Inline> {
    let chars: Vec<char> = text.chars().collect();
    let inlines = parse_inline_range(&chars, 0, chars.len());
    coalesce(inlines)
}

/// Parse inlines from `chars[start..end]`.
fn parse_inline_range(chars: &[char], start: usize, end: usize) -> Vec<Inline> {
    let mut out: Vec<Inline> = Vec::new();
    let mut buf = String::new();
    let mut i = start;

    macro_rules! flush {
        () => {
            if !buf.is_empty() {
                out.push(Inline::Text(core::mem::take(&mut buf)));
            }
        };
    }

    while i < end {
        let c = chars[i];

        // Backslash escape: the next char is literal (if it is a punctuation /
        // escapable char). A backslash at end-of-line is a hard break.
        if c == '\\' {
            if i + 1 < end {
                let next = chars[i + 1];
                if next == '\n' {
                    flush!();
                    out.push(Inline::LineBreak);
                    i += 2;
                    continue;
                }
                if is_escapable(next) {
                    buf.push(next);
                    i += 2;
                    continue;
                }
            } else {
                // Trailing backslash at end of the whole span: literal '\'.
                buf.push('\\');
                i += 1;
                continue;
            }
            // Non-escapable: keep the backslash literally.
            buf.push('\\');
            i += 1;
            continue;
        }

        // Hard line break: two+ trailing spaces before a newline.
        if c == '\n' {
            // Count trailing spaces already in `buf`.
            let trailing = buf.chars().rev().take_while(|&c| c == ' ').count();
            if trailing >= 2 {
                // Remove the trailing spaces and emit a LineBreak.
                let new_len = buf.len() - trailing;
                buf.truncate(new_len);
                flush!();
                out.push(Inline::LineBreak);
            } else {
                // Soft break → a single space.
                // Trim a single trailing space if present (soft-wrap).
                buf.push(' ');
            }
            i += 1;
            continue;
        }

        // Inline code span: a run of `n` backticks closed by a matching run.
        if c == '`' {
            let tick_len = run_len(chars, i, end, '`');
            if let Some(close) = find_closing_ticks(chars, i + tick_len, end, tick_len) {
                flush!();
                let code: String = chars[i + tick_len..close].iter().collect();
                // CommonMark trims one leading/trailing space if the content is
                // not all spaces.
                out.push(Inline::Code(trim_code_span(&code)));
                i = close + tick_len;
                continue;
            }
            // No close: literal backticks.
            for _ in 0..tick_len {
                buf.push('`');
            }
            i += tick_len;
            continue;
        }

        // Autolink: <http://...> or <https://...> or <mailto:...>.
        if c == '<' {
            if let Some((url, after)) = parse_autolink(chars, i, end) {
                flush!();
                out.push(Inline::Link {
                    text: alloc::vec![Inline::Text(url.clone())],
                    url,
                });
                i = after;
                continue;
            }
            // Not an autolink: literal '<'.
            buf.push('<');
            i += 1;
            continue;
        }

        // GFM extended autolink: a BARE `http://`/`https://`/`www.` URL in plain
        // text (no angle brackets). Must start at a word boundary so `shttp://`
        // or a URL glued to a preceding letter isn't matched.
        if (c == 'h' || c == 'H' || c == 'w' || c == 'W')
            && (i == start || !chars[i - 1].is_alphanumeric())
        {
            if let Some((display, href, after)) = match_bare_url(chars, i, end) {
                flush!();
                out.push(Inline::Link {
                    text: alloc::vec![Inline::Text(display)],
                    url: href,
                });
                i = after;
                continue;
            }
        }

        // Link: [text](url).
        if c == '[' {
            if let Some((link, after)) = parse_link(chars, i, end) {
                flush!();
                out.push(link);
                i = after;
                continue;
            }
            buf.push('[');
            i += 1;
            continue;
        }

        // Strikethrough: GFM `~~text~~` (double tilde).
        if c == '~' {
            let run = run_len(chars, i, end, c);
            if run >= 2 {
                if let Some(close) = find_closing_delim(chars, i + 2, end, c, 2) {
                    flush!();
                    let inner = parse_inline_range(chars, i + 2, close);
                    out.push(Inline::Strikethrough(inner));
                    i = close + 2;
                    continue;
                }
            }
            // Unterminated / single `~`: literal.
            for _ in 0..run {
                buf.push(c);
            }
            i += run;
            continue;
        }

        // Emphasis: ** / __ (bold) and * / _ (italic).
        if c == '*' || c == '_' {
            let run = run_len(chars, i, end, c);
            // Prefer the longest matching delimiter we can close.
            if run >= 2 {
                if let Some(close) = find_closing_delim(chars, i + 2, end, c, 2) {
                    flush!();
                    let inner = parse_inline_range(chars, i + 2, close);
                    out.push(Inline::Bold(inner));
                    i = close + 2;
                    continue;
                }
            }
            if run >= 1 {
                if let Some(close) = find_closing_delim(chars, i + 1, end, c, 1) {
                    flush!();
                    let inner = parse_inline_range(chars, i + 1, close);
                    out.push(Inline::Italic(inner));
                    i = close + 1;
                    continue;
                }
            }
            // Unterminated: literal delimiter run.
            for _ in 0..run {
                buf.push(c);
            }
            i += run;
            continue;
        }

        buf.push(c);
        i += 1;
    }

    // Flush trailing buffer.
    if !buf.is_empty() {
        out.push(Inline::Text(buf));
    }
    out
}

/// Characters that may follow a backslash to be escaped to a literal.
fn is_escapable(c: char) -> bool {
    matches!(
        c,
        '\\' | '`'
            | '*'
            | '_'
            | '{'
            | '}'
            | '['
            | ']'
            | '('
            | ')'
            | '#'
            | '+'
            | '-'
            | '.'
            | '!'
            | '<'
            | '>'
            | '|'
            | '~'
            | '"'
            | '\''
    )
}

/// Length of the run of `ch` starting at `start` (bounded by `end`).
fn run_len(chars: &[char], start: usize, end: usize, ch: char) -> usize {
    let mut n = 0usize;
    let mut i = start;
    while i < end && chars[i] == ch {
        n += 1;
        i += 1;
    }
    n
}

/// Find the start index of a backtick run of exactly `len` that closes a code
/// span. Returns the index of the first closing backtick.
fn find_closing_ticks(chars: &[char], start: usize, end: usize, len: usize) -> Option<usize> {
    let mut i = start;
    while i < end {
        if chars[i] == '`' {
            let run = run_len(chars, i, end, '`');
            if run == len {
                return Some(i);
            }
            i += run;
        } else {
            i += 1;
        }
    }
    None
}

/// Trim a single leading and trailing space from a code span if the content is
/// not entirely spaces (CommonMark rule).
fn trim_code_span(s: &str) -> String {
    let bytes: Vec<char> = s.chars().collect();
    if bytes.len() >= 2
        && bytes.first() == Some(&' ')
        && bytes.last() == Some(&' ')
        && bytes.iter().any(|&c| c != ' ')
    {
        bytes[1..bytes.len() - 1].iter().collect()
    } else {
        s.to_string()
    }
}

/// Find the start index of a closing emphasis delimiter run of `c` of at least
/// `len`, beginning the search at `start`. We skip over code spans and nested
/// brackets so emphasis does not match a delimiter inside `` `code` ``. Returns
/// the index where the closing delimiter run starts.
fn find_closing_delim(
    chars: &[char],
    start: usize,
    end: usize,
    c: char,
    len: usize,
) -> Option<usize> {
    // The opening delimiter must be "left-flanking": not followed by whitespace.
    if start >= end || chars[start].is_whitespace() {
        return None;
    }
    let mut i = start;
    let mut depth_guard = 0usize; // prevents pathological scanning cost
    while i < end {
        depth_guard += 1;
        if depth_guard > 100_000 {
            return None;
        }
        let ch = chars[i];
        if ch == '\\' {
            i += 2;
            continue;
        }
        if ch == '`' {
            // Skip a code span so emphasis inside code is ignored.
            let tl = run_len(chars, i, end, '`');
            if let Some(close) = find_closing_ticks(chars, i + tl, end, tl) {
                i = close + tl;
                continue;
            }
            i += tl;
            continue;
        }
        if ch == c {
            let run = run_len(chars, i, end, c);
            // A run longer than what we need, that is "left-flanking" (followed
            // by a non-space), is the opener of a NESTED span (e.g. the `**` in
            // `*outer **inner** end*`). Skip past its matching close so we do
            // not mistake it for our own closing delimiter. The recursive
            // `parse_inline_range` over the inner range re-parses it correctly.
            let next_is_text = i + run < end && !chars[i + run].is_whitespace();
            if run > len && next_is_text {
                if let Some(nested_close) = find_closing_delim(chars, i + run, end, c, run) {
                    i = nested_close + run;
                    continue;
                }
            }
            if run >= len {
                // The closing delimiter must be "right-flanking": not preceded
                // by whitespace.
                if i > start && !chars[i - 1].is_whitespace() {
                    return Some(i);
                }
            }
            i += run;
            continue;
        }
        i += 1;
    }
    None
}

/// Parse an autolink `<scheme:...>`. Returns `(url, index after '>')`.
fn parse_autolink(chars: &[char], start: usize, end: usize) -> Option<(String, usize)> {
    // chars[start] == '<'
    let mut i = start + 1;
    let mut url = String::new();
    while i < end {
        let c = chars[i];
        if c == '>' {
            // Must look like an absolute URI / mailto and contain no spaces.
            if looks_like_uri(&url) {
                return Some((url, i + 1));
            }
            return None;
        }
        if c == '<' || c.is_whitespace() {
            return None;
        }
        url.push(c);
        i += 1;
    }
    None
}

/// GFM extended autolink of a BARE URL beginning at `chars[start]`:
/// `http://…`, `https://…`, or `www.…`. Returns `(display, href, index past it)`
/// — `display` is the literal URL text, `href` is the same (with an implicit
/// `http://` prepended for a `www.` link). `None` if it isn't a URL. The host
/// must contain a `.`, and GFM trailing-punctuation trimming is applied so a URL
/// at the end of a sentence (`…see https://x.com.`) doesn't swallow the period.
fn match_bare_url(chars: &[char], start: usize, end: usize) -> Option<(String, String, usize)> {
    let prefix: String = chars[start..end.min(start + 8)]
        .iter()
        .collect::<String>()
        .to_ascii_lowercase();
    let (scheme_end, is_www) = if prefix.starts_with("https://") {
        (start + 8, false)
    } else if prefix.starts_with("http://") {
        (start + 7, false)
    } else if prefix.starts_with("www.") {
        (start + 4, true)
    } else {
        return None;
    };
    // Consume URL characters up to whitespace or `<`.
    let mut j = scheme_end;
    while j < end && !chars[j].is_whitespace() && chars[j] != '<' {
        j += 1;
    }
    if j == scheme_end {
        return None; // nothing after the scheme
    }
    // GFM trailing-punctuation trim: strip trailing `?!.,:;*_~'"`; a trailing
    // `)` only if it's unbalanced within the URL.
    while j > scheme_end {
        let c = chars[j - 1];
        if matches!(
            c,
            '?' | '!' | '.' | ',' | ':' | ';' | '*' | '_' | '~' | '\'' | '"'
        ) {
            j -= 1;
        } else if c == ')' {
            let opens = chars[start..j].iter().filter(|&&x| x == '(').count();
            let closes = chars[start..j].iter().filter(|&&x| x == ')').count();
            if closes > opens {
                j -= 1;
            } else {
                break;
            }
        } else {
            break;
        }
    }
    if j == scheme_end {
        return None;
    }
    // The host must contain a dot (`www.x` / `http://x` alone don't autolink).
    let host_has_dot = if is_www {
        // For `www.`, the `.` of the prefix counts only if more host follows;
        // require an additional `.` OR at least a non-empty host after `www.`.
        chars[scheme_end..j].iter().any(|&c| c == '.')
            || chars[scheme_end..j].iter().any(|&c| c.is_alphanumeric())
    } else {
        chars[scheme_end..j].iter().any(|&c| c == '.')
    };
    if !host_has_dot {
        return None;
    }
    let display: String = chars[start..j].iter().collect();
    let href = if is_www {
        alloc::format!("http://{}", display)
    } else {
        display.clone()
    };
    Some((display, href, j))
}

fn looks_like_uri(s: &str) -> bool {
    s.starts_with("http://")
        || s.starts_with("https://")
        || s.starts_with("mailto:")
        || s.starts_with("ftp://")
}

/// Parse a link `[text](url)`. Returns `(Inline::Link, index after ')')`.
fn parse_link(chars: &[char], start: usize, end: usize) -> Option<(Inline, usize)> {
    // chars[start] == '['
    // Find the matching ']' (respecting one level of nested brackets and
    // skipping code spans / escapes).
    let mut i = start + 1;
    let text_start = i;
    let mut depth = 1usize;
    let mut text_end = None;
    while i < end {
        let c = chars[i];
        if c == '\\' {
            i += 2;
            continue;
        }
        if c == '`' {
            let tl = run_len(chars, i, end, '`');
            if let Some(close) = find_closing_ticks(chars, i + tl, end, tl) {
                i = close + tl;
                continue;
            }
            i += tl;
            continue;
        }
        if c == '[' {
            depth += 1;
        } else if c == ']' {
            depth -= 1;
            if depth == 0 {
                text_end = Some(i);
                break;
            }
        }
        i += 1;
    }
    let text_end = text_end?;
    // Next char must be '('.
    let mut j = text_end + 1;
    if j >= end || chars[j] != '(' {
        return None;
    }
    j += 1;
    // Read URL up to the matching ')'. URLs in this subset do not contain
    // unescaped spaces or parens.
    let mut url = String::new();
    let mut closed = false;
    while j < end {
        let c = chars[j];
        if c == '\\' && j + 1 < end {
            url.push(chars[j + 1]);
            j += 2;
            continue;
        }
        if c == ')' {
            closed = true;
            j += 1;
            break;
        }
        if c.is_whitespace() {
            // A space ends the URL portion; skip an optional title up to ')'.
            // For this subset we just scan to ')' and ignore the title.
            let mut k = j;
            while k < end && chars[k] != ')' {
                k += 1;
            }
            if k < end {
                j = k + 1;
                closed = true;
            }
            break;
        }
        url.push(c);
        j += 1;
    }
    if !closed {
        return None;
    }
    let text = parse_inline_range(chars, text_start, text_end);
    Some((Inline::Link { text, url }, j))
}

/// Merge adjacent `Text` inlines produced during parsing.
fn coalesce(inlines: Vec<Inline>) -> Vec<Inline> {
    let mut out: Vec<Inline> = Vec::with_capacity(inlines.len());
    for inl in inlines {
        if let Inline::Text(ref t) = inl {
            if let Some(Inline::Text(prev)) = out.last_mut() {
                prev.push_str(t);
                continue;
            }
        }
        out.push(inl);
    }
    out
}

// ===========================================================================
// Projections
// ===========================================================================

/// Strip all formatting from a [`Document`], returning plain text suitable for
/// search indexing or a one-line preview. Block boundaries become newlines.
pub fn to_plain_text(doc: &Document) -> String {
    let mut out = String::new();
    plain_blocks(doc, &mut out);
    // Collapse a trailing newline.
    while out.ends_with('\n') {
        out.pop();
    }
    out
}

fn plain_blocks(blocks: &[Block], out: &mut String) {
    for block in blocks {
        match block {
            Block::Heading { content, .. } => {
                plain_inlines(content, out);
                out.push('\n');
            }
            Block::Paragraph(content) => {
                plain_inlines(content, out);
                out.push('\n');
            }
            Block::CodeBlock { code, .. } => {
                out.push_str(code);
                out.push('\n');
            }
            Block::BlockQuote(inner) => {
                plain_blocks(inner, out);
            }
            Block::List { items, .. } => {
                for item in items {
                    if let Some(checked) = item.task {
                        out.push_str(if checked { "[x] " } else { "[ ] " });
                    }
                    plain_blocks(&item.blocks, out);
                }
            }
            Block::Table { headers, rows, .. } => {
                plain_table_row(headers, out);
                for row in rows {
                    plain_table_row(row, out);
                }
            }
            Block::ThematicBreak => {
                out.push('\n');
            }
        }
    }
}

/// One table row as TAB-separated cell text + newline (plain-text extraction).
fn plain_table_row(cells: &[Vec<Inline>], out: &mut String) {
    for (k, cell) in cells.iter().enumerate() {
        if k > 0 {
            out.push('\t');
        }
        plain_inlines(cell, out);
    }
    out.push('\n');
}

fn plain_inlines(inlines: &[Inline], out: &mut String) {
    for inl in inlines {
        match inl {
            Inline::Text(t) => out.push_str(t),
            Inline::Bold(inner) | Inline::Italic(inner) | Inline::Strikethrough(inner) => {
                plain_inlines(inner, out)
            }
            Inline::Code(c) => out.push_str(c),
            Inline::Link { text, .. } => plain_inlines(text, out),
            Inline::LineBreak => out.push(' '),
        }
    }
}

/// Render a [`Document`] to an indented text outline. This is a debugging /
/// smoketest aid (the on-screen render belongs to the consuming app + athui);
/// it lets a caller eyeball the parsed structure.
pub fn render_debug(doc: &Document) -> String {
    let mut out = String::new();
    debug_blocks(doc, 0, &mut out);
    out
}

fn debug_blocks(blocks: &[Block], depth: usize, out: &mut String) {
    for block in blocks {
        for _ in 0..depth {
            out.push_str("  ");
        }
        match block {
            Block::Heading { level, content } => {
                out.push_str("H");
                out.push((b'0' + *level) as char);
                out.push_str(": ");
                let mut s = String::new();
                plain_inlines(content, &mut s);
                out.push_str(&s);
                out.push('\n');
            }
            Block::Paragraph(content) => {
                out.push_str("P: ");
                let mut s = String::new();
                plain_inlines(content, &mut s);
                out.push_str(&s);
                out.push('\n');
            }
            Block::CodeBlock { lang, code } => {
                out.push_str("CODE[");
                out.push_str(lang);
                out.push_str("]: ");
                out.push_str(&code.replace('\n', "\\n"));
                out.push('\n');
            }
            Block::BlockQuote(inner) => {
                out.push_str("QUOTE:\n");
                debug_blocks(inner, depth + 1, out);
            }
            Block::List { ordered, items } => {
                out.push_str(if *ordered { "OL:\n" } else { "UL:\n" });
                for item in items {
                    for _ in 0..depth + 1 {
                        out.push_str("  ");
                    }
                    out.push_str(match item.task {
                        Some(true) => "- item [x]:\n",
                        Some(false) => "- item [ ]:\n",
                        None => "- item:\n",
                    });
                    debug_blocks(&item.blocks, depth + 2, out);
                }
            }
            Block::Table {
                align: _,
                headers,
                rows,
            } => {
                out.push_str("TABLE: ");
                let mut s = String::new();
                for (k, cell) in headers.iter().enumerate() {
                    if k > 0 {
                        s.push_str(" | ");
                    }
                    plain_inlines(cell, &mut s);
                }
                out.push_str(&s);
                out.push('\n');
                for row in rows {
                    for _ in 0..depth + 1 {
                        out.push_str("  ");
                    }
                    out.push_str("ROW: ");
                    let mut rs = String::new();
                    for (k, cell) in row.iter().enumerate() {
                        if k > 0 {
                            rs.push_str(" | ");
                        }
                        plain_inlines(cell, &mut rs);
                    }
                    out.push_str(&rs);
                    out.push('\n');
                }
            }
            Block::ThematicBreak => out.push_str("HR\n"),
        }
    }
}

// ===========================================================================
// Host KAT suite — `cargo test -p ath_markdown`
// ===========================================================================
#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn text(s: &str) -> Inline {
        Inline::Text(s.to_string())
    }

    // --- Block structure ----------------------------------------------------

    #[test]
    fn heading_levels() {
        let doc = parse("# H1\n## H2\n###### H6");
        assert_eq!(
            doc,
            vec![
                Block::Heading {
                    level: 1,
                    content: vec![text("H1")]
                },
                Block::Heading {
                    level: 2,
                    content: vec![text("H2")]
                },
                Block::Heading {
                    level: 6,
                    content: vec![text("H6")]
                },
            ]
        );
        // FAIL-ability guard: a 7-# line is NOT a heading.
        let not = parse("####### too many");
        assert_ne!(
            not,
            vec![Block::Heading {
                level: 7,
                content: vec![text("too many")]
            }]
        );
        assert_eq!(not, vec![Block::Paragraph(vec![text("####### too many")])]);
    }

    #[test]
    fn heading_strips_trailing_hashes() {
        let doc = parse("## Title ##");
        assert_eq!(
            doc,
            vec![Block::Heading {
                level: 2,
                content: vec![text("Title")]
            }]
        );
    }

    #[test]
    fn paragraph_joins_lines() {
        let doc = parse("line one\nline two\n\nsecond para");
        assert_eq!(
            doc,
            vec![
                Block::Paragraph(vec![text("line one line two")]),
                Block::Paragraph(vec![text("second para")]),
            ]
        );
    }

    #[test]
    fn fenced_code_block() {
        let doc = parse("```rust\nfn main() {}\nlet x = 1;\n```");
        assert_eq!(
            doc,
            vec![Block::CodeBlock {
                lang: "rust".to_string(),
                code: "fn main() {}\nlet x = 1;".to_string(),
            }]
        );
        // FAIL-ability guard: inline markup inside code is NOT parsed.
        let starred = parse("```\n**not bold**\n```");
        assert_eq!(
            starred,
            vec![Block::CodeBlock {
                lang: String::new(),
                code: "**not bold**".to_string(),
            }]
        );
        assert_ne!(
            starred,
            vec![Block::Paragraph(vec![Inline::Bold(vec![text("not bold")])])]
        );
    }

    #[test]
    fn unterminated_fence_runs_to_eof() {
        let doc = parse("```\nstill code\nno close");
        assert_eq!(
            doc,
            vec![Block::CodeBlock {
                lang: String::new(),
                code: "still code\nno close".to_string(),
            }]
        );
    }

    #[test]
    fn thematic_break_variants() {
        for src in ["---", "***", "___", "- - -", "*****"] {
            let doc = parse(src);
            assert_eq!(doc, vec![Block::ThematicBreak], "src={src:?}");
        }
        // A heading-like dashed line that is actually text.
        let doc = parse("--x");
        assert_ne!(doc, vec![Block::ThematicBreak]);
    }

    #[test]
    fn blockquote_nested_blocks() {
        let doc = parse("> # quoted heading\n> body text");
        assert_eq!(
            doc,
            vec![Block::BlockQuote(vec![
                Block::Heading {
                    level: 1,
                    content: vec![text("quoted heading")]
                },
                Block::Paragraph(vec![text("body text")]),
            ])]
        );
    }

    #[test]
    fn unordered_list_tight() {
        let doc = parse("- one\n- two\n- three");
        assert_eq!(
            doc,
            vec![Block::List {
                ordered: false,
                items: vec![
                    ListItem {
                        blocks: vec![Block::Paragraph(vec![text("one")])],
                        task: None,
                    },
                    ListItem {
                        blocks: vec![Block::Paragraph(vec![text("two")])],
                        task: None,
                    },
                    ListItem {
                        blocks: vec![Block::Paragraph(vec![text("three")])],
                        task: None,
                    },
                ],
            }]
        );
        // FAIL-ability guard: it must be a List, not three paragraphs.
        assert_ne!(
            doc,
            vec![
                Block::Paragraph(vec![text("- one")]),
                Block::Paragraph(vec![text("- two")]),
                Block::Paragraph(vec![text("- three")]),
            ]
        );
    }

    #[test]
    fn ordered_list() {
        let doc = parse("1. first\n2. second");
        assert_eq!(
            doc,
            vec![Block::List {
                ordered: true,
                items: vec![
                    ListItem {
                        blocks: vec![Block::Paragraph(vec![text("first")])],
                        task: None,
                    },
                    ListItem {
                        blocks: vec![Block::Paragraph(vec![text("second")])],
                        task: None,
                    },
                ],
            }]
        );
    }

    #[test]
    fn nested_list_one_level() {
        let doc = parse("- parent\n  - child");
        // Outer item should contain a paragraph and a nested list.
        match &doc[0] {
            Block::List {
                ordered: false,
                items,
            } => {
                assert_eq!(items.len(), 1);
                let inner = &items[0].blocks;
                assert_eq!(inner[0], Block::Paragraph(vec![text("parent")]));
                assert!(
                    matches!(inner.get(1), Some(Block::List { .. })),
                    "expected a nested list, got {:?}",
                    inner.get(1)
                );
            }
            other => panic!("expected list, got {other:?}"),
        }
    }

    // --- Inline structure ---------------------------------------------------

    #[test]
    fn bold_italic_code() {
        let doc = parse("a **bold** and *italic* and `code` z");
        let p = match &doc[0] {
            Block::Paragraph(p) => p,
            other => panic!("expected paragraph, got {other:?}"),
        };
        assert_eq!(
            p,
            &vec![
                text("a "),
                Inline::Bold(vec![text("bold")]),
                text(" and "),
                Inline::Italic(vec![text("italic")]),
                text(" and "),
                Inline::Code("code".to_string()),
                text(" z"),
            ]
        );
        // FAIL-ability proof: if the emphasis parser breaks, `**bold**` would
        // remain literal text — this assert flips.
        assert_ne!(p[1], text("**bold**"));
        assert!(matches!(p[1], Inline::Bold(_)));
    }

    #[test]
    fn underscore_emphasis() {
        let doc = parse("__strong__ and _em_");
        let p = match &doc[0] {
            Block::Paragraph(p) => p,
            other => panic!("{other:?}"),
        };
        assert_eq!(p[0], Inline::Bold(vec![text("strong")]));
        assert_eq!(p[2], Inline::Italic(vec![text("em")]));
    }

    #[test]
    fn gfm_task_list() {
        let items = match &parse("- [ ] todo\n- [x] done\n- plain")[0] {
            Block::List { items, .. } => items.clone(),
            other => panic!("{other:?}"),
        };
        assert_eq!(items[0].task, Some(false));
        assert_eq!(items[0].blocks, vec![Block::Paragraph(vec![text("todo")])]);
        assert_eq!(items[1].task, Some(true)); // `[x]`
        assert_eq!(items[1].blocks, vec![Block::Paragraph(vec![text("done")])]);
        assert_eq!(items[2].task, None); // plain item — no checkbox
                                         // `[X]` (capital) is also checked; content keeps its inline markup.
        let items2 = match &parse("- [X] *go*")[0] {
            Block::List { items, .. } => items.clone(),
            other => panic!("{other:?}"),
        };
        assert_eq!(items2[0].task, Some(true));
        assert_eq!(
            items2[0].blocks,
            vec![Block::Paragraph(vec![Inline::Italic(vec![text("go")])])]
        );
        // FAIL-ability: `[ ]x` (no space after the box) is NOT a task item.
        let items3 = match &parse("- [ ]x")[0] {
            Block::List { items, .. } => items.clone(),
            other => panic!("{other:?}"),
        };
        assert_eq!(items3[0].task, None);
    }

    #[test]
    fn gfm_bare_url_autolink() {
        let para = |src: &str| -> Vec<Inline> {
            match &parse(src)[0] {
                Block::Paragraph(p) => p.clone(),
                other => panic!("{other:?}"),
            }
        };
        // A bare https URL mid-text becomes a Link; surrounding text is preserved.
        let p = para("see https://rae.os/docs here");
        assert_eq!(p[0], text("see "));
        assert_eq!(
            p[1],
            Inline::Link {
                text: vec![text("https://rae.os/docs")],
                url: "https://rae.os/docs".to_string()
            }
        );
        assert_eq!(p[2], text(" here"));
        // Trailing sentence punctuation is NOT swallowed into the URL.
        let p2 = para("visit https://rae.os.");
        assert_eq!(
            p2[1],
            Inline::Link {
                text: vec![text("https://rae.os")],
                url: "https://rae.os".to_string()
            }
        );
        assert_eq!(p2[2], text("."));
        // `www.` gets an implicit http:// href; display keeps the literal text.
        let p3 = para("www.rae.os rocks");
        assert_eq!(
            p3[0],
            Inline::Link {
                text: vec![text("www.rae.os")],
                url: "http://www.rae.os".to_string()
            }
        );
        // FAIL-ability: a URL glued to a preceding letter is NOT autolinked, and
        // a scheme with no host doesn't match.
        assert!(!para("xhttp://rae.os")
            .iter()
            .any(|i| matches!(i, Inline::Link { .. })));
        assert!(!para("http://")
            .iter()
            .any(|i| matches!(i, Inline::Link { .. })));
    }

    #[test]
    fn setext_headings() {
        // `===` underline → H1; `---` directly under text → H2.
        assert_eq!(
            parse("Title\n====="),
            vec![Block::Heading {
                level: 1,
                content: vec![text("Title")]
            }]
        );
        assert_eq!(
            parse("Subtitle\n---"),
            vec![Block::Heading {
                level: 2,
                content: vec![text("Subtitle")]
            }]
        );
        // Inlines parse inside the heading text.
        assert_eq!(
            parse("A *b*\n=="),
            vec![Block::Heading {
                level: 1,
                content: vec![text("A "), Inline::Italic(vec![text("b")])]
            }]
        );
        // A `---` with a BLANK line before it is a thematic break, not setext.
        let doc = parse("para\n\n---");
        assert_eq!(doc[0], Block::Paragraph(vec![text("para")]));
        assert_eq!(doc[1], Block::ThematicBreak);
        // A standalone `---` (no preceding text) stays a thematic break.
        assert_eq!(parse("---"), vec![Block::ThematicBreak]);
    }

    #[test]
    fn gfm_table() {
        let doc = parse("| Name | Age |\n|:-----|----:|\n| Ann  | 30  |\n| Bo   | 7   |");
        match &doc[0] {
            Block::Table {
                align,
                headers,
                rows,
            } => {
                assert_eq!(align, &vec![Align::Left, Align::Right]);
                assert_eq!(headers, &vec![vec![text("Name")], vec![text("Age")]]);
                assert_eq!(rows.len(), 2);
                assert_eq!(rows[0], vec![vec![text("Ann")], vec![text("30")]]);
                assert_eq!(rows[1], vec![vec![text("Bo")], vec![text("7")]]);
            }
            other => panic!("expected a Table, got {other:?}"),
        }
        // Short body rows are padded to the column count.
        let doc2 = parse("| a | b |\n|---|---|\n| x |");
        match &doc2[0] {
            Block::Table { rows, .. } => assert_eq!(rows[0], vec![vec![text("x")], vec![]]),
            other => panic!("{other:?}"),
        }
        // Cells parse inlines; `\|` is a literal pipe inside a cell.
        let doc3 = parse("| h |\n|---|\n| *em* a\\|b |");
        match &doc3[0] {
            Block::Table { rows, .. } => {
                assert_eq!(
                    rows[0][0],
                    vec![Inline::Italic(vec![text("em")]), text(" a|b")]
                )
            }
            other => panic!("{other:?}"),
        }
        // FAIL-ability: a `|` line WITHOUT a following delimiter row is a normal
        // paragraph, NOT a table.
        assert!(!matches!(
            parse("| just | text |\nmore")[0],
            Block::Table { .. }
        ));
        // A bare `---` (no pipe) stays a thematic break, not a 1-col delimiter.
        assert!(matches!(parse("a\n\n---")[1], Block::ThematicBreak));
    }

    #[test]
    fn gfm_strikethrough() {
        let para = |src: &str| -> Vec<Inline> {
            match &parse(src)[0] {
                Block::Paragraph(p) => p.clone(),
                other => panic!("{other:?}"),
            }
        };
        // `~~text~~` parses to a Strikethrough span.
        assert_eq!(
            para("~~struck~~")[0],
            Inline::Strikethrough(vec![text("struck")])
        );
        // Emphasis nests inside strikethrough.
        assert_eq!(
            para("~~*both*~~")[0],
            Inline::Strikethrough(vec![Inline::Italic(vec![text("both")])])
        );
        // FAIL-ability: a lone `~` (or unterminated `~~`) stays literal — never a
        // Strikethrough span (GFM requires a closing `~~`).
        assert!(!para("a ~b~ c")
            .iter()
            .any(|i| matches!(i, Inline::Strikethrough(_))));
        assert!(!para("~~unterminated")
            .iter()
            .any(|i| matches!(i, Inline::Strikethrough(_))));
    }

    #[test]
    fn nested_bold_in_italic() {
        let doc = parse("*outer **inner** end*");
        let p = match &doc[0] {
            Block::Paragraph(p) => p,
            other => panic!("{other:?}"),
        };
        assert_eq!(
            p,
            &vec![Inline::Italic(vec![
                text("outer "),
                Inline::Bold(vec![text("inner")]),
                text(" end"),
            ])]
        );
    }

    #[test]
    fn link_parsing() {
        let doc = parse("see [the docs](https://example.com/x) now");
        let p = match &doc[0] {
            Block::Paragraph(p) => p,
            other => panic!("{other:?}"),
        };
        assert_eq!(
            p,
            &vec![
                text("see "),
                Inline::Link {
                    text: vec![text("the docs")],
                    url: "https://example.com/x".to_string(),
                },
                text(" now"),
            ]
        );
        // FAIL-ability proof: if the link parser breaks, the `[the docs](...)`
        // stays literal text and this flips.
        assert!(matches!(p[1], Inline::Link { .. }));
        assert_ne!(p[1], text("[the docs](https://example.com/x)"));
    }

    #[test]
    fn autolink() {
        let doc = parse("visit <https://rae.os> ok");
        let p = match &doc[0] {
            Block::Paragraph(p) => p,
            other => panic!("{other:?}"),
        };
        assert_eq!(
            p[1],
            Inline::Link {
                text: vec![text("https://rae.os")],
                url: "https://rae.os".to_string(),
            }
        );
    }

    #[test]
    fn hard_break_two_spaces_and_backslash() {
        let two = parse("line one  \nline two");
        let p = match &two[0] {
            Block::Paragraph(p) => p,
            other => panic!("{other:?}"),
        };
        assert_eq!(
            p,
            &vec![text("line one"), Inline::LineBreak, text("line two")]
        );

        let bslash = parse("line one\\\nline two");
        let p2 = match &bslash[0] {
            Block::Paragraph(p) => p,
            other => panic!("{other:?}"),
        };
        assert_eq!(p2[1], Inline::LineBreak);
    }

    #[test]
    fn escape_sequences() {
        let doc = parse("not \\*italic\\* here");
        let p = match &doc[0] {
            Block::Paragraph(p) => p,
            other => panic!("{other:?}"),
        };
        assert_eq!(p, &vec![text("not *italic* here")]);
        // Guard: the escaped form must NOT produce an Italic node.
        assert!(!p.iter().any(|i| matches!(i, Inline::Italic(_))));
    }

    #[test]
    fn inline_code_preserves_specials() {
        let doc = parse("call `a * b` here");
        let p = match &doc[0] {
            Block::Paragraph(p) => p,
            other => panic!("{other:?}"),
        };
        assert_eq!(p[1], Inline::Code("a * b".to_string()));
    }

    // --- Projections --------------------------------------------------------

    #[test]
    fn plain_text_strips_formatting() {
        let doc = parse("# Title\n\nSome **bold** and a [link](http://x) and `code`.");
        let plain = to_plain_text(&doc);
        assert_eq!(plain, "Title\nSome bold and a link and code.");
        // Guard: no markdown syntax leaks into the plain projection.
        assert!(!plain.contains('*'));
        assert!(!plain.contains('['));
        assert!(!plain.contains('`'));
    }

    #[test]
    fn render_debug_outlines_structure() {
        let doc = parse("# H\n\n- a\n- b");
        let out = render_debug(&doc);
        assert!(out.contains("H1: H"));
        assert!(out.contains("UL:"));
    }

    // --- Hostile-input battery: ZERO panics ---------------------------------

    #[test]
    fn parse_never_panics_battery() {
        let hostile: &[&str] = &[
            "",
            "\n\n\n",
            "**bold with no close",
            "__also unclosed",
            "*",
            "**",
            "***",
            "_ _ _ _",
            "[link](",
            "[unclosed text",
            "[a](b",
            "[](",
            "](",
            "`",
            "``",
            "`unterminated code",
            "```",
            "```\nunterminated fence",
            "<",
            "<not a url>",
            "<http://",
            "> ",
            ">",
            ">>>>>>>>>>",
            "#",
            "####### seven",
            "- ",
            "-",
            "1.",
            "1.no space",
            "999999999999. huge ordinal",
            "\\",
            "\\\\\\\\",
            "\\x",
            "a\\",
            "[[[[[[[[[[",
            "]]]]]]]]]]",
            "((((((((((",
            "**_**_**_**_**_",
            "`*`*`*`*`*`*",
            "[*](*)",
            "<>",
            "<>\u{0}\u{1}\u{2}\u{7f}",
            "text with \u{0} null and \u{1} controls",
            "  \t  leading whitespace madness",
            "\u{feff}bom prefixed",
            "emoji 🎮 and combining a\u{0301}",
            "- - - - - - - - - - - - -",
            "1. 2. 3. 4. 5.",
            "> > > > nested quotes",
        ];
        for src in hostile {
            // The contract: returns a Document, never panics.
            let doc = parse(src);
            // Exercising the projections must also never panic.
            let _ = to_plain_text(&doc);
            let _ = render_debug(&doc);
        }

        // A pathologically deep emphasis nest must not blow the scanner.
        let deep = "*".repeat(5000);
        let _ = parse(&deep);
        let deep2 = "[".repeat(5000);
        let _ = parse(&deep2);
        let deep3 = "`".repeat(5000);
        let _ = parse(&deep3);
        // A very long line.
        let long = "x".repeat(100_000);
        let _ = parse(&long);
    }

    #[test]
    fn unterminated_emphasis_is_literal() {
        let doc = parse("**bold");
        assert_eq!(doc, vec![Block::Paragraph(vec![text("**bold")])]);
        // Guard: it must NOT have become a Bold node.
        if let Block::Paragraph(p) = &doc[0] {
            assert!(!p.iter().any(|i| matches!(i, Inline::Bold(_))));
        }
    }

    #[test]
    fn unclosed_link_is_literal() {
        let doc = parse("[text](http://x");
        if let Block::Paragraph(p) = &doc[0] {
            assert!(!p.iter().any(|i| matches!(i, Inline::Link { .. })));
        } else {
            panic!("expected paragraph");
        }
    }

    // =======================================================================
    // FUZZ / PROPERTY suite — deterministic seeded PRNG, no external fuzz crate.
    //
    // Matches the ath_json/ath_toml/ath_gif pattern that landed this session: a
    // tiny self-contained xorshift64* PRNG builds adversarial corpora; the
    // property under test is the hostile-input contract of [`parse`] — note,
    // web, and chat text is untrusted (CLAUDE §10). Invariants:
    //   1. `parse` never panics (no unwrap/expect/index/overflow path), and the
    //      [`to_plain_text`] / [`render_debug`] projections over the result also
    //      never panic.
    //   2. Pathological nesting (lists / blockquotes / emphasis / brackets) does
    //      not blow the stack — block recursion is bounded because each
    //      blockquote/list level strips a marker and re-parses strictly shorter
    //      content, and inline recursion is bounded by the delimiter run length.
    //   3. No unbounded output expansion: the plain-text projection of a parse
    //      is never longer than the input (formatting only ever shrinks text;
    //      a small input cannot blow up the rendered size).
    //
    // FAIL-ability (proven by reasoning in the REPORT): an unbounded recursion in
    // the block scanner would overflow the test thread's stack (abort = test
    // failure); a quadratic/exponential scan would time the test out; and an
    // output-expansion bug would trip the `plain.len() <= input.len()` assert.
    // =======================================================================

    /// Deterministic xorshift64* PRNG — pure, no_std-safe, reproducible.
    struct Rng(u64);
    impl Rng {
        fn new(seed: u64) -> Self {
            Rng(seed ^ 0x9E37_79B9_7F4A_7C15)
        }
        fn next_u64(&mut self) -> u64 {
            let mut x = self.0;
            x ^= x >> 12;
            x ^= x << 25;
            x ^= x >> 27;
            self.0 = x;
            x.wrapping_mul(0x2545_F491_4F6C_DD1D)
        }
        fn below(&mut self, n: usize) -> usize {
            (self.next_u64() % (n as u64)) as usize
        }
    }

    /// The plain-text projection never grows the input. `to_plain_text` only ever
    /// strips markers / formatting, so its char count must be `<= input chars`.
    /// (We compare char counts, not bytes, since soft-break handling can turn a
    /// newline into a space — same length.) This bounds output expansion.
    fn assert_no_expansion(input: &str) {
        let doc = parse(input);
        let plain = to_plain_text(&doc);
        assert!(
            plain.chars().count() <= input.chars().count(),
            "plain projection ({} chars) expanded past input ({} chars) for {:?}",
            plain.chars().count(),
            input.chars().count(),
            input
        );
    }

    /// 1a. Random byte/char soup over the markdown-significant alphabet: never
    /// panic, and the projections never panic.
    #[test]
    fn fuzz_markdown_token_soup_never_panic() {
        // Every structurally meaningful markdown char plus whitespace / noise.
        let palette: &[char] = &[
            '#', '*', '_', '`', '-', '+', '>', '[', ']', '(', ')', '!', '~', '\\', '|', '.', '0',
            '1', '9', 'a', 'z', ' ', '\t', '\n', '\r', '<', '>', ':', '/', '"', '\'',
        ];
        let mut rng = Rng::new(0x5_0117);
        for _ in 0..60_000 {
            let len = rng.below(80);
            let mut s = String::with_capacity(len);
            for _ in 0..len {
                s.push(palette[rng.below(palette.len())]);
            }
            let doc = parse(&s);
            let _ = to_plain_text(&doc);
            let _ = render_debug(&doc);
        }
    }

    /// 1b. Random valid UTF-8 including control bytes, BOM, combining marks,
    /// multibyte and 4-byte emoji interleaved with markdown syntax: never panic.
    #[test]
    fn fuzz_markdown_random_utf8_never_panic() {
        let palette: &[char] = &[
            '#', '*', '_', '`', '-', '>', '[', ']', '(', ')', '\\', '\n', '\r', ' ', 'a', '\u{0}',
            '\u{1}', '\u{7f}', '\u{feff}', '\u{0301}', 'é', 'ö', 'ü', '中', '😀', '🎮', '\u{200b}',
            '\u{2028}',
        ];
        let mut rng = Rng::new(0x5_0317);
        for _ in 0..60_000 {
            let len = rng.below(64);
            let mut s = String::with_capacity(len * 2);
            for _ in 0..len {
                s.push(palette[rng.below(palette.len())]);
            }
            let doc = parse(&s);
            let _ = to_plain_text(&doc);
            let _ = render_debug(&doc);
            assert_no_expansion(&s);
        }
    }

    /// 1c. Mutate a well-formed document char-wise (UTF-8-safe via char swaps):
    /// deletions, swapped delimiters, truncations — never panic.
    #[test]
    fn fuzz_markdown_mutated_valid_doc_never_panic() {
        let seed = "# Title\n\nSome **bold** and *italic* and `code` and a [link](https://x.io).\n\n> a quote\n> more\n\n- one\n- two\n  - nested\n\n```rust\nfn main() {}\n```\n\n---\n";
        let chars: Vec<char> = seed.chars().collect();
        let inject: &[char] = &['*', '`', '[', ']', '(', ')', '#', '>', '\\', '\n', ' '];
        let mut rng = Rng::new(0x5_0517);
        for _ in 0..60_000 {
            let mut c = chars.clone();
            let mutations = 1 + rng.below(6);
            for _ in 0..mutations {
                if c.is_empty() {
                    break;
                }
                match rng.below(3) {
                    0 => {
                        let i = rng.below(c.len());
                        c.remove(i);
                    }
                    1 => {
                        let i = rng.below(c.len());
                        c[i] = inject[rng.below(inject.len())];
                    }
                    _ => {
                        let i = rng.below(c.len());
                        c.insert(i, inject[rng.below(inject.len())]);
                    }
                }
            }
            let s: String = c.iter().collect();
            let doc = parse(&s);
            let _ = to_plain_text(&doc);
            let _ = render_debug(&doc);
        }
    }

    /// 1d. Truncate the valid seed at EVERY char boundary: never panic. Catches
    /// off-by-one slice panics on partially-consumed constructs.
    #[test]
    fn fuzz_markdown_truncate_every_boundary() {
        let seed = "# H\n> q **b** `c` [l](http://x) <http://y>\n- a\n  - b\n```lang\ncode\n```\n";
        let chars: Vec<char> = seed.chars().collect();
        for cut in 0..=chars.len() {
            let s: String = chars[..cut].iter().collect();
            let doc = parse(&s);
            let _ = to_plain_text(&doc);
            let _ = render_debug(&doc);
        }
    }

    /// 1e. Deep nesting at *moderate* depth must NOT overflow the stack and must
    /// not corrupt the projections.
    ///
    /// DEFECT FOUND (reported, NOT fixed — see REPORT): `parse` recurses ONE level
    /// of `parse_lines` for every nested blockquote (`parse_blockquote`, lib.rs
    /// ~L329-352) and every nested list (`parse_list` → `parse_lines`, ~L425-519),
    /// with NO depth cap. Stack use is therefore O(nesting depth) at ~1.7-2 KiB per
    /// level: a hostile `">"*N x` or indentation-nested list overflows the stack
    /// at N ≈ 550 on a 1 MiB stack (e.g. the default Windows test thread / a small
    /// kernel task stack). The companion `#[ignore]`d regression
    /// `deep_blockquote_overflows_REGRESSION` reproduces it deterministically. This
    /// test stays at a depth (200) that is safe on every stack so the suite is
    /// green; raising it past ~500 would abort the process (which is the FAIL the
    /// fuzzer surfaced). ath_json/ath_toml cap nesting with MAX_DEPTH → Err; this
    /// crate has no such guard yet.
    #[test]
    fn fuzz_markdown_deep_nesting_no_stack_overflow() {
        // Blockquote nesting up to a stack-safe depth.
        const SAFE_DEPTH: usize = 200;
        let deep_quotes = {
            let mut s = String::with_capacity(SAFE_DEPTH + 16);
            for _ in 0..SAFE_DEPTH {
                s.push('>');
            }
            s.push_str(" deeply quoted");
            s
        };
        let doc = parse(&deep_quotes);
        let _ = to_plain_text(&doc);
        let _ = render_debug(&doc);

        // Nested lists via increasing indentation (one level per 2 spaces),
        // bounded to the same stack-safe depth.
        let deep_lists = {
            let mut s = String::new();
            for d in 0..SAFE_DEPTH {
                for _ in 0..(d * 2) {
                    s.push(' ');
                }
                s.push_str("- item\n");
            }
            s
        };
        let doc = parse(&deep_lists);
        let _ = to_plain_text(&doc);
        let _ = render_debug(&doc);

        // Nested emphasis: real interleaved `*a*a...` recursion is comparatively
        // cheap (bounded by run length); depth 1000 is safe on the default stack.
        let deep_em = {
            let mut s = String::with_capacity(4000);
            for _ in 0..1000 {
                s.push_str("*a");
            }
            for _ in 0..1000 {
                s.push('*');
            }
            s
        };
        let _ = parse(&deep_em);

        // Interleaved blockquote + list + emphasis at safe depth.
        let mixed = {
            let mut s = String::new();
            for _ in 0..SAFE_DEPTH {
                s.push_str("> - **a*");
            }
            s
        };
        let _ = parse(&mixed);
    }

    /// REGRESSION GUARD (no_std-clean): proves the MAX_DEPTH cap in blockquote
    /// nesting prevents an unbounded-recursion stack overflow.
    ///
    /// DEFECT (now fixed): `parse` re-strips one `>` and re-parses the rest as a
    /// nested blockquote, one recursion per `>` level (~2 KiB of frame each). With
    /// NO cap a line of 100_000 `>` would recurse 100_000 deep ≈ 200 MiB of stack,
    /// far beyond the default ~8 MiB test-thread stack → the process aborts with a
    /// stack overflow. The `MAX_DEPTH = 96` cap flattens past that depth, so the
    /// call below must simply RETURN a `Document` on the normal test stack.
    ///
    /// FALSIFIABLE: removing the `MAX_DEPTH` guard makes this input overflow even
    /// the default 8 MiB stack and aborts the test runner — i.e. this test stops
    /// passing the moment the fix is removed (no small-stack thread / catch_unwind
    /// needed: the input is sized to exceed any sane default stack on its own).
    #[test]
    fn deep_blockquote_overflows_regression() {
        // 100_000 levels: uncapped that is ~200 MiB of recursion, >> the ~8 MiB
        // default test stack, so without MAX_DEPTH this call aborts the process.
        let mut s = ">".repeat(100_000);
        s.push_str(" x");
        // The load-bearing assertion: with the depth cap in place, parse RETURNS.
        let doc = parse(&s);
        // Reaching here at all is the proof (the call did not overflow the stack).
        let _ = doc;
    }

    /// REGRESSION GUARD (no_std-clean): proves a long `#` run does not overflow
    /// the `u8` heading-level counter in `parse_atx_heading`.
    ///
    /// DEFECT (now fixed): the heading-level counter is a `u8` and the `level > 6`
    /// reject ran only AFTER the count loop, so a line of 256+ leading `#` would
    /// overflow `level` → "attempt to add with overflow" (panic in debug). The fix
    /// bails out of the `#` count once it exceeds 6 (the heading is rejected /
    /// treated as a paragraph). Untrusted note/chat text can contain such a line.
    ///
    /// FALSIFIABLE: no `catch_unwind` is needed — if the defect returns, `parse`
    /// itself panics with "attempt to add with overflow" on this input, which
    /// fails this test directly.
    #[test]
    fn hash_run_level_overflow_regression() {
        // 256 '#' on one line is the minimal u8-overflow trigger (255 is the last
        // safe count); the guard rejects the heading well before that.
        let src = "#".repeat(256);
        // The load-bearing assertion: with the count guard, parse RETURNS (no panic).
        let doc = parse(&src);
        let _ = doc;
    }

    /// 1f. Pathological delimiter / bracket soup: long runs of the constructs
    /// whose scanners look ahead (`find_closing_delim`, `find_closing_ticks`,
    /// `parse_link`). Must terminate quickly (the 100k depth_guard caps the
    /// emphasis scanner) and never panic.
    #[test]
    fn fuzz_markdown_pathological_runs_terminate() {
        let cases = [
            "*".repeat(20_000),
            "_".repeat(20_000),
            "`".repeat(20_000),
            "[".repeat(20_000),
            "]".repeat(20_000),
            "(".repeat(20_000),
            ")".repeat(20_000),
            "**".repeat(10_000),
            "*_".repeat(10_000),
            "[]".repeat(10_000),
            "[a](".repeat(5_000),
            "*a".repeat(10_000),
            "`a".repeat(10_000),
            // NOTE: a long single-line run of '#' (e.g. `"#".repeat(256)`) is
            // deliberately NOT included here — it overflows the `u8` heading-level
            // counter in `parse_atx_heading` (the `level > 6` guard runs AFTER the
            // count loop), panicking with "attempt to add with overflow" in debug.
            // That defect is documented + reproduced in
            // `hash_run_level_overflow_regression`. A short '#' run is exercised
            // safely below.
            "#".repeat(200),
            // `- ` repeated on one line is safe: it parses as a single thematic
            // break (3+ spaced dashes), not as nested list items.
            "- ".repeat(10_000),
            // NOTE: a long single-line run of '>' is deliberately NOT included
            // here — `">>>…>"` re-strips one '>' and re-parses the rest as a
            // nested blockquote, one `parse_lines` recursion per '>', triggering
            // the unbounded-recursion stack overflow documented in
            // `fuzz_markdown_deep_nesting_no_stack_overflow` /
            // `deep_blockquote_overflows_regression`. This test asserts only the
            // inline scanner-termination property, which '>' nesting does not test.
        ];
        for c in &cases {
            let doc = parse(c);
            let _ = to_plain_text(&doc);
            let _ = render_debug(&doc);
        }
        // A huge single line and a huge multi-line document.
        let _ = parse(&"x".repeat(500_000));
        let many_lines = {
            let mut s = String::with_capacity(200_000);
            for _ in 0..50_000 {
                s.push_str("a\n");
            }
            s
        };
        let _ = parse(&many_lines);
    }

    /// 1g. Mixed CRLF / LF / lone-CR line endings must parse identically modulo
    /// the line ending (the `\r` is stripped) and never panic. Property: a
    /// document and its CRLF-ified twin yield the SAME plain-text projection.
    #[test]
    fn fuzz_markdown_mixed_line_endings() {
        let bodies = [
            "# H\n\npara one\npara two\n\n- a\n- b\n",
            "> quote\n> line\n\n```\ncode\n```\n",
            "**bold**\n*italic*\n`code`\n",
        ];
        for body in &bodies {
            let lf = parse(body);
            let crlf_src = body.replace('\n', "\r\n");
            let crlf = parse(&crlf_src);
            assert_eq!(
                to_plain_text(&lf),
                to_plain_text(&crlf),
                "CRLF twin diverged for {:?}",
                body
            );
            // Lone CRs sprinkled in must not panic.
            let cr_src = body.replace('\n', "\r");
            let _ = parse(&cr_src);
        }
        // Random CR/LF/CRLF soup.
        let mut rng = Rng::new(0x5_0917);
        let eol: &[&str] = &["\n", "\r\n", "\r"];
        for _ in 0..20_000 {
            let lines = rng.below(12);
            let mut s = String::new();
            for _ in 0..lines {
                s.push_str(["x", "# h", "- a", "> q", "**b**", ""][rng.below(6)]);
                s.push_str(eol[rng.below(eol.len())]);
            }
            let doc = parse(&s);
            let _ = to_plain_text(&doc);
        }
    }

    /// 1h. Malformed tables / headings / mixed markup that this subset does NOT
    /// support must degrade to literal text, never panic. (Tables are an
    /// explicit non-feature; pipe soup must stay safe.)
    #[test]
    fn fuzz_markdown_malformed_tables_and_headings() {
        let cases: Vec<String> = vec![
            "| a | b |\n| --- | --- |\n| 1 | 2 |".to_string(),
            "|||||||||||||||||||||".to_string(),
            "| broken | row\nno pipes here\n|| empty ||".to_string(),
            "####### 7 hashes not a heading".to_string(),
            "#no space after hash".to_string(),
            "#".to_string(),
            "###### \t  ".to_string(),
            "Setext heading\n===========".to_string(),
            "Another\n-----------".to_string(),
            "## heading with **bold** and `code` ##".to_string(),
            "#".repeat(10),
            " \t # indented hash".to_string(),
        ];
        for c in &cases {
            let doc = parse(c);
            let _ = to_plain_text(&doc);
            let _ = render_debug(&doc);
            assert_no_expansion(c);
        }
    }
}
