# Mesa ↔ amdgpu seam — the on-ramp for hardware Vulkan/GL

**Status:** Design + Phase-1 landed (2026-06-17). Phases 2–4 scoped below.
**Owner:** Opus. **Precedes:** the Phase-6 "Vulkan triangle on a real GPU" deliverable.

## TL;DR

Porting Mesa (radeonsi for OpenGL, radv for Vulkan) is the path to **real GPU
rendering** on RaeenOS — but it is a multi-month effort and is **gated on the
amdgpu bring-up being iron-proven first** (you don't render through a driver that
can't bring the GPU up). This doc maps the exact seam Mesa talks to, audits what
the bring-up already provides, and stages the build so each piece is host-proven
before the next. **Mesa itself is not in the tree and is not promised here** —
this is the *interface* that lets it link, built incrementally.

## The stack (who calls what)

Mesa is a **userspace application's renderer**, NOT a kernel driver. It does not
link `raeen_drm` (that's the kernel-side DRM core the amdgpu *kernel* driver
uses). Mesa talks to the GPU through ioctls:

```
  app (game / compositor)
    └─ Mesa  radeonsi (GL) / radv (Vulkan)
         └─ src/amd/winsys/amdgpu   (Mesa's amdgpu winsys)
              └─ libdrm_amdgpu      (amdgpu_device_initialize, amdgpu_bo_alloc,
                                      amdgpu_cs_submit, amdgpu_cs_query_fence_status…)
                   └─ DRM ioctls on a render node  (DRM_IOCTL_AMDGPU_*)
                        └─ RaeenOS: routed to amdgpud (the Path-C userspace driver)
                             └─ raeen_amdgpu::bringup  (the ring submit + fence we built)
                                  └─ the GPU
```

So the seam has two halves we own:
1. **`libdrm_amdgpu` C-ABI shim** — the userspace lib Mesa links (a sibling of
   `raeen_linuxkpi`, but the *amdgpu winsys* surface rather than the kernel KPI).
2. **The DRM amdgpu ioctl surface** — what those lib calls turn into, routed to
   `amdgpud` and serviced by the bring-up primitives.

## The 8 ioctls Mesa actually uses (uapi `amdgpu_drm.h`)

| ioctl (`DRM_AMDGPU_*` id) | what Mesa needs it for | maps onto (we have it) |
|---|---|---|
| `INFO` (0x05) | identify + size the GPU (dev info, memory, hw-ip, fw ver) | `bringup::Device` + `regs`/discovery — **Phase 1 ✅** |
| `GEM_CREATE` (0x00) | allocate a buffer object (VRAM/GTT) | `GpuOps::dma_alloc` (IOMMU-sandboxed) |
| `GEM_MMAP` (0x01) | map a BO into the app's address space | the DMA buffer's CPU mapping (`DmaBuf.id`) |
| `GEM_VA` (0x08) | bind a BO into the GPU virtual address space | GPUVM / GMC (stage 3) — bind path TBD |
| `CS` (0x04) | submit a command stream (IB) to a ring | `program_sdma_ring` / the CP submit (ring + WPTR) |
| `WAIT_CS` (0x09) | wait for a submission to complete | `sdma_submit_and_wait` / the fence-poll |
| `CTX` (0x02) | create a submission context (priority, reset) | a thin ctx table over the rings |
| `FENCE_TO_HANDLE` (0x14) | turn a CS fence into a syncobj/fd | `raeen_drm::fence` + syncobj surface |

The encouraging part: **the four load-bearing ioctls already have primitives.**
`GEM_CREATE`→`dma_alloc`, `CS`→the ring submit, `WAIT_CS`→the fence-poll, `INFO`→
`Device`. The bring-up work these last commits built (ring program + submit +
fence-poll for both the GFX CP and SDMA engines) is *exactly* the substrate the
`CS`/`WAIT_CS` ioctls sit on.

## Phase 1 — `AMDGPU_INFO` device query (landed)

`raeen_amdgpu::uapi` — the byte-exact uapi surface Mesa's winsys reads first:
the ioctl ids, the `AMDGPU_INFO` sub-queries, HW-IP / GEM-domain constants, and
the `drm_amdgpu_info_device` struct (transcribed field-for-field from
`amdgpu_drm.h`, ABI-guarded with `offset_of!` KATs). `query_dev_info(&Device)`
fills the fields we know authoritatively (PCI device id, VBIOS bootup clocks,
gfx11 wave/page constants) and leaves the rest 0 (never fabricated — the uapi's
own "older chips set 0" rule). Host-KAT'd (`cargo test -p raeen_amdgpu`).

## Phase 2 — the DRM ioctl handlers (next, host-provable)

Build the `amdgpu_*` ioctl dispatch in `amdgpud` (or `raeen_drm`), each handler a
thin map onto an existing primitive, all host-testable over `GpuOps`:
- `GEM_CREATE`/`GEM_MMAP` → `dma_alloc` + the CPU mapping; a BO handle table.
- `INFO` (`DEV_INFO`/`MEMORY`/`HW_IP_INFO`/`FW_VERSION`) → `query_dev_info` +
  a `drm_amdgpu_memory_info` from `Device.vram_size` + the firmware versions.
- `CS` → write the app's IB into a ring + `program_sdma_ring`/CP submit.
- `WAIT_CS` → `sdma_submit_and_wait` / the fence-poll.
Proof: a host harness that drives `INFO → GEM_CREATE → CS(fill) → WAIT_CS` against
the mock GPU and sees the fence post — the ioctl-level twin of today's bring-up test.

## Phase 3 — the `libdrm_amdgpu` C-ABI shim

A `raeen_libdrm_amdgpu` crate exposing the C symbols Mesa's winsys links
(`amdgpu_device_initialize`, `amdgpu_query_info`, `amdgpu_bo_alloc`,
`amdgpu_bo_cpu_map`, `amdgpu_cs_submit`, `amdgpu_cs_query_fence_status`, …), each
forwarding to the Phase-2 ioctl surface. Sibling discipline to `raeen_linuxkpi`:
`#![no_std]`, host-KAT'd, no fabricated returns.

## Phase 4 — build Mesa (the big one, iron-gated)

`cargo`/`meson` Mesa (radeonsi + radv) against the Phase-3 shim, `build-std`-style.
This is the multi-month subsystem. It only makes sense **after** the bring-up is
iron-proven (discovery blob vendored → SOC15 offsets live → CP/SDMA execute on
the 780M). Until then, Phases 1–3 are the productive, host-provable work.

## Honest constraints

- **Not promised:** running arbitrary Mesa today. Phases 1–3 build the *interface*;
  Phase 4 is the real port, gated on iron.
- **The licensing island holds:** Mesa is MIT (not GPL), so the userspace shim is
  clean (`docs/LINUX_DRIVER_STRATEGY.md`). No GPL DRM source enters the tree.
- **Everything downstream of `CS`/`WAIT_CS` is gated on the same `ip_discovery.bin`
  capture** as the rest of the driver — the seam is built and host-proven, but it
  only moves real pixels once discovery goes live on the 780M.

## Hand-off

→ **Phase 2** (`raeen-gpu` / `raeen-drivers`): the DRM ioctl handlers over the
bring-up primitives. Proof line: a host harness logs
`[drm] INFO dev_info: 1002:15bf` then `[drm] CS submitted -> WAIT_CS fence posted`.
