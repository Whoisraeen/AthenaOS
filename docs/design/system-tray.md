# Design Spec: System Tray / Status Area

> *"Built for people who care about how things feel."* — LEGACY_GAMING_CONCEPT.md
>
> The right edge of the taskbar — the always-on status cluster (network, volume,
> battery, clock) plus app-owned tray icons. It must clear: **macOS Sequoia/26's
> menu-bar extras (clean glyphs, click-for-popover, drag-to-reorder, Control
> Center entry) and Windows 11 24H2's tray + overflow flyout + per-icon context
> menus** — without Win11's cramped hidden-icon chevron confusion or macOS's
> menu-bar overcrowding.

**All tokens below are defined in [`design-language.md`](./design-language.md)**
and live in the `ath_tokens` crate (ADR 0003). This spec only assigns them; it
introduces no new magic numbers.

This spec **owns the tray as a surface** and supersedes the one-bullet sketch in
[`desktop-shell.md`](./desktop-shell.md) §1 ("tray cluster") and §3 ("tray
icons"). The tray *opens* Control Center and the Notification Center; those
flyouts are owned by [`control-center.md`](./control-center.md) and
[`notifications.md`](./notifications.md) respectively — this spec owns the
**cluster, the icons, their states, overflow, and the click model**.

---

## Concept promise + bar to clear

> "The user owns the machine… no ads, no bloat" + "Unified Settings — every option
> searchable." — LEGACY_GAMING_CONCEPT.md. The tray is the machine's *status at a glance*
> and the launch point for owning that status.

- **Bar to clear:**
  - **macOS Sequoia/26 menu-bar extras** — crisp monochrome glyphs that brighten on
    hover; click opens a focused popover (Wi-Fi list, volume slider, battery
    detail); `Cmd`-drag to reorder or remove; Control Center is a fixed entry; the
    clock is calm and right-most.
  - **Windows 11 24H2** — a status cluster (network/volume/battery as one clickable
    group → Quick Settings), an **overflow flyout** (the `^` chevron) for app tray
    icons, per-icon left-click (primary) and right-click (context menu), and the
    clock+calendar at the far right.
- **The AthenaOS-specific promise:** the tray cluster is **one click to Control
  Center** (the gaming/RGB/power fast lane lives there) and **one click to the
  Notification Center** (zero-amnesia history) — the two right-edge flyouts the
  whole shell shares material with. No mystery icons: every tray glyph has a
  tooltip and a known action, and overflow is *opt-in clarity*, not a junk drawer.

---

## Already built (delta only — verify-before-spec)

Grounded in `components/athshell/src/lib.rs` + `system_tray_daemon` (read, not
assumed).

| Piece | Where | Today | This spec adds |
|---|---|---|---|
| `SystemTray` / `TrayIcon` | `athshell::SystemTray` (`lib.rs:477+`), `add_icon`/`remove_icon`/`set_clock`/`total_width`/`render` | LIVE: a flat `Vec<TrayIcon>` of glyphs + tooltip + `active` flag, `TRAY_ICON_SIZE=32`, `space.1` gaps, clock right-aligned (`type.caption`, AA RaeSans) | hover/active/focus **state threading**, click model, overflow flyout, per-icon popover/context menu, two-line clock |
| Tray daemon | `athshell::system_tray_daemon` | module exists | binds **live** status (net/volume/battery) to icon glyphs + state |
| Clock | `set_clock` + `tray_clock_string` | single time line, `text.primary` | two-line time/date (`desktop-shell.md` §1), click → calendar/Notification-Center |
| Status sources | kernel net (`rtl8125`/DHCP), audio (HDA master), battery (`battery`/EDID power), capture (`capture.rs` `state.danger` dot) | LIVE subsystems | the **icon-state mapping** (signal bars, mute slash, charge %, recording dot) |
| Control Center / Notification Center triggers | `control-center.md` §1 (`Super+A`), `notifications.md` §3 (clock click) | flyouts spec'd | the tray is the *pointer* trigger for both |
| Glass / accent / soft shadow | `compositor.rs`, `proof_accent()` | LIVE | popovers use `material.glass` + soft `elev.2` |

**The honest gap (from `render` itself):** the code comment says *"hover →
text.primary is driven by input state the shell does not yet thread into the
tray"* and *"Two-line time/date once the kernel passes a date string."* So the
**delta is real**: thread hover/active/focus into the tray, add the overflow +
click model + popovers, and map live status into glyph state. **Not a rebuild** —
the cluster, icon model, and clock exist.

---

## Prior art distilled (current systems, 2024–2025)

- **macOS Sequoia/26 menu bar:** monochrome glyphs, ~22px tall, even spacing;
  hover does almost nothing (click is the verb); click → a *popover* anchored under
  the glyph (Wi-Fi network list, a volume slider, battery breakdown); `Cmd`-drag to
  reorder/remove; Control Center is a fixed cluster entry; the clock is the calm
  right anchor. **Take:** monochrome resting glyphs, click→anchored popover, the
  Control Center entry, drag-to-customize, calm clock. **Avoid:** menu-bar
  overcrowding with no overflow (AthenaOS has an overflow flyout); the fact that
  third-party extras can hide off-screen with no recovery.
- **Windows 11 24H2 tray:** the network+volume+battery glyphs are **one clickable
  group** that opens Quick Settings; the clock+date (two lines) opens the
  calendar/notification flyout; app icons live in a visible row with a `^`
  **overflow chevron** for the rest; left-click = primary action, right-click =
  per-icon context menu. **Take:** the status-group-as-one-Control-Center-button,
  two-line clock, the overflow chevron, the left/right click split. **Avoid:** the
  overflow being a confusing "where did my icon go" drawer (AthenaOS labels overflow
  clearly and lets the user pin/unpin from it); the cramped fixed widths.
- **GNOME 46+ top bar:** a single "system status" button (net/volume/battery)
  opens one Quick Settings popover; clock+calendar centered; very few standalone
  app icons (apps use the Quick Settings or notifications instead). **Take:** the
  consolidated status button (AthenaOS's status cluster → Control Center mirrors
  this). **Avoid:** removing app tray icons entirely — AthenaOS keeps them (Windows
  switchers expect them) but with overflow discipline.
- **SteamOS/couch:** the status area must read at 3m and be d-pad navigable; no
  hover. **Take:** every tray affordance has a focus equivalent with `elev.focus`;
  couch mode uses the 48px hit floor. **Avoid:** desktop density at the couch.

**AthenaOS synthesis:** macOS's **monochrome glyphs + click→anchored popover +
drag-to-customize**, Win11's **status-group→Control-Center + two-line clock +
overflow chevron + left/right click split**, GNOME's **consolidated status
button**, on the shell's `material.glass` popovers with one shared accent.

---

## AthenaOS design tokens this surface uses

Pulled verbatim from `design-language.md` / `ath_tokens`. No new magic numbers.

- **spacing:** `space.1` (intra-icon gap — already used), `space.2` (cluster edge
  inset, popover padding), `space.3` (status-group internal gap, clock inset),
  `space.4` (popover content padding).
- **radius:** `radius.xs` (icon hover/focus chip, overflow chevron), `radius.sm`
  (small popovers e.g. volume slider), `radius.md` (the overflow flyout, per-icon
  context menu), `radius.pill` (the volume/brightness slider track in a popover).
- **elevation:** `elev.1` (the taskbar the tray sits on), `elev.2` (popovers /
  overflow flyout / context menu — transient glass), `elev.focus` (focused icon
  glow). All soft per [`material-and-shadow.md`](./material-and-shadow.md).
- **type:** `type.caption` (clock, tooltip, icon popover labels — already used for
  clock), `type.label` (popover headers, context-menu items), `type.body`
  (popover list rows e.g. Wi-Fi network).
- **accent model:** seed = `ThemeAbi.accent_argb`; `ath_tokens::derive_accent`.
  Active/connected icon (e.g. Wi-Fi connected, recording) tints `accent.text` or
  `state.*`; hover/focus = `bg.elevated` chip + `accent.base` ring + `accent.glow`.
  Resting glyph = `text.secondary` (already), full `text.primary` on hover. **No
  private accent.**
- **material:** popovers/overflow/context-menu = `material.glass` (small transient
  surfaces — the Acrylic doctrine's case). The tray *cluster itself* sits on the
  taskbar's `material.mica` (static) — it does not add its own blur layer.
- **motion:** `motion.fast` (popover/overflow open), `motion.exit` (close),
  `motion.micro` (icon hover/press, glyph state change e.g. signal bars updating),
  `motion.instant` (reduced-motion).

---

## 1. The cluster layout (right → left within the tray region)

The tray occupies the right end of the 44px taskbar (`desktop-shell.md` §1).
Order, right edge inward:

1. **Clock + date** (far right, `space.2` inset) — two lines: time (`type.caption`
   `text.primary`) over date (`type.caption` `text.tertiary`), right-aligned. The
   delta from today's single line. Click → opens the **Notification Center**
   (`notifications.md` §3) with a mini-calendar at its top (the Win11/macOS clock
   behavior).
2. **Status group** — network, volume, battery (when a battery is present), as a
   **single clickable group** with `space.1` internal gaps. Click anywhere in the
   group → opens **Control Center** (`control-center.md` §1). Each glyph also
   supports a **direct popover** on a deliberate single-icon click (see §3) for the
   one-reach case (e.g. click the volume glyph → just a volume slider popover, no
   full Control Center) — Control Center is the *group* click; the icon popover is
   the *precise* click.
3. **Dynamic system icons** — transient status the OS owns: a **recording/capture
   dot** (`state.danger`, from `capture.rs`), a **DND/Focus** badge (from
   `notify.rs` `quick_settings::dnd_enabled`), an **update-ready** glyph. Shown only
   when active; never a permanent slot.
4. **App tray icons** — app-owned (`add_icon`): the visible row holds up to **4**
   (local layout constant), the rest go to the **overflow flyout** (§4) behind a
   `^` chevron. Each app icon: left-click = primary action (app-defined), right-click
   = context menu (§5), hover = tooltip.

All icons are 32px hit targets (`TRAY_ICON_SIZE`, already), `space.1` gaps,
`text.secondary` resting.

---

## 2. Status-icon state mapping (the live binding — delta)

The tray daemon maps live subsystem state to glyph + tone. Each is its own small
state machine; all FAIL-testable (`system-tray.md` smoketest):

| Icon | Source | Glyph states | Tone |
|---|---|---|---|
| **Network** | kernel net / DHCP (`rtl8125`, lease state) | wired-up / Wi-Fi signal bars (0–4) / disconnected (slash) / connecting (pulse) | connected = `text.secondary`; disconnected = `text.tertiary`; error = `state.warn` |
| **Volume** | HDA master + mute | speaker with 0–3 waves by level / muted (slash) | resting `text.secondary`; muted = `text.tertiary` |
| **Battery** | `battery` subsystem (present-only) | fill level (0–100%) / charging bolt / low (≤15%) | normal `text.secondary`; charging `state.ok` accent; low `state.danger` |
| **Recording** | `capture.rs` | dot, visible only while capturing/recording | `state.danger` (pulses `motion.micro`) |
| **DND/Focus** | `notify.rs` `dnd_enabled` | crescent/focus glyph, visible only when on | `accent.text` |

- **Update cadence:** the daemon repaints an icon only on *state change*, not per
  frame (memory `iron-console-logging-tax` — the tray must not tax the compositor).
  A signal-bar change animates `motion.micro`; reduced-motion = instant swap.

---

## 3. Per-icon click model (the precise-vs-group distinction)

- **Status group click** (network/volume/battery region as a whole) → **Control
  Center** (`control-center.md`). This is the discoverable, everything-here path.
- **Single status-icon click** (a deliberate click on *just* one glyph) → a small
  **anchored popover** for that one thing (`material.glass`, `radius.sm`, `elev.2`,
  anchored under the glyph at `space.2`):
  - **Volume** → a single horizontal slider (control-kit Slider) + mute toggle.
  - **Network** → a compact network list (top 4 + "More… → Control Center").
  - **Battery** → charge %, time-remaining, a "Battery settings" link.
  - These popovers are *subsets* of the Control Center modules — same widgets, same
    accent (`control-center.md` reuse). They exist for the one-reach case so the
    user isn't forced into the full flyout to nudge volume.
- **App-icon left-click** → the app's declared primary action (capability-gated IPC).
- **App-icon / status-icon right-click** → context menu (§5).
- **Clock click** → Notification Center (§1).

Disambiguation rule: the group→Control-Center click is the default for a click that
lands in the gap *between* status glyphs; a click squarely on a single glyph opens
its popover. (Keyboard/controller pick the popover explicitly — see §6.)

---

## 4. Overflow flyout (the visible junk-drawer fix — delta)

When app tray icons exceed the visible cap (4), a `^` **chevron** appears at the
left edge of the app-icon row.

- Click → a `material.glass` `radius.md` `elev.2` flyout anchored above the
  chevron, showing the overflowed icons as a small grid (each 36px, `radius.xs`
  hover, glyph + a `type.caption` label so nothing is a mystery).
- Each overflow item: left-click = primary action, right-click = context menu, and
  a **pin** affordance (drag into the visible row, or a context-menu "Show in
  tray") so the user *owns* what's visible (Concept ownership principle) — unlike
  Win11's opaque chevron.
- **Empty overflow:** the chevron is hidden entirely (no empty drawer).
- **States:** flyout open = `motion.fast` scale 96%→100% + fade; item hover
  `bg.elevated`; focus ring + glow; close on click-away / `Esc` / `motion.exit`.

---

## 5. Context menu (right-click — delta)

Right-click any tray icon → a `material.glass` `radius.md` `elev.2` context menu:

- App icons: app-declared menu items (`type.label` rows, `radius.xs` hover) + a
  fixed footer ("Hide from tray" / "Show in tray", "App settings").
- Status icons: a short menu (e.g. Network → "Wi-Fi settings", "Forget network…";
  Volume → "Sound settings", "Output device…").
- **States:** row hover `bg.elevated`; row press `accent.subtle` flash; focus ring;
  disabled item `text.tertiary` no hover; submenu opens on hover/`→`.
- **reduced-motion:** menu appears instantly.
- **`Cap`-gating:** app context-menu actions route through the app's
  capability-checked IPC; the tray never bypasses AthGuard.

---

## 6. States & keyboard / controller navigation

### Per-icon state matrix
- **default:** monochrome glyph `text.secondary` over taskbar mica.
- **hover (pointer):** `bg.elevated` chip @ `radius.xs` + glyph → `text.primary`,
  `motion.micro` (this is the threading the current `render` comment flags as
  missing). Tooltip (`material.glass` mini, `type.caption`) after ~500ms.
- **active (popover/menu open from this icon):** `accent.subtle` chip + glyph
  `accent.text`.
- **focus (keyboard/controller):** 2px `accent.base` ring + `elev.focus` glow.
- **disabled** (e.g. no battery hardware → battery icon absent, not greyed): not
  rendered rather than disabled.
- **dark/light:** glyph tones + popover glass per palette.
- **reduced-motion:** instant state swaps, no chip fade, no signal-bar animation.

### Keyboard / controller map
- `Super+B` — focus the tray cluster (from `desktop-shell.md` §1); arrow keys move
  between icons; `Enter` opens the focused icon's popover; `Menu`/`Shift+F10` opens
  its context menu; `Esc` returns focus to the desktop.
- `Super+A` — Control Center (status-group equivalent, `control-center.md`).
- `Super+N` — Notification Center (clock equivalent, `notifications.md`).
- **Controller (couch):** tray icons use the 48px hit floor; d-pad navigates the
  cluster, `A` opens the popover, `Y` the context menu, `B` closes; `elev.focus`
  glow reads at 3m (SteamOS lesson). Couch mode may consolidate the whole cluster
  into a single "System" button → Control Center (density reduction).

---

## 7. Empty / degraded states

- **No app tray icons:** the app-icon row + chevron are absent; only the status
  group + clock show. The tray is never empty (clock always present).
- **No network hardware:** network glyph absent (not a permanent error glyph).
- **No battery (desktop):** battery glyph absent.
- **Status source error** (e.g. net driver fault): glyph → `state.warn` with a
  tooltip naming the issue — degraded, legible, never silent.

---

## 8. Accessibility (in scope from the start)

| Concern | Rule | Owner |
|---|---|---|
| Contrast | resting glyph `text.secondary` ≥4.5:1 on mica; state glyphs (`state.*`) ≥3:1; clock `type.caption` ≥4.5:1 | athena-accessibility |
| Focus visibility | every icon/popover/menu item is **never color-only**: `accent.base` ring + `elev.focus` glow | athena-accessibility |
| Tooltips | every icon has a tooltip + a non-pointer label (no mystery glyphs) | athena-accessibility |
| Reduced-motion | no chip fades, no signal-bar/recording-dot animation; instant popovers | athena-accessibility |
| Hit targets | 32px pointer (already `TRAY_ICON_SIZE`) / 48px couch | athena-visual-qa |
| Keyboard-complete | reach + open every icon's popover and context menu with no pointer | athena-accessibility |

Flag to **athena-accessibility:** confirm the network signal-bar and battery-level
glyphs are distinguishable without color (shape/fill, not hue alone) for
color-vision-deficient users; confirm the recording dot is announced, not only
shown.

---

## 9. Cohesion acceptance (the whole-surface test)

Ships only when:
1. **Same accent:** active-icon tint, hover ring, and the popovers read the *same*
   `accent.base` as the taskbar / Start / Control Center / Notification Center —
   one Vibe preset switch recolors them together.
2. **Same material/radii/shadow:** popovers/overflow/context-menu are
   `material.glass` `radius.md` + *soft* `elev.2` (post `material-and-shadow.md`),
   identical to Control Center's sub-panels.
3. **No mystery icons:** every glyph has a tooltip + a known action; overflow is
   labeled and pin-able.
4. **Live binding works:** network/volume/battery glyphs reflect real state.
5. **Dark + light parity** with passing contrast.

---

## Handoff

### Implementer
- **athena-shell-apps — primary owner.** `athshell::SystemTray` (`lib.rs`) +
  `system_tray_daemon`: thread hover/active/focus state into `render` (the comment
  flags this as the missing piece), add the two-line clock, the click model (§3
  group-vs-icon), the overflow flyout (§4), per-icon popovers (subsets of Control
  Center widgets, §3), and the right-click context menu (§5). The daemon maps live
  net/volume/battery/capture/DND state to glyph + tone (§2), repainting only on
  state change (compositor-latency doctrine). Wire the status-group click →
  `control-center` and the clock click → `notify::toggle_center`.
- **athena-ui** — reuse the Control Center control-kit widgets (Slider, list rows)
  for the icon popovers; expose the **anchored-popover** + **overflow-flyout** +
  **context-menu** as reusable `ath_tokens`-consuming containers (shared with
  other shell surfaces).
- **kernel (status sources)** — expose the live status the daemon reads: net link/
  signal/lease, HDA master level + mute, battery %/charging, capture-active, DND.
  Most exist; confirm a poll/notify path the daemon can read without per-frame cost.
- **athena-accessibility (flagged)** — color-independent network/battery glyphs;
  recording-dot announcement; keyboard-complete popover/menu reach; reduced-motion.

### FAIL-able boot-log proof lines
A `system_tray` (or `athshell`) `run_boot_smoketest` driving synthetic status:

```
[tray] state smoketest: net=connected(bars=3) vol=muted batt=charging(82%) -> glyphs OK -> PASS
[tray] click smoketest: group_click->control_center=1 vol_icon_click->slider_popover=1 clock_click->notify_center=1 -> PASS
[tray] overflow smoketest: app_icons=6 visible=4 chevron_shown=1 overflow_count=2 -> PASS
[tray] context smoketest: rightclick app=N items, status=N items, nocap_action_hidden=1 -> PASS
[tray] accent smoketest: active_icon_tint=0x.. == derive_accent(active_accent).text -> PASS
```

(FAIL if a glyph state diverges from the synthetic source, if the group click does
not open Control Center, if overflow miscounts, if a no-capability app action still
renders, or if the active-icon tint diverges from the live accent.)

### Visual-QA verification list (athena-visual-qa)
Verify on iron / host-render / QEMU window (headless screendump striping is a
capture artifact — memory `ui-glass-design-system`):
- Screenshot: the full cluster — network/volume/battery glyphs, dynamic icons,
  app icons, two-line clock — resting `text.secondary`, on taskbar mica.
- Screenshot: an icon **hovered** (chip + `text.primary`) and an icon **focused**
  (ring + glow) — proving the threaded state.
- Screenshot: a single volume-icon click → the anchored slider popover (glass,
  soft `elev.2`).
- Screenshot: the status-group click → Control Center open (cohesion: same accent
  as the tray in the same frame).
- Screenshot: the clock click → Notification Center open.
- Screenshot: the overflow chevron + its flyout with labeled icons.
- Screenshot: a right-click context menu (app + status variants).
- Screenshot: live state — muted volume (slash), charging battery (bolt),
  disconnected network (slash), recording dot (danger).
- Cohesion: before/after one Vibe preset — active-icon tint + popover accent change
  with the taskbar/Control Center.
- Reduced-motion on: no chip fade, no signal-bar animation, instant popovers.

### Unblocks (MasterChecklist)
- **Phase 14 (AthShell + apps):** the status area from a flat glyph list to a
  full macOS/Win11-rival tray (states, overflow, popovers, context menus, live
  binding).
- **Phase 8 (AthUI/AthKit):** the anchored-popover + overflow-flyout + context-menu
  reusable containers.
- **Phase 13 (Customization):** accent-coherent tray + the Control Center fast lane
  one click from the right edge.
