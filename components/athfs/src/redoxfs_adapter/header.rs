//! RedoxFS on-disk superblock header (R08 slice 2).
//!
//! Adapted from `redox_reference_upstream/redoxfs/src/header.rs` and `lib.rs` (MIT).
//! `no_std` subset: signature/version validation only — no encryption hash path yet.

use core::mem;

use super::tree::BlockPtr;

pub const BLOCK_SIZE: usize = 4096;
pub const SIGNATURE: &[u8; 8] = b"RedoxFS\0";
pub const VERSION: u64 = 8;

/// Placeholder for typed tree root in header (full `Tree` type in `tree.rs`).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TreeRoot {
    pub addr: u64,
    pub hash: u64,
}

impl TreeRoot {
    pub const fn null() -> Self {
        Self { addr: 0, hash: 0 }
    }
}

/// First block of a RedoxFS volume (packed layout matches upstream).
#[repr(C, packed)]
pub struct Header {
    pub signature: [u8; 8],
    pub version: u64,
    pub uuid: [u8; 16],
    pub size: u64,
    pub generation: u64,
    pub tree: BlockPtr<TreeRoot>,
    pub alloc: BlockPtr<TreeRoot>,
    // Key slots and remainder omitted in slice 2 — zeroed when mounting read-only probe.
    pub _rest: [u8; BLOCK_SIZE - 8 - 8 - 16 - 8 - 8 - 16 - 16],
}

impl Header {
    pub fn valid(&self) -> bool {
        if &self.signature != SIGNATURE {
            return false;
        }
        if self.version != VERSION {
            return false;
        }
        true
    }

    pub fn uuid(&self) -> [u8; 16] {
        self.uuid
    }

    pub fn size_sectors(&self) -> u64 {
        self.size
    }
}

/// Read header from a 4 KiB block buffer.
pub fn header_from_bytes(block: &[u8]) -> Option<Header> {
    if block.len() < mem::size_of::<Header>() {
        return None;
    }
    // Safety: `Header` is `repr(C, packed)` and fits in the first sector.
    let h = unsafe { core::ptr::read_unaligned(block.as_ptr() as *const Header) };
    if h.valid() {
        Some(h)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_size_fits_block() {
        assert!(mem::size_of::<Header>() <= BLOCK_SIZE);
    }

    #[test]
    fn signature_roundtrip() {
        let mut block = [0u8; BLOCK_SIZE];
        block[..8].copy_from_slice(SIGNATURE);
        block[8..16].copy_from_slice(&VERSION.to_le_bytes());
        let h = header_from_bytes(&block).expect("valid header");
        let version = unsafe { core::ptr::read_unaligned(core::ptr::addr_of!(h.version)) };
        assert_eq!(version, VERSION);
    }
}
