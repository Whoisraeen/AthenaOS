//! AthGuard — capability-based security framework for AthenaOS.
//!
//! Sandbox policy engine, code signing, security audit log, process
//! attestation, and mandatory access control. Attestation is for **body/safety
//! and integrity** — game anti-cheat vendor partnerships are parked (see
//! `docs/PARKED_GAMING.md`).
#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

pub mod attest;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

pub const KB: u64 = 1024;
pub const MB: u64 = 1024 * KB;
pub const GB: u64 = 1024 * MB;

// ---------------------------------------------------------------------------
// 1. Capability types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum Capability {
    FileRead = 0,
    FileWrite = 1,
    NetworkAccess = 2,
    CameraAccess = 3,
    MicAccess = 4,
    GpuAccess = 5,
    ProcessSpawn = 6,
    SystemConfig = 7,
    HardwareAccess = 8,
    FullAccess = 9,
    AudioPlayback = 10,
    AudioCapture = 11,
    IpcSend = 12,
    IpcReceive = 13,
    DebugAttach = 14,
    CryptoKeyAccess = 15,
}

impl Capability {
    const COUNT: usize = 16;

    fn mask(self) -> u32 {
        1u32 << (self as u32)
    }
}

/// Compact bitfield set of [`Capability`] values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CapabilitySet {
    bits: u32,
}

impl CapabilitySet {
    pub const fn empty() -> Self {
        Self { bits: 0 }
    }

    pub const fn all() -> Self {
        Self {
            bits: (1u32 << Capability::COUNT) - 1,
        }
    }

    pub fn grant(&mut self, cap: Capability) {
        self.bits |= cap.mask();
    }

    pub fn revoke(&mut self, cap: Capability) {
        self.bits &= !cap.mask();
    }

    pub fn has(&self, cap: Capability) -> bool {
        self.bits & cap.mask() != 0
    }

    pub fn is_empty(&self) -> bool {
        self.bits == 0
    }

    pub fn union(&self, other: &CapabilitySet) -> CapabilitySet {
        CapabilitySet {
            bits: self.bits | other.bits,
        }
    }

    pub fn intersection(&self, other: &CapabilitySet) -> CapabilitySet {
        CapabilitySet {
            bits: self.bits & other.bits,
        }
    }

    pub fn raw_bits(&self) -> u32 {
        self.bits
    }
}

// ---------------------------------------------------------------------------
// 2. Access modes, port ranges, direction
// ---------------------------------------------------------------------------

/// Permission mode for filesystem, GPU, audio, and other resources.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessMode {
    ReadOnly,
    WriteOnly,
    ReadWrite,
}

impl AccessMode {
    pub fn allows_read(self) -> bool {
        matches!(self, AccessMode::ReadOnly | AccessMode::ReadWrite)
    }

    pub fn allows_write(self) -> bool {
        matches!(self, AccessMode::WriteOnly | AccessMode::ReadWrite)
    }
}

/// Inclusive port range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PortRange(pub u16, pub u16);

impl PortRange {
    pub fn contains(&self, port: u16) -> bool {
        port >= self.0 && port <= self.1
    }

    pub fn single(port: u16) -> Self {
        PortRange(port, port)
    }

    pub fn all() -> Self {
        PortRange(0, 65535)
    }
}

/// Network traffic direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Inbound,
    Outbound,
    Both,
}

impl Direction {
    pub fn matches(self, other: Direction) -> bool {
        matches!(
            (self, other),
            (Direction::Both, _)
                | (_, Direction::Both)
                | (Direction::Inbound, Direction::Inbound)
                | (Direction::Outbound, Direction::Outbound)
        )
    }
}

// ---------------------------------------------------------------------------
// 3. Sandbox policy rules
// ---------------------------------------------------------------------------

/// Filesystem access rule: path prefix plus permitted mode.
#[derive(Debug, Clone)]
pub struct FilesystemRule {
    pub path: String,
    pub mode: AccessMode,
}

/// Network access rule with port range and direction.
#[derive(Debug, Clone)]
pub struct NetworkRule {
    pub port_range: PortRange,
    pub direction: Direction,
    pub allow: bool,
}

/// Device access rule for GPU, audio, camera, etc.
///
/// A raw device claim (the kernel's userspace-driver framework) gates against
/// the SPECIFIC kind of the claimed device — a NIC claim must not be evaluated
/// as a GPU claim. Each kind is a distinct rule key: `check_device` matches
/// `rule.kind == kind` exactly, so a profile that grants one kind (e.g. a game
/// with `allow_gpu(ReadWrite)`) does NOT thereby gain any other kind. A kind
/// with no matching `allow_*` rule is denied by every profile by default
/// (fail-closed).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceKind {
    Gpu,
    Audio,
    Camera,
    Mic,
    /// Network interface (Ethernet, Wi-Fi).
    Nic,
    /// Block storage controller (NVMe, AHCI, virtio-blk).
    Storage,
    /// USB host controller / USB device.
    Usb,
    /// Any other / unclassified device — the most-restrictive default for a
    /// claim whose PCI class maps to no more specific kind.
    Other,
}

#[derive(Debug, Clone)]
pub struct DeviceRule {
    pub kind: DeviceKind,
    pub mode: Option<AccessMode>,
    pub allow: bool,
}

/// Hard resource limits for a sandboxed process.
#[derive(Debug, Clone, Copy)]
pub struct ResourceLimits {
    pub max_memory: u64,
    pub max_cpu_percent: u8,
    pub max_open_files: u32,
    pub max_threads: u32,
    pub max_ipc_channels: u32,
    pub max_network_connections: u32,
}

impl Default for ResourceLimits {
    fn default() -> Self {
        Self {
            max_memory: 256 * MB,
            max_cpu_percent: 25,
            max_open_files: 64,
            max_threads: 8,
            max_ipc_channels: 16,
            max_network_connections: 32,
        }
    }
}

// ---------------------------------------------------------------------------
// 4. SandboxPolicy — declarative permission set
// ---------------------------------------------------------------------------

/// Full sandbox policy combining capabilities, filesystem/network/device
/// rules, and resource limits.
#[derive(Debug, Clone)]
pub struct SandboxPolicy {
    pub capabilities: CapabilitySet,
    pub filesystem_rules: Vec<FilesystemRule>,
    pub network_rules: Vec<NetworkRule>,
    pub device_rules: Vec<DeviceRule>,
    pub resource_limits: ResourceLimits,
    pub allow_process_spawn: bool,
    pub allow_ipc: bool,
    pub mac_label: MacLabel,
}

impl SandboxPolicy {
    pub fn builder() -> SandboxPolicyBuilder {
        SandboxPolicyBuilder::new()
    }

    /// Create from a pre-built profile.
    pub fn from_profile(profile: SandboxProfile) -> Self {
        SandboxPolicyBuilder::from_profile(profile).build()
    }
}

// ---------------------------------------------------------------------------
// 5. SandboxPolicyBuilder — clean builder-pattern API
// ---------------------------------------------------------------------------

pub struct SandboxPolicyBuilder {
    capabilities: CapabilitySet,
    filesystem_rules: Vec<FilesystemRule>,
    network_rules: Vec<NetworkRule>,
    device_rules: Vec<DeviceRule>,
    resource_limits: ResourceLimits,
    allow_process_spawn: bool,
    allow_ipc: bool,
    mac_label: MacLabel,
}

impl SandboxPolicyBuilder {
    pub fn new() -> Self {
        Self {
            capabilities: CapabilitySet::empty(),
            filesystem_rules: Vec::new(),
            network_rules: Vec::new(),
            device_rules: Vec::new(),
            resource_limits: ResourceLimits::default(),
            allow_process_spawn: false,
            allow_ipc: false,
            mac_label: MacLabel::User,
        }
    }

    pub fn from_profile(profile: SandboxProfile) -> Self {
        match profile {
            SandboxProfile::Untrusted => Self::profile_untrusted(),
            SandboxProfile::Sandboxed => Self::profile_sandboxed(),
            SandboxProfile::TrustedApp => Self::profile_trusted_app(),
            SandboxProfile::SystemService => Self::profile_system_service(),
            SandboxProfile::Game => Self::profile_game(),
        }
    }

    // ── Filesystem rules ────────────────────────────────────────────────

    pub fn allow_filesystem(mut self, path: &str, mode: AccessMode) -> Self {
        self.filesystem_rules.push(FilesystemRule {
            path: String::from(path),
            mode,
        });
        match mode {
            AccessMode::ReadOnly => self.capabilities.grant(Capability::FileRead),
            AccessMode::WriteOnly => self.capabilities.grant(Capability::FileWrite),
            AccessMode::ReadWrite => {
                self.capabilities.grant(Capability::FileRead);
                self.capabilities.grant(Capability::FileWrite);
            }
        }
        self
    }

    // ── Network rules ───────────────────────────────────────────────────

    pub fn allow_network(mut self, port_range: PortRange, direction: Direction) -> Self {
        self.capabilities.grant(Capability::NetworkAccess);
        self.network_rules.push(NetworkRule {
            port_range,
            direction,
            allow: true,
        });
        self
    }

    pub fn deny_network(mut self, port_range: PortRange, direction: Direction) -> Self {
        self.network_rules.push(NetworkRule {
            port_range,
            direction,
            allow: false,
        });
        self
    }

    // ── Device rules ────────────────────────────────────────────────────

    pub fn allow_gpu(mut self, mode: AccessMode) -> Self {
        self.capabilities.grant(Capability::GpuAccess);
        self.device_rules.push(DeviceRule {
            kind: DeviceKind::Gpu,
            mode: Some(mode),
            allow: true,
        });
        self
    }

    pub fn deny_gpu(mut self) -> Self {
        self.device_rules.push(DeviceRule {
            kind: DeviceKind::Gpu,
            mode: None,
            allow: false,
        });
        self
    }

    pub fn allow_audio(mut self, mode: AccessMode) -> Self {
        if mode.allows_read() {
            self.capabilities.grant(Capability::AudioCapture);
        }
        if mode.allows_write() {
            self.capabilities.grant(Capability::AudioPlayback);
        }
        self.device_rules.push(DeviceRule {
            kind: DeviceKind::Audio,
            mode: Some(mode),
            allow: true,
        });
        self
    }

    pub fn deny_audio(mut self) -> Self {
        self.device_rules.push(DeviceRule {
            kind: DeviceKind::Audio,
            mode: None,
            allow: false,
        });
        self
    }

    pub fn allow_camera(mut self, mode: AccessMode) -> Self {
        self.capabilities.grant(Capability::CameraAccess);
        self.device_rules.push(DeviceRule {
            kind: DeviceKind::Camera,
            mode: Some(mode),
            allow: true,
        });
        self
    }

    pub fn deny_camera(mut self) -> Self {
        self.device_rules.push(DeviceRule {
            kind: DeviceKind::Camera,
            mode: None,
            allow: false,
        });
        self
    }

    pub fn allow_mic(mut self, mode: AccessMode) -> Self {
        self.capabilities.grant(Capability::MicAccess);
        self.device_rules.push(DeviceRule {
            kind: DeviceKind::Mic,
            mode: Some(mode),
            allow: true,
        });
        self
    }

    pub fn deny_mic(mut self) -> Self {
        self.device_rules.push(DeviceRule {
            kind: DeviceKind::Mic,
            mode: None,
            allow: false,
        });
        self
    }

    // ── Resource limits ─────────────────────────────────────────────────

    pub fn max_memory(mut self, bytes: u64) -> Self {
        self.resource_limits.max_memory = bytes;
        self
    }

    pub fn max_cpu_percent(mut self, pct: u8) -> Self {
        self.resource_limits.max_cpu_percent = pct.min(100);
        self
    }

    pub fn max_threads(mut self, n: u32) -> Self {
        self.resource_limits.max_threads = n;
        self
    }

    pub fn max_open_files(mut self, n: u32) -> Self {
        self.resource_limits.max_open_files = n;
        self
    }

    pub fn max_ipc_channels(mut self, n: u32) -> Self {
        self.resource_limits.max_ipc_channels = n;
        self
    }

    pub fn max_network_connections(mut self, n: u32) -> Self {
        self.resource_limits.max_network_connections = n;
        self
    }

    // ── Misc ────────────────────────────────────────────────────────────

    pub fn allow_process_spawn(mut self) -> Self {
        self.allow_process_spawn = true;
        self.capabilities.grant(Capability::ProcessSpawn);
        self
    }

    pub fn allow_ipc(mut self) -> Self {
        self.allow_ipc = true;
        self.capabilities.grant(Capability::IpcSend);
        self.capabilities.grant(Capability::IpcReceive);
        self
    }

    pub fn grant_capability(mut self, cap: Capability) -> Self {
        self.capabilities.grant(cap);
        self
    }

    pub fn mac_label(mut self, label: MacLabel) -> Self {
        self.mac_label = label;
        self
    }

    // ── Build ───────────────────────────────────────────────────────────

    pub fn build(self) -> SandboxPolicy {
        SandboxPolicy {
            capabilities: self.capabilities,
            filesystem_rules: self.filesystem_rules,
            network_rules: self.network_rules,
            device_rules: self.device_rules,
            resource_limits: self.resource_limits,
            allow_process_spawn: self.allow_process_spawn,
            allow_ipc: self.allow_ipc,
            mac_label: self.mac_label,
        }
    }

    // ── Pre-built profiles ──────────────────────────────────────────────

    fn profile_untrusted() -> Self {
        Self::new()
            .max_memory(64 * MB)
            .max_cpu_percent(10)
            .max_threads(2)
            .max_open_files(16)
            .mac_label(MacLabel::Untrusted)
    }

    fn profile_sandboxed() -> Self {
        Self::new()
            .allow_gpu(AccessMode::ReadOnly)
            .max_memory(256 * MB)
            .max_cpu_percent(25)
            .max_threads(16)
            .max_open_files(64)
            .mac_label(MacLabel::User)
    }

    fn profile_trusted_app() -> Self {
        Self::new()
            .allow_gpu(AccessMode::ReadWrite)
            .allow_audio(AccessMode::ReadWrite)
            .allow_network(PortRange::all(), Direction::Outbound)
            .allow_process_spawn()
            .allow_ipc()
            .max_memory(1 * GB)
            .max_cpu_percent(80)
            .max_threads(64)
            .max_open_files(256)
            .mac_label(MacLabel::User)
    }

    fn profile_system_service() -> Self {
        let mut b = Self::new()
            .allow_gpu(AccessMode::ReadWrite)
            .allow_audio(AccessMode::ReadWrite)
            .allow_network(PortRange::all(), Direction::Both)
            .allow_process_spawn()
            .allow_ipc()
            .max_memory(u64::MAX)
            .max_cpu_percent(100)
            .max_threads(u32::MAX)
            .max_open_files(u32::MAX)
            .mac_label(MacLabel::System);
        b.capabilities = CapabilitySet::all();
        b
    }

    fn profile_game() -> Self {
        Self::new()
            .allow_gpu(AccessMode::ReadWrite)
            .allow_audio(AccessMode::ReadWrite)
            .allow_network(PortRange::all(), Direction::Outbound)
            .allow_ipc()
            .max_memory(8 * GB)
            .max_cpu_percent(95)
            .max_threads(128)
            .max_open_files(512)
            .mac_label(MacLabel::Game)
    }
}

impl Default for SandboxPolicyBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Pre-defined sandbox profiles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxProfile {
    /// Minimal permissions — untrusted/unknown executables.
    Untrusted,
    /// Default sandbox — limited GPU, no network.
    Sandboxed,
    /// Trusted application — broader access.
    TrustedApp,
    /// System service — full access.
    SystemService,
    /// Game process — high GPU/CPU/memory, outbound network.
    Game,
}

// ---------------------------------------------------------------------------
// 6. PolicyEnforcer — validates syscalls against policy
// ---------------------------------------------------------------------------

/// A syscall-level request that the enforcer evaluates.
#[derive(Debug, Clone)]
pub enum SyscallRequest {
    FileOpen { path: String, write: bool },
    NetworkConnect { port: u16, direction: Direction },
    NetworkListen { port: u16 },
    DeviceAccess { kind: DeviceKind, write: bool },
    ProcessSpawn { binary_path: String },
    IpcSend { channel_id: u64 },
    IpcReceive { channel_id: u64 },
    MemoryAllocate { size: u64 },
    ThreadCreate,
    CapabilityRequest { cap: Capability },
}

/// Outcome of a policy enforcement check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecurityDecision {
    Allowed,
    Denied,
}

/// Detail about a policy violation.
#[derive(Debug, Clone)]
pub struct PolicyViolation {
    pub timestamp: u64,
    pub process_id: u64,
    pub request: SyscallRequest,
    pub reason: ViolationReason,
    pub severity: AuditSeverity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViolationReason {
    MissingCapability,
    PathNotAllowed,
    PortNotAllowed,
    DirectionNotAllowed,
    DeviceDenied,
    SpawnDenied,
    IpcDenied,
    ResourceLimitExceeded,
    MacLabelMismatch,
}

/// Disposition after a violation is detected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViolationAction {
    Log,
    Deny,
    Kill,
}

/// Validates syscalls against a [`SandboxPolicy`].
pub struct PolicyEnforcer {
    pub policy: SandboxPolicy,
    pub violation_action: ViolationAction,
    pub violations: Vec<PolicyViolation>,
    pub violation_threshold: usize,
    current_memory: u64,
    current_threads: u32,
    current_open_files: u32,
}

impl PolicyEnforcer {
    pub fn new(policy: SandboxPolicy, action: ViolationAction) -> Self {
        Self {
            policy,
            violation_action: action,
            violations: Vec::new(),
            violation_threshold: 100,
            current_memory: 0,
            current_threads: 1,
            current_open_files: 0,
        }
    }

    /// Check whether `request` is permitted under the current policy.
    pub fn check(
        &mut self,
        request: &SyscallRequest,
        pid: u64,
        timestamp: u64,
    ) -> SecurityDecision {
        let result = match request {
            SyscallRequest::FileOpen { path, write } => self.check_file(path, *write),
            SyscallRequest::NetworkConnect { port, direction } => {
                self.check_network(*port, *direction)
            }
            SyscallRequest::NetworkListen { port } => self.check_network(*port, Direction::Inbound),
            SyscallRequest::DeviceAccess { kind, write } => self.check_device(*kind, *write),
            SyscallRequest::ProcessSpawn { .. } => {
                if self.policy.allow_process_spawn {
                    SecurityDecision::Allowed
                } else {
                    SecurityDecision::Denied
                }
            }
            SyscallRequest::IpcSend { .. } | SyscallRequest::IpcReceive { .. } => {
                if self.policy.allow_ipc {
                    SecurityDecision::Allowed
                } else {
                    SecurityDecision::Denied
                }
            }
            SyscallRequest::MemoryAllocate { size } => {
                if self.current_memory.saturating_add(*size)
                    <= self.policy.resource_limits.max_memory
                {
                    SecurityDecision::Allowed
                } else {
                    SecurityDecision::Denied
                }
            }
            SyscallRequest::ThreadCreate => {
                if self.current_threads < self.policy.resource_limits.max_threads {
                    SecurityDecision::Allowed
                } else {
                    SecurityDecision::Denied
                }
            }
            SyscallRequest::CapabilityRequest { cap } => {
                if self.policy.capabilities.has(*cap) {
                    SecurityDecision::Allowed
                } else {
                    SecurityDecision::Denied
                }
            }
        };

        if result == SecurityDecision::Denied {
            let reason = match request {
                SyscallRequest::FileOpen { .. } => ViolationReason::PathNotAllowed,
                SyscallRequest::NetworkConnect { .. } | SyscallRequest::NetworkListen { .. } => {
                    ViolationReason::PortNotAllowed
                }
                SyscallRequest::DeviceAccess { .. } => ViolationReason::DeviceDenied,
                SyscallRequest::ProcessSpawn { .. } => ViolationReason::SpawnDenied,
                SyscallRequest::IpcSend { .. } | SyscallRequest::IpcReceive { .. } => {
                    ViolationReason::IpcDenied
                }
                SyscallRequest::MemoryAllocate { .. } | SyscallRequest::ThreadCreate => {
                    ViolationReason::ResourceLimitExceeded
                }
                SyscallRequest::CapabilityRequest { .. } => ViolationReason::MissingCapability,
            };
            self.record_violation(pid, timestamp, request.clone(), reason);
        }

        result
    }

    fn check_file(&self, path: &str, write: bool) -> SecurityDecision {
        let cap = if write {
            Capability::FileWrite
        } else {
            Capability::FileRead
        };
        if !self.policy.capabilities.has(cap) {
            return SecurityDecision::Denied;
        }
        if self.policy.filesystem_rules.is_empty() {
            return SecurityDecision::Allowed;
        }
        for rule in &self.policy.filesystem_rules {
            if path.starts_with(rule.path.as_str()) {
                let ok = if write {
                    rule.mode.allows_write()
                } else {
                    rule.mode.allows_read()
                };
                if ok {
                    return SecurityDecision::Allowed;
                }
            }
        }
        SecurityDecision::Denied
    }

    fn check_network(&self, port: u16, direction: Direction) -> SecurityDecision {
        if !self.policy.capabilities.has(Capability::NetworkAccess) {
            return SecurityDecision::Denied;
        }
        // Explicit deny rules take precedence.
        for rule in &self.policy.network_rules {
            if !rule.allow && rule.port_range.contains(port) && rule.direction.matches(direction) {
                return SecurityDecision::Denied;
            }
        }
        // Then check allow rules.
        for rule in &self.policy.network_rules {
            if rule.allow && rule.port_range.contains(port) && rule.direction.matches(direction) {
                return SecurityDecision::Allowed;
            }
        }
        // No network rules defined but capability present → allow.
        if self.policy.network_rules.is_empty() {
            return SecurityDecision::Allowed;
        }
        SecurityDecision::Denied
    }

    fn check_device(&self, kind: DeviceKind, write: bool) -> SecurityDecision {
        // Explicit deny rules take precedence.
        for rule in &self.policy.device_rules {
            if rule.kind == kind && !rule.allow {
                return SecurityDecision::Denied;
            }
        }
        for rule in &self.policy.device_rules {
            if rule.kind == kind && rule.allow {
                if let Some(mode) = rule.mode {
                    let ok = if write {
                        mode.allows_write()
                    } else {
                        mode.allows_read()
                    };
                    if ok {
                        return SecurityDecision::Allowed;
                    }
                }
            }
        }
        SecurityDecision::Denied
    }

    fn record_violation(
        &mut self,
        pid: u64,
        timestamp: u64,
        request: SyscallRequest,
        reason: ViolationReason,
    ) {
        let severity = match reason {
            ViolationReason::MissingCapability | ViolationReason::MacLabelMismatch => {
                AuditSeverity::Alert
            }
            ViolationReason::ResourceLimitExceeded => AuditSeverity::Warning,
            _ => AuditSeverity::Warning,
        };
        self.violations.push(PolicyViolation {
            timestamp,
            process_id: pid,
            request,
            reason,
            severity,
        });
    }

    /// Whether the process should be killed based on accumulated violations.
    pub fn should_kill(&self) -> bool {
        self.violation_action == ViolationAction::Kill
            && self.violations.len() >= self.violation_threshold
    }

    pub fn violation_count(&self) -> usize {
        self.violations.len()
    }

    pub fn track_allocation(&mut self, size: u64) {
        self.current_memory = self.current_memory.saturating_add(size);
    }

    pub fn track_deallocation(&mut self, size: u64) {
        self.current_memory = self.current_memory.saturating_sub(size);
    }

    pub fn track_thread_create(&mut self) {
        self.current_threads = self.current_threads.saturating_add(1);
    }

    pub fn track_thread_exit(&mut self) {
        self.current_threads = self.current_threads.saturating_sub(1);
    }

    pub fn track_file_open(&mut self) {
        self.current_open_files = self.current_open_files.saturating_add(1);
    }

    pub fn track_file_close(&mut self) {
        self.current_open_files = self.current_open_files.saturating_sub(1);
    }
}

// ---------------------------------------------------------------------------
// 7. Runtime permission request
// ---------------------------------------------------------------------------

/// An app can ask for additional capabilities at runtime (prompts the user).
#[derive(Debug, Clone)]
pub struct RuntimePermissionRequest {
    pub process_id: u64,
    pub requested_cap: Capability,
    pub justification: String,
    pub timestamp: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionResponse {
    Granted,
    DeniedByUser,
    DeniedByPolicy,
    AlreadyHeld,
}

/// Evaluate a runtime permission request against the process's trust level
/// and existing policy.
pub fn evaluate_permission_request(
    request: &RuntimePermissionRequest,
    enforcer: &PolicyEnforcer,
    identity: Option<&SigningIdentity>,
) -> PermissionResponse {
    if enforcer.policy.capabilities.has(request.requested_cap) {
        return PermissionResponse::AlreadyHeld;
    }

    let min_trust = match request.requested_cap {
        Capability::SystemConfig
        | Capability::HardwareAccess
        | Capability::FullAccess
        | Capability::DebugAttach => TrustLevel::System,
        Capability::ProcessSpawn | Capability::FileWrite | Capability::CryptoKeyAccess => {
            TrustLevel::Verified
        }
        _ => TrustLevel::SelfSigned,
    };

    let trust = identity
        .map(|id| id.trust_level)
        .unwrap_or(TrustLevel::Unsigned);

    if trust < min_trust {
        return PermissionResponse::DeniedByPolicy;
    }

    // In a real implementation this would trigger a user-facing prompt.
    // For now, auto-grant if trust level is sufficient.
    PermissionResponse::Granted
}

// ---------------------------------------------------------------------------
// 8. Code signing — Ed25519 and RSA-4096
// ---------------------------------------------------------------------------

/// Cryptographic algorithm used for signing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SigningAlgorithm {
    Ed25519,
    Rsa4096,
}

impl SigningAlgorithm {
    pub fn public_key_size(self) -> usize {
        match self {
            SigningAlgorithm::Ed25519 => 32,
            SigningAlgorithm::Rsa4096 => 512,
        }
    }

    pub fn signature_size(self) -> usize {
        match self {
            SigningAlgorithm::Ed25519 => 64,
            SigningAlgorithm::Rsa4096 => 512,
        }
    }
}

/// Level of trust assigned to a code signer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum TrustLevel {
    Unsigned = 0,
    SelfSigned = 1,
    Verified = 2,
    System = 3,
}

/// Developer certificate containing a public key, issuer, and validity.
#[derive(Debug, Clone)]
pub struct SigningIdentity {
    pub name: String,
    pub public_key: Vec<u8>,
    pub issuer: String,
    pub serial: u64,
    pub not_before: u64,
    pub not_after: u64,
    pub algorithm: SigningAlgorithm,
    pub trust_level: TrustLevel,
    pub fingerprint: [u8; 32],
}

impl SigningIdentity {
    pub fn is_expired(&self, now: u64) -> bool {
        now < self.not_before || now > self.not_after
    }

    pub fn is_valid_at(&self, now: u64) -> bool {
        now >= self.not_before && now <= self.not_after
    }

    /// Compute the fingerprint from the public key (SHA-256 stub).
    pub fn compute_fingerprint(public_key: &[u8]) -> [u8; 32] {
        simple_sha256(public_key)
    }
}

/// Signature attached to a code artifact.
#[derive(Debug, Clone)]
pub struct CodeSignature {
    pub algorithm: SigningAlgorithm,
    pub binary_hash: [u8; 32],
    pub signature_bytes: Vec<u8>,
    pub signer: SigningIdentity,
    pub timestamp: u64,
}

/// Certificate in the signing chain: Root CA -> Intermediate -> Leaf.
#[derive(Debug, Clone)]
pub struct Certificate {
    pub identity: SigningIdentity,
    pub issuer_fingerprint: [u8; 32],
    pub is_ca: bool,
    pub signature: Vec<u8>,
}

impl Certificate {
    pub fn is_self_signed(&self) -> bool {
        self.identity.fingerprint == self.issuer_fingerprint
    }

    /// The canonical "to-be-signed" bytes an issuer signs to certify this cert.
    /// Covers every issuer-bound field (identity + issuer link + CA flag) in a
    /// fixed, length-prefixed order so a signature is unambiguous. Deliberately
    /// EXCLUDES `signature` (the thing being produced) and `trust_level` (the
    /// verifier's local policy, never signed by the issuer).
    pub fn signed_data(&self) -> Vec<u8> {
        let id = &self.identity;
        let mut b = Vec::new();
        let field = |bytes: &[u8], out: &mut Vec<u8>| {
            out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
            out.extend_from_slice(bytes);
        };
        field(id.name.as_bytes(), &mut b);
        field(&id.public_key, &mut b);
        field(id.issuer.as_bytes(), &mut b);
        b.extend_from_slice(&id.serial.to_le_bytes());
        b.extend_from_slice(&id.not_before.to_le_bytes());
        b.extend_from_slice(&id.not_after.to_le_bytes());
        b.push(id.algorithm as u8);
        b.extend_from_slice(&id.fingerprint);
        b.extend_from_slice(&self.issuer_fingerprint);
        b.push(self.is_ca as u8);
        b
    }

    /// Sign this certificate with the issuer's Ed25519 seed (its 32-byte secret).
    /// Sets `signature` to `Ed25519(issuer_seed, signed_data())`. For a
    /// self-signed root, `issuer_seed` is the root's own secret.
    pub fn sign(&mut self, issuer_seed: &[u8; 32]) {
        let msg = self.signed_data();
        self.signature = ath_crypto::ed25519::sign(issuer_seed, &msg).to_vec();
    }

    /// Verify this cert's signature under `issuer_public_key` (32-byte Ed25519).
    /// Fail-closed on any wrong-sized key/signature — never accepts an
    /// unverifiable link.
    fn signature_valid_under(&self, issuer_public_key: &[u8]) -> bool {
        let pk: [u8; 32] = match issuer_public_key.try_into() {
            Ok(p) => p,
            Err(_) => return false,
        };
        let sig: [u8; 64] = match self.signature.as_slice().try_into() {
            Ok(s) => s,
            Err(_) => return false,
        };
        ath_crypto::ed25519::verify(&pk, &self.signed_data(), &sig)
    }
}

/// A chain of certificates from leaf to root.
#[derive(Debug, Clone)]
pub struct CertificateChain {
    pub certificates: Vec<Certificate>,
}

impl CertificateChain {
    pub fn new() -> Self {
        Self {
            certificates: Vec::new(),
        }
    }

    pub fn push(&mut self, cert: Certificate) {
        self.certificates.push(cert);
    }

    pub fn len(&self) -> usize {
        self.certificates.len()
    }

    pub fn is_empty(&self) -> bool {
        self.certificates.is_empty()
    }

    /// Verify the chain leaf→root: name-chaining, validity, and — the part the
    /// former implementation skipped — that each parent CRYPTOGRAPHICALLY signed
    /// its child, that the root's self-signature is valid, and that the root is
    /// one of `trusted_anchor_fps` (a PINNED trust anchor, not merely any
    /// self-signed cert). Without both signature verification and the anchor
    /// pin, an attacker forges a leaf→intermediate→self-signed-root chain with
    /// matching fingerprints and it "validates" — a code-signing trust bypass.
    pub fn verify_chain(&self, now: u64, trusted_anchor_fps: &[[u8; 32]]) -> ChainVerifyResult {
        if self.certificates.is_empty() {
            return ChainVerifyResult::EmptyChain;
        }

        for cert in &self.certificates {
            if cert.identity.is_expired(now) {
                return ChainVerifyResult::Expired;
            }
        }

        for i in 0..self.certificates.len() - 1 {
            let child = &self.certificates[i];
            let parent = &self.certificates[i + 1];
            if child.issuer_fingerprint != parent.identity.fingerprint {
                return ChainVerifyResult::BrokenChain;
            }
            if !parent.is_ca {
                return ChainVerifyResult::NonCaIssuer;
            }
            // The parent must have actually signed the child — not just share a
            // fingerprint. This is the check that stops a forged link.
            if !child.signature_valid_under(&parent.identity.public_key) {
                return ChainVerifyResult::InvalidSignature;
            }
        }

        let root = &self.certificates[self.certificates.len() - 1];
        if !root.is_self_signed() {
            return ChainVerifyResult::UntrustedRoot;
        }
        // The root's self-signature must be cryptographically valid…
        if !root.signature_valid_under(&root.identity.public_key) {
            return ChainVerifyResult::InvalidSignature;
        }
        // …AND the root must be a pinned trust anchor. Any self-signed cert is
        // otherwise a valid "root", so an attacker-minted root would pass.
        if !trusted_anchor_fps.contains(&root.identity.fingerprint) {
            return ChainVerifyResult::UntrustedRoot;
        }

        ChainVerifyResult::Valid
    }
}

impl Default for CertificateChain {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChainVerifyResult {
    Valid,
    EmptyChain,
    Expired,
    BrokenChain,
    NonCaIssuer,
    UntrustedRoot,
    /// A certificate's signature did not verify under its issuer's public key
    /// (a forged link), or a key/signature was the wrong size for Ed25519.
    InvalidSignature,
}

/// Certificate Revocation List.
#[derive(Debug, Clone)]
pub struct CertificateRevocationList {
    pub revoked_serials: Vec<u64>,
    pub revoked_fingerprints: Vec<[u8; 32]>,
    pub last_updated: u64,
}

impl CertificateRevocationList {
    pub fn new() -> Self {
        Self {
            revoked_serials: Vec::new(),
            revoked_fingerprints: Vec::new(),
            last_updated: 0,
        }
    }

    pub fn revoke_serial(&mut self, serial: u64) {
        if !self.revoked_serials.contains(&serial) {
            self.revoked_serials.push(serial);
        }
    }

    pub fn revoke_fingerprint(&mut self, fp: [u8; 32]) {
        if !self.revoked_fingerprints.contains(&fp) {
            self.revoked_fingerprints.push(fp);
        }
    }

    pub fn is_revoked_serial(&self, serial: u64) -> bool {
        self.revoked_serials.contains(&serial)
    }

    pub fn is_revoked_fingerprint(&self, fp: &[u8; 32]) -> bool {
        self.revoked_fingerprints.contains(fp)
    }

    pub fn is_revoked(&self, identity: &SigningIdentity) -> bool {
        self.is_revoked_serial(identity.serial)
            || self.is_revoked_fingerprint(&identity.fingerprint)
    }
}

impl Default for CertificateRevocationList {
    fn default() -> Self {
        Self::new()
    }
}

/// Result of signature verification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SigningResult {
    Valid,
    Expired,
    Revoked,
    Tampered,
    Untrusted,
    InvalidSignature,
    InvalidChain,
}

/// Verify a code signature against the binary's expected hash.
///
/// The structural checks (size, non-zero, expiry, CRL, trust level) are
/// PRE-FILTERS. The load-bearing step is the real Ed25519 verification of the
/// signature over the 32-byte binary hash: the signer signs `binary_hash` with
/// its secret key, and we verify that signature against `signer.public_key`.
/// (This function previously returned `Valid` for ANY correctly-sized non-zero
/// blob — a complete code-signing bypass: an attacker could attach a random
/// signature over the target's own hash and be trusted. The cryptographic
/// check below closes that.)
pub fn verify_signature(
    binary_hash: &[u8; 32],
    signature: &CodeSignature,
    crl: &CertificateRevocationList,
    now: u64,
) -> SigningResult {
    // Digest must match the binary we're checking.
    if signature.binary_hash != *binary_hash {
        return SigningResult::Tampered;
    }

    // Signature blob size must be correct for the algorithm.
    if signature.signature_bytes.len() != signature.algorithm.signature_size() {
        return SigningResult::InvalidSignature;
    }

    // Signature bytes must not be all-zero.
    if signature.signature_bytes.iter().all(|&b| b == 0) {
        return SigningResult::InvalidSignature;
    }

    // Public key size must match the algorithm.
    if signature.signer.public_key.len() != signature.algorithm.public_key_size() {
        return SigningResult::InvalidSignature;
    }

    // Check expiry.
    if signature.signer.is_expired(now) {
        return SigningResult::Expired;
    }

    // Check CRL.
    if crl.is_revoked(&signature.signer) {
        return SigningResult::Revoked;
    }

    // Check trust level.
    if signature.signer.trust_level == TrustLevel::Unsigned {
        return SigningResult::Untrusted;
    }

    // Real cryptographic verification — everything above is only a pre-filter.
    match signature.algorithm {
        SigningAlgorithm::Ed25519 => {
            // The size checks above guarantee 32-byte key / 64-byte sig for
            // Ed25519; the fallible conversions are belt-and-suspenders.
            let public_key: [u8; 32] = match signature.signer.public_key.as_slice().try_into() {
                Ok(k) => k,
                Err(_) => return SigningResult::InvalidSignature,
            };
            let sig: [u8; 64] = match signature.signature_bytes.as_slice().try_into() {
                Ok(s) => s,
                Err(_) => return SigningResult::InvalidSignature,
            };
            if !ath_crypto::ed25519::verify(&public_key, binary_hash, &sig) {
                return SigningResult::InvalidSignature;
            }
        }
        SigningAlgorithm::Rsa4096 => {
            // ath_crypto ships no RSA verifier, so an RSA-4096 signature cannot
            // be cryptographically checked here. Fail CLOSED rather than accept
            // it unverified (accepting = the same bypass). Nothing in the tree
            // currently produces RSA-signed binaries; AthenaOS code signing is
            // Ed25519 (see keys/ + `athsign`).
            return SigningResult::InvalidSignature;
        }
    }

    SigningResult::Valid
}

/// Full verification including certificate chain.
pub fn verify_signature_with_chain(
    binary_hash: &[u8; 32],
    signature: &CodeSignature,
    chain: &CertificateChain,
    crl: &CertificateRevocationList,
    trusted_anchor_fps: &[[u8; 32]],
    now: u64,
) -> SigningResult {
    let base = verify_signature(binary_hash, signature, crl, now);
    if base != SigningResult::Valid {
        return base;
    }

    match chain.verify_chain(now, trusted_anchor_fps) {
        ChainVerifyResult::Valid => SigningResult::Valid,
        ChainVerifyResult::Expired => SigningResult::Expired,
        _ => SigningResult::InvalidChain,
    }
}

// ---------------------------------------------------------------------------
// 9. Security audit log — ring buffer
// ---------------------------------------------------------------------------

/// Severity level for audit events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AuditSeverity {
    Info = 0,
    Warning = 1,
    Alert = 2,
    Critical = 3,
}

/// Classification of auditable security events.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditEventKind {
    PolicyViolation,
    CapabilityGrant,
    CapabilityRevoke,
    AuthAttempt,
    SignatureCheck,
    TpmOperation,
    FileAccess,
    NetworkAccess,
    ProcessSpawn,
    ProcessExit,
    MacViolation,
    SandboxCreated,
    SandboxDestroyed,
    PermissionRequest,
    CrlUpdate,
    AttestationRequest,
    AttestationVerified,
    BruteForceDetected,
    PrivilegeEscalation,
}

/// Single entry in the security audit log.
#[derive(Debug, Clone)]
pub struct AuditEvent {
    pub timestamp: u64,
    pub event_type: AuditEventKind,
    pub severity: AuditSeverity,
    pub subject_pid: u64,
    pub subject_name: String,
    pub object: String,
    pub action: String,
    pub result: SecurityDecision,
    pub details: String,
}

/// Ring-buffer security audit log with configurable capacity.
pub struct AuditLog {
    buffer: Vec<Option<AuditEvent>>,
    capacity: usize,
    write_pos: usize,
    total_events: u64,
}

impl AuditLog {
    pub fn new(capacity: usize) -> Self {
        let cap = if capacity == 0 { 1024 } else { capacity };
        let mut buffer = Vec::with_capacity(cap);
        for _ in 0..cap {
            buffer.push(None);
        }
        Self {
            buffer,
            capacity: cap,
            write_pos: 0,
            total_events: 0,
        }
    }

    /// Record an event, overwriting the oldest if the buffer is full.
    pub fn record(&mut self, event: AuditEvent) {
        self.buffer[self.write_pos] = Some(event);
        self.write_pos = (self.write_pos + 1) % self.capacity;
        self.total_events += 1;
    }

    pub fn record_simple(
        &mut self,
        timestamp: u64,
        kind: AuditEventKind,
        severity: AuditSeverity,
        pid: u64,
        result: SecurityDecision,
    ) {
        self.record(AuditEvent {
            timestamp,
            event_type: kind,
            severity,
            subject_pid: pid,
            subject_name: String::new(),
            object: String::new(),
            action: String::new(),
            result,
            details: String::new(),
        });
    }

    /// Total events ever recorded (including overwritten).
    pub fn total_events(&self) -> u64 {
        self.total_events
    }

    /// Number of events currently stored.
    pub fn len(&self) -> usize {
        let count = self.total_events as usize;
        if count > self.capacity {
            self.capacity
        } else {
            count
        }
    }

    pub fn is_empty(&self) -> bool {
        self.total_events == 0
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Iterate stored events in chronological order.
    pub fn iter(&self) -> AuditLogIter<'_> {
        let count = self.len();
        let start = if self.total_events as usize > self.capacity {
            self.write_pos
        } else {
            0
        };
        AuditLogIter {
            log: self,
            pos: start,
            remaining: count,
        }
    }

    /// Return all events matching the given kind.
    pub fn query_by_kind(&self, kind: AuditEventKind) -> Vec<&AuditEvent> {
        self.iter().filter(|e| e.event_type == kind).collect()
    }

    /// Return all events at or above the given severity.
    pub fn query_by_severity(&self, min_severity: AuditSeverity) -> Vec<&AuditEvent> {
        self.iter().filter(|e| e.severity >= min_severity).collect()
    }

    /// Return all events for a given process.
    pub fn query_by_pid(&self, pid: u64) -> Vec<&AuditEvent> {
        self.iter().filter(|e| e.subject_pid == pid).collect()
    }

    /// Return events within a time range [start, end].
    pub fn query_by_time_range(&self, start: u64, end: u64) -> Vec<&AuditEvent> {
        self.iter()
            .filter(|e| e.timestamp >= start && e.timestamp <= end)
            .collect()
    }

    /// Compound query: filter by kind, severity, and pid simultaneously.
    pub fn query(
        &self,
        kind: Option<AuditEventKind>,
        min_severity: Option<AuditSeverity>,
        pid: Option<u64>,
        time_start: Option<u64>,
        time_end: Option<u64>,
    ) -> Vec<&AuditEvent> {
        self.iter()
            .filter(|e| {
                kind.map_or(true, |k| e.event_type == k)
                    && min_severity.map_or(true, |s| e.severity >= s)
                    && pid.map_or(true, |p| e.subject_pid == p)
                    && time_start.map_or(true, |t| e.timestamp >= t)
                    && time_end.map_or(true, |t| e.timestamp <= t)
            })
            .collect()
    }
}

impl Default for AuditLog {
    fn default() -> Self {
        Self::new(4096)
    }
}

/// Iterator over events in chronological order.
pub struct AuditLogIter<'a> {
    log: &'a AuditLog,
    pos: usize,
    remaining: usize,
}

impl<'a> Iterator for AuditLogIter<'a> {
    type Item = &'a AuditEvent;

    fn next(&mut self) -> Option<Self::Item> {
        while self.remaining > 0 {
            let idx = self.pos % self.log.capacity;
            self.pos += 1;
            self.remaining -= 1;
            if let Some(ref event) = self.log.buffer[idx] {
                return Some(event);
            }
        }
        None
    }
}

// ---------------------------------------------------------------------------
// 10. Alert rules — detect patterns in audit events
// ---------------------------------------------------------------------------

/// An alert rule that triggers when a threshold is exceeded within a window.
#[derive(Debug, Clone)]
pub struct AlertRule {
    pub name: String,
    pub event_kind: AuditEventKind,
    pub min_severity: AuditSeverity,
    pub threshold_count: usize,
    pub window_seconds: u64,
}

/// Alert that has fired.
#[derive(Debug, Clone)]
pub struct FiredAlert {
    pub rule_name: String,
    pub timestamp: u64,
    pub event_count: usize,
    pub subject_pid: Option<u64>,
}

/// Evaluate alert rules against the audit log.
pub fn evaluate_alerts(log: &AuditLog, rules: &[AlertRule], now: u64) -> Vec<FiredAlert> {
    let mut fired = Vec::new();

    for rule in rules {
        let window_start = now.saturating_sub(rule.window_seconds);
        let matching: Vec<&AuditEvent> = log
            .iter()
            .filter(|e| {
                e.event_type == rule.event_kind
                    && e.severity >= rule.min_severity
                    && e.timestamp >= window_start
            })
            .collect();

        if matching.len() >= rule.threshold_count {
            let pid = matching.first().map(|e| e.subject_pid);
            fired.push(FiredAlert {
                rule_name: rule.name.clone(),
                timestamp: now,
                event_count: matching.len(),
                subject_pid: pid,
            });
        }
    }

    fired
}

/// Pre-built alert rule: brute-force detection (5 auth failures in 60s).
pub fn alert_rule_brute_force() -> AlertRule {
    AlertRule {
        name: String::from("brute_force_detection"),
        event_kind: AuditEventKind::AuthAttempt,
        min_severity: AuditSeverity::Warning,
        threshold_count: 5,
        window_seconds: 60,
    }
}

/// Pre-built alert rule: privilege escalation (3 denied cap requests in 30s).
pub fn alert_rule_privilege_escalation() -> AlertRule {
    AlertRule {
        name: String::from("privilege_escalation_attempt"),
        event_kind: AuditEventKind::CapabilityGrant,
        min_severity: AuditSeverity::Alert,
        threshold_count: 3,
        window_seconds: 30,
    }
}

/// Pre-built alert rule: sandbox violations (10 in 120s).
pub fn alert_rule_sandbox_flood() -> AlertRule {
    AlertRule {
        name: String::from("sandbox_violation_flood"),
        event_kind: AuditEventKind::PolicyViolation,
        min_severity: AuditSeverity::Warning,
        threshold_count: 10,
        window_seconds: 120,
    }
}

// ---------------------------------------------------------------------------
// 11. Process attestation API
// ---------------------------------------------------------------------------

/// What the verifier wants to know about a process.
#[derive(Debug, Clone)]
pub struct AttestationRequest {
    pub process_id: u64,
    pub nonce: [u8; 32],
    pub require_boot_chain: bool,
    pub require_code_hash: bool,
    pub require_wx_status: bool,
    pub require_loaded_modules: bool,
    pub require_sandbox_policy: bool,
}

impl AttestationRequest {
    pub fn new(pid: u64, nonce: [u8; 32]) -> Self {
        Self {
            process_id: pid,
            nonce,
            require_boot_chain: true,
            require_code_hash: true,
            require_wx_status: true,
            require_loaded_modules: true,
            require_sandbox_policy: true,
        }
    }
}

/// Loaded module descriptor (for attestation responses).
#[derive(Debug, Clone)]
pub struct LoadedModule {
    pub name: String,
    pub base_address: u64,
    pub size: u64,
    pub hash: [u8; 32],
    pub signed: bool,
}

/// Signed attestation response blob.
#[derive(Debug, Clone)]
pub struct AttestationResponse {
    pub process_id: u64,
    pub nonce: [u8; 32],
    pub timestamp: u64,
    pub binary_hash: [u8; 32],
    pub boot_chain_measurements: Vec<[u8; 32]>,
    pub sandbox_policy_hash: [u8; 32],
    pub wx_clean: bool,
    pub loaded_modules: Vec<LoadedModule>,
    pub memory_integrity_ok: bool,
    pub platform_signature: [u8; 64],
}

impl AttestationResponse {
    /// Serialize to a byte format compatible with EAC/BattlEye expectations.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(512);

        buf.extend_from_slice(b"RAET"); // AthenaOS aTtestation magic
        buf.extend_from_slice(&1u32.to_le_bytes()); // version

        buf.extend_from_slice(&self.process_id.to_le_bytes());
        buf.extend_from_slice(&self.nonce);
        buf.extend_from_slice(&self.timestamp.to_le_bytes());
        buf.extend_from_slice(&self.binary_hash);

        buf.extend_from_slice(&(self.boot_chain_measurements.len() as u32).to_le_bytes());
        for m in &self.boot_chain_measurements {
            buf.extend_from_slice(m);
        }

        buf.extend_from_slice(&self.sandbox_policy_hash);
        buf.push(if self.wx_clean { 1 } else { 0 });
        buf.push(if self.memory_integrity_ok { 1 } else { 0 });

        buf.extend_from_slice(&(self.loaded_modules.len() as u32).to_le_bytes());
        for module in &self.loaded_modules {
            let name_bytes = module.name.as_bytes();
            buf.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
            buf.extend_from_slice(name_bytes);
            buf.extend_from_slice(&module.base_address.to_le_bytes());
            buf.extend_from_slice(&module.size.to_le_bytes());
            buf.extend_from_slice(&module.hash);
            buf.push(if module.signed { 1 } else { 0 });
        }

        buf.extend_from_slice(&self.platform_signature);
        buf
    }

    /// Verify the response against the original request nonce.
    pub fn verify_nonce(&self, expected_nonce: &[u8; 32]) -> bool {
        self.nonce == *expected_nonce
    }
}

/// Build an attestation response for a process.
pub fn create_attestation(
    request: &AttestationRequest,
    binary_hash: [u8; 32],
    boot_measurements: &[[u8; 32]],
    loaded_modules: &[LoadedModule],
    wx_clean: bool,
    memory_ok: bool,
    policy: &SandboxPolicy,
    platform_key: &[u8; 32],
    timestamp: u64,
) -> AttestationResponse {
    let sandbox_hash = hash_sandbox_policy(policy);
    let signature = sign_attestation(
        request,
        &binary_hash,
        &sandbox_hash,
        wx_clean,
        memory_ok,
        platform_key,
        timestamp,
    );

    AttestationResponse {
        process_id: request.process_id,
        nonce: request.nonce,
        timestamp,
        binary_hash,
        boot_chain_measurements: boot_measurements.to_vec(),
        sandbox_policy_hash: sandbox_hash,
        wx_clean,
        loaded_modules: loaded_modules.to_vec(),
        memory_integrity_ok: memory_ok,
        platform_signature: signature,
    }
}

fn hash_sandbox_policy(policy: &SandboxPolicy) -> [u8; 32] {
    let mut data = Vec::with_capacity(64);
    data.extend_from_slice(&policy.capabilities.raw_bits().to_le_bytes());
    data.extend_from_slice(&policy.resource_limits.max_memory.to_le_bytes());
    data.push(policy.resource_limits.max_cpu_percent);
    data.extend_from_slice(&policy.resource_limits.max_threads.to_le_bytes());
    data.push(if policy.allow_process_spawn { 1 } else { 0 });
    data.push(if policy.allow_ipc { 1 } else { 0 });
    data.push(policy.mac_label.as_u8());
    simple_sha256(&data)
}

fn sign_attestation(
    request: &AttestationRequest,
    binary_hash: &[u8; 32],
    sandbox_hash: &[u8; 32],
    wx_clean: bool,
    memory_ok: bool,
    platform_key: &[u8; 32],
    timestamp: u64,
) -> [u8; 64] {
    let mut msg = Vec::with_capacity(128);
    msg.extend_from_slice(&request.nonce);
    msg.extend_from_slice(&request.process_id.to_le_bytes());
    msg.extend_from_slice(binary_hash);
    msg.extend_from_slice(sandbox_hash);
    msg.push(if wx_clean { 1 } else { 0 });
    msg.push(if memory_ok { 1 } else { 0 });
    msg.extend_from_slice(&timestamp.to_le_bytes());
    msg.extend_from_slice(platform_key);

    let hash = simple_sha256(&msg);
    let mut sig = [0u8; 64];
    sig[..32].copy_from_slice(&hash);

    let mut second = Vec::with_capacity(64);
    second.extend_from_slice(&hash);
    second.extend_from_slice(platform_key);
    let hash2 = simple_sha256(&second);
    sig[32..].copy_from_slice(&hash2);

    sig
}

// ---------------------------------------------------------------------------
// 12. Mandatory Access Control
// ---------------------------------------------------------------------------

/// Security label attached to processes, files, and IPC channels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MacLabel {
    System,
    User,
    Untrusted,
    Game,
    Network,
    Custom(u8),
}

impl MacLabel {
    pub fn as_u8(self) -> u8 {
        match self {
            MacLabel::System => 0,
            MacLabel::User => 1,
            MacLabel::Untrusted => 2,
            MacLabel::Game => 3,
            MacLabel::Network => 4,
            MacLabel::Custom(v) => v.wrapping_add(5),
        }
    }

    pub fn from_u8(v: u8) -> Self {
        match v {
            0 => MacLabel::System,
            1 => MacLabel::User,
            2 => MacLabel::Untrusted,
            3 => MacLabel::Game,
            4 => MacLabel::Network,
            n => MacLabel::Custom(n.wrapping_sub(5)),
        }
    }

    /// Dominance: System > User > Game > Network > Untrusted.
    pub fn dominance_level(self) -> u8 {
        match self {
            MacLabel::System => 4,
            MacLabel::User => 3,
            MacLabel::Game => 2,
            MacLabel::Network => 1,
            MacLabel::Untrusted => 0,
            MacLabel::Custom(_) => 1,
        }
    }

    /// Whether `self` dominates `other` (can access resources at or below).
    pub fn dominates(self, other: MacLabel) -> bool {
        self.dominance_level() >= other.dominance_level()
    }
}

/// A single MAC access rule.
#[derive(Debug, Clone)]
pub struct MacRule {
    pub subject_label: MacLabel,
    pub object_label: MacLabel,
    pub allowed_access: AccessMode,
}

/// MAC policy containing a set of rules.
pub struct MacPolicy {
    rules: Vec<MacRule>,
    default_deny: bool,
}

impl MacPolicy {
    pub fn new(default_deny: bool) -> Self {
        Self {
            rules: Vec::new(),
            default_deny,
        }
    }

    /// Pre-built default MAC policy for AthenaOS.
    pub fn default_policy() -> Self {
        let mut policy = Self::new(true);

        // System can access everything.
        policy.add_rule(MacRule {
            subject_label: MacLabel::System,
            object_label: MacLabel::System,
            allowed_access: AccessMode::ReadWrite,
        });
        policy.add_rule(MacRule {
            subject_label: MacLabel::System,
            object_label: MacLabel::User,
            allowed_access: AccessMode::ReadWrite,
        });
        policy.add_rule(MacRule {
            subject_label: MacLabel::System,
            object_label: MacLabel::Game,
            allowed_access: AccessMode::ReadWrite,
        });
        policy.add_rule(MacRule {
            subject_label: MacLabel::System,
            object_label: MacLabel::Network,
            allowed_access: AccessMode::ReadWrite,
        });
        policy.add_rule(MacRule {
            subject_label: MacLabel::System,
            object_label: MacLabel::Untrusted,
            allowed_access: AccessMode::ReadWrite,
        });

        // User can read/write own and read game/network.
        policy.add_rule(MacRule {
            subject_label: MacLabel::User,
            object_label: MacLabel::User,
            allowed_access: AccessMode::ReadWrite,
        });
        policy.add_rule(MacRule {
            subject_label: MacLabel::User,
            object_label: MacLabel::Game,
            allowed_access: AccessMode::ReadOnly,
        });
        policy.add_rule(MacRule {
            subject_label: MacLabel::User,
            object_label: MacLabel::Network,
            allowed_access: AccessMode::ReadOnly,
        });

        // Game can access own data and read network.
        policy.add_rule(MacRule {
            subject_label: MacLabel::Game,
            object_label: MacLabel::Game,
            allowed_access: AccessMode::ReadWrite,
        });
        policy.add_rule(MacRule {
            subject_label: MacLabel::Game,
            object_label: MacLabel::Network,
            allowed_access: AccessMode::ReadWrite,
        });

        // Network-labeled processes access own resources only.
        policy.add_rule(MacRule {
            subject_label: MacLabel::Network,
            object_label: MacLabel::Network,
            allowed_access: AccessMode::ReadWrite,
        });

        // Untrusted: read own only.
        policy.add_rule(MacRule {
            subject_label: MacLabel::Untrusted,
            object_label: MacLabel::Untrusted,
            allowed_access: AccessMode::ReadOnly,
        });

        policy
    }

    pub fn add_rule(&mut self, rule: MacRule) {
        self.rules.push(rule);
    }

    pub fn remove_rules_for(&mut self, subject: MacLabel) {
        self.rules.retain(|r| r.subject_label != subject);
    }

    /// Check whether a subject with `subject_label` can access an object
    /// with `object_label` in the given mode.  MAC runs **before** the
    /// capability check.
    pub fn check(
        &self,
        subject_label: MacLabel,
        object_label: MacLabel,
        write: bool,
    ) -> SecurityDecision {
        for rule in &self.rules {
            if rule.subject_label == subject_label && rule.object_label == object_label {
                let ok = if write {
                    rule.allowed_access.allows_write()
                } else {
                    rule.allowed_access.allows_read()
                };
                if ok {
                    return SecurityDecision::Allowed;
                }
            }
        }

        if self.default_deny {
            SecurityDecision::Denied
        } else {
            SecurityDecision::Allowed
        }
    }

    pub fn rule_count(&self) -> usize {
        self.rules.len()
    }
}

impl Default for MacPolicy {
    fn default() -> Self {
        Self::default_policy()
    }
}

/// Labeled object — files, IPC channels, etc.
#[derive(Debug, Clone)]
pub struct LabeledObject {
    pub id: u64,
    pub label: MacLabel,
    pub name: String,
}

/// Registry of labeled objects for MAC enforcement.
pub struct MacLabelRegistry {
    objects: Vec<LabeledObject>,
}

impl MacLabelRegistry {
    pub fn new() -> Self {
        Self {
            objects: Vec::new(),
        }
    }

    pub fn register(&mut self, id: u64, label: MacLabel, name: String) {
        if !self.objects.iter().any(|o| o.id == id) {
            self.objects.push(LabeledObject { id, label, name });
        }
    }

    pub fn get_label(&self, id: u64) -> Option<MacLabel> {
        self.objects.iter().find(|o| o.id == id).map(|o| o.label)
    }

    pub fn set_label(&mut self, id: u64, label: MacLabel) {
        if let Some(obj) = self.objects.iter_mut().find(|o| o.id == id) {
            obj.label = label;
        }
    }

    pub fn unregister(&mut self, id: u64) {
        self.objects.retain(|o| o.id != id);
    }

    pub fn objects_with_label(&self, label: MacLabel) -> Vec<&LabeledObject> {
        self.objects.iter().filter(|o| o.label == label).collect()
    }

    pub fn len(&self) -> usize {
        self.objects.len()
    }

    pub fn is_empty(&self) -> bool {
        self.objects.is_empty()
    }
}

impl Default for MacLabelRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// 13. Security context — per-process security state
// ---------------------------------------------------------------------------

/// Per-process security context that combines sandbox policy, MAC label,
/// code signing identity, audit trail, and attestation state.
pub struct SecurityContext {
    pub process_id: u64,
    pub enforcer: PolicyEnforcer,
    pub identity: Option<SigningIdentity>,
    pub mac_label: MacLabel,
    pub audit_log: AuditLog,
    pub trust_level: TrustLevel,
    pub attestation_nonce: Option<[u8; 32]>,
    pub violation_count: u64,
}

impl SecurityContext {
    pub fn new(process_id: u64, policy: SandboxPolicy) -> Self {
        let mac_label = policy.mac_label;
        Self {
            process_id,
            enforcer: PolicyEnforcer::new(policy, ViolationAction::Deny),
            identity: None,
            mac_label,
            audit_log: AuditLog::new(1024),
            trust_level: TrustLevel::Unsigned,
            attestation_nonce: None,
            violation_count: 0,
        }
    }

    pub fn with_identity(mut self, identity: SigningIdentity) -> Self {
        self.trust_level = identity.trust_level;
        self.identity = Some(identity);
        self
    }

    /// Full access check: MAC → Capability → Policy.
    pub fn check_access(
        &mut self,
        request: &SyscallRequest,
        mac_policy: &MacPolicy,
        object_label: MacLabel,
        timestamp: u64,
    ) -> SecurityDecision {
        // 1. MAC check runs first.
        let write = match request {
            SyscallRequest::FileOpen { write, .. } => *write,
            SyscallRequest::DeviceAccess { write, .. } => *write,
            _ => false,
        };

        if mac_policy.check(self.mac_label, object_label, write) == SecurityDecision::Denied {
            self.audit_log.record_simple(
                timestamp,
                AuditEventKind::MacViolation,
                AuditSeverity::Alert,
                self.process_id,
                SecurityDecision::Denied,
            );
            self.violation_count += 1;
            return SecurityDecision::Denied;
        }

        // 2. Policy enforcer check (includes capability check).
        let result = self.enforcer.check(request, self.process_id, timestamp);

        if result == SecurityDecision::Denied {
            self.audit_log.record_simple(
                timestamp,
                AuditEventKind::PolicyViolation,
                AuditSeverity::Warning,
                self.process_id,
                SecurityDecision::Denied,
            );
            self.violation_count += 1;
        }

        result
    }

    /// Check a file access against the full security stack.
    pub fn check_file_access(
        &mut self,
        path: &str,
        write: bool,
        mac_policy: &MacPolicy,
        file_label: MacLabel,
        timestamp: u64,
    ) -> SecurityDecision {
        let request = SyscallRequest::FileOpen {
            path: String::from(path),
            write,
        };
        self.check_access(&request, mac_policy, file_label, timestamp)
    }

    /// Check a network access against the full security stack.
    pub fn check_network_access(
        &mut self,
        port: u16,
        direction: Direction,
        mac_policy: &MacPolicy,
        timestamp: u64,
    ) -> SecurityDecision {
        let request = SyscallRequest::NetworkConnect { port, direction };
        self.check_access(&request, mac_policy, MacLabel::Network, timestamp)
    }

    /// Request an additional capability at runtime.
    pub fn request_permission(
        &mut self,
        cap: Capability,
        justification: &str,
        timestamp: u64,
    ) -> PermissionResponse {
        let request = RuntimePermissionRequest {
            process_id: self.process_id,
            requested_cap: cap,
            justification: String::from(justification),
            timestamp,
        };
        let response =
            evaluate_permission_request(&request, &self.enforcer, self.identity.as_ref());

        let decision = match response {
            PermissionResponse::Granted | PermissionResponse::AlreadyHeld => {
                SecurityDecision::Allowed
            }
            _ => SecurityDecision::Denied,
        };

        self.audit_log.record(AuditEvent {
            timestamp,
            event_type: AuditEventKind::PermissionRequest,
            severity: if decision == SecurityDecision::Allowed {
                AuditSeverity::Info
            } else {
                AuditSeverity::Warning
            },
            subject_pid: self.process_id,
            subject_name: String::new(),
            object: String::new(),
            action: String::from(justification),
            result: decision,
            details: String::new(),
        });

        if response == PermissionResponse::Granted {
            self.enforcer.policy.capabilities.grant(cap);
        }

        response
    }

    /// Whether the process should be terminated due to excessive violations.
    pub fn should_terminate(&self) -> bool {
        self.enforcer.should_kill()
            || (self.trust_level == TrustLevel::Unsigned && self.violation_count > 10)
    }

    /// Generate an attestation for this process.
    pub fn create_attestation(
        &self,
        nonce: [u8; 32],
        boot_measurements: &[[u8; 32]],
        loaded_modules: &[LoadedModule],
        wx_clean: bool,
        memory_ok: bool,
        platform_key: &[u8; 32],
        timestamp: u64,
    ) -> AttestationResponse {
        let binary_hash = self
            .identity
            .as_ref()
            .map(|id| id.fingerprint)
            .unwrap_or([0u8; 32]);

        let request = AttestationRequest::new(self.process_id, nonce);
        create_attestation(
            &request,
            binary_hash,
            boot_measurements,
            loaded_modules,
            wx_clean,
            memory_ok,
            &self.enforcer.policy,
            platform_key,
            timestamp,
        )
    }
}

// ---------------------------------------------------------------------------
// 14. Security manager — system-wide security state
// ---------------------------------------------------------------------------

/// System-wide security manager coordinating all subsystems.
pub struct SecurityManager {
    pub mac_policy: MacPolicy,
    pub label_registry: MacLabelRegistry,
    pub crl: CertificateRevocationList,
    pub global_audit_log: AuditLog,
    pub alert_rules: Vec<AlertRule>,
    pub trusted_root_cas: Vec<Certificate>,
    pub contexts: Vec<SecurityContext>,
}

impl SecurityManager {
    pub fn new() -> Self {
        let alert_rules = alloc::vec![
            alert_rule_brute_force(),
            alert_rule_privilege_escalation(),
            alert_rule_sandbox_flood(),
        ];

        Self {
            mac_policy: MacPolicy::default_policy(),
            label_registry: MacLabelRegistry::new(),
            crl: CertificateRevocationList::new(),
            global_audit_log: AuditLog::new(8192),
            alert_rules,
            trusted_root_cas: Vec::new(),
            contexts: Vec::new(),
        }
    }

    /// Create a new security context for a process.
    pub fn create_context(
        &mut self,
        pid: u64,
        policy: SandboxPolicy,
        identity: Option<SigningIdentity>,
    ) -> usize {
        let label = policy.mac_label;
        let mut ctx = SecurityContext::new(pid, policy);
        if let Some(id) = identity {
            ctx = ctx.with_identity(id);
        }
        self.contexts.push(ctx);
        self.global_audit_log.record_simple(
            0,
            AuditEventKind::SandboxCreated,
            AuditSeverity::Info,
            pid,
            SecurityDecision::Allowed,
        );
        self.label_registry.register(pid, label, String::new());
        self.contexts.len() - 1
    }

    /// Destroy a process's security context.
    pub fn destroy_context(&mut self, pid: u64) {
        self.contexts.retain(|c| c.process_id != pid);
        self.label_registry.unregister(pid);
        self.global_audit_log.record_simple(
            0,
            AuditEventKind::SandboxDestroyed,
            AuditSeverity::Info,
            pid,
            SecurityDecision::Allowed,
        );
    }

    pub fn get_context(&self, pid: u64) -> Option<&SecurityContext> {
        self.contexts.iter().find(|c| c.process_id == pid)
    }

    pub fn get_context_mut(&mut self, pid: u64) -> Option<&mut SecurityContext> {
        self.contexts.iter_mut().find(|c| c.process_id == pid)
    }

    /// Verify a binary's signature before execution.
    pub fn verify_binary(
        &mut self,
        binary_hash: &[u8; 32],
        signature: &CodeSignature,
        chain: Option<&CertificateChain>,
        now: u64,
    ) -> SigningResult {
        let result = if let Some(c) = chain {
            // The pinned trust anchors are the fingerprints of the roots the
            // administrator explicitly added via `add_root_ca` — a chain must
            // terminate at one of these, not at any self-signed cert.
            let anchors: Vec<[u8; 32]> = self
                .trusted_root_cas
                .iter()
                .map(|c| c.identity.fingerprint)
                .collect();
            verify_signature_with_chain(binary_hash, signature, c, &self.crl, &anchors, now)
        } else {
            verify_signature(binary_hash, signature, &self.crl, now)
        };

        let severity = match result {
            SigningResult::Valid => AuditSeverity::Info,
            SigningResult::Tampered | SigningResult::Revoked => AuditSeverity::Critical,
            _ => AuditSeverity::Warning,
        };

        let decision = if result == SigningResult::Valid {
            SecurityDecision::Allowed
        } else {
            SecurityDecision::Denied
        };

        self.global_audit_log.record(AuditEvent {
            timestamp: now,
            event_type: AuditEventKind::SignatureCheck,
            severity,
            subject_pid: 0,
            subject_name: signature.signer.name.clone(),
            object: String::new(),
            action: String::new(),
            result: decision,
            details: String::new(),
        });

        result
    }

    /// Add a trusted root CA.
    pub fn add_root_ca(&mut self, cert: Certificate) {
        self.trusted_root_cas.push(cert);
    }

    /// Check and fire alerts based on the global audit log.
    pub fn check_alerts(&self, now: u64) -> Vec<FiredAlert> {
        evaluate_alerts(&self.global_audit_log, &self.alert_rules, now)
    }

    /// Run periodic maintenance: check alerts, clean up dead contexts.
    pub fn periodic_scan(&mut self, now: u64) -> Vec<FiredAlert> {
        let mut pids_to_kill = Vec::new();
        for ctx in &self.contexts {
            if ctx.should_terminate() {
                pids_to_kill.push(ctx.process_id);
            }
        }

        for pid in &pids_to_kill {
            self.global_audit_log.record_simple(
                now,
                AuditEventKind::PolicyViolation,
                AuditSeverity::Critical,
                *pid,
                SecurityDecision::Denied,
            );
        }

        self.check_alerts(now)
    }
}

impl Default for SecurityManager {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// 15. Utility: minimal SHA-256 (for no_std environments without crypto deps)
// ---------------------------------------------------------------------------

/// Simplified SHA-256 for hashing within the AthGuard component.
/// Not cryptographically rigorous — the kernel's `crypto` module provides
/// the real implementation.  This exists so the component crate compiles
/// independently.
fn simple_sha256(data: &[u8]) -> [u8; 32] {
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];

    #[rustfmt::skip]
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5,
        0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
        0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3,
        0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
        0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc,
        0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
        0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
        0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13,
        0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
        0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3,
        0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
        0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5,
        0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208,
        0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
    ];

    let bit_len = (data.len() as u64) * 8;
    let mut padded = Vec::from(data);
    padded.push(0x80);
    while (padded.len() % 64) != 56 {
        padded.push(0x00);
    }
    padded.extend_from_slice(&bit_len.to_be_bytes());

    for chunk in padded.chunks_exact(64) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([
                chunk[i * 4],
                chunk[i * 4 + 1],
                chunk[i * 4 + 2],
                chunk[i * 4 + 3],
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
                .wrapping_add(K[i])
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

    let mut out = [0u8; 32];
    for (i, val) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&val.to_be_bytes());
    }
    out
}

#[cfg(test)]
mod codesign_verify_tests {
    //! FAIL-able host KAT for `verify_signature` (the code-signing gate).
    //!
    //! Regression fence for the auth-bypass where the function returned `Valid`
    //! for any correctly-sized non-zero signature blob. These tests sign a real
    //! binary hash with Ed25519 and assert that a genuine signature is accepted
    //! while a forged / wrong-key / tampered one is rejected. If the crypto
    //! check is ever removed again, cases 2/4/5 flip to `Valid` and FAIL.
    use super::*;

    fn signer_for(pubkey: [u8; 32], trust: TrustLevel) -> SigningIdentity {
        SigningIdentity {
            name: String::from("test-signer"),
            public_key: pubkey.to_vec(),
            issuer: String::from("test-ca"),
            serial: 1,
            not_before: 0,
            not_after: 1_000_000,
            algorithm: SigningAlgorithm::Ed25519,
            trust_level: trust,
            fingerprint: SigningIdentity::compute_fingerprint(&pubkey),
        }
    }

    #[test]
    fn ed25519_codesign_accepts_genuine_rejects_forged() {
        let seed = [7u8; 32];
        let pubkey = ath_crypto::ed25519::derive_public_key(&seed);
        let binary_hash = [0x33u8; 32];
        let sig = ath_crypto::ed25519::sign(&seed, &binary_hash);

        let crl = CertificateRevocationList::new();
        let now = 100u64;

        let good = CodeSignature {
            algorithm: SigningAlgorithm::Ed25519,
            binary_hash,
            signature_bytes: sig.to_vec(),
            signer: signer_for(pubkey, TrustLevel::Verified),
            timestamp: now,
        };

        // 1. Genuine signature over the hash -> Valid.
        assert_eq!(
            verify_signature(&binary_hash, &good, &crl, now),
            SigningResult::Valid
        );

        // 2. Forged signature (flip one byte) -> InvalidSignature. THE fence:
        //    this is exactly what the old structural-only path wrongly accepted.
        let mut forged = good.clone();
        forged.signature_bytes[10] ^= 0xFF;
        assert_eq!(
            verify_signature(&binary_hash, &forged, &crl, now),
            SigningResult::InvalidSignature
        );

        // 3. Correct signature but a DIFFERENT binary is presented -> Tampered.
        let other_hash = [0x44u8; 32];
        assert_eq!(
            verify_signature(&other_hash, &good, &crl, now),
            SigningResult::Tampered
        );

        // 4. Well-formed signature, WRONG signer public key -> InvalidSignature.
        let wrong_pk = ath_crypto::ed25519::derive_public_key(&[9u8; 32]);
        let mut wrong_signer = good.clone();
        wrong_signer.signer = signer_for(wrong_pk, TrustLevel::Verified);
        assert_eq!(
            verify_signature(&binary_hash, &wrong_signer, &crl, now),
            SigningResult::InvalidSignature
        );

        // 5. All-zero signature -> InvalidSignature (structural + crypto agree).
        let mut zero = good.clone();
        zero.signature_bytes = alloc::vec![0u8; 64];
        assert_eq!(
            verify_signature(&binary_hash, &zero, &crl, now),
            SigningResult::InvalidSignature
        );

        // 6. RSA-4096 is unverifiable here -> fail CLOSED (InvalidSignature).
        let mut rsa = good.clone();
        rsa.algorithm = SigningAlgorithm::Rsa4096;
        rsa.signature_bytes = alloc::vec![0x01u8; 512];
        rsa.signer.algorithm = SigningAlgorithm::Rsa4096;
        rsa.signer.public_key = alloc::vec![0x01u8; 512];
        assert_eq!(
            verify_signature(&binary_hash, &rsa, &crl, now),
            SigningResult::InvalidSignature
        );
    }

    // Build a cert for `key_seed`'s public key, claiming `issuer_fp` as issuer
    // and CA-ness `is_ca`, signed by `issuer_seed` (== key_seed for a root).
    fn make_cert(
        key_seed: &[u8; 32],
        issuer_fp: [u8; 32],
        is_ca: bool,
        issuer_seed: &[u8; 32],
    ) -> Certificate {
        let pubkey = ath_crypto::ed25519::derive_public_key(key_seed);
        let mut c = Certificate {
            identity: signer_for(pubkey, TrustLevel::Verified),
            issuer_fingerprint: issuer_fp,
            is_ca,
            signature: Vec::new(),
        };
        c.sign(issuer_seed);
        c
    }

    #[test]
    fn chain_requires_real_signatures_and_a_pinned_anchor() {
        let (root_s, inter_s, leaf_s) = ([1u8; 32], [2u8; 32], [3u8; 32]);
        let root_fp =
            SigningIdentity::compute_fingerprint(&ath_crypto::ed25519::derive_public_key(&root_s));
        let inter_fp =
            SigningIdentity::compute_fingerprint(&ath_crypto::ed25519::derive_public_key(&inter_s));

        // Root self-signs; intermediate signed by root; leaf signed by inter.
        let root = make_cert(&root_s, root_fp, true, &root_s);
        let inter = make_cert(&inter_s, root_fp, true, &root_s);
        let leaf = make_cert(&leaf_s, inter_fp, false, &inter_s);

        let mut chain = CertificateChain::new();
        chain.push(leaf.clone());
        chain.push(inter.clone());
        chain.push(root.clone());
        let anchors = [root_fp];
        let now = 100u64;

        // 1. A genuine chain to a pinned root -> Valid.
        assert_eq!(chain.verify_chain(now, &anchors), ChainVerifyResult::Valid);

        // 2. Forge a link signature -> InvalidSignature. THE fence: the old
        //    structural-only check accepted this.
        let mut forged = chain.clone();
        forged.certificates[0].signature[10] ^= 0xFF;
        assert_eq!(
            forged.verify_chain(now, &anchors),
            ChainVerifyResult::InvalidSignature
        );

        // 3. A perfectly self-consistent chain whose root is NOT a pinned anchor
        //    (an attacker-minted root) -> UntrustedRoot. This is the code-signing
        //    trust bypass the anchor pin closes.
        assert_eq!(
            chain.verify_chain(now, &[]),
            ChainVerifyResult::UntrustedRoot
        );

        // 4. Intermediate NOT actually signed by the root (signed by a stranger),
        //    even though fingerprints chain up -> InvalidSignature.
        let stranger = [9u8; 32];
        let mut mitm = CertificateChain::new();
        mitm.push(leaf.clone());
        mitm.push(make_cert(&inter_s, root_fp, true, &stranger)); // wrong issuer sig
        mitm.push(root.clone());
        assert_eq!(
            mitm.verify_chain(now, &anchors),
            ChainVerifyResult::InvalidSignature
        );

        // 5. Expired cert in the chain -> Expired (not_after is 1_000_000).
        assert_eq!(
            chain.verify_chain(2_000_000, &anchors),
            ChainVerifyResult::Expired
        );

        // 6. Empty chain -> EmptyChain.
        assert_eq!(
            CertificateChain::new().verify_chain(now, &anchors),
            ChainVerifyResult::EmptyChain
        );
    }
}
