//! Capability system — unforgeable resource tokens for dynamic drivers.
//!
//! See `docs/design/capabilities.md` for the full design.
//!
//! Every `Task` owns a `CapTable`. Each entry maps an opaque `CapHandle`
//! to a `Cap` describing some resource (IPC channel, MMIO region, IRQ
//! vector, or I/O port range). Handles are kernel-managed, per-task, and
//! impossible to forge from userspace.
//!
//! A cap that carries the `GRANT` right may be *derived* — its holder
//! may pass a strict subset of its authority to another task. This is
//! how a `driver_supervisor` task hands a NIC driver permission to touch
//! exactly the NIC's MMIO window and exactly the NIC's IRQ vector — and
//! nothing else.

use alloc::collections::BTreeMap;

use crate::task::TaskId;

// ── Rights bitset ────────────────────────────────────────────────────────

/// Capability rights bitset. Hand-rolled because we don't want a third-party
/// `bitflags` dependency inside the kernel for one type.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Rights(u32);

impl Rights {
    pub const NONE: Rights = Rights(0);
    pub const READ: Rights = Rights(1 << 0);
    pub const WRITE: Rights = Rights(1 << 1);
    pub const EXEC: Rights = Rights(1 << 2);
    pub const MAP: Rights = Rights(1 << 3);
    pub const WAIT: Rights = Rights(1 << 4);
    pub const GRANT: Rights = Rights(1 << 5);
    pub const REVOKE: Rights = Rights(1 << 6);

    /// All seven defined bits.
    pub const ALL: Rights = Rights(0b0111_1111);

    pub const fn bits(self) -> u32 {
        self.0
    }

    /// Drops any bits we don't recognise — userspace can't smuggle
    /// undefined rights through `cap_grant`.
    pub const fn from_bits_truncate(b: u32) -> Rights {
        Rights(b & Self::ALL.0)
    }

    pub const fn contains(self, other: Rights) -> bool {
        (self.0 & other.0) == other.0
    }

    /// `self ⊆ other` — every bit set in `self` is also set in `other`.
    /// Used to enforce "derived cap must be ≤ parent".
    pub const fn is_subset_of(self, other: Rights) -> bool {
        (self.0 & !other.0) == 0
    }
}

impl core::ops::BitOr for Rights {
    type Output = Rights;
    fn bitor(self, rhs: Rights) -> Rights {
        Rights(self.0 | rhs.0)
    }
}
impl core::ops::BitAnd for Rights {
    type Output = Rights;
    fn bitand(self, rhs: Rights) -> Rights {
        Rights(self.0 & rhs.0)
    }
}

// ── Cap enum ─────────────────────────────────────────────────────────────

/// One unforgeable token referring to one resource.
///
/// RaeShield model: every privileged operation requires holding the
/// appropriate Cap variant with the correct Rights. No parallel
/// access-control systems — this is the single authority source.
#[derive(Clone, Copy, Debug)]
pub enum Cap {
    /// An IPC endpoint. The holder may `send` / `recv` per `rights`.
    Channel { chan_id: u32, rights: Rights },

    /// A physical MMIO region (page-aligned start, length in bytes).
    /// `MAP` right is required to actually paste this into a PML4.
    Mmio {
        start_phys: u64,
        len: usize,
        rights: Rights,
    },

    /// The right to receive interrupts on a specific vector.
    /// `WAIT` right is required to `sys_irq_wait`.
    Irq { vector: u8, rights: Rights },

    /// Legacy x86 I/O port range.
    Port {
        base: u16,
        count: u16,
        rights: Rights,
    },

    // ── RaeShield extended capabilities ─────────────────────────────
    /// Filesystem access scoped to a subtree.
    Filesystem { root_inode: u64, rights: Rights },

    /// Network access: socket creation, listen, connect.
    Network {
        port_range_start: u16,
        port_range_end: u16,
        rights: Rights,
    },

    /// GPU/display access for a specific device.
    Gpu { device_id: u32, rights: Rights },

    /// Audio device access (mixer, capture, playback).
    Audio { device_id: u32, rights: Rights },

    /// Camera / video capture device.
    Camera { device_id: u32, rights: Rights },

    /// Process management (spawn, signal, inspect).
    Process { target_pid: u64, rights: Rights },

    /// Crypto key material access.
    CryptoKey { key_id: u64, rights: Rights },

    /// VM / hypervisor operations.
    Hypervisor { vm_id: u64, rights: Rights },

    /// Anti-cheat attestation sessions.
    Attestation { session_id: u64, rights: Rights },

    /// Debug/profiling (replaces ftrace/kprobes capability gate).
    Debug { scope: u32, rights: Rights },

    /// System control (shutdown, reboot, time).
    System { rights: Rights },

    /// Screen capture at the compositor (screenshots, Game Bar overlay,
    /// recording). Privacy-sensitive: the holder may read composited front-buffer
    /// pixels off the screen. Gates `SYS_CAPTURE_START`. Appended at the END of
    /// the enum (fresh `flavor_id` 16) so adding it is ADDITIVE — the Cap wire
    /// contract is flavor-tag-serialized (`flavor_id`), never index/bit-packed,
    /// so a new tail variant breaks nothing. (Concept §creators: "capture &
    /// stream at the compositor, zero-cost".)
    ScreenCapture { rights: Rights },

    /// Assistive-technology access to the accessibility tree. The holder may
    /// READ the (kernel-owned, window-tier) a11y tree (`SYS_A11Y_SNAPSHOT`) and,
    /// with WRITE, dispatch focus/activate/scroll/set-value actions to nodes
    /// (`SYS_A11Y_ACTION`). Privileged because an AT client reads OTHER apps' UI
    /// structure + labels and can drive their widgets — the analogue of macOS
    /// TCC Accessibility / Windows UIA. Appended at the END of the enum (fresh
    /// `flavor_id` 17) so adding it is ADDITIVE — the `Cap` wire contract is
    /// flavor-tag-serialized (`flavor_id`), never index/bit-packed, so a new
    /// tail variant breaks nothing (NO `ABI_VERSION` bump; identical reasoning
    /// to `ScreenCapture` above). (Concept §Security: "OS enforces capabilities
    /// at the syscall layer; no app reads another's UI tree unprompted.")
    Accessibility { rights: Rights },
}

impl Cap {
    pub const fn rights(&self) -> Rights {
        match *self {
            Cap::Channel { rights, .. } => rights,
            Cap::Mmio { rights, .. } => rights,
            Cap::Irq { rights, .. } => rights,
            Cap::Port { rights, .. } => rights,
            Cap::Filesystem { rights, .. } => rights,
            Cap::Network { rights, .. } => rights,
            Cap::Gpu { rights, .. } => rights,
            Cap::Audio { rights, .. } => rights,
            Cap::Camera { rights, .. } => rights,
            Cap::Process { rights, .. } => rights,
            Cap::CryptoKey { rights, .. } => rights,
            Cap::Hypervisor { rights, .. } => rights,
            Cap::Attestation { rights, .. } => rights,
            Cap::Debug { rights, .. } => rights,
            Cap::System { rights, .. } => rights,
            Cap::ScreenCapture { rights, .. } => rights,
            Cap::Accessibility { rights, .. } => rights,
        }
    }

    /// Tag of the cap flavor, for `SYS_CAP_QUERY`.
    pub const fn flavor_id(&self) -> u32 {
        match self {
            Cap::Channel { .. } => 1,
            Cap::Mmio { .. } => 2,
            Cap::Irq { .. } => 3,
            Cap::Port { .. } => 4,
            Cap::Filesystem { .. } => 5,
            Cap::Network { .. } => 6,
            Cap::Gpu { .. } => 7,
            Cap::Audio { .. } => 8,
            Cap::Camera { .. } => 9,
            Cap::Process { .. } => 10,
            Cap::CryptoKey { .. } => 11,
            Cap::Hypervisor { .. } => 12,
            Cap::Attestation { .. } => 13,
            Cap::Debug { .. } => 14,
            Cap::System { .. } => 15,
            Cap::ScreenCapture { .. } => 16,
            Cap::Accessibility { .. } => 17,
        }
    }
}

// ── Handles ──────────────────────────────────────────────────────────────

/// Opaque per-task identifier for a cap slot. Userspace sees this as a `u64`
/// with no internal structure; only the kernel resolves it to a `Cap`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct CapHandle(pub u64);

impl CapHandle {
    pub const fn raw(self) -> u64 {
        self.0
    }
    pub const fn from_raw(n: u64) -> Self {
        CapHandle(n)
    }
}

/// Records who granted us a capability, so revoke can chase children.
#[derive(Clone, Copy, Debug)]
pub struct GrantRecord {
    pub granter_task: TaskId,
    pub granter_handle: CapHandle,
}

// ── Per-task CapTable ────────────────────────────────────────────────────

#[derive(Debug)]
pub struct CapTable {
    slots: BTreeMap<CapHandle, Cap>,
    /// For every handle we know was granted by another task, remember the
    /// grant chain so `cap_revoke` can find children.
    parents: BTreeMap<CapHandle, GrantRecord>,
    next: u64,
}

impl CapTable {
    pub const fn new() -> Self {
        CapTable {
            slots: BTreeMap::new(),
            parents: BTreeMap::new(),
            next: 1,
        }
    }

    /// Insert a top-level cap (no parent — kernel-minted, e.g. for the
    /// `driver_supervisor` at boot).
    pub fn insert_root(&mut self, cap: Cap) -> CapHandle {
        let h = CapHandle(self.next);
        self.next += 1;
        self.slots.insert(h, cap);
        h
    }

    /// Insert a derived cap with its grant record.
    pub fn insert_derived(&mut self, cap: Cap, parent: GrantRecord) -> CapHandle {
        let h = self.insert_root(cap);
        self.parents.insert(h, parent);
        h
    }

    pub fn get(&self, h: CapHandle) -> Option<Cap> {
        self.slots.get(&h).copied()
    }

    pub fn parent_of(&self, h: CapHandle) -> Option<GrantRecord> {
        self.parents.get(&h).copied()
    }

    pub fn remove(&mut self, h: CapHandle) -> Option<Cap> {
        self.parents.remove(&h);
        self.slots.remove(&h)
    }

    pub fn iter(&self) -> impl Iterator<Item = (CapHandle, &Cap)> {
        self.slots.iter().map(|(h, c)| (*h, c))
    }

    pub fn iter_parents(&self) -> impl Iterator<Item = (CapHandle, &GrantRecord)> {
        self.parents.iter().map(|(h, p)| (*h, p))
    }

    pub fn len(&self) -> usize {
        self.slots.len()
    }
}

impl Default for CapTable {
    fn default() -> Self {
        Self::new()
    }
}

// ── Errors ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub enum CapError {
    NoSuchHandle,
    InsufficientRights,
    /// Requested cap is not a subset of the parent.
    InvalidDerivation,
    /// Target TaskId doesn't exist.
    NoSuchTask,
    /// Tried to derive a Cap of a different flavor than the parent.
    WrongFlavor,
}

// Userspace-visible u64 error codes, returned in rax by the syscall handler.
// Picked from the top so they don't collide with the `0..N` success space.
pub const E_NO_HANDLE: u64 = u64::MAX - 1;
pub const E_RIGHTS: u64 = u64::MAX - 2;
pub const E_INVALID_DERIVE: u64 = u64::MAX - 3;
pub const E_NO_TASK: u64 = u64::MAX - 4;
pub const E_WRONG_FLAVOR: u64 = u64::MAX - 5;
/// Generic "invalid argument" — out-of-range port, unaligned vaddr, etc.
pub const E_INVAL: u64 = u64::MAX - 6;

impl CapError {
    pub const fn as_u64(self) -> u64 {
        match self {
            CapError::NoSuchHandle => E_NO_HANDLE,
            CapError::InsufficientRights => E_RIGHTS,
            CapError::InvalidDerivation => E_INVALID_DERIVE,
            CapError::NoSuchTask => E_NO_TASK,
            CapError::WrongFlavor => E_WRONG_FLAVOR,
        }
    }
}

// ── Derivation rules ─────────────────────────────────────────────────────

/// Is `child` a legal derivation of `parent`?
///
/// Rules per docs/design/capabilities.md:
/// - Same flavor (Channel/Mmio/Irq/Port — no flavor-swapping).
/// - Same underlying resource (chan_id / vector / Mmio sub-range / Port sub-range).
/// - `child.rights ⊆ parent.rights`.
pub fn is_valid_derivation(parent: &Cap, child: &Cap) -> bool {
    match (*parent, *child) {
        (
            Cap::Channel {
                chan_id: a,
                rights: ra,
            },
            Cap::Channel {
                chan_id: b,
                rights: rb,
            },
        ) => a == b && rb.is_subset_of(ra),
        (
            Cap::Mmio {
                start_phys: pa,
                len: la,
                rights: ra,
            },
            Cap::Mmio {
                start_phys: pb,
                len: lb,
                rights: rb,
            },
        ) => {
            let pa_end = pa.saturating_add(la as u64);
            let pb_end = pb.saturating_add(lb as u64);
            pb >= pa && pb_end <= pa_end && rb.is_subset_of(ra)
        }
        (
            Cap::Irq {
                vector: va,
                rights: ra,
            },
            Cap::Irq {
                vector: vb,
                rights: rb,
            },
        ) => va == vb && rb.is_subset_of(ra),
        (
            Cap::Port {
                base: ba,
                count: ca,
                rights: ra,
            },
            Cap::Port {
                base: bb,
                count: cb,
                rights: rb,
            },
        ) => {
            let ea = ba as u32 + ca as u32;
            let eb = bb as u32 + cb as u32;
            bb >= ba && eb <= ea && rb.is_subset_of(ra)
        }
        (
            Cap::Filesystem {
                root_inode: a,
                rights: ra,
            },
            Cap::Filesystem {
                root_inode: b,
                rights: rb,
            },
        ) => a == b && rb.is_subset_of(ra),
        (
            Cap::Network {
                port_range_start: sa,
                port_range_end: ea,
                rights: ra,
            },
            Cap::Network {
                port_range_start: sb,
                port_range_end: eb,
                rights: rb,
            },
        ) => sb >= sa && eb <= ea && rb.is_subset_of(ra),
        (
            Cap::Gpu {
                device_id: a,
                rights: ra,
            },
            Cap::Gpu {
                device_id: b,
                rights: rb,
            },
        ) => a == b && rb.is_subset_of(ra),
        (
            Cap::Audio {
                device_id: a,
                rights: ra,
            },
            Cap::Audio {
                device_id: b,
                rights: rb,
            },
        ) => a == b && rb.is_subset_of(ra),
        (
            Cap::Camera {
                device_id: a,
                rights: ra,
            },
            Cap::Camera {
                device_id: b,
                rights: rb,
            },
        ) => a == b && rb.is_subset_of(ra),
        (
            Cap::Process {
                target_pid: a,
                rights: ra,
            },
            Cap::Process {
                target_pid: b,
                rights: rb,
            },
        ) => a == b && rb.is_subset_of(ra),
        (
            Cap::CryptoKey {
                key_id: a,
                rights: ra,
            },
            Cap::CryptoKey {
                key_id: b,
                rights: rb,
            },
        ) => a == b && rb.is_subset_of(ra),
        (
            Cap::Hypervisor {
                vm_id: a,
                rights: ra,
            },
            Cap::Hypervisor {
                vm_id: b,
                rights: rb,
            },
        ) => a == b && rb.is_subset_of(ra),
        (
            Cap::Attestation {
                session_id: a,
                rights: ra,
            },
            Cap::Attestation {
                session_id: b,
                rights: rb,
            },
        ) => a == b && rb.is_subset_of(ra),
        (
            Cap::Debug {
                scope: a,
                rights: ra,
            },
            Cap::Debug {
                scope: b,
                rights: rb,
            },
        ) => a == b && rb.is_subset_of(ra),
        (Cap::System { rights: ra }, Cap::System { rights: rb }) => rb.is_subset_of(ra),
        (Cap::ScreenCapture { rights: ra }, Cap::ScreenCapture { rights: rb }) => {
            rb.is_subset_of(ra)
        }
        (Cap::Accessibility { rights: ra }, Cap::Accessibility { rights: rb }) => {
            rb.is_subset_of(ra)
        }
        _ => false, // flavor mismatch
    }
}

// ── Kernel-side API (used by syscall.rs) ─────────────────────────────────

/// Grant a derived cap from `granter`'s handle `src_handle` into `target`.
///
/// Walks the cap table of the granter (must hold the cap with GRANT right),
/// validates the derivation, then inserts into the target's table.
///
/// Returns the new `CapHandle` *in the target task*. Userspace gets that
/// number back in rax; it's only useful to the target (which sees its own
/// table) — the granter typically passes it via IPC.
pub fn grant(
    granter_id: TaskId,
    src_handle: CapHandle,
    target_id: TaskId,
    derived: Cap,
) -> Result<CapHandle, CapError> {
    // 1. Read the parent cap from granter's table.
    let parent = crate::scheduler::with_task_by_id(granter_id, |t| t.cap_table.get(src_handle))
        .ok_or(CapError::NoSuchTask)?
        .ok_or(CapError::NoSuchHandle)?;

    // 2. Authority check.
    if !parent.rights().contains(Rights::GRANT) {
        return Err(CapError::InsufficientRights);
    }

    // 3. Subset check.
    if !is_valid_derivation(&parent, &derived) {
        return Err(CapError::InvalidDerivation);
    }

    // 4. Insert into target.
    let new_handle = crate::scheduler::with_task_by_id(target_id, |t| {
        t.cap_table.insert_derived(
            derived,
            GrantRecord {
                granter_task: granter_id,
                granter_handle: src_handle,
            },
        )
    })
    .ok_or(CapError::NoSuchTask)?;

    Ok(new_handle)
}

/// Revoke a cap from `target`. The caller (`revoker`) must be the original
/// granter recorded on the target's cap.
///
/// Revocation is **recursive**: if `target` held GRANT and passed the cap on,
/// the whole derivation subtree is nuked too. We walk it with an explicit
/// worklist (`to_revoke`), removing each handle and chasing its children via
/// `find_cap_children`, so a revoked authority cannot survive through any
/// descendant it was granted to.
pub fn revoke(
    revoker_id: TaskId,
    target_id: TaskId,
    target_handle: CapHandle,
) -> Result<(), CapError> {
    let parent =
        crate::scheduler::with_task_by_id(target_id, |t| t.cap_table.parent_of(target_handle))
            .ok_or(CapError::NoSuchTask)?
            .ok_or(CapError::NoSuchHandle)?;

    if parent.granter_task != revoker_id {
        return Err(CapError::InsufficientRights);
    }

    let mut to_revoke = alloc::vec::Vec::new();
    to_revoke.push((target_id, target_handle));

    while let Some((curr_task, curr_handle)) = to_revoke.pop() {
        crate::scheduler::with_task_by_id(curr_task, |t| {
            t.cap_table.remove(curr_handle);
        });
        let children = crate::scheduler::find_cap_children(curr_task, curr_handle);
        to_revoke.extend(children);
    }

    Ok(())
}
