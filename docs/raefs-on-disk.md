# AthFS On-Disk Format Specification v0.1

AthFS is a high-performance, Copy-on-Write (CoW) B-tree filesystem designed for gaming workloads. It prioritizes sequential read performance for assets while providing iOS-grade per-app isolation.

## 1. Physical Layout

- **Block Size**: 4096 bytes.
- **Sectors per Block**: 8 (assuming 512-byte sectors).

| Block | Content | Description |
|---|---|---|
| 0 | Superblock | FS identification, root pointers, and features. |
| 1 | Inode Bitmap | Bitmask for allocated inodes. |
| 2 | Block Bitmap | Bitmask for allocated physical blocks. |
| 3 | Inode Table Start | Array of `DiskInode` structures. |
| 4 | Refcount Table | Reference counts for CoW block sharing. |
| 5 | Snapshot Table | Slots for frozen root pointers. |
| 6 | Journal / WAL | Write-ahead log for atomic metadata updates. |
| 7 | Bucket Table | App-id to subdirectory root mapping. |
| 8 | Versioned Metadata | Versioning info for config files. |
| 9+ | Data / B-tree Nodes | Actual file contents and tree structure. |

## 2. Core Structures

### 2.1 Superblock (Block 0)

Occupies the first 4096 bytes. Magic: `0x526165465321` ("AthFS!").

| Offset | Type | Field |
|---|---|---|
| 0 | u64 | Magic |
| 8 | u64 | Total Blocks |
| 16 | u64 | Free Blocks |
| 24 | u64 | Root Inode ID |
| 32 | u64 | Inode Bitmap Block |
| 40 | u64 | Block Bitmap Block |
| 48 | u64 | Inode Table Block |
| 56 | u64 | Refcount Block |
| 64 | u64 | Snapshot Block |
| 72 | u64 | Journal Block |
| 80 | u32 | Snapshot Count |
| 84 | u32 | Journal Sequence |
| 88 | u8 | Encryption Flag |
| 89 | u8 | Compression Flag |
| 90 | u8 | Tiering Flag |

### 2.2 Disk Inode (128 bytes)

32 inodes per 4KB block.

| Offset | Type | Field |
|---|---|---|
| 0 | u64 | Inode ID |
| 8 | u64 | File Size (bytes) |
| 16 | u8 | Type (0=File, 1=Dir, 2=Symlink) |
| 17 | u8 | Flags |
| 24 | u64 | B-tree Root Block |
| 32 | u64[12] | Direct Blocks (Inline Cache / Small Files) |
| 120 | u64 | Indirect Block (Legacy) / B-tree Depth |

### 2.3 B-tree Node (4KB)

AthFS uses a B-tree to map **File Offsets** to **Physical Extents**.

**Internal Node Layout:**
- Header (Magic, Level, Key Count)
- Keys: [Logical Block Offset; N]
- Values: [Physical Block Number; N+1]

**Leaf Node Layout:**
- Header (Magic, Level, Key Count)
- Extents: [Logical Offset, Physical Block, Length, Flags; N]

## 3. Copy-on-Write (CoW) Semantics

Any modification to a data block or B-tree node triggers:
1. Allocation of a new block.
2. Writing updated data to the new block.
3. Updating the parent B-tree node to point to the new block (which itself triggers a CoW write up to the root).
4. Updating the Inode.

## 4. Journaling (Undo Log)

The Journal acts as a safety net for metadata updates that cannot be made atomic via CoW (e.g., updating the block bitmap and inode table synchronously).

Entries: `[Seq, Op, InodeID, OldBlock, NewBlock, CommittedFlag]`.
Operation 1 (COW_WRITE) ensures that if the system crashes after allocating a block but before updating the inode, the block is freed during replay.

## 5. Security & Isolation

Per-app "Data Buckets" are implemented as subdirectories mapped in the `Bucket Table`. The kernel enforces that an app can only access its own bucket unless `BCAP_READ_SHARED` is granted.
