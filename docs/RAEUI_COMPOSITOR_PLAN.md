# AthUI: retained layer tree + compositor-driven animation (the "macOS feel")

**Goal:** deliver the macOS/iOS *feel* — buttery, GPU-composited, declarative
animation that stays at refresh rate even when the app thread is busy. Per
`LEGACY_GAMING_CONCEPT.md §AthUI` ("compositor-aware, GPU-accelerated, glassmorphic by
default, SwiftUI-style API").

**Key insight (why this doc exists):** the feel is NOT the 2D library (Skia /
Vello / tiny-skia just draw *pixels*). The feel is the **architecture** macOS
calls Core Animation: a **retained layer tree**, **GPU compositing**, and
**animations the compositor runs off the app's main thread**. Skia is our
Core-Graphics-equivalent; this plan is our Core-Animation-equivalent.

## Current state (2026-06-11)

AthUI already has the logical pieces — but wired the wrong way for the feel:

| Module | Has | Gap for the feel |
|---|---|---|
| `tree.rs` (522) | retained `WidgetTree`/`WidgetNode`, `layout()` | nodes are NOT layer-backed; rendered immediate-mode |
| `animation.rs` (626) | `EasingFunction`, `Animation`, `AnimatedProperty`, repeat modes | ticked app-side; animates widget content, not compositor layers |
| `layout.rs` (635) | flexbox (`Display`/`FlexDirection`/`JustifyContent`/`Edges`) | hand-rolled; `taffy` is the drop-in upgrade |
| `binding.rs` (445) | `Observable` reactive state | not wired to implicit animations |
| render path | `raegfx::Canvas` (software), placeholder "rectangle" font raster | immediate-mode redraw; no GPU layers; no real text |

So content is redrawn every frame through a software canvas, and animation runs
on the app thread — the exact opposite of the Core Animation model. The fix is
to **split content rendering from compositing**, **back nodes with cached layer
textures**, and **move animation execution to the compositor's SCHED_BODY
thread**.

## Target architecture

```
  App thread                        Compositor (SCHED_BODY, off app thread)
  ──────────                        ───────────────────────────────────────
  WidgetTree (retained)             Layer tree (transform, opacity, material)
    │ layout (taffy)                  │ per-frame:
    │ content dirty?  ── yes ──►      │   - run AnimationDriver (mutate layer
    │   rasterize layer texture       │     transform/opacity/radius/blur)
    │   (tiny-skia now / Skia GPU      │   - composite all layers (GPU blend)
    │    later) ── upload ──►          │   - system material passes (vibrancy)
    │ content unchanged ► reuse        │   - present (VRR/HDR)
    cached layer texture              (NEVER calls app code)
```

Two independent clocks: the **app** re-rasterizes a layer only when its
*content* changes; the **compositor** re-composites + animates *every frame*.
A spring animation on a layer's position needs zero app involvement → it cannot
jank when the app is busy. That is the whole trick.

## Workstreams

### A. Layer backing (AthUI — `tree.rs`, new `layer.rs`)
- **A1.** Add `Layer { surface: SurfaceHandle, transform: Affine, opacity: f32,
  corner_radius: f32, material: Material, needs_redraw: bool }`. A `WidgetNode`
  owns an optional `Layer` (leaf widgets + anything animated get one).
- **A2.** Split `Widget::render`: (a) `draw_content(&self, canvas)` rasterizes
  into the layer's surface **only when `needs_redraw`**; (b) the compositor
  consumes `(surface, transform, opacity, material)` each frame.
- **A3.** Backing-store cache: keep layer surfaces across frames; invalidate on
  content/size/scale change. This is what makes scrolling/animation cheap.

### B. Compositor-driven animation (kernel `compositor.rs` + AthUI `animation.rs`)
- **B1.** Move the `Animation` registry into a compositor-side `AnimationDriver`
  ticked on the **SCHED_BODY** thread (it already runs the VRR pacer there).
- **B2.** Animations target **layer** properties (transform/opacity/radius/blur),
  not widget content — so a running animation never re-rasterizes or calls app
  code.
- **B3.** Implicit animations (SwiftUI `.animation()`): a property mutation
  inside a `with_animation(curve, duration) { … }` scope auto-creates a layer
  animation. Wire `binding.rs::Observable` changes through this.
- **B4.** Spring physics — add a critically-damped spring solver beside the
  easing curves in `animation.rs`. Springs (not just bezier easing) are what
  make iOS feel alive.
- **B5.** Acceptance test: an app that spins a busy loop while a layer
  spring-animates — the animation MUST hold refresh rate. This single test is
  the definition of "the feel" and gates the whole effort.

### C. Real content rendering (swap the placeholder)
- **C1. NOW (no GPU needed):** replace the rectangle font rasterizer + software
  fills with **`tiny-skia`** (CPU, `no_std`+alloc) → real anti-aliased paths,
  gradients, blends into layer surfaces. Makes AthUI *look* good immediately,
  before any GPU work.
- **C2. Text:** integrate **`cosmic-text`** (shaping/BiDi/ligatures/fallback) →
  glyph runs → tiny-skia raster. (Already recommended in OSS doc.)
- **C3. LATER (Phase 6 GPU):** swap tiny-skia → **`skia-safe`** (Graphite) **or
  `vello`** rendering into `wgpu` textures. The A/B architecture is unchanged —
  that is the payoff of splitting content from compositing.

### D. System materials + color (kernel `compositor.rs`)
- **D1. Glassmorphism/vibrancy:** a compositor blur pass sampling the backbuffer
  *behind* a layer flagged `Material::Glass` (the blur pipeline exists — wire it
  system-wide via the layer material flag, not per-app Skia blur).
- **D2. Color management:** `palette` for sRGB↔linear↔Display-P3; wire to the
  existing HDR pipeline so wide-gamut + HDR are correct, not approximated.
- **D3. Retina/point scaling:** lay out in points; render layer surfaces at
  device pixels with an @1x/@2x/@3x backing scale, like macOS.

## Sequencing (what to do first)

**C1 + A1–A2 + B1–B4 need NO GPU** — they run against the existing software
compositor. Do them now:

1. `tiny-skia` content rendering (C1) — instant visual upgrade.
2. `taffy` layout (replace `layout.rs` internals) — correct, battle-tested.
3. Layer backing (A1–A3) — split content from compositing.
4. Compositor `AnimationDriver` + springs + implicit animations (B1–B4).
5. The busy-app animation acceptance test (B5) — prove the feel.
6. THEN, when Phase 6 GPU lands: swap tiny-skia→Skia/Vello (C3), wire materials
   + color + Retina (D). Architecture unchanged.

**Acceptance for "the feel":** a glassmorphic panel that spring-animates at
refresh rate while its owning app thread is blocked, with anti-aliased
cosmic-text text — achievable entirely on the software path before the GPU.

## Crate dependencies

See `docs/OSS_RECOMMENDATIONS.md` "AthUI rendering + feel stack" — `tiny-skia`,
`taffy`, `cosmic-text`/`swash`, `palette`, and the Linebender primitives
(`kurbo`/`peniko`/`vello`) for the GPU path. Align AthUI's geometry/paint types
with `kurbo`/`peniko` so the eventual Skia-or-Vello swap is a backend change,
not a rewrite.
