#![allow(dead_code)]

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

// ═══════════════════════════════════════════════════════════════════════════
// File Dialog System — Full open/save/browse dialog for RaeenOS
// ═══════════════════════════════════════════════════════════════════════════

static DIALOG_INITIALIZED: AtomicBool = AtomicBool::new(false);
static NEXT_DIALOG_ID: AtomicU64 = AtomicU64::new(1);

// ── Dialog types ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialogType {
    Open,
    Save,
    SaveAs,
    SelectFolder,
    OpenMultiple,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialogResult {
    Ok,
    Cancel,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewMode {
    List,
    Details,
    Tiles,
    LargeIcons,
    SmallIcons,
    Content,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortColumn {
    Name,
    DateModified,
    Type,
    Size,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortDirection {
    Ascending,
    Descending,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavigationAction {
    Back,
    Forward,
    Up,
    Refresh,
    Home,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchScope {
    CurrentFolder,
    Subfolders,
    AllIndexed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuickAccessCategory {
    Desktop,
    Documents,
    Downloads,
    Pictures,
    Music,
    Videos,
    Recent,
    Favorites,
    ThisPC,
    Network,
    RemovableDrives,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviewType {
    None,
    Image,
    Text,
    Pdf,
    Audio,
    Video,
    Binary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DragDropAction {
    DragFromDialog,
    DropPathToAddressBar,
    DropFilesToFolder,
}

// ── File filter ──────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct FileFilter {
    pub display_name: String,
    pub extensions: Vec<String>,
}

impl FileFilter {
    pub fn new(name: &str, extensions: &[&str]) -> Self {
        Self {
            display_name: String::from(name),
            extensions: extensions.iter().map(|e| String::from(*e)).collect(),
        }
    }

    pub fn all_files() -> Self {
        Self {
            display_name: String::from("All Files"),
            extensions: vec![String::from("*")],
        }
    }

    pub fn images() -> Self {
        Self::new(
            "Images",
            &["png", "jpg", "jpeg", "gif", "bmp", "webp", "svg", "tiff"],
        )
    }

    pub fn documents() -> Self {
        Self::new(
            "Documents",
            &["pdf", "doc", "docx", "txt", "rtf", "odt", "md"],
        )
    }

    pub fn audio() -> Self {
        Self::new("Audio", &["mp3", "wav", "flac", "ogg", "aac", "wma", "m4a"])
    }

    pub fn video() -> Self {
        Self::new("Video", &["mp4", "mkv", "avi", "mov", "wmv", "webm", "flv"])
    }

    pub fn matches(&self, filename: &str) -> bool {
        if self.extensions.iter().any(|e| e == "*") {
            return true;
        }
        let ext = filename.rsplit('.').next().unwrap_or("");
        self.extensions.iter().any(|e| e.as_str() == ext)
    }
}

// ── File entry (in file list) ────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct FileEntry {
    pub name: String,
    pub path: String,
    pub is_directory: bool,
    pub size_bytes: u64,
    pub date_modified: u64,
    pub date_created: u64,
    pub date_accessed: u64,
    pub file_type: String,
    pub is_hidden: bool,
    pub is_readonly: bool,
    pub is_system: bool,
    pub icon_char: char,
    pub selected: bool,
}

impl FileEntry {
    pub fn new_file(name: &str, path: &str, size: u64, modified: u64) -> Self {
        let ext = name.rsplit('.').next().unwrap_or("");
        let icon = match ext {
            "txt" | "md" | "log" => '\u{1F4C4}',
            "png" | "jpg" | "gif" | "bmp" => '\u{1F5BC}',
            "mp3" | "wav" | "flac" => '\u{1F3B5}',
            "mp4" | "mkv" | "avi" => '\u{1F3AC}',
            "pdf" => '\u{1F4D1}',
            "zip" | "tar" | "gz" => '\u{1F4E6}',
            "exe" | "elf" => '\u{2699}',
            _ => '\u{1F4C4}',
        };
        Self {
            name: String::from(name),
            path: String::from(path),
            is_directory: false,
            size_bytes: size,
            date_modified: modified,
            date_created: modified,
            date_accessed: modified,
            file_type: String::from(ext),
            is_hidden: name.starts_with('.'),
            is_readonly: false,
            is_system: false,
            icon_char: icon,
            selected: false,
        }
    }

    pub fn new_directory(name: &str, path: &str, modified: u64) -> Self {
        Self {
            name: String::from(name),
            path: String::from(path),
            is_directory: true,
            size_bytes: 0,
            date_modified: modified,
            date_created: modified,
            date_accessed: modified,
            file_type: String::from("Folder"),
            is_hidden: name.starts_with('.'),
            is_readonly: false,
            is_system: false,
            icon_char: '\u{1F4C1}',
            selected: false,
        }
    }

    pub fn formatted_size(&self) -> String {
        if self.is_directory {
            return String::from("");
        }
        if self.size_bytes < 1024 {
            let mut s = String::new();
            format_u64_into(&mut s, self.size_bytes);
            s.push_str(" B");
            s
        } else if self.size_bytes < 1024 * 1024 {
            let mut s = String::new();
            format_u64_into(&mut s, self.size_bytes / 1024);
            s.push_str(" KB");
            s
        } else if self.size_bytes < 1024 * 1024 * 1024 {
            let mut s = String::new();
            format_u64_into(&mut s, self.size_bytes / (1024 * 1024));
            s.push_str(" MB");
            s
        } else {
            let mut s = String::new();
            format_u64_into(&mut s, self.size_bytes / (1024 * 1024 * 1024));
            s.push_str(" GB");
            s
        }
    }
}

fn format_u64_into(s: &mut String, mut n: u64) {
    if n == 0 {
        s.push('0');
        return;
    }
    let mut buf = [0u8; 20];
    let mut pos = 20;
    while n > 0 {
        pos -= 1;
        buf[pos] = b'0' + (n % 10) as u8;
        n /= 10;
    }
    for &b in &buf[pos..20] {
        s.push(b as char);
    }
}

// ── Directory tree node (lazy-loading) ───────────────────────────────────

#[derive(Debug, Clone)]
pub struct DirectoryTreeNode {
    pub name: String,
    pub path: String,
    pub expanded: bool,
    pub loaded: bool,
    pub children: Vec<DirectoryTreeNode>,
    pub depth: u32,
}

impl DirectoryTreeNode {
    pub fn new(name: &str, path: &str, depth: u32) -> Self {
        Self {
            name: String::from(name),
            path: String::from(path),
            expanded: false,
            loaded: false,
            children: Vec::new(),
            depth,
        }
    }

    pub fn toggle_expand(&mut self) {
        self.expanded = !self.expanded;
    }

    pub fn add_child(&mut self, name: &str, path: &str) {
        self.children
            .push(DirectoryTreeNode::new(name, path, self.depth + 1));
    }

    pub fn mark_loaded(&mut self) {
        self.loaded = true;
    }

    pub fn visible_nodes(&self) -> Vec<&DirectoryTreeNode> {
        let mut result = Vec::new();
        self.collect_visible(&mut result);
        result
    }

    fn collect_visible<'a>(&'a self, out: &mut Vec<&'a DirectoryTreeNode>) {
        out.push(self);
        if self.expanded {
            for child in &self.children {
                child.collect_visible(out);
            }
        }
    }
}

// ── Breadcrumb path bar ──────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct BreadcrumbSegment {
    pub label: String,
    pub path: String,
    pub has_siblings: bool,
}

#[derive(Debug, Clone)]
pub struct BreadcrumbBar {
    pub segments: Vec<BreadcrumbSegment>,
    pub editing: bool,
    pub text_input: String,
}

impl BreadcrumbBar {
    pub fn new() -> Self {
        Self {
            segments: Vec::new(),
            editing: false,
            text_input: String::new(),
        }
    }

    pub fn set_path(&mut self, path: &str) {
        self.segments.clear();
        let mut accumulated = String::new();
        for part in path.split('/').filter(|p| !p.is_empty()) {
            if !accumulated.is_empty() {
                accumulated.push('/');
            }
            accumulated.push_str(part);
            self.segments.push(BreadcrumbSegment {
                label: String::from(part),
                path: accumulated.clone(),
                has_siblings: true,
            });
        }
        self.text_input = String::from(path);
    }

    pub fn enter_edit_mode(&mut self) {
        self.editing = true;
    }

    pub fn exit_edit_mode(&mut self) {
        self.editing = false;
    }

    pub fn current_path(&self) -> &str {
        if let Some(last) = self.segments.last() {
            &last.path
        } else {
            "/"
        }
    }

    pub fn navigate_to_segment(&mut self, index: usize) -> Option<String> {
        if index < self.segments.len() {
            let path = self.segments[index].path.clone();
            self.segments.truncate(index + 1);
            Some(path)
        } else {
            None
        }
    }
}

// ── Navigation history ───────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct NavigationHistory {
    pub back_stack: Vec<String>,
    pub forward_stack: Vec<String>,
    pub current: String,
}

impl NavigationHistory {
    pub fn new(initial: &str) -> Self {
        Self {
            back_stack: Vec::new(),
            forward_stack: Vec::new(),
            current: String::from(initial),
        }
    }

    pub fn navigate(&mut self, path: &str) {
        self.back_stack.push(self.current.clone());
        self.current = String::from(path);
        self.forward_stack.clear();
    }

    pub fn go_back(&mut self) -> Option<&str> {
        if let Some(prev) = self.back_stack.pop() {
            self.forward_stack
                .push(core::mem::replace(&mut self.current, prev));
            Some(&self.current)
        } else {
            None
        }
    }

    pub fn go_forward(&mut self) -> Option<&str> {
        if let Some(next) = self.forward_stack.pop() {
            self.back_stack
                .push(core::mem::replace(&mut self.current, next));
            Some(&self.current)
        } else {
            None
        }
    }

    pub fn go_up(&mut self) -> Option<&str> {
        let parent = if let Some(pos) = self.current.rfind('/') {
            if pos == 0 {
                String::from("/")
            } else {
                String::from(&self.current[..pos])
            }
        } else {
            return None;
        };
        self.back_stack
            .push(core::mem::replace(&mut self.current, parent));
        self.forward_stack.clear();
        Some(&self.current)
    }

    pub fn can_go_back(&self) -> bool {
        !self.back_stack.is_empty()
    }

    pub fn can_go_forward(&self) -> bool {
        !self.forward_stack.is_empty()
    }
}

// ── Quick access / sidebar ───────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct QuickAccessItem {
    pub label: String,
    pub path: String,
    pub category: QuickAccessCategory,
    pub pinned: bool,
    pub icon_char: char,
    pub access_count: u32,
    pub last_accessed: u64,
}

#[derive(Debug, Clone)]
pub struct Sidebar {
    pub quick_access: Vec<QuickAccessItem>,
    pub this_pc_drives: Vec<DriveInfo>,
    pub network_locations: Vec<NetworkLocation>,
    pub show_favorites: bool,
    pub show_this_pc: bool,
    pub show_network: bool,
    pub width: u32,
}

#[derive(Debug, Clone)]
pub struct DriveInfo {
    pub label: String,
    pub mount_point: String,
    pub total_bytes: u64,
    pub free_bytes: u64,
    pub drive_type: DriveType,
    pub icon_char: char,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriveType {
    Internal,
    External,
    USB,
    Network,
    Optical,
    Ram,
}

#[derive(Debug, Clone)]
pub struct NetworkLocation {
    pub name: String,
    pub address: String,
    pub connected: bool,
}

impl Sidebar {
    pub fn new() -> Self {
        let mut items = Vec::new();
        items.push(QuickAccessItem {
            label: String::from("Desktop"),
            path: String::from("/home/user/Desktop"),
            category: QuickAccessCategory::Desktop,
            pinned: true,
            icon_char: '\u{1F5A5}',
            access_count: 0,
            last_accessed: 0,
        });
        items.push(QuickAccessItem {
            label: String::from("Documents"),
            path: String::from("/home/user/Documents"),
            category: QuickAccessCategory::Documents,
            pinned: true,
            icon_char: '\u{1F4C1}',
            access_count: 0,
            last_accessed: 0,
        });
        items.push(QuickAccessItem {
            label: String::from("Downloads"),
            path: String::from("/home/user/Downloads"),
            category: QuickAccessCategory::Downloads,
            pinned: true,
            icon_char: '\u{2B07}',
            access_count: 0,
            last_accessed: 0,
        });
        items.push(QuickAccessItem {
            label: String::from("Pictures"),
            path: String::from("/home/user/Pictures"),
            category: QuickAccessCategory::Pictures,
            pinned: true,
            icon_char: '\u{1F5BC}',
            access_count: 0,
            last_accessed: 0,
        });
        items.push(QuickAccessItem {
            label: String::from("Music"),
            path: String::from("/home/user/Music"),
            category: QuickAccessCategory::Music,
            pinned: true,
            icon_char: '\u{1F3B5}',
            access_count: 0,
            last_accessed: 0,
        });
        items.push(QuickAccessItem {
            label: String::from("Videos"),
            path: String::from("/home/user/Videos"),
            category: QuickAccessCategory::Videos,
            pinned: true,
            icon_char: '\u{1F3AC}',
            access_count: 0,
            last_accessed: 0,
        });

        Self {
            quick_access: items,
            this_pc_drives: Vec::new(),
            network_locations: Vec::new(),
            show_favorites: true,
            show_this_pc: true,
            show_network: false,
            width: 200,
        }
    }

    pub fn add_pin(&mut self, label: &str, path: &str, category: QuickAccessCategory) {
        self.quick_access.push(QuickAccessItem {
            label: String::from(label),
            path: String::from(path),
            category,
            pinned: true,
            icon_char: '\u{1F4CC}',
            access_count: 0,
            last_accessed: 0,
        });
    }

    pub fn remove_pin(&mut self, path: &str) {
        self.quick_access
            .retain(|item| item.path != path || !item.pinned);
    }

    pub fn reorder_pin(&mut self, from_index: usize, to_index: usize) {
        if from_index < self.quick_access.len() && to_index < self.quick_access.len() {
            let item = self.quick_access.remove(from_index);
            self.quick_access.insert(to_index, item);
        }
    }

    pub fn record_access(&mut self, path: &str, timestamp: u64) {
        if let Some(item) = self.quick_access.iter_mut().find(|i| i.path == path) {
            item.access_count += 1;
            item.last_accessed = timestamp;
        }
    }

    pub fn frequent_folders(&self) -> Vec<&QuickAccessItem> {
        let mut sorted: Vec<&QuickAccessItem> = self
            .quick_access
            .iter()
            .filter(|i| i.access_count > 0)
            .collect();
        sorted.sort_by(|a, b| b.access_count.cmp(&a.access_count));
        sorted.truncate(10);
        sorted
    }

    pub fn add_drive(&mut self, label: &str, mount: &str, total: u64, free: u64, dtype: DriveType) {
        self.this_pc_drives.push(DriveInfo {
            label: String::from(label),
            mount_point: String::from(mount),
            total_bytes: total,
            free_bytes: free,
            drive_type: dtype,
            icon_char: match dtype {
                DriveType::Internal => '\u{1F4BD}',
                DriveType::External => '\u{1F4BE}',
                DriveType::USB => '\u{1F50C}',
                DriveType::Network => '\u{1F310}',
                DriveType::Optical => '\u{1F4BF}',
                DriveType::Ram => '\u{1F4A1}',
            },
        });
    }
}

// ── Preview pane ─────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct PreviewInfo {
    pub preview_type: PreviewType,
    pub file_name: String,
    pub file_size: u64,
    pub mime_type: String,
    pub dimensions: Option<(u32, u32)>,
    pub duration_secs: Option<u32>,
    pub text_preview: Option<String>,
    pub thumbnail_data: Option<Vec<u8>>,
}

impl PreviewInfo {
    pub fn from_file(entry: &FileEntry) -> Self {
        let ext = entry.name.rsplit('.').next().unwrap_or("");
        let ptype = match ext {
            "png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp" => PreviewType::Image,
            "txt" | "md" | "log" | "rs" | "c" | "h" | "py" | "js" => PreviewType::Text,
            "pdf" => PreviewType::Pdf,
            "mp3" | "wav" | "flac" | "ogg" => PreviewType::Audio,
            "mp4" | "mkv" | "avi" | "webm" => PreviewType::Video,
            _ => PreviewType::None,
        };
        Self {
            preview_type: ptype,
            file_name: entry.name.clone(),
            file_size: entry.size_bytes,
            mime_type: String::from(ext),
            dimensions: None,
            duration_secs: None,
            text_preview: None,
            thumbnail_data: None,
        }
    }
}

// ── Search state ─────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct DialogSearch {
    pub query: String,
    pub scope: SearchScope,
    pub search_content: bool,
    pub active: bool,
    pub results: Vec<FileEntry>,
    pub result_count: usize,
    pub searching: bool,
}

impl DialogSearch {
    pub fn new() -> Self {
        Self {
            query: String::new(),
            scope: SearchScope::CurrentFolder,
            search_content: false,
            active: false,
            results: Vec::new(),
            result_count: 0,
            searching: false,
        }
    }

    pub fn start_search(&mut self, query: &str, scope: SearchScope) {
        self.query = String::from(query);
        self.scope = scope;
        self.active = true;
        self.searching = true;
        self.results.clear();
        self.result_count = 0;
    }

    pub fn add_result(&mut self, entry: FileEntry) {
        self.result_count += 1;
        self.results.push(entry);
    }

    pub fn finish_search(&mut self) {
        self.searching = false;
    }

    pub fn clear(&mut self) {
        self.query.clear();
        self.active = false;
        self.searching = false;
        self.results.clear();
        self.result_count = 0;
    }
}

// ── File properties ──────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct FileProperties {
    pub name: String,
    pub path: String,
    pub size_bytes: u64,
    pub date_created: u64,
    pub date_modified: u64,
    pub date_accessed: u64,
    pub is_readonly: bool,
    pub is_hidden: bool,
    pub is_system: bool,
    pub owner: String,
    pub group: String,
    pub permissions: u32,
    pub link_target: Option<String>,
}

impl FileProperties {
    pub fn from_entry(entry: &FileEntry) -> Self {
        Self {
            name: entry.name.clone(),
            path: entry.path.clone(),
            size_bytes: entry.size_bytes,
            date_created: entry.date_created,
            date_modified: entry.date_modified,
            date_accessed: entry.date_accessed,
            is_readonly: entry.is_readonly,
            is_hidden: entry.is_hidden,
            is_system: entry.is_system,
            owner: String::from("user"),
            group: String::from("users"),
            permissions: 0o644,
            link_target: None,
        }
    }
}

// ── Bookmark management ──────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Bookmark {
    pub label: String,
    pub path: String,
    pub icon_char: char,
    pub order: u32,
}

#[derive(Debug, Clone)]
pub struct BookmarkManager {
    pub bookmarks: Vec<Bookmark>,
    next_order: u32,
}

impl BookmarkManager {
    pub fn new() -> Self {
        Self {
            bookmarks: Vec::new(),
            next_order: 0,
        }
    }

    pub fn add(&mut self, label: &str, path: &str) {
        if self.bookmarks.iter().any(|b| b.path == path) {
            return;
        }
        self.bookmarks.push(Bookmark {
            label: String::from(label),
            path: String::from(path),
            icon_char: '\u{2B50}',
            order: self.next_order,
        });
        self.next_order += 1;
    }

    pub fn remove(&mut self, path: &str) {
        self.bookmarks.retain(|b| b.path != path);
    }

    pub fn reorder(&mut self, from: usize, to: usize) {
        if from < self.bookmarks.len() && to < self.bookmarks.len() {
            let item = self.bookmarks.remove(from);
            self.bookmarks.insert(to, item);
            for (i, b) in self.bookmarks.iter_mut().enumerate() {
                b.order = i as u32;
            }
        }
    }

    pub fn rename(&mut self, path: &str, new_label: &str) {
        if let Some(b) = self.bookmarks.iter_mut().find(|b| b.path == path) {
            b.label = String::from(new_label);
        }
    }

    pub fn is_bookmarked(&self, path: &str) -> bool {
        self.bookmarks.iter().any(|b| b.path == path)
    }
}

// ── Recent files ─────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct RecentFile {
    pub path: String,
    pub name: String,
    pub timestamp: u64,
}

#[derive(Debug, Clone)]
pub struct RecentFilesList {
    pub entries: Vec<RecentFile>,
    pub max_entries: usize,
}

impl RecentFilesList {
    pub fn new(max: usize) -> Self {
        Self {
            entries: Vec::new(),
            max_entries: max,
        }
    }

    pub fn add(&mut self, path: &str, name: &str, timestamp: u64) {
        self.entries.retain(|e| e.path != path);
        self.entries.insert(
            0,
            RecentFile {
                path: String::from(path),
                name: String::from(name),
                timestamp,
            },
        );
        if self.entries.len() > self.max_entries {
            self.entries.truncate(self.max_entries);
        }
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    pub fn remove(&mut self, path: &str) {
        self.entries.retain(|e| e.path != path);
    }
}

// ── Keyboard shortcuts ───────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialogShortcut {
    AddressBar,
    Rename,
    Delete,
    Refresh,
    ParentFolder,
    NewFolder,
    SelectAll,
    Copy,
    Cut,
    Paste,
    Search,
    TogglePreview,
    ToggleHidden,
    ViewList,
    ViewDetails,
    ViewTiles,
    ViewLargeIcons,
}

impl DialogShortcut {
    pub fn from_key(ctrl: bool, alt: bool, _shift: bool, key: u8) -> Option<Self> {
        match (ctrl, alt, key) {
            (true, false, b'L') => Some(Self::AddressBar),
            (false, false, 0xF2) => Some(Self::Rename), // F2
            (false, false, 0x7F) => Some(Self::Delete), // Delete
            (false, false, 0xF5) => Some(Self::Refresh), // F5
            (false, true, 0x26) => Some(Self::ParentFolder), // Alt+Up
            (true, false, b'N') => Some(Self::NewFolder),
            (true, false, b'A') => Some(Self::SelectAll),
            (true, false, b'C') => Some(Self::Copy),
            (true, false, b'X') => Some(Self::Cut),
            (true, false, b'V') => Some(Self::Paste),
            (true, false, b'F') => Some(Self::Search),
            (true, false, b'P') => Some(Self::TogglePreview),
            (true, false, b'H') => Some(Self::ToggleHidden),
            _ => None,
        }
    }
}

// ── File operations ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileOperation {
    Copy,
    Move,
    Delete,
    Rename,
    CreateFolder,
}

#[derive(Debug, Clone)]
pub struct FileOperationRequest {
    pub operation: FileOperation,
    pub source_paths: Vec<String>,
    pub destination: Option<String>,
    pub new_name: Option<String>,
    pub confirmed: bool,
}

impl FileOperationRequest {
    pub fn copy(paths: &[&str], dest: &str) -> Self {
        Self {
            operation: FileOperation::Copy,
            source_paths: paths.iter().map(|p| String::from(*p)).collect(),
            destination: Some(String::from(dest)),
            new_name: None,
            confirmed: false,
        }
    }

    pub fn move_files(paths: &[&str], dest: &str) -> Self {
        Self {
            operation: FileOperation::Move,
            source_paths: paths.iter().map(|p| String::from(*p)).collect(),
            destination: Some(String::from(dest)),
            new_name: None,
            confirmed: false,
        }
    }

    pub fn delete(paths: &[&str]) -> Self {
        Self {
            operation: FileOperation::Delete,
            source_paths: paths.iter().map(|p| String::from(*p)).collect(),
            destination: None,
            new_name: None,
            confirmed: false,
        }
    }

    pub fn rename(path: &str, new_name: &str) -> Self {
        Self {
            operation: FileOperation::Rename,
            source_paths: vec![String::from(path)],
            destination: None,
            new_name: Some(String::from(new_name)),
            confirmed: false,
        }
    }

    pub fn create_folder(parent: &str, name: &str) -> Self {
        Self {
            operation: FileOperation::CreateFolder,
            source_paths: vec![String::from(parent)],
            destination: None,
            new_name: Some(String::from(name)),
            confirmed: false,
        }
    }
}

// ── Main FileDialog struct ───────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct FileDialog {
    pub id: u64,
    pub dialog_type: DialogType,
    pub title: String,
    pub initial_directory: String,
    pub current_directory: String,
    pub filters: Vec<FileFilter>,
    pub active_filter_index: usize,
    pub default_extension: String,
    pub show_hidden: bool,
    pub allow_multi_select: bool,
    pub start_in_recent: bool,
    pub overwrite_prompt: bool,
    pub view_mode: ViewMode,
    pub sort_column: SortColumn,
    pub sort_direction: SortDirection,
    pub entries: Vec<FileEntry>,
    pub selected_paths: Vec<String>,
    pub filename_input: String,
    pub navigation: NavigationHistory,
    pub breadcrumb: BreadcrumbBar,
    pub sidebar: Sidebar,
    pub search: DialogSearch,
    pub bookmarks: BookmarkManager,
    pub recent_files: RecentFilesList,
    pub directory_tree: Option<DirectoryTreeNode>,
    pub preview: Option<PreviewInfo>,
    pub show_preview_pane: bool,
    pub creating_new_folder: bool,
    pub new_folder_name: String,
    pub renaming_entry: Option<usize>,
    pub rename_text: String,
    pub pending_operation: Option<FileOperationRequest>,
    pub result: Option<DialogResult>,
    pub visible: bool,
}

impl FileDialog {
    pub fn new(dialog_type: DialogType) -> Self {
        let id = NEXT_DIALOG_ID.fetch_add(1, Ordering::Relaxed);
        let title = match dialog_type {
            DialogType::Open => String::from("Open File"),
            DialogType::Save => String::from("Save File"),
            DialogType::SaveAs => String::from("Save As"),
            DialogType::SelectFolder => String::from("Select Folder"),
            DialogType::OpenMultiple => String::from("Open Files"),
        };
        let initial = String::from("/home/user");
        Self {
            id,
            dialog_type,
            title,
            initial_directory: initial.clone(),
            current_directory: initial.clone(),
            filters: vec![FileFilter::all_files()],
            active_filter_index: 0,
            default_extension: String::new(),
            show_hidden: false,
            allow_multi_select: matches!(dialog_type, DialogType::OpenMultiple),
            start_in_recent: false,
            overwrite_prompt: matches!(dialog_type, DialogType::Save | DialogType::SaveAs),
            view_mode: ViewMode::Details,
            sort_column: SortColumn::Name,
            sort_direction: SortDirection::Ascending,
            entries: Vec::new(),
            selected_paths: Vec::new(),
            filename_input: String::new(),
            navigation: NavigationHistory::new(&initial),
            breadcrumb: BreadcrumbBar::new(),
            sidebar: Sidebar::new(),
            search: DialogSearch::new(),
            bookmarks: BookmarkManager::new(),
            recent_files: RecentFilesList::new(50),
            directory_tree: None,
            preview: None,
            show_preview_pane: true,
            creating_new_folder: false,
            new_folder_name: String::new(),
            renaming_entry: None,
            rename_text: String::new(),
            pending_operation: None,
            result: None,
            visible: false,
        }
    }

    pub fn with_title(mut self, title: &str) -> Self {
        self.title = String::from(title);
        self
    }

    pub fn with_initial_directory(mut self, dir: &str) -> Self {
        self.initial_directory = String::from(dir);
        self.current_directory = String::from(dir);
        self.navigation = NavigationHistory::new(dir);
        self.breadcrumb.set_path(dir);
        self
    }

    pub fn with_filters(mut self, filters: Vec<FileFilter>) -> Self {
        self.filters = filters;
        self
    }

    pub fn with_default_extension(mut self, ext: &str) -> Self {
        self.default_extension = String::from(ext);
        self
    }

    pub fn show(&mut self) {
        self.visible = true;
        self.result = None;
        self.breadcrumb.set_path(&self.current_directory.clone());
    }

    pub fn hide(&mut self) {
        self.visible = false;
    }

    pub fn navigate_to(&mut self, path: &str) {
        self.navigation.navigate(path);
        self.current_directory = String::from(path);
        self.breadcrumb.set_path(path);
        self.entries.clear();
        self.search.clear();
    }

    pub fn go_back(&mut self) -> bool {
        if let Some(_) = self.navigation.go_back() {
            self.current_directory = self.navigation.current.clone();
            self.breadcrumb.set_path(&self.current_directory.clone());
            self.entries.clear();
            true
        } else {
            false
        }
    }

    pub fn go_forward(&mut self) -> bool {
        if let Some(_) = self.navigation.go_forward() {
            self.current_directory = self.navigation.current.clone();
            self.breadcrumb.set_path(&self.current_directory.clone());
            self.entries.clear();
            true
        } else {
            false
        }
    }

    pub fn go_up(&mut self) -> bool {
        if let Some(_) = self.navigation.go_up() {
            self.current_directory = self.navigation.current.clone();
            self.breadcrumb.set_path(&self.current_directory.clone());
            self.entries.clear();
            true
        } else {
            false
        }
    }

    pub fn set_view_mode(&mut self, mode: ViewMode) {
        self.view_mode = mode;
    }

    pub fn set_sort(&mut self, column: SortColumn, direction: SortDirection) {
        self.sort_column = column;
        self.sort_direction = direction;
        self.sort_entries();
    }

    pub fn toggle_sort_column(&mut self, column: SortColumn) {
        if self.sort_column == column {
            self.sort_direction = match self.sort_direction {
                SortDirection::Ascending => SortDirection::Descending,
                SortDirection::Descending => SortDirection::Ascending,
            };
        } else {
            self.sort_column = column;
            self.sort_direction = SortDirection::Ascending;
        }
        self.sort_entries();
    }

    fn sort_entries(&mut self) {
        let dir = matches!(self.sort_direction, SortDirection::Ascending);
        self.entries.sort_by(|a, b| {
            if a.is_directory != b.is_directory {
                return b.is_directory.cmp(&a.is_directory);
            }
            let cmp = match self.sort_column {
                SortColumn::Name => a.name.cmp(&b.name),
                SortColumn::DateModified => a.date_modified.cmp(&b.date_modified),
                SortColumn::Type => a.file_type.cmp(&b.file_type),
                SortColumn::Size => a.size_bytes.cmp(&b.size_bytes),
            };
            if dir {
                cmp
            } else {
                cmp.reverse()
            }
        });
    }

    pub fn select_entry(&mut self, index: usize) {
        if index >= self.entries.len() {
            return;
        }
        if !self.allow_multi_select {
            for e in &mut self.entries {
                e.selected = false;
            }
            self.selected_paths.clear();
        }
        self.entries[index].selected = true;
        self.selected_paths.push(self.entries[index].path.clone());
        if !self.entries[index].is_directory {
            self.filename_input = self.entries[index].name.clone();
            self.preview = Some(PreviewInfo::from_file(&self.entries[index]));
        }
    }

    pub fn deselect_entry(&mut self, index: usize) {
        if index < self.entries.len() {
            self.entries[index].selected = false;
            let path = &self.entries[index].path;
            self.selected_paths.retain(|p| p != path);
        }
    }

    pub fn select_all(&mut self) {
        if !self.allow_multi_select {
            return;
        }
        self.selected_paths.clear();
        for entry in &mut self.entries {
            entry.selected = true;
            self.selected_paths.push(entry.path.clone());
        }
    }

    pub fn open_selected(&mut self) -> bool {
        if let Some(entry) = self.entries.iter().find(|e| e.selected) {
            if entry.is_directory {
                let path = entry.path.clone();
                self.navigate_to(&path);
                return false;
            }
        }
        true
    }

    pub fn confirm(&mut self) -> DialogResult {
        self.result = Some(DialogResult::Ok);
        self.visible = false;
        DialogResult::Ok
    }

    pub fn cancel(&mut self) -> DialogResult {
        self.result = Some(DialogResult::Cancel);
        self.visible = false;
        self.selected_paths.clear();
        DialogResult::Cancel
    }

    pub fn start_new_folder(&mut self) {
        self.creating_new_folder = true;
        self.new_folder_name = String::from("New Folder");
    }

    pub fn finish_new_folder(&mut self) -> Option<FileOperationRequest> {
        if self.creating_new_folder && !self.new_folder_name.is_empty() {
            self.creating_new_folder = false;
            let op =
                FileOperationRequest::create_folder(&self.current_directory, &self.new_folder_name);
            self.new_folder_name.clear();
            Some(op)
        } else {
            self.creating_new_folder = false;
            None
        }
    }

    pub fn start_rename(&mut self, index: usize) {
        if index < self.entries.len() {
            self.renaming_entry = Some(index);
            self.rename_text = self.entries[index].name.clone();
        }
    }

    pub fn finish_rename(&mut self) -> Option<FileOperationRequest> {
        if let Some(idx) = self.renaming_entry {
            if idx < self.entries.len() && !self.rename_text.is_empty() {
                let op = FileOperationRequest::rename(&self.entries[idx].path, &self.rename_text);
                self.renaming_entry = None;
                self.rename_text.clear();
                return Some(op);
            }
        }
        self.renaming_entry = None;
        None
    }

    pub fn delete_selected(&mut self) -> Option<FileOperationRequest> {
        let paths: Vec<String> = self
            .entries
            .iter()
            .filter(|e| e.selected)
            .map(|e| e.path.clone())
            .collect();
        if paths.is_empty() {
            return None;
        }
        let refs: Vec<&str> = paths.iter().map(|s| s.as_str()).collect();
        Some(FileOperationRequest::delete(&refs))
    }

    pub fn handle_shortcut(&mut self, shortcut: DialogShortcut) {
        match shortcut {
            DialogShortcut::AddressBar => self.breadcrumb.enter_edit_mode(),
            DialogShortcut::Refresh => {
                self.entries.clear();
            }
            DialogShortcut::ParentFolder => {
                self.go_up();
            }
            DialogShortcut::NewFolder => self.start_new_folder(),
            DialogShortcut::SelectAll => self.select_all(),
            DialogShortcut::TogglePreview => {
                self.show_preview_pane = !self.show_preview_pane;
            }
            DialogShortcut::ToggleHidden => {
                self.show_hidden = !self.show_hidden;
            }
            DialogShortcut::Search => {
                self.search.active = true;
            }
            DialogShortcut::ViewList => self.view_mode = ViewMode::List,
            DialogShortcut::ViewDetails => self.view_mode = ViewMode::Details,
            DialogShortcut::ViewTiles => self.view_mode = ViewMode::Tiles,
            DialogShortcut::ViewLargeIcons => self.view_mode = ViewMode::LargeIcons,
            _ => {}
        }
    }

    pub fn filtered_entries(&self) -> Vec<&FileEntry> {
        let filter = self.filters.get(self.active_filter_index);
        self.entries
            .iter()
            .filter(|e| {
                if !self.show_hidden && e.is_hidden {
                    return false;
                }
                if e.is_directory {
                    return true;
                }
                if self.dialog_type == DialogType::SelectFolder {
                    return false;
                }
                if let Some(f) = filter {
                    f.matches(&e.name)
                } else {
                    true
                }
            })
            .collect()
    }

    pub fn selected_file_path(&self) -> Option<String> {
        if self.filename_input.is_empty() {
            return None;
        }
        let mut path = self.current_directory.clone();
        path.push('/');
        path.push_str(&self.filename_input);
        if !self.default_extension.is_empty() && !self.filename_input.contains('.') {
            path.push('.');
            path.push_str(&self.default_extension);
        }
        Some(path)
    }
}

// ── Drag and drop ────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct DragDropState {
    pub active: bool,
    pub action: DragDropAction,
    pub dragged_paths: Vec<String>,
    pub drop_target: Option<String>,
    pub cursor_x: i32,
    pub cursor_y: i32,
}

impl DragDropState {
    pub fn new() -> Self {
        Self {
            active: false,
            action: DragDropAction::DragFromDialog,
            dragged_paths: Vec::new(),
            drop_target: None,
            cursor_x: 0,
            cursor_y: 0,
        }
    }

    pub fn start_drag(&mut self, paths: Vec<String>, x: i32, y: i32) {
        self.active = true;
        self.dragged_paths = paths;
        self.cursor_x = x;
        self.cursor_y = y;
    }

    pub fn update_position(&mut self, x: i32, y: i32) {
        self.cursor_x = x;
        self.cursor_y = y;
    }

    pub fn set_drop_target(&mut self, target: Option<String>) {
        self.drop_target = target;
    }

    pub fn finish_drop(&mut self) -> Option<(Vec<String>, String)> {
        if !self.active {
            return None;
        }
        self.active = false;
        if let Some(target) = self.drop_target.take() {
            let paths = core::mem::take(&mut self.dragged_paths);
            Some((paths, target))
        } else {
            self.dragged_paths.clear();
            None
        }
    }

    pub fn cancel(&mut self) {
        self.active = false;
        self.dragged_paths.clear();
        self.drop_target = None;
    }
}

// ── Global file dialog service ───────────────────────────────────────────

pub struct FileDialogService {
    pub active_dialogs: Vec<FileDialog>,
    pub max_recent: usize,
    pub global_bookmarks: BookmarkManager,
    pub global_recent: RecentFilesList,
    pub default_sidebar: Sidebar,
}

impl FileDialogService {
    pub fn new() -> Self {
        Self {
            active_dialogs: Vec::new(),
            max_recent: 50,
            global_bookmarks: BookmarkManager::new(),
            global_recent: RecentFilesList::new(50),
            default_sidebar: Sidebar::new(),
        }
    }

    pub fn open_dialog(&mut self, dialog_type: DialogType) -> u64 {
        let mut dialog = FileDialog::new(dialog_type);
        dialog.bookmarks = self.global_bookmarks.clone();
        dialog.recent_files = self.global_recent.clone();
        dialog.sidebar = self.default_sidebar.clone();
        dialog.show();
        let id = dialog.id;
        self.active_dialogs.push(dialog);
        id
    }

    pub fn close_dialog(&mut self, id: u64) {
        if let Some(pos) = self.active_dialogs.iter().position(|d| d.id == id) {
            let dialog = self.active_dialogs.remove(pos);
            self.global_bookmarks = dialog.bookmarks;
            self.global_recent = dialog.recent_files;
        }
    }

    pub fn get_dialog(&self, id: u64) -> Option<&FileDialog> {
        self.active_dialogs.iter().find(|d| d.id == id)
    }

    pub fn get_dialog_mut(&mut self, id: u64) -> Option<&mut FileDialog> {
        self.active_dialogs.iter_mut().find(|d| d.id == id)
    }

    pub fn active_count(&self) -> usize {
        self.active_dialogs.len()
    }
}

static mut FILE_DIALOG_SERVICE: Option<FileDialogService> = None;

pub fn init() {
    unsafe {
        if !DIALOG_INITIALIZED.swap(true, Ordering::SeqCst) {
            FILE_DIALOG_SERVICE = Some(FileDialogService::new());
        }
    }
}

pub fn service() -> &'static FileDialogService {
    unsafe {
        FILE_DIALOG_SERVICE
            .as_ref()
            .expect("file_dialog not initialized")
    }
}

pub fn service_mut() -> &'static mut FileDialogService {
    unsafe {
        FILE_DIALOG_SERVICE
            .as_mut()
            .expect("file_dialog not initialized")
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Convenience API — show_open_dialog / show_save_dialog
// ═══════════════════════════════════════════════════════════════════════════

/// Show an open-file dialog with the given filters. Returns the selected path
/// once the user confirms, or `None` if cancelled.
pub fn show_open_dialog(filters: &[FileFilter]) -> Option<String> {
    let mut dialog = FileDialog::new(DialogType::Open);
    if !filters.is_empty() {
        dialog.filters = filters.to_vec();
    }
    dialog.show();
    // In a real OS this would block until the user interacts; here we return
    // the dialog's state for the event loop to drive.
    dialog.selected_file_path()
}

/// Show a save-file dialog with a default filename and filters.
pub fn show_save_dialog(default_name: &str, filters: &[FileFilter]) -> Option<String> {
    let mut dialog = FileDialog::new(DialogType::Save);
    dialog.filename_input = String::from(default_name);
    if !filters.is_empty() {
        dialog.filters = filters.to_vec();
    }
    dialog.show();
    dialog.selected_file_path()
}

/// Show a folder-selection dialog.
pub fn show_folder_dialog() -> Option<String> {
    let mut dialog = FileDialog::new(DialogType::SelectFolder);
    dialog.show();
    Some(dialog.current_directory.clone())
}

// ═══════════════════════════════════════════════════════════════════════════
// Canvas rendering — full dialog UI
// ═══════════════════════════════════════════════════════════════════════════

const DIALOG_BG: u32 = 0xFF_14_16_22;
const DIALOG_TITLE_BG: u32 = 0xFF_0A_0E_1A;
const DIALOG_ACCENT: u32 = 0xFF_4E_9C_FF;
const DIALOG_FG: u32 = 0xFF_C0_C0_D0;
const DIALOG_DIM: u32 = 0xFF_70_70_80;
const DIALOG_HOVER: u32 = 0xFF_28_2C_44;
const DIALOG_SELECT: u32 = 0xFF_33_33_55;
const SIDEBAR_BG: u32 = 0xFF_10_12_1C;
const INPUT_BG: u32 = 0xFF_1A_1E_2E;
const BUTTON_BG: u32 = 0xFF_4E_9C_FF;
const BUTTON_FG: u32 = 0xFF_FF_FF_FF;
const FD_GLYPH_W: usize = 8;
const FD_GLYPH_H: usize = 8;
const TITLE_BAR_H: usize = 28;
const BREADCRUMB_H: usize = 28;
const SIDEBAR_W: usize = 160;
const FOOTER_H: usize = 64;
const ROW_H: usize = 24;
const COLUMN_HEADER_H: usize = 22;

impl FileDialog {
    /// Render the full file dialog into a Canvas at a given position and size.
    pub fn render(&self, canvas: &mut raegfx::Canvas, dx: usize, dy: usize, dw: usize, dh: usize) {
        if !self.visible {
            return;
        }

        // Background
        canvas.fill_rect(dx, dy, dw, dh, DIALOG_BG);
        canvas.draw_rect_outline(dx, dy, dw, dh, DIALOG_ACCENT);

        // Title bar
        canvas.fill_rect(dx, dy, dw, TITLE_BAR_H, DIALOG_TITLE_BG);
        canvas.draw_text(dx + 8, dy + 8, &self.title, DIALOG_FG, None);
        // Close button
        canvas.draw_text(dx + dw - 20, dy + 8, "x", DIALOG_DIM, None);

        let content_y = dy + TITLE_BAR_H;

        // Breadcrumb bar / path bar
        let bread_y = content_y;
        canvas.fill_rect(dx, bread_y, dw, BREADCRUMB_H, DIALOG_TITLE_BG);

        // Navigation buttons
        let nav_fg = |can: bool| if can { DIALOG_FG } else { DIALOG_DIM };
        canvas.draw_text(
            dx + 8,
            bread_y + 8,
            "<",
            nav_fg(self.navigation.can_go_back()),
            None,
        );
        canvas.draw_text(
            dx + 24,
            bread_y + 8,
            ">",
            nav_fg(self.navigation.can_go_forward()),
            None,
        );
        canvas.draw_text(dx + 40, bread_y + 8, "^", DIALOG_FG, None);

        // Breadcrumb segments
        if self.breadcrumb.editing {
            canvas.fill_rect(dx + 60, bread_y + 4, dw - 80, BREADCRUMB_H - 8, INPUT_BG);
            canvas.draw_rect_outline(
                dx + 60,
                bread_y + 4,
                dw - 80,
                BREADCRUMB_H - 8,
                DIALOG_ACCENT,
            );
            canvas.draw_text(
                dx + 64,
                bread_y + 8,
                &self.breadcrumb.text_input,
                DIALOG_FG,
                None,
            );
        } else {
            let mut bx = dx + 60;
            for seg in &self.breadcrumb.segments {
                canvas.draw_text(bx, bread_y + 8, &seg.label, DIALOG_ACCENT, None);
                bx += seg.label.len() * FD_GLYPH_W + 4;
                canvas.draw_text(bx, bread_y + 8, "/", DIALOG_DIM, None);
                bx += FD_GLYPH_W + 4;
            }
        }

        let body_y = bread_y + BREADCRUMB_H;
        let body_h = dh.saturating_sub(TITLE_BAR_H + BREADCRUMB_H + FOOTER_H);

        // Sidebar
        canvas.fill_rect(dx, body_y, SIDEBAR_W, body_h, SIDEBAR_BG);
        // Separator
        for sy in body_y..body_y + body_h {
            canvas.draw_pixel(dx + SIDEBAR_W, sy, DIALOG_DIM);
        }

        let mut sy = body_y + 8;
        for item in &self.sidebar.quick_access {
            let fg = if self.current_directory == item.path {
                DIALOG_ACCENT
            } else {
                DIALOG_FG
            };
            if self.current_directory == item.path {
                canvas.fill_rect(dx, sy - 2, SIDEBAR_W, ROW_H, DIALOG_SELECT);
            }
            canvas.draw_glyph(dx + 8, sy + 4, item.icon_char, fg, None);
            let max_label = (SIDEBAR_W - 28) / FD_GLYPH_W;
            let label = crate::text_util::truncate_chars(&item.label, max_label);
            canvas.draw_text(dx + 24, sy + 4, label, fg, None);
            sy += ROW_H;
        }

        // Drive list
        if !self.sidebar.this_pc_drives.is_empty() {
            sy += 8;
            canvas.draw_text(dx + 8, sy + 4, "Drives", DIALOG_DIM, None);
            sy += ROW_H;
            for drive in &self.sidebar.this_pc_drives {
                canvas.draw_glyph(dx + 8, sy + 4, drive.icon_char, DIALOG_FG, None);
                canvas.draw_text(dx + 24, sy + 4, &drive.label, DIALOG_FG, None);
                sy += ROW_H;
            }
        }

        // File list area
        let list_x = dx + SIDEBAR_W + 1;
        let preview_w = if self.show_preview_pane { 180usize } else { 0 };
        let list_w = dw.saturating_sub(SIDEBAR_W + 1 + preview_w);
        let list_y = body_y;

        // Column headers
        canvas.fill_rect(list_x, list_y, list_w, COLUMN_HEADER_H, DIALOG_TITLE_BG);
        let col_name_w = list_w / 2;
        let col_size_w = list_w / 6;
        let col_type_w = list_w / 6;
        let _col_date_w = list_w.saturating_sub(col_name_w + col_size_w + col_type_w);

        let sort_indicator = |col: SortColumn| -> &'static str {
            if self.sort_column == col {
                match self.sort_direction {
                    SortDirection::Ascending => " v",
                    SortDirection::Descending => " ^",
                }
            } else {
                ""
            }
        };

        canvas.draw_text(list_x + 4, list_y + 6, "Name", DIALOG_FG, None);
        let ni = sort_indicator(SortColumn::Name);
        if !ni.is_empty() {
            canvas.draw_text(list_x + 36, list_y + 6, ni, DIALOG_ACCENT, None);
        }

        canvas.draw_text(list_x + col_name_w + 4, list_y + 6, "Size", DIALOG_FG, None);
        canvas.draw_text(
            list_x + col_name_w + col_size_w + 4,
            list_y + 6,
            "Type",
            DIALOG_FG,
            None,
        );
        canvas.draw_text(
            list_x + col_name_w + col_size_w + col_type_w + 4,
            list_y + 6,
            "Modified",
            DIALOG_FG,
            None,
        );

        // File entries
        let entries = self.filtered_entries();
        let entry_start_y = list_y + COLUMN_HEADER_H;
        let max_visible = body_h.saturating_sub(COLUMN_HEADER_H) / ROW_H;

        for (i, entry) in entries.iter().take(max_visible).enumerate() {
            let ey = entry_start_y + i * ROW_H;
            if ey + ROW_H > list_y + body_h {
                break;
            }

            if entry.selected {
                canvas.fill_rect(list_x, ey, list_w, ROW_H, DIALOG_SELECT);
            }

            // Icon
            canvas.draw_glyph(list_x + 4, ey + 6, entry.icon_char, DIALOG_ACCENT, None);

            // Name (truncated)
            let max_name_chars = (col_name_w - 24) / FD_GLYPH_W;
            let display_name = crate::text_util::truncate_chars(&entry.name, max_name_chars);
            let name_fg = if entry.is_directory {
                DIALOG_ACCENT
            } else {
                DIALOG_FG
            };
            canvas.draw_text(list_x + 20, ey + 6, display_name, name_fg, None);

            // Size
            let size_str = entry.formatted_size();
            canvas.draw_text(list_x + col_name_w + 4, ey + 6, &size_str, DIALOG_DIM, None);

            // Type
            let type_str = if entry.is_directory {
                "Folder"
            } else {
                entry.file_type.as_str()
            };
            canvas.draw_text(
                list_x + col_name_w + col_size_w + 4,
                ey + 6,
                type_str,
                DIALOG_DIM,
                None,
            );
        }

        // Preview pane
        if self.show_preview_pane {
            let prev_x = dx + dw - preview_w;
            canvas.fill_rect(prev_x, body_y, preview_w, body_h, SIDEBAR_BG);
            for py in body_y..body_y + body_h {
                canvas.draw_pixel(prev_x, py, DIALOG_DIM);
            }

            if let Some(ref preview) = self.preview {
                canvas.draw_text(prev_x + 8, body_y + 8, &preview.file_name, DIALOG_FG, None);

                let type_label = match preview.preview_type {
                    PreviewType::Image => "Image",
                    PreviewType::Text => "Text",
                    PreviewType::Audio => "Audio",
                    PreviewType::Video => "Video",
                    PreviewType::Pdf => "PDF",
                    PreviewType::Binary => "Binary",
                    PreviewType::None => "File",
                };
                canvas.draw_text(prev_x + 8, body_y + 24, type_label, DIALOG_DIM, None);

                let mut size_str = String::new();
                format_u64_into(&mut size_str, preview.file_size / 1024);
                size_str.push_str(" KB");
                canvas.draw_text(prev_x + 8, body_y + 40, &size_str, DIALOG_DIM, None);

                if let Some((w, h)) = preview.dimensions {
                    let mut dim = String::new();
                    format_u64_into(&mut dim, w as u64);
                    dim.push_str(" x ");
                    format_u64_into(&mut dim, h as u64);
                    canvas.draw_text(prev_x + 8, body_y + 56, &dim, DIALOG_DIM, None);
                }
            } else {
                canvas.draw_text(prev_x + 8, body_y + 8, "No preview", DIALOG_DIM, None);
            }
        }

        // Footer — filename input + filter dropdown + OK/Cancel buttons
        let foot_y = dy + dh - FOOTER_H;
        canvas.fill_rect(dx, foot_y, dw, FOOTER_H, DIALOG_TITLE_BG);
        // Separator
        for fx in dx..dx + dw {
            canvas.draw_pixel(fx, foot_y, DIALOG_DIM);
        }

        // Filename label + input
        canvas.draw_text(dx + 8, foot_y + 10, "File name:", DIALOG_FG, None);
        let input_x = dx + 88;
        let input_w = dw.saturating_sub(280);
        canvas.fill_rect(input_x, foot_y + 6, input_w, 20, INPUT_BG);
        canvas.draw_rect_outline(input_x, foot_y + 6, input_w, 20, DIALOG_DIM);
        canvas.draw_text(
            input_x + 4,
            foot_y + 10,
            &self.filename_input,
            DIALOG_FG,
            None,
        );

        // Filter dropdown
        let filter_y = foot_y + 32;
        canvas.draw_text(dx + 8, filter_y + 4, "File type:", DIALOG_FG, None);
        canvas.fill_rect(input_x, filter_y, input_w, 20, INPUT_BG);
        canvas.draw_rect_outline(input_x, filter_y, input_w, 20, DIALOG_DIM);
        if let Some(filter) = self.filters.get(self.active_filter_index) {
            let filter_text = &filter.display_name;
            canvas.draw_text(input_x + 4, filter_y + 4, filter_text, DIALOG_FG, None);
        }

        // OK button
        let btn_w = 72usize;
        let btn_h = 24usize;
        let ok_x = dx + dw - btn_w - 84;
        let ok_label = match self.dialog_type {
            DialogType::Open | DialogType::OpenMultiple => "Open",
            DialogType::Save | DialogType::SaveAs => "Save",
            DialogType::SelectFolder => "Select",
        };
        canvas.fill_rect(ok_x, foot_y + 8, btn_w, btn_h, BUTTON_BG);
        let label_x = ok_x + (btn_w - ok_label.len() * FD_GLYPH_W) / 2;
        canvas.draw_text(label_x, foot_y + 16, ok_label, BUTTON_FG, None);

        // Cancel button
        let cancel_x = dx + dw - btn_w - 8;
        canvas.fill_rect(cancel_x, foot_y + 8, btn_w, btn_h, DIALOG_DIM);
        let cancel_lx = cancel_x + (btn_w - 6 * FD_GLYPH_W) / 2;
        canvas.draw_text(cancel_lx, foot_y + 16, "Cancel", BUTTON_FG, None);

        // Search overlay
        if self.search.active {
            let search_x = dx + SIDEBAR_W + 8;
            let search_w = list_w.saturating_sub(16);
            canvas.fill_rect(search_x, body_y, search_w, 28, INPUT_BG);
            canvas.draw_rect_outline(search_x, body_y, search_w, 28, DIALOG_ACCENT);
            let search_label = if self.search.query.is_empty() {
                "Search..."
            } else {
                &self.search.query
            };
            let sfg = if self.search.query.is_empty() {
                DIALOG_DIM
            } else {
                DIALOG_FG
            };
            canvas.draw_text(search_x + 8, body_y + 8, search_label, sfg, None);
        }

        // New folder input overlay
        if self.creating_new_folder {
            let nf_x = dx + dw / 4;
            let nf_w = dw / 2;
            let nf_y = dy + dh / 3;
            canvas.fill_rect(nf_x, nf_y, nf_w, 80, DIALOG_BG);
            canvas.draw_rect_outline(nf_x, nf_y, nf_w, 80, DIALOG_ACCENT);
            canvas.draw_text(nf_x + 8, nf_y + 8, "New Folder Name:", DIALOG_FG, None);
            canvas.fill_rect(nf_x + 8, nf_y + 28, nf_w - 16, 20, INPUT_BG);
            canvas.draw_rect_outline(nf_x + 8, nf_y + 28, nf_w - 16, 20, DIALOG_DIM);
            canvas.draw_text(nf_x + 12, nf_y + 32, &self.new_folder_name, DIALOG_FG, None);

            canvas.fill_rect(nf_x + nf_w - 160, nf_y + 54, 72, 20, BUTTON_BG);
            canvas.draw_text(nf_x + nf_w - 148, nf_y + 58, "Create", BUTTON_FG, None);
            canvas.fill_rect(nf_x + nf_w - 80, nf_y + 54, 72, 20, DIALOG_DIM);
            canvas.draw_text(nf_x + nf_w - 68, nf_y + 58, "Cancel", BUTTON_FG, None);
        }
    }
}

impl FileDialogService {
    /// Render all active dialogs.
    pub fn render_all(&self, canvas: &mut raegfx::Canvas) {
        let cw = canvas.width();
        let ch = canvas.height();
        let dialog_w = (cw * 3 / 4).max(400);
        let dialog_h = (ch * 3 / 4).max(300);

        for (i, dialog) in self.active_dialogs.iter().enumerate() {
            let offset = i * 24;
            let dx = (cw.saturating_sub(dialog_w)) / 2 + offset;
            let dy = (ch.saturating_sub(dialog_h)) / 2 + offset;
            dialog.render(canvas, dx, dy, dialog_w, dialog_h);
        }
    }
}
