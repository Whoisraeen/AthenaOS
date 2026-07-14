# Visual QA — Round 3 — Liquid Glass Identity (2026-06-21)

Static-pixel critique of the host-rendered identity surfaces in
`docs/design/screenshots/` against `docs/design/IDENTITY.md` and the curated
references in `docs/design/reference/`. No QEMU boot — these are the kernel SW
rasterizer's own output via `tools/ui_screenshot`.

Measured with PIL (`Python310`), not eyeballed. Numbers below are reproducible.

---

## Verdict up front

**Identity parity vs macOS 26 Tahoe / Windows 11 24H2: ~35%.**

The two *structural* moves landed: the void is gone (there's a real backdrop now)
and three tiers exist as distinct surfaces. That alone is most of the 35%. But the
three things that make liquid glass *liquid glass* — **luminous frosted brightness,
the iridescent chromatic edge, and convincing backdrop refraction** — are all either
absent or so far under-cooked that the result still reads as "dark blue cards on a
blurry blue gradient," not "luminous glass on a living aurora." Against the gold
reference (`Liquid glass guide_...jpg`) the gap is stark: that image is *bright*,
the glass is near-white and clearly see-through, and the chromatic rim is
unmistakable. Ours is dark, and the rim measures to literally zero chromatic pixels.

This is fixable and the architecture is right. But do not let anyone call the
identity "done." It is a strong skeleton with none of the signature skin.

---

## Adjudicating the four specific questions

### Q1. The iridescent rim — is the subtlety right, or push toward 0x40?

**Verdict: it is not subtle, it is ABSENT. Push hard — and the alpha isn't even the
main bug.**

Hard data from `glass-iridescent-edge-3x.png` (the shot whose entire job is to be
the "instantly recognizable" signature):

| Hue family expected | Pixels found in the 3× crop |
|---|---|
| cyan `#7CE7FF` (top) | **0** (the "cyan-family" count is all blue glass body `~(59,108,150)`, not cyan) |
| violet `#B47CFF` (right) | **0** |
| warm amber `#FFC97C` (bottom) | **0** |
| white top-highlight | 720 px (present, fine) |

There is no chromatic sweep anywhere in the crop. gfx's "felt, not neon" is too
kind — it isn't felt, it's missing. Two root causes, in order:

1. **Hue is being lost, not just dimmed.** A 2px band at `0x33` (20%) additive over
   `(59,108,150)` blue glass should still shift the pixel measurably toward cyan/
   violet/amber. It doesn't — every rim pixel is still pure blue-slate. This reads
   like the rim is either (a) not being drawn at all on this surface, (b) being
   drawn *under* an opaque stroke that covers it, or (c) the additive blend is
   clamping/compositing such that a low-alpha colored add over a mid-blue base
   produces no perceptible hue delta. **This is a rendering correctness bug, not a
   tuning value.** Owner: **athena-gfx**.

2. **Once it actually draws, the alpha IS too low for the signature the owner
   wants.** The spec ceilings the rim at `0x2E–0x33` "to be felt, not neon." But the
   reference's rim is genuinely vivid — look at the secondary button and the
   bottom-right card in `Liquid glass guide_...jpg`: the chromatic edge is a clear,
   saturated halo, not a whisper. The owner's bar is "RECOGNIZABLE / visually
   stunning," and a whisper-rim is neither.

   **Concrete target:** raise the in-band ceiling. Set the three edge tokens to the
   `0x40` band cap: `GLASS_EDGE_CYAN 0x40_7CE7FF`, `GLASS_EDGE_VIOLET 0x40_B47CFF`,
   `GLASS_EDGE_WARM 0x40_FFC97C`, and widen the band from 2px → **3px** (`GLASS_EDGE_
   BAND_PX = 3`) so it survives the SW rasterizer's AA at 1×. Keep the host-KAT
   `[0x20, 0x40]` invariant — `0x40` is the legal ceiling, so this stays in-spec.
   The spec's `[0x20,0x40]` already anticipated this headroom; use it.
   Owner: token bump **athena-ui**, draw fix **athena-gfx**.

   Note for **athena-design-researcher**: the spec body text (§2.4) says "felt, not
   neon, ~18–20%" while the KAT ceiling is `0x40` (25%). The owner now wants it
   pushed to the ceiling. Reconcile the §2.4 prose to "felt at rest, but the
   signature must be legible at the corners — target the `0x40` band cap" so the
   spec stops contradicting the goal.

### Q2. Aurora — premium signature, or washed-out/banded?

**Verdict: structurally correct, but currently reads as a generic blurry-gradient,
not a curated wallpaper. Two concrete defects.**

Measured (`wallpaper-aurora-dark.png`, 1280×800): meanLuma 43.7, luma range
[13..94], brightest cell `(42,104,140)` at lower-left, most-saturated `(49,94,157)`,
center `(39,61,110)`, corners ≈ `(12,18,33)` (vignette is working — good).

What's right: the base is a real night-sky blue-violet, the vignette darkens corners
so chrome will read, the blue and violet/teal blobs are placed and visible, and luma
stays modest so text will survive. This is a genuine, shipping-grade *mood*.

What's wrong vs a shipping macOS/Win11 wallpaper:

1. **Peak luminance is too low — it looks underexposed.** Brightest pixel is luma
   ~94 (out of 255). The reference Fluent/aurora wallpapers (e.g. `download (1).jpg`)
   push their brightest ribbon to ~150–180 so the glass has something bright to
   refract. At 94-max the whole frame sits in the bottom 37% of the range and reads
   "dim," which is exactly why the glass on top can't look luminous — **there is no
   bright backdrop for the glass to be luminous against.** Lift the blue blob's peak
   contribution so its core hits luma ~140–150 (`AURORA_BLOB_BLUE` core alpha up
   ~25%), keep the violet/teal as accents. The vignette already protects the edges.
   Owner: **athena-gfx**.

2. **The blobs read as two soft circles, not "flowing aurora."** The reference and
   the Concept §3.1 word is *ribbon / mesh* — directional, flowing color, not round
   radial dots. Right now it's two `1/(1+d²)` circles on a gradient. To read as
   "aurora" rather than "blurry gradient," the blobs need to be **stretched/sheared
   along a diagonal axis** (anisotropic falloff, ~2.5:1 aspect along a NW→SE axis)
   so they become ribbons, and a third smaller teal accent should overlap the
   blue→violet seam to create the additive color-mixing zone that makes mesh
   gradients read as premium. Right now there's no visible blue×violet *blend* zone
   — they're separated. Owner: **athena-gfx** (falloff math), spec note to
   **athena-design-researcher** to pin "anisotropic ribbon, not isotropic blob."

   No banding artifacts detected (smooth luma falloff) — that part is clean.

### Q3. Glass luminance/translucency — does the backdrop read through at each tier?

**Verdict: the backdrop bleeds through (good — it's translucent), but the glass is
DARK, not luminous, and the tiers are visually indistinguishable. This is the
biggest single miss vs the reference.**

Measured interiors from `glass-tiers-over-aurora.png` (backdrop just above panels =
`(18,29,53)`):

| Surface | Interior RGB | Reads as |
|---|---|---|
| backdrop (no glass) | (18,29,53) | — |
| **chrome (25%)** | (24,49,75) | slightly lighter than backdrop |
| **panel (45%)** | (30,46,81) | ~same as chrome |
| **popover (60%)** | (27,31,59) | *darker* than chrome/panel |

Two problems fall straight out of this table:

1. **Tier discipline is not visually legible.** Chrome (24,49,75) and panel
   (30,46,81) are within ~3 levels of each other, and popover (27,31,59) is actually
   the *darkest* of the three — because it happens to sit over a darker patch of
   backdrop, the per-tier opacity difference is swamped by backdrop variance. The
   spec's headline promise ("opacity rises left→right, you can see the tiers")
   **fails the eye test.** The tint alphas (0x40/0x73/0x99) are correct in the
   tokens, but because the tint color is so dark and close to the backdrop, adding
   more of it barely changes the result. Owner: **athena-gfx** — the tier separation
   must survive backdrop variance.

2. **The glass is dark glass, the reference is LIGHT glass.** This is the core
   identity gap. In `Liquid glass guide_...jpg` and `download (1).jpg`, the glass is
   a luminous near-white/milky frost — it *adds* light. Ours is a dark blue-slate
   tint that *subtracts* light (every interior is darker-blue than a bright backdrop
   would be). The spec's own §0 table calls the current look "dark muddy navy" as a
   FAIL, and we have... a less-dark but still-navy, still-subtractive glass. To read
   as *frosted* glass on the dark theme, the tint needs a **luminous floor**: composite
   a low-alpha *white/light* frost (≈ `0x14_FFFFFF` add) over the blurred backdrop
   *before* the colored tier tint, so the glass brightens the backdrop the way frost
   scatters light, then the slate tint colors it. Right now there's tint but no
   frost-lightening. **This single change is what will move the glass from "dark
   card" to "frosted glass."** Owner: **athena-gfx**; spec note to
   **athena-design-researcher** to add a `GLASS_FROST_LIGHTEN` token (≈`0x14` white
   add) to §2 — the current tier model has alpha+tint but no luminance-add term, and
   that's the missing ingredient vs the reference.

### Q4. Control Center — #1 remaining defect (besides the known internal-tint repoint)?

**Verdict: the panel does not sit on the aurora — it sits in a dark gutter, and the
whole composition is starved of the identity it's supposed to showcase.**

From `surface-control-center.png`: the CC panel is docked hard to the right edge,
and the region immediately around/behind it is the *darkest* part of the frame —
the aurora's bright blue blob is bottom-left, nowhere near the panel. So the headline
fix surface is rendered over the deadest part of the backdrop, which means:

- **#1 defect: no soft drop shadow reads, and there's no backdrop refracting through
  the panel** — because the panel is over near-black, not over color. The shadow
  (`elev.3`, 40px soft ambient) is invisible against the dark gutter, and the glass
  has nothing colorful to refract, so even after the internal-tint repoint it will
  still look like a dark slab unless the *backdrop behind it has color.* Fix: either
  (a) shift the aurora composition so a color blob falls behind the right-docked CC
  (place the violet blob upper-right instead of bottom-right for the CC capture), or
  (b) confirm on a real desktop the CC will routinely sit over wallpaper color. For
  the proof shot specifically, the panel must be photographed over a lit region or
  the surface will always under-sell. Owner: **athena-gfx** (aurora blob placement) +
  **athena-shell-apps** (CC docking / where the panel lands).

- Secondary (after the known internal-tint repoint): the CC's internal cards have no
  visible iridescent rim and the toggles are still square-ish flat fills, not the
  pill-with-accent-glow the reference shows (`Liquid glass guide_...jpg` primary/
  secondary buttons). That's the §11 Step 5 re-skin and is already queued — owner
  **athena-shell-apps** — so it's not the surprise defect, but it stays on the list.

---

## Prioritized defect list (each: surface → problem → fix → owner)

**P0 — blocks the identity reading as liquid glass at all**

1. **Iridescent rim → renders ZERO chromatic pixels** (measured: 0 cyan, 0 violet, 0
   amber in the 3× crop; only a white hairline). The signature is absent, not subtle.
   → Fix the draw so the colored additive band actually shifts rim pixels toward
   cyan/violet/amber (correctness bug — rim either not drawn, drawn under an opaque
   stroke, or additive-clamped away). Then bump alpha to the `0x40` ceiling and band
   to 3px. → **athena-gfx** (draw correctness, the real bug) + **athena-ui** (token
   `0x33→0x40`, `GLASS_EDGE_BAND_PX 2→3`).

2. **Glass is dark/subtractive, not luminous/frosted** (interiors are darker-blue
   than a bright backdrop; reference glass is milky and light-adding). → Add a
   luminance-add frost term (`≈0x14_FFFFFF` over the blurred backdrop) before the
   slate tint, so glass brightens like real frost. → **athena-gfx**; new token
   `GLASS_FROST_LIGHTEN` from **athena-design-researcher** + **athena-ui**.

**P1 — the backdrop and tier legibility**

3. **Aurora peak too dim** (brightest luma 94/255; reference ~150–180). Whole frame
   underexposed → glass has nothing bright to refract. → Lift blue-blob core peak to
   luma ~140–150; vignette already protects edges. → **athena-gfx**.

4. **Tiers visually indistinguishable** (chrome 49 / panel 46 / popover 31 on G —
   ordering inverted by backdrop variance). The "opacity rises left→right" promise
   fails the eye. → Make tier separation survive backdrop variance (the frost-lighten
   in #2 helps; consider tying a small fixed luminance step to tier, not just alpha).
   → **athena-gfx**.

5. **Aurora reads as 2 round blobs, not flowing mesh.** → Anisotropic (sheared,
   ~2.5:1, NW→SE) falloff + a third teal accent over the blue×violet seam to create a
   visible color-blend zone. → **athena-gfx**; spec pin from **athena-design-researcher**.

**P2 — surface composition / re-skin (mostly already queued)**

6. **Control Center docked over the dead/dark backdrop gutter** → shadow + refraction
   both invisible. → Place a color blob behind the right-docked CC for the proof shot
   / confirm real-desktop placement. → **athena-gfx** + **athena-shell-apps**.

7. **CC internal glass tint + flat square toggles** (known repoint + pill/accent-glow
   controls per reference). → §11 Step 5 re-skin. → **athena-shell-apps**.

8. **Notifications toasts** (`surface-notifications.png`): toasts read as flat dark
   slates with hairline borders — same dark-glass + missing-rim issues as the panels;
   they'll inherit the P0/P1 fixes. Body text is tiny/low-contrast over the toast —
   hand the measured ratio to **athena-accessibility** once the frost-lighten lands
   (brighter glass changes the contrast math). → inherits P0/P1; a11y to confirm.

---

## Reference comparison (named gaps, with source)

- **`reference/Liquid glass guide_...jpg` (gold standard):** their glass is luminous
  near-white milky frost with a vivid chromatic rim and long soft shadows on a *light*
  backdrop. Ours is dark blue-slate glass with no rim and shadows lost in a dark
  gutter. Gap: **luminance polarity** (light-adding vs dark-subtractive glass) + the
  **missing chromatic edge**. These two are 80% of the felt difference.
- **`reference/download (1).jpg` (W11 Fluent Files on aurora):** their backdrop is a
  bright flowing blue ribbon (peak luma ~160+); the Files glass is a light translucent
  panel. Gap: our aurora peaks at luma 94 (underexposed) and our glass is dark. Same
  two root causes.
- **macOS 26 Tahoe (knowledge):** Tahoe glass is defined by specular highlight + edge
  light-bending + genuine background luminance lift. We have the top-edge highlight
  (good) but neither the edge light-bend (the rim, which renders to nothing) nor the
  luminance lift (frost term, absent).
- **Win11 24H2 Acrylic/Mica (knowledge):** Acrylic is *lighter* than its backdrop and
  noisy-frosted; Mica tints toward the wallpaper. Ours currently behaves like neither
  — it's a flat dark tint. The frost-lighten term (#2) is what closes this.

## Consistency issues

- Tier ordering not visually monotonic (P1 #4) — the one cohesion promise the tier
  system exists to deliver, currently failing the eye test.
- Corner radii look consistent across panels/toasts (lg ~16) — **no defect**.
- Top-edge highlight present and consistent across surfaces — **good, keep it.**
- Vignette consistent and correct — **good.**

## Blocking (won't render)

None — all five surfaces rendered cleanly via the host rasterizer. No handoff to
verifier/debugger needed.

## Confidence

**High** on the rim being absent and the glass being dark/subtractive (both
measured to hard numbers — 0 chromatic rim pixels, interior luma below a bright
backdrop). **Medium** on the exact target values (frost `0x14`, aurora peak ~140,
rim `0x40`/3px are informed starting points, not proven — they need one host-render
iteration each to dial in). **High** on the ~35% parity score: the skeleton is real,
the signature skin is not yet there.
