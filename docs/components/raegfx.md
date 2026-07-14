# AthGFX

Native graphics API. Vulkan-equivalent capability, friendlier surface, first-class
HDR and VRR.

## Goals

- Explicit, modern command-buffer / pipeline-state-object model (like Vulkan, Metal, DX12)
- "Looks like Metal, performs like Vulkan" surface ergonomics
- Compositor-aware path AND a zero-overhead direct-to-GPU path for exclusive fullscreen games
- DirectX 11/12 translation at the driver level (DXVK / VKD3D-Proton lineage)
- OS-level shader cache, shared across Vulkan and AthGFX, persistent across reinstalls
- First-class HDR (HDR10, Dolby Vision metadata pass-through where licensed)
- First-class VRR (FreeSync, G-Sync compatible, per-window where the display supports it)

## Non-goals

- Bytecode portability across vendors (we'll lean on SPIR-V)
- Software fallback rasterizer beyond "is the system usable enough to ship a bug report"

## Surface sketch (Rust-style)

```rust
let device = Device::open(Adapter::default())?;
let pipeline = device.build_pipeline()
    .vertex_shader(spirv!("triangle.vert.spv"))
    .fragment_shader(spirv!("triangle.frag.spv"))
    .color_target(Format::Bgra8Srgb)
    .build()?;
let mut frame = device.begin_frame()?;
frame.bind(&pipeline);
frame.draw(0..3);
frame.present()?;
```

## Layering

- **raegfx-api** (public): the Rust surface above.
- **raegfx-runtime**: command buffer encoding, pipeline cache, swapchain.
- **raegfx-backend-vulkan**: lowers to Vulkan on PC GPUs.
- **raegfx-backend-direct**: native driver path for RaeReady GPUs (later).
- **raegfx-translate-d3d**: DXVK/VKD3D heritage.

## Open design questions

- Shader language: SPIR-V only, or accept a friendlier Slang-like front end?
- Mesh shading abstraction across vendors that don't all have it natively
- Bindless vs. descriptor sets for the public API default
