//! RedoxFS `Tree` / `TreeList` / `TreePtr` — first R08 port slice.
//!
//! Adapted from `redox_reference_upstream/redoxfs/src/tree.rs` (MIT).
//! Changes for RaeFS: `no_std`, plain `u32` LE in `TreePtr` (no `endian_num` crate).

use core::{marker::PhantomData, mem, ops, slice};

/// RedoxFS block size (matches upstream `BLOCK_SIZE`).
pub const BLOCK_SIZE: usize = 4096;

/// 1 << 8 = 256 entries per `TreeList` (minus link slots).
pub const TREE_LIST_SHIFT: u32 = 8;
pub const TREE_LIST_ENTRIES: usize = (1 << TREE_LIST_SHIFT) - 2;

/// Opaque leaf block placeholder for type-parameter plumbing.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct BlockRaw;

/// A tree with four index levels (RedoxFS inode B+ tree root layout).
pub type Tree = TreeList<TreeList<TreeList<TreeList<BlockRaw>>>>;

/// `TreePtr` plus the block payload it references (in-memory mount helper).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TreeData<T> {
    id: u32,
    data: T,
}

impl<T> TreeData<T> {
    pub const fn new(id: u32, data: T) -> Self {
        Self { id, data }
    }

    pub const fn id(&self) -> u32 {
        self.id
    }

    pub const fn data(&self) -> &T {
        &self.data
    }

    pub fn data_mut(&mut self) -> &mut T {
        &mut self.data
    }

    pub fn into_data(self) -> T {
        self.data
    }

    pub fn ptr(&self) -> TreePtr<T> {
        TreePtr::new(self.id)
    }
}

/// One level of the RedoxFS tree: up to [`TREE_LIST_ENTRIES`] child pointers + fullness bitmap.
#[repr(C)]
pub struct TreeList<T> {
    pub ptrs: [BlockPtr<T>; TREE_LIST_ENTRIES],
    pub full_flags: [u128; 2],
}

impl<T> TreeList<T> {
    pub const fn empty() -> Self {
        Self {
            ptrs: [BlockPtr::NULL; TREE_LIST_ENTRIES],
            full_flags: [0; 2],
        }
    }

    pub fn tree_list_is_full(&self) -> bool {
        self.full_flags[1] == u128::MAX & !(3 << 126) && self.full_flags[0] == u128::MAX
    }

    pub fn tree_list_is_empty(&self) -> bool {
        self.ptrs.iter().all(|p| p.is_null())
    }

    pub fn branch_is_full(&self, index: usize) -> bool {
        assert!(index < TREE_LIST_ENTRIES);
        let shift = index % 128;
        let full_flags_index = index / 128;
        self.full_flags[full_flags_index] & (1 << shift) != 0
    }

    pub fn set_branch_full(&mut self, index: usize, full: bool) {
        assert!(index < TREE_LIST_ENTRIES);
        let shift = index % 128;
        let full_flags_index = index / 128;
        if full {
            self.full_flags[full_flags_index] |= 1 << shift;
        } else {
            self.full_flags[full_flags_index] &= !(1 << shift);
        }
    }
}

impl<T> ops::Deref for TreeList<T> {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        unsafe {
            slice::from_raw_parts(
                self as *const TreeList<T> as *const u8,
                mem::size_of::<TreeList<T>>(),
            )
        }
    }
}

impl<T> ops::DerefMut for TreeList<T> {
    fn deref_mut(&mut self) -> &mut [u8] {
        unsafe {
            slice::from_raw_parts_mut(
                self as *mut TreeList<T> as *mut u8,
                mem::size_of::<TreeList<T>>(),
            )
        }
    }
}

/// Child pointer stored inside a [`TreeList`] (matches RedoxFS `BlockPtr` layout: addr + hash).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct BlockPtr<T> {
    addr: u64,
    hash: u64,
    _phantom: PhantomData<T>,
}

impl<T> BlockPtr<T> {
    pub const NULL: Self = Self {
        addr: 0,
        hash: 0,
        _phantom: PhantomData,
    };

    pub const fn new(addr: u64) -> Self {
        Self {
            addr,
            hash: 0,
            _phantom: PhantomData,
        }
    }

    pub const fn addr(&self) -> u64 {
        self.addr
    }

    pub const fn is_null(&self) -> bool {
        self.addr == 0
    }
}

/// Encoded position inside the 4-level RedoxFS tree (single `u32` id).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TreePtr<T> {
    id: u32,
    _phantom: PhantomData<T>,
}

impl<T> TreePtr<T> {
    pub const fn root() -> Self {
        Self::new(1)
    }

    pub const fn new(id: u32) -> Self {
        Self {
            id,
            _phantom: PhantomData,
        }
    }

    pub const fn from_indexes(indexes: (usize, usize, usize, usize)) -> Self {
        let id = ((indexes.0 << (3 * TREE_LIST_SHIFT)) as u32)
            | ((indexes.1 << (2 * TREE_LIST_SHIFT)) as u32)
            | ((indexes.2 << TREE_LIST_SHIFT) as u32)
            | (indexes.3 as u32);
        Self::new(id)
    }

    pub const fn id(&self) -> u32 {
        self.id
    }

    pub const fn is_null(&self) -> bool {
        self.id == 0
    }

    pub const fn indexes(&self) -> (usize, usize, usize, usize) {
        const SHIFT: u32 = TREE_LIST_SHIFT;
        const NUM: u32 = 1 << SHIFT;
        const MASK: u32 = NUM - 1;
        let id = self.id;
        let i3 = ((id >> (3 * SHIFT)) & MASK) as usize;
        let i2 = ((id >> (2 * SHIFT)) & MASK) as usize;
        let i1 = ((id >> SHIFT) & MASK) as usize;
        let i0 = (id & MASK) as usize;
        (i3, i2, i1, i0)
    }

    pub const fn to_bytes(self) -> [u8; 4] {
        self.id.to_le_bytes()
    }

    pub const fn from_bytes(bytes: [u8; 4]) -> Self {
        Self::new(u32::from_le_bytes(bytes))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tree_list_fits_block() {
        assert_eq!(mem::size_of::<TreeList<BlockRaw>>(), BLOCK_SIZE);
    }

    #[test]
    fn tree_ptr_roundtrip() {
        let ptr = TreePtr::<BlockRaw>::from_indexes((1, 2, 3, 4));
        assert_eq!(ptr.indexes(), (1, 2, 3, 4));
        assert_eq!(
            TreePtr::<BlockRaw>::from_bytes(ptr.to_bytes()).id(),
            ptr.id()
        );
    }
}
