# RaeenOS Visual Identity v2 — "Obsidian" (2026-07-01)

> Supersedes the §2 "Liquid Glass frost" material recipe in `IDENTITY.md`.
> Everything else in IDENTITY.md (type ramp, icon register, accent discipline,
> a11y guardrails, aurora wallpaper §3) remains normative. Owner directive
> 2026-07-01: the frost-glass look "reads toy OS / bad Linux clone"; real
> Liquid Glass needs GPU refraction we do not have. Own what a software
> rasterizer renders at FULL quality instead of imitating what it can't.

## 1. The diagnosis (why frost failed)

The frost recipe (slate tint + white sheen) lands every panel at interior
luma L77–112 — **mid-gray**. Mid-gray panels + monochrome line icons +
rainbow edge rims = the GTK-dark-theme register. The two dark references the
owner picked (ShadowMist Win11, macOS dark Finder) are **near-black (L≤25)**
with polish from *depth and color*, not surface brightness:

| Signal | ShadowMist / macOS dark | Our frost (retired) |
|---|---|---|
| Panel luma | L 12–25 near-black | L 77–112 gray |
| Translucency | a whisper (90–96% opaque) | milky (45–62%) |
| Edge | 1px light hairline + soft shadow | 3px iridescent rainbow rim |
| Icons | big, filled, COLORFUL | thin monochrome strokes |
| Accent | sparse, GLOWING on black | washes and fills everywhere |

## 2. The Obsidian material (normative recipe)

One material, three tiers — same tier names/roles as before so call sites
don't change:

```
surface = backdrop                       (aurora or app content)
        → obsidian tint (per tier)      near-black, HIGH alpha
        → chroma bleed                   +6% saturation lift of what survives
        → 1px outer hairline             0x26_FFFFFF
        → 1px top-inner highlight        0x14_FFFFFF (the lit top lip)
        → soft drop shadow (per elev)    neutral black, y-offset, wide blur
```

Tier constants (rae_tokens `GLASS_*_DARK`, values re-baked):

| tier | tint (ARGB) | reads as |
|---|---|---|
| chrome (taskbar) | `0xE4_0C0E12` | deepest, most wallpaper shows |
| panel (CC, sidebars) | `0xF0_101318` | workhorse near-black |
| popover (Start, menus) | `0xF6_14171D` | most opaque, floats highest |

* **Frost is dead.** Tier `frost` alphas drop to ≤`0x06` (a breath, not milk).
* **The iridescent rim is retired** on all surfaces. The old §2.4 called it
  "the signature"; it read as a theme-mod rainbow border. The new signature
  is **obsidian depth**: near-black + hairline + glow + aurora bleed.
* Interior hierarchy comes from **elevation steps of solid dark** — cards and
  tiles inside a panel use `bg.raised`/`bg.elevated` fills (L16→L22→L28),
  never a white wash.
* White `text.primary` on L≤25 clears WCAG AAA everywhere — the entire
  luma-cap/WCAG-clamp machinery becomes a safety net, not a load-bearing fix.

## 3. Accent: sparse + glowing

RaeBlue stays the only accent. Two new rules:
1. **Glow, don't wash.** The focused/active element gets an additive accent
   halo (`glow.accent` = accent @ 0x2E, blur 14) behind its pill — the
   "lit on black" read from ShadowMist. Hover states use elevation steps,
   not accent washes.
2. Dark-on-accent ink rule unchanged.

## 4. Color where it counts

Monochrome line icons remain the CHROME register (tray, toolbars, chevrons).
CONTENT icons (folders, file types, app tiles) get the §4.4 ftype palette as
**fills, not strokes** — a folder is a solid blue folder (macOS register),
media is violet, archives amber. `raegfx::icon` gains filled primitives;
line variants stay for chrome.

## 5. Acceptance (visual-QA measures against THIS)

- Panel interior luma L ≤ 30 over the bright aurora core (was 77–112).
- Zero rainbow-rim pixels on any surface; hairline + top-light only.
- Focused element shows a measurable accent halo (≥2% accent-hued px in the
  8px ring around the active pill).
- Folder icon in Files/Start renders ≥60% filled blue pixels at 24px.
- White text ≥ 7:1 (AAA) on every tier interior.
- The marquee (desktop + Start open + Files window) reads NEAR-BLACK with
  visible wallpaper color at panel edges — side-by-side against the
  ShadowMist reference, same luminance register.
