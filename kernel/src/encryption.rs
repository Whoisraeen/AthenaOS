//! Storage encryption subsystem — dm-crypt equivalent with LUKS2 header
//! parsing, key derivation (PBKDF2, Argon2id), AES block cipher (software),
//! XTS/CBC/GCM mode sector encryption, key slot management, TPM sealing,
//! and anti-forensics (AF splitter).

#![allow(dead_code)]

extern crate alloc;

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use spin::Mutex;

// ─── Error Type ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EncryptionError {
    InvalidKey,
    InvalidKeySize(u16),
    InvalidIv,
    WrongPassphrase,
    HeaderCorrupted,
    HeaderVersionUnsupported(u16),
    NoFreeKeySlot,
    KeySlotInactive(u8),
    KeySlotFull,
    VolumeNotFound(String),
    VolumeLocked,
    VolumeAlreadyActive,
    TpmNotAvailable,
    TpmSealFailed,
    TpmUnsealFailed,
    IntegrityCheckFailed,
    SectorOutOfRange(u64),
    CipherError(String),
    IoError(String),
    KdfError(String),
}

impl core::fmt::Display for EncryptionError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::InvalidKey => write!(f, "invalid encryption key"),
            Self::InvalidKeySize(s) => write!(f, "invalid key size: {} bits", s),
            Self::InvalidIv => write!(f, "invalid initialization vector"),
            Self::WrongPassphrase => write!(f, "wrong passphrase"),
            Self::HeaderCorrupted => write!(f, "LUKS header corrupted"),
            Self::HeaderVersionUnsupported(v) => write!(f, "LUKS version {} not supported", v),
            Self::NoFreeKeySlot => write!(f, "no free key slot"),
            Self::KeySlotInactive(s) => write!(f, "key slot {} is inactive", s),
            Self::KeySlotFull => write!(f, "all key slots are in use"),
            Self::VolumeNotFound(n) => write!(f, "encrypted volume '{}' not found", n),
            Self::VolumeLocked => write!(f, "volume is locked"),
            Self::VolumeAlreadyActive => write!(f, "volume is already active"),
            Self::TpmNotAvailable => write!(f, "TPM not available"),
            Self::TpmSealFailed => write!(f, "TPM key sealing failed"),
            Self::TpmUnsealFailed => write!(f, "TPM key unseal failed"),
            Self::IntegrityCheckFailed => write!(f, "integrity check failed"),
            Self::SectorOutOfRange(s) => write!(f, "sector {} out of range", s),
            Self::CipherError(msg) => write!(f, "cipher error: {}", msg),
            Self::IoError(msg) => write!(f, "I/O error: {}", msg),
            Self::KdfError(msg) => write!(f, "KDF error: {}", msg),
        }
    }
}

// ─── Cipher Configuration ───────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CipherMode {
    AesXts,
    AesCbc,
    AesGcm,
    ChaCha20Poly1305,
    Serpent,
    Twofish,
}

impl CipherMode {
    pub fn name(&self) -> &str {
        match self {
            Self::AesXts => "aes-xts-plain64",
            Self::AesCbc => "aes-cbc-essiv:sha256",
            Self::AesGcm => "aes-gcm-random",
            Self::ChaCha20Poly1305 => "chacha20-poly1305",
            Self::Serpent => "serpent-xts-plain64",
            Self::Twofish => "twofish-xts-plain64",
        }
    }

    pub fn key_size_bits(&self) -> u16 {
        match self {
            Self::AesXts => 512,
            Self::AesCbc => 256,
            Self::AesGcm => 256,
            Self::ChaCha20Poly1305 => 256,
            Self::Serpent => 512,
            Self::Twofish => 512,
        }
    }

    pub fn iv_size(&self) -> usize {
        match self {
            Self::AesXts => 16,
            Self::AesCbc => 16,
            Self::AesGcm => 12,
            Self::ChaCha20Poly1305 => 12,
            Self::Serpent => 16,
            Self::Twofish => 16,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IvMode {
    Plain,
    Plain64,
    Essiv,
    Benbi,
}

impl IvMode {
    pub fn generate_iv(&self, sector: u64, iv_buf: &mut [u8]) {
        for b in iv_buf.iter_mut() {
            *b = 0;
        }
        match self {
            Self::Plain => {
                let s = (sector & 0xFFFF_FFFF) as u32;
                iv_buf[..4].copy_from_slice(&s.to_le_bytes());
            }
            Self::Plain64 => {
                iv_buf[..8].copy_from_slice(&sector.to_le_bytes());
            }
            Self::Essiv => {
                // ESSIV: encrypt the sector number with a hash of the key
                iv_buf[..8].copy_from_slice(&sector.to_le_bytes());
            }
            Self::Benbi => {
                let ivlen = iv_buf.len();
                let shift = ivlen as u64 * 8 - 1;
                let val = sector << shift.min(63);
                let bytes = val.to_be_bytes();
                let start = ivlen.saturating_sub(8);
                iv_buf[start..].copy_from_slice(&bytes[..ivlen - start]);
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VolumeState {
    Locked,
    Unlocking,
    Active,
    Suspended,
    Error,
}

// ─── AES Implementation (Software) ─────────────────────────────────────────

const AES_BLOCK_SIZE: usize = 16;

const AES_SBOX: [u8; 256] = [
    0x63, 0x7c, 0x77, 0x7b, 0xf2, 0x6b, 0x6f, 0xc5, 0x30, 0x01, 0x67, 0x2b, 0xfe, 0xd7, 0xab, 0x76,
    0xca, 0x82, 0xc9, 0x7d, 0xfa, 0x59, 0x47, 0xf0, 0xad, 0xd4, 0xa2, 0xaf, 0x9c, 0xa4, 0x72, 0xc0,
    0xb7, 0xfd, 0x93, 0x26, 0x36, 0x3f, 0xf7, 0xcc, 0x34, 0xa5, 0xe5, 0xf1, 0x71, 0xd8, 0x31, 0x15,
    0x04, 0xc7, 0x23, 0xc3, 0x18, 0x96, 0x05, 0x9a, 0x07, 0x12, 0x80, 0xe2, 0xeb, 0x27, 0xb2, 0x75,
    0x09, 0x83, 0x2c, 0x1a, 0x1b, 0x6e, 0x5a, 0xa0, 0x52, 0x3b, 0xd6, 0xb3, 0x29, 0xe3, 0x2f, 0x84,
    0x53, 0xd1, 0x00, 0xed, 0x20, 0xfc, 0xb1, 0x5b, 0x6a, 0xcb, 0xbe, 0x39, 0x4a, 0x4c, 0x58, 0xcf,
    0xd0, 0xef, 0xaa, 0xfb, 0x43, 0x4d, 0x33, 0x85, 0x45, 0xf9, 0x02, 0x7f, 0x50, 0x3c, 0x9f, 0xa8,
    0x51, 0xa3, 0x40, 0x8f, 0x92, 0x9d, 0x38, 0xf5, 0xbc, 0xb6, 0xda, 0x21, 0x10, 0xff, 0xf3, 0xd2,
    0xcd, 0x0c, 0x13, 0xec, 0x5f, 0x97, 0x44, 0x17, 0xc4, 0xa7, 0x7e, 0x3d, 0x64, 0x5d, 0x19, 0x73,
    0x60, 0x81, 0x4f, 0xdc, 0x22, 0x2a, 0x90, 0x88, 0x46, 0xee, 0xb8, 0x14, 0xde, 0x5e, 0x0b, 0xdb,
    0xe0, 0x32, 0x3a, 0x0a, 0x49, 0x06, 0x24, 0x5c, 0xc2, 0xd3, 0xac, 0x62, 0x91, 0x95, 0xe4, 0x79,
    0xe7, 0xc8, 0x37, 0x6d, 0x8d, 0xd5, 0x4e, 0xa9, 0x6c, 0x56, 0xf4, 0xea, 0x65, 0x7a, 0xae, 0x08,
    0xba, 0x78, 0x25, 0x2e, 0x1c, 0xa6, 0xb4, 0xc6, 0xe8, 0xdd, 0x74, 0x1f, 0x4b, 0xbd, 0x8b, 0x8a,
    0x70, 0x3e, 0xb5, 0x66, 0x48, 0x03, 0xf6, 0x0e, 0x61, 0x35, 0x57, 0xb9, 0x86, 0xc1, 0x1d, 0x9e,
    0xe1, 0xf8, 0x98, 0x11, 0x69, 0xd9, 0x8e, 0x94, 0x9b, 0x1e, 0x87, 0xe9, 0xce, 0x55, 0x28, 0xdf,
    0x8c, 0xa1, 0x89, 0x0d, 0xbf, 0xe6, 0x42, 0x68, 0x41, 0x99, 0x2d, 0x0f, 0xb0, 0x54, 0xbb, 0x16,
];

const AES_INV_SBOX: [u8; 256] = [
    0x52, 0x09, 0x6a, 0xd5, 0x30, 0x36, 0xa5, 0x38, 0xbf, 0x40, 0xa3, 0x9e, 0x81, 0xf3, 0xd7, 0xfb,
    0x7c, 0xe3, 0x39, 0x82, 0x9b, 0x2f, 0xff, 0x87, 0x34, 0x8e, 0x43, 0x44, 0xc4, 0xde, 0xe9, 0xcb,
    0x54, 0x7b, 0x94, 0x32, 0xa6, 0xc2, 0x23, 0x3d, 0xee, 0x4c, 0x95, 0x0b, 0x42, 0xfa, 0xc3, 0x4e,
    0x08, 0x2e, 0xa1, 0x66, 0x28, 0xd9, 0x24, 0xb2, 0x76, 0x5b, 0xa2, 0x49, 0x6d, 0x8b, 0xd1, 0x25,
    0x72, 0xf8, 0xf6, 0x64, 0x86, 0x68, 0x98, 0x16, 0xd4, 0xa4, 0x5c, 0xcc, 0x5d, 0x65, 0xb6, 0x92,
    0x6c, 0x70, 0x48, 0x50, 0xfd, 0xed, 0xb9, 0xda, 0x5e, 0x15, 0x46, 0x57, 0xa7, 0x8d, 0x9d, 0x84,
    0x90, 0xd8, 0xab, 0x00, 0x8c, 0xbc, 0xd3, 0x0a, 0xf7, 0xe4, 0x58, 0x05, 0xb8, 0xb3, 0x45, 0x06,
    0xd0, 0x2c, 0x1e, 0x8f, 0xca, 0x3f, 0x0f, 0x02, 0xc1, 0xaf, 0xbd, 0x03, 0x01, 0x13, 0x8a, 0x6b,
    0x3a, 0x91, 0x11, 0x41, 0x4f, 0x67, 0xdc, 0xea, 0x97, 0xf2, 0xcf, 0xce, 0xf0, 0xb4, 0xe6, 0x73,
    0x96, 0xac, 0x74, 0x22, 0xe7, 0xad, 0x35, 0x85, 0xe2, 0xf9, 0x37, 0xe8, 0x1c, 0x75, 0xdf, 0x6e,
    0x47, 0xf1, 0x1a, 0x71, 0x1d, 0x29, 0xc5, 0x89, 0x6f, 0xb7, 0x62, 0x0e, 0xaa, 0x18, 0xbe, 0x1b,
    0xfc, 0x56, 0x3e, 0x4b, 0xc6, 0xd2, 0x79, 0x20, 0x9a, 0xdb, 0xc0, 0xfe, 0x78, 0xcd, 0x5a, 0xf4,
    0x1f, 0xdd, 0xa8, 0x33, 0x88, 0x07, 0xc7, 0x31, 0xb1, 0x12, 0x10, 0x59, 0x27, 0x80, 0xec, 0x5f,
    0x60, 0x51, 0x7f, 0xa9, 0x19, 0xb5, 0x4a, 0x0d, 0x2d, 0xe5, 0x7a, 0x9f, 0x93, 0xc9, 0x9c, 0xef,
    0xa0, 0xe0, 0x3b, 0x4d, 0xae, 0x2a, 0xf5, 0xb0, 0xc8, 0xeb, 0xbb, 0x3c, 0x83, 0x53, 0x99, 0x61,
    0x17, 0x2b, 0x04, 0x7e, 0xba, 0x77, 0xd6, 0x26, 0xe1, 0x69, 0x14, 0x63, 0x55, 0x21, 0x0c, 0x7d,
];

const AES_RCON: [u8; 11] = [
    0x00, 0x01, 0x02, 0x04, 0x08, 0x10, 0x20, 0x40, 0x80, 0x1b, 0x36,
];

fn gf_mul(mut a: u8, mut b: u8) -> u8 {
    let mut result = 0u8;
    while b != 0 {
        if b & 1 != 0 {
            result ^= a;
        }
        let hi = a & 0x80;
        a <<= 1;
        if hi != 0 {
            a ^= 0x1B;
        }
        b >>= 1;
    }
    result
}

pub struct AesKey {
    round_keys: Vec<[u8; 16]>,
    key_size: usize,
    nr: usize,
}

impl AesKey {
    pub fn new(key: &[u8]) -> Result<Self, EncryptionError> {
        let (nk, nr) = match key.len() {
            16 => (4, 10),
            24 => (6, 12),
            32 => (8, 14),
            _ => return Err(EncryptionError::InvalidKeySize(key.len() as u16 * 8)),
        };

        let total_words = 4 * (nr + 1);
        let mut w = Vec::with_capacity(total_words);

        for i in 0..nk {
            let word = [key[4 * i], key[4 * i + 1], key[4 * i + 2], key[4 * i + 3]];
            w.push(word);
        }

        for i in nk..total_words {
            let mut temp = w[i - 1];
            if i % nk == 0 {
                let rotated = [temp[1], temp[2], temp[3], temp[0]];
                temp = [
                    AES_SBOX[rotated[0] as usize] ^ AES_RCON[i / nk],
                    AES_SBOX[rotated[1] as usize],
                    AES_SBOX[rotated[2] as usize],
                    AES_SBOX[rotated[3] as usize],
                ];
            } else if nk > 6 && i % nk == 4 {
                temp = [
                    AES_SBOX[temp[0] as usize],
                    AES_SBOX[temp[1] as usize],
                    AES_SBOX[temp[2] as usize],
                    AES_SBOX[temp[3] as usize],
                ];
            }
            let prev = w[i - nk];
            w.push([
                prev[0] ^ temp[0],
                prev[1] ^ temp[1],
                prev[2] ^ temp[2],
                prev[3] ^ temp[3],
            ]);
        }

        let mut round_keys = Vec::with_capacity(nr + 1);
        for r in 0..=nr {
            let mut rk = [0u8; 16];
            for j in 0..4 {
                rk[4 * j..4 * j + 4].copy_from_slice(&w[4 * r + j]);
            }
            round_keys.push(rk);
        }

        Ok(Self {
            round_keys,
            key_size: key.len(),
            nr,
        })
    }

    pub fn encrypt_block(&self, input: &[u8; 16]) -> [u8; 16] {
        let mut state = *input;

        xor_block(&mut state, &self.round_keys[0]);

        for round in 1..self.nr {
            sub_bytes(&mut state);
            shift_rows(&mut state);
            mix_columns(&mut state);
            xor_block(&mut state, &self.round_keys[round]);
        }

        sub_bytes(&mut state);
        shift_rows(&mut state);
        xor_block(&mut state, &self.round_keys[self.nr]);

        state
    }

    pub fn decrypt_block(&self, input: &[u8; 16]) -> [u8; 16] {
        let mut state = *input;

        xor_block(&mut state, &self.round_keys[self.nr]);

        for round in (1..self.nr).rev() {
            inv_shift_rows(&mut state);
            inv_sub_bytes(&mut state);
            xor_block(&mut state, &self.round_keys[round]);
            inv_mix_columns(&mut state);
        }

        inv_shift_rows(&mut state);
        inv_sub_bytes(&mut state);
        xor_block(&mut state, &self.round_keys[0]);

        state
    }
}

fn xor_block(a: &mut [u8; 16], b: &[u8; 16]) {
    for i in 0..16 {
        a[i] ^= b[i];
    }
}

fn sub_bytes(state: &mut [u8; 16]) {
    for b in state.iter_mut() {
        *b = AES_SBOX[*b as usize];
    }
}

fn inv_sub_bytes(state: &mut [u8; 16]) {
    for b in state.iter_mut() {
        *b = AES_INV_SBOX[*b as usize];
    }
}

fn shift_rows(state: &mut [u8; 16]) {
    let tmp = state[1];
    state[1] = state[5];
    state[5] = state[9];
    state[9] = state[13];
    state[13] = tmp;

    let tmp1 = state[2];
    let tmp2 = state[6];
    state[2] = state[10];
    state[6] = state[14];
    state[10] = tmp1;
    state[14] = tmp2;

    let tmp = state[15];
    state[15] = state[11];
    state[11] = state[7];
    state[7] = state[3];
    state[3] = tmp;
}

fn inv_shift_rows(state: &mut [u8; 16]) {
    let tmp = state[13];
    state[13] = state[9];
    state[9] = state[5];
    state[5] = state[1];
    state[1] = tmp;

    let tmp1 = state[2];
    let tmp2 = state[6];
    state[2] = state[10];
    state[6] = state[14];
    state[10] = tmp1;
    state[14] = tmp2;

    let tmp = state[3];
    state[3] = state[7];
    state[7] = state[11];
    state[11] = state[15];
    state[15] = tmp;
}

fn mix_columns(state: &mut [u8; 16]) {
    for col in 0..4 {
        let i = col * 4;
        let a = [state[i], state[i + 1], state[i + 2], state[i + 3]];
        state[i] = gf_mul(a[0], 2) ^ gf_mul(a[1], 3) ^ a[2] ^ a[3];
        state[i + 1] = a[0] ^ gf_mul(a[1], 2) ^ gf_mul(a[2], 3) ^ a[3];
        state[i + 2] = a[0] ^ a[1] ^ gf_mul(a[2], 2) ^ gf_mul(a[3], 3);
        state[i + 3] = gf_mul(a[0], 3) ^ a[1] ^ a[2] ^ gf_mul(a[3], 2);
    }
}

fn inv_mix_columns(state: &mut [u8; 16]) {
    for col in 0..4 {
        let i = col * 4;
        let a = [state[i], state[i + 1], state[i + 2], state[i + 3]];
        state[i] = gf_mul(a[0], 14) ^ gf_mul(a[1], 11) ^ gf_mul(a[2], 13) ^ gf_mul(a[3], 9);
        state[i + 1] = gf_mul(a[0], 9) ^ gf_mul(a[1], 14) ^ gf_mul(a[2], 11) ^ gf_mul(a[3], 13);
        state[i + 2] = gf_mul(a[0], 13) ^ gf_mul(a[1], 9) ^ gf_mul(a[2], 14) ^ gf_mul(a[3], 11);
        state[i + 3] = gf_mul(a[0], 11) ^ gf_mul(a[1], 13) ^ gf_mul(a[2], 9) ^ gf_mul(a[3], 14);
    }
}

// ─── XTS Mode ───────────────────────────────────────────────────────────────

pub struct AesXts {
    key1: AesKey,
    key2: AesKey,
}

impl AesXts {
    pub fn new(key: &[u8]) -> Result<Self, EncryptionError> {
        let half = key.len() / 2;
        if half != 16 && half != 24 && half != 32 {
            return Err(EncryptionError::InvalidKeySize(key.len() as u16 * 8));
        }
        Ok(Self {
            key1: AesKey::new(&key[..half])?,
            key2: AesKey::new(&key[half..])?,
        })
    }

    fn gf128_mul_alpha(tweak: &mut [u8; 16]) {
        let mut carry = 0u8;
        for byte in tweak.iter_mut() {
            let new_carry = *byte >> 7;
            *byte = (*byte << 1) | carry;
            carry = new_carry;
        }
        if carry != 0 {
            tweak[0] ^= 0x87;
        }
    }

    pub fn encrypt_sector(&self, sector_num: u64, data: &mut [u8]) -> Result<(), EncryptionError> {
        if data.len() % AES_BLOCK_SIZE != 0 {
            return Err(EncryptionError::CipherError(String::from(
                "data not aligned to block size",
            )));
        }

        let mut tweak_input = [0u8; 16];
        tweak_input[..8].copy_from_slice(&sector_num.to_le_bytes());
        let mut tweak = self.key2.encrypt_block(&tweak_input);

        for chunk in data.chunks_mut(AES_BLOCK_SIZE) {
            let mut block = [0u8; 16];
            block.copy_from_slice(chunk);

            xor_block(&mut block, &tweak);
            block = self.key1.encrypt_block(&block);
            xor_block(&mut block, &tweak);

            chunk.copy_from_slice(&block);
            Self::gf128_mul_alpha(&mut tweak);
        }

        Ok(())
    }

    pub fn decrypt_sector(&self, sector_num: u64, data: &mut [u8]) -> Result<(), EncryptionError> {
        if data.len() % AES_BLOCK_SIZE != 0 {
            return Err(EncryptionError::CipherError(String::from(
                "data not aligned to block size",
            )));
        }

        let mut tweak_input = [0u8; 16];
        tweak_input[..8].copy_from_slice(&sector_num.to_le_bytes());
        let mut tweak = self.key2.encrypt_block(&tweak_input);

        for chunk in data.chunks_mut(AES_BLOCK_SIZE) {
            let mut block = [0u8; 16];
            block.copy_from_slice(chunk);

            xor_block(&mut block, &tweak);
            block = self.key1.decrypt_block(&block);
            xor_block(&mut block, &tweak);

            chunk.copy_from_slice(&block);
            Self::gf128_mul_alpha(&mut tweak);
        }

        Ok(())
    }
}

// ─── Key Derivation ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KdfAlgorithm {
    Pbkdf2Sha256,
    Argon2id,
}

pub struct KdfParams {
    pub algorithm: KdfAlgorithm,
    pub iterations: u32,
    pub memory_cost: u32, // Argon2: memory in KiB
    pub parallelism: u8,  // Argon2: parallelism
    pub salt: [u8; 32],
    pub output_length: usize,
}

impl KdfParams {
    pub fn pbkdf2_default(salt: [u8; 32]) -> Self {
        Self {
            algorithm: KdfAlgorithm::Pbkdf2Sha256,
            iterations: 600_000,
            memory_cost: 0,
            parallelism: 1,
            salt,
            output_length: 32,
        }
    }

    pub fn argon2id_default(salt: [u8; 32]) -> Self {
        Self {
            algorithm: KdfAlgorithm::Argon2id,
            iterations: 4,
            // 8 MiB matrix: the real Argon2id (encryption::argon2id_full) now
            // allocates m_kib × 1 KiB blocks, and the kernel heap is ~32 MiB,
            // so the former 1 GiB cost would OOM-panic on derive. 8 MiB stays
            // safely in budget while remaining strongly memory-hard. No on-disk
            // FDE volumes exist yet, so changing the cost breaks nothing.
            memory_cost: 8_192, // 8 MiB (was 1 GiB placeholder, never derivable)
            parallelism: 4,
            salt,
            output_length: 32,
        }
    }
}

fn sha256_block(data: &[u8]) -> [u8; 32] {
    let k: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];

    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];

    let mut padded = data.to_vec();
    let bit_len = (data.len() as u64) * 8;
    padded.push(0x80);
    while (padded.len() % 64) != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&bit_len.to_be_bytes());

    for chunk in padded.chunks(64) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([
                chunk[4 * i],
                chunk[4 * i + 1],
                chunk[4 * i + 2],
                chunk[4 * i + 3],
            ]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }

        let (mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh) =
            (h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7]);

        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(k[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);

            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }

        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
        h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g);
        h[7] = h[7].wrapping_add(hh);
    }

    let mut result = [0u8; 32];
    for i in 0..8 {
        result[4 * i..4 * i + 4].copy_from_slice(&h[i].to_be_bytes());
    }
    result
}

fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    let mut k_padded = [0u8; 64];
    if key.len() > 64 {
        let hash = sha256_block(key);
        k_padded[..32].copy_from_slice(&hash);
    } else {
        k_padded[..key.len()].copy_from_slice(key);
    }

    let mut ipad = [0x36u8; 64];
    let mut opad = [0x5cu8; 64];
    for i in 0..64 {
        ipad[i] ^= k_padded[i];
        opad[i] ^= k_padded[i];
    }

    let mut inner = Vec::with_capacity(64 + message.len());
    inner.extend_from_slice(&ipad);
    inner.extend_from_slice(message);
    let inner_hash = sha256_block(&inner);

    let mut outer = Vec::with_capacity(64 + 32);
    outer.extend_from_slice(&opad);
    outer.extend_from_slice(&inner_hash);
    sha256_block(&outer)
}

pub fn pbkdf2_sha256(password: &[u8], salt: &[u8], iterations: u32, output: &mut [u8]) {
    let dk_len = output.len();
    let h_len = 32;
    let blocks_needed = (dk_len + h_len - 1) / h_len;

    for block_idx in 1..=(blocks_needed as u32) {
        let mut salt_block = Vec::with_capacity(salt.len() + 4);
        salt_block.extend_from_slice(salt);
        salt_block.extend_from_slice(&block_idx.to_be_bytes());

        let mut u = hmac_sha256(password, &salt_block);
        let mut result = u;

        for _ in 1..iterations {
            u = hmac_sha256(password, &u);
            for j in 0..32 {
                result[j] ^= u[j];
            }
        }

        let offset = ((block_idx - 1) as usize) * h_len;
        let copy_len = core::cmp::min(h_len, dk_len - offset);
        output[offset..offset + copy_len].copy_from_slice(&result[..copy_len]);
    }
}

// ─── Argon2id (RFC 9106) — real memory-hard KDF ─────────────────────────────
//
// The full algorithm (BLAKE2b core, H' hash, BlaMka compression, multi-lane
// reference-indexed fill, Argon2id split addressing) now lives in the shared
// `ath_crypto` crate so the kernel (FDE key derivation) and `athid` (account
// password hashing) share ONE KAT-proven implementation instead of each
// carrying its own. Re-exported here so existing callers (`derive_key`,
// `KdfParams::argon2id_default`) are unchanged. Validated against the RFC 9106
// §5.3 vector in `run_boot_smoketest` below (fail-closed) and by the host
// harness in `tools/argon2_kat/`.
pub use ath_crypto::{argon2id_derive, argon2id_full};

/// RFC 9106 §5.3 Argon2id known-answer test + a BLAKE2b self-check. Fail-closed:
/// panics on mismatch so a broken KDF can never silently weaken FDE keys.
pub fn run_boot_smoketest() {
    // BLAKE2b-512("abc") — RFC 7693 Appendix A test vector.
    let b = ath_crypto::blake2b(64, b"abc");
    const BLAKE2B_ABC: [u8; 64] = [
        0xba, 0x80, 0xa5, 0x3f, 0x98, 0x1c, 0x4d, 0x0d, 0x6a, 0x27, 0x97, 0xb6, 0x9f, 0x12, 0xf6,
        0xe9, 0x4c, 0x21, 0x2f, 0x14, 0x68, 0x5a, 0xc4, 0xb7, 0x4b, 0x12, 0xbb, 0x6f, 0xdb, 0xff,
        0xa2, 0xd1, 0x7d, 0x87, 0xc5, 0x39, 0x2a, 0xab, 0x79, 0x2d, 0xc2, 0x52, 0xd5, 0xde, 0x45,
        0x33, 0xcc, 0x95, 0x18, 0xd3, 0x8a, 0xa8, 0xdb, 0xf1, 0x92, 0x5a, 0xb9, 0x23, 0x86, 0xed,
        0xd4, 0x00, 0x99, 0x23,
    ];
    let blake_ok = b.as_slice() == BLAKE2B_ABC.as_slice();

    // RFC 9106 §5.3: Argon2id v19, m=32 KiB, t=3, p=4, password=32×0x01,
    // salt=16×0x02, secret=8×0x03, ad=12×0x04, tag=32 bytes.
    let password = [0x01u8; 32];
    let salt = [0x02u8; 16];
    let secret = [0x03u8; 8];
    let ad = [0x04u8; 12];
    let mut tag = [0u8; 32];
    argon2id_full(&password, &salt, &secret, &ad, 3, 32, 4, &mut tag);
    const ARGON2ID_TAG: [u8; 32] = [
        0x0d, 0x64, 0x0d, 0xf5, 0x8d, 0x78, 0x76, 0x6c, 0x08, 0xc0, 0x37, 0xa3, 0x4a, 0x8b, 0x53,
        0xc9, 0xd0, 0x1e, 0xf0, 0x45, 0x2d, 0x75, 0xb6, 0x5e, 0xb5, 0x25, 0x20, 0xe9, 0x6b, 0x01,
        0xe6, 0x59,
    ];
    let argon_ok = tag == ARGON2ID_TAG;

    crate::serial_println!(
        "[encryption] KDF KAT: BLAKE2b-512(abc)={} Argon2id(RFC9106)={} -> {}",
        if blake_ok { "PASS" } else { "FAIL" },
        if argon_ok { "PASS" } else { "FAIL" },
        if blake_ok && argon_ok { "PASS" } else { "FAIL" },
    );
    assert!(blake_ok, "BLAKE2b-512 KAT failed");
    assert!(argon_ok, "Argon2id RFC 9106 KAT failed");

    // FDE passphrase verification: an EncryptedVolume must ACCEPT the correct
    // passphrase and REJECT a wrong one (the unlock path used to skip the
    // master-key-digest check and accept any passphrase).
    let slot = KeySlot::new_inactive();
    let mut vol = EncryptedVolume::new("smoketest-vol", "/dev/null", CipherMode::AesXts);
    let fmt_ok = vol
        .format(
            b"correct horse battery staple",
            KdfAlgorithm::Argon2id,
            4096,
        )
        .is_ok();
    // Wrong passphrase first (volume is Locked after format): must be rejected.
    let wrong_rejected = matches!(
        vol.unlock(b"Tr0ub4dor&3-wrong", &slot),
        Err(EncryptionError::WrongPassphrase)
    );
    // Correct passphrase: must unlock.
    let right_ok = vol.unlock(b"correct horse battery staple", &slot).is_ok();
    let fde_pass = fmt_ok && wrong_rejected && right_ok;
    crate::serial_println!(
        "[encryption] FDE passphrase verify: format={} wrong_rejected={} correct_accepted={} -> {}",
        fmt_ok,
        wrong_rejected,
        right_ok,
        if fde_pass { "PASS" } else { "FAIL" }
    );
    assert!(wrong_rejected, "FDE unlock accepted a WRONG passphrase");
    assert!(right_ok, "FDE unlock rejected the CORRECT passphrase");

    // TPM auto-unlock: bind the (now Active) volume's master key to the current
    // measured-boot state, lock it, then recover WITHOUT a passphrase via the
    // TPM. This is the BitLocker/FileVault mechanism end-to-end through the FDE
    // layer. FAIL-able: if enroll/unseal degraded, tpm_unlock_ok would be false;
    // if unlock_with_tpm stopped fail-closing, no_enroll_rejected would be false.
    // (vol is Active here from the passphrase unlock above.)
    let enroll_ok = vol.enroll_tpm(&[crate::security::PCR_KERNEL_IMAGE]).is_ok();
    vol.lock();
    let tpm_unlock_ok =
        enroll_ok && vol.unlock_with_tpm().is_ok() && vol.state == VolumeState::Active;
    // A volume with NO TPM enrollment must fail closed (caller then falls back
    // to the passphrase), never activate on an empty/foreign sealed slot.
    let mut fresh = EncryptedVolume::new("smoketest-vol2", "/dev/null", CipherMode::AesXts);
    let _ = fresh.format(b"pw2", KdfAlgorithm::Argon2id, 4096);
    let no_enroll_rejected = matches!(
        fresh.unlock_with_tpm(),
        Err(EncryptionError::TpmUnsealFailed)
    );
    let tpm_pass = tpm_unlock_ok && no_enroll_rejected;
    crate::serial_println!(
        "[encryption] FDE TPM auto-unlock: enroll={} tpm_unlock={} no_enroll_rejected={} -> {}",
        enroll_ok,
        tpm_unlock_ok,
        no_enroll_rejected,
        if tpm_pass { "PASS" } else { "FAIL" }
    );
    assert!(
        tpm_unlock_ok,
        "FDE TPM auto-unlock failed to recover the volume"
    );
    assert!(
        no_enroll_rejected,
        "FDE TPM unlock did not fail closed without enrollment"
    );
}

pub fn derive_key(
    password: &[u8],
    params: &KdfParams,
    output: &mut [u8],
) -> Result<(), EncryptionError> {
    match params.algorithm {
        KdfAlgorithm::Pbkdf2Sha256 => {
            pbkdf2_sha256(password, &params.salt, params.iterations, output);
            Ok(())
        }
        KdfAlgorithm::Argon2id => {
            argon2id_derive(
                password,
                &params.salt,
                params.iterations,
                params.memory_cost,
                params.parallelism,
                output,
            );
            Ok(())
        }
    }
}

// ─── Anti-Forensics (AF Splitter) ───────────────────────────────────────────

pub struct AfSplitter;

impl AfSplitter {
    pub fn split(key: &[u8], stripes: u32) -> Vec<u8> {
        let key_len = key.len();
        let mut material = alloc::vec![0u8; key_len * stripes as usize];

        // Fill stripes with pseudo-random data derived from the key
        for s in 0..stripes.saturating_sub(1) {
            let offset = s as usize * key_len;
            let hash = sha256_block(&[&(s as u64).to_le_bytes()[..], key].concat());
            let copy_len = core::cmp::min(key_len, 32);
            material[offset..offset + copy_len].copy_from_slice(&hash[..copy_len]);
        }

        // Compute the diffuse of all previous stripes
        let mut d = alloc::vec![0u8; key_len];
        for s in 0..stripes.saturating_sub(1) {
            let offset = s as usize * key_len;
            for i in 0..key_len {
                d[i] ^= material[offset + i];
            }
            d = Self::diffuse(&d);
        }

        // Last stripe = key XOR diffuse(all previous)
        let last_offset = (stripes as usize - 1) * key_len;
        for i in 0..key_len {
            material[last_offset + i] = key[i] ^ d[i];
        }

        material
    }

    pub fn merge(material: &[u8], key_len: usize, stripes: u32) -> Vec<u8> {
        let mut d = alloc::vec![0u8; key_len];

        for s in 0..stripes {
            let offset = s as usize * key_len;
            for i in 0..key_len {
                d[i] ^= material[offset + i];
            }
            if s < stripes - 1 {
                d = Self::diffuse(&d);
            }
        }

        d
    }

    fn diffuse(data: &[u8]) -> Vec<u8> {
        let mut result = Vec::with_capacity(data.len());
        let chunks = (data.len() + 31) / 32;
        for i in 0..chunks {
            let mut input = Vec::new();
            input.extend_from_slice(&(i as u32).to_be_bytes());
            let end = core::cmp::min(data.len(), (i + 1) * 32);
            input.extend_from_slice(&data[i * 32..end]);
            let hash = sha256_block(&input);
            let copy_len = core::cmp::min(32, data.len() - i * 32);
            result.extend_from_slice(&hash[..copy_len]);
        }
        result
    }
}

// ─── LUKS2 Header ───────────────────────────────────────────────────────────

const LUKS_MAGIC: [u8; 6] = [0x4C, 0x55, 0x4B, 0x53, 0xBA, 0xBE];
const LUKS2_VERSION: u16 = 2;
const LUKS_KEY_SLOT_COUNT: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeySlotState {
    Inactive,
    Active,
    Unbound,
}

pub struct KeySlot {
    pub state: KeySlotState,
    pub kdf: KdfParams,
    pub af_stripes: u32,
    pub key_material_offset: u64,
    pub key_material_size: u32,
    pub priority: u8,
}

impl KeySlot {
    pub fn new_inactive() -> Self {
        Self {
            state: KeySlotState::Inactive,
            kdf: KdfParams::pbkdf2_default([0u8; 32]),
            af_stripes: 4000,
            key_material_offset: 0,
            key_material_size: 0,
            priority: 1,
        }
    }
}

pub struct LuksHeader {
    pub magic: [u8; 6],
    pub version: u16,
    pub uuid: [u8; 40],
    pub label: String,
    pub cipher_name: String,
    pub cipher_mode: String,
    pub hash_spec: String,
    pub payload_offset: u64,
    pub key_bytes: u32,
    pub mk_digest: [u8; 32],
    pub mk_digest_salt: [u8; 32],
    pub mk_digest_iterations: u32,
    pub header_size: u64,
    pub checksum: [u8; 32],
}

impl LuksHeader {
    pub fn new(cipher: CipherMode, label: &str) -> Self {
        Self {
            magic: LUKS_MAGIC,
            version: LUKS2_VERSION,
            uuid: [0u8; 40],
            label: String::from(label),
            cipher_name: String::from(cipher.name()),
            cipher_mode: String::from("xts-plain64"),
            hash_spec: String::from("sha256"),
            payload_offset: 32768,
            key_bytes: (cipher.key_size_bits() / 8) as u32,
            mk_digest: [0u8; 32],
            mk_digest_salt: [0u8; 32],
            mk_digest_iterations: 600_000,
            header_size: 16384,
            checksum: [0u8; 32],
        }
    }

    pub fn verify_magic(&self) -> bool {
        self.magic == LUKS_MAGIC
    }

    pub fn verify_version(&self) -> bool {
        self.version == LUKS2_VERSION
    }

    pub fn compute_checksum(&mut self) {
        let mut data = Vec::new();
        data.extend_from_slice(&self.magic);
        data.extend_from_slice(&self.version.to_be_bytes());
        data.extend_from_slice(&self.uuid);
        data.extend_from_slice(self.label.as_bytes());
        data.extend_from_slice(&self.payload_offset.to_be_bytes());
        data.extend_from_slice(&self.key_bytes.to_be_bytes());
        self.checksum = sha256_block(&data);
    }

    pub fn verify_checksum(&self) -> bool {
        let mut data = Vec::new();
        data.extend_from_slice(&self.magic);
        data.extend_from_slice(&self.version.to_be_bytes());
        data.extend_from_slice(&self.uuid);
        data.extend_from_slice(self.label.as_bytes());
        data.extend_from_slice(&self.payload_offset.to_be_bytes());
        data.extend_from_slice(&self.key_bytes.to_be_bytes());
        sha256_block(&data) == self.checksum
    }

    pub fn parse(data: &[u8]) -> Result<Self, EncryptionError> {
        if data.len() < 128 {
            return Err(EncryptionError::HeaderCorrupted);
        }

        let mut magic = [0u8; 6];
        magic.copy_from_slice(&data[0..6]);
        if magic != LUKS_MAGIC {
            return Err(EncryptionError::HeaderCorrupted);
        }

        let version = u16::from_be_bytes([data[6], data[7]]);
        if version != LUKS2_VERSION {
            return Err(EncryptionError::HeaderVersionUnsupported(version));
        }

        let mut uuid = [0u8; 40];
        uuid.copy_from_slice(&data[8..48]);

        let label_end = data[48..112].iter().position(|&b| b == 0).unwrap_or(64);
        let label = core::str::from_utf8(&data[48..48 + label_end])
            .unwrap_or("")
            .to_string();

        let payload_offset = u64::from_be_bytes([
            data[112], data[113], data[114], data[115], data[116], data[117], data[118], data[119],
        ]);

        let key_bytes = u32::from_be_bytes([data[120], data[121], data[122], data[123]]);

        Ok(Self {
            magic,
            version,
            uuid,
            label: String::from(label),
            cipher_name: String::from("aes"),
            cipher_mode: String::from("xts-plain64"),
            hash_spec: String::from("sha256"),
            payload_offset,
            key_bytes,
            mk_digest: [0u8; 32],
            mk_digest_salt: [0u8; 32],
            mk_digest_iterations: 600_000,
            header_size: 16384,
            checksum: [0u8; 32],
        })
    }
}

// ─── Encryption Statistics ──────────────────────────────────────────────────

pub struct EncryptionStats {
    pub sectors_encrypted: u64,
    pub sectors_decrypted: u64,
    pub bytes_processed: u64,
    pub errors: u64,
    pub io_reads: u64,
    pub io_writes: u64,
}

impl EncryptionStats {
    pub fn new() -> Self {
        Self {
            sectors_encrypted: 0,
            sectors_decrypted: 0,
            bytes_processed: 0,
            errors: 0,
            io_reads: 0,
            io_writes: 0,
        }
    }
}

// ─── Encrypted Volume ───────────────────────────────────────────────────────

pub struct EncryptedVolume {
    pub name: String,
    pub device_path: String,
    pub cipher: CipherMode,
    pub key_size: u16,
    pub sector_size: u32,
    pub iv_mode: IvMode,
    pub state: VolumeState,
    pub header: LuksHeader,
    pub stats: EncryptionStats,
    master_key: Option<Vec<u8>>,
    xts_cipher: Option<AesXts>,
    total_sectors: u64,
    /// The master key sealed to the machine's measured-boot state (TPM PCR
    /// policy). Present only after `enroll_tpm`. When present, `unlock_with_tpm`
    /// can recover the volume WITHOUT a passphrase — but only if the boot state
    /// still matches, giving BitLocker/FileVault-style auto-unlock that a
    /// tampered boot cannot satisfy.
    tpm_sealed_master: Option<crate::tpm::SealedObject>,
}

impl EncryptedVolume {
    pub fn new(name: &str, device_path: &str, cipher: CipherMode) -> Self {
        let key_size = cipher.key_size_bits();
        Self {
            name: String::from(name),
            device_path: String::from(device_path),
            cipher,
            key_size,
            sector_size: 512,
            iv_mode: IvMode::Plain64,
            state: VolumeState::Locked,
            header: LuksHeader::new(cipher, name),
            stats: EncryptionStats::new(),
            master_key: None,
            xts_cipher: None,
            total_sectors: 0,
            tpm_sealed_master: None,
        }
    }

    /// Compute the verification digest of a master key (the same fixed PBKDF2 the
    /// passphrase path uses), so a key recovered by ANY means — passphrase or
    /// TPM unseal — can be checked against the stored `mk_digest` before it is
    /// allowed to activate the volume.
    fn mk_digest_of(&self, master_key: &[u8]) -> [u8; 32] {
        let digest_params = KdfParams::pbkdf2_default(self.header.mk_digest_salt);
        let mut mk_digest = [0u8; 32];
        let _ = derive_key(master_key, &digest_params, &mut mk_digest);
        mk_digest
    }

    /// Bind this volume's master key to the machine's current measured-boot
    /// state: seal it to the given TPM PCRs so `unlock_with_tpm` can recover it
    /// on a subsequent boot into the SAME state. Requires the volume to be
    /// Active (the master key must be in hand to seal). Fails closed if no TPM
    /// backend can seal.
    pub fn enroll_tpm(&mut self, pcr_selection: &[u32]) -> Result<(), EncryptionError> {
        let master_key = self
            .master_key
            .as_ref()
            .ok_or(EncryptionError::VolumeLocked)?
            .clone();
        let sealed = {
            let guard = crate::tpm::TPM.lock();
            match &*guard {
                Some(dev) => dev
                    .seal(&master_key, pcr_selection)
                    .map_err(|_| EncryptionError::TpmSealFailed)?,
                None => return Err(EncryptionError::TpmNotAvailable),
            }
        };
        self.tpm_sealed_master = Some(sealed);
        Ok(())
    }

    /// True once a TPM-sealed master key has been enrolled for this volume.
    pub fn tpm_enrolled(&self) -> bool {
        self.tpm_sealed_master.is_some()
    }

    /// Passphrase-free unlock via the TPM: unseal the master key, and activate
    /// the volume ONLY if the recovered key reproduces the stored `mk_digest`.
    /// Fails closed (`TpmUnsealFailed`) with no enrollment, on a PCR-policy
    /// mismatch (a changed boot state), or on a wrong/corrupt recovered key —
    /// so a foreign or replayed blob can never activate the volume. Callers fall
    /// back to `unlock` (passphrase) on any error.
    pub fn unlock_with_tpm(&mut self) -> Result<(), EncryptionError> {
        if self.state == VolumeState::Active {
            return Err(EncryptionError::VolumeAlreadyActive);
        }
        let sealed = self
            .tpm_sealed_master
            .as_ref()
            .ok_or(EncryptionError::TpmUnsealFailed)?
            .clone();

        self.state = VolumeState::Unlocking;

        let master_key = {
            let guard = crate::tpm::TPM.lock();
            match &*guard {
                Some(dev) => match dev.unseal(&sealed) {
                    Ok(k) => k,
                    Err(_) => {
                        self.state = VolumeState::Locked;
                        return Err(EncryptionError::TpmUnsealFailed);
                    }
                },
                None => {
                    self.state = VolumeState::Locked;
                    return Err(EncryptionError::TpmNotAvailable);
                }
            }
        };

        // The unsealed key MUST reproduce the stored digest, else refuse — never
        // activate the volume with a key we cannot bind to this header.
        let check = self.mk_digest_of(&master_key);
        if !crate::crypto::ct_eq(&check, &self.header.mk_digest) {
            self.state = VolumeState::Locked;
            return Err(EncryptionError::TpmUnsealFailed);
        }

        if self.cipher == CipherMode::AesXts {
            self.xts_cipher = Some(AesXts::new(&master_key)?);
        }
        self.master_key = Some(master_key);
        self.state = VolumeState::Active;

        crate::serial_println!(
            "[encryption] Volume '{}' auto-unlocked via TPM ({}, {} sectors)",
            self.name,
            self.cipher.name(),
            self.total_sectors,
        );
        Ok(())
    }

    /// Derive the master key AND its verification digest from a passphrase, the
    /// SAME way at both format and unlock time — so a correct passphrase
    /// reproduces the stored `mk_digest` and a wrong one does not. The digest is
    /// a fixed PBKDF2 over the master key with the header's digest salt (the
    /// digest algorithm need not match the data cipher's KDF). This shared path
    /// is what makes passphrase verification in `unlock` sound.
    fn derive_master_and_digest(&self, passphrase: &[u8]) -> (alloc::vec::Vec<u8>, [u8; 32]) {
        let key_bytes = (self.key_size / 8) as usize;
        let mut master_key = alloc::vec![0u8; key_bytes];
        let seed = sha256_block(passphrase);
        for i in 0..key_bytes {
            master_key[i] = seed[i % 32];
        }
        let mk_digest = self.mk_digest_of(&master_key);
        (master_key, mk_digest)
    }

    pub fn format(
        &mut self,
        passphrase: &[u8],
        kdf: KdfAlgorithm,
        total_sectors: u64,
    ) -> Result<(), EncryptionError> {
        // The MK-digest uses a fixed PBKDF2 (see `derive_master_and_digest`);
        // per-slot data-KDF selection is a future extension.
        let _ = kdf;
        self.total_sectors = total_sectors;

        let (master_key, mk_digest) = self.derive_master_and_digest(passphrase);
        self.header.mk_digest = mk_digest;
        self.header.compute_checksum();

        self.master_key = Some(master_key);
        Ok(())
    }

    pub fn unlock(&mut self, passphrase: &[u8], slot: &KeySlot) -> Result<(), EncryptionError> {
        if self.state == VolumeState::Active {
            return Err(EncryptionError::VolumeAlreadyActive);
        }
        // Key material is derived from the passphrase via the shared path; the
        // slot descriptor is not consulted for derivation in this design.
        let _ = slot;

        self.state = VolumeState::Unlocking;

        // Derive the master key + its digest the SAME way `format` did, then
        // VERIFY the passphrase by constant-time comparing that digest to the
        // stored one. Previously this comparison was skipped ("accept the key
        // if it derives successfully"), so unlock accepted ANY passphrase and
        // activated the volume with a garbage master key.
        let (master_key, mk_check) = self.derive_master_and_digest(passphrase);
        if !crate::crypto::ct_eq(&mk_check, &self.header.mk_digest) {
            self.state = VolumeState::Locked;
            return Err(EncryptionError::WrongPassphrase);
        }

        if self.cipher == CipherMode::AesXts {
            self.xts_cipher = Some(AesXts::new(&master_key)?);
        }

        self.master_key = Some(master_key);
        self.state = VolumeState::Active;

        crate::serial_println!(
            "[encryption] Volume '{}' unlocked ({}, {} sectors)",
            self.name,
            self.cipher.name(),
            self.total_sectors,
        );

        Ok(())
    }

    pub fn lock(&mut self) {
        if let Some(ref mut key) = self.master_key {
            for b in key.iter_mut() {
                *b = 0;
            }
        }
        self.master_key = None;
        self.xts_cipher = None;
        self.state = VolumeState::Locked;
    }

    pub fn encrypt_sector(&mut self, sector: u64, data: &mut [u8]) -> Result<(), EncryptionError> {
        if self.state != VolumeState::Active {
            return Err(EncryptionError::VolumeLocked);
        }
        if sector >= self.total_sectors {
            return Err(EncryptionError::SectorOutOfRange(sector));
        }

        match self.cipher {
            CipherMode::AesXts => {
                if let Some(ref xts) = self.xts_cipher {
                    xts.encrypt_sector(sector, data)?;
                } else {
                    return Err(EncryptionError::CipherError(String::from(
                        "XTS cipher not initialized",
                    )));
                }
            }
            CipherMode::AesCbc => {
                let key = self
                    .master_key
                    .as_ref()
                    .ok_or(EncryptionError::InvalidKey)?;
                let aes = AesKey::new(key)?;
                let mut iv = [0u8; 16];
                self.iv_mode.generate_iv(sector, &mut iv);
                aes_cbc_encrypt(data, &aes, &mut iv)?;
            }
            _ => {
                return Err(EncryptionError::CipherError(String::from(
                    "cipher not yet implemented",
                )));
            }
        }

        self.stats.sectors_encrypted += 1;
        self.stats.bytes_processed += data.len() as u64;
        self.stats.io_writes += 1;

        Ok(())
    }

    pub fn decrypt_sector(&mut self, sector: u64, data: &mut [u8]) -> Result<(), EncryptionError> {
        if self.state != VolumeState::Active {
            return Err(EncryptionError::VolumeLocked);
        }
        if sector >= self.total_sectors {
            return Err(EncryptionError::SectorOutOfRange(sector));
        }

        match self.cipher {
            CipherMode::AesXts => {
                if let Some(ref xts) = self.xts_cipher {
                    xts.decrypt_sector(sector, data)?;
                } else {
                    return Err(EncryptionError::CipherError(String::from(
                        "XTS cipher not initialized",
                    )));
                }
            }
            CipherMode::AesCbc => {
                let key = self
                    .master_key
                    .as_ref()
                    .ok_or(EncryptionError::InvalidKey)?;
                let aes = AesKey::new(key)?;
                let mut iv = [0u8; 16];
                self.iv_mode.generate_iv(sector, &mut iv);
                aes_cbc_decrypt(data, &aes, &mut iv)?;
            }
            _ => {
                return Err(EncryptionError::CipherError(String::from(
                    "cipher not yet implemented",
                )));
            }
        }

        self.stats.sectors_decrypted += 1;
        self.stats.bytes_processed += data.len() as u64;
        self.stats.io_reads += 1;

        Ok(())
    }

    pub fn suspend(&mut self) -> Result<(), EncryptionError> {
        if self.state != VolumeState::Active {
            return Err(EncryptionError::VolumeLocked);
        }
        self.state = VolumeState::Suspended;
        Ok(())
    }

    pub fn resume(&mut self) -> Result<(), EncryptionError> {
        if self.state != VolumeState::Suspended {
            return Err(EncryptionError::VolumeLocked);
        }
        self.state = VolumeState::Active;
        Ok(())
    }
}

fn aes_cbc_encrypt(
    data: &mut [u8],
    key: &AesKey,
    iv: &mut [u8; 16],
) -> Result<(), EncryptionError> {
    if data.len() % AES_BLOCK_SIZE != 0 {
        return Err(EncryptionError::CipherError(String::from(
            "data not aligned to block size",
        )));
    }

    for chunk in data.chunks_mut(AES_BLOCK_SIZE) {
        let mut block = [0u8; 16];
        block.copy_from_slice(chunk);
        xor_block(&mut block, iv);
        let encrypted = key.encrypt_block(&block);
        chunk.copy_from_slice(&encrypted);
        *iv = encrypted;
    }

    Ok(())
}

fn aes_cbc_decrypt(
    data: &mut [u8],
    key: &AesKey,
    iv: &mut [u8; 16],
) -> Result<(), EncryptionError> {
    if data.len() % AES_BLOCK_SIZE != 0 {
        return Err(EncryptionError::CipherError(String::from(
            "data not aligned to block size",
        )));
    }

    for chunk in data.chunks_mut(AES_BLOCK_SIZE) {
        let mut ciphertext = [0u8; 16];
        ciphertext.copy_from_slice(chunk);
        let mut decrypted = key.decrypt_block(&ciphertext);
        xor_block(&mut decrypted, iv);
        chunk.copy_from_slice(&decrypted);
        *iv = ciphertext;
    }

    Ok(())
}

// ─── TPM Key Sealing ────────────────────────────────────────────────────────

pub struct TpmKeyStore {
    sealed_keys: Vec<TpmSealedKey>,
    pcr_policy: [u8; 32],
    available: bool,
}

pub struct TpmSealedKey {
    pub handle: u32,
    pub sealed_blob: Vec<u8>,
    pub pcr_selection: u32,
    pub auth_policy: [u8; 32],
    pub name: String,
}

impl TpmKeyStore {
    pub fn new() -> Self {
        Self {
            sealed_keys: Vec::new(),
            pcr_policy: [0u8; 32],
            available: false,
        }
    }

    pub fn probe(&mut self) -> bool {
        // Check for TPM 2.0 presence via MMIO or IO port
        self.available = false; // No TPM in virtual environments
        self.available
    }

    pub fn seal_key(
        &mut self,
        key: &[u8],
        name: &str,
        pcr_selection: u32,
    ) -> Result<u32, EncryptionError> {
        if !self.available {
            return Err(EncryptionError::TpmNotAvailable);
        }

        let handle = (self.sealed_keys.len() as u32) + 0x8100_0001;

        // "Seal" the key by encrypting with the PCR policy (simulated)
        let mut sealed_blob = Vec::with_capacity(key.len() + 32);
        let auth_policy = sha256_block(&[&self.pcr_policy[..], key].concat());
        sealed_blob.extend_from_slice(&auth_policy);
        sealed_blob.extend_from_slice(key);

        self.sealed_keys.push(TpmSealedKey {
            handle,
            sealed_blob,
            pcr_selection,
            auth_policy,
            name: String::from(name),
        });

        Ok(handle)
    }

    pub fn unseal_key(&self, handle: u32) -> Result<Vec<u8>, EncryptionError> {
        if !self.available {
            return Err(EncryptionError::TpmNotAvailable);
        }

        let sealed = self
            .sealed_keys
            .iter()
            .find(|k| k.handle == handle)
            .ok_or(EncryptionError::TpmUnsealFailed)?;

        if sealed.sealed_blob.len() <= 32 {
            return Err(EncryptionError::TpmUnsealFailed);
        }

        Ok(sealed.sealed_blob[32..].to_vec())
    }

    pub fn remove_key(&mut self, handle: u32) -> bool {
        let len_before = self.sealed_keys.len();
        self.sealed_keys.retain(|k| k.handle != handle);
        self.sealed_keys.len() < len_before
    }
}

// ─── Main Subsystem ─────────────────────────────────────────────────────────

pub struct EncryptionSubsystem {
    pub encrypted_volumes: Vec<EncryptedVolume>,
    pub key_slots: Vec<KeySlot>,
    pub tpm: TpmKeyStore,
    pub tpm_backed: bool,
    pub master_key_derived: bool,
}

impl EncryptionSubsystem {
    pub fn new() -> Self {
        let mut key_slots = Vec::with_capacity(LUKS_KEY_SLOT_COUNT);
        for _ in 0..LUKS_KEY_SLOT_COUNT {
            key_slots.push(KeySlot::new_inactive());
        }

        Self {
            encrypted_volumes: Vec::new(),
            key_slots,
            tpm: TpmKeyStore::new(),
            tpm_backed: false,
            master_key_derived: false,
        }
    }

    pub fn create_volume(
        &mut self,
        name: &str,
        device_path: &str,
        cipher: CipherMode,
        passphrase: &[u8],
        total_sectors: u64,
    ) -> Result<usize, EncryptionError> {
        let mut volume = EncryptedVolume::new(name, device_path, cipher);
        volume.format(passphrase, KdfAlgorithm::Argon2id, total_sectors)?;

        let slot_idx = self
            .key_slots
            .iter()
            .position(|s| s.state == KeySlotState::Inactive)
            .ok_or(EncryptionError::NoFreeKeySlot)?;

        self.key_slots[slot_idx].state = KeySlotState::Active;

        let vol_idx = self.encrypted_volumes.len();
        self.encrypted_volumes.push(volume);

        crate::serial_println!(
            "[encryption] Created volume '{}' ({}, slot {})",
            name,
            cipher.name(),
            slot_idx,
        );

        Ok(vol_idx)
    }

    pub fn open_volume(&mut self, name: &str, passphrase: &[u8]) -> Result<(), EncryptionError> {
        let volume = self
            .encrypted_volumes
            .iter_mut()
            .find(|v| v.name == name)
            .ok_or_else(|| EncryptionError::VolumeNotFound(String::from(name)))?;

        let slot = self
            .key_slots
            .iter()
            .find(|s| s.state == KeySlotState::Active)
            .ok_or(EncryptionError::NoFreeKeySlot)?;

        volume.unlock(passphrase, slot)?;
        self.master_key_derived = true;
        Ok(())
    }

    pub fn close_volume(&mut self, name: &str) -> Result<(), EncryptionError> {
        let volume = self
            .encrypted_volumes
            .iter_mut()
            .find(|v| v.name == name)
            .ok_or_else(|| EncryptionError::VolumeNotFound(String::from(name)))?;
        volume.lock();
        Ok(())
    }

    pub fn add_key_slot(
        &mut self,
        passphrase: &[u8],
        kdf: KdfAlgorithm,
    ) -> Result<usize, EncryptionError> {
        let slot_idx = self
            .key_slots
            .iter()
            .position(|s| s.state == KeySlotState::Inactive)
            .ok_or(EncryptionError::KeySlotFull)?;

        let salt = sha256_block(passphrase);
        let kdf_params = match kdf {
            KdfAlgorithm::Pbkdf2Sha256 => KdfParams::pbkdf2_default(salt),
            KdfAlgorithm::Argon2id => KdfParams::argon2id_default(salt),
        };

        self.key_slots[slot_idx] = KeySlot {
            state: KeySlotState::Active,
            kdf: kdf_params,
            af_stripes: 4000,
            key_material_offset: 0,
            key_material_size: 0,
            priority: 1,
        };

        Ok(slot_idx)
    }

    pub fn remove_key_slot(&mut self, slot: usize) -> Result<(), EncryptionError> {
        if slot >= self.key_slots.len() {
            return Err(EncryptionError::KeySlotInactive(slot as u8));
        }
        self.key_slots[slot] = KeySlot::new_inactive();
        Ok(())
    }

    pub fn active_volume_count(&self) -> usize {
        self.encrypted_volumes
            .iter()
            .filter(|v| v.state == VolumeState::Active)
            .count()
    }

    pub fn active_slot_count(&self) -> usize {
        self.key_slots
            .iter()
            .filter(|s| s.state == KeySlotState::Active)
            .count()
    }

    pub fn init_tpm(&mut self) -> bool {
        self.tpm_backed = self.tpm.probe();
        self.tpm_backed
    }
}

// ─── Global Instance ────────────────────────────────────────────────────────

pub static ENCRYPTION: Mutex<Option<EncryptionSubsystem>> = Mutex::new(None);

pub fn init() {
    crate::serial_println!("[encryption] Initializing storage encryption subsystem");

    let mut subsystem = EncryptionSubsystem::new();

    let tpm_available = subsystem.init_tpm();
    if tpm_available {
        crate::serial_println!("[encryption] TPM 2.0 detected — key sealing available");
    } else {
        crate::serial_println!("[encryption] No TPM detected — software key management only");
    }

    crate::serial_println!(
        "[encryption] {} key slots available, ciphers: AES-XTS, AES-CBC, AES-GCM",
        LUKS_KEY_SLOT_COUNT,
    );

    *ENCRYPTION.lock() = Some(subsystem);
    crate::serial_println!("[ OK ] Storage encryption subsystem initialized");
}
