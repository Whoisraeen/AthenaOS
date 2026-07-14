# Visual QA — Round 4 — Liquid Glass Identity, post-polish re-measure (2026-06-21)

Static-pixel re-measure of the regenerated identity surfaces (commit `dcb4ee3`) in
`docs/design/screenshots/` against `docs/design/IDENTITY.md`, the curated references
in `docs/design/reference/`, and macOS 26 Tahoe / Windows 11 24H2 from knowledge.

Measured with PIL (Python 3.10.11), not eyeballed. Every number below is
reproducible from the committed PNGs. This is the follow-up to
`visual-qa-round3-liquid-glass-identity-2026-06-21.md`, which scored the identity at
**~35%** and named four P0/P1 defects. Round 4 adjudicates whether those are fixed.

---

## Verdict up front

**Identity parity vs macOS 26 Tahoe / Windows 11 24H2: ~58%** (was 35%).

This is a real, large jump and the polish pass earned it. Three of the four Round-3
defects are genuinely, measurably fixed in the pixels — not just on the meter, but
to the eye. The void is dead, the aurora is luminous, the glass adds light instead
of subtracting it, the tier ordering is now monotonic, and — the headline — **the
iridescent rim that measured ZERO chromatic pixels in Round 3 now renders 13,398
cyan + 444 violet chromatic pixels on the tiers panel.** The signature exists.

What keeps us short of parity (and short of "better than Win11") is now a tighter,
more specific list: the rim is **cyan-monochrome, not a cyan→violet→amber sweep**
(warm stop renders ~0 pixels), the **internal cards inside Control Center /
notifications are still dark athshell slates** sitting on luminous glass (the single
ugliest remaining thing), controls are **flat squares not accent-glow pills**, and
**Files is still the dead twin** (flat navy void, letter glyphs, no aurora — not on
the live render path). None of these is a render-correctness bug now; they are
finish work on known surfaces.

Do not let anyone call the identity "done." But Round 3's "strong skeleton, no skin"
is no longer true — the skin is now half-applied, and what's applied is good.

---

## 1. Adjudicating the four Round-3 P0/P1 defects — are they fixed in the pixels?

### Defect 1 (P0) — Iridescent rim rendered ZERO chromatic pixels → **FIXED (cyan), PARTIAL (full sweep)**

The Round-3 blocker was a correctness bug: 0 cyan / 0 violet / 0 warm in the 3× crop.
That is **resolved**. Hard data:

| Image | cyan px | violet px | warm px |
|---|---:|---:|---:|
| `glass-iridescent-edge-3x.png` (one corner crop) | **109,944** | 0 | 0 |
| `glass-tiers-over-aurora.png` (full, 3 panels) | **13,398** | **444** | **36** |
| `surface-control-center.png` | **82,955** | 0 | 0 |

Per-edge scan of the middle (panel) surface confirms the rim is **position-aware**:
TOP edge cyan=43/violet=12, LEFT edge cyan=111, RIGHT edge violet=21/cyan=1
(sample `(81,69,160)` = R≈B>G, genuinely violet), BOTTOM edge cyan=0/violet=0/warm=0.

So: the rim draws, it reads as **chromatic not bright-white** (the 3× crop's left edge
sample `(114,201,232)` is unmistakable cyan, distinct from the white top-highlight
`(>225,>225,>225)` which tallies separately at 801 px), and the hue **shifts** along
the perimeter (cyan top/left → violet right). gfx's claim of "168 cyan / 232
violet-warm px" is **directionally confirmed but the ratio is wrong**: cyan
outnumbers violet ~30:1 and warm is effectively absent (36 px total, all weak
`(168,140,127)` near-amber, not the spec's `0x40_FFC97C`). See §3 for the verdict on
whether that matters.

**Round-3 P0 #1: FIXED.** The "instantly recognizable signature" now exists. The
remaining gap (full rainbow sweep) is a tuning item, not a blocker — re-filed as P2
below.

### Defect 2 (P0) — Glass is dark/subtractive, not luminous/frosted → **FIXED**

Round-3 measured glass interiors *darker* than a bright backdrop (subtractive). The
frost luminance-add term landed and the polarity flipped. Hard data:

| Surface | Round-3 interior | Round-4 interior | Reads as |
|---|---|---|---|
| tiers chrome | (24,49,75) L≈49 | (33,71,98) **L 65** | light frost over aurora |
| tiers panel | (30,46,81) L≈46 | (72,96,139) **L 94** | luminous |
| tiers popover | (27,31,59) L≈31 | (94,93,134) **L 96** | luminous |
| CC panel interior | "dark muddy navy" | (86,89,179) **L 95** | luminous blue glass |

The glass now **adds light**: panel interior L94 sits at parity with the local
backdrop (L94) rather than well below it — it's reading as frosted glass, not a dark
card. **Round-3 P0 #2: FIXED.** (Caveat: still only ~52% of the gold reference's
frosted-glass brightness — see §4 — but that's a light-vs-dark-theme delta, and the
polarity itself is correct now.)

### Defect 3 (P1) — Aurora too dim (peak luma 94) → **FIXED**

| Metric | Round-3 | Round-4 | Target (IDENTITY §3.2) |
|---|---:|---:|---|
| mean luma | 43.7 | **86.5** | — |
| peak luma | 94 | **171** | ~140–150 |
| brightest px | (42,104,140) | **(124,177,255)** | bright RaeBlue core |
| center luma | — | 149 | legible |

The aurora is now a luminous blue-violet-teal mesh, not dim blobs. To the eye it
reads as a living wallpaper with a bright RaeBlue core drifting center, a violet wash
right, vignette intact (corners L≈30/42). **Round-3 P1 #3: FIXED** — and slightly
overshot the target (see §2).

### Defect 4 (P1) — Tiers visually indistinguishable / ordering inverted → **FIXED (ordering), PARTIAL (panel↔popover separation)**

Round-3: chrome 49 / panel 46 / popover 31 — **inverted** (popover darkest). Round-4
interior luma is now **monotonic**: chrome 65 < panel 94 < popover 96. The ordering
inversion is gone — gfx's fixed per-tier frost step did its job, and the
`tier_luminance_is_monotonic` KAT is doing what it promised.

**But to the eye, panel (94) and popover (96) are 2 luma apart — indistinguishable.**
The chrome→panel step (65→94, Δ29) reads clearly; the panel→popover step (94→96, Δ2)
does not. The "opacity rises left→right" promise is now *half* legible: you can see
chrome is more see-through, but panel and popover look like the same glass.
**Round-3 P1 #4: ordering FIXED; panel/popover visual separation still weak** —
re-filed as P1 below.

**Scorecard: 4/4 defects measurably improved; 2 fully closed, 2 closed on the
primary axis (rim draws / ordering monotonic) with a secondary tuning gap remaining.**

---

## 2. Is the aurora peak 169/171 "slightly hot"? Drop blue weight 138→128?

**Verdict: yes, trim it — but only a little. Drop blue weight 138→132, not all the
way to 128.**

Measured peak is **luma 171** (gfx flagged 169; my step-4 sample found 171 at
`(124,177,255)`). The IDENTITY §3.2 target was "core hits luma ~140–150." So we're
**~21 over target** at the very brightest pixel. Is that a problem?

- **For mood: no.** Mean luma 86.5 is a healthy, premium night-sky exposure; the
  frame is not washed out.
- **For glass legibility: marginally yes.** The brightest aurora region (L171,
  center) is exactly where the Control Center and notification panels sit. Glass +
  body text over a L171 backdrop is the worst-case contrast cell — and the §2.3
  over-bright luma auto-adjust (`GLASS_LUMA_HI = 0.6` ≈ L153) will be kicking in
  across a large bright area, nudging every centered panel's alpha up `+0x18`. That's
  the system working as designed, but it means panels over center will trend toward
  their opaque bound, undercutting the "see-through" read precisely on the headline
  surfaces.

So the peak is **slightly hot** for a reason that matters (it's under the panels),
not just cosmetics. Recommendation: trim the blue blob core so peak lands ~150–155
(L171→~152). Dropping the blue weight 138→**132** gets roughly there
(linear-ish: 171×132/138 ≈ 164; go to ~128 only if a second iteration still measures
>158). **Do not flatten it to dim** — the brightness is what makes the glass
luminous; just pull the single hottest core down out of the auto-adjust's
over-bright band. Owner: **athena-gfx**. Hand the post-trim peak to
**athena-accessibility** to confirm `text.primary` over the new peak still clears
4.5:1 (IDENTITY §8.7 / §9).

---

## 3. Is the rim now too WHITE at the corner? Should other corners show violet/amber?

**Two separate questions; verdicts differ.**

**(a) Too white at the corner? — No, the rim chroma is fine; the white is the
correct, separate top-highlight.** The 3× crop classifies **109,944 cyan px** vs
**801 white-highlight px** — the chromatic band is 137× more present than the white.
The white you see at the very top edge is the intended 1px `stroke.strong`
specular highlight (IDENTITY §2.4, "On top of the rim: the 1px top-edge highlight"),
which is *supposed* to read as a bright chrome highlight. It is correctly layered
ON TOP of a clearly cyan rim (sample `(114,201,232)`), not replacing it. So the
corner is **not** a white chrome highlight masquerading as the edge — it's
highlight-over-cyan-rim, exactly per spec. No defect here.

**(b) Should other corners sweep violet→amber? — YES. One cyan corner is NOT
enough, and this is the rim's remaining gap.** The full-image scan is decisive:

| Surface | cyan | violet | warm |
|---|---:|---:|---:|
| tiers (3 panels) | 13,398 | 444 | **36** |
| control-center | 82,955 | 0 | **0** |
| **gold reference** (bottom-right card) | 6,178 | **10,264** | **1,139** |

The gold reference (`Liquid glass guide_...jpg`) has a **balanced full sweep** —
violet actually *dominant*, warm strongly present (1,139 px). Ours is
**cyan-monochrome**: violet is a minor hint on right edges only, and the warm/amber
bottom stop renders to **~0 real pixels** (the 36 "warm" px on the tiers are a weak
`(168,140,127)`, nowhere near the spec `0x40_FFC97C`; CC has literally 0). The bottom
edge of every panel scanned shows cyan or nothing where it should show amber.

The reference's whole "liquid glass refracts a *rainbow*" identity comes from the
multi-hue sweep. A cyan-only rim reads as "a glowing cyan outline," which is
pretty but is **not** the iridescent signature and is closer to a generic neon-glass
theme than to macOS Tahoe / the gold kit. **Fix: make the bottom/bottom-right edge
actually render the warm `GLASS_EDGE_WARM 0x40_FFC97C` stop, and strengthen the
violet on the right edge** so the per-perimeter interpolation produces the full
cyan(top/left)→violet(right)→amber(bottom) cycle, not a cyan band with a violet
whisper. Owner: **athena-gfx** (the per-perimeter hue-position mapping is dropping the
warm half of the cycle).

---

## 4. New overall parity % vs macOS-26 / Win11 — honest

**~58%, up from 35%.** Reasoning, calibrated against measured reference numbers:

What we now have that we didn't (the +23):
- **Luminous aurora backdrop** (peak L171, mean 86.5) — at or above reference mood. **Full credit.**
- **Light-adding frosted glass** (polarity fixed, panel L94–96) — the single biggest identity move, done. **Most credit.**
- **The chromatic edge exists** (13k cyan px where there were 0) — the signature is present. **Partial credit (cyan-only).**
- **Monotonic tier ordering** — cohesion mechanism works on the meter. **Partial credit.**

What still caps us below parity (the −42):
- **Glass brightness is ~52% of true frosted glass.** Gold reference glass interior =
  **L181.6** (rgb 155/188/197, near-white milky frost). Ours = **L94–96**. Even
  granting that ours is the *dark* theme (Tahoe dark glass is legitimately darker
  than the light gold kit), our glass still reads as "tinted translucent blue," not
  "frosted glass that scatters light." macOS Tahoe dark glass lifts its backdrop more
  than we do. Room to push frost +1 step on panel/popover.
- **Rim is cyan-monochrome, not a rainbow sweep** (§3b). The reference's defining
  refraction is multi-hue; ours is one hue.
- **Internal cards are dark athshell slates on luminous glass** (§5 P0). Measured: CC
  tile-grid region **L40.6** (rgb 36/40/60), darkest internal pixel **L25.7** — sitting
  on panel glass at L72–95. This is the *most jarring* remaining thing: a beautiful
  luminous panel with dark-card holes punched in it. The reference has light cards on
  light glass; we have dark cards on light glass — a polarity clash *inside* one
  surface. This alone probably costs ~8–10 parity points because it's the headline
  surface.
- **Controls are flat squares, not accent-glow pills.** The reference's pill buttons
  with colored inner glow are core to the look (§5).
- **Files is the dead twin** — flat navy void (content L18.4, sidebar L17.2, no
  aurora), letter glyphs not icons. Not on the identity render path, so it doesn't
  drag the *identity* score, but it IS a shipped surface a user sees, so it caps the
  *product* parity.

58% is honest: the foundation and two of three signature moves are in; the third
signature (full iridescent sweep) is half-in; and the surface-level finish
(internal cards, pills, Files) is the long tail to parity.

---

## 5. NEXT prioritized defect list — closing the gap to "better than Win11"

Each: **surface → defect (measured) → fix + target value → owner.**

### P0 — the polarity clash that's now the ugliest thing

1. **Control Center internal cards / tiles** → measured **L40.6** (rgb 36/40/60),
   darkest internal pixel **L25.7**, sitting on panel glass at **L72–95** — dark
   athshell slates punched into luminous glass (polarity clash inside one surface).
   → Re-skin internal cards to a **light frosted sub-tier**: composite the same
   `GLASS_FROST_LIGHTEN`-style white-add over the card region so cards read *lighter*
   than the panel, or make them near-transparent so the panel glass shows through.
   Target: internal card interior **L ≥ panel interior** (≥95), never below it.
   → owner: **athena-shell-apps** (the CC render uses a hardcoded dark tile fill, not
   the glass tier system).

2. **Notification toast cards** (`surface-notifications.png`) → toast interior
   **L45.4** (rgb 41/44/72) over aurora at **L137.7** — same dark-slate-on-luminous
   problem as CC. → Repoint toast fill to `glass.popover` tier + frost (IDENTITY §7),
   not a dark fill. Target interior **L ≥ 90**. → owner: **athena-shell-apps**.

### P1 — the signature and tier finish

3. **Iridescent rim is cyan-monochrome** → cyan 13,398 / violet 444 / warm **36**
   across the tiers (gold reference: cyan 6,178 / violet 10,264 / warm 1,139). Warm
   amber stop renders ~0; violet is a whisper. → Fix the per-perimeter hue-position
   map so the bottom/bottom-right edge renders `GLASS_EDGE_WARM 0x40_FFC97C` and the
   right edge renders `GLASS_EDGE_VIOLET 0x40_B47CFF` at full band, producing the
   continuous cyan→violet→amber cycle. Target: each of violet and warm ≥ ~30% of the
   cyan pixel count on a 4-edge panel (a balanced sweep, like the reference).
   → owner: **athena-gfx**.

4. **Panel↔popover tiers visually indistinguishable** → interior luma 94 vs 96
   (Δ2, monotonic on the meter but not to the eye); chrome→panel is Δ29 (good).
   → Widen the popover frost/alpha step so popover reads ~10+ luma above panel.
   Target: chrome < panel < popover with **each step ≥ 8 luma** over a fixed
   backdrop (tighten the `tier_luminance_is_monotonic` KAT from ">" to "≥8 apart").
   → owner: **athena-gfx** (frost step) + **athena-ui** (token + KAT).

5. **Aurora peak slightly hot** → peak **L171** vs target 140–150, and it sits
   directly under the centered panels (worst-case glass-contrast cell). → Trim blue
   blob core weight **138→132** so peak lands ~150–155; re-measure, go to 128 only if
   still >158. → owner: **athena-gfx**; a11y to re-confirm 4.5:1 over the new peak.

### P1 — controls (the reference's core look we don't have)

6. **Controls are flat squares, not accent-glow pills** → CC toggles/buttons render
   as flat fills with no pill radius and no inner accent glow; the reference
   (`Liquid glass guide_...jpg` Primary/Secondary buttons) are `radius_pill` pills
   with a colored inner-glow halo. → Re-skin interactive controls to
   `radius_pill = h/2` + an inner accent-glow (RaeBlue/Vibe accent at low alpha,
   inset) + glossy top highlight. Target: every toggle/button/chip/slider-thumb is a
   pill with accent glow on the "on"/primary state. → owner: **athena-shell-apps**
   (control draw) + **athena-ui** (a reusable pill-with-glow primitive token if not
   already present).

### P2 — Files (flag: critique is moot until live-wire lands)

7. **`surface-files.png` is the dead twin** → content L18.4, sidebar L17.2, flat navy
   void, no aurora, letter glyphs (H/D/d/L/M/P/V/T) not icons. → **This is a known
   capture limitation, not a render regression**: the live `apps/files` preview can't
   be host-wired due to the athkit lang-item conflict (per the task note + MEMORY
   `no-std-workspace-host-test`), so this PNG is still rendering the old dead
   `FileManager` twin, NOT the live Files. **Its identity critique is moot** — do not
   spend gfx/ui cycles re-skinning *this image*. The real fix is upstream: resolve
   the athkit lang-item conflict so the live Files (with ftype-colored icons, already
   committed `3102a46`) can be host-rendered, THEN re-shoot. Until then, mark this
   surface "not identity-verifiable." → owner: **athena-shell-apps** (lang-item
   conflict / live-wire), then re-shoot for a real critique.

### Note for athena-design-researcher

- The 3× rim crop's title text says "TOP-LEFT edge" but the manifest
  (`manifest.txt` line for `glass-iridescent-edge-3x.png`) says "TOP-RIGHT corner."
  Minor, but the proof shot and its manifest disagree on which corner — reconcile so
  the "instantly recognizable" shot is unambiguous, and ideally re-shoot it at a
  corner that shows TWO hues (e.g. the bottom-right, to prove violet→amber in one
  crop) once defect 3 lands.

---

## Reference comparison (named gaps, with measured source)

- **`reference/Liquid glass guide_...jpg` (gold):** glass interior **L181.6** (near-
  white milky frost), rim cyan 6,178 / violet 10,264 / warm 1,139 (full balanced
  sweep), overall mean L173. Our gap: glass **L94–96** (~52% as bright — frost can go
  up one step), rim **cyan-monochrome** (warm ~0). These two are now the bulk of the
  felt difference (down from "everything" in Round 3).
- **`reference/download (1).jpg` (W11 Fluent Files on aurora):** bright flowing blue
  ribbon backdrop + light translucent Files panel + **colorful pill folder chips**.
  Our aurora now matches the backdrop energy (peak L171); our Files is the dead navy
  twin (P2 #7), and we have no pill chips (P1 #6).
- **macOS 26 Tahoe (knowledge):** defined by specular highlight + edge light-bend +
  background luminance lift. We now have the highlight (white top edge, 801 px) AND a
  luminance lift (frost, L94) AND a partial edge light-bend (cyan rim). Remaining gap
  vs Tahoe: the edge bend is single-hue, and Tahoe's dark glass still lifts its
  backdrop more than our L94 does.
- **Win11 24H2 Acrylic/Mica (knowledge):** Acrylic is lighter than its backdrop +
  noisy-frosted; Mica tints toward wallpaper. Our glass is now correctly *lighter*
  than its backdrop (polarity fixed) — we've matched and arguably beaten Mica's
  flatness with the rim. To clear "better than Win11" outright, close P0 #1/#2 (dark
  internal cards) and P1 #6 (pills) — Win11 has no dark-card-on-glass clash and does
  have pill-ish controls.

## Consistency issues

- **Internal-card polarity clash** (P0 #1/#2): dark cards inside luminous panels — the
  one cohesion break that reads as "broken," not "branded." Highest-priority fix.
- **Rim hue not uniform around the perimeter** (P1 #3): cyan-heavy, warm absent — the
  sweep isn't cohesive yet.
- **Tier panel/popover separation** (P1 #4): Δ2 luma — fails the eye test on 2 of 3 tiers.
- Corner radii consistent across panels/toasts (lg ~16) — **no defect.**
- Top-edge white highlight present + consistent (801 px on the crop) — **good, keep.**
- Aurora vignette consistent (corners L30/42) — **good.**

## Blocking (won't render)

None for identity. All five identity surfaces render cleanly via the host
rasterizer. **`surface-files.png` is a known dead-twin capture limitation** (athkit
lang-item conflict blocks host-wiring the live Files) — flagged to athena-shell-apps,
not a critique target until re-wired. No handoff to verifier/debugger needed.

## Confidence

**High** that 3/4 Round-3 defects are fixed and the rim now renders (all measured to
hard numbers: aurora peak 94→171, glass polarity flipped to L94, rim cyan 0→13,398,
tier ordering de-inverted). **High** that the rim is cyan-monochrome and the internal
cards are dark (warm ~0, internal L40.6 — both measured). **Medium** on the exact
parity number (58% is a calibrated estimate against measured reference luma/chroma,
not a metric). **Medium** on the precise trim targets (blue 138→132, popover frost
step ≥8 luma, internal card L≥95) — each needs one host-render iteration to dial in,
as in Round 3.
