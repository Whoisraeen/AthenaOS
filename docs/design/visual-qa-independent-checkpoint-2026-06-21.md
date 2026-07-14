# Visual QA — Independent Acceptance Checkpoint — Criterion #1 (2026-06-21)

**Gate:** Goal criterion #1 — *"the UI is visually stunning and themeable, PROVEN by
raeen-visual-qa screenshots judged against current macOS and Windows 11."*

**Method:** read-only independent pass over the **committed** artifacts on disk (NO build —
a concurrent session has ~38 dirty files mid-iteration). I looked at every committed PNG in
`docs/design/screenshots/` (regenerated 14:47, *after* the round docs), the curated
references in `docs/design/reference/`, the spec `docs/design/IDENTITY.md`, and the
concurrent session's self-assessment `visual-qa-round8/round9-2026-06-21.md`. I judged the
**pixels**, then checked them against the round-9 self-claims.

**Baseline:** my last independent pass measured **78% world-class** (commit `ca5f3fd`).
The concurrent session has since landed Rounds 7–9 (real icons, Settings pane, glassified
taskbar/Start/lock/context-menu/command-palette, WCAG glass cap, warm rim primitive). The
concurrent session's own number is **86%** (round 9). This doc is the independent re-measure.

This is the goal gate — not a build. Read-only except this one new file.

---

## 1. Per-surface acceptance scores (0–100 vs "world-class: matches/beats macOS Tahoe + Win11")

Each score is *my* judgment of the committed pixels against the gold reference
(`reference/Liquid glass guide…jpg`, interior ~L188 milky frost, raised pills w/ shadow +
bright top highlight + full chromatic rim) and the Win11 Files reference
(`reference/download (1).jpg`, raised cards w/ depth, crisp SF-class type, colorful icons).

| Surface | Screenshot | Score | One-line justification |
|---|---|:---:|---|
| **Desktop / Aurora wallpaper** | `wallpaper-aurora-dark.png` | **88** | Calm, premium blue-violet-teal mesh. The "void" is dead. Could carry more NW→SE ribbon anisotropy vs `download(1).jpg`, but this genuinely reads as a designed backdrop. |
| **Settings** | `surface-settings.png` | **86** | The strongest shipped surface: frosted sidebar, de-tinted *readable* content area, accent on the selected nav row, Vibe preset chips with real icons. Reads like a real OS settings app. Type is the only big tell. |
| **Files** | `surface-files.png` | **82** | Real window chrome: frosted titlebar + toolbar + sidebar + selection row, backdrop bleeding through. Beats stock Linux file managers. Flat list rows + blocky type + faintly-cool content field keep it off Win11's bar. |
| **Control Center** | `surface-control-center.png` | **80** | Luminous panel glass over aurora, tile grid, sliders, media card, expanded Wi-Fi. The headline before/after win. Tiles are flat (no lift) and frost is one step dark vs the gold milky white. |
| **Lock screen** | `surface-lock-screen.png` | **80** | Centered frosted popover card: clock, avatar monogram, name, password pill. Clean and ship-worthy. Reads closer to a generic login dialog than Tahoe's full-bleed top-anchored-time lock gravity; top band marginal contrast. |
| **Command palette** | `surface-command-palette.png` | **80** | Frosted popover, real magnifier icon (the R8 `?` placeholder is gone), accent-wash selected row w/ dark ink, category tags. Competitive; secondary/path text legibility over bright zones unverified; warm rim weak. |
| **Start menu** | `surface-start-menu.png` | **76** | Frosted flyout, app tiles as frosted cards, accent selection w/ dark ink. But tiles are **flat/recessed** (face darker than the inter-tile gap) — the opposite of the Win11 Aero reference's raised chips. Depth is the gap. |
| **Notifications / toasts** | `surface-notifications.png` | **76** | Stacked glass toast cards w/ urgency tiers, correct popover opacity. The urgent/danger accent reads a dull maroon, not a clean danger red (token-blocked). |
| **Taskbar** | `surface-taskbar.png` | **75** | Frosted chrome bar, frosted app pills, accent-filled active app w/ dark ink, tinted tray. But the chrome tier is **not visibly more see-through than panel** (L77≈L78) — the "floats on the wallpaper" intent doesn't land; pills are flat. |
| **Context menu** | `surface-context-menu.png` | **72** | Top 2/3 is excellent (real icons, shortcut hints, accent-hover row w/ dark ink, separators). **Lower third drops below AA** (white text ~3.7–3.9:1 over a bright aurora teal bleed) — a genuine legibility regression. |

### Supporting atom/primitive proofs (not surfaces, but they gate the surfaces)
| Proof | Verdict |
|---|---|
| `atom-icons.png` | **Excellent.** Crisp vector line-icon set, scales clean to 72px, tints to any token color. This is world-class and is wired into the surfaces. |
| `atom-type-ramp.png` | **Crisp RaeSans** — Display/Title/Subtitle/Body all clean and well-hierarchized. **BUT this crisp face is NOT what renders on the shipped surfaces** (they show a blocky fallback). The good type exists; it isn't wired into the surface render path. This is the single biggest disconnect. |
| `glass-iridescent-edge-3x.png` | The rim primitive **does** render the warm-amber bottom stop + violet right stop (visible at the BR corner). The signature works *in the primitive*. |
| `glass-tiers-over-aurora.png` | Three tiers read correctly (chrome→panel→popover opacity rises L→R), backdrop bleeds through, and warm-amber rim is visible along each panel bottom. |

**Overall criterion-#1 acceptance: ~82% world-class.**

(Up from my 78% baseline. I land 4 points below the concurrent session's 86% — see §4 for
why: they measure favorably on the rim and discount the type disconnect, both of which I read
harder against the "matches/beats macOS+Win11" bar.)

---

## 2. Delta vs the 78% baseline — what genuinely improved

**Genuinely improved (verified in pixels), worth the +4:**

1. **The void is dead (biggest single win).** `wallpaper-aurora-dark.png` is a real designed
   backdrop, not the flat navy `0x0A0E1A→0x1A2844`. Glass now has something to refract. This
   was the #1 root-cause defect in IDENTITY §0 and it is resolved.
2. **Breadth: every host-renderable surface is now glassed and coherent.** At 78% several
   surfaces (lock, context menu, command palette, taskbar, Start) were either un-glassed,
   opaque-navy, or not critiquable. All 9 now carry the same tiered glass + aurora + accent.
   The set reads as **one OS** — that cohesion is real and is the headline of the round.
3. **Settings went from absent/weak to the best surface** — de-tinted readable content area is
   the right move and beats a naive "dark glass box."
4. **Real icons everywhere** (`atom-icons.png` wired into surfaces) — retired the letter/`?`
   placeholders. Crisp, scalable, token-tinted. World-class.
5. **Accent discipline held** — RaeBlue is the only accent, selection/hover rows carry correct
   dark-on-accent ink, no surface drowns labels in blue (IDENTITY §4).
6. **The iridescent rim primitive now renders all three stops** including the warm-amber
   (visible in `glass-iridescent-edge-3x.png` + `glass-tiers-over-aurora.png`).

**Still short of world-class (the load-bearing reasons it's 82 not 92):**

- **Frost is ~55–60% of reference brightness.** Every interior reads navy-dark next to the
  gold reference's milky near-white (L~104–112 vs L188). This is the dominant "ours looks
  dimmer/heavier" tell across all surfaces.
- **No card/tile depth anywhere.** Start tiles, CC tiles, taskbar pills, toast cards, Files
  list rows are all flat. Every reference (gold + Win11 Aero + macOS Tahoe) has *raised*
  controls with a real soft shadow + bright top highlight. This is the biggest "Win11-tier"
  tell still missing.
- **Type disconnect.** The crisp RaeSans exists (`atom-type-ramp.png`) but the shipped
  surfaces render a blocky fallback. SF Pro / Segoe Variable crispness is the largest *felt*
  quality gap, and the fix already exists in the codebase — it just isn't wired to the
  surface text path.
- **Warm rim renders in the primitive but not on the shipped surfaces** (round 9's 0/23,416
  census over taskbar/CC/Start is consistent with my read — the warm stop is swamped by the
  additive blend over the blue backdrop on the real surfaces, even though the isolated proof
  shows it). The signature is incomplete *where users actually see it*.
- **Two legibility soft spots** (context-menu lower third, lock-card top band) sit under AA.

---

## 3. Prioritized remaining gaps to world-class (routable, specific)

Ordered by leverage to close the 82→92+ gap. Each is surface + pixel issue + reference.

### P0 — highest leverage, render-side (no token bake needed)
1. **Wire the crisp RaeSans into the shipped-surface text path.** `atom-type-ramp.png` proves
   the face is crisp; every shipped surface (`surface-*.png`) renders a blocky mono fallback
   instead. vs `download(1).jpg` SF-class type this is the largest felt gap. **→ raeen-ui /
   raeen-shell-apps** (route the surface text rasterizer to the RaeSans atom path).
2. **Add card/tile depth: `elev.1/2` soft shadow + 1px `stroke.strong` top highlight at tile
   scale.** Start tiles (`surface-start-menu.png`) have faces *darker* than the inter-tile gap
   — the inverse of `reference/Aero Start Menu…jpg` raised chips. Also CC tiles, taskbar
   pills, toast cards. Target: tile face ≥ +8 L over gap with a visible penumbra. **→
   raeen-gfx** (render) + **raeen-shell-apps** (call sites).
3. **Complete the warm-amber rim on the shipped surfaces.** Primitive renders it
   (`glass-iridescent-edge-3x.png`) but the shipped surfaces show ~0 warm bottom-edge px — the
   additive blend over a blue backdrop swamps the warm hue. Make the bottom-arc warm stop
   survive over blue. Target ≥2% warm px on each surface's bottom edge, visible amber at BR
   corner. **→ raeen-gfx.**
4. **Fix context-menu lower-third legibility.** `surface-context-menu.png` rows over the bright
   teal aurora bleed push white text to ~3.7–3.9:1 (AA FAIL). The §2.3/§9 luma cap isn't
   biting on the popover tier over the bright bleed. Clamp the composited popover interior to
   `GLASS_INTERIOR_LUMA_CEIL 0.40` there. **→ raeen-gfx** + **raeen-accessibility** (confirm).
5. **Make chrome visibly more see-through than panel.** Taskbar (chrome) L77 ≈ CC (panel) L78;
   per IDENTITY §2.1 chrome is 25% vs panel 45%. The "floats on the wallpaper" intent
   (`surface-taskbar.png`) doesn't land. Drop chrome effective alpha / lift backdrop bleed,
   ≥+8 backdrop-bleed delta vs panel. **→ raeen-gfx.**

### P1 — token-blocked (held by the concurrent session's dirty `rae_tokens`)
6. **+1 frost step per tier** to lift interiors toward the gold L188 milky frost (currently
   ~55–60%), with an a11y re-confirm so white text still clears AA. The dominant "looks dimmer
   than the reference" gap. **→ raeen-ui (token) + raeen-gfx + raeen-accessibility.**
7. **Clean `status.danger` red** (~(255,69,58)) — notifications/taskbar urgent currently reads
   a dull maroon (`surface-notifications.png`). **→ raeen-ui (token) + raeen-shell-apps.**
8. **Neutral `bg.content`** (~L57, ±3 channels) — Settings/Files content fields read faintly
   cool (blue-biased). **→ raeen-ui.**

### P2 — kernel/risky tail (not yet glassed)
9. **Login / OOBE** still predate the glass system (`login-card-preview.png`,
   `oobe-*-2026-06-17.png`). Re-compose over the aurora with `glass.panel` card + rim + depth.
   These are the literal first impression. **→ raeen-shell-apps + raeen-gfx** (kernel-side
   framebuffer render; respect the `oobe-auto-advance-login-only` session-phase flow).
10. **Lock-screen gravity** — `surface-lock-screen.png` is a centered card; macOS Tahoe lock is
    full-bleed with top-anchored time. Composition question, not a defect. **→ raeen-shell-apps.**

---

## 4. Independent verification of the round-9 self-assessment (86%)

I checked the concurrent session's round-9 claims against the pixels:

- **"Every surface glassed / reads as one OS" (86% headline):** CONFIRMED. Verified in all 9
  PNGs. The cohesion is real.
- **"Warm rim renders nowhere (0/23,416 px)":** CONFIRMED for the *shipped surfaces*, but
  **the rim PRIMITIVE does render warm** (`glass-iridescent-edge-3x.png` BR corner +
  `glass-tiers-over-aurora.png` panel bottoms). So the defect is "warm stop swamped on real
  surfaces," not "primitive broken" — a narrower, more fixable framing than the round doc's
  phrasing implies. Net: agree it's the top consistency defect.
- **"Context-menu lower third fails AA":** CONFIRMED — visible lower-contrast rows over the
  bright bleed in `surface-context-menu.png`.
- **"Chrome ≈ panel":** CONFIRMED — taskbar reads as opaque as CC.
- **"Type is mono fallback on all surfaces":** CONFIRMED, and I weight it *harder* than the
  round doc: the crisp face already exists in `atom-type-ramp.png`, so this is a wiring gap,
  not a missing capability — it's both the biggest felt gap AND a P0 doable-now, not a
  token-blocked P1 as round 9 files it.

**Why I land at 82%, not 86%:** the bar is "matches/beats macOS Tahoe + Win11." Against the
gold reference's milky brightness + universal control depth + crisp type, three structural
finishes (frost step, depth, type-wiring) are still absent *system-wide* — those are not 9
points of polish, they're ~15–18 points of the "stunning" delta because they hit every
surface at once. The concurrent session's 86% is a defensible self-grade on *cohesion*; my
82% is a stricter grade on the *world-class* bar. We agree on the gap list; we disagree only on
the number, by 4 points.

---

## 5. Verdict — criterion #1

**CLOSE — NOT YET MET. ~82% world-class.**

Load-bearing reasons:
- **What's there is genuinely good and cohesive** — the aurora kills the void, all 9 surfaces
  belong to one system, Settings/icons are near-reference, accent discipline is clean. This is
  no longer "basic"; it is a real, recognizable identity. That's why it's CLOSE, not NOT-YET.
- **But it does not yet beat or match Win11+macOS** because three *structural* finishes are
  missing on every surface at once: (1) frost is ~55–60% of the reference's milky brightness,
  (2) zero control/tile/card depth vs universally-raised reference controls, (3) the crisp
  type exists but isn't wired to the surfaces. Plus the warm-rim signature doesn't survive on
  shipped surfaces, and two spots sit under AA.
- **The good news for the gate:** the top three gaps are mostly render-side wiring of things
  that already exist (RaeSans is rendered, the rim primitive renders warm, depth is a
  shadow+highlight the renderer already has). The frost step is one token bake. None of this is
  a structural rebuild. I'd expect one focused session (P0 1–5 + P1 6) to land 90–92% and put
  criterion #1 at MET.

**Recommendation:** do not mark criterion #1 done. Route P0 #1 (type wiring) and #2 (depth)
first — they are the two biggest "matches Win11/macOS" tells and both are doable-now. Re-shoot
and re-checkpoint after.

---

### REPORT — raeen-visual-qa — 2026-06-21
- Booted to: N/A (checkpoint of committed host-render PNGs; no QEMU/iron — concurrent session
  mid-iteration, build not attempted per instructions). Screenshots judged:
  `docs/design/screenshots/{wallpaper-aurora-dark, surface-control-center, surface-settings,
  surface-files, surface-taskbar, surface-start-menu, surface-notifications, surface-lock-screen,
  surface-context-menu, surface-command-palette, glass-iridescent-edge-3x, glass-tiers-over-aurora,
  atom-icons, atom-type-ramp}.png`
- Against spec: `docs/design/IDENTITY.md` (§0 verdict, §2 tiers, §2.4 rim, §3 aurora, §7 tier
  table), references `reference/Liquid glass guide…jpg` + `reference/download (1).jpg`.
- Overall criterion-#1 acceptance: **~82% world-class** (baseline was 78%; concurrent session
  self-graded 86%).
- Per-surface high/low: **HIGH** Desktop/Aurora 88, Settings 86. **LOW** Taskbar 75, Context
  menu 72.
- Top remaining gaps: (1) wire crisp RaeSans into shipped-surface text (raeen-ui/shell-apps),
  (2) add card/tile/pill depth — soft shadow + top highlight (raeen-gfx + shell-apps),
  (3) complete the warm-amber rim on shipped surfaces (raeen-gfx), (4) fix context-menu
  lower-third AA fail (raeen-gfx + a11y), (5) chrome must read more see-through than panel
  (raeen-gfx); then (6) +1 frost step toward the milky reference (token-blocked, raeen-ui).
- Consistency issues: warm rim absent on shipped surfaces; chrome≈panel; tile/card depth absent
  shell-wide; frost one step low uniformly; type mono not RaeSans uniformly; danger red maroon.
- Blocking (won't render): none — all 9 surfaces render cleanly; no handoff to verifier/debugger.
- Doc: `docs/design/visual-qa-independent-checkpoint-2026-06-21.md`
- Verdict: **CLOSE — NOT YET MET (~82%).** Cohesive and recognizable, but three structural
  finishes (frost brightness, control depth, type wiring) are absent system-wide vs the
  macOS+Win11 bar. All top gaps are render-side wiring of capabilities that already exist —
  one focused session likely lands MET.
- Confidence: **High** on the gap list and the per-surface reads (every claim verified in the
  committed pixels). **Medium** on the exact number (82% is a calibrated judgment vs measured
  reference luma, defensible band 80–84%).
