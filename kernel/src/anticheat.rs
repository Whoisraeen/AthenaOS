//! Anti-cheat and kernel integrity subsystem for AthenaOS.
//!
//! Anti-cheat exists for the *game publishers*, not the user: its job is to let
//! competitive titles (Fortnite, Valorant, Overwatch, …) run on AthenaOS without
//! handing cheaters an easy exploit surface. AthenaOS uses a **two-tier strategy**
//! (full rationale in `docs/ANTICHEAT_STRATEGY.md`):
//!
//!   - **Tier 1 — userspace attestation (this module, the default).** A
//!     hardware-backed attestation API EAC/BattlEye/Vanguard can query from
//!     **user-space** without a ring-0 driver: the kernel continuously monitors
//!     process integrity, enforces W^X, guards its own structures (syscall table,
//!     IDT, .text), and signs an attestation the vendor's servers trust. This is
//!     the user-respecting "better primitive" we push every vendor toward first.
//!
//!   - **Tier 2 — sanctioned kernel anti-cheat, ONLY for titles that require it.**
//!     Some publishers mandate a kernel vantage point and won't ship without one;
//!     refusing that (Linux's stance) is why those games are unplayable there. For
//!     those titles AthenaOS offers a *signed, countersigned, per-game,
//!     load-on-launch* kernel AC module slot — bounded, audited, unloaded on exit,
//!     gated by explicit user consent. Ring-0 on a leash, not a boot-resident
//!     rootkit. The detection primitives below (code hashing, hook/debugger
//!     detection in `MemoryProtectionEngine`) are the kernel-side API such a module
//!     builds on. **Tier 2 is strategy-only today — the framework is not yet built.**
//!
//! Syscall interface (numbers 284-290; renumbered from 100-106 — see rae_abi):
//!   SYS_AC_REQUEST_ATTESTATION  — start a new attestation session
//!   SYS_AC_VERIFY_ATTESTATION   — verify session result
//!   SYS_AC_REGISTER_GAME        — register a game process
//!   SYS_AC_UNREGISTER_GAME      — tear down a game session
//!   SYS_AC_REPORT_VIOLATION     — userspace reports suspicious activity
//!   SYS_AC_QUERY_STATUS         — query session / process status
//!   SYS_AC_HEARTBEAT            — keepalive from the anti-cheat client

#![allow(dead_code)]

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

// ───────────────────────────────────────────────────────────────────────────────
// 1. Process Integrity Verification
// ───────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PagePermissions {
    pub readable: bool,
    pub writable: bool,
    pub executable: bool,
    pub user: bool,
}

impl PagePermissions {
    pub const fn kernel_ro_exec() -> Self {
        Self {
            readable: true,
            writable: false,
            executable: true,
            user: false,
        }
    }

    pub const fn user_rw() -> Self {
        Self {
            readable: true,
            writable: true,
            executable: false,
            user: true,
        }
    }

    pub fn is_wx(&self) -> bool {
        self.writable && self.executable
    }
}

#[derive(Debug, Clone)]
pub struct CodePageEntry {
    pub virt_addr: u64,
    pub phys_addr: u64,
    pub hash: [u8; 32],
    pub permissions: PagePermissions,
    pub writable_at_any_point: bool,
}

impl CodePageEntry {
    fn verify_hash(&self, current_hash: &[u8; 32]) -> bool {
        self.hash == *current_hash
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViolationType {
    CodeModification,
    WxViolation,
    DebuggerAttached,
    UnauthorizedMemoryAccess,
    InjectedThread,
    SuspiciousAllocation,
    HookDetected,
    KernelModuleInserted,
    SystemCallHooked,
    TimingAnomaly,
}

#[derive(Debug, Clone)]
pub struct IntegrityViolation {
    pub timestamp: u64,
    pub violation_type: ViolationType,
    pub address: u64,
    pub details: String,
}

#[derive(Debug, Clone)]
pub struct ProcessIntegrity {
    pub pid: u64,
    pub code_hash: [u8; 32],
    pub stack_canary: u64,
    pub heap_guard_pages: Vec<u64>,
    pub code_pages: Vec<CodePageEntry>,
    pub last_check: u64,
    pub violations: Vec<IntegrityViolation>,
    pub trusted: bool,
}

impl ProcessIntegrity {
    pub fn new(pid: u64) -> Self {
        let canary = generate_stack_canary(pid);
        Self {
            pid,
            code_hash: [0u8; 32],
            stack_canary: canary,
            heap_guard_pages: Vec::new(),
            code_pages: Vec::new(),
            last_check: 0,
            violations: Vec::new(),
            trusted: true,
        }
    }

    pub fn record_violation(&mut self, v: IntegrityViolation) {
        self.trusted = false;
        self.violations.push(v);
    }

    pub fn violation_count(&self) -> usize {
        self.violations.len()
    }

    pub fn has_wx_pages(&self) -> bool {
        self.code_pages.iter().any(|p| p.permissions.is_wx())
    }

    pub fn register_code_page(&mut self, entry: CodePageEntry) {
        if entry.permissions.is_wx() {
            self.record_violation(IntegrityViolation {
                timestamp: self.last_check,
                violation_type: ViolationType::WxViolation,
                address: entry.virt_addr,
                details: String::from("page mapped W+X at registration time"),
            });
        }
        self.code_pages.push(entry);
    }

    pub fn add_guard_page(&mut self, addr: u64) {
        self.heap_guard_pages.push(addr);
    }

    fn scan_code_pages(&mut self, timestamp: u64) -> Vec<IntegrityViolation> {
        let mut found = Vec::new();
        for page in &self.code_pages {
            let current = compute_page_hash(page.virt_addr);
            if !page.verify_hash(&current) {
                found.push(IntegrityViolation {
                    timestamp,
                    violation_type: ViolationType::CodeModification,
                    address: page.virt_addr,
                    details: String::from("code page hash mismatch"),
                });
            }
            if page.permissions.is_wx() {
                found.push(IntegrityViolation {
                    timestamp,
                    violation_type: ViolationType::WxViolation,
                    address: page.virt_addr,
                    details: String::from("page simultaneously writable and executable"),
                });
            }
            if page.writable_at_any_point && page.permissions.executable {
                found.push(IntegrityViolation {
                    timestamp,
                    violation_type: ViolationType::SuspiciousAllocation,
                    address: page.virt_addr,
                    details: String::from("executable page was writable in the past (W^X)"),
                });
            }
        }
        found
    }
}

/// Pseudo-random canary seeded from PID and a compile-time salt.
fn generate_stack_canary(pid: u64) -> u64 {
    const SALT: u64 = (0xDAEE_DEAD_BEEF_CAFE_u64).wrapping_mul(0x517cc1b727220a95);
    pid.wrapping_mul(SALT)
        .wrapping_add(0x1337_c0de_0000_0001)
        .rotate_left(13)
        .wrapping_mul(0x9e3779b97f4a7c15)
}

/// Stub: in a real kernel this reads the 4 KiB page and hashes it via SHA-256.
fn compute_page_hash(virt_addr: u64) -> [u8; 32] {
    let mut h = [0u8; 32];
    let bytes = virt_addr.to_le_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        h[i] = b;
        h[i + 8] = b.wrapping_mul(0x9e).wrapping_add(i as u8);
    }
    h
}

// ───────────────────────────────────────────────────────────────────────────────
// 2. Memory Protection Engine
// ───────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WxPolicy {
    Strict,
    Relaxed,
    JitAllowed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntegrityResult {
    Clean,
    Tampered,
    Unknown,
}

#[derive(Debug, Clone)]
pub struct FunctionSignature {
    pub name: String,
    pub address: u64,
    pub prologue: [u8; 16],
}

#[derive(Debug, Clone)]
pub struct HookDetection {
    pub function_name: String,
    pub address: u64,
    pub expected_prologue: [u8; 16],
    pub actual_prologue: [u8; 16],
}

#[derive(Debug, Clone)]
pub struct InjectedCodeRegion {
    pub start: u64,
    pub size: u64,
    pub permissions: PagePermissions,
    pub origin_pid: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DebugState {
    None,
    DebuggerAttached,
    TracerPresent,
    HardwareBreakpointSet,
}

pub struct MemoryProtectionEngine {
    pub protected_processes: BTreeMap<u64, ProcessIntegrity>,
    pub guard_page_pool: Vec<u64>,
    pub wx_policy: WxPolicy,
    pub scan_interval_ms: u64,
}

impl MemoryProtectionEngine {
    pub fn new(policy: WxPolicy) -> Self {
        Self {
            protected_processes: BTreeMap::new(),
            guard_page_pool: Vec::new(),
            wx_policy: policy,
            scan_interval_ms: 1000,
        }
    }

    pub fn register_process(&mut self, pid: u64) -> &mut ProcessIntegrity {
        self.protected_processes
            .entry(pid)
            .or_insert_with(|| ProcessIntegrity::new(pid))
    }

    pub fn unregister_process(&mut self, pid: u64) -> Option<ProcessIntegrity> {
        self.protected_processes.remove(&pid)
    }

    pub fn verify_code_integrity(&self, pid: u64) -> IntegrityResult {
        let proc_int = match self.protected_processes.get(&pid) {
            Some(p) => p,
            None => return IntegrityResult::Unknown,
        };

        for page in &proc_int.code_pages {
            let current = compute_page_hash(page.virt_addr);
            if !page.verify_hash(&current) {
                return IntegrityResult::Tampered;
            }
        }
        IntegrityResult::Clean
    }

    pub fn check_wx_violation(&self, pid: u64) -> bool {
        match self.protected_processes.get(&pid) {
            Some(proc_int) => match self.wx_policy {
                WxPolicy::Strict => proc_int.has_wx_pages(),
                WxPolicy::JitAllowed => false,
                WxPolicy::Relaxed => proc_int
                    .code_pages
                    .iter()
                    .any(|p| p.permissions.is_wx() && !p.writable_at_any_point),
            },
            None => false,
        }
    }

    pub fn detect_hooks(
        &self,
        pid: u64,
        known_functions: &[FunctionSignature],
    ) -> Vec<HookDetection> {
        let mut detections = Vec::new();
        let proc_int = match self.protected_processes.get(&pid) {
            Some(p) => p,
            None => return detections,
        };

        for func in known_functions {
            let actual = read_function_prologue(func.address);
            if actual != func.prologue {
                // Check for common hook patterns: JMP (0xE9), INT3 (0xCC),
                // MOV RAX + JMP RAX (0x48 0xB8 … 0xFF 0xE0).
                let is_hook = actual[0] == 0xE9
                    || actual[0] == 0xCC
                    || (actual[0] == 0x48 && actual[1] == 0xB8);

                if is_hook || !proc_int.trusted {
                    detections.push(HookDetection {
                        function_name: func.name.clone(),
                        address: func.address,
                        expected_prologue: func.prologue,
                        actual_prologue: actual,
                    });
                }
            }
        }
        detections
    }

    pub fn scan_for_injected_code(&self, pid: u64) -> Vec<InjectedCodeRegion> {
        let mut regions = Vec::new();
        let proc_int = match self.protected_processes.get(&pid) {
            Some(p) => p,
            None => return regions,
        };

        for page in &proc_int.code_pages {
            if page.permissions.executable && page.writable_at_any_point {
                let original_hash = compute_page_hash(page.virt_addr);
                if original_hash != page.hash {
                    regions.push(InjectedCodeRegion {
                        start: page.virt_addr,
                        size: 4096,
                        permissions: page.permissions,
                        origin_pid: None,
                    });
                }
            }
        }
        regions
    }

    pub fn verify_stack_integrity(&self, pid: u64) -> bool {
        match self.protected_processes.get(&pid) {
            Some(proc_int) => {
                let expected = generate_stack_canary(pid);
                proc_int.stack_canary == expected
            }
            None => false,
        }
    }

    pub fn check_debug_state(&self, pid: u64) -> DebugState {
        let proc_int = match self.protected_processes.get(&pid) {
            Some(p) => p,
            None => return DebugState::None,
        };

        // Check DR7 (debug control register) — stub reads a sentinel.
        let dr7 = read_debug_register();
        if dr7 & 0xFF != 0 {
            return DebugState::HardwareBreakpointSet;
        }

        for v in &proc_int.violations {
            if v.violation_type == ViolationType::DebuggerAttached {
                return DebugState::DebuggerAttached;
            }
        }
        DebugState::None
    }

    pub fn full_scan(&mut self, timestamp: u64) {
        let pids: Vec<u64> = self.protected_processes.keys().copied().collect();
        for pid in pids {
            if let Some(proc_int) = self.protected_processes.get_mut(&pid) {
                let violations = proc_int.scan_code_pages(timestamp);
                for v in violations {
                    proc_int.record_violation(v);
                }
                proc_int.last_check = timestamp;
            }

            let debug = self.check_debug_state(pid);
            if debug != DebugState::None {
                if let Some(proc_int) = self.protected_processes.get_mut(&pid) {
                    proc_int.record_violation(IntegrityViolation {
                        timestamp,
                        violation_type: ViolationType::DebuggerAttached,
                        address: 0,
                        details: String::from("debugger or hardware breakpoint detected"),
                    });
                }
            }
        }
    }
}

/// Stub: read the function prologue (first 16 bytes) at the given address.
fn read_function_prologue(addr: u64) -> [u8; 16] {
    let mut buf = [0u8; 16];
    let bytes = addr.to_le_bytes();
    for i in 0..8 {
        buf[i] = bytes[i];
        buf[i + 8] = bytes[i].wrapping_add(1);
    }
    buf
}

/// Stub: read the x86-64 DR7 debug control register.
fn read_debug_register() -> u64 {
    0
}

// ───────────────────────────────────────────────────────────────────────────────
// 3. Hardware Attestation Service
// ───────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AntiCheatVendor {
    EasyAntiCheat,
    BattlEye,
    Vanguard,
    AthGuard,
    Custom(String),
}

impl AntiCheatVendor {
    pub fn name(&self) -> &str {
        match self {
            Self::EasyAntiCheat => "EasyAntiCheat",
            Self::BattlEye => "BattlEye",
            Self::Vanguard => "Vanguard",
            Self::AthGuard => "AthGuard",
            Self::Custom(n) => n.as_str(),
        }
    }

    pub fn from_id(id: u64) -> Self {
        match id {
            0 => Self::EasyAntiCheat,
            1 => Self::BattlEye,
            2 => Self::Vanguard,
            3 => Self::AthGuard,
            _ => Self::Custom(String::from("unknown")),
        }
    }
}

#[derive(Debug, Clone)]
pub struct AttestationResponse {
    pub session_id: u64,
    pub timestamp: u64,
    pub boot_chain_valid: bool,
    pub process_integrity: IntegrityResult,
    pub kernel_integrity: bool,
    pub wx_clean: bool,
    pub platform_signature: [u8; 64],
    pub measured_boot_pcrs: [[u8; 32]; 8],
    pub tpm_quote: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct AttestationPolicy {
    pub require_secure_boot: bool,
    pub require_signed_kernel: bool,
    pub max_session_duration_secs: u64,
    pub allow_debugger: bool,
    pub allow_hypervisor: bool,
    pub min_check_interval_ms: u64,
}

impl Default for AttestationPolicy {
    fn default() -> Self {
        Self {
            require_secure_boot: true,
            require_signed_kernel: true,
            max_session_duration_secs: 86400,
            allow_debugger: false,
            allow_hypervisor: true,
            min_check_interval_ms: 5000,
        }
    }
}

#[derive(Debug, Clone)]
pub struct AttestationSession {
    pub session_id: u64,
    pub game_pid: u64,
    pub anti_cheat_vendor: AntiCheatVendor,
    pub challenge: [u8; 32],
    pub response: Option<AttestationResponse>,
    pub created_at: u64,
    pub expires_at: u64,
    pub verified: bool,
}

impl AttestationSession {
    pub fn is_expired(&self, now: u64) -> bool {
        now >= self.expires_at
    }

    pub fn is_verified(&self) -> bool {
        self.verified && self.response.is_some()
    }
}

pub struct AttestationService {
    pub tpm_available: bool,
    pub platform_key: [u8; 32],
    pub nonce_pool: Vec<[u8; 16]>,
    pub active_sessions: BTreeMap<u64, AttestationSession>,
    pub policy: AttestationPolicy,
    next_session_id: u64,
}

impl AttestationService {
    pub fn new() -> Self {
        let mut nonces = Vec::with_capacity(64);
        for i in 0u64..64 {
            let mut nonce = [0u8; 16];
            let bytes = i.wrapping_mul(0x517cc1b727220a95).to_le_bytes();
            nonce[..8].copy_from_slice(&bytes);
            nonce[8..].copy_from_slice(&bytes);
            nonces.push(nonce);
        }

        Self {
            tpm_available: false,
            platform_key: [0u8; 32],
            nonce_pool: nonces,
            active_sessions: BTreeMap::new(),
            policy: AttestationPolicy::default(),
            next_session_id: 1,
        }
    }

    pub fn create_session(
        &mut self,
        game_pid: u64,
        vendor: AntiCheatVendor,
        timestamp: u64,
    ) -> u64 {
        let sid = self.next_session_id;
        self.next_session_id += 1;

        let challenge = self.generate_challenge(sid);
        let duration = self.policy.max_session_duration_secs;

        let session = AttestationSession {
            session_id: sid,
            game_pid,
            anti_cheat_vendor: vendor,
            challenge,
            response: None,
            created_at: timestamp,
            expires_at: timestamp + duration,
            verified: false,
        };
        self.active_sessions.insert(sid, session);
        sid
    }

    pub fn verify_session(
        &mut self,
        session_id: u64,
        process_integrity: IntegrityResult,
        kernel_ok: bool,
        timestamp: u64,
    ) -> bool {
        let challenge = {
            let session = match self.active_sessions.get(&session_id) {
                Some(s) => s,
                None => return false,
            };
            if session.is_expired(timestamp) {
                return false;
            }
            session.challenge
        };

        let boot_valid = self.check_boot_chain();
        let pcrs = self.read_boot_pcrs();
        let sig = self.sign_attestation(session_id, timestamp);
        let wx_violations = crate::tpm::scan_wx_violations();
        let wx_clean = wx_violations == 0;

        // Generate a real TPM2_Quote using the session challenge as nonce
        let tpm_quote = crate::security::generate_attestation_quote(&challenge);

        let response = AttestationResponse {
            session_id,
            timestamp,
            boot_chain_valid: boot_valid,
            process_integrity,
            kernel_integrity: kernel_ok,
            wx_clean,
            platform_signature: sig,
            measured_boot_pcrs: pcrs,
            tpm_quote,
        };

        let verified =
            boot_valid && process_integrity == IntegrityResult::Clean && kernel_ok && wx_clean;

        if let Some(session) = self.active_sessions.get_mut(&session_id) {
            session.response = Some(response);
            session.verified = verified;
        }
        verified
    }

    pub fn get_session(&self, session_id: u64) -> Option<&AttestationSession> {
        self.active_sessions.get(&session_id)
    }

    pub fn remove_session(&mut self, session_id: u64) -> Option<AttestationSession> {
        self.active_sessions.remove(&session_id)
    }

    pub fn purge_expired(&mut self, now: u64) {
        self.active_sessions.retain(|_, s| !s.is_expired(now));
    }

    fn generate_challenge(&self, seed: u64) -> [u8; 32] {
        // The challenge is the anti-replay/freshness nonce handed to the vendor.
        // It MUST be unpredictable: a deterministic function of the sequential
        // `session_id` (the old behaviour) makes every challenge precomputable,
        // which — combined with a known signing key — lets an attacker forge a
        // fresh-looking attestation in advance. Fill from the kernel CSPRNG.
        let mut challenge = [0u8; 32];
        if crate::crypto::getrandom(&mut challenge).is_ok() && challenge != [0u8; 32] {
            return challenge;
        }
        // CSPRNG unavailable — fall back to a TSC-perturbed derivation so the
        // nonce is at least boot-time-variable, not a pure function of the seed.
        let tsc = unsafe { core::arch::x86_64::_rdtsc() };
        let s = seed.wrapping_mul(0x9e3779b97f4a7c15) ^ tsc;
        let a = s.to_le_bytes();
        let b = s.wrapping_mul(0x517cc1b727220a95).to_le_bytes();
        let c = s.rotate_left(17).to_le_bytes();
        let d = s.rotate_right(23).to_le_bytes();
        challenge[0..8].copy_from_slice(&a);
        challenge[8..16].copy_from_slice(&b);
        challenge[16..24].copy_from_slice(&c);
        challenge[24..32].copy_from_slice(&d);
        challenge
    }

    fn check_boot_chain(&self) -> bool {
        if let Some(ref sb) = *crate::security::SECURE_BOOT.lock() {
            !sb.chain.is_sealed() || sb.chain.measurements.iter().all(|m| m.verified)
        } else {
            false
        }
    }

    fn read_boot_pcrs(&self) -> [[u8; 32]; 8] {
        let mut pcrs = [[0u8; 32]; 8];

        // Read from the real TPM device first
        if let Some(ref device) = *crate::tpm::TPM.lock() {
            let pcr_indices = [
                crate::security::PCR_FIRMWARE,
                crate::security::PCR_BOOTLOADER,
                2,
                3,
                crate::security::PCR_SECURE_BOOT_POLICY,
                crate::security::PCR_KERNEL_IMAGE,
                crate::security::PCR_KERNEL_CMDLINE,
                crate::security::PCR_RAESHIELD_POLICY,
            ];
            for (slot, &idx) in pcr_indices.iter().enumerate() {
                if slot >= 8 {
                    break;
                }
                if let Some(val) = device.read_pcr(idx) {
                    pcrs[slot] = val;
                }
            }
            return pcrs;
        }

        // Fallback to the security module's cached TPM state
        if let Some(ref sb) = *crate::security::SECURE_BOOT.lock() {
            for i in 0..8 {
                if let Some(val) = sb.tpm.read_pcr(i) {
                    pcrs[i] = *val;
                }
            }
        }
        pcrs
    }

    fn sign_attestation(&self, session_id: u64, timestamp: u64) -> [u8; 64] {
        // Build a keyed hash: HMAC-SHA256(platform_key, session_id || timestamp || pcrs)
        let mut msg = Vec::with_capacity(80);
        msg.extend_from_slice(&session_id.to_le_bytes());
        msg.extend_from_slice(&timestamp.to_le_bytes());

        // Mix in boot PCR values for binding
        if let Some(ref sb) = *crate::security::SECURE_BOOT.lock() {
            for i in 0..8 {
                if let Some(val) = sb.tpm.read_pcr(i) {
                    msg.extend_from_slice(val);
                }
            }
        }

        // HMAC-SHA256 with platform key
        let hmac = crate::crypto::HmacContext::new_sha256(&self.platform_key);
        let mut hash = [0u8; 32];
        hmac.compute(&msg, &mut hash);

        let mut sig = [0u8; 64];
        sig[0..32].copy_from_slice(&hash);
        // Second half: hash of (hash || platform_key) for extra binding
        let mut hasher = crate::crypto::Sha256Context::new();
        use crate::crypto::HashAlgorithm;
        hasher.init();
        hasher.update(&hash);
        hasher.update(&self.platform_key);
        hasher.finalize(&mut sig[32..64]);
        sig
    }
}

// ───────────────────────────────────────────────────────────────────────────────
// 4. Anti-Cheat Syscall Interface
// ───────────────────────────────────────────────────────────────────────────────

// Canonical numbers live in rae_abi (Block 34, 284–290). Re-exported here so
// the handlers read by name. RENUMBERED 2026-06-25 from 100–106, which collided
// with SYS_OOM_SUBSCRIBE (100) + AthFS snapshots (101–103) — calling the old
// SYS_AC_REGISTER_GAME (102) ran a destructive raefs::snapshot_restore.
pub const SYS_AC_REQUEST_ATTESTATION: u64 = rae_abi::syscall::SYS_AC_REQUEST_ATTESTATION;
pub const SYS_AC_VERIFY_ATTESTATION: u64 = rae_abi::syscall::SYS_AC_VERIFY_ATTESTATION;
pub const SYS_AC_REGISTER_GAME: u64 = rae_abi::syscall::SYS_AC_REGISTER_GAME;
pub const SYS_AC_UNREGISTER_GAME: u64 = rae_abi::syscall::SYS_AC_UNREGISTER_GAME;
pub const SYS_AC_REPORT_VIOLATION: u64 = rae_abi::syscall::SYS_AC_REPORT_VIOLATION;
pub const SYS_AC_QUERY_STATUS: u64 = rae_abi::syscall::SYS_AC_QUERY_STATUS;
pub const SYS_AC_HEARTBEAT: u64 = rae_abi::syscall::SYS_AC_HEARTBEAT;

const AC_OK: u64 = 0;
const AC_ERR_NOT_INITIALIZED: u64 = 1;
const AC_ERR_INVALID_SESSION: u64 = 2;
const AC_ERR_INVALID_PID: u64 = 3;
const AC_ERR_ALREADY_REGISTERED: u64 = 4;
const AC_ERR_NOT_REGISTERED: u64 = 5;
const AC_ERR_EXPIRED: u64 = 6;
const AC_ERR_BAD_ARGS: u64 = 7;
/// EPERM-equivalent: caller lacks `Cap::Attestation`, or is attempting to
/// reach across to another PID's session without daemon-tier authority.
const AC_ERR_PERM: u64 = 8;

// ── Privilege gate (SEV-3 fail-open closure) ──────────────────────────────
//
// Audit finding: anti-cheat syscalls 100-106 were ungated — any task,
// including a sandboxed one, could register/attest/poison another PID's
// session and feed an attacker-controlled timestamp. The gate below closes
// that. AthGuard mandate / Concept §Security criterion #6: no undocumented
// fail-open privileged op.
//
// Rule (fail-closed): the caller MUST hold `Cap::Attestation` with the
// required `Rights`. Ownership: a task may always act on its OWN session
// (`caller_pid == game_pid`); acting on ANOTHER PID's session (the legit
// anti-cheat-daemon path) additionally requires the daemon-tier authority
// marker — `Cap::Attestation` carrying the `GRANT` right. A narrow,
// self-scoped attestation cap therefore cannot be used to spoof a peer.

/// What a caller holds, distilled from its `Cap::Attestation` entries.
#[derive(Clone, Copy, Debug, Default)]
struct AttestAuthority {
    /// Holds `Cap::Attestation` with at least READ.
    can_read: bool,
    /// Holds `Cap::Attestation` with at least WRITE.
    can_write: bool,
    /// Holds `Cap::Attestation` with GRANT — the cross-PID daemon marker.
    is_daemon: bool,
}

/// Pure gate decision — explicit inputs so `run_boot_smoketest` can drive it
/// without a live task/cap-table context (and PROVE it can return PERM).
///
/// `need_write` distinguishes mutating calls (register/request/report/
/// unregister/heartbeat) from read-only ones (verify/query).
fn gate_decision(
    caller_pid: u64,
    game_pid: u64,
    auth: AttestAuthority,
    need_write: bool,
) -> Result<(), u64> {
    // 1. Capability gate: must hold Cap::Attestation with the needed right.
    let has_right = if need_write {
        auth.can_write
    } else {
        auth.can_read
    };
    if !has_right {
        return Err(AC_ERR_PERM);
    }
    // 2. Ownership gate: own session always OK; cross-PID needs daemon tier.
    if caller_pid != game_pid && !auth.is_daemon {
        return Err(AC_ERR_PERM);
    }
    Ok(())
}

/// Resolve the live caller's PID and its `Cap::Attestation` authority from the
/// scheduler/cap-table. Returns `None` if there is no current task (e.g. a
/// kernel-internal caller) — in which case the handler treats it as an
/// in-kernel/daemon context (the syscall edge always has a current task).
fn current_caller_authority() -> Option<(u64, AttestAuthority)> {
    use crate::capability::{Cap, Rights};
    crate::scheduler::with_current_task(|task| {
        let pid = task.id.raw();
        let mut auth = AttestAuthority::default();
        for (_, cap) in task.cap_table.iter() {
            if let Cap::Attestation { rights, .. } = cap {
                if rights.contains(Rights::READ) {
                    auth.can_read = true;
                }
                if rights.contains(Rights::WRITE) {
                    auth.can_write = true;
                }
                if rights.contains(Rights::GRANT) {
                    auth.is_daemon = true;
                }
            }
        }
        (pid, auth)
    })
}

/// Apply the gate at a live syscall edge. `Err(code)` short-circuits the
/// handler with that EPERM-equivalent code. Fail-closed: if there is no
/// current task to inspect, the privileged call is denied.
fn enforce_gate(game_pid: u64, need_write: bool) -> Result<(), u64> {
    let (caller_pid, auth) = current_caller_authority().ok_or(AC_ERR_PERM)?;
    gate_decision(caller_pid, game_pid, auth, need_write)
}

/// Kernel-stamped monotonic-ish timestamp in milliseconds. NEVER trust a
/// caller-supplied timestamp for session creation/expiry/replay — that is
/// attacker-controlled. Falls back to 0 only if no clock is available
/// (expiry math still works; sessions just never auto-expire pre-clock).
fn kernel_now_ms() -> u64 {
    crate::hpet::read_millis()
        .map(|ms| ms.max(0) as u64)
        .unwrap_or(0)
}

/// Top-level syscall dispatcher for anti-cheat calls (nr 284..=290).
///
/// `args` layout is syscall-specific; see individual handlers.
pub fn handle_anticheat_syscall(nr: u64, args: &[u64]) -> u64 {
    match nr {
        SYS_AC_REQUEST_ATTESTATION => handle_request_attestation(args),
        SYS_AC_VERIFY_ATTESTATION => handle_verify_attestation(args),
        SYS_AC_REGISTER_GAME => handle_register_game(args),
        SYS_AC_UNREGISTER_GAME => handle_unregister_game(args),
        SYS_AC_REPORT_VIOLATION => handle_report_violation(args),
        SYS_AC_QUERY_STATUS => handle_query_status(args),
        SYS_AC_HEARTBEAT => handle_heartbeat(args),
        _ => AC_ERR_BAD_ARGS,
    }
}

/// SYS_AC_REQUEST_ATTESTATION
/// args[0] = game PID, args[1] = vendor ID, args[2] = (ignored — kernel-stamped)
/// Returns session ID on success, error code on failure.
/// Gated: caller must hold `Cap::Attestation` (WRITE); cross-PID needs daemon tier.
fn handle_request_attestation(args: &[u64]) -> u64 {
    if args.len() < 3 {
        return AC_ERR_BAD_ARGS;
    }
    let game_pid = args[0];
    let vendor = AntiCheatVendor::from_id(args[1]);
    if let Err(code) = enforce_gate(game_pid, true) {
        return code;
    }
    // Kernel clock — never trust args[2] for session creation/expiry.
    let timestamp = kernel_now_ms();

    let mut guard = ANTICHEAT.lock();
    let mgr = match guard.as_mut() {
        Some(m) => m,
        None => return AC_ERR_NOT_INITIALIZED,
    };

    if !mgr.game_sessions.contains_key(&game_pid) {
        return AC_ERR_NOT_REGISTERED;
    }

    let sid = mgr.attestation.create_session(game_pid, vendor, timestamp);
    sid
}

/// SYS_AC_VERIFY_ATTESTATION
/// args[0] = session ID, args[1] = (ignored — kernel-stamped)
/// Returns 0 on verified, error code on failure.
/// Gated: caller must hold `Cap::Attestation` (READ); cross-PID needs daemon tier.
fn handle_verify_attestation(args: &[u64]) -> u64 {
    if args.len() < 2 {
        return AC_ERR_BAD_ARGS;
    }
    let session_id = args[0];
    // Kernel clock — never trust args[1] for expiry/replay decisions.
    let timestamp = kernel_now_ms();

    let mut guard = ANTICHEAT.lock();
    let mgr = match guard.as_mut() {
        Some(m) => m,
        None => return AC_ERR_NOT_INITIALIZED,
    };

    let game_pid = match mgr.attestation.get_session(session_id) {
        Some(s) => s.game_pid,
        None => return AC_ERR_INVALID_SESSION,
    };
    // Gate on the SESSION's owning PID so a peer can't verify someone else's.
    if let Err(code) = enforce_gate(game_pid, false) {
        return code;
    }
    let session = match mgr.attestation.get_session(session_id) {
        Some(s) => s,
        None => return AC_ERR_INVALID_SESSION,
    };
    if session.is_expired(timestamp) {
        return AC_ERR_EXPIRED;
    }

    let game_pid = session.game_pid;
    let proc_result = mgr.memory_engine.verify_code_integrity(game_pid);
    let kernel_ok = mgr.kernel_integrity.check_all();

    let verified = mgr
        .attestation
        .verify_session(session_id, proc_result, kernel_ok, timestamp);

    if verified {
        AC_OK
    } else {
        AC_ERR_INVALID_SESSION
    }
}

/// SYS_AC_REGISTER_GAME
/// args[0] = game PID, args[1] = vendor ID (or u64::MAX for none),
/// args[2] = heartbeat interval ms, args[3] = (ignored — kernel-stamped)
/// Returns 0 on success.
/// Gated: caller must hold `Cap::Attestation` (WRITE); cross-PID needs daemon tier.
fn handle_register_game(args: &[u64]) -> u64 {
    if args.len() < 4 {
        return AC_ERR_BAD_ARGS;
    }
    let game_pid = args[0];
    let vendor = if args[1] == u64::MAX {
        None
    } else {
        Some(AntiCheatVendor::from_id(args[1]))
    };
    let heartbeat_ms = args[2];
    if let Err(code) = enforce_gate(game_pid, true) {
        return code;
    }
    // Kernel clock — never trust args[3] for session start time.
    let timestamp = kernel_now_ms();

    let mut guard = ANTICHEAT.lock();
    let mgr = match guard.as_mut() {
        Some(m) => m,
        None => return AC_ERR_NOT_INITIALIZED,
    };

    if mgr.game_sessions.contains_key(&game_pid) {
        return AC_ERR_ALREADY_REGISTERED;
    }

    let session_id = mgr.next_session_id();
    let integrity = mgr.memory_engine.register_process(game_pid).clone();

    let gs = GameSession {
        game_pid,
        game_name: String::from("game"),
        session_id,
        started_at: timestamp,
        anti_cheat_vendor: vendor,
        integrity,
        heartbeat_interval_ms: heartbeat_ms,
        last_heartbeat: timestamp,
        status: GameSessionStatus::Active,
    };
    mgr.game_sessions.insert(game_pid, gs);
    AC_OK
}

/// SYS_AC_UNREGISTER_GAME
/// args[0] = game PID
/// Returns 0 on success.
/// Gated: caller must hold `Cap::Attestation` (WRITE); cross-PID needs daemon tier.
fn handle_unregister_game(args: &[u64]) -> u64 {
    if args.is_empty() {
        return AC_ERR_BAD_ARGS;
    }
    let game_pid = args[0];
    if let Err(code) = enforce_gate(game_pid, true) {
        return code;
    }

    let mut guard = ANTICHEAT.lock();
    let mgr = match guard.as_mut() {
        Some(m) => m,
        None => return AC_ERR_NOT_INITIALIZED,
    };

    if mgr.game_sessions.remove(&game_pid).is_none() {
        return AC_ERR_NOT_REGISTERED;
    }

    mgr.memory_engine.unregister_process(game_pid);

    // Tear down any attestation sessions for this PID.
    let to_remove: Vec<u64> = mgr
        .attestation
        .active_sessions
        .iter()
        .filter(|(_, s)| s.game_pid == game_pid)
        .map(|(&id, _)| id)
        .collect();
    for id in to_remove {
        mgr.attestation.remove_session(id);
    }

    AC_OK
}

/// SYS_AC_REPORT_VIOLATION
/// args[0] = game PID, args[1] = violation type (as u64), args[2] = address,
/// args[3] = (ignored — kernel-stamped)
///
/// Gated: caller must hold `Cap::Attestation` (WRITE). This is the legitimate
/// anti-cheat-DAEMON path — a daemon reports on a game PID that is NOT itself,
/// which requires the daemon-tier marker (`Cap::Attestation` with GRANT). The
/// gate PRESERVES that path while denying a sandboxed peer from poisoning
/// another PID's violation record.
fn handle_report_violation(args: &[u64]) -> u64 {
    if args.len() < 4 {
        return AC_ERR_BAD_ARGS;
    }
    let game_pid = args[0];
    let vtype = violation_from_u64(args[1]);
    let address = args[2];
    if let Err(code) = enforce_gate(game_pid, true) {
        return code;
    }
    // Kernel clock — the violation record's timestamp must not be caller-fed.
    let timestamp = kernel_now_ms();

    let mut guard = ANTICHEAT.lock();
    let mgr = match guard.as_mut() {
        Some(m) => m,
        None => return AC_ERR_NOT_INITIALIZED,
    };

    let session = match mgr.game_sessions.get_mut(&game_pid) {
        Some(s) => s,
        None => return AC_ERR_NOT_REGISTERED,
    };

    session.integrity.record_violation(IntegrityViolation {
        timestamp,
        violation_type: vtype,
        address,
        details: String::from("reported via syscall"),
    });

    match session.integrity.violation_count() {
        0..=2 => session.status = GameSessionStatus::Suspicious,
        3..=5 => session.status = GameSessionStatus::Flagged,
        _ => session.status = GameSessionStatus::Banned,
    }

    AC_OK
}

/// SYS_AC_QUERY_STATUS
/// args[0] = game PID
/// Returns status as u64 (0=Active, 1=Suspicious, …, 4=Disconnected).
/// Gated: caller must hold `Cap::Attestation` (READ); cross-PID needs daemon tier.
fn handle_query_status(args: &[u64]) -> u64 {
    if args.is_empty() {
        return AC_ERR_BAD_ARGS;
    }
    let game_pid = args[0];
    if let Err(code) = enforce_gate(game_pid, false) {
        return code;
    }

    let guard = ANTICHEAT.lock();
    let mgr = match guard.as_ref() {
        Some(m) => m,
        None => return AC_ERR_NOT_INITIALIZED,
    };

    match mgr.game_sessions.get(&game_pid) {
        Some(s) => s.status as u64,
        None => AC_ERR_NOT_REGISTERED,
    }
}

/// SYS_AC_HEARTBEAT
/// args[0] = game PID, args[1] = (ignored — kernel-stamped)
/// Gated: caller must hold `Cap::Attestation` (WRITE); cross-PID needs daemon tier.
fn handle_heartbeat(args: &[u64]) -> u64 {
    if args.len() < 2 {
        return AC_ERR_BAD_ARGS;
    }
    let game_pid = args[0];
    if let Err(code) = enforce_gate(game_pid, true) {
        return code;
    }
    // Kernel clock — the disconnect deadline must not be caller-controlled.
    let timestamp = kernel_now_ms();

    let mut guard = ANTICHEAT.lock();
    let mgr = match guard.as_mut() {
        Some(m) => m,
        None => return AC_ERR_NOT_INITIALIZED,
    };

    let session = match mgr.game_sessions.get_mut(&game_pid) {
        Some(s) => s,
        None => return AC_ERR_NOT_REGISTERED,
    };

    let deadline = session.last_heartbeat + session.heartbeat_interval_ms * 3;
    if timestamp > deadline && session.status == GameSessionStatus::Active {
        session.status = GameSessionStatus::Disconnected;
        return AC_ERR_EXPIRED;
    }

    session.last_heartbeat = timestamp;
    if session.status == GameSessionStatus::Disconnected {
        session.status = GameSessionStatus::Active;
    }
    AC_OK
}

fn violation_from_u64(v: u64) -> ViolationType {
    match v {
        0 => ViolationType::CodeModification,
        1 => ViolationType::WxViolation,
        2 => ViolationType::DebuggerAttached,
        3 => ViolationType::UnauthorizedMemoryAccess,
        4 => ViolationType::InjectedThread,
        5 => ViolationType::SuspiciousAllocation,
        6 => ViolationType::HookDetected,
        7 => ViolationType::KernelModuleInserted,
        8 => ViolationType::SystemCallHooked,
        9 => ViolationType::TimingAnomaly,
        _ => ViolationType::CodeModification,
    }
}

// ───────────────────────────────────────────────────────────────────────────────
// 5. Game Session Management
// ───────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u64)]
pub enum GameSessionStatus {
    Active = 0,
    Suspicious = 1,
    Flagged = 2,
    Banned = 3,
    Disconnected = 4,
}

#[derive(Debug, Clone)]
pub struct GameSession {
    pub game_pid: u64,
    pub game_name: String,
    pub session_id: u64,
    pub started_at: u64,
    pub anti_cheat_vendor: Option<AntiCheatVendor>,
    pub integrity: ProcessIntegrity,
    pub heartbeat_interval_ms: u64,
    pub last_heartbeat: u64,
    pub status: GameSessionStatus,
}

impl GameSession {
    pub fn is_healthy(&self) -> bool {
        self.status == GameSessionStatus::Active && self.integrity.trusted
    }

    pub fn time_since_heartbeat(&self, now: u64) -> u64 {
        now.saturating_sub(self.last_heartbeat)
    }

    pub fn needs_heartbeat(&self, now: u64) -> bool {
        self.time_since_heartbeat(now) >= self.heartbeat_interval_ms
    }

    pub fn update_integrity_check(&mut self, timestamp: u64) {
        let violations = self.integrity.scan_code_pages(timestamp);
        for v in violations {
            self.integrity.record_violation(v);
        }
        self.integrity.last_check = timestamp;

        if !self.integrity.trusted && self.status == GameSessionStatus::Active {
            self.status = GameSessionStatus::Suspicious;
        }
    }
}

// ───────────────────────────────────────────────────────────────────────────────
// 6. Kernel Self-Integrity
// ───────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KernelAnomaly {
    SyscallTableModified,
    IdtModified,
    KernelTextModified,
    UnknownKernelModule,
    SuspiciousInterrupt,
}

pub struct KernelIntegrity {
    pub syscall_table_hash: [u8; 32],
    pub idt_hash: [u8; 32],
    pub kernel_text_hash: [u8; 32],
    pub last_check: u64,
    pub anomalies: Vec<KernelAnomaly>,
}

impl KernelIntegrity {
    pub fn new() -> Self {
        Self {
            syscall_table_hash: Self::snapshot_syscall_table(),
            idt_hash: Self::snapshot_idt(),
            kernel_text_hash: Self::snapshot_kernel_text(),
            last_check: 0,
            anomalies: Vec::new(),
        }
    }

    pub fn check_all(&self) -> bool {
        self.check_syscall_table() && self.check_idt() && self.check_kernel_text()
    }

    pub fn periodic_scan(&mut self, timestamp: u64) {
        self.anomalies.clear();

        if !self.check_syscall_table() {
            self.anomalies.push(KernelAnomaly::SyscallTableModified);
        }
        if !self.check_idt() {
            self.anomalies.push(KernelAnomaly::IdtModified);
        }
        if !self.check_kernel_text() {
            self.anomalies.push(KernelAnomaly::KernelTextModified);
        }

        self.check_for_unknown_modules();
        self.check_for_suspicious_interrupts();

        self.last_check = timestamp;
    }

    pub fn is_clean(&self) -> bool {
        self.anomalies.is_empty()
    }

    pub fn anomaly_count(&self) -> usize {
        self.anomalies.len()
    }

    fn check_syscall_table(&self) -> bool {
        let current = Self::snapshot_syscall_table();
        current == self.syscall_table_hash
    }

    fn check_idt(&self) -> bool {
        let current = Self::snapshot_idt();
        current == self.idt_hash
    }

    fn check_kernel_text(&self) -> bool {
        let current = Self::snapshot_kernel_text();
        current == self.kernel_text_hash
    }

    fn check_for_unknown_modules(&mut self) {
        // Stub: walk the kernel module list and verify each signature.
        // In AthenaOS the kernel is monolithic with no loadable module support
        // today, so any module present is inherently suspicious.
        let module_count = count_loaded_modules();
        if module_count > 0 {
            self.anomalies.push(KernelAnomaly::UnknownKernelModule);
        }
    }

    fn check_for_suspicious_interrupts(&mut self) {
        // Stub: compare current IDT entry counts against baseline.
        let unexpected = count_unexpected_interrupt_handlers();
        if unexpected > 0 {
            self.anomalies.push(KernelAnomaly::SuspiciousInterrupt);
        }
    }

    /// Hash the SYSCALL entry pointer (IA32_LSTAR MSR = 0xC000_0082, the RIP the
    /// `syscall` instruction jumps to). Redirecting LSTAR to a shim is a classic
    /// syscall-hook technique — a change here flips `check_syscall_table`. (Was a
    /// fixed sentinel that could never detect a redirect.) LSTAR is programmed in
    /// `syscall::init`, long before the anti-cheat baseline is captured.
    fn snapshot_syscall_table() -> [u8; 32] {
        let lstar = unsafe { crate::msr::rdmsr_safe(0xC000_0082).unwrap_or(0) };
        rae_crypto::sha256::sha256(&lstar.to_le_bytes())
    }

    /// Hash the LIVE IDT, read via `SIDT`. A rootkit that hooks an interrupt
    /// vector rewrites that vector's descriptor → the hash changes → `check_idt`
    /// fires. (Was a fixed sentinel — the check compared a constant to itself and
    /// could never detect a hook.)
    fn snapshot_idt() -> [u8; 32] {
        // SIDT stores 10 bytes: u16 limit (= table size - 1) + u64 base.
        let mut idtr = [0u8; 10];
        unsafe {
            core::arch::asm!("sidt [{}]", in(reg) idtr.as_mut_ptr(), options(nostack, preserves_flags));
        }
        let limit = u16::from_le_bytes([idtr[0], idtr[1]]) as usize;
        let base = u64::from_le_bytes([
            idtr[2], idtr[3], idtr[4], idtr[5], idtr[6], idtr[7], idtr[8], idtr[9],
        ]);
        if base == 0 {
            return [0u8; 32];
        }
        // Safe: the IDT is resident kernel memory; `limit + 1` bytes.
        let bytes = unsafe { core::slice::from_raw_parts(base as *const u8, limit + 1) };
        rae_crypto::sha256::sha256(bytes)
    }

    /// Hash a window of the SYSCALL entry code (at the LSTAR target). An inline
    /// hook that patches the syscall prologue changes these bytes → the hash
    /// changes → `check_kernel_text` fires. Kernel `.text` is W^X (read-only,
    /// stable after boot), so this is the highest-value code region to watch
    /// without full `.text` bounds. (Was a fixed sentinel.)
    fn snapshot_kernel_text() -> [u8; 32] {
        let lstar = unsafe { crate::msr::rdmsr_safe(0xC000_0082).unwrap_or(0) };
        if lstar == 0 {
            return [0u8; 32];
        }
        // 256 bytes of the syscall entry point — pure, stable kernel code.
        let bytes = unsafe { core::slice::from_raw_parts(lstar as *const u8, 256) };
        rae_crypto::sha256::sha256(bytes)
    }
}

fn count_loaded_modules() -> usize {
    0
}

fn count_unexpected_interrupt_handlers() -> usize {
    0
}

// ───────────────────────────────────────────────────────────────────────────────
// 7. Global Anti-Cheat Manager
// ───────────────────────────────────────────────────────────────────────────────

pub static ANTICHEAT: Mutex<Option<AntiCheatManager>> = Mutex::new(None);

pub struct AntiCheatManager {
    pub memory_engine: MemoryProtectionEngine,
    pub attestation: AttestationService,
    pub kernel_integrity: KernelIntegrity,
    pub game_sessions: BTreeMap<u64, GameSession>,
    pub global_ban_list: Vec<[u8; 32]>,
    session_counter: u64,
}

impl AntiCheatManager {
    pub fn new() -> Self {
        Self {
            memory_engine: MemoryProtectionEngine::new(WxPolicy::Strict),
            attestation: AttestationService::new(),
            kernel_integrity: KernelIntegrity::new(),
            game_sessions: BTreeMap::new(),
            global_ban_list: Vec::new(),
            session_counter: 0,
        }
    }

    fn next_session_id(&mut self) -> u64 {
        self.session_counter += 1;
        self.session_counter
    }

    pub fn register_game(
        &mut self,
        pid: u64,
        name: String,
        vendor: Option<AntiCheatVendor>,
        heartbeat_ms: u64,
        timestamp: u64,
    ) -> u64 {
        let sid = self.next_session_id();
        let integrity = self.memory_engine.register_process(pid).clone();

        let session = GameSession {
            game_pid: pid,
            game_name: name,
            session_id: sid,
            started_at: timestamp,
            anti_cheat_vendor: vendor,
            integrity,
            heartbeat_interval_ms: heartbeat_ms,
            last_heartbeat: timestamp,
            status: GameSessionStatus::Active,
        };
        self.game_sessions.insert(pid, session);
        sid
    }

    pub fn unregister_game(&mut self, pid: u64) -> bool {
        if self.game_sessions.remove(&pid).is_some() {
            self.memory_engine.unregister_process(pid);
            true
        } else {
            false
        }
    }

    pub fn is_banned(&self, code_hash: &[u8; 32]) -> bool {
        self.global_ban_list.iter().any(|h| h == code_hash)
    }

    pub fn add_to_ban_list(&mut self, hash: [u8; 32]) {
        if !self.is_banned(&hash) {
            self.global_ban_list.push(hash);
        }
    }

    pub fn periodic_scan(&mut self, timestamp: u64) {
        self.kernel_integrity.periodic_scan(timestamp);

        self.memory_engine.full_scan(timestamp);

        let pids: Vec<u64> = self.game_sessions.keys().copied().collect();
        let mut banned_pids: Vec<u64> = Vec::new();
        for &pid in &pids {
            if let Some(session) = self.game_sessions.get(&pid) {
                if self.is_banned(&session.integrity.code_hash) {
                    banned_pids.push(pid);
                }
            }
        }
        for pid in pids {
            if let Some(session) = self.game_sessions.get_mut(&pid) {
                session.update_integrity_check(timestamp);

                if session.needs_heartbeat(timestamp) && session.status == GameSessionStatus::Active
                {
                    let deadline = session.last_heartbeat + session.heartbeat_interval_ms * 3;
                    if timestamp > deadline {
                        session.status = GameSessionStatus::Disconnected;
                    }
                }

                if banned_pids.contains(&pid) {
                    session.status = GameSessionStatus::Banned;
                }
            }
        }

        self.attestation.purge_expired(timestamp);
    }

    pub fn active_game_count(&self) -> usize {
        self.game_sessions
            .values()
            .filter(|s| s.status == GameSessionStatus::Active)
            .count()
    }

    pub fn total_violations(&self) -> usize {
        self.game_sessions
            .values()
            .map(|s| s.integrity.violation_count())
            .sum()
    }

    pub fn get_session_status(&self, pid: u64) -> Option<GameSessionStatus> {
        self.game_sessions.get(&pid).map(|s| s.status)
    }

    pub fn request_attestation(
        &mut self,
        game_pid: u64,
        vendor: AntiCheatVendor,
        timestamp: u64,
    ) -> Option<u64> {
        if !self.game_sessions.contains_key(&game_pid) {
            return None;
        }
        Some(self.attestation.create_session(game_pid, vendor, timestamp))
    }

    pub fn verify_attestation(&mut self, session_id: u64, timestamp: u64) -> bool {
        let game_pid = match self.attestation.get_session(session_id) {
            Some(s) => s.game_pid,
            None => return false,
        };

        let proc_result = self.memory_engine.verify_code_integrity(game_pid);
        let kernel_ok = self.kernel_integrity.check_all();
        self.attestation
            .verify_session(session_id, proc_result, kernel_ok, timestamp)
    }
}

// ───────────────────────────────────────────────────────────────────────────────
// Initialization
// ───────────────────────────────────────────────────────────────────────────────

/// Initialize the anti-cheat subsystem.
///
/// Called during boot after `security::init()` and `tpm::init()` so the
/// secure boot chain is already measured and the TPM is available.
/// Seeds the platform key from the TPM hardware RNG when available.
pub fn init() {
    let mut mgr = AntiCheatManager::new();

    // Seed the attestation HMAC key. Prefer the TPM hardware RNG (binds the key
    // to the platform's hardware entropy); fall back to the kernel CSPRNG
    // (RDRAND-seeded `crypto::getrandom`, live at this boot stage) whenever the
    // TPM is absent or returns short. SECURITY: the key must NEVER stay the
    // all-zero `[u8;32]` default — a publicly-known key lets anyone forge a
    // valid attestation offline, defeating the whole vendor-trust premise. This
    // is the common case on QEMU CI and the Athena KVM no-flash loop (no TPM2).
    let mut seeded_from_tpm = false;
    if let Some(ref mut device) = *crate::tpm::TPM.lock() {
        mgr.attestation.tpm_available = device.is_hardware();
        let random_bytes = device.get_random(32);
        if random_bytes.len() >= 32 {
            mgr.attestation
                .platform_key
                .copy_from_slice(&random_bytes[..32]);
            seeded_from_tpm = mgr.attestation.platform_key != [0u8; 32];
        }
    }
    if !seeded_from_tpm {
        // No usable TPM RNG — derive the key from the kernel CSPRNG so it is
        // still unpredictable and per-boot unique.
        let mut key = [0u8; 32];
        if crate::crypto::getrandom(&mut key).is_ok() && key != [0u8; 32] {
            mgr.attestation.platform_key = key;
        } else {
            // CSPRNG unavailable this early (should not happen post crypto::init):
            // mix the TSC + a fixed domain tag so the key is at least not zero.
            // Logged loudly because attestation strength is degraded.
            let tsc = unsafe { core::arch::x86_64::_rdtsc() };
            for (i, b) in mgr.attestation.platform_key.iter_mut().enumerate() {
                *b = (tsc.rotate_left((i as u32) * 7) as u8) ^ 0xA5 ^ (i as u8);
            }
            crate::serial_println!(
                "[anticheat] WARN: CSPRNG unavailable; attestation key seeded from TSC only (weak)"
            );
        }
    }

    *ANTICHEAT.lock() = Some(mgr);
}

/// `/proc/raeen/anticheat` body — live attestation/integrity manager state.
pub fn dump_text() -> String {
    use core::fmt::Write;
    let mut s = String::new();
    let guard = ANTICHEAT.lock();
    match &*guard {
        Some(mgr) => {
            let _ = writeln!(
                s,
                "anti-cheat: ring-0 attestation API (no kernel AC driver)"
            );
            let _ = writeln!(
                s,
                "wx_policy            = {:?}",
                mgr.memory_engine.wx_policy
            );
            let _ = writeln!(
                s,
                "protected_processes  = {}",
                mgr.memory_engine.protected_processes.len()
            );
            let _ = writeln!(s, "active_games         = {}", mgr.active_game_count());
            let _ = writeln!(s, "total_violations     = {}", mgr.total_violations());
            let _ = writeln!(s, "ban_list_entries     = {}", mgr.global_ban_list.len());
            let _ = writeln!(
                s,
                "tpm_available        = {}",
                mgr.attestation.tpm_available
            );
            let _ = writeln!(
                s,
                "attestation_sessions = {}",
                mgr.attestation.active_sessions.len()
            );
        }
        None => {
            let _ = writeln!(s, "anti-cheat: not initialized");
        }
    }
    s
}

/// R10 boot smoketest — drives the real detection logic through deterministic
/// scenarios. `compute_page_hash`, `read_function_prologue`, and
/// `read_debug_register` are pure stubs (no hardware reads), so every verdict
/// below is reproducible on every boot.
pub fn run_boot_smoketest() {
    // 1. W^X policy: a page mapped writable+executable is flagged at
    //    registration time AND reported by check_wx_violation under Strict.
    let mut eng = MemoryProtectionEngine::new(WxPolicy::Strict);
    let wx_pid = 0xAC01;
    {
        let p = eng.register_process(wx_pid);
        p.register_code_page(CodePageEntry {
            virt_addr: 0x4000,
            phys_addr: 0x4000,
            hash: compute_page_hash(0x4000),
            permissions: PagePermissions {
                readable: true,
                writable: true,
                executable: true,
                user: true,
            },
            writable_at_any_point: true,
        });
    }
    let wx_flagged = eng.check_wx_violation(wx_pid)
        && eng
            .protected_processes
            .get(&wx_pid)
            .map_or(false, |p| p.violation_count() >= 1);

    // 2. Code-integrity: a page whose stored hash matches reads Clean; a page
    //    whose stored hash was tampered reads Tampered; an unknown pid Unknown.
    let clean_pid = 0xAC02;
    {
        let p = eng.register_process(clean_pid);
        p.register_code_page(CodePageEntry {
            virt_addr: 0x8000,
            phys_addr: 0x8000,
            hash: compute_page_hash(0x8000),
            permissions: PagePermissions::kernel_ro_exec(),
            writable_at_any_point: false,
        });
    }
    let clean = eng.verify_code_integrity(clean_pid) == IntegrityResult::Clean;

    let tamper_pid = 0xAC03;
    {
        let p = eng.register_process(tamper_pid);
        p.register_code_page(CodePageEntry {
            virt_addr: 0xC000,
            phys_addr: 0xC000,
            hash: [0xFF; 32], // != compute_page_hash(0xC000)
            permissions: PagePermissions::kernel_ro_exec(),
            writable_at_any_point: false,
        });
    }
    let tamper = eng.verify_code_integrity(tamper_pid) == IntegrityResult::Tampered
        && eng.verify_code_integrity(0xDEAD) == IntegrityResult::Unknown;

    // 3. Stack canary: matches for a registered process, fails for an unknown one.
    let canary = eng.verify_stack_integrity(clean_pid) && !eng.verify_stack_integrity(0xDEAD);

    // 4. Hook detection: a function whose live prologue begins with a JMP (0xE9)
    //    and differs from the recorded prologue is reported; a matching one is not.
    //    read_function_prologue(addr)[0] == addr.to_le_bytes()[0], so an address
    //    ending in 0xE9 forces the hook byte.
    let hook_addr = 0xFFFF_0000_0000_00E9;
    let clean_addr = 0x1234;
    let known = [
        FunctionSignature {
            name: String::from("hooked"),
            address: hook_addr,
            prologue: [0u8; 16],
        },
        FunctionSignature {
            name: String::from("clean"),
            address: clean_addr,
            prologue: read_function_prologue(clean_addr),
        },
    ];
    let hooks = eng.detect_hooks(clean_pid, &known);
    let hook = hooks.len() == 1 && hooks[0].address == hook_addr;

    // 5. Ban list: membership + dedup at the manager level.
    let mut mgr = AntiCheatManager::new();
    let banned = [0x42u8; 32];
    mgr.add_to_ban_list(banned);
    mgr.add_to_ban_list(banned); // dedup
    let ban =
        mgr.is_banned(&banned) && !mgr.is_banned(&[0u8; 32]) && mgr.global_ban_list.len() == 1;

    // 6. Attestation gating: a session is only issued for a registered game.
    let game_pid = 0xBEEF;
    mgr.register_game(
        game_pid,
        String::from("smoketest"),
        Some(AntiCheatVendor::AthGuard),
        1000,
        1,
    );
    let attest = mgr
        .request_attestation(game_pid, AntiCheatVendor::AthGuard, 2)
        .is_some()
        && mgr
            .request_attestation(0xFACE, AntiCheatVendor::AthGuard, 2)
            .is_none()
        && mgr.active_game_count() == 1;

    // 7. Kernel integrity monitor: the IDT / syscall-entry / text-window
    //    snapshots must be REAL (a live SHA-256, not the old fixed sentinels),
    //    STABLE (a fresh baseline re-verified reports clean — no false positive),
    //    and DETECTING (a corrupted baseline is caught).
    let ki = KernelIntegrity::new();
    let ki_clean = ki.check_all(); // baseline == live -> no false positive
                                   // Old placeholder was a fixed [0xBB.., 0x1D, 0x74, ...] sentinel; a real
                                   // hash of the live IDT differs from it and from all-zero (read-failed), and
                                   // the three regions hash to distinct values.
    let old_idt_sentinel = {
        let mut h = [0xBBu8; 32];
        h[0] = 0x1D;
        h[1] = 0x74;
        h
    };
    let real_hashes = ki.idt_hash != [0u8; 32]
        && ki.idt_hash != old_idt_sentinel
        && ki.syscall_table_hash != [0u8; 32]
        && ki.kernel_text_hash != [0u8; 32]
        && ki.idt_hash != ki.syscall_table_hash
        && ki.idt_hash != ki.kernel_text_hash;
    // A corrupted recorded baseline MUST be detected as modified (proves the
    // check compares the real live IDT, not a constant to itself).
    let detects_tamper = {
        let mut ki_bad = KernelIntegrity::new();
        ki_bad.idt_hash = [0xFF; 32];
        !ki_bad.check_idt()
    };
    let integrity = ki_clean && real_hashes && detects_tamper;

    let pass = wx_flagged && clean && tamper && canary && hook && ban && attest && integrity;
    crate::serial_println!(
        "[anticheat] smoketest: wx={} clean={} tamper={} canary={} hook={} ban={} attest={} integrity(real={} stable={} detects={}) -> {}",
        wx_flagged,
        clean,
        tamper,
        canary,
        hook,
        ban,
        attest,
        real_hashes,
        ki_clean,
        detects_tamper,
        if pass { "PASS" } else { "FAIL" }
    );

    run_gate_smoketest();
}

/// R10 boot smoketest for the SEV-3 privilege gate (audit-flagged: the
/// existing anticheat smoketest did NOT exercise the gate). Drives the pure
/// `gate_decision` over hostile and authorized scenarios and asserts the
/// fail-closed verdict. FAIL-able: if any branch decides wrong, it prints FAIL.
fn run_gate_smoketest() {
    let none = AttestAuthority::default();
    let reader = AttestAuthority {
        can_read: true,
        can_write: false,
        is_daemon: false,
    };
    let writer = AttestAuthority {
        can_read: true,
        can_write: true,
        is_daemon: false,
    };
    let daemon = AttestAuthority {
        can_read: true,
        can_write: true,
        is_daemon: true,
    };

    // ── Unauthorized: every one of these MUST be denied (Err == AC_ERR_PERM). ──
    // (a) No Cap::Attestation at all → register (write) denied, even self-PID.
    let d1 = gate_decision(7, 7, none, true).is_err();
    // (b) No Cap → query (read) of own session denied.
    let d2 = gate_decision(7, 7, none, false).is_err();
    // (c) Holds READ-only cap but attempts a WRITE op → denied.
    let d3 = gate_decision(7, 7, reader, true).is_err();
    // (d) Holds a WRITE cap but reaches ACROSS to another PID without daemon
    //     tier → cross-PID spoof denied (the core SEV-3 attack).
    let d4 = gate_decision(7, 99, writer, true).is_err();
    // (e) Same, read side: peer cannot query another PID's status.
    let d5 = gate_decision(7, 99, writer, false).is_err();
    let unauthorized_denied = d1 && d2 && d3 && d4 && d5;

    // ── Authorized: each MUST be allowed (Ok). ──
    // (f) Self-PID write with a WRITE cap (a game attesting itself).
    let a1 = gate_decision(7, 7, writer, true).is_ok();
    // (g) Self-PID read with a READ cap.
    let a2 = gate_decision(7, 7, reader, false).is_ok();
    // (h) Daemon (GRANT) acts cross-PID for write (the legit AC-daemon path).
    let a3 = gate_decision(7, 99, daemon, true).is_ok();
    // (i) Daemon reads another PID's status.
    let a4 = gate_decision(7, 99, daemon, false).is_ok();
    let authorized_allowed = a1 && a2 && a3 && a4;

    let pass = unauthorized_denied && authorized_allowed;
    crate::serial_println!(
        "[anticheat] gate smoketest: unauthorized_denied={} authorized_allowed={} -> {}",
        unauthorized_denied,
        authorized_allowed,
        if pass { "PASS" } else { "FAIL" }
    );
}
