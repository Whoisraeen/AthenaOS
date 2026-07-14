//! RedoxFS on-disk tree layer adapted for AthFS (`no_std`).
//!
//! Provenance: Redox OS `redoxfs` `src/tree.rs` (MIT, see `vendor/redoxfs/LICENSE`).
//! Types are trimmed to the 4-level `TreeList` pointer tower used for inode lookup;
//! disk I/O and transactions remain in future R08 slices.

pub mod disk;
pub mod header;
pub mod tree;

pub use disk::{probe_superblock, Disk};
pub use header::{header_from_bytes, Header, BLOCK_SIZE as REDOXFS_BLOCK_SIZE, SIGNATURE, VERSION};
pub use tree::{Tree, TreeData, TreeList, TreePtr, TREE_LIST_ENTRIES, TREE_LIST_SHIFT};
