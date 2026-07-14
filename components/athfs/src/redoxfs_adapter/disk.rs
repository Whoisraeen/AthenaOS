use super::header::{header_from_bytes, Header, BLOCK_SIZE};
use crate::{FsError, FsResult};
use alloc::vec;

/// A disk block storage device trait.
/// Adapted from `redoxfs::Disk` (MIT).
pub trait Disk {
    /// Read blocks from disk
    ///
    /// # Safety
    /// Unsafe to discourage use, use filesystem wrappers instead
    unsafe fn read_at(&mut self, block: u64, buffer: &mut [u8]) -> FsResult<usize>;

    /// Write blocks from disk
    ///
    /// # Safety
    /// Unsafe to discourage use, use filesystem wrappers instead
    unsafe fn write_at(&mut self, block: u64, buffer: &[u8]) -> FsResult<usize>;

    /// Get size of disk in bytes
    fn size(&mut self) -> FsResult<u64>;
}

/// Probes the first block of the disk for a valid RedoxFS superblock.
pub fn probe_superblock<D: Disk>(disk: &mut D) -> FsResult<Header> {
    let mut buffer = vec![0u8; BLOCK_SIZE];

    // Safety: The buffer is exactly one BLOCK_SIZE, which is what the fs block size is.
    let bytes_read = unsafe { disk.read_at(0, &mut buffer)? };
    if bytes_read < BLOCK_SIZE {
        return Err(FsError::IoError);
    }

    match header_from_bytes(&buffer) {
        Some(h) => Ok(h),
        None => Err(FsError::CorruptedData),
    }
}
