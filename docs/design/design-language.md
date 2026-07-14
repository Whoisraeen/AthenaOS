# AthenaOS Design Language

> *"Built for people who care about how things feel."* — LEGACY_GAMING_CONCEPT.md
>
> This is the canonical token set. Every per-surface spec in `docs/design/`
> references this file by token name (e.g. `radius.lg`, `elev.2`,
> `motion.standard`). When a surface needs a value that is not here, propose it
> here first, then reference it — never inline a new magic number. Incoherence
> across surfaces is the single biggest UI risk; this file is the defense.

The bar: **the familiarity of Windows 11, the cleanness of macOS, the cool
factor of Linux** — without copying any of them. AthenaOS's distinct synthesis is
*one accent-driven material system* that the user owns end-to-end (Vibe Mode),
rendered by a compositor that already does the expensive parts (blur, shadow,
HDR tone-map) in `kernel/src/compositor.rs`.

---

## 0. Verify-before-spec: what already exists

Grounded in the actual code, not assumptions:

| Capability | Where | State |
|---|---|---|
| Drop shadow (quadratic falloff, configurable offset/radius/color) | `compositor::render_drop_shadow` | LIVE, per-surface `SurfaceEffect::DropShadow` |
| Glassmorphism blur (3-pass box blur ≈ Gaussian + tint) | `compositor::BlurEngine::box_blur_3pass`, `set_surface_blur` | LIVE, per-surface `BlurRegion { radius, tint_color }` |
| HDR tone-map (sRGB→linear→ACES/Reinhard→sRGB) | `compositor::HdrPipeline` | LIVE (pipeline); GPU scanout pending |
| Canvas primitives: rounded-rect, gradient, circle, alpha blend, scaled AA text | `athgfx::Canvas` | LIVE (host-KAT'd 5/5) |
| Theme token carrier (`accent_argb`, bg/fg/text, `blur_radius`, `corner_radius`, font, cursor) | `kernel/src/theme_engine.rs::ThemeAbi` | LIVE, 8 signed builtins |
| Vibe Mode (theme+wallpaper+sound+RGB switched as a set) | `kernel/src/vibe_mode.rs` | LIVE, 5 presets |
| Window chrome titlebar (28px flat bar, block-font, min/max/close) | `kernel/src/window_chrome.rs` | LIVE but **pre-polish** |
| Taskbar (40px flat bar, top accent line, window buttons) | `components/athshell/src/desktop.rs` | LIVE but **pre-polish** |

**The cohesion problem to fix:** ~30 files each redefine `const ACCENT: u32 =
0xFF_4E_9C_FF` and a private palette (`calculator.rs`, `capture.rs`,
`desktop.rs`, …). Tokens below are the single source; `ThemeAbi` is their
runtime carrier so the accent the user picks flows everywhere.

---

## 1. Prior art distilled (current systems, 2024–2025)

- **macOS Sequoia / Tahoe (Liquid Glass, 2025):** continuous-curvature
  ("squircle") corners; concentric radii (a control inside a panel uses
  `outer − padding`); heavy real-time backdrop blur with a bright top edge
  highlight and a faint inner shadow; system accent tints controls but chrome
  stays neutral; spring-based motion (no fixed duration — mass/stiffness).
  **Take:** concentric radii, the top-edge highlight on glass, restraint in
  chrome color. **Avoid:** Liquid Glass's legibility regressions over busy
  wallpapers (we always keep a tint floor); spring math is overkill for a
  software compositor — we use tuned cubic-bezier curves.
- **Windows 11 (Fluent / Mica / Acrylic):** 4px grid; corner radius 8 (windows/
  flyouts), 4 (buttons/controls); **Mica** = cheap, wallpaper-tinted, *static*
  desktop-level material for window backgrounds; **Acrylic** = live blur for
  transient surfaces (flyouts, menus) — blur ≈ 30px + 80% tint + 2% noise;
  reveal/hover highlight; accent drives Start, taskbar, focus. **Take:** the
  Mica-vs-Acrylic split (static material for big always-on surfaces, live blur
  only for transient pop-overs — this is a *performance* doctrine that suits a
  software compositor). 4px grid. **Avoid:** Fluent's inconsistency (three
  control styles across eras); noise texture (skip until GPU scanout).
- **GNOME 46+ (libadwaita):** flat, high-contrast, minimal blur; strong focus
  rings; generous spacing; system font (Cantarell/Inter-like); CSS-token
  theming. **Take:** the discipline of a strict spacing scale and a *visible*
  focus ring (we owe this for a11y + controller). **Avoid:** the flatness —
  AthenaOS is explicitly glass-and-depth.
- **SteamOS (gamescope/Big Picture):** large hit targets (≥48px), high-contrast
  focus glow that reads from across a room, controller-first nav with a single
  always-visible focus cursor, reduced reliance on hover. **Take:** the focus
  *glow* and the ≥48px target floor for the couch/controller mode; the idea
  that focus must be legible at 3 meters. **Avoid:** abandoning the desktop
  density — couch mode is a *mode*, not the default.

**AthenaOS synthesis:** Windows-familiar *layout* (bottom taskbar, Start at left,
tray at right) + macOS-clean *materiality* (continuous radii, glass with a top
highlight, neutral chrome) + Linux-grade *ownership* (every token above is a
theme variable the user re-skins via Vibe Mode), all on a compositor that
already blurs and shadows for free.

---

## 2. Spacing & grid

**Base unit = 4px** (Windows-grid familiarity; divides evenly at integer
software-raster scale). All spacing, padding, and gaps are multiples.

| Token | px | Use |
|---|---|---|
| `space.0` | 0 | flush |
| `space.1` | 4 | icon-to-label, tight inset |
| `space.2` | 8 | default control padding, intra-group gap |
| `space.3` | 12 | control vertical padding, list-row inset |
| `space.4` | 16 | panel padding, inter-group gap |
| `space.5` | 24 | section gap |
| `space.6` | 32 | window content margin |
| `space.8` | 48 | large couch-mode gap |

**Hit-target floor:** desktop/mouse = **32px**; touch/couch/controller =
**48px** (SteamOS bar). Flag to athena-accessibility if any interactive element
ships below `32px` in pointer mode.

---

## 3. Corner-radius scale

Continuous-feel rounding via `athgfx::Canvas` rounded-rect. **Concentric rule:**
a child's radius = `parent.radius − parent.padding` (clamped ≥ `radius.xs`), so
nested glass never shows mismatched corners (the macOS lesson).

| Token | px | Use |
|---|---|---|
| `radius.xs` | 4 | buttons, chips, tray icons, menu rows |
| `radius.sm` | 8 | controls, search field, toasts |
| `radius.md` | 12 | window corners, flyouts, cards |
| `radius.lg` | 16 | Start menu, quick-settings panel, large cards |
| `radius.xl` | 24 | OOBE / full-screen modal cards |
| `radius.pill` | h/2 | pill buttons, the Start pill, segmented toggles |

`ThemeAbi.corner_radius` carries the *window* radius (`radius.md` default);
themes may override (Bauhaus = 0, Holographic = 16).

---

## 4. Color system

All colors are ARGB `0xAARRGGBB` (compositor-native). Two base palettes
(dark default, light), plus an accent ramp derived from a single seed.

### 4.1 Dark palette (default)

| Token | ARGB | Role |
|---|---|---|
| `bg.base` | `0xFF_0A_0E_1A` | desktop void / deepest layer (matches `compositor` clear + `login_ui` gradient top) |
| `bg.raised` | `0xFF_12_16_24` | window client, panels |
| `bg.overlay` | `0xFF_1A_1E_2E` | menus, flyouts (pre-glass solid fallback) |
| `bg.elevated` | `0xFF_22_27_38` | hovered rows, selected list items |
| `stroke.subtle` | `0x33_FF_FF_FF` | 20% white hairline dividers |
| `stroke.strong` | `0x55_FF_FF_FF` | glass top-edge highlight |
| `text.primary` | `0xFF_F0_F2_F8` | headings, active labels |
| `text.secondary` | `0xFF_AE_B4_C6` | body, inactive labels |
| `text.tertiary` | `0xFF_6E_76_8C` | hints, disabled, timestamps |
| `state.danger` | `0xFF_E5_4B_4B` | close hover, destructive |
| `state.warn` | `0xFF_E8_B5_4B` | warnings |
| `state.ok` | `0xFF_3FBF_7F` → `0xFF_3F_BF_7F` | success, link-up |
| `scrim.modal` | `0x1F_00_00_00` | ~12% dim behind a transient modal (command palette) so glass reads over busy wallpaper |
| `scrim.capture` | `0x99_06_08_10` | ~60% near-black dim over the whole screen during region capture; the selected region is punched back to full brightness (`capture` spec). Palette-neutral — same value in light mode (it darkens the screenshot target, not chrome). |

### 4.2 Light palette

| Token | ARGB | Role |
|---|---|---|
| `bg.base` | `0xFF_EC_EF_F5` | desktop void |
| `bg.raised` | `0xFF_F7_F9_FC` | window client, panels |
| `bg.overlay` | `0xFF_FF_FF_FF` | menus/flyouts solid fallback |
| `bg.elevated` | `0xFF_E2_E7_F0` | hover/selection |
| `stroke.subtle` | `0x1A_00_00_00` | 10% black dividers |
| `stroke.strong` | `0x33_FF_FF_FF` | glass top highlight (still white) |
| `text.primary` | `0xFF_14_18_22` | |
| `text.secondary` | `0xFF_45_4C_5E` | |
| `text.tertiary` | `0xFF_8A_90_A0` | |

**Dark/light parity rule:** every surface spec lists both. Contrast target
(WCAG AA): `text.primary` ≥ 7:1 on its bg, `text.secondary` ≥ 4.5:1,
`text.tertiary` ≥ 3:1 (large/non-essential only). athena-accessibility verifies.

### 4.3 Accent model (the cohesion engine)

A theme defines **one seed accent** (`ThemeAbi.accent_argb`, default
`0xFF_4E_9C_FF` "RaeBlue"). The rest is derived deterministically so a re-skin
is a single value change:

| Derived token | Rule | Default (RaeBlue) |
|---|---|---|
| `accent.base` | seed | `0xFF_4E_9C_FF` |
| `accent.hover` | lighten seed ~18% (toward white) | `0xFF_6E_AE_FF` |
| `accent.active` | value-darken seed (~×0.897 − 12/ch) | `0xFF_3A_80_DB` |
| `accent.subtle` | seed @ 24% alpha over bg | `0x3D_4E_9C_FF` |
| `accent.text` | seed if contrast≥4.5:1 on bg else `text.primary` | `0xFF_4E_9C_FF` (RaeBlue clears ~7:1 on dark → seed) |
| `accent.glow` | seed @ 40% alpha, used as shadow color for focus | `0x66_4E_9C_FF` |

### 4.4 File-type semantics (fixed — NOT accent-derived)

A small fixed palette so a file *type* reads consistently across every Vibe
preset (a directory must look like a directory in any theme). Reusable by any app
that shows a file chip; lives in `ath_tokens` alongside the palettes. Two
exceptions track the accent on purpose (`dir`/`code` read as "primary"). See
`files.md` §4 for the mapping from the legacy `FM_*` colors.

| Token | ARGB | Role |
|---|---|---|
| `ftype.dir` | = `accent.base` | directories (the one type that tracks accent) |
| `ftype.code` | = `accent.base` | source code |
| `ftype.exec` | `0xFF_3F_BF_7F` (= `state.ok`) | executables |
| `ftype.media` | `0xFF_C0_7C_FF` | image / video / audio (collapsed from the old 4-hue rainbow — premium restraint) |
| `ftype.doc` | `0xFF_F0_C8_5C` | documents / pdf |
| `ftype.archive` | `0xFF_F0_A0_3C` | archives |
| `ftype.neutral` | = `text.secondary` | plain / unknown / device / socket / pipe |

athena-accessibility verifies each clears 3:1 on `bg.raised`.

Derivation lives once in `ath_tokens::derive_accent(seed, palette) -> AccentRamp`
(per ADR 0003; `theme_engine::derive_accent` delegates to it) and feeds every
surface. The `Default (RaeBlue)` column is the authoritative contract — the
host KATs in `ath_tokens` assert `base`/`subtle`/`glow` exactly and
`hover`/`active` within ±2/channel; the rule prose is the intent, the values win
on any disagreement. **No surface hardcodes an accent.** This is what makes Vibe
Mode's "the desktop becomes a different place in one tap" real rather than a
wallpaper swap.

---

## 5. Material / glass recipe

Two materials, mirroring the Windows Mica/Acrylic split — chosen here as a
*performance* doctrine for the software compositor (live blur is per-frame box
blur; we limit it to small transient surfaces).

### 5.1 `material.glass` — live (transient surfaces only)

Maps to `compositor::set_surface_blur(BlurRegion { radius, tint_color })`.

| Property | Value | Maps to |
|---|---|---|
| blur radius | **16px** (`ThemeAbi.blur_radius` default) | `BlurRegion.radius` |
| tint (dark) | `bg.overlay` @ ~62% → `0x9E_1A_1E_2E` | `BlurRegion.tint_color` (ARGB; alpha = tint strength) |
| tint (light) | white @ ~70% → `0xB3_FF_FF_FF` | |
| top-edge highlight | 1px `stroke.strong` along the top edge | drawn by surface (Canvas line) |
| border | 1px `stroke.subtle` on remaining edges | drawn by surface |
| corner | `radius.lg` for panels, `radius.md` for flyouts | Canvas rounded-rect mask |

**Use for:** Start menu, quick-settings panel, flyouts, context menus, toasts,
Alt-Tab switcher, OOBE card. These are small and short-lived — live blur cost is
bounded.

### 5.2 `material.mica` — static (large always-on surfaces)

A cheap, wallpaper-derived **solid** tint (no per-frame blur). Sample the
wallpaper's average color once per wallpaper change, desaturate ~50%, darken to
~`bg.base` luminance. **Use for:** taskbar background, window titlebars,
maximized window client backdrops. This keeps the always-on chrome off the
per-frame blur path (compositor latency doctrine, per memory
`iron-console-logging-tax` / `compositor-IF=0` concerns).

### 5.3 Elevation / shadow ladder

Maps to `compositor::SurfaceEffect::DropShadow { offset_x, offset_y, radius,
color }`. Shadow color is always near-black with alpha; for *focused/active* glow
it becomes `accent.glow`.

> **RENDERING CONTRACT (do not skip):** the shadow must be a **soft blurred
> silhouette** — rounded alpha mask filled with the constant shadow `color`,
> blurred by `radius` via the existing 3-pass box blur, offset, composited under
> the surface. It is NOT an analytic per-pixel falloff and it NEVER samples the
> backdrop. athena-visual-qa found the current renderer produces a *hard blue
> offset block* — the #1 "looks basic" defect. The full algorithm + acceptance
> (the penumbra test) live in [`material-and-shadow.md`](./material-and-shadow.md).
> Every surface that references `elev.*` below inherits that fix.

| Token | offset_y | radius | color (dark) | Use |
|---|---|---|---|---|
| `elev.0` | 0 | 0 | none | flush to desktop |
| `elev.1` | 1 | 6 | `0x30_00_00_00` | taskbar, resting cards |
| `elev.2` | 3 | 14 | `0x40_00_00_00` | flyouts, toasts, menus |
| `elev.3` | 8 | 28 | `0x55_00_00_00` | Start menu, quick-settings, modals |
| `elev.4` | 12 | 40 | `0x66_00_00_00` | dragged window, OOBE card |
| `elev.focus` | 0 | 10 | `accent.glow` | focused control / couch focus ring (additive glow, not displacement) |

Light mode multiplies shadow alpha by ~0.6 (shadows read heavier on light bg).

---

## 6. Typography

**Direction:** a single neutral humanist-grotesque UI face — "RaeSans" — in the
Inter / SF Pro / Segoe UI family (open, large x-height, legible at small px on a
software raster). `ThemeAbi.font_family` defaults to `"Inter"`; the `athfont`
follow-up (per memory `ui-glass-design-system`) supplies crisp scalable text via
`Canvas::draw_text` (scaled AA already exists). Monospace companion = "RaeMono"
(JetBrains Mono family) for Terminal / code.

**Type ramp** (px / weight / line-height). Until `athfont` lands, the block/8px
glyph path approximates these at the nearest integer scale.

| Token | px | weight | line-height | Use |
|---|---|---|---|---|
| `type.display` | 32 | 600 | 40 | OOBE, lock-screen clock |
| `type.title` | 22 | 600 | 28 | window/section titles, Start header |
| `type.subtitle` | 17 | 500 | 24 | flyout headers, settings group |
| `type.body` | 14 | 400 | 20 | default UI text |
| `type.label` | 13 | 500 | 16 | buttons, tabs, taskbar labels |
| `type.caption` | 11 | 400 | 14 | timestamps, hints, tray |

**Weights shipped:** 400 / 500 / 600. No italics in chrome. Letter-spacing 0
except `type.caption` (+0.2px) and all-caps labels (+0.5px, used sparingly).

---

## 7. Motion system

Durations + cubic-bezier easing (the compositor is frame-driven; spring math is
out of scope — these are the tuned curves). All motion respects
**reduced-motion** (see §8): when set, durations collapse to `0ms` (instant) and
only opacity cross-fades remain.

| Token | duration | easing (cubic-bezier) | Use |
|---|---|---|---|
| `motion.instant` | 0ms | — | reduced-motion fallback |
| `motion.micro` | 90ms | `0.4, 0.0, 0.2, 1` (standard-out) | hover/press state, focus ring appear |
| `motion.fast` | 140ms | `0.3, 0.0, 0.1, 1` | toast in, tray flyout, button press |
| `motion.standard` | 220ms | `0.2, 0.0, 0.0, 1` (decelerate) | Start/quick-settings open, window open |
| `motion.emphasized` | 320ms | `0.2, 0.0, 0.0, 1` then settle | maximize/restore, Vibe Mode transition |
| `motion.exit` | 120ms | `0.4, 0.0, 1, 1` (accelerate-in) | dismiss/close (faster than entry) |

**Principles:** entrances decelerate (ease-out), exits accelerate (ease-in),
exits are ~40% faster than their entrance. Transient surfaces (`material.glass`)
scale from 96%→100% + fade 0→1 on `motion.standard`. Window open = fade + 8px
upward translate. `ThemeAbi`'s animation-curve field selects the family (Vibe
Mode swaps these — e.g. Cyberpunk = snappier, Ghibli = softer/longer).

---

## 8. Accessibility (in scope from the start)

| Concern | Token / rule | Owner verify |
|---|---|---|
| Contrast | §4.2 ratios (AA: 7:1 / 4.5:1 / 3:1) | athena-accessibility |
| Focus visibility | `elev.focus` accent glow + 2px `accent.base` ring on every focusable element; never *only* a color change | athena-accessibility |
| Reduced motion | `motion.instant` collapse; surface specs MUST define the reduced path | athena-accessibility |
| Hit targets | 32px pointer floor / 48px couch floor (§2) | athena-visual-qa |
| Hover-independence | every hover affordance has a keyboard/controller focus equivalent (GNOME/SteamOS lesson) | athena-accessibility |

Anything a surface cannot satisfy here is **flagged to athena-accessibility**, not
silently shipped.

---

## 9. Token → runtime carrier map

So implementers know where a token *lives* at runtime:

- Accent + window radius + blur radius + font + cursor → `theme_engine::ThemeAbi`
  (signed bundle; the user's pick).
- Per-surface shadow → `compositor::SurfaceEffect::DropShadow` (use `elev.*`).
- Per-surface live blur/tint → `compositor::set_surface_blur` (use
  `material.glass`).
- Static taskbar/titlebar tint → `material.mica` (computed once per wallpaper).
- Spacing/radius/type/motion constants → proposed shared crate
  `athui::tokens` (so apps stop redefining `const ACCENT`). **This consolidation
  is itself a polish item** (see surface specs).

---

## Handoff

- **athena-ui (framework):** own `athui::tokens` — the shared constant module
  that ends the per-app palette duplication; expose `derive_accent(seed)`.
- **athena-gfx:** confirm `Canvas` rounded-rect supports the `radius.*` scale and
  per-corner masking; confirm shadow color can be accent-tinted for
  `elev.focus`.
- **theme_engine (kernel):** add `derive_accent` so all six accent-derived
  tokens come from one seed.
- **Visual-QA evidence:** a screenshot of any two surfaces (e.g. taskbar +
  Start menu) showing *identical* accent, radii, and shadow treatment — proof of
  cohesion. Boot-log: theme/vibe smoketest already prints applied accent; extend
  to assert all surfaces read the same `accent.base`.

This document is the source of truth. Per-surface specs reference these token
names verbatim.
