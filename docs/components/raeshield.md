# AthGuard

iOS-grade security without iOS-grade lockdown. Capability-based permissions,
sandboxing, attestation.

## Current status

✅ **Per-task `CapTable`** — every `Task` carries an unforgeable handle table.
✅ **Four cap flavors** — `Channel`, `Mmio`, `Irq`, `Port` (see `kernel/src/capability.rs`).
✅ **Rights bitset** — `READ | WRITE | EXEC | MAP | WAIT | GRANT | REVOKE`.
✅ **Derivation rules** — subset-only; same flavor, same underlying resource, `child.rights ⊆ parent.rights`.
✅ **Syscalls 4/5/6** — `SYS_CAP_GRANT`, `SYS_CAP_REVOKE`, `SYS_CAP_QUERY`.
✅ **Boot seeding** — `user_init` starts as proto-`driver_supervisor` holding
   master `Mmio` + `Irq` + `Port` caps.

End-to-end verified: user-space issues `SYS_CAP_QUERY(h)`, kernel returns
flavor and rights through `rsi`/`rdx` — see boot log.

See [docs/design/capabilities.md](../design/capabilities.md) for the full design.

### What landed in the driver-framework slice

✅ **`SYS_MMIO_MAP`** (syscall 7) — holder of a `Cap::Mmio` with the `MAP`
   right can map the cap's physical pages into its own PML4 at a chosen
   user-virt address. PRESENT + WRITABLE + USER + NO_CACHE + WRITE_THROUGH
   flags, full TLB flush after.
✅ **`SYS_IRQ_WAIT`** (syscall 8) — holder of a `Cap::Irq` with `WAIT` blocks
   on the IRQ vector. The keyboard handler (and any future IRQ handler that
   calls `scheduler::unblock_irq_waiters(vec)`) wakes blocked tasks on
   interrupt arrival.
✅ **`SYS_PORT_READ`** / **`SYS_PORT_WRITE`** (syscalls 9, 10) — holder of a
   `Cap::Port` can issue `in`/`out` instructions via the kernel, bounds-checked
   against the cap's port range. No IOPL escalation needed.
✅ **`driver_supervisor` ELF** — separate workspace crate, packed into the
   initramfs alongside `user_init`, spawned by the kernel at boot in place of
   `user_init`. Exercises all four new syscalls on every boot.
✅ **End-to-end demo verified in QEMU**: userspace mapped physical MMIO,
   wrote 4 KiB of pixel data through it, drove COM1 byte-by-byte via
   `SYS_PORT_WRITE` (the resulting `[drvsup] hello from userspace via
   SYS_PORT_WRITE` line appears inline in the kernel's serial log), and
   blocked on the keyboard IRQ.

## Roadmap (next commits)

- A `spawn_elf` syscall so `driver_supervisor` can launch child driver tasks
  and `cap_grant` derived caps to them. That closes the userspace-driver loop.
- Per-IRQ data delivery: when an IRQ fires, deliver the relevant device data
  (scancode, packet header, etc.) into the holder's IPC ring buffer instead
  of just a wake signal.
- IOMMU programming (VT-d / AMD-Vi) so MMIO caps are enforced by hardware,
  not just by the kernel cooperatively. Required for production-grade isolation.
- Measured-boot attestation API for the EAC/BattlEye pitch.
- An x86 `iopb`-based fast path for hot `SYS_PORT_READ/WRITE` so per-byte
  serial drives don't pay a syscall round-trip each.

## Goals

- Apps request capabilities; user grants; kernel enforces at the syscall layer
- Mandatory sandboxing for every app by default; "Trusted app" mode for legacy,
  clearly marked
- Code signing required for app store, optional for sideload (clear "unverified
  developer" UX, not punitive)
- Driver sandboxing — IOMMU-enforced, no exceptions
- Memory tagging on supported CPUs (ARMv8.5 MTE, Intel/AMD as they ship)
- No kernel-level anti-cheat needed: expose an **attestation API** that EAC/BattlEye
  can use without owning ring 0

## Capability model

Capabilities are unforgeable tokens (kernel-managed handles). An app can only
exercise a capability it currently holds. Capabilities can be:

- **Granted** by user (e.g. Camera access via system UI)
- **Derived** from another capability (e.g. a directory cap → a single-file cap)
- **Delegated** across IPC, with the receiver's set strictly ⊆ sender's set

A bad GPU driver crashes a service, not the kernel, because the driver's
capability set scopes its blast radius.

## Anti-cheat attestation pitch

The pitch to EAC and BattlEye, summarized: *you don't need kernel access on
AthenaOS; here's a better, harder-to-bypass primitive.*

The attestation API surfaces:
- Measured boot chain (UEFI → bootloader → kernel → init → compositor)
- Process integrity (binary hash, runtime memory pages signed)
- "Trusted display path" — the rendered frame the user sees is the frame the GPU produced

A kernel-level cheat would have to compromise the measured-boot chain, which
trips remote attestation on the game's backend.

## Open design questions

- Capability namespace: hierarchical paths, or flat opaque IDs?
- Revocation: epoch-tagged caps with lazy invalidation, or eager?
- Attestation key hierarchy and the cross-vendor TPM story
