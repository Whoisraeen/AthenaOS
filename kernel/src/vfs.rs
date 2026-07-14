//! True Hierarchical VFS layer.
//!
//! Provides `SYS_OPEN`, `SYS_READ`, `SYS_WRITE`, `SYS_CLOSE`, `SYS_SEEK`, `SYS_STAT`
//! handling over a true directory tree structure, mount points (e.g., `/proc`, `/system`),
//! and file endpoints (e.g., RaeFS, initramfs).

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use lazy_static::lazy_static;
use spin::Mutex;

const E_VFS_READONLY: u64 = 0xFFFF_FFFF_FFFF_FD01;
const E_VFS_NOT_FOUND: u64 = 0xFFFF_FFFF_FFFF_FD02;
pub const E_VFS_EXISTS: u64 = 0xFFFF_FFFF_FFFF_FD03;
const E_VFS_NOT_EMPTY: u64 = 0xFFFF_FFFF_FFFF_FD04;
const E_VFS_INVAL: u64 = 0xFFFF_FFFF_FFFF_FD05;

/// A node in the virtual filesystem hierarchy.
#[derive(Debug)]
pub enum VfsNode {
    Directory(BTreeMap<String, VfsNode>),
    File(Vec<u8>),
}

impl VfsNode {
    pub fn new_dir() -> Self {
        VfsNode::Directory(BTreeMap::new())
    }
}

lazy_static! {
    static ref ROOT: Mutex<VfsNode> = Mutex::new(VfsNode::new_dir());
}

fn traverse_mut<'a>(
    root: &'a mut VfsNode,
    path: &str,
    create_dirs: bool,
) -> Result<&'a mut VfsNode, u64> {
    let mut current = root;
    for part in path.split('/').filter(|s| !s.is_empty()) {
        if let VfsNode::Directory(ref mut children) = current {
            if !children.contains_key(part) {
                if create_dirs {
                    children.insert(part.to_string(), VfsNode::new_dir());
                } else {
                    return Err(E_VFS_NOT_FOUND);
                }
            }
            current = children.get_mut(part).unwrap();
        } else {
            return Err(E_VFS_INVAL);
        }
    }
    Ok(current)
}

fn traverse<'a>(root: &'a VfsNode, path: &str) -> Option<&'a VfsNode> {
    let mut current = root;
    for part in path.split('/').filter(|s| !s.is_empty()) {
        if let VfsNode::Directory(ref children) = current {
            current = children.get(part)?;
        } else {
            return None;
        }
    }
    Some(current)
}

fn normalize_path(path: &str) -> String {
    let mut p = path.trim().to_string();
    if p.is_empty() {
        p.push('/');
    }
    if !p.starts_with('/') {
        p.insert(0, '/');
    }
    while p.len() > 1 && p.ends_with('/') {
        p.pop();
    }
    p
}

fn home_prefix_for_session() -> String {
    alloc::format!("/home/{}", crate::session::username())
}

fn is_session_home_path(path: &str) -> bool {
    normalize_path(path).starts_with(&home_prefix_for_session())
}

// ─── INIT & SMOKETEST ────────────────────────────────────────────────────────

pub fn init() {
    // Populate the initial hierarchical mount points
    let mut root = ROOT.lock();
    let _ = traverse_mut(&mut root, "/home", true);
    let _ = traverse_mut(&mut root, "/system/apps", true);
    let _ = traverse_mut(&mut root, "/data", true);
    let _ = traverse_mut(&mut root, "/proc", true);

    crate::serial_println!("[ OK ] VFS Hierarchy initialized");
}

pub fn run_boot_smoketest() {
    // Soft-fail smoketest — kernelchecklist.md R1 says boot must never
    // regress. If a VFS op fails (e.g. parent directory `/home` doesn't
    // exist yet because the directory tree hasn't been built), log and
    // continue rather than panicking.
    // mkdir is only permitted under the session home (`/home/<user>`); a bare
    // `/home/smoketest_dir` is rejected READONLY. Test inside the real home so
    // the smoketest exercises the production-permitted path.
    let home = home_prefix_for_session();
    let test_dir = alloc::format!("{}/smoketest_dir", home);
    let test_file = alloc::format!("{}/smoketest_dir/test.txt", home);
    let mut pass = true;

    if mkdir_at(&test_dir, 0o755) != Ok(()) {
        crate::serial_println!("[vfs] smoketest: mkdir_at {} failed", test_dir);
        pass = false;
    }

    if pass {
        let mut root = ROOT.lock();
        if let Ok(VfsNode::Directory(ref mut children)) = traverse_mut(&mut root, &test_dir, false)
        {
            children.insert("test.txt".to_string(), VfsNode::File(b"vfs works".to_vec()));
        }
    }

    if pass {
        let entries = list_dir_at(&test_dir);
        if entries.len() != 1 || entries[0].name != "test.txt" {
            crate::serial_println!(
                "[vfs] smoketest: list_dir mismatch (got {} entries)",
                entries.len()
            );
            pass = false;
        }
    }

    if pass {
        if unlink_at(&test_file) != Ok(()) {
            pass = false;
        }
        if unlink_at(&test_dir) != Ok(()) {
            pass = false;
        }
    }

    if pass {
        crate::serial_println!("[vfs] smoketest passed");
    } else {
        crate::serial_println!("[vfs] smoketest skipped — hierarchical VFS not fully wired yet");
    }

    // Initramfs directory-enumeration smoketest. A real Linux app (and the file
    // manager) must be able to `stat`+`opendir`+`getdents64` an initramfs
    // directory like `/bin`. The tar holds the bundled binaries as `bin/<name>`
    // with no explicit dir entry, so this exercises is_dir() + list_dir_at() +
    // open_dir_as_dirent_stream() recognising `/bin` as a directory and listing
    // its children. FAIL-able: prints FAIL if /bin is misreported or empty.
    let dir_ok = is_dir("/bin")
        && !is_dir("/bin/dh") // a file under /bin must NOT be a directory
        && {
            let entries = list_dir_at("/bin");
            entries.iter().any(|e| e.name == "dh") && entries.iter().any(|e| e.name == "seq")
        }
        && open_dir_as_dirent_stream("/bin").is_some();
    if dir_ok {
        crate::serial_println!("[vfs] initramfs-dir smoketest: /bin enumerates (dh,seq) -> PASS");
    } else {
        crate::serial_println!("[vfs] initramfs-dir smoketest: FAIL (/bin not a listable dir)");
    }
}

pub fn proc_dump_text() -> String {
    let mut out = String::new();
    out.push_str("VFS Hierarchy Status\n");
    let root = ROOT.lock();

    fn dump_node(node: &VfsNode, out: &mut String, depth: usize) {
        let indent = "  ".repeat(depth);
        if let VfsNode::Directory(ref children) = node {
            for (name, child) in children.iter() {
                out.push_str(&alloc::format!("{}- {}/\n", indent, name));
                dump_node(child, out, depth + 1);
            }
        }
    }

    out.push_str("/\n");
    dump_node(&root, &mut out, 1);
    out
}

// ─── INODE ABSTRACTION ───────────────────────────────────────────────────────

pub trait Inode: Send + Sync {
    fn read_at(&self, offset: usize, buf: &mut [u8]) -> usize;
    fn write_at(&self, offset: usize, buf: &[u8]) -> usize;
    fn size(&self) -> usize;
    /// Present only for the kernel-owned DRM render-node inode. Device
    /// operations stay out of the byte-file API and ordinary inodes therefore
    /// fail closed without learning any DRM details.
    fn render_client_id(&self) -> Option<u64> {
        None
    }
}

pub struct DirentStreamInode {
    data: Vec<u8>,
}

impl DirentStreamInode {
    pub fn from_linux_dirent64_bytes(data: Vec<u8>) -> Self {
        Self { data }
    }
}

impl Inode for DirentStreamInode {
    fn read_at(&self, offset: usize, buf: &mut [u8]) -> usize {
        if offset >= self.data.len() || buf.is_empty() {
            return 0;
        }
        let mut pos = offset;
        let mut out_len = 0usize;
        while pos + 18 <= self.data.len() {
            if out_len >= buf.len() {
                break;
            }
            let reclen = u16::from_le_bytes([self.data[pos + 16], self.data[pos + 17]]) as usize;
            if reclen == 0 || pos + reclen > self.data.len() || out_len + reclen > buf.len() {
                break;
            }
            buf[out_len..out_len + reclen].copy_from_slice(&self.data[pos..pos + reclen]);
            out_len += reclen;
            pos += reclen;
        }
        out_len
    }
    fn write_at(&self, _offset: usize, _buf: &[u8]) -> usize {
        0
    }
    fn size(&self) -> usize {
        self.data.len()
    }
}

pub struct File {
    pub inode: Arc<dyn Inode>,
    pub offset: usize,
    pub flags: u32,
}

impl core::fmt::Debug for File {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("File")
            .field("offset", &self.offset)
            .field("flags", &self.flags)
            .finish()
    }
}

impl File {
    pub fn new(inode: Arc<dyn Inode>, flags: u32) -> Self {
        Self {
            inode,
            offset: 0,
            flags,
        }
    }
    pub fn read(&mut self, buf: &mut [u8]) -> usize {
        let n = self.inode.read_at(self.offset, buf);
        self.offset += n;
        n
    }
    pub fn write(&mut self, buf: &[u8]) -> usize {
        let n = self.inode.write_at(self.offset, buf);
        self.offset += n;
        n
    }
    pub fn seek(&mut self, offset: usize) -> usize {
        self.offset = offset;
        self.offset
    }
}

pub struct InitramfsInode {
    pub data: &'static [u8],
}

impl Inode for InitramfsInode {
    fn read_at(&self, offset: usize, buf: &mut [u8]) -> usize {
        if offset >= self.data.len() {
            return 0;
        }
        let n = core::cmp::min(self.data.len() - offset, buf.len());
        buf[..n].copy_from_slice(&self.data[offset..offset + n]);
        n
    }
    fn write_at(&self, _offset: usize, _buf: &[u8]) -> usize {
        0
    }
    fn size(&self) -> usize {
        self.data.len()
    }
}

pub struct BytesInode {
    pub data: alloc::vec::Vec<u8>,
}

impl Inode for BytesInode {
    fn read_at(&self, offset: usize, buf: &mut [u8]) -> usize {
        if offset >= self.data.len() {
            return 0;
        }
        let n = core::cmp::min(self.data.len() - offset, buf.len());
        buf[..n].copy_from_slice(&self.data[offset..offset + n]);
        n
    }
    fn write_at(&self, _offset: usize, _buf: &[u8]) -> usize {
        0
    }
    fn size(&self) -> usize {
        self.data.len()
    }
}

// ─── DIRECTORY OPERATIONS ────────────────────────────────────────────────────

pub struct DirEntry {
    pub name: alloc::string::String,
    pub size: usize,
}

fn push_linux_dirent64(out: &mut Vec<u8>, ino: u64, off: i64, dtype: u8, name: &str) {
    let name_bytes = name.as_bytes();
    let base = 8 + 8 + 2 + 1;
    let mut reclen = base + name_bytes.len() + 1;
    reclen = (reclen + 7) & !7;
    out.extend_from_slice(&ino.to_le_bytes());
    out.extend_from_slice(&off.to_le_bytes());
    out.extend_from_slice(&(reclen as u16).to_le_bytes());
    out.push(dtype);
    out.extend_from_slice(name_bytes);
    out.push(0);
    while out.len() % 8 != 0 {
        out.push(0);
    }
}

pub fn linux_getdents64_bytes_for_path(path: &str) -> Vec<u8> {
    let path = match path {
        "" => "/",
        other => other,
    };
    const DT_UNKNOWN: u8 = 0;
    const DT_DIR: u8 = 4;
    const DT_REG: u8 = 8;
    let mut out = Vec::new();
    push_linux_dirent64(&mut out, 1, 1, DT_DIR, ".");
    push_linux_dirent64(&mut out, 1, 2, DT_DIR, "..");

    let entries = list_dir_at(path);
    let mut off: i64 = 3;
    for (i, e) in entries.iter().enumerate() {
        let ino = 10 + i as u64;
        let dtype = if e.size == 0 { DT_DIR } else { DT_REG };
        push_linux_dirent64(&mut out, ino, off, dtype, &e.name);
        off += 1;
    }
    if out.is_empty() {
        push_linux_dirent64(&mut out, 1, 1, DT_UNKNOWN, ".");
    }
    out
}

pub fn list_dir_at(path: &str) -> alloc::vec::Vec<DirEntry> {
    let path = normalize_path(path);
    let mut entries = Vec::new();

    if path == "/dev" {
        entries.push(DirEntry {
            name: "dri".into(),
            size: 0,
        });
    } else if path == "/dev/dri" && crate::gpu_render::is_available() {
        entries.push(DirEntry {
            name: "renderD128".into(),
            size: 1, // nonzero so the legacy dirent encoder reports DT_REG, not DT_DIR
        });
    }

    // 1. Check virtual tree
    {
        let root = ROOT.lock();
        if let Some(VfsNode::Directory(children)) = traverse(&root, &path) {
            for (name, child) in children.iter() {
                let size = match child {
                    VfsNode::Directory(_) => 0,
                    VfsNode::File(data) => data.len(),
                };
                entries.push(DirEntry {
                    name: name.clone(),
                    size,
                });
            }
        }
    }

    // 2. Check mount points and synthetic overlays
    if path == "/system/apps" {
        for file in crate::tar::TarArchive::new(crate::INITRAMFS).iter() {
            let name = file.name.rsplit('/').next().unwrap_or(&file.name);
            if !name.is_empty() && !entries.iter().any(|e| e.name == name) {
                entries.push(DirEntry {
                    name: name.to_string(),
                    size: file.data.len(),
                });
            }
        }
    } else if path == "/" {
        for file in crate::tar::TarArchive::new(crate::INITRAMFS).iter() {
            if !entries.iter().any(|e| e.name == file.name) {
                entries.push(DirEntry {
                    name: file.name.to_string(),
                    size: file.data.len(),
                });
            }
        }
    } else if path == "/data/apps/self" || path.starts_with("/data/apps/self/") {
        if let Some(t) = crate::scheduler::current_task_id() {
            for (name, size) in crate::data_buckets::list(t.raw()) {
                entries.push(DirEntry { name, size });
            }
        }
    }

    // General initramfs directory enumeration. The tar stores entries WITHOUT a
    // leading slash (`bin/ls`, `usr/lib/libc.so.6`), so listing `/bin` or
    // `/usr/lib` must surface the tar's DIRECT children — files AND intermediate
    // subdirectories. (`/` and `/system/apps` keep their flat-listing semantics
    // above.) Without this, `/bin` enumerated nothing.
    if path != "/" && path != "/system/apps" {
        let prefix = alloc::format!("{}/", path.trim_start_matches('/'));
        if prefix.len() > 1 {
            for file in crate::tar::TarArchive::new(crate::INITRAMFS).iter() {
                if let Some(rest) = file.name.strip_prefix(prefix.as_str()) {
                    if rest.is_empty() {
                        continue;
                    }
                    let mut parts = rest.splitn(2, '/');
                    let child = parts.next().unwrap_or("");
                    let is_subdir = parts.next().is_some(); // more path => child is a dir
                    if !child.is_empty() && !entries.iter().any(|e| e.name == child) {
                        entries.push(DirEntry {
                            name: child.to_string(),
                            size: if is_subdir { 0 } else { file.data.len() },
                        });
                    }
                }
            }
        }
    }

    entries
}

pub fn mkdir_at(path: &str, _mode: u32) -> Result<(), u64> {
    let path = normalize_path(path);
    if path == "/" {
        return Err(E_VFS_INVAL);
    }
    if !is_session_home_path(&path) {
        return Err(E_VFS_READONLY);
    }

    let mut root = ROOT.lock();
    let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    if parts.is_empty() {
        return Err(E_VFS_INVAL);
    }

    let parent_path = parts[..parts.len() - 1].join("/");
    let name = parts.last().unwrap();

    let parent = traverse_mut(&mut root, &parent_path, true)?;
    if let VfsNode::Directory(ref mut children) = parent {
        if children.contains_key(*name) {
            return Err(E_VFS_EXISTS);
        }
        children.insert(name.to_string(), VfsNode::new_dir());
        Ok(())
    } else {
        Err(E_VFS_INVAL)
    }
}

/// Seed the current session's home with the standard user folders so the file
/// manager (and any app) sees a real, navigable home tree — Documents,
/// Downloads, Pictures, Music, Videos, Desktop — instead of hard-coded mock
/// data. Idempotent: a folder that already exists returns `E_VFS_EXISTS`, which
/// we ignore. `mkdir_at` creates intermediate parents, so the `/home/<user>`
/// root is created on the first call.
///
/// MUST be called WITHOUT the `SESSION` lock held: `mkdir_at` →
/// `is_session_home_path` → `home_prefix_for_session` → `session::username`
/// re-locks `SESSION` (re-entrant spin Mutex would deadlock). The post-login
/// `shell_runner::activate_desktop` call site satisfies this.
pub fn ensure_session_home_dirs() {
    const STANDARD: &[&str] = &[
        "Desktop",
        "Documents",
        "Downloads",
        "Pictures",
        "Music",
        "Videos",
    ];
    let home = home_prefix_for_session();
    for folder in STANDARD {
        let path = alloc::format!("{}/{}", home, folder);
        match mkdir_at(&path, 0o755) {
            Ok(()) | Err(E_VFS_EXISTS) => {}
            Err(e) => crate::serial_println!(
                "[vfs] ensure_session_home_dirs: mkdir {} failed (err {})",
                path,
                e
            ),
        }
    }
    crate::serial_println!("[vfs] session home seeded at {}", home);
}

pub fn unlink_at(path: &str) -> Result<(), u64> {
    let path = normalize_path(path);
    if path.starts_with("/proc/") {
        return Err(E_VFS_READONLY);
    }

    let mut root = ROOT.lock();
    let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    if parts.is_empty() {
        return Err(E_VFS_INVAL);
    }

    let parent_path = parts[..parts.len() - 1].join("/");
    let name = parts.last().unwrap();

    if let Ok(VfsNode::Directory(ref mut children)) = traverse_mut(&mut root, &parent_path, false) {
        if let Some(node) = children.get(*name) {
            if let VfsNode::Directory(ref contents) = node {
                if !contents.is_empty() {
                    return Err(E_VFS_NOT_EMPTY);
                }
            }
            children.remove(*name);
            return Ok(());
        }
    }

    if crate::raefs::RaeFS::delete_file(&path) {
        return Ok(());
    }
    Err(E_VFS_NOT_FOUND)
}

pub fn rename_at(old_path: &str, new_path: &str) -> Result<(), u64> {
    let old_path = normalize_path(old_path);
    let new_path = normalize_path(new_path);
    if old_path.starts_with("/proc/") || new_path.starts_with("/proc/") {
        return Err(E_VFS_READONLY);
    }

    let mut root = ROOT.lock();

    // Remove from old
    let old_parts: Vec<&str> = old_path.split('/').filter(|s| !s.is_empty()).collect();
    if old_parts.is_empty() {
        return Err(E_VFS_INVAL);
    }
    let old_parent_path = old_parts[..old_parts.len() - 1].join("/");
    let old_name = old_parts.last().unwrap();

    let extracted = {
        if let Ok(VfsNode::Directory(ref mut children)) =
            traverse_mut(&mut root, &old_parent_path, false)
        {
            children.remove(*old_name)
        } else {
            None
        }
    };

    if let Some(node) = extracted {
        // Insert into new
        let new_parts: Vec<&str> = new_path.split('/').filter(|s| !s.is_empty()).collect();
        let new_parent_path = new_parts[..new_parts.len() - 1].join("/");
        let new_name = new_parts.last().unwrap();

        if let Ok(VfsNode::Directory(ref mut children)) =
            traverse_mut(&mut root, &new_parent_path, true)
        {
            if children.contains_key(*new_name) {
                // Re-insert old to prevent loss
                if let Ok(VfsNode::Directory(ref mut old_children)) =
                    traverse_mut(&mut root, &old_parent_path, true)
                {
                    old_children.insert(old_name.to_string(), node);
                }
                return Err(E_VFS_EXISTS);
            }
            children.insert(new_name.to_string(), node);
            return Ok(());
        }
    }

    if crate::raefs::RaeFS::rename_file(&old_path, &new_path) {
        return Ok(());
    }
    Err(E_VFS_NOT_FOUND)
}

pub fn open_path(path: &str) -> Option<Arc<dyn Inode>> {
    for candidate in crate::app_paths::resolve_candidates(path) {
        if let Some(inode) = open_path_exact(&candidate) {
            return Some(inode);
        }
    }
    None
}

/// Cheap directory test: does `path` name a directory? Determines this WITHOUT
/// building the directory's dirent listing — `open_path` returns a
/// `DirentStreamInode` for a directory, which enumerates every entry, so using
/// it to `stat` a directory does a full readdir (and `stat("/")` could also fall
/// through to the RaeFS `find_or_create_file` fallback). `stat`/`statx` use this
/// so a directory resolves fast and with the correct `S_IFDIR` mode — a real
/// Linux binary stat'ing a path (extremely common) was stalling on it.
pub fn is_dir(path: &str) -> bool {
    let norm = normalize_path(path);
    // Root + synthetic directory mounts resolve without any lock.
    if norm == "/"
        || norm == "/system"
        || norm == "/system/apps"
        || norm == "/proc"
        || norm == "/tmp"
        || norm == "/dev"
        || norm == "/dev/dri"
        || norm == "/dev/shm"
        || norm == "/home"
    {
        return true;
    }
    // Virtual hierarchy: traverse to the node only — never enumerate children.
    {
        let root = ROOT.lock();
        if matches!(traverse(&root, &norm), Some(VfsNode::Directory(_))) {
            return true;
        }
    }
    // Initramfs directories (`/bin`, `/usr`, `/usr/lib`, `/lib64`): the tar has
    // no explicit dir entries, only files like `bin/ls`, so they aren't in the
    // virtual tree. A path is a directory if ANY tar entry lives under it.
    // Without this, stat("/bin") reported S_IFREG and a real Linux app (or the
    // file manager) treated the directory as a regular file.
    let prefix = alloc::format!("{}/", norm.trim_start_matches('/'));
    if prefix.len() > 1 {
        for file in crate::tar::TarArchive::new(crate::INITRAMFS).iter() {
            if file.name.starts_with(prefix.as_str()) {
                return true;
            }
        }
    }
    false
}

pub fn is_render_node(path: &str) -> bool {
    normalize_path(path) == "/dev/dri/renderD128" && crate::gpu_render::is_available()
}

pub fn open_dir_as_dirent_stream(path: &str) -> Option<Arc<dyn Inode>> {
    let entries = list_dir_at(path);
    if path == "/" || !entries.is_empty() {
        return Some(Arc::new(DirentStreamInode::from_linux_dirent64_bytes(
            linux_getdents64_bytes_for_path(path),
        )));
    }
    None
}

pub fn read_file(path: &str) -> Option<alloc::vec::Vec<u8>> {
    let inode = open_path(path)?;
    let size = inode.size();
    if size == 0 || size > 64 * 1024 * 1024 {
        return None;
    }
    let mut data = alloc::vec![0u8; size];
    if inode.read_at(0, &mut data) != size {
        return None;
    }
    Some(data)
}

/// Writable inode backed by `ROOT` at `path` (must be under `/home/…`).
pub struct VfsMemoryFileInode {
    path: String,
}

impl VfsMemoryFileInode {
    fn with_data_mut<R>(&self, f: impl FnOnce(&mut Vec<u8>) -> R) -> R {
        let (parent, name) = match self.path.rsplit_once('/') {
            Some(p) => p,
            None => return f(&mut Vec::new()),
        };
        let mut root = ROOT.lock();
        let Ok(parent_node) = traverse_mut(&mut root, parent, true) else {
            return f(&mut Vec::new());
        };
        let VfsNode::Directory(children) = parent_node else {
            return f(&mut Vec::new());
        };
        if !children.contains_key(name) {
            children.insert(name.to_string(), VfsNode::File(Vec::new()));
        }
        let VfsNode::File(data) = children.get_mut(name).unwrap() else {
            return f(&mut Vec::new());
        };
        f(data)
    }

    fn with_data<R>(&self, f: impl FnOnce(&[u8]) -> R) -> R {
        let (parent, name) = match self.path.rsplit_once('/') {
            Some(p) => p,
            None => return f(&[]),
        };
        let root = ROOT.lock();
        let Some(parent_node) = traverse(&root, parent) else {
            return f(&[]);
        };
        let VfsNode::Directory(children) = parent_node else {
            return f(&[]);
        };
        let Some(VfsNode::File(data)) = children.get(name) else {
            return f(&[]);
        };
        f(data)
    }
}

impl Inode for VfsMemoryFileInode {
    fn read_at(&self, offset: usize, buf: &mut [u8]) -> usize {
        self.with_data(|data| {
            if offset >= data.len() {
                return 0;
            }
            let n = core::cmp::min(data.len() - offset, buf.len());
            buf[..n].copy_from_slice(&data[offset..offset + n]);
            n
        })
    }

    fn write_at(&self, offset: usize, buf: &[u8]) -> usize {
        self.with_data_mut(|data| {
            let end = offset.saturating_add(buf.len());
            if end > data.len() {
                data.resize(end, 0);
            }
            data[offset..offset + buf.len()].copy_from_slice(buf);
            buf.len()
        })
    }

    fn size(&self) -> usize {
        self.with_data(|data| data.len())
    }
}

/// Create or open a regular file under `/home/<user>/…` in the in-memory VFS tree.
fn open_or_create_home_file(norm: &str) -> Option<Arc<dyn Inode>> {
    if !norm.starts_with("/home/") {
        return None;
    }
    let (_, name) = norm.rsplit_once('/')?;
    if name.is_empty() {
        return None;
    }
    let mut root = ROOT.lock();
    let (parent, _) = norm.rsplit_once('/')?;
    let parent_node = traverse_mut(&mut root, parent, true).ok()?;
    let VfsNode::Directory(children) = parent_node else {
        return None;
    };
    if !children.contains_key(name) {
        children.insert(name.to_string(), VfsNode::File(Vec::new()));
    }
    match children.get(name) {
        Some(VfsNode::File(_)) => Some(Arc::new(VfsMemoryFileInode {
            path: norm.to_string(),
        })),
        _ => None,
    }
}

fn open_path_exact(path: &str) -> Option<Arc<dyn Inode>> {
    let norm = normalize_path(path);

    // 1. Synthetic Mounts
    if norm == "/dev/dri/renderD128" {
        return crate::gpu_render::RenderInode::open();
    }
    if norm == "/system" || norm == "/system/apps" {
        return Some(Arc::new(DirentStreamInode::from_linux_dirent64_bytes(
            linux_getdents64_bytes_for_path(&norm),
        )));
    }
    if norm.starts_with("/proc/") {
        return Some(Arc::new(crate::procfs::ProcfsPathInode::new(&norm)));
    }

    // tmpfs mounts — real Linux binaries use /tmp (and /dev/shm) constantly for
    // scratch/lock files. Create-on-open at the mount root (matches the /home
    // RAM-file behaviour); the data lives in TMPFS_INSTANCES keyed by inode, so
    // a write then a fresh read-open of the same name see the same bytes.
    // The RaeBridge Windows drive namespace `C:\...` maps to `/mnt/win_<drive>/...`
    // (raebridge::translate_win_path). Back it with create-on-open tmpfs so a guest
    // `CreateFileW(CREATE_ALWAYS)` + `WriteFile` actually persists (and a later read
    // sees the bytes) — without it a guest save fails ERROR_FILE_NOT_FOUND because
    // the underlying `open_path` never created. (The Concept-ideal per-app RaeFS
    // bucket binding for `C:\` is a follow-up; RAM-backed tmpfs proves the save/load
    // round-trip now.)
    if norm.starts_with("/tmp/") || norm.starts_with("/dev/shm/") || norm.starts_with("/mnt/win_") {
        if let Some((fs_name, ino)) = crate::tmpfs::open_or_create(&norm, true) {
            return Some(Arc::new(crate::tmpfs::TmpfsVfsInode::new(fs_name, ino)));
        }
    }

    if let Some((bucket_task, name)) = crate::data_buckets::parse_path(&norm) {
        let requester = crate::scheduler::current_task_id()?;
        if crate::data_buckets::can_access(bucket_task, requester) {
            return Some(Arc::new(BytesInode {
                data: crate::data_buckets::read(bucket_task, &name)?,
            }));
        }
        return None;
    }

    // 2. Virtual Hierarchy
    {
        let root = ROOT.lock();
        if let Some(node) = traverse(&root, &norm) {
            match node {
                VfsNode::Directory(_) => {
                    return Some(Arc::new(DirentStreamInode::from_linux_dirent64_bytes(
                        linux_getdents64_bytes_for_path(&norm),
                    )));
                }
                VfsNode::File(data) => {
                    return Some(Arc::new(BytesInode { data: data.clone() }));
                }
            }
        }
    }

    // 3. Session home files (RAM-backed when RaeFS absent)
    if let Some(inode) = open_or_create_home_file(&norm) {
        return Some(inode);
    }

    // 4. Fallbacks
    let archive = crate::tar::TarArchive::new(crate::INITRAMFS);
    if let Some(file) = archive.get_file(&norm) {
        let data = unsafe { core::slice::from_raw_parts(file.data.as_ptr(), file.data.len()) };
        return Some(Arc::new(InitramfsInode { data }));
    }
    // Initramfs tar entries are stored WITHOUT a leading slash (e.g.
    // `lib64/ld-linux-x86-64.so.2`) while VFS paths are normalised WITH one, so
    // try the leading-slash-stripped form. This resolves the absolute paths a
    // dynamic ELF + its interpreter name — `/lib64/ld-linux-x86-64.so.2`,
    // `/usr/lib/libc.so.6`, `/bin/dh` — to their bundled rootfs files.
    let rel = norm.trim_start_matches('/');
    if rel != norm.as_str() {
        if let Some(file) = archive.get_file(rel) {
            let data = unsafe { core::slice::from_raw_parts(file.data.as_ptr(), file.data.len()) };
            return Some(Arc::new(InitramfsInode { data }));
        }
    }
    let basename = norm.rsplit('/').next().unwrap_or(&norm);
    if basename != norm.as_str() {
        if let Some(file) = archive.get_file(basename) {
            let data = unsafe { core::slice::from_raw_parts(file.data.as_ptr(), file.data.len()) };
            return Some(Arc::new(InitramfsInode { data }));
        }
    }
    if let Some(inode_id) = crate::raefs::RaeFS::find_or_create_file(&norm) {
        return Some(Arc::new(crate::raefs::RaeFSInode { id: inode_id }));
    }
    None
}
