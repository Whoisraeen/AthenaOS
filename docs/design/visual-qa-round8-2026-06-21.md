# Visual QA — Round 8 — measured re-critique after the Round-7 fixes (2026-06-21)

Static-pixel re-measure of the regenerated identity surfaces (commits `e3c477f` icons +
Settings pane, `9809331` chrome floor + Start frost chroma) in
`docs/design/screenshots/`, against `docs/design/IDENTITY.md`,
`docs/design/material-and-shadow.md`, the curated references in `docs/design/reference/`,
and macOS 26 Tahoe / Windows 11 24H2 from knowledge.

Measured with PIL 12.2.0 (Python 3.10). Every number below is reproducible from the
committed PNGs (1280×800 each). Follow-up to Round 6 (74%) / Round 7 (77%). Round-7
projected ~85% once the three "looks unfinished" tells were fixed — this verifies in the
pixels.

---

## Verdict up front

**Parity vs macOS 26 Tahoe / Win11 24H2: ~84%** (was 77% at Round 7, 74% at Round 6).

All four Round-7 claims are **confirmed in the pixels**. The three "looks unfinished"
tells — letter/`?`-placeholder icons, a near-black Settings void, an invisible taskbar —
are dead. The product now reads as one coherent Liquid Glass OS across desktop, CC,
notifications, Files, taskbar, Start, **and** Settings — the first round where every
shipped surface in the set carries the identity. The +7 is real and earned almost entirely
by Settings going from a void to a populated, macOS-System-Settings-class pane and by the
shell chrome (taskbar/Start) finally reading as frosted glass instead of vanishing.

What now caps parity is a short, well-understood tail, and **three of the highest-leverage
items are token-blocked** by a concurrent session that holds `components/ath_tokens/src/lib.rs`
(confirmed dirty in the working tree): the danger-red ink, the RaeSans chrome font, and the
`bg.content` token. The non-token items — Start tile-card lift, and the Phase-2 surfaces
(lock/login/window-chrome) that have never been glassed — are doable now.

---

## Round-7 fix confirmation (the four claims)

### 1. Icons — REAL line icons, no `?`/letter placeholders → **CONFIRMED** (one residual)

3×/2× zoom crops (`/tmp/set_nav_3x.png`, `/tmp/start_tiles_2x.png`, `/tmp/taskbar_2x.png`):

- **`surface-settings.png` — 10 nav rows:** all real line icons. Clearly legible at 3×:
  Appearance & Vibe (color-wheel), Display (sun/brightness), Sound (speaker+waves), Network
  (wifi arcs), Bluetooth & Devices (the bluetooth rune), Power & Gaming (gamepad),
  Accessibility (person-in-circle), Privacy & Security (lock/shield), plus Storage / System.
  No letters, no `?`. **Pass.**
- **`surface-start-menu.png` — 6 pinned tiles:** Files (folder), Browser (globe/eye),
  Terminal (window), Settings (gear), Editor (document), RaeGames (controller). Real icons,
  no letters. Recents/Recommended row present. **Pass.**
- **`surface-taskbar.png` — app pills + tray:** Files (folder), RaeBrowser (wifi/globe),
  Terminal (prompt window), Messages (document) — real icons; tray shows the `12:00` clock.
  **Pass.**
- **RESIDUAL — search-field magnifier is still a `?` placeholder.** The Settings search box
  reads `? Search settings` (the magnifier glyph slot renders the missing-glyph `?`). Same
  `?` is likely on the Start search field. Small but it's a literal placeholder in the most
  "is this finished?" spot. **Defect carried to the list (P1 #4).** → owner: **athena-shell-apps**.

### 2. Settings content pane — populated, readable, macOS-class → **CONFIRMED**

Measured: content pane empty bg **L61–68** (claimed ~57; it's even a touch brighter),
rgb (56,60,78)→(62,68,87), min 27 / **max 242** (rim + control specular present). The
Round-7 luma-22 void is dead. The pane (`/tmp/set_content_2x.png`) lands on a real
**Appearance & Vibe** detail: a "Colors / Accent color, dark/light mode, transparency"
header, four toggle/selector rows (App Mode = Dark dropdown, Accent Color, Transparency,
Title Bar Accent — pill toggles drawn), an **Accent ramp** of 6 swatch chips, and a
**3×3 grid of 9 Vibe preset tiles** (Cyberpunk Night, Ghibli Morning, Bauhaus, Neo Noir,
Nordic Frost, RetroWave, Minimal Zen, Forest Dusk, Ocean Breeze), each with a 3-swatch
color cluster + label. **This reads like macOS System Settings** — left rail of icon'd
categories, a titled detail pane with grouped toggle rows on the right. **Pass.**

One note: the content bg is faintly cool (B +18 over R, like Files was) and ~L61 is a dark
neutral, fine for a dark theme; a `bg.content` token (~L57, near-neutral) is the follow-up
(P1 #5, token-blocked).

### 3. Taskbar chrome floor — consistent frosted dock → **CONFIRMED**

The bar is now a real surface across its whole width (top rim highlight at y740–746
L96–106; interior y750–790). Floor test (empty bar segment vs wallpaper directly above, at
matched x):

| Position | wp above (L) | bar (L) | delta |
|---|---:|---:|---:|
| left edge (over **dark** wp) | 19.4 | 39.8 | **+20.4** |
| far-right tray gap | 47.2 | 49.6 | +2.4 |
| right-empty (over **bright** wp) | 60.8 | 43.5 | **−17.4** |

So the bar reads **+20 brighter over dark / −17 darker over bright** — a constant ~L40–50
frosted floor that holds regardless of the aurora beneath it, exactly the claim (was 3–6
delta = invisible at Round 7). It now reads as a single dock-like bar, not a window-cut.
**Pass.**

### 4. Start frost chroma — aurora tint bleeds through, +chroma → **CONFIRMED**

Start popover interior measures **chroma 54–90** (max−min channel spread) at L100–106 —
the backdrop's blue/violet aurora survives the frost (rgb (81,104,136) mid, (80,106,170)
lower). It is **not** flat grey; the tint reads through. Claim of +36% chroma → ~50 is met
and exceeded in the lower body. **Pass.**

---

## Per-surface parity scan (the whole set)

| Surface | key measure | reads as | verdict |
|---|---|---|---|
| Desktop / aurora | peak L148.5 (in 140–150 band) | calm procedural Aurora Mesh | strong |
| Control Center | panel interior L90, rgb (83,87,122) | luminous tiered glass, tile grid | strong |
| Notifications | toast cards L60–95, stacked | frosted toast stack | good (urgent red weak) |
| Files | chrome L57–81, RaeBlue selection | glass app window, Finder-like | strong |
| Taskbar | constant floor L40–50, real pills | frosted dock | **now competes** |
| Start | popover L100–106 chroma 54–90 | frosted flyout, real tiles | good (tiles flat) |
| Settings | pane L61–68 populated, 10 icon'd nav | macOS System Settings | **now competes** |

Every shipped surface in the set now carries the glass identity. That cohesion is the story
of this round.

---

## NEW overall parity % — honest

**~84%, up from 77%.**

The +7 breaks down: Settings void→populated-and-icon'd is the largest single move (a
flagship settings surface going from "obviously unfinished" to "reads like macOS System
Settings" is worth ~4); taskbar floor invisible→constant-frosted-dock (~2, it's the
single most-on-screen surface); Start frost grey→chromatic + real-icon tiles (~1).

Calibrated against the gold reference (`reference/Liquid glass guide…jpg`: full-image mean
**L172.9**, glass interior **L188**).

**What caps us below parity (the −16):**

- **Glass still ~55–60% of reference frost brightness** — gold interior **L188**; ours CC
  L90 / Start L100–106 / Files chrome L57–81 / Settings L61. Even granting dark theme, chrome
  bands have room for +1 frost step. **~5 pts.**
- **No depth/lift on tiles & cards** — Start tiles measure L132 face vs L138 gap = **−6
  (inverted: tiles are flatter/darker than the gaps between them)**; no card shadow or
  top-highlight separates a tile from the popover. macOS/Win11 tiles read as raised chips.
  This is the `material-and-shadow.md` `elev.*` ladder not reaching tile-scale surfaces.
  **~4 pts.**
- **Danger/urgent red is a dull maroon** — reddest danger px in notifications is (122,45,69);
  the taskbar Messages "urgent" pill reads desaturated maroon, no clean red. Should be a
  saturated danger red (~(255,69,58)). **Token-blocked. ~2 pts.**
- **Chrome type is a blocky mono/placeholder font, not RaeSans** — every chrome label (RaeOS,
  Files, RaeBrowser, Terminal, Settings nav, Vibe tile labels) renders in a heavy monospace,
  not a refined UI sans. This is a large felt-quality gap vs SF/Segoe. **Token/font-blocked.
  ~3 pts.**
- **Phase-2 surfaces never glassed/shot** — lock screen, login, window chrome (titlebar/
  traffic-lights on a generic app window), context menus. Half the felt journey
  (you log in before you ever see the desktop). **~2 pts.**

84% is honest: every surface we ship now competes, and the remaining distance is finish
(frost step, depth, type) plus the unbuilt first-run surfaces — not a structural break.

---

## Prioritized REMAINING defect list → 90%+

Each: **surface → defect (measured) → fix + target → owner → [blocked?]**

### P0 — the two finish moves that read instantly as "premium"

1. **Start tiles (and CC tiles) have no card lift — they read flat/recessed.**
   Start tile face **L132 vs inter-tile gap L138 = −6** (tiles darker than the gap; the
   opposite of a raised card). → Give each tile an `elev.1`/`elev.2` soft shadow + 1px
   `stroke.strong` top-edge highlight so it lifts off the popover (the
   `material-and-shadow.md` contract, applied at tile scale). **Target:** tile face ≥ +8 L
   over the inter-tile gap, with a visible soft penumbra below each tile. → owner:
   **athena-gfx** (shadow/highlight on tile surfaces) + **athena-shell-apps** (call sites).
   **Doable now** (compositor/render-side; no token needed). *Highest leverage — depth is
   the single biggest "Win11-tier" tell still missing.*

2. **Chrome frost ~55–60% of reference brightness.** CC L90 / Start L100 / Files chrome
   L57–81 / Settings L61 vs gold interior **L188**. → Push chrome-band frost +1 white-add
   step. **Target:** chrome bands ≥L90, popover/panel interiors ≥L115, while a11y re-confirms
   text still clears 4.5:1. → owner: **athena-gfx** (frost step) + **athena-ui** (token) +
   **athena-accessibility**. **Token-blocked** for the token bake; athena-gfx can prototype the
   render-side lift now and bake when `ath_tokens` frees.

### P1 — the type + color tokens (mostly token-blocked)

3. **Urgent/danger red is a dull maroon, not a danger red.** Reddest danger px (122,45,69);
   taskbar Messages pill desaturated maroon. → Introduce/route a `status.danger` ink (~RaeRed
   (255,69,58)) for urgent toasts' accent strip and urgent taskbar pills; ensure it survives
   the glass legibility cap without washing to maroon. **Target:** urgent accent rgb R≥200,
   R−G≥120, R−B≥120. → owner: **athena-ui** (token) + **athena-shell-apps** (apply) +
   **athena-gfx** (cap must not desaturate the danger hue). **TOKEN-BLOCKED** (needs a new
   `status.danger` in `ath_tokens`, currently held).

4. **`?` placeholder glyph in search fields.** Settings (`? Search settings`) and likely Start
   render the missing-glyph `?` where the magnifier icon belongs. → Wire the real
   `athgfx::icon` magnifier (same system that fixed the nav rows). **Target:** no `?` anywhere
   in the chrome. → owner: **athena-shell-apps**. **Doable now** (the icon exists; just unwired
   in the search field).

5. **Chrome type is blocky monospace, not RaeSans.** Every chrome label renders in a heavy
   mono — the largest felt-quality gap vs SF Pro / Segoe Variable. → Ship a real RaeSans UI
   font for chrome labels (the `atom-type-ramp.png` shows athfont can render crisp grayscale
   AA; the chrome call sites need to use the sans ramp, not the mono fallback). **Target:**
   chrome labels in RaeSans at the documented type scale/weights. → owner: **athena-ui**
   (font/token wiring) + **athena-shell-apps** (call sites). **PARTLY TOKEN/FONT-BLOCKED** —
   the typeface plumbing touches the shared type tokens in `ath_tokens`.

6. **`bg.content` token (Settings/Files content fields faintly cool).** Settings pane B +18
   over R; Files content B +7 over R. → Add a near-neutral `bg.content` (~L57, rgb within ±3).
   **Target:** content rgb within ±3 across channels. → owner: **athena-ui**. **TOKEN-BLOCKED**.

### P2 — the Phase-2 extraction surfaces (never glassed or shot)

These are doable now (no token dependency for the structure) and close the first-run journey.
Each needs a host-render shot first, then the same glass-chrome + rim + the new tile-lift.

7. **Lock screen** — first thing a user sees; never shot over the aurora with the glass card.
   → `glass.panel`/card over aurora, clock, auth field with the new frost + rim. → owner:
   **athena-shell-apps** (compose) + **athena-gfx** (glass). **Doable now.**

8. **Login screen** — `login-card-preview.png` exists but predates the glass system. → Re-shoot
   over the aurora with the glass card + rim + tile-lift. → owner: **athena-shell-apps**.
   **Doable now.**

9. **Window chrome** — a generic app window's titlebar / traffic-lights / borders have not been
   critiqued as a standalone surface (only Files inline). → Shoot a non-Files app window; apply
   the Files chrome recipe (frosted titlebar ≥ toolbar, full-perimeter rim, traffic-light
   controls). → owner: **athena-shell-apps** + **athena-gfx**. **Doable now.**

10. **Context menus** — the small-surface tell separating polished from basic; never shot.
    → `glass.popover` + rim. → owner: **athena-shell-apps** + **athena-gfx**. **Doable now.**

---

## Reference comparison (named gaps, measured source)

- **`reference/Liquid glass guide…jpg` (gold):** full-image mean **L172.9**, glass interior
  **L188** (near-white milky frost), tiles/buttons are **raised pills with a clear drop
  shadow + bright top-edge highlight** (the depth our Start tiles lack: their L132<L138 gap
  means no lift). Our gaps: frost L57–106 (~55–60% as bright, P0 #2); **no tile depth**
  (P0 #1). The reference's whole signature is depth + milky brightness — that's our two P0s.
- **macOS 26 Tahoe (knowledge):** specular + edge light-bend + background luminance lift +
  glass-chromed windows — we have all four on shell *and* Settings/Files now. Remaining vs
  Tahoe: (a) Tahoe's controls/tiles cast real soft shadows (our flat tiles, P0 #1); (b)
  Tahoe's chrome type is SF Pro (our blocky mono, P1 #5); (c) Tahoe shadow-scrims white text
  on colored toggles. The Settings void that put us behind Tahoe at R7 is **closed** — our
  Settings now matches Tahoe's System Settings structure (icon'd rail + titled toggle pane).
- **`reference/Aero Start Menu…jpg` (Win11 glass Start):** the reference Start tiles are
  **raised glass chips with visible separation + lift** over a translucent flyout. Ours is a
  chromatic frosted flyout now (good) but the **tiles are flat** (P0 #1) — that's the
  remaining gap vs this exact reference. Our flyout frost chroma already beats the reference's
  flatter Acrylic; with tile-lift + the frost step, Start clears "better than Win11."

## Consistency issues

- **Tile depth absent shell-wide** (P0 #1) — Start tiles flat/recessed (L132<L138 gap); CC
  tiles likewise have no lift. The `elev.*` ladder isn't reaching tile-scale surfaces.
- **Frost brightness one step low** (P0 #2) — consistent across CC/Start/Settings/Files chrome.
- **Danger red desaturated** (P1 #3) — maroon, not red; inconsistent with a clear status palette.
- **Chrome type mono, not sans** (P1 #5) — uniform across all chrome (consistently wrong).
- **`?` placeholder in search fields** (P1 #4) — Settings + Start.
- **Content fields faintly cool** (P1 #6) — Settings B+18, Files B+7; no neutral `bg.content`.
- **RESOLVED:** icon placeholders (nav/tiles/pills now real icons), Settings void (now L61
  populated), taskbar invisibility (now constant L40–50 floor), Start frost grey (now chroma
  54–90). Corner radii consistent (lg ~16) across all surfaces — no defect.

## Blocking (won't render)

None. All surfaces render cleanly via the host rasterizer. No handoff to verifier/debugger.

## Token-block status (for sequencing the next session)

`components/ath_tokens/src/lib.rs` is **dirty in the working tree** (concurrent session).
`components/athshell/src/control_center.rs` is also mid-edit. **Token-blocked** items:
P0 #2 frost-step token bake, P1 #3 `status.danger`, P1 #5 RaeSans type tokens, P1 #6
`bg.content`. **Doable now (no token):** P0 #1 tile-lift (render-side shadow/highlight), P1
#4 wire the magnifier icon, all of P2 (#7–10 shoot + glass the lock/login/window/menu
surfaces). **Highest-leverage doable-now move: P0 #1 tile-lift** — it's the depth cue that
most reads as "Win11-tier," and it needs no token.

## Confidence

**High** on all four Round-7 fixes (each measured: icons confirmed at 3× zoom; Settings pane
L61 populated with 9 Vibe tiles + 10 icon'd nav; taskbar floor +20/−17 constant; Start chroma
54–90). **High** that Start tiles lack lift (L132 face < L138 gap, inverted). **High** that
the danger red is a dull maroon ((122,45,69)). **High** that chrome type is mono not sans
(visible at 2× across every surface). **Medium** on the exact parity number (84% is a
calibrated estimate against measured reference luma L172.9/188, not a metric). **Medium** on
precise trim targets (frost +1 step, danger-red R≥200, tile-lift +8 L) — each needs one
host-render iteration to dial in.
