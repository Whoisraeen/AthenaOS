# Design Spec: window-management (Spaces + Overview + Snap Groups + live App Switcher)

> Companion to `docs/design/design-language.md` (the canonical token set). Every
> value below is a token name from that file — no new magic numbers. When this
> spec needed a value the language did not define, it is flagged "PROPOSE to
> DESIGN_LANGUAGE" inline; nothing is inlined silently.

This surface spans the **compositor/shell boundary**. Read the Handoff section
first if you only care about who builds what: the *mechanism* (surface buffers,
placement, animation tick) is kernel/compositor; the *policy and chrome*
(grids, labels, gestures, save/restore sets) is the shell. The single rule that
keeps it GPU-cheap: **the overview and the switcher reuse the compositor's
existing per-surface buffers — they never ask apps to re-render.**

---

## Concept promise + bar to clear

> "Your desktop, your rules — tiling, stacking, floating are POLICIES over the
> compositor, not forks of it; switching is one call, not a different OS."
> — `RaeenOS_Concept.md` §RaeUI (already quoted at the top of `wm_policy.rs`)

> "Built for people who care about how things feel." — the thesis. Multitasking
> *is* how the machine feels under load; expose/spaces is where "fast is a
> feature" becomes visible.

Bar to clear, per surface:
- **Overview/Expose** must rival **macOS Mission Control** (live thumbnails,
  drag-between-spaces, gesture-driven) — and beat it on *honesty* (every
  thumbnail is the literal last frame the compositor already holds, never a
  stale snapshot).
- **Spaces** must rival **macOS Spaces** + **Windows 11 Virtual Desktops**
  (per-monitor, per-space wallpaper, move-window-to-space).
- **Snap Groups** must rival **Windows 11 Snap Groups** (save a window set,
  restore it as one unit) — built *on top of* the zone origins `wm_policy.rs`
  already computes, not a second layout engine.
- **App switcher** must rival **macOS Cmd-Tab** / **Win Alt-Tab Task-View hover
  previews** — the current text-only list is the thing being replaced.

---

## Prior art distilled

- **macOS Mission Control + Spaces:** one gesture (3-finger swipe up) zooms all
  windows of the current space into a non-overlapping grid using *live*
  thumbnails; spaces sit in a top strip you scrub between; drag a thumbnail onto
  a space to move the window; each space can carry its own wallpaper; spaces are
  per-display. Enter/exit is a single spring that scales each window from its
  real screen rect to its grid cell (the "magic" is that the start and end rects
  are both real, so it reads as the *same* windows moving, not a montage).
  **Take:** thumbnail = real surface buffer scaled into a cell; animate from the
  window's actual rect to its cell rect; per-space wallpaper; per-display.
  **Avoid:** Mission Control's grid reflows unpredictably as you hover (windows
  jump) — we lock the grid the moment overview opens.
- **Windows 11 Task View + Virtual Desktops + Snap:** Task View is a grid of
  live previews + a desktop strip; Snap Layouts appear on hover over the
  maximize button (a flyout of zone templates); **Snap Groups** remembers the
  set of windows that were snapped together so hovering one taskbar entry
  re-summons the whole group. Named desktops, per-desktop wallpaper.
  **Take:** the *Snap Group = saved window set per display* model (this is the
  best idea Windows has here and macOS lacks it); hover-to-zone Snap Layouts as
  the discoverable entry to tiling. **Avoid:** Task View's slow open animation
  and its mixing of "timeline/history" into the spaces UI — we keep overview
  strictly spatial (live windows only, no history clutter).
- **GNOME Activities overview:** one hot-corner/Super press → windows zoom to a
  grid *and* the workspace strip appears on the right *and* search is one
  keystroke away — a single unified surface. Strong, legible. **Take:** unify
  overview + spaces strip + (later) search into ONE surface so there is one
  "show me everything" gesture, not three. **Avoid:** GNOME forcing the dash/
  search into the same view can feel busy on a small screen — make the spaces
  strip collapsible.
- **KDE (Overview / Present Windows / Grid):** offers tiling-by-template and a
  configurable grid; proves a desktop can ship multiple overview layouts as
  policy. **Take:** overview layout is a `WmMode`-adjacent policy choice, not
  hardcoded.
- **SteamOS / Big Picture:** the couch case — overview must be navigable with a
  d-pad and a single always-visible focus glow that reads at 3 meters; no hover.
  **Take:** every overview/switcher affordance has a controller-focus equivalent
  with `elev.focus` glow (per DESIGN_LANGUAGE §8 hover-independence). **Avoid:**
  desktop density in couch mode — couch overview uses the 48px hit-target floor.

---

## RaeenOS design tokens (the canonical set this surface uses)

All names are from `docs/design/design-language.md` / `rae_tokens`. This surface
introduces **no new color/spacing/radius/motion values**.

- **spacing:** grid gutter between thumbnails = `space.4` (16); overview outer
  margin = `space.6` (32); spaces-strip item gap = `space.2` (8); switcher tile
  gap = `space.3` (12).
- **radius:** thumbnail card = `radius.md` (12, matches a real window's
  `ThemeAbi.corner_radius`); spaces-strip space chip = `radius.sm` (8); switcher
  panel = `radius.lg` (16, it is a `material.glass` transient surface).
- **elevation:** resting thumbnail = `elev.2`; **focused/hovered** thumbnail and
  the selected switcher tile = `elev.focus` (`elev_focus(accent.glow)` — the
  accent-tinted additive glow, the *only* focus signal that is not color-only);
  the switcher panel itself = `elev.3`.
- **material:** overview backdrop = `scrim.modal` (`0x1F_000000`, ~12% dim) over
  the live desktop so thumbnails read without hiding the wallpaper; the switcher
  panel = `material.glass` (live blur 16px + `bg.overlay` tint + 1px
  `stroke.strong` top highlight). Thumbnails themselves are opaque (they are
  real window pixels) with a 1px `stroke.subtle` border.
- **type:** thumbnail label (app title) = `type.label` (13/500); space name in
  the strip = `type.caption` (11/400); switcher app title = `type.subtitle`
  (17/500).
- **accent model:** selection/focus uses `accent.base` ring (2px) + `accent.glow`
  shadow via `derive_accent(ThemeAbi.accent_argb)` — **never** a hardcoded
  `0xFF_4E_9C_FF` (the current `render_alt_tab_overlay` hardcodes exactly this at
  `shell_runner.rs:1986`; that literal is the bug this spec retires).
- **motion:**
  - Overview enter = `motion.standard` (220ms, decelerate): each thumbnail
    interpolates `rect_screen → rect_cell` (translate + uniform scale) on this
    curve; backdrop scrim cross-fades `0 → 0x1F` on the same tick.
  - Overview exit = `motion.exit` (120ms, accelerate): cells fly back to
    `rect_screen`; ~40% faster than entry (DESIGN_LANGUAGE §7 rule).
  - Space switch = `motion.emphasized` (320ms): the outgoing space slides out and
    the incoming slides in horizontally by one screen width (direction = sign of
    target − current index); per-space wallpaper cross-fades underneath.
  - Switcher open = `motion.fast` (140ms) scale 96%→100% + fade (it is a
    `material.glass` flyout); tile-to-tile selection move = `motion.micro`
    (90ms).
  - **reduced-motion:** all of the above collapse to `motion.instant` (0ms) —
    thumbnails appear in final position, space switch is a hard cut with only the
    wallpaper opacity cross-fade retained (DESIGN_LANGUAGE §7/§8).

---

## 1. Overview / Expose

### Behavior
Hotkey (default **Super**, see keymap below) toggles overview for the focused
display's **current space**. All non-minimized userspace surfaces of that space
zoom from their real screen rects into a non-overlapping grid of live
thumbnails. A collapsible **spaces strip** docks at the top (§2). Click/Enter a
thumbnail → exit overview onto that window (focused). Drag a thumbnail onto a
space chip → move that window to that space (§2). Esc or Super → exit to the
previously focused window.

### Grid layout (policy — shell side, reuses the existing layout math)
Reuse `wm_policy::compute_layout(WmMode::Tile, grid_w, grid_h, windows)` to get
non-overlapping cell origins — the **near-square `cols = ceil(sqrt(n))`,
row-major** grid it already computes (`wm_policy.rs:78–100`). Overview differs
from live tiling in exactly one way: a thumbnail is *scaled to fit its cell with
the window's aspect ratio preserved and `space.4` gutters*, whereas live tiling
places the window at the cell origin at native size. So overview is "Tile policy
origins + aspect-fit scale," not a new engine. The grid is **frozen** at open
(snapshot the window list once) so cells never reflow under the cursor — the
macOS bug we avoid.

### GPU-cheap thumbnail sourcing (the load-bearing constraint)
**Do NOT re-render windows.** Each `Surface` already owns its last fully-painted
pixels at `Surface.kernel_ptr` (BGRA, `width*height*4`, see
`compositor.rs:1481–1501`). A thumbnail is a *downscaled read* of that buffer.
Two ways, in preference order:

1. **Compositor-side downscale into the frame (preferred, zero new buffers):**
   `recomposite` already composites every surface into `comp_buf` each frame.
   When overview is active, the compositor composites each surface **scaled into
   its cell rect** instead of at its native rect — same per-pixel blend path,
   just a scaled source walk (nearest or 2x2-box; `raegfx::Canvas` already has
   scaled-AA sampling per DESIGN_LANGUAGE §0). No extra allocation, no extra
   pass: the thumbnail *is* the composite. This is the macOS-grade "the same
   windows are moving" effect for free.
2. **One-shot snapshot copy (fallback / for the switcher):** a new compositor
   call `snapshot_surface(id, dst, dst_w, dst_h)` box-downscales
   `Surface.kernel_ptr` into a caller-provided buffer once. Used by the app
   switcher (§4), which wants a stable small preview that is not re-sampled every
   frame.

The **delta** is small (see "Already built"): the compositor needs an
"overview mode" flag + per-surface destination rect so its existing composite
loop scales instead of 1:1-blits. The animation just interpolates that
destination rect from `rect_screen` to `rect_cell` over `motion.standard`.

### Animation
Driven by a compositor-side tick (the compositor already owns the frame clock).
On open: for each surface, `dst_rect(t) = lerp(rect_screen, rect_cell, ease(t))`
with `ease = motion.standard.ease`, `t` advancing by `frame_dt / 220ms`. On
close: `lerp(rect_cell, rect_screen, ease_exit(t))` over 120ms. Backdrop scrim
alpha tracks `t`. Because both endpoints are real rects, no app involvement and
no re-layout — pure compositor geometry.

### Gesture (later — depends on trackpad, PARITY §O is `[ ]`)
3-finger swipe up = open overview, swipe down = close, swipe left/right while in
overview = scrub spaces. Flagged as **blocked on trackpad gesture support**
(PARITY matrix O "Trackpad gestures" is `[ ]`); ship hotkey first, wire the
gesture when raeen-input lands the gesture stream. Do not block overview on it.

---

## 2. Virtual desktops / Spaces

### Model
A **Space** is a named set of surface ids + an optional per-space wallpaper id,
scoped **per display**. This is the concept `wm_policy.rs` explicitly lacks
today (PARITY: "`wm_policy.rs` has no workspace concept"). Introduce it as shell
state with a thin compositor hook:

- Shell owns `struct Space { name, wallpaper_id, member_surface_ids }` and an
  ordered `Vec<Space>` per display + a `current` index. Default: one space named
  "1" holding all current windows (zero-migration upgrade — existing single-space
  behavior is just "one space").
- The compositor needs only **per-surface visibility by space**: a surface that
  is not a member of the current space is `visible = false` (it already has the
  `Surface.visible` flag at `compositor.rs:1490`, and `list_userspace_surfaces`
  already filters on it at `compositor.rs:2457`). Switching spaces = flip
  `visible` for the two membership sets + swap the wallpaper surface + animate.

### Move-window-to-space
From overview: drag a thumbnail onto a space chip. Programmatic:
shell removes the id from the source space's members, adds it to the target's,
toggles `Surface.visible` accordingly, recomposites. No window destruction, no
re-render — membership + visibility only.

### Per-space wallpaper
The wallpaper is itself a `z=0` kernel surface (created via
`create_kernel_surface`, the same path the desktop shell already uses). Each
space references a wallpaper surface id; space switch cross-fades the outgoing
wallpaper surface's alpha to 0 while the incoming goes 0→1 over
`motion.emphasized`. If a space has no custom wallpaper it shares the default —
no extra cost.

### Switch animation
`motion.emphasized` (320ms): outgoing space's member surfaces translate by
`−screen_w * dir`, incoming translate from `+screen_w * dir` to 0, where
`dir = sign(target − current)`. Implemented with the existing
`set_surface_origin` per frame (the same call `wm_policy::apply_to` uses at
`wm_policy.rs:128`) — no new surface mechanism. Reduced-motion: hard cut +
wallpaper opacity cross-fade only.

### Per-monitor
Spaces are per-display: switching a space on display 0 leaves display 1
untouched (the macOS model, which users prefer over Windows' all-displays
switch). `screen_dimensions()` is single-display today; multi-display spaces are
gated on the multi-output compositor work — ship single-display spaces now,
structure the `Vec<Space>` keyed by display id so multi-display is additive.

---

## 3. Snap Groups (save/restore a window set per display)

### Built on existing zone origins
`wm_policy::compute_layout` already returns `Vec<(id, x, y)>` cell origins for
Tile/Stack (`wm_policy.rs:70`). A **Snap Group** captures the *result* of a
layout as a reusable set: `{ display_id, mode: WmMode, members: Vec<(surface_id
or app_identity, cell_index)> }`. Restoring re-runs the same policy and re-binds
each member to its cell.

- **Save (shell):** "Save layout" captures the current managed-window set:
  current `WmMode`, the ordered surface ids (their grid order *is* their cell
  assignment, since `compute_layout` is row-major and deterministic), and the
  app identity behind each surface (`surface_owner(id)` →
  `compositor.rs:2432`, plus the app bundle id, so restore can re-launch a
  missing member). Name it.
- **Restore (shell):** for each member, if its surface still exists, reuse it;
  if the app is running with a different surface, bind that; if absent, launch
  the app bundle. Then call `wm_policy::apply_to(&ordered_ids)` — the **existing
  live-apply path** (`wm_policy.rs:112`) — which feeds the ids through
  `compute_layout` and moves each via `set_surface_origin`. Snap Group restore
  is therefore "reconstruct the id list, then call the function that already
  works."

### The honest gap: client resize
`wm_policy.rs` places windows at cell origins but **keeps their size** (the
TODO at `wm_policy.rs:16–19`: true tiling needs a surface-resize protocol).
Snap Groups inherit this: today a restored group reproduces *positions*, not
*sizes*, until the resize-negotiation protocol lands (PARITY A "Snap/tile
layouts" `[~]`). Spec the group format to store the cell **rect** (origin+size)
now, so when resize negotiation arrives, restore fills the cell with zero format
change. Flag: **client-resize is the one piece Snap Groups cannot fully honor
until the surface-resize protocol exists** — that protocol is a separate
raeen-ui/raeen-kernel item, not this surface's to build.

### Snap Layouts (hover-to-zone, the discoverable entry)
On hover over a window's maximize button (chrome lives in `window_chrome.rs`), a
`material.glass` flyout (`elev.2`, `radius.md`) shows zone templates (2-up,
3-up, 2x2 — these map directly to `WmMode::Tile` with N=2/3/4 + a "quad"
arrangement). Releasing over a zone calls `set_surface_origin` for that cell.
This is the Windows-11 affordance; it is pure shell chrome over the existing
placement math. Couch/controller equivalent: a "Snap" item in the window's
context menu navigable by d-pad with `elev.focus` on the highlighted zone.

---

## 4. App-switcher live previews (upgrade `cycle_alt_tab`)

### What exists (the thing being replaced)
`kernel/src/shell_runner.rs:1926 cycle_alt_tab` + `1950 render_alt_tab_overlay`:
on Alt+Tab it sorts `list_userspace_surfaces()` by z-order, advances
`alt_tab_index`, focuses the surface, and draws a **text-only** list overlay
(app titles via `surface_title`). State lives in `ShellRunnerState.alt_tab_open`
/ `alt_tab_index` (`shell_runner.rs:62–63`). It **hardcodes** the selection
color `0xFF_4E_9C_FF` (`shell_runner.rs:1986`) — a DESIGN_LANGUAGE cohesion
violation.

### Upgrade (delta, not rebuild)
Keep the entire state machine and the Alt+Tab cycling. Change only the overlay:
1. **Live preview tiles instead of text rows.** For each surface, draw a
   `radius.lg` `material.glass` row containing a small live thumbnail +
   `type.subtitle` title. The thumbnail comes from `snapshot_surface` (§1
   fallback path) — a one-shot box-downscale of `Surface.kernel_ptr` into the
   overlay buffer, refreshed each time the index advances (not per frame; the
   switcher is short-lived). This reuses the *same* buffer-read primitive as
   overview, so building overview's `snapshot_surface` unblocks the switcher.
2. **Tokenized selection.** Replace `0xFF_4E_9C_FF` with
   `derive_accent(ThemeAbi.accent_argb).base` for the ring +
   `.glow` for `elev.focus` on the selected tile. Now the switcher re-skins with
   Vibe Mode like everything else.
3. **Type-to-filter (later).** Once live keyboard text reaches the shell, typing
   narrows the tiles by title substring (the `search_indexer` is overkill here —
   substring is enough). Flag as a follow-up; the tile overlay is the prerequisite
   and ships first.
4. **Reduced-motion / couch:** selection move uses `motion.micro`, collapses to
   `motion.instant`; tiles are ≥48px tall in couch mode; d-pad navigates,
   `elev.focus` is the cross-room focus signal.

The overlay panel is a `material.glass` transient surface (`elev.3`,
`bg.overlay` tint, top `stroke.strong` highlight) centered on the focused
display — replacing the current opaque `0xE0_1A_1D_28` rectangle at
`shell_runner.rs:1969`.

---

## States & interaction

| State | Overview thumbnail | Switcher tile | Space chip |
|---|---|---|---|
| default | `elev.2`, 1px `stroke.subtle` | glass row, no glow | `radius.sm`, `text.secondary` label |
| hover | `elev.focus` glow, scale +2% (`motion.micro`) | n/a (keyboard/d-pad driven) | `bg.elevated` fill |
| active/selected | 2px `accent.base` ring + `elev.focus` | 2px `accent.base` ring + `elev.focus` | `accent.subtle` fill + `accent.base` ring |
| focus (kbd/controller) | identical to selected — focus is never color-only | identical | identical |
| disabled | n/a (no disabled windows in overview) | n/a | dimmed `text.tertiary` |
| dark / light | both palettes per DESIGN_LANGUAGE §4.1/§4.2; scrim alpha unchanged dark/light (it dims the live desktop, not chrome) | same | same |
| reduced-motion | thumbnails appear in final cell, no scale/translate | no scale/fade, instant index jump | hard cut, wallpaper cross-fade only |

### Keyboard + controller navigation map

| Action | Keyboard | Controller (couch) |
|---|---|---|
| Open/close overview | `Super` (toggle), `Esc` (close) | `View`/`Select` button |
| Next/prev window in switcher | `Alt+Tab` / `Alt+Shift+Tab` (existing) | bumper L/R |
| Move focus within overview grid | arrow keys | d-pad |
| Activate focused thumbnail | `Enter` | `A` |
| Next/prev space | `Super+Ctrl+→ / ←` (Win parity) | d-pad on spaces strip |
| Move window to space N | `Super+Shift+N` | drag thumbnail to chip |
| Save Snap Group | `Super+Shift+S` (proposal) | context menu |
| Snap window to zone | `Super+←/→/↑` (half/quarter) | "Snap" context-menu item |

Every hover affordance above has a keyboard + controller equivalent
(DESIGN_LANGUAGE §8 hover-independence). Focus is always `accent.base` ring +
`elev.focus` glow, never color-only — flag any deviation to
**raeen-accessibility**.

---

## Already built (delta only)

Verified by reading the source — wire these, do not rebuild:

- **Placement engine — EXISTS.** `wm_policy::compute_layout` (Tile near-square
  grid, Stack cascade, Float no-op) and the live-apply path
  `apply_to(&ids) → set_surface_origin` (`wm_policy.rs:70,112,128`). Overview grid
  and Snap Group restore both call these. **Delta:** none for placement.
- **Per-surface buffers — EXIST.** `Surface.kernel_ptr` holds each window's last
  BGRA frame (`compositor.rs:1486`); the double-buffered scanout
  (`scanout_ready`/`scanout_backbuf`, swapped under the IF=0 guard,
  `compositor.rs:3080`) and `recomposite` already walk every surface each frame.
  **Delta:** add an "overview mode" flag + per-surface destination rect to the
  composite loop so it *scales* a surface into a cell instead of 1:1-blitting;
  add `snapshot_surface(id, dst, w, h)` (one-shot box-downscale) for the switcher.
- **Surface visibility + listing — EXIST.** `Surface.visible`
  (`compositor.rs:1490`), `list_userspace_surfaces` filtering on it
  (`compositor.rs:2457`), `set_surface_origin` (`2353`), `focus_surface` (`2463`),
  `surface_frame` (`2442`), `surface_owner` (`2432`), `surface_title` (`2419`).
  **Delta:** Spaces is *membership state in the shell* + visibility flips — no new
  compositor surface model.
- **App switcher — EXISTS (text-only).** `cycle_alt_tab` +
  `render_alt_tab_overlay` + `ShellRunnerState.alt_tab_open/alt_tab_index`
  (`shell_runner.rs:62,1926,1950`). **Delta:** swap the text-row renderer for the
  live-preview glass overlay; replace the hardcoded `0xFF_4E_9C_FF`
  (`shell_runner.rs:1986`) with `derive_accent(...).base`.
- **Glass + shadow + scrim — EXIST.** `set_surface_blur` / `BlurRegion`
  (`compositor.rs:2552`), `SurfaceEffect::DropShadow`, and the `elev.*` ladder in
  `rae_tokens` (`elev_focus(glow)` at `rae_tokens/src/lib.rs:394`). **Delta:**
  none — apply the tokens.
- **Tokens — EXIST.** `SPACE_*`, `RADIUS_*`, `ELEV_*`, `TYPE_*`, `MOTION_*`,
  `derive_accent` all in `rae_tokens` (`lib.rs:29–504,241`). **Delta:** none — no
  new tokens needed for this surface.

**Net delta to build:** (1) compositor overview-mode scaled-composite +
`snapshot_surface`; (2) shell-side `Space`/`Snap Group` state + the overview
grid/strip chrome + the upgraded switcher overlay; (3) the animation tick that
interpolates destination rects. Everything else is wiring.

---

## Handoff

### Implementer split (this surface crosses the compositor/shell line)

**raeen-kernel / raeen-gfx (compositor side — the mechanism):**
- `compositor.rs`: add `overview_set_mode(on: bool)` + a per-surface
  `overview_dst: Option<Rect>` so the existing composite loop scales each surface
  into its cell rect (reuse `raegfx::Canvas` scaled-AA sampling). Animate by
  advancing `overview_dst` toward `rect_cell` on the frame clock.
- `compositor.rs`: add `snapshot_surface(id, dst: *mut u8, dst_w, dst_h)` — a
  one-shot box-downscale of `Surface.kernel_ptr`, capability-gated, reads only
  compositor-owned memory under `lock_compositor()` (same UAF-safety discipline
  as the scanout swap at `compositor.rs:3080`).
- `compositor.rs`: per-space wallpaper opacity cross-fade hook (the wallpaper is
  already a `z=0` surface; expose `set_surface_alpha(id, a)` if not present).
- Keep all of it on the existing IF=0 → unlock-before-scanout structure; no new
  per-frame allocations (DESIGN_LANGUAGE compositor-latency doctrine + memory
  `compositor-IF=0`).

**raeen-shell-apps (shell side — the policy + chrome):**
- `Space`/`Vec<Space>` per-display state + current index + space-switch driver
  (toggle `visible`, `set_surface_origin` slide, wallpaper cross-fade).
- Overview grid chrome (call `wm_policy::compute_layout(Tile,…)` for cells),
  spaces strip, drag-to-space, thumbnail labels.
- `SnapGroup` save/restore (capture ordered ids + app identity → store cell
  rects → `wm_policy::apply_to` on restore) + the hover Snap-Layouts flyout over
  `window_chrome` maximize.
- Upgrade `cycle_alt_tab`/`render_alt_tab_overlay`: live preview tiles via
  `snapshot_surface`, tokenized selection via `derive_accent`.

**raeen-ui:** confirm `raeui::tokens`/`rae_tokens` expose `elev_focus`,
`derive_accent`, and the `MOTION_*` curves to the shell (they do). No new tokens.

**raeen-accessibility (flag now):** focus-ring legibility in overview at couch
distance; reduced-motion paths for all four animations; the 48px couch hit-target
floor for thumbnails/tiles; confirm `accent.base`-ring contrast ≥3:1 against the
busiest wallpaper a thumbnail might sit on.

**raeen-input (blocked dependency):** 3-finger trackpad gestures for
overview/spaces — PARITY §O is `[ ]`; ship hotkeys first, wire gestures when the
gesture stream lands.

### Boot-log smoketest lines (R10, FAIL-able)

Each is a deterministic boot smoketest that can print `FAIL`. Compositor-side
lines go in `compositor::run_boot_smoketest`; policy/shell-side go in
`wm_policy::run_boot_smoketest` (extend the existing one at `wm_policy.rs:158`)
or a new `spaces::run_boot_smoketest`.

1. **Overview scaled composite (kernel/gfx).** Create 4 kernel test surfaces,
   enter overview mode, assert each surface's `overview_dst` equals the
   `compute_layout(Tile,…)` cell rect (aspect-fit), then exit and assert
   `overview_dst == None`:
   `[compositor] overview smoketest: cells_match=<bool> dst_cleared=<bool> -> PASS|FAIL`

2. **snapshot_surface downscale (kernel/gfx).** Paint a known 2-color pattern
   into a test surface, `snapshot_surface` it into a 16×16 buffer, assert the
   downscaled corners carry the source colors:
   `[compositor] snapshot smoketest: src_topleft=<hex> dst_topleft=<hex> match=<bool> -> PASS|FAIL`

3. **Spaces membership + visibility (shell/policy).** Two spaces, three test
   surfaces (2 in space A, 1 in space B); switch to B and assert exactly the
   space-A surfaces went `visible=false` and B's went `visible=true`:
   `[spaces] smoketest: a_hidden=<n>/2 b_shown=<n>/1 current=<idx> -> PASS|FAIL`

4. **Move-window-to-space (shell).** Move surface X from A to B; assert
   membership moved and visibility followed the active space:
   `[spaces] move smoketest: removed_from_a=<bool> added_to_b=<bool> visible_consistent=<bool> -> PASS|FAIL`

5. **Snap Group save/restore (shell, builds on wm_policy).** Save a 3-window
   Tile group, scramble origins via Float, restore, assert each window returned
   to its `compute_layout(Tile,…)` cell origin (reuses the existing live-apply
   assertion shape at `wm_policy.rs:186`):
   `[snapgroup] smoketest: saved=3 restored=3 origins_match=<bool> -> PASS|FAIL`

6. **App-switcher tokenized selection (shell).** Open the switcher with 3
   surfaces; assert the selected tile's ring color equals
   `derive_accent(active_accent).base` (NOT the old `0xFF_4E_9C_FF` literal —
   prove the hardcode is gone) and a live thumbnail was sampled (nonzero pixels):
   `[switcher] smoketest: ring=<hex> matches_accent=<bool> thumb_nonzero=<bool> -> PASS|FAIL`

Each line must be reachable in QEMU CI (no iron dependency) so it gates every
boot; downgrade any to `[~]` until its compositor delta lands.

### Unblocks PARITY_MATRIX §A lines

- "Overview / expose `[ ]` → `[~]`" (smoketests 1–2)
- "Virtual desktops / Spaces `[ ]` → `[~]`" (smoketests 3–4)
- "Snap Groups / restore `[ ]` → `[~]`" (smoketest 5)
- "App switcher `[~]` → upgraded" (smoketest 6)
- Partially advances "Snap / tile layouts `[~]`" (Snap Layouts hover flyout) —
  full `[x]` still blocked on the client-resize protocol (separate item).

---

## Visual-QA checklist (raeen-visual-qa)

Screenshot and verify on iron / host-render / QEMU window (per memory
`ui-glass-design-system`: headless QEMU screendump striping is a capture
artifact, not the render):

- Overview grid: 4+ windows as non-overlapping aspect-correct thumbnails, equal
  `space.4` gutters, `radius.md` corners, `elev.2` resting shadow; the selected
  thumbnail shows the `accent.base` ring + `accent.glow` — and the accent matches
  the taskbar/Start accent in the *same* screenshot (cohesion proof).
- Space switch: per-space wallpaper actually differs and cross-fades; windows of
  the inactive space are gone (not just behind).
- Switcher: glass panel (blur + top `stroke.strong` highlight), live thumbnails
  (not text), selection ring = current Vibe accent (swap Vibe Mode → the ring
  color changes with it).
- Snap Group restore: the three windows return to the saved grid positions.
- Reduced-motion on: confirm no scale/slide, only opacity cross-fades.
- Couch mode: focus glow legible at distance; tiles ≥48px.
