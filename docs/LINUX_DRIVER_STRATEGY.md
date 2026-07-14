# Linux driver compatibility strategy (RaeenOS)

**Status:** Planning / scaffolding only. **Linux driver compatibility is not a
shipped feature** and must not be advertised as one until Phase 3 criteria
below are met on real hardware.

## Context

RaeenOS targets modern desktop gaming hardware (NVMe, xHCI, Intel/AMD GPU,
PCIe MSI-X). The Concept doc mandates **IOMMU-sandboxed user-space drivers**
for anything that can fail without taking the system down. Bare-metal boot
still depends on a small set of in-kernel drivers today; closing the gap to
“vendor driver on RaeenOS” is a multi-year arc, not a single module.

## Options A–D (summary)

## Current GPU accelerator decision (2026-07-10)

**The LinuxKPI-hosted upstream amdgpu daemon is RaeenOS's sole hardware
acceleration path for Phoenix-class AMD GPUs.** It is the only path authorized
to grow toward Mesa/RADV, Vulkan, DXVK/VKD3D, and game workloads. The native
Rust `raeen_amdgpu` implementation remains supported only as the DCN scanout
path and hardware-forensics fallback while the upstream driver clears the
bare-metal PSP to MES to fence gate.

This is a convergence decision, not a premature feature claim: no path is
gaming-ready until it proves, on bare metal, an IOMMU-confined BO to GPU-VM to
submit to IRQ/fence to present loop, plus timeout/reset recovery. Mesa/RADV and
a Vulkan game remain subsequent milestones. Do not add a parallel native GFX
submit path unless the documented LinuxKPI M1 fallback trigger is reached.

| Option | Approach | Pros | Cons |
|--------|----------|------|------|
| **A** | Userspace Linux ABI only (`linux_compat.rs`, `linux_syscall.rs`, ELF `.so`) | Aligns with Concept §Architecture; MPL-clean; crash isolation | Does not load `.ko` kernel modules |
| **B** | In-kernel kABI shim (symbol stubs → RaeenOS implementations) | Familiar to porting `.ko` blobs | GPL boundary, large surface, kernel attack surface |
| **C** | FreeBSD-style **LinuxKPI** partition (separate license island) | Proven for Mesa on BSD | Heavy maintenance; license partitioning complexity |
| **D** | **RaeenOS recommendation:** targeted bootable-vendor scaffolding + native Rust hot paths | Honest scope; name registry + tooling first; userspace drivers for isolation | No universal “run any Linux driver” promise |

**Recommendation (Option D):** Phase 1 symbol registry (`linux_kabi.rs`), Phase 2
userspace driver framework (IOMMU + capability IPC), Phase 3 native Rust shims
for hot paths (IRQ, DMA, PCI config) where profiling proves kernel residency is
required. Reject universal Linux-driver compat as a product claim.

## Foundation gaps (abbreviated)

| Area | Current state | Blocker for vendor drivers |
|------|---------------|------------------------------|
| Boot / storage | NVMe/AHCI in kernel; MSI-X probe | Real HW validation, tiered RaeFS on disk |
| PCIe / IRQ | Legacy I/O on QEMU; MSI-X hooks | ECAM (MCFG) on UEFI hardware |
| GPU | Bochs VBE / scanout path | No Intel/AMD/NVIDIA native or shimmed DRM |
| USB | xHCI framework | Full stack still evolving |
| IOMMU | Module present | Per-device domains not end-to-end for arbitrary DMA |
| Licensing | MPL-2.0 kernel | `EXPORT_SYMBOL_GPL` — no static GPL `.ko` in tree |

## GPL / R7 / license boundary

- **R7 (workspace rules):** No Linux clones (ext4, ALSA, DRM/KMS as Linux,
  netfilter, procfs as Linux, etc.). `linux_kabi.rs` is **symbol name +
  metadata only** — not struct layouts, not epoll, not a monolithic kernel.
- **No GPL Linux kernel source** in the RaeenOS tree. Stubs are MPL-2.0.
- **`EXPORT_SYMBOL_GPL`:** Even with stubs, loading GPL-only Linux modules into
  a non-GPL kernel raises compliance questions. Supported path: userspace
  drivers + attestation, or native Rust device classes.
- **Stub calls** return `LinuxKabiError` (e.g. `ENOSYS`) — **no silent success**.

## Roadmap

### Phase 1 — Symbol registry (this work)

- `kernel/src/linux_kabi.rs`: ~50 canonical `EXPORT_SYMBOL` names, categories
  (Memory, PCI, IRQ, Device, DMA, Misc), status (Stub / Unimplemented / Planned).
- `/proc/raeen/linux_kabi` introspection.
- Boot smoketest: resolve probe names, log `N/M symbols registered`.
- Tooling (future): parse `.ko` undefined symbols against registry.

### Phase 2 — Userspace driver framework

- Dedicated driver process per device class; capabilities via `crate::capability`.
- IOMMU DMA buffer registration; IRQ delivery via secure IPC (not raw Linux `request_irq` semantics in kernel).
- Mesa / NIC userspace stacks hosted as ELFs, not `.ko` loads.

### Phase 3 — Native Rust hot-path shims

- Implement only symbols profiling shows on the frame/audio/storage hot path.
- Prefer RaeGFX / RaeNet / native block path over Linux DRM/ALSA shims.
- Success criterion: one certified device class on real hardware (e.g. one GPU
  generation or one NVMe quirk), not “all of lspci works.”

## Related code

| Component | Role |
|-----------|------|
| `linux_compat.rs` | In-kernel driver API shim (DMA/IRQ/kmalloc scaffolding) |
| `linux_syscall.rs` | Linux x86_64 syscall translation for ELFs |
| `linux_kabi.rs` | Kernel module **symbol name** registry (this doc) |
| `linuxkpi_host.rs` + `components/raeen_linuxkpi` | **LinuxKPI Phase 1-4** — C ABI + host syscalls 127–140: timing/log (P1), ioremap+PCI+IRQ doorbells (P2), zero-copy `dma_alloc_coherent` (P3), IOMMU sandbox + daemon supervisor (P4). See `docs/LINUXKPI_PHASE1.md`, `docs/LINUXKPI_PHASE2.md` |
| `linux_exec.rs` | Mark tasks as Linux ABI for syscall dispatch |
| `userspace_driver.rs` | Capability-gated claim/DMA/IRQ (syscalls 109–118); Phase 2 bridge target |

## What we do not claim

- Running arbitrary upstream Linux kernel modules (`.ko`).
- Feature parity with Linux `drm/kms`, `cfg80211`, or `nvme` subsystems.
- GPL module loading inside the MPL-2.0 kernel without a separate license partition.
