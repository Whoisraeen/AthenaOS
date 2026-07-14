//! Clipboard manager for the AthenaOS desktop shell.
//!
//! Provides a multi-format clipboard with persistent history, pinning,
//! search, listener notifications, and memory-bounded storage.
//!
//! The clipboard-history panel (`clipboard_panel`) now drives this manager as
//! its ordering authority — `new`/`copy`/`pin`/`history` are live. The remaining
//! manager surface (`paste`/`cut`/`search`/format queries/listeners/`clear*`) is
//! the full public API a userspace clipboard service will use, so it is marked
//! `dead_code` rather than deleted (the methods are reachable + tested-shaped).

use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

// ── Types ────────────────────────────────────────────────────────────────

// The full multi-format content model. The panel uses `Text` today; the rich
// variants are the manager's public surface (reserved for the userspace
// clipboard service), hence `dead_code` rather than deleted.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub enum ClipboardContent {
    Text(String),
    RichText {
        html: String,
        plain: String,
    },
    Image {
        width: u32,
        height: u32,
        data: Vec<u8>,
        format: ImageFormat,
    },
    Files(Vec<String>),
    Url(String),
    Color(u32),
    Custom {
        mime_type: String,
        data: Vec<u8>,
    },
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageFormat {
    Png,
    Jpeg,
    Bmp,
    Svg,
}

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClipboardFormat {
    Text,
    Html,
    Image,
    Files,
    Url,
    Custom(String),
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct ClipboardEntry {
    pub id: u64,
    pub content: ClipboardContent,
    pub source_app: Option<String>,
    pub timestamp: u64,
    pub pinned: bool,
    pub paste_count: u32,
    pub preview: Option<String>,
    pub size: usize,
}

// ── ClipboardManager ─────────────────────────────────────────────────────

#[allow(dead_code)]
pub struct ClipboardManager {
    history: Vec<ClipboardEntry>,
    max_history: usize,
    current: Option<usize>,
    pinned: Vec<usize>,
    formats: Vec<ClipboardFormat>,
    sync_enabled: bool,
    max_entry_size: usize,
    total_size: usize,
    max_total_size: usize,
    listeners: Vec<u64>,
    paste_count: u64,
    next_id: u64,
    current_time: u64,
}

#[allow(dead_code)] // panel uses new/copy/pin/history; rest is the service API
impl ClipboardManager {
    pub fn new(max_history: usize) -> Self {
        Self {
            history: Vec::new(),
            max_history,
            current: None,
            pinned: Vec::new(),
            formats: Vec::new(),
            sync_enabled: false,
            max_entry_size: 10 * 1024 * 1024,
            total_size: 0,
            max_total_size: 100 * 1024 * 1024,
            listeners: Vec::new(),
            paste_count: 0,
            next_id: 1,
            current_time: 0,
        }
    }

    pub fn copy(&mut self, content: ClipboardContent, source: Option<&str>) {
        let size = Self::calculate_size(&content);

        if size > self.max_entry_size {
            return;
        }

        let preview = Self::generate_preview(&content);
        let format = Self::content_format(&content);

        let entry = ClipboardEntry {
            id: self.next_id,
            content,
            source_app: source.map(String::from),
            timestamp: self.current_time,
            pinned: false,
            paste_count: 0,
            preview,
            size,
        };
        self.next_id += 1;
        self.total_size += size;

        self.history.insert(0, entry);
        self.current = Some(0);

        if !self.formats.contains(&format) {
            self.formats.push(format);
        }

        self.enforce_limits();
        self.notify_listeners();
    }

    pub fn paste(&mut self) -> Option<&ClipboardContent> {
        let idx = self.current?;
        if idx < self.history.len() {
            self.history[idx].paste_count += 1;
            self.paste_count += 1;
            Some(&self.history[idx].content)
        } else {
            None
        }
    }

    pub fn paste_at(&self, index: usize) -> Option<&ClipboardContent> {
        self.history.get(index).map(|e| &e.content)
    }

    pub fn cut(&mut self, content: ClipboardContent, source: Option<&str>) {
        self.copy(content, source);
    }

    pub fn clear(&mut self) {
        if let Some(idx) = self.current {
            if idx < self.history.len() && !self.history[idx].pinned {
                let size = self.history[idx].size;
                self.total_size = self.total_size.saturating_sub(size);
                self.history.remove(idx);
                self.current = if self.history.is_empty() {
                    None
                } else {
                    Some(0)
                };
                self.rebuild_pinned();
            }
        }
    }

    pub fn clear_history(&mut self) {
        let pinned_entries: Vec<ClipboardEntry> =
            self.history.drain(..).filter(|e| e.pinned).collect();

        self.total_size = pinned_entries.iter().map(|e| e.size).sum();
        self.history = pinned_entries;
        self.current = if self.history.is_empty() {
            None
        } else {
            Some(0)
        };
        self.rebuild_pinned();
        self.formats.clear();

        for entry in &self.history {
            let fmt = Self::content_format(&entry.content);
            if !self.formats.contains(&fmt) {
                self.formats.push(fmt);
            }
        }
    }

    pub fn pin(&mut self, index: usize) {
        if index < self.history.len() {
            self.history[index].pinned = true;
            if !self.pinned.contains(&index) {
                self.pinned.push(index);
            }
        }
    }

    pub fn unpin(&mut self, index: usize) {
        if index < self.history.len() {
            self.history[index].pinned = false;
            self.pinned.retain(|&i| i != index);
        }
    }

    pub fn delete_entry(&mut self, index: usize) {
        if index < self.history.len() {
            if self.history[index].pinned {
                return;
            }
            let size = self.history[index].size;
            self.total_size = self.total_size.saturating_sub(size);
            self.history.remove(index);
            self.rebuild_pinned();

            match self.current {
                Some(cur) if cur == index => {
                    self.current = if self.history.is_empty() {
                        None
                    } else {
                        Some(0)
                    };
                }
                Some(cur) if cur > index => {
                    self.current = Some(cur - 1);
                }
                _ => {}
            }
        }
    }

    pub fn search(&self, query: &str) -> Vec<(usize, &ClipboardEntry)> {
        let q = to_lowercase_clip(query);
        self.history
            .iter()
            .enumerate()
            .filter(|(_, entry)| match &entry.content {
                ClipboardContent::Text(t) => to_lowercase_clip(t).contains(&q),
                ClipboardContent::RichText { plain, .. } => to_lowercase_clip(plain).contains(&q),
                ClipboardContent::Url(u) => to_lowercase_clip(u).contains(&q),
                ClipboardContent::Files(files) => {
                    files.iter().any(|f| to_lowercase_clip(f).contains(&q))
                }
                _ => entry
                    .preview
                    .as_ref()
                    .map_or(false, |p| to_lowercase_clip(p).contains(&q)),
            })
            .collect()
    }

    pub fn history(&self) -> &[ClipboardEntry] {
        &self.history
    }

    pub fn history_by_type(&self, format: &ClipboardFormat) -> Vec<&ClipboardEntry> {
        self.history
            .iter()
            .filter(|e| &Self::content_format(&e.content) == format)
            .collect()
    }

    pub fn get_as_text(&self) -> Option<String> {
        let idx = self.current?;
        let entry = self.history.get(idx)?;
        match &entry.content {
            ClipboardContent::Text(t) => Some(t.clone()),
            ClipboardContent::RichText { plain, .. } => Some(plain.clone()),
            ClipboardContent::Url(u) => Some(u.clone()),
            ClipboardContent::Color(c) => Some(format!("#{:06X}", c & 0x00FF_FFFF)),
            ClipboardContent::Files(files) => {
                let mut out = String::new();
                for (i, f) in files.iter().enumerate() {
                    if i > 0 {
                        out.push('\n');
                    }
                    out.push_str(f);
                }
                Some(out)
            }
            _ => None,
        }
    }

    pub fn get_as_html(&self) -> Option<String> {
        let idx = self.current?;
        let entry = self.history.get(idx)?;
        match &entry.content {
            ClipboardContent::RichText { html, .. } => Some(html.clone()),
            ClipboardContent::Text(t) => Some(format!("<pre>{}</pre>", t)),
            ClipboardContent::Url(u) => Some(format!("<a href=\"{}\">{}</a>", u, u)),
            _ => None,
        }
    }

    pub fn available_formats(&self) -> Vec<ClipboardFormat> {
        match self.current {
            Some(idx) if idx < self.history.len() => {
                let entry = &self.history[idx];
                match &entry.content {
                    ClipboardContent::Text(_) => vec![ClipboardFormat::Text],
                    ClipboardContent::RichText { .. } => {
                        vec![ClipboardFormat::Text, ClipboardFormat::Html]
                    }
                    ClipboardContent::Image { .. } => vec![ClipboardFormat::Image],
                    ClipboardContent::Files(_) => {
                        vec![ClipboardFormat::Files, ClipboardFormat::Text]
                    }
                    ClipboardContent::Url(_) => vec![ClipboardFormat::Url, ClipboardFormat::Text],
                    ClipboardContent::Color(_) => vec![ClipboardFormat::Text],
                    ClipboardContent::Custom { ref mime_type, .. } => {
                        vec![ClipboardFormat::Custom(mime_type.clone())]
                    }
                }
            }
            _ => Vec::new(),
        }
    }

    pub fn register_listener(&mut self, listener_id: u64) {
        if !self.listeners.contains(&listener_id) {
            self.listeners.push(listener_id);
        }
    }

    pub fn unregister_listener(&mut self, listener_id: u64) {
        self.listeners.retain(|&id| id != listener_id);
    }

    // ── Internal helpers ─────────────────────────────────────────────────

    fn generate_preview(content: &ClipboardContent) -> Option<String> {
        const MAX_PREVIEW: usize = 120;

        match content {
            ClipboardContent::Text(t) => {
                if t.len() <= MAX_PREVIEW {
                    Some(t.clone())
                } else {
                    let mut preview = String::new();
                    for (i, ch) in t.chars().enumerate() {
                        if i >= MAX_PREVIEW {
                            break;
                        }
                        preview.push(ch);
                    }
                    preview.push_str("...");
                    Some(preview)
                }
            }
            ClipboardContent::RichText { plain, .. } => {
                if plain.len() <= MAX_PREVIEW {
                    Some(plain.clone())
                } else {
                    let mut preview = String::new();
                    for (i, ch) in plain.chars().enumerate() {
                        if i >= MAX_PREVIEW {
                            break;
                        }
                        preview.push(ch);
                    }
                    preview.push_str("...");
                    Some(preview)
                }
            }
            ClipboardContent::Image {
                width,
                height,
                format,
                ..
            } => {
                let fmt = match format {
                    ImageFormat::Png => "PNG",
                    ImageFormat::Jpeg => "JPEG",
                    ImageFormat::Bmp => "BMP",
                    ImageFormat::Svg => "SVG",
                };
                Some(format!("{} image {}x{}", fmt, width, height))
            }
            ClipboardContent::Files(files) => {
                if files.len() == 1 {
                    Some(files[0].clone())
                } else {
                    Some(format!("{} files", files.len()))
                }
            }
            ClipboardContent::Url(u) => {
                if u.len() <= MAX_PREVIEW {
                    Some(u.clone())
                } else {
                    Some(format!(
                        "{}...",
                        crate::text_util::truncate_chars(u, MAX_PREVIEW)
                    ))
                }
            }
            ClipboardContent::Color(c) => Some(format!("#{:06X}", c & 0x00FF_FFFF)),
            ClipboardContent::Custom { mime_type, data } => {
                Some(format!("{} ({} bytes)", mime_type, data.len()))
            }
        }
    }

    fn enforce_limits(&mut self) {
        while self.history.len() > self.max_history {
            if let Some(pos) = self.history.iter().rposition(|e| !e.pinned) {
                let size = self.history[pos].size;
                self.total_size = self.total_size.saturating_sub(size);
                self.history.remove(pos);
            } else {
                break;
            }
        }

        while self.total_size > self.max_total_size {
            if let Some(pos) = self.history.iter().rposition(|e| !e.pinned) {
                let size = self.history[pos].size;
                self.total_size = self.total_size.saturating_sub(size);
                self.history.remove(pos);
            } else {
                break;
            }
        }

        self.rebuild_pinned();
    }

    fn calculate_size(content: &ClipboardContent) -> usize {
        match content {
            ClipboardContent::Text(t) => t.len(),
            ClipboardContent::RichText { html, plain } => html.len() + plain.len(),
            ClipboardContent::Image { data, .. } => data.len() + 12,
            ClipboardContent::Files(files) => files.iter().map(|f| f.len()).sum(),
            ClipboardContent::Url(u) => u.len(),
            ClipboardContent::Color(_) => 4,
            ClipboardContent::Custom { data, mime_type } => data.len() + mime_type.len(),
        }
    }

    fn content_format(content: &ClipboardContent) -> ClipboardFormat {
        match content {
            ClipboardContent::Text(_) => ClipboardFormat::Text,
            ClipboardContent::RichText { .. } => ClipboardFormat::Html,
            ClipboardContent::Image { .. } => ClipboardFormat::Image,
            ClipboardContent::Files(_) => ClipboardFormat::Files,
            ClipboardContent::Url(_) => ClipboardFormat::Url,
            ClipboardContent::Color(_) => ClipboardFormat::Text,
            ClipboardContent::Custom { ref mime_type, .. } => {
                ClipboardFormat::Custom(mime_type.clone())
            }
        }
    }

    fn rebuild_pinned(&mut self) {
        self.pinned.clear();
        for (i, entry) in self.history.iter().enumerate() {
            if entry.pinned {
                self.pinned.push(i);
            }
        }
    }

    fn notify_listeners(&self) {
        // In a real implementation this would dispatch events to registered
        // listener IDs through the shell's event system.
    }
}

// ── Helper ───────────────────────────────────────────────────────────────

#[allow(dead_code)] // used by ClipboardManager::search (the service-API surface)
fn to_lowercase_clip(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c >= 'A' && c <= 'Z' {
                (c as u8 + 32) as char
            } else {
                c
            }
        })
        .collect()
}
