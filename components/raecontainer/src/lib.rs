#![no_std]

extern crate alloc;

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

// ─── Error Types ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContainerError {
    NotFound,
    AlreadyExists,
    InvalidState,
    InvalidConfig,
    NamespaceError,
    MountError,
    NetworkError,
    ResourceLimitError,
    SecurityError,
    ImageError,
    RegistryError,
    VolumeError,
    HookError,
    CheckpointError,
    RestoreError,
    PermissionDenied,
    IoError,
    OomKilled,
    SignalError,
    LockFailed,
    InternalError,
}

pub type Result<T> = core::result::Result<T, ContainerError>;

// ─── OCI Runtime Spec ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContainerState {
    Creating,
    Created,
    Running,
    Stopped,
    Paused,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContainerId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VolumeId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NetworkId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LayerId(pub u64);

#[derive(Debug, Clone)]
pub struct OciSpec {
    pub oci_version: String,
    pub root: OciRoot,
    pub mounts: Vec<OciMount>,
    pub process: OciProcess,
    pub hostname: String,
    pub domainname: String,
    pub annotations: BTreeMap<String, String>,
    pub hooks: OciHooks,
    pub linux: OciLinux,
}

impl OciSpec {
    pub fn new() -> Self {
        Self {
            oci_version: String::from("1.0.2"),
            root: OciRoot::new(),
            mounts: Vec::new(),
            process: OciProcess::new(),
            hostname: String::new(),
            domainname: String::new(),
            annotations: BTreeMap::new(),
            hooks: OciHooks::new(),
            linux: OciLinux::new(),
        }
    }

    pub fn validate(&self) -> Result<()> {
        if self.oci_version.is_empty() {
            return Err(ContainerError::InvalidConfig);
        }
        if self.root.path.is_empty() {
            return Err(ContainerError::InvalidConfig);
        }
        if self.process.args.is_empty() {
            return Err(ContainerError::InvalidConfig);
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct OciRoot {
    pub path: String,
    pub readonly: bool,
}

impl OciRoot {
    pub fn new() -> Self {
        Self {
            path: String::from("rootfs"),
            readonly: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct OciMount {
    pub destination: String,
    pub mount_type: String,
    pub source: String,
    pub options: Vec<String>,
}

impl OciMount {
    pub fn new(destination: String, mount_type: String, source: String) -> Self {
        Self {
            destination,
            mount_type,
            source,
            options: Vec::new(),
        }
    }

    pub fn default_mounts() -> Vec<Self> {
        let mut mounts = Vec::new();
        mounts.push(Self::new(
            String::from("/proc"),
            String::from("proc"),
            String::from("proc"),
        ));
        mounts.push(Self::new(
            String::from("/dev"),
            String::from("tmpfs"),
            String::from("tmpfs"),
        ));
        mounts.push(Self::new(
            String::from("/dev/pts"),
            String::from("devpts"),
            String::from("devpts"),
        ));
        mounts.push(Self::new(
            String::from("/dev/shm"),
            String::from("tmpfs"),
            String::from("shm"),
        ));
        mounts.push(Self::new(
            String::from("/dev/mqueue"),
            String::from("mqueue"),
            String::from("mqueue"),
        ));
        mounts.push(Self::new(
            String::from("/sys"),
            String::from("sysfs"),
            String::from("sysfs"),
        ));
        mounts.push(Self::new(
            String::from("/sys/fs/cgroup"),
            String::from("cgroup"),
            String::from("cgroup"),
        ));
        mounts
    }
}

// ─── OCI Process ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct OciProcess {
    pub terminal: bool,
    pub console_size: Option<ConsoleSize>,
    pub cwd: String,
    pub env: Vec<String>,
    pub args: Vec<String>,
    pub user: OciUser,
    pub capabilities: Capabilities,
    pub rlimits: Vec<Rlimit>,
    pub no_new_privileges: bool,
    pub apparmor_profile: String,
    pub selinux_label: String,
    pub oom_score_adj: i32,
}

impl OciProcess {
    pub fn new() -> Self {
        Self {
            terminal: false,
            console_size: None,
            cwd: String::from("/"),
            env: Vec::new(),
            args: Vec::new(),
            user: OciUser::new(),
            capabilities: Capabilities::new(),
            rlimits: Vec::new(),
            no_new_privileges: true,
            apparmor_profile: String::new(),
            selinux_label: String::new(),
            oom_score_adj: 0,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ConsoleSize {
    pub height: u32,
    pub width: u32,
}

#[derive(Debug, Clone)]
pub struct OciUser {
    pub uid: u32,
    pub gid: u32,
    pub umask: u32,
    pub additional_gids: Vec<u32>,
}

impl OciUser {
    pub fn new() -> Self {
        Self {
            uid: 0,
            gid: 0,
            umask: 0o022,
            additional_gids: Vec::new(),
        }
    }
}

// ─── Capabilities ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u64)]
pub enum Capability {
    Chown = 0,
    DacOverride = 1,
    DacReadSearch = 2,
    Fowner = 3,
    Fsetid = 4,
    Kill = 5,
    Setgid = 6,
    Setuid = 7,
    Setpcap = 8,
    LinuxImmutable = 9,
    NetBindService = 10,
    NetBroadcast = 11,
    NetAdmin = 12,
    NetRaw = 13,
    IpcLock = 14,
    IpcOwner = 15,
    SysModule = 16,
    SysRawio = 17,
    SysChroot = 18,
    SysPtrace = 19,
    SysPacket = 20,
    SysAdmin = 21,
    SysBoot = 22,
    SysNice = 23,
    SysResource = 24,
    SysTime = 25,
    SysTtyConfig = 26,
    Mknod = 27,
    Lease = 28,
    AuditWrite = 29,
    AuditControl = 30,
    Setfcap = 31,
    MacOverride = 32,
    MacAdmin = 33,
    Syslog = 34,
    WakeAlarm = 35,
    BlockSuspend = 36,
    AuditRead = 37,
    Perfmon = 38,
    Bpf = 39,
    CheckpointRestore = 40,
}

#[derive(Debug, Clone)]
pub struct Capabilities {
    pub bounding: Vec<Capability>,
    pub effective: Vec<Capability>,
    pub inheritable: Vec<Capability>,
    pub permitted: Vec<Capability>,
    pub ambient: Vec<Capability>,
}

impl Capabilities {
    pub fn new() -> Self {
        Self {
            bounding: Vec::new(),
            effective: Vec::new(),
            inheritable: Vec::new(),
            permitted: Vec::new(),
            ambient: Vec::new(),
        }
    }

    pub fn default_caps() -> Self {
        let defaults = alloc::vec![
            Capability::Chown,
            Capability::DacOverride,
            Capability::Fsetid,
            Capability::Fowner,
            Capability::Mknod,
            Capability::NetRaw,
            Capability::Setgid,
            Capability::Setuid,
            Capability::Setfcap,
            Capability::Setpcap,
            Capability::NetBindService,
            Capability::SysChroot,
            Capability::Kill,
            Capability::AuditWrite,
        ];
        Self {
            bounding: defaults.clone(),
            effective: defaults.clone(),
            inheritable: defaults.clone(),
            permitted: defaults.clone(),
            ambient: Vec::new(),
        }
    }

    pub fn has_cap(&self, set: CapSet, cap: Capability) -> bool {
        match set {
            CapSet::Bounding => self.bounding.contains(&cap),
            CapSet::Effective => self.effective.contains(&cap),
            CapSet::Inheritable => self.inheritable.contains(&cap),
            CapSet::Permitted => self.permitted.contains(&cap),
            CapSet::Ambient => self.ambient.contains(&cap),
        }
    }

    pub fn add_cap(&mut self, set: CapSet, cap: Capability) {
        let v = match set {
            CapSet::Bounding => &mut self.bounding,
            CapSet::Effective => &mut self.effective,
            CapSet::Inheritable => &mut self.inheritable,
            CapSet::Permitted => &mut self.permitted,
            CapSet::Ambient => &mut self.ambient,
        };
        if !v.contains(&cap) {
            v.push(cap);
        }
    }

    pub fn drop_cap(&mut self, set: CapSet, cap: Capability) {
        let v = match set {
            CapSet::Bounding => &mut self.bounding,
            CapSet::Effective => &mut self.effective,
            CapSet::Inheritable => &mut self.inheritable,
            CapSet::Permitted => &mut self.permitted,
            CapSet::Ambient => &mut self.ambient,
        };
        v.retain(|c| *c != cap);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapSet {
    Bounding,
    Effective,
    Inheritable,
    Permitted,
    Ambient,
}

// ─── Resource Limits ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RlimitType {
    Cpu,
    Fsize,
    Data,
    Stack,
    Core,
    Rss,
    Nproc,
    Nofile,
    Memlock,
    As,
    Locks,
    Sigpending,
    Msgqueue,
    Nice,
    Rtprio,
    Rttime,
}

#[derive(Debug, Clone, Copy)]
pub struct Rlimit {
    pub limit_type: RlimitType,
    pub hard: u64,
    pub soft: u64,
}

// ─── Seccomp ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeccompAction {
    Kill,
    KillProcess,
    KillThread,
    Trap,
    Errno(u32),
    Trace(u32),
    Allow,
    Log,
    Notify,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeccompArch {
    X86,
    X86_64,
    Arm,
    Aarch64,
    Riscv64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeccompOp {
    NotEqual,
    LessThan,
    LessEqual,
    EqualTo,
    GreaterEqual,
    GreaterThan,
    MaskedEqual(u64),
}

#[derive(Debug, Clone)]
pub struct SeccompArg {
    pub index: u32,
    pub value: u64,
    pub value_two: u64,
    pub op: SeccompOp,
}

#[derive(Debug, Clone)]
pub struct SeccompSyscall {
    pub names: Vec<String>,
    pub action: SeccompAction,
    pub errno_ret: u32,
    pub args: Vec<SeccompArg>,
}

#[derive(Debug, Clone)]
pub struct SeccompProfile {
    pub default_action: SeccompAction,
    pub default_errno_ret: u32,
    pub architectures: Vec<SeccompArch>,
    pub flags: Vec<String>,
    pub listener_path: String,
    pub listener_metadata: String,
    pub syscalls: Vec<SeccompSyscall>,
}

impl SeccompProfile {
    pub fn new(default_action: SeccompAction) -> Self {
        Self {
            default_action,
            default_errno_ret: 0,
            architectures: Vec::new(),
            flags: Vec::new(),
            listener_path: String::new(),
            listener_metadata: String::new(),
            syscalls: Vec::new(),
        }
    }

    pub fn default_profile() -> Self {
        let mut p = Self::new(SeccompAction::Errno(1));
        p.architectures.push(SeccompArch::X86_64);
        p
    }

    pub fn evaluate(&self, syscall_nr: u32, args: &[u64; 6]) -> SeccompAction {
        for rule in &self.syscalls {
            let name_match = true; // name lookup deferred to runtime
            if !name_match {
                continue;
            }
            let mut args_match = true;
            for arg in &rule.args {
                let val = args.get(arg.index as usize).copied().unwrap_or(0);
                let ok = match arg.op {
                    SeccompOp::EqualTo => val == arg.value,
                    SeccompOp::NotEqual => val != arg.value,
                    SeccompOp::LessThan => val < arg.value,
                    SeccompOp::LessEqual => val <= arg.value,
                    SeccompOp::GreaterEqual => val >= arg.value,
                    SeccompOp::GreaterThan => val > arg.value,
                    SeccompOp::MaskedEqual(mask) => (val & mask) == arg.value,
                };
                if !ok {
                    args_match = false;
                    break;
                }
            }
            if args_match {
                return rule.action;
            }
        }
        let _ = syscall_nr;
        self.default_action
    }
}

// ─── OCI Linux ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NamespaceType {
    Pid,
    Network,
    Mount,
    Uts,
    Ipc,
    User,
    Cgroup,
    Time,
}

#[derive(Debug, Clone)]
pub struct Namespace {
    pub ns_type: NamespaceType,
    pub path: String,
}

impl Namespace {
    pub fn new(ns_type: NamespaceType) -> Self {
        Self {
            ns_type,
            path: String::new(),
        }
    }

    pub fn with_path(ns_type: NamespaceType, path: String) -> Self {
        Self { ns_type, path }
    }
}

#[derive(Debug, Clone)]
pub struct IdMapping {
    pub container_id: u32,
    pub host_id: u32,
    pub size: u32,
}

#[derive(Debug, Clone)]
pub struct OciLinux {
    pub namespaces: Vec<Namespace>,
    pub uid_mappings: Vec<IdMapping>,
    pub gid_mappings: Vec<IdMapping>,
    pub devices: Vec<DeviceNode>,
    pub cgroups_path: String,
    pub resources: CgroupResources,
    pub seccomp: Option<SeccompProfile>,
    pub rootfs_propagation: MountPropagation,
    pub masked_paths: Vec<String>,
    pub readonly_paths: Vec<String>,
}

impl OciLinux {
    pub fn new() -> Self {
        Self {
            namespaces: Vec::new(),
            uid_mappings: Vec::new(),
            gid_mappings: Vec::new(),
            devices: Vec::new(),
            cgroups_path: String::new(),
            resources: CgroupResources::new(),
            seccomp: None,
            rootfs_propagation: MountPropagation::Private,
            masked_paths: Vec::new(),
            readonly_paths: Vec::new(),
        }
    }

    pub fn default_namespaces() -> Vec<Namespace> {
        alloc::vec![
            Namespace::new(NamespaceType::Pid),
            Namespace::new(NamespaceType::Network),
            Namespace::new(NamespaceType::Mount),
            Namespace::new(NamespaceType::Uts),
            Namespace::new(NamespaceType::Ipc),
        ]
    }

    pub fn default_masked_paths() -> Vec<String> {
        alloc::vec![
            String::from("/proc/acpi"),
            String::from("/proc/asound"),
            String::from("/proc/kcore"),
            String::from("/proc/keys"),
            String::from("/proc/latency_stats"),
            String::from("/proc/timer_list"),
            String::from("/proc/timer_stats"),
            String::from("/proc/sched_debug"),
            String::from("/proc/scsi"),
            String::from("/sys/firmware"),
        ]
    }

    pub fn default_readonly_paths() -> Vec<String> {
        alloc::vec![
            String::from("/proc/bus"),
            String::from("/proc/fs"),
            String::from("/proc/irq"),
            String::from("/proc/sys"),
            String::from("/proc/sysrq-trigger"),
        ]
    }
}

// ─── Mount Propagation ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MountPropagation {
    Private,
    Slave,
    Shared,
    Unbindable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MountFlags(pub u32);

impl MountFlags {
    pub const RDONLY: u32 = 1 << 0;
    pub const NOSUID: u32 = 1 << 1;
    pub const NODEV: u32 = 1 << 2;
    pub const NOEXEC: u32 = 1 << 3;
    pub const SYNCHRONOUS: u32 = 1 << 4;
    pub const REMOUNT: u32 = 1 << 5;
    pub const MANDLOCK: u32 = 1 << 6;
    pub const DIRSYNC: u32 = 1 << 7;
    pub const NOATIME: u32 = 1 << 10;
    pub const NODIRATIME: u32 = 1 << 11;
    pub const BIND: u32 = 1 << 12;
    pub const MOVE: u32 = 1 << 13;
    pub const REC: u32 = 1 << 14;
    pub const PRIVATE: u32 = 1 << 18;
    pub const SLAVE: u32 = 1 << 19;
    pub const SHARED: u32 = 1 << 20;
    pub const RELATIME: u32 = 1 << 21;
    pub const STRICTATIME: u32 = 1 << 24;

    pub fn from_options(opts: &[&str]) -> Self {
        let mut flags = 0u32;
        for opt in opts {
            match *opt {
                "ro" | "readonly" => flags |= Self::RDONLY,
                "nosuid" => flags |= Self::NOSUID,
                "nodev" => flags |= Self::NODEV,
                "noexec" => flags |= Self::NOEXEC,
                "sync" => flags |= Self::SYNCHRONOUS,
                "remount" => flags |= Self::REMOUNT,
                "bind" | "rbind" => flags |= Self::BIND,
                "noatime" => flags |= Self::NOATIME,
                "nodiratime" => flags |= Self::NODIRATIME,
                "relatime" => flags |= Self::RELATIME,
                "strictatime" => flags |= Self::STRICTATIME,
                "private" => flags |= Self::PRIVATE,
                "slave" => flags |= Self::SLAVE,
                "shared" => flags |= Self::SHARED,
                _ => {}
            }
        }
        Self(flags)
    }

    pub fn has(&self, flag: u32) -> bool {
        self.0 & flag != 0
    }
}

// ─── Device Management ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceType {
    Char,
    Block,
    Fifo,
}

#[derive(Debug, Clone)]
pub struct DeviceNode {
    pub path: String,
    pub dev_type: DeviceType,
    pub major: u32,
    pub minor: u32,
    pub file_mode: u32,
    pub uid: u32,
    pub gid: u32,
}

impl DeviceNode {
    pub fn default_devices() -> Vec<Self> {
        alloc::vec![
            Self {
                path: String::from("/dev/null"),
                dev_type: DeviceType::Char,
                major: 1,
                minor: 3,
                file_mode: 0o666,
                uid: 0,
                gid: 0
            },
            Self {
                path: String::from("/dev/zero"),
                dev_type: DeviceType::Char,
                major: 1,
                minor: 5,
                file_mode: 0o666,
                uid: 0,
                gid: 0
            },
            Self {
                path: String::from("/dev/full"),
                dev_type: DeviceType::Char,
                major: 1,
                minor: 7,
                file_mode: 0o666,
                uid: 0,
                gid: 0
            },
            Self {
                path: String::from("/dev/random"),
                dev_type: DeviceType::Char,
                major: 1,
                minor: 8,
                file_mode: 0o666,
                uid: 0,
                gid: 0
            },
            Self {
                path: String::from("/dev/urandom"),
                dev_type: DeviceType::Char,
                major: 1,
                minor: 9,
                file_mode: 0o666,
                uid: 0,
                gid: 0
            },
            Self {
                path: String::from("/dev/tty"),
                dev_type: DeviceType::Char,
                major: 5,
                minor: 0,
                file_mode: 0o666,
                uid: 0,
                gid: 0
            },
            Self {
                path: String::from("/dev/console"),
                dev_type: DeviceType::Char,
                major: 5,
                minor: 1,
                file_mode: 0o620,
                uid: 0,
                gid: 0
            },
            Self {
                path: String::from("/dev/ptmx"),
                dev_type: DeviceType::Char,
                major: 5,
                minor: 2,
                file_mode: 0o666,
                uid: 0,
                gid: 0
            },
        ]
    }
}

#[derive(Debug, Clone)]
pub struct DeviceCgroup {
    pub allow: bool,
    pub dev_type: Option<DeviceType>,
    pub major: Option<u32>,
    pub minor: Option<u32>,
    pub access: String,
}

impl DeviceCgroup {
    pub fn default_rules() -> Vec<Self> {
        alloc::vec![
            Self {
                allow: false,
                dev_type: None,
                major: None,
                minor: None,
                access: String::from("rwm")
            },
            Self {
                allow: true,
                dev_type: Some(DeviceType::Char),
                major: Some(1),
                minor: Some(3),
                access: String::from("rwm")
            },
            Self {
                allow: true,
                dev_type: Some(DeviceType::Char),
                major: Some(1),
                minor: Some(5),
                access: String::from("rwm")
            },
            Self {
                allow: true,
                dev_type: Some(DeviceType::Char),
                major: Some(1),
                minor: Some(7),
                access: String::from("rwm")
            },
            Self {
                allow: true,
                dev_type: Some(DeviceType::Char),
                major: Some(1),
                minor: Some(8),
                access: String::from("rwm")
            },
            Self {
                allow: true,
                dev_type: Some(DeviceType::Char),
                major: Some(1),
                minor: Some(9),
                access: String::from("rwm")
            },
            Self {
                allow: true,
                dev_type: Some(DeviceType::Char),
                major: Some(5),
                minor: Some(0),
                access: String::from("rwm")
            },
            Self {
                allow: true,
                dev_type: Some(DeviceType::Char),
                major: Some(5),
                minor: Some(2),
                access: String::from("rwm")
            },
        ]
    }
}

// ─── Cgroup Resources ───────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct CgroupResources {
    pub memory: MemoryLimit,
    pub cpu: CpuLimit,
    pub pids: PidsLimit,
    pub block_io: BlockIoLimit,
    pub hugepage_limits: Vec<HugepageLimit>,
}

impl CgroupResources {
    pub fn new() -> Self {
        Self {
            memory: MemoryLimit::new(),
            cpu: CpuLimit::new(),
            pids: PidsLimit::new(),
            block_io: BlockIoLimit::new(),
            hugepage_limits: Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct MemoryLimit {
    pub limit: i64,
    pub swap: i64,
    pub reservation: i64,
    pub kernel: i64,
    pub kernel_tcp: i64,
    pub swappiness: u64,
    pub disable_oom_killer: bool,
    pub oom_score_adj: i32,
    pub use_hierarchy: bool,
}

impl MemoryLimit {
    pub fn new() -> Self {
        Self {
            limit: -1,
            swap: -1,
            reservation: -1,
            kernel: -1,
            kernel_tcp: -1,
            swappiness: 60,
            disable_oom_killer: false,
            oom_score_adj: 0,
            use_hierarchy: true,
        }
    }

    pub fn set_limit_mb(&mut self, mb: u64) {
        self.limit = (mb * 1024 * 1024) as i64;
    }

    pub fn set_swap_mb(&mut self, mb: u64) {
        self.swap = (mb * 1024 * 1024) as i64;
    }

    pub fn effective_limit(&self) -> Option<u64> {
        if self.limit >= 0 {
            Some(self.limit as u64)
        } else {
            None
        }
    }
}

#[derive(Debug, Clone)]
pub struct CpuLimit {
    pub shares: u64,
    pub quota: i64,
    pub period: u64,
    pub realtime_runtime: i64,
    pub realtime_period: u64,
    pub cpus: String,
    pub mems: String,
}

impl CpuLimit {
    pub fn new() -> Self {
        Self {
            shares: 1024,
            quota: -1,
            period: 100_000,
            realtime_runtime: 0,
            realtime_period: 1_000_000,
            cpus: String::new(),
            mems: String::new(),
        }
    }

    pub fn set_cpus(&mut self, cpuset: &str) {
        self.cpus = String::from(cpuset);
    }

    pub fn cpu_fraction(&self) -> Option<f64> {
        if self.quota > 0 && self.period > 0 {
            Some(self.quota as f64 / self.period as f64)
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct PidsLimit {
    pub limit: i64,
}

impl PidsLimit {
    pub fn new() -> Self {
        Self { limit: -1 }
    }

    pub fn set_max(&mut self, max: u64) {
        self.limit = max as i64;
    }
}

#[derive(Debug, Clone)]
pub struct BlockIoLimit {
    pub weight: u16,
    pub leaf_weight: u16,
    pub weight_device: Vec<WeightDevice>,
    pub throttle_read_bps: Vec<ThrottleDevice>,
    pub throttle_write_bps: Vec<ThrottleDevice>,
    pub throttle_read_iops: Vec<ThrottleDevice>,
    pub throttle_write_iops: Vec<ThrottleDevice>,
}

impl BlockIoLimit {
    pub fn new() -> Self {
        Self {
            weight: 500,
            leaf_weight: 500,
            weight_device: Vec::new(),
            throttle_read_bps: Vec::new(),
            throttle_write_bps: Vec::new(),
            throttle_read_iops: Vec::new(),
            throttle_write_iops: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct WeightDevice {
    pub major: u32,
    pub minor: u32,
    pub weight: u16,
    pub leaf_weight: u16,
}

#[derive(Debug, Clone, Copy)]
pub struct ThrottleDevice {
    pub major: u32,
    pub minor: u32,
    pub rate: u64,
}

#[derive(Debug, Clone)]
pub struct HugepageLimit {
    pub page_size: String,
    pub limit: u64,
}

// ─── OCI Hooks ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct HookEntry {
    pub path: String,
    pub args: Vec<String>,
    pub env: Vec<String>,
    pub timeout_secs: u32,
}

impl HookEntry {
    pub fn new(path: String) -> Self {
        Self {
            path,
            args: Vec::new(),
            env: Vec::new(),
            timeout_secs: 30,
        }
    }
}

#[derive(Debug, Clone)]
pub struct OciHooks {
    pub prestart: Vec<HookEntry>,
    pub create_runtime: Vec<HookEntry>,
    pub create_container: Vec<HookEntry>,
    pub start_container: Vec<HookEntry>,
    pub poststart: Vec<HookEntry>,
    pub poststop: Vec<HookEntry>,
}

impl OciHooks {
    pub fn new() -> Self {
        Self {
            prestart: Vec::new(),
            create_runtime: Vec::new(),
            create_container: Vec::new(),
            start_container: Vec::new(),
            poststart: Vec::new(),
            poststop: Vec::new(),
        }
    }

    pub fn run_hooks(hooks: &[HookEntry], state: &ContainerStateInfo) -> Result<()> {
        for hook in hooks {
            let _path = &hook.path;
            let _timeout = hook.timeout_secs;
            let _ = state;
        }
        Ok(())
    }
}

// ─── Container State Info ────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ContainerStateInfo {
    pub oci_version: String,
    pub id: ContainerId,
    pub status: ContainerState,
    pub pid: u64,
    pub bundle: String,
    pub annotations: BTreeMap<String, String>,
    pub created: u64,
}

impl ContainerStateInfo {
    pub fn new(id: ContainerId, bundle: String) -> Self {
        Self {
            oci_version: String::from("1.0.2"),
            id,
            status: ContainerState::Creating,
            pid: 0,
            bundle,
            annotations: BTreeMap::new(),
            created: 0,
        }
    }
}

// ─── Log Driver ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogDriver {
    JsonFile,
    Syslog,
    Journald,
    None,
}

#[derive(Debug, Clone)]
pub struct LogConfig {
    pub driver: LogDriver,
    pub path: String,
    pub max_size_bytes: u64,
    pub max_files: u32,
    pub compress: bool,
    pub tag: String,
}

impl LogConfig {
    pub fn new(driver: LogDriver) -> Self {
        Self {
            driver,
            path: String::new(),
            max_size_bytes: 10 * 1024 * 1024,
            max_files: 3,
            compress: false,
            tag: String::new(),
        }
    }

    pub fn should_rotate(&self, current_size: u64) -> bool {
        self.max_size_bytes > 0 && current_size >= self.max_size_bytes
    }
}

#[derive(Debug, Clone)]
pub struct LogEntry {
    pub timestamp: u64,
    pub stream: LogStream,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogStream {
    Stdout,
    Stderr,
}

pub struct ContainerLogger {
    pub config: LogConfig,
    pub entries: Vec<LogEntry>,
    pub total_bytes: u64,
    pub current_file_bytes: u64,
    pub rotations: u32,
}

impl ContainerLogger {
    pub fn new(config: LogConfig) -> Self {
        Self {
            config,
            entries: Vec::new(),
            total_bytes: 0,
            current_file_bytes: 0,
            rotations: 0,
        }
    }

    pub fn log(&mut self, stream: LogStream, data: Vec<u8>, timestamp: u64) {
        let len = data.len() as u64;
        if self.config.should_rotate(self.current_file_bytes + len) {
            self.rotate();
        }
        self.current_file_bytes += len;
        self.total_bytes += len;
        self.entries.push(LogEntry {
            timestamp,
            stream,
            data,
        });
    }

    fn rotate(&mut self) {
        self.rotations += 1;
        self.current_file_bytes = 0;
        if self.rotations >= self.config.max_files {
            let keep = self.entries.len() / 2;
            self.entries.drain(..self.entries.len() - keep);
        }
    }
}

// ─── OCI Image Spec ──────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ImageManifest {
    pub schema_version: u32,
    pub media_type: String,
    pub config: ImageDescriptor,
    pub layers: Vec<ImageDescriptor>,
    pub annotations: BTreeMap<String, String>,
}

impl ImageManifest {
    pub fn new() -> Self {
        Self {
            schema_version: 2,
            media_type: String::from("application/vnd.oci.image.manifest.v1+json"),
            config: ImageDescriptor::new(),
            layers: Vec::new(),
            annotations: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ImageDescriptor {
    pub media_type: String,
    pub digest: String,
    pub size: u64,
    pub urls: Vec<String>,
    pub annotations: BTreeMap<String, String>,
}

impl ImageDescriptor {
    pub fn new() -> Self {
        Self {
            media_type: String::new(),
            digest: String::new(),
            size: 0,
            urls: Vec::new(),
            annotations: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ImageConfig {
    pub created: String,
    pub author: String,
    pub architecture: String,
    pub os: String,
    pub os_version: String,
    pub os_features: Vec<String>,
    pub variant: String,
    pub config: ImageExecutionConfig,
    pub rootfs: ImageRootfs,
    pub history: Vec<ImageHistory>,
}

impl ImageConfig {
    pub fn new() -> Self {
        Self {
            created: String::new(),
            author: String::new(),
            architecture: String::from("amd64"),
            os: String::from("raeenos"),
            os_version: String::new(),
            os_features: Vec::new(),
            variant: String::new(),
            config: ImageExecutionConfig::new(),
            rootfs: ImageRootfs::new(),
            history: Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ImageExecutionConfig {
    pub user: String,
    pub exposed_ports: Vec<String>,
    pub env: Vec<String>,
    pub entrypoint: Vec<String>,
    pub cmd: Vec<String>,
    pub volumes: Vec<String>,
    pub working_dir: String,
    pub labels: BTreeMap<String, String>,
    pub stop_signal: String,
}

impl ImageExecutionConfig {
    pub fn new() -> Self {
        Self {
            user: String::new(),
            exposed_ports: Vec::new(),
            env: Vec::new(),
            entrypoint: Vec::new(),
            cmd: Vec::new(),
            volumes: Vec::new(),
            working_dir: String::new(),
            labels: BTreeMap::new(),
            stop_signal: String::from("SIGTERM"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ImageRootfs {
    pub diff_type: String,
    pub diff_ids: Vec<String>,
}

impl ImageRootfs {
    pub fn new() -> Self {
        Self {
            diff_type: String::from("layers"),
            diff_ids: Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ImageHistory {
    pub created: String,
    pub created_by: String,
    pub author: String,
    pub comment: String,
    pub empty_layer: bool,
}

// ─── Content-Addressable Storage ─────────────────────────────────────────────

pub struct ContentStore {
    pub blobs: BTreeMap<String, Vec<u8>>,
    pub total_size: u64,
}

impl ContentStore {
    pub fn new() -> Self {
        Self {
            blobs: BTreeMap::new(),
            total_size: 0,
        }
    }

    pub fn put(&mut self, data: Vec<u8>) -> String {
        let digest = sha256_hex(&data);
        let size = data.len() as u64;
        if !self.blobs.contains_key(&digest) {
            self.total_size += size;
            self.blobs.insert(digest.clone(), data);
        }
        digest
    }

    pub fn get(&self, digest: &str) -> Option<&Vec<u8>> {
        self.blobs.get(digest)
    }

    pub fn remove(&mut self, digest: &str) -> bool {
        if let Some(data) = self.blobs.remove(digest) {
            self.total_size -= data.len() as u64;
            true
        } else {
            false
        }
    }

    pub fn contains(&self, digest: &str) -> bool {
        self.blobs.contains_key(digest)
    }

    pub fn gc(&mut self, referenced: &[String]) {
        let keep: alloc::collections::BTreeSet<&String> = referenced.iter().collect();
        let to_remove: Vec<String> = self
            .blobs
            .keys()
            .filter(|k| !keep.contains(k))
            .cloned()
            .collect();
        for key in to_remove {
            self.remove(&key);
        }
    }
}

/// SHA-256 (FIPS 180-4) content digest as a 64-char lowercase hex string — the
/// real cryptographic hash an OCI content-addressed digest requires. Replaces a
/// homebrew FNV-style stub that had no collision resistance.
fn sha256_hex(data: &[u8]) -> String {
    let digest = rae_crypto::sha256::sha256(data);
    let mut hex = String::with_capacity(64);
    for &byte in digest.iter() {
        for nibble in [byte >> 4, byte & 0xf] {
            let ch = if nibble < 10 {
                b'0' + nibble
            } else {
                b'a' + (nibble - 10)
            };
            hex.push(ch as char);
        }
    }
    hex
}

// ─── Layer Storage ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayerMediaType {
    TarGzip,
    Tar,
    TarZstd,
    Nondistributable,
}

#[derive(Debug, Clone)]
pub struct Layer {
    pub id: LayerId,
    pub digest: String,
    pub diff_id: String,
    pub media_type: LayerMediaType,
    pub size: u64,
    pub parent: Option<LayerId>,
    pub created: u64,
    pub ref_count: u32,
}

pub struct LayerStore {
    pub layers: BTreeMap<u64, Layer>,
    pub content: ContentStore,
    next_id: u64,
}

impl LayerStore {
    pub fn new() -> Self {
        Self {
            layers: BTreeMap::new(),
            content: ContentStore::new(),
            next_id: 1,
        }
    }

    pub fn add_layer(
        &mut self,
        data: Vec<u8>,
        media_type: LayerMediaType,
        parent: Option<LayerId>,
    ) -> LayerId {
        let digest = self.content.put(data.clone());
        let diff_id = sha256_hex(&data);
        let size = data.len() as u64;
        let id = LayerId(self.next_id);
        self.next_id += 1;
        self.layers.insert(
            id.0,
            Layer {
                id,
                digest,
                diff_id,
                media_type,
                size,
                parent,
                created: 0,
                ref_count: 1,
            },
        );
        id
    }

    pub fn get_layer(&self, id: LayerId) -> Option<&Layer> {
        self.layers.get(&id.0)
    }

    pub fn remove_layer(&mut self, id: LayerId) -> bool {
        if let Some(layer) = self.layers.remove(&id.0) {
            self.content.remove(&layer.digest);
            true
        } else {
            false
        }
    }

    pub fn layer_chain(&self, id: LayerId) -> Vec<LayerId> {
        let mut chain = Vec::new();
        let mut current = Some(id);
        while let Some(lid) = current {
            chain.push(lid);
            current = self.layers.get(&lid.0).and_then(|l| l.parent);
        }
        chain.reverse();
        chain
    }
}

// ─── Registry Protocol ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistryAuthType {
    None,
    Basic,
    Bearer,
}

#[derive(Debug, Clone)]
pub struct RegistryConfig {
    pub url: String,
    pub auth_type: RegistryAuthType,
    pub username: String,
    pub password: String,
    pub token: String,
    pub tls_verify: bool,
    pub tls_ca_cert: Vec<u8>,
}

impl RegistryConfig {
    pub fn new(url: String) -> Self {
        Self {
            url,
            auth_type: RegistryAuthType::None,
            username: String::new(),
            password: String::new(),
            token: String::new(),
            tls_verify: true,
            tls_ca_cert: Vec::new(),
        }
    }

    pub fn with_basic_auth(mut self, username: String, password: String) -> Self {
        self.auth_type = RegistryAuthType::Basic;
        self.username = username;
        self.password = password;
        self
    }

    pub fn with_bearer_token(mut self, token: String) -> Self {
        self.auth_type = RegistryAuthType::Bearer;
        self.token = token;
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistryOp {
    PullManifest,
    PushManifest,
    PullBlob,
    PushBlob,
    ListTags,
    Catalog,
    CheckBlob,
}

#[derive(Debug, Clone)]
pub struct RegistryRequest {
    pub op: RegistryOp,
    pub repository: String,
    pub reference: String,
    pub digest: String,
    pub data: Vec<u8>,
}

impl RegistryRequest {
    pub fn pull_manifest(repo: String, reference: String) -> Self {
        Self {
            op: RegistryOp::PullManifest,
            repository: repo,
            reference,
            digest: String::new(),
            data: Vec::new(),
        }
    }

    pub fn pull_blob(repo: String, digest: String) -> Self {
        Self {
            op: RegistryOp::PullBlob,
            repository: repo,
            reference: String::new(),
            digest,
            data: Vec::new(),
        }
    }

    pub fn push_manifest(repo: String, reference: String, data: Vec<u8>) -> Self {
        Self {
            op: RegistryOp::PushManifest,
            repository: repo,
            reference,
            digest: String::new(),
            data,
        }
    }

    pub fn list_tags(repo: String) -> Self {
        Self {
            op: RegistryOp::ListTags,
            repository: repo,
            reference: String::new(),
            digest: String::new(),
            data: Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct RegistryResponse {
    pub status: u16,
    pub digest: String,
    pub content_type: String,
    pub data: Vec<u8>,
    pub tags: Vec<String>,
}

// ─── Volume Management ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VolumeDriver {
    Local,
    Tmpfs,
    Nfs,
    Custom,
}

#[derive(Debug, Clone)]
pub struct Volume {
    pub id: VolumeId,
    pub name: String,
    pub driver: VolumeDriver,
    pub mountpoint: String,
    pub options: BTreeMap<String, String>,
    pub labels: BTreeMap<String, String>,
    pub created: u64,
    pub ref_count: u32,
}

pub struct VolumeManager {
    pub volumes: BTreeMap<u64, Volume>,
    next_id: u64,
}

impl VolumeManager {
    pub fn new() -> Self {
        Self {
            volumes: BTreeMap::new(),
            next_id: 1,
        }
    }

    pub fn create(
        &mut self,
        name: String,
        driver: VolumeDriver,
        mountpoint: String,
    ) -> Result<VolumeId> {
        for vol in self.volumes.values() {
            if vol.name == name {
                return Err(ContainerError::AlreadyExists);
            }
        }
        let id = VolumeId(self.next_id);
        self.next_id += 1;
        self.volumes.insert(
            id.0,
            Volume {
                id,
                name,
                driver,
                mountpoint,
                options: BTreeMap::new(),
                labels: BTreeMap::new(),
                created: 0,
                ref_count: 0,
            },
        );
        Ok(id)
    }

    pub fn remove(&mut self, id: VolumeId) -> Result<()> {
        let vol = self.volumes.get(&id.0).ok_or(ContainerError::NotFound)?;
        if vol.ref_count > 0 {
            return Err(ContainerError::InvalidState);
        }
        self.volumes.remove(&id.0);
        Ok(())
    }

    pub fn get(&self, id: VolumeId) -> Option<&Volume> {
        self.volumes.get(&id.0)
    }

    pub fn find_by_name(&self, name: &str) -> Option<&Volume> {
        self.volumes.values().find(|v| v.name == name)
    }

    pub fn list(&self) -> Vec<&Volume> {
        self.volumes.values().collect()
    }

    pub fn add_ref(&mut self, id: VolumeId) -> Result<()> {
        let vol = self
            .volumes
            .get_mut(&id.0)
            .ok_or(ContainerError::NotFound)?;
        vol.ref_count += 1;
        Ok(())
    }

    pub fn release_ref(&mut self, id: VolumeId) -> Result<()> {
        let vol = self
            .volumes
            .get_mut(&id.0)
            .ok_or(ContainerError::NotFound)?;
        if vol.ref_count > 0 {
            vol.ref_count -= 1;
        }
        Ok(())
    }
}

// ─── Container Networking (CNI) ──────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CniPlugin {
    Bridge,
    Host,
    None,
    Macvlan,
    Ipvlan,
    Ptp,
    Loopback,
}

#[derive(Debug, Clone)]
pub struct CniConfig {
    pub cni_version: String,
    pub name: String,
    pub plugin: CniPlugin,
    pub bridge_name: String,
    pub is_gateway: bool,
    pub ip_masq: bool,
    pub subnet: String,
    pub gateway: String,
    pub mtu: u32,
}

impl CniConfig {
    pub fn default_bridge() -> Self {
        Self {
            cni_version: String::from("1.0.0"),
            name: String::from("raenet0"),
            plugin: CniPlugin::Bridge,
            bridge_name: String::from("cni0"),
            is_gateway: true,
            ip_masq: true,
            subnet: String::from("10.88.0.0/16"),
            gateway: String::from("10.88.0.1"),
            mtu: 1500,
        }
    }
}

#[derive(Debug, Clone)]
pub struct VethPair {
    pub host_name: String,
    pub container_name: String,
    pub host_mac: [u8; 6],
    pub container_mac: [u8; 6],
    pub container_ip: [u8; 4],
    pub container_mask: u8,
    pub mtu: u32,
}

#[derive(Debug, Clone)]
pub struct PortMapping {
    pub host_ip: [u8; 4],
    pub host_port: u16,
    pub container_port: u16,
    pub protocol: PortProtocol,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortProtocol {
    Tcp,
    Udp,
    Sctp,
}

#[derive(Debug, Clone)]
pub struct ContainerNetwork {
    pub id: NetworkId,
    pub name: String,
    pub cni: CniConfig,
    pub veth: Option<VethPair>,
    pub port_mappings: Vec<PortMapping>,
    pub dns_servers: Vec<String>,
    pub dns_search: Vec<String>,
    pub dns_options: Vec<String>,
    pub extra_hosts: Vec<(String, String)>,
}

impl ContainerNetwork {
    pub fn new(id: NetworkId, name: String) -> Self {
        Self {
            id,
            name,
            cni: CniConfig::default_bridge(),
            veth: None,
            port_mappings: Vec::new(),
            dns_servers: alloc::vec![String::from("8.8.8.8"), String::from("8.8.4.4")],
            dns_search: Vec::new(),
            dns_options: Vec::new(),
            extra_hosts: Vec::new(),
        }
    }

    pub fn generate_resolv_conf(&self) -> String {
        let mut conf = String::new();
        for ns in &self.dns_servers {
            conf.push_str("nameserver ");
            conf.push_str(ns);
            conf.push('\n');
        }
        for s in &self.dns_search {
            conf.push_str("search ");
            conf.push_str(s);
            conf.push('\n');
        }
        for o in &self.dns_options {
            conf.push_str("options ");
            conf.push_str(o);
            conf.push('\n');
        }
        conf
    }

    pub fn generate_hosts(&self, hostname: &str, ip: &str) -> String {
        let mut hosts = String::new();
        hosts.push_str("127.0.0.1\tlocalhost\n");
        hosts.push_str("::1\t\tlocalhost ip6-localhost ip6-loopback\n");
        hosts.push_str(ip);
        hosts.push('\t');
        hosts.push_str(hostname);
        hosts.push('\n');
        for (host, addr) in &self.extra_hosts {
            hosts.push_str(addr);
            hosts.push('\t');
            hosts.push_str(host);
            hosts.push('\n');
        }
        hosts
    }

    pub fn generate_hostname_file(&self, hostname: &str) -> String {
        let mut s = String::from(hostname);
        s.push('\n');
        s
    }
}

// ─── Container ───────────────────────────────────────────────────────────────

pub struct Container {
    pub id: ContainerId,
    pub name: String,
    pub state: ContainerState,
    pub spec: OciSpec,
    pub pid: u64,
    pub exit_code: i32,
    pub created_at: u64,
    pub started_at: u64,
    pub finished_at: u64,
    pub bundle_path: String,
    pub log: ContainerLogger,
    pub network: Option<ContainerNetwork>,
    pub volumes: Vec<VolumeId>,
    pub checkpoint_data: Option<Vec<u8>>,
}

impl Container {
    pub fn new(id: ContainerId, name: String, spec: OciSpec, bundle: String) -> Self {
        Self {
            id,
            name,
            state: ContainerState::Creating,
            spec,
            pid: 0,
            exit_code: -1,
            created_at: 0,
            started_at: 0,
            finished_at: 0,
            bundle_path: bundle,
            log: ContainerLogger::new(LogConfig::new(LogDriver::JsonFile)),
            network: None,
            volumes: Vec::new(),
            checkpoint_data: None,
        }
    }

    pub fn create(&mut self) -> Result<()> {
        if self.state != ContainerState::Creating {
            return Err(ContainerError::InvalidState);
        }
        self.spec.validate()?;
        self.setup_rootfs()?;
        self.setup_namespaces()?;
        self.setup_devices()?;
        self.setup_cgroups()?;
        self.setup_security()?;
        OciHooks::run_hooks(&self.spec.hooks.create_runtime, &self.state_info())?;
        OciHooks::run_hooks(&self.spec.hooks.create_container, &self.state_info())?;
        self.state = ContainerState::Created;
        Ok(())
    }

    pub fn start(&mut self) -> Result<()> {
        if self.state != ContainerState::Created {
            return Err(ContainerError::InvalidState);
        }
        OciHooks::run_hooks(&self.spec.hooks.prestart, &self.state_info())?;
        OciHooks::run_hooks(&self.spec.hooks.start_container, &self.state_info())?;
        self.state = ContainerState::Running;
        self.pid = 1;
        OciHooks::run_hooks(&self.spec.hooks.poststart, &self.state_info())?;
        Ok(())
    }

    pub fn kill(&mut self, signal: u32) -> Result<()> {
        if self.state != ContainerState::Running && self.state != ContainerState::Paused {
            return Err(ContainerError::InvalidState);
        }
        let _ = signal;
        self.state = ContainerState::Stopped;
        self.exit_code = 128 + signal as i32;
        self.pid = 0;
        OciHooks::run_hooks(&self.spec.hooks.poststop, &self.state_info())?;
        Ok(())
    }

    pub fn delete(&mut self) -> Result<()> {
        if self.state != ContainerState::Stopped && self.state != ContainerState::Created {
            return Err(ContainerError::InvalidState);
        }
        self.cleanup_cgroups();
        self.cleanup_network();
        self.cleanup_mounts();
        Ok(())
    }

    pub fn pause(&mut self) -> Result<()> {
        if self.state != ContainerState::Running {
            return Err(ContainerError::InvalidState);
        }
        self.state = ContainerState::Paused;
        Ok(())
    }

    pub fn resume(&mut self) -> Result<()> {
        if self.state != ContainerState::Paused {
            return Err(ContainerError::InvalidState);
        }
        self.state = ContainerState::Running;
        Ok(())
    }

    pub fn checkpoint(&mut self) -> Result<Vec<u8>> {
        if self.state != ContainerState::Running {
            return Err(ContainerError::InvalidState);
        }
        let data = alloc::vec![0u8; 4096];
        self.checkpoint_data = Some(data.clone());
        Ok(data)
    }

    pub fn restore(&mut self, data: &[u8]) -> Result<()> {
        if self.state != ContainerState::Stopped && self.state != ContainerState::Created {
            return Err(ContainerError::InvalidState);
        }
        self.checkpoint_data = Some(data.to_vec());
        self.state = ContainerState::Running;
        Ok(())
    }

    pub fn state_info(&self) -> ContainerStateInfo {
        ContainerStateInfo {
            oci_version: self.spec.oci_version.clone(),
            id: self.id,
            status: self.state,
            pid: self.pid,
            bundle: self.bundle_path.clone(),
            annotations: self.spec.annotations.clone(),
            created: self.created_at,
        }
    }

    fn setup_rootfs(&self) -> Result<()> {
        let _root = &self.spec.root;
        for mount in &self.spec.mounts {
            let _dest = &mount.destination;
            let _src = &mount.source;
            let _typ = &mount.mount_type;
        }
        Ok(())
    }

    fn setup_namespaces(&self) -> Result<()> {
        for ns in &self.spec.linux.namespaces {
            match ns.ns_type {
                NamespaceType::Pid => {}
                NamespaceType::Network => {}
                NamespaceType::Mount => {}
                NamespaceType::Uts => {}
                NamespaceType::Ipc => {}
                NamespaceType::User => {
                    for _map in &self.spec.linux.uid_mappings {}
                    for _map in &self.spec.linux.gid_mappings {}
                }
                NamespaceType::Cgroup => {}
                NamespaceType::Time => {}
            }
        }
        Ok(())
    }

    fn setup_devices(&self) -> Result<()> {
        for _dev in &self.spec.linux.devices {}
        Ok(())
    }

    fn setup_cgroups(&self) -> Result<()> {
        let _res = &self.spec.linux.resources;
        Ok(())
    }

    fn setup_security(&self) -> Result<()> {
        let proc = &self.spec.process;
        if proc.no_new_privileges {}
        if let Some(_seccomp) = &self.spec.linux.seccomp {}
        let _ = &proc.apparmor_profile;
        let _ = &proc.selinux_label;
        Ok(())
    }

    fn cleanup_cgroups(&self) {
        let _ = &self.spec.linux.cgroups_path;
    }

    fn cleanup_network(&self) {
        let _ = &self.network;
    }

    fn cleanup_mounts(&self) {
        let _ = &self.spec.mounts;
    }
}

// ─── Image Manager ───────────────────────────────────────────────────────────

pub struct OciImage {
    pub id: ImageId,
    pub name: String,
    pub tag: String,
    pub manifest: ImageManifest,
    pub config: ImageConfig,
    pub layer_ids: Vec<LayerId>,
    pub size: u64,
    pub created: u64,
}

pub struct ImageManager {
    pub images: BTreeMap<u64, OciImage>,
    pub layer_store: LayerStore,
    next_id: u64,
}

impl ImageManager {
    pub fn new() -> Self {
        Self {
            images: BTreeMap::new(),
            layer_store: LayerStore::new(),
            next_id: 1,
        }
    }

    pub fn pull_image(
        &mut self,
        _registry: &RegistryConfig,
        name: &str,
        tag: &str,
    ) -> Result<ImageId> {
        let id = ImageId(self.next_id);
        self.next_id += 1;
        let image = OciImage {
            id,
            name: String::from(name),
            tag: String::from(tag),
            manifest: ImageManifest::new(),
            config: ImageConfig::new(),
            layer_ids: Vec::new(),
            size: 0,
            created: 0,
        };
        self.images.insert(id.0, image);
        Ok(id)
    }

    pub fn remove_image(&mut self, id: ImageId) -> Result<()> {
        let image = self.images.remove(&id.0).ok_or(ContainerError::NotFound)?;
        for lid in &image.layer_ids {
            self.layer_store.remove_layer(*lid);
        }
        Ok(())
    }

    pub fn get_image(&self, id: ImageId) -> Option<&OciImage> {
        self.images.get(&id.0)
    }

    pub fn find_by_name(&self, name: &str, tag: &str) -> Option<&OciImage> {
        self.images
            .values()
            .find(|i| i.name == name && i.tag == tag)
    }

    pub fn list_images(&self) -> Vec<&OciImage> {
        self.images.values().collect()
    }

    pub fn gc_unreferenced_layers(&mut self, used: &[LayerId]) {
        let used_set: alloc::collections::BTreeSet<u64> = used.iter().map(|l| l.0).collect();
        let to_remove: Vec<LayerId> = self
            .layer_store
            .layers
            .keys()
            .filter(|k| !used_set.contains(k))
            .map(|k| LayerId(*k))
            .collect();
        for lid in to_remove {
            self.layer_store.remove_layer(lid);
        }
    }
}

// ─── Container Runtime ───────────────────────────────────────────────────────

pub struct ContainerRuntime {
    pub containers: BTreeMap<u64, Container>,
    pub image_manager: ImageManager,
    pub volume_manager: VolumeManager,
    pub networks: BTreeMap<u64, ContainerNetwork>,
    pub registries: Vec<RegistryConfig>,
    next_container_id: u64,
    next_network_id: u64,
    initialized: bool,
}

static RUNTIME_INITIALIZED: AtomicBool = AtomicBool::new(false);
static TOTAL_CONTAINERS_CREATED: AtomicU64 = AtomicU64::new(0);

impl ContainerRuntime {
    pub fn new() -> Self {
        Self {
            containers: BTreeMap::new(),
            image_manager: ImageManager::new(),
            volume_manager: VolumeManager::new(),
            networks: BTreeMap::new(),
            registries: Vec::new(),
            next_container_id: 1,
            next_network_id: 1,
            initialized: false,
        }
    }

    pub fn create_container(
        &mut self,
        name: String,
        spec: OciSpec,
        bundle: String,
    ) -> Result<ContainerId> {
        for c in self.containers.values() {
            if c.name == name {
                return Err(ContainerError::AlreadyExists);
            }
        }
        let id = ContainerId(self.next_container_id);
        self.next_container_id += 1;
        let mut container = Container::new(id, name, spec, bundle);
        container.create()?;
        self.containers.insert(id.0, container);
        TOTAL_CONTAINERS_CREATED.fetch_add(1, Ordering::Relaxed);
        Ok(id)
    }

    pub fn start_container(&mut self, id: ContainerId) -> Result<()> {
        let container = self
            .containers
            .get_mut(&id.0)
            .ok_or(ContainerError::NotFound)?;
        container.start()
    }

    pub fn kill_container(&mut self, id: ContainerId, signal: u32) -> Result<()> {
        let container = self
            .containers
            .get_mut(&id.0)
            .ok_or(ContainerError::NotFound)?;
        container.kill(signal)
    }

    pub fn delete_container(&mut self, id: ContainerId) -> Result<()> {
        let container = self
            .containers
            .get_mut(&id.0)
            .ok_or(ContainerError::NotFound)?;
        container.delete()?;
        for vid in &container.volumes {
            let _ = self.volume_manager.release_ref(*vid);
        }
        self.containers.remove(&id.0);
        Ok(())
    }

    pub fn container_state(&self, id: ContainerId) -> Result<ContainerStateInfo> {
        let container = self.containers.get(&id.0).ok_or(ContainerError::NotFound)?;
        Ok(container.state_info())
    }

    pub fn pause_container(&mut self, id: ContainerId) -> Result<()> {
        let container = self
            .containers
            .get_mut(&id.0)
            .ok_or(ContainerError::NotFound)?;
        container.pause()
    }

    pub fn resume_container(&mut self, id: ContainerId) -> Result<()> {
        let container = self
            .containers
            .get_mut(&id.0)
            .ok_or(ContainerError::NotFound)?;
        container.resume()
    }

    pub fn checkpoint_container(&mut self, id: ContainerId) -> Result<Vec<u8>> {
        let container = self
            .containers
            .get_mut(&id.0)
            .ok_or(ContainerError::NotFound)?;
        container.checkpoint()
    }

    pub fn restore_container(&mut self, id: ContainerId, data: &[u8]) -> Result<()> {
        let container = self
            .containers
            .get_mut(&id.0)
            .ok_or(ContainerError::NotFound)?;
        container.restore(data)
    }

    pub fn list_containers(&self, state_filter: Option<ContainerState>) -> Vec<&Container> {
        self.containers
            .values()
            .filter(|c| state_filter.map_or(true, |s| c.state == s))
            .collect()
    }

    pub fn create_network(&mut self, name: String, cni: CniConfig) -> Result<NetworkId> {
        let id = NetworkId(self.next_network_id);
        self.next_network_id += 1;
        let mut net = ContainerNetwork::new(id, name);
        net.cni = cni;
        self.networks.insert(id.0, net);
        Ok(id)
    }

    pub fn remove_network(&mut self, id: NetworkId) -> Result<()> {
        self.networks
            .remove(&id.0)
            .ok_or(ContainerError::NotFound)?;
        Ok(())
    }

    pub fn attach_network(
        &mut self,
        container_id: ContainerId,
        network_id: NetworkId,
    ) -> Result<()> {
        let net = self
            .networks
            .get(&network_id.0)
            .ok_or(ContainerError::NotFound)?
            .clone();
        let container = self
            .containers
            .get_mut(&container_id.0)
            .ok_or(ContainerError::NotFound)?;
        container.network = Some(net);
        Ok(())
    }

    pub fn add_registry(&mut self, config: RegistryConfig) {
        self.registries.push(config);
    }

    pub fn pull_image(&mut self, name: &str, tag: &str) -> Result<ImageId> {
        let registry =
            self.registries.first().cloned().unwrap_or_else(|| {
                RegistryConfig::new(String::from("https://registry.raeenos.dev"))
            });
        self.image_manager.pull_image(&registry, name, tag)
    }

    pub fn container_count(&self) -> usize {
        self.containers.len()
    }

    pub fn running_count(&self) -> usize {
        self.containers
            .values()
            .filter(|c| c.state == ContainerState::Running)
            .count()
    }

    pub fn total_created() -> u64 {
        TOTAL_CONTAINERS_CREATED.load(Ordering::Relaxed)
    }
}

// ─── Global Runtime ──────────────────────────────────────────────────────────

static mut CONTAINER_RUNTIME: Option<ContainerRuntime> = None;

pub fn init() {
    if RUNTIME_INITIALIZED.swap(true, Ordering::SeqCst) {
        return;
    }
    unsafe {
        CONTAINER_RUNTIME = Some(ContainerRuntime::new());
    }
}

pub fn runtime() -> &'static ContainerRuntime {
    unsafe {
        CONTAINER_RUNTIME
            .as_ref()
            .expect("container runtime not initialized")
    }
}

pub fn runtime_mut() -> &'static mut ContainerRuntime {
    unsafe {
        CONTAINER_RUNTIME
            .as_mut()
            .expect("container runtime not initialized")
    }
}

#[cfg(test)]
mod crypto_tests {
    use super::*;

    #[test]
    fn sha256_hex_is_real() {
        // OCI digests must be genuine SHA-256: FIPS 180-4 SHA-256("abc").
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        // Distinct inputs produce distinct digests (the stub collided trivially).
        assert_ne!(sha256_hex(b"layer-a"), sha256_hex(b"layer-b"));
    }
}
