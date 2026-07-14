# Athena native AMDGPU render/DRM ABI oracle (2026-07-11)

This is a read-mostly capture from the working Arch Linux installation on the
Beelink Athena (`1002:15bf`, Phoenix1/Radeon 760M). It defines the concrete
kernel/userspace contract AthenaOS must reproduce for libdrm, Mesa/RADV, Vulkan
WSI, command submission, fence completion, and hang recovery.

The only workload was a ten-frame, 128x128 `vkcube` run through the existing
SDDM X11 session. No error injection, reset trigger, unbind, module reload, or
module-parameter write was performed. In particular, the debugfs
`amdgpu_gpu_recover` file was **not read**: its read callback schedules a full
GPU reset.

## Reference stack

| Component | Native value |
|---|---|
| Kernel | `7.0.12-arch1-1` |
| Kernel DRM driver ABI | `amdgpu 3.64.0` |
| Mesa/RADV | `26.1.2-arch1.2` |
| libdrm/libdrm_amdgpu | `2.4.134` |
| Vulkan loader/tools | `1.4.350` |
| RADV Vulkan API | `1.4.348` |
| GPU | `AMD Radeon 760M Graphics (RADV PHOENIX)` |
| Render node | `/dev/dri/renderD128`, major 226 minor 128 |
| Primary node | `/dev/dri/card1`, major 226 minor 1 |
| ICD | `/usr/share/vulkan/icd.d/radeon_icd.json` -> `libvulkan_radeon.so` |

`drmGetCap` reports:

- `DRM_CAP_PRIME = 0x3` (import and export)
- `DRM_CAP_SYNCOBJ = 1`
- `DRM_CAP_SYNCOBJ_TIMELINE = 1`

`amdgpu_device_initialize` succeeds and reports libdrm_amdgpu ABI `3.64`.

## Live device query results

```text
asic_id=0x15bf chip_rev=9 external_rev=10 family=148 ids_flags=0x1d
max_sclk=2599000 max_mclk=2800000 shader_engines=1 shader_arrays_per_se=2

VRAM:         size=2066558976 usage=310050816 max_alloc=2066558976
visible VRAM: size=2066558976 usage=310050816 max_alloc=2066558976
GTT:          size=7193960448 usage=35192832  max_alloc=2066558976
```

No eviction or VRAM-loss event had occurred:

```text
bytes_moved=0 evictions=0 vram_lost_counter=0
```

### Hardware-IP query contract

| IP | Count/version | Rings | IB start alignment | IB size alignment |
|---|---:|---:|---:|---:|
| GFX | 1 / 11.0.1 | `0x1` | 32 | 32 |
| Compute | 1 / 11.0.1 | `0xf` | 32 | 32 |
| DMA/SDMA | 1 / 6.0.1 | `0x1` | 256 | 4 |
| VCN decode | 1 / 4.0.2 | `0x0` | 256 | 64 |
| VCN encode | 1 / 4.0.2 | `0x1` | 256 | 4 |
| JPEG | 1 / 4.0.2 | `0x1` | 256 | 64 |

UMR reports the live discovery blocks required for reset and power decisions:

```text
MP0 13.0.4  MP1 13.0.4  GFX 11.0.1  SDMA 6.0.1
DCN 3.1.4   VCN 4.0.2   NBIO 7.7.0  SMUIO 13.0.7
```

## Mesa render ioctl ABI

### Command numbers and structure sizes

The minimum AMDGPU render-node ABI is not a custom AthenaOS protocol. It is the
Linux DRM UAPI below, with 64-bit little-endian layouts and 8-byte alignment
unless noted.

| Command | Header ioctl | Payload size |
|---|---:|---:|
| `GEM_CREATE` | `0xc0206440` | 32 |
| `GEM_MMAP` | `0xc0086441` | 8 |
| `CTX` | `0xc0106442` | 16 |
| `BO_LIST` | `0xc0186443` | 24 |
| `CS` | `0xc0186444` | 24 |
| `INFO` | `0x40206445` | 32 |
| `GEM_METADATA` | `0xc1206446` | 288 |
| `GEM_WAIT_IDLE` | `0xc0106447` | 16 |
| `GEM_VA` | `0x40406448` | 64 |
| `WAIT_CS` | `0xc0206449` | 32 |
| `GEM_OP` | `0xc0106450` | 16 |
| `GEM_USERPTR` | `0xc0186451` | 24 |
| `WAIT_FENCES` | `0xc0186452` | 24 |
| `VM` | `0xc0086453` | 8 |
| `FENCE_TO_HANDLE` | `0xc0206454` | 32 |
| `SCHED` | `0x40106455` | 16 (align 4) |
| `USERQ` | `0xc0486456` | 72 |
| `USERQ_SIGNAL` | `0xc0306457` | 48 |
| `USERQ_WAIT` | `0xc0486458` | 72 |

Important compatibility detail: Mesa 26.1.2 actually issued GEM-VA request
`0xc0406448` (`IOWR`), while the installed libdrm 2.4.134 public header expands
`DRM_IOCTL_AMDGPU_GEM_VA` to `0x40406448` (`IOW`). Linux dispatches by DRM
command number and descriptor rather than demanding identical direction bits.
AthenaOS must accept both encodings (or normalize by type/number/size) instead of
matching only one full 32-bit ioctl value.

Other structures observed on the submission path:

| Structure | Size/alignment |
|---|---:|
| `drm_amdgpu_cs_chunk` | 16 / 8 |
| `drm_amdgpu_cs_chunk_ib` | 32 / 8 |
| `drm_amdgpu_cs_chunk_dep` | 24 / 8 |
| `drm_amdgpu_cs_chunk_fence` | 8 / 4 |
| `drm_amdgpu_cs_chunk_syncobj` | 16 / 8 |
| `drm_amdgpu_fence` | 24 / 8 |
| `drm_syncobj_create` | 8 / 4 |
| `drm_syncobj_wait` | 40 / 8 |
| `drm_syncobj_timeline_wait` | 48 / 8 |
| `drm_syncobj_transfer` | 32 / 8 |

### What one RADV frame actually calls

A raw `strace -X raw` of one XCB `vkcube` frame produced this ioctl mix:

| Count | Raw request | Meaning |
|---:|---:|---|
| 46 | `0xc0406448` | AMDGPU GEM VA map/unmap/replace |
| 31 | `0x40206445` | AMDGPU INFO queries |
| 23 | `0xc0206440` | AMDGPU GEM create |
| 23 | `0x40086409` | DRM GEM close |
| 16 | `0xc0086441` | AMDGPU GEM mmap |
| 16 | `0xc00864bf` | syncobj create |
| 16 | `0xc00864c0` | syncobj destroy |
| 7 | `0xc02864c3` | syncobj wait |
| 4 | `0xc0406400` | DRM version |
| 4 | `0xc02064cc` | syncobj timeline transfer |
| 4 | `0xc00c642d` | PRIME handle-to-fd |
| 3 | `0xc1206446` | AMDGPU GEM metadata |
| 3 | `0xc03064ca` | syncobj timeline wait |
| 3 | `0xc01064c4` | syncobj reset |
| 3 | `0xc010640c` | DRM get-cap |
| 2 | `0xc0186444` | AMDGPU command submission |
| 2 | `0xc0106442` | AMDGPU context operation |
| 2 | `0xc0086202` | DMA-BUF export sync-file |
| 2 | `0x40086203` | DMA-BUF import sync-file |
| 1 | `0xc01864c1` | syncobj handle-to-fd |
| 1 | `0xc01864c2` | syncobj fd-to-handle |

This is the correct first Mesa/RADV ABI target. Legacy `WAIT_CS`, persistent
`BO_LIST`, userptr, and USERQ are required for broader compatibility, but RADV's
observed first-frame path used CS chunks plus explicit/timeline syncobjs.

## BO and GPU-VM behavior under RADV

While `vkcube` was active, `amdgpu_vm_info` showed a distinct VM for the client.
Its BOs included:

- GTT objects with `CPU_ACCESS_REQUIRED` and `CPU_GTT_USWC`;
- visible VRAM render/image objects with `NO_CPU_ACCESS` where appropriate;
- `VM_ALWAYS_VALID` command/internal BOs;
- `EXPLICIT_SYNC` on render resources;
- three exported 64 KiB DMA-BUF image objects;
- a pinned 2 MiB GTT object;
- signaled per-object write fences.

At the sample point the client had 13 idle BOs (393,216 bytes) and 19 completed
BOs (3,899,392 bytes). Evicted, relocated, moved, and invalidated lists were all
empty. This gives AthenaOS a concrete validation rule: a Mesa client needs a
per-file VM, validated BO residency, VA map/unmap, explicit reservation fences,
and PRIME/DMA-BUF export—not merely a physical allocator.

## Submit, fence, and IRQ proof

The ten-frame workload selected `AMD Radeon 760M Graphics (RADV PHOENIX)` and
completed without an AMDGPU error, VM fault, timeout, hang, or reset.

| Signal | Before | After | Delta |
|---|---:|---:|---:|
| MSI-X IRQ 60 | 10,778 | 10,914 | +136 |
| GFX emitted/signaled | `0x0f44` | `0x0f79` | +53 |
| SDMA emitted/signaled | `0x0406` | `0x046a` | +100 |
| Compute 1.1 emitted/signaled | `0x04` | `0x05` | +1 |
| MES emitted/signaled | `0x27` | `0x2b` | +4 |

Every post-run `last emitted` value equaled `last signaled`. All ring reset
counters remained zero. The implementation gate should therefore require an
IRQ-driven fence transition, not a polling shim that marks submitted work done.

## Recovery contract

Live settings:

```text
gpu_recovery=-1                  # auto; Phoenix is recovery-enabled
reset_method=-1                  # auto
queue_preemption_timeout_ms=9000
sched_jobs=32
sched_hw_submission=2
sched_policy=0
timeout_fatal_disable=N
halt_if_hws_hang=0
```

The live MP1 version is 13.0.4. Linux 7.0.12's `soc21_asic_reset_method()` maps
MP1 13.0.4 to `AMD_RESET_METHOD_MODE2`, so Athena's automatic internal recovery
method is PMFW MODE2. PCI core separately reports only `reset_method=bus`; the
device advertises `FLReset-`. A correct AthenaOS recovery path cannot assume PCI
FLR is available.

The upstream recovery sequence schedules reset work, marks a full reset,
quiesces scheduling, performs MODE2 via PMFW, reinitializes the device/IP blocks,
and re-establishes VM/fence state. Native boot history showed no real reset,
devcoredump, VRAM loss, or fence reset. An induced-hang proof remains a AthenaOS
hardware gate and should be run only when a recovery image and out-of-band boot
path are ready.

## Mesa, WSI, and scanout contract

RADV exposes the required modern sharing/synchronization extensions:

- `VK_KHR_swapchain`, timeline semaphore, present ID, and present wait;
- external memory, semaphore, and fence FD extensions;
- `VK_EXT_external_memory_dma_buf`;
- `VK_EXT_image_drm_format_modifier`;
- `VK_EXT_memory_budget`;
- swapchain maintenance extensions.

X11 exposes DRI3 and Present, and the XCB swapchain completed. The primary KMS
node exposes an HDMI connector, atomic properties, VRR capability, primary/
overlay/cursor planes, `IN_FORMATS`, and AMD GFX11 modifiers including 64/256 KiB
tiled layouts, DCC variants, and linear fallback. Initial WSI enablement can use
linear `XR24`/`AR24`; production presentation needs PRIME FD transfer, explicit
sync-file exchange, DRI3/Present (or the AthenaOS-native equivalent), and format
modifier negotiation.

## Implementation order derived from the trace

1. DRM render-node/file lifecycle, `VERSION`, `GET_CAP`, GEM close, and per-file
   handles.
2. AMDGPU `INFO` responses matching ABI 3.64 and the live Phoenix IP/alignment
   values.
3. GEM create/mmap plus real TTM-backed GTT/VRAM placement.
4. Per-file GPU VM and GEM-VA operations (accept both observed direction-bit
   encodings).
5. Context creation and chunked `CS` submission with validated IB alignment.
6. DRM syncobj and timeline syncobj semantics; reservation fences and DMA-BUF
   sync-file import/export.
7. MSI IRQ completion advancing real ring fences and waking syncobj waiters.
8. PRIME FD export/import and image metadata/modifier negotiation.
9. WSI present loop through DRI3/Present-equivalent compositor IPC.
10. Scheduler timeout, PMFW MODE2 reset, IP reinitialization, VM recovery, and
    post-reset resubmission proof.

