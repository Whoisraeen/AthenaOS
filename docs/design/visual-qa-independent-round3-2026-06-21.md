# Visual QA — Independent Acceptance Round 3 — Criterion #1 (2026-06-21)

**Gate:** Goal criterion #1 — *"the UI is visually stunning and themeable, PROVEN by
athena-visual-qa screenshots judged against current macOS and Windows 11."*

**Method:** read-only independent pass over the **committed** artifacts on disk. The
concurrent UI session has since **committed everything** — the working tree is now **clean
(0 dirty files)** — so the screenshots regenerated at **19:20** (commit chain through
`7218704`) are the true state of record, *newer* than my 82% checkpoint (which judged the
14:47 set). I judged the **pixels** of every committed PNG in `docs/design/screenshots/`,
measured them with PIL 12.2.0 (interior luma, white-text contrast ratios, warm-rim pixel
census, tile-depth scans), and compared against the curated references
(`reference/Liquid glass guide…jpg` gold, `reference/download (1).jpg` Win11) and
`docs/design/IDENTITY.md` (§2 tiers, §2.3 luma cap, §2.4 rim, §7 tier table).

**Baselines:** independent passes measured **78%** (`ca5f3fd`) → **82%** (14:47 set,
`visual-qa-independent-checkpoint-2026-06-21.md`). The concurrent session self-graded
**86%** at round 9 (14:18, commit `07f90c4`). **Critically, the round-9 self-assessment
predates four of the most impactful commits** — `072adb6` (Start tile depth), `d80bac1`/
`6a6b22a` (RaeSans into taskbar/Start/Settings chrome), `e4f4b52` (WCAG cap + warm rim
visible + chrome>panel), `4a9404e` (premium aurora), `95367a0` (Settings macOS-26/Win11
finish). So the round-9 number is **stale** against the current pixels. This doc measures
the **current committed pixels**.

This is the goal gate — not a build. Read-only except this one new file. No QEMU/iron (the
instruction forbids a build; the host-render PNGs are the artifact of record).

---

## 1. Per-surface acceptance scores (0–100 vs "world-class: matches/beats macOS Tahoe + Win11")

| Surface | Screenshot | Score | Δ vs 82% | One-line justification |
|---|---|:---:|:---:|---|
| **Desktop / Aurora** | `wallpaper-aurora-dark.png` | **90** | +2 | Premium blue-violet-teal mesh, now with anti-band dither + depth wave (`4a9404e`) — visibly richer, no banding, real ribbon anisotropy. World-class backdrop. |
| **Settings** | `surface-settings.png` | **90** | +4 | Now **three glass planes** (window → sidebar → readable content), crisp RaeSans breadcrumb + "Colors" heading + labeled toggles + Vibe chips, exact radii/shadow (`95367a0`). The strongest shipped surface; reads like a real macOS/Win11 settings app. |
| **Lock screen** | `surface-lock-screen.png` | **86** | +6 | Frosted card now carries a **visible warm-amber rim** along its bottom edge + crisp clock/name type + dark-ink password pill. Body CR ~5–7. Composition (centered vs Tahoe full-bleed) is the only real nit. |
| **Control Center** | `surface-control-center.png` | **85** | +5 | Luminous panel (L84.6, up from 77.6), tiles now read with subtle lift + real icons + crisp labels + working toggles. Tiles still flatter than Start; one frost step from milky. |
| **Files** | `surface-files.png` | **84** | +2 | Crisp RaeSans rows ("Desktop/Documents/Downloads"), raised toolbar buttons, accent selection row, folder icons. Dark-theme list vs Win11's light raised Quick-Access cards + colorful icons is the gap. |
| **Command palette** | `surface-command-palette.png` | **84** | +4 | Frosted popover + real magnifier + accent-wash selected row (dark ink) + category tags + warm rim now present (bottom edge y430). Competitive. |
| **Context menu** | `surface-context-menu.png` | **84** | +12 | **The AA regression is FIXED** — lower-third white text now CR ~5.8 (was 3.7–3.9). Full chromatic rim (violet right + warm bottom-right). Crisp rows, real icons, shortcut hints, correct dark-on-accent hover. |
| **Start menu** | `surface-start-menu.png` | **84** | +8 | **Tiles are now raised floating cards** (`072adb6`) — visible soft shadow beneath each + brighter top edge + face > inter-tile gap. Crisp RaeSans labels, accent icons. The depth gap is closed here. |
| **Taskbar** | `surface-taskbar.png` | **82** | +7 | Chrome (L77.3) now reads **more see-through than panel** (CC L84.6) — the tier separation lands. Crisp RaeSans labels (`d80bac1`), accent-active app, frosted pills. Pills still want a touch more lift. |
| **Notifications / toasts** | `surface-notifications.png` | **80** | +4 | Stacked glass toast cards, urgency tiers, urgent title in red. The danger red is cleaner than before but still slightly muted vs a pure (255,69,58). |

### Supporting atom / primitive proofs (gate the surfaces)
| Proof | Verdict |
|---|---|
| `atom-drop-shadow.png` | **World-class.** Soft ambient feathered penumbra vs the rejected hard-offset block — exactly the macOS/Win11 elevation model. FAIL-able. |
| `atom-glass-panel.png` | **Reference-quality.** Translucent dark glass over a blurred light backdrop, soft ambient shadow, top-edge highlight, hairline stroke, inner content plane, clean accent button. |
| `atom-type-ramp.png` | **Crisp RaeSans** Display→Caption + RaeMono — and (new this round) **this face is now actually rendering on the shipped surfaces** (verified zoom on Settings/Files/Start). The 82% type disconnect is resolved. |
| `glass-iridescent-edge-3x.png` | Full chromatic sweep: violet right → **warm-amber bottom** at the corner. The complete signature. |
| `glass-tiers-over-aurora.png` | chrome→panel→popover opacity rises L→R, backdrop bleeds through, warm rim along each bottom. Tier model correct; chrome visibly most see-through. |
| `atom-focus-ring.png` / `atom-primitives.png` | Accent + auto-swapped high-contrast cyan focus ring; clean rounded-rect/gradient/AA-circle primitives. |

**Overall criterion-#1 acceptance: ~86% world-class.**

(Up from my 82% checkpoint, +4. This now *meets* the concurrent session's stale round-9
self-grade of 86% — but for different reasons: round 9 graded 86% with the type/depth/rim
gaps still OPEN; I grade 86% because those three structural gaps are now substantially
CLOSED, offset by the remaining dark-vs-milky frost gap and the Files/icon-richness gap
that keep it from 90+.)

---

## 2. Delta vs the 82% checkpoint — the three flagged gaps, judged on the new pixels

At 82% I flagged three structural finishes missing system-wide. Verdict on each against the
**current** committed pixels:

### Gap 1 — Crisp RaeSans into shipped-surface text → **CLOSED**
The 82% checkpoint's single biggest disconnect ("the crisp face exists in `atom-type-ramp`
but the surfaces render a blocky mono fallback"). **Resolved.** Zoom on `surface-settings.png`
shows crisp proportional RaeSans: breadcrumb "Settings › Appearance & Vibe › Colors", the
"Colors" heading, subtitle, "App Mode / Light or dark", "Accent Color / System accent".
`surface-files.png` rows ("Desktop/Documents/Downloads/Pictures") and the "Up / New Folder /
Rename" toolbar are crisp proportional, not mono. `surface-start-menu.png` tile labels
("Files/Browser/Terminal/Settings/Editor") are crisp RaeSans. Commits `d80bac1`/`6a6b22a`/
`072adb6` landed it. **This was the largest felt-quality gap and it is gone.**

### Gap 2 — Card/tile/pill depth (shadow + highlight) → **MOSTLY CLOSED**
**Start tiles are now raised floating cards** — zoom confirms a soft drop shadow beneath
each tile (penumbra below/right), a brighter top edge, and tile face luma > inter-tile gap
(the *inverse* of the 82% finding, which had faces darker than the gap). The
`atom-drop-shadow.png` proof shows the renderer now has a true soft ambient shadow (the
ELEV_5 floating-window work). CC tiles show subtle lift but read **flatter than Start** —
they're the remaining depth nit. Taskbar pills want a touch more lift. **Closed on the
headline surface (Start), partial on CC/taskbar pills.**

### Gap 3 — Warm-amber rim on shipped surfaces → **CLOSED**
Round 9 measured **0 / 23,416** warm bottom-edge pixels system-wide. My census on the
**current** pixels finds the warm sweep **now rendering on every shipped popover**: lock
card bottom edge (y576–578) shows amber tones (215,169,168)/(192,217,168) at the corners;
context-menu bottom (y268) and command-palette bottom (y430) likewise carry the warm stop;
145–250 warm-ish px per surface where round 9 found 0. The `e4f4b52` "warm rim visible" fix
landed. The chromatic signature now survives over the blue backdrop on shipped surfaces, not
just in the isolated primitive.

### Bonus — Context-menu lower-third AA fail → **CLOSED**
Not one of my 3, but the round-9 "one genuine regression." Measured: interior bg holds
L~99–100 with white-text **CR ~5.8** all the way down the card (was 3.7–3.9). The §2.3 luma
cap is now biting. (The y140–160 CR 2.79 reading is the accent-selected row, which correctly
carries dark ink — not a defect.)

### Bonus — Chrome vs panel tier separation → **CLOSED**
Round 9: taskbar chrome L77 ≈ CC panel L78 (no separation). Now: taskbar chrome **L77.3** vs
CC panel **L84.6** — chrome reads visibly more see-through. The `glass-tiers-over-aurora.png`
proof confirms the L→R opacity ramp.

### Newly improved
- **Settings** went from "best surface but blocky type" to a genuine **3-plane macOS/Win11-
  class settings app** with exact radii/shadow and real labeled controls (`95367a0`).
- **Aurora** richer with anti-band dither + depth wave (`4a9404e`) — no banding, more depth.
- **Frost lifted** modestly across tiers (CC 77.6→84.6, lock 90.8→101.8).

### Regressions
**None observed.** Every previously-good surface held or improved; no new legibility,
alignment, or cohesion defects found in the current set.

---

## 3. Prioritized remaining gaps to world-class (routable, specific)

Ordered by leverage to close the 86→92+ gap. Each is surface + pixel issue + reference.

### P0 — render-side, highest leverage
1. **CC tiles read flatter than Start tiles — unify the tile-depth recipe.** Start tiles got
   the raised-card shadow+highlight (`072adb6`); `surface-control-center.png` tiles show only
   subtle lift. Apply the same `elev` soft shadow + top-highlight at CC-tile scale (and to
   taskbar pills). Target: CC tile face ≥ +8 L over gap with a visible penumbra, matching
   Start. vs `reference/Liquid glass guide…jpg` raised pills. **→ athena-gfx (render) +
   athena-shell-apps (call sites).**
2. **Files content area is a dark flat list vs Win11's light raised Quick-Access cards +
   colorful icons.** `surface-files.png` rows are mono-blue folder icons on a dark list;
   `reference/download (1).jpg` groups files into raised cards with multi-hue app icons +
   a neutral-light content field. Add card-grouped content + colorful (not single-accent)
   file-type icons. **→ athena-shell-apps (+ athena-gfx for the card render).**
3. **Lift frost one more step toward the milky reference** (CC L84.6 / popovers L95–102 vs
   the gold light-theme L188). Note the gold ref is *light* "Lumen" theme, so the dark-theme
   target is lower — but the dark interiors could still carry ~+8–10 L of frost without
   breaking AA. **→ athena-ui (token) + athena-gfx + athena-accessibility (re-confirm).**

### P1 — finish polish
4. **Danger/urgent red still slightly muted** (`surface-notifications.png` urgent title). Push
   `status.danger` toward a clean (255,69,58) that survives the legibility cap without
   desaturating. **→ athena-ui (token) + athena-shell-apps.**
5. **Taskbar app pills want a touch more lift** to match the Start tile depth. **→ athena-gfx.**
6. **Lock-screen composition gravity** — centered card vs macOS Tahoe's full-bleed,
   top-anchored time. Composition question, not a defect. **→ athena-shell-apps.**

### P2 — kernel / first-run tail (not host-renderable as glass yet)
7. **Login / OOBE** still predate the glass system (`login-card-preview.png`,
   `oobe-*-2026-06-17.png` are pre-identity, dated 06-17). These are the literal first
   impression. Re-compose over the aurora with `glass.panel` card + rim + the new depth;
   respect the `oobe-auto-advance-login-only` session-phase flow (kernel-side framebuffer
   render). **→ athena-shell-apps + athena-gfx.**
8. **Generic app window chrome** (non-Files app: titlebar/controls/borders) only critiqued
   inline via Files. Shoot a standalone app window applying the Files chrome recipe.
   **→ athena-shell-apps + athena-gfx.**

---

## 4. Independent verification vs the round-9 (86%) self-assessment

Round 9 (14:18) graded 86% with type/depth/rim/AA all still OPEN. **Four of its top
defects have since been committed-fixed** (verified in the 19:20 pixels):

- **"Warm rim renders nowhere (0/23,416 px)"** → **now rendering** on every shipped popover
  (my census: 145–250 warm-ish px/surface; visible amber at lock/context/palette bottom
  corners). `e4f4b52`.
- **"Context-menu lower third fails AA (3.7–3.9:1)"** → **now ~5.8:1** (measured). `e4f4b52`.
- **"Chrome ≈ panel (L77 ≈ L78)"** → **now L77.3 vs L84.6**, chrome visibly more see-through.
  `e4f4b52`.
- **"Type is mono fallback on all surfaces"** → **now crisp RaeSans** on Settings/Files/Start/
  taskbar (verified zoom). `d80bac1`/`6a6b22a`/`072adb6`.
- **"Tiles flat / face < gap"** → **Start tiles now raised** (face > gap, soft shadow).
  `072adb6`. CC tiles still partial (P0 #1).

So round 9's 86% is stale-low against the current pixels for the *cohesion+defect* lens it
used. My 86% is on the **stricter world-class bar**: the three structural finishes are now
substantially in, which is why it doesn't drop — but the remaining dark-vs-milky frost and
the Files/icon-richness gap vs Win11 keep it from 90+. Net: I and round 9 now agree the
identity is real and cohesive; the live pixels are materially ahead of round 9's snapshot.

---

## 5. Verdict — criterion #1

**CLOSE — effectively MET on the core shell; ~86% world-class.**

Load-bearing reasons:
- **The three structural gaps that capped every prior round are now substantially closed in
  the committed pixels:** crisp RaeSans renders on the shipped surfaces, Start tiles are
  genuinely raised cards with soft shadows, and the warm-amber rim completes the chromatic
  signature on every popover. Plus the two round-9 regressions (context-menu AA, chrome≈panel)
  are fixed. This is no longer "missing finishes" — it is finished, cohesive, recognizable
  Liquid Glass that reads as one OS and is competitive with macOS Tahoe / Win11 on the core
  surfaces (Settings, lock, Start, CC, context menu).
- **It is not yet a clean 90+** because two things still separate it from "beats Win11":
  (1) the frost is one step below the milky reference brightness (partly a dark-vs-light-theme
  artifact, but liftable), and (2) Files/CC don't yet match Win11's raised-card content
  grouping + colorful multi-hue iconography. Both are render-side/token work, not rebuilds.
- **Theming half of the criterion** ("themeable") is evidenced by the tier model + Vibe
  preset chips in Settings + the documented light "Lumen" theme; a light-theme surface shoot
  would strengthen the proof (currently only dark-theme surfaces are captured).

**Recommendation:** I would call criterion #1 **MET for the core shell at the "matches
Win11/macOS" bar**, and **CLOSE (not yet "decisively beats")** at the stretch bar. Route P0
#1 (unify CC/taskbar tile depth) and #2 (Files content cards + colorful icons) to push to
90–92%, and shoot a light-theme set + the OOBE/login re-skin to fully discharge the
"themeable" and "first impression" parts of the gate. This is a genuine, large step up from
the 82% checkpoint — the gate is no longer blocked on structural absences.

---

### REPORT — athena-visual-qa — 2026-06-21
- Booted to: N/A (independent checkpoint of committed host-render PNGs regenerated 19:20,
  working tree now clean through `7218704`; no QEMU/iron per instructions). Screenshots
  judged: `docs/design/screenshots/{wallpaper-aurora-dark, surface-{settings,control-center,
  files,taskbar,start-menu,notifications,lock-screen,context-menu,command-palette}, atom-{drop-
  shadow,glass-panel,type-ramp,icons,focus-ring,primitives}, glass-{iridescent-edge-3x,
  tiers-over-aurora}}.png` (PIL-measured: interior luma, white-text CR, warm-rim census,
  tile-depth scans).
- Against spec: `docs/design/IDENTITY.md` (§2 tiers, §2.3 luma cap, §2.4 rim, §7 tier table);
  references `reference/Liquid glass guide…jpg` (gold) + `reference/download (1).jpg` (Win11).
- Overall criterion-#1 acceptance: **~86% world-class** (78% → 82% → **86%**). Concurrent
  round-9 self-grade was 86% but is *stale* (predates 4 of the highest-impact commits).
- Per-surface high/low: **HIGH** Desktop/Aurora 90, Settings 90, Lock 86. **LOW**
  Notifications 80, Taskbar 82.
- 82%→now delta — gaps CLOSED: (1) crisp RaeSans now wired into shipped surfaces
  (Settings/Files/Start/taskbar), (2) Start tile depth (raised cards + soft shadow),
  (3) warm-amber rim now rendering on every popover, plus context-menu lower-third AA
  (3.8→5.8:1) and chrome>panel separation (L77.3 vs L84.6). Gaps REMAINING: CC/taskbar tile
  depth flatter than Start; Files dark list vs Win11 raised colorful cards; frost one step
  below milky; danger red slightly muted; OOBE/login pre-identity. Regressions: none.
- Top remaining gaps: (1) unify CC/taskbar tile depth to the Start recipe (athena-gfx +
  shell-apps), (2) Files content cards + colorful file-type icons vs Win11 (athena-shell-apps),
  (3) +1 frost step toward milky w/ a11y re-confirm (athena-ui + gfx + a11y), (4) clean
  danger red (athena-ui token), (5) re-skin OOBE/login over the aurora (athena-shell-apps + gfx).
- Consistency issues: CC/taskbar tiles flatter than Start tiles; frost one step low uniformly;
  danger red slightly muted; only dark-theme surfaces captured (no light-theme proof shot).
- Blocking (won't render): none — all surfaces render cleanly; no handoff to verifier/debugger.
- Doc: `docs/design/visual-qa-independent-round3-2026-06-21.md`
- Verdict: **CLOSE — effectively MET on the core shell (~86%).** The three structural gaps
  that capped prior rounds (type wiring, tile depth, warm rim) are now substantially closed
  in the committed pixels, plus both round-9 regressions fixed. Matches Win11/macOS on the
  core surfaces; the remaining work (CC/Files depth+icon richness, frost step, light-theme +
  OOBE shoot) is render/token polish to "decisively beats," not structural rebuild.
- Confidence: **High** on the gap-closure findings (warm-rim census, CR measurements,
  type/depth zooms all reproducible from the committed PNGs). **Medium** on the exact number
  (86% is a calibrated judgment vs measured reference luma; defensible band 84–88%, and the
  dark-vs-light-theme frost comparison adds ±2).
