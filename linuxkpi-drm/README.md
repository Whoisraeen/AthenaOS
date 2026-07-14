# linuxkpi-drm — running the real amdgpu driver via LinuxKPI (Path C)

**Status: foundation / scaffolding. NOT a shipped feature.** This is the FreeBSD
`drm-kmod` model applied to AthenaOS: compile the *real* upstream `amdgpu` C driver
against a LinuxKPI compatibility layer (headers here + the C-ABI implementations in
`components/ath_linuxkpi`), so the complete, battle-tested amdgpu init brings the
GPU up — instead of our hand-ported `components/ath_amdgpu` Rust reimplementation.

## Why this exists

`components/ath_amdgpu` reimplements the gfx11 / MES bring-up in Rust. As of
2026-06-30 we proved (via `docs/gpu-oracle/`, ftrace of the live driver + the real
`mes_v11_0.c` source) that **our entire MES submission path is byte-identical to
amdgpu** — queue-init (KIQ MAP_QUEUES + ring-test drain), the MES enable, and every
`set_hw_resources` packet field all match — yet the SCHED pipe (pipe0) still halts
mid-handler with `mcause=0`. The handler *executes correctly*, then the microengine
stops. The remaining gap is therefore in the **broader device init** (a power / clock
/ SMU / RLC / IH state the full driver establishes and our hand-port doesn't) that
keeps pipe0's microengine alive.

The only way to close a gap that diffuse is to **run the real init**. That is this
directory.

## The GPL license boundary (READ THIS)

The upstream amdgpu driver is **GPL-2.0**. The AthenaOS kernel + components are
**MPL-2.0**. Per `docs/LINUX_DRIVER_STRATEGY.md` (Option C, "FreeBSD-style LinuxKPI
partition — separate license island"):

- **The GPL amdgpu source is NEVER committed to this repo.** It is fetched into
  `vendor/` (git-ignored) by `fetch-source.sh`. `vendor/` is the only place GPL
  source lives, and it is not tracked.
- This directory (`linuxkpi-drm/`) is a **separate build** — it is NOT a cargo
  workspace member, NOT part of `kernel/`, and is not built by `xtask`. It links the
  GPL amdgpu objects against `ath_linuxkpi` at the **userspace daemon** boundary
  (`amdgpud`), preserving crash isolation (Concept §Architecture: IOMMU-sandboxed
  userspace drivers).
- The LinuxKPI shim headers under `include/` are **MPL-2.0** (original work — Linux
  API *signatures*, not GPL kernel source).

If you are uncomfortable shipping GPL amdgpu objects linked into an MPL system, that
is a product decision for the owner — this tree only makes it *technically possible*
and keeps the boundary clean.

## Build host

Native Linux + gcc (the kernel C idioms need a real Linux toolchain). On the dev box
this is **WSL2 Ubuntu** (`gcc 11.4`). The header-shim iteration loop runs natively on
the host for speed; cross-compilation for the AthenaOS target is a later phase.

## Layout

```
linuxkpi-drm/
  README.md          — this file
  SCOPE.md           — honest dependency map + the error-driven iteration loop
  fetch-source.sh    — fetch the amd subtree (kernel 7.0.12, matches Athena's firmware)
  Makefile           — the incremental build (host/WSL)
  include/linux/     — LinuxKPI C header shim (MPL; grows error-driven)
  include/drm/       — DRM core header shim (MPL)
  tests/             — standalone compile targets (the green milestones)
  vendor/            — GPL amdgpu source (git-IGNORED, fetched, never committed)
```

## Quick start

```bash
./fetch-source.sh          # populates vendor/ (git-ignored)
make mes-structs           # MILESTONE 1: the MES packet + MQD structs compile (GREEN today)
make probe                 # show the next wall of missing LinuxKPI surface
```

## Current milestone

- **[x] M1 — MES core structs compile.** `mes_v11_api_def.h` (the `set_hw_resources`
  packet union) + `v11_structs.h` (the MQD layout) compile against stdint/stdbool.
  This is the MES-specific definition layer — the part our Rust reimpl mirrors.
- **[~] M2 — `mes_v11_0.c` preprocesses** against the `linux/` + `drm/` + amd header
  set (resolve every `#include`). *In progress:* `linux/firmware.h` + `linux/module.h`
  shimmed; the wall is now **inside `amdgpu.h`** (`amdgpu_ctx.h` → `linux/ktime.h`) —
  the 74-include graph. Run `make probe` to see the current wall.
- **[ ] M3 — `mes_v11_0.c` + its direct deps *compile*** (the `amdgpu_device` struct
  surface stubbed enough to typecheck).
- **[ ] M4 — the MES bring-up subset links** against `ath_linuxkpi`.
- **[ ] M5 — runs against the live device** (the real init brings pipe0 up).

See `SCOPE.md` for the realistic effort and the per-milestone dependency walls.
