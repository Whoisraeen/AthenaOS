# Design Spec: Settings

> *"Built for people who care about how things feel."* — RaeenOS_Concept.md
>
> Settings is the surface where the design language becomes **user-controllable**.
> It must clear: **the cleanness of macOS System Settings, the legible category
> IA of Windows 11 Settings, and Linux-grade ownership** — without copying the
> over-long macOS sidebar or Fluent's three-eras-of-controls inconsistency.

**All tokens below are defined in [`design-language.md`](./design-language.md)**
and live in the `rae_tokens` crate (ADR 0003). This spec only assigns them; it
introduces no new magic numbers. Where a token is genuinely missing, it is
flagged as a *proposed addition to `design-language.md`* (§ "Proposed tokens").

---

## Concept promise + bar to clear

> "The user owns the machine… Vibe Mode changes wallpaper, accent, sound, font,
> cursor, and animations as a coherent set." — RaeenOS_Concept.md (§Customization)

- **Bar to clear:** macOS System Settings (Ventura+) sidebar-list IA + Windows 11
  Settings' left-nav categories with a content pane. Plus Spotlight-grade
  fuzzy search at the top of the sidebar.
- **The Settings-specific promise:** the **Appearance & Vibe** page is the one
  place a user picks an accent seed and a Vibe preset and *the whole desktop
  changes coherently* — this page is the design language's control surface, so
  it is the showcase of this spec.

---

## Already built (delta only — verify-before-spec)

Grounded in the actual code, not assumptions. `control_panel.rs` is a large,
already-rich data model — this spec is a **re-skin + state-completeness + IA
re-grouping** layer over it, NOT a rebuild.

| Piece | Where | Today | This spec changes |
|---|---|---|---|
| Settings app (data model) | `components/raeshell/src/control_panel.rs` (`ControlPanel`, 2446 lines) | full page/category model, 11 categories, nav stack, search, profiles, MDM policy | → re-group IA (11 Windows-clone cats → 10 RaeenOS cats); re-skin render |
| Two-pane render | `control_panel.rs::ControlPanel::render` (l.2236) | sidebar (180px) + content; flat `CP_*` palette; 8px block glyphs; 28px rows | → `material.glass`/`mica`, token spacing/radius, real control kit, states |
| Control kit (enum) | `control_panel.rs::SettingControl` | Toggle / Slider / Dropdown / TextInput / ColorPicker / KeyBinding / RadioGroup / CheckboxGroup / Button / Link / InfoBar / ExpandableSection | → spec full visual states per control (high fan-out; reused by all apps) |
| Search | `control_panel.rs::SearchState::search` | substring match over title/desc/keywords + per-setting | → fuzzy ranking + Spotlight-style results panel; case-insensitive |
| Breadcrumb / nav | `control_panel.rs::NavigationState` (`breadcrumb`, back/forward/home) | LIVE | → render breadcrumb in content-pane header; tokenize |
| Hardcoded palette | `control_panel.rs` `CP_BG/CP_ACCENT/CP_FG/CP_SIDEBAR_BG/CP_SELECTED/CP_TOGGLE_*` (l.2219–2227) | private `const CP_ACCENT = 0xFF_4E_9C_FF` etc. | → **delete**, consume `rae_tokens` (the cohesion fix) |
| Accent / theme carrier | `kernel/src/theme_engine.rs::ThemeAbi` (`accent_argb`, 8 builtins) | LIVE | accent picker writes the seed; `derive_accent` ramps it |
| `derive_accent(seed, palette)` | `components/rae_tokens/src/lib.rs` | LIVE, host-KAT'd (RaeBlue ramp within ±2) | the accent picker calls this to preview the full ramp |
| Vibe presets | `components/raeshell/src/vibe_mode.rs` (`ALL_PRESETS`, **12** presets) | LIVE data; `VibeEngine::apply_preset` w/ transition lerp | → preset grid on the Appearance page |
| RGB engine | `kernel/src/rgb.rs` (9 effect modes) + `components/raeshell/src/rgb_api.rs` | LIVE (simulated devices) | → Power & Gaming page RGB sub-section |
| Per-game power / fan | Gaming page `game.gpu_power` slider + Phase 13.3 fan-curve | slider exists; fan-curve `[ ]` | → Power & Gaming page hosts these; surface only |
| Accessibility settings | EaseOfAccess pages (text scale, cursor, narrator, high-contrast, animations) | LIVE data | → Accessibility category; coordinate toggles w/ raeen-accessibility |
| Locale | `time.language` dropdown + `raelocale` | LIVE data | → System / Time & Language |

**Verify note:** project memory said "5 Vibe presets" — the code now ships **12**
(`vibe_mode::ALL_PRESETS`). The grid sizes to the live count, not a hardcoded 5.

---

## Prior art distilled (current systems, 2024–2025)

- **macOS System Settings (Ventura+):** single scrolling sidebar *list* (not the
  old grid) with SF Symbols icons; right pane is a single scrollable column of
  grouped "cards"; search filters the sidebar; controls use the system accent
  tint, neutral chrome. **Take:** the icon+label sidebar list, grouped cards in
  the content pane, accent-only-on-controls restraint. **Avoid:** the *over-long*
  flat sidebar — by Ventura the list scrolls past a screenful with no sub-grouping;
  RaeenOS caps the top-level list at ~10 categories and pushes depth into the
  content pane, not the sidebar.
- **Windows 11 Settings:** left-nav with ~10 clear top-level categories, each
  opening a content pane of expandable "setting cards"; a persistent search box
  pinned above the nav; breadcrumb trail in the content header. **Take:** the
  ~10-category ceiling, the persistent top search, the breadcrumb, the expandable
  card pattern (`SettingControl::ExpandableSection` already models this).
  **Avoid:** the deep `Settings > System > Display > Advanced > …` burrows that
  lose the breadcrumb; cap at 2 levels (category → page) like the current model.
- **GNOME Settings (libadwaita):** strict spacing, strong *visible* focus rings,
  generous row padding, a clean `AdwPreferencesGroup` (titled card with rows).
  **Take:** the titled-group-card structure and the always-visible focus ring
  (we owe this for a11y + controller). **Avoid:** the flatness — RaeenOS panels
  are glass/mica with depth.

**RaeenOS synthesis:** Win11's **clear ~10-category left nav + breadcrumb +
persistent fuzzy search**, macOS's **grouped-cards content pane + accent
restraint**, GNOME's **titled-group cards + visible focus**, all on the
`material.glass`/`material.mica` system the shell already uses — and the
Appearance page wires straight into `derive_accent` + Vibe Mode so the whole
desktop is owned from one screen.

---

## RaeenOS design tokens this surface uses

Pulled verbatim from `design-language.md` / `rae_tokens`. No new magic numbers.

- **spacing:** `space.2` (control padding, intra-group), `space.3` (row inset,
  control vertical pad), `space.4` (panel padding, group gap), `space.5` (section
  gap), `space.6` (content-pane margin).
- **radius:** `radius.md` (window corners — from `ThemeAbi.corner_radius`),
  `radius.lg` (group cards), `radius.sm` (controls, search field), `radius.xs`
  (sidebar rows, chips), `radius.pill` (toggle track + knob, segmented control).
- **elevation:** `elev.0` (content pane, flush), `elev.1` (group cards resting),
  `elev.2` (dropdown popover, color popover), `elev.3` (the Settings window
  itself / search results overlay), `elev.focus` (focused control glow).
- **type:** `type.title` (page title), `type.subtitle` (group header),
  `type.body` (row label + values), `type.label` (buttons, segmented),
  `type.caption` (row description, breadcrumb, hints).
- **accent model:** the seed is `ThemeAbi.accent_argb`; `rae_tokens::derive_accent`
  yields `accent.base/hover/active/subtle/text/glow`. Settings **reads the same
  ramp as the shell** — never a private `CP_ACCENT`.
- **material:** `material.mica` (the Settings window backdrop + sidebar — large,
  always-on, off the per-frame blur path); `material.glass` (transient popovers
  only: dropdown lists, color-picker popover, search-results overlay).
- **motion:** `motion.micro` (hover/press, focus ring, toggle knob slide),
  `motion.fast` (dropdown/popover open, page cross-fade), `motion.standard`
  (Settings window open), `motion.emphasized` (Vibe preset apply transition —
  matches `VibeEngine` lerp), `motion.instant` (reduced-motion fallback).

---

## 1. Window & layout (two-pane settings pattern)

**Bar to clear:** Win11 left-nav + content pane; macOS sidebar list.

### Geometry
- Settings is a normal app window: chrome per `window-chrome` spec
  (`radius.md` corners, 32px titlebar, `material.mica`, `elev.3` while active).
  Default size **920 × 640px**, min **720 × 480px**.
- **Two panes** inside the client (`bg.raised`):
  - **Sidebar** (left): width **240px** fixed (collapses to a 56px icon-rail
    below the 720px min-width breakpoint). `material.mica` (continuous with the
    titlebar). 1px `stroke.subtle` divider on its right edge.
  - **Content pane** (right): fills the remainder, `bg.raised`, `space.6` (32px)
    outer margin, vertically scrollable.

> **Proposed token:** the 240px sidebar / 56px icon-rail / 920×640 default are
> *layout dimensions specific to this surface*, not design-language tokens — they
> compose from `space.*` (240 = `space.6`×7.5; expressed as a surface constant in
> the implementer's layout, not a global token). No global token addition needed.

### Sidebar (top → bottom)
1. **Search field** — pinned at the top, full sidebar width minus `space.4`
   inset, height **32px** (pointer hit-target floor), `radius.sm`, `bg.elevated`
   fill, search glyph + placeholder "Search settings" (`text.tertiary`,
   `type.body`). Fuzzy, Spotlight-style: typing opens a `material.glass` results
   overlay (`elev.3`) over the content pane (see §3). Focus on `Ctrl+F` / app open.
2. **Category list** — one row per `SettingsCategory` (§2 IA). Each row: icon
   (`space.3` left inset) + label (`type.body`), row height **36px** (≥32px
   floor + padding), `radius.xs` on hover/selection fill, `space.1` vertical gap.
3. **Footer** — user avatar + display name (`type.label`, `text.secondary`) at
   the sidebar bottom, `space.4` inset; clicking opens the Accounts category.

### Content pane (top → bottom)
1. **Breadcrumb header** — `Settings › <Category> › <Page>` (`type.caption`,
   `text.tertiary`, the current leaf in `text.primary`), wired to
   `NavigationState::breadcrumb`. Back/forward chevrons left of it (32px targets,
   `radius.xs` hover) wired to `go_back`/`go_forward`/`can_go_*`.
2. **Page title** — `type.title`, `text.primary`, `space.4` below the breadcrumb.
   Optional one-line page description (`type.body`, `text.secondary`).
3. **Setting groups** — each a titled **group card**: `material` solid
   `bg.elevated`-tint card, `radius.lg`, `elev.1`, `space.4` internal padding,
   `space.5` between cards. Group header `type.subtitle`, `text.secondary`. Rows
   inside use the concentric child radius (`concentric(radius.lg, space.4)` =
   `radius.xs`).
4. **Setting rows** — label (`type.body` `text.primary`) + description below
   (`type.caption` `text.tertiary`) on the left; the **control** right-aligned.
   Row min-height **44px** (label + description + `space.3` padding; ≥32px floor).
   `requires_restart` rows show a `state.warn` "Restart needed" chip
   (`type.caption`). `mdm_locked` rows render disabled with a lock glyph (the
   `PolicyManager` already models this).

### States (window + panes)
- **default / hover / active:** see the control kit (§4) and category rows below.
- **dark / light:** all surfaces read the active `Palette` (`DARK`/`LIGHT`); the
  sidebar mica + content `bg.raised` + cards `bg.elevated` flip per palette.
- **reduced-motion:** page transitions become instant opacity swaps; no slide.

---

## 2. Category IA (the proposed RaeenOS list)

The current code has **11 Windows-clone categories** (System, Devices, Network,
Personalization, Apps, Accounts, Time & Language, Gaming, Ease of Access,
Privacy, Update & Security). This spec **re-groups to 10 RaeenOS-native
categories** — grounded in what the code actually has — to (a) lead with the
showcase, (b) merge the thin "Time & Language" + "Update & Security" + "Apps"
into clearer homes, and (c) name them the RaeenOS way.

| # | Category | Icon role | Maps to existing pages | Why |
|---|---|---|---|---|
| 1 | **Appearance & Vibe** | palette | `pers.colors`, `pers.background`, `pers.taskbar`, + new Vibe grid | Leads with the showcase — the Concept's signature surface (§5) |
| 2 | **Display** | monitor | `sys.display` | First-class; brightness/HDR/VRR are gaming-relevant, deserve top level (not buried in "System") |
| 3 | **Sound** | speaker | `sys.sound` | First-class (RaeAudio is a pillar) |
| 4 | **Network** | globe | `net.wifi`, `net.vpn`, `net.proxy`, Ethernet/status | As-is |
| 5 | **Bluetooth & Devices** | devices | `dev.bluetooth`, `dev.mouse`, `dev.printers`, USB | Merge Devices (macOS "Bluetooth & …" pattern) |
| 6 | **Power & Gaming** | controller | `sys.power`, `game.mode`, `game.bar`, `game.gpu_power`, RGB, fan-curve | RaeenOS-native merge: "gaming isn't a mode" — power + game tuning + RGB together |
| 7 | **Accounts** | person | `acc.info`, `acc.signin` | As-is |
| 8 | **Privacy & Security** | shield | `priv.*`, sign-in/biometric, RaeShield manifests | Merge Privacy + Update&Security security bits (RaeShield home) |
| 9 | **Accessibility** | accessibility | `access.display`, `access.narrator`, `access.highcontrast`, cursor | Renamed from "Ease of Access"; raeen-accessibility owns the contents |
| 10 | **System & About** | info | `sys.storage`, `sys.notifications`, `time.*` (date/region/language), `apps.*`, `sys.about`, updates/recovery | Catch-all for time/language/storage/apps/about/update — the macOS "General" |

**Implementer note:** this is an IA re-grouping of `SettingsCategory` + the
`category` field on each `SettingsPage`; the pages themselves and their
`SettingItem`s mostly survive. It is a data edit in `populate_pages`, not a
rewrite. The Win11 `UpdateSecurity`/`TimeLanguage` enum variants fold into
System & About; Privacy gains the security pages.

---

## 3. Search (Spotlight-style, fuzzy)

**Bar to clear:** Win11 settings search + macOS Spotlight feel.

- Trigger: type in the sidebar search field, or `Ctrl+F`.
- The existing `SearchState::search` does substring match over page
  title/description/keywords and per-setting label/description. **Delta:** make
  it (a) case-insensitive (lowercase both sides), (b) rank exact-prefix > word-
  start > substring, and (c) render results.
- **Results overlay** — a `material.glass` panel (`radius.lg`, `elev.3`,
  `motion.fast` open) over the content pane: each result row shows the matched
  setting label (`type.body` `text.primary`), its category › page breadcrumb
  (`type.caption` `text.tertiary`), highlighted match span in `accent.text`.
- States: arrow keys move `selected_index` (wired to `select_next/prev`),
  `Enter` navigates to that page + scrolls to the setting (`selected_result`),
  `Esc` clears (`SearchState::clear`). Selected row = `accent.subtle` fill +
  focus ring. Empty query = overlay closed.

---

## 4. The control kit (high fan-out — reused by ALL apps)

Each maps to an existing `SettingControl` variant. **These states are the
canonical control states for the whole OS** (raeen-ui owns the widgets). Every
control: 32px pointer hit-target floor, 48px in couch mode, a focus state
distinct from hover, and a defined reduced-motion path.

### 4.1 Toggle (`SettingControl::Toggle`)
- **Geometry:** track 40×22px `radius.pill`; knob 18px circle `radius.pill` with
  `elev.1`, `space.1` inset from the track edge.
- **default off:** track `bg.elevated`, knob `text.secondary`, 1px `stroke.subtle`.
- **default on:** track `accent.base`, knob `text.primary` (or white).
- **hover:** track lightens (off → `bg.overlay`; on → `accent.hover`),
  `motion.micro`.
- **active (pressing):** knob squashes ~10% width; track `accent.active` if on.
- **focus (keyboard/controller):** 2px `accent.base` ring + `elev.focus` glow.
- **disabled / mdm_locked:** track + knob `text.tertiary` @ 50%, no hover, lock glyph.
- **dark/light:** per palette.
- **reduced-motion:** knob jumps (no slide); fill swaps instantly.

### 4.2 Slider (`SettingControl::Slider { value, min, max, step }`)
- **Geometry:** track 4px tall `radius.pill`, full control width (min 120px);
  filled portion `accent.base`; knob 18px circle `radius.pill`, `elev.2`. Current
  value shown right of the track in `type.caption` `text.secondary`.
- **default:** unfilled track `bg.elevated`, filled `accent.base`.
- **hover:** knob grows to 20px, `motion.micro`; a `material.glass` value
  tooltip appears above the knob.
- **active (drag):** knob `accent.active`, value tooltip stays; snaps to `step`.
- **focus:** ring + glow on knob; `←/→` move by `step`, `Home/End` to min/max.
- **disabled:** track + knob `text.tertiary`; no drag.
- **reduced-motion:** no knob-grow; value updates instantly.

### 4.3 Segmented control (NEW visual; backs `RadioGroup` w/ ≤4 short options)
- **Geometry:** a `radius.pill` pill, `bg.elevated`, segments of equal width,
  selected segment a `radius.pill` `accent.base` fill sliding under the label.
- **default:** labels `text.secondary` (`type.label`); selected label
  `text.primary` on `accent.base`.
- **hover (unselected segment):** `bg.overlay` wash.
- **active:** selected fill slides (`motion.micro`) to the pressed segment.
- **focus:** ring around the whole control; `←/→` move selection.
- **disabled:** all `text.tertiary`.
- **reduced-motion:** selected fill jumps, no slide.
- *Use when ≤4 options fit; otherwise fall to the Dropdown.* (`RadioGroup` with
  many options renders as a vertical radio list with the same focus/hover rules.)

### 4.4 Dropdown / picker (`SettingControl::Dropdown { selected, options }`)
- **Closed:** a `radius.sm` `bg.elevated` field showing the selected option
  (`type.body` `text.primary`) + a chevron (`text.secondary`), min-width 140px.
- **hover:** field `bg.overlay`.
- **open:** a `material.glass` popover (`radius.md`, `elev.2`, `motion.fast`)
  listing options; current = `accent.subtle` fill + check glyph; hovered row =
  `bg.elevated`; `radius.xs` rows.
- **focus:** field ring; `Space/Enter` opens, `↑/↓` move, `Enter` commits, `Esc`
  cancels.
- **disabled:** field `text.tertiary`, no open.
- **reduced-motion:** popover appears instantly (opacity only).

### 4.5 Stepper (NEW; for bounded numeric like cursor size, scroll lines)
- **Geometry:** `[ − ] value [ + ]`, the two buttons 32px square `radius.sm`
  `bg.elevated`, value centered `type.body`. Backs a `Slider` with a small range
  or an integer field.
- **hover/active:** button `bg.overlay` / `accent.subtle`.
- **focus:** ring on the focused button; `↑/↓` or `±` keys adjust.
- **disabled:** `text.tertiary`. **reduced-motion:** value changes instantly.

### 4.6 Text field (`SettingControl::TextInput { value, placeholder, max_length }`)
- **Geometry:** `radius.sm` `bg.elevated` field, `space.3` inset, min-height 32px,
  `type.body`. Placeholder `text.tertiary`; entered text `text.primary`.
- **hover:** 1px `stroke.subtle` → slightly stronger.
- **focus:** 2px `accent.base` ring + caret in `accent.base`; `elev.focus` glow.
- **active (typing):** caret blink (suppressed under reduced-motion → solid caret).
- **disabled / locked:** `text.tertiary`, no caret. `max_length` enforced.
- **dark/light:** per palette.

### 4.7 Color / accent picker (`SettingControl::ColorPicker { r,g,b,a }`)
- **Closed:** a `radius.sm` swatch (28×20px) showing the current color, 1px
  `stroke.subtle` border, optional hex label (`type.caption` `text.tertiary`).
- **open:** a `material.glass` popover (`radius.md`, `elev.2`): a hue/sat field +
  a value slider + a hex text field + a row of preset swatches. Live-updates the
  swatch.
- **focus:** ring on the swatch; `Enter` opens.
- **reduced-motion:** popover instant; no animated field.
- **The accent variant** (used on the Appearance page, §5.1) additionally
  previews the derived ramp — see below.

### 4.8 Button / Link / InfoBar (existing variants)
- **Button:** `radius.sm`, `bg.elevated` resting / `accent.subtle` hover /
  `accent.active` pressed, `type.label`; destructive buttons use `state.danger`
  text + border. Focus ring + glow. Disabled `text.tertiary`.
- **Link:** `accent.text`, underline on hover; focus ring; opens via `Cap`-gated
  handler.
- **InfoBar:** a `radius.sm` tinted strip — `state.ok`/`state.warn`/`state.danger`/
  `accent.subtle` per `InfoSeverity`, icon + message (`type.body`). Non-interactive
  (no focus) unless it carries an action button.

---

## 5. Appearance & Vibe page (the showcase)

**Bar to clear:** macOS "Appearance" + "Wallpaper" panes; this is where the
RaeenOS design language becomes user-owned. One screen drives the whole desktop.

### 5.1 Accent seed picker (feeds `derive_accent` → `ThemeAbi.accent_argb`)
- A `ColorPicker` (§4.7) seeded from `ThemeAbi.accent_argb` (default `RAEBLUE`).
- **Live ramp preview:** below the swatch, a row of 6 chips showing
  `derive_accent(seed, palette)` → `base / hover / active / subtle / text / glow`
  (the exact tokens, labeled `type.caption`). Changing the seed re-derives all 6
  live — this *is* the cohesion engine made visible.
- A row of **preset accent swatches** (RaeBlue + the Vibe primaries) for one-click
  seeds. Selecting commits the seed to `ThemeAbi`; the shell re-skins (the §6 test).
- **Contrast guard:** if the chosen seed fails WCAG 4.5:1 as `accent.text` on the
  active `bg.base` (computed by `rae_tokens::contrast_ratio`), show a `state.warn`
  InfoBar "Low contrast — text will use the neutral color instead" (this is
  exactly the `derive_accent` fallback rule). **Flag to raeen-accessibility.**

### 5.2 Theme preset grid (Vibe Mode)
- A grid of preset tiles, one per `vibe_mode::ALL_PRESETS` (**12** today; grid
  sizes to the live count, 3 columns). Each tile: a mini swatch trio
  (`accent.primary/secondary/tertiary`) over the preset's wallpaper gradient,
  name (`type.label`), `radius.md`, `elev.1`.
- **default / hover:** hover lifts to `elev.2` + `motion.micro`.
- **selected (active vibe):** 2px `accent.base` border + check glyph.
- **focus (keyboard/controller):** ring + glow; `Enter` applies.
- **Apply transition:** selecting calls `VibeEngine::apply_preset`; the desktop
  cross-fades on `motion.emphasized` (matches the existing `tick_transition`
  lerp). **reduced-motion:** instant swap, no lerp.

### 5.3 Mode + wallpaper + a11y toggles
- **Appearance mode** — a 3-segment segmented control (§4.3): **Light / Dark /
  Auto** (Auto follows the time schedule that `VibeEngine` already supports).
- **Wallpaper** — picker for `WallpaperMode` (Static / Gradient / Solid / Live
  shader / Slideshow), backed by `WallpaperConfig`; gradient/solid use the color
  picker; slideshow exposes the interval `Slider`.
- **Reduced motion** — a `Toggle` that sets the system reduced-motion flag (every
  surface's `motion.instant` path keys off this). **Owned with raeen-accessibility.**
- **High contrast** — a `Toggle` + theme `Dropdown` (the existing
  `access.hc_*` settings surface here too, cross-linked). **Owned with
  raeen-accessibility** — high-contrast must override the accent ramp with a
  guaranteed-AA palette.

---

## 6. Cohesion acceptance (the whole-surface test)

Because incoherence is the top UI risk, Settings ships only when:
1. **Same accent:** Settings reads the *same* `accent.base` as the taskbar, Start,
   and window chrome — proven by changing the accent seed on the Appearance page
   and seeing the Settings selection highlight, the taskbar, and an open toast all
   change together (one `derive_accent` ramp, no private `CP_ACCENT`).
2. **Same radii/materials:** Settings window uses `radius.md` (= `ThemeAbi.corner_radius`)
   and `material.mica`/`material.glass` exactly as `desktop-shell` specifies —
   switch a Vibe preset with `corner_radius` override (Bauhaus = 0) and the
   Settings window corners change with everything else.
3. **Concentric radii:** group-card children are never sharper than
   `concentric(radius.lg, space.4)`.
4. **Focus everywhere:** every control in §4 has a focus state distinct from hover
   (raeen-accessibility sign-off), and the accent picker's contrast guard fires
   on a failing seed.
5. **Dark + light parity:** both palettes render with passing contrast.

---

## Proposed tokens

No new global tokens are required. All values compose from existing `rae_tokens`
constants. Surface-specific layout numbers (sidebar 240px / icon-rail 56px /
default window 920×640) are *local layout constants* for raeen-shell-apps, not
design-language tokens — they are expressed as multiples of `space.*` in the
layout code and deliberately NOT added to `design-language.md` (a one-off window
size is not a reusable token).

---

## Handoff

### Implementers
- **raeen-ui (framework):** own the **control kit** (§4) as reusable widgets
  consuming `rae_tokens` — Toggle, Slider, Segmented, Dropdown, Stepper, Text
  field, Color/accent picker, Button/Link/InfoBar, each with the full state set.
  This is the highest-fan-out deliverable: every app reuses it. Expose the
  group-card container (titled `AdwPreferencesGroup`-equivalent) and the
  two-pane settings scaffold.
- **raeen-shell-apps (the Settings surface):** in `control_panel.rs` —
  (a) delete the private `CP_*` palette + `GLYPH_*`, consume `rae_tokens`;
  (b) re-group `SettingsCategory`/`populate_pages` to the 10-category IA (§2);
  (c) re-skin `ControlPanel::render` to the two-pane glass/mica layout with the
  control kit; (d) render the breadcrumb from `NavigationState`; (e) build the
  fuzzy search results overlay (§3) over the existing `SearchState`;
  (f) build the Appearance & Vibe page (§5) including the preset grid over
  `vibe_mode::ALL_PRESETS`.
- **theme_engine (kernel):** wire the accent picker → `ThemeAbi.accent_argb`;
  expose the live ramp via `rae_tokens::derive_accent` for the §5.1 preview and
  the §6 cohesion proof; honor `corner_radius`/`blur_radius` overrides so the
  Settings window re-skins with the shell on a Vibe switch.
- **raeen-gfx:** confirm `Canvas` rounded-rect per-corner masking for group cards
  + popovers; confirm `material.glass` popover blur is bounded (transient only).
- **raeen-accessibility (flagged):** owns the **Accessibility** category contents;
  signs off the §4 focus states; verifies the §5.3 reduced-motion + high-contrast
  toggles actually change compositor output (MasterChecklist 23.x line 1263);
  verifies the §5.1 contrast guard; audits 32px/48px hit targets and AA contrast
  on both palettes.

### On-screen / boot-log evidence (raeen-visual-qa + smoketests)
- **Two-pane window:** QEMU screenshot of Settings open — 240px mica sidebar with
  the 10-category list + icons, search field on top, content pane with a breadcrumb,
  page title, and ≥2 glass group cards. Log: `[settings] open panes=2 sidebar=240
  cats=10 accent=0x..`.
- **Control kit:** a screenshot showing a Toggle (on + off), a Slider mid-drag, a
  Segmented control, an open Dropdown popover, a Text field focused, and a Color
  swatch — with hover/focus/disabled states visible side by side. Log:
  `[settings] controls: toggle slider segmented dropdown stepper text color` and a
  `control_panel::run_boot_smoketest` that asserts each control renders and reports
  its state (must be able to print FAIL).
- **Search:** screenshot of the glass results overlay for a query (e.g. "accent")
  with the match span highlighted + breadcrumb. Log: `[settings] search "accent"
  hits=N ranked=true case_insensitive=true`.
- **Appearance & Vibe:** screenshot of the accent seed picker with the 6-chip
  derived ramp preview + the 12-tile Vibe preset grid + the Light/Dark/Auto
  segmented control. Log: `[settings] appearance: seed=0x.. ramp=[base,hover,
  active,subtle,text,glow] vibe_tiles=12`.
- **Cohesion (the §6 test):** before/after screenshots of changing the accent
  seed — Settings selection highlight + taskbar + an open toast all change to the
  new `accent.base` together. Log: extend `vibe_mode`/`theme_engine` smoketest to
  assert `settings.accent == taskbar.accent == chrome.accent == derive_accent(seed).base`.
- **Accessibility:** screenshot of the contrast-guard `state.warn` InfoBar firing
  on a low-contrast seed; screenshot of high-contrast toggle visibly changing the
  Settings render. Log: `[settings] a11y: contrast_guard=fired hc_toggle=applied`.

### Unblocks (MasterChecklist)
- **Phase 8.1/8.2 (RaeUI/RaeKit):** the control kit + group-card scaffold are the
  reusable widget set RaeUI owes; the theming hook (8.2) lands here.
- **Phase 13.1 (Customization):** the Vibe preset grid + accent seed picker make
  "Vibe Mode includes wallpaper, accent, sound, fonts, cursor, animations"
  user-drivable (l.1073–1074: the noted "Settings UI tile" gap closes here).
- **Phase 13.3:** Power & Gaming page hosts the RGB section + the per-game GPU
  power slider + fan-curve UI (l.1089/1094).
- **Phase 14.2:** "Settings — exists, basic" `[~] → [x]` (l.1121).
- **Phase 14.4 acceptance:** "click Settings, change wallpaper, end to end, no
  terminal" (l.1139) — the Appearance page is the wallpaper/accent surface that
  closes it.
- **Phase 21/Accessibility:** "Magnifier + high-contrast toggle from Settings and
  visibly change the compositor output" (l.1263) — §5.3 is that surface.
