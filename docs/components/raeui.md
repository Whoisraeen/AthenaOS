# AthUI

Native UI framework. Skia for 2D, wgpu for 3D and compositor effects. Glassmorphic
by default. The AppKit-on-Core-Graphics pattern: AthUI is the proprietary surface;
Skia and wgpu are implementation detail.

## Goals

- Native rendering, native input, native audio — sub-frame latency end to end
- Declarative, SwiftUI-style ergonomics, no GC
- Themes change *the rendering*, not just colors (frosted glass, CRT scanlines,
  holographic, brutalist — all GPU shaders)
- Vibe Mode: system-wide visual personalities as a coherent set
- Compositor-aware: every animation locked to the display refresh rate
- Live wallpapers that don't murder battery (paused when occluded)

## Non-goals

- Cross-platform support beyond AthenaOS (themes are AthUI; there's no Windows port)
- HTML/CSS compatibility. Web is via the PWA path, not this framework.

## Surface sketch

```rust
use raeui::prelude::*;

#[derive(View)]
struct Counter { count: State<i32> }

impl Counter {
    fn body(&self) -> impl View {
        VStack::new()
            .push(Text::new(format!("Count: {}", *self.count)))
            .push(Button::new("Increment").on_tap(|s: &mut Self| *s.count += 1))
            .padding(16)
            .glass(GlassLevel::Frosted)
    }
}
```

## Layering

- **raeui-core**: view tree, layout, state, diffing.
- **raeui-paint**: Skia-backed 2D paint pass.
- **raeui-fx**: wgpu shaders for glass, blur, holographic, CRT, etc.
- **raeui-compositor**: window manager hand-off, damage tracking, VRR-aware presentation.
- **raeui-themes**: declarative theme bundle format, sandboxed.

## Open design questions

- Reactivity: signals (Leptos / Sycamore style) vs. SwiftUI's `@State` analogue?
- Layout engine: bespoke (taffy-style) or fork an existing one?
- How much of the design system ships as code vs. as themable resource bundles?
