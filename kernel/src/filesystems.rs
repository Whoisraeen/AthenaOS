#![allow(dead_code)]

extern crate alloc;

use alloc::format;
use alloc::string::ToString;
use alloc::vec;
use alloc::{boxed::Box, collections::BTreeMap, string::String, vec::Vec};
use spin::Mutex;

// ═══════════════════════════════════════════════════════════════════════════════
// §1  VFS LAYER
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FsError {
    NotFound,
    PermissionDenied,
    AlreadyExists,
    NotADirectory,
    IsADirectory,
    NotEmpty,
    InvalidName,
    NoSpace,
    IoError,
    ReadOnly,
    NotMounted,
    AlreadyMounted,
    InvalidSuperblock,
    CorruptedFs,
    CrossDevice,
    NotSupported,
    InvalidOffset,
    BufferTooSmall,
    SymlinkLoop,
    NameTooLong,
    Busy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileType {
    Regular,
    Directory,
    Symlink,
    CharDevice,
    BlockDevice,
    Fifo,
    Socket,
}

#[derive(Debug, Clone)]
pub struct InodeInfo {
    pub inode: u64,
    pub file_type: FileType,
    pub mode: u16,
    pub uid: u32,
    pub gid: u32,
    pub size: u64,
    pub blocks: u64,
    pub block_size: u32,
    pub atime: u64,
    pub mtime: u64,
    pub ctime: u64,
    pub crtime: u64,
    pub nlinks: u32,
    pub flags: u32,
}

#[derive(Debug, Clone)]
pub struct DirEntry {
    pub inode: u64,
    pub name: String,
    pub file_type: FileType,
    pub offset: u64,
}

#[derive(Debug, Clone)]
pub struct FsStats {
    pub total_blocks: u64,
    pub free_blocks: u64,
    pub available_blocks: u64,
    pub total_inodes: u64,
    pub free_inodes: u64,
    pub block_size: u32,
    pub max_name_length: u32,
    pub fs_type: String,
}

pub trait Filesystem: Send {
    fn name(&self) -> &str;
    fn mount(&mut self, device: u64) -> Result<(), FsError>;
    fn unmount(&mut self) -> Result<(), FsError>;
    fn stat_fs(&self) -> Result<FsStats, FsError>;
    fn lookup(&self, parent: u64, name: &str) -> Result<InodeInfo, FsError>;
    fn read_dir(&self, inode: u64) -> Result<Vec<DirEntry>, FsError>;
    fn read_file(&self, inode: u64, offset: u64, buf: &mut [u8]) -> Result<usize, FsError>;
    fn write_file(&mut self, inode: u64, offset: u64, data: &[u8]) -> Result<usize, FsError>;
    fn create(
        &mut self,
        parent: u64,
        name: &str,
        file_type: FileType,
        mode: u16,
    ) -> Result<u64, FsError>;
    fn unlink(&mut self, parent: u64, name: &str) -> Result<(), FsError>;
    fn mkdir(&mut self, parent: u64, name: &str, mode: u16) -> Result<u64, FsError>;
    fn rmdir(&mut self, parent: u64, name: &str) -> Result<(), FsError>;
    fn rename(
        &mut self,
        old_parent: u64,
        old_name: &str,
        new_parent: u64,
        new_name: &str,
    ) -> Result<(), FsError>;
    fn truncate(&mut self, inode: u64, size: u64) -> Result<(), FsError>;
    fn symlink(&mut self, parent: u64, name: &str, target: &str) -> Result<u64, FsError>;
    fn readlink(&self, inode: u64) -> Result<String, FsError>;
    fn chmod(&mut self, inode: u64, mode: u16) -> Result<(), FsError>;
    fn chown(&mut self, inode: u64, uid: u32, gid: u32) -> Result<(), FsError>;
    fn sync(&mut self) -> Result<(), FsError>;
}

pub struct MountFlags {
    pub read_only: bool,
    pub no_exec: bool,
    pub no_suid: bool,
    pub no_dev: bool,
    pub no_atime: bool,
    pub sync: bool,
}

impl MountFlags {
    pub fn new() -> Self {
        Self {
            read_only: false,
            no_exec: false,
            no_suid: false,
            no_dev: false,
            no_atime: false,
            sync: false,
        }
    }

    pub fn read_only() -> Self {
        Self {
            read_only: true,
            ..Self::new()
        }
    }
}

pub struct MountPoint {
    pub path: String,
    pub device: String,
    pub fs_type: String,
    pub flags: MountFlags,
    pub filesystem: Box<dyn Filesystem + Send>,
}

pub struct Vfs {
    mounts: BTreeMap<String, MountPoint>,
    path_cache: BTreeMap<String, (String, u64)>,
}

impl Vfs {
    pub fn new() -> Self {
        Self {
            mounts: BTreeMap::new(),
            path_cache: BTreeMap::new(),
        }
    }

    pub fn mount(
        &mut self,
        path: &str,
        device: &str,
        fs_type: &str,
        flags: MountFlags,
    ) -> Result<(), FsError> {
        if self.mounts.contains_key(path) {
            return Err(FsError::AlreadyMounted);
        }

        let mut fs: Box<dyn Filesystem + Send> = match fs_type {
            "tmpfs" => Box::new(TmpFs::new(64 * 1024 * 1024)),
            "sysfs" => Box::new(SysFs::new()),
            "ext4" => Box::new(Ext4Fs::new()),
            "fat32" | "vfat" => Box::new(Fat32Fs::new()),
            "ntfs" => Box::new(NtfsFs::new()),
            _ => return Err(FsError::NotSupported),
        };

        fs.mount(0)?;

        self.mounts.insert(
            path.to_string(),
            MountPoint {
                path: path.to_string(),
                device: device.to_string(),
                fs_type: fs_type.to_string(),
                flags,
                filesystem: fs,
            },
        );

        Ok(())
    }

    pub fn unmount(&mut self, path: &str) -> Result<(), FsError> {
        if let Some(mut mp) = self.mounts.remove(path) {
            mp.filesystem.unmount()?;
            self.path_cache.retain(|k, _| !k.starts_with(path));
            Ok(())
        } else {
            Err(FsError::NotMounted)
        }
    }

    pub fn resolve_path(&self, path: &str) -> Result<(&MountPoint, u64), FsError> {
        if let Some((mount_path, inode)) = self.path_cache.get(path) {
            if let Some(mp) = self.mounts.get(mount_path.as_str()) {
                return Ok((mp, *inode));
            }
        }

        let mut best_match = "";
        let mut best_len = 0;

        for mount_path in self.mounts.keys() {
            if path.starts_with(mount_path.as_str()) && mount_path.len() > best_len {
                best_match = mount_path.as_str();
                best_len = mount_path.len();
            }
        }

        if best_len == 0 {
            return Err(FsError::NotMounted);
        }

        let mp = self.mounts.get(best_match).unwrap();
        let relative = &path[best_len..];
        let relative = relative.trim_start_matches('/');

        if relative.is_empty() {
            return Ok((mp, 1));
        }

        let mut current_inode = 1u64;
        for component in relative.split('/') {
            if component.is_empty() {
                continue;
            }
            let info = mp.filesystem.lookup(current_inode, component)?;
            current_inode = info.inode;
        }

        Ok((mp, current_inode))
    }

    pub fn open(&self, path: &str) -> Result<InodeInfo, FsError> {
        let (mp, inode) = self.resolve_path(path)?;
        let parent_path = path.rsplit_once('/').map(|(p, _)| p).unwrap_or("/");
        let name = path.rsplit_once('/').map(|(_, n)| n).unwrap_or(path);

        if inode == 1 && name.is_empty() {
            return mp.filesystem.lookup(1, ".");
        }

        let (parent_mp, parent_inode) = self.resolve_path(parent_path)?;
        parent_mp.filesystem.lookup(parent_inode, name)
    }

    pub fn read(&self, path: &str, offset: u64, buf: &mut [u8]) -> Result<usize, FsError> {
        let (mp, inode) = self.resolve_path(path)?;
        if mp.flags.read_only {
            // read is fine on read-only
        }
        mp.filesystem.read_file(inode, offset, buf)
    }

    pub fn list_mounts(&self) -> Vec<&MountPoint> {
        self.mounts.values().collect()
    }

    pub fn stat(&self, path: &str) -> Result<FsStats, FsError> {
        let (mp, _) = self.resolve_path(path)?;
        mp.filesystem.stat_fs()
    }

    pub fn read_dir(&self, path: &str) -> Result<Vec<DirEntry>, FsError> {
        let (mp, inode) = self.resolve_path(path)?;
        mp.filesystem.read_dir(inode)
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// §2  EXT4 READER
// ═══════════════════════════════════════════════════════════════════════════════

pub struct Ext4Superblock {
    pub inodes_count: u32,
    pub blocks_count_lo: u32,
    pub r_blocks_count_lo: u32,
    pub free_blocks_count_lo: u32,
    pub free_inodes_count_lo: u32,
    pub first_data_block: u32,
    pub log_block_size: u32,
    pub log_cluster_size: u32,
    pub blocks_per_group: u32,
    pub clusters_per_group: u32,
    pub inodes_per_group: u32,
    pub mtime: u32,
    pub wtime: u32,
    pub mnt_count: u16,
    pub max_mnt_count: u16,
    pub magic: u16,
    pub state: u16,
    pub errors: u16,
    pub minor_rev_level: u16,
    pub lastcheck: u32,
    pub checkinterval: u32,
    pub creator_os: u32,
    pub rev_level: u32,
    pub def_resuid: u16,
    pub def_resgid: u16,
    pub first_ino: u32,
    pub inode_size: u16,
    pub block_group_nr: u16,
    pub feature_compat: u32,
    pub feature_incompat: u32,
    pub feature_ro_compat: u32,
    pub uuid: [u8; 16],
    pub volume_name: [u8; 16],
    pub blocks_count_hi: u32,
    pub r_blocks_count_hi: u32,
    pub free_blocks_count_hi: u32,
    pub min_extra_isize: u16,
    pub want_extra_isize: u16,
}

impl Ext4Superblock {
    fn new() -> Self {
        Self {
            inodes_count: 0,
            blocks_count_lo: 0,
            r_blocks_count_lo: 0,
            free_blocks_count_lo: 0,
            free_inodes_count_lo: 0,
            first_data_block: 0,
            log_block_size: 0,
            log_cluster_size: 0,
            blocks_per_group: 0,
            clusters_per_group: 0,
            inodes_per_group: 0,
            mtime: 0,
            wtime: 0,
            mnt_count: 0,
            max_mnt_count: 0,
            magic: 0,
            state: 0,
            errors: 0,
            minor_rev_level: 0,
            lastcheck: 0,
            checkinterval: 0,
            creator_os: 0,
            rev_level: 0,
            def_resuid: 0,
            def_resgid: 0,
            first_ino: 0,
            inode_size: 0,
            block_group_nr: 0,
            feature_compat: 0,
            feature_incompat: 0,
            feature_ro_compat: 0,
            uuid: [0; 16],
            volume_name: [0; 16],
            blocks_count_hi: 0,
            r_blocks_count_hi: 0,
            free_blocks_count_hi: 0,
            min_extra_isize: 0,
            want_extra_isize: 0,
        }
    }

    fn total_blocks(&self) -> u64 {
        (self.blocks_count_hi as u64) << 32 | self.blocks_count_lo as u64
    }

    fn total_free_blocks(&self) -> u64 {
        (self.free_blocks_count_hi as u64) << 32 | self.free_blocks_count_lo as u64
    }
}

pub struct Ext4Inode {
    pub mode: u16,
    pub uid: u16,
    pub size_lo: u32,
    pub atime: u32,
    pub ctime: u32,
    pub mtime: u32,
    pub dtime: u32,
    pub gid: u16,
    pub links_count: u16,
    pub blocks_lo: u32,
    pub flags: u32,
    pub block: [u32; 15],
    pub generation: u32,
    pub file_acl_lo: u32,
    pub size_high: u32,
    pub extra_isize: u16,
    pub crtime: u32,
    pub crtime_extra: u32,
}

impl Ext4Inode {
    fn new() -> Self {
        Self {
            mode: 0,
            uid: 0,
            size_lo: 0,
            atime: 0,
            ctime: 0,
            mtime: 0,
            dtime: 0,
            gid: 0,
            links_count: 0,
            blocks_lo: 0,
            flags: 0,
            block: [0; 15],
            generation: 0,
            file_acl_lo: 0,
            size_high: 0,
            extra_isize: 0,
            crtime: 0,
            crtime_extra: 0,
        }
    }

    fn size(&self) -> u64 {
        (self.size_high as u64) << 32 | self.size_lo as u64
    }

    fn file_type(&self) -> FileType {
        match self.mode >> 12 {
            0x1 => FileType::Fifo,
            0x2 => FileType::CharDevice,
            0x4 => FileType::Directory,
            0x6 => FileType::BlockDevice,
            0x8 => FileType::Regular,
            0xA => FileType::Symlink,
            0xC => FileType::Socket,
            _ => FileType::Regular,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Ext4GroupDesc {
    pub block_bitmap_lo: u32,
    pub inode_bitmap_lo: u32,
    pub inode_table_lo: u32,
    pub free_blocks_count_lo: u16,
    pub free_inodes_count_lo: u16,
    pub used_dirs_count_lo: u16,
    pub flags: u16,
    pub checksum: u16,
    pub block_bitmap_hi: u32,
    pub inode_bitmap_hi: u32,
    pub inode_table_hi: u32,
}

impl Ext4GroupDesc {
    fn new() -> Self {
        Self {
            block_bitmap_lo: 0,
            inode_bitmap_lo: 0,
            inode_table_lo: 0,
            free_blocks_count_lo: 0,
            free_inodes_count_lo: 0,
            used_dirs_count_lo: 0,
            flags: 0,
            checksum: 0,
            block_bitmap_hi: 0,
            inode_bitmap_hi: 0,
            inode_table_hi: 0,
        }
    }

    fn inode_table(&self) -> u64 {
        (self.inode_table_hi as u64) << 32 | self.inode_table_lo as u64
    }
}

pub struct Ext4ExtentHeader {
    magic: u16,
    entries: u16,
    max: u16,
    depth: u16,
    generation: u32,
}

pub struct Ext4ExtentIdx {
    block: u32,
    leaf_lo: u32,
    leaf_hi: u16,
}

impl Ext4ExtentIdx {
    fn leaf(&self) -> u64 {
        (self.leaf_hi as u64) << 32 | self.leaf_lo as u64
    }
}

pub struct Ext4Extent {
    block: u32,
    len: u16,
    start_hi: u16,
    start_lo: u32,
}

impl Ext4Extent {
    fn start(&self) -> u64 {
        (self.start_hi as u64) << 32 | self.start_lo as u64
    }

    fn is_uninitialized(&self) -> bool {
        self.len > 32768
    }

    fn actual_len(&self) -> u32 {
        if self.is_uninitialized() {
            (self.len - 32768) as u32
        } else {
            self.len as u32
        }
    }
}

pub struct Ext4DirEntry {
    inode: u32,
    rec_len: u16,
    name_len: u8,
    file_type: u8,
    name: String,
}

pub struct Ext4Fs {
    device_id: u64,
    superblock: Ext4Superblock,
    group_descs: Vec<Ext4GroupDesc>,
    block_size: u32,
    inode_size: u16,
    inodes_per_group: u32,
    blocks_per_group: u32,
    mounted: bool,
    block_cache: BTreeMap<u64, Vec<u8>>,
}

impl Ext4Fs {
    pub fn new() -> Self {
        Self {
            device_id: 0,
            superblock: Ext4Superblock::new(),
            group_descs: Vec::new(),
            block_size: 4096,
            inode_size: 256,
            inodes_per_group: 0,
            blocks_per_group: 0,
            mounted: false,
            block_cache: BTreeMap::new(),
        }
    }

    fn parse_superblock(&mut self, data: &[u8]) -> Result<(), FsError> {
        if data.len() < 1024 {
            return Err(FsError::InvalidSuperblock);
        }

        let sb_offset = 1024;
        let sb = &data[sb_offset..];

        self.superblock.inodes_count = u32::from_le_bytes([sb[0], sb[1], sb[2], sb[3]]);
        self.superblock.blocks_count_lo = u32::from_le_bytes([sb[4], sb[5], sb[6], sb[7]]);
        self.superblock.r_blocks_count_lo = u32::from_le_bytes([sb[8], sb[9], sb[10], sb[11]]);
        self.superblock.free_blocks_count_lo = u32::from_le_bytes([sb[12], sb[13], sb[14], sb[15]]);
        self.superblock.free_inodes_count_lo = u32::from_le_bytes([sb[16], sb[17], sb[18], sb[19]]);
        self.superblock.first_data_block = u32::from_le_bytes([sb[20], sb[21], sb[22], sb[23]]);
        self.superblock.log_block_size = u32::from_le_bytes([sb[24], sb[25], sb[26], sb[27]]);
        self.superblock.log_cluster_size = u32::from_le_bytes([sb[28], sb[29], sb[30], sb[31]]);
        self.superblock.blocks_per_group = u32::from_le_bytes([sb[32], sb[33], sb[34], sb[35]]);
        self.superblock.clusters_per_group = u32::from_le_bytes([sb[36], sb[37], sb[38], sb[39]]);
        self.superblock.inodes_per_group = u32::from_le_bytes([sb[40], sb[41], sb[42], sb[43]]);
        self.superblock.mtime = u32::from_le_bytes([sb[44], sb[45], sb[46], sb[47]]);
        self.superblock.wtime = u32::from_le_bytes([sb[48], sb[49], sb[50], sb[51]]);
        self.superblock.mnt_count = u16::from_le_bytes([sb[52], sb[53]]);
        self.superblock.max_mnt_count = u16::from_le_bytes([sb[54], sb[55]]);
        self.superblock.magic = u16::from_le_bytes([sb[56], sb[57]]);

        if self.superblock.magic != 0xEF53 {
            return Err(FsError::InvalidSuperblock);
        }

        self.superblock.state = u16::from_le_bytes([sb[58], sb[59]]);
        self.superblock.errors = u16::from_le_bytes([sb[60], sb[61]]);
        self.superblock.minor_rev_level = u16::from_le_bytes([sb[62], sb[63]]);
        self.superblock.lastcheck = u32::from_le_bytes([sb[64], sb[65], sb[66], sb[67]]);
        self.superblock.checkinterval = u32::from_le_bytes([sb[68], sb[69], sb[70], sb[71]]);
        self.superblock.creator_os = u32::from_le_bytes([sb[72], sb[73], sb[74], sb[75]]);
        self.superblock.rev_level = u32::from_le_bytes([sb[76], sb[77], sb[78], sb[79]]);
        self.superblock.def_resuid = u16::from_le_bytes([sb[80], sb[81]]);
        self.superblock.def_resgid = u16::from_le_bytes([sb[82], sb[83]]);
        self.superblock.first_ino = u32::from_le_bytes([sb[84], sb[85], sb[86], sb[87]]);
        self.superblock.inode_size = u16::from_le_bytes([sb[88], sb[89]]);
        self.superblock.block_group_nr = u16::from_le_bytes([sb[90], sb[91]]);
        self.superblock.feature_compat = u32::from_le_bytes([sb[92], sb[93], sb[94], sb[95]]);
        self.superblock.feature_incompat = u32::from_le_bytes([sb[96], sb[97], sb[98], sb[99]]);
        self.superblock.feature_ro_compat =
            u32::from_le_bytes([sb[100], sb[101], sb[102], sb[103]]);
        self.superblock.uuid.copy_from_slice(&sb[104..120]);
        self.superblock.volume_name.copy_from_slice(&sb[120..136]);

        self.block_size = 1024 << self.superblock.log_block_size;
        self.inode_size = self.superblock.inode_size;
        self.inodes_per_group = self.superblock.inodes_per_group;
        self.blocks_per_group = self.superblock.blocks_per_group;

        Ok(())
    }

    fn read_group_desc(&mut self, data: &[u8]) -> Result<(), FsError> {
        let gdt_block = if self.block_size == 1024 { 2 } else { 1 };
        let gdt_offset = (gdt_block as usize) * (self.block_size as usize);
        let num_groups =
            (self.superblock.blocks_count_lo + self.blocks_per_group - 1) / self.blocks_per_group;

        self.group_descs.clear();
        for i in 0..num_groups as usize {
            let offset = gdt_offset + i * 64;
            if offset + 64 > data.len() {
                break;
            }
            let gd = &data[offset..];
            self.group_descs.push(Ext4GroupDesc {
                block_bitmap_lo: u32::from_le_bytes([gd[0], gd[1], gd[2], gd[3]]),
                inode_bitmap_lo: u32::from_le_bytes([gd[4], gd[5], gd[6], gd[7]]),
                inode_table_lo: u32::from_le_bytes([gd[8], gd[9], gd[10], gd[11]]),
                free_blocks_count_lo: u16::from_le_bytes([gd[12], gd[13]]),
                free_inodes_count_lo: u16::from_le_bytes([gd[14], gd[15]]),
                used_dirs_count_lo: u16::from_le_bytes([gd[16], gd[17]]),
                flags: u16::from_le_bytes([gd[18], gd[19]]),
                checksum: u16::from_le_bytes([gd[30], gd[31]]),
                block_bitmap_hi: u32::from_le_bytes([gd[32], gd[33], gd[34], gd[35]]),
                inode_bitmap_hi: u32::from_le_bytes([gd[36], gd[37], gd[38], gd[39]]),
                inode_table_hi: u32::from_le_bytes([gd[40], gd[41], gd[42], gd[43]]),
            });
        }

        Ok(())
    }

    fn read_inode(&self, inode_num: u64, data: &[u8]) -> Result<Ext4Inode, FsError> {
        if inode_num == 0 {
            return Err(FsError::NotFound);
        }
        let inode_idx = (inode_num - 1) as u32;
        let group = inode_idx / self.inodes_per_group;
        let local_idx = inode_idx % self.inodes_per_group;

        if group as usize >= self.group_descs.len() {
            return Err(FsError::NotFound);
        }

        let inode_table = self.group_descs[group as usize].inode_table();
        let offset =
            (inode_table * self.block_size as u64) + (local_idx as u64 * self.inode_size as u64);

        if offset as usize + self.inode_size as usize > data.len() {
            return Err(FsError::IoError);
        }

        let raw = &data[offset as usize..];
        let mut inode = Ext4Inode::new();
        inode.mode = u16::from_le_bytes([raw[0], raw[1]]);
        inode.uid = u16::from_le_bytes([raw[2], raw[3]]);
        inode.size_lo = u32::from_le_bytes([raw[4], raw[5], raw[6], raw[7]]);
        inode.atime = u32::from_le_bytes([raw[8], raw[9], raw[10], raw[11]]);
        inode.ctime = u32::from_le_bytes([raw[12], raw[13], raw[14], raw[15]]);
        inode.mtime = u32::from_le_bytes([raw[16], raw[17], raw[18], raw[19]]);
        inode.dtime = u32::from_le_bytes([raw[20], raw[21], raw[22], raw[23]]);
        inode.gid = u16::from_le_bytes([raw[24], raw[25]]);
        inode.links_count = u16::from_le_bytes([raw[26], raw[27]]);
        inode.blocks_lo = u32::from_le_bytes([raw[28], raw[29], raw[30], raw[31]]);
        inode.flags = u32::from_le_bytes([raw[32], raw[33], raw[34], raw[35]]);

        for i in 0..15 {
            let off = 40 + i * 4;
            inode.block[i] =
                u32::from_le_bytes([raw[off], raw[off + 1], raw[off + 2], raw[off + 3]]);
        }

        inode.generation = u32::from_le_bytes([raw[100], raw[101], raw[102], raw[103]]);
        inode.file_acl_lo = u32::from_le_bytes([raw[104], raw[105], raw[106], raw[107]]);
        inode.size_high = u32::from_le_bytes([raw[108], raw[109], raw[110], raw[111]]);

        if self.inode_size > 128 {
            inode.extra_isize = u16::from_le_bytes([raw[128], raw[129]]);
        }

        Ok(inode)
    }

    fn read_extent_tree(
        &self,
        inode: &Ext4Inode,
        logical_block: u64,
        data: &[u8],
    ) -> Result<u64, FsError> {
        let block_data =
            unsafe { core::slice::from_raw_parts(inode.block.as_ptr() as *const u8, 60) };

        let header = Ext4ExtentHeader {
            magic: u16::from_le_bytes([block_data[0], block_data[1]]),
            entries: u16::from_le_bytes([block_data[2], block_data[3]]),
            max: u16::from_le_bytes([block_data[4], block_data[5]]),
            depth: u16::from_le_bytes([block_data[6], block_data[7]]),
            generation: u32::from_le_bytes([
                block_data[8],
                block_data[9],
                block_data[10],
                block_data[11],
            ]),
        };

        if header.magic != 0xF30A {
            return Err(FsError::CorruptedFs);
        }

        if header.depth == 0 {
            for i in 0..header.entries as usize {
                let off = 12 + i * 12;
                let extent = Ext4Extent {
                    block: u32::from_le_bytes([
                        block_data[off],
                        block_data[off + 1],
                        block_data[off + 2],
                        block_data[off + 3],
                    ]),
                    len: u16::from_le_bytes([block_data[off + 4], block_data[off + 5]]),
                    start_hi: u16::from_le_bytes([block_data[off + 6], block_data[off + 7]]),
                    start_lo: u32::from_le_bytes([
                        block_data[off + 8],
                        block_data[off + 9],
                        block_data[off + 10],
                        block_data[off + 11],
                    ]),
                };

                if logical_block >= extent.block as u64
                    && logical_block < extent.block as u64 + extent.actual_len() as u64
                {
                    let offset_in_extent = logical_block - extent.block as u64;
                    return Ok(extent.start() + offset_in_extent);
                }
            }
            Err(FsError::NotFound)
        } else {
            for i in 0..header.entries as usize {
                let off = 12 + i * 12;
                let _idx = Ext4ExtentIdx {
                    block: u32::from_le_bytes([
                        block_data[off],
                        block_data[off + 1],
                        block_data[off + 2],
                        block_data[off + 3],
                    ]),
                    leaf_lo: u32::from_le_bytes([
                        block_data[off + 4],
                        block_data[off + 5],
                        block_data[off + 6],
                        block_data[off + 7],
                    ]),
                    leaf_hi: u16::from_le_bytes([block_data[off + 8], block_data[off + 9]]),
                };
            }
            Err(FsError::NotSupported)
        }
    }

    fn read_block<'a>(&self, block_num: u64, data: &'a [u8]) -> Result<&'a [u8], FsError> {
        let offset = block_num * self.block_size as u64;
        let end = offset + self.block_size as u64;
        if end as usize > data.len() {
            return Err(FsError::IoError);
        }
        Ok(&data[offset as usize..end as usize])
    }

    fn read_dir_entries(
        &self,
        inode: &Ext4Inode,
        data: &[u8],
    ) -> Result<Vec<Ext4DirEntry>, FsError> {
        let mut entries = Vec::new();
        let size = inode.size();
        let num_blocks = (size + self.block_size as u64 - 1) / self.block_size as u64;

        for logical_block in 0..num_blocks {
            let phys_block = match self.read_extent_tree(inode, logical_block, data) {
                Ok(b) => b,
                Err(_) => continue,
            };

            let block_data = match self.read_block(phys_block, data) {
                Ok(d) => d,
                Err(_) => continue,
            };

            let mut pos = 0;
            while pos + 8 <= block_data.len() {
                let entry_inode = u32::from_le_bytes([
                    block_data[pos],
                    block_data[pos + 1],
                    block_data[pos + 2],
                    block_data[pos + 3],
                ]);
                let rec_len = u16::from_le_bytes([block_data[pos + 4], block_data[pos + 5]]);
                let name_len = block_data[pos + 6];
                let file_type = block_data[pos + 7];

                if rec_len == 0 {
                    break;
                }

                if entry_inode != 0 && name_len > 0 {
                    let name_end = pos + 8 + name_len as usize;
                    if name_end <= block_data.len() {
                        let name =
                            String::from_utf8_lossy(&block_data[pos + 8..name_end]).to_string();
                        entries.push(Ext4DirEntry {
                            inode: entry_inode,
                            rec_len,
                            name_len,
                            file_type,
                            name,
                        });
                    }
                }

                pos += rec_len as usize;
            }
        }

        Ok(entries)
    }

    fn ext4_file_type(ft: u8) -> FileType {
        match ft {
            1 => FileType::Regular,
            2 => FileType::Directory,
            3 => FileType::CharDevice,
            4 => FileType::BlockDevice,
            5 => FileType::Fifo,
            6 => FileType::Socket,
            7 => FileType::Symlink,
            _ => FileType::Regular,
        }
    }
}

impl Filesystem for Ext4Fs {
    fn name(&self) -> &str {
        "ext4"
    }

    fn mount(&mut self, device: u64) -> Result<(), FsError> {
        self.device_id = device;
        self.mounted = true;
        Ok(())
    }

    fn unmount(&mut self) -> Result<(), FsError> {
        self.mounted = false;
        self.block_cache.clear();
        Ok(())
    }

    fn stat_fs(&self) -> Result<FsStats, FsError> {
        if !self.mounted {
            return Err(FsError::NotMounted);
        }
        Ok(FsStats {
            total_blocks: self.superblock.total_blocks(),
            free_blocks: self.superblock.total_free_blocks(),
            available_blocks: self.superblock.total_free_blocks(),
            total_inodes: self.superblock.inodes_count as u64,
            free_inodes: self.superblock.free_inodes_count_lo as u64,
            block_size: self.block_size,
            max_name_length: 255,
            fs_type: "ext4".to_string(),
        })
    }

    fn lookup(&self, _parent: u64, _name: &str) -> Result<InodeInfo, FsError> {
        if !self.mounted {
            return Err(FsError::NotMounted);
        }
        Err(FsError::NotSupported)
    }

    fn read_dir(&self, _inode: u64) -> Result<Vec<DirEntry>, FsError> {
        if !self.mounted {
            return Err(FsError::NotMounted);
        }
        Err(FsError::NotSupported)
    }

    fn read_file(&self, _inode: u64, _offset: u64, _buf: &mut [u8]) -> Result<usize, FsError> {
        if !self.mounted {
            return Err(FsError::NotMounted);
        }
        Err(FsError::NotSupported)
    }

    fn write_file(&mut self, _inode: u64, _offset: u64, _data: &[u8]) -> Result<usize, FsError> {
        Err(FsError::ReadOnly)
    }

    fn create(
        &mut self,
        _parent: u64,
        _name: &str,
        _file_type: FileType,
        _mode: u16,
    ) -> Result<u64, FsError> {
        Err(FsError::ReadOnly)
    }

    fn unlink(&mut self, _parent: u64, _name: &str) -> Result<(), FsError> {
        Err(FsError::ReadOnly)
    }

    fn mkdir(&mut self, _parent: u64, _name: &str, _mode: u16) -> Result<u64, FsError> {
        Err(FsError::ReadOnly)
    }

    fn rmdir(&mut self, _parent: u64, _name: &str) -> Result<(), FsError> {
        Err(FsError::ReadOnly)
    }

    fn rename(
        &mut self,
        _old_parent: u64,
        _old_name: &str,
        _new_parent: u64,
        _new_name: &str,
    ) -> Result<(), FsError> {
        Err(FsError::ReadOnly)
    }

    fn truncate(&mut self, _inode: u64, _size: u64) -> Result<(), FsError> {
        Err(FsError::ReadOnly)
    }

    fn symlink(&mut self, _parent: u64, _name: &str, _target: &str) -> Result<u64, FsError> {
        Err(FsError::ReadOnly)
    }

    fn readlink(&self, _inode: u64) -> Result<String, FsError> {
        if !self.mounted {
            return Err(FsError::NotMounted);
        }
        Err(FsError::NotSupported)
    }

    fn chmod(&mut self, _inode: u64, _mode: u16) -> Result<(), FsError> {
        Err(FsError::ReadOnly)
    }

    fn chown(&mut self, _inode: u64, _uid: u32, _gid: u32) -> Result<(), FsError> {
        Err(FsError::ReadOnly)
    }

    fn sync(&mut self) -> Result<(), FsError> {
        Ok(())
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// §3  FAT32 FILESYSTEM
// ═══════════════════════════════════════════════════════════════════════════════

pub struct Fat32Bpb {
    pub bytes_per_sector: u16,
    pub sectors_per_cluster: u8,
    pub reserved_sectors: u16,
    pub num_fats: u8,
    pub root_entry_count: u16,
    pub total_sectors_16: u16,
    pub media: u8,
    pub fat_size_16: u16,
    pub sectors_per_track: u16,
    pub num_heads: u16,
    pub hidden_sectors: u32,
    pub total_sectors_32: u32,
    pub fat_size_32: u32,
    pub ext_flags: u16,
    pub fs_version: u16,
    pub root_cluster: u32,
    pub fs_info: u16,
    pub backup_boot: u16,
    pub volume_id: u32,
    pub volume_label: [u8; 11],
}

impl Fat32Bpb {
    fn new() -> Self {
        Self {
            bytes_per_sector: 512,
            sectors_per_cluster: 8,
            reserved_sectors: 32,
            num_fats: 2,
            root_entry_count: 0,
            total_sectors_16: 0,
            media: 0xF8,
            fat_size_16: 0,
            sectors_per_track: 0,
            num_heads: 0,
            hidden_sectors: 0,
            total_sectors_32: 0,
            fat_size_32: 0,
            ext_flags: 0,
            fs_version: 0,
            root_cluster: 2,
            fs_info: 1,
            backup_boot: 6,
            volume_id: 0,
            volume_label: [0x20; 11],
        }
    }
}

pub struct Fat32DirEntry {
    pub name: [u8; 8],
    pub ext: [u8; 3],
    pub attrs: u8,
    pub nt_reserved: u8,
    pub create_time_tenth: u8,
    pub create_time: u16,
    pub create_date: u16,
    pub access_date: u16,
    pub first_cluster_hi: u16,
    pub write_time: u16,
    pub write_date: u16,
    pub first_cluster_lo: u16,
    pub file_size: u32,
}

impl Fat32DirEntry {
    fn first_cluster(&self) -> u32 {
        ((self.first_cluster_hi as u32) << 16) | self.first_cluster_lo as u32
    }

    fn is_directory(&self) -> bool {
        self.attrs & 0x10 != 0
    }

    fn is_volume_label(&self) -> bool {
        self.attrs & 0x08 != 0
    }

    fn is_lfn(&self) -> bool {
        self.attrs == 0x0F
    }

    fn is_deleted(&self) -> bool {
        self.name[0] == 0xE5
    }

    fn is_end(&self) -> bool {
        self.name[0] == 0x00
    }
}

pub struct Fat32LfnEntry {
    order: u8,
    name1: [u16; 5],
    attrs: u8,
    lfn_type: u8,
    checksum: u8,
    name2: [u16; 6],
    name3: [u16; 2],
}

pub struct Fat32Fs {
    device_id: u64,
    bpb: Fat32Bpb,
    fat_start: u64,
    data_start: u64,
    cluster_size: u32,
    total_clusters: u32,
    fat_cache: BTreeMap<u32, u32>,
    mounted: bool,
    inode_map: BTreeMap<u64, (u32, String)>,
    next_inode: u64,
}

impl Fat32Fs {
    pub fn new() -> Self {
        Self {
            device_id: 0,
            bpb: Fat32Bpb::new(),
            fat_start: 0,
            data_start: 0,
            cluster_size: 0,
            total_clusters: 0,
            fat_cache: BTreeMap::new(),
            mounted: false,
            inode_map: BTreeMap::new(),
            next_inode: 2,
        }
    }

    fn parse_bpb(&mut self, data: &[u8]) -> Result<(), FsError> {
        if data.len() < 512 {
            return Err(FsError::InvalidSuperblock);
        }

        self.bpb.bytes_per_sector = u16::from_le_bytes([data[11], data[12]]);
        self.bpb.sectors_per_cluster = data[13];
        self.bpb.reserved_sectors = u16::from_le_bytes([data[14], data[15]]);
        self.bpb.num_fats = data[16];
        self.bpb.root_entry_count = u16::from_le_bytes([data[17], data[18]]);
        self.bpb.total_sectors_16 = u16::from_le_bytes([data[19], data[20]]);
        self.bpb.media = data[21];
        self.bpb.fat_size_16 = u16::from_le_bytes([data[22], data[23]]);
        self.bpb.sectors_per_track = u16::from_le_bytes([data[24], data[25]]);
        self.bpb.num_heads = u16::from_le_bytes([data[26], data[27]]);
        self.bpb.hidden_sectors = u32::from_le_bytes([data[28], data[29], data[30], data[31]]);
        self.bpb.total_sectors_32 = u32::from_le_bytes([data[32], data[33], data[34], data[35]]);
        self.bpb.fat_size_32 = u32::from_le_bytes([data[36], data[37], data[38], data[39]]);
        self.bpb.ext_flags = u16::from_le_bytes([data[40], data[41]]);
        self.bpb.fs_version = u16::from_le_bytes([data[42], data[43]]);
        self.bpb.root_cluster = u32::from_le_bytes([data[44], data[45], data[46], data[47]]);
        self.bpb.fs_info = u16::from_le_bytes([data[48], data[49]]);
        self.bpb.backup_boot = u16::from_le_bytes([data[50], data[51]]);
        self.bpb.volume_id = u32::from_le_bytes([data[67], data[68], data[69], data[70]]);
        self.bpb.volume_label.copy_from_slice(&data[71..82]);

        self.fat_start = self.bpb.reserved_sectors as u64 * self.bpb.bytes_per_sector as u64;
        let fat_total_size = self.bpb.num_fats as u64
            * self.bpb.fat_size_32 as u64
            * self.bpb.bytes_per_sector as u64;
        self.data_start = self.fat_start + fat_total_size;
        self.cluster_size = self.bpb.sectors_per_cluster as u32 * self.bpb.bytes_per_sector as u32;

        let total_sectors = if self.bpb.total_sectors_32 != 0 {
            self.bpb.total_sectors_32
        } else {
            self.bpb.total_sectors_16 as u32
        };
        let data_sectors =
            total_sectors - (self.data_start / self.bpb.bytes_per_sector as u64) as u32;
        self.total_clusters = data_sectors / self.bpb.sectors_per_cluster as u32;

        Ok(())
    }

    fn read_fat_entry(&self, cluster: u32, data: &[u8]) -> Result<u32, FsError> {
        if let Some(&cached) = self.fat_cache.get(&cluster) {
            return Ok(cached);
        }

        let offset = self.fat_start as usize + cluster as usize * 4;
        if offset + 4 > data.len() {
            return Err(FsError::IoError);
        }

        let entry = u32::from_le_bytes([
            data[offset],
            data[offset + 1],
            data[offset + 2],
            data[offset + 3],
        ]);
        Ok(entry & 0x0FFFFFFF)
    }

    fn follow_cluster_chain(&self, start: u32, data: &[u8]) -> Result<Vec<u32>, FsError> {
        let mut chain = Vec::new();
        let mut current = start;

        loop {
            if current < 2 || current >= 0x0FFFFFF8 {
                break;
            }
            chain.push(current);
            current = self.read_fat_entry(current, data)?;

            if chain.len() > 1_000_000 {
                return Err(FsError::CorruptedFs);
            }
        }

        Ok(chain)
    }

    fn read_cluster<'a>(&self, cluster: u32, data: &'a [u8]) -> Result<&'a [u8], FsError> {
        let offset = self.data_start as usize + (cluster as usize - 2) * self.cluster_size as usize;
        let end = offset + self.cluster_size as usize;
        if end > data.len() {
            return Err(FsError::IoError);
        }
        Ok(&data[offset..end])
    }

    fn read_dir(&self, cluster: u32, data: &[u8]) -> Result<Vec<(Fat32DirEntry, String)>, FsError> {
        let chain = self.follow_cluster_chain(cluster, data)?;
        let mut entries = Vec::new();
        let mut lfn_parts: Vec<Fat32LfnEntry> = Vec::new();

        for &cl in &chain {
            let cluster_data = self.read_cluster(cl, data)?;
            let mut pos = 0;

            while pos + 32 <= cluster_data.len() {
                let raw = &cluster_data[pos..pos + 32];

                if raw[0] == 0x00 {
                    return Ok(entries);
                }

                if raw[0] == 0xE5 {
                    pos += 32;
                    lfn_parts.clear();
                    continue;
                }

                if raw[11] == 0x0F {
                    let lfn = Fat32LfnEntry {
                        order: raw[0],
                        name1: [
                            u16::from_le_bytes([raw[1], raw[2]]),
                            u16::from_le_bytes([raw[3], raw[4]]),
                            u16::from_le_bytes([raw[5], raw[6]]),
                            u16::from_le_bytes([raw[7], raw[8]]),
                            u16::from_le_bytes([raw[9], raw[10]]),
                        ],
                        attrs: raw[11],
                        lfn_type: raw[12],
                        checksum: raw[13],
                        name2: [
                            u16::from_le_bytes([raw[14], raw[15]]),
                            u16::from_le_bytes([raw[16], raw[17]]),
                            u16::from_le_bytes([raw[18], raw[19]]),
                            u16::from_le_bytes([raw[20], raw[21]]),
                            u16::from_le_bytes([raw[22], raw[23]]),
                            u16::from_le_bytes([raw[24], raw[25]]),
                        ],
                        name3: [
                            u16::from_le_bytes([raw[26], raw[27]]),
                            u16::from_le_bytes([raw[28], raw[29]]),
                        ],
                    };
                    lfn_parts.push(lfn);
                    pos += 32;
                    continue;
                }

                let mut name_buf = [0u8; 8];
                let mut ext_buf = [0u8; 3];
                name_buf.copy_from_slice(&raw[0..8]);
                ext_buf.copy_from_slice(&raw[8..11]);

                let entry = Fat32DirEntry {
                    name: name_buf,
                    ext: ext_buf,
                    attrs: raw[11],
                    nt_reserved: raw[12],
                    create_time_tenth: raw[13],
                    create_time: u16::from_le_bytes([raw[14], raw[15]]),
                    create_date: u16::from_le_bytes([raw[16], raw[17]]),
                    access_date: u16::from_le_bytes([raw[18], raw[19]]),
                    first_cluster_hi: u16::from_le_bytes([raw[20], raw[21]]),
                    write_time: u16::from_le_bytes([raw[22], raw[23]]),
                    write_date: u16::from_le_bytes([raw[24], raw[25]]),
                    first_cluster_lo: u16::from_le_bytes([raw[26], raw[27]]),
                    file_size: u32::from_le_bytes([raw[28], raw[29], raw[30], raw[31]]),
                };

                let long_name = if !lfn_parts.is_empty() {
                    self.decode_lfn(&lfn_parts)
                } else {
                    self.decode_83_name(&entry)
                };

                lfn_parts.clear();
                entries.push((entry, long_name));
                pos += 32;
            }
        }

        Ok(entries)
    }

    fn find_entry(
        &self,
        parent_cluster: u32,
        name: &str,
        data: &[u8],
    ) -> Result<Fat32DirEntry, FsError> {
        let entries = self.read_dir(parent_cluster, data)?;
        for (entry, entry_name) in entries {
            if entry_name.eq_ignore_ascii_case(name) {
                return Ok(entry);
            }
        }
        Err(FsError::NotFound)
    }

    fn decode_lfn(&self, parts: &[Fat32LfnEntry]) -> String {
        let mut sorted = parts.to_vec_workaround();
        sorted.sort_by_key(|e| e.order & 0x3F);

        let mut chars: Vec<u16> = Vec::new();
        for part in &sorted {
            for &c in &part.name1 {
                if c == 0x0000 || c == 0xFFFF {
                    return Self::u16_vec_to_string(&chars);
                }
                chars.push(c);
            }
            for &c in &part.name2 {
                if c == 0x0000 || c == 0xFFFF {
                    return Self::u16_vec_to_string(&chars);
                }
                chars.push(c);
            }
            for &c in &part.name3 {
                if c == 0x0000 || c == 0xFFFF {
                    return Self::u16_vec_to_string(&chars);
                }
                chars.push(c);
            }
        }

        Self::u16_vec_to_string(&chars)
    }

    fn u16_vec_to_string(chars: &[u16]) -> String {
        let mut s = String::new();
        for &c in chars {
            if let Some(ch) = char::from_u32(c as u32) {
                s.push(ch);
            }
        }
        s
    }

    fn decode_83_name(&self, entry: &Fat32DirEntry) -> String {
        let name_part: String = entry
            .name
            .iter()
            .take_while(|&&b| b != 0x20)
            .map(|&b| (b as char).to_ascii_lowercase())
            .collect();

        let ext_part: String = entry
            .ext
            .iter()
            .take_while(|&&b| b != 0x20)
            .map(|&b| (b as char).to_ascii_lowercase())
            .collect();

        if ext_part.is_empty() {
            name_part
        } else {
            format!("{}.{}", name_part, ext_part)
        }
    }

    fn decode_fat_datetime(&self, date: u16, time: u16) -> u64 {
        let year = ((date >> 9) & 0x7F) as u64 + 1980;
        let month = ((date >> 5) & 0x0F) as u64;
        let day = (date & 0x1F) as u64;
        let hour = ((time >> 11) & 0x1F) as u64;
        let minute = ((time >> 5) & 0x3F) as u64;
        let second = ((time & 0x1F) as u64) * 2;

        // Rough Unix timestamp approximation
        let days = (year - 1970) * 365 + (year - 1969) / 4 + month * 30 + day;
        days * 86400 + hour * 3600 + minute * 60 + second
    }
}

trait LfnVecWorkaround {
    fn to_vec_workaround(&self) -> Vec<Fat32LfnEntryCopy>;
}

#[derive(Clone)]
struct Fat32LfnEntryCopy {
    order: u8,
    name1: [u16; 5],
    name2: [u16; 6],
    name3: [u16; 2],
}

impl LfnVecWorkaround for [Fat32LfnEntry] {
    fn to_vec_workaround(&self) -> Vec<Fat32LfnEntryCopy> {
        self.iter()
            .map(|e| Fat32LfnEntryCopy {
                order: e.order,
                name1: e.name1,
                name2: e.name2,
                name3: e.name3,
            })
            .collect()
    }
}

impl Fat32Fs {
    fn decode_lfn_copies(&self, parts: &[Fat32LfnEntryCopy]) -> String {
        let mut sorted: Vec<Fat32LfnEntryCopy> = parts.to_vec();
        sorted.sort_by_key(|e| e.order & 0x3F);

        let mut chars: Vec<u16> = Vec::new();
        for part in &sorted {
            for &c in &part.name1 {
                if c == 0x0000 || c == 0xFFFF {
                    return Self::u16_vec_to_string(&chars);
                }
                chars.push(c);
            }
            for &c in &part.name2 {
                if c == 0x0000 || c == 0xFFFF {
                    return Self::u16_vec_to_string(&chars);
                }
                chars.push(c);
            }
            for &c in &part.name3 {
                if c == 0x0000 || c == 0xFFFF {
                    return Self::u16_vec_to_string(&chars);
                }
                chars.push(c);
            }
        }

        Self::u16_vec_to_string(&chars)
    }
}

impl Filesystem for Fat32Fs {
    fn name(&self) -> &str {
        "fat32"
    }

    fn mount(&mut self, device: u64) -> Result<(), FsError> {
        self.device_id = device;
        self.mounted = true;
        self.inode_map
            .insert(1, (self.bpb.root_cluster, String::from("/")));
        Ok(())
    }

    fn unmount(&mut self) -> Result<(), FsError> {
        self.mounted = false;
        self.fat_cache.clear();
        self.inode_map.clear();
        Ok(())
    }

    fn stat_fs(&self) -> Result<FsStats, FsError> {
        if !self.mounted {
            return Err(FsError::NotMounted);
        }
        Ok(FsStats {
            total_blocks: self.total_clusters as u64,
            free_blocks: 0,
            available_blocks: 0,
            total_inodes: 0,
            free_inodes: 0,
            block_size: self.cluster_size,
            max_name_length: 255,
            fs_type: "fat32".to_string(),
        })
    }

    fn lookup(&self, _parent: u64, _name: &str) -> Result<InodeInfo, FsError> {
        if !self.mounted {
            return Err(FsError::NotMounted);
        }
        Err(FsError::NotSupported)
    }

    fn read_dir(&self, _inode: u64) -> Result<Vec<DirEntry>, FsError> {
        if !self.mounted {
            return Err(FsError::NotMounted);
        }
        Err(FsError::NotSupported)
    }

    fn read_file(&self, _inode: u64, _offset: u64, _buf: &mut [u8]) -> Result<usize, FsError> {
        if !self.mounted {
            return Err(FsError::NotMounted);
        }
        Err(FsError::NotSupported)
    }

    fn write_file(&mut self, _inode: u64, _offset: u64, _data: &[u8]) -> Result<usize, FsError> {
        Err(FsError::NotSupported)
    }

    fn create(
        &mut self,
        _parent: u64,
        _name: &str,
        _file_type: FileType,
        _mode: u16,
    ) -> Result<u64, FsError> {
        Err(FsError::NotSupported)
    }

    fn unlink(&mut self, _parent: u64, _name: &str) -> Result<(), FsError> {
        Err(FsError::NotSupported)
    }

    fn mkdir(&mut self, _parent: u64, _name: &str, _mode: u16) -> Result<u64, FsError> {
        Err(FsError::NotSupported)
    }

    fn rmdir(&mut self, _parent: u64, _name: &str) -> Result<(), FsError> {
        Err(FsError::NotSupported)
    }

    fn rename(
        &mut self,
        _old_parent: u64,
        _old_name: &str,
        _new_parent: u64,
        _new_name: &str,
    ) -> Result<(), FsError> {
        Err(FsError::NotSupported)
    }

    fn truncate(&mut self, _inode: u64, _size: u64) -> Result<(), FsError> {
        Err(FsError::NotSupported)
    }

    fn symlink(&mut self, _parent: u64, _name: &str, _target: &str) -> Result<u64, FsError> {
        Err(FsError::NotSupported)
    }

    fn readlink(&self, _inode: u64) -> Result<String, FsError> {
        Err(FsError::NotSupported)
    }

    fn chmod(&mut self, _inode: u64, _mode: u16) -> Result<(), FsError> {
        Err(FsError::NotSupported)
    }

    fn chown(&mut self, _inode: u64, _uid: u32, _gid: u32) -> Result<(), FsError> {
        Err(FsError::NotSupported)
    }

    fn sync(&mut self) -> Result<(), FsError> {
        Ok(())
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// §4  NTFS READER
// ═══════════════════════════════════════════════════════════════════════════════

pub struct NtfsBoot {
    pub oem_id: [u8; 8],
    pub bytes_per_sector: u16,
    pub sectors_per_cluster: u8,
    pub media_descriptor: u8,
    pub total_sectors: u64,
    pub mft_cluster: u64,
    pub mft_mirror_cluster: u64,
    pub mft_record_size: i8,
    pub index_record_size: i8,
    pub serial_number: u64,
}

impl NtfsBoot {
    fn new() -> Self {
        Self {
            oem_id: [0; 8],
            bytes_per_sector: 512,
            sectors_per_cluster: 8,
            media_descriptor: 0xF8,
            total_sectors: 0,
            mft_cluster: 0,
            mft_mirror_cluster: 0,
            mft_record_size: -10,
            index_record_size: -14,
            serial_number: 0,
        }
    }

    fn record_size(&self) -> u32 {
        if self.mft_record_size > 0 {
            self.mft_record_size as u32
                * self.sectors_per_cluster as u32
                * self.bytes_per_sector as u32
        } else {
            1u32 << (-(self.mft_record_size as i32)) as u32
        }
    }

    fn cluster_size(&self) -> u32 {
        self.sectors_per_cluster as u32 * self.bytes_per_sector as u32
    }
}

#[derive(Debug, Clone)]
pub struct MftRecord {
    pub signature: [u8; 4],
    pub sequence_number: u16,
    pub link_count: u16,
    pub attrs_offset: u16,
    pub flags: u16,
    pub used_size: u32,
    pub allocated_size: u32,
    pub base_record: u64,
    pub next_attr_id: u16,
    pub record_number: u32,
    pub attributes: Vec<NtfsAttribute>,
}

#[derive(Debug, Clone)]
pub enum NtfsAttribute {
    StandardInfo(StdInfo),
    FileName(FileNameAttr),
    Data(DataAttr),
    IndexRoot(IndexRoot),
    IndexAlloc(IndexAlloc),
    Bitmap(Vec<u8>),
    SecurityDescriptor(Vec<u8>),
    VolumeName(String),
    VolumeInfo { major: u8, minor: u8, flags: u16 },
    ObjectId([u8; 16]),
    ReparsePoint { tag: u32, data: Vec<u8> },
    Unknown { attr_type: u32, data: Vec<u8> },
}

#[derive(Debug, Clone)]
pub struct StdInfo {
    creation_time: u64,
    modification_time: u64,
    mft_modification_time: u64,
    access_time: u64,
    flags: u32,
    owner_id: u32,
    security_id: u32,
}

#[derive(Debug, Clone)]
pub struct FileNameAttr {
    parent_ref: u64,
    creation_time: u64,
    modification_time: u64,
    mft_modification_time: u64,
    access_time: u64,
    allocated_size: u64,
    real_size: u64,
    flags: u32,
    name: String,
    namespace: u8,
}

#[derive(Debug, Clone)]
pub struct DataAttr {
    name: Option<String>,
    resident: bool,
    data: Vec<u8>,
    runs: Vec<DataRun>,
}

#[derive(Debug, Clone)]
pub struct DataRun {
    offset: i64,
    length: u64,
}

#[derive(Debug, Clone)]
pub struct IndexRoot {
    attr_type: u32,
    collation_rule: u32,
    index_size: u32,
    clusters_per_index: u8,
    entries: Vec<IndexEntry>,
}

#[derive(Debug, Clone)]
pub struct IndexAlloc {
    entries: Vec<IndexEntry>,
}

#[derive(Debug, Clone)]
pub struct IndexEntry {
    mft_ref: u64,
    file_name: Option<FileNameAttr>,
    flags: u32,
    sub_node_vcn: Option<u64>,
}

pub struct NtfsFs {
    device_id: u64,
    boot: NtfsBoot,
    mft_start: u64,
    mft_record_size: u32,
    cluster_size: u32,
    sector_size: u16,
    mounted: bool,
    mft_cache: BTreeMap<u32, MftRecord>,
}

impl NtfsFs {
    pub fn new() -> Self {
        Self {
            device_id: 0,
            boot: NtfsBoot::new(),
            mft_start: 0,
            mft_record_size: 1024,
            cluster_size: 4096,
            sector_size: 512,
            mounted: false,
            mft_cache: BTreeMap::new(),
        }
    }

    fn parse_boot_sector(&mut self, data: &[u8]) -> Result<(), FsError> {
        if data.len() < 512 {
            return Err(FsError::InvalidSuperblock);
        }

        self.boot.oem_id.copy_from_slice(&data[3..11]);

        if &self.boot.oem_id != b"NTFS    " {
            return Err(FsError::InvalidSuperblock);
        }

        self.boot.bytes_per_sector = u16::from_le_bytes([data[11], data[12]]);
        self.boot.sectors_per_cluster = data[13];
        self.boot.media_descriptor = data[21];
        self.boot.total_sectors = u64::from_le_bytes([
            data[40], data[41], data[42], data[43], data[44], data[45], data[46], data[47],
        ]);
        self.boot.mft_cluster = u64::from_le_bytes([
            data[48], data[49], data[50], data[51], data[52], data[53], data[54], data[55],
        ]);
        self.boot.mft_mirror_cluster = u64::from_le_bytes([
            data[56], data[57], data[58], data[59], data[60], data[61], data[62], data[63],
        ]);
        self.boot.mft_record_size = data[64] as i8;
        self.boot.index_record_size = data[68] as i8;
        self.boot.serial_number = u64::from_le_bytes([
            data[72], data[73], data[74], data[75], data[76], data[77], data[78], data[79],
        ]);

        self.sector_size = self.boot.bytes_per_sector;
        self.cluster_size = self.boot.cluster_size();
        self.mft_record_size = self.boot.record_size();
        self.mft_start = self.boot.mft_cluster * self.cluster_size as u64;

        Ok(())
    }

    fn read_mft_record(&self, record_num: u32, data: &[u8]) -> Result<MftRecord, FsError> {
        let offset = self.mft_start as usize + record_num as usize * self.mft_record_size as usize;
        if offset + self.mft_record_size as usize > data.len() {
            return Err(FsError::IoError);
        }

        let raw = &data[offset..offset + self.mft_record_size as usize];

        let mut signature = [0u8; 4];
        signature.copy_from_slice(&raw[0..4]);

        if &signature != b"FILE" {
            return Err(FsError::CorruptedFs);
        }

        let update_seq_offset = u16::from_le_bytes([raw[4], raw[5]]);
        let update_seq_size = u16::from_le_bytes([raw[6], raw[7]]);
        let sequence_number = u16::from_le_bytes([raw[16], raw[17]]);
        let link_count = u16::from_le_bytes([raw[18], raw[19]]);
        let attrs_offset = u16::from_le_bytes([raw[20], raw[21]]);
        let flags = u16::from_le_bytes([raw[22], raw[23]]);
        let used_size = u32::from_le_bytes([raw[24], raw[25], raw[26], raw[27]]);
        let allocated_size = u32::from_le_bytes([raw[28], raw[29], raw[30], raw[31]]);
        let base_record = u64::from_le_bytes([
            raw[32], raw[33], raw[34], raw[35], raw[36], raw[37], raw[38], raw[39],
        ]);
        let next_attr_id = u16::from_le_bytes([raw[40], raw[41]]);

        let mut fixed_raw = raw.to_vec();
        if update_seq_offset as usize + 2 * update_seq_size as usize
            <= self.mft_record_size as usize
        {
            let usn = u16::from_le_bytes([
                raw[update_seq_offset as usize],
                raw[update_seq_offset as usize + 1],
            ]);
            for i in 1..update_seq_size {
                let fixup_offset = update_seq_offset as usize + i as usize * 2;
                let sector_end = i as usize * self.sector_size as usize - 2;
                if sector_end + 1 < fixed_raw.len() && fixup_offset + 1 < raw.len() {
                    if u16::from_le_bytes([fixed_raw[sector_end], fixed_raw[sector_end + 1]]) == usn
                    {
                        fixed_raw[sector_end] = raw[fixup_offset];
                        fixed_raw[sector_end + 1] = raw[fixup_offset + 1];
                    }
                }
            }
        }

        let attributes = self.parse_attributes(&fixed_raw, attrs_offset as usize)?;

        Ok(MftRecord {
            signature,
            sequence_number,
            link_count,
            attrs_offset,
            flags,
            used_size,
            allocated_size,
            base_record,
            next_attr_id,
            record_number: record_num,
            attributes,
        })
    }

    fn parse_attributes(&self, raw: &[u8], start: usize) -> Result<Vec<NtfsAttribute>, FsError> {
        let mut attrs = Vec::new();
        let mut pos = start;

        loop {
            if pos + 4 > raw.len() {
                break;
            }

            let attr_type =
                u32::from_le_bytes([raw[pos], raw[pos + 1], raw[pos + 2], raw[pos + 3]]);
            if attr_type == 0xFFFFFFFF {
                break;
            }

            if pos + 8 > raw.len() {
                break;
            }
            let attr_len =
                u32::from_le_bytes([raw[pos + 4], raw[pos + 5], raw[pos + 6], raw[pos + 7]])
                    as usize;
            if attr_len == 0 || pos + attr_len > raw.len() {
                break;
            }

            let non_resident = raw[pos + 8] != 0;
            let _name_length = raw[pos + 9];
            let _name_offset = u16::from_le_bytes([raw[pos + 10], raw[pos + 11]]);

            let attr = match attr_type {
                0x10 => self.parse_std_info(&raw[pos..pos + attr_len]),
                0x30 => self.parse_file_name(&raw[pos..pos + attr_len]),
                0x80 => self.parse_data_attr(&raw[pos..pos + attr_len], non_resident),
                0x90 => Some(NtfsAttribute::IndexRoot(IndexRoot {
                    attr_type: 0x30,
                    collation_rule: 1,
                    index_size: 4096,
                    clusters_per_index: 1,
                    entries: Vec::new(),
                })),
                0xA0 => Some(NtfsAttribute::IndexAlloc(IndexAlloc {
                    entries: Vec::new(),
                })),
                0xB0 => {
                    let content = self.get_resident_content(&raw[pos..pos + attr_len]);
                    Some(NtfsAttribute::Bitmap(content))
                }
                0x50 => {
                    let content = self.get_resident_content(&raw[pos..pos + attr_len]);
                    Some(NtfsAttribute::SecurityDescriptor(content))
                }
                0x60 => {
                    let content = self.get_resident_content(&raw[pos..pos + attr_len]);
                    let name = String::from_utf8_lossy(&content).to_string();
                    Some(NtfsAttribute::VolumeName(name))
                }
                0x70 => {
                    let content = self.get_resident_content(&raw[pos..pos + attr_len]);
                    if content.len() >= 4 {
                        Some(NtfsAttribute::VolumeInfo {
                            major: content[0],
                            minor: content[1],
                            flags: u16::from_le_bytes([content[2], content[3]]),
                        })
                    } else {
                        Some(NtfsAttribute::Unknown {
                            attr_type,
                            data: content,
                        })
                    }
                }
                0x40 => {
                    let content = self.get_resident_content(&raw[pos..pos + attr_len]);
                    let mut oid = [0u8; 16];
                    if content.len() >= 16 {
                        oid.copy_from_slice(&content[..16]);
                    }
                    Some(NtfsAttribute::ObjectId(oid))
                }
                0xC0 => {
                    let content = self.get_resident_content(&raw[pos..pos + attr_len]);
                    let tag = if content.len() >= 4 {
                        u32::from_le_bytes([content[0], content[1], content[2], content[3]])
                    } else {
                        0
                    };
                    Some(NtfsAttribute::ReparsePoint {
                        tag,
                        data: if content.len() > 4 {
                            content[4..].to_vec()
                        } else {
                            Vec::new()
                        },
                    })
                }
                _ => {
                    let content = self.get_resident_content(&raw[pos..pos + attr_len]);
                    Some(NtfsAttribute::Unknown {
                        attr_type,
                        data: content,
                    })
                }
            };

            if let Some(a) = attr {
                attrs.push(a);
            }

            pos += attr_len;
        }

        Ok(attrs)
    }

    fn parse_std_info(&self, raw: &[u8]) -> Option<NtfsAttribute> {
        let content_offset = if raw[8] == 0 {
            u16::from_le_bytes([raw[20], raw[21]]) as usize
        } else {
            return None;
        };

        if content_offset + 48 > raw.len() {
            return None;
        }

        let c = &raw[content_offset..];
        Some(NtfsAttribute::StandardInfo(StdInfo {
            creation_time: u64::from_le_bytes([c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7]]),
            modification_time: u64::from_le_bytes([
                c[8], c[9], c[10], c[11], c[12], c[13], c[14], c[15],
            ]),
            mft_modification_time: u64::from_le_bytes([
                c[16], c[17], c[18], c[19], c[20], c[21], c[22], c[23],
            ]),
            access_time: u64::from_le_bytes([
                c[24], c[25], c[26], c[27], c[28], c[29], c[30], c[31],
            ]),
            flags: u32::from_le_bytes([c[32], c[33], c[34], c[35]]),
            owner_id: if c.len() > 52 {
                u32::from_le_bytes([c[48], c[49], c[50], c[51]])
            } else {
                0
            },
            security_id: if c.len() > 56 {
                u32::from_le_bytes([c[52], c[53], c[54], c[55]])
            } else {
                0
            },
        }))
    }

    fn parse_file_name(&self, raw: &[u8]) -> Option<NtfsAttribute> {
        if raw[8] != 0 {
            return None;
        }
        let content_offset = u16::from_le_bytes([raw[20], raw[21]]) as usize;
        let content_length = u32::from_le_bytes([raw[16], raw[17], raw[18], raw[19]]) as usize;

        if content_offset + content_length > raw.len() || content_length < 66 {
            return None;
        }

        let c = &raw[content_offset..];
        let parent_ref = u64::from_le_bytes([c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7]]);
        let creation_time =
            u64::from_le_bytes([c[8], c[9], c[10], c[11], c[12], c[13], c[14], c[15]]);
        let modification_time =
            u64::from_le_bytes([c[16], c[17], c[18], c[19], c[20], c[21], c[22], c[23]]);
        let mft_modification_time =
            u64::from_le_bytes([c[24], c[25], c[26], c[27], c[28], c[29], c[30], c[31]]);
        let access_time =
            u64::from_le_bytes([c[32], c[33], c[34], c[35], c[36], c[37], c[38], c[39]]);
        let allocated_size =
            u64::from_le_bytes([c[40], c[41], c[42], c[43], c[44], c[45], c[46], c[47]]);
        let real_size =
            u64::from_le_bytes([c[48], c[49], c[50], c[51], c[52], c[53], c[54], c[55]]);
        let flags = u32::from_le_bytes([c[56], c[57], c[58], c[59]]);
        let name_length = c[64] as usize;
        let namespace = c[65];

        let name = if 66 + name_length * 2 <= c.len() {
            let name_bytes: Vec<u16> = (0..name_length)
                .map(|i| u16::from_le_bytes([c[66 + i * 2], c[67 + i * 2]]))
                .collect();
            String::from_utf16_lossy(&name_bytes)
        } else {
            String::new()
        };

        Some(NtfsAttribute::FileName(FileNameAttr {
            parent_ref: parent_ref & 0x0000_FFFF_FFFF_FFFF,
            creation_time,
            modification_time,
            mft_modification_time,
            access_time,
            allocated_size,
            real_size,
            flags,
            name,
            namespace,
        }))
    }

    fn parse_data_attr(&self, raw: &[u8], non_resident: bool) -> Option<NtfsAttribute> {
        if !non_resident {
            let content = self.get_resident_content(raw);
            Some(NtfsAttribute::Data(DataAttr {
                name: None,
                resident: true,
                data: content,
                runs: Vec::new(),
            }))
        } else {
            let runs = self.parse_data_runs(raw);
            Some(NtfsAttribute::Data(DataAttr {
                name: None,
                resident: false,
                data: Vec::new(),
                runs,
            }))
        }
    }

    fn parse_data_runs(&self, raw: &[u8]) -> Vec<DataRun> {
        let mut runs = Vec::new();

        if raw.len() < 64 || raw[8] == 0 {
            return runs;
        }

        let run_offset = u16::from_le_bytes([raw[32], raw[33]]) as usize;
        if run_offset >= raw.len() {
            return runs;
        }

        let mut pos = run_offset;
        let mut prev_offset: i64 = 0;

        while pos < raw.len() {
            let header = raw[pos];
            if header == 0 {
                break;
            }

            let length_size = (header & 0x0F) as usize;
            let offset_size = ((header >> 4) & 0x0F) as usize;

            pos += 1;
            if pos + length_size + offset_size > raw.len() {
                break;
            }

            let mut length: u64 = 0;
            for i in 0..length_size {
                length |= (raw[pos + i] as u64) << (i * 8);
            }
            pos += length_size;

            let mut offset: i64 = 0;
            if offset_size > 0 {
                for i in 0..offset_size {
                    offset |= (raw[pos + i] as i64) << (i * 8);
                }
                if raw[pos + offset_size - 1] & 0x80 != 0 {
                    for i in offset_size..8 {
                        offset |= 0xFFi64 << (i * 8);
                    }
                }
                pos += offset_size;
                offset += prev_offset;
                prev_offset = offset;
            }

            runs.push(DataRun { offset, length });
        }

        runs
    }

    fn get_resident_content(&self, raw: &[u8]) -> Vec<u8> {
        if raw.len() < 24 || raw[8] != 0 {
            return Vec::new();
        }

        let content_length = u32::from_le_bytes([raw[16], raw[17], raw[18], raw[19]]) as usize;
        let content_offset = u16::from_le_bytes([raw[20], raw[21]]) as usize;

        if content_offset + content_length > raw.len() {
            return Vec::new();
        }

        raw[content_offset..content_offset + content_length].to_vec()
    }

    fn read_file_data(&self, record: &MftRecord, _data: &[u8]) -> Result<Vec<u8>, FsError> {
        for attr in &record.attributes {
            if let NtfsAttribute::Data(data_attr) = attr {
                if data_attr.resident {
                    return Ok(data_attr.data.clone());
                } else {
                    let mut result = Vec::new();
                    for run in &data_attr.runs {
                        let _start = run.offset as u64 * self.cluster_size as u64;
                        let len = run.length * self.cluster_size as u64;
                        result.resize(result.len() + len as usize, 0);
                    }
                    return Ok(result);
                }
            }
        }
        Err(FsError::NotFound)
    }

    fn read_index(&self, _record: &MftRecord) -> Result<Vec<IndexEntry>, FsError> {
        let mut entries = Vec::new();
        // Placeholder for full index parsing
        Ok(entries)
    }

    fn ntfs_time_to_unix(ntfs_time: u64) -> u64 {
        if ntfs_time < 116444736000000000 {
            return 0;
        }
        (ntfs_time - 116444736000000000) / 10000000
    }
}

impl Filesystem for NtfsFs {
    fn name(&self) -> &str {
        "ntfs"
    }

    fn mount(&mut self, device: u64) -> Result<(), FsError> {
        self.device_id = device;
        self.mounted = true;
        Ok(())
    }

    fn unmount(&mut self) -> Result<(), FsError> {
        self.mounted = false;
        self.mft_cache.clear();
        Ok(())
    }

    fn stat_fs(&self) -> Result<FsStats, FsError> {
        if !self.mounted {
            return Err(FsError::NotMounted);
        }
        let total =
            self.boot.total_sectors * self.boot.bytes_per_sector as u64 / self.cluster_size as u64;
        Ok(FsStats {
            total_blocks: total,
            free_blocks: 0,
            available_blocks: 0,
            total_inodes: 0,
            free_inodes: 0,
            block_size: self.cluster_size,
            max_name_length: 255,
            fs_type: "ntfs".to_string(),
        })
    }

    fn lookup(&self, _parent: u64, _name: &str) -> Result<InodeInfo, FsError> {
        if !self.mounted {
            return Err(FsError::NotMounted);
        }
        Err(FsError::NotSupported)
    }

    fn read_dir(&self, _inode: u64) -> Result<Vec<DirEntry>, FsError> {
        if !self.mounted {
            return Err(FsError::NotMounted);
        }
        Err(FsError::NotSupported)
    }

    fn read_file(&self, _inode: u64, _offset: u64, _buf: &mut [u8]) -> Result<usize, FsError> {
        if !self.mounted {
            return Err(FsError::NotMounted);
        }
        Err(FsError::NotSupported)
    }

    fn write_file(&mut self, _inode: u64, _offset: u64, _data: &[u8]) -> Result<usize, FsError> {
        Err(FsError::ReadOnly)
    }

    fn create(
        &mut self,
        _parent: u64,
        _name: &str,
        _file_type: FileType,
        _mode: u16,
    ) -> Result<u64, FsError> {
        Err(FsError::ReadOnly)
    }

    fn unlink(&mut self, _parent: u64, _name: &str) -> Result<(), FsError> {
        Err(FsError::ReadOnly)
    }

    fn mkdir(&mut self, _parent: u64, _name: &str, _mode: u16) -> Result<u64, FsError> {
        Err(FsError::ReadOnly)
    }

    fn rmdir(&mut self, _parent: u64, _name: &str) -> Result<(), FsError> {
        Err(FsError::ReadOnly)
    }

    fn rename(
        &mut self,
        _old_parent: u64,
        _old_name: &str,
        _new_parent: u64,
        _new_name: &str,
    ) -> Result<(), FsError> {
        Err(FsError::ReadOnly)
    }

    fn truncate(&mut self, _inode: u64, _size: u64) -> Result<(), FsError> {
        Err(FsError::ReadOnly)
    }

    fn symlink(&mut self, _parent: u64, _name: &str, _target: &str) -> Result<u64, FsError> {
        Err(FsError::ReadOnly)
    }

    fn readlink(&self, _inode: u64) -> Result<String, FsError> {
        Err(FsError::NotSupported)
    }

    fn chmod(&mut self, _inode: u64, _mode: u16) -> Result<(), FsError> {
        Err(FsError::ReadOnly)
    }

    fn chown(&mut self, _inode: u64, _uid: u32, _gid: u32) -> Result<(), FsError> {
        Err(FsError::ReadOnly)
    }

    fn sync(&mut self) -> Result<(), FsError> {
        Ok(())
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// §5  TMPFS — IN-MEMORY FILESYSTEM
// ═══════════════════════════════════════════════════════════════════════════════

pub struct TmpInode {
    pub info: InodeInfo,
    pub data: Vec<u8>,
    pub children: BTreeMap<String, u64>,
    pub symlink_target: Option<String>,
}

pub struct TmpFs {
    inodes: BTreeMap<u64, TmpInode>,
    next_inode: u64,
    total_size: u64,
    max_size: u64,
}

impl TmpFs {
    pub fn new(max_size: u64) -> Self {
        let mut fs = Self {
            inodes: BTreeMap::new(),
            next_inode: 2,
            total_size: 0,
            max_size,
        };

        let root = TmpInode {
            info: InodeInfo {
                inode: 1,
                file_type: FileType::Directory,
                mode: 0o755,
                uid: 0,
                gid: 0,
                size: 0,
                blocks: 0,
                block_size: 4096,
                atime: 0,
                mtime: 0,
                ctime: 0,
                crtime: 0,
                nlinks: 2,
                flags: 0,
            },
            data: Vec::new(),
            children: BTreeMap::new(),
            symlink_target: None,
        };
        fs.inodes.insert(1, root);
        fs
    }

    fn alloc_inode(&mut self) -> u64 {
        let id = self.next_inode;
        self.next_inode += 1;
        id
    }
}

impl Filesystem for TmpFs {
    fn name(&self) -> &str {
        "tmpfs"
    }

    fn mount(&mut self, _device: u64) -> Result<(), FsError> {
        Ok(())
    }

    fn unmount(&mut self) -> Result<(), FsError> {
        self.inodes.clear();
        self.total_size = 0;
        Ok(())
    }

    fn stat_fs(&self) -> Result<FsStats, FsError> {
        let used_blocks = self.total_size / 4096;
        let total_blocks = self.max_size / 4096;
        Ok(FsStats {
            total_blocks,
            free_blocks: total_blocks - used_blocks,
            available_blocks: total_blocks - used_blocks,
            total_inodes: u64::MAX,
            free_inodes: u64::MAX - self.next_inode,
            block_size: 4096,
            max_name_length: 255,
            fs_type: "tmpfs".to_string(),
        })
    }

    fn lookup(&self, parent: u64, name: &str) -> Result<InodeInfo, FsError> {
        let parent_node = self.inodes.get(&parent).ok_or(FsError::NotFound)?;
        if parent_node.info.file_type != FileType::Directory {
            return Err(FsError::NotADirectory);
        }

        if name == "." {
            return Ok(parent_node.info.clone());
        }

        let &child_inode = parent_node.children.get(name).ok_or(FsError::NotFound)?;
        let child = self.inodes.get(&child_inode).ok_or(FsError::NotFound)?;
        Ok(child.info.clone())
    }

    fn read_dir(&self, inode: u64) -> Result<Vec<DirEntry>, FsError> {
        let node = self.inodes.get(&inode).ok_or(FsError::NotFound)?;
        if node.info.file_type != FileType::Directory {
            return Err(FsError::NotADirectory);
        }

        let mut entries = Vec::new();
        let mut offset = 0u64;
        for (name, &child_inode) in &node.children {
            let child = self.inodes.get(&child_inode).ok_or(FsError::NotFound)?;
            entries.push(DirEntry {
                inode: child_inode,
                name: name.clone(),
                file_type: child.info.file_type,
                offset,
            });
            offset += 1;
        }
        Ok(entries)
    }

    fn read_file(&self, inode: u64, offset: u64, buf: &mut [u8]) -> Result<usize, FsError> {
        let node = self.inodes.get(&inode).ok_or(FsError::NotFound)?;
        if node.info.file_type == FileType::Directory {
            return Err(FsError::IsADirectory);
        }

        let data = &node.data;
        if offset >= data.len() as u64 {
            return Ok(0);
        }

        let start = offset as usize;
        let available = data.len() - start;
        let to_read = buf.len().min(available);
        buf[..to_read].copy_from_slice(&data[start..start + to_read]);
        Ok(to_read)
    }

    fn write_file(&mut self, inode: u64, offset: u64, data: &[u8]) -> Result<usize, FsError> {
        let node = self.inodes.get_mut(&inode).ok_or(FsError::NotFound)?;
        if node.info.file_type == FileType::Directory {
            return Err(FsError::IsADirectory);
        }

        let end = offset as usize + data.len();
        if end > node.data.len() {
            let growth = end - node.data.len();
            if self.total_size + growth as u64 > self.max_size {
                return Err(FsError::NoSpace);
            }
            node.data.resize(end, 0);
            self.total_size += growth as u64;
        }

        node.data[offset as usize..end].copy_from_slice(data);
        node.info.size = node.data.len() as u64;
        node.info.blocks = (node.info.size + 4095) / 4096;
        Ok(data.len())
    }

    fn create(
        &mut self,
        parent: u64,
        name: &str,
        file_type: FileType,
        mode: u16,
    ) -> Result<u64, FsError> {
        if name.is_empty() || name.len() > 255 {
            return Err(FsError::InvalidName);
        }

        {
            let parent_node = self.inodes.get(&parent).ok_or(FsError::NotFound)?;
            if parent_node.info.file_type != FileType::Directory {
                return Err(FsError::NotADirectory);
            }
            if parent_node.children.contains_key(name) {
                return Err(FsError::AlreadyExists);
            }
        }

        let new_inode = self.alloc_inode();
        let node = TmpInode {
            info: InodeInfo {
                inode: new_inode,
                file_type,
                mode,
                uid: 0,
                gid: 0,
                size: 0,
                blocks: 0,
                block_size: 4096,
                atime: 0,
                mtime: 0,
                ctime: 0,
                crtime: 0,
                nlinks: 1,
                flags: 0,
            },
            data: Vec::new(),
            children: BTreeMap::new(),
            symlink_target: None,
        };
        self.inodes.insert(new_inode, node);

        let parent_node = self.inodes.get_mut(&parent).unwrap();
        parent_node.children.insert(name.to_string(), new_inode);

        Ok(new_inode)
    }

    fn unlink(&mut self, parent: u64, name: &str) -> Result<(), FsError> {
        let child_inode = {
            let parent_node = self.inodes.get(&parent).ok_or(FsError::NotFound)?;
            *parent_node.children.get(name).ok_or(FsError::NotFound)?
        };

        {
            let child = self.inodes.get(&child_inode).ok_or(FsError::NotFound)?;
            if child.info.file_type == FileType::Directory {
                return Err(FsError::IsADirectory);
            }
            self.total_size -= child.data.len() as u64;
        }

        self.inodes.remove(&child_inode);
        let parent_node = self.inodes.get_mut(&parent).unwrap();
        parent_node.children.remove(name);
        Ok(())
    }

    fn mkdir(&mut self, parent: u64, name: &str, mode: u16) -> Result<u64, FsError> {
        let inode = self.create(parent, name, FileType::Directory, mode)?;
        if let Some(node) = self.inodes.get_mut(&inode) {
            node.info.nlinks = 2;
        }
        if let Some(parent_node) = self.inodes.get_mut(&parent) {
            parent_node.info.nlinks += 1;
        }
        Ok(inode)
    }

    fn rmdir(&mut self, parent: u64, name: &str) -> Result<(), FsError> {
        let child_inode = {
            let parent_node = self.inodes.get(&parent).ok_or(FsError::NotFound)?;
            *parent_node.children.get(name).ok_or(FsError::NotFound)?
        };

        {
            let child = self.inodes.get(&child_inode).ok_or(FsError::NotFound)?;
            if child.info.file_type != FileType::Directory {
                return Err(FsError::NotADirectory);
            }
            if !child.children.is_empty() {
                return Err(FsError::NotEmpty);
            }
        }

        self.inodes.remove(&child_inode);
        let parent_node = self.inodes.get_mut(&parent).unwrap();
        parent_node.children.remove(name);
        parent_node.info.nlinks -= 1;
        Ok(())
    }

    fn rename(
        &mut self,
        old_parent: u64,
        old_name: &str,
        new_parent: u64,
        new_name: &str,
    ) -> Result<(), FsError> {
        let child_inode = {
            let parent_node = self.inodes.get(&old_parent).ok_or(FsError::NotFound)?;
            *parent_node
                .children
                .get(old_name)
                .ok_or(FsError::NotFound)?
        };

        if let Some(existing) = self
            .inodes
            .get(&new_parent)
            .and_then(|p| p.children.get(new_name).copied())
        {
            self.inodes.remove(&existing);
        }

        let old_parent_node = self.inodes.get_mut(&old_parent).ok_or(FsError::NotFound)?;
        old_parent_node.children.remove(old_name);

        let new_parent_node = self.inodes.get_mut(&new_parent).ok_or(FsError::NotFound)?;
        new_parent_node
            .children
            .insert(new_name.to_string(), child_inode);

        Ok(())
    }

    fn truncate(&mut self, inode: u64, size: u64) -> Result<(), FsError> {
        let node = self.inodes.get_mut(&inode).ok_or(FsError::NotFound)?;
        let old_size = node.data.len() as u64;
        node.data.resize(size as usize, 0);
        node.info.size = size;
        node.info.blocks = (size + 4095) / 4096;

        if size < old_size {
            self.total_size -= old_size - size;
        } else {
            let growth = size - old_size;
            if self.total_size + growth > self.max_size {
                return Err(FsError::NoSpace);
            }
            self.total_size += growth;
        }
        Ok(())
    }

    fn symlink(&mut self, parent: u64, name: &str, target: &str) -> Result<u64, FsError> {
        let inode = self.create(parent, name, FileType::Symlink, 0o777)?;
        if let Some(node) = self.inodes.get_mut(&inode) {
            node.symlink_target = Some(target.to_string());
            node.info.size = target.len() as u64;
        }
        Ok(inode)
    }

    fn readlink(&self, inode: u64) -> Result<String, FsError> {
        let node = self.inodes.get(&inode).ok_or(FsError::NotFound)?;
        if node.info.file_type != FileType::Symlink {
            return Err(FsError::NotSupported);
        }
        node.symlink_target.clone().ok_or(FsError::NotFound)
    }

    fn chmod(&mut self, inode: u64, mode: u16) -> Result<(), FsError> {
        let node = self.inodes.get_mut(&inode).ok_or(FsError::NotFound)?;
        node.info.mode = mode;
        Ok(())
    }

    fn chown(&mut self, inode: u64, uid: u32, gid: u32) -> Result<(), FsError> {
        let node = self.inodes.get_mut(&inode).ok_or(FsError::NotFound)?;
        node.info.uid = uid;
        node.info.gid = gid;
        Ok(())
    }

    fn sync(&mut self) -> Result<(), FsError> {
        Ok(())
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// §6  SYSFS — KERNEL PARAMETER EXPOSURE
// ═══════════════════════════════════════════════════════════════════════════════

pub enum SysEntry {
    Dir(BTreeMap<String, SysEntry>),
    File {
        read: fn() -> String,
        write: Option<fn(&str)>,
    },
    Value(String),
}

pub struct SysFs {
    entries: BTreeMap<String, SysEntry>,
    mounted: bool,
}

impl SysFs {
    pub fn new() -> Self {
        let mut entries = BTreeMap::new();

        entries.insert(
            "kernel".to_string(),
            SysEntry::Dir({
                let mut kernel = BTreeMap::new();
                kernel.insert(
                    "hostname".to_string(),
                    SysEntry::Value("raeenos".to_string()),
                );
                kernel.insert("version".to_string(), SysEntry::Value("0.0.1".to_string()));
                kernel.insert("ostype".to_string(), SysEntry::Value("AthenaOS".to_string()));
                kernel.insert(
                    "osrelease".to_string(),
                    SysEntry::Value("0.0.1-rae".to_string()),
                );
                kernel
            }),
        );

        entries.insert(
            "devices".to_string(),
            SysEntry::Dir({
                let mut devices = BTreeMap::new();
                devices.insert(
                    "cpu".to_string(),
                    SysEntry::Dir({
                        let mut cpu = BTreeMap::new();
                        cpu.insert("count".to_string(), SysEntry::Value("1".to_string()));
                        cpu.insert(
                            "vendor".to_string(),
                            SysEntry::Value("GenuineIntel".to_string()),
                        );
                        cpu
                    }),
                );
                devices.insert(
                    "memory".to_string(),
                    SysEntry::Dir({
                        let mut mem = BTreeMap::new();
                        mem.insert(
                            "total".to_string(),
                            SysEntry::Value("268435456".to_string()),
                        );
                        mem.insert(
                            "available".to_string(),
                            SysEntry::Value("134217728".to_string()),
                        );
                        mem
                    }),
                );
                devices
            }),
        );

        entries.insert(
            "fs".to_string(),
            SysEntry::Dir({
                let mut fs = BTreeMap::new();
                fs.insert(
                    "supported".to_string(),
                    SysEntry::Value("ext4 fat32 ntfs tmpfs sysfs".to_string()),
                );
                fs
            }),
        );

        Self {
            entries,
            mounted: false,
        }
    }

    fn resolve_entry(&self, path_parts: &[&str]) -> Option<&SysEntry> {
        let mut current: &BTreeMap<String, SysEntry> = &self.entries;

        for (i, part) in path_parts.iter().enumerate() {
            if let Some(entry) = current.get(*part) {
                if i == path_parts.len() - 1 {
                    return Some(entry);
                }
                match entry {
                    SysEntry::Dir(children) => current = children,
                    _ => return None,
                }
            } else {
                return None;
            }
        }
        None
    }

    fn list_dir(&self, path_parts: &[&str]) -> Option<Vec<String>> {
        if path_parts.is_empty() {
            return Some(self.entries.keys().cloned().collect());
        }

        let mut current: &BTreeMap<String, SysEntry> = &self.entries;
        for part in path_parts {
            if let Some(entry) = current.get(*part) {
                match entry {
                    SysEntry::Dir(children) => current = children,
                    _ => return None,
                }
            } else {
                return None;
            }
        }
        Some(current.keys().cloned().collect())
    }

    fn inode_for_path(&self, parts: &[&str]) -> u64 {
        let mut hash: u64 = 0x1000;
        for part in parts {
            for b in part.bytes() {
                hash = hash.wrapping_mul(31).wrapping_add(b as u64);
            }
            hash = hash.wrapping_mul(37);
        }
        hash | 0x8000_0000_0000_0000
    }
}

impl Filesystem for SysFs {
    fn name(&self) -> &str {
        "sysfs"
    }

    fn mount(&mut self, _device: u64) -> Result<(), FsError> {
        self.mounted = true;
        Ok(())
    }

    fn unmount(&mut self) -> Result<(), FsError> {
        self.mounted = false;
        Ok(())
    }

    fn stat_fs(&self) -> Result<FsStats, FsError> {
        Ok(FsStats {
            total_blocks: 0,
            free_blocks: 0,
            available_blocks: 0,
            total_inodes: 0,
            free_inodes: 0,
            block_size: 4096,
            max_name_length: 255,
            fs_type: "sysfs".to_string(),
        })
    }

    fn lookup(&self, _parent: u64, name: &str) -> Result<InodeInfo, FsError> {
        if !self.mounted {
            return Err(FsError::NotMounted);
        }

        let parts: Vec<&str> = name.split('/').filter(|s| !s.is_empty()).collect();
        if let Some(entry) = self.resolve_entry(&parts) {
            let ft = match entry {
                SysEntry::Dir(_) => FileType::Directory,
                _ => FileType::Regular,
            };
            Ok(InodeInfo {
                inode: self.inode_for_path(&parts),
                file_type: ft,
                mode: 0o444,
                uid: 0,
                gid: 0,
                size: 0,
                blocks: 0,
                block_size: 4096,
                atime: 0,
                mtime: 0,
                ctime: 0,
                crtime: 0,
                nlinks: 1,
                flags: 0,
            })
        } else {
            Err(FsError::NotFound)
        }
    }

    fn read_dir(&self, _inode: u64) -> Result<Vec<DirEntry>, FsError> {
        if !self.mounted {
            return Err(FsError::NotMounted);
        }
        let names = self.list_dir(&[]).ok_or(FsError::NotFound)?;
        let mut entries = Vec::new();
        for (i, name) in names.iter().enumerate() {
            entries.push(DirEntry {
                inode: i as u64 + 2,
                name: name.clone(),
                file_type: FileType::Directory,
                offset: i as u64,
            });
        }
        Ok(entries)
    }

    fn read_file(&self, _inode: u64, _offset: u64, _buf: &mut [u8]) -> Result<usize, FsError> {
        if !self.mounted {
            return Err(FsError::NotMounted);
        }
        Err(FsError::NotSupported)
    }

    fn write_file(&mut self, _inode: u64, _offset: u64, _data: &[u8]) -> Result<usize, FsError> {
        Err(FsError::ReadOnly)
    }

    fn create(
        &mut self,
        _parent: u64,
        _name: &str,
        _file_type: FileType,
        _mode: u16,
    ) -> Result<u64, FsError> {
        Err(FsError::ReadOnly)
    }

    fn unlink(&mut self, _parent: u64, _name: &str) -> Result<(), FsError> {
        Err(FsError::ReadOnly)
    }

    fn mkdir(&mut self, _parent: u64, _name: &str, _mode: u16) -> Result<u64, FsError> {
        Err(FsError::ReadOnly)
    }

    fn rmdir(&mut self, _parent: u64, _name: &str) -> Result<(), FsError> {
        Err(FsError::ReadOnly)
    }

    fn rename(
        &mut self,
        _old_parent: u64,
        _old_name: &str,
        _new_parent: u64,
        _new_name: &str,
    ) -> Result<(), FsError> {
        Err(FsError::ReadOnly)
    }

    fn truncate(&mut self, _inode: u64, _size: u64) -> Result<(), FsError> {
        Err(FsError::ReadOnly)
    }

    fn symlink(&mut self, _parent: u64, _name: &str, _target: &str) -> Result<u64, FsError> {
        Err(FsError::ReadOnly)
    }

    fn readlink(&self, _inode: u64) -> Result<String, FsError> {
        Err(FsError::NotSupported)
    }

    fn chmod(&mut self, _inode: u64, _mode: u16) -> Result<(), FsError> {
        Err(FsError::ReadOnly)
    }

    fn chown(&mut self, _inode: u64, _uid: u32, _gid: u32) -> Result<(), FsError> {
        Err(FsError::ReadOnly)
    }

    fn sync(&mut self) -> Result<(), FsError> {
        Ok(())
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// §7  GLOBAL VFS + INIT
// ═══════════════════════════════════════════════════════════════════════════════

pub static VFS: Mutex<Option<Vfs>> = Mutex::new(None);

pub fn init() {
    let mut vfs = Vfs::new();

    let _ = vfs.mount("/", "none", "tmpfs", MountFlags::new());
    let _ = vfs.mount("/sys", "none", "sysfs", MountFlags::read_only());
    let _ = vfs.mount("/tmp", "none", "tmpfs", MountFlags::new());

    *VFS.lock() = Some(vfs);
}
