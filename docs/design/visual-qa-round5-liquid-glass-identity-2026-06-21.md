# Visual QA — Round 5 — Liquid Glass Identity, post-CC/toast re-skin + live Files (2026-06-21)

Static-pixel re-measure of the regenerated identity surfaces (commit `629f93b`) in
`docs/design/screenshots/` against `docs/design/IDENTITY.md`, the curated references
in `docs/design/reference/`, and macOS 26 Tahoe / Windows 11 24H2 from knowledge.

Measured with PIL (Python 3.14, PIL 12.2.0), not eyeballed. Every number below is
reproducible from the committed PNGs. This is the follow-up to
`visual-qa-round4-liquid-glass-identity-2026-06-21.md` (scored **58%**, P0 = dark
internal cards on luminous glass). Round 5 adjudicates the CC/toast re-skin + pill
toggles + popover-frost bump, and critiques the **live Files window for the first
time**.

---

## Verdict up front

**Identity parity vs macOS 26 Tahoe / Windows 11 24H2: ~66%** (was 62% post-rim,
58% at Round 4).

The Round-4 P0 — the single ugliest defect, dark raeshell slate cards punched into
luminous panel glass — is **dead in the pixels.** Measured: CC tiles are now **L91**
sitting on panel glass at **L90.6** (Round 4: tiles L40 on panel L72–95, a polarity
clash). Toasts went **L45 → L84–93.** Pill toggles read as on/off switches. The
popover-frost bump made the third tier finally separate from panel to the eye
(panel L102 / popover L114, Δ11 — was Δ2). Four of the four headline shell surfaces
(CC, toasts, tiers, rim) now belong to one luminous-glass system.

**What now caps parity is one big new thing and a short tail.** The big new thing:
the **live Files window is a flat dark opaque box** — toolbar L51, sidebar L42,
content L45, titlebar L20, zero aurora reading through any chrome. It is a *correct,
real app window* (toolbar/sidebar/ftype-tinted rows, real icons — the dead twin is
finally retired) but it does **not belong to the Liquid Glass identity at all.** It
reads like a 2015 dark-theme file manager dropped onto a 2026 glass desktop. This is
the new headline cohesion break. The tail: glass is still ~half the brightness of
true light-theme frosted glass, the rim warm-amber stop is present-but-weak, and the
aurora peak is still slightly hot under the panels.

Do not call the identity "done." But the shell surfaces (CC/toasts/Start-class
panels) are now genuinely close to the bar; the gap is concentrated in **app
windows**, which haven't received the glass treatment the chrome did.

---

## 1. Adjudicating the Round-4 claims in the pixels

### Claim: CC tiles flipped polarity — luminous, at/above panel glass → **CONFIRMED, FIXED**

Round 4 measured CC internal tiles at **L40.6** (rgb 36/40/60), darkest pixel L25.7,
sitting on panel glass L72–95 — the polarity clash. Round 5 hard data:

| CC region | Round-4 | Round-5 | Reads as |
|---|---|---|---|
| tile interior | L40.6 (36/40/60) | **L91** (87/89/112) | frosted card on glass |
| top-left tile | — | L82.7 mean, med 98.5 | luminous |
| panel glass gap (between tiles) | L72–95 | **L90.6** (86/89/111) | matched |
| media/slider card | — | L70.9 (63/63/128) | slightly heavier, OK |

The decisive number: **tile L91 vs panel-gap L90.6 — Δ0.4.** The cards no longer
punch dark holes in the panel; they sit at the panel's own luminance, exactly the
"light cards on light glass" the gold reference has and Round 4 demanded
(`internal card L ≥ panel interior`). **The Round-4 P0 is closed.** To the eye
(`surface-control-center.png`) the panel now reads as one continuous luminous glass
sheet with subtly-lighter tiles — the cohesion break is gone.

### Claim: toasts luminous (was L45) → **CONFIRMED, FIXED**

| Toast | Round-4 | Round-5 |
|---|---|---|
| toast 1 interior | L45.4 (41/44/72) | **L84.4** (82/81/105) |
| toast 2 interior | — | **L92.7** (89/89/118) |
| aurora under toasts | L137.7 | L123.2 (67/135/205) |

Toasts went from dark slates **L45 → L84–93**, now reading as luminous frosted
popover cards over the aurora (`surface-notifications.png` confirms to the eye:
stacked light-glass cards, urgency accent bar on the left edge, backdrop bleeding
through). **Round-4 P0 #2 closed.** Target was L≥90 — toast 2 clears it (92.7),
toast 1 is just under (84.4) but unambiguously in the luminous band, not the dark-
slate band. Minor: push toast-1 frost +1 step to land both ≥90 uniformly.

### Claim: pill toggles read as on/off switches → **CONFIRMED**

The gaming-row toggles in `surface-control-center.png` now render as pill switches
(rounded track + knob), measured toggle-region L87 with an accent-tinted "on" track.
This is the reference pill-control look (`Liquid glass guide_...jpg` Primary/
Secondary). Round-4 P1 #6 (flat squares, not accent-glow pills) is **substantially
addressed** for toggles. Remaining nit: the inner accent *glow halo* on the "on"
state is faint — the pill shape is there, the colored inner-glow bloom is subtle.
Minor P2 tuning, not a defect.

### Claim: tier separation now visible to the eye (popover > panel after 0x48 frost) → **CONFIRMED, FIXED**

| Tier interior L | Round-4 | Round-5 | Δ to prev |
|---|---|---|---|
| chrome | 65 | **58.9** | — |
| panel | 94 | **102.5** | **+43.6** |
| popover | 96 | **113.8** | **+11.3** |

Round 4's fatal weakness was panel→popover Δ2 (indistinguishable). Round 5 is
chrome 59 < panel 102 < popover 114 — **every step ≥11 luma**, clearing the Round-4
P1 #4 target of "each step ≥8 luma." To the eye (`glass-tiers-over-aurora.png`) all
three panels now read as distinctly different glass densities, left→right, exactly as
the "opacity rises left→right" caption promises. **Round-4 P1 #4 closed.**

### Claim: rim is now a full sweep, not cyan-monochrome → **MOSTLY FIXED**

The Round-4 rim was cyan 13,398 / violet 444 / warm 36 (cyan-monochrome). The 3×
crop was re-shot at the **bottom-right corner** and now shows two real hues:

| Image | cyan | violet | warm |
|---|---:|---:|---:|
| `glass-tiers-over-aurora.png` (full) | 2,775 | **919** | **249** |
| `glass-iridescent-edge-3x.png` (BR corner) | 0 | **1,863** | (warm-pink, see below) |
| `surface-control-center.png` | 2,573 | 0 | 0 |

Direct edge samples on the 3× crop: **right edge avg `(88,95,195)`** = unmistakable
violet; **bottom edge avg `(105,113,160)`** = warm-leaning lilac/pink (the amber
stop reading as a desaturated warm-pink against the violet-blue aurora, not the spec
`0x40_FFC97C` saturated amber). So the **cyan→violet→warm cycle now renders** — the
3× crop proves violet(right)→warm(bottom) in one frame, which Round 4 explicitly
asked for. Violet is no longer a whisper (919 vs 444 on tiers, 1,863 in the crop).

**Remaining rim gaps:** (a) **warm is still desaturated** — it reads as pink-lilac,
not the gold reference's true amber (gold ref warm renders strongly; ours leans
toward the violet end). (b) **Control Center's rim is still cyan-only** (cyan 2,573 /
violet 0 / warm 0) — the CC surface render isn't yet using the full per-perimeter
hue map that the tiers/crop demo uses; CC gets only the cyan top/left band. So the
*demo* surfaces have the sweep, the *shipped CC* surface does not yet. See P1 below.

**Scorecard: all four Round-4 follow-ups closed or mostly-closed.** CC polarity
FIXED, toasts FIXED, pills CONFIRMED, tier separation FIXED, rim sweep MOSTLY FIXED
(warm desaturated + CC still cyan-only).

---

## 2. NEW — the live Files window (`surface-files.png`), critiqued for the first time

This is the real `apps/files::render_preview` (Home demo state) — toolbar, sidebar,
ftype-tinted folder/file icons, real rows with Name/Size columns, selection highlight,
keyboard-hint footer. The dead navy twin with letter glyphs (H/D/d…) that Round 4
flagged as "moot" is **retired** — this is genuinely the shipped surface. Credit for
that: it's a real, populated, legible file manager.

**But it does NOT fit the Liquid Glass identity. It reads as a flat dark opaque
window that breaks cohesion.** Hard data:

| Files region | measured L | rgb | Verdict |
|---|---:|---|---|
| titlebar | **20.4** | (15/20/33) | near-black, opaque |
| toolbar (New Folder / Rename / Trash) | **51.2** | (46/50/66) | dark opaque slate |
| sidebar (Quick Access) | **42.1** | (28/43/68) | dark opaque slate |
| content list | **45.2** | (31/46/74) | dark opaque |
| selected row | 22.4 | (18/22/36) | dark |
| aurora *outside* the window | 71.6 | (35/81/114) | the living backdrop |

The whole window sits at **L20–51 over an aurora at L72** — it is **darker than its
own backdrop**, the exact subtractive-dark-card polarity that IDENTITY §0 calls the
"FAIL — glass looks like a flat dark card," now at the scale of a whole app window.
Zero aurora reads through any part of it (the window is effectively opaque). There is
**no iridescent rim, no frost, no glass tier** — it's the only identity surface in
the set that received none of the Round-2→5 treatment. Next to the now-luminous
Control Center, it looks like it's from a different OS.

### Concrete target treatment (per IDENTITY §7)

IDENTITY §7 assigns Files to `glass.panel` (elev.2, radius md), with the explicit
note that the sidebar may be slightly more translucent within the panel tier. The
right split, matching macOS Finder / Win11 Explorer Mica:

1. **Chrome = frosted glass, content-list = solid (legibility).** Do NOT glassify the
   row list — a dense file list over a moving aurora is a legibility nightmare and
   neither Finder nor Explorer does it. Instead:
   - **Titlebar + toolbar → `glass.chrome`** (L-target ≥ panel interior, ~95;
     currently L20/51). Aurora reads through the top chrome — this is the single
     change that makes the window "belong."
   - **Sidebar → `glass.panel`** with the §2.3 auto-adjust allowing it slightly more
     translucent (target sidebar interior ~L88–95; currently L42). The sidebar is the
     classic Finder/Explorer translucent zone — backdrop should bleed through it.
   - **Content list → solid `bg.surface`** (a near-opaque light-neutral, NOT the
     current L45 dark navy). Target a light content field (Finder uses near-white;
     our dark theme equivalent is `bg.surface` ~L30–40 **neutral**, not the bluish
     L45) so ftype icon colors pop. Keep it solid for row legibility, but lift it off
     near-black and de-tint it so it doesn't read as "dark mode from 2015."
2. **Window edge → the iridescent rim + soft elev.2 shadow** (currently neither).
   This is the fingerprint that makes it visibly a RaeenOS window. Without the rim the
   window has no identity tell at all.
3. **Titlebar close button** is a saturated solid red square (visible top-right) —
   off-system. Should be a `radius_pill`/circular control or the macOS-style traffic-
   light treatment, tinted, not a hard red block. Minor but it's the most eye-catching
   wrong pixel in the frame.

**Owner: raeen-shell-apps** (Files chrome → glass tiers; content-field de-tint;
close-button restyle) with **raeen-gfx** providing the window-chrome glass+rim draw
path if Files isn't yet routed through `draw_glass_surface`. Until the chrome is
frosted, Files is the surface that most drags the *product* parity even though the
demo surfaces look good.

---

## 3. NEW overall parity % vs macOS 26 Tahoe / Win11 24H2 — honest

**~66%, up from 62%.**

The +4 since the last number (62% post-rim) is earned narrowly by closing the
Round-4 P0 (dark internal cards) and the tier-separation gap — both real, both
measured — but it's only +4 because the **live Files window, now critiqued for the
first time, is a fresh ~−6 to −8 drag** that wasn't in the prior scoring (Round 4
scored the dead twin as "moot/not identity-verifiable"). So the shell got better AND
a real shipped surface got added to the ledger as a flat dark box. Net +4.

Calibrated against measured reference numbers:

What we now have (the credit):
- **Luminous tiered glass shell** — CC L91, toasts L84–93, tiers cleanly separated
  (59/102/114). The cohesion mechanism reads. **Most credit.**
- **Internal-card polarity fixed** — the Round-4 ugliest-thing is gone. **Full credit.**
- **Pill toggles** — reference control shape present. **Partial credit** (glow faint).
- **Multi-hue rim on the demo/tiers surfaces** — violet+warm now render. **Partial.**

What caps us below parity (the −34):
- **App windows have no glass identity** (Files: L20–51, opaque, no rim, no frost,
  darker than its backdrop). This is the new headline gap — the shell is glass, the
  apps are not. **~6–8 points.**
- **Glass brightness is still ~52–63% of true frosted glass.** Gold reference
  interior **L174–181 (near-white milky frost)**; ours L91–114. Even granting dark
  theme, Tahoe dark glass lifts its backdrop more. Room for +1 frost step on
  panel/popover. **~6 points.**
- **Rim warm stop desaturated + CC rim still cyan-only.** Gold ref rim is a balanced
  cyan/violet/warm sweep with strong saturated stops; ours: tiers warm 249 (weak,
  pink-leaning), CC warm 0 / violet 0 (cyan-only on the shipped surface). **~4 points.**
- **Aurora peak still hot** under the panels (last measured L171 vs 140–150 target),
  pushing centered panels toward their opaque auto-adjust bound. **~2 points.**
- **Long tail:** glow halo on "on" pills faint; close-button red block; toast-1 frost
  1 step shy of L90. **~few points.**

66% is honest: the shell now genuinely competes; app-window glass and the last frost
step are what stand between us and 80%+.

---

## 4. Prioritized defect list to reach "on par / better than Win11"

Each: **surface → defect (measured) → fix + target value → owner.**

### P0 — the new headline cohesion break

1. **Files window chrome is flat dark opaque, not glass** → titlebar L20.4 / toolbar
   L51.2 / sidebar L42.1 / content L45.2, all *darker than the aurora outside (L72)*;
   no rim, no frost, no tier. Reads as a different-OS dark window next to the luminous
   CC. → Per IDENTITY §7: **titlebar+toolbar → `glass.chrome`** (target interior ≥L90),
   **sidebar → `glass.panel`** slightly-translucent (target ~L88–95), **content list →
   solid de-tinted `bg.surface`** (lift off the bluish L45 near-black to a neutral
   light field so ftype icons pop; keep solid for legibility), **window edge → iridescent
   rim + elev.2 soft shadow.** → owner: **raeen-shell-apps** (tier wiring + content
   de-tint) + **raeen-gfx** (route Files chrome through `draw_glass_surface` if not
   already). This is the single highest-leverage fix for product parity.

2. **Files titlebar close button is a hard saturated-red square** → reads as the most
   off-system pixel in the frame. → Restyle to a pill/circular tinted control (or
   macOS traffic-light set). Target: no hard primary-red square; control matches the
   pill language. → owner: **raeen-shell-apps**.

### P1 — the glass/rim finish on the shell (close the gap to "better than Win11")

3. **Glass interior ~52–63% of reference frost brightness** → ours panel L102 /
   popover L114 vs gold ref interior L174–181. → Push panel + popover `frost`
   white-add +1 step (panel `0x23→~0x2E`, popover `0x38→~0x44`). Target panel ≥L115,
   popover ≥L128 over the aurora, while a11y confirms text still clears 4.5:1.
   → owner: **raeen-gfx** (frost step) + **raeen-ui** (token) + **raeen-accessibility**
   (re-confirm contrast over the lifted glass).

4. **Shipped Control Center rim is cyan-only** → CC chroma cyan 2,573 / violet 0 /
   warm 0, while the tiers demo gets cyan 2,775 / violet 919 / warm 249. The CC render
   path isn't applying the full per-perimeter hue map that the demo surface uses.
   → Route CC's glass edge through the same per-perimeter cyan→violet→warm map the
   tiers/3× crop use. Target CC violet+warm each ≥ ~25% of its cyan count.
   → owner: **raeen-gfx** (per-perimeter hue map on the shipped CC surface).

5. **Rim warm-amber stop is desaturated (reads pink-lilac, not amber)** → tiers warm
   249 px, bottom-edge sample `(105,113,160)` = warm-leaning lilac, not the spec
   `0x40_FFC97C`. Gold ref shows strong saturated warm. → Boost the warm stop's R and
   drop its B at the bottom/bottom-right perimeter position so it reads amber, not
   pink, over the violet-blue aurora. Target a bottom-edge sample with R>G>B and R≥190.
   → owner: **raeen-gfx**.

6. **Aurora peak still hot under the panels** → last measured peak L171 vs 140–150
   target, sitting directly under the centered CC/toast panels (worst-case glass
   contrast cell, pushing them toward their opaque auto-adjust bound). → Trim blue
   blob core weight (138→132 per Round 4); re-measure, 128 only if still >158.
   → owner: **raeen-gfx**; a11y re-confirms 4.5:1 over the new peak.

### P2 — the long tail

7. **"On" pill toggle inner-glow halo is faint** → pill shape present (toggle-region
   L87, accent track) but the colored inner-glow bloom on the on-state is subtle.
   → Strengthen the inset accent-glow on the on-state track. Target a visible RaeBlue
   bloom inside the on pill. → owner: **raeen-shell-apps**.

8. **Toast-1 frost 1 step shy of uniform luminous** → toast-1 L84.4 vs toast-2 L92.7;
   target both ≥90 for a uniform stack. → +1 frost step on the topmost/normal-urgency
   toast fill. → owner: **raeen-shell-apps**.

---

## Reference comparison (named gaps, with measured source)

- **`reference/Liquid glass guide_...jpg` (gold):** full-image mean **L174.2**, glass
  interior ~L174–181 (near-white milky frost), rim cyan 48,945 / violet 38,882 (a
  balanced, saturated sweep). Our gaps: glass **L91–114** (~52–63% as bright — frost
  can go +1 step, P1 #3); rim warm desaturated and CC rim cyan-only (P1 #4/#5). These
  two are now the bulk of the felt *shell* difference.
- **macOS 26 Tahoe (knowledge):** specular highlight + edge light-bend + background
  luminance lift. We have all three on the shell now (white top edge, cyan→violet→warm
  rim on demo surfaces, frost L91+). Remaining vs Tahoe: warm stop desaturated, dark
  glass lifts its backdrop less than Tahoe, and — the big one — **Tahoe's app windows
  (Finder) are themselves glass-chromed; our Files is not** (P0 #1).
- **Win11 24H2 Acrylic/Mica + `reference/download (1).jpg` (Fluent Files on aurora):**
  the reference Files panel is a **light translucent Mica window** with colorful pill
  folder chips over a flowing blue ribbon. Ours: live Files is a **dark opaque box**
  (L20–51) over the aurora — we are *behind* Win11 on the file-manager surface
  specifically, even though our shell glass now beats Mica's flatness. **To clear
  "better than Win11" we must glass-chrome Files (P0 #1).** Win11 has no dark-window-
  on-glass clash; right now we do, in Files.

## Consistency issues

- **Files window has zero identity treatment** (P0 #1) — the one surface that received
  none of the glass/rim/frost system; reads as a different OS. Highest-priority
  cohesion fix.
- **Shipped CC rim ≠ demo rim** (P1 #4): the tiers/crop have the full hue sweep, the
  actual CC surface gets cyan-only — the signature is inconsistent between the proof
  shot and the real surface.
- **Rim warm hue not uniform/saturated** (P1 #5): warm reads pink-lilac, not amber.
- **Internal-card polarity — RESOLVED** (CC tile L91 ≈ panel L90.6). No longer a defect.
- **Tier separation — RESOLVED** (59/102/114, each step ≥11 luma). No longer a defect.
- Corner radii consistent across CC/toasts/tiers (lg ~16) — **no defect.**
- Files close button red block — off-system control styling (P0 #2).

## Blocking (won't render)

None. All six identity surfaces render cleanly via the host rasterizer, and
`surface-files.png` is now the **live** `apps/files` render (the dead twin is retired)
— so it is finally identity-verifiable, and the verdict is in §2. No handoff to
verifier/debugger needed.

## Confidence

**High** that the Round-4 P0 (dark internal cards) and the tier-separation gap are
fixed — both measured to hard numbers (CC tile L40→L91, tile≈panel Δ0.4; tiers
59/102/114). **High** that toasts are luminous (L45→L84–93) and the rim sweep now
renders violet+warm on the demo surfaces (right-edge `(88,95,195)` violet, bottom
warm-pink). **High** that the live Files window is a flat dark opaque box with no
glass identity (titlebar L20 / sidebar L42 / content L45, all darker than the L72
aurora outside — measured). **Medium** on the exact parity number (66% is a calibrated
estimate against measured reference luma/chroma, not a metric). **Medium** on the
precise trim targets (frost +1 step, warm R≥190, blue 138→132) — each needs one
host-render iteration to dial in.
