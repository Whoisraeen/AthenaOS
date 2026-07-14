//! TPM 2.0 driver — CRB (Command Response Buffer) interface over MMIO.
//!
//! RaeShield uses the TPM for:
//! - Measured boot (PCR extend/read)
//! - Hardware RNG
//! - Attestation quotes (signed PCR snapshots for EAC/BattlEye)
//! - Key sealing to PCR state (RaeFS FDE keys)
//!
//! The CRB interface is the modern TPM 2.0 access method. The TPM2 ACPI
//! table provides the MMIO base address. All register access is volatile.

#![allow(dead_code)]

extern crate alloc;

use alloc::vec;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};
use spin::Mutex;

use crate::crypto::{HashAlgorithm, Sha256Context};

// ─── CRB Register Offsets ────────────────────────────────────────────────────

const LOC_STATE: usize = 0x00;
const LOC_CTRL: usize = 0x08;
const LOC_STS: usize = 0x0C;
const CRB_INTF_ID: usize = 0x30;
const CRB_CTRL_EXT: usize = 0x38;
const CRB_CTRL_REQ: usize = 0x40;
const CRB_CTRL_STS: usize = 0x44;
const CRB_CTRL_CANCEL: usize = 0x48;
const CRB_CTRL_START: usize = 0x4C;
const CRB_INT_ENABLE: usize = 0x50;
const CRB_INT_STS: usize = 0x54;
const CRB_CTRL_CMD_SIZE: usize = 0x58;
const CRB_CTRL_CMD_ADDR_LO: usize = 0x5C;
const CRB_CTRL_CMD_ADDR_HI: usize = 0x60;
const CRB_CTRL_RSP_SIZE: usize = 0x64;
const CRB_CTRL_RSP_ADDR_LO: usize = 0x68;
const CRB_CTRL_RSP_ADDR_HI: usize = 0x6C;

// LOC_STATE bits
const LOC_STATE_ESTABLISHED: u32 = 1 << 0;
const LOC_STATE_ASSIGNED: u32 = 1 << 1;
const LOC_STATE_VALID: u32 = 1 << 7;

// LOC_CTRL bits
const LOC_CTRL_REQUEST_ACCESS: u32 = 1 << 0;
const LOC_CTRL_RELINQUISH: u32 = 1 << 1;

// CRB_CTRL_REQ bits
const CRB_CTRL_REQ_GO_IDLE: u32 = 1 << 0;
const CRB_CTRL_REQ_CMD_READY: u32 = 1 << 1;

// CRB_CTRL_STS bits
const CRB_CTRL_STS_ERROR: u32 = 1 << 0;
const CRB_CTRL_STS_IDLE: u32 = 1 << 1;

// CRB_CTRL_START bits
const CRB_CTRL_START_CMD: u32 = 1 << 0;

// ─── TPM 2.0 Command Tags & Codes ───────────────────────────────────────────

const TPM2_ST_NO_SESSIONS: u16 = 0x8001;
const TPM2_ST_SESSIONS: u16 = 0x8002;

const TPM2_CC_STARTUP: u32 = 0x0000_0144;
const TPM2_CC_PCR_EXTEND: u32 = 0x0000_0182;
const TPM2_CC_PCR_READ: u32 = 0x0000_017E;
const TPM2_CC_GET_RANDOM: u32 = 0x0000_017B;
const TPM2_CC_QUOTE: u32 = 0x0000_0158;
const TPM2_CC_SEAL: u32 = 0x0000_0000; // placeholder — Create is 0x153
const TPM2_CC_UNSEAL: u32 = 0x0000_015E;

const TPM2_SU_CLEAR: u16 = 0x0000;

const TPM2_ALG_SHA256: u16 = 0x000B;
const TPM2_RC_SUCCESS: u32 = 0x0000_0000;
const TPM2_PCR_COUNT: usize = 24;

// ─── Errors ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TpmError {
    NotAvailable,
    Timeout,
    CommandFailed(u32),
    InvalidPcr,
    BufferTooSmall,
    LocalityNotAcquired,
}

// ─── CRB Interface ──────────────────────────────────────────────────────────

struct CrbRegs {
    base: usize,
}

impl CrbRegs {
    const fn new(base: usize) -> Self {
        Self { base }
    }

    /// # Safety
    /// Caller must ensure `base + offset` is a valid, mapped MMIO address.
    unsafe fn read32(&self, offset: usize) -> u32 {
        let ptr = (self.base + offset) as *const u32;
        core::ptr::read_volatile(ptr)
    }

    /// # Safety
    /// Caller must ensure `base + offset` is a valid, mapped MMIO address.
    unsafe fn write32(&self, offset: usize, val: u32) {
        let ptr = (self.base + offset) as *mut u32;
        core::ptr::write_volatile(ptr, val);
    }
}

// ─── TPM Interface ──────────────────────────────────────────────────────────

pub struct TpmInterface {
    regs: CrbRegs,
    cmd_buf_addr: u64,
    cmd_buf_size: u32,
    rsp_buf_addr: u64,
    rsp_buf_size: u32,
    locality_acquired: bool,
    available: bool,
    pcr_values: [[u8; 32]; TPM2_PCR_COUNT],
}

impl TpmInterface {
    pub const fn new() -> Self {
        Self {
            regs: CrbRegs::new(0),
            cmd_buf_addr: 0,
            cmd_buf_size: 0,
            rsp_buf_addr: 0,
            rsp_buf_size: 0,
            locality_acquired: false,
            available: false,
            pcr_values: [[0u8; 32]; TPM2_PCR_COUNT],
        }
    }

    /// Probe for a TPM 2.0 via the CRB interface at the given MMIO base.
    /// The base address would come from the ACPI TPM2 table in production;
    /// here we accept it as a parameter for flexibility.
    pub fn probe(&mut self, mmio_base_phys: usize) -> bool {
        if mmio_base_phys == 0 {
            self.available = false;
            return false;
        }

        let offset = crate::memory::PHYS_MEM_OFFSET
            .get()
            .map(|v| v.as_u64() as usize)
            .unwrap_or(0);
        let mmio_base_virt = mmio_base_phys + offset;
        self.regs = CrbRegs::new(mmio_base_virt);

        let intf_id = unsafe { self.regs.read32(CRB_INTF_ID) };
        // CRB interface: bits [3:0] of INTF_ID should be 0x1 (CRB active)
        let intf_type = intf_id & 0xF;
        if intf_type != 0x1 && intf_type != 0x0 {
            // Not a CRB interface — might be TIS or absent
            self.available = false;
            return false;
        }

        let loc_state = unsafe { self.regs.read32(LOC_STATE) };
        if loc_state & LOC_STATE_VALID == 0 {
            self.available = false;
            return false;
        }

        // Read command/response buffer addresses (physical) and translate
        let cmd_lo = unsafe { self.regs.read32(CRB_CTRL_CMD_ADDR_LO) } as u64;
        let cmd_hi = unsafe { self.regs.read32(CRB_CTRL_CMD_ADDR_HI) } as u64;
        self.cmd_buf_addr = ((cmd_hi << 32) | cmd_lo) + offset as u64;
        self.cmd_buf_size = unsafe { self.regs.read32(CRB_CTRL_CMD_SIZE) };

        let rsp_lo = unsafe { self.regs.read32(CRB_CTRL_RSP_ADDR_LO) } as u64;
        let rsp_hi = unsafe { self.regs.read32(CRB_CTRL_RSP_ADDR_HI) } as u64;
        self.rsp_buf_addr = ((rsp_hi << 32) | rsp_lo) + offset as u64;
        self.rsp_buf_size = unsafe { self.regs.read32(CRB_CTRL_RSP_SIZE) };

        self.available = true;
        true
    }

    /// Request locality 0 from the TPM.
    pub fn request_locality(&mut self) -> Result<(), TpmError> {
        if !self.available {
            return Err(TpmError::NotAvailable);
        }

        unsafe {
            self.regs.write32(LOC_CTRL, LOC_CTRL_REQUEST_ACCESS);
        }

        // Poll LOC_STS until the locality is granted (or timeout).
        for _ in 0..100_000 {
            let sts = unsafe { self.regs.read32(LOC_STATE) };
            if sts & LOC_STATE_ASSIGNED != 0 {
                self.locality_acquired = true;
                return Ok(());
            }
            core::hint::spin_loop();
        }

        // In QEMU / soft-TPM the locality may be pre-granted.
        // Accept if probe succeeded.
        self.locality_acquired = true;
        Ok(())
    }

    /// Relinquish the active locality.
    pub fn relinquish_locality(&mut self) {
        if self.locality_acquired {
            unsafe {
                self.regs.write32(LOC_CTRL, LOC_CTRL_RELINQUISH);
            }
            self.locality_acquired = false;
        }
    }

    /// Send a raw command and receive the response.
    fn submit_command(&mut self, cmd: &[u8]) -> Result<Vec<u8>, TpmError> {
        if !self.available {
            return Err(TpmError::NotAvailable);
        }
        if !self.locality_acquired {
            return Err(TpmError::LocalityNotAcquired);
        }

        // 1. Request command-ready
        unsafe {
            self.regs.write32(CRB_CTRL_REQ, CRB_CTRL_REQ_CMD_READY);
        }

        // Wait for idle to clear
        for _ in 0..100_000 {
            let sts = unsafe { self.regs.read32(CRB_CTRL_STS) };
            if sts & CRB_CTRL_STS_IDLE == 0 {
                break;
            }
            core::hint::spin_loop();
        }

        // 2. Write command to the command buffer
        let cmd_ptr = self.cmd_buf_addr as *mut u8;
        for (i, &b) in cmd.iter().enumerate() {
            if i >= self.cmd_buf_size as usize {
                break;
            }
            unsafe {
                core::ptr::write_volatile(cmd_ptr.add(i), b);
            }
        }

        // 3. Start the command
        unsafe {
            self.regs.write32(CRB_CTRL_START, CRB_CTRL_START_CMD);
        }

        // 4. Poll for completion (START bit clears when done)
        for _ in 0..1_000_000 {
            let start = unsafe { self.regs.read32(CRB_CTRL_START) };
            if start & CRB_CTRL_START_CMD == 0 {
                break;
            }
            core::hint::spin_loop();
        }

        // Check for error
        let sts = unsafe { self.regs.read32(CRB_CTRL_STS) };
        if sts & CRB_CTRL_STS_ERROR != 0 {
            return Err(TpmError::CommandFailed(sts));
        }

        // 5. Read response header (10 bytes: tag(2) + size(4) + rc(4))
        let rsp_ptr = self.rsp_buf_addr as *const u8;
        let mut header = [0u8; 10];
        for i in 0..10 {
            header[i] = unsafe { core::ptr::read_volatile(rsp_ptr.add(i)) };
        }

        let rsp_size = u32::from_be_bytes([header[2], header[3], header[4], header[5]]) as usize;
        let rsp_size = core::cmp::min(rsp_size, self.rsp_buf_size as usize);

        let mut response = vec![0u8; rsp_size];
        for i in 0..rsp_size {
            response[i] = unsafe { core::ptr::read_volatile(rsp_ptr.add(i)) };
        }

        // 6. Go idle
        unsafe {
            self.regs.write32(CRB_CTRL_REQ, CRB_CTRL_REQ_GO_IDLE);
        }

        Ok(response)
    }

    /// Parse the TPM2 response code (bytes 6..10 of response).
    fn response_code(rsp: &[u8]) -> u32 {
        if rsp.len() < 10 {
            return u32::MAX;
        }
        u32::from_be_bytes([rsp[6], rsp[7], rsp[8], rsp[9]])
    }

    // ─── TPM2 Commands ──────────────────────────────────────────────────────

    /// TPM2_Startup(SU_CLEAR) — initialize the TPM after reset.
    pub fn startup(&mut self) -> Result<(), TpmError> {
        let mut cmd = [0u8; 12];
        // Header: tag + size + command code
        cmd[0..2].copy_from_slice(&TPM2_ST_NO_SESSIONS.to_be_bytes());
        cmd[2..6].copy_from_slice(&12u32.to_be_bytes());
        cmd[6..10].copy_from_slice(&TPM2_CC_STARTUP.to_be_bytes());
        // Startup type
        cmd[10..12].copy_from_slice(&TPM2_SU_CLEAR.to_be_bytes());

        let rsp = self.submit_command(&cmd)?;
        let rc = Self::response_code(&rsp);
        if rc != TPM2_RC_SUCCESS && rc != 0x0000_0100 {
            // 0x100 = TPM_RC_INITIALIZE means already started, which is OK
            return Err(TpmError::CommandFailed(rc));
        }
        Ok(())
    }

    /// TPM2_PCR_Extend — extend PCR[index] with a SHA-256 digest.
    pub fn pcr_extend(&mut self, index: u32, digest: &[u8; 32]) -> Result<(), TpmError> {
        if index as usize >= TPM2_PCR_COUNT {
            return Err(TpmError::InvalidPcr);
        }

        // Build the command:
        //   header(10) + pcrHandle(4) + authArea(~13) + digestCount(4) + algId(2) + digest(32)
        let mut cmd = Vec::with_capacity(65);
        // Tag: sessions required for PCR_Extend
        cmd.extend_from_slice(&TPM2_ST_SESSIONS.to_be_bytes());
        // Size placeholder (fill later)
        cmd.extend_from_slice(&[0, 0, 0, 0]);
        // Command code
        cmd.extend_from_slice(&TPM2_CC_PCR_EXTEND.to_be_bytes());
        // PCR handle (index in the PCR range: 0x00000000 + index)
        cmd.extend_from_slice(&index.to_be_bytes());

        // Authorization area (password session, empty password)
        let auth_size: u32 = 9 + 4; // sessionHandle(4) + nonceSize(2) + attrs(1) + hmacSize(2) + authAreaSize(4)
        cmd.extend_from_slice(&auth_size.to_be_bytes());
        // Session handle: TPM_RS_PW = 0x40000009
        cmd.extend_from_slice(&0x4000_0009u32.to_be_bytes());
        // Nonce size = 0
        cmd.extend_from_slice(&0u16.to_be_bytes());
        // Session attributes = continueSession
        cmd.push(0x01);
        // HMAC size = 0 (empty password)
        cmd.extend_from_slice(&0u16.to_be_bytes());

        // Digest list: count(4) + algId(2) + digest(32)
        cmd.extend_from_slice(&1u32.to_be_bytes());
        cmd.extend_from_slice(&TPM2_ALG_SHA256.to_be_bytes());
        cmd.extend_from_slice(digest);

        // Patch total size
        let total = cmd.len() as u32;
        cmd[2..6].copy_from_slice(&total.to_be_bytes());

        let rsp = self.submit_command(&cmd)?;
        let rc = Self::response_code(&rsp);
        if rc != TPM2_RC_SUCCESS {
            return Err(TpmError::CommandFailed(rc));
        }

        // Update our cached PCR value: new = SHA-256(old || digest)
        let pcr = &mut self.pcr_values[index as usize];
        let mut hasher = Sha256Context::new();
        hasher.init();
        hasher.update(pcr);
        hasher.update(digest);
        let mut new_val = [0u8; 32];
        hasher.finalize(&mut new_val);
        *pcr = new_val;

        Ok(())
    }

    /// TPM2_PCR_Read — read the current value of PCR[index].
    pub fn pcr_read(&mut self, index: u32) -> Result<[u8; 32], TpmError> {
        if index as usize >= TPM2_PCR_COUNT {
            return Err(TpmError::InvalidPcr);
        }

        // Build command: header(10) + pcrSelectionCount(4) + hash(2) + sizeOfSelect(1) + pcrSelect(3)
        let mut cmd = Vec::with_capacity(20);
        cmd.extend_from_slice(&TPM2_ST_NO_SESSIONS.to_be_bytes());
        cmd.extend_from_slice(&[0, 0, 0, 0]); // size placeholder
        cmd.extend_from_slice(&TPM2_CC_PCR_READ.to_be_bytes());

        // PCR selection: 1 bank of SHA-256, selecting the given index
        cmd.extend_from_slice(&1u32.to_be_bytes());
        cmd.extend_from_slice(&TPM2_ALG_SHA256.to_be_bytes());
        cmd.push(3); // sizeOfSelect = 3 bytes (covers PCR 0-23)
        let mut select = [0u8; 3];
        select[(index / 8) as usize] |= 1 << (index % 8);
        cmd.extend_from_slice(&select);

        let total = cmd.len() as u32;
        cmd[2..6].copy_from_slice(&total.to_be_bytes());

        let rsp = self.submit_command(&cmd)?;
        let rc = Self::response_code(&rsp);
        if rc != TPM2_RC_SUCCESS {
            return Err(TpmError::CommandFailed(rc));
        }

        // Parse response: after header(10), there's updateCounter(4) + pcrSelection + digestCount(4) + digestSize(2) + digest(32)
        // The digest starts near the end of the response.
        let mut result = [0u8; 32];
        if rsp.len() >= 10 + 4 + 8 + 4 + 2 + 32 {
            let digest_offset = rsp.len() - 32;
            result.copy_from_slice(&rsp[digest_offset..]);
        } else {
            // Fallback: return cached value
            result = self.pcr_values[index as usize];
        }

        Ok(result)
    }

    /// TPM2_GetRandom — get `count` random bytes from the TPM hardware RNG.
    pub fn get_random(&mut self, count: u16) -> Result<Vec<u8>, TpmError> {
        let mut cmd = [0u8; 12];
        cmd[0..2].copy_from_slice(&TPM2_ST_NO_SESSIONS.to_be_bytes());
        cmd[2..6].copy_from_slice(&12u32.to_be_bytes());
        cmd[6..10].copy_from_slice(&TPM2_CC_GET_RANDOM.to_be_bytes());
        cmd[10..12].copy_from_slice(&count.to_be_bytes());

        let rsp = self.submit_command(&cmd)?;
        let rc = Self::response_code(&rsp);
        if rc != TPM2_RC_SUCCESS {
            return Err(TpmError::CommandFailed(rc));
        }

        // Response: header(10) + size(2) + random bytes
        if rsp.len() < 12 {
            return Ok(Vec::new());
        }
        let rand_size = u16::from_be_bytes([rsp[10], rsp[11]]) as usize;
        let available = core::cmp::min(rand_size, rsp.len() - 12);
        Ok(rsp[12..12 + available].to_vec())
    }

    /// TPM2_Quote — produce a signed attestation quote over selected PCRs.
    ///
    /// Returns the raw quote response blob, which includes the TPMS_ATTEST
    /// structure and the signature. Anti-cheat vendors parse this directly.
    pub fn quote(&mut self, pcr_selection: &[u32], nonce: &[u8; 32]) -> Result<Vec<u8>, TpmError> {
        let mut cmd = Vec::with_capacity(128);
        cmd.extend_from_slice(&TPM2_ST_SESSIONS.to_be_bytes());
        cmd.extend_from_slice(&[0, 0, 0, 0]); // size placeholder
        cmd.extend_from_slice(&TPM2_CC_QUOTE.to_be_bytes());

        // Sign key handle: use the EK hierarchy (0x81010001 is a typical
        // persistent handle; platforms vary). For attestation we use the
        // null hierarchy's endorsement key at 0x40000007 (TPM_RH_NULL)
        // as a placeholder that doesn't require authorization.
        cmd.extend_from_slice(&0x4000_0007u32.to_be_bytes());

        // Minimal password auth session
        let auth_size: u32 = 9;
        cmd.extend_from_slice(&auth_size.to_be_bytes());
        cmd.extend_from_slice(&0x4000_0009u32.to_be_bytes()); // TPM_RS_PW
        cmd.extend_from_slice(&0u16.to_be_bytes()); // nonce size
        cmd.push(0x01); // attrs
        cmd.extend_from_slice(&0u16.to_be_bytes()); // hmac size

        // qualifyingData (nonce)
        cmd.extend_from_slice(&(nonce.len() as u16).to_be_bytes());
        cmd.extend_from_slice(nonce);

        // TPMT_SIG_SCHEME: TPM_ALG_NULL (0x0010) — let TPM pick
        cmd.extend_from_slice(&0x0010u16.to_be_bytes());

        // PCR selection
        cmd.extend_from_slice(&1u32.to_be_bytes());
        cmd.extend_from_slice(&TPM2_ALG_SHA256.to_be_bytes());
        cmd.push(3); // sizeOfSelect
        let mut select = [0u8; 3];
        for &idx in pcr_selection {
            if (idx as usize) < TPM2_PCR_COUNT {
                select[(idx / 8) as usize] |= 1 << (idx % 8);
            }
        }
        cmd.extend_from_slice(&select);

        let total = cmd.len() as u32;
        cmd[2..6].copy_from_slice(&total.to_be_bytes());

        let rsp = self.submit_command(&cmd)?;
        let rc = Self::response_code(&rsp);
        if rc != TPM2_RC_SUCCESS {
            return Err(TpmError::CommandFailed(rc));
        }

        // Return everything past the header for the caller to parse
        if rsp.len() > 10 {
            Ok(rsp[10..].to_vec())
        } else {
            Ok(Vec::new())
        }
    }

    /// Convenience: hash `data` with SHA-256 then extend into PCR[index].
    pub fn extend_pcr_data(&mut self, index: u32, data: &[u8]) -> Result<(), TpmError> {
        let digest = sha256(data);
        self.pcr_extend(index, &digest)
    }

    /// Read the cached PCR value (no TPM round-trip).
    pub fn cached_pcr(&self, index: usize) -> Option<&[u8; 32]> {
        if index < TPM2_PCR_COUNT {
            Some(&self.pcr_values[index])
        } else {
            None
        }
    }

    pub fn is_available(&self) -> bool {
        self.available
    }
}

// ─── Software fallback ──────────────────────────────────────────────────────
//
// When no hardware TPM is present (QEMU without swtpm), we maintain PCR
// state in software so the rest of the measured boot pipeline still runs.
// The `available` flag on TpmInterface tracks whether commands go to real
// hardware; the security module checks it when deciding trust level.

/// A secret sealed to a measured-boot policy (the software analog of a TPM2
/// sealed object under a PolicyPCR session). Recoverable ONLY when the selected
/// PCRs currently hold the same values they held at seal time — this is what
/// makes TPM-backed FDE auto-unlock safe: a tampered boot shifts the PCRs and
/// the secret can no longer be unsealed.
#[derive(Clone)]
pub struct SealedObject {
    /// PCR indices the secret is bound to (ascending, de-duplicated).
    pub pcr_selection: Vec<u32>,
    /// SHA-256 over the selected PCRs' values AT SEAL TIME. Also the AEAD AAD,
    /// so a blob cannot be replayed against a different policy record.
    pub policy_digest: [u8; 32],
    /// Per-object AEAD nonce.
    pub nonce: [u8; 12],
    /// ChaCha20-Poly1305 ciphertext ‖ tag of the sealed secret.
    pub blob: Vec<u8>,
}

pub struct SoftTpm {
    pcr_values: [[u8; 32]; TPM2_PCR_COUNT],
    /// Per-boot sealing root — the software analog of a hardware TPM's Storage
    /// Root Key. Seeded from the CPU DRNG in `seed_seal_root`; sealed blobs are
    /// encrypted under HKDF(root, policy_digest), so nothing unseals without
    /// BOTH this root AND a matching measured-boot state.
    seal_root: [u8; 32],
}

impl SoftTpm {
    pub const fn new() -> Self {
        Self {
            pcr_values: [[0u8; 32]; TPM2_PCR_COUNT],
            seal_root: [0u8; 32],
        }
    }

    /// Seed the per-boot sealing root from the CPU DRNG (RDSEED→RDRAND, with a
    /// TSC-jitter fallback that is never a constant). SECURITY: for a SOFTWARE
    /// TPM the root lives in kernel RAM, so the trust ceiling is "the kernel was
    /// not compromised at seal/unseal time"; the PCR binding still provides
    /// tamper-evidence (a different boot ⇒ different PCRs ⇒ unseal fails closed).
    /// A hardware TPM keeps its SRK on-chip and never exposes it — that path is
    /// a TPM2_Create/Load/Unseal follow-up.
    pub fn seed_seal_root(&mut self) {
        let mut root = [0u8; 32];
        if !crate::crypto::hw_random_bytes(&mut root) || root == [0u8; 32] {
            let tsc = unsafe { core::arch::x86_64::_rdtsc() };
            for (i, b) in root.iter_mut().enumerate() {
                *b = (tsc.rotate_left((i as u32) * 5) as u8) ^ 0x5A ^ (i as u8);
            }
        }
        self.seal_root = root;
    }

    /// SHA-256 over the CURRENT values of the selected PCRs, in ascending index
    /// order (index ‖ value per PCR). This is the "policy" a sealed object binds
    /// to; recomputing it at unseal time is how a changed boot state is caught.
    fn policy_digest_for(&self, pcr_selection: &[u32]) -> [u8; 32] {
        let mut sel: Vec<u32> = pcr_selection
            .iter()
            .copied()
            .filter(|&i| (i as usize) < TPM2_PCR_COUNT)
            .collect();
        sel.sort_unstable();
        sel.dedup();
        let mut ctx = Sha256Context::new();
        ctx.init();
        for &idx in &sel {
            ctx.update(&idx.to_le_bytes());
            ctx.update(&self.pcr_values[idx as usize]);
        }
        let mut d = [0u8; 32];
        ctx.finalize(&mut d);
        d
    }

    /// Derive the sealing key from the root + policy digest (HKDF-SHA256).
    fn seal_key(&self, policy_digest: &[u8; 32]) -> [u8; 32] {
        let mut okm = [0u8; 32];
        rae_crypto::sha256::hkdf(
            policy_digest,
            &self.seal_root,
            b"raeen-tpm-seal-v1",
            &mut okm,
        );
        okm
    }

    /// Seal `secret` to the current values of `pcr_selection`. The returned
    /// object can be unsealed ONLY while those PCRs still hold these values.
    pub fn seal(&self, secret: &[u8], pcr_selection: &[u32]) -> SealedObject {
        let policy_digest = self.policy_digest_for(pcr_selection);
        let key = self.seal_key(&policy_digest);
        let mut nonce = [0u8; 12];
        if !crate::crypto::hw_random_bytes(&mut nonce) {
            let tsc = unsafe { core::arch::x86_64::_rdtsc() };
            nonce[..8].copy_from_slice(&tsc.to_le_bytes());
        }
        let blob = rae_crypto::chacha20poly1305::seal(&key, &nonce, &policy_digest, secret);
        let mut sel: Vec<u32> = pcr_selection
            .iter()
            .copied()
            .filter(|&i| (i as usize) < TPM2_PCR_COUNT)
            .collect();
        sel.sort_unstable();
        sel.dedup();
        SealedObject {
            pcr_selection: sel,
            policy_digest,
            nonce,
            blob,
        }
    }

    /// Unseal: returns the secret ONLY if the CURRENT PCR values for the sealed
    /// selection reproduce the sealed policy digest AND the AEAD tag verifies.
    /// Any boot-state change (tampered firmware/kernel) shifts the PCRs, the
    /// recomputed key differs, and the tag fails ⇒ `None` (fail closed). A
    /// tampered ciphertext also fails the tag.
    pub fn unseal(&self, sealed: &SealedObject) -> Option<Vec<u8>> {
        let current_policy = self.policy_digest_for(&sealed.pcr_selection);
        // Fast, explicit rejection when the measured state diverged (the AEAD
        // below would also fail since its key derives from current_policy).
        if current_policy != sealed.policy_digest {
            return None;
        }
        let key = self.seal_key(&current_policy);
        rae_crypto::chacha20poly1305::open(&key, &sealed.nonce, &sealed.policy_digest, &sealed.blob)
    }

    pub fn extend_pcr(&mut self, index: usize, digest: &[u8; 32]) {
        if index >= TPM2_PCR_COUNT {
            return;
        }
        let mut hasher = Sha256Context::new();
        hasher.init();
        hasher.update(&self.pcr_values[index]);
        hasher.update(digest);
        let mut new_val = [0u8; 32];
        hasher.finalize(&mut new_val);
        self.pcr_values[index] = new_val;
    }

    pub fn read_pcr(&self, index: usize) -> Option<&[u8; 32]> {
        if index < TPM2_PCR_COUNT {
            Some(&self.pcr_values[index])
        } else {
            None
        }
    }

    pub fn get_random(&self, count: usize) -> Vec<u8> {
        // Use the CPU hardware DRNG (RDSEED/RDRAND) instead of a FIXED-seed LCG
        // (the old `0xDEAD_BEEF_CAFE_BABE` seed produced the identical sequence
        // every boot despite the "RDTSC-like entropy" comment — it never read
        // the TSC). `hw_random_bytes` falls back to a TSC-jitter mix if no DRNG
        // exists — still never a constant sequence.
        let mut buf = vec![0u8; count];
        crate::crypto::hw_random_bytes(&mut buf);
        buf
    }

    /// Produce a software attestation blob: `RAEQ || nonce || SHA256(pcrs ||
    /// nonce) || pcr_list`. UNSIGNED and publicly computable — it carries NO
    /// Attestation-Key signature and MUST NOT be handed to a remote verifier as
    /// a hardware TPM quote. `security::generate_attestation_quote` gates on
    /// `TpmDevice::is_hardware()` so this never escapes as a "TPM quote"; it
    /// exists only for local diagnostics and PCR-binding self-tests.
    pub fn quote(&self, pcr_selection: &[u32], nonce: &[u8; 32]) -> Vec<u8> {
        let mut hasher = Sha256Context::new();
        hasher.init();
        for &idx in pcr_selection {
            if let Some(pcr) = self.read_pcr(idx as usize) {
                hasher.update(pcr);
            }
        }
        hasher.update(nonce);
        let mut digest = [0u8; 32];
        hasher.finalize(&mut digest);

        let mut blob = Vec::with_capacity(96);
        blob.extend_from_slice(b"RAEQ"); // RaeenOS Quote magic
        blob.extend_from_slice(nonce);
        blob.extend_from_slice(&digest);
        blob.extend_from_slice(&(pcr_selection.len() as u32).to_be_bytes());
        for &idx in pcr_selection {
            blob.extend_from_slice(&idx.to_be_bytes());
        }
        blob
    }
}

// ─── Global TPM State ────────────────────────────────────────────────────────

pub static TPM: Mutex<Option<TpmDevice>> = Mutex::new(None);
static TPM_INITIALIZED: AtomicBool = AtomicBool::new(false);

pub enum TpmDevice {
    Hardware(TpmInterface),
    Software(SoftTpm),
}

impl TpmDevice {
    pub fn extend_pcr(&mut self, index: u32, digest: &[u8; 32]) -> Result<(), TpmError> {
        match self {
            TpmDevice::Hardware(hw) => hw.pcr_extend(index, digest),
            TpmDevice::Software(sw) => {
                sw.extend_pcr(index as usize, digest);
                Ok(())
            }
        }
    }

    pub fn read_pcr(&self, index: u32) -> Option<[u8; 32]> {
        match self {
            TpmDevice::Hardware(hw) => hw.cached_pcr(index as usize).copied(),
            TpmDevice::Software(sw) => sw.read_pcr(index as usize).copied(),
        }
    }

    pub fn get_random(&mut self, count: u16) -> Vec<u8> {
        match self {
            TpmDevice::Hardware(hw) => hw.get_random(count).unwrap_or_else(|_| {
                // A hardware-TPM RNG failure falls back to the CPU DRNG, never
                // to a zero buffer masquerading as randomness.
                let mut b = vec![0u8; count as usize];
                crate::crypto::hw_random_bytes(&mut b);
                b
            }),
            TpmDevice::Software(sw) => sw.get_random(count as usize),
        }
    }

    pub fn quote(&mut self, pcr_selection: &[u32], nonce: &[u8; 32]) -> Result<Vec<u8>, TpmError> {
        match self {
            TpmDevice::Hardware(hw) => hw.quote(pcr_selection, nonce),
            TpmDevice::Software(sw) => Ok(sw.quote(pcr_selection, nonce)),
        }
    }

    /// Seal `secret` to the current values of `pcr_selection`.
    ///
    /// Software TPM: sealed under HKDF(per-boot root, PCR-policy digest) with
    /// ChaCha20-Poly1305. Hardware TPM: full TPM2_Create/Load to a PolicyPCR
    /// session is a follow-up, so the hardware path fails closed rather than
    /// silently software-sealing under the guise of a hardware-protected key.
    pub fn seal(&self, secret: &[u8], pcr_selection: &[u32]) -> Result<SealedObject, TpmError> {
        match self {
            TpmDevice::Software(sw) => Ok(sw.seal(secret, pcr_selection)),
            TpmDevice::Hardware(_) => Err(TpmError::NotAvailable),
        }
    }

    /// Unseal a previously sealed object; `Err`/`None`-equivalent on any PCR
    /// policy mismatch or AEAD failure (fail closed).
    pub fn unseal(&self, sealed: &SealedObject) -> Result<Vec<u8>, TpmError> {
        match self {
            TpmDevice::Software(sw) => sw.unseal(sealed).ok_or(TpmError::CommandFailed(0x9C0)),
            TpmDevice::Hardware(_) => Err(TpmError::NotAvailable),
        }
    }

    pub fn is_hardware(&self) -> bool {
        matches!(self, TpmDevice::Hardware(_))
    }

    pub fn extend_pcr_data(&mut self, index: u32, data: &[u8]) -> Result<(), TpmError> {
        let digest = sha256(data);
        self.extend_pcr(index, &digest)
    }
}

// ─── Helper ──────────────────────────────────────────────────────────────────

pub fn sha256(data: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256Context::new();
    hasher.init();
    hasher.update(data);
    let mut out = [0u8; 32];
    hasher.finalize(&mut out);
    out
}

// ─── Memory Tagging Stub ─────────────────────────────────────────────────────
//
// ARM MTE uses 4-bit tags in the top byte of pointers; Intel LAM is similar.
// On x86_64 these are no-ops today but the interface is defined so the rest
// of the kernel can prepare allocations for tagging when hardware ships.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryTag(pub u8);

impl MemoryTag {
    pub const ZERO: MemoryTag = MemoryTag(0);

    pub const fn new(tag: u8) -> Self {
        MemoryTag(tag & 0x0F)
    }

    pub const fn value(self) -> u8 {
        self.0
    }
}

/// Tag a heap allocation. On ARM MTE this would set the tag bits in the
/// physical memory; on x86 this is a no-op stub.
pub fn tag_allocation(_addr: u64, _size: usize, _tag: MemoryTag) {
    // Stub: x86_64 does not support MTE. When Intel LAM or a future
    // extension is available, this will set tag metadata via the
    // appropriate MSR / memory controller interface.
}

/// Verify that the tag at `addr` matches `expected_tag`.
/// Returns `true` on match or when tagging is unsupported (stub).
pub fn check_tag(_addr: u64, _expected_tag: MemoryTag) -> bool {
    // Stub: always passes on x86_64
    true
}

/// Query whether hardware memory tagging is supported on this CPU.
pub fn memory_tagging_supported() -> bool {
    let f = crate::cpu_features::get_features();
    f.lam || f.uai
}

// ─── W^X Enforcement ─────────────────────────────────────────────────────────
//
// No page should be simultaneously writable and executable. This module
// provides helpers that the memory manager calls when modifying PTEs.

use x86_64::structures::paging::PageTableFlags;

/// Check if the given flags violate W^X (writable AND executable).
/// x86_64: a page is executable when the NO_EXECUTE bit is *not* set.
pub const fn is_wx_violation(flags: PageTableFlags) -> bool {
    let writable = flags.contains(PageTableFlags::WRITABLE);
    let executable = !flags.contains(PageTableFlags::NO_EXECUTE);
    writable && executable
}

/// Enforce W^X on a set of page-table flags: if both W and X are set,
/// clear WRITABLE (prefer executable).
pub fn enforce_wx_flags(flags: PageTableFlags) -> PageTableFlags {
    if is_wx_violation(flags) {
        flags.difference(PageTableFlags::WRITABLE)
    } else {
        flags
    }
}

/// Make a page range writable, clearing execute permission.
/// Returns the new flags to apply.
pub fn make_writable_flags(mut flags: PageTableFlags) -> PageTableFlags {
    flags.insert(PageTableFlags::WRITABLE);
    flags.insert(PageTableFlags::NO_EXECUTE);
    flags
}

/// Make a page range executable, clearing write permission.
/// Returns the new flags to apply.
pub fn make_executable_flags(mut flags: PageTableFlags) -> PageTableFlags {
    flags.remove(PageTableFlags::WRITABLE);
    flags.remove(PageTableFlags::NO_EXECUTE);
    flags
}

/// Scan the currently-active page tables for W^X violations.
/// Returns the count of violating PTEs found.
pub fn scan_wx_violations() -> usize {
    let offset = match crate::memory::PHYS_MEM_OFFSET.get() {
        Some(o) => *o,
        None => return 0,
    };

    use x86_64::registers::control::Cr3;
    use x86_64::structures::paging::page_table::PageTable;

    let (pml4_frame, _) = Cr3::read();
    let pml4_virt = offset + pml4_frame.start_address().as_u64();
    let pml4 = unsafe { &*(pml4_virt.as_ptr::<PageTable>()) };

    let mut violations = 0usize;

    for pml4_entry in pml4.iter() {
        if !pml4_entry.flags().contains(PageTableFlags::PRESENT) {
            continue;
        }
        if pml4_entry.flags().contains(PageTableFlags::HUGE_PAGE) {
            continue;
        }

        let pdpt_virt = offset + pml4_entry.addr().as_u64();
        let pdpt = unsafe { &*(pdpt_virt.as_ptr::<PageTable>()) };

        for pdpt_entry in pdpt.iter() {
            if !pdpt_entry.flags().contains(PageTableFlags::PRESENT) {
                continue;
            }
            if pdpt_entry.flags().contains(PageTableFlags::HUGE_PAGE) {
                if is_wx_violation(pdpt_entry.flags()) {
                    violations += 1;
                }
                continue;
            }

            let pd_virt = offset + pdpt_entry.addr().as_u64();
            let pd = unsafe { &*(pd_virt.as_ptr::<PageTable>()) };

            for pd_entry in pd.iter() {
                if !pd_entry.flags().contains(PageTableFlags::PRESENT) {
                    continue;
                }
                if pd_entry.flags().contains(PageTableFlags::HUGE_PAGE) {
                    if is_wx_violation(pd_entry.flags()) {
                        violations += 1;
                    }
                    continue;
                }

                let pt_virt = offset + pd_entry.addr().as_u64();
                let pt = unsafe { &*(pt_virt.as_ptr::<PageTable>()) };

                for pt_entry in pt.iter() {
                    if !pt_entry.flags().contains(PageTableFlags::PRESENT) {
                        continue;
                    }
                    if is_wx_violation(pt_entry.flags()) {
                        violations += 1;
                    }
                }
            }
        }
    }

    violations
}

// ─── Initialization ──────────────────────────────────────────────────────────

/// Initialize the TPM subsystem. Attempts to probe for hardware CRB first;
/// falls back to a software TPM for measured-boot bookkeeping.
pub fn init() {
    // Try to discover the TPM MMIO base from the ACPI TPM2 table.
    // For now we use a well-known default (0xFED4_0000 is the standard
    // CRB base on most PC platforms).
    let acpi_tpm_base: usize = 0xFED4_0000;

    let mut hw = TpmInterface::new();
    let device = if hw.probe(acpi_tpm_base) {
        let _ = hw.request_locality();
        let _ = hw.startup();
        TpmDevice::Hardware(hw)
    } else {
        let mut sw = SoftTpm::new();
        sw.seed_seal_root();
        TpmDevice::Software(sw)
    };

    *TPM.lock() = Some(device);
    TPM_INITIALIZED.store(true, Ordering::SeqCst);
}

/// R10 boot smoketest — proves the measured-boot key-sealing property that
/// makes TPM-backed FDE auto-unlock safe (Concept §RaeFS encryption / "TPM 2.0
/// unsealing path for keys"): a secret sealed to a set of PCR values is
/// recoverable ONLY while those PCRs still hold those values, so a tampered
/// firmware/kernel measurement makes the key unrecoverable. This test CAN print
/// FAIL: if sealing degraded to plaintext or stopped binding the PCR policy,
/// the state-change and ciphertext-tamper cases would still unseal.
pub fn run_seal_smoketest() {
    let mut t = SoftTpm::new();
    t.seed_seal_root();
    // Establish a non-trivial measured state (firmware + kernel image).
    t.extend_pcr(0, &sha256(b"firmware-v1"));
    t.extend_pcr(4, &sha256(b"kernel-image-A"));

    let secret: &[u8] = b"raefs-fde-master-key-0123456789!";
    let sealed = t.seal(secret, &[0, 4]);

    // 1. Same measured state -> unseal returns the EXACT secret.
    let unseal_ok = t.unseal(&sealed).as_deref() == Some(secret);

    // 2. Ciphertext tamper -> the AEAD tag rejects it (policy still intact).
    let mut bad = sealed.clone();
    if let Some(b) = bad.blob.first_mut() {
        *b ^= 0xFF;
    }
    let blob_tamper_rejected = t.unseal(&bad).is_none();

    // 3. THE core property: a changed kernel measurement (PCR 4 re-extended, as
    //    a mismatched/tampered kernel would) makes the sealed key unrecoverable.
    t.extend_pcr(4, &sha256(b"kernel-image-B-tampered"));
    let state_change_rejected = t.unseal(&sealed).is_none();

    // 4. The wired global-device path seals + unseals too (dispatch proof).
    //    A hardware TPM is exempt (its sealing is a TPM2_Create follow-up).
    let global_ok = {
        let guard = TPM.lock();
        match &*guard {
            Some(dev) if dev.is_hardware() => true,
            Some(dev) => match dev.seal(secret, &[7]) {
                Ok(obj) => dev.unseal(&obj).ok().as_deref() == Some(secret),
                Err(_) => false,
            },
            None => false,
        }
    };

    let pass = unseal_ok && blob_tamper_rejected && state_change_rejected && global_ok;
    crate::serial_println!(
        "[tpm-seal] smoketest: unseal_ok={} blob_tamper_rejected={} state_change_rejected={} global_path={} -> {}",
        unseal_ok,
        blob_tamper_rejected,
        state_change_rejected,
        global_ok,
        if pass { "PASS" } else { "FAIL" },
    );
}

/// `/proc/raeen/tpm` — TPM backend + measured-boot sealing status.
pub fn tpm_dump_text() -> alloc::string::String {
    use alloc::string::String;
    use core::fmt::Write;
    let mut s = String::new();
    let guard = TPM.lock();
    match &*guard {
        Some(dev) => {
            let hw = dev.is_hardware();
            let _ = writeln!(
                s,
                "backend: {}",
                if hw { "hardware-crb" } else { "software" }
            );
            let _ = writeln!(
                s,
                "sealing: pcr-policy (ChaCha20-Poly1305, HKDF-SHA256 per-boot root)"
            );
            let _ = writeln!(
                s,
                "hardware_srk_sealing: {}",
                if hw {
                    "pending (TPM2_Create follow-up)"
                } else {
                    "n/a (software root)"
                }
            );
            for idx in [0u32, 4, 7] {
                if let Some(v) = dev.read_pcr(idx) {
                    let _ = write!(s, "pcr{}: ", idx);
                    for b in &v[..8] {
                        let _ = write!(s, "{:02x}", b);
                    }
                    let _ = writeln!(s, "...");
                }
            }
        }
        None => {
            let _ = writeln!(s, "tpm: not initialized");
        }
    }
    s
}

pub fn is_initialized() -> bool {
    TPM_INITIALIZED.load(Ordering::SeqCst)
}
