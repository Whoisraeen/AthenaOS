# RaeenOS IDENTITY — The Liquid Glass Design Language

> *"Built for people who care about how things feel."* — RaeenOS_Concept.md (§RaeUI)
>
> The owner's verdict on the current build: **"it has no native theme or identity
> that is clean and stunning."** This document fixes that. It defines the ONE
> signature look that makes RaeenOS instantly recognizable and on par with — or
> better than — macOS 26 "Tahoe" Liquid Glass and Windows 11 Mica/Acrylic.
>
> This is the **identity layer**. It sits above `design-language.md` (the full
> token catalog) and `material-and-shadow.md` (the shadow rendering contract). On
> any conflict about *what RaeenOS looks like*, this file wins; on token plumbing
> mechanics, `design-language.md` + `rae_tokens` win. New tokens proposed here are
> collected in the **Token Diff** at the end so `rae_tokens` can implement them
> mechanically.

---

## 0. The verdict, made concrete (current pixels vs. the bar)

Side-by-side of our shipped Control Center (`screenshots/surface-control-center.png`)
against the reference set (`reference/Liquid Glass UI Kit`, `reference/download (1).jpg`,
`reference/download (2).jpg`):

| Dimension | Reference (the bar) | Our current pixels | Verdict |
|---|---|---|---|
| Glass luminance | luminous, **light** frosted glass; backdrop clearly visible through it | dark muddy navy panel @ ~62% alpha over a near-black void → reads near-opaque | **FAIL** — glass looks like a flat dark card |
| Backdrop | flowing color (aurora ribbon / mesh) gives glass something to refract | flat `0x0A0E1A→0x1A2844` navy gradient = a **void** | **FAIL** — the "flat void desktop" defect |
| Edge | iridescent chromatic rim (cyan→violet→warm) + bright top highlight | flat 1px white-ish hairline, no refraction | **FAIL** — no liquid-glass signature |
| Shadow | long, soft, wide near-black ambient | renderer fixed (material-and-shadow.md) but invisible on a void | partial — needs a backdrop to read against |
| Buttons | pill, colored inner glow, glossy | square-ish, flat fills | **FAIL** |
| Accent | one confident, recognizable hue | RaeBlue exists but drowns in the navy | underused |

**Root cause is not one bug — it is the absence of an identity.** Three systemic
moves fix 90% of it: (1) brighter **tiered** glass, (2) a **signature aurora
backdrop** for the glass to refract, (3) the **iridescent edge**. Everything else
is application of tokens we mostly already have.

---

## 1. The single rule (frame everything around this)

> **"Use tiers early. Stop inventing new glass per screen."**
> — `reference/Liquid glass guide_ Use tiers early...`

RaeenOS ships **exactly three glass tiers and never a fourth.** Every translucent
surface in the OS picks one of `glass.chrome`, `glass.panel`, `glass.popover`.
No surface is allowed to invent its own alpha/blur/tint. This is *the* cohesion
mechanism — it is the difference between "a collection of pretty screens" and "one
system." A per-surface application table (§7) assigns the tier for every surface so
there is no judgment call at the call site.

If a designer ever feels they need a fourth glass, the answer is: pick the nearest
of the three and adjust *elevation* (shadow) and *accent*, not the material.

---

## 2. The signature material system — three glass tiers

All values ARGB `0xAARRGGBB`, compositor-native. These REPLACE the single
`GLASS_TINT_DARK/LIGHT` pair, which is too dark and untiered. The defining change
vs. today: **glass is brighter and more translucent so the backdrop reads through
it**, and every tier carries the **iridescent edge**.

### 2.0 The frost luminance-add (the missing ingredient)

The tier model was **alpha + slate tint only**, which is purely *subtractive* — it
can darken the backdrop but never lift it, so glass read as a "dark card," not
"frosted glass." Round-3 visual-QA measured the identity at ~35% parity for exactly
this reason. The fix is a **luminance-add term**: a low-alpha **WHITE**
(`0xFFFFFF`) composited as a frost sheen, token `GLASS_FROST_LIGHTEN` (≈8% white,
the reference magnitude).

Compositing order inside `draw_glass_surface` (raegfx) is therefore:

> blurred backdrop → slate **tint** → **frost** white-add → iridescent rim

The frost is laid **on top of the tinted glass** (not before it) so the slate can't
re-darken it away. Each tier carries its OWN per-tier `frost` (a field on
`GlassTier`, white RGB, only the alpha differs): a **FIXED per-tier luminance step**
— dark `0x04 / 0x23 / 0x38`, light `0x06 / 0x18 / 0x2E` for chrome / panel /
popover. Because it is fixed (not derived from the tint alpha), the interior
luminance is **monotonic chrome < panel < popover regardless of the backdrop**,
which is what fixes the Round-3 tier inversion (backdrop variance was swamping the
alpha-only steps so a busy backdrop could flip the ordering). The
`tier_luminance_is_monotonic` KAT flattens each tier (tint then frost) over a fixed
mid backdrop and asserts the strict chain.

### 2.1 The tiers (dark theme — default)

| Tier | Token | Tint RGBA | Effective alpha | Blur radius | Use |
|---|---|---|---|---|---|
| **Chrome** | `glass.chrome` | `0x40_1A_22_3A` | **25%** (very see-through) | 24px | Taskbar, title bars, the always-on system chrome that frames content. Must show the most backdrop — chrome should feel like it floats *on* the wallpaper. |
| **Panel** | `glass.panel` | `0x73_1C_24_3E` | **45%** | 20px | The workhorse. Control Center, Start, Settings panes, Files sidebar, large cards. Legible but clearly translucent. |
| **Popover** | `glass.popover` | `0x99_1E_26_42` | **60%** | 16px | Transient surfaces over arbitrary content: menus, flyouts, toasts, tooltips, command palette. Slightly more opaque so text is instantly readable over a busy backdrop. |

Notes:
- **Tint hue moved off pure navy.** `0x1A223A`–`0x1E2642` is a *blue-violet slate*,
  not the dead `0x12_16_24`. Under the aurora backdrop it picks up color and reads
  as glass, not as a gray card. The old `0x1A1E2E` was too neutral-dark.
- **The single biggest fix is the alpha drop.** Today's glass is `0x9E` (62%) which,
  over a near-black void, is visually ~opaque. Chrome at 25% and panel at 45% let
  the backdrop through — that is what makes it *glass*.

### 2.2 The tiers (light theme — "Lumen")

Light glass is the *luminous frosted* look in `reference/download (1).jpg` and the
UI Kit — a near-white milky glass with a faint cool tint.

| Tier | Token | Tint RGBA | Effective alpha | Blur radius |
|---|---|---|---|---|
| **Chrome** | `glass.chrome` (light) | `0x59_F4_F7_FF` | 35% | 24px |
| **Panel** | `glass.panel` (light) | `0x8C_FB_FC_FF` | 55% | 20px |
| **Popover** | `glass.popover` (light) | `0xB3_FF_FF_FF` | 70% | 16px |

The faint cool blue (`F4F7FF`) instead of pure white is deliberate — pure-white
glass reads as plastic; the cool tint reads as glass.

### 2.3 The luminance rule (over-light / over-dark adaptation)

Glass over an arbitrary backdrop must stay legible without going opaque. The
compositor samples the **mean luminance of the blurred backdrop under the surface**
(it already has the blurred buffer — this is one extra reduction) and nudges the
tint alpha:

- **Over-bright backdrop** (mean luma > `GLASS_LUMA_HI`): add `+0x18` to the tier's
  alpha and darken the tint slightly so text stays legible. (Glass over a white
  photo must not wash out.)
- **Over-dark backdrop** (mean luma < 0.2): subtract `-0x14` from the alpha so the
  glass doesn't read as a solid black slab. (Glass over a black video stays glassy.)
- Clamp to `[tier_alpha-0x18, tier_alpha+0x20]` so the surface never strays far from
  its tier identity (the "stop inventing new glass" rule still holds — this is an
  automatic micro-adjust, not a new tier).

Tokens for the bounds: `GLASS_LUMA_HI = 0.38`, `GLASS_LUMA_LO = 0.2`,
`GLASS_ALPHA_BOOST = 0x18`, `GLASS_ALPHA_DROP = 0x14`.

**The legibility luma cap (dark-theme, white-text guarantee — SHIP-GATE a11y).**
The alpha micro-adjust above steers the look; it does **not** by itself guarantee
contrast. The hard guarantee is a cap on the *composited glass interior's*
effective luminance:

> On a **dark theme the body text is WHITE**, so glass cannot rise as bright as a
> light-theme reference without the text washing out. Glass over a **bright**
> backdrop is therefore **capped in effective luminance** where text sits; glass
> over a **dark** backdrop keeps its full frostiness. Stunning *and* legible.

- The composited interior (backdrop → tint → frost) is scaled uniformly toward
  black until its mean-channel luminance is **≤ `GLASS_INTERIOR_LUMA_CEIL` (0.40)**.
  Uniform scaling preserves hue (the glass keeps its tint cast, just dimmer).
- This is a **no-op over dark regions** (panel ≈ 0.22, popover ≈ 0.37 over the
  aurora base — both already under the ceiling), so the frosted, see-through
  dark-region look is preserved **exactly**. It bites **only** over a bright
  backdrop (an aurora blob, a light photo) where the frost-lifted interior would
  otherwise climb past the line. Over the brightest aurora peak, white
  `text.primary` over panel/popover goes from **3.87:1 / 3.36:1 (FAIL)** to
  **≈4.9 / ≈5.1:1 (AA pass)**.
- Why 0.40: the WCAG-exact crossing for `text.primary` (`F0F2F8`) is ≈0.435; 0.40
  keeps a deliberate margin (≤ the "L48" accessibility-recommended ceiling).
- **The cap is a white-text guarantee, not a tier property.** It applies to the
  DARK tiers only. The LIGHT ("Lumen") theme paints **dark** text on intentionally
  bright milky glass, so capping it would wreck *its* legibility — light glass uses
  the uncapped interior (`glass_tier_interior_raw`). Tier-ordering monotonicity is
  likewise a property of the uncapped frost ladder (over a bright backdrop the cap
  deliberately flattens the tiers to the ceiling — text wins over tier distinction).

Tokens: `GLASS_INTERIOR_LUMA_CEIL = 0.40`. Applied inside
`rae_tokens::glass_tier_interior` (the dark, capped interior); the raw ladder is
`glass_tier_interior_raw`.

### 2.4 The iridescent / chromatic edge — THE signature

This is the one thing that says "RaeenOS liquid glass" at a glance and that no flat
Acrylic/Mica has. Real liquid glass refracts light into a thin rainbow at its
border. We fake it cheaply on the SW rasterizer with a **multi-hue rim drawn
inside the glass edge**, at low alpha, over the rounded-rect stroke:

**The rim recipe (`glass.edge.iridescent`):**
- A **3px band** hugging the inside of the surface's rounded-rect border (widened
  from 2px in Round-3 so the chromatic sweep actually renders at the corners).
- Hue varies **by position along the perimeter**, cycling through three stops, all
  at the `0x40` ceiling alpha:
  - top-left / top edge → **cyan** `0x40_7CE7FF`
  - right edge → **violet** `0x40_B47CFF`
  - bottom / bottom-right → **warm amber** `0x40_FFC97C`
  - (interpolate linearly around the perimeter; the three stops at low alpha
    blend into a continuous iridescent sweep)
- Drawn **additively** (screen/linear-add) over the blurred glass so it reads as a
  light refraction, not a painted border. The rim is **felt at rest, but the
  signature must be legible at the corners — target the `0x40` band cap** (the
  in-band ceiling). Round-3 visual-QA measured the old `0x33`/2px rim as rendering
  ZERO chromatic pixels, so all three stops sit at the `0x40` ceiling and the band
  is **3px** (was 2px): present enough to actually read, still well short of a neon
  outline (the `iridescent_rim_alpha_is_subtle` KAT caps it at `0x40`).
- **On top of the rim**: the existing **1px top-edge highlight** (`stroke.strong`,
  the macOS cue, already speced in material-and-shadow.md §"Material recipe") and a
  **1px `stroke.subtle` hairline** on the remaining edges. Order, outer→inner:
  hairline → iridescent rim → 1px top highlight.

This is a per-surface draw, cheap (perimeter only, not fill), and is the visual
fingerprint. **Every glass surface gets it** — that's what makes the system cohere.

Reduced-transparency / high-contrast mode **drops the rim and the blur** and falls
back to the solid `bg.overlay` fill with a solid `stroke.strong` border (the rim is
decorative; legibility wins — see §9).

---

## 3. The signature backdrop — "Aurora Mesh" wallpaper (kills the void)

The flat void is half the problem: glass has nothing to refract. The default
RaeenOS wallpaper is a **procedural aurora mesh** — no asset file, rendered by
raeen-gfx as a `LiveWallpaper` engine (the trait already exists; today's seed is a
two-color navy `GradientWallpaper`, which is the void).

### 3.1 Concept
A slow-drifting **mesh gradient aurora**: three to four soft radial color blobs on a
deep base, drifting on independent low-frequency sine paths, blended additively so
they overlap into smooth color fields — the flowing-ribbon energy of
`reference/download (1).jpg` and the dreamy mesh of the UI Kit backdrop, but darker
and more premium so chrome and text stay legible.

### 3.2 Procedural recipe (raeen-gfx implements as `AuroraWallpaper: LiveWallpaper`)
- **Base:** deep blue-violet `0x0B_0F_1E` (slightly warmer/bluer than today's
  `0x0A0E1A`, so it's a night sky, not pure black).
- **Three drifting radial blobs**, each a soft `1/(1+d²)`-falloff radial:
  1. **RaeBlue** `0x4E_9C_FF`, large, drifts top-left↔center.
  2. **Violet** `0x9B_5CFF`, medium, drifts bottom-right.
  3. **Teal/cyan** `0x3F_C8E0`, smaller, drifts mid-left.
- **ANISOTROPY (spec pin — raegfx implements):** the blobs are NOT isotropic
  circles. The aurora is a **sheared ribbon** flowing **NW→SE**, with the falloff
  stretched ~**2.5:1** along that diagonal axis (apply an anisotropic scale to `d`
  before the radial falloff, oriented NW→SE). This gives the flowing-ribbon energy
  of `reference/download (1).jpg` rather than three round lava-lamp blobs. Where the
  **blue and violet blobs overlap** (the diagonal seam), bias the additive blend
  toward **teal** — a teal **blend zone** over the blue×violet seam — so the seam
  reads as a refracted color transition, not a muddy purple.
- Blob centers move on `sin(time * f_i + phase_i)` with tiny frequencies
  (`f ≈ 0.02–0.05 rad/s`) so the motion is barely perceptible — *alive, not busy*.
- **Additive blend** the blobs over the base, then a **subtle vignette** (×0.85 at
  corners) to keep the screen edges calm so chrome reads.
- Peak blob luminance kept modest (blobs at ~30–45% contribution) so the aurora is
  a *mood*, not a lava lamp. Text and chrome must stay legible over the brightest
  region.
- **Performance:** this is the existing per-frame `LiveWallpaper` path (frame-capped
  at 33ms, auto-paused when fully occluded). The math is 3 radials + add per pixel —
  cheap, and already gated by the occlusion-pause the compositor has.

### 3.3 Light variant ("Lumen Dawn")
Same engine, lighter palette: base `0xE8_EEF8` (cool off-white), blobs in
RaeBlue/violet/peach at low saturation — a soft sunrise mesh. This is the backdrop
behind the light "Lumen" theme.

**Tokens:** `WALLPAPER_AURORA_BASE_DARK = 0xFF0B0F1E`,
`WALLPAPER_AURORA_BASE_LIGHT = 0xFFE8EEF8`, plus the three blob hues
(`AURORA_BLOB_BLUE/VIOLET/TEAL`) reused from the accent + Vibe palette so the
backdrop re-tints with Vibe Mode automatically.

---

## 4. Accent + identity color — "RaeBlue"

RaeenOS has ONE signature hue, the way macOS owns its blue and Windows owns its
blue. Ours is **RaeBlue `0xFF_4E_9C_FF`** — a bright, slightly electric azure
(already the `rae_tokens` seed and `ThemeAbi.accent_argb` default). It is:

- the **accent** for interactive fills, selection, focus glow, links, toggles-on;
- the **primary aurora blob** in the default wallpaper (so the hue is *everywhere*,
  not just on buttons);
- the **directory/code** file-type color (already wired);
- the focus-ring base (dark) / deepened `LIGHT_FOCUS_RING` (light).

**Restraint rule (already in material-and-shadow.md, restated as identity law):**
accent is for *interaction* only. Static labels, headings, body text are NEVER
accent — they are `text.*`. A surface drowning in blue text reads as broken, not
branded. The brand comes from the *glass + aurora + one confident accent on the
controls*, not from coloring text.

### 4.1 Vibe Mode varies the accent (cohesion engine)
Vibe Mode re-seeds ONE value — `accent_argb` — and `derive_accent()` flows it to the
full six-token ramp, the aurora primary blob, the iridescent rim warm-stop, the
selection wash, the focus glow. Five shipped presets, each a single seed:

| Vibe | Seed accent | Mood |
|---|---|---|
| **RaeBlue** (default) | `0xFF_4E_9C_FF` | the signature electric azure |
| **Sunset** | `0xFF_FF_6B_5C` | warm coral |
| **Aurora** | `0xFF_3F_D0_A8` | teal-green |
| **Orchid** | `0xFF_C0_7C_FF` | violet |
| **Gold** | `0xFF_F0_B8_4C` | warm amber |

Because the accent flows to the wallpaper blob and the iridescent rim, switching
Vibe re-skins the *whole desktop coherently* in one tap — the Concept promise made
real. The other two aurora blobs (violet, teal) stay fixed so the mesh keeps depth;
only the primary blob tracks the seed.

---

## 5. Rounding, shadow, motion (identity-level summary)

These are already in `rae_tokens`; this section pins the *identity intent* so they
aren't drifted. **No changes to the corner/motion tokens** — they are correct.

### 5.1 Rounding — generous, concentric
`radius.xs 4 / sm 8 / md 12 / lg 16 / xl 24`. Identity intent: RaeenOS rounds
**generously**. Windows = `md 12`, Start/Control Center/large cards = `lg 16`, OOBE
/ full-screen modals = `xl 24`. Pills (`radius_pill = h/2`) for buttons, toggles,
search fields, chips — the reference's pill buttons are core to the look. Nested
glass uses `concentric()` so inner corners never mismatch (already implemented).

### 5.2 Shadow — long, soft, near-black (the recipe lives in material-and-shadow.md)
The `elev.*` ladder (`elev.1`–`elev.4` + `elev.focus`) is correct and the soft-shadow
renderer is fixed. Identity intent: shadows are **wide and low-alpha** (the
`elev.4` = 40px radius / `0x66` alpha ambient), never a hard offset ledge. Over the
new aurora backdrop these finally read. Focus = `elev.focus` accent glow **+** 2px
`accent.base` ring (never one alone).

### 5.3 Motion — named curves
`motion.instant/micro(90)/fast(140)/standard(220)/emphasized(320)/exit(120)`,
cubic-bezier eased. Identity intent: motion is **quick and confident, decelerating
to rest** (standard/emphasized use a strong decelerate `(0.2,0,0,1)`). Glass
surfaces open with `motion.standard`; Vibe transitions use `motion.emphasized` (the
whole desktop re-tints over 320ms). Reduced-motion → all durations 0 (already
wired); the iridescent rim and shadows are static, unaffected.

---

## 6. Type — the identity face

Type ramp is set (`design/typography-rendering.md` + `rae_tokens` §6:
display 32/600 · title 22/600 · subtitle 17/500 · body 14/400 · label 13/500 ·
caption 11/400). Identity intent: a **clean, slightly tight grotesque** —
weights 400/500/600 only (no thins, no blacks; the reference "elegant typeface"
restraint). Headings 600, body 400, controls 500. Crisp anti-aliased text over
glass is part of "polished" — the `typography-rendering.md` crisp-text work is the
gate. No new type tokens.

---

## 7. Per-surface tier application (the cohesion table — no judgment calls)

Every translucent surface in RaeenOS maps to exactly one glass tier + one elevation.
This table is **normative** — call sites read the tier, they do not choose alpha.

| Surface | Glass tier | Elevation | Radius | Notes |
|---|---|---|---|---|
| **Taskbar** | `glass.chrome` | `elev.1` | `md` (floating) / 0 (edge-docked) | most see-through; floats on aurora |
| **Title bars / window chrome** | `glass.chrome` | inherits window | `md` top corners | unifies with taskbar |
| **Start menu** | `glass.panel` | `elev.3` | `lg` | iridescent rim prominent |
| **Control Center** | `glass.panel` | `elev.3` | `lg` | the headline fix surface |
| **Settings panes / cards** | `glass.panel` | `elev.2` (cards) / `elev.3` (window) | `lg` card / `md` window | |
| **Files (window + sidebar)** | `glass.panel` | `elev.2` | `md` | sidebar slightly more translucent is OK *within* the panel tier via the §2.3 auto-adjust |
| **Context menus** | `glass.popover` | `elev.2` | `sm`–`md` | readable over busy content |
| **Flyouts / tray popovers** | `glass.popover` | `elev.2` | `md` | |
| **Notifications / toasts** | `glass.popover` | `elev.2` | `md` | |
| **Command palette** | `glass.popover` | `elev.3` | `lg` | floats high |
| **Dialogs / modals / OOBE** | `glass.panel` | `elev.4` | `xl` | widest ambient shadow |
| **Tooltips** | `glass.popover` | `elev.1` | `sm` | |
| **Couch/GameOS overlay** | `glass.panel` | `elev.3` | `lg` | larger hit targets (`HIT_TARGET_COUCH 48`) |

Three tiers. Every surface placed. **No fourth glass anywhere.**

---

## 8. Token Diff — exactly what `rae_tokens` needs (for raeen-ui)

New / changed entries in `components/rae_tokens/src/lib.rs`. Names are final; values
above. All are pure consts/structs → host-KAT'able in the existing test harness.

### 8.1 New: tiered glass (replaces the single `GLASS_TINT_*` pair)
```
// frost = per-tier WHITE luminance-add (sheen on top of the tint); see §2.0
pub struct GlassTier { pub tint: u32, pub blur_radius: u32, pub frost: u32 }

// dark (default)               tint            blur  frost (white add)
pub const GLASS_CHROME_DARK   = { 0x40_1A_22_3A, 24,  0x04_FF_FF_FF };
pub const GLASS_PANEL_DARK    = { 0x73_1C_24_3E, 20,  0x23_FF_FF_FF };
pub const GLASS_POPOVER_DARK  = { 0x99_1E_26_42, 16,  0x38_FF_FF_FF };
// light ("Lumen")
pub const GLASS_CHROME_LIGHT  = { 0x59_F4_F7_FF, 24,  0x06_FF_FF_FF };
pub const GLASS_PANEL_LIGHT   = { 0x8C_FB_FC_FF, 20,  0x18_FF_FF_FF };
pub const GLASS_POPOVER_LIGHT = { 0xB3_FF_FF_FF, 16,  0x2E_FF_FF_FF };

// the §2.0 reference frost magnitude + the shared interior-flatten helper
pub const GLASS_FROST_LIGHTEN: u32 = 0x14_FF_FF_FF;  // ≈8% white add
pub fn glass_tier_interior(tier: GlassTier, backdrop: u32) -> u32;  // backdrop→tint→frost
```
- **Keep** `GLASS_BLUR_RADIUS = 16` as the popover/default fallback; deprecate
  `GLASS_TINT_DARK/LIGHT` (leave them aliased to the panel tier for one cycle so no
  call site breaks, then remove). The contrast audit's flatten pairs switch to the
  panel tier surfaces.
- **`frost`** is a NEW field on `GlassTier` (white RGB, per-tier alpha). raegfx
  applies it as a sheen ON TOP of the tint inside `draw_glass_surface` (order in
  §2.0); the fixed per-tier step makes interior luminance monotonic across tiers.

### 8.2 New: luminance auto-adjust bounds + legibility cap
```
pub const GLASS_LUMA_HI: f32 = 0.38;  // SHIP-GATE a11y: was 0.6 (above the aurora
                                      // blob's ~0.41 luma → over-bright branch never
                                      // fired); 0.38 sits just below the blob.
pub const GLASS_LUMA_LO: f32 = 0.2;
pub const GLASS_ALPHA_BOOST: u32 = 0x18;
pub const GLASS_ALPHA_DROP:  u32 = 0x14;
pub const GLASS_INTERIOR_LUMA_CEIL: f32 = 0.40;  // §2.3/§9 dark-theme legibility cap:
                                      // composited glass interior is scaled down so
                                      // white text.primary always clears AA 4.5:1 over
                                      // a bright backdrop; no-op over dark regions.
```
- `glass_tier_interior(tier, backdrop)` = the DARK, capped interior (white-text
  guarantee). `glass_tier_interior_raw(tier, backdrop)` = the uncapped frost ladder,
  used by the LIGHT theme (dark text on bright glass) and the tier-ordering KAT.

### 8.3 New: iridescent edge (Round-3: alphas at the `0x40` ceiling, band 3px)
```
pub const GLASS_EDGE_CYAN:   u32 = 0x40_7C_E7_FF;  // top
pub const GLASS_EDGE_VIOLET: u32 = 0x40_B4_7C_FF;  // right
pub const GLASS_EDGE_WARM:   u32 = 0x40_FF_C9_7C;  // bottom (tracks Vibe accent warm-stop)
pub const GLASS_EDGE_BAND_PX: u32 = 3;
```
- All three stops sit at the in-band ceiling `0x40` and the band widened 2px→3px so
  the rim renders (Round-3 measured the old `0x33`/2px as zero chromatic pixels).
  The `iridescent_rim_alpha_is_subtle` KAT still caps the band at `[0x20, 0x40]`.

### 8.4 New: aurora wallpaper palette
```
pub const WALLPAPER_AURORA_BASE_DARK:  u32 = 0xFF_0B_0F_1E;
pub const WALLPAPER_AURORA_BASE_LIGHT: u32 = 0xFF_E8_EE_F8;
pub const AURORA_BLOB_BLUE:   u32 = 0xFF_4E_9C_FF;  // = RAEBLUE; tracks Vibe seed
pub const AURORA_BLOB_VIOLET: u32 = 0xFF_9B_5C_FF;  // fixed
pub const AURORA_BLOB_TEAL:   u32 = 0xFF_3F_C8_E0;  // fixed
```

### 8.5 New: Vibe accent presets (so Vibe Mode has named seeds)
```
pub const VIBE_RAEBLUE: u32 = 0xFF_4E_9C_FF;  // = RAEBLUE
pub const VIBE_SUNSET:  u32 = 0xFF_FF_6B_5C;
pub const VIBE_AURORA:  u32 = 0xFF_3F_D0_A8;
pub const VIBE_ORCHID:  u32 = 0xFF_C0_7C_FF;
pub const VIBE_GOLD:    u32 = 0xFF_F0_B8_4C;
```

### 8.6 Unchanged (explicitly): spacing, radius, type, motion, elevation, accent
ramp, ftype, the whole WCAG audit. The identity is achieved by the glass + backdrop
+ edge above, NOT by churning the working foundation.

### 8.7 Host-KAT additions (R10 — must be FAIL-able)
- `glass tiers are ordered`: `chrome.alpha < panel.alpha < popover.alpha` (a tier
  inversion = FAIL).
- `glass over aurora stays legible`: `text.primary` over each dark tier **flattened
  over `WALLPAPER_AURORA_BASE_DARK`** clears AA 4.5:1 (added to the contrast audit's
  shipped pairs — the audit flattens over the aurora base, the real backdrop).
- `glass over the BRIGHT aurora stays legible` (SHIP-GATE a11y — the failure class
  that motivated the luma cap): the shipped contrast pairs now also flatten
  `text.primary` over **panel AND popover composited over the aurora BLOB and the
  aurora PEAK** (base + blue + teal blobs, saturating), not just the dark base. These
  measure 2.3–3.9:1 **without** the §2.3/§9 luma cap (the real defect) and ≈4.9–5.1:1
  **with** it — so the cap is FAIL-able-proven (`legibility_luma_cap_rescues_text_over_bright_aurora`
  disables the cap and confirms the pairs flip). The strict audit
  (`contrast_audit_passes_wcag_aa_strictly`) now covers 24 pairs.
- `legibility cap is a no-op in dark regions`: over `WALLPAPER_AURORA_BASE_DARK` the
  capped interior equals the raw interior (the frosted dark-region look is preserved
  exactly); the cap bites only over bright backdrops.
- `iridescent rim alpha is subtle`: each `GLASS_EDGE_*` alpha ∈ `[0x20, 0x40]` (a
  neon rim = FAIL). Round-3 puts all three at the `0x40` ceiling — still legal.
- `tier luminance is monotonic`: flatten each tier (tint THEN frost) over a fixed
  mid backdrop and assert interior luminance chrome < panel < popover (a tier
  inversion from too-small a per-tier frost step = FAIL). Proves the §2.0 frost
  ordering term works.
- `vibe seeds are distinct`: the 5 seeds are pairwise distinct and each produces a
  valid `derive_accent` ramp.

---

## 9. Accessibility (in scope from the start — flag for raeen-accessibility)

- **Contrast over the new glass:** §8.7 extends the WCAG audit to flatten over the
  aurora base AND the brightest aurora region (blob + peak), not just `bg_base`. The
  brighter/thinner glass is the risk. **RESOLVED (SHIP-GATE a11y):** the
  raeen-accessibility audit found `text.primary` over panel/popover sitting on a
  bright aurora blob measured 3.36–3.87:1 (and worse over the peak) — the §2.3
  alpha-adjust alone could not fix it (it tweaks alpha, not the frost white-add that
  was fighting the white text), and the old `GLASS_LUMA_HI = 0.6` sat *above* the
  blob's ~0.41 luma so the over-bright branch never even fired. The fix is the **§2.3
  legibility luma cap** (`GLASS_INTERIOR_LUMA_CEIL = 0.40`) + lowering
  `GLASS_LUMA_HI` to `0.38`: glass over a bright backdrop is capped in effective
  luminance so white `text.primary` always clears AA (≈4.9–5.1:1 over the peak), while
  the dark-region frostiness is untouched. The WCAG KAT now covers the bright-aurora
  region (24 pairs) and is the permanent guard. **NOTE (route to raeen-shell-apps):**
  `text.secondary` (mid-grey) over a bright popover cannot reach 4.5:1 by design even
  with the cap (the cap targets the white-text 4.5:1 line, and darkening
  `text.secondary` further would fail it elsewhere) — surfaces that paint
  body-weight secondary text over bright glass must **promote it to `text.primary`**,
  not darken the token.
- **Reduced transparency:** a system toggle drops blur + iridescent rim, falls back
  to solid `bg.overlay` fill + solid `stroke.strong` border. The aurora wallpaper
  also gets a "reduce motion → freeze drift" and "reduce transparency → flat
  `bg.base`" path. Tokens already support the solid fallback (`bg.overlay`).
- **High contrast:** the existing `HIGH_CONTRAST` palette + `active_palette()` swap
  already overrides everything; glass/rim/aurora are all suppressed (pure black bg,
  white text, cyan focus). No new work — just ensure the new glass-draw path checks
  `high_contrast()` before applying tint/rim.
- **Focus visibility:** unchanged — `elev.focus` glow **+** 2px ring, both. The rim
  is decorative and must NOT be mistaken for or replace the focus indicator.
- **Hit targets:** unchanged — `HIT_TARGET_POINTER 32` / `HIT_TARGET_COUCH 48`.

---

## 10. Already-built delta (verify-before-spec — what exists vs. what's new)

| Capability | Where | State | This spec's delta |
|---|---|---|---|
| Live blur (3-pass box ≈ Gaussian) | `compositor::BlurEngine::box_blur_3pass` | LIVE | reused as-is; per-tier radius is just a parameter |
| Soft drop shadow | `compositor::render_drop_shadow` | LIVE (fixed per material-and-shadow.md) | unchanged; now reads against the aurora |
| Glass tint compositing | `compositor` glass path + `GLASS_TINT_*` | LIVE but **single tier, too dark** | **replace with 3 tiers + brighter/thinner + luma auto-adjust** |
| `LiveWallpaper` trait + engine swap | `compositor::set_live_wallpaper`, `GradientWallpaper`, `PlasmaWallpaper` | LIVE | **add `AuroraWallpaper` engine; make it the default seed** (today's default is the navy `GradientWallpaper(0x0A0E1A,0x1A2844)` = the void) |
| Top-edge highlight | surface-drawn `stroke.strong` | spec'd (material-and-shadow.md) | unchanged; rim layers under it |
| Iridescent edge | — | **does not exist** | **new per-surface perimeter draw** (raeen-gfx) |
| Accent ramp + Vibe seed flow | `rae_tokens::derive_accent`, `ThemeAbi.accent_argb` | LIVE | extend the seed to also drive the aurora primary blob + rim warm-stop |
| HDR tone-map | `compositor::HdrPipeline` | LIVE | unchanged |

**This is a delta, not a rebuild.** The two genuinely new draw primitives are the
**iridescent rim** and the **aurora wallpaper engine**; everything else is
re-parameterizing live code with new tokens.

---

## 11. Prioritized hand-off (implementer · order · proof screenshot)

Ordered by leverage — each step is independently shippable and visibly improves the
build. The host-render screenshot is the cheap proof (no QEMU/iron needed; renders
the surface to a buffer → PNG on the dev box, per the existing screenshot pipeline).

### Step 1 — raeen-ui: land the tokens (1 file, mechanical)
Implement §8.1–§8.5 in `rae_tokens` + §8.7 host KATs. No rendering yet.
**Proof:** `cargo test -p rae_tokens` green incl. the 4 new FAIL-able KATs.
*Unblocks everything below.*

### Step 2 — raeen-gfx: Aurora wallpaper engine (kills the void — highest visible impact)
Add `AuroraWallpaper: LiveWallpaper` (§3.2), make it the default in compositor init
(replace the `GradientWallpaper(0x0A0E1A,0x1A2844)` seed at `compositor.rs:3257`).
**Proof screenshot:** `screenshots/wallpaper-aurora-dark.png` — full desktop, no
windows: a living blue-violet-teal mesh, NOT a flat navy void. (And `-light.png`.)

### Step 3 — raeen-gfx: brighter tiered glass + luma auto-adjust
Switch the glass compositing path from the single `GLASS_TINT_*` to the three
`GlassTier`s (surface declares its tier), apply §2.3 luma adjust off the blurred
backdrop mean.
**Proof screenshot:** `screenshots/glass-tiers-over-aurora.png` — three panels
(chrome/panel/popover) over the aurora, the backdrop visibly reading through each,
increasing opacity left→right.

### Step 4 — raeen-gfx: the iridescent edge (the signature)
Add the perimeter rim draw (§2.4): hairline → iridescent rim → top highlight.
**Proof screenshot:** `screenshots/glass-iridescent-edge-3x.png` — a 3× zoom of a
panel corner showing the cyan→violet→warm rim sweep + bright top edge, low-alpha,
over the aurora. This is the "instantly recognizable" shot.

### Step 5 — raeen-shell-apps: re-skin the surfaces to the §7 table
Repoint Control Center, taskbar, Start, Files, notifications, context menus, dialogs
to their assigned tier + elevation + radius. Enforce the accent-restraint rule
(labels in `text.*`, never accent).
**Proof screenshot:** `screenshots/surface-control-center-v2.png` — the headline
before/after vs. the current `surface-control-center.png`: luminous panel glass,
aurora reading through, iridescent edge, pill toggles with accent glow, soft wide
shadow. Plus `surface-files-v2.png`, `surface-notifications-v2.png`.

### Step 6 — raeen-ui / raeen-shell-apps: Vibe Mode wired to the seed
One-tap Vibe switch re-seeds accent → ramp + aurora primary blob + rim warm-stop,
animated over `motion.emphasized`.
**Proof screenshot:** `screenshots/vibe-5up.png` — the same desktop in all five
Vibe presets, proving one seed coherently re-tints glass + aurora + controls.

### raeen-visual-qa verification list (what to screenshot + assert)
1. **No void:** default desktop is the living aurora mesh, not flat navy.
2. **Glass reads as glass:** backdrop visibly refracts through each tier; sample a
   line across a panel edge — backdrop color bleeds through (alpha < opaque).
3. **Iridescent edge present** on every glass surface; rim alpha subtle (not neon),
   hue sweeps cyan→violet→warm around the perimeter.
4. **Tier discipline:** chrome more see-through than panel more than popover —
   measurable in the composited alpha; no surface off-tier.
5. **Legibility preserved:** text over the brightest aurora region still clears AA
   (hand to raeen-accessibility for the measured ratio).
6. **Vibe coherence:** switching Vibe moves the accent AND the aurora primary blob
   AND the rim warm-stop together — not just the buttons.
7. **Reduced-transparency / high-contrast:** rim + blur + aurora drift all suppress
   correctly; focus ring + glow intact.

### Unblocks (MasterChecklist)
- **Phase 8 (RaeUI/RaeKit):** the `material.glass` system is only "done" when it's
  tiered + luminous + iridescent — this is that definition.
- **Phase 13 (Customization / Vibe Mode):** Vibe's "one tap re-skins the whole
  desktop" needs the seed→aurora→rim flow (Step 6).
- **Phase 14 (RaeShell + apps):** every surface spec (`control-center`, `files`,
  `notifications`, `taskbar`, `start`) inherits this identity; they were each
  "fine but didn't belong to one system" — the three tiers + aurora + rim are what
  make them one system.
```
