# Capability Model — Dynamic Drivers Edition

> Today's state: per-task `CNode` in `ipc.rs` with `Capability { chan_id, rights }`.
> Channel-only. Granted only by kernel-internal API.
>
> This doc specifies the next step: a real capability system that supports
> dynamic drivers via MMIO, IRQ, and port caps, with a userspace
> grant / revoke / derive protocol.

## Why capabilities, not POSIX permissions

The driver story we promised in the concept doc:

> Every driver runs in its own protection domain with IOMMU enforcement.
> A bad GPU driver crashes a service, not the kernel.

POSIX ownership (uid/gid) can't express "this driver may touch *only*
MMIO bytes `0xFED0_0000..=0xFED0_0FFF` and *only* IRQ vector 11". Capabilities
can. Each capability is an unforgeable token referring to a specific resource
with a specific rights mask.

## The four capability flavors

```rust
pub enum Cap {
    /// IPC endpoint. Existing send/recv use these.
    Channel { chan_id: usize, rights: Rights },

    /// A physical MMIO region the holder may map into its address space.
    /// `start_phys` is page-aligned; `len` is a multiple of 4 KiB.
    Mmio { start_phys: u64, len: usize, rights: Rights },

    /// The right to receive interrupts on a specific vector.
    /// The kernel routes IRQs to the holder's bound channel.
    Irq { vector: u8, rights: Rights },

    /// Legacy I/O port range. x86-specific.
    Port { base: u16, count: u16, rights: Rights },
}
```

`Rights` is a bitflag set:

```text
READ      0b0000_0001    may read / recv / observe
WRITE     0b0000_0010    may write / send
EXEC      0b0000_0100    (reserved; future: code-page Mmio caps)
MAP       0b0000_1000    may map an Mmio cap into address space
WAIT      0b0001_0000    may wait on an Irq cap
GRANT     0b0010_0000    may pass derived copies of this cap to other tasks
REVOKE    0b0100_0000    may revoke caps it previously granted
```

`GRANT` and `REVOKE` are the meta-rights that make the system extensible.
Without `GRANT`, a cap is leaf-only.

## Per-task capability table

Each `Task` carries a `CapTable`:

```rust
pub struct CapTable {
    slots: BTreeMap<CapHandle, Cap>,   // handle -> cap
    next: u64,                          // monotonic handle id
    parent_of: BTreeMap<CapHandle, GrantRecord>, // for revoke
}

pub struct GrantRecord {
    granter_task: TaskId,
    granter_handle: CapHandle,
}
```

A `CapHandle` is opaque to userspace — a `u64` that's meaningless without the
kernel's lookup table. Handles are NOT shared globally: handle `5` in task A
has nothing to do with handle `5` in task B. This is the "process descriptor
table" pattern — like file descriptors but for everything.

## Derivation rules

When task `A` calls `cap_grant(target=B, src=h, new=spec)`:

1. **Authority check** — `A` must hold cap `h` with the `GRANT` right.
2. **Subset check** — the new spec must be a strict subset of cap `h`:
   - Same flavor (Channel/Mmio/Irq/Port)
   - Same chan_id / vector / port base (no swapping resources)
   - Mmio region: `new.start >= h.start && new.end <= h.end`
   - Rights: `new.rights ⊆ h.rights`
3. **Insert** — kernel creates a new `Cap` in `B`'s table, returning a new
   `CapHandle` to `A` (or, optionally, deposited directly into `B` via IPC).
4. **Record** — kernel notes `granted_by = (A, h)` in `B`'s `parent_of` map
   so revoke is possible.

## Revocation

`cap_revoke(handle=h_in_B, by=A)`:
- Kernel checks that `A` is recorded as the granter for `h_in_B` (or kernel-root).
- Removes the cap from `B`'s table.
- Recursively revokes any caps `B` granted *derived from* `h_in_B`.
- Future hardening: handle epochs so revocation is O(1) even with deep trees.

This is the seL4 derivation tree pattern — every cap remembers its parent,
revoke walks the children.

## Syscalls

| # | Name | Args (rax / rdi / rsi / rdx / r10 / r8) | Returns |
|--:|------|---------------------------------------|---------|
| 1 | SYS_PRINT      | rdi=value             | u64 ok    |
| 2 | SYS_SEND       | rdi=cap_h, rsi/rdx/r10/r8 = msg | 0 / err |
| 3 | SYS_RECV       | rdi=cap_h             | msg in rsi.. or err |
| 4 | SYS_CAP_GRANT  | rdi=target_task, rsi=src_cap_h, rdx=new_rights, r10=mmio_off, r8=mmio_len | new cap_h in target / err |
| 5 | SYS_CAP_REVOKE | rdi=target_task, rsi=cap_h_in_target | 0 / err |
| 6 | SYS_CAP_QUERY  | rdi=cap_h             | flavor in rax_high, rights in rsi |
| 7 | SYS_MMIO_MAP   | rdi=cap_h, rsi=user_virt_addr | 0 / err |
| 8 | SYS_IRQ_WAIT   | rdi=cap_h             | 0 on IRQ fire / err |

`SYS_MMIO_MAP` is the cap-redemption step: holding an Mmio cap with `MAP`
right, the task asks the kernel to actually map the physical pages into its
PML4 at a chosen user virtual address. `SYS_IRQ_WAIT` blocks the task until
the IRQ fires, with the kernel routing the vector to the holder's IPC endpoint.

## Driver supervisor

A privileged user-space task (`driver_supervisor`) starts at boot with:

- `Mmio { start: 0, len: TOTAL_MMIO_SPACE, rights: R|W|MAP|GRANT }` — covers
  all of MMIO space the firmware reported
- `Irq { vector: 0, rights: R|WAIT|GRANT }` ... one per usable IRQ vector
- `Port { base: 0, count: 0x10000, rights: R|W|GRANT }`

Spawning a driver looks like:

```rust
// inside driver_supervisor (user-space pseudocode)
let driver_pid = spawn_elf(driver_blob);
let nic_mmio = cap_grant(
    target = driver_pid,
    src    = MASTER_MMIO,
    rights = R|W|MAP,                // no GRANT — driver can't re-grant
    range  = 0xfeb00000..0xfec00000, // exact device window
)?;
let nic_irq = cap_grant(target = driver_pid, src = MASTER_IRQ, vector = 0x2b, rights = R|WAIT)?;
ipc_send(driver_pid, BootupChannel, &[nic_mmio, nic_irq])?;
```

The driver receives two opaque handles. It can `SYS_MMIO_MAP` to get
addressable memory, `SYS_IRQ_WAIT` to receive interrupts. It cannot grant
either onward (no `GRANT` right), so a compromise can't spawn a sub-driver
that touches *other* devices.

## Threat model

| Attack | Defense |
|---|---|
| Malicious driver reads disk MMIO | Driver only has the NIC cap; disk MMIO is a different region. |
| Malicious driver re-grants its cap | Driver's cap lacks `GRANT` right. |
| Malicious driver spams IPC | Channel caps have `WRITE` only; backpressure via bounded ring buffers blocks them. |
| Compromised supervisor | Supervisor is itself an isolated user-space task. Compromise it and you have driver authority — but not kernel authority. Kernel can be re-attested via RaeShield's measured-boot chain. |
| Cap forgery | Handles are u64 with no semantics; only kernel translates them. Userspace can't synthesize a valid handle for a target task. |

## What ships this commit

- New `kernel/src/capability.rs` with `Cap`, `Rights`, `CapTable`.
- Refactor `ipc.rs` to use the new types via `Cap::Channel`.
- `task.rs`: `cnode: CNode` → `cap_table: CapTable`.
- `syscall.rs`: add SYS_CAP_GRANT (4), SYS_CAP_REVOKE (5), SYS_CAP_QUERY (6).
  `SYS_MMIO_MAP` and `SYS_IRQ_WAIT` come in the next commit alongside an
  actual driver-supervisor user task.
- Update SYS_SEND / SYS_RECV to require a `Cap::Channel` (other variants → err).

## What does NOT ship this commit

- `SYS_MMIO_MAP` actually mutating the holder's PML4 — the code path exists
  but is feature-gated. Needs a user-virt-address allocator.
- `SYS_IRQ_WAIT` and the kernel's interrupt→IPC routing table.
- A user-space `driver_supervisor` ELF. The kernel still seeds master caps,
  just into a kernel placeholder. Next commit promotes it to user-space.
- IOMMU programming. Required for production but x86 + QEMU testing works
  without it for now. Real-hardware story is VT-d / AMD-Vi.
