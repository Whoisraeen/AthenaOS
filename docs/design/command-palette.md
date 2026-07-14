# Design Spec: Command Palette (instant launcher + action runner)

> *"Built for people who care about how things feel."* — RaeenOS_Concept.md
>
> A single keystroke summons a floating glass field; you type, and the OS answers
> instantly — launch an app, jump to a setting, open a file, do a sum. It must
> clear: **macOS Spotlight's instant-everywhere feel**, with **VSCode/rofi's
> action-running power** layered on top.

**All tokens below are defined in [`design-language.md`](./design-language.md)**
and live in `rae_tokens` (ADR 0003). This spec only *assigns* them; it introduces
no new magic numbers. Where a number is surface-specific layout (panel width), it
is a local constant composed from `space.*`, explicitly NOT a new global token.

---

## Concept promise + bar to clear

> "Familiar enough to switch from Windows or Mac in 10 minutes." — RaeenOS_Concept.md
> (§Three User Experiences) + "Fast is a feature."

- **Bar to clear:** macOS Spotlight (`Cmd+Space` → instant fuzzy launch + inline
  calculator + system actions) **and** the VSCode command palette / rofi (run an
  *action*, not just open a thing). RaeenOS fuses them: one surface that both
  **launches** (apps, files) and **runs** (settings jumps, system actions),
  ranked together, dispatched by one Enter.

---

## Already built (delta only — verify-before-spec)

Grounded in code, not assumption. The backing engine **exists and is currently
dead** (`#![allow(unused)]`); this surface *wires it to a window*, it does not
build a search engine.

| Piece | Where | Today | This spec adds |
|---|---|---|---|
| Search engine | `components/raeshell/src/search_indexer.rs::SearchEngine` / `SearchIndexer` | LIVE data model, **unwired** (`allow(unused)`). `quick_search(&str)`, TF-IDF + trigram + Levenshtein fuzzy, app/settings/file/contact/bookmark indices, **inline calculator** (`evaluate_calculator`), latency tracking (`p99_latency_us`, target 100ms) | a window that calls `quick_search` per keystroke and renders `SearchResult` rows |
| Result model | `search_indexer.rs::SearchResult { entry_type, title, subtitle, icon, path, score, highlights, action }` | LIVE — already carries `highlights: Vec<(usize,usize)>` for match-span emphasis and a typed `SearchAction` | rendered directly; no new fields needed |
| Action dispatch | `SearchAction::{Open, Launch, Navigate, Calculate, WebSearch}` | LIVE enum | the Enter handler maps each variant to the shell's existing launch / settings-nav / clipboard handlers (§ Dispatch) |
| Result categories | `SearchResultType::{Application, Setting, File, Folder, Calculator, …}` | LIVE | drives the category label + icon role per row |
| Glass material / blur / shadow | `compositor::set_surface_blur`, `SurfaceEffect::DropShadow` | LIVE | reused as `material.glass` + `elev.3` |
| Crisp AA text | `raegfx::Canvas::draw_text` (Inter) | LIVE | the query field + rows render with it (not the 8px block path) |

**This is a wire-up, not a rebuild.** The engine, fuzzy ranking, calculator, and
the typed result/action model already exist; what is missing is (a) a global
hotkey, (b) a floating glass surface, and (c) the Enter→dispatch mapping.

---

## Prior art distilled (current systems, 2024–2025)

- **macOS Spotlight:** centered floating field, top-third of screen; instant
  results as you type; inline calculator/conversion; top hit is auto-selected and
  Enter fires it; categories are grouped with subtle headers. **Take:** centered
  top-third placement, auto-select-top-hit, inline calc. **Avoid:** the web/Siri
  results creeping in and pushing local hits down (we keep local first; web is a
  single explicit "Search the web for …" row at the bottom, never auto-ranked up).
- **VSCode command palette:** `>` runs commands, plain text finds files; match
  characters are **bolded** in each row; arrow + Enter; a one-line "no results".
  **Take:** match-span emphasis (`SearchResult.highlights` already provides it),
  the run-an-action model, keyboard-only flow. **Avoid:** the mode-prefix
  (`>`/`@`/`#`) cognitive load — RaeenOS ranks apps/actions/files in *one* list,
  no prefix required (a category filter is optional, not mandatory).
- **rofi / Albert (Linux):** dmenu-style, extremely fast, plugin actions, strong
  keyboard focus line. **Take:** the "everything is an action" breadth + the
  unmissable selection row. **Avoid:** the spartan, theme-incoherent look — ours
  is glass and reads the live accent.
- **Windows 11 Start search:** combines apps/settings/files/web in one box but is
  *slow to appear* and web-biased. **Take:** the unified categories. **Avoid:**
  the latency and the web bias (`SearchEngine` already targets <100ms p99).

**RaeenOS synthesis:** Spotlight's *placement and instancy* + VSCode's *match
emphasis and action-running* + rofi's *keyboard-first breadth*, rendered as one
glass surface that reads the live accent — no mode prefixes, local-first ranking.

---

## RaeenOS design tokens this surface uses

Pulled verbatim from `design-language.md` / `rae_tokens`. No new magic numbers.

- **spacing:** `space.2` (icon→text gap, row inset), `space.3` (row vertical pad,
  field inset), `space.4` (panel padding), `space.5` (gap between field and list).
- **radius:** `radius.lg` (the palette panel), `radius.sm` (the query field),
  `radius.xs` (result rows, category chips).
- **elevation:** `elev.3` (the floating palette — it is a transient modal over the
  desktop), `elev.focus` (the selected-row accent glow).
- **type:** `type.subtitle` (the query text the user types — large and legible),
  `type.body` (result title), `type.caption` (result subtitle/path + category
  label + the "↩ open / ⏎ run" hint footer).
- **accent model:** seed is `ThemeAbi.accent_argb`; `rae_tokens::derive_accent`
  yields the ramp. Selected row = `accent.subtle` fill; match-span emphasis =
  `accent.text`; selection ring/glow = `accent.base` + `accent.glow`. **No private
  `const ACCENT`.**
- **material:** `material.glass` (live blur — the palette is small and transient,
  exactly the doctrine case for live blur).
- **motion:** `motion.standard` (open: scale 96%→100% + fade + 8px down-translate),
  `motion.exit` (close: faster fade + scale to 96%), `motion.micro` (selection
  move between rows; row hover), `motion.instant` (reduced-motion fallback).

---

## 1. Invocation

- **Primary hotkey:** `Super+Space` (mirrors Spotlight's `Cmd+Space`; `Super` is
  already the Start key in `desktop-shell`, so this is the natural sibling chord).
- **Alternate:** `Ctrl+Space` is reserved (IME); the palette also opens from a
  Start-menu "Search everything" affordance and from typing in the Start search
  field promoting to the full palette.
- **Toggle semantics:** pressing the hotkey while open **closes** it (no second
  surface). Opening focuses the query field and selects all existing text so a new
  query replaces the last.
- The palette is a **single global instance**, always centered on the focused
  monitor; it does not remember position (it is not a window the user arranges).

---

## 2. The floating glass surface

**Bar to clear:** Spotlight's centered field that feels like it belongs to the OS,
not to an app.

### Geometry
- **Width:** **640px** (local layout constant = `space.6` × 20; clamped to
  screen − `space.6` margin on small displays). Fixed; does not grow with query.
- **Vertical placement:** anchored so the **query field top sits at 28% of screen
  height** (Spotlight's top-third feel — high enough to not collide with the
  taskbar when the result list grows downward).
- **Query field height:** **52px** (well above the 32px pointer floor; this is the
  hero input — generous like Spotlight).
- **Result list:** drops *below* the field, same width, each row **44px**
  (≥32px floor + room for title + subtitle). **Max 8 rows visible**
  (`quick_search` already caps at 10; show 8, the rest scroll). Total surface
  height = field + (rows × 44) + `space.4` padding, animating as results change.
- **Material:** `material.glass` — `radius.lg`, blur 16 (`GLASS_BLUR_RADIUS`),
  tint per palette (`GLASS_TINT_DARK`/`GLASS_TINT_LIGHT`), 1px `stroke.strong`
  top-edge highlight, 1px `stroke.subtle` on the other edges. Shadow `elev.3`.
- **Backdrop:** the rest of the desktop dims ~12% (`scrim.modal`, defined in
  `design-language.md` §4) so the glass reads against busy wallpaper (the macOS-
  Liquid-Glass legibility lesson).

### Query field (top)
- A search glyph (`text.tertiary`) at `space.4` left inset, then the input text in
  **`type.subtitle`** (`text.primary`), caret in `accent.base`. Placeholder
  "Search apps, settings, files — or do math" in `text.tertiary`.
- Rendered with `Canvas::draw_text` (crisp AA Inter), not the block-glyph path.
- A right-aligned category-filter chip cluster is **optional/deferred** (the no-
  prefix doctrine). If shipped later, chips are `radius.xs`, `accent.subtle` when
  active — but the default is *no filter*, everything ranked together.

---

## 3. Result rows

Each row renders one `SearchResult`. **Bar to clear:** VSCode's bolded match +
Spotlight's icon/title/subtitle clarity.

### Row anatomy (left → right)
1. **Icon** (28px box, `space.4` left inset) — keyed off `entry_type`
   (`Application`/`Setting`/`File`/`Folder`/`Calculator`). `result.icon` if
   present, else a category glyph in `text.secondary`.
2. **Title** (`type.body`, `text.primary`) — `result.title`. The matched spans
   from `result.highlights` render in **`accent.text`** (the match emphasis;
   `highlights` is already a `Vec<(usize,usize)>` byte-range list).
3. **Subtitle** (`type.caption`, `text.secondary`) below the title — `result.subtitle`
   (path for files, description for apps/settings, "Calculator" for math).
4. **Category label** (`type.caption`, `text.tertiary`, right-aligned) — a short
   tag from `entry_type`: "App" / "Setting" / "File" / "Folder" / "Math".
5. **Action hint** (right edge, only on the *selected* row): a `type.caption`
   `text.tertiary` glyph pair showing what Enter does — "↩ Open" for launch/open,
   "→ Go" for a settings Navigate, "⧉ Copy" for a Calculate result.

### Ranking display
- Rows are pre-sorted by `SearchEngine` (TF-IDF + fuzzy + recency boosts). The
  **top hit is auto-selected** (Spotlight behavior) so a blind `type → Enter`
  launches the best match. A `Calculator` result always sorts to the top with its
  big inline answer (the engine already scores it at 100).
- No visible score number. Ranking is *expressed* purely by order + the top-hit
  selection.

### States (per row)
- **default:** transparent over glass.
- **hover (pointer):** `bg.elevated` fill @ `radius.xs`, `motion.micro`.
- **selected (keyboard/auto-top-hit):** `accent.subtle` fill + 2px `accent.base`
  left-edge bar + `elev.focus` glow; the action hint appears. This is distinct
  from hover (hover = neutral fill; selected = accent).
- **active (Enter/click fired):** brief `accent.active` flash, then the palette
  dismisses on `motion.exit`.
- **dark/light:** glass tint + text tokens per palette.
- **reduced-motion:** selection moves with no slide; open/close are opacity-only.

---

## 4. Result categories

The engine returns a flat ranked list spanning categories; the row's `entry_type`
labels it. The three first-class categories for v1 (matching what the indices
actually populate):

| Category | `SearchResultType` | Action on Enter | Source index |
|---|---|---|---|
| **Apps** | `Application` | `SearchAction::Launch(exec)` → shell app-launch | `AppIndex` |
| **Settings actions** | `Setting` | `SearchAction::Navigate(path)` → open Settings at that page | `SettingsIndex` |
| **Files** | `File` / `Folder` | `SearchAction::Open(path)` → file manager / default app | `FileIndex` |
| **Math** (inline) | `Calculator` | `SearchAction::Calculate(result)` → copy answer to clipboard | `evaluate_calculator` |

> **Note on "Settings actions":** because settings are indexed as navigable
> targets, this *is* the action-runner story — typing "night light" jumps you
> straight to the toggle, the VSCode-command-palette experience. Contacts /
> bookmarks indices exist in the engine and can light up later; v1 ships the four
> above so the surface has a clear acceptance bar.

---

## 5. Dispatch: launch-vs-action (the Enter contract)

Enter (or click) fires the **selected** row's `result.action`. The handler is a
single match on `SearchAction`:

- `Launch(exec)` → shell process-launch (the same path the Start grid uses).
  Palette closes immediately on `motion.exit`.
- `Open(path)` → open in the file manager / default handler. Palette closes.
- `Navigate(path)` → open **Settings** and route `NavigationState` to that page
  (`control_panel`'s nav), scrolling to the setting. Palette closes.
- `Calculate(value)` → **copy** the result to the clipboard (kernel `clipboard::set`,
  syscall 108) and show a brief `state.ok` confirmation in the field ("Copied 42").
  Palette stays open one beat then closes — math is a glance-and-grab flow.
- `WebSearch(q)` → reserved; renders as a single explicit bottom row, never
  auto-selected.

**`Cap`-gating:** every dispatch routes through the shell's existing
capability-checked launch/open handlers — the palette never bypasses `RaeShield`.

---

## 6. Keyboard + controller navigation map

- `Super+Space` — open/close (toggle).
- type — live filter (`quick_search` per keystroke; debounce not needed at <100ms
  p99, but coalesce to one query per frame).
- `↓` / `↑` — move selection (`motion.micro`); wraps at ends.
- `Enter` — fire selected row's action (§5).
- `Tab` — (optional) cycle category filter; default no-op in the no-prefix model.
- `Esc` — close (`motion.exit`), discard query.
- `Ctrl+C` on a `Calculator` row — copy the answer (same as Enter for math).
- **Controller (couch):** D-pad up/down = selection, A = fire, B = close; rows use
  the 48px couch hit-target floor in couch mode and the `elev.focus` glow reads at
  3m (SteamOS lesson). Couch palette is a mode-scaled variant of the same surface.

---

## 7. Empty / no-results states

- **Empty query (just opened):** show a hint block instead of rows — a short
  `type.caption` list of example queries ("calc 12*8", "night light", "Terminal")
  and the most-recent / most-used apps as quick-launch rows (the engine's
  recency/usage boosts already exist). No scrim of blank rows.
- **No results:** a single centered row, `type.body` `text.secondary`: "No results
  for '<query>'." with a `type.caption` `text.tertiary` subrow offering "Press ↩
  to search the web" (the explicit `WebSearch` fallback, never auto-ranked). The
  surface does **not** collapse to just the field — it keeps one row of height so
  it doesn't flicker as the user types through a non-matching prefix.

---

## 8. Accessibility (in scope from the start)

| Concern | Rule | Owner |
|---|---|---|
| Contrast | query text `type.subtitle` ≥7:1; subtitles ≥4.5:1; match-emphasis `accent.text` must itself clear 4.5:1 on `accent.subtle` (use the `derive_accent` contrast fallback) | raeen-accessibility |
| Focus visibility | selected row is **never color-only**: `accent.subtle` fill **+** 2px `accent.base` left bar **+** `elev.focus` glow | raeen-accessibility |
| Reduced-motion | open/close opacity-only; selection jumps; no scale/translate | raeen-accessibility |
| Hit targets | 44px rows (pointer) / 48px (couch); query field 52px | raeen-visual-qa |
| Keyboard-complete | every action reachable with no pointer (the surface is keyboard-first by design) | raeen-accessibility |

Flag to **raeen-accessibility:** confirm the match-span `accent.text` emphasis
stays legible when the accent is re-seeded via Vibe Mode (low-saturation accents
must fall back to a bold weight, not just color).

---

## 9. Cohesion acceptance

The palette ships only when:
1. It reads the **same `accent.base`** as the taskbar / Start / Settings — switch
   one Vibe preset and the selected-row fill + match emphasis change with the
   whole shell.
2. Glass material, `radius.lg`, and `elev.3` match the Start menu exactly (it is
   the same transient-glass family).
3. The selected state is visibly distinct from hover.
4. Dark and light both pass contrast.

---

## Handoff

### Implementer
- **raeen-shell-apps** — owns the surface. Wire `search_indexer::SearchEngine`
  (remove its `allow(unused)` dead status) to a new floating palette window:
  global `Super+Space` hotkey, the glass field + result list render via
  `Canvas` + `set_surface_blur` + `elev.3`, per-keystroke `quick_search`, the
  §3 row renderer reading `SearchResult.highlights`, and the §5 `SearchAction`
  dispatch into the existing launch/Settings-nav/clipboard handlers.
- **raeen-ui** (supporting) — the result-row widget (icon + emphasized title +
  subtitle + category tag + selection state) is reusable; expose it from
  `raeui::tokens`-consuming widgets so Settings search (`settings.md` §3) and the
  Start search share it.
- **raeen-accessibility** (flagged) — match-emphasis contrast under re-seeded
  accents; focus-state and reduced-motion audit.

### FAIL-able boot-log proof line
The implementer emits, from a `run_boot_smoketest` that drives a synthetic query
through the wired engine (must be able to print FAIL):

```
[cmdpalette] smoketest: query="calc 6*7" hits=N top=Calculator(42) launch_ok=1 nav_ok=1 -> PASS
```

(FAIL if the engine returns zero hits for a seeded app, if the calculator row is
not top-ranked for an arithmetic query, or if a `Launch`/`Navigate` dispatch
returns an error.) A second line asserts cohesion:
`[cmdpalette] accent=0x.. == derive_accent(seed).base -> PASS`.

### Visual-QA verification list (raeen-visual-qa)
- QEMU screenshot: palette open over wallpaper — glass blur visible, 640px field
  at ~28% height, scrim dimming the desktop, placeholder text crisp.
- Screenshot: a query showing ≥3 rows across categories (an app, a setting, a
  file) with the **top row selected** (accent fill + left bar + glow) and **match
  spans emphasized** in the accent.
- Screenshot: an arithmetic query ("12*8") with the `Calculator` row top with its
  inline answer.
- Screenshot: the no-results state (single row + web fallback hint).
- Cohesion: before/after a Vibe preset switch — selected-row accent changes with
  the taskbar/Start.

### Unblocks (MasterChecklist)
- Phase 8 (RaeUI/RaeKit): the shared result-row widget + glass-popover pattern.
- Phase 14 (RaeShell + apps): the launcher/command surface; activates the
  currently-dead `search_indexer`.
