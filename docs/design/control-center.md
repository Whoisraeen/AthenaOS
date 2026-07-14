# Design Spec: Control Center (Quick Settings flyout)

> *"Built for people who care about how things feel."* — RaeenOS_Concept.md
>
> The one-tap glance-and-toggle surface. It must clear: **macOS Control Center's
> grouped module layout + expandable modules, and Windows 11 Quick Settings'
> toggle grid + sliders + edit-tiles** — without macOS's two-tap-to-expand
> friction or Win11's cramped 2-column-only grid.

**All tokens below are defined in [`design-language.md`](./design-language.md)**
and live in `rae_tokens`. This spec only assigns them; it introduces no new magic
numbers. It **supersedes and deepens** `desktop-shell.md` §3 (which sketched the
flyout in half a page) — that section now defers here.

---

## Concept promise + bar to clear

> "The user owns the machine… one tap to a different place." — RaeenOS_Concept.md
> (§Customization). Control Center is where the machine's *current state* (volume,
> network, game mode, RGB, do-not-disturb) is owned at a glance.

- **Bar to clear:** macOS Control Center (Sequoia) — modular tiles, some
  expandable into a full sub-panel (Wi-Fi list, audio output picker, Focus modes),
  drag-to-rearrange via Settings; + Windows 11 Quick Settings — 6–8 toggle pills,
  brightness + volume sliders, an edit pencil, an inline media-transport card.
- **The RaeenOS-specific promise:** because "gaming isn't a mode," Control Center
  is also the **fast lane to Game Mode + RGB + per-game power** without opening
  Settings — the gaming pillar lives one tap from the tray.

---

## Already built (delta only — verify-before-spec)

| Piece | Where | Today | This spec changes |
|---|---|---|---|
| Quick-settings flyout (sketch) | `desktop-shell.md` §3 | half-page: 2-col toggle tiles, 2 sliders, footer | → full module model, expandable tiles, media card, RGB/Game tiles, complete states |
| Tray cluster trigger | `raeshell::SystemTray` (`desktop.rs`/`lib.rs`) | clock + (sketched) icons | → opens this flyout; tray icons reflect live state |
| Toggle / Slider controls | `control_panel.rs::SettingControl` + the control kit (`settings.md` §4) | LIVE model + spec'd widgets | → **reuse** the control kit widgets; do not re-invent |
| Game Mode | `kernel`/`gameos.rs` + `game.mode` setting | LIVE | → a Control Center tile (fast lane) |
| RGB engine | `kernel/src/rgb.rs` (9 modes) + `rgb_api.rs` | LIVE | → an expandable RGB tile (effect quick-pick) |
| Vibe presets | `vibe_mode::ALL_PRESETS` (12) | LIVE | → an optional Vibe quick-switch row |
| Media player | `raeshell::media_player` | LIVE | → the inline media-transport card source |
| `material.glass` / `elev.*` | `compositor.rs` | LIVE (shadow fix pending — see `material-and-shadow.md`) | reused |

**Not a rebuild** — it composes the existing control kit + live subsystems into a
glass flyout with the full state matrix.

---

## Prior art distilled (current systems, 2024–2025)

- **macOS Control Center (Sequoia):** a single glass panel of *modules* — some are
  simple toggles, some are **expandable** (clicking the module's chevron/region
  slides in a sub-panel: Wi-Fi shows the network list, Sound shows the output
  device picker, Focus shows the mode list) — expansion happens *in place* with a
  push/slide. Brightness + Sound are always-visible sliders. Drag-to-customize
  lives in Settings. **Take:** expandable-in-place modules (no separate window),
  always-visible brightness/volume, glass material. **Avoid:** the two-tap cost to
  reach a Wi-Fi network (RaeenOS expands on first tap of the tile's expand region).
- **Windows 11 Quick Settings:** a 2-column grid of toggle *pills* (Wi-Fi, BT,
  Airplane, Battery saver, Night light, Accessibility…), brightness + volume
  sliders below, a **pencil** to add/remove/reorder tiles, and an inline **media
  transport card** (album art + prev/play/next) when audio plays. **Take:** the
  edit-tiles affordance, the inline media card, the slider row. **Avoid:** the
  hard 2-column cap (RaeenOS uses a responsive 2–4 col grid) and the separation
  from notifications into two different flyouts (RaeenOS keeps them adjacent).
- **GNOME Quick Settings (46+):** compact toggle buttons with a built-in
  expand-arrow for the few that need a sub-menu (Wi-Fi, Power mode); strong focus
  rings. **Take:** the per-tile expand-arrow as the expand affordance; visible
  focus.

**RaeenOS synthesis:** macOS's **expand-in-place modules** + Win11's **edit-tiles
+ inline media card + slider row**, on the shell's `material.glass`, with a
**Gaming row** (Game Mode + RGB + per-game power) that is uniquely ours and
honors "gaming isn't a mode."

---

## RaeenOS design tokens this surface uses

- **spacing:** `space.2` (tile-internal padding, intra-grid gap), `space.3` (tile
  label inset, slider row padding), `space.4` (panel padding, section gap).
- **radius:** `radius.lg` (the panel), `radius.md` (toggle tiles, media card,
  expanded sub-panel), `radius.sm` (slider track ends, small buttons),
  `radius.xs` (footer icon hover), `radius.pill` (slider track + knob).
- **elevation:** `elev.3` (the flyout panel), `elev.2` (the media card / expanded
  sub-panel lift within the panel), `elev.focus` (focused tile/slider glow).
- **type:** `type.subtitle` (section header, e.g. "Gaming"), `type.label` (tile
  labels, media title), `type.body` (expanded sub-panel rows), `type.caption`
  (tile sub-state e.g. network name, media artist).
- **accent model:** seed = `ThemeAbi.accent_argb`; `rae_tokens::derive_accent`.
  An **on** tile = `accent.subtle` fill + `accent.text` icon; off = `bg.elevated`.
  Sliders fill `accent.base`. Same ramp as the whole shell — no private accent.
- **material:** `material.glass` (the panel + expanded sub-panels + media card —
  this IS a transient surface, so live blur is correct and bounded per the
  Mica/Acrylic doctrine).
- **motion:** `motion.fast` (panel open/close — smaller than Start), `motion.micro`
  (tile hover/press, slider knob, toggle), `motion.standard` (tile **expand**
  push/slide), `motion.instant` (reduced-motion).

---

## 1. Panel geometry

- **Trigger:** click the tray cluster, or `Super+A`.
- **Panel:** width **360px**, height sizes to content (max = screen − taskbar −
  `space.4`), anchored **bottom-right** above the tray at `space.2` inset.
  `material.glass` (`radius.lg`, blur 16, tint per palette, 1px `stroke.strong`
  top-edge highlight, `stroke.subtle` border), `elev.3`.
- **Padding:** `space.4` (16px) all sides; `space.4` between sections.
- **Open:** scale 96%→100% + fade 0→1 + 8px upward translate, `motion.fast`.
- **Close:** `motion.exit` (fade + scale 96%); click-away or `Esc` closes;
  **never steals focus from a running game** (it overlays, like a toast).
- **reduced-motion:** instant show/hide, opacity only.

---

## 2. Content (top → bottom)

### 2.1 Toggle-tile grid (responsive 2–4 col)
- Tiles **default 168×60px** in a 2-col grid at 360px width; a tile MAY be
  **wide** (full-row) when expanded. `radius.md`, `space.2` gaps.
- Each tile: an icon (left, `space.3` inset) + label (`type.label`) + an optional
  sub-state line (`type.caption`, e.g. the connected SSID, "On"/"Off"), and — for
  expandable tiles — a chevron on the right third (the **expand region**).
- **Default set:** Wi-Fi (expandable), Bluetooth (expandable), Do-Not-Disturb,
  Night Light, Airplane, Accessibility shortcut. (Editable — §4.)
- **States per tile:**
  - **off:** `bg.elevated` fill, `text.secondary` icon+label.
  - **on:** `accent.subtle` fill, `accent.text` icon, `text.primary` label.
  - **hover:** off→`bg.overlay`, on→`accent.subtle` lighter, `motion.micro`.
  - **active (press):** brief `accent.active` flash on the toggle half.
  - **focus (keyboard/controller):** 2px `accent.base` ring + `elev.focus` glow.
  - **disabled** (e.g. no Bluetooth radio): `text.tertiary`, no hover.
  - **dark/light:** per palette. **reduced-motion:** state swaps instant.

### 2.2 Expandable tiles (the macOS win — expand in place)
- Tapping a tile's **expand region** (chevron / right third) slides a sub-panel
  *into the flyout below the grid* (`motion.standard` push; the rest of the panel
  reflows down), `material.glass` `radius.md` `elev.2`, NOT a new window.
- **Wi-Fi expanded:** a scrollable list of networks — each row icon (signal) +
  SSID (`type.body`) + lock glyph; connected = `accent.subtle` + check; row
  height 36px; a "Network settings…" link footer to Settings.
- **Bluetooth expanded:** paired/available device rows, same row style.
- **collapse:** tapping the chevron again or another tile's expand collapses it
  (`motion.standard` reverse). One expansion at a time.
- **focus:** arrow keys move through the expanded list; `Enter` connects; `Esc`
  collapses. **reduced-motion:** the sub-panel appears/disappears instantly.

### 2.3 Slider row (always visible)
- **Volume** + **Brightness** sliders (control kit Slider, `settings.md` §4.2):
  4px track `radius.pill`, filled `accent.base`, 18px knob `radius.pill` `elev.2`,
  a small leading icon. Full panel width. Volume knob → click the icon to mute
  (icon → `text.tertiary` muted state).
- States/keyboard inherit the control-kit Slider (← / → step, focus ring + glow).

### 2.4 Media transport card (when audio is playing)
- A full-width `material.glass` `radius.md` `elev.2` card: album art (48px, left),
  title (`type.label` `text.primary`, 1-line clamp) + artist (`type.caption`
  `text.secondary`), and prev / play-pause / next buttons (32px targets,
  `radius.xs` hover, icon `text.primary`). Sourced from `raeshell::media_player`.
- Hidden entirely when nothing is playing (no empty card). **focus:** tab through
  the transport buttons; reduced-motion: no art cross-fade on track change.

### 2.5 Gaming row (the RaeenOS-native section — "gaming isn't a mode")
- Section header "Gaming" (`type.subtitle` `text.secondary`).
- **Game Mode** tile (toggle; on = `accent.subtle`, drives `gameos`/SCHED_GAME
  prioritization).
- **RGB** tile (expandable → quick-pick of the 9 `rgb.rs` effect modes as a
  horizontal chip row + a brightness mini-slider; full control in Settings →
  Power & Gaming).
- **Performance** tile (expandable → a 3-segment Balanced / Performance / Battery
  segmented control, mirrors `sys.power` + per-game GPU power).
- Each follows the §2.1 tile state matrix; the RGB chips show their effect color
  live.

### 2.6 Footer
- Left: user avatar + name (`type.label`, click → Accounts). Right: a **Settings**
  gear and a **power** glyph (32px targets, `radius.xs` hover, focus ring). Power
  opens the lock/sleep/restart/shutdown menu (`material.glass` mini popover).

---

## 3. Notifications adjacency (one mental model, not two flyouts)

Unlike Win11 (Quick Settings and Notifications are separate flyouts), RaeenOS
keeps them coherent: the **notification stack** (`notify.rs`, see `desktop-shell.md`
§5) lives top-right; Control Center lives bottom-right; both use the **same glass
material, same accent, same `elev.3`**. A future "notification center" history
panel (not in scope here) would dock above Control Center as a continuous column —
flagged so the geometry leaves room. No duplicate material systems.

---

## 4. Edit tiles (Win11's pencil — customization)

- An **edit** affordance (pencil glyph) in the panel header: enters an edit mode
  where tiles show a remove (−) badge and a drag handle; a "+ Add" row reveals the
  available-but-hidden tiles. Reorder by drag; layout persists per user.
- This is the Concept's ownership principle applied to the glance surface. Edit
  mode uses `accent.base` handle accents and `motion.micro` on reorder.
- **reduced-motion:** drag still works; no animated reflow (snap).

---

## 5. Cohesion acceptance (the whole-surface test)

Control Center ships only when:
1. **Same accent:** on-tiles, slider fills, and the media card accent all read the
   *same* `accent.base` as the taskbar / Start / Settings / Files (one
   `derive_accent` ramp).
2. **Same material/radii:** panel `radius.lg` + `material.glass` + soft `elev.3`
   shadow (post `material-and-shadow.md` fix) identical to the Start menu — switch
   one Vibe preset and Control Center re-skins with everything else.
3. **Expand-in-place feels native:** Wi-Fi expands within the panel, not a new
   window; reflow is smooth and reduced-motion-safe.
4. **Focus everywhere:** every tile, slider, expanded row, media button, footer
   icon has a focus state distinct from hover (raeen-accessibility sign-off).
5. **Dark + light parity:** both palettes render with passing contrast.

---

## Proposed tokens

None new required — all values compose from existing `rae_tokens`. Tile sizes
(168×60, 360px panel) are *local layout constants* for raeen-shell-apps, expressed
as `space.*` multiples, deliberately not global tokens.

---

## Handoff

### Implementers
- **raeen-ui (framework):** reuse the control kit (Toggle, Slider, Segmented) from
  `settings.md` §4; add the **expandable-tile container** (a tile that pushes an
  in-place glass sub-panel) and the **media-transport card** as reusable widgets.
- **raeen-shell-apps (the surface):** build the Control Center flyout — panel
  geometry, the 2–4 col tile grid, expandable Wi-Fi/BT/RGB/Performance sub-panels
  (wired to the live net / `rgb.rs` / power subsystems), the always-visible slider
  row (wired to volume/brightness), the media card (wired to `media_player`), the
  Gaming row (wired to `gameos` + `rgb_api`), the footer, and edit-tiles mode. Tray
  cluster opens it; tray icons reflect live state. Replace the `desktop-shell.md`
  §3 sketch.
- **raeen-gfx:** the panel + sub-panels are `material.glass` (bounded transient
  blur — correct use); they inherit the soft-shadow fix from
  `material-and-shadow.md` (must land first or the panel shows the hard-block
  shadow). Confirm in-place reflow doesn't re-blur the whole backdrop per frame
  (blur the panel region only).
- **theme_engine (kernel):** honor accent + `corner_radius`/`blur_radius` so the
  flyout re-skins on a Vibe switch; persist the edit-tiles layout per user.
- **raeen-accessibility (flagged):** focus states across tiles/sliders/expanded
  rows; 32px/48px hit targets (tiles are 60px tall — fine; verify chevron/footer
  hit areas); reduced-motion expand path; AA contrast (on-tile `accent.text` on
  `accent.subtle` must clear 4.5:1) on both palettes.

### On-screen / boot-log evidence (raeen-visual-qa + smoketests)
- **Panel:** QEMU screenshot of Control Center open over wallpaper — glass blur
  visible, 2-col tile grid (on + off tiles distinguishable), volume + brightness
  sliders, footer. Log: `[cc] open tiles=N sliders=2 accent=0x.. blur=16`.
- **Expand-in-place:** before/after screenshot of tapping Wi-Fi — the network list
  sub-panel pushed into the flyout (not a new window). Log: `[cc] expand=wifi
  rows=N inplace=true`.
- **Media card:** screenshot of the transport card with art + title/artist +
  prev/play/next while audio plays; and a screenshot proving the card is absent
  when nothing plays. Log: `[cc] media: shown=true` / `shown=false`.
- **Gaming row:** screenshot of the Gaming section — Game Mode on, RGB expanded to
  the effect chips, Performance segmented control. Log: `[cc] gaming: gamemode=on
  rgb_chips=9 perf=balanced`.
- **States:** a frame showing a tile in off / on / hover / keyboard-focus side by
  side; a focused slider with glow + ring. Plus a `run_boot_smoketest` that
  asserts the on-tile reads `accent.subtle` and the slider fill reads `accent.base`
  (must be able to print FAIL).
- **Cohesion (the §5 test):** before/after one Vibe preset switch — Control Center
  on-tiles + sliders change accent together with the taskbar + Start. Log: extend
  `vibe_mode` smoketest to assert `cc.accent == taskbar.accent ==
  derive_accent(seed).base`.

### Unblocks (MasterChecklist)
- **Phase 8 (RaeUI/RaeKit):** expandable-tile + media-card widgets.
- **Phase 13 (Customization):** edit-tiles + the Vibe/RGB/Game fast lane make the
  ownership story one tap from the tray.
- **Phase 14 (RaeShell + apps):** the quick-settings/Control-Center surface, from
  `desktop-shell.md` §3 sketch to a macOS/Win11-rival flyout.
- **Consumer Production Gate "Gamer":** Game Mode + RGB + performance one tap from
  the tray is a gaming-OS differentiator.
