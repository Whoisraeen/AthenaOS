# RaeBridge

Windows app compatibility layer. Wine + Proton heritage, but **tightly integrated**:
not a "subsystem," not a launcher you have to configure — Windows apps just run.

## Goals

- Run a Win32 / Win64 binary by double-clicking it. No prefix configuration, no
  "is this app supported" gating.
- DirectX 11/12 → RaeGFX translation at the driver level (DXVK / VKD3D-Proton lineage)
- A single signed runtime maintained by us, not a per-app sprawl of `wine_*` versions
- Capability sandbox applies: a Windows app sees a virtual C:\ that's actually its
  per-app data bucket; no global registry; no shared state with other Windows apps
- Native-feel: Win32 windows route through RaeUI's compositor, so they get glass,
  HDR, VRR, and consistent input latency

## What we won't do

- Ship a separate "compatibility mode" UI. There's one OS; this layer is invisible.
- Promise 100% compatibility. We target the top games and top creative tools first;
  long tail follows.

## Layering

- **raebridge-runtime**: the integrated Wine/Proton-lineage userland.
- **raebridge-d3d**: from-scratch Rust *shader translator* (DXBC/DXIL→SPIR-V,
  `dxbc_spirv.rs`) + source-ported DXVK/VKD3D *runtime* (via `zig cc`), output to
  RaeGFX. Split ratified 2026-06-26 — see `raebridge-wine-strategy.md` §5. Not a
  DXVK `.so` bolt-on; not a from-scratch runtime.
- **raebridge-sandbox**: capability-mapped Windows API shims (file system, registry,
  IPC, networking).
- **raebridge-install**: per-app install profile (overrides, DLL versions) maintained
  by us, not the user.

## Open design questions

- Wine upstreaming policy — fork lightly, contribute back, or maintain a strict downstream?
- Per-app DLL override management UX (we *will* still need this for the long tail)
- Anti-cheat compatibility — pairs with RaeShield's attestation pitch
