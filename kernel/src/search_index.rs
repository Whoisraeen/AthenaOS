//! Local-first, in-memory inverted index for the system-wide search bar.
//!
//! Concept §Windows pain points:
//! > "Search is broken → Local-first, indexed, sub-100ms results."
//!
//! Windows Search routinely takes seconds to return results from a local
//! file tree and silently falls back to web search. AthenaOS ships a real
//! inverted index that lives entirely in kernel memory: every registered
//! item turns into a set of tokens, every token maps to a posting list of
//! item ids, and a query is `O(tokens_in_query × avg_posting_len)` with a
//! literal `&&` of `BTreeSet<u64>`s.
//!
//! ## Items
//!
//! An item is anything searchable: a file path, an app name, a setting,
//! a contact, a recent document. The kernel doesn't care — it just stores
//! `(id, kind, display, secondary)` and tokenizes the strings. The actual
//! file/app/setting layer is responsible for re-registering items as they
//! appear and removing them when they go away.
//!
//! ## Syscalls (54–57)
//!
//! | nr | name              | rdi/rsi/rdx/r10                                          | rax |
//! |----|-------------------|-----------------------------------------------------------|----|
//! | 54 | SEARCH_ADD        | rdi=display_ptr, rsi=display_len, rdx=kind                | item id |
//! | 55 | SEARCH_REMOVE     | rdi=item_id                                               | 0/err |
//! | 56 | SEARCH_QUERY      | rdi=q_ptr, rsi=q_len, rdx=out_ptr (id,kind pairs), r10=cap| count |
//! | 57 | SEARCH_STATS      | rdi=out_ptr (u64×4)                                       | 32 |
//!
//! Plus the NAMED-result counterpart, `SYS_SEARCH_QUERY_RESOLVED` (281):
//! serializes resolved hits (name + path + kind + folder flag) so the Files app /
//! command palette can render clickable rows — see [`serialize_resolved`] +
//! docs/SYSCALL_TABLE.md Block 32. Same args as 56; variable-length records.

#![allow(dead_code)]

extern crate alloc;

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;

// ── Item kinds ─────────────────────────────────────────────────────────

#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    App = 1,
    File = 2,
    Setting = 3,
    Contact = 4,
    Document = 5,
    Other = 99,
}

impl Kind {
    fn from_u64(n: u64) -> Self {
        match n {
            1 => Kind::App,
            2 => Kind::File,
            3 => Kind::Setting,
            4 => Kind::Contact,
            5 => Kind::Document,
            _ => Kind::Other,
        }
    }
}

#[derive(Debug, Clone)]
struct Item {
    id: u64,
    kind: Kind,
    /// The tokenized string (what the inverted index is built from). For a file
    /// this is `"<name> <parent>"`; for an app/setting it is the label.
    display: String,
    /// Leaf display name the palette row shows. For non-file items this equals
    /// the label; for crawled files it is the file/folder leaf name only.
    name: String,
    /// Absolute path (the palette `Open` target + row subtitle). Empty for
    /// items that have no filesystem path (apps, settings).
    path: String,
    /// True for a directory entry (palette renders a `Folder` row).
    is_folder: bool,
}

/// Display info resolved from an index id — the richer view the command palette
/// needs (the raw `query` only returns `(id, kind)`). See [`resolve`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemInfo {
    pub name: String,
    pub path: String,
    pub kind: Kind,
    pub is_folder: bool,
}

// ── Index ──────────────────────────────────────────────────────────────

struct Index {
    items: BTreeMap<u64, Item>,
    /// token -> set of item ids that contain that token
    postings: BTreeMap<String, BTreeSet<u64>>,
    /// rolling stats for /proc/raeen/search
    queries_total: u64,
    last_query_cycles: u64,
    best_query_cycles: u64,
    worst_query_cycles: u64,
    /// number of hits returned by the most recent query
    last_query_hits: u64,
}

impl Index {
    fn new() -> Self {
        Self {
            items: BTreeMap::new(),
            postings: BTreeMap::new(),
            queries_total: 0,
            last_query_cycles: 0,
            best_query_cycles: u64::MAX,
            worst_query_cycles: 0,
            last_query_hits: 0,
        }
    }

    /// Add a label-only item (app/setting): name == display, no path.
    fn add(&mut self, id: u64, kind: Kind, display: &str) {
        self.add_full(id, kind, display, display, "", false);
    }

    /// Add a fully-described item. `display` is what gets tokenized (the inverted
    /// index source — typically `"<name> <parent>"` for a file); `name`/`path`
    /// are stored verbatim so [`Index::resolve`] can return rich display info to
    /// the command palette.
    fn add_full(
        &mut self,
        id: u64,
        kind: Kind,
        display: &str,
        name: &str,
        path: &str,
        is_folder: bool,
    ) {
        for tok in tokenize(display) {
            self.postings
                .entry(tok)
                .or_insert_with(BTreeSet::new)
                .insert(id);
        }
        self.items.insert(
            id,
            Item {
                id,
                kind,
                display: String::from(display),
                name: String::from(name),
                path: String::from(path),
                is_folder,
            },
        );
    }

    /// Resolve an id to its display info (name, path, kind, is_folder). Returns
    /// `None` if the id was removed or never existed.
    fn resolve(&self, id: u64) -> Option<ItemInfo> {
        self.items.get(&id).map(|it| ItemInfo {
            name: it.name.clone(),
            path: it.path.clone(),
            kind: it.kind,
            is_folder: it.is_folder,
        })
    }

    fn remove(&mut self, id: u64) -> bool {
        let item = match self.items.remove(&id) {
            Some(i) => i,
            None => return false,
        };
        for tok in tokenize(&item.display) {
            if let Some(set) = self.postings.get_mut(&tok) {
                set.remove(&id);
                if set.is_empty() {
                    self.postings.remove(&tok);
                }
            }
        }
        true
    }

    /// Returns up to `max` item ids matching all tokens in `query` (AND
    /// semantics). Empty query → empty result.
    fn query(&mut self, query: &str, max: usize) -> Vec<(u64, Kind)> {
        let toks: Vec<String> = tokenize(query);
        if toks.is_empty() {
            return Vec::new();
        }

        let start = read_tsc();

        // For each query token, compute the union of postings for every
        // stored token that *starts with* it. This gives a "search as you
        // type" feel: typing "calc" matches "calculator", "term" matches
        // "terminal", etc. — without the user having to type the whole
        // word like Windows Search demands.
        let token_sets: Vec<BTreeSet<u64>> = toks
            .iter()
            .map(|q| {
                let mut union: BTreeSet<u64> = BTreeSet::new();
                for (k, v) in self.postings.range(q.clone()..) {
                    if !k.starts_with(q.as_str()) {
                        break;
                    }
                    for id in v {
                        union.insert(*id);
                    }
                }
                union
            })
            .collect();

        // Walk smallest-set-first so the intersection shrinks fast.
        let mut sorted_sets: Vec<&BTreeSet<u64>> = token_sets.iter().collect();
        sorted_sets.sort_by_key(|s| s.len());

        let mut candidates: BTreeSet<u64> = match sorted_sets.first() {
            Some(first) if !first.is_empty() => (*first).clone(),
            _ => return Vec::new(),
        };
        for set in sorted_sets.iter().skip(1) {
            candidates = candidates.intersection(*set).copied().collect();
            if candidates.is_empty() {
                break;
            }
        }

        let results: Vec<(u64, Kind)> = candidates
            .iter()
            .take(max)
            .filter_map(|id| self.items.get(id).map(|it| (*id, it.kind)))
            .collect();

        let elapsed = read_tsc().saturating_sub(start);
        self.queries_total += 1;
        self.last_query_cycles = elapsed;
        self.best_query_cycles = self.best_query_cycles.min(elapsed);
        self.worst_query_cycles = self.worst_query_cycles.max(elapsed);
        self.last_query_hits = results.len() as u64;

        results
    }
}

fn read_tsc() -> u64 {
    let lo: u32;
    let hi: u32;
    unsafe {
        core::arch::asm!(
            "rdtsc",
            out("eax") lo, out("edx") hi,
            options(nomem, nostack, preserves_flags),
        );
    }
    ((hi as u64) << 32) | (lo as u64)
}

/// Lowercase-ascii unicode-permissive token splitter. Splits on every byte
/// that isn't alphanumeric, lowercases, drops tokens shorter than 2 chars.
fn tokenize(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            cur.push(c.to_ascii_lowercase());
        } else if !cur.is_empty() {
            if cur.len() >= 2 {
                out.push(core::mem::take(&mut cur));
            }
            cur.clear();
        }
    }
    if cur.len() >= 2 {
        out.push(cur);
    }
    out
}

// ── Singleton ──────────────────────────────────────────────────────────

static INDEX: Mutex<Option<Index>> = Mutex::new(None);
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// RAII guard returned by [`lock_index`]: holds the `INDEX` spin lock with
/// interrupts disabled for the whole critical section, restoring the previous
/// interrupt state on drop.
///
/// SINGLE-CPU IF=0 DEADLOCK GUARD (mirrors `compositor::CompositorGuard`,
/// root-caused on iron 2026-06-15). `INDEX` is shared between the IF=0 syscall
/// path (`sys_search_add/remove/query/stats` — syscalls run with `RFLAGS.IF=0`,
/// SFMASK clears it on SYSCALL entry, see syscall.rs:156) AND the preemptible
/// crawl thread on CPU0 (`crawl_session_home` -> `add_item`, spawned from
/// `shell_runner::activate_desktop`). On this kernel only the BSP schedules
/// post-boot (APs halt — scheduler::ap_enter_idle), so a spinning IF=0 waiter
/// can NEVER be preempted: if the crawl thread were preempted while holding a
/// raw `INDEX.lock()`, a search syscall on CPU0 would spin on `.lock()` forever
/// because the crawl holder could never resume — a hard hang. Disabling
/// interrupts for the entire hold makes every critical section atomic w.r.t.
/// every other, so a waiter always finds the lock free. Same precedent and
/// rationale as `lock_compositor()`.
struct IndexGuard {
    guard: Option<spin::MutexGuard<'static, Option<Index>>>,
    was_enabled: bool,
}

impl core::ops::Deref for IndexGuard {
    type Target = Option<Index>;
    fn deref(&self) -> &Self::Target {
        self.guard.as_ref().unwrap()
    }
}

impl core::ops::DerefMut for IndexGuard {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.guard.as_mut().unwrap()
    }
}

impl Drop for IndexGuard {
    fn drop(&mut self) {
        // Release the spin lock FIRST, then restore interrupts — never the
        // reverse, or an IRQ between unlock and re-enable could observe a
        // half-torn state.
        self.guard = None;
        if self.was_enabled {
            x86_64::instructions::interrupts::enable();
        }
    }
}

/// Acquire `INDEX` with interrupts disabled. Use this everywhere instead of
/// `INDEX.lock()` — see [`IndexGuard`] for why (single-CPU IF=0 deadlock
/// avoidance, mirroring `compositor::lock_compositor`).
#[inline]
fn lock_index() -> IndexGuard {
    let was_enabled = x86_64::instructions::interrupts::are_enabled();
    x86_64::instructions::interrupts::disable();
    IndexGuard {
        guard: Some(INDEX.lock()),
        was_enabled,
    }
}

/// Seed the index with system apps + settings keys. Mirrors what the start
/// menu and the Settings sidebar advertise, so the search bar can be
/// useful before VFS / userspace start registering items themselves.
pub fn init() {
    let mut idx = Index::new();
    let seed: &[(Kind, &str)] = &[
        // Apps
        (Kind::App, "Terminal"),
        (Kind::App, "Calculator"),
        (Kind::App, "File Manager Files"),
        (Kind::App, "Settings"),
        (Kind::App, "AthPlay games"),
        (Kind::App, "Text Editor Notepad"),
        (Kind::App, "Hello Window"),
        (Kind::App, "Task Manager"),
        (Kind::App, "Camera"),
        // Settings keys
        (Kind::Setting, "System Device name"),
        (Kind::Setting, "System Game Mode"),
        (Kind::Setting, "Display Resolution"),
        (Kind::Setting, "Display Refresh rate"),
        (Kind::Setting, "Display HDR"),
        (Kind::Setting, "Display Night light"),
        (Kind::Setting, "Display Brightness"),
        (Kind::Setting, "Sound Master volume"),
        (Kind::Setting, "Sound Output device"),
        (Kind::Setting, "Sound Mute"),
        (Kind::Setting, "Network Wi-Fi"),
        (Kind::Setting, "Network DNS"),
        (Kind::Setting, "Network AthGuard firewall"),
        (Kind::Setting, "Network VPN WireGuard"),
        (Kind::Setting, "Personalization Theme"),
        (Kind::Setting, "Personalization Accent color"),
        (Kind::Setting, "Personalization Vibe Mode"),
        (Kind::Setting, "Personalization Glassmorphism"),
        (Kind::Setting, "Personalization Animations"),
        (Kind::Setting, "Power profile"),
        (Kind::Setting, "Power Sleep after"),
        (Kind::Setting, "Privacy Telemetry"),
        (Kind::Setting, "Privacy Location"),
        (Kind::Setting, "Privacy Camera"),
        (Kind::Setting, "Privacy Microphone"),
        (Kind::Setting, "About OS version kernel"),
    ];
    for (kind, display) in seed {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        idx.add(id, *kind, display);
    }
    let n = idx.items.len();
    let postings = idx.postings.len();
    *lock_index() = Some(idx);
    crate::serial_println!(
        "[ OK ] Search index: {} items, {} unique tokens (target <100ms / query)",
        n,
        postings,
    );
}

/// Register an item directly from kernel code (the AthFS crawler, the app
/// registry, etc.) without going through the user-pointer syscall path.
/// Returns the assigned item id, or `u64::MAX` if the index isn't up yet.
pub fn add_item(kind: Kind, display: &str) -> u64 {
    if display.is_empty() {
        return u64::MAX;
    }
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let mut g = lock_index();
    match g.as_mut() {
        Some(idx) => {
            idx.add(id, kind, display);
            id
        }
        None => u64::MAX,
    }
}

/// Register a filesystem entry with its name + path retained so the command
/// palette can resolve a query hit back to a rich row (name, path, folder flag).
/// `display` is the tokenized string (`"<name> <parent>"`), `name` is the leaf
/// shown in the row, `path` is the absolute open target. Returns the new id, or
/// `u64::MAX` if the index isn't up. Acquires `INDEX` via [`lock_index`] (IF=0).
pub fn add_item_full(kind: Kind, display: &str, name: &str, path: &str, is_folder: bool) -> u64 {
    if display.is_empty() {
        return u64::MAX;
    }
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let mut g = lock_index();
    match g.as_mut() {
        Some(idx) => {
            idx.add_full(id, kind, display, name, path, is_folder);
            id
        }
        None => u64::MAX,
    }
}

/// Run a query and resolve each hit to full display info in a SINGLE `lock_index`
/// critical section — this is the exact path the command palette's kernel file
/// source uses (`shell_runner::populate_command_palette`). Bounded by `max`.
///
/// One lock acquisition for query+resolve avoids re-locking per id (and the
/// associated IF toggle churn) and guarantees the resolved items can't be
/// removed between the query and the resolve.
pub fn query_resolved(query: &str, max: usize) -> Vec<ItemInfo> {
    if max == 0 {
        return Vec::new();
    }
    let mut g = lock_index();
    let idx = match g.as_mut() {
        Some(i) => i,
        None => return Vec::new(),
    };
    let hits = idx.query(query, max);
    let mut out = Vec::with_capacity(hits.len());
    for (id, _kind) in hits {
        if let Some(info) = idx.resolve(id) {
            out.push(info);
        }
    }
    out
}

/// Kind discriminant as the on-the-wire `u32` tag (mirrors
/// `rae_abi::SEARCH_KIND_*`). Used by [`serialize_resolved`].
#[inline]
fn kind_tag(k: Kind) -> u32 {
    k as u32
}

/// Truncate `s` to at most `max` bytes on a UTF-8 char boundary. Never splits a
/// multi-byte char (so the bytes we emit are always valid UTF-8).
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

/// Serialize the resolved hits for `query` into the documented
/// `SYS_SEARCH_QUERY_RESOLVED` (281) wire format: back-to-back records, each a
/// 24-byte [`rae_abi::SearchResolvedHeader`] followed by `name_len` UTF-8 name
/// bytes then `path_len` UTF-8 path bytes. Returns `(bytes, count)`.
///
/// Only WHOLE records that fit in `out_cap_bytes` are emitted (a partial
/// trailing record is never produced), the count is capped at
/// [`rae_abi::syscall::SEARCH_RESOLVED_MAX_RESULTS`], and each name/path is
/// truncated to [`rae_abi::syscall::SEARCH_RESOLVED_MAX_STR`] on a char boundary.
/// PURE serialization over the resolved `ItemInfo` list — the syscall arm
/// `copy_to_user`s the returned bytes. The id is not carried by `ItemInfo`
/// (the palette opens by path, not id), so the header `id` is `0` here; a future
/// resolver that surfaces the id fills it without a format change.
pub fn serialize_resolved(query: &str, out_cap_bytes: usize) -> (Vec<u8>, usize) {
    let max_results = rae_abi::syscall::SEARCH_RESOLVED_MAX_RESULTS;
    let max_str = rae_abi::syscall::SEARCH_RESOLVED_MAX_STR;
    let header_size = rae_abi::syscall::SEARCH_RESOLVED_HEADER_SIZE;

    let hits = query_resolved(query, max_results);
    let mut out: Vec<u8> = Vec::new();
    let mut count = 0usize;

    for info in hits.iter() {
        if count >= max_results {
            break;
        }
        let name = truncate_on_char_boundary(&info.name, max_str);
        let path = truncate_on_char_boundary(&info.path, max_str);
        let rec_len = header_size + name.len() + path.len();
        // Only emit a record that fits WHOLE in the remaining capacity.
        if out.len() + rec_len > out_cap_bytes {
            break;
        }
        // Header (little-endian, matches rae_abi::SearchResolvedHeader layout).
        out.extend_from_slice(&0u64.to_le_bytes()); // id (ItemInfo carries none)
        out.extend_from_slice(&kind_tag(info.kind).to_le_bytes()); // kind
        out.push(if info.is_folder { 1 } else { 0 }); // is_folder
        out.push(0u8); // reserved0
        out.extend_from_slice(&0u16.to_le_bytes()); // reserved1
        out.extend_from_slice(&(name.len() as u16).to_le_bytes()); // name_len
        out.extend_from_slice(&(path.len() as u16).to_le_bytes()); // path_len
        out.extend_from_slice(&0u32.to_le_bytes()); // reserved2
                                                    // Payload: name then path (UTF-8, no terminators).
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(path.as_bytes());
        count += 1;
    }

    (out, count)
}

// ── AthFS crawler ──────────────────────────────────────────────────────
//
// Concept §Windows pain points:
// > "Search is broken → Local-first, indexed, sub-100ms results."
//
// The static seed in `init()` covers apps + settings, but a search bar is
// only "local-first" if it actually finds the user's *files*. This walks the
// logged-in session's home (Desktop/Documents/Downloads/Pictures/Music/
// Videos + their first level of children) and feeds each entry into the same
// inverted index, so typing a filename returns the file.
//
// Bounded on purpose: a runaway crawl on a full disk would be both a boot-time
// and a memory hazard (CLAUDE.md §15: keep the boot path light). We cap total
// entries and recursion depth, and the caller runs us POST-marker (after login
// seeds the home tree), never on the boot critical path.

/// Max entries the crawler will register in one pass (memory + latency bound).
const CRAWL_ENTRY_CAP: usize = 4096;
/// Max directory depth below the home root (home=0, its children=1, …).
const CRAWL_DEPTH_CAP: usize = 3;

/// Walk `home` and register every file/folder we find (bounded). Indexes both
/// the leaf name AND a coarse path segment so a query like "downloads report"
/// narrows correctly. Returns the number of entries registered.
///
/// A folder (size==0) is registered as `Kind::File` with its name; a regular
/// file is registered as `Kind::Document`. The `display` string is
/// `"<name> <parent-folder>"` so the parent folder is a searchable token too.
pub fn crawl_dir(home: &str) -> usize {
    let mut registered = 0usize;
    crawl_recursive(home, 0, &mut registered);
    crate::serial_println!(
        "[search] crawl of {} registered {} entries (cap {})",
        home,
        registered,
        CRAWL_ENTRY_CAP,
    );
    registered
}

fn crawl_recursive(path: &str, depth: usize, registered: &mut usize) {
    if depth > CRAWL_DEPTH_CAP || *registered >= CRAWL_ENTRY_CAP {
        return;
    }
    let trimmed = path.trim_end_matches('/');
    let parent_name = trimmed.rsplit('/').next().unwrap_or("");
    for entry in crate::vfs::list_dir_at(path) {
        if *registered >= CRAWL_ENTRY_CAP {
            break;
        }
        if entry.name.starts_with('.') {
            continue;
        }
        let child_path = alloc::format!("{}/{}", trimmed, entry.name);
        // "<name> <parent>" → both the file name and its containing folder are
        // tokens, so "documents resume" and "resume" both hit.
        let display = if parent_name.is_empty() {
            entry.name.clone()
        } else {
            alloc::format!("{} {}", entry.name, parent_name)
        };
        let is_folder = entry.size == 0;
        let kind = if is_folder {
            Kind::File
        } else {
            Kind::Document
        };
        // Retain the leaf name + absolute path so the command palette can resolve
        // a hit to a real `Open` target (search_index::query_resolved -> row).
        if add_item_full(kind, &display, &entry.name, &child_path, is_folder) != u64::MAX {
            *registered += 1;
        }
        // Recurse into folders (size==0).
        if entry.size == 0 {
            crawl_recursive(&child_path, depth + 1, registered);
        }
    }
}

/// Post-login entry point: crawl the active session's home into the index.
/// Called from `shell_runner::activate_desktop` AFTER `ensure_session_home_dirs`
/// so the standard folders exist. Off the boot critical path by construction
/// (login already happened, success marker is long printed).
pub fn crawl_session_home() {
    let home = crate::session::home_dir();
    let n = crawl_dir(&home);
    let s = stats();
    crate::serial_println!(
        "[search] session home crawled: +{} entries, index now {} items / {} tokens",
        n,
        s.items,
        s.tokens,
    );
}

// ── Helpers used by procfs ─────────────────────────────────────────────

#[derive(Debug, Clone, Copy, Default)]
pub struct Stats {
    pub items: u64,
    pub tokens: u64,
    pub queries_total: u64,
    pub last_query_cycles: u64,
    pub best_query_cycles: u64,
    pub worst_query_cycles: u64,
    pub last_query_hits: u64,
}

pub fn stats() -> Stats {
    let g = lock_index();
    match g.as_ref() {
        Some(i) => Stats {
            items: i.items.len() as u64,
            tokens: i.postings.len() as u64,
            queries_total: i.queries_total,
            last_query_cycles: i.last_query_cycles,
            best_query_cycles: if i.best_query_cycles == u64::MAX {
                0
            } else {
                i.best_query_cycles
            },
            worst_query_cycles: i.worst_query_cycles,
            last_query_hits: i.last_query_hits,
        },
        None => Stats::default(),
    }
}

// ── Syscall surface ────────────────────────────────────────────────────

pub const SYS_SEARCH_ADD: u64 = 54;
pub const SYS_SEARCH_REMOVE: u64 = 55;
pub const SYS_SEARCH_QUERY: u64 = 56;
pub const SYS_SEARCH_STATS: u64 = 57;

pub fn sys_search_add(
    display_ptr: u64,
    display_len: u64,
    kind: u64,
    validate: impl Fn(u64, u64, bool) -> bool,
) -> u64 {
    if !validate(display_ptr, display_len, false) {
        return u64::MAX;
    }
    let display = read_user_string(display_ptr, display_len);
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let mut g = lock_index();
    if let Some(idx) = g.as_mut() {
        idx.add(id, Kind::from_u64(kind), &display);
        id
    } else {
        u64::MAX
    }
}

pub fn sys_search_remove(id: u64) -> u64 {
    let mut g = lock_index();
    match g.as_mut() {
        Some(idx) => {
            if idx.remove(id) {
                0
            } else {
                u64::MAX
            }
        }
        None => u64::MAX,
    }
}

/// Output layout per result: `[u64 id][u32 kind][u32 padding]` = 16 bytes.
/// rax = number of results written.
pub fn sys_search_query(
    q_ptr: u64,
    q_len: u64,
    out_ptr: u64,
    out_cap_bytes: u64,
    validate_r: impl Fn(u64, u64, bool) -> bool,
    validate_w: impl Fn(u64, u64, bool) -> bool,
) -> u64 {
    if !validate_r(q_ptr, q_len, false) {
        return 0;
    }
    if out_cap_bytes > 0 && !validate_w(out_ptr, out_cap_bytes, true) {
        return 0;
    }
    let query = read_user_string(q_ptr, q_len);
    let max = (out_cap_bytes / 16) as usize;

    let mut g = lock_index();
    let results: Vec<(u64, Kind)> = match g.as_mut() {
        Some(idx) => idx.query(&query, max),
        None => return 0,
    };

    // SMAP-safe: assemble kernel-side, one validated extable copy-out.
    let mut out: Vec<u8> = Vec::with_capacity(results.len() * 16);
    for (id, kind) in &results {
        out.extend_from_slice(&id.to_le_bytes());
        out.extend_from_slice(&(*kind as u32).to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
    }
    if crate::uaccess::copy_to_user(out_ptr, &out).is_err() {
        return 0;
    }
    results.len() as u64
}

/// Writes 32 bytes (4 × u64): items, tokens, queries_total, last_query_cycles.
/// rax = bytes written.
pub fn sys_search_stats(
    out_ptr: u64,
    out_cap: u64,
    validate_w: impl Fn(u64, u64, bool) -> bool,
) -> u64 {
    if out_cap < 32 {
        return 0;
    }
    if !validate_w(out_ptr, 32, true) {
        return 0;
    }
    let s = stats();
    // SMAP-safe: kernel-side pack + one validated extable copy-out.
    let mut buf = [0u8; 32];
    buf[0..8].copy_from_slice(&s.items.to_le_bytes());
    buf[8..16].copy_from_slice(&s.tokens.to_le_bytes());
    buf[16..24].copy_from_slice(&s.queries_total.to_le_bytes());
    buf[24..32].copy_from_slice(&s.last_query_cycles.to_le_bytes());
    if crate::uaccess::copy_to_user(out_ptr, &buf).is_err() {
        return 0;
    }
    32
}

fn read_user_string(ptr: u64, len: u64) -> String {
    // Validated + fault-fixup via the single uaccess chokepoint (was a raw
    // unvalidated copy_nonoverlapping).
    crate::uaccess::read_user_string(ptr, len)
}

// ── Boot smoketest ─────────────────────────────────────────────────────
//
// Run a couple of representative queries during init so the boot log shows
// the real measured latency (proves the <100ms claim with numbers, not
// adjectives).

pub fn run_boot_smoketest() {
    let test_queries: &[&str] = &[
        "calc",      // → Calculator
        "display",   // → multiple display settings
        "wifi",      // → Network Wi-Fi
        "vibe mode", // → Personalization Vibe Mode
        "term",      // → Terminal
    ];
    let cyc_per_ms = cycles_per_ms();

    {
        let mut g = lock_index();
        let idx = match g.as_mut() {
            Some(x) => x,
            None => {
                crate::serial_println!("[search] SMOKETEST FAIL: index not initialized");
                return;
            }
        };

        for q in test_queries {
            let start = read_tsc();
            let hits = idx.query(q, 32);
            let elapsed = read_tsc().saturating_sub(start);
            let us = if cyc_per_ms == 0 {
                0
            } else {
                (elapsed * 1000) / cyc_per_ms
            };
            crate::serial_println!(
                "[search] q=\"{}\" hits={} elapsed={} cycles ({} us)",
                q,
                hits.len(),
                elapsed,
                us,
            );
        }
    }

    // ── FAIL-able crawl→query proof ────────────────────────────────────
    //
    // Seed a synthetic set of file entries through the SAME crawl path the
    // AthFS crawler uses (`add_item`), then query for a distinctive term and
    // assert the exact hit. An empty index or a broken query returns 0 hits →
    // FAIL. This proves the crawler→index→query pipe end-to-end without
    // depending on a logged-in session (the live home crawl runs post-login).
    const PROBE: &str = "raeenzzz_searchprobe_quux";
    const PROBE_NAME: &str = "raeenzzz_searchprobe_quux.txt";
    const PROBE_PATH: &str = "/home/raeen/Documents/raeenzzz_searchprobe_quux.txt";
    // Seed the probe entry through the SAME path the crawler uses (add_item_full),
    // so the resolver sees the exact name/path/folder fields the palette will.
    let before = stats().items;
    add_item_full(
        Kind::Document,
        "raeenzzz_searchprobe_quux Documents",
        PROBE_NAME,
        PROBE_PATH,
        false,
    );
    // A folder entry to prove is_folder resolves correctly too.
    add_item_full(
        Kind::File,
        "Vacation_Photos Pictures",
        "Vacation_Photos",
        "/home/raeen/Pictures/Vacation_Photos",
        true,
    );
    add_item(Kind::Document, "Budget_2026 Documents");
    let seeded = 3usize;

    // ── The EXACT palette path: query_resolved (query -> resolve -> ItemInfo). ──
    // This is what shell_runner's kernel file source calls. Assert the resolved
    // hit carries the right name + path + folder flag — a broken resolver that
    // returned the wrong/empty name or path makes this FAIL.
    let resolved = query_resolved(PROBE, 32);
    let probe_hits = resolved.len();
    let resolve_ok = resolved.len() == 1
        && resolved[0].name == PROBE_NAME
        && resolved[0].path == PROBE_PATH
        && resolved[0].kind == Kind::Document
        && !resolved[0].is_folder;

    // Folder hit resolves with is_folder=true and the right name.
    let folder = query_resolved("vacation", 32);
    let photos_hits = folder.len();
    let folder_ok = folder.len() == 1
        && folder[0].name == "Vacation_Photos"
        && folder[0].is_folder
        && folder[0].path == "/home/raeen/Pictures/Vacation_Photos";

    let after = stats().items;

    // ── FAIL-able SYS_SEARCH_QUERY_RESOLVED (281) wire round-trip ──────────
    //
    // Serialize the probe hit through the EXACT encoder the syscall arm uses
    // (serialize_resolved → the rae_abi::SearchResolvedHeader wire format), then
    // decode it back here with a defensive walk and assert the name/path/kind/
    // folder-flag survive the round-trip. A broken encoder (wrong offsets, bad
    // length fields, truncated payload) makes this FAIL — it proves the kernel
    // half of the contract the raekit host KAT proves on the decoder half.
    let (wire, wire_count) = serialize_resolved(PROBE, 4096);
    let decoded = decode_resolved_for_test(&wire, wire_count);
    let wire_ok = wire_count == 1
        && decoded.len() == 1
        && decoded[0].0 == PROBE_NAME
        && decoded[0].1 == PROBE_PATH
        && decoded[0].2 == kind_tag(Kind::Document)
        && !decoded[0].3;
    // A capacity that fits ZERO whole records must emit zero (never a partial
    // trailing record) — the bound check is load-bearing for copy_to_user safety.
    let (tiny, tiny_count) = serialize_resolved(PROBE, 8);
    let tiny_ok = tiny_count == 0 && tiny.is_empty();

    let pass = resolve_ok
        && folder_ok
        && probe_hits == 1
        && photos_hits == 1
        && wire_ok
        && tiny_ok
        && after == before + seeded as u64;
    crate::serial_println!(
        "[search] crawl-query+resolve smoketest: probe_hits={} (want 1) resolve_ok={} \
         vacation_hits={} folder_ok={} wire_count={} wire_ok={} tiny_count={} tiny_ok={} \
         items {}->{} (+{}) -> {}",
        probe_hits,
        resolve_ok,
        photos_hits,
        folder_ok,
        wire_count,
        wire_ok,
        tiny_count,
        tiny_ok,
        before,
        after,
        seeded,
        if pass { "PASS" } else { "FAIL" },
    );
}

/// Defensive decoder used ONLY by the boot smoketest to verify the
/// `serialize_resolved` wire output round-trips. Mirrors the raekit decoder's
/// bounds discipline: clamp the walk to `count`, bounds-check every header +
/// payload length against the remaining buffer, and stop (never panic) on a
/// short/garbage record. Returns `(name, path, kind, is_folder)` per record.
fn decode_resolved_for_test(buf: &[u8], count: usize) -> Vec<(String, String, u32, bool)> {
    let header = rae_abi::syscall::SEARCH_RESOLVED_HEADER_SIZE;
    let mut out = Vec::new();
    let mut off = 0usize;
    for _ in 0..count {
        if off + header > buf.len() {
            break;
        }
        let kind = u32::from_le_bytes([buf[off + 8], buf[off + 9], buf[off + 10], buf[off + 11]]);
        let is_folder = buf[off + 12] != 0;
        let name_len = u16::from_le_bytes([buf[off + 16], buf[off + 17]]) as usize;
        let path_len = u16::from_le_bytes([buf[off + 18], buf[off + 19]]) as usize;
        let name_start = off + header;
        let path_start = name_start + name_len;
        let rec_end = path_start + path_len;
        if rec_end > buf.len() {
            break;
        }
        let name = String::from_utf8_lossy(&buf[name_start..path_start]).into_owned();
        let path = String::from_utf8_lossy(&buf[path_start..rec_end]).into_owned();
        out.push((name, path, kind, is_folder));
        off = rec_end;
    }
    out
}

fn cycles_per_ms() -> u64 {
    // Use the inverse helper: ns_to_tsc(1_000_000) = cycles per millisecond.
    let g = crate::timers::TIMER_SUBSYSTEM.lock();
    match g.as_ref() {
        Some(ts) => ts.tsc.ns_to_tsc(1_000_000),
        None => 0,
    }
}
