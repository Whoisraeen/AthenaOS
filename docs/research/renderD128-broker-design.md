# `/dev/dri/renderD128` broker — design spec

**Date:** 2026-07-12
**Author:** opus (interface steward)
**Status:** DESIGN — host-testable foundation landing incrementally; Athena
validation of the render-client lifecycle (step 1 of the owner's GPU plan) is a
prerequisite for wiring the live path.
**Concept line served:** §AthGFX + §AthGuard — "the anti-cheat answer *without*
giving vendors ring 0": a game reaches the GPU through a capability-brokered
render node, never through raw MMIO/DMA. Implements step 2 of the owner's
2026-07-12 GPU vertical-slice plan; builds on the daemon-side render seam
(`linuxkpi-drm/bringup_render.c`: `rae_amdgpu_render_open/ioctl/close`).

---

## 1. Threat model and the one invariant

A game (Mesa/RADV, later DXVK/Proton) is **untrusted**. It must be able to
allocate BOs, build command buffers, submit, and wait — but it must **never**
hold `Cap::Mmio`, `Cap::Irq`, or DMA authority over the GPU. If it did, a
compromised or malicious game would have ring-0-equivalent power (the exact
sandbox-escape class the 2026-07-06 audit closed for driver daemons).

**Invariant:** the client's only GPU-facing authority is a `Cap::Gpu` handle and
one capability-gated shared-memory channel to the kernel broker. The broker —
not the client — owns the trust boundary. The persistent `amdgpud` daemon owns
the real `drm_file`, GPU VM, rings, fences, and hardware; it runs
IOMMU-sandboxed (caps 109–118 lineage) and is the only holder of GPU MMIO/DMA.

```
┌ game (untrusted) ─────────────┐   Cap::Gpu + Cap::Channel only
│ Mesa/RADV → libdrm shim       │   (no Mmio, no Irq, no DMA)
│  open/ioctl/mmap/close on     │
│  /dev/dri/renderD128          │
└──────────────┬────────────────┘
               │ SYS_IOCTL / SYS_MMAP on the render fd
┌ kernel broker (TRUST BOUNDARY)┐   rae_render_broker (this design)
│  · per-fd session table       │   · dispatch on DRM type/number/size
│  · fail-closed ioctl allowlist│   · bounded marshal into shmem queue
│  · copies bounded payloads    │   · never forwards raw pointers
│  · maps GEM offsets, not files│
└──────────────┬────────────────┘
               │ request/reply queue in a cap-gated shared frame
┌ amdgpud (sandboxed daemon) ───┐   linuxkpi-drm/bringup_render.c
│  rae_amdgpu_render_open →      │   real drm_file + per-client GPU VM
│  rae_amdgpu_render_ioctl →     │   real upstream amdgpu ioctl handlers
│  rae_amdgpu_render_close       │   owns rings/fences/VM/BOs + GPU HW
└────────────────────────────────┘
```

## 2. Behavior contract (owner's step-2 checklist → mechanism)

| Requirement | Mechanism |
|---|---|
| `open()` creates a unique daemon-side `drm_file` | broker allocates a session id, cap-gated shared frame + request/reply queue, sends `OPEN`; daemon calls `rae_amdgpu_render_open()` → per-client VM |
| `close()` destroys that client's handles/contexts/VM/fences | broker sends `CLOSE`; daemon calls `rae_amdgpu_render_close()` (already tears down `evf_mgr` + `drm_file_free`); broker frees the session + shared frame |
| `ioctl()` forwards metadata + bounded payloads | broker `dispatch()`s (§4), copies **exactly** the allowlisted struct size in/out through the shared frame — never the client's pointer |
| `mmap()` maps GEM offsets, not file contents | broker resolves the fake mmap offset (from `GEM_MMAP`) to the BO's daemon-owned physical pages and maps them into the client (ownership-gated, `MAP_PHYS` security model: firmware/daemon-owned pages only, never usable RAM) |
| Multiple clients isolated | one session (drm_file + VM + shared frame + queue) per fd; no shared mutable state between sessions; session ids are kernel-allocated, not client-guessable |
| Invalid sizes/pointers/directions/commands fail closed | `dispatch()` rejects unknown type/nr, size mismatch, over-`MAX_PAYLOAD`; all user copies bounds-checked against the client mapping; direction taken from the broker allowlist, never the client's `_IOC` dir bits |
| Daemon death → `ENODEV`, revoke all sessions | broker watches the daemon pid; on exit every session's next op returns `-ENODEV` and its shared frame + caps are revoked |

## 3. ABI surface (steward plan — NOT yet allocated)

This will be **one batched `[interface]` commit** (`RAEEN_AGENT=opus`) when the
live path is wired, with `docs/SYSCALL_TABLE.md` updated in the same commit and
`ABI_VERSION` bumped. Nothing below is a live magic number yet.

- **No new top-level syscalls if avoidable.** The render node is a VFS device
  (`/dev/dri/renderD128`) reached through the existing `SYS_OPEN`/`SYS_IOCTL`/
  `SYS_MMAP`/`SYS_CLOSE`. `sys_ioctl` (`kernel/src/posix.rs`) gains a render-fd
  arm; `/dev/dri/renderD128` is registered as a device node in `vfs.rs`.
- **Gate:** opening the node requires a `Cap::Gpu { device_id, rights: READ }`
  (present in `capability.rs`). No `Cap::Gpu` → `-EACCES`, fail closed. This is
  the least-privilege replacement for the coarse `Cap::System{WRITE}` the
  daemon-claim path uses.
- **Transport:** the per-session request/reply queue rides a `Cap::Channel`
  shared frame (`SYS_CHANNEL_SHMEM_MAP`, 119) between kernel broker and daemon.
  The client never maps this frame — only the broker and `amdgpud` do.
- **Reserved, not allocated:** a small `SYS_RENDER_*` block (or an ioctl
  sub-protocol on the node) for session setup if the VFS path proves
  insufficient. Deferred until the live wiring slice decides.

## 4. The ioctl dispatch/normalization gate (LANDING NOW — `rae_render_broker`)

The first host-testable slice, because it is the fail-closed heart of `ioctl()`
forwarding and it encodes the oracle's hardest compatibility rule.

`docs/gpu-oracle/ATHENA-AMDGPU-DRM-ABI-20260711.md`: Mesa 26.1.2 issues
`GEM_VA` as `0xc0406448` (`_IOWR`) while libdrm's header expands the same command
to `0x40406448` (`_IOW`). Linux dispatches on **command number + descriptor**,
not the full 32-bit value. So the broker must, too.

`rae_render_broker::ioctl`:
- `decode(req: u32) -> Ioctl { dir, type_, nr, size }` — the Linux `_IOC` bit
  layout (nr 0–7, type 8–15, size 16–29, dir 30–31).
- A fail-closed **allowlist** of the render node's permitted commands: the 3
  generic DRM ioctls the trace uses (`VERSION`, `GET_CAP`, `GEM_CLOSE`) plus the
  17 AMDGPU render ioctls `bringup_render.c` registers, each with its canonical
  `(nr, struct size, copy direction)` transcribed from the oracle.
- `dispatch(req) -> Result<Resolved, DispatchError>`:
  1. reject any `type_ != 'd'` (`0x64`) — not a DRM ioctl;
  2. look up `nr` in the allowlist — **absent ⇒ `UnknownCommand`** (the allowlist
     *is* the policy; unregistered ioctls never reach the daemon);
  3. require the decoded `size` to equal the registered size — **mismatch ⇒
     `SizeMismatch`** (a truncated/oversized struct is rejected before any copy);
  4. **ignore the client's `dir` bits**; the copy plan comes from the broker's
     own `copy` field. This is the GEM-VA normalization: `0x40406448` and
     `0xc0406448` share `(type, nr, size)` and resolve to the *same* `Resolved`.
- `MAX_PAYLOAD` bounds every marshaled struct (largest is `GEM_METADATA` = 288).

Copying `size` bytes per the broker's own allowlist — never per the client's
`_IOC` bits — is also a hardening win: the client cannot coax an over-read/over-
write by lying about direction or size.

## 5. Proof ladder

1. **Host KATs (this slice):** `cargo test -p rae_render_broker` — every oracle
   ioctl value resolves to the right command; **both** GEM-VA encodings normalize
   to one `Resolved`; wrong type / unknown nr / size mismatch all fail closed; no
   payload exceeds `MAX_PAYLOAD`. FAIL-able by construction.
2. **Safe build + QEMU CI:** once wired, the node registers and the daemon is
   render-capable; QEMU has no Radeon so the daemon self-skips (`9099`) — the
   broker's *plumbing* (session alloc, cap gate, `-ENODEV` on absent daemon) is
   still exercisable with a stub daemon.
3. **Athena:** the step-4 purpose-built client (open → GTT BO → mmap → GPU VA →
   CS on SDMA/GFX → readback → teardown) run against the live node. No synthetic
   completion accepted.

## 6. Open decisions (for the live-wiring slice, after step-1 Athena proof)

- VFS-node ioctl arm vs a dedicated `SYS_RENDER_*` block — pick by whether
  `SYS_MMAP` on a device fd can carry a GEM fake-offset cleanly.
- Queue doorbell: reuse the `Cap::Channel` send/recv wakeup, or a lighter
  shared-frame futex. The hot path (CS submit, fence wait) should carry ring
  indices, not copies (per the 2026-07-06 submit spec §5).
- Fence/wait: `WAIT_CS`/`GEM_WAIT_IDLE` must block on the **IRQ-driven** fence
  (step 5), not poll — the daemon's IRQ pump must run while resident.
