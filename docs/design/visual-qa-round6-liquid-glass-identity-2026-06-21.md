# Visual QA — Round 6 — Liquid Glass Identity, post-Files-glass-chrome + CC rim sweep (2026-06-21)

Static-pixel re-measure of the regenerated identity surfaces (commit `0e161b4`) in
`docs/design/screenshots/` against `docs/design/IDENTITY.md`, the curated references in
`docs/design/reference/`, and macOS 26 Tahoe / Windows 11 24H2 from knowledge.

Measured with PIL 12.2.0 (Python 3.10), not eyeballed. Every number below is reproducible
from the committed PNGs. Follow-up to
`visual-qa-round5-liquid-glass-identity-2026-06-21.md` (scored **66%**; P0 = the live
Files window rendering as a flat dark opaque box with no glass identity).

---

## Verdict up front

**Identity parity vs macOS 26 Tahoe / Windows 11 24H2: ~74%** (was 66% at Round 5, 62%
post-rim, 58% at Round 4).

**The Round-5 P0 — the flat dark Files window — is dead in the pixels.** This was the
single biggest cohesion break in the whole set, and it is genuinely fixed. The Files
window now reads as a Liquid Glass app that belongs to the same OS as the Control Center.
Measured deltas, all in the right direction:

| Files region | Round-5 | Round-6 | Reads as |
|---|---:|---:|---|
| titlebar | L20.4 | **L57.4** | frosted chrome (max-px 242 = traffic-light/specular) |
| toolbar | L51.2 | **L71.8** | frosted chrome |
| sidebar interior | L42.1 | **L81.2** (66/83/105, teal-lean) | translucent — aurora bleeds through, rim max 242 |
| content rows | L45.2 (31/46/74 bluish) | **L57–64 (55/57/64 NEUTRAL)** | solid de-tinted light field, ftype icons pop |
| selected row | L22.4 dark | **L123.3 (63/131/217)** | real RaeBlue selection |

The window now sits *at or above* its backdrop instead of darker than it (Round 5's
subtractive-dark-card polarity at app-window scale), the content list was de-tinted from a
bluish near-black to a near-neutral light field, the sidebar is genuinely translucent
(teal-leaning, backdrop reading through, an iridescent rim spike at max-242), and the
selected row is a proper saturated RaeBlue instead of a darker-than-everything slate. The
"2015 dark file manager on a 2026 glass desktop" verdict is retired. **This is the
highest-leverage fix in the whole Round-2→6 arc and it landed.**

**What now caps parity is a short, well-defined tail — no single headline break remains.**
The three things between us and 80–90%: (1) **white text on the brightest accent-tinted
tiles is below WCAG AA** — the luma clamp protects aurora-driven glass but accent "on"
tiles add their own blue lift and white labels wash to ~1.7–1.9:1; (2) **the CC rim is now
a real cyan→violet sweep but the warm-amber stop is still weak** (warm ~29–63 px vs violet
~1900–3500); (3) **whole surfaces are still untouched** — taskbar, Start/launcher, Settings,
context menus, login. The glass *mechanism* now competes; the remaining distance is fine
finish on what exists plus extending it to the surfaces we have not shot yet.

---

## 1. Files window — the headline fix, adjudicated → **CONFIRMED, FIXED**

`surface-files.png` (1100×720). Does it now read as a Liquid Glass app that belongs to the
same OS as the CC? **Yes.** Hard data above; to the eye the window has a frosted titlebar
with the backdrop reading through the top chrome, a translucent teal-tinted sidebar
(Quick Access) with the aurora bleeding through it the way Finder's sidebar does, a solid
but de-tinted neutral content field where the ftype-colored icons now read against a light
background, a saturated RaeBlue selection row, and an iridescent edge spike (max-px 242 on
the sidebar/titlebar bands = the rim/highlight). The traffic-light close treatment replaced
the hard red square.

**Remaining Files breaks (all minor, none structural):**

- **Content field is still slightly cool, not fully neutral.** Rows measure (55/57/64) —
  R≈G but B is +7 over R, a faint residual blue cast. Target a true neutral `bg.surface`
  (R≈G≈B within ±3) so ftype icon hues read against a hue-free field, exactly as Finder's
  near-white / our dark-neutral. **Defect:** content L57–64 reads marginally cooler than a
  clean neutral. **Fix:** drop the residual blue tint on the content fill by ~5–7 on B.
  **Target:** content rgb within ±3 across channels, L held at ~60. **Owner: raeen-shell-apps.**
- **Titlebar L57 is the lightest *chrome* but still the darkest *frosted* band** — it's
  frosted now (up from L20) but sits below the toolbar (L72) and well below the sidebar
  (L81). macOS/Win11 titlebars are typically the *brightest* glass band, not the dimmest.
  **Defect:** titlebar L57 < toolbar L72 < sidebar L81 (chrome tier inverted top-to-bottom).
  **Fix:** lift titlebar frost to ≥ toolbar (target titlebar ≥L72) so the chrome reads
  top-down brightest-to-content. **Owner: raeen-gfx** (chrome frost step) + **raeen-shell-apps**.
- **Window rim is present but asymmetric / weak on the bottom edge** — the max-242 spikes
  cluster on the sidebar/titlebar (top-left); the bottom and right window edges don't show
  the same iridescent tell. **Defect:** rim energy concentrated top-left, weak bottom-right.
  **Fix:** route the Files window edge through the same full-perimeter cyan→violet→warm map
  the tiers demo uses. **Owner: raeen-gfx.**

**Net: the Files window is fixed.** It belongs to the OS now. The three items above are
polish, not cohesion breaks — they move Files from "belongs" to "best-in-class."

---

## 2. Legibility — white text over the brightest glass → **PARTIAL: clamp works on aurora glass, FAILS on accent-tinted "on" tiles**

The Round-5 claim is that the WCAG luma-cap is applied in the render (glass over bright
aurora capped, dark untouched). **Spot-check confirms the clamp works where the brightness
comes from the aurora, but NOT where an accent-tinted "on" tile adds its own blue lift.**

Measured true glyph-vs-local-background contrast (glyph pixels L>160 vs surrounding glass
L<140, not the tile mean):

| Region | glyph L | local-bg L | contrast | AA (4.5:1)? |
|---|---:|---:|---:|:--:|
| CC "Game Mode" label (on bright blue on-tile) | 204 | 121 | **1.66:1** | **FAIL** |
| CC "Raeen-5G" (selected/brightest wifi row) | 214 | 108 | **1.94:1** | **FAIL** |
| Toast body text (frosted card L94–99) | dark-on-glass | 96 | passes (dark text) | OK |
| Files content rows (neutral field L60) | mixed | 60 | OK | OK |

The zoom (`/tmp/gm_text_4x.png` source) shows "Game Mode" is *legible but soft* and the "On"
sublabel beneath is barely visible. This is the new top legibility defect: **white text on
the brightest accent-tinted tiles is ~1.7–1.9:1, well under AA 4.5:1.** The clamp caps the
aurora contribution but the "on"-state accent fill (RaeBlue) pushes the local background to
L108–121, and white-on-that washes out.

- **Defect:** white label + sublabel on accent-"on" tiles (Game Mode, selected Wi-Fi row)
  measure 1.66–1.94:1.
- **Fix (two options, prefer the first):** (a) on accent-tinted "on" tiles, switch label
  ink to `text.on-accent` = a near-white at higher opacity *with a subtle dark scrim/halo
  behind the glyph* (macOS does exactly this — text on Control Center colored toggles gets a
  shadow), OR (b) darken the accent-"on" fill itself so white clears 4.5:1. Target: every
  primary tile label ≥4.5:1, sublabel ≥3:1.
- **Owner: raeen-shell-apps** (ink + scrim on accent tiles) with **raeen-accessibility**
  re-confirming 4.5:1 / 3:1, and **raeen-gfx** if the clamp needs to also factor accent-fill
  luma (not just aurora luma) into the cap.

**Verdict:** the clamp is real and working on aurora-driven glass (that's why the panel and
non-accent tiles are fine), but it does not yet account for accent-fill self-luminance, so
the brightest *tinted* tiles still wash white text. This is now P0 — it's the one thing that
actually hurts daily usability rather than just the look.

---

## 3. CC rim — full cyan→violet→amber sweep? → **MOSTLY: cyan→violet shipped, warm still weak**

The Round-5 P1 #4 was "shipped CC rim is cyan-only (cyan 2573 / violet 0 / warm 0)." Round 6
the CC surface render now carries a real multi-hue perimeter. Per-edge averages on the
shipped CC panel (`surface-control-center.png`, panel bbox x903–1271, y18–746):

| Edge | avg rgb | reads as |
|---|---|---|
| top | (52, 86, 107) | **cyan** (G>R, B high) |
| left | (62, 65, 136) | **violet/blue** |
| right | (71, 67, 109) | **violet-leaning** |
| bottom | (28, 33, 61) | **dark — warm stop weak/absent** |

Perimeter hue classification over the panel: **violet ~1924–3483 px, cyan/cyanblue
~1433 px, warm ~29–63 px.** So the **cyan→violet half of the sweep is genuinely shipped on
the real CC surface now** (Round 5: violet 0). The 3× bottom-right crop (`/tmp/cc_br_3x.png`)
visually confirms a violet/magenta right edge and a faint warm transition at the BR corner —
but the bottom edge measures L28–33 (dark), so the **amber stop the claim cites (warm 949)
is not landing on the shipped surface; it's ~29–63 px, the same desaturated/missing warm
Round 5 flagged on the demo tiers.**

- **Defect:** CC rim warm-amber stop is ~29–63 px (bottom edge L28–33, no amber); the sweep
  is effectively cyan→violet only on the shipped surface.
- **Fix:** boost the warm stop at the bottom/bottom-right perimeter position — raise R, drop
  B so it reads amber not pink/dark over the violet-blue aurora. **Target:** bottom-edge
  sample with R>G>B and R≥180; warm-px count ≥25% of violet count.
- **Owner: raeen-gfx** (per-perimeter warm stop on the shipped CC render path).

**Verdict on the claim (violet 1004 / warm 949):** violet **CONFIRMED** (1900–3500, well
over the claimed 1004). Warm **NOT confirmed** — measures ~29–63, not ~949. The amber stop
is the one rim element still not reaching the shipped surface.

---

## 4. NEW overall parity % vs macOS 26 Tahoe / Win11 24H2 — honest

**~74%, up from 66%.**

The +8 is earned almost entirely by closing the Round-5 P0 (the flat dark Files window),
which was scored as a fresh −6 to −8 drag last round. Bringing a real shipped app window
into the glass identity is worth more than any shell-polish step because it's the difference
between "the shell looks good but the apps don't" and "the whole product reads as one OS."
Two smaller closes also count: aurora peak is now in-band (L148.6, was L171) so panels no
longer get pushed to their opaque auto-adjust bound, and toasts are now uniformly luminous
(L94–99, both clear ≥90 — Round 5's "toast-1 one step shy" is fixed).

Calibrated against measured reference numbers (gold ref full-image mean **L172.9**, glass
interior **L188**):

**What we now have (the credit):**
- **Luminous tiered glass shell AND a glass app window** — CC L95–100, toasts L94–99, Files
  chrome L57–81 with a translucent sidebar and de-tinted content. The cohesion mechanism
  reads across both shell and apps now. **Most credit, and the new credit.**
- **Files polarity fixed** — window at/above its backdrop, de-tinted neutral content,
  RaeBlue selection, traffic-light close. **Full credit.**
- **Aurora peak in target band** (L148.6 vs 140–150). **Full credit.**
- **CC cyan→violet rim shipped** (violet 1900–3500 on the real surface). **Partial** (warm weak).

**What caps us below parity (the −26):**
- **White text on accent-"on" tiles below AA** (1.66–1.94:1). The clamp doesn't factor
  accent-fill self-luminance. This is a *usability* gap, not just aesthetic. **~5 points.**
- **Glass still ~50–55% of reference frost brightness.** Gold ref interior **L188**; ours
  panel L100 / Files chrome L57–81. Even granting dark theme, there's room for +1 frost step
  on chrome bands. **~6 points.**
- **CC rim warm-amber stop missing** (warm 29–63 vs violet ~2500). **~3 points.**
- **Untouched surfaces** — taskbar, Start/launcher, Settings, context menus, login have not
  been shot or glassed. These are half the felt surface area of a desktop OS, and parity is
  measured against the *whole* product. **~10 points** (the new single largest bucket).
- **Long tail** — Files titlebar dimmer than its toolbar; content field faintly cool; Files
  rim asymmetric. **~2 points.**

74% is honest: the shell and the flagship app window now genuinely compete; what stands
between us and 80–90% is (a) the accent-tile legibility fix, (b) one more frost step, and
(c) extending the now-proven glass system to the surfaces we have not built/shot yet.

---

## 5. Prioritized defect list to reach "on par / better than Win11"

Each: **surface → defect (measured) → fix + target value → owner.**

### P0 — usability + the rim finish

1. **White text on accent-"on" tiles below WCAG AA** (CC Game Mode label 1.66:1, Raeen-5G
   row 1.94:1; glyph L204–214 on local-bg L108–121). The luma clamp caps the aurora
   contribution but not the accent-fill self-luminance, so tinted "on" tiles wash white
   labels. → **Add a `text.on-accent` ink with a subtle dark scrim/halo behind glyphs on
   accent-tinted tiles** (the macOS Control Center pattern), or darken the accent-"on" fill.
   **Target:** primary label ≥4.5:1, sublabel ≥3:1 on every tile. → owner: **raeen-shell-apps**
   (ink + scrim) + **raeen-accessibility** (re-confirm) + **raeen-gfx** (clamp factors
   accent-fill luma if needed). *Highest leverage — it's the one thing that hurts daily use.*

2. **CC rim warm-amber stop is missing on the shipped surface** (bottom edge L28–33;
   warm 29–63 px vs violet ~2500). The cyan→violet half ships; amber does not. → Boost the
   warm stop's R, drop B at the bottom/BR perimeter so it reads amber over the violet-blue
   aurora. **Target:** bottom-edge sample R>G>B, R≥180; warm ≥25% of violet count. → owner:
   **raeen-gfx**.

### P1 — the frost finish + Files top-down chrome (close the gap to "better than Win11")

3. **Glass interior ~50–55% of reference frost brightness** — ours panel L100 / Files
   chrome L57–81 vs gold ref interior **L188**. → Push chrome-band frost +1 white-add step
   (titlebar/toolbar/CC chrome). **Target** chrome bands ≥L90, panel ≥L115, while a11y
   confirms text still clears 4.5:1 (interacts with P0 #1). → owner: **raeen-gfx** (frost
   step) + **raeen-ui** (token) + **raeen-accessibility**.

4. **Files chrome tier inverted top-to-bottom** — titlebar L57 < toolbar L72 < sidebar L81;
   the titlebar (should be brightest chrome) is the dimmest band. → Lift titlebar frost to
   ≥ toolbar. **Target:** titlebar ≥L72 (brightest-to-content top-down). → owner:
   **raeen-gfx** + **raeen-shell-apps**.

5. **Files content field faintly cool** — rows (55/57/64), B +7 over R. → Drop residual blue
   on the content fill. **Target:** content rgb within ±3 across channels, L~60. → owner:
   **raeen-shell-apps**.

6. **Files window rim asymmetric** — max-242 iridescent spikes cluster top-left
   (sidebar/titlebar); bottom/right edges weak. → Route Files window edge through the full
   per-perimeter cyan→violet→warm map. **Target:** all four edges carry rim. → owner:
   **raeen-gfx**.

### P2 — the surfaces we have NOT touched (the new largest bucket toward 80–90%)

These are *not yet shot or glassed* and are half the felt surface area of the OS. Each needs
a host-render shot first, then the same glass-chrome + rim treatment the CC/Files got. Listed
in leverage order (most-seen surface first):

7. **Taskbar / dock** — the single most-on-screen surface; not yet captured or glassed. →
   Glass `chrome` tier, running-app indicators, rim. Shoot it, then critique. → owner:
   **raeen-shell-apps** (compose) + **raeen-gfx** (glass chrome). *Spec exists:
   `docs/design/taskbar-running-apps.md`, `system-tray.md`.*

8. **Start / launcher / command palette** — second-most-invoked surface; spec exists
   (`command-palette.md`) but no shipped shot. The Aero/glass Start references
   (`reference/Aero Start Menu...jpg`, `Liquid Glass Theme for Windows 11...jpg`) are the
   bar. → `glass.popover` tier, search field, app grid, rim. → owner: **raeen-shell-apps**.

9. **Settings window** — spec exists (`settings.md`); should reuse the Files glass-chrome
   pattern (frosted chrome + translucent sidebar + solid content). → owner: **raeen-shell-apps**.

10. **Context menus** — the small-surface tell that separates polished from basic; should be
    `glass.popover` with the rim. Not shot. → owner: **raeen-shell-apps** + **raeen-gfx**.

11. **Login / lock screen** — first thing a user sees; a `login-card-preview.png` exists from
    Round 1 but predates the glass system. → Re-shoot over the aurora with the glass card +
    rim. → owner: **raeen-shell-apps**.

### P2 — long tail

12. **Toast close "x" and text-color hierarchy faint** — legible but the dismiss affordance
    and secondary text are subtle. → Strengthen the dismiss control contrast. → owner:
    **raeen-shell-apps**.

---

## Reference comparison (named gaps, with measured source)

- **`reference/Liquid glass guide_...jpg` (gold):** full-image mean **L172.9**, glass
  interior **L188** (near-white milky frost), balanced saturated cyan/violet/warm rim. Our
  gaps: glass **L57–100** (~50–55% as bright — chrome can go +1 step, P1 #3); rim warm
  missing on shipped CC (P0 #2). The frost-brightness gap is now the bulk of the felt *shell*
  difference, since the structural breaks are closed.
- **macOS 26 Tahoe (knowledge):** specular highlight + edge light-bend + background luminance
  lift + **glass-chromed app windows (Finder)**. We now have all four on both shell *and*
  Files (white top edge, cyan→violet rim, frost L57–100, translucent Finder-style sidebar).
  Remaining vs Tahoe: (a) Tahoe shadow-scrims white text on colored Control Center toggles —
  we don't yet, hence our P0 #1 wash-out; (b) Tahoe's dark glass lifts its backdrop more;
  (c) the warm rim stop. The **Files-window-is-not-glass** gap that put us behind Tahoe last
  round is **closed.**
- **Win11 24H2 Acrylic/Mica + `reference/download (1).jpg` (Fluent Files on aurora):** the
  reference Files panel is a light translucent Mica window with colorful folder chips over a
  blue ribbon. Ours is now **also a translucent glass window** (chrome L57–81, translucent
  sidebar, RaeBlue selection) over the aurora — we have **caught up on the file-manager
  surface** (Round 5 we were behind). Our shell glass already beats Mica's flatness; with the
  warm rim stop and the frost step, Files clears "better than Win11." **The dark-window-on-
  glass clash that put us behind Win11 in Files last round is gone.**

## Consistency issues

- **Accent-tile text contrast** (P0 #1) — white labels wash on the brightest tinted tiles;
  inconsistent legibility tile-to-tile (non-accent tiles fine, accent "on" tiles fail).
- **CC rim warm stop** (P0 #2) — cyan→violet ships, amber doesn't; sweep is 2/3 complete.
- **Files chrome tier inverted** (P1 #4) — titlebar dimmer than toolbar dimmer than sidebar.
- **Files rim asymmetric** (P1 #6) — top-left strong, bottom-right weak.
- **Files content field — RESOLVED to neutral-ish** (55/57/64, was bluish 31/46/74). Faint
  residual cool cast (P1 #5), no longer a polarity defect.
- **Files window polarity — RESOLVED** (chrome L57–81 ≥ backdrop; was L20–51 darker than
  backdrop). No longer a defect. *The Round-5 headline break is closed.*
- **Aurora peak — RESOLVED** (L148.6, in the 140–150 band; was L171). No longer a defect.
- **Toast luminance uniformity — RESOLVED** (both L94–99 ≥90; Round-5 toast-1 was L84). No
  longer a defect.
- **Untouched surfaces** (P2 #7–11) — taskbar, Start, Settings, context menus, login carry
  *no* glass treatment yet because they aren't shot; the system exists, it just hasn't been
  applied there. Largest remaining cohesion surface area.
- Corner radii consistent across CC/toasts/Files (lg ~16) — **no defect.**

## Blocking (won't render)

None. All identity surfaces render cleanly via the host rasterizer; `surface-files.png` is
the live `apps/files` render with the new glass chrome. No handoff to verifier/debugger.

## Confidence

**High** that the Round-5 P0 (flat dark Files window) is fixed — measured to hard numbers
(titlebar L20→57, toolbar L51→72, sidebar L42→81 with aurora bleed + rim-242, content
de-tinted to near-neutral 55/57/64, selection L22→123 RaeBlue). **High** that the aurora
peak (L148.6) and toast uniformity (L94–99) are fixed. **High** that white text on the
brightest accent-"on" tiles is below AA (Game Mode 1.66:1, Raeen-5G 1.94:1 — glyph L204–214
on local-bg L108–121, measured glyph-vs-local-bg not tile-mean). **High** that the CC rim
ships cyan→violet (violet 1900–3500 on the shipped surface) but the warm-amber stop does
not (warm 29–63, bottom edge L28–33). **Medium** on the exact parity number (74% is a
calibrated estimate against measured reference luma L172.9/188, not a metric). **Medium** on
the precise trim targets (frost +1 step, warm R≥180, scrim opacity) — each needs one
host-render iteration to dial in.
