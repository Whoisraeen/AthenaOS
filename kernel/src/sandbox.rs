//! Per-task sandbox enforcement — wires RaeShield's `PolicyEnforcer` into the
//! kernel syscall edge.
//!
//! Concept §Security: "Mandatory app sandboxing — every app runs in its own
//! sandbox by default. Capability-based sandboxing that's invisible to users and
//! predictable to developers." This module is the kernel-side enforcement point:
//! the policy *model* lives in `components/raeshield` (`SandboxPolicy` /
//! `PolicyEnforcer`); this module binds it to live tasks and gates the syscalls
//! that touch userspace-visible state (device claim, DMA, network, install).
//!
//! Design — zero-regression by default:
//!   * Tasks are **Trusted** unless explicitly sandboxed, so `user_init`, the
//!     shell, and existing apps are unaffected (full access, no new denials).
//!   * A fast-path atomic (`SANDBOXED_COUNT`) means the syscall hot path pays
//!     only one relaxed load when no task is sandboxed — the common boot state.
//!   * When an app IS sandboxed (AppSandbox / Strict), the gated syscall classes
//!     are checked against a RaeShield profile and denied with `EPERM` if the
//!     policy says no — and the violation is counted for `/proc/raeen/sandbox`.
//!
//! R10: `init()` + `run_boot_smoketest()` + `/proc/raeen/sandbox` + this docstring.

#![allow(dead_code)]

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::string::String;
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use rae_abi::syscall as abi;
use raeshield::{
    Capability, DeviceKind, Direction, PolicyEnforcer, SandboxPolicy, SandboxProfile,
    SecurityDecision, SyscallRequest, ViolationAction,
};
use spin::Mutex;

/// Enforcement strength applied to a task. Maps to a RaeShield profile.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SandboxLevel {
    /// System/trusted code — full access (user_init, shell, services). Default.
    Trusted,
    /// A normal sandboxed app — limited GPU, no raw device claim, no network by default.
    AppSandbox,
    /// Untrusted / unverified — minimal surface; device/network/spawn denied.
    Strict,
}

impl SandboxLevel {
    fn profile(self) -> SandboxProfile {
        match self {
            SandboxLevel::Trusted => SandboxProfile::SystemService,
            SandboxLevel::AppSandbox => SandboxProfile::Sandboxed,
            SandboxLevel::Strict => SandboxProfile::Untrusted,
        }
    }
}

/// pid -> sandbox level. Absent = Trusted. Guarded; only mutated on spawn/exit.
static TABLE: Mutex<Option<BTreeMap<u64, SandboxLevel>>> = Mutex::new(None);
/// Number of tasks with a non-Trusted level. Fast-path: 0 => skip all checks.
static SANDBOXED_COUNT: AtomicUsize = AtomicUsize::new(0);
static CHECKS: AtomicU64 = AtomicU64::new(0);
static DENIALS: AtomicU64 = AtomicU64::new(0);
static GRANT_ALLOWS: AtomicU64 = AtomicU64::new(0);

/// Per-app permission grants declared in `RaeManifest.toml` (`[permissions]`).
/// Each field maps 1:1 to a gated syscall class (see `class_of`). Grants only
/// take effect at `AppSandbox` level: Trusted needs none, and Strict ignores
/// them (an unverified app must not be able to grant itself anything).
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct Grants {
    pub network: bool,
    pub devices: bool,
    pub install: bool,
}

/// pid -> manifest permission grants. Absent = no grants (all gated classes
/// follow the level's RaeShield profile).
static GRANTS: Mutex<Option<BTreeMap<u64, Grants>>> = Mutex::new(None);

pub fn init() {
    *TABLE.lock() = Some(BTreeMap::new());
    *GRANTS.lock() = Some(BTreeMap::new());
    crate::serial_println!(
        "[sandbox] enforcement online (RaeShield PolicyEnforcer; default level = Trusted)"
    );
}

/// Mark a task as sandboxed at `level`. Called from the spawn path when an app
/// bundle declares a sandbox (Trusted is the implicit default; setting it back
/// to Trusted removes the entry).
pub fn set_task_level(pid: u64, level: SandboxLevel) {
    let mut g = TABLE.lock();
    let Some(map) = g.as_mut() else {
        return;
    };
    let was_sandboxed = map
        .get(&pid)
        .map(|l| *l != SandboxLevel::Trusted)
        .unwrap_or(false);
    if level == SandboxLevel::Trusted {
        if map.remove(&pid).is_some() && was_sandboxed {
            SANDBOXED_COUNT.fetch_sub(1, Ordering::Relaxed);
        }
    } else {
        map.insert(pid, level);
        if !was_sandboxed {
            SANDBOXED_COUNT.fetch_add(1, Ordering::Relaxed);
        }
    }
    crate::serial_println!("[sandbox] task {} level set to {:?}", pid, level);
}

/// Record a task's manifest permission grants. Called from the spawn path
/// alongside `set_task_level` when the app's `RaeManifest.toml` declares a
/// `[permissions]` section. No-op grants (all false) are not stored.
pub fn set_task_grants(pid: u64, grants: Grants) {
    let mut g = GRANTS.lock();
    let Some(map) = g.as_mut() else {
        return;
    };
    if grants == Grants::default() {
        map.remove(&pid);
    } else {
        map.insert(pid, grants);
    }
}

pub fn grants_of(pid: u64) -> Grants {
    let g = GRANTS.lock();
    g.as_ref()
        .and_then(|m| m.get(&pid).copied())
        .unwrap_or_default()
}

/// Drop a task's sandbox entry on exit.
pub fn forget_task(pid: u64) {
    let mut g = TABLE.lock();
    if let Some(map) = g.as_mut() {
        if let Some(l) = map.remove(&pid) {
            if l != SandboxLevel::Trusted {
                SANDBOXED_COUNT.fetch_sub(1, Ordering::Relaxed);
            }
        }
    }
    drop(g);
    let mut g = GRANTS.lock();
    if let Some(map) = g.as_mut() {
        map.remove(&pid);
    }
}

pub fn level_of(pid: u64) -> SandboxLevel {
    let g = TABLE.lock();
    g.as_ref()
        .and_then(|m| m.get(&pid).copied())
        .unwrap_or(SandboxLevel::Trusted)
}

/// First-party system apps + daemons that run Trusted (full access). Everything
/// else launched from the shell is sandboxed by default per Concept §Security
/// ("every app runs in its own sandbox by default"). The richer per-app
/// `RaeManifest.toml` sandbox declaration is the next increment; this allowlist
/// is the safe default until manifests are wired.
const TRUSTED_APPS: &[&str] = &[
    "user_init",
    "rae-sh",
    "raeshell",
    "driver_supervisor",
    "amdgpud",
    "i915d",
    "nvidiad",
    "raeinstaller", // needs Cap::System anyway; gated separately
    "settings",
    "task_mgr",
    "files",
    "terminal",
    "text_editor",
    "kanata_daemon",
];

/// Classify an app (by its initramfs/VFS basename) into a launch sandbox level.
/// Trusted first-party apps run unrestricted; unknown/sideloaded apps get
/// AppSandbox, which only denies the raw device/network/install syscall classes
/// (normal apps use VFS/IPC, which are not gated, so they are unaffected).
pub fn level_for_app(name: &str) -> SandboxLevel {
    let base = name.rsplit('/').next().unwrap_or(name);
    if TRUSTED_APPS.iter().any(|a| *a == base) {
        SandboxLevel::Trusted
    } else {
        SandboxLevel::AppSandbox
    }
}

/// The gated syscall classes. Each maps 1:1 to a `Grants` field so a manifest
/// `[permissions]` declaration grants exactly one class.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum GateClass {
    Device,
    Network,
    Install,
}

/// Map a syscall number to its gate class. Returns `None` for syscalls that
/// don't touch sandbox-relevant userspace state (those are always allowed at
/// this layer; capability checks still apply elsewhere).
fn class_of(nr: u64) -> Option<GateClass> {
    match nr {
        // Raw device claim / DMA / driver registration → device access. ALL of
        // the userspace-driver framework's privileged syscalls are gated here so
        // a sandboxed (Strict / un-granted AppSandbox) task can touch none of
        // them — including IRQ-delivery setup and DMA TEARDOWN, which were
        // previously ungated (only reachable post-claim, but defense-in-depth +
        // consistency: every driver-framework privileged op gates identically).
        n if n == abi::SYS_DRIVER_REGISTER
            || n == abi::SYS_DRIVER_CLAIM_DEVICE
            || n == abi::SYS_DRIVER_ENABLE_DMA
            || n == abi::SYS_DRIVER_IRQ_SETUP
            || n == abi::SYS_DRIVER_DMA_MAP
            || n == abi::SYS_DRIVER_DMA_UNMAP
            || n == abi::SYS_LINUXKPI_PCI_ENABLE =>
        {
            Some(GateClass::Device)
        }
        // Userspace network sockets (RaeShield net block 121-125).
        121 | 122 => Some(GateClass::Network),
        // Install onto a disk is a privileged device write.
        n if n == abi::SYS_INSTALL_RUN || n == abi::SYS_INSTALL_CREATE_ACCOUNT => {
            Some(GateClass::Install)
        }
        _ => None,
    }
}

/// Map a PCI class code (the high byte of the device's class:subclass) to the
/// RaeShield [`DeviceKind`] the gate evaluates a raw claim against. This is the
/// fix for the device-kind aliasing flaw: every gated device syscall used to
/// build `DeviceKind::Gpu`, so the gate asked "may this task write the GPU?"
/// for a NIC / storage / USB claim alike — a profile that ever granted
/// `allow_gpu(ReadWrite)` (e.g. a game) would then also pass a raw NIC or disk
/// claim. Threading the claimed device's real PCI class through `request_for`
/// makes the gate correct-by-construction.
///
/// PCI base-class codes (PCI spec §D): 0x01 mass storage, 0x02 network,
/// 0x03 display, 0x04 multimedia (audio/video), 0x0C serial bus (USB is
/// subclass 0x03 but we gate at base-class granularity → `Usb` for the whole
/// serial-bus class). Anything else maps to the most-restrictive `Other`.
fn class_of_device(pci_class: u8) -> DeviceKind {
    match pci_class {
        0x01 => DeviceKind::Storage,
        0x02 => DeviceKind::Nic,
        0x03 => DeviceKind::Gpu,
        0x04 => DeviceKind::Audio,
        0x0C => DeviceKind::Usb,
        _ => DeviceKind::Other,
    }
}

/// Look up the [`DeviceKind`] of a device-claim target from its opaque
/// `device_id` (a packed PCI BDF for the only claim path that exists today).
/// Returns `Other` — the most-restrictive default — when the id is not a PCI
/// BDF or the device is not in the enumeration (a claim of an unknown device
/// must not be evaluated as some permissive kind). This is the safe default
/// the task calls for: never the old wrong-by-construction `Gpu` hardcode.
fn device_kind_of_id(device_id: u64) -> DeviceKind {
    // BDF triples fit in bits 0..24; larger ids are USB/platform (no PCI class
    // available) → most-restrictive.
    if device_id >> 24 != 0 {
        return DeviceKind::Other;
    }
    let bus = ((device_id >> 16) & 0xFF) as u8;
    let dev = ((device_id >> 8) & 0xFF) as u8;
    let func = (device_id & 0xFF) as u8;
    crate::pci::enumerate()
        .iter()
        .find(|d| d.bus == bus && d.device == dev && d.function == func)
        .map(|d| class_of_device(d.class))
        .unwrap_or(DeviceKind::Other)
}

/// The RaeShield request a gate class represents, for the PolicyEnforcer.
/// For the Device class the caller supplies the SPECIFIC [`DeviceKind`] of the
/// claimed device (see `class_of_device`); the old hardcoded `Gpu` aliased
/// every device claim to the GPU rule.
fn request_for(class: GateClass, dev_kind: DeviceKind) -> SyscallRequest {
    match class {
        GateClass::Device => SyscallRequest::DeviceAccess {
            kind: dev_kind,
            write: true,
        },
        // BUG-40: install must NOT alias to device access. A sandboxed game's
        // profile may permit GPU device access; aliasing install to
        // DeviceAccess{Gpu} would then let that game write the raw disk / run
        // the installer through the fallback path. Map it to a system-config
        // capability request instead — denied unless the profile actually holds
        // Capability::SystemConfig (Sandboxed/Untrusted profiles do not).
        GateClass::Install => SyscallRequest::CapabilityRequest {
            cap: Capability::SystemConfig,
        },
        GateClass::Network => SyscallRequest::NetworkConnect {
            port: 0,
            direction: Direction::Outbound,
        },
    }
}

/// Linux x86_64 syscall numbers → gate class. The Linux translation table
/// (`linux_syscall.rs`) is a SECOND door into kernel state: a Linux-ABI task
/// never issues native numbers, so gating only `class_of` left every
/// sandboxed Linux binary ungated (found 2026-06-10 — the dispatch also
/// returned before the gate; see syscall_handler_inner). Today the Linux
/// table exposes only the network class (sockets); any future Linux handler
/// that touches devices or install MUST add its number here in the same
/// commit.
fn class_of_linux(nr: u64) -> Option<GateClass> {
    match nr {
        // The full x86_64 socket-call block is one contiguous range, all of it
        // network class: 41 socket, 42 connect, 43 accept, 44 sendto,
        // 45 recvfrom, 46 sendmsg, 47 recvmsg, 48 shutdown, 49 bind, 50 listen,
        // 51 getsockname, 52 getpeername, 53 socketpair, 54 setsockopt,
        // 55 getsockopt. Gating only 41..=45|49|50 (the old set) let a
        // sandboxed Linux task move data over an established socket via
        // sendmsg/recvmsg/shutdown without ever crossing the Network gate.
        41..=55 => Some(GateClass::Network),
        _ => None,
    }
}

/// The syscall-edge gate. Returns `true` if the calling task may proceed.
/// Fast path: a single relaxed load when no task is sandboxed.
///
/// Device-claim syscalls (`SYS_DRIVER_CLAIM_DEVICE`) carry the target's opaque
/// device id (packed PCI BDF) in their first argument; pass it via
/// [`check_syscall_dev`] so the gate evaluates the claim against the device's
/// SPECIFIC kind. This bare entry point has no device context, so it uses the
/// most-restrictive `Other` kind for any device-class syscall — fail-closed.
pub fn check_syscall(pid: u64, nr: u64) -> bool {
    gate(pid, class_of(nr), nr, "native", DeviceKind::Other)
}

/// Device-aware variant of [`check_syscall`]: `device_id` is the
/// `SYS_DRIVER_CLAIM_DEVICE` target id (the packed PCI BDF passed in the
/// syscall's `rsi`). Its real PCI class selects the [`DeviceKind`] the gate
/// checks; for every non-claim syscall the kind is unused (the gate only
/// consults it for the Device class).
pub fn check_syscall_dev(pid: u64, nr: u64, device_id: u64) -> bool {
    let dev_kind = if nr == abi::SYS_DRIVER_CLAIM_DEVICE {
        device_kind_of_id(device_id)
    } else {
        DeviceKind::Other
    };
    gate(pid, class_of(nr), nr, "native", dev_kind)
}

/// Same gate for tasks dispatching through the Linux syscall table. The Linux
/// table exposes no device-claim syscall today, so no device id is threaded.
pub fn check_linux_syscall(pid: u64, nr: u64) -> bool {
    gate(pid, class_of_linux(nr), nr, "linux", DeviceKind::Other)
}

fn gate(
    pid: u64,
    class: Option<GateClass>,
    nr: u64,
    abi: &'static str,
    dev_kind: DeviceKind,
) -> bool {
    if SANDBOXED_COUNT.load(Ordering::Relaxed) == 0 {
        return true; // no sandboxed tasks — common boot state, zero overhead
    }
    let level = level_of(pid);
    if level == SandboxLevel::Trusted {
        return true;
    }
    let Some(class) = class else {
        return true; // not a sandbox-gated syscall
    };
    CHECKS.fetch_add(1, Ordering::Relaxed);
    // Manifest permission grant: an AppSandbox task whose RaeManifest.toml
    // declared this class (`[permissions] network/devices/install = true`)
    // passes the gate — that is the grant the manifest surface exists to
    // express. Strict tasks never receive grants (unverified posture,
    // fail-close), and Trusted never reaches here.
    if level == SandboxLevel::AppSandbox {
        let g = grants_of(pid);
        let granted = match class {
            GateClass::Device => g.devices,
            GateClass::Network => g.network,
            GateClass::Install => g.install,
        };
        if granted {
            GRANT_ALLOWS.fetch_add(1, Ordering::Relaxed);
            return true;
        }
    }
    let request = request_for(class, dev_kind);
    let now = crate::timers::JIFFIES.load(Ordering::Relaxed);
    let mut enforcer = PolicyEnforcer::new(
        SandboxPolicy::from_profile(level.profile()),
        ViolationAction::Deny,
    );
    match enforcer.check(&request, pid, now) {
        SecurityDecision::Allowed => true,
        SecurityDecision::Denied => {
            DENIALS.fetch_add(1, Ordering::Relaxed);
            crate::serial_println!(
                "[sandbox] DENY pid={} syscall={} abi={} level={:?} (RaeShield policy)",
                pid,
                nr,
                abi,
                level
            );
            false
        }
    }
}

pub fn run_boot_smoketest() {
    // Use synthetic pids so we never disturb real tasks.
    let strict_pid: u64 = 0x5A_FE_00_01;
    let trusted_pid: u64 = 0x5A_FE_00_02;

    set_task_level(strict_pid, SandboxLevel::Strict);

    // A Strict task must be DENIED a raw device claim…
    let strict_denied = !check_syscall(strict_pid, abi::SYS_DRIVER_CLAIM_DEVICE);
    // …while a Trusted (default, untracked) task is ALLOWED the same syscall…
    let trusted_allowed = check_syscall(trusted_pid, abi::SYS_DRIVER_CLAIM_DEVICE);
    // …and a non-gated syscall (SYS_PRINT) is allowed even when Strict.
    let benign_allowed = check_syscall(strict_pid, abi::SYS_PRINT);

    // Linux-ABI door: the same Strict task must be DENIED a Linux socket(41)
    // through check_linux_syscall (this gate was bypassed entirely until
    // 2026-06-10 — the dispatcher returned before the sandbox check), while
    // a benign Linux write(1) stays allowed.
    let linux_strict_denied = !check_linux_syscall(strict_pid, 41);
    let linux_benign_allowed = check_linux_syscall(strict_pid, 1);

    // Clean up the synthetic entry.
    forget_task(strict_pid);

    // Launch classifier: a first-party app is Trusted; an unknown app is sandboxed.
    let firstparty_trusted = level_for_app("terminal") == SandboxLevel::Trusted;
    let unknown_sandboxed = level_for_app("totally_unknown_app") == SandboxLevel::AppSandbox;

    // Coverage audit (MasterChecklist 4.10): EVERY privileged userspace-driver
    // syscall must classify as a gated Device op, so a sandboxed (Strict /
    // un-granted AppSandbox) task can reach none of the device-claim / DMA /
    // IRQ-setup surface. Falsifiable — a new driver syscall added without gating
    // (or a typo'd const) flips this to FAIL.
    let driver_priv = [
        abi::SYS_DRIVER_REGISTER,
        abi::SYS_DRIVER_CLAIM_DEVICE,
        abi::SYS_DRIVER_ENABLE_DMA,
        abi::SYS_DRIVER_IRQ_SETUP,
        abi::SYS_DRIVER_DMA_MAP,
        abi::SYS_DRIVER_DMA_UNMAP,
        abi::SYS_LINUXKPI_PCI_ENABLE,
    ];
    let driver_coverage = driver_priv
        .iter()
        .all(|&nr| class_of(nr) == Some(GateClass::Device));

    // Device-kind gate (the aliasing-flaw regression test). Build a profile
    // that grants GPU ReadWrite (a game-style policy — the exact case that
    // exposed the bug) and confirm the gate now DISTINGUISHES device kinds:
    // a GPU claim is ALLOWED, but a NIC or Storage claim through the SAME
    // profile is DENIED. The old hardcoded `DeviceKind::Gpu` request aliased
    // every device claim to the GPU rule, so this profile would have let a raw
    // NIC / disk claim slip through. Falsifiable: if `request_for` ever stops
    // threading the real kind, nic/storage flip to ALLOWED and this FAILs.
    let gpu_rw_policy = SandboxPolicy::builder()
        .allow_gpu(raeshield::AccessMode::ReadWrite)
        .build();
    let now = crate::timers::JIFFIES.load(Ordering::Relaxed);
    let decides = |kind: DeviceKind| -> bool {
        let mut enforcer = PolicyEnforcer::new(gpu_rw_policy.clone(), ViolationAction::Deny);
        enforcer.check(&request_for(GateClass::Device, kind), 0xD37, now)
            == SecurityDecision::Allowed
    };
    let gpu_rw_allowed = decides(DeviceKind::Gpu);
    let nic_denied = !decides(DeviceKind::Nic);
    let storage_denied = !decides(DeviceKind::Storage);
    let device_kind_gate = gpu_rw_allowed && nic_denied && storage_denied;

    // PCI class → DeviceKind mapping must be correct (a typo here would let the
    // wrong rule decide a claim). Spot-check the load-bearing classes.
    let class_map_ok = class_of_device(0x02) == DeviceKind::Nic
        && class_of_device(0x01) == DeviceKind::Storage
        && class_of_device(0x03) == DeviceKind::Gpu
        && class_of_device(0xFF) == DeviceKind::Other;

    let pass = strict_denied
        && trusted_allowed
        && benign_allowed
        && linux_strict_denied
        && linux_benign_allowed
        && firstparty_trusted
        && unknown_sandboxed
        && driver_coverage
        && device_kind_gate
        && class_map_ok;
    crate::serial_println!(
        "[sandbox] run_boot_smoketest: strict_deny={} trusted_allow={} benign_allow={} linux_gate={} app_classify={} driver_priv_gated={} -> {}",
        strict_denied,
        trusted_allowed,
        benign_allowed,
        linux_strict_denied && linux_benign_allowed,
        firstparty_trusted && unknown_sandboxed,
        driver_coverage,
        if pass { "PASS" } else { "FAIL" }
    );
    crate::serial_println!(
        "[sandbox] device-kind gate: gpu_rw_allowed={} nic_denied={} storage_denied={} class_map={} -> {}",
        gpu_rw_allowed,
        nic_denied,
        storage_denied,
        class_map_ok,
        if device_kind_gate && class_map_ok {
            "PASS"
        } else {
            "FAIL"
        }
    );
}

pub fn dump_text() -> String {
    let count = SANDBOXED_COUNT.load(Ordering::Relaxed);
    let checks = CHECKS.load(Ordering::Relaxed);
    let denials = DENIALS.load(Ordering::Relaxed);
    let grant_allows = GRANT_ALLOWS.load(Ordering::Relaxed);
    let mut out = String::from("# RaeenOS sandbox enforcement (RaeShield PolicyEnforcer)\n");
    out.push_str(&alloc::format!(
        "sandboxed_tasks: {}\nchecks: {}\ndenials: {}\ngrant_allows: {}\ndefault_level: Trusted\n",
        count,
        checks,
        denials,
        grant_allows,
    ));
    let g = TABLE.lock();
    if let Some(map) = g.as_ref() {
        for (pid, level) in map.iter() {
            let grants = grants_of(*pid);
            if grants == Grants::default() {
                out.push_str(&alloc::format!("  pid {} -> {:?}\n", pid, level));
            } else {
                out.push_str(&alloc::format!(
                    "  pid {} -> {:?} (grants: net={} dev={} install={})\n",
                    pid,
                    level,
                    grants.network,
                    grants.devices,
                    grants.install,
                ));
            }
        }
    }
    out
}
