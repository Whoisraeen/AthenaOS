//! Full-disk encryption binding for AthFS (Concept §AthFS: "encryption is a
//! property of the volume, not an app feature — lose the laptop, lose
//! nothing"). MasterChecklist Phase 3.8 — "LUKS-equivalent FDE for AthFS
//! root".
//!
//! The cipher core lives in `encryption.rs` (AES-256-XTS per-sector, key
//! slots, AF-split, RaeVault header). This module is the BINDING: an
//! [`EncryptedBlockDevice`] that implements `block_io::BlockDevice` and
//! transparently AES-XTS-encrypts every sector on its way to the inner
//! device — so the whole AthFS volume, superblock and metadata included, is
//! ciphertext at rest. AthFS itself never knows: it mounts through the
//! wrapper exactly as it would a raw NVMe namespace.
//!
//! The smoketest is the proof that matters for FDE: format + mount AthFS
//! THROUGH the wrapper over a RAM disk, write a known canary file, then scan
//! the RAW underlying bytes — the canary (and any recognizable plaintext)
//! must be absent; reading back through the crypto layer must return it
//! intact; and decrypting sector 0 with a WRONG key must NOT yield what the
//! right key yields. Deterministic, identical on QEMU and iron.
//!
//! Boot-time unlock (passphrase from USB HID / TPM unseal) is the iron half
//! (MasterChecklist Phase 3.8 — separate items).

#![allow(dead_code)]

extern crate alloc;

use alloc::boxed::Box;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

use crate::block_io::BlockDevice;
use crate::encryption::{pbkdf2_sha256, AesXts};

static SECTORS_ENCRYPTED: AtomicU64 = AtomicU64::new(0);
static SECTORS_DECRYPTED: AtomicU64 = AtomicU64::new(0);

/// A RAM disk whose backing store is shared (`Arc`), so the smoketest can
/// inspect the RAW (post-encryption) bytes while AthFS writes through the
/// crypto wrapper. Pure memory: no safe-mode interaction by construction.
pub struct SharedRamDisk {
    sectors: Arc<spin::Mutex<Vec<[u8; 512]>>>,
}

impl SharedRamDisk {
    pub fn new(sector_count: usize) -> (Self, Arc<spin::Mutex<Vec<[u8; 512]>>>) {
        let store = Arc::new(spin::Mutex::new(alloc::vec![[0u8; 512]; sector_count]));
        (
            Self {
                sectors: store.clone(),
            },
            store,
        )
    }

    pub fn from_store(store: Arc<spin::Mutex<Vec<[u8; 512]>>>) -> Self {
        Self { sectors: store }
    }
}

impl BlockDevice for SharedRamDisk {
    fn read_sector(&self, lba: u64, buf: &mut [u8]) -> Result<(), &'static str> {
        let lock = self.sectors.lock();
        let sec = lock
            .get(lba as usize)
            .ok_or("SharedRamDisk: LBA out of range")?;
        let len = buf.len().min(512);
        buf[..len].copy_from_slice(&sec[..len]);
        Ok(())
    }

    fn write_sector(&self, lba: u64, buf: &[u8]) -> Result<(), &'static str> {
        let mut lock = self.sectors.lock();
        let sec = lock
            .get_mut(lba as usize)
            .ok_or("SharedRamDisk: LBA out of range")?;
        let len = buf.len().min(512);
        sec[..len].copy_from_slice(&buf[..len]);
        Ok(())
    }

    fn sector_size(&self) -> usize {
        512
    }

    fn total_sectors(&self) -> u64 {
        self.sectors.lock().len() as u64
    }
}

/// The FDE wrapper: every sector is AES-256-XTS transformed (tweak = LBA) on
/// the way through, so the inner device only ever sees ciphertext. Wraps any
/// `BlockDevice` — RAM disk here, the real AthFS root partition on iron.
pub struct EncryptedBlockDevice {
    inner: Box<dyn BlockDevice>,
    xts: AesXts,
}

impl EncryptedBlockDevice {
    /// `key` is the 64-byte AES-256-XTS key (two AES-256 keys), normally the
    /// output of Argon2id over the user passphrase + volume salt.
    pub fn new(inner: Box<dyn BlockDevice>, key: &[u8; 64]) -> Option<Self> {
        Some(Self {
            inner,
            xts: AesXts::new(key).ok()?,
        })
    }
}

impl BlockDevice for EncryptedBlockDevice {
    fn read_sector(&self, lba: u64, buf: &mut [u8]) -> Result<(), &'static str> {
        if buf.len() != 512 {
            return Err("EncryptedBlockDevice: partial-sector read unsupported");
        }
        self.inner.read_sector(lba, buf)?;
        self.xts
            .decrypt_sector(lba, buf)
            .map_err(|_| "EncryptedBlockDevice: decrypt failed")?;
        SECTORS_DECRYPTED.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    fn write_sector(&self, lba: u64, buf: &[u8]) -> Result<(), &'static str> {
        if buf.len() != 512 {
            return Err("EncryptedBlockDevice: partial-sector write unsupported");
        }
        let mut ct = [0u8; 512];
        ct.copy_from_slice(buf);
        self.xts
            .encrypt_sector(lba, &mut ct)
            .map_err(|_| "EncryptedBlockDevice: encrypt failed")?;
        SECTORS_ENCRYPTED.fetch_add(1, Ordering::Relaxed);
        self.inner.write_sector(lba, &ct)
    }

    fn sector_size(&self) -> usize {
        512
    }

    fn total_sectors(&self) -> u64 {
        self.inner.total_sectors()
    }

    fn flush_cache(&self) -> Result<(), &'static str> {
        self.inner.flush_cache()
    }
}

/// Scan the raw sector store for a byte pattern, including across sector
/// boundaries (window = sector + pattern-length prefix of the next).
fn raw_contains(store: &[[u8; 512]], needle: &[u8]) -> bool {
    if needle.is_empty() || needle.len() > 512 {
        return false;
    }
    let mut window = Vec::with_capacity(512 + needle.len());
    for i in 0..store.len() {
        window.clear();
        window.extend_from_slice(&store[i]);
        if i + 1 < store.len() {
            window.extend_from_slice(&store[i + 1][..needle.len() - 1]);
        }
        if window.windows(needle.len()).any(|w| w == needle) {
            return true;
        }
    }
    false
}

pub fn init() {
    crate::serial_println!(
        "[fde] AES-256-XTS volume encryption ready (per-sector tweak=LBA, whole-volume incl. metadata)"
    );
}

/// Deterministic FDE proof: AthFS mounts through the AES-XTS wrapper over a
/// RAM disk; a canary file written through the FS must be (1) readable back
/// through the crypto layer, (2) ABSENT from the raw device bytes — and the
/// AthFS superblock magic must be ciphertext at rest too; (3) a wrong key
/// must not decrypt what the right key decrypts.
pub fn run_boot_smoketest() {
    const CANARY: &[u8] = b"FDE-PLAINTEXT-CANARY-athena-0x52414545";
    const PASSPHRASE: &[u8] = b"athena-fde-selftest-passphrase";
    const SALT: &[u8] = b"athena-fde-selftest-salt";

    // Real volumes derive this with Argon2id (RFC 9106); the selftest uses
    // PBKDF2 at low cost to keep boot time flat — the binding under test is
    // the XTS sector transform, not the KDF (Argon2id has its own KAT).
    let mut key = [0u8; 64];
    pbkdf2_sha256(PASSPHRASE, SALT, 1_000, &mut key);

    let (disk, store) = SharedRamDisk::new(4096);
    let Some(enc_dev) = EncryptedBlockDevice::new(Box::new(disk), &key) else {
        crate::serial_println!("[fde] smoketest: XTS key setup failed -> FAIL");
        return;
    };

    let io = crate::athfs::with_custom_athfs_device(Box::new(enc_dev), || {
        let wrote = crate::athfs::write_flat_file("fde-canary.txt", CANARY);
        let read = crate::athfs::read_flat_file("fde-canary.txt");
        (wrote, read.as_deref() == Some(CANARY))
    });
    let (wrote, read_ok) = io.unwrap_or((false, false));

    // (2) Raw bytes: no canary plaintext anywhere; superblock magic
    // ("AthFS!" little-endian on disk) must not be recognizable either.
    let (plaintext_leaked, magic_leaked) = {
        let guard = store.lock();
        let magic_le = 0x0052_6165_4653_21u64.to_le_bytes();
        (
            raw_contains(&guard, CANARY),
            raw_contains(&guard, &magic_le[..6]),
        )
    };

    // (3) Wrong key: sector 0 decrypted with a different key must differ
    // from the right key's plaintext (and the right key's plaintext must
    // differ from the raw ciphertext, i.e. encryption actually happened).
    let mut wrong_key = key;
    wrong_key[0] ^= 0xFF;
    let right_dev =
        EncryptedBlockDevice::new(Box::new(SharedRamDisk::from_store(store.clone())), &key);
    let wrong_dev = EncryptedBlockDevice::new(
        Box::new(SharedRamDisk::from_store(store.clone())),
        &wrong_key,
    );
    let (mut right0, mut wrong0, raw0) = ([0u8; 512], [0u8; 512], store.lock()[0]);
    let right_read = right_dev
        .map(|d| d.read_sector(0, &mut right0).is_ok())
        .unwrap_or(false);
    let wrong_read = wrong_dev
        .map(|d| d.read_sector(0, &mut wrong0).is_ok())
        .unwrap_or(false);
    let cipher_at_rest = right_read && right0 != raw0;
    let wrong_key_rejected = wrong_read && wrong0 != right0;

    let pass = wrote
        && read_ok
        && !plaintext_leaked
        && !magic_leaked
        && cipher_at_rest
        && wrong_key_rejected;
    crate::serial_println!(
        "[fde] smoketest: write={} read_through_crypto={} plaintext_on_disk={} superblock_plaintext={} cipher_at_rest={} wrong_key_rejected={} -> {}",
        wrote,
        read_ok,
        plaintext_leaked,
        magic_leaked,
        cipher_at_rest,
        wrong_key_rejected,
        if pass { "PASS" } else { "FAIL" },
    );
}

/// `/proc/athena/fde` — FDE binding state.
pub fn dump_text() -> String {
    alloc::format!(
        "# AthFS full-disk encryption (AES-256-XTS, tweak=LBA)\ncipher: AES-256-XTS\nkdf: Argon2id (RFC 9106; selftest uses PBKDF2-low)\nsectors_encrypted: {}\nsectors_decrypted: {}\nroot_volume_encrypted: false (opt-in at install; binding proven by smoketest)\n",
        SECTORS_ENCRYPTED.load(Ordering::Relaxed),
        SECTORS_DECRYPTED.load(Ordering::Relaxed),
    )
}
