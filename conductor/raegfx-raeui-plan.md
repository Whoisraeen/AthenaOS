# Implementation Plan: AthGFX & AthUI Foundation

## Background & Motivation
With the foundational kernel components (scheduler, capability IPC, VFS, and user-space transitions) successfully implemented, AthenaOS is ready to transition from text-mode/serial output to a graphical desktop. The concept document outlines a embodiment-first, native-feeling OS with sub-frame latency. To achieve this, we need a native graphics API (`AthGFX`) and a declarative UI framework (`AthUI`). This plan focuses on the immediate next milestones: drawing pixels to the screen via `AthGFX` and establishing the core event loop for a `AthUI` window.

## Scope & Impact
This plan covers:
1. **AthGFX Initialization**: Exporting the kernel framebuffer capability to userspace and building a basic 2D line rasterizer.
2. **AthUI Core**: Creating a foundational windowing abstraction, an event loop, and a basic widget (a button) that renders using AthGFX.

This does *not* cover:
- Hardware-accelerated Vulkan/3D rendering (future Year 1 milestone).
- Full compositor window management or Vibe Mode themes.
- Complex input routing (keyboard/mouse will be integrated into the event loop, but advanced game controller support is deferred).

## Proposed Solution
- **AthGFX (`components/raegfx`)**: Implement a userspace library that takes a framebuffer memory map (provided via the `SYS_MMIO_MAP` syscall) and provides a 2D rendering canvas. It will include basic primitives: `clear`, `draw_pixel`, `draw_rect`, and `draw_line`.
- **AthUI (`components/raeui`)**: Implement an event-driven UI framework on top of AthGFX. It will introduce a `Window` struct, a `Widget` trait, and an `EventLoop` that polls for user input (via IPC channels from the keyboard driver) and triggers `render` passes.

## Alternatives Considered
- **Porting an existing UI library (e.g., Slint, Iced)**: While faster, this violates the AthenaOS core principle of a deeply integrated, proprietary native stack optimized for specific latency and architectural goals. We will build the foundational abstractions ourselves, potentially leveraging Skia/wgpu *under the hood* later as per the architecture doc, but the immediate goal is raw pixel manipulation to prove the IPC/Framebuffer pipeline.

## Implementation Plan

### Phase 1: AthGFX 2D Rasterizer
1. **Framebuffer Mapping**: Update `driver_supervisor` (or a dedicated display driver) to pass the framebuffer capability to the `raegfx` component.
2. **Canvas Abstraction**: Create `Canvas` in `raegfx/src/lib.rs` that wraps the raw pixel buffer.
3. **Drawing Primitives**: Implement `draw_pixel`, `draw_line` (Bresenham's algorithm), and `fill_rect` in `Canvas`.

### Phase 2: AthUI Window & Event Loop
1. **Widget Trait**: Define a `Widget` trait in `raeui/src/lib.rs` with `render(&mut Canvas)` and `on_event(Event)`.
2. **Window Abstraction**: Create a `Window` struct that holds a root `Widget` and manages a backbuffer.
3. **Event Loop**: Implement an `EventLoop` that waits for IPC messages (keyboard/mouse events) and calls the window's render/event methods.
4. **Button Widget**: Implement a `Button` struct implementing the `Widget` trait.

### Phase 3: Integration & Demo
1. **Demo App**: Create a simple userspace application (e.g., `user_init`) that links `raegfx` and `raeui`.
2. **Execution**: The app will initialize a window, add a button, and enter the event loop, rendering to the screen.

## Verification
- Compile the kernel and components using `cargo run -p xtask`.
- Verify in QEMU that the framebuffer successfully transitions from the boot banner to the AthUI window displaying a button.
- Verify that keyboard input triggers an event (e.g., clicking the button changes its color or logs a message via `sys_print`).

## Migration & Rollback
- These changes are purely additive to the `components` directory and user-space binaries. If issues arise, we can roll back the `user_init` or `driver_supervisor` entry points to the previous stable state without affecting the kernel's stability.