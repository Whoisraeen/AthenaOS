//! Full-text search engine for the AthenaOS desktop shell.
//!
//! Provides file, application, and settings indexing with TF-IDF scoring,
//! trigram-based fuzzy matching, incremental updates, and calculator evaluation.

#![allow(unused)]

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

// ── no_std math helpers ──────────────────────────────────────────────────

fn f32_ln(x: f32) -> f32 {
    if x <= 0.0 {
        return -1e30;
    }
    let mut val = x as f64;
    let mut result: f64 = 0.0;
    while val > 2.0 {
        val /= 2.718281828;
        result += 1.0;
    }
    while val < 0.5 {
        val *= 2.718281828;
        result -= 1.0;
    }
    let y = (val - 1.0) / (val + 1.0);
    let y2 = y * y;
    let mut term = y;
    for i in 0..10 {
        result += 2.0 * term / (2 * i + 1) as f64;
        term *= y2;
    }
    result as f32
}

fn f64_floor(x: f64) -> f64 {
    let i = x as i64;
    if (i as f64) > x {
        (i - 1) as f64
    } else {
        i as f64
    }
}

fn f64_abs(x: f64) -> f64 {
    if x < 0.0 {
        -x
    } else {
        x
    }
}

// ── Types ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchResultType {
    File,
    Folder,
    Application,
    Setting,
    RecentDocument,
    WebSuggestion,
    Calculator,
    Definition,
    Contact,
    Email,
    Calendar,
    Bookmark,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileType {
    Document,
    Image,
    Video,
    Audio,
    Archive,
    Code,
    Executable,
    Font,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchSort {
    Relevance,
    Name,
    Date,
    Size,
    Frequency,
}

#[derive(Debug, Clone)]
pub enum SearchFilter {
    Type(SearchResultType),
    Extension(String),
    Path(String),
    DateAfter(u64),
    DateBefore(u64),
    SizeMin(u64),
    SizeMax(u64),
}

#[derive(Debug, Clone)]
pub enum SearchAction {
    Open(String),
    Launch(String),
    Navigate(String),
    Calculate(String),
    WebSearch(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexingState {
    Idle,
    Complete,
}

#[derive(Debug, Clone)]
pub enum IndexingProgress {
    Idle,
    Scanning(String, f32),
    Indexing(f32),
    Complete,
    Error(String),
}

#[derive(Debug, Clone)]
pub enum SearchError {
    NotFound(String),
    IoError(String),
    IndexCorrupted,
    PathExcluded,
}

// ── Index structures ─────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct IndexEntry {
    pub id: u64,
    pub title: String,
    pub content: String,
    pub path: Option<String>,
    pub entry_type: SearchResultType,
    pub metadata: BTreeMap<String, String>,
    pub score_boost: f32,
    pub last_accessed: u64,
    pub access_count: u32,
}

pub struct SearchIndex {
    pub name: String,
    pub entries: Vec<IndexEntry>,
    pub inverted: BTreeMap<String, Vec<usize>>,
    pub trigrams: BTreeMap<[u8; 3], Vec<usize>>,
    pub total_docs: usize,
    pub last_updated: u64,
}

impl SearchIndex {
    fn new(name: &str) -> Self {
        Self {
            name: String::from(name),
            entries: Vec::new(),
            inverted: BTreeMap::new(),
            trigrams: BTreeMap::new(),
            total_docs: 0,
            last_updated: 0,
        }
    }

    fn add_entry(&mut self, entry: IndexEntry, tokens: &[String], tri: &[[u8; 3]]) {
        let idx = self.entries.len();
        self.entries.push(entry);
        self.total_docs += 1;

        for token in tokens {
            self.inverted
                .entry(token.clone())
                .or_insert_with(Vec::new)
                .push(idx);
        }
        for t in tri {
            self.trigrams.entry(*t).or_insert_with(Vec::new).push(idx);
        }
    }

    fn remove_by_path(&mut self, path: &str) {
        let mut removed_indices = Vec::new();
        for (i, entry) in self.entries.iter().enumerate() {
            if entry.path.as_deref() == Some(path) {
                removed_indices.push(i);
            }
        }
        for &idx in removed_indices.iter().rev() {
            self.entries.remove(idx);
            self.total_docs = self.total_docs.saturating_sub(1);
        }
        self.rebuild_posting_lists();
    }

    fn rebuild_posting_lists(&mut self) {
        self.inverted.clear();
        self.trigrams.clear();
    }
}

// ── File index ───────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct FileIndexEntry {
    pub path: String,
    pub name: String,
    pub extension: String,
    pub size: u64,
    pub modified: u64,
    pub created: u64,
    pub file_type: FileType,
    pub thumbnail: Option<String>,
    pub content_hash: Option<u64>,
}

pub struct FileIndex {
    pub entries: Vec<FileIndexEntry>,
    pub path_map: BTreeMap<String, usize>,
    pub extension_map: BTreeMap<String, Vec<usize>>,
    pub size_total: u64,
}

impl FileIndex {
    fn new() -> Self {
        Self {
            entries: Vec::new(),
            path_map: BTreeMap::new(),
            extension_map: BTreeMap::new(),
            size_total: 0,
        }
    }

    fn add(&mut self, entry: FileIndexEntry) {
        let idx = self.entries.len();
        self.size_total += entry.size;
        self.path_map.insert(entry.path.clone(), idx);
        self.extension_map
            .entry(entry.extension.clone())
            .or_insert_with(Vec::new)
            .push(idx);
        self.entries.push(entry);
    }

    fn remove(&mut self, path: &str) {
        if let Some(&idx) = self.path_map.get(path) {
            if idx < self.entries.len() {
                self.size_total = self.size_total.saturating_sub(self.entries[idx].size);
            }
            self.path_map.remove(path);
        }
    }

    fn classify_extension(ext: &str) -> FileType {
        match ext {
            "doc" | "docx" | "pdf" | "txt" | "odt" | "rtf" | "md" => FileType::Document,
            "jpg" | "jpeg" | "png" | "gif" | "bmp" | "svg" | "webp" | "ico" => FileType::Image,
            "mp4" | "mkv" | "avi" | "mov" | "webm" | "flv" => FileType::Video,
            "mp3" | "flac" | "ogg" | "wav" | "aac" | "m4a" | "opus" => FileType::Audio,
            "zip" | "tar" | "gz" | "bz2" | "xz" | "7z" | "rar" | "zst" => FileType::Archive,
            "rs" | "c" | "cpp" | "h" | "py" | "js" | "ts" | "go" | "java" | "rb" => FileType::Code,
            "exe" | "elf" | "bin" | "sh" | "bat" | "cmd" => FileType::Executable,
            "ttf" | "otf" | "woff" | "woff2" => FileType::Font,
            _ => FileType::Other,
        }
    }
}

// ── Application and settings indices ─────────────────────────────────────

#[derive(Debug, Clone)]
pub struct AppIndexEntry {
    pub name: String,
    pub exec: String,
    pub icon: Option<String>,
    pub description: String,
    pub keywords: Vec<String>,
    pub categories: Vec<String>,
    pub desktop_file: String,
    pub usage_count: u32,
    pub last_used: u64,
}

pub struct AppIndex {
    pub apps: Vec<AppIndexEntry>,
}

impl AppIndex {
    fn new() -> Self {
        Self { apps: Vec::new() }
    }

    fn add(&mut self, entry: AppIndexEntry) {
        if let Some(existing) = self.apps.iter_mut().find(|a| a.exec == entry.exec) {
            existing.name = entry.name;
            existing.description = entry.description;
            existing.keywords = entry.keywords;
            existing.categories = entry.categories;
        } else {
            self.apps.push(entry);
        }
    }

    fn search(&self, query: &str) -> Vec<(usize, f32)> {
        let query_lower = to_lowercase(query);
        let mut results = Vec::new();

        for (i, app) in self.apps.iter().enumerate() {
            let mut score = 0.0f32;

            let name_lower = to_lowercase(&app.name);
            if name_lower == query_lower {
                score += 10.0;
            } else if name_lower.starts_with(&query_lower) {
                score += 7.0;
            } else if name_lower.contains(&query_lower) {
                score += 4.0;
            }

            let desc_lower = to_lowercase(&app.description);
            if desc_lower.contains(&query_lower) {
                score += 2.0;
            }

            for kw in &app.keywords {
                if to_lowercase(kw).contains(&query_lower) {
                    score += 3.0;
                    break;
                }
            }

            score += f32_ln(app.usage_count as f32).max(0.0) * 0.5;

            if score > 0.0 {
                results.push((i, score));
            }
        }

        results.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(core::cmp::Ordering::Equal));
        results
    }
}

#[derive(Debug, Clone)]
pub struct SettingIndexEntry {
    pub name: String,
    pub path: String,
    pub description: String,
    pub keywords: Vec<String>,
    pub category: String,
    pub icon: Option<String>,
}

pub struct SettingsIndex {
    pub settings: Vec<SettingIndexEntry>,
}

impl SettingsIndex {
    fn new() -> Self {
        Self {
            settings: Vec::new(),
        }
    }

    fn add(&mut self, entry: SettingIndexEntry) {
        self.settings.push(entry);
    }

    fn search(&self, query: &str) -> Vec<(usize, f32)> {
        let query_lower = to_lowercase(query);
        let mut results = Vec::new();

        for (i, s) in self.settings.iter().enumerate() {
            let mut score = 0.0f32;

            let name_lower = to_lowercase(&s.name);
            if name_lower.contains(&query_lower) {
                score += 5.0;
            }

            let desc_lower = to_lowercase(&s.description);
            if desc_lower.contains(&query_lower) {
                score += 2.0;
            }

            for kw in &s.keywords {
                if to_lowercase(kw).contains(&query_lower) {
                    score += 3.0;
                    break;
                }
            }

            if score > 0.0 {
                results.push((i, score));
            }
        }

        results.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(core::cmp::Ordering::Equal));
        results
    }
}

// ── Search query & result ────────────────────────────────────────────────

pub struct SearchQuery {
    pub text: String,
    pub filters: Vec<SearchFilter>,
    pub max_results: usize,
    pub include_types: Option<Vec<SearchResultType>>,
    pub date_range: Option<(u64, u64)>,
    pub size_range: Option<(u64, u64)>,
    pub sort: SearchSort,
}

impl SearchQuery {
    pub fn simple(text: &str) -> Self {
        Self {
            text: String::from(text),
            filters: Vec::new(),
            max_results: 20,
            include_types: None,
            date_range: None,
            size_range: None,
            sort: SearchSort::Relevance,
        }
    }
}

pub struct SearchResult {
    pub entry_type: SearchResultType,
    pub title: String,
    pub subtitle: String,
    pub icon: Option<String>,
    pub path: Option<String>,
    pub score: f32,
    pub highlights: Vec<(usize, usize)>,
    pub action: SearchAction,
}

#[derive(Debug, Clone)]
pub struct IndexStats {
    pub total_files: u64,
    pub total_apps: u64,
    pub total_settings: u64,
    pub index_size_bytes: u64,
    pub last_full_scan: u64,
    pub avg_query_ms: f32,
}

// ── SearchIndexer ────────────────────────────────────────────────────────

pub struct SearchIndexer {
    indices: BTreeMap<String, SearchIndex>,
    file_index: FileIndex,
    app_index: AppIndex,
    settings_index: SettingsIndex,
    web_suggestions: bool,
    max_results: usize,
    indexing_state: IndexingProgress,
    stats: IndexStats,
    exclusions: Vec<String>,
    watched_paths: Vec<String>,
    next_id: u64,
    query_times: Vec<f32>,
}

impl SearchIndexer {
    pub fn new() -> Self {
        let mut indices = BTreeMap::new();
        indices.insert(String::from("files"), SearchIndex::new("files"));
        indices.insert(String::from("apps"), SearchIndex::new("apps"));
        indices.insert(String::from("settings"), SearchIndex::new("settings"));

        Self {
            indices,
            file_index: FileIndex::new(),
            app_index: AppIndex::new(),
            settings_index: SettingsIndex::new(),
            web_suggestions: false,
            max_results: 50,
            indexing_state: IndexingProgress::Idle,
            stats: IndexStats {
                total_files: 0,
                total_apps: 0,
                total_settings: 0,
                index_size_bytes: 0,
                last_full_scan: 0,
                avg_query_ms: 0.0,
            },
            exclusions: Vec::new(),
            watched_paths: Vec::new(),
            next_id: 1,
            query_times: Vec::new(),
        }
    }

    pub fn search(&self, query: &SearchQuery) -> Vec<SearchResult> {
        let text = query.text.as_str();
        if text.is_empty() {
            return Vec::new();
        }

        if let Some(calc_result) = self.evaluate_calculator(text) {
            return vec![SearchResult {
                entry_type: SearchResultType::Calculator,
                title: format!("{} = {}", text, calc_result),
                subtitle: String::from("Calculator"),
                icon: Some(String::from("=")),
                path: None,
                score: 100.0,
                highlights: Vec::new(),
                action: SearchAction::Calculate(calc_result),
            }];
        }

        let mut results = Vec::new();

        self.search_files(text, &query.filters, &mut results);
        self.search_apps(text, &mut results);
        self.search_settings(text, &mut results);
        self.search_generic_indices(text, &mut results);

        if let Some(ref types) = query.include_types {
            results.retain(|r| types.contains(&r.entry_type));
        }

        self.apply_filters(&mut results, &query.filters);
        self.rank_results(&mut results, text);

        match query.sort {
            SearchSort::Relevance => {}
            SearchSort::Name => results.sort_by(|a, b| a.title.cmp(&b.title)),
            SearchSort::Date => {}
            SearchSort::Size => {}
            SearchSort::Frequency => results.sort_by(|a, b| {
                b.score
                    .partial_cmp(&a.score)
                    .unwrap_or(core::cmp::Ordering::Equal)
            }),
        }

        results.truncate(query.max_results.min(self.max_results));
        results
    }

    pub fn quick_search(&self, text: &str) -> Vec<SearchResult> {
        let query = SearchQuery {
            text: String::from(text),
            filters: Vec::new(),
            max_results: 10,
            include_types: None,
            date_range: None,
            size_range: None,
            sort: SearchSort::Relevance,
        };
        self.search(&query)
    }

    pub fn index_file(&mut self, path: &str) -> Result<(), SearchError> {
        if self.is_excluded(path) {
            return Err(SearchError::PathExcluded);
        }

        let name = path_filename(path);
        let ext = path_extension(path);
        let file_type = FileIndex::classify_extension(&ext);

        let file_entry = FileIndexEntry {
            path: String::from(path),
            name: name.clone(),
            extension: ext.clone(),
            size: 0,
            modified: 0,
            created: 0,
            file_type,
            thumbnail: None,
            content_hash: None,
        };
        self.file_index.add(file_entry);

        let id = self.next_id;
        self.next_id += 1;

        let index_entry = IndexEntry {
            id,
            title: name.clone(),
            content: String::new(),
            path: Some(String::from(path)),
            entry_type: SearchResultType::File,
            metadata: BTreeMap::new(),
            score_boost: 1.0,
            last_accessed: 0,
            access_count: 0,
        };

        let tokens = self.tokenize(&name);
        let trigrams = self.generate_trigrams(&name);

        if let Some(idx) = self.indices.get_mut("files") {
            idx.add_entry(index_entry, &tokens, &trigrams);
        }

        self.stats.total_files += 1;
        self.update_index_size();

        Ok(())
    }

    pub fn remove_file(&mut self, path: &str) {
        self.file_index.remove(path);
        if let Some(idx) = self.indices.get_mut("files") {
            idx.remove_by_path(path);
        }
        self.stats.total_files = self.stats.total_files.saturating_sub(1);
        self.update_index_size();
    }

    pub fn index_directory(&mut self, path: &str, recursive: bool) -> Result<u64, SearchError> {
        if self.is_excluded(path) {
            return Err(SearchError::PathExcluded);
        }

        self.indexing_state = IndexingProgress::Scanning(String::from(path), 0.0);

        let id = self.next_id;
        self.next_id += 1;

        let entry = IndexEntry {
            id,
            title: path_filename(path),
            content: String::new(),
            path: Some(String::from(path)),
            entry_type: SearchResultType::Folder,
            metadata: BTreeMap::new(),
            score_boost: 0.5,
            last_accessed: 0,
            access_count: 0,
        };

        let tokens = self.tokenize(&entry.title);
        let trigrams = self.generate_trigrams(&entry.title);

        if let Some(idx) = self.indices.get_mut("files") {
            idx.add_entry(entry, &tokens, &trigrams);
        }

        if !self.watched_paths.contains(&String::from(path)) {
            self.watched_paths.push(String::from(path));
        }

        self.indexing_state = IndexingProgress::Complete;
        Ok(1)
    }

    pub fn index_application(&mut self, entry: AppIndexEntry) {
        let id = self.next_id;
        self.next_id += 1;

        let name = entry.name.clone();
        let desc = entry.description.clone();
        let exec = entry.exec.clone();

        self.app_index.add(entry);

        let index_entry = IndexEntry {
            id,
            title: name.clone(),
            content: desc,
            path: Some(exec.clone()),
            entry_type: SearchResultType::Application,
            metadata: BTreeMap::new(),
            score_boost: 2.0,
            last_accessed: 0,
            access_count: 0,
        };

        let tokens = self.tokenize(&name);
        let trigrams = self.generate_trigrams(&name);

        if let Some(idx) = self.indices.get_mut("apps") {
            idx.add_entry(index_entry, &tokens, &trigrams);
        }

        self.stats.total_apps += 1;
    }

    pub fn index_setting(&mut self, entry: SettingIndexEntry) {
        let id = self.next_id;
        self.next_id += 1;

        let name = entry.name.clone();
        let path = entry.path.clone();
        let desc = entry.description.clone();

        self.settings_index.add(entry);

        let index_entry = IndexEntry {
            id,
            title: name.clone(),
            content: desc,
            path: Some(path),
            entry_type: SearchResultType::Setting,
            metadata: BTreeMap::new(),
            score_boost: 1.5,
            last_accessed: 0,
            access_count: 0,
        };

        let tokens = self.tokenize(&name);
        let trigrams = self.generate_trigrams(&name);

        if let Some(idx) = self.indices.get_mut("settings") {
            idx.add_entry(index_entry, &tokens, &trigrams);
        }

        self.stats.total_settings += 1;
    }

    pub fn rebuild_index(&mut self) {
        self.indexing_state = IndexingProgress::Indexing(0.0);

        for (_, index) in self.indices.iter_mut() {
            index.inverted.clear();
            index.trigrams.clear();

            for i in 0..index.entries.len() {
                let title = index.entries[i].title.clone();
                let content = index.entries[i].content.clone();
                let combined = format!("{} {}", title, content);

                let tokens = tokenize_text(&combined);
                for token in &tokens {
                    index
                        .inverted
                        .entry(token.clone())
                        .or_insert_with(Vec::new)
                        .push(i);
                }

                let tris = generate_trigrams_for(&combined);
                for t in &tris {
                    index.trigrams.entry(*t).or_insert_with(Vec::new).push(i);
                }
            }
        }

        self.update_index_size();
        self.indexing_state = IndexingProgress::Complete;
    }

    pub fn update_incremental(&mut self, changed_paths: &[String]) {
        for path in changed_paths {
            self.remove_file(path);
            let _ = self.index_file(path);
        }
    }

    pub fn add_exclusion(&mut self, pattern: &str) {
        if !self.exclusions.contains(&String::from(pattern)) {
            self.exclusions.push(String::from(pattern));
        }
    }

    pub fn stats(&self) -> IndexStats {
        self.stats.clone()
    }

    // ── Internal search helpers ──────────────────────────────────────────

    fn search_files(&self, query: &str, filters: &[SearchFilter], results: &mut Vec<SearchResult>) {
        if let Some(index) = self.indices.get("files") {
            let tokens = self.tokenize(query);

            for token in &tokens {
                if let Some(entry_indices) = index.inverted.get(token) {
                    for &ei in entry_indices {
                        if let Some(entry) = index.entries.get(ei) {
                            let tf_idf = self.calculate_tf_idf(token, ei, index);
                            let (fuzz_score, highlights) = self
                                .fuzzy_match(query, &entry.title)
                                .unwrap_or((0.0, Vec::new()));

                            results.push(SearchResult {
                                entry_type: entry.entry_type,
                                title: entry.title.clone(),
                                subtitle: entry.path.clone().unwrap_or_default(),
                                icon: None,
                                path: entry.path.clone(),
                                score: tf_idf + fuzz_score + entry.score_boost,
                                highlights,
                                action: SearchAction::Open(entry.path.clone().unwrap_or_default()),
                            });
                        }
                    }
                }
            }

            self.search_by_trigrams(query, index, results);
        }
    }

    fn search_apps(&self, query: &str, results: &mut Vec<SearchResult>) {
        let app_results = self.app_index.search(query);
        for (idx, score) in app_results {
            if let Some(app) = self.app_index.apps.get(idx) {
                results.push(SearchResult {
                    entry_type: SearchResultType::Application,
                    title: app.name.clone(),
                    subtitle: app.description.clone(),
                    icon: app.icon.clone(),
                    path: Some(app.exec.clone()),
                    score,
                    highlights: Vec::new(),
                    action: SearchAction::Launch(app.exec.clone()),
                });
            }
        }
    }

    fn search_settings(&self, query: &str, results: &mut Vec<SearchResult>) {
        let setting_results = self.settings_index.search(query);
        for (idx, score) in setting_results {
            if let Some(s) = self.settings_index.settings.get(idx) {
                results.push(SearchResult {
                    entry_type: SearchResultType::Setting,
                    title: s.name.clone(),
                    subtitle: s.description.clone(),
                    icon: s.icon.clone(),
                    path: Some(s.path.clone()),
                    score,
                    highlights: Vec::new(),
                    action: SearchAction::Navigate(s.path.clone()),
                });
            }
        }
    }

    fn search_generic_indices(&self, query: &str, results: &mut Vec<SearchResult>) {
        for (key, index) in &self.indices {
            if key == "files" || key == "apps" || key == "settings" {
                continue;
            }
            let tokens = self.tokenize(query);
            for token in &tokens {
                if let Some(entry_indices) = index.inverted.get(token.as_str()) {
                    for &ei in entry_indices {
                        if let Some(entry) = index.entries.get(ei) {
                            results.push(SearchResult {
                                entry_type: entry.entry_type,
                                title: entry.title.clone(),
                                subtitle: entry.content.clone(),
                                icon: None,
                                path: entry.path.clone(),
                                score: entry.score_boost,
                                highlights: Vec::new(),
                                action: SearchAction::Open(entry.path.clone().unwrap_or_default()),
                            });
                        }
                    }
                }
            }
        }
    }

    fn search_by_trigrams(
        &self,
        query: &str,
        index: &SearchIndex,
        results: &mut Vec<SearchResult>,
    ) {
        let qtri = self.generate_trigrams(query);
        if qtri.is_empty() {
            return;
        }

        let mut hit_counts: BTreeMap<usize, usize> = BTreeMap::new();
        for tri in &qtri {
            if let Some(indices) = index.trigrams.get(tri) {
                for &idx in indices {
                    *hit_counts.entry(idx).or_insert(0) += 1;
                }
            }
        }

        let threshold = qtri.len() / 3;
        for (&entry_idx, &hits) in &hit_counts {
            if hits >= threshold.max(1) {
                if let Some(entry) = index.entries.get(entry_idx) {
                    let already_present = results.iter().any(|r| r.title == entry.title);
                    if !already_present {
                        let score = hits as f32 / qtri.len() as f32;
                        results.push(SearchResult {
                            entry_type: entry.entry_type,
                            title: entry.title.clone(),
                            subtitle: entry.path.clone().unwrap_or_default(),
                            icon: None,
                            path: entry.path.clone(),
                            score: score * 0.5,
                            highlights: Vec::new(),
                            action: SearchAction::Open(entry.path.clone().unwrap_or_default()),
                        });
                    }
                }
            }
        }
    }

    fn apply_filters(&self, results: &mut Vec<SearchResult>, filters: &[SearchFilter]) {
        for filter in filters {
            match filter {
                SearchFilter::Type(t) => {
                    results.retain(|r| r.entry_type == *t);
                }
                SearchFilter::Extension(ext) => {
                    results
                        .retain(|r| r.path.as_ref().map_or(false, |p| p.ends_with(ext.as_str())));
                }
                SearchFilter::Path(prefix) => {
                    results.retain(|r| {
                        r.path
                            .as_ref()
                            .map_or(false, |p| p.starts_with(prefix.as_str()))
                    });
                }
                _ => {}
            }
        }
    }

    fn tokenize(&self, text: &str) -> Vec<String> {
        tokenize_text(text)
    }

    fn calculate_tf_idf(&self, term: &str, doc_idx: usize, index: &SearchIndex) -> f32 {
        let entry = match index.entries.get(doc_idx) {
            Some(e) => e,
            None => return 0.0,
        };

        let combined = format!("{} {}", entry.title, entry.content);
        let tokens = tokenize_text(&combined);
        let doc_len = tokens.len() as f32;
        if doc_len == 0.0 {
            return 0.0;
        }

        let tf = tokens.iter().filter(|t| t.as_str() == term).count() as f32 / doc_len;

        let df = index.inverted.get(term).map_or(1, |v| v.len()) as f32;
        let n = index.total_docs.max(1) as f32;
        let idf = f32_ln(n / df) + 1.0;

        tf * idf
    }

    fn fuzzy_match(&self, query: &str, target: &str) -> Option<(f32, Vec<(usize, usize)>)> {
        let q = to_lowercase(query);
        let t = to_lowercase(target);

        if t.contains(&q) {
            let start = t.find(&q).unwrap_or(0);
            let end = start + q.len();
            let score = q.len() as f32 / t.len() as f32 * 5.0;
            return Some((score, vec![(start, end)]));
        }

        let mut qi = 0usize;
        let mut highlights = Vec::new();
        let q_bytes = q.as_bytes();
        let t_bytes = t.as_bytes();

        for (ti, &tb) in t_bytes.iter().enumerate() {
            if qi < q_bytes.len() && tb == q_bytes[qi] {
                highlights.push((ti, ti + 1));
                qi += 1;
            }
        }

        if qi == q_bytes.len() {
            let score = q.len() as f32 / t.len().max(1) as f32 * 2.0;
            Some((score, highlights))
        } else {
            None
        }
    }

    fn generate_trigrams(&self, text: &str) -> Vec<[u8; 3]> {
        generate_trigrams_for(text)
    }

    fn rank_results(&self, results: &mut Vec<SearchResult>, query: &str) {
        let q = to_lowercase(query);

        for result in results.iter_mut() {
            let title_lower = to_lowercase(&result.title);
            if title_lower == q {
                result.score += 20.0;
            } else if title_lower.starts_with(&q) {
                result.score += 10.0;
            }

            match result.entry_type {
                SearchResultType::Application => result.score += 3.0,
                SearchResultType::Setting => result.score += 2.0,
                SearchResultType::RecentDocument => result.score += 4.0,
                SearchResultType::Calculator => result.score += 50.0,
                _ => {}
            }
        }

        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(core::cmp::Ordering::Equal)
        });

        let mut seen = Vec::new();
        results.retain(|r| {
            if seen.contains(&r.title) {
                false
            } else {
                seen.push(r.title.clone());
                true
            }
        });
    }

    fn evaluate_calculator(&self, expr: &str) -> Option<String> {
        let trimmed = expr.trim();
        if trimmed.is_empty() {
            return None;
        }

        let has_operator = trimmed
            .bytes()
            .any(|b| b == b'+' || b == b'-' || b == b'*' || b == b'/');
        if !has_operator {
            return None;
        }

        let valid = trimmed.bytes().all(|b| {
            b.is_ascii_digit()
                || b == b'.'
                || b == b'+'
                || b == b'-'
                || b == b'*'
                || b == b'/'
                || b == b' '
                || b == b'('
                || b == b')'
        });
        if !valid {
            return None;
        }

        let parts: Vec<&str> = trimmed
            .splitn(2, |c: char| c == '+' || c == '-' || c == '*' || c == '/')
            .collect();
        if parts.len() != 2 {
            return None;
        }

        let a = parts[0].trim().parse::<f64>().ok()?;
        let op = trimmed
            .bytes()
            .find(|&b| b == b'+' || b == b'-' || b == b'*' || b == b'/')?;
        let b = parts[1].trim().parse::<f64>().ok()?;

        let result = match op {
            b'+' => a + b,
            b'-' => a - b,
            b'*' => a * b,
            b'/' => {
                if b == 0.0 {
                    return Some(String::from("Error: division by zero"));
                }
                a / b
            }
            _ => return None,
        };

        if result == f64_floor(result) && f64_abs(result) < 1e15 {
            Some(format!("{}", result as i64))
        } else {
            Some(format!("{:.6}", result))
        }
    }

    fn is_excluded(&self, path: &str) -> bool {
        for pattern in &self.exclusions {
            if path.contains(pattern.as_str()) {
                return true;
            }
        }
        false
    }

    fn update_index_size(&mut self) {
        let mut total = 0u64;
        for (_, index) in &self.indices {
            for entry in &index.entries {
                total += entry.title.len() as u64;
                total += entry.content.len() as u64;
                if let Some(ref p) = entry.path {
                    total += p.len() as u64;
                }
            }
            total += (index.inverted.len() * 64) as u64;
            total += (index.trigrams.len() * 16) as u64;
        }
        self.stats.index_size_bytes = total;
    }
}

// ── Free helpers ─────────────────────────────────────────────────────────

fn to_lowercase(s: &str) -> String {
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

fn tokenize_text(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();

    for ch in text.chars() {
        if ch.is_alphanumeric() || ch == '_' {
            current.push(if ch >= 'A' && ch <= 'Z' {
                (ch as u8 + 32) as char
            } else {
                ch
            });
        } else if !current.is_empty() {
            if current.len() >= 2 {
                tokens.push(current.clone());
            }
            current.clear();
        }
    }
    if current.len() >= 2 {
        tokens.push(current);
    }

    tokens
}

fn generate_trigrams_for(text: &str) -> Vec<[u8; 3]> {
    let lower = to_lowercase(text);
    let bytes = lower.as_bytes();
    let mut tris = Vec::new();

    if bytes.len() < 3 {
        return tris;
    }

    for window in bytes.windows(3) {
        if window.iter().all(|&b| b.is_ascii_alphanumeric()) {
            tris.push([window[0], window[1], window[2]]);
        }
    }

    tris
}

fn path_filename(path: &str) -> String {
    let sep = if path.contains('\\') { '\\' } else { '/' };
    path.rsplit(sep).next().unwrap_or(path).into()
}

fn path_extension(path: &str) -> String {
    let name = path_filename(path);
    name.rsplit('.').next().unwrap_or("").into()
}

// ── Levenshtein Edit Distance ────────────────────────────────────────────

fn levenshtein_distance(a: &str, b: &str) -> usize {
    let a_bytes = a.as_bytes();
    let b_bytes = b.as_bytes();
    let a_len = a_bytes.len();
    let b_len = b_bytes.len();

    if a_len == 0 {
        return b_len;
    }
    if b_len == 0 {
        return a_len;
    }

    let mut prev_row: Vec<usize> = (0..=b_len).collect();
    let mut curr_row = vec![0usize; b_len + 1];

    for i in 1..=a_len {
        curr_row[0] = i;
        for j in 1..=b_len {
            let cost = if a_bytes[i - 1] == b_bytes[j - 1] {
                0
            } else {
                1
            };
            let delete = prev_row[j] + 1;
            let insert = curr_row[j - 1] + 1;
            let substitute = prev_row[j - 1] + cost;
            curr_row[j] = min3(delete, insert, substitute);
        }
        core::mem::swap(&mut prev_row, &mut curr_row);
    }
    prev_row[b_len]
}

fn min3(a: usize, b: usize, c: usize) -> usize {
    let m = if a < b { a } else { b };
    if m < c {
        m
    } else {
        c
    }
}

/// Fuzzy match: returns true if any word in `target` has edit distance ≤ max_dist
/// from any word in `query`.
fn fuzzy_match_edit_distance(
    query: &str,
    target: &str,
    max_dist: usize,
) -> Option<(f32, Vec<(usize, usize)>)> {
    let q_lower = to_lowercase(query);
    let t_lower = to_lowercase(target);

    let q_words = tokenize_text(&q_lower);
    let t_words = tokenize_text(&t_lower);

    if q_words.is_empty() || t_words.is_empty() {
        return None;
    }

    let mut total_score = 0.0f32;
    let mut highlights = Vec::new();
    let mut matched = 0usize;

    for qw in &q_words {
        let mut best_dist = usize::MAX;
        let mut best_idx = 0usize;
        let mut best_len = 0usize;

        let mut search_pos = 0usize;
        for tw in &t_words {
            let dist = levenshtein_distance(qw, tw);
            if dist < best_dist {
                best_dist = dist;
                if let Some(pos) = t_lower[search_pos..].find(tw.as_str()) {
                    best_idx = search_pos + pos;
                    best_len = tw.len();
                }
            }
            if let Some(pos) = t_lower[search_pos..].find(tw.as_str()) {
                search_pos = search_pos + pos + tw.len();
            }
        }

        if best_dist <= max_dist {
            matched += 1;
            let proximity = 1.0 - (best_dist as f32 / (max_dist as f32 + 1.0));
            total_score += proximity * 3.0;
            if best_len > 0 {
                highlights.push((best_idx, best_idx + best_len));
            }
        }
    }

    if matched == 0 {
        return None;
    }

    let coverage = matched as f32 / q_words.len() as f32;
    total_score *= coverage;

    Some((total_score, highlights))
}

// ── Filesystem Watcher (Incremental Indexing) ────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FsEventKind {
    Created,
    Modified,
    Deleted,
    Renamed,
}

#[derive(Debug, Clone)]
pub struct FsEvent {
    pub path: String,
    pub kind: FsEventKind,
    pub timestamp: u64,
}

pub struct FsWatcher {
    pub watched_paths: Vec<String>,
    pub pending_events: Vec<FsEvent>,
    pub debounce_ms: u64,
    pub last_process_time: u64,
    pub event_count: u64,
    pub max_batch_size: usize,
}

impl FsWatcher {
    pub fn new() -> Self {
        Self {
            watched_paths: Vec::new(),
            pending_events: Vec::new(),
            debounce_ms: 500,
            last_process_time: 0,
            event_count: 0,
            max_batch_size: 100,
        }
    }

    pub fn watch(&mut self, path: &str) {
        if !self.watched_paths.iter().any(|p| p.as_str() == path) {
            self.watched_paths.push(String::from(path));
        }
    }

    pub fn unwatch(&mut self, path: &str) {
        self.watched_paths.retain(|p| p.as_str() != path);
    }

    pub fn push_event(&mut self, event: FsEvent) {
        self.event_count += 1;

        if let Some(existing) = self
            .pending_events
            .iter_mut()
            .find(|e| e.path == event.path)
        {
            existing.kind = event.kind;
            existing.timestamp = event.timestamp;
        } else {
            self.pending_events.push(event);
        }
    }

    /// Drain ready events (debounced). Returns events older than debounce_ms.
    pub fn drain_ready(&mut self, now: u64) -> Vec<FsEvent> {
        let cutoff = now.saturating_sub(self.debounce_ms);
        let mut ready = Vec::new();
        let mut remaining = Vec::new();

        for event in self.pending_events.drain(..) {
            if event.timestamp <= cutoff {
                ready.push(event);
            } else {
                remaining.push(event);
            }
        }

        if ready.len() > self.max_batch_size {
            let overflow = ready.split_off(self.max_batch_size);
            remaining.extend(overflow);
        }

        self.pending_events = remaining;
        self.last_process_time = now;
        ready
    }

    pub fn pending_count(&self) -> usize {
        self.pending_events.len()
    }

    pub fn is_watching(&self, path: &str) -> bool {
        self.watched_paths
            .iter()
            .any(|p| path.starts_with(p.as_str()))
    }
}

// ── Contact Index ────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ContactEntry {
    pub name: String,
    pub email: Option<String>,
    pub phone: Option<String>,
    pub organization: Option<String>,
    pub avatar: Option<String>,
    pub last_contacted: u64,
    pub frequency: u32,
}

pub struct ContactIndex {
    pub contacts: Vec<ContactEntry>,
}

impl ContactIndex {
    fn new() -> Self {
        Self {
            contacts: Vec::new(),
        }
    }

    pub fn add(&mut self, contact: ContactEntry) {
        if let Some(existing) = self
            .contacts
            .iter_mut()
            .find(|c| c.name == contact.name && c.email == contact.email)
        {
            existing.phone = contact.phone;
            existing.organization = contact.organization;
            existing.frequency = contact.frequency;
        } else {
            self.contacts.push(contact);
        }
    }

    pub fn remove(&mut self, name: &str) {
        self.contacts.retain(|c| c.name.as_str() != name);
    }

    pub fn search(&self, query: &str) -> Vec<(usize, f32)> {
        let query_lower = to_lowercase(query);
        let mut results = Vec::new();

        for (i, contact) in self.contacts.iter().enumerate() {
            let mut score = 0.0f32;

            let name_lower = to_lowercase(&contact.name);
            if name_lower == query_lower {
                score += 10.0;
            } else if name_lower.starts_with(&query_lower) {
                score += 7.0;
            } else if name_lower.contains(&query_lower) {
                score += 4.0;
            } else if let Some((dist_score, _)) =
                fuzzy_match_edit_distance(&query_lower, &name_lower, 2)
            {
                score += dist_score;
            }

            if let Some(ref email) = contact.email {
                let email_lower = to_lowercase(email);
                if email_lower.contains(&query_lower) {
                    score += 3.0;
                }
            }

            if let Some(ref org) = contact.organization {
                if to_lowercase(org).contains(&query_lower) {
                    score += 2.0;
                }
            }

            score += f32_ln(contact.frequency as f32).max(0.0) * 0.3;

            if score > 0.0 {
                results.push((i, score));
            }
        }

        results.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(core::cmp::Ordering::Equal));
        results
    }
}

// ── Bookmark Index ───────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct BookmarkEntry {
    pub title: String,
    pub url: String,
    pub tags: Vec<String>,
    pub folder: Option<String>,
    pub added_at: u64,
    pub visit_count: u32,
}

pub struct BookmarkIndex {
    pub bookmarks: Vec<BookmarkEntry>,
}

impl BookmarkIndex {
    fn new() -> Self {
        Self {
            bookmarks: Vec::new(),
        }
    }

    pub fn add(&mut self, bookmark: BookmarkEntry) {
        if let Some(existing) = self.bookmarks.iter_mut().find(|b| b.url == bookmark.url) {
            existing.title = bookmark.title;
            existing.tags = bookmark.tags;
            existing.visit_count = bookmark.visit_count;
        } else {
            self.bookmarks.push(bookmark);
        }
    }

    pub fn remove_by_url(&mut self, url: &str) {
        self.bookmarks.retain(|b| b.url.as_str() != url);
    }

    pub fn search(&self, query: &str) -> Vec<(usize, f32)> {
        let query_lower = to_lowercase(query);
        let mut results = Vec::new();

        for (i, bm) in self.bookmarks.iter().enumerate() {
            let mut score = 0.0f32;

            let title_lower = to_lowercase(&bm.title);
            if title_lower == query_lower {
                score += 10.0;
            } else if title_lower.starts_with(&query_lower) {
                score += 7.0;
            } else if title_lower.contains(&query_lower) {
                score += 4.0;
            } else if let Some((dist_score, _)) =
                fuzzy_match_edit_distance(&query_lower, &title_lower, 2)
            {
                score += dist_score;
            }

            let url_lower = to_lowercase(&bm.url);
            if url_lower.contains(&query_lower) {
                score += 2.0;
            }

            for tag in &bm.tags {
                if to_lowercase(tag).contains(&query_lower) {
                    score += 3.0;
                    break;
                }
            }

            score += f32_ln(bm.visit_count as f32).max(0.0) * 0.4;

            if score > 0.0 {
                results.push((i, score));
            }
        }

        results.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(core::cmp::Ordering::Equal));
        results
    }
}

// ── Unified Search Engine ────────────────────────────────────────────────

/// High-level search engine wrapping the `SearchIndexer` with additional
/// categories (contacts, bookmarks), edit-distance fuzzy matching, and
/// filesystem-watcher-driven incremental indexing.
pub struct SearchEngine {
    pub indexer: SearchIndexer,
    pub contact_index: ContactIndex,
    pub bookmark_index: BookmarkIndex,
    pub fs_watcher: FsWatcher,
    pub query_latency_us: Vec<u64>,
    pub max_latency_history: usize,
    pub target_latency_us: u64,
}

impl SearchEngine {
    pub fn new() -> Self {
        Self {
            indexer: SearchIndexer::new(),
            contact_index: ContactIndex::new(),
            bookmark_index: BookmarkIndex::new(),
            fs_watcher: FsWatcher::new(),
            query_latency_us: Vec::new(),
            max_latency_history: 100,
            target_latency_us: 100_000,
        }
    }

    /// Unified search across all categories with edit-distance fuzzy matching.
    pub fn search(&mut self, query_text: &str) -> Vec<SearchResult> {
        if query_text.is_empty() {
            return Vec::new();
        }

        let query = SearchQuery::simple(query_text);
        let mut results = self.indexer.search(&query);

        self.search_contacts(query_text, &mut results);
        self.search_bookmarks(query_text, &mut results);

        self.fuzzy_enhance(&mut results, query_text);

        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(core::cmp::Ordering::Equal)
        });

        let mut seen = Vec::new();
        results.retain(|r| {
            if seen.contains(&r.title) {
                false
            } else {
                seen.push(r.title.clone());
                true
            }
        });

        results.truncate(20);
        results
    }

    fn search_contacts(&self, query: &str, results: &mut Vec<SearchResult>) {
        let hits = self.contact_index.search(query);
        for (idx, score) in hits.into_iter().take(5) {
            if let Some(contact) = self.contact_index.contacts.get(idx) {
                let subtitle = contact
                    .email
                    .clone()
                    .or_else(|| contact.phone.clone())
                    .unwrap_or_default();
                results.push(SearchResult {
                    entry_type: SearchResultType::Contact,
                    title: contact.name.clone(),
                    subtitle,
                    icon: contact.avatar.clone(),
                    path: None,
                    score,
                    highlights: Vec::new(),
                    action: SearchAction::Open(contact.name.clone()),
                });
            }
        }
    }

    fn search_bookmarks(&self, query: &str, results: &mut Vec<SearchResult>) {
        let hits = self.bookmark_index.search(query);
        for (idx, score) in hits.into_iter().take(5) {
            if let Some(bm) = self.bookmark_index.bookmarks.get(idx) {
                results.push(SearchResult {
                    entry_type: SearchResultType::Bookmark,
                    title: bm.title.clone(),
                    subtitle: bm.url.clone(),
                    icon: None,
                    path: Some(bm.url.clone()),
                    score,
                    highlights: Vec::new(),
                    action: SearchAction::Navigate(bm.url.clone()),
                });
            }
        }
    }

    /// Re-rank results using edit-distance fuzzy matching for typo tolerance.
    fn fuzzy_enhance(&self, results: &mut Vec<SearchResult>, query: &str) {
        for result in results.iter_mut() {
            if let Some((extra_score, extra_hl)) =
                fuzzy_match_edit_distance(query, &result.title, 2)
            {
                result.score += extra_score * 0.5;
                for hl in extra_hl {
                    if !result.highlights.contains(&hl) {
                        result.highlights.push(hl);
                    }
                }
            }
        }
    }

    /// Process filesystem events and incrementally re-index changed files.
    pub fn process_fs_events(&mut self, now: u64) {
        let events = self.fs_watcher.drain_ready(now);
        if events.is_empty() {
            return;
        }

        for event in &events {
            match event.kind {
                FsEventKind::Created | FsEventKind::Modified => {
                    self.indexer.remove_file(&event.path);
                    let _ = self.indexer.index_file(&event.path);
                }
                FsEventKind::Deleted => {
                    self.indexer.remove_file(&event.path);
                }
                FsEventKind::Renamed => {
                    let _ = self.indexer.index_file(&event.path);
                }
            }
        }
    }

    /// Record a query latency measurement.
    pub fn record_latency(&mut self, latency_us: u64) {
        self.query_latency_us.push(latency_us);
        if self.query_latency_us.len() > self.max_latency_history {
            self.query_latency_us.remove(0);
        }
    }

    pub fn avg_latency_us(&self) -> u64 {
        if self.query_latency_us.is_empty() {
            return 0;
        }
        let sum: u64 = self.query_latency_us.iter().sum();
        sum / self.query_latency_us.len() as u64
    }

    pub fn p99_latency_us(&self) -> u64 {
        if self.query_latency_us.is_empty() {
            return 0;
        }
        let mut sorted = self.query_latency_us.clone();
        sorted.sort();
        let idx = (sorted.len() * 99) / 100;
        sorted[idx.min(sorted.len() - 1)]
    }

    pub fn meets_target(&self) -> bool {
        self.avg_latency_us() <= self.target_latency_us
    }

    pub fn index_contact(&mut self, contact: ContactEntry) {
        let name = contact.name.clone();
        self.contact_index.add(contact);

        let id = self.indexer.next_id;
        self.indexer.next_id += 1;

        let entry = IndexEntry {
            id,
            title: name.clone(),
            content: String::new(),
            path: None,
            entry_type: SearchResultType::Contact,
            metadata: BTreeMap::new(),
            score_boost: 1.5,
            last_accessed: 0,
            access_count: 0,
        };

        let tokens = tokenize_text(&name);
        let trigrams = generate_trigrams_for(&name);

        if let Some(idx) = self.indexer.indices.get_mut("contacts") {
            idx.add_entry(entry, &tokens, &trigrams);
        } else {
            let mut idx = SearchIndex::new("contacts");
            idx.add_entry(entry, &tokens, &trigrams);
            self.indexer.indices.insert(String::from("contacts"), idx);
        }
    }

    pub fn index_bookmark(&mut self, bookmark: BookmarkEntry) {
        let title = bookmark.title.clone();
        let url = bookmark.url.clone();
        self.bookmark_index.add(bookmark);

        let id = self.indexer.next_id;
        self.indexer.next_id += 1;

        let combined = format!("{} {}", title, url);
        let entry = IndexEntry {
            id,
            title: title.clone(),
            content: url,
            path: None,
            entry_type: SearchResultType::Bookmark,
            metadata: BTreeMap::new(),
            score_boost: 1.2,
            last_accessed: 0,
            access_count: 0,
        };

        let tokens = tokenize_text(&combined);
        let trigrams = generate_trigrams_for(&combined);

        if let Some(idx) = self.indexer.indices.get_mut("bookmarks") {
            idx.add_entry(entry, &tokens, &trigrams);
        } else {
            let mut idx = SearchIndex::new("bookmarks");
            idx.add_entry(entry, &tokens, &trigrams);
            self.indexer.indices.insert(String::from("bookmarks"), idx);
        }
    }

    pub fn watch_directory(&mut self, path: &str) -> Result<u64, SearchError> {
        self.fs_watcher.watch(path);
        self.indexer.index_directory(path, true)
    }

    pub fn total_indexed_items(&self) -> u64 {
        let mut total = self.indexer.stats.total_files
            + self.indexer.stats.total_apps
            + self.indexer.stats.total_settings;
        total += self.contact_index.contacts.len() as u64;
        total += self.bookmark_index.bookmarks.len() as u64;
        total
    }

    pub fn rebuild_all(&mut self) {
        self.indexer.rebuild_index();
    }
}

// ── Content-Based File Indexing ──────────────────────────────────────────

/// Tokenizes file content for full-text indexing. Handles common formats
/// by extracting meaningful tokens from the raw content bytes.
pub struct ContentIndexer {
    pub max_content_bytes: usize,
    pub min_token_length: usize,
    pub max_token_length: usize,
    pub stop_words: Vec<String>,
    pub indexed_extensions: Vec<String>,
}

impl ContentIndexer {
    pub fn new() -> Self {
        Self {
            max_content_bytes: 1_048_576,
            min_token_length: 2,
            max_token_length: 64,
            stop_words: Self::default_stop_words(),
            indexed_extensions: Self::default_extensions(),
        }
    }

    fn default_stop_words() -> Vec<String> {
        let words = [
            "the", "a", "an", "is", "are", "was", "were", "be", "been", "being", "have", "has",
            "had", "do", "does", "did", "will", "would", "could", "should", "may", "might",
            "shall", "can", "to", "of", "in", "for", "on", "with", "at", "by", "from", "as",
            "into", "through", "during", "before", "after", "above", "below", "between", "out",
            "off", "over", "under", "again", "further", "then", "once", "it", "its", "this",
            "that", "these", "those", "and", "but", "or", "nor", "not", "so", "if",
        ];
        words.iter().map(|w| String::from(*w)).collect()
    }

    fn default_extensions() -> Vec<String> {
        let exts = [
            "txt", "md", "rs", "py", "js", "ts", "c", "cpp", "h", "java", "go", "rb", "sh", "bat",
            "toml", "yaml", "yml", "json", "xml", "html", "css", "sql", "ini", "cfg", "conf",
            "log",
        ];
        exts.iter().map(|e| String::from(*e)).collect()
    }

    pub fn should_index_content(&self, extension: &str) -> bool {
        self.indexed_extensions
            .iter()
            .any(|e| e.as_str() == extension)
    }

    pub fn extract_tokens(&self, content: &str) -> Vec<String> {
        let limit = content.len().min(self.max_content_bytes);
        let text = &content[..limit];
        let mut tokens = Vec::new();
        let mut current = String::new();

        for ch in text.chars() {
            if ch.is_alphanumeric() || ch == '_' {
                let lower = if ch >= 'A' && ch <= 'Z' {
                    (ch as u8 + 32) as char
                } else {
                    ch
                };
                current.push(lower);

                if current.len() > self.max_token_length {
                    current.clear();
                }
            } else if !current.is_empty() {
                if current.len() >= self.min_token_length
                    && !self
                        .stop_words
                        .iter()
                        .any(|sw| sw.as_str() == current.as_str())
                {
                    tokens.push(current.clone());
                }
                current.clear();
            }
        }

        if current.len() >= self.min_token_length
            && !self
                .stop_words
                .iter()
                .any(|sw| sw.as_str() == current.as_str())
        {
            tokens.push(current);
        }

        tokens.dedup();
        tokens
    }

    pub fn content_summary(&self, content: &str, max_len: usize) -> String {
        let limit = content.len().min(max_len);
        let mut summary = String::new();
        for ch in content[..limit].chars() {
            if ch == '\n' || ch == '\r' {
                summary.push(' ');
            } else {
                summary.push(ch);
            }
        }
        summary
    }
}

// ── As-You-Type Search Input ─────────────────────────────────────────────

/// Search input state for the search bar in the taskbar/start menu.
/// Handles as-you-type query updates, cursor position, and completion hints.
pub struct SearchInputState {
    pub query: String,
    pub cursor_pos: usize,
    pub selection_start: Option<usize>,
    pub history: Vec<String>,
    pub history_index: Option<usize>,
    pub completion_hint: Option<String>,
    pub last_query_time: u64,
    pub debounce_ms: u64,
    pub pending_search: bool,
    pub max_history: usize,
}

impl SearchInputState {
    pub fn new() -> Self {
        Self {
            query: String::new(),
            cursor_pos: 0,
            selection_start: None,
            history: Vec::new(),
            history_index: None,
            completion_hint: None,
            last_query_time: 0,
            debounce_ms: 50,
            pending_search: false,
            max_history: 50,
        }
    }

    pub fn insert_char(&mut self, ch: char, now: u64) {
        if self.cursor_pos <= self.query.len() {
            self.query.insert(self.cursor_pos, ch);
            self.cursor_pos += 1;
        }
        self.mark_changed(now);
    }

    pub fn delete_back(&mut self, now: u64) {
        if self.cursor_pos > 0 {
            self.cursor_pos -= 1;
            self.query.remove(self.cursor_pos);
        }
        self.mark_changed(now);
    }

    pub fn delete_forward(&mut self, now: u64) {
        if self.cursor_pos < self.query.len() {
            self.query.remove(self.cursor_pos);
        }
        self.mark_changed(now);
    }

    pub fn move_left(&mut self) {
        if self.cursor_pos > 0 {
            self.cursor_pos -= 1;
        }
    }

    pub fn move_right(&mut self) {
        if self.cursor_pos < self.query.len() {
            self.cursor_pos += 1;
        }
    }

    pub fn move_home(&mut self) {
        self.cursor_pos = 0;
    }
    pub fn move_end(&mut self) {
        self.cursor_pos = self.query.len();
    }

    pub fn select_all(&mut self) {
        self.selection_start = Some(0);
        self.cursor_pos = self.query.len();
    }

    pub fn clear(&mut self, now: u64) {
        self.query.clear();
        self.cursor_pos = 0;
        self.selection_start = None;
        self.completion_hint = None;
        self.mark_changed(now);
    }

    pub fn set_query(&mut self, text: &str, now: u64) {
        self.query = String::from(text);
        self.cursor_pos = self.query.len();
        self.mark_changed(now);
    }

    pub fn commit_to_history(&mut self) {
        if self.query.is_empty() {
            return;
        }

        self.history.retain(|h| h.as_str() != self.query.as_str());
        self.history.push(self.query.clone());

        if self.history.len() > self.max_history {
            self.history.remove(0);
        }
        self.history_index = None;
    }

    pub fn history_prev(&mut self) {
        if self.history.is_empty() {
            return;
        }
        let idx = match self.history_index {
            Some(i) => i.saturating_sub(1),
            None => self.history.len() - 1,
        };
        self.history_index = Some(idx);
        if let Some(h) = self.history.get(idx) {
            self.query = h.clone();
            self.cursor_pos = self.query.len();
        }
    }

    pub fn history_next(&mut self) {
        if let Some(idx) = self.history_index {
            if idx + 1 < self.history.len() {
                self.history_index = Some(idx + 1);
                if let Some(h) = self.history.get(idx + 1) {
                    self.query = h.clone();
                    self.cursor_pos = self.query.len();
                }
            } else {
                self.history_index = None;
                self.query.clear();
                self.cursor_pos = 0;
            }
        }
    }

    pub fn set_completion(&mut self, hint: Option<String>) {
        self.completion_hint = hint;
    }

    pub fn accept_completion(&mut self, now: u64) {
        if let Some(hint) = self.completion_hint.take() {
            self.query = hint;
            self.cursor_pos = self.query.len();
            self.mark_changed(now);
        }
    }

    pub fn should_search(&self, now: u64) -> bool {
        self.pending_search && now.saturating_sub(self.last_query_time) >= self.debounce_ms
    }

    pub fn consume_pending(&mut self) {
        self.pending_search = false;
    }

    fn mark_changed(&mut self, now: u64) {
        self.last_query_time = now;
        self.pending_search = true;
        self.history_index = None;
    }
}

// ── Search Results Display ───────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchResultCategory {
    TopHit,
    Applications,
    Files,
    Settings,
    Contacts,
    Bookmarks,
    Calculator,
    WebSuggestion,
}

impl SearchResultCategory {
    pub fn from_result_type(rt: SearchResultType) -> Self {
        match rt {
            SearchResultType::Application => Self::Applications,
            SearchResultType::File
            | SearchResultType::Folder
            | SearchResultType::RecentDocument => Self::Files,
            SearchResultType::Setting => Self::Settings,
            SearchResultType::Contact => Self::Contacts,
            SearchResultType::Bookmark => Self::Bookmarks,
            SearchResultType::Calculator => Self::Calculator,
            SearchResultType::WebSuggestion => Self::WebSuggestion,
            _ => Self::Files,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::TopHit => "Top Hit",
            Self::Applications => "Applications",
            Self::Files => "Files",
            Self::Settings => "Settings",
            Self::Contacts => "Contacts",
            Self::Bookmarks => "Bookmarks",
            Self::Calculator => "Calculator",
            Self::WebSuggestion => "Web",
        }
    }
}

/// Groups search results by category for display.
pub fn categorize_results(
    results: &[SearchResult],
) -> Vec<(SearchResultCategory, Vec<&SearchResult>)> {
    let mut categories: Vec<(SearchResultCategory, Vec<&SearchResult>)> = Vec::new();

    if let Some(first) = results.first() {
        if first.score > 15.0 {
            categories.push((SearchResultCategory::TopHit, vec![first]));
        }
    }

    let cat_order = [
        SearchResultCategory::Applications,
        SearchResultCategory::Files,
        SearchResultCategory::Settings,
        SearchResultCategory::Contacts,
        SearchResultCategory::Bookmarks,
        SearchResultCategory::Calculator,
    ];

    for cat in &cat_order {
        let items: Vec<&SearchResult> = results
            .iter()
            .filter(|r| SearchResultCategory::from_result_type(r.entry_type) == *cat)
            .take(5)
            .collect();
        if !items.is_empty() {
            categories.push((*cat, items));
        }
    }

    categories
}

// ── Search Performance Benchmark ─────────────────────────────────────────

pub struct SearchBenchmark {
    pub query_count: u64,
    pub total_latency_us: u64,
    pub max_latency_us: u64,
    pub min_latency_us: u64,
    pub index_size_at_test: u64,
    pub queries_under_target: u64,
    pub target_latency_us: u64,
}

impl SearchBenchmark {
    pub fn new(target_us: u64) -> Self {
        Self {
            query_count: 0,
            total_latency_us: 0,
            max_latency_us: 0,
            min_latency_us: u64::MAX,
            index_size_at_test: 0,
            queries_under_target: 0,
            target_latency_us: target_us,
        }
    }

    pub fn record(&mut self, latency_us: u64) {
        self.query_count += 1;
        self.total_latency_us += latency_us;
        if latency_us > self.max_latency_us {
            self.max_latency_us = latency_us;
        }
        if latency_us < self.min_latency_us {
            self.min_latency_us = latency_us;
        }
        if latency_us <= self.target_latency_us {
            self.queries_under_target += 1;
        }
    }

    pub fn avg_latency_us(&self) -> u64 {
        if self.query_count == 0 {
            return 0;
        }
        self.total_latency_us / self.query_count
    }

    pub fn pass_rate(&self) -> f32 {
        if self.query_count == 0 {
            return 0.0;
        }
        self.queries_under_target as f32 / self.query_count as f32
    }

    pub fn meets_target(&self) -> bool {
        self.pass_rate() >= 0.95
    }

    pub fn reset(&mut self) {
        self.query_count = 0;
        self.total_latency_us = 0;
        self.max_latency_us = 0;
        self.min_latency_us = u64::MAX;
        self.queries_under_target = 0;
    }
}

// ── Damerau-Levenshtein Distance (with transpositions) ──────────────────

fn damerau_levenshtein(a: &str, b: &str) -> usize {
    let a_bytes = a.as_bytes();
    let b_bytes = b.as_bytes();
    let a_len = a_bytes.len();
    let b_len = b_bytes.len();

    if a_len == 0 {
        return b_len;
    }
    if b_len == 0 {
        return a_len;
    }

    let mut d = vec![vec![0usize; b_len + 1]; a_len + 1];

    for i in 0..=a_len {
        d[i][0] = i;
    }
    for j in 0..=b_len {
        d[0][j] = j;
    }

    for i in 1..=a_len {
        for j in 1..=b_len {
            let cost = if a_bytes[i - 1] == b_bytes[j - 1] {
                0
            } else {
                1
            };
            d[i][j] = min3(d[i - 1][j] + 1, d[i][j - 1] + 1, d[i - 1][j - 1] + cost);

            if i > 1
                && j > 1
                && a_bytes[i - 1] == b_bytes[j - 2]
                && a_bytes[i - 2] == b_bytes[j - 1]
            {
                let transposition = d[i - 2][j - 2] + cost;
                if transposition < d[i][j] {
                    d[i][j] = transposition;
                }
            }
        }
    }

    d[a_len][b_len]
}

/// Enhanced fuzzy match using Damerau-Levenshtein (handles transpositions like "teh" → "the").
pub fn fuzzy_match_with_transpositions(
    query: &str,
    target: &str,
    max_dist: usize,
) -> Option<(f32, Vec<(usize, usize)>)> {
    let q = to_lowercase(query);
    let t = to_lowercase(target);

    if t.contains(&q) {
        let start = t.find(&q).unwrap_or(0);
        let end = start + q.len();
        let score = q.len() as f32 / t.len().max(1) as f32 * 8.0;
        return Some((score, vec![(start, end)]));
    }

    let q_words = tokenize_text(&q);
    let t_words = tokenize_text(&t);
    if q_words.is_empty() {
        return None;
    }

    let mut total_score = 0.0f32;
    let mut highlights = Vec::new();
    let mut matched = 0usize;

    for qw in &q_words {
        let mut best_dist = usize::MAX;
        let mut best_start = 0usize;
        let mut best_len = 0usize;

        let mut pos = 0;
        for tw in &t_words {
            let dist = damerau_levenshtein(qw, tw);
            if let Some(found) = t[pos..].find(tw.as_str()) {
                if dist < best_dist {
                    best_dist = dist;
                    best_start = pos + found;
                    best_len = tw.len();
                }
                pos = pos + found + tw.len();
            }
        }

        if best_dist <= max_dist {
            matched += 1;
            let proximity = 1.0 - (best_dist as f32 / (max_dist as f32 + 1.0));
            total_score += proximity * 4.0;
            if best_len > 0 {
                highlights.push((best_start, best_start + best_len));
            }
        }
    }

    if matched == 0 {
        return None;
    }
    let coverage = matched as f32 / q_words.len() as f32;
    total_score *= coverage;
    Some((total_score, highlights))
}
