# Design Spec: Material & Shadow (the systemic premium-feel fix)

> *"Built for people who care about how things feel."* — LEGACY_GAMING_CONCEPT.md
>
> This spec exists because athena-visual-qa found the single biggest "looks basic,
> not premium" signal in the whole UI: the drop shadow renders as a **hard,
> opaque, blue, offset duplicate rectangle**, not a soft ambient shadow. It is
> systemic — it hits *every* `elev.*` surface (windows, Start, flyouts, toasts,
> Quick Look, the OOBE card). Fix it once in the compositor and every surface
> goes premium. See `critique-desktop-2026-06-17.md` finding #1.

**All tokens below are defined in [`design-language.md`](./design-language.md).**
This spec tightens the *rendering contract* behind `elev.*` and `material.*`; it
adds no new tokens — it makes the existing ones actually look right.

---

## Concept promise + bar to clear

> "glassmorphic, GPU-accelerated… looks like Metal." — LEGACY_GAMING_CONCEPT.md (§AthUI)

- **Bar to clear:** macOS Sequoia and Windows 11 card/dialog shadows — **wide,
  soft, low-alpha, near-black ambient shadows** with a smooth penumbra and no
  visible edge. The current AthenaOS shadow reads as "a flat sticker with a colored
  ledge" — the opposite of premium.

---

## The defect (measured, from visual-QA)

Pixel measurements on a clean OOBE frame (`zoom-shadow-corner-3x.png`):

- Below the card edge the pixel snaps to a flat `(78,123,200)` and holds ~12px,
  then **hard-cuts** back to wallpaper — a solid offset duplicate with sharp inner
  AND outer edges.
- The color is **blue** (inheriting the wallpaper), not the spec's near-black
  `0x66_00_00_00`.
- **Zero feathering / penumbra.** The documented quadratic falloff is collapsing
  to a constant, OR the surface is drawing its own flat offset rect instead of
  calling `compositor::render_drop_shadow`.

Two root causes to chase (athena-gfx):
1. **Is `render_drop_shadow` even invoked for this surface?** If the OOBE card /
   window draws its own offset rect, delete that and route through the compositor
   `SurfaceEffect::DropShadow`.
2. **If invoked, the falloff term is wrong** — a quadratic falloff that produces a
   hard edge means the distance-to-alpha curve is clamped/stepped, or the shadow
   buffer is being alpha-composited as opaque, or the color is sampling the
   backdrop instead of the constant shadow color.

---

## The rendering contract (what `elev.*` must actually produce)

`compositor::SurfaceEffect::DropShadow { offset_x, offset_y, radius, color }` must
produce a **separable Gaussian-approximating soft shadow**, identical in spirit to
the blur engine that already works (`BlurEngine::box_blur_3pass` — a 3-pass box
blur ≈ Gaussian). The shadow is just *a blurred, offset, solid-color silhouette of
the surface's alpha mask, composited under the surface.*

### Algorithm (reuse the working blur path)
1. Take the surface's **alpha mask** (the rounded-rect coverage, incl. the
   `radius.*` corners — the shadow must be rounded, not a square block).
2. Fill it with the **shadow color** (`color`, e.g. `0x66_00_00_00` — near-black,
   alpha = the elevation's strength). **Never sample the backdrop** — the shadow
   color is a constant; the blue tint is the bug.
3. **Blur** that silhouette by `radius` using the existing 3-pass box blur (same
   code as glass). A `radius=40` shadow gets a ~40px penumbra — that IS the
   softness. This is why the falloff must be a real blur, not an analytic
   per-pixel falloff that's collapsing.
4. Offset by `(offset_x, offset_y)` and **alpha-composite UNDER** the surface
   (source-over with the surface on top). The shadow's own alpha must be honored —
   it is translucent, so the wallpaper shows through the penumbra.

### Per-elevation contract (the `elev.*` ladder must look like this)

| Token | offset_y | blur radius | color (dark) | reads as |
|---|---|---|---|---|
| `elev.1` | 1 | 6 | `0x30_00_00_00` | a hairline lift (taskbar, resting card) |
| `elev.2` | 3 | 14 | `0x40_00_00_00` | a soft floated menu/toast |
| `elev.3` | 8 | 28 | `0x55_00_00_00` | a clearly-above modal (Start, quick-settings) |
| `elev.4` | 12 | 40 | `0x66_00_00_00` | a wide ambient dialog shadow (OOBE card, dragged window) |
| `elev.focus` | 0 | 10 | `accent.glow` (`0x66_4E_9C_FF`) | an **additive glow** ring, not displacement (centered, no offset) |

- **Light mode:** multiply shadow alpha by ~0.6 (shadows read heavier on light bg).
- **`elev.focus` is different:** zero offset, `accent.glow` color, additive over
  the surface edge — a glow, not a cast shadow. Currently focus shows only a 1px
  ring (visual-QA finding #3); it must be ring **+ glow**.

### Acceptance: the penumbra test
A correct `elev.4` shadow, sampled along a line crossing the card's bottom edge
outward, shows alpha **monotonically decreasing from ~40% to 0 over ~40px** — a
smooth ramp, not a flat band then a step. athena-visual-qa proves this with a pixel
sample line (the same method that caught the defect).

---

## Material recipe corrections (while we're here)

Two `material.glass` details visual-QA flagged as systemic chrome rules:

1. **Top-edge highlight:** every `material.glass` surface gets a 1px `stroke.strong`
   highlight along its *top* edge (the macOS Liquid Glass cue) and 1px
   `stroke.subtle` on remaining edges. Confirm this is drawn (the OOBE card frame
   should show it).
2. **Chrome color restraint:** static labels/headings render in **neutral text
   tokens** (`text.primary`/`text.secondary`), **never the accent** (visual-QA
   finding #2: title/subtitle/labels were rendering blue). Accent is reserved for
   interactive fills, selection, and focus. This is a draw-time rule for every
   surface, enforced by reading text color from `text.*`, not `accent.*`. (This
   half is a athena-ui / athena-shell-apps fix at the call sites, not a compositor
   fix — flagged here because it's the same "looks basic" cluster.)

---

## States & interaction

This surface is non-interactive (it's a rendering contract), but the *states it
serves* must all use the corrected ladder:
- **resting** surfaces → `elev.1`/`elev.2` soft shadow.
- **raised/modal** → `elev.3`/`elev.4`.
- **focused control / window** → `elev.focus` accent glow **+** 2px `accent.base`
  ring (never glow-only or ring-only).
- **dragged window** → `elev.4` (the lift cue).
- **reduced-motion:** shadows are static — no change; reduced-motion does not
  affect shadow rendering (it's not animated). The *focus glow appearing* uses
  `motion.micro`; reduced-motion makes that appearance instant.
- **dark / light:** alpha ×0.6 in light.

---

## Already built (delta only — verify-before-spec)

| Capability | Where | State | This spec |
|---|---|---|---|
| 3-pass box blur (≈ Gaussian) | `compositor::BlurEngine::box_blur_3pass` | LIVE, works (glass is soft) | **reuse it** for the shadow silhouette blur |
| Drop shadow | `compositor::render_drop_shadow` | LIVE but renders a hard blue block | rewrite the falloff to the blur-silhouette algorithm above |
| Per-surface shadow effect | `SurfaceEffect::DropShadow { offset, radius, color }` | LIVE (the carrier is fine) | the *carrier* stays; the *renderer* is fixed |
| Glass top-edge highlight | surface-drawn `stroke.strong` line | spec'd | confirm it's actually drawn |

**The fix is a compositor renderer change, not new tokens and not a rebuild** —
the blur math that makes this soft *already exists three functions away*.

---

## Handoff

### Implementers
- **athena-gfx (PRIMARY):** rewrite `compositor::render_drop_shadow` to the
  blur-silhouette algorithm — rounded alpha mask → constant shadow color → 3-pass
  box blur by `radius` → offset → composite-under. Kill any backdrop color
  sampling (the blue). Make `elev.focus` an additive `accent.glow` glow. Audit
  every surface that draws its own offset rect (OOBE card, `window_chrome.rs`) and
  route them through `SurfaceEffect::DropShadow` instead.
- **athena-ui / athena-shell-apps:** the chrome-color-restraint half — fix call
  sites that color static labels/headings with `accent.*`; read text color from
  `text.*`. (OOBE card title/subtitle/labels first; then audit Settings group
  headers, Files metadata, every label.)
- **athena-accessibility (flagged):** the neutral-label fix must still clear AA —
  `text.secondary` ≥ 4.5:1 on the light card bg; the focus glow + ring must be the
  a11y-required visible focus (glow alone is insufficient for low-vision; the 2px
  ring is the contract).

### On-screen / boot-log evidence (athena-visual-qa + smoketests)
- **The penumbra test:** screenshot of the OOBE card (or any `elev.4` surface)
  with a pixel-sample line across the bottom edge proving a smooth ~40px alpha
  ramp from ~40%→0, near-black (not blue). This is the headline proof.
- **Rounded shadow:** zoom crop of a card corner showing the shadow is *rounded*
  (follows the `radius.*` mask), not a square block.
- **Elevation ladder:** one frame showing `elev.2` (toast), `elev.3` (Start),
  `elev.4` (window) side by side — visibly increasing soft-shadow spread.
- **Focus glow:** screenshot of a focused field showing accent glow + 2px ring,
  distinct from a plain border (visual-QA finding #3 cleared).
- **Neutral labels:** zoom crop of the OOBE title/subtitle now in `text.primary`/
  `text.secondary` neutral, accent reserved for the button only (finding #2).
- **Boot log:** extend `compositor::run_boot_smoketest` to render an `elev.4`
  shadow into a scratch buffer and assert the alpha ramp is monotonic-decreasing
  and the color channel is the constant shadow color, not backdrop-sampled (must
  be able to print FAIL — e.g. FAIL if the edge alpha steps >X per px or any
  blue channel leaks in).

### Unblocks (MasterChecklist)
- **Phase 8 (AthUI/AthKit):** the `elev.*` material system is only "done" when the
  shadow looks soft — this is the gate.
- **Every surface spec** (`desktop-shell`, `settings`, `files`,
  `window-management`) depends on this: they all reference `elev.*` and inherit the
  defect until it's fixed. This is the highest-leverage single fix in the UI.
