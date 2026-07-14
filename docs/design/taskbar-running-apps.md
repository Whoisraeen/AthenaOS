# Design Spec: Taskbar Running-App Affordances (previews · jump lists · indicators · badges)

> *"Built for people who care about how things feel."* — LEGACY_GAMING_CONCEPT.md
>
> The taskbar's running-app *interaction model* — what a button tells you about an
> app (running / focused / how many windows / unread count) and what it lets you do
> (hover-preview each window, right-click a jump list, drag to pin/reorder). It must
> clear: **macOS Sequoia/26 Dock (running dots, click-to-cycle, drag-to-pin, app
> context menu with window list) and Windows 11 24H2 taskbar (per-window hover
> thumbnails, grouped buttons, jump lists, badge counts, drag-to-reorder)** —
> without Win11's flaky never-combine modes or the Dock's hidden window-management.

**All tokens below are defined in [`design-language.md`](./design-language.md)**
and live in the `rae_tokens` crate (ADR 0003). This spec only assigns them; it
introduces no new magic numbers. Button sizes (36px) are local layout constants
from `desktop-shell.md` §1, not new global tokens.

This spec **deepens** [`desktop-shell.md`](./desktop-shell.md) §1 (which spec'd the
taskbar *geometry* + the per-button state matrix) by owning the **running-app
interaction model** — previews, jump lists, indicator language, badges, grouping,
and drag-to-pin — which §1 left as a single "running indicator" bullet. It is the
dock/taskbar sibling to [`window-management.md`](./window-management.md) §4 (the
Alt-Tab switcher): the switcher is the *keyboard* cycle, this is the *pointer +
right-click* model on the bar itself. They share the `snapshot_surface` thumbnail
primitive that `window-management.md` §1 introduces.

---

## Concept promise + bar to clear

> "Familiar enough to switch from Windows or Mac in 10 minutes." — LEGACY_GAMING_CONCEPT.md
> (§Three User Experiences) + "The user owns the machine" (you pin and arrange
> what *you* use).

- **Bar to clear:**
  - **macOS Sequoia/26 Dock** — a running app shows a dot under its icon; click
    cycles/raises its windows; right-click (or click-hold) opens a menu listing the
    app's open windows + "New Window" + "Keep in Dock" + "Quit"; drag an icon to
    pin or reorder; minimized windows genie into the Dock's right side.
  - **Windows 11 24H2 taskbar** — hover a button → live **per-window thumbnail
    previews** (a row of small live captures with close ×); grouped buttons when an
    app has multiple windows; **jump lists** on right-click (recent files, app
    tasks, pin/unpin); **badge counts** (unread) on the icon; drag-to-reorder + pin.
- **The AthenaOS-specific promise:** the running indicator + previews use the **same
  live `snapshot_surface` buffers** the overview/switcher use (zero re-render,
  compositor-cheap — `window-management.md` §1), the jump list is **capability-aware**
  (only actions the app actually granted), and what's pinned/arranged is **yours**
  (Concept ownership) — no algorithmic "recommended" churn reordering your bar.

---

## Already built (delta only — verify-before-spec)

Grounded in `components/raeshell/src/lib.rs` (read, not assumed).

| Piece | Where | Today | This spec adds |
|---|---|---|---|
| `TaskbarItem` | `raeshell::TaskbarItem` (`lib.rs:205`) | LIVE: `title`, `surface_id`, `focused`, `minimized`, `icon_char` — one item per window | the **grouped button** model (N windows → one button), the indicator language, hover-preview, jump list, badge |
| Taskbar render + states | `desktop-shell.md` §1 + `lib.rs` render path (`TASKBAR_HEIGHT=44`, 36px buttons, `material.mica`, accent indicator bar) | LIVE geometry + hover/active/focus/disabled states | the **running-indicator semantics** (count/focus), preview popover, jump-list popover, drag-to-pin |
| Pinned apps | `AppEntry { pinned, launch_count }` (`lib.rs:217`) | LIVE pinned + launch-count model in the Start menu | drag-from-Start / drag-on-bar to pin to the taskbar; pin/unpin from the jump list |
| Live window buffers + snapshot | `compositor.rs` `Surface.kernel_ptr`; `snapshot_surface` introduced by `window-management.md` §1 | buffers LIVE; `snapshot_surface` is the §4 switcher's primitive | **reused** for the hover-preview thumbnails (same one-shot box-downscale) |
| App-switcher | `window-management.md` §4 (`cycle_alt_tab`) | spec'd (live-preview tiles, tokenized) | the taskbar is the *pointer* counterpart; shares the thumbnail primitive |
| Glass / accent / soft shadow | `compositor.rs`, `proof_accent()` | LIVE | preview/jump-list popovers = `material.glass` + soft `elev.2` |

**The honest gap:** `TaskbarItem` is **one-item-per-window** with no grouping, no
preview, no jump list, no badge, and the indicator is a single accent bar
(`desktop-shell.md` §1) with no count/multi-window semantics. **Delta is real but
bounded** — it reuses the existing pinned model, the existing indicator bar, and
the `snapshot_surface` primitive the switcher already needs. **Not a rebuild.**

---

## Prior art distilled (current systems, 2024–2025)

- **macOS Sequoia/26 Dock:** running = a small dot; click raises/cycles; **app
  context menu** (right-click / long-press) lists open windows by title + app tasks
  + "Keep in Dock" + "Options" + "Quit"; drag-to-pin and drag-to-reorder are
  fluid; minimized windows live separately on the Dock's right with a live
  thumbnail. **Take:** the running dot, the window-list context menu, drag-to-pin,
  click-to-cycle. **Avoid:** the Dock *hiding* window management (you can't see
  per-window previews without Mission Control) — AthenaOS adds hover previews.
- **Windows 11 24H2:** hover → a **flyout of live per-window thumbnails** (with a
  close × on each and an app title header); buttons **group** an app's windows
  (with a "combine" setting); **jump lists** (right-click) show pinned/recent files
  + app tasks + pin/unpin/close; **badge counts** for unread (e.g. mail, chat); drag
  to reorder + pin. **Take:** per-window hover thumbnails with close ×, grouped
  buttons, jump lists, badge counts, drag-to-reorder. **Avoid:** the historically
  flaky "never combine / show labels" toggles and the "recommended" churn —
  AthenaOS: grouping is consistent, ordering is user-owned.
- **GNOME Dash / KDE Task Manager:** dash shows running dots; KDE offers
  thumbnails-on-hover and grouping options, proving these are achievable as policy.
  **Take:** thumbnails-on-hover as policy; running-dot legibility. **Avoid:** GNOME
  hiding the dash off the desktop by default (AthenaOS taskbar is always-on, Win11
  familiar).
- **SteamOS/couch:** the running surface must be d-pad navigable with a 3m-legible
  focus glow; no hover. **Take:** every preview/jump-list affordance has a focus
  equivalent (`elev.focus`); couch uses 48px floor. **Avoid:** desktop density.

**AthenaOS synthesis:** Win11's **per-window hover thumbnails + jump lists + badges
+ drag-to-reorder** and macOS's **running dot + window-list context menu +
click-to-cycle + drag-to-pin**, on live `snapshot_surface` buffers, capability-aware
jump lists, and user-owned ordering — one accent, one glass.

---

## AthenaOS design tokens this surface uses

Pulled verbatim from `design-language.md` / `rae_tokens`. No new magic numbers.

- **spacing:** `space.1` (button gaps — already), `space.2` (preview-tile gap,
  popover padding), `space.3` (jump-list row inset), `space.4` (popover content
  padding).
- **radius:** `radius.sm` (taskbar buttons — already), `radius.md` (preview-flyout
  + jump-list popover, preview tiles), `radius.xs` (jump-list rows, preview close ×,
  badge), `radius.pill` (the unread-count badge when numeric).
- **elevation:** `elev.1` (the taskbar — already), `elev.2` (preview flyout +
  jump-list popover — transient glass), `elev.focus` (focused button/tile glow).
  All soft per [`material-and-shadow.md`](./material-and-shadow.md).
- **type:** `type.label` (preview-flyout app header, jump-list section header),
  `type.caption` (per-window thumbnail title, jump-list rows, badge count),
  `type.body` (jump-list file names).
- **accent model:** seed = `ThemeAbi.accent_argb`; `rae_tokens::derive_accent`.
  Focused window indicator = full-width `accent.base`; preview-tile selection =
  `accent.subtle` + `accent.base` ring; badge = `accent.base` (or `state.danger`
  for urgent); hover/focus = `bg.elevated` + `accent.base` ring + `accent.glow`.
  **No private accent.**
- **material:** preview flyout + jump-list = `material.glass` (small transient
  surfaces — the Acrylic doctrine). The taskbar itself stays `material.mica` (static).
- **motion:** `motion.fast` (preview/jump-list open), `motion.exit` (close),
  `motion.micro` (button hover, indicator grow on focus, preview-tile hover, badge
  appear), `motion.standard` (drag-to-pin settle), `motion.instant` (reduced-motion).

---

## 1. Running-indicator language (delta — replaces the single bar)

A taskbar button's indicator (the strip under the icon, `desktop-shell.md` §1)
encodes **running + focus + window count** at a glance:

| State | Indicator | Notes |
|---|---|---|
| **Not running** (pinned only) | none | the icon resting `text.secondary` |
| **Running, unfocused, 1 window** | 12px stub bar, `text.tertiary` (already) | the at-rest running cue |
| **Running, focused, 1 window** | full-width bar, `accent.base` (already) | the focused app |
| **Running, multiple windows** | a **stacked/segmented bar** — N short segments (cap visual at 3 + "…") under the icon, `text.tertiary`; the focused window's segment is `accent.base` | the grouping cue (the macOS dot can't show count; this is better) |
| **Needs attention** (window requesting focus) | the bar pulses `state.warn` `motion.micro` until clicked | the Win11 flashing-button equivalent, calmer |

- **Grouping rule:** all windows of one app collapse to **one button** with the
  segmented indicator (consistent, not a toggleable mode — the Win11 "never combine"
  flakiness is the thing avoided). Hover reveals the per-window previews (§2);
  click cycles/raises (§4).

---

## 2. Hover-preview flyout (the Win11 win — delta)

Hover a running button (~400ms, or immediately if a preview is already open and you
move to an adjacent button) → a `material.glass` `radius.md` `elev.2` flyout
anchored above the button:

- **Header:** app icon + name (`type.label`).
- **Per-window tiles:** one tile per window of that app, in a horizontal row (wrap
  to 2 rows past 4). Each tile:
  - a **live thumbnail** via `snapshot_surface(id, …)` — the *same* one-shot
    box-downscale of `Surface.kernel_ptr` the switcher uses (`window-management.md`
    §4); refreshed when the flyout opens, not per frame (cheap, short-lived).
  - a window title (`type.caption`, 1-line clamp) below.
  - a **close ×** (top-right, `radius.xs`, on tile hover) → closes that window.
  - tile `radius.md`, 1px `stroke.subtle`; the focused window's tile shows the
    `accent.base` ring.
- **Click a tile** → raise + focus that specific window; flyout closes
  (`motion.exit`).
- **Minimized windows** show their last-frame thumbnail dimmed ~20% with a small
  "minimized" caption — restore on click.
- **Single-window apps** still get a flyout (one tile) for consistency + the close ×
  affordance; a `ThemeAbi`/setting may skip the flyout for single-window apps
  (macOS-like) — default = always show.

### States
- **flyout open:** `motion.fast` scale 96%→100% + fade + 4px up-translate.
- **tile hover:** `bg.elevated` lift + close × appears, `motion.micro`.
- **tile selected (kbd/controller):** `accent.subtle` + `accent.base` ring +
  `elev.focus`.
- **close:** click-away / `Esc` / pointer-leave (with a ~200ms grace so moving
  between buttons doesn't flicker) → `motion.exit`.
- **reduced-motion:** instant open/close; no tile scale; thumbnails still shown.

---

## 3. Jump list (right-click — delta, capability-aware)

Right-click a taskbar button → a `material.glass` `radius.md` `elev.2` jump-list
popover anchored above the button:

- **Sections (top → bottom):**
  1. **Tasks** — app-declared quick actions ("New Window", "New Private Window",
     app-specific). Each a `type.label` row, `radius.xs` hover.
  2. **Recent** — recent files/locations the app declared (`type.body` file name +
     `type.caption` path), capped at ~6. (Sourced from the app's recent list over
     its IPC; absent if the app declares none — never fabricated.)
  3. **Footer (fixed):** "Pin to taskbar" / "Unpin from taskbar", and (if running)
     "Close all windows".
- **Capability-aware (the AthenaOS delta):** Tasks and Recent only appear if the app
  granted the corresponding capability over its declared IPC; an app with no
  declared tasks/recents shows just the pin + close footer. The jump list **never
  fabricates** entries and never bypasses AthGuard.
- **States:** row hover `bg.elevated`; press `accent.subtle` flash; focus ring +
  glow; disabled `text.tertiary`; submenu on `→`/hover.
- **reduced-motion:** appears instantly.

---

## 4. Click model + drag-to-pin

- **Left-click (not running):** launch (the existing Start launch path).
- **Left-click (running, 1 window):** raise + focus; click again when focused →
  minimize (the Win11 toggle, also macOS-friendly).
- **Left-click (running, multiple):** raise the most-recent window; subsequent
  clicks **cycle** through the app's windows (with a brief `accent.base` ring on the
  raised one). The hover-preview flyout (§2) is the explicit picker.
- **Middle-click:** open a new window (if the app supports it).
- **Drag a button:** reorder within the taskbar (`motion.standard` settle); drag a
  Start-menu app onto the taskbar → **pin** it (sets `AppEntry.pinned`); drag a
  pinned button off the bar → unpin. Ordering persists per user (Concept ownership).

---

## 5. Badges (unread / progress — delta)

- An app may set a **badge** on its taskbar button over its IPC:
  - **count badge:** a `radius.pill` `accent.base` (or `state.danger` for urgent)
    pill at the icon's top-right, `type.caption` `text.primary`, showing a number
    (clamped "99+"). Appears `motion.micro`.
  - **progress badge:** a thin `accent.base` arc/bar along the icon's bottom for an
    indeterminate or 0–100% task (downloads, installs).
- **`Cap`-gated:** only an app with the badge capability can set one; no system
  fabrication.
- **reduced-motion:** badge appears instantly, no pulse.
- **Flag to raeen-accessibility:** count badges must be color-independent (the pill
  shape + number carry the meaning, not hue alone).

---

## 6. States & keyboard / controller navigation

### Per-button state matrix (extends `desktop-shell.md` §1)
- **default / hover / active / focus / disabled / dark / light / reduced-motion:**
  inherit `desktop-shell.md` §1; this spec adds the **indicator semantics** (§1),
  **preview-open** (button shows `accent.subtle` while its flyout is open), and
  **drag** (the dragged button lifts to `elev.2`, others slide to make room).

### Keyboard / controller map
- `Super+1..9` — launch/focus pinned app N (already, `desktop-shell.md` §1); a
  second press cycles that app's windows.
- `Super+B` then arrows — focus the taskbar; on a focused running button,
  `↑`/`Space` opens the **hover-preview flyout** as a keyboard picker (arrows move
  tiles, `Enter` raises, `Del` closes the window); `Menu`/`Shift+F10` opens the
  **jump list**.
- **Controller (couch):** buttons + preview tiles + jump-list rows use the 48px hit
  floor; d-pad navigates, `A` raises, `Y` opens the jump list, `X` opens previews,
  `B` closes; `elev.focus` glow reads at 3m. Couch mode may hide the bar entirely in
  favor of the overview (`window-management.md`) — couch is a mode.

Every hover affordance (preview flyout, jump list) has a keyboard + controller
equivalent (design-language §8 hover-independence). Focus is always `accent.base`
ring + `elev.focus`, never color-only — flag deviations to raeen-accessibility.

---

## 7. Empty / edge states

- **No running apps:** only pinned buttons (no indicators) + the Start pill + tray.
- **App with no declared tasks/recents:** the jump list shows only the pin + close
  footer (never fabricated entries).
- **Window with no paintable buffer yet** (just launched): the preview tile shows
  the app icon centered on `bg.elevated` until the first frame lands (no broken
  black tile).
- **Many windows** (>8 of one app): the preview flyout scrolls; the indicator caps
  its segments at 3 + "…".

---

## 8. Accessibility (in scope from the start)

| Concern | Rule | Owner |
|---|---|---|
| Contrast | indicator bars ≥3:1 on mica; preview titles ≥4.5:1; badge text ≥4.5:1 on its pill | raeen-accessibility |
| Focus visibility | every button/tile/jump-row is **never color-only**: `accent.base` ring + `elev.focus` glow | raeen-accessibility |
| Indicator legibility | running/focused/count distinguishable without color (length/segments, not hue alone) | raeen-accessibility |
| Badge legibility | count/progress carried by shape+number, not hue | raeen-accessibility |
| Reduced-motion | no indicator grow, badge pulse, drag-settle, or tile scale; instant popovers | raeen-accessibility |
| Hit targets | 36px buttons (≥32px floor) / 48px couch; preview close × ≥24px (≥32 couch) | raeen-visual-qa |
| Keyboard-complete | open previews + jump list, raise/close windows, pin/reorder — all with no pointer | raeen-accessibility |

Flag to **raeen-accessibility:** confirm the multi-window segmented indicator and
the focused-segment accent stay distinguishable for color-vision-deficient users,
and that "needs attention" pulse has a non-color cue.

---

## 9. Cohesion acceptance (the whole-surface test)

Ships only when:
1. **Same accent:** focused indicator, preview-tile ring, jump-list press, and
   badges read the *same* `accent.base` as the Start / tray / Control Center /
   Notification Center — one Vibe preset switch recolors them together.
2. **Same material/radii/shadow:** preview flyout + jump list are `material.glass`
   `radius.md` + *soft* `elev.2` (post `material-and-shadow.md`), identical to the
   tray popovers and Control Center sub-panels.
3. **Live previews are real:** thumbnails are the windows' actual last frames
   (`snapshot_surface`), not icons — same primitive as the switcher.
4. **Ownership:** pin/reorder persists; nothing reorders the bar on its own.
5. **Dark + light parity** with passing contrast.

---

## Handoff

### Implementer
- **raeen-shell-apps — primary owner.** Extend `raeshell::TaskbarItem`/the taskbar
  render path (`lib.rs`): group windows-per-app into one button, the §1 indicator
  semantics, the §2 hover-preview flyout (calling `snapshot_surface`), the §3
  capability-aware jump list, the §4 click/cycle + drag-to-pin (reusing
  `AppEntry.pinned`), and §5 badges. Repaint indicators only on state change
  (compositor-latency doctrine, memory `iron-console-logging-tax`).
- **raeen-gfx / kernel (compositor)** — `snapshot_surface(id, dst, w, h)` is the
  shared primitive (introduced by `window-management.md` §1); confirm it serves
  both the switcher and these previews under `lock_compositor()` (UAF-safe read of
  compositor-owned memory, memory `compositor-IF=0`). No new per-frame allocations.
- **raeen-ui** — expose the **preview-flyout** (live-thumbnail tile row) and the
  **jump-list popover** as reusable `rae_tokens`-consuming widgets (shared with the
  tray's overflow/context-menu containers, `system-tray.md`).
- **app-side IPC (raebridge / app SDK)** — the capability-checked channels by which
  an app declares jump-list tasks/recents and sets badges; the taskbar reads these,
  never fabricates them.
- **raeen-accessibility (flagged)** — color-independent indicators + badges;
  attention-pulse non-color cue; keyboard-complete preview/jump-list reach;
  reduced-motion.

### FAIL-able boot-log proof lines
A `taskbar` (or `raeshell`) `run_boot_smoketest` driving synthetic windows/apps:

```
[taskbar] group smoketest: app=Term windows=3 buttons=1 segments=3 focused_seg=1 -> PASS
[taskbar] preview smoketest: hover app=Term tiles=3 thumb_nonzero=1 close_x=1 -> PASS
[taskbar] jumplist smoketest: tasks=2 recents=4 nocap_section_hidden=1 pin_toggle=1 -> PASS
[taskbar] click smoketest: click1=raise click2(focused)=minimize multi_click=cycle -> PASS
[taskbar] badge smoketest: count=12 cap_required=1 nocap_badge_hidden=1 -> PASS
[taskbar] accent smoketest: focused_indicator=0x.. == derive_accent(active_accent).base -> PASS
```

(FAIL if multi-window apps don't group, if a preview thumbnail is all-zero, if a
no-capability jump-list section or badge still renders, if click-cycle doesn't
advance windows, or if the focused indicator diverges from the live accent.)

### Visual-QA verification list (raeen-visual-qa)
Verify on iron / host-render / QEMU window (headless screendump striping is a
capture artifact — memory `ui-glass-design-system`):
- Screenshot: indicator states side by side — not-running / running-unfocused /
  running-focused / multi-window segmented / needs-attention pulse.
- Screenshot: a hover-preview flyout with ≥2 **live** per-window thumbnails (real
  window content, not icons), titles, and a close × on a tile; focused tile ring.
- Screenshot: a jump list — Tasks + Recent + pin/close footer; and one for an app
  with **no** declared tasks (footer only — proving no fabrication).
- Screenshot: a count badge + a progress badge on taskbar buttons.
- Screenshot: a button mid-drag (lifted `elev.2`, others sliding) → pinned.
- Cohesion: before/after one Vibe preset — focused indicator + preview ring + badge
  change accent with the Start/tray in the same frame.
- Reduced-motion on: no indicator grow / badge pulse / drag-settle; instant
  popovers; thumbnails still present.

### Unblocks (MasterChecklist)
- **Phase 14 (AthShell + apps):** the running-app interaction model — from one-item-
  per-window buttons to grouped buttons with previews, jump lists, badges, and
  drag-to-pin (the dock/taskbar parity gap).
- **Phase 8 (AthUI/AthKit):** the preview-flyout + jump-list reusable widgets;
  shares `snapshot_surface` with the switcher.
- **Phase 13 (Customization):** user-owned pinning/ordering + accent-coherent
  indicators/badges.
