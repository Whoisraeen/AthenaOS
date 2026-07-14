# SCOPE — what "run the real amdgpu via LinuxKPI" actually costs

Honest assessment so nobody (human or agent) mistakes the foundation for the finish.

## The size of the thing

- `drivers/gpu/drm/amd/` is **511 MB**, **300 `.c` files** in `amdgpu/` alone.
- `mes_v11_0.c` (our target) is 11 `#include`s, but one of them is `amdgpu.h` —
  **74 includes deep**, transitively the entire driver's struct graph + DRM + TTM +
  the Linux kernel API.
- `raeen_linuxkpi` already implements **488 C-ABI functions**. That is a real head
  start, but it is the *implementation* side. The **C header shim** that declares
  those functions AND the hundreds of kernel *types/macros* amdgpu needs
  (`struct device`, `pci_dev`, `dma_fence`, `drm_device`, `ttm_*`, `REG_SET_FIELD`,
  `WREG32_SOC15`, the `container_of`/`list_head`/`completion` idioms, …) does not
  exist yet and is the bulk of the work.

This is FreeBSD `drm-kmod` scale (years of upstream effort there). RaeenOS does **not**
need all 300 files — the MES bring-up subset is ~50–100 files — but that is still a
multi-month, error-driven grind. Treat each milestone in `README.md` as weeks, not
hours.

## Strategy: error-driven, host-first, subset-only

1. **Host-first.** Compile on WSL/gcc natively. Fast loop, no flash, no cross-compile
   noise. Only after the subset *compiles + links* do we cross-compile for RaeenOS.
2. **Subset-only.** We are not porting "all of amdgpu". We need exactly the call graph
   that brings the GPU to a live MES + first submit: device/IP-discovery → GMC (GART)
   → PSP (firmware load) → GFX/RLC → MES → SMU (power) → IH. Everything else
   (display, VCN, video, debugfs, sysfs, power-profiling) is `#if 0`'d or stubbed.
3. **Error-driven shim.** Don't pre-write headers. Compile, read the first error, add
   exactly the type/macro/decl it needs to `include/`, repeat. The shim grows to fit
   the actual call graph, nothing more. **Every stub returns a real error or a defined
   value — no silent-success fakes** (RaeenOS rule 9 + `LinuxKabiError` discipline).

## Milestone walls (what blocks each step)

| Milestone | The wall |
|---|---|
| **M1 structs** ✅ | none — `mes_v11_api_def.h` + `v11_structs.h` are stdint/stdbool only. |
| **M2 preprocess** | provide every `linux/*.h` + `drm/*.h` `mes_v11_0.c` reaches. `amdgpu.h` is the dragon: 74 includes. |
| **M3 compile** | `struct amdgpu_device` + ~40 sub-block structs must typecheck. `WREG32_SOC15`/`REG_SET_FIELD`/`soc15_common.h` macros must expand. This is where most of the type shim gets written. |
| **M4 link** | map every undefined symbol to a `raeen_linuxkpi` export or a new shim. Build `raeen_linuxkpi` as a `staticlib` (the `clib` feature, currently deferred). |
| **M5 run** | wire the linked amdgpu objects into `amdgpud`, feed it the real BAR/IRQ/DMA via the existing `GpuOps`/userspace-driver path, and run its init against the live Athena GPU. |

## What success looks like

The real `amdgpu` `gfx_v11_0` + `mes_v11_0` init runs on Athena and the SCHED pipe
does **not** halt — i.e. the broader-init state our Rust port is missing gets set,
because the real code sets it. Then `set_hw_resources` ACKs, and we have a live MES =
the gate to real rendering = the gate to games.

## Fallback / parallel track

If a specific milestone stalls, the `docs/gpu-oracle/` loop (ftrace/mmiotrace the live
driver, no flash) can extract the exact broader-init register delta and we port *just
that* into `components/raeen_amdgpu` — cheaper if the gap turns out to be small. The
two tracks share the same oracle infrastructure.
