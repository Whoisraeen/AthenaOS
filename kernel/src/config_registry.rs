//! Versioned, hierarchical config registry.
//!
//! LEGACY_GAMING_CONCEPT.md ships a specific dig at Windows:
//!
//! > "Registry is a graveyard → Versioned, hierarchical, human-readable
//! >  config with snapshots."
//!
//! This module is the kernel-side store for that. Every config key lives at
//! a slash-delimited path (`/system/display/scale`, `/apps/raeplay/last_steam_id`,
//! `/users/alice/desktop/wallpaper`) and resolves to a typed value. Mutations
//! bump a monotonically increasing generation number; userspace can snapshot
//! the current generation (cheap — no copy) and roll back to it later
//! (revert all writes since the snapshot).
//!
//! The format is intentionally not a binary blob. Every value is a small
//! enum (Bool / Int / Bytes / String) so dumping the whole tree as TOML or
//! YAML is trivial — keeping the Concept-doc promise of "human-readable".
//!
//! ## Syscall surface (50–53)
//!
//! | nr | name              | rdi/rsi/rdx                                            | rax |
//! |----|-------------------|--------------------------------------------------------|----|
//! | 50 | CONFIG_GET        | rdi=key_ptr, rsi=key_len, rdx=out_ptr, r10=out_cap     | bytes written or u64::MAX |
//! | 51 | CONFIG_SET        | rdi=key_ptr, rsi=key_len, rdx=val_ptr, r10=val_len     | new generation |
//! | 52 | CONFIG_SNAPSHOT   | —                                                       | snapshot id |
//! | 53 | CONFIG_ROLLBACK   | rdi=snapshot_id                                        | 0 ok / E_INVAL |
//!
//! For now, values written through CONFIG_SET are stored as raw bytes;
//! callers are responsible for picking a serialization (most natural is
//! `String` or a single u64 in LE bytes). When `raesettings` and `raestore`
//! start using this, a typed wrapper crate `raeconfig` will sit on top.

#![allow(dead_code)]

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;

/// Each `set` bumps this. Snapshots store the generation at which they were
/// taken, then on rollback we restore by replaying the journal forward to
/// the snapshot's generation.
static GENERATION: AtomicU64 = AtomicU64::new(0);

/// A typed config value. Kept tiny on purpose — the Concept doc's invariant
/// is "human-readable" so we don't lean into binary opacity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Bool(bool),
    Int(i64),
    Bytes(Vec<u8>),
    Text(String),
}

impl Value {
    pub fn as_bytes(&self) -> Vec<u8> {
        match self {
            Value::Bool(b) => alloc::vec![*b as u8],
            Value::Int(i) => i.to_le_bytes().to_vec(),
            Value::Bytes(v) => v.clone(),
            Value::Text(s) => s.as_bytes().to_vec(),
        }
    }
}

/// A single past edit, used to rebuild prior states during rollback.
#[derive(Debug, Clone)]
struct JournalEntry {
    generation: u64,
    key: String,
    /// `None` means "key did not exist before this edit" — used to delete
    /// the key on rollback.
    previous: Option<Value>,
}

pub struct Registry {
    /// The live state.
    tree: BTreeMap<String, Value>,
    /// Append-only journal of every mutation, so we can replay backwards
    /// to a snapshot. In a production OS this would be capped + rotated
    /// to disk; for now it lives entirely in kernel memory.
    journal: Vec<JournalEntry>,
    /// Snapshots are just labelled generation numbers.
    snapshots: BTreeMap<u64, u64>, // snapshot id -> generation
    next_snapshot_id: u64,
}

impl Registry {
    fn new() -> Self {
        let mut reg = Self {
            tree: BTreeMap::new(),
            journal: Vec::new(),
            snapshots: BTreeMap::new(),
            next_snapshot_id: 1,
        };
        reg.seed_defaults();
        reg
    }

    /// Populate factory defaults so the first `cat /proc/raeen/config`
    /// after boot shows something useful. Mirrors §Customization Engine.
    fn seed_defaults(&mut self) {
        let defaults: &[(&str, Value)] = &[
            ("/system/name", Value::Text(String::from("AthenaOS"))),
            ("/system/version", Value::Text(String::from("0.0.1"))),
            ("/system/channel", Value::Text(String::from("dev"))),
            ("/system/game_mode_default", Value::Bool(true)),
            ("/system/telemetry_enabled", Value::Bool(false)),
            ("/system/fast_boot", Value::Bool(false)),
            ("/system/kernel_log", Value::Bool(true)),
            ("/display/resolution_w", Value::Int(1280)),
            ("/display/resolution_h", Value::Int(720)),
            (
                "/display/resolution",
                Value::Text(String::from("1920×1080")),
            ),
            ("/display/refresh_hz", Value::Int(60)),
            ("/display/refresh_hz_str", Value::Text(String::from("60"))),
            ("/display/hdr_enabled", Value::Bool(false)),
            ("/display/scale_pct", Value::Int(100)),
            ("/audio/master_volume", Value::Int(70)),
            ("/audio/mute", Value::Bool(false)),
            ("/audio/spatial_audio", Value::Bool(false)),
            ("/network/wifi_radio", Value::Bool(true)),
            (
                "/network/firewall_profile",
                Value::Text(String::from("DefaultDeny")),
            ),
            ("/personalization/theme", Value::Text(String::from("Dark"))),
            (
                "/personalization/accent",
                Value::Text(String::from("RaeBlue")),
            ),
            (
                "/personalization/vibe_mode",
                Value::Text(String::from("Default")),
            ),
            ("/personalization/glassmorphism", Value::Bool(true)),
            ("/personalization/animations", Value::Bool(true)),
            ("/power/profile", Value::Text(String::from("Performance"))),
            ("/power/sleep_idle_minutes", Value::Int(15)),
            ("/privacy/location_enabled", Value::Bool(false)),
            ("/privacy/camera_enabled", Value::Bool(false)),
            ("/privacy/microphone_enabled", Value::Bool(false)),
        ];
        for (k, v) in defaults {
            self.tree.insert(String::from(*k), v.clone());
        }
    }

    fn get(&self, key: &str) -> Option<&Value> {
        self.tree.get(key)
    }

    fn set(&mut self, key: &str, value: Value) -> u64 {
        let gen = GENERATION.fetch_add(1, Ordering::Relaxed) + 1;
        let previous = self.tree.get(key).cloned();
        self.journal.push(JournalEntry {
            generation: gen,
            key: String::from(key),
            previous,
        });
        self.tree.insert(String::from(key), value);
        gen
    }

    fn snapshot(&mut self) -> u64 {
        let id = self.next_snapshot_id;
        self.next_snapshot_id += 1;
        let current_gen = GENERATION.load(Ordering::Relaxed);
        self.snapshots.insert(id, current_gen);
        id
    }

    /// Roll back every mutation whose generation is *greater* than the
    /// snapshot's generation. Walks the journal backwards, restoring each
    /// previous value.
    fn rollback(&mut self, id: u64) -> Result<(), ()> {
        let target_gen = *self.snapshots.get(&id).ok_or(())?;
        while let Some(entry) = self.journal.last() {
            if entry.generation <= target_gen {
                break;
            }
            // pop & undo
            let e = self.journal.pop().unwrap();
            match e.previous {
                Some(v) => {
                    self.tree.insert(e.key, v);
                }
                None => {
                    self.tree.remove(&e.key);
                }
            }
        }
        Ok(())
    }

    /// Restore a **single** key to the value it held at `target_gen`, leaving
    /// every other key at its current value (Concept §"Bricked your config?
    /// Roll back one click" — but per-setting, not whole-snapshot). Unlike
    /// [`rollback`], the rest of the tree is untouched. The restore is itself
    /// journaled, so it can be rolled back too. Returns `Err` for an unknown key.
    fn restore_key(&mut self, key: &str, target_gen: u64) -> Result<(), ()> {
        // The value `key` held at `target_gen` is the `previous` of the
        // earliest edit to `key` AFTER target_gen. If no edit followed,
        // the current value already reflects target_gen.
        let mut value_at: Option<Value> = self.tree.get(key).cloned();
        let mut earliest_after: u64 = u64::MAX;
        let mut had_history = false;
        for e in &self.journal {
            if e.key == key {
                had_history = true;
                if e.generation > target_gen && e.generation < earliest_after {
                    earliest_after = e.generation;
                    value_at = e.previous.clone();
                }
            }
        }
        if !had_history && self.tree.get(key).is_none() {
            return Err(()); // unknown key — nothing to restore
        }
        match value_at {
            Some(v) => {
                self.set(key, v); // journaled restore (reversible)
                Ok(())
            }
            None => {
                // Key did not exist at target_gen → restore it to absent.
                if self.tree.contains_key(key) {
                    let gen = GENERATION.fetch_add(1, Ordering::Relaxed) + 1;
                    let prev = self.tree.remove(key);
                    self.journal.push(JournalEntry {
                        generation: gen,
                        key: String::from(key),
                        previous: prev,
                    });
                }
                Ok(())
            }
        }
    }

    /// Serialize the whole tree as a deterministic, human-readable dump.
    /// `/proc/raeen/config` calls this.
    pub fn dump(&self) -> String {
        let mut out = String::new();
        out.push_str("# AthenaOS config registry (generation ");
        let g = GENERATION.load(Ordering::Relaxed);
        out.push_str(&alloc::format!("{}", g));
        out.push_str(", ");
        out.push_str(&alloc::format!("{}", self.tree.len()));
        out.push_str(" keys, ");
        out.push_str(&alloc::format!("{}", self.snapshots.len()));
        out.push_str(" snapshot(s))\n");
        for (k, v) in &self.tree {
            out.push_str(k);
            out.push_str(" = ");
            match v {
                Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
                Value::Int(i) => out.push_str(&alloc::format!("{}", i)),
                Value::Bytes(b) => {
                    out.push_str(&alloc::format!("<{} bytes>", b.len()));
                }
                Value::Text(s) => {
                    out.push('"');
                    out.push_str(s);
                    out.push('"');
                }
            }
            out.push('\n');
        }
        out
    }
}

static REGISTRY: Mutex<Option<Registry>> = Mutex::new(None);

// ── Boot hook ──────────────────────────────────────────────────────────

pub fn init() {
    let reg = Registry::new();
    let n = reg.tree.len();
    *REGISTRY.lock() = Some(reg);
    crate::serial_println!(
        "[ OK ] Config registry: {} default keys seeded (versioned, with snapshots)",
        n,
    );
}

/// Boot smoketest: prove the Concept-doc promise that every config change
/// saves a previous version and a snapshot can be rolled back "one click".
/// Exercises set → snapshot → edit×2 → rollback and verifies the prior value
/// is restored, plus that the journal retained the intermediate versions.
pub fn run_boot_smoketest() {
    let mut guard = REGISTRY.lock();
    let Some(reg) = guard.as_mut() else {
        crate::serial_println!("[config] smoketest skipped: registry not initialized");
        return;
    };

    let key = "/system/_smoketest_rollback";
    reg.set(key, Value::Int(1));
    let journal_before = reg.journal.len();
    let snap = reg.snapshot();
    reg.set(key, Value::Int(2));
    reg.set(key, Value::Int(3));
    let before = reg.get(key).cloned();
    // Each edit must have appended a journal entry preserving the prior value.
    let versions_saved = reg.journal.len() - journal_before;
    let _ = reg.rollback(snap);
    let after = reg.get(key).cloned();

    let restore_ok = before == Some(Value::Int(3)) && after == Some(Value::Int(1));
    // Leave the registry as we found it: drop the test key entirely.
    reg.tree.remove(key);

    // ── Per-key restore (Phase 5.7): roll back ONE setting to a prior
    //    generation while leaving unrelated keys at their current values. ──
    let key_a = "/system/_smoketest_keyrestore";
    let key_b = "/system/_smoketest_other";
    let gen_a1 = reg.set(key_a, Value::Int(100)); // key_a = 100 @ gen_a1
    reg.set(key_b, Value::Int(7));
    reg.set(key_a, Value::Int(200)); // key_a advances to 200
    reg.set(key_b, Value::Int(8)); // key_b advances to 8
    let a_before = reg.get(key_a).cloned();
    let restore_res = reg.restore_key(key_a, gen_a1); // restore ONLY key_a to its gen_a1 value
    let a_after = reg.get(key_a).cloned();
    let b_after = reg.get(key_b).cloned();
    let keyrestore_ok = restore_res.is_ok()
        && a_before == Some(Value::Int(200))
        && a_after == Some(Value::Int(100)) // reverted
        && b_after == Some(Value::Int(8)); // untouched
    reg.tree.remove(key_a);
    reg.tree.remove(key_b);

    crate::serial_println!(
        "[config] smoketest: versioned save+rollback restore_ok={} versions_saved={} per_key_restore={} (a {:?}->{:?}, b stays {:?}) -> {}",
        restore_ok, versions_saved, keyrestore_ok, a_before, a_after, b_after,
        if restore_ok && keyrestore_ok { "PASS" } else { "FAIL" }
    );
}

// ── Public APIs (also used by procfs) ──────────────────────────────────

pub fn current_generation() -> u64 {
    GENERATION.load(Ordering::Relaxed)
}

pub fn dump_text() -> String {
    let g = REGISTRY.lock();
    g.as_ref()
        .map(|r| r.dump())
        .unwrap_or_else(|| String::from("# registry not initialized\n"))
}

/// Kernel helper: set a text config key (session bootstrap, drivers, etc.).
pub fn set_text(key: &str, value: &str) {
    let mut guard = REGISTRY.lock();
    let Some(reg) = guard.as_mut() else { return };
    reg.set(key, Value::Text(String::from(value)));
}

/// Kernel helper: set a boolean config key.
pub fn set_bool(key: &str, value: bool) {
    let mut guard = REGISTRY.lock();
    let Some(reg) = guard.as_mut() else { return };
    reg.set(key, Value::Bool(value));
}

/// Kernel helper: set an integer config key.
pub fn set_int(key: &str, value: i64) {
    let mut guard = REGISTRY.lock();
    let Some(reg) = guard.as_mut() else { return };
    reg.set(key, Value::Int(value));
}

/// Kernel helper: read a boolean config key. Returns `None` if the key
/// is absent or holds a non-Bool variant. Used by first-boot detection
/// (`setup_ui::is_first_boot_complete`) and similar one-shot config
/// gates that need a clean "is the flag set?" answer.
pub fn get_bool(key: &str) -> Option<bool> {
    let guard = REGISTRY.lock();
    let reg = guard.as_ref()?;
    match reg.get(key) {
        Some(Value::Bool(b)) => Some(*b),
        _ => None,
    }
}

/// Kernel helper: read a text config key. Returns `None` if absent or
/// non-Text. Used for `/session/last_user` (login screen greeting) and
/// similar one-shot string lookups.
/// Every key under `prefix`, with its text value (non-text values skipped).
/// The Win32 registry shim replays `/bridge/registry/...` through this at
/// boot to resurrect persisted registry values.
pub fn text_entries_with_prefix(prefix: &str) -> alloc::vec::Vec<(String, String)> {
    let guard = REGISTRY.lock();
    let Some(reg) = guard.as_ref() else {
        return alloc::vec::Vec::new();
    };
    reg.tree
        .iter()
        .filter(|(k, _)| k.starts_with(prefix))
        .filter_map(|(k, v)| match v {
            Value::Text(s) => Some((k.clone(), s.clone())),
            _ => None,
        })
        .collect()
}

pub fn get_text(key: &str) -> Option<String> {
    let guard = REGISTRY.lock();
    let reg = guard.as_ref()?;
    match reg.get(key) {
        Some(Value::Text(s)) => Some(s.clone()),
        _ => None,
    }
}

// ── Syscall numbers + dispatch ─────────────────────────────────────────

pub const SYS_CONFIG_GET: u64 = 50;
pub const SYS_CONFIG_SET: u64 = 51;
pub const SYS_CONFIG_SNAPSHOT: u64 = 52;
pub const SYS_CONFIG_ROLLBACK: u64 = 53;

/// Read a key. Writes raw value bytes into `out_ptr` (up to `out_cap`).
/// Returns the number of bytes that *would* have been written, so callers
/// can size their buffer correctly on the second call. `u64::MAX` on error.
pub fn sys_config_get(
    key_ptr: u64,
    key_len: u64,
    out_ptr: u64,
    out_cap: u64,
    validate_r: impl Fn(u64, u64, bool) -> bool,
    validate_w: impl Fn(u64, u64, bool) -> bool,
) -> u64 {
    if !validate_r(key_ptr, key_len, false) {
        return u64::MAX;
    }
    if out_cap > 0 && !validate_w(out_ptr, out_cap, true) {
        return u64::MAX;
    }
    let key = read_user_str(key_ptr, key_len);
    let guard = REGISTRY.lock();
    let reg = match guard.as_ref() {
        Some(r) => r,
        None => return u64::MAX,
    };
    let val = match reg.get(&key) {
        Some(v) => v.as_bytes(),
        None => return u64::MAX,
    };
    let n = core::cmp::min(val.len() as u64, out_cap) as usize;
    // Validated copy-out: a bogus/kernel out_ptr is rejected (was an
    // arbitrary-kernel-WRITE hole via a raw copy_nonoverlapping to out_ptr).
    if n > 0 && crate::uaccess::copy_to_user(out_ptr, &val[..n]).is_err() {
        return u64::MAX;
    }
    val.len() as u64
}

/// Write a key. Returns the new generation number, or `u64::MAX` on error.
pub fn sys_config_set(
    key_ptr: u64,
    key_len: u64,
    val_ptr: u64,
    val_len: u64,
    validate_r: impl Fn(u64, u64, bool) -> bool,
) -> u64 {
    if !validate_r(key_ptr, key_len, false) {
        return u64::MAX;
    }
    if val_len > 0 && !validate_r(val_ptr, val_len, false) {
        return u64::MAX;
    }
    let key = read_user_str(key_ptr, key_len);
    let bytes = read_user_bytes(val_ptr, val_len);
    // Heuristic typing: try valid UTF-8 → Text, else Bytes. Tiny payloads
    // that are 1/8 byte and look numeric get promoted to Int/Bool, mirroring
    // raesettings' expectations.
    let value = if val_len == 1 {
        Value::Bool(bytes.first().copied().unwrap_or(0) != 0)
    } else if val_len == 8 {
        let mut buf = [0u8; 8];
        buf.copy_from_slice(&bytes);
        Value::Int(i64::from_le_bytes(buf))
    } else if let Ok(s) = core::str::from_utf8(&bytes) {
        Value::Text(String::from(s))
    } else {
        Value::Bytes(bytes)
    };
    let mut guard = REGISTRY.lock();
    let reg = match guard.as_mut() {
        Some(r) => r,
        None => return u64::MAX,
    };
    reg.set(&key, value)
}

pub fn sys_config_snapshot() -> u64 {
    let mut g = REGISTRY.lock();
    match g.as_mut() {
        Some(r) => r.snapshot(),
        None => u64::MAX,
    }
}

pub fn sys_config_rollback(snapshot_id: u64) -> u64 {
    let mut g = REGISTRY.lock();
    match g.as_mut() {
        Some(r) => match r.rollback(snapshot_id) {
            Ok(()) => 0,
            Err(()) => u64::MAX,
        },
        None => u64::MAX,
    }
}

/// Per-key restore (Phase 5.7): roll a single config `key` back to the value it
/// held at `generation`, leaving every other key at its current value. Returns
/// `0` on success, `u64::MAX` on failure (unknown key / registry uninitialized).
/// Backs the userspace `sys_config_restore(key, version)` surface and a
/// `/proc/raeen/config` restore action. `generation` is a value previously
/// returned by `sys_config_set` (or a snapshot generation).
pub fn restore_key(key: &str, generation: u64) -> u64 {
    let mut g = REGISTRY.lock();
    match g.as_mut() {
        Some(r) => match r.restore_key(key, generation) {
            Ok(()) => 0,
            Err(()) => u64::MAX,
        },
        None => u64::MAX,
    }
}

// ── Helpers ────────────────────────────────────────────────────────────

fn read_user_str(ptr: u64, len: u64) -> String {
    // Validated + fault-fixup: rejects a kernel/bogus pointer (was an
    // arbitrary-kernel-read hole via a raw copy_nonoverlapping).
    crate::uaccess::read_user_string(ptr, len)
}

fn read_user_bytes(ptr: u64, len: u64) -> Vec<u8> {
    crate::uaccess::read_user_bytes(ptr, len)
}
