# Design Spec: Files

> *"Built for people who care about how things feel."* — RaeenOS_Concept.md
>
> The file manager is the surface power-users live in. It must clear: **macOS
> Finder's column-view + Quick Look cleanness, Windows 11 Explorer's tabbed
> familiarity, and the GNOME Files / Dolphin split-pane power** — without copying
> Explorer's ribbon clutter or Finder's hidden-by-default chrome.

**All tokens below are defined in [`design-language.md`](./design-language.md)**
and live in the `rae_tokens` crate (ADR 0003). This spec only assigns them; it
introduces no new magic numbers.

---

## Concept promise + bar to clear

> "RaeFS — CoW, snapshots, tiered storage, per-app data buckets, versioned
> config." — RaeenOS_Concept.md (§RaeFS)

- **Bar to clear:** Finder (column view, Quick Look spacebar preview, tag
  sidebar, instant fuzzy search) + Windows 11 Explorer (tabs, address-bar
  breadcrumb, details pane) + Dolphin/Nautilus (split pane, embedded terminal).
- **The RaeenOS-specific promise:** RaeFS snapshots and per-app buckets are
  first-class — the Files app is the *only* place a normal user sees "restore a
  previous version" and "this folder belongs to app X". That sidebar section is
  the showcase that no other OS has natively.

---

## Already built (delta only — verify-before-spec)

Grounded in `components/raeshell/src/file_manager.rs` (a large, already-rich
data model). This spec is a **re-skin + state-completeness + AA-text + token
consolidation** layer, NOT a rebuild.

| Piece | Where | Today | This spec changes |
|---|---|---|---|
| File manager (data model) | `file_manager.rs` (icon/list/details/column/tree views, tabs, dual-pane, ops+progress, batch rename, search, preview, thumbnails, trash, bookmarks, shares) | full model exists | → re-skin render, AA text, token states |
| Private palette | `file_manager.rs` `FM_BG/FM_SIDEBAR_BG/FM_ACCENT/FM_HOVER/FM_SELECTED/…` (l.21–41) | ~20 hardcoded `const FM_*` incl. `FM_ACCENT = 0xFF_4E_9C_FF` | → **delete**, consume `rae_tokens` (the cohesion fix) |
| Block glyphs | `file_manager.rs` `GLYPH_W/GLYPH_H = 8` (l.42–43) | 8×8 bitmap font | → AA Inter via `Canvas::draw_text` (raefont path, already live for OOBE/chrome) |
| File-type color/icon | `FileEntryType::color()` / `icon_char()` (l.67–102) | private type colors (`FM_DIR_FG` etc.) | → keep the *semantic* mapping; remap to a tokenized type-accent palette (§4) |
| Views | icon / list / details / column / tree | all modeled | → spec spacing/row geometry + states per view (§2) |
| Tabs + dual-pane | tab strip + split | modeled | → tab-strip chrome + split divider tokens (§3) |
| Search | substring over entries | exists | → fuzzy ranking + glass results, Finder/Spotlight feel (§5) |

The Files app is **not a rebuild** — it is the same `control_panel.rs` story:
delete the private palette, render with `rae_tokens` + AA text, complete the
hover/active/focus/disabled/dark/light/reduced-motion state matrix.

---

## Prior art distilled (current systems, 2024–2025)

- **macOS Finder (Sequoia):** four-pane column view (each column a directory
  level, last column = Quick Look preview); spacebar = full Quick Look overlay;
  unified toolbar with back/forward + view-switcher + search; left sidebar with
  Favorites / iCloud / Locations / Tags sections, each a titled group; ~24px
  list rows, generous left inset, SF Symbols file icons; accent only on
  selection + sidebar-active. **Take:** column view, spacebar Quick Look,
  titled-section sidebar, accent restraint. **Avoid:** chrome that hides until
  hover (path bar off by default) — RaeenOS shows the breadcrumb always.
- **Windows 11 Explorer:** tabs (the headline 11 feature), address-bar
  breadcrumb with clickable segments + dropdown chevrons, command bar (not the
  old ribbon), details/preview pane toggle, ~28px rows on a 4px grid, Mica
  window. **Take:** tabs, clickable breadcrumb segments, the slim command bar,
  details pane. **Avoid:** the residual right-click → "show more options" two-tier
  context menu (RaeenOS ships one flat, complete context menu).
- **GNOME Files / KDE Dolphin:** Dolphin's split-pane (two independent panes,
  one toolbar), embedded terminal panel, and breadcrumb-as-buttons; Nautilus's
  strong focus rings + recursive search + batch rename. **Take:** split-pane as a
  first-class toggle, embedded-terminal affordance (RaeenOS has a terminal app to
  dock), batch rename, visible focus. **Avoid:** Dolphin's information density
  knobs sprawl — RaeenOS exposes 3 view densities, not a settings burrow.

**RaeenOS synthesis:** Explorer's **tabs + clickable breadcrumb**, Finder's
**column view + spacebar Quick Look + titled-section sidebar + accent
restraint**, Dolphin's **split-pane toggle**, all on the `material.mica` window +
`material.glass` transient-popover system the shell uses — plus a **RaeFS
sidebar section** (Snapshots / per-app Buckets) that is uniquely ours.

---

## RaeenOS design tokens this surface uses

Pulled verbatim from `design-language.md` / `rae_tokens`. No new magic numbers.

- **spacing:** `space.1` (icon-to-label), `space.2` (intra-row padding, tab gap),
  `space.3` (list-row inset, sidebar-row inset), `space.4` (pane padding, sidebar
  section gap), `space.5` (toolbar-to-content gap).
- **radius:** `radius.md` (window corners = `ThemeAbi.corner_radius`), `radius.sm`
  (search field, tabs, toolbar buttons), `radius.xs` (list rows, sidebar rows,
  icon-grid cells, chips), `radius.lg` (Quick Look overlay card), `radius.pill`
  (view-density segmented control).
- **elevation:** `elev.0` (panes flush), `elev.1` (toolbar resting), `elev.2`
  (breadcrumb dropdown, context menu, details popover), `elev.3` (Files window
  itself / Quick Look overlay / fuzzy-search overlay), `elev.focus` (focused
  cell/row glow).
- **type:** `type.subtitle` (sidebar section headers), `type.body` (file names,
  breadcrumb segments), `type.label` (toolbar buttons, tab labels, column
  headers), `type.caption` (size/date/type metadata, status bar).
- **accent model:** seed = `ThemeAbi.accent_argb`; `rae_tokens::derive_accent`
  yields `accent.base/hover/active/subtle/text/glow`. Files reads the **same
  ramp** as the shell — never a private `FM_ACCENT`. File-type colors are a
  separate fixed *semantic* palette (§4), deliberately NOT accent-derived
  (directories must stay recognizable across Vibe presets).
- **material:** `material.mica` (the Files window backdrop + toolbar + sidebar —
  large, always-on, off the per-frame blur path); `material.glass` (transient
  popovers only: breadcrumb dropdown, context menu, Quick Look overlay,
  fuzzy-search results).
- **motion:** `motion.micro` (row/cell hover + press, focus ring), `motion.fast`
  (tab switch cross-fade, context menu open, breadcrumb dropdown), `motion.standard`
  (Quick Look overlay open, window open), `motion.instant` (reduced-motion).

---

## 1. Window & layout (toolbar + sidebar + content + optional details)

**Bar to clear:** Explorer tabs+breadcrumb; Finder column+sidebar.

### Geometry
- Files is a normal app window: chrome per `window-chrome` spec (`radius.md`
  corners, 32px titlebar, `material.mica`, `elev.3` while active). Default size
  **1040 × 680px**, min **640 × 420px**.
- **Tab strip** (under the titlebar, only when >1 tab): height **34px**, tabs
  `radius.sm` top corners, `space.2` gaps, active tab `bg.raised` + 2px top
  `accent.base` rule, inactive `bg.overlay` `text.secondary`; `+` new-tab button
  32px square `radius.xs`.
- **Toolbar** (height **44px**, `material.mica`, `elev.1`): back / forward / up
  chevrons (32px targets, `radius.xs` hover) | **breadcrumb** (clickable
  segments, each a `radius.xs` hover target; a chevron between segments opens a
  `material.glass` sibling-folder dropdown — the Explorer pattern) | spacer |
  **view-density segmented control** (Icons / List / Columns) `radius.pill` |
  **search field** (`radius.sm`, `bg.elevated`, 200px, expands on focus).
- **Sidebar** (left): width **220px** (collapses to 0 below 640px), `material.mica`,
  1px `stroke.subtle` right divider. Titled sections (§6).
- **Content pane:** fills remainder, `bg.raised`, `space.4` inset, scrollable.
- **Details pane** (right, toggleable): width **300px**, `material.mica`, shows
  the selected file's thumbnail + metadata + RaeFS version history.
- **Status bar** (bottom, 24px): item count + selection size (`type.caption`,
  `text.tertiary`).

### States (window-level)
- **dark / light:** every surface reads the active `Palette`; mica + `bg.raised`
  + `bg.elevated` flip per palette.
- **reduced-motion:** tab/view/overlay transitions become instant opacity swaps.

---

## 2. Views (the three densities + tree)

All rows/cells: 32px pointer hit-target floor, a focus state distinct from hover,
a defined reduced-motion path. Selection is `accent.subtle` fill + (multi-select)
a 1px `accent.base` ring on the lead item.

### 2.1 Icons (grid)
- Cell: icon **48px** + name below (`type.body`, 2-line clamp, centered),
  `radius.xs`, `space.3` grid gaps, cell padding `space.2`.
- **hover:** `bg.elevated` fill, `motion.micro`. **selected:** `accent.subtle`.
- **focus (keyboard/controller):** 2px `accent.base` ring + `elev.focus` glow;
  arrow keys move across the grid, `Enter` opens, `Space` = Quick Look.

### 2.2 List
- Row height **32px**, icon **20px** + name (`type.body`) left at `space.3`
  inset; secondary metadata (date) right-aligned `type.caption` `text.tertiary`.
- **hover** `bg.elevated`; **selected** `accent.subtle`; alternating rows MAY use
  a 3%-alpha zebra (`stroke.subtle`) — off by default.

### 2.3 Details (columns)
- Column headers `type.label` `text.secondary`, sortable (active sort shows a
  chevron in `accent.text`). Rows 32px, columns: Name | Size | Type | Modified.
- Resizable column dividers (1px `stroke.subtle`, hover `accent.base`).

### 2.4 Columns (Finder-style miller columns)
- Each column **220px**, 1px `stroke.subtle` dividers; selecting a directory in
  column N opens column N+1; the final selection's column shows a **preview**
  (thumbnail + metadata). Selected row per column = `accent.subtle`; the *active*
  column's selection is full `accent.base` ring, prior columns dimmed selection.

### 2.5 Tree (sidebar-embedded or content)
- Disclosure triangles (`text.secondary`, rotate `motion.micro` on expand;
  reduced-motion = instant), 16px indent per level, rows 28px.

---

## 3. Tabs & split-pane

- **Tabs:** §1 geometry. Drag to reorder; middle-click closes; `Ctrl+T` new,
  `Ctrl+W` close, `Ctrl+Tab` cycle. Active tab carries the 2px `accent.base` top
  rule (the running-indicator pattern, consistent with the taskbar).
- **Split-pane:** toggle splits the content pane into two independent panes with
  their own selection, sharing one toolbar; a **draggable divider** (4px,
  `stroke.subtle`, hover→`accent.base`, `radius.pill` grip). The *focused* pane
  shows the toolbar breadcrumb + a 1px `accent.subtle` inner top edge; the
  unfocused pane dims its selection. Drag-and-drop between panes is the headline
  copy/move affordance (a `material.glass` `accent.subtle` drop-target ghost on
  the receiving pane, `motion.fast`).

---

## 4. File-type semantic palette (fixed, NOT accent-derived)

The existing `FileEntryType::color()` maps each type to a hue. Keep the *mapping*;
re-express as a small fixed semantic palette that survives Vibe re-skins (a
directory must look like a directory in every theme). These are **proposed
additions to `design-language.md` §4.4 "file-type semantics"** — they are
reusable (other apps show file chips) so they belong in the language, not inline:

| Token (PROPOSE to DESIGN_LANGUAGE) | Role | Maps from |
|---|---|---|
| `ftype.dir` | directories | `FM_DIR_FG` → use `accent.base` (the one type that DOES track accent — folders read as "primary") |
| `ftype.exec` | executables | `FM_EXEC_FG` `0xFF_44_DD_66` → `state.ok` |
| `ftype.media` | image/video/audio | the warm/violet `FM_IMAGE/VIDEO/AUDIO_FG` cluster → a single `ftype.media` violet `0xFF_C0_7C_FF` |
| `ftype.doc` | documents/pdf | `FM_DOC_FG` `0xFF_FF_CC_66` → `state.warn`-adjacent gold |
| `ftype.archive` | archives | `FM_ARCHIVE_FG` `0xFF_FF_AA_33` |
| `ftype.code` | code | `FM_ACCENT` → keep `accent.base` |
| `ftype.neutral` | plain/unknown/device/socket | `text.secondary` |

**Decision flag:** collapsing image/video/audio into one `ftype.media` reduces
the current rainbow (4 distinct hues) to a calmer palette — this is a *premium
restraint* call; if raeen-visual-qa wants per-medium hues back, keep 3 media
tokens. Default: collapse. **raeen-accessibility** verifies each `ftype.*` clears
3:1 on `bg.raised` (icons are non-text but should still read).

---

## 5. Search (fuzzy, Finder/Spotlight feel)

- Trigger: toolbar search field or `Ctrl+F`. Scope toggle: "This folder" /
  "Everywhere" (`raeshell::search_indexer` backs the recursive case).
- **Delta over current substring search:** case-insensitive; rank exact-prefix >
  word-start > substring > fuzzy-subsequence; debounce 120ms.
- **Results:** filter the current view live (icons/list/details), OR — for
  "Everywhere" — a `material.glass` results overlay (`radius.lg`, `elev.3`,
  `motion.standard`) with each row: file icon + name (match span in `accent.text`)
  + breadcrumb path (`type.caption` `text.tertiary`). Arrow keys move selection,
  `Enter` reveals + opens, `Esc` closes.

---

## 6. Sidebar (titled sections — incl. the RaeFS showcase)

Finder-style titled groups (`type.subtitle` `text.secondary` headers, collapsible):

1. **Favorites** — Home, Desktop, Documents, Downloads, Pictures (bookmarks model
   exists). Rows 32px, icon `space.3` inset + label `type.body`; active location
   = `accent.subtle` fill + 2px left `accent.base` rule.
2. **Locations** — mounted volumes + network shares (volume model exists);
   eject affordance on hover (32px target).
3. **Snapshots (RaeFS — the showcase)** — RaeFS snapshots for the current folder:
   each a row "2026-06-21 14:30" (`type.body`) + "restore" on hover. Selecting
   shows that snapshot's contents read-only in the content pane with a
   `state.warn` InfoBar "Viewing a snapshot — read only". **This is the surface
   no other OS has natively** (RaeFS CoW snapshots are live per memory
   `raefs-snapshot-cow`).
4. **App Buckets (RaeFS)** — per-app data buckets (`RaeFS` per-app buckets are a
   Concept pillar): each app's sandboxed data dir, shown so a user can see/clear
   what an app stores. Cross-links to Privacy & Security in Settings.

States per sidebar row: hover `bg.elevated`; active `accent.subtle` + left rule;
focus ring + glow; reduced-motion instant.

---

## 7. Quick Look (spacebar preview — the Finder win)

- `Space` on a selection opens a centered `material.glass` overlay card
  (`radius.lg`, `elev.3`, `scrim.modal` behind it, `motion.standard` open
  scale-96→100 + fade): renders image/text/pdf/font preview via the existing
  preview/thumbnail model + the `image_viewer`/`media_player` apps for media.
- `Space` / `Esc` closes (`motion.exit`); arrow keys move to the next selection
  *without closing* (Finder behavior). **reduced-motion:** instant show/hide.

---

## 8. Cohesion acceptance (the whole-surface test)

Files ships only when:
1. **Same accent:** the Files selection highlight, the active-tab rule, and the
   active-location sidebar rule all read the *same* `accent.base` as the taskbar,
   Start, Settings, and window chrome — proven by switching one Vibe preset and
   seeing them all change (no private `FM_ACCENT`).
2. **Same radii/materials:** the window uses `radius.md` (= `ThemeAbi.corner_radius`)
   and `material.mica`/`material.glass` exactly as the shell spec — a Bauhaus
   (radius 0) Vibe preset squares the Files window with everything else.
3. **Concentric radii:** grid cells / rows inside the content pane are never
   sharper than their container − padding.
4. **AA text everywhere:** no 8×8 block glyphs remain — file names, breadcrumb,
   metadata all render via `Canvas::draw_text` (the OOBE/chrome AA path).
5. **Focus everywhere:** every row/cell/control has a focus state distinct from
   hover (raeen-accessibility sign-off).
6. **Dark + light parity:** both palettes render with passing contrast.

---

## Proposed tokens

- **PROPOSE to `design-language.md` §4 — a new §4.4 "file-type semantics"**: the
  `ftype.*` palette in §4 above (7 tokens, fixed, NOT accent-derived except
  `ftype.dir`/`ftype.code`). Reusable beyond Files (any app showing a file chip),
  so it belongs in the language. Update `DESIGN_LANGUAGE` before implementers
  consume it.
- Surface-specific layout numbers (1040×680 default, 220px sidebar, 300px details
  pane, 220px column width) are *local layout constants* for raeen-shell-apps,
  composed from `space.*`, deliberately NOT global tokens.

---

## Handoff

### Implementers
- **raeen-ui (framework):** the **tab strip**, **breadcrumb-with-dropdown**,
  **split-pane divider**, **view-density segmented control**, and **titled-section
  sidebar** are reusable widgets consuming `rae_tokens` — build them in the control
  kit (shared with `control_panel.rs`). High fan-out: the terminal/editor apps
  reuse tabs + split.
- **raeen-shell-apps (the Files surface):** in `file_manager.rs` — (a) delete the
  private `FM_*` palette + `GLYPH_*`, consume `rae_tokens`; (b) swap block glyphs
  for `Canvas::draw_text` AA (raefont); (c) re-skin all five views to the token
  geometry + state matrix (§2); (d) build the breadcrumb dropdown, split-pane
  focus model, tab chrome (§1, §3); (e) wire the RaeFS Snapshots + App Buckets
  sidebar sections (§6) to the live `raefs` snapshot/bucket APIs; (f) build the
  fuzzy search + Quick Look overlay (§5, §7).
- **raeen-gfx:** confirm `Canvas` rounded-rect per-corner masking for grid cells +
  the Quick Look card; confirm `material.glass` popover blur is bounded (transient
  only); the soft-shadow fix (see `material-and-shadow.md`) must land first or
  Quick Look/context menus inherit the hard-block shadow defect.
- **theme_engine (kernel):** honor `corner_radius`/`blur_radius` overrides so the
  Files window re-skins with the shell on a Vibe switch; expose the same
  `derive_accent` ramp Files reads for the §8 cohesion proof.
- **raeen-accessibility (flagged):** the §4 `ftype.*` 3:1 audit; the focus-state
  audit across all five views + sidebar; reduced-motion paths; 32px/48px hit
  targets; AA contrast on both palettes.

### On-screen / boot-log evidence (raeen-visual-qa + smoketests)
- **Window:** QEMU screenshot of Files open — mica toolbar with clickable
  breadcrumb, 220px mica sidebar with titled sections, content pane in List view,
  status bar. Log: `[files] open view=list sidebar=220 tabs=1 accent=0x..`.
- **Views:** four screenshots — Icons grid, List, Details (sorted), Columns
  (Miller) — each with hover + selected + keyboard-focus states visible.
  Log: `[files] views: icons list details columns tree` + a
  `file_manager::run_boot_smoketest` that renders each view and asserts row
  geometry (must be able to print FAIL).
- **AA text:** zoom crop proving file names/breadcrumb render as AA Inter, no
  8×8 blocks. Log: `[files] text=aa glyphs=real`.
- **Tabs + split:** screenshot of 3 tabs (active tab accent rule) and the
  split-pane with a drop-target ghost mid-drag. Log: `[files] tabs=3 split=2
  active_tab_rule=accent`.
- **RaeFS sidebar:** screenshot of the Snapshots section listing ≥1 snapshot +
  the read-only `state.warn` InfoBar when viewing one. Log: `[files] raefs:
  snapshots=N buckets=M`.
- **Quick Look:** screenshot of the spacebar overlay over an image, glass card +
  scrim. Log: `[files] quicklook: open kind=image`.
- **Cohesion (the §8 test):** before/after one Vibe preset switch — Files
  selection + active tab rule + active sidebar location all change accent with the
  taskbar. Log: extend `vibe_mode`/`theme_engine` smoketest to assert
  `files.accent == taskbar.accent == derive_accent(seed).base`.

### Unblocks (MasterChecklist)
- **Phase 8.1/8.2 (RaeUI/RaeKit):** tabs / breadcrumb / split-pane / sidebar are
  reusable widgets RaeUI owes.
- **Phase 14 (RaeShell + apps):** the Files app polish from `[~]`-basic to a
  premium daily-driver surface.
- **Phase 5 (RaeFS UX):** the Snapshots + App Buckets sidebar makes CoW snapshots
  and per-app buckets *user-visible* — the only native surface for them.
- **Consumer Production Gate "Switcher":** a Finder/Explorer-rival file manager is
  table stakes for someone leaving macOS/Windows.
