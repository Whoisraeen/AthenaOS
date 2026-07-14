# Visual-QA Pixel Critique — 2026-06-21

> raeen-visual-qa, judging the host-rendered shipped UI (the EXACT SW rasterizer
> the kernel composites with) against current macOS Sequoia/Tahoe 26 + Windows 11
> 24H2. Source PNGs: `docs/design/screenshots/` (regenerate:
> `cd tools/ui_screenshot && cargo run --release`). Specs judged against:
> `design-language.md`, `material-and-shadow.md`, `control-center.md`,
> `notifications.md`, `files.md`, `typography-rendering.md`.

This is a harsh, specific critique. The goal is world-class, not "fine."

---

## Overall verdict

**Split-personality UI.** The *atoms* (shadow, glass, type, primitives, focus)
are genuinely close to world-class — the rendering primitives are there. The
*surfaces* that compose them range from "promising but unfinished" (Control
Center) to "1995 terminal" (Files, Notifications). The gap is NOT the renderer —
it's that two of the three shipped surfaces are still drawn with the **8×8 bitmap
block font and hard rects**, bypassing the raefont AA engine and the glass/shadow
ladder the atoms prove exist.

- **Atoms: ~85% of world-class.** Shadow ramp is soft and premium. Type AA is
  crisp, em-dashes correct. Focus ring has the glow+ring contract. Primitives clean.
- **Control Center: ~45%.** Right layout skeleton, but icons are stray single
  letters, sublabels clip into the next tile, and the panel casts NO shadow on the
  desktop. Reads "wireframe," not "Sequoia."
- **Files: ~20%.** Entirely 8×8 block font, hard rects, letter "icons." This is
  the single worst-looking shipped surface and it's a flagship app.
- **Notifications: ~20%** — but this is the *unpolished twin*
  (`raeshell::notifications_daemon`), NOT the polished kernel `notify.rs`. Flagged
  as a renderer-selection bug in the harness, not the real toast UX. Still: it
  ships, so it counts until the harness points at `notify.rs`.

**Bottom line:** the foundation is world-class-capable; the surface composition is
not yet shipped through it. Closing this is mostly "route surfaces through raefont
+ the glass/shadow path the atoms already use," not new rendering work.

---

## Ranked top polish items (do these in order)

| # | Surface | Defect | Severity | Owner |
|---|---|---|---|---|
| 1 | Files | Whole window is 8×8 block font + hard rects + letter "icons" — looks 30 years old | **CRITICAL** | raeen-shell-apps |
| 2 | Control Center | Tile "icons" are stray single letters (W/B/F/N/A/X/G/R/P); not glyphs | **CRITICAL** | raeen-shell-apps |
| 3 | Control Center | "Off" sublabel clips/overlaps into the tile row below (vertical rhythm broken) | **HIGH** | raeen-shell-apps |
| 4 | Control Center | Panel casts NO drop shadow against the dark desktop — floats with no `elev.3` | **HIGH** | raeen-gfx |
| 5 | Control Center | Panel top is clipped at the screen edge (no `space.4` top inset / off-screen) | **HIGH** | raeen-shell-apps |
| 6 | Notifications | Unpolished twin renders (8×8 font, hard rects, literal "?" glyphs) | **HIGH** | raeen-shell-apps (renderer selection) |
| 7 | Glass panel | Tint too opaque/dark — reads as smoked-solid, not translucent blur | **MEDIUM** | raeen-gfx |
| 8 | Type ramp / CC | Chrome text colored accent-blue (RaeMono header, CC labels) — violates chrome-restraint rule | **MEDIUM** | raeen-ui |
| 9 | Drop shadow | Soft ramp is correct but reads slightly over-wide/under-dense vs macOS at `elev.4` | **LOW** | raeen-gfx |
| 10 | Files | No glass, no `material.mica` titlebar, no row hover/selection styling, no file-type color chips | **MEDIUM** | raeen-shell-apps |

---

## Per-surface findings

### atom-drop-shadow.png — PASS (premium), minor tuning
The left "Soft ambient" card shows a genuinely **soft, feathered penumbra** — wide
spread, smooth alpha decay, near-neutral (not blue) — clearly distinct from the
right "Old offset block" with its tight hard dark band. The harness even prints
`near=176 < far=214 <= field=244 -> SOFT RAMP`, and the pixels back it up. The
`material-and-shadow.md` #1 defect is **fixed**. This is the headline win.

- **LOW — owner: raeen-gfx.** Against macOS Sequoia `elev.4` card shadows, ours is
  a touch *too diffuse and too light* — macOS holds a slightly denser core near the
  card edge then decays. Consider nudging the core alpha up (~`0x66`→core) while
  keeping the long tail, so the lift reads more confident. Not a blocker.
- Both cards are near-white surfaces on a light-gray field — fine for an isolation
  swatch, but verify the same shadow over a *busy wallpaper* (the real failure mode
  the spec called out) in a follow-up capture.

### atom-glass-panel.png — MEDIUM defect
Top-edge highlight is present (faint bright line along the top). Corner radius and
border hairline read correctly. **But the glass tint is too opaque** — over a
bright blue backdrop the panel reads as a *dark smoked-glass solid*, not a
translucent blurred material. The backdrop blobs are barely perceptible through it.

- **MEDIUM — owner: raeen-gfx.** `material.glass` dark tint is `0x9E_1A_1E_2E`
  (~62% alpha). At this alpha over a bright backdrop the blur is invisible —
  macOS/Win11 Acrylic let significantly more backdrop luminance and color bleed
  through. Either lower tint alpha toward ~45–50% on bright backdrops or strengthen
  the blur so the backdrop reads as *color-bled frost*, not a flat dark sheet. Right
  now it looks like tinted plexiglass, not frosted glass.
- The inner content panel (the darker inset rect) has almost no contrast against the
  glass body — it reads as a second flat layer with no material distinction.
- "Primary" button: flat accent fill is fine, but it's slightly tall for its label
  and has no visible hover/press affordance baked into this swatch (acceptable for a
  static atom).

### atom-type-ramp.png — PASS (crisp), one color nit
Text AA is **crisp** at every ramp step (Display 32 → Caption 11). Em-dashes render
correctly (`Display — RaeSans`). Weight differentiation between Display/Title and
Body/Caption reads. RaeMono renders cleanly with code ligature-free monospace. This
is world-class text rendering for a software raster.

- **MEDIUM — owner: raeen-ui.** The `RaeMono — JetBrains Mono` *section header* is
  rendered in accent-blue. Per `material-and-shadow.md` §"Chrome color restraint"
  and `design-language.md` §4.3, static labels/headings use `text.*`, never
  `accent.*`. This header should be `text.secondary` neutral. Same rule the CC
  labels violate (#8).
- Optical nit: `Caption — RaeSans` at 11px is legible but the letter-spacing looks
  slightly tight vs the spec's `+0.2px` caption tracking — verify the tracking
  token is applied.

### atom-primitives.png — PASS
Rounded-rects across radii 0/6/14/24/40 are clean with correct corner geometry.
Vertical gradients are smooth (no banding visible). AA circles are smooth at all
four sizes. No notes — this is the proof the primitive layer is solid.

### atom-focus-ring.png — PASS
The accent focus ring (left) shows the **glow halo + ring** contract from
`design-language.md` §8 / `material-and-shadow.md` `elev.focus` — not a flat 1px
border. The high-contrast forced-colors variant (right) auto-swaps to cyan and is
unmistakable. This clears the a11y focus-visibility requirement.

- **LOW — coordinate raeen-accessibility.** Confirm the ring is the full 2px
  `accent.base` ring *plus* the glow (spec says never glow-only or ring-only). It
  looks like both are present; verify thickness measures 2px at 1x.

### surface-control-center.png — CRITICAL composition gap (the headline surface)
This is supposed to be the highest-bar glassmorphic surface. Right now it's a
recognizable Quick-Settings *skeleton* with several disqualifying defects:

- **CRITICAL #2 — owner: raeen-shell-apps.** Every tile's icon slot is a **stray
  single letter** in tiny block font (`W` Wi-Fi, `B` Bluetooth, `F` Focus, `N`
  Night Light, `A` Airplane, `X` High Contrast, `G` Game Mode, `R` RGB, `P`
  Performance). macOS/Win11 use crisp filled glyphs. These read as placeholder
  mnemonics, not icons. The Wi-Fi list rows and the footer also show stray
  glyphs (`#`, `*`, `S`, `P`). Needs a real icon set (or at minimum a consistent
  monochrome glyph font), not ASCII keycaps.
- **HIGH #3 — owner: raeen-shell-apps.** The tile label + "Off" sublabel **collide
  with the next row** — "Off" sits flush against / overlapping the tile boundary
  below it. Vertical rhythm is broken; the spec's `space.2`/`space.3` tile internal
  padding isn't being honored. Tiles need fixed internal padding and the grid needs
  consistent row gaps.
- **HIGH #4 — owner: raeen-gfx.** The panel casts **no drop shadow** against the
  dark desktop. Per `control-center.md` §1 it's `elev.3` (offset 8, radius 28).
  Against `bg.base` the dark-on-dark shadow is invisible — but `elev.3` at
  `0x55_00_00_00` over a near-black desktop genuinely produces almost nothing, so
  the panel has no separation from the wallpaper. Either the shadow isn't being
  applied to this surface, or `elev.3` needs a subtle light/stroke edge-lift on dark
  desktops so glass panels read as floating. macOS solves this with a bright 1px
  rim + faint shadow; ours has the rim spec'd but the panel edge here is faint.
- **HIGH #5 — owner: raeen-shell-apps.** The panel **top is clipped at the screen
  edge** (the first tile row is jammed against y=0). Spec anchors bottom-right with
  `space.4` insets and sizes to content; here it's overflowing the top of an
  800px-tall frame. Either content overflows the panel max-height (needs scroll) or
  the anchor math is wrong.
- **MEDIUM #8 — owner: raeen-ui.** Tile labels and section text appear desaturated
  but the "on" Game Mode tile and the Wi-Fi selected row use accent fills correctly;
  confirm *off* tile labels read `text.primary`/`text.secondary` and not a dim
  accent. The "Gaming" section header is correctly neutral — good.
- **Positive:** the overall structure is right — 2-col toggle grid, expanded Wi-Fi
  list in place (the macOS expand-in-place model), brightness/volume sliders with
  accent-filled tracks and round knobs, inline media card ("Midnight City / M83"),
  the Gaming section (Game Mode/RGB/Performance) that is uniquely ours, and a
  footer. The *information architecture* clears the bar; the *finish* does not.
- **vs macOS Sequoia:** ours lacks the soft module grouping (macOS uses subtle
  rounded module containers with breathing room); our tiles are flatter and more
  cramped. **vs Win11 24H2:** our slider knobs are good, but Win11's tiles have
  larger glyphs + clearer on/off fill states; ours are ambiguous (the "Off"
  sublabel does the work an icon-state should).

### surface-files.png — CRITICAL (worst shipped surface)
This is a flagship app and it currently looks like a **DOS file manager**:

- **CRITICAL #1 — owner: raeen-shell-apps.** The entire window — tab "user", path
  bar `/home/user`, sidebar (Home/Desktop/Documents/Downloads/Music/Pictures/
  Videos/Trash), column headers (Name/Size/Type), and the "0 items" status bar — is
  rendered in the **8×8 bitmap block font**, NOT raefont. The atoms prove crisp AA
  text exists; Files doesn't use it. This alone disqualifies it from "world-class."
- **HIGH — owner: raeen-shell-apps.** Sidebar "icons" are single letters
  (`H`/`D`/`d`/`L`/`M`/`P`/`V`/`T`) — same placeholder-glyph problem as CC. macOS
  Finder / Win11 Explorer use crisp filled folder/media glyphs with `ftype.*` color
  semantics (`design-language.md` §4.4, `files.md` §4). None of that color coding is
  present.
- **MEDIUM #10 — owner: raeen-shell-apps.** No glass, no `material.mica` titlebar,
  no row hover/selection elevation, no concentric corner radii, no drop shadow. The
  window is a flat dark rect with hairline dividers. It needs the full chrome
  treatment the atoms enable.
- **Positive:** the *layout* is correct — tab strip, path/breadcrumb bar, left
  sidebar, three-column list header, status bar. Bones are right; every pixel of
  finish is missing.

### surface-notifications.png — HIGH, but it's the wrong renderer
Flagged context confirmed in the pixels: this renders
`raeshell::notifications_daemon` (the **unpolished twin**), not the polished kernel
`notify.rs`. What's on screen:

- Hard rectangular cards with **1px flat blue borders** (no glass, no soft shadow,
  no rounded corners to speak of).
- 8×8 block font throughout (titles, body, buttons).
- Literal `?` glyphs where emoji/icons should be ("plug in soon. ?", "tonight? ?").
- Title text colored accent-blue ("Update ready", "Aria Chen") — chrome-restraint
  violation again.
- Buttons (Install / Reply / Mute) are block-font text in 1px-bordered boxes.

- **HIGH #6 — owner: raeen-shell-apps.** The fix is a **renderer-selection** bug:
  the screenshot harness (and presumably the running shell) points at the
  daemon-side `notifications_daemon` renderer instead of the kernel `notify.rs`
  (which per `notifications.md` §49 already has glass tokens, `RADIUS_MD`,
  `stroke_strong` top edge, urgency bars, stack-depth cue wired). Either retire the
  daemon renderer or route both through `notify.rs`. Until then, the *shipped* toast
  UX on this path is basic — so it counts.
- **vs macOS/Win11:** both reference systems use rounded glass/acrylic toast cards
  with app icon, soft shadow, and clean type. This twin has none of it. The kernel
  `notify.rs` reportedly does — so the priority is killing the twin, not rebuilding.

---

## Cross-surface consistency issues

- **Two text renderers ship simultaneously.** Atoms + (some) CC text use raefont AA;
  Files, Notifications, and CC tile/icon glyphs use the 8×8 block font. This is the
  single biggest *incoherence* — the same OS renders text two completely different
  ways. **Owner: raeen-shell-apps** (route every surface through raefont);
  **raeen-ui** owns the shared `raeui::tokens`/text path.
- **Placeholder letter-glyphs as icons** across CC and Files (W/B/F/H/D/…). No icon
  system is wired. **Owner: raeen-ui** (define an icon token set) + raeen-shell-apps
  (consume it).
- **Accent-colored chrome text** recurs (RaeMono header, notification titles,
  possibly CC labels) — violates the chrome-restraint rule in three places. **Owner:
  raeen-ui** (enforce `text.*` at label call sites).
- **Drop shadow not applied to live surfaces.** The atom proves a great soft shadow,
  but the Control Center panel shows none. The `elev.*` ladder is built but not
  *routed* to shell surfaces. **Owner: raeen-gfx** (confirm `SurfaceEffect::
  DropShadow` is attached to CC/Files/window chrome).
- **Radii inconsistency:** atoms use the full `radius.*` scale; Files/Notifications
  twin use ~0px hard corners. Once surfaces route through the primitive path this
  resolves.

---

## Blocking (won't render): none
All 8 PNGs rendered cleanly via the host rasterizer (no QEMU striping artifact).
No hand-off to verifier/debugger needed — these are finish defects, not boot/render
failures.

## Confidence: HIGH
Judged directly on the host-rendered pixels (the exact kernel SW rasterizer),
cross-checked against the design tokens and current macOS Sequoia/Tahoe 26 + Win11
24H2 behavior. The atom verdicts are unambiguous; the surface verdicts are about
finish, not interpretation.


---

# Round 2 - re-critique after polish pass 1 (icon system + Control Center) - 2026-06-21

> Re-judged on the FRESH host-rendered PNGs (no QEMU boot, no harness re-run -
> read the existing docs/design/screenshots/*.png). New this round:
> atom-icons.png (23-glyph line-icon set) + the polished
> surface-control-center.png. Atoms re-spot-checked: drop-shadow, glass, type,
> focus-ring - all unchanged-good (same pixels as Round 1; verdicts stand).

## Round-2 verdict

**The polish moved the UI meaningfully toward world-class - call it +15 points on
the surface that got touched, and it unlocked the single biggest remaining win.**
Control Center went from ~45% to ~60% of world-class. Three of the four CC fixes
landed cleanly in pixels; the fourth (top inset) only half-landed. More importantly,
the icon system is the headline win of this round - it is genuinely good and
retires the "no icon system is wired" cross-surface blocker at the *atom* level. The
UI is no longer split between "world-class atoms / wireframe surfaces" on the icon
axis - the atoms now PROVE a real icon set exists. The remaining gap is purely
*routing*: the icons exist but are not consumed by CC tiles, Files sidebar, or the
notification cards yet. That is the #1 next item and it is now a wiring job, not a
design job.

## Per-fix landed / not (the 4 from Round 1)

**(a) Bright top rim-lift so the panel reads as floating on the dark desktop -
LANDED (improved), but light.**
There is now a visible bright hairline along the top edge of the panel - the
stroke_strong / rim-lift is present where Round 1 had effectively nothing. Combined
with the new vertical luminance gradient on the panel body, the panel reads as a
distinct floating object rather than a hole in the wallpaper. *Still slightly off:*
the rim is faint and only really reads along the very top; the left/right vertical
edges of the panel are nearly invisible against the dark desktop. macOS keeps a
continuous ~1px bright rim around the *whole* glass perimeter (not just the top) plus
a faint ambient shadow. Nudge the side/bottom edge stroke up a touch so the panel has
a complete floating outline, not just a top lip. Owner: raeen-gfx. (Down from
HIGH to LOW.)

**(b) Frosted glass with backdrop bleed-through (not smoked-opaque) - LANDED.**
The panel body now reads as translucent navy *frost* with a clear top-to-bottom
luminance gradient (lighter, lifted at the top; settling darker toward the footer),
not the flat dark plexiglass sheet of Round 1. This is the right material direction.
*Caveat on evidence:* the desktop content (the blue ambient blob) sits entirely to
the LEFT of the panel in this capture, so there is no busy backdrop *directly behind*
the panel to prove color bleed-through conclusively - the frost reads correctly but
the true backdrop-bleed test still wants a capture with wallpaper detail behind the
panel body (same follow-up the shadow atom needs). Treat as landed; verify bleed over
a busy wallpaper next capture. Owner: raeen-gfx. (Down from MEDIUM to LOW.)

**(c) On/Off sublabels separated + inside each tile (no overlap) - LANDED, clean.**
This is the cleanest fix of the four. Every tile now shows its label and the "Off" /
"On" sublabel stacked *inside* the tile bounds with clear separation; nothing
collides with the row below. Vertical rhythm is restored - the space.2/3 tile
internal padding is being honored now. The Wi-Fi/Bluetooth/Focus/Night Light/
Airplane/High Contrast grid and the Gaming tiles (Game Mode "On", RGB/Performance
"Off") all read correctly. Fully resolved. (Was HIGH #3 - closed.)

**(d) Footer fully visible + panel top not clipped - HALF-LANDED.**
Footer: LANDED. The footer row (avatar + "AthenaOS" + the two trailing glyphs) is
fully visible and not clipped at the bottom - Round 1 overflow is gone and the panel
now sizes to contain its full content.
Panel top: STILL OFF (improved). The first tile row (Wi-Fi/Bluetooth) is no longer
*clipped*, but it is jammed right up against the top of the frame (~y=20). The spec
space.4 top inset is not there - the panel starts almost at the screen edge with no
breathing room above the first row. macOS/Win11 float the panel down from the top edge
with a clear margin. Add the top inset so the panel does not kiss the screen edge.
Owner: raeen-shell-apps. (Down from HIGH to MEDIUM - no longer clipping, just no
top margin.)

**Fix scorecard: (a) landed-light - (b) landed - (c) landed-clean - (d) footer landed /
top inset still off.** 3.5 of 4.

## Icon-system verdict - STRONG PASS, ship it

atom-icons.png is the standout of this round. The 23-glyph set (wifi, bluetooth,
focus, night-light, airplane, accessibility, game, palette, performance, folder,
file, code, media, doc, archive, exec, bell, gear, search, close, chevron, plus,
check) are real, recognizable line icons - not letters, not bitmaps.

- Recognizability: every glyph reads as its concept at a glance. Wifi arcs,
  bluetooth rune, crescent focus/moon, sun-rays night-light, paper-plane airplane,
  the accessibility person-in-circle, the gamepad, the artist palette - all
  unambiguous. This clears the macOS SF-Symbols / Win11 Fluent "instantly legible
  monochrome glyph" bar.
- Stroke consistency: uniform stroke weight across the set at the base size; the
  family reads as ONE coherent system, not mixed sources. This is the #1 thing that
  separates a real icon set from a grab-bag, and it holds.
- Scaling proof: the 72px hero row is the convincing part - strokes stay crisp
  and the geometry holds at 6x with no fuzz or aliasing breakdown, which proves these
  are vector glyphs rendered through the AA path, not upscaled bitmaps. The gamepad,
  accessibility circle, and palette in particular survive the blow-up cleanly.
- Tinting: the monochrome bottom row proves the glyphs take any token color
  (folder = accent blue, code = green, media = magenta, doc = amber, archive =
  orange, performance = warm) - so the ftype.* file-type color semantics the Files
  spec wants are now *possible* with one glyph + a tint, exactly right.

Minor notes (LOW, not blockers): a couple of the smallest glyphs (search, close) are
a hair light in weight relative to the denser ones (gamepad, archive) - at 16px the
optical weight is not perfectly matched; consider a tiny stroke-weight bump on the
sparse glyphs for optical evenness. And there is no folder-open vs folder-closed
variant yet (Files will want both). Neither blocks shipping the set.

Verdict: this is a world-class-capable icon system. It is the proof that the
"placeholder letter-glyph" cross-surface defect from Round 1 is design-solved. What
remains is consumption.

## Updated ranked remaining gaps (post-Round-2)

| # | Surface | Defect | Severity | Owner |
|---|---|---|---|---|
| 1 | CC + Files | Wire the new icon set into the surfaces - CC tiles still show letter placeholders (W/B/F/N/A/X/G/R/P); Files sidebar still H/D/L/M/P/V/T. Icons EXIST now; consume them. | CRITICAL | raeen-shell-apps |
| 2 | Files | Whole window still 8x8 block font + hard rects - route through raefont + glass/shadow path. Untouched this round; now the worst shipped surface. | CRITICAL | raeen-shell-apps |
| 3 | Notifications | Unpolished daemon twin still renders (8x8 font, flat blue borders, ? glyphs) instead of kernel notify.rs - renderer-selection / dead-twin fix. | HIGH | raeen-shell-apps |
| 4 | CC | Panel top has no space.4 inset - first row kisses the screen edge (no longer clipped, just no margin). | MEDIUM | raeen-shell-apps |
| 5 | CC panel | Rim-lift is top-only - extend the bright 1px rim around the full perimeter (side/bottom edges vanish on dark). | LOW | raeen-gfx |
| 6 | Glass / CC | Backdrop bleed-through unproven over a busy wallpaper - capture with wallpaper detail directly behind the panel to confirm frost reads as color-bled, not just gradient-tinted. | LOW | raeen-gfx (capture) |
| 7 | Icon set | Optical stroke-weight evenness at 16px (sparse glyphs lighter than dense) + no folder-open variant for Files. | LOW | raeen-ui |
| 8 | Type ramp / chrome | Accent-blue chrome text (RaeMono header) - chrome-restraint nit, unchanged from R1. | LOW | raeen-ui |

**The dead-twin issue (Files/Notifications):** unchanged from Round 1 and now the
limiting factor on two of three shipped surfaces. Files was not touched this round
(still DOS-grade), and Notifications still routes to the unpolished daemon twin. Once
items #1-#3 land - icons wired into CC tiles + Files, Files routed through raefont/
glass, and the notification renderer pointed at notify.rs - the UI crosses from
"world-class atoms + one good surface" to "world-class throughout." That is the next
round whole job, and it is overwhelmingly *wiring/routing* work, not new design.

## Round-2 confidence: HIGH
Judged on the fresh host-rendered pixels (the exact kernel SW rasterizer). The icon
verdict and three of the four CC fixes are unambiguous in the pixels; the half-landed
top-inset and the bleed-through caveat are stated as such. Atoms re-checked and
unchanged.
