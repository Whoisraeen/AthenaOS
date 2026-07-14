//! tmpfs — in-memory filesystem for AthenaOS.
//!
//! Provides a POSIX-compatible in-memory filesystem mounted at `/tmp` and
//! `/dev/shm`. Data lives in `Vec<u8>` pages, supports hard links, symlinks,
//! permission bits, and configurable size limits. All data is lost on reboot.
//!
//! Implements the VFS `Inode` trait so tmpfs files can be used transparently
//! through the fd/syscall layer alongside AthFS and initramfs files.

#![allow(dead_code)]

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::string::ToString;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;

// ═══════════════════════════════════════════════════════════════════════════════
// Inode types and metadata
// ═══════════════════════════════════════════════════════════════════════════════

const PAGE_SIZE: usize = 4096;
const DEFAULT_MAX_SIZE: u64 = 128 * 1024 * 1024; // 128 MiB default

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TmpfsFileType {
    Regular,
    Directory,
    Symlink,
    CharDevice,
    BlockDevice,
    Fifo,
    Socket,
}

impl TmpfsFileType {
    pub fn mode_bits(&self) -> u32 {
        match self {
            Self::Regular => 0o100000,
            Self::Directory => 0o040000,
            Self::Symlink => 0o120000,
            Self::CharDevice => 0o020000,
            Self::BlockDevice => 0o060000,
            Self::Fifo => 0o010000,
            Self::Socket => 0o140000,
        }
    }

    pub fn dt_type(&self) -> u8 {
        match self {
            Self::Regular => 8,
            Self::Directory => 4,
            Self::Symlink => 10,
            Self::CharDevice => 2,
            Self::BlockDevice => 6,
            Self::Fifo => 1,
            Self::Socket => 12,
        }
    }
}

#[derive(Debug, Clone)]
pub struct TmpfsInodeData {
    pub ino: u64,
    pub file_type: TmpfsFileType,
    pub mode: u16,
    pub uid: u32,
    pub gid: u32,
    pub nlinks: u32,
    pub size: u64,
    pub atime: u64,
    pub mtime: u64,
    pub ctime: u64,
    pub data: Vec<u8>,
    pub children: BTreeMap<String, u64>,
    pub symlink_target: Option<String>,
    pub dev_major: u32,
    pub dev_minor: u32,
}

impl TmpfsInodeData {
    fn new(ino: u64, file_type: TmpfsFileType, mode: u16, uid: u32, gid: u32) -> Self {
        let nlinks = if file_type == TmpfsFileType::Directory {
            2
        } else {
            1
        };
        Self {
            ino,
            file_type,
            mode,
            uid,
            gid,
            nlinks,
            size: 0,
            atime: 0,
            mtime: 0,
            ctime: 0,
            data: Vec::new(),
            children: BTreeMap::new(),
            symlink_target: None,
            dev_major: 0,
            dev_minor: 0,
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// TmpfsFilesystem
// ═══════════════════════════════════════════════════════════════════════════════

pub struct TmpfsFilesystem {
    name: String,
    inodes: BTreeMap<u64, TmpfsInodeData>,
    next_ino: u64,
    total_used: u64,
    max_size: u64,
    mount_point: String,
}

impl TmpfsFilesystem {
    pub fn new(name: &str, max_size: u64) -> Self {
        let mut fs = Self {
            name: String::from(name),
            inodes: BTreeMap::new(),
            next_ino: 2,
            total_used: 0,
            max_size,
            mount_point: String::new(),
        };

        let mut root = TmpfsInodeData::new(1, TmpfsFileType::Directory, 0o1777, 0, 0);
        root.children.insert(String::from("."), 1);
        root.children.insert(String::from(".."), 1);
        fs.inodes.insert(1, root);
        fs
    }

    pub fn with_mount_point(mut self, mount: &str) -> Self {
        self.mount_point = String::from(mount);
        self
    }

    fn alloc_ino(&mut self) -> u64 {
        let ino = self.next_ino;
        self.next_ino += 1;
        ino
    }

    fn check_perm(&self, inode: &TmpfsInodeData, uid: u32, want_write: bool) -> bool {
        if uid == 0 {
            return true;
        }
        let mode = inode.mode;
        if uid == inode.uid {
            if want_write {
                mode & 0o200 != 0
            } else {
                mode & 0o400 != 0
            }
        } else if uid == inode.gid {
            if want_write {
                mode & 0o020 != 0
            } else {
                mode & 0o040 != 0
            }
        } else {
            if want_write {
                mode & 0o002 != 0
            } else {
                mode & 0o004 != 0
            }
        }
    }

    // ── File operations ────────────────────────────────────────────────

    pub fn create(
        &mut self,
        parent_ino: u64,
        name: &str,
        file_type: TmpfsFileType,
        mode: u16,
        uid: u32,
        gid: u32,
    ) -> Result<u64, TmpfsError> {
        if name.is_empty() || name.len() > 255 || name.contains('/') {
            return Err(TmpfsError::InvalidName);
        }
        if name == "." || name == ".." {
            return Err(TmpfsError::AlreadyExists);
        }

        let parent = self.inodes.get(&parent_ino).ok_or(TmpfsError::NotFound)?;
        if parent.file_type != TmpfsFileType::Directory {
            return Err(TmpfsError::NotADirectory);
        }
        if parent.children.contains_key(name) {
            return Err(TmpfsError::AlreadyExists);
        }

        let ino = self.alloc_ino();
        let mut node = TmpfsInodeData::new(ino, file_type, mode, uid, gid);

        if file_type == TmpfsFileType::Directory {
            node.children.insert(String::from("."), ino);
            node.children.insert(String::from(".."), parent_ino);
        }

        self.inodes.insert(ino, node);

        let parent = self.inodes.get_mut(&parent_ino).unwrap();
        parent.children.insert(String::from(name), ino);
        if file_type == TmpfsFileType::Directory {
            parent.nlinks += 1;
        }

        Ok(ino)
    }

    pub fn read_file(&self, ino: u64, offset: u64, buf: &mut [u8]) -> Result<usize, TmpfsError> {
        let node = self.inodes.get(&ino).ok_or(TmpfsError::NotFound)?;
        if node.file_type == TmpfsFileType::Directory {
            return Err(TmpfsError::IsADirectory);
        }
        if offset >= node.data.len() as u64 {
            return Ok(0);
        }
        let start = offset as usize;
        let avail = node.data.len() - start;
        let n = buf.len().min(avail);
        buf[..n].copy_from_slice(&node.data[start..start + n]);
        Ok(n)
    }

    pub fn write_file(&mut self, ino: u64, offset: u64, data: &[u8]) -> Result<usize, TmpfsError> {
        let end = offset as usize + data.len();

        {
            let node = self.inodes.get(&ino).ok_or(TmpfsError::NotFound)?;
            if node.file_type == TmpfsFileType::Directory {
                return Err(TmpfsError::IsADirectory);
            }
            if end > node.data.len() {
                let growth = (end - node.data.len()) as u64;
                if self.total_used + growth > self.max_size {
                    return Err(TmpfsError::NoSpace);
                }
            }
        }

        let node = self.inodes.get_mut(&ino).ok_or(TmpfsError::NotFound)?;
        let old_len = node.data.len();
        if end > old_len {
            node.data.resize(end, 0);
            self.total_used += (end - old_len) as u64;
        }
        node.data[offset as usize..end].copy_from_slice(data);
        node.size = node.data.len() as u64;
        Ok(data.len())
    }

    pub fn truncate(&mut self, ino: u64, size: u64) -> Result<(), TmpfsError> {
        let node = self.inodes.get_mut(&ino).ok_or(TmpfsError::NotFound)?;
        let old_size = node.data.len() as u64;

        if size > old_size {
            let growth = size - old_size;
            if self.total_used + growth > self.max_size {
                return Err(TmpfsError::NoSpace);
            }
            self.total_used += growth;
        } else {
            self.total_used -= old_size - size;
        }

        node.data.resize(size as usize, 0);
        node.size = size;
        Ok(())
    }

    pub fn unlink(&mut self, parent_ino: u64, name: &str) -> Result<(), TmpfsError> {
        if name == "." || name == ".." {
            return Err(TmpfsError::InvalidName);
        }

        let child_ino = {
            let parent = self.inodes.get(&parent_ino).ok_or(TmpfsError::NotFound)?;
            *parent.children.get(name).ok_or(TmpfsError::NotFound)?
        };

        let child = self.inodes.get(&child_ino).ok_or(TmpfsError::NotFound)?;
        if child.file_type == TmpfsFileType::Directory {
            return Err(TmpfsError::IsADirectory);
        }

        let nlinks = {
            let child = self.inodes.get_mut(&child_ino).unwrap();
            child.nlinks = child.nlinks.saturating_sub(1);
            child.nlinks
        };

        if nlinks == 0 {
            if let Some(removed) = self.inodes.remove(&child_ino) {
                self.total_used -= removed.data.len() as u64;
            }
        }

        let parent = self.inodes.get_mut(&parent_ino).unwrap();
        parent.children.remove(name);
        Ok(())
    }

    pub fn link(&mut self, parent_ino: u64, name: &str, target_ino: u64) -> Result<(), TmpfsError> {
        if name.is_empty() || name.len() > 255 || name.contains('/') {
            return Err(TmpfsError::InvalidName);
        }

        let target = self.inodes.get(&target_ino).ok_or(TmpfsError::NotFound)?;
        if target.file_type == TmpfsFileType::Directory {
            return Err(TmpfsError::IsADirectory);
        }

        let parent = self.inodes.get(&parent_ino).ok_or(TmpfsError::NotFound)?;
        if parent.file_type != TmpfsFileType::Directory {
            return Err(TmpfsError::NotADirectory);
        }
        if parent.children.contains_key(name) {
            return Err(TmpfsError::AlreadyExists);
        }

        self.inodes.get_mut(&target_ino).unwrap().nlinks += 1;
        self.inodes
            .get_mut(&parent_ino)
            .unwrap()
            .children
            .insert(String::from(name), target_ino);
        Ok(())
    }

    pub fn symlink(
        &mut self,
        parent_ino: u64,
        name: &str,
        target: &str,
        uid: u32,
        gid: u32,
    ) -> Result<u64, TmpfsError> {
        let ino = self.create(parent_ino, name, TmpfsFileType::Symlink, 0o777, uid, gid)?;
        let node = self.inodes.get_mut(&ino).unwrap();
        node.symlink_target = Some(String::from(target));
        node.size = target.len() as u64;
        Ok(ino)
    }

    pub fn readlink(&self, ino: u64) -> Result<String, TmpfsError> {
        let node = self.inodes.get(&ino).ok_or(TmpfsError::NotFound)?;
        if node.file_type != TmpfsFileType::Symlink {
            return Err(TmpfsError::NotASymlink);
        }
        node.symlink_target.clone().ok_or(TmpfsError::NotFound)
    }

    // ── Directory operations ───────────────────────────────────────────

    pub fn mkdir(
        &mut self,
        parent_ino: u64,
        name: &str,
        mode: u16,
        uid: u32,
        gid: u32,
    ) -> Result<u64, TmpfsError> {
        self.create(parent_ino, name, TmpfsFileType::Directory, mode, uid, gid)
    }

    pub fn rmdir(&mut self, parent_ino: u64, name: &str) -> Result<(), TmpfsError> {
        if name == "." || name == ".." {
            return Err(TmpfsError::InvalidName);
        }

        let child_ino = {
            let parent = self.inodes.get(&parent_ino).ok_or(TmpfsError::NotFound)?;
            *parent.children.get(name).ok_or(TmpfsError::NotFound)?
        };

        let child = self.inodes.get(&child_ino).ok_or(TmpfsError::NotFound)?;
        if child.file_type != TmpfsFileType::Directory {
            return Err(TmpfsError::NotADirectory);
        }
        let real_children = child
            .children
            .keys()
            .filter(|k| k.as_str() != "." && k.as_str() != "..")
            .count();
        if real_children > 0 {
            return Err(TmpfsError::NotEmpty);
        }

        self.inodes.remove(&child_ino);

        let parent = self.inodes.get_mut(&parent_ino).unwrap();
        parent.children.remove(name);
        parent.nlinks = parent.nlinks.saturating_sub(1);
        Ok(())
    }

    pub fn readdir(&self, ino: u64) -> Result<Vec<TmpfsDirEntry>, TmpfsError> {
        let node = self.inodes.get(&ino).ok_or(TmpfsError::NotFound)?;
        if node.file_type != TmpfsFileType::Directory {
            return Err(TmpfsError::NotADirectory);
        }

        let mut entries = Vec::new();
        for (name, &child_ino) in &node.children {
            let child_type = self
                .inodes
                .get(&child_ino)
                .map(|c| c.file_type)
                .unwrap_or(TmpfsFileType::Regular);
            entries.push(TmpfsDirEntry {
                ino: child_ino,
                name: name.clone(),
                file_type: child_type,
            });
        }
        Ok(entries)
    }

    pub fn lookup(&self, parent_ino: u64, name: &str) -> Result<u64, TmpfsError> {
        let parent = self.inodes.get(&parent_ino).ok_or(TmpfsError::NotFound)?;
        if parent.file_type != TmpfsFileType::Directory {
            return Err(TmpfsError::NotADirectory);
        }
        parent
            .children
            .get(name)
            .copied()
            .ok_or(TmpfsError::NotFound)
    }

    // ── Metadata operations ────────────────────────────────────────────

    pub fn stat(&self, ino: u64) -> Result<TmpfsStat, TmpfsError> {
        let node = self.inodes.get(&ino).ok_or(TmpfsError::NotFound)?;
        Ok(TmpfsStat {
            ino: node.ino,
            file_type: node.file_type,
            mode: node.mode,
            nlinks: node.nlinks,
            uid: node.uid,
            gid: node.gid,
            size: node.size,
            blocks: (node.size + 511) / 512,
            blksize: PAGE_SIZE as u64,
            atime: node.atime,
            mtime: node.mtime,
            ctime: node.ctime,
            dev_major: node.dev_major,
            dev_minor: node.dev_minor,
        })
    }

    pub fn chmod(&mut self, ino: u64, mode: u16) -> Result<(), TmpfsError> {
        let node = self.inodes.get_mut(&ino).ok_or(TmpfsError::NotFound)?;
        node.mode = mode;
        Ok(())
    }

    pub fn chown(&mut self, ino: u64, uid: u32, gid: u32) -> Result<(), TmpfsError> {
        let node = self.inodes.get_mut(&ino).ok_or(TmpfsError::NotFound)?;
        node.uid = uid;
        node.gid = gid;
        Ok(())
    }

    pub fn rename(
        &mut self,
        old_parent: u64,
        old_name: &str,
        new_parent: u64,
        new_name: &str,
    ) -> Result<(), TmpfsError> {
        if old_name == "." || old_name == ".." || new_name == "." || new_name == ".." {
            return Err(TmpfsError::InvalidName);
        }

        let child_ino = {
            let parent = self.inodes.get(&old_parent).ok_or(TmpfsError::NotFound)?;
            *parent.children.get(old_name).ok_or(TmpfsError::NotFound)?
        };

        if let Some(&existing) = self
            .inodes
            .get(&new_parent)
            .and_then(|p| p.children.get(new_name))
        {
            let existing_node = self.inodes.get(&existing).ok_or(TmpfsError::NotFound)?;
            if existing_node.file_type == TmpfsFileType::Directory {
                let real = existing_node
                    .children
                    .keys()
                    .filter(|k| k.as_str() != "." && k.as_str() != "..")
                    .count();
                if real > 0 {
                    return Err(TmpfsError::NotEmpty);
                }
            }
            self.inodes.remove(&existing);
        }

        self.inodes
            .get_mut(&old_parent)
            .unwrap()
            .children
            .remove(old_name);
        self.inodes
            .get_mut(&new_parent)
            .unwrap()
            .children
            .insert(String::from(new_name), child_ino);

        if self.inodes.get(&child_ino).map(|n| n.file_type) == Some(TmpfsFileType::Directory) {
            if let Some(node) = self.inodes.get_mut(&child_ino) {
                node.children.insert(String::from(".."), new_parent);
            }
        }

        Ok(())
    }

    // ── Info ───────────────────────────────────────────────────────────

    pub fn total_used(&self) -> u64 {
        self.total_used
    }
    pub fn max_size(&self) -> u64 {
        self.max_size
    }
    pub fn inode_count(&self) -> usize {
        self.inodes.len()
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn mount_point(&self) -> &str {
        &self.mount_point
    }

    pub fn statfs(&self) -> TmpfsStatFs {
        let total_blocks = self.max_size / PAGE_SIZE as u64;
        let used_blocks = self.total_used / PAGE_SIZE as u64;
        TmpfsStatFs {
            fs_type: 0x01021994, // TMPFS_MAGIC
            block_size: PAGE_SIZE as u64,
            total_blocks,
            free_blocks: total_blocks.saturating_sub(used_blocks),
            available_blocks: total_blocks.saturating_sub(used_blocks),
            total_inodes: u64::MAX,
            free_inodes: u64::MAX - self.next_ino,
            name_max: 255,
        }
    }

    pub fn resolve_path(&self, path: &str) -> Result<u64, TmpfsError> {
        let mut current = 1u64;
        for component in path.split('/').filter(|s| !s.is_empty()) {
            current = self.lookup(current, component)?;

            let node = self.inodes.get(&current).ok_or(TmpfsError::NotFound)?;
            if node.file_type == TmpfsFileType::Symlink {
                if let Some(ref target) = node.symlink_target {
                    if target.starts_with('/') {
                        return Err(TmpfsError::SymlinkLoop);
                    }
                }
            }
        }
        Ok(current)
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// VFS Inode adapter — lets tmpfs files be used via the fd layer
// ═══════════════════════════════════════════════════════════════════════════════

pub struct TmpfsVfsInode {
    fs_name: String,
    ino: u64,
}

impl TmpfsVfsInode {
    pub fn new(fs_name: &str, ino: u64) -> Self {
        Self {
            fs_name: String::from(fs_name),
            ino,
        }
    }
}

impl crate::vfs::Inode for TmpfsVfsInode {
    fn read_at(&self, offset: usize, buf: &mut [u8]) -> usize {
        let guard = TMPFS_INSTANCES.lock();
        if let Some(fs) = guard.get(&self.fs_name) {
            fs.read_file(self.ino, offset as u64, buf).unwrap_or(0)
        } else {
            0
        }
    }

    fn write_at(&self, offset: usize, buf: &[u8]) -> usize {
        let mut guard = TMPFS_INSTANCES.lock();
        if let Some(fs) = guard.get_mut(&self.fs_name) {
            fs.write_file(self.ino, offset as u64, buf).unwrap_or(0)
        } else {
            0
        }
    }

    fn size(&self) -> usize {
        let guard = TMPFS_INSTANCES.lock();
        if let Some(fs) = guard.get(&self.fs_name) {
            fs.stat(self.ino).map(|s| s.size as usize).unwrap_or(0)
        } else {
            0
        }
    }
}

/// Resolve a full VFS path under a tmpfs mount (`/tmp/<name>`, `/dev/shm/<name>`)
/// to `(fs_name, inode)`, creating a regular file at the mount root if `create`
/// and it is absent. Real Linux binaries lean on `/tmp` constantly (scratch
/// files, lock files, sockets), so the VFS routes those paths here. Flat
/// namespace at the mount root for now (no nested `/tmp/dir/file` yet — the
/// common case is a flat scratch name). Returns `None` if `path` is not under a
/// known tmpfs mount or the (flat) name is unusable.
pub fn open_or_create(path: &str, create: bool) -> Option<(&'static str, u64)> {
    // `rel` may be owned (the AthBridge Windows-drive case flattens a sub-path),
    // so carry a String and borrow it below.
    let (fs_name, rel): (&'static str, alloc::string::String) =
        if let Some(r) = path.strip_prefix("/tmp/") {
            ("tmp", r.into())
        } else if let Some(r) = path.strip_prefix("/dev/shm/") {
            ("dev_shm", r.into())
        } else if let Some(r) = path.strip_prefix("/mnt/win_") {
            // AthBridge `C:\...` -> `/mnt/win_c/...`. The tmpfs holds flat names, so
            // flatten the drive + sub-path into the shared "tmp" instance under a
            // `win_` prefix (e.g. `/mnt/win_c/out.txt` -> `win_c_out.txt`). Both the
            // guest CreateFileW and a later read translate to the same path, so the
            // flattened name is stable across open/save/read.
            ("tmp", alloc::format!("win_{}", r.replace('/', "_")))
        } else {
            return None;
        };
    let rel = rel.as_str();
    if rel.is_empty() || rel.contains('/') {
        return None; // flat names only for now
    }
    let mut guard = TMPFS_INSTANCES.lock();
    let fs = guard.get_mut(fs_name)?;
    if let Ok(ino) = fs.lookup(1, rel) {
        return Some((fs_name, ino));
    }
    if create {
        if let Ok(ino) = fs.create(1, rel, TmpfsFileType::Regular, 0o644, 0, 0) {
            return Some((fs_name, ino));
        }
    }
    None
}

// ═══════════════════════════════════════════════════════════════════════════════
// Supporting types
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TmpfsError {
    NotFound,
    AlreadyExists,
    NotADirectory,
    IsADirectory,
    NotASymlink,
    NotEmpty,
    InvalidName,
    NoSpace,
    PermissionDenied,
    ReadOnly,
    SymlinkLoop,
}

#[derive(Debug, Clone)]
pub struct TmpfsDirEntry {
    pub ino: u64,
    pub name: String,
    pub file_type: TmpfsFileType,
}

#[derive(Debug, Clone)]
pub struct TmpfsStat {
    pub ino: u64,
    pub file_type: TmpfsFileType,
    pub mode: u16,
    pub nlinks: u32,
    pub uid: u32,
    pub gid: u32,
    pub size: u64,
    pub blocks: u64,
    pub blksize: u64,
    pub atime: u64,
    pub mtime: u64,
    pub ctime: u64,
    pub dev_major: u32,
    pub dev_minor: u32,
}

#[derive(Debug, Clone)]
pub struct TmpfsStatFs {
    pub fs_type: u64,
    pub block_size: u64,
    pub total_blocks: u64,
    pub free_blocks: u64,
    pub available_blocks: u64,
    pub total_inodes: u64,
    pub free_inodes: u64,
    pub name_max: u64,
}

// ═══════════════════════════════════════════════════════════════════════════════
// Global instances
// ═══════════════════════════════════════════════════════════════════════════════

pub static TMPFS_INSTANCES: Mutex<BTreeMap<String, TmpfsFilesystem>> = Mutex::new(BTreeMap::new());

pub fn mount_tmpfs(name: &str, mount_point: &str, max_size: u64) {
    let fs = TmpfsFilesystem::new(name, max_size).with_mount_point(mount_point);
    TMPFS_INSTANCES.lock().insert(String::from(name), fs);
}

pub fn unmount_tmpfs(name: &str) {
    TMPFS_INSTANCES.lock().remove(name);
}

pub fn tmpfs_create(
    fs_name: &str,
    parent: u64,
    name: &str,
    file_type: TmpfsFileType,
    mode: u16,
) -> Result<u64, TmpfsError> {
    let mut guard = TMPFS_INSTANCES.lock();
    let fs = guard.get_mut(fs_name).ok_or(TmpfsError::NotFound)?;
    fs.create(parent, name, file_type, mode, 0, 0)
}

pub fn tmpfs_read(
    fs_name: &str,
    ino: u64,
    offset: u64,
    buf: &mut [u8],
) -> Result<usize, TmpfsError> {
    let guard = TMPFS_INSTANCES.lock();
    let fs = guard.get(fs_name).ok_or(TmpfsError::NotFound)?;
    fs.read_file(ino, offset, buf)
}

pub fn tmpfs_write(fs_name: &str, ino: u64, offset: u64, data: &[u8]) -> Result<usize, TmpfsError> {
    let mut guard = TMPFS_INSTANCES.lock();
    let fs = guard.get_mut(fs_name).ok_or(TmpfsError::NotFound)?;
    fs.write_file(ino, offset, data)
}

pub fn tmpfs_unlink(fs_name: &str, parent: u64, name: &str) -> Result<(), TmpfsError> {
    let mut guard = TMPFS_INSTANCES.lock();
    let fs = guard.get_mut(fs_name).ok_or(TmpfsError::NotFound)?;
    fs.unlink(parent, name)
}

pub fn tmpfs_mkdir(fs_name: &str, parent: u64, name: &str, mode: u16) -> Result<u64, TmpfsError> {
    let mut guard = TMPFS_INSTANCES.lock();
    let fs = guard.get_mut(fs_name).ok_or(TmpfsError::NotFound)?;
    fs.mkdir(parent, name, mode, 0, 0)
}

pub fn tmpfs_rmdir(fs_name: &str, parent: u64, name: &str) -> Result<(), TmpfsError> {
    let mut guard = TMPFS_INSTANCES.lock();
    let fs = guard.get_mut(fs_name).ok_or(TmpfsError::NotFound)?;
    fs.rmdir(parent, name)
}

pub fn tmpfs_stat(fs_name: &str, ino: u64) -> Result<TmpfsStat, TmpfsError> {
    let guard = TMPFS_INSTANCES.lock();
    let fs = guard.get(fs_name).ok_or(TmpfsError::NotFound)?;
    fs.stat(ino)
}

pub fn tmpfs_readdir(fs_name: &str, ino: u64) -> Result<Vec<TmpfsDirEntry>, TmpfsError> {
    let guard = TMPFS_INSTANCES.lock();
    let fs = guard.get(fs_name).ok_or(TmpfsError::NotFound)?;
    fs.readdir(ino)
}

pub fn init() {
    mount_tmpfs("tmp", "/tmp", DEFAULT_MAX_SIZE);
    mount_tmpfs("dev_shm", "/dev/shm", DEFAULT_MAX_SIZE);
    crate::serial_println!("[ OK ] tmpfs mounted at /tmp and /dev/shm (128 MiB each)");
}
