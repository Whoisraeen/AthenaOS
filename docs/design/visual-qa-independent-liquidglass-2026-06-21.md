# Visual QA — INDEPENDENT acceptance pass — Liquid Glass identity — 2026-06-21

> This is the **independent** acceptance pass required by Goal #1 ("UI visually
> stunning, proven by raeen-visual-qa screenshots judged against current macOS /
> Windows 11 references"). It is deliberately a DISTINCT document from the concurrent
> session's `visual-qa-round1..6` docs — I did not read their verdicts before judging.
> I re-rendered the current pixels myself (`tools/ui_screenshot` → exit 0, 15 PNGs at
> 2026-06-21 12:45) and inspected each at full resolution (full-res crops via a
> throwaway raemedia decode/crop tool, outside the repo). Harsh, specific, pixel-level.

- **Booted to:** N/A — host-render harness (the live desktop is unreachable in
  headless CI; this is the sanctioned host-render proof path, ADR 0004). Atoms +
  surfaces rendered on the SAME `raegfx::Canvas` software rasterizer the kernel
  composites with.
- **Screenshots judged (all `docs/design/screenshots/`, regenerated this session):**
  `wallpaper-aurora-dark.png`, `glass-tiers-over-aurora.png`,
  `glass-iridescent-edge-3x.png`, `atom-glass-panel.png`, `atom-drop-shadow.png`,
  `atom-type-ramp.png`, `atom-icons.png`, `atom-focus-ring.png`,
  `atom-primitives.png`, `surface-control-center.png`, `surface-files.png`,
  `surface-notifications.png`, `surface-start-menu.png`, `surface-taskbar.png`,
  `surface-settings.png`.
- **Against spec:** `IDENTITY.md` (the Liquid Glass identity), `design-language.md`,
  `material-and-shadow.md`, `control-center.md`, `files.md`, `notifications.md`,
  `taskbar-running-apps.md`, `typography-rendering.md`.
- **References:** current macOS 26 "Tahoe" Liquid Glass Control Center
  (translucent, light-reflecting, customizable; tinted-mode option in 26.1);
  Windows 11 24H2 Mica/Acrylic; plus the in-repo reference set
  (`reference/Liquid Glass UI Kit`, `reference/download (1).jpg`).

---

## Headline verdict

**The identity has LANDED.** Six rounds of concurrent work have moved RaeenOS from
the "flat dark card over a void" defect (IDENTITY.md §0) to a genuinely coherent,
recognizable Liquid Glass look. The three systemic moves the spec demanded are all
visibly present in the pixels: (1) brighter **tiered** glass, (2) a **living aurora
backdrop** the glass refracts, (3) the **iridescent chromatic rim**. The Control
Center, the icon system, the shadow renderer, the type ramp, and the focus ring are
**world-class or near it** — Control Center genuinely competes with macOS Tahoe.

But it is **not uniformly finished.** Two shipped surfaces — the **Start menu** and
the **Taskbar** — are still rendering app labels in the **monospace (RaeMono)** font
with placeholder single-letter icons, which instantly drops them to "basic/unfinished"
and breaks the cohesion the whole identity is built on. The Files toolbar buttons are
flat gray rects (the spec's own §0 "Buttons FAIL"). These are the difference between
"stunning system" and "stunning demo screens + a few unfinished ones."

### Overall: **78% toward world-class.**

Up sharply from the ~35% the spec recorded in Round 3. The remaining 22% is NOT
foundational — the materials, shadows, tokens, and the flagship surface are done.
It is **application discipline**: the same finished glass + RaeSans + real icons
applied to the 2 unfinished surfaces, plus glossy pill buttons. That is days, not
weeks.

**Does it beat macOS Tahoe + Win11?**
- **vs Windows 11 24H2:** YES, already, on the finished surfaces. Win11's Mica is
  flatter, more opaque, and has no iridescent signature; RaeenOS Control Center and
  notifications read more premium. Win11 still wins on *breadth* (every surface is
  finished) — RaeenOS's start/taskbar are behind any shipped Win11 surface.
- **vs macOS Tahoe Liquid Glass:** NOT YET, but close on the flagship. macOS still
  wins on (a) luminous *light* glass that reflects the backdrop with specular life,
  (b) glossy pill controls with inner glow, (c) total finish across every surface,
  (d) SF Display weight contrast. RaeenOS's dark glass is excellent but the rim is
  subtler and the controls are flatter than Tahoe's. Call it **~85% of Tahoe on
  Control Center, ~50% system-wide** because of the unfinished surfaces.

---

## Per-surface scores

| Surface | Score | One-line verdict |
|---|---:|---|
| Aurora wallpaper | **9/10** | Living blue→violet→cyan mesh, real depth, NOT a void. Spec's #1 fix nailed. |
| Glass tiers (3) | **8.5/10** | Chrome/panel/popover visibly distinct, monotonic, aurora reads through all three. |
| Iridescent rim | **7/10** | Present and legible (violet right / amber bottom), but subtle + pixelated at corners. |
| Soft shadow | **9/10** | Feathered ambient penumbra, clearly beats the rejected hard-offset block. macOS-grade. |
| Icon system (36) | **9/10** | Uniform 2px line set, scale-clean at 72px, tintable. A real differentiator. |
| Type ramp | **8/10** | Crisp AA RaeSans + JetBrains Mono, clean hierarchy; weak weight contrast vs SF. |
| Focus ring | **9/10** | Accent glow ring + cyan HC forced-colors variant. Accessibility win. |
| Primitives | **8.5/10** | Radius ramp, smooth gradients (no banding), AA circles all clean. |
| **Control Center** | **8.5/10** | Flagship. Tile grid, pill toggles w/ glow, sliders, media card, gaming section, rim. Competes with Tahoe. |
| **Files** | **6.5/10** | Crisp text, colored file-type icons, clean selection — but flat gray toolbar buttons, no glass, no toolbar icons. |
| **Notifications** | **7.5/10** | Clean popover toast stack, semantic title colors, rim — but action buttons (Reply/More) near-illegible low contrast. |
| **Start menu** | **4/10** | Monospace tile labels, tiny smudge icons, flat slab with no aurora bleed. Reads unfinished. |
| **Taskbar** | **4.5/10** | Monospace labels, placeholder letter-icons, chrome too opaque. Running-pills + red dot are good. |
| Settings | **n/a** | Rendered but not deep-inspected this pass — flag for next round. |

---

## Findings (each actionable)

### Blocking-the-"stunning"-claim (high priority)

1. **Start menu — app tile labels render in MONOSPACE (RaeMono), not RaeSans.**
   "Files / Browser / Terminal / Settings / Editor / RaeGames" all read as console
   text. Tile icons are tiny blue smudges, not the crisp 36-icon set. The panel is a
   flat uniform slab with **no aurora bleed-through** (unlike Control Center).
   *Should be:* RaeSans 500 labels (typography-rendering.md), real tinted icons from
   the icon sheet, panel tier = `glass.panel` over the aurora so it reads as glass.
   **Owner: raeshell** (label font + icon wiring), **raegfx** (panel must composite
   over the live backdrop, not a flat fill).

2. **Taskbar — same monospace-label + placeholder-letter-icon problem.**
   "RaeOS / RaeBrowser / Terminal / Messages / 12:00" in RaeMono; app glyphs are
   single letters (R/F/W/T/M). The chrome glass is also too dark/opaque — it should
   be the **25% chrome tier** that "floats on the wallpaper" (IDENTITY.md §2.1), but
   it reads ~60% opaque.
   *Should be:* RaeSans labels, real app icons, chrome at 25% effective alpha.
   The active-app pill (bright RaeBlue) and the red unread dot on Messages are good —
   keep those. **Owner: raeshell** (font + icons), **raegfx** (chrome tier alpha).

3. **Files toolbar buttons are flat gray rectangles — the spec's own §0 "Buttons
   FAIL."** "Up / New Folder / Rename / Trash" are bare text on flat fills, no pill
   radius, no glass, no toolbar icons. They are the single most "basic" element on an
   otherwise strong surface, and they clash with the pill toggles in Control Center.
   *Should be:* pill controls (`radius_pill`, IDENTITY.md §"radii"), subtle glass
   fill, leading icons from the 36-icon set (macOS Finder / Win11 Explorer both lead
   toolbar actions with icons). **Owner: raeui** (button component), **raeshell/files**.

### Medium (polish gaps to reach Tahoe)

4. **Notification action buttons (Reply / More / Install) are near-illegible.** Very
   low-contrast pills over the popover glass — fail WCAG at a glance. *Should be:*
   accent-tinted or `stroke.strong`-bordered pills with legible label contrast.
   **Owner: raeui** — coordinate contrast target with **raeen-accessibility**.

5. **Iridescent rim is pixelated/steppy at the corners** (visible in
   `glass-iridescent-edge-3x.png`: the violet→amber sweep jaggies on the corner
   arc). It reads as aliasing rather than smooth refraction. *Should be:* AA the rim
   band along the rounded-rect path (sub-pixel coverage), not a stair-stepped stroke.
   **Owner: raegfx.**

6. **Controls are flat-filled, not glossy.** The reference UI Kit's defining button
   look is a **pill with a colored inner glow + glossy top sheen**
   (`reference/Liquid Glass UI Kit`). RaeenOS toggles are pills with accent glow
   (good) but buttons/tiles are flat. This is the biggest single gap vs both Tahoe
   and the reference kit. **Owner: raeui** (a glossy-pill control recipe), spec
   already calls for it — **owner: raeen-design-researcher** to lock the inner-glow
   token if missing.

7. **`atom-glass-panel.png` reads muddy/dark over a LIGHT backdrop.** Over the light
   blue scene the panel goes dark-navy instead of luminous-frosted. This is the
   dark-tier glass behaving as designed, but it exposes that the **light "Lumen"
   theme** (IDENTITY.md §2.2) is not what the atom is showing — the milky-light glass
   that matches `reference/download (1).jpg` is not yet demonstrated in any
   screenshot. *Action:* render a Lumen (light-theme) capture to prove §2.2 exists.
   **Owner: raeshell/ui** (light-theme surface render).

### Low / type

8. **Type weight contrast is thin.** Display vs Title differ mostly by *size*; macOS
   SF gets premium feel from heavier Display weights. *Should be:* consider a heavier
   Display weight per the §"Headings 600" rule applied with more separation.
   **Owner: raeen-design-researcher** (weight ramp), **raegfx/raefont**.

---

## Reference comparison (named gaps, with source)

- **vs macOS Tahoe Liquid Glass Control Center** (512pixels Aqua library; AppleWorld
  Today on the 26.1 tinted toggle): Tahoe's glass is *clear and light-reflecting* with
  a specular highlight that tracks the backdrop; RaeenOS's dark glass is excellent but
  **does not reflect** — it tints + frosts only. Tahoe also ships a **tinted (more
  opaque) mode** as a user option; RaeenOS has the luma auto-adjust but no user
  clear/tinted toggle surfaced. Gap: specular reflection + a user opacity toggle.
- **vs `reference/Liquid Glass UI Kit`:** the reference's **glossy pill buttons with
  colored inner glow** are the headline; RaeenOS controls are flat by comparison
  (finding #6). The reference rim is **vivid and obvious**; RaeenOS rim is subtle
  (finding #5) — defensible as taste (subtle = premium) but currently *under* the
  reference, not a deliberate beat of it.
- **vs Windows 11 24H2 (Mica/Acrylic):** RaeenOS already **beats** Win11 on material
  richness — Mica is flat and opaque with no iridescent signature. Win11's win is
  **finish breadth**: every Win11 surface is complete; RaeenOS's start/taskbar are not.

---

## Consistency issues

- **FONT INCONSISTENCY (worst offender):** Control Center / Files / Notifications use
  RaeSans (correct); Start menu + Taskbar use RaeMono for labels. This is the #1
  cohesion break — it's exactly the "collection of pretty screens vs one system"
  failure IDENTITY.md §1 warns about. Fix → instant uplift.
- **Glass tier discipline:** GOOD. Chrome/panel/popover are visibly the three tiers;
  no surface invents a fourth. The taskbar chrome is the one tier that reads wrong
  (too opaque for chrome).
- **Icon discipline:** GOOD where wired (Files file-types, Control Center). BROKEN on
  start/taskbar (placeholder letters/smudges instead of the 36-icon set).
- **Radii:** consistent on cards/panels; Files toolbar buttons miss the pill radius.
- **Iridescent rim:** applied consistently to panels/popovers/cards — good cohesion.

---

## Blocking (won't render): NONE

All 15 PNGs rendered clean, exit 0, no harness hiccup. No hand-off to verifier/debugger.

---

## Confidence: HIGH

Re-rendered the current pixels myself this session and inspected every surface at full
resolution (full-res crops, not just the downscaled thumbnails). The one caveat:
`surface-settings.png` was rendered but not deep-inspected — flag for the next pass.
This is a host-render judgment; the LIVE on-iron compositor result can differ (project
memory: headless QEMU screendump stripes; iron + host-render are the trustworthy
paths) — an on-iron desktop capture remains the final acceptance gate.
