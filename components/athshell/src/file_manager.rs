//! Full desktop file manager for AthenaOS.
//!
//! QUARANTINED / DEAD TWIN (CLAUDE.md rule 7, see docs/QUARANTINED_MODULES.md):
//! this is NOT the live Files app. The live, launched Files is the `apps/files`
//! ELF (`/usr/bin/raefiles`, taskbar pin + `exec "files"`), which renders via its
//! own `apps/files/src/main.rs`. This module is only `pub mod`-declared in
//! `athshell/src/lib.rs`; its `with_file_manager`/`FileManager` are referenced
//! nowhere outside this file and nothing calls `render`. The design re-skin
//! (docs/design/files.md) lands on the live `apps/files` crate, NOT here. Do not
//! re-skin this twin; if it is ever wired, retire `apps/files` first, don't run
//! both. Its private `FM_*` palette + 8x8 block glyphs are intentionally left as
//! the historical reference the spec was written against.
//!
//! Provides (model only, unrendered): icon/list/details/column/tree views, tabbed
//! navigation, dual-pane mode, file operations with progress tracking, batch
//! rename, search, preview, thumbnails, trash, bookmarks, network shares, volume
//! management, drag-and-drop, context menus, and plugin extension points.

#![allow(dead_code)]

extern crate alloc;

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};

// ── Colour palette ───────────────────────────────────────────────────────

const FM_BG: u32 = 0xFF_10_12_1E;
const FM_SIDEBAR_BG: u32 = 0xFF_0C_0E_18;
const FM_HEADER_BG: u32 = 0xFF_16_18_26;
const FM_SELECTED: u32 = 0xFF_28_2C_44;
const FM_HOVER: u32 = 0xFF_1E_20_30;
const FM_FG: u32 = 0xFF_C0_C0_D0;
const FM_DIM: u32 = 0xFF_70_70_80;
const FM_ACCENT: u32 = 0xFF_4E_9C_FF;
const FM_DIR_FG: u32 = 0xFF_4E_9C_FF;
const FM_EXEC_FG: u32 = 0xFF_44_DD_66;
const FM_SYMLINK_FG: u32 = 0xFF_BB_88_FF;
const FM_BORDER: u32 = 0xFF_33_33_55;
const FM_TAB_ACTIVE: u32 = 0xFF_22_24_38;
const FM_TAB_BG: u32 = 0xFF_14_16_22;
const FM_SEARCH_BG: u32 = 0xFF_1A_1C_2A;
const FM_DANGER: u32 = 0xFF_FF_44_44;
const FM_ARCHIVE_FG: u32 = 0xFF_FF_AA_33;
const FM_IMAGE_FG: u32 = 0xFF_FF_66_99;
const FM_VIDEO_FG: u32 = 0xFF_CC_66_FF;
const FM_AUDIO_FG: u32 = 0xFF_66_DD_CC;
const FM_DOC_FG: u32 = 0xFF_FF_CC_66;
const GLYPH_W: usize = 8;
const GLYPH_H: usize = 8;

// ── File entry types ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileEntryType {
    File,
    Directory,
    Symlink,
    Executable,
    Image,
    Video,
    Audio,
    Archive,
    Document,
    Code,
    Pdf,
    Font,
    Database,
    DeviceNode,
    Socket,
    Pipe,
}

impl FileEntryType {
    pub fn color(self) -> u32 {
        match self {
            Self::Directory => FM_DIR_FG,
            Self::Symlink => FM_SYMLINK_FG,
            Self::Executable => FM_EXEC_FG,
            Self::Archive => FM_ARCHIVE_FG,
            Self::Image => FM_IMAGE_FG,
            Self::Video => FM_VIDEO_FG,
            Self::Audio => FM_AUDIO_FG,
            Self::Document | Self::Pdf => FM_DOC_FG,
            Self::Code => FM_ACCENT,
            _ => FM_FG,
        }
    }

    pub fn icon_char(self) -> char {
        match self {
            Self::Directory => '\u{1F4C1}',
            Self::Symlink => '\u{1F517}',
            Self::Executable => '\u{2699}',
            Self::Archive => '\u{1F4E6}',
            Self::Image => '\u{1F5BC}',
            Self::Video => '\u{1F3AC}',
            Self::Audio => '\u{1F3B5}',
            Self::Document => '\u{1F4C4}',
            Self::Pdf => '\u{1F4D1}',
            Self::Code => '\u{1F4BB}',
            Self::Font => 'A',
            Self::Database => '\u{1F5C3}',
            Self::DeviceNode => 'D',
            Self::Socket => 'S',
            Self::Pipe => 'P',
            Self::File => '\u{1F4C4}',
        }
    }

    pub fn from_extension(ext: &str) -> Self {
        match ext {
            "png" | "jpg" | "jpeg" | "gif" | "bmp" | "svg" | "webp" | "ico" | "tiff" => Self::Image,
            "mp4" | "mkv" | "avi" | "mov" | "wmv" | "flv" | "webm" => Self::Video,
            "mp3" | "flac" | "wav" | "ogg" | "aac" | "wma" | "m4a" | "opus" => Self::Audio,
            "zip" | "tar" | "gz" | "bz2" | "xz" | "7z" | "rar" | "zst" => Self::Archive,
            "pdf" => Self::Pdf,
            "doc" | "docx" | "odt" | "rtf" | "txt" | "md" => Self::Document,
            "rs" | "c" | "cpp" | "h" | "py" | "js" | "ts" | "go" | "java" | "rb" | "sh"
            | "toml" | "yaml" | "json" | "xml" | "html" | "css" => Self::Code,
            "ttf" | "otf" | "woff" | "woff2" => Self::Font,
            "db" | "sqlite" | "sqlite3" => Self::Database,
            _ => Self::File,
        }
    }
}

// ── Permissions ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Permissions {
    pub bits: u16,
}

impl Permissions {
    pub const fn new(bits: u16) -> Self {
        Self { bits }
    }

    pub fn owner_read(self) -> bool {
        self.bits & 0o400 != 0
    }
    pub fn owner_write(self) -> bool {
        self.bits & 0o200 != 0
    }
    pub fn owner_exec(self) -> bool {
        self.bits & 0o100 != 0
    }
    pub fn group_read(self) -> bool {
        self.bits & 0o040 != 0
    }
    pub fn group_write(self) -> bool {
        self.bits & 0o020 != 0
    }
    pub fn group_exec(self) -> bool {
        self.bits & 0o010 != 0
    }
    pub fn other_read(self) -> bool {
        self.bits & 0o004 != 0
    }
    pub fn other_write(self) -> bool {
        self.bits & 0o002 != 0
    }
    pub fn other_exec(self) -> bool {
        self.bits & 0o001 != 0
    }

    pub fn to_rwx_string(self) -> String {
        let mut s = String::with_capacity(9);
        s.push(if self.owner_read() { 'r' } else { '-' });
        s.push(if self.owner_write() { 'w' } else { '-' });
        s.push(if self.owner_exec() { 'x' } else { '-' });
        s.push(if self.group_read() { 'r' } else { '-' });
        s.push(if self.group_write() { 'w' } else { '-' });
        s.push(if self.group_exec() { 'x' } else { '-' });
        s.push(if self.other_read() { 'r' } else { '-' });
        s.push(if self.other_write() { 'w' } else { '-' });
        s.push(if self.other_exec() { 'x' } else { '-' });
        s
    }

    pub fn to_octal_string(self) -> String {
        let mut s = String::with_capacity(4);
        push_octal_digit(&mut s, (self.bits >> 9) & 7);
        push_octal_digit(&mut s, (self.bits >> 6) & 7);
        push_octal_digit(&mut s, (self.bits >> 3) & 7);
        push_octal_digit(&mut s, self.bits & 7);
        s
    }
}

fn push_octal_digit(s: &mut String, d: u16) {
    s.push((b'0' + d as u8) as char);
}

// ── File entry ───────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct FileEntry {
    pub name: String,
    pub path: String,
    pub entry_type: FileEntryType,
    pub size: u64,
    pub modified: u64,
    pub created: u64,
    pub accessed: u64,
    pub permissions: Permissions,
    pub owner: String,
    pub group: String,
    pub mime_type: String,
    pub symlink_target: Option<String>,
    pub is_hidden: bool,
    pub selected: bool,
    pub tags: Vec<String>,
    pub selinux_context: Option<String>,
}

impl FileEntry {
    pub fn new(name: &str, path: &str, entry_type: FileEntryType) -> Self {
        Self {
            name: String::from(name),
            path: String::from(path),
            entry_type,
            size: 0,
            modified: 0,
            created: 0,
            accessed: 0,
            permissions: Permissions::new(0o644),
            owner: String::from("root"),
            group: String::from("root"),
            mime_type: String::new(),
            symlink_target: None,
            is_hidden: name.starts_with('.'),
            selected: false,
            tags: Vec::new(),
            selinux_context: None,
        }
    }

    pub fn extension(&self) -> &str {
        if let Some(pos) = self.name.rfind('.') {
            &self.name[pos + 1..]
        } else {
            ""
        }
    }

    pub fn size_human(&self) -> String {
        format_size(self.size)
    }

    pub fn is_directory(&self) -> bool {
        self.entry_type == FileEntryType::Directory
    }
}

fn format_size(bytes: u64) -> String {
    if bytes < 1024 {
        let mut s = String::new();
        push_u64(&mut s, bytes);
        s.push_str(" B");
        return s;
    }
    let units = ["KiB", "MiB", "GiB", "TiB", "PiB"];
    let mut val = bytes as f64 / 1024.0;
    let mut unit_idx = 0;
    while val >= 1024.0 && unit_idx < units.len() - 1 {
        val /= 1024.0;
        unit_idx += 1;
    }
    let whole = val as u64;
    let frac = ((val - whole as f64) * 10.0) as u64;
    let mut s = String::new();
    push_u64(&mut s, whole);
    s.push('.');
    push_u64(&mut s, frac);
    s.push(' ');
    s.push_str(units[unit_idx]);
    s
}

fn push_u64(s: &mut String, mut n: u64) {
    if n == 0 {
        s.push('0');
        return;
    }
    let start = s.len();
    while n > 0 {
        s.push((b'0' + (n % 10) as u8) as char);
        n /= 10;
    }
    let bytes = unsafe { s.as_bytes_mut() };
    bytes[start..].reverse();
}

// ── Checksum ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChecksumAlgorithm {
    Md5,
    Sha1,
    Sha256,
}

#[derive(Debug, Clone)]
pub struct FileChecksum {
    pub algorithm: ChecksumAlgorithm,
    pub hex_value: String,
}

// ── Views ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewMode {
    Icon,
    List,
    Details,
    Column,
    Tree,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThumbnailSize {
    Small,
    Medium,
    Large,
    ExtraLarge,
}

impl ThumbnailSize {
    pub fn pixels(self) -> u32 {
        match self {
            Self::Small => 48,
            Self::Medium => 96,
            Self::Large => 192,
            Self::ExtraLarge => 384,
        }
    }
}

// ── Sort ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortField {
    Name,
    Size,
    Type,
    DateModified,
    DateCreated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortOrder {
    Ascending,
    Descending,
}

#[derive(Debug, Clone, Copy)]
pub struct SortConfig {
    pub field: SortField,
    pub order: SortOrder,
    pub folders_first: bool,
    pub case_insensitive: bool,
}

impl SortConfig {
    pub fn default_sort() -> Self {
        Self {
            field: SortField::Name,
            order: SortOrder::Ascending,
            folders_first: true,
            case_insensitive: true,
        }
    }

    pub fn compare(&self, a: &FileEntry, b: &FileEntry) -> core::cmp::Ordering {
        if self.folders_first {
            let a_dir = a.is_directory();
            let b_dir = b.is_directory();
            if a_dir && !b_dir {
                return core::cmp::Ordering::Less;
            }
            if !a_dir && b_dir {
                return core::cmp::Ordering::Greater;
            }
        }

        let primary = match self.field {
            SortField::Name => {
                if self.case_insensitive {
                    cmp_str_case_insensitive(&a.name, &b.name)
                } else {
                    a.name.as_str().cmp(b.name.as_str())
                }
            }
            SortField::Size => a.size.cmp(&b.size),
            SortField::Type => a.extension().cmp(b.extension()),
            SortField::DateModified => a.modified.cmp(&b.modified),
            SortField::DateCreated => a.created.cmp(&b.created),
        };

        match self.order {
            SortOrder::Ascending => primary,
            SortOrder::Descending => primary.reverse(),
        }
    }
}

fn cmp_str_case_insensitive(a: &str, b: &str) -> core::cmp::Ordering {
    let mut ai = a.chars().flat_map(|c| c.to_lowercase());
    let mut bi = b.chars().flat_map(|c| c.to_lowercase());
    loop {
        match (ai.next(), bi.next()) {
            (Some(ac), Some(bc)) => {
                let ord = ac.cmp(&bc);
                if ord != core::cmp::Ordering::Equal {
                    return ord;
                }
            }
            (Some(_), None) => return core::cmp::Ordering::Greater,
            (None, Some(_)) => return core::cmp::Ordering::Less,
            (None, None) => return core::cmp::Ordering::Equal,
        }
    }
}

// ── Filter ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct FileFilter {
    pub show_hidden: bool,
    pub type_filter: Option<FileEntryType>,
    pub glob_pattern: Option<String>,
    pub min_size: Option<u64>,
    pub max_size: Option<u64>,
    pub modified_after: Option<u64>,
    pub modified_before: Option<u64>,
}

impl FileFilter {
    pub fn default_filter() -> Self {
        Self {
            show_hidden: false,
            type_filter: None,
            glob_pattern: None,
            min_size: None,
            max_size: None,
            modified_after: None,
            modified_before: None,
        }
    }

    pub fn matches(&self, entry: &FileEntry) -> bool {
        if !self.show_hidden && entry.is_hidden {
            return false;
        }
        if let Some(t) = self.type_filter {
            if entry.entry_type != t {
                return false;
            }
        }
        if let Some(ref pattern) = self.glob_pattern {
            if !glob_match(pattern, &entry.name) {
                return false;
            }
        }
        if let Some(min) = self.min_size {
            if entry.size < min {
                return false;
            }
        }
        if let Some(max) = self.max_size {
            if entry.size > max {
                return false;
            }
        }
        if let Some(after) = self.modified_after {
            if entry.modified < after {
                return false;
            }
        }
        if let Some(before) = self.modified_before {
            if entry.modified > before {
                return false;
            }
        }
        true
    }
}

fn glob_match(pattern: &str, text: &str) -> bool {
    let mut pi = pattern.chars().peekable();
    let mut ti = text.chars().peekable();
    let mut star_p = None;
    let mut star_t = None;

    loop {
        match (pi.peek(), ti.peek()) {
            (Some(&'*'), _) => {
                pi.next();
                star_p = Some(pi.clone());
                star_t = Some(ti.clone());
            }
            (Some(&'?'), Some(_)) => {
                pi.next();
                ti.next();
            }
            (Some(&pc), Some(&tc)) if pc == tc => {
                pi.next();
                ti.next();
            }
            (None, None) => return true,
            _ => {
                if let (Some(sp), Some(mut st)) = (star_p.clone(), star_t.clone()) {
                    st.next();
                    star_t = Some(st.clone());
                    pi = sp;
                    ti = st;
                } else {
                    return false;
                }
            }
        }
    }
}

// ── Bookmark ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Bookmark {
    pub name: String,
    pub path: String,
    pub icon_char: char,
}

// ── Places (sidebar) ────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidebarSection {
    Places,
    Devices,
    Bookmarks,
    Tags,
    RecentFiles,
    Network,
    Trash,
}

#[derive(Debug, Clone)]
pub struct SidebarItem {
    pub section: SidebarSection,
    pub label: String,
    pub path: String,
    pub icon_char: char,
    pub is_removable: bool,
    pub is_mounted: bool,
}

#[derive(Debug, Clone)]
pub struct Sidebar {
    pub items: Vec<SidebarItem>,
    pub selected: Option<usize>,
    pub width: usize,
    pub collapsed: bool,
}

impl Sidebar {
    pub fn new() -> Self {
        let mut items = Vec::new();
        let places = [
            ("Home", "/home/user", 'H'),
            ("Desktop", "/home/user/Desktop", 'D'),
            ("Documents", "/home/user/Documents", 'd'),
            ("Downloads", "/home/user/Downloads", 'L'),
            ("Music", "/home/user/Music", 'M'),
            ("Pictures", "/home/user/Pictures", 'P'),
            ("Videos", "/home/user/Videos", 'V'),
        ];
        for (label, path, icon) in places {
            items.push(SidebarItem {
                section: SidebarSection::Places,
                label: String::from(label),
                path: String::from(path),
                icon_char: icon,
                is_removable: false,
                is_mounted: true,
            });
        }
        items.push(SidebarItem {
            section: SidebarSection::Trash,
            label: String::from("Trash"),
            path: String::from("trash:///"),
            icon_char: 'T',
            is_removable: false,
            is_mounted: true,
        });
        Self {
            items,
            selected: Some(0),
            width: 180,
            collapsed: false,
        }
    }

    pub fn add_device(&mut self, label: &str, path: &str, removable: bool) {
        self.items.push(SidebarItem {
            section: SidebarSection::Devices,
            label: String::from(label),
            path: String::from(path),
            icon_char: if removable { 'U' } else { 'K' },
            is_removable: removable,
            is_mounted: true,
        });
    }

    pub fn add_bookmark(&mut self, name: &str, path: &str) {
        self.items.push(SidebarItem {
            section: SidebarSection::Bookmarks,
            label: String::from(name),
            path: String::from(path),
            icon_char: 'B',
            is_removable: false,
            is_mounted: true,
        });
    }

    pub fn add_network_location(&mut self, label: &str, uri: &str) {
        self.items.push(SidebarItem {
            section: SidebarSection::Network,
            label: String::from(label),
            path: String::from(uri),
            icon_char: 'N',
            is_removable: false,
            is_mounted: false,
        });
    }

    pub fn remove_bookmark(&mut self, path: &str) {
        self.items
            .retain(|i| !(i.section == SidebarSection::Bookmarks && i.path == path));
    }

    pub fn items_for_section(&self, section: SidebarSection) -> Vec<&SidebarItem> {
        self.items.iter().filter(|i| i.section == section).collect()
    }
}

// ── Navigation ───────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct NavigationHistory {
    pub back: Vec<String>,
    pub forward: Vec<String>,
    pub current: String,
}

impl NavigationHistory {
    pub fn new(initial: &str) -> Self {
        Self {
            back: Vec::new(),
            forward: Vec::new(),
            current: String::from(initial),
        }
    }

    pub fn navigate(&mut self, path: &str) {
        self.back.push(self.current.clone());
        self.current.clear();
        self.current.push_str(path);
        self.forward.clear();
    }

    pub fn go_back(&mut self) -> bool {
        if let Some(prev) = self.back.pop() {
            self.forward.push(self.current.clone());
            self.current = prev;
            true
        } else {
            false
        }
    }

    pub fn go_forward(&mut self) -> bool {
        if let Some(next) = self.forward.pop() {
            self.back.push(self.current.clone());
            self.current = next;
            true
        } else {
            false
        }
    }

    pub fn go_up(&mut self) -> bool {
        if let Some(pos) = self.current.rfind('/') {
            if pos > 0 {
                let parent = String::from(&self.current[..pos]);
                self.navigate(&parent);
                return true;
            }
        }
        false
    }

    pub fn breadcrumbs(&self) -> Vec<(&str, String)> {
        let mut parts = Vec::new();
        let mut accumulated = String::new();
        for segment in self.current.split('/') {
            if segment.is_empty() {
                accumulated.push('/');
                parts.push(("/", String::from("/")));
                continue;
            }
            accumulated.push_str(segment);
            accumulated.push('/');
            parts.push((segment, accumulated.clone()));
        }
        parts
    }
}

// ── Clipboard ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipboardOp {
    Copy,
    Cut,
}

#[derive(Debug, Clone)]
pub struct FileClipboard {
    pub operation: ClipboardOp,
    pub paths: Vec<String>,
}

impl FileClipboard {
    pub fn new() -> Self {
        Self {
            operation: ClipboardOp::Copy,
            paths: Vec::new(),
        }
    }

    pub fn cut(&mut self, paths: Vec<String>) {
        self.operation = ClipboardOp::Cut;
        self.paths = paths;
    }

    pub fn copy(&mut self, paths: Vec<String>) {
        self.operation = ClipboardOp::Copy;
        self.paths = paths;
    }

    pub fn clear(&mut self) {
        self.paths.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.paths.is_empty()
    }
}

// ── File operations ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileOp {
    Copy,
    Move,
    Delete,
    Trash,
    Compress,
    Extract,
    Rename,
    CreateFile,
    CreateDirectory,
    CreateSymlink,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictResolution {
    Skip,
    Overwrite,
    OverwriteAll,
    Rename,
    Merge,
    Cancel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationStatus {
    Pending,
    Running,
    Paused,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone)]
pub struct FileOperation {
    pub id: u64,
    pub op: FileOp,
    pub sources: Vec<String>,
    pub destination: String,
    pub status: OperationStatus,
    pub current_file: String,
    pub total_bytes: u64,
    pub processed_bytes: u64,
    pub total_files: u64,
    pub processed_files: u64,
    pub speed_bytes_per_sec: u64,
    pub eta_seconds: u64,
    pub error_message: Option<String>,
    pub conflict_resolution: ConflictResolution,
}

impl FileOperation {
    pub fn new(id: u64, op: FileOp, sources: Vec<String>, dest: &str) -> Self {
        Self {
            id,
            op,
            sources,
            destination: String::from(dest),
            status: OperationStatus::Pending,
            current_file: String::new(),
            total_bytes: 0,
            processed_bytes: 0,
            total_files: 0,
            processed_files: 0,
            speed_bytes_per_sec: 0,
            eta_seconds: 0,
            error_message: None,
            conflict_resolution: ConflictResolution::Skip,
        }
    }

    pub fn progress_fraction(&self) -> f32 {
        if self.total_bytes == 0 {
            return 0.0;
        }
        self.processed_bytes as f32 / self.total_bytes as f32
    }

    pub fn pause(&mut self) {
        if self.status == OperationStatus::Running {
            self.status = OperationStatus::Paused;
        }
    }

    pub fn resume(&mut self) {
        if self.status == OperationStatus::Paused {
            self.status = OperationStatus::Running;
        }
    }

    pub fn cancel(&mut self) {
        self.status = OperationStatus::Cancelled;
    }
}

// ── Batch rename ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatchRenameMode {
    FindReplace,
    Sequential,
    DatePrefix,
    CaseChange,
    RegexReplace,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaseMode {
    Lower,
    Upper,
    Title,
    CamelCase,
    SnakeCase,
    KebabCase,
}

#[derive(Debug, Clone)]
pub struct BatchRename {
    pub mode: BatchRenameMode,
    pub find: String,
    pub replace: String,
    pub start_number: u32,
    pub step: u32,
    pub padding: u32,
    pub prefix: String,
    pub suffix: String,
    pub case_mode: CaseMode,
    pub preview: Vec<(String, String)>,
}

impl BatchRename {
    pub fn new() -> Self {
        Self {
            mode: BatchRenameMode::FindReplace,
            find: String::new(),
            replace: String::new(),
            start_number: 1,
            step: 1,
            padding: 3,
            prefix: String::new(),
            suffix: String::new(),
            case_mode: CaseMode::Lower,
            preview: Vec::new(),
        }
    }

    pub fn generate_preview(&mut self, files: &[FileEntry]) {
        self.preview.clear();
        for (i, file) in files.iter().enumerate() {
            let new_name = match self.mode {
                BatchRenameMode::FindReplace => {
                    file.name.replace(self.find.as_str(), self.replace.as_str())
                }
                BatchRenameMode::Sequential => {
                    let num = self.start_number + (i as u32) * self.step;
                    let ext = file.extension();
                    let base = if ext.is_empty() {
                        let mut s = self.prefix.clone();
                        push_padded_number(&mut s, num, self.padding);
                        s.push_str(&self.suffix);
                        s
                    } else {
                        let mut s = self.prefix.clone();
                        push_padded_number(&mut s, num, self.padding);
                        s.push_str(&self.suffix);
                        s.push('.');
                        s.push_str(ext);
                        s
                    };
                    base
                }
                BatchRenameMode::CaseChange => apply_case(&file.name, self.case_mode),
                BatchRenameMode::DatePrefix => {
                    let mut s = String::new();
                    push_u64(&mut s, file.modified);
                    s.push('_');
                    s.push_str(&file.name);
                    s
                }
                BatchRenameMode::RegexReplace => {
                    file.name.replace(self.find.as_str(), self.replace.as_str())
                }
            };
            self.preview.push((file.name.clone(), new_name));
        }
    }
}

fn push_padded_number(s: &mut String, n: u32, width: u32) {
    let mut buf = [0u8; 10];
    let mut pos = 10;
    let mut val = n;
    if val == 0 {
        pos -= 1;
        buf[pos] = b'0';
    } else {
        while val > 0 {
            pos -= 1;
            buf[pos] = b'0' + (val % 10) as u8;
            val /= 10;
        }
    }
    let digits = 10 - pos;
    for _ in 0..width.saturating_sub(digits as u32) {
        s.push('0');
    }
    for i in pos..10 {
        s.push(buf[i] as char);
    }
}

fn apply_case(name: &str, mode: CaseMode) -> String {
    match mode {
        CaseMode::Lower => {
            let mut s = String::with_capacity(name.len());
            for ch in name.chars() {
                for lc in ch.to_lowercase() {
                    s.push(lc);
                }
            }
            s
        }
        CaseMode::Upper => {
            let mut s = String::with_capacity(name.len());
            for ch in name.chars() {
                for uc in ch.to_uppercase() {
                    s.push(uc);
                }
            }
            s
        }
        CaseMode::Title => {
            let mut s = String::with_capacity(name.len());
            let mut capitalize_next = true;
            for ch in name.chars() {
                if ch == ' ' || ch == '_' || ch == '-' {
                    s.push(ch);
                    capitalize_next = true;
                } else if capitalize_next {
                    for uc in ch.to_uppercase() {
                        s.push(uc);
                    }
                    capitalize_next = false;
                } else {
                    for lc in ch.to_lowercase() {
                        s.push(lc);
                    }
                }
            }
            s
        }
        CaseMode::SnakeCase => {
            let mut s = String::with_capacity(name.len() + 4);
            for (i, ch) in name.chars().enumerate() {
                if ch.is_uppercase() && i > 0 {
                    s.push('_');
                }
                for lc in ch.to_lowercase() {
                    s.push(lc);
                }
            }
            s.replace(' ', "_").replace('-', "_")
        }
        CaseMode::KebabCase => {
            let mut s = String::with_capacity(name.len() + 4);
            for (i, ch) in name.chars().enumerate() {
                if ch.is_uppercase() && i > 0 {
                    s.push('-');
                }
                for lc in ch.to_lowercase() {
                    s.push(lc);
                }
            }
            s.replace(' ', "-").replace('_', "-")
        }
        CaseMode::CamelCase => {
            let mut s = String::with_capacity(name.len());
            let mut capitalize_next = false;
            for ch in name.chars() {
                if ch == ' ' || ch == '_' || ch == '-' {
                    capitalize_next = true;
                } else if capitalize_next {
                    for uc in ch.to_uppercase() {
                        s.push(uc);
                    }
                    capitalize_next = false;
                } else {
                    s.push(ch);
                }
            }
            s
        }
    }
}

// ── Compression ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveFormat {
    Zip,
    TarGz,
    TarBz2,
    TarXz,
    SevenZip,
    TarZst,
}

impl ArchiveFormat {
    pub fn extension(self) -> &'static str {
        match self {
            Self::Zip => "zip",
            Self::TarGz => "tar.gz",
            Self::TarBz2 => "tar.bz2",
            Self::TarXz => "tar.xz",
            Self::SevenZip => "7z",
            Self::TarZst => "tar.zst",
        }
    }
}

// ── Search ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchScope {
    CurrentDirectory,
    Recursive,
    Everywhere,
}

#[derive(Debug, Clone)]
pub struct SearchQuery {
    pub text: String,
    pub scope: SearchScope,
    pub search_contents: bool,
    pub use_regex: bool,
    pub case_sensitive: bool,
    pub type_filter: Option<FileEntryType>,
    pub size_min: Option<u64>,
    pub size_max: Option<u64>,
    pub date_min: Option<u64>,
    pub date_max: Option<u64>,
    pub owner_filter: Option<String>,
}

impl SearchQuery {
    pub fn simple(text: &str) -> Self {
        Self {
            text: String::from(text),
            scope: SearchScope::Recursive,
            search_contents: false,
            use_regex: false,
            case_sensitive: false,
            type_filter: None,
            size_min: None,
            size_max: None,
            date_min: None,
            date_max: None,
            owner_filter: None,
        }
    }

    pub fn matches_entry(&self, entry: &FileEntry) -> bool {
        let haystack = if self.case_sensitive {
            entry.name.clone()
        } else {
            let mut s = String::with_capacity(entry.name.len());
            for c in entry.name.chars() {
                for lc in c.to_lowercase() {
                    s.push(lc);
                }
            }
            s
        };
        let needle = if self.case_sensitive {
            self.text.clone()
        } else {
            let mut s = String::with_capacity(self.text.len());
            for c in self.text.chars() {
                for lc in c.to_lowercase() {
                    s.push(lc);
                }
            }
            s
        };

        if self.use_regex {
            haystack.contains(needle.as_str())
        } else {
            glob_match(&needle, &haystack)
        }
    }
}

#[derive(Debug, Clone)]
pub struct SearchResults {
    pub query: SearchQuery,
    pub results: Vec<FileEntry>,
    pub searching: bool,
    pub total_scanned: u64,
}

impl SearchResults {
    pub fn new(query: SearchQuery) -> Self {
        Self {
            query,
            results: Vec::new(),
            searching: true,
            total_scanned: 0,
        }
    }
}

// ── Thumbnails ───────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ThumbnailEntry {
    pub path: String,
    pub size: ThumbnailSize,
    pub data: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub generated: u64,
}

#[derive(Debug)]
pub struct ThumbnailCache {
    pub entries: BTreeMap<String, ThumbnailEntry>,
    pub max_entries: usize,
    pub generation_queue: Vec<String>,
}

impl ThumbnailCache {
    pub fn new(max_entries: usize) -> Self {
        Self {
            entries: BTreeMap::new(),
            max_entries,
            generation_queue: Vec::new(),
        }
    }

    pub fn get(&self, path: &str) -> Option<&ThumbnailEntry> {
        self.entries.get(path)
    }

    pub fn insert(&mut self, entry: ThumbnailEntry) {
        if self.entries.len() >= self.max_entries {
            if let Some(oldest) = self.entries.keys().next().cloned() {
                self.entries.remove(&oldest);
            }
        }
        let key = entry.path.clone();
        self.entries.insert(key, entry);
    }

    pub fn invalidate(&mut self, path: &str) {
        self.entries.remove(path);
    }

    pub fn queue_generation(&mut self, path: &str) {
        if !self.entries.contains_key(path) && !self.generation_queue.iter().any(|p| p == path) {
            self.generation_queue.push(String::from(path));
        }
    }

    pub fn drain_queue(&mut self, max: usize) -> Vec<String> {
        let count = max.min(self.generation_queue.len());
        self.generation_queue.drain(..count).collect()
    }
}

// ── Preview ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviewType {
    Text,
    Image,
    Video,
    Audio,
    Pdf,
    Code,
    Markdown,
    Binary,
    Directory,
    Unsupported,
}

#[derive(Debug, Clone)]
pub struct PreviewPanel {
    pub visible: bool,
    pub preview_type: PreviewType,
    pub text_content: String,
    pub file_path: String,
    pub scroll_offset: usize,
    pub width: usize,
    pub syntax_language: String,
}

impl PreviewPanel {
    pub fn new() -> Self {
        Self {
            visible: false,
            preview_type: PreviewType::Unsupported,
            text_content: String::new(),
            file_path: String::new(),
            scroll_offset: 0,
            width: 300,
            syntax_language: String::new(),
        }
    }

    pub fn toggle(&mut self) {
        self.visible = !self.visible;
    }

    pub fn set_file(&mut self, entry: &FileEntry) {
        self.file_path.clear();
        self.file_path.push_str(&entry.path);
        self.text_content.clear();
        self.scroll_offset = 0;
        self.preview_type = match entry.entry_type {
            FileEntryType::Image => PreviewType::Image,
            FileEntryType::Video => PreviewType::Video,
            FileEntryType::Audio => PreviewType::Audio,
            FileEntryType::Pdf => PreviewType::Pdf,
            FileEntryType::Code => PreviewType::Code,
            FileEntryType::Document if entry.extension() == "md" => PreviewType::Markdown,
            FileEntryType::Document => PreviewType::Text,
            FileEntryType::Directory => PreviewType::Directory,
            _ => PreviewType::Text,
        };
        self.syntax_language.clear();
        self.syntax_language.push_str(entry.extension());
    }
}

// ── Trash ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct TrashEntry {
    pub original_path: String,
    pub trash_path: String,
    pub deleted_at: u64,
    pub size: u64,
    pub entry_type: FileEntryType,
    pub name: String,
}

#[derive(Debug)]
pub struct Trash {
    pub entries: Vec<TrashEntry>,
    pub trash_dir: String,
}

impl Trash {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            trash_dir: String::from("/home/user/.local/share/Trash"),
        }
    }

    pub fn move_to_trash(&mut self, entry: &FileEntry, timestamp: u64) -> TrashEntry {
        let mut trash_path = self.trash_dir.clone();
        trash_path.push_str("/files/");
        trash_path.push_str(&entry.name);

        let te = TrashEntry {
            original_path: entry.path.clone(),
            trash_path,
            deleted_at: timestamp,
            size: entry.size,
            entry_type: entry.entry_type,
            name: entry.name.clone(),
        };
        self.entries.push(te.clone());
        te
    }

    pub fn restore(&mut self, trash_path: &str) -> Option<TrashEntry> {
        if let Some(pos) = self.entries.iter().position(|e| e.trash_path == trash_path) {
            Some(self.entries.remove(pos))
        } else {
            None
        }
    }

    pub fn empty(&mut self) {
        self.entries.clear();
    }

    pub fn total_size(&self) -> u64 {
        self.entries.iter().map(|e| e.size).sum()
    }

    pub fn count(&self) -> usize {
        self.entries.len()
    }
}

// ── Context menu ─────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum ContextMenuItem {
    Action {
        label: String,
        id: String,
        icon: char,
        shortcut: String,
        enabled: bool,
    },
    Separator,
    Submenu {
        label: String,
        icon: char,
        items: Vec<ContextMenuItem>,
    },
}

#[derive(Debug, Clone)]
pub struct ContextMenu {
    pub items: Vec<ContextMenuItem>,
    pub x: usize,
    pub y: usize,
    pub visible: bool,
    pub selected: Option<usize>,
    pub width: usize,
}

impl ContextMenu {
    pub fn new() -> Self {
        Self {
            items: Vec::new(),
            x: 0,
            y: 0,
            visible: false,
            selected: None,
            width: 200,
        }
    }

    pub fn show_for_file(&mut self, entry: &FileEntry, x: usize, y: usize) {
        self.items.clear();
        self.x = x;
        self.y = y;
        self.visible = true;
        self.selected = None;

        self.items.push(ContextMenuItem::Action {
            label: String::from("Open"),
            id: String::from("open"),
            icon: 'O',
            shortcut: String::from("Enter"),
            enabled: true,
        });
        self.items.push(ContextMenuItem::Submenu {
            label: String::from("Open With"),
            icon: 'W',
            items: vec![
                ContextMenuItem::Action {
                    label: String::from("Text Editor"),
                    id: String::from("open_text"),
                    icon: 'T',
                    shortcut: String::new(),
                    enabled: true,
                },
                ContextMenuItem::Action {
                    label: String::from("Image Viewer"),
                    id: String::from("open_image"),
                    icon: 'I',
                    shortcut: String::new(),
                    enabled: entry.entry_type == FileEntryType::Image,
                },
            ],
        });
        self.items.push(ContextMenuItem::Action {
            label: String::from("Open in Terminal"),
            id: String::from("open_terminal"),
            icon: '>',
            shortcut: String::new(),
            enabled: entry.is_directory(),
        });
        self.items.push(ContextMenuItem::Separator);
        self.items.push(ContextMenuItem::Action {
            label: String::from("Cut"),
            id: String::from("cut"),
            icon: 'X',
            shortcut: String::from("Ctrl+X"),
            enabled: true,
        });
        self.items.push(ContextMenuItem::Action {
            label: String::from("Copy"),
            id: String::from("copy"),
            icon: 'C',
            shortcut: String::from("Ctrl+C"),
            enabled: true,
        });
        self.items.push(ContextMenuItem::Action {
            label: String::from("Paste"),
            id: String::from("paste"),
            icon: 'V',
            shortcut: String::from("Ctrl+V"),
            enabled: true,
        });
        self.items.push(ContextMenuItem::Separator);
        self.items.push(ContextMenuItem::Action {
            label: String::from("Rename"),
            id: String::from("rename"),
            icon: 'R',
            shortcut: String::from("F2"),
            enabled: true,
        });
        self.items.push(ContextMenuItem::Action {
            label: String::from("Move to Trash"),
            id: String::from("trash"),
            icon: 'T',
            shortcut: String::from("Delete"),
            enabled: true,
        });
        self.items.push(ContextMenuItem::Action {
            label: String::from("Delete Permanently"),
            id: String::from("delete"),
            icon: 'D',
            shortcut: String::from("Shift+Del"),
            enabled: true,
        });
        self.items.push(ContextMenuItem::Separator);
        self.items.push(ContextMenuItem::Submenu {
            label: String::from("Compress"),
            icon: 'Z',
            items: vec![
                ContextMenuItem::Action {
                    label: String::from("ZIP"),
                    id: String::from("compress_zip"),
                    icon: 'Z',
                    shortcut: String::new(),
                    enabled: true,
                },
                ContextMenuItem::Action {
                    label: String::from("tar.gz"),
                    id: String::from("compress_targz"),
                    icon: 'G',
                    shortcut: String::new(),
                    enabled: true,
                },
                ContextMenuItem::Action {
                    label: String::from("tar.xz"),
                    id: String::from("compress_tarxz"),
                    icon: 'X',
                    shortcut: String::new(),
                    enabled: true,
                },
                ContextMenuItem::Action {
                    label: String::from("7z"),
                    id: String::from("compress_7z"),
                    icon: '7',
                    shortcut: String::new(),
                    enabled: true,
                },
            ],
        });
        if entry.entry_type == FileEntryType::Archive {
            self.items.push(ContextMenuItem::Action {
                label: String::from("Extract Here"),
                id: String::from("extract"),
                icon: 'E',
                shortcut: String::new(),
                enabled: true,
            });
        }
        self.items.push(ContextMenuItem::Separator);
        self.items.push(ContextMenuItem::Submenu {
            label: String::from("New"),
            icon: '+',
            items: vec![
                ContextMenuItem::Action {
                    label: String::from("Folder"),
                    id: String::from("new_folder"),
                    icon: 'F',
                    shortcut: String::from("Ctrl+Shift+N"),
                    enabled: true,
                },
                ContextMenuItem::Action {
                    label: String::from("File"),
                    id: String::from("new_file"),
                    icon: 'f',
                    shortcut: String::new(),
                    enabled: true,
                },
                ContextMenuItem::Action {
                    label: String::from("Symlink"),
                    id: String::from("new_symlink"),
                    icon: 'L',
                    shortcut: String::new(),
                    enabled: true,
                },
            ],
        });
        self.items.push(ContextMenuItem::Separator);
        self.items.push(ContextMenuItem::Action {
            label: String::from("Properties"),
            id: String::from("properties"),
            icon: 'i',
            shortcut: String::from("Alt+Enter"),
            enabled: true,
        });
    }

    pub fn show_background(&mut self, x: usize, y: usize) {
        self.items.clear();
        self.x = x;
        self.y = y;
        self.visible = true;
        self.selected = None;

        self.items.push(ContextMenuItem::Action {
            label: String::from("Paste"),
            id: String::from("paste"),
            icon: 'V',
            shortcut: String::from("Ctrl+V"),
            enabled: true,
        });
        self.items.push(ContextMenuItem::Separator);
        self.items.push(ContextMenuItem::Submenu {
            label: String::from("New"),
            icon: '+',
            items: vec![
                ContextMenuItem::Action {
                    label: String::from("Folder"),
                    id: String::from("new_folder"),
                    icon: 'F',
                    shortcut: String::from("Ctrl+Shift+N"),
                    enabled: true,
                },
                ContextMenuItem::Action {
                    label: String::from("File"),
                    id: String::from("new_file"),
                    icon: 'f',
                    shortcut: String::new(),
                    enabled: true,
                },
            ],
        });
        self.items.push(ContextMenuItem::Separator);
        self.items.push(ContextMenuItem::Action {
            label: String::from("Select All"),
            id: String::from("select_all"),
            icon: 'A',
            shortcut: String::from("Ctrl+A"),
            enabled: true,
        });
        self.items.push(ContextMenuItem::Action {
            label: String::from("Properties"),
            id: String::from("properties"),
            icon: 'i',
            shortcut: String::from("Alt+Enter"),
            enabled: true,
        });
    }

    pub fn dismiss(&mut self) {
        self.visible = false;
        self.items.clear();
    }

    pub fn select_next(&mut self) {
        let count = self.items.len();
        if count == 0 {
            return;
        }
        let mut idx = self.selected.map(|i| i + 1).unwrap_or(0);
        while idx < count {
            if !matches!(self.items[idx], ContextMenuItem::Separator) {
                self.selected = Some(idx);
                return;
            }
            idx += 1;
        }
    }

    pub fn select_prev(&mut self) {
        let count = self.items.len();
        if count == 0 {
            return;
        }
        let mut idx = self
            .selected
            .map(|i| i.saturating_sub(1))
            .unwrap_or(count - 1);
        loop {
            if !matches!(self.items[idx], ContextMenuItem::Separator) {
                self.selected = Some(idx);
                return;
            }
            if idx == 0 {
                break;
            }
            idx -= 1;
        }
    }
}

// ── Drag and drop ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DragEffect {
    Copy,
    Move,
    Link,
    None,
}

#[derive(Debug, Clone)]
pub struct DragState {
    pub active: bool,
    pub source_paths: Vec<String>,
    pub effect: DragEffect,
    pub cursor_x: usize,
    pub cursor_y: usize,
    pub drop_target: Option<String>,
}

impl DragState {
    pub fn new() -> Self {
        Self {
            active: false,
            source_paths: Vec::new(),
            effect: DragEffect::None,
            cursor_x: 0,
            cursor_y: 0,
            drop_target: None,
        }
    }

    pub fn start(&mut self, paths: Vec<String>, x: usize, y: usize) {
        self.active = true;
        self.source_paths = paths;
        self.effect = DragEffect::Move;
        self.cursor_x = x;
        self.cursor_y = y;
    }

    pub fn update(&mut self, x: usize, y: usize, modifier_ctrl: bool) {
        self.cursor_x = x;
        self.cursor_y = y;
        self.effect = if modifier_ctrl {
            DragEffect::Copy
        } else {
            DragEffect::Move
        };
    }

    pub fn cancel(&mut self) {
        self.active = false;
        self.source_paths.clear();
        self.drop_target = None;
    }

    pub fn drop_on(&mut self, target: &str) -> Option<(Vec<String>, DragEffect)> {
        if !self.active {
            return None;
        }
        self.active = false;
        let paths = core::mem::take(&mut self.source_paths);
        let effect = self.effect;
        self.drop_target = None;
        Some((paths, effect))
    }
}

// ── Volume / device management ───────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VolumeType {
    Internal,
    External,
    Usb,
    Optical,
    Network,
    Virtual,
}

#[derive(Debug, Clone)]
pub struct Volume {
    pub id: u64,
    pub label: String,
    pub mount_point: Option<String>,
    pub device_path: String,
    pub filesystem: String,
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub volume_type: VolumeType,
    pub mounted: bool,
    pub ejectable: bool,
    pub read_only: bool,
}

impl Volume {
    pub fn free_bytes(&self) -> u64 {
        self.total_bytes.saturating_sub(self.used_bytes)
    }

    pub fn usage_fraction(&self) -> f32 {
        if self.total_bytes == 0 {
            return 0.0;
        }
        self.used_bytes as f32 / self.total_bytes as f32
    }

    pub fn total_human(&self) -> String {
        format_size(self.total_bytes)
    }
    pub fn used_human(&self) -> String {
        format_size(self.used_bytes)
    }
    pub fn free_human(&self) -> String {
        format_size(self.free_bytes())
    }
}

#[derive(Debug)]
pub struct VolumeManager {
    pub volumes: Vec<Volume>,
    pub next_id: u64,
}

impl VolumeManager {
    pub fn new() -> Self {
        Self {
            volumes: Vec::new(),
            next_id: 1,
        }
    }

    pub fn add_volume(&mut self, label: &str, device: &str, fs: &str, vtype: VolumeType) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.volumes.push(Volume {
            id,
            label: String::from(label),
            mount_point: None,
            device_path: String::from(device),
            filesystem: String::from(fs),
            total_bytes: 0,
            used_bytes: 0,
            volume_type: vtype,
            mounted: false,
            ejectable: matches!(
                vtype,
                VolumeType::Usb | VolumeType::Optical | VolumeType::External
            ),
            read_only: false,
        });
        id
    }

    pub fn mount(&mut self, id: u64, mount_point: &str) -> bool {
        if let Some(vol) = self.volumes.iter_mut().find(|v| v.id == id) {
            vol.mounted = true;
            vol.mount_point = Some(String::from(mount_point));
            true
        } else {
            false
        }
    }

    pub fn unmount(&mut self, id: u64) -> bool {
        if let Some(vol) = self.volumes.iter_mut().find(|v| v.id == id) {
            vol.mounted = false;
            vol.mount_point = None;
            true
        } else {
            false
        }
    }

    pub fn eject(&mut self, id: u64) -> bool {
        if let Some(vol) = self.volumes.iter_mut().find(|v| v.id == id && v.ejectable) {
            vol.mounted = false;
            vol.mount_point = None;
            true
        } else {
            false
        }
    }

    pub fn mounted_volumes(&self) -> Vec<&Volume> {
        self.volumes.iter().filter(|v| v.mounted).collect()
    }
}

// ── Network locations ────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkProtocol {
    Smb,
    Ftp,
    Sftp,
    WebDav,
    Nfs,
}

impl NetworkProtocol {
    pub fn default_port(self) -> u16 {
        match self {
            Self::Smb => 445,
            Self::Ftp => 21,
            Self::Sftp => 22,
            Self::WebDav => 443,
            Self::Nfs => 2049,
        }
    }

    pub fn scheme(self) -> &'static str {
        match self {
            Self::Smb => "smb",
            Self::Ftp => "ftp",
            Self::Sftp => "sftp",
            Self::WebDav => "davs",
            Self::Nfs => "nfs",
        }
    }
}

#[derive(Debug, Clone)]
pub struct NetworkConnection {
    pub id: u64,
    pub protocol: NetworkProtocol,
    pub host: String,
    pub port: u16,
    pub share: String,
    pub username: String,
    pub domain: String,
    pub mount_point: Option<String>,
    pub connected: bool,
    pub saved: bool,
}

#[derive(Debug)]
pub struct NetworkManager {
    pub connections: Vec<NetworkConnection>,
    pub next_id: u64,
    pub credentials: BTreeMap<String, (String, String)>,
}

impl NetworkManager {
    pub fn new() -> Self {
        Self {
            connections: Vec::new(),
            next_id: 1,
            credentials: BTreeMap::new(),
        }
    }

    pub fn add_connection(
        &mut self,
        proto: NetworkProtocol,
        host: &str,
        share: &str,
        user: &str,
    ) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.connections.push(NetworkConnection {
            id,
            protocol: proto,
            host: String::from(host),
            port: proto.default_port(),
            share: String::from(share),
            username: String::from(user),
            domain: String::new(),
            mount_point: None,
            connected: false,
            saved: false,
        });
        id
    }

    pub fn connect(&mut self, id: u64, mount_point: &str) -> bool {
        if let Some(conn) = self.connections.iter_mut().find(|c| c.id == id) {
            conn.connected = true;
            conn.mount_point = Some(String::from(mount_point));
            true
        } else {
            false
        }
    }

    pub fn disconnect(&mut self, id: u64) -> bool {
        if let Some(conn) = self.connections.iter_mut().find(|c| c.id == id) {
            conn.connected = false;
            conn.mount_point = None;
            true
        } else {
            false
        }
    }

    pub fn save_connection(&mut self, id: u64) {
        if let Some(conn) = self.connections.iter_mut().find(|c| c.id == id) {
            conn.saved = true;
        }
    }

    pub fn store_credentials(&mut self, host: &str, username: &str, password: &str) {
        self.credentials.insert(
            String::from(host),
            (String::from(username), String::from(password)),
        );
    }

    pub fn saved_connections(&self) -> Vec<&NetworkConnection> {
        self.connections.iter().filter(|c| c.saved).collect()
    }
}

// ── Disk usage (treemap visualization data) ──────────────────────────────

#[derive(Debug, Clone)]
pub struct DiskUsageNode {
    pub name: String,
    pub path: String,
    pub size: u64,
    pub children: Vec<DiskUsageNode>,
    pub is_dir: bool,
}

impl DiskUsageNode {
    pub fn new_file(name: &str, path: &str, size: u64) -> Self {
        Self {
            name: String::from(name),
            path: String::from(path),
            size,
            children: Vec::new(),
            is_dir: false,
        }
    }

    pub fn new_dir(name: &str, path: &str) -> Self {
        Self {
            name: String::from(name),
            path: String::from(path),
            size: 0,
            children: Vec::new(),
            is_dir: true,
        }
    }

    pub fn total_size(&self) -> u64 {
        if self.children.is_empty() {
            self.size
        } else {
            self.children.iter().map(|c| c.total_size()).sum()
        }
    }

    pub fn add_child(&mut self, child: DiskUsageNode) {
        self.children.push(child);
    }
}

// ── Properties dialog ────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct PropertiesDialog {
    pub visible: bool,
    pub entry: Option<FileEntry>,
    pub checksums: Vec<FileChecksum>,
    pub calculating_checksums: bool,
}

impl PropertiesDialog {
    pub fn new() -> Self {
        Self {
            visible: false,
            entry: None,
            checksums: Vec::new(),
            calculating_checksums: false,
        }
    }

    pub fn show(&mut self, entry: FileEntry) {
        self.visible = true;
        self.entry = Some(entry);
        self.checksums.clear();
        self.calculating_checksums = false;
    }

    pub fn dismiss(&mut self) {
        self.visible = false;
        self.entry = None;
        self.checksums.clear();
    }

    pub fn add_checksum(&mut self, algo: ChecksumAlgorithm, value: &str) {
        self.checksums.push(FileChecksum {
            algorithm: algo,
            hex_value: String::from(value),
        });
    }
}

// ── Plugin extension points ──────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct PluginContextAction {
    pub id: String,
    pub label: String,
    pub icon: char,
    pub file_types: Vec<FileEntryType>,
}

#[derive(Debug, Clone)]
pub struct PluginColumn {
    pub id: String,
    pub label: String,
    pub width: usize,
}

#[derive(Debug)]
pub struct PluginRegistry {
    pub context_actions: Vec<PluginContextAction>,
    pub custom_columns: Vec<PluginColumn>,
    pub preview_handlers: BTreeMap<String, String>,
}

impl PluginRegistry {
    pub fn new() -> Self {
        Self {
            context_actions: Vec::new(),
            custom_columns: Vec::new(),
            preview_handlers: BTreeMap::new(),
        }
    }

    pub fn register_context_action(&mut self, action: PluginContextAction) {
        self.context_actions.push(action);
    }

    pub fn register_column(&mut self, column: PluginColumn) {
        self.custom_columns.push(column);
    }

    pub fn register_preview_handler(&mut self, extension: &str, handler_id: &str) {
        self.preview_handlers
            .insert(String::from(extension), String::from(handler_id));
    }

    pub fn actions_for_type(&self, ft: FileEntryType) -> Vec<&PluginContextAction> {
        self.context_actions
            .iter()
            .filter(|a| a.file_types.is_empty() || a.file_types.contains(&ft))
            .collect()
    }
}

// ── Dual pane ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DualPaneFocus {
    Left,
    Right,
}

#[derive(Debug)]
pub struct PaneState {
    pub entries: Vec<FileEntry>,
    pub selected_index: usize,
    pub scroll_offset: usize,
    pub current_path: String,
    pub navigation: NavigationHistory,
    pub sort: SortConfig,
    pub filter: FileFilter,
}

impl PaneState {
    pub fn new(path: &str) -> Self {
        Self {
            entries: Vec::new(),
            selected_index: 0,
            scroll_offset: 0,
            current_path: String::from(path),
            navigation: NavigationHistory::new(path),
            sort: SortConfig::default_sort(),
            filter: FileFilter::default_filter(),
        }
    }

    pub fn sorted_and_filtered(&self) -> Vec<&FileEntry> {
        let mut visible: Vec<&FileEntry> = self
            .entries
            .iter()
            .filter(|e| self.filter.matches(e))
            .collect();
        visible.sort_by(|a, b| self.sort.compare(a, b));
        visible
    }

    pub fn selected_entry(&self) -> Option<&FileEntry> {
        let filtered = self.sorted_and_filtered();
        filtered.get(self.selected_index).copied()
    }

    pub fn selected_entries(&self) -> Vec<&FileEntry> {
        self.entries.iter().filter(|e| e.selected).collect()
    }

    pub fn select_all(&mut self) {
        for e in &mut self.entries {
            e.selected = true;
        }
    }

    pub fn invert_selection(&mut self) {
        for e in &mut self.entries {
            e.selected = !e.selected;
        }
    }

    pub fn clear_selection(&mut self) {
        for e in &mut self.entries {
            e.selected = false;
        }
    }

    pub fn select_by_glob(&mut self, pattern: &str) {
        for e in &mut self.entries {
            e.selected = glob_match(pattern, &e.name);
        }
    }

    pub fn move_cursor(&mut self, delta: isize) {
        let count = self.sorted_and_filtered().len();
        if count == 0 {
            return;
        }
        let new_idx = if delta > 0 {
            (self.selected_index + delta as usize).min(count - 1)
        } else {
            self.selected_index.saturating_sub((-delta) as usize)
        };
        self.selected_index = new_idx;
    }
}

// ── Tab ──────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub struct FmTab {
    pub id: u64,
    pub label: String,
    pub left_pane: PaneState,
    pub right_pane: Option<PaneState>,
    pub dual_pane_focus: DualPaneFocus,
    pub sync_navigation: bool,
}

impl FmTab {
    pub fn new(id: u64, path: &str) -> Self {
        Self {
            id,
            label: path_last_component(path),
            left_pane: PaneState::new(path),
            right_pane: None,
            dual_pane_focus: DualPaneFocus::Left,
            sync_navigation: false,
        }
    }

    pub fn active_pane(&self) -> &PaneState {
        match self.dual_pane_focus {
            DualPaneFocus::Left => &self.left_pane,
            DualPaneFocus::Right => self.right_pane.as_ref().unwrap_or(&self.left_pane),
        }
    }

    pub fn active_pane_mut(&mut self) -> &mut PaneState {
        match self.dual_pane_focus {
            DualPaneFocus::Left => &mut self.left_pane,
            DualPaneFocus::Right => {
                if self.right_pane.is_none() {
                    self.right_pane = Some(PaneState::new(&self.left_pane.current_path));
                }
                self.right_pane.as_mut().unwrap()
            }
        }
    }

    pub fn enable_dual_pane(&mut self, path: &str) {
        if self.right_pane.is_none() {
            self.right_pane = Some(PaneState::new(path));
        }
    }

    pub fn disable_dual_pane(&mut self) {
        self.right_pane = None;
        self.dual_pane_focus = DualPaneFocus::Left;
    }

    pub fn toggle_focus(&mut self) {
        if self.right_pane.is_some() {
            self.dual_pane_focus = match self.dual_pane_focus {
                DualPaneFocus::Left => DualPaneFocus::Right,
                DualPaneFocus::Right => DualPaneFocus::Left,
            };
        }
    }
}

fn path_last_component(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    if let Some(pos) = trimmed.rfind('/') {
        String::from(&trimmed[pos + 1..])
    } else {
        String::from(trimmed)
    }
}

// ── File manager ─────────────────────────────────────────────────────────

pub struct FileManager {
    pub tabs: Vec<FmTab>,
    pub active_tab: usize,
    pub next_tab_id: u64,
    pub next_op_id: u64,

    pub view_mode: ViewMode,
    pub thumbnail_size: ThumbnailSize,

    pub sidebar: Sidebar,
    pub preview: PreviewPanel,
    pub trash: Trash,
    pub clipboard: FileClipboard,
    pub context_menu: ContextMenu,
    pub drag_state: DragState,
    pub batch_rename: BatchRename,
    pub properties: PropertiesDialog,

    pub operations: Vec<FileOperation>,
    pub search_results: Option<SearchResults>,
    pub thumbnail_cache: ThumbnailCache,

    pub volume_manager: VolumeManager,
    pub network_manager: NetworkManager,
    pub plugin_registry: PluginRegistry,

    pub show_status_bar: bool,
    pub show_path_bar: bool,
    pub confirm_delete: bool,
    pub follow_symlinks: bool,
    pub single_click_open: bool,
}

impl FileManager {
    pub fn new() -> Self {
        let tab = FmTab::new(1, "/home/user");
        Self {
            tabs: vec![tab],
            active_tab: 0,
            next_tab_id: 2,
            next_op_id: 1,
            view_mode: ViewMode::Details,
            thumbnail_size: ThumbnailSize::Medium,
            sidebar: Sidebar::new(),
            preview: PreviewPanel::new(),
            trash: Trash::new(),
            clipboard: FileClipboard::new(),
            context_menu: ContextMenu::new(),
            drag_state: DragState::new(),
            batch_rename: BatchRename::new(),
            properties: PropertiesDialog::new(),
            operations: Vec::new(),
            search_results: None,
            thumbnail_cache: ThumbnailCache::new(2000),
            volume_manager: VolumeManager::new(),
            network_manager: NetworkManager::new(),
            plugin_registry: PluginRegistry::new(),
            show_status_bar: true,
            show_path_bar: true,
            confirm_delete: true,
            follow_symlinks: true,
            single_click_open: false,
        }
    }

    pub fn new_tab(&mut self, path: &str) -> u64 {
        let id = self.next_tab_id;
        self.next_tab_id += 1;
        let tab = FmTab::new(id, path);
        self.tabs.push(tab);
        self.active_tab = self.tabs.len() - 1;
        id
    }

    pub fn close_tab(&mut self, tab_id: u64) {
        if self.tabs.len() <= 1 {
            return;
        }
        self.tabs.retain(|t| t.id != tab_id);
        if self.active_tab >= self.tabs.len() {
            self.active_tab = self.tabs.len() - 1;
        }
    }

    pub fn select_tab(&mut self, index: usize) {
        if index < self.tabs.len() {
            self.active_tab = index;
        }
    }

    pub fn active_tab(&self) -> &FmTab {
        &self.tabs[self.active_tab]
    }

    pub fn active_tab_mut(&mut self) -> &mut FmTab {
        &mut self.tabs[self.active_tab]
    }

    pub fn navigate(&mut self, path: &str) {
        let tab = self.active_tab_mut();
        tab.active_pane_mut().navigation.navigate(path);
        tab.active_pane_mut().current_path.clear();
        tab.active_pane_mut().current_path.push_str(path);
        tab.active_pane_mut().selected_index = 0;
        tab.active_pane_mut().scroll_offset = 0;
        tab.label = path_last_component(path);
    }

    pub fn go_back(&mut self) -> bool {
        let tab = self.active_tab_mut();
        let pane = tab.active_pane_mut();
        if pane.navigation.go_back() {
            pane.current_path = pane.navigation.current.clone();
            pane.selected_index = 0;
            true
        } else {
            false
        }
    }

    pub fn go_forward(&mut self) -> bool {
        let tab = self.active_tab_mut();
        let pane = tab.active_pane_mut();
        if pane.navigation.go_forward() {
            pane.current_path = pane.navigation.current.clone();
            pane.selected_index = 0;
            true
        } else {
            false
        }
    }

    pub fn go_up(&mut self) -> bool {
        let tab = self.active_tab_mut();
        let pane = tab.active_pane_mut();
        if pane.navigation.go_up() {
            pane.current_path = pane.navigation.current.clone();
            pane.selected_index = 0;
            true
        } else {
            false
        }
    }

    pub fn set_view_mode(&mut self, mode: ViewMode) {
        self.view_mode = mode;
    }

    pub fn toggle_hidden_files(&mut self) {
        let tab = self.active_tab_mut();
        let pane = tab.active_pane_mut();
        pane.filter.show_hidden = !pane.filter.show_hidden;
    }

    pub fn set_sort(&mut self, field: SortField, order: SortOrder) {
        let tab = self.active_tab_mut();
        let pane = tab.active_pane_mut();
        pane.sort.field = field;
        pane.sort.order = order;
    }

    pub fn enqueue_copy(&mut self, sources: Vec<String>, dest: &str) -> u64 {
        let id = self.next_op_id;
        self.next_op_id += 1;
        self.operations
            .push(FileOperation::new(id, FileOp::Copy, sources, dest));
        id
    }

    pub fn enqueue_move(&mut self, sources: Vec<String>, dest: &str) -> u64 {
        let id = self.next_op_id;
        self.next_op_id += 1;
        self.operations
            .push(FileOperation::new(id, FileOp::Move, sources, dest));
        id
    }

    pub fn enqueue_delete(&mut self, paths: Vec<String>) -> u64 {
        let id = self.next_op_id;
        self.next_op_id += 1;
        self.operations
            .push(FileOperation::new(id, FileOp::Delete, paths, ""));
        id
    }

    pub fn enqueue_trash(&mut self, entries: &[FileEntry], timestamp: u64) -> u64 {
        let id = self.next_op_id;
        self.next_op_id += 1;
        let paths: Vec<String> = entries.iter().map(|e| e.path.clone()).collect();
        for entry in entries {
            self.trash.move_to_trash(entry, timestamp);
        }
        self.operations
            .push(FileOperation::new(id, FileOp::Trash, paths, ""));
        id
    }

    pub fn enqueue_compress(
        &mut self,
        sources: Vec<String>,
        dest: &str,
        _format: ArchiveFormat,
    ) -> u64 {
        let id = self.next_op_id;
        self.next_op_id += 1;
        self.operations
            .push(FileOperation::new(id, FileOp::Compress, sources, dest));
        id
    }

    pub fn enqueue_extract(&mut self, archive: &str, dest: &str) -> u64 {
        let id = self.next_op_id;
        self.next_op_id += 1;
        self.operations.push(FileOperation::new(
            id,
            FileOp::Extract,
            vec![String::from(archive)],
            dest,
        ));
        id
    }

    pub fn cancel_operation(&mut self, id: u64) {
        if let Some(op) = self.operations.iter_mut().find(|o| o.id == id) {
            op.cancel();
        }
    }

    pub fn pause_operation(&mut self, id: u64) {
        if let Some(op) = self.operations.iter_mut().find(|o| o.id == id) {
            op.pause();
        }
    }

    pub fn resume_operation(&mut self, id: u64) {
        if let Some(op) = self.operations.iter_mut().find(|o| o.id == id) {
            op.resume();
        }
    }

    pub fn active_operations(&self) -> Vec<&FileOperation> {
        self.operations
            .iter()
            .filter(|o| {
                matches!(
                    o.status,
                    OperationStatus::Running | OperationStatus::Paused | OperationStatus::Pending
                )
            })
            .collect()
    }

    pub fn start_search(&mut self, query: SearchQuery) {
        self.search_results = Some(SearchResults::new(query));
    }

    pub fn clear_search(&mut self) {
        self.search_results = None;
    }

    pub fn cut_selected(&mut self) {
        let tab = self.active_tab();
        let paths: Vec<String> = tab
            .active_pane()
            .selected_entries()
            .iter()
            .map(|e| e.path.clone())
            .collect();
        if !paths.is_empty() {
            self.clipboard.cut(paths);
        }
    }

    pub fn copy_selected(&mut self) {
        let tab = self.active_tab();
        let paths: Vec<String> = tab
            .active_pane()
            .selected_entries()
            .iter()
            .map(|e| e.path.clone())
            .collect();
        if !paths.is_empty() {
            self.clipboard.copy(paths);
        }
    }

    pub fn paste(&mut self) -> Option<u64> {
        if self.clipboard.is_empty() {
            return None;
        }
        let dest = self.active_tab().active_pane().current_path.clone();
        let paths = self.clipboard.paths.clone();
        let op_id = match self.clipboard.operation {
            ClipboardOp::Copy => self.enqueue_copy(paths, &dest),
            ClipboardOp::Cut => {
                let id = self.enqueue_move(paths, &dest);
                self.clipboard.clear();
                id
            }
        };
        Some(op_id)
    }

    pub fn toggle_dual_pane(&mut self) {
        let tab = self.active_tab_mut();
        if tab.right_pane.is_some() {
            tab.disable_dual_pane();
        } else {
            let path = tab.left_pane.current_path.clone();
            tab.enable_dual_pane(&path);
        }
    }

    pub fn compare_directories(&self) -> Vec<(String, bool, bool)> {
        let tab = self.active_tab();
        let left_names: Vec<&str> = tab
            .left_pane
            .entries
            .iter()
            .map(|e| e.name.as_str())
            .collect();
        let right_names: Vec<&str> = match &tab.right_pane {
            Some(rp) => rp.entries.iter().map(|e| e.name.as_str()).collect(),
            None => return Vec::new(),
        };

        let mut result = Vec::new();
        let mut all_names: Vec<&str> = Vec::new();
        all_names.extend_from_slice(&left_names);
        for name in &right_names {
            if !all_names.contains(name) {
                all_names.push(name);
            }
        }
        all_names.sort();

        for name in all_names {
            let in_left = left_names.contains(&name);
            let in_right = right_names.contains(&name);
            result.push((String::from(name), in_left, in_right));
        }
        result
    }

    pub fn restore_from_trash(&mut self, trash_path: &str) -> Option<String> {
        if let Some(entry) = self.trash.restore(trash_path) {
            Some(entry.original_path)
        } else {
            None
        }
    }

    pub fn empty_trash(&mut self) {
        self.trash.empty();
    }

    pub fn status_text(&self) -> String {
        let tab = self.active_tab();
        let pane = tab.active_pane();
        let filtered = pane.sorted_and_filtered();
        let selected_count = pane.entries.iter().filter(|e| e.selected).count();
        let total_size: u64 = pane
            .entries
            .iter()
            .filter(|e| e.selected)
            .map(|e| e.size)
            .sum();

        let mut s = String::new();
        push_u64(&mut s, filtered.len() as u64);
        s.push_str(" items");
        if selected_count > 0 {
            s.push_str(" | ");
            push_u64(&mut s, selected_count as u64);
            s.push_str(" selected (");
            s.push_str(&format_size(total_size));
            s.push(')');
        }
        s
    }
}

// ── Rendering ────────────────────────────────────────────────────────────

impl FileManager {
    pub fn render(&self, canvas: &mut athgfx::Canvas, ox: usize, oy: usize, w: usize, h: usize) {
        canvas.fill_rect(ox, oy, w, h, FM_BG);

        let tab_h = GLYPH_H + 8;
        let mut tx = ox;
        for (i, tab) in self.tabs.iter().enumerate() {
            let bg = if i == self.active_tab {
                FM_TAB_ACTIVE
            } else {
                FM_TAB_BG
            };
            let label_w = tab.label.len() * GLYPH_W + 16;
            canvas.fill_rect(tx, oy, label_w, tab_h, bg);
            let fg = if i == self.active_tab {
                FM_ACCENT
            } else {
                FM_FG
            };
            canvas.draw_text(tx + 8, oy + 4, &tab.label, fg, None);
            tx += label_w + 2;
        }

        let path_y = oy + tab_h;
        let tab = self.active_tab();
        let pane = tab.active_pane();

        if self.show_path_bar {
            canvas.fill_rect(ox, path_y, w, GLYPH_H + 8, FM_HEADER_BG);
            canvas.draw_glyph(ox + 4, path_y + 4, '<', FM_ACCENT, None);
            canvas.draw_text(ox + 20, path_y + 4, &pane.current_path, FM_FG, None);
        }

        let content_y = path_y + if self.show_path_bar { GLYPH_H + 8 } else { 0 };
        let sidebar_w = 140usize.min(w / 3);

        canvas.fill_rect(
            ox,
            content_y,
            sidebar_w,
            h.saturating_sub(content_y - oy),
            FM_SIDEBAR_BG,
        );
        let bmarks = &self.sidebar.items;
        for (i, bm) in bmarks.iter().enumerate().take(20) {
            let by = content_y + 4 + i * (GLYPH_H + 6);
            if by + GLYPH_H > oy + h {
                break;
            }
            canvas.draw_glyph(ox + 8, by, bm.icon_char, FM_ACCENT, None);
            let max_chars = (sidebar_w.saturating_sub(24)) / GLYPH_W;
            let name = crate::text_util::truncate_chars(&bm.label, max_chars);
            canvas.draw_text(ox + 22, by, name, FM_FG, None);
        }

        for sy in content_y..oy + h {
            canvas.draw_pixel(ox + sidebar_w, sy, FM_BORDER);
        }

        let list_x = ox + sidebar_w + 4;
        let list_w = w.saturating_sub(sidebar_w + 4);

        if matches!(self.view_mode, ViewMode::Details) {
            let hdr_y = content_y + 2;
            canvas.draw_text(list_x, hdr_y, "Name", FM_DIM, None);
            let size_col = list_x + list_w / 2;
            canvas.draw_text(size_col, hdr_y, "Size", FM_DIM, None);
            let type_col = size_col + 80;
            canvas.draw_text(type_col, hdr_y, "Type", FM_DIM, None);

            let row_h = GLYPH_H + 4;
            let entries = pane.sorted_and_filtered();
            let max_visible = (h.saturating_sub(content_y - oy + row_h + 8)) / row_h;

            for (i, entry) in entries
                .iter()
                .skip(pane.scroll_offset)
                .take(max_visible)
                .enumerate()
            {
                let ey = content_y + row_h + 4 + i * row_h;
                let actual_idx = i + pane.scroll_offset;

                if actual_idx == pane.selected_index || entry.selected {
                    canvas.fill_rect(list_x, ey, list_w, row_h, FM_SELECTED);
                }

                let color = entry.entry_type.color();
                let icon = entry.entry_type.icon();
                canvas.draw_glyph(list_x + 2, ey, icon, color, None);

                let name_max = (list_w / 2 - 16) / GLYPH_W;
                let name = crate::text_util::truncate_chars(&entry.name, name_max);
                canvas.draw_text(list_x + 14, ey, name, color, None);

                canvas.draw_text(size_col, ey, &format_size(entry.size), FM_DIM, None);
                let type_str = entry.entry_type.short_label();
                canvas.draw_text(type_col, ey, type_str, FM_DIM, None);
            }
        }

        if self.show_status_bar {
            let sb_y = oy + h - GLYPH_H - 4;
            canvas.fill_rect(ox, sb_y, w, GLYPH_H + 4, FM_HEADER_BG);
            let status = self.status_text();
            canvas.draw_text(ox + 8, sb_y + 2, &status, FM_DIM, None);
        }
    }

    pub fn handle_key_input(&mut self, key: u8) {
        match key {
            b'j' | 0x50 => self.active_tab_mut().active_pane_mut().move_cursor(1),
            b'k' | 0x48 => self.active_tab_mut().active_pane_mut().move_cursor(-1),
            0x0D => {
                let tab = self.active_tab();
                let pane = tab.active_pane();
                if let Some(entry) = pane.selected_entry() {
                    if entry.entry_type == FileEntryType::Directory {
                        let path = entry.path.clone();
                        self.navigate(&path);
                    }
                }
            }
            0x08 => {
                self.go_back();
            }
            _ => {}
        }
    }

    pub fn handle_click_at(&mut self, x: usize, y: usize, content_y: usize) {
        let row_h = GLYPH_H + 4;
        if y > content_y + row_h {
            let idx = (y - content_y - row_h) / row_h;
            let pane = self.active_tab_mut().active_pane_mut();
            let count = pane.sorted_and_filtered().len();
            let actual = idx + pane.scroll_offset;
            if actual < count {
                pane.selected_index = actual;
            }
        }
        let _ = x;
    }
}

impl FileEntryType {
    pub fn icon(self) -> char {
        match self {
            Self::Directory => 'D',
            Self::Symlink => '@',
            Self::Executable => '*',
            Self::Archive => '#',
            Self::Image => 'I',
            Self::Video => 'V',
            Self::Audio => 'A',
            Self::Document | Self::Pdf => 'T',
            Self::Code => '>',
            _ => '-',
        }
    }

    pub fn short_label(self) -> &'static str {
        match self {
            Self::Directory => "DIR",
            Self::File => "FILE",
            Self::Symlink => "LINK",
            Self::Executable => "EXEC",
            Self::Image => "IMG",
            Self::Video => "VID",
            Self::Audio => "AUD",
            Self::Archive => "ZIP",
            Self::Document => "DOC",
            Self::Code => "CODE",
            Self::Pdf => "PDF",
            Self::Font => "FONT",
            Self::Database => "DB",
            Self::DeviceNode => "DEV",
            Self::Socket => "SOCK",
            Self::Pipe => "PIPE",
        }
    }
}

// ── Global instance ──────────────────────────────────────────────────────

struct FileManagerHolder {
    inner: Option<FileManager>,
}

static mut FILE_MANAGER_HOLDER: FileManagerHolder = FileManagerHolder { inner: None };
static FILE_MANAGER_INIT: AtomicBool = AtomicBool::new(false);

pub fn init() {
    if FILE_MANAGER_INIT.swap(true, Ordering::SeqCst) {
        return;
    }
    unsafe {
        FILE_MANAGER_HOLDER.inner = Some(FileManager::new());
    }
}

pub fn with_file_manager<F, R>(f: F) -> Option<R>
where
    F: FnOnce(&mut FileManager) -> R,
{
    if !FILE_MANAGER_INIT.load(Ordering::SeqCst) {
        return None;
    }
    unsafe { FILE_MANAGER_HOLDER.inner.as_mut().map(f) }
}

// ── Host KAT suite ─────────────────────────────────────────────────────────
//
// FAIL-able pure-logic KATs for the file-manager model (permissions, size
// formatting, extension/type mapping, path handling, sort/filter/glob, search).
// Every assertion is written against the POSIX / conventional CORRECT answer.
// Where the current implementation diverges, the test is `#[ignore]`d and its
// body documents input -> observed -> correct -> file:line as a `// BUG:` note
// so the divergence is visible without weakening the suite. Deterministic only.
#[cfg(test)]
mod fm_kat_tests {
    use super::*;

    fn entry(name: &str) -> FileEntry {
        FileEntry::new(name, "/", FileEntryType::File)
    }

    fn typed(name: &str, t: FileEntryType) -> FileEntry {
        FileEntry::new(name, "/", t)
    }

    fn sized(name: &str, size: u64) -> FileEntry {
        let mut e = FileEntry::new(name, "/", FileEntryType::File);
        e.size = size;
        e
    }

    // ── Permissions: to_rwx_string ────────────────────────────────────────

    #[test]
    fn rwx_0755() {
        assert_eq!(Permissions::new(0o755).to_rwx_string(), "rwxr-xr-x");
    }

    #[test]
    fn rwx_0644() {
        assert_eq!(Permissions::new(0o644).to_rwx_string(), "rw-r--r--");
    }

    #[test]
    fn rwx_0000() {
        assert_eq!(Permissions::new(0o000).to_rwx_string(), "---------");
    }

    #[test]
    fn rwx_0777() {
        assert_eq!(Permissions::new(0o777).to_rwx_string(), "rwxrwxrwx");
    }

    #[test]
    fn rwx_0700() {
        assert_eq!(Permissions::new(0o700).to_rwx_string(), "rwx------");
    }

    #[test]
    fn rwx_0111() {
        assert_eq!(Permissions::new(0o111).to_rwx_string(), "--x--x--x");
    }

    #[test]
    fn rwx_bit_order_0421() {
        // owner r-- (4), group -w- (2), other --x (1): pins triad bit ordering.
        assert_eq!(Permissions::new(0o421).to_rwx_string(), "r---w---x");
    }

    #[test]
    fn rwx_is_nine_chars_no_type_prefix() {
        // to_rwx_string is the bare 9-char perm field: no leading '-'/'d'/'l'.
        let s = Permissions::new(0o644).to_rwx_string();
        assert_eq!(s.len(), 9);
        assert!(s.starts_with('r'));
    }

    // ── Permissions: to_octal_string ──────────────────────────────────────

    #[test]
    fn octal_0755() {
        assert_eq!(Permissions::new(0o755).to_octal_string(), "0755");
    }

    #[test]
    fn octal_0644() {
        assert_eq!(Permissions::new(0o644).to_octal_string(), "0644");
    }

    #[test]
    fn octal_0000() {
        assert_eq!(Permissions::new(0o000).to_octal_string(), "0000");
    }

    #[test]
    fn octal_0777() {
        assert_eq!(Permissions::new(0o777).to_octal_string(), "0777");
    }

    #[test]
    fn octal_renders_special_triad_setuid() {
        // Special bits (setuid/setgid/sticky) occupy the leading octal digit.
        assert_eq!(Permissions::new(0o4755).to_octal_string(), "4755");
        assert_eq!(Permissions::new(0o1777).to_octal_string(), "1777");
    }

    // ── Permissions: bit accessors ────────────────────────────────────────

    #[test]
    fn accessors_0640() {
        let p = Permissions::new(0o640);
        assert!(p.owner_read());
        assert!(p.owner_write());
        assert!(!p.owner_exec());
        assert!(p.group_read());
        assert!(!p.group_write());
        assert!(!p.group_exec());
        assert!(!p.other_read());
        assert!(!p.other_write());
        assert!(!p.other_exec());
    }

    #[test]
    #[ignore = "BUG: to_rwx_string does not render setuid/setgid/sticky"]
    fn rwx_should_render_setuid_bit() {
        // BUG: input 0o4755 (setuid + rwxr-xr-x)
        //   observed: to_rwx_string() == "rwxr-xr-x" (the exec 'x' is shown as-is)
        //   correct (ls -l): "rwsr-xr-x" (owner exec becomes 's'); setgid -> 's'
        //                    in group triad; sticky -> 't' in other triad.
        //   suspect: to_rwx_string @ file_manager.rs:172-184 ignores bits 0o7000.
        assert_eq!(Permissions::new(0o4755).to_rwx_string(), "rwsr-xr-x");
    }

    // ── format_size ───────────────────────────────────────────────────────

    #[test]
    fn size_zero() {
        assert_eq!(format_size(0), "0 B");
    }

    #[test]
    fn size_one() {
        assert_eq!(format_size(1), "1 B");
    }

    #[test]
    fn size_512() {
        assert_eq!(format_size(512), "512 B");
    }

    #[test]
    fn size_1000_is_bytes_base_1024() {
        // Base-1024 formatter: 1000 is still under 1 KiB.
        assert_eq!(format_size(1000), "1000 B");
    }

    #[test]
    fn size_1023_upper_byte_boundary() {
        assert_eq!(format_size(1023), "1023 B");
    }

    #[test]
    fn size_1024_is_one_kib() {
        assert_eq!(format_size(1024), "1.0 KiB");
    }

    #[test]
    fn size_1536_is_1_5_kib() {
        assert_eq!(format_size(1536), "1.5 KiB");
    }

    #[test]
    fn size_one_mib() {
        assert_eq!(format_size(1024 * 1024), "1.0 MiB");
    }

    #[test]
    fn size_one_gib() {
        assert_eq!(format_size(1024 * 1024 * 1024), "1.0 GiB");
    }

    #[test]
    fn size_2047_truncates_fraction_characterization() {
        // Characterization of CURRENT behavior: the fractional digit is
        // truncated, not rounded. 2047/1024 = 1.999... -> shows "1.9 KiB".
        // (See size_2047_should_round_up for the convention divergence.)
        assert_eq!(format_size(2047), "1.9 KiB");
    }

    #[test]
    #[ignore = "BUG: format_size truncates the tenths digit instead of rounding"]
    fn size_2047_should_round_up() {
        // BUG: input 2047 bytes (= 1.9990 KiB)
        //   observed: "1.9 KiB"  (frac = ((val-whole)*10) as u64 truncates 9.99 -> 9)
        //   correct (ls -h convention rounds/ceils): "2.0 KiB"
        //   suspect: format_size @ file_manager.rs:276 truncates via `as u64`.
        assert_eq!(format_size(2047), "2.0 KiB");
    }

    #[test]
    fn size_human_delegates_to_format_size() {
        assert_eq!(sized("f", 1536).size_human(), "1.5 KiB");
    }

    // ── FileEntry::extension ──────────────────────────────────────────────

    #[test]
    fn ext_simple() {
        assert_eq!(entry("photo.png").extension(), "png");
    }

    #[test]
    fn ext_double_takes_last_segment() {
        assert_eq!(entry("archive.tar.gz").extension(), "gz");
    }

    #[test]
    fn ext_none() {
        assert_eq!(entry("README").extension(), "");
    }

    #[test]
    fn ext_trailing_dot_is_empty() {
        assert_eq!(entry("file.").extension(), "");
    }

    #[test]
    #[ignore = "BUG: dotfile name treated as an extension"]
    fn ext_dotfile_has_no_extension() {
        // BUG: input ".bashrc" (a hidden file, not "name.ext")
        //   observed: extension() == "bashrc"
        //   correct: "" (a leading dot denotes a hidden file, not an extension;
        //            cf. std::path::Path::extension(".bashrc") == None)
        //   suspect: extension() @ file_manager.rs:244-250 uses rfind('.') with
        //            no guard for a dot at index 0.
        assert_eq!(entry(".bashrc").extension(), "");
    }

    // ── FileEntryType::from_extension ─────────────────────────────────────

    #[test]
    fn from_ext_image() {
        assert_eq!(FileEntryType::from_extension("png"), FileEntryType::Image);
        assert_eq!(FileEntryType::from_extension("jpeg"), FileEntryType::Image);
    }

    #[test]
    fn from_ext_video_audio_archive() {
        assert_eq!(FileEntryType::from_extension("mp4"), FileEntryType::Video);
        assert_eq!(FileEntryType::from_extension("mp3"), FileEntryType::Audio);
        assert_eq!(FileEntryType::from_extension("gz"), FileEntryType::Archive);
    }

    #[test]
    fn from_ext_doc_code_pdf_font_db() {
        assert_eq!(
            FileEntryType::from_extension("txt"),
            FileEntryType::Document
        );
        assert_eq!(FileEntryType::from_extension("rs"), FileEntryType::Code);
        assert_eq!(FileEntryType::from_extension("pdf"), FileEntryType::Pdf);
        assert_eq!(FileEntryType::from_extension("ttf"), FileEntryType::Font);
        assert_eq!(
            FileEntryType::from_extension("sqlite"),
            FileEntryType::Database
        );
    }

    #[test]
    fn from_ext_unknown_is_file() {
        assert_eq!(FileEntryType::from_extension("xyzzy"), FileEntryType::File);
        assert_eq!(FileEntryType::from_extension(""), FileEntryType::File);
    }

    #[test]
    #[ignore = "BUG: from_extension is case-sensitive"]
    fn from_ext_should_be_case_insensitive() {
        // BUG: input "PNG" (an image extension in uppercase)
        //   observed: from_extension("PNG") == FileEntryType::File
        //   correct: FileEntryType::Image (extensions are case-insensitive;
        //            a "PHOTO.PNG" should get the image icon/colour)
        //   suspect: from_extension @ file_manager.rs:115-129 matches only
        //            lowercase literals without lowercasing the input.
        assert_eq!(FileEntryType::from_extension("PNG"), FileEntryType::Image);
    }

    // ── FileEntryType metadata ────────────────────────────────────────────

    #[test]
    fn type_short_labels() {
        assert_eq!(FileEntryType::Directory.short_label(), "DIR");
        assert_eq!(FileEntryType::Image.short_label(), "IMG");
        assert_eq!(FileEntryType::Archive.short_label(), "ZIP");
        assert_eq!(FileEntryType::Code.short_label(), "CODE");
    }

    #[test]
    fn type_colors() {
        assert_eq!(FileEntryType::Directory.color(), FM_DIR_FG);
        assert_eq!(FileEntryType::Executable.color(), FM_EXEC_FG);
        assert_eq!(FileEntryType::Document.color(), FM_DOC_FG);
        assert_eq!(FileEntryType::Pdf.color(), FM_DOC_FG);
        assert_eq!(FileEntryType::File.color(), FM_FG);
    }

    // ── FileEntry::new hidden detection ───────────────────────────────────

    #[test]
    fn hidden_flag_from_leading_dot() {
        assert!(entry(".config").is_hidden);
        assert!(!entry("config").is_hidden);
    }

    // ── cmp_str_case_insensitive ──────────────────────────────────────────

    #[test]
    fn ci_cmp_equal_ignoring_case() {
        assert_eq!(
            cmp_str_case_insensitive("abc", "ABC"),
            core::cmp::Ordering::Equal
        );
    }

    #[test]
    fn ci_cmp_less() {
        assert_eq!(
            cmp_str_case_insensitive("abc", "abd"),
            core::cmp::Ordering::Less
        );
    }

    #[test]
    fn ci_cmp_prefix_is_shorter_first() {
        assert_eq!(
            cmp_str_case_insensitive("abc", "abcd"),
            core::cmp::Ordering::Less
        );
        assert_eq!(
            cmp_str_case_insensitive("abcd", "abc"),
            core::cmp::Ordering::Greater
        );
    }

    #[test]
    fn ci_cmp_beats_naive_byte_order() {
        // Byte order would put 'Z'(0x5A) before 'a'(0x61); case-insensitive
        // must order "apple" before "Zebra".
        assert_eq!(
            cmp_str_case_insensitive("Zebra", "apple"),
            core::cmp::Ordering::Greater
        );
    }

    // ── SortConfig::compare ───────────────────────────────────────────────

    #[test]
    fn sort_folders_first_puts_dir_before_file() {
        let cfg = SortConfig::default_sort();
        let d = typed("zzz", FileEntryType::Directory);
        let f = typed("aaa", FileEntryType::File);
        assert_eq!(cfg.compare(&d, &f), core::cmp::Ordering::Less);
        assert_eq!(cfg.compare(&f, &d), core::cmp::Ordering::Greater);
    }

    #[test]
    fn sort_name_case_insensitive_default() {
        let cfg = SortConfig::default_sort();
        assert_eq!(
            cfg.compare(&entry("Apple"), &entry("banana")),
            core::cmp::Ordering::Less
        );
        assert_eq!(
            cfg.compare(&entry("ZEBRA"), &entry("apple")),
            core::cmp::Ordering::Greater
        );
    }

    #[test]
    fn sort_by_size_ascending() {
        let cfg = SortConfig {
            field: SortField::Size,
            order: SortOrder::Ascending,
            folders_first: false,
            case_insensitive: true,
        };
        assert_eq!(
            cfg.compare(&sized("a", 10), &sized("b", 20)),
            core::cmp::Ordering::Less
        );
    }

    #[test]
    fn sort_descending_reverses() {
        let cfg = SortConfig {
            field: SortField::Size,
            order: SortOrder::Descending,
            folders_first: false,
            case_insensitive: true,
        };
        assert_eq!(
            cfg.compare(&sized("a", 10), &sized("b", 20)),
            core::cmp::Ordering::Greater
        );
    }

    // ── FileFilter::matches ───────────────────────────────────────────────

    #[test]
    fn filter_hides_hidden_by_default() {
        let f = FileFilter::default_filter();
        assert!(!f.matches(&entry(".secret")));
        assert!(f.matches(&entry("visible")));
    }

    #[test]
    fn filter_show_hidden_passes_dotfiles() {
        let mut f = FileFilter::default_filter();
        f.show_hidden = true;
        assert!(f.matches(&entry(".secret")));
    }

    #[test]
    fn filter_by_type() {
        let mut f = FileFilter::default_filter();
        f.type_filter = Some(FileEntryType::Image);
        assert!(f.matches(&typed("a.png", FileEntryType::Image)));
        assert!(!f.matches(&typed("a.txt", FileEntryType::Document)));
    }

    #[test]
    fn filter_by_size_bounds() {
        let mut f = FileFilter::default_filter();
        f.min_size = Some(100);
        f.max_size = Some(200);
        assert!(!f.matches(&sized("small", 99)));
        assert!(f.matches(&sized("ok", 150)));
        assert!(!f.matches(&sized("big", 201)));
    }

    #[test]
    fn filter_glob_positive() {
        // Only glob patterns that MATCH are exercised here; a non-matching
        // '*' pattern hangs the matcher (see glob_star_nonmatch_hangs).
        let mut f = FileFilter::default_filter();
        f.glob_pattern = Some(String::from("*.txt"));
        assert!(f.matches(&entry("notes.txt")));
    }

    // ── glob_match (terminating cases only) ───────────────────────────────

    #[test]
    fn glob_star_matches_all() {
        assert!(glob_match("*", "anything"));
        assert!(glob_match("*", ""));
    }

    #[test]
    fn glob_suffix() {
        assert!(glob_match("*.txt", "readme.txt"));
        assert!(glob_match("*.txt", "a.txt"));
        assert!(glob_match("*.txt", ".txt"));
    }

    #[test]
    fn glob_question_matches_exactly_one() {
        assert!(glob_match("file?", "file1"));
        assert!(!glob_match("file?", "file")); // '?' requires a char
        assert!(glob_match("a?c", "abc"));
    }

    #[test]
    fn glob_literal_and_middle_star() {
        assert!(glob_match("abc", "abc"));
        assert!(!glob_match("abc", "abd"));
        assert!(!glob_match("abc", "ab"));
        assert!(glob_match("a*c", "abbbbc"));
    }

    #[test]
    #[ignore = "BUG: glob_match INFINITE-LOOPS on a non-matching '*' pattern (HANGS)"]
    fn glob_star_nonmatch_hangs() {
        // BUG (HIGH / hang): input pattern "*x", text "a"
        //   observed: glob_match never returns (infinite loop) -> the whole
        //             file-manager thread hangs whenever a '*' glob fails to
        //             match (e.g. filtering "*.txt" over a file named "readme").
        //   correct: return false.
        //   suspect: glob_match @ file_manager.rs:495-529 -- when the post-star
        //            tail cannot match and the text is exhausted, the fallback
        //            arm advances an already-empty `star_t` and re-loops on the
        //            identical (Some, None) state forever.
        //   WARNING: this assertion will HANG if un-ignored. Do not remove
        //            #[ignore] until the matcher is fixed.
        assert!(!glob_match("*x", "a"));
    }

    // ── SearchQuery::matches_entry ────────────────────────────────────────

    #[test]
    fn search_exact_name_matches() {
        assert!(SearchQuery::simple("report.pdf").matches_entry(&entry("report.pdf")));
    }

    #[test]
    fn search_is_case_insensitive_by_default() {
        assert!(SearchQuery::simple("REPORT.PDF").matches_entry(&entry("report.pdf")));
    }

    #[test]
    #[ignore = "BUG: simple (non-regex) search requires a full glob match, not substring"]
    fn search_simple_should_match_substring() {
        // BUG: input query "readme" (a plain substring the user typed)
        //   observed: SearchQuery::simple("readme").matches_entry("readme.txt")
        //             == false (glob_match requires the whole name to match)
        //   correct: true -- a simple search box matches substrings (the
        //            use_regex=true path already does `haystack.contains`).
        //   suspect: matches_entry @ file_manager.rs:1154-1158 routes the
        //            non-regex case through glob_match (anchored whole-string)
        //            instead of a substring/contains test.
        assert!(SearchQuery::simple("readme").matches_entry(&entry("readme.txt")));
    }

    // ── NavigationHistory ─────────────────────────────────────────────────

    #[test]
    fn nav_navigate_pushes_back_clears_forward() {
        let mut nav = NavigationHistory::new("/root");
        nav.navigate("/a");
        nav.navigate("/b");
        assert_eq!(nav.current, "/b");
        assert_eq!(nav.back, vec![String::from("/root"), String::from("/a")]);
        assert!(nav.forward.is_empty());
    }

    #[test]
    fn nav_back_and_forward_round_trip() {
        let mut nav = NavigationHistory::new("/root");
        nav.navigate("/a");
        nav.navigate("/b");
        assert!(nav.go_back());
        assert_eq!(nav.current, "/a");
        assert!(nav.go_back());
        assert_eq!(nav.current, "/root");
        assert!(!nav.go_back());
        assert!(nav.go_forward());
        assert_eq!(nav.current, "/a");
    }

    #[test]
    fn nav_go_up_from_nested() {
        let mut nav = NavigationHistory::new("/home/user");
        assert!(nav.go_up());
        assert_eq!(nav.current, "/home");
    }

    #[test]
    fn nav_breadcrumbs() {
        let nav = NavigationHistory::new("/home/user");
        let bc = nav.breadcrumbs();
        assert_eq!(bc.len(), 3);
        assert_eq!(bc[0], ("/", String::from("/")));
        assert_eq!(bc[1], ("home", String::from("/home/")));
        assert_eq!(bc[2], ("user", String::from("/home/user/")));
    }

    #[test]
    #[ignore = "BUG: go_up cannot reach root '/' from a top-level directory"]
    fn nav_go_up_from_toplevel_should_reach_root() {
        // BUG: input current == "/home"
        //   observed: go_up() returns false and current stays "/home"
        //   correct: go_up() returns true and current becomes "/" (the parent
        //            of "/home" is the root directory)
        //   suspect: go_up @ file_manager.rs:697-706 requires `pos > 0`, so a
        //            single leading-slash path never navigates to "/".
        let mut nav = NavigationHistory::new("/home");
        assert!(nav.go_up());
        assert_eq!(nav.current, "/");
    }

    // ── path_last_component ───────────────────────────────────────────────

    #[test]
    fn last_component_basic() {
        assert_eq!(path_last_component("/home/user"), "user");
        assert_eq!(path_last_component("/home/user/"), "user");
        assert_eq!(path_last_component("file"), "file");
    }

    #[test]
    fn last_component_root_is_empty_characterization() {
        // Characterization: basename of "/" yields "" here (POSIX `basename /`
        // is "/"). Low-severity divergence; pinned so a change is visible.
        assert_eq!(path_last_component("/"), "");
    }

    // ── FileClipboard ─────────────────────────────────────────────────────

    #[test]
    fn clipboard_cut_copy_clear() {
        let mut c = FileClipboard::new();
        assert!(c.is_empty());
        c.cut(vec![String::from("/a"), String::from("/b")]);
        assert_eq!(c.operation, ClipboardOp::Cut);
        assert_eq!(c.paths.len(), 2);
        assert!(!c.is_empty());
        c.copy(vec![String::from("/c")]);
        assert_eq!(c.operation, ClipboardOp::Copy);
        assert_eq!(c.paths.len(), 1);
        c.clear();
        assert!(c.is_empty());
        // clear() empties paths but does not reset the operation field.
        assert_eq!(c.operation, ClipboardOp::Copy);
    }
}
