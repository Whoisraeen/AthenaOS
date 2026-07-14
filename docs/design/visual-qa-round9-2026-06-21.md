# Visual QA — Round 9 — definitive closing parity audit of the COMPLETE Liquid Glass identity (2026-06-21)

Comprehensive PIL-measured parity audit of all **9 shipped surfaces** in
`docs/design/screenshots/` (commit `07f90c4`) against `docs/design/IDENTITY.md`
(the identity bible — §7 tier table is normative), `material-and-shadow.md`, the
curated references in `docs/design/reference/`, and macOS 26 "Tahoe" Liquid Glass /
Windows 11 24H2 from knowledge. Three surfaces (**lock-screen, context-menu,
command-palette**) are NEW since Round 8 and get first critique.

Measured with PIL 12.2.0 (Python 3.10). Every number below is reproducible from the
committed PNGs. Bar: **"visually stunning, instantly recognizable, better than Win11."**
This is the session's closing assessment. Follow-up to Round 8 (84%).

---

## 1. Definitive overall parity %  —  **86%**  (was 84% at Round 8)

**+2 vs Round 8.** The whole journey is now glassed: every host-renderable surface —
desktop, Control Center, notifications, Files, taskbar, Start, Settings, **plus the
three new ones (lock, context menu, command palette)** — carries the same tiered
glass + aurora backdrop + rim + RaeBlue accent. That completeness is the story of the
round: there is no longer a single un-glassed surface in the host-renderable set that
betrays the identity. **The cohesion is real and it reads as one OS.**

Why only +2 and not more: the three new surfaces are *good* (B-grade, competitive),
but they do not move the headline because (a) they inherit the **same two finish gaps
that already cap the core set** — the missing warm rim stop and the one-step-low frost
— and (b) one of them (context menu) actually surfaces a **new legibility regression**
in its lower third (white text drops to 3.7:1 over a bright aurora bleed). So the new
surfaces add breadth at parity, not a quality jump.

Calibrated against the gold reference (`reference/Liquid glass guide…jpg`: glass
interior **L188**, near-white milky frost, raised pills with real drop shadow + bright
top highlight). Our interiors: popovers L104–112, panels L77–84, chrome L77. We are at
~**55–60% of reference frost brightness** and **still missing tile/card depth** — the
same two structural finishes that have capped every round since R6. 86% is honest:
breadth is essentially complete; the remaining 9–14 is *finish* (warm rim, frost step,
depth, RaeSans type) plus the kernel-gated first-run surfaces, not a structural break.

---

## 2. Per-surface one-line grades (A / B / C + biggest remaining nit each)

| Surface | Grade | Single biggest remaining nit |
|---|:---:|---|
| **Desktop / Aurora** | **A−** | calm, premium mesh; only nit is it could carry slightly more NW→SE ribbon anisotropy vs `download(1).jpg` |
| **Control Center** | **B+** | tiles still flat (no `elev` lift); panel interior L78 is ~one frost step low |
| **Notifications** | **B** | urgent/danger accent reads dull maroon, not a clean danger red (token-blocked) |
| **Files** | **B+** | content list field faintly cool (B+7 over R, no neutral `bg.content`); chrome L57–81 a touch dark |
| **Taskbar** | **B** | chrome tier interior L77 ≈ panel L78 — chrome is NOT visibly more see-through than panel (tier separation too small) |
| **Start menu** | **B** | tiles flat/recessed (face darker than inter-tile gap); no card lift |
| **Settings** | **B+** | chrome label type is blocky mono, not RaeSans; content field faintly cool |
| **Lock screen** (NEW) | **B+** | top band of card marginal (white CR 4.27 at y270); warm rim stop absent |
| **Context menu** (NEW) | **B−** | **lower third white text fails AA (3.7–3.9:1)** over a bright aurora bleed |
| **Command palette** (NEW) | **B** | warm rim stop absent; result-row secondary/path text legibility unverified over bright zones |

No surface grades A (the warm-rim + frost-step + depth finishes that would earn it are
absent system-wide); no surface grades below B− (every surface is coherent and reads as
glass). The set is tightly clustered at B/B+, which is itself the cohesion win.

---

## 3. Critique of the 3 NEW surfaces

### Lock screen (`surface-lock-screen.png`, 1280×800) — **B+**

A centered frosted `glass.popover` card over the aurora holding clock (`12:34` / date),
a round avatar with monogram, display name, and a password pill. **This is the strongest
of the three** and the right first impression.

- **Card interior** L90.8, rgb(74,92,129), chroma 54 — the aurora blue/violet bleeds
  through the frost (not a flat grey card). **Reads as glass. Good.**
- **Legibility is solid where it matters:** the clock/name band (y282–414) measures
  white-text CR **5.17–7.66:1** — comfortable AA, even AAA in the lower band. The
  password pill (y462–486, L144–153) is brighter but carries **dark ink** so that's
  correct, not a defect.
- **Nits:** (a) the very top band of the card (y270, L114) is marginal at white CR
  **4.27:1** — just under AA, where the clock's top edge sits; the §2.3 cap should pull
  this ≥4.5. (b) The **warm rim stop is absent** (0/4080 bottom-edge warm px — see §4).
  (c) The card feels slightly small/centered-generic vs macOS's full-bleed lock with the
  time top-anchored; this is a composition choice, flag to **athena-shell-apps** as a
  "does this match the macOS lock gravity" question, not a defect.

**Verdict: matches core-surface quality. Ship-worthy. No defect blocks it.**

### Context menu (`surface-context-menu.png`, 640×480) — **B−**

A frosted `glass.popover` flyout: 6 rows with leading line-icons (folder/Open, Open file
location, Run as administrator, gear/App settings, Pin to taskbar [disabled], X/Uninstall),
a RaeBlue accent-hover row on "Run as administrator" with shortcut hint `Ctrl+Shift+Enter`,
right-aligned shortcut hints, and separators.

- **Top 2/3 is excellent:** body interior y85–261 measures clean **L100, white-text CR
  5.15–5.22:1** — solid AA. Real icons, right-aligned shortcut hint present, stroke.subtle
  separators read. The accent-hover row is clean **RaeBlue (78,156,255)** carrying
  **dark ink (darkest L14)** — correct per IDENTITY §4 (dark-on-accent).
- **DEFECT — lower third fails legibility.** Below the accent row (y277–357, the
  "Pin to taskbar" disabled + "Uninstall" rows) the menu sits over a **bright aurora
  teal bleed** and the popover interior climbs to rgb(57–62,117–128,167–189), L107–115,
  pushing **white text to CR 3.73–3.94:1 — AA FAIL.** This is exactly the §2.3/§9
  legibility-luma-cap failure class: the cap is not biting hard enough on the popover
  tier over a bright backdrop region. The disabled row uses `text.tertiary` which is
  even dimmer → worse. **Owner: athena-gfx** (cap must clamp the composited interior over
  the bright bleed) + **athena-accessibility** (confirm ratio). This is the one genuine
  new regression of the round.

**Verdict: good bones, but the lower-third legibility drop keeps it at B−. Fixable
render-side via the existing cap.**

### Command palette (`surface-command-palette.png`, 1280×800) — **B**

A frosted `glass.popover` flyout with a search field (magnifier + query), result rows
with leading line-icons, an **accent-wash selected row** (Firefox), secondary/path text,
and right-aligned category tags.

- **Search field** rgb(73,94,176) — picks up the aurora violet, reads as frosted, good.
- **Selected row** is clean **RaeBlue (78,156,255)** carrying **dark-ish ink (darkest
  L43)** — correct dark-on-accent direction (slightly less dark than the context menu's
  L14; would prefer L≤25 for crisper ink, minor).
- **Magnifier renders as a real icon** (no `?` placeholder visible in the search slot) —
  the R8 P1 #4 `?`-placeholder appears fixed here. Good.
- **Nits:** (a) **warm rim absent** (0/6000 bottom-edge warm px). (b) The palette interior
  bottom (L104) is fine for white primary text (CR 4.91), but the **secondary path text**
  on result rows is unmeasurable at this zoom and is the R8-flagged class where
  `text.secondary` over bright popover can't reach 4.5:1 — **route to athena-shell-apps to
  confirm path/secondary text is promoted to `text.primary`** per IDENTITY §9 note.

**Verdict: competitive, matches core quality. No blocking defect; the rim + secondary-text
items are the same system-wide finishes.**

**Do the 3 new surfaces match the core?** Yes — lock and palette are at core-surface
quality (B+/B); context menu is one notch below (B−) solely due to the lower-third
legibility drop, which is a render-cap fix, not a design miss.

---

## 4. Consistency check across all 9 (the cohesion verdict)

**Glass tier usage — mostly coherent, one tier-separation weakness:**

| Tier | Surfaces | Measured interior L | Verdict |
|---|---|---|---|
| **chrome** | taskbar | 76.9 | per §7 should be the MOST see-through |
| **panel** | CC 77.6 · Settings sidebar 83.6 | 77–84 | workhorse |
| **popover** | context 112 · palette 104 · lock 112 | 104–112 | correctly the most opaque |

- **Popover > panel ordering holds and reads** (104–112 vs 77–84) — the tier model is
  working at that boundary. Lock/context/palette all correctly use popover per §7. **Pass.**
- **CONSISTENCY DEFECT — chrome ≈ panel.** Taskbar (chrome) interior **L77** is
  effectively equal to CC (panel) **L78**. Per IDENTITY §2.1 chrome is 25% alpha and
  panel is 45% — chrome must be *visibly more see-through* than panel. Right now they
  read the same. The "chrome floats on the wallpaper, most backdrop shows" intent (§7) is
  not landing — chrome needs to drop alpha / show more backdrop relative to panel. **Owner:
  athena-gfx.**

**Iridescent rim — INCONSISTENT and INCOMPLETE (the headline consistency defect):**

- The **violet right-edge stop renders** on the new surfaces (context (100,102,212) v=54,
  palette (64,60,133) v=38, lock weaker v=19) and cyan top renders — so the rim is *present*.
- But the **warm-amber bottom stop (`GLASS_EDGE_WARM 0x40FFC97C`, IDENTITY §2.4/§8.3)
  renders on ZERO surfaces.** Census of bottom-edge warm pixels (r−b>20):

  | surface | warm px / total | % |
  |---|---|---|
  | lock | 0 / 4080 | 0.0% |
  | palette | 0 / 6000 | 0.0% |
  | context | 0 / 4480 | 0.0% |
  | Control Center | 0 / 3480 | 0.0% |
  | Start | 0 / 5376 | 0.0% |

  **0 / 23,416 warm pixels system-wide.** The iridescent signature — the one thing IDENTITY
  §2.4 calls "THE signature… that no flat Acrylic/Mica has" — is rendering as a
  **two-stop cyan→violet sweep, never reaching the warm third stop at the bottom.** This
  is the single most consistent identity defect across the entire set. (The lone warm pixel
  found anywhere was one (219,190,109) artifact on a context-menu *internal* separator at
  y269, not the rim.) **Owner: athena-gfx** — the perimeter hue interpolation is not
  completing the bottom arc, or the warm stop alpha is being lost in the additive blend
  over the blue backdrop.

**Type — uniformly NOT RaeSans (consistently wrong):** every chrome/label string across
all 9 surfaces renders in the blocky mono fallback, not a refined UI sans. Consistent, but
consistently the largest felt-quality gap vs SF Pro / Segoe Variable. **Token/font-blocked.**

**Icons — real everywhere:** nav rows, tiles, pills, context-menu rows, palette result
rows all carry real line-icons. The R8 search-field `?` placeholder appears resolved in the
palette. **Pass.**

**Accent discipline — clean:** RaeBlue (78,156,255 / 79,146,244) is the only accent;
selection/hover rows carry dark-on-accent ink correctly; no surface drowns labels in blue.
**Pass** (IDENTITY §4 restraint rule held).

**Corner radii — consistent** (lg≈16 on panels/popovers) across all surfaces. **Pass.**

**Legibility — mostly holds, two soft spots:** white `text.primary` clears AA on Files/CC/
Settings/Start/taskbar/lock-body/palette-body. **Fails** in (a) context-menu lower third
(3.7–3.9:1) and (b) lock card top band (4.27:1). Both are the §2.3 cap not biting over
bright aurora bleed — route to **athena-gfx + athena-accessibility**.

**Surfaces that DRIFT:** (1) **taskbar** — chrome tier not distinct from panel; (2)
**context menu** — lower-third legibility. Everything else is in-system.

---

## 5. Definitive prioritized remaining-defect list → 90–95%

Tagged by blocker status. **Doable-now items alone are worth an estimated +4–5
(→ ~90–91%); the token + kernel items carry the rest to 95%.**

### DOABLE-NOW (no token, render-side — highest leverage this session)

1. **Warm-amber bottom rim stop renders nowhere (0/23,416 px).** The iridescent signature
   completes cyan→violet only. → Fix the perimeter hue interpolation so the bottom arc
   reaches `GLASS_EDGE_WARM 0x40FFC97C`; ensure the additive blend preserves the warm hue
   over a blue backdrop (it's currently being swamped). **Target:** ≥2% warm px on each
   surface's bottom edge, visible amber at the BR corner. **Owner: athena-gfx.**
   *Highest-leverage doable-now — it's the literal "instantly recognizable" signature, and
   it's missing on every surface.* **~2 pts.**

2. **Context-menu lower third fails AA (white text 3.7–3.9:1 over bright aurora bleed).**
   The §2.3/§9 legibility luma cap isn't biting on the popover tier over a bright backdrop
   region. → Clamp the composited popover interior to `GLASS_INTERIOR_LUMA_CEIL 0.40` where
   it sits over the bright bleed. **Target:** white `text.primary` ≥4.5:1 across the whole
   menu. **Owner: athena-gfx** + **athena-accessibility** (confirm). **~1 pt.**

3. **Chrome tier not distinguishable from panel (taskbar L77 ≈ CC panel L78).** Chrome
   must show more backdrop per §2.1 (25% vs 45% alpha). → Drop chrome effective alpha /
   lift its backdrop bleed so it reads visibly more see-through than panel. **Target:**
   chrome interior ≥ +8 backdrop-bleed delta vs panel over matched backdrop. **Owner:
   athena-gfx.** **~1 pt.**

4. **Tiles/cards have no depth (Start tiles face < inter-tile gap; CC tiles flat).**
   Carried from R8 P0 #1 — still the single biggest "Win11-tier" tell missing. → `elev.1/2`
   soft shadow + 1px `stroke.strong` top highlight at tile scale. **Target:** tile face
   ≥+8 L over the gap with a visible penumbra. **Owner: athena-gfx** (render) +
   **athena-shell-apps** (call sites). **~2 pts.**

5. **Lock card top band marginal (white CR 4.27 at y270).** Minor — the §2.3 cap fix in
   #2 likely covers it. **Owner: athena-gfx.** **~0.5 pt.**

### TOKEN-BLOCKED (ath_tokens held by concurrent session)

6. **Frost one step low system-wide** (popovers L104–112 / panels L77–84 / chrome L77 vs
   gold **L188**, ~55–60%). → +1 white-add frost step per tier, a11y re-confirm. **Owner:
   athena-ui (token) + athena-gfx + athena-accessibility.** **TOKEN-BLOCKED** (frost-step bake).
   **~2–3 pts.**

7. **Danger/urgent red is a dull maroon, not a clean danger red** (notifications/taskbar
   urgent). → `status.danger` ink (~(255,69,58)) that survives the legibility cap without
   desaturating to maroon. **Owner: athena-ui (token) + athena-shell-apps + athena-gfx.**
   **TOKEN-BLOCKED** (`status.danger`). **~1 pt.**

8. **Chrome type is blocky mono, not RaeSans** (all 9 surfaces). → real RaeSans UI font on
   chrome call sites at the documented scale/weights. **Owner: athena-ui (font/token) +
   athena-shell-apps.** **TOKEN/FONT-BLOCKED** (shared type tokens). **~2–3 pts** — largest
   single felt-quality gap vs SF Pro / Segoe.

9. **Content fields faintly cool** (Settings B+18, Files B+7; no neutral `bg.content`). →
   near-neutral `bg.content` (~L57, ±3 channels). **Owner: athena-ui.** **TOKEN-BLOCKED.**
   **~0.5 pt.**

### KERNEL / RISKY (not yet glassed — first-run + window chrome)

10. **Login / OOBE screens** predate the glass system (`login-card-preview.png`,
    `oobe-*-2026-06-17.png` are pre-identity). → re-compose over the aurora with
    `glass.panel` card + rim + the new depth. **Owner: athena-shell-apps + athena-gfx.**
    **KERNEL/RISKY** (login/OOBE are kernel-side framebuffer renders per the memory
    `oobe-auto-advance-login-only`; needs care to not regress the boot session-phase flow).
    **~1–1.5 pts.**

11. **Generic window chrome (titlebar / traffic-lights / borders) on a non-Files app.**
    Only critiqued inline via Files. → shoot a standalone app window; apply the Files chrome
    recipe (frosted titlebar ≥ toolbar, full-perimeter rim, traffic-light controls).
    **Owner: athena-shell-apps + athena-gfx.** **KERNEL/RISKY** (window-chrome path is
    compositor-side). **~1 pt.**

---

## Reference comparison (named gaps, measured source)

- **`reference/Liquid glass guide…jpg` (gold):** glass interior **L188** near-white milky
  frost; buttons/tiles are **raised pills with a clear drop shadow + bright top-edge
  highlight**; the rim shows a **full chromatic sweep including warm tones** (visible amber/
  pink at the bottom-right of the "Modern Glass UI Design" card). Our gaps vs this exact
  image: (a) frost L104–112 ≈ 55–60% as bright (#6); (b) **no tile depth** (#4); (c) our rim
  **stops at violet, never reaching the warm stop** the reference clearly shows (#1). The
  reference's whole signature is *milky brightness + depth + full chromatic rim* — those are
  our three top defects.
- **macOS 26 Tahoe (knowledge):** specular + edge light-bend + background-luminance lift +
  glass-chromed windows — we have all four on shell, Settings, Files, and now lock/context/
  palette. Remaining vs Tahoe: (a) Tahoe controls/tiles cast real soft shadows (our flat
  tiles, #4); (b) Tahoe chrome type is SF Pro (our mono, #8); (c) Tahoe's lock is full-bleed
  with top-anchored time (our centered card, lock nit). The lock screen specifically reads
  closer to a generic login dialog than Tahoe's lock gravity.
- **`reference/Aero Start Menu…jpg` (Win11 glass):** reference Start tiles are **raised glass
  chips with lift**; ours remain flat (#4). Our flyout frost chroma already beats the
  reference's flatter Acrylic — with tile-lift (#4) + the frost step (#6) + the warm rim (#1),
  Start clears "better than Win11."

## Consistency issues (list)

- **Warm rim stop absent system-wide** (0/23,416 px) — the signature is incomplete on every
  surface (#1). *Top consistency defect.*
- **Chrome tier ≈ panel tier** (taskbar L77 ≈ CC L78) — chrome not visibly more see-through (#3).
- **Tile/card depth absent shell-wide** (Start face < gap; CC flat) (#4).
- **Frost one step low**, uniform across CC/Start/Settings/Files/popovers (#6).
- **Chrome type mono not RaeSans**, uniform across all 9 (#8).
- **Danger red desaturated** (notifications/taskbar) (#7).
- **Context-menu lower-third legibility** drops below AA over bright bleed (#2).
- **RESOLVED since R8:** search-field `?` placeholder (palette magnifier renders real);
  every surface now glassed (lock/context/palette shipped); icons real everywhere; corner
  radii consistent; accent discipline held.

## Blocking (won't render)

**None.** All 9 surfaces render cleanly via the host rasterizer. No handoff to
verifier/debugger. (Login/OOBE/window-chrome are *unbuilt-as-glass*, not broken renders —
they're the KERNEL/RISKY tail, items #10–11.)

## Confidence

**High** on the warm-rim-absent finding (0/23,416 px census, reproducible). **High** on the
context-menu lower-third AA fail (3.7–3.9:1 measured across y277–357). **High** on chrome≈panel
(L77≈L78). **High** that lock-card body legibility is solid (5.17–7.66:1) and the new surfaces
carry correct dark-on-accent ink. **Medium** on the exact parity number (**86%** is a
calibrated estimate against measured reference luma L188, not a metric — defensible band
84–87%). **Medium** on precise fix targets (warm-rim ≥2%, frost +1 step, tile-lift +8 L,
chrome-bleed +8) — each needs one host-render iteration to dial in.
