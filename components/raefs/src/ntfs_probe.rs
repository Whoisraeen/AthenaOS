//! Read-only NTFS — Concept §"the user owns the machine" (dual-boot migration).
//!
//! To migrate a user off Windows we must read their existing NTFS partition:
//! documents, save games, the Steam library. The `ntfs` crate is the pure-Rust,
//! no_std read-only reader; this module adapts a byte source to its `binrw` io
//! traits and exposes a safe probe. Behind the `ntfs_ro` feature.

extern crate alloc;
// binrw 0.11 — pinned to ntfs 0.4's exact version so these io traits are the
// ones ntfs's reader bound requires.
use binrw::io::{Read, Seek, SeekFrom};

/// A cursor over an in-memory volume image implementing the `binrw` io traits
/// the `ntfs` crate requires. The on-device adapter wraps a `BlockDevice`
/// instead; the parse path is identical.
pub struct SliceReader<'a> {
    data: &'a [u8],
    pos: u64,
}

impl<'a> SliceReader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }
}

impl<'a> Read for SliceReader<'a> {
    fn read(&mut self, buf: &mut [u8]) -> binrw::io::Result<usize> {
        let pos = self.pos as usize;
        let n = core::cmp::min(buf.len(), self.data.len().saturating_sub(pos));
        buf[..n].copy_from_slice(&self.data[pos..pos + n]);
        self.pos += n as u64;
        Ok(n)
    }
}

impl<'a> Seek for SliceReader<'a> {
    fn seek(&mut self, pos: SeekFrom) -> binrw::io::Result<u64> {
        let new = match pos {
            SeekFrom::Start(n) => n,
            SeekFrom::End(n) => (self.data.len() as i64 + n).max(0) as u64,
            SeekFrom::Current(n) => (self.pos as i64 + n).max(0) as u64,
        };
        self.pos = new;
        Ok(self.pos)
    }
}

/// Cheap, panic-free NTFS boot-sector check: the "NTFS    " OEM id at offset 3,
/// the 0x55AA boot signature, and sane geometry. The `ntfs` crate's reader
/// `unreachable!()`s on a zeroed/garbage boot sector (it computes a bogus MFT
/// offset and EOFs), so we MUST gate it with this first — a junk partition can
/// never reach the panic path.
pub fn looks_like_ntfs(boot_sector: &[u8]) -> bool {
    if boot_sector.len() < 512 {
        return false;
    }
    if &boot_sector[3..11] != b"NTFS    " {
        return false;
    }
    if boot_sector[510] != 0x55 || boot_sector[511] != 0xAA {
        return false;
    }
    // bytes_per_sector (off 11, u16 LE) must be a real sector size, and
    // sectors_per_cluster (off 13) must be nonzero — guards the geometry the
    // ntfs reader divides/seeks by.
    let bps = u16::from_le_bytes([boot_sector[11], boot_sector[12]]);
    matches!(bps, 512 | 1024 | 2048 | 4096) && boot_sector[13] != 0
}

/// True if `data` is a mountable NTFS volume. Junk/non-NTFS input returns false
/// via [`looks_like_ntfs`] without ever entering the panic-prone ntfs reader.
pub fn is_ntfs_volume(data: &[u8]) -> bool {
    if !looks_like_ntfs(data) {
        return false;
    }
    let mut reader = SliceReader::new(data);
    ntfs::Ntfs::new(&mut reader).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_ntfs_gracefully() {
        // Junk must be rejected by the signature gate, never reaching the
        // panic-prone ntfs reader.
        assert!(!looks_like_ntfs(&[0u8; 1024]));
        assert!(!looks_like_ntfs(b"short"));
        assert!(!is_ntfs_volume(&[0u8; 1024]));
        assert!(!is_ntfs_volume(b"this is not a filesystem image at all"));
    }

    #[test]
    fn accepts_valid_ntfs_signature() {
        // A boot sector with the NTFS OEM id, sane geometry, and the 0x55AA
        // signature passes the cheap gate (full mount still needs a real MFT).
        let mut bs = [0u8; 512];
        bs[3..11].copy_from_slice(b"NTFS    ");
        bs[11..13].copy_from_slice(&512u16.to_le_bytes()); // bytes/sector
        bs[13] = 8; // sectors/cluster
        bs[510] = 0x55;
        bs[511] = 0xAA;
        assert!(looks_like_ntfs(&bs));
        // Wrong OEM id -> rejected.
        bs[3] = b'X';
        assert!(!looks_like_ntfs(&bs));
    }

    #[test]
    fn slice_reader_reads_and_seeks() {
        let data = [1u8, 2, 3, 4, 5, 6, 7, 8];
        let mut r = SliceReader::new(&data);
        let mut buf = [0u8; 4];
        assert_eq!(r.read(&mut buf).unwrap(), 4);
        assert_eq!(buf, [1, 2, 3, 4]);
        r.seek(SeekFrom::Start(6)).unwrap();
        let mut tail = [0u8; 4];
        let n = r.read(&mut tail).unwrap();
        assert_eq!(n, 2); // only 2 bytes left
        assert_eq!(&tail[..2], &[7, 8]);
    }
}
