//! Per-app data buckets — Concept §Security: FS-layer isolation.
//!
//! Each task gets an in-memory namespace at `/data/apps/self/<file>` (and
//! `/data/apps/<task_id>/<file>` for debugging). Only the owning task may
//! read or write its bucket unless the caller holds `Cap::FS_ADMIN`.

#![allow(dead_code)]

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

use crate::task::TaskId;

struct Bucket {
    files: BTreeMap<String, Vec<u8>>,
}

static BUCKETS: Mutex<BTreeMap<u64, Bucket>> = Mutex::new(BTreeMap::new());

pub const SELF_PREFIX: &str = "/data/apps/self/";
pub const APPS_PREFIX: &str = "/data/apps/";

pub fn init() {
    crate::serial_println!("[ OK ] Per-app data buckets (/data/apps/self/)");
}

pub fn on_task_spawn(task_id: TaskId) {
    let mut guard = BUCKETS.lock();
    guard.entry(task_id.raw()).or_insert_with(|| Bucket {
        files: BTreeMap::new(),
    });
}

pub fn on_task_exit(task_id: TaskId) {
    BUCKETS.lock().remove(&task_id.raw());
}

/// Parse `/data/apps/self/foo` or `/data/apps/123/foo`.
pub fn parse_path(path: &str) -> Option<(u64, String)> {
    let rest = path.strip_prefix(APPS_PREFIX)?;
    if let Some(file) = rest.strip_prefix("self/") {
        let current = crate::scheduler::current_task_id()?.raw();
        return Some((current, String::from(file)));
    }
    let mut parts = rest.splitn(2, '/');
    let id_str = parts.next()?;
    let file = parts.next()?;
    let id = id_str.parse::<u64>().ok()?;
    Some((id, String::from(file)))
}

pub fn can_access(bucket_task: u64, requester: TaskId) -> bool {
    bucket_task == requester.raw()
}

pub fn read(bucket_task: u64, name: &str) -> Option<Vec<u8>> {
    BUCKETS
        .lock()
        .get(&bucket_task)
        .and_then(|b| b.files.get(name).cloned())
}

pub fn write(bucket_task: u64, name: &str, data: Vec<u8>) {
    let mut guard = BUCKETS.lock();
    let bucket = guard.entry(bucket_task).or_insert_with(|| Bucket {
        files: BTreeMap::new(),
    });
    bucket.files.insert(String::from(name), data);
}

pub fn list(bucket_task: u64) -> Vec<(String, usize)> {
    BUCKETS
        .lock()
        .get(&bucket_task)
        .map(|b| b.files.iter().map(|(k, v)| (k.clone(), v.len())).collect())
        .unwrap_or_default()
}

/// Boot proof of the FS-layer isolation claim (Concept §Security: "apps
/// can't touch other apps' data without explicit permission" — the
/// ransomware-resistance acceptance, MasterChecklist Phase 9.3). Exercises
/// the REAL VFS read path a malicious app would use, not just the API:
///   1. a victim bucket holds a secret,
///   2. reading it through `vfs::read_file("/data/apps/<victim>/…")` with no
///      matching task identity is DENIED (the vfs gate is fail-close:
///      no current task → deny; wrong task → `can_access` false),
///   3. the ownership predicate the gate uses discriminates owner vs
///      attacker exactly,
///   4. `self/` paths refuse to resolve without a task identity.
pub fn run_boot_smoketest() {
    let victim: u64 = 0xA11CE;
    let attacker = TaskId::from_raw(victim + 1);

    write(victim, "secret.txt", Vec::from(&b"top-secret"[..]));
    let store = read(victim, "secret.txt").as_deref() == Some(&b"top-secret"[..]);

    // The actual attack surface: the VFS path. During boot there is no
    // current task, and the gate must fail CLOSED (deny), exactly as it
    // denies a mismatched task id at runtime.
    let path = alloc::format!("/data/apps/{}/secret.txt", victim);
    let vfs_denied = crate::vfs::read_file(&path).is_none();

    // The predicate the gate evaluates for a live (malicious) task.
    let deny_foreign = !can_access(victim, attacker);
    let allow_owner = can_access(victim, TaskId::from_raw(victim));

    // `self/` must not resolve to ANY bucket without a task identity.
    let self_requires_task = crate::scheduler::current_task_id().is_some()
        || parse_path("/data/apps/self/secret.txt").is_none();

    on_task_exit(TaskId::from_raw(victim));

    let pass = store && vfs_denied && deny_foreign && allow_owner && self_requires_task;
    crate::serial_println!(
        "[buckets] run_boot_smoketest: store={} vfs_deny={} deny_foreign={} allow_owner={} self_needs_task={} -> {}",
        store,
        vfs_denied,
        deny_foreign,
        allow_owner,
        self_requires_task,
        if pass { "PASS" } else { "FAIL" },
    );
}
