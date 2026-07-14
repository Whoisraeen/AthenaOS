# AthenaOS capability table

**Authoritative source.** Required by `kernelchecklist.md` R10: every new
`Cap` flavor MUST add a row here in the same commit.

Concept doc §Security:

> "Capability-based permissions — apps request capabilities (file access,
>  camera, mic, network), user grants, OS enforces at the syscall layer."

The 14-variant `Cap` enum in `kernel/src/capability.rs` is the **complete
authority list**. Adding a 15th variant requires updating this file and
`docs/SYSCALL_TABLE.md` (which syscalls accept it) in the same PR.

## Rights bitset

8-bit field, applied uniformly across every cap flavor:

| bit | flag         | meaning |
|---:|---|---|
| 0  | `READ`       | read resource state |
| 1  | `WRITE`      | mutate resource state |
| 2  | `EXEC`       | execute / activate (e.g. fire an IRQ vector) |
| 3  | `MAP`        | map into the holder's address space |
| 4  | `WAIT`       | block on an event (IRQ_WAIT, RECV) |
| 5  | `GRANT`      | derive + pass to another process |
| 6  | `REVOKE`     | revoke from another process |
| 7  | reserved     | future |

`Rights::ALL = 0b0111_1111`.

## Cap flavors

| flavor | label        | members | who issues | typical rights |
|---:|---|---|---|---|
| 0  | `Channel`     | `chan_id: u32`                     | IPC subsystem at task spawn      | `READ \| WRITE \| WAIT \| GRANT` |
| 1  | `Mmio`        | `start_phys: u64, len: usize`      | kernel grants to driver supervisors | `READ \| WRITE \| MAP` |
| 2  | `Irq`         | `vector: u8`                       | kernel grants to drivers          | `WAIT` |
| 3  | `Port`        | `base: u16, count: u16`            | kernel grants to legacy drivers   | `READ \| WRITE` |
| 4  | `Filesystem`  | `root_inode: u64`                  | VFS at mount                      | `READ \| WRITE` |
| 5  | `Network`     | `port_range_start, port_range_end: u16` | net stack at socket creation | `READ \| WRITE \| WAIT` |
| 6  | `Gpu`         | `device_id: u32`                   | gpu subsystem                     | `READ \| WRITE \| MAP` |
| 7  | `Audio`       | `device_id: u32`                   | audio subsystem                   | `READ \| WRITE` |
| 8  | `Camera`      | `device_id: u32`                   | media subsystem (user prompt PLANNED — see note) | `READ` |
| 9  | `Process`     | `target_pid: u64`                  | scheduler at fork                 | `READ \| WRITE \| WAIT` |
| 10 | `CryptoKey`   | `key_id: u64`                      | TPM / raeshield                   | `READ \| WRITE` |
| 11 | `Hypervisor`  | `vm_id: u64`                       | VMM (privileged)                  | `READ \| WRITE \| EXEC` |
| 12 | `Attestation` | `session_id: u64`                  | anti-cheat subsystem              | `READ \| WAIT` |
| 13 | `Debug`       | `scope: u32`                       | kernel (root-only)                | `READ` |

> **Enforcement is uneven — see `docs/THREAT_MODEL.md` §4 for the per-flavor
> status.** The derivation/revocation *algebra* below is fully enforced and
> boot-proven, but several flavors are not yet *consulted* at their operation
> sites. In particular `Camera`'s "requires user prompt" is DESIGNED, not wired:
> the kernel prompt queue (`perm_prompt.rs`) has no compositor UI consumer, so
> holding a Camera cap is not currently gated on live user consent. Do not treat
> the "who issues" column as a guarantee of mint-time consent until §4 says
> ENFORCED for that flavor.

## Derivation rules

`grant(parent, derived)` succeeds only if:

1. `parent.rights().contains(Rights::GRANT)`.
2. `derived` is the **same flavor** as `parent`.
3. `derived.rights().is_subset_of(parent.rights())` — you cannot grant more
   than you hold.
4. Flavor-specific narrowing:
   - `Mmio`: derived range must be inside parent range.
   - `Port`: derived port window must be inside parent's.
   - `Network`: derived port range must be inside parent's.
   - `Filesystem`: derived root_inode must be a child of parent's.
   - Others: only rights narrow.

`revoke(target, handle)` succeeds only if the calling task is the one
recorded in `GrantRecord::granter_task` for that target handle. Revocation
is **transitive**: if target had `GRANT` and passed the cap on, every
descendant cap is revoked too.

## Audit

Every grant, revoke, query, use, and denial is recorded in a 256-event
ring buffer (`kernel/src/cap_audit.rs`) and visible via
`cat /proc/raeen/caps`. Aggregate counters survive ring wrap.

## Error codes

```rust
pub const E_NO_HANDLE:       u64 = u64::MAX - 1;
pub const E_RIGHTS:          u64 = u64::MAX - 2;
pub const E_INVALID_DERIVE:  u64 = u64::MAX - 3;
pub const E_NO_TASK:         u64 = u64::MAX - 4;
pub const E_WRONG_FLAVOR:    u64 = u64::MAX - 5;
pub const E_INVAL:           u64 = u64::MAX - 6;
```

## Adding a new flavor

1. Edit `kernel/src/capability.rs`: add the `Cap::Foo` variant with its
   members; add a match arm in every internal function that switches on
   `Cap` (validators, granters, revokers).
2. Add a row to this file.
3. Add it to the `flavor_label()` table in `kernel/src/cap_audit.rs`.
4. Note in `docs/SYSCALL_TABLE.md` which syscalls require it.
5. Write a docstring quoting the Concept doc clause that motivated it.

If you can't justify the new flavor from the Concept doc, don't add it —
narrow an existing flavor instead.
